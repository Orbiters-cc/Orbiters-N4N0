//! Progressive blur where scrolled content passes under a bar (the title bar, a sticky header), like the website's
//! navigation bar with its stacked backdrop filters. egui cannot read back what it drew, so the shapes reaching
//! into the band are drawn a second time offscreen, with the window's own textures, then blurred more the nearer
//! they are to the band's top edge, and painted in place of the sharp ones. A band whose content, size and look
//! are what they were last frame is not blurred again: its last result is painted.

use egui::epaint::emath::TSTransform;
use egui::epaint::textures::{TextureFilter, TextureOptions, TextureWrapMode};
use egui::epaint::{ClippedPrimitive, Primitive, TextureId};
use egui::layers::ShapeIdx;
use egui::{pos2, Id, Rect, Shape, Ui};
use egui_wgpu::wgpu;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// How a band looks.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// Blur (gaussian sigma, points) at the band's top edge, easing out to none at its bottom edge.
    pub blur: f32,
    /// Points from the top edge that keep the full blur, darkening and fading before the ease out.
    pub solid: f32,
    /// How much darker content gets at the top edge (0 to 1), easing out the same way.
    pub darken: f32,
    /// How much of it fades away at the top edge (0 to 1): 1 under a sticky header, so that nothing is cut.
    pub fade: f32,
}

/// The renderer that holds the textures the shapes use: the window's, or an offscreen one.
pub type Textures = Arc<egui::mutex::RwLock<egui_wgpu::Renderer>>;

/// Points of content below the band that are drawn offscreen too, for the blur near the bottom edge to reach.
const MARGIN: f32 = 8.0;

/// The index the next shape of `ui`'s layer gets: where the content to blur starts.
pub fn next_shape(ui: &Ui) -> ShapeIdx {
    ui.ctx().graphics_mut(|g| g.entry(ui.layer_id()).next_idx())
}

/// Takes the shapes `from..` of `ui`'s layer out of `band` (their clip ends at its bottom edge) and paints a
/// progressively blurred copy of what they drew there instead. Call it after the content and before what sits on
/// the band (the bar's text and buttons). `id` names the band's GPU objects: keep it stable, and give each band
/// drawn in a frame its own (two sharing one would blur with each other's targets).
pub fn under(ui: &Ui, id: Id, textures: &Textures, from: ShapeIdx, band: Rect, look: Look) {
    let ctx = ui.ctx();
    let ppp = ctx.pixels_per_point();
    // On whole pixels, so the copy rasterizes exactly as the sharp content it continues.
    let snap = |p: egui::Pos2| pos2((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp);
    let band = Rect::from_min_max(snap(band.min), snap(band.max));
    if band.width() < 1.0 || band.height() < 1.0 {
        return;
    }
    if atlas_changing(ctx, textures) {
        // Text laid out this frame points into a font atlas the renderer gets only after this frame: blurred with
        // the one it has, it would come out garbled. The content stays sharp for this one frame.
        return;
    }
    let reach = Rect::from_min_max(band.min, pos2(band.max.x, band.max.y + MARGIN));
    let to_band = TSTransform::from_translation(-band.min.to_vec2());
    let mut copies = Vec::new();
    ctx.graphics_mut(|g| {
        let list = g.entry(ui.layer_id());
        for i in from.0..list.next_idx().0 {
            list.mutate_shape(ShapeIdx(i), |s| {
                if !s.shape.visual_bounding_rect().intersect(s.clip_rect).intersects(band) {
                    return;
                }
                // GPU content of its own (a 3D view) is not drawn again: it ends at the band.
                if !matches!(s.shape, Shape::Callback(_)) {
                    let mut copy = s.clone();
                    copy.clip_rect = copy.clip_rect.intersect(reach);
                    copy.transform(to_band);
                    copies.push(copy);
                }
                s.clip_rect.min.y = s.clip_rect.min.y.max(band.max.y);
            });
        }
    });
    if copies.is_empty() {
        return;
    }
    let primitives = ctx.tessellate(copies, ppp);
    let mut used: Vec<(TextureId, wgpu::Texture, TextureOptions)> = Vec::new();
    {
        let renderer = textures.read();
        for p in &primitives {
            if let Primitive::Mesh(m) = &p.primitive {
                if used.iter().any(|(id, _, _)| *id == m.texture_id) {
                    continue;
                }
                if let Some(egui_wgpu::Texture { texture: Some(t), options: Some(o), .. }) = renderer.texture(&m.texture_id) {
                    used.push((m.texture_id, t.clone(), *o));
                }
            }
        }
    }
    let size = [(band.width() * ppp).round() as u32, (reach.height() * ppp).round() as u32];
    let band_px = (band.height() * ppp).round();
    let fingerprint = {
        let mut h = Fingerprint::default();
        (size, band_px.to_bits(), ppp.to_bits(), [look.blur, look.solid, look.darken, look.fade].map(f32::to_bits)).hash(&mut h);
        for p in &primitives {
            let r = p.clip_rect;
            [r.min.x, r.min.y, r.max.x, r.max.y].map(f32::to_bits).hash(&mut h);
            if let Primitive::Mesh(m) = &p.primitive {
                m.texture_id.hash(&mut h);
                h.write(bytemuck::cast_slice(&m.indices));
                h.write(bytemuck::cast_slice(&m.vertices));
            }
        }
        h.finish()
    };
    let pass = ctx.cumulative_pass_nr();
    let callback = BlurCallback { id, primitives, textures: used, size, band_px, ppp, look, fingerprint, pass };
    debug_assert!(look.solid < band.height(), "the plateau must end inside the band");
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(band, callback));
}

