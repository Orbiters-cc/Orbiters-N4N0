//! The beta's performance figures (Enzo, 2026-11-16: "add the FPS and CPU / GPU / RAM metrics over time so we can also track how
//! much resources each feature uses and how they impact the in-game FPS"). Sent with the usage statistics, under the same switch.
//!
//! Every sample of the performance self-check (`crate::perf::Keeper::sample`: every 10 s while VRChat or SteamVR runs, every minute
//! otherwise) becomes a [`Sample`]: the whole computer's processor, memory and graphics card; N4N0's own processes together; VRChat's
//! process (from the same system call and Windows' GPU counters, never by opening it); VRChat's frames as SteamVR's compositor saw them
//! (VR only: on the desktop nothing honest gives VRChat's frame rate without hooking the game, so none is sent); and what N4N0 was
//! doing — the screens its sessions were on (the usage statistics' own names and session ids, so the server joins both), the
//! features at work, VR or desktop. Samples with the same context are summed on the computer into a [`Window`] of at most a minute
//! (mean, low and high of each figure): about 1 KB a minute is what leaves, at most [`super::spool::MAX_PERF_KEPT`] wait on disk.
//!
//! Also in each window, the basic hardware class (processor model, graphics card model, RAM size, headset and its refresh rate) and
//! what the sampling itself cost.

use crate::perf::gpu::GpuNow;
use crate::perf::Line;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

/// A window closes after this long even when nothing changed.
pub const WINDOW_MS: u64 = 60_000;

/// What N4N0 was doing during a sample: a window only sums samples of one context.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    /// `vr`, `desktop` (VRChat without SteamVR), `steamvr` (SteamVR without VRChat) or `idle`.
    pub mode: String,
    /// Each open surface's screen, by surface (`app`, `vr`, `vrchat-desktop`), as the usage statistics name them.
    pub screens: BTreeMap<String, String>,
    /// The usage statistics' sessions open (to join the events' timeline).
    pub sessions: Vec<String>,
    /// What is at work now (see [`active`]), sorted.
    pub active: Vec<String>,
}

/// One sample's figures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub at: u64,
    pub seconds: f32,
    pub context: Context,
    /// The whole processor, percent.
    pub cpu_system: f32,
    /// N4N0's processes together and VRChat's, percent of the whole processor.
    pub cpu_ours: f32,
    pub cpu_vrchat: Option<f32>,
    pub ram_total_mb: f32,
    pub ram_used_mb: f32,
    /// Private memory.
    pub ram_ours_mb: f32,
    pub ram_vrchat_mb: Option<f32>,
    pub gpu: Option<GpuFigures>,
    pub frames: Option<Frames>,
    pub headset: Option<String>,
    pub refresh_hz: Option<f32>,
    pub cores: u32,
    /// What making this sample cost N4N0, microseconds.
    pub cost_us: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuFigures {
    pub system: f32,
    pub ours: f32,
    pub vrchat: Option<f32>,
    pub vram_used_mb: f32,
    pub vram_ours_mb: f32,
    pub vram_vrchat_mb: Option<f32>,
}

/// VRChat's frames over a sample, as SteamVR's compositor saw them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frames {
    /// Distinct frames a second (what SteamVR filled in is not counted).
    pub fps: f32,
    /// The interval between frames, median and 95th percentile, ms.
    pub frame_ms: f32,
    pub frame_p95_ms: f32,
    /// VRChat's own GPU time a frame, median, ms.
    pub app_gpu_ms: f32,
    /// Share of presents SteamVR reprojected (filled in).
    pub reprojected: f32,
    pub dropped: u32,
    /// Frames over twice the headset's budget: felt as stalls.
    pub stalls: u32,
}

/// Spread of a figure over a window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Spread {
    pub mean: f32,
    pub p5: f32,
    pub p95: f32,
}

impl Spread {
    pub fn of(values: &[f32]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let mut v = values.to_vec();
        v.sort_by(f32::total_cmp);
        let at = |q: f32| v[((v.len() - 1) as f32 * q).round() as usize];
        Some(Self { mean: r1(v.iter().sum::<f32>() / v.len() as f32), p5: r1(at(0.05)), p95: r1(at(0.95)) })
    }
}

fn r1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

