//! Draws a project preview (contracts' Docs/project-preview.md) on the window's device: its meshes in Unity's own
//! left-handed world, lit by the scene's own lights when Toolkit recorded them (its directional lights, the main one
//! casting its shadow, its ambient light and reflections) or by a studio as bright as Unity's default scene (its
//! directional light as the key, whose soft shadow falls on the scene and on a ground of our own, the default sky's
//! ambient light and reflections, a faint cool fill and rim for the side away from it), each material as its shader takes the light (Standard's metallic and smoothness, toon
//! bands, flat, unlit; normal maps, emission), the scene mirrored faintly in that ground, a sweep that reveals it from
//! the ground up, then 4× multisampling resolved into a texture egui paints like any picture (squircle corners, fading
//! over the avatar pictures). Loading runs on a worker: one file read, pictures decoded on every core
//! with their mipmaps, vertex and index blobs uploaded as they are, while the pipelines compile on another thread.

use eframe::egui_wgpu::wgpu;
use orbiters_projects::preview::{self, Alpha, Cull, ImageFormat, Kind, Preview, Shading};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use wgpu::util::DeviceExt as _;

const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Normal and metallic-smoothness pictures: data, not colours.
const DATA: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const SAMPLES: u32 = 4;
const SHADOW_SIZE: u32 = 2048;

/// Orbiters green, linear: the reveal's edge.
const ACCENT: [f32; 3] = [0.0, 0.701, 0.153];

/// The layouts every scene's bind groups use (made at once on the loading thread; the pipelines compile meanwhile).
pub(super) struct Layouts {
    frame: wgpu::BindGroupLayout,
    shadow_frame: wgpu::BindGroupLayout,
    material: wgpu::BindGroupLayout,
}

impl Layouts {
    fn new(device: &wgpu::Device) -> Self {
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let both = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let frame = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview frame"),
            entries: &[
                uniform(0, both),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let shadow_frame =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("preview shadow frame"), entries: &[uniform(0, both)] });
        let picture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        // The base colour, the sampler, then the normal, metallic-smoothness and emission pictures.
        let material = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview material"),
            entries: &[
                uniform(0, both),
                picture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                picture(3),
                picture(4),
                picture(5),
            ],
        });
        Self { frame, shadow_frame, material }
    }
}

/// The render pipelines, made once per device (the first preview of a session compiles them, beside its loading).
pub(super) struct Pipelines {
    layouts: Arc<Layouts>,
    background: wgpu::RenderPipeline,
    ground: wgpu::RenderPipeline,
    /// By cull mode (back, front, none): opaque and cut-out, see-through, mirrored in the ground.
    opaque: [wgpu::RenderPipeline; 3],
    blend: [wgpu::RenderPipeline; 3],
    mirror: [wgpu::RenderPipeline; 3],
    shadow: wgpu::RenderPipeline,
}

/// Index of a cull mode in the pipeline arrays.
fn cull_index(c: Option<wgpu::Face>) -> usize {
    match c {
        Some(wgpu::Face::Back) => 0,
        Some(wgpu::Face::Front) => 1,
        None => 2,
    }
}

impl Pipelines {
    fn new(device: &wgpu::Device, layouts: Arc<Layouts>) -> Self {
        let module =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("project preview"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview scene"),
            bind_group_layouts: &[Some(&layouts.frame), Some(&layouts.material)],
            immediate_size: 0,
        });
        let frame_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview frame"),
            bind_group_layouts: &[Some(&layouts.frame)],
            immediate_size: 0,
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview shadow"),
            bind_group_layouts: &[Some(&layouts.shadow_frame), Some(&layouts.material)],
            immediate_size: 0,
        });
        let vertex_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: preview::VERTEX_SIZE as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Snorm16x2, offset: 12, shader_location: 1 },
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float16x2, offset: 16, shader_location: 2 },
                ],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &[
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 3 },
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 4 },
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 5 },
                    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 48, shader_location: 6 },
                ],
            }),
        ];
        let over = Some(wgpu::BlendState::ALPHA_BLENDING);
        let depth = |write: bool, compare| {
            Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: Default::default(),
                bias: Default::default(),
            })
        };
        let msaa = wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false };
        let scene = |label: &str, cull: Option<wgpu::Face>, fs: &str, blend: Option<wgpu::BlendState>, write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&scene_layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_scene"), buffers: &vertex_buffers, compilation_options: Default::default() },
                // Unity's front faces are clockwise.
                primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Cw, cull_mode: cull, ..Default::default() },
                depth_stencil: depth(write, wgpu::CompareFunction::GreaterEqual),
                multisample: msaa,
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState { format: COLOR, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let culls = [Some(wgpu::Face::Back), Some(wgpu::Face::Front), None];
        let opaque = culls.map(|c| scene("preview opaque", c, "fs_scene", None, true));
        let blend = culls.map(|c| scene("preview blend", c, "fs_scene", over, false));
        let mirror = culls.map(|c| scene("preview mirror", c, "fs_mirror", over, true));
        let flat = |label: &str, vs: &str, fs: &str, strip: bool, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&frame_layout),
                vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), buffers: &[], compilation_options: Default::default() },
                primitive: wgpu::PrimitiveState {
                    topology: if strip { wgpu::PrimitiveTopology::TriangleStrip } else { wgpu::PrimitiveTopology::TriangleList },
                    ..Default::default()
                },
                depth_stencil: depth(false, wgpu::CompareFunction::Always),
                multisample: msaa,
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    targets: &[Some(wgpu::ColorTargetState { format: COLOR, blend, write_mask: wgpu::ColorWrites::ALL })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let background = flat("preview background", "vs_full", "fs_background", false, None);
        let ground = flat("preview ground", "vs_ground", "fs_ground", true, over);
        let shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview shadow"),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_shadow"), buffers: &vertex_buffers, compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_shadow"), targets: &[], compilation_options: Default::default() }),
            multiview_mask: None,
            cache: None,
        });
        Self { layouts, background, ground, opaque, blend, mirror, shadow }
    }
}

/// Where the camera is and what moves (the parts of a frame that change).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct View {
    pub eye: [f32; 3],
    pub forward: [f32; 3],
    pub fov_y: f32,
    /// How far up the picture moves (a lens shift, in halves of its height): what it frames clears what lies over the
    /// view's lower part.
    pub shift: f32,
    /// 0 to 1 while the scene rises out of the ground, 1 once shown.
    pub reveal: f32,
    /// The scene's light or the studio's.
    pub light: LightMode,
}

/// How a preview is lit: by the lights its scene has in Unity (contracts' Docs/project-preview.md, `lighting`) or by
/// the desktop's studio, the same for every preview.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum LightMode {
    #[default]
    Studio,
    Scene,
}

/// The light a preview is drawn in: the scene's when the person prefers it and the file recorded it, else the studio's.
pub(super) fn light_mode(prefer_scene: bool, h: &preview::Header) -> LightMode {
    if prefer_scene && h.lighting.is_some() {
        LightMode::Scene
    } else {
        LightMode::Studio
    }
}

/// What the loading thread measured (docs/project-preview.md).
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Timings {
    pub read_ms: f32,
    pub decode_ms: f32,
    pub upload_ms: f32,
    pub pipelines_ms: f32,
    pub total_ms: f32,
    pub bytes: usize,
}

struct Material {
    bind_group: wgpu::BindGroup,
    alpha: Alpha,
    cull: Cull,
}

struct Draw {
    mesh: usize,
    instance: u32,
    start: u32,
    count: u32,
    material: usize,
    /// The instance's matrix mirrors (its winding flips).
    mirrored: bool,
    shadows: bool,
    center: [f32; 3],
}

struct MeshBuffers {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    format: wgpu::IndexFormat,
}

struct Targets {
    size: (u32, u32),
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    resolve: wgpu::TextureView,
    /// The resolved picture as egui samples it (its bytes, encoded sRGB).
    shown: wgpu::TextureView,
}

