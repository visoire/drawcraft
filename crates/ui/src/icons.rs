//! Semantic icon registry backed by the vendored Lucide outline SVG assets.

use egui::{
    Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Vec2, pos2,
};

use crate::theme::Tokens;

/// Semantic icon names used throughout the interface.
///
/// Each variant is mapped to a local Lucide outline SVG in [`icon_svg`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    FountainPen,
    BallPen,
    BrushPen,
    Highlighter,
    HighlighterFreehand,
    HighlighterStraight,
    Eraser,
    StrokeEraser,
    PrecisionEraser,
    Undo,
    Redo,
    ClearPage,
    FitPage,
    ZoomIn,
    ZoomOut,
    Settings,
    Preset,
    Stabilizer,
    Pressure,
    Paper,
    Telemetry,
    Reset,
    Info,
    Notebook,
    Library,
    Search,
    Plus,
    Export,
    AddPage,
    Duplicate,
    PrevPage,
    NextPage,
    Close,
    Palette,
    Edit,
    Trash,
    Check,
    Sun,
    Moon,
    Account,
    Sync,
    CheckCircle,
    AlertCircle,
    Server,
    Eye,
    EyeOff,
    ShapeTool,
    LineShape,
    RectangleShape,
    EllipseShape,
    TriangleShape,
    ArrowShape,
    Ruler,
    Select,
    Image,
    Link,
    Folder,
    FolderOpen,
    FolderPlus,
    FolderPen,
    MoveToFolder,
    Inbox,
    ChevronUp,
    ChevronDown,
    More,
    Star,
    Tag,
    ListView,
    GridView,
    Sort,
    MoveUp,
    MoveDown,
    Bookmark,
    RotateCw,
    CheckSquare,
    Transfer,
    Pencil,
    Lasso,
    PolygonShape,
    FillBucket,
    Paste,
    Bold,
    Italic,
    Underline,
    AlignLeft,
    AlignCenter,
    AlignRight,
    CheckList,
    Sticker,
    LaserPointer,
    GripHorizontal,
    Text,
    StudyTape,
    Flashcards,
    SidebarToggle,
    Camera,
    Scan,
    Users,
    Marketplace,
    QuickNote,
    TextDoc,
    Whiteboard,
    Cloud,
    CloudOff,
    CloudCheck,
    ListFilter,
    FolderApple,
    FolderAtom,
    FolderBriefcase,
    FolderCalculator,
    FolderChart,
    FolderCode,
    FolderGear,
    FolderLandmark,
    FolderMoney,
    FolderFlask,
    FolderGradCap,
    FolderScale,
    FolderMusic,
    FolderMoon,
    FolderHeart,
    FolderCompass,
    FolderGlobe,
    Ban,
}

/// Ordered folder emblem icons (`(storage_key, label, icon)`) matching the 7×3 picker layout.
pub(crate) const FOLDER_ICON_CHOICES: &[(&str, &str, Icon)] = &[
    ("apple", "Apple", Icon::FolderApple),
    ("atom", "Science", Icon::FolderAtom),
    ("book", "Reading", Icon::Notebook),
    ("briefcase", "Work", Icon::FolderBriefcase),
    ("calculator", "Math", Icon::FolderCalculator),
    ("chart", "Analytics", Icon::FolderChart),
    ("code", "Code", Icon::FolderCode),
    ("gear", "Engineering", Icon::FolderGear),
    ("landmark", "History", Icon::FolderLandmark),
    ("money", "Finance", Icon::FolderMoney),
    ("flask", "Chemistry", Icon::FolderFlask),
    ("graduation", "Study", Icon::FolderGradCap),
    ("scale", "Law", Icon::FolderScale),
    ("music", "Audio", Icon::FolderMusic),
    ("moon", "Journal", Icon::FolderMoon),
    ("document", "Notes", Icon::TextDoc),
    ("palette", "Art", Icon::Palette),
    ("heart", "Personal", Icon::FolderHeart),
    ("compass", "Design", Icon::FolderCompass),
    ("globe", "Languages", Icon::FolderGlobe),
];

/// Resolves a persisted folder icon key to its [`Icon`].
pub(crate) fn folder_icon_from_key(key: &str) -> Option<Icon> {
    FOLDER_ICON_CHOICES
        .iter()
        .find(|&&(k, _, _)| k.eq_ignore_ascii_case(key))
        .map(|&(_, _, icon)| icon)
}

/// Minimum interactive side on touch screens, in CSS px (DESIGN.md: at least 44).
pub const TOUCH_TARGET: f32 = 44.0;

#[derive(Clone, Copy)]
struct TouchUi(bool);

/// Enables or disables touch-sized controls for this egui context.
pub(crate) fn set_touch_ui(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("drawcraft_touch_ui"), TouchUi(on)));
}

/// `base` on pointer devices, at least [TOUCH_TARGET] on touch screens.
pub(crate) fn min_target(ctx: &egui::Context, base: f32) -> f32 {
    let touch = ctx
        .data(|d| d.get_temp::<TouchUi>(egui::Id::unique("drawcraft_touch_ui")))
        .is_some_and(|t| t.0);
    if touch { base.max(TOUCH_TARGET) } else { base }
}

