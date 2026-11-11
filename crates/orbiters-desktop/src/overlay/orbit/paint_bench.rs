//! `orbiters-desktop --orbit-paint-bench [--out <folder>] [--runs <n>] [--only vr|desktop] [--upload] [--fonts-anew]`: what these menus cost the loop that also reads the hands, with no headset, no
//! SteamVR and no VRChat. The real [`Panels`] are taken turn by turn as `vr.rs` and `desktop.rs` take them (opened, pointed at, a sheet opened, scrolled, closed, opened again), over a
//! backend that does nothing, and two things are written:
//!
//! - **How long each turn took**, in a release build: from a menu opening to every part having a picture, and the longest single turn while a sheet paints, scrolls or animates. While
//!   that turn runs nothing else of the loop does: hands, beams and the 3D view wait for it (docs/overlay-latency-results-2026-10-30.md, "This tail remains").
//! - **What was drawn**, as a hash of every part's picture at every turn, with where it is, whether it shows and how far in (`pictures-<mode>.txt`), and each part's picture as a PNG
//!   wherever the script comes to rest. The same script on two builds gives two such folders: a change that must not move one pixel is shown not to by comparing them
//!   (`diff`, `cmp`). The time the pictures are painted at is the script's own, a turn at a time, so that two runs paint the same.
//!
//! The pictures of the gallery are the bench's own (a folder of made-up ones in its scratch), never this computer's library: what it measures is the same on any day. Nothing here
//! reads or writes the app's own folders, which are pointed at the scratch for the run.
//!
//! With `--upload` each picture handed over is also written to a Direct3D texture as the headset's overlays have them (`orbiters_openvr::gpu`, two textures a part, only what
//! changed), on this computer's default graphics card: the part of handing a picture to SteamVR that is this process's own work, timed apart. Run that one under the lab's GPU lock.
//!
//! With `--fonts-anew` a part kept from an earlier opening has its fonts' atlas made from nothing again (`Panels::fonts_anew`): slower, and every pixel as a new part's.
//!
//! What it does not measure: SteamVR taking the texture, a window taking its picture on the desktop (the count of pixels handed over is given instead), and anything of a headset.

use super::desktop::Hints;
use super::panels::Action;
use super::vr;
use super::zones::{self, Around, Placed, Zone};
use super::{LoadedRecent, Panel, Panels};
use crate::state::Snapshot;
use anyhow::Context as _;
use egui::{vec2, Pos2, Vec2};
use orbiters_core::settings::Settings;
use orbiters_n4n0_core::menu::Target;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// VRChat's window in desktop mode, for where the parts go and how large they are.
const CLIENT: Vec2 = vec2(1920.0, 1080.0);
/// How many pictures the made-up gallery holds: several pages of it, so that it scrolls.
static RETRO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static GALLERY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Vr,
    Desktop,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Vr => "vr",
            Mode::Desktop => "desktop",
        }
    }

    /// A turn of its loop, in seconds (`overlay::TURN`, `desktop::TURN`).
    fn turn(self) -> f32 {
        match self {
            Mode::Vr => 0.011,
            Mode::Desktop => 0.008,
        }
    }
}

/// What a hand does in a turn, after pointing.
#[derive(Clone, Copy, Debug)]
enum Act {
    Press,
    Release,
    /// The mouse's wheel, in notches.
    Wheel(f32),
    /// The stick, up positive.
    Stick(f32),
}

/// What a turn cost, by what it did.
#[derive(Clone, Copy, Debug, Default)]
struct Cost {
    /// `set_open`: the parts made or kept.
    open: Duration,
    /// `follow`, `menu_turn`, `step_springs`.
    follow: Duration,
    /// Pointing and pressing.
    hand: Duration,
    paint: Duration,
    phases: crate::overlay::PaintPhases,
    worker: crate::overlay::PaintPhases,
    wall: Duration,
    /// Writing the pictures to their textures, with `--upload` (not part of `paint`).
    upload: Duration,
    /// Pictures handed to the platform, and their pixels.
    shows: u32,
    pixels: u64,
}

impl Cost {
    fn whole(&self) -> Duration {
        self.open + self.follow + self.hand + self.paint + self.upload
    }
}

/// How a step's turns are written to the pictures' log.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Logged {
    /// Every turn: what it does depends on nothing but the script.
    Turns,
    /// Only where it comes to rest: something in it runs on the clock or on another thread (a list read, a picture decoded, a ring's grace).
    Rest,
}

/// A step of the script, measured.
struct Step {
    name: String,
    turns: Vec<Cost>,
    /// For an opening: the turn at whose end every part had a picture, and the one at whose end none was due another.
    pictured: Option<usize>,
    complete: Option<usize>,
}

