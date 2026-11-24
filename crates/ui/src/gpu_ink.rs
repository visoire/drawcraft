//! GPU-resident chunked stroke buffers and offscreen layer cache via `egui_wgpu`.
//!
//! Committed strokes are tessellated once in zoom-invariant page coordinates
//! (`ScalableRibbonMesh`), batched into chunked GPU vertex/index buffers, and cached
//! into two viewport-sized offscreen `Rgba8Unorm` layers (`highlighter` and `ink`).
//! Idle, live-drawing, hover, commit, and pan frames avoid re-tessellating or
//! re-uploading existing strokes to the GPU.

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;
use std::sync::Arc;

use drawcraft_doc::{InkStroke, PaperColor, PenKind, StrokeId, UiTheme};
use drawcraft_geom::Affine2;
use drawcraft_paint::{
    ScalableRibbonMesh, build_stroke_ribbon_scalable, evaluate_spine, graphite_grain_factor,
};
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu::{self, util::DeviceExt};
use egui::{Color32, Rect};

use crate::DrawCraftApp;

const MAX_CHUNK_STROKES: usize = 64;
const MAX_CHUNK_VERTS: usize = 48_000;
const PROXY_ZOOM_TRI_THRESHOLD: usize = 150_000;
/// Seconds without zoom input before proxy-scaled layers are rebaked.
const ZOOM_SETTLE_S: f64 = 0.06;
const VERTEX_STRIDE: u64 = 20;

const STROKE_WGSL: &str = r#"
struct StrokeUniforms {
    scale_feather_tex: vec4<f32>,
    pan_px: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u_stroke: StrokeUniforms;

struct StrokeIn {
    @location(0) pos: vec2<f32>,
    @location(1) aa_offset: vec2<f32>,
    @location(2) color: u32,
};

struct StrokeOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
};

fn unpack_color(color: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(color & 255u),
        f32((color >> 8u) & 255u),
        f32((color >> 16u) & 255u),
        f32((color >> 24u) & 255u),
    ) / 255.0;
}

fn linear_from_gamma_rgb(srgb: vec3<f32>) -> vec3<f32> {
    let cutoff = srgb < vec3<f32>(0.04045);
    let lower = srgb / vec3<f32>(12.92);
    let higher = pow((srgb + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

@vertex
fn vs_stroke(in: StrokeIn) -> StrokeOut {
    let page_pt = in.pos + in.aa_offset * u_stroke.scale_feather_tex.y;
    let tex_px = page_pt * u_stroke.scale_feather_tex.x + u_stroke.pan_px.xy;
    let ndc = vec2<f32>(
        2.0 * tex_px.x / u_stroke.scale_feather_tex.z - 1.0,
        1.0 - 2.0 * tex_px.y / u_stroke.scale_feather_tex.w,
    );
    var out: StrokeOut;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.color = unpack_color(in.color);
    return out;
}

@fragment
fn fs_stroke_gamma(in: StrokeOut) -> @location(0) vec4<f32> {
    return in.color;
}

@fragment
fn fs_stroke_linear(in: StrokeOut) -> @location(0) vec4<f32> {
    return vec4<f32>(linear_from_gamma_rgb(in.color.rgb), in.color.a);
}
"#;

const QUAD_WGSL: &str = r#"
struct QuadUniforms {
    uv_scale_offset: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u_quad: QuadUniforms;
@group(1) @binding(0) var t_layer: texture_2d<f32>;
@group(1) @binding(1) var s_layer: sampler;

struct QuadIn {
    @location(0) pos_ndc: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct QuadOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

fn linear_from_gamma_rgb(srgb: vec3<f32>) -> vec3<f32> {
    let cutoff = srgb < vec3<f32>(0.04045);
    let lower = srgb / vec3<f32>(12.92);
    let higher = pow((srgb + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

@vertex
fn vs_quad(in: QuadIn) -> QuadOut {
    var out: QuadOut;
    out.position = vec4<f32>(in.pos_ndc, 0.0, 1.0);
    out.uv = in.uv * u_quad.uv_scale_offset.xy + u_quad.uv_scale_offset.zw;
    return out;
}

@fragment
fn fs_quad_gamma(in: QuadOut) -> @location(0) vec4<f32> {
    let c = textureSample(t_layer, s_layer, in.uv);
    let inside = in.uv.x >= 0.0 && in.uv.x <= 1.0 && in.uv.y >= 0.0 && in.uv.y <= 1.0;
    return select(vec4<f32>(0.0, 0.0, 0.0, 0.0), c, inside);
}

@fragment
fn fs_quad_linear(in: QuadOut) -> @location(0) vec4<f32> {
    let c = textureSample(t_layer, s_layer, in.uv);
    let inside = in.uv.x >= 0.0 && in.uv.x <= 1.0 && in.uv.y >= 0.0 && in.uv.y <= 1.0;
    let linear_c = vec4<f32>(linear_from_gamma_rgb(c.rgb), c.a);
    return select(vec4<f32>(0.0, 0.0, 0.0, 0.0), linear_c, inside);
}
"#;

#[derive(Clone, Debug)]
pub(crate) struct CachedScalableStroke {
    source: Arc<InkStroke>,
    color_key: (UiTheme, bool, PaperColor),
    bounds: drawcraft_geom::Rect,
    mesh: Arc<ScalableRibbonMesh>,
    packed_colors: Arc<Vec<u32>>,
}

#[derive(Clone, Debug)]
struct GpuChunkData {
    id: u64,
    parent_id: Option<u64>,
    bounds: drawcraft_geom::Rect,
    strokes: Vec<Arc<InkStroke>>,
    vertex_count: usize,
    index_count: u32,
    stroke_ranges: Vec<(drawcraft_geom::Rect, u32, u32)>,
    last_stroke_index_range: (u32, u32),
    vertex_bytes: Arc<Vec<u8>>,
    index_bytes: Arc<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub(crate) struct GpuByteMesh {
    vertex_bytes: Arc<Vec<u8>>,
    index_bytes: Arc<Vec<u8>>,
    index_count: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct GpuOverlayHighlighters {
    vertex_bytes: Arc<Vec<u8>>,
    index_bytes: Arc<Vec<u8>>,
    ranges: Vec<(drawcraft_geom::Rect, u32, u32)>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct GpuStrokeSceneCache {
    page_key: (usize, usize),
    color_key: Option<(UiTheme, bool, PaperColor)>,
    next_chunk_id: u64,
    strokes: HashMap<StrokeId, CachedScalableStroke>,
    hl_chunks: Vec<Arc<GpuChunkData>>,
    ink_chunks: Vec<Arc<GpuChunkData>>,
    overlay_hl_ids: Vec<StrokeId>,
    last_sync_sig: Option<(usize, u64, usize, Option<StrokeId>, usize)>,
    pending_zoom_rebake_since_s: Option<f64>,
    last_zoom_bits: u32,
}

impl GpuStrokeSceneCache {
    pub(crate) fn clear(&mut self) {
        self.strokes.clear();
        self.hl_chunks.clear();
        self.ink_chunks.clear();
        self.overlay_hl_ids.clear();
        self.last_sync_sig = None;
        self.pending_zoom_rebake_since_s = None;
    }

    fn alloc_chunk_id(&mut self) -> u64 {
        self.next_chunk_id = self.next_chunk_id.saturating_add(1);
        self.next_chunk_id
    }
}

struct GpuChunkBuffers {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

struct LayerTarget {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    linear_bg: wgpu::BindGroup,
    nearest_bg: wgpu::BindGroup,
}

#[derive(Default)]
struct BakedLayerState {
    valid: bool,
    page_key: (usize, usize),
    color_key: (UiTheme, bool, PaperColor),
    tex_size: [u32; 2],
    zoom: f32,
    scale_px: f32,
    pan_px: [f32; 2],
    ppp: f32,
    has_hl: bool,
    has_ink: bool,
    hl_chunk_ids: Vec<u64>,
    ink_chunk_ids: Vec<u64>,
}

struct ScratchBuffer {
    buffer: Option<wgpu::Buffer>,
    capacity: u64,
}

impl ScratchBuffer {
    fn new() -> Self {
        Self {
            buffer: None,
            capacity: 0,
        }
    }

    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        usage: wgpu::BufferUsages,
        bytes: &[u8],
    ) {
        if bytes.is_empty() {
            return;
        }
        let needed = bytes.len() as u64;
        if self.buffer.is_none() || self.capacity < needed {
            let next_cap = needed.next_power_of_two().max(4096);
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: next_cap,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.capacity = next_cap;
        }
        if let Some(buf) = &self.buffer {
            queue.write_buffer(buf, 0, bytes);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OverlayHlTarget {
    #[default]
    None,
    Scratch,
    OverlayLayer,
}

pub(crate) struct GpuInkResources {
    stroke_offscreen_pipeline: wgpu::RenderPipeline,
    stroke_max_pipeline: wgpu::RenderPipeline,
    stroke_overlay_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    composite_offscreen_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    tex_bind_group_layout: wgpu::BindGroupLayout,
    quad_vb: wgpu::Buffer,
    quad_ib: wgpu::Buffer,
    linear_sampler: wgpu::Sampler,
    nearest_sampler: wgpu::Sampler,
    stroke_uniform_buf: wgpu::Buffer,
    stroke_uniform_bg: wgpu::BindGroup,
    overlay_uniform_buf: wgpu::Buffer,
    overlay_uniform_bg: wgpu::BindGroup,
    blit_uniform_buf: wgpu::Buffer,
    blit_uniform_bg: wgpu::BindGroup,
    identity_quad_uniform_bg: wgpu::BindGroup,
    overlay_comp_uniform_buf: wgpu::Buffer,
    overlay_comp_uniform_bg: wgpu::BindGroup,
    comp_hl_uniform_buf: wgpu::Buffer,
    comp_hl_uniform_bg: wgpu::BindGroup,
    comp_ink_uniform_buf: wgpu::Buffer,
    comp_ink_uniform_bg: wgpu::BindGroup,
    chunk_buffers: HashMap<u64, GpuChunkBuffers>,
    overlay_hl_vb: ScratchBuffer,
    overlay_hl_ib: ScratchBuffer,
    overlay_ink_vb: ScratchBuffer,
    overlay_ink_ib: ScratchBuffer,
    hl_layers: [Option<LayerTarget>; 2],
    hl_scratch_layer: Option<LayerTarget>,
    hl_overlay_layer: Option<LayerTarget>,
    ink_layers: [Option<LayerTarget>; 2],
    hl_active: usize,
    ink_active: usize,
    overlay_hl_target: OverlayHlTarget,
    baked: BakedLayerState,
}

fn make_uniform_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    label: &str,
    size: u64,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buf.as_entire_binding(),
        }],
    });
    (buf, bg)
}

fn premul_blend() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::OneMinusDstAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

fn max_blend() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Max,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Max,
        },
    }
}

fn create_stroke_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    fs_entry: &str,
    blend: wgpu::BlendState,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_stroke"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: VERTEX_STRIDE,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Uint32],
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fs_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn create_quad_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    fs_entry: &str,
    blend: wgpu::BlendState,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs_quad"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 16,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(fs_entry),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn create_quad_geometry(device: &wgpu::Device) -> (wgpu::Buffer, wgpu::Buffer) {
    let quad_verts: [f32; 16] = [
        -1.0, 1.0, 0.0, 0.0, // top-left
        1.0, 1.0, 1.0, 0.0, // top-right
        1.0, -1.0, 1.0, 1.0, // bottom-right
        -1.0, -1.0, 0.0, 1.0, // bottom-left
    ];
    let mut quad_v_bytes = Vec::with_capacity(64);
    for f in quad_verts {
        quad_v_bytes.extend_from_slice(&f.to_le_bytes());
    }
    let quad_indices: [u32; 6] = [0, 1, 2, 0, 2, 3];
    let mut quad_i_bytes = Vec::with_capacity(24);
    for idx in quad_indices {
        quad_i_bytes.extend_from_slice(&idx.to_le_bytes());
    }
    let quad_vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("drawcraft_ink_quad_vb"),
        contents: &quad_v_bytes,
        usage: wgpu::BufferUsages::VERTEX,
    });
    let quad_ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("drawcraft_ink_quad_ib"),
        contents: &quad_i_bytes,
        usage: wgpu::BufferUsages::INDEX,
    });
    (quad_vb, quad_ib)
}

fn uniform_buffer_layout(
    min_bytes: u64,
    label: &str,
    device: &wgpu::Device,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(min_bytes),
            },
            count: None,
        }],
    })
}

