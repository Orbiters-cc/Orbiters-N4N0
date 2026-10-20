//! The app's brain, independent of the window: owns the capture worker, watches VRChat (log, process, Spout
//! senders) and the session library, listens to the Action Menu over OSC, and publishes a [`Snapshot`] that
//! the interface reads every frame. It keeps working while the window is hidden in the tray.

use crate::ipc::ToClient;
use crate::state::{self, CaptureState, Snapshot};
use crate::worker::WorkerHandle;
use orbiters_contracts::manifest::{ConsentInfo, WorldInfo};
use orbiters_core::bundle::{self, SessionSummary};
use orbiters_core::clock;
use orbiters_core::osc::{OscEvent, OscService, OscValue, AVATAR_CHANGE, PARAMETERS_PREFIX};
use orbiters_core::protocol::{OscSample, PerformanceSpec, SourceSpec, StartCapture, WorkerCommand, WorkerEvent};
use orbiters_core::settings::Settings;
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
}

pub type Shared = Arc<Controller>;

impl Controller {
    pub fn new(settings: Settings) -> Shared {
        let snapshot = Snapshot::new(settings);
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
                ctl.osc_values.lock().unwrap().forwarding = false;
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
            }),
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
                if !restarting {
                    self.osc_values.lock().unwrap().forwarding = false;
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
            s.capture.from_osc = from_osc;
        });
        *self.last_source.lock().unwrap() = (!importing).then_some(source);
        let record_parameters = !importing && settings.osc.enabled && settings.osc.record_parameters;
        // VRChat only sends values when they change: its OSCQuery tree has the current ones.
        let fetch_started = clock::now_ns();
        let vrchat = if record_parameters { self.vrchat_values() } else { Vec::new() };
        let sent = {
            let mut osc = self.osc_values.lock().unwrap();
            // The previous session (a world change restarts it) gets its last values first.
            self.flush_osc(&mut osc);
            osc.forwarding = false;
            let sent = self.send(&WorkerCommand::Start(cmd));
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
            self.update(|s| {
                s.capture.state = CaptureState::Idle;
                s.capture.error = Some(e.to_string());
            });
            return Err(e);
        }
        Ok(())
    }

    fn performance_spec(&self, settings: &Settings) -> PerformanceSpec {
        let p = &settings.performance;
        PerformanceSpec {
            mic: p.record_mic.then(|| p.mic_device.clone()),
            game_audio_pid: if p.record_game_audio { self.snapshot.lock().unwrap().vrchat.pid } else { None },
        }
    }

    /// Current values from VRChat's OSCQuery service (empty when it is not running or not found).
    fn vrchat_values(&self) -> Vec<(String, OscValue)> {
        match self.osc.lock().unwrap().as_ref().map(|o| o.vrchat_values(Duration::from_millis(500))) {
            Some(Ok(values)) => values,
            Some(Err(e)) => {
                eprintln!("current OSC values: {e:#}");
                Vec::new()
            }
            None => Vec::new(),
        }
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
                    if name == parameter {
                        let on = value.truthy();
                        ctl.update(|s| s.osc.last_event = Some(format!("{name} = {}", if on { "on" } else { "off" })));
                        // Starting reads VRChat's current values over HTTP: not on the receiving thread.
                        let c = ctl.clone();
                        std::thread::spawn(move || {
                            let r = if on { c.start_capture(true) } else { c.stop_capture() };
                            if let Err(err) = r {
                                c.notify(NoticeKind::Error, err.to_string());
                            }
                        });
                    }
                    (format!("{PARAMETERS_PREFIX}{name}"), vec![value])
                }
                OscEvent::AvatarChanged { id } => (AVATAR_CHANGE.to_string(), vec![OscValue::Text(id)]),
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

    /// Stops OSC and the worker (which finishes a running session).
    pub fn shutdown(&self) {
        *self.osc.lock().unwrap() = None;
        *self.worker.lock().unwrap() = None;
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
            let mut system = sysinfo::System::new();
            let mut tick = 0u64;
            let mut running = false;
            loop {
                // VRChat process, every 3 s (first, so the world below knows whether VRChat runs).
                if tick.is_multiple_of(3) {
                    // Names only: the default refresh also reads every process's command line and environment.
                    system.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, sysinfo::ProcessRefreshKind::nothing());
                    let pid = system
                        .processes()
                        .values()
                        .find(|p| {
                            let n = p.name().to_string_lossy().to_lowercase();
                            n == "vrchat.exe" || n == "vrchat"
                        })
                        .map(|p| p.pid().as_u32());
                    running = pid.is_some();
                    ctl.update_if(|s| {
                        let c = s.vrchat.running != running || s.vrchat.pid != pid;
                        s.vrchat.running = running;
                        s.vrchat.pid = pid;
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
