//! Stylus input feed, sub-frame sample interpolation, eraser-end & barrel auto-switch,
//! and real-time hardware telemetry.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use drawcraft_doc::{PenKind, StylusButtonAction, StylusButtonConfig};
use drawcraft_engine::{Session, ToolKind};

const FEED_QUEUE_CAP: usize = 2048;
const MIN_CONTACT_PRESSURE: f32 = 0.02;

/// Normalized pen reading consumed by the UI shell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenSample {
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub rotation: f32,
    pub eraser: bool,
    pub barrel: bool,
    pub barrel_secondary: bool,
    pub time_ms: Option<f64>,
    /// Pen azimuth in radians in 0..TAU, when the platform reports it.
    pub azimuth: Option<f32>,
    /// Pen altitude in radians in 0..=PI/2 (PI/2 is perpendicular), when reported.
    pub altitude: Option<f32>,
}

/// Kind of platform-reported pen contact event used as a fallback when a browser
/// omits compatibility mouse events (for example, inverted eraser tip `button == 5`
/// or barrel-button contact where `mousedown` is suppressed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PenContactKind {
    Down,
    Move,
    Up,
    Cancel,
}

/// Canvas-local pen contact event emitted by platform pointer listeners.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PenContactEvent {
    pub kind: PenContactKind,
    pub pos: egui::Pos2,
    pub modifiers: egui::Modifiers,
}

impl Default for PenSample {
    fn default() -> Self {
        Self {
            pressure: 1.0,
            tilt_x: 0.0,
            tilt_y: 0.0,
            rotation: 0.0,
            eraser: false,
            barrel: false,
            barrel_secondary: false,
            time_ms: None,
            azimuth: None,
            altitude: None,
        }
    }
}

impl PenSample {
    pub fn sanitized(self) -> Self {
        let clamp = |v: f32, lo: f32, hi: f32, d: f32| {
            if v.is_finite() { v.clamp(lo, hi) } else { d }
        };
        let rot = if self.rotation.is_finite() {
            let r = self.rotation.rem_euclid(360.0);
            if r >= 360.0 { 0.0 } else { r }
        } else {
            0.0
        };
        Self {
            pressure: clamp(self.pressure, 0.0, 1.0, 1.0),
            tilt_x: clamp(self.tilt_x, -90.0, 90.0, 0.0),
            tilt_y: clamp(self.tilt_y, -90.0, 90.0, 0.0),
            rotation: rot,
            eraser: self.eraser,
            barrel: self.barrel,
            barrel_secondary: self.barrel_secondary,
            time_ms: self.time_ms.filter(|t| t.is_finite() && *t >= 0.0),
            azimuth: self
                .azimuth
                .filter(|a| a.is_finite())
                .map(|a| a.rem_euclid(std::f32::consts::TAU)),
            altitude: self
                .altitude
                .filter(|a| a.is_finite())
                .map(|a| a.clamp(0.0, std::f32::consts::FRAC_PI_2)),
        }
    }
}

#[derive(Debug, Default)]
struct FeedState {
    latest: Option<PenSample>,
    queue: VecDeque<PenSample>,
}

/// Thread-safe shared queue written by OS tablet hooks and drained by the UI frame loop.
#[derive(Clone, Debug, Default)]
pub struct StylusFeed(Arc<Mutex<FeedState>>);

impl StylusFeed {
    pub fn push(&self, sample: Option<PenSample>) {
        let mut g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let s = sample.map(PenSample::sanitized);
        g.latest = s;
        if let Some(val) = s {
            if g.queue.len() >= FEED_QUEUE_CAP {
                g.queue.pop_front();
            }
            g.queue.push_back(val);
        }
    }

    pub fn push_batch(&self, samples: impl IntoIterator<Item = PenSample>) {
        let mut g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for raw in samples {
            let s = raw.sanitized();
            g.latest = Some(s);
            if g.queue.len() >= FEED_QUEUE_CAP {
                g.queue.pop_front();
            }
            g.queue.push_back(s);
        }
    }

    pub fn get(&self) -> Option<PenSample> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).latest
    }

    pub fn clear_latest(&self) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).latest = None;
    }

    pub fn drain(&self) -> Vec<PenSample> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .queue
            .drain(..)
            .collect()
    }
}

/// Connected hardware pen digitizer reported by the platform or session input stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectedStylusInfo {
    pub name: String,
    pub is_external: bool,
}

/// Live stylus readout displayed in the Pen & Stylus Inspector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StylusTelemetry {
    pub hardware_active: bool,
    pub pressure: f32,
    pub tilt_x: f32,
    pub tilt_y: f32,
    pub rotation: f32,
    pub eraser_inverted: bool,
    pub barrel_held: bool,
    pub secondary_barrel_held: bool,
    pub velocity_px_ms: f32,
}

/// Snapshot of physical stylus buttons and UI interaction state evaluated each frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct StylusButtonInput {
    pub platform_end: Option<bool>,
    pub platform_barrel: bool,
    pub platform_secondary_barrel: bool,
    pub barrel_presses: u32,
    pub secondary_barrel_presses: u32,
    pub allow_shortcuts: bool,
    pub allow_instant_actions: bool,
    pub selection_drag_active: bool,
}

/// Mutations requested by the stylus shortcut state machine for the app shell to execute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StylusShortcutOutcome {
    pub switch_tool: Option<ToolKind>,
    pub restore_last_pen_kind: Option<PenKind>,
    pub set_freeform_lasso: Option<bool>,
    pub execute_command: Option<&'static str>,
    pub cancel_modes: bool,
    pub clear_selection: bool,
    pub pan_override_changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HoldToolState {
    target_tool: ToolKind,
    saved_tool: ToolKind,
    saved_last_pen_kind: PenKind,
    saved_freeform_lasso: bool,
}

#[derive(Clone, Copy)]
struct ToggleSpec {
    target_tool: ToolKind,
    fallback_pen: PenKind,
    enable_lasso: bool,
}

fn hold_tool_for_action(action: StylusButtonAction) -> Option<ToolKind> {
    match action {
        StylusButtonAction::EraserHold => Some(ToolKind::Eraser),
        StylusButtonAction::LassoSelectHold => Some(ToolKind::Select),
        StylusButtonAction::HighlighterHold => Some(ToolKind::Pen(PenKind::Highlighter)),
        _ => None,
    }
}

fn resolve_desired_hold_action(
    cfg: StylusButtonConfig,
    eraser_inverted: bool,
    barrel_held: bool,
    secondary_barrel_held: bool,
) -> Option<StylusButtonAction> {
    let candidates = [
        (eraser_inverted, cfg.eraser_end),
        (barrel_held, cfg.barrel_button),
        (secondary_barrel_held, cfg.secondary_barrel_button),
    ];
    for (active, action) in candidates {
        if active && action.is_hold_action() {
            return Some(action);
        }
    }
    None
}

fn pick_instant_action(
    cfg: StylusButtonConfig,
    barrel_triggered: bool,
    secondary_triggered: bool,
) -> Option<StylusButtonAction> {
    if barrel_triggered
        && !cfg.barrel_button.is_hold_action()
        && cfg.barrel_button != StylusButtonAction::None
    {
        return Some(cfg.barrel_button);
    }
    if secondary_triggered
        && !cfg.secondary_barrel_button.is_hold_action()
        && cfg.secondary_barrel_button != StylusButtonAction::None
    {
        return Some(cfg.secondary_barrel_button);
    }
    None
}

/// Per-application stylus coordinator.
#[derive(Clone, Debug, Default)]
pub struct Stylus {
    pub feed: StylusFeed,
    pub connected_devices: Vec<ConnectedStylusInfo>,
    pub hardware_seen_in_session: bool,
    had_os_pen_devices: bool,
    updated_frame: Option<u64>,
    touch_force: Option<f32>,
    last_active_pressure: Option<f32>,
    contacts: Vec<egui::TouchId>,
    touch_pen: bool,
    lifted: bool,
    hardware_feed_this_frame: bool,
    frame_samples: Vec<PenSample>,
    prev_sample: Option<PenSample>,
    selected_sample: Option<PenSample>,
    active_hold_tool: Option<HoldToolState>,
    tool_before_toggle: Option<(ToolKind, PenKind, bool)>,
    pan_override_active: bool,
    suppress_middle_pan_active: bool,
    prev_barrel_down: bool,
    prev_secondary_barrel_down: bool,
    os_pen_id: Option<u64>,
    last_pen_touch_id: Option<egui::TouchId>,
    last_pen_pos: Option<egui::Pos2>,
    last_pen_activity_s: Option<f64>,
    ignored_touch_ids: Vec<egui::TouchId>,
    ended_ignored_touch_ids: Vec<egui::TouchId>,
    emulated_primary_down_by_touch: Option<egui::TouchId>,
    promoted_barrel_button: Option<egui::PointerButton>,
    synthetic_contact_active: bool,
    pub telemetry: StylusTelemetry,
    pub clock_ms: f64,
}