/// A loaded preview on the GPU.
pub(super) struct Scene {
    pub header: preview::Header,
    pub timings: Timings,
    pipelines: Arc<Pipelines>,
    meshes: Vec<MeshBuffers>,
    instances: wgpu::Buffer,
    materials: Vec<Material>,
    draws: Vec<Draw>,
    /// See-through draws, far to near for the last view.
    blend_order: Vec<usize>,
    frame: wgpu::Buffer,
    mirror_frame: wgpu::Buffer,
    shadow_frame: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    mirror_group: wgpu::BindGroup,
    shadow_group: wgpu::BindGroup,
    shadow_view: wgpu::TextureView,
    /// The reveal the shadow map was last drawn at.
    shadow_reveal: Option<f32>,
    light: Light,
    light_mode: LightMode,
    targets: Option<Targets>,
    texture: Option<(egui::TextureId, orbiters_ui::blur::Textures)>,
    last: Option<(View, (u32, u32))>,
}

impl Drop for Scene {
    fn drop(&mut self) {
        if let Some((id, textures)) = self.texture.take() {
            textures.write().free_texture(&id);
        }
    }
}

/// The lights a preview is drawn in, and the key light's shadow projection.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Light {
    /// Towards the key light (the scene's main one, or the studio's).
    direction: [f32; 3],
    /// Linear, as Unity gives shaders `_LightColor0`.
    color: [f32; 3],
    /// How dark the key light's shadow is on the scene, and on our ground.
    shadow: f32,
    ground_shadow: f32,
    /// Towards a second directional light, and its colour (black: none).
    second: [f32; 3],
    second_color: [f32; 3],
    /// The studio's fill (towards it), the studio's alone.
    fill: [f32; 3],
    studio: bool,
    /// The ambient light facing up, round the horizon, facing down (linear).
    ambient: [[f32; 3]; 3],
    reflection: f32,
    view_proj: [f32; 16],
}

/// The studio's ambient light: Unity's default procedural sky's, facing up, round the horizon, facing down.
const STUDIO_AMBIENT: [[f32; 3]; 3] = [[0.25, 0.31, 0.43], [0.185, 0.215, 0.28], [0.12, 0.12, 0.13]];
/// Unity's default directional light (colour 1, 0.956, 0.839, intensity 1).
const STUDIO_SUN: [f32; 3] = [1.0, 0.956, 0.839];

/// Reads, decodes and uploads the preview at `path` on `device`, compiling the pipelines beside it when `pipelines`
/// has none yet.
pub(super) fn load(device: &wgpu::Device, queue: &wgpu::Queue, pipelines: Option<Arc<Pipelines>>, path: &Path) -> anyhow::Result<Scene> {
    let start = std::time::Instant::now();
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f32() * 1000.0;
    std::thread::scope(|s| {
        let compiling = pipelines.is_none().then(|| {
            s.spawn(|| {
                let t = std::time::Instant::now();
                let layouts = Arc::new(Layouts::new(device));
                (Arc::new(Pipelines::new(device, layouts)), ms(t))
            })
        });
        let t = std::time::Instant::now();
        let p = preview::read(path)?;
        let read_ms = ms(t);
        let t = std::time::Instant::now();
        let textures = decode_and_upload(device, queue, &p);
        let decode_ms = ms(t);
        let (pipelines, pipelines_ms) = match (pipelines, compiling) {
            (Some(p), _) => (p, 0.0),
            (None, Some(job)) => job.join().map_err(|_| anyhow::anyhow!("the preview's pipelines could not be made"))?,
            (None, None) => unreachable!(),
        };
        let t = std::time::Instant::now();
        let mut scene = upload(device, queue, &p, &pipelines, textures);
        scene.timings = Timings { read_ms, decode_ms, upload_ms: ms(t), pipelines_ms, total_ms: ms(start), bytes: p.size() };
        Ok(scene)
    })
}

/// Every picture decoded with its mipmaps and uploaded, on all cores; a picture that cannot be read is left white.
fn decode_and_upload(device: &wgpu::Device, queue: &wgpu::Queue, p: &Preview) -> Vec<Option<wgpu::TextureView>> {
    let list = &p.header.textures;
    let next = AtomicUsize::new(0);
    let out: Vec<std::sync::Mutex<Option<wgpu::TextureView>>> = list.iter().map(|_| Default::default()).collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8).min(list.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(t) = list.get(i) else { break };
                let format = match t.format {
                    ImageFormat::Jpeg => image::ImageFormat::Jpeg,
                    ImageFormat::Png => image::ImageFormat::Png,
                };
                match image::load_from_memory_with_format(p.blob(t.data), format) {
                    Ok(img) => *out[i].lock().unwrap() = Some(upload_texture(device, queue, img.to_rgba8(), if t.linear { DATA } else { COLOR })),
                    Err(e) => eprintln!("project preview: picture {i}: {e}"),
                }
            });
        }
    });
    out.into_iter().map(|m| m.into_inner().unwrap()).collect()
}

/// A texture with its mip chain (box-filtered on the CPU), sRGB colours or linear data by `format`.
fn upload_texture(device: &wgpu::Device, queue: &wgpu::Queue, img: image::RgbaImage, format: wgpu::TextureFormat) -> wgpu::TextureView {
    let (w, h) = img.dimensions();
    let levels = 32 - w.max(h).max(1).leading_zeros();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("preview picture"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let (mut level, mut lw, mut lh) = (img.into_raw(), w, h);
    for mip in 0..levels {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: mip, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &level,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(lw * 4), rows_per_image: Some(lh) },
            wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 },
        );
        if mip + 1 < levels {
            (level, lw, lh) = half_size(&level, lw, lh);
        }
    }
    texture.create_view(&Default::default())
}

