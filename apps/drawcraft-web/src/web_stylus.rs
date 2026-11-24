//! W3C Pointer Events bridge for hardware stylus pressure, tilt, twist, barrel buttons,
//! and inverted eraser tip in the WebAssembly browser build.

use std::sync::{Arc, Mutex, PoisonError};

use drawcraft_tablet::palm::{Gesture, PalmRejection};
use drawcraft_tablet::stylus::{InputKind, RawPointer, StylusSample};
use drawcraft_ui::TouchSourceEvent;
use drawcraft_ui::stylus::{PenContactEvent, PenContactKind, PenSample};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::touch_source::{TouchPhase, TouchTracker, classify_touch};
use crate::web_files::SharedEguiCtx;

const MAX_QUEUED_GESTURES: usize = 16;

const MAX_QUEUED_SAMPLES: usize = 2048;
const MAX_QUEUED_CONTACTS: usize = 512;
const MIN_WEB_CONTACT_PRESSURE: f32 = 0.02;
const CONTEXTMENU_SUPPRESS_GRACE_MS: f64 = 750.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PointerPhase {
    Enter,
    Down,
    Move,
    Up,
    Cancel,
    Leave,
}

#[derive(Default)]
struct WebStylusState {
    samples: Vec<PenSample>,
    contact_events: Vec<PenContactEvent>,
    pen_active: bool,
    pen_end_eraser: Option<bool>,
    barrel_down: bool,
    secondary_barrel_down: bool,
    barrel_press_count: u32,
    secondary_barrel_press_count: u32,
    tracked_contact_pointer_id: Option<i32>,
    last_pen_barrel_time_ms: Option<f64>,
    /// Palm rejection and two/three-finger tap recognition (`drawcraft-tablet`).
    palm: PalmRejection,
    gestures: Vec<Gesture>,
    /// Source (Pencil or finger) of every live DOM touch, and which were swallowed as palms.
    touches: TouchTracker,
    /// `true` while the most recent DOM `PointerEvent` came from `pointerType === "touch"` (a
    /// finger). Read by `drawcraft-ui` during `eframe`'s synchronous `pointerdown` logic call,
    /// which runs *before* `touchstart` fires on iPadOS Safari.
    active_pointer_is_touch: bool,
}

/// Shared handle to browser `PointerEvent` stylus state consumed by `PlatformHooks`.
#[derive(Clone, Default)]
pub struct WebStylusHandle {
    state: Arc<Mutex<WebStylusState>>,
}

impl WebStylusHandle {
    /// Installs `pointer*` and `contextmenu` DOM listeners on the canvas and window.
    pub fn install(canvas: &web_sys::HtmlCanvasElement, ctx_slot: SharedEguiCtx) -> Self {
        let handle = Self::default();
        handle.attach_pointer_listener(canvas, "pointerenter", PointerPhase::Enter, &ctx_slot);
        handle.attach_pointer_listener(canvas, "pointerdown", PointerPhase::Down, &ctx_slot);
        handle.attach_pointer_listener(canvas, "pointermove", PointerPhase::Move, &ctx_slot);
        handle.attach_pointer_listener(canvas, "pointerup", PointerPhase::Up, &ctx_slot);
        handle.attach_pointer_listener(canvas, "pointercancel", PointerPhase::Cancel, &ctx_slot);
        handle.attach_pointer_listener(canvas, "pointerleave", PointerPhase::Leave, &ctx_slot);
        handle.attach_contextmenu_listener(canvas);

        if let Some(window) = web_sys::window() {
            handle.attach_touch_guard(&window);
            handle.attach_window_release_listener(
                &window,
                canvas,
                "pointerup",
                PointerPhase::Up,
                &ctx_slot,
            );
            handle.attach_window_release_listener(
                &window,
                canvas,
                "pointercancel",
                PointerPhase::Cancel,
                &ctx_slot,
            );
        }
        handle
    }

