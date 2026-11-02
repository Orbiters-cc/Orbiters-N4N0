//! Edit in 3D: the photo editor over the live 3D view. Its layers and controls stay beside a canvas where the view goes
//! on being drawn and steered as in the view (drag to orbit, right-drag to pan, scroll to zoom, WASD and QE to fly,
//! double-click for the recorded view; a drag that starts on a handle moves the handle), and every layer applies to
//! each frame on the GPU ([`orbiters_ui::photo::gpu::Stack`]: the CPU editor's maths, redone from the layer being
//! changed while the camera stands still, not at all while nothing changes). The layers are shares of the picture, so
//! they stay where they are on it while the camera moves.
//!
//! The picture has the view's shape when the mode opened, [`STILL_LONG`] pixels on its long side when saved; on the
//! canvas it is drawn at the canvas's pixels (more of the view when cropped, up to its full size). Save picture renders
//! the view at full size, puts the layers on it on the GPU and writes the view as Save this view does, then its edited
//! copy beside it with the edit and the camera ([`files::View3d`]): Edit in 3D on that copy comes back to this view
//! with these layers. Save this view writes the view without them. Leaving keeps the layers for the next time.
//!
//! A depth blur is drawn on the splats themselves ([`lens`], orbiters_reconstruct::dof): each frame's projection blurs
//! every splat by its own circle of confusion. Its auto focus asks the projection for the depth under the focus dot
//! (the dot's place on the picture taken back through the crop to the view's pixels) and eases towards each answer.

use super::super::editor::{files, Editor, LiveAsk};
use super::*;
use orbiters_ui::photo::gpu::{self, Stack};
use orbiters_ui::photo::render::{self as photo_render, Stage};
use orbiters_reconstruct::dof;
use orbiters_ui::photo::{Edit, Focus};
use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Default)]
pub(super) struct State {
    editing: Option<Editing>,
    /// The layers when the mode was left: the next time starts with them.
    kept: Option<Edit>,
    /// A copy's view and edit to open once the splats are there (Edit in 3D on a copy saved here).
    pending: Option<(files::View3d, Edit)>,
}

impl State {
    pub(super) fn reopen(&mut self, view: files::View3d, edit: Edit) {
        self.pending = Some((view, edit));
        self.editing = None;
    }
}

struct Editing {
    editor: Editor,
    /// The picture's size when saved.
    full: (u32, u32),
    shared: Arc<Mutex<Shared>>,
    /// When a picture was asked, for the shutter's flash.
    flashed: f64,
    /// The auto focus's distance as drawn (eased towards each probe's), and when it was last eased.
    focus: Option<f32>,
    focused_at: f64,
}

/// What the paint callback and the interface share.
#[derive(Default)]
struct Shared {
    /// Made on a thread of its own when the mode opens (its passes take a moment to make).
    stack: Option<Stack>,
    /// The source's frames last seen: a new one goes through the layers again.
    frames: Option<u64>,
    /// What draws the stack's picture, for the size it is.
    bind: Option<((u32, u32), wgpu::BindGroup)>,
}

fn unsure_name(u: Unsure) -> &'static str {
    match u {
        Unsure::Shown => "shown",
        Unsure::Faded => "faded",
        Unsure::Blurred => "blurred",
        Unsure::Hazed => "hazed",
    }
}

fn far_name(f: FarMode) -> &'static str {
    match f {
        FarMode::Shown => "shown",
        FarMode::Fog => "fog",
        FarMode::Crop => "crop",
    }
}

/// How the view shows poorly seen and far splats.
fn shown_as(unsure: Unsure, amount: f32, far: FarMode, far_distance: f32, horizon: [f32; 3]) -> (Confidence, FarField) {
    let mode = match unsure {
        Unsure::Shown => None,
        Unsure::Faded => Some(ConfidenceMode::Fade),
        Unsure::Blurred => Some(ConfidenceMode::Blur),
        Unsure::Hazed => Some(ConfidenceMode::Haze),
    };
    (mode.map(|mode| Confidence { amount, mode }).unwrap_or_default(), FarField { mode: far, distance: far_distance, horizon })
}

