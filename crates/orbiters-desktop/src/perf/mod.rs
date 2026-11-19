//! The performance self-check (N-friend-lag, 2026-11-16): what Orbiters - N4N0 costs this computer, kept always, so that a
//! person who lags can tell why without a debugger.
//!
//! - The overlay and the desktop panels measure their own loop ([`TurnMeter`]): turns a second, the time each part of a
//!   turn took (per second), the longest turn, the pictures handed to SteamVR and their size, and VRChat's frames as
//!   SteamVR's compositor saw them. A summary goes to the service every ten seconds ([`ProcessTurns`]).
//! - The service reads every process's processor time, memory, disk traffic and priority from one system call
//!   (`NtQuerySystemInformation`, what Task Manager reads), every ten seconds while VRChat or SteamVR runs and every minute
//!   otherwise ([`Keeper::sample`]), joins the loops' summaries and the features that are on into one [`Line`], keeps the
//!   last hour in memory and appends it to `perf.jsonl` in the logs' folder (at most 1 MB, then one older file).
//! - From the last minute it may conclude that N4N0 itself is the cost ([`hint`]), once per cause.
//!
//! What it costs: a clock read and an addition per part per turn, one system call every ten seconds, and a line of about
//! 1 KB. Nothing reads the GPU's counters or opens another process here: the Performance report does that once, when asked.

mod facts;
pub mod gpu;
pub mod report;
#[cfg(test)]
mod tests;

use orbiters_booster::frames::{Frame, Summary};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How often a loop's summary goes to the service, and the service samples while a game runs.
pub const WINDOW: Duration = Duration::from_secs(10);
/// How often the service samples while neither VRChat nor SteamVR runs.
pub const QUIET_WINDOW: Duration = Duration::from_secs(60);
/// Lines kept in memory: an hour at ten seconds.
pub const KEPT: usize = 360;
/// The log's size before it starts again (the older one kept as `perf.jsonl.1`).
pub const LOG_MAX: u64 = 1024 * 1024;
/// A turn this long is felt (a press waits for it).
pub const SLOW_TURN: Duration = Duration::from_millis(50);

/// The parts of a turn of the overlay's (or the desktop panels') loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// SteamVR's events, the controllers and the play space.
    SteamVr,
    /// VRChat's frame timings, the FPS booster.
    Frames,
    /// Fast walk.
    Walk,
    /// The floating panel and the watch.
    Floating,
    /// The panels around VRChat's menus and its camera.
    Orbit,
    /// N4N0's menu.
    Menu,
    /// A 3D capture shown in the headset.
    View3d,
    /// Live translation.
    Translate,
    /// The SteamVR dashboard's Orbiters tab.
    Tab,
    /// Reading VRChat's window (desktop panels).
    Capture,
}

impl Part {
    pub const ALL: [Part; 10] = [Part::SteamVr, Part::Frames, Part::Walk, Part::Floating, Part::Orbit, Part::Menu, Part::View3d, Part::Translate, Part::Tab, Part::Capture];

    pub fn key(self) -> &'static str {
        match self {
            Part::SteamVr => "steamvr",
            Part::Frames => "frames",
            Part::Walk => "walk",
            Part::Floating => "floating",
            Part::Orbit => "orbit",
            Part::Menu => "menu",
            Part::View3d => "view3d",
            Part::Translate => "translate",
            Part::Tab => "tab",
            Part::Capture => "capture",
        }
    }
}

/// Time parts of a process spend on threads of their own (the desktop panels' window looks), nanoseconds, by part.
static ELSEWHERE: [AtomicU64; Part::ALL.len()] = [const { AtomicU64::new(0) }; Part::ALL.len()];

/// `part` worked `took` on another thread of this process: the loop's meter counts it in its next summary.
pub fn elsewhere(part: Part, took: Duration) {
    ELSEWHERE[part as usize].fetch_add(took.as_nanos() as u64, Ordering::Relaxed);
}

static UPLOADS: AtomicU64 = AtomicU64::new(0);
static UPLOAD_PIXELS: AtomicU64 = AtomicU64::new(0);