/// Load the local outline SVG assigned to a semantic icon.
fn icon_svg(icon: Icon) -> &'static str {
    match icon {
        Icon::FountainPen => include_str!("../assets/icons/drawcraft/fountain-pen.svg"),
        Icon::BallPen => include_str!("../assets/icons/drawcraft/ball-pen.svg"),
        Icon::BrushPen => include_str!("../assets/icons/drawcraft/brush.svg"),
        Icon::Highlighter => include_str!("../assets/icons/drawcraft/highlighter.svg"),
        Icon::HighlighterFreehand => include_str!("../assets/icons/lucide/signature.svg"),
        Icon::HighlighterStraight => include_str!("../assets/icons/lucide/ruler.svg"),
        Icon::Eraser => include_str!("../assets/icons/drawcraft/eraser.svg"),
        Icon::StrokeEraser => include_str!("../assets/icons/lucide/scissors.svg"),
        Icon::PrecisionEraser => include_str!("../assets/icons/lucide/crosshair.svg"),
        Icon::Undo => include_str!("../assets/icons/lucide/undo-2.svg"),
        Icon::Redo => include_str!("../assets/icons/lucide/redo-2.svg"),
        Icon::ClearPage => include_str!("../assets/icons/lucide/file-x.svg"),
        Icon::FitPage => include_str!("../assets/icons/lucide/maximize.svg"),
        Icon::ZoomIn => include_str!("../assets/icons/lucide/zoom-in.svg"),
        Icon::ZoomOut => include_str!("../assets/icons/lucide/zoom-out.svg"),
        Icon::Settings => include_str!("../assets/icons/lucide/settings-2.svg"),
        Icon::Preset => include_str!("../assets/icons/lucide/sparkles.svg"),
        Icon::Stabilizer => include_str!("../assets/icons/lucide/spline.svg"),
        Icon::Pressure => include_str!("../assets/icons/lucide/activity.svg"),
        Icon::Paper => include_str!("../assets/icons/lucide/file.svg"),
        Icon::Telemetry => include_str!("../assets/icons/lucide/gauge.svg"),
        Icon::Reset => include_str!("../assets/icons/lucide/rotate-ccw.svg"),
        Icon::Info => include_str!("../assets/icons/lucide/info.svg"),
        Icon::Notebook => include_str!("../assets/icons/lucide/book-open.svg"),
        Icon::Library => include_str!("../assets/icons/lucide/library.svg"),
        Icon::Search => include_str!("../assets/icons/lucide/search.svg"),
        Icon::Plus => include_str!("../assets/icons/lucide/plus.svg"),
        Icon::Export => include_str!("../assets/icons/lucide/upload.svg"),
        Icon::AddPage => include_str!("../assets/icons/lucide/file-plus.svg"),
        Icon::Duplicate => include_str!("../assets/icons/lucide/copy.svg"),
        Icon::PrevPage => include_str!("../assets/icons/lucide/chevron-left.svg"),
        Icon::NextPage => include_str!("../assets/icons/lucide/chevron-right.svg"),
        Icon::Close => include_str!("../assets/icons/lucide/x.svg"),
        Icon::Palette => include_str!("../assets/icons/lucide/palette.svg"),
        Icon::Edit => include_str!("../assets/icons/lucide/pencil.svg"),
        Icon::Trash => include_str!("../assets/icons/lucide/trash.svg"),
        Icon::Check => include_str!("../assets/icons/lucide/check.svg"),
        Icon::Sun => include_str!("../assets/icons/lucide/sun.svg"),
        Icon::Moon => include_str!("../assets/icons/lucide/moon.svg"),
        Icon::Account => include_str!("../assets/icons/lucide/user-round.svg"),
        Icon::Sync => include_str!("../assets/icons/lucide/cloud-sync.svg"),
        Icon::CheckCircle => include_str!("../assets/icons/lucide/circle-check.svg"),
        Icon::AlertCircle => include_str!("../assets/icons/lucide/circle-exclamation-point.svg"),
        Icon::Server => include_str!("../assets/icons/lucide/server.svg"),
        Icon::Eye => include_str!("../assets/icons/lucide/eye.svg"),
        Icon::EyeOff => include_str!("../assets/icons/lucide/eye-off.svg"),
        Icon::ShapeTool => include_str!("../assets/icons/lucide/shapes.svg"),
        Icon::LineShape => include_str!("../assets/icons/lucide/minus.svg"),
        Icon::RectangleShape => include_str!("../assets/icons/lucide/square.svg"),
        Icon::EllipseShape => include_str!("../assets/icons/lucide/circle.svg"),
        Icon::TriangleShape => include_str!("../assets/icons/lucide/triangle.svg"),
        Icon::ArrowShape => include_str!("../assets/icons/lucide/arrow-up-right.svg"),
        Icon::Ruler => include_str!("../assets/icons/drawcraft/ruler.svg"),
        Icon::Select => include_str!("../assets/icons/lucide/mouse-pointer-2.svg"),
        Icon::Image => include_str!("../assets/icons/lucide/image.svg"),
        Icon::Link => include_str!("../assets/icons/lucide/link.svg"),
        Icon::Folder => include_str!("../assets/icons/lucide/folder.svg"),
        Icon::FolderOpen => include_str!("../assets/icons/lucide/folder-open.svg"),
        Icon::FolderPlus => include_str!("../assets/icons/lucide/folder-plus.svg"),
        Icon::FolderPen => include_str!("../assets/icons/lucide/folder-pen.svg"),
        Icon::MoveToFolder => include_str!("../assets/icons/lucide/folder-input.svg"),
        Icon::Inbox => include_str!("../assets/icons/lucide/inbox.svg"),
        Icon::ChevronUp => include_str!("../assets/icons/lucide/chevron-up.svg"),
        Icon::ChevronDown => include_str!("../assets/icons/lucide/chevron-down.svg"),
        Icon::More => include_str!("../assets/icons/lucide/ellipsis.svg"),
        Icon::Star => include_str!("../assets/icons/lucide/star.svg"),
        Icon::Tag => include_str!("../assets/icons/lucide/tag.svg"),
        Icon::ListView => include_str!("../assets/icons/lucide/list.svg"),
        Icon::GridView => include_str!("../assets/icons/lucide/layout-grid.svg"),
        Icon::Sort => include_str!("../assets/icons/lucide/arrow-up-down.svg"),
        Icon::MoveUp => include_str!("../assets/icons/lucide/arrow-up.svg"),
        Icon::MoveDown => include_str!("../assets/icons/lucide/arrow-down.svg"),
        Icon::Bookmark => include_str!("../assets/icons/lucide/bookmark.svg"),
        Icon::RotateCw => include_str!("../assets/icons/lucide/rotate-cw.svg"),
        Icon::CheckSquare => include_str!("../assets/icons/lucide/square-check.svg"),
        Icon::Transfer => include_str!("../assets/icons/lucide/arrow-right-left.svg"),
        Icon::Pencil => include_str!("../assets/icons/drawcraft/pencil.svg"),
        Icon::Lasso => include_str!("../assets/icons/drawcraft/lasso.svg"),
        Icon::PolygonShape => include_str!("../assets/icons/lucide/pentagon.svg"),
        Icon::FillBucket => include_str!("../assets/icons/lucide/paint-bucket.svg"),
        Icon::Paste => include_str!("../assets/icons/lucide/clipboard-paste.svg"),
        Icon::Bold => include_str!("../assets/icons/lucide/letter-b.svg"),
        Icon::Italic => include_str!("../assets/icons/lucide/italic.svg"),
        Icon::Underline => include_str!("../assets/icons/lucide/underline.svg"),
        Icon::AlignLeft => include_str!("../assets/icons/lucide/text-align-start.svg"),
        Icon::AlignCenter => include_str!("../assets/icons/lucide/text-align-center.svg"),
        Icon::AlignRight => include_str!("../assets/icons/lucide/text-align-end.svg"),
        Icon::CheckList => include_str!("../assets/icons/lucide/list-todo.svg"),
        Icon::Sticker => include_str!("../assets/icons/lucide/sticker.svg"),
        Icon::LaserPointer => include_str!("../assets/icons/drawcraft/laser.svg"),
        Icon::GripHorizontal => include_str!("../assets/icons/lucide/grip-horizontal.svg"),
        Icon::Text => include_str!("../assets/icons/lucide/type.svg"),
        Icon::StudyTape => include_str!("../assets/icons/drawcraft/study-tape.svg"),
        Icon::Flashcards => include_str!("../assets/icons/lucide/copy.svg"),
        Icon::SidebarToggle => include_str!("../assets/icons/lucide/panel-left.svg"),
        Icon::Camera => include_str!("../assets/icons/lucide/camera.svg"),
        Icon::Scan => include_str!("../assets/icons/lucide/scan.svg"),
        Icon::Users => include_str!("../assets/icons/lucide/users.svg"),
        Icon::Marketplace => include_str!("../assets/icons/lucide/store.svg"),
        Icon::QuickNote => include_str!("../assets/icons/lucide/sticky-note.svg"),
        Icon::TextDoc => include_str!("../assets/icons/lucide/file-text.svg"),
        Icon::Whiteboard => include_str!("../assets/icons/lucide/presentation.svg"),
        Icon::Cloud => include_str!("../assets/icons/lucide/cloud.svg"),
        Icon::CloudOff => include_str!("../assets/icons/lucide/cloud-off.svg"),
        Icon::CloudCheck => include_str!("../assets/icons/lucide/cloud-check.svg"),
        Icon::ListFilter => include_str!("../assets/icons/lucide/list-filter.svg"),
        Icon::FolderApple => include_str!("../assets/icons/lucide/apple.svg"),
        Icon::FolderAtom => include_str!("../assets/icons/lucide/atom.svg"),
        Icon::FolderBriefcase => include_str!("../assets/icons/lucide/briefcase.svg"),
        Icon::FolderCalculator => include_str!("../assets/icons/lucide/calculator.svg"),
        Icon::FolderChart => include_str!("../assets/icons/lucide/chart-column.svg"),
        Icon::FolderCode => include_str!("../assets/icons/lucide/code-xml.svg"),
        Icon::FolderGear => include_str!("../assets/icons/lucide/settings.svg"),
        Icon::FolderLandmark => include_str!("../assets/icons/lucide/landmark.svg"),
        Icon::FolderMoney => include_str!("../assets/icons/lucide/banknote.svg"),
        Icon::FolderFlask => include_str!("../assets/icons/lucide/flask-conical.svg"),
        Icon::FolderGradCap => include_str!("../assets/icons/lucide/graduation-cap.svg"),
        Icon::FolderScale => include_str!("../assets/icons/lucide/scale.svg"),
        Icon::FolderMusic => include_str!("../assets/icons/lucide/headphones.svg"),
        Icon::FolderMoon => include_str!("../assets/icons/lucide/moon-star.svg"),
        Icon::FolderHeart => include_str!("../assets/icons/lucide/heart.svg"),
        Icon::FolderCompass => include_str!("../assets/icons/lucide/drafting-compass.svg"),
        Icon::FolderGlobe => include_str!("../assets/icons/lucide/globe.svg"),
        Icon::Ban => include_str!("../assets/icons/lucide/ban.svg"),
    }
}