/// The basic hardware class.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Hardware {
    pub cpu: String,
    pub gpu: String,
    pub ram_gb: u32,
    pub cores: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hz: Option<f32>,
}

/// A window as Orbiters receives it (`POST /n4n0/metrics`, `perf`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// Its first sample's start and its length, ms since 1970 and seconds.
    pub at: u64,
    pub seconds: f32,
    pub samples: u32,
    pub build: String,
    pub os: String,
    pub mode: String,
    pub screens: BTreeMap<String, String>,
    pub sessions: Vec<String>,
    pub active: Vec<String>,
    pub hw: Hardware,
    /// Percent of the whole processor.
    pub cpu_system: Spread,
    pub cpu_ours: Spread,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_vrchat: Option<Spread>,
    pub ram_total_mb: f32,
    pub ram_used_mb: Spread,
    pub ram_ours_mb: Spread,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ram_vrchat_mb: Option<Spread>,
    /// Percent of the graphics card's time (none where Windows has no GPU counters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_system: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_ours: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_vrchat: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_used_mb: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_ours_mb: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_vrchat_mb: Option<Spread>,
    /// VRChat's frames (VR only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_ms: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_gpu_ms: Option<Spread>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reprojected: Option<f32>,
    #[serde(default)]
    pub dropped: u32,
    #[serde(default)]
    pub stalls: u32,
    /// What the sampling cost N4N0 over the window: mean microseconds a sample.
    pub cost_us: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub debug: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sandbox: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub agent: bool,
}

/// What is at work now, as short keys: the person's activities and the overlay loop's parts that did work in the last summary.
pub fn active(snap: &crate::state::Snapshot, line: &Line, open: &BTreeMap<String, String>) -> Vec<String> {
    use crate::controller::camera_tools::Qr;
    use crate::state::TranslateState;
    let mut on: Vec<String> = Vec::new();
    let mut add = |yes: bool, key: &str| {
        if yes {
            on.push(key.to_string());
        }
    };
    add(snap.is_capturing(), "recording");
    add(snap.reconstruction.is_some(), "reconstruction");
    add(snap.translate.state != TranslateState::Off, "translation");
    add(!matches!(snap.camera_tools.qr, Qr::Off), "qr_scanning");
    add(snap.camera_open_said == Some(true), "vrchat_camera");
    add(snap.n4n0.calls.call.is_some(), "call");
    add(snap.settings.booster.level != orbiters_booster::Level::Off, "fps_booster");
    add(snap.settings.walk_boost.enabled, "fast_walk");
    add(snap.settings.tracking.enabled, "smoothing");
    add(snap.settings.floating.watch, "watch");
    add(open.contains_key("app"), "window_open");
    add(open.get("vr").is_some_and(|s| s == "closed") || open.get("vrchat-desktop").is_some_and(|s| s == "closed"), "menus_hidden");
    // The loops' parts that worked at least half a millisecond a second: what the overlay or the desktop panels drew or read.
    for turns in &line.turns {
        for (part, ms) in &turns.parts {
            let key = match part.as_str() {
                "view3d" => "drawing_3d_view",
                "translate" => "drawing_translation",
                "floating" => "drawing_floating_panel",
                "orbit" => "drawing_menu_panels",
                "menu" => "drawing_n4n0_menu",
                "capture" => "reading_vrchat_window",
                "walk" => "fast_walk_moving",
                _ => continue,
            };
            add(*ms >= 0.5, key);
        }
    }
    on.sort();
    on.dedup();
    on
}

