//! `--orbit-lab`: the Mac's overlay with no game, no service and no person, for `tools/mac-overlay/check.py`. Everything it does is in a private folder ([`crate::harness`]).
//!
//! - `frames <testdata> <out> <width> <height>` writes `wheel.bgra`, `qm.bgra` and `none.bgra`: pictures of that size with the real frames of VRChat's menus in their places.
//! - `desk <launcher's process>` turns the overlay's loop over the window of that process's window server, with a keyboard read from its standard input: `press esc|r|option|1..9`,
//!   `let go ...`, `button down|up`, `point <zone>` (the pointer over that part, while the keyboard is ours), `click`, `type <code>`, `say`, `cpu`, `quit`. Each change of what it
//!   does, and each `say`, is a line of JSON on its standard output.
//! - `picture <window> <process> <width> <height> <out.png>`: one look's picture of a window.

use super::super::zones::{self, Zone};
use super::keyboard::{self, Source};
use super::{panel, Desk, Next};
use crate::backend::Backend;
use crate::ipc::N4n0Command;
use crate::state::Snapshot;
use anyhow::{bail, Context as _};
use egui::{pos2, vec2, Pos2, Vec2};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::{NSPoint, NSString};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["frames", data, out, w, h] => frames(Path::new(data), Path::new(out), w.parse()?, h.parse()?),
        ["desk", launcher, ..] => desk(launcher.parse()?),
        _ => bail!("--orbit-lab frames <testdata> <out> <width> <height> | desk <process>"), // i18n: not shown (a developer's command)
    }
}

// --------------------------------------------------------------------------------------------- pictures

/// Which menu a picture shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shows {
    Wheel,
    QuickMenu,
    Nothing,
    /// The wheel, a little off its place and smaller: where the first-person view, the avatar's height or VRChat itself may have it ([`BESIDE`]).
    Beside,
}

/// How far off its place (in radii) and how large that wheel is.
pub const BESIDE: (Vec2, f32) = (Vec2::new(-0.06, 0.04), 0.93);

impl Shows {
    const ALL: [(Shows, &'static str); 4] = [(Shows::Wheel, "wheel"), (Shows::QuickMenu, "qm"), (Shows::Nothing, "none"), (Shows::Beside, "beside")];

    /// The real frames it is made of ([`super::super::detect`]'s own), by what they show: the wheel's part and the icons' row, of a 1280 x 720 window.
    fn fixtures(self) -> [&'static str; 2] {
        match self {
            Shows::Wheel | Shows::Beside => ["wheel-forest-1280x720", "wheel-forest-1280x720-row"],
            Shows::QuickMenu => ["qm-forest-1280x720", "qm-forest-1280x720-row"],
            Shows::Nothing => ["none-forest-1280x720", "none-forest-1280x720-row"],
        }
    }
}

/// A picture of top-down BGRA.
#[derive(Clone)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub bgra: Vec<u8>,
}

impl Picture {
    fn at(&self, x: f32, y: f32) -> [u8; 4] {
        // Between four pixels, as macOS stretches a picture.
        let (x, y) = (x.clamp(0.0, self.width as f32 - 1.001), y.clamp(0.0, self.height as f32 - 1.001));
        let (ix, iy, ax, ay) = (x as usize, y as usize, x.fract(), y.fract());
        let px = |x: usize, y: usize| {
            let i = (y.min(self.height - 1) * self.width + x.min(self.width - 1)) * 4;
            [0, 1, 2, 3].map(|c| f32::from(self.bgra[i + c]))
        };
        let (a, b, c, d) = (px(ix, iy), px(ix + 1, iy), px(ix, iy + 1), px(ix + 1, iy + 1));
        std::array::from_fn(|i| ((a[i] * (1.0 - ax) + b[i] * ax) * (1.0 - ay) + (c[i] * (1.0 - ax) + d[i] * ax) * ay).round() as u8)
    }

    /// Stretched to another size.
    pub fn stretched(&self, width: usize, height: usize) -> Self {
        let (kx, ky) = (self.width as f32 / width as f32, self.height as f32 / height as f32);
        let mut bgra = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                bgra.extend(self.at((x as f32 + 0.5) * kx - 0.5, (y as f32 + 0.5) * ky - 0.5));
            }
        }
        Self { width, height, bgra }
    }
}

