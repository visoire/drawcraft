//! UI design tokens and Light/Dark theme visual styling for DrawCraft.

use drawcraft_color::Color;
use drawcraft_doc::UiTheme;
use egui::Color32;

#[inline]
pub fn to_color32(c: Color) -> Color32 {
    let [r, g, b, a] = c.to_rgba8();
    Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// Color token set backing every DrawCraft shell, sidebar, dialog, and canvas backdrop.
///
/// Calm, flat studio: warm daylight (Light) or neutral anthracite (Dark), hairline borders, one
/// accent. Controls are ghosts on these surfaces; only floating popovers get a shadow.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub is_dark: bool,
    pub toolbar_bg: Color32,
    pub panel_bg: Color32,
    pub canvas_bg: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_soft: Color32,
    pub field_bg: Color32,
    /// Background of a hovered ghost control.
    pub ghost_hover: Color32,
    /// Background of a pressed or active tool pill.
    pub pill_active: Color32,
    pub danger_fg: Color32,
    pub danger_border: Color32,
    pub danger_bg: Color32,
    pub success_fg: Color32,
    pub success_border: Color32,
    pub success_bg: Color32,
    pub warning_fg: Color32,
}

impl Tokens {
    /// Returns the token set for the active [`UiTheme`].
    pub fn for_theme(theme: UiTheme) -> Self {
        match theme {
            UiTheme::Light => Self::light(),
            UiTheme::Dark => Self::dark(),
        }
    }

    /// Quiet iPadOS daylight: `#FFFFFF` toolbar & cards, `#F5F5F7` sidebar & tab strip, `#EBECEF` desk
    /// behind paper, `#E5E5EA` hairlines and one system-blue accent (darkened slightly from
    /// `#007AFF` so white labels keep WCAG AA contrast).
    pub fn light() -> Self {
        Self {
            is_dark: false,
            toolbar_bg: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            panel_bg: Color32::from_rgb(0xF5, 0xF5, 0xF7),
            canvas_bg: Color32::from_rgb(0xEB, 0xEC, 0xEF),
            border: Color32::from_rgb(0xE5, 0xE5, 0xEA),
            text: Color32::from_rgb(0x1C, 0x1C, 0x1E),
            text_muted: Color32::from_rgb(0x66, 0x66, 0x6B),
            accent: Color32::from_rgb(0x00, 0x68, 0xF5),
            accent_hover: Color32::from_rgb(0x00, 0x5A, 0xD6),
            accent_soft: Color32::from_rgb(0xE4, 0xEE, 0xFC),
            field_bg: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            ghost_hover: Color32::from_rgba_unmultiplied(0x1C, 0x1C, 0x1E, 0x0A),
            pill_active: Color32::from_rgb(0xEC, 0xEC, 0xF0),
            danger_fg: Color32::from_rgb(0xB4, 0x28, 0x28),
            danger_border: Color32::from_rgb(0xE4, 0xBE, 0xBE),
            danger_bg: Color32::from_rgb(0xFD, 0xF5, 0xF5),
            success_fg: Color32::from_rgb(0x15, 0x80, 0x3D),
            success_border: Color32::from_rgb(0x9E, 0xDE, 0xB8),
            success_bg: Color32::from_rgb(0xEC, 0xFD, 0xF3),
            warning_fg: Color32::from_rgb(0xB4, 0x53, 0x09),
        }
    }

