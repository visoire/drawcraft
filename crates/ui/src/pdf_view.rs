//! Bounded GPU texture cache and background rendering helpers for PDF page backdrops.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Condvar, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use drawcraft_doc::{
    CustomPaperTemplate, CustomTemplateKind, Document, EmbeddedImageTemplate, EmbeddedPdf,
    NotebookPage, PdfPageBackdrop,
};
use drawcraft_engine::Session;
use egui::{Color32, CornerRadius, Margin, Mesh, Rect, Sense, Shape, Stroke, Vec2, pos2};

use crate::icons::{Icon, paint_icon};
use crate::theme::Tokens;

const MAX_CANVAS_TEXTURES: usize = 16;
const MAX_THUMB_TEXTURES: usize = 96;
const MAX_FAILED_KEYS: usize = 512;
#[cfg(not(target_arch = "wasm32"))]
const MAX_PENDING_CANVAS_JOBS: usize = 8;
#[cfg(not(target_arch = "wasm32"))]
const MAX_PENDING_THUMB_JOBS: usize = 32;
#[cfg(not(target_arch = "wasm32"))]
const PDF_SESSION_IDLE_KEEPALIVE: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PdfTexKey {
    kind: u8,
    fingerprint: u64,
    byte_len: usize,
    page_index: usize,
    max_dim: u32,
}

#[derive(Clone)]
struct PdfTexEntry {
    handle: egui::TextureHandle,
    last_tick: u64,
}

#[derive(Clone, Default)]
struct PdfTexCache {
    entries: HashMap<PdfTexKey, PdfTexEntry>,
    failed: HashSet<PdfTexKey>,
    tick: u64,
}

fn fnv1a_extend(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn pdf_bytes_fingerprint(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    hash = fnv1a_extend(hash, &bytes.len().to_le_bytes());
    let head = &bytes[..bytes.len().min(128)];
    hash = fnv1a_extend(hash, head);
    if bytes.len() > 256 {
        let mid_start = bytes.len() / 2 - 64;
        hash = fnv1a_extend(hash, &bytes[mid_start..mid_start + 128]);
    }
    if bytes.len() > 128 {
        let tail_start = bytes.len() - 128;
        hash = fnv1a_extend(hash, &bytes[tail_start..]);
    }
    hash
}

/// Texture kind for a PDF page rendered normally.
const KIND_PDF: u8 = 0;
/// Texture kind for a PDF page whose lightness is inverted for dark reading.
const KIND_PDF_DARK: u8 = 2;

/// Records whether PDF pages are shown inverted (set once per frame from the preferences).
pub(crate) fn set_pdf_dark(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("drawcraft_pdf_dark"), on));
}

fn pdf_dark(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(egui::Id::unique("drawcraft_pdf_dark")))
        .unwrap_or(false)
}

fn cache_id_for_backdrop(is_thumb: bool) -> egui::Id {
    egui::Id::unique(("drawcraft_pdf_tex_cache", is_thumb))
}

fn insert_rendered_texture(
    ctx: &egui::Context,
    key: PdfTexKey,
    rgba: Option<drawcraft_engine::RenderedPdfPage>,
) {
    let is_thumb = key.max_dim <= 300;
    let cache_id = cache_id_for_backdrop(is_thumb);
    let cap = if is_thumb {
        MAX_THUMB_TEXTURES
    } else {
        MAX_CANVAS_TEXTURES
    };

    let Some(mut rendered) = rgba else {
        ctx.data_mut(|d| {
            let cache = d.get_temp_mut_or_default::<PdfTexCache>(cache_id);
            if cache.failed.len() >= MAX_FAILED_KEYS {
                cache.failed.clear();
            }
            cache.failed.insert(key);
        });
        return;
    };

    if key.kind == KIND_PDF_DARK {
        drawcraft_color::invert_rgba8_buffer_for_dark(&mut rendered.rgba);
    }
    let color_image = egui::ColorImage::from_rgba_unmultiplied(
        [rendered.width as usize, rendered.height as usize],
        &rendered.rgba,
    );
    let handle = ctx.load_texture(
        format!(
            "drawcraft_pdf_{:016x}_{}_{}_{}",
            key.fingerprint, key.page_index, key.max_dim, key.kind
        ),
        color_image,
        egui::TextureOptions::LINEAR,
    );

    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<PdfTexCache>(cache_id);
        cache.tick = cache.tick.saturating_add(1);
        let now = cache.tick;
        while cache.entries.len() >= cap {
            let Some(oldest) = cache
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_tick)
                .map(|(k, _)| *k)
            else {
                break;
            };
            cache.entries.remove(&oldest);
        }
        cache.entries.insert(
            key,
            PdfTexEntry {
                handle,
                last_tick: now,
            },
        );
    });
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, PartialEq, Eq)]
struct PdfDocIdentity {
    fingerprint: u64,
    byte_len: usize,
}

