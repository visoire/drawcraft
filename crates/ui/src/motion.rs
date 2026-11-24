//! Calm, standardized UI motion tokens, easing curves, and transition state machines
//! for DrawCraft (`drawcraft-ui`).
//!
//! Every screen switch, sidebar slide, modal fade/settle, tab cross-fade, page-turn
//! reveal, camera glide, and interactive hover highlight uses these shared constants
//! and helpers so transitions bring visual comfort without abrupt cuts or aggressive motion.

use drawcraft_doc::{LibrarySortBy, LibraryViewMode};
use egui::{Color32, Context, Id, Rangef, Style, Ui, style::ScrollAnimation};

use crate::AppScreen;
use crate::SettingsTab;
use crate::modal::ActiveModal;

/// Fast micro-interaction duration (hover states, selection highlights, tabs, page reveal).
pub const MICRO_DURATION_S: f32 = 0.14;

/// Standard UI transition duration (sidebars, modals, global `egui` animation clock).
pub const STANDARD_DURATION_S: f32 = 0.20;

/// Slightly crisper modal dismissal duration so returning to the canvas feels immediate.
pub const MODAL_EXIT_DURATION_S: f32 = 0.16;

/// Crisp exit duration when transitioning directly from one modal dialog to another.
pub const MODAL_SWAP_EXIT_DURATION_S: f32 = 0.11;

/// Smooth step/tab/sub-flow transition duration inside modals.
pub const MODAL_FLOW_DURATION_S: f32 = 0.16;

/// Subtle horizontal glide (in logical points) when transitioning between modal flows.
pub const MODAL_FLOW_SLIDE_PX: f32 = 16.0;

/// Outgoing screen gentle fade into the studio surface before switching screens.
pub const SCREEN_EXIT_DURATION_S: f32 = 0.085;

/// Incoming screen soft reveal after switching screens.
pub const SCREEN_ENTER_DURATION_S: f32 = 0.145;

/// Smooth camera glide duration for discrete toolbar Fit and Zoom button actions.
pub const CAMERA_GLIDE_DURATION_S: f32 = 0.18;

/// Subtle page-switch sheet veil duration (visual softness without shifting coordinates).
pub const PAGE_FADE_DURATION_S: f32 = 0.14;

/// Subtle vertical travel (in logical points) as a modal fades in or out.
pub const MODAL_SETTLE_OFFSET_Y: f32 = 10.0;

/// Peak alpha of the modal backdrop dim overlay.
pub const MODAL_BACKDROP_MAX_ALPHA: f32 = 92.0;

/// Upper bound on a single frame's delta-time so hitches or wakeups never snap transitions.
pub const MAX_FRAME_DT_S: f32 = 0.10;

/// Clamps `x` to `[0.0, 1.0]`, replacing `NaN` or `Infinity` with `0.0`.
#[inline]
pub fn clamp01(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Sanitizes a frame delta-time in seconds against negative, non-finite, or spiked values.
#[inline]
pub fn sanitize_dt(dt: f32) -> f32 {
    if dt.is_finite() && dt > 0.0 {
        dt.min(MAX_FRAME_DT_S)
    } else {
        0.0
    }
}

/// Calm cubic ease-out: responsive initial movement that settles softly with zero overshoot.
#[inline]
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = clamp01(t);
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}

/// Symmetric smoothstep curve for gentle cross-dissolves.
#[inline]
pub fn smoothstep(t: f32) -> f32 {
    let t = clamp01(t);
    t * t * (3.0 - 2.0 * t)
}

/// Linearly interpolates between `a` and `b` using `t` clamped to `[0.0, 1.0]`.
#[inline]
pub fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    let u = clamp01(t);
    a + (b - a) * u
}

/// Linearly interpolates between `a` and `b` (`f64`) using `t` clamped to `[0.0, 1.0]`.
#[inline]
pub fn lerp_f64(a: f64, b: f64, t: f64) -> f64 {
    let u = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    a + (b - a) * u
}

/// Smoothly interpolates two premultiplied `Color32` values without dark alpha halos.
#[inline]
pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, clamp01(t))
}