/// A real frame's part: its picture, the window it is of, and where in it.
fn fixture(data: &Path, name: &str) -> anyhow::Result<(Picture, Vec2, Pos2)> {
    for entry in std::fs::read_dir(data)? {
        let path = entry?.path();
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let fields: Vec<&str> = stem.split('_').collect();
        if fields.len() != 4 || fields[0] != name {
            continue;
        }
        let size: Vec<f32> = fields[1].split('x').filter_map(|v| v.parse().ok()).collect();
        let image = image::open(&path)?.to_rgba8();
        let (w, h) = image.dimensions();
        let bgra = image.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
        return Ok((Picture { width: w as usize, height: h as usize, bgra }, vec2(size[0], size[1]), pos2(fields[2].parse()?, fields[3].parse()?)));
    }
    bail!("no fixture {name} in {}", data.display()) // i18n: not shown
}

/// A window's whole picture of `client` pixels with the menu `shows` drawn as VRChat draws it at that size, over a world's noise: the real frame's parts, each scaled and laid where
/// its menu is.
pub fn frame(data: &Path, shows: Shows, client: Vec2) -> anyhow::Result<Picture> {
    let (w, h) = (client.x as usize, client.y as usize);
    let mut bgra = vec![0u8; w * h * 4];
    // Slow hills of green and brown with a little grain: a world, with nothing of a menu in it.
    let hills = |n: usize, fast: f32| -> Vec<i32> { (0..n).map(|i| ((i as f32 / h as f32 * fast).sin() * 16.0) as i32).collect() };
    let (across, down) = (hills(w, 9.0), hills(h, 7.0));
    for (y, row) in bgra.chunks_exact_mut(w * 4).enumerate() {
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            let wave = across[x] + down[y] + ((x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(40_503)) >> 7 & 15) as i32;
            px.copy_from_slice(&[(50 + wave) as u8, (80 + wave) as u8, (66 + wave) as u8, 255]);
        }
    }
    let mut out = Picture { width: w, height: h, bgra };
    for (name, menu) in shows.fixtures().into_iter().zip([zones::Around::Wheel, zones::Around::QuickMenu]) {
        let (part, of, corner) = fixture(data, name)?;
        let (from, to) = (zones::frame(menu, of), zones::frame(menu, client));
        let (mut to, mut k) = (to, to.1 / from.1);
        if shows == Shows::Beside && menu == zones::Around::Wheel {
            to.0 += BESIDE.0 * zones::wheel(client).1;
            k *= BESIDE.1;
        }
        let at = to.0 + (corner - from.0) * k;
        let (pw, ph) = ((part.width as f32 * k).round() as usize, (part.height as f32 * k).round() as usize);
        let scaled = part.stretched(pw.max(1), ph.max(1));
        for y in 0..scaled.height {
            for x in 0..scaled.width {
                let (ox, oy) = (at.x.round() as i64 + x as i64, at.y.round() as i64 + y as i64);
                if ox < 0 || oy < 0 || ox >= w as i64 || oy >= h as i64 {
                    continue;
                }
                let (i, j) = ((oy as usize * w + ox as usize) * 4, (y * scaled.width + x) * 4);
                out.bgra[i..i + 4].copy_from_slice(&scaled.bgra[j..j + 4]);
            }
        }
    }
    Ok(out)
}

fn frames(data: &Path, out: &Path, w: u32, h: u32) -> anyhow::Result<()> {
    std::fs::create_dir_all(out)?;
    for (shows, name) in Shows::ALL {
        let picture = frame(data, shows, vec2(w as f32, h as f32))?;
        std::fs::write(out.join(format!("{name}.bgra")), &picture.bgra)?;
    }
    Ok(())
}

// --------------------------------------------------------------------------------------------- the loop

/// The service, as far as the overlay's loop asks: one state, and what it said back.
struct Stub {
    view: Mutex<Arc<Snapshot>>,
    said: Mutex<Vec<String>>,
}

impl Stub {
    fn say(&self, what: String) {
        self.said.lock().unwrap().push(what);
    }
}

