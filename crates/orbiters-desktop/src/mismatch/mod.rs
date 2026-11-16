//! The beta's menu mismatch buttons (Enzo, 2026-11-16): with Settings › Beta feedback › "Menu mismatch buttons" on, three buttons stay in
//! view, on the desktop in a corner of VRChat's window and in VR on the watch's wrist: "Menu should be off", "Wheel should be on", "Quick menu
//! should be on". A tester presses one when N4N0's menus do not match VRChat's; the press is kept as a report of what led to it, so that an
//! agent can later make the rules that follow VRChat's menus better from real cases (`docs/menu-mismatch-reports.md`, `cargo xtask
//! menu-reports`). A press only reports: it never changes what the menus do.
//!
//! **What is kept**, in each process that draws menus (the VR overlay, the desktop-mode panels), only while the switch is on: a ring of the
//! last [`KEEP`] of lines ([`note`]): the inputs the menu logic reads (menu buttons, triggers, sticks, keys Esc / R / Left Alt / numbers),
//! its decisions and their reasons (what it takes as open and why, the Quick Menu's look at the headset's picture, a desktop look at
//! VRChat's window), every line of `vr-test.log`, and what VRChat says (its camera, the instance). A press waits [`AFTER`] and writes the
//! ring's last 30 s and those few seconds into a report, with the build, the controllers and their binding, the menus' state at the press,
//! the lines of VRChat's log about its menus and worlds (never people), and, when its own switch is on, a small blurred picture of the game.
//!
//! **Never**: anything typed (no key but Esc, R, Left Alt, Alt+F7..F9 and the numbers is read), anything said, names of people (VRChat's log
//! lines with a person in them are dropped), the account. The picture is of the game, so it can show what the game showed: it is its own
//! switch, off by default, and is made too small to read names.
//!
//! Costs: off, one atomic load per [`note`]; on, a line pushed behind a mutex only when something changes.

pub mod send;

use orbiters_i18n::mark;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How far back a report reaches before its press.
pub const BEFORE: Duration = Duration::from_secs(30);
/// How long after a press it waits before it is written, so that what VRChat and the menus did right after is in it.
pub const AFTER: Duration = Duration::from_secs(4);
/// What the ring keeps: a little more than a report needs.
pub const KEEP: Duration = Duration::from_secs(40);
/// At most this many lines in the ring: more is a loop gone wrong, and the oldest go.
pub const MOST: usize = 6_000;
/// The longest line kept.
pub const LONGEST: usize = 300;
/// A report's file, and its picture's, in its folder.
pub const REPORT: &str = "report.json";
pub const PICTURE: &str = "picture.png";
/// In a report's folder once Orbiters took it.
pub const SENT: &str = "sent";
/// The format written: an agent reading a report checks it.
pub const FORMAT: u32 = 1;

/// What a press says. A new kind is one more variant here, in [`Kind::ALL`] and its words: the desktop's corner and the VR strip show
/// every kind of [`Kind::ALL`], in that order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// N4N0 shows a menu of its own (around the wheel, beside the Quick Menu) while VRChat has none of these up. Covers "wheel should be
    /// off" and "quick menu should be off" too: whichever of ours shows, it should not.
    MenuShouldBeOff,
    /// VRChat's wheel (action menu) is up and N4N0's ring is not (none of ours shows, or ours beside the Quick Menu).
    WheelShouldBeOn,
    /// VRChat's Quick Menu is up and N4N0's parts beside it are not (none shows, or the ring).
    QuickMenuShouldBeOn,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::MenuShouldBeOff, Kind::WheelShouldBeOn, Kind::QuickMenuShouldBeOn];

    /// Its name in files and on the server.
    pub fn key(self) -> &'static str {
        match self {
            Kind::MenuShouldBeOff => "menu-should-be-off",
            Kind::WheelShouldBeOn => "wheel-should-be-on",
            Kind::QuickMenuShouldBeOn => "quick-menu-should-be-on",
        }
    }

    pub fn from_key(key: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.key() == key)
    }

    /// Its words on the button (Enzo's).
    pub fn words(self) -> &'static str {
        match self {
            Kind::MenuShouldBeOff => mark("Menu should be off"),
            Kind::WheelShouldBeOn => mark("Wheel should be on"),
            Kind::QuickMenuShouldBeOn => mark("Quick menu should be on"),
        }
    }

    /// Its function key on the desktop with Left Alt held (Alt+F7, Alt+F8, Alt+F9), which works while VRChat's wheel holds the pointer.
    /// Not numbers: those press the ring's cells (and the second ring's, 7 and 8), and AltGr+number types characters on many keyboards.
    pub fn function_key(self) -> u8 {
        7 + Kind::ALL.iter().position(|k| *k == self).unwrap_or(0) as u8
    }
}