/// A picture handed to SteamVR (or a window), `pixels` of it new.
pub fn uploaded(pixels: u64) {
    UPLOADS.fetch_add(1, Ordering::Relaxed);
    UPLOAD_PIXELS.fetch_add(pixels, Ordering::Relaxed);
}

fn uploads_now() -> (u64, u64) {
    (UPLOADS.load(Ordering::Relaxed), UPLOAD_PIXELS.load(Ordering::Relaxed))
}

/// A loop's own measure, summed over [`WINDOW`].
pub struct TurnMeter {
    process: &'static str,
    since: Instant,
    turns: u32,
    busy: Duration,
    longest: Duration,
    slow: u32,
    parts: [Duration; Part::ALL.len()],
    uploads: (u64, u64),
    frames: Vec<Frame>,
    refresh_hz: Option<f32>,
    headset: Option<String>,
}

impl TurnMeter {
    /// The most frames kept over a window (ten seconds at 144 Hz, and some).
    const FRAMES_KEPT: usize = 2000;

    pub fn new(process: &'static str, now: Instant) -> Self {
        Self { process, since: now, turns: 0, busy: Duration::ZERO, longest: Duration::ZERO, slow: 0, parts: [Duration::ZERO; Part::ALL.len()], uploads: uploads_now(), frames: Vec::new(), refresh_hz: None, headset: None }
    }

    /// The headset's model, as SteamVR names it.
    pub fn headset(&mut self, model: Option<String>) {
        self.headset = model;
    }

    /// `part` ran from `began` until now.
    pub fn add(&mut self, part: Part, began: Instant) {
        self.parts[part as usize] += began.elapsed();
    }

    /// A turn ended, its work having taken `took` (its wait not counted).
    pub fn turn(&mut self, took: Duration) {
        self.turns += 1;
        self.busy += took;
        self.longest = self.longest.max(took);
        if took >= SLOW_TURN {
            self.slow += 1;
        }
    }

    /// VRChat's frames the compositor showed since the last turn.
    pub fn frames(&mut self, fresh: &[Frame], refresh_hz: Option<f32>) {
        if refresh_hz.is_some() {
            self.refresh_hz = refresh_hz;
        }
        let room = Self::FRAMES_KEPT.saturating_sub(self.frames.len());
        self.frames.extend(fresh.iter().take(room).copied());
    }

    /// The summary of the window that ended, once [`WINDOW`] has passed.
    pub fn take(&mut self, now: Instant) -> Option<ProcessTurns> {
        let seconds = now.duration_since(self.since).as_secs_f32();
        if seconds < WINDOW.as_secs_f32() {
            return None;
        }
        let per_s = |d: Duration| d.as_secs_f32() * 1000.0 / seconds;
        for (i, part) in self.parts.iter_mut().enumerate() {
            *part += Duration::from_nanos(ELSEWHERE[i].swap(0, Ordering::Relaxed));
        }
        let (uploads, pixels) = uploads_now();
        let parts = Part::ALL.iter().map(|p| (p.key().to_string(), round2(per_s(self.parts[*p as usize])))).filter(|(_, ms)| *ms >= 0.01).collect();
        let vrchat = orbiters_booster::frames::summarize(&self.frames, self.refresh_hz.unwrap_or(90.0));
        let out = ProcessTurns {
            process: self.process.to_string(),
            seconds: round2(seconds),
            turns_per_s: round2(self.turns as f32 / seconds),
            busy_ms_per_s: round2(per_s(self.busy)),
            longest_ms: round2(self.longest.as_secs_f32() * 1000.0),
            slow_turns: self.slow,
            parts,
            uploads_per_s: round2(uploads.wrapping_sub(self.uploads.0) as f32 / seconds),
            upload_mpx_per_s: round2(pixels.wrapping_sub(self.uploads.1) as f32 / 1e6 / seconds),
            refresh_hz: self.refresh_hz,
            headset: self.headset.clone(),
            vrchat,
        };
        *self = Self { refresh_hz: self.refresh_hz, headset: self.headset.take(), ..Self::new(self.process, now) };
        Some(out)
    }
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// A loop's last ten seconds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcessTurns {
    /// `overlay` (VR) or `orbit-desktop` (the panels over VRChat's window).
    pub process: String,
    pub seconds: f32,
    pub turns_per_s: f32,
    /// Time the loop worked, milliseconds per second (10 is 1 % of a core).
    pub busy_ms_per_s: f32,
    pub longest_ms: f32,
    /// Turns of [`SLOW_TURN`] or more.
    pub slow_turns: u32,
    /// Each part's time, milliseconds per second.
    pub parts: Vec<(String, f32)>,
    /// Pictures handed to SteamVR, and their new pixels, per second.
    pub uploads_per_s: f32,
    pub upload_mpx_per_s: f32,
    pub refresh_hz: Option<f32>,
    pub headset: Option<String>,
    /// VRChat's frames as SteamVR's compositor saw them (VR only).
    pub vrchat: Option<Summary>,
}

