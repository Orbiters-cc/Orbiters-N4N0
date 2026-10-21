//! The app's brain, independent of the window: owns the capture worker, watches VRChat (log, process, Spout
//! senders) and the session library, listens to the Action Menu over OSC, and publishes a [`Snapshot`] that
//! the interface reads every frame. It keeps working while the window is hidden in the tray.

use crate::ipc::ToClient;
use crate::reconstruct::ReconstructHandle;
use crate::state::{self, CalibrationInfo, CaptureState, MapsMine, ReconstructionJob, Snapshot};
use crate::worker::WorkerHandle;
use orbiters_contracts::manifest::{ConsentInfo, WorldInfo};
use orbiters_contracts::performance::calibration::Calibration;
use orbiters_core::bundle::{self, SessionSummary};
use orbiters_core::clock;
use orbiters_core::osc::{OscEvent, OscService, OscValue, AVATAR_CHANGE, DEPTH_FRAMES_PARAMETER, PARAMETERS_PREFIX};
use orbiters_core::protocol::{OscSample, PerformanceSpec, SourceSpec, StartCapture, WorkerCommand, WorkerEvent};
use orbiters_core::reconstruction::Event as ReconstructEvent;
use orbiters_core::settings::Settings;
use orbiters_core::visited;
use orbiters_core::vrchat_log::{LogEvent, LogWatcher, WorldState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Info,
    Success,
    Warning,
    Error,
}

/// Something the interface should tell the person about.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Notice {
    Toast { kind: NoticeKind, message: String },
    SessionSaved { reason: String, frames: u64, keyframes: u64, depth_frames: u64 },
}

/// A raw capture written by Mixer's capture simulator.
#[derive(Clone, Debug)]
pub struct RawCaptureSummary {
    pub dir: PathBuf,
    pub name: String,
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub depth_every: u32,
    pub has_ground_truth: bool,
    pub created_utc: String,
}

/// OSC values VRChat sent: the latest of every address (a session's initial values) and, while a session
/// records, the batch on its way to the worker.
#[derive(Default)]
struct OscValues {
    latest: HashMap<String, (Vec<OscValue>, u64)>,
    forwarding: bool,
    pending: Vec<OscSample>,
    last_flush: Option<Instant>,
}

/// One sample per argument (`index` only for messages with several).
fn osc_samples<'a>(address: &'a str, values: &'a [OscValue], at_ns: u64, initial: bool) -> impl Iterator<Item = OscSample> + 'a {
    let several = values.len() > 1;
    values.iter().enumerate().map(move |(i, value)| OscSample {
        at_ns,
        address: address.to_string(),
        index: several.then_some(i as u8),
        value: value.clone(),
        initial,
    })
}

/// Initial values of a session: what VRChat's OSCQuery tree reported (fetched from `fetch_started_ns`), and the
/// values that arrived since or that the tree does not list (head and wrist poses).
fn initial_samples(vrchat: Vec<(String, OscValue)>, latest: &HashMap<String, (Vec<OscValue>, u64)>, fetch_started_ns: u64) -> Vec<OscSample> {
    let mut merged: BTreeMap<String, Vec<OscValue>> = vrchat.into_iter().map(|(a, v)| (a, vec![v])).collect();
    for (address, (values, at)) in latest {
        if *at >= fetch_started_ns || !merged.contains_key(address) {
            merged.insert(address.clone(), values.clone());
        }
    }
    merged.iter().flat_map(|(address, values)| osc_samples(address, values, 0, true).collect::<Vec<_>>()).collect()
}

pub struct Controller {
    snapshot: Mutex<Snapshot>,
    /// What the interface reads: a copy of the snapshot with a light trail, swapped on every change.
    published: Mutex<Arc<Snapshot>>,
    worker: Mutex<Option<WorkerHandle>>,
    osc: Mutex<Option<OscService>>,
    osc_values: Mutex<OscValues>,
    worker_log: Mutex<VecDeque<String>>,
    notices: Mutex<Vec<Notice>>,
    /// The source of the running capture, reused when a world change starts a new segment.
    last_source: Mutex<Option<SourceSpec>>,
    /// Bumped when the session library changes.
    library_version: AtomicU64,
    /// An interface in this process (screenshots, tests) repaints on every change.
    repaint: Mutex<Option<egui::Context>>,
    /// Interfaces in other processes (window, overlay) get every change over their link.
    subscribers: Mutex<Vec<mpsc::Sender<ToClient>>>,
    /// The latest T-pose calibration and when it was taken (Unix ms; the shared clock restarts with Windows).
    calibration: Mutex<Option<StoredCalibration>>,
    reconstruction: Mutex<Option<Reconstruction>>,
    /// Sessions waiting to be reconstructed, first first: 3D pictures taken while one ran, and a run a capture stopped.
    reconstruct_queue: Mutex<VecDeque<PathBuf>>,
    /// VRChat's camera as it was before a recording opened it in Stream mode (put back when recording ends).
    camera_before: Mutex<Option<CameraState>>,
    /// The capture kit's toggles as they were before a recording started from the app switched them on (Some while
    /// the app drives them), and when the avatar last (re)loaded, which resets them.
    avatar_before: Mutex<Option<AvatarToggles>>,
    avatar_changed_at: Mutex<Option<Instant>>,
    /// The Orbiters account's credential and a sign-in in progress (controller/account.rs).
    account_runtime: Mutex<account::AccountRuntime>,
    /// What this member added to the Map explorer, and the uploads' numbering (controller/maps.rs).
    maps_mine: Mutex<Option<MapsMine>>,
    upload_serial: AtomicU64,
    /// Orb, the companion on the avatar (controller/orb.rs), and its ears (controller/voice.rs).
    orb: Mutex<orb::OrbRuntime>,
    voice: Mutex<voice::VoiceRuntime>,
    /// VRChat's frame rate per world, gathered for Orbiters (controller/performance.rs).
    performance: Mutex<performance::PerformanceRuntime>,
    /// A 3D picture under way (controller/burst.rs).
    burst: Mutex<burst::BurstRuntime>,
    /// A scan under way (controller/scan.rs).
    scan: Mutex<scan::ScanRuntime>,
    /// Calls with VRChat friends (controller/calls.rs).
    calls: Mutex<calls::CallsRuntime>,
}

#[derive(Clone, Copy, Debug)]
struct AvatarToggles {
    record: bool,
    depth: bool,
}

/// After an avatar (re)load, the record toggle's reset to off is the reload's, not the person's.
const AVATAR_RELOAD_GRACE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug)]
struct CameraState {
    mode: i32,
    streaming: bool,
}

const CAMERA_MODE: &str = "/usercamera/Mode";
const CAMERA_STREAMING: &str = "/usercamera/Streaming";
/// `/usercamera/Mode` for the Stream camera (0 off, 1 photo, 2 stream).
const STREAM_MODE: i32 = 2;

/// The running reconstruction worker, and whether its end was already told (finished, failed or cancelled).
struct Reconstruction {
    handle: ReconstructHandle,
    dir: PathBuf,
    told: bool,
}

/// `calibration.json` in the config folder: the latest calibration, kept across restarts.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredCalibration {
    utc_ms: u64,
    calibration: Calibration,
}

fn calibration_file() -> PathBuf {
    orbiters_core::paths::config_dir().join("calibration.json")
}

pub(crate) fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn calibration_info(stored: &StoredCalibration) -> CalibrationInfo {
    let c = &stored.calibration;
    CalibrationInfo { head_height_m: c.head_height_m, arm_span_m: c.arm_span_m, devices: c.devices.len(), utc_ms: stored.utc_ms }
}

pub type Shared = Arc<Controller>;

mod account;
mod burst;
mod calls;
mod maps;
mod orb;
mod performance;
mod scan;
pub(crate) mod screen;
mod voice;
pub use account::open_url;

