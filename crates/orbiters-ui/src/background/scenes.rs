//! The scenes themselves: what each draws, pass by pass, with the parameters the website gives it
//! (`CreatorBackground.jsx`). React Bits' scenes (Aero Shards, Grainient, Gradient Waves, Pattern Waves, Gradient ASCII)
//! are Copyright (c) 2026 David Haz, MIT + Commons Clause (`assets/licenses/react-bits.txt`): part of this application,
//! never a library of their own. Window Light is Orbiters' own.

use super::{made, Backdrop, Scene, Time};
use crate::look;
use egui::Color32;
use egui_wgpu::wgpu;
use std::sync::{Arc, OnceLock};
use wgpu::util::DeviceExt;

const FULL: &str = include_str!("full.wgsl");
const OUT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const HALO: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

const AERO_SHARDS: &str = include_str!("aero_shards.wgsl");
const AERO_BLOOM: &str = include_str!("aero_bloom.wgsl");
const AERO_BLUR: &str = include_str!("aero_blur.wgsl");
const AERO_FINISH: &str = include_str!("aero_finish.wgsl");

fn full(parts: &[&str]) -> String {
    std::iter::once(FULL).chain(parts.iter().copied()).collect()
}

/// A shader by its pipeline's name, whole.
fn source(name: &str) -> String {
    let common = include_str!("ascii_common.wgsl");
    let glyphs = include_str!("ascii_glyphs.wgsl");
    match name {
        "grainient" => full(&[include_str!("grainient.wgsl")]),
        "waves" => full(&[include_str!("waves.wgsl")]),
        "leaf" => full(&[include_str!("leaf.wgsl")]),
        "pattern-field" => full(&[include_str!("pattern_field.wgsl")]),
        "pattern-mark" => full(&[include_str!("pattern_mark.wgsl")]),
        "ascii-field" => full(&[common, include_str!("ascii_field.wgsl")]),
        "ascii-halo" => full(&[common, glyphs, include_str!("ascii_halo.wgsl")]),
        "ascii-composite" => full(&[common, glyphs, include_str!("ascii_composite.wgsl")]),
        "ascii-blur" | "aero-blur" => AERO_BLUR.into(),
        "aero-shards" => AERO_SHARDS.into(),
        "aero-bloom" => AERO_BLOOM.into(),
        "aero-finish" => AERO_FINISH.into(),
        _ => full(&[include_str!("composite.wgsl")]),
    }
}

/// Every shader, as it is compiled (tests).
#[cfg(test)]
pub(super) fn sources() -> Vec<(&'static str, String)> {
    ["composite", "grainient", "waves", "leaf", "pattern-field", "pattern-mark", "ascii-field", "ascii-halo", "ascii-composite", "aero-shards", "aero-bloom", "aero-blur", "aero-finish"]
        .into_iter()
        .map(|n| (n, source(n)))
        .collect()
}

pub(super) fn pipeline(device: &wgpu::Device, source: &str, vertex: &str, targets: &[Option<wgpu::ColorTargetState>], samples: u32) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("orbiters background"), // i18n: not shown (names a graphics debugger shows)
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("orbiters background"), // i18n: not shown (names a graphics debugger shows)
        layout: None,
        vertex: wgpu::VertexState { module: &module, entry_point: Some(vertex), buffers: &[], compilation_options: Default::default() },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState { count: samples.max(1), mask: !0, alpha_to_coverage_enabled: false },
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_main"), targets, compilation_options: Default::default() }),
        multiview_mask: None,
        cache: None,
    })
}

fn plain(format: wgpu::TextureFormat) -> Option<wgpu::ColorTargetState> {
    Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })
}

/// What the window's own pass draws with: the composite's target.
#[derive(Clone, Copy, Debug)]
pub(super) struct Onto {
    pub(super) format: wgpu::TextureFormat,
    pub(super) samples: u32,
}

/// A scene's pipelines, and the one that lays it onto the window.
pub(super) struct Made {
    made: std::collections::HashMap<&'static str, wgpu::RenderPipeline>,
}