impl Stylus {
    #[must_use]
    pub fn is_connected(&self) -> bool {
        !self.connected_devices.is_empty()
            || self.hardware_seen_in_session
            || self.telemetry.hardware_active
    }

    #[must_use]
    pub fn connected_device_label(&self) -> String {
        match self.connected_devices.as_slice() {
            [] => "Hardware Stylus".to_string(),
            [single] => single.name.clone(),
            [first, rest @ ..] => format!("{} (+{})", first.name, rest.len()),
        }
    }

    pub fn set_connected_devices(&mut self, devices: Vec<ConnectedStylusInfo>) {
        if !devices.is_empty() {
            self.had_os_pen_devices = true;
        } else if self.had_os_pen_devices {
            self.had_os_pen_devices = false;
            self.hardware_seen_in_session = false;
        }
        self.connected_devices = devices;
    }

    #[must_use]
    pub fn pan_held(&self) -> bool {
        self.pan_override_active
    }

    #[must_use]
    pub fn suppress_middle_pan(&self) -> bool {
        self.suppress_middle_pan_active
    }

    pub fn update_for_frame(&mut self, frame: u64, events: &[egui::Event], now_ms: f64) {
        if self.updated_frame.replace(frame) != Some(frame) {
            self.clock_ms = now_ms;
            self.update(events, now_ms);
        }
    }