impl Viewer {
    /// Opens the 3D view of `view`'s capture where `view` was, in Edit in 3D with `edit` (a copy saved there).
    pub fn open_edit(ctx: &egui::Context, gpu: &Handles, view: files::View3d, edit: Edit, title: String, integrated: bool) -> Self {
        let mut v = Self::open(ctx, gpu, &view.capture, title, integrated);
        v.edit3d.reopen(view, edit);
        v
    }

    /// Edit in 3D is open.
    pub fn editing(&self) -> bool {
        self.edit3d.editing.is_some()
    }

    /// Edit in 3D's editor (screenshots stage its layers).
    pub fn editing_mut(&mut self) -> Option<&mut Editor> {
        self.edit3d.editing.as_mut().map(|e| &mut e.editor)
    }

    /// Turns the camera about its pivot by `yaw` and `pitch` radians (screenshots shoot two angles).
    pub fn turn(&mut self, yaw: f32, pitch: f32) {
        if let Some(o) = self.orbit.as_mut() {
            o.yaw += yaw;
            o.pitch = (o.pitch + pitch).clamp(-1.54, 1.54);
        }
    }

    /// Opens Edit in 3D for a picture of the view's shape in `rect` (the layers left last time, or none).
    pub(super) fn enter_edit(&mut self, ctx: &egui::Context, rect: Rect) {
        let ppp = ctx.pixels_per_point();
        let (vw, vh) = ((rect.width() * ppp).max(16.0), (rect.height() * ppp).max(16.0));
        let k = (STILL_LONG / vw.max(vh)).max(1.0);
        let doc = self.edit3d.kept.take().unwrap_or_default();
        self.enter_with(ctx, ((vw * k).round() as u32, (vh * k).round() as u32), doc);
    }

    fn enter_with(&mut self, ctx: &egui::Context, full: (u32, u32), doc: Edit) {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (s, (device, queue), c) = (shared.clone(), self.window.clone(), ctx.clone());
        let made = std::thread::Builder::new().name("photo layers on the GPU".into()).spawn(move || {
            let stack = Stack::live(&device, &queue);
            if let Ok(mut s) = s.lock() {
                s.stack = Some(stack);
            }
            c.request_repaint();
        });
        if let Err(e) = made {
            eprintln!("Edit in 3D: {e}");
        }
        self.edit3d.editing = Some(Editing { editor: Editor::live(self.title.clone(), full, doc), full, shared, flashed: f64::NEG_INFINITY, focus: None, focused_at: 0.0 });
    }
}

/// A copy's view asked for (Edit in 3D on it): the camera and the display as they were, then the mode with its edit,
/// once the splats are there.
fn resume(v: &mut Viewer, ctx: &egui::Context, rect: Rect) {
    if v.loaded.is_none() || v.edit3d.pending.is_none() {
        return;
    }
    let Some((view, edit)) = v.edit3d.pending.take() else { return };
    if view.distance > 0.0 && view.fov_y > 0.0 {
        v.orbit = Some(Orbit { pivot: view.pivot, yaw: view.yaw, pitch: view.pitch, distance: view.distance, fov_y: view.fov_y });
    }
    v.unsure = match view.poorly_seen.as_str() {
        "faded" => Unsure::Faded,
        "blurred" => Unsure::Blurred,
        "hazed" => Unsure::Hazed,
        _ => Unsure::Shown,
    };
    if view.amount > 0.0 {
        v.amount = view.amount.clamp(0.0, 1.0);
    }
    v.far = match view.far.as_str() {
        "fog" => FarMode::Fog,
        "crop" => FarMode::Crop,
        _ => FarMode::Shown,
    };
    if view.far_distance > 0.0 {
        v.far_distance = view.far_distance;
    }
    if view.size[0] >= 16 && view.size[1] >= 16 {
        v.enter_with(ctx, (view.size[0], view.size[1]), edit);
    } else {
        v.edit3d.kept = Some(edit);
        v.enter_edit(ctx, rect);
    }
}