/// Advances `current` linearly toward `target` over `duration_s`, sanitized for hostile `dt`.
pub fn step_towards(current: f32, target: f32, dt: f32, duration_s: f32) -> f32 {
    let cur = clamp01(current);
    let tgt = clamp01(target);
    let dt = sanitize_dt(dt);
    if !duration_s.is_finite() || duration_s <= 0.0 {
        return tgt;
    }
    let step = dt / duration_s;
    if cur < tgt {
        (cur + step).min(tgt)
    } else if cur > tgt {
        (cur - step).max(tgt)
    } else {
        tgt
    }
}

/// Applies DrawCraft's calm motion timing to `egui`'s global `Style`.
pub fn apply_motion_style(style: &mut Style) {
    style.animation_time = STANDARD_DURATION_S;
    style.scroll_animation = ScrollAnimation::new(1100.0, Rangef::new(0.10, 0.24));
}

/// Returns a smooth `[0.0, 1.0]` factor for hover, focus, and pill selection states.
#[inline]
pub fn hover_factor(ctx: &Context, id: Id, active: bool) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, active, MICRO_DURATION_S, ease_out_cubic)
}

/// Returns a smooth `[0.0, 1.0]` factor synchronized with collapsible sidebars.
#[inline]
pub fn panel_reveal_factor(ctx: &Context, id: Id, open: bool) -> f32 {
    ctx.animate_bool_with_time_and_easing(id, open, STANDARD_DURATION_S, ease_out_cubic)
}

/// Computes soft content opacity during sidebar slide transitions without harsh clipping.
#[inline]
pub fn panel_content_opacity(reveal: f32) -> f32 {
    lerp_f32(0.30, 1.0, clamp01(reveal))
}

/// Paints a soft full-viewport studio-surface dissolve veil during screen switches.
pub fn paint_screen_veil(ui: &Ui, veil_alpha: f32, color: Color32) {
    let alpha = clamp01(veil_alpha);
    if alpha <= 0.001 {
        return;
    }
    let viewport = ui.ctx().content_rect();
    ui.painter()
        .with_clip_rect(viewport)
        .rect_filled(viewport, 0.0, color.gamma_multiply(alpha));
}

/// Current stage of a top-level `Home` <-> `Editor` transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScreenPhase {
    Steady,
    Exiting { from: AppScreen, progress: f32 },
    Entering { progress: f32 },
}

/// Coordinates smooth, layout-conflict-free transitions between `Home` and `Editor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenTransition {
    pub rendered: AppScreen,
    pub phase: ScreenPhase,
}

impl Default for ScreenTransition {
    fn default() -> Self {
        Self::new(AppScreen::Home)
    }
}

impl ScreenTransition {
    pub const fn new(initial: AppScreen) -> Self {
        Self {
            rendered: initial,
            phase: ScreenPhase::Steady,
        }
    }

    /// Advances the screen transition toward `target` and returns `true` while animating.
    pub fn tick(&mut self, target: AppScreen, dt: f32) -> bool {
        let dt = sanitize_dt(dt);
        match self.phase {
            ScreenPhase::Steady => {
                if target != self.rendered {
                    self.phase = ScreenPhase::Exiting {
                        from: self.rendered,
                        progress: step_towards(0.0, 1.0, dt, SCREEN_EXIT_DURATION_S),
                    };
                    self.complete_exit_if_ready(target);
                }
            }
            ScreenPhase::Exiting { from, progress } => {
                if target == from {
                    let reversed = (1.0 - clamp01(progress)).clamp(0.0, 1.0);
                    self.rendered = from;
                    self.phase = ScreenPhase::Entering {
                        progress: step_towards(reversed, 1.0, dt, SCREEN_ENTER_DURATION_S),
                    };
                    self.complete_enter_if_ready();
                } else {
                    let next = step_towards(progress, 1.0, dt, SCREEN_EXIT_DURATION_S);
                    self.phase = ScreenPhase::Exiting {
                        from,
                        progress: next,
                    };
                    self.complete_exit_if_ready(target);
                }
            }
            ScreenPhase::Entering { progress } => {
                if target != self.rendered {
                    let veil = 1.0 - ease_out_cubic(progress);
                    self.phase = ScreenPhase::Exiting {
                        from: self.rendered,
                        progress: clamp01(veil),
                    };
                    self.complete_exit_if_ready(target);
                } else {
                    let next = step_towards(progress, 1.0, dt, SCREEN_ENTER_DURATION_S);
                    self.phase = ScreenPhase::Entering { progress: next };
                    self.complete_enter_if_ready();
                }
            }
        }
        self.is_animating()
    }