    fn handle_touch_event(
        &mut self,
        id: egui::TouchId,
        phase: egui::TouchPhase,
        force: Option<f32>,
        hardware_from_feed: bool,
        now_ms: f64,
    ) {
        let clean_force = force.filter(|f| f.is_finite()).map(|f| f.clamp(0.0, 1.0));
        match phase {
            egui::TouchPhase::Start | egui::TouchPhase::Move => {
                let start = phase == egui::TouchPhase::Start;
                if start {
                    self.lifted = false;
                    if !self.contacts.contains(&id) {
                        if self.contacts.len() >= 16 {
                            self.contacts.remove(0);
                        }
                        if self.contacts.is_empty() {
                            self.prev_sample = None;
                            self.touch_force = None;
                            self.last_active_pressure = None;
                        }
                        self.contacts.push(id);
                    }
                }
                let down = start || self.contacts.contains(&id);
                if clean_force.is_some() {
                    self.touch_pen = true;
                }
                if !down {
                    // Hovering moves (!down) must not inject 0.0 contact samples or overwrite pressure.
                    return;
                }
                if let Some(raw_f) = clean_force {
                    let p = if raw_f > 0.0 {
                        raw_f.max(MIN_CONTACT_PRESSURE)
                    } else {
                        self.last_active_pressure.unwrap_or(MIN_CONTACT_PRESSURE)
                    };
                    self.last_active_pressure = Some(p);
                    self.touch_force = Some(p);
                    if !hardware_from_feed {
                        self.frame_samples.push(PenSample {
                            pressure: p,
                            time_ms: Some(now_ms),
                            ..Default::default()
                        });
                    }
                } else if !hardware_from_feed {
                    self.touch_force = None;
                }
            }
            egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                self.contacts.retain(|c| *c != id);
                if self.contacts.is_empty() {
                    self.lifted = true;
                }
            }
        }
    }

    pub fn update(&mut self, events: &[egui::Event], now_ms: f64) {
        let was_lifted = std::mem::take(&mut self.lifted);
        if was_lifted {
            self.prev_sample = None;
            self.touch_force = None;
            self.last_active_pressure = None;
        } else if let Some(last) = self.frame_samples.last() {
            self.prev_sample = Some(*last);
        }
        self.frame_samples = self.feed.drain();
        let hardware_from_feed = !self.frame_samples.is_empty();
        self.hardware_feed_this_frame = hardware_from_feed;
        if was_lifted && !hardware_from_feed {
            self.feed.clear_latest();
        }
        self.selected_sample = None;

        let mut saw_touch = false;
        let mut saw_mouse_press = false;
        let mut saw_primary_press = false;
        for ev in events {
            match ev {
                egui::Event::Touch {
                    id, phase, force, ..
                } => {
                    saw_touch = true;
                    self.handle_touch_event(*id, *phase, *force, hardware_from_feed, now_ms);
                }
                egui::Event::PointerButton {
                    button,
                    pressed: true,
                    ..
                } => {
                    saw_mouse_press = true;
                    if *button == egui::PointerButton::Primary {
                        saw_primary_press = true;
                    }
                }
                _ => {}
            }
        }

        // A primary press with no active contacts begins a fresh stroke without inheriting
        // ending pressure from the prior stroke.
        if !saw_touch && saw_primary_press && self.contacts.is_empty() {
            self.prev_sample = None;
            self.touch_force = None;
            self.last_active_pressure = None;
        }

        // Pure mouse/non-touch clicks must not inherit stale stylus hover/contact samples.
        if !saw_touch && saw_mouse_press && !hardware_from_feed && self.contacts.is_empty() {
            self.prev_sample = None;
            self.feed.clear_latest();
            self.touch_force = None;
            self.last_active_pressure = None;
            self.touch_pen = false;
        }

        if self.frame_samples.iter().any(|s| s.pressure > 0.0) {
            self.frame_samples.retain(|s| s.pressure > 0.0);
        }

        if hardware_from_feed || self.touch_pen {
            self.hardware_seen_in_session = true;
        }

        if let Some(s) = self.sample() {
            self.telemetry.hardware_active = true;
            self.telemetry.pressure = s.pressure;
            self.telemetry.tilt_x = s.tilt_x;
            self.telemetry.tilt_y = s.tilt_y;
            self.telemetry.rotation = s.rotation;
            self.telemetry.eraser_inverted = s.eraser;
            self.telemetry.barrel_held = s.barrel;
            self.telemetry.secondary_barrel_held = s.barrel_secondary;
        } else {
            self.telemetry.hardware_active = false;
            self.telemetry.pressure = 1.0;
            self.telemetry.eraser_inverted = false;
            self.telemetry.barrel_held = false;
            self.telemetry.secondary_barrel_held = false;
        }
    }

    pub fn sample(&self) -> Option<PenSample> {
        if let Some(s) = self.selected_sample {
            return Some(s);
        }
        let feed = self.feed.get();
        let latest = match self.frame_samples.last() {
            Some(s) if s.pressure > 0.0 && feed.is_none_or(|f| f.pressure <= 0.0) => Some(*s),
            _ => feed,
        };
        latest.or_else(|| {
            self.touch_force.map(|pressure| PenSample {
                pressure,
                time_ms: Some(self.clock_ms),
                ..Default::default()
            })
        })
    }

    /// Interpolates the pen reading for pointer move `k` out of `n` moves within a frame.
    pub fn has_hardware_samples(&self) -> bool {
        self.hardware_feed_this_frame
    }

    pub fn select(&mut self, k: usize, n: usize) {
        self.selected_sample = None;
        if n == 0 || self.frame_samples.is_empty() {
            return;
        }
        let m = self.frame_samples.len();
        let start = self
            .prev_sample
            .or_else(|| self.frame_samples.first().copied())
            .unwrap_or_default();
        let at = |idx: usize| -> PenSample {
            if idx == 0 {
                start
            } else {
                self.frame_samples.get(idx - 1).copied().unwrap_or(start)
            }
        };
        let pos = ((k + 1).min(n) as f32 / n as f32) * (m as f32);
        let i = pos.floor() as usize;
        let t = pos - pos.floor();
        let a = at(i);
        let b = at((i + 1).min(m));
        let lerp = |x: f32, y: f32| x + (y - x) * t;
        let dr = (b.rotation - a.rotation + 540.0).rem_euclid(360.0) - 180.0;
        self.selected_sample = Some(
            PenSample {
                pressure: lerp(a.pressure, b.pressure),
                tilt_x: lerp(a.tilt_x, b.tilt_x),
                tilt_y: lerp(a.tilt_y, b.tilt_y),
                rotation: a.rotation + dr * t,
                eraser: b.eraser,
                barrel: b.barrel,
                barrel_secondary: b.barrel_secondary,
                time_ms: b.time_ms.or(a.time_ms),
                azimuth: b.azimuth.or(a.azimuth),
                altitude: b.altitude.or(a.altitude),
            }
            .sanitized(),
        );
    }

    pub fn clear_selection(&mut self) {
        self.selected_sample = None;
    }

    fn apply_hold_tool(
        &mut self,
        session: &Session,
        current_freeform_lasso: bool,
        desired_tool: Option<ToolKind>,
        outcome: &mut StylusShortcutOutcome,
    ) {
        match desired_tool {
            Some(target_tool) => match self.active_hold_tool.as_mut() {
                None => {
                    self.active_hold_tool = Some(HoldToolState {
                        target_tool,
                        saved_tool: session.active_tool,
                        saved_last_pen_kind: session.last_pen_kind,
                        saved_freeform_lasso: current_freeform_lasso,
                    });
                    if session.active_tool != target_tool {
                        outcome.switch_tool = Some(target_tool);
                        outcome.cancel_modes = true;
                        if target_tool != ToolKind::Select {
                            outcome.clear_selection = true;
                        }
                    }
                    if target_tool == ToolKind::Select && !current_freeform_lasso {
                        outcome.set_freeform_lasso = Some(true);
                    }
                }
                Some(state) if state.target_tool != target_tool => {
                    let saved_lasso = state.saved_freeform_lasso;
                    state.target_tool = target_tool;
                    if session.active_tool != target_tool {
                        outcome.switch_tool = Some(target_tool);
                        outcome.cancel_modes = true;
                        if target_tool != ToolKind::Select {
                            outcome.clear_selection = true;
                        }
                    }
                    outcome.set_freeform_lasso = if target_tool == ToolKind::Select {
                        Some(true)
                    } else {
                        Some(saved_lasso)
                    };
                }
                Some(_) => {}
            },
            None => self.release_hold_tool(session, outcome),
        }
    }

    fn release_hold_tool(&mut self, session: &Session, outcome: &mut StylusShortcutOutcome) {
        let Some(saved) = self.active_hold_tool.take() else {
            return;
        };
        if session.active_tool != saved.target_tool {
            return;
        }
        if saved.saved_tool != saved.target_tool {
            outcome.switch_tool = Some(saved.saved_tool);
            outcome.cancel_modes = true;
            if saved.saved_tool != ToolKind::Select {
                outcome.clear_selection = true;
            }
        }
        outcome.restore_last_pen_kind = Some(saved.saved_last_pen_kind);
        outcome.set_freeform_lasso = Some(saved.saved_freeform_lasso);
    }

    fn apply_toggle_action(
        &mut self,
        session: &Session,
        current_freeform_lasso: bool,
        spec: ToggleSpec,
        outcome: &mut StylusShortcutOutcome,
    ) {
        if session.active_tool != spec.target_tool {
            self.tool_before_toggle = Some((
                session.active_tool,
                session.last_pen_kind,
                current_freeform_lasso,
            ));
            outcome.switch_tool = Some(spec.target_tool);
            outcome.cancel_modes = true;
            if spec.enable_lasso {
                outcome.set_freeform_lasso = Some(true);
            } else {
                outcome.clear_selection = true;
            }
            return;
        }

        if let Some((prev_tool, prev_pen, prev_lasso)) = self.tool_before_toggle.take() {
            let target = if prev_tool == spec.target_tool {
                ToolKind::Pen(spec.fallback_pen)
            } else {
                prev_tool
            };
            let restored_pen = if spec.target_tool == ToolKind::Pen(PenKind::Highlighter)
                && prev_pen == PenKind::Highlighter
                && target != ToolKind::Pen(PenKind::Highlighter)
            {
                spec.fallback_pen
            } else {
                prev_pen
            };
            outcome.switch_tool = Some(target);
            outcome.restore_last_pen_kind = Some(restored_pen);
            outcome.set_freeform_lasso = Some(prev_lasso);
            outcome.cancel_modes = true;
            if target != ToolKind::Select {
                outcome.clear_selection = true;
            }
        } else {
            let fallback_pen = if spec.target_tool == ToolKind::Pen(session.last_pen_kind) {
                spec.fallback_pen
            } else {
                session.last_pen_kind
            };
            outcome.switch_tool = Some(ToolKind::Pen(fallback_pen));
            outcome.cancel_modes = true;
            outcome.clear_selection = true;
        }
    }

    fn apply_instant_action(
        &mut self,
        session: &Session,
        current_freeform_lasso: bool,
        action: StylusButtonAction,
        outcome: &mut StylusShortcutOutcome,
    ) {
        match action {
            StylusButtonAction::Undo => outcome.execute_command = Some("edit.undo"),
            StylusButtonAction::Redo => outcome.execute_command = Some("edit.redo"),
            StylusButtonAction::ToggleEraser => self.apply_toggle_action(
                session,
                current_freeform_lasso,
                ToggleSpec {
                    target_tool: ToolKind::Eraser,
                    fallback_pen: session.last_pen_kind,
                    enable_lasso: false,
                },
                outcome,
            ),
            StylusButtonAction::ToggleSelect => self.apply_toggle_action(
                session,
                current_freeform_lasso,
                ToggleSpec {
                    target_tool: ToolKind::Select,
                    fallback_pen: session.last_pen_kind,
                    enable_lasso: true,
                },
                outcome,
            ),
            StylusButtonAction::ToggleHighlighter => self.apply_toggle_action(
                session,
                current_freeform_lasso,
                ToggleSpec {
                    target_tool: ToolKind::Pen(PenKind::Highlighter),
                    fallback_pen: PenKind::FountainPen,
                    enable_lasso: false,
                },
                outcome,
            ),
            StylusButtonAction::None
            | StylusButtonAction::EraserHold
            | StylusButtonAction::LassoSelectHold
            | StylusButtonAction::PanCanvasHold
            | StylusButtonAction::HighlighterHold => {}
        }
    }

    /// Evaluates physical stylus buttons and inverted eraser state against user preferences,
    /// returning tool, pan, and command transitions without touching UI widgets directly.
    pub fn evaluate_button_shortcuts(
        &mut self,
        session: &Session,
        current_freeform_lasso: bool,
        input: StylusButtonInput,
    ) -> StylusShortcutOutcome {
        let mut outcome = StylusShortcutOutcome::default();
        let sample = self.feed.get();
        let eraser_inverted =
            sample.is_some_and(|s| s.eraser) || input.platform_end.unwrap_or(false);
        let barrel_held = sample.is_some_and(|s| s.barrel) || input.platform_barrel;
        let secondary_barrel_held =
            sample.is_some_and(|s| s.barrel_secondary) || input.platform_secondary_barrel;

        self.telemetry.eraser_inverted = eraser_inverted;
        self.telemetry.barrel_held = barrel_held;
        self.telemetry.secondary_barrel_held = secondary_barrel_held;
        if eraser_inverted
            || barrel_held
            || secondary_barrel_held
            || input.barrel_presses > 0
            || input.secondary_barrel_presses > 0
        {
            self.hardware_seen_in_session = true;
        }

        let barrel_triggered = input.barrel_presses > 0 || (!self.prev_barrel_down && barrel_held);
        let secondary_triggered = input.secondary_barrel_presses > 0
            || (!self.prev_secondary_barrel_down && secondary_barrel_held);
        self.prev_barrel_down = barrel_held;
        self.prev_secondary_barrel_down = secondary_barrel_held;

        let cfg = session.prefs.stylus_buttons.sanitized();
        self.suppress_middle_pan_active = secondary_barrel_held
            && cfg.secondary_barrel_button != StylusButtonAction::PanCanvasHold;

        let desired_hold = if input.allow_shortcuts {
            resolve_desired_hold_action(cfg, eraser_inverted, barrel_held, secondary_barrel_held)
        } else {
            None
        };
        let wants_pan = desired_hold == Some(StylusButtonAction::PanCanvasHold);
        if self.pan_override_active != wants_pan {
            self.pan_override_active = wants_pan;
            outcome.pan_override_changed = true;
        }

        let gesture_in_progress = session.live_stroke.is_some()
            || session.live_eraser.is_some()
            || input.selection_drag_active;
        if gesture_in_progress {
            return outcome;
        }

        let desired_tool = desired_hold.and_then(hold_tool_for_action);
        self.apply_hold_tool(session, current_freeform_lasso, desired_tool, &mut outcome);

        if self.active_hold_tool.is_none()
            && !self.pan_override_active
            && input.allow_instant_actions
            && let Some(action) = pick_instant_action(cfg, barrel_triggered, secondary_triggered)
        {
            self.apply_instant_action(session, current_freeform_lasso, action, &mut outcome);
        }

        outcome
    }

    /// Promotes pen barrel contact presses (`Secondary` for Primary Barrel, `Middle` for
    /// Secondary Barrel) into `Primary` presses while a hold tool (such as Eraser, Lasso,
    /// or Highlighter hold) is active. Pure mouse clicks remain untouched.
    pub fn remap_barrel_contact_events(&mut self, raw_input: &mut egui::RawInput) {
        let can_promote = self.active_hold_tool.is_some() && !self.pan_override_active;
        for ev in &mut raw_input.events {
            match ev {
                egui::Event::PointerButton {
                    button,
                    pressed: true,
                    ..
                } if can_promote => {
                    let is_primary_barrel =
                        *button == egui::PointerButton::Secondary && self.telemetry.barrel_held;
                    let is_secondary_barrel = *button == egui::PointerButton::Middle
                        && self.telemetry.secondary_barrel_held;
                    if is_primary_barrel || is_secondary_barrel {
                        self.promoted_barrel_button = Some(*button);
                        *button = egui::PointerButton::Primary;
                    }
                }
                egui::Event::PointerButton {
                    button,
                    pressed: false,
                    ..
                } if self.promoted_barrel_button == Some(*button) => {
                    *button = egui::PointerButton::Primary;
                    self.promoted_barrel_button = None;
                }
                egui::Event::PointerGone => {
                    self.promoted_barrel_button = None;
                }
                _ => {}
            }
        }
    }

    /// Injects synthetic `Primary` pointer events from platform `PointerEvent` pen
    /// contact when a browser omits compatibility mouse events (such as inverted eraser
    /// tip `button == 5` or suppressed barrel `mousedown`), without duplicating native
    /// `Primary` events.
    pub fn apply_pen_contact_events(
        &mut self,
        raw_input: &mut egui::RawInput,
        contacts: &[PenContactEvent],
    ) {
        if contacts.is_empty() {
            if self.synthetic_contact_active
                && raw_input
                    .events
                    .iter()
                    .any(|ev| matches!(ev, egui::Event::PointerGone))
            {
                self.synthetic_contact_active = false;
            }
            return;
        }

        let has_native_primary_down = raw_input.events.iter().any(|ev| {
            matches!(
                ev,
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                }
            )
        });
        let has_native_primary_up = raw_input.events.iter().any(|ev| {
            matches!(
                ev,
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                }
            )
        });
        let hold_tool_ready = self.active_hold_tool.is_some() && !self.pan_override_active;

        for contact in contacts {
            if !contact.pos.x.is_finite() || !contact.pos.y.is_finite() {
                continue;
            }
            match contact.kind {
                PenContactKind::Down => {
                    if hold_tool_ready && !has_native_primary_down && !self.synthetic_contact_active
                    {
                        self.synthetic_contact_active = true;
                        raw_input
                            .events
                            .push(egui::Event::PointerMoved(contact.pos));
                        raw_input.events.push(egui::Event::PointerButton {
                            pos: contact.pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: contact.modifiers,
                        });
                    }
                }
                PenContactKind::Move => {
                    if self.synthetic_contact_active {
                        raw_input
                            .events
                            .push(egui::Event::PointerMoved(contact.pos));
                    }
                }
                PenContactKind::Up | PenContactKind::Cancel => {
                    if self.synthetic_contact_active {
                        self.synthetic_contact_active = false;
                        if !has_native_primary_up {
                            raw_input
                                .events
                                .push(egui::Event::PointerMoved(contact.pos));
                            raw_input.events.push(egui::Event::PointerButton {
                                pos: contact.pos,
                                button: egui::PointerButton::Primary,
                                pressed: false,
                                modifiers: contact.modifiers,
                            });
                        }
                    }
                }
            }
        }
    }
}