/// What a button says for a moment after its press.
pub const NOTED_WORDS: &str = mark("Noted");

/// One line of the ring: when (ms since 1970, UTC), from what, and what.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub at: u64,
    pub source: String,
    pub text: String,
}

/// The ring itself: lines in time order, the oldest let go past [`KEEP`] or [`MOST`].
#[derive(Debug, Default)]
pub struct Ring {
    lines: VecDeque<Line>,
}

impl Ring {
    pub fn push(&mut self, at: u64, source: &str, text: String) {
        let mut text = text;
        if text.len() > LONGEST {
            let mut cut = LONGEST;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push('…');
        }
        self.lines.push_back(Line { at, source: source.to_owned(), text });
        let oldest = at.saturating_sub(KEEP.as_millis() as u64);
        while self.lines.front().is_some_and(|l| l.at < oldest) || self.lines.len() > MOST {
            self.lines.pop_front();
        }
    }

    /// The lines from `from` to `to` (ms), in order.
    pub fn between(&self, from: u64, to: u64) -> Vec<Line> {
        self.lines.iter().filter(|l| l.at >= from && l.at <= to).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

static ON: AtomicBool = AtomicBool::new(false);
static RING: Mutex<Ring> = Mutex::new(Ring { lines: VecDeque::new() });

fn ring<R>(f: impl FnOnce(&mut Ring) -> R) -> R {
    f(&mut RING.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// Milliseconds since 1970, UTC.
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The switches as a process that draws menus reads them: the settings; in the VR sandbox (a debug build it started), also on while its logs folder
/// holds `mismatch-on.txt`, so that a test turn does not change the settings other turns read.
pub fn switches(settings: &orbiters_core::settings::Settings) -> orbiters_core::settings::MismatchSettings {
    if cfg!(debug_assertions) && std::env::var_os("ORBITERS_VR_SANDBOX_ROOT").is_some() {
        static LOOKED: Mutex<Option<(std::time::Instant, bool)>> = Mutex::new(None);
        let mut looked = LOOKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if looked.is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(1)) {
            let on = std::env::var_os("ORBITERS_SANDBOX_LOGS").is_some_and(|d| PathBuf::from(d).join("mismatch-on.txt").is_file());
            *looked = Some((std::time::Instant::now(), on));
        }
        if looked.is_some_and(|(_, on)| on) {
            return orbiters_core::settings::MismatchSettings { buttons: true, picture: true };
        }
    }
    settings.mismatch
}

/// Whether the buttons are on, as this process last heard.
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// The setting as the process sees it now: off forgets everything kept.
pub fn set_on(on: bool) {
    if ON.swap(on, Ordering::Relaxed) && !on {
        ring(Ring::clear);
    }
}

/// A line for the ring, made only while the buttons are on. Tests note nothing: they run side by side.
pub fn note(source: &str, text: impl FnOnce() -> String) {
    if cfg!(test) || !on() {
        return;
    }
    let text = text();
    ring(|r| r.push(now_ms(), source, text));
}

/// Notes `now` under `source` when it differs from what was noted there last (`was`): for states read every turn.
pub fn note_change<T: PartialEq + std::fmt::Debug>(source: &str, was: &mut Option<T>, now: T) {
    if was.as_ref() == Some(&now) {
        return;
    }
    if on() {
        let text = match was {
            Some(old) => format!("{old:?} -> {now:?}"), // i18n: not shown (a menu mismatch report's line)
            None => format!("{now:?}"),
        };
        note(source, || text);
    }
    *was = Some(now);
}

/// What the process says of itself and its menus at the press.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Context {
    /// "vr" or "desktop".
    pub surface: String,
    /// Each hand's controller type (SteamVR's `controller_type`), left then right; none on the desktop.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controllers: Vec<Option<String>>,
    /// The binding SteamVR uses for N4N0's actions (a saved one or ours), per controller type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bindings: Vec<(String, String)>,
    /// The menus' state then: what is taken as open and why, what of ours shows, the window's size, and so on.
    pub state: Vec<(String, String)>,
}

/// A report, as written in `report.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub format: u32,
    pub id: String,
    pub kind: Kind,
    /// When it was pressed, UTC, ISO and ms since 1970.
    pub pressed_at: String,
    pub pressed_ms: u64,
    pub build: String,
    pub os: String,
    pub language: String,
    pub surface: String,
    #[serde(default)]
    pub controllers: Vec<Option<String>>,
    #[serde(default)]
    pub bindings: Vec<(String, String)>,
    pub state: Vec<(String, String)>,
    /// The settings that shape the menus.
    pub settings: serde_json::Value,
    /// The ring's lines from [`BEFORE`] the press to [`AFTER`] it: `t` is ms from the press (negative before).
    pub timeline: Vec<Entry>,
    /// VRChat's log lines about its menus, input and worlds in the same time, never one with a person in it.
    pub vrchat_log: Vec<Entry>,
    /// The picture's file in the report's folder, when there is one.
    pub picture: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub t: i64,
    pub source: String,
    pub text: String,
}

/// A small picture of the game at the press: RGBA, `width` × `height`.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The widest picture kept: names in the world and on nameplates are not readable at this size, what menus show is.
pub const PICTURE_WIDTH: u32 = 400;

impl Picture {
    /// Made smaller (an average over each block, which also blurs) to at most [`PICTURE_WIDTH`] wide.
    pub fn small(width: u32, height: u32, rgba: &[u8]) -> Option<Picture> {
        if width == 0 || height == 0 || rgba.len() < (width * height * 4) as usize {
            return None;
        }
        let step = width.div_ceil(PICTURE_WIDTH).max(1);
        let (w, h) = (width / step, height / step);
        let mut out = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0u32; 4];
                for dy in 0..step {
                    for dx in 0..step {
                        let i = (((y * step + dy) * width + x * step + dx) * 4) as usize;
                        for c in 0..4 {
                            sum[c] += rgba[i + c] as u32;
                        }
                    }
                }
                let o = ((y * w + x) * 4) as usize;
                for c in 0..3 {
                    out[o + c] = (sum[c] / (step * step)) as u8;
                }
                out[o + 3] = 255;
            }
        }
        Some(Picture { width: w, height: h, rgba: out })
    }
}

