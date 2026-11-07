//! The FPS booster in the overlay: where SteamVR's frames are, and the only process that talks to SteamVR. It runs the
//! frame logger at every level, Off included, and decides. The one thing it changes is SteamVR's `supersampleScale` with its
//! manual override, through SteamVR's public interface, by `orbiters_booster::steamvr::Sharpness`.
//!
//! Nothing of that runs on the overlay's own turn, which reads the controllers: the journal waits on a lock another process may
//! hold and flushes to disk, so SteamVR's settings and the journal are on a thread of their own ([`Worker`]), as the log is.
//! The turn only says what it wants and hears back what came of it.

use crate::backend::Ctl;
use crate::booster::OverlayReport;
use crate::state::Snapshot;
use orbiters_booster::engine::{Engine, Inputs, LeftAlone, Request, Wants};
use orbiters_booster::frames::Frame;
use orbiters_booster::log::{BlockLine, Logger, SteamVrRead};
use orbiters_booster::steamvr::{self, SettingsApi, Sharpness};
use orbiters_booster::{vrsettings, Level};
use orbiters_openvr::compositor::FrameTiming;
use orbiters_openvr::{self as vr};
use std::ffi::CString;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// VRChat's Steam app key, for what SteamVR keeps per application: read for the log only.
const VRCHAT_APP: &str = "steam.app.438100";
/// Owned values are looked at this often: a change by hand is let go of at once, not at the end.
const CHECK_OWNED: Duration = Duration::from_secs(5);
const REPORT_EVERY: Duration = Duration::from_secs(1);

/// How often Orbiters' own panels may draw: `MIN_FRAME` in `overlay/mod.rs`, and N4N0's menu, read it. 30 a second on a PC
/// short of graphics card, 45 otherwise. Too small a saving to measure, so nothing ever claims it.
static PANEL_PACE_MS: AtomicU32 = AtomicU32::new(NORMAL_PACE_MS);
const NORMAL_PACE_MS: u32 = 22;
const SLOW_PACE_MS: u32 = 33;

pub fn panel_pace() -> Duration {
    Duration::from_millis(PANEL_PACE_MS.load(Ordering::Relaxed) as u64)
}

/// One frame as the booster reads it.
pub fn frame_of(t: &FrameTiming) -> Frame {
    Frame {
        index: t.frame_index,
        time_s: t.system_time_s,
        presents: t.num_frame_presents,
        mis_presented: t.num_mis_presented,
        dropped: t.num_dropped_frames,
        flags: t.reprojection_flags,
        interval_ms: t.client_frame_interval_ms,
        pre_submit_gpu_ms: t.pre_submit_gpu_ms,
        post_submit_gpu_ms: t.post_submit_gpu_ms,
        total_gpu_ms: t.total_render_gpu_ms,
        compositor_gpu_ms: t.compositor_render_gpu_ms,
        compositor_cpu_ms: t.compositor_render_cpu_ms,
        poses_ready_ms: t.new_poses_ready_ms,
        frame_ready_ms: t.new_frame_ready_ms,
    }
}

fn cstring(text: &str) -> CString {
    CString::new(text).unwrap_or_default()
}

/// SteamVR's settings, with its error codes, for the booster's thread.
struct Api(vr::SettingsHandle);

impl SettingsApi for Api {
    fn get_bool(&self, section: &str, key: &str) -> Result<bool, i32> {
        self.0.get_bool(&cstring(section), &cstring(key))
    }
    fn get_float(&self, section: &str, key: &str) -> Result<f32, i32> {
        self.0.get_float(&cstring(section), &cstring(key))
    }
    fn set_bool(&self, section: &str, key: &str, value: bool) -> Result<(), i32> {
        self.0.set_bool(&cstring(section), &cstring(key), value)
    }
    fn set_float(&self, section: &str, key: &str, value: f32) -> Result<(), i32> {
        self.0.set_float(&cstring(section), &cstring(key), value)
    }
    fn remove(&self, section: &str, key: &str) -> Result<(), i32> {
        self.0.remove(&cstring(section), &cstring(key))
    }
}

fn sharpness() -> Option<Sharpness> {
    Some(Sharpness::new(crate::controller::booster_store()?, vrsettings::settings_path_for_user()))
}

