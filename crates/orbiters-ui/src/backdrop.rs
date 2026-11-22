//! A real blur of everything behind a panel that opens over a page (REQUEST21 3, the chat that grows out of the title
//! bar): the page stays recognisable behind it, blurred and dimmed more the further the panel has opened, and more near
//! the panel than far from it.
//!
//! egui cannot read back what it drew, so (as [`crate::blur`] does under bars) the shapes of the page's layer are drawn
//! a second time offscreen, with the window's own textures, at half the window's density. That copy goes through a
//! dual-filter (Kawase) pyramid: six levels down, each half the size of the one before, then up again from each level,
//! which gives six blurs, each twice as wide as the one before, for a handful of tiny passes. The composite reads, for
//! each pixel, the two blurs around the strength it wants there (the opening's progress, eased towards a floor away from
//! the panel), darkens it, and lays it over the page, which shows through where the wanted blur is almost none.
//!
//! A page that draws the same shapes as last frame is not drawn or blurred again: only the composite runs, with the new
//! strength (a morph over a still page costs one full-window pass a frame). GPU content of its own (a 3D view's paint
//! callback) cannot be drawn again: it is left out of the copy, and the page under it shows through there.

use crate::blur::{atlas_changing, sampler, Fingerprint, Textures, OFFSCREEN};
use egui::emath::TSTransform;
use egui::epaint::textures::TextureOptions;
use egui::epaint::{ClippedPrimitive, Primitive, TextureId};
use egui::layers::ShapeIdx;
use egui::{pos2, Id, LayerId, Painter, Rect, Shape};
use egui_wgpu::wgpu;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// How the page behind looks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// How far the panel has opened, 0 (nothing: the page as it is) to 1.
    pub strength: f32,
    /// The strongest blur (a gaussian's sigma, in points), reached next to the panel once it is open.
    pub blur: f32,
    /// How much darker the page gets at full strength next to the panel (0 to 1).
    pub dim: f32,
    /// The panel, in window points, and its corner radius.
    pub focus: Rect,
    pub radius: f32,
    /// Points from the panel's edge over which the strength eases down to [`Look::far`] of itself.
    pub reach: f32,
    /// The share of the strength kept far from the panel (0 to 1).
    pub far: f32,
}

/// The copy is drawn at this fraction of the window's density: it is about to be blurred.
const DENSITY: f32 = 0.5;

/// Levels of the pyramid under the copy.
const LEVELS: usize = 6;

/// The blur (sigma, in pixels of the copy) of the first level's way up; each level after doubles it.
const SIGMA1: f32 = 1.5;

/// Pixels of the copy's blur under which the page itself shows through, fading the blur in.
const RAMP: f32 = 1.2;