/// A sample from a self-check line, the GPU's counters over the same interval, and the context.
pub fn sample(line: &Line, gpu: Option<&GpuNow>, context: Context, at: u64, cost: Duration) -> Sample {
    let cores = line.logical_processors.max(1);
    let whole = |one_core: f32| r1(one_core / cores as f32);
    let ours_pids: Vec<u32> = line.ours.iter().map(|p| p.pid).collect();
    let vrchat_pid = line.vrchat.as_ref().map(|p| p.pid);
    let overlay = line.overlay();
    let frames = overlay.and_then(|o| o.vrchat.as_ref()).map(|v| Frames {
        fps: v.real_fps,
        frame_ms: v.interval.median,
        frame_p95_ms: v.interval.p95,
        app_gpu_ms: v.app_gpu.median,
        reprojected: v.filled_share,
        dropped: v.dropped,
        stalls: v.over_2x,
    });
    Sample {
        at,
        seconds: line.seconds,
        context,
        cpu_system: line.system_cpu,
        cpu_ours: whole(line.ours_cpu()),
        cpu_vrchat: line.vrchat.as_ref().map(|v| whole(v.cpu)),
        ram_total_mb: line.ram_total_mb,
        ram_used_mb: (line.ram_total_mb - line.ram_free_mb).max(0.0),
        ram_ours_mb: line.ours.iter().map(|p| p.private_mb).sum(),
        ram_vrchat_mb: line.vrchat.as_ref().map(|v| v.private_mb),
        gpu: gpu.map(|g| {
            let (ours, vram_ours_mb) = g.of(&ours_pids);
            let vrchat = vrchat_pid.map(|p| g.of(&[p]));
            GpuFigures { system: g.system, ours, vrchat: vrchat.map(|v| v.0), vram_used_mb: g.vram_used_mb, vram_ours_mb, vram_vrchat_mb: vrchat.map(|v| v.1) }
        }),
        frames,
        headset: line.turns.iter().find_map(|t| t.headset.clone()),
        refresh_hz: line.turns.iter().find_map(|t| t.refresh_hz),
        cores,
        cost_us: cost.as_micros().min(u32::MAX as u128) as u32,
    }
}

/// Where this run comes from, for the admin to leave out its own testing: a debug build, a build the VR sandbox started.
pub fn debug_build() -> bool {
    cfg!(debug_assertions)
}

pub fn sandbox_run() -> bool {
    static SANDBOX: OnceLock<bool> = OnceLock::new();
    *SANDBOX.get_or_init(|| ["ORBITERS_VR_SANDBOX_ROOT", "ORBITERS_SANDBOX_LOGS"].iter().any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty())))
}

/// The hardware class, read once a run (the processor's and graphics card's names do not change while it runs).
fn hardware(cores: u32) -> Hardware {
    static HW: OnceLock<(String, String, u32)> = OnceLock::new();
    let (cpu, gpu, ram_gb) = HW.get_or_init(crate::perf::report::hardware_class).clone();
    Hardware { cpu, gpu, ram_gb, cores, headset: None, hz: None }
}

/// Sums samples into windows: a window closes when the context changes or [`WINDOW_MS`] passed.
#[derive(Default)]
pub struct Windows {
    open: Vec<Sample>,
    /// The bridge drove VRChat or the app at some point of the open window.
    agent: bool,
}

impl Windows {
    /// A sample came; the window it closed, if any.
    pub fn push(&mut self, sample: Sample, agent: bool) -> Option<Window> {
        let closes = self.open.first().is_some_and(|first| first.context != sample.context || sample.at.saturating_sub(first.at) >= WINDOW_MS);
        let done = if closes { self.close() } else { None };
        self.agent |= agent;
        self.open.push(sample);
        done
    }