impl GpuInkResources {
    pub(crate) fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let stroke_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("drawcraft_ink_stroke_shader"),
            source: wgpu::ShaderSource::Wgsl(STROKE_WGSL.into()),
        });
        let quad_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("drawcraft_ink_quad_shader"),
            source: wgpu::ShaderSource::Wgsl(QUAD_WGSL.into()),
        });

        let stroke_uniform_layout =
            uniform_buffer_layout(32, "drawcraft_stroke_uniform_layout", device);
        let quad_uniform_layout =
            uniform_buffer_layout(16, "drawcraft_quad_uniform_layout", device);
        let tex_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("drawcraft_layer_tex_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let stroke_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("drawcraft_stroke_pipeline_layout"),
                bind_group_layouts: &[Some(&stroke_uniform_layout)],
                immediate_size: 0,
            });
        let quad_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("drawcraft_quad_pipeline_layout"),
            bind_group_layouts: &[Some(&quad_uniform_layout), Some(&tex_bind_group_layout)],
            immediate_size: 0,
        });

        let target_fs_stroke = if target_format.is_srgb() {
            "fs_stroke_linear"
        } else {
            "fs_stroke_gamma"
        };
        let target_fs_quad = if target_format.is_srgb() {
            "fs_quad_linear"
        } else {
            "fs_quad_gamma"
        };

        let stroke_offscreen_pipeline = create_stroke_pipeline(
            device,
            &stroke_pipeline_layout,
            &stroke_module,
            wgpu::TextureFormat::Rgba8Unorm,
            "fs_stroke_gamma",
            premul_blend(),
            "drawcraft_stroke_offscreen_pipeline",
        );
        let stroke_max_pipeline = create_stroke_pipeline(
            device,
            &stroke_pipeline_layout,
            &stroke_module,
            wgpu::TextureFormat::Rgba8Unorm,
            "fs_stroke_gamma",
            max_blend(),
            "drawcraft_stroke_max_pipeline",
        );
        let stroke_overlay_pipeline = create_stroke_pipeline(
            device,
            &stroke_pipeline_layout,
            &stroke_module,
            target_format,
            target_fs_stroke,
            premul_blend(),
            "drawcraft_stroke_overlay_pipeline",
        );
        let blit_pipeline = create_quad_pipeline(
            device,
            &quad_pipeline_layout,
            &quad_module,
            wgpu::TextureFormat::Rgba8Unorm,
            "fs_quad_gamma",
            wgpu::BlendState::REPLACE,
            "drawcraft_ink_blit_pipeline",
        );
        let composite_offscreen_pipeline = create_quad_pipeline(
            device,
            &quad_pipeline_layout,
            &quad_module,
            wgpu::TextureFormat::Rgba8Unorm,
            "fs_quad_gamma",
            premul_blend(),
            "drawcraft_ink_composite_offscreen_pipeline",
        );
        let composite_pipeline = create_quad_pipeline(
            device,
            &quad_pipeline_layout,
            &quad_module,
            target_format,
            target_fs_quad,
            premul_blend(),
            "drawcraft_ink_composite_pipeline",
        );

        let (quad_vb, quad_ib) = create_quad_geometry(device);
        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("drawcraft_ink_linear_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("drawcraft_ink_nearest_sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let (stroke_uniform_buf, stroke_uniform_bg) =
            make_uniform_bg(device, &stroke_uniform_layout, "drawcraft_stroke_u", 32);
        let (overlay_uniform_buf, overlay_uniform_bg) =
            make_uniform_bg(device, &stroke_uniform_layout, "drawcraft_overlay_u", 32);
        let (blit_uniform_buf, blit_uniform_bg) =
            make_uniform_bg(device, &quad_uniform_layout, "drawcraft_blit_u", 16);
        let mut identity_bytes = [0_u8; 16];
        for (i, val) in [1.0_f32, 1.0_f32, 0.0_f32, 0.0_f32].iter().enumerate() {
            identity_bytes[i * 4..(i + 1) * 4].copy_from_slice(&val.to_le_bytes());
        }
        let identity_quad_uniform_buf =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("drawcraft_identity_quad_u"),
                contents: &identity_bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let identity_quad_uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("drawcraft_identity_quad_u"),
            layout: &quad_uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: identity_quad_uniform_buf.as_entire_binding(),
            }],
        });
        let (overlay_comp_uniform_buf, overlay_comp_uniform_bg) =
            make_uniform_bg(device, &quad_uniform_layout, "drawcraft_overlay_comp_u", 16);
        let (comp_hl_uniform_buf, comp_hl_uniform_bg) =
            make_uniform_bg(device, &quad_uniform_layout, "drawcraft_comp_hl_u", 16);
        let (comp_ink_uniform_buf, comp_ink_uniform_bg) =
            make_uniform_bg(device, &quad_uniform_layout, "drawcraft_comp_ink_u", 16);

        Self {
            stroke_offscreen_pipeline,
            stroke_max_pipeline,
            stroke_overlay_pipeline,
            blit_pipeline,
            composite_offscreen_pipeline,
            composite_pipeline,
            tex_bind_group_layout,
            quad_vb,
            quad_ib,
            linear_sampler,
            nearest_sampler,
            stroke_uniform_buf,
            stroke_uniform_bg,
            overlay_uniform_buf,
            overlay_uniform_bg,
            blit_uniform_buf,
            blit_uniform_bg,
            identity_quad_uniform_bg,
            overlay_comp_uniform_buf,
            overlay_comp_uniform_bg,
            comp_hl_uniform_buf,
            comp_hl_uniform_bg,
            comp_ink_uniform_buf,
            comp_ink_uniform_bg,
            chunk_buffers: HashMap::new(),
            overlay_hl_vb: ScratchBuffer::new(),
            overlay_hl_ib: ScratchBuffer::new(),
            overlay_ink_vb: ScratchBuffer::new(),
            overlay_ink_ib: ScratchBuffer::new(),
            hl_layers: [None, None],
            hl_scratch_layer: None,
            hl_overlay_layer: None,
            ink_layers: [None, None],
            hl_active: 0,
            ink_active: 0,
            overlay_hl_target: OverlayHlTarget::None,
            baked: BakedLayerState::default(),
        }
    }

    fn create_layer_target(
        &self,
        device: &wgpu::Device,
        size: [u32; 2],
        label: &str,
    ) -> LayerTarget {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let linear_bg = self.create_layer_bind_group(device, &view, &self.linear_sampler, label);
        let nearest_bg = self.create_layer_bind_group(device, &view, &self.nearest_sampler, label);
        LayerTarget {
            _texture: texture,
            view,
            linear_bg,
            nearest_bg,
        }
    }

    fn create_layer_bind_group(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        label: &str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.tex_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    fn sync_chunk_buffers(
        &mut self,
        device: &wgpu::Device,
        hl_chunks: &[Arc<GpuChunkData>],
        ink_chunks: &[Arc<GpuChunkData>],
    ) {
        let mut active_ids = HashSet::with_capacity(hl_chunks.len() + ink_chunks.len());
        for chunk in hl_chunks.iter().chain(ink_chunks.iter()) {
            active_ids.insert(chunk.id);
            self.chunk_buffers.entry(chunk.id).or_insert_with(|| {
                let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("drawcraft_chunk_vb"),
                    contents: &chunk.vertex_bytes,
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("drawcraft_chunk_ib"),
                    contents: &chunk.index_bytes,
                    usage: wgpu::BufferUsages::INDEX,
                });
                GpuChunkBuffers {
                    vertex_buffer,
                    index_buffer,
                    index_count: chunk.index_count,
                }
            });
        }
        self.chunk_buffers.retain(|id, _| active_ids.contains(id));
    }

    fn upload_overlays(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        overlay_hl: Option<&GpuOverlayHighlighters>,
        overlay_ink: Option<&GpuByteMesh>,
    ) {
        if let Some(hl) = overlay_hl {
            self.overlay_hl_vb.upload(
                device,
                queue,
                "drawcraft_overlay_hl_vb",
                wgpu::BufferUsages::VERTEX,
                &hl.vertex_bytes,
            );
            self.overlay_hl_ib.upload(
                device,
                queue,
                "drawcraft_overlay_hl_ib",
                wgpu::BufferUsages::INDEX,
                &hl.index_bytes,
            );
        }
        if let Some(ink) = overlay_ink {
            self.overlay_ink_vb.upload(
                device,
                queue,
                "drawcraft_overlay_ink_vb",
                wgpu::BufferUsages::VERTEX,
                &ink.vertex_bytes,
            );
            self.overlay_ink_ib.upload(
                device,
                queue,
                "drawcraft_overlay_ink_ib",
                wgpu::BufferUsages::INDEX,
                &ink.index_bytes,
            );
        }
    }
}