/// Paints, under what `painter` paints next, a blurred and dimmed copy of the shapes `from..` of the layer `source`
/// that reach into `area`. `id` names the GPU objects of this backdrop: keep it stable. Nothing is drawn while the
/// strength is about nothing.
#[allow(clippy::too_many_arguments)]
pub fn behind(painter: &Painter, id: Id, textures: &Textures, source: LayerId, from: ShapeIdx, area: Rect, look: Look) {
    let ctx = painter.ctx();
    let ppp = ctx.pixels_per_point();
    let snap = |p: egui::Pos2| pos2((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp);
    let area = Rect::from_min_max(snap(area.min), snap(area.max));
    if area.width() < 1.0 || area.height() < 1.0 || look.strength <= 0.002 {
        return;
    }
    let low = ppp * DENSITY;
    let size = [((area.width() * low).ceil() as u32).max(1), ((area.height() * low).ceil() as u32).max(1)];
    let full = [(area.width() * ppp).round(), (area.height() * ppp).round()];
    let composite = Composite::of(&look, area, ppp, full);
    let pass = ctx.cumulative_pass_nr();
    // Text laid out this frame points into a font atlas the renderer gets only after it: last frame's copy is painted.
    if atlas_changing(ctx, textures) {
        painter.add(egui_wgpu::Callback::new_paint_callback(area, BackdropCallback { id, content: None, size, composite, pass }));
        return;
    }
    let to_area = TSTransform::from_translation(-area.min.to_vec2());
    let mut copies = Vec::new();
    ctx.graphics(|g| {
        let Some(list) = g.get(source) else { return };
        for (i, s) in list.all_entries().enumerate() {
            if i < from.0 || matches!(s.shape, Shape::Callback(_)) {
                continue;
            }
            if !s.shape.visual_bounding_rect().intersect(s.clip_rect).intersects(area) {
                continue;
            }
            let mut copy = s.clone();
            copy.clip_rect = copy.clip_rect.intersect(area);
            copy.transform(to_area);
            copies.push(copy);
        }
    });
    let primitives = ctx.tessellate(copies, low);
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
    let fingerprint = {
        let mut h = Fingerprint::default();
        (size, low.to_bits()).hash(&mut h);
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
    let content = Some(Content { primitives, textures: used, ppp: low, fingerprint });
    painter.add(egui_wgpu::Callback::new_paint_callback(area, BackdropCallback { id, content, size, composite, pass }));
}

/// The composite's uniforms (see the shader's `C`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Composite {
    /// Area in window pixels (w, h); the strongest blur in pixels of the copy; strength.
    size: [f32; 4],
    /// The panel in area pixels: centre, half size.
    focus: [f32; 4],
    /// Its corner radius and the reach (window pixels), the far share, the dim.
    shape: [f32; 4],
    /// Linear target (set at prepare), the copy's density, the ramp, unused.
    extra: [f32; 4],
}

impl Composite {
    fn of(look: &Look, area: Rect, ppp: f32, full: [f32; 2]) -> Self {
        let focus = look.focus.translate(-area.min.to_vec2());
        Self {
            size: [full[0], full[1], look.blur * ppp * DENSITY, look.strength.clamp(0.0, 1.0)],
            focus: [focus.center().x * ppp, focus.center().y * ppp, focus.width() * 0.5 * ppp, focus.height() * 0.5 * ppp],
            shape: [look.radius * ppp, look.reach.max(1.0) * ppp, look.far.clamp(0.0, 1.0), look.dim.clamp(0.0, 1.0)],
            extra: [0.0, DENSITY, RAMP, 0.0],
        }
    }
}

/// The page's copy for this frame.
struct Content {
    /// Tessellated in area coordinates at the copy's density.
    primitives: Vec<ClippedPrimitive>,
    textures: Vec<(TextureId, wgpu::Texture, TextureOptions)>,
    ppp: f32,
    fingerprint: u64,
}

struct BackdropCallback {
    id: Id,
    /// `None`: paint last frame's copy (the font atlas is being replaced).
    content: Option<Content>,
    /// The copy's size in pixels.
    size: [u32; 2],
    composite: Composite,
    pass: u64,
}

const SHADER: &str = r#"
// A pass reads `size.xy` as one texel of its source (1 / width, 1 / height); the composite reads all of it.
struct C {
    size: vec4<f32>,
    focus: vec4<f32>,
    shape: vec4<f32>,
    extra: vec4<f32>,
};
@group(0) @binding(0) var<uniform> c: C;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var src: texture_2d<f32>;
@group(0) @binding(3) var b1: texture_2d<f32>;
@group(0) @binding(4) var b2: texture_2d<f32>;
@group(0) @binding(5) var b3: texture_2d<f32>;
@group(0) @binding(6) var b4: texture_2d<f32>;
@group(0) @binding(7) var b5: texture_2d<f32>;
@group(0) @binding(8) var b6: texture_2d<f32>;

struct V {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> V {
    let q = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: V;
    o.pos = vec4<f32>(q.x * 2.0 - 1.0, 1.0 - q.y * 2.0, 0.0, 1.0);
    o.uv = q;
    return o;
}

fn tap(uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(src, samp, uv, 0.0);
}

// Dual filter, down: the centre four times and four corners half a texel of the source away.
@fragment
fn fs_down(in: V) -> @location(0) vec4<f32> {
    let h = c.size.xy * 0.5;
    var sum = tap(in.uv) * 4.0;
    sum += tap(in.uv - h);
    sum += tap(in.uv + h);
    sum += tap(in.uv + vec2<f32>(h.x, -h.y));
    sum += tap(in.uv - vec2<f32>(h.x, -h.y));
    return sum / 8.0;
}

// Dual filter, up: a tent of eight taps around the pixel, the source's texel apart.
@fragment
fn fs_up(in: V) -> @location(0) vec4<f32> {
    let h = c.size.xy * 0.5;
    var sum = tap(in.uv + vec2<f32>(-h.x * 2.0, 0.0));
    sum += tap(in.uv + vec2<f32>(-h.x, h.y)) * 2.0;
    sum += tap(in.uv + vec2<f32>(0.0, h.y * 2.0));
    sum += tap(in.uv + vec2<f32>(h.x, h.y)) * 2.0;
    sum += tap(in.uv + vec2<f32>(h.x * 2.0, 0.0));
    sum += tap(in.uv + vec2<f32>(h.x, -h.y)) * 2.0;
    sum += tap(in.uv + vec2<f32>(0.0, -h.y * 2.0));
    sum += tap(in.uv + vec2<f32>(-h.x, -h.y)) * 2.0;
    return sum / 12.0;
}

fn srgb_to_linear(x: vec3<f32>) -> vec3<f32> {
    let lo = x / 12.92;
    let hi = pow((x + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, x <= vec3<f32>(0.04045));
}

fn rounded_rect(q: vec2<f32>, centre: vec2<f32>, half: vec2<f32>, r: f32) -> f32 {
    let d = abs(q - centre) - half + vec2<f32>(r);
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

fn level(k: i32, uv: vec2<f32>) -> vec4<f32> {
    switch k {
        case 0: { return textureSampleLevel(src, samp, uv, 0.0); }
        case 1: { return textureSampleLevel(b1, samp, uv, 0.0); }
        case 2: { return textureSampleLevel(b2, samp, uv, 0.0); }
        case 3: { return textureSampleLevel(b3, samp, uv, 0.0); }
        case 4: { return textureSampleLevel(b4, samp, uv, 0.0); }
        case 5: { return textureSampleLevel(b5, samp, uv, 0.0); }
        default: { return textureSampleLevel(b6, samp, uv, 0.0); }
    }
}

// The copy over the page (premultiplied): as blurred as the pixel wants (strongest next to the panel), darker, and
// fading out where the blur wanted is about none.
@fragment
fn fs_composite(in: V) -> @location(0) vec4<f32> {
    let px = in.uv * c.size.xy;
    let d = rounded_rect(px, c.focus.xy, c.focus.zw, c.shape.x);
    let near = 1.0 - smoothstep(0.0, c.shape.y, max(d, 0.0));
    let s = c.size.w * mix(c.shape.z, 1.0, near);
    let sigma = s * c.size.z;
    // Level k (from 1) blurs SIGMA1 * 2^(k-1).
    let k = log2(max(sigma, 1e-4) / SIGMA1) + 1.0;
    var col: vec4<f32>;
    if (k <= 1.0) {
        col = mix(level(0, in.uv), level(1, in.uv), clamp(sigma / SIGMA1, 0.0, 1.0));
    } else if (k >= 6.0) {
        col = level(6, in.uv);
    } else {
        let lo = i32(floor(k));
        col = mix(level(lo, in.uv), level(lo + 1, in.uv), k - f32(lo));
    }
    let a = clamp(sigma / c.extra.z, 0.0, 1.0);
    col = vec4<f32>(col.rgb * (1.0 - c.shape.w * s), col.a);
    if (c.extra.x > 0.5 && col.a > 0.0) {
        col = vec4<f32>(srgb_to_linear(col.rgb / col.a) * col.a, col.a);
    }
    return col * a;
}
"#;

/// The shader, with the first level's blur written in.
fn shader() -> String {
    SHADER.replace("SIGMA1", &format!("{SIGMA1:.4}"))
}

/// GPU objects of the backdrops, kept in egui-wgpu's callback resources.
pub struct BackdropResources {
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    pass_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    linear_target: bool,
    backdrops: HashMap<Id, Backdrop>,
}

/// One backdrop's renderer (for the copy), its textures and passes.
struct Backdrop {
    renderer: egui_wgpu::Renderer,
    /// The window renderer's textures as this renderer knows them.
    mirrored: HashMap<TextureId, (wgpu::Texture, TextureId)>,
    size: [u32; 2],
    targets: Option<Targets>,
    composite_uniforms: wgpu::Buffer,
    shows: Option<u64>,
    pass: u64,
}

/// The ways up: from level k (1 to 6) to level max(k - 2, 0), the last one being blur k.
fn chain(k: usize) -> std::ops::RangeInclusive<usize> {
    k.saturating_sub(2)..=k - 1
}

struct Targets {
    content: wgpu::TextureView,
    /// Each pass: its target and bind group, in the order they run (the downs, then each way up).
    passes: Vec<(wgpu::TextureView, wgpu::BindGroup, bool)>,
    composite: wgpu::BindGroup,
}

/// A backdrop not painted for this many passes gives its textures back.
const FORGET_AFTER: u64 = 120;

/// Creates the pipelines for render targets of `format` (call once per renderer).
pub fn install(device: &wgpu::Device, format: wgpu::TextureFormat, msaa_samples: u32, resources: &mut egui_wgpu::CallbackResources) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("orbiters backdrop"), source: wgpu::ShaderSource::Wgsl(shader().into()) }); // i18n: not shown (names a graphics debugger shows)
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
    let pass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("orbiters backdrop pass"), entries: &common }); // i18n: not shown (names a graphics debugger shows)
    let mut entries = common.to_vec();
    entries.extend((0..LEVELS as u32).map(|i| texture(3 + i)));
    let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("orbiters backdrop composite"), entries: &entries }); // i18n: not shown (names a graphics debugger shows)
    let pipeline = |layout: &wgpu::BindGroupLayout, entry: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>, samples: u32| {
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("orbiters backdrop"), // i18n: not shown (names a graphics debugger shows)
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("orbiters backdrop"), // i18n: not shown (names a graphics debugger shows)
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
    let down = pipeline(&pass_layout, "fs_down", OFFSCREEN, None, 1);
    let up = pipeline(&pass_layout, "fs_up", OFFSCREEN, None, 1);
    let composite = pipeline(&composite_layout, "fs_composite", format, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), msaa_samples.max(1));
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("orbiters backdrop"), // i18n: not shown (names a graphics debugger shows)
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    resources.insert(BackdropResources {
        down,
        up,
        composite,
        pass_layout,
        composite_layout,
        sampler,
        linear_target: format.is_srgb(),
        backdrops: HashMap::new(),
    });
}