impl Backend for Stub {
    fn view(&self) -> Arc<Snapshot> {
        self.view.lock().unwrap().clone()
    }
    fn orbit_status(&self, status: crate::state::OrbitStatus) {
        self.say(format!("status {:?} {:?}", status.needs, status.note)); // i18n: not shown
    }
    fn typed(&self, key: orbiters_n4n0_core::menu::KeyInput) {
        self.say(format!("typed {key:?}")); // i18n: not shown
    }
    fn n4n0(&self, command: N4n0Command) {
        self.say(format!("n4n0 {command:?}")); // i18n: not shown
    }
    fn patch_settings(&self, patch: serde_json::Value) {
        self.say(format!("patch {patch}")); // i18n: not shown
    }
    fn take_notices(&self) -> Vec<crate::controller::Notice> {
        Vec::new()
    }
    fn library_version(&self) -> u64 {
        0
    }
    fn attach(&self, _ctx: &egui::Context) {}
    fn start_capture(&self) {}
    fn stop_capture(&self) {}
    fn pause_capture(&self, _paused: bool) {}
    fn start_test_capture(&self) {}
    fn import_raw_capture(&self, _dir: PathBuf) {}
    fn trash_session(&self, _dir: PathBuf) {}
    fn clean_up(&self, _trim: Vec<PathBuf>, _delete: Vec<PathBuf>) {}
    fn reconstruct(&self, _dir: PathBuf) {}
    fn stop_reconstruction(&self) {}
    fn unqueue_reconstruction(&self, _dir: PathBuf) {}
    fn send_to_mixer(&self, _dir: PathBuf, _title: String) {}
    fn sign_in(&self) {}
    fn reopen_sign_in(&self) {}
    fn cancel_sign_in(&self) {}
    fn sign_out(&self) {}
    fn request_worker_log(&self) {}
    fn worker_log(&self) -> Option<Vec<String>> {
        None
    }
    fn upload_pictures(&self, _world_id: String, _world_name: String, _paths: Vec<PathBuf>) {}
    fn upload_preview(&self, _world_id: String, _world_name: String, _session: PathBuf) {}
    fn request_maps(&self) {}
    fn maps(&self) -> Option<crate::state::MapsMine> {
        None
    }
    fn calibrated(&self, _calibration: orbiters_contracts::performance::calibration::Calibration) {}
    fn frames(&self, _chunk: crate::state::FrameChunk) {}
    fn headset(&self, _in_use: bool) {}
    fn walk_boost(&self, _strength: f32) {}
    fn osc_reconnect(&self) {}
    fn updates(&self, _action: crate::ipc::UpdateAction) {}
    fn quit(&self) {}
}

/// A keyboard the check presses.
#[derive(Clone, Default)]
struct Pressed {
    held: Rc<RefCell<Vec<u16>>>,
    downs: Rc<Cell<u32>>,
    button: Rc<Cell<bool>>,
}

impl Source for Pressed {
    fn down(&self, key: u16) -> bool {
        self.held.borrow().contains(&key)
    }
    fn downs(&self) -> u32 {
        self.downs.get()
    }
    fn button(&self) -> bool {
        self.button.get()
    }
    fn r(&self) -> u16 {
        0x0f
    }
    fn allowed(&self) -> bool {
        true
    }
}

fn key(name: &str) -> anyhow::Result<u16> {
    const DIGITS: [u16; 9] = [0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1a, 0x1c, 0x19];
    Ok(match name {
        "esc" => keyboard::ESC,
        "r" => 0x0f,
        "option" => keyboard::LEFT_OPTION,
        d => DIGITS[d.parse::<usize>().ok().filter(|d| (1..=9).contains(d)).context("a key's name")? - 1], // i18n: not shown
    })
}

/// The processor time this process used, in seconds.
fn cpu() -> f64 {
    // SAFETY: a plain query into a structure of ours.
    unsafe {
        let mut used: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut used);
        let s = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
        s(used.ru_utime) + s(used.ru_stime)
    }
}