fn compute_scalable_mesh_bounds(
    mesh: &ScalableRibbonMesh,
    fallback: drawcraft_geom::Rect,
) -> drawcraft_geom::Rect {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    for v in &mesh.vertices {
        let x = f64::from(v.pos[0]);
        let y = f64::from(v.pos[1]);
        if x.is_finite() && y.is_finite() {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    if min_x.is_finite() && min_y.is_finite() && max_x.is_finite() && max_y.is_finite() {
        drawcraft_geom::Rect::new(
            min_x - 2.0,
            min_y - 2.0,
            (max_x - min_x) + 4.0,
            (max_y - min_y) + 4.0,
        )
    } else {
        fallback
    }
}

fn write_uniform_f32<const N: usize>(queue: &wgpu::Queue, buf: &wgpu::Buffer, vals: [f32; N]) {
    let mut bytes = [0u8; 32];
    for (i, v) in vals.iter().enumerate() {
        let off = i * 4;
        if off + 4 <= bytes.len() {
            bytes[off..off + 4].copy_from_slice(&v.to_le_bytes());
        }
    }
    let slice_len = (N * 4).min(bytes.len());
    queue.write_buffer(buf, 0, &bytes[..slice_len]);
}

fn pack_rgba_premul(rgba: [f32; 4], grain: f32) -> u32 {
    let c = Color32::from_rgba_unmultiplied(
        (rgba[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgba[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        ((rgba[3] * grain).clamp(0.0, 1.0) * 255.0).round() as u8,
    );
    u32::from_le_bytes(c.to_array())
}

fn pack_mesh_colors(mesh: &ScalableRibbonMesh, is_pencil: bool) -> Vec<u32> {
    let mut packed_colors = Vec::with_capacity(mesh.vertices.len());
    for v in &mesh.vertices {
        let grain = if is_pencil {
            0.58 + 0.42 * graphite_grain_factor(v.pos[0], v.pos[1])
        } else {
            1.0
        };
        packed_colors.push(pack_rgba_premul(v.rgba, grain));
    }
    packed_colors
}

fn build_cached_scalable_stroke(
    stroke: &Arc<InkStroke>,
    color_key: (UiTheme, bool, PaperColor),
) -> CachedScalableStroke {
    let (theme, auto_contrast, paper_color) = color_key;
    let is_hl = stroke.pen.kind == PenKind::Highlighter;
    let mut render_pen = stroke.pen.clone();
    render_pen.color = paper_color.resolve_ink_color(theme, auto_contrast, stroke.pen.color, is_hl);
    let nodes = evaluate_spine(&render_pen, &stroke.smoothed_points);
    let mesh = build_stroke_ribbon_scalable(&render_pen, &nodes);
    let packed_colors = pack_mesh_colors(&mesh, render_pen.kind == PenKind::Pencil);
    let bounds = compute_scalable_mesh_bounds(&mesh, stroke.bounds);
    CachedScalableStroke {
        source: Arc::clone(stroke),
        color_key,
        bounds,
        mesh: Arc::new(mesh),
        packed_colors: Arc::new(packed_colors),
    }
}

fn append_mesh_parts_to_bytes(
    mesh: &ScalableRibbonMesh,
    packed_colors: &[u32],
    vertex_bytes: &mut Vec<u8>,
    index_bytes: &mut Vec<u8>,
    base_vertex: u32,
) {
    vertex_bytes.reserve(mesh.vertices.len() * 20);
    for (v, &col) in mesh.vertices.iter().zip(packed_colors.iter()) {
        vertex_bytes.extend_from_slice(&v.pos[0].to_le_bytes());
        vertex_bytes.extend_from_slice(&v.pos[1].to_le_bytes());
        vertex_bytes.extend_from_slice(&v.aa_offset[0].to_le_bytes());
        vertex_bytes.extend_from_slice(&v.aa_offset[1].to_le_bytes());
        vertex_bytes.extend_from_slice(&col.to_le_bytes());
    }
    index_bytes.reserve(mesh.indices.len() * 4);
    for &idx in &mesh.indices {
        index_bytes.extend_from_slice(&(base_vertex + idx).to_le_bytes());
    }
}

fn append_scalable_to_bytes(
    entry: &CachedScalableStroke,
    vertex_bytes: &mut Vec<u8>,
    index_bytes: &mut Vec<u8>,
    base_vertex: u32,
) {
    append_mesh_parts_to_bytes(
        &entry.mesh,
        &entry.packed_colors,
        vertex_bytes,
        index_bytes,
        base_vertex,
    );
}

fn build_chunk_from_entries(
    id: u64,
    parent_id: Option<u64>,
    entries: &[&CachedScalableStroke],
) -> Arc<GpuChunkData> {
    let mut total_verts = 0_usize;
    let mut total_indices = 0_usize;
    let mut bounds = drawcraft_geom::Rect::EMPTY;
    for entry in entries {
        total_verts += entry.mesh.vertices.len();
        total_indices += entry.mesh.indices.len();
        if !entry.bounds.is_empty() {
            bounds = bounds.union(entry.bounds);
        }
    }
    let mut vertex_bytes = Vec::with_capacity(total_verts * 20);
    let mut index_bytes = Vec::with_capacity(total_indices * 4);
    let mut strokes = Vec::with_capacity(entries.len());
    let mut stroke_ranges = Vec::with_capacity(entries.len());
    let mut base_v = 0_u32;
    let mut last_range = (0_u32, 0_u32);
    for (i, entry) in entries.iter().enumerate() {
        let start_idx = (index_bytes.len() / 4) as u32;
        append_scalable_to_bytes(entry, &mut vertex_bytes, &mut index_bytes, base_v);
        let idx_len = entry.mesh.indices.len() as u32;
        stroke_ranges.push((entry.bounds, start_idx, idx_len));
        if i + 1 == entries.len() {
            last_range = (start_idx, idx_len);
        }
        base_v = base_v.saturating_add(entry.mesh.vertices.len() as u32);
        strokes.push(Arc::clone(&entry.source));
    }
    Arc::new(GpuChunkData {
        id,
        parent_id,
        bounds,
        strokes,
        vertex_count: total_verts,
        index_count: total_indices as u32,
        stroke_ranges,
        last_stroke_index_range: last_range,
        vertex_bytes: Arc::new(vertex_bytes),
        index_bytes: Arc::new(index_bytes),
    })
}

fn try_append_tail_chunk(
    cache: &mut GpuStrokeSceneCache,
    old_tail: &Arc<GpuChunkData>,
    tail_ids: &[StrokeId],
) -> Option<Arc<GpuChunkData>> {
    let tail_len = old_tail.strokes.len();
    if tail_ids.len() != tail_len + 1 || tail_len >= MAX_CHUNK_STROKES {
        return None;
    }
    let added = cache.strokes.get(&tail_ids[tail_len])?;
    if old_tail.vertex_count + added.mesh.vertices.len() > MAX_CHUNK_VERTS {
        return None;
    }
    let prefix_matches = old_tail.strokes.iter().enumerate().all(|(i, old_arc)| {
        cache
            .strokes
            .get(&tail_ids[i])
            .is_some_and(|e| Arc::ptr_eq(&e.source, old_arc))
    });
    if !prefix_matches {
        return None;
    }
    let id = cache.alloc_chunk_id();
    let entries: Vec<&CachedScalableStroke> = tail_ids
        .iter()
        .filter_map(|sid| cache.strokes.get(sid))
        .collect();
    Some(build_chunk_from_entries(id, Some(old_tail.id), &entries))
}

fn sync_chunk_list(cache: &mut GpuStrokeSceneCache, is_hl: bool, ordered_ids: &[StrokeId]) {
    let old_chunks = if is_hl {
        std::mem::take(&mut cache.hl_chunks)
    } else {
        std::mem::take(&mut cache.ink_chunks)
    };

    let mut new_chunks: Vec<Arc<GpuChunkData>> = Vec::with_capacity(old_chunks.len().max(1));
    let mut pos = 0_usize;
    let total = ordered_ids.len();

    for (idx, old) in old_chunks.iter().enumerate() {
        let len = old.strokes.len();
        if len == 0 || pos + len > total {
            break;
        }
        let try_coalesce_last =
            idx + 1 == old_chunks.len() && total == pos + len + 1 && len < MAX_CHUNK_STROKES;
        if try_coalesce_last {
            break;
        }
        let matches = old.strokes.iter().enumerate().all(|(i, old_arc)| {
            cache
                .strokes
                .get(&ordered_ids[pos + i])
                .is_some_and(|e| Arc::ptr_eq(&e.source, old_arc))
        });
        if matches {
            new_chunks.push(Arc::clone(old));
            pos += len;
        } else {
            break;
        }
    }

    if pos < total && new_chunks.len() + 1 == old_chunks.len() {
        let old_tail = &old_chunks[new_chunks.len()];
        if let Some(tail_chunk) = try_append_tail_chunk(cache, old_tail, &ordered_ids[pos..]) {
            new_chunks.push(tail_chunk);
            pos = total;
        } else if pos + old_tail.strokes.len() <= total {
            let matches_old = old_tail.strokes.iter().enumerate().all(|(i, old_arc)| {
                cache
                    .strokes
                    .get(&ordered_ids[pos + i])
                    .is_some_and(|e| Arc::ptr_eq(&e.source, old_arc))
            });
            if matches_old {
                new_chunks.push(Arc::clone(old_tail));
                pos += old_tail.strokes.len();
            }
        }
    }

    let mut next_chunk_id = cache.next_chunk_id;
    while pos < total {
        let mut entries: Vec<&CachedScalableStroke> = Vec::with_capacity(MAX_CHUNK_STROKES);
        let mut vert_sum = 0_usize;
        while pos < total && entries.len() < MAX_CHUNK_STROKES {
            let Some(e) = cache.strokes.get(&ordered_ids[pos]) else {
                pos += 1;
                continue;
            };
            if !entries.is_empty() && vert_sum + e.mesh.vertices.len() > MAX_CHUNK_VERTS {
                break;
            }
            vert_sum += e.mesh.vertices.len();
            entries.push(e);
            pos += 1;
        }
        if !entries.is_empty() {
            next_chunk_id = next_chunk_id.saturating_add(1);
            new_chunks.push(build_chunk_from_entries(next_chunk_id, None, &entries));
        }
    }
    cache.next_chunk_id = next_chunk_id;

    if is_hl {
        cache.hl_chunks = new_chunks;
    } else {
        cache.ink_chunks = new_chunks;
    }
}

fn build_overlay_highlighters(
    cache: &GpuStrokeSceneCache,
    ids: &[StrokeId],
    live_hl: Option<(&ScalableRibbonMesh, &[u32], drawcraft_geom::Rect)>,
) -> Option<Arc<GpuOverlayHighlighters>> {
    let mut total_v = 0_usize;
    let mut total_i = 0_usize;
    for &id in ids {
        if let Some(e) = cache.strokes.get(&id) {
            total_v += e.mesh.vertices.len();
            total_i += e.mesh.indices.len();
        }
    }
    if let Some((mesh, _, _)) = live_hl {
        total_v += mesh.vertices.len();
        total_i += mesh.indices.len();
    }
    if total_i == 0 {
        return None;
    }
    let mut vertex_bytes = Vec::with_capacity(total_v * 20);
    let mut index_bytes = Vec::with_capacity(total_i * 4);
    let mut ranges = Vec::with_capacity(ids.len() + usize::from(live_hl.is_some()));
    let mut base_v = 0_u32;
    for &id in ids {
        if let Some(e) = cache.strokes.get(&id)
            && !e.mesh.indices.is_empty()
        {
            let start_idx = (index_bytes.len() / 4) as u32;
            append_scalable_to_bytes(e, &mut vertex_bytes, &mut index_bytes, base_v);
            let idx_len = e.mesh.indices.len() as u32;
            ranges.push((e.bounds, start_idx, idx_len));
            base_v = base_v.saturating_add(e.mesh.vertices.len() as u32);
        }
    }
    if let Some((mesh, packed_colors, bounds)) = live_hl
        && !mesh.indices.is_empty()
    {
        let start_idx = (index_bytes.len() / 4) as u32;
        append_mesh_parts_to_bytes(
            mesh,
            packed_colors,
            &mut vertex_bytes,
            &mut index_bytes,
            base_v,
        );
        let idx_len = mesh.indices.len() as u32;
        ranges.push((bounds, start_idx, idx_len));
    }
    (!ranges.is_empty()).then(|| {
        Arc::new(GpuOverlayHighlighters {
            vertex_bytes: Arc::new(vertex_bytes),
            index_bytes: Arc::new(index_bytes),
            ranges,
        })
    })
}

fn build_overlay_bytes(cache: &GpuStrokeSceneCache, ids: &[StrokeId]) -> Option<Arc<GpuByteMesh>> {
    if ids.is_empty() {
        return None;
    }
    let mut total_v = 0_usize;
    let mut total_i = 0_usize;
    for &id in ids {
        if let Some(e) = cache.strokes.get(&id) {
            total_v += e.mesh.vertices.len();
            total_i += e.mesh.indices.len();
        }
    }
    if total_i == 0 {
        return None;
    }
    let mut vertex_bytes = Vec::with_capacity(total_v * 20);
    let mut index_bytes = Vec::with_capacity(total_i * 4);
    let mut base_v = 0_u32;
    for &id in ids {
        if let Some(e) = cache.strokes.get(&id) {
            append_scalable_to_bytes(e, &mut vertex_bytes, &mut index_bytes, base_v);
            base_v = base_v.saturating_add(e.mesh.vertices.len() as u32);
        }
    }
    Some(Arc::new(GpuByteMesh {
        vertex_bytes: Arc::new(vertex_bytes),
        index_bytes: Arc::new(index_bytes),
        index_count: total_i as u32,
    }))
}

fn active_drag_stroke_ids(app: &DrawCraftApp) -> HashSet<StrokeId> {
    let dragging = matches!(
        app.view.selection_drag.as_ref().map(|d| &d.mode),
        Some(
            crate::SelectionDragMode::Move { .. }
                | crate::SelectionDragMode::Resize { .. }
                | crate::SelectionDragMode::Rotate { .. }
        )
    );
    if !dragging {
        return HashSet::new();
    }
    let mut ids = HashSet::new();
    for obj in &app.view.selected_objects {
        if let crate::SelectedObject::Stroke(id) = *obj {
            ids.insert(id);
        }
    }
    if ids.is_empty()
        && let Some(crate::SelectedObject::Stroke(id)) = app.view.selected_object
    {
        ids.insert(id);
    }
    ids
}

struct GpuInkPaintCallback {
    avail_rect: Rect,
    page_key: (usize, usize),
    color_key: (UiTheme, bool, PaperColor),
    zoom: f32,
    scale: f64,
    offset_x: f64,
    offset_y: f64,
    /// `true` once the zoom level has been stable long enough to warrant a
    /// full-quality rebake instead of a scaled proxy of the old layers.
    zoom_settled: bool,
    hl_chunks: Vec<Arc<GpuChunkData>>,
    ink_chunks: Vec<Arc<GpuChunkData>>,
    overlay_hl: Option<Arc<GpuOverlayHighlighters>>,
    overlay_ink: Option<Arc<GpuByteMesh>>,
}

fn viewport_px_from_rect(
    rect: Rect,
    screen_descriptor: &ScreenDescriptor,
) -> egui::epaint::ViewportInPixels {
    egui::epaint::ViewportInPixels::from_points(
        &rect,
        screen_descriptor.pixels_per_point,
        screen_descriptor.size_in_pixels,
    )
}

fn page_rect_for_screen_box(
    sx: u32,
    sy: u32,
    sw: u32,
    sh: u32,
    scale_px: f32,
    pan_px: [f32; 2],
    pad_px: f32,
) -> drawcraft_geom::Rect {
    let inv = 1.0 / f64::from(scale_px.max(1e-4));
    let min_x = (f64::from(sx) - f64::from(pad_px) - f64::from(pan_px[0])) * inv;
    let min_y = (f64::from(sy) - f64::from(pad_px) - f64::from(pan_px[1])) * inv;
    let max_x = (f64::from(sx + sw) + f64::from(pad_px) - f64::from(pan_px[0])) * inv;
    let max_y = (f64::from(sy + sh) + f64::from(pad_px) - f64::from(pan_px[1])) * inv;
    drawcraft_geom::Rect {
        x: min_x,
        y: min_y,
        w: (max_x - min_x).max(0.0),
        h: (max_y - min_y).max(0.0),
    }
}

fn count_visible_triangles(
    chunks: &[Arc<GpuChunkData>],
    vis_page_rect: drawcraft_geom::Rect,
) -> usize {
    chunks
        .iter()
        .filter(|c| c.bounds.is_empty() || c.bounds.intersects(vis_page_rect))
        .map(|c| (c.index_count as usize) / 3)
        .sum()
}

fn detect_single_append<'a>(
    baked_ids: &[u64],
    chunks: &'a [Arc<GpuChunkData>],
) -> Option<&'a Arc<GpuChunkData>> {
    if chunks.is_empty() {
        return None;
    }
    if chunks.len() == baked_ids.len() + 1 {
        let prefix_ok = baked_ids.iter().zip(chunks.iter()).all(|(&b, c)| b == c.id);
        let tail = chunks.last()?;
        if prefix_ok && tail.strokes.len() == 1 {
            return Some(tail);
        }
    } else if chunks.len() == baked_ids.len() && !baked_ids.is_empty() {
        let last_idx = chunks.len() - 1;
        let prefix_ok = baked_ids[..last_idx]
            .iter()
            .zip(chunks[..last_idx].iter())
            .all(|(&b, c)| b == c.id);
        let tail = &chunks[last_idx];
        if prefix_ok && tail.parent_id == Some(baked_ids[last_idx]) {
            return Some(tail);
        }
    }
    None
}

struct RenderPassLayerCtx<'a> {
    encoder: &'a mut wgpu::CommandEncoder,
    queue: &'a wgpu::Queue,
    tex_size: [u32; 2],
    scale_px: f32,
    feather_page: f32,
    pan_px: [f32; 2],
}

#[derive(Clone, Copy)]
struct HighlighterSlice<'a> {
    vb: &'a wgpu::Buffer,
    ib: &'a wgpu::Buffer,
    start_idx: u32,
    count_idx: u32,
    scissor: [u32; 4],
}

struct IsolatedLayerTarget<'a> {
    scratch: &'a LayerTarget,
    target_view: &'a wgpu::TextureView,
    stroke_uniform_bg: &'a wgpu::BindGroup,
}