impl Controller {
    pub fn new(settings: Settings) -> Shared {
        let mut snapshot = Snapshot::new(settings);
        let calibration = std::fs::read_to_string(calibration_file()).ok().and_then(|t| serde_json::from_str::<StoredCalibration>(&t).ok());
        snapshot.steamvr.calibration = calibration.as_ref().map(calibration_info);
        Arc::new(Self {
            published: Mutex::new(Arc::new(snapshot.clone())),
            snapshot: Mutex::new(snapshot),
            worker: Mutex::new(None),
            osc: Mutex::new(None),
            osc_values: Mutex::new(OscValues::default()),
            worker_log: Mutex::new(VecDeque::new()),
            notices: Mutex::new(Vec::new()),
            last_source: Mutex::new(None),
            library_version: AtomicU64::new(0),
            repaint: Mutex::new(None),
            subscribers: Mutex::new(Vec::new()),
            calibration: Mutex::new(calibration),
            reconstruction: Mutex::new(None),
            reconstruct_queue: Mutex::new(VecDeque::new()),
            camera_before: Mutex::new(None),
            avatar_before: Mutex::new(None),
            avatar_changed_at: Mutex::new(None),
            account_runtime: Mutex::new(account::AccountRuntime::default()),
            maps_mine: Mutex::new(None),
            upload_serial: AtomicU64::new(0),
            orb: Mutex::new(orb::OrbRuntime::default()),
            voice: Mutex::new(voice::VoiceRuntime::default()),
            performance: Mutex::new(performance::PerformanceRuntime::default()),
            burst: Mutex::new(burst::BurstRuntime::default()),
            scan: Mutex::new(scan::ScanRuntime::default()),
            calls: Mutex::new(calls::CallsRuntime::default()),
        })
    }