/// The depth of field `edit` draws the splats with at `stage` (its topmost shown depth blur; none while compared with
/// the view without its layers) for a view of `full` pixels rendered at `input` pixels, and the pixel of that render
/// its auto focus asks about. `focus` is the auto focus's distance as drawn; `plane` tints the focus of a manual one.
pub(super) fn lens(edit: &Edit, stage: Stage, full: (u32, u32), input: (u32, u32), focus: Option<f32>, plane: bool) -> (DepthOfField, Option<[f32; 2]>) {
    let Some((b, opacity)) = edit.depth_blur().filter(|_| stage == Stage::Full) else { return (DepthOfField::default(), None) };
    let probe = match b.focus {
        Focus::Auto => probe_pixel(edit, full, input, b.point),
        Focus::Manual => None,
    };
    let distance = match b.focus {
        Focus::Manual => b.distance.clamp(orbiters_ui::photo::Blur::MIN_DISTANCE, orbiters_ui::photo::Blur::MAX_DISTANCE),
        Focus::Auto => focus.unwrap_or(0.0),
    };
    let dof = DepthOfField {
        aperture: b.aperture.clamp(0.0, 1.0) * opacity * dof::APERTURE_MAX,
        focus: distance,
        near: b.near.clamp(0.0, 1.0),
        far: b.far.clamp(0.0, 1.0),
        show_plane: plane && b.focus == Focus::Manual,
    };
    (dof, probe)
}

/// The pixel of a render of `input` pixels (of the whole view, `full` pixels when saved) under `point`, a share of the
/// cropped picture: back through the crop, the turns, the straightening and the tilts. None beyond the view.
pub(super) fn probe_pixel(edit: &Edit, full: (u32, u32), input: (u32, u32), point: [f32; 2]) -> Option<[f32; 2]> {
    let g = &edit.geometry;
    let s = g.source_point(g.effective_crop(full.0, full.1), full.0 as f32 / full.1.max(1) as f32, point[0].clamp(0.0, 1.0), point[1].clamp(0.0, 1.0));
    ((0.0..=1.0).contains(&s[0]) && (0.0..=1.0).contains(&s[1])).then(|| [s[0] * input.0 as f32 - 0.5, s[1] * input.1 as f32 - 0.5])
}