/// The next mip level: each pixel the average of up to four (alpha-weighted colours, so cut-out edges keep their
/// colour instead of darkening).
fn half_size(src: &[u8], w: u32, h: u32) -> (Vec<u8>, u32, u32) {
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; (nw * nh * 4) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let mut sum = [0u32; 4];
            let mut weight = 0u32;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (sx, sy) = ((x * 2 + dx).min(w - 1), (y * 2 + dy).min(h - 1));
                let i = ((sy * w + sx) * 4) as usize;
                let a = src[i + 3] as u32;
                for k in 0..3 {
                    sum[k] += src[i + k] as u32 * (a + 1);
                }
                sum[3] += a;
                weight += a + 1;
            }
            let o = ((y * nw + x) * 4) as usize;
            for k in 0..3 {
                out[o + k] = (sum[k] / weight.max(1)) as u8;
            }
            out[o + 3] = (sum[3] / 4) as u8;
        }
    }
    (out, nw, nh)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn upload(device: &wgpu::Device, queue: &wgpu::Queue, p: &Preview, pipelines: &Arc<Pipelines>, textures: Vec<Option<wgpu::TextureView>>) -> Scene {
    let h = &p.header;
    let layouts = &pipelines.layouts;
    let meshes: Vec<MeshBuffers> = h
        .meshes
        .iter()
        .map(|m| MeshBuffers {
            vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("preview vertices"),
                contents: nonempty(p.blob(m.data)),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("preview indices"),
                contents: nonempty(p.blob(m.indices)),
                usage: wgpu::BufferUsages::INDEX,
            }),
            format: match m.index_format {
                preview::IndexFormat::U16 => wgpu::IndexFormat::Uint16,
                preview::IndexFormat::U32 => wgpu::IndexFormat::Uint32,
            },
        })
        .collect();
    let instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("preview instances"),
        contents: nonempty(p.blob(h.instances.matrices)),
        usage: wgpu::BufferUsages::VERTEX,
    });

    let white = upload_white(device, queue);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("preview picture"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    });
    let materials: Vec<Material> = h
        .materials
        .iter()
        .map(|m| {
            let picture = |i: i32| usize::try_from(i).ok().and_then(|i| textures.get(i)).and_then(|t| t.as_ref());
            let (tex, normal, surface, emission) = (picture(m.texture), picture(m.normal_texture), picture(m.surface_texture), picture(m.emission_texture));
            let uniform = material_uniform(m, tex.is_some(), normal.is_some(), surface.is_some(), emission.is_some());
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("preview material"),
                contents: bytemuck::cast_slice(&uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("preview material"),
                layout: &layouts.material,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(tex.unwrap_or(&white)) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(normal.unwrap_or(&white)) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(surface.unwrap_or(&white)) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(emission.unwrap_or(&white)) },
                ],
            });
            Material { bind_group, alpha: m.alpha, cull: m.cull }
        })
        .collect();

    let mut draws = Vec::new();
    for d in &h.draws {
        let m = p.matrix(d.instance);
        let det = m[0] * (m[5] * m[10] - m[6] * m[9]) - m[4] * (m[1] * m[10] - m[2] * m[9]) + m[8] * (m[1] * m[6] - m[2] * m[5]);
        let mesh = &h.meshes[d.mesh as usize];
        let c = mesh.bounds.center();
        let center = [0, 1, 2].map(|k| m[k] * c[0] + m[4 + k] * c[1] + m[8 + k] * c[2] + m[12 + k]);
        for (sub, &material) in mesh.submeshes.iter().zip(&d.materials) {
            if material < 0 || sub.count == 0 {
                continue;
            }
            draws.push(Draw {
                mesh: d.mesh as usize,
                instance: d.instance,
                start: sub.start,
                count: sub.count,
                material: material as usize,
                mirrored: det < 0.0,
                shadows: d.shadows,
                center,
            });
        }
    }
    // Opaque draws grouped by material.
    draws.sort_by_key(|d| (materials[d.material].alpha == Alpha::Blend, d.material, d.mesh));
    let blend_order = (0..draws.len()).filter(|&i| materials[draws[i].material].alpha == Alpha::Blend).collect();

    let frame_buffer = |label| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: FRAME_SIZE as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let (frame, mirror_frame, shadow_frame) = (frame_buffer("preview frame"), frame_buffer("preview mirror"), frame_buffer("preview shadow"));
    let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("preview shadow map"),
        size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let shadow_view = shadow_texture.create_view(&Default::default());
    let compare = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("preview shadow"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });
    let group = |buffer: &wgpu::Buffer| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preview frame"),
            layout: &layouts.frame,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&shadow_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&compare) },
            ],
        })
    };
    let (frame_group, mirror_group) = (group(&frame), group(&mirror_frame));
    let shadow_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("preview shadow frame"),
        layout: &layouts.shadow_frame,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: shadow_frame.as_entire_binding() }],
    });
    Scene {
        light: light_for(h, LightMode::Studio),
        light_mode: LightMode::Studio,
        header: h.clone(),
        timings: Timings::default(),
        pipelines: pipelines.clone(),
        meshes,
        instances,
        materials,
        draws,
        blend_order,
        frame,
        mirror_frame,
        shadow_frame,
        frame_group,
        mirror_group,
        shadow_group,
        shadow_view,
        shadow_reveal: None,
        targets: None,
        texture: None,
        last: None,
    }
}

/// A material's uniform (the shader's `Material`): colour, tiling, then how it takes the light. A picture the file
/// names but that could not be read counts as absent.
fn material_uniform(m: &preview::Material, tex: bool, normal: bool, surface: bool, emission: bool) -> [f32; 28] {
    let c = m.color;
    let alpha = match m.alpha {
        Alpha::Opaque => 0.0f32,
        Alpha::Cutout => 1.0,
        Alpha::Blend => 2.0,
    };
    let shading = match m.shading {
        Shading::Lit => 0.0f32,
        Shading::Toon => 1.0,
        Shading::Flat => 2.0,
        Shading::Unlit => 3.0,
    };
    let flag = |b: bool| if b { 1.0f32 } else { 0.0 };
    let finite = |v: f32, d: f32| if v.is_finite() { v } else { d };
    let e = m.emission.map(|v| finite(v, 0.0).clamp(0.0, 64.0));
    let shade = m.shade.map(|v| finite(v, 0.7).clamp(0.0, 1.0));
    [
        srgb_to_linear(c[0]),
        srgb_to_linear(c[1]),
        srgb_to_linear(c[2]),
        c[3],
        m.tiling[0],
        m.tiling[1],
        m.offset[0],
        m.offset[1],
        m.cutoff,
        alpha,
        flag(tex),
        shading,
        finite(m.metallic, 0.0).clamp(0.0, 1.0),
        finite(m.smoothness, 0.2).clamp(0.0, 1.0),
        if normal { finite(m.normal_scale, 1.0).clamp(-4.0, 4.0) } else { 0.0 },
        flag(surface),
        e[0],
        e[1],
        e[2],
        flag(emission),
        shade[0],
        shade[1],
        shade[2],
        finite(m.shade_border, 0.5).clamp(0.0, 1.0),
        finite(m.shade_blur, 0.1).clamp(0.0, 1.0),
        flag(normal),
        0.0,
        0.0,
    ]
}

/// wgpu refuses empty buffers.
fn nonempty(b: &[u8]) -> &[u8] {
    if b.is_empty() {
        &[0; 16]
    } else {
        b
    }
}

fn upload_white(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("preview white"),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &[255; 4],
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
        wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
    );
    texture.create_view(&Default::default())
}

/// The lights of `mode`: the studio's key from the front left and above the first avatar (else a fixed direction),
/// or the scene's recorded lights (the main one the key, a second one beside it, its ambient light; the studio's
/// ambient light when the file has none); and the key's shadow map's orthographic projection around what the preview
/// frames.
fn light_for(h: &preview::Header, mode: LightMode) -> Light {
    let forward = h.avatars.first().map(|a| a.forward).filter(|f| f[0].abs() + f[2].abs() > 1e-3).unwrap_or([0.0, 0.0, 1.0]);
    let f = normalize([forward[0], 0.0, forward[2]]);
    // Right of something facing f, in Unity's left-handed world: up × f.
    let right = [f[2], 0.0, -f[0]];
    let studio_dir = normalize([f[0] * 0.75 - right[0] * 0.6, 1.25, f[2] * 0.75 - right[2] * 0.6]);
    let fill = normalize([-f[0] * 0.3 + right[0] * 0.9, 0.35, -f[2] * 0.3 + right[2] * 0.9]);
    let mut light = Light {
        direction: studio_dir,
        color: STUDIO_SUN,
        shadow: 0.65,
        ground_shadow: 1.0,
        second: [0.0, 1.0, 0.0],
        second_color: [0.0; 3],
        fill,
        studio: true,
        ambient: STUDIO_AMBIENT,
        reflection: 1.0,
        view_proj: [0.0; 16],
    };
    if let (LightMode::Scene, Some(l)) = (mode, &h.lighting) {
        let colour = |c: [f32; 3]| c.map(|v| if v.is_finite() { v.clamp(0.0, 16.0) } else { 0.0 });
        // Towards the light (the file has the way it travels); none that cannot be pointed.
        let towards = |d: [f32; 3]| {
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            (len.is_finite() && len > 1e-6).then(|| [-d[0] / len, -d[1] / len, -d[2] / len])
        };
        let mut lights = l.lights.iter().filter_map(|s| towards(s.direction).map(|d| (d, s)));
        light.studio = false;
        light.fill = [0.0; 3];
        match lights.next() {
            Some((d, key)) => {
                light.direction = d;
                light.color = colour(key.color);
                let strength = if key.shadows && key.shadow_strength.is_finite() { key.shadow_strength.clamp(0.0, 1.0) } else { 0.0 };
                (light.shadow, light.ground_shadow) = (strength, strength);
            }
            None => {
                // Only the ambient light, as in Unity; the shadow map keeps the studio's direction, unused.
                light.color = [0.0; 3];
                (light.shadow, light.ground_shadow) = (0.0, 0.0);
            }
        }
        if let Some((d, second)) = lights.next() {
            light.second = d;
            light.second_color = colour(second.color);
        }
        if let Some(a) = l.ambient {
            light.ambient = [colour(a.sky), colour(a.equator), colour(a.ground)];
        }
        light.reflection = if l.reflection_intensity.is_finite() { l.reflection_intensity.clamp(0.0, 4.0) } else { 1.0 };
    }
    let dir = light.direction;
    let box_ = if h.kind == Kind::Avatars { h.focus } else { h.bounds };
    let center = box_.center();
    let radius = box_.radius().clamp(0.5, 120.0) * if h.kind == Kind::Avatars { 1.6 } else { 1.05 };
    // Looking along -dir from beyond the scene.
    let eye = [center[0] + dir[0] * radius * 2.0, center[1] + dir[1] * radius * 2.0, center[2] + dir[2] * radius * 2.0];
    let view = look_to(eye, [-dir[0], -dir[1], -dir[2]]);
    let proj = ortho(radius, radius, 0.01, radius * 4.0);
    light.view_proj = mul(&proj, &view);
    light
}

