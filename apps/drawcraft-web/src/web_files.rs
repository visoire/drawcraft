use std::sync::{Arc, Mutex};

use drawcraft_ui::{AsyncFileEvent, FileDialogKind, PickedOpenFile};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::{JsFuture, spawn_local};

pub type SharedFileEvents = Arc<Mutex<Vec<AsyncFileEvent>>>;
pub type SharedEguiCtx = Arc<Mutex<Option<egui::Context>>>;

pub fn push_file_event(events: &SharedFileEvents, ctx: &SharedEguiCtx, evt: AsyncFileEvent) {
    if let Ok(mut guard) = events.lock() {
        guard.push(evt);
    }
    if let Ok(guard) = ctx.lock()
        && let Some(egui_ctx) = guard.as_ref()
    {
        egui_ctx.request_repaint();
    }
}

pub fn trigger_save_bytes(
    kind: FileDialogKind,
    default_name: &str,
    bytes: Vec<u8>,
    events: SharedFileEvents,
    ctx: SharedEguiCtx,
) {
    if let Some(promise) = try_start_save_picker(kind, default_name) {
        let fallback_name = default_name.to_string();
        spawn_local(async move {
            match JsFuture::from(promise).await {
                Ok(handle) => match write_bytes_to_handle(&handle, &bytes).await {
                    Ok(()) => push_file_event(
                        &events,
                        &ctx,
                        AsyncFileEvent::Saved {
                            message: format!("Saved `{fallback_name}` ({} bytes).", bytes.len()),
                        },
                    ),
                    Err(msg) => push_file_event(&events, &ctx, AsyncFileEvent::Error(msg)),
                },
                Err(err) => {
                    if is_user_abort_error(&err) {
                        push_file_event(&events, &ctx, AsyncFileEvent::Cancelled);
                    } else {
                        trigger_blob_download_and_notify(
                            kind,
                            &fallback_name,
                            &bytes,
                            &events,
                            &ctx,
                        );
                    }
                }
            }
        });
        return;
    }

    trigger_blob_download_and_notify(kind, default_name, &bytes, &events, &ctx);
}

pub fn trigger_open_file(kind: FileDialogKind, events: SharedFileEvents, ctx: SharedEguiCtx) {
    if let Some(promise) = try_start_open_picker(kind) {
        spawn_local(async move {
            match JsFuture::from(promise).await {
                Ok(handles_val) => {
                    let array = js_sys::Array::from(&handles_val);
                    if array.length() == 0 {
                        push_file_event(&events, &ctx, AsyncFileEvent::Cancelled);
                        return;
                    }
                    let handle = array.get(0);
                    match read_handle_bytes(&handle).await {
                        Ok((name, bytes)) => push_file_event(
                            &events,
                            &ctx,
                            AsyncFileEvent::Opened(PickedOpenFile::Bytes { name, bytes }),
                        ),
                        Err(msg) => push_file_event(&events, &ctx, AsyncFileEvent::Error(msg)),
                    }
                }
                Err(err) => {
                    if is_user_abort_error(&err) {
                        push_file_event(&events, &ctx, AsyncFileEvent::Cancelled);
                    } else {
                        trigger_input_file_picker(kind, events, ctx);
                    }
                }
            }
        });
        return;
    }

    trigger_input_file_picker(kind, events, ctx);
}

pub fn trigger_open_image_file(events: SharedFileEvents, ctx: SharedEguiCtx) {
    trigger_input_accept_picker(
        ".png,.jpg,.jpeg,.jfif,.webp,.gif,.bmp,.ico,.tif,.tiff,.tga,.qoi,image/png,image/jpeg,image/webp,image/gif,image/bmp,image/x-icon,image/tiff",
        None,
        events,
        ctx,
    );
}

/// Opens the device rear camera (or photo capture sheet on iPadOS Safari) for Photo / Document Scan.
pub fn trigger_camera_capture(events: SharedFileEvents, ctx: SharedEguiCtx) {
    trigger_input_accept_picker("image/*", Some("environment"), events, ctx);
}