#[derive(Clone)]
enum CachedIconTexture {
    Ready(egui::TextureHandle),
    Failed,
}

fn icon_texture(ctx: &egui::Context, icon: Icon) -> Option<egui::TextureHandle> {
    let cache_id = egui::Id::unique(("drawcraft-lucide", icon));
    if let Some(cached) = ctx.data_mut(|data| data.get_temp::<CachedIconTexture>(cache_id)) {
        return match cached {
            CachedIconTexture::Ready(texture) => Some(texture),
            CachedIconTexture::Failed => None,
        };
    }

    // Lucide's currentColor is replaced with white so Painter's tint can apply
    // the active light/dark theme color without duplicating SVGs per theme.
    let svg = icon_svg(icon).replace("currentColor", "#ffffff");
    let image = match egui_extras::image::load_svg_bytes_with_size(
        svg.as_bytes(),
        egui::SizeHint::Width(72),
        &Default::default(),
    ) {
        Ok(image) => image,
        Err(error) => {
            log::warn!("Could not render embedded Lucide SVG {icon:?}: {error}");
            ctx.data_mut(|data| data.insert_temp(cache_id, CachedIconTexture::Failed));
            return None;
        }
    };

    let texture = ctx.load_texture(
        format!("drawcraft-lucide-{icon:?}"),
        image,
        egui::TextureOptions::LINEAR,
    );
    ctx.data_mut(|data| {
        data.insert_temp(cache_id, CachedIconTexture::Ready(texture.clone()));
    });
    Some(texture)
}

/// Paints a theme-tinted Lucide SVG centered inside `rect`.
pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    if let Some(texture) = icon_texture(painter.ctx(), icon) {
        painter.image(
            texture.id(),
            rect,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            color,
        );
    }
}

struct ButtonStyle {
    max_width: Option<f32>,
    icon_dim: f32,
    font_size: f32,
    pad_left: f32,
    gap: f32,
    pad_right: f32,
}

impl ButtonStyle {
    const fn standard(max_width: Option<f32>) -> Self {
        Self {
            max_width,
            icon_dim: 15.0,
            font_size: 12.5,
            pad_left: 9.0,
            gap: 6.0,
            pad_right: 10.0,
        }
    }

    const fn compact() -> Self {
        Self {
            max_width: None,
            icon_dim: 14.0,
            font_size: 11.8,
            pad_left: 7.0,
            gap: 4.5,
            pad_right: 8.0,
        }
    }
}

fn layout_button_label_and_width(
    ui: &egui::Ui,
    label: Option<&str>,
    style: &ButtonStyle,
    text_color: Color32,
) -> (Option<std::sync::Arc<egui::Galley>>, f32) {
    let icon_dim = style.icon_dim;
    let chrome_w = style.pad_left + icon_dim + style.gap + style.pad_right;
    let max_width = style
        .max_width
        .filter(|w| w.is_finite())
        .map(|w| w.max(28.0));
    let label = label.filter(|txt| !txt.is_empty());
    let galley = label.map(|txt| {
        let font = FontId::proportional(style.font_size);
        match max_width {
            Some(limit_w) => {
                let max_text_w = (limit_w - chrome_w).max(12.0);
                let mut job = egui::text::LayoutJob::single_section(
                    txt.to_string(),
                    egui::TextFormat {
                        font_id: font,
                        color: text_color,
                        ..Default::default()
                    },
                );
                job.wrap.max_width = max_text_w;
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                job.wrap.overflow_character = Some('…');
                ui.painter().layout_job(job)
            }
            None => ui
                .painter()
                .layout_no_wrap(txt.to_string(), font, text_color),
        }
    });

    let width = match (max_width, &galley) {
        (Some(limit_w), Some(galley)) => (chrome_w + galley.size().x).min(limit_w).max(28.0),
        (Some(limit_w), None) => 28.0_f32.min(limit_w),
        (None, Some(galley)) => (chrome_w + galley.size().x).max(28.0),
        (None, None) => 28.0,
    };

    (galley, width)
}

/// Measures the rendered width of a [`compact_icon_button`] with `label`.
pub fn compact_icon_button_width(ui: &egui::Ui, label: &str) -> f32 {
    layout_button_label_and_width(ui, Some(label), &ButtonStyle::compact(), Color32::BLACK).1
}