/// Edit in 3D over `rect` while it is open: Some(whether the view stays open), else None (the view draws itself).
pub(super) fn show(v: &mut Viewer, ui: &mut Ui, rect: Rect) -> Option<bool> {
    let ctx = ui.ctx().clone();
    resume(v, &ctx, rect);
    let ed = v.edit3d.editing.as_mut()?;
    let open = ed.editor.show(ui, rect);
    let (slot, free, asked) = match ed.editor.live_mut() {
        Some(l) => (l.slot.take(), l.free.take(), l.asked.take()),
        None => (None, None, None),
    };
    if !open {
        v.edit3d.kept = Some(ed.editor.edit().clone());
        v.edit3d.editing = None;
        return Some(true);
    }
    let horizon = v.loaded.as_ref().map_or(BACKGROUND, |l| l.horizon);
    let (confidence, far) = shown_as(v.unsure, v.amount, v.far, v.far_distance, horizon);
    let (Some(l), Some(orbit)) = (v.loaded.as_mut(), v.orbit.as_mut()) else { return Some(true) };
    let ppp = ctx.pixels_per_point();
    let now = ctx.input(|i| i.time);
    let before = *orbit;
    let typing = ctx.memory(|m| m.focused().is_some());
    if let (Some(resp), Some(s), false) = (&free, &slot, typing) {
        steer(ui, resp, orbit, s.frame, ((s.frame.height() * ppp) as u32).max(16), false);
        if resp.double_clicked() {
            if let Some(view) = l.views.get(v.view) {
                *orbit = Orbit::from_view(view, l.depth);
            }
        }
    }
    if *orbit != before {
        v.moved = now;
    }
    let pose = *orbit;
    if let Some(s) = &slot {
        // The view drawn at about the canvas's pixels of the picture shown (more of it when cropped).
        let out = (((s.frame.width() * ppp).round() as u32).max(16), ((s.frame.height() * ppp).round() as u32).max(16));
        let shown = photo_render::full_size(ed.editor.edit(), ed.full, s.stage);
        let k = (out.0 as f32 / shown.0.max(1) as f32).max(out.1 as f32 / shown.1.max(1) as f32).min(1.0);
        let mut input = (((ed.full.0 as f32 * k).round() as u32).max(16), ((ed.full.1 as f32 * k).round() as u32).max(16));
        if (v.integrated || l.source.integrated()) && now - v.moved < SHARP_AFTER {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SHARP_AFTER - (now - v.moved)));
            input = within(input.0, input.1, MOVING_PIXELS);
        }
        let camera = orbit.camera(input.0, input.1);
        // The auto focus: towards the distance its last probe found, eased.
        let measured = match &l.source {
            Source::Window(renderer) => renderer.lock().ok().and_then(|r| r.focus),
            Source::Integrated(worker) => worker.focus(),
        };
        let auto = ed.editor.edit().depth_blur().is_some_and(|(b, _)| b.focus == Focus::Auto);
        if auto {
            let dt = (now - ed.focused_at).clamp(0.0, 0.1) as f32;
            ed.focus = match (ed.focus, measured) {
                (Some(f), Some(m)) if (f.ln() - m.ln()).abs() > 0.002 => {
                    ctx.request_repaint();
                    Some(dof::ease(f, m, dt))
                }
                (_, Some(m)) => Some(m),
                (f, None) => f,
            };
        }
        ed.focused_at = now;
        let plane = ed.editor.live_mut().is_some_and(|l| l.setting_focus && l.show_plane);
        let (depth, probe) = lens(ed.editor.edit(), s.stage, ed.full, input, ed.focus, plane);
        let shown_focus = (ed.editor.edit().depth_blur().is_some() && depth.focus > 0.0).then_some(depth.focus);
        if let Some(live) = ed.editor.live_mut() {
            live.focus = shown_focus;
        }
        // Without what Select splats hid or deleted.
        let marks = l.marks();
        let (inner, error) = match &mut l.source {
            Source::Window(renderer) => {
                let (error, checking) = renderer.lock().map(|r| (r.error.clone(), r.checks > 0 || r.probing)).unwrap_or_default();
                if checking {
                    ctx.request_repaint();
                }
                (Inner::Window(SplatCallback { renderer: renderer.clone(), camera, confidence, far, dof: depth, probe, marks }, renderer.clone()), error)
            }
            Source::Integrated(worker) => {
                if let Some((version, m)) = marks {
                    worker.set_marks(version, m);
                }
                worker.ask(Request { camera, confidence, far, dof: depth, probe, marks: worker.marks() });
                (Inner::Integrated(Box::new(worker.callback()), worker.shown()), worker.error())
            }
        };
        let callback = LiveCallback { inner, edit: ed.editor.edit().clone(), stage: s.stage, full: ed.full, size: out, shared: ed.shared.clone() };
        ctx.layer_painter(s.layer).with_clip_rect(s.clip).set(s.shape, egui_wgpu::Callback::new_paint_callback(s.frame, callback));
        let p = ctx.layer_painter(s.layer).with_clip_rect(s.clip.intersect(s.frame));
        if let Some(e) = error {
            widgets::text(&p, s.frame.center(), Align2::CENTER_CENTER, &Txt::new(format!("This view could not be drawn: {e}"), 13.0).color(theme::DANGER));
        } else if ed.shared.lock().map(|sh| sh.stack.is_none()).unwrap_or(false) {
            orbiters_ui::feedback::spinner(&p, s.frame.center(), 12.0, 2.5, theme::ACCENT);
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        // The shutter.
        let flash = ((now - ed.flashed) / 0.35) as f32;
        if (0.0..1.0).contains(&flash) {
            p.rect_filled(s.frame, 0.0, theme::white(0.3 * (1.0 - flash)));
            ctx.request_repaint();
        }
    }
    let Some(asked) = asked else { return Some(true) };
    if ed.editor.saving() {
        return Some(true);
    }
    // At full size, from where the camera is now.
    let camera = pose.camera(ed.full.0, ed.full.1);
    let progress = Arc::new(AtomicU32::new(0));
    // The splats as drawn: without what Select splats hid or deleted; no picture while its edits wait to be discarded.
    let drawn = v.loaded.as_ref().map_or(Ok(None), |l| l.drawn());

    let (dir, window, p) = (v.session.clone(), v.window.clone(), progress.clone());
    let job = match asked {
        LiveAsk::View => Job::spawn(&ctx, move || {
            let r = drawn.and_then(|d| still(&dir, window, camera, confidence, far, d).map_err(|e| format!("{e:#}")));
            p.store(1000, Ordering::Relaxed);
            r
        }),
        LiveAsk::Picture => {
            let view = files::View3d {
                capture: v.session.clone(),
                pivot: pose.pivot,
                yaw: pose.yaw,
                pitch: pose.pitch,
                distance: pose.distance,
                fov_y: pose.fov_y,
                position: camera.position(),
                forward: camera.forward(),
                size: [ed.full.0, ed.full.1],
                poorly_seen: unsure_name(v.unsure).into(),
                amount: v.amount,
                far: far_name(v.far).into(),
                far_distance: v.far_distance,
            };
            let edit = ed.editor.edit().clone();
            // The depth blur as drawn now (the auto focus where it is), at full size.
            let (depth, _) = lens(&edit, Stage::Full, ed.full, ed.full, ed.focus, false);
            Job::spawn(&ctx, move || drawn.and_then(|d| picture(&dir, window, camera, (confidence, far, depth), &edit, view, &p, d).map_err(|e| format!("{e:#}"))))
        }
    };
    ed.editor.start_saving(job, progress);
    ed.flashed = now;
    Some(true)
}