/// Bytes of the frame uniform (two matrices, seventeen vectors).
const FRAME_SIZE: usize = 64 * 2 + 16 * 17;

/// How an orbit's frame is drawn: the background and ground looks of the preview's kind.
struct Look {
    sky_top: [f32; 3],
    sky_bottom: [f32; 3],
    ground: bool,
}

impl Scene {
    /// The pipelines it draws with (the next preview reuses them).
    pub fn pipelines(&self) -> Arc<Pipelines> {
        self.pipelines.clone()
    }

    /// Draws `view` at `size` pixels if it changed since the last frame drawn, then returns the texture and the part of
    /// it the frame covers.
    pub fn render(&mut self, gpu: &crate::gpu::Handles, size: (u32, u32), view: View) -> (egui::TextureId, egui::Rect) {
        let device = &gpu.device;
        let size = (size.0.clamp(16, 8192), size.1.clamp(16, 8192));
        // Grows to what is asked, shrinks once far larger than needed (a window closing back into its card keeps the
        // large one until it lands).
        let grow = self.targets.as_ref().is_none_or(|t| t.size.0 < size.0 || t.size.1 < size.1);
        let shrink = self.targets.as_ref().is_some_and(|t| (t.size.0 as u64 * t.size.1 as u64) > 6 * size.0 as u64 * size.1 as u64);
        if grow || shrink {
            let alloc = if grow { (size.0.next_multiple_of(64), size.1.next_multiple_of(64)) } else { size };
            let t = targets(device, alloc);
            match &self.texture {
                Some((id, textures)) => textures.write().update_egui_texture_from_wgpu_texture(device, &t.shown, wgpu::FilterMode::Linear, *id),
                None => {
                    let id = gpu.textures.write().register_native_texture(device, &t.shown, wgpu::FilterMode::Linear);
                    self.texture = Some((id, gpu.textures.clone()));
                }
            }
            self.targets = Some(t);
            self.last = None;
        }
        let t = self.targets.as_ref().unwrap();
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(size.0 as f32 / t.size.0 as f32, size.1 as f32 / t.size.1 as f32));
        let id = self.texture.as_ref().unwrap().0;
        if self.last == Some((view, size)) {
            return (id, uv);
        }
        if view.light != self.light_mode {
            self.light = light_for(&self.header, view.light);
            self.light_mode = view.light;
            self.shadow_reveal = None;
        }
        self.last = Some((view, size));
        self.draw(gpu, size, view);
        (id, uv)
    }

    fn look(&self) -> Look {
        match self.header.kind {
            Kind::Avatars => Look { sky_top: [0.0105, 0.0105, 0.0125], sky_bottom: [0.0045, 0.0045, 0.0055], ground: true },
            _ => Look { sky_top: [0.030, 0.034, 0.045], sky_bottom: [0.006, 0.006, 0.008], ground: false },
        }
    }

    fn draw(&mut self, gpu: &crate::gpu::Handles, size: (u32, u32), view: View) {
        let h = &self.header;
        let look = self.look();
        let focus = if h.kind == Kind::Avatars { h.focus } else { h.bounds };
        let center = focus.center();
        let radius = focus.radius().max(0.05);
        let ground_y = h.ground.unwrap_or(focus.min[1]);
        let (w, hgt) = size;
        let aspect = w as f32 / hgt as f32;
        let near = (radius * 0.002).clamp(0.003, 0.05);
        let view_m = look_to(view.eye, view.forward);
        let proj = perspective(view.fov_y, aspect, near, view.shift);
        let view_proj = mul(&proj, &view_m);
        // The reveal: a plane rising from the ground to above everything, its edge glowing.
        let height = (focus.max[1] - ground_y).max(0.1);
        let band = (height * 0.06).clamp(0.02, 2.0);
        let k = orbiters_ui::anim::ease::in_out(view.reveal);
        let reveal_y = if view.reveal >= 1.0 { f32::MAX } else { ground_y - band + (height + band * 2.5) * k };
        let ground_radius = radius * 4.0;
        let light = self.light;
        let uniform = |vp: &[f32; 16], eye: [f32; 3], mirror: bool| -> Vec<f32> {
            let mut u = Vec::with_capacity(FRAME_SIZE / 4);
            u.extend_from_slice(vp);
            u.extend_from_slice(&light.view_proj);
            u.extend_from_slice(&[eye[0], eye[1], eye[2], if mirror { 1.0 } else { 0.0 }]);
            u.extend_from_slice(&[light.direction[0], light.direction[1], light.direction[2], light.shadow]);
            u.extend_from_slice(&[light.fill[0], light.fill[1], light.fill[2], 0.0]);
            u.extend_from_slice(&[ground_y, ground_radius, if look.ground { 1.0 } else { 0.0 }, 0.22]);
            u.extend_from_slice(&[reveal_y, band, view.reveal, (1.0 - view.reveal).clamp(0.0, 1.0)]);
            u.extend_from_slice(&[center[0], center[1], center[2], radius]);
            u.extend_from_slice(&[w as f32, hgt as f32, aspect, 0.0]);
            u.extend_from_slice(&[look.sky_top[0], look.sky_top[1], look.sky_top[2], 0.0]);
            u.extend_from_slice(&[look.sky_bottom[0], look.sky_bottom[1], look.sky_bottom[2], 0.0]);
            u.extend_from_slice(&[ACCENT[0], ACCENT[1], ACCENT[2], 0.0]);
            // Where the subject is on screen (the spot behind it).
            let c = transform(&view_proj, [center[0], center[1], center[2]]);
            u.extend_from_slice(&[c[0] * 0.5 + 0.5, 0.5 - c[1] * 0.5, 0.0, 0.0]);
            let [r, g, b] = light.color;
            u.extend_from_slice(&[r, g, b, if light.studio { 1.0 } else { 0.0 }]);
            u.extend_from_slice(&[light.second[0], light.second[1], light.second[2], 0.0]);
            u.extend_from_slice(&[light.second_color[0], light.second_color[1], light.second_color[2], 0.0]);
            let [sky, equator, ground] = light.ambient;
            u.extend_from_slice(&[sky[0], sky[1], sky[2], light.ground_shadow]);
            u.extend_from_slice(&[equator[0], equator[1], equator[2], light.reflection]);
            u.extend_from_slice(&[ground[0], ground[1], ground[2], 0.0]);
            u
        };
        let q = &gpu.queue;
        q.write_buffer(&self.frame, 0, bytemuck::cast_slice(&uniform(&view_proj, view.eye, false)));
        // Mirrored in the ground: y → 2g − y.
        let reflect: [f32; 16] = [1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0 * ground_y, 0.0, 1.0];
        let mirrored_eye = [view.eye[0], 2.0 * ground_y - view.eye[1], view.eye[2]];
        q.write_buffer(&self.mirror_frame, 0, bytemuck::cast_slice(&uniform(&mul(&view_proj, &reflect), mirrored_eye, true)));
        let shadow_needed = self.shadow_reveal != Some(reveal_y);
        if shadow_needed {
            q.write_buffer(&self.shadow_frame, 0, bytemuck::cast_slice(&uniform(&light.view_proj, view.eye, false)));
        }

        // See-through draws far to near.
        let eye = view.eye;
        let dist = |d: &Draw| (0..3).map(|k| (d.center[k] - eye[k]).powi(2)).sum::<f32>();
        let draws = &self.draws;
        self.blend_order.sort_by(|&a, &b| dist(&draws[b]).total_cmp(&dist(&draws[a])));

        let mut encoder = gpu.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("project preview") });
        let pl = &self.pipelines;
        if shadow_needed {
            self.shadow_reveal = Some(reveal_y);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&pl.shadow);
            pass.set_bind_group(0, &self.shadow_group, &[]);
            pass.set_vertex_buffer(1, self.instances.slice(..));
            for d in self.draws.iter().filter(|d| d.shadows) {
                self.draw_one(&mut pass, d);
            }
        }
        let t = self.targets.as_ref().unwrap();
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.color,
                    depth_slice: None,
                    resolve_target: Some(&t.resolve),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_viewport(0.0, 0.0, w as f32, hgt as f32, 0.0, 1.0);
            pass.set_scissor_rect(0, 0, w, hgt);
            pass.set_pipeline(&pl.background);
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.draw(0..3, 0..1);
            pass.set_vertex_buffer(1, self.instances.slice(..));
            if look.ground {
                pass.set_pipeline(&pl.ground);
                pass.draw(0..4, 0..1);
                // The reflection: every draw mirrored (its winding flips), fading with height above the ground.
                pass.set_bind_group(0, &self.mirror_group, &[]);
                for d in &self.draws {
                    pass.set_pipeline(&pl.mirror[self.cull_of(d, true)]);
                    self.draw_one(&mut pass, d);
                }
                pass.set_bind_group(0, &self.frame_group, &[]);
            }
            for d in self.draws.iter().filter(|d| self.materials[d.material].alpha != Alpha::Blend) {
                pass.set_pipeline(&pl.opaque[self.cull_of(d, false)]);
                self.draw_one(&mut pass, d);
            }
            for &i in &self.blend_order {
                let d = &self.draws[i];
                pass.set_pipeline(&pl.blend[self.cull_of(d, false)]);
                self.draw_one(&mut pass, d);
            }
        }
        gpu.queue.submit([encoder.finish()]);
    }

    fn cull_of(&self, d: &Draw, mirror: bool) -> usize {
        let face = match self.materials[d.material].cull {
            Cull::Off => None,
            Cull::Back => Some(wgpu::Face::Back),
            Cull::Front => Some(wgpu::Face::Front),
        };
        let flip = d.mirrored != mirror;
        cull_index(match (face, flip) {
            (Some(wgpu::Face::Back), true) => Some(wgpu::Face::Front),
            (Some(wgpu::Face::Front), true) => Some(wgpu::Face::Back),
            (f, _) => f,
        })
    }

    fn draw_one(&self, pass: &mut wgpu::RenderPass<'_>, d: &Draw) {
        let m = &self.meshes[d.mesh];
        pass.set_bind_group(1, &self.materials[d.material].bind_group, &[]);
        pass.set_vertex_buffer(0, m.vertices.slice(..));
        pass.set_index_buffer(m.indices.slice(..), m.format);
        pass.draw_indexed(d.start..d.start + d.count, 0, d.instance..d.instance + 1);
    }
}

