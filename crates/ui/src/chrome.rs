//! Top navigation bar of the editor: the first of the two chrome levels.
//!
//! Level 1 (this module, 48 px): library, pages, search | open notebook tabs | save state, share,
//! flashcards, focus mode, overflow menu. Level 2 (`toolbar.rs`): the tool strip with colors,
//! stroke sizes and history. Calm, flat, hairline-separated; every control is at least 44 px on
//! touch devices. All actions are engine commands or view toggles.

use egui::{
    Align2, Color32, CornerRadius, FontId, Frame, Margin, Rect, Response, RichText, Sense, Stroke,
    Vec2, pos2, vec2,
};
use serde_json::json;

use crate::DrawCraftApp;
use crate::icons::{Icon, compact_icon_button, min_target};
use crate::theme::{CONTROL_RADIUS, Tokens};

/// Height of the navigation bar.
pub const NAV_BAR_HEIGHT: f32 = 48.0;
const TAB_MIN_WIDTH: f32 = 96.0;
const TAB_MAX_WIDTH: f32 = 220.0;
const MAX_TITLE_CHARS: usize = 28;

/// Shortens a notebook title for a tab without cutting through a multi-byte character.
#[must_use]
pub fn tab_label(title: &str) -> String {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return "Untitled".to_string();
    }
    if trimmed.chars().count() <= MAX_TITLE_CHARS {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(MAX_TITLE_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Entries of the overflow (`...`) menu, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverflowItem {
    AddPage,
    BookmarkPage,
    DuplicatePage,
    RotatePage,
    DeletePage,
    ClearPage,
    PageFormat,
    PdfDarkMode,
    LeftHanded,
    FingerDrawing,
    Theme,
    ProfileSync,
    Settings,
}

/// Human label for an overflow entry given the current preferences.
#[must_use]
pub fn overflow_label(item: OverflowItem, app: &DrawCraftApp) -> String {
    let p = &app.session.prefs;
    let on = |v: bool| if v { "on" } else { "off" };
    match item {
        OverflowItem::AddPage => "Add page".to_string(),
        OverflowItem::BookmarkPage => "Bookmark page".to_string(),
        OverflowItem::DuplicatePage => "Duplicate page".to_string(),
        OverflowItem::RotatePage => "Rotate page".to_string(),
        OverflowItem::DeletePage => "Delete page".to_string(),
        OverflowItem::ClearPage => "Clear page".to_string(),
        OverflowItem::PageFormat => "Page format".to_string(),
        OverflowItem::PdfDarkMode => format!("Dark PDF pages ({})", on(p.pdf_dark_mode)),
        OverflowItem::LeftHanded => format!("Left-handed layout ({})", on(p.left_handed)),
        OverflowItem::FingerDrawing => format!("Draw with finger ({})", on(p.finger_drawing)),
        OverflowItem::Theme => match p.theme {
            drawcraft_doc::UiTheme::Dark => "Switch to light studio".to_string(),
            drawcraft_doc::UiTheme::Light => "Switch to dark studio".to_string(),
        },
        OverflowItem::ProfileSync => "Profile & Sync".to_string(),
        OverflowItem::Settings => "Settings".to_string(),
    }
}

const OVERFLOW_ITEMS: [OverflowItem; 13] = [
    OverflowItem::AddPage,
    OverflowItem::BookmarkPage,
    OverflowItem::DuplicatePage,
    OverflowItem::RotatePage,
    OverflowItem::DeletePage,
    OverflowItem::ClearPage,
    OverflowItem::PageFormat,
    OverflowItem::PdfDarkMode,
    OverflowItem::LeftHanded,
    OverflowItem::FingerDrawing,
    OverflowItem::Theme,
    OverflowItem::ProfileSync,
    OverflowItem::Settings,
];

fn run(app: &mut DrawCraftApp, id: &str, params: serde_json::Value) {
    let _ = app.session.execute(id, &params);
}

/// Executes an overflow entry through the engine (or toggles the matching drawer).
pub(crate) fn run_overflow(app: &mut DrawCraftApp, item: OverflowItem) {
    match item {
        OverflowItem::AddPage => {
            crate::canvas::commit_pending_text(app);
            run(app, "page.add", json!({}));
        }
        OverflowItem::BookmarkPage => run(app, "page.setBookmark", json!({})),
        OverflowItem::DuplicatePage => {
            crate::canvas::commit_pending_text(app);
            run(app, "page.duplicate", json!({}));
        }
        OverflowItem::RotatePage => {
            crate::canvas::commit_pending_text(app);
            run(app, "page.rotate", json!({ "quarter_turns": 1 }));
        }
        OverflowItem::DeletePage => {
            crate::canvas::commit_pending_text(app);
            run(app, "page.remove", json!({}));
        }
        OverflowItem::ClearPage => {
            crate::canvas::commit_pending_text(app);
            run(app, "page.clear", json!({}));
        }
        OverflowItem::PageFormat => {
            crate::modal::open(app, crate::modal::ActiveModal::NotebookSettings);
        }
        OverflowItem::PdfDarkMode => {
            let v = !app.session.prefs.pdf_dark_mode;
            run(app, "prefs.setStudy", json!({ "pdf_dark_mode": v }));
        }
        OverflowItem::LeftHanded => {
            let v = !app.session.prefs.left_handed;
            run(app, "prefs.setStudy", json!({ "left_handed": v }));
        }
        OverflowItem::FingerDrawing => {
            let v = !app.session.prefs.finger_drawing;
            run(app, "prefs.setStudy", json!({ "finger_drawing": v }));
        }
        OverflowItem::Theme => {
            let next = match app.session.prefs.theme {
                drawcraft_doc::UiTheme::Dark => drawcraft_doc::UiTheme::Light,
                drawcraft_doc::UiTheme::Light => drawcraft_doc::UiTheme::Dark,
            };
            run(app, "theme.set", json!({ "theme": next }));
        }
        OverflowItem::ProfileSync => crate::open_profile_screen(app),
        OverflowItem::Settings => app.view.inspector_open = !app.view.inspector_open,
    }
}

fn nav_button(ui: &mut egui::Ui, icon: Icon, active: bool, tip: &str, t: &Tokens) -> Response {
    compact_icon_button(ui, icon, None, active, true, t).on_hover_text(tip)
}

/// One notebook tab. Returns `(selected, close_requested)`.
fn tab(ui: &mut egui::Ui, title: &str, active: bool, t: &Tokens) -> (bool, bool) {
    let label = tab_label(title);
    let font = FontId::new(
        13.5,
        egui::FontFamily::Name(crate::theme::MEDIUM_FAMILY.into()),
    );
    let galley = ui.painter().layout_no_wrap(label, font, t.text);
    let close_w = if active { 28.0 } else { 0.0 };
    let width = (galley.size().x + 28.0 + close_w).clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH);
    let height = min_target(ui.ctx(), 34.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let mut close = false;
    if ui.is_rect_visible(rect) {
        let hover_t = crate::motion::hover_factor(ui.ctx(), resp.id.with("hov"), resp.hovered());
        let fill = if active {
            t.toolbar_bg
        } else {
            crate::motion::lerp_color(Color32::TRANSPARENT, t.ghost_hover, hover_t)
        };
        let radius = CornerRadius::same(CONTROL_RADIUS);
        if fill.a() > 0 {
            ui.painter().rect_filled(rect, radius, fill);
        }
        if active {
            // Quiet accent underline marks the open notebook instead of a boxed outline.
            let underline = Rect::from_min_max(
                pos2(rect.left() + 12.0, rect.bottom() - 3.0),
                pos2(rect.right() - 12.0, rect.bottom() - 1.0),
            );
            ui.painter()
                .rect_filled(underline, CornerRadius::same(1), t.accent);
        }
        let color = if active { t.text } else { t.text_muted };
        ui.painter().galley(
            pos2(rect.left() + 14.0, rect.center().y - galley.size().y * 0.5),
            galley,
            color,
        );
        if active {
            let close_rect = Rect::from_center_size(
                pos2(rect.right() - 16.0, rect.center().y),
                Vec2::splat(22.0),
            );
            let close_resp = ui.interact(close_rect, resp.id.with("close"), Sense::click());
            if close_resp.hovered() {
                ui.painter()
                    .rect_filled(close_rect, CornerRadius::same(6), t.ghost_hover);
            }
            crate::icons::paint_icon(
                ui.painter(),
                close_rect.shrink(5.0),
                Icon::Close,
                t.text_muted,
            );
            close = close_resp.on_hover_text("Close notebook").clicked();
        }
    }
    (resp.clicked() && !close, close)
}

/// Draws the navigation bar and returns its rect.
pub(crate) fn show(app: &mut DrawCraftApp, ui: &mut egui::Ui) -> Rect {
    let t = Tokens::for_theme(app.session.prefs.theme);
    let frame = Frame::new()
        .fill(t.panel_bg)
        .inner_margin(Margin::symmetric(12, 0));
    let height = min_target(ui.ctx(), NAV_BAR_HEIGHT).max(NAV_BAR_HEIGHT);
    let resp = egui::Panel::top("drawcraft_nav_bar")
        .frame(frame)
        .exact_size(height)
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                draw_left(app, ui, &t);
                draw_tabs(app, ui, &t);
                draw_right(app, ui, &t);
            });
        })
        .response;
    // Calm bottom hairline separating the recessed tab strip from the pure-white tool strip.
    ui.painter().line_segment(
        [resp.rect.left_bottom(), resp.rect.right_bottom()],
        Stroke::new(1.0, t.border),
    );
    resp.rect
}

