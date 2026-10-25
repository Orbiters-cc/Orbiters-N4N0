//! The app's brain, independent of the window: owns the capture worker, watches VRChat (log, process, Spout
//! senders) and the session library, listens to the Action Menu over OSC, and publishes a [`Snapshot`] that
//! the interface reads every frame. It keeps working while the window is hidden in the tray.

use crate::ipc::ToClient;
use crate::mailbox::Mailbox;
use crate::reconstruct::ReconstructHandle;
use crate::state::{self, CalibrationInfo, CaptureState, MapsMine, ReconstructionJob, Snapshot};
use crate::worker::WorkerHandle;
use orbiters_contracts::manifest::{ConsentInfo, WorldInfo};
use orbiters_contracts::performance::calibration::Calibration;
use orbiters_core::bundle::{self, SessionSummary};
use orbiters_core::clock;
use orbiters_core::osc::{Latest, Network, OscEvent, OscService, OscValue, AVATAR_CHANGE, DEPTH_FRAMES_PARAMETER, PARAMETERS_PREFIX};
use orbiters_core::protocol::{OscSample, PerformanceSpec, SourceSpec, StartCapture, WorkerCommand, WorkerEvent};
use orbiters_core::reconstruction::Event as ReconstructEvent;
use orbiters_core::settings::{Settings, WhilePlaying};
use orbiters_core::visited;
use orbiters_core::vrchat_log::{LogEvent, LogWatcher, WorldState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
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
    /// The avatar worn's parameters (those of avatars worn before go when it changes), the poses, the camera.
    latest: Latest,
    forwarding: bool,
    pending: Vec<OscSample>,
    last_flush: Option<Instant>,
}

/// Addresses kept in [`OscValues::latest`] at most: an avatar's parameters, the poses and the camera are a few
/// hundred; past this, a new address is not kept.
#[cfg(test)]
const MAX_LATEST: usize = Latest::MOST;

/// One sample per argument (`index` only for messages with several; the receiver passes at most 16).
fn osc_samples<'a>(address: &'a str, values: &'a [OscValue], at_ns: u64, initial: bool) -> impl Iterator<Item = OscSample> + 'a {
    let several = values.len() > 1;
    values.iter().enumerate().filter_map(move |(i, value)| {
        Some(OscSample {
            at_ns,
            address: address.to_string(),
            index: if several { Some(u8::try_from(i).ok()?) } else { None },
            value: value.clone(),
            initial,
        })
    })
}

/// Face tracking's parameters (VRCFaceTracking's `v2/…`, and `FT/…`): hundreds a second that only the recording
/// reads, never a button of N4N0's.
fn face_tracking(name: &str) -> bool {
    name.starts_with("v2/") || name.starts_with("FT/")
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
    /// Interfaces in other processes (window, overlay) get every change over their link, the tray too.
    subscribers: Mutex<Vec<Arc<Mailbox>>>,
    /// Held while a snapshot is taken and handed out: they go out in the order they were taken.
    publishing: Mutex<()>,
    /// The latest T-pose calibration and when it was taken (Unix ms; the shared clock restarts with Windows).
    calibration: Mutex<Option<StoredCalibration>>,
    reconstruction: Mutex<Option<Reconstruction>>,
    /// Since when the headset has been off (SteamVR's activity level, from the overlay).
    headset_off_since: Mutex<Option<Instant>>,
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
    /// N4N0, the companion on the avatar (controller/orb.rs), and its ears (controller/voice.rs).
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
    /// Updates of an installed app (controller/update.rs).
    updates: Mutex<update::UpdateRuntime>,
    /// Space being freed in the library (controller/cleanup.rs).
    cleanup: Mutex<cleanup::CleanupRuntime>,
    /// The thread that writes the settings, the newest only ([`Controller::queue_save`]).
    settings_writer: Mutex<Option<(mpsc::Sender<Settings>, std::thread::JoinHandle<()>)>>,
    /// A capture asked for while the last one was still being saved: it starts once that one is (source, importing,
    /// started from the record toggle).
    pending_start: Mutex<Option<(SourceSpec, bool, bool)>>,
    /// Since when the running capture is being stopped (a worker that never says it finished is restarted).
    stopping_since: Mutex<Option<Instant>>,
    /// The thread that does what OSC messages ask for, in their order ([`Controller::osc_action`]).
    osc_actions: Mutex<Option<mpsc::SyncSender<OscAction>>>,
    /// The record toggle's newest value not acted on yet ([`TOGGLE_NONE`], [`TOGGLE_OFF`], [`TOGGLE_ON`]): toggles that
    /// come faster than they are acted on are acted on once, for the last.
    record_toggle: AtomicU8,
    /// A switch of the kit back on after an avatar change waits (one at a time, for the newest avatar).
    kit_rearm: AtomicBool,
    /// The app quits: no capture worker starts again, nor a capture.
    quitting: AtomicBool,
}

/// Work an OSC message asks for (Talk, a hand on N4N0, the record toggle).
type OscAction = Box<dyn FnOnce(&Shared) + Send>;

/// OSC actions waiting at most: past this (a flood of messages), new ones are dropped rather than kept without end.
const OSC_ACTIONS: usize = 64;

const TOGGLE_NONE: u8 = 0;
const TOGGLE_OFF: u8 = 1;
const TOGGLE_ON: u8 = 2;

/// A capture worker that has not finished saving a session this long after Stop is stuck: it is restarted.
const STOP_LIMIT: Duration = Duration::from_secs(120);

/// The settings are written once they stopped changing for this long (a slider dragged, the avatar's radial moving).
const SAVE_QUIET: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug)]
struct AvatarToggles {
    record: bool,
    depth: bool,
}

/// After an avatar (re)load, the record toggle's reset to off is the reload's, not the person's.
const AVATAR_RELOAD_GRACE: Duration = Duration::from_secs(5);
/// A new avatar's parameters exist this long after `/avatar/change`: the kit is switched on again then.
const KIT_SETTLE: Duration = Duration::from_millis(1500);
/// VRChat sent nothing for this long: it may not know the OSC service (`OscStatus::receiving` goes off, and VRChat is
/// asked to find it again).
const OSC_QUIET: Duration = Duration::from_secs(60);

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
    /// Training on the integrated GPU (VRChat was running).
    integrated: bool,
    /// Started while VRChat ran because the headset was off ([`HEADSET_BREAK`]): putting it back on puts it back in
    /// the queue.
    on_break: bool,
}

/// How long the headset stays off before reconstructions held for VRChat start.
const HEADSET_BREAK: Duration = Duration::from_secs(60);

/// `calibration.json` in the config folder: the latest calibration, kept across restarts.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredCalibration {
    utc_ms: u64,
    calibration: Calibration,
}

fn calibration_file() -> PathBuf {
    orbiters_core::paths::config_dir().join("calibration.json")
}

/// The calibration kept; one that cannot be read is kept beside (`calibration.json.bad-<time>`) before the next
/// calibration replaces it. Tests read none (the person's config folder stays out of them).
fn read_calibration() -> Option<StoredCalibration> {
    if cfg!(test) {
        return None;
    }
    let path = calibration_file();
    let text = match orbiters_core::read_text(&path) {
        Ok(text) => text?,
        Err(e) => {
            eprintln!("calibration: {}: {e}", path.display());
            return None;
        }
    };
    match serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)) {
        Ok(stored) => Some(stored),
        Err(e) => {
            eprintln!("calibration: {}: {e}; kept as {:?}", path.display(), orbiters_core::keep_aside(&path));
            None
        }
    }
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
mod cleanup;
mod maps;
mod orb;
mod performance;
mod scan;
pub(crate) mod screen;
mod states;
pub mod update;
mod voice;
pub use account::open_url;