/// The view of `camera` with the layers of `edit` put on it on the GPU, saved: the view as Save this view saves it,
/// then its edited copy beside it with the edit and `view` (where the camera was). Returns the copy. With a depth of
/// field (`shown.2`), the copy's layers go over a second render of the splats blurred by it. `drawn`: what Select splats
/// hid or deleted is not drawn.
#[allow(clippy::too_many_arguments)]
fn picture(
    dir: &Path,
    window: (wgpu::Device, wgpu::Queue),
    camera: Camera,
    shown: (Confidence, FarField, DepthOfField),
    edit: &Edit,
    view: files::View3d,
    progress: &AtomicU32,
    drawn: Option<Drawn>,
) -> anyhow::Result<PathBuf> {
    let (confidence, far, depth) = shown;
    let full = (camera.width, camera.height);
    progress.store(50, Ordering::Relaxed);
    let dofs = if depth.on() { vec![DepthOfField::default(), depth] } else { vec![DepthOfField::default()] };
    let mut images = shoot(dir, window.clone(), camera, confidence, far, &dofs, drawn)?;
    let image = images.pop().expect("a view");
    let plain = images.pop();
    progress.store(550, Ordering::Relaxed);
    let original = files::save_view(dir, plain.as_ref().unwrap_or(&image))?;
    progress.store(650, Ordering::Relaxed);
    let mut stack = Stack::new(&window.0, &window.1);
    let (_texture, input) = gpu::upload(&window.0, &window.1, &image);
    let size = photo_render::full_size(edit, full, Stage::Full);
    stack.run(edit, &input, full, full, Stage::Full, size, true);
    let edited = stack.read()?;
    progress.store(850, Ordering::Relaxed);
    let copy = files::copy_path(&original);
    files::write_png(&copy, &edited, files::xmp_chunk(&original).as_deref())?;
    let saved = files::Saved { view: Some(view), ..files::Saved::new(super::super::editor::Source::picture(&original), edit.clone()) };
    files::write_sidecar(&copy, &saved)?;
    progress.store(1000, Ordering::Relaxed);
    Ok(copy)
}