/// Where reports are kept: `menu-reports` in the local data folder (a debug build started by the VR sandbox keeps them in the sandbox's
/// folder, as its logs: a test build never writes the person's own).
pub fn folder() -> PathBuf {
    if cfg!(debug_assertions) {
        if let Some(dir) = std::env::var_os("ORBITERS_SANDBOX_LOGS").filter(|d| !d.is_empty()) {
            return PathBuf::from(dir).join("menu-reports");
        }
    }
    orbiters_core::paths::local_data_root().join("menu-reports")
}

/// A press: what to keep is taken now and written in [`AFTER`], on a thread of its own, into a new folder of [`folder`]. Returns that
/// folder's path (it exists once the report is written).
pub fn press(kind: Kind, context: Context, settings: &orbiters_core::settings::Settings, vrchat_log: Option<PathBuf>, picture: Option<Picture>) -> PathBuf {
    let pressed = now_ms();
    let id = uuid::Uuid::new_v4().simple().to_string();
    let dir = folder().join(format!("{}-{}-{}", stamp(pressed), kind.key(), &id[..6]));
    note("press", || format!("{} pressed", kind.key())); // i18n: not shown (a menu mismatch report's line)
    let settings = menu_settings(settings);
    let out = dir.clone();
    let _ = std::thread::Builder::new().name("mismatch-report".into()).spawn(move || {
        std::thread::sleep(AFTER);
        let lines = ring(|r| r.between(pressed.saturating_sub(BEFORE.as_millis() as u64), pressed + AFTER.as_millis() as u64));
        let log = vrchat_log.as_deref().map(|p| vrchat_lines(p, pressed)).unwrap_or_default();
        let report = build(kind, id, pressed, context, settings, lines, log, picture.is_some());
        if let Err(e) = write(&out, &report, picture.as_ref()) {
            eprintln!("menu mismatch: the report could not be written in {}: {e}", out.display()); // i18n: not shown (a log's line)
        }
    });
    dir
}