impl ProcessTurns {
    /// The part that took longest, and its milliseconds a second.
    pub fn heaviest(&self) -> Option<(&str, f32)> {
        self.parts.iter().filter(|(k, _)| k != "steamvr" && k != "frames").max_by(|a, b| a.1.total_cmp(&b.1)).map(|(k, ms)| (k.as_str(), *ms))
    }
}

/// One process, as a [`Line`] has it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProcUse {
    pub name: String,
    pub pid: u32,
    /// Percent of one core (100: a whole core; the whole processor is 100 × its logical processors).
    pub cpu: f32,
    pub working_mb: f32,
    pub private_mb: f32,
    pub read_kb_s: f32,
    pub write_kb_s: f32,
    pub threads: u32,
    /// Windows' base priority (8 normal, 6 below normal, 4 idle, 10 above normal, 13 high).
    pub priority: i32,
}

/// What the system call gives for a process.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Raw {
    pub pid: u32,
    pub name: String,
    pub created: i64,
    /// User and kernel time, 100 ns units.
    pub cpu: u64,
    pub working: u64,
    pub private: u64,
    pub read: u64,
    pub write: u64,
    pub threads: u32,
    pub priority: i32,
}

/// Every process now (empty where this is not Windows).
#[cfg(windows)]
pub fn scan() -> Vec<Raw> {
    use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
    // SYSTEM_PROCESS_INFORMATION, 64-bit Windows, as far as the I/O counters.
    #[repr(C)]
    struct Info {
        next: u32,
        threads: u32,
        _private_working: i64,
        _hard_faults: u32,
        _threads_peak: u32,
        _cycles: u64,
        created: i64,
        user: i64,
        kernel: i64,
        name_bytes: u16,
        _name_max: u16,
        name: *const u16,
        priority: i32,
        pid: usize,
        _parent: usize,
        _handles: u32,
        _session: u32,
        _key: usize,
        _peak_virtual: usize,
        _virtual: usize,
        _page_faults: u32,
        _peak_working: usize,
        working: usize,
        _quota: [usize; 4],
        _pagefile: usize,
        _peak_pagefile: usize,
        private: usize,
        _read_ops: i64,
        _write_ops: i64,
        _other_ops: i64,
        read: i64,
        write: i64,
    }
    let mut buffer = vec![0u64; 256 * 1024];
    for _ in 0..4 {
        let mut needed = 0u32;
        // SAFETY: the buffer is as large as said, and aligned for the system's own structures.
        let status = unsafe { NtQuerySystemInformation(SystemProcessInformation, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &mut needed) };
        if status.is_ok() {
            let mut out = Vec::with_capacity(512);
            let base = buffer.as_ptr().cast::<u8>();
            let mut at = 0usize;
            loop {
                // SAFETY: each entry's offset comes from the one before, within what the system filled.
                let info = unsafe { &*base.add(at).cast::<Info>() };
                let name = if info.name.is_null() || info.name_bytes == 0 {
                    String::new()
                } else {
                    // SAFETY: the system's own text, of the length it says, inside the buffer.
                    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(info.name, info.name_bytes as usize / 2) })
                };
                out.push(Raw {
                    pid: info.pid as u32,
                    name,
                    created: info.created,
                    cpu: (info.user + info.kernel).max(0) as u64,
                    working: info.working as u64,
                    private: info.private as u64,
                    read: info.read.max(0) as u64,
                    write: info.write.max(0) as u64,
                    threads: info.threads,
                    priority: info.priority,
                });
                if info.next == 0 {
                    return out;
                }
                at += info.next as usize;
            }
        }
        buffer = vec![0u64; (needed as usize / 8).max(buffer.len()) + 16 * 1024];
    }
    Vec::new()
}