/// Whether the font atlas is being replaced this frame (it grew, or egui cleared it once full): the renderer's copy
/// is then not the one this frame's text was laid out in.
fn atlas_changing(ctx: &egui::Context, textures: &Textures) -> bool {
    #[derive(Clone, Copy)]
    struct Seen {
        pass: u64,
        fill: f32,
        cleared: bool,
    }
    let (size, fill) = ctx.fonts(|f| (f.font_image_size(), f.font_atlas_fill_ratio()));
    let uploaded = textures.read().texture(&TextureId::default()).and_then(|t| t.texture.as_ref().map(|t| [t.width() as usize, t.height() as usize]));
    let pass = ctx.cumulative_pass_nr();
    // Once per frame, for every band of it: a clear shows as the atlas filling less than it did.
    let cleared = ctx.data_mut(|d| {
        let seen = d.get_temp_mut_or_insert_with(Id::unique("orbiters-blur-atlas"), || Seen { pass, fill, cleared: false });
        if seen.pass != pass {
            *seen = Seen { pass, fill, cleared: fill < seen.fill };
        }
        seen.cleared
    });
    uploaded != Some(size) || cleared
}

/// What a band shows, summed up (a fast multiplicative hash of the copies' meshes and the band's size and look):
/// the same as last frame, last frame's blur is painted again.
#[derive(Default)]
struct Fingerprint(u64);

impl Fingerprint {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Hasher for Fingerprint {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for w in words {
            self.add(u64::from_le_bytes(*w));
        }
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(last) ^ ((rest.len() as u64) << 56));
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.add(v);
    }
}