/// The report from what was kept.
#[allow(clippy::too_many_arguments)]
pub fn build(kind: Kind, id: String, pressed: u64, context: Context, settings: serde_json::Value, lines: Vec<Line>, vrchat_log: Vec<Entry>, picture: bool) -> Report {
    let rel = |at: u64| at as i64 - pressed as i64;
    Report {
        format: FORMAT,
        id,
        kind,
        pressed_at: iso(pressed),
        pressed_ms: pressed,
        build: env!("CARGO_PKG_VERSION").to_owned(),
        os: std::env::consts::OS.to_owned(),
        language: orbiters_i18n::language().tag().to_owned(),
        surface: context.surface,
        controllers: context.controllers,
        bindings: context.bindings,
        state: context.state,
        settings,
        timeline: lines.into_iter().map(|l| Entry { t: rel(l.at), source: l.source, text: l.text }).collect(),
        vrchat_log,
        picture: picture.then(|| PICTURE.to_owned()),
    }
}

/// Writes a report's folder: its JSON, then its picture.
pub fn write(dir: &Path, report: &Report, picture: Option<&Picture>) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    if let Some(p) = picture {
        let file = std::fs::File::create(dir.join(PICTURE))?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), p.width, p.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&p.rgba)?;
    }
    // The JSON last: a folder with it is a whole report.
    let tmp = dir.join("report.json.part");
    std::fs::write(&tmp, serde_json::to_vec_pretty(report)?)?;
    std::fs::rename(tmp, dir.join(REPORT))?;
    Ok(())
}

/// The settings that shape what the menus do, for the report.
pub fn menu_settings(s: &orbiters_core::settings::Settings) -> serde_json::Value {
    serde_json::json!({
        "orbit": serde_json::to_value(s.orbit).unwrap_or_default(),
        "floating_watch": s.floating.watch,
        "watch_hand": serde_json::to_value(s.floating.watch_hand).unwrap_or_default(),
        "osc_enabled": s.osc.enabled,
        "reduce_motion": s.reduce_motion,
    })
}

/// A report kept on this computer: its folder, what it says of itself, and whether Orbiters took it.
#[derive(Clone, Debug)]
pub struct Kept {
    pub dir: PathBuf,
    pub kind: Option<Kind>,
    pub sent: bool,
}

/// The reports in [`folder`], oldest first. A folder without its `report.json` (still being written, or broken) is not one.
pub fn kept() -> Vec<Kept> {
    kept_in(&folder())
}

pub fn kept_in(root: &Path) -> Vec<Kept> {
    let Ok(read) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<Kept> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join(REPORT).is_file())
        .map(|dir| {
            let name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let kind = Kind::ALL.into_iter().find(|k| name.contains(k.key()));
            Kept { sent: dir.join(SENT).exists(), kind, dir }
        })
        .collect();
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// Deletes every report kept (the folder stays, empty). Returns how many went.
pub fn delete_all() -> usize {
    let mut gone = 0;
    for k in kept() {
        if std::fs::remove_dir_all(&k.dir).is_ok() {
            gone += 1;
        }
    }
    gone
}

/// A file name's stamp for `ms`: `2026-11-16T10-22-33Z`.
fn stamp(ms: u64) -> String {
    iso(ms).replace(':', "-")
}