fn targets(device: &wgpu::Device, size: (u32, u32)) -> Targets {
    let extent = wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 };
    let make = |label, format, samples, usage, view_formats: &[wgpu::TextureFormat]| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats,
        })
    };
    let color = make("preview color", COLOR, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT, &[]);
    let depth = make("preview depth", DEPTH, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT, &[]);
    let resolve =
        make("preview picture", COLOR, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, &[wgpu::TextureFormat::Rgba8Unorm]);
    let shown = resolve.create_view(&wgpu::TextureViewDescriptor { format: Some(wgpu::TextureFormat::Rgba8Unorm), ..Default::default() });
    Targets {
        size,
        color: color.create_view(&Default::default()),
        depth: depth.create_view(&Default::default()),
        resolve: resolve.create_view(&Default::default()),
        shown,
    }
}

pub(super) fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// A left-handed view matrix (column-major): +X right, +Y up, +Z along `forward`.
fn look_to(eye: [f32; 3], forward: [f32; 3]) -> [f32; 16] {
    let f = normalize(forward);
    let up = if f[1].abs() > 0.999 { [0.0, 0.0, 1.0] } else { [0.0, 1.0, 0.0] };
    let r = normalize(cross(up, f));
    let u = cross(f, r);
    [r[0], u[0], f[0], 0.0, r[1], u[1], f[1], 0.0, r[2], u[2], f[2], 0.0, -dot(r, eye), -dot(u, eye), -dot(f, eye), 1.0]
}

/// Left-handed perspective with reversed depth to infinity (`near` at 1, the horizon at 0), the picture moved up by
/// `shift`.
fn perspective(fov_y: f32, aspect: f32, near: f32, shift: f32) -> [f32; 16] {
    let f = 1.0 / (fov_y * 0.5).tan();
    [f / aspect, 0.0, 0.0, 0.0, 0.0, f, 0.0, 0.0, 0.0, shift, 0.0, 1.0, 0.0, 0.0, near, 0.0]
}

/// Left-handed orthographic projection, depth 0 at `near` to 1 at `far`.
fn ortho(half_w: f32, half_h: f32, near: f32, far: f32) -> [f32; 16] {
    let d = far - near;
    [1.0 / half_w, 0.0, 0.0, 0.0, 0.0, 1.0 / half_h, 0.0, 0.0, 0.0, 0.0, 1.0 / d, 0.0, 0.0, 0.0, -near / d, 1.0]
}

fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    std::array::from_fn(|i| {
        let (col, row) = (i / 4, i % 4);
        (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum()
    })
}

/// `p` through `m`, divided by w.
fn transform(m: &[f32; 16], p: [f32; 3]) -> [f32; 3] {
    let v: [f32; 4] = std::array::from_fn(|row| m[row] * p[0] + m[4 + row] * p[1] + m[8 + row] * p[2] + m[12 + row]);
    let w = if v[3].abs() < 1e-6 { 1e-6 } else { v[3] };
    [v[0] / w, v[1] / w, v[2] / w]
}

const SHADER: &str = r#"
struct Frame {
    view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    camera: vec4<f32>,
    light: vec4<f32>,
    fill: vec4<f32>,
    ground: vec4<f32>,
    reveal: vec4<f32>,
    focus: vec4<f32>,
    viewport: vec4<f32>,
    sky_top: vec4<f32>,
    sky_bottom: vec4<f32>,
    accent: vec4<f32>,
    spot: vec4<f32>,
    // The key light's colour (linear); w 1 in the studio (its fill and rim, its sky's reflections).
    key_color: vec4<f32>,
    // Towards a second directional light, and its colour.
    light2: vec4<f32>,
    light2_color: vec4<f32>,
    // The ambient light facing up (w: how dark the key's shadow is on our ground), round the horizon (w: the
    // reflections' intensity), facing down.
    ambient_sky: vec4<f32>,
    ambient_equator: vec4<f32>,
    ambient_ground: vec4<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_cmp: sampler_comparison;

struct Material {
    color: vec4<f32>,
    st: vec4<f32>,
    // Cutoff, alpha mode (0 opaque, 1 cut out, 2 blended), has a base picture, shading (0 lit, 1 toon, 2 flat, 3 unlit).
    params: vec4<f32>,
    // Metallic, smoothness, normal map strength (0 without one), has a metallic-smoothness picture.
    look: vec4<f32>,
    // Emission (linear, HDR), has an emission picture.
    emission: vec4<f32>,
    // Toon: the shadowed band's tint, its border on the half-Lambert term.
    shade: vec4<f32>,
    // Toon: the border's softness; has a normal map.
    toon: vec4<f32>,
}

@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var base: texture_2d<f32>;
@group(1) @binding(2) var base_sampler: sampler;
@group(1) @binding(3) var normal_map: texture_2d<f32>;
@group(1) @binding(4) var surface_map: texture_2d<f32>;
@group(1) @binding(5) var emission_map: texture_2d<f32>;

struct VIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
}

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    let t = max(-n.z, 0.0);
    n.x = n.x + select(t, -t, n.x >= 0.0);
    n.y = n.y + select(t, -t, n.y >= 0.0);
    return normalize(n);
}

