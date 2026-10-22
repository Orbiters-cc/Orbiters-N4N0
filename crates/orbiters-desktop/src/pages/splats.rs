//! The 3D view of a session's reconstruction: its splats drawn by the same GPU rasteriser that trained them
//! (`orbiters_reconstruct::gpu::Gpu` in render-only mode, on the window's own device), through a paint callback.
//! It opens on the camera of the keyframe the preview shows; drag to orbit, right-drag to pan, scroll to zoom,
//! WASD/QE to fly, and step through the recorded keyframe views. What the keyframes saw poorly (weakly, rarely, from
//! one side, or from much farther than the view now is) can be faded, blurred or turned to haze, and what lies beyond
//! the reach of the depth frames fogged towards the sky's horizon or cropped; both show as trained again.

use super::Job;
use crate::gpu::Handles;
use eframe::egui_wgpu::{self, wgpu};
use egui::{pos2, vec2, Align2, Color32, Id, Rect, Sense, Ui, UiBuilder};
use crate::views::{self, View};
use orbiters_core::reconstruction;
use orbiters_reconstruct::camera::{normalize, Camera};
use orbiters_reconstruct::gpu::{Confidence, ConfidenceMode, FarField, FarMode, Gpu, Target};
use orbiters_reconstruct::ply;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, SegOption, Variant};
use orbiters_ui::{format, icons};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Behind the splats (encoded sRGB, like the trained colours).
const BACKGROUND: [f32; 3] = [0.035, 0.035, 0.043];

/// What the loading thread hands over.
struct Loaded {
    renderer: Arc<Mutex<Renderer>>,
    splats: usize,
    views: Vec<View>,
    /// The view to start from and how far ahead its subject is.
    start: usize,
    depth: f32,
    /// Whether the PLY says what the keyframes saw of its splats (what the confidence control needs), and whether any
    /// lies beyond the depth frames' reach (what the far field control needs).
    seen: bool,
    far: bool,
    /// The sky's colour at the horizon (orbiters_reconstruct::sky::horizon), else the background's: what fog and haze
    /// turn to.
    horizon: [f32; 3],
}

/// What the confidence control shows poorly seen splats as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unsure {
    Shown,
    Faded,
    Blurred,
    Hazed,
}

/// The renderer and the texture it presents into (shared with the paint callback).
struct Renderer {
    gpu: Gpu,
    target: Option<Presented>,
    last: Option<Camera>,
    /// Why the last view could not be drawn (more splats than the GPU sorts, say): shown instead of an empty view.
    error: Option<String>,
    /// Frames left to ask whether the last one left splats out: it did not wait for the GPU's count, which comes back
    /// a frame or two later ([`Gpu::needs_redraw`]), and such a frame is drawn again.
    checks: u8,
}

/// How many frames after a render its count is waited for.
const REDRAW_CHECKS: u8 = 4;

struct Presented {
    size: (u32, u32),
    view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
}

/// Orbiting camera: around `pivot`, up is the world's Y.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Orbit {
    pivot: [f32; 3],
    yaw: f32,
    pitch: f32,
    distance: f32,
    fov_y: f32,
}

impl Orbit {
    fn from_view(v: &View, depth: f32) -> Self {
        let f = normalize(v.forward);
        let pivot = [v.position[0] + f[0] * depth, v.position[1] + f[1] * depth, v.position[2] + f[2] * depth];
        Self { pivot, yaw: f[0].atan2(f[2]), pitch: f[1].clamp(-1.0, 1.0).asin(), distance: depth, fov_y: views::usable_fov(v.fov_y) }
    }

    fn forward(&self) -> [f32; 3] {
        [self.yaw.sin() * self.pitch.cos(), self.pitch.sin(), self.yaw.cos() * self.pitch.cos()]
    }

    fn camera(&self, width: u32, height: u32) -> Camera {
        let f = self.forward();
        let eye = [self.pivot[0] - f[0] * self.distance, self.pivot[1] - f[1] * self.distance, self.pivot[2] - f[2] * self.distance];
        Camera::look(eye, f, [0.0, 1.0, 0.0], self.fov_y, width, height)
    }
}

pub struct Viewer {
    pub session: PathBuf,
    title: String,
    loading: Option<Job<Result<Loaded, String>>>,
    loaded: Option<Loaded>,
    error: Option<String>,
    orbit: Option<Orbit>,
    view: usize,
    unsure: Unsure,
    /// How strongly the control acts, 0 to 1.
    amount: f32,
    far: FarMode,
    /// The far field's distance: fog's thickness, the crop's soft edge (metres).
    far_distance: f32,
}

