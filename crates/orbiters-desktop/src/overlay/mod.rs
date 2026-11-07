//! The SteamVR overlay process (`orbiters-desktop --overlay --service <port> <token>`): Orbiters as a tab in
//! SteamVR's dashboard, started by the tray service while SteamVR runs. Its egui interface is painted on the CPU
//! ([`orbiters_ui::software`], only what changed) and handed to SteamVR as D3D11 textures of a small device on the
//! headset's GPU (raw pixels without one), only while the dashboard shows the tab and only the part that changed. It
//! reads the device poses itself for T-pose calibrations, which also put a mirror and two hand targets in the play
//! space, and runs walk boost and N4N0's menu in the play space. The same interface also floats in the play space
//! beside VRChat, opened by a watch on a wrist and used with the controllers' own lasers ([`floating`]): the dashboard
//! takes the controllers from VRChat, the floating panel does not.
//!
//! It never starts SteamVR: it connects as an overlay application (the kind SteamVR starts for) only after a
//! background connection, which SteamVR refuses when it is not running, found it running; and it ends when SteamVR
//! quits or its server is gone, with [`EXIT_STEAMVR_GONE`].

mod actions;
pub mod calibration;
pub mod coverage;
#[cfg(windows)]
pub mod eye_picture;
pub mod floating;
mod frames;
mod menu;
pub mod menu_pages;
pub mod menu_retro;
pub mod menu_ui;
pub mod orbit;
pub mod ui;
pub mod walk;
pub mod worlds;

use crate::backend::{Ctl, Remote};
use crate::ipc::{Client, Endpoint, N4n0Command, Role};
use anyhow::Context as _;
use calibration::{MIRROR_PX, MIRROR_WIDTH_M, TARGET_PX, TARGET_WIDTH_M};
use egui::{pos2, Event, PointerButton, RawInput, Rect, ViewportId, ViewportInfo};
use orbiters_contracts::performance::tracking::{Device, DeviceClass, Sample};
use orbiters_openvr::{self as vr, overlay_flag, Pose, MAX_DEVICES};
use orbiters_ui::software::Software;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Pixels of the dashboard tab (the interface is laid out in points at 1 pixel per point).
const WIDTH: u32 = ui::SIZE.x as u32;
const HEIGHT: u32 = ui::SIZE.y as u32;
const KEY: &str = "cc.orbiters.desktop";

/// How an overlay that ended because SteamVR quit or is not running exits. The service should start no other one
/// for the same SteamVR: a new one would find it gone, or going. ("ST": no runtime exits with it, unlike 3, which is
/// the C runtime's `abort`.)
pub const EXIT_STEAMVR_GONE: i32 = 0x5354;
/// SteamVR's server can end without a quit event (a crash): once the service has not seen it this long, the overlay
/// ends too, and the next SteamVR gets a new one.
const STEAMVR_GONE_AFTER: Duration = Duration::from_secs(5);
/// The tab's pictures: at most 45 a second, whatever asks for them (the laser moving, the service's news).
const MIN_FRAME: Duration = Duration::from_millis(22);
/// A turn of the loop while nothing moves the play space: every turn reads the controllers, as a binding that opens
/// N4N0's menu (a double press) is true for a moment only and SteamVR keeps no press between two reads, and the taps
/// that open it by default are timed from these reads.
const TURN: Duration = Duration::from_millis(11);
/// A turn waiting on the compositor is at least this long, should the wait come back at once.
const SHORTEST_TURN: Duration = Duration::from_millis(3);
/// After the D3D11 device was lost (a driver reset), raw pixels this long, then a new device. After a refusal (a
/// texture SteamVR would not take, a device that could not be made) twice as long each time in a row.
#[cfg(windows)]
const DEVICE_RETRY: Duration = Duration::from_secs(5);

/// How long raw pixels last after the `refusals`-th refusal in a row: 5 s, 10 s, 20 s… up to about ten minutes.
#[cfg(windows)]
fn after_refusals(refusals: u32) -> Duration {
    DEVICE_RETRY * 2u32.pow(refusals.saturating_sub(1).min(7))
}

/// SteamVR's device poses for calibrating, each device described when first seen (again after it changed).
struct SteamVrPoses<'a> {
    client: &'a vr::Client,
    described: HashMap<u32, Option<Device>>,
}

impl SteamVrPoses<'_> {
    /// A device came, went or changed (its role, a property): it is described again when next read. A role can
    /// pass from one device to another, so a role change has every device described again.
    fn forget(&mut self, e: &vr::Event) {
        if e.event_type == vr::event::TRACKED_DEVICE_ROLE_CHANGED {
            self.described.clear();
        } else {
            self.described.remove(&e.device);
        }
    }
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

/// Hands pictures to SteamVR: through D3D11 textures (raw pixels flicker when they change every frame), uploading
/// only what changed, or as raw pixels while no device works. A lost device (a driver reset) is replaced a moment
/// later; after refusals the wait doubles each time, so a SteamVR that keeps refusing is not asked every few seconds.
struct Presenter {
    #[cfg(windows)]
    gpu: Option<vr::gpu::OverlayTextures>,
    /// The headset's GPU, for a new device.
    #[cfg(windows)]
    adapter: Option<u32>,
    /// When to make a device again, while there is none.
    #[cfg(windows)]
    retry: Option<Instant>,
    /// Refusals in a row (a texture SteamVR would not take, a device that could not be made).
    #[cfg(windows)]
    refusals: u32,
}