    /// Starts OSC, the worker and the watchers.
    pub fn start(self: &Shared) {
        self.start_osc();
        let c = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = c.ensure_worker() {
                eprintln!("capture worker: {e}");
            }
        });
        spawn_watchers(self.clone());
        self.start_account();
        self.start_orb();
        self.start_calls();
    }

    /// The interface's context, repainted on every change.
    pub fn attach(&self, ctx: &egui::Context) {
        *self.repaint.lock().unwrap() = Some(ctx.clone());
    }

    /// Sends the current state to `tx` now and every change from then on (until the receiver is gone). Notices
    /// that arrived while nobody listened are delivered first.
    pub fn subscribe(&self, tx: mpsc::Sender<ToClient>) {
        let _ = tx.send(ToClient::Snapshot { snapshot: Box::new((*self.view()).clone()) });
        let _ = tx.send(ToClient::Library { version: self.library_version() });
        for notice in self.take_notices() {
            let _ = tx.send(ToClient::Notice { notice });
        }
        self.subscribers.lock().unwrap().push(tx);
    }

    fn broadcast(&self, msg: impl Fn() -> ToClient) -> bool {
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|tx| tx.send(msg()).is_ok());
        !subs.is_empty()
    }

    fn push_notice(&self, notice: Notice) {
        if !self.broadcast(|| ToClient::Notice { notice: notice.clone() }) {
            let mut pending = self.notices.lock().unwrap();
            pending.push(notice);
            let excess = pending.len().saturating_sub(8);
            pending.drain(..excess);
        }
        if let Some(ctx) = self.repaint.lock().unwrap().as_ref() {
            ctx.request_repaint();
        }
    }

    fn bump_library(&self) {
        let version = self.library_version.fetch_add(1, Ordering::Relaxed) + 1;
        self.broadcast(|| ToClient::Library { version });
    }

    pub fn view(&self) -> Arc<Snapshot> {
        self.published.lock().unwrap().clone()
    }

    pub fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut *self.notices.lock().unwrap())
    }

    pub fn library_version(&self) -> u64 {
        self.library_version.load(Ordering::Relaxed)
    }

    /// Replaces the whole snapshot (screenshots and tests).
    pub fn set_snapshot(&self, snapshot: Snapshot) {
        *self.snapshot.lock().unwrap() = snapshot;
        self.publish();
    }

    fn publish(&self) {
        let mut s = self.snapshot.lock().unwrap().clone();
        // The interface only needs a light trail.
        if s.capture.trail.len() > 1500 {
            let step = s.capture.trail.len().div_ceil(1500);
            s.capture.trail = s.capture.trail.iter().step_by(step).copied().collect();
        }
        let s = Arc::new(s);
        *self.published.lock().unwrap() = s.clone();
        self.broadcast(|| ToClient::Snapshot { snapshot: Box::new((*s).clone()) });
        if let Some(ctx) = self.repaint.lock().unwrap().as_ref() {
            ctx.request_repaint();
        }
    }

    fn update<R>(&self, f: impl FnOnce(&mut Snapshot) -> R) -> R {
        let r = f(&mut self.snapshot.lock().unwrap());
        self.publish();
        r
    }

    /// Applies a change and publishes only when `f` reports one.
    fn update_if(&self, f: impl FnOnce(&mut Snapshot) -> bool) -> bool {
        let changed = f(&mut self.snapshot.lock().unwrap());
        if changed {
            self.publish();
        }
        changed
    }

    pub fn notify(&self, kind: NoticeKind, message: impl Into<String>) {
        self.push_notice(Notice::Toast { kind, message: message.into() });
    }

    pub fn settings(&self) -> Settings {
        self.snapshot.lock().unwrap().settings.clone()
    }

    pub fn is_capturing(&self) -> bool {
        self.snapshot.lock().unwrap().is_capturing()
    }

    fn ensure_worker(self: &Shared) -> anyhow::Result<()> {
        let mut guard = self.worker.lock().unwrap();
        if let Some(w) = guard.as_mut() {
            if w.alive() {
                return Ok(());
            }
        }
        let on_event = {
            let ctl = self.clone();
            move |e| ctl.on_worker_event(e)
        };
        let on_log = {
            let ctl = self.clone();
            move |line: String| {
                let mut log = ctl.worker_log.lock().unwrap();
                log.push_back(line);
                while log.len() > 400 {
                    log.pop_front();
                }
            }
        };
        let on_exit = {
            let ctl = self.clone();
            move || {
                ctl.put_vrchat_back();
                ctl.update(|s| {
                    s.worker.ready = false;
                    if s.is_capturing() {
                        s.capture.state = CaptureState::Idle;
                        s.capture.error = Some("The capture worker stopped unexpectedly.".into());
                    }
                });
            }
        };
        let handle = WorkerHandle::spawn(on_event, on_log, on_exit).inspect_err(|e| {
            self.update(|s| s.worker.error = Some(e.to_string()));
        })?;
        *guard = Some(handle);
        Ok(())
    }

    fn send(self: &Shared, cmd: &WorkerCommand) -> anyhow::Result<()> {
        self.ensure_worker()?;
        let mut guard = self.worker.lock().unwrap();
        guard.as_mut().expect("worker").send(cmd)
    }

    fn on_worker_event(self: &Shared, e: WorkerEvent) {
        match e {
            WorkerEvent::Ready { version } => {
                self.update(|s| {
                    s.worker.ready = true;
                    s.worker.error = (version != orbiters_core::APP_VERSION).then(|| format!("worker version {version} differs from the app"));
                });
                // The microphones for the settings.
                let ctl = self.clone();
                std::thread::spawn(move || {
                    let _ = ctl.send(&WorkerCommand::ListSources);
                });
            }
            // Spout senders are watched here (`spawn_watchers`); the worker knows the microphones.
            WorkerEvent::Sources { mics, .. } => {
                self.update_if(|s| {
                    let changed = s.mics != mics;
                    s.mics = mics;
                    changed
                });
            }
            WorkerEvent::Started { session_dir, source } => self.update(|s| {
                if s.capture.state != CaptureState::Importing {
                    s.capture.state = CaptureState::Recording;
                }
                s.capture.session_dir = Some(session_dir);
                s.capture.source = Some(source);
                s.capture.error = None;
                s.capture.trail.clear();
                s.capture.coverage = None;
            }),
            // Published with the next stats (four times a second).
            WorkerEvent::Coverage(map) => self.snapshot.lock().unwrap().capture.coverage = Some(map),
            WorkerEvent::Stats(stats) => {
                let over_limit = {
                    let mut s = self.snapshot.lock().unwrap();
                    if let Some(p) = stats.position {
                        s.push_trail(p);
                    }
                    if s.capture.state == CaptureState::Recording && stats.paused {
                        s.capture.state = CaptureState::Paused;
                    } else if s.capture.state == CaptureState::Paused && !stats.paused {
                        s.capture.state = CaptureState::Recording;
                    }
                    let limit = s.settings.storage_limit_gb;
                    let over = limit > 0.0 && (s.storage.used_bytes + stats.bytes) as f64 > limit * 1e9 && s.capture.state == CaptureState::Recording;
                    s.capture.stats = Some(stats);
                    over
                };
                // Stats arrive four times a second; that is also the interface's refresh rate while recording.
                self.publish();
                if over_limit {
                    self.notify(NoticeKind::Warning, "Storage limit reached: recording stopped. Raise the limit in Settings or free space.");
                    let _ = self.stop_capture();
                }
            }
            WorkerEvent::Stopped { manifest, reason, .. } => {
                let restarting = reason == "restarted";
                let session = self.snapshot.lock().unwrap().capture.session_dir.clone();
                if !restarting {
                    self.put_vrchat_back();
                }
                self.update(|s| {
                    if !restarting {
                        s.capture.state = CaptureState::Idle;
                        s.capture.from_osc = false;
                    }
                    s.storage.used_bytes += manifest.stats.bytes;
                    s.storage.sessions += 1;
                });
                self.bump_library();
                self.push_notice(Notice::SessionSaved {
                    reason,
                    frames: manifest.stats.frames,
                    keyframes: manifest.stats.keyframes,
                    depth_frames: manifest.stats.depth_frames,
                });
                if !restarting {
                    self.burst_saved(session.clone());
                    self.scan_saved(session);
                    self.next_reconstruction();
                }
            }
            WorkerEvent::StartFailed { message } => {
                // Nothing records: VRChat's camera and the kit were switched on for nothing.
                self.put_vrchat_back();
                self.update(|s| {
                    s.capture.error = Some(message.clone());
                    if matches!(s.capture.state, CaptureState::Starting | CaptureState::Importing) {
                        s.capture.state = CaptureState::Idle;
                        s.capture.from_osc = false;
                    }
                });
                self.next_reconstruction();
                self.notify(NoticeKind::Error, message);
            }
            WorkerEvent::Error { message } => {
                self.update(|s| {
                    s.capture.error = Some(message.clone());
                    if s.capture.state == CaptureState::Starting {
                        s.capture.state = CaptureState::Idle;
                    }
                });
                self.notify(NoticeKind::Error, message);
            }
            WorkerEvent::Log { .. } => {}
        }
    }

    fn current_world(&self) -> Option<WorldInfo> {
        let s = self.snapshot.lock().unwrap();
        let w = &s.vrchat.world;
        (w.in_world && (w.world_id.is_some() || w.world_name.is_some())).then(|| WorldInfo {
            id: w.world_id.clone(),
            name: w.world_name.clone(),
            author_name: None,
            instance: w.instance.clone(),
        })
    }

    fn start_with(self: &Shared, source: SourceSpec, importing: bool, from_osc: bool) -> anyhow::Result<()> {
        let settings = self.settings();
        let spout = matches!(source, SourceSpec::Spout { .. });
        if !importing && !settings.consent_acknowledged {
            anyhow::bail!("Please read and accept the capture notice first (Capture page).");
        }
        let cmd = StartCapture {
            source: source.clone(),
            sessions_dir: settings.sessions_dir.clone(),
            mode: settings.capture_mode,
            quality: settings.storage_quality,
            keyframes: settings.keyframes.clone(),
            world: if importing { None } else { self.current_world() },
            consent: ConsentInfo::default(),
            folder_name: None,
            performance: if importing { PerformanceSpec::default() } else { self.performance_spec(&settings) },
        };
        self.update(|s| {
            s.capture.state = if importing { CaptureState::Importing } else { CaptureState::Starting };
            s.capture.error = None;
            s.capture.stats = None;
            s.capture.trail.clear();
            s.capture.coverage = None;
            s.capture.from_osc = from_osc;
        });
        // Training takes the whole GPU: the game and the recording come first; the run starts over once they are done
        // (stopped once the state says capturing, so the run's exit does not start it again).
        if !importing {
            if let Some(dir) = self.running_reconstruction() {
                self.stop_reconstruction();
                self.change_queue(|q| q.push_front(dir));
                self.notify(NoticeKind::Warning, "Reconstruction paused: a capture started. It starts again when the capture ends.");
            }
        }
        *self.last_source.lock().unwrap() = (!importing).then_some(source);
        let record_parameters = !importing && settings.osc.enabled && settings.osc.record_parameters;
        // VRChat only sends values when they change: its OSCQuery tree has the current ones.
        let fetch_started = clock::now_ns();
        let drive_camera = !importing && settings.osc.enabled && settings.osc.drive_camera && spout;
        // Started from the app: the capture kit draws only while its record toggle is on (the Action Menu sets it).
        let drive_avatar = !importing && settings.osc.enabled && spout && (!from_osc || self.avatar_before.lock().unwrap().is_some());
        let vrchat = if record_parameters || drive_camera || drive_avatar { self.vrchat_values() } else { Vec::new() };
        if drive_camera {
            self.open_stream_camera(&vrchat);
        }
        if drive_avatar {
            self.switch_kit_on(&vrchat, &settings.osc.record_parameter);
        }
        let sent = {
            let mut osc = self.osc_values.lock().unwrap();
            // The previous session (a world change restarts it) gets its last values first.
            self.flush_osc(&mut osc);
            osc.forwarding = false;
            let sent = self.send(&WorkerCommand::Start(Box::new(cmd)));
            if sent.is_ok() && record_parameters {
                let samples = initial_samples(vrchat, &osc.latest, fetch_started);
                osc.forwarding = true;
                osc.pending.clear();
                if !samples.is_empty() {
                    let _ = self.send(&WorkerCommand::Osc { samples });
                }
            }
            sent
        };
        if let Err(e) = sent {
            self.put_vrchat_back();
            self.update(|s| {
                s.capture.state = CaptureState::Idle;
                s.capture.error = Some(e.to_string());
            });
            self.next_reconstruction();
            return Err(e);
        }
        Ok(())
    }

    /// The recording ended or never started: no more values to the worker, and VRChat's camera and the capture
    /// kit's toggles back as they were.
    fn put_vrchat_back(&self) {
        self.osc_values.lock().unwrap().forwarding = false;
        self.restore_camera();
        self.restore_kit();
    }

    /// VRChat publishes its camera over Spout only in Stream mode with Spout Stream on: switch both on over OSC
    /// (VRChat 2025.3.3 and later), remembering how the camera was. Spout Stream goes on a moment after the camera
    /// opens in Stream mode.
    fn open_stream_camera(self: &Shared, vrchat: &[(String, OscValue)]) {
        let value = |path: &str| vrchat.iter().find(|(p, _)| p == path).map(|(_, v)| v.clone());
        let mode = match value(CAMERA_MODE) {
            Some(OscValue::Int(m)) => m,
            _ => 0,
        };
        let streaming = value(CAMERA_STREAMING).is_some_and(|v| v.truthy());
        if mode == STREAM_MODE && streaming {
            return;
        }
        self.camera_before.lock().unwrap().get_or_insert(CameraState { mode, streaming });
        if mode != STREAM_MODE {
            let _ = self.with_osc(|o| o.send(CAMERA_MODE, vec![rosc::OscType::Int(STREAM_MODE)]));
        }
        let ctl = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            // Unless the camera was put back meanwhile (the start failed).
            if ctl.camera_before.lock().unwrap().is_some() {
                let _ = ctl.with_osc(|o| o.send(CAMERA_STREAMING, vec![rosc::OscType::Bool(true)]));
            }
        });
    }

    /// Switches the capture kit's record and depth-frames toggles on (OSC), remembering how they were.
    fn switch_kit_on(&self, vrchat: &[(String, OscValue)], record_parameter: &str) {
        let on = |name: &str| vrchat.iter().any(|(p, v)| p.strip_prefix(PARAMETERS_PREFIX) == Some(name) && v.truthy());
        let (record, depth) = (on(record_parameter), on(DEPTH_FRAMES_PARAMETER));
        self.avatar_before.lock().unwrap().get_or_insert(AvatarToggles { record, depth });
        let _ = self.with_osc(|o| {
            if !record {
                o.send(&format!("{PARAMETERS_PREFIX}{record_parameter}"), vec![rosc::OscType::Bool(true)])?;
            }
            if !depth {
                o.send(&format!("{PARAMETERS_PREFIX}{DEPTH_FRAMES_PARAMETER}"), vec![rosc::OscType::Bool(true)])?;
            }
            anyhow::Ok(())
        });
    }

    /// The capture kit's toggles back as they were (first forgotten, so their echo is the person's again).
    fn restore_kit(&self) {
        let Some(before) = self.avatar_before.lock().unwrap().take() else { return };
        let record = self.settings().osc.record_parameter;
        let _ = self.with_osc(|o| {
            o.send(&format!("{PARAMETERS_PREFIX}{record}"), vec![rosc::OscType::Bool(before.record)])?;
            o.send(&format!("{PARAMETERS_PREFIX}{DEPTH_FRAMES_PARAMETER}"), vec![rosc::OscType::Bool(before.depth)])
        });
    }

    /// The record toggle changed in VRChat: start or stop, unless it is an avatar reload's reset of a toggle the app
    /// drives (then switch it back on).
    fn on_record_toggle(self: &Shared, on: bool) {
        let driven = self.avatar_before.lock().unwrap().is_some();
        let reloading = self.avatar_changed_at.lock().unwrap().is_some_and(|t| t.elapsed() < AVATAR_RELOAD_GRACE);
        if driven && !on && reloading && self.is_capturing() {
            let record = self.settings().osc.record_parameter;
            self.switch_kit_on(&[], &record);
            return;
        }
        let r = if on { self.start_capture(true) } else { self.stop_capture() };
        if let Err(err) = r {
            self.notify(NoticeKind::Error, err.to_string());
        }
    }

    /// A new avatar (or the same one reloaded, as in every new world) starts with the kit's toggles off.
    fn on_avatar_changed(self: &Shared) {
        *self.avatar_changed_at.lock().unwrap() = Some(Instant::now());
        if self.avatar_before.lock().unwrap().is_none() || !self.is_capturing() {
            return;
        }
        let ctl = self.clone();
        std::thread::spawn(move || {
            // The avatar's parameters take a moment to exist after the change message.
            std::thread::sleep(Duration::from_millis(1500));
            if ctl.avatar_before.lock().unwrap().is_some() && ctl.is_capturing() {
                let record = ctl.settings().osc.record_parameter;
                ctl.switch_kit_on(&[], &record);
            }
        });
    }

    /// Puts VRChat's camera back as it was before recording opened it.
    fn restore_camera(&self) {
        let Some(before) = self.camera_before.lock().unwrap().take() else { return };
        let _ = self.with_osc(|o| {
            o.send(CAMERA_STREAMING, vec![rosc::OscType::Bool(before.streaming)])?;
            o.send(CAMERA_MODE, vec![rosc::OscType::Int(before.mode)])
        });
    }

    fn performance_spec(&self, settings: &Settings) -> PerformanceSpec {
        let p = &settings.performance;
        // The stored calibration's clock reading may come from before a restart: date it by its age instead.
        let calibration = self.calibration.lock().unwrap().clone().map(|s| {
            let age_ns = unix_ms().saturating_sub(s.utc_ms) as i64 * 1_000_000;
            Calibration { time_ns: clock::now_ns() as i64 - age_ns, ..s.calibration }
        });
        PerformanceSpec {
            mic: p.record_mic.then(|| p.mic_device.clone()),
            game_audio_pid: if p.record_game_audio { self.snapshot.lock().unwrap().vrchat.pid } else { None },
            tracking_hz: p.record_tracking.then_some(p.tracking_rate_hz),
            calibration: p.record_tracking.then_some(calibration).flatten(),
        }
    }

    /// A T-pose calibration from the overlay: kept for the next sessions, and added to the running one.
    pub fn calibrated(self: &Shared, calibration: Calibration) {
        let stored = StoredCalibration { utc_ms: unix_ms(), calibration };
        let info = calibration_info(&stored);
        *self.calibration.lock().unwrap() = Some(stored.clone());
        self.update(|s| s.steamvr.calibration = Some(info.clone()));
        if self.is_capturing() {
            if let Err(e) = self.send(&WorkerCommand::Calibration { calibration: Box::new(stored.calibration.clone()) }) {
                eprintln!("calibration to the worker: {e:#}");
            }
        }
        self.notify(NoticeKind::Success, format!("Calibrated: eyes at {:.2} m, hands {:.2} m apart.", info.head_height_m, info.arm_span_m));
        let ctl = self.clone();
        std::thread::spawn(move || {
            let path = calibration_file();
            let r = path
                .parent()
                .map(std::fs::create_dir_all)
                .unwrap_or(Ok(()))
                .and_then(|_| std::fs::write(&path, serde_json::to_vec_pretty(&stored).unwrap_or_default()));
            if let Err(e) = r {
                ctl.notify(NoticeKind::Error, format!("Could not keep the calibration: {e}"));
            }
        });
    }

    /// The overlay process connected or left.
    pub fn set_overlay_connected(&self, connected: bool) {
        self.update_if(|s| {
            let changed = s.steamvr.overlay != connected;
            s.steamvr.overlay = connected;
            changed
        });
    }

    /// Current values from VRChat's OSCQuery service (empty when it is not running or not found). The request runs
    /// without the OSC lock: camera and kit messages, and the watchers, never wait for VRChat's answer.
    fn vrchat_values(&self) -> Vec<(String, OscValue)> {
        let result = match self.with_osc(|o| o.vrchat_query()) {
            None => return Vec::new(),
            Some(None) => Err(anyhow::anyhow!("VRChat's OSCQuery service was not found")),
            Some(Some(addr)) => orbiters_core::osc::vrchat_values(addr, Duration::from_secs(1)),
        };
        result.unwrap_or_else(|e| {
            eprintln!("current OSC values: {e:#}");
            Vec::new()
        })
    }

    /// Sends the waiting OSC samples to the worker. Callers hold the values lock, which keeps batches in order.
    fn flush_osc(self: &Shared, osc: &mut OscValues) {
        osc.last_flush = Some(Instant::now());
        if osc.pending.is_empty() {
            return;
        }
        let samples = std::mem::take(&mut osc.pending);
        if let Err(e) = self.send(&WorkerCommand::Osc { samples }) {
            eprintln!("OSC values to the worker: {e:#}");
        }
    }

    fn on_osc(self: &Shared, address: String, values: Vec<OscValue>, at_ns: u64) {
        let mut osc = self.osc_values.lock().unwrap();
        if osc.forwarding {
            let samples: Vec<OscSample> = osc_samples(&address, &values, at_ns, false).collect();
            osc.pending.extend(samples);
            if osc.last_flush.is_none_or(|t| t.elapsed() >= Duration::from_millis(50)) {
                self.flush_osc(&mut osc);
            }
        }
        osc.latest.insert(address, (values, at_ns));
    }

    pub fn start_capture(self: &Shared, from_osc: bool) -> anyhow::Result<()> {
        if self.is_capturing() {
            return Ok(());
        }
        let name = self.settings().spout_sender;
        self.start_with(SourceSpec::Spout { name }, false, from_osc)
    }

    pub fn start_test_capture(self: &Shared) -> anyhow::Result<()> {
        self.start_with(SourceSpec::TestPattern { width: 1920, height: 1080, fps: 30.0, depth_every: 4, frames: 0 }, false, false)
    }

    pub fn import_raw_capture(self: &Shared, dir: PathBuf) -> anyhow::Result<()> {
        if self.is_capturing() {
            anyhow::bail!("Finish the current capture first.");
        }
        self.start_with(SourceSpec::RawSequence { path: dir, realtime: false }, true, false)
    }

    pub fn stop_capture(self: &Shared) -> anyhow::Result<()> {
        if !self.is_capturing() {
            return Ok(());
        }
        self.update(|s| s.capture.state = CaptureState::Stopping);
        let mut osc = self.osc_values.lock().unwrap();
        self.flush_osc(&mut osc);
        osc.forwarding = false;
        self.send(&WorkerCommand::Stop)
    }

    pub fn pause_capture(self: &Shared, paused: bool) -> anyhow::Result<()> {
        self.send(if paused { &WorkerCommand::Pause } else { &WorkerCommand::Resume })?;
        self.update(|s| {
            if s.capture.state == CaptureState::Recording || s.capture.state == CaptureState::Paused {
                s.capture.state = if paused { CaptureState::Paused } else { CaptureState::Recording };
            }
        });
        Ok(())
    }

    /// Applies settings at once (the interface shows them immediately) and saves them on a background thread.
    pub fn save_settings(self: &Shared, settings: Settings) {
        let previous = self.settings();
        if previous == settings {
            return;
        }
        self.update(|s| {
            s.settings = settings.clone();
            s.storage.limit_gb = settings.storage_limit_gb;
        });
        if previous.walk_boost.strength != settings.walk_boost.strength {
            // The avatar's Walk boost radial (Orb's option) follows the slider.
            self.orb_send_parameter(orbiters_contracts::orb::params::WALK_BOOST, rosc::OscType::Float(settings.walk_boost.strength));
        }
        let ctl = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = settings.save() {
                ctl.notify(NoticeKind::Error, format!("Could not save the settings: {e}"));
            }
            if previous.osc != settings.osc {
                ctl.start_osc();
            }
            if previous.sessions_dir != settings.sessions_dir {
                refresh_storage(&ctl);
                ctl.bump_library();
            }
        });
    }

    /// Sets the walk boost's strength (0 to 1): the SteamVR tab's slider, or the avatar's Walk boost radial.
    pub fn set_walk_boost(self: &Shared, strength: f32) {
        let mut settings = self.settings();
        let strength = strength.clamp(0.0, 1.0);
        if (settings.walk_boost.strength - strength).abs() < 0.005 {
            return;
        }
        settings.walk_boost.strength = strength;
        self.save_settings(settings);
    }

    fn start_osc(self: &Shared) {
        self.start_osc_with(true);
    }

    /// Runs `f` with the OSC service when it runs (tests point it at a stand-in VRChat).
    pub fn with_osc<R>(&self, f: impl FnOnce(&OscService) -> R) -> Option<R> {
        self.osc.lock().unwrap().as_ref().map(f)
    }

    /// Starts OSC; without `advertise` nothing listens beyond the loopback interface (tests).
    pub fn start_osc_with(self: &Shared, advertise: bool) {
        *self.osc.lock().unwrap() = None;
        let settings = self.settings();
        if !settings.osc.enabled {
            self.update(|s| s.osc = state::OscStatus { enabled: false, ..Default::default() });
            return;
        }
        let ctl = self.clone();
        let parameter = settings.osc.record_parameter.clone();
        let wanted = vec![parameter.clone()];
        let result = OscService::start("Orbiters desktop", &wanted, settings.osc.vrchat_port, advertise, move |e, at_ns| {
            let (address, values) = match e {
                OscEvent::Parameter { name, value } => {
                    ctl.orb_on_parameter(&name, &value);
                    if name == parameter {
                        let on = value.truthy();
                        ctl.update(|s| s.osc.last_event = Some(format!("{name} = {}", if on { "on" } else { "off" })));
                        // Starting reads VRChat's current values over HTTP: not on the receiving thread.
                        let c = ctl.clone();
                        std::thread::spawn(move || c.on_record_toggle(on));
                    }
                    (format!("{PARAMETERS_PREFIX}{name}"), vec![value])
                }
                OscEvent::AvatarChanged { id } => {
                    ctl.on_avatar_changed();
                    ctl.orb_on_avatar(id.clone());
                    (AVATAR_CHANGE.to_string(), vec![OscValue::Text(id)])
                }
                OscEvent::Values { address, values } => (address, values),
            };
            ctl.on_osc(address, values, at_ns);
        });
        match result {
            Ok(service) => {
                let (port, adv) = (service.osc_port, service.advertising());
                *self.osc.lock().unwrap() = Some(service);
                self.update(|s| s.osc = state::OscStatus { enabled: true, listening: true, advertising: adv, port: Some(port), ..Default::default() });
            }
            Err(e) => self.update(|s| s.osc = state::OscStatus { enabled: true, error: Some(e.to_string()), ..Default::default() }),
        }
    }

    pub fn worker_log(&self) -> Vec<String> {
        self.worker_log.lock().unwrap().iter().cloned().collect()
    }

    /// Stops OSC and the workers (the capture worker finishes a running session). VRChat's camera and the kit go
    /// back first, while OSC can still tell VRChat; OSC then stops before the worker finishes, so VRChat's echo of the
    /// record toggle starts or stops nothing.
    pub fn shutdown(self: &Shared) {
        self.voice_stop();
        self.orb_stop();
        self.calls_stop();
        {
            let mut osc = self.osc_values.lock().unwrap();
            if osc.forwarding {
                self.flush_osc(&mut osc);
            }
        }
        self.put_vrchat_back();
        *self.osc.lock().unwrap() = None;
        *self.worker.lock().unwrap() = None;
        *self.reconstruction.lock().unwrap() = None;
        self.change_queue(VecDeque::clear);
    }

    /// Sends a session's splats to the Unity editor that runs Mixer (the most recently loaded one when several do).
    pub fn send_to_mixer(&self, dir: PathBuf, title: String) -> anyhow::Result<()> {
        if bundle::read_manifest(&dir).is_err() || orbiters_core::reconstruction::read_report(&dir).is_none() {
            anyhow::bail!("This session has no reconstruction to send.");
        }
        let editors = crate::mixer_inbox::endpoints(&orbiters_core::paths::mixer_inbox_dir(), crate::mixer_inbox::process_alive);
        let Some(editor) = editors.first() else {
            anyhow::bail!("Open your Unity project with Mixer to send the splats to it.");
        };
        crate::mixer_inbox::send_splats(editor, &dir, &title)?;
        let project = if editor.project_name.is_empty() { "Unity".to_string() } else { editor.project_name.clone() };
        self.notify(NoticeKind::Success, format!("Sent to Mixer in {project}: the splats appear in its open scene."));
        Ok(())
    }

    /// Trains splats from a session: now when nothing runs and nothing records, else once its turn comes.
    pub fn reconstruct(self: &Shared, dir: PathBuf) -> anyhow::Result<()> {
        if self.queue_reconstruction(dir)? {
            let after = if self.is_capturing() { "the capture" } else { "the reconstruction under way" };
            self.notify(NoticeKind::Info, format!("Waiting its turn: the reconstruction starts after {after}."));
        }
        Ok(())
    }

    /// Starts the reconstruction worker on a session (one at a time, never during a capture).
    fn start_reconstruction(self: &Shared, dir: PathBuf) -> anyhow::Result<()> {
        let root = self.settings().sessions_dir;
        if !inside(&root, &dir) || !dir.join("manifest.json").is_file() {
            anyhow::bail!("Only sessions inside the sessions folder can be reconstructed.");
        }
        if self.is_capturing() {
            anyhow::bail!("Finish the capture first: reconstruction takes the whole GPU.");
        }
        let mut guard = self.reconstruction.lock().unwrap();
        if guard.is_some() {
            anyhow::bail!("A reconstruction is already running.");
        }
        let on_event = {
            let ctl = self.clone();
            move |e| ctl.on_reconstruct_event(e)
        };
        let on_log = {
            let ctl = self.clone();
            move |line: String| {
                let mut log = ctl.worker_log.lock().unwrap();
                log.push_back(format!("reconstruct: {line}"));
                while log.len() > 400 {
                    log.pop_front();
                }
            }
        };
        let on_exit = {
            let ctl = self.clone();
            move || ctl.on_reconstruct_exit()
        };
        self.update(|s| s.reconstruction = Some(ReconstructionJob::new(dir.clone())));
        match ReconstructHandle::spawn(&dir, on_event, on_log, on_exit) {
            Ok(handle) => {
                *guard = Some(Reconstruction { handle, dir, told: false });
                Ok(())
            }
            Err(e) => {
                self.update(|s| s.reconstruction = None);
                Err(e)
            }
        }
    }

    /// The session being reconstructed.
    fn running_reconstruction(&self) -> Option<PathBuf> {
        self.reconstruction.lock().unwrap().as_ref().map(|r| r.dir.clone())
    }

    /// Reconstructs `dir` after the sessions already waiting (now when nothing runs and nothing records). Whether it
    /// has to wait.
    pub fn queue_reconstruction(self: &Shared, dir: PathBuf) -> anyhow::Result<bool> {
        let root = self.settings().sessions_dir;
        if !inside(&root, &dir) || !dir.join("manifest.json").is_file() {
            anyhow::bail!("Only sessions inside the sessions folder can be reconstructed.");
        }
        let running = self.running_reconstruction();
        self.change_queue(|q| {
            if !q.contains(&dir) && running.as_ref() != Some(&dir) {
                q.push_back(dir.clone());
            }
        });
        self.next_reconstruction();
        Ok(self.running_reconstruction().as_ref() != Some(&dir))
    }

    /// Takes a session off the waiting list (it was not started yet).
    pub fn unqueue_reconstruction(&self, dir: &Path) {
        self.change_queue(|q| q.retain(|d| d != dir));
    }

    /// Changes the waiting list and shows it.
    fn change_queue<R>(&self, f: impl FnOnce(&mut VecDeque<PathBuf>) -> R) -> R {
        let mut queue = self.reconstruct_queue.lock().unwrap();
        let r = f(&mut queue);
        let waiting: Vec<PathBuf> = queue.iter().cloned().collect();
        drop(queue);
        self.update(|s| s.reconstruct_queue = waiting);
        r
    }

    /// Starts the first waiting session when nothing runs and nothing records.
    fn next_reconstruction(self: &Shared) {
        loop {
            if self.is_capturing() || self.reconstruction.lock().unwrap().is_some() {
                return;
            }
            let Some(dir) = self.change_queue(VecDeque::pop_front) else { return };
            match self.start_reconstruction(dir.clone()) {
                Ok(()) => return,
                Err(e) => {
                    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.notify(NoticeKind::Error, format!("{name} was not reconstructed: {e}"));
                }
            }
        }
    }

    /// Stops the running reconstruction; the session keeps its previous result. Returns whether one ran.
    pub fn stop_reconstruction(&self) -> bool {
        let mut guard = self.reconstruction.lock().unwrap();
        let Some(r) = guard.as_mut() else { return false };
        r.told = true;
        r.handle.cancel();
        true
    }

    fn on_reconstruct_event(self: &Shared, e: ReconstructEvent) {
        match e {
            ReconstructEvent::Stage { stage, progress } => self.update(|s| {
                if let Some(j) = s.reconstruction.as_mut() {
                    j.stage = stage;
                    j.progress = progress;
                }
            }),
            ReconstructEvent::Progress { iteration, iterations, gaussians, psnr, seconds, .. } => self.update(|s| {
                if let Some(j) = s.reconstruction.as_mut() {
                    j.iteration = iteration;
                    j.iterations = iterations;
                    j.progress = iteration as f32 / iterations.max(1) as f32;
                    j.gaussians = gaussians;
                    j.psnr = psnr.or(j.psnr);
                    j.seconds = seconds;
                }
            }),
            ReconstructEvent::Done { report } => {
                self.mark_reconstruction_told();
                let quality = report.psnr_held_out.map(|p| format!(", {p:.1} dB on views it never saw")).unwrap_or_default();
                self.notify(
                    NoticeKind::Success,
                    format!("Reconstruction finished: {} splats{quality}.", orbiters_ui::format::integer(report.gaussians as f64)),
                );
                refresh_storage(self);
                self.bump_library();
            }
            ReconstructEvent::Error { message } => {
                self.mark_reconstruction_told();
                self.notify(NoticeKind::Error, format!("Reconstruction failed: {message}"));
            }
        }
    }

    fn mark_reconstruction_told(&self) {
        if let Some(r) = self.reconstruction.lock().unwrap().as_mut() {
            r.told = true;
        }
    }

    fn on_reconstruct_exit(self: &Shared) {
        let ended = self.reconstruction.lock().unwrap().take();
        if ended.is_some_and(|r| !r.told) {
            self.notify(NoticeKind::Error, "The reconstruction worker stopped unexpectedly.");
        }
        self.update(|s| s.reconstruction = None);
        // A cancelled run may leave its partial folder behind: the next run clears it.
        self.next_reconstruction();
    }

    /// Moves a session folder to the recycle bin (only folders inside the sessions folder with a manifest).
    pub fn trash_session(self: &Shared, dir: &Path) -> anyhow::Result<()> {
        let root = self.settings().sessions_dir;
        if !inside(&root, dir) || !dir.join("manifest.json").is_file() {
            anyhow::bail!("Only session folders inside the sessions folder can be removed.");
        }
        trash::delete(dir)?;
        refresh_storage(self);
        self.bump_library();
        Ok(())
    }
}

