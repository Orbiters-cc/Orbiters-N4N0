//! An edit's layers on the GPU, for a picture that changes every frame: the 3D view's, edited while the camera moves.
//! Each layer is a compute pass or a few (`gpu.wgsl`) over half-float textures in linear light with premultiplied
//! alpha, as [`super::Image`], with the maths of the CPU passes ([`super::tone`], [`super::effects`], [`super::blur`],
//! [`super::text`], [`super::geometry`]): the same tables, kernels, box widths, running sums and letters (texts are
//! drawn on the CPU and kept while they do not change), so that a picture saved from here looks as the CPU editor
//! makes it (the tests compare both). Like [`super::render::Renderer`], a [`Stack`] kept between frames keeps the
//! picture before the layer being changed while the source stays the same (a slider dragged with the camera still
//! redoes that layer and those over it), and does nothing when neither changed.
//!
//! What it takes: an RGBA8 texture of encoded sRGB (straight alpha), the source; what it makes: an RGBA8 texture of
//! encoded sRGB with straight alpha ([`Stack::output`]), the picture as [`super::Image::to_rgba8`] would make it.
//! VRChat's multi-layer pictures ([`Edit::sources`]) are not drawn here: one source only.

use super::render::{Stage, DIRECTIONAL_MAX, GAUSSIAN_MAX, LENS_MAX};
use super::{blur, text, tone, BlurKind, Comic, Crop, Edit, Effect, Layer, Mask, Rgba8};
use anyhow::Context as _;
use egui_wgpu::wgpu;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// The working pictures' format: linear light, premultiplied.
const WORK: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// What comes in and goes out: encoded sRGB, straight alpha.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const SHADER: &str = include_str!("gpu.wgsl");
/// Bytes of a pass's parameters (the uniform buffer's offsets are this far apart).
const PARAMS: u64 = 256;
/// Entries of a tone table ([`tone::Lut`]).
const LUT: usize = 1025;
/// Working textures: the picture and the next (taking turns), and three for a layer's passes in between.
const WORKING: usize = 5;

/// A pass's parameters, as `Params` in `gpu.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    size: [f32; 2],
    opacity: f32,
    mixing: f32,
    mask: [f32; 4],
    mask_at: [f32; 4],
    ints: [i32; 4],
    v: [[f32; 4]; 12],
}

const _: () = assert!(std::mem::size_of::<Params>() == PARAMS as usize);

impl Params {
    fn new(size: (u32, u32)) -> Self {
        Self { size: [size.0 as f32, size.1 as f32], opacity: 1.0, mixing: 0.0, mask: [0.0; 4], mask_at: [0.0; 4], ints: [0; 4], v: [[0.0; 4]; 12] }
    }

    /// Mixed back towards the picture before the layer by its opacity and mask (unless it shows whole).
    fn layer(size: (u32, u32), l: &Layer) -> Self {
        let mut p = Self::new(size);
        let whole = l.opacity >= 1.0 && l.mask == Mask::Everywhere;
        p.opacity = l.opacity.clamp(0.0, 1.0);
        p.mixing = if whole { 0.0 } else { 1.0 };
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        match l.mask {
            Mask::Everywhere => {}
            Mask::Radial { center, radius, feather, invert } => {
                p.mask = [1.0, flag(invert), radius, feather];
                p.mask_at = [center[0], center[1], 0.0, 0.0];
            }
            Mask::Band { center, angle, width, feather, invert } => {
                let (s, c) = angle.to_radians().sin_cos();
                p.mask = [2.0, flag(invert), width, feather];
                p.mask_at = [center[0], center[1], s, c];
            }
        }
        p
    }
}

/// The passes, by their entry point in `gpu.wgsl`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Take,
    Light,
    Color,
    Curves,
    Vignette,
    Aberration,
    ComicLuma,
    Comic,
    GaussH,
    GaussV,
    BoxH,
    BoxV,
    Directional,
    LensSums,
    Lens,
    Text,
    Out,
}

impl Entry {
    const ALL: [Entry; 17] = [
        Entry::Take,
        Entry::Light,
        Entry::Color,
        Entry::Curves,
        Entry::Vignette,
        Entry::Aberration,
        Entry::ComicLuma,
        Entry::Comic,
        Entry::GaussH,
        Entry::GaussV,
        Entry::BoxH,
        Entry::BoxV,
        Entry::Directional,
        Entry::LensSums,
        Entry::Lens,
        Entry::Text,
        Entry::Out,
    ];

    fn name(self) -> &'static str {
        match self {
            Entry::Take => "take",
            Entry::Light => "light",
            Entry::Color => "color",
            Entry::Curves => "curves",
            Entry::Vignette => "vignette",
            Entry::Aberration => "aberration",
            Entry::ComicLuma => "comic_luma",
            Entry::Comic => "comic",
            Entry::GaussH => "gauss_h",
            Entry::GaussV => "gauss_v",
            Entry::BoxH => "box_h",
            Entry::BoxV => "box_v",
            Entry::Directional => "directional",
            Entry::LensSums => "lens_sums",
            Entry::Lens => "lens",
            Entry::Text => "text",
            Entry::Out => "out",
        }
    }
}

/// A texture a pass reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tex {
    /// The source given to [`Stack::run`].
    Input,
    Work(usize),
    /// The picture kept before the layer being changed.
    Kept,
    /// A text's letters, by its place in [`Stack::texts`].
    Text(usize),
    /// The picture made.
    Output,
}

/// What a run does, in order.
enum Step {
    Pass { entry: Entry, src: Tex, dst: Tex, aux: Tex, params: Params, groups: (u32, u32) },
    /// The picture so far copied aside (before the layer being changed).
    Keep(Tex),
}