fn place(v: VIn) -> VOut {
    let m = mat4x4<f32>(v.m0, v.m1, v.m2, v.m3);
    let world = (m * vec4<f32>(v.position, 1.0)).xyz;
    // Normals through the cofactor matrix (scaled and mirrored objects alike).
    let c0 = v.m0.xyz;
    let c1 = v.m1.xyz;
    let c2 = v.m2.xyz;
    let det = dot(c0, cross(c1, c2));
    let cof = mat3x3<f32>(cross(c1, c2), cross(c2, c0), cross(c0, c1));
    var o: VOut;
    o.clip = frame.view_proj * vec4<f32>(world, 1.0);
    o.world = world;
    o.normal = cof * oct_decode(v.normal) * select(1.0, -1.0, det < 0.0);
    // Unity's v runs up the picture; its rows are stored from the top.
    let uv = v.uv * material.st.xy + material.st.zw;
    o.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return o;
}

@vertex
fn vs_scene(v: VIn) -> VOut {
    return place(v);
}

@vertex
fn vs_shadow(v: VIn) -> VOut {
    return place(v);
}

fn albedo_at(uv: vec2<f32>) -> vec4<f32> {
    let t = textureSample(base, base_sampler, uv);
    return material.color * select(vec4<f32>(1.0), t, material.params.z > 0.5);
}

// What a fragment shows, read before anything is discarded (samples and derivatives need every neighbour).
struct Surface {
    albedo: vec4<f32>,
    n: vec3<f32>,
    metallic: f32,
    smoothness: f32,
    emission: vec3<f32>,
}

// The normal map's normal in the frame the UVs make on screen (no tangents stored): Schüler's cotangent frame.
fn perturb(n: vec3<f32>, world: vec3<f32>, uv: vec2<f32>, packed: vec4<f32>) -> vec3<f32> {
    let dp1 = dpdx(world);
    let dp2 = dpdy(world);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let p2 = cross(dp2, n);
    let p1 = cross(n, dp1);
    // The gradients of u and of the picture's row (which runs against Unity's v), whatever way the triangle faces.
    let sgn = select(1.0, -1.0, dot(n, cross(dp1, dp2)) < 0.0);
    let t = (p2 * duv1.x + p1 * duv2.x) * sgn;
    let b = (p2 * duv1.y + p1 * duv2.y) * sgn;
    let inv = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-20));
    // Unity's UnpackScaleNormal.
    let xy = (packed.xy * 2.0 - 1.0) * material.look.z;
    let z = sqrt(max(1.0 - dot(xy, xy), 0.0));
    return normalize(t * inv * xy.x - b * inv * xy.y + n * max(z, 0.05));
}

fn surface_at(i: VOut, front: bool) -> Surface {
    var s: Surface;
    s.albedo = albedo_at(i.uv);
    let packed = textureSample(normal_map, base_sampler, i.uv);
    let ms = textureSample(surface_map, base_sampler, i.uv);
    let glow = textureSample(emission_map, base_sampler, i.uv);
    let n = normalize(i.normal) * select(-1.0, 1.0, front);
    let bumped = perturb(n, i.world, i.uv, packed);
    s.n = select(n, bumped, material.toon.y > 0.5);
    s.metallic = material.look.x * select(1.0, ms.r, material.look.w > 0.5);
    s.smoothness = material.look.y * select(1.0, ms.g, material.look.w > 0.5);
    s.emission = material.emission.rgb * select(vec3<f32>(1.0), glow.rgb, material.emission.w > 0.5);
    return s;
}

const TAPS = array<vec2<f32>, 12>(
    vec2<f32>(-0.326, -0.406), vec2<f32>(-0.840, -0.074), vec2<f32>(-0.696, 0.457), vec2<f32>(-0.203, 0.621),
    vec2<f32>(0.962, -0.195), vec2<f32>(0.473, -0.480), vec2<f32>(0.519, 0.767), vec2<f32>(0.185, -0.893),
    vec2<f32>(0.507, 0.064), vec2<f32>(0.896, 0.412), vec2<f32>(-0.322, -0.933), vec2<f32>(-0.792, -0.598),
);

fn shadow_at(world: vec3<f32>, n: vec3<f32>, softness: f32) -> f32 {
    let lp = frame.light_view_proj * vec4<f32>(world + n * frame.focus.w * 0.004, 1.0);
    let uv = vec2<f32>(lp.x * 0.5 + 0.5, 0.5 - lp.y * 0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || lp.z > 1.0) {
        return 1.0;
    }
    let texel = softness / 2048.0;
    var sum = 0.0;
    for (var i = 0; i < 12; i = i + 1) {
        sum = sum + textureSampleCompareLevel(shadow_map, shadow_cmp, uv + TAPS[i] * texel, lp.z - 0.0015);
    }
    return sum / 12.0;
}

// Unity's output below the highlights (its Scene view has no tone mapping), then Khronos PBR Neutral's shoulder so
// that what Unity would clip rolls off instead.
fn shoulder(c: vec3<f32>) -> vec3<f32> {
    let start = 0.8;
    let desaturation = 0.15;
    let peak = max(c.r, max(c.g, c.b));
    if (peak < start) {
        return c;
    }
    let d = 1.0 - start;
    let new_peak = 1.0 - d * d / (peak + d - start);
    let g = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(c * (new_peak / peak), vec3<f32>(new_peak), g);
}

const PI = 3.14159265;

fn studio() -> bool {
    return frame.key_color.w > 0.5;
}

// The ambient light (the scene's, or in the studio Unity's default procedural sky's): facing up, round the horizon,
// facing down, in between by the normal's height.
fn ambient(n: vec3<f32>) -> vec3<f32> {
    let y = clamp(n.y, -1.0, 1.0);
    return select(mix(frame.ambient_equator.rgb, frame.ambient_ground.rgb, -y), mix(frame.ambient_equator.rgb, frame.ambient_sky.rgb, y), y >= 0.0);
}

// The ambient light averaged over every direction, as toon shaders add it to the light's.
fn ambient_mean() -> vec3<f32> {
    return (frame.ambient_sky.rgb + frame.ambient_equator.rgb * 2.0 + frame.ambient_ground.rgb) * 0.25;
}

// What a reflection shows, blurred towards the ambient light as the surface roughens. The studio's is Unity's default
// sky: a bright hazy horizon, blue above, the dark ground below. The scene's follows its ambient light's three
// colours, sharper, at its reflections' intensity.
fn environment(r: vec3<f32>, perceptual_roughness: f32) -> vec3<f32> {
    let up = clamp(r.y, -1.0, 1.0);
    if (studio()) {
        let sky = mix(vec3<f32>(0.52, 0.55, 0.58), vec3<f32>(0.20, 0.30, 0.50), sqrt(max(up, 0.0)));
        let sharp = mix(vec3<f32>(0.14, 0.13, 0.12), sky, smoothstep(-0.08, 0.04, up));
        return mix(sharp, ambient(r), perceptual_roughness);
    }
    let above = mix(frame.ambient_equator.rgb, frame.ambient_sky.rgb, sqrt(max(up, 0.0)));
    let below = mix(frame.ambient_equator.rgb, frame.ambient_ground.rgb, sqrt(max(-up, 0.0)));
    let sharp = select(below, above, up >= 0.0);
    return mix(sharp, ambient(r), perceptual_roughness) * frame.ambient_equator.w;
}