/// The very first thing a new overlay does, before anything else can change anything: what the last one left comes back, if
/// SteamVR answers. At start, so on this thread: nothing else waits on it yet.
pub fn recover_first(client: &vr::Client) {
    if let (Some(mut sharpness), Some(handle)) = (sharpness(), client.settings_handle()) {
        sharpness.recover(&Api(handle));
    }
}

/// What the booster's SteamVR thread is asked.
enum Job {
    Factor(f32),
    Check {
        watching: bool,
    },
    /// The session ends: restore live, unless SteamVR is gone.
    End {
        steamvr_gone: bool,
    },
}

/// What it says back.
enum Heard {
    Left(LeftAlone),
    Shown(Option<(Option<f32>, f32)>),
}

/// The thread that holds the journal's SteamVR side. The client it came from outlives it: [`Host::teardown`] waits for it.
struct Worker {
    jobs: mpsc::Sender<Job>,
    heard: mpsc::Receiver<Heard>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Worker {
    fn start(handle: vr::SettingsHandle) -> Option<Self> {
        let mut sharpness = sharpness()?;
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (tell, heard) = mpsc::channel::<Heard>();
        let thread = std::thread::Builder::new()
            .name("fps-booster-steamvr".into())
            .spawn(move || {
                let api = Api(handle);
                for job in inbox {
                    let left = match job {
                        Job::Factor(factor) => sharpness.follow(&api, factor),
                        Job::Check { watching } => sharpness.check(&api, watching),
                        Job::End { steamvr_gone } => {
                            sharpness.teardown(&api, steamvr_gone);
                            return;
                        }
                    };
                    if let Some(left) = left {
                        let _ = tell.send(Heard::Left(left));
                    }
                    let _ = tell.send(Heard::Shown(sharpness.shown()));
                }
            })
            .ok()?;
        Some(Self { jobs, heard, thread: Some(thread) })
    }
}

/// What the log's thread is asked: writing is off the overlay's turn, which reads the controllers.
enum LogJob {
    Line(Box<BlockLine>),
    Raw(Vec<Frame>, orbiters_booster::trial::Kind),
    SelfCheck { real_fps: f32, refresh_hz: f32 },
}

fn start_log() -> mpsc::Sender<LogJob> {
    let (tx, rx) = mpsc::channel::<LogJob>();
    let _ = std::thread::Builder::new().name("fps-log".into()).spawn(move || {
        let mut logger = Logger::new(orbiters_core::paths::logs_dir());
        for job in rx {
            match job {
                LogJob::Line(mut line) => {
                    line.log_ms = logger.log_ms();
                    logger.write(&*line);
                }
                LogJob::Raw(frames, kind) => {
                    let _ = logger.write_raw(&frames, kind);
                }
                LogJob::SelfCheck { real_fps, refresh_hz } => {
                    let Some(path) = vrsettings::compositor_log_for_user() else { continue };
                    let Some(stats) = std::fs::read_to_string(path).ok().and_then(|t| orbiters_booster::log::parse_compositor(&t).pop()) else { continue };
                    logger.write(&serde_json::json!({ "self_check": orbiters_booster::log::self_check(real_fps, refresh_hz, &stats), "compositor": stats }));
                }
            }
        }
    });
    tx
}

/// The booster's part of the overlay's turn.
pub struct Host {
    engine: Engine,
    worker: Option<Worker>,
    log: mpsc::Sender<LogJob>,
    began: Instant,
    refresh_hz: f32,
    streamer: bool,
    answered: u64,
    world_key: String,
    world_number: u32,
    world_hash: u64,
    last_report: Option<Instant>,
    last_check: Option<Instant>,
    /// The factor last asked of the worker, and what it says is in force.
    asked: f32,
    shown: Option<(Option<f32>, f32)>,
    session_frames: f64,
    session_seconds: f64,
    vrchat_was_running: bool,
}

impl Host {
    pub fn new(client: &vr::Client) -> Self {
        let system = client.string_property(0, vr::prop::TRACKING_SYSTEM_NAME).unwrap_or_default().to_lowercase();
        Self {
            engine: Engine::new(),
            worker: client.settings_handle().and_then(Worker::start),
            log: start_log(),
            began: Instant::now(),
            refresh_hz: client.float_property(0, vr::prop::DISPLAY_FREQUENCY).filter(|hz| *hz > 1.0).unwrap_or(0.0),
            streamer: system.contains("virtualdesktop") || system.contains("alvr"),
            answered: 0,
            world_key: String::new(),
            world_number: 0,
            world_hash: 0,
            last_report: None,
            last_check: None,
            asked: 1.0,
            shown: None,
            session_frames: 0.0,
            session_seconds: 0.0,
            vrchat_was_running: false,
        }
    }