fn desk(launcher: u32) -> anyhow::Result<()> {
    let mtm = MainThreadMarker::new().context("not on the main thread")?; // i18n: not shown
    super::stay_awake();
    let mut snap = Snapshot::new(orbiters_core::settings::Settings::default());
    snap.launcher.desktop_game = Some(launcher);
    let stub = Arc::new(Stub { view: Mutex::new(Arc::new(snap)), said: Mutex::default() });
    let keys = Pressed::default();
    let mut desk = Desk::new(mtm, stub.clone(), Box::new(keys.clone()));
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in crate::worker::log_lines(std::io::stdin()) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let began = Instant::now();
    let mut last = String::new();
    let (mut cpu_at, mut cpu_was, mut turns) = (Instant::now(), cpu(), 0u64);
    loop {
        loop {
            let line = match rx.try_recv() {
                Ok(line) => line,
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                // Whoever asked is gone.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(()),
            };
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["press", k] => {
                    keys.held.borrow_mut().push(key(k)?);
                    keys.downs.set(keys.downs.get() + 1);
                }
                ["let", "go", k] => {
                    let code = key(k)?;
                    keys.held.borrow_mut().retain(|held| *held != code);
                }
                ["button", to] => keys.button.set(*to == "down"),
                ["point", zone] => println!("{}", serde_json::json!({ "pointed": point(&mut desk, zone) })),
                ["spot", zone] => println!("{}", serde_json::json!({ "spot": desk.windows.list.iter().find(|(z, w)| format!("{z:?}") == *zone && w.shown()).and_then(|(_, w)| w.spot()) })),
                ["client"] => println!("{}", serde_json::json!({ "client": desk.last.map(|g| [g.client().x, g.client().y]) })),
                // The pointer over the control of a part that does `action`, as macOS' own pointer goes.
                ["control", zone, action] => println!("{}", serde_json::json!({ "pointed": control(&mut desk, zone, action) })),
                // What a person sees of a part: macOS's own picture of its window (how much of it is painted, how much it varies), and which of ours lie over it.
                ["panel", zone] => println!("{}", serde_json::json!({ "panel": seen_of(&desk, zone) })),
                // As macOS gives our own windows what was clicked and typed on them.
                ["click"] => {
                    post_button(mtm, true);
                    post_button(mtm, false);
                }
                ["type", code] => {
                    post_key(mtm, code.parse()?, true);
                    post_key(mtm, code.parse()?, false);
                }
                ["hide-menus"] => {
                    let mut view = stub.view.lock().unwrap();
                    let snap = Arc::make_mut(&mut view);
                    snap.settings.orbit.wheel = false;
                    snap.settings.orbit.quick_menu = false;
                }
                ["hang"] => std::thread::sleep(Duration::from_secs(10)),
                ["cpu"] => {
                    let (now, used) = (Instant::now(), cpu());
                    let wall = now.duration_since(cpu_at).as_secs_f64();
                    println!("{}", serde_json::json!({ "cpu_percent": 100.0 * (used - cpu_was) / wall.max(1e-6), "seconds": wall, "turns": turns }));
                    (cpu_at, cpu_was, turns) = (now, used, 0);
                }
                ["say"] => last.clear(),
                // The parts as they lie, over grey, at the size of the game's picture.
                ["save", path] => println!("{}", serde_json::json!({ "saved": save(&desk, Path::new(path)).is_ok() })),
                // The windows on screen from the front to the game's: ours, the game's, others'.
                ["order"] => {
                    let ours = desk.doing().shown;
                    let game = desk.last.map(|g| g.window.id);
                    let mut list = Vec::new();
                    for w in crate::bridge::mac::on_screen().unwrap_or_default() {
                        let who = if Some(w.id) == game { "game" } else if ours.contains(&w.id) { "ours" } else if w.owner == std::process::id() { "own" } else { "other" };
                        list.push(serde_json::json!([who, w.layer, w.width, w.height]));
                        if who == "game" {
                            break;
                        }
                    }
                    println!("{}", serde_json::json!({ "order": list }));
                }
                ["quit"] => return Ok(()),
                other => println!("{}", serde_json::json!({ "unknown": other })),
            }
            use std::io::Write as _;
            let _ = std::io::stdout().flush();
        }
        let next = desk.turn();
        turns += 1;
        for what in std::mem::take(&mut *stub.said.lock().unwrap()) {
            println!("{}", serde_json::json!({ "at": began.elapsed().as_secs_f64(), "said": what }));
        }
        let doing = desk.doing();
        let now = serde_json::json!({
            "seen": format!("{:?}", doing.seen),
            "open": doing.open.map(|o| format!("{o:?}")),
            "shown": doing.shown,
            "catching": doing.catching,
            "held": doing.held,
            "keyboard": doing.keyboard,
            "pointing": doing.pointing,
            "sheet": doing.sheet,
            "looking": doing.looking.0,
            "left_out": doing.left_out.map(|z| format!("{z:?}")),
        });
        if now.to_string() != last {
            last = now.to_string();
            println!("{}", serde_json::json!({ "at": began.elapsed().as_secs_f64(), "doing": now, "painted": doing.painted, "frames": doing.looking.1 }));
        }
        match next {
            Next::Wait(long) => desk.wait(long),
            Next::Anew => {
                println!("{}", serde_json::json!({ "anew": true }));
                return Ok(());
            }
        }
    }
}