impl Presenter {
    fn new(steamvr: &vr::Client) -> Self {
        #[cfg(windows)]
        {
            let mut presenter = Self { gpu: None, adapter: steamvr.dxgi_adapter_index(), retry: Some(Instant::now()), refusals: 0 };
            presenter.device();
            presenter
        }
        #[cfg(not(windows))]
        {
            let _ = steamvr;
            Self {}
        }
    }

    /// The device, made again once the time for it came.
    #[cfg(windows)]
    fn device(&mut self) -> Option<&mut vr::gpu::OverlayTextures> {
        if self.gpu.is_none() && self.retry.is_some_and(|t| t <= Instant::now()) {
            match vr::gpu::OverlayTextures::new(self.adapter) {
                Ok(gpu) => {
                    self.gpu = Some(gpu);
                    self.retry = None;
                }
                Err(e) => {
                    eprintln!("overlay textures: {e}; raw pixels for now");
                    self.refusals += 1;
                    self.retry = Some(Instant::now() + after_refusals(self.refusals));
                }
            }
        }
        self.gpu.as_mut()
    }

    /// Shows a whole picture on `handle`.
    fn show(&mut self, overlay: &vr::Overlay, handle: vr::OverlayHandle, rgba: &[u8], width: u32, height: u32) {
        self.show_part(overlay, handle, rgba, width, height, [0, 0, width, height]);
    }

    /// Shows a picture on `handle` that differs from the last one shown there only in `changed` (x0, y0, x1, y1).
    fn show_part(&mut self, overlay: &vr::Overlay, handle: vr::OverlayHandle, rgba: &[u8], width: u32, height: u32, changed: [u32; 4]) {
        #[cfg(windows)]
        if let Some(gpu) = self.device() {
            let failed = match gpu.present_part(overlay, handle, rgba, width, height, changed) {
                Ok(()) => {
                    self.refusals = 0;
                    return;
                }
                Err(vr::gpu::PresentError::DeviceLost(e)) => (e, DEVICE_RETRY),
                Err(vr::gpu::PresentError::Refused(e)) => {
                    self.refusals += 1;
                    (e, after_refusals(self.refusals))
                }
            };
            eprintln!("{}; raw pixels for {} s", failed.0, failed.1.as_secs());
            self.gpu = None;
            self.retry = Some(Instant::now() + failed.1);
        }
        #[cfg(not(windows))]
        let _ = changed;
        if let Err(code) = overlay.set_raw(handle, rgba, width, height) {
            eprintln!("overlay pixels: SteamVR overlay error {code}");
        }
    }
}

/// An egui canvas painted on the CPU for one of the world overlays.
pub struct Canvas {
    ctx: egui::Context,
    painter: Software,
    /// In points.
    size: egui::Vec2,
    /// Pixels a point.
    zoom: f32,
    /// What its frames go through last: the look chosen for the menus around VRChat's. `restyle_off` leaves a page that has this drawing already.
    restyle: Option<orbit::retro::Pass>,
    restyle_off: bool,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        Self { ctx, painter: Software::new(width, height), size: egui::vec2(width as f32, height as f32), zoom: 1.0, restyle: None, restyle_off: false }
    }

    /// One of `width` x `height` pixels drawn at `zoom` pixels a point (a panel that scales with what it stands beside: text
    /// stays crisp).
    pub fn zoomed(width: u32, height: u32, zoom: f32) -> Self {
        Self { size: egui::vec2(width as f32, height as f32) / zoom, zoom, ..Self::new(width, height) }
    }

    /// One whose frames take the look chosen for the menus around VRChat's.
    pub fn in_orbit_look(mut self) -> Self {
        self.restyle = Some(orbit::retro::pass);
        self
    }

    /// Whether the look's drawing stays off: a page of N4N0's that draws it itself (its pictures go on as they were).
    pub fn set_restyle_off(&mut self, off: bool) {
        if self.restyle_off != off {
            self.restyle_off = off;
            self.painter.invalidate();
        }
    }

    /// How its pictures are drawn now.
    fn pictures_now(&mut self) {
        if self.restyle.is_some() {
            self.painter.set_picture_look(orbit::panels::look::picture_look());
        }
    }

    /// The canvas's egui context (textures it paints are loaded through it).
    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// Paints a frame (RGBA, premultiplied, transparent where nothing was drawn): only what changed is painted again,
    /// and [`Canvas::damage`] says where.
    pub fn paint(&mut self, time: f64, draw: impl FnMut(&egui::Painter)) -> &[u8] {
        let mut draw = draw;
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), self.size)), time: Some(time), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(self.zoom), ..Default::default() });
        let (restyle, size) = (self.restyle.filter(|_| !self.restyle_off), self.size);
        let mut full = self.ctx.run_ui(input, |ui| {
            draw(ui.painter());
            if let Some(pass) = restyle {
                pass(ui.ctx(), ui.layer_id(), size);
            }
        });
        orbiters_ui::text_audit::record(&full.shapes);
        self.pictures_now();
        self.painter.set_textures(&mut full.textures_delta);
        let primitives = self.ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
        // Textures egui let go of are freed once painted with (the frame may still draw them).
        self.painter.paint(&primitives, full.pixels_per_point, egui::Color32::TRANSPARENT);
        self.painter.free_textures(&mut full.textures_delta);
        self.painter.pixels()
    }

    /// The same with a whole `Ui` to draw in (widgets that lay out or animate).
    pub fn paint_ui(&mut self, time: f64, draw: impl FnMut(&mut egui::Ui)) -> &[u8] {
        let mut draw = draw;
        let mut input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), self.size)), time: Some(time), ..Default::default() };
        input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(self.zoom), ..Default::default() });
        let (restyle, size) = (self.restyle.filter(|_| !self.restyle_off), self.size);
        let mut full = self.ctx.run_ui(input, |ui| {
            draw(ui);
            if let Some(pass) = restyle {
                pass(ui.ctx(), ui.layer_id(), size);
            }
        });
        orbiters_ui::text_audit::record(&full.shapes);
        self.pictures_now();
        self.painter.set_textures(&mut full.textures_delta);
        let primitives = self.ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
        self.painter.paint(&primitives, full.pixels_per_point, egui::Color32::TRANSPARENT);
        self.painter.free_textures(&mut full.textures_delta);
        self.painter.pixels()
    }

    /// The last frame (RGBA, premultiplied).
    pub fn pixels(&self) -> &[u8] {
        self.painter.pixels()
    }

    /// What the last frame changed (x0, y0, x1, y1), `None` when it is the frame before again.
    pub fn damage(&self) -> Option<[u32; 4]> {
        self.painter.damage()
    }

    /// The next frame is painted, and counted as changed, whole.
    pub fn invalidate(&mut self) {
        self.painter.invalidate();
    }
}