fn draw_left(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    if compact_icon_button(ui, Icon::Library, Some("Library"), false, true, t)
        .on_hover_text("Library (H)")
        .clicked()
    {
        crate::return_to_home(app);
    }
    let pages = app.view.pages_sidebar_open;
    if nav_button(ui, Icon::GridView, pages, "Pages (P)", t).clicked() {
        app.view.pages_sidebar_open = !pages;
    }
    if nav_button(ui, Icon::Search, false, "Find in notebook (Cmd/Ctrl+F)", t).clicked() {
        crate::study_ui::execute_shortcut(app, crate::study_ui::StudyShortcut::FindInNotebook);
    }
}

fn draw_tabs(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.add_space(8.0);
    let active = app.session.active_notebook;
    let titles: Vec<String> = app
        .session
        .notebooks
        .iter()
        .enumerate()
        .map(|(i, nb)| {
            if i == active {
                app.session.doc.title.clone()
            } else {
                nb.title.clone()
            }
        })
        .collect();
    let mut select: Option<usize> = None;
    let mut close_active = false;
    egui::ScrollArea::horizontal()
        .id_salt("drawcraft_nav_tabs")
        .auto_shrink([true, false])
        .max_width((ui.available_width() - 330.0).max(120.0))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (i, title) in titles.iter().enumerate() {
                    let (clicked, close) = tab(ui, title, i == active, t);
                    if clicked && i != active {
                        select = Some(i);
                    }
                    close_active |= close;
                }
            });
        });
    if let Some(i) = select {
        crate::canvas::commit_pending_text(app);
        run(app, "notebook.select", json!({ "index": i }));
    }
    if nav_button(ui, Icon::Plus, false, "New notebook", t).clicked() {
        crate::modal::open(app, crate::modal::ActiveModal::NewNotebook);
    }
    if close_active {
        crate::return_to_home(app);
    }
}