impl Viewer {
    pub fn open(ctx: &egui::Context, gpu: &Handles, session: &Path, title: String) -> Self {
        let (device, queue, dir) = (gpu.device.clone(), gpu.queue.clone(), session.to_path_buf());
        let loading = Job::spawn(ctx, move || load(&dir, device, queue).map_err(|e| format!("{e:#}")));
        Self {
            session: session.to_path_buf(),
            title,
            loading: Some(loading),
            loaded: None,
            error: None,
            orbit: None,
            view: 0,
            unsure: Unsure::Shown,
            amount: 0.75,
            far: FarMode::Shown,
            far_distance: 20.0,
        }
    }

    pub fn loaded(&self) -> bool {
        self.loaded.is_some()
    }

    /// Draws the viewer over `rect`; returns false once closed.
    pub fn show(&mut self, ui: &mut Ui, rect: Rect) -> bool {
        let ctx = ui.ctx().clone();
        if let Some(job) = self.loading.as_mut() {
            if let Some(result) = job.take() {
                match result {
                    Ok(l) => {
                        self.view = l.start;
                        self.orbit = l.views.get(l.start).map(|v| Orbit::from_view(v, l.depth));
                        self.loaded = Some(l);
                    }
                    Err(e) => self.error = Some(e),
                }
                self.loading = None;
            }
        }
        let p = ui.painter().with_clip_rect(rect);
        p.rect_filled(rect, 0.0, Color32::from_rgb(9, 9, 11));
        let id = Id::unique(("splat-viewer", &self.session));
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        let ppp = ctx.pixels_per_point();
        let (w, h) = (((rect.width() * ppp).round() as u32).max(16), ((rect.height() * ppp).round() as u32).max(16));
        if let (Some(l), Some(orbit)) = (self.loaded.as_ref(), self.orbit.as_mut()) {
            steer(ui, &resp, orbit, rect, h);
            if resp.double_clicked() {
                if let Some(v) = l.views.get(self.view) {
                    *orbit = Orbit::from_view(v, l.depth);
                }
            }
            let camera = orbit.camera(w, h);
            let mode = match self.unsure {
                Unsure::Shown => None,
                Unsure::Faded => Some(ConfidenceMode::Fade),
                Unsure::Blurred => Some(ConfidenceMode::Blur),
                Unsure::Hazed => Some(ConfidenceMode::Haze),
            };
            let confidence = mode.map(|mode| Confidence { amount: self.amount, mode }).unwrap_or_default();
            let far = FarField { mode: self.far, distance: self.far_distance, horizon: l.horizon };
            p.add(egui_wgpu::Callback::new_paint_callback(rect, SplatCallback { renderer: l.renderer.clone(), camera, confidence, far }));
            let (error, checking) = l.renderer.lock().map(|r| (r.error.clone(), r.checks > 0)).unwrap_or_default();
            if let Some(e) = error {
                widgets::text(&p, rect.center(), Align2::CENTER_CENTER, &Txt::new(format!("This view could not be drawn: {e}"), 13.0).color(theme::DANGER));
            }
            if checking {
                ctx.request_repaint();
            }
        } else {
            let (text, color) = match &self.error {
                Some(e) => (format!("The splats could not be shown: {e}"), theme::DANGER),
                None => ("Loading the splats…".to_string(), theme::FG_3),
            };
            widgets::text(&p, rect.center(), Align2::CENTER_CENTER, &Txt::new(text, 13.0).color(color));
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // Top bar: title, counts, views, reset, close.
        let bar = Rect::from_min_size(rect.min + vec2(16.0, 16.0), vec2(rect.width() - 32.0, 44.0));
        let mut open = true;
        let chip = |p: &egui::Painter, r: Rect| p.rect_filled(r, theme::radius(r.height() * 0.5), theme::black(0.55));
        let title_w = (widgets::text_size(ui, &Txt::new(&self.title, 14.0).weight(weight::SEMIBOLD)).x + 40.0).min(bar.width() * 0.5);
        let title_r = Rect::from_min_size(bar.min, vec2(title_w, 44.0));
        chip(&p, title_r);
        widgets::text_truncated(&p, pos2(title_r.left() + 20.0, title_r.center().y - 7.0), Align2::LEFT_CENTER, &Txt::new(&self.title, 14.0).weight(weight::SEMIBOLD), title_w - 40.0);
        let sub = match &self.loaded {
            Some(l) => format!("{} splats", format::integer(l.splats as f64)),
            None => "3D view".into(),
        };
        widgets::text(&p, pos2(title_r.left() + 20.0, title_r.center().y + 9.0), Align2::LEFT_CENTER, &Txt::new(sub, 11.0).color(theme::FG_3));
        let close_r = Rect::from_min_size(pos2(bar.right() - 44.0, bar.top()), vec2(44.0, 44.0));
        if IconButton::new(icons::X, "Close the 3D view").size(44.0, 18.0).backdrop(theme::black(0.55)).show_at(ui, close_r).clicked() {
            open = false;
        }
        if let Some(l) = self.loaded.as_ref() {
            let reset_w = 132.0;
            let reset_r = Rect::from_min_size(pos2(close_r.left() - 10.0 - reset_w, bar.top() + 2.0), vec2(reset_w, 40.0));
            if Button::new("Recorded view").icon(icons::ARROW_COUNTER_CLOCKWISE).variant(Variant::Glass).width(reset_w).show_at(ui, reset_r).clicked() {
                if let (Some(v), Some(o)) = (l.views.get(self.view), self.orbit.as_mut()) {
                    *o = Orbit::from_view(v, l.depth);
                }
            }
            if l.views.len() > 1 {
                let step_w = 40.0;
                let label_w = 112.0;
                let next_r = Rect::from_min_size(pos2(reset_r.left() - 10.0 - step_w, bar.top() + 2.0), vec2(step_w, 40.0));
                let label_r = Rect::from_min_size(pos2(next_r.left() - label_w, bar.top() + 2.0), vec2(label_w, 40.0));
                let prev_r = Rect::from_min_size(pos2(label_r.left() - step_w, bar.top() + 2.0), vec2(step_w, 40.0));
                chip(&p, Rect::from_min_max(prev_r.min, next_r.max));
                widgets::text(&p, label_r.center(), Align2::CENTER_CENTER, &Txt::new(format!("View {} of {}", self.view + 1, l.views.len()), 12.0).color(theme::FG_2));
                let mut step = 0i64;
                if Button::new("‹").variant(Variant::Ghost).width(step_w).show_at(ui, prev_r).clicked() {
                    step = -1;
                }
                if Button::new("›").variant(Variant::Ghost).width(step_w).show_at(ui, next_r).clicked() {
                    step = 1;
                }
                if step != 0 {
                    self.view = (self.view as i64 + step).rem_euclid(l.views.len() as i64) as usize;
                    if let Some(o) = self.orbit.as_mut() {
                        *o = Orbit::from_view(&l.views[self.view], l.depth);
                    }
                }
            }
            let hint = "Drag to orbit · right-drag to pan · scroll to zoom · WASD and QE to fly · double-click for the recorded view";
            let hint_t = Txt::new(hint, 11.5).color(theme::FG_2);
            let hs = widgets::text_size(ui, &hint_t);
            let hint_r = Rect::from_min_size(pos2(rect.left() + 16.0, rect.bottom() - 16.0 - 30.0), vec2(hs.x + 28.0, 30.0));
            chip(&p, hint_r);
            widgets::text(&p, hint_r.center(), Align2::CENTER_CENTER, &hint_t);
            if l.seen {
                let far = l.far;
                self.controls(ui, &p, rect, hint_r, far);
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
        open
    }
}

/// A slider beside a segmented control: its value, range, step and how its value reads.
type ChipSlider<'a> = (&'a mut f32, std::ops::RangeInclusive<f32>, f32, &'a dyn Fn(f32) -> String);

/// A labelled segmented control in a chip ending at `right` with its bottom at `bottom`, with a slider after it while
/// `slider` is given; returns the chip.
#[allow(clippy::too_many_arguments)]
fn control_chip<T: PartialEq + Copy>(
    ui: &mut Ui,
    p: &egui::Painter,
    id: &str,
    right: f32,
    bottom: f32,
    label: &str,
    value: &mut T,
    options: &[SegOption<'_, T>],
    slider: Option<ChipSlider<'_>>,
) -> Rect {
    let label = Txt::new(label, 12.0).color(theme::FG_3);
    let label_w = widgets::text_size(ui, &label).x;
    // The segmented track's width for these labels (widgets::segmented: small labels, 12 px padding, 4 px inset).
    let seg_w = options.iter().map(|o| widgets::text_size(ui, &Txt::new(o.label, 12.0).weight(weight::MEDIUM)).x + 24.0).sum::<f32>() + 8.0;
    let slider_w = if slider.is_some() { 284.0 + 16.0 } else { 0.0 };
    let (w, h) = (18.0 + label_w + 12.0 + seg_w + slider_w + 6.0, 44.0);
    let r = Rect::from_min_size(pos2(right - w, bottom - h), vec2(w, h));
    p.rect_filled(r, theme::radius(h * 0.5), theme::black(0.55));
    widgets::text(p, pos2(r.left() + 18.0, r.center().y), Align2::LEFT_CENTER, &label);
    let seg_r = Rect::from_min_size(pos2(r.left() + 18.0 + label_w + 12.0, r.center().y - 18.0), vec2(seg_w, 36.0));
    let mut seg = ui.new_child(UiBuilder::new().max_rect(seg_r).id_salt(id));
    widgets::segmented(&mut seg, id, value, options, true);
    if let Some((v, range, step, format)) = slider {
        let slider_r = Rect::from_min_size(pos2(seg_r.right() + 16.0, r.center().y - 14.0), vec2(284.0, 28.0));
        let mut s = ui.new_child(UiBuilder::new().max_rect(slider_r).id_salt((id, "amount")));
        widgets::slider(&mut s, (id, "amount"), v, range, step, format);
    }
    r
}

impl Viewer {
    /// The display controls in chips at the bottom right (above the hint where the first does not fit beside it): what
    /// poorly seen splats show as and how strongly, and above it, with `far`, the far field and its distance.
    fn controls(&mut self, ui: &mut Ui, p: &egui::Painter, rect: Rect, hint: Rect, far: bool) {
        let unsure = [SegOption::new(Unsure::Shown, "Show"), SegOption::new(Unsure::Faded, "Fade"), SegOption::new(Unsure::Blurred, "Blur"), SegOption::new(Unsure::Hazed, "Haze")];
        let percent = |v: f32| format!("{:.0} %", v * 100.0);
        let right = rect.right() - 16.0;
        // Beside the hint when the widest chip fits there.
        let widest = 18.0 + 90.0 + 12.0 + 230.0 + 300.0 + 6.0;
        let bottom = if hint.right() + 12.0 + widest <= right { rect.bottom() - 16.0 + 7.0 } else { hint.top() - 10.0 };
        let amount = (self.unsure != Unsure::Shown).then_some((&mut self.amount, 0.0..=1.0, 0.05, &percent as &dyn Fn(f32) -> String));
        let first = control_chip(ui, p, "splat-unsure", right, bottom, "Poorly seen", &mut self.unsure, &unsure, amount);
        if far {
            let options = [SegOption::new(FarMode::Shown, "Show"), SegOption::new(FarMode::Fog, "Fog"), SegOption::new(FarMode::Crop, "Crop")];
            let metres = |v: f32| format!("{v:.0} m");
            let distance = (self.far != FarMode::Shown).then_some((&mut self.far_distance, 5.0..=100.0, 5.0, &metres as &dyn Fn(f32) -> String));
            control_chip(ui, p, "splat-far", right, first.top() - 10.0, "Far away", &mut self.far, &options, distance);
        }
    }
}

/// Mouse and keyboard to the orbit.
fn steer(ui: &Ui, resp: &egui::Response, orbit: &mut Orbit, rect: Rect, height_px: u32) {
    let ctx = ui.ctx();
    let delta = resp.drag_delta();
    if resp.dragged_by(egui::PointerButton::Primary) {
        orbit.yaw += delta.x * 0.006;
        orbit.pitch = (orbit.pitch - delta.y * 0.006).clamp(-1.54, 1.54);
    }
    // World units per point at the pivot, for panning and flying.
    let per_point = orbit.distance * 2.0 * (0.5 * views::usable_fov(orbit.fov_y)).tan() / rect.height().max(1.0);
    let cam = orbit.camera(16, height_px.max(16));
    let (right, up, forward) = (cam.r[0], cam.up(), cam.forward());
    let mut moved = [0f32; 3];
    if resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle) {
        for k in 0..3 {
            moved[k] += -right[k] * delta.x * per_point + up[k] * delta.y * per_point;
        }
    }
    if resp.hovered() {
        let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            orbit.distance = (orbit.distance * (-scroll * 0.002).exp()).clamp(0.05, 5000.0);
        }
        let (dt, keys) = ctx.input(|i| {
            let k = |key| if i.key_down(key) { 1.0f32 } else { 0.0 };
            let fast = if i.modifiers.shift { 3.0 } else { 1.0 };
            (i.stable_dt.min(0.05) * fast, [k(egui::Key::D) - k(egui::Key::A), k(egui::Key::E) - k(egui::Key::Q), k(egui::Key::W) - k(egui::Key::S)])
        });
        if keys != [0.0; 3] {
            let speed = orbit.distance.max(0.5) * dt;
            for k in 0..3 {
                moved[k] += (right[k] * keys[0] + up[k] * keys[1] + forward[k] * keys[2]) * speed;
            }
            ctx.request_repaint();
        }
    }
    for (p, m) in orbit.pivot.iter_mut().zip(moved) {
        *p += m;
    }
}

/// Reads the splats and the recorded views, and sets the renderer up on the window's device.
fn load(dir: &Path, device: wgpu::Device, queue: wgpu::Queue) -> anyhow::Result<Loaded> {
    let g = ply::read(&reconstruction::result_dir(dir).join(reconstruction::PLY_FILE))?;
    let (views, start) = views::recorded(dir)?;
    // Orbit around what that view looks at.
    let depth = views::depth_ahead(&g, &views[start]);
    let mut gpu = Gpu::renderer(device, queue);
    gpu.upload(&g);
    let seen = g.seen.iter().any(|s| s.weight >= 0.0);
    let far = g.seen.iter().any(|s| s.beyond > 0.0);
    let horizon = orbiters_reconstruct::sky::horizon(&g).unwrap_or(BACKGROUND);
    Ok(Loaded { renderer: Arc::new(Mutex::new(Renderer { gpu, target: None, last: None, error: None, checks: 0 })), splats: g.len(), views, start, depth, seen, far, horizon })
}

const BLIT: &str = r#"
@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> params: vec4<f32>;

struct V {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> V {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var v: V;
    v.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    v.uv = uv;
    return v;
}

@fragment
fn fs_main(v: V) -> @location(0) vec4<f32> {
    var c = textureSample(image, image_sampler, v.uv).rgb;
    // The image holds encoded sRGB; an sRGB target encodes again.
    if (params.x > 0.5) {
        c = select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
    }
    return vec4<f32>(c, 1.0);
}
"#;

/// The blit pipeline for the renderer's target (created on first use, kept in its callback resources).
struct Blit {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
}

impl Blit {
    fn new(device: &wgpu::Device, target: orbiters_ui::RenderTarget) -> Self {
        use wgpu::util::DeviceExt;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("splat blit"), source: wgpu::ShaderSource::Wgsl(BLIT.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("splat blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("splat blit"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("splat blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState { count: target.msaa_samples.max(1), mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState { format: target.format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("splat blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let linear = if target.format.is_srgb() { 1.0f32 } else { 0.0 };
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("splat blit"),
            contents: bytemuck::cast_slice(&[linear, 0.0, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        Self { pipeline, layout, sampler, params }
    }
}

struct SplatCallback {
    renderer: Arc<Mutex<Renderer>>,
    camera: Camera,
    confidence: Confidence,
    far: FarField,
}

impl egui_wgpu::CallbackTrait for SplatCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if resources.get::<Blit>().is_none() {
            let Some(target) = resources.get::<orbiters_ui::RenderTarget>().copied() else { return Vec::new() };
            resources.insert(Blit::new(device, target));
        }
        let blit = resources.get::<Blit>().expect("inserted");
        let mut r = self.renderer.lock().unwrap();
        let size = (self.camera.width, self.camera.height);
        if r.target.as_ref().is_none_or(|t| t.size != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("splats"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("splat blit"),
                layout: &blit.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&blit.sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: blit.params.as_entire_binding() },
                ],
            });
            r.target = Some(Presented { size, view, bind_group });
            r.last = None;
        }
        let changed = r.gpu.confidence != self.confidence || r.gpu.far != self.far;
        r.gpu.confidence = self.confidence;
        r.gpu.far = self.far;
        let moved = r.last != Some(self.camera) || changed;
        let left_out = !moved && r.checks > 0 && {
            r.checks -= 1;
            r.gpu.needs_redraw()
        };
        if moved || left_out {
            match r.gpu.step(&self.camera, BACKGROUND, Target::None, None) {
                Ok(_) => {
                    let view = r.target.as_ref().expect("target").view.clone();
                    r.gpu.present(&view);
                    r.error = None;
                    r.checks = REDRAW_CHECKS;
                }
                Err(e) => r.error = Some(format!("{e:#}")),
            }
            r.last = Some(self.camera);
        }
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let (Some(blit), Ok(r)) = (resources.get::<Blit>(), self.renderer.lock()) else { return };
        let Some(t) = r.target.as_ref() else { return };
        pass.set_pipeline(&blit.pipeline);
        pass.set_bind_group(0, &t.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
