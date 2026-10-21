//! The SteamVR overlay process (`orbiters-desktop --overlay --service <port> <token>`): Orbiters as a tab in
//! SteamVR's dashboard, started by the tray service while SteamVR runs. Its egui interface is painted on the CPU
//! ([`orbiters_ui::software`]: no GPU device, so none of a graphics driver's ~100 MB per process) and handed to
//! SteamVR as a texture, only while the dashboard shows the tab and only when something changed. It reads the
//! device poses itself for T-pose calibrations, which also put a mirror and two hand targets in the play space.

pub mod calibration;
mod frames;
pub mod ui;
pub mod walk;

use crate::backend::{Ctl, Remote};
use crate::ipc::{Client, Endpoint, Role};
use anyhow::Context as _;
use calibration::{MIRROR_PX, MIRROR_WIDTH_M, TARGET_PX, TARGET_WIDTH_M};
use egui::{pos2, Event, PointerButton, RawInput, Rect, ViewportId, ViewportInfo};
use orbiters_contracts::performance::tracking::{Device, DeviceClass, Sample};
use orbiters_openvr::{self as vr, Pose, MAX_DEVICES};
use orbiters_ui::software::Software;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Pixels of the dashboard tab (the interface is laid out in points at 1 pixel per point).
const WIDTH: u32 = ui::SIZE.x as u32;
const HEIGHT: u32 = ui::SIZE.y as u32;
const KEY: &str = "cc.orbiters.desktop";

/// SteamVR's device poses for calibrating, described once per device.
struct SteamVrPoses<'a> {
    client: &'a vr::Client,
    described: HashMap<u32, Option<Device>>,
}

impl ui::Poses for SteamVrPoses<'_> {
    fn read(&mut self) -> (Vec<Device>, Vec<Sample>) {
        let mut poses = [Pose::default(); MAX_DEVICES];
        self.client.poses(vr::Universe::Standing, &mut poses);
        let mut devices = Vec::new();
        let mut samples = Vec::new();
        for (i, p) in poses.iter().enumerate() {
            if !p.device_is_connected {
                continue;
            }
            let d = self.described.entry(i as u32).or_insert_with(|| self.client.describe(i as u32)).clone();
            let Some(d) = d.filter(|d| d.class != DeviceClass::TrackingReference) else { continue };
            samples.push(vr::sample_of(0, devices.len(), p));
            devices.push(d);
        }
        (devices, samples)
    }
}

/// Hands pictures to SteamVR: through D3D11 textures (raw pixels flicker when they change every frame), or as raw
/// pixels when no device could be made.
struct Presenter {
    #[cfg(windows)]
    gpu: Option<vr::gpu::OverlayTextures>,
}

impl Presenter {
    fn new(steamvr: &vr::Client) -> Self {
        #[cfg(windows)]
        {
            let gpu = vr::gpu::OverlayTextures::new(steamvr.dxgi_adapter_index()).map_err(|e| eprintln!("overlay textures: {e}; raw pixels instead")).ok();
            Self { gpu }
        }
        #[cfg(not(windows))]
        {
            let _ = steamvr;
            Self {}
        }
    }

    fn show(&mut self, overlay: &vr::Overlay, handle: vr::OverlayHandle, rgba: &[u8], width: u32, height: u32) {
        #[cfg(windows)]
        if let Some(gpu) = self.gpu.as_mut() {
            match gpu.present(overlay, handle, rgba, width, height) {
                Ok(()) => return,
                Err(e) => {
                    eprintln!("{e}; raw pixels instead");
                    self.gpu = None;
                }
            }
        }
        if let Err(code) = overlay.set_raw(handle, rgba, width, height) {
            eprintln!("overlay pixels: SteamVR overlay error {code}");
        }
    }
}

/// An egui canvas painted on the CPU for one of the world overlays.
pub struct Canvas {
    ctx: egui::Context,
    painter: Software,
    size: egui::Vec2,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        Self { ctx, painter: Software::new(width, height), size: egui::vec2(width as f32, height as f32) }
    }

    /// Paints a frame (RGBA, premultiplied, transparent where nothing was drawn).
    pub fn paint(&mut self, time: f64, draw: impl FnMut(&egui::Painter)) -> &[u8] {
        let mut draw = draw;
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), self.size)), time: Some(time), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), ..Default::default() });
        let mut full = self.ctx.run_ui(input, |ui| draw(ui.painter()));
        self.painter.set_textures(&mut full.textures_delta);
        let primitives = self.ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
        self.painter.free_textures(&mut full.textures_delta);
        self.painter.paint(&primitives, full.pixels_per_point, egui::Color32::TRANSPARENT)
    }
}

