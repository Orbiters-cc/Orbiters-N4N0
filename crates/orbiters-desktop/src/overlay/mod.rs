//! The SteamVR overlay process (`orbiters-desktop --overlay --service <port> <token>`): Orbiters as a tab in
//! SteamVR's dashboard, started by the tray service while SteamVR runs. Its egui interface is painted on the CPU
//! ([`orbiters_ui::software`]: no GPU device, so none of a graphics driver's ~100 MB per process) and handed to
//! SteamVR as pixels, only while the dashboard shows the tab and only when something changed. It reads the device
//! poses itself for T-pose calibrations.

pub mod ui;

use crate::backend::{Ctl, Remote};
use crate::ipc::{Client, Endpoint, Role};
use anyhow::Context as _;
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
    let mut painter = painter();
    if let Some(icon) = thumbnail() {
        let _ = overlay.set_raw(thumb, &icon, 128, 128);
    }
    let mut state = ui::OverlayUi::default();
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
            painter.set_textures(&mut full.textures_delta);
            let primitives = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
            let rgba = painter.paint(&primitives, full.pixels_per_point, orbiters_ui::theme::INK_950);
            if let Err(code) = overlay.set_raw(main, rgba, WIDTH, HEIGHT) {
                eprintln!("overlay pixels: SteamVR overlay error {code}");
            }
            painter.free_textures(&mut full.textures_delta);
            // egui says when the next frame is due (animations, springs); at most 45 frames a second.
            let delay = full.viewport_output.get(&ViewportId::ROOT).map(|v| v.repaint_delay).unwrap_or(Duration::MAX);
            next_frame = (delay < Duration::from_secs(3600) || state.busy()).then(|| Instant::now() + delay.max(Duration::from_millis(22)));
        } else if !visible {
            events.clear();
        }
        std::thread::sleep(if visible { Duration::from_millis(11) } else { Duration::from_millis(100) });
    }
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