    /// One turn, with the frames read since the last. Whether a 3D panel shows: said by the caller, as what is here does not
    /// know it.
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, client: &vr::Client, backend: &Ctl, snap: &Snapshot, frames: &[Frame], scene: bool, dashboard: bool, panel_3d: bool) -> Wants {
        let world = &snap.vrchat.world;
        let key = world.instance.clone().unwrap_or_default();
        if key != self.world_key {
            self.world_key = key;
            self.world_number += 1;
            // Only that it changed: no ID is kept or logged.
            self.world_hash = self.world_number as u64;
        }
        let inputs = Inputs {
            now: self.began.elapsed().as_secs_f64(),
            scene,
            dashboard,
            world: self.world_hash,
            people: (!world.players.is_empty()).then_some(world.players.len() as u32),
            recording: snap.capture.state != crate::state::CaptureState::Idle,
            level: snap.settings.booster.level,
            refresh_hz: self.refresh_hz,
            listed_cores: snap.booster.listed_cores,
            panel_3d,
            reconstruction: snap.reconstruction.as_ref().is_some_and(|j| !j.paused),
        };
        if scene {
            self.engine.push(&inputs, frames);
        }
        // Each request once, by its number, however many snapshots carry it.
        let mut request = None;
        if let Some((number, asked)) = snap.booster.request {
            if number > self.answered {
                self.answered = number;
                request = Some(asked);
            }
        }
        if request == Some(Request::SimulateCrash) && crate::controller::booster_dev_build() {
            // Test builds' "Simulate crash": nothing tidies up, as in a real one, and the next overlay restores first.
            std::process::abort();
        }
        self.hear();
        let wants = self.engine.tick(&inputs, request);
        if let Some(worker) = &self.worker {
            if (wants.sharpness - self.asked).abs() > 0.0005 {
                self.asked = wants.sharpness;
                let _ = worker.jobs.send(Job::Factor(wants.sharpness));
            }
            if self.last_check.is_none_or(|at| at.elapsed() >= CHECK_OWNED) {
                self.last_check = Some(Instant::now());
                let _ = worker.jobs.send(Job::Check { watching: scene && snap.settings.booster.level >= Level::Picture });
            }
        }
        PANEL_PACE_MS.store(if wants.pace_panels { SLOW_PACE_MS } else { NORMAL_PACE_MS }, Ordering::Relaxed);
        self.write_log(client, snap, &inputs, &wants);
        self.self_check_at_session_end(snap);
        if self.last_report.is_none_or(|at| at.elapsed() >= REPORT_EVERY) {
            self.last_report = Some(Instant::now());
            let report = OverlayReport {
                report: self.engine.report(&inputs, &wants),
                streamer: self.streamer,
                sharpness: self.shown,
                answered: self.answered,
                panel_3d,
            };
            backend.booster_report(report);
        }
        wants
    }

    /// What the worker said since the last turn.
    fn hear(&mut self) {
        let Some(worker) = &self.worker else { return };
        while let Ok(heard) = worker.heard.try_recv() {
            match heard {
                Heard::Left(why) => {
                    self.engine.leave_sharpness_alone(why);
                    // The engine wants 1.0 from now on: asked again, so that whatever is owed is restored.
                    self.asked = f32::NAN;
                }
                Heard::Shown(shown) => self.shown = shown,
            }
        }
    }