fn page_bounds_to_scissor(
    bounds: drawcraft_geom::Rect,
    scale_px: f32,
    pan_px: [f32; 2],
    pad_px: f32,
    tex_size: [u32; 2],
) -> Option<[u32; 4]> {
    if bounds.is_empty() {
        return Some([0, 0, tex_size[0], tex_size[1]]);
    }
    let scale = f64::from(scale_px.max(1e-4));
    let pad = f64::from(pad_px);
    let x0 = (bounds.x * scale + f64::from(pan_px[0]) - pad).floor();
    let y0 = (bounds.y * scale + f64::from(pan_px[1]) - pad).floor();
    let x1 = (bounds.right() * scale + f64::from(pan_px[0]) + pad).ceil();
    let y1 = (bounds.bottom() * scale + f64::from(pan_px[1]) + pad).ceil();
    let sx = (x0.max(0.0) as u32).min(tex_size[0]);
    let sy = (y0.max(0.0) as u32).min(tex_size[1]);
    let ex = (x1.max(0.0) as u32).min(tex_size[0]);
    let ey = (y1.max(0.0) as u32).min(tex_size[1]);
    (ex > sx && ey > sy).then_some([sx, sy, ex - sx, ey - sy])
}

fn intersect_scissor(a: [u32; 4], b: [u32; 4]) -> Option<[u32; 4]> {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1 - x0, y1 - y0])
}

fn scissors_disjoint(a: [u32; 4], b: [u32; 4]) -> bool {
    a[0] + a[2] <= b[0] || b[0] + b[2] <= a[0] || a[1] + a[3] <= b[1] || b[1] + b[3] <= a[1]
}

