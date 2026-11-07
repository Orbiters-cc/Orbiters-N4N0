//! The FPS booster in the overlay: where SteamVR's frames are, and the only process that talks to SteamVR. It runs the
//! frame logger at every level, Off included, and acts on SteamVR's settings by the journal: the one thing that changes there
//! is `steamvr.supersampleScale` with its manual override, through SteamVR's public interface. No window into VRChat, and
//! nothing written of its own.

use crate::backend::Ctl;
use crate::booster::OverlayReport;
use crate::state::Snapshot;
use orbiters_booster::engine::{Engine, Inputs, LeftAlone, Request, ValueWatch, Wants};
use orbiters_booster::frames::Frame;
use orbiters_booster::journal::{Access, Applied, Change, Read, Store, Target, Value};
use orbiters_booster::log::{BlockLine, Logger, SteamVrRead};
use orbiters_booster::{vrsettings, Level};
use orbiters_openvr::compositor::FrameTiming;
use orbiters_openvr::{self as vr};
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const SECTION: &str = "steamvr";
const MANUAL_OVERRIDE: &str = "supersampleManualOverride";
const SCALE: &str = "supersampleScale";
const AUTO_SECTION: &str = "GpuSpeed";
const AUTO_SCALE: &str = "gpuSpeedRenderTargetScale";
/// VRChat's Steam app key, for what SteamVR keeps per application: read for the log only.
const VRCHAT_APP: &str = "steam.app.438100";
/// Owned values are looked at this often: a change by hand is let go of at once, not at the end.
const CHECK_OWNED: Duration = Duration::from_secs(5);
const REPORT_EVERY: Duration = Duration::from_secs(1);
/// Lower than this, whatever it was, nothing goes.
const SHARPNESS_FLOOR: f32 = 0.7;
/// Nor below this share of what it was.
const SHARPNESS_FLOOR_OF: f32 = 0.7;

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

/// SteamVR's settings, live. It answers a key not set with its default, so whether a key is there at all comes from its file.
struct Live<'a> {
    client: &'a vr::Client,
    file: Option<PathBuf>,
    /// Taken away this session: its file may not say so yet.
    removed: Vec<String>,
}

impl Live<'_> {
    fn in_file(&self, section: &str, key: &str) -> Option<bool> {
        let text = std::fs::read_to_string(self.file.as_ref()?).ok()?;
        Some(vrsettings::read_value(&text, section, key) != Value::Absent)
    }

    fn read_value(&self, section: &str, key: &str) -> Value {
        let (section, key_c) = (cstring(section), cstring(key));
        if key == MANUAL_OVERRIDE {
            return self.client.setting_bool(&section, &key_c).map_or(Value::Absent, Value::Bool);
        }
        // Unset is told by the error code, and is not 0.
        self.client.setting_float(&section, &key_c).map_or(Value::Absent, Value::Float)
    }
}

impl Access for Live<'_> {
    fn read(&mut self, target: &Target) -> Read {
        let Target::SteamVr { section, key } = target else { return Read::Unreachable };
        Read::Value(self.read_value(section, key))
    }

    fn write(&mut self, target: &Target, value: &Value) -> bool {
        let Target::SteamVr { section, key } = target else { return false };
        let (section_c, key_c) = (cstring(section), cstring(key));
        match value {
            Value::Absent => {
                let done = self.client.remove_setting(&section_c, &key_c);
                if done {
                    self.removed.push(key.clone());
                }
                done
            }
            Value::Bool(b) => self.client.set_setting_bool(&section_c, &key_c, *b),
            Value::Float(f) => self.client.set_setting_float(&section_c, &key_c, *f),
            Value::Int(_) => false,
        }
    }

    fn absent_now(&mut self, target: &Target) -> bool {
        let Target::SteamVr { section, key } = target else { return false };
        self.removed.contains(key) || self.in_file(section, key) == Some(false)
    }

    fn original(&mut self, target: &Target) -> Read {
        let Target::SteamVr { section, key } = target else { return Read::Unreachable };
        // The key not there in the file is what was here: absent, not the default SteamVR answers with.
        if self.in_file(section, key) == Some(false) {
            return Read::Value(Value::Absent);
        }
        Read::Value(self.read_value(section, key))
    }
}

fn is_steamvr(e: &orbiters_booster::journal::Entry) -> bool {
    matches!(e.target, Target::SteamVr { .. })
}