pub fn install_clipboard_image_paste_listener(events: SharedFileEvents, ctx: SharedEguiCtx) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let target: &web_sys::EventTarget = window.as_ref();
    let closure = Closure::<dyn FnMut(web_sys::ClipboardEvent)>::new(
        move |event: web_sys::ClipboardEvent| {
            let Some(data) = event.clipboard_data() else {
                return;
            };
            let mut image_file: Option<web_sys::File> = None;
            let items = data.items();
            for idx in 0..items.length() {
                let Some(item) = items.get(idx) else {
                    continue;
                };
                if item.kind() == "file"
                    && item.type_().starts_with("image/")
                    && let Ok(Some(file)) = item.get_as_file()
                {
                    image_file = Some(file);
                    break;
                }
            }
            if image_file.is_none()
                && let Some(files) = data.files()
            {
                for idx in 0..files.length() {
                    let Some(file) = files.get(idx) else {
                        continue;
                    };
                    if file.type_().starts_with("image/") {
                        image_file = Some(file);
                        break;
                    }
                }
            }
            let Some(file) = image_file else {
                return;
            };
            let events_ref = Arc::clone(&events);
            let ctx_ref = Arc::clone(&ctx);
            spawn_local(async move {
                match read_web_file_bytes(&file).await {
                    Ok((name, bytes)) => {
                        let safe_name = if name.trim().is_empty() {
                            "pasted.png".to_string()
                        } else {
                            name
                        };
                        push_file_event(
                            &events_ref,
                            &ctx_ref,
                            AsyncFileEvent::Opened(PickedOpenFile::Bytes {
                                name: safe_name,
                                bytes,
                            }),
                        );
                    }
                    Err(msg) => push_file_event(&events_ref, &ctx_ref, AsyncFileEvent::Error(msg)),
                }
            });
        },
    );
    let _ = target.add_event_listener_with_callback("paste", closure.as_ref().unchecked_ref());
    closure.forget();
}

pub fn handle_dropped_file(
    file: std::sync::Arc<dyn egui::DroppedFile + Send + Sync>,
    events: SharedFileEvents,
    ctx: SharedEguiCtx,
) {
    let name = file
        .path()
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dropped.dcraft")
        .to_string();
    spawn_local(async move {
        match file.bytes_async().await {
            Ok(bytes) => {
                let looks_like_pdf = bytes[..bytes.len().min(1024)]
                    .windows(5)
                    .any(|w| w == b"%PDF-");
                let effective_name = if name == "dropped.dcraft" && looks_like_pdf {
                    "dropped.pdf".to_string()
                } else {
                    name
                };
                push_file_event(
                    &events,
                    &ctx,
                    AsyncFileEvent::Opened(PickedOpenFile::Bytes {
                        name: effective_name,
                        bytes,
                    }),
                );
            }
            Err(err) => push_file_event(
                &events,
                &ctx,
                AsyncFileEvent::Error(format!("Failed to read dropped file `{name}`: {err}")),
            ),
        }
    });
}

fn try_start_save_picker(kind: FileDialogKind, default_name: &str) -> Option<js_sys::Promise> {
    let window = web_sys::window()?;
    let func_val = js_sys::Reflect::get(&window, &JsValue::from_str("showSaveFilePicker")).ok()?;
    let func = func_val.dyn_into::<js_sys::Function>().ok()?;
    let opts = build_save_picker_options(kind, default_name).ok()?;
    let res = func.call1(&window, &opts).ok()?;
    res.dyn_into::<js_sys::Promise>().ok()
}

fn try_start_open_picker(kind: FileDialogKind) -> Option<js_sys::Promise> {
    let window = web_sys::window()?;
    let func_val = js_sys::Reflect::get(&window, &JsValue::from_str("showOpenFilePicker")).ok()?;
    let func = func_val.dyn_into::<js_sys::Function>().ok()?;
    let opts = build_open_picker_options(kind).ok()?;
    let res = func.call1(&window, &opts).ok()?;
    res.dyn_into::<js_sys::Promise>().ok()
}

fn build_type_entry(kind: FileDialogKind, for_open: bool) -> Result<JsValue, JsValue> {
    let exts = js_sys::Array::new();
    if for_open {
        for ext in kind.open_extensions() {
            exts.push(&JsValue::from_str(&format!(".{ext}")));
        }
    } else {
        exts.push(&JsValue::from_str(&format!(".{}", kind.extension())));
    }

    let accept = js_sys::Object::new();
    js_sys::Reflect::set(&accept, &JsValue::from_str(kind.mime_type()), &exts)?;

    let entry = js_sys::Object::new();
    js_sys::Reflect::set(
        &entry,
        &JsValue::from_str("description"),
        &JsValue::from_str(kind.filter_label()),
    )?;
    js_sys::Reflect::set(&entry, &JsValue::from_str("accept"), &accept)?;
    Ok(entry.into())
}