impl Controller {
    pub fn new(settings: Settings) -> Shared {
        let mut snapshot = Snapshot::new(settings);
        let calibration = read_calibration();
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
            publishing: Mutex::new(()),
            calibration: Mutex::new(calibration),
            reconstruction: Mutex::new(None),
            headset_off_since: Mutex::new(None),
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
            updates: Mutex::new(update::UpdateRuntime::default()),
            cleanup: Mutex::new(cleanup::CleanupRuntime::default()),
            settings_writer: Mutex::new(None),
            pending_start: Mutex::new(None),
            stopping_since: Mutex::new(None),
            osc_actions: Mutex::new(None),
            record_toggle: AtomicU8::new(TOGGLE_NONE),
            kit_rearm: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
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
        self.start_updates();
    }

    /// The interface's context, repainted on every change.
    pub fn attach(&self, ctx: &egui::Context) {
        *self.repaint.lock().unwrap() = Some(ctx.clone());
    }

    /// Posts the current state to `mailbox` now and every change from then on (until it closes). Notices that came
    /// while nothing showed them go first to one that does. Registered in the same turn as a publish, so no change
    /// falls between the first state and the next.
    pub fn subscribe(&self, mailbox: Arc<Mailbox>) {
        let _turn = self.publishing.lock().unwrap();
        mailbox.post_snapshot(self.view());
        mailbox.post(ToClient::Library { version: self.library_version() });
        if mailbox.shows_notices() {
            for notice in self.take_notices() {
                mailbox.post(ToClient::Notice { notice });
            }
        }
        self.subscribers.lock().unwrap().push(mailbox);
    }