    fn complete_exit_if_ready(&mut self, target: AppScreen) {
        if let ScreenPhase::Exiting { progress, .. } = self.phase
            && progress >= 1.0
        {
            self.rendered = target;
            self.phase = ScreenPhase::Entering { progress: 0.0 };
        }
    }

    fn complete_enter_if_ready(&mut self) {
        if let ScreenPhase::Entering { progress } = self.phase
            && progress >= 1.0
        {
            self.phase = ScreenPhase::Steady;
        }
    }

    /// Opacity `[0.0, 1.0]` of the soft dissolve veil painted over the active screen.
    pub fn veil_alpha(&self) -> f32 {
        match self.phase {
            ScreenPhase::Steady => 0.0,
            ScreenPhase::Exiting { progress, .. } => smoothstep(progress),
            ScreenPhase::Entering { progress } => 1.0 - ease_out_cubic(progress),
        }
    }

    /// True during the brief outgoing fade so duplicate clicks are ignored cleanly.
    #[inline]
    pub fn is_exiting(&self) -> bool {
        matches!(self.phase, ScreenPhase::Exiting { .. })
    }

    #[inline]
    pub fn is_animating(&self) -> bool {
        !matches!(self.phase, ScreenPhase::Steady)
    }
}

/// Coordinates smooth fade-in and fade-out for optional overlays such as modals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotTransition<T: Copy + Eq> {
    pub rendered: Option<T>,
    pub progress: f32,
}

impl<T: Copy + Eq> Default for SlotTransition<T> {
    fn default() -> Self {
        Self {
            rendered: None,
            progress: 0.0,
        }
    }
}

impl<T: Copy + Eq> SlotTransition<T> {
    /// Advances toward `target`. When `sizing_pass` is true on first open, keeps progress at `0.0`
    /// so backdrop and modal fade in together on the first visible frame.
    pub fn advance(&mut self, target: Option<T>, dt: f32, sizing_pass: bool) -> bool {
        let step_dt = if sizing_pass { 0.0 } else { sanitize_dt(dt) };
        match (self.rendered, target) {
            (None, None) => {
                self.progress = 0.0;
                false
            }
            (None, Some(next)) => {
                self.rendered = Some(next);
                self.progress = step_towards(0.0, 1.0, step_dt, STANDARD_DURATION_S);
                true
            }
            (Some(cur), Some(next)) if cur == next => {
                self.progress = step_towards(self.progress, 1.0, step_dt, STANDARD_DURATION_S);
                self.progress < 1.0
            }
            (Some(_), next_target) => {
                let exit_duration = if next_target.is_some() {
                    MODAL_SWAP_EXIT_DURATION_S
                } else {
                    MODAL_EXIT_DURATION_S
                };
                self.progress = step_towards(self.progress, 0.0, sanitize_dt(dt), exit_duration);
                if self.progress <= 0.0 {
                    self.rendered = next_target;
                    self.progress = 0.0;
                    self.rendered.is_some()
                } else {
                    true
                }
            }
        }
    }
}

/// Tracks modal backdrop dimming independently of dialog-to-dialog content swaps
/// so transitioning `Modal A -> Modal B` keeps a continuous calm dim without flashing bright.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BackdropTransition {
    pub progress: f32,
}

impl BackdropTransition {
    /// Advances backdrop dimming toward `1.0` while any modal is visible (including across swaps),
    /// or smoothly back toward `0.0` when all modals are dismissed.
    pub fn advance(&mut self, any_modal_visible: bool, dt: f32, sizing_pass: bool) -> bool {
        let target = if any_modal_visible { 1.0 } else { 0.0 };
        let step_dt = if sizing_pass && any_modal_visible {
            0.0
        } else {
            sanitize_dt(dt)
        };
        let duration = if any_modal_visible {
            STANDARD_DURATION_S
        } else {
            MODAL_EXIT_DURATION_S
        };
        self.progress = step_towards(self.progress, target, step_dt, duration);
        (self.progress - target).abs() > f32::EPSILON
    }

    #[inline]
    pub fn eased(&self) -> f32 {
        ease_out_cubic(self.progress)
    }
}

/// Directional horizontal slide + opacity reveal for modal step, tab, and sub-flow changes.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowTransition<K> {
    pub last_key: Option<K>,
    pub last_order: Option<isize>,
    pub progress: f32,
    pub direction: f32,
}