const SHADER: &str = r#"
struct U {
    // Offscreen size (pixels), band height (pixels), plateau height (pixels).
    size: vec4<f32>,
    // Strongest blur (pixels), darken, fade, linear target (1).
    look: vec4<f32>,
    // This blur pass: direction (0 across, 1 down), gaussian sigma (pixels).
    pass_: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var samp: sampler;
// What a blur pass reads; the sharp content for the composite.
@group(0) @binding(2) var src: texture_2d<f32>;
// The composite's levels, each blurred twice as much as the one before, the last as much as the top edge wants.
@group(0) @binding(3) var level1: texture_2d<f32>;
@group(0) @binding(4) var level2: texture_2d<f32>;
@group(0) @binding(5) var level3: texture_2d<f32>;
@group(0) @binding(6) var level4: texture_2d<f32>;

struct V {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One triangle over the viewport, uv 0 to 1 from its top left corner.
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> V {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: V;
    o.pos = vec4<f32>(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    o.uv = p;
    return o;
}

// One direction of a gaussian.
@fragment
fn fs_blur(in: V) -> @location(0) vec4<f32> {
    // (No blur at all would divide nothing by nothing below.)
    let sigma = max(u.pass_.y, 1e-3);
    let dir = select(vec2<f32>(1.0 / u.size.x, 0.0), vec2<f32>(0.0, 1.0 / u.size.y), u.pass_.x > 0.5);
    let reach = 3.0 * sigma;
    // Bilinear taps at most a pixel and a half apart.
    let taps = clamp(ceil(reach / 1.5), 1.0, 48.0);
    let step = reach / taps;
    var sum = textureSampleLevel(src, samp, in.uv, 0.0);
    var weight = 1.0;
    for (var k = 1.0; k <= taps; k += 1.0) {
        let d = k * step;
        let w = exp(-0.5 * d * d / (sigma * sigma));
        sum += w * (textureSampleLevel(src, samp, in.uv + dir * d, 0.0) + textureSampleLevel(src, samp, in.uv - dir * d, 0.0));
        weight += 2.0 * w;
    }
    return sum / weight;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// The band over the window (premultiplied): each row as blurred, darker and fainter as its depth under the top edge
// asks (full through the plateau, easing out to nothing at the bottom edge), between the two nearest levels.
@fragment
fn fs_composite(in: V) -> @location(0) vec4<f32> {
    let uv = vec2<f32>(in.uv.x, in.uv.y * u.size.z / u.size.y);
    let depth = 1.0 - smoothstep(u.size.w, u.size.z, in.uv.y * u.size.z);
    let c0 = textureSampleLevel(src, samp, uv, 0.0);
    let c1 = textureSampleLevel(level1, samp, uv, 0.0);
    let c2 = textureSampleLevel(level2, samp, uv, 0.0);
    let c3 = textureSampleLevel(level3, samp, uv, 0.0);
    let c4 = textureSampleLevel(level4, samp, uv, 0.0);
    // Level k has an eighth of the strongest blur times 2^(k-1): the wanted blur sits at k = 4 + log2(depth).
    let k = 4.0 + log2(max(depth, 1e-6));
    var c: vec4<f32>;
    if (k <= 1.0) {
        c = mix(c0, c1, clamp(depth * 8.0, 0.0, 1.0));
    } else if (k <= 2.0) {
        c = mix(c1, c2, k - 1.0);
    } else if (k <= 3.0) {
        c = mix(c2, c3, k - 2.0);
    } else {
        c = mix(c3, c4, k - 3.0);
    }
    c = vec4<f32>(c.rgb * (1.0 - u.look.y * depth), c.a) * (1.0 - u.look.z * depth);
    if (u.look.w > 0.5 && c.a > 0.0) {
        c = vec4<f32>(srgb_to_linear(c.rgb / c.a) * c.a, c.a);
    }
    return c;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    size: [f32; 4],
    look: [f32; 4],
    pass: [f32; 4],
}

/// What the offscreen passes draw into: egui-wgpu's own texture format, in which its colours are gamma encoded.
const OFFSCREEN: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Blur levels, each twice the one before; a row blends the two around the blur it wants.
const LEVELS: usize = 4;

/// The sigma each level adds to the one before it (fractions of the strongest), so that level k (from 1) holds
/// 2^(k-1)/8 of it in all: gaussians add in squares.
fn level_steps() -> [f32; LEVELS] {
    let total = |k: usize| 2f32.powi(k as i32) / 16.0;
    std::array::from_fn(|i| if i == 0 { total(1) } else { (total(i + 1).powi(2) - total(i).powi(2)).sqrt() })
}

/// GPU objects of the blurred bands, kept in egui-wgpu's callback resources.
pub struct BlurResources {
    blur: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    blur_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    linear_target: bool,
    bands: HashMap<Id, Band>,
    /// Made with the pipelines (its renderer compiles one), for the first band, so its first frame does not hitch;
    /// later the renderer of a band no page draws any more, kept for the next one.
    spare: Option<Band>,
}

/// A band not drawn for this many passes gives its textures back (a page left).
const FORGET_AFTER: u64 = 120;

/// One band's renderer (its own buffers: two bands drawn in one frame must not share them) and textures.
struct Band {
    renderer: egui_wgpu::Renderer,
    /// The window renderer's textures as this renderer knows them.
    mirrored: HashMap<TextureId, (wgpu::Texture, TextureId)>,
    /// Two per level (across, down), then the composite's.
    uniforms: Vec<wgpu::Buffer>,
    size: [u32; 2],
    targets: Option<Targets>,
    /// What the targets hold now ([`Fingerprint`]).
    shows: Option<u64>,
    /// The pass it was last drawn in.
    pass: u64,
}

struct Targets {
    /// The sharp content, the across pass in between, the levels.
    content: wgpu::TextureView,
    across: wgpu::TextureView,
    levels: [wgpu::TextureView; LEVELS],
    /// Per level: across (reads the level before, or the content), down (reads the across pass).
    passes: Vec<wgpu::BindGroup>,
    composite: wgpu::BindGroup,
}

/// Creates the pipelines for render targets of `format` (call once per renderer).
pub fn install(device: &wgpu::Device, format: wgpu::TextureFormat, msaa_samples: u32, resources: &mut egui_wgpu::CallbackResources) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("orbiters blur"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let common = [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        texture(2),
    ];
    let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("orbiters blur pass"), entries: &common });
    let mut entries = common.to_vec();
    entries.extend((0..LEVELS as u32).map(|i| texture(3 + i)));
    let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("orbiters blur composite"), entries: &entries });
    let pipeline = |layout: &wgpu::BindGroupLayout, entry: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>, samples: u32| {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("orbiters blur"),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("orbiters blur"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(entry),
                targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    let blur = pipeline(&blur_layout, "fs_blur", OFFSCREEN, None, 1);
    let composite = pipeline(&composite_layout, "fs_composite", format, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), msaa_samples.max(1));
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("orbiters blur"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let spare = Some(Band::new(device));
    resources.insert(BlurResources { blur, composite, blur_layout, composite_layout, sampler, linear_target: format.is_srgb(), bands: HashMap::new(), spare });
}

impl Band {
    fn new(device: &wgpu::Device) -> Self {
        let renderer = egui_wgpu::Renderer::new(device, OFFSCREEN, egui_wgpu::RendererOptions { msaa_samples: 1, dithering: true, ..Default::default() });
        let uniforms = (0..2 * LEVELS + 1)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("orbiters blur uniforms"),
                    size: std::mem::size_of::<Uniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        Self { renderer, mirrored: HashMap::new(), uniforms, size: [0, 0], targets: None, shows: None, pass: 0 }
    }

    /// Knows exactly this frame's textures (a texture the window renderer replaced is registered again); true when
    /// that changed anything.
    fn mirror(&mut self, device: &wgpu::Device, textures: &[(TextureId, wgpu::Texture, TextureOptions)]) -> bool {
        let stale: Vec<TextureId> =
            self.mirrored.iter().filter(|(id, (t, _))| !textures.iter().any(|(i, u, _)| i == *id && u == t)).map(|(id, _)| *id).collect();
        let mut changed = !stale.is_empty();
        for id in stale {
            if let Some((_, mine)) = self.mirrored.remove(&id) {
                self.renderer.free_texture(&mine);
            }
        }
        for (id, texture, options) in textures {
            if self.mirrored.contains_key(id) {
                continue;
            }
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let mine = self.renderer.register_native_texture_with_sampler_options(device, &view, sampler(*options));
            self.mirrored.insert(*id, (texture.clone(), mine));
            changed = true;
        }
        changed
    }

    /// Gives its textures back, keeping the renderer (and its compiled pipeline) for another band.
    fn release(&mut self) {
        for (_, (_, mine)) in self.mirrored.drain() {
            self.renderer.free_texture(&mine);
        }
        self.targets = None;
        self.shows = None;
        self.size = [0, 0];
    }

    fn resize(&mut self, device: &wgpu::Device, res: &BlurResources, size: [u32; 2]) {
        if self.size == size && self.targets.is_some() {
            return;
        }
        self.size = size;
        self.shows = None;
        let view = || {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("orbiters blur"),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: OFFSCREEN,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let (content, across) = (view(), view());
        let levels: [wgpu::TextureView; LEVELS] = std::array::from_fn(|_| view());
        let group = |layout: &wgpu::BindGroupLayout, uniforms: &wgpu::Buffer, textures: &[&wgpu::TextureView]| {
            let mut entries = vec![
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&res.sampler) },
            ];
            entries.extend(
                textures.iter().enumerate().map(|(i, t)| wgpu::BindGroupEntry { binding: 2 + i as u32, resource: wgpu::BindingResource::TextureView(t) }),
            );
            device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("orbiters blur"), layout, entries: &entries })
        };
        let mut passes = Vec::with_capacity(2 * LEVELS);
        for k in 0..LEVELS {
            let source = if k == 0 { &content } else { &levels[k - 1] };
            passes.push(group(&res.blur_layout, &self.uniforms[2 * k], &[source]));
            passes.push(group(&res.blur_layout, &self.uniforms[2 * k + 1], &[&across]));
        }
        let composite = group(&res.composite_layout, &self.uniforms[2 * LEVELS], &[&content, &levels[0], &levels[1], &levels[2], &levels[3]]);
        self.targets = Some(Targets { content, across, levels, passes, composite });
    }
}