/// The pipelines of each scene, made the first time it shows: on a thread of their own, where they may take tens of
/// milliseconds (so may the pictures some scenes read), and the window goes on drawing without them meanwhile.
pub(super) struct Pipelines {
    scenes: std::collections::HashMap<Scene, Arc<OnceLock<Made>>>,
    onto: Onto,
    /// Off screen, where a frame is asked for a picture: made at once.
    pub(super) at_once: bool,
}

impl Pipelines {
    pub(super) fn new(onto: Onto, at_once: bool) -> Self {
        Self { scenes: Default::default(), onto, at_once }
    }

    /// The pipelines of `scene` once they are there; `woken` when they come.
    pub(super) fn of(&mut self, device: &wgpu::Device, scene: Scene, woken: impl FnOnce() + Send + 'static) -> Option<Arc<OnceLock<Made>>> {
        let onto = self.onto;
        let at_once = self.at_once;
        let made = self.scenes.entry(scene).or_insert_with(|| {
            let slot = Arc::new(OnceLock::new());
            if at_once {
                slot.get_or_init(|| Made::make(device, scene, onto));
            } else {
                let (device, to) = (device.clone(), slot.clone());
                let work = move || {
                    let _ = to.set(Made::make(&device, scene, onto));
                    woken();
                };
                // No thread to be had: the scene does not show, the page does.
                let _ = std::thread::Builder::new().name("background pipelines".into()).spawn(work);
            }
            slot
        });
        made.get().is_some().then(|| made.clone())
    }
}

impl Made {
    fn make(device: &wgpu::Device, scene: Scene, onto: Onto) -> Self {
        // What the scene reads is drawn here too, not on the window's thread.
        match scene {
            Scene::WindowLight => drop(leaves()),
            Scene::GradientAscii => drop(levels()),
            _ => {}
        }
        let mut made = std::collections::HashMap::new();
        let blend = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
        let laid = [Some(wgpu::ColorTargetState { format: onto.format, blend, write_mask: wgpu::ColorWrites::ALL })];
        made.insert("composite", pipeline(device, &source("composite"), "vs_full", &laid, onto.samples));
        let mut add = |name: &'static str, vertex: &str, targets: &[Option<wgpu::ColorTargetState>]| {
            made.insert(name, pipeline(device, &source(name), vertex, targets, 1));
        };
        match scene {
            Scene::None | Scene::Grainient => add("grainient", "vs_full", &[plain(OUT)]),
            Scene::GradientWaves => add("waves", "vs_full", &[plain(OUT)]),
            Scene::WindowLight => add("leaf", "vs_full", &[plain(OUT)]),
            Scene::PatternWaves => {
                add("pattern-field", "vs_full", &[plain(OUT)]);
                add("pattern-mark", "vs_full", &[plain(OUT)]);
            }
            Scene::GradientAscii => {
                add("ascii-field", "vs_full", &[plain(OUT), plain(OUT)]);
                add("ascii-halo", "vs_full", &[plain(OUT)]);
                add("ascii-blur", "vs_full", &[plain(OUT)]);
                add("ascii-composite", "vs_full", &[plain(OUT)]);
            }
            Scene::AeroShards => {
                let blend = Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING);
                add("aero-shards", "vs_main", &[Some(wgpu::ColorTargetState { format: OUT, blend, write_mask: wgpu::ColorWrites::ALL })]);
                add("aero-bloom", "vs_full", &[plain(HALO)]);
                add("aero-blur", "vs_full", &[plain(HALO)]);
                add("aero-finish", "vs_full", &[plain(OUT)]);
            }
        }
        Self { made }
    }

    pub(super) fn get(&self, name: &str) -> &wgpu::RenderPipeline {
        &self.made[name]
    }
}