    pub fn drain_samples(&self) -> Vec<PenSample> {
        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut g.samples)
    }

    pub fn drain_contact_events(&self) -> Vec<PenContactEvent> {
        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut g.contact_events)
    }

    /// Touch source changes (Pencil vs finger) recorded since the last call.
    pub fn drain_touch_sources(&self) -> Vec<TouchSourceEvent> {
        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        g.touches.drain_reports()
    }

    /// `true` when the latest DOM pointer event came from a finger (`pointerType === "touch"`).
    pub fn pointer_is_direct_touch(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active_pointer_is_touch
    }

    /// Two-finger (undo) and three-finger (redo) taps recognised since the last call.
    pub fn drain_gestures(&self) -> Vec<Gesture> {
        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut g.gestures)
    }

    /// Drops touch events that belong to a resting palm before the page (egui) sees them.
    ///
    /// Runs in the capture phase on `window`, ahead of eframe's own listeners.
    fn attach_touch_guard(&self, window: &web_sys::Window) {
        for name in ["touchstart", "touchmove", "touchend", "touchcancel"] {
            let state = Arc::clone(&self.state);
            let phase = name.to_string();
            let cb = Closure::<dyn FnMut(web_sys::TouchEvent)>::wrap(Box::new(
                move |ev: web_sys::TouchEvent| {
                    if touch_event_is_palm(&state, &ev, &phase) {
                        ev.stop_immediate_propagation();
                        ev.prevent_default();
                    }
                },
            ));
            let options = web_sys::AddEventListenerOptions::new();
            options.set_capture(true);
            options.set_passive(false);
            let _ = window.add_event_listener_with_callback_and_add_event_listener_options(
                name,
                cb.as_ref().unchecked_ref(),
                &options,
            );
            cb.forget();
        }
    }

    pub fn pen_end(&self) -> Option<bool> {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pen_end_eraser
    }

    pub fn barrel_down(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .barrel_down
    }

    pub fn secondary_barrel_down(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .secondary_barrel_down
    }

    pub fn take_barrel_press_count(&self) -> u32 {
        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut g.barrel_press_count)
    }

    pub fn take_secondary_barrel_press_count(&self) -> u32 {
        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut g.secondary_barrel_press_count)
    }

    fn attach_pointer_listener(
        &self,
        canvas: &web_sys::HtmlCanvasElement,
        event_name: &str,
        phase: PointerPhase,
        ctx_slot: &SharedEguiCtx,
    ) {
        let state = Arc::clone(&self.state);
        let canvas_ref = canvas.clone();
        let ctx_ref = Arc::clone(ctx_slot);
        let cb = Closure::<dyn FnMut(web_sys::PointerEvent)>::wrap(Box::new(
            move |ev: web_sys::PointerEvent| {
                let needs_repaint = handle_pointer_event(&state, &canvas_ref, &ev, phase);
                if needs_repaint {
                    request_repaint(&ctx_ref);
                }
            },
        ));
        // Capture phase ensures we classify `pointerType` ("touch" vs "pen" vs "mouse") *before*
        // eframe's bubble-phase `pointerdown` listener synchronously invokes `runner.logic()`.
        let options = web_sys::AddEventListenerOptions::new();
        options.set_capture(true);
        let _ = canvas.add_event_listener_with_callback_and_add_event_listener_options(
            event_name,
            cb.as_ref().unchecked_ref(),
            &options,
        );
        cb.forget();
    }

    fn attach_window_release_listener(
        &self,
        window: &web_sys::Window,
        canvas: &web_sys::HtmlCanvasElement,
        event_name: &str,
        phase: PointerPhase,
        ctx_slot: &SharedEguiCtx,
    ) {
        let state = Arc::clone(&self.state);
        let canvas_ref = canvas.clone();
        let ctx_ref = Arc::clone(ctx_slot);
        let cb = Closure::<dyn FnMut(web_sys::PointerEvent)>::wrap(Box::new(
            move |ev: web_sys::PointerEvent| {
                let needs_repaint = handle_window_release(&state, &canvas_ref, &ev, phase);
                if needs_repaint {
                    request_repaint(&ctx_ref);
                }
            },
        ));
        let _ = window.add_event_listener_with_callback(event_name, cb.as_ref().unchecked_ref());
        cb.forget();
    }

    fn attach_contextmenu_listener(&self, canvas: &web_sys::HtmlCanvasElement) {
        let state = Arc::clone(&self.state);
        let cb = Closure::<dyn FnMut(web_sys::MouseEvent)>::wrap(Box::new(
            move |ev: web_sys::MouseEvent| {
                let now_ms = ev.time_stamp();
                let g = state.lock().unwrap_or_else(PoisonError::into_inner);
                let recent_barrel = g.last_pen_barrel_time_ms.is_some_and(|t| {
                    now_ms.is_finite()
                        && t.is_finite()
                        && now_ms >= t
                        && (now_ms - t) <= CONTEXTMENU_SUPPRESS_GRACE_MS
                });
                if g.pen_active
                    || g.barrel_down
                    || g.secondary_barrel_down
                    || g.pen_end_eraser == Some(true)
                    || recent_barrel
                {
                    ev.prevent_default();
                }
            },
        ));
        let _ = canvas.add_event_listener_with_callback("contextmenu", cb.as_ref().unchecked_ref());
        cb.forget();
    }
}