// A fragment in the studio, as its material's shader takes the light.
fn shade(s: Surface, world: vec3<f32>) -> vec3<f32> {
    let albedo = s.albedo.rgb;
    let mode = material.params.w;
    if (mode > 2.5) {
        return albedo + s.emission;
    }
    let n = s.n;
    let l = frame.light.xyz;
    let v = normalize(frame.camera.xyz - world);
    let nl = dot(n, l);
    let shadow = mix(1.0, shadow_at(world, n, 2.5), frame.light.w);
    let sun = frame.key_color.rgb;
    let sun2 = frame.light2_color.rgb;
    // Every light and the sky together, capped at white as toon shaders cap it.
    let toon_light = min(sun + sun2 + ambient_mean(), vec3<f32>(1.0));
    if (mode > 1.5) {
        // Flat: lit alike whichever way it faces.
        return albedo * toon_light + s.emission;
    }
    if (mode > 0.5) {
        // Toon: two bands on the half-Lambert term; cast shadows left out, as Poiyomi and lilToon leave them by default.
        let half_lambert = nl * 0.5 + 0.5;
        let blur = max(material.toon.x, 0.002) * 0.5;
        let lit = smoothstep(material.shade.w - blur, material.shade.w + blur, half_lambert);
        var c = albedo * mix(material.shade.rgb, vec3<f32>(1.0), lit) * toon_light;
        // A highlight only when the toon shader's reflections are on (smoothness above 0).
        if (s.smoothness > 0.001) {
            c = c + standard_specular(s, n, l, v, nl, shadow);
        }
        return c + s.emission;
    }
    // Lit: Unity's Standard (BRDF1: Lambert, GGX with Smith-joint visibility, its π scaling and grazing reflections).
    let one_minus_reflectivity = 0.96 * (1.0 - s.metallic);
    let diffuse = albedo * one_minus_reflectivity;
    let direct = sun * clamp(nl, 0.0, 1.0) * shadow + sun2 * clamp(dot(n, frame.light2.xyz), 0.0, 1.0);
    // In the studio, a faint cool fill and rim keep the side away from the light from sinking into the dark.
    let in_studio = select(0.0, 1.0, studio());
    let fill = max(dot(n, frame.fill.xyz), 0.0) * vec3<f32>(0.06, 0.07, 0.09) * in_studio;
    let rim = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 4.0) * clamp(n.y + 0.6, 0.0, 1.0) * vec3<f32>(0.05, 0.06, 0.08) * in_studio;
    return diffuse * (ambient(n) + direct + fill) + standard_specular(s, n, l, v, nl, shadow) + rim * diffuse * 2.0 + s.emission;
}

// Standard's highlight from the light and reflection of the sky.
fn standard_specular(s: Surface, n: vec3<f32>, l: vec3<f32>, v: vec3<f32>, nl_raw: f32, shadow: f32) -> vec3<f32> {
    let spec_color = mix(vec3<f32>(0.04), s.albedo.rgb, s.metallic);
    let one_minus_reflectivity = 0.96 * (1.0 - s.metallic);
    let perceptual = 1.0 - s.smoothness;
    let a = max(perceptual * perceptual, 0.002);
    let h = normalize(l + v);
    let nl = clamp(nl_raw, 0.0, 1.0);
    let nv = abs(dot(n, v));
    let nh = clamp(dot(n, h), 0.0, 1.0);
    let lh = clamp(dot(l, h), 0.0, 1.0);
    let vis = 0.5 / (nl * (nv * (1.0 - a) + a) + nv * (nl * (1.0 - a) + a) + 1e-5);
    let a2 = a * a;
    let d = (nh * a2 - nh) * nh + 1.0;
    let ggx = a2 / (PI * (d * d) + 1e-7);
    let term = max(vis * ggx * PI * nl, 0.0);
    let fresnel = spec_color + (1.0 - spec_color) * pow(1.0 - lh, 5.0);
    let reduction = 1.0 / (a * a + 1.0);
    let grazing = clamp(s.smoothness + (1.0 - one_minus_reflectivity), 0.0, 1.0);
    let fresnel_lerp = mix(spec_color, vec3<f32>(grazing), pow(1.0 - nv, 5.0));
    let r = reflect(-v, n);
    return term * frame.key_color.rgb * shadow * fresnel + reduction * environment(r, perceptual) * fresnel_lerp;
}

// The reveal's glowing edge, just under the rising plane.
fn edge(world: vec3<f32>) -> f32 {
    let e = 1.0 - clamp((frame.reveal.x - world.y) / frame.reveal.y, 0.0, 1.0);
    return e * e * frame.reveal.w;
}

@fragment
fn fs_scene(i: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let s = surface_at(i, front);
    let a = s.albedo;
    if (i.world.y > frame.reveal.x) {
        discard;
    }
    if (material.params.y > 0.5 && material.params.y < 1.5 && a.a < material.params.x) {
        discard;
    }
    let c = shoulder(shade(s, i.world)) + frame.accent.rgb * edge(i.world) * 3.0;
    let alpha = select(1.0, a.a, material.params.y > 1.5);
    return vec4<f32>(c, alpha);
}

@fragment
fn fs_mirror(i: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let s = surface_at(i, front);
    let a = s.albedo;
    let above = i.world.y - frame.ground.x;
    if (i.world.y > frame.reveal.x || above < -0.002) {
        discard;
    }
    if (material.params.y > 0.5 && a.a < select(material.params.x, 0.5, material.params.y > 1.5)) {
        discard;
    }
    let c = shoulder(shade(s, i.world)) + frame.accent.rgb * edge(i.world) * 2.2;
    // Fainter the higher above the ground, and towards the ground's edge.
    let fade = exp(-above / max(frame.focus.w * 0.35, 0.05));
    let d = length(i.world.xz - frame.focus.xz) / frame.ground.y;
    let edge_fade = 1.0 - smoothstep(0.15, 0.6, d);
    return vec4<f32>(c, frame.ground.w * fade * edge_fade);
}

@fragment
fn fs_shadow(i: VOut) {
    let a = albedo_at(i.uv);
    if (i.world.y > frame.reveal.x) {
        discard;
    }
    if (material.params.y > 0.5 && a.a < select(material.params.x, 0.5, material.params.y > 1.5)) {
        discard;
    }
}