/// The calibration's mirror and hand targets in the play space.
struct World {
    mirror: vr::OverlayHandle,
    targets: [vr::OverlayHandle; 2],
    mirror_canvas: Canvas,
    target_canvas: Canvas,
    /// The frame they were placed for (placed again when a new calibration starts).
    placed: Option<[f32; 3]>,
    shown: bool,
    last_paint: Option<Instant>,
}

impl World {
    fn new(overlay: &vr::Overlay) -> anyhow::Result<Self> {
        let fail = |what: &str, code: i32| anyhow::anyhow!("{what}: SteamVR overlay error {code}");
        let mirror = overlay.create(&format!("{KEY}.mirror"), "Orbiters calibration mirror").map_err(|c| fail("mirror", c))?;
        let _ = overlay.set_width_in_meters(mirror, MIRROR_WIDTH_M);
        let mut targets = [0; 2];
        for (i, side) in ["left", "right"].into_iter().enumerate() {
            targets[i] = overlay.create(&format!("{KEY}.target.{side}"), &format!("Orbiters {side} hand target")).map_err(|c| fail("hand target", c))?;
            let _ = overlay.set_width_in_meters(targets[i], TARGET_WIDTH_M);
        }
        Ok(Self {
            mirror,
            targets,
            mirror_canvas: Canvas::new(MIRROR_PX[0], MIRROR_PX[1]),
            target_canvas: Canvas::new(TARGET_PX, TARGET_PX),
            placed: None,
            shown: false,
            last_paint: None,
        })
    }

    fn update(&mut self, overlay: &vr::Overlay, presenter: &mut Presenter, cal: &calibration::Calibrator, now: f64) {
        let frame = cal.frame.filter(|_| cal.in_world(now));
        let Some(frame) = frame else {
            if self.shown {
                for h in [self.mirror, self.targets[0], self.targets[1]] {
                    let _ = overlay.hide(h);
                }
                self.shown = false;
            }
            return;
        };
        if self.placed != Some(frame.origin) {
            let _ = overlay.set_transform_absolute(self.mirror, &frame.mirror_transform());
            for i in 0..2 {
                let _ = overlay.set_transform_absolute(self.targets[i], &frame.target_transform(i));
            }
            self.placed = Some(frame.origin);
        }
        // About 30 pictures a second.
        if self.last_paint.is_some_and(|t| t.elapsed() < Duration::from_millis(33)) {
            return;
        }
        self.last_paint = Some(Instant::now());
        let rgba = self.mirror_canvas.paint(now, |p| calibration::paint_mirror(p, cal, now)).to_vec();
        presenter.show(overlay, self.mirror, &rgba, MIRROR_PX[0], MIRROR_PX[1]);
        for (i, label) in ["L", "R"].into_iter().enumerate() {
            let on = cal.hand_on_target(i);
            let rgba = self.target_canvas.paint(now, |p| calibration::paint_target(p, label, on, now)).to_vec();
            presenter.show(overlay, self.targets[i], &rgba, TARGET_PX, TARGET_PX);
        }
        if !self.shown {
            for h in [self.mirror, self.targets[0], self.targets[1]] {
                let _ = overlay.show(h);
            }
            self.shown = true;
        }
    }

    fn destroy(&self, overlay: &vr::Overlay) {
        for h in [self.mirror, self.targets[0], self.targets[1]] {
            let _ = overlay.destroy(h);
        }
    }
}

/// The overlay's sounds: "Reveal" as Orbiters appears in SteamVR (this process starts when SteamVR does, or with the
/// app while SteamVR runs), "Rocker" on a press. Computed on a thread at start (about a third of a second).
struct Sounds {
    output: orbiters_sound::Output,
    click: Arc<std::sync::OnceLock<Arc<orbiters_sound::Sound>>>,
}

impl Sounds {
    fn start() -> Self {
        let output = orbiters_sound::Output::new();
        let click = Arc::new(std::sync::OnceLock::new());
        let (out, slot) = (output.clone(), click.clone());
        let _ = std::thread::Builder::new().name("overlay-sounds".into()).spawn(move || {
            use orbiters_sound::{render, sounds, RATE};
            out.play(&Arc::new(render(&sounds::REVEAL, RATE)));
            let _ = slot.set(Arc::new(render(&sounds::ROCKER, RATE)));
        });
        Self { output, click }
    }