fn request_repaint(ctx_slot: &SharedEguiCtx) {
    if let Ok(guard) = ctx_slot.lock()
        && let Some(egui_ctx) = guard.as_ref()
    {
        egui_ctx.request_repaint();
    }
}

fn canvas_local_pos(canvas: &web_sys::HtmlCanvasElement, ev: &web_sys::PointerEvent) -> egui::Pos2 {
    let rect = canvas.get_bounding_client_rect();
    let x = f64::from(ev.client_x()) - rect.left();
    let y = f64::from(ev.client_y()) - rect.top();
    let x_f32 = if x.is_finite() { x as f32 } else { 0.0 };
    let y_f32 = if y.is_finite() { y as f32 } else { 0.0 };
    egui::pos2(x_f32, y_f32)
}

fn event_modifiers(ev: &web_sys::PointerEvent) -> egui::Modifiers {
    egui::Modifiers {
        alt: ev.alt_key(),
        ctrl: ev.ctrl_key(),
        shift: ev.shift_key(),
        mac_cmd: ev.meta_key(),
        command: ev.ctrl_key() || ev.meta_key(),
    }
}

fn decode_pen_flags(phase: PointerPhase, button: i16, buttons: u16) -> (bool, bool, bool) {
    let is_down = phase == PointerPhase::Down;
    let is_release = matches!(
        phase,
        PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Leave
    );
    let barrel =
        ((buttons & 2) != 0 || (is_down && button == 2)) && !(is_release && (buttons & 2) == 0);
    let secondary =
        ((buttons & 4) != 0 || (is_down && button == 1)) && !(is_release && (buttons & 4) == 0);
    let eraser =
        ((buttons & 32) != 0 || (is_down && button == 5)) && !(is_release && (buttons & 32) == 0);
    (barrel, secondary, eraser)
}

fn sample_pressure(phase: PointerPhase, buttons: u16, raw_pressure: f32) -> f32 {
    let clean = if raw_pressure.is_finite() {
        raw_pressure.clamp(0.0, 1.0)
    } else {
        0.0
    };
    match phase {
        PointerPhase::Down => clean.max(MIN_WEB_CONTACT_PRESSURE),
        PointerPhase::Move if (buttons & 1) != 0 => clean.max(MIN_WEB_CONTACT_PRESSURE),
        PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Leave => 0.0,
        PointerPhase::Enter | PointerPhase::Move => clean,
    }
}

fn reflect_f32_obj(obj: &wasm_bindgen::JsValue, field: &str) -> Option<f32> {
    js_sys::Reflect::get(obj, &wasm_bindgen::JsValue::from_str(field))
        .ok()
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite())
        .map(|v| v as f32)
}

fn reflect_f32(ev: &web_sys::PointerEvent, field: &str) -> Option<f32> {
    reflect_f32_obj(ev.as_ref(), field)
}

/// Safely calls `PointerEvent.getCoalescedEvents()` when available.
///
/// Per the W3C Pointer Events Level 3 spec, `getCoalescedEvents` is `[SecureContext]` only
/// (it is `undefined` over plain HTTP LAN URLs like `http://<ip>:8080`, and absent on older
/// Safari releases). Calling `ev.get_coalesced_events()` directly throws a JS `TypeError`
/// inside `wasm-bindgen` and freezes the entire WASM runtime.
fn safe_coalesced_events(ev: &web_sys::PointerEvent) -> Vec<web_sys::PointerEvent> {
    let Ok(prop) = js_sys::Reflect::get(
        ev.as_ref(),
        &wasm_bindgen::JsValue::from_str("getCoalescedEvents"),
    ) else {
        return Vec::new();
    };
    let Ok(func) = prop.dyn_into::<js_sys::Function>() else {
        return Vec::new();
    };
    let Ok(result) = func.call0(ev.as_ref()) else {
        return Vec::new();
    };
    let Ok(array) = result.dyn_into::<js_sys::Array>() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(array.length() as usize);
    for item in array.iter() {
        if let Ok(inner) = item.dyn_into::<web_sys::PointerEvent>() {
            out.push(inner);
        }
    }
    out
}