const MAX_IGNORED_TOUCHES: usize = 32;
const POST_PEN_MOUSE_GRACE_S: f64 = 0.08;

fn pointer_pos_matches_touch(pointer_pos: egui::Pos2, touch_pos: egui::Pos2) -> bool {
    pointer_pos.x.is_finite()
        && pointer_pos.y.is_finite()
        && touch_pos.x.is_finite()
        && touch_pos.y.is_finite()
        && (pointer_pos - touch_pos).length_sq() <= 4.0
}

fn is_touch_paired_pointer_event(
    ev: &egui::Event,
    pos: egui::Pos2,
    phase: egui::TouchPhase,
) -> bool {
    match ev {
        egui::Event::PointerMoved(p) => pointer_pos_matches_touch(*p, pos),
        egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            ..
        } => {
            pointer_pos_matches_touch(*p, pos)
                && ((phase == egui::TouchPhase::Start && *pressed)
                    || (matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel)
                        && !*pressed))
        }
        egui::Event::PointerGone => {
            matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel)
        }
        _ => false,
    }
}

fn is_candidate_touch_pointer_event(ev: &egui::Event) -> bool {
    matches!(
        ev,
        egui::Event::PointerMoved(_)
            | egui::Event::PointerButton {
                button: egui::PointerButton::Primary,
                ..
            }
            | egui::Event::PointerGone
    )
}

/// Detects web-ordered (`eframe`) touch emulation where up to 3 paired pointer events
/// (`PointerMoved`, `PointerButton::Primary`, `PointerGone`) immediately precede their
/// corresponding `Event::Touch`. Returns the index of that `Event::Touch` when every
/// intervening event in `start_idx..touch_idx` belongs to it.
fn touch_prefix_index(events: &[egui::Event], start_idx: usize) -> Option<usize> {
    if !events
        .get(start_idx)
        .is_some_and(is_candidate_touch_pointer_event)
    {
        return None;
    }
    let max_offset = (events.len().saturating_sub(start_idx + 1)).min(3);
    for offset in 1..=max_offset {
        let idx = start_idx + offset;
        match &events[idx] {
            egui::Event::Touch { pos, phase, .. } => {
                let all_paired = events[start_idx..idx]
                    .iter()
                    .all(|ev| is_touch_paired_pointer_event(ev, *pos, *phase));
                return all_paired.then_some(idx);
            }
            ev if is_candidate_touch_pointer_event(ev) => {}
            _ => return None,
        }
    }
    None
}

fn touch_tail_count(
    events: &[egui::Event],
    touch_idx: usize,
    pos: egui::Pos2,
    phase: egui::TouchPhase,
) -> usize {
    events.get(touch_idx + 1..).map_or(0, |rest| {
        rest.iter()
            .take(3)
            .take_while(|ev| is_touch_paired_pointer_event(ev, pos, phase))
            .count()
    })
}

fn batch_has_explicit_mouse_action(events: &[egui::Event]) -> bool {
    let mut i = 0;
    while i < events.len() {
        if let Some(touch_idx) = touch_prefix_index(events, i) {
            i = touch_idx + 1;
            continue;
        }
        match &events[i] {
            egui::Event::Touch { pos, phase, .. } => {
                i += 1 + touch_tail_count(events, i, *pos, *phase);
            }
            egui::Event::PointerButton { .. } | egui::Event::MouseWheel { .. } => return true,
            _ => i += 1,
        }
    }
    false
}

fn batch_has_touch_id(events: &[egui::Event], target: egui::TouchId) -> bool {
    events
        .iter()
        .any(|ev| matches!(ev, egui::Event::Touch { id, .. } if *id == target))
}

fn pen_ended_or_cancelled_in_batch(events: &[egui::Event], target: egui::TouchId) -> bool {
    events.iter().any(|ev| {
        matches!(
            ev,
            egui::Event::Touch {
                id,
                phase: egui::TouchPhase::End | egui::TouchPhase::Cancel,
                ..
            } if *id == target
        )
    })
}

impl Stylus {
    /// True when `id` belongs to a palm/non-pen contact rejected while the stylus is active.
    #[must_use]
    pub fn is_ignored_touch(&self, id: egui::TouchId) -> bool {
        self.ignored_touch_ids.contains(&id)
    }