fn draw_right(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        draw_overflow(app, ui, t);
        let focus = app.view.study.focus_mode;
        if nav_button(
            ui,
            Icon::FitPage,
            focus,
            "Focus mode: hide everything but the page",
            t,
        )
        .clicked()
        {
            app.view.study.focus_mode = !focus;
        }
        let cards = app.view.study.flashcards_open;
        if nav_button(ui, Icon::Flashcards, cards, "Flashcards", t).clicked() {
            app.view.study.flashcards_open = !cards;
        }
        let export_open = app.view.active_modal == Some(crate::modal::ActiveModal::Export);
        if nav_button(ui, Icon::Export, export_open, "Share and export", t).clicked() {
            if export_open {
                crate::modal::close(app);
            } else {
                crate::modal::open(app, crate::modal::ActiveModal::Export);
            }
        }
        crate::study_ui::draw_save_status(app, ui, t);
    });
}

fn draw_overflow(app: &mut DrawCraftApp, ui: &mut egui::Ui, t: &Tokens) {
    let popup_id = egui::Id::unique("drawcraft_overflow_menu");
    let open = app.view.study.overflow_open;
    let resp = nav_button(ui, Icon::More, open, "More", t);
    if resp.clicked() {
        app.view.study.overflow_open = !open;
    }
    if !app.view.study.overflow_open {
        return;
    }
    let ctx = ui.ctx().clone();
    let mut chosen: Option<OverflowItem> = None;
    let area = egui::Area::new(popup_id)
        .order(egui::Order::Foreground)
        .anchor(Align2::RIGHT_TOP, vec2(-10.0, NAV_BAR_HEIGHT + 6.0))
        .show(&ctx, |ui| {
            Frame::popup(ui.style())
                .fill(t.panel_bg)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(crate::theme::CARD_RADIUS))
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(260.0);
                    let max_h = (ctx.content_rect().height() - NAV_BAR_HEIGHT - 40.0).max(160.0);
                    egui::ScrollArea::vertical()
                        .max_height(max_h)
                        .show(ui, |ui| {
                            for item in OVERFLOW_ITEMS {
                                if matches!(item, OverflowItem::PdfDarkMode) {
                                    let (sep, _) = ui.allocate_exact_size(
                                        vec2(ui.available_width(), 9.0),
                                        Sense::hover(),
                                    );
                                    ui.painter().hline(
                                        sep.x_range(),
                                        sep.center().y,
                                        Stroke::new(1.0, t.border),
                                    );
                                }
                                let label = overflow_label(item, app);
                                let (rect, r) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), min_target(ui.ctx(), 44.0)),
                                    Sense::click(),
                                );
                                if r.hovered() {
                                    ui.painter().rect_filled(
                                        rect,
                                        CornerRadius::same(CONTROL_RADIUS),
                                        t.ghost_hover,
                                    );
                                }
                                ui.painter().text(
                                    rect.left_center() + vec2(12.0, 0.0),
                                    Align2::LEFT_CENTER,
                                    label,
                                    FontId::proportional(14.0),
                                    t.text,
                                );
                                if r.clicked() {
                                    chosen = Some(item);
                                }
                            }
                        });
                });
        });
    let clicked_outside = ctx.input(|i| i.pointer.any_click())
        && !area.response.hovered()
        && !resp.hovered()
        && chosen.is_none();
    if let Some(item) = chosen {
        app.view.study.overflow_open = false;
        run_overflow(app, item);
    } else if clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.view.study.overflow_open = false;
    }
    let _ = RichText::new("");
}

