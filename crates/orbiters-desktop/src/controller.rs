//! The app's brain, independent of the window: owns the capture worker, watches VRChat (log, process, Spout
//! senders) and the session library, listens to the Action Menu over OSC, and publishes a [`Snapshot`] that
//! the interface reads every frame. It keeps working while the window is hidden in the tray.

use orbiters_core::app_log::OrLog as _;
use orbiters_core::locks::*;
use crate::ipc::ToClient;
use crate::mailbox::Mailbox;
use crate::reconstruct::ReconstructHandle;
use crate::state::{self, CalibrationInfo, CaptureState, MapsMine, ReconstructionJob, Snapshot};
use crate::worker::WorkerHandle;
use orbiters_contracts::manifest::{ConsentInfo, WorldInfo};
use orbiters_contracts::performance::calibration::Calibration;
use orbiters_core::bundle::{self, SessionSummary};
use orbiters_core::clock;
use orbiters_core::osc::{Latest, Network, OscEvent, OscService, OscValue, AVATAR_CHANGE, DEPTH_FRAMES_PARAMETER, PARAMETERS_PREFIX, RECORD_PARAMETER};
use orbiters_core::protocol::{OscSample, PerformanceSpec, SourceSpec, StartCapture, WorkerCommand, WorkerEvent};
use orbiters_core::reconstruction::Event as ReconstructEvent;
use orbiters_core::settings::{Settings, WhilePlaying};
use orbiters_core::visited;
use orbiters_i18n::{mark, tr};
use orbiters_core::vrchat_log::{LogEvent, WorldState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub use orbiters_n4n0::NoticeKind;

/// Something the interface should tell the person about.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Notice {
    Toast { kind: NoticeKind, message: String },
    /// Toasts with these messages go at once (posted for an account that left: a friend's name calling it).
    Retract { messages: Vec<String> },
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
    camera_received: Option<bool>,
    camera_closed_at: Option<u64>,
    camera_command: Option<(i32,u64)>,
    /// The camera modes the app wrote on this connection, with when it last wrote each: a Mode received within
    /// [`CAMERA_ECHO_WINDOW_NS`] of the app writing it may be VRChat's echo, never proof of a camera opened.
    camera_commanded: std::collections::HashMap<i32,u64>,
    /// The avatar worn's parameters (those of avatars worn before go when it changes), the poses, the camera.
    latest: Latest,
    forwarding: bool,
    pending: Vec<OscSample>,
    last_flush: Option<Instant>,
}

/// How long after the app wrote a camera Mode the same Mode received may still be VRChat's echo of it. VRChat answers an
/// OSC write within milliseconds; a person closes and opens the camera in seconds.
const CAMERA_ECHO_WINDOW_NS: u64 = 2_000_000_000;

/// Addresses kept in [`OscValues::latest`] at most: an avatar's parameters, the poses and the camera are a few
/// hundred; past this, a new address is not kept.
#[cfg(test)]
const MAX_LATEST: usize = Latest::MOST;

/// One sample per argument (`index` only for messages with several; the receiver passes at most 16).
fn osc_samples<'a>(address: &'a str, values: &'a [OscValue], at_ns: u64, initial: bool) -> impl Iterator<Item = OscSample> + 'a {
    let several = values.len() > 1;
    values.iter().enumerate().filter_map(move |(i, value)| {
        Some(OscSample { at_ns, address: address.to_string(), index: if several { Some(u8::try_from(i).ok()?) } else { None }, value: value.clone(), initial })
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

/// Which recording a stop is for ([`Controller::stop_capture`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StopOf {
    /// The person's own Stop: what records now, and a start waiting.
    Person,
    /// The recording of this number only ([`Controller::recording_number`] when it started).
    Recording(u64),
}

#[derive(Clone, Copy, PartialEq)]
enum CapturePurpose { CameraOverlay, Short, Avatar }

/// Lock order (read 2026-11-19, night review CTL-14 with the challenge's corrections; nothing checks it but reading): a
/// lock on the left may be held while one on its right is taken, never the reverse.
///
/// - `publishing` -> `snapshot` -> {`account_runtime` then `vrchat_login`, one after the other (in `publish`), `trail`, `burst` (inside `update` closures)}.
///   `update` lets `snapshot` go before it publishes.
/// - `capture_start_gate` -> `osc_values` -> `snapshot` (`start_segment`), and -> `worker`. The gate is held across
///   VRChat's HTTP answers while a start reads them; a scan's flight takes it around each pose it sends (C-2).
/// - `osc_values` -> `snapshot`, `osc_values` -> `worker`. Never `snapshot` -> `osc_values`: the no-keyframe save
///   did, and deadlocked a start (C-1, fixed).
/// - `camera_stream_owners` -> `osc_values` -> `camera_before` (acquire) and `camera_before` -> `osc` -> `osc_values`
///   (release): opposite orders, safe only because both run under `camera_stream_owners`.
/// - `kit_epoch` -> `publishing`, `kit_epoch` -> `osc_values`.
/// - `reconstruction` -> `reconstruct_queue` -> `publishing`; a reconstruction worker is spawned under `reconstruction`.
/// - `cleanup` -> `publishing`; `camera_scan` -> `publishing`, with VRChat's HTTP under `camera_scan`.
/// - N4N0's effects gate -> `account_runtime` (`install_account`), and -> `snapshot` (what it forgets is published). Nothing
///   takes it with `account_runtime` or `snapshot` held.
/// - `account_runtime` is let go before the account is published; its generation is read under `snapshot` through
///   `account_generation` (C-7).
pub struct Controller {
    snapshot: StartsOver<Snapshot>,
    /// What the interface reads: the snapshot as it was last handed out, swapped on every change. A copy shares every
    /// part with the snapshot (`orbiters_core::slice`): none is copied until the snapshot changes it.
    published: Arc<Mutex<Arc<Snapshot>>>,
    /// The running capture's whole camera path; the snapshot holds it thinned ([`state::Trail`]). Taken after
    /// `snapshot`, never before.
    trail: Mutex<state::Trail>,
    /// How many times the state was handed out (`--publish-cost` and tests read it).
    publishes: AtomicU64,
    worker: Mutex<Option<WorkerHandle>>,
    osc: Mutex<Option<OscService>>,
    /// VRChat on the network, where it does not run on this computer.
    lan: lan::Runtime,
    #[cfg(target_os = "macos")]
    mac_game: mac_game::Runtime,
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
    /// The waits of the service's loops.
    rests: rest::Rests,
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
    camera_stream_owners: Mutex<[bool; 2]>,
    camera_scan: Mutex<Option<Box<dyn orbiters_n4n0::QrScan>>>,
    stop_generation: AtomicU64,
    osc_generation: AtomicU64,
    /// Bumped whenever the VRChat whose camera the app shows changes (OSC started again, another VRChat followed): a
    /// camera answer asked for before is that VRChat's, and is not shown for the next one (C-8).
    camera_route: AtomicU64,
    capture_start_gate: Mutex<()>,
    /// Bumped (under `capture_start_gate`) by every start of a new recording, a world split's next part excepted: what
    /// belongs to one recording (a scan's flight) checks it is still the current one under the gate (C-2).
    capture_serial: AtomicU64,
    /// Bumped by every capture worker process started; and the one the recording under way was started on.
    worker_incarnation: AtomicU64,
    recording_worker: AtomicU64,
    /// The worker installed now (set under `worker` when it is), and the number of the recording `recording_worker`
    /// was started for (verify2-controller: an end is judged by both, in the same update that acts on it).
    worker_installed: AtomicU64,
    recording_worker_number: AtomicU64,
    kit_epoch: Mutex<(u64,Option<String>)>,
    /// The capture kit's toggles as they were before a recording started from the app switched them on (Some while
    /// the app drives them), and when the avatar last (re)loaded, which resets them.
    avatar_before: Mutex<Option<AvatarToggles>>,
    /// Recording without the wearer, and what is known about the strip.
    hide: Mutex<hide_self::HideRuntime>,
    avatar_changed_at: Mutex<Option<Instant>>,
    /// The Orbiters account's credential and a sign-in in progress (controller/account.rs).
    account_runtime: StartsOver<account::AccountRuntime>,
    /// Whether the member has what the assistant does from an account (controller/assistant_access.rs).
    assistant_access: Mutex<assistant_access::AssistantAccess>,
    /// `account_runtime`'s generation, readable under the snapshot's lock (controller/account.rs, C-7).
    account_generation: AtomicU64,
    /// The Orbiters identity's epoch, as `account_runtime` has it (controller/identity.rs): moved under its lock.
    identity_epoch: AtomicU64,
    vrchat_login: StartsOver<vrchat_login::Runtime>,
    /// Home's launcher: friends, invites, boops, Play (controller/launcher.rs).
    launcher: Mutex<launcher::Runtime>,
    /// When Play last started VRChat ("starting" until it shows up).
    launcher_started: Mutex<Option<Instant>>,
    /// The VRChat bridge's keeper (`crate::bridge`), in the tray service once it started.
    bridge: Mutex<Option<Arc<crate::bridge::keeper::Keeper>>>,
    /// The VRChats the bridge runs beside the person's (bridge/sessions.rs): the app follows none of them, unless lent.
    pub(crate) sides: Arc<crate::bridge::sessions::Sides>,
    /// The VRChat the app's OSC, camera evidence and capture were last pointed at ([`Controller::follow_target`]).
    followed: Mutex<Option<u32>>,
    /// The other VRChats whose Spout Stream a recording switched off (their OSC input), to switch on again after it.
    quieted: Mutex<Vec<std::net::SocketAddr>>,
    /// How Go there reaches the person's VRChat (controller/travel.rs): this computer, or a test's.
    pub(crate) travel: Mutex<Option<Arc<dyn travel::Travel>>>,
    /// The bridge restarts VRChat between two worlds: the moments it is closed start no reconstruction.
    bridge_holds_reconstructions: AtomicBool,
    /// What the FPS booster holds back, and all it keeps here (controller/booster.rs).
    pub(crate) booster: booster::Hub,
    /// Why a session's last reconstruction failed, for the VRChat bridge: a toast reaches no agent.
    reconstruction_failures: Mutex<std::collections::HashMap<PathBuf, String>>,
    /// What this member added to the Map explorer, and the uploads' numbering (controller/maps.rs).
    /// The list with whose it is (server and credential), so another account's is never shown as mine.
    maps_mine: Mutex<Option<(Option<String>, MapsMine)>>,
    /// Bumped by each fetch of "My maps" (and by a sign-out): only the newest answer is kept (CTL2-8).
    maps_turn: AtomicU64,
    upload_serial: AtomicU64,
    /// N4N0, the companion on the avatar (`orbiters_n4n0`; the service's side in controller/n4n0.rs).
    n4n0: orbiters_n4n0::Shared,
    /// The 3D captures N4N0's menu lists and shares (controller/menu_shares.rs).
    menu_shares: Mutex<menu_shares::MenuShares>,
    /// Explorer points: the notice in worlds with no public 3D picture, the camera's share buttons (controller/explorer.rs).
    explorer: Mutex<explorer::ExplorerState>,
    /// VRChat's frame rate per world, gathered for Orbiters (controller/performance.rs).
    performance: Mutex<performance::PerformanceRuntime>,
    /// A 3D picture under way (controller/burst.rs).
    burst: Mutex<burst::BurstRuntime>,
    /// What My World asked for (controller/world_captures.rs).
    world_captures: Mutex<world_captures::WorldCapturesRuntime>,
    /// World feedback: the N4N0 world's lines, the queue to Orbiters, the menus' worlds (controller/world_feedback.rs).
    world_feedback: Mutex<world_feedback::WorldFeedbackRuntime>,
    /// A scan under way (controller/scan.rs).
    scan: Mutex<scan::ScanRuntime>,
    /// N4N0's camera rig while recording (controller/rig.rs).
    rig: Mutex<rig::RigRuntime>,
    /// Calls with VRChat friends (controller/calls.rs).
    calls: Mutex<calls::CallsRuntime>,
    /// The avatar gun: its parameters, the event stream, shots, hits and the change (controller/avatar_gun.rs).
    avatar_gun: Mutex<avatar_gun::Runtime>,
    shared_view: Mutex<shared_view::SharedRuntime>,
    /// Updates of an installed app (controller/update.rs).
    updates: Mutex<update::UpdateRuntime>,
    /// Space being freed in the library (controller/cleanup.rs).
    cleanup: Mutex<cleanup::CleanupRuntime>,
    /// Held for the whole of a lend to a side session's VRChat, or a give-back ([`Controller::lend_vrchat`]).
    lend_gate: Mutex<()>,
    /// The instances Go and Invite may send the person to (controller/vrchat_data.rs).
    instance_permits: Mutex<vrchat_data::InstancePermits>,
    /// VRChat groups' prepared writes and what is being written (REQUEST22; never in the snapshot).
    groups: Mutex<groups::Runtime>,
    /// What was last read from VRChat, kept on this computer (controller/vrchat_kept.rs).
    vrchat_kept: vrchat_kept::Keeper,
    /// Groups requests the service is running now (admitted before their thread starts, verify-r22 Major 9).
    groups_admitted: Arc<std::sync::atomic::AtomicUsize>,
    /// The thread that writes the settings, the newest only ([`Controller::queue_save`]).
    settings_writer: Mutex<SettingsWriter>,
    /// The highest settings revision written to disk, by the writing thread or after it closed (CTL-1).
    settings_written: Arc<AtomicU64>,
    #[cfg(test)]
    test_settings_writes: Arc<Mutex<Vec<Settings>>>,
    #[cfg(test)]
    test_settings_failing: Arc<AtomicU64>,
    /// How many times the settings were changed: taken under `snapshot` with the change, so the writer can tell the
    /// newer of two copies whatever order they reach it in.
    settings_revision: AtomicU64,
    settings_changes: Mutex<SettingsChanges>,
    /// Held while the OSC service is started for the settings: a later start reads later settings.
    osc_starting: Mutex<()>,
    /// A test's look at a change about to be acted on (`false`), and acted on.
    #[cfg(test)]
    test_settings_acted: Mutex<Option<Arc<dyn Fn(&Settings, bool) + Send + Sync>>>,
    /// A capture asked for while the last one was still being saved: it starts once that one is (source, importing,
    /// started from the record toggle).
    pending_start: Mutex<Option<(SourceSpec, bool, bool, u64)>>,
    #[cfg(test)]
    test_commands: Mutex<Option<Vec<WorkerCommand>>>,
    #[cfg(test)]
    test_qr_events: Mutex<Option<orbiters_n4n0::QrEvents>>,
    /// Since when the running capture is being stopped (a worker that never says it finished is restarted).
    stopping_since: Mutex<Option<Instant>>,
    /// The thread that does what OSC messages ask for, in their order ([`Controller::osc_action`]).
    osc_actions: orbiters_n4n0::Actions,
    /// The record toggle's newest value not acted on yet ([`TOGGLE_NONE`], [`TOGGLE_OFF`], [`TOGGLE_ON`]): toggles that
    /// come faster than they are acted on are acted on once, for the last.
    record_toggle: AtomicU8,
    /// A switch of the kit back on after an avatar change waits (one at a time, for the newest avatar).
    kit_rearm: AtomicBool,
    /// The app quits: no capture worker starts again, nor a capture.
    /// What tracking smoothing's driver does now (the service's keeper says), for N4N0's Tracking page.
    tracking_note: Mutex<String>,
    quitting: AtomicBool,
    /// The last Performance report made, for Send (`perf`).
    perf_last: Mutex<Option<serde_json::Value>>,
}

const TOGGLE_NONE: u8 = 0;
const TOGGLE_OFF: u8 = 1;
const TOGGLE_ON: u8 = 2;

/// A capture worker that has not finished saving a session this long after Stop is stuck: it is restarted.
const STOP_LIMIT: Duration = Duration::from_secs(120);

/// The settings are written once they stopped changing for this long (a slider dragged, the avatar's radial moving).
const SAVE_QUIET: Duration = Duration::from_millis(250);
/// The last flush tries this many times, this far apart, before it says the settings could not be written.
const SAVE_TRIES: u32 = 3;
const SAVE_RETRY: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, Debug)]
struct AvatarToggles {
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
/// VRChat's avatar parameters that say how the player moves, forwarded to the overlay for live translation
/// ([`ToClient::Locomotion`]): the avatar's velocity in its own axes, world metres a second.
const LOCOMOTION: [&str; 3] = ["VelocityX", "VelocityY", "VelocityZ"];
const CAMERA_STREAMING: &str = "/usercamera/Streaming";
/// `/usercamera/Mode` for the Stream camera (0 off, 1 photo, 2 stream).
const STREAM_MODE: i32 = 2;

/// The running reconstruction worker, and whether its end was already told (finished, failed, cancelled or asked to
/// pause).
struct Reconstruction {
    handle: ReconstructHandle,
    dir: PathBuf,
    told: bool,
    /// Training on the integrated GPU (VRChat was running).
    integrated: bool,
    /// Asked to pause at this moment: it stays the first in line, and its job stays on show.
    pausing: Option<Instant>,
    /// It said it saw the pause (orbiters_core::reconstruction::Event::PauseSeen), at this moment.
    seen: Option<Instant>,
    /// Off the GPU since (orbiters_core::reconstruction::Event::Pausing): what is left is its file.
    freed: bool,
    /// Stopped by the person: what it could have gone on from goes with it.
    stopped: bool,
}

/// How long a worker asked to pause may stay on the GPU. A step and the read back of the set take under half a second on
/// the largest sessions: past this it is stuck, and killing it costs what happened since its last checkpoint.
const PAUSE_LIMIT: Duration = Duration::from_secs(15);
/// How long a worker that said it saw the pause (it is reading back where it stands and giving the GPU back) may take
/// to leave it: bounded, since a driver that hangs after that still holds the GPU (night2, the verifier).
const SEEN_PAUSE_LIMIT: Duration = Duration::from_secs(60);

/// Whether a worker asked to pause `asked` ago, that said it saw it `seen` ago, has stayed on the GPU too long.
fn pause_overdue(asked: Duration, seen: Option<Duration>) -> bool {
    match seen {
        Some(seen) => seen > SEEN_PAUSE_LIMIT,
        None => asked > PAUSE_LIMIT,
    }
}
/// How long quitting waits for a running reconstruction's checkpoint.
const QUIT_PAUSE_LIMIT: Duration = Duration::from_secs(10);

/// How long the headset stays off before reconstructions held for VRChat start.
const HEADSET_BREAK: Duration = Duration::from_secs(60);

/// `calibration.json` in the config folder: the latest calibration, kept across restarts.
/// A change of the settings, made and not acted on yet.
struct SettingsChange {
    previous: orbiters_core::slice::Slice<Settings>,
    settings: orbiters_core::slice::Slice<Settings>,
    echo: bool,
}

/// The changes of the settings waiting to be acted on, in the order they were made, by one thread at a time.
#[derive(Default)]
struct SettingsChanges {
    waiting: VecDeque<SettingsChange>,
    acting: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredCalibration {
    utc_ms: u64,
    calibration: Calibration,
}

fn calibration_file() -> PathBuf {
    orbiters_core::paths::config_dir().join("calibration.json")
}

/// Talks to Orbiters' development environment (the server on this computer, its account and update feed) or to
/// orbiters.cc, as the settings' Development environment switch says, whatever the Unity tools are set to.
pub fn use_environment(development: bool) {
    follow_environment(development);
}

/// [`use_environment`] for any process of the app, each time it learns the settings (orbiters_core's
/// `follow_environment`): returns whether the environment changed for this process (true the first time).
pub fn follow_environment(development: bool) -> bool {
    orbiters_update::set_development(development);
    orbiters_core::account::follow_environment(development)
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
            log_warn!("calibration: {}: {e}", path.display(), e = e);
            return None;
        }
    };
    match serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)) {
        Ok(stored) => Some(stored),
        Err(e) => {
            log_warn!("calibration: {}: {e}; kept as {:?}", path.display(), orbiters_core::keep_aside(&path), e = e);
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

/// When the service last handed out a wrist of VRChat's ([`wrist_said`]), each hand's, on the shared clock.
static WRISTS_HANDED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// VRChat's wrist pose for `hand` (it says both every frame over OSC, `/tracking/vrsystem/*wrist/pose`): kept in the state
/// always, and handed out (returns true: publish) at most 20 times a second, only while VRChat's camera is said open in
/// VR, the one time anything reads them (the camera's tools in the headset, `camera_reckon`). Handing the whole state out
/// for each, whatever showed, was 40 publishes a second with both hands, most of what the service published in VR (Enzo's
/// 0.3.5 session of 2026-11-15: 2,000 to 2,500 a minute for six hours, 20 a minute without VRChat; N-friend-lag). The
/// opening of the camera publishes the latest one with it.
fn wrist_said(s: &mut Snapshot, hand: usize, pose: [f32; 6], now: u64) -> bool {
    s.vrchat_wrists[hand] = Some(pose);
    s.vrchat_wrists_ns[hand] = now;
    let wanted = (s.camera_open_said == Some(true) && s.steamvr.running) || crate::perf::old_publishing();
    if !wanted || now.saturating_sub(WRISTS_HANDED[hand].load(Ordering::Relaxed)) < 50_000_000 {
        return false;
    }
    WRISTS_HANDED[hand].store(now, Ordering::Relaxed);
    true
}

/// How often the service publishes its state, for `logs/publishes.log`: a real session's answer to what the
/// architecture review could only estimate (DRT-1). Counts only, one line a minute at most.
#[derive(Default)]
pub(crate) struct PublishRate {
    /// Publishes in all when last looked.
    seen: u64,
    /// In the minute under way: how many, the most in one second, and the seconds so far.
    minute: u64,
    most: u64,
    seconds: u32,
}

impl PublishRate {
    /// Another second went by (`seconds` of them, at rest), the controller having published `publishes` times in all, to
    /// `readers` interfaces. Returns the minute's line once sixty have, when anything was published in them.
    pub(crate) fn second(&mut self, seconds: u32, publishes: u64, readers: usize) -> Option<String> {
        let now = publishes.saturating_sub(self.seen);
        self.seen = publishes;
        self.minute += now;
        self.most = self.most.max(now);
        self.seconds += seconds;
        if self.seconds < 60 {
            return None;
        }
        let line = (self.minute > 0).then(|| format!("{} publishes in a minute, at most {} in one second, to {readers} readers", self.minute, self.most)); // i18n: not shown (a log's line)
        (self.minute, self.most, self.seconds) = (0, 0, 0);
        line
    }
}

/// A line in `%LOCALAPPDATA%/Orbiters/logs/publishes.log`, kept under 1 MB (the last one renamed .old). Tests write none.
fn publish_log(line: &str) {
    #[cfg(not(test))]
    {
        use std::io::Write;
        let dir = orbiters_core::paths::logs_dir();
        let path = dir.join("publishes.log");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
            let _ = std::fs::rename(&path, dir.join("publishes.old.log"));
        }
        if let Ok(mut f) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(&path)) {
            let _ = writeln!(f, "{} {line}", orbiters_core::now_utc_iso());
        }
    }
    #[cfg(test)]
    let _ = line;
}

mod account;
pub mod avatar_gun;
pub mod assistant;
mod assistant_access;
mod around;
mod booster;
pub(crate) use booster::{dev_build as booster_dev_build, restore_at_sign_in as booster_restore_at_sign_in, store as booster_store};
mod bridge_game;
#[cfg(target_os = "macos")]
mod bridge_mac;
mod bridge_publish;
mod bridge_side;
mod burst;
#[cfg(all(test, windows))]
mod pause_tests;
mod calls;
mod calls_access;
mod event_stream;
mod cleanup;
mod dolly;
pub mod groups;
mod groups_ledger;
pub(crate) mod hide_self;
pub use hide_self::{upload_can_hide, uploaded_avatar_name};
mod lan;
#[cfg(target_os = "macos")]
mod mac_game;
#[cfg(target_os = "linux")]
mod linux_vrchat;
pub mod launcher;
mod obelisk_cli;
mod identity;
mod incident;
mod picture;
mod language;
mod maps;
pub mod member;
pub(crate) mod menu_shares;
mod n4n0;
mod perf;
mod previous_invite;
pub(crate) mod rest;
mod performance;
pub mod publish_cost;
pub mod picture_repair;
mod rig;
mod scan;
pub mod shares;
pub mod update;
mod videos;
pub(crate) mod vrchats;
pub mod translate;
pub mod travel;
pub mod vrchat_data;
pub mod vrchat_login;
mod vrchat_kept;
mod vrchat_scope;
pub mod camera_tools;
pub mod vr_audio;
pub mod explorer;
mod shared_view;
mod window_rest;
mod world_captures;
mod world_feedback;
pub use account::open_url;
pub(crate) use maps::{prepare as prepare_upload, Prepared};