    /// Isolates stylus pointer updates from resting palm touches, premature hover-end `PointerGone`,
    /// and spurious OS mouse-promotion `PointerMoved` events before `egui` processes `raw_input`.
    pub fn sanitize_raw_input(
        &mut self,
        raw_input: &mut egui::RawInput,
        os_pen_id: Option<u64>,
        has_pen_hook: bool,
    ) {
        self.prune_ended_ignored_touches();
        let prev_os_pen_id = self.os_pen_id;
        self.os_pen_id = os_pen_id;
        if os_pen_id.is_some() {
            self.hardware_seen_in_session = true;
        }

        let now_s = raw_input.time.filter(|t| t.is_finite() && *t >= 0.0);
        let explicit_mouse = batch_has_explicit_mouse_action(&raw_input.events);
        let pen_touch_id = self.resolve_pen_touch_id(&raw_input.events, os_pen_id, has_pen_hook);
        let batch_has_pen =
            pen_touch_id.is_some_and(|pid| batch_has_touch_id(&raw_input.events, pid));

        if os_pen_id.is_none() && explicit_mouse {
            self.last_pen_activity_s = None;
            if !batch_has_pen {
                self.last_pen_touch_id = None;
            }
        }
        if (os_pen_id.is_some() || batch_has_pen)
            && let Some(t) = now_s
        {
            self.last_pen_activity_s = Some(t);
        }

        let within_grace = !explicit_mouse
            && os_pen_id.is_none()
            && matches!(
                (now_s, self.last_pen_activity_s),
                (Some(now), Some(last)) if now >= last && (now - last) <= POST_PEN_MOUSE_GRACE_S
            );
        let suppress_mouse_moves = os_pen_id.is_some() || batch_has_pen || within_grace;
        let pen_active_now = os_pen_id.is_some() || pen_touch_id.is_some();

        let old = std::mem::take(&mut raw_input.events);
        let mut out = Vec::with_capacity(old.len().saturating_add(2));
        let mut i = 0;
        while i < old.len() {
            if let Some(touch_idx) = touch_prefix_index(&old, i) {
                let paired = &old[i..touch_idx];
                self.emit_sanitized_touch(
                    &old[touch_idx],
                    paired,
                    pen_touch_id,
                    pen_active_now,
                    &mut out,
                );
                i = touch_idx + 1;
                continue;
            }
            match &old[i] {
                egui::Event::Touch { phase, pos, .. } => {
                    let tail_len = touch_tail_count(&old, i, *pos, *phase);
                    let tail = &old[i + 1..i + 1 + tail_len];
                    self.emit_sanitized_touch(
                        &old[i],
                        tail,
                        pen_touch_id,
                        pen_active_now,
                        &mut out,
                    );
                    i += 1 + tail_len;
                }
                egui::Event::PointerMoved(_) => {
                    if !suppress_mouse_moves {
                        out.push(old[i].clone());
                    }
                    i += 1;
                }
                egui::Event::PointerGone => {
                    if os_pen_id.is_none() {
                        out.push(old[i].clone());
                    }
                    i += 1;
                }
                other => {
                    if matches!(
                        other,
                        egui::Event::PointerButton {
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            ..
                        }
                    ) {
                        self.emulated_primary_down_by_touch = None;
                    }
                    out.push(other.clone());
                    i += 1;
                }
            }
        }

        if prev_os_pen_id.is_some()
            && os_pen_id.is_none()
            && !out
                .iter()
                .any(|ev| matches!(ev, egui::Event::PointerGone | egui::Event::PointerMoved(_)))
        {
            out.push(egui::Event::PointerGone);
        }

        if os_pen_id.is_none()
            && pen_touch_id.is_some_and(|pid| pen_ended_or_cancelled_in_batch(&out, pid))
        {
            self.last_pen_touch_id = None;
        }
        raw_input.events = out;
    }

    fn emit_sanitized_touch(
        &mut self,
        touch_ev: &egui::Event,
        paired: &[egui::Event],
        pen_touch_id: Option<egui::TouchId>,
        pen_active_now: bool,
        out: &mut Vec<egui::Event>,
    ) {
        let egui::Event::Touch { id, phase, pos, .. } = *touch_ev else {
            return;
        };
        out.push(touch_ev.clone());
        if Some(id) == pen_touch_id {
            self.process_pen_touch(paired, (id, phase, pos), out);
        } else {
            if !pen_active_now && phase == egui::TouchPhase::Start {
                self.ignored_touch_ids.retain(|x| *x != id);
            }
            if pen_active_now || self.is_ignored_touch(id) {
                self.process_ignored_touch(paired, (id, phase, pos), out);
            } else {
                self.forward_touch_tail(paired, id, out);
            }
        }
    }

    fn prune_ended_ignored_touches(&mut self) {
        for ended in self.ended_ignored_touch_ids.drain(..) {
            self.ignored_touch_ids.retain(|id| *id != ended);
        }
    }