#[cfg(not(windows))]
pub fn scan() -> Vec<Raw> {
    Vec::new()
}

/// The whole processor's busy and total time since Windows started (100 ns units).
#[cfg(windows)]
fn system_times() -> Option<(u64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    let v = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    // SAFETY: three out-parameters on the stack.
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
    // Kernel time includes idle time.
    let total = v(kernel) + v(user);
    Some((total.saturating_sub(v(idle)), total))
}

#[cfg(not(windows))]
fn system_times() -> Option<(u64, u64)> {
    None
}

/// Physical memory: total and available, megabytes.
#[cfg(windows)]
pub fn memory_mb() -> Option<(f32, f32)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    // SAFETY: the structure with its length set, as the call asks.
    unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
    Some((m.ullTotalPhys as f32 / 1048576.0, m.ullAvailPhys as f32 / 1048576.0))
}

#[cfg(not(windows))]
pub fn memory_mb() -> Option<(f32, f32)> {
    None
}

/// The program names a line follows by name: Orbiters' own, VRChat, SteamVR's.
pub fn is_ours(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("orbiters-")
}

fn is_steamvr(name: &str) -> bool {
    matches!(name.to_ascii_lowercase().as_str(), "vrserver.exe" | "vrcompositor.exe" | "vrmonitor.exe" | "vrdashboard.exe" | "vrwebhelper.exe" | "vrstartup.exe")
}

fn is_vrchat(name: &str) -> bool {
    name.eq_ignore_ascii_case("vrchat.exe")
}

/// What processes used between two scans `seconds` apart (a process new since the first counts from its start).
pub fn uses(before: &[Raw], now: &[Raw], seconds: f32) -> Vec<ProcUse> {
    let seconds = seconds.max(0.001);
    now.iter()
        .map(|p| {
            let was = before.iter().find(|b| b.pid == p.pid && b.created == p.created);
            let (cpu0, read0, write0) = was.map_or((0, 0, 0), |b| (b.cpu, b.read, b.write));
            ProcUse {
                name: p.name.clone(),
                pid: p.pid,
                cpu: round2(p.cpu.saturating_sub(cpu0) as f32 / 1e5 / seconds),
                working_mb: round2(p.working as f32 / 1048576.0),
                private_mb: round2(p.private as f32 / 1048576.0),
                read_kb_s: round2(p.read.saturating_sub(read0) as f32 / 1024.0 / seconds),
                write_kb_s: round2(p.write.saturating_sub(write0) as f32 / 1024.0 / seconds),
                threads: p.threads,
                priority: p.priority,
            }
        })
        .collect()
}

/// One sample of the self-check.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Line {
    pub at: String,
    pub seconds: f32,
    /// `vr`, `desktop` (VRChat without SteamVR), `steamvr` (SteamVR without VRChat) or `idle`.
    pub mode: String,
    pub logical_processors: u32,
    /// The whole processor, percent.
    pub system_cpu: f32,
    pub ram_total_mb: f32,
    pub ram_free_mb: f32,
    /// Orbiters' processes.
    pub ours: Vec<ProcUse>,
    pub vrchat: Option<ProcUse>,
    pub steamvr: Vec<ProcUse>,
    /// The five other processes that used the most processor time.
    pub others: Vec<ProcUse>,
    /// The loops' last summaries.
    pub turns: Vec<ProcessTurns>,
    /// What is on ([`features`]).
    pub features: Vec<String>,
    /// What the self-check concluded ([`hint`]), by its key.
    pub hint: Option<String>,
}

impl Line {
    /// Orbiters' processes together, percent of one core.
    pub fn ours_cpu(&self) -> f32 {
        self.ours.iter().map(|p| p.cpu).sum()
    }

    /// The overlay's loop, when it reported.
    pub fn overlay(&self) -> Option<&ProcessTurns> {
        self.turns.iter().find(|t| t.process == "overlay")
    }
}