fn clear_layer_target(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
    let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("drawcraft_clear_layer"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
}

fn render_single_hl_to_scratch(
    encoder: &mut wgpu::CommandEncoder,
    res: &GpuInkResources,
    scratch: &LayerTarget,
    stroke_uniform_bg: &wgpu::BindGroup,
    tex_size: [u32; 2],
    slice: HighlighterSlice<'_>,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("drawcraft_hl_single_scratch_max"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &scratch.view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_viewport(0.0, 0.0, tex_size[0] as f32, tex_size[1] as f32, 0.0, 1.0);
    pass.set_pipeline(&res.stroke_max_pipeline);
    pass.set_bind_group(0, stroke_uniform_bg, &[]);
    pass.set_scissor_rect(
        slice.scissor[0],
        slice.scissor[1],
        slice.scissor[2],
        slice.scissor[3],
    );
    pass.set_vertex_buffer(0, slice.vb.slice(..));
    pass.set_index_buffer(slice.ib.slice(..), wgpu::IndexFormat::Uint32);
    pass.draw_indexed(slice.start_idx..slice.start_idx + slice.count_idx, 0, 0..1);
}

fn flush_highlighter_batch(
    encoder: &mut wgpu::CommandEncoder,
    res: &GpuInkResources,
    target: &IsolatedLayerTarget<'_>,
    tex_size: [u32; 2],
    batch: &[HighlighterSlice<'_>],
) {
    if batch.is_empty() {
        return;
    }
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("drawcraft_hl_scratch_max"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.scratch.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_viewport(0.0, 0.0, tex_size[0] as f32, tex_size[1] as f32, 0.0, 1.0);
        pass.set_pipeline(&res.stroke_max_pipeline);
        pass.set_bind_group(0, target.stroke_uniform_bg, &[]);
        for slice in batch {
            pass.set_scissor_rect(
                slice.scissor[0],
                slice.scissor[1],
                slice.scissor[2],
                slice.scissor[3],
            );
            pass.set_vertex_buffer(0, slice.vb.slice(..));
            pass.set_index_buffer(slice.ib.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(slice.start_idx..slice.start_idx + slice.count_idx, 0, 0..1);
        }
    }
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("drawcraft_hl_composite_layer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_viewport(0.0, 0.0, tex_size[0] as f32, tex_size[1] as f32, 0.0, 1.0);
        pass.set_pipeline(&res.composite_offscreen_pipeline);
        pass.set_bind_group(0, &res.identity_quad_uniform_bg, &[]);
        pass.set_bind_group(1, &target.scratch.nearest_bg, &[]);
        pass.set_vertex_buffer(0, res.quad_vb.slice(..));
        pass.set_index_buffer(res.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
        for slice in batch {
            pass.set_scissor_rect(
                slice.scissor[0],
                slice.scissor[1],
                slice.scissor[2],
                slice.scissor[3],
            );
            pass.draw_indexed(0..6, 0, 0..1);
        }
    }
}

fn draw_isolated_highlighter_slices(
    encoder: &mut wgpu::CommandEncoder,
    res: &GpuInkResources,
    target: &IsolatedLayerTarget<'_>,
    tex_size: [u32; 2],
    slices: &[HighlighterSlice<'_>],
) {
    let mut batch: Vec<HighlighterSlice<'_>> = Vec::new();
    for &slice in slices {
        let overlaps_batch = batch
            .iter()
            .any(|existing| !scissors_disjoint(existing.scissor, slice.scissor));
        if overlaps_batch {
            flush_highlighter_batch(encoder, res, target, tex_size, &batch);
            batch.clear();
        }
        batch.push(slice);
    }
    flush_highlighter_batch(encoder, res, target, tex_size, &batch);
}

fn collect_hl_slices_in_scissor<'a>(
    res: &'a GpuInkResources,
    chunks: &[Arc<GpuChunkData>],
    ctx: &RenderPassLayerCtx<'_>,
    clip_scissor: [u32; 4],
) -> Vec<HighlighterSlice<'a>> {
    let pad_px = (ctx.feather_page * ctx.scale_px).max(4.0) + 4.0;
    let clip_page_rect = page_rect_for_screen_box(
        clip_scissor[0],
        clip_scissor[1],
        clip_scissor[2],
        clip_scissor[3],
        ctx.scale_px,
        ctx.pan_px,
        pad_px,
    );
    let mut out = Vec::new();
    for chunk in chunks {
        if !chunk.bounds.is_empty() && !chunk.bounds.intersects(clip_page_rect) {
            continue;
        }
        let Some(bufs) = res.chunk_buffers.get(&chunk.id) else {
            continue;
        };
        for &(bounds, start_idx, count_idx) in &chunk.stroke_ranges {
            if count_idx == 0 {
                continue;
            }
            if !bounds.is_empty() && !bounds.intersects(clip_page_rect) {
                continue;
            }
            let Some(raw_scissor) =
                page_bounds_to_scissor(bounds, ctx.scale_px, ctx.pan_px, pad_px, ctx.tex_size)
            else {
                continue;
            };
            let Some(scissor) = intersect_scissor(raw_scissor, clip_scissor) else {
                continue;
            };
            out.push(HighlighterSlice {
                vb: &bufs.vertex_buffer,
                ib: &bufs.index_buffer,
                start_idx,
                count_idx,
                scissor,
            });
        }
    }
    out
}

fn draw_chunks_in_rect(
    pass: &mut wgpu::RenderPass<'_>,
    res: &GpuInkResources,
    chunks: &[Arc<GpuChunkData>],
    clip_page_rect: drawcraft_geom::Rect,
) {
    pass.set_pipeline(&res.stroke_offscreen_pipeline);
    pass.set_bind_group(0, &res.stroke_uniform_bg, &[]);
    for chunk in chunks {
        if !chunk.bounds.is_empty() && !chunk.bounds.intersects(clip_page_rect) {
            continue;
        }
        if let Some(bufs) = res.chunk_buffers.get(&chunk.id)
            && bufs.index_count > 0
        {
            pass.set_vertex_buffer(0, bufs.vertex_buffer.slice(..));
            pass.set_index_buffer(bufs.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..bufs.index_count, 0, 0..1);
        }
    }
}

fn full_rebake_hl_layer(
    ctx: &mut RenderPassLayerCtx<'_>,
    res: &GpuInkResources,
    target_view: &wgpu::TextureView,
    scratch: &LayerTarget,
    chunks: &[Arc<GpuChunkData>],
) {
    clear_layer_target(ctx.encoder, target_view);
    let slices =
        collect_hl_slices_in_scissor(res, chunks, ctx, [0, 0, ctx.tex_size[0], ctx.tex_size[1]]);
    let target = IsolatedLayerTarget {
        scratch,
        target_view,
        stroke_uniform_bg: &res.stroke_uniform_bg,
    };
    draw_isolated_highlighter_slices(ctx.encoder, res, &target, ctx.tex_size, &slices);
}

fn incremental_append_hl_layer(
    ctx: &mut RenderPassLayerCtx<'_>,
    res: &GpuInkResources,
    target_view: &wgpu::TextureView,
    scratch: &LayerTarget,
    clear_first: bool,
    tail_chunk: &GpuChunkData,
) {
    if clear_first {
        clear_layer_target(ctx.encoder, target_view);
    }
    let Some(&(bounds, start_idx, count_idx)) = tail_chunk.stroke_ranges.last() else {
        return;
    };
    if count_idx == 0 {
        return;
    }
    let Some(bufs) = res.chunk_buffers.get(&tail_chunk.id) else {
        return;
    };
    let pad_px = (ctx.feather_page * ctx.scale_px).max(4.0) + 4.0;
    let Some(scissor) =
        page_bounds_to_scissor(bounds, ctx.scale_px, ctx.pan_px, pad_px, ctx.tex_size)
    else {
        return;
    };
    let target = IsolatedLayerTarget {
        scratch,
        target_view,
        stroke_uniform_bg: &res.stroke_uniform_bg,
    };
    let slice = HighlighterSlice {
        vb: &bufs.vertex_buffer,
        ib: &bufs.index_buffer,
        start_idx,
        count_idx,
        scissor,
    };
    draw_isolated_highlighter_slices(ctx.encoder, res, &target, ctx.tex_size, &[slice]);
}

fn pan_shift_blit_only(
    ctx: &mut RenderPassLayerCtx<'_>,
    res: &GpuInkResources,
    src_bg: &wgpu::BindGroup,
    dst_view: &wgpu::TextureView,
    delta_px: [i32; 2],
) -> (u32, u32) {
    let w = ctx.tex_size[0] as i32;
    let h = ctx.tex_size[1] as i32;
    let ix = delta_px[0];
    let iy = delta_px[1];
    let dst_x0 = ix.clamp(0, w) as u32;
    let dst_x1 = (w + ix).clamp(0, w) as u32;
    let dst_y0 = iy.clamp(0, h) as u32;
    let dst_y1 = (h + iy).clamp(0, h) as u32;

    write_uniform_f32(
        ctx.queue,
        &res.blit_uniform_buf,
        [
            1.0,
            1.0,
            -(ix as f32) / ctx.tex_size[0] as f32,
            -(iy as f32) / ctx.tex_size[1] as f32,
        ],
    );

    let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("drawcraft_pan_shift_hl_blit"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: dst_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_viewport(
        0.0,
        0.0,
        ctx.tex_size[0] as f32,
        ctx.tex_size[1] as f32,
        0.0,
        1.0,
    );
    if dst_x1 > dst_x0 && dst_y1 > dst_y0 {
        pass.set_scissor_rect(dst_x0, dst_y0, dst_x1 - dst_x0, dst_y1 - dst_y0);
        pass.set_pipeline(&res.blit_pipeline);
        pass.set_bind_group(0, &res.blit_uniform_bg, &[]);
        pass.set_bind_group(1, src_bg, &[]);
        pass.set_vertex_buffer(0, res.quad_vb.slice(..));
        pass.set_index_buffer(res.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..6, 0, 0..1);
    }
    (dst_y0, dst_y1)
}

fn pan_shift_hl_layer(
    ctx: &mut RenderPassLayerCtx<'_>,
    res: &GpuInkResources,
    src_bg: &wgpu::BindGroup,
    dst_view: &wgpu::TextureView,
    scratch: &LayerTarget,
    chunks: &[Arc<GpuChunkData>],
    delta_px: [i32; 2],
) {
    let (dst_y0, dst_y1) = pan_shift_blit_only(ctx, res, src_bg, dst_view, delta_px);
    let w = ctx.tex_size[0] as i32;
    let h = ctx.tex_size[1] as i32;
    let ix = delta_px[0];
    let iy = delta_px[1];

    let mut slices = Vec::new();
    if iy != 0 {
        let (sy, sh) = if iy > 0 {
            (0_u32, iy.min(h) as u32)
        } else {
            ((h + iy).max(0) as u32, (-iy).min(h) as u32)
        };
        if sh > 0 {
            slices.extend(collect_hl_slices_in_scissor(
                res,
                chunks,
                ctx,
                [0, sy, ctx.tex_size[0], sh],
            ));
        }
    }
    if ix != 0 && dst_y1 > dst_y0 {
        let (sx, sw) = if ix > 0 {
            (0_u32, ix.min(w) as u32)
        } else {
            ((w + ix).max(0) as u32, (-ix).min(w) as u32)
        };
        if sw > 0 {
            slices.extend(collect_hl_slices_in_scissor(
                res,
                chunks,
                ctx,
                [sx, dst_y0, sw, dst_y1 - dst_y0],
            ));
        }
    }

    let target = IsolatedLayerTarget {
        scratch,
        target_view: dst_view,
        stroke_uniform_bg: &res.stroke_uniform_bg,
    };
    draw_isolated_highlighter_slices(ctx.encoder, res, &target, ctx.tex_size, &slices);
}

fn full_rebake_layer(
    ctx: &mut RenderPassLayerCtx<'_>,
    res: &GpuInkResources,
    target_view: &wgpu::TextureView,
    chunks: &[Arc<GpuChunkData>],
    vis_page_rect: drawcraft_geom::Rect,
) {
    let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("drawcraft_rebake_layer"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_viewport(
        0.0,
        0.0,
        ctx.tex_size[0] as f32,
        ctx.tex_size[1] as f32,
        0.0,
        1.0,
    );
    pass.set_scissor_rect(0, 0, ctx.tex_size[0], ctx.tex_size[1]);
    draw_chunks_in_rect(&mut pass, res, chunks, vis_page_rect);
}

fn incremental_append_layer(
    ctx: &mut RenderPassLayerCtx<'_>,
    res: &GpuInkResources,
    target_view: &wgpu::TextureView,
    clear_first: bool,
    tail_chunk: &GpuChunkData,
) {
    let (start_idx, count_idx) = tail_chunk.last_stroke_index_range;
    if count_idx == 0 {
        return;
    }
    let Some(bufs) = res.chunk_buffers.get(&tail_chunk.id) else {
        return;
    };
    let load = if clear_first {
        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
    } else {
        wgpu::LoadOp::Load
    };
    let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("drawcraft_append_stroke_layer"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_viewport(
        0.0,
        0.0,
        ctx.tex_size[0] as f32,
        ctx.tex_size[1] as f32,
        0.0,
        1.0,
    );
    pass.set_scissor_rect(0, 0, ctx.tex_size[0], ctx.tex_size[1]);
    pass.set_pipeline(&res.stroke_offscreen_pipeline);
    pass.set_bind_group(0, &res.stroke_uniform_bg, &[]);
    pass.set_vertex_buffer(0, bufs.vertex_buffer.slice(..));
    pass.set_index_buffer(bufs.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
    pass.draw_indexed(start_idx..start_idx + count_idx, 0, 0..1);
}

fn pan_shift_layer(
    ctx: &mut RenderPassLayerCtx<'_>,
    res: &GpuInkResources,
    src_bg: &wgpu::BindGroup,
    dst_view: &wgpu::TextureView,
    chunks: &[Arc<GpuChunkData>],
    delta_px: [i32; 2],
) {
    let w = ctx.tex_size[0] as i32;
    let h = ctx.tex_size[1] as i32;
    let ix = delta_px[0];
    let iy = delta_px[1];

    let dst_x0 = ix.clamp(0, w) as u32;
    let dst_x1 = (w + ix).clamp(0, w) as u32;
    let dst_y0 = iy.clamp(0, h) as u32;
    let dst_y1 = (h + iy).clamp(0, h) as u32;

    write_uniform_f32(
        ctx.queue,
        &res.blit_uniform_buf,
        [
            1.0,
            1.0,
            -(ix as f32) / ctx.tex_size[0] as f32,
            -(iy as f32) / ctx.tex_size[1] as f32,
        ],
    );

    let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("drawcraft_pan_shift_layer"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: dst_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_viewport(
        0.0,
        0.0,
        ctx.tex_size[0] as f32,
        ctx.tex_size[1] as f32,
        0.0,
        1.0,
    );

    if dst_x1 > dst_x0 && dst_y1 > dst_y0 {
        pass.set_scissor_rect(dst_x0, dst_y0, dst_x1 - dst_x0, dst_y1 - dst_y0);
        pass.set_pipeline(&res.blit_pipeline);
        pass.set_bind_group(0, &res.blit_uniform_bg, &[]);
        pass.set_bind_group(1, src_bg, &[]);
        pass.set_vertex_buffer(0, res.quad_vb.slice(..));
        pass.set_index_buffer(res.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..6, 0, 0..1);
    }

    let pad_px = (ctx.feather_page * ctx.scale_px).max(4.0) + 4.0;
    if iy != 0 {
        let (sy, sh) = if iy > 0 {
            (0_u32, iy.min(h) as u32)
        } else {
            ((h + iy).max(0) as u32, (-iy).min(h) as u32)
        };
        if sh > 0 {
            pass.set_scissor_rect(0, sy, ctx.tex_size[0], sh);
            let strip_rect = page_rect_for_screen_box(
                0,
                sy,
                ctx.tex_size[0],
                sh,
                ctx.scale_px,
                ctx.pan_px,
                pad_px,
            );
            draw_chunks_in_rect(&mut pass, res, chunks, strip_rect);
        }
    }
    if ix != 0 && dst_y1 > dst_y0 {
        let (sx, sw) = if ix > 0 {
            (0_u32, ix.min(w) as u32)
        } else {
            ((w + ix).max(0) as u32, (-ix).min(w) as u32)
        };
        if sw > 0 {
            pass.set_scissor_rect(sx, dst_y0, sw, dst_y1 - dst_y0);
            let strip_rect = page_rect_for_screen_box(
                sx,
                dst_y0,
                sw,
                dst_y1 - dst_y0,
                ctx.scale_px,
                ctx.pan_px,
                pad_px,
            );
            draw_chunks_in_rect(&mut pass, res, chunks, strip_rect);
        }
    }
}

enum LayerAction<'a> {
    None,
    FullRebake,
    Append {
        hl: Option<&'a Arc<GpuChunkData>>,
        ink: Option<&'a Arc<GpuChunkData>>,
    },
    PanShift([i32; 2]),
}

struct FrameMetrics {
    tex_size: [u32; 2],
    ppp: f32,
    scale_px: f32,
    target_pan_px: [f32; 2],
    feather_page: f32,
}

fn decide_layer_action<'a>(
    cb: &'a GpuInkPaintCallback,
    res: &mut GpuInkResources,
    fm: &FrameMetrics,
    cur_hl_ids: &[u64],
    cur_ink_ids: &[u64],
) -> LayerAction<'a> {
    let same_page_color =
        res.baked.valid && res.baked.page_key == cb.page_key && res.baked.color_key == cb.color_key;
    if !same_page_color {
        return LayerAction::FullRebake;
    }

    let same_zoom_ppp =
        (res.baked.zoom - cb.zoom).abs() < 1e-5 && (res.baked.ppp - fm.ppp).abs() < 1e-5;
    let pan_delta = [
        fm.target_pan_px[0] - res.baked.pan_px[0],
        fm.target_pan_px[1] - res.baked.pan_px[1],
    ];
    let small_pan = pan_delta[0].abs() <= 0.55 && pan_delta[1].abs() <= 0.55;

    let hl_same = res.baked.hl_chunk_ids == cur_hl_ids;
    let ink_same = res.baked.ink_chunk_ids == cur_ink_ids;
    if !hl_same || !ink_same {
        if !(same_zoom_ppp && small_pan) {
            return LayerAction::FullRebake;
        }
        let hl_tail = if hl_same {
            None
        } else {
            detect_single_append(&res.baked.hl_chunk_ids, &cb.hl_chunks)
        };
        let ink_tail = if ink_same {
            None
        } else {
            detect_single_append(&res.baked.ink_chunk_ids, &cb.ink_chunks)
        };
        let hl_ok = hl_same || hl_tail.is_some();
        let ink_ok = ink_same || ink_tail.is_some();
        return if hl_ok && ink_ok {
            LayerAction::Append {
                hl: hl_tail,
                ink: ink_tail,
            }
        } else {
            LayerAction::FullRebake
        };
    }

    if !same_zoom_ppp {
        // While a zoom gesture is still in motion on a heavy page, reuse the
        // previously baked layers as a scaled proxy. Once the zoom settles the
        // layers must be rebaked, otherwise regions outside the old texture
        // stay blank until some unrelated change forces a rebake.
        if !cb.zoom_settled {
            let pad_px = (fm.feather_page * fm.scale_px).max(4.0) + 4.0;
            let vis_rect = page_rect_for_screen_box(
                0,
                0,
                fm.tex_size[0],
                fm.tex_size[1],
                fm.scale_px,
                fm.target_pan_px,
                pad_px,
            );
            let vis_tris = count_visible_triangles(&cb.hl_chunks, vis_rect)
                + count_visible_triangles(&cb.ink_chunks, vis_rect);
            let ratio = cb.zoom / res.baked.zoom.max(0.01);
            if vis_tris >= PROXY_ZOOM_TRI_THRESHOLD && (0.68..=1.45).contains(&ratio) {
                return LayerAction::None;
            }
        }
        return LayerAction::FullRebake;
    }

    if !small_pan {
        let ix = pan_delta[0].round() as i32;
        let iy = pan_delta[1].round() as i32;
        if ix.abs() >= fm.tex_size[0] as i32 || iy.abs() >= fm.tex_size[1] as i32 {
            return LayerAction::FullRebake;
        }
        if ix != 0 || iy != 0 {
            return LayerAction::PanShift([ix, iy]);
        }
    }

    LayerAction::None
}

#[allow(clippy::too_many_arguments)]
fn execute_full_rebake(
    cb: &GpuInkPaintCallback,
    res: &mut GpuInkResources,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    fm: &FrameMetrics,
    cur_hl_ids: Vec<u64>,
    cur_ink_ids: Vec<u64>,
) {
    write_uniform_f32(
        queue,
        &res.stroke_uniform_buf,
        [
            fm.scale_px,
            fm.feather_page,
            fm.tex_size[0] as f32,
            fm.tex_size[1] as f32,
            fm.target_pan_px[0],
            fm.target_pan_px[1],
            0.0,
            0.0,
        ],
    );
    let pad_px = (fm.feather_page * fm.scale_px).max(4.0) + 4.0;
    let vis_rect = page_rect_for_screen_box(
        0,
        0,
        fm.tex_size[0],
        fm.tex_size[1],
        fm.scale_px,
        fm.target_pan_px,
        pad_px,
    );
    let has_hl = !cb.hl_chunks.is_empty();
    let has_ink = !cb.ink_chunks.is_empty();
    if has_hl {
        if res.hl_layers[res.hl_active].is_none() {
            res.hl_layers[res.hl_active] =
                Some(res.create_layer_target(device, fm.tex_size, "drawcraft_hl_layer"));
        }
        if res.hl_scratch_layer.is_none() {
            res.hl_scratch_layer =
                Some(res.create_layer_target(device, fm.tex_size, "drawcraft_hl_scratch_layer"));
        }
    }
    if has_ink && res.ink_layers[res.ink_active].is_none() {
        res.ink_layers[res.ink_active] =
            Some(res.create_layer_target(device, fm.tex_size, "drawcraft_ink_layer"));
    }
    let mut pass_ctx = RenderPassLayerCtx {
        encoder,
        queue,
        tex_size: fm.tex_size,
        scale_px: fm.scale_px,
        feather_page: fm.feather_page,
        pan_px: fm.target_pan_px,
    };
    if has_hl
        && let (Some(layer), Some(scratch)) = (&res.hl_layers[res.hl_active], &res.hl_scratch_layer)
    {
        full_rebake_hl_layer(&mut pass_ctx, res, &layer.view, scratch, &cb.hl_chunks);
    }
    if has_ink && let Some(layer) = &res.ink_layers[res.ink_active] {
        full_rebake_layer(&mut pass_ctx, res, &layer.view, &cb.ink_chunks, vis_rect);
    }
    res.baked = BakedLayerState {
        valid: true,
        page_key: cb.page_key,
        color_key: cb.color_key,
        tex_size: fm.tex_size,
        zoom: cb.zoom,
        scale_px: fm.scale_px,
        pan_px: fm.target_pan_px,
        ppp: fm.ppp,
        has_hl,
        has_ink,
        hl_chunk_ids: cur_hl_ids,
        ink_chunk_ids: cur_ink_ids,
    };
}

#[allow(clippy::too_many_arguments)]
fn execute_append(
    res: &mut GpuInkResources,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    fm: &FrameMetrics,
    hl_tail: Option<&Arc<GpuChunkData>>,
    ink_tail: Option<&Arc<GpuChunkData>>,
    cur_hl_ids: Vec<u64>,
    cur_ink_ids: Vec<u64>,
) {
    write_uniform_f32(
        queue,
        &res.stroke_uniform_buf,
        [
            res.baked.scale_px,
            fm.feather_page,
            fm.tex_size[0] as f32,
            fm.tex_size[1] as f32,
            res.baked.pan_px[0],
            res.baked.pan_px[1],
            0.0,
            0.0,
        ],
    );
    if hl_tail.is_some() {
        if res.hl_layers[res.hl_active].is_none() {
            res.hl_layers[res.hl_active] =
                Some(res.create_layer_target(device, fm.tex_size, "drawcraft_hl_layer"));
        }
        if res.hl_scratch_layer.is_none() {
            res.hl_scratch_layer =
                Some(res.create_layer_target(device, fm.tex_size, "drawcraft_hl_scratch_layer"));
        }
    }
    if ink_tail.is_some() && res.ink_layers[res.ink_active].is_none() {
        res.ink_layers[res.ink_active] =
            Some(res.create_layer_target(device, fm.tex_size, "drawcraft_ink_layer"));
    }
    let was_hl = res.baked.has_hl;
    let was_ink = res.baked.has_ink;
    let mut pass_ctx = RenderPassLayerCtx {
        encoder,
        queue,
        tex_size: fm.tex_size,
        scale_px: res.baked.scale_px,
        feather_page: fm.feather_page,
        pan_px: res.baked.pan_px,
    };
    if let Some(tail) = hl_tail
        && let (Some(layer), Some(scratch)) = (&res.hl_layers[res.hl_active], &res.hl_scratch_layer)
    {
        incremental_append_hl_layer(&mut pass_ctx, res, &layer.view, scratch, !was_hl, tail);
        res.baked.has_hl = true;
    }
    if let Some(tail) = ink_tail
        && let Some(layer) = &res.ink_layers[res.ink_active]
    {
        incremental_append_layer(&mut pass_ctx, res, &layer.view, !was_ink, tail);
        res.baked.has_ink = true;
    }
    res.baked.hl_chunk_ids = cur_hl_ids;
    res.baked.ink_chunk_ids = cur_ink_ids;
}

fn execute_pan_shift(
    cb: &GpuInkPaintCallback,
    res: &mut GpuInkResources,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    fm: &FrameMetrics,
    delta_px: [i32; 2],
) {
    let new_pan = [
        res.baked.pan_px[0] + delta_px[0] as f32,
        res.baked.pan_px[1] + delta_px[1] as f32,
    ];
    write_uniform_f32(
        queue,
        &res.stroke_uniform_buf,
        [
            res.baked.scale_px,
            fm.feather_page,
            fm.tex_size[0] as f32,
            fm.tex_size[1] as f32,
            new_pan[0],
            new_pan[1],
            0.0,
            0.0,
        ],
    );
    if res.baked.has_hl {
        let src_idx = res.hl_active;
        let dst_idx = src_idx ^ 1;
        if res.hl_layers[dst_idx].is_none() {
            res.hl_layers[dst_idx] =
                Some(res.create_layer_target(device, fm.tex_size, "drawcraft_hl_layer"));
        }
        if res.hl_scratch_layer.is_none() {
            res.hl_scratch_layer =
                Some(res.create_layer_target(device, fm.tex_size, "drawcraft_hl_scratch_layer"));
        }
        if let (Some(src), Some(dst), Some(scratch)) = (
            &res.hl_layers[src_idx],
            &res.hl_layers[dst_idx],
            &res.hl_scratch_layer,
        ) {
            let mut pass_ctx = RenderPassLayerCtx {
                encoder,
                queue,
                tex_size: fm.tex_size,
                scale_px: res.baked.scale_px,
                feather_page: fm.feather_page,
                pan_px: new_pan,
            };
            pan_shift_hl_layer(
                &mut pass_ctx,
                res,
                &src.nearest_bg,
                &dst.view,
                scratch,
                &cb.hl_chunks,
                delta_px,
            );
            res.hl_active = dst_idx;
        }
    }
    if res.baked.has_ink {
        let src_idx = res.ink_active;
        let dst_idx = src_idx ^ 1;
        if res.ink_layers[dst_idx].is_none() {
            res.ink_layers[dst_idx] =
                Some(res.create_layer_target(device, fm.tex_size, "drawcraft_ink_layer"));
        }
        if let (Some(src), Some(dst)) = (&res.ink_layers[src_idx], &res.ink_layers[dst_idx]) {
            let mut pass_ctx = RenderPassLayerCtx {
                encoder,
                queue,
                tex_size: fm.tex_size,
                scale_px: res.baked.scale_px,
                feather_page: fm.feather_page,
                pan_px: new_pan,
            };
            pan_shift_layer(
                &mut pass_ctx,
                res,
                &src.nearest_bg,
                &dst.view,
                &cb.ink_chunks,
                delta_px,
            );
            res.ink_active = dst_idx;
        }
    }
    res.baked.pan_px = new_pan;
}

fn prepare_overlay_highlighters(
    cb: &GpuInkPaintCallback,
    res: &mut GpuInkResources,
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    fm: &FrameMetrics,
) {
    let Some(hl) = &cb.overlay_hl else {
        res.overlay_hl_target = OverlayHlTarget::None;
        return;
    };
    if hl.ranges.is_empty()
        || res.overlay_hl_vb.buffer.is_none()
        || res.overlay_hl_ib.buffer.is_none()
    {
        res.overlay_hl_target = OverlayHlTarget::None;
        return;
    }

    let pad_px = (fm.feather_page * fm.scale_px).max(4.0) + 4.0;
    let valid_slices: Vec<(u32, u32, [u32; 4])> = hl
        .ranges
        .iter()
        .filter_map(|&(bounds, start_idx, count_idx)| {
            if count_idx == 0 {
                return None;
            }
            let scissor =
                page_bounds_to_scissor(bounds, fm.scale_px, fm.target_pan_px, pad_px, fm.tex_size)?;
            Some((start_idx, count_idx, scissor))
        })
        .collect();

    if valid_slices.is_empty() {
        res.overlay_hl_target = OverlayHlTarget::None;
        return;
    }

    if res.hl_scratch_layer.is_none() {
        res.hl_scratch_layer =
            Some(res.create_layer_target(device, fm.tex_size, "drawcraft_hl_scratch_layer"));
    }
    if valid_slices.len() > 1 && res.hl_overlay_layer.is_none() {
        res.hl_overlay_layer =
            Some(res.create_layer_target(device, fm.tex_size, "drawcraft_hl_overlay_layer"));
    }

    let (Some(vb), Some(ib), Some(scratch)) = (
        &res.overlay_hl_vb.buffer,
        &res.overlay_hl_ib.buffer,
        &res.hl_scratch_layer,
    ) else {
        res.overlay_hl_target = OverlayHlTarget::None;
        return;
    };

    let slices: Vec<HighlighterSlice<'_>> = valid_slices
        .iter()
        .map(|&(start_idx, count_idx, scissor)| HighlighterSlice {
            vb,
            ib,
            start_idx,
            count_idx,
            scissor,
        })
        .collect();

    if slices.len() == 1 {
        render_single_hl_to_scratch(
            encoder,
            res,
            scratch,
            &res.overlay_uniform_bg,
            fm.tex_size,
            slices[0],
        );
        res.overlay_hl_target = OverlayHlTarget::Scratch;
    } else if let Some(overlay_layer) = &res.hl_overlay_layer {
        clear_layer_target(encoder, &overlay_layer.view);
        let target = IsolatedLayerTarget {
            scratch,
            target_view: &overlay_layer.view,
            stroke_uniform_bg: &res.overlay_uniform_bg,
        };
        draw_isolated_highlighter_slices(encoder, res, &target, fm.tex_size, &slices);
        res.overlay_hl_target = OverlayHlTarget::OverlayLayer;
    } else {
        res.overlay_hl_target = OverlayHlTarget::None;
    }
}

impl CallbackTrait for GpuInkPaintCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_descriptor: &ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = callback_resources.get_mut::<GpuInkResources>() else {
            return Vec::new();
        };

        let vp = viewport_px_from_rect(self.avail_rect, screen_descriptor);
        if vp.width_px <= 0 || vp.height_px <= 0 {
            return Vec::new();
        }
        let max_dim = device.limits().max_texture_dimension_2d.max(1);
        let tex_size = [
            (vp.width_px as u32).min(max_dim).max(1),
            (vp.height_px as u32).min(max_dim).max(1),
        ];

        res.sync_chunk_buffers(device, &self.hl_chunks, &self.ink_chunks);
        res.upload_overlays(
            device,
            queue,
            self.overlay_hl.as_deref(),
            self.overlay_ink.as_deref(),
        );

        if res.baked.tex_size != tex_size {
            res.hl_layers = [None, None];
            res.hl_scratch_layer = None;
            res.hl_overlay_layer = None;
            res.ink_layers = [None, None];
            res.hl_active = 0;
            res.ink_active = 0;
            res.overlay_hl_target = OverlayHlTarget::None;
            res.baked.valid = false;
            res.baked.tex_size = tex_size;
        }

        let ppp = screen_descriptor.pixels_per_point.max(0.1);
        let fm = FrameMetrics {
            tex_size,
            ppp,
            scale_px: (self.scale as f32) * ppp,
            target_pan_px: [
                (self.offset_x as f32) * ppp - vp.left_px as f32,
                (self.offset_y as f32) * ppp - vp.top_px as f32,
            ],
            feather_page: (0.9 / self.zoom.max(0.25)).clamp(0.3, 2.5),
        };

        write_uniform_f32(
            queue,
            &res.overlay_uniform_buf,
            [
                fm.scale_px,
                fm.feather_page,
                vp.width_px as f32,
                vp.height_px as f32,
                fm.target_pan_px[0],
                fm.target_pan_px[1],
                0.0,
                0.0,
            ],
        );
        write_uniform_f32(
            queue,
            &res.overlay_comp_uniform_buf,
            [
                (vp.width_px as f32) / (tex_size[0] as f32),
                (vp.height_px as f32) / (tex_size[1] as f32),
                0.0,
                0.0,
            ],
        );

        let cur_hl_ids: Vec<u64> = self.hl_chunks.iter().map(|c| c.id).collect();
        let cur_ink_ids: Vec<u64> = self.ink_chunks.iter().map(|c| c.id).collect();
        let action = decide_layer_action(self, res, &fm, &cur_hl_ids, &cur_ink_ids);
        match action {
            LayerAction::None => {}
            LayerAction::FullRebake => execute_full_rebake(
                self,
                res,
                device,
                queue,
                egui_encoder,
                &fm,
                cur_hl_ids,
                cur_ink_ids,
            ),
            LayerAction::Append { hl, ink } => execute_append(
                res,
                device,
                queue,
                egui_encoder,
                &fm,
                hl,
                ink,
                cur_hl_ids,
                cur_ink_ids,
            ),
            LayerAction::PanShift(delta) => {
                execute_pan_shift(self, res, device, queue, egui_encoder, &fm, delta);
            }
        }

        prepare_overlay_highlighters(self, res, device, egui_encoder, &fm);

        let k = res.baked.scale_px / fm.scale_px.max(1e-4);
        let uv_sx = k * (vp.width_px as f32) / (tex_size[0] as f32);
        let uv_sy = k * (vp.height_px as f32) / (tex_size[1] as f32);
        let uv_ox = (res.baked.pan_px[0] - fm.target_pan_px[0] * k) / (tex_size[0] as f32);
        let uv_oy = (res.baked.pan_px[1] - fm.target_pan_px[1] * k) / (tex_size[1] as f32);
        write_uniform_f32(
            queue,
            &res.comp_hl_uniform_buf,
            [uv_sx, uv_sy, uv_ox, uv_oy],
        );
        write_uniform_f32(
            queue,
            &res.comp_ink_uniform_buf,
            [uv_sx, uv_sy, uv_ox, uv_oy],
        );
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &CallbackResources,
    ) {
        let Some(res) = callback_resources.get::<GpuInkResources>() else {
            return;
        };
        if !res.baked.valid {
            return;
        }

        if res.baked.has_hl
            && let Some(layer) = &res.hl_layers[res.hl_active]
        {
            render_pass.set_pipeline(&res.composite_pipeline);
            render_pass.set_bind_group(0, &res.comp_hl_uniform_bg, &[]);
            render_pass.set_bind_group(1, &layer.linear_bg, &[]);
            render_pass.set_vertex_buffer(0, res.quad_vb.slice(..));
            render_pass.set_index_buffer(res.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..6, 0, 0..1);
        }

        let overlay_hl_layer = match res.overlay_hl_target {
            OverlayHlTarget::None => None,
            OverlayHlTarget::Scratch => res.hl_scratch_layer.as_ref(),
            OverlayHlTarget::OverlayLayer => res.hl_overlay_layer.as_ref(),
        };
        if let Some(layer) = overlay_hl_layer {
            render_pass.set_pipeline(&res.composite_pipeline);
            render_pass.set_bind_group(0, &res.overlay_comp_uniform_bg, &[]);
            render_pass.set_bind_group(1, &layer.linear_bg, &[]);
            render_pass.set_vertex_buffer(0, res.quad_vb.slice(..));
            render_pass.set_index_buffer(res.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..6, 0, 0..1);
        }

        if res.baked.has_ink
            && let Some(layer) = &res.ink_layers[res.ink_active]
        {
            render_pass.set_pipeline(&res.composite_pipeline);
            render_pass.set_bind_group(0, &res.comp_ink_uniform_bg, &[]);
            render_pass.set_bind_group(1, &layer.linear_bg, &[]);
            render_pass.set_vertex_buffer(0, res.quad_vb.slice(..));
            render_pass.set_index_buffer(res.quad_ib.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..6, 0, 0..1);
        }

        if let Some(ink) = &self.overlay_ink
            && let (Some(vb), Some(ib)) = (&res.overlay_ink_vb.buffer, &res.overlay_ink_ib.buffer)
            && ink.index_count > 0
        {
            render_pass.set_pipeline(&res.stroke_overlay_pipeline);
            render_pass.set_bind_group(0, &res.overlay_uniform_bg, &[]);
            render_pass.set_vertex_buffer(0, vb.slice(..));
            render_pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..ink.index_count, 0, 0..1);
        }
    }
}

pub(crate) fn init_wgpu_state(
    app: &mut DrawCraftApp,
    render_state: Option<&egui_wgpu::RenderState>,
) {
    let Some(state) = render_state else {
        app.wgpu_enabled = false;
        return;
    };
    let resources = GpuInkResources::new(&state.device, state.target_format);
    state.renderer.write().callback_resources.insert(resources);
    app.wgpu_enabled = true;
}

pub(crate) fn paint_committed_strokes_gpu(
    app: &mut DrawCraftApp,
    p: &egui::Painter,
    avail_rect: Rect,
    xf: Affine2,
    page_key: (usize, usize),
    color_key: (UiTheme, bool, PaperColor),
) {
    if !avail_rect.is_positive() {
        return;
    }

    let dragging_ids = active_drag_stroke_ids(app);
    let drag_sig = dragging_ids.len();
    let doc_ptr = Arc::as_ptr(&app.session.doc) as usize;
    let sync_sig = (
        doc_ptr,
        app.session.revision,
        app.session.doc.strokes.len(),
        app.session.doc.strokes.last().map(|s| s.id),
        drag_sig,
    );

    let cache = &mut app.canvas_cache.gpu_scene;
    if cache.page_key != page_key || cache.color_key != Some(color_key) {
        cache.clear();
        cache.page_key = page_key;
        cache.color_key = Some(color_key);
    }

    let strokes_empty = app.session.doc.strokes.is_empty();
    let need_sync =
        cache.last_sync_sig != Some(sync_sig) || (cache.strokes.is_empty() && !strokes_empty);

    if need_sync {
        let mut hl_ids = Vec::new();
        let mut ink_ids = Vec::new();
        let mut overlay_hl_ids = Vec::new();
        let mut overlay_ink_ids = Vec::new();

        for stroke in &app.session.doc.strokes {
            let needs_build = match cache.strokes.get(&stroke.id) {
                Some(entry) => !Arc::ptr_eq(&entry.source, stroke) || entry.color_key != color_key,
                None => true,
            };
            if needs_build {
                let built = build_cached_scalable_stroke(stroke, color_key);
                cache.strokes.insert(stroke.id, built);
            }
            let is_hl = stroke.pen.kind == PenKind::Highlighter;
            if dragging_ids.contains(&stroke.id) {
                if is_hl {
                    overlay_hl_ids.push(stroke.id);
                } else {
                    overlay_ink_ids.push(stroke.id);
                }
            } else if is_hl {
                hl_ids.push(stroke.id);
            } else {
                ink_ids.push(stroke.id);
            }
        }

        if cache.strokes.len() > app.session.doc.strokes.len().saturating_add(32) {
            let active_set: HashSet<StrokeId> =
                app.session.doc.strokes.iter().map(|s| s.id).collect();
            cache.strokes.retain(|id, _| active_set.contains(id));
        }

        sync_chunk_list(cache, true, &hl_ids);
        sync_chunk_list(cache, false, &ink_ids);
        cache.overlay_hl_ids = overlay_hl_ids;
        cache.last_sync_sig = Some(sync_sig);

        let overlay_hl = build_overlay_highlighters(cache, &cache.overlay_hl_ids, None);
        let overlay_ink = build_overlay_bytes(cache, &overlay_ink_ids);
        app.canvas_cache.gpu_overlay_hl = overlay_hl;
        app.canvas_cache.gpu_overlay_ink = overlay_ink;
    }

    let (theme, auto_contrast, paper_color) = color_key;
    let live_overlay_hl = app
        .session
        .live_stroke
        .as_ref()
        .filter(|live| live.recognized_shape.is_none() && live.pen.kind == PenKind::Highlighter)
        .and_then(|live| {
            let preview = live.preview_samples();
            if preview.is_empty() {
                return None;
            }
            let mut display_pen = live.pen.clone();
            display_pen.color =
                paper_color.resolve_ink_color(theme, auto_contrast, live.pen.color, true);
            let nodes = evaluate_spine(&display_pen, &preview);
            let mesh = build_stroke_ribbon_scalable(&display_pen, &nodes);
            if mesh.indices.is_empty() {
                return None;
            }
            let packed = pack_mesh_colors(&mesh, false);
            let bounds = compute_scalable_mesh_bounds(&mesh, drawcraft_geom::Rect::EMPTY);
            build_overlay_highlighters(
                &app.canvas_cache.gpu_scene,
                &app.canvas_cache.gpu_scene.overlay_hl_ids,
                Some((&mesh, &packed, bounds)),
            )
        });

    let now_s = p.ctx().input(|i| i.time);
    let zoom_bits = app.view.zoom.to_bits();
    let cache = &mut app.canvas_cache.gpu_scene;
    let zoom_settled = if cache.last_zoom_bits != zoom_bits {
        cache.last_zoom_bits = zoom_bits;
        cache.pending_zoom_rebake_since_s = Some(now_s);
        p.ctx()
            .request_repaint_after(std::time::Duration::from_millis(75));
        false
    } else if let Some(since) = cache.pending_zoom_rebake_since_s {
        let elapsed = now_s - since;
        // NaN or clock anomalies count as settled so the layers never get stuck.
        if elapsed.is_nan() || elapsed < 0.0 || elapsed >= ZOOM_SETTLE_S {
            cache.pending_zoom_rebake_since_s = None;
            true
        } else {
            // Keep frames coming so the settle deadline is observed even if
            // an intermediate frame consumed the earlier repaint request.
            p.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
            false
        }
    } else {
        true
    };

    let overlay_hl = live_overlay_hl.or_else(|| app.canvas_cache.gpu_overlay_hl.clone());
    let callback = egui_wgpu::Callback::new_paint_callback(
        avail_rect,
        GpuInkPaintCallback {
            avail_rect,
            page_key,
            color_key,
            zoom: app.view.zoom,
            scale: xf.scale,
            offset_x: xf.offset_x,
            offset_y: xf.offset_y,
            zoom_settled,
            hl_chunks: cache.hl_chunks.clone(),
            ink_chunks: cache.ink_chunks.clone(),
            overlay_hl,
            overlay_ink: app.canvas_cache.gpu_overlay_ink.clone(),
        },
    );
    p.add(egui::Shape::Callback(callback));
}

#[cfg(test)]
mod tests {
    use super::*;
    use drawcraft_color::Color;
    use drawcraft_doc::{PenSettings, StrokeSample};

    fn make_test_stroke(id: u64, kind: PenKind, offset_x: f64, points: usize) -> Arc<InkStroke> {
        let mut pts = Vec::with_capacity(points);
        for i in 0..points {
            let x = offset_x + (i as f64) * 3.0;
            let y = 40.0 + ((i as f64) * 0.25).sin() * 12.0;
            let mut s = StrokeSample::from_xy_pressure(x, y, 0.6);
            s.time_ms = (i as f64) * 8.0;
            pts.push(s);
        }
        let pen = PenSettings {
            kind,
            size: 4.0,
            color: Color::from_rgba8(20, 90, 220, 255),
            ..PenSettings::default()
        };
        let bounds = drawcraft_doc::compute_stroke_bounds(&pts, pen.size);
        let pts_arc = Arc::new(pts);
        Arc::new(InkStroke {
            id: StrokeId(id),
            pen,
            raw_points: Arc::clone(&pts_arc),
            smoothed_points: pts_arc,
            bounds,
        })
    }

    #[test]
    fn gpu_chunk_sync_appends_single_stroke_incrementally_and_packs_20_byte_stride() {
        let color_key = (UiTheme::Light, true, PaperColor::PureWhite);
        let mut cache = GpuStrokeSceneCache::default();
        let s1 = make_test_stroke(1, PenKind::FountainPen, 20.0, 24);
        let s2 = make_test_stroke(2, PenKind::FountainPen, 120.0, 24);
        cache
            .strokes
            .insert(s1.id, build_cached_scalable_stroke(&s1, color_key));
        cache
            .strokes
            .insert(s2.id, build_cached_scalable_stroke(&s2, color_key));

        sync_chunk_list(&mut cache, false, &[s1.id, s2.id]);
        assert_eq!(cache.ink_chunks.len(), 1);
        let chunk1 = Arc::clone(&cache.ink_chunks[0]);
        assert_eq!(chunk1.vertex_bytes.len(), chunk1.vertex_count * 20);
        assert_eq!(chunk1.index_bytes.len(), (chunk1.index_count as usize) * 4);
        assert!(chunk1.index_count > 0);
        let prev_ids = vec![chunk1.id];

        let s3 = make_test_stroke(3, PenKind::FountainPen, 240.0, 24);
        cache
            .strokes
            .insert(s3.id, build_cached_scalable_stroke(&s3, color_key));
        sync_chunk_list(&mut cache, false, &[s1.id, s2.id, s3.id]);
        assert_eq!(cache.ink_chunks.len(), 1);
        let chunk2 = &cache.ink_chunks[0];
        assert_eq!(chunk2.parent_id, Some(chunk1.id));
        let detected = detect_single_append(&prev_ids, &cache.ink_chunks);
        assert_eq!(detected.map(|c| c.id), Some(chunk2.id));
        assert!(chunk2.last_stroke_index_range.1 > 0);

        let vis_left = drawcraft_geom::Rect::new(0.0, 0.0, 200.0, 200.0);
        let vis_far = drawcraft_geom::Rect::new(5000.0, 5000.0, 200.0, 200.0);
        assert!(count_visible_triangles(&cache.ink_chunks, vis_left) > 0);
        assert_eq!(count_visible_triangles(&cache.ink_chunks, vis_far), 0);
    }

    #[test]
    fn gpu_chunk_splits_beyond_max_chunk_strokes_and_detects_new_tail_chunk() {
        let color_key = (UiTheme::Light, true, PaperColor::PureWhite);
        let mut cache = GpuStrokeSceneCache::default();
        let mut ids = Vec::with_capacity(MAX_CHUNK_STROKES + 1);
        for i in 1..=(MAX_CHUNK_STROKES as u64) {
            let s = make_test_stroke(i, PenKind::BallPen, (i as f64) * 5.0, 6);
            ids.push(s.id);
            cache
                .strokes
                .insert(s.id, build_cached_scalable_stroke(&s, color_key));
        }
        sync_chunk_list(&mut cache, false, &ids);
        assert_eq!(cache.ink_chunks.len(), 1);
        let first_chunk_id = cache.ink_chunks[0].id;

        let extra = make_test_stroke((MAX_CHUNK_STROKES + 1) as u64, PenKind::BallPen, 900.0, 6);
        ids.push(extra.id);
        cache
            .strokes
            .insert(extra.id, build_cached_scalable_stroke(&extra, color_key));
        sync_chunk_list(&mut cache, false, &ids);
        assert_eq!(cache.ink_chunks.len(), 2);
        assert_eq!(cache.ink_chunks[0].id, first_chunk_id);
        assert_eq!(cache.ink_chunks[1].strokes.len(), 1);
        let tail = detect_single_append(&[first_chunk_id], &cache.ink_chunks);
        assert_eq!(tail.map(|c| c.id), Some(cache.ink_chunks[1].id));
    }

    #[test]
    fn gpu_hl_chunks_and_overlay_track_per_stroke_ranges_and_disjoint_scissors() {
        let color_key = (UiTheme::Light, true, PaperColor::PureWhite);
        let mut cache = GpuStrokeSceneCache::default();
        let h1 = make_test_stroke(1, PenKind::Highlighter, 20.0, 12);
        let h2 = make_test_stroke(2, PenKind::Highlighter, 300.0, 12);
        cache
            .strokes
            .insert(h1.id, build_cached_scalable_stroke(&h1, color_key));
        cache
            .strokes
            .insert(h2.id, build_cached_scalable_stroke(&h2, color_key));

        sync_chunk_list(&mut cache, true, &[h1.id, h2.id]);
        assert_eq!(cache.hl_chunks.len(), 1);
        let chunk = &cache.hl_chunks[0];
        assert_eq!(chunk.stroke_ranges.len(), 2);
        assert!(chunk.stroke_ranges[0].2 > 0);
        assert!(chunk.stroke_ranges[1].2 > 0);

        let tex_size = [1280, 800];
        let s1 = page_bounds_to_scissor(chunk.stroke_ranges[0].0, 1.0, [0.0, 0.0], 4.0, tex_size)
            .unwrap();
        let s2 = page_bounds_to_scissor(chunk.stroke_ranges[1].0, 1.0, [0.0, 0.0], 4.0, tex_size)
            .unwrap();
        assert!(scissors_disjoint(s1, s2));
        assert!(!scissors_disjoint(s1, s1));

        let live_entry = &cache.strokes[&h2.id];
        let overlay = build_overlay_highlighters(
            &cache,
            &[h1.id],
            Some((
                &live_entry.mesh,
                &live_entry.packed_colors,
                live_entry.bounds,
            )),
        )
        .expect("expected combined dragged + live overlay highlighters");
        assert_eq!(overlay.ranges.len(), 2);
        assert!(overlay.ranges[0].2 > 0);
        assert!(overlay.ranges[1].2 > 0);
    }
}