/// Measures the rendered width of a [`max_width_icon_button`] with `label` and `max_width`.
pub fn max_width_icon_button_width(ui: &egui::Ui, label: &str, max_width: f32) -> f32 {
    layout_button_label_and_width(
        ui,
        Some(label),
        &ButtonStyle::standard(Some(max_width)),
        Color32::BLACK,
    )
    .1
}

/// Tactile toolbar or segmented-tab button with an aligned icon and optional label.
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: Option<&str>,
    selected: bool,
    enabled: bool,
    t: &Tokens,
) -> Response {
    let mut style = ButtonStyle::standard(None);
    if icon == Icon::Palette {
        style.icon_dim = 20.0;
    }
    draw_styled_icon_button(ui, icon, label, selected, enabled, &style, t)
}

/// Compact tactile toolbar button used for denser segmented controls.
pub fn compact_icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: Option<&str>,
    selected: bool,
    enabled: bool,
    t: &Tokens,
) -> Response {
    draw_styled_icon_button(
        ui,
        icon,
        label,
        selected,
        enabled,
        &ButtonStyle::compact(),
        t,
    )
}

/// Tactile toolbar button that fits its label closely and truncates with an
/// ellipsis once it reaches `max_width`.
pub fn max_width_icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    max_width: f32,
    selected: bool,
    enabled: bool,
    t: &Tokens,
) -> Response {
    draw_styled_icon_button(
        ui,
        icon,
        Some(label),
        selected,
        enabled,
        &ButtonStyle::standard(Some(max_width)),
        t,
    )
}

fn draw_styled_icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    label: Option<&str>,
    selected: bool,
    enabled: bool,
    style: &ButtonStyle,
    t: &Tokens,
) -> Response {
    let height = min_target(ui.ctx(), 28.0);
    let touch = height >= TOUCH_TARGET;
    let icon_dim = if touch {
        (style.icon_dim + 3.0).min(20.0)
    } else {
        style.icon_dim
    };
    let base_color = if !enabled {
        t.text_muted.gamma_multiply(0.45)
    } else {
        t.text
    };

    let (galley, width) = layout_button_label_and_width(ui, label, style, base_color);
    let width = width.max(min_target(ui.ctx(), 0.0));

    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), sense);
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let hovered = enabled && response.hovered();
        let hover_t =
            crate::motion::hover_factor(ui.ctx(), response.id.with("hov"), hovered && !selected);
        let select_t = if enabled {
            crate::motion::hover_factor(ui.ctx(), response.id.with("sel"), selected)
        } else {
            0.0
        };
        // Calm ghost control at rest (zero resting box borders); hover tints softly, active pops in accent_soft.
        let rest_fill = crate::motion::lerp_color(Color32::TRANSPARENT, t.ghost_hover, hover_t);
        let fill = crate::motion::lerp_color(rest_fill, t.accent_soft, select_t);
        let radius = CornerRadius::same(crate::theme::CONTROL_RADIUS);
        painter.rect_filled(rect, radius, fill);
        let rest_fg = if label.is_some() {
            t.text
        } else {
            crate::motion::lerp_color(t.text_muted, t.text, 0.55 + 0.45 * hover_t)
        };
        let foreground = if !enabled {
            base_color
        } else {
            crate::motion::lerp_color(rest_fg, t.accent, select_t)
        };

        let icon_rect = match &galley {
            Some(galley) => {
                let icon_rect = Rect::from_center_size(
                    pos2(
                        rect.left() + style.pad_left + icon_dim * 0.5,
                        rect.center().y,
                    ),
                    Vec2::splat(icon_dim),
                );
                let text_pos = pos2(
                    icon_rect.right() + style.gap,
                    rect.center().y - galley.size().y * 0.5,
                );
                painter.galley_with_override_text_color(text_pos, galley.clone(), foreground);
                icon_rect
            }
            None => Rect::from_center_size(rect.center(), Vec2::splat(icon_dim)),
        };
        paint_icon(painter, icon_rect, icon, foreground);
    }
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}

/// Layout strategy for [`segmented_switch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchLayout {
    /// Inline compact switch (`28.0` px tall) hugging its segment labels and icons.
    Compact,
    /// Full-width switch dividing `ui.available_width()` equally across all segments (`32.0` px tall).
    EqualWidth,
}

/// One mutually exclusive option rendered inside [`segmented_switch`].
#[derive(Clone, Copy, Debug)]
pub struct SwitchOption<'a> {
    pub icon: Option<Icon>,
    pub label: Option<&'a str>,
    pub tooltip: &'a str,
    pub enabled: bool,
}

impl<'a> SwitchOption<'a> {
    pub const fn new(
        icon: Option<Icon>,
        label: Option<&'a str>,
        tooltip: &'a str,
        enabled: bool,
    ) -> Self {
        Self {
            icon,
            label,
            tooltip,
            enabled,
        }
    }
}

struct MeasuredSegment {
    galley: Option<std::sync::Arc<egui::Galley>>,
    compact_width: f32,
}

fn measure_switch_segment(
    ui: &egui::Ui,
    opt: &SwitchOption<'_>,
    font_size: f32,
    seg_h: f32,
) -> MeasuredSegment {
    let icon_dim = 14.0_f32;
    let pad_x = 7.0_f32;
    let gap = 4.5_f32;
    let galley = opt.label.filter(|txt| !txt.is_empty()).map(|txt| {
        ui.painter().layout_no_wrap(
            txt.to_string(),
            FontId::proportional(font_size),
            Color32::WHITE,
        )
    });
    let compact_width = match (opt.icon.is_some(), &galley) {
        (true, Some(g)) => (pad_x + icon_dim + gap + g.size().x + pad_x).max(seg_h),
        (true, None) => seg_h,
        (false, Some(g)) => (pad_x + 2.0 + g.size().x + pad_x + 2.0).max(34.0),
        (false, None) => seg_h,
    };
    MeasuredSegment {
        galley,
        compact_width,
    }
}

fn paint_switch_segment_content(
    painter: &egui::Painter,
    seg_rect: Rect,
    icon: Option<Icon>,
    galley: Option<&std::sync::Arc<egui::Galley>>,
    fg: Color32,
) {
    let icon_dim = 14.0_f32;
    let gap = 4.5_f32;
    let cy = seg_rect.center().y;
    match (icon, galley) {
        (Some(icon), Some(g)) => {
            let total_w = icon_dim + gap + g.size().x;
            let start_x = seg_rect.center().x - total_w * 0.5;
            let icon_rect =
                Rect::from_center_size(pos2(start_x + icon_dim * 0.5, cy), Vec2::splat(icon_dim));
            paint_icon(painter, icon_rect, icon, fg);
            let text_pos = pos2(start_x + icon_dim + gap, cy - g.size().y * 0.5);
            painter.galley_with_override_text_color(text_pos, g.clone(), fg);
        }
        (Some(icon), None) => {
            let icon_rect = Rect::from_center_size(seg_rect.center(), Vec2::splat(icon_dim));
            paint_icon(painter, icon_rect, icon, fg);
        }
        (None, Some(g)) => {
            let text_pos = pos2(
                seg_rect.center().x - g.size().x * 0.5,
                cy - g.size().y * 0.5,
            );
            painter.galley_with_override_text_color(text_pos, g.clone(), fg);
        }
        (None, None) => {}
    }
}