/// The sampler egui-wgpu makes for these options.
fn sampler(o: TextureOptions) -> wgpu::SamplerDescriptor<'static> {
    let filter = |f| match f {
        TextureFilter::Nearest => wgpu::FilterMode::Nearest,
        TextureFilter::Linear => wgpu::FilterMode::Linear,
    };
    let address = match o.wrap_mode {
        TextureWrapMode::ClampToEdge => wgpu::AddressMode::ClampToEdge,
        TextureWrapMode::Repeat => wgpu::AddressMode::Repeat,
        TextureWrapMode::MirroredRepeat => wgpu::AddressMode::MirrorRepeat,
    };
    wgpu::SamplerDescriptor {
        label: Some("orbiters blur mirror"),
        mag_filter: filter(o.magnification),
        min_filter: filter(o.minification),
        address_mode_u: address,
        address_mode_v: address,
        ..Default::default()
    }
}

struct BlurCallback {
    id: Id,
    /// The copies, tessellated in band coordinates.
    primitives: Vec<ClippedPrimitive>,
    textures: Vec<(TextureId, wgpu::Texture, TextureOptions)>,
    /// Offscreen size in pixels: the band and the margin below it.
    size: [u32; 2],
    band_px: f32,
    ppp: f32,
    look: Look,
    /// What the copies and the look sum up to ([`Fingerprint`]).
    fingerprint: u64,
    /// The pass it was made in.
    pass: u64,
}