fn save(desk: &Desk, path: &Path) -> anyhow::Result<()> {
    let size = desk.last.context("no game")?.client(); // i18n: not shown
    let (w, h) = (size.x as u32, size.y as u32);
    let mut out = image::RgbaImage::from_pixel(w, h, image::Rgba([72, 76, 80, 255]));
    for window in desk.windows.shown() {
        let ([pw, ph], Some([x0, y0])) = (window.px(), window.spot()) else { continue };
        for (i, p) in window.pixels().chunks_exact(4).enumerate() {
            let (x, y) = (x0 + (i as u32 % pw) as i32, y0 + (i as u32 / pw) as i32);
            if i as u32 / pw >= ph || x < 0 || y < 0 || x >= w as i32 || y >= h as i32 || p[3] == 0 {
                continue;
            }
            let under = out.get_pixel_mut(x as u32, y as u32);
            for c in 0..3 {
                under.0[c] = (u32::from(p[c]) + u32::from(under.0[c]) * (255 - u32::from(p[3])) / 255).min(255) as u8;
            }
        }
    }
    out.save(path)?;
    Ok(())
}

/// The pointer over the middle of the part named, while the keyboard is ours.
fn point(desk: &mut Desk, zone: &str) -> bool {
    let Some(game) = desk.last else { return false };
    let Some(at) = desk.panels.all().find(|p| format!("{:?}", p.zone) == zone && p.shown).map(|p| middle(p.zone, p.placed)) else { return false };
    panel::put_pointer(&desk.hold, game.point(at))
}

/// The pointer over the control that does `action` (its name's start) on the part in `zone`.
fn control(desk: &mut Desk, zone: &str, action: &str) -> bool {
    let Some(game) = desk.last else { return false };
    let at = desk.panels.all().find(|p| format!("{:?}", p.zone) == zone && p.shown).and_then(|p| {
        let item = p.items.iter().find(|i| format!("{:?}", i.action).starts_with(action))?;
        Some(p.placed.rect.min + item.rect.center().to_vec2() / p.logical * p.placed.rect.size())
    });
    at.is_some_and(|at| panel::put_pointer(&desk.hold, game.point(at)))
}

fn seen_of(desk: &Desk, zone: &str) -> serde_json::Value {
    let Some(window) = desk.windows.list.iter().find(|(z, w)| format!("{z:?}") == zone && w.shown()).map(|(_, w)| w) else { return serde_json::Value::Null };
    let number = u32::try_from(window.number()).unwrap_or(0);
    let list = crate::bridge::mac::on_screen().unwrap_or_default();
    let ours = desk.doing().shown;
    let me = list.iter().find(|w| w.id == number).copied();
    let over: Vec<u32> = list
        .iter()
        .take_while(|w| w.id != number)
        .filter(|w| ours.contains(&w.id) && me.is_some_and(|m| w.x < m.x + m.width && m.x < w.x + w.width && w.y < m.y + m.height && m.y < w.y + w.height))
        // (what is wide and tall enough to hide it: the arrow, a tile's corner are not)
        .filter(|w| me.is_some_and(|m| w.width * w.height > m.width * m.height / 4.0))
        .map(|w| w.id)
        .collect();
    let picture = crate::bridge::mac::capture_png(number).ok().and_then(|png| image::load_from_memory(&png).ok()).map(|i| i.to_rgba8());
    let (opaque, spread) = picture.map_or((0.0, 0.0), |p| {
        let n = (p.width() * p.height()).max(1) as f64;
        let grey = |p: &image::Rgba<u8>| (f64::from(p[0]) + f64::from(p[1]) + f64::from(p[2])) / 3.0;
        let mean = p.pixels().map(grey).sum::<f64>() / n;
        (p.pixels().filter(|p| p[3] > 0).count() as f64 / n, (p.pixels().map(|p| (grey(p) - mean).powi(2)).sum::<f64>() / n).sqrt())
    });
    serde_json::json!({ "opaque": opaque, "spread": spread, "over": over })
}

fn middle(zone: Zone, placed: zones::Placed) -> Pos2 {
    match zone.ring() {
        Some(ring) => placed.center + zones::polar((ring.inner + ring.outer) / 2.0, ring.middle()) * placed.unit,
        None => placed.rect.center(),
    }
}

