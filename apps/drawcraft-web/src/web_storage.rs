//! Connects [`StorageController`] to the browser: OPFS with an IndexedDB fallback through the
//! small `storage_adapter.js` shim. The workspace is never kept only in `localStorage`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use drawcraft_storage::{FlushOutcome, StorageError};
use drawcraft_ui::StorageIndicator;
use js_sys::Uint8Array;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;

use crate::storage_model::{StorageController, WORKSPACE_BUDGET, error_from_js};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = drawcraftStorage, catch)]
    async fn open() -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_namespace = drawcraftStorage, catch)]
    async fn get(key: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_namespace = drawcraftStorage, catch)]
    async fn put(key: &str, bytes: &Uint8Array) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_namespace = drawcraftStorage, catch)]
    async fn estimate() -> Result<JsValue, JsValue>;
}

const RETRY_DELAY_MS: i32 = 5_000;
const LEGACY_LOCAL_STORAGE_KEY: &str = "drawcraft_workspace_v1";

fn js_error(value: &JsValue) -> StorageError {
    let read = |field: &str| {
        js_sys::Reflect::get(value, &JsValue::from_str(field))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default()
    };
    error_from_js(&read("name"), &read("message"))
}

fn number(value: &JsValue, field: &str) -> u64 {
    js_sys::Reflect::get(value, &JsValue::from_str(field))
        .ok()
        .and_then(|v| v.as_f64())
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map_or(0, |n| n as u64)
}

/// Shared handle used by the UI hooks and the flush loop.
#[derive(Clone)]
pub struct WebStorage {
    controller: Rc<RefCell<StorageController>>,
    pumping: Rc<Cell<bool>>,
}

impl WebStorage {
    /// Opens the best available backend and loads the saved workspace.
    ///
    /// With no usable backend the app still runs and reports "Not saved". `None` is returned only if
    /// the controller cannot be built (an invalid constant key, which cannot happen).
    pub async fn open_and_load() -> (Option<Self>, Option<String>) {
        let backend = open()
            .await
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_else(|| "none".to_string());
        let Ok(mut controller) = StorageController::new(WORKSPACE_BUDGET, &backend) else {
            return (None, loaded_from_legacy_only());
        };
        if let Ok(est) = estimate().await {
            controller.apply_estimate(number(&est, "quota"), number(&est, "usage"));
        }
        let mut loaded = None;
        if backend != "none"
            && let Ok(value) = get(controller.key().as_str()).await
            && !value.is_null()
            && !value.is_undefined()
        {
            let bytes = Uint8Array::new(&value).to_vec();
            if controller.hydrate(bytes.clone()).is_ok() {
                loaded = String::from_utf8(bytes).ok();
            }
        }
        let storage = Self {
            controller: Rc::new(RefCell::new(controller)),
            pumping: Rc::new(Cell::new(false)),
        };
        if loaded.is_none() {
            loaded = legacy_local_storage_workspace();
            if let Some(json) = &loaded {
                // One-time migration of an earlier localStorage copy into durable storage.
                storage.save(json);
            }
        }
        (Some(storage), loaded)
    }

    /// Step one of a save: accept into the hot mirror, then start persisting.
    pub fn save(&self, json: &str) {
        self.controller.borrow_mut().save(json.as_bytes().to_vec());
        self.pump();
    }

    /// Current state for the toolbar label and the storage-full banner.
    pub fn indicator(&self) -> StorageIndicator {
        self.controller.borrow().indicator()
    }

    fn pump(&self) {
        if self.pumping.replace(true) {
            return;
        }
        let this = self.clone();
        spawn_local(async move {
            this.flush_loop().await;
            this.pumping.set(false);
        });
    }

    async fn flush_loop(&self) {
        loop {
            let batch = self.controller.borrow_mut().next_batch();
            let Some(batch) = batch else { break };
            let mut outcomes = Vec::with_capacity(batch.writes.len());
            for write in &batch.writes {
                let bytes = Uint8Array::from(&write.data[..]);
                let result = put(write.key.as_str(), &bytes)
                    .await
                    .map(|_| ())
                    .map_err(|e| js_error(&e));
                outcomes.push(FlushOutcome {
                    key: write.key.clone(),
                    revision: write.revision,
                    result,
                });
            }
            let needs_retry = self.controller.borrow_mut().finish(batch.id, outcomes);
            if needs_retry {
                self.schedule_retry();
                break;
            }
        }
    }

    fn schedule_retry(&self) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let this = self.clone();
        let callback = wasm_bindgen::closure::Closure::once_into_js(move || this.pump());
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            callback.unchecked_ref(),
            RETRY_DELAY_MS,
        );
    }
}

fn loaded_from_legacy_only() -> Option<String> {
    legacy_local_storage_workspace()
}

fn legacy_local_storage_workspace() -> Option<String> {
    let storage = web_sys::window()?.local_storage().ok()??;
    storage.get_item(LEGACY_LOCAL_STORAGE_KEY).ok().flatten()
}