/// The splats drawn as the view draws them, then its picture through the layers into the canvas's place.
struct LiveCallback {
    inner: Inner,
    edit: Edit,
    stage: Stage,
    full: (u32, u32),
    size: (u32, u32),
    shared: Arc<Mutex<Shared>>,
}

/// What draws the splats, and where its frames are then.
enum Inner {
    Window(SplatCallback, Arc<Mutex<Renderer>>),
    Integrated(Box<dyn egui_wgpu::CallbackTrait>, super::super::splat_worker::Latest),
}

impl egui_wgpu::CallbackTrait for LiveCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let cmds = match &self.inner {
            Inner::Window(cb, _) => cb.prepare(device, queue, screen, encoder, resources),
            Inner::Integrated(cb, _) => cb.prepare(device, queue, screen, encoder, resources),
        };
        let source = match &self.inner {
            Inner::Window(_, r) => r.lock().ok().and_then(|r| r.target.as_ref().map(|t| (t.view.clone(), t.size, r.drawn))),
            Inner::Integrated(_, latest) => latest.get(),
        };
        let Some((view, input_size, frames)) = source else { return cmds };
        let Some(show) = Show::of(device, resources) else { return cmds };
        let Ok(mut shared) = self.shared.lock() else { return cmds };
        let Shared { stack, frames: seen, bind } = &mut *shared;
        let fresh = *seen != Some(frames);
        *seen = Some(frames);
        let Some(stack) = stack.as_mut() else { return cmds };
        stack.run(&self.edit, &view, input_size, self.full, self.stage, self.size, fresh);
        if let (Some(out), Some(size)) = (stack.output(), stack.output_size()) {
            if bind.as_ref().is_none_or(|(s, _)| *s != size) {
                *bind = Some((size, show.bind_group(device, out)));
            }
        }
        cmds
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let (Some(show), Ok(shared)) = (resources.get::<Show>(), self.shared.lock()) else { return };
        if let Some((_, b)) = &shared.bind {
            show.draw(pass, b);
        }
    }
}

const SHOW: &str = r#"
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
    let s = textureSample(image, image_sampler, v.uv);
    var c = s.rgb;
    // The picture holds encoded sRGB; an sRGB target encodes again.
    if (params.x > 0.5) {
        c = select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
    }
    // Over the canvas (its checkerboard shows where the picture is transparent).
    return vec4<f32>(c * s.a, s.a);
}
"#;

/// Draws the layers' picture (RGBA8, encoded, straight alpha) over the callback's rectangle, blended over what is
/// under it (created on first use, kept in the callback resources).
struct Show {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params: wgpu::Buffer,
}