fn the_store() -> Option<Store> {
    crate::controller::booster_store()
}

/// The very first thing a new overlay does: what the last one left comes back, before anything else can change anything.
pub fn recover_first(client: &vr::Client) {
    let Some(store) = the_store() else { return };
    let mut live = Live { client, file: vrsettings::settings_path_for_user(), removed: Vec::new() };
    let _ = store.restore(&mut live, &is_steamvr, &mut |_| {});
    let _ = store.rearm();
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
    store: Option<Store>,
    log: mpsc::Sender<LogJob>,
    began: Instant,
    settings_file: Option<PathBuf>,
    refresh_hz: f32,
    streamer: bool,
    answered: u64,
    world_key: String,
    world_number: u32,
    world_hash: u64,
    last_report: Option<Instant>,
    last_check: Option<Instant>,
    /// What SteamVR's sharpness was before the booster, and the factor in force now.
    base: Option<(Option<f32>, f32)>,
    factor: f32,
    watch: ValueWatch,
    removed: Vec<String>,
    session_frames: f64,
    session_seconds: f64,
    vrchat_was_running: bool,
}

impl Host {
    pub fn new(client: &vr::Client) -> Self {
        let system = client.string_property(0, vr::prop::TRACKING_SYSTEM_NAME).unwrap_or_default().to_lowercase();
        Self {
            engine: Engine::new(),
            store: the_store(),
            log: start_log(),
            began: Instant::now(),
            settings_file: vrsettings::settings_path_for_user(),
            refresh_hz: client.float_property(0, vr::prop::DISPLAY_FREQUENCY).filter(|hz| *hz > 1.0).unwrap_or(0.0),
            streamer: system.contains("virtualdesktop") || system.contains("alvr"),
            answered: 0,
            world_key: String::new(),
            world_number: 0,
            world_hash: 0,
            last_report: None,
            last_check: None,
            base: None,
            factor: 1.0,
            watch: ValueWatch::default(),
            removed: Vec::new(),
            session_frames: 0.0,
            session_seconds: 0.0,
            vrchat_was_running: false,
        }
    }