impl Controller {
    /// `settings`: the settings, or a snapshot's own slice of them.
    pub fn new(settings: impl Into<orbiters_core::slice::Slice<Settings>>) -> Shared {
        let settings = settings.into();
        use_environment(settings.development);
        let mut snapshot = Snapshot::with(settings);
        let calibration = read_calibration();
        snapshot.steamvr.calibration = calibration.as_ref().map(calibration_info);
        let sides = Arc::<crate::bridge::sessions::Sides>::default();
        let published = Arc::new(Mutex::new(Arc::new(snapshot.clone())));
        let last_good = published.clone();
        // N4N0 lives in the controller and reaches it through a weak link: no cycle keeps either alive.
        Arc::new_cyclic(|me: &std::sync::Weak<Self>| Self {
            published,
            snapshot: StartsOver::new("the service snapshot", snapshot, move |snapshot| { // i18n: not shown (the log)
                *snapshot = (**last_good.locked()).clone();
            }),
            trail: Mutex::default(),
            publishes: AtomicU64::new(0),
            worker: Mutex::new(None),
            osc: Mutex::new(None),
            lan: Default::default(),
            #[cfg(target_os = "macos")]
            mac_game: Default::default(),
            osc_values: Mutex::new(OscValues::default()),
            worker_log: Mutex::new(VecDeque::new()),
            notices: Mutex::new(Vec::new()),
            last_source: Mutex::new(None),
            library_version: AtomicU64::new(0),
            repaint: Mutex::new(None),
            subscribers: Mutex::new(Vec::new()),
            rests: Default::default(),
            publishing: Mutex::new(()),
            calibration: Mutex::new(calibration),
            reconstruction: Mutex::new(None),
            headset_off_since: Mutex::new(None),
            reconstruct_queue: Mutex::new(VecDeque::new()),
            camera_before: Mutex::new(None),
            camera_stream_owners: Mutex::new([false; 2]),
            camera_scan: Mutex::new(None),
            stop_generation: AtomicU64::new(0),
            osc_generation: AtomicU64::new(0),
            camera_route: AtomicU64::new(0),
            capture_start_gate: Mutex::new(()),
            capture_serial: AtomicU64::new(0),
            worker_incarnation: AtomicU64::new(0),
            recording_worker: AtomicU64::new(0),
            worker_installed: AtomicU64::new(0),
            recording_worker_number: AtomicU64::new(0),
            kit_epoch: Mutex::default(),
            avatar_before: Mutex::new(None),
            hide: Mutex::new(hide_self::HideRuntime::with_journal((!cfg!(test)).then(hide_self::masks_journal))),
            avatar_changed_at: Mutex::new(None),
            account_runtime: {
                let owner = me.clone();
                StartsOver::new("the Orbiters account", account::AccountRuntime::default(), move |runtime| { // i18n: not shown (the log)
                    let generation = runtime.start_over();
                    if let Some(owner) = owner.upgrade() {
                        owner.account_generation.store(generation, Ordering::SeqCst);
                    }
                })
            },
            assistant_access: Mutex::default(),
            account_generation: AtomicU64::new(0),
            identity_epoch: AtomicU64::new(orbiters_n4n0::FIRST_EPOCH),
            vrchat_login: StartsOver::new("the VRChat sign-in", vrchat_login::Runtime::default(), vrchat_login::Runtime::start_over), // i18n: not shown (the log)
            launcher: Mutex::new(launcher::Runtime::default()),
            launcher_started: Mutex::new(None),
            bridge: Mutex::new(None),
            sides: sides.clone(),
            followed: Mutex::new(None),
            quieted: Mutex::new(Vec::new()),
            travel: Mutex::new(None),
            bridge_holds_reconstructions: AtomicBool::new(false),
            booster: Default::default(),
            reconstruction_failures: Mutex::default(),
            maps_mine: Mutex::new(None),
            maps_turn: AtomicU64::new(0),
            upload_serial: AtomicU64::new(0),
            n4n0: {
                let link = Arc::new(n4n0::Link(me.clone()));
                orbiters_n4n0::N4n0::new(link.clone(), link, crate::device::platform(), n4n0::files())
            },
            menu_shares: Mutex::new(menu_shares::MenuShares::default()),
            explorer: Mutex::new(explorer::ExplorerState::default()),
            performance: Mutex::new(performance::PerformanceRuntime::default()),
            burst: Mutex::new(burst::BurstRuntime::default()),
            world_captures: Mutex::new(world_captures::WorldCapturesRuntime::default()),
            world_feedback: Mutex::new(world_feedback::WorldFeedbackRuntime::new(vrchats::TargetLog::new(None, sides))),
            scan: Mutex::new(scan::ScanRuntime::default()),
            rig: Mutex::new(rig::RigRuntime::default()),
            // (Counted on from the moment it starts: a press that waited in a window since before is for no call of now.)
            calls: Mutex::new(calls::CallsRuntime::from(if cfg!(test) { 0 } else { orbiters_n4n0::unix_ms() })),
            avatar_gun: Mutex::new(avatar_gun::Runtime::default()),
            shared_view: Mutex::new(shared_view::SharedRuntime::default()),
            updates: Mutex::new(update::UpdateRuntime::default()),
            cleanup: Mutex::new(cleanup::CleanupRuntime::default()),
            settings_writer: Mutex::new(SettingsWriter::Idle),
            settings_written: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            test_settings_writes: Arc::default(),
            #[cfg(test)]
            test_settings_failing: Arc::default(),
            instance_permits: Mutex::default(),
            groups: Mutex::default(),
            vrchat_kept: Default::default(),
            groups_admitted: Arc::default(),
            lend_gate: Mutex::new(()),
            settings_revision: AtomicU64::new(0),
            settings_changes: Mutex::default(),
            osc_starting: Mutex::new(()),
            #[cfg(test)]
            test_settings_acted: Mutex::default(),
            pending_start: Mutex::new(None),
            #[cfg(test)]
            test_commands: Mutex::new(None),
            #[cfg(test)]
            test_qr_events: Mutex::new(None),
            stopping_since: Mutex::new(None),
            osc_actions: orbiters_n4n0::Actions::default(),
            record_toggle: AtomicU8::new(TOGGLE_NONE),
            kit_rearm: AtomicBool::new(false),
            tracking_note: Mutex::new(String::new()),
            quitting: AtomicBool::new(false),
            perf_last: Mutex::new(None),
        })
    }

    /// What tracking smoothing's driver does now, as the service's keeper found it.
    pub fn set_tracking_note(&self, note: String) {
        if std::mem::replace(&mut *self.tracking_note.locked(), note.clone()) != note {
            self.n4n0.look();
        }
    }

    pub fn tracking_note(&self) -> String {
        self.tracking_note.locked().clone()
    }