    fn record_ignored_touch(&mut self, id: egui::TouchId, phase: egui::TouchPhase) {
        if !self.ignored_touch_ids.contains(&id) {
            if self.ignored_touch_ids.len() >= MAX_IGNORED_TOUCHES {
                self.ignored_touch_ids.remove(0);
            }
            self.ignored_touch_ids.push(id);
        }
        if matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel)
            && !self.ended_ignored_touch_ids.contains(&id)
        {
            self.ended_ignored_touch_ids.push(id);
        }
    }

    fn resolve_pen_touch_id(
        &self,
        events: &[egui::Event],
        os_pen_id: Option<u64>,
        has_pen_hook: bool,
    ) -> Option<egui::TouchId> {
        if let Some(id) = os_pen_id {
            return Some(egui::TouchId(id));
        }
        if let Some(prev) = self.last_pen_touch_id
            && batch_has_touch_id(events, prev)
        {
            return Some(prev);
        }
        if !has_pen_hook {
            return events.iter().find_map(|ev| match ev {
                egui::Event::Touch {
                    id, force: Some(f), ..
                } if f.is_finite() && *f > 0.0 => Some(*id),
                _ => None,
            });
        }
        None
    }

    fn process_pen_touch(
        &mut self,
        tail: &[egui::Event],
        touch: (egui::TouchId, egui::TouchPhase, egui::Pos2),
        out: &mut Vec<egui::Event>,
    ) {
        let (id, phase, pos) = touch;
        self.last_pen_touch_id = Some(id);
        self.last_pen_pos = Some(pos);
        let pen_still_hovering = self.os_pen_id == Some(id.0);

        let mut saw_moved = false;
        let mut saw_gone = false;
        for ev in tail {
            match ev {
                egui::Event::PointerMoved(_) => {
                    saw_moved = true;
                    out.push(ev.clone());
                }
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed,
                    ..
                } => {
                    if !saw_moved && *pressed {
                        out.push(egui::Event::PointerMoved(pos));
                        saw_moved = true;
                    }
                    self.track_emulated_primary(id, *pressed);
                    out.push(ev.clone());
                }
                egui::Event::PointerGone => {
                    saw_gone = true;
                    if !pen_still_hovering {
                        out.push(egui::Event::PointerGone);
                    }
                }
                _ => out.push(ev.clone()),
            }
        }

        let ensure_hover_pos = (matches!(phase, egui::TouchPhase::Start | egui::TouchPhase::Move)
            && !saw_moved)
            || (matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel)
                && pen_still_hovering);
        if ensure_hover_pos {
            out.push(egui::Event::PointerMoved(pos));
        } else if phase == egui::TouchPhase::Cancel && !pen_still_hovering && !saw_gone {
            out.push(egui::Event::PointerGone);
        }
    }

    fn process_ignored_touch(
        &mut self,
        tail: &[egui::Event],
        touch: (egui::TouchId, egui::TouchPhase, egui::Pos2),
        out: &mut Vec<egui::Event>,
    ) {
        let (id, phase, pos) = touch;
        self.record_ignored_touch(id, phase);
        if matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel)
            && self.emulated_primary_down_by_touch == Some(id)
        {
            let modifiers = tail
                .iter()
                .find_map(|ev| match ev {
                    egui::Event::PointerButton { modifiers, .. } => Some(*modifiers),
                    _ => None,
                })
                .unwrap_or(egui::Modifiers::NONE);
            out.push(egui::Event::PointerButton {
                pos: self.last_pen_pos.unwrap_or(pos),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers,
            });
            self.emulated_primary_down_by_touch = None;
        }
    }

    fn forward_touch_tail(
        &mut self,
        tail: &[egui::Event],
        id: egui::TouchId,
        out: &mut Vec<egui::Event>,
    ) {
        for ev in tail {
            if let egui::Event::PointerButton {
                button: egui::PointerButton::Primary,
                pressed,
                ..
            } = ev
            {
                self.track_emulated_primary(id, *pressed);
            }
            out.push(ev.clone());
        }
    }

    fn track_emulated_primary(&mut self, id: egui::TouchId, pressed: bool) {
        if pressed {
            self.emulated_primary_down_by_touch = Some(id);
        } else if self.emulated_primary_down_by_touch == Some(id) {
            self.emulated_primary_down_by_touch = None;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn hover_zero_force_and_stale_feed_never_poison_touch_or_mouse() {
        let mut stylus = Stylus::default();
        let dev = egui::TouchDeviceId(1);
        let id = egui::TouchId(10);

        // Hover Move (no Start / not in contacts) with force 0.0 must not overwrite sample with 0.0.
        stylus.update(
            &[egui::Event::Touch {
                device_id: dev,
                id,
                phase: egui::TouchPhase::Move,
                pos: egui::pos2(10.0, 10.0),
                force: Some(0.0),
            }],
            10.0,
        );
        assert_eq!(stylus.sample(), None);

        // Active Start with 0.0 light touch gets contact pressure floor, never 0.0.
        stylus.update(
            &[egui::Event::Touch {
                device_id: dev,
                id,
                phase: egui::TouchPhase::Start,
                pos: egui::pos2(10.0, 10.0),
                force: Some(0.0),
            }],
            20.0,
        );
        let s = stylus.sample().unwrap();
        assert!(s.pressure >= MIN_CONTACT_PRESSURE);

        // Lift then click mouse -> clears stale pen feed/touch pressure completely.
        stylus.update(
            &[egui::Event::Touch {
                device_id: dev,
                id,
                phase: egui::TouchPhase::End,
                pos: egui::pos2(10.0, 10.0),
                force: None,
            }],
            30.0,
        );
        stylus.feed.push(Some(PenSample {
            pressure: 0.4,
            ..Default::default()
        }));
        stylus.update(&[], 35.0);
        stylus.update(
            &[egui::Event::PointerButton {
                pos: egui::pos2(50.0, 50.0),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            40.0,
        );
        assert_eq!(stylus.sample(), None);
    }

    #[test]
    fn pen_hover_ignores_palm_moves_stale_mouse_and_premature_pointer_gone() {
        let mut stylus = Stylus::default();
        let dev = egui::TouchDeviceId(1);
        let pen = egui::TouchId(9);
        let palm = egui::TouchId(1);

        // Contact end while pen remains in OS proximity keeps hover alive (no PointerGone)
        // and strips palm & stale mouse events even when palm previously touched.
        let mut raw = egui::RawInput {
            time: Some(1.0),
            events: vec![
                egui::Event::Touch {
                    device_id: dev,
                    id: pen,
                    phase: egui::TouchPhase::End,
                    pos: egui::pos2(525.0, 312.0),
                    force: None,
                },
                egui::Event::PointerButton {
                    pos: egui::pos2(525.0, 312.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
                egui::Event::Touch {
                    device_id: dev,
                    id: palm,
                    phase: egui::TouchPhase::Start,
                    pos: egui::pos2(240.0, 480.0),
                    force: None,
                },
                egui::Event::PointerMoved(egui::pos2(240.0, 480.0)),
                egui::Event::PointerButton {
                    pos: egui::pos2(240.0, 480.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerMoved(egui::pos2(80.0, 80.0)),
            ],
            ..Default::default()
        };
        stylus.sanitize_raw_input(&mut raw, Some(9), true);
        assert!(stylus.is_ignored_touch(palm));
        assert!(
            !raw.events
                .iter()
                .any(|ev| matches!(ev, egui::Event::PointerGone))
        );
        assert!(!raw.events.iter().any(|ev| matches!(
            ev,
            egui::Event::PointerMoved(p) if *p != egui::pos2(525.0, 312.0)
        )));
        assert!(!raw.events.iter().any(|ev| matches!(
            ev,
            egui::Event::PointerButton { pos, .. } if *pos != egui::pos2(525.0, 312.0)
        )));

        // Pen hover move without paired PointerMoved still drives PointerMoved; palm lift cleans up.
        let mut raw_hover = egui::RawInput {
            time: Some(1.016),
            events: vec![
                egui::Event::Touch {
                    device_id: dev,
                    id: pen,
                    phase: egui::TouchPhase::Move,
                    pos: egui::pos2(540.0, 325.0),
                    force: None,
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: palm,
                    phase: egui::TouchPhase::End,
                    pos: egui::pos2(240.0, 480.0),
                    force: None,
                },
                egui::Event::PointerButton {
                    pos: egui::pos2(240.0, 480.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
            ],
            ..Default::default()
        };
        stylus.sanitize_raw_input(&mut raw_hover, Some(9), true);
        assert!(stylus.is_ignored_touch(palm));
        assert!(raw_hover.events.iter().any(|ev| matches!(
            ev,
            egui::Event::PointerMoved(p) if *p == egui::pos2(540.0, 325.0)
        )));
        assert!(
            !raw_hover
                .events
                .iter()
                .any(|ev| matches!(ev, egui::Event::PointerGone))
        );

        // Pen exits digitizer proximity -> PointerGone emitted cleanly & ignored palm pruned.
        let mut raw_leave = egui::RawInput {
            time: Some(1.032),
            events: Vec::new(),
            ..Default::default()
        };
        stylus.sanitize_raw_input(&mut raw_leave, None, true);
        assert!(!stylus.is_ignored_touch(palm));
        assert!(
            raw_leave
                .events
                .iter()
                .any(|ev| matches!(ev, egui::Event::PointerGone))
        );

        // Within 80ms post-leave grace, stale mouse move is suppressed; after grace, mouse resumes.
        let mut raw_stale_mouse = egui::RawInput {
            time: Some(1.06),
            events: vec![egui::Event::PointerMoved(egui::pos2(90.0, 90.0))],
            ..Default::default()
        };
        stylus.sanitize_raw_input(&mut raw_stale_mouse, None, true);
        assert!(raw_stale_mouse.events.is_empty());

        let mut raw_real_mouse = egui::RawInput {
            time: Some(1.20),
            events: vec![egui::Event::PointerMoved(egui::pos2(600.0, 350.0))],
            ..Default::default()
        };
        stylus.sanitize_raw_input(&mut raw_real_mouse, None, true);
        assert!(raw_real_mouse.events.iter().any(|ev| matches!(
            ev,
            egui::Event::PointerMoved(p) if *p == egui::pos2(600.0, 350.0)
        )));
    }

    #[test]
    fn finite_and_nan_time_never_panics_during_sanitize() {
        let mut stylus = Stylus::default();
        for t in [
            None,
            Some(f64::NAN),
            Some(f64::INFINITY),
            Some(-5.0),
            Some(2.0),
            Some(1.0),
        ] {
            let mut raw = egui::RawInput {
                time: t,
                events: vec![egui::Event::PointerMoved(egui::pos2(10.0, 20.0))],
                ..Default::default()
            };
            stylus.sanitize_raw_input(&mut raw, Some(5), true);
            assert!(raw.events.is_empty());
        }
    }

    #[test]
    fn high_pressure_before_lift_does_not_leak_into_subsequent_light_stroke() {
        let mut stylus = Stylus::default();
        let feed = stylus.feed.clone();
        let pen = egui::TouchId::from(11_u64);

        // Stroke 1 ends firmly at 0.90 pressure.
        feed.push(Some(PenSample {
            pressure: 0.90,
            time_ms: Some(10.0),
            ..Default::default()
        }));
        stylus.update(
            &[
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(1),
                    id: pen,
                    phase: egui::TouchPhase::Start,
                    pos: egui::pos2(100.0, 100.0),
                    force: Some(0.90),
                },
                egui::Event::Touch {
                    device_id: egui::TouchDeviceId(1),
                    id: pen,
                    phase: egui::TouchPhase::End,
                    pos: egui::pos2(120.0, 100.0),
                    force: Some(0.90),
                },
            ],
            10.0,
        );

        // Stroke 2 starts very lightly at 0.04 pressure and must not blend up from 0.90.
        feed.push(Some(PenSample {
            pressure: 0.04,
            time_ms: Some(32.0),
            ..Default::default()
        }));
        stylus.update(
            &[egui::Event::Touch {
                device_id: egui::TouchDeviceId(1),
                id: pen,
                phase: egui::TouchPhase::Start,
                pos: egui::pos2(160.0, 100.0),
                force: Some(0.04),
            }],
            32.0,
        );

        stylus.select(0, 1);
        let sample = stylus.sample().expect("light stroke sample");
        assert!(
            sample.pressure <= 0.05,
            "expected light start pressure <= 0.05 without leak from 0.90, got {}",
            sample.pressure
        );
    }

    fn apply_outcome_to_session(
        session: &mut Session,
        lasso: &mut bool,
        outcome: StylusShortcutOutcome,
    ) {
        if let Some(tool) = outcome.switch_tool {
            session.set_active_tool(tool);
        }
        if let Some(pen) = outcome.restore_last_pen_kind {
            session.last_pen_kind = pen;
        }
        if let Some(next_lasso) = outcome.set_freeform_lasso {
            *lasso = next_lasso;
        }
    }

    #[test]
    fn stylus_connection_tracks_hotplug_unplug_and_device_labels() {
        let mut stylus = Stylus::default();
        assert!(!stylus.is_connected());
        assert_eq!(stylus.connected_device_label(), "Hardware Stylus");

        // Reporting an external pen tablet marks connected and renders its name.
        stylus.set_connected_devices(vec![ConnectedStylusInfo {
            name: "External Pen Tablet".to_string(),
            is_external: true,
        }]);
        assert!(stylus.is_connected());
        assert_eq!(stylus.connected_device_label(), "External Pen Tablet");

        // Stylus input during session marks hardware_seen_in_session too.
        stylus.hardware_seen_in_session = true;

        // Multiple connected devices summarize cleanly.
        stylus.set_connected_devices(vec![
            ConnectedStylusInfo {
                name: "Primary Tablet".to_string(),
                is_external: true,
            },
            ConnectedStylusInfo {
                name: "Integrated Stylus".to_string(),
                is_external: false,
            },
        ]);
        assert_eq!(stylus.connected_device_label(), "Primary Tablet (+1)");

        // Hot-unplugging (OS list drops from non-empty to empty) clears session visibility cleanly.
        stylus.set_connected_devices(Vec::new());
        assert!(!stylus.is_connected());
    }

    #[test]
    fn stylus_button_hold_actions_priority_deferral_and_restore_work_cleanly() {
        let mut stylus = Stylus::default();
        let mut session = Session::default();
        session.set_active_tool(ToolKind::Pen(PenKind::BallPen));
        let mut lasso = false;

        // 1. Hold LassoSelectHold on secondary barrel button (switches to Select + enables lasso),
        // then release restores BallPen and lasso=false.
        session.prefs.stylus_buttons = StylusButtonConfig {
            barrel_button: StylusButtonAction::HighlighterHold,
            secondary_barrel_button: StylusButtonAction::LassoSelectHold,
            eraser_end: StylusButtonAction::EraserHold,
        };
        let out = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                platform_secondary_barrel: true,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(out.switch_tool, Some(ToolKind::Select));
        assert_eq!(out.set_freeform_lasso, Some(true));
        assert!(stylus.suppress_middle_pan());
        apply_outcome_to_session(&mut session, &mut lasso, out);

        // Priority: barrel_button (HighlighterHold) preempts secondary (LassoSelectHold),
        // and eraser_end (EraserHold) preempts both.
        let out_barrel = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                platform_barrel: true,
                platform_secondary_barrel: true,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(
            out_barrel.switch_tool,
            Some(ToolKind::Pen(PenKind::Highlighter))
        );
        apply_outcome_to_session(&mut session, &mut lasso, out_barrel);

        let out_eraser = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                platform_end: Some(true),
                platform_barrel: true,
                platform_secondary_barrel: true,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(out_eraser.switch_tool, Some(ToolKind::Eraser));
        apply_outcome_to_session(&mut session, &mut lasso, out_eraser);

        // Mid-stroke deferral: while a live eraser/stroke is active, releasing everything must not switch tools yet.
        session.begin_live_eraser(drawcraft_geom::Point::new(10.0, 10.0));
        let out_deferred = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(out_deferred.switch_tool, None);
        session.cancel_live_eraser();

        // Once gesture finishes, original BallPen + last_pen_kind + lasso=false are restored cleanly.
        let out_restore = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(
            out_restore.switch_tool,
            Some(ToolKind::Pen(PenKind::BallPen))
        );
        assert_eq!(out_restore.restore_last_pen_kind, Some(PenKind::BallPen));
        assert_eq!(out_restore.set_freeform_lasso, Some(false));
        apply_outcome_to_session(&mut session, &mut lasso, out_restore);
        assert_eq!(session.active_tool, ToolKind::Pen(PenKind::BallPen));
        assert_eq!(session.last_pen_kind, PenKind::BallPen);
        assert!(!lasso);
    }

    #[test]
    fn stylus_button_instant_toggles_undo_redo_pan_and_fast_clicks_work_cleanly() {
        let mut stylus = Stylus::default();
        let mut session = Session::default();
        session.set_active_tool(ToolKind::Pen(PenKind::BrushPen));
        let mut lasso = false;

        // 1. Fast between-frame click (`barrel_presses = 1` with `platform_barrel = false`) triggers Undo.
        session.prefs.stylus_buttons.barrel_button = StylusButtonAction::Undo;
        let out_undo = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                barrel_presses: 1,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(out_undo.execute_command, Some("edit.undo"));

        // 2. Secondary barrel configured to Redo triggers Redo and suppresses middle-pan.
        session.prefs.stylus_buttons.barrel_button = StylusButtonAction::None;
        session.prefs.stylus_buttons.secondary_barrel_button = StylusButtonAction::Redo;
        let out_redo = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                platform_secondary_barrel: true,
                secondary_barrel_presses: 1,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(out_redo.execute_command, Some("edit.redo"));
        assert!(stylus.suppress_middle_pan());

        // Holding into next frame does NOT re-trigger Redo.
        let out_held = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                platform_secondary_barrel: true,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(out_held.execute_command, None);

        // Release.
        let _ = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );

        // 3. ToggleSelect switches BrushPen <-> Select (lasso=true) and restores BrushPen + lasso=false.
        session.prefs.stylus_buttons.barrel_button = StylusButtonAction::ToggleSelect;
        let out_sel_on = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                platform_barrel: true,
                barrel_presses: 1,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(out_sel_on.switch_tool, Some(ToolKind::Select));
        assert_eq!(out_sel_on.set_freeform_lasso, Some(true));
        apply_outcome_to_session(&mut session, &mut lasso, out_sel_on);

        let _ = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );

        let out_sel_off = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                platform_barrel: true,
                barrel_presses: 1,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(
            out_sel_off.switch_tool,
            Some(ToolKind::Pen(PenKind::BrushPen))
        );
        assert_eq!(out_sel_off.set_freeform_lasso, Some(false));
        apply_outcome_to_session(&mut session, &mut lasso, out_sel_off);

        let _ = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );

        // 4. ToggleHighlighter & ToggleEraser restore previous pen cleanly.
        session.prefs.stylus_buttons.barrel_button = StylusButtonAction::ToggleHighlighter;
        let out_hl = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                barrel_presses: 1,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(
            out_hl.switch_tool,
            Some(ToolKind::Pen(PenKind::Highlighter))
        );
        apply_outcome_to_session(&mut session, &mut lasso, out_hl);
        let out_hl_back = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                barrel_presses: 1,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(
            out_hl_back.switch_tool,
            Some(ToolKind::Pen(PenKind::BrushPen))
        );
        assert_eq!(out_hl_back.restore_last_pen_kind, Some(PenKind::BrushPen));
        apply_outcome_to_session(&mut session, &mut lasso, out_hl_back);

        session.prefs.stylus_buttons.barrel_button = StylusButtonAction::ToggleEraser;
        let out_er = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                barrel_presses: 1,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(out_er.switch_tool, Some(ToolKind::Eraser));
        apply_outcome_to_session(&mut session, &mut lasso, out_er);
        let out_er_back = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                barrel_presses: 1,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(
            out_er_back.switch_tool,
            Some(ToolKind::Pen(PenKind::BrushPen))
        );
        apply_outcome_to_session(&mut session, &mut lasso, out_er_back);

        // 5. PanCanvasHold sets pan_held() and does NOT suppress middle pan.
        session.prefs.stylus_buttons.secondary_barrel_button = StylusButtonAction::PanCanvasHold;
        let out_pan = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                platform_secondary_barrel: true,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert!(out_pan.pan_override_changed);
        assert!(stylus.pan_held());
        assert!(!stylus.suppress_middle_pan());
    }

    #[test]
    fn web_ordered_pointer_before_touch_normalizes_and_filters_palm() {
        let mut stylus = Stylus::default();
        let dev = egui::TouchDeviceId(0);
        let pen = egui::TouchId(3);
        let palm = egui::TouchId(4);
        let p0 = egui::pos2(500.0, 300.0);
        let p1 = egui::pos2(540.0, 320.0);
        let palm_pos = egui::pos2(240.0, 480.0);

        // Web runner emits PointerMoved + PointerButton(Primary, true) before Touch(Start).
        let mut raw_start = egui::RawInput {
            time: Some(1.0),
            events: vec![
                egui::Event::PointerMoved(p0),
                egui::Event::PointerButton {
                    pos: p0,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: pen,
                    phase: egui::TouchPhase::Start,
                    pos: p0,
                    force: Some(0.35),
                },
            ],
            ..Default::default()
        };
        assert!(!batch_has_explicit_mouse_action(&raw_start.events));
        stylus.sanitize_raw_input(&mut raw_start, None, false);
        assert!(matches!(
            raw_start.events.first(),
            Some(egui::Event::Touch {
                id,
                phase: egui::TouchPhase::Start,
                ..
            }) if *id == pen
        ));
        assert!(matches!(
            raw_start.events.get(1),
            Some(egui::Event::PointerMoved(p)) if *p == p0
        ));
        assert!(matches!(
            raw_start.events.get(2),
            Some(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            }) if *pos == p0
        ));

        // Resting palm in web order while pen is moving must have its paired pointer press stripped.
        let mut raw_move = egui::RawInput {
            time: Some(1.016),
            events: vec![
                egui::Event::PointerMoved(palm_pos),
                egui::Event::PointerButton {
                    pos: palm_pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::Touch {
                    device_id: dev,
                    id: palm,
                    phase: egui::TouchPhase::Start,
                    pos: palm_pos,
                    force: Some(0.0),
                },
                egui::Event::PointerMoved(p1),
                egui::Event::Touch {
                    device_id: dev,
                    id: pen,
                    phase: egui::TouchPhase::Move,
                    pos: p1,
                    force: Some(0.55),
                },
            ],
            ..Default::default()
        };
        stylus.sanitize_raw_input(&mut raw_move, None, false);
        assert!(stylus.is_ignored_touch(palm));
        assert!(!raw_move.events.iter().any(|ev| matches!(
            ev,
            egui::Event::PointerButton { pos, .. } if *pos == palm_pos
        )));
        assert!(raw_move.events.iter().any(|ev| matches!(
            ev,
            egui::Event::PointerMoved(p) if *p == p1
        )));

        // Web touchend emits PointerButton(false) + PointerGone before Touch(End).
        let mut raw_end = egui::RawInput {
            time: Some(1.032),
            events: vec![
                egui::Event::PointerButton {
                    pos: p1,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerGone,
                egui::Event::Touch {
                    device_id: dev,
                    id: pen,
                    phase: egui::TouchPhase::End,
                    pos: p1,
                    force: Some(0.0),
                },
            ],
            ..Default::default()
        };
        assert!(!batch_has_explicit_mouse_action(&raw_end.events));
        stylus.sanitize_raw_input(&mut raw_end, None, false);
        assert!(matches!(
            raw_end.events.first(),
            Some(egui::Event::Touch {
                id,
                phase: egui::TouchPhase::End,
                ..
            }) if *id == pen
        ));
        assert!(
            raw_end
                .events
                .iter()
                .any(|ev| matches!(ev, egui::Event::PointerGone))
        );
    }

    #[test]
    fn barrel_eraser_hold_remaps_secondary_button_to_primary_without_affecting_mouse() {
        let mut session = Session::default();
        session.active_tool = ToolKind::Pen(PenKind::FountainPen);
        session.prefs.stylus_buttons.barrel_button = StylusButtonAction::EraserHold;
        let mut stylus = Stylus::default();
        let mut lasso = false;
        let pos = egui::pos2(320.0, 240.0);

        // Pure mouse right-click without barrel held must stay Secondary.
        let mut mouse_raw = egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        stylus.remap_barrel_contact_events(&mut mouse_raw);
        assert!(matches!(
            mouse_raw.events.first(),
            Some(egui::Event::PointerButton {
                button: egui::PointerButton::Secondary,
                pressed: true,
                ..
            })
        ));

        // Pen holding Side Button 1 switches to Eraser and remaps Secondary press to Primary.
        let out_down = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                platform_barrel: true,
                barrel_presses: 1,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(out_down.switch_tool, Some(ToolKind::Eraser));
        apply_outcome_to_session(&mut session, &mut lasso, out_down);

        let mut pen_down = egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        stylus.remap_barrel_contact_events(&mut pen_down);
        assert!(matches!(
            pen_down.events.first(),
            Some(egui::Event::PointerButton {
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            })
        ));

        // Even if the barrel button is released before the pen lifts, the matching Secondary
        // release must still remap to Primary so the live eraser stroke terminates cleanly.
        let _ = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                platform_barrel: false,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        let mut pen_up = egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        stylus.remap_barrel_contact_events(&mut pen_up);
        assert!(matches!(
            pen_up.events.first(),
            Some(egui::Event::PointerButton {
                button: egui::PointerButton::Primary,
                pressed: false,
                ..
            })
        ));
    }

    #[test]
    fn barrel_pan_hold_does_not_remap_middle_button() {
        let mut session = Session::default();
        session.prefs.stylus_buttons.secondary_barrel_button = StylusButtonAction::PanCanvasHold;
        let mut stylus = Stylus::default();
        let _ = stylus.evaluate_button_shortcuts(
            &session,
            false,
            StylusButtonInput {
                platform_secondary_barrel: true,
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert!(stylus.pan_held());
        let mut raw = egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: egui::pos2(200.0, 200.0),
                button: egui::PointerButton::Middle,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        stylus.remap_barrel_contact_events(&mut raw);
        assert!(matches!(
            raw.events.first(),
            Some(egui::Event::PointerButton {
                button: egui::PointerButton::Middle,
                pressed: true,
                ..
            })
        ));
    }

    #[test]
    fn synthetic_pen_contact_drives_inverted_eraser_and_avoids_native_duplicates() {
        let mut session = Session::default();
        session.active_tool = ToolKind::Pen(PenKind::FountainPen);
        let mut stylus = Stylus::default();
        let mut lasso = false;

        let out = stylus.evaluate_button_shortcuts(
            &session,
            lasso,
            StylusButtonInput {
                platform_end: Some(true),
                allow_shortcuts: true,
                allow_instant_actions: true,
                ..Default::default()
            },
        );
        assert_eq!(out.switch_tool, Some(ToolKind::Eraser));
        apply_outcome_to_session(&mut session, &mut lasso, out);

        // When browser omits mousedown (e.g. button == 5), fallback injects Primary press/move/up.
        let p0 = egui::pos2(100.0, 120.0);
        let p1 = egui::pos2(140.0, 160.0);
        let mut raw_down = egui::RawInput::default();
        stylus.apply_pen_contact_events(
            &mut raw_down,
            &[PenContactEvent {
                kind: PenContactKind::Down,
                pos: p0,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(raw_down.events.iter().any(|ev| matches!(
            ev,
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                ..
            } if *pos == p0
        )));

        let mut raw_move_up = egui::RawInput::default();
        stylus.apply_pen_contact_events(
            &mut raw_move_up,
            &[
                PenContactEvent {
                    kind: PenContactKind::Move,
                    pos: p1,
                    modifiers: egui::Modifiers::NONE,
                },
                PenContactEvent {
                    kind: PenContactKind::Up,
                    pos: p1,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        assert!(raw_move_up.events.iter().any(|ev| matches!(
            ev,
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                ..
            } if *pos == p1
        )));

        // When native Primary press is already present, synthetic Down must NOT duplicate it.
        let mut raw_native = egui::RawInput {
            events: vec![egui::Event::PointerButton {
                pos: p0,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        stylus.apply_pen_contact_events(
            &mut raw_native,
            &[PenContactEvent {
                kind: PenContactKind::Down,
                pos: p0,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let primary_down_count = raw_native
            .events
            .iter()
            .filter(|ev| {
                matches!(
                    ev,
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(primary_down_count, 1);
    }
}