    /// Neutral graphite studio (saturation 0, no blue cast): `#0E0E10` desk, `#141416` sidebar and
    /// tab strip, `#1C1C1E` toolbar/cards, `#262629` raised controls, `#2C2C30` active pill.
    pub fn dark() -> Self {
        Self {
            is_dark: true,
            toolbar_bg: Color32::from_rgb(0x1C, 0x1C, 0x1E),
            panel_bg: Color32::from_rgb(0x14, 0x14, 0x16),
            canvas_bg: Color32::from_rgb(0x0E, 0x0E, 0x10),
            border: Color32::from_rgb(0x2A, 0x2A, 0x2E),
            text: Color32::from_rgb(0xF2, 0xF2, 0xF5),
            text_muted: Color32::from_rgb(0x8E, 0x8E, 0x93),
            accent: Color32::from_rgb(0x4D, 0x8B, 0xFF),
            accent_hover: Color32::from_rgb(0x6B, 0x9E, 0xFF),
            accent_soft: Color32::from_rgb(0x2C, 0x2C, 0x30),
            field_bg: Color32::from_rgb(0x26, 0x26, 0x29),
            ghost_hover: Color32::from_rgba_unmultiplied(0xFF, 0xFF, 0xFF, 0x0F),
            pill_active: Color32::from_rgb(0x2C, 0x2C, 0x30),
            danger_fg: Color32::from_rgb(0xF0, 0x7A, 0x7A),
            danger_border: Color32::from_rgb(0x5E, 0x31, 0x36),
            danger_bg: Color32::from_rgb(0x2D, 0x1C, 0x20),
            success_fg: Color32::from_rgb(0x3E, 0xD0, 0x76),
            success_border: Color32::from_rgb(0x23, 0x5B, 0x3B),
            success_bg: Color32::from_rgb(0x14, 0x2B, 0x1E),
            warning_fg: Color32::from_rgb(0xF5, 0x9E, 0x0B),
        }
    }
}

/// Corner radius of buttons, tabs and inputs (concentric with 14 px cards and popovers).
pub const CONTROL_RADIUS: u8 = 10;
/// Corner radius of cards, popovers and windows.
pub const CARD_RADIUS: u8 = 14;

/// Applies DrawCraft's active studio palette (`Light` or `Dark`) to standard `egui` widgets.
pub fn apply_visuals(ctx: &egui::Context, theme: UiTheme) {
    let t = Tokens::for_theme(theme);
    let egui_theme = match theme {
        UiTheme::Light => egui::Theme::Light,
        UiTheme::Dark => egui::Theme::Dark,
    };
    let active_style = ctx.style_of(egui_theme);
    let needs_apply = ctx.theme() != egui_theme
        || active_style.visuals.dark_mode != t.is_dark
        || active_style.visuals.panel_fill != t.panel_bg
        || active_style.visuals.extreme_bg_color != t.field_bg
        || (active_style.animation_time - crate::motion::STANDARD_DURATION_S).abs() > 1e-4;
    if !needs_apply {
        return;
    }

    ctx.set_theme(egui_theme);
    let mut visuals = if t.is_dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.override_text_color = Some(t.text);
    visuals.panel_fill = t.panel_bg;
    visuals.window_fill = t.field_bg;
    visuals.window_stroke = egui::Stroke::new(1.0, t.border);
    visuals.window_corner_radius = egui::CornerRadius::same(CARD_RADIUS);
    visuals.menu_corner_radius = egui::CornerRadius::same(CARD_RADIUS);
    // Shadow only on floating surfaces; panels stay flat.
    let shadow_alpha = if t.is_dark { 110 } else { 38 };
    visuals.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_rgba_unmultiplied(0, 0, 0, shadow_alpha),
    };
    visuals.popup_shadow = visuals.window_shadow;
    visuals.faint_bg_color = t.toolbar_bg;
    visuals.extreme_bg_color = t.field_bg;
    visuals.text_edit_bg_color = Some(t.field_bg);
    visuals.selection.bg_fill = t.accent.gamma_multiply(0.28);
    visuals.selection.stroke = egui::Stroke::new(1.5, t.accent);

    let control_radius = egui::CornerRadius::same(CONTROL_RADIUS);
    let no_stroke = egui::Stroke::NONE;

    visuals.widgets.noninteractive.bg_fill = t.panel_bg;
    visuals.widgets.noninteractive.weak_bg_fill = t.toolbar_bg;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, t.border);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, t.text);
    visuals.widgets.noninteractive.corner_radius = control_radius;

    // Ghost controls: no fill and no outline at rest.
    visuals.widgets.inactive.bg_fill = t.field_bg;
    visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    visuals.widgets.inactive.bg_stroke = no_stroke;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, t.text);
    visuals.widgets.inactive.corner_radius = control_radius;

    visuals.widgets.hovered.bg_fill = t.ghost_hover;
    visuals.widgets.hovered.weak_bg_fill = t.ghost_hover;
    visuals.widgets.hovered.bg_stroke = no_stroke;
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, t.text);
    visuals.widgets.hovered.corner_radius = control_radius;

    visuals.widgets.active.bg_fill = t.pill_active;
    visuals.widgets.active.weak_bg_fill = t.pill_active;
    visuals.widgets.active.bg_stroke = no_stroke;
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, t.text);
    visuals.widgets.active.corner_radius = control_radius;

    visuals.widgets.open.bg_fill = t.pill_active;
    visuals.widgets.open.weak_bg_fill = t.pill_active;
    visuals.widgets.open.bg_stroke = no_stroke;
    visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0, t.text);
    visuals.widgets.open.corner_radius = control_radius;

    ctx.set_visuals(visuals);
    ctx.all_styles_mut(crate::motion::apply_motion_style);
}