/// The calibration's mirror and hand targets in the play space.
struct World {
    mirror: vr::OverlayHandle,
    targets: [vr::OverlayHandle; 2],
    mirror_canvas: Canvas,
    /// One canvas per target: what changed is counted from each one's own last picture.
    target_canvases: [Canvas; 2],
    /// The frame they were placed for (placed again when a new calibration starts).
    placed: Option<[f32; 3]>,
    shown: bool,
    last_paint: Option<Instant>,
}

impl World {
    fn new(overlay: &vr::Overlay) -> anyhow::Result<Self> {
        let fail = |what: &str, code: i32| anyhow::anyhow!("{what}: SteamVR overlay error {code}");
        let mirror = overlay.create(&format!("{KEY}.mirror"), "Orbiters calibration mirror").map_err(|c| fail("mirror", c))?; // i18n: not shown (a name for SteamVR)
        let _ = overlay.set_width_in_meters(mirror, MIRROR_WIDTH_M);
        let mut targets = [0; 2];
        for (i, side) in ["left", "right"].into_iter().enumerate() {
            targets[i] = overlay.create(&format!("{KEY}.target.{side}"), &format!("Orbiters {side} hand target")).map_err(|c| fail("hand target", c))?; // i18n: not shown (a name for SteamVR)
            let _ = overlay.set_width_in_meters(targets[i], TARGET_WIDTH_M);
        }
        // Painted premultiplied, as the menu: without the flag the glow shows dim and edges get dark fringes.
        for h in [mirror, targets[0], targets[1]] {
            let _ = overlay.set_flag(h, overlay_flag::IS_PREMULTIPLIED, true);
        }
        Ok(Self {
            mirror,
            targets,
            mirror_canvas: Canvas::new(MIRROR_PX[0], MIRROR_PX[1]),
            target_canvases: [Canvas::new(TARGET_PX, TARGET_PX), Canvas::new(TARGET_PX, TARGET_PX)],
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
        if !self.shown {
            // Shown again for a new calibration: SteamVR gets whole pictures first.
            self.mirror_canvas.invalidate();
            self.target_canvases.iter_mut().for_each(Canvas::invalidate);
        }
        self.mirror_canvas.paint(now, |p| calibration::paint_mirror(p, cal, now));
        if let Some(changed) = self.mirror_canvas.damage() {
            presenter.show_part(overlay, self.mirror, self.mirror_canvas.pixels(), MIRROR_PX[0], MIRROR_PX[1], changed);
        }
        for (i, label) in ["L", "R"].into_iter().enumerate() {
            let on = cal.hand_on_target(i);
            let canvas = &mut self.target_canvases[i];
            canvas.paint(now, |p| calibration::paint_target(p, label, on, now));
            if let Some(changed) = canvas.damage() {
                presenter.show_part(overlay, self.targets[i], canvas.pixels(), TARGET_PX, TARGET_PX, changed);
            }
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
/// app while SteamVR runs), "Rocker" on a press. Computed on a thread at start, the short press sound first, so that a
/// press right away already has it.
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
            let _ = slot.set(Arc::new(render(&sounds::ROCKER, RATE)));
            out.play(&Arc::new(render(&sounds::REVEAL, RATE)));
        });
        Self { output, click }
    }

    fn click(&self) {
        if let Some(s) = self.click.get() {
            self.output.play(s);
        }
    }
}

/// The laser let go of the tab without its button's end (it left the tab, the dashboard closed). egui keeps a button
/// down until it hears it go up, and would press Talk or drag a slider again when the tab shows next: it hears the
/// pointer move off the tab (a press that moved that far is no click), the button go up there (which ends a drag
/// where it stood), and the pointer go.
fn let_go(events: &mut Vec<Event>) {
    let away = pos2(-100.0, -100.0);
    events.push(Event::PointerMoved(away));
    events.push(Event::PointerButton { pos: away, button: PointerButton::Primary, pressed: false, modifiers: Default::default() });
    events.push(Event::PointerGone);
}

/// A turn with the tab hidden: nothing is painted and the laser's events go, but a let-go waits for the tab's next
/// frame (once).
fn while_hidden(events: &mut Vec<Event>) {
    let waits = events.iter().any(|e| matches!(e, Event::PointerButton { pressed: false, .. } | Event::PointerGone));
    events.clear();
    if waits {
        let_go(events);
    }
}

/// Why the overlay's session ended.
enum Ended {
    /// The service closed it (the setting was turned off, the app quits) or went away.
    Closed,
    /// SteamVR quit, is quitting, or its server is gone.
    SteamVrGone,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    match session(args)? {
        Ended::Closed => Ok(()),
        // Everything was put back as the session ended: only the exit code is left to say.
        Ended::SteamVrGone => std::process::exit(EXIT_STEAMVR_GONE),
    }
}

/// SteamVR, never started for it. An overlay application's connection starts SteamVR when it is not running (right
/// after the person quit it, while the service still sees its server go), a background one is refused instead: so a
/// background connection looks first, and the overlay connects as what it is only once that found SteamVR running and
/// not quitting. `None`: SteamVR is not running, or is shutting down.
fn connect_steamvr() -> anyhow::Result<Option<vr::Client>> {
    match vr::Client::connect(vr::AppType::Background) {
        Ok(probe) => {
            let quitting = std::iter::from_fn(|| probe.poll_event()).any(|e| e.event_type == vr::event::QUIT);
            if quitting {
                probe.acknowledge_quit();
                return Ok(None);
            }
        }
        Err(e) if e.not_running() => return Ok(None),
        Err(e) => return Err(anyhow::anyhow!("{e}")),
    }
    vr::Client::connect(vr::AppType::Overlay).map(Some).map_err(|e| anyhow::anyhow!("{e}"))
}

/// SteamVR's server process, watched for its end: it can end without a quit event (a crash), and the service, which
/// looks at the processes every few seconds, may not see it go when SteamVR comes straight back.
struct Server {
    #[cfg(windows)]
    process: Option<windows::Win32::Foundation::HANDLE>,
}

impl Server {
    /// The `vrserver` running now (none found: only the service's view of it counts).
    fn find() -> Self {
        #[cfg(windows)]
        {
            use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
            let mut system = sysinfo::System::new();
            system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
            let pid = system.processes().values().find(|p| p.name().eq_ignore_ascii_case("vrserver.exe")).map(|p| p.pid().as_u32());
            // SAFETY: a handle that can only be waited on, closed when dropped.
            let process = pid.and_then(|pid| unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.ok());
            Self { process }
        }
        #[cfg(not(windows))]
        {
            Self {}
        }
    }