/// Converts one DOM pointer event (or one coalesced sample of it) into a pen sample carrying the
/// Apple Pencil fields: pressure, tilt, azimuth, altitude, twist and the button flags.
fn pen_sample(
    ev: &web_sys::PointerEvent,
    phase: PointerPhase,
    barrel: bool,
    secondary: bool,
    eraser: bool,
) -> PenSample {
    let pointer_type = ev.pointer_type();
    let raw = RawPointer {
        pointer_type: &pointer_type,
        pointer_id: ev.pointer_id(),
        x: ev.client_x() as f32,
        y: ev.client_y() as f32,
        pressure: ev.pressure(),
        tilt_x: ev.tilt_x() as f32,
        tilt_y: ev.tilt_y() as f32,
        // Safari exposes the Pointer Events 3 angles on pen events; older engines derive them.
        azimuth_angle: reflect_f32(ev, "azimuthAngle"),
        altitude_angle: reflect_f32(ev, "altitudeAngle"),
        twist: ev.twist() as f32,
        buttons: ev.buttons(),
        time_ms: ev.time_stamp(),
    };
    let stylus = StylusSample::from_raw(&raw);
    PenSample {
        pressure: sample_pressure(phase, ev.buttons(), ev.pressure()),
        tilt_x: stylus.tilt_x,
        tilt_y: stylus.tilt_y,
        rotation: stylus.twist,
        eraser,
        barrel,
        barrel_secondary: secondary,
        time_ms: Some(stylus.time_ms),
        azimuth: Some(stylus.azimuth),
        altitude: Some(stylus.altitude),
    }
}

/// Feeds pen, finger and mouse pointers to the palm-rejection state machine.
fn track_palm(
    g: &mut WebStylusState,
    kind: InputKind,
    id: i32,
    x: f32,
    y: f32,
    t: f64,
    phase: PointerPhase,
) {
    match phase {
        PointerPhase::Down => {
            g.palm.pointer_down(kind, id, x, y, t);
        }
        PointerPhase::Move => {
            if kind == InputKind::Stylus {
                g.palm.pencil_hover(t);
            }
            g.palm.pointer_move(id, x, y, t);
        }
        PointerPhase::Up => {
            if let Some(gesture) = g.palm.pointer_up(id, t) {
                if g.gestures.len() >= MAX_QUEUED_GESTURES {
                    g.gestures.remove(0);
                }
                g.gestures.push(gesture);
            }
        }
        PointerPhase::Cancel => g.palm.pointer_cancel(id),
        PointerPhase::Enter => {
            if kind == InputKind::Stylus {
                g.palm.pencil_hover(t);
            }
        }
        PointerPhase::Leave => {}
    }
}

/// Reads Safari's non-standard `Touch.touchType` (`"stylus"` for the Apple Pencil, `"direct"` for
/// a finger). `None` on browsers that do not report it.
fn touch_type(touch: &web_sys::Touch) -> Option<String> {
    js_sys::Reflect::get(touch, &wasm_bindgen::JsValue::from_str("touchType"))
        .ok()
        .and_then(|v| v.as_string())
}