    /// The open window, closed now (at a stop, or when the statistics are turned off).
    pub fn close(&mut self) -> Option<Window> {
        let samples = std::mem::take(&mut self.open);
        let agent = std::mem::take(&mut self.agent);
        let first = samples.first()?;
        let spread = |f: &dyn Fn(&Sample) -> Option<f32>| Spread::of(&samples.iter().filter_map(f).collect::<Vec<_>>());
        let all = |f: &dyn Fn(&Sample) -> f32| Spread::of(&samples.iter().map(f).collect::<Vec<_>>()).unwrap_or_default();
        let frames: Vec<Frames> = samples.iter().filter_map(|s| s.frames).collect();
        let mut hw = hardware(first.cores);
        hw.headset = samples.iter().rev().find_map(|s| s.headset.clone());
        hw.hz = samples.iter().rev().find_map(|s| s.refresh_hz);
        Some(Window {
            at: first.at.saturating_sub((first.seconds * 1000.0) as u64),
            seconds: r1(samples.iter().map(|s| s.seconds).sum()),
            samples: samples.len() as u32,
            build: env!("ORBITERS_VERSION").into(),
            os: std::env::consts::OS.into(),
            mode: first.context.mode.clone(),
            screens: first.context.screens.clone(),
            sessions: first.context.sessions.clone(),
            active: first.context.active.clone(),
            hw,
            cpu_system: all(&|s| s.cpu_system),
            cpu_ours: all(&|s| s.cpu_ours),
            cpu_vrchat: spread(&|s| s.cpu_vrchat),
            ram_total_mb: r1(first.ram_total_mb),
            ram_used_mb: all(&|s| s.ram_used_mb),
            ram_ours_mb: all(&|s| s.ram_ours_mb),
            ram_vrchat_mb: spread(&|s| s.ram_vrchat_mb),
            gpu_system: spread(&|s| s.gpu.map(|g| g.system)),
            gpu_ours: spread(&|s| s.gpu.map(|g| g.ours)),
            gpu_vrchat: spread(&|s| s.gpu.and_then(|g| g.vrchat)),
            vram_used_mb: spread(&|s| s.gpu.map(|g| g.vram_used_mb)),
            vram_ours_mb: spread(&|s| s.gpu.map(|g| g.vram_ours_mb)),
            vram_vrchat_mb: spread(&|s| s.gpu.and_then(|g| g.vram_vrchat_mb)),
            fps: Spread::of(&frames.iter().map(|f| f.fps).collect::<Vec<_>>()),
            frame_ms: Spread::of(&frames.iter().map(|f| f.frame_ms).collect::<Vec<_>>()).map(|mut s| {
                // The window's high end is its samples' own 95th percentiles, not the spread of their medians.
                s.p95 = r1(frames.iter().map(|f| f.frame_p95_ms).fold(0.0, f32::max));
                s
            }),
            app_gpu_ms: Spread::of(&frames.iter().map(|f| f.app_gpu_ms).collect::<Vec<_>>()),
            reprojected: (!frames.is_empty()).then(|| (frames.iter().map(|f| f.reprojected).sum::<f32>() / frames.len() as f32 * 1000.0).round() / 1000.0),
            dropped: frames.iter().map(|f| f.dropped).sum(),
            stalls: frames.iter().map(|f| f.stalls).sum(),
            cost_us: (samples.iter().map(|s| s.cost_us as u64).sum::<u64>() / samples.len() as u64) as u32,
            debug: debug_build(),
            sandbox: sandbox_run(),
            agent,
        })
    }
}

/// The signed-in Orbiters account, as a code the admin can match ("leave out my own use"), only for accounts Orbiters gave its
/// experimental tools (the team's: `bridge.allowed`); never for anyone else. 16 hex characters of SHA-256 of a fixed prefix and the
/// lower-cased name: the server computes the same from a name the admin types.
pub fn team_code(snap: &crate::state::Snapshot) -> Option<String> {
    use sha2::{Digest, Sha256};
    if !snap.bridge.allowed {
        return None;
    }
    let crate::state::AccountState::SignedIn { username } = &snap.account.state else { return None };
    let digest = Sha256::digest(format!("orbiters-n4n0-usage\u{0}{}", username.trim().to_lowercase()).as_bytes());
    Some(digest.iter().take(8).map(|b| format!("{b:02x}")).collect())
}