impl<K> Default for FlowTransition<K> {
    fn default() -> Self {
        Self {
            last_key: None,
            last_order: None,
            progress: 1.0,
            direction: 0.0,
        }
    }
}

impl<K: PartialEq> FlowTransition<K> {
    /// Clears tracked flow state (typically when a modal closes).
    pub fn reset(&mut self) {
        self.last_key = None;
        self.last_order = None;
        self.progress = 1.0;
        self.direction = 0.0;
    }

    /// Synchronizes immediately to `key` without starting a slide or fade.
    pub fn sync(&mut self, key: K, order: isize) {
        self.last_key = Some(key);
        self.last_order = Some(order);
        self.progress = 1.0;
        self.direction = 0.0;
    }

    /// Advances toward settled state (`1.0`), restarting cleanly whenever `key` changes after
    /// an initial observation. Forward order changes glide in from the right; backward from the left.
    pub fn advance(&mut self, key: K, order: isize, dt: f32) -> bool {
        if self.last_key.as_ref().is_some_and(|prev| prev != &key) {
            let dir = match self.last_order {
                Some(prev_order) if order > prev_order => 1.0,
                Some(prev_order) if order < prev_order => -1.0,
                _ => 1.0,
            };
            self.progress = 0.0;
            self.direction = dir;
        }
        self.last_key = Some(key);
        self.last_order = Some(order);
        self.progress = step_towards(self.progress, 1.0, dt, MODAL_FLOW_DURATION_S);
        if self.progress >= 1.0 {
            self.direction = 0.0;
            false
        } else {
            true
        }
    }

    #[inline]
    pub fn opacity(&self) -> f32 {
        lerp_f32(0.18, 1.0, ease_out_cubic(self.progress))
    }

    #[inline]
    pub fn offset_x(&self) -> f32 {
        let settled = ease_out_cubic(self.progress);
        self.direction * MODAL_FLOW_SLIDE_PX * (1.0 - settled)
    }
}

/// Smoothly expands or collapses inline secondary UI using an animated height slot,
/// opacity crossfade, and child clipping so drawers inside modals open and close smoothly.
pub fn animated_collapse(
    ui: &mut Ui,
    id: Id,
    open: bool,
    fallback_h: f32,
    add_contents: impl FnOnce(&mut Ui),
) {
    let reveal = panel_reveal_factor(ui.ctx(), id, open);
    if !open && reveal <= 0.001 {
        return;
    }

    let height_id = id.with("collapse_h");
    let cached_h = ui
        .ctx()
        .data(|d| d.get_temp::<f32>(height_id))
        .filter(|h| h.is_finite() && *h > 1.0);

    // When settled open (including frame 1 if initially open), lay out directly at natural
    // height so tests and parent cards always measure exact unclipped bounds.
    if open && reveal >= 0.999 {
        let resp = ui.scope(|ui| {
            add_contents(ui);
        });
        let natural_h = resp.response.rect.height();
        if natural_h.is_finite() && natural_h > 1.0 {
            ui.ctx().data_mut(|d| d.insert_temp(height_id, natural_h));
        }
        return;
    }

    let avail_w = ui.available_width().max(1.0);
    let eased_h = smoothstep(reveal);
    let known_h = cached_h.unwrap_or_else(|| fallback_h.max(1.0));
    let target_h = (known_h * eased_h).max(0.0);
    if target_h <= 0.5 {
        return;
    }

    let (slot_rect, _) =
        ui.allocate_exact_size(egui::vec2(avail_w, target_h), egui::Sense::hover());
    let clip_rect = ui.clip_rect().intersect(slot_rect);
    let child_max = egui::Rect::from_min_size(
        slot_rect.min,
        egui::vec2(avail_w, (known_h.max(fallback_h) + 64.0).max(200.0)),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(child_max)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(clip_rect);
    child.set_width(avail_w);
    child.multiply_opacity(lerp_f32(0.08, 1.0, ease_out_cubic(reveal)));
    if !open {
        // Prevent stray clicks on controls inside a collapsing drawer while fading shut.
        child.disable();
    }
    add_contents(&mut child);
    let measured_h = child.min_rect().height();
    if measured_h.is_finite() && measured_h > 1.0 {
        ui.ctx().data_mut(|d| d.insert_temp(height_id, measured_h));
    }
}

#[inline]
fn reveal_opacity(progress: f32) -> f32 {
    lerp_f32(0.15, 1.0, ease_out_cubic(progress))
}

/// Key identifying the Home Library's active layout, sort order, and empty-state status.
pub type HomeViewKey = (LibraryViewMode, LibrarySortBy, bool, bool);

/// Soft opacity reveal when switching Library views, folders, or display modes
/// without shifting layout coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct RevealFade<K> {
    pub last_key: Option<K>,
    pub progress: f32,
}