impl Backdrop {
    fn new(device: &wgpu::Device) -> Self {
        let renderer = egui_wgpu::Renderer::new(device, OFFSCREEN, egui_wgpu::RendererOptions { msaa_samples: 1, dithering: true, ..Default::default() });
        let composite_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("orbiters backdrop composite"), // i18n: not shown (names a graphics debugger shows)
            size: std::mem::size_of::<Composite>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { renderer, mirrored: HashMap::new(), size: [0, 0], targets: None, composite_uniforms, shows: None, pass: 0 }
    }

    /// Knows exactly this frame's textures; true when that changed anything.
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

    fn release(&mut self) {
        for (_, (_, mine)) in self.mirrored.drain() {
            self.renderer.free_texture(&mine);
        }
        self.targets = None;
        self.shows = None;
        self.size = [0, 0];
    }

    fn resize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, res: &BackdropResources, size: [u32; 2]) {
        if self.size == size && self.targets.is_some() {
            return;
        }
        self.size = size;
        self.shows = None;
        let mut sizes = vec![size];
        for _ in 0..LEVELS {
            let [w, h] = *sizes.last().unwrap_or(&size);
            sizes.push([w.div_ceil(2).max(1), h.div_ceil(2).max(1)]);
        }
        let view = |[w, h]: [u32; 2]| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("orbiters backdrop"), // i18n: not shown (names a graphics debugger shows)
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: OFFSCREEN,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let uniforms = |texel_of: [u32; 2]| {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("orbiters backdrop pass"), // i18n: not shown (names a graphics debugger shows)
                size: std::mem::size_of::<Composite>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut texel = [0.0f32; 16];
            texel[..2].copy_from_slice(&[1.0 / texel_of[0] as f32, 1.0 / texel_of[1] as f32]);
            queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&texel));
            buffer
        };
        let group = |layout: &wgpu::BindGroupLayout, uniforms: &wgpu::Buffer, textures: &[&wgpu::TextureView]| {
            let mut entries = vec![
                wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&res.sampler) },
            ];
            entries.extend(
                textures.iter().enumerate().map(|(i, t)| wgpu::BindGroupEntry { binding: 2 + i as u32, resource: wgpu::BindingResource::TextureView(t) }),
            );
            device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("orbiters backdrop"), layout, entries: &entries }) // i18n: not shown (names a graphics debugger shows)
        };
        let content = view(size);
        // Down: level i from level i - 1 (level 0 is the copy).
        let downs: Vec<wgpu::TextureView> = (1..=LEVELS).map(|i| view(sizes[i])).collect();
        let mut passes = Vec::new();
        for i in 1..=LEVELS {
            let source = if i == 1 { &content } else { &downs[i - 2] };
            let bind = group(&res.pass_layout, &uniforms(sizes[i - 1]), &[source]);
            passes.push((downs[i - 1].clone(), bind, true));
        }
        // Up from each level, to two levels above it (the first two to the copy's own size).
        let mut blurs = Vec::new();
        for k in 1..=LEVELS {
            let mut from = downs[k - 1].clone();
            let mut from_size = sizes[k];
            for level in chain(k).rev() {
                let target = view(sizes[level]);
                let bind = group(&res.pass_layout, &uniforms(from_size), &[&from]);
                passes.push((target.clone(), bind, false));
                from = target;
                from_size = sizes[level];
            }
            blurs.push(from);
        }
        let mut inputs: Vec<&wgpu::TextureView> = vec![&content];
        inputs.extend(blurs.iter());
        let composite = group(&res.composite_layout, &self.composite_uniforms, &inputs);
        self.targets = Some(Targets { content, passes, composite });
    }
}