/// The 1D passes of a blur (along the rows, then down the columns).
#[derive(Clone, Copy)]
enum Pass1d {
    Gauss { offset: i32, reach: i32 },
    Box { reach: i32 },
}

struct Working {
    size: (u32, u32),
    textures: Vec<(wgpu::Texture, wgpu::TextureView)>,
    kept: (wgpu::Texture, wgpu::TextureView),
}

struct Lettering {
    key: u64,
    view: wgpu::TextureView,
    size: (u32, u32),
    used: bool,
}

/// An edit drawn on the GPU, kept between frames.
pub struct Stack {
    device: wgpu::Device,
    queue: wgpu::Queue,
    work_layout: wgpu::BindGroupLayout,
    out_layout: wgpu::BindGroupLayout,
    pipelines: Vec<wgpu::ComputePipeline>,
    uniforms: wgpu::Buffer,
    data: wgpu::Buffer,
    sums: wgpu::Buffer,
    weights: wgpu::Buffer,
    working: Option<Working>,
    output: Option<((u32, u32), wgpu::Texture, wgpu::TextureView)>,
    texts: Vec<Lettering>,
    /// Keep the picture before the layer being changed (live views; a single picture needs none).
    keep: bool,
    /// Counts the sources given (a new one each time it changed).
    generation: u64,
    /// The kept picture: the key of the stack under it and how many layers that is.
    kept: Option<(u64, usize)>,
    /// The last stack drawn, layer by layer: the first that differs next time is being changed.
    last: Vec<u64>,
    /// What the output shows.
    drawn: Option<u64>,
}

fn hash(v: &impl std::fmt::Debug) -> u64 {
    let mut h = DefaultHasher::new();
    format!("{v:?}").hash(&mut h);
    h.finish()
}

fn storage_buffer(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn layout(device: &wgpu::Device, dst: wgpu::TextureFormat) -> wgpu::BindGroupLayout {
    let texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    };
    let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("photo layers"), // i18n: not shown (names a graphics debugger shows)
        entries: &[
            texture(0),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: dst, view_dimension: wgpu::TextureViewDimension::D2 },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(PARAMS) },
                count: None,
            },
            texture(3),
            storage(4, true),
            storage(5, false),
            storage(6, false),
        ],
    })
}