    /// Starts OSC, the worker and the watchers.
    pub fn start(self: &Shared) {
        self.start_osc();
        self.start_lan();
        {
            // VRChat's camera masks a run killed during a recording left changed (CTL2-2): once VRChat is known.
            let ctl = self.clone();
            std::thread::Builder::new().name("masks-left".into()).spawn(move || ctl.put_back_masks_left(Duration::from_secs(20))).or_log("a thread could not be started");
        }
        {
            let ctl = self.clone();
            std::thread::Builder::new().name("paused-reconstructions".into()).spawn(move || ctl.go_on_with_paused()).or_log("a thread could not be started");
        }
        // The computer's keyboard for N4N0's menu keyboard: its keys go to N4N0, off the hook's thread.
        let (tx, rx) = mpsc::channel();
        crate::keys::start(tx);
        let ctl = Arc::downgrade(self);
        std::thread::Builder::new().name("keys".into()).spawn(move || {
            while let Ok(key) = rx.recv() {
                let Some(ctl) = ctl.upgrade() else { return };
                ctl.n4n0.key(key);
            }
        }).or_log("a thread could not be started");
        let c = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = c.ensure_worker() {
                log_warn!("capture worker: {e}", e = e);
            }
        });
        self.start_loops();
    }

    /// The idle harness measures the normal loops over private data and sockets.
    pub(crate) fn start_idle_service(self: &Shared) {
        self.start_osc();
        self.start_loops();
    }

    /// The loops that follow what goes on (a test starts these alone: nothing listens, hooks or is started).
    fn start_loops(self: &Shared) {
        spawn_watchers(self.clone());
        shares::spawn_presence(self.clone());
        explorer::spawn(self.clone());
        world_captures::spawn(self.clone());
        world_feedback::spawn(self.clone());
        self.load_identity(&orbiters_core::paths::config_dir().join("identity.json"));
        self.start_account();
        crate::language::follow(&self.settings());
        self.start_language();
        self.start_vrchat_login();
        self.start_vrchat_kept();
        self.start_launcher();
        self.start_n4n0();
        self.start_perf();
        self.start_calls();
        self.start_avatar_gun();
        self.start_shared_view();
        self.start_updates();
    }

    /// The interface's context, repainted on every change.
    pub fn attach(&self, ctx: &egui::Context) {
        *self.repaint.locked() = Some(ctx.clone());
    }

    /// Posts the current state to `mailbox` now and every change from then on (until it closes). Notices that came
    /// while nothing showed them go first to one that does. Registered in the same turn as a publish, so no change
    /// falls between the first state and the next.
    pub fn subscribe(&self, mailbox: Arc<Mailbox>) {
        let _turn = self.publishing.locked();
        mailbox.post_snapshot(self.view());
        mailbox.post(ToClient::Library { version: self.library_version() });
        if mailbox.shows_notices() {
            for notice in self.take_notices() {
                mailbox.post(ToClient::Notice { notice });
            }
        }
        let shows = mailbox.shows_notices();
        self.subscribers.locked().push(mailbox);
        // What nobody looked at until now is looked at again.
        if shows {
            self.rests.wake_all();
        }
    }

    /// Every change from now on, to a mailbox that watches one thing: it has the state now, and is not woken for it.
    fn watch_from_now(&self, mailbox: &Arc<Mailbox>) {
        let _turn = self.publishing.locked();
        mailbox.post_snapshot(self.view());
        mailbox.wait(Some(Duration::ZERO));
        self.subscribers.locked().push(mailbox.clone());
    }

    /// Posts a message to every subscriber (forgetting those gone). Returns whether one that shows notices got it.
    fn broadcast(&self, msg: impl Fn() -> ToClient) -> bool {
        let mut subs = self.subscribers.locked();
        subs.retain(|m| m.post(msg()));
        subs.iter().any(|m| m.shows_notices())
    }

    fn push_notice(&self, notice: Notice) {
        // Kept for Settings, Log: a toast is gone in seconds.
        if let (Notice::Toast { kind, message }, false) = (&notice, cfg!(test)) {
            let kind = match kind {
                NoticeKind::Info => "info",
                NoticeKind::Success => "success",
                NoticeKind::Warning => "warning",
                NoticeKind::Error => "error",
            };
            orbiters_core::notice_log::append(&orbiters_core::paths::logs_dir(), kind, message, time::OffsetDateTime::now_utc().unix_timestamp());
        }
        // The tray takes no notice: one that only it got waits for the next window.
        if !self.broadcast(|| ToClient::Notice { notice: notice.clone() }) {
            let mut pending = self.notices.locked();
            pending.push(notice);
            let excess = pending.len().saturating_sub(8);
            pending.drain(..excess);
        }
        if let Some(ctx) = self.repaint.locked().as_ref() {
            orbiters_ui::anim::request_frame(ctx);
        }
    }

    pub(crate) fn bump_library(&self) {
        let version = self.library_version.fetch_add(1, Ordering::Relaxed) + 1;
        self.broadcast(|| ToClient::Library { version });
    }

    pub fn view(&self) -> Arc<Snapshot> {
        self.published.locked().clone()
    }

    pub fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut *self.notices.locked())
    }

    pub fn library_version(&self) -> u64 {
        self.library_version.load(Ordering::Relaxed)
    }

    /// Replaces the whole snapshot (screenshots and tests). Its camera path becomes the running capture's, shown thinned
    /// as a recorded one is.
    pub fn set_snapshot(&self, mut snapshot: Snapshot) {
        {
            let mut s = self.snapshot.locked();
            let trail = state::Trail::of(snapshot.capture.trail.clone());
            if trail.len() > state::Trail::SHOWN {
                snapshot.capture.trail = trail.shown();
            }
            *self.trail.locked() = trail;
            *s = snapshot;
        }
        self.publish();
    }

    /// How many times the state was handed out since the controller was made.
    pub fn publishes(&self) -> u64 {
        self.publishes.load(Ordering::Relaxed)
    }

    /// Hands the current state out: one copy, shared by every subscriber (each link writes the newest it has when it
    /// can). Taken and handed out in one turn, so a slower publisher never puts back an older state.
    ///
    /// The copy shares every part of the snapshot. A part that was written to since the last publish and holds what it
    /// held (a flag set to what it was) becomes the published part again first ([`Parts::settle`]): from here on a
    /// part is another allocation exactly when it changed.
    ///
    /// Nothing changed (a flag set to what it was, two changes another publish already took): nothing is handed out, and
    /// no window, overlay or watcher is woken (XCT-18).
    fn publish(&self) {
        use orbiters_core::slice::Parts;
        let _turn = self.publishing.locked();
        let previous = self.view();
        let s = {
            let mut s = self.snapshot.locked();
            // With the snapshot held: the account this very state is for, and no other.
            let now = self.vrchat_stamp_of(&s.account);
            vrchat_login::follow_orbiters_and_record(&mut s, &mut self.vrchat_login.locked(), now.as_ref());
            s.settle(&previous);
            if s.same(&previous) {
                return;
            }
            let published = Arc::new(s.clone());
            // Recovery needs the newest published value before another update can acquire the snapshot.
            *self.published.locked() = published.clone();
            published
        };
        self.publishes.fetch_add(1, Ordering::Relaxed);
        self.subscribers.locked().retain(|m| m.post_snapshot(s.clone()));
        if let Some(ctx) = self.repaint.locked().as_ref() {
            orbiters_ui::anim::request_frame(ctx);
        }
    }

    pub(crate) fn update<R>(&self, f: impl FnOnce(&mut Snapshot) -> R) -> R {
        let r = f(&mut self.snapshot.locked());
        self.publish();
        r
    }

    /// Applies a change and publishes only when `f` reports one.
    fn update_if(&self, f: impl FnOnce(&mut Snapshot) -> bool) -> bool {
        let changed = f(&mut self.snapshot.locked());
        if changed {
            self.publish();
        }
        changed
    }

    /// Takes back toasts posted earlier with these messages: those still waiting for a window are dropped, the windows
    /// showing them let them go now.
    pub(crate) fn retract_notices(&self, messages: Vec<String>) {
        if messages.is_empty() {
            return;
        }
        self.notices.locked().retain(|n| !matches!(n, Notice::Toast { message, .. } if messages.contains(message)));
        self.broadcast(|| ToClient::Notice { notice: Notice::Retract { messages: messages.clone() } });
        if let Some(ctx) = self.repaint.locked().as_ref() {
            orbiters_ui::anim::request_frame(ctx);
        }
    }

    pub fn notify(&self, kind: NoticeKind, message: impl Into<String>) {
        let mut message = message.into();
        // Several VRChats on this computer: the camera to open is the target's, named (in any language the key came in).
        if message == tr("Open VRChat's camera first") || message == "Open VRChat's camera first" { // i18n: not shown (the English key, as the bridge and N4N0 pass it)
            message = self.camera_first();
        }
        self.push_notice(Notice::Toast { kind, message });
    }

    /// Looks at the settings without copying them (the OSC receiving thread does, for every value VRChat sends).
    pub fn with_settings<R>(&self, f: impl FnOnce(&Settings) -> R) -> R {
        f(&self.snapshot.locked().settings)
    }

    pub fn settings(&self) -> Settings {
        (*self.snapshot.locked().settings).clone()
    }

    pub fn is_capturing(&self) -> bool {
        self.snapshot.locked().is_capturing()
    }

    /// How many Stops were asked for (tests of the service's links).
    #[cfg(test)]
    pub(crate) fn stop_generation(&self) -> u64 {
        self.stop_generation.load(Ordering::Acquire)
    }

    fn ensure_worker(self: &Shared) -> anyhow::Result<()> {
        // A late OSC echo or a queued start while the app quits starts no worker.
        anyhow::ensure!(!self.quitting.load(Ordering::Relaxed), "{}", tr("Orbiters - N4N0 is quitting."));
        let mut guard = self.worker.locked();
        if let Some(w) = guard.as_mut() {
            if w.alive() {
                return Ok(());
            }
        }
        // This worker process: its events and its end count only while the recording under way is its own, or while no
        // other worker started since (verify-controller: a dead worker's late end reset a new worker's recording).
        let incarnation = self.worker_incarnation.fetch_add(1, Ordering::AcqRel) + 1;
        // Installed from now (under `worker`): its first events (Ready) may come before the spawn returns.
        self.worker_installed.store(incarnation, Ordering::Release);
        let on_event = {
            let ctl = self.clone();
            move |e| {
                if ctl.worker_counts(incarnation) {
                    ctl.on_worker_event(e)
                }
            }
        };
        let on_log = {
            let ctl = self.clone();
            move |line: String| {
                let mut log = ctl.worker_log.locked();
                log.push_back(line);
                while log.len() > 400 {
                    log.pop_front();
                }
            }
        };
        let on_exit = {
            let ctl = self.clone();
            move || ctl.on_worker_exit(incarnation)
        };
        let handle = WorkerHandle::spawn(on_event, on_log, on_exit).inspect_err(|e| {
            self.update(|s| s.worker.error = Some(e.to_string()));
        })?;
        // A Mac's is told first whose frames it reads.
        #[cfg(target_os = "macos")]
        let handle = {
            let mut handle = handle;
            handle.send(&self.worker_host()).or_log("what a worker was asked did not reach it");
            handle
        };
        *guard = Some(handle);
        Ok(())
    }

    /// Whether what worker `incarnation` says still counts: it is the newest worker, or the recording under way was
    /// started on it.
    fn worker_counts(&self, incarnation: u64) -> bool {
        self.worker_installed.load(Ordering::Acquire) == incarnation || self.worker_runs_recording(incarnation)
    }

    /// Whether the recording under way now was started on worker `incarnation`.
    fn worker_runs_recording(&self, incarnation: u64) -> bool {
        self.recording_worker.load(Ordering::Acquire) == incarnation && self.recording_worker_number.load(Ordering::Acquire) == self.capture_serial.load(Ordering::Acquire)
    }

    /// Worker `incarnation` ended. Its recording, if one was under way, ended with it; a worker replaced since (a stuck
    /// one restarted) whose end is heard late leaves the newer worker's recording alone.
    fn on_worker_exit(self: &Shared, incarnation: u64) {
        // Its recording is the one under way only if it was started on this worker, for this number: a recording
        // being started on a replacement (already numbered, its Start not sent yet) is not its to end (verify2).
        if self.worker_runs_recording(incarnation) {
            self.put_vrchat_back();
        }
        let mut ended = false;
        self.update(|s| {
            // Judged again here, in the update that acts on it: a replacement installed meanwhile wins.
            let mine = self.worker_runs_recording(incarnation);
            if self.worker_installed.load(Ordering::Acquire) == incarnation {
                s.worker.ready = false;
            }
            if mine && s.capture.state != CaptureState::Idle {
                // What the recording was for ends with it, before Idle shows (CTL-11 a).
                self.burst.locked().recording = false;
                self.scan.locked().recording = false;
                s.capture.state = CaptureState::Idle;
                s.capture.from_osc = false;
                s.capture.error = Some(tr("The capture worker stopped unexpectedly.").into());
                ended = true;
            }
        });
        if ended {
            self.capture_ended();
        }
    }

    fn send(self: &Shared, cmd: &WorkerCommand) -> anyhow::Result<()> {
        #[cfg(test)]
        if let Some(commands) = self.test_commands.lock().unwrap().as_mut() { commands.push(cmd.clone()); return Ok(()); }
        self.ensure_worker()?;
        self.send_if_running(cmd)
    }

    /// Sends to the worker that runs, never starting one: OSC values reach it from the OSC receiving thread, under
    /// the values' lock, which must not wait for a process to start. Gone meanwhile (the app quits): an error.
    fn send_if_running(&self, cmd: &WorkerCommand) -> anyhow::Result<()> {
        let mut guard = self.worker.locked();
        guard.as_mut().ok_or_else(|| anyhow::anyhow!("{}", tr("the capture worker is not running")))?.send(cmd)
    }

    /// A 3D picture or a scan records a session of its own: none may run, nor still be being saved.
    pub(super) fn ensure_capture_idle(&self) -> anyhow::Result<()> {
        match self.snapshot.locked().capture.state {
            CaptureState::Idle => Ok(()),
            CaptureState::Stopping => anyhow::bail!("{}", tr("The last recording is still being saved: try again in a moment.")),
            _ => anyhow::bail!("{}", tr("A recording is under way: stop it first.")),
        }
    }

    /// A capture ended, or never started: a start asked for while the last session was being saved runs now, else
    /// a reconstruction that waited for the GPU.
    fn capture_ended(self: &Shared) {
        *self.stopping_since.locked() = None;
        let pending = self.pending_start.locked().take();
        match pending {
            Some((source, importing, from_osc, generation)) => {
                // Starting reads VRChat's current values over HTTP: not on the thread that told us.
                let ctl = self.clone();
                std::thread::spawn(move || {
                    if let Err(e) = ctl.resume_pending((source, importing, from_osc, generation)) {
                        ctl.notify(NoticeKind::Error, e.to_string());
                    }
                });
            }
            None => self.next_reconstruction(),
        }
    }

    fn resume_pending(self: &Shared, pending: (SourceSpec, bool, bool, u64)) -> anyhow::Result<()> {
        let (source, importing, from_osc, generation) = pending;
        self.start_with_generation(source, importing, from_osc, generation)
    }

    /// A worker that has not finished saving a session long after it was asked to is stuck: it is restarted (its
    /// session stays as its last checkpoint left it). Called by the watchers every second.
    fn check_stopping(self: &Shared) {
        let stuck = self.snapshot.locked().capture.state == CaptureState::Stopping
            && self.stopping_since.locked().is_some_and(|t| t.elapsed() > STOP_LIMIT);
        if stuck {
            *self.stopping_since.locked() = None;
            self.notify(NoticeKind::Error, tr("The capture worker did not finish saving the session: it was restarted. The session keeps what was saved."));
            // Dropping the handle ends the process; its end resets the state (`ensure_worker`'s exit).
            let worker = self.worker.locked().take();
            drop(worker);
        }
    }

    fn on_worker_event(self: &Shared, e: WorkerEvent) {
        match e {
            WorkerEvent::Ready { version } => {
                self.update(|s| {
                    s.worker.ready = true;
                    s.worker.error = (version != orbiters_core::app_version()).then(|| tr!("worker version {version} differs from the app", version = version));
                });
                // The microphones for the settings.
                let ctl = self.clone();
                std::thread::spawn(move || {
                    ctl.send(&WorkerCommand::ListSources).or_log("what a worker was asked did not reach it");
                });
            }
            // Spout senders are watched here (`spawn_watchers`); the worker knows the microphones.
            WorkerEvent::Sources { mics, .. } => {
                self.update_if(|s| s.mics.set(mics));
            }
            WorkerEvent::Started { session_dir, source } => {
                // Worker events run off the UI thread. Save before publishing the new recording.
                let mut content = orbiters_core::explicit::Content::parameters(
                    self.osc_values.locked().latest.keys().filter(|n| n.starts_with(PARAMETERS_PREFIX)).map(String::as_str),
                );
                let world = self.snapshot.locked().vrchat.world.clone();
                content.merge(&orbiters_core::explicit::local_avatar_labels(&world));
                let avatar = match self.osc_values.locked().latest.first(AVATAR_CHANGE) {
                    Some(OscValue::Text(id)) => Some(id),
                    _ => None,
                }
                .or_else(|| self.snapshot.locked().n4n0.avatar_id.clone());
                content.merge(&orbiters_core::explicit::manifest_content(avatar.as_deref()));
                if let Err(e) = content.save(&session_dir) {
                    self.notify(NoticeKind::Error, tr!("Could not save capture content flags: {error}", error = e));
                }
                self.update(|s| {
                    // Stopping: a stop came before the worker started, and its Stop follows the Start.
                    if !matches!(s.capture.state, CaptureState::Importing | CaptureState::Stopping) {
                        s.capture.state = CaptureState::Recording;
                    }
                    s.capture.session_dir = Some(session_dir);
                    s.capture.source = Some(source);
                    s.capture.error = None;
                    s.capture.trail.clear();
                    self.trail.locked().clear();
                    s.capture.coverage = None;
                });
            }
            // Published with the next stats (four times a second).
            WorkerEvent::Coverage(map) => self.snapshot.locked().capture.coverage = Some(map),
            WorkerEvent::Hips { position, rotation } => self.rig_hips(position, rotation),
            WorkerEvent::Stats(stats) => {
                let missing_s = stats.strip_missing_s;
                let over_limit = {
                    let mut s = self.snapshot.locked();
                    if let Some(p) = stats.position {
                        // The snapshot's path only when the whole one grew: a camera standing still copies nothing.
                        let mut trail = self.trail.locked();
                        if trail.push(p) {
                            s.capture.trail = trail.shown();
                        }
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
                self.check_strip(missing_s);
                // Stats arrive four times a second; that is also the interface's refresh rate while recording.
                self.publish();
                if over_limit {
                    self.notify(NoticeKind::Warning, tr("Storage limit reached: recording stopped. Raise the limit in Settings or free space."));
                    let _ = self.stop_capture(StopOf::Recording(self.recording_number()));
                }
            }
            WorkerEvent::Stopped { session_dir, manifest, reason } => {
                // A new Start came (a world split), or the worker started over by itself at a new frame size
                // ("resized": a session holds one size): the recording goes on in the next session.
                let resized = reason == "resized";
                let restarting = reason == "restarted" || resized;
                // The session the worker says it saved, not whatever the snapshot names by now (verify2).
                let session = Some(session_dir).filter(|d| !d.as_os_str().is_empty()).or_else(|| self.snapshot.locked().capture.session_dir.clone());
                if !restarting {
                    self.put_vrchat_back();
                }
                // Whether this recording was a scan, taken BEFORE Idle is published: a scan started after Idle sets
                // the flag for its own recording, which this save must neither take nor clear (CTL-11 a, C-2).
                let was_scan = if restarting {
                    self.scan.locked().recording
                } else {
                    std::mem::take(&mut self.scan.locked().recording)
                };
                // The same for a 3D picture (its flag, what it is of, who it left out), and the reason it has no
                // keyframe, all before Idle: after it, the next picture's own are there (verify-controller).
                let ended = self.capture_serial.load(Ordering::Acquire);
                let picture = if restarting { None } else { self.take_picture_ended() };
                let nothing = !restarting && manifest.stats.frames > 0 && manifest.stats.keyframes == 0;
                let why = nothing.then(|| self.no_strip_reason());
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
                // Frames, but not one with a pose: no capture, whatever a green "saved" would say, and nothing to reconstruct.
                if let Some(why) = why {
                    // Only on this recording: one started since (it took the next number) keeps its own.
                    self.update_if(|s| {
                        let ours = self.no_recording_since(ended) && !s.is_capturing();
                        if ours {
                            s.capture.no_strip = Some(why);
                        }
                        ours
                    });
                    self.notify(
                        NoticeKind::Error,
                        tr!("No 3D capture: {frames} frames came, none with the capture strip, so there are no keyframes. {reason}", frames = manifest.stats.frames, reason = why.message()),
                    );
                }
                self.push_notice(Notice::SessionSaved {
                    reason: if nothing { "restarted".into() } else { reason },
                    frames: manifest.stats.frames,
                    keyframes: manifest.stats.keyframes,
                    depth_frames: manifest.stats.depth_frames,
                });
                if resized {
                    // A 3D picture's or a scan's part at the old size is reconstructed on its own (a reconstruction
                    // takes one frame size); the part after it when the recording ends.
                    let part = self.burst.locked().recording || was_scan;
                    if let Some(dir) = session.clone().filter(|_| part) {
                        if let Err(e) = self.queue_reconstruction(dir) {
                            self.notify(NoticeKind::Warning, tr!("A part of the recording, at another resolution, is not reconstructed: {error}", error = e));
                        }
                    }
                }
                if !restarting {
                    self.world_import_saved(session.clone());
                    self.burst_saved(session.clone(), picture.filter(|_| !nothing));
                    self.scan_saved(session.filter(|_| was_scan && !nothing));
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
                let repeated = self.snapshot.locked().capture.error.as_deref() == Some(message.as_str());
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

    /// The world VRChat is in: a Mac's own game's, where one runs, which is said for captures alone.
    pub(super) fn world_of_captures(&self) -> orbiters_core::vrchat_log::WorldState {
        #[cfg(target_os = "macos")]
        if self.own_game() {
            return self.mac_world();
        }
        (*self.snapshot.locked().vrchat.world).clone()
    }

    fn current_world(&self) -> Option<WorldInfo> {
        let w = self.world_of_captures();
        (w.in_world && (w.world_id.is_some() || w.world_name.is_some())).then(|| WorldInfo {
            id: w.world_id,
            name: w.world_name,
            author_name: None,
            instance: w.instance,
        })
    }

    /// A start that is a picture's or a scan's own: it fails when anything records or is being saved (it would adopt
    /// that recording), `claim` runs under the start gate the moment it is accepted (the flag that says what it is
    /// for), and the number of the recording it started is handed back (verify-controller, C-2, CTL2-7).
    /// `unclaim` undoes `claim` when the start fails after it was accepted, under the same gate: a request refused
    /// before it claimed anything clears nothing (another capture's flag, while it is saved, is that capture's:
    /// verify2-controller).
    fn start_capture_owned(self: &Shared, purpose: CapturePurpose, claim: impl FnOnce(), unclaim: impl FnOnce()) -> anyhow::Result<u64> {
        let (name, dolly) = self.with_settings(|s| (s.spout_sender.clone(), s.dolly_streams));
        let (name, extra) = if dolly {
            (if name.is_empty() { dolly::MAIN_SENDER.to_string() } else { name }, dolly::EXTRA_SENDERS.map(String::from).to_vec())
        } else {
            (name, Vec::new())
        };
        let _ = purpose;
        let generation = self.stop_generation.load(Ordering::Acquire);
        let started = self.start_segment_claimed(SourceSpec::Spout { name, extra }, false, false, generation, false, Some((claim, unclaim)))?;
        started.ok_or_else(|| anyhow::anyhow!("{}", tr("A recording is under way: stop it first.")))
    }

    fn start_with(self: &Shared, source: SourceSpec, importing: bool, from_osc: bool) -> anyhow::Result<()> {
        let stop_generation = self.stop_generation.load(Ordering::Acquire);
        self.start_with_generation(source, importing, from_osc, stop_generation)
    }

    fn start_with_generation(self: &Shared, source: SourceSpec, importing: bool, from_osc: bool, stop_generation: u64) -> anyhow::Result<()> {
        self.start_segment(source, importing, from_osc, stop_generation, false)
    }

    fn start_segment(self: &Shared, source: SourceSpec, importing: bool, from_osc: bool, stop_generation: u64, rollover: bool) -> anyhow::Result<()> {
        self.start_segment_claimed(source, importing, from_osc, stop_generation, rollover, None::<(fn(), fn())>).map(|_| ())
    }

    /// [`Controller::start_segment`]; with `claim` (a picture's or a scan's own start), refused while anything records
    /// or is being saved, `claim` run under the gate once accepted. `Some(number)`: the recording this call started (or,
    /// for a world split, the one it goes on with); `None`: nothing started (one records already, or a stop came).
    fn start_segment_claimed(
        self: &Shared,
        source: SourceSpec,
        importing: bool,
        from_osc: bool,
        stop_generation: u64,
        rollover: bool,
        claim: Option<(impl FnOnce(), impl FnOnce())>,
    ) -> anyhow::Result<Option<u64>> {
        let _start = self.capture_start_gate.locked();
        let exclusive = claim.is_some();
        anyhow::ensure!(self.stop_generation.load(Ordering::Acquire) == stop_generation, "{}", tr("Finish the current capture first."));
        if rollover { anyhow::ensure!(matches!(self.view().capture.state,CaptureState::Recording|CaptureState::Paused), "{}", tr("Finish the current capture first.")); }
        if !rollover && matches!(self.view().capture.state, CaptureState::Starting | CaptureState::Recording | CaptureState::Paused) {
            anyhow::ensure!(!exclusive, "{}", tr("A recording is under way: stop it first."));
            return Ok(None);
        }
        if exclusive {
            self.ensure_capture_idle()?;
        }
        anyhow::ensure!(!self.quitting.load(Ordering::Relaxed), "{}", tr("Orbiters - N4N0 is quitting."));
        let settings = self.settings();
        let spout = matches!(source, SourceSpec::Spout { .. });
        if spout && !importing {
            self.require_live_camera()?;
        }
        anyhow::ensure!(self.stop_generation.load(Ordering::Acquire) == stop_generation, "{}", tr("Finish the current capture first."));
        if !importing && !settings.consent_acknowledged {
            anyhow::bail!("{}", tr("Please read and accept the capture notice first (Photos & captures)."));
        }
        if self.snapshot.locked().capture.state == CaptureState::Stopping {
            anyhow::ensure!(!rollover, "{}", tr("Finish the current capture first."));
            // The last session is still being saved: this one starts once it is (`capture_ended`), and gets its number
            // then (a start refused or queued takes none: it would end what the last flight still owes).
            *self.pending_start.locked() = Some((source, importing, from_osc, stop_generation));
            return Ok(None);
        }
        // Accepted: what it is for, then its number, under the gate (nothing else starts in between).
        // Undone (still under the gate: dropped before it) if the start does not go through.
        struct Undo<F: FnOnce()>(Option<F>);
        impl<F: FnOnce()> Drop for Undo<F> {
            fn drop(&mut self) {
                if let Some(undo) = self.0.take() {
                    undo();
                }
            }
        }
        let mut undo = claim.map(|(claim, unclaim)| {
            claim();
            Undo(Some(unclaim))
        });
        let number = if rollover {
            self.capture_serial.load(Ordering::Acquire)
        } else {
            self.capture_serial.fetch_add(1, Ordering::AcqRel) + 1
        };
        let cmd = StartCapture {
            source: source.clone(),
            sessions_dir: settings.sessions_dir.clone(),
            mode: settings.capture_mode,
            quality: settings.storage_quality,
            keyframes: settings.keyframes.clone(),
            world: if importing { raw_capture_world(&source) } else { self.current_world() },
            consent: ConsentInfo::default(),
            folder_name: None,
            performance: if importing { PerformanceSpec::default() } else { self.performance_spec(&settings) },
        };
        self.update(|s| {
            s.capture.state = if importing { CaptureState::Importing } else { CaptureState::Starting };
            s.capture.live = spout && !importing;
            s.capture.error = None;
            s.capture.stats = None;
            s.capture.trail.clear();
            self.trail.locked().clear();
            s.capture.coverage = None;
            s.capture.from_osc = from_osc;
            s.capture.photo_3d = self.burst.locked().recording;
            s.capture.picture = self.burst.locked().picture_mode();
            s.capture.left_out = Default::default();
            s.capture.no_strip = None;
        });
        if !importing {
            self.capture_takes_the_gpu();
        }
        *self.last_source.locked() = (!importing).then_some(source);
        let record_parameters = !importing && settings.osc.enabled && settings.osc.record_parameters;
        // VRChat only sends values when they change: its OSCQuery tree has the current ones.
        let fetch_started = clock::now_ns();
        // A 3D picture always needs VRChat's camera streaming over Spout; a recording, as the setting says.
        let drive_camera = {
            let mut owners=self.camera_stream_owners.locked();
            let drive = !importing && settings.osc.enabled && spout && (settings.osc.drive_camera || self.burst.locked().recording || owners[1]);
            // Reserve before querying: QR completion cannot restore the producer midway through this read.
            if drive { owners[0]=true; }
            drive
        };
        // N4N0's capture kit draws only while its record toggle is on (the app sets it, or N4N0 › Record in the Action
        // Menu did), with depth frames while its depth toggle is (only the app sets it); the camera rig follows the camera.
        let drive_avatar = !importing && settings.osc.enabled && spout;
        let vrchat = if record_parameters || drive_camera || drive_avatar { self.vrchat_values() } else { Vec::new() };
        if drive_camera && self.view().camera_open_said == Some(true) {
            self.open_stream_camera(&vrchat);
        }
        if drive_avatar && self.view().camera_open_said == Some(true) {
            self.switch_kit_on(&vrchat);
            self.start_rig();
            // After the kit's toggle: the avatar's layer only hides while the kit records.
            let world = self.burst.locked().picture_mode() == Some(orbiters_core::settings::PictureMode::World);
            self.start_hide_self(&vrchat, drive_camera, world);
        }
        let sent = {
            let mut osc = self.osc_values.locked();
            // A stop came while VRChat's values were read: nothing starts. Checked under the lock `stop_capture`
            // takes to send its Stop, so a stop that comes later reaches the worker after this Start.
            if self.snapshot.locked().capture.state == CaptureState::Stopping
                || self.stop_generation.load(Ordering::Acquire) != stop_generation
                || (spout && self.view().camera_open_said != Some(true)) {
                None
            } else {
                // The previous session (a world change restarts it) gets its last values first.
                self.flush_osc(&mut osc);
                osc.forwarding = false;
                let sent = self.send(&WorkerCommand::Start(Box::new(cmd)));
                // The worker this recording runs on, once its Start went: the one installed (`send` started one if
                // none ran), with the recording's number. A Start that failed leaves the last one's (verify2).
                if sent.is_ok() {
                    self.recording_worker_number.store(number, Ordering::Release);
                    self.recording_worker.store(self.worker_installed.load(Ordering::Acquire), Ordering::Release);
                }
                if sent.is_ok() && record_parameters {
                    let samples = initial_samples(vrchat, &osc.latest, fetch_started);
                    osc.forwarding = true;
                    osc.pending.clear();
                    if !samples.is_empty() {
                        self.send(&WorkerCommand::Osc { samples }).or_log("what a worker was asked did not reach it");
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
            return Ok(None);
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
        if let Some(undo) = undo.as_mut() {
            undo.0 = None;
        }
        Ok(Some(number))
    }

    /// The recording ended or never started: no more values to the worker, and VRChat's camera and the capture
    /// kit's toggles back as they were.
    /// The recording under way now (`Controller::capture_serial`), for a flight that must act on its own only.
    pub(super) fn recording_number(&self) -> u64 {
        self.capture_serial.load(Ordering::Acquire)
    }

    /// Whether the recording a flight started (`owner`, [`Controller::recording_number`] after its start) is still the
    /// one under way. Asked under `capture_start_gate` by whoever acts on the answer (C-2, CTL2-7).
    pub(super) fn owns_recording(&self, owner: u64) -> bool {
        self.capture_serial.load(Ordering::Acquire) == owner && self.is_capturing()
    }

    /// Whether no other recording began since `owner`'s (it may have ended): what a flight changed of the camera is
    /// still its own to put back.
    pub(super) fn no_recording_since(&self, owner: u64) -> bool {
        self.capture_serial.load(Ordering::Acquire) == owner
    }

    /// Stops `owner`'s own recording, never one started after it, and calls off no start asked since.
    pub(super) fn stop_recording_of(self: &Shared, owner: u64) {
        let _gate = self.capture_start_gate.locked();
        if !self.owns_recording(owner) {
            return;
        }
        if let Err(e) = self.stop_capture_now() {
            self.notify(NoticeKind::Error, e.to_string());
        }
    }

    /// Does `effect` (an OSC message, the rig, a mask) for `owner`'s recording only: checked and done under the start
    /// gate, so no other recording can begin between the check and the effect. Whether it was done.
    pub(super) fn for_recording(&self, owner: u64, effect: impl FnOnce()) -> bool {
        let _gate = self.capture_start_gate.locked();
        let ours = self.owns_recording(owner);
        if ours {
            effect();
        }
        ours
    }

    /// A flight that gives its start up (it never reached Recording, or its thread could not start): `flag` (what said
    /// the recording was a picture's or a scan's) goes, and the start is stopped when it is still under way, all under
    /// the gate; nothing when another recording began since (its flag and recording are its own).
    pub(super) fn give_up_start_of(self: &Shared, owner: u64, flag: impl FnOnce()) {
        let _gate = self.capture_start_gate.locked();
        if !self.no_recording_since(owner) {
            return;
        }
        flag();
        if self.is_capturing() {
            if let Err(e) = self.stop_capture_now() {
                self.notify(NoticeKind::Error, e.to_string());
            }
        }
    }

    /// Why a saved recording had no keyframe: what was said while it ran, or else what is known now. The said reason
    /// is copied out and the snapshot let go BEFORE the fallback: `why_no_strip_now` takes `osc_values`, and a start
    /// meanwhile holds `osc_values` and waits for `snapshot` (`start_segment`): the two would wait for each other (C-1).
    fn no_strip_reason(&self) -> state::NoStrip {
        let said = self.snapshot.locked().capture.no_strip;
        said.unwrap_or_else(|| self.why_no_strip_now())
    }

    fn put_vrchat_back(&self) {
        self.osc_values.locked().forwarding = false;
        self.stop_rig();
        // Before the camera, which may close and take the mask's message with it, and the kit, whose toggle ends the hiding.
        self.restore_hide_self();
        self.restore_camera();
        self.restore_kit();
    }

    /// VRChat publishes its camera over Spout only in Stream mode with Spout Stream on: switch both on over OSC
    /// (VRChat 2025.3.3 and later), remembering how the camera was. Spout Stream goes on a moment after the camera
    /// opens in Stream mode.
    fn open_stream_camera(self: &Shared, vrchat: &[(String, OscValue)]) {
        self.acquire_camera_stream(0, vrchat);
    }

    fn received_camera_closed(&self, evidence:&OscValues) -> bool {
        let Some(closed)=evidence.camera_closed_at else { return false; };
        let now=clock::now_ns();
        !self.view().orbit.camera_panel_seen_ns.is_some_and(|seen|seen>closed && seen<=now && now-seen<2_000_000_000)
    }

    fn acquire_camera_stream(self: &Shared, owner: usize, vrchat: &[(String, OscValue)]) -> bool {
        // Several VRChats: only the target's camera may stream (they all name their sender VRCSender1).
        let shared_name = self.view().camera_open_said != Some(false) && self.quiet_other_streams();
        let mut owners = self.camera_stream_owners.locked();
        let mut evidence=self.osc_values.locked();
        if self.received_camera_closed(&evidence) || self.view().camera_open_said==Some(false) { owners[owner]=false; drop(evidence); drop(owners); self.unquiet_unless_streaming(); return false; }
        owners[owner] = true;
        let value = |path: &str| vrchat.iter().find(|(p, _)| p == path).map(|(_, v)| v.clone());
        let mode = match value(CAMERA_MODE) {
            Some(OscValue::Int(m)) => m,
            _ => 0,
        };
        let streaming = value(CAMERA_STREAMING).is_some_and(|v| v.truthy());
        if mode == STREAM_MODE && streaming && !shared_name {
            return true;
        }
        self.camera_before.locked().get_or_insert(CameraState { mode, streaming });
        if streaming && shared_name {
            // The target streamed already, under the name the others just left: it announces its own picture again.
            let _ = self.with_osc(|o| o.send(CAMERA_STREAMING, vec![rosc::OscType::Bool(false)]));
        }
        if mode != STREAM_MODE {
            let now=clock::now_ns();
            evidence.camera_command=Some((STREAM_MODE,now));
            evidence.camera_commanded.insert(STREAM_MODE,now);
            let _ = self.with_osc(|o| o.send(CAMERA_MODE, vec![rosc::OscType::Int(STREAM_MODE)]));
        }
        drop(evidence);
        let ctl = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            // Unless the camera was put back meanwhile (the start failed).
            let owners = ctl.camera_stream_owners.locked();
            if owners.iter().any(|on| *on) && ctl.view().camera_open_said != Some(false) && !ctl.received_camera_closed(&ctl.osc_values.locked()) && ctl.camera_before.locked().is_some() {
                let _ = ctl.with_osc(|o| o.send(CAMERA_STREAMING, vec![rosc::OscType::Bool(true)]));
            }
        });
        true
    }

    /// Switches N4N0's capture kit's record and depth-frames toggles on (OSC), remembering how depth frames were.
    fn switch_kit_on(&self, vrchat: &[(String, OscValue)]) {
        let on = |name: &str| vrchat.iter().any(|(p, v)| p.strip_prefix(PARAMETERS_PREFIX) == Some(name) && v.truthy());
        let (record, depth) = (on(RECORD_PARAMETER), on(DEPTH_FRAMES_PARAMETER));
        self.avatar_before.locked().get_or_insert(AvatarToggles { depth });
        let _ = self.with_osc(|o| {
            if !record {
                o.send(&format!("{PARAMETERS_PREFIX}{RECORD_PARAMETER}"), vec![rosc::OscType::Bool(true)])?;
            }
            if !depth {
                o.send(&format!("{PARAMETERS_PREFIX}{DEPTH_FRAMES_PARAMETER}"), vec![rosc::OscType::Bool(true)])?;
            }
            anyhow::Ok(())
        });
    }

    /// The capture kit's toggles back (first forgotten, so their echo is the person's again): record off, as no recording
    /// runs any more, and depth frames as they were.
    fn restore_kit(&self) {
        let Some(before) = self.avatar_before.locked().take() else { return };
        let _ = self.with_osc(|o| {
            o.send(&format!("{PARAMETERS_PREFIX}{RECORD_PARAMETER}"), vec![rosc::OscType::Bool(false)])?;
            o.send(&format!("{PARAMETERS_PREFIX}{DEPTH_FRAMES_PARAMETER}"), vec![rosc::OscType::Bool(before.depth)])
        });
    }

    /// The record toggle changed in VRChat: start or stop, unless it is an avatar reload's reset of a toggle the app
    /// drives (then switch it back on).
    fn on_record_toggle(self: &Shared, on: bool) {
        let driven = self.avatar_before.locked().is_some();
        let reloading = self.avatar_changed_at.locked().is_some_and(|t| t.elapsed() < AVATAR_RELOAD_GRACE);
        if driven && !on && reloading && self.is_capturing() {
            self.switch_kit_on(&[]);
            return;
        }
        let r = if on { self.start_capture(CapturePurpose::Avatar) } else { self.stop_capture(StopOf::Person) };
        if let Err(err) = r {
            self.notify(NoticeKind::Error, err.to_string());
        }
    }

    /// A new avatar (or the same one reloaded, as in every new world) starts with the kit's toggles off: they are
    /// switched on again once it settled. One thread waits for that whatever the number of changes (a burst of them, or
    /// datagrams posing as VRChat's, starts no thread each), counting from the newest.
    fn on_avatar_changed(self: &Shared) {
        *self.avatar_changed_at.locked() = Some(Instant::now());
        if self.avatar_before.locked().is_none() || !self.is_capturing() || self.kit_rearm.swap(true, Ordering::AcqRel) {
            return;
        }
        let ctl = self.clone();
        let spawned = std::thread::Builder::new().name("kit-rearm".into()).spawn(move || {
            // The avatar's parameters take a moment to exist after the change message.
            loop {
                let since = ctl.avatar_changed_at.locked().map_or(KIT_SETTLE, |t| t.elapsed());
                if since >= KIT_SETTLE {
                    break;
                }
                std::thread::sleep(KIT_SETTLE - since);
            }
            ctl.kit_rearm.store(false, Ordering::Release);
            if ctl.avatar_before.locked().is_some() && ctl.is_capturing() {
                ctl.switch_kit_on(&[]);
                // The new avatar's rig rests at its hips until told to follow again.
                ctl.rig_avatar_changed();
                ctl.hide_self_avatar_changed();
            }
        });
        if spawned.is_err() {
            self.kit_rearm.store(false, Ordering::Release);
        }
    }

    /// Puts VRChat's camera back as it was before recording opened it.
    fn restore_camera(&self) {
        self.release_camera_stream(0);
    }

    /// The other VRChats' Spout Stream back on, unless a recording or a code scan still streams here.
    fn unquiet_unless_streaming(&self) {
        if !self.camera_stream_owners.locked().iter().any(|on| *on) {
            self.unquiet_other_streams();
        }
    }

    fn release_camera_stream(&self, owner: usize) {
        let mut owners = self.camera_stream_owners.locked();
        owners[owner] = false;
        if owners.iter().any(|on| *on) { return; }
        self.unquiet_other_streams();
        let Some(before) = self.camera_before.locked().take() else { return };
        let _ = self.with_osc(|o| {
            o.send(CAMERA_STREAMING, vec![rosc::OscType::Bool(before.streaming)])?;
            // Closing the real camera wins over restoration. Writing a nonzero Mode while closed
            // changes its stored query value and would falsely certify the next live start.
            if self.view().camera_open_said == Some(false) || self.received_camera_closed(&self.osc_values.locked()) { return Ok(()); }
            self.camera_mode_command(before.mode);
            o.send(CAMERA_MODE, vec![rosc::OscType::Int(before.mode)])
        });
    }

    fn performance_spec(&self, settings: &Settings) -> PerformanceSpec {
        let p = &settings.performance;
        // The stored calibration's clock reading may come from before a restart: date it by its age instead.
        let calibration = self.calibration.locked().clone().map(|s| {
            let age_ns = unix_ms().saturating_sub(s.utc_ms) as i64 * 1_000_000;
            Calibration { time_ns: clock::now_ns() as i64 - age_ns, ..s.calibration }
        });
        PerformanceSpec {
            mic: p.record_mic.then(|| p.mic_device.clone()),
            game_audio_pid: if p.record_game_audio { self.snapshot.locked().vrchat.pid } else { None },
            tracking_hz: p.record_tracking.then_some(p.tracking_rate_hz),
            calibration: p.record_tracking.then_some(calibration).flatten(),
        }
    }

    /// A T-pose calibration from the overlay: kept for the next sessions, and added to the running one.
    pub fn calibrated(self: &Shared, calibration: Calibration) {
        let stored = StoredCalibration { utc_ms: unix_ms(), calibration };
        let info = calibration_info(&stored);
        *self.calibration.locked() = Some(stored.clone());
        self.update(|s| s.steamvr.calibration = Some(info.clone()));
        if self.is_capturing() {
            if let Err(e) = self.send(&WorkerCommand::Calibration { calibration: Box::new(stored.calibration.clone()) }) {
                log_warn!("calibration to the worker: {e:#}", e = e);
            }
        }
        self.notify(NoticeKind::Success, tr!("Calibrated: eyes at {eyes} m, hands {hands} m apart.", eyes = format!("{:.2}", info.head_height_m), hands = format!("{:.2}", info.arm_span_m)));
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
                ctl.notify(NoticeKind::Error, tr!("Could not keep the calibration: {error}", error = e));
            }
        });
    }

    /// The overlay process connected or left (with it, whether the headset is worn is unknown).
    /// What the panels around VRChat's menus say of themselves, for Settings.
    pub fn set_orbit_status(self: &Shared, status: crate::state::OrbitStatus) {
        self.update(|s| {
            s.orbit.set(status);
        });
    }

    /// Reads again whether the avatar worn has the capture kit, for what offers 3D pictures. Published only when it changed.
    pub fn follow_worn_kit(self: &Shared) {
        let epoch=self.kit_epoch.locked().clone();
        let kit = if self.view().vrchat.running { self.worn_avatar_has_kit() } else { None };
        self.apply_kit_observation(&epoch,kit);
        if self.view().camera_open_said.is_none() {
            let route = self.camera_route.load(Ordering::Acquire);
            let camera = self.camera_values().into_iter().find_map(|(p,v)| match v {
                OscValue::Int(mode) if p == CAMERA_MODE => Some(mode != 0),
                _ => None,
            });
            if let Some(open) = camera { self.apply_keeper_camera(route, open); }
        }
    }

    /// What the keeper's look at VRChat's tree found of its camera, shown while nothing is known yet, and only if the
    /// VRChat it asked (`route`) is still the one followed: a slow answer of the last one does not seed the next (C-8).
    fn apply_keeper_camera(&self, route: u64, open: bool) -> bool {
        self.update_if(|s| {
            if s.camera_open_said.is_some() || self.camera_route.load(Ordering::Acquire) != route { return false; }
            s.camera_open_said = Some(open);
            true
        })
    }

    /// Another VRChat's camera from now on ([`Controller::apply_keeper_camera`]).
    pub(super) fn camera_route_changed(&self) {
        self.camera_route.fetch_add(1, Ordering::AcqRel);
    }

    fn invalidate_kit(&self, avatar: Option<String>) {
        let mut epoch=self.kit_epoch.locked();
        epoch.0=epoch.0.wrapping_add(1);
        epoch.1=avatar;
        self.update(|s| s.worn_kit=None);
    }

    fn apply_kit_observation(&self, observed: &(u64,Option<String>), kit: Option<bool>) -> bool {
        let epoch=self.kit_epoch.locked();
        if *epoch != *observed { return false; }
        if let Some(expected)=&epoch.1 {
            if !self.osc_values.locked().latest.get(AVATAR_CHANGE).is_some_and(|(v,_)| matches!(v.first(),Some(OscValue::Text(id)) if id==expected)) { return false; }
        }
        self.update_if(|s| {
            let changed=s.worn_kit!=kit;
            s.worn_kit=kit;
            changed
        });
        true
    }

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
    /// it was off pauses, to wait for VRChat to close (or the next break).
    pub fn set_headset(self: &Shared, in_use: Option<bool>) {
        self.update_if(|s| {
            let changed = s.steamvr.headset_in_use != in_use;
            s.steamvr.headset_in_use = in_use;
            changed
        });
        {
            let mut since = self.headset_off_since.locked();
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
        if self.held_for_vrchat() {
            crate::upload_prep::pause();
        }
        self.follow_while_playing();
    }

    /// The headset has been off long enough for reconstructions held for VRChat to start ([`HEADSET_BREAK`]).
    fn headset_on_break(&self) -> bool {
        self.headset_off_since.locked().is_some_and(|t| t.elapsed() >= HEADSET_BREAK)
    }

    /// Current values from VRChat's OSCQuery service (empty when it is not running or not found). The request runs
    /// without the OSC lock: camera and kit messages, and the watchers, never wait for VRChat's answer.
    fn vrchat_values(&self) -> Vec<(String, OscValue)> {
        let (query, mut values) = self.vrchat_avatar_values();
        let Some(query) = query else { return values };
        // The camera's state, which recording changes and puts back: from VRChat's tree, else from what VRChat
        // sent when it last changed.
        let camera = query.map(|addr| orbiters_core::osc::vrchat_camera_values(addr, Duration::from_millis(500))).unwrap_or_default();
        if camera.iter().any(|(p, _)| p == CAMERA_MODE) {
            values.extend(camera);
        } else {
            let osc = self.osc_values.locked();
            for path in [CAMERA_MODE, CAMERA_STREAMING] {
                if let Some(value) = osc.latest.get(path).and_then(|(v, _)| v.first().cloned()) {
                    values.push((path.to_string(), value));
                }
            }
        }
        values
    }

    /// VRChat's `/avatar` tree alone, and where its OSCQuery answers (`None` outside: no OSC; `Some(None)`: not found).
    /// The kit probe asks only this: the camera's tree beside it was a second request every 2 s for nothing (CTL-10).
    pub(super) fn vrchat_avatar_values(&self) -> (Option<Option<std::net::SocketAddr>>, Vec<(String, OscValue)>) {
        self.read_vrchat_avatar_values(|addr| orbiters_core::osc::vrchat_values(addr, Duration::from_secs(1)))
    }

    fn read_vrchat_avatar_values(
        &self,
        fetch: impl FnOnce(std::net::SocketAddr) -> anyhow::Result<Vec<(String, OscValue)>>,
    ) -> (Option<Option<std::net::SocketAddr>>, Vec<(String, OscValue)>) {
        let query = self.with_osc(|o| o.vrchat_query());
        let result = match query {
            None => return (None, Vec::new()),
            Some(None) => Err(anyhow::anyhow!("VRChat's OSCQuery service was not found")), // i18n: not shown (logged as "current OSC values")
            Some(Some(addr)) => fetch(addr),
        };
        let values = result.unwrap_or_else(|e| {
            log_warn!("current OSC values: {e:#}", e = e);
            Vec::new()
        });
        (query, values)
    }

    /// Sends the waiting OSC samples to the worker. Callers hold the values lock, which keeps batches in order.
    fn flush_osc(&self, osc: &mut OscValues) {
        osc.last_flush = Some(Instant::now());
        if osc.pending.is_empty() {
            return;
        }
        let samples = std::mem::take(&mut osc.pending);
        if let Err(e) = self.send_if_running(&WorkerCommand::Osc { samples }) {
            log_warn!("OSC values to the worker: {e:#}", e = e);
        }
    }

    /// Does what an OSC message asked for on the OSC action thread (started on first use), after what earlier
    /// messages asked for (N4N0's among them): a record toggle pressed twice stops what it started. Not on the receiving
    /// thread, which must keep reading (face tracking sends hundreds of values a second into a small socket buffer)
    /// while starting a capture or N4N0's ears takes a while. False when the action was dropped:
    /// [`orbiters_n4n0::Actions::MOST`] wait already (a flood of messages, which must not pile up work and memory without
    /// end). A waiting action holds the controller weakly: once it is gone, what waited does nothing.
    pub(super) fn osc_action(self: &Shared, action: impl FnOnce(&Shared) + Send + 'static) -> bool {
        let ctl = Arc::downgrade(self);
        self.osc_actions.run(Box::new(move || {
            if let Some(ctl) = ctl.upgrade() {
                action(&ctl);
            }
        }))
    }

    /// What one datagram from VRChat said, on the OSC receiving thread, which never waits for the OSC lock (see
    /// n4n0.rs): what it starts goes to the OSC action thread, what it says is kept, under one lock for the whole
    /// datagram.
    fn on_osc_events(self: &Shared, events: &mut Vec<OscEvent>, at_ns: u64, record_parameter: &str) {
        let mut messages = Vec::with_capacity(events.len());
        let mut went_off = false;
        // The avatar this batch's events already invalidated the kit for: `on_osc` does not do it a second time.
        let mut invalidated: Option<String> = None;
        // How the player moves, for live translation in VR (the overlay keeps translations on the world as the person
        // walks with the stick): forwarded only while it is on.
        let mut locomotion: Vec<(String, f32)> = Vec::new();
        let translating = self.view().settings.translate.enabled
            || (cfg!(debug_assertions) && (std::env::var_os("ORBITERS_TRANSLATE_ON").is_some() || std::env::var_os("ORBITERS_TRANSLATE_ON_FILE").is_some()));
        for event in events.drain(..) {
            if translating {
                match &event {
                    OscEvent::Parameter { name, value } if LOCOMOTION.contains(&name.as_str()) => {
                        if let OscValue::Float(v) = value {
                            locomotion.push((name.clone(), *v));
                        }
                    }
                    OscEvent::Values { address, values } if address == orbiters_n4n0_core::states::EYE_HEIGHT => {
                        if let Some(OscValue::Float(v)) = values.first() {
                            locomotion.push((address.clone(), *v));
                        }
                    }
                    _ => {}
                }
            }
            match event {
                OscEvent::Parameter { name, value } => {
                    // Face tracking's flood skips N4N0's buttons (none is one of its parameters).
                    if !face_tracking(&name) {
                        self.n4n0.on_parameter(&name, &value);
                        self.avatar_gun_parameter(&name, &value);
                    }
                    if name == record_parameter {
                        self.record_toggled(value.truthy());
                    }
                    messages.push((format!("{PARAMETERS_PREFIX}{name}"), vec![value]));
                }
                OscEvent::AvatarChanged { id } => {
                    self.invalidate_kit(Some(id.clone()));
                    invalidated = Some(id.clone());
                    self.on_avatar_changed();
                    // N4N0 reads the parameters the last avatar held before they go (`on_osc`).
                    self.n4n0.on_avatar(id.clone());
                    self.avatar_gun_avatar(&id);
                    messages.push((AVATAR_CHANGE.to_string(), vec![OscValue::Text(id)]));
                }
                OscEvent::Values { address, values } => {
                    if cfg!(debug_assertions) && std::env::var_os("ORBITERS_VR_SANDBOX_ROOT").is_some()
                        // Not /tracking/vrsystem/*: VRChat sends those every frame, and they grew the sandbox's app-err.log by 2.7 MB a minute (227 MB in one session).
                        && (address=="/usercamera/Pose" || address=="/usercamera/Flying") {
                        log_info!("sandbox camera OSC {at_ns} {address} {values:?}", at_ns = at_ns, address = address, values = values); // i18n: not shown (developer diagnostics)
                    }
                    // Where VRChat's avatar's hands are: its camera panel opens by the left, and is grabbed from either (`camera_reckon`).
                    let wrist = match address.as_str() { "/tracking/vrsystem/leftwrist/pose" => Some(0), "/tracking/vrsystem/rightwrist/pose" => Some(1), _ => None };
                    if let Some(hand) = wrist.filter(|_| values.len() == 6) {
                        let floats: Vec<f32> = values.iter().filter_map(|v| if let OscValue::Float(f) = v { Some(*f) } else { None }).collect();
                        if let Ok(pose) = <[f32; 6]>::try_from(floats.as_slice()) {
                            let now = orbiters_core::clock::now_ns();
                            self.update_if(|s| wrist_said(s, hand, pose, now));
                        }
                    }
                    if address == orbiters_n4n0_core::states::EYE_HEIGHT {
                        if let Some(OscValue::Float(height)) = values.first() {
                            self.n4n0.on_eye_height(*height);
                        }
                    }
                    let mask_off = address == hide_self::LOCAL_PLAYER && values.first().is_some_and(|v| !v.truthy());
                    messages.push((address, values));
                    if mask_off {
                        went_off = true;
                    }
                }
            }
        }
        if !locomotion.is_empty() {
            let mut subs = self.subscribers.locked();
            // Only the readers that show things (the overlay, a window): the tray's mailbox would only keep them.
            subs.retain(|m| !m.shows_notices() || m.post(ToClient::Locomotion { values: locomotion.clone() }));
        }
        self.on_osc_after(messages, at_ns, invalidated);
        if went_off {
            let _ = self.osc_action(|c| c.local_user_went_off());
        }
    }

    /// Keeps what VRChat said (and passes it to a session recording it). An avatar change forgets the last avatar's
    /// parameters: a session started now lists the avatar worn, not every avatar worn before.
    fn on_osc(&self, messages: Vec<(String, Vec<OscValue>)>, at_ns: u64) {
        self.on_osc_after(messages, at_ns, None);
    }

    /// [`Controller::on_osc`], for a batch whose events already invalidated the kit for the avatar `invalidated`
    /// ([`Controller::on_osc_events`]): one invalidation (an epoch, a publish) per avatar change, not two (CTL-11 c).
    fn on_osc_after(&self, messages: Vec<(String, Vec<OscValue>)>, at_ns: u64, invalidated: Option<String>) {
        let avatar_changed = messages.iter().any(|(p,_)| p == AVATAR_CHANGE);
        if avatar_changed {
            let avatar=messages.iter().rev().find(|(p,_)| p==AVATAR_CHANGE).and_then(|(_,v)| match v.first() { Some(OscValue::Text(id))=>Some(id.clone()), _=>None });
            // Invalidate before the new id becomes observable to the demo completion watcher.
            if avatar.is_none() || avatar != invalidated {
                self.invalidate_kit(avatar);
            }
        }
        let camera = messages.iter().rev().find(|(p, _)| p == CAMERA_MODE).and_then(|(_, v)| match v.first() {
            Some(OscValue::Int(mode)) => Some(*mode != 0),
            _ => None,
        });
        let mut osc = self.osc_values.locked();
        let mode = messages.iter().rev().find(|(p,_)| p == CAMERA_MODE).and_then(|(_,v)| match v.first() { Some(OscValue::Int(m)) => Some(*m), _ => None });
        let echo = mode.is_some_and(|mode| osc.camera_command.is_some_and(|(written,_)| written == mode));
        if echo { log_info!("camera Mode app echo ignored at {at_ns}: {mode:?}", at_ns = at_ns, mode = mode); } // i18n: not shown (diagnostics)
        // Command provenance lasts for this connection; a close permits a real reopening.
        if mode == Some(0) { osc.camera_command=None; osc.camera_closed_at=Some(at_ns); }
        let camera = if echo { None } else { camera };
        if let Some(open) = camera {
            // Same-mode reopening is a visibility hint, but a previously commanded value
            // needs fresh visual evidence before authorizing a live start.
            // A Mode the app wrote is proof only once it cannot be VRChat's echo of that write any more: the person
            // closing the camera and opening it again in the mode a picture put back (2026-11-15 13:48, desktop: the
            // second picture said "Open VRChat's camera first" with the camera open) is a real opening.
            let independent=open && mode.is_none_or(|m| osc.camera_commanded.get(&m).is_none_or(|written| at_ns.saturating_sub(*written) > CAMERA_ECHO_WINDOW_NS));
            osc.camera_received=Some(independent);
            if independent { osc.camera_closed_at=None; }
        }
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
        drop(osc);
        if let Some(open) = camera {
            log_info!("camera Mode received at {at_ns}, publishing open={open}", at_ns = at_ns, open = open); // i18n: not shown (diagnostics)
            self.update_if(|s| {
                let changed = s.camera_open_said != Some(open);
                s.camera_open_said = Some(open);
                if !open {
                    s.orbit.camera_panel_seen_ns = None;
                    s.camera_tools.qr_generation += 1;
                    s.camera_tools.qr = camera_tools::Qr::Off;
                }
                changed
            });
            if !open { self.camera_scan.locked().take(); self.release_camera_stream(1); }
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
        let queued = self.osc_action(|c| {
            let on = match c.record_toggle.swap(TOGGLE_NONE, Ordering::AcqRel) {
                TOGGLE_ON => true,
                TOGGLE_OFF => false,
                _ => return,
            };
            let parameter = RECORD_PARAMETER;
            c.update(|s| s.osc.last_event = Some(format!("{parameter} = {}", if on { "on" } else { "off" })));
            if !c.vrchat_agrees(parameter, on) {
                log_info!("{parameter} = {on} came over OSC, but VRChat's avatar does not say so: ignored", parameter = parameter, on = on);
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

    fn start_capture(self: &Shared, purpose: CapturePurpose) -> anyhow::Result<()> {
        self.start_capture_generation(purpose, self.stop_generation.load(Ordering::Acquire))
    }

    fn start_capture_generation(self: &Shared, purpose: CapturePurpose, stop_generation: u64) -> anyhow::Result<()> {
        if self.is_capturing() {
            return Ok(());
        }
        anyhow::ensure!(purpose != CapturePurpose::Avatar, "{}", tr("Open VRChat's camera first"));
        let (name, dolly) = self.with_settings(|s| (s.spout_sender.clone(), s.dolly_streams));
        // The camera dolly's other streams (experimental): VRChat's own camera is then VRCSender1, whatever else sends.
        let (name, extra) = if dolly {
            (if name.is_empty() { dolly::MAIN_SENDER.to_string() } else { name }, dolly::EXTRA_SENDERS.map(String::from).to_vec())
        } else {
            (name, Vec::new())
        };
        self.start_with_generation(SourceSpec::Spout { name, extra }, false, false, stop_generation)
    }

    /// All UI long-recording requests carry their surface through IPC.
    pub fn start_capture_from(self: &Shared, origin: crate::ipc::RecordingOrigin) -> anyhow::Result<()> {
        anyhow::ensure!(origin == crate::ipc::RecordingOrigin::CameraOverlay, "{}", tr("Open VRChat's camera first"));
        self.start_capture(CapturePurpose::CameraOverlay)
    }

    /// Keep IPC reading while preflight runs, so Stop cancels even a start still waiting for OSCQuery.
    pub fn request_capture_from(self: &Shared, origin: crate::ipc::RecordingOrigin) {
        let generation = self.stop_generation.load(Ordering::Acquire);
        let ctl = self.clone();
        std::thread::spawn(move || {
            let r = if origin == crate::ipc::RecordingOrigin::CameraOverlay {
                ctl.start_capture_generation(CapturePurpose::CameraOverlay,generation)
            } else { ctl.start_capture_from(origin) };
            if let Err(e) = r { ctl.notify(NoticeKind::Error,e.to_string()); }
        });
    }

    /// A new query, never a value written by the app or a cached Mode from an earlier connection.
    fn require_live_camera(&self) -> anyhow::Result<()> {
        let open = self.camera_values().iter().any(|(p, v)| p == CAMERA_MODE && matches!(v, OscValue::Int(_)) && v.truthy());
        let received = self.osc_values.locked().camera_received == Some(true);
        let now = clock::now_ns();
        let seen = self.view().orbit.camera_panel_seen_ns.is_some_and(|at| at > 0 && at <= now && now-at < 2_000_000_000);
        anyhow::ensure!(open && self.view().camera_open_said == Some(true) && (received || seen), "{}", tr("Open VRChat's camera first"));
        Ok(())
    }

    fn camera_mode_command(&self, mode: i32) {
        let mut osc=self.osc_values.locked();
        let now=clock::now_ns();
        osc.camera_command=Some((mode,now));
        osc.camera_commanded.insert(mode,now);
    }

    pub fn start_test_capture(self: &Shared) -> anyhow::Result<()> {
        self.start_with(SourceSpec::TestPattern { width: 1920, height: 1080, fps: 30.0, depth_every: 4, frames: 0 }, false, false)
    }

    pub fn import_raw_capture(self: &Shared, dir: PathBuf) -> anyhow::Result<()> {
        if self.is_capturing() {
            anyhow::bail!("{}", tr("Finish the current capture first."));
        }
        self.start_with(SourceSpec::RawSequence { path: dir, realtime: false }, true, false)
    }

    /// Stops a recording. A stop must say which: the person's own Stop (the button, the tray, the avatar's record toggle)
    /// stops what records now and calls off a start waiting; anything automatic (a picture's flight, the bridge's
    /// clean-up, the storage limit) names the recording it is about, and a recording begun since is left alone
    /// (verify2-controller: no caller can stop "whatever is recording").
    pub fn stop_capture(self: &Shared, of: StopOf) -> anyhow::Result<()> {
        match of {
            StopOf::Person => self.stop_capture_now(),
            StopOf::Recording(number) => {
                let _gate = self.capture_start_gate.locked();
                if !self.owns_recording(number) {
                    return Ok(());
                }
                self.stop_capture_now()
            }
        }
    }

    /// The stop itself; callers hold the start gate when they checked ownership (or stop for the person, or quit).
    fn stop_capture_now(self: &Shared) -> anyhow::Result<()> {
        self.stop_generation.fetch_add(1, Ordering::AcqRel);
        // A start waiting for the last session to be saved is called off too.
        self.pending_start.locked().take();
        if !self.is_capturing() {
            return Ok(());
        }
        self.update(|s| s.capture.state = CaptureState::Stopping);
        self.stopping_since.locked().get_or_insert_with(Instant::now);
        let sent = {
            let mut osc = self.osc_values.locked();
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
        if !paused && self.view().capture.live { self.require_live_camera()?; }
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
            log_info!("settings from a window: {} left out", left_out.join(", "));
        }
    }

    /// Applies `f` to the settings, publishes them and queues them to be written. `echo`: a changed walk boost
    /// strength goes to the avatar's Walk boost radial (not when that radial set it: the contract has only the
    /// slider's changes sent, and the echo would fight a radial still moving).
    fn apply_settings(self: &Shared, f: impl FnOnce(&mut Settings), echo: bool) {
        let (revision, settings) = {
            let mut s = self.snapshot.locked();
            let previous = s.settings.clone();
            // A copy: whole or not at all, should `f` panic halfway.
            let mut next = previous.clone();
            f(&mut next);
            if next == previous {
                return;
            }
            s.settings = next;
            if previous.booster.level != s.settings.booster.level && s.settings.booster.level == orbiters_booster::Level::Off {
                self.booster.cancel_work();
            }
            s.storage.limit_gb = s.settings.storage_limit_gb;
            // Numbered under the lock that made the change: two changes on two threads can reach the writer in
            // either order, and the higher number is the one that holds both.
            let revision = self.settings_revision.fetch_add(1, Ordering::SeqCst) + 1;
            // With the change itself: which server this process talks to is never another than the settings say.
            if previous.development != s.settings.development {
                use_environment(s.settings.development);
            }
            self.settings_changes.locked().waiting.push_back(SettingsChange { previous, settings: s.settings.clone(), echo });
            (revision, s.settings.clone())
        };
        self.publish();
        self.queue_save(revision, (*settings).clone());
        self.act_on_settings();
    }

    /// Acts on the changes of the settings waiting, in the order they were made: two on two threads never leave what runs as
    /// the older one had it. One made while another thread acts (or by what acting does) is acted on by that one, after it.
    fn act_on_settings(self: &Shared) {
        struct Acting<'a>(&'a Mutex<SettingsChanges>);
        impl Drop for Acting<'_> {
            fn drop(&mut self) {
                self.0.locked().acting = false;
            }
        }
        loop {
            let change = {
                let mut changes = self.settings_changes.locked();
                if changes.acting {
                    return;
                }
                let Some(change) = changes.waiting.pop_front() else { return };
                changes.acting = true;
                change
            };
            let _acting = Acting(&self.settings_changes);
            #[cfg(test)]
            let (look, changed) = (self.test_settings_acted.lock().unwrap().clone(), change.settings.clone());
            #[cfg(test)]
            look.iter().for_each(|look| look(&changed, false));
            self.settings_changed(change);
            #[cfg(test)]
            look.iter().for_each(|look| look(&changed, true));
        }
    }

    /// What a change of the settings does to what runs.
    fn settings_changed(self: &Shared, SettingsChange { previous, settings, echo }: SettingsChange) {
        if echo && previous.walk_boost.strength != settings.walk_boost.strength {
            self.n4n0.send_parameter(orbiters_contracts::n4n0::params::WALK_BOOST, rosc::OscType::Float(settings.walk_boost.strength));
        }
        if previous.language != settings.language {
            crate::language::follow(&settings);
        }
        if previous.world_performance && !settings.world_performance {
            self.performance_forget();
        }
        if previous.reconstruct_while_playing != settings.reconstruct_while_playing {
            // One running where the setting no longer has it pauses; those held while VRChat runs may start now.
            self.follow_while_playing();
            self.next_reconstruction();
        }
        if previous.development != settings.development {
            // The account thread reads the other server's account within half a second; the updates look there now.
            // Signed out until it is, with nothing of the last one's credential left to send anywhere.
            self.account_environment_changed();
            // What was read of worlds' feedback is the other environment's.
            self.world_feedback_environment_changed();
            self.update(|s| s.update.state = crate::state::UpdateState::Idle);
            self.update_action(crate::ipc::UpdateAction::Check);
        }
        let (osc_changed, sessions_changed) = (previous.osc != settings.osc, previous.sessions_dir != settings.sessions_dir);
        if osc_changed || sessions_changed {
            let ctl = self.clone();
            std::thread::spawn(move || {
                if osc_changed {
                    ctl.start_osc();
                }
                if sessions_changed {
                    // Upload records of the captures that moved with the library (CTL2-11 k).
                    shares::follow_moved_library(orbiters_core::account::Server::current(), &ctl.with_settings(|s| s.sessions_dir.clone()));
                    refresh_storage(&ctl);
                    ctl.bump_library();
                }
            });
        }
    }

    /// Hands the settings to the writing thread (started on first use). One writer, so saves never race; it keeps
    /// the newest of a burst (by `revision`, not by arrival) and writes it once nothing changed for [`SAVE_QUIET`].
    fn queue_save(self: &Shared, revision: u64, settings: Settings) {
        let mut writer = self.settings_writer.locked();
        match &*writer {
            SettingsWriter::Idle => {
                let (tx, rx) = mpsc::channel::<(u64, Settings)>();
                let (mut write, written) = (self.settings_sink(), self.settings_written.clone());
                let thread = std::thread::Builder::new()
                    .name("settings-writer".into())
                    .spawn(move || write_settings_in_order(rx, SAVE_QUIET, &written, &mut write));
                match thread {
                    Ok(thread) => {
                        let _ = tx.send((revision, settings));
                        *writer = SettingsWriter::Running(tx, thread);
                    }
                    Err(e) => log_warn!("settings writer: {e}", e = e),
                }
            }
            SettingsWriter::Running(tx, _) => {
                let _ = tx.send((revision, settings));
            }
            // A change applied after the last flush (a link still talking while the app quits): written here and now,
            // under this lock, and only when it is newer than what is on disk. A second writer started then began from
            // nothing and could write an older copy over the flushed one (verify-controller, CTL-1).
            SettingsWriter::Closed => {
                if revision > self.settings_written.load(Ordering::SeqCst) && (self.settings_sink())(settings) {
                    self.settings_written.store(revision, Ordering::SeqCst);
                }
            }
        }
    }

    /// Where settings are written: the settings file, a notice when that fails (`false`). Tests keep them in memory.
    fn settings_sink(self: &Shared) -> impl FnMut(Settings) -> bool + Send + 'static {
        #[cfg(test)]
        {
            let (writes, failing) = (self.test_settings_writes.clone(), self.test_settings_failing.clone());
            move |s: Settings| {
                // A test can make the next writes fail.
                if failing.load(Ordering::SeqCst) > 0 {
                    failing.fetch_sub(1, Ordering::SeqCst);
                    return false;
                }
                writes.lock().unwrap().push(s);
                true
            }
        }
        #[cfg(not(test))]
        {
            let ctl = Arc::downgrade(self);
            move |s: Settings| match s.save() {
                Ok(()) => true,
                Err(e) => {
                    if let Some(ctl) = ctl.upgrade() {
                        ctl.notify(NoticeKind::Error, tr!("Could not save the settings: {error}", error = e));
                    }
                    false
                }
            }
        }
    }

    /// Writes the settings still waiting, and stops the writing thread (the app quits). The slot stays closed, held
    /// through the join: nothing writes beside the last flush, and what comes after it is written by `queue_save`.
    fn flush_settings(self: &Shared) {
        let mut writer = self.settings_writer.locked();
        if let SettingsWriter::Running(tx, thread) = std::mem::replace(&mut *writer, SettingsWriter::Closed) {
            drop(tx);
            let _ = thread.join();
        }
        // What was applied may never have reached the writer (a change stalled between applying and queueing), or its
        // write may have failed: the newest applied settings are written here, tried a few times, and a failure is
        // said (verify2-controller). Revision and settings read together under the snapshot's lock, where both change.
        let (revision, settings) = {
            let s = self.snapshot.locked();
            (self.settings_revision.load(Ordering::SeqCst), (*s.settings).clone())
        };
        if revision <= self.settings_written.load(Ordering::SeqCst) {
            return;
        }
        let mut sink = self.settings_sink();
        for attempt in 0..SAVE_TRIES {
            if sink(settings.clone()) {
                self.settings_written.store(revision, Ordering::SeqCst);
                return;
            }
            if attempt + 1 < SAVE_TRIES {
                std::thread::sleep(SAVE_RETRY);
            }
        }
        // The sink told the person each time; the log keeps it too.
        log_warn!("settings: revision {revision} could not be written before quitting", revision = revision);
    }

    /// How Fast walk carries the person now, for the windows.
    pub fn set_walk_path(self: &Shared, path: crate::state::WalkPath) {
        self.update(|s| s.steamvr.walk_path = Some(path));
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
        let _one = self.osc_starting.locked();
        self.start_osc_with(true);
    }

    /// Runs `f` with the OSC service when it runs (tests point it at a stand-in VRChat).
    pub fn with_osc<R>(&self, f: impl FnOnce(&OscService) -> R) -> Option<R> {
        self.osc.locked().as_ref().map(f)
    }

    /// Makes VRChat find the OSC service again (the Reconnect button, or VRChat quiet for a while): without it VRChat
    /// sends no avatar parameter, so nothing on the avatar reaches N4N0 (Talk, a hand on N4N0, N4N0's screen).
    pub fn osc_rediscover(&self) {
        self.with_osc(|o| o.rediscover());
        self.lan.look_again();
    }

    /// Starts OSC; without `advertise` nothing listens beyond the loopback interface (tests).
    pub fn start_osc_with(self: &Shared, advertise: bool) {
        // The service before goes outside the lock: stopping it waits for its receiving thread, which may be calling in.
        let before = self.osc.locked().take();
        drop(before);
        // No VRChat is asked any more over the service that went: a camera answer of it is no one's now.
        self.camera_route_changed();
        let (enabled, vrchat_port) = self.with_settings(|s| (s.osc.enabled, s.osc.vrchat_port));
        if !enabled {
            self.update(|s| *s.osc = state::OscStatus { enabled: false, ..Default::default() });
            return;
        }
        // Where VRChat runs elsewhere, the network's keeper starts it, once it knows on which of this computer's networks
        // (`lan`); a change of settings reaches it there.
        if !orbiters_core::osc::VRCHAT_RUNS_HERE && advertise {
            self.lan.settings_changed();
            return;
        }
        let network = Network::Loopback { vrchat_port };
        let discovery = if advertise { orbiters_core::osc::Discovery::Mdns } else { orbiters_core::osc::Discovery::Off };
        self.start_osc_on(network, discovery, orbiters_core::osc::Ports::default());
    }

    /// Starts OSC for the VRChat `network` says, replacing any that runs.
    fn apply_camera_seed(&self, generation:u64, route: u64, open:bool) -> bool {
        self.update_if(|s| {
            // The OSC start's seed: for its connection, and for the VRChat it asked (a followed VRChat may change
            // within one connection: verify-controller, C-8).
            if self.osc_generation.load(Ordering::Acquire)!=generation || self.camera_route.load(Ordering::Acquire) != route || s.camera_open_said.is_some() { return false; }
            s.camera_open_said=Some(open);
            true
        })
    }

    fn start_osc_on(self: &Shared, network: Network, discovery: orbiters_core::osc::Discovery, ports: orbiters_core::osc::Ports) {
        let generation=self.osc_generation.fetch_add(1,Ordering::AcqRel)+1;
        self.camera_route_changed();
        self.camera_scan.locked().take();
        self.release_camera_stream(1);
        self.invalidate_kit(None);
        self.update(|s| { s.camera_tools.qr_generation+=1; s.camera_tools.qr=camera_tools::Qr::Off; s.camera_open_said = None; s.orbit.camera_panel_seen_ns = None; });
        { let mut osc=self.osc_values.locked(); osc.camera_received=None; osc.camera_closed_at=None; osc.camera_command=None; osc.camera_commanded.clear(); }
        let parameter = RECORD_PARAMETER.to_string();
        let ctl = self.clone();
        let wanted = vec![parameter.clone()];
        // Starting a recording reads VRChat's current values over HTTP: the record toggle is acted on on the action
        // thread, in order (`record_toggled`).
        let result = OscService::start_with("Orbiters - N4N0", &wanted, network, discovery, ports, move |events, at_ns| {
            ctl.on_osc_events(events, at_ns, &parameter);
        });
        match result {
            Ok(service) => {
                service.set_datagram_filter(Some(crate::bridge::sessions::datagram_filter(self.sides.clone())));
                if let Some(lent) = self.sides.lent() {
                    service.lend(lent.query.map(|q| (std::net::SocketAddr::from(([127, 0, 0, 1], lent.osc_in)), q)));
                }
                let (port, adv) = (service.osc_port, service.advertising());
                let replaced = self.osc.locked().replace(service);
                drop(replaced);
                self.update(|s| *s.osc = state::OscStatus { enabled: true, listening: true, advertising: adv, port: Some(port), ..Default::default() });
                let ctl = self.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        if ctl.osc_generation.load(Ordering::Acquire)!=generation || ctl.view().camera_open_said.is_some() { break; }
                        let route = ctl.camera_route.load(Ordering::Acquire);
                        let camera = ctl.camera_values().into_iter().find_map(|(p,v)| match v {
                            OscValue::Int(mode) if p == CAMERA_MODE => Some(mode != 0),
                            _ => None,
                        });
                        if let Some(open) = camera {
                            if ctl.apply_camera_seed(generation, route, open) {
                                break;
                            }
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                });
            }
            Err(e) => self.update(|s| *s.osc = state::OscStatus { enabled: true, error: Some(e.to_string()), ..Default::default() }),
        }
    }

    pub fn worker_log(&self) -> Vec<String> {
        self.worker_log.locked().iter().cloned().collect()
    }

    /// Stops OSC and the workers. A running capture starts being saved first (its last OSC values with it), while
    /// the rest closes, and the worker gets the time saving takes. VRChat's camera and the kit go back while OSC can
    /// still tell VRChat; OSC then stops before the worker leaves, so VRChat's echo of the record toggle starts or
    /// stops nothing, and nothing queued starts once the capture or the running reconstruction ends.
    pub fn shutdown(self: &Shared) {
        self.osc_generation.fetch_add(1,Ordering::AcqRel);
        self.stop_bridge();
        self.pending_start.locked().take();
        self.change_queue(VecDeque::clear);
        if let Err(e) = self.stop_capture_now() {
            log_warn!("stopping the capture: {e:#}", e = e);
        }
        self.quitting.store(true, Ordering::Relaxed);
        self.rests.wake_all();
        // N4N0's ears and its thread stop, and its screen is closed while OSC can still tell VRChat.
        self.n4n0.shutdown();
        // The friends list as it is, for the next start.
        self.kept_save(true);
        self.calls_stop();
        self.stop_avatar_gun();
        self.put_vrchat_back();
        // Stopped outside the lock: stopping waits for the receiving thread.
        let osc = self.osc.locked().take();
        drop(osc);
        let worker = self.worker.locked().take();
        if let Some(worker) = worker {
            worker.quit();
        }
        // A run under way keeps what it has for the next time the app opens (Controller::go_on_with_paused). Past the
        // limit it is killed: the file it was writing goes, whole or not at all.
        if self.pause_reconstruction() {
            let until = Instant::now() + QUIT_PAUSE_LIMIT;
            while self.reconstruction.locked().is_some() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        *self.reconstruction.locked() = None;
        self.flush_settings();
    }

    /// Sends a session's splats to the Unity editor that runs Mixer (the most recently loaded one when several do).
    pub fn send_to_mixer(&self, dir: PathBuf, title: String) -> anyhow::Result<()> {
        if bundle::read_manifest(&dir).is_err() || orbiters_core::reconstruction::read_report(&dir).is_none() {
            anyhow::bail!("{}", tr("This session has no reconstruction to send."));
        }
        let editors = crate::mixer_inbox::endpoints(&orbiters_core::paths::mixer_inbox_dir(), crate::mixer_inbox::process_alive);
        let Some(editor) = editors.first() else {
            anyhow::bail!("{}", tr("Open your Unity project with Mixer to send the splats to it."));
        };
        // Mixer reads the PLY itself: an edited capture goes as a copy holding only the splats its 3D view keeps.
        let sent = crate::mixer_inbox::session_for_mixer(&dir, &crate::mixer_inbox::exports_dir())?;
        crate::mixer_inbox::send_splats(editor, &sent, &title)?;
        let project = if editor.project_name.is_empty() { "Unity".to_string() } else { editor.project_name.clone() };
        let text = if sent != dir {
            tr!("Sent to Mixer in {project}, without the splats hidden or deleted in its 3D view: the splats appear in its open scene.", project = project)
        } else {
            tr!("Sent to Mixer in {project}: the splats appear in its open scene.", project = project)
        };
        self.notify(NoticeKind::Success, text);
        Ok(())
    }

    /// Trains splats from a session: now when nothing runs and nothing records, else once its turn comes.
    pub fn reconstruct(self: &Shared, dir: PathBuf) -> anyhow::Result<()> {
        // Splat edits name splats of the result they were made on, so a finished run replaces them with it; every press
        // that can lose them asks first (pages::reconstruct_again).
        if self.queue_reconstruction(dir)? {
            self.notify(NoticeKind::Info, tr!("Waiting its turn: the reconstruction starts {when}.", when = tr(self.reconstruction_waits_for())));
        }
        Ok(())
    }

    /// What a waiting reconstruction waits for, as the end of "it starts …".
    pub(crate) fn reconstruction_waits_for(&self) -> &'static str {
        if self.is_capturing() {
            mark("after the capture")
        } else if self.reconstruction.locked().is_some() {
            mark("after the ones before it")
        } else {
            mark("once VRChat closes or the headset has been off a minute")
        }
    }

    /// Reconstructions wait for VRChat to close (settings' `reconstruct_while_playing`), or for a break: the headset off
    /// for a minute.
    fn held_for_vrchat(&self) -> bool {
        if self.bridge_holds_reconstructions.load(Ordering::Relaxed) {
            return true;
        }
        // The FPS booster's step 2. Headset off for a minute is a break here too: nothing has to wait through it.
        if self.booster.holds_reconstructions() && !self.headset_on_break() {
            return true;
        }
        self.settings().reconstruct_while_playing == WhilePlaying::AfterVrchat && self.snapshot.locked().vrchat.runs_here() && !self.headset_on_break()
    }

    /// The person's own VRChat runs on this Mac, through the engine (elsewhere its process is seen).
    #[cfg(not(target_os = "macos"))]
    pub(crate) fn own_game(&self) -> bool {
        false
    }

    /// Whether VRChat runs on this very Mac, through the engine: what looks at the Mac now and then does so less often beside it
    /// (its CPU, its GPU and N4N0 share one budget of power and heat, docs/mac-performance.md).
    pub(crate) fn game_on_this_mac(&self) -> bool {
        !orbiters_core::osc::VRCHAT_RUNS_HERE && matches!(self.snapshot.locked().launcher.game, orbiters_core::launcher::Game::Running(_))
    }

    /// VRChat started or closed. A run goes where the setting has it beside VRChat (pausing, and going on there) and those
    /// held for VRChat start once it closes.
    pub(crate) fn vrchat_changed(self: &Shared, running: bool) {
        // Preparing uploads in the background goes on beside VRChat only while SteamVR's frame timing paces it.
        if running && !self.snapshot.locked().steamvr.running {
            crate::upload_prep::pause();
        }
        self.follow_while_playing();
        if !running {
            self.next_reconstruction();
        }
    }

    /// A run under way goes where settings' `reconstruct_while_playing` has it as things stand: pausing costs nothing now,
    /// so it never stays on a GPU that is not its to use. Held for VRChat, it pauses until VRChat closes or the next
    /// break, whenever it started; on the wrong GPU, it goes on on the right one (the worker's state fits both).
    fn follow_while_playing(self: &Shared) {
        let Some(integrated) = self.reconstruction.locked().as_ref().filter(|r| !r.told).map(|r| r.integrated) else { return };
        let playing_there = self.settings().reconstruct_while_playing == WhilePlaying::IntegratedGpu && self.snapshot.locked().vrchat.running;
        let says = if self.held_for_vrchat() {
            Some(tr("VRChat is running: the reconstruction is paused. It continues from where it was once VRChat closes or the headset has been off a minute."))
        } else if integrated && !playing_there {
            Some(tr("The reconstruction continues from where it was on the graphics card."))
        } else if !integrated && playing_there {
            Some(tr("VRChat started: the reconstruction continues from where it was on the integrated GPU."))
        } else {
            None
        };
        if let Some(says) = says {
            if self.pause_reconstruction() {
                self.notify(NoticeKind::Info, says);
            }
        }
    }

    /// The latest lines the workers logged.
    pub(crate) fn worker_log_lines(&self) -> Vec<String> {
        self.worker_log()
    }

    /// A capture is starting. Training takes the whole GPU: the game and the recording come first; the run goes on once they
    /// are done (paused once the state says capturing, so the worker's exit does not start it again). The capture waits for
    /// none of this: the worker is asked, and frees the GPU in about a third of a second on its own.
    pub(crate) fn capture_takes_the_gpu(&self) {
        crate::upload_prep::pause();
        if self.pause_reconstruction() {
            self.notify(NoticeKind::Info, tr("Reconstruction paused: a capture started. It continues from where it was when the capture ends."));
        }
    }

    /// Asks the running reconstruction to pause: it leaves the GPU and writes where it stands
    /// (orbiters_reconstruct::checkpoint), its job stays on show with its progress, and it is the first in line. Whether
    /// this paused one (not one already pausing, or one whose end was told).
    pub(crate) fn pause_reconstruction(&self) -> bool {
        let dir = {
            let mut guard = self.reconstruction.locked();
            let Some(r) = guard.as_mut().filter(|r| !r.told) else { return false };
            r.told = true;
            r.pausing = Some(Instant::now());
            let dir = r.dir.clone();
            self.change_queue(|q| {
                q.retain(|d| *d != dir);
                q.push_front(dir.clone());
            });
            if let Err(e) = r.handle.pause() {
                // It has gone or is going by itself: its exit keeps it first in line.
                log_warn!("could not ask the reconstruction to pause: {e}", e = e);
                r.handle.cancel();
            }
            dir
        };
        self.update(|s| {
            if let Some(j) = s.reconstruction.as_mut().filter(|j| j.session_dir == dir) {
                j.floor = crate::pages::sessions::overall_progress(j);
                j.paused = true;
                j.resuming = false;
            }
        });
        true
    }

    /// A worker that was asked to pause and stays on the GPU is stuck: it goes, and its session goes on from its last
    /// checkpoint. Called by the watchers every second.
    fn check_pausing(&self) {
        let mut guard = self.reconstruction.locked();
        if let Some(r) = guard.as_mut().filter(|r| !r.freed && r.pausing.is_some_and(|t| pause_overdue(t.elapsed(), r.seen.map(|s| s.elapsed())))) {
            log_warn!("the reconstruction did not leave the GPU in time after it was asked to pause (seen: {}): it was stopped", r.seen.is_some());
            r.freed = true;
            r.handle.cancel();
        }
    }

    /// What the app was closed on: sessions a checkpoint says were paused go on, the one that stood first on show as
    /// paused, and those whose upload files were being prepared are prepared. Walks the library: off the interface thread.
    pub(crate) fn go_on_with_paused(self: &Shared) {
        let root = self.settings().sessions_dir;
        let mut paused: Vec<(std::time::SystemTime, PathBuf, orbiters_reconstruct::checkpoint::Header)> = Vec::new();
        for session in list_sessions(&root) {
            if crate::upload_prep::has_partial(&session.dir) {
                crate::upload_prep::queue(session.dir.clone());
            }
            if let Some(header) = orbiters_reconstruct::checkpoint::peek(&session.dir) {
                let modified = std::fs::metadata(orbiters_reconstruct::checkpoint::file(&session.dir)).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                paused.push((modified, session.dir, header));
            }
        }
        // In the order they were paused.
        paused.sort_by_key(|(modified, ..)| *modified);
        let shown = paused.first().map(|(_, dir, header)| (dir.clone(), header.clone()));
        let running = self.running_reconstruction();
        self.change_queue(|q| {
            for (_, dir, _) in &paused {
                if !q.contains(dir) && running.as_ref() != Some(dir) {
                    q.push_back(dir.clone());
                }
            }
        });
        if let Some((dir, header)) = shown {
            self.update(|s| {
                if s.reconstruction.is_none() {
                    let mut job = ReconstructionJob::new(dir.clone(), false);
                    job.at(header.iteration, header.iterations, header.gaussians, header.psnr, header.seconds);
                    job.stage = if header.trained { orbiters_core::reconstruction::Stage::Saving } else { orbiters_core::reconstruction::Stage::Training };
                    job.floor = crate::pages::sessions::overall_progress(&job);
                    job.paused = true;
                    s.reconstruction = Some(job);
                }
            });
        }
        self.next_reconstruction();
    }

    /// Starts the reconstruction worker on a session (one at a time, never during a capture).
    fn start_reconstruction(self: &Shared, dir: PathBuf) -> anyhow::Result<()> {
        let root = self.settings().sessions_dir;
        if !inside(&root, &dir) || !dir.join("manifest.json").is_file() {
            anyhow::bail!("{}", tr("Only sessions inside the sessions folder can be reconstructed."));
        }
        if orbiters_core::bundle::read_manifest(&dir).is_ok_and(|m| m.stats.frames > 0 && m.stats.keyframes == 0) {
            anyhow::bail!("{}", tr("This recording has no keyframes: its frames came without the capture strip, so there is nothing to reconstruct."));
        }
        self.ensure_frames_kept(&dir)?;
        if self.is_capturing() {
            anyhow::bail!("{}", tr("Finish the capture first: reconstruction takes the whole GPU."));
        }
        let integrated = self.settings().reconstruct_while_playing == WhilePlaying::IntegratedGpu && self.snapshot.locked().vrchat.runs_here();
        let mut guard = self.reconstruction.locked();
        if guard.is_some() {
            anyhow::bail!("{}", tr("A reconstruction is already running."));
        }
        crate::upload_prep::forget(&dir);
        crate::upload_prep::pause();
        let on_event = {
            let ctl = self.clone();
            move |e| ctl.on_reconstruct_event(e)
        };
        let on_log = {
            let ctl = self.clone();
            move |line: String| {
                let mut log = ctl.worker_log.locked();
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
        // A paused run keeps what it shows until its worker says otherwise.
        self.update(|s| match s.reconstruction.as_mut().filter(|j| j.session_dir == dir && j.paused) {
            Some(j) => {
                j.paused = false;
                j.resuming = true;
                j.integrated = integrated;
            }
            None => s.reconstruction = Some(ReconstructionJob::new(dir.clone(), integrated)),
        });
        match ReconstructHandle::spawn(&dir, integrated, on_event, on_log, on_exit) {
            Ok(handle) => {
                *guard = Some(Reconstruction { handle, dir, told: false, integrated, pausing: None, seen: None, freed: false, stopped: false });
                Ok(())
            }
            Err(e) => {
                self.update(|s| s.reconstruction = None);
                Err(e)
            }
        }
    }

    /// The session being reconstructed.
    pub(crate) fn running_reconstruction(&self) -> Option<PathBuf> {
        self.reconstruction.locked().as_ref().map(|r| r.dir.clone())
    }

    /// Reconstructs `dir` after the sessions already waiting (now when nothing runs and nothing records). Whether it
    /// has to wait.
    pub fn queue_reconstruction(self: &Shared, dir: PathBuf) -> anyhow::Result<bool> {
        let root = self.settings().sessions_dir;
        if !inside(&root, &dir) || !dir.join("manifest.json").is_file() {
            anyhow::bail!("{}", tr("Only sessions inside the sessions folder can be reconstructed."));
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

    /// Takes a session off the waiting list; what a pause kept of it goes too: it was asked not to go on.
    pub fn unqueue_reconstruction(self: &Shared, dir: &Path) {
        self.change_queue(|q| q.retain(|d| d != dir));
        self.forget_paused(dir);
    }

    /// A paused run of `dir` is over: no job on show, no checkpoint.
    fn forget_paused(self: &Shared, dir: &Path) {
        self.update(|s| {
            if s.reconstruction.as_ref().is_some_and(|j| j.session_dir == dir && j.paused) {
                s.reconstruction = None;
            }
        });
        if self.running_reconstruction().as_deref() != Some(dir) {
            orbiters_reconstruct::checkpoint::discard(dir);
        }
    }

    /// Changes the waiting list and shows it.
    fn change_queue<R>(&self, f: impl FnOnce(&mut VecDeque<PathBuf>) -> R) -> R {
        let mut queue = self.reconstruct_queue.locked();
        let r = f(&mut queue);
        let waiting: Vec<PathBuf> = queue.iter().cloned().collect();
        drop(queue);
        self.update(|s| {
            s.reconstruct_queue.set(waiting);
        });
        r
    }

    /// Starts the first waiting session when nothing runs, nothing records and VRChat does not hold them.
    pub(crate) fn next_reconstruction(self: &Shared) {
        loop {
            if self.quitting.load(Ordering::Relaxed) || self.is_capturing() || self.reconstruction.locked().is_some() || self.held_for_vrchat() {
                return;
            }
            let Some(dir) = self.change_queue(VecDeque::pop_front) else {
                self.next_upload_prep();
                return;
            };
            match self.start_reconstruction(dir.clone()) {
                Ok(()) => return,
                Err(e) => {
                    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.notify(NoticeKind::Error, tr!("{name} was not reconstructed: {error}", name = name, error = e));
                    self.forget_paused(&dir);
                }
            }
        }
    }

    /// Stops the reconstruction on show, running or paused; the session keeps its previous result, and what the run had
    /// done goes (stopping still means stopping). Returns whether there was one.
    pub fn stop_reconstruction(self: &Shared) -> bool {
        {
            let mut guard = self.reconstruction.locked();
            if let Some(r) = guard.as_mut() {
                r.told = true;
                r.stopped = true;
                r.pausing = None;
                let dir = r.dir.clone();
                // Its exit deletes what it leaves, once nothing is writing it.
                self.change_queue(|q| q.retain(|d| *d != dir));
                r.handle.cancel();
                return true;
            }
        }
        let paused = self.snapshot.locked().reconstruction.as_ref().filter(|j| j.paused).map(|j| j.session_dir.clone());
        let Some(dir) = paused else { return false };
        self.unqueue_reconstruction(&dir);
        self.next_reconstruction();
        true
    }

    fn on_reconstruct_event(self: &Shared, e: ReconstructEvent) {
        match e {
            ReconstructEvent::Stage { stage, progress } => {
                let mut over = false;
                self.update(|s| {
                    if let Some(j) = s.reconstruction.as_mut() {
                        // Seeding only happens from the start: what it was to go on from could not be used.
                        if stage == orbiters_core::reconstruction::Stage::Seeding && j.floor > 0.0 {
                            over = j.iteration > 0;
                            j.floor = 0.0;
                            j.at(0, j.iterations, 0, None, 0.0);
                            j.psnr = None;
                        }
                        j.stage = stage;
                        j.progress = progress;
                        if stage != orbiters_core::reconstruction::Stage::Loading {
                            j.resuming = false;
                        }
                    }
                });
                if over {
                    self.notify(NoticeKind::Info, tr("The paused reconstruction could not continue from where it was (N4N0 was updated, or its file was damaged): it starts over."));
                }
            }
            ReconstructEvent::Resumed { iteration, iterations, gaussians, psnr, seconds } => self.update(|s| {
                if let Some(j) = s.reconstruction.as_mut() {
                    j.at(iteration, iterations, gaussians, psnr, seconds);
                    let kept = 0.06 + 0.92 * j.progress;
                    j.floor = j.floor.max(kept);
                    j.resuming = true;
                }
            }),
            // It saw the pause: it has longer (bounded) to read back and give the GPU back.
            ReconstructEvent::PauseSeen => {
                if let Some(r) = self.reconstruction.locked().as_mut() {
                    r.seen = Some(Instant::now());
                }
            }
            // Off the GPU; only its file is left, however long the disk takes beside a capture's frames.
            ReconstructEvent::Pausing => {
                if let Some(r) = self.reconstruction.locked().as_mut() {
                    r.freed = true;
                }
            }
            ReconstructEvent::Paused { kept, message } => {
                if let Some(r) = self.reconstruction.locked().as_mut() {
                    r.freed = true;
                }
                if !kept {
                    // Nothing to go on from: what it shows starts over with it.
                    self.update(|s| {
                        if let Some(j) = s.reconstruction.as_mut() {
                            j.floor = 0.0;
                            j.at(0, j.iterations, 0, None, 0.0);
                        }
                    });
                }
                if let Some(message) = message {
                    let text = if kept {
                        tr!("The reconstruction's progress could not be kept ({message}). It continues from an earlier pause when it can.", message = message)
                    } else {
                        tr!("The reconstruction's progress could not be kept ({message}). It starts over when it can.", message = message)
                    };
                    self.notify(NoticeKind::Warning, text);
                }
            }
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
                self.mark_reconstruction_over();
                // Its upload files, prepared once nothing else runs: sharing it is then at once (crate::upload_prep).
                if let Some(dir) = self.running_reconstruction() {
                    // What is shared is the result this one replaced: its window offers to update it.
                    shares::reconstructed(&dir);
                    crate::upload_prep::queue(dir);
                }
                let splats = orbiters_ui::format::integer(report.gaussians as f64);
                let text = match report.psnr_held_out {
                    Some(p) => tr!("Reconstruction finished: {splats} splats, {quality} dB on views it never saw.", splats = splats, quality = format!("{p:.1}")),
                    None => tr!("Reconstruction finished: {splats} splats.", splats = splats),
                };
                self.notify(NoticeKind::Success, text);
                refresh_storage(self);
                self.bump_library();
            }
            ReconstructEvent::Error { message } => {
                if let Some(session) = self.snapshot.locked().reconstruction.as_ref().map(|job| job.session_dir.clone()) {
                    self.reconstruction_failures.locked().insert(session, message.clone());
                }
                self.mark_reconstruction_over();
                self.notify(NoticeKind::Error, tr!("Reconstruction failed: {message}", message = message));
            }
        }
    }

    /// Prepares the next session waiting for its upload files in the background (crate::upload_prep), on the integrated
    /// GPU while VRChat runs when reconstructions train there; its exit looks for the next work. Beside VRChat only while
    /// SteamVR runs: its frame timing paces the steps (orbiters_reconstruct::governor). Without it, nothing does: the
    /// preparation of a 970k-splat capture made 17 % of a stand-in game's 90 Hz frames late at low queue priority as at
    /// normal priority (docs/capture-detail-2026-10-29.md), so it waits for VRChat to close (a share prepares at once).
    fn next_upload_prep(self: &Shared) {
        if self.booster.holds_preparing() {
            return;
        }
        let (vrchat, steamvr) = {
            let s = self.snapshot.locked();
            (s.vrchat.runs_here(), s.steamvr.running)
        };
        if vrchat && !steamvr {
            return;
        }
        let integrated = self.settings().reconstruct_while_playing == WhilePlaying::IntegratedGpu && vrchat;
        let log = self.clone();
        let next = self.clone();
        let started = crate::upload_prep::start_next(
            integrated,
            move |line| {
                let mut log = log.worker_log.locked();
                log.push_back(format!("prepare upload: {line}")); // i18n: not shown
                while log.len() > 400 {
                    log.pop_front();
                }
            },
            move || next.next_reconstruction(),
        );
        if let Err(e) = started {
            log_warn!("could not start preparing an upload in the background: {e:#}", e = e);
        }
    }

    /// It finished or failed, also if a pause was asked too late for it: nothing of it waits any more.
    fn mark_reconstruction_over(&self) {
        let mut guard = self.reconstruction.locked();
        if let Some(r) = guard.as_mut() {
            r.told = true;
            r.pausing = None;
            let dir = r.dir.clone();
            self.change_queue(|q| q.retain(|d| *d != dir));
        }
    }

    fn on_reconstruct_exit(self: &Shared) {
        let ended = self.reconstruction.locked().take();
        let mut keep_on_show = false;
        if let Some(r) = ended {
            if !r.told {
                self.notify(NoticeKind::Error, tr("The reconstruction worker stopped unexpectedly."));
            }
            // Stopped, or crashed (what it would go on from may be why): the next run starts over.
            if r.stopped || !r.told {
                orbiters_reconstruct::checkpoint::discard(&r.dir);
            }
            keep_on_show = r.pausing.is_some();
        }
        if !keep_on_show {
            self.update(|s| s.reconstruction = None);
        }
        // A cancelled run may leave its partial folder behind: the next run clears it.
        self.next_reconstruction();
    }

    /// A session reconstructs from its frames: not one whose capture files were deleted, nor one being cleaned up.
    fn ensure_frames_kept(&self, dir: &Path) -> anyhow::Result<()> {
        anyhow::ensure!(!self.cleaning(dir), "{}", tr("Its capture files are being deleted."));
        anyhow::ensure!(
            !(orbiters_core::cleanup::trimmed(dir) && orbiters_core::cleanup::has_view(dir)),
            "{}",
            tr("Only its 3D view is kept: its capture files were deleted.")
        );
        Ok(())
    }

    /// Moves a session folder to the recycle bin (only folders inside the sessions folder with a manifest).
    pub fn trash_session(self: &Shared, dir: &Path) -> anyhow::Result<()> {
        let root = self.settings().sessions_dir;
        self.trash_session_in(&root, dir)
    }

    /// [`Controller::trash_session`] of a session of the library `root` (the one it was asked for: a Trash waiting behind
    /// a link's other file work is judged against the library it was sent for, not one switched to since).
    pub fn trash_session_in(self: &Shared, root: &Path, dir: &Path) -> anyhow::Result<()> {
        if !inside(root, dir) || !dir.join("manifest.json").is_file() {
            anyhow::bail!("{}", tr("Only session folders inside the sessions folder can be removed."));
        }
        anyhow::ensure!(!self.cleaning(dir), "{}", tr("This session is being cleaned up: try again once that is done."));
        // Waiting or paused, it no longer goes on.
        self.unqueue_reconstruction(dir);
        move_to_bin(dir)?;
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

/// The world a raw capture being imported shows, when its `capture.json` names one by id (contracts'
/// Docs/raw-capture.md): its session is then one of that world.
fn raw_capture_world(source: &SourceSpec) -> Option<orbiters_contracts::manifest::WorldInfo> {
    let SourceSpec::RawSequence { path, .. } = source else { return None };
    let text = std::fs::read_to_string(path.join(orbiters_contracts::raw_capture::RAW_FILE)).ok()?;
    let meta: orbiters_contracts::raw_capture::RawCapture = serde_json::from_str(&text).ok()?;
    meta.world.filter(|w| w.id.as_deref().is_some_and(|id| id.starts_with("wrld_")))
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

/// To the recycle bin or the Trash. On the Mac through the file manager API and not the `trash` crate's default, which
/// scripts the Finder (a permission prompt for an app's first file, and the Finder's sound).
fn move_to_bin(dir: &Path) -> Result<(), trash::Error> {
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        let mut bin = trash::TrashContext::default();
        bin.set_delete_method(DeleteMethod::NsFileManager);
        bin.delete(dir)
    }
    #[cfg(not(target_os = "macos"))]
    {
        trash::delete(dir)
    }
}

/// Opens a folder in the platform's file manager.
pub fn open_path(dir: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        crate::linux::open::path(dir)
    }
    #[cfg(not(target_os = "linux"))]
    {
        #[cfg(windows)]
        let mut cmd = std::process::Command::new("explorer");
        #[cfg(target_os = "macos")]
        let mut cmd = std::process::Command::new("open");
        cmd.arg(dir).spawn()?;
        Ok(())
    }
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

    /// A join is under way.
    fn waits(&self) -> bool {
        self.joining.is_some() || self.due
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
        let s = ctl.snapshot.locked();
        (s.capture.state != CaptureState::Idle).then(|| s.capture.session_dir.clone()).flatten()
    };
    let sessions: Vec<_> = sessions.into_iter().filter(|s| current.as_ref() != Some(&s.dir)).collect();
    let used: u64 = sessions.iter().map(|s| s.size_bytes).sum();
    // What keeping only their 3D views would free.
    let trimmable: Vec<u64> = sessions.iter().filter(|s| s.reconstruction.is_some() && s.capture_bytes > 0).map(|s| s.capture_bytes).collect();
    let free = free_space(&dir);
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

/// Free bytes on the disk that holds `dir`: one question to the system for that folder, not a list of every volume
/// (LIGHT-DESKTOP-9); sysinfo's list where that cannot answer (the folder does not exist yet, a Mac).
fn free_space(dir: &Path) -> Option<u64> {
    if cfg!(not(target_os = "macos")) {
        if let Some(available) = orbiters_core::disk::free_space(dir) {
            return Some(available);
        }
    }
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()) || dir.to_string_lossy().to_lowercase().starts_with(&d.mount_point().to_string_lossy().to_lowercase()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

/// Only the free space, for the 30 s turns when no session changed ([`StorageSweep`]).
fn refresh_free_space(ctl: &Shared) {
    let dir = ctl.with_settings(|s| s.sessions_dir.clone());
    let free = free_space(&dir);
    ctl.update_if(|s| {
        let changed = s.storage.free_bytes != free;
        s.storage.free_bytes = free;
        changed
    });
}

/// When the watchers' 30 s turn walks the whole library again (every manifest, report and picture file read and parsed:
/// ~13 file calls a session) rather than only asking for the free space: the app changed a session (`bump_library`),
/// the folder or the library's own folder changed (a session added or removed by hand), a recording runs (the storage
/// limit), or ten minutes passed (LIGHT-DESKTOP-9).
#[derive(Default)]
struct StorageSweep {
    last: Option<(u64, PathBuf, u64, Instant)>,
}

/// What every session folder's size and summary depend on, in one number: the library's own time and, for each session,
/// the five times the size cache keys on (orbiters_core::bundle `stamp`: its manifest, its folder, its reconstruction
/// report, its keyframes and depth folders). One `metadata` each, no file opened or parsed: a turn with an unchanged
/// fingerprint has nothing a full walk would find, also for a session changed by hand (verify-controller).
fn library_fingerprint(dir: &Path) -> u64 {
    use orbiters_core::contracts::manifest::{DEPTH_DIR, KEYFRAMES_DIR, MANIFEST_FILE};
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let time = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    time(dir).hash(&mut hash);
    let mut sessions: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    sessions.sort();
    for session in &sessions {
        session.hash(&mut hash);
        let report = orbiters_core::reconstruction::result_dir(session).join(orbiters_core::reconstruction::REPORT_FILE);
        for part in [session.join(MANIFEST_FILE), session.clone(), report, session.join(KEYFRAMES_DIR), session.join(DEPTH_DIR)] {
            time(&part).hash(&mut hash);
        }
    }
    hash.finish()
}

impl StorageSweep {
    const AT_MOST: Duration = Duration::from_secs(600);

    fn due(&mut self, library: u64, dir: &Path, modified: u64, recording: bool, now: Instant) -> bool {
        let due = recording
            || self.last.as_ref().is_none_or(|(v, d, m, at)| *v != library || d != dir || *m != modified || now.duration_since(*at) >= Self::AT_MOST);
        if due {
            self.last = Some((library, dir.to_path_buf(), modified, now));
        }
        due
    }
}

/// How long after VRChat closed its log is still followed as while it ran.
const SETTLED_AFTER_VRCHAT: Duration = Duration::from_secs(30);

fn spawn_watchers(ctl: Shared) {
    std::thread::Builder::new()
        .name("watchers".into())
        .spawn(move || {
            // The bridge's side sessions' logs are theirs: the app follows its target's log only.
            let mut log = vrchats::TargetLog::new(None, ctl.sides.clone());
            // Every VRChat running, and which the app follows (controller/vrchats.rs).
            let mut roster = vrchats::Roster::seeing(ctl.launcher_machine_now().vrchats_seen().unwrap_or_default());
            #[cfg(target_os = "macos")]
            let (mut own_game, library) = (mac_game::OwnGame::default(), orbiters_core::paths::roots().is_none().then(|| orbiters_core::obelisk::library(&orbiters_core::obelisk::home())));
            let mut split = WorldSplit::default();
            // The worlds visited, kept from VRChat's logs before VRChat deletes them (the Visited worlds page).
            let history_file = visited::history_file();
            let mut history = visited::History::load(&history_file);
            let mut history_changed = false;
            // VRChat closed: how far its log was read is saved at the next beat, not minutes later.
            let mut history_flush = false;
            // The videos the worlds played, the same way (the Videos page).
            let mut videos = videos::Videos::load();
            let mut tick = 0u64;
            let mut storage_sweep = StorageSweep::default();
            let mut rate = PublishRate::default();
            let mut running = false;
            // Since when VRChat runs, and when VRChat was last asked to find the OSC service.
            let mut vrchat_since: Option<Instant> = None;
            let mut osc_asked: Option<Instant> = None;
            // When VRChat was last seen running (its log's last lines come after it).
            let mut vrchat_gone: Option<Instant> = None;
            let rest = ctl.rest("watchers", |s| s.capture.state);
            let mut storage_looked: Option<Instant> = None;
            let mut step = 1;
            let private = orbiters_core::paths::roots().is_some();
            while !ctl.quitting.load(Ordering::Relaxed) {
                // VRChat process, every 3 s (first, so the world below knows whether VRChat runs).
                if tick.is_multiple_of(3) {
                    // One Windows inventory gives both process names and the full birth time.
                    // Linux's /proc inventory counts Wine processes without their similarly named threads.
                    #[cfg(windows)]
                    let system = if cfg!(test) { Vec::new() } else { orbiters_booster::programs::scan() };
                    #[cfg(target_os = "linux")]
                    let system = if cfg!(test) { Vec::new() } else { orbiters_core::processes::all() };
                    #[cfg(any(windows, target_os = "linux"))]
                    let steamvr_pid = system.iter().find(|p| p.name.eq_ignore_ascii_case("vrserver.exe") || p.name.eq_ignore_ascii_case("vrserver")).map(|p| p.pid);
                    #[cfg(any(windows, target_os = "linux"))]
                    let started = bridge_side::vrchat_births(&system);
                    // Home follows its own engine's records on a Mac.
                    #[cfg(not(any(windows, target_os = "linux")))]
                    let (steamvr_pid, started) = (None::<u32>, Vec::<(u32, u64)>::new());
                    // A game that starts during a private harness run belongs to the person.
                    // Still measure the inventory, while keeping its results out of the harness.
                    let (steamvr_pid, started) = if private { (None, Vec::new()) } else { (steamvr_pid, started) };
                    let vrchats: Vec<u32> = started.iter().map(|(p, _)| *p).collect();
                    if orbiters_core::osc::VRCHAT_RUNS_HERE {
                        let dir = orbiters_core::paths::vrchat_log_dir();
                        let runs = roster.scan(&started, dir.as_deref(), |query| {
                            orbiters_core::osc::vrchat_camera_values(query, Duration::from_millis(300)).into_iter().find_map(|(p, v)| match v {
                                OscValue::Int(mode) if p == CAMERA_MODE => Some(mode != 0),
                                _ => None,
                            })
                        });
                        ctl.sides.set_roster(runs);
                        ctl.follow_target();
                        ctl.publish_runs();
                    }
                    let pid = ctl.sides.app_pid(&vrchats);
                    // The developer VR sandbox's SteamVR (a bridge VR session's emulated headset) is not the person's VR: no
                    // overlay, sound devices or start choice for it (steamvr_start::sandbox_runs).
                    let steamvr_pid = steamvr_pid.filter(|_| !crate::steamvr_start::sandbox_runs() || crate::steamvr_start::sandbox_test_app());
                    let steamvr = steamvr_pid.is_some();
                    let was = running;
                    // A VRChat on the network counts as running, for N4N0 and the interface; `remote` says it costs
                    // this computer nothing.
                    let own = ctl.own_game();
                    let remote = pid.is_none() && !own && ctl.lan.alive();
                    running = pid.is_some() || own || remote;
                    vrchat_since = if running { vrchat_since.or(Some(Instant::now())) } else { None };
                    let mut started = None;
                    ctl.update_if(|s| {
                        let c = s.vrchat.running != running
                            || s.vrchat.remote != remote
                            || s.vrchat.pid != pid
                            || s.steamvr.running != steamvr
                            || s.steamvr.pid != steamvr_pid;
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
                        s.vrchat.remote = remote;
                        s.vrchat.pid = pid;
                        s.steamvr.running = steamvr;
                        s.steamvr.pid = steamvr_pid;
                        c
                    });
                    if let Some(pid) = started {
                        let _ = ctl.send_if_running(&WorkerCommand::GameAudio { pid });
                    }
                    if was != running && !remote {
                        ctl.vrchat_changed(running);
                        if !running {
                            history_flush = true;
                            videos.flush();
                        }
                    }
                    // A minute into a break from the headset, what waited for VRChat to close starts.
                    if running && ctl.headset_on_break() {
                        ctl.next_reconstruction();
                    }
                    // Microphones come and go (headsets): ask the worker again now and then.
                    if !private && tick.is_multiple_of(15) && tick > 0 {
                        ctl.send(&WorkerCommand::ListSources).or_log("what a worker was asked did not reach it");
                    }
                }

                if running {
                    vrchat_gone = Some(Instant::now());
                }
                let looked_at = ctl.shown() || running || ctl.view().capture.state != CaptureState::Idle || ctl.stopping_since.locked().is_some();

                // VRChat log: world changes (and new segments when recording). The last log keeps the last world
                // after VRChat closes; only a running VRChat is in a world. Until the log is read to its end, the world
                // it names may be one left long ago: it is neither shown nor given to a session.
                let events = log.poll();
                if let Some((name, from, to)) = log.last_read() {
                    history_changed |= history.follow(name, from, to, &events);
                }
                videos.step(log.last_read(), &events, tick.is_multiple_of(10));
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
                        s.vrchat.world.set(world);
                    }
                    s.vrchat.log_found = log_found;
                    changed
                });
                split.read(&events, log.live(), Instant::now());
                #[cfg(target_os = "macos")]
                if let Some(library) = &library {
                    let (events, live) = own_game.turn(&ctl, library, tick);
                    split.read(&events, live, Instant::now());
                }
                // A world joined: the avatar's default state goes back once it loaded there.
                ctl.n4n0.states_log(&events, log.live());
                let (state, split_session) = {
                    let s = ctl.snapshot.locked();
                    (s.capture.state, s.settings.split_on_world_change)
                };
                let paused = state == CaptureState::Paused;
                if split.take(state == CaptureState::Recording || paused, Instant::now()) {
                    if split_session {
                        let source = ctl.last_source.locked().clone();
                        if let Some(source) = source {
                            let from_osc = ctl.snapshot.locked().capture.from_osc;
                            match ctl.start_segment(source, false, from_osc, ctl.stop_generation.load(Ordering::Acquire), true) {
                                Ok(()) => {
                                    // A paused recording stays paused in the new world: its Pause follows the Start.
                                    if paused {
                                        let _ = ctl.pause_capture(true);
                                    }
                                    let told = if paused { tr("New world: a new session, paused like the last one.") } else { tr("New world: started a new session.") };
                                    ctl.notify(NoticeKind::Info, told);
                                }
                                Err(e) => ctl.notify(NoticeKind::Error, tr!("New world: the new session did not start: {error}", error = e)),
                            }
                        }
                    } else {
                        ctl.send(&WorkerCommand::SetWorld { world: ctl.current_world() }).or_log("what a worker was asked did not reach it");
                    }
                }
                ctl.check_stopping();
                ctl.check_pausing();

                // Spout senders, every 2 s while something could use or show one: on a Mac, those of the VRChat Home started, through
                // its engine.
                #[cfg(target_os = "macos")]
                ctl.follow_camera_host();
                #[cfg(any(windows, target_os = "macos"))]
                if tick.is_multiple_of(2) && looked_at {
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
                    ctl.update_if(|s| s.sources.set(sources));
                }

                // OSC: the last values of a quiet second reach the worker, and the status follows VRChat.
                {
                    let mut osc = ctl.osc_values.locked();
                    if osc.forwarding {
                        ctl.flush_osc(&mut osc);
                    }
                    let values = osc.latest.len();
                    drop(osc);
                    let (found, received) = ctl.osc.locked().as_ref().map_or((false, None), |o| (o.vrchat_query().is_some(), o.last_received_ns()));
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
                    if running && !receiving && vrchat_since.is_some_and(|t| t.elapsed() > OSC_QUIET) && osc_asked.is_none_or(|t| t.elapsed() > OSC_QUIET) {
                        osc_asked = Some(Instant::now());
                        ctl.osc_rediscover();
                    }
                }

                // Visited worlds, every 10 s: the older logs read here, the newest as the follower reads it.
                if tick.is_multiple_of(10) {
                    if let Some(dir) = orbiters_core::paths::vrchat_log_dir() {
                        let following = log.last_read().map(|(name, _, to)| (name, to));
                        history_changed |= history.read_logs_following(&dir, following);
                        // At once when a visit is new, every few minutes for how far the logs were read; a save that
                        // failed stays to do and is tried again.
                        let flush = std::mem::take(&mut history_flush);
                        if history_changed && (flush || history.save_due(true)) {
                            match history.checkpoint(&history_file) {
                                Ok(()) => history_changed = false,
                                Err(e) => log_warn!("visited worlds: {e}", e = e),
                            }
                        }
                    }
                }

                // Storage, every 30 s while it is shown or a recording fills it: the whole library when something may have changed, else
                // the free space.
                if storage_looked.is_none_or(|at| looked_at && at.elapsed() >= Duration::from_secs(29)) {
                    storage_looked = Some(Instant::now());
                    let dir = ctl.with_settings(|s| s.sessions_dir.clone());
                    let modified = library_fingerprint(&dir);
                    if storage_sweep.due(ctl.library_version(), &dir, modified, ctl.is_capturing(), Instant::now()) {
                        refresh_storage(&ctl);
                    } else {
                        refresh_free_space(&ctl);
                    }
                }
                // How often the state was published, once a minute (this loop turns once a second, and a little more).
                let readers = ctl.subscribers.locked().iter().filter(|m| m.shows_notices()).count();
                if let Some(line) = rate.second(step as u32, ctl.publishes(), readers) {
                    publish_log(&line);
                }
                // Nobody looks, no game and no recording: what is left is whether one of them starts (the next process look).
                let resting = !looked_at && !running && !split.waits() && vrchat_gone.is_none_or(|at| at.elapsed() > SETTLED_AFTER_VRCHAT);
                step = if resting { 3 - tick % 3 } else { 1 };
                tick += step;
                rest.wait(Some(Duration::from_secs(step)));
            }
        })
        .expect("watchers thread");
}

/// The settings writer's loop: waits for [`SAVE_QUIET`] of quiet after a change, then writes the newest copy it was
/// given. Newest is the highest revision, kept across writes too: a copy numbered lower than one already written (a
/// thread that applied first and queued last) is never written over it.
/// The settings writer's state: not started, running, or closed by the last flush (the app quits).
enum SettingsWriter {
    Idle,
    Running(mpsc::Sender<(u64, Settings)>, std::thread::JoinHandle<()>),
    Closed,
}

fn write_settings_in_order(rx: mpsc::Receiver<(u64, Settings)>, quiet: Duration, watermark: &AtomicU64, write: &mut impl FnMut(Settings) -> bool) {
    let mut written = watermark.load(Ordering::SeqCst);
    // A copy whose write failed waits for the next burst, unless a newer one comes (it holds the same changes).
    let mut carried: Option<(u64, Settings)> = None;
    while let Ok(first) = rx.recv() {
        let mut latest = carried.take().filter(|(r, _)| *r > written);
        let keep = |latest: &mut Option<(u64, Settings)>, (r, s): (u64, Settings)| {
            if r > latest.as_ref().map_or(written, |(l, _)| *l) {
                *latest = Some((r, s));
            }
        };
        keep(&mut latest, first);
        let mut done = false;
        loop {
            match rx.recv_timeout(quiet) {
                Ok(newer) => keep(&mut latest, newer),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    done = true;
                    break;
                }
            }
        }
        if let Some((r, s)) = latest {
            if write(s.clone()) {
                written = r;
                watermark.store(r, Ordering::SeqCst);
            } else {
                carried = Some((r, s));
            }
        }
        if done {
            // The last flush: one more try for a copy that failed.
            if let Some((r, s)) = carried.take() {
                if write(s) {
                    watermark.store(r, Ordering::SeqCst);
                }
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    /// CTL-1 / audit F10: two threads that applied A then B (B holding both changes) can queue B then A. The file
    /// must get B, whether A arrives in the same burst or after B was already written.
    #[test]
    fn the_settings_file_gets_the_newest_applied_whatever_the_queue_order() {
        use orbiters_core::settings::Settings;
        use std::sync::mpsc;
        use std::time::Duration;
        let (a, b) = {
            let mut a = Settings::default();
            a.storage_limit_gb = 1.0;
            let mut b = a.clone();
            b.storage_limit_gb = 2.0;
            (a, b)
        };
        let quiet = Duration::from_millis(40);
        // Each send, then (`true`) a wait until the writer has written: not a sleep, which a loaded machine (the
        // release gate's full parallel run) can outlast before the writer thread even runs, turning "after a write"
        // into "in one burst" (it failed so once: the code was right, the test's timing was not).
        let run = |sends: Vec<(u64, Settings, bool)>| {
            let (tx, rx) = mpsc::channel();
            let (wrote_tx, wrote) = mpsc::channel();
            let writer = std::thread::spawn(move || {
                let mut written = Vec::new();
                let watermark = std::sync::atomic::AtomicU64::new(0);
                super::write_settings_in_order(rx, quiet, &watermark, &mut |s: Settings| {
                    written.push(s.storage_limit_gb);
                    let _ = wrote_tx.send(());
                    true
                });
                written
            });
            for (r, s, wait) in sends {
                tx.send((r, s)).unwrap();
                if wait {
                    wrote.recv_timeout(Duration::from_secs(30)).expect("the writer wrote");
                }
            }
            drop(tx);
            writer.join().unwrap()
        };
        assert_eq!(run(vec![(2, b.clone(), false), (1, a.clone(), false)]), vec![2.0], "reversed in one burst");
        assert_eq!(run(vec![(2, b.clone(), true), (1, a.clone(), false)]), vec![2.0], "the older after a write");
        assert_eq!(run(vec![(1, a.clone(), true), (2, b.clone(), false)]), vec![1.0, 2.0], "in order");
    }

    /// verify-controller: the late end of a worker replaced since (its recording not the one under way) leaves the newer
    /// worker's recording running; the end of the worker the recording runs on still ends it.
    #[test]
    fn a_replaced_workers_late_end_leaves_the_new_recording_alone() {
        use std::sync::atomic::Ordering;
        let ctl = super::Controller::new(orbiters_core::settings::Settings::default());
        // Worker 1 died, worker 2 was started and records (recording 5).
        ctl.worker_installed.store(2, Ordering::Release);
        ctl.recording_worker.store(2, Ordering::Release);
        ctl.capture_serial.store(5, Ordering::Release);
        ctl.recording_worker_number.store(5, Ordering::Release);
        ctl.update(|s| s.capture.state = super::CaptureState::Recording);
        ctl.on_worker_exit(1);
        assert_eq!(ctl.view().capture.state, super::CaptureState::Recording, "worker 1's late end");
        ctl.on_worker_exit(2);
        assert_eq!(ctl.view().capture.state, super::CaptureState::Idle, "its own worker ended");
    }

    /// verify2-controller: a worker whose end was heard while it was still the one installed (nothing recording), and
    /// is acted on only after the next recording was numbered on a replacement, does not end that recording; the old
    /// recording number of a worker is not taken for the new one either.
    #[test]
    fn a_workers_late_end_never_ends_a_recording_started_since() {
        use std::sync::atomic::Ordering;
        let ctl = super::Controller::new(orbiters_core::settings::Settings::default());
        // Worker 1 ran recording 3 earlier; it is still installed and idle when it dies.
        ctl.worker_installed.store(1, Ordering::Release);
        ctl.recording_worker.store(1, Ordering::Release);
        ctl.recording_worker_number.store(3, Ordering::Release);
        ctl.capture_serial.store(3, Ordering::Release);
        // Before its end is acted on: recording 4 is numbered and Starting (its Start not sent yet).
        ctl.capture_serial.store(4, Ordering::Release);
        ctl.update(|s| s.capture.state = super::CaptureState::Starting);
        ctl.on_worker_exit(1);
        assert_eq!(ctl.view().capture.state, super::CaptureState::Starting, "recording 4 is not worker 1's");
        // Recording 4 starts on worker 2; worker 1's end, heard again, leaves it.
        ctl.worker_installed.store(2, Ordering::Release);
        ctl.recording_worker.store(2, Ordering::Release);
        ctl.recording_worker_number.store(4, Ordering::Release);
        ctl.update(|s| s.capture.state = super::CaptureState::Recording);
        ctl.on_worker_exit(1);
        assert_eq!(ctl.view().capture.state, super::CaptureState::Recording);
    }

    /// verify2-controller: at Quit, a change applied but stalled before it was queued is written by the last flush; and
    /// a write that failed in the debounce, with nothing newer after it, is tried again then (not dropped).
    #[test]
    fn quitting_writes_what_was_applied_even_if_never_queued_or_failed_once() {
        use orbiters_core::settings::Settings;
        use std::sync::atomic::Ordering;
        let ctl = super::Controller::new(Settings::default());
        let written = || ctl.test_settings_writes.lock().unwrap().iter().map(|s| s.storage_limit_gb).collect::<Vec<_>>();
        // Applied (revision 1, under the snapshot's lock, as `apply_settings` does), stalled before `queue_save`.
        {
            let mut s = ctl.snapshot.lock().unwrap();
            s.settings.storage_limit_gb = 7.0;
            ctl.settings_revision.fetch_add(1, Ordering::SeqCst);
        }
        ctl.flush_settings();
        assert_eq!(written(), vec![7.0], "the applied change is on disk");

        let ctl = super::Controller::new(Settings::default());
        let written = || ctl.test_settings_writes.lock().unwrap().iter().map(|s| s.storage_limit_gb).collect::<Vec<_>>();
        ctl.test_settings_failing.store(1, Ordering::SeqCst);
        let mut b = Settings::default();
        b.storage_limit_gb = 8.0;
        {
            let mut s = ctl.snapshot.lock().unwrap();
            s.settings.storage_limit_gb = 8.0;
            ctl.settings_revision.fetch_add(1, Ordering::SeqCst);
        }
        ctl.queue_save(1, b);
        // The debounced write fails; the writer is back waiting for a next change when the app quits.
        std::thread::sleep(super::SAVE_QUIET * 4);
        assert!(written().is_empty(), "the first write failed");
        ctl.flush_settings();
        assert_eq!(written(), vec![8.0], "tried again at the last flush");
    }

    /// verify-controller (CTL-1 at shutdown): a change queued after the last flush never starts a second writer that writes
    /// an older copy over the flushed one; a newer one is still written.
    #[test]
    fn a_change_after_the_last_flush_never_writes_an_older_copy() {
        use orbiters_core::settings::Settings;
        let ctl = super::Controller::new(Settings::default());
        let with = |gb: f64| {
            let mut s = Settings::default();
            s.storage_limit_gb = gb;
            s
        };
        let written = || ctl.test_settings_writes.lock().unwrap().iter().map(|s| s.storage_limit_gb).collect::<Vec<_>>();
        ctl.queue_save(2, with(2.0));
        ctl.flush_settings();
        assert_eq!(written(), vec![2.0], "the flush wrote B");
        // A applied first (revision 1) and queued late, after the flush.
        ctl.queue_save(1, with(1.0));
        ctl.flush_settings();
        assert_eq!(written(), vec![2.0], "A is older than what is on disk");
        ctl.queue_save(3, with(3.0));
        assert_eq!(written(), vec![2.0, 3.0], "a newer change after the flush is written at once");
    }

    /// LIGHT-DESKTOP-9: the library is walked again when something changed, while recording, and every ten minutes;
    /// the other 30 s turns ask only for the free space.
    #[test]
    fn the_library_is_walked_again_only_when_it_may_have_changed() {
        use std::time::{Duration, Instant};
        let mut sweep = super::StorageSweep::default();
        let dir = std::path::PathBuf::from("lib");
        let t0 = Instant::now();
        let m = 1u64;
        assert!(sweep.due(1, &dir, m, false, t0), "the first turn");
        assert!(!sweep.due(1, &dir, m, false, t0 + Duration::from_secs(30)), "nothing changed");
        assert!(sweep.due(2, &dir, m, false, t0 + Duration::from_secs(60)), "a session saved, trashed...");
        assert!(sweep.due(2, &dir, 2u64, false, t0 + Duration::from_secs(90)), "a folder added by hand");
        assert!(sweep.due(2, std::path::Path::new("other"), 2u64, false, t0 + Duration::from_secs(120)));
        assert!(sweep.due(2, std::path::Path::new("other"), 2u64, true, t0 + Duration::from_secs(150)), "recording");
        assert!(!sweep.due(2, std::path::Path::new("other"), 2u64, false, t0 + Duration::from_secs(180)));
        assert!(sweep.due(2, std::path::Path::new("other"), 2u64, false, t0 + Duration::from_secs(800)), "ten minutes");
    }

    /// verify-controller (LIGHT-DESKTOP-9): a session changed inside (its manifest rewritten, keyframes added), with the
    /// library's own folder untouched, changes the fingerprint: the next 30 s turn walks the library, as before.
    #[test]
    fn a_change_inside_a_session_is_seen_on_the_next_turn() {
        use std::time::{Duration, SystemTime};
        let root = std::env::temp_dir().join(format!("orbiters-fingerprint-{}", uuid::Uuid::new_v4()));
        let session = root.join("Home 1");
        std::fs::create_dir_all(session.join("keyframes")).unwrap();
        std::fs::write(session.join("manifest.json"), b"{}").unwrap();
        let at = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + s);
        let touch = |p: &std::path::Path, s: u64| std::fs::File::options().write(true).open(p).unwrap().set_modified(at(s)).unwrap();
        touch(&session.join("manifest.json"), 0);
        let before = super::library_fingerprint(&root);
        assert_eq!(super::library_fingerprint(&root), before, "nothing changed");
        touch(&session.join("manifest.json"), 60);
        let after = super::library_fingerprint(&root);
        assert_ne!(after, before, "its manifest rewritten");
        let mut sweep = super::StorageSweep::default();
        let t0 = std::time::Instant::now();
        assert!(sweep.due(1, &root, before, false, t0));
        assert!(sweep.due(1, &root, after, false, t0 + Duration::from_secs(30)), "walked on the next turn");
        std::fs::remove_dir_all(root).ok();
    }

    /// CTL-11 c: an avatar change heard as an event invalidates the kit once (one epoch, the last avatar's), also with
    /// two changes in one batch; a message given straight to `on_osc` still invalidates by itself.
    #[test]
    fn an_avatar_change_invalidates_the_kit_once() {
        let ctl = super::Controller::new(orbiters_core::settings::Settings::default());
        let epoch = || ctl.kit_epoch.lock().unwrap().clone();
        let start = epoch().0;
        ctl.on_osc_events(&mut vec![OscEvent::AvatarChanged { id: "avtr_a".into() }], clock::now_ns(), RECORD_PARAMETER);
        assert_eq!(epoch(), (start + 1, Some("avtr_a".into())));
        ctl.on_osc_events(&mut vec![OscEvent::AvatarChanged { id: "avtr_b".into() }, OscEvent::AvatarChanged { id: "avtr_c".into() }], clock::now_ns(), RECORD_PARAMETER);
        assert_eq!(epoch(), (start + 3, Some("avtr_c".into())), "one per change, the last avatar's");
        ctl.on_osc(vec![(AVATAR_CHANGE.to_string(), vec![OscValue::Text("avtr_d".into())])], clock::now_ns());
        assert_eq!(epoch(), (start + 4, Some("avtr_d".into())));
    }

    /// C-8: the keeper's answer about VRChat A's camera, arriving after the app followed VRChat B, is not shown for B;
    /// one asked of B is.
    #[test]
    fn a_late_camera_answer_of_the_last_vrchat_is_not_shown() {
        use std::sync::atomic::Ordering;
        let ctl = super::Controller::new(orbiters_core::settings::Settings::default());
        let asked_a = ctl.camera_route.load(Ordering::Acquire);
        ctl.camera_route_changed();
        ctl.update(|s| s.camera_open_said = None);
        assert!(!ctl.apply_keeper_camera(asked_a, true));
        assert_eq!(ctl.view().camera_open_said, None);
        let asked_b = ctl.camera_route.load(Ordering::Acquire);
        assert!(ctl.apply_keeper_camera(asked_b, false));
        assert_eq!(ctl.view().camera_open_said, Some(false));
    }

    /// C-1: the reason of a recording with no keyframe is worked out without holding `snapshot` while it waits for
    /// `osc_values`. A start holds `osc_values` and then takes `snapshot` (`start_segment`): here that start can take
    /// the snapshot while the reason waits.
    #[test]
    fn the_no_keyframe_reason_never_holds_the_snapshot_while_it_waits_for_osc_values() {
        use std::time::{Duration, Instant};
        let ctl = super::Controller::new(orbiters_core::settings::Settings::default());
        let start = ctl.osc_values.lock().unwrap();
        let saving = {
            let ctl = ctl.clone();
            std::thread::spawn(move || ctl.no_strip_reason())
        };
        std::thread::sleep(Duration::from_millis(100));
        let until = Instant::now() + Duration::from_secs(1);
        let mut took = false;
        while !took && Instant::now() < until {
            took = ctl.snapshot.try_lock().is_ok();
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(start);
        saving.join().unwrap();
        assert!(took, "the start (holding osc_values) could take the snapshot while the reason was being worked out");
    }

    /// The publish-rate log says a minute's publishes once, with the busiest second, and nothing for a silent minute.
    #[test]
    fn the_publish_rate_is_said_once_a_minute() {
        let mut rate = PublishRate::default();
        let mut publishes = 100;
        // What was published before it first looked counts in the first second.
        assert_eq!(rate.second(1, publishes, 3), None);
        let mut lines = Vec::new();
        for second in 1..180u64 {
            // The first minute: one a second, and a burst of 25; the second: nothing; the third: two.
            publishes += match second {
                1..=59 if second == 30 => 25,
                1..=59 => 1,
                130 | 131 => 1,
                _ => 0,
            };
            lines.extend(rate.second(1, publishes, 3).map(|line| (second, line)));
        }
        assert_eq!(
            lines,
            [
                (59, "183 publishes in a minute, at most 100 in one second, to 3 readers".to_string()),
                (179, "2 publishes in a minute, at most 1 in one second, to 3 readers".to_string())
            ]
        );
    }

    /// N-friend-lag: VRChat's wrists come every frame; they are kept, and the state goes out for them at most 20 times a
    /// second a hand, only while VRChat's camera is open in VR (the camera's tools read them then, and nothing else).
    #[test]
    fn vrchat_wrists_are_handed_out_only_while_its_camera_is_open_in_vr() {
        let mut s = Snapshot::new(Settings::default());
        // Far ahead of any time another test gave hand 1.
        let base = 9_000_000_000_000_000u64;
        let frame = 11_111_111u64;
        let mut handed = 0;
        for f in 0..90u64 {
            handed += super::wrist_said(&mut s, 1, [f as f32; 6], base + f * frame) as u32;
        }
        assert_eq!(handed, 0, "camera closed: nothing handed out");
        assert_eq!(s.vrchat_wrists[1], Some([89.0; 6]), "but the latest kept");
        assert_eq!(s.vrchat_wrists_ns[1], base + 89 * frame);
        s.camera_open_said = Some(true);
        for f in 90..180u64 {
            handed += super::wrist_said(&mut s, 1, [f as f32; 6], base + f * frame) as u32;
        }
        assert_eq!(handed, 0, "desktop mode (no SteamVR): the tools there do not read them");
        s.steamvr.running = true;
        for f in 180..270u64 {
            handed += super::wrist_said(&mut s, 1, [f as f32; 6], base + f * frame) as u32;
        }
        assert!((18..=20).contains(&handed), "{handed} in a second at 90 Hz");
    }

    /// The same through VRChat's OSC: a second of wrists with the camera closed publishes nothing.
    #[test]
    fn a_second_of_vrchat_wrists_publishes_nothing_with_its_camera_closed() {
        let ctl = Controller::new(Settings::default());
        let before = ctl.publishes.load(Ordering::Relaxed);
        for f in 0..90 {
            let pose = (0..6).map(|i| OscValue::Float((f * 6 + i) as f32)).collect();
            ctl.on_osc_events(&mut vec![OscEvent::Values { address: "/tracking/vrsystem/leftwrist/pose".into(), values: pose }], clock::now_ns(), RECORD_PARAMETER);
        }
        assert_eq!(ctl.publishes.load(Ordering::Relaxed), before);
        assert!(ctl.snapshot.lock().unwrap().vrchat_wrists[0].is_some());
    }

    /// A publish hands out a snapshot that shares every part with the one before, but those that changed; a part that
    /// was written to and holds what it held is the same part still; the camera path is kept whole and shown thinned.
    #[test]
    fn a_publish_shares_what_did_not_change() {
        use orbiters_core::slice::Slice;
        let ctl = Controller::new(Settings::default());
        let first = ctl.view();
        let told = ctl.publishes();
        ctl.update(|s| s.n4n0.afk = true);
        let second = ctl.view();
        assert_eq!(ctl.publishes(), told + 1);
        assert!(!Arc::ptr_eq(&first, &second) && second.n4n0.afk && !first.n4n0.afk, "the state handed out before stays as it was");
        assert!(Slice::same(&first.settings, &second.settings) && Slice::same(&first.capture, &second.capture) && Slice::same(&first.vrchat.world, &second.vrchat.world));
        assert!(Slice::same(&first.n4n0.lines, &second.n4n0.lines));
        // Written to, with what they held: nothing to hand out (XCT-18), and the same parts again at the next change.
        let told = ctl.publishes();
        ctl.update(|s| {
            s.osc.listening = first.osc.listening;
            s.capture.trail.clear();
            s.steamvr.overlay = false;
            s.vrchat.world.players.clear();
            s.settings.walk_boost.strength = first.settings.walk_boost.strength;
        });
        assert!(Arc::ptr_eq(&second, &ctl.view()) && ctl.publishes() == told, "nothing changed: nothing published");
        ctl.update(|s| s.n4n0.afk = false);
        let third = ctl.view();
        for same in [
            Slice::same(&second.osc, &third.osc),
            Slice::same(&second.capture, &third.capture),
            Slice::same(&second.steamvr, &third.steamvr),
            Slice::same(&second.vrchat.world, &third.vrchat.world),
            Slice::same(&second.settings, &third.settings),
        ] {
            assert!(same);
        }
        // One that changed is its own, and only that one.
        ctl.update(|s| s.settings.walk_boost.strength = first.settings.walk_boost.strength + 0.25);
        let fourth = ctl.view();
        assert!(!Slice::same(&third.settings, &fourth.settings) && Slice::same(&third.osc, &fourth.osc));
        assert_eq!(third.settings.walk_boost.strength, first.settings.walk_boost.strength, "the settings handed out before did not change under their readers");
        // A long camera path: 1500 points at most are shown, the first and the newest part of it among them.
        let mut recording = (*fourth).clone();
        recording.capture.trail = (0..6000).map(|i| [i as f32, 0.0, 0.0]).collect();
        ctl.set_snapshot(recording);
        let shown = ctl.view().capture.trail.clone();
        assert_eq!(shown.len(), 1500);
        assert_eq!((shown[0], shown[1], shown[1499]), ([0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [5996.0, 0.0, 0.0]));
        assert_eq!(ctl.trail.lock().unwrap().len(), 6000, "the whole path stays with the controller");
    }

    use super::*;

    fn review_controller() -> (Shared, std::net::UdpSocket) {
        let socket=std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_millis(30))).unwrap();
        let mut settings=Settings::default();
        settings.osc.vrchat_port=socket.local_addr().unwrap().port();
        settings.consent_acknowledged=true;
        let ctl=Controller::new(settings);
        ctl.start_osc_with(false);
        ctl.with_osc(|o|o.set_vrchat_query(Some(camera_tree())));
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(1)])],clock::now_ns());
        *ctl.test_commands.lock().unwrap()=Some(Vec::new());
        (ctl,socket)
    }
    fn review_dispatch_tree(record: Arc<AtomicBool>) -> std::net::SocketAddr {
        use std::io::{Read,Write};
        let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap(); let addr=listener.local_addr().unwrap();
        std::thread::spawn(move || { for mut stream in listener.incoming().flatten() {
            let mut buf=[0u8;2048]; let n=stream.read(&mut buf).unwrap_or(0);
            let body=if String::from_utf8_lossy(&buf[..n]).starts_with("GET /avatar/parameters/N4N0/Capture/Record ") {
                format!(r#"{{"FULL_PATH":"/avatar/parameters/N4N0/Capture/Record","TYPE":"T","VALUE":[{}]}}"#,record.load(Ordering::Acquire))
            } else {r#"{"FULL_PATH":"/usercamera","CONTENTS":{"Mode":{"FULL_PATH":"/usercamera/Mode","TYPE":"i","VALUE":[1]}}}"#.into()};
            let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
        }}); addr
    }
    fn review_source() -> SourceSpec { SourceSpec::TestPattern {width:16,height:16,fps:1.,depth_every:4,frames:0} }

    #[test]
    fn review_world_rollover_sends_a_second_start_and_pause() {
        for paused in [false,true] {
            let (ctl,_socket)=review_controller();
            ctl.start_with(review_source(),false,false).unwrap();
            ctl.update(|s|s.capture.state=if paused {CaptureState::Paused} else {CaptureState::Recording});
            ctl.update(|s|s.vrchat.world.set(WorldState {in_world:true, world_id:Some("wrld_new".into()), world_name:Some("New world".into()), ..Default::default()}));
            ctl.start_segment(review_source(),false,false,0,true).unwrap();
            if paused { ctl.pause_capture(true).unwrap(); }
            let commands=ctl.test_commands.lock().unwrap();
            assert_eq!(commands.as_ref().unwrap().iter().filter(|c|matches!(c,WorkerCommand::Start(_))).count(),2);
            assert!(commands.as_ref().unwrap().iter().rev().find_map(|c|if let WorkerCommand::Start(start)=c {Some(start.world.as_ref().unwrap().id.as_deref())} else {None}).flatten()==Some("wrld_new"));
            if paused { assert!(matches!(commands.as_ref().unwrap().last(),Some(WorkerCommand::Pause))); }
            drop(commands);
            ctl.update(|s|s.capture.state=CaptureState::Idle);
            ctl.shutdown();
        }
    }
    #[test]
    fn review_stop_cancels_taken_pending_start_at_thread_handoff() {
        let (ctl,_socket)=review_controller();
        ctl.update(|s|s.capture.state=CaptureState::Stopping);
        ctl.start_with(review_source(),false,false).unwrap();
        let pending=ctl.pending_start.lock().unwrap().take().unwrap();
        let barrier=Arc::new(std::sync::Barrier::new(2));
        let b=barrier.clone(); let c=ctl.clone();
        let thread=std::thread::spawn(move || {b.wait(); c.resume_pending(pending)});
        ctl.update(|s|s.capture.state=CaptureState::Idle);
        ctl.stop_capture(StopOf::Person).unwrap();
        barrier.wait();
        assert!(thread.join().unwrap().is_err());
        assert!(ctl.test_commands.lock().unwrap().as_ref().unwrap().is_empty());
        ctl.shutdown();
    }
    #[test]
    fn review_avatar_action_cannot_start_but_can_stop() {
        let (ctl,_socket)=review_controller();
        let record=Arc::new(AtomicBool::new(true));
        ctl.with_osc(|o|o.set_vrchat_query(Some(review_dispatch_tree(record.clone()))));
        ctl.on_osc_events(&mut vec![OscEvent::Parameter {name:RECORD_PARAMETER.into(),value:OscValue::Bool(true)}],clock::now_ns(),RECORD_PARAMETER);
        actions_done(&ctl);
        assert!(ctl.test_commands.lock().unwrap().as_ref().unwrap().is_empty());
        ctl.take_notices();
        ctl.update(|s|s.capture.state=CaptureState::Recording);
        ctl.on_record_toggle(true);
        assert!(ctl.take_notices().is_empty(), "an echo during recording is not a new start");
        record.store(false,Ordering::Release);
        ctl.on_osc_events(&mut vec![OscEvent::Parameter {name:RECORD_PARAMETER.into(),value:OscValue::Bool(false)}],clock::now_ns(),RECORD_PARAMETER);
        actions_done(&ctl);
        assert!(ctl.test_commands.lock().unwrap().as_ref().unwrap().iter().any(|c|matches!(c,WorkerCommand::Stop)));
        ctl.update(|s|s.capture.state=CaptureState::Idle); ctl.shutdown();
    }
    #[test]
    fn review_photo_qr_stream_is_shared_and_restored() {
        let (ctl,socket)=review_controller();
        ctl.camera_qr(true);
        let sent=received(&socket,Duration::from_millis(700));
        assert!(sent.contains(&(CAMERA_MODE.into(),rosc::OscType::Int(2))));
        assert!(sent.contains(&(CAMERA_STREAMING.into(),rosc::OscType::Bool(true))));
        assert!(ctl.avatar_before.lock().unwrap().is_none());
        ctl.open_stream_camera(&[(CAMERA_MODE.into(),OscValue::Int(2)),(CAMERA_STREAMING.into(),OscValue::Bool(true))]);
        ctl.camera_qr(false);
        assert!(received(&socket,Duration::from_millis(100)).is_empty());
        ctl.restore_camera();
        assert!(received(&socket,Duration::from_millis(200)).contains(&(CAMERA_MODE.into(),rosc::OscType::Int(1))));
        ctl.camera_qr(true);
        ctl.test_qr_events.lock().unwrap().as_ref().unwrap()(orbiters_n4n0::QrSeen::Code("https://example.com/".into()));
        assert!(matches!(ctl.view().camera_tools.qr,camera_tools::Qr::Found{..}));
        assert!(!ctl.camera_stream_owners.lock().unwrap()[1]);
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(0)])],clock::now_ns());
        ctl.camera_qr(true);
        assert_eq!(ctl.view().camera_tools.qr,camera_tools::Qr::NoStream);
        ctl.shutdown();
    }
    #[test]
    fn review_visual_reopening_allows_stream_but_a_late_mode_echo_cannot_reopen_it() {
        let (ctl,socket)=review_controller();
        ctl.camera_mode_command(2);
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(0)])],clock::now_ns());
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(2)])],clock::now_ns());
        assert_eq!(ctl.view().camera_open_said,Some(true));
        assert!(!ctl.acquire_camera_stream(1,&[(CAMERA_MODE.into(),OscValue::Int(1))]));
        assert!(received(&socket,Duration::from_millis(50)).is_empty());
        ctl.update(|s|s.orbit.camera_panel_seen_ns=Some(clock::now_ns()));
        assert!(ctl.acquire_camera_stream(1,&[(CAMERA_MODE.into(),OscValue::Int(1))]));
        assert!(received(&socket,Duration::from_millis(50)).contains(&(CAMERA_MODE.into(),rosc::OscType::Int(2))));
        ctl.camera_qr(false); ctl.shutdown();
    }

    #[test]
    fn review_stream_acquisition_cannot_write_mode_after_received_close() {
        let (ctl,socket)=review_controller();
        { let mut evidence=ctl.osc_values.lock().unwrap(); evidence.camera_received=Some(false); evidence.camera_closed_at=Some(clock::now_ns()); }
        assert!(!ctl.acquire_camera_stream(1,&[(CAMERA_MODE.into(),OscValue::Int(1))]));
        assert!(!ctl.camera_stream_owners.lock().unwrap()[1]);
        assert!(received(&socket,Duration::from_millis(100)).is_empty());
        ctl.shutdown();
    }

    #[test]
    fn review_qr_close_and_cancel_restore_without_reopening_or_a_late_stream_toggle() {
        let (ctl,socket)=review_controller();
        ctl.camera_qr(true);
        received(&socket,Duration::from_millis(30));
        // The receive/publish interval: restoration must honor a received close already.
        { let mut evidence=ctl.osc_values.lock().unwrap(); evidence.camera_received=Some(false); evidence.camera_closed_at=Some(clock::now_ns()); }
        ctl.camera_qr(false);
        let sent=received(&socket,Duration::from_millis(550));
        assert!(!sent.iter().any(|(p,v)| p==CAMERA_MODE && *v!=rosc::OscType::Int(0)));
        assert!(!sent.contains(&(CAMERA_STREAMING.into(),rosc::OscType::Bool(true))));
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(1)])],clock::now_ns());
        ctl.camera_qr(true);
        received(&socket,Duration::from_millis(30));
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(0)])],clock::now_ns());
        let sent=received(&socket,Duration::from_millis(550));
        assert_eq!(ctl.view().camera_tools.qr,camera_tools::Qr::Off);
        assert!(!ctl.camera_stream_owners.lock().unwrap()[1]);
        assert!(!sent.iter().any(|(p,v)| p==CAMERA_MODE && *v!=rosc::OscType::Int(0)));
        assert!(!sent.contains(&(CAMERA_STREAMING.into(),rosc::OscType::Bool(true))));
        ctl.shutdown();
    }

    #[test]
    fn review_camera_bootstrap_cannot_publish_across_reconnection() {
        let (ctl,_socket)=review_controller();
        let old=ctl.osc_generation.load(Ordering::Acquire);
        let route=ctl.camera_route.load(Ordering::Acquire);
        let release=Arc::new(std::sync::Barrier::new(2));
        let b=release.clone(); let c=ctl.clone();
        let response=std::thread::spawn(move || {b.wait(); c.apply_camera_seed(old,route,true)});
        ctl.start_osc_with(false);
        release.wait();
        response.join().unwrap();
        assert_eq!(ctl.view().camera_open_said,None);
        ctl.apply_camera_seed(ctl.osc_generation.load(Ordering::Acquire),ctl.camera_route.load(Ordering::Acquire),false);
        assert_eq!(ctl.view().camera_open_said,Some(false));
        ctl.shutdown();
    }

    /// verify-controller (C-8): the OSC start's camera seed asked of VRChat A, answering after the app followed VRChat B
    /// within the same OSC connection, is not shown for B.
    #[test]
    fn the_start_seed_of_one_vrchat_is_not_shown_for_the_next() {
        let (ctl,_socket)=review_controller();
        let generation=ctl.osc_generation.load(Ordering::Acquire);
        let asked_a=ctl.camera_route.load(Ordering::Acquire);
        // The app follows another VRChat: same connection, a new route.
        ctl.camera_route_changed();
        ctl.update(|s| s.camera_open_said=None);
        assert!(!ctl.apply_camera_seed(generation,asked_a,true));
        assert_eq!(ctl.view().camera_open_said,None);
        ctl.shutdown();
    }

    #[test]
    fn review_duplicate_and_delayed_mode_echoes_are_not_open_evidence() {
        let (ctl,_socket)=review_controller();
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(0)])],clock::now_ns());
        ctl.camera_mode_command(2);
        for offset in [0,1,3_000_000_000] {
            ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(2)])],clock::now_ns()+offset);
            assert_eq!(ctl.view().camera_open_said,Some(false));
        }
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(0)])],clock::now_ns());
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(2)])],clock::now_ns());
        assert_eq!(ctl.view().camera_open_said,Some(true));
        assert_eq!(ctl.osc_values.lock().unwrap().camera_received,Some(false));
        ctl.camera_mode_command(1);
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(2)])],clock::now_ns());
        assert_eq!(ctl.osc_values.lock().unwrap().camera_received,Some(false));
        ctl.start_osc_with(false);
        assert!(ctl.osc_values.lock().unwrap().camera_commanded.is_empty());
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(2)])],clock::now_ns());
        assert_eq!(ctl.osc_values.lock().unwrap().camera_received,Some(true));
        ctl.shutdown();
    }
    /// A VRChat of the roster, as the watcher reads it from its log.
    fn vrchat_run(pid: u32, profile: u32, osc_in: u16, driver: crate::bridge::sessions::Driver, account: &str, world: &str) -> crate::bridge::sessions::VrchatRun {
        crate::bridge::sessions::VrchatRun {
            pid,
            started_ns: u64::from(pid),
            profile,
            profile_known: true,
            log: None,
            osc_in,
            query: None,
            account: Some((format!("usr_{account}"), account.to_string())),
            account_sure: true,
            log_sure: true,
            signed_out: false,
            one_profile: true,
            world: Some(world.to_string()),
            desktop: Some(true),
            camera: None,
            driver,
        }
    }

    /// A datagram VRChat sends when its camera opens or closes.
    fn mode_datagram(mode: i32) -> Vec<u8> {
        rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage { addr: CAMERA_MODE.into(), args: vec![rosc::OscType::Int(mode)] })).unwrap()
    }

    /// Enzo's case, 2026-11-15 13:48 (logs7/two-vrchats.png): his own VRChat (desktop, FRONTLINE KIZUNA) beside the world
    /// tour's (profile 1, held by no session after the update, its camera closed). A picture in his worked at 13:48:10 and
    /// put his camera back in Stream mode (the app wrote Mode 2); at 13:48:56 VRChat's menu closed the camera and he opened
    /// it again in Stream mode; the next picture said "Open VRChat's camera first". Now: his VRChat is the target, the tour's
    /// datagrams never reach the app, and his camera opened again is the proof a picture needs.
    #[test]
    fn his_camera_opened_again_after_a_picture_is_proof_beside_a_closed_bridge_vrchat() {
        use crate::bridge::sessions::{takes_datagram, Driver};
        let (ctl, _socket) = review_controller();
        ctl.sides.set_roster(vec![vrchat_run(8244, 1, 9010, Driver::Bridge, "Orbiters", "Audience Anarchy"), vrchat_run(37076, 0, 9000, Driver::Person, "blackorbit", "FRONTLINE KIZUNA")]);
        ctl.follow_target();
        ctl.publish_runs();
        assert_eq!(ctl.sides.target().map(|t| t.pid), Some(37076), "his own, never the one the bridge left");
        assert_eq!(ctl.sides.app_pid(&[8244, 37076]), Some(37076));
        let shown = ctl.view().vrchat.runs.clone();
        assert_eq!(shown.len(), 2);
        assert!(shown.iter().any(|r| r.pid == 37076 && r.target && r.driver == crate::state::VrchatDriverShown::Person));
        assert!(shown.iter().any(|r| r.pid == 8244 && !r.target && r.driver == crate::state::VrchatDriverShown::Left));
        let t0 = clock::now_ns();
        // He opens the camera in Stream mode: proof.
        ctl.on_osc(vec![(CAMERA_MODE.into(), vec![OscValue::Int(2)])], t0);
        assert!(ctl.require_live_camera().is_ok());
        // The picture puts it back: the app writes Mode 2, VRChat echoes it.
        ctl.camera_mode_command(2);
        let written = clock::now_ns();
        ctl.on_osc(vec![(CAMERA_MODE.into(), vec![OscValue::Int(2)])], written + 1_000_000);
        // The tour's VRChat says its camera is closed: dropped, kept as its own.
        assert!(!takes_datagram(&ctl.sides, Some(8244), Some(37076), &mode_datagram(0)));
        assert_eq!(ctl.sides.camera_heard(8244).map(|h| h.0), Some(false));
        assert!(takes_datagram(&ctl.sides, Some(37076), Some(37076), &mode_datagram(0)), "his own is heard");
        // VRChat's menu closes his camera, then he opens it again in Stream mode.
        ctl.on_osc(vec![(CAMERA_MODE.into(), vec![OscValue::Int(0)])], written + 46_000_000_000);
        assert!(ctl.require_live_camera().is_err());
        ctl.on_osc(vec![(CAMERA_MODE.into(), vec![OscValue::Int(2)])], written + 60_000_000_000);
        assert_eq!(ctl.view().camera_open_said, Some(true));
        assert!(ctl.require_live_camera().is_ok(), "his camera opened again is his proof");
        // An echo of the app's own write, a moment after it, is still never proof.
        ctl.camera_mode_command(2);
        ctl.on_osc(vec![(CAMERA_MODE.into(), vec![OscValue::Int(0)])], clock::now_ns());
        ctl.on_osc(vec![(CAMERA_MODE.into(), vec![OscValue::Int(2)])], clock::now_ns());
        assert!(ctl.require_live_camera().is_err());
        ctl.shutdown();
    }

    /// The reverse: the person chose a bridge session's VRChat (its camera open) while their own has its camera closed. The
    /// target's camera is what counts, their own VRChat's datagrams are dropped, and the notice names the VRChat.
    #[test]
    fn a_chosen_bridge_vrchat_is_followed_and_the_persons_closed_camera_does_not_count() {
        use crate::bridge::sessions::{takes_datagram, Driver};
        let (ctl, _socket) = review_controller();
        let session = "p1-3f2a9c0d1e2f".to_string();
        ctl.sides.set_roster(vec![vrchat_run(8244, 1, 9010, Driver::Session(session.clone()), "Orbiters", "Audience Anarchy"), vrchat_run(37076, 0, 9000, Driver::Person, "blackorbit", "FRONTLINE KIZUNA")]);
        ctl.follow_target();
        // The session's camera opens while it is not followed: heard and kept.
        assert!(!takes_datagram(&ctl.sides, Some(8244), Some(37076), &mode_datagram(2)));
        ctl.choose_vrchat(Some(8244)).unwrap();
        assert_eq!(ctl.sides.target().map(|t| t.pid), Some(8244));
        assert_eq!(ctl.sides.chosen(), Some(8244));
        assert_eq!(ctl.view().camera_open_said, Some(true));
        assert!(ctl.require_live_camera().is_ok(), "its camera, heard while it was not followed");
        // His own closes its camera: not the target's.
        assert!(!takes_datagram(&ctl.sides, Some(37076), Some(8244), &mode_datagram(0)));
        assert!(ctl.require_live_camera().is_ok());
        let shown = ctl.view().vrchat.runs.clone();
        assert!(shown.iter().any(|r| r.pid == 8244 && r.target && r.chosen && r.driver == crate::state::VrchatDriverShown::Agent { session: "p1-3f2a…".into() }));
        // Back to his own: its camera closed, said as his, by name.
        ctl.choose_vrchat(None).unwrap();
        assert_eq!(ctl.sides.target().map(|t| t.pid), Some(37076));
        assert_eq!(ctl.sides.chosen(), None);
        assert_eq!(ctl.view().camera_open_said, Some(false));
        assert!(ctl.require_live_camera().is_err());
        orbiters_i18n::in_language(orbiters_i18n::Language::En, || assert_eq!(ctl.camera_first(), "Open the camera in blackorbit's VRChat first"));
        // No switch during a recording.
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        assert!(ctl.choose_vrchat(Some(8244)).is_err());
        assert_eq!(ctl.sides.target().map(|t| t.pid), Some(37076));
        ctl.update(|s| s.capture.state = CaptureState::Idle);
        // The chosen one going away gives the default back.
        ctl.choose_vrchat(Some(8244)).unwrap();
        ctl.sides.set_roster(vec![vrchat_run(37076, 0, 9000, Driver::Person, "blackorbit", "FRONTLINE KIZUNA")]);
        assert_eq!(ctl.sides.target().map(|t| t.pid), Some(37076));
        ctl.shutdown();
    }

    /// One VRChat: nothing new is shown, every datagram is taken as before, and the notice is the one of before.
    #[test]
    fn with_one_vrchat_nothing_changes() {
        use crate::bridge::sessions::Driver;
        let (ctl, _socket) = review_controller();
        ctl.sides.set_roster(vec![vrchat_run(37076, 0, 9000, Driver::Person, "blackorbit", "FRONTLINE KIZUNA")]);
        ctl.follow_target();
        ctl.publish_runs();
        assert_eq!(ctl.view().vrchat.runs.len(), 1);
        assert!(ctl.view().vrchat.several_target().is_none(), "nothing to choose, nothing said");
        orbiters_i18n::in_language(orbiters_i18n::Language::En, || assert_eq!(ctl.camera_first(), "Open VRChat's camera first"));
        let filter = crate::bridge::sessions::datagram_filter(ctl.sides.clone());
        assert!(filter(std::net::SocketAddr::from(([127, 0, 0, 1], 50000)), &mode_datagram(0)), "every datagram, as before");
        assert_eq!(ctl.sides.app_pid(&[37076]), Some(37076));
        ctl.on_osc(vec![(CAMERA_MODE.into(), vec![OscValue::Int(2)])], clock::now_ns());
        assert!(ctl.require_live_camera().is_ok());
        ctl.shutdown();
    }

    #[test]
    fn camera_mode_is_published_on_receive_and_unknown_cannot_record() {
        let ctl = Controller::new(Settings::default());
        assert!(ctl.require_live_camera().is_err());
        assert!(ctl.start_capture_from(crate::ipc::RecordingOrigin::OtherUi).is_err());
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(1)])],clock::now_ns());
        assert_eq!(ctl.view().camera_open_said,Some(true));
        assert!(ctl.require_live_camera().is_err(), "cached received Mode without a fresh query cannot authorize recording");
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(0)])],clock::now_ns());
        assert_eq!(ctl.view().camera_open_said,Some(false));
        assert!(ctl.stop_capture(StopOf::Person).is_ok());
        assert!(!ctl.view().live_recording_running());
    }
    #[test]
    fn a_stop_cancels_a_start_that_has_not_passed_preflight() {
        let ctl=Controller::new(Settings::default());
        let generation=ctl.stop_generation.load(Ordering::Acquire);
        ctl.stop_capture(StopOf::Person).unwrap();
        assert!(ctl.start_capture_generation(CapturePurpose::CameraOverlay,generation).is_err());
        assert_eq!(ctl.view().capture.state,CaptureState::Idle);
    }

    #[test]
    fn query_seed_and_an_app_written_mode_do_not_authorize_recording() {
        let stand_in=std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut settings=Settings::default();
        settings.osc.vrchat_port=stand_in.local_addr().unwrap().port();
        let ctl=Controller::new(settings);
        ctl.start_osc_with(false);
        ctl.with_osc(|o| o.set_vrchat_query(Some(camera_tree())));
        ctl.update(|s| s.camera_open_said=Some(true));
        assert!(ctl.require_live_camera().is_err(), "a query seed alone is not proof");
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(0)])],clock::now_ns());
        ctl.camera_mode_command(2);
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(2)])],clock::now_ns());
        assert_eq!(ctl.view().camera_open_said,Some(false));
        assert!(ctl.require_live_camera().is_err(), "the app's Mode echo is not proof");
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(1)])],clock::now_ns());
        assert!(ctl.require_live_camera().is_ok());
        ctl.shutdown();
    }

    #[test]
    fn a_kit_lookup_cannot_cross_an_avatar_change() {
        let ctl=Controller::new(Settings::default());
        let old=ctl.kit_epoch.lock().unwrap().clone();
        ctl.on_osc(vec![(AVATAR_CHANGE.into(),vec![OscValue::Text("avtr_first".into())])],clock::now_ns());
        assert!(!ctl.apply_kit_observation(&old,Some(true)));
        assert_eq!(ctl.view().worn_kit,None);
        let first=ctl.kit_epoch.lock().unwrap().clone();
        assert!(ctl.apply_kit_observation(&first,Some(false)));
        assert_eq!(ctl.view().worn_kit,Some(false));
        ctl.invalidate_kit(Some("avtr_second".into()));
        let second=ctl.kit_epoch.lock().unwrap().clone();
        assert!(!ctl.apply_kit_observation(&first,Some(true)));
        assert!(!ctl.apply_kit_observation(&second,Some(true)), "new avatar payload has not arrived yet");
        assert_eq!(ctl.view().worn_kit,None);
        ctl.on_osc(vec![(AVATAR_CHANGE.into(),vec![OscValue::Text("avtr_second".into())])],clock::now_ns());
        let second=ctl.kit_epoch.lock().unwrap().clone();
        assert!(ctl.apply_kit_observation(&second,Some(true)));
        assert_eq!(ctl.view().worn_kit,Some(true));
    }

    #[test]
    fn closing_the_camera_is_not_undone_by_recording_cleanup() {
        let (ctl, vrchat)=driving_vrchat();
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(0)])],clock::now_ns());
        ctl.restore_camera();
        let sent=received(&vrchat,Duration::from_millis(300));
        assert!(!sent.iter().any(|(p,v)| p==CAMERA_MODE && *v!=rosc::OscType::Int(0)));
        assert_eq!(ctl.view().camera_open_said,Some(false));
        ctl.shutdown();
    }

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
        let record = format!("/avatar/parameters/{RECORD_PARAMETER}");
        let depth = format!("/avatar/parameters/{DEPTH_FRAMES_PARAMETER}");
        assert_eq!((record.as_str(), depth.as_str()), ("/avatar/parameters/N4N0/Capture/Record", "/avatar/parameters/N4N0/Capture/Depth"));
        // Depth frames already on: only the record toggle is switched.
        ctl.switch_kit_on(&[(record.clone(), OscValue::Bool(false)), (depth.clone(), OscValue::Bool(true))]);
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
        ctl.switch_kit_on(&[]);
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
            ("/avatar/parameters/N4N0/Capture/Record".into(), rosc::OscType::Bool(false)),
            ("/avatar/parameters/N4N0/Capture/Depth".into(), rosc::OscType::Bool(false)),
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
        ctl.with_osc(|o| o.set_vrchat_query(Some(camera_tree())));
        ctl.on_osc(vec![(CAMERA_MODE.into(),vec![OscValue::Int(1)])],clock::now_ns());
        assert!(ctl.start_capture(CapturePurpose::CameraOverlay).is_err());
        let sent = received(&vrchat, Duration::from_millis(700));
        assert_eq!(sent.last(), Some(&("/avatar/parameters/N4N0/Capture/Depth".into(), rosc::OscType::Bool(false))), "{sent:?}");
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

    /// XCT-31: a thread that panics in a change of the state takes nobody with it: every other goes on changing it and
    /// publishing. (Before, the lock it held was poisoned, and each one that came after panicked in turn.)
    #[test]
    fn a_panic_under_the_snapshot_does_not_stop_the_service() {
        let ctl = Controller::new(Settings::default());
        let c = ctl.clone();
        let panicked = std::thread::spawn(move || {
            c.update(|s| {
                s.n4n0.photo_countdown = Some(9);
                s.capture.error = Some("half a change".into());
                panic!("in a change of the state, on purpose");
            })
        });
        assert!(panicked.join().is_err());
        for n in [3, 2, 1] {
            let c = ctl.clone();
            std::thread::spawn(move || c.update(|s| s.n4n0.photo_countdown = Some(n))).join().expect("the next change is made");
            assert_eq!(ctl.view().n4n0.photo_countdown, Some(n), "and published");
            assert!(ctl.view().capture.error.is_none(), "the panicking update was discarded");
        }
        // The settings are changed whole or not at all.
        let before = ctl.settings();
        let c = ctl.clone();
        let panicked = std::thread::spawn(move || {
            c.update_settings(|s| {
                s.storage_limit_gb = 1.0;
                panic!("in a change of the settings, on purpose");
            })
        });
        assert!(panicked.join().is_err());
        assert_eq!(ctl.settings(), before, "nothing of half a change");
        ctl.update_settings(|s| s.storage_limit_gb = 2.0);
        assert_eq!(ctl.view().settings.storage_limit_gb, 2.0);
        ctl.shutdown();
    }

    #[test]
    fn reading_vrchats_values_never_holds_the_osc_lock() {
        // What the test sends over OSC goes to a stand-in, never to a VRChat running on this computer.
        let stand_in = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = stand_in.local_addr().unwrap().port();
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        let addr = stand_in.local_addr().unwrap();
        ctl.with_osc(|o| o.set_vrchat_query(Some(addr))).unwrap();
        let (query, values) = ctl.read_vrchat_avatar_values(|asked| {
            assert_eq!(asked, addr);
            assert!(ctl.osc.try_lock().is_ok(), "the query runs with the OSC lock free");
            Ok(vec![("/avatar/parameters/Test".into(), OscValue::Bool(true))])
        });
        assert_eq!(query, Some(Some(addr)));
        assert_eq!(values, [("/avatar/parameters/Test".into(), OscValue::Bool(true))]);
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
    fn camera_tree() -> std::net::SocketAddr {
        use std::io::{Read,Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr=listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf=[0u8;2048];
                if stream.read(&mut buf).is_err() { continue; }
                let body=r#"{"FULL_PATH":"/usercamera","CONTENTS":{"Mode":{"FULL_PATH":"/usercamera/Mode","TYPE":"i","VALUE":[1]}}}"#;
                let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
            }
        });
        addr
    }

    /// CTL-10: the kit probe (every keeper turn while VRChat runs) asks VRChat for its avatar's tree only, not for its
    /// camera's as well.
    #[test]
    fn the_kit_probe_asks_for_the_avatar_tree_only() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let asked = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = asked.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let line = String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or_default().to_string();
                log.lock().unwrap().push(line);
                let body = r#"{"FULL_PATH":"/avatar","CONTENTS":{"parameters":{"FULL_PATH":"/avatar/parameters","CONTENTS":{"VRCEmote":{"FULL_PATH":"/avatar/parameters/VRCEmote","TYPE":"i","VALUE":[0]}}}}}"#;
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            }
        });
        let ctl = Controller::new(Settings::default());
        ctl.start_osc_with(false);
        // The OSC start's own camera seed asks `/usercamera` until the camera is known: known here, and that thread let
        // go (it looks every 100 ms) before the tree answers, so only the probe asks it.
        ctl.update(|s| s.camera_open_said = Some(true));
        std::thread::sleep(std::time::Duration::from_millis(400));
        ctl.with_osc(|o| o.set_vrchat_query(Some(addr)));
        let _ = ctl.worn_avatar_has_kit();
        let asked = asked.lock().unwrap().clone();
        assert!(!asked.is_empty(), "the avatar's tree was asked for");
        assert!(asked.iter().all(|l| !l.contains("/usercamera")), "{asked:?}");
    }

    fn vrchat_tree(record: Arc<AtomicBool>) -> std::net::SocketAddr {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).unwrap_or(0);
                let answer = if String::from_utf8_lossy(&buf[..n]).starts_with("GET /avatar/parameters/N4N0/Capture/Record ") {
                    let body = format!(r#"{{"FULL_PATH":"/avatar/parameters/N4N0/Capture/Record","TYPE":"T","VALUE":[{}]}}"#, record.load(Ordering::Relaxed));
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
        assert_eq!(ctl.view().osc.last_event.as_deref(), Some("N4N0/Capture/Record = off"));
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
        assert_eq!(ctl.view().osc.last_event.as_deref(), Some("N4N0/Capture/Record = off"));
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
        *ctl.avatar_before.lock().unwrap() = Some(AvatarToggles { depth: false });
        ctl.update(|s| s.capture.state = CaptureState::Recording);
        for _ in 0..50 {
            ctl.on_avatar_changed();
        }
        let sent = received(&vrchat, KIT_SETTLE + Duration::from_millis(700));
        assert_eq!(sent.len(), 2, "record and depth switched on once: {sent:?}");
        ctl.update(|s| s.capture.state = CaptureState::Idle);
        ctl.shutdown();
    }

    /// DCT-2: two changes at once are acted on in the order they were made, whichever thread is the slower: what runs is as the
    /// newest has it. A change made by what acting does comes after, and waits for nobody.
    #[test]
    fn settings_are_acted_on_in_the_order_they_changed() {
        let ctl = Controller::new(Settings::default());
        let (first, stalled) = mpsc::channel();
        let (go, wait) = mpsc::channel::<()>();
        let wait = Mutex::new(Some(wait));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (first, acted, weak) = (Mutex::new(first), seen.clone(), Arc::downgrade(&ctl));
        *ctl.test_settings_acted.lock().unwrap() = Some(Arc::new(move |s, done| {
            if done {
                return acted.lock().unwrap().push((s.walk_boost.strength, s.walk_boost.enabled));
            }
            let held = wait.lock().unwrap().take();
            if let Some(wait) = held {
                first.lock().unwrap().send(()).unwrap();
                let _ = wait.recv_timeout(Duration::from_secs(20));
                weak.upgrade().unwrap().update_settings(|s| s.walk_boost.enabled = true);
            }
        }));
        std::thread::scope(|scope| {
            scope.spawn(|| ctl.set_walk_boost(0.25, false));
            stalled.recv().unwrap();
            // The second is made, and its thread is done, while the first is still acted on.
            scope.spawn(|| ctl.set_walk_boost(0.75, false)).join().unwrap();
            assert_eq!(ctl.settings().walk_boost.strength, 0.75);
            go.send(()).unwrap();
        });
        assert_eq!(*seen.lock().unwrap(), [(0.25, false), (0.75, false), (0.75, true)]);
    }

    #[test]
    fn a_settings_patch_keeps_what_changed_meanwhile() {
        let ctl = Controller::new(Settings::default());
        // The avatar's radial moves after the window read its copy of the settings.
        ctl.set_walk_boost(0.9, true);
        ctl.patch_settings(&serde_json::json!({ "n4n0": { "wake_word": true }, "walk_boost": { "enabled": true }, "osc": { "vrchat_port": "x" } }));
        let s = ctl.settings();
        assert!(s.n4n0.wake_word && s.walk_boost.enabled);
        assert_eq!(s.walk_boost.strength, 0.9, "the radial's change stays");
        assert_eq!(s.osc.vrchat_port, 9000, "a value that does not fit is left out");
        assert!(ctl.view().settings.n4n0.wake_word, "published");
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