/// The features that are on, as short keys (what a [`Line`] notes, and the report lists).
pub fn features(snap: &crate::state::Snapshot) -> Vec<String> {
    let s = &snap.settings;
    let mut on: Vec<String> = Vec::new();
    let mut add = |yes: bool, key: &str| {
        if yes {
            on.push(key.to_string());
        }
    };
    add(s.steamvr_overlay, "steamvr_overlay");
    add(s.orbit.wheel, "panels_wheel");
    add(s.orbit.quick_menu, "panels_quick_menu");
    add(s.orbit.camera_tiles, "camera_tools");
    add(s.orbit.camera_pattern, "camera_pattern");
    add(s.orbit.immersive_turn, "immersive_turn");
    add(s.floating.watch, "watch");
    add(s.n4n0.enabled, "n4n0");
    add(s.n4n0.enabled && s.n4n0.assistant, "assistant");
    add(s.n4n0.enabled && s.n4n0.wake_word, "wake_word");
    add(s.n4n0.enabled && s.n4n0.controller_input, "controller_input");
    add(s.n4n0.calls, "calls");
    add(s.osc.enabled, "osc");
    add(s.osc.record_parameters, "record_parameters");
    add(s.translate.enabled, "translation");
    add(s.walk_boost.enabled, "fast_walk");
    add(s.tracking.enabled, "smoothing");
    add(s.world_performance, "world_performance");
    add(s.usage.enabled, "usage_statistics");
    add(s.dolly_streams, "dolly_streams");
    add(s.experimental.vrchat_bridge, "vrchat_bridge");
    add(s.booster.level != orbiters_booster::Level::Off, &format!("booster_{:?}", s.booster.level).to_lowercase());
    add(snap.is_capturing(), "recording");
    add(snap.reconstruction.is_some(), "reconstructing");
    on
}

/// What the self-check says to the person: a cause, its key, and which part it names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hint {
    /// `overlay_cpu`, `panels_gpu`, `reconstruct_gpu`, `translate_memory`, `desktop_cpu`.
    pub key: String,
    /// The overlay's part that took the most (`orbit`, `translate`, `view3d`…), for `overlay_cpu`.
    pub part: Option<String>,
}

impl Hint {
    /// Said in the app's language: what is costly, and what turns it off.
    pub fn words(&self) -> (String, String) {
        use orbiters_i18n::tr;
        let title = match self.key.as_str() {
            "panels_gpu" => tr("N4N0's VR panels redraw often while VRChat is short of graphics time"),
            "reconstruct_gpu" => tr("A 3D picture is being made on your graphics card while VRChat is short of graphics time"),
            "translate_memory" => tr("Live translation holds a lot of memory while your computer has little left"),
            "desktop_cpu" => tr("N4N0's panels over VRChat's window are using more of your processor than they should"),
            _ => tr("N4N0 is using more of your processor than it should"),
        };
        let advice = match (self.key.as_str(), self.part.as_deref()) {
            ("reconstruct_gpu", _) => tr("Settings › Capture › Reconstruct while VRChat runs: choose After VRChat."),
            ("translate_memory", _) | (_, Some("translate")) => tr("Turn off Translation, or have an AI app on another computer translate."),
            ("panels_gpu", _) | (_, Some("orbit")) => tr("Settings › Performance: turn off Around the Quick Menu, or Around the Action Menu's wheel."),
            (_, Some("view3d")) => tr("Close the 3D capture shown in your headset."),
            (_, Some("floating")) => tr("Turn off the watch, or close the floating panel."),
            (_, Some("walk")) => tr("Turn off Fast walk."),
            (_, Some("menu")) => tr("Close N4N0's menu when you do not use it."),
            ("desktop_cpu", _) | (_, Some("capture")) => tr("Settings › Performance: turn off Around the Quick Menu, or Around the Action Menu's wheel."),
            _ => tr("Make a Performance report in Settings and send it: it says what takes the time."),
        };
        (title.to_string(), advice.to_string())
    }
}