/// Classifies every changed touch and decides whether the whole DOM event is a resting palm.
///
/// All DOM reads happen *before* acquiring the state lock so a browser quirk can never leave the
/// mutex locked in single-threaded WASM. The Apple Pencil's own touch events
/// (`touchType === "stylus"`) are never swallowed, even though its `pointerdown` has already
/// armed the palm guard.
fn touch_event_is_palm(
    state: &Arc<Mutex<WebStylusState>>,
    ev: &web_sys::TouchEvent,
    phase: &str,
) -> bool {
    let Some(phase) = TouchPhase::from_event_name(phase) else {
        return false;
    };
    let changed = ev.changed_touches();
    let count = changed.length();
    if count == 0 {
        return false;
    }
    let now = ev.time_stamp();
    let mut parsed: Vec<Option<(i32, drawcraft_ui::TouchSource)>> =
        Vec::with_capacity(count as usize);
    for i in 0..count {
        let entry = changed.get(i).map(|touch| {
            let force = reflect_f32_obj(touch.as_ref(), "force").unwrap_or(0.0);
            let source = classify_touch(touch_type(&touch).as_deref(), force);
            (touch.identifier(), source)
        });
        parsed.push(entry);
    }

    let mut g = state.lock().unwrap_or_else(PoisonError::into_inner);
    let mut all_rejected = true;
    for item in parsed {
        let Some((id, source)) = item else {
            all_rejected = false;
            continue;
        };
        let guard = g.palm.touch_rejected(now);
        if source == drawcraft_ui::TouchSource::Stylus {
            // Seeing the Pencil as a touch also arms the palm guard for fingers that follow.
            g.palm.pencil_hover(now);
        }
        let rejected = g.touches.handle(phase, id, source, guard);
        all_rejected &= rejected;
    }
    all_rejected
}
fn push_bounded_sample(samples: &mut Vec<PenSample>, sample: PenSample) {
    if samples.len() >= MAX_QUEUED_SAMPLES {
        samples.remove(0);
    }
    samples.push(sample.sanitized());
}

fn push_bounded_contact(contacts: &mut Vec<PenContactEvent>, event: PenContactEvent) {
    if contacts.len() >= MAX_QUEUED_CONTACTS {
        contacts.remove(0);
    }
    contacts.push(event);
}

fn clear_pen_state(state: &mut WebStylusState) -> bool {
    let changed = state.pen_active
        || state.barrel_down
        || state.secondary_barrel_down
        || state.pen_end_eraser.is_some();
    state.pen_active = false;
    state.pen_end_eraser = None;
    state.barrel_down = false;
    state.secondary_barrel_down = false;
    state.tracked_contact_pointer_id = None;
    changed
}

fn handle_pointer_event(
    state: &Arc<Mutex<WebStylusState>>,
    canvas: &web_sys::HtmlCanvasElement,
    ev: &web_sys::PointerEvent,
    phase: PointerPhase,
) -> bool {
    // Read every DOM property *before* locking `state` so no JS throw can poison/deadlock WASM.
    let pointer_type = ev.pointer_type();
    let kind = InputKind::from_pointer_type(&pointer_type);
    let pointer_id = ev.pointer_id();
    let client_x = ev.client_x() as f32;
    let client_y = ev.client_y() as f32;
    let now_ms = ev.time_stamp();

    if pointer_type != "pen" {
        let mut g = state.lock().unwrap_or_else(PoisonError::into_inner);
        g.active_pointer_is_touch = pointer_type == "touch";
        track_palm(&mut g, kind, pointer_id, client_x, client_y, now_ms, phase);
        if pointer_type == "mouse" {
            return clear_pen_state(&mut g);
        }
        return false;
    }

    let pos = canvas_local_pos(canvas, ev);
    let modifiers = event_modifiers(ev);
    let button = ev.button();
    let buttons = ev.buttons();
    let (barrel, secondary, eraser) = decode_pen_flags(phase, button, buttons);

    let extracted_samples: Vec<PenSample> =
        if matches!(phase, PointerPhase::Cancel | PointerPhase::Leave) {
            vec![PenSample {
                pressure: 0.0,
                time_ms: Some(now_ms),
                ..Default::default()
            }]
        } else if phase == PointerPhase::Move {
            let coalesced = safe_coalesced_events(ev);
            if coalesced.is_empty() {
                vec![pen_sample(ev, phase, barrel, secondary, eraser)]
            } else {
                coalesced
                    .iter()
                    .map(|inner| pen_sample(inner, phase, barrel, secondary, eraser))
                    .collect()
            }
        } else {
            vec![pen_sample(ev, phase, barrel, secondary, eraser)]
        };

    let mut g = state.lock().unwrap_or_else(PoisonError::into_inner);
    g.active_pointer_is_touch = false;
    track_palm(&mut g, kind, pointer_id, client_x, client_y, now_ms, phase);

    if matches!(phase, PointerPhase::Cancel | PointerPhase::Leave) {
        let mut needs_repaint = clear_pen_state(&mut g);
        if phase == PointerPhase::Cancel {
            push_bounded_contact(
                &mut g.contact_events,
                PenContactEvent {
                    kind: PenContactKind::Cancel,
                    pos,
                    modifiers,
                },
            );
            needs_repaint = true;
        }
        for sample in extracted_samples {
            push_bounded_sample(&mut g.samples, sample);
        }
        return needs_repaint;
    }

    let prev_barrel = g.barrel_down;
    let prev_secondary = g.secondary_barrel_down;
    let prev_eraser = g.pen_end_eraser;

    g.pen_active = true;
    g.barrel_down = barrel;
    g.secondary_barrel_down = secondary;
    g.pen_end_eraser = Some(eraser);

    if barrel && !prev_barrel {
        g.barrel_press_count = g.barrel_press_count.saturating_add(1);
    }
    if secondary && !prev_secondary {
        g.secondary_barrel_press_count = g.secondary_barrel_press_count.saturating_add(1);
    }
    if (barrel || secondary || eraser) && now_ms.is_finite() {
        g.last_pen_barrel_time_ms = Some(now_ms);
    }

    for sample in extracted_samples {
        push_bounded_sample(&mut g.samples, sample);
    }

    let contact_queued = record_fallback_contact(&mut g, pointer_id, pos, modifiers, phase);
    contact_queued
        || prev_barrel != barrel
        || prev_secondary != secondary
        || prev_eraser != Some(eraser)
}