/// Font family of display headings (Geist SemiBold).
pub const DISPLAY_FAMILY: &str = "StudyDisplay";
/// Font family of medium-weight labels (Inter Medium).
pub const MEDIUM_FAMILY: &str = "StudyMedium";
/// Font family of strong labels (Inter SemiBold).
pub const SEMIBOLD_FAMILY: &str = "StudySemibold";

/// Local SIL-OFL fonts (see `assets/fonts/LICENSE-OFL.txt`).
const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
const INTER_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
const GEIST_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Geist-SemiBold.ttf");
const GEIST_MONO: &[u8] = include_bytes!("../assets/fonts/GeistMono-Regular.ttf");

/// Registers Inter (body), Geist (display) and Geist Mono (numbers) in `fonts`. Inter becomes the
/// default proportional font; the egui defaults stay behind them as glyph fallbacks (symbols).
pub(crate) fn add_brand_fonts(fonts: &mut egui::FontDefinitions) {
    use egui::{FontData, FontFamily};
    for (name, bytes) in [
        ("inter_regular", INTER_REGULAR),
        ("inter_medium", INTER_MEDIUM),
        ("inter_semibold", INTER_SEMIBOLD),
        ("geist_semibold", GEIST_SEMIBOLD),
        ("geist_mono", GEIST_MONO),
    ] {
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(FontData::from_static(bytes)),
        );
    }
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "inter_regular".to_owned());
    let fallback: Vec<String> = proportional.clone();
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "geist_mono".to_owned());
    for (family, primary) in [
        (DISPLAY_FAMILY, "geist_semibold"),
        (MEDIUM_FAMILY, "inter_medium"),
        (SEMIBOLD_FAMILY, "inter_semibold"),
    ] {
        let mut list = vec![primary.to_owned()];
        list.extend(fallback.iter().filter(|n| *n != primary).cloned());
        fonts.families.insert(FontFamily::Name(family.into()), list);
    }
}

/// Applies the brand text scale. Must run one frame *after* [`add_brand_fonts`] was handed to
/// `Context::set_fonts` (egui binds new font families at the next pass).
pub(crate) fn apply_brand_text_styles(ctx: &egui::Context) {
    use egui::{FontFamily, FontId, TextStyle};
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(22.0, FontFamily::Name(DISPLAY_FAMILY.into())),
        );
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Name(MEDIUM_FAMILY.into())),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        );
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
    });
}

/// Renders a `ComboBox` matching DrawCraft's standard 28px control height,
/// 7px corner radius, crisp border, and pointing-hand hover cursor.
pub(crate) fn studio_combo_box<R>(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    selected_text: impl Into<egui::WidgetText>,
    width: f32,
    menu_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    let selected_text: egui::WidgetText = selected_text.into();
    ui.scope(|ui| {
        let spacing = ui.spacing_mut();
        spacing.interact_size.y = 28.0;
        spacing.button_padding = egui::vec2(10.0, 4.0);
        let out = egui::ComboBox::from_id_salt(id_salt)
            .selected_text(selected_text)
            .width(width)
            .show_ui(ui, menu_contents);
        let resp = out.response.on_hover_cursor(egui::CursorIcon::PointingHand);
        egui::InnerResponse::new(out.inner, resp)
    })
    .inner
}