    fn click(&self) {
        if let Some(s) = self.click.get() {
            self.output.play(s);
        }
    }
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let endpoint = Endpoint::from_args(args).context("--overlay needs --service <port> <token>")?;
    let client = Client::connect(&endpoint, Role::Overlay)?;
    let backend: Ctl = Arc::new(Remote(client.clone()));
    // The service starts this process only while SteamVR runs; as an overlay application it would start SteamVR.
    let steamvr = vr::Client::connect(vr::AppType::Overlay).map_err(|e| anyhow::anyhow!("{e}"))?;
    let overlay = steamvr.overlay().context("SteamVR has no overlay interface")?;
    let fail = |what: &str, code: i32| anyhow::anyhow!("{what}: SteamVR overlay error {code}");
    let (main, thumb) = overlay.create_dashboard(KEY, "Orbiters").map_err(|c| fail("dashboard tab", c))?;
    overlay.set_width_in_meters(main, 2.0).map_err(|c| fail("width", c))?;
    overlay.set_mouse_input(main).map_err(|c| fail("input", c))?;
    overlay.set_mouse_scale(main, WIDTH as f32, HEIGHT as f32).map_err(|c| fail("mouse scale", c))?;
    let _ = overlay.set_flag(main, vr::overlay_flag::ENABLE_CLICK_STABILIZATION, true);

    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let sounds = Sounds::start();
    let mut painter = painter();
    let mut presenter = Presenter::new(&steamvr);
    if let Some(icon) = thumbnail() {
        presenter.show(&overlay, thumb, &icon, 128, 128);
    }
    let mut state = ui::OverlayUi::default();
    let mut world = World::new(&overlay)?;
    let mut walk = walk::WalkBoost::new(&steamvr);
    state.walk_unavailable = walk.unavailable().map(str::to_string);
    let mut frames = frames::FrameSampler::new(&steamvr);