/// What the self-check concludes from the last minute (`lines`, oldest first): `None` unless every line of the minute
/// says the same, so that a moment's work (a capture opening, a page painted) never says anything.
pub fn hint(lines: &[Line]) -> Option<Hint> {
    const MINUTE: usize = 6;
    let last: Vec<&Line> = lines.iter().rev().take(MINUTE).collect();
    if last.len() < MINUTE || last.iter().any(|l| l.mode != last[0].mode) {
        return None;
    }
    let mode = last[0].mode.as_str();
    let cores = last[0].logical_processors.max(1) as f32;
    let all = |f: &dyn Fn(&Line) -> bool| last.iter().all(|l| f(l));
    let vrchat_gpu_short = |l: &Line| l.overlay().and_then(|o| o.vrchat).is_some_and(|v| v.late_gpu_share >= 0.15 || v.dropped > 0 && v.late_gpu_share >= 0.05);
    // Too much of the processor: a fifth of a core, or two percent of the whole processor, whichever is more. On a
    // 4-thread processor 20 % of a core is 5 % of all of it; on 20 threads it takes 40 %.
    let too_much = (20.0f32).max(2.0 * cores);
    if mode == "vr" {
        if all(&|l: &Line| l.ours.iter().any(|p| p.name.eq_ignore_ascii_case("orbiters-reconstruct.exe") && p.cpu > 5.0) && vrchat_gpu_short(l)) {
            return Some(Hint { key: "reconstruct_gpu".into(), part: None });
        }
        if all(&|l: &Line| l.features.iter().any(|f| f == "translation") && l.ram_free_mb > 0.0 && l.ram_free_mb < 1500.0 && l.ours.iter().any(|p| p.private_mb > 1500.0)) {
            return Some(Hint { key: "translate_memory".into(), part: Some("translate".into()) });
        }
        if all(&|l: &Line| l.overlay().is_some_and(|o| o.upload_mpx_per_s >= 20.0) && vrchat_gpu_short(l)) {
            return Some(Hint { key: "panels_gpu".into(), part: Some("orbit".into()) });
        }
        if all(&|l: &Line| l.overlay().is_some_and(|o| o.busy_ms_per_s * 0.1 >= too_much)) {
            let part = last[0].overlay().and_then(|o| o.heaviest()).map(|(k, _)| k.to_string());
            return Some(Hint { key: "overlay_cpu".into(), part });
        }
    }
    if mode == "desktop" && all(&|l: &Line| l.turns.iter().find(|t| t.process == "orbit-desktop").is_some_and(|o| o.busy_ms_per_s * 0.1 >= too_much)) {
        return Some(Hint { key: "desktop_cpu".into(), part: Some("capture".into()) });
    }
    None
}

/// A debug build measuring what the fixes of N-friend-lag gained: with `perf-old-publishing.flag` in the logs' folder when it
/// starts, VRChat's wrists and speed publish the state as 0.3.5 did (every wrist at most 20 times a second a hand, every speed).
/// Never in a build people run.
pub fn old_publishing() -> bool {
    static OLD: OnceLock<bool> = OnceLock::new();
    cfg!(debug_assertions) && *OLD.get_or_init(|| orbiters_core::paths::logs_dir().join("perf-old-publishing.flag").exists())
}

/// The causes the person said not to be told again (`perf-hints.json` in the configuration folder).
fn hints_file() -> PathBuf {
    orbiters_core::paths::config_dir().join("perf-hints.json")
}