impl Stack {
    /// Its passes made on `device` (a few dozen milliseconds: off the interface thread when it can be).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let work_layout = layout(device, WORK);
        let out_layout = layout(device, FORMAT);
        let module = |format: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("photo layers"), source: wgpu::ShaderSource::Wgsl(SHADER.replace("DST_FORMAT", format).into()) }) // i18n: not shown (names a graphics debugger shows)
        };
        let (work_module, out_module) = (module("rgba16float"), module("rgba8unorm"));
        let pipeline_layout = |l: &wgpu::BindGroupLayout| device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("photo layers"), bind_group_layouts: &[Some(l)], immediate_size: 0 }); // i18n: not shown (names a graphics debugger shows)
        let (work_pl, out_pl) = (pipeline_layout(&work_layout), pipeline_layout(&out_layout));
        let pipelines = Entry::ALL
            .iter()
            .map(|e| {
                let (layout, module) = if *e == Entry::Out { (&out_pl, &out_module) } else { (&work_pl, &work_module) };
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(e.name()),
                    layout: Some(layout),
                    module,
                    entry_point: Some(e.name()),
                    compilation_options: Default::default(),
                    cache: None,
                })
            })
            .collect();
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photo layers' parameters"), // i18n: not shown (names a graphics debugger shows)
            size: PARAMS * 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            data: storage_buffer(device, "photo layers' tables", 4096 * 4), // i18n: not shown (names a graphics debugger shows)
            sums: storage_buffer(device, "lens blur sums", 16), // i18n: not shown (names a graphics debugger shows)
            weights: storage_buffer(device, "lens blur weights", 16), // i18n: not shown (names a graphics debugger shows)
            device: device.clone(),
            queue: queue.clone(),
            work_layout,
            out_layout,
            pipelines,
            uniforms,
            working: None,
            output: None,
            texts: Vec::new(),
            keep: false,
            generation: 0,
            kept: None,
            last: Vec::new(),
            drawn: None,
        }
    }

    /// For a live view: keeps the picture before the layer being changed.
    pub fn live(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self { keep: true, ..Self::new(device, queue) }
    }

    /// The picture made last: RGBA8, encoded sRGB, straight alpha, of the size asked.
    pub fn output(&self) -> Option<&wgpu::TextureView> {
        self.output.as_ref().map(|o| &o.2)
    }

    pub fn output_size(&self) -> Option<(u32, u32)> {
        self.output.as_ref().map(|o| o.0)
    }

    /// Makes the edit of `input` (an RGBA8 texture of encoded sRGB of `input_size`; at full size the picture is `full`
    /// pixels, as for [`super::render::Renderer::render`]) at `size` for `stage`, into [`Stack::output`]. `fresh`: the
    /// source changed since the last run (else, with the same edit, nothing is done: false).
    #[allow(clippy::too_many_arguments)]
    pub fn run(&mut self, edit: &Edit, input: &wgpu::TextureView, input_size: (u32, u32), full: (u32, u32), stage: Stage, size: (u32, u32), fresh: bool) -> bool {
        let size = (size.0.max(1), size.1.max(1));
        if fresh {
            self.generation += 1;
        }
        let crop = match stage {
            Stage::Uncropped => Crop::FULL,
            _ => edit.geometry.effective_crop(full.0, full.1),
        };
        let base = hash(&(self.generation, &edit.geometry, crop, stage, size, input_size, full));
        let layers: &[Layer] = if stage == Stage::Full { &edit.layers } else { &[] };
        let keys: Vec<u64> = layers.iter().map(hash).collect();
        let prefix: Vec<u64> = (0..=layers.len()).map(|k| hash(&(base, &keys[..k]))).collect();
        if self.drawn == Some(prefix[layers.len()]) && self.output.as_ref().is_some_and(|o| o.0 == size) {
            return false;
        }
        let changed = keys.iter().zip(&self.last).position(|(a, b)| a != b).unwrap_or(keys.len().min(self.last.len()));
        self.last = keys;
        self.textures(size);

        let mut steps = Vec::new();
        let mut data: Vec<f32> = Vec::new();
        let mut lens_rows = 0u64;
        self.texts.iter_mut().for_each(|t| t.used = false);
        let groups = (size.0.div_ceil(8), size.1.div_ceil(8));
        let resume = self.kept.filter(|(h, k)| *k <= layers.len() && prefix[*k] == *h).map(|(_, k)| k);
        let (start, mut cur) = match resume {
            Some(k) => (k, Tex::Kept),
            None => {
                let mut p = Params::new(size);
                let h = homography(edit, crop, input_size);
                p.v[0] = [h[0][0], h[0][1], h[0][2], 0.0];
                p.v[1] = [h[1][0], h[1][1], h[1][2], 0.0];
                p.v[2] = [h[2][0], h[2][1], h[2][2], 0.0];
                let plain = edit.geometry.warp(input_size.0, input_size.1).is_identity();
                p.v[3] = [input_size.0 as f32, input_size.1 as f32, if plain { 1.0 } else { 0.0 }, 0.0];
                steps.push(Step::Pass { entry: Entry::Take, src: Tex::Input, dst: Tex::Work(0), aux: Tex::Input, params: p, groups });
                (0, Tex::Work(0))
            }
        };
        for (i, l) in layers.iter().enumerate().skip(start) {
            if self.keep && i == changed && self.kept.is_none_or(|(h, _)| h != prefix[i]) && cur != Tex::Kept {
                steps.push(Step::Keep(cur));
                self.kept = Some((prefix[i], i));
            }
            cur = self.layer(&mut steps, &mut data, &mut lens_rows, cur, l, size);
        }
        steps.push(Step::Pass { entry: Entry::Out, src: cur, dst: Tex::Output, aux: cur, params: Params::new(size), groups });
        self.encode(&steps, &data, lens_rows, input);
        // Letters no layer drew this time go (after the passes that name the others by their place).
        self.texts.retain(|t| t.used);
        self.drawn = Some(prefix[layers.len()]);
        true
    }

    /// The working textures and the output at `size`.
    fn textures(&mut self, size: (u32, u32)) {
        let texture = |label: &str, format, usage| {
            let t = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            });
            let v = t.create_view(&Default::default());
            (t, v)
        };
        let work = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST;
        if self.working.as_ref().is_none_or(|w| w.size != size) {
            self.working = Some(Working {
                size,
                textures: (0..WORKING).map(|_| texture("photo layer", WORK, work)).collect(), // i18n: not shown (names a graphics debugger shows)
                kept: texture("photo layer kept", WORK, work), // i18n: not shown (names a graphics debugger shows)
            });
            self.kept = None;
        }
        if self.output.as_ref().is_none_or(|o| o.0 != size) {
            let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC;
            let (t, v) = texture("photo layers' picture", FORMAT, usage); // i18n: not shown (names a graphics debugger shows)
            self.output = Some((size, t, v));
        }
    }

    /// The passes of one layer over `cur`; returns where its result is.
    fn layer(&mut self, steps: &mut Vec<Step>, data: &mut Vec<f32>, lens_rows: &mut u64, cur: Tex, l: &Layer, size: (u32, u32)) -> Tex {
        // A depth blur is drawn on the splats before the layers (Edit in 3D).
        if !l.visible || l.opacity <= 0.0 || l.effect.on_splats() {
            return cur;
        }
        let next = if cur == Tex::Work(0) { Tex::Work(1) } else { Tex::Work(0) };
        let groups = (size.0.div_ceil(8), size.1.div_ceil(8));
        let h = size.1 as f32;
        let mut p = Params::layer(size, l);
        let mut table = |values: &mut dyn Iterator<Item = f32>| -> i32 {
            let at = data.len() as i32;
            data.extend(values);
            at
        };
        let entry = match &l.effect {
            Effect::Light(light) => {
                p.v[0][0] = 2f32.powf(light.exposure.clamp(-4.0, 4.0));
                p.ints[0] = table(&mut (0..LUT).map(|i| tone::light_curve(light, i as f32 / (LUT - 1) as f32)));
                Entry::Light
            }
            Effect::Color(c) => {
                let g = tone::balance(c.temperature, c.tint);
                p.v[0] = [g[0], g[1], g[2], if c.hue != 0.0 { 1.0 } else { 0.0 }];
                let m = tone::hue_matrix(c.hue);
                for (k, row) in m.iter().enumerate() {
                    p.v[1 + k] = [row[0], row[1], row[2], 0.0];
                }
                p.v[4] = [c.saturation.clamp(-1.0, 1.0), c.vibrance.clamp(-1.0, 1.0), 0.0, 0.0];
                Entry::Color
            }
            Effect::Curves(c) => {
                let luts = tone::curve_luts(c);
                p.ints[0] = table(&mut luts.iter().flat_map(|lut| (0..LUT).map(|i| lut.at(i as f32 / (LUT - 1) as f32))));
                Entry::Curves
            }
            Effect::Vignette(v) => {
                let exponent = 0.6 + v.feather.clamp(0.0, 1.0) * 3.2;
                p.v[0] = [v.amount.clamp(-1.0, 1.0), v.size.clamp(0.2, 3.0), exponent, v.center[0]];
                p.v[1][0] = v.center[1];
                Entry::Vignette
            }
            Effect::Aberration(a) => {
                if a.amount <= 0.0 {
                    return cur;
                }
                p.v[0] = [a.amount.clamp(0.0, 3.0) * 0.012, a.center[0], a.center[1], 0.0];
                Entry::Aberration
            }
            Effect::Comic(c) => {
                let ink = c.ink.clamp(0.0, 1.0);
                let steps_n = (c.tones.clamp(Comic::MIN_TONES, Comic::MAX_TONES) - 1).max(1) as f32;
                p.v[0] = [steps_n, if c.dots { 1.0 } else { 0.0 }, c.dot_cell(h), if c.ink > 0.0 { ink } else { 0.0 }];
                let line = Comic::line_width(h);
                p.ints[1] = line.round().max(1.0) as i32;
                let mut soft = cur;
                if c.ink > 0.0 {
                    steps.push(Step::Pass { entry: Entry::ComicLuma, src: cur, dst: Tex::Work(2), aux: cur, params: Params::new(size), groups });
                    let passes = blur_passes(data, (line * 0.6).max(0.6));
                    chain(steps, &passes, Tex::Work(2), Tex::Work(4), [Tex::Work(3), Tex::Work(2)], size, None);
                    soft = Tex::Work(4);
                }
                steps.push(Step::Pass { entry: Entry::Comic, src: cur, dst: next, aux: soft, params: p, groups });
                return next;
            }
            Effect::Blur(b) => match b.kind {
                BlurKind::Gaussian => {
                    let sigma = b.radius.clamp(0.0, GAUSSIAN_MAX) * h;
                    if sigma < 0.3 {
                        return cur;
                    }
                    let passes = blur_passes(data, sigma);
                    chain(steps, &passes, cur, next, [Tex::Work(2), Tex::Work(3)], size, Some((p, cur)));
                    return next;
                }
                BlurKind::Lens => {
                    let radius = b.radius.clamp(0.0, LENS_MAX) * h;
                    if radius < 0.5 {
                        return cur;
                    }
                    let r = radius.ceil() as i32;
                    let span = size.0 as i32 + 2 * r;
                    *lens_rows = (*lens_rows).max((span as u64 + 1) * size.1 as u64);
                    let mut sums = Params::new(size);
                    sums.ints = [0, r, span, 0];
                    sums.v[0][0] = b.bokeh.clamp(0.0, 1.0) * 12.0;
                    steps.push(Step::Pass { entry: Entry::LensSums, src: cur, dst: Tex::Work(2), aux: cur, params: sums, groups: (size.1.div_ceil(64), 1) });
                    p.ints = [0, r, span, 0];
                    p.v[0][0] = radius;
                    Entry::Lens
                }
                BlurKind::Directional => {
                    let length = b.radius.clamp(0.0, DIRECTIONAL_MAX) * h;
                    if length < 1.0 {
                        return cur;
                    }
                    let (s, c) = b.angle.to_radians().sin_cos();
                    p.v[0] = [length, s, c, 0.0];
                    p.ints[1] = (length.ceil() as i32 + 1).max(2);
                    Entry::Directional
                }
                BlurKind::Depth => return cur,
            },
            Effect::Text(t) => {
                let Some(slot) = self.lettering(t, size) else { return cur };
                let (left, top) = slot.1;
                let mut q = Params::new(size);
                q.ints = [left, top, 0, 0];
                q.v[0][0] = l.opacity.clamp(0.0, 1.0);
                steps.push(Step::Pass { entry: Entry::Text, src: cur, dst: next, aux: Tex::Text(slot.0), params: q, groups });
                return next;
            }
        };
        steps.push(Step::Pass { entry, src: cur, dst: next, aux: cur, params: p, groups });
        next
    }

    /// A text's letters as a texture (drawn again only when the text or the picture's height changed), and where its
    /// top left goes.
    fn lettering(&mut self, t: &super::Text, size: (u32, u32)) -> Option<(usize, (i32, i32))> {
        let key = hash(&(t, size.1));
        let index = match self.texts.iter().position(|x| x.key == key) {
            Some(i) => i,
            None => {
                let d = text::draw(t, size.1)?;
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("photo text"), // i18n: not shown (names a graphics debugger shows)
                    size: wgpu::Extent3d { width: d.width, height: d.height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                self.queue.write_texture(
                    texture.as_image_copy(),
                    bytemuck::cast_slice(&d.pixels),
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(d.width * 16), rows_per_image: Some(d.height) },
                    wgpu::Extent3d { width: d.width, height: d.height, depth_or_array_layers: 1 },
                );
                self.texts.push(Lettering { key, view: texture.create_view(&Default::default()), size: (d.width, d.height), used: false });
                self.texts.len() - 1
            }
        };
        let (dw, dh) = self.texts[index].size;
        self.texts[index].used = true;
        let left = (t.center[0] * size.0 as f32 - dw as f32 * 0.5).round() as i32;
        let top = (t.center[1] * size.1 as f32 - dh as f32 * 0.5).round() as i32;
        Some((index, (left, top)))
    }

    fn view<'a>(&'a self, t: Tex, input: &'a wgpu::TextureView) -> &'a wgpu::TextureView {
        let w = self.working.as_ref().expect("working textures");
        match t {
            Tex::Input => input,
            Tex::Work(i) => &w.textures[i].1,
            Tex::Kept => &w.kept.1,
            Tex::Text(i) => &self.texts[i].view,
            Tex::Output => &self.output.as_ref().expect("output").2,
        }
    }

    fn texture(&self, t: Tex) -> &wgpu::Texture {
        let w = self.working.as_ref().expect("working textures");
        match t {
            Tex::Work(i) => &w.textures[i].0,
            Tex::Kept => &w.kept.0,
            _ => unreachable!("only working pictures are kept"),
        }
    }

    /// Writes the parameters and tables, then records and submits the passes.
    fn encode(&mut self, steps: &[Step], data: &[f32], lens_rows: u64, input: &wgpu::TextureView) {
        let params: Vec<Params> = steps.iter().filter_map(|s| if let Step::Pass { params, .. } = s { Some(*params) } else { None }).collect();
        let bytes = params.len() as u64 * PARAMS;
        if self.uniforms.size() < bytes {
            self.uniforms = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("photo layers' parameters"), // i18n: not shown (names a graphics debugger shows)
                size: bytes.next_power_of_two(),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        self.queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&params));
        let data_bytes = (data.len() as u64 * 4).max(16);
        if self.data.size() < data_bytes {
            self.data = storage_buffer(&self.device, "photo layers' tables", data_bytes.next_power_of_two()); // i18n: not shown (names a graphics debugger shows)
        }
        if !data.is_empty() {
            self.queue.write_buffer(&self.data, 0, bytemuck::cast_slice(data));
        }
        if lens_rows > 0 && self.weights.size() < lens_rows * 4 {
            self.sums = storage_buffer(&self.device, "lens blur sums", lens_rows * 16); // i18n: not shown (names a graphics debugger shows)
            self.weights = storage_buffer(&self.device, "lens blur weights", lens_rows * 4); // i18n: not shown (names a graphics debugger shows)
        }
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("photo layers") }); // i18n: not shown (names a graphics debugger shows)
        let mut index = 0u64;
        let mut i = 0;
        while i < steps.len() {
            // Passes up to the next picture kept, in one compute pass.
            {
                let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("photo layers"), timestamp_writes: None }); // i18n: not shown (names a graphics debugger shows)
                while let Some(Step::Pass { entry, src, dst, aux, groups, .. }) = steps.get(i) {
                    let out = *entry == Entry::Out;
                    let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(entry.name()),
                        layout: if out { &self.out_layout } else { &self.work_layout },
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(self.view(*src, input)) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(self.view(*dst, input)) },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &self.uniforms, offset: 0, size: wgpu::BufferSize::new(PARAMS) }),
                            },
                            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(self.view(*aux, input)) },
                            wgpu::BindGroupEntry { binding: 4, resource: self.data.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 5, resource: self.sums.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 6, resource: self.weights.as_entire_binding() },
                        ],
                    });
                    pass.set_pipeline(&self.pipelines[Entry::ALL.iter().position(|e| e == entry).expect("an entry")]);
                    pass.set_bind_group(0, &bind, &[(index * PARAMS) as u32]);
                    pass.dispatch_workgroups(groups.0, groups.1, 1);
                    index += 1;
                    i += 1;
                }
            }
            if let Some(Step::Keep(from)) = steps.get(i) {
                let size = self.working.as_ref().map_or((1, 1), |w| w.size);
                enc.copy_texture_to_texture(
                    self.texture(*from).as_image_copy(),
                    self.texture(Tex::Kept).as_image_copy(),
                    wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                );
                i += 1;
            }
        }
        self.queue.submit([enc.finish()]);
    }

    /// The picture made last, read back (waits for the GPU): for saving it and for tests.
    pub fn read(&self) -> anyhow::Result<Rgba8> {
        let (size, texture, _) = self.output.as_ref().context("nothing drawn")?;
        read_texture(&self.device, &self.queue, texture, *size)
    }
}