impl<K> Default for RevealFade<K> {
    fn default() -> Self {
        Self {
            last_key: None,
            progress: 1.0,
        }
    }
}

impl<K: PartialEq> RevealFade<K> {
    /// Synchronizes the tracked key immediately without starting a fade.
    pub fn sync(&mut self, key: K) {
        self.last_key = Some(key);
        self.progress = 1.0;
    }

    /// Advances toward full opacity (`1.0`), restarting from `0.0` when `key` changes
    /// after an initial observation. Returns `true` while animating.
    pub fn advance(&mut self, key: K, dt: f32) -> bool {
        if self.last_key.as_ref().is_some_and(|prev| prev != &key) {
            self.progress = 0.0;
        }
        self.last_key = Some(key);
        self.progress = step_towards(self.progress, 1.0, dt, MICRO_DURATION_S);
        self.progress < 1.0
    }

    #[inline]
    pub fn opacity(&self) -> f32 {
        reveal_opacity(self.progress)
    }
}

/// Soft opacity reveal when switching tabs inside the Settings drawer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabFadeTransition {
    pub last_tab: SettingsTab,
    pub progress: f32,
}

impl Default for TabFadeTransition {
    fn default() -> Self {
        Self {
            last_tab: SettingsTab::Pen,
            progress: 1.0,
        }
    }
}

impl TabFadeTransition {
    pub fn advance(&mut self, tab: SettingsTab, dt: f32) -> bool {
        if tab != self.last_tab {
            self.last_tab = tab;
            self.progress = 0.0;
        }
        self.progress = step_towards(self.progress, 1.0, dt, MICRO_DURATION_S);
        self.progress < 1.0
    }

    #[inline]
    pub fn opacity(&self) -> f32 {
        reveal_opacity(self.progress)
    }
}

/// Subtle non-displacing veil fade when turning pages inside the same notebook.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageFadeTransition {
    pub last_key: Option<(usize, usize)>,
    pub progress: f32,
}

impl Default for PageFadeTransition {
    fn default() -> Self {
        Self {
            last_key: None,
            progress: 1.0,
        }
    }
}

impl PageFadeTransition {
    pub fn advance(&mut self, key: (usize, usize), dt: f32) -> bool {
        if let Some(prev) = self.last_key {
            if prev.0 != key.0 {
                self.progress = 1.0;
            } else if prev.1 != key.1 {
                self.progress = 0.0;
            }
        }
        self.last_key = Some(key);
        self.progress = step_towards(self.progress, 1.0, dt, PAGE_FADE_DURATION_S);
        self.progress < 1.0
    }

    #[inline]
    pub fn veil_alpha(&self) -> f32 {
        (1.0 - ease_out_cubic(self.progress)) * 0.45
    }
}

/// Smooth log-zoom and straight-line anchor glide for discrete Fit and Zoom actions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraGlide {
    pub start_zoom: f32,
    pub start_pan: [f64; 2],
    pub target_zoom: f32,
    pub target_pan: [f64; 2],
    pub anchor_screen: [f64; 2],
    pub progress: f32,
}

impl CameraGlide {
    pub fn new(
        start_zoom: f32,
        start_pan: [f64; 2],
        target_zoom: f32,
        target_pan: [f64; 2],
        anchor_screen: [f64; 2],
    ) -> Self {
        let safe_zoom = |z: f32| {
            if z.is_finite() {
                z.clamp(0.15, 6.0)
            } else {
                1.0
            }
        };
        let safe_pair = |p: [f64; 2]| {
            [
                if p[0].is_finite() { p[0] } else { 0.0 },
                if p[1].is_finite() { p[1] } else { 0.0 },
            ]
        };
        Self {
            start_zoom: safe_zoom(start_zoom),
            start_pan: safe_pair(start_pan),
            target_zoom: safe_zoom(target_zoom),
            target_pan: safe_pair(target_pan),
            anchor_screen: safe_pair(anchor_screen),
            progress: 0.0,
        }
    }