/// Sessions of the library, newest first (walks the folders: call off the interface thread).
pub fn list_sessions(root: &Path) -> Vec<SessionSummary> {
    bundle::list_sessions(root)
}

/// Camera positions of a session (Unity world, x y z), thinned to at most 2000 points.
pub fn session_trail(dir: &Path) -> anyhow::Result<Vec<[f32; 3]>> {
    use orbiters_contracts::frame_record::record_flags;
    let records = bundle::read_records(dir)?;
    let points: Vec<[f32; 3]> = records
        .iter()
        .filter(|r| r.has(record_flags::STRIP_OK) && !r.has(record_flags::GATED))
        .map(|r| [r.camera_to_world[3], r.camera_to_world[7], r.camera_to_world[11]])
        .collect();
    let step = points.len().div_ceil(2000).max(1);
    Ok(points.into_iter().step_by(step).collect())
}

/// Raw captures in the raw captures folder, newest first.
pub fn list_raw_captures(root: &Path) -> Vec<RawCaptureSummary> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let dir = e.path();
            let Ok(text) = std::fs::read_to_string(dir.join(orbiters_contracts::raw_capture::RAW_FILE)) else { continue };
            let Ok(meta) = serde_json::from_str::<orbiters_contracts::raw_capture::RawCapture>(&text) else { continue };
            out.push(RawCaptureSummary {
                name: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                dir,
                frames: meta.frame_count,
                width: meta.width,
                height: meta.height,
                depth_every: meta.depth_every,
                has_ground_truth: meta.has_ground_truth,
                created_utc: meta.created_utc,
            });
        }
    }
    out.sort_by(|a, b| b.created_utc.cmp(&a.created_utc));
    out
}