/// The passes of a gaussian of deviation `sigma` pixels, as [`blur::gaussian`]: its exact kernel (put in `data`) up
/// to 3, else three box blurs of [`blur::box_widths`]; along the rows, then down the columns.
fn blur_passes(data: &mut Vec<f32>, sigma: f32) -> Vec<(bool, Pass1d)> {
    if sigma <= 3.0 {
        let kernel = blur::gaussian_kernel(sigma);
        let offset = data.len() as i32;
        let reach = (kernel.len() / 2) as i32;
        data.extend(kernel);
        let g = Pass1d::Gauss { offset, reach };
        vec![(false, g), (true, g)]
    } else {
        let widths = blur::box_widths(sigma, 3);
        let mut out: Vec<(bool, Pass1d)> = widths.iter().map(|w| (false, Pass1d::Box { reach: ((w - 1) / 2) as i32 })).collect();
        out.extend(widths.iter().map(|w| (true, Pass1d::Box { reach: ((w - 1) / 2) as i32 })));
        out
    }
}

/// `passes` from `src` into `out` through `temps` taking turns; the last mixed back towards `mix.1` by the layer's
/// parameters `mix.0` (its opacity and mask).
fn chain(steps: &mut Vec<Step>, passes: &[(bool, Pass1d)], src: Tex, out: Tex, temps: [Tex; 2], size: (u32, u32), mix: Option<(Params, Tex)>) {
    let groups = (size.0.div_ceil(8), size.1.div_ceil(8));
    let mut from = src;
    for (i, (down, pass)) in passes.iter().enumerate() {
        let last = i + 1 == passes.len();
        let to = if last {
            out
        } else if from == temps[0] {
            temps[1]
        } else {
            temps[0]
        };
        let (mut params, aux) = match (last, mix) {
            (true, Some((p, before))) => (p, before),
            _ => (Params::new(size), from),
        };
        let entry = match (pass, down) {
            (Pass1d::Gauss { offset, reach }, d) => {
                params.ints[0] = *offset;
                params.ints[1] = *reach;
                if *d {
                    Entry::GaussV
                } else {
                    Entry::GaussH
                }
            }
            (Pass1d::Box { reach }, d) => {
                params.ints[1] = *reach;
                if *d {
                    Entry::BoxV
                } else {
                    Entry::BoxH
                }
            }
        };
        steps.push(Step::Pass { entry, src: from, dst: to, aux, params, groups });
        from = to;
    }
}