#[cfg(test)]
mod tests {
    #[test]
    fn page_actions_in_the_overflow_menu_duplicate_rotate_bookmark_and_delete() {
        let mut app = DrawCraftApp::default();
        let before = app.session.doc.page_count();
        run_overflow(&mut app, OverflowItem::DuplicatePage);
        assert_eq!(app.session.doc.page_count(), before + 1);
        let (w, h) = (app.session.doc.width, app.session.doc.height);
        run_overflow(&mut app, OverflowItem::RotatePage);
        assert_eq!((app.session.doc.width, app.session.doc.height), (h, w));
        run_overflow(&mut app, OverflowItem::BookmarkPage);
        run_overflow(&mut app, OverflowItem::DeletePage);
        assert_eq!(app.session.doc.page_count(), before);
        // The last remaining page can never be deleted.
        for _ in 0..3 {
            run_overflow(&mut app, OverflowItem::DeletePage);
        }
        assert_eq!(app.session.doc.page_count(), 1);
    }
    use super::*;

    #[test]
    fn tab_labels_are_trimmed_and_never_split_characters() {
        assert_eq!(tab_label("  Biologie  "), "Biologie");
        assert_eq!(tab_label("   "), "Untitled");
        let long = "Überlänge ".repeat(10);
        let label = tab_label(&long);
        assert!(label.chars().count() <= MAX_TITLE_CHARS);
        assert!(label.ends_with('…'));
        assert!(label.starts_with("Überlänge"));
    }