fn record_fallback_contact(
    g: &mut WebStylusState,
    pointer_id: i32,
    pos: egui::Pos2,
    modifiers: egui::Modifiers,
    phase: PointerPhase,
) -> bool {
    // Every pen contact is tracked, including the plain tip (button == 0): the UI de-duplicates
    // against native pointer events, and the contact stream stays correct even when Safari drops
    // the compatibility mouse events.
    match phase {
        PointerPhase::Down => {
            g.tracked_contact_pointer_id = Some(pointer_id);
            push_bounded_contact(
                &mut g.contact_events,
                PenContactEvent {
                    kind: PenContactKind::Down,
                    pos,
                    modifiers,
                },
            );
            true
        }
        PointerPhase::Move if g.tracked_contact_pointer_id == Some(pointer_id) => {
            push_bounded_contact(
                &mut g.contact_events,
                PenContactEvent {
                    kind: PenContactKind::Move,
                    pos,
                    modifiers,
                },
            );
            true
        }
        PointerPhase::Up if g.tracked_contact_pointer_id == Some(pointer_id) => {
            g.tracked_contact_pointer_id = None;
            push_bounded_contact(
                &mut g.contact_events,
                PenContactEvent {
                    kind: PenContactKind::Up,
                    pos,
                    modifiers,
                },
            );
            true
        }
        _ => false,
    }
}

fn handle_window_release(
    state: &Arc<Mutex<WebStylusState>>,
    canvas: &web_sys::HtmlCanvasElement,
    ev: &web_sys::PointerEvent,
    phase: PointerPhase,
) -> bool {
    if ev.pointer_type() != "pen" {
        return false;
    }
    let pointer_id = ev.pointer_id();
    let buttons = ev.buttons();
    let pos = canvas_local_pos(canvas, ev);
    let modifiers = event_modifiers(ev);
    let now_ms = ev.time_stamp();

    let mut g = state.lock().unwrap_or_else(PoisonError::into_inner);
    let had_contact = g.tracked_contact_pointer_id == Some(pointer_id);
    let had_buttons = g.barrel_down || g.secondary_barrel_down || g.pen_end_eraser == Some(true);
    if !had_contact && !had_buttons {
        return false;
    }
    g.barrel_down = (buttons & 2) != 0;
    g.secondary_barrel_down = (buttons & 4) != 0;
    g.pen_end_eraser = Some((buttons & 32) != 0);
    if had_contact {
        g.tracked_contact_pointer_id = None;
        let kind = if phase == PointerPhase::Cancel {
            PenContactKind::Cancel
        } else {
            PenContactKind::Up
        };
        push_bounded_contact(
            &mut g.contact_events,
            PenContactEvent {
                kind,
                pos,
                modifiers,
            },
        );
    }
    push_bounded_sample(
        &mut g.samples,
        PenSample {
            pressure: 0.0,
            eraser: (buttons & 32) != 0,
            barrel: (buttons & 2) != 0,
            barrel_secondary: (buttons & 4) != 0,
            time_ms: Some(now_ms),
            ..Default::default()
        },
    );
    true
}