/// Lets go of the textures of bands no longer drawn; one renderer is kept for the next band.
fn forget_old(res: &mut BlurResources, pass: u64) {
    let old: Vec<Id> = res.bands.iter().filter(|(_, b)| b.pass + FORGET_AFTER < pass).map(|(id, _)| *id).collect();
    for id in old {
        if let Some(mut band) = res.bands.remove(&id) {
            band.release();
            if res.spare.is_none() {
                res.spare = Some(band);
            }
        }
    }
}

fn begin(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) -> wgpu::RenderPass<'static> {
    encoder
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("orbiters blur"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        })
        .forget_lifetime()
}

impl egui_wgpu::CallbackTrait for BlurCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<BlurResources>() else { return Vec::new() };
        forget_old(res, self.pass);
        let mut band = res.bands.remove(&self.id).or_else(|| res.spare.take()).unwrap_or_else(|| Band::new(device));
        band.pass = self.pass;
        // A band wider or taller than a texture can be (a window across several monitors) is blurred at a lower
        // density instead of failing validation.
        let limit = device.limits().max_texture_dimension_2d.max(1) as f32;
        let k = (limit / self.size[0].max(1) as f32).min(limit / self.size[1].max(1) as f32).min(1.0);
        let size = self.size.map(|v| ((v as f32 * k).floor() as u32).max(1));
        let (ppp, band_px) = (self.ppp * k, (self.band_px * k).round());
        let textures_changed = band.mirror(device, &self.textures);
        band.resize(device, res, size);
        if !textures_changed && band.shows == Some(self.fingerprint) && band.targets.is_some() {
            // Nothing under the band changed: last frame's blur is still right.
            res.bands.insert(self.id, band);
            return Vec::new();
        }
        // The copies with this renderer's names for the textures; a mesh whose texture it lacks is left out.
        let primitives: Vec<ClippedPrimitive> = self
            .primitives
            .iter()
            .filter_map(|p| match &p.primitive {
                Primitive::Mesh(m) => band.mirrored.get(&m.texture_id).map(|(_, mine)| {
                    let mut m = m.clone();
                    m.texture_id = *mine;
                    ClippedPrimitive { clip_rect: p.clip_rect, primitive: Primitive::Mesh(m) }
                }),
                Primitive::Callback(_) => None,
            })
            .collect();
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: ppp };
        let commands = band.renderer.update_buffers(device, queue, encoder, &primitives, &screen);
        let strongest = self.look.blur * ppp;
        let [w, h] = size.map(|v| v as f32);
        let uniforms = |pass: [f32; 4]| Uniforms {
            size: [w, h, band_px, (self.look.solid * ppp).round()],
            look: [strongest, self.look.darken, self.look.fade, if res.linear_target { 1.0 } else { 0.0 }],
            pass,
        };
        for (k, step) in level_steps().into_iter().enumerate() {
            for direction in 0..2 {
                queue.write_buffer(&band.uniforms[2 * k + direction], 0, bytemuck::bytes_of(&uniforms([direction as f32, step * strongest, 0.0, 0.0])));
            }
        }
        queue.write_buffer(&band.uniforms[2 * LEVELS], 0, bytemuck::bytes_of(&uniforms([0.0; 4])));
        if let Some(t) = &band.targets {
            band.renderer.render(&mut begin(encoder, &t.content), &primitives, &screen);
            // Each level from the one before: across into the scratch texture, then down into the level.
            for k in 0..LEVELS {
                for (pass, target) in [(2 * k, &t.across), (2 * k + 1, &t.levels[k])] {
                    let mut p = begin(encoder, target);
                    p.set_pipeline(&res.blur);
                    p.set_bind_group(0, &t.passes[pass], &[]);
                    p.draw(0..3, 0..1);
                }
            }
            band.shows = Some(self.fingerprint);
        }
        res.bands.insert(self.id, band);
        commands
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let Some(res) = resources.get::<BlurResources>() else { return };
        let Some(t) = res.bands.get(&self.id).and_then(|b| b.targets.as_ref()) else { return };
        pass.set_pipeline(&res.composite);
        pass.set_bind_group(0, &t.composite, &[]);
        pass.draw(0..3, 0..1);
    }
}