    /// Posts a message to every subscriber (forgetting those gone). Returns whether one that shows notices got it.
    fn broadcast(&self, msg: impl Fn() -> ToClient) -> bool {
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|m| m.post(msg()));
        subs.iter().any(|m| m.shows_notices())
    }

    fn push_notice(&self, notice: Notice) {
        // The tray takes no notice: one that only it got waits for the next window.
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

    /// Hands the current state out: one copy, shared by every subscriber (each link writes the newest it has when it
    /// can). Taken and handed out in one turn, so a slower publisher never puts back an older state.
    fn publish(&self) {
        let _turn = self.publishing.lock().unwrap();
        let s = {
            let mut s = self.snapshot.lock().unwrap();
            // Copied without its trail (up to 6000 points), which the interface gets thinned to 1500 at most.
            let trail = std::mem::take(&mut s.capture.trail);
            let mut copy = s.clone();
            let step = trail.len().div_ceil(1500).max(1);
            copy.capture.trail = trail.iter().step_by(step).copied().collect();
            s.capture.trail = trail;
            copy
        };
        let s = Arc::new(s);
        *self.published.lock().unwrap() = s.clone();
        self.subscribers.lock().unwrap().retain(|m| m.post_snapshot(s.clone()));
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

    /// Looks at the settings without copying them (the OSC receiving thread does, for every value VRChat sends).
    pub fn with_settings<R>(&self, f: impl FnOnce(&Settings) -> R) -> R {
        f(&self.snapshot.lock().unwrap().settings)
    }

    pub fn settings(&self) -> Settings {
        self.snapshot.lock().unwrap().settings.clone()
    }

    pub fn is_capturing(&self) -> bool {
        self.snapshot.lock().unwrap().is_capturing()
    }

    fn ensure_worker(self: &Shared) -> anyhow::Result<()> {
        // A late OSC echo or a queued start while the app quits starts no worker.
        anyhow::ensure!(!self.quitting.load(Ordering::Relaxed), "Orbiters - N4N0 is quitting.");
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
                let ended = ctl.update(|s| {
                    s.worker.ready = false;
                    // Whatever it was doing (recording, saving a session) ended with it.
                    let ended = s.capture.state != CaptureState::Idle;
                    if ended {
                        s.capture.state = CaptureState::Idle;
                        s.capture.from_osc = false;
                        s.capture.error = Some("The capture worker stopped unexpectedly.".into());
                    }
                    ended
                });
                if ended {
                    // A 3D picture under way never comes: the next recording is an ordinary one.
                    ctl.burst.lock().unwrap().recording = false;
                }
                ctl.capture_ended();
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
        self.send_if_running(cmd)
    }

    /// Sends to the worker that runs, never starting one: OSC values reach it from the OSC receiving thread, under
    /// the values' lock, which must not wait for a process to start. Gone meanwhile (the app quits): an error.
    fn send_if_running(&self, cmd: &WorkerCommand) -> anyhow::Result<()> {
        let mut guard = self.worker.lock().unwrap();
        guard.as_mut().ok_or_else(|| anyhow::anyhow!("the capture worker is not running"))?.send(cmd)
    }

    /// A 3D picture or a scan records a session of its own: none may run, nor still be being saved.
    pub(super) fn ensure_capture_idle(&self) -> anyhow::Result<()> {
        match self.snapshot.lock().unwrap().capture.state {
            CaptureState::Idle => Ok(()),
            CaptureState::Stopping => anyhow::bail!("The last recording is still being saved: try again in a moment."),
            _ => anyhow::bail!("A recording is under way: stop it first."),
        }
    }

    /// A capture ended, or never started: a start asked for while the last session was being saved runs now, else
    /// a reconstruction that waited for the GPU.
    fn capture_ended(self: &Shared) {
        *self.stopping_since.lock().unwrap() = None;
        let pending = self.pending_start.lock().unwrap().take();
        match pending {
            Some((source, importing, from_osc)) => {
                // Starting reads VRChat's current values over HTTP: not on the thread that told us.
                let ctl = self.clone();
                std::thread::spawn(move || {
                    if let Err(e) = ctl.start_with(source, importing, from_osc) {
                        ctl.notify(NoticeKind::Error, e.to_string());
                    }
                });
            }
            None => self.next_reconstruction(),
        }
    }

    /// A worker that has not finished saving a session long after it was asked to is stuck: it is restarted (its
    /// session stays as its last checkpoint left it). Called by the watchers every second.
    fn check_stopping(self: &Shared) {
        let stuck = self.snapshot.lock().unwrap().capture.state == CaptureState::Stopping
            && self.stopping_since.lock().unwrap().is_some_and(|t| t.elapsed() > STOP_LIMIT);
        if stuck {
            *self.stopping_since.lock().unwrap() = None;
            self.notify(NoticeKind::Error, "The capture worker did not finish saving the session: it was restarted. The session keeps what was saved.");
            // Dropping the handle ends the process; its end resets the state (`ensure_worker`'s exit).
            let worker = self.worker.lock().unwrap().take();
            drop(worker);
        }
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
                // Stopping: a stop came before the worker started, and its Stop follows the Start.
                if !matches!(s.capture.state, CaptureState::Importing | CaptureState::Stopping) {
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
                // A new Start came (a world split), or the worker started over by itself at a new frame size
                // ("resized": a session holds one size): the recording goes on in the next session.
                let resized = reason == "resized";
                let restarting = reason == "restarted" || resized;
                let session = self.snapshot.lock().unwrap().capture.session_dir.clone();
                if !restarting {
                    self.put_vrchat_back();
                }
                self.update(|s| {
                    if !restarting {
                        s.capture.state = CaptureState::Idle;
                        s.capture.from_osc = false;
                    }
                    // Left out of the folder sizes while it was written (`refresh_storage`): it counts from now.
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
                if resized {
                    // A 3D picture's or a scan's part at the old size is reconstructed on its own (a reconstruction
                    // takes one frame size); the part after it when the recording ends.
                    let part = self.burst.lock().unwrap().recording || self.scan.lock().unwrap().recording;
                    if let Some(dir) = session.clone().filter(|_| part) {
                        if let Err(e) = self.queue_reconstruction(dir) {
                            self.notify(NoticeKind::Warning, format!("A part of the recording, at another resolution, is not reconstructed: {e}"));
                        }
                    }
                }
                if !restarting {
                    self.burst_saved(session.clone());
                    self.scan_saved(session);
                    self.capture_ended();
                }
            }
            WorkerEvent::StartFailed { message } => {
                // Nothing records: VRChat's camera and the kit were switched on for nothing. A stop that came
                // meanwhile finds nothing to stop either.
                self.put_vrchat_back();
                self.update(|s| {
                    s.capture.error = Some(message.clone());
                    if matches!(s.capture.state, CaptureState::Starting | CaptureState::Importing | CaptureState::Stopping) {
                        s.capture.state = CaptureState::Idle;
                        s.capture.from_osc = false;
                    }
                });
                self.capture_ended();
                self.notify(NoticeKind::Error, message);
            }
            WorkerEvent::Error { message } => {
                // While recording, the same error can come for every frame (a full disk): told once until it changes.
                let repeated = self.snapshot.lock().unwrap().capture.error.as_deref() == Some(message.as_str());
                self.update(|s| {
                    s.capture.error = Some(message.clone());
                    if s.capture.state == CaptureState::Starting {
                        s.capture.state = CaptureState::Idle;
                    }
                });
                if !repeated {
                    self.notify(NoticeKind::Error, message);
                }
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
        anyhow::ensure!(!self.quitting.load(Ordering::Relaxed), "Orbiters - N4N0 is quitting.");
        let settings = self.settings();
        let spout = matches!(source, SourceSpec::Spout { .. });
        if !importing && !settings.consent_acknowledged {
            anyhow::bail!("Please read and accept the capture notice first (Capture page).");
        }
        if self.snapshot.lock().unwrap().capture.state == CaptureState::Stopping {
            // The last session is still being saved: this one starts once it is (`capture_ended`).
            *self.pending_start.lock().unwrap() = Some((source, importing, from_osc));
            return Ok(());
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
            // A stop came while VRChat's values were read: nothing starts. Checked under the lock `stop_capture`
            // takes to send its Stop, so a stop that comes later reaches the worker after this Start.
            if self.snapshot.lock().unwrap().capture.state == CaptureState::Stopping {
                None
            } else {
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
                Some(sent)
            }
        };
        let Some(sent) = sent else {
            self.put_vrchat_back();
            self.update(|s| {
                s.capture.state = CaptureState::Idle;
                s.capture.from_osc = false;
            });
            self.capture_ended();
            return Ok(());
        };
        if let Err(e) = sent {
            self.put_vrchat_back();
            self.update(|s| {
                s.capture.state = CaptureState::Idle;
                s.capture.from_osc = false;
                s.capture.error = Some(e.to_string());
            });
            self.capture_ended();
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

    /// A new avatar (or the same one reloaded, as in every new world) starts with the kit's toggles off: they are
    /// switched on again once it settled. One thread waits for that whatever the number of changes (a burst of them, or
    /// datagrams posing as VRChat's, starts no thread each), counting from the newest.
    fn on_avatar_changed(self: &Shared) {
        *self.avatar_changed_at.lock().unwrap() = Some(Instant::now());
        if self.avatar_before.lock().unwrap().is_none() || !self.is_capturing() || self.kit_rearm.swap(true, Ordering::AcqRel) {
            return;
        }
        let ctl = self.clone();
        let spawned = std::thread::Builder::new().name("kit-rearm".into()).spawn(move || {
            // The avatar's parameters take a moment to exist after the change message.
            loop {
                let since = ctl.avatar_changed_at.lock().unwrap().map_or(KIT_SETTLE, |t| t.elapsed());
                if since >= KIT_SETTLE {
                    break;
                }
                std::thread::sleep(KIT_SETTLE - since);
            }
            ctl.kit_rearm.store(false, Ordering::Release);
            if ctl.avatar_before.lock().unwrap().is_some() && ctl.is_capturing() {
                let record = ctl.with_settings(|s| s.osc.record_parameter.clone());
                ctl.switch_kit_on(&[], &record);
            }
        });
        if spawned.is_err() {
            self.kit_rearm.store(false, Ordering::Release);
        }
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
        if cfg!(test) {
            return;
        }
        let ctl = self.clone();
        std::thread::spawn(move || {
            // Written whole: a crash or a power cut leaves the last calibration or this one.
            let path = calibration_file();
            let r = path
                .parent()
                .map(std::fs::create_dir_all)
                .unwrap_or(Ok(()))
                .and_then(|_| orbiters_core::write_atomic(&path, &serde_json::to_vec_pretty(&stored).unwrap_or_default()));
            if let Err(e) = r {
                ctl.notify(NoticeKind::Error, format!("Could not keep the calibration: {e}"));
            }
        });
    }

    /// The overlay process connected or left (with it, whether the headset is worn is unknown).
    pub fn set_overlay_connected(self: &Shared, connected: bool) {
        self.update_if(|s| {
            let changed = s.steamvr.overlay != connected;
            s.steamvr.overlay = connected;
            changed
        });
        if !connected {
            self.set_headset(None);
        }
    }

    /// Someone put the headset on or took it off (`None`: unknown). Put back on, a reconstruction that started because
    /// it was off goes back to the head of the queue, to wait for VRChat to close (or the next break).
    pub fn set_headset(self: &Shared, in_use: Option<bool>) {
        self.update_if(|s| {
            let changed = s.steamvr.headset_in_use != in_use;
            s.steamvr.headset_in_use = in_use;
            changed
        });
        {
            let mut since = self.headset_off_since.lock().unwrap();
            match in_use {
                Some(false) => {
                    since.get_or_insert_with(Instant::now);
                }
                _ => *since = None,
            }
        }
        if in_use != Some(true) {
            return;
        }
        let moved = {
            let mut guard = self.reconstruction.lock().unwrap();
            match guard.as_mut() {
                Some(r) if r.on_break && !r.told && self.held_for_vrchat() => {
                    r.told = true;
                    let dir = r.dir.clone();
                    self.change_queue(|q| q.push_front(dir));
                    r.handle.cancel();
                    true
                }
                _ => false,
            }
        };
        if moved {
            self.notify(NoticeKind::Info, "The headset is back on: the reconstruction waits until VRChat closes.");
        }
    }

    /// The headset has been off long enough for reconstructions held for VRChat to start ([`HEADSET_BREAK`]).
    fn headset_on_break(&self) -> bool {
        self.headset_off_since.lock().unwrap().is_some_and(|t| t.elapsed() >= HEADSET_BREAK)
    }

    /// Current values from VRChat's OSCQuery service (empty when it is not running or not found). The request runs
    /// without the OSC lock: camera and kit messages, and the watchers, never wait for VRChat's answer.
    fn vrchat_values(&self) -> Vec<(String, OscValue)> {
        let query = self.with_osc(|o| o.vrchat_query());
        let result = match query {
            None => return Vec::new(),
            Some(None) => Err(anyhow::anyhow!("VRChat's OSCQuery service was not found")),
            Some(Some(addr)) => orbiters_core::osc::vrchat_values(addr, Duration::from_secs(1)),
        };
        let mut values = result.unwrap_or_else(|e| {
            eprintln!("current OSC values: {e:#}");
            Vec::new()
        });
        // The camera's state, which recording changes and puts back: from VRChat's tree, else from what VRChat
        // sent when it last changed.
        let camera = query.flatten().map(|addr| orbiters_core::osc::vrchat_camera_values(addr, Duration::from_millis(500))).unwrap_or_default();
        if camera.iter().any(|(p, _)| p == CAMERA_MODE) {
            values.extend(camera);
        } else {
            let osc = self.osc_values.lock().unwrap();
            for path in [CAMERA_MODE, CAMERA_STREAMING] {
                if let Some(value) = osc.latest.get(path).and_then(|(v, _)| v.first().cloned()) {
                    values.push((path.to_string(), value));
                }
            }
        }
        values
    }

    /// Sends the waiting OSC samples to the worker. Callers hold the values lock, which keeps batches in order.
    fn flush_osc(&self, osc: &mut OscValues) {
        osc.last_flush = Some(Instant::now());
        if osc.pending.is_empty() {
            return;
        }
        let samples = std::mem::take(&mut osc.pending);
        if let Err(e) = self.send_if_running(&WorkerCommand::Osc { samples }) {
            eprintln!("OSC values to the worker: {e:#}");
        }
    }

    /// Does what an OSC message asked for on the OSC action thread (started on first use), after what earlier
    /// messages asked for: a record toggle pressed twice stops what it started. Not on the receiving thread, which
    /// must keep reading (face tracking sends hundreds of values a second into a small socket buffer) while starting
    /// a capture or N4N0's ears takes a while.
    pub(super) fn osc_action(self: &Shared, action: impl FnOnce(&Shared) + Send + 'static) {
        self.try_osc_action(action);
    }

    /// [`Controller::osc_action`]; false when the action was dropped: [`OSC_ACTIONS`] wait already (a flood of
    /// messages, which must not pile up work and memory without end).
    fn try_osc_action(self: &Shared, action: impl FnOnce(&Shared) + Send + 'static) -> bool {
        let mut actions = self.osc_actions.lock().unwrap();
        if actions.is_none() {
            let (tx, rx) = mpsc::sync_channel::<OscAction>(OSC_ACTIONS);
            let ctl = Arc::downgrade(self);
            let spawned = std::thread::Builder::new().name("osc-actions".into()).spawn(move || {
                for action in rx {
                    let Some(ctl) = ctl.upgrade() else { return };
                    action(&ctl);
                }
            });
            match spawned {
                Ok(_) => *actions = Some(tx),
                Err(e) => {
                    eprintln!("OSC actions: {e}");
                    drop(actions);
                    action(self);
                    return true;
                }
            }
        }
        match actions.as_ref().map(|tx| tx.try_send(Box::new(action))) {
            Some(Ok(())) => true,
            Some(Err(mpsc::TrySendError::Full(_))) => {
                eprintln!("OSC actions: {OSC_ACTIONS} are waiting already, one more is dropped");
                false
            }
            _ => false,
        }
    }

    /// What one datagram from VRChat said, on the OSC receiving thread, which never waits for the OSC lock (see
    /// orb.rs): what it starts goes to the OSC action thread, what it says is kept, under one lock for the whole
    /// datagram.
    fn on_osc_events(self: &Shared, events: &mut Vec<OscEvent>, at_ns: u64, record_parameter: &str) {
        let mut messages = Vec::with_capacity(events.len());
        for event in events.drain(..) {
            match event {
                OscEvent::Parameter { name, value } => {
                    // Face tracking's flood skips N4N0's buttons (none is one of its parameters).
                    if !face_tracking(&name) {
                        self.orb_on_parameter(&name, &value);
                    }
                    if name == record_parameter {
                        self.record_toggled(value.truthy());
                    }
                    messages.push((format!("{PARAMETERS_PREFIX}{name}"), vec![value]));
                }
                OscEvent::AvatarChanged { id } => {
                    self.on_avatar_changed();
                    // N4N0 reads the parameters the last avatar held before they go (`on_osc`).
                    self.orb_on_avatar(id.clone());
                    messages.push((AVATAR_CHANGE.to_string(), vec![OscValue::Text(id)]));
                }
                OscEvent::Values { address, values } => messages.push((address, values)),
            }
        }
        self.on_osc(messages, at_ns);
    }

    /// Keeps what VRChat said (and passes it to a session recording it). An avatar change forgets the last avatar's
    /// parameters: a session started now lists the avatar worn, not every avatar worn before.
    fn on_osc(&self, messages: Vec<(String, Vec<OscValue>)>, at_ns: u64) {
        let mut osc = self.osc_values.lock().unwrap();
        for (address, values) in messages {
            if osc.forwarding {
                let samples: Vec<OscSample> = osc_samples(&address, &values, at_ns, false).collect();
                osc.pending.extend(samples);
            }
            osc.latest.record(address, values, at_ns);
        }
        if osc.forwarding && osc.last_flush.is_none_or(|t| t.elapsed() >= Duration::from_millis(50)) {
            self.flush_osc(&mut osc);
        }
    }

    /// The record toggle VRChat sent, on the OSC receiving thread: acted on in order on the OSC action thread, the
    /// newest value when several came meanwhile.
    fn record_toggled(self: &Shared, on: bool) {
        let before = self.record_toggle.swap(if on { TOGGLE_ON } else { TOGGLE_OFF }, Ordering::AcqRel);
        if before != TOGGLE_NONE {
            // An action waits already: it takes this value.
            return;
        }
        let queued = self.try_osc_action(|c| {
            let on = match c.record_toggle.swap(TOGGLE_NONE, Ordering::AcqRel) {
                TOGGLE_ON => true,
                TOGGLE_OFF => false,
                _ => return,
            };
            let parameter = c.with_settings(|s| s.osc.record_parameter.clone());
            c.update(|s| s.osc.last_event = Some(format!("{parameter} = {}", if on { "on" } else { "off" })));
            if !c.vrchat_agrees(&parameter, on) {
                eprintln!("{parameter} = {on} came over OSC, but VRChat's avatar does not say so: ignored");
                return;
            }
            c.on_record_toggle(on);
        });
        if !queued {
            self.record_toggle.store(TOGGLE_NONE, Ordering::Release);
        }
    }

    /// Whether VRChat's own OSCQuery tree says the avatar worn has the toggle `parameter` and that it is `on`; yes when
    /// VRChat cannot be asked. OSC comes over UDP, whose sender a host on the network can pose as (review DSK-13):
    /// such a datagram alone does not start or stop a recording. Asked twice, a moment apart, in case VRChat's tree
    /// had not caught up yet.
    fn vrchat_agrees(&self, parameter: &str, on: bool) -> bool {
        let Some(Some(addr)) = self.with_osc(|o| o.vrchat_query()) else { return true };
        let path = format!("{PARAMETERS_PREFIX}{parameter}");
        let ask = || orbiters_core::osc::vrchat_value(addr, &path, Duration::from_millis(500));
        match ask() {
            Err(_) => true,
            Ok(Some(value)) if value.truthy() == on => true,
            Ok(_) => {
                std::thread::sleep(Duration::from_millis(300));
                match ask() {
                    Err(_) => true,
                    Ok(value) => value.is_some_and(|v| v.truthy() == on),
                }
            }
        }
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
        // A start waiting for the last session to be saved is called off too.
        self.pending_start.lock().unwrap().take();
        if !self.is_capturing() {
            return Ok(());
        }
        self.update(|s| s.capture.state = CaptureState::Stopping);
        self.stopping_since.lock().unwrap().get_or_insert_with(Instant::now);
        let sent = {
            let mut osc = self.osc_values.lock().unwrap();
            self.flush_osc(&mut osc);
            osc.forwarding = false;
            self.send(&WorkerCommand::Stop)
        };
        if let Err(e) = sent {
            // No worker to say it finished: the capture is over here.
            self.put_vrchat_back();
            self.update(|s| {
                s.capture.state = CaptureState::Idle;
                s.capture.from_osc = false;
            });
            self.capture_ended();
            return Err(e);
        }
        Ok(())
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

    /// Applies settings at once (the interface shows them immediately); one thread writes them a moment later.
    pub fn save_settings(self: &Shared, settings: Settings) {
        self.apply_settings(|s| *s = settings, true);
    }

    /// Changes the settings in place, read and written under one lock: two changes at once (the avatar's radial and
    /// an assistant's pin, say) both stay, where a copy changed and saved would put back what the other changed.
    pub fn update_settings(self: &Shared, f: impl FnOnce(&mut Settings)) {
        self.apply_settings(f, true);
    }

    /// The fields a window or the overlay changed (`ToService::PatchSettings`), merged into the settings as they are
    /// now ([`Settings::patched`]): its copy may lag the service's, and saving it whole would put back what changed in
    /// between. A field the settings do not have, or a value that does not fit it, is left out.
    pub fn patch_settings(self: &Shared, patch: &serde_json::Value) {
        let mut left_out = Vec::new();
        self.update_settings(|s| {
            let (patched, rejected) = s.patched(patch);
            *s = patched;
            left_out = rejected;
        });
        if !left_out.is_empty() {
            eprintln!("settings from a window: {} left out", left_out.join(", "));
        }
    }

    /// Applies `f` to the settings, publishes them and queues them to be written. `echo`: a changed walk boost
    /// strength goes to the avatar's Walk boost radial (not when that radial set it: the contract has only the
    /// slider's changes sent, and the echo would fight a radial still moving).
    fn apply_settings(self: &Shared, f: impl FnOnce(&mut Settings), echo: bool) {
        let (previous, settings) = {
            let mut s = self.snapshot.lock().unwrap();
            let previous = s.settings.clone();
            f(&mut s.settings);
            if s.settings == previous {
                return;
            }
            s.storage.limit_gb = s.settings.storage_limit_gb;
            (previous, s.settings.clone())
        };
        self.publish();
        if echo && previous.walk_boost.strength != settings.walk_boost.strength {
            self.orb_send_parameter(orbiters_contracts::orb::params::WALK_BOOST, rosc::OscType::Float(settings.walk_boost.strength));
        }
        if previous.reconstruct_while_playing != settings.reconstruct_while_playing {
            // Reconstructions held while VRChat runs may start now.
            self.next_reconstruction();
        }
        let (osc_changed, sessions_changed) = (previous.osc != settings.osc, previous.sessions_dir != settings.sessions_dir);
        self.queue_save(settings);
        if osc_changed || sessions_changed {
            let ctl = self.clone();
            std::thread::spawn(move || {
                if osc_changed {
                    ctl.start_osc();
                }
                if sessions_changed {
                    refresh_storage(&ctl);
                    ctl.bump_library();
                }
            });
        }
    }

    /// Hands the settings to the writing thread (started on first use). One writer, so saves never race; it keeps
    /// the newest of a burst and writes it once nothing changed for [`SAVE_QUIET`].
    fn queue_save(self: &Shared, settings: Settings) {
        let mut writer = self.settings_writer.lock().unwrap();
        if writer.is_none() {
            let (tx, rx) = mpsc::channel::<Settings>();
            let ctl = Arc::downgrade(self);
            let write = move |s: Settings| {
                // Tests never touch the person's settings file.
                if cfg!(test) {
                    return;
                }
                if let Err(e) = s.save() {
                    if let Some(ctl) = ctl.upgrade() {
                        ctl.notify(NoticeKind::Error, format!("Could not save the settings: {e}"));
                    }
                }
            };
            let thread = std::thread::Builder::new().name("settings-writer".into()).spawn(move || {
                while let Ok(mut latest) = rx.recv() {
                    loop {
                        match rx.recv_timeout(SAVE_QUIET) {
                            Ok(newer) => latest = newer,
                            Err(mpsc::RecvTimeoutError::Timeout) => break,
                            Err(mpsc::RecvTimeoutError::Disconnected) => {
                                write(latest);
                                return;
                            }
                        }
                    }
                    write(latest);
                }
            });
            match thread {
                Ok(thread) => *writer = Some((tx, thread)),
                Err(e) => {
                    eprintln!("settings writer: {e}");
                    return;
                }
            }
        }
        if let Some((tx, _)) = writer.as_ref() {
            let _ = tx.send(settings);
        }
    }

    /// Writes the settings still waiting, and stops the writing thread (the app quits).
    fn flush_settings(&self) {
        let writer = self.settings_writer.lock().unwrap().take();
        if let Some((tx, thread)) = writer {
            drop(tx);
            let _ = thread.join();
        }
    }

    /// Sets the walk boost's strength (0 to 1): the SteamVR tab's slider, or (`from_avatar`) the avatar's Walk boost
    /// radial, which is not sent its own value back.
    pub fn set_walk_boost(self: &Shared, strength: f32, from_avatar: bool) {
        let strength = if strength.is_finite() { strength.clamp(0.0, 1.0) } else { return };
        self.apply_settings(
            |s| {
                if (s.walk_boost.strength - strength).abs() >= 0.005 {
                    s.walk_boost.strength = strength;
                }
            },
            !from_avatar,
        );
    }

    fn start_osc(self: &Shared) {
        self.start_osc_with(true);
    }

    /// Runs `f` with the OSC service when it runs (tests point it at a stand-in VRChat).
    pub fn with_osc<R>(&self, f: impl FnOnce(&OscService) -> R) -> Option<R> {
        self.osc.lock().unwrap().as_ref().map(f)
    }

    /// Makes VRChat find the OSC service again (the Reconnect button, or VRChat quiet for a while): without it VRChat
    /// sends no avatar parameter, so nothing on the avatar reaches N4N0 (Talk, a hand on N4N0, N4N0's screen).
    pub fn osc_rediscover(&self) {
        self.with_osc(|o| o.rediscover());
    }

    /// Starts OSC; without `advertise` nothing listens beyond the loopback interface (tests).
    pub fn start_osc_with(self: &Shared, advertise: bool) {
        // The service before goes outside the lock: stopping it waits for its receiving thread, which may be calling in.
        let before = self.osc.lock().unwrap().take();
        drop(before);
        let (enabled, parameter, vrchat_port) = self.with_settings(|s| (s.osc.enabled, s.osc.record_parameter.clone(), s.osc.vrchat_port));
        if !enabled {
            self.update(|s| s.osc = state::OscStatus { enabled: false, ..Default::default() });
            return;
        }
        let ctl = self.clone();
        let wanted = vec![parameter.clone()];
        // Starting a recording reads VRChat's current values over HTTP: the record toggle is acted on on the action
        // thread, in order (`record_toggled`).
        let result = OscService::start_batched("Orbiters - N4N0", &wanted, Network::Loopback { vrchat_port }, advertise, move |events, at_ns| {
            ctl.on_osc_events(events, at_ns, &parameter);
        });
        match result {
            Ok(service) => {
                let (port, adv) = (service.osc_port, service.advertising());
                let replaced = self.osc.lock().unwrap().replace(service);
                drop(replaced);
                self.update(|s| s.osc = state::OscStatus { enabled: true, listening: true, advertising: adv, port: Some(port), ..Default::default() });
            }
            Err(e) => self.update(|s| s.osc = state::OscStatus { enabled: true, error: Some(e.to_string()), ..Default::default() }),
        }
    }

    pub fn worker_log(&self) -> Vec<String> {
        self.worker_log.lock().unwrap().iter().cloned().collect()
    }

    /// Stops OSC and the workers. A running capture starts being saved first (its last OSC values with it), while
    /// the rest closes, and the worker gets the time saving takes. VRChat's camera and the kit go back while OSC can
    /// still tell VRChat; OSC then stops before the worker leaves, so VRChat's echo of the record toggle starts or
    /// stops nothing, and nothing queued starts once the capture or the running reconstruction ends.
    pub fn shutdown(self: &Shared) {
        self.pending_start.lock().unwrap().take();
        self.change_queue(VecDeque::clear);
        if let Err(e) = self.stop_capture() {
            eprintln!("stopping the capture: {e:#}");
        }
        self.quitting.store(true, Ordering::Relaxed);
        self.voice_stop();
        self.orb_stop();
        self.orb_close_on_quit();
        self.calls_stop();
        self.put_vrchat_back();
        // Stopped outside the lock: stopping waits for the receiving thread.
        let osc = self.osc.lock().unwrap().take();
        drop(osc);
        let worker = self.worker.lock().unwrap().take();
        if let Some(worker) = worker {
            worker.quit();
        }
        *self.reconstruction.lock().unwrap() = None;
        self.flush_settings();
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
            self.notify(NoticeKind::Info, format!("Waiting its turn: the reconstruction starts {}.", self.reconstruction_waits_for()));
        }
        Ok(())
    }

    /// What a waiting reconstruction waits for, as the end of "it starts …".
    pub(crate) fn reconstruction_waits_for(&self) -> &'static str {
        if self.is_capturing() {
            "after the capture"
        } else if self.reconstruction.lock().unwrap().is_some() {
            "after the ones before it"
        } else {
            "once VRChat closes or the headset has been off a minute"
        }
    }

    /// Reconstructions wait for VRChat to close (settings' `reconstruct_while_playing`), or for a break: the headset off
    /// for a minute.
    fn held_for_vrchat(&self) -> bool {
        self.settings().reconstruct_while_playing == WhilePlaying::AfterVrchat && self.snapshot.lock().unwrap().vrchat.running && !self.headset_on_break()
    }

    /// VRChat started or closed. Once it closes, a reconstruction training on the integrated GPU starts again on the
    /// graphics card (a minute or two there instead of an hour), and the ones held for VRChat start.
    fn vrchat_changed(self: &Shared, running: bool) {
        if running {
            return;
        }
        let moved = {
            let mut guard = self.reconstruction.lock().unwrap();
            match guard.as_mut() {
                Some(r) if r.integrated => {
                    r.told = true;
                    // Its turn again first, once the stopped worker has exited (on_reconstruct_exit).
                    let dir = r.dir.clone();
                    self.change_queue(|q| q.push_front(dir));
                    r.handle.cancel();
                    true
                }
                _ => false,
            }
        };
        if moved {
            self.notify(NoticeKind::Info, "VRChat closed: the reconstruction starts again on the graphics card.");
        }
        self.next_reconstruction();
    }

    /// Starts the reconstruction worker on a session (one at a time, never during a capture).
    fn start_reconstruction(self: &Shared, dir: PathBuf) -> anyhow::Result<()> {
        let root = self.settings().sessions_dir;
        if !inside(&root, &dir) || !dir.join("manifest.json").is_file() {
            anyhow::bail!("Only sessions inside the sessions folder can be reconstructed.");
        }
        self.ensure_frames_kept(&dir)?;
        if self.is_capturing() {
            anyhow::bail!("Finish the capture first: reconstruction takes the whole GPU.");
        }
        let integrated = self.settings().reconstruct_while_playing == WhilePlaying::IntegratedGpu && self.snapshot.lock().unwrap().vrchat.running;
        let on_break = self.settings().reconstruct_while_playing == WhilePlaying::AfterVrchat && self.snapshot.lock().unwrap().vrchat.running;
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
        self.update(|s| s.reconstruction = Some(ReconstructionJob::new(dir.clone(), integrated)));
        match ReconstructHandle::spawn(&dir, integrated, on_event, on_log, on_exit) {
            Ok(handle) => {
                *guard = Some(Reconstruction { handle, dir, told: false, integrated, on_break });
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
        self.ensure_frames_kept(&dir)?;
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

    /// Starts the first waiting session when nothing runs, nothing records and VRChat does not hold them.
    fn next_reconstruction(self: &Shared) {
        loop {
            if self.is_capturing() || self.reconstruction.lock().unwrap().is_some() || self.held_for_vrchat() {
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

    /// A session reconstructs from its frames: not one whose capture files were deleted, nor one being cleaned up.
    fn ensure_frames_kept(&self, dir: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(!self.cleaning(dir), "Its capture files are being deleted.");
        anyhow::ensure!(
            !(orbiters_core::cleanup::trimmed(dir) && orbiters_core::cleanup::has_view(dir)),
            "Only its 3D view is kept: its capture files were deleted."
        );
        Ok(())
    }

    /// Moves a session folder to the recycle bin (only folders inside the sessions folder with a manifest).
    pub fn trash_session(self: &Shared, dir: &Path) -> anyhow::Result<()> {
        let root = self.settings().sessions_dir;
        if !inside(&root, dir) || !dir.join("manifest.json").is_file() {
            anyhow::bail!("Only session folders inside the sessions folder can be removed.");
        }
        anyhow::ensure!(!self.cleaning(dir), "This session is being cleaned up: try again once that is done.");
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

/// A world change while recording starts a new session (or names the new world to the one recording) once VRChat
/// finished joining: its "Joining or Creating Room" follows the join, which one line a world's script writes into the
/// log does not make. At most one every [`SPLIT_EVERY`], whatever the log says: a world hop sooner waits for it.
#[derive(Default)]
struct WorldSplit {
    /// A join read, its room not yet.
    joining: Option<Instant>,
    /// A join completed while recording: the session splits once it may.
    due: bool,
    last: Option<Instant>,
}

/// Splits come at least this far apart.
const SPLIT_EVERY: Duration = Duration::from_secs(15);
/// A join whose room has not come after this is forgotten.
const JOIN_LIMIT: Duration = Duration::from_secs(60);

impl WorldSplit {
    /// What the log said; `live`: as it happened (not read back from before Orbiters followed the log).
    fn read(&mut self, events: &[LogEvent], live: bool, now: Instant) {
        if live {
            for e in events {
                match e {
                    LogEvent::Joining { .. } => self.joining = Some(now),
                    LogEvent::JoinedRoom { .. } if self.joining.take().is_some() => self.due = true,
                    _ => {}
                }
            }
        }
        if self.joining.is_some_and(|t| now.duration_since(t) > JOIN_LIMIT) {
            self.joining = None;
        }
    }

    /// Whether the session splits now (`recording`: one records, paused or not). A split due while nothing records is
    /// forgotten.
    fn take(&mut self, recording: bool, now: Instant) -> bool {
        if !recording {
            self.due = false;
            return false;
        }
        if !self.due || self.last.is_some_and(|t| now.duration_since(t) < SPLIT_EVERY) {
            return false;
        }
        self.due = false;
        self.last = Some(now);
        true
    }
}

fn refresh_storage(ctl: &Shared) {
    let dir = ctl.with_settings(|s| s.sessions_dir.clone());
    // Sizes are measured again only for the sessions that changed (`bundle::session_size`).
    let sessions = bundle::list_sessions(&dir);
    // The session being written counts through its own stats (the Stats handler adds them, `Stopped` its final
    // size): its folder is left out here, or it would count twice and stop recordings early.
    let current = {
        let s = ctl.snapshot.lock().unwrap();
        (s.capture.state != CaptureState::Idle).then(|| s.capture.session_dir.clone()).flatten()
    };
    let sessions: Vec<_> = sessions.into_iter().filter(|s| current.as_ref() != Some(&s.dir)).collect();
    let used: u64 = sessions.iter().map(|s| s.size_bytes).sum();
    // What keeping only their 3D views would free.
    let trimmable: Vec<u64> = sessions.iter().filter(|s| s.reconstruction.is_some() && s.capture_bytes > 0).map(|s| s.capture_bytes).collect();
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
        s.storage.capture_bytes = trimmable.iter().sum();
        s.storage.capture_sessions = trimmable.len();
    });
}

fn spawn_watchers(ctl: Shared) {
    std::thread::Builder::new()
        .name("watchers".into())
        .spawn(move || {
            let mut log = LogWatcher::new(None);
            let mut split = WorldSplit::default();
            // The worlds visited, kept from VRChat's logs before VRChat deletes them (the Visited worlds page).
            let history_file = visited::history_file();
            let mut history = visited::History::load(&history_file);
            let mut history_changed = false;
            let mut system = sysinfo::System::new();
            let mut tick = 0u64;
            let mut running = false;
            // Since when VRChat runs, and when VRChat was last asked to find the OSC service.
            let mut vrchat_since: Option<Instant> = None;
            let mut osc_asked: Option<Instant> = None;
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
                    let steamvr_pid = named(&["vrserver.exe", "vrserver"]);
                    let steamvr = steamvr_pid.is_some();
                    let was = running;
                    running = pid.is_some();
                    vrchat_since = if running { vrchat_since.or(Some(Instant::now())) } else { None };
                    let mut started = None;
                    ctl.update_if(|s| {
                        let c = s.vrchat.running != running || s.vrchat.pid != pid || s.steamvr.running != steamvr || s.steamvr.pid != steamvr_pid;
                        // VRChat started (or restarted) while a session records its sound: the worker follows it.
                        let recording = matches!(s.capture.state, CaptureState::Recording | CaptureState::Paused);
                        if pid.is_some() && s.vrchat.pid != pid && recording && s.settings.performance.record_game_audio {
                            started = pid;
                        }
                        // What the last VRChat said of the avatar's moving goes with it (the walk boost then follows the
                        // stick alone until the new one says it).
                        if s.vrchat.pid != pid {
                            s.vrchat.motion = None;
                        }
                        s.vrchat.running = running;
                        s.vrchat.pid = pid;
                        s.steamvr.running = steamvr;
                        s.steamvr.pid = steamvr_pid;
                        c
                    });
                    if let Some(pid) = started {
                        let _ = ctl.send_if_running(&WorkerCommand::GameAudio { pid });
                    }
                    if was != running {
                        ctl.vrchat_changed(running);
                    }
                    // A minute into a break from the headset, what waited for VRChat to close starts.
                    if running && ctl.headset_on_break() {
                        ctl.next_reconstruction();
                    }
                    // Microphones come and go (headsets): ask the worker again now and then.
                    if tick.is_multiple_of(15) && tick > 0 {
                        let _ = ctl.send(&WorkerCommand::ListSources);
                    }
                }

                // VRChat log: world changes (and new segments when recording). The last log keeps the last world
                // after VRChat closes; only a running VRChat is in a world. Until the log is read to its end, the world
                // it names may be one left long ago: it is neither shown nor given to a session.
                let events = log.poll();
                if let Some((name, from, to)) = log.last_read() {
                    history_changed |= history.follow(name, from, to, &events);
                }
                let log_found = log.state.log_file.is_some() || orbiters_core::paths::vrchat_log_dir().is_some();
                let world = log.caught_up().then(|| {
                    let world = log.state.clone();
                    if running {
                        world
                    } else {
                        WorldState { log_file: world.log_file, ..Default::default() }
                    }
                });
                ctl.update_if(|s| {
                    let changed = world.as_ref().is_some_and(|w| s.vrchat.world != *w) || s.vrchat.log_found != log_found;
                    if let Some(world) = world {
                        s.vrchat.world = world;
                    }
                    s.vrchat.log_found = log_found;
                    changed
                });
                split.read(&events, log.live(), Instant::now());
                // A world joined: the avatar's default state goes back once it loaded there.
                ctl.states_log(&events, log.live());
                let (state, split_session) = {
                    let s = ctl.snapshot.lock().unwrap();
                    (s.capture.state, s.settings.split_on_world_change)
                };
                let paused = state == CaptureState::Paused;
                if split.take(state == CaptureState::Recording || paused, Instant::now()) {
                    if split_session {
                        let source = ctl.last_source.lock().unwrap().clone();
                        if let Some(source) = source {
                            let from_osc = ctl.snapshot.lock().unwrap().capture.from_osc;
                            match ctl.start_with(source, false, from_osc) {
                                Ok(()) => {
                                    // A paused recording stays paused in the new world: its Pause follows the Start.
                                    if paused {
                                        let _ = ctl.pause_capture(true);
                                    }
                                    let told = if paused { "New world: a new session, paused like the last one." } else { "New world: started a new session." };
                                    ctl.notify(NoticeKind::Info, told);
                                }
                                Err(e) => ctl.notify(NoticeKind::Error, format!("New world: the new session did not start: {e}")),
                            }
                        }
                    } else {
                        let _ = ctl.send(&WorkerCommand::SetWorld { world: ctl.current_world() });
                    }
                }
                ctl.check_stopping();

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
                    let (found, received) = ctl.osc.lock().unwrap().as_ref().map_or((false, None), |o| (o.vrchat_query().is_some(), o.last_received_ns()));
                    let quiet = received.map_or(u64::MAX, |t| orbiters_core::clock::now_ns().saturating_sub(t));
                    let receiving = quiet < OSC_QUIET.as_nanos() as u64;
                    ctl.update_if(|s| {
                        let changed = s.osc.values != values || s.osc.vrchat_found != found || s.osc.receiving != receiving;
                        s.osc.values = values;
                        s.osc.vrchat_found = found;
                        s.osc.receiving = receiving;
                        changed
                    });
                    // VRChat sends avatar parameters only to the OSC services it found, and it finds one only from an
                    // answer it hears (the OSC service makes one when it starts and whenever a VRChat starts). Quiet a
                    // while with VRChat running: maybe it missed both, ask again (a VRChat that knows us ignores it).
                    if running
                        && !receiving
                        && vrchat_since.is_some_and(|t| t.elapsed() > OSC_QUIET)
                        && osc_asked.is_none_or(|t| t.elapsed() > OSC_QUIET)
                    {
                        osc_asked = Some(Instant::now());
                        ctl.osc_rediscover();
                    }
                }

                // Visited worlds, every 10 s: the older logs read here, the newest as the follower reads it.
                if tick.is_multiple_of(10) {
                    if let Some(dir) = orbiters_core::paths::vrchat_log_dir() {
                        let following = log.last_read().map(|(name, _, to)| (name, to));
                        history_changed |= history.read_logs_following(&dir, following);
                        if std::mem::take(&mut history_changed) {
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

    /// The worker starts over at a new resolution ("resized"): the recording goes on, VRChat's camera stays as it is,
    /// and a 3D picture's part at the old size waits for its own reconstruction (the next part follows at the end).
    #[test]
    fn a_new_resolution_keeps_recording_and_reconstructs_each_part() {
        let vrchat = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        vrchat.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let root = std::env::temp_dir().join(format!("orbiters-resized-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let part = root.join("Picture 1080p");
        std::fs::create_dir_all(&part).unwrap();
        std::fs::write(part.join("manifest.json"), "{}").unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = vrchat.local_addr().unwrap().port();
        settings.sessions_dir = root.clone();
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        ctl.update(|s| {
            s.capture.state = CaptureState::Recording;
            s.capture.session_dir = Some(part.clone());
        });
        ctl.burst.lock().unwrap().recording = true;
        ctl.on_worker_event(WorkerEvent::Stopped { session_dir: part.clone(), manifest: Box::default(), reason: "resized".into() });
        assert_eq!(ctl.view().capture.state, CaptureState::Recording);
        assert!(ctl.burst.lock().unwrap().recording, "the 3D picture goes on");
        assert_eq!(ctl.reconstruct_queue.lock().unwrap().iter().collect::<Vec<_>>(), vec![&part], "waits for the recording to end");
        assert!(received(&vrchat, Duration::from_millis(200)).is_empty(), "VRChat's camera and kit untouched");
        ctl.shutdown();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reading_vrchats_values_never_holds_the_osc_lock() {
        // What the test sends over OSC goes to a stand-in, never to a VRChat running on this computer.
        let stand_in = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = stand_in.local_addr().unwrap().port();
        let ctl = Controller::new(settings);
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
    fn an_avatars_parameters_go_with_it_and_addresses_kept_are_bounded() {
        let ctl = Controller::new(Settings::default());
        ctl.on_osc(
            vec![("/avatar/parameters/Old".into(), vec![OscValue::Bool(true)]), ("/tracking/vrsystem/head/pose".into(), vec![OscValue::Float(1.0); 6])],
            5,
        );
        ctl.on_osc(vec![(AVATAR_CHANGE.into(), vec![OscValue::Text("avtr_x".into())]), ("/avatar/parameters/New".into(), vec![OscValue::Int(2)])], 6);
        {
            let osc = ctl.osc_values.lock().unwrap();
            assert!(!osc.latest.contains_key("/avatar/parameters/Old"), "the last avatar's parameter went");
            for kept in ["/avatar/parameters/New", "/tracking/vrsystem/head/pose", AVATAR_CHANGE] {
                assert!(osc.latest.contains_key(kept), "{kept}");
            }
        }
        // A known address changes in place; past the bound, new ones are not kept.
        ctl.on_osc(vec![("/avatar/parameters/New".into(), vec![OscValue::Int(3)])], 7);
        assert_eq!(ctl.osc_values.lock().unwrap().latest["/avatar/parameters/New"], (vec![OscValue::Int(3)], 7));
        ctl.on_osc((0..MAX_LATEST + 10).map(|i| (format!("/avatar/parameters/P{i}"), vec![OscValue::Int(1)])).collect(), 8);
        assert_eq!(ctl.osc_values.lock().unwrap().latest.len(), MAX_LATEST);
        // A sample's index fits a byte: past 255 arguments, none is made rather than one wrapped around.
        assert_eq!(osc_samples("/x", &vec![OscValue::Int(1); 300], 0, false).count(), 256);
    }

    #[test]
    fn a_world_change_splits_once_vrchat_finished_joining() {
        let mut split = WorldSplit::default();
        let t0 = Instant::now();
        let join = LogEvent::Joining { world_id: "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b".into(), instance: "1".into(), at: None };
        let room = LogEvent::JoinedRoom { name: "B".into() };
        // A join alone (one line a script can write) splits nothing; its room completes it.
        split.read(std::slice::from_ref(&join), true, t0);
        assert!(!split.take(true, t0));
        split.read(std::slice::from_ref(&room), true, t0);
        assert!(split.take(true, t0));
        // Another one soon after waits for its time.
        split.read(&[join.clone(), room.clone()], true, t0 + Duration::from_secs(1));
        assert!(!split.take(true, t0 + Duration::from_secs(2)));
        assert!(split.take(true, t0 + SPLIT_EVERY));
        // Read back from before the log was followed, or while nothing records: nothing.
        split.read(&[join.clone(), room.clone()], false, t0 + Duration::from_secs(60));
        assert!(!split.take(true, t0 + Duration::from_secs(60)));
        split.read(&[join, room], true, t0 + Duration::from_secs(61));
        assert!(!split.take(false, t0 + Duration::from_secs(61)));
        assert!(!split.take(true, t0 + Duration::from_secs(62)), "forgotten while nothing recorded");
    }

    /// Waits until the OSC action thread did everything asked of it so far.
    fn actions_done(ctl: &Shared) {
        let (tx, rx) = mpsc::channel();
        ctl.osc_action(move |_| {
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(5)).expect("the OSC actions ran");
    }

    /// A stand-in for VRChat's OSCQuery service, whose avatar's `OrbitersRecord` is `record`.
    fn vrchat_tree(record: Arc<AtomicBool>) -> std::net::SocketAddr {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).unwrap_or(0);
                let answer = if String::from_utf8_lossy(&buf[..n]).starts_with("GET /avatar/parameters/OrbitersRecord ") {
                    let body = format!(r#"{{"FULL_PATH":"/avatar/parameters/OrbitersRecord","TYPE":"T","VALUE":[{}]}}"#, record.load(Ordering::Relaxed));
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                } else {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string()
                };
                let _ = s.write_all(answer.as_bytes());
            }
        });
        addr
    }

    #[test]
    fn a_record_toggle_vrchat_does_not_confirm_is_ignored() {
        // What the test sends over OSC goes to a stand-in, never to a VRChat running on this computer.
        let stand_in = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = stand_in.local_addr().unwrap().port();
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        let record = Arc::new(AtomicBool::new(true));
        ctl.with_osc(|o| o.set_vrchat_query(Some(vrchat_tree(record.clone()))));
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        // "Off" over OSC while VRChat's avatar says on (a datagram posing as VRChat's): still recording.
        ctl.record_toggled(false);
        actions_done(&ctl);
        assert!(ctl.is_capturing());
        assert_eq!(ctl.view().osc.last_event.as_deref(), Some("OrbitersRecord = off"));
        // VRChat says off too: the recording stops (no worker in tests: it ends at once).
        record.store(false, Ordering::Relaxed);
        ctl.record_toggled(false);
        actions_done(&ctl);
        assert!(!ctl.is_capturing());
        ctl.shutdown();
    }

    #[test]
    fn record_toggles_faster_than_acted_on_are_acted_on_once() {
        let ctl = Controller::new(Settings::default());
        let (hold, wait) = mpsc::channel::<()>();
        ctl.osc_action(move |_| {
            let _ = wait.recv();
        });
        for on in [true, false, true, false] {
            ctl.record_toggled(on);
        }
        hold.send(()).unwrap();
        actions_done(&ctl);
        assert_eq!(ctl.view().osc.last_event.as_deref(), Some("OrbitersRecord = off"));
        // Each "on" acted on would have tried to start (and told that the capture notice waits).
        assert!(ctl.take_notices().is_empty());
    }

    #[test]
    fn a_burst_of_avatar_changes_switches_the_kit_on_once() {
        let vrchat = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        vrchat.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = vrchat.local_addr().unwrap().port();
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        *ctl.avatar_before.lock().unwrap() = Some(AvatarToggles { record: false, depth: false });
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        for _ in 0..50 {
            ctl.on_avatar_changed();
        }
        let sent = received(&vrchat, KIT_SETTLE + Duration::from_millis(700));
        assert_eq!(sent.len(), 2, "record and depth switched on once: {sent:?}");
        ctl.update(|s| s.capture.state = CaptureState::Idle);
        ctl.shutdown();
    }

    #[test]
    fn a_settings_patch_keeps_what_changed_meanwhile() {
        let ctl = Controller::new(Settings::default());
        // The avatar's radial moves after the window read its copy of the settings.
        ctl.set_walk_boost(0.9, true);
        ctl.patch_settings(&serde_json::json!({ "orb": { "wake_word": true }, "walk_boost": { "enabled": true }, "osc": { "vrchat_port": "x" } }));
        let s = ctl.settings();
        assert!(s.orb.wake_word && s.walk_boost.enabled);
        assert_eq!(s.walk_boost.strength, 0.9, "the radial's change stays");
        assert_eq!(s.osc.vrchat_port, 9000, "a value that does not fit is left out");
        assert!(ctl.view().settings.orb.wake_word, "published");
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