fn build_save_picker_options(kind: FileDialogKind, default_name: &str) -> Result<JsValue, JsValue> {
    let types = js_sys::Array::new();
    types.push(&build_type_entry(kind, false)?);

    let opts = js_sys::Object::new();
    js_sys::Reflect::set(
        &opts,
        &JsValue::from_str("suggestedName"),
        &JsValue::from_str(default_name),
    )?;
    js_sys::Reflect::set(&opts, &JsValue::from_str("types"), &types)?;
    Ok(opts.into())
}

fn build_open_picker_options(kind: FileDialogKind) -> Result<JsValue, JsValue> {
    let types = js_sys::Array::new();
    types.push(&build_type_entry(kind, true)?);

    let opts = js_sys::Object::new();
    js_sys::Reflect::set(&opts, &JsValue::from_str("multiple"), &JsValue::FALSE)?;
    js_sys::Reflect::set(&opts, &JsValue::from_str("types"), &types)?;
    Ok(opts.into())
}

async fn write_bytes_to_handle(handle: &JsValue, bytes: &[u8]) -> Result<(), String> {
    let create_writable = js_sys::Reflect::get(handle, &JsValue::from_str("createWritable"))
        .map_err(|e| format_js_error("createWritable unavailable", &e))?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "createWritable is not a function".to_string())?;
    let writable_promise = create_writable
        .call0(handle)
        .map_err(|e| format_js_error("createWritable failed", &e))?
        .dyn_into::<js_sys::Promise>()
        .map_err(|_| "createWritable did not return a Promise".to_string())?;
    let writable = JsFuture::from(writable_promise)
        .await
        .map_err(|e| format_js_error("failed opening file writer", &e))?;

    let write_fn = js_sys::Reflect::get(&writable, &JsValue::from_str("write"))
        .map_err(|e| format_js_error("writer.write unavailable", &e))?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "writer.write is not a function".to_string())?;
    let uint8 = js_sys::Uint8Array::from(bytes);
    let write_promise = write_fn
        .call1(&writable, &uint8)
        .map_err(|e| format_js_error("writer.write failed", &e))?
        .dyn_into::<js_sys::Promise>()
        .map_err(|_| "writer.write did not return a Promise".to_string())?;
    JsFuture::from(write_promise)
        .await
        .map_err(|e| format_js_error("failed writing file bytes", &e))?;

    let close_fn = js_sys::Reflect::get(&writable, &JsValue::from_str("close"))
        .map_err(|e| format_js_error("writer.close unavailable", &e))?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "writer.close is not a function".to_string())?;
    let close_promise = close_fn
        .call0(&writable)
        .map_err(|e| format_js_error("writer.close failed", &e))?
        .dyn_into::<js_sys::Promise>()
        .map_err(|_| "writer.close did not return a Promise".to_string())?;
    JsFuture::from(close_promise)
        .await
        .map_err(|e| format_js_error("failed closing file writer", &e))?;
    Ok(())
}

async fn read_handle_bytes(handle: &JsValue) -> Result<(String, Vec<u8>), String> {
    let get_file = js_sys::Reflect::get(handle, &JsValue::from_str("getFile"))
        .map_err(|e| format_js_error("getFile unavailable", &e))?
        .dyn_into::<js_sys::Function>()
        .map_err(|_| "getFile is not a function".to_string())?;
    let file_promise = get_file
        .call0(handle)
        .map_err(|e| format_js_error("getFile failed", &e))?
        .dyn_into::<js_sys::Promise>()
        .map_err(|_| "getFile did not return a Promise".to_string())?;
    let file_val = JsFuture::from(file_promise)
        .await
        .map_err(|e| format_js_error("failed reading file handle", &e))?;
    let file = file_val
        .dyn_into::<web_sys::File>()
        .map_err(|_| "selected item is not a File".to_string())?;
    read_web_file_bytes(&file).await
}