fn target(device: &wgpu::Device, size: [u32; 2], format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("orbiters background"), // i18n: not shown (names a graphics debugger shows)
        size: wgpu::Extent3d { width: size[0].max(1), height: size[1].max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

fn view(t: &wgpu::Texture) -> wgpu::TextureView {
    t.create_view(&Default::default())
}

fn uniforms(device: &wgpu::Device, floats: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("orbiters background"), // i18n: not shown (names a graphics debugger shows)
        size: (floats * 4) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

enum Bound<'a> {
    Uniforms(&'a wgpu::Buffer),
    Texture(&'a wgpu::Texture),
    Sampler(&'a wgpu::Sampler),
}

fn bind(device: &wgpu::Device, pipeline: &wgpu::RenderPipeline, what: &[(u32, Bound)]) -> wgpu::BindGroup {
    let views: Vec<Option<wgpu::TextureView>> = what.iter().map(|(_, b)| if let Bound::Texture(t) = b { Some(view(t)) } else { None }).collect();
    let entries: Vec<wgpu::BindGroupEntry> = what
        .iter()
        .zip(&views)
        .filter_map(|((binding, b), v)| {
            let resource = match b {
                Bound::Uniforms(u) => u.as_entire_binding(),
                Bound::Texture(_) => wgpu::BindingResource::TextureView(v.as_ref()?),
                Bound::Sampler(s) => wgpu::BindingResource::Sampler(s),
            };
            Some(wgpu::BindGroupEntry { binding: *binding, resource })
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("orbiters background"), // i18n: not shown (names a graphics debugger shows)
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    })
}

fn pass(encoder: &mut wgpu::CommandEncoder, targets: &[&wgpu::TextureView], clear: wgpu::Color, pipeline: &wgpu::RenderPipeline, group: &wgpu::BindGroup, count: (u32, u32)) {
    let ops = wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store };
    let attachments: Vec<Option<wgpu::RenderPassColorAttachment>> =
        targets.iter().map(|v| Some(wgpu::RenderPassColorAttachment { view: v, depth_slice: None, resolve_target: None, ops })).collect();
    let mut p = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("orbiters background"), // i18n: not shown (names a graphics debugger shows)
        color_attachments: &attachments,
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    p.set_pipeline(pipeline);
    p.set_bind_group(0, group, &[]);
    p.draw(0..count.0, 0..count.1);
}

/// A full-screen triangle.
const ONCE: (u32, u32) = (3, 1);

fn rgb(c: Color32) -> [f32; 3] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0]
}

fn four([r, g, b]: [f32; 3]) -> [f32; 4] {
    [r, g, b, 1.0]
}

fn rgba(c: Color32) -> [f32; 4] {
    four(rgb(c))
}

fn mixed(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// The website's hex mixing, rounded to bytes as it rounds.
fn mixed_bytes(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// A colour in OKLab, as `gradientAsciiRenderer.js` has it.
fn lab(c: Color32) -> [f32; 4] {
    let k = look::Oklch::of(c);
    let h = k.h.to_radians();
    [k.l, k.c * h.cos(), k.c * h.sin(), 0.0]
}

/// A scene at a size, with its targets.
pub(super) struct Scenic {
    scene: Scene,
    css: [f32; 2],
    ppp: f32,
    size: [u32; 2],
    output: wgpu::Texture,
    out: wgpu::TextureView,
    made: Arc<OnceLock<Made>>,
    kind: Kind,
}

enum Kind {
    /// One pass: Grainient, Gradient Waves, Window Light.
    One(One),
    Pattern(Pattern),
    Ascii(Box<Ascii>),
    Aero(Box<Aero>),
}

struct One {
    name: &'static str,
    params: wgpu::Buffer,
    group: wgpu::BindGroup,
}

struct Pattern {
    field: wgpu::TextureView,
    cells: [u32; 2],
    origin: [f32; 2],
    pitch: f32,
    dpr: f32,
    field_params: wgpu::Buffer,
    mark_params: wgpu::Buffer,
    field_group: wgpu::BindGroup,
    mark_group: wgpu::BindGroup,
}

struct Ascii {
    params: wgpu::Buffer,
    cell: [f32; 2],
    halo_size: [u32; 2],
    field: wgpu::TextureView,
    aux: wgpu::TextureView,
    halo_a: wgpu::TextureView,
    halo_b: wgpu::TextureView,
    field_group: wgpu::BindGroup,
    halo_group: wgpu::BindGroup,
    blur_x_group: wgpu::BindGroup,
    blur_y_group: wgpu::BindGroup,
    composite_group: wgpu::BindGroup,
}

struct Aero {
    view: wgpu::Buffer,
    post: wgpu::Buffer,
    scene: wgpu::TextureView,
    bloom: wgpu::TextureView,
    scratch: wgpu::TextureView,
    bloom_size: [u32; 2],
    /// How far along its path the stream is, and when that was.
    travel: f64,
    at: f64,
    shards_group: wgpu::BindGroup,
    bloom_group: wgpu::BindGroup,
    blur_x_group: wgpu::BindGroup,
    blur_y_group: wgpu::BindGroup,
    finish_group: wgpu::BindGroup,
}

/// Aero Shards: the website's medium quality (a page is wider than its small canvases' high one), balanced detail, at a
/// density of 1.5.
const SHARDS: u32 = 4800;

/// Pixels per point of a scene. The website's rules are the most; under them a budget of pixels: a game may share the GPU. The
/// scenes that are soft all over are drawn small and scaled up, as the website already does Window Light; those with an
/// edge to lose (dots, characters, shards) keep the screen's pixels up to a full HD's worth.
fn density(scene: Scene, css: [f32; 2], ppp: f32) -> f32 {
    let area = (css[0] * css[1]).max(1.0);
    let budget = |most: f32, pixels: f32| ppp.min(most).min((pixels / area).sqrt());
    match scene {
        Scene::PatternWaves => budget(2.0, 2.1e6),
        Scene::GradientAscii => budget(1.5, 2.1e6).max(0.5),
        Scene::AeroShards => budget(2.0, 2.1e6),
        Scene::WindowLight => (0.6 * ppp.min(2.0)).min((0.4e6 / area).sqrt()),
        Scene::None | Scene::Grainient | Scene::GradientWaves => budget(1.0, 0.4e6),
    }
}

impl One {
    fn new(device: &wgpu::Device, pipelines: &Made, name: &'static str, floats: usize) -> Self {
        let params = uniforms(device, floats);
        let group = bind(device, pipelines.get(name), &[(0, Bound::Uniforms(&params))]);
        One { name, params, group }
    }

    fn leaf(device: &wgpu::Device, queue: &wgpu::Queue, pipelines: &Made, sampler: &wgpu::Sampler) -> Self {
        let params = uniforms(device, 20);
        let side = made::LEAVES as u32;
        let descriptor = wgpu::TextureDescriptor {
            label: Some("orbiters leaves"), // i18n: not shown (names a graphics debugger shows)
            size: wgpu::Extent3d { width: side, height: side, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OUT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        let leaves = device.create_texture_with_data(queue, &descriptor, Default::default(), leaves());
        let group = bind(device, pipelines.get("leaf"), &[(0, Bound::Uniforms(&params)), (1, Bound::Texture(&leaves)), (2, Bound::Sampler(sampler))]);
        One { name: "leaf", params, group }
    }
}

impl Pattern {
    fn new(device: &wgpu::Device, pipelines: &Made, css: [f32; 2], size: [u32; 2]) -> Self {
        let dpr = size[0] as f32 / css[0].max(1.0);
        let pitch = (9.0 * dpr).round().max(4.0);
        let cells = [((size[0] as f32 / pitch).ceil() as u32 + 1).min(4096), ((size[1] as f32 / pitch).ceil() as u32 + 2).min(4096)];
        let origin = [((size[0] as f32 - cells[0] as f32 * pitch) / 2.0).floor(), ((size[1] as f32 - cells[1] as f32 * pitch) / 2.0).floor()];
        let field = target(device, cells, OUT);
        let (field_params, mark_params) = (uniforms(device, 20), uniforms(device, 20));
        let field_group = bind(device, pipelines.get("pattern-field"), &[(0, Bound::Uniforms(&field_params))]);
        let mark_group = bind(device, pipelines.get("pattern-mark"), &[(0, Bound::Uniforms(&mark_params)), (1, Bound::Texture(&field))]);
        Pattern { field: view(&field), cells, origin, pitch, dpr, field_params, mark_params, field_group, mark_group }
    }
}

impl Ascii {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, pipelines: &Made, sampler: &wgpu::Sampler, css: [f32; 2], size: [u32; 2]) -> Self {
        let part = |n: f32| size.map(|v| ((v as f32 / n).round() as u32).max(1));
        let (half, halo_size) = (part(2.0), part(4.0));
        let (field, aux) = (target(device, half, OUT), target(device, half, OUT));
        let (halo_a, halo_b) = (target(device, halo_size, OUT), target(device, halo_size, OUT));
        let atlas = atlas(device, queue);
        let params = uniforms(device, 44);
        let blur = pipelines.get("ascii-blur");
        let (blur_x, blur_y) = blur_steps(device, queue, halo_size);
        let glyphs = |extra: Option<&wgpu::Texture>, name: &str| {
            let mut what = vec![(0, Bound::Uniforms(&params)), (1, Bound::Texture(&field)), (2, Bound::Texture(&aux)), (3, Bound::Texture(&atlas)), (5, Bound::Sampler(sampler))];
            if let Some(t) = extra {
                what.push((4, Bound::Texture(t)));
            }
            bind(device, pipelines.get(name), &what)
        };
        let cell_h = 16.0 * size[0] as f32 / css[0].max(1.0);
        Ascii {
            cell: [cell_h * 0.6, cell_h],
            halo_size,
            field_group: bind(device, pipelines.get("ascii-field"), &[(0, Bound::Uniforms(&params))]),
            halo_group: glyphs(None, "ascii-halo"),
            blur_x_group: bind(device, blur, &[(0, Bound::Texture(&halo_a)), (1, Bound::Sampler(sampler)), (2, Bound::Uniforms(&blur_x))]),
            blur_y_group: bind(device, blur, &[(0, Bound::Texture(&halo_b)), (1, Bound::Sampler(sampler)), (2, Bound::Uniforms(&blur_y))]),
            composite_group: glyphs(Some(&halo_a), "ascii-composite"),
            field: view(&field),
            aux: view(&aux),
            halo_a: view(&halo_a),
            halo_b: view(&halo_b),
            params,
        }
    }
}

impl Aero {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, pipelines: &Made, sampler: &wgpu::Sampler, css: [f32; 2], size: [u32; 2]) -> Self {
        let bloom_size = css.map(|v| ((v * 0.25).round() as u32).max(1));
        let scene = target(device, size, OUT);
        let (bloom, scratch) = (target(device, bloom_size, HALO), target(device, bloom_size, HALO));
        let (params, post) = (uniforms(device, 72), uniforms(device, 24));
        let (blur_x, blur_y) = blur_steps(device, queue, bloom_size);
        let blur = pipelines.get("aero-blur");
        Aero {
            travel: 0.0,
            at: 0.0,
            bloom_size,
            shards_group: bind(device, pipelines.get("aero-shards"), &[(0, Bound::Uniforms(&params))]),
            bloom_group: bind(device, pipelines.get("aero-bloom"), &[(0, Bound::Texture(&scene)), (1, Bound::Sampler(sampler)), (2, Bound::Uniforms(&post))]),
            blur_x_group: bind(device, blur, &[(0, Bound::Texture(&bloom)), (1, Bound::Sampler(sampler)), (2, Bound::Uniforms(&blur_x))]),
            blur_y_group: bind(device, blur, &[(0, Bound::Texture(&scratch)), (1, Bound::Sampler(sampler)), (2, Bound::Uniforms(&blur_y))]),
            finish_group: bind(
                device,
                pipelines.get("aero-finish"),
                &[(0, Bound::Texture(&scene)), (1, Bound::Texture(&bloom)), (2, Bound::Sampler(sampler)), (3, Bound::Uniforms(&post))],
            ),
            scene: view(&scene),
            bloom: view(&bloom),
            scratch: view(&scratch),
            view: params,
            post,
        }
    }
}

fn blur_steps(device: &wgpu::Device, queue: &wgpu::Queue, size: [u32; 2]) -> (wgpu::Buffer, wgpu::Buffer) {
    let (x, y) = (uniforms(device, 4), uniforms(device, 4));
    queue.write_buffer(&x, 0, bytemuck::cast_slice(&[1.0 / size[0] as f32, 0.0, 0.0, 0.0]));
    queue.write_buffer(&y, 0, bytemuck::cast_slice(&[0.0, 1.0 / size[1] as f32, 0.0, 0.0]));
    (x, y)
}

impl Scenic {
    pub(super) fn fits(&self, scene: Scene, css: [f32; 2], ppp: f32) -> bool {
        self.scene == scene && self.css == css && self.ppp == ppp
    }

    pub(super) fn output(&self) -> &wgpu::Texture {
        &self.output
    }

    /// `None` when the pipelines are not there after all.
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue, made: Arc<OnceLock<Made>>, sampler: &wgpu::Sampler, scene: Scene, css: [f32; 2], ppp: f32) -> Option<Self> {
        let pipelines = made.get()?;
        let dpr = density(scene, css, ppp);
        let limit = device.limits().max_texture_dimension_2d.max(1);
        let size = css.map(|v| ((v * dpr).round() as u32).clamp(1, limit));
        let kind = match scene {
            Scene::Grainient | Scene::None => Kind::One(One::new(device, pipelines, "grainient", 32)),
            Scene::GradientWaves => Kind::One(One::new(device, pipelines, "waves", 28)),
            Scene::WindowLight => Kind::One(One::leaf(device, queue, pipelines, sampler)),
            Scene::PatternWaves => Kind::Pattern(Pattern::new(device, pipelines, css, size)),
            Scene::GradientAscii => Kind::Ascii(Box::new(Ascii::new(device, queue, pipelines, sampler, css, size))),
            Scene::AeroShards => Kind::Aero(Box::new(Aero::new(device, queue, pipelines, sampler, css, size))),
        };
        let output = target(device, size, OUT);
        Some(Self { scene, css, ppp, size, out: view(&output), output, kind, made })
    }

    /// The pipeline that lays it onto the window.
    pub(super) fn composite(&self) -> Option<&wgpu::RenderPipeline> {
        self.made.get().map(|m| m.get("composite"))
    }

    /// Encodes the picture of `backdrop` at `time`.
    pub(super) fn draw(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, backdrop: &Backdrop, time: Time) {
        let Some(pipelines) = self.made.get() else { return };
        let out = &self.out;
        let size = self.size.map(|v| v as f32);
        // Scenes the website has no still frame of stand where they show themselves.
        let moved = if time.still { 8.0 } else { time.moved };
        match &mut self.kind {
            Kind::One(s) => {
                let floats = one(self.scene, size, backdrop, time, moved);
                queue.write_buffer(&s.params, 0, bytemuck::cast_slice(&floats));
                pass(encoder, &[out], wgpu::Color::TRANSPARENT, pipelines.get(s.name), &s.group, ONCE);
            }
            Kind::Pattern(s) => s.draw(queue, encoder, pipelines, out, self.css, size, backdrop, time),
            Kind::Ascii(s) => s.draw(queue, encoder, pipelines, out, size, backdrop, time),
            Kind::Aero(s) => s.draw(queue, encoder, pipelines, out, size, backdrop, moved),
        }
    }
}

/// The parameters of the scenes of one pass.
fn one(scene: Scene, [w, h]: [f32; 2], backdrop: &Backdrop, time: Time, moved: f64) -> Vec<[f32; 4]> {
    let [a, b, c] = backdrop.colors.map(rgba);
    match scene {
        Scene::GradientWaves => vec![[w, h, moved as f32, 40.0], [0.4, 2.5, 0.6, 0.9], [35.0, 20.0, 1.11, 1.0], [5.5, 15.0, 0.05, 0.0], a, b, c],
        Scene::WindowLight => {
            // A calm moment (no gust, no bird, clear sky) to start from and for the still frame.
            let t = if time.still { 55.0 } else { (55.0 + time.moved) % 240.0 };
            let [rx, ry, rs] = made::REGION;
            vec![[w, h, t as f32, 0.0], [rx, ry, rs, 0.0], a, b, c]
        }
        _ => vec![[w, h, moved as f32, 0.0], [0.9, 0.0, 1.0, 1.7], [2.5, 50.0, 0.0, 0.05], [500.0, 2.0, 0.1, 2.0], [1.5, 1.0, 1.0, 0.9], a, b, c],
    }
}

impl Pattern {
    #[allow(clippy::too_many_arguments)]
    fn draw(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, pipelines: &Made, out: &wgpu::TextureView, css: [f32; 2], [_, h]: [f32; 2], backdrop: &Backdrop, time: Time) {
        let [a, b, c] = backdrop.colors;
        // It rises over two seconds.
        let intro = if time.still { 1.0 } else { (time.moved / 2.0).clamp(0.0, 1.0) as f32 };
        let appear = 1.0 - (1.0 - (intro / 0.75).clamp(0.0, 1.0)).powi(3);
        let rise = ((intro - 0.1) / 0.9).clamp(0.0, 1.0);
        let amp = rise * rise * (3.0 - 2.0 * rise);
        let t = if time.still { 0.0 } else { time.moved * 0.15 } as f32;
        let heading = 20.0f32.to_radians();
        let lit = 200.0f32.to_radians();
        let ink = luma(rgb(b)) < luma(rgb(a));
        let grid = [self.origin[0], self.origin[1], self.pitch, self.pitch];
        let floats: [[f32; 4]; 5] = [
            [css[0], css[1], self.dpr, appear],
            grid,
            [heading.cos(), heading.sin(), t, 520.0],
            [lit.cos() * 0.78, lit.sin() * 0.78, 0.62, amp],
            [0.95, 0.8, 1.2, if ink { 1.0 } else { 0.0 }],
        ];
        queue.write_buffer(&self.field_params, 0, bytemuck::cast_slice(&floats));
        pass(encoder, &[&self.field], wgpu::Color::TRANSPARENT, pipelines.get("pattern-field"), &self.field_group, ONCE);
        let marks: [[f32; 4]; 5] = [grid, [self.cells[0] as f32, self.cells[1] as f32, 0.95, h], rgba(b), rgba(c), rgba(a)];
        queue.write_buffer(&self.mark_params, 0, bytemuck::cast_slice(&marks));
        pass(encoder, &[out], wgpu::Color::TRANSPARENT, pipelines.get("pattern-mark"), &self.mark_group, ONCE);
    }
}

impl Ascii {
    #[allow(clippy::too_many_arguments)]
    fn draw(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, pipelines: &Made, out: &wgpu::TextureView, [w, h]: [f32; 2], backdrop: &Backdrop, time: Time) {
        let [a, b, c] = backdrop.colors;
        let white = Color32::WHITE;
        let light = look::luminance(a) > 0.45;
        let ramp = if light {
            [mixed_bytes(b, white, 0.55), mixed_bytes(c, white, 0.35), c, backdrop.primary]
        } else {
            [b, mixed_bytes(c, white, 0.85), mixed_bytes(c, white, 0.45), backdrop.primary]
        };
        let (t, glyph_t, intro, seed) =
            if time.still { (0.0, 0.0, 1.0, 0.0) } else { (time.moved * 0.3, time.moved * 0.6, (time.moved / 2.4).min(1.0), (time.moved * 24.0).floor() % 97.0) };
        let columns = made::CHARACTERS.chars().count() as f32;
        let [cw, ch] = self.cell;
        let floats: [[f32; 4]; 11] = [
            [w, h, t as f32, glyph_t as f32],
            [cw, ch, (w - (w / cw).ceil() * cw) / 2.0, (h - (h / ch).ceil() * ch) / 2.0],
            [columns, 1.0, columns, w / self.halo_size[0] as f32],
            [0.5, 0.5, intro as f32, seed as f32],
            [0.8, 0.85, if time.still { 0.0 } else { 0.2 }, if light { 1.0 } else { 0.0 }],
            [0.2, 0.9, 0.06, w / h.max(1.0)],
            rgba(a),
            lab(ramp[0]),
            lab(ramp[1]),
            lab(ramp[2]),
            lab(ramp[3]),
        ];
        queue.write_buffer(&self.params, 0, bytemuck::cast_slice(&floats));
        let clear = wgpu::Color::TRANSPARENT;
        pass(encoder, &[&self.field, &self.aux], clear, pipelines.get("ascii-field"), &self.field_group, ONCE);
        pass(encoder, &[&self.halo_a], clear, pipelines.get("ascii-halo"), &self.halo_group, ONCE);
        pass(encoder, &[&self.halo_b], clear, pipelines.get("ascii-blur"), &self.blur_x_group, ONCE);
        pass(encoder, &[&self.halo_a], clear, pipelines.get("ascii-blur"), &self.blur_y_group, ONCE);
        pass(encoder, &[out], clear, pipelines.get("ascii-composite"), &self.composite_group, ONCE);
    }
}

impl Aero {
    #[allow(clippy::too_many_arguments)]
    fn draw(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, pipelines: &Made, out: &wgpu::TextureView, [w, h]: [f32; 2], backdrop: &Backdrop, moved: f64) {
        const SPEED: f64 = 0.2 * 0.34;
        let aspect = w / h.max(1.0);
        let length = f64::from((2.44 * aspect).hypot(5.0f32.sqrt()));
        // The stream goes on from where it is when the window's shape, and so its path, changes.
        self.travel = (self.travel + (moved - self.at).max(0.0) * SPEED / length) % 1.0;
        self.at = moved;
        let [a, b, c] = backdrop.colors.map(rgb);
        let k = ((luma(a) - 0.58) / 0.24).clamp(0.0, 1.0);
        let surface = k * k * (3.0 - 2.0 * k);
        let light = (0.38f32 * 0.38 + 0.58 * 0.58 + 1.0).sqrt();
        let floats: [[f32; 4]; 18] = [
            [aspect, 0.0125 * 0.96 * 1.1, 1.0, (moved * SPEED) as f32],
            [1.0, 1.0, 0.36, 0.0],
            [1.0, 2.0, 1.0 + (0.2f32 * 0.34).min(1.2) * 0.1, 1.12 + (0.96 - 1.12) * surface],
            [0.0, 0.0, 0.0, 1.0],
            [self.travel as f32, 0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0; 4],
            [0.0, 0.0, 0.54, 0.0],
            [0.0; 4],
            [0.0; 4],
            [0.0; 4],
            [0.0; 4],
            [0.46, 0.0, 0.92, 0.54],
            [-0.38 / light, 0.58 / light, 1.0 / light, 0.0],
            [surface, 0.0, 0.0, 0.0],
            four(b),
            four(mixed(c, [1.0; 3], 0.78)),
            four(c),
        ];
        queue.write_buffer(&self.view, 0, bytemuck::cast_slice(&floats));
        let post: [[f32; 4]; 6] = [
            [w, h, 1.0 / w, 1.0 / h],
            [1.0 / self.bloom_size[0] as f32, 1.0 / self.bloom_size[1] as f32, 0.2, 0.12],
            [0.5, 0.05, 0.0075, surface],
            four(a),
            [moved as f32, 0.0, 0.0, 0.0],
            four(mixed(b, c, 0.4)),
        ];
        queue.write_buffer(&self.post, 0, bytemuck::cast_slice(&post));
        let ground = wgpu::Color { r: a[0] as f64, g: a[1] as f64, b: a[2] as f64, a: 1.0 };
        pass(encoder, &[&self.scene], ground, pipelines.get("aero-shards"), &self.shards_group, (6, SHARDS));
        pass(encoder, &[&self.bloom], wgpu::Color::BLACK, pipelines.get("aero-bloom"), &self.bloom_group, ONCE);
        pass(encoder, &[&self.scratch], wgpu::Color::BLACK, pipelines.get("aero-blur"), &self.blur_x_group, ONCE);
        pass(encoder, &[&self.bloom], wgpu::Color::BLACK, pipelines.get("aero-blur"), &self.blur_y_group, ONCE);
        pass(encoder, &[out], ground, pipelines.get("aero-finish"), &self.finish_group, ONCE);
    }
}

/// The foliage, painted once per process.
fn leaves() -> &'static [u8] {
    static LEAVES: OnceLock<Vec<u8>> = OnceLock::new();
    LEAVES.get_or_init(made::leaves)
}

/// The characters, drawn once per process.
fn levels() -> &'static [(usize, usize, Vec<u8>)] {
    static LEVELS: OnceLock<Vec<(usize, usize, Vec<u8>)>> = OnceLock::new();
    LEVELS.get_or_init(made::glyphs)
}

fn atlas(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
    let levels = levels();
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("orbiters glyphs"), // i18n: not shown (names a graphics debugger shows)
        size: wgpu::Extent3d { width: levels[0].0 as u32, height: levels[0].1 as u32, depth_or_array_layers: 1 },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (i, (w, h, bytes)) in levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &t, mip_level: i as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(*w as u32), rows_per_image: Some(*h as u32) },
            wgpu::Extent3d { width: *w as u32, height: *h as u32, depth_or_array_layers: 1 },
        );
    }
    t
}