fn iso(ms: u64) -> String {
    let t = time::OffsetDateTime::from_unix_timestamp((ms / 1000) as i64).unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", t.year(), t.month() as u8, t.day(), t.hour(), t.minute(), t.second())
}

/// VRChat's log lines from [`BEFORE`] the press to [`AFTER`] it that say something of its menus, input, camera or worlds: from the end of
/// the file, at most 4 MB of it. A line with a person in it (a name, a user id) is never taken.
pub fn vrchat_lines(file: &Path, pressed: u64) -> Vec<Entry> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(file) else { return Vec::new() };
    let len = f.metadata().map_or(0, |m| m.len());
    let from = len.saturating_sub(4 << 20);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if f.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let offset = time::UtcOffset::current_local_offset().unwrap_or(time::UtcOffset::UTC);
    vrchat_lines_in(&String::from_utf8_lossy(&bytes), pressed, offset)
}

/// [`vrchat_lines`] over a log's text, its times read in `offset` (VRChat writes local time).
pub fn vrchat_lines_in(text: &str, pressed: u64, offset: time::UtcOffset) -> Vec<Entry> {
    let (from, to) = (pressed as i64 - BEFORE.as_millis() as i64, pressed as i64 + AFTER.as_millis() as i64);
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(at) = vrchat_time(line, offset) else { continue };
        // Lines carry seconds only: a second's margin either side.
        if at + 999 < from || at > to {
            continue;
        }
        let Some(said) = vrchat_said(line) else { continue };
        out.push(Entry { t: at - pressed as i64, source: "vrchat".into(), text: said });
    }
    out
}

/// "2026.11.16 10:22:33 Log        -  …": the time in ms since 1970.
fn vrchat_time(line: &str, offset: time::UtcOffset) -> Option<i64> {
    let stamp = line.get(..19)?;
    let n = |a: usize, b: usize| stamp.get(a..b)?.parse::<u32>().ok();
    let date = time::Date::from_calendar_date(n(0, 4)? as i32, time::Month::try_from(n(5, 7)? as u8).ok()?, n(8, 10)? as u8).ok()?;
    let at = time::Time::from_hms(n(11, 13)? as u8, n(14, 16)? as u8, n(17, 19)? as u8).ok()?;
    let t = time::PrimitiveDateTime::new(date, at).assume_offset(offset);
    Some((t.unix_timestamp_nanos() / 1_000_000) as i64)
}

/// What a line of VRChat's log says, when it is about menus, input, the camera, OSC or the world, and has no person in it.
pub fn vrchat_said(line: &str) -> Option<String> {
    let said = line.get(19..)?.trim_start();
    let said = said.split_once(" -  ").map_or(said, |(_, s)| s).trim();
    // An instance's id after the world's says who owns it: only the world is kept, before anything else is looked at.
    let said = match said.find("wrld_") {
        Some(i) => {
            let end = said[i..].find([':', ' ']).map_or(said.len(), |e| i + e);
            format!("{}{}", &said[..i], &said[i..end])
        }
        None => said.to_owned(),
    };
    let lower = said.to_lowercase();
    // A person: their name follows these, or their id is in it.
    const PERSON: [&str; 9] = ["usr_", "onplayer", "displayname", "player ", "user ", "avatar ", "invite", "friend", "@"]; // i18n: not shown (a menu mismatch report's line)
    if PERSON.iter().any(|p| lower.contains(p)) {
        return None;
    }
    const ABOUT: [&str; 14] = ["menu", "quickmenu", "actionmenu", "keyboard", "input", "camera", "osc", "joining wrld_", "entering room", "onleftroom", "left room", "vrcuimanager", "controller", "steamvr"]; // i18n: not shown (a menu mismatch report's line)
    if !ABOUT.iter().any(|a| lower.contains(a)) {
        return None;
    }
    let mut said = said;
    if said.len() > LONGEST {
        let mut cut = LONGEST;
        while !said.is_char_boundary(cut) {
            cut -= 1;
        }
        said.truncate(cut);
    }
    Some(said)
}

#[cfg(test)]
mod tests;