fn compute_switch_track_width(
    ui: &egui::Ui,
    measured: &[MeasuredSegment],
    layout: SwitchLayout,
    track_pad: f32,
    gaps_total: f32,
) -> f32 {
    let n = measured.len();
    match layout {
        SwitchLayout::Compact => {
            let sum_w: f32 = measured.iter().map(|m| m.compact_width).sum();
            track_pad * 2.0 + sum_w + gaps_total
        }
        SwitchLayout::EqualWidth => ui.available_width().max(n as f32 * 44.0),
    }
}

/// Measures the exact rendered track width of a [`segmented_switch`].
pub fn segmented_switch_width(
    ui: &egui::Ui,
    options: &[SwitchOption<'_>],
    layout: SwitchLayout,
) -> f32 {
    if options.is_empty() {
        return 0.0;
    }
    let (base_h, font_size) = match layout {
        SwitchLayout::Compact => (28.0_f32, 11.8_f32),
        SwitchLayout::EqualWidth => (32.0_f32, 12.0_f32),
    };
    let track_h = min_target(ui.ctx(), base_h);
    let track_pad = 3.0_f32;
    let seg_gap = 2.0_f32;
    let seg_h = track_h - track_pad * 2.0;
    let measured: Vec<MeasuredSegment> = options
        .iter()
        .map(|opt| measure_switch_segment(ui, opt, font_size, seg_h))
        .collect();
    let gaps_total = seg_gap * options.len().saturating_sub(1) as f32;
    compute_switch_track_width(ui, &measured, layout, track_pad, gaps_total)
}

/// Renders an animated segmented switch for 2, 3, or more mutually exclusive options.
///
/// Returns `(clicked_index, track_response)` so callers can handle both switching and
/// optional re-click actions while animating the active pill smoothly across segments.
pub fn segmented_switch(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    options: &[SwitchOption<'_>],
    active: Option<usize>,
    layout: SwitchLayout,
    t: &Tokens,
) -> (Option<usize>, Response) {
    if options.is_empty() {
        let (_, empty_resp) = ui.allocate_exact_size(Vec2::ZERO, Sense::hover());
        return (None, empty_resp);
    }

    let (base_h, track_radius, pill_radius, font_size) = match layout {
        SwitchLayout::Compact => (28.0_f32, 10_u8, 8_u8, 11.8_f32),
        SwitchLayout::EqualWidth => (32.0_f32, 10_u8, 8_u8, 12.0_f32),
    };
    let track_h = min_target(ui.ctx(), base_h);
    let touch = track_h >= TOUCH_TARGET;
    let track_pad = 3.0_f32;
    let seg_gap = 2.0_f32;
    let seg_h = track_h - track_pad * 2.0;

    let measured: Vec<MeasuredSegment> = options
        .iter()
        .map(|opt| measure_switch_segment(ui, opt, font_size, seg_h))
        .collect();

    let n = options.len();
    let gaps_total = seg_gap * n.saturating_sub(1) as f32;
    let track_w = compute_switch_track_width(ui, &measured, layout, track_pad, gaps_total);
    let equal_seg_w = (track_w - track_pad * 2.0 - gaps_total).max(20.0) / n as f32;

    let track_id = ui.make_persistent_id(id_salt);
    let (track_rect, track_resp) =
        ui.allocate_exact_size(Vec2::new(track_w, track_h), Sense::hover());

    let mut seg_rects = Vec::with_capacity(n);
    let mut x = track_rect.left() + track_pad;
    let seg_top = track_rect.top() + track_pad;
    for item in &measured {
        let w = match layout {
            SwitchLayout::Compact => item.compact_width,
            SwitchLayout::EqualWidth => equal_seg_w,
        };
        seg_rects.push(Rect::from_min_size(pos2(x, seg_top), Vec2::new(w, seg_h)));
        x += w + seg_gap;
    }

    let mut clicked = None;
    let mut hovered_flags = Vec::with_capacity(n);
    for (idx, opt) in options.iter().enumerate() {
        let seg_id = track_id.with(("seg", idx));
        let sense = if opt.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let mut resp = ui.interact(seg_rects[idx], seg_id, sense);
        if opt.enabled {
            resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        if !opt.tooltip.is_empty() {
            resp = resp.on_hover_text(opt.tooltip);
        }
        if opt.enabled && resp.clicked() {
            clicked = Some(idx);
        }
        hovered_flags.push(opt.enabled && resp.hovered());
    }

    if ui.is_rect_visible(track_rect) {
        let p = ui.painter();
        // Calm borderless recessed track; the elevated active pill provides the visual focal point.
        p.rect_filled(track_rect, CornerRadius::same(track_radius), t.pill_active);

        for (idx, opt) in options.iter().enumerate() {
            let is_active = active == Some(idx);
            let hover_t = crate::motion::hover_factor(
                ui.ctx(),
                track_id.with(("hov", idx)),
                hovered_flags[idx] && !is_active,
            );
            if opt.enabled && hover_t > 0.02 {
                p.rect_filled(
                    seg_rects[idx],
                    CornerRadius::same(pill_radius),
                    t.ghost_hover.gamma_multiply(hover_t),
                );
            }
        }

        if let Some(active_idx) = active.filter(|&i| i < n) {
            let target_r = seg_rects[active_idx];
            let rel_x0 = target_r.left() - track_rect.left();
            let rel_x1 = target_r.right() - track_rect.left();
            let anim_x0 = track_rect.left()
                + ui.ctx().animate_value_with_time(
                    track_id.with("pill_x0"),
                    rel_x0,
                    crate::motion::MICRO_DURATION_S,
                );
            let anim_x1 = track_rect.left()
                + ui.ctx().animate_value_with_time(
                    track_id.with("pill_x1"),
                    rel_x1,
                    crate::motion::MICRO_DURATION_S,
                );
            let pill_rect = Rect::from_min_max(
                pos2(anim_x0, target_r.top()),
                pos2(anim_x1, target_r.bottom()),
            );
            // Subtle elevation drop shadow under the active cobalt-tinted pill.
            p.rect_filled(
                pill_rect.translate(Vec2::new(0.0, 1.0)),
                CornerRadius::same(pill_radius),
                Color32::from_rgba_unmultiplied(0, 0, 0, if t.is_dark { 64 } else { 14 }),
            );
            p.rect(
                pill_rect,
                CornerRadius::same(pill_radius),
                t.accent_soft,
                Stroke::new(
                    1.0,
                    t.accent.gamma_multiply(if t.is_dark { 0.50 } else { 0.30 }),
                ),
                StrokeKind::Inside,
            );
        }

        for (idx, opt) in options.iter().enumerate() {
            let is_active = active == Some(idx);
            let fg = if !opt.enabled {
                t.text_muted.gamma_multiply(0.55)
            } else {
                let select_t =
                    crate::motion::hover_factor(ui.ctx(), track_id.with(("sel", idx)), is_active);
                let hover_t = crate::motion::hover_factor(
                    ui.ctx(),
                    track_id.with(("hov", idx)),
                    hovered_flags[idx] && !is_active,
                );
                let rest_fg = crate::motion::lerp_color(t.text_muted, t.text, 0.35);
                let hovered_fg = crate::motion::lerp_color(rest_fg, t.text, hover_t);
                crate::motion::lerp_color(hovered_fg, t.accent, select_t)
            };
            let seg_rect = if touch && opt.label.is_none() {
                seg_rects[idx].expand(1.5)
            } else {
                seg_rects[idx]
            };
            paint_switch_segment_content(p, seg_rect, opt.icon, measured[idx].galley.as_ref(), fg);
        }
    }

    (clicked, track_resp)
}

/// Renders a compact Lucide close/remove icon button (used inside tag and template chips).
pub fn chip_close_button(ui: &mut egui::Ui, tooltip: &str, color: Color32, t: &Tokens) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(4), t.border.gamma_multiply(0.45));
        }
        let fg = if resp.hovered() { t.text } else { color };
        paint_icon(ui.painter(), rect.shrink(2.5), Icon::Close, fg);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tooltip)
}