    /// The server ended.
    fn ended(&self) -> bool {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::WAIT_OBJECT_0;
            use windows::Win32::System::Threading::WaitForSingleObject;
            // SAFETY: a live handle, closed only when dropped.
            self.process.is_some_and(|h| unsafe { WaitForSingleObject(h, 0) } == WAIT_OBJECT_0)
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

#[cfg(windows)]
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(h) = self.process.take() {
            // SAFETY: opened by `find`, closed once.
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(h);
            }
        }
    }
}

/// What the overlay changed in SteamVR and told the service, put back however the session ends (a quit, a close,
/// SteamVR gone, an error, a panic unwinding): a held Talk let go, N4N0's menu said closed, the play space back where
/// it was, the overlays taken down. (A process killed outside leaves no moved play space: SteamVR goes back to the
/// saved chaperone once the process previewing one is gone.)
struct Teardown<'a> {
    steamvr: &'a vr::Client,
    overlay: &'a vr::Overlay<'a>,
    backend: &'a Ctl,
    main: vr::OverlayHandle,
    state: ui::OverlayUi,
    walk: walk::WalkBoost,
    world: World,
    menu: menu::MenuOverlay,
    coverage: coverage::CoverageOverlayPanel,
    /// The floating panel, the watch and their lasers (none when SteamVR would not make their overlays).
    floating: Option<floating::Floating>,
    /// The panels around VRChat's menus (none when SteamVR would not make their overlays).
    orbit: Option<orbit::vr::OrbitVr>,
}