    let mut poses = SteamVrPoses { client: &steamvr, described: HashMap::new() };
    let started = Instant::now();
    let mut events: Vec<Event> = Vec::new();
    let mut shown = None::<Arc<crate::state::Snapshot>>;
    let mut next_frame: Option<Instant> = Some(Instant::now());
    loop {
        if client.should_close() {
            break;
        }
        while let Some(e) = steamvr.poll_event() {
            if e.event_type == vr::event::QUIT {
                walk.reset(&steamvr);
                steamvr.acknowledge_quit();
                return Ok(());
            }
        }
        // Laser-pointer input: SteamVR's mouse coordinates start at the bottom left.
        while let Some(e) = overlay.poll_event(main) {
            let at = |x: f32, y: f32| pos2(x, HEIGHT as f32 - y);
            match e.event_type {
                vr::event::MOUSE_MOVE => {
                    let (x, y, _) = e.mouse();
                    events.push(Event::PointerMoved(at(x, y)));
                }
                vr::event::MOUSE_BUTTON_DOWN | vr::event::MOUSE_BUTTON_UP => {
                    let (x, y, button) = e.mouse();
                    let button = match button {
                        2 => PointerButton::Secondary,
                        4 => PointerButton::Middle,
                        _ => PointerButton::Primary,
                    };
                    events.push(Event::PointerButton {
                        pos: at(x, y),
                        button,
                        pressed: e.event_type == vr::event::MOUSE_BUTTON_DOWN,
                        modifiers: Default::default(),
                    });
                }
                vr::event::FOCUS_LEAVE => events.push(Event::PointerGone),
                vr::event::OVERLAY_SHOWN => next_frame = Some(Instant::now()),
                _ => {}
            }
        }
        let visible = overlay.is_visible(main);
        let snap = backend.view();
        // The calibration runs whether the tab is drawn or not (the dashboard is closed while the pose is held).
        let now = started.elapsed().as_secs_f64();
        state.tick(now, overlay.is_dashboard_visible(), &mut poses, &backend);
        world.update(&overlay, &mut presenter, &state.calibrator, now);
        // VRChat has the scene (not a loading screen): its frames count, and walking may be boosted.
        let compositor = steamvr.compositor().ok();
        let vrchat_scene = compositor.as_ref().is_some_and(|c| snap.vrchat.pid.is_some_and(|pid| c.scene_focus_process() == pid) && !c.is_scene_loading())
            && snap.vrchat.world.in_world;
        if snap.settings.world_performance {
            if let Some(chunk) = frames.update(&steamvr, vrchat_scene) {
                backend.frames(chunk);
            }
        }
        let place = snap.vrchat.running.then(|| snap.vrchat.world.instance.clone()).flatten();
        let moment =
            walk::Moment { settings: &snap.settings.walk_boost, walking_allowed: vrchat_scene && !overlay.is_dashboard_visible(), place: place.as_deref() };
        walk.update(&steamvr, &moment);
        let walking = walk.active(&moment);
        if !shown.as_ref().is_some_and(|s| Arc::ptr_eq(s, &snap)) {
            shown = Some(snap.clone());
            next_frame = Some(next_frame.map_or(Instant::now(), |t| t.min(Instant::now())));
        }
        if !events.is_empty() {
            next_frame = Some(Instant::now());
        }
        if visible && next_frame.is_some_and(|t| t <= Instant::now()) {
            orbiters_ui::anim::set_live(&ctx, !snap.settings.reduce_motion);
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), ui::SIZE)),
                time: Some(started.elapsed().as_secs_f64()),
                events: std::mem::take(&mut events),
                ..Default::default()
            };
            input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() });
            let mut full = ctx.run_ui(input, |ui| state.show(ui, &snap, &backend, &mut poses));
            if orbiters_ui::widgets::take_press(&ctx) {
                sounds.click();
            }
            painter.set_textures(&mut full.textures_delta);
            let primitives = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
            let rgba = painter.paint(&primitives, full.pixels_per_point, orbiters_ui::theme::INK_950);
            presenter.show(&overlay, main, rgba, WIDTH, HEIGHT);
            painter.free_textures(&mut full.textures_delta);
            // egui says when the next frame is due (animations, springs); at most 45 frames a second.
            let delay = full.viewport_output.get(&ViewportId::ROOT).map(|v| v.repaint_delay).unwrap_or(Duration::MAX);
            next_frame = (delay < Duration::from_secs(3600) || state.busy()).then(|| Instant::now() + delay.max(Duration::from_millis(22)));
        } else if !visible {
            events.clear();
        }
        let fast = visible || walking || state.calibrator.in_world(now) || state.busy();
        std::thread::sleep(if fast { Duration::from_millis(11) } else { Duration::from_millis(100) });
    }
    walk.reset(&steamvr);
    world.destroy(&overlay);
    let _ = overlay.destroy(main);
    Ok(())
}

/// A CPU painter for the tab with its backdrop painted once (it never changes).
pub fn painter() -> Software {
    let ctx = egui::Context::default();
    let mut sw = Software::new(WIDTH, HEIGHT);
    let mut full = ctx.run_ui(RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), ui::SIZE)), ..Default::default() }, |ui| ui::backdrop(ui));
    sw.set_textures(&mut full.textures_delta);
    let primitives = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
    sw.paint(&primitives, full.pixels_per_point, orbiters_ui::theme::INK_950);
    sw.keep_as_backdrop();
    sw.free_textures(&mut full.textures_delta);
    sw
}

/// The dashboard button's picture: the Orbiters mark on ink, 128 × 128 (its own context: the tab's keeps its state).
fn thumbnail() -> Option<Vec<u8>> {
    let ctx = &egui::Context::default();
    orbiters_ui::install(ctx);
    let mut sw = Software::new(128, 128);
    let mut input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(128.0, 128.0))), ..Default::default() };
    input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), ..Default::default() });
    // The mark's textures load on the first frame; the second one paints them.
    let mut out = None;
    for _ in 0..2 {
        let mut full = ctx.run_ui(input.clone(), |ui| {
            let size = orbiters_ui::logo::size_for_width(96.0);
            orbiters_ui::logo::paint(ui.painter(), Rect::from_center_size(pos2(64.0, 64.0), size), orbiters_ui::logo::Look::default());
        });
        sw.set_textures(&mut full.textures_delta);
        let primitives = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
        out = Some(sw.paint(&primitives, full.pixels_per_point, orbiters_ui::theme::INK_900).to_vec());
        sw.free_textures(&mut full.textures_delta);
    }
    out
}