type WorldFixture = (Vec<crate::overlay::worlds::catalog::Entry>, Vec<egui::ColorImage>);
fn register_world_fixture(panels: &mut Panels, fixture: &mut Option<WorldFixture>) {
    // The real lazy module constructor remains inside its measured opening turn.
    if let Some(worlds) = panels.stage.worlds.as_mut() {
        if let Some((entries, images)) = fixture.take() {
            assert!(worlds.freeze_paint_fixture(entries, images) && worlds.paint_fixture_ready(), "local thumbnails must be ready before replay");
        }
    }
}
struct Rig {
    world_fixture: Option<WorldFixture>,
    mode: Mode,
    panels: Panels,
    snap: Arc<Snapshot>,
    hints: Hints,
    around: Option<Around>,
    pointer: Option<(usize, Pos2)>,
    clock: f64,
    was_shown: bool,
    sheet: vr::SheetPicture,
    step: String,
    logged: Logged,
    turn_in_step: usize,
    steps: Vec<Step>,
    /// The pictures' log and where the PNGs go, on the run that writes them.
    log: Option<std::io::BufWriter<std::fs::File>>,
    pictures: Option<PathBuf>,
    notes: Vec<String>,
    /// How often a part was painted in a turn that then handed nothing over, by step and part: work that changed no pixel.
    for_nothing: std::collections::BTreeMap<(String, String), u32>,
    /// The most parts kept from menus that closed at any turn of the script, and the most bytes they held.
    most_kept: (usize, usize),
    /// What the facts cost, by where the script was: made again and compared, and copied, in microseconds.
    facts: Vec<(String, f64, f64)>,
    /// With `--upload`: the textures the pictures are written to, and each part's stand-in for its overlay.
    gpu: Option<orbiters_openvr::gpu::OverlayTextures>,
    handles: std::collections::HashMap<Zone, orbiters_openvr::OverlayHandle>,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// FNV-1a over the picture, eight bytes at a time: told apart is all it has to do.
fn hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let (words, rest) = bytes.as_chunks::<8>();
    for w in words {
        h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    for b in rest {
        h = (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The made-up gallery's pictures, once written: their files and sizes.
static PICTURES: OnceLock<Vec<(PathBuf, u64)>> = OnceLock::new();

/// Writes the made-up pictures: skies of a few colours with a sun somewhere, told apart at a glance.
fn make_pictures(dir: &Path) -> anyhow::Result<Vec<(PathBuf, u64)>> {
    std::fs::create_dir_all(dir)?;
    let mut out: Vec<(PathBuf, u64)> = Vec::new();
    for n in 0..GALLERY.load(std::sync::atomic::Ordering::Relaxed) {
        if n >= 60 {
            let path = dir.join(format!("bench-picture-{n:04}.png"));
            std::fs::hard_link(&out[n % 60].0, &path)?;
            out.push((path, out[n % 60].1));
            continue;
        }
        let (w, h) = (640u32, 360u32);
        let mut image = image::RgbImage::new(w, h);
        let (sx, sy) = (((n * 97) % 640) as f32, ((n * 53) % 300) as f32);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let t = y as f32 / h as f32;
            let sun = (1.0 - ((x as f32 - sx).hypot(y as f32 - sy) / 140.0)).clamp(0.0, 1.0);
            let base = [(30 + (n * 37) % 150) as f32, (40 + (n * 71) % 120) as f32, (70 + (n * 13) % 160) as f32];
            *pixel = image::Rgb([
                (base[0] * (1.0 - t * 0.6) + 200.0 * sun).min(255.0) as u8,
                (base[1] * (1.0 - t * 0.5) + 150.0 * sun).min(255.0) as u8,
                (base[2] * (1.0 - t * 0.3) + 60.0 * sun).min(255.0) as u8,
            ]);
        }
        let path = dir.join(format!("bench-picture-{n:02}.png")); // i18n: not shown (a developer's benchmark command)
        image.save(&path).with_context(|| format!("writing {}", path.display()))?; // i18n: not shown (a developer's benchmark command)
        let size = std::fs::metadata(&path)?.len();
        out.push((path, size));
    }
    Ok(out)
}

/// The gallery's list, in place of this computer's: the made-up pictures, each of a world of a few, a day apart, long ago (so that what says when is a date, the same on any day).
fn list(_sessions: &Path, _folders: &[PathBuf]) -> Vec<LoadedRecent> {
    const WORLDS: [&str; 5] = ["Sky Dream", "Rainy Skyline", "Pacific Dreams (Night)", "After The Rain", "Canine Test Lab"]; // i18n: not shown (fixed synthetic world names for pixel comparisons)
    PICTURES
        .get()
        .map(|pictures| {
            pictures
                .iter()
                .enumerate()
                .map(|(n, (path, size))| {
                    let taken = 1_750_000_000 - n as u64 * 86_400;
                    LoadedRecent {
                        key: path.clone(),
                        world_id: String::new(),
                        title: WORLDS[n % WORLDS.len()].to_string(),
                        image: None,
                        asked: false,
                        has_view: false,
                        session: false,
                        picture: false,
                        taken,
                        modified: taken,
                        source: crate::worlds::Picture::Shot { path: path.clone(), size: *size, width: crate::worlds::THUMB_WIDTH },
                        aspect: 16.0 / 9.0,
                        at: None,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What plays on the computer, for the run: the menus' sample song at `position` seconds, never what this computer plays.
fn music(position: u32) {
    static ART: OnceLock<Arc<crate::media::Art>> = OnceLock::new();
    let art = ART.get_or_init(|| Arc::new(crate::media::Art { key: crate::overlay::menu_pages::SAMPLE_ART.into(), rgb: crate::overlay::menu_pages::sample_art() }));
    let mut media = crate::overlay::menu_pages::sample_media();
    media.position = Some(position);
    crate::media::watcher().stand_in(crate::media::Reading { media: Some(media), volume: Some(orbiters_n4n0_core::menu::Volume { percent: 40, muted: false }), art: Some(art.clone()) });
}

impl Rig {
    fn new(mode: Mode, scratch: &Path, log: Option<(std::fs::File, PathBuf)>) -> Self {
        let mut settings = Settings { sessions_dir: scratch.join("sessions"), picture_folders: Vec::new(), ..Settings::default() };
        if RETRO.load(std::sync::atomic::Ordering::Relaxed) { settings.orbit.theme = orbiters_core::settings::MenuLook::Retro; }
        let mut snap = crate::screenshot::idle_snapshot(&settings, &[]);
        // In a headset SteamVR runs; on the desktop it does not, which is what the parts go by.
        snap.steamvr.running = mode == Mode::Vr;
        snap.steamvr.overlay = mode == Mode::Vr;
        let mut panels = Panels::new(Arc::new(crate::screenshot::Still), if mode == Mode::Vr { 2.0 } else { 1.0 }, true);
        let world_fixture = {
            let previews: Vec<_> = (0..6).map(|i| serde_json::json!({"slot": i, "key": i+1, "id": i+1, "worldId": format!("wrld_00000000-0000-4000-8000-{i:012}"), "name": format!("Replay world {i}"), "author": "Replay", "uploader": "fixture", "view": {"position": [0,1,0], "target": [0,1,1]}})).collect();
            let entries = crate::overlay::worlds::catalog::parse(&serde_json::json!({"previews": previews}).to_string()).expect("local world fixtures");
            let images = (0..entries.len()).map(|i| {
                let image = image::open(&PICTURES.get().expect("local pictures")[i].0).expect("fixture image").to_rgba8();
                egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw())
            }).collect();
            Some((entries, images))
        };
        panels.recents.lister = Some(list);
        panels.clock = Some(0.0);
        let (log, pictures) = match log {
            Some((file, dir)) => (Some(std::io::BufWriter::new(file)), Some(dir)),
            None => (None, None),
        };
        Self {
            world_fixture,
            mode,
            panels: panels.with_page_worker(),
            snap: Arc::new(snap),
            hints: Hints::default(),
            around: None,
            pointer: None,
            clock: 0.0,
            was_shown: false,
            sheet: vr::SheetPicture::default(),
            step: String::new(),
            logged: Logged::Turns,
            turn_in_step: 0,
            steps: Vec::new(),
            log,
            pictures,
            notes: Vec::new(),
            for_nothing: Default::default(),
            most_kept: (0, 0),
            facts: Vec::new(),
            gpu: None,
            handles: Default::default(),
        }
    }

    /// Where the parts go around `around`, as the platform says each turn.
    fn places(&self, around: Option<Around>) -> Vec<Placed> {
        match (self.mode, around) {
            (_, None) => Vec::new(),
            (Mode::Vr, Some(Around::QuickMenu)) => vr::quick_menu_places(0),
            (Mode::Vr, Some(Around::Wheel)) => {
                let mut places: Vec<Placed> = Zone::around(Around::Wheel).iter().map(|z| vr::wheel_place(*z)).collect();
                if !self.panels.sheet_open() {
                    places.push(vr::wheel_place(Zone::WheelHide));
                }
                places
            }
            (Mode::Vr, Some(m)) => Zone::around(m).iter().map(|z| zones::in_units(*z)).collect(),
            (Mode::Desktop, Some(m)) => zones::layout_as(m, CLIENT, false),
        }
    }

    /// One turn of the loop, as `vr.rs` and `desktop.rs` take it, with what a hand does in it.
    fn turn(&mut self, act: Option<Act>) {
        let wall = Instant::now();
        std::thread::sleep(Duration::from_millis(11));
        let dt = self.mode.turn();
        self.clock += dt as f64;
        self.panels.clock = Some(self.clock);
        let mut cost = Cost::default();
        crate::overlay::paint_phases(true);
        let t = Instant::now();
        let mut places = self.places(self.around);
        match self.mode {
            Mode::Vr => {
                self.panels.way_back(self.around, &mut places);
                self.panels.takeover_place = Some(vr::sheet_place(Zone::Takeover));
                self.panels.back_place = Some(vr::sheet_place(Zone::SheetBack));
                self.panels.view_taken = false;
            }
            Mode::Desktop => {
                self.panels.takeover_place = zones::place(Zone::Takeover, CLIENT);
                self.panels.back_place = zones::place(Zone::SheetBack, CLIENT);
                self.panels.hint_place = zones::place(Zone::SheetHint, CLIENT);
            }
        }
        self.panels.set_open(self.around, places, &self.snap);
        if self.mode == Mode::Desktop {
            self.panels.follow_note(&self.snap);
        }
        cost.open = t.elapsed();
        if self.panels.is_empty() {
            // As `OrbitVr::close` does when the last part went.
            if self.was_shown && self.mode == Mode::Vr {
                self.panels.menu_turn(&self.snap);
            }
            self.was_shown = false;
        } else {
            self.was_shown = true;
            let t = Instant::now();
            self.panels.follow(&self.snap, 0);
            self.panels.menu_turn(&self.snap);
            self.panels.step_springs(dt);
            cost.follow = t.elapsed();
            let t = Instant::now();
            if self.mode == Mode::Desktop {
                self.panels.set_pointing(false, !self.snap.settings.reduce_motion);
                self.panels.set_key_caps(false);
            }
            self.panels.hover(self.pointer);
            if self.mode == Mode::Desktop {
                self.hints.follow(self.around, false);
                self.panels.set_hint(self.hints.now(self.around, false));
            }
            match act {
                Some(Act::Press) => {
                    let pressed = self.panels.press(&self.snap);
                    if pressed.hide {
                        self.panels.set_open(self.panels.open_menu(), Vec::new(), &self.snap);
                    }
                }
                Some(Act::Release) => self.panels.release(),
                Some(Act::Wheel(notches)) => self.panels.wheel(notches),
                Some(Act::Stick(up)) => self.panels.stick(up, dt, &self.snap),
                None => {}
            }
            if self.mode == Mode::Vr {
                let _ = self.panels.world_relay(crate::overlay::worlds::panel::Status { headset: true, ..Default::default() });
            }
            cost.hand = t.elapsed();
            // What is due a picture now: painted for nothing, when the turn then hands nothing over.
            assert!(self.panels.facts.sheet != Some(super::panels::Sheet::Worlds) || self.world_fixture.is_none(), "Worlds fixture must be registered before its first paint");
            let due: Vec<Zone> = self.panels.all().filter(|p| p.dirty).map(|p| p.zone).collect();
            let t = Instant::now();
            let (mut shows, mut pixels, mut upload) = (0u32, 0u64, Duration::ZERO);
            let (gpu, handles, notes) = (&mut self.gpu, &mut self.handles, &mut self.notes);
            let page_top = (self.mode == Mode::Vr).then(|| self.panels.all().find(|p| p.zone == Zone::SheetBack).zip(self.panels.takeover.as_ref()).and_then(|(b, p)| vr::page_row(b, p))).flatten();
            let mode = self.mode;
            let worker_sheet = mode == Mode::Vr && self.panels.stage.async_pages && page_top.is_some();
            let sheet = &mut self.sheet;
            if page_top.is_none() { sheet.page = false; }
            self.panels.paint(&self.snap, |panel, changed| {
                if let (Some(top), Zone::SheetBack | Zone::Takeover) = (page_top, panel.zone) {
                    if worker_sheet { return; }
                    let at = Instant::now();
                    if panel.zone == Zone::SheetBack { sheet.back(panel.pixels(), panel.px, top, changed); }
                    else { sheet.page(panel.pixels(), panel.px, top, changed); }
                    super::super::record_phase(3, at.elapsed());
                    return;
                }
                if mode == Mode::Vr && panel.zone == Zone::Takeover { return; }
                shows += 1;
                pixels += (changed[2].saturating_sub(changed[0]) as u64) * (changed[3].saturating_sub(changed[1]) as u64);
                if let Some(textures) = gpu.as_mut() {
                    let t = Instant::now();
                    let next = handles.len() as orbiters_openvr::OverlayHandle + 1;
                    let handle = *handles.entry(panel.zone).or_insert(next);
                    if let Err(e) = textures.write_part(handle, panel.pixels(), panel.px[0], panel.px[1], changed) {
                        notes.push(format!("a texture could not be written: {e}")); // i18n: not shown (a developer's benchmark command)
                    }
                    upload += t.elapsed();
                }
            });
            if worker_sheet { if let Some(picture) = self.panels.take_sheet_picture() { sheet.adopt(picture); } }
            if let Some(top) = page_top.filter(|_| !worker_sheet) {
                if !sheet.page {
                    if let Some(page) = self.panels.takeover.as_ref().filter(|p| p.painted) {
                        sheet.page(page.pixels(), page.px, top, [0, 0, page.px[0], page.px[1]]);
                    }
                }
            }
            if let Some(changed) = sheet.changed.take() {
                shows += 1;
                pixels += (changed[2] - changed[0]) as u64 * (changed[3] - changed[1]) as u64;
                if let Some(textures) = gpu.as_mut() {
                    let at = Instant::now();
                    let next = handles.len() as orbiters_openvr::OverlayHandle + 1;
                    let handle = *handles.entry(Zone::SheetBack).or_insert(next);
                    if let Err(e) = textures.write_part(handle, sheet.pixels(), sheet.px[0], sheet.px[1], changed) {
                        notes.push(format!("a composed texture could not be written: {e}")); // i18n: not shown (benchmark diagnostics)
                    }
                    upload += at.elapsed();
                }
            }
            cost.worker = self.panels.stage.worker_phases;
            cost.upload = upload;
            cost.paint = t.elapsed().saturating_sub(upload);
            (cost.shows, cost.pixels) = (shows, pixels);
            if shows == 0 {
                for zone in due {
                    *self.for_nothing.entry((self.step.clone(), format!("{zone:?}"))).or_default() += 1;
                }
            }
        }
        cost.phases = crate::overlay::paint_phases(false);
        cost.wall = wall.elapsed();
        // Fixture image registration is untimed preparation, as file generation is. The module
        // itself was constructed by the real opening above, with that cost still included.
        register_world_fixture(&mut self.panels, &mut self.world_fixture);
        self.turn_in_step += 1;
        if self.log.is_some() {
            let (kept, bytes) = self.panels.resting();
            self.most_kept = (self.most_kept.0.max(kept), self.most_kept.1.max(bytes));
        }
        if let Some(step) = self.steps.last_mut() {
            step.turns.push(cost);
        }
        if self.logged == Logged::Turns {
            self.write("turn");
        }
    }

    fn turns(&mut self, n: usize) {
        for _ in 0..n {
            self.turn(None);
        }
    }

    /// Whether a part is due a picture, is still coming in, or was handed one in the turn just taken. (Not `Panels::busy`: that also counts N4N0's menu being due a picture, which
    /// it stays while it is on no sheet.)
    fn moving(&self) -> bool {
        (!self.panels.is_empty() && self.panels.stage.painter.busy()) || self.panels.all().any(|p| p.dirty || !p.appear.settled()) || self.steps.last().and_then(|s| s.turns.last()).is_some_and(|c| c.shows > 0)
    }

    /// The seconds to a 3D picture counting down, then where the music is moving on, eight turns a second's worth: what one part says changes, and nothing else.
    fn ticks(&mut self, name: &str) {
        self.begin(name, Logged::Turns);
        for seconds in [Some(3), Some(2), Some(1), None] {
            let mut snap = (*self.snap).clone();
            snap.n4n0.photo_countdown = seconds;
            self.snap = Arc::new(snap);
            self.turns(8);
        }
        for position in [84, 85, 86, 83] {
            music(position);
            self.turns(8);
        }
        self.rest();
    }

    /// What making the parts' facts again and comparing them costs a turn, and copying them for a part to paint from, in microseconds, as things stand now.
    fn facts_cost(&mut self, state: &str) {
        if self.log.is_some() {
            return;
        }
        const TIMES: u32 = 2000;
        let t = Instant::now();
        for _ in 0..TIMES {
            let facts = self.panels.facts_of(&self.snap);
            std::hint::black_box(facts != self.panels.facts);
        }
        let made = t.elapsed().as_secs_f64() * 1e6 / TIMES as f64;
        let t = Instant::now();
        for _ in 0..TIMES {
            std::hint::black_box(self.panels.facts.clone());
        }
        let copied = t.elapsed().as_secs_f64() * 1e6 / TIMES as f64;
        self.facts.push((state.to_string(), made, copied));
    }

    /// What every part shows now, to the pictures' log: one line a part, and one for what the sheet is on.
    fn write(&mut self, kind: &str) {
        let Some(log) = self.log.as_mut() else { return };
        let (mode, step, turn) = (self.mode.name(), &self.step, self.turn_in_step);
        let facts = &self.panels.facts;
        let _ = writeln!(log, "{mode} {step} {kind} {turn:04} state open={:?} sheet={:?} pointed={:?} scroll={:.2} hidden={}", self.panels.open_menu(), facts.sheet, facts.pointed, facts.gallery_scroll, self.panels.hidden());
        for panel in self.panels.all() {
            // A part with no picture yet shows nothing, whatever its buffer holds.
            let picture = if panel.painted { hash(panel.pixels()) } else { 0 };
            let r = panel.placed.rect;
            let _ = writeln!(
                log,
                "{mode} {step} {kind} {turn:04} {:?} {}x{} shown={} painted={} alpha={:.4} out={:.3} at=[{:.2},{:.2},{:.2},{:.2}] hover={:?} pressed={:?} picture={picture:016x}",
                panel.zone,
                panel.px[0],
                panel.px[1],
                panel.shown as u8,
                panel.painted as u8,
                panel.appear.alpha(),
                panel.outward(),
                r.min.x,
                r.min.y,
                r.max.x,
                r.max.y,
                panel.hover,
                panel.pressed,
            );
        }
    }

    /// Each part's picture as a file, where the script rests.
    fn save_pictures(&mut self) {
        let Some(dir) = self.pictures.clone() else { return };
        // The logged replay is untimed. Audit the same frozen draw inputs, fonts and textures
        // through full serial and parallel redraws before saving the shown picture.
        if self.panels.stage.async_pages {
            if let Some(frame) = self.panels.stage.cache.iter().rev().find(|f| self.panels.facts.sheet == Some(f.sheet)) {
                let rx = self.panels.stage.painter.audit(frame.clone());
                match rx.recv_timeout(Duration::from_secs(20)) {
                    Ok(audit) => {
                        let line = format!("{} {} frozen pixels={} header={} parallel={} parallel_header={} geometry={} hash={:016x} sheet_pixels={} parallel_sheet_pixels={}\n", self.mode.name(), self.step, audit.pixels, audit.header_pixels, audit.parallel_pixels, audit.parallel_header_pixels, audit.items, audit.hash, audit.sheet_pixels, audit.parallel_sheet_pixels);
                        use std::io::Write;
                        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.parent().unwrap().join("frozen-parity.txt")) { let _ = file.write_all(line.as_bytes()); }
                        if audit.sheet_pixels != 0 || audit.parallel_sheet_pixels != 0 || audit.pixels != 0 || audit.header_pixels != 0 || audit.parallel_pixels != 0 || audit.parallel_header_pixels != 0 || !audit.items { self.notes.push(line); }
                    }
                    Err(e) => self.notes.push(format!("frozen parity did not finish: {e}")),
                }
            }
        }

        if self.mode == Mode::Vr && self.panels.sheet_open() && !self.sheet.pixels().is_empty() {
            let image = egui::ColorImage::from_rgba_premultiplied([self.sheet.px[0] as usize, self.sheet.px[1] as usize], self.sheet.pixels());
            let path = dir.join(format!("{}-{}-Composed.png", self.mode.name(), self.step)); // i18n: not shown (benchmark artifact path)
            if let Err(e) = orbiters_ui::offscreen::save_png(&path, &image) { self.notes.push(e.to_string()); }
        }
        for panel in self.panels.all().filter(|p| p.painted && p.shown) {
            let image = egui::ColorImage::from_rgba_premultiplied([panel.px[0] as usize, panel.px[1] as usize], panel.pixels());
            let path = dir.join(format!("{}-{}-{:?}.png", self.mode.name(), self.step, panel.zone)); // i18n: not shown (a developer's benchmark command)
            if let Err(e) = orbiters_ui::offscreen::save_png(&path, &image) {
                self.notes.push(format!("{}: {e}", path.display())); // i18n: not shown (a developer's benchmark command)
            }
        }
    }

    fn begin(&mut self, name: &str, logged: Logged) {
        self.step = name.to_string();
        self.logged = logged;
        self.turn_in_step = 0;
        self.steps.push(Step { name: name.to_string(), turns: Vec::new(), pictured: None, complete: None });
    }

    /// Turns until nothing is due and nothing moves, three turns in a row; where something runs on another thread (a list, a picture), until that has come too. Then says where it
    /// rests.
    fn rest(&mut self) {
        let mut quiet = 0;
        let began = Instant::now();
        for n in 0..4000 {
            self.turn(None);
            let waiting = self.panels.recents.rx.is_some();
            quiet = if self.moving() || waiting { 0 } else { quiet + 1 };
            if quiet >= 3 {
                if self.logged == Logged::Turns {
                    break;
                }
                // What another thread reads (a picture decoded) comes in its own time: a moment with nothing new, and it is at rest.
                std::thread::sleep(Duration::from_millis(120));
                let before = self.steps.last().map_or(0, |s| s.turns.iter().map(|c| c.shows).sum::<u32>());
                self.turns(3);
                let after = self.steps.last().map_or(0, |s| s.turns.iter().map(|c| c.shows).sum::<u32>());
                if before == after && !self.moving() && self.panels.recents.rx.is_none() {
                    break;
                }
                quiet = 0;
            }
            if waiting {
                std::thread::sleep(Duration::from_millis(2));
            }
            if n == 3999 || began.elapsed() > Duration::from_secs(20) {
                self.notes.push(format!("{} {}: it never comes to rest", self.mode.name(), self.step)); // i18n: not shown (a developer's benchmark command)
                break;
            }
        }
        self.write("rest");
        self.save_pictures();
    }

    /// A menu opens: turned until it rests, noting when every part had a picture and when none was due another.
    fn open(&mut self, name: &str, around: Around) {
        self.begin(name, Logged::Turns);
        self.around = Some(around);
        self.pointer = None;
        let mut quiet = 0;
        for n in 0..2000 {
            self.turn(None);
            let pictured = !self.panels.is_empty() && self.panels.all().all(|p| p.painted);
            let complete = pictured && self.panels.all().all(|p| !p.dirty);
            let step = self.steps.last_mut().expect("begun");
            if pictured && step.pictured.is_none() {
                step.pictured = Some(n);
            }
            if complete && step.complete.is_none() {
                step.complete = Some(n);
            }
            quiet = if self.moving() { 0 } else { quiet + 1 };
            if quiet >= 3 {
                break;
            }
        }
        self.write("rest");
        self.save_pictures();
    }

    fn close(&mut self, name: &str) {
        self.begin(name, Logged::Turns);
        self.around = None;
        self.pointer = None;
        self.turns(3);
        self.write("rest");
    }

    /// Where something that does `action` is, among what shows.
    fn find(&self, action: Action) -> Option<(usize, Pos2)> {
        self.panels.all().enumerate().filter(|(_, p)| p.shown && p.painted).find_map(|(i, p)| p.items.iter().find(|item| item.action == action).map(|item| (i, item.rect.center())))
    }

    /// Points at what does `action`, presses and lets go, a turn each as a hand takes them, and the hand goes. Says whether there was such a thing.
    fn press(&mut self, action: Action) -> bool {
        // The wing is hidden with a page up: go back through its real Close control first.
        if self.find(action).is_none() && self.panels.sheet_open() && action != Action::CloseSheet {
            self.press(Action::CloseSheet);
            self.rest();
        }
        if self.find(action).is_none() && !matches!(action, Action::All | Action::Back | Action::CloseSheet) && self.find(Action::All).is_some() {
            self.press(Action::All);
            self.rest();
        }
        let Some(at) = self.find(action) else {
            self.notes.push(format!("{} {}: nothing does {action:?}", self.mode.name(), self.step)); // i18n: not shown (a developer's benchmark command)
            return false;
        };
        self.pointer = Some(at);
        self.turn(None);
        self.turn(Some(Act::Press));
        self.turn(Some(Act::Release));
        self.pointer = None;
        true
    }

    /// A step that presses `action` and rests.
    fn pressing(&mut self, name: &str, action: Action, logged: Logged) {
        self.begin(name, logged);
        self.press(action);
        self.rest();
    }

    /// Points at everything that can be pressed, one after the other, two turns each; on the sheet's page when `sheet`, on the parts around the menu otherwise.
    fn point_at_each(&mut self, name: &str, sheet: bool, logged: Logged) {
        self.begin(name, logged);
        let parts = self.panels.list.len();
        let targets: Vec<(usize, Pos2)> = self
            .panels
            .all()
            .enumerate()
            .filter(|(i, p)| p.shown && p.painted && (*i == parts) == sheet)
            .flat_map(|(i, p)| p.items.iter().filter(|item| item.action != Action::Idle).map(move |item| (i, item.rect.center())))
            .collect();
        for target in targets {
            // The parts can change under the hand (a second ring comes out): it points only at what is still there.
            if self.panels.all().nth(target.0).is_none() {
                continue;
            }
            self.pointer = Some(target);
            self.turns(2);
        }
        self.pointer = None;
        // A second ring stays a moment after the hand left it, by the clock.
        if logged == Logged::Rest {
            std::thread::sleep(super::BLOOM_GRACE + Duration::from_millis(60));
        }
        self.rest();
    }

    /// The list on the sheet moved down and back up: by the mouse's wheel on a desktop, by the stick in a headset.
    fn scroll(&mut self, name: &str, logged: Logged) {
        self.begin(name, logged);
        for way in [-1.0f32, 1.0] {
            match self.mode {
                Mode::Desktop => {
                    for _ in 0..3 {
                        self.turn(Some(Act::Wheel(way * 2.0)));
                        self.turns(6);
                    }
                }
                Mode::Vr => {
                    for _ in 0..45 {
                        self.turn(Some(Act::Stick(way)));
                    }
                    self.turn(Some(Act::Stick(0.0)));
                }
            }
            self.rest();
        }
    }
}

/// What a person does, the same in both modes.
fn script(rig: &mut Rig) {
    use Logged::{Rest, Turns};
    rig.begin("closed", Turns);
    rig.turns(3);
    // The Quick Menu, three times: the first opening of the process, and two more as a person opens it all evening.
    rig.open("qm-open-1", Around::QuickMenu);
    rig.point_at_each("qm-point", false, Turns);
    rig.ticks("qm-ticks");
    rig.facts_cost("the Quick Menu, no sheet"); // i18n: not shown (benchmark report labels)
    rig.close("qm-close-1");
    rig.open("qm-open-2", Around::QuickMenu);
    // The sheets from it. Their lists are read on other threads: at rest only.
    rig.pressing("qm-sheet-all", Action::All, Rest);
    rig.point_at_each("qm-sheet-all-point", true, Rest);
    rig.ticks("qm-sheet-all-ticks");
    rig.facts_cost("All tools"); // i18n: not shown (benchmark report labels)
    rig.scroll("qm-sheet-all-scroll", Rest);
    rig.pressing("qm-sheet-pictures", Action::Pictures, Rest);
    rig.panels.recents.keep(1, 0);
    if let Some(panel) = rig.panels.takeover.as_mut() { panel.pictures.clear(); panel.dirty = true; }
    rig.scroll("pictures-cold-ingestion-scroll", Rest);
    rig.panels.recents.keep(1, 0);
    if let Some(panel) = rig.panels.takeover.as_mut() { panel.pictures.clear(); panel.dirty = true; }
    rig.begin("pictures-cold-ingestion-fling", Rest);
    rig.panels.throw = 1800.0;
    rig.turns(90);
    rig.rest();
    rig.panels.facts.gallery_scroll = 0.0;
    rig.panels.gallery_to = 0.0;
    rig.panels.throw = 0.0;
    if let Some(panel) = rig.panels.takeover.as_mut() { panel.dirty = true; }
    rig.facts_cost("Pictures, loaded"); // i18n: not shown (benchmark report labels)
    rig.scroll("qm-sheet-pictures-scroll", Rest);
    rig.begin("pictures-fling", Rest);
    rig.panels.throw = 1800.0;
    rig.turns(90);
    rig.rest();
    for round in 0..2 {
        for sheet in [super::panels::Sheet::All, super::panels::Sheet::Pictures, super::panels::Sheet::Settings, super::panels::Sheet::Tracking, super::panels::Sheet::Mics, super::panels::Sheet::Booster, super::panels::Sheet::Talk, super::panels::Sheet::Instances, super::panels::Sheet::Results, super::panels::Sheet::Menu, super::panels::Sheet::Worlds, super::panels::Sheet::Pictures] {
            rig.begin(&format!("switch-{round}-{sheet:?}"), Rest);
            let opening = Instant::now();
            match sheet {
                super::panels::Sheet::Worlds => { rig.panels.act(Action::Worlds, &rig.snap, &mut super::Pressed::default()); }
                _ => {
                    if sheet == super::panels::Sheet::Menu { rig.panels.stage.host.menu.page = orbiters_n4n0_core::menu::Page::States; }
                    if sheet == super::panels::Sheet::Instances { rig.panels.stage.host.menu.page = orbiters_n4n0_core::menu::Page::Instances; }
                    if sheet == super::panels::Sheet::Results { rig.panels.stage.host.menu.page = orbiters_n4n0_core::menu::Page::Results; }
                    rig.panels.open_sheet(sheet, "", &rig.snap);
                }
            }
            let opening = opening.elapsed();
            rig.turns(1);
            rig.steps.last_mut().unwrap().turns.last_mut().unwrap().open += opening;
            rig.write("switch-next-turn");
            if round == 0 { rig.rest(); }
        }
    }
    rig.rest();
    // Every shared menu screen, with first visits settled and a second rapid sweep.
    use orbiters_n4n0_core::menu::Page;
    for round in 0..2 {
        for page in [Page::Home, Page::Search, Page::Results, Page::Call, Page::Photo, Page::Music, Page::Light, Page::States, Page::Card, Page::NewState, Page::Type, Page::Tracking, Page::Suggest, Page::More, Page::Instances, Page::AddFriend, Page::Qr, Page::History, Page::ShareTo, Page::Explicit, Page::Feedback, Page::FeedbackLabels, Page::Comments, Page::Mirror, Page::States] {
            rig.begin(&format!("menu-switch-{round}-{page:?}"), Rest);
            let at = Instant::now();
            rig.panels.stage.host.menu.page = page;
            rig.panels.open_sheet(super::panels::Sheet::Menu, "", &rig.snap);
            let opening = at.elapsed(); rig.turns(1);
            rig.steps.last_mut().unwrap().turns.last_mut().unwrap().open += opening;
            rig.write("switch-next-turn"); if round == 0 { rig.rest(); }
        }
    }
    rig.rest();
    rig.begin("return-pictures-after-menu", Rest);
    rig.panels.open_sheet(super::panels::Sheet::Pictures, "", &rig.snap); rig.turns(1); rig.rest();
    rig.begin("live-color-scroll", Rest);
    let from_scroll = rig.panels.facts.gallery_scroll;
    let mut moved = false;
    for n in 0..90 {
        if n % 15 == 0 { Arc::make_mut(&mut rig.snap).settings.orbit.retro_color = if n % 30 == 0 { [170,30,210] } else { [0,190,160] }; }
        rig.turn(Some(Act::Wheel(-1.0)));
        moved |= (rig.panels.facts.gallery_scroll - from_scroll).abs() > 0.01;
    }
    assert!(moved, "live color scrolling must move the gallery, not snap to the same row");
    rig.rest();
    rig.point_at_each("qm-sheet-pictures-point", true, Rest);
    rig.pressing("qm-sheet-instances", Action::Page(Target::Instances), Rest);
    rig.pressing("qm-sheet-all-again", Action::All, Rest);
    rig.pressing("qm-sheet-settings", Action::Settings, Rest);
    rig.point_at_each("qm-sheet-settings-point", true, Rest);
    rig.pressing("qm-sheet-talk", Action::Talk, Rest);
    for n in 0..4 {
        if rig.panels.facts.sheet.is_none() {
            break;
        }
        rig.pressing(&format!("qm-back-{n}"), Action::Back, Rest);
    }
    rig.close("qm-close-2");
    rig.open("qm-open-3", Around::QuickMenu);
    rig.close("qm-close-3");
    // The wheel, the same.
    rig.open("wheel-open-1", Around::Wheel);
    rig.point_at_each("wheel-point", false, Rest);
    rig.ticks("wheel-ticks");
    rig.facts_cost("the wheel, no sheet"); // i18n: not shown (benchmark report labels)
    rig.close("wheel-close-1");
    rig.open("wheel-open-2", Around::Wheel);
    rig.pressing("wheel-sheet-all", Action::All, Rest);
    rig.pressing("wheel-sheet-pictures", Action::Pictures, Rest);
    rig.close("wheel-close-2");
    rig.open("wheel-open-3", Around::Wheel);
    rig.close("wheel-close-3");
    // And from one to the other, as VRChat's own menus go.
    rig.open("qm-open-4", Around::QuickMenu);
    rig.open("wheel-open-4", Around::Wheel);
    rig.open("qm-open-5", Around::QuickMenu);
    // Left open on a sheet and closed under it, then opened again: nothing of the sheet may be there.
    rig.pressing("qm-sheet-settings-left", Action::Settings, Rest);
    rig.begin("qm-point-left", Turns);
    if let Some(at) = rig.find(Action::Pictures) {
        rig.pointer = Some(at);
        rig.turns(2);
        rig.turn(Some(Act::Press));
    }
    let pointer = rig.pointer;
    rig.close("qm-close-held");
    rig.pointer = pointer;
    rig.open("qm-open-6", Around::QuickMenu);
    rig.close("qm-close-6");
}

/// The memory the parts hold now: each part's two pictures, its shape, and its interface's own font atlas (once in the interface, once in what paints it).
fn memory(panels: &Panels) -> (usize, u64, u64) {
    let (mut parts, mut pictures, mut atlases) = (0, 0u64, 0u64);
    let mut count = |panel: &Panel| {
        parts += 1;
        let px = panel.px[0] as u64 * panel.px[1] as u64;
        pictures += px * 8 + panel.mask.as_ref().map_or(0, |m| m.len() as u64);
        atlases += panel.canvas.atlas_bytes() as u64;
    };
    panels.all().for_each(&mut count);
    (parts, pictures, atlases)
}

fn summary(mode: Mode, run: usize, rig: &Rig, out: &mut String) {
    use std::fmt::Write as _;
    let turn = mode.turn() as f64 * 1000.0;
    let _ = writeln!(out, "\n## {} run {run}\n", mode.name());
    let _ = writeln!(out, "| step | turns | longest turn ms (set_open / follow / hand / paint / upload) | all turns ms | of which upload ms | pictures handed over | megapixels handed over |");
    let _ = writeln!(out, "|---|---:|---|---:|---:|---:|---:|");
    for step in &rig.steps {
        let Some(longest) = step.turns.iter().max_by_key(|c| c.whole()) else { continue };
        let total: Duration = step.turns.iter().map(Cost::whole).sum();
        let _ = writeln!(
            out,
            "| {} | {} | {:.2} ({:.2} / {:.2} / {:.2} / {:.2} / {:.2}) | {:.2} | {:.2} | {} | {:.2} |",
            step.name,
            step.turns.len(),
            ms(longest.whole()),
            ms(longest.open),
            ms(longest.follow),
            ms(longest.hand),
            ms(longest.paint),
            ms(longest.upload),
            ms(total),
            ms(step.turns.iter().map(|c| c.upload).sum()),
            step.turns.iter().map(|c| c.shows).sum::<u32>(),
            step.turns.iter().map(|c| c.pixels).sum::<u64>() as f64 / 1e6,
        );
    }
    let _ = writeln!(out, "\n| step | completed page frames | active wall seconds | completed page frames/s |\n|---|---:|---:|---:|");
    for step in &rig.steps {
        let frames = step.turns.iter().filter(|c| c.worker.raster > Duration::ZERO).count();
        let wall: Duration = step.turns.iter().map(|c| c.wall).sum();
        let _ = writeln!(out, "| {} | {frames} | {:.3} | {:.2} |", step.name, wall.as_secs_f64(), frames as f64 / wall.as_secs_f64().max(0.001));
    }
    let _ = writeln!(out, "\n| step | phase | p50 ms | p95 ms | p99 ms | max ms |\n|---|---|---:|---:|---:|---:|");
    for step in &rig.steps {
        for phase in 0..9 {
            let mut values: Vec<f64> = step.turns.iter().map(|c| ms(match phase {
                0 => c.whole(), 1 => c.phases.text, 2 => c.phases.tessellation,
                3 => c.phases.raster, 4 => c.phases.finish,
                7 => c.phases.facts, 8 => c.worker.facts,
                6 => c.worker.facts + c.worker.text + c.worker.tessellation + c.worker.raster + c.worker.finish,
                _ => c.paint.saturating_sub(c.phases.text + c.phases.tessellation + c.phases.raster + c.phases.finish),
            })).collect();
            values.sort_by(f64::total_cmp);
            if values.is_empty() { continue; }
            let percentile = |p: f64| values[((values.len() as f64 * p).ceil() as usize).saturating_sub(1)];
            let label = ["turn", "text/layout", "textures/tessellation", "raster", "finish/copy", "paint remainder", "worker", "facts/items", "worker items"][phase];
            let _ = writeln!(out, "| {} | {label} | {:.3} | {:.3} | {:.3} | {:.3} |", step.name, percentile(0.50), percentile(0.95), percentile(0.99), values[values.len()-1]);
        }
    }
    for (state, made, copied) in &rig.facts {
        let _ = writeln!(out, "\nFacts, {state}: made again and compared {made:.1} us a turn; copied {copied:.1} us a part painted.");
    }
    if !rig.for_nothing.is_empty() {
        let mut by_step: std::collections::BTreeMap<&str, Vec<String>> = Default::default();
        for ((step, zone), times) in &rig.for_nothing {
            by_step.entry(step).or_default().push(format!("{zone} {times}"));
        }
        let _ = writeln!(out, "\nParts painted in a turn that handed nothing over (times, by step):\n");
        for (step, parts) in by_step {
            let _ = writeln!(out, "- {step}: {}", parts.join(", "));
        }
    }
    let _ = writeln!(out, "\nOpenings: the work until every part has a picture and until none is due another, and when that is on the loop's clock (a turn every {turn:.0} ms, a longer one running late).\n");
    let _ = writeln!(out, "| opening | first turn ms | every part pictured: turn, work ms, on the clock ms | none due another: turn, work ms, on the clock ms |");
    let _ = writeln!(out, "|---|---:|---|---|");
    for step in rig.steps.iter().filter(|s| s.pictured.is_some()) {
        let until = |n: usize| -> (f64, f64) {
            let work: f64 = step.turns[..=n].iter().map(|c| ms(c.whole())).sum();
            let clock: f64 = step.turns[..n].iter().map(|c| ms(c.whole()).max(turn)).sum::<f64>() + ms(step.turns[n].whole());
            (work, clock)
        };
        let first = step.turns.first().map_or(0.0, |c| ms(c.whole()));
        let cell = |n: Option<usize>| n.map_or("never".to_string(), |n| { let (work, clock) = until(n); format!("{}, {work:.2}, {clock:.2}", n + 1) }); // i18n: not shown (a developer's benchmark command)
        let _ = writeln!(out, "| {} | {first:.2} | {} | {} |", step.name, cell(step.pictured), cell(step.complete));
    }
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let runs: usize = value("--runs").and_then(|v| v.parse().ok()).unwrap_or(5);
    GALLERY.store(value("--entries").and_then(|v| v.parse().ok()).unwrap_or(60), std::sync::atomic::Ordering::Relaxed);
    if let Some(tag) = value("--language") { if let Some(language) = orbiters_i18n::normalize(&tag) { orbiters_i18n::pin_language(language); orbiters_ui::fonts::prefer_language(language.tag()); } }
    let only = value("--only");
    let upload = args.iter().any(|a| a == "--upload");
    RETRO.store(args.iter().any(|a| a == "--retro"), std::sync::atomic::Ordering::Relaxed);
    let synchronous = args.iter().any(|a| a == "--sync");
    let fonts_anew = args.iter().any(|a| a == "--fonts-anew");
    let out = value("--out").map(PathBuf::from);
    // Nothing of the app's own folders: all of it in a scratch that goes when this ends.
    let roots = orbiters_core::paths::roots().context("paint bench requires the harness bootstrap")?;
    let scratch = roots.config.parent().context("private root")?.to_path_buf();
    for sub in ["config", "local", "cache", "documents", "sessions"] {
        std::fs::create_dir_all(scratch.join(sub))?;
    }
    music(83);
    let pictures = make_pictures(&scratch.join("pictures"))?;
    let _ = PICTURES.set(pictures);
    if let Some(out) = &out {
        std::fs::create_dir_all(out)?;
    }
    let mut report = String::from("# orbit paint bench\n"); // i18n: not shown (a developer's benchmark command)
    for mode in [Mode::Vr, Mode::Desktop] {
        if only.as_deref().is_some_and(|o| o != mode.name()) {
            continue;
        }
        // First, once, what is drawn; then the timings, with nothing written between turns.
        if let Some(out) = &out {
            let dir = out.join("pictures");
            std::fs::create_dir_all(&dir)?;
            let log = std::fs::File::create(out.join(format!("pictures-{}.txt", mode.name())))?; // i18n: not shown (a developer's benchmark command)
            let mut rig = Rig::new(mode, &scratch, Some((log, dir)));
            rig.panels.fonts_anew = fonts_anew;
            rig.panels.stage.async_pages = !synchronous;
            script(&mut rig);
            if let Some(log) = rig.log.as_mut() {
                log.flush()?;
            }
            let (parts, pictures, atlases) = memory(&rig.panels);
            use std::fmt::Write as _;
            let _ = writeln!(report, "\n{}: {parts} parts at the end of the script hold {:.1} MB of pictures and {:.1} MB of font atlases.", mode.name(), pictures as f64 / 1e6, atlases as f64 / 1e6);
            let (kept, bytes) = rig.panels.resting();
            let _ = writeln!(report, "{}: {kept} parts of menus that closed are kept at the end of the script, holding {:.1} MB; most at once in the script {} parts, {:.1} MB.", mode.name(), bytes as f64 / 1e6, rig.most_kept.0, rig.most_kept.1 as f64 / 1e6);
            for note in &rig.notes {
                let _ = writeln!(report, "note: {note}");
            }
        }
        for run in 1..=runs {
            let mut rig = Rig::new(mode, &scratch, None);
            rig.panels.fonts_anew = fonts_anew;
            rig.panels.stage.async_pages = !synchronous;
            if upload {
                rig.gpu = Some(orbiters_openvr::gpu::OverlayTextures::new(None).map_err(|e| anyhow::anyhow!(e))?);
            }
            script(&mut rig);
            for note in &rig.notes {
                use std::fmt::Write as _;
                let _ = writeln!(report, "note: {note}");
            }
            summary(mode, run, &rig, &mut report);
            if let Some(out) = &out {
                let mut csv = String::from("step,turn,set_open_ms,follow_ms,hand_ms,paint_ms,upload_ms,shows,pixels,wall_ms,worker_text_ms,worker_tess_ms,worker_raster_ms,worker_finish_ms,facts_items_ms,text_ms,tess_ms,raster_ms,finish_ms,worker_items_ms\n"); // i18n: not shown (machine-readable benchmark column names)
                for step in &rig.steps {
                    for (n, c) in step.turns.iter().enumerate() {
                        use std::fmt::Write as _;
                        let _ = writeln!(csv, "{},{n},{:.4},{:.4},{:.4},{:.4},{:.4},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}", step.name, ms(c.open), ms(c.follow), ms(c.hand), ms(c.paint), ms(c.upload), c.shows, c.pixels, ms(c.wall), ms(c.worker.text), ms(c.worker.tessellation), ms(c.worker.raster), ms(c.worker.finish), ms(c.phases.facts), ms(c.phases.text), ms(c.phases.tessellation), ms(c.phases.raster), ms(c.phases.finish), ms(c.worker.facts));
                    }
                }
                std::fs::write(out.join(format!("turns-{}-{run}.csv", mode.name())), csv)?; // i18n: not shown (a developer's benchmark command)
            }
        }
    }
    println!("{report}");
    if let Some(out) = &out {
        std::fs::write(out.join("report.md"), &report)?;
    }
    Ok(())
}

#[cfg(test)]
mod review_fixture {
    use super::*;
    #[test]
    fn review_world_fixture_waits_for_lazy_initialization_and_cannot_be_skipped() {
        let _one = super::super::flows::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let mut panels = Panels::new(Arc::new(crate::screenshot::Still), 2.0, true);
        let found = orbiters_core::maps::FoundWorld { world: orbiters_core::maps::World { world_id: "wrld_fixture".into(), ..Default::default() }, ..Default::default() };
        let entry = crate::overlay::worlds::catalog::Entry::world(&found, None, Default::default());
        let mut fixture = Some((vec![entry], vec![egui::ColorImage::filled([4,4], egui::Color32::RED)]));
        register_world_fixture(&mut panels, &mut fixture);
        assert!(panels.stage.worlds.is_none() && fixture.is_some(), "keep assets pending until the lazy module exists");
        let snap = crate::screenshot::idle_snapshot(&Settings::default(), &[]);
        panels.takeover_place = Some(zones::in_units(Zone::Takeover));
        panels.open_sheet(super::super::panels::Sheet::Settings, "", &snap);
        register_world_fixture(&mut panels, &mut fixture);
        assert!(fixture.is_none() && panels.stage.worlds.as_ref().unwrap().paint_fixture_ready());
    }
}