fn post_button(mtm: MainThreadMarker, down: bool) {
    let kind = if down { NSEventType::LeftMouseDown } else { NSEventType::LeftMouseUp };
    let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(kind, NSPoint::new(1.0, 1.0), NSEventModifierFlags::empty(), 0.0, 0, None, 0, 1, 1.0);
    if let Some(event) = event {
        NSApplication::sharedApplication(mtm).postEvent_atStart(&event, false);
    }
}

fn post_key(mtm: MainThreadMarker, code: u16, down: bool) {
    let kind = if down { NSEventType::KeyDown } else { NSEventType::KeyUp };
    let none = NSString::from_str("");
    let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(kind, NSPoint::new(1.0, 1.0), NSEventModifierFlags::empty(), 0.0, 0, None, &none, &none, false, code);
    if let Some(event) = event {
        NSApplication::sharedApplication(mtm).postEvent_atStart(&event, false);
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::zones::Around;
    use super::super::capture;
    use super::super::game::{Game, Shot};
    use super::*;

    fn data() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata")
    }

    fn expected(shows: Shows) -> Option<Around> {
        match shows {
            Shows::Wheel => Some(Around::Wheel),
            // (not in its place: what looks around it finds)
            Shows::Beside => None,
            Shows::QuickMenu => Some(Around::QuickMenu),
            Shows::Nothing => None,
        }
    }

    /// A Mac's screen (points, scale) with the game over the whole of it.
    fn screen(w: f64, h: f64, scale: f64) -> Game {
        let window = crate::bridge::mac::MacWindow { id: 1, owner: 2, x: 0.0, y: 0.0, width: w, height: h, layer: 25, on_screen: true, alpha: Some(1.0) };
        Game::of(window, scale, Some((0.0, 0.0, w, h)))
    }

    /// The pixel at (x, y) of the picture asked of macOS (`shot`) of a game whose drawing `guest` the engine stretches over its whole window of `client` pixels: between four of the
    /// screen's, each between four of the game's.
    fn taken(guest: &Picture, client: Vec2, shot: &Shot, x: usize, y: usize) -> [u8; 4] {
        let on_screen = |v: usize, of: f32, to: f32| (v as f32 + 0.5) * of / to - 0.5;
        let (sx, sy) = (on_screen(x, client.x, shot.width as f32).max(0.0), on_screen(y, client.y, shot.client().y).max(0.0));
        let in_guest = |sx: f32, sy: f32| guest.at((sx + 0.5) * guest.width as f32 / client.x - 0.5, (sy + 0.5) * guest.height as f32 / client.y - 0.5);
        let (ix, iy, ax, ay) = (sx.floor(), sy.floor(), sx.fract(), sy.fract());
        let four = [in_guest(ix, iy), in_guest(ix + 1.0, iy), in_guest(ix, iy + 1.0), in_guest(ix + 1.0, iy + 1.0)].map(|p| p.map(f32::from));
        std::array::from_fn(|c| ((four[0][c] * (1.0 - ax) + four[1][c] * ax) * (1.0 - ay) + (four[2][c] * (1.0 - ax) + four[3][c] * ax) * ay).round() as u8)
    }

    /// What the engine shows of a game that draws `percent` % of the display's pixels, as ScreenCaptureKit hands it over: drawn small, stretched over the screen, taken at the size
    /// looks are asked at. The menus are found, and nothing else is, at the render percents Home offers, on this Mac's screen and on another.
    #[test]
    fn the_menus_are_found_whatever_part_of_the_screen_s_pixels_the_game_draws() {
        for (game, percents) in [(screen(1470.0, 956.0, 2.0), &[100.0, 58.0, 45.0][..]), (screen(1920.0, 1080.0, 1.0), &[100.0, 45.0])] {
            let shot = Shot::of(&game);
            for percent in percents {
                let drawn = (game.client() * *percent / 100.0).round();
                for (shows, name) in Shows::ALL {
                    let guest = frame(&data(), shows, drawn).unwrap();
                    // Only what a look reads is made.
                    let (w, h) = (shot.width as usize, shot.height as usize);
                    let mut bgra = vec![0u8; w * h * 4];
                    for part in super::super::super::detect::parts(shot.client()) {
                        for y in part.min.y as usize..(part.max.y.ceil() as usize + 1).min(h) {
                            for x in part.min.x as usize..(part.max.x.ceil() as usize + 1).min(w) {
                                bgra[(y * w + x) * 4..][..4].copy_from_slice(&taken(&guest, game.client(), &shot, x, y));
                            }
                        }
                    }
                    if shows == Shows::Beside {
                        // (the whole of it is read: in place it is no wheel, around it is one, where it is)
                        let mut bgra = vec![0u8; w * h * 4];
                        for (i, px) in bgra.chunks_exact_mut(4).enumerate() {
                            px.copy_from_slice(&taken(&guest, game.client(), &shot, i % w, i / w));
                        }
                        assert_eq!(capture::look(&bgra, w, h, w * 4, &shot).unwrap().0, None, "in place, {percent} %");
                        let found = capture::measure(&bgra, w, h, w * 4, &shot, &capture::Search::Around).unwrap();
                        assert_eq!(found.menu, Some(Around::Wheel), "around its place, {percent} %: {:?}", found.fit);
                        let fit = found.fit.unwrap();
                        let want = BESIDE.0 * 0.139;
                        assert!((fit.off - want).length() <= 0.139 * 0.03 && (fit.size - BESIDE.1).abs() <= 0.03, "{fit}, not {want:?} {}", BESIDE.1);
                        // Where it was found, it stays found; and nothing is there once it is gone.
                        let at = capture::Search::At(fit);
                        assert_eq!(capture::measure(&bgra, w, h, w * 4, &shot, &at).unwrap().menu, Some(Around::Wheel));
                        let none = frame(&data(), Shows::Nothing, shot.client()).unwrap();
                        for search in [at, capture::Search::Around] {
                            assert_eq!(capture::measure(&none.bgra, w, h, w * 4, &shot, &search).unwrap().menu, None);
                        }
                        // What the log says of a look that found nothing names where it is.
                        let anywhere = super::super::wheel::anywhere(&super::super::super::detect::Part { x0: 0, y0: 0, width: w as u32, height: h as u32, stride: w as u32 * 4, bgra: &bgra }, shot.client()).unwrap();
                        assert!((anywhere.off - want).length() <= 0.139 * 0.06, "{anywhere}");
                        continue;
                    }
                    let found = capture::look(&bgra, w, h, w * 4, &shot).expect("a frame of the size asked");
                    assert_eq!(found.0, expected(shows), "{name} at {percent} % of {:?}", game.client());
                    if shows == Shows::Wheel {
                        assert!(found.1, "its rim, {percent} %");
                    }
                }
            }
        }
    }

    /// Every real frame there is of a menu and of none, as if the game drew it at 45 % of a screen: stretched up by 1 / 0.45 and taken at the looks' size. What it shows is what it
    /// showed.
    #[test]
    fn real_frames_keep_what_they_show_through_the_engine_s_stretching() {
        use super::super::super::detect::{self, Part};
        let mut tried = 0;
        for entry in std::fs::read_dir(data()).unwrap() {
            let path = entry.unwrap().path();
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            let kind = stem.split('-').next().unwrap_or_default().to_owned();
            let fields: Vec<&str> = stem.split('_').collect();
            if !["wheel", "qm", "none"].contains(&kind.as_str()) || fields.len() != 4 {
                continue;
            }
            let (part, client, corner) = fixture(&data(), fields[0]).unwrap();
            let screen = client / 0.45;
            let k = (1080.0f32 / screen.y).min(1.0) / 0.45;
            let shot = Shot { width: (client.x * k).round() as u32, height: (client.y * k).round() as u32, title: 0 };
            // (of the part alone: its own corner of the screen's pixels)
            let (w, h) = ((part.width as f32 * k) as usize, (part.height as f32 * k) as usize);
            let here = Shot { width: w as u32, height: h as u32, title: 0 };
            let over = vec2(part.width as f32, part.height as f32) / 0.45;
            let mut bgra = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for x in 0..w {
                    bgra.extend(taken(&part, over, &here, x, y));
                }
            }
            let at = (corner.to_vec2() * k).round();
            let seen = Part { x0: at.x as u32, y0: at.y as u32, width: w as u32, height: h as u32, stride: w as u32 * 4, bgra: &bgra };
            tried += 1;
            if fields[0].ends_with("-row") {
                assert_eq!(detect::quick_menu_seen(&seen, shot.client()), kind == "qm", "{stem} at 45 %: {} icons", detect::icons_found(&seen, shot.client()));
            } else {
                assert_eq!(detect::wheel_seen(&seen, shot.client()), kind == "wheel", "{stem} at 45 %: {:?}", detect::wheel_scores(&seen, shot.client()));
            }
        }
        assert!(tried >= 40, "{tried}");
    }

    /// Every real frame with no wheel, and the Quick Menu's, looked at around the wheel's place: none is taken for one, at what is asked there. And every real wheel is one there too.
    #[test]
    fn around_its_place_only_a_wheel_is_one() {
        use super::super::super::detect::Part;
        use super::super::wheel;
        let (mut tried, mut most) = (0, 0.0f32);
        for entry in std::fs::read_dir(data()).unwrap() {
            let path = entry.unwrap().path();
            let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
            let kind = stem.split('-').next().unwrap_or_default().to_owned();
            let fields: Vec<&str> = stem.split('_').collect();
            if !["wheel", "qm", "none"].contains(&kind.as_str()) || fields.len() != 4 || fields[0].ends_with("-row") {
                continue;
            }
            let (part, client, corner) = fixture(&data(), fields[0]).unwrap();
            let seen = Part { x0: corner.x as u32, y0: corner.y as u32, width: part.width as u32, height: part.height as u32, stride: part.width as u32 * 4, bgra: &part.bgra };
            let best = wheel::near(&seen, client);
            tried += 1;
            if kind == "wheel" {
                let best = best.expect("a wheel");
                assert!(best.scores.has(&wheel::FIT), "{stem}: {best}");
                assert!(best.off.length() < 0.139 * 0.045 && (best.size - 1.0).abs() < 0.045, "{stem}: {best}");
                assert!(wheel::still(&seen, client, &best), "{stem}");
            } else if let Some(best) = best {
                most = most.max(best.scores.of(&wheel::FIT));
                assert!(best.scores.of(&wheel::FIT) < 0.9, "{stem}: {best}");
            }
        }
        // And every whole frame of a world there is here, laid under the wheel's place at many places.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata_camera/desktop");
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let Ok(image) = image::open(&path) else { continue };
            let image = image.to_rgba8();
            let (w, h) = image.dimensions();
            let bgra: Vec<u8> = image.pixels().flat_map(|p| [p[2], p[1], p[0], 255]).collect();
            let client = vec2(w as f32, h as f32);
            for (x, y) in [(0, 0), (w / 6, 0), (w / 3, 0), (0, h / 5), (w / 6, h / 5), (w / 3, h / 5)] {
                // (a part that starts there, taken for the whole: another piece of the world under the wheel's place)
                let part = Part { x0: 0, y0: 0, width: w - x, height: h - y, stride: w * 4, bgra: &bgra[((y * w + x) * 4) as usize..] };
                tried += 1;
                if let Some(best) = wheel::near(&part, client) {
                    most = most.max(best.scores.of(&wheel::FIT));
                    assert!(!best.scores.has(&wheel::FIT), "{} at {x} {y}: {best}", path.display());
                }
            }
        }
        assert!(tried >= 60, "{tried}");
        println!("{tried} looked at; the nearest to a wheel where none is: {most:.2} of what is asked");
    }

    /// A frame of the window's older size says nothing, and a title bar is not looked at.
    #[test]
    fn a_frame_of_another_size_is_no_look() {
        let game = screen(1470.0, 956.0, 2.0);
        let shot = Shot::of(&game);
        let picture = frame(&data(), Shows::Wheel, shot.client()).unwrap();
        assert!(capture::look(&picture.bgra, picture.width, picture.height, picture.width * 4, &shot).is_some());
        assert!(capture::look(&picture.bgra, picture.width - 2, picture.height, picture.width * 4, &shot).is_none());
        assert!(capture::look(&picture.bgra[..1000], picture.width, picture.height, picture.width * 4, &shot).is_none());
        // With a title bar over it, and rows longer than the picture.
        let titled = Shot { height: shot.height + 40, title: 40, ..shot };
        let stride = picture.width * 4 + 64;
        let mut bgra = vec![0u8; stride * 40];
        for row in picture.bgra.chunks(picture.width * 4) {
            bgra.extend_from_slice(row);
            bgra.extend([0u8; 64]);
        }
        assert_eq!(capture::look(&bgra, picture.width, picture.height + 40, stride, &titled), Some((Some(Around::Wheel), true)));
    }
}