/// The map from shares of the output frame to shares of the source, as [`super::Geometry::source_point`] (quarter
/// turns, flips, straightening and perspective are all projective): the homography through the frame's corners.
fn homography(edit: &Edit, crop: Crop, input: (u32, u32)) -> [[f32; 3]; 3] {
    let aspect = input.0 as f32 / input.1.max(1) as f32;
    let at = |u: f32, v: f32| {
        let p = edit.geometry.source_point(crop, aspect, u, v);
        [p[0] as f64, p[1] as f64]
    };
    square_to_quad([at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0), at(1.0, 1.0)])
}

/// The homography taking the unit square's corners (0,0), (1,0), (0,1), (1,1) to `q` (Heckbert's square-to-quad).
fn square_to_quad(q: [[f64; 2]; 4]) -> [[f32; 3]; 3] {
    let [p00, p10, p01, p11] = q;
    let (dx1, dx2, dx3) = (p10[0] - p11[0], p01[0] - p11[0], p00[0] - p10[0] + p11[0] - p01[0]);
    let (dy1, dy2, dy3) = (p10[1] - p11[1], p01[1] - p11[1], p00[1] - p10[1] + p11[1] - p01[1]);
    let det = dx1 * dy2 - dx2 * dy1;
    let (g, h) = if (dx3.abs() < 1e-12 && dy3.abs() < 1e-12) || det.abs() < 1e-18 { (0.0, 0.0) } else { ((dx3 * dy2 - dx2 * dy3) / det, (dx1 * dy3 - dx3 * dy1) / det) };
    let m = [
        [p10[0] - p00[0] + g * p10[0], p01[0] - p00[0] + h * p01[0], p00[0]],
        [p10[1] - p00[1] + g * p10[1], p01[1] - p00[1] + h * p01[1], p00[1]],
        [g, h, 1.0],
    ];
    m.map(|r| r.map(|v| v as f32))
}