struct Flat {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
}

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> Flat {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Flat;
    o.clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    o.world = vec3<f32>(0.0);
    return o;
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

@fragment
fn fs_background(f: Flat) -> @location(0) vec4<f32> {
    var c = mix(frame.sky_top.rgb, frame.sky_bottom.rgb, f.uv.y);
    // A soft spot of light behind the subject, and a vignette.
    let aspect = frame.viewport.z;
    let d = (f.uv - frame.spot.xy) * vec2<f32>(aspect, 1.0);
    c = c + vec3<f32>(0.020, 0.021, 0.026) * exp(-dot(d, d) * 2.2);
    let e = (f.uv - vec2<f32>(0.5)) * vec2<f32>(1.0, 1.0);
    c = c * (1.0 - 0.35 * dot(e, e));
    // Against banding in 8 bits.
    c = c + (hash(f.clip.xy) - 0.5) * vec3<f32>(0.0009);
    return vec4<f32>(max(c, vec3<f32>(0.0)), 1.0);
}

@vertex
fn vs_ground(@builtin(vertex_index) i: u32) -> Flat {
    let corner = vec2<f32>(f32(i & 1u), f32((i >> 1u) & 1u)) * 2.0 - 1.0;
    let r = frame.ground.y;
    let world = vec3<f32>(frame.focus.x + corner.x * r, frame.ground.x, frame.focus.z + corner.y * r);
    var o: Flat;
    o.clip = frame.view_proj * vec4<f32>(world, 1.0);
    o.uv = corner;
    o.world = world;
    return o;
}

@fragment
fn fs_ground(f: Flat) -> @location(0) vec4<f32> {
    let d = length(f.world.xz - frame.focus.xz) / frame.ground.y;
    let fade = 1.0 - smoothstep(0.08, 0.62, d);
    let s = mix(1.0, shadow_at(f.world, vec3<f32>(0.0, 1.0, 0.0), 6.0), frame.ambient_sky.w);
    // Contact shadow under what stands there.
    let r = max(frame.focus.w * 0.42, 0.05);
    let near = length(f.world.xz - frame.focus.xz) / r;
    let contact = 1.0 - 0.45 * exp(-near * near * 1.6);
    let pool = 1.0 + 1.6 * exp(-d * d * 22.0);
    var c = vec3<f32>(0.024, 0.024, 0.028) * pool * mix(0.3, 1.0, s) * contact;
    // A ring running out from the subject as it rises.
    let ring_r = frame.reveal.z * 0.55;
    let ring = exp(-pow((d - ring_r) * 60.0, 2.0)) * frame.reveal.w;
    c = c + frame.accent.rgb * ring * 0.6;
    return vec4<f32>(c, clamp(fade * 0.94 + ring, 0.0, 1.0));
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_looks_along_forward_and_depth_reverses() {
        let view = look_to([0.0, 1.0, -3.0], [0.0, 0.0, 1.0]);
        let proj = perspective(1.0, 1.0, 0.1, 0.0);
        let vp = mul(&proj, &view);
        let ahead = transform(&vp, [0.0, 1.0, 0.0]);
        assert!(ahead[0].abs() < 1e-5 && ahead[1].abs() < 1e-5, "{ahead:?}");
        let near = transform(&vp, [0.0, 1.0, -2.8]);
        let far = transform(&vp, [0.0, 1.0, 50.0]);
        assert!(near[2] > far[2] && far[2] > 0.0 && near[2] <= 1.0, "{near:?} {far:?}");
        // +X is to the right on screen, +Y up.
        assert!(transform(&vp, [1.0, 1.0, 0.0])[0] > 0.0);
        assert!(transform(&vp, [0.0, 2.0, 0.0])[1] > 0.0);
    }

    #[test]
    fn materials_without_their_look_draw_lit_and_nearly_matte() {
        // A schema 1 material: Standard-lit, smoothness 0.2, no metal, no normal map, no glow.
        let u = material_uniform(&preview::Material::default(), true, false, false, false);
        assert_eq!((u[10], u[11]), (1.0, 0.0), "base picture, lit");
        assert_eq!((u[12], u[13], u[14], u[15]), (0.0, 0.2, 0.0, 0.0));
        assert_eq!(&u[16..20], &[0.0; 4]);
        // A toon material with a normal map whose picture could not be read draws without it; nonsense is clamped.
        let m = preview::Material { shading: Shading::Toon, normal_texture: 0, smoothness: f32::NAN, emission: [-1.0, 100.0, 1.0], ..Default::default() };
        let u = material_uniform(&m, false, false, false, false);
        assert_eq!((u[11], u[13], u[14], u[25]), (1.0, 0.2, 0.0, 0.0));
        assert_eq!(&u[16..19], &[0.0, 64.0, 1.0]);
        let u = material_uniform(&m, false, true, false, false);
        assert_eq!((u[14], u[25]), (1.0, 1.0), "normal map used");
    }

    fn lit_header(lighting: Option<preview::Lighting>) -> preview::Header {
        let b = preview::Bounds { min: [-0.5, 0.0, -0.3], max: [0.5, 1.8, 0.3] };
        preview::Header { kind: Kind::Avatars, bounds: b, focus: b, ground: Some(0.0), lighting, ..Default::default() }
    }

    #[test]
    fn the_scenes_light_is_used_when_preferred_and_recorded() {
        let sun = preview::SceneLight { direction: [0.0, -1.0, 0.0], color: [2.0, 1.0, 0.5], intensity: 2.0, shadows: false, shadow_strength: 1.0 };
        let lighting = preview::Lighting { lights: vec![sun], ambient: None, reflection_intensity: 0.5 };
        let with = lit_header(Some(lighting));
        let without = lit_header(None);
        assert_eq!(light_mode(true, &with), LightMode::Scene);
        assert_eq!(light_mode(false, &with), LightMode::Studio, "the switch on the studio");
        assert_eq!(light_mode(true, &without), LightMode::Studio, "a file without the scene's light");

        let scene = light_for(&with, LightMode::Scene);
        assert_eq!(scene.direction, [0.0, 1.0, 0.0], "towards the light, which shines straight down");
        assert_eq!(scene.color, [2.0, 1.0, 0.5]);
        assert_eq!((scene.shadow, scene.ground_shadow, scene.studio, scene.reflection), (0.0, 0.0, false, 0.5));
        assert_eq!(scene.fill, [0.0; 3], "no studio fill");
        assert_eq!(scene.ambient, STUDIO_AMBIENT, "the studio's ambient light when the file has none");
        assert_eq!(scene.second_color, [0.0; 3]);

        let studio = light_for(&with, LightMode::Studio);
        assert!(studio.studio && studio.color == STUDIO_SUN && studio.shadow == 0.65);
        assert_eq!(studio, light_for(&without, LightMode::Scene), "without the scene's light, the studio's");
    }

    #[test]
    fn a_scene_without_directional_lights_is_lit_by_its_ambient_light_alone() {
        let ambient = preview::Ambient { mode: preview::AmbientMode::Flat, sky: [0.5; 3], equator: [0.5; 3], ground: [0.5; 3] };
        let broken = preview::SceneLight { direction: [0.0; 3], color: [f32::NAN, 1.0, 1e9], intensity: 1.0, shadows: true, shadow_strength: 1.0 };
        let h = lit_header(Some(preview::Lighting { lights: vec![broken], ambient: Some(ambient), reflection_intensity: 1.0 }));
        let l = light_for(&h, LightMode::Scene);
        assert_eq!((l.color, l.shadow, l.ambient), ([0.0; 3], 0.0, [[0.5; 3]; 3]), "a light that cannot be pointed is none");
        // A second light beside the key; colours kept finite.
        let key = preview::SceneLight { direction: [0.0, 0.0, 1.0], color: [f32::NAN, 1.0, 1e9], intensity: 1.0, shadows: true, shadow_strength: 0.7 };
        let back = preview::SceneLight { direction: [0.0, 0.0, -2.0], color: [0.1, 0.2, 0.3], intensity: 1.0, shadows: false, shadow_strength: 1.0 };
        let h = lit_header(Some(preview::Lighting { lights: vec![key, back], ambient: None, reflection_intensity: f32::NAN }));
        let l = light_for(&h, LightMode::Scene);
        assert_eq!((l.direction, l.color, l.shadow), ([0.0, 0.0, -1.0], [0.0, 1.0, 16.0], 0.7));
        assert_eq!((l.second, l.second_color, l.reflection), ([0.0, 0.0, 1.0], [0.1, 0.2, 0.3], 1.0));
    }

    #[test]
    fn mip_levels_keep_cut_out_colours() {
        // A red pixel beside three transparent black ones stays red, a quarter opaque.
        let src = [255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let (out, w, h) = half_size(&src, 2, 2);
        assert_eq!((w, h), (1, 1));
        assert!(out[0] > 250 && out[1] < 3 && out[3] == 63, "{out:?}");
    }
}