    /// One turn, with the frames read since the last. Whether a 3D panel shows and what is held there: said by the caller, as
    /// what is here does not know it.
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
        let wants = self.engine.tick(&inputs, request);
        self.follow_sharpness(client, wants.sharpness);
        self.check_owned(client, scene, snap.settings.booster.level);
        PANEL_PACE_MS.store(if wants.pace_panels { SLOW_PACE_MS } else { NORMAL_PACE_MS }, Ordering::Relaxed);
        self.write_log(client, snap, &inputs, &wants);
        self.self_check_at_session_end(snap);
        if self.last_report.is_none_or(|at| at.elapsed() >= REPORT_EVERY) {
            self.last_report = Some(Instant::now());
            let report = OverlayReport {
                report: self.engine.report(&inputs, &wants),
                streamer: self.streamer,
                sharpness: self.base.filter(|_| self.factor < 1.0).map(|(was, base)| (was, (base * self.factor).max(floor_of(base)))),
                answered: self.answered,
                panel_3d,
            };
            backend.booster_report(report);
        }
        wants
    }

    fn live<'a>(&mut self, client: &'a vr::Client) -> Live<'a> {
        Live { client, file: self.settings_file.clone(), removed: std::mem::take(&mut self.removed) }
    }

    /// What it is before the booster: what SteamVR's own Auto chose counts as the base when no manual value is set.
    fn base_now(client: &vr::Client) -> (Option<f32>, f32) {
        let manual = client.setting_bool(&cstring(SECTION), &cstring(MANUAL_OVERRIDE)).unwrap_or(false);
        let scale = client.setting_float(&cstring(SECTION), &cstring(SCALE));
        if manual {
            let scale = scale.unwrap_or(1.0);
            return (Some(scale), scale);
        }
        (None, client.setting_float(&cstring(AUTO_SECTION), &cstring(AUTO_SCALE)).unwrap_or(1.0))
    }

    fn follow_sharpness(&mut self, client: &vr::Client, factor: f32) {
        if (factor - self.factor).abs() < 0.001 {
            return;
        }
        let Some(store) = self.store.take() else { return };
        if factor >= 0.999 {
            let mut live = self.live(client);
            let got = store.restore(&mut live, &is_steamvr, &mut |_| {});
            self.removed = live.removed;
            if got.is_ok_and(|g| !g.kept.is_empty()) {
                self.engine.leave_sharpness_alone(LeftAlone::HandChanged);
            }
            self.base = None;
            self.factor = 1.0;
            self.watch.forget_last();
        } else {
            let (_, base) = *self.base.get_or_insert_with(|| Self::base_now(client));
            let target = (base * factor).max(floor_of(base));
            if target < base {
                let scale = Target::SteamVr { section: SECTION.into(), key: SCALE.into() };
                let manual = Target::SteamVr { section: SECTION.into(), key: MANUAL_OVERRIDE.into() };
                let changes = [
                    Change { target: manual, value: Value::Bool(true), leave_with: Some(scale.clone()) },
                    Change { target: scale, value: Value::Float(target), leave_with: None },
                ];
                let mut live = self.live(client);
                let got = store.apply(&mut live, &changes, &mut |_| {});
                self.removed = live.removed;
                match got {
                    Ok(results) if results.iter().any(|r| *r == Applied::ChangedSince) => self.engine.leave_sharpness_alone(LeftAlone::HandChanged),
                    Ok(results) if results.iter().all(|r| matches!(r, Applied::Done | Applied::AlreadySo)) => {
                        self.factor = factor;
                        self.watch.forget_last();
                    }
                    // Refused, or nothing could be recorded: nothing is in force, and it is not tried again here.
                    _ => {
                        self.base = None;
                        self.engine.leave_sharpness_alone(LeftAlone::Refused);
                    }
                }
            }
        }
        self.store = Some(store);
    }

    /// Every few seconds, the value the booster owns, or the one it might come to own: a change without it is someone's.
    fn check_owned(&mut self, client: &vr::Client, scene: bool, level: Level) {
        if self.last_check.is_some_and(|at| at.elapsed() < CHECK_OWNED) {
            return;
        }
        self.last_check = Some(Instant::now());
        if self.factor < 1.0 {
            let Some(store) = self.store.take() else { return };
            let mut live = self.live(client);
            let kept = store.release_changed(&mut live, &is_steamvr);
            self.removed = live.removed;
            if kept.is_ok_and(|k| !k.is_empty()) {
                self.factor = 1.0;
                self.base = None;
                self.engine.leave_sharpness_alone(LeftAlone::HandChanged);
            }
            self.store = Some(store);
        } else if scene && level >= Level::Picture {
            let value = client.setting_float(&cstring(SECTION), &cstring(SCALE));
            if value.is_some_and(|v| self.watch.see(v)) {
                self.engine.leave_sharpness_alone(LeftAlone::AnotherApp);
            }
        }
    }

    fn write_log(&mut self, client: &vr::Client, snap: &Snapshot, inputs: &Inputs, wants: &Wants) {
        let raw = self.engine.take_raw_if_any();
        if let Some((frames, kind)) = raw {
            let _ = self.log.send(LogJob::Raw(frames, kind));
        }
        let Some(block) = self.engine.take_block() else { return };
        self.session_frames += block.summary.frames as f64;
        self.session_seconds += block.summary.seconds as f64;
        let report = self.engine.report(inputs, wants);
        let (section, app) = (cstring(SECTION), cstring(VRCHAT_APP));
        let steamvr = SteamVrRead {
            disable_async: client.setting_bool(&section, &cstring("disableAsync")),
            motion_smoothing: client.setting_bool(&section, &cstring("motionSmoothing")),
            supersample_manual_override: client.setting_bool(&section, &cstring(MANUAL_OVERRIDE)),
            supersample_scale: client.setting_float(&section, &cstring(SCALE)),
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

    /// The session is ending, cleanly: everything comes back before it does.
    pub fn teardown(&mut self, client: &vr::Client) {
        PANEL_PACE_MS.store(NORMAL_PACE_MS, Ordering::Relaxed);
        let Some(store) = self.store.take() else { return };
        let mut live = self.live(client);
        let _ = store.restore(&mut live, &is_steamvr, &mut |_| {});
        let _ = store.rearm();
    }
}

fn floor_of(base: f32) -> f32 {
    (base * SHARPNESS_FLOOR_OF).max(SHARPNESS_FLOOR)
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

    #[test]
    fn sharpness_has_a_floor() {
        assert_eq!(floor_of(1.0), 0.7);
        assert_eq!(floor_of(2.0), 1.4);
        assert_eq!(floor_of(0.5), 0.7, "from a low value, the booster has nothing to take");
    }
}