/// Lets go of the textures of backdrops no longer painted.
fn forget_old(res: &mut BackdropResources, pass: u64) {
    res.backdrops.retain(|_, b| {
        let keep = b.pass + FORGET_AFTER >= pass;
        if !keep {
            b.release();
        }
        keep
    });
}

fn begin(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) -> wgpu::RenderPass<'static> {
    encoder
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("orbiters backdrop"), // i18n: not shown (names a graphics debugger shows)
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

impl egui_wgpu::CallbackTrait for BackdropCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(res) = resources.get_mut::<BackdropResources>() else { return Vec::new() };
        forget_old(res, self.pass);
        let mut backdrop = res.backdrops.remove(&self.id).unwrap_or_else(|| Backdrop::new(device));
        backdrop.pass = self.pass;
        let composite = Composite { extra: [if res.linear_target { 1.0 } else { 0.0 }, self.composite.extra[1], self.composite.extra[2], 0.0], ..self.composite };
        queue.write_buffer(&backdrop.composite_uniforms, 0, bytemuck::bytes_of(&composite));
        let Some(content) = &self.content else {
            res.backdrops.insert(self.id, backdrop);
            return Vec::new();
        };
        // An area wider than a texture can be is drawn at a lower density instead of failing validation.
        let limit = device.limits().max_texture_dimension_2d.max(1) as f32;
        let k = (limit / self.size[0].max(1) as f32).min(limit / self.size[1].max(1) as f32).min(1.0);
        let size = self.size.map(|v| ((v as f32 * k).floor() as u32).max(1));
        let textures_changed = backdrop.mirror(device, &content.textures);
        backdrop.resize(device, queue, res, size);
        if !textures_changed && backdrop.shows == Some(content.fingerprint) && backdrop.targets.is_some() {
            res.backdrops.insert(self.id, backdrop);
            return Vec::new();
        }
        let primitives: Vec<ClippedPrimitive> = content
            .primitives
            .iter()
            .filter_map(|p| match &p.primitive {
                Primitive::Mesh(m) => backdrop.mirrored.get(&m.texture_id).map(|(_, mine)| {
                    let mut m = m.clone();
                    m.texture_id = *mine;
                    ClippedPrimitive { clip_rect: p.clip_rect, primitive: Primitive::Mesh(m) }
                }),
                Primitive::Callback(_) => None,
            })
            .collect();
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: content.ppp * k };
        let commands = backdrop.renderer.update_buffers(device, queue, encoder, &primitives, &screen);
        if let Some(t) = &backdrop.targets {
            backdrop.renderer.render(&mut begin(encoder, &t.content), &primitives, &screen);
            for (target, bind, down) in &t.passes {
                let mut pass = begin(encoder, target);
                pass.set_pipeline(if *down { &res.down } else { &res.up });
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..3, 0..1);
            }
            backdrop.shows = Some(content.fingerprint);
        }
        res.backdrops.insert(self.id, backdrop);
        commands
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let Some(res) = resources.get::<BackdropResources>() else { return };
        let Some(t) = res.backdrops.get(&self.id).and_then(|b| b.targets.as_ref()) else { return };
        pass.set_pipeline(&res.composite);
        pass.set_bind_group(0, &t.composite, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shader_parses_with_its_constants_written_in() {
        let module = naga::front::wgsl::parse_str(&shader()).expect("the backdrop's shader parses");
        let mut validator = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty());
        validator.validate(&module).expect("the backdrop's shader is valid");
    }

    #[test]
    fn each_blur_comes_up_two_levels_at_most_and_the_first_two_reach_the_copy() {
        assert_eq!(chain(1), 0..=0);
        assert_eq!(chain(2), 0..=1);
        for k in 3..=LEVELS {
            assert_eq!(chain(k), k - 2..=k - 1);
        }
    }

    #[test]
    fn the_composite_is_told_the_panel_in_pixels_of_the_area() {
        let look = Look { strength: 2.0, blur: 24.0, dim: 0.4, focus: Rect::from_min_size(pos2(300.0, 40.0), egui::vec2(600.0, 500.0)), radius: 20.0, reach: 300.0, far: 0.5 };
        let area = Rect::from_min_size(pos2(0.0, 40.0), egui::vec2(1280.0, 780.0));
        let c = Composite::of(&look, area, 2.0, [2560.0, 1560.0]);
        assert_eq!(c.size, [2560.0, 1560.0, 24.0, 1.0], "strength clamped, blur in pixels of the half-density copy");
        assert_eq!(c.focus, [1200.0, 500.0, 600.0, 500.0]);
        assert_eq!(c.shape, [40.0, 600.0, 0.5, 0.4]);
    }
}