/// Opens a folder in the platform's file manager.
pub fn open_path(dir: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(target_os = "linux")]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    cmd.arg(dir).spawn()?;
    Ok(())
}

fn inside(root: &Path, dir: &Path) -> bool {
    match (root.canonicalize(), dir.canonicalize()) {
        (Ok(r), Ok(d)) => d.starts_with(&r) && d != r,
        _ => false,
    }
}

fn refresh_storage(ctl: &Shared) {
    let dir = ctl.settings().sessions_dir;
    let sessions = bundle::list_sessions(&dir);
    let used: u64 = sessions.iter().map(|s| s.size_bytes).sum();
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let free = disks
        .list()
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()) || dir.to_string_lossy().to_lowercase().starts_with(&d.mount_point().to_string_lossy().to_lowercase()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space());
    ctl.update(|s| {
        s.storage.sessions_dir = dir.clone();
        s.storage.used_bytes = used;
        s.storage.free_bytes = free;
        s.storage.limit_gb = s.settings.storage_limit_gb;
        s.storage.sessions = sessions.len();
    });
}

fn spawn_watchers(ctl: Shared) {
    std::thread::Builder::new()
        .name("watchers".into())
        .spawn(move || {
            let mut log = LogWatcher::new(None);
            // The worlds visited, kept from VRChat's logs before VRChat deletes them (the Visited worlds page).
            let history_file = visited::history_file();
            let mut history = visited::History::load(&history_file);
            let mut system = sysinfo::System::new();
            let mut tick = 0u64;
            let mut running = false;
            loop {
                // VRChat process, every 3 s (first, so the world below knows whether VRChat runs).
                if tick.is_multiple_of(3) {
                    // Names only: the default refresh also reads every process's command line and environment.
                    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
                    let named = |names: &[&str]| {
                        system
                            .processes()
                            .values()
                            .find(|p| {
                                let n = p.name().to_string_lossy().to_lowercase();
                                names.contains(&n.as_str())
                            })
                            .map(|p| p.pid().as_u32())
                    };
                    let pid = named(&["vrchat.exe", "vrchat"]);
                    let steamvr = named(&["vrserver.exe", "vrserver"]).is_some();
                    running = pid.is_some();
                    ctl.update_if(|s| {
                        let c = s.vrchat.running != running || s.vrchat.pid != pid || s.steamvr.running != steamvr;
                        s.vrchat.running = running;
                        s.vrchat.pid = pid;
                        s.steamvr.running = steamvr;
                        c
                    });
                    // Microphones come and go (headsets): ask the worker again now and then.
                    if tick.is_multiple_of(15) && tick > 0 {
                        let _ = ctl.send(&WorkerCommand::ListSources);
                    }
                }

                // VRChat log: world changes (and new segments when recording). The last log keeps the last world
                // after VRChat closes; only a running VRChat is in a world.
                let events = log.poll();
                let mut world = log.state.clone();
                if !running {
                    world = WorldState { log_file: world.log_file, ..Default::default() };
                }
                let joined = events.iter().any(|e| matches!(e, LogEvent::Joining { .. }));
                let log_found = log.state.log_file.is_some() || orbiters_core::paths::vrchat_log_dir().is_some();
                ctl.update_if(|s| {
                    let changed = s.vrchat.world != world || s.vrchat.log_found != log_found;
                    s.vrchat.world = world;
                    s.vrchat.log_found = log_found;
                    changed
                });
                if joined {
                    let (recording, split) = {
                        let s = ctl.snapshot.lock().unwrap();
                        (s.capture.state == CaptureState::Recording || s.capture.state == CaptureState::Paused, s.settings.split_on_world_change)
                    };
                    if recording {
                        if split {
                            let source = ctl.last_source.lock().unwrap().clone();
                            if let Some(source) = source {
                                let from_osc = ctl.snapshot.lock().unwrap().capture.from_osc;
                                let _ = ctl.start_with(source, false, from_osc);
                                ctl.notify(NoticeKind::Info, "New world: started a new session.");
                            }
                        } else {
                            let _ = ctl.send(&WorkerCommand::SetWorld { world: ctl.current_world() });
                        }
                    }
                }

                // Spout senders, every 2 s.
                #[cfg(windows)]
                if tick.is_multiple_of(2) {
                    let sources: Vec<_> = orbiters_spout::list_senders()
                        .into_iter()
                        .filter_map(|n| orbiters_spout::sender_info(&n))
                        .map(|i| orbiters_core::protocol::SpoutSourceInfo {
                            name: i.name,
                            width: i.width,
                            height: i.height,
                            format: i.format,
                            description: i.description,
                        })
                        .collect();
                    ctl.update_if(|s| {
                        let c = s.sources != sources;
                        s.sources = sources;
                        c
                    });
                }

                // OSC: the last values of a quiet second reach the worker, and the status follows VRChat.
                {
                    let mut osc = ctl.osc_values.lock().unwrap();
                    if osc.forwarding {
                        ctl.flush_osc(&mut osc);
                    }
                    let values = osc.latest.len();
                    drop(osc);
                    let found = ctl.osc.lock().unwrap().as_ref().is_some_and(|o| o.vrchat_query().is_some());
                    ctl.update_if(|s| {
                        let changed = s.osc.values != values || s.osc.vrchat_found != found;
                        s.osc.values = values;
                        s.osc.vrchat_found = found;
                        changed
                    });
                }

                // Visited worlds, every 10 s.
                if tick.is_multiple_of(10) {
                    if let Some(dir) = orbiters_core::paths::vrchat_log_dir() {
                        if history.read_logs(&dir) {
                            if let Err(e) = history.save(&history_file) {
                                eprintln!("visited worlds: {e}");
                            }
                        }
                    }
                }

                // Storage, every 30 s (it walks the session folders).
                if tick.is_multiple_of(30) {
                    refresh_storage(&ctl);
                }
                tick += 1;
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("watchers thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OSC messages a stand-in VRChat received: (address, first argument).
    fn received(socket: &std::net::UdpSocket, wait: Duration) -> Vec<(String, rosc::OscType)> {
        let mut out = Vec::new();
        let until = Instant::now() + wait;
        let mut buf = [0u8; 1024];
        while Instant::now() < until {
            if let Ok(n) = socket.recv(&mut buf) {
                if let Ok((_, rosc::OscPacket::Message(m))) = rosc::decoder::decode_udp(&buf[..n]) {
                    out.push((m.addr, m.args[0].clone()));
                }
            }
        }
        out
    }

    #[test]
    fn recording_opens_vrchats_stream_camera_and_puts_it_back() {
        let vrchat = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        vrchat.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = vrchat.local_addr().unwrap().port();
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        let off = vec![(CAMERA_MODE.to_string(), OscValue::Int(1)), (CAMERA_STREAMING.to_string(), OscValue::Bool(false))];
        ctl.open_stream_camera(&off);
        let sent = received(&vrchat, Duration::from_millis(900));
        assert_eq!(sent, vec![(CAMERA_MODE.into(), rosc::OscType::Int(2)), (CAMERA_STREAMING.into(), rosc::OscType::Bool(true))]);
        // A world change restarts the session: the camera already streams and its first state is kept.
        let streaming = vec![(CAMERA_MODE.to_string(), OscValue::Int(2)), (CAMERA_STREAMING.to_string(), OscValue::Bool(true))];
        ctl.open_stream_camera(&streaming);
        assert!(received(&vrchat, Duration::from_millis(600)).is_empty());
        ctl.restore_camera();
        assert_eq!(
            received(&vrchat, Duration::from_millis(300)),
            vec![(CAMERA_STREAMING.into(), rosc::OscType::Bool(false)), (CAMERA_MODE.into(), rosc::OscType::Int(1))]
        );
        // Nothing more to put back.
        ctl.restore_camera();
        assert!(received(&vrchat, Duration::from_millis(200)).is_empty());
        ctl.shutdown();
    }

    #[test]
    fn recording_from_the_app_switches_the_kit_on_and_back() {
        let vrchat = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        vrchat.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = vrchat.local_addr().unwrap().port();
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        let record = "/avatar/parameters/OrbitersRecord".to_string();
        let depth = "/avatar/parameters/OrbitersDepthFrames".to_string();
        // Depth frames already on (the person's menu choice): only the record toggle is switched.
        ctl.switch_kit_on(&[(record.clone(), OscValue::Bool(false)), (depth.clone(), OscValue::Bool(true))], "OrbitersRecord");
        assert_eq!(received(&vrchat, Duration::from_millis(200)), vec![(record.clone(), rosc::OscType::Bool(true))]);
        // Recording, then the avatar reloads (a new world): its reset to off is switched back on, not a stop.
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        *ctl.avatar_changed_at.lock().unwrap() = Some(Instant::now());
        ctl.on_record_toggle(false);
        assert!(ctl.is_capturing());
        let again = received(&vrchat, Duration::from_millis(200));
        assert!(again.contains(&(record.clone(), rosc::OscType::Bool(true))), "{again:?}");
        // The end puts both back as they were.
        ctl.restore_kit();
        assert_eq!(received(&vrchat, Duration::from_millis(200)), vec![(record, rosc::OscType::Bool(false)), (depth, rosc::OscType::Bool(true))]);
        assert!(ctl.avatar_before.lock().unwrap().is_none());
        ctl.shutdown();
    }

    /// A controller talking to a stand-in VRChat, recording with the Stream camera and the kit switched on.
    fn driving_vrchat() -> (Shared, std::net::UdpSocket) {
        let vrchat = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        vrchat.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = vrchat.local_addr().unwrap().port();
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        ctl.open_stream_camera(&[(CAMERA_MODE.to_string(), OscValue::Int(1)), (CAMERA_STREAMING.to_string(), OscValue::Bool(false))]);
        ctl.switch_kit_on(&[], "OrbitersRecord");
        ctl.osc_values.lock().unwrap().forwarding = true;
        ctl.update(|s| s.capture.state = CaptureState::Starting);
        // Mode 2, record and depth on, then Spout Stream on.
        assert_eq!(received(&vrchat, Duration::from_millis(700)).len(), 4);
        (ctl, vrchat)
    }

    /// Camera and kit as they were: Stream off, photo mode, record and depth frames off.
    fn put_back() -> Vec<(String, rosc::OscType)> {
        vec![
            (CAMERA_STREAMING.into(), rosc::OscType::Bool(false)),
            (CAMERA_MODE.into(), rosc::OscType::Int(1)),
            ("/avatar/parameters/OrbitersRecord".into(), rosc::OscType::Bool(false)),
            ("/avatar/parameters/OrbitersDepthFrames".into(), rosc::OscType::Bool(false)),
        ]
    }

    #[test]
    fn quitting_puts_vrchats_camera_and_kit_back() {
        let (ctl, vrchat) = driving_vrchat();
        ctl.shutdown();
        assert_eq!(received(&vrchat, Duration::from_millis(300)), put_back());
        assert!(ctl.with_osc(|_| ()).is_none());
    }

    #[test]
    fn a_failed_start_puts_vrchats_camera_and_kit_back() {
        let (ctl, vrchat) = driving_vrchat();
        ctl.on_worker_event(WorkerEvent::StartFailed { message: "The Spout receiver could not start.".into() });
        assert_eq!(received(&vrchat, Duration::from_millis(300)), put_back());
        assert!(!ctl.osc_values.lock().unwrap().forwarding);
        assert_eq!(ctl.view().capture.state, CaptureState::Idle);
        ctl.shutdown();

        // A start that cannot reach the worker (no worker next to the test binary) puts them back too, also the
        // Spout Stream switch that follows the camera a moment later.
        let vrchat = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        vrchat.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = vrchat.local_addr().unwrap().port();
        settings.consent_acknowledged = true;
        settings.sessions_dir = std::env::temp_dir().join(format!("orbiters-failed-start-{}", std::process::id()));
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        assert!(ctl.start_capture(false).is_err());
        let sent = received(&vrchat, Duration::from_millis(700));
        assert_eq!(sent.last(), Some(&("/avatar/parameters/OrbitersDepthFrames".into(), rosc::OscType::Bool(false))), "{sent:?}");
        assert!(!sent.contains(&(CAMERA_STREAMING.into(), rosc::OscType::Bool(true))), "{sent:?}");
        assert_eq!(ctl.view().capture.state, CaptureState::Idle);
        ctl.shutdown();
    }

    #[test]
    fn reading_vrchats_values_never_holds_the_osc_lock() {
        let ctl = Controller::new(Settings::default());
        ctl.start_osc_with(false);
        // A VRChat that takes the connection and never answers.
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        ctl.with_osc(|o| o.set_vrchat_query(Some(silent.local_addr().unwrap())));
        let c = ctl.clone();
        let started = Instant::now();
        let reader = std::thread::spawn(move || c.vrchat_values());
        std::thread::sleep(Duration::from_millis(200));
        let waited = Instant::now();
        assert!(ctl.with_osc(|_| ()).is_some());
        assert!(waited.elapsed() < Duration::from_millis(100), "{:?}", waited.elapsed());
        assert!(reader.join().unwrap().is_empty());
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        ctl.shutdown();
    }

    #[test]
    fn initial_values_prefer_fresh_ones() {
        let mut latest = HashMap::new();
        latest.insert("/avatar/parameters/Old".to_string(), (vec![OscValue::Int(1)], 10));
        latest.insert("/avatar/parameters/Fresh".to_string(), (vec![OscValue::Float(0.9)], 120));
        latest.insert("/tracking/vrsystem/head/pose".to_string(), (vec![OscValue::Float(0.0), OscValue::Float(1.6)], 50));
        let vrchat = vec![
            ("/avatar/parameters/Old".to_string(), OscValue::Int(5)),
            ("/avatar/parameters/Fresh".to_string(), OscValue::Float(0.1)),
            ("/avatar/change".to_string(), OscValue::Text("avtr_1".into())),
        ];
        let samples = initial_samples(vrchat, &latest, 100);
        let short: Vec<(&str, Option<u8>, &OscValue)> = samples.iter().map(|s| (s.address.as_str(), s.index, &s.value)).collect();
        assert_eq!(
            short,
            vec![
                ("/avatar/change", None, &OscValue::Text("avtr_1".into())),
                ("/avatar/parameters/Fresh", None, &OscValue::Float(0.9)),
                ("/avatar/parameters/Old", None, &OscValue::Int(5)),
                ("/tracking/vrsystem/head/pose", Some(0), &OscValue::Float(0.0)),
                ("/tracking/vrsystem/head/pose", Some(1), &OscValue::Float(1.6)),
            ]
        );
        assert!(samples.iter().all(|s| s.initial && s.at_ns == 0));
    }
}