    #[test]
    fn overflow_entries_run_engine_commands_and_report_state() {
        let mut app = DrawCraftApp::default();
        assert_eq!(
            overflow_label(OverflowItem::PdfDarkMode, &app),
            "Dark PDF pages (off)"
        );
        run_overflow(&mut app, OverflowItem::PdfDarkMode);
        run_overflow(&mut app, OverflowItem::LeftHanded);
        run_overflow(&mut app, OverflowItem::FingerDrawing);
        assert!(app.session.prefs.pdf_dark_mode && app.session.prefs.left_handed);
        assert!(app.session.prefs.finger_drawing);
        assert_eq!(
            overflow_label(OverflowItem::PdfDarkMode, &app),
            "Dark PDF pages (on)"
        );

        assert_eq!(
            overflow_label(OverflowItem::Theme, &app),
            "Switch to dark studio"
        );
        run_overflow(&mut app, OverflowItem::Theme);
        assert_eq!(app.session.prefs.theme, drawcraft_doc::UiTheme::Dark);
        assert_eq!(
            overflow_label(OverflowItem::Theme, &app),
            "Switch to light studio"
        );

        run_overflow(&mut app, OverflowItem::Settings);
        assert!(app.view.inspector_open);
    }

    fn nav_bar_texts(app: &mut DrawCraftApp, width: f32) -> (String, Rect) {
        let ctx = egui::Context::default();
        let mut rect = Rect::NOTHING;
        let mut texts = String::new();
        for _ in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 800.0))),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| rect = show(app, ui));
            texts.clear();
            for clipped in &out.shapes {
                fn visit(shape: &egui::Shape, buf: &mut String) {
                    match shape {
                        egui::Shape::Text(t) => {
                            buf.push_str(t.galley.text());
                            buf.push('\n');
                        }
                        egui::Shape::Vec(items) => items.iter().for_each(|s| visit(s, buf)),
                        _ => {}
                    }
                }
                visit(&clipped.shape, &mut texts);
            }
            out.drop_without_applying_deltas();
        }
        (texts, rect)
    }

    #[test]
    fn navigation_bar_shows_library_and_one_tab_per_open_notebook() {
        let mut app = DrawCraftApp::default();
        crate::open_notebook_in_editor(&mut app, 0);
        let _ = app
            .session
            .execute("notebook.rename", &json!({ "title": "Zellbiologie" }));
        let (texts, rect) = nav_bar_texts(&mut app, 1280.0);
        assert!(texts.contains("Library"), "{texts}");
        assert!(
            texts.contains("Zellbiologie"),
            "active tab shows the title: {texts}"
        );
        assert!(rect.height() >= NAV_BAR_HEIGHT - 1.0 && rect.height() <= NAV_BAR_HEIGHT + 2.0);
    }

    #[test]
    fn navigation_bar_stays_one_row_on_a_narrow_ipad_width() {
        let mut app = DrawCraftApp::default();
        crate::open_notebook_in_editor(&mut app, 0);
        let (_, rect) = nav_bar_texts(&mut app, 744.0);
        assert!(
            rect.height() <= NAV_BAR_HEIGHT + 2.0,
            "height {}",
            rect.height()
        );
    }

    #[test]
    fn touch_mode_grows_the_navigation_bar_to_the_44_px_minimum() {
        let mut app = DrawCraftApp::default();
        crate::open_notebook_in_editor(&mut app, 0);
        app.set_touch_ui(true);
        let ctx = egui::Context::default();
        crate::icons::set_touch_ui(&ctx, true);
        let mut rect = Rect::NOTHING;
        for _ in 0..2 {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 700.0))),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| rect = show(&mut app, ui));
            out.drop_without_applying_deltas();
        }
        assert!(rect.height() >= 44.0, "height {}", rect.height());
    }
}