    fn write_log(&mut self, client: &vr::Client, snap: &Snapshot, inputs: &Inputs, wants: &Wants) {
        if let Some((frames, kind)) = self.engine.take_raw_if_any() {
            let _ = self.log.send(LogJob::Raw(frames, kind));
        }
        let Some(block) = self.engine.take_block() else { return };
        self.session_frames += block.summary.frames as f64;
        self.session_seconds += block.summary.seconds as f64;
        let report = self.engine.report(inputs, wants);
        let (section, app) = (cstring(steamvr::SECTION), cstring(VRCHAT_APP));
        let steamvr = SteamVrRead {
            disable_async: client.setting_bool(&section, &cstring("disableAsync")),
            motion_smoothing: client.setting_bool(&section, &cstring("motionSmoothing")),
            supersample_manual_override: client.setting_bool(&section, &cstring(steamvr::MANUAL_OVERRIDE)),
            supersample_scale: client.setting_float(&section, &cstring(steamvr::SCALE)),
            app_resolution_scale: client.setting_float(&app, &cstring("resolutionScale")),
        };
        let line = BlockLine {
            at: orbiters_core::now_utc_iso(),
            summary: block.summary,
            refresh_hz: inputs.refresh_hz,
            level: inputs.level,
            limit: block.limit,
            line: report.line,
            wants: *wants,
            lowered: snap.booster.lowered.iter().map(|(_, n)| *n).sum(),
            trial: report.testing.map(|t| (t.kind, t.side)),
            people: inputs.people,
            world_number: self.world_number,
            reconstruction: inputs.reconstruction,
            recording: inputs.recording,
            panel_3d: inputs.panel_3d,
            cpu_total: snap.booster.cpu_total,
            top: snap.booster.top.clone(),
            steamvr,
            log_ms: 0.0,
        };
        let _ = self.log.send(LogJob::Line(Box::new(line)));
    }

    /// VRChat left: what this log counted against what SteamVR's compositor counted, in the log.
    fn self_check_at_session_end(&mut self, snap: &Snapshot) {
        let running = snap.vrchat.running;
        if self.vrchat_was_running && !running && self.session_seconds > 0.0 {
            let real_fps = (self.session_frames / self.session_seconds) as f32;
            let _ = self.log.send(LogJob::SelfCheck { real_fps, refresh_hz: self.refresh_hz });
            (self.session_frames, self.session_seconds) = (0.0, 0.0);
        }
        self.vrchat_was_running = running;
    }

    /// The session ends. With SteamVR still there, everything comes back before it does; with SteamVR gone (a crash, its
    /// server ended), nothing is restored through it, and what is owed stays in the journal for its file and the next overlay.
    /// Waits for the worker, whose SteamVR handle must not outlive the client.
    pub fn teardown(&mut self, steamvr_gone: bool) {
        PANEL_PACE_MS.store(NORMAL_PACE_MS, Ordering::Relaxed);
        if let Some(mut worker) = self.worker.take() {
            let _ = worker.jobs.send(Job::End { steamvr_gone });
            if let Some(thread) = worker.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layout's fields, as the booster reads them: any mistake here would mislabel every figure in the log.
    #[test]
    fn a_frame_reads_the_right_fields() {
        let t = FrameTiming {
            frame_index: 7,
            system_time_s: 2.5,
            num_frame_presents: 3,
            num_mis_presented: 4,
            num_dropped_frames: 5,
            reprojection_flags: 0x21,
            client_frame_interval_ms: 14.0,
            pre_submit_gpu_ms: 6.0,
            post_submit_gpu_ms: 1.0,
            total_render_gpu_ms: 9.0,
            compositor_render_gpu_ms: 0.5,
            compositor_render_cpu_ms: 0.7,
            new_poses_ready_ms: 2.0,
            new_frame_ready_ms: 13.0,
            ..FrameTiming::default()
        };
        let f = frame_of(&t);
        assert_eq!((f.index, f.presents, f.mis_presented, f.dropped, f.flags), (7, 3, 4, 5, 0x21));
        assert_eq!((f.time_s, f.interval_ms, f.total_gpu_ms), (2.5, 14.0, 9.0));
        assert_eq!((f.app_gpu_ms(), f.app_cpu_ms()), (7.0, 11.0));
        assert_eq!((f.compositor_gpu_ms, f.compositor_cpu_ms), (0.5, 0.7));
    }
}