    /// Advances the glide by `dt`, returning `(zoom, pan, finished)`.
    pub fn step(&mut self, dt: f32) -> (f32, [f64; 2], bool) {
        self.progress = step_towards(self.progress, 1.0, dt, CAMERA_GLIDE_DURATION_S);
        let u = f64::from(ease_out_cubic(self.progress));
        let s0 = f64::from(self.start_zoom).max(0.05);
        let s1 = f64::from(self.target_zoom).max(0.05);
        let scale = s0 * (s1 / s0).powf(u);

        let ax = self.anchor_screen[0];
        let ay = self.anchor_screen[1];
        let p0x = (ax - self.start_pan[0]) / s0;
        let p0y = (ay - self.start_pan[1]) / s0;
        let p1x = (ax - self.target_pan[0]) / s1;
        let p1y = (ay - self.target_pan[1]) / s1;

        let px = lerp_f64(p0x, p1x, u);
        let py = lerp_f64(p0y, p1y, u);
        let pan = [ax - px * scale, ay - py * scale];
        let finished = self.progress >= 1.0;
        (scale as f32, pan, finished)
    }
}

/// Consolidated UI transition state owned by [`crate::ViewState`].
#[derive(Clone, Debug, Default)]
pub struct MotionState {
    pub screen: ScreenTransition,
    pub modal: SlotTransition<ActiveModal>,
    pub modal_backdrop: BackdropTransition,
    pub modal_flow: FlowTransition<(u64, u8, usize)>,
    pub modal_subflow: FlowTransition<(u64, u8, u8)>,
    pub settings_tab: TabFadeTransition,
    pub page_fade: PageFadeTransition,
    pub home_scope: RevealFade<String>,
    pub home_view: RevealFade<HomeViewKey>,
    pub camera_glide: Option<CameraGlide>,
    pub fit_glide_requested: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_and_step_resist_hostile_inputs() {
        assert_eq!(clamp01(f32::NAN), 0.0);
        assert_eq!(clamp01(f32::INFINITY), 0.0);
        assert_eq!(clamp01(-2.0), 0.0);
        assert_eq!(clamp01(3.0), 1.0);

        assert_eq!(sanitize_dt(f32::NAN), 0.0);
        assert_eq!(sanitize_dt(-1.0), 0.0);
        assert_eq!(sanitize_dt(5.0), MAX_FRAME_DT_S);

        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert!(ease_out_cubic(0.5) > 0.5);

        assert_eq!(step_towards(0.2, 0.8, f32::NAN, 0.2), 0.2);
        assert_eq!(step_towards(0.2, 0.8, 0.05, 0.0), 0.8);
        assert_eq!(step_towards(0.2, 0.8, 0.05, -1.0), 0.8);
    }

    #[test]
    fn screen_transition_dissolves_out_switches_and_reveals_calmly() {
        let mut st = ScreenTransition::new(AppScreen::Home);
        assert!(!st.is_animating());
        assert_eq!(st.veil_alpha(), 0.0);

        // Start Home -> Editor
        assert!(st.tick(AppScreen::Editor, 0.04));
        assert_eq!(st.rendered, AppScreen::Home);
        assert!(st.is_exiting());
        assert!(st.veil_alpha() > 0.0 && st.veil_alpha() < 1.0);

        // Complete exit phase -> switches rendered screen and starts Entering
        assert!(st.tick(AppScreen::Editor, 0.06));
        assert_eq!(st.rendered, AppScreen::Editor);
        assert!(!st.is_exiting());
        assert_eq!(st.veil_alpha(), 1.0);

        // Advance entering phase to steady state
        for _ in 0..5 {
            st.tick(AppScreen::Editor, 0.05);
        }
        assert!(!st.is_animating());
        assert_eq!(st.rendered, AppScreen::Editor);
        assert_eq!(st.veil_alpha(), 0.0);
    }