/// Renders a section heading with an icon and muted plain-English subtitle.
pub fn section_card_header(ui: &mut egui::Ui, icon: Icon, title: &str, subtitle: &str, t: &Tokens) {
    ui.horizontal(|ui| {
        let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
        paint_icon(ui.painter(), icon_rect, icon, t.accent);
        ui.label(egui::RichText::new(title).size(15.0).strong().color(t.text));
    });
    ui.label(egui::RichText::new(subtitle).size(11.5).color(t.text_muted));
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::{
        Icon, SwitchLayout, SwitchOption, max_width_icon_button, max_width_icon_button_width,
        paint_icon, segmented_switch,
    };
    use egui::{Color32, Pos2, RawInput, Rect, Vec2, epaint::Primitive};

    fn all_icons() -> &'static [(Icon, &'static str)] {
        &[
            (Icon::FountainPen, "FountainPen"),
            (Icon::BallPen, "BallPen"),
            (Icon::BrushPen, "BrushPen"),
            (Icon::Highlighter, "Highlighter"),
            (Icon::HighlighterFreehand, "HighlighterFreehand"),
            (Icon::HighlighterStraight, "HighlighterStraight"),
            (Icon::Eraser, "Eraser"),
            (Icon::StrokeEraser, "StrokeEraser"),
            (Icon::PrecisionEraser, "PrecisionEraser"),
            (Icon::Undo, "Undo"),
            (Icon::Redo, "Redo"),
            (Icon::ClearPage, "ClearPage"),
            (Icon::FitPage, "FitPage"),
            (Icon::ZoomIn, "ZoomIn"),
            (Icon::ZoomOut, "ZoomOut"),
            (Icon::Settings, "Settings"),
            (Icon::Preset, "Preset"),
            (Icon::Stabilizer, "Stabilizer"),
            (Icon::Pressure, "Pressure"),
            (Icon::Paper, "Paper"),
            (Icon::Telemetry, "Telemetry"),
            (Icon::Reset, "Reset"),
            (Icon::Info, "Info"),
            (Icon::Notebook, "Notebook"),
            (Icon::Library, "Library"),
            (Icon::Search, "Search"),
            (Icon::Plus, "Plus"),
            (Icon::Export, "Export"),
            (Icon::AddPage, "AddPage"),
            (Icon::Duplicate, "Duplicate"),
            (Icon::PrevPage, "PrevPage"),
            (Icon::NextPage, "NextPage"),
            (Icon::Close, "Close"),
            (Icon::Palette, "Palette"),
            (Icon::Edit, "Edit"),
            (Icon::Trash, "Trash"),
            (Icon::Check, "Check"),
            (Icon::Sun, "Sun"),
            (Icon::Moon, "Moon"),
            (Icon::Account, "Account"),
            (Icon::Sync, "Sync"),
            (Icon::CheckCircle, "CheckCircle"),
            (Icon::AlertCircle, "AlertCircle"),
            (Icon::Server, "Server"),
            (Icon::Eye, "Eye"),
            (Icon::EyeOff, "EyeOff"),
            (Icon::ShapeTool, "ShapeTool"),
            (Icon::LineShape, "LineShape"),
            (Icon::RectangleShape, "RectangleShape"),
            (Icon::EllipseShape, "EllipseShape"),
            (Icon::TriangleShape, "TriangleShape"),
            (Icon::ArrowShape, "ArrowShape"),
            (Icon::Ruler, "Ruler"),
            (Icon::Select, "Select"),
            (Icon::Image, "Image"),
            (Icon::Link, "Link"),
            (Icon::Folder, "Folder"),
            (Icon::FolderOpen, "FolderOpen"),
            (Icon::FolderPlus, "FolderPlus"),
            (Icon::FolderPen, "FolderPen"),
            (Icon::MoveToFolder, "MoveToFolder"),
            (Icon::Inbox, "Inbox"),
            (Icon::ChevronUp, "ChevronUp"),
            (Icon::ChevronDown, "ChevronDown"),
            (Icon::More, "More"),
            (Icon::Star, "Star"),
            (Icon::Tag, "Tag"),
            (Icon::ListView, "ListView"),
            (Icon::SidebarToggle, "SidebarToggle"),
            (Icon::Camera, "Camera"),
            (Icon::Scan, "Scan"),
            (Icon::Users, "Users"),
            (Icon::Marketplace, "Marketplace"),
            (Icon::QuickNote, "QuickNote"),
            (Icon::TextDoc, "TextDoc"),
            (Icon::Whiteboard, "Whiteboard"),
            (Icon::Cloud, "Cloud"),
            (Icon::CloudOff, "CloudOff"),
            (Icon::CloudCheck, "CloudCheck"),
            (Icon::ListFilter, "ListFilter"),
            (Icon::FolderApple, "FolderApple"),
            (Icon::FolderAtom, "FolderAtom"),
            (Icon::FolderBriefcase, "FolderBriefcase"),
            (Icon::FolderCalculator, "FolderCalculator"),
            (Icon::FolderChart, "FolderChart"),
            (Icon::FolderCode, "FolderCode"),
            (Icon::FolderGear, "FolderGear"),
            (Icon::FolderLandmark, "FolderLandmark"),
            (Icon::FolderMoney, "FolderMoney"),
            (Icon::FolderFlask, "FolderFlask"),
            (Icon::FolderGradCap, "FolderGradCap"),
            (Icon::FolderScale, "FolderScale"),
            (Icon::FolderMusic, "FolderMusic"),
            (Icon::FolderMoon, "FolderMoon"),
            (Icon::FolderHeart, "FolderHeart"),
            (Icon::FolderCompass, "FolderCompass"),
            (Icon::FolderGlobe, "FolderGlobe"),
            (Icon::Ban, "Ban"),
            (Icon::GridView, "GridView"),
            (Icon::Sort, "Sort"),
            (Icon::MoveUp, "MoveUp"),
            (Icon::MoveDown, "MoveDown"),
            (Icon::Bookmark, "Bookmark"),
            (Icon::RotateCw, "RotateCw"),
            (Icon::CheckSquare, "CheckSquare"),
            (Icon::Transfer, "Transfer"),
            (Icon::Pencil, "Pencil"),
            (Icon::Lasso, "Lasso"),
            (Icon::PolygonShape, "PolygonShape"),
            (Icon::FillBucket, "FillBucket"),
            (Icon::Paste, "Paste"),
            (Icon::Bold, "Bold"),
            (Icon::Italic, "Italic"),
            (Icon::Underline, "Underline"),
            (Icon::AlignLeft, "AlignLeft"),
            (Icon::AlignCenter, "AlignCenter"),
            (Icon::AlignRight, "AlignRight"),
            (Icon::CheckList, "CheckList"),
            (Icon::Sticker, "Sticker"),
            (Icon::LaserPointer, "LaserPointer"),
            (Icon::GripHorizontal, "GripHorizontal"),
            (Icon::Text, "Text"),
        ]
    }

    #[test]
    fn all_icons_tessellate_within_bounds_without_miter_spikes() {
        let icon_rect = Rect::from_min_size(Pos2::new(20.0, 20.0), Vec2::splat(15.0));
        let safe_bounds = icon_rect.expand(2.0);
        let moon_bounds = icon_rect.expand(0.5);
        for &(icon, name) in all_icons() {
            let ctx = egui::Context::default();
            let raw = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(80.0))),
                ..Default::default()
            };
            let out = ctx.run_ui(raw, |ui| {
                paint_icon(ui.painter(), icon_rect, icon, Color32::WHITE);
            });
            let primitives = ctx.tessellate(out.shapes.clone(), 1.0);
            out.drop_without_applying_deltas();

            let bounds = if icon == Icon::Moon {
                moon_bounds
            } else {
                safe_bounds
            };
            let mut vertex_count = 0usize;
            for clipped in &primitives {
                if let Primitive::Mesh(mesh) = &clipped.primitive {
                    for vertex in &mesh.vertices {
                        vertex_count += 1;
                        assert!(
                            bounds.contains(vertex.pos),
                            "icon {name} emitted out-of-bounds vertex {:?} outside {:?}",
                            vertex.pos,
                            bounds
                        );
                    }
                }
            }
            assert!(vertex_count > 0, "icon {name} emitted zero mesh vertices");
        }
    }

    fn collect_galleys(shapes: &[egui::epaint::ClippedShape]) -> Vec<std::sync::Arc<egui::Galley>> {
        let mut galleys = Vec::new();
        fn visit(shape: &egui::Shape, out: &mut Vec<std::sync::Arc<egui::Galley>>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.clone()),
                egui::Shape::Vec(items) => {
                    for s in items {
                        visit(s, out);
                    }
                }
                _ => {}
            }
        }
        for clipped in shapes {
            visit(&clipped.shape, &mut galleys);
        }
        galleys
    }

    fn galley_rendered_text(galley: &egui::Galley) -> String {
        let mut out = String::new();
        for placed in &galley.rows {
            for glyph in &placed.row.glyphs {
                out.push(glyph.chr);
            }
        }
        out
    }

    #[test]
    fn max_width_icon_button_fits_short_text_and_truncates_long_text() {
        let t = crate::theme::Tokens::dark();
        let max_w = 132.0_f32;
        let run_case = |label: &str, cap: f32| -> (Rect, f32, String) {
            let ctx = egui::Context::default();
            let raw = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 80.0))),
                ..Default::default()
            };
            let mut btn_rect = Rect::NOTHING;
            let mut measured_w = 0.0_f32;
            let out = ctx.run_ui(raw, |ui| {
                btn_rect =
                    max_width_icon_button(ui, Icon::Notebook, label, cap, true, true, &t).rect;
                measured_w = max_width_icon_button_width(ui, label, cap);
            });
            let galleys = collect_galleys(&out.shapes);
            let rendered = galleys
                .first()
                .map(|g| galley_rendered_text(g))
                .unwrap_or_default();
            out.drop_without_applying_deltas();
            (btn_rect, measured_w, rendered)
        };

        let (math_rect, math_measured, math_text) = run_case("Math", max_w);
        assert!(
            (math_rect.width() - math_measured).abs() < 0.01,
            "drawn width {} != measured width {}",
            math_rect.width(),
            math_measured
        );
        assert!(
            math_rect.width() < 90.0,
            "expected short label 'Math' to shrink-to-fit below 90px, got {:.1}",
            math_rect.width()
        );
        assert_eq!(math_text, "Math");

        let (calc_rect, _, calc_text) = run_case("Calculus II", max_w);
        assert!(
            calc_rect.width() > math_rect.width() && calc_rect.width() <= max_w,
            "expected 'Calculus II' ({:.1}) wider than 'Math' ({:.1}) and <= {max_w}",
            calc_rect.width(),
            math_rect.width()
        );
        assert_eq!(calc_text, "Calculus II");

        let long = "Mathsssssssssssssssssssssssssssssssssssssssssssssssssssssssss";
        let (long_rect, long_measured, long_text) = run_case(long, max_w);
        assert!(
            (long_rect.width() - long_measured).abs() < 0.01,
            "drawn width {} != measured width {}",
            long_rect.width(),
            long_measured
        );
        assert!(
            long_rect.width() <= max_w + 0.01 && long_rect.width() > 110.0,
            "expected long label to cap near {max_w}, got {:.1}",
            long_rect.width()
        );
        assert!(
            long_text.ends_with('…'),
            "expected truncated text to end with ellipsis, got {long_text:?}"
        );

        // Hostile inputs never panic and remain bounded.
        let (empty_rect, _, empty_text) = run_case("", max_w);
        assert!((empty_rect.width() - 28.0).abs() < 0.01);
        assert!(empty_text.is_empty());

        let (neg_rect, _, _) = run_case("Math", -10.0);
        assert!((neg_rect.width() - 28.0).abs() < 0.01);

        let (nan_rect, _, nan_text) = run_case("Math", f32::NAN);
        assert!(nan_rect.width().is_finite() && nan_rect.width() >= 28.0);
        assert_eq!(nan_text, "Math");

        let (inf_rect, _, inf_text) = run_case("Math", f32::INFINITY);
        assert!(inf_rect.width().is_finite() && inf_rect.width() >= 28.0);
        assert_eq!(inf_text, "Math");
    }

    fn collect_fill_rects(shapes: &[egui::epaint::ClippedShape], fill: Color32) -> Vec<Rect> {
        fn walk(shape: &egui::Shape, fill: Color32, out: &mut Vec<Rect>) {
            match shape {
                egui::Shape::Rect(r) if r.fill == fill => out.push(r.rect),
                egui::Shape::Vec(v) => {
                    for s in v {
                        walk(s, fill, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, fill, &mut out);
        }
        out
    }

    #[test]
    fn segmented_switch_layouts_heights_and_active_none() {
        let t = crate::theme::Tokens::light();
        let ctx = egui::Context::default();
        let opts_2 = [
            SwitchOption::new(Some(Icon::GridView), Some("Grid"), "Grid view", true),
            SwitchOption::new(Some(Icon::ListView), Some("List"), "List view", true),
        ];
        let opts_3 = [
            SwitchOption::new(None, Some("One"), "Option 1", true),
            SwitchOption::new(None, Some("Two"), "Option 2", true),
            SwitchOption::new(None, Some("Three"), "Option 3", true),
        ];
        let opts_5 = [
            SwitchOption::new(Some(Icon::LineShape), None, "Line", true),
            SwitchOption::new(Some(Icon::RectangleShape), None, "Rect", true),
            SwitchOption::new(Some(Icon::EllipseShape), None, "Ellipse", true),
            SwitchOption::new(Some(Icon::TriangleShape), None, "Triangle", true),
            SwitchOption::new(Some(Icon::ArrowShape), None, "Arrow", true),
        ];

        let mut compact_rect = Rect::NOTHING;
        let mut equal_rect = Rect::NOTHING;
        let mut five_rect = Rect::NOTHING;

        let out = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 200.0))),
                ..Default::default()
            },
            |ui| {
                compact_rect = segmented_switch(
                    ui,
                    "sw_compact_2",
                    &opts_2,
                    Some(0),
                    SwitchLayout::Compact,
                    &t,
                )
                .1
                .rect;
                equal_rect = segmented_switch(
                    ui,
                    "sw_equal_3",
                    &opts_3,
                    None,
                    SwitchLayout::EqualWidth,
                    &t,
                )
                .1
                .rect;
                five_rect = segmented_switch(
                    ui,
                    "sw_compact_5",
                    &opts_5,
                    Some(2),
                    SwitchLayout::Compact,
                    &t,
                )
                .1
                .rect;
            },
        );
        let pill_rects = collect_fill_rects(&out.shapes, t.accent_soft);
        out.drop_without_applying_deltas();

        assert!((compact_rect.height() - 28.0).abs() < 0.01);
        assert!((equal_rect.height() - 32.0).abs() < 0.01);
        assert!((five_rect.height() - 28.0).abs() < 0.01);
        assert!(equal_rect.width() >= 300.0);
        // sw_compact_2 and sw_compact_5 draw active pills; sw_equal_3 (active=None) draws none.
        assert_eq!(pill_rects.len(), 2);
        assert!(pill_rects[0].width() < compact_rect.width() * 0.65);
        assert!(pill_rects[1].width() < five_rect.width() * 0.35);
    }

    #[test]
    fn segmented_switch_clicks_disabled_handling_and_smooth_pill_slide() {
        let t = crate::theme::Tokens::light();
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(360.0, 120.0));
        let options = [
            SwitchOption::new(None, Some("Copy"), "Copy", true),
            SwitchOption::new(None, Some("Disabled"), "Disabled", false),
            SwitchOption::new(None, Some("Move"), "Move", true),
        ];

        let run_frame = |time: f64, events: Vec<egui::Event>, active: Option<usize>| {
            let mut track_rect = Rect::NOTHING;
            let mut clicked = None;
            let out = ctx.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(300.0);
                    let (c, r) = segmented_switch(
                        ui,
                        "sw_anim_test",
                        &options,
                        active,
                        SwitchLayout::EqualWidth,
                        &t,
                    );
                    clicked = c;
                    track_rect = r.rect;
                },
            );
            let pills = collect_fill_rects(&out.shapes, t.accent_soft);
            out.drop_without_applying_deltas();
            (track_rect, clicked, pills)
        };

        // Initial settle on segment 0.
        let (track_rect, _, pills_0) = run_frame(0.0, vec![], Some(0));
        assert_eq!(pills_0.len(), 1);
        let pill_start_x = pills_0[0].left();

        // Click disabled middle segment -> must not fire.
        let mid_pt = track_rect.center();
        run_frame(
            0.05,
            vec![
                egui::Event::PointerMoved(mid_pt),
                egui::Event::PointerButton {
                    pos: mid_pt,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            Some(0),
        );
        let (_, disabled_click, _) = run_frame(
            0.06,
            vec![egui::Event::PointerButton {
                pos: mid_pt,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            Some(0),
        );
        assert_eq!(disabled_click, None);

        // Re-clicking active segment 0 still reports Some(0).
        let left_pt = Pos2::new(track_rect.left() + 30.0, track_rect.center().y);
        run_frame(
            0.10,
            vec![
                egui::Event::PointerMoved(left_pt),
                egui::Event::PointerButton {
                    pos: left_pt,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            Some(0),
        );
        let (_, re_click, _) = run_frame(
            0.11,
            vec![egui::Event::PointerButton {
                pos: left_pt,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            Some(0),
        );
        assert_eq!(re_click, Some(0));

        // Click segment 2 and switch active to Some(2), checking intermediate pill slide.
        let right_pt = Pos2::new(track_rect.right() - 30.0, track_rect.center().y);
        run_frame(
            0.20,
            vec![
                egui::Event::PointerMoved(right_pt),
                egui::Event::PointerButton {
                    pos: right_pt,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            Some(0),
        );
        let (_, third_click, _) = run_frame(
            0.21,
            vec![egui::Event::PointerButton {
                pos: right_pt,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            Some(0),
        );
        assert_eq!(third_click, Some(2));

        // Frame switching active to 2 (mid-animation ~0.04s later) then settled.
        run_frame(0.22, vec![], Some(2));
        let (_, _, pills_mid) = run_frame(0.26, vec![], Some(2));
        let (_, _, pills_end) = run_frame(0.50, vec![], Some(2));
        assert_eq!(pills_mid.len(), 1);
        assert_eq!(pills_end.len(), 1);
        assert!(
            pills_mid[0].left() > pill_start_x + 10.0 && pills_mid[0].left() < pills_end[0].left(),
            "expected smooth mid-slide between {pill_start_x:.1} and {:.1}, got {:.1}",
            pills_end[0].left(),
            pills_mid[0].left()
        );
    }
}