impl Drop for Teardown<'_> {
    fn drop(&mut self) {
        self.state.release(self.backend);
        // Always: the menu may have closed on this turn's effects after its state was reported, unheard yet.
        self.backend.n4n0(N4n0Command::MenuState { open: false, dictation: false, keyboard: false });
        self.walk.reset(self.steamvr);
        self.world.destroy(self.overlay);
        self.menu.destroy(self.overlay);
        self.coverage.destroy(self.overlay);
        if let Some(f) = &self.floating {
            f.destroy(self.overlay);
        }
        if let Some(o) = &self.orbit {
            o.destroy(self.overlay);
        }
        let _ = self.overlay.destroy(self.main);
    }
}

fn session(args: &[String]) -> anyhow::Result<Ended> {
    let endpoint = Endpoint::from_args(args).context("--overlay needs --service <port> <token>")?;
    // SteamVR first: an overlay that finds it gone leaves before the app ever counts it connected.
    let Some(steamvr) = connect_steamvr()? else { return Ok(Ended::SteamVrGone) };
    let server = Server::find();
    let client = Client::connect(&endpoint, Role::Overlay)?;
    let backend: Ctl = Arc::new(Remote(client.clone()));
    let overlay = steamvr.overlay().context("SteamVR has no overlay interface")?;
    let fail = |what: &str, code: i32| anyhow::anyhow!("{what}: SteamVR overlay error {code}");
    let (main, thumb) = overlay.create_dashboard(KEY, "Orbiters").map_err(|c| fail("dashboard tab", c))?; // i18n: not shown (an error text)
    overlay.set_width_in_meters(main, 2.0).map_err(|c| fail("width", c))?;
    overlay.set_mouse_input(main).map_err(|c| fail("input", c))?;
    overlay.set_mouse_scale(main, WIDTH as f32, HEIGHT as f32).map_err(|c| fail("mouse scale", c))?; // i18n: not shown (an error text)
    let _ = overlay.set_flag(main, vr::overlay_flag::ENABLE_CLICK_STABILIZATION, true);

    let ctx = egui::Context::default();
    orbiters_ui::install(&ctx);
    let sounds = Sounds::start();
    let mut painter = painter();
    let mut presenter = Presenter::new(&steamvr);
    if let Some(icon) = thumbnail() {
        presenter.show(&overlay, thumb, &icon, 128, 128);
    }
    let mut actions = actions::Actions::load(&steamvr);
    let mut walk = walk::WalkBoost::new(actions.walk, actions.failed.clone());
    walk.set_frame(steamvr.float_property(0, vr::prop::DISPLAY_FREQUENCY).filter(|hz| *hz > 1.0).map(|hz| Duration::from_secs_f32(1.0 / hz)));
    let mut devices = crate::tracking::DeviceKindsWatch::default();
    let mut state = ui::OverlayUi::default();
    state.walk_unavailable = walk.unavailable().map(str::to_string);
    let world = World::new(&overlay)?;
    let menu = menu::MenuOverlay::new(&overlay, KEY)?;
    let coverage = coverage::CoverageOverlayPanel::new(&overlay)?;
    // The floating panel is an addition: without its overlays the tab and the rest work as before.
    let floating = floating::Floating::new(&overlay, KEY).map_err(|e| eprintln!("floating panel unavailable: {e}")).ok();
    let orbit = orbit::vr::OrbitVr::new(&overlay, KEY, backend.clone()).map_err(|e| eprintln!("panels around VRChat's menus unavailable: {e}")).ok();
    let mut t = Teardown { steamvr: &steamvr, overlay: &overlay, backend: &backend, main, state, walk, world, menu, coverage, floating, orbit };
    let mut frames = frames::FrameSampler::new(&steamvr);
    // What of the touchpads and sticks goes to the avatar's input parameters.
    let mut pads = orbiters_n4n0::input::Pacer::default();

    let mut poses = SteamVrPoses { client: &steamvr, described: HashMap::new() };
    let started = Instant::now();
    let mut events: Vec<Event> = Vec::new();
    let mut shown = None::<Arc<crate::state::Snapshot>>;
    let mut next_frame: Option<Instant> = Some(Instant::now());
    let mut last_frame: Option<Instant> = None;
    // Whether the tab showed last turn.
    let mut was_visible = false;
    // When the service last saw SteamVR's server; VRChat's part of the compositor, read ten times a second.
    let mut steamvr_seen = Instant::now();
    let mut vrchat_scene = false;
    let mut vrchat_read: Option<Instant> = None;
    // Whether someone wears the headset, read every second and told when it changes.
    let mut worn: Option<bool> = None;
    let mut worn_read: Option<Instant> = None;
    // A frame of the headset, the longest wait for the compositor's next one (its advice).
    let frame_ms = steamvr.float_property(0, vr::prop::DISPLAY_FREQUENCY).filter(|hz| *hz > 1.0).map_or(11, |hz| (1000.0 / hz).ceil() as u32).clamp(4, 50);
    loop {
        let turn = Instant::now();
        t.menu.worlds.view.input_turn(turn);
        if client.should_close() {
            return Ok(Ended::Closed);
        }
        while let Some(e) = steamvr.poll_event() {
            match e.event_type {
                vr::event::QUIT => {
                    // Everything put back, then SteamVR told this process is going.
                    drop(t);
                    steamvr.acknowledge_quit();
                    return Ok(Ended::SteamVrGone);
                }
                vr::event::TRACKED_DEVICE_ACTIVATED | vr::event::TRACKED_DEVICE_DEACTIVATED | vr::event::TRACKED_DEVICE_ROLE_CHANGED => {
                    poses.forget(&e);
                    devices.changed();
                }
                vr::event::PROPERTY_CHANGED => poses.forget(&e),
                _ => {
                    // A recenter, room setup, another universe: walk boost ends its carry; its own previews' echoes
                    // change nothing.
                    if let Some(event) = walk::PlaySpaceEvent::from_event(&e) {
                        t.walk.play_space_event(&steamvr, event);
                    }
                }
            }
        }
        // Which device is which, for tracking smoothing's strengths: the service asks SteamVR nothing itself while this
        // overlay runs (its connections, every few seconds, disturbed this one's input and play space).
        if let Some(kinds) = devices.due(Instant::now(), |i| steamvr.device_class(i)) {
            backend.steamvr_devices(kinds);
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
                vr::event::FOCUS_LEAVE => {
                    let_go(&mut events);
                    t.state.release(&backend);
                }
                vr::event::OVERLAY_SHOWN => {
                    // Shown again: a whole picture, should SteamVR have dropped the last one.
                    next_frame = Some(Instant::now());
                    painter.invalidate();
                }
                _ => {}
            }
        }
        let visible = overlay.is_visible(main);
        if was_visible && !visible {
            // The dashboard closed mid-press: the press's end went with it.
            let_go(&mut events);
            t.state.release(&backend);
        }
        was_visible = visible;
        let snap = backend.view();
        // The language the settings ask for, for the dashboard tab too, whether or not the menus around VRChat are on.
        crate::language::follow(&snap.settings);
        // SteamVR's server can end without a quit event: its process is watched here, and by the service.
        if snap.steamvr.running {
            steamvr_seen = Instant::now();
        }
        if server.ended() || steamvr_seen.elapsed() > STEAMVR_GONE_AFTER {
            return Ok(Ended::SteamVrGone);
        }
        // The calibration runs whether the tab is drawn or not (the dashboard is closed while the pose is held).
        let now = started.elapsed().as_secs_f64();
        let dashboard = overlay.is_dashboard_visible();
        t.state.tick(now, dashboard, &mut poses, &backend);
        t.world.update(&overlay, &mut presenter, &t.state.calibrator, now);
        // What the recording has covered, on a hand (never in VRChat's camera: overlay/coverage.rs).
        t.coverage.update(&overlay, &steamvr, &mut presenter, &snap, now);
        // VRChat has the scene (not a loading screen): its frames count, and walking may be boosted.
        if vrchat_read.is_none_or(|r| r.elapsed() >= Duration::from_millis(100)) {
            vrchat_read = Some(Instant::now());
            let compositor = steamvr.compositor().ok();
            vrchat_scene = compositor.is_some_and(|c| snap.vrchat.pid.is_some_and(|pid| c.scene_focus_process() == pid) && !c.is_scene_loading())
                && snap.vrchat.world.in_world;
        }
        if worn_read.is_none_or(|r| r.elapsed() >= Duration::from_secs(1)) {
            worn_read = Some(Instant::now());
            let activity = steamvr.activity(0);
            if activity != vr::Activity::Unknown && worn != Some(activity.in_use()) {
                worn = Some(activity.in_use());
                backend.headset(activity.in_use());
            }
        }
        // VRChat's frame times also pace N4N0's projected screen (the owner's steps follow the frame rate).
        if snap.settings.world_performance || snap.settings.n4n0.enabled {
            if let Some(chunk) = frames.update(&steamvr, vrchat_scene) {
                backend.frames(chunk);
            }
        }
        let place = snap.vrchat.running.then(|| snap.vrchat.world.instance.clone()).flatten();
        // Every action set at once, then what reads them; a failed read leaves the last frame's states, not to be
        // taken for new ones (a press counted twice, a stick still pushed).
        let input_read = actions.update(&steamvr);
        let moment = walk::Moment {
            settings: &snap.settings.walk_boost,
            walking_allowed: vrchat_scene && !dashboard,
            place: place.as_deref(),
            held: t.menu.host.menu.open || t.state.busy() || t.state.calibrator.in_world(now),
            away: snap.n4n0.afk,
            // VRChat's InStation, Seated and VelocityMagnitude tell whether it walks the avatar; without them (VRChat
            // not talking to Orbiters, or not lately) the stick alone says it.
            motion: snap
                .vrchat
                .motion
                .filter(|_| snap.vrchat.running && snap.osc.receiving)
                .map(|m| walk::Motion { in_station: m.in_station || m.seated, speed: m.speed }),
            input_read,
        };
        t.walk.update(&steamvr, &moment);
        // The triggers press where the index fingers point on N4N0's screen (N4N0 reads them only while its menu is open
        // there): each change goes, nothing else. The floating panel reads them held (read whether N4N0 is on or not).
        if input_read {
            let changes = actions.presses(&steamvr);
            if snap.settings.n4n0.enabled {
                for (hand, change) in changes.into_iter().enumerate() {
                    if let Some(down) = change {
                        backend.n4n0(N4n0Command::Trigger { right: hand == 1, down });
                    }
                }
            }
        }
        // The touchpads and sticks reach the avatar's N4N0 input parameters (contracts `Docs/n4n0-input.md`), paced; turned
        // off (or VRChat gone), they go back to rest once.
        let pads_wanted = snap.settings.n4n0.enabled && snap.settings.n4n0.controller_input && snap.vrchat.running;
        let paced = match (pads_wanted, input_read) {
            (true, true) => actions.pads(&steamvr).and_then(|read| pads.step(Instant::now(), read)),
            (true, false) => None,
            (false, _) => pads.stop(),
        };
        if let Some(state) = paced {
            backend.n4n0(N4n0Command::Input { pads: state });
        }
        let button = input_read && actions.menu_pressed(&steamvr);
        // The floating panel, the watch and N4N0's menu, used with the controllers' own lasers beside VRChat
        // (overlay/floating): first, so that the menu reads this turn's laser events, and the watch opens it now.
        let mut watch = false;
        if let Some(floating) = t.floating.as_mut() {
            let pointables = t.menu.pointables();
            let menu_open = t.menu.host.menu.open || (menu::on_avatar_screen(&snap) && snap.n4n0.screen_open);
            let cx = floating::Cx {
                overlay: &overlay,
                steamvr: &steamvr,
                presenter: &mut presenter,
                snap: &snap,
                backend: &backend,
                actions: &actions,
                input_read,
                dashboard,
                menu: &pointables,
                menu_open,
            };
            let turn = floating.update(cx, &mut t.state, &mut poses);
            if turn.clicked {
                sounds.click();
            }
            watch = turn.toggle_menu;
            if turn.menu_moved.is_some() || turn.menu_released {
                let mut head = [Pose::default(); MAX_DEVICES];
                steamvr.poses(vr::Universe::Standing, &mut head);
                let head = head[0].pose_is_valid.then_some(head[0].device_to_absolute);
                t.menu.carry(&overlay, turn.menu_moved, turn.menu_released, head.as_ref());
            }
        }
        // The panels around VRChat's menus, and whose trigger VRChat does not get from the next read on.
        if let Some(orbit) = t.orbit.as_mut() {
            let since = steamvr.time_since_vsync().unwrap_or(0.0);
            let frame = frame_ms as f32 / 1000.0;
            let cx = orbit::vr::Cx {
                overlay: &overlay,
                steamvr: &steamvr,
                presenter: &mut presenter,
                snap: &snap,
                backend: &backend,
                actions: &mut actions,
                input_read,
                dashboard,
                in_game: vrchat_scene,
                frame,
                world_status: t.menu.worlds.view_status(),
                prediction: (frame - since).clamp(0.0, 0.05),
            };
            let turn = orbit.update(cx);
            if turn.clicked {
                sounds.click();
            }
            t.menu.worlds.from_elsewhere(turn.worlds);
            t.menu.worlds.small_place(turn.small_place);
            if t.menu.worlds.small_showing() {
                let sticks = actions.pads(&steamvr).map_or([0.0; 2], |pads| [pads.sticks[0].y, pads.sticks[1].y]);
                let lasers = actions.lasers(&steamvr, 0.0);
                t.menu.worlds.small_input(lasers, actions.grabs(&steamvr), sticks);
                t.menu.worlds.small_tools(lasers, actions.orbit_triggers(&steamvr, [false; 2]));
            }
        }
        if t.menu.update(&overlay, &steamvr, &mut presenter, &snap, &backend, button, watch) {
            sounds.click();
        }
        t.menu.worlds.view.publish(&overlay);
        if !shown.as_ref().is_some_and(|s| Arc::ptr_eq(s, &snap)) {
            shown = Some(snap.clone());
            next_frame = Some(next_frame.map_or(Instant::now(), |f| f.min(Instant::now())));
        }
        if !events.is_empty() {
            next_frame = Some(Instant::now());
        }
        // At most 45 pictures a second, whatever asks for them.
        let due = next_frame.is_some_and(|f| f <= Instant::now()) && last_frame.is_none_or(|f| f.elapsed() >= MIN_FRAME);
        if visible && due {
            last_frame = Some(Instant::now());
            orbiters_ui::anim::set_live(&ctx, !snap.settings.reduce_motion);
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), ui::SIZE)),
                time: Some(started.elapsed().as_secs_f64()),
                events: std::mem::take(&mut events),
                ..Default::default()
            };
            input.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() });
            let mut full = ctx.run_ui(input, |ui| t.state.show(ui, &snap, &backend, &mut poses));
            if orbiters_ui::widgets::take_press(&ctx) {
                sounds.click();
            }
            painter.set_textures(&mut full.textures_delta);
            let primitives = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
            // Only what changed is painted, and only that part sent; a frame like the last one costs no upload.
            painter.paint(&primitives, full.pixels_per_point, orbiters_ui::theme::INK_950);
            if let Some(changed) = painter.damage() {
                presenter.show_part(&overlay, main, painter.pixels(), WIDTH, HEIGHT, changed);
            }
            painter.free_textures(&mut full.textures_delta);
            // egui says when the next frame is due (animations, springs; `Duration::MAX`: nothing to repaint). A
            // calibration under way shows its progress on whichever tab is up.
            let delay = full.viewport_output.get(&ViewportId::ROOT).map(|v| v.repaint_delay).unwrap_or(Duration::MAX);
            next_frame = if delay < Duration::from_secs(3600) {
                Some(Instant::now() + delay.max(MIN_FRAME))
            } else if t.state.busy() {
                Some(Instant::now() + Duration::from_millis(100))
            } else {
                None
            };
        } else if !visible {
            while_hidden(&mut events);
        }
        t.menu.worlds.view.publish(&overlay);
        if t.walk.moving() {
            // Walk boost moves the play space once per frame the compositor shows: unsynchronised steps judder.
            if overlay.wait_frame_sync(frame_ms).is_err() {
                std::thread::sleep(TURN.saturating_sub(turn.elapsed()));
            }
            std::thread::sleep(SHORTEST_TURN.saturating_sub(turn.elapsed()));
            t.menu.worlds.view.update(&overlay, &steamvr);
        } else {
            // Keep input at its existing cadence. Only displayed 3D content gets short, high-resolution service
            // slices: a completed texture need not wait through the rest of this turn. No tick-based channel waits.
            let end = turn + TURN;
            while Instant::now() < end {
                let now = Instant::now();
                let left = end.saturating_duration_since(now);
                std::thread::sleep(worlds::pacing::service_slice(left, t.menu.worlds.view.wake_after(now)));
                if t.menu.worlds.view.active() {
                    t.menu.worlds.view.update(&overlay, &steamvr);
                }
            }
        }
    }
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
            let size = orbiters_ui::n4n0_logo::size_for_width(88.0);
            orbiters_ui::n4n0_logo::paint(ui.painter(), Rect::from_center_size(pos2(64.0, 64.0), size), orbiters_ui::theme::MARK);
        });
        sw.set_textures(&mut full.textures_delta);
        let primitives = ctx.tessellate(std::mem::take(&mut full.shapes), full.pixels_per_point);
        out = Some(sw.paint(&primitives, full.pixels_per_point, orbiters_ui::theme::INK_900).to_vec());
        sw.free_textures(&mut full.textures_delta);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_cut_by_the_dashboard_closing_is_let_go() {
        let ctx = egui::Context::default();
        let (talk, slider) = (Rect::from_min_size(pos2(10.0, 10.0), egui::vec2(100.0, 100.0)), Rect::from_min_size(pos2(10.0, 140.0), egui::vec2(180.0, 20.0)));
        // What a hold button and a slider see after `events`: Talk held, Talk clicked, the slider following the laser
        // (as the toolkit's slider does: down on it or dragged, with a pointer).
        let frame = |events: Vec<Event>| {
            let input = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(200.0, 200.0))), events, ..Default::default() };
            let mut seen = (false, false, false);
            let mut full = ctx.run_ui(input, |ui| {
                let t = ui.interact(talk, egui::Id::unique("talk"), egui::Sense::click_and_drag());
                let s = ui.interact(slider, egui::Id::unique("slider"), egui::Sense::click_and_drag());
                seen = (t.is_pointer_button_down_on(), t.clicked(), (s.is_pointer_button_down_on() || s.dragged()) && s.interact_pointer_pos().is_some());
            });
            full.textures_delta.clear();
            seen
        };
        let press =
            |at| vec![Event::PointerMoved(at), Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() }];
        frame(Vec::new());
        assert_eq!(frame(press(pos2(50.0, 50.0))), (true, false, false), "the laser holds Talk");
        // Its events dropped while hidden, egui would still hold it when the tab shows again.
        assert_eq!(frame(Vec::new()), (true, false, false));
        // The dashboard closes: the let-go waits through hidden turns, once.
        let mut events = Vec::new();
        let_go(&mut events);
        while_hidden(&mut events);
        let_go(&mut events);
        events.push(Event::PointerMoved(pos2(5.0, 5.0)));
        while_hidden(&mut events);
        assert_eq!(events.len(), 3);
        // Shown again: let go, and not counted as a click.
        assert_eq!(frame(events), (false, false, false));
        assert_eq!(frame(Vec::new()), (false, false, false));
        // A slider dragged as the dashboard closes stops where it was, not at the let-go's place.
        assert!(frame(press(pos2(40.0, 150.0))).2);
        assert!(frame(vec![Event::PointerMoved(pos2(80.0, 150.0))]).2, "dragged");
        let mut events = Vec::new();
        let_go(&mut events);
        assert_eq!(frame(events), (false, false, false));
        // Nothing to let go of: hidden turns keep nothing.
        let mut events = vec![Event::PointerMoved(pos2(50.0, 50.0))];
        while_hidden(&mut events);
        assert!(events.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn refusals_wait_longer_each_time() {
        assert_eq!(after_refusals(1), DEVICE_RETRY);
        assert_eq!(after_refusals(2), DEVICE_RETRY * 2);
        assert_eq!(after_refusals(8), Duration::from_secs(640));
        assert_eq!(after_refusals(1000), Duration::from_secs(640), "about ten minutes at most");
    }
}