#[cfg(not(target_arch = "wasm32"))]
struct PendingRenderJob {
    key: PdfTexKey,
    bytes: Arc<Vec<u8>>,
    page_index: usize,
    max_dim: u32,
    ctx: egui::Context,
    seq: u64,
}

#[cfg(not(target_arch = "wasm32"))]
struct CompletedRender {
    key: PdfTexKey,
    rgba: Option<drawcraft_engine::RenderedPdfPage>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct RenderQueueState {
    pending_canvas: Vec<PendingRenderJob>,
    pending_thumbs: Vec<PendingRenderJob>,
    in_flight: Option<PdfTexKey>,
    completed: Vec<CompletedRender>,
    next_seq: u64,
    shutdown: bool,
}

#[cfg(not(target_arch = "wasm32"))]
struct RenderShared {
    state: Mutex<RenderQueueState>,
    condvar: Condvar,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct PdfRenderCoordinator {
    inner: Arc<PdfRenderCoordinatorInner>,
}

#[cfg(not(target_arch = "wasm32"))]
struct PdfRenderCoordinatorInner {
    shared: Arc<RenderShared>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for PdfRenderCoordinatorInner {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.shared.state.lock() {
            guard.shutdown = true;
            guard.pending_canvas.clear();
            guard.pending_thumbs.clear();
        }
        self.shared.condvar.notify_all();
        if let Ok(mut h) = self.handle.lock()
            && let Some(join) = h.take()
        {
            let _ = join.join();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl PdfRenderCoordinator {
    fn new() -> Self {
        let shared = Arc::new(RenderShared {
            state: Mutex::new(RenderQueueState::default()),
            condvar: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let handle = std::thread::Builder::new()
            .name("drawcraft-pdf-render".to_string())
            .spawn(move || run_pdf_render_worker(worker_shared))
            .ok();
        Self {
            inner: Arc::new(PdfRenderCoordinatorInner {
                shared,
                handle: Mutex::new(handle),
            }),
        }
    }

    fn enqueue(
        &self,
        key: PdfTexKey,
        bytes: Arc<Vec<u8>>,
        page_index: usize,
        max_dim: u32,
        ctx: &egui::Context,
    ) {
        let Ok(mut state) = self.inner.shared.state.lock() else {
            return;
        };
        if state.shutdown
            || state.in_flight == Some(key)
            || state.completed.iter().any(|c| c.key == key)
        {
            return;
        }
        state.next_seq = state.next_seq.saturating_add(1);
        let seq = state.next_seq;
        let is_thumb = max_dim <= 300;
        let (queue, cap) = if is_thumb {
            (&mut state.pending_thumbs, MAX_PENDING_THUMB_JOBS)
        } else {
            (&mut state.pending_canvas, MAX_PENDING_CANVAS_JOBS)
        };
        if let Some(existing) = queue.iter_mut().find(|job| job.key == key) {
            existing.seq = seq;
            return;
        }
        if queue.len() >= cap
            && let Some((oldest_idx, _)) = queue.iter().enumerate().min_by_key(|(_, j)| j.seq)
        {
            queue.swap_remove(oldest_idx);
        }
        queue.push(PendingRenderJob {
            key,
            bytes,
            page_index,
            max_dim,
            ctx: ctx.clone(),
            seq,
        });
        self.inner.shared.condvar.notify_one();
    }

    fn drain_completed(&self) -> Vec<CompletedRender> {
        let Ok(mut state) = self.inner.shared.state.lock() else {
            return Vec::new();
        };
        if state.completed.is_empty() {
            return Vec::new();
        }
        std::mem::take(&mut state.completed)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn pop_highest_seq(
    queue: &mut Vec<PendingRenderJob>,
    doc_filter: Option<PdfDocIdentity>,
) -> Option<PendingRenderJob> {
    let idx = queue
        .iter()
        .enumerate()
        .filter(|(_, job)| {
            doc_filter.is_none_or(|id| {
                job.key.fingerprint == id.fingerprint && job.key.byte_len == id.byte_len
            })
        })
        .max_by_key(|(_, job)| job.seq)
        .map(|(i, _)| i)?;
    Some(queue.swap_remove(idx))
}

#[cfg(not(target_arch = "wasm32"))]
fn pop_next_render_job(
    state: &mut RenderQueueState,
    preferred: Option<PdfDocIdentity>,
) -> Option<PendingRenderJob> {
    if let Some(id) = preferred
        && let Some(job) = pop_highest_seq(&mut state.pending_canvas, Some(id))
    {
        return Some(job);
    }
    if let Some(job) = pop_highest_seq(&mut state.pending_canvas, None) {
        return Some(job);
    }
    if let Some(id) = preferred
        && let Some(job) = pop_highest_seq(&mut state.pending_thumbs, Some(id))
    {
        return Some(job);
    }
    pop_highest_seq(&mut state.pending_thumbs, None)
}

#[cfg(not(target_arch = "wasm32"))]
fn take_session_followup_job(
    shared: &RenderShared,
    doc_id: PdfDocIdentity,
) -> Option<PendingRenderJob> {
    let Ok(mut state) = shared.state.lock() else {
        return None;
    };
    state.in_flight = None;
    let deadline = std::time::Instant::now() + PDF_SESSION_IDLE_KEEPALIVE;
    loop {
        if state.shutdown {
            return None;
        }
        if let Some(job) = pop_highest_seq(&mut state.pending_canvas, Some(doc_id)) {
            state.in_flight = Some(job.key);
            return Some(job);
        }
        if !state.pending_canvas.is_empty() {
            // Yield current PDF session immediately when another PDF's canvas page is waiting.
            return None;
        }
        if let Some(job) = pop_highest_seq(&mut state.pending_thumbs, Some(doc_id)) {
            state.in_flight = Some(job.key);
            return Some(job);
        }
        if !state.pending_thumbs.is_empty() {
            return None;
        }
        let now = std::time::Instant::now();
        if now >= deadline {
            return None;
        }
        let Ok((next_state, wait_res)) = shared.condvar.wait_timeout(state, deadline - now) else {
            return None;
        };
        state = next_state;
        if wait_res.timed_out()
            && state.pending_canvas.is_empty()
            && state.pending_thumbs.is_empty()
        {
            return None;
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn record_completed_job(
    shared: &RenderShared,
    key: PdfTexKey,
    ctx: &egui::Context,
    rgba: Option<drawcraft_engine::RenderedPdfPage>,
) {
    if let Ok(mut state) = shared.state.lock() {
        state.in_flight = None;
        state.completed.push(CompletedRender { key, rgba });
    }
    ctx.request_repaint();
}

#[cfg(not(target_arch = "wasm32"))]
fn run_pdf_render_worker(shared: Arc<RenderShared>) {
    loop {
        let first_job = {
            let Ok(mut state) = shared.state.lock() else {
                return;
            };
            while !state.shutdown
                && state.pending_canvas.is_empty()
                && state.pending_thumbs.is_empty()
            {
                let Ok(next) = shared.condvar.wait(state) else {
                    return;
                };
                state = next;
            }
            if state.shutdown {
                return;
            }
            let Some(job) = pop_next_render_job(&mut state, None) else {
                continue;
            };
            state.in_flight = Some(job.key);
            job
        };

        let doc_id = PdfDocIdentity {
            fingerprint: first_job.key.fingerprint,
            byte_len: first_job.key.byte_len,
        };
        let first_key = first_job.key;
        let first_ctx = first_job.ctx.clone();
        let open_res = Session::with_pdf_renderer(Arc::clone(&first_job.bytes), |renderer| {
            let mut job = Some(first_job);
            while let Some(current) = job {
                let rgba = renderer
                    .render_page_rgba(current.page_index, current.max_dim, current.max_dim)
                    .ok();
                record_completed_job(&shared, current.key, &current.ctx, rgba);
                job = take_session_followup_job(&shared, doc_id);
            }
        });
        if open_res.is_err() {
            record_completed_job(&shared, first_key, &first_ctx, None);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn pdf_coordinator(ctx: &egui::Context) -> PdfRenderCoordinator {
    let id = egui::Id::unique("drawcraft_pdf_render_coordinator");
    ctx.data_mut(|d| {
        if let Some(existing) = d.get_temp::<PdfRenderCoordinator>(id) {
            existing
        } else {
            let created = PdfRenderCoordinator::new();
            d.insert_temp(id, created.clone());
            created
        }
    })
}

/// Uploads completed background PDF renders into the texture cache for `ctx`.
pub(crate) fn pump_completed_renders(ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let coord = pdf_coordinator(ctx);
        for done in coord.drain_completed() {
            insert_rendered_texture(ctx, done.key, done.rgba);
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
    }
}

fn lookup_cached_texture(
    ctx: &egui::Context,
    key: PdfTexKey,
    is_thumb: bool,
) -> Option<egui::TextureId> {
    let cache_id = cache_id_for_backdrop(is_thumb);
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<PdfTexCache>(cache_id);
        cache.tick = cache.tick.saturating_add(1);
        let now = cache.tick;
        cache.entries.get_mut(&key).map(|entry| {
            entry.last_tick = now;
            entry.handle.id()
        })
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn lookup_fallback_canvas_texture(
    ctx: &egui::Context,
    fingerprint: u64,
    byte_len: usize,
    page_index: usize,
    kind: u8,
) -> Option<egui::TextureId> {
    for is_thumb in [false, true] {
        let cache_id = cache_id_for_backdrop(is_thumb);
        let found = ctx.data_mut(|d| {
            let cache = d.get_temp_mut_or_default::<PdfTexCache>(cache_id);
            cache
                .entries
                .iter()
                .filter(|(k, _)| {
                    k.kind == kind
                        && k.fingerprint == fingerprint
                        && k.byte_len == byte_len
                        && k.page_index == page_index
                })
                .max_by_key(|(k, _)| k.max_dim)
                .map(|(_, entry)| entry.handle.id())
        });
        if found.is_some() {
            return found;
        }
    }
    None
}

fn is_failed_key(ctx: &egui::Context, key: PdfTexKey, is_thumb: bool) -> bool {
    let cache_id = cache_id_for_backdrop(is_thumb);
    ctx.data(|d| {
        d.get_temp::<PdfTexCache>(cache_id)
            .is_some_and(|cache| cache.failed.contains(&key))
    })
}

fn resolve_backdrop_texture(
    ctx: &egui::Context,
    pdf: &EmbeddedPdf,
    page_index: usize,
    max_dim: u32,
) -> Option<egui::TextureId> {
    pump_completed_renders(ctx);
    let fp = pdf_bytes_fingerprint(&pdf.bytes);
    let kind = if pdf_dark(ctx) {
        KIND_PDF_DARK
    } else {
        KIND_PDF
    };
    let key = PdfTexKey {
        kind,
        fingerprint: fp,
        byte_len: pdf.bytes.len(),
        page_index,
        max_dim,
    };
    let is_thumb = max_dim <= 300;
    if let Some(hit) = lookup_cached_texture(ctx, key, is_thumb) {
        return Some(hit);
    }
    if is_failed_key(ctx, key, is_thumb) {
        return None;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let coord = pdf_coordinator(ctx);
        coord.enqueue(key, Arc::clone(&pdf.bytes), page_index, max_dim, ctx);
        if !is_thumb {
            return lookup_fallback_canvas_texture(ctx, fp, pdf.bytes.len(), page_index, kind);
        }
        None
    }

    #[cfg(target_arch = "wasm32")]
    {
        let rendered =
            Session::render_pdf_page_rgba(Arc::clone(&pdf.bytes), page_index, max_dim, max_dim)
                .ok();
        insert_rendered_texture(ctx, key, rendered);
        lookup_cached_texture(ctx, key, is_thumb)
    }
}

fn resolve_custom_image_texture(
    ctx: &egui::Context,
    tpl: &EmbeddedImageTemplate,
) -> Option<egui::TextureId> {
    if tpl.png_bytes.is_empty() {
        return None;
    }
    let fp = pdf_bytes_fingerprint(&tpl.png_bytes);
    let key = PdfTexKey {
        kind: 1,
        fingerprint: fp,
        byte_len: tpl.png_bytes.len(),
        page_index: 0,
        max_dim: 0,
    };
    let cache_id = egui::Id::unique("drawcraft_custom_tpl_tex_cache");
    if let Some(hit) = ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<PdfTexCache>(cache_id);
        cache.tick = cache.tick.saturating_add(1);
        let now = cache.tick;
        cache.entries.get_mut(&key).map(|entry| {
            entry.last_tick = now;
            entry.handle.id()
        })
    }) {
        return Some(hit);
    }

    let decoded = image::load_from_memory(&tpl.png_bytes).ok()?;
    let rgba = decoded.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let color_image = egui::ColorImage::from_rgba_unmultiplied([w, h], rgba.as_raw());
    let handle = ctx.load_texture(
        format!("drawcraft_custom_tpl_{fp:016x}"),
        color_image,
        egui::TextureOptions::LINEAR,
    );
    let tex_id = handle.id();
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<PdfTexCache>(cache_id);
        cache.tick = cache.tick.saturating_add(1);
        let now = cache.tick;
        while cache.entries.len() >= 32 {
            let Some(oldest) = cache
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_tick)
                .map(|(k, _)| *k)
            else {
                break;
            };
            cache.entries.remove(&oldest);
        }
        cache.entries.insert(
            key,
            PdfTexEntry {
                handle,
                last_tick: now,
            },
        );
    });
    Some(tex_id)
}

fn paint_rotated_texture(p: &egui::Painter, rect: Rect, tex_id: egui::TextureId, rotation: u8) {
    let uvs = match rotation % 4 {
        1 => [
            pos2(0.0, 1.0),
            pos2(0.0, 0.0),
            pos2(1.0, 0.0),
            pos2(1.0, 1.0),
        ],
        2 => [
            pos2(1.0, 1.0),
            pos2(0.0, 1.0),
            pos2(0.0, 0.0),
            pos2(1.0, 0.0),
        ],
        3 => [
            pos2(1.0, 0.0),
            pos2(1.0, 1.0),
            pos2(0.0, 1.0),
            pos2(0.0, 0.0),
        ],
        _ => [
            pos2(0.0, 0.0),
            pos2(1.0, 0.0),
            pos2(1.0, 1.0),
            pos2(0.0, 1.0),
        ],
    };
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    let mut mesh = Mesh::with_texture(tex_id);
    for (pos, uv) in corners.into_iter().zip(uvs) {
        mesh.vertices.push(egui::epaint::Vertex {
            pos,
            uv,
            color: Color32::WHITE,
        });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    p.with_clip_rect(rect).add(Shape::mesh(mesh));
}

/// Chooses one of three quantized resolution tiers (`900`, `1500`, `2400`) from zoomed canvas dimensions.
#[must_use]
pub(crate) fn canvas_pdf_tier_dim(ctx: &egui::Context, page_rect: Rect) -> u32 {
    let display_max = page_rect.width().max(page_rect.height()).max(64.0) * ctx.pixels_per_point();
    if display_max <= 900.0 {
        900
    } else if display_max <= 1500.0 {
        1500
    } else {
        2400
    }
}

fn paint_backdrop(
    ctx: &egui::Context,
    p: &egui::Painter,
    rect: Rect,
    pdfs: &[EmbeddedPdf],
    backdrop: Option<PdfPageBackdrop>,
    rotation: u8,
    max_dim: u32,
) -> bool {
    let Some(backdrop) = backdrop else {
        return false;
    };
    let Some(pdf) = pdfs.get(backdrop.pdf_index) else {
        return false;
    };
    let Some(tex_id) = resolve_backdrop_texture(ctx, pdf, backdrop.page_index, max_dim) else {
        return false;
    };
    paint_rotated_texture(p, rect, tex_id, rotation);
    true
}

/// Paints the active document's PDF or custom image backdrop texture into `rect` with
/// an extra clockwise `quarter_turns` offset (used by modal previews).
pub(crate) fn paint_doc_pdf_backdrop_with_rotation(
    ctx: &egui::Context,
    p: &egui::Painter,
    rect: Rect,
    doc: &Document,
    max_dim: u32,
    extra_quarter_turns: u8,
) -> bool {
    let rotation = doc.backdrop_rotation.wrapping_add(extra_quarter_turns) % 4;
    if paint_backdrop(
        ctx,
        p,
        rect,
        &doc.pdf_documents,
        doc.pdf_backdrop,
        rotation,
        max_dim,
    ) {
        return true;
    }
    if let Some(img_idx) = doc.custom_image_backdrop
        && let Some(tpl) = doc.image_templates.get(img_idx)
        && let Some(tex_id) = resolve_custom_image_texture(ctx, tpl)
    {
        paint_rotated_texture(p, rect, tex_id, rotation);
        return true;
    }
    false
}

/// Paints a saved [`CustomPaperTemplate`] preview into `rect` rotated by `rotation` quarter turns.
pub(crate) fn paint_custom_template_preview_rotated(
    ctx: &egui::Context,
    p: &egui::Painter,
    rect: Rect,
    tpl: &CustomPaperTemplate,
    max_dim: u32,
    rotation: u8,
) -> bool {
    let rot = rotation % 4;
    match tpl.kind {
        CustomTemplateKind::Image => {
            let tpl_img = EmbeddedImageTemplate {
                name: tpl.name.clone(),
                png_bytes: Arc::clone(&tpl.bytes),
            };
            if let Some(tex_id) = resolve_custom_image_texture(ctx, &tpl_img) {
                paint_rotated_texture(p, rect, tex_id, rot);
                return true;
            }
            false
        }
        CustomTemplateKind::Pdf => {
            let pdf = EmbeddedPdf {
                name: tpl.name.clone(),
                bytes: Arc::clone(&tpl.bytes),
            };
            if let Some(tex_id) = resolve_backdrop_texture(ctx, &pdf, tpl.pdf_page_index, max_dim) {
                paint_rotated_texture(p, rect, tex_id, rot);
                return true;
            }
            false
        }
    }
}

/// Paints the active document's PDF or custom image backdrop texture into `rect` when present.
pub(crate) fn paint_doc_pdf_backdrop_if_present(
    ctx: &egui::Context,
    p: &egui::Painter,
    rect: Rect,
    doc: &Document,
    max_dim: u32,
) -> bool {
    paint_doc_pdf_backdrop_with_rotation(ctx, p, rect, doc, max_dim, 0)
}

/// Paints a specific `NotebookPage`'s PDF or custom image backdrop texture into `rect` when present.
pub(crate) fn paint_page_pdf_backdrop_if_present(
    ctx: &egui::Context,
    p: &egui::Painter,
    rect: Rect,
    doc: &Document,
    page: &NotebookPage,
    max_dim: u32,
) -> bool {
    if paint_backdrop(
        ctx,
        p,
        rect,
        &doc.pdf_documents,
        page.pdf_backdrop,
        page.backdrop_rotation,
        max_dim,
    ) {
        return true;
    }
    if let Some(img_idx) = page.custom_image_backdrop
        && let Some(tpl) = doc.image_templates.get(img_idx)
        && let Some(tex_id) = resolve_custom_image_texture(ctx, tpl)
    {
        paint_rotated_texture(p, rect, tex_id, page.backdrop_rotation);
        return true;
    }
    false
}

/// Renders a dismissible floating canvas error banner; returns `true` when dismissed.
pub(crate) fn render_floating_error_banner(
    ctx: &egui::Context,
    avail: Rect,
    id_salt: &str,
    message: &str,
    t: &Tokens,
) -> bool {
    let mut dismiss = false;
    let max_w = (avail.width() - 32.0).clamp(180.0, 520.0);
    egui::Area::new(egui::Id::unique(id_salt))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::CENTER_TOP)
        .fixed_pos(pos2(avail.center().x, avail.top() + 12.0))
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(t.danger_bg)
                .stroke(Stroke::new(1.0, t.danger_border))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_max_width(max_w);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let (icon_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        paint_icon(ui.painter(), icon_rect, Icon::AlertCircle, t.danger_fg);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(message).size(12.0).color(t.danger_fg),
                            )
                            .wrap(),
                        );
                        let (close_rect, close_resp) =
                            ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
                        if close_resp.hovered() {
                            ui.painter().rect_filled(
                                close_rect,
                                CornerRadius::same(4),
                                t.danger_border.gamma_multiply(0.35),
                            );
                        }
                        paint_icon(
                            ui.painter(),
                            close_rect.shrink(3.0),
                            Icon::Close,
                            t.danger_fg,
                        );
                        if close_resp.on_hover_text("Dismiss error").clicked() {
                            dismiss = true;
                        }
                    });
                });
        });
    dismiss
}