impl Show {
    fn new(device: &wgpu::Device, target: orbiters_ui::RenderTarget) -> Self {
        use wgpu::util::DeviceExt;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("edit in 3D"), source: wgpu::ShaderSource::Wgsl(SHOW.into()) });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("edit in 3D"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: fragment,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: fragment, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: fragment,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("edit in 3D"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("edit in 3D"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState { count: target.msaa_samples.max(1), mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState { format: target.format, blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("edit in 3D"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let linear = if target.format.is_srgb() { 1.0f32 } else { 0.0 };
        let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("edit in 3D"),
            contents: bytemuck::cast_slice(&[linear, 0.0, 0.0, 0.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        Self { pipeline, layout, sampler, params }
    }

    fn of<'a>(device: &wgpu::Device, resources: &'a mut egui_wgpu::CallbackResources) -> Option<&'a Show> {
        if resources.get::<Show>().is_none() {
            let target = resources.get::<orbiters_ui::RenderTarget>().copied()?;
            resources.insert(Show::new(device, target));
        }
        resources.get::<Show>()
    }

    fn bind_group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("edit in 3D"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: self.params.as_entire_binding() },
            ],
        })
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'static>, bind_group: &wgpu::BindGroup) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_ui::photo::{Effect, Light};
    use orbiters_ui::photo::render::Stage;

    /// The depth of field a stack draws the splats with: its topmost depth blur's lens and focus, none while compared
    /// with the plain view, an auto focus off until its first probe and asking about the dot's pixel through the crop.
    #[test]
    fn a_depth_blur_sets_the_lens_and_where_the_auto_focus_looks() {
        use orbiters_ui::photo::{Blur, Crop, Focus};
        let full = (2000, 1000);
        let input = (1000, 500);
        let mut edit = Edit::default();
        assert_eq!(lens(&edit, Stage::Full, full, input, None, false), (DepthOfField::default(), None));
        let mut l = edit.layer(Effect::Blur(Blur { focus: Focus::Manual, distance: 4.0, aperture: 0.5, near: 0.3, ..Blur::depth() }));
        l.opacity = 0.8;
        edit.layers.push(l);
        let (d, probe) = lens(&edit, Stage::Full, full, input, Some(9.0), true);
        assert_eq!(probe, None, "a manual focus asks nothing");
        assert_eq!((d.focus, d.near, d.far, d.show_plane), (4.0, 0.3, 1.0, true));
        assert!((d.aperture - 0.5 * 0.8 * dof::APERTURE_MAX).abs() < 1e-6);
        assert_eq!(lens(&edit, Stage::Plain, full, input, None, false).0, DepthOfField::default(), "compared: the view as it is");
        // Auto: off until a focus came back, then at it; the dot's pixel asked about, never the plane.
        edit.layers[0].effect = Effect::Blur(Blur { point: [0.25, 0.5], ..Blur::depth() });
        let (d, probe) = lens(&edit, Stage::Full, full, input, None, true);
        assert!(!d.on() && !d.show_plane);
        assert_eq!(probe, Some([249.5, 249.5]));
        let (d, _) = lens(&edit, Stage::Full, full, input, Some(6.0), false);
        assert!(d.on() && d.focus == 6.0);
        // Cropped to the right half: the dot in the middle of the picture is three quarters across the view.
        edit.geometry.crop = Crop { x: 0.5, y: 0.0, w: 0.5, h: 1.0 };
        let p = probe_pixel(&edit, full, input, [0.5, 0.5]).expect("inside");
        assert!((p[0] - 749.5).abs() < 0.5 && (p[1] - 249.5).abs() < 0.5, "{p:?}");
    }

    #[test]
    fn a_view_reads_back_as_it_was_written() {
        let view = files::View3d {
            capture: "C:/captures/rooftop".into(),
            pivot: [1.0, 1.6, -3.0],
            yaw: 0.4,
            pitch: -0.1,
            distance: 2.5,
            fov_y: 1.0,
            size: [2560, 1440],
            poorly_seen: unsure_name(Unsure::Hazed).into(),
            amount: 0.6,
            far: far_name(FarMode::Fog).into(),
            far_distance: 35.0,
            ..Default::default()
        };
        let mut edit = Edit::default();
        let l = edit.layer(Effect::Light(Light { exposure: 0.3, ..Default::default() }));
        edit.layers.push(l);
        let saved = files::Saved { view: Some(view.clone()), ..files::Saved::new(super::super::super::editor::Source::picture("a.png"), edit) };
        let back: files::Saved = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
        assert_eq!(back, saved);
        // An edit saved without a view has none.
        let plain: files::Saved = serde_json::from_str(r#"{"format":"orbiters-photo-edit","version":1,"source":{"picture":"x.png"},"edit":{}}"#).unwrap();
        assert!(plain.view.is_none());
        assert!(!serde_json::to_string(&files::Saved::new(Default::default(), Edit::default())).unwrap().contains("\"view\""));
    }
}