    #[test]
    fn screen_transition_reverses_cleanly_if_cancelled_mid_exit() {
        let mut st = ScreenTransition::new(AppScreen::Home);
        st.tick(AppScreen::Editor, 0.04);
        assert!(st.is_exiting());

        // User immediately goes back to Home before exit finishes
        st.tick(AppScreen::Home, 0.02);
        assert_eq!(st.rendered, AppScreen::Home);
        assert!(!st.is_exiting());

        for _ in 0..5 {
            st.tick(AppScreen::Home, 0.05);
        }
        assert!(!st.is_animating());
        assert_eq!(st.veil_alpha(), 0.0);
    }

    #[test]
    fn modal_slot_transition_opens_and_closes_smoothly() {
        let mut slot = SlotTransition::default();
        assert!(!slot.advance(None, 0.02, false));

        // Sizing frame keeps progress at 0.0 while allocating rendered slot.
        assert!(slot.advance(Some(ActiveModal::Export), 0.02, true));
        assert_eq!(slot.rendered, Some(ActiveModal::Export));
        assert_eq!(slot.progress, 0.0);

        // Subsequent frames fade smoothly to 1.0.
        assert!(slot.advance(Some(ActiveModal::Export), 0.05, false));
        assert!(slot.progress > 0.0 && slot.progress < 1.0);
        for _ in 0..5 {
            slot.advance(Some(ActiveModal::Export), 0.05, false);
        }
        assert!(!slot.advance(Some(ActiveModal::Export), 0.05, false));
        assert_eq!(slot.progress, 1.0);

        // Closing retains rendered modal while fading out smoothly to 0.0.
        assert!(slot.advance(None, 0.05, false));
        assert_eq!(slot.rendered, Some(ActiveModal::Export));
        assert!(slot.progress > 0.0 && slot.progress < 1.0);

        for _ in 0..5 {
            slot.advance(None, 0.05, false);
        }
        assert!(!slot.advance(None, 0.05, false));
        assert_eq!(slot.rendered, None);
        assert_eq!(slot.progress, 0.0);
    }

    #[test]
    fn camera_glide_preserves_anchor_and_reaches_target() {
        let mut glide = CameraGlide::new(1.0, [100.0, 50.0], 2.0, [-200.0, -150.0], [400.0, 250.0]);
        let (z_mid, pan_mid, done_mid) = glide.step(0.09);
        assert!(!done_mid);
        assert!(z_mid > 1.0 && z_mid < 2.0);
        assert!(pan_mid[0] < 100.0 && pan_mid[0] > -200.0);

        let mut final_res = (0.0, [0.0, 0.0], false);
        for _ in 0..5 {
            final_res = glide.step(0.05);
        }
        assert!(final_res.2);
        assert!((final_res.0 - 2.0).abs() < 1e-4);
        assert!((final_res.1[0] - (-200.0)).abs() < 1e-3);
        assert!((final_res.1[1] - (-150.0)).abs() < 1e-3);
    }

    #[test]
    fn page_fade_only_triggers_when_page_changes_in_same_notebook() {
        let mut pf = PageFadeTransition::default();
        assert!(!pf.advance((0, 0), 0.02));
        assert_eq!(pf.veil_alpha(), 0.0);

        // Page turn within notebook 0 triggers fade.
        assert!(pf.advance((0, 1), 0.02));
        assert!(pf.veil_alpha() > 0.0);
        for _ in 0..5 {
            pf.advance((0, 1), 0.05);
        }
        assert_eq!(pf.veil_alpha(), 0.0);

        // Switching to notebook 1 does NOT trigger page-turn veil.
        assert!(!pf.advance((1, 0), 0.02));
        assert_eq!(pf.veil_alpha(), 0.0);
    }

    #[test]
    fn reveal_fade_stays_steady_on_first_frame_and_fades_on_key_change() {
        let mut fade = RevealFade::default();
        assert!(!fade.advance("all".to_string(), 0.0));
        assert_eq!(fade.opacity(), 1.0);

        assert!(fade.advance("folder:test/mk".to_string(), 0.0));
        assert!(fade.opacity() < 0.3);

        for _ in 0..4 {
            fade.advance("folder:test/mk".to_string(), 0.04);
        }
        assert!(!fade.advance("folder:test/mk".to_string(), 0.04));
        assert_eq!(fade.opacity(), 1.0);
    }