/// A texture of `picture` for [`Stack::run`] (RGBA8, encoded, straight).
pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, picture: &Rgba8) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("photo source"), // i18n: not shown (names a graphics debugger shows)
        size: wgpu::Extent3d { width: picture.width, height: picture.height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &picture.pixels,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(picture.width * 4), rows_per_image: Some(picture.height) },
        wgpu::Extent3d { width: picture.width, height: picture.height, depth_or_array_layers: 1 },
    );
    let view = texture.create_view(&Default::default());
    (texture, view)
}

/// An RGBA8 texture of `size` read back (waits for the GPU).
pub fn read_texture(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, size: (u32, u32)) -> anyhow::Result<Rgba8> {
    let (w, h) = size;
    let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("photo layers read back"), // i18n: not shown (names a graphics debugger shows)
        size: u64::from(row) * u64::from(h),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("waiting for the GPU: {e}"))?;
    rx.recv()?.map_err(|e| anyhow::anyhow!("reading the picture back: {e}"))?;
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    {
        let data = slice.get_mapped_range().map_err(|e| anyhow::anyhow!("reading the picture back: {e:?}"))?;
        for r in data.chunks_exact(row as usize) {
            pixels.extend_from_slice(&r[..(w * 4) as usize]);
        }
    }
    buffer.unmap();
    Ok(Rgba8::new(w, h, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo::render::{self, Renderer};
    use crate::photo::{Aberration, Blur, Color, Curves, Kind, Light, Text, TextStyle, Vignette};

    /// A device of the GPU the tests run on; None without one (the tests then check nothing).
    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        pollster::block_on(adapter.request_device(&Default::default())).ok()
    }

    /// Gradients, a hard edge, bright points and, with `alpha`, a transparent corner fading in.
    fn picture(w: u32, h: u32, alpha: bool) -> Rgba8 {
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let mut c = [(u * 230.0) as u8 + 10, (v * 200.0) as u8 + 20, (((u - v).abs() * 255.0) as u8).max(30)];
                if x > w / 2 && y > h / 3 && y < h * 2 / 3 {
                    c = [240, 235, 225];
                }
                if (x * 7 + y * 3) % 23 == 0 {
                    c = [255, 255, 255];
                }
                let a = if alpha && u + v < 0.5 { ((u + v) * 2.0 * 255.0) as u8 } else { 255 };
                px.extend_from_slice(&[c[0], c[1], c[2], a]);
            }
        }
        Rgba8::new(w, h, px)
    }

    /// How far apart two pictures are: the worst byte, the mean difference and the share of bytes off by more than 2.
    fn apart(a: &Rgba8, b: &Rgba8) -> (u8, f32, f32) {
        assert_eq!((a.width, a.height), (b.width, b.height));
        let d: Vec<u8> = a.pixels.iter().zip(&b.pixels).map(|(x, y)| x.abs_diff(*y)).collect();
        let worst = d.iter().copied().max().unwrap_or(0);
        let mean = d.iter().map(|v| *v as f32).sum::<f32>() / d.len() as f32;
        let off = d.iter().filter(|v| **v > 2).count() as f32 / d.len() as f32;
        (worst, mean, off)
    }

    fn cpu(edit: &Edit, src: &Rgba8, stage: Stage, size: (u32, u32)) -> Rgba8 {
        Renderer::default().render(edit, &[src], (src.width, src.height), stage, size, &mut |_| true).unwrap().to_rgba8()
    }

    fn gpu(stack: &mut Stack, view: &wgpu::TextureView, edit: &Edit, src: &Rgba8, stage: Stage, size: (u32, u32)) -> Rgba8 {
        stack.run(edit, view, (src.width, src.height), (src.width, src.height), stage, size, true);
        stack.read().unwrap()
    }

    /// Each kind of layer and the variants of its maths, alone, on the GPU and the CPU: the same picture but for
    /// rounding (half floats on the GPU, tables on the CPU), and the few pixels a step of the comic's tones or ink
    /// falls on the other side of.
    #[test]
    fn every_layer_draws_on_the_gpu_as_on_the_cpu() {
        let Some((device, queue)) = device() else { return };
        let mut stack = Stack::new(&device, &queue);
        for alpha in [false, true] {
            let src = picture(120, 80, alpha);
            let (_t, view) = upload(&device, &queue, &src);
            let mut effects: Vec<(String, Effect)> = Kind::ALL.iter().map(|k| (k.name().to_string(), k.effect())).collect();
            effects.extend([
                ("light, every slider".into(), Effect::Light(Light { exposure: 0.7, brightness: 0.3, contrast: 0.5, highlights: -0.4, shadows: 0.6, black_point: 0.05 })),
                ("light, faded black".into(), Effect::Light(Light { exposure: -0.5, contrast: -0.6, black_point: -0.2, ..Default::default() })),
                ("color with a hue".into(), Effect::Color(Color { temperature: 0.4, tint: -0.3, saturation: 0.5, vibrance: 0.6, hue: 35.0 })),
                ("color, grey".into(), Effect::Color(Color { saturation: -1.0, ..Default::default() })),
                ("curves".into(), Effect::Curves(Curves { rgb: vec![[0.0, 0.1], [0.4, 0.3], [1.0, 0.95]], red: vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]], ..Default::default() })),
                ("vignette, lighter, moved".into(), Effect::Vignette(Vignette { amount: -0.7, size: 0.6, feather: 0.9, center: [0.3, 0.6] })),
                ("aberration, strong".into(), Effect::Aberration(Aberration { amount: 3.0, center: [0.2, 0.3] })),
                ("comic with dots".into(), Effect::Comic(Comic { tones: 6, dots: true, dot_size: 0.8, ink: 1.0 })),
                ("comic, no ink".into(), Effect::Comic(Comic { tones: 3, dots: false, dot_size: 0.3, ink: 0.0 })),
                ("gaussian, box path".into(), Effect::Blur(Blur { kind: BlurKind::Gaussian, radius: 0.05, ..Default::default() })),
                ("lens with bokeh".into(), Effect::Blur(Blur { kind: BlurKind::Lens, radius: 0.04, bokeh: 0.9, ..Default::default() })),
                ("lens, small".into(), Effect::Blur(Blur { kind: BlurKind::Lens, radius: 0.012, bokeh: 0.0, ..Default::default() })),
                ("directional".into(), Effect::Blur(Blur { kind: BlurKind::Directional, radius: 0.08, angle: 30.0, ..Default::default() })),
                ("text, glow".into(), Effect::Text(Text { value: "Hi 3D".into(), style: TextStyle::Glow, center: [0.4, 0.5], size: 0.3, ..Default::default() })),
                ("text, shadow".into(), Effect::Text(Text { value: "Orbiters".into(), style: TextStyle::Shadow, size: 0.2, color: [250, 120, 40], ..Default::default() })),
            ]);
            for (name, effect) in effects {
                for (masked, opacity) in [(false, 1.0), (true, 0.6)] {
                    let mut edit = Edit::default();
                    let mut l = edit.layer(effect.clone());
                    l.opacity = opacity;
                    if masked {
                        l.mask = if name.len() % 2 == 0 { Mask::radial() } else { Mask::Band { center: [0.4, 0.5], angle: 20.0, width: 0.3, feather: 0.2, invert: true } };
                    }
                    edit.layers.push(l);
                    let size = (120, 80);
                    let (worst, mean, off) = apart(&gpu(&mut stack, &view, &edit, &src, Stage::Full, size), &cpu(&edit, &src, Stage::Full, size));
                    let comic = name.to_lowercase().starts_with("comic");
                    let (max_mean, max_off) = if comic { (0.6, 0.012) } else { (0.35, 0.002) };
                    eprintln!("{name:>26} (alpha {alpha:5}, masked {masked:5}): worst {worst:3}, mean {mean:.3}, {:.3} % off by more than 2", off * 100.0);
                    assert!(mean <= max_mean && off <= max_off && (comic || worst <= 6), "{name} (alpha {alpha}, masked {masked}): worst {worst}, mean {mean:.3}, {:.2} % off by more than 2", off * 100.0);
                }
            }
        }
    }

    /// Cropped, turned, flipped and straightened, and the stages without the layers.
    #[test]
    fn the_geometry_and_stages_draw_as_on_the_cpu() {
        let Some((device, queue)) = device() else { return };
        let mut stack = Stack::new(&device, &queue);
        let src = picture(96, 64, false);
        let (_t, view) = upload(&device, &queue, &src);
        let mut edit = Edit::default();
        let l = edit.layer(Effect::Light(Light { exposure: 0.4, ..Default::default() }));
        edit.layers.push(l);
        let cases = [
            (0u8, false, 0.0f32, [0.0f32, 0.0f32], Crop::FULL),
            (1, true, 0.0, [0.0, 0.0], Crop { x: 0.1, y: 0.2, w: 0.6, h: 0.5 }),
            (2, false, 7.5, [0.0, 0.0], Crop::FULL),
            (3, true, -12.0, [0.0, 0.0], Crop { x: 0.3, y: 0.1, w: 0.5, h: 0.7 }),
            // Perspective: a homography the frame's corners give.
            (0, false, 4.0, [14.0, -9.0], Crop { x: 0.1, y: 0.1, w: 0.8, h: 0.8 }),
            (1, false, 0.0, [-20.0, 0.0], Crop::FULL),
        ];
        for (turns, flip, straighten, [vertical, horizontal], crop) in cases {
            edit.geometry.turns = turns;
            edit.geometry.flip_h = flip;
            edit.geometry.straighten = straighten;
            edit.geometry.vertical = vertical;
            edit.geometry.horizontal = horizontal;
            edit.geometry.crop = crop;
            for stage in [Stage::Full, Stage::Plain, Stage::Uncropped] {
                let size = render::fit_size(render::full_size(&edit, (96, 64), stage), 80, 80);
                let (worst, mean, off) = apart(&gpu(&mut stack, &view, &edit, &src, stage, size), &cpu(&edit, &src, stage, size));
                assert!(worst <= 8 && mean <= 0.3 && off <= 0.004, "turns {turns}, straighten {straighten}, tilt {vertical} {horizontal}, {stage:?}: worst {worst}, mean {mean:.3}, off {off}");
            }
        }
    }

    /// A slider dragged with the source still: the layer and those over it redone from the picture kept before it,
    /// drawing what a fresh run draws; nothing done when nothing changed.
    #[test]
    fn a_live_stack_redoes_only_what_changed() {
        let Some((device, queue)) = device() else { return };
        let src = picture(64, 48, false);
        let (_t, view) = upload(&device, &queue, &src);
        let mut edit = Edit::default();
        for k in Kind::ALL {
            let l = edit.layer(k.effect());
            edit.layers.push(l);
        }
        let mut live = Stack::live(&device, &queue);
        let size = (64, 48);
        assert!(live.run(&edit, &view, (64, 48), (64, 48), Stage::Full, size, true));
        assert!(!live.run(&edit, &view, (64, 48), (64, 48), Stage::Full, size, false), "nothing changed");
        for amount in [0.2f32, -0.4, 0.9] {
            if let Effect::Vignette(v) = &mut edit.layers[3].effect {
                v.amount = amount;
            }
            assert!(live.run(&edit, &view, (64, 48), (64, 48), Stage::Full, size, false));
            assert_eq!(live.kept.map(|k| k.1), Some(3), "the picture before the vignette kept");
            let again = live.read().unwrap();
            let mut fresh = Stack::new(&device, &queue);
            fresh.run(&edit, &view, (64, 48), (64, 48), Stage::Full, size, true);
            assert_eq!(again, fresh.read().unwrap());
        }
        // A new source draws everything again.
        assert!(live.run(&edit, &view, (64, 48), (64, 48), Stage::Full, size, true));
    }

    #[test]
    fn the_square_maps_to_its_quad() {
        let q = [[0.1, 0.2], [0.9, 0.1], [0.2, 0.8], [1.0, 1.1]];
        let m = square_to_quad(q);
        for (i, (u, v)) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].into_iter().enumerate() {
            let w = m[2][0] * u + m[2][1] * v + m[2][2];
            let p = [(m[0][0] * u + m[0][1] * v + m[0][2]) / w, (m[1][0] * u + m[1][1] * v + m[1][2]) / w];
            assert!((p[0] - q[i][0] as f32).abs() < 1e-5 && (p[1] - q[i][1] as f32).abs() < 1e-5, "{i}: {p:?}");
        }
    }

    /// `cargo test -p orbiters-ui photo::gpu::tests::a_view_sized -- --ignored --nocapture`: each kind of layer on a
    /// 1600×900 picture (about the 3D view's canvas), and all of them, as the GPU takes them.
    #[test]
    #[ignore]
    fn a_view_sized_picture_through_every_kind_of_layer() {
        let Some((device, queue)) = device() else { return };
        let (w, h) = (1600u32, 900u32);
        let src = picture(w, h, false);
        let (_t, view) = upload(&device, &queue, &src);
        let mut stack = Stack::new(&device, &queue);
        let mut all = Edit::default();
        let mut effects: Vec<Effect> = Kind::ALL.iter().map(|k| k.effect()).collect();
        effects.push(Effect::Blur(Blur { kind: BlurKind::Lens, radius: 0.04, bokeh: 0.8, ..Default::default() }));
        effects.push(Effect::Blur(Blur { kind: BlurKind::Directional, radius: 0.1, ..Default::default() }));
        effects.push(Effect::Blur(Blur { kind: BlurKind::Gaussian, radius: 0.05, ..Default::default() }));
        effects.push(Effect::Comic(Comic { dots: true, ..Default::default() }));
        let time = |stack: &mut Stack, edit: &Edit| {
            stack.run(edit, &view, (w, h), (w, h), Stage::Full, (w, h), true);
            stack.read().unwrap();
            let start = std::time::Instant::now();
            for _ in 0..5 {
                stack.run(edit, &view, (w, h), (w, h), Stage::Full, (w, h), true);
            }
            stack.read().unwrap();
            start.elapsed().as_secs_f64() * 1000.0 / 5.0
        };
        for e in effects {
            let mut one = Edit::default();
            let l = one.layer(e);
            let name = l.effect.title();
            one.layers.push(l.clone());
            eprintln!("{name:>22}: {:.2} ms", time(&mut stack, &one));
            all.layers.push(l);
        }
        eprintln!("    all of them at {w}x{h}: {:.2} ms", time(&mut stack, &all));
    }
}