fn studio_row_label_galley(
    ui: &egui::Ui,
    text: impl Into<egui::WidgetText>,
) -> std::sync::Arc<egui::Galley> {
    let text: egui::WidgetText = text.into();
    text.into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    )
}

/// Measures the horizontal width of a [`studio_row_label`].
pub(crate) fn studio_row_label_width(ui: &egui::Ui, text: impl Into<egui::WidgetText>) -> f32 {
    studio_row_label_galley(ui, text).size().x
}

/// Measures the minimum/rendered width of a [`studio_combo_box`].
pub(crate) fn studio_combo_box_width(ui: &egui::Ui, selected_text: &str, min_width: f32) -> f32 {
    let galley = ui.painter().layout_no_wrap(
        selected_text.to_string(),
        egui::TextStyle::Button.resolve(ui.style()),
        Color32::WHITE,
    );
    let chrome = 20.0 + ui.spacing().icon_width + ui.spacing().icon_spacing + 4.0;
    min_width.max(galley.size().x + chrome)
}

/// Renders a single-line label vertically centered inside `row_height` so it aligns
/// with adjacent fixed-height controls (`studio_combo_box`, `DragValue`, buttons).
pub(crate) fn studio_row_label(
    ui: &mut egui::Ui,
    text: impl Into<egui::WidgetText>,
    row_height: f32,
) -> egui::Response {
    let galley = studio_row_label_galley(ui, text);
    let desired_size = egui::vec2(galley.size().x, row_height.max(galley.size().y));
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let pos = egui::pos2(rect.min.x, rect.center().y - galley.size().y * 0.5);
        ui.painter().galley(pos, galley, ui.visuals().text_color());
    }
    response
}

/// Runs `add_contents` inside a wrapping horizontal row with vertical center
/// alignment and a minimum row height so leading labels never start shifted high.
pub(crate) fn studio_wrapped_row<R>(
    ui: &mut egui::Ui,
    spacing: egui::Vec2,
    row_height: f32,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let initial_size = egui::vec2(
        ui.available_size_before_wrap().x,
        ui.spacing().interact_size.y.max(row_height),
    );
    ui.allocate_ui_with_layout(
        initial_size,
        egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
        |ui| {
            ui.spacing_mut().item_spacing = spacing;
            add_contents(ui)
        },
    )
}

/// Applies the Light studio palette cleanly (used by theme-neutral unit tests).
#[cfg(test)]
pub fn apply_light_visuals(ctx: &egui::Context) {
    apply_visuals(ctx, UiTheme::Light);
}

#[cfg(test)]
mod tests {
    use super::{Tokens, apply_visuals};
    use crate::{AppScreen, DrawCraftApp, SettingsTab, modal, motion, open_notebook_in_editor};
    use drawcraft_doc::UiTheme;
    use egui::{Color32, Pos2, RawInput, Rect, Shape, Vec2};

    fn channel_sum(c: Color32) -> u16 {
        u16::from(c.r()) + u16::from(c.g()) + u16::from(c.b())
    }

    fn collect_text_shapes(shapes: &[egui::epaint::ClippedShape]) -> String {
        let mut out = String::new();
        fn visit(shape: &Shape, buf: &mut String) {
            match shape {
                Shape::Text(text) => {
                    buf.push_str(text.galley.text());
                    buf.push('\n');
                }
                Shape::Vec(items) => {
                    for s in items {
                        visit(s, buf);
                    }
                }
                _ => {}
            }
        }
        for clipped in shapes {
            visit(&clipped.shape, &mut out);
        }
        out
    }