async fn read_web_file_bytes(file: &web_sys::File) -> Result<(String, Vec<u8>), String> {
    let name = file.name();
    let buffer = JsFuture::from(file.array_buffer())
        .await
        .map_err(|e| format_js_error("failed reading selected file bytes", &e))?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    Ok((name, bytes))
}

fn trigger_blob_download_and_notify(
    kind: FileDialogKind,
    filename: &str,
    bytes: &[u8],
    events: &SharedFileEvents,
    ctx: &SharedEguiCtx,
) {
    match trigger_blob_download(filename, kind.mime_type(), bytes) {
        Ok(()) => push_file_event(
            events,
            ctx,
            AsyncFileEvent::Saved {
                message: format!("Downloaded `{filename}` ({} bytes).", bytes.len()),
            },
        ),
        Err(msg) => push_file_event(events, ctx, AsyncFileEvent::Error(msg)),
    }
}

fn trigger_blob_download(filename: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    let window = web_sys::window().ok_or_else(|| "browser window unavailable".to_string())?;
    let document = window
        .document()
        .ok_or_else(|| "browser document unavailable".to_string())?;
    let body = document
        .body()
        .ok_or_else(|| "document body unavailable".to_string())?;

    let uint8 = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&uint8);
    let props = web_sys::BlobPropertyBag::new();
    props.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &props)
        .map_err(|e| format_js_error("failed to create download blob", &e))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|e| format_js_error("failed to create object URL", &e))?;

    let anchor_el = document
        .create_element("a")
        .map_err(|e| format_js_error("failed creating download link", &e))?;
    let anchor = anchor_el
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .map_err(|_| "element is not an HtmlAnchorElement".to_string())?;
    anchor.set_href(&url);
    anchor.set_download(filename);
    let _ = anchor.set_attribute("style", "display:none");
    let _ = body.append_child(&anchor);
    anchor.click();
    anchor.remove();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

fn trigger_input_file_picker(kind: FileDialogKind, events: SharedFileEvents, ctx: SharedEguiCtx) {
    let accept = kind
        .open_extensions()
        .iter()
        .map(|ext| format!(".{ext}"))
        .collect::<Vec<_>>()
        .join(",");
    trigger_input_accept_picker(&accept, None, events, ctx);
}

fn trigger_input_accept_picker(
    accept: &str,
    capture: Option<&str>,
    events: SharedFileEvents,
    ctx: SharedEguiCtx,
) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let Ok(el) = document.create_element("input") else {
        return;
    };
    let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() else {
        return;
    };
    input.set_type("file");
    input.set_accept(accept);
    if let Some(mode) = capture {
        let _ = input.set_attribute("capture", mode);
    }
    let _ = input.set_attribute("style", "display:none");

    let input_ref = input.clone();
    let on_change = Closure::once_into_js(move || {
        let maybe_file = input_ref.files().and_then(|list| list.get(0));
        input_ref.remove();
        let Some(file) = maybe_file else {
            push_file_event(&events, &ctx, AsyncFileEvent::Cancelled);
            return;
        };
        spawn_local(async move {
            match read_web_file_bytes(&file).await {
                Ok((name, bytes)) => push_file_event(
                    &events,
                    &ctx,
                    AsyncFileEvent::Opened(PickedOpenFile::Bytes { name, bytes }),
                ),
                Err(msg) => push_file_event(&events, &ctx, AsyncFileEvent::Error(msg)),
            }
        });
    });

    input.set_onchange(Some(on_change.as_ref().unchecked_ref()));
    if let Some(body) = document.body() {
        let _ = body.append_child(&input);
    }
    input.click();
}

fn is_user_abort_error(err: &JsValue) -> bool {
    if let Ok(name_val) = js_sys::Reflect::get(err, &JsValue::from_str("name"))
        && let Some(name) = name_val.as_string()
        && name == "AbortError"
    {
        return true;
    }
    err.as_string()
        .is_some_and(|s| s.contains("AbortError") || s.to_ascii_lowercase().contains("aborted"))
}

fn format_js_error(prefix: &str, err: &JsValue) -> String {
    let detail = err
        .as_string()
        .or_else(|| {
            js_sys::Reflect::get(err, &JsValue::from_str("message"))
                .ok()
                .and_then(|v| v.as_string())
        })
        .unwrap_or_else(|| "browser file operation failed".to_string());
    format!("{prefix}: {detail}")
}