/// The VRChat bridge (an AI agent) drives VRChat or the app now.
pub fn agent_driving(snap: &crate::state::Snapshot) -> bool {
    let b = &snap.bridge;
    b.acting || !b.driven.is_empty() || b.client.as_ref().is_some_and(|c| c.connected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perf::{Line, ProcUse, ProcessTurns};

    fn line(system: f32, ours_cpu: f32, fps: Option<f32>) -> Line {
        let vrchat = fps.map(|fps| orbiters_booster::frames::Summary { real_fps: fps, filled_share: 0.1, dropped: 1, over_2x: 2, ..Default::default() });
        Line {
            seconds: 10.0,
            mode: "vr".into(),
            logical_processors: 20,
            system_cpu: system,
            ram_total_mb: 32000.0,
            ram_free_mb: 12000.0,
            ours: vec![ProcUse { name: "orbiters-desktop.exe".into(), pid: 10, cpu: ours_cpu, private_mb: 100.0, ..Default::default() }],
            vrchat: Some(ProcUse { name: "VRChat.exe".into(), pid: 20, cpu: 200.0, private_mb: 7000.0, ..Default::default() }),
            turns: vec![ProcessTurns { process: "overlay".into(), parts: vec![("orbit".into(), 3.0), ("view3d".into(), 0.1)], headset: Some("Quest Pro".into()), refresh_hz: Some(72.0), vrchat, ..Default::default() }],
            ..Default::default()
        }
    }

    fn context(screen: &str) -> Context {
        Context { mode: "vr".into(), screens: [("vr".to_string(), screen.to_string())].into(), sessions: vec!["a".repeat(32)], active: vec!["drawing_menu_panels".into()] }
    }

    /// Samples of one context make one window of at most a minute, with each figure's mean and spread; a new screen starts another.
    #[test]
    fn samples_sum_into_windows_by_what_n4n0_was_doing() {
        let gpu = GpuNow { system: 80.0, vram_used_mb: 9000.0, by_pid: [(10, (2.0, 60.0)), (20, (70.0, 3000.0))].into_iter().collect() };
        let mut w = Windows::default();
        let mut at = 1_000_000u64;
        let mut closed = Vec::new();
        for i in 0..8 {
            at += 10_000;
            let s = sample(&line(30.0 + i as f32, 20.0, Some(60.0 + i as f32)), Some(&gpu), context("qm"), at, Duration::from_micros(400));
            closed.extend(w.push(s, false));
        }
        // 8 samples 10 s apart: the first window closes when the seventh comes (60 s after the first).
        assert_eq!(closed.len(), 1);
        let first = &closed[0];
        assert_eq!(first.samples, 6);
        assert_eq!(first.cpu_system.mean, 32.5);
        assert_eq!(first.cpu_ours.mean, 1.0, "20 % of one core on 20 threads is 1 % of the processor");
        assert_eq!(first.cpu_vrchat.unwrap().mean, 10.0);
        assert_eq!(first.ram_used_mb.mean, 20000.0);
        assert_eq!(first.gpu_vrchat.unwrap().mean, 70.0);
        assert_eq!(first.vram_ours_mb.unwrap().mean, 60.0);
        assert_eq!(first.fps.unwrap().p5, 60.0);
        assert_eq!(first.fps.unwrap().p95, 65.0);
        assert_eq!((first.dropped, first.stalls), (6, 12));
        assert_eq!(first.hw.headset.as_deref(), Some("Quest Pro"));
        assert_eq!(first.hw.hz, Some(72.0));
        assert_eq!(first.cost_us, 400);
        // Another screen closes the open window at once.
        at += 10_000;
        let other = w.push(sample(&line(30.0, 20.0, None), None, context("qm/Worlds"), at, Duration::ZERO), true).expect("closed by the change");
        assert_eq!(other.samples, 2);
        let last = w.close().expect("open window");
        assert!(last.agent && last.fps.is_none() && last.gpu_system.is_none(), "no frames, no GPU counters: nothing made up");
        assert_eq!(last.screens["vr"], "qm/Worlds");
        // What leaves the computer: no process names, no window titles.
        let json = serde_json::to_string(&last).unwrap();
        assert!(!json.contains("VRChat.exe") && !json.contains("orbiters-desktop"), "{json}");
        assert!(json.len() < 1500, "{} bytes", json.len());
    }

    #[test]
    fn the_overlays_parts_say_what_was_drawn() {
        let snap = crate::state::Snapshot::new(orbiters_core::settings::Settings::default());
        let open: BTreeMap<String, String> = [("vr".to_string(), "closed".to_string())].into();
        let keys = active(&snap, &line(10.0, 1.0, None), &open);
        assert!(keys.contains(&"drawing_menu_panels".to_string()));
        assert!(!keys.contains(&"drawing_3d_view".to_string()), "0.1 ms/s is not at work");
        assert!(keys.contains(&"menus_hidden".to_string()));
    }

    #[test]
    fn only_a_team_account_gives_a_code() {
        let mut snap = crate::state::Snapshot::new(orbiters_core::settings::Settings::default());
        snap.account.state = crate::state::AccountState::SignedIn { username: "BlackOrbit".into() };
        assert_eq!(team_code(&snap), None);
        snap.bridge.allowed = true;
        let code = team_code(&snap).unwrap();
        assert_eq!(code.len(), 16);
        // The server's side computes the same (backend services/n4n0/usageExclusions.js accountCode).
        assert_eq!(code, team_code_of("blackorbit"));
    }

    fn team_code_of(name: &str) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(format!("orbiters-n4n0-usage\u{0}{name}").as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
    }
}