    fn run_app_frame(ctx: &egui::Context, app: &mut DrawCraftApp) -> String {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))),
            ..Default::default()
        };
        let out = ctx.run_ui(raw, |ui| app.show_ui(ui));
        let texts = collect_text_shapes(&out.shapes);
        assert!(!out.shapes.is_empty(), "rendered frame must emit shapes");
        out.drop_without_applying_deltas();
        texts
    }

    #[test]
    fn tokens_for_theme_distinguish_light_and_dark_palettes() {
        let light = Tokens::for_theme(UiTheme::Light);
        let dark = Tokens::for_theme(UiTheme::Dark);

        assert!(!light.is_dark);
        assert!(dark.is_dark);
        assert_ne!(light.canvas_bg, dark.canvas_bg);
        assert_ne!(light.panel_bg, dark.panel_bg);
        assert_ne!(light.toolbar_bg, dark.toolbar_bg);
        assert_ne!(light.text, dark.text);
        assert_ne!(light.danger_fg, dark.danger_fg);
        assert_ne!(light.success_fg, dark.success_fg);
        assert_ne!(light.warning_fg, dark.warning_fg);

        assert!(channel_sum(light.panel_bg) > channel_sum(light.text));
        assert!(channel_sum(light.canvas_bg) > channel_sum(light.text));
        assert!(channel_sum(dark.panel_bg) < channel_sum(dark.text));
        assert!(channel_sum(dark.canvas_bg) < channel_sum(dark.text));
    }

    #[test]
    fn theme_switching_updates_egui_visuals_and_renders_home_editor_and_modal() {
        let ctx = egui::Context::default();
        let mut app = DrawCraftApp::default();
        assert_eq!(app.session.prefs.theme, UiTheme::Light);

        // The default studio is Light; this test starts from Dark.
        assert!(
            app.session
                .execute("theme.set", &serde_json::json!({ "theme": "Dark" }))
                .is_ok()
        );
        let home_dark_init = run_app_frame(&ctx, &mut app);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        assert!(ctx.style_of(ctx.theme()).visuals.dark_mode);
        assert!(home_dark_init.contains("Library"));
        assert!(home_dark_init.contains("New"));

        // Switch to Light via engine command and verify Home updates.
        assert!(
            app.session
                .execute("theme.set", &serde_json::json!({ "theme": "Light" }))
                .is_ok()
        );
        assert_eq!(app.session.prefs.theme, UiTheme::Light);
        let home_light = run_app_frame(&ctx, &mut app);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert!(!ctx.style_of(ctx.theme()).visuals.dark_mode);
        assert!(home_light.contains("Library"));
        assert!(home_light.contains("New"));

        // Enter Editor on Appearance tab and switch to Dark mode.
        assert!(
            app.session
                .execute("theme.set", &serde_json::json!({ "theme": "Dark" }))
                .is_ok()
        );
        open_notebook_in_editor(&mut app, 0);
        app.view.motion.screen = motion::ScreenTransition::new(AppScreen::Editor);
        app.view.inspector_open = true;
        app.view.settings_tab = SettingsTab::Appearance;
        let _ = run_app_frame(&ctx, &mut app);
        let editor_dark = run_app_frame(&ctx, &mut app);
        assert!(editor_dark.contains("Appearance"));
        assert!(editor_dark.contains("Dark"));
        assert!(editor_dark.contains("Light"));

        // Open Export modal in Dark mode, then toggle back to Light and re-render cleanly.
        modal::open(&mut app, modal::ActiveModal::Export);
        let _ = run_app_frame(&ctx, &mut app);
        let modal_dark = run_app_frame(&ctx, &mut app);
        assert!(modal_dark.contains("Export Notebook"));

        apply_visuals(&ctx, UiTheme::Light);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert!(
            app.session
                .execute("theme.set", &serde_json::json!({ "theme": "Light" }))
                .is_ok()
        );
        let modal_light = run_app_frame(&ctx, &mut app);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert!(!ctx.style_of(ctx.theme()).visuals.dark_mode);
        assert!(modal_light.contains("Export Notebook"));
    }
}
