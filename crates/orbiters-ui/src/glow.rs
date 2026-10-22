//! The glow background: the website's three swirling colour blobs, slowed down for a desktop app and tinted by
//! what the app is doing, with a vignette and film grain, drawn by one fragment shader as an egui paint callback.
//! The blobs drift on [`crate::anim::clock`], so they stand still (and cost nothing) while the window is not live or
//! has not been used for a while.

use crate::anim::{self, ease, Spring};
use egui::{Color32, Id, Painter, Rect};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;

/// What the background tells at a glance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mood {
    Idle,
    Recording,
    Importing,
}

/// Blob colours (unmultiplied RGBA) per mood: the web's, fainter than the website's home (a violet corner on black).
fn palette(mood: Mood) -> [[u8; 4]; 3] {
    let a = |r: u8, g: u8, b: u8, alpha: f32| [r, g, b, (alpha * 0.28 * 255.0).round() as u8];
    match mood {
        Mood::Idle => [a(162, 89, 247, 0.55), a(79, 137, 255, 0.42), a(72, 230, 255, 0.22)],
        Mood::Recording => [a(255, 77, 109, 0.42), a(189, 101, 255, 0.45), a(255, 138, 92, 0.2)],
        Mood::Importing => [a(72, 230, 255, 0.4), a(116, 102, 255, 0.45), a(60, 242, 154, 0.18)],
    }
}

/// A blob's resting ellipse (centre and radii as fractions of the window), blur (points), drift keyframes
/// (points), scale keyframes and loop period (seconds).
struct Blob {
    center: [f32; 2],
    radius: [f32; 2],
    blur: f32,
    x: [f32; 3],
    y: [f32; 3],
    scale: [f32; 3],
    period: f64,
}

const BLOBS: [Blob; 3] = [
    Blob { center: [0.155, 0.13], radius: [0.275, 0.35], blur: 110.0, x: [0.0, 80.0, -30.0], y: [0.0, 40.0, 90.0], scale: [1.0, 1.12, 0.95], period: 22.0 },
    Blob { center: [0.86, 0.38], radius: [0.24, 0.30], blur: 120.0, x: [0.0, -70.0, 20.0], y: [0.0, 60.0, -20.0], scale: [1.0, 0.92, 1.1], period: 26.0 },
    Blob { center: [0.525, 0.975], radius: [0.225, 0.275], blur: 130.0, x: [0.0, 60.0, -60.0], y: [0.0, -40.0, 10.0], scale: [1.0, 1.15, 1.0], period: 30.0 },
];

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    /// Size in points, grain amount, grain seed.
    size: [f32; 4],
    /// Base colour (sRGB 0..1), vignette strength.
    base: [f32; 4],
    /// Centre (fraction) and radii (fraction) per blob.
    blob_pos: [[f32; 4]; 3],
    /// sRGB colour and alpha per blob.
    blob_color: [[f32; 4]; 3],
    /// Blur in points per blob; w = 1 when the target stores linear colour (sRGB format).
    blob_blur: [f32; 4],
}

const SHADER: &str = r#"
struct Glow {
    size: vec4<f32>,
    base: vec4<f32>,
    blob_pos: array<vec4<f32>, 3>,
    blob_color: array<vec4<f32>, 3>,
    blob_blur: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Glow;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var out: VsOut;
    out.pos = vec4<f32>(p[i], 0.0, 1.0);
    out.uv = vec2<f32>(p[i].x * 0.5 + 0.5, 0.5 - p[i].y * 0.5);
    return out;
}

// Complementary error function (Abramowitz-Stegun 7.1.26): a blurred edge's coverage.
fn erfc_approx(x: f32) -> f32 {
    let z = abs(x);
    let t = 1.0 / (1.0 + 0.3275911 * z);
    let y = t * (0.254829592 + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429)))) * exp(-z * z);
    return select(2.0 - y, y, x >= 0.0);
}

// How much of a Gaussian-blurred ellipse covers point p (points).
fn coverage(p: vec2<f32>, k: u32) -> f32 {
    let c = u.blob_pos[k].xy * u.size.xy;
    let r = max(u.blob_pos[k].zw * u.size.xy, vec2<f32>(1.0, 1.0));
    let d = p - c;
    let q = length(d / r);
    var dist = -min(r.x, r.y);
    if (q > 1e-4) {
        dist = (q - 1.0) * length(d) / q;
    }
    let s = max(u.blob_blur[k], 0.5);
    return 0.5 * erfc_approx(dist / (s * 1.41421356));
}

fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.uv * u.size.xy;
    var col = u.base.rgb;
    for (var k = 0u; k < 3u; k = k + 1u) {
        let a = u.blob_color[k].a * coverage(p, k);
        col = mix(col, u.blob_color[k].rgb, a);
    }
    // Vignette: transparent to 35 %, then towards the base colour at the corners.
    let q = (in.uv - vec2<f32>(0.5, 0.5)) * 2.0;
    let rr = length(q) / 1.41421356;
    col = mix(col, u.base.rgb, clamp((rr - 0.35) / 0.65, 0.0, 1.0) * u.base.w);
    // A faint dither, one value per physical pixel and centred on zero, so gradients never band and the black
    // stays black.
    let px = vec2<u32>(in.pos.xy);
    let n = f32(pcg(px.x + pcg(px.y + u32(u.size.w)))) / 4294967295.0;
    col = max(col + vec3<f32>((n - 0.5) * u.size.z), vec3<f32>(0.0));
    if (u.blob_blur.w > 0.5) {
        col = srgb_to_linear(col);
    }
    return vec4<f32>(col, 1.0);
}
"#;

/// GPU objects of the glow, kept in egui-wgpu's callback resources.
pub struct GlowResources {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    linear_target: bool,
}

/// Creates the pipeline for render targets of `format` (call once per renderer).
pub fn install(device: &wgpu::Device, format: wgpu::TextureFormat, msaa_samples: u32, resources: &mut egui_wgpu::CallbackResources) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("orbiters glow"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("orbiters glow uniforms"),
        contents: bytemuck::bytes_of(&Uniforms::zeroed_default()),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("orbiters glow"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        }],
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("orbiters glow"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("orbiters glow"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("orbiters glow"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState { count: msaa_samples.max(1), mask: !0, alpha_to_coverage_enabled: false },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    });
    resources.insert(GlowResources { pipeline, buffer, bind_group, linear_target: format.is_srgb() });
}

impl Uniforms {
    fn zeroed_default() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

struct GlowCallback {
    uniforms: Uniforms,
}

impl egui_wgpu::CallbackTrait for GlowCallback {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(res) = resources.get::<GlowResources>() {
            let mut u = self.uniforms;
            u.blob_blur[3] = if res.linear_target { 1.0 } else { 0.0 };
            queue.write_buffer(&res.buffer, 0, bytemuck::bytes_of(&u));
        }
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        if let Some(res) = resources.get::<GlowResources>() {
            pass.set_pipeline(&res.pipeline);
            pass.set_bind_group(0, &res.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

fn srgb(c: u8) -> f32 {
    c as f32 / 255.0
}

/// Paints the glow over `rect` (normally the whole window) for `mood`. Colours ease between moods; the blobs
/// drift while the window is live.
pub fn paint(painter: &Painter, rect: Rect, mood: Mood) {
    let ctx = painter.ctx();
    // The drift is a few points per second: the decorations' shared frame rate is plenty.
    let t = anim::clock(ctx, anim::DECOR_FPS);
    let mut blob_pos = [[0.0f32; 4]; 3];
    let mut blob_color = [[0.0f32; 4]; 3];
    let mut blob_blur = [0.0f32; 4];
    let w = rect.width().max(1.0);
    let h = rect.height().max(1.0);
    for (k, (b, target)) in BLOBS.iter().zip(palette(mood)).enumerate() {
        let phase = ease::mirror(t, b.period);
        let x = ease::keyframes(phase, &b.x, ease::in_out);
        let y = ease::keyframes(phase, &b.y, ease::in_out);
        let s = ease::keyframes(phase, &b.scale, ease::in_out);
        blob_pos[k] = [b.center[0] + x / w, b.center[1] + y / h, b.radius[0] * s, b.radius[1] * s];
        let c = anim::spring(ctx, Id::unique(("orbiters-glow", k)), target.map(srgb), Spring::SLOW);
        blob_color[k] = [c[0], c[1], c[2], c[3].clamp(0.0, 1.0)];
        blob_blur[k] = b.blur;
    }
    // Near-black, as the website's page, so the zinc cards stand out.
    let base = Color32::from_rgb(0x04, 0x04, 0x05);
    let uniforms = Uniforms {
        // Dither amplitude (about ±1 code of 255).
        size: [w, h, 0.008, 0.0],
        base: [srgb(base.r()), srgb(base.g()), srgb(base.b()), 0.85],
        blob_pos,
        blob_color,
        blob_blur,
    };
    painter.add(egui_wgpu::Callback::new_paint_callback(rect, GlowCallback { uniforms }));
}