    #[test]
    fn modal_swap_keeps_backdrop_continuous_and_uses_crisp_exit() {
        let mut slot = SlotTransition::default();
        let mut backdrop = BackdropTransition::default();

        // Settle NotebookSettings open.
        slot.advance(Some(ActiveModal::NotebookSettings), 0.02, true);
        backdrop.advance(true, 0.02, true);
        for _ in 0..8 {
            slot.advance(Some(ActiveModal::NotebookSettings), 0.05, false);
            backdrop.advance(true, 0.05, false);
        }
        assert_eq!(slot.rendered, Some(ActiveModal::NotebookSettings));
        assert_eq!(slot.progress, 1.0);
        assert_eq!(backdrop.progress, 1.0);

        // Swap NotebookSettings -> ConfirmDeleteNotebook: slot exits over MODAL_SWAP_EXIT_DURATION_S
        // while backdrop remains steadily dimmed at 1.0.
        assert!(slot.advance(Some(ActiveModal::ConfirmDeleteNotebook), 0.055, false));
        assert!(!backdrop.advance(true, 0.055, false));
        assert_eq!(slot.rendered, Some(ActiveModal::NotebookSettings));
        assert!((slot.progress - 0.5).abs() < 1e-4);
        assert_eq!(backdrop.progress, 1.0);

        // Complete exit phase -> swaps immediately to ConfirmDeleteNotebook and retains dim backdrop.
        assert!(slot.advance(Some(ActiveModal::ConfirmDeleteNotebook), 0.06, false));
        assert_eq!(slot.rendered, Some(ActiveModal::ConfirmDeleteNotebook));
        assert_eq!(slot.progress, 0.0);
        assert_eq!(backdrop.progress, 1.0);

        // New modal settles in smoothly.
        for _ in 0..6 {
            slot.advance(Some(ActiveModal::ConfirmDeleteNotebook), 0.05, false);
        }
        assert_eq!(slot.progress, 1.0);
    }

    #[test]
    fn flow_transition_glides_directionally_between_steps_and_syncs_cleanly() {
        let mut flow = FlowTransition::default();
        flow.sync((0, 1, 0), 0);
        assert_eq!(flow.opacity(), 1.0);
        assert_eq!(flow.offset_x(), 0.0);

        // Forward step 0 -> 1 glides in from the right (+offset_x).
        assert!(flow.advance((0, 1, 1), 1, 0.02));
        assert!(flow.opacity() < 1.0);
        assert!(flow.offset_x() > 0.0);

        for _ in 0..6 {
            flow.advance((0, 1, 1), 1, 0.05);
        }
        assert_eq!(flow.opacity(), 1.0);
        assert_eq!(flow.offset_x(), 0.0);

        // Backward step 1 -> 0 glides in from the left (-offset_x).
        assert!(flow.advance((0, 1, 0), 0, 0.02));
        assert!(flow.offset_x() < 0.0);

        flow.reset();
        assert_eq!(flow.last_key, None);
        assert_eq!(flow.offset_x(), 0.0);
    }

    #[test]
    fn animated_collapse_interpolates_height_smoothly_between_open_and_closed() {
        let ctx = Context::default();
        let id = Id::unique("test_animated_collapse");
        let run = |open: bool, time: f64| -> f32 {
            let mut allocated_h = 0.0;
            let raw = egui::RawInput {
                time: Some(time),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                let start_y = ui.cursor().top();
                animated_collapse(ui, id, open, 120.0, |ui| {
                    ui.allocate_exact_size(egui::vec2(180.0, 120.0), egui::Sense::hover());
                });
                allocated_h = (ui.cursor().top() - start_y).max(0.0);
            });
            out.drop_without_applying_deltas();
            allocated_h
        };

        // Frame 1 closed -> 0 height allocated.
        assert_eq!(run(false, 0.0), 0.0);
        // Frame 2 open -> intermediate partial height, then settles near 120px.
        let mid_open = run(true, 0.05);
        assert!(mid_open > 0.0 && mid_open < 119.0, "mid_open={mid_open}");
        let settled_open = run(true, 0.35);
        assert!(
            (settled_open - 120.0).abs() < 6.0,
            "settled_open={settled_open}"
        );
        // Closing shrinks smoothly without jumping immediately to 0.
        let mid_close = run(false, 0.42);
        assert!(
            mid_close > 0.0 && mid_close < settled_open,
            "mid_close={mid_close}"
        );
        let settled_close = run(false, 0.75);
        assert_eq!(settled_close, 0.0);
    }
}