pub fn hints_muted() -> Vec<String> {
    std::fs::read(hints_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn mute_hint(key: &str) {
    let mut muted = hints_muted();
    if !muted.iter().any(|k| k == key) {
        muted.push(key.to_string());
        let _ = orbiters_core::write_atomic(&hints_file(), &serde_json::to_vec(&muted).unwrap_or_default());
    }
}

/// What the window asks of the self-check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    /// Make a Performance report (and save it).
    Make,
    /// Send the last one made to Orbiters.
    Send,
    /// Never say this hint (its key) again.
    Mute(String),
    /// Put the report away (its file stays).
    Close,
}

/// What the window shows of the self-check.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerfView {
    /// What the self-check says now, unless muted.
    pub hint: Option<Hint>,
    /// The last report made: where it was saved, its summary lines, and whether it was sent.
    pub report: Option<ReportView>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReportView {
    /// Still being put together (the GPU is watched for a few seconds).
    pub making: bool,
    pub path: Option<PathBuf>,
    /// What is in it, line by line, in the app's language.
    pub summary: Vec<String>,
    pub sending: bool,
    /// `Some(true)`: Orbiters has it; `Some(false)`: it could not be sent (`error` says why).
    pub sent: Option<bool>,
    pub error: Option<String>,
}

/// The service's side: the loops' last summaries, the last scan, the hour of lines.
pub struct Keeper {
    lines: VecDeque<Line>,
    turns: Vec<(Instant, ProcessTurns)>,
    before: Option<(Instant, Vec<Raw>, Option<(u64, u64)>)>,
    log: PathBuf,
    said: Vec<String>,
    /// Which of our processes is which (the service tells it the children it starts), for [`MemoryLine`].
    roles: Vec<(u32, &'static str)>,
    /// When the last [`MemoryLine`] was written.
    memory_at: Option<Instant>,
}

pub fn keeper() -> &'static Mutex<Keeper> {
    static KEEPER: OnceLock<Mutex<Keeper>> = OnceLock::new();
    KEEPER.get_or_init(|| Mutex::new(Keeper::new(orbiters_core::paths::logs_dir().join("perf.jsonl"))))
}

impl Keeper {
    pub fn new(log: PathBuf) -> Self {
        Self { lines: VecDeque::with_capacity(KEPT), turns: Vec::new(), before: None, log, said: Vec::new(), roles: Vec::new(), memory_at: None }
    }

    /// The service started `pid` as `role` (`window`, `overlay`, `orbit-desktop`).
    pub fn role(&mut self, pid: u32, role: &'static str) {
        self.roles.retain(|(p, r)| *p != pid && *r != role);
        self.roles.push((pid, role));
    }

    /// `perf-memory.jsonl`, beside the log.
    fn memory_log(&self) -> PathBuf {
        self.log.with_file_name(MEMORY_LOG)
    }

    /// A loop's summary came.
    pub fn turns(&mut self, turns: ProcessTurns) {
        self.turns.retain(|(_, t)| t.process != turns.process);
        self.turns.push((Instant::now(), turns));
    }

    /// The lines kept, oldest first.
    pub fn lines(&self) -> Vec<Line> {
        self.lines.iter().cloned().collect()
    }

    /// Samples now: a line from the processes as they are against the last scan, kept and logged. The first call only
    /// takes the scan to compare with.
    pub fn sample(&mut self, now: Instant, mode: &str, features: Vec<String>) -> Option<Line> {
        let scan = scan();
        let times = system_times();
        let previous = self.before.replace((now, scan, times));
        let (then, before, times_before) = previous?;
        let (_, scan, times) = self.before.as_ref()?;
        let seconds = now.duration_since(then).as_secs_f32();
        let used = uses(&before, scan, seconds);
        let system_cpu = match (times_before, times) {
            (Some((b0, t0)), Some((b1, t1))) if t1 > &t0 => round2(100.0 * (b1 - b0) as f32 / (t1 - t0) as f32),
            _ => 0.0,
        };
        let (ram_total_mb, ram_free_mb) = memory_mb().unwrap_or((0.0, 0.0));
        let mut others: Vec<ProcUse> = used.iter().filter(|p| !is_ours(&p.name) && !is_vrchat(&p.name) && !is_steamvr(&p.name) && p.pid > 4).cloned().collect();
        others.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
        others.truncate(5);
        // A loop's summary older than two windows is from a loop that ended.
        self.turns.retain(|(at, _)| now.duration_since(*at) <= 2 * WINDOW + Duration::from_secs(1));
        let mut line = Line {
            at: orbiters_core::now_utc_iso(),
            seconds: round2(seconds),
            mode: mode.to_string(),
            logical_processors: std::thread::available_parallelism().map_or(1, |n| n.get() as u32),
            system_cpu,
            ram_total_mb: round2(ram_total_mb),
            ram_free_mb: round2(ram_free_mb),
            ours: used.iter().filter(|p| is_ours(&p.name)).cloned().collect(),
            vrchat: used.iter().filter(|p| is_vrchat(&p.name)).max_by(|a, b| a.cpu.total_cmp(&b.cpu)).cloned(),
            steamvr: used.iter().filter(|p| is_steamvr(&p.name) && p.cpu >= 0.5).cloned().collect(),
            others,
            turns: self.turns.iter().map(|(_, t)| t.clone()).collect(),
            features,
            hint: None,
        };
        if self.lines.len() >= KEPT {
            self.lines.pop_front();
        }
        self.lines.push_back(line.clone());
        let lines: Vec<Line> = self.lines.iter().rev().take(6).rev().cloned().collect();
        line.hint = hint(&lines).map(|h| h.key);
        if let Some(last) = self.lines.back_mut() {
            last.hint = line.hint.clone();
        }
        append(&self.log, &line);
        if line.mode != "idle" && self.memory_at.is_none_or(|at| now.duration_since(at) >= MEMORY_EVERY) {
            self.memory_at = Some(now);
            let memory = MemoryLine::of(&line, std::process::id(), &self.roles);
            append_to(&self.memory_log(), &memory, MEMORY_LOG_MAX);
        }
        Some(line)
    }

    /// A cause to tell the person now: one not told before in this run and not muted.
    pub fn to_tell(&mut self, hint: &Hint, muted: &[String]) -> bool {
        if muted.iter().any(|k| *k == hint.key) || self.said.iter().any(|k| *k == hint.key) {
            return false;
        }
        self.said.push(hint.key.clone());
        true
    }
}

/// The memory of our processes while VRChat or SteamVR runs, a line a minute, in its own file: `perf.jsonl` keeps 10 to 20 hours,
/// mostly idle lines, and no VR evening survived in it when Windows had flagged the VR overlay as leaking 8 times (REL-4). About
/// 300 bytes a minute: [`MEMORY_LOG_MAX`] and its older file hold some 400 hours in VR. Nothing reads it but a person or a
/// script looking for a leak: whether the overlay grows over a VR night.
pub const MEMORY_LOG: &str = "perf-memory.jsonl";
pub const MEMORY_LOG_MAX: u64 = 4 * 1024 * 1024;
pub const MEMORY_EVERY: Duration = Duration::from_secs(60);

/// One [`MEMORY_LOG`] line.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryLine {
    pub at: String,
    pub mode: String,
    pub ram_free_mb: f32,
    pub ours: Vec<MemoryUse>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryUse {
    /// `service`, `window`, `overlay`, `orbit-desktop`, or the program's name for a worker.
    pub role: String,
    pub pid: u32,
    pub private_mb: f32,
    pub working_mb: f32,
    pub threads: u32,
}

impl MemoryLine {
    /// From a [`Line`]: each of our processes by its role, `service` being `service_pid`.
    pub fn of(line: &Line, service_pid: u32, roles: &[(u32, &'static str)]) -> Self {
        let role = |p: &ProcUse| {
            if p.pid == service_pid {
                return "service".to_string();
            }
            roles.iter().find(|(pid, _)| *pid == p.pid).map_or_else(|| p.name.clone(), |(_, r)| (*r).to_string())
        };
        Self {
            at: line.at.clone(),
            mode: line.mode.clone(),
            ram_free_mb: line.ram_free_mb,
            ours: line.ours.iter().map(|p| MemoryUse { role: role(p), pid: p.pid, private_mb: p.private_mb, working_mb: p.working_mb, threads: p.threads }).collect(),
        }
    }
}

/// Appends `line` to the log, starting it again past [`LOG_MAX`] (the older one kept once).
pub fn append(path: &Path, line: &Line) {
    append_to(path, line, LOG_MAX);
}

/// Appends `line` as JSON to `path`, starting it again past `max` bytes (the older one kept once, as `<path>.1`).
pub fn append_to<T: Serialize>(path: &Path, line: &T, max: u64) {
    use std::io::Write;
    let Ok(mut text) = serde_json::to_string(line) else { return };
    text.push('\n');
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() + text.len() as u64 > max) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::rename(path, PathBuf::from(old));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        let _ = file.write_all(text.as_bytes());
    }
}
