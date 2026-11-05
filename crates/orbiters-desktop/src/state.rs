//! Everything the interface shows lives in one [`Snapshot`], republished on every change.

use orbiters_core::protocol::{AudioDeviceInfo, CaptureStats, CoverageMap, SpoutSourceInfo};
use orbiters_core::reconstruction::Stage;
use orbiters_core::settings::Settings;
use orbiters_core::vrchat_log::WorldState;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Idle,
    Starting,
    Recording,
    Paused,
    Stopping,
    Importing,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VrchatStatus {
    pub running: bool,
    /// Process id while it runs (its sound can be recorded).
    #[serde(default)]
    pub pid: Option<u32>,
    pub world: WorldState,
    pub log_found: bool,
    /// How VRChat moves the avatar, once its parameters said so: the SteamVR tab's walk boost carries only while VRChat
    /// walks the avatar itself.
    #[serde(default)]
    pub motion: Option<AvatarMotion>,
    /// It runs on another device of the network ([`LanStatus`]): it takes nothing of this computer's GPU or processor.
    #[serde(default)]
    pub remote: bool,
}

impl VrchatStatus {
    /// Whether a VRChat runs on this very computer: what shares its GPU, and what its log, Spout and sound come from.
    pub fn runs_here(&self) -> bool {
        self.running && !self.remote
    }
}

/// How the app stands with VRChat on the network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanLink {
    /// Not looking: VRChat runs on this computer, or OSC is off.
    #[default]
    Off,
    /// This computer is on no local network.
    NoNetwork,
    /// No VRChat found yet.
    Looking,
    /// Several found: the person chooses.
    Choose,
    /// A VRChat is chosen but neither answers nor sends anything.
    Waiting,
    /// VRChat answers or sends.
    Connected,
}

/// VRChat on the network, where VRChat does not run on this computer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LanStatus {
    pub link: LanLink,
    /// Every VRChat on the network now.
    pub found: Vec<orbiters_core::lan::FoundVrchat>,
    /// The one talked with: its address, and what to call it.
    pub peer: Option<std::net::Ipv4Addr>,
    pub peer_label: Option<String>,
    /// This computer's address on that network, for what a PC's launch options must say.
    pub this_computer: Option<std::net::Ipv4Addr>,
}

/// What VRChat's own avatar parameters (`InStation`, `Seated`, `VelocityMagnitude`) say of its moving. The speed is
/// kept to a quarter metre a second, so that walking does not publish the state at the rate VRChat sends it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AvatarMotion {
    pub in_station: bool,
    pub seated: bool,
    /// Metres a second.
    pub speed: f32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OscStatus {
    pub enabled: bool,
    pub listening: bool,
    pub advertising: bool,
    pub port: Option<u16>,
    pub last_event: Option<String>,
    pub error: Option<String>,
    /// VRChat's own OSCQuery service was found (current values can be read at session start).
    pub vrchat_found: bool,
    /// Distinct OSC addresses VRChat sent values for (avatar parameters, poses).
    pub values: usize,
    /// VRChat sent something within the last minute. It sends avatar parameters only to OSC services it found:
    /// off while VRChat runs, the avatar's buttons (Talk, a hand on N4N0) do not reach Orbiters.
    #[serde(default)]
    pub receiving: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaptureStatus {
    pub state: CaptureState,
    pub session_dir: Option<PathBuf>,
    pub source: Option<String>,
    pub stats: Option<CaptureStats>,
    pub error: Option<String>,
    /// Camera positions (Unity world) of the running capture, thinned out.
    pub trail: Vec<[f32; 3]>,
    /// Capture started by the Action Menu (OSC) rather than the interface.
    pub from_osc: bool,
    /// What the running capture's depth frames have seen so far, with hints.
    #[serde(default)]
    pub coverage: Option<CoverageMap>,
    /// The running capture is a 3D picture (its camera flies on its own: no coverage to follow).
    #[serde(default)]
    pub photo_3d: bool,
    /// Why frames come without the capture strip, while they do and after a recording that never had it.
    #[serde(default)]
    pub no_strip: Option<NoStrip>,
    /// What the running or last 3D picture is of: `None` for a recording.
    #[serde(default)]
    pub picture: Option<orbiters_core::settings::PictureMode>,
    /// Who the running or last capture left out, and what the person should know about it.
    #[serde(default)]
    pub left_out: LeftOut,
}

/// Who a capture left out of the picture.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftOut {
    pub wearer: orbiters_core::picture::Left,
    pub others: orbiters_core::picture::Left,
    /// Why somebody is in the picture after all, and what to do about it.
    pub why: Option<String>,
}

/// Why frames come without the capture strip: no camera pose, so no keyframes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoStrip {
    /// VRChat's camera has Local User off, which culls the capture kit with the avatar.
    MaskOff,
    /// The worn avatar has no capture kit.
    NoKit,
    /// The kit and the mask are on: a world's depth of field or motion blur, or an old upload.
    NotDrawn,
    /// VRChat does not say (OSC is off, or it did not answer).
    Unknown,
}

impl NoStrip {
    /// The sentence the person reads, once a stretch.
    pub fn message(self) -> &'static str {
        match self {
            Self::MaskOff => "VRChat's camera has Local User off, which hides your avatar from the camera and N4N0's capture kit with it, so there is no camera pose and there are no keyframes. Turn Local User on in the camera's Mask menu.",
            Self::NoKit => "The avatar you are wearing has no N4N0 capture kit, so there is no camera pose and there are no keyframes. Wear an avatar with N4N0, or add N4N0 to this one in My Avatar and upload it.",
            Self::NotDrawn => "N4N0's capture kit and Local User are on, but the capture strip is not in the picture, so there are no keyframes. A world's depth of field or motion blur can wipe it. If it never shows, update N4N0 in My Avatar and upload the avatar again.",
            Self::Unknown => "No capture strip in the picture, so there is no camera pose and there are no keyframes. Check that you wear an avatar with N4N0 and that Local User is on in the camera's Mask menu.",
        }
    }

    /// The few words the page's tile has room for.
    pub fn short(self) -> &'static str {
        match self {
            Self::MaskOff => "Local User is off",
            Self::NoKit => "no capture kit on this avatar",
            Self::NotDrawn => "the world may wipe it",
            Self::Unknown => "no camera pose",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StorageStatus {
    pub sessions_dir: PathBuf,
    pub used_bytes: u64,
    pub free_bytes: Option<u64>,
    pub limit_gb: f64,
    pub sessions: usize,
    /// Of `used_bytes`, the capture files of reconstructed sessions (their frames, depth and sound), which keeping only
    /// their 3D views frees, in that many sessions.
    #[serde(default)]
    pub capture_bytes: u64,
    #[serde(default)]
    pub capture_sessions: usize,
}

/// Space being freed (controller/cleanup.rs), for good: the capture files of sessions whose 3D views stay, and sessions
/// that go whole.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CleanupJob {
    /// Waiting, in order: sessions keeping only their 3D view, then sessions going whole.
    pub trim: Vec<PathBuf>,
    pub delete: Vec<PathBuf>,
    /// The session being done now, and whether it goes whole.
    pub current: Option<(PathBuf, bool)>,
    /// Bytes freed so far, of what the whole job frees (as measured when it was asked).
    pub freed: u64,
    pub total: u64,
}

impl CleanupJob {
    /// Whether the job has `dir` still to do or doing: `Some(true)` when it goes whole.
    pub fn of(&self, dir: &std::path::Path) -> Option<bool> {
        match &self.current {
            Some((current, whole)) if current == dir => Some(*whole),
            _ if self.trim.iter().any(|d| d == dir) => Some(false),
            _ if self.delete.iter().any(|d| d == dir) => Some(true),
            _ => None,
        }
    }

    /// Adds sessions to do, each once (one already waiting or being done keeps its place).
    pub fn add(&mut self, trim: &[PathBuf], delete: &[PathBuf]) {
        for dir in trim {
            if self.of(dir).is_none() {
                self.trim.push(dir.clone());
            }
        }
        for dir in delete {
            if self.of(dir).is_none() {
                self.delete.push(dir.clone());
            }
        }
    }

    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.freed as f64 / self.total as f64).min(1.0) as f32
        }
    }
}

/// Where the Orbiters account stands (the one the Unity tools share).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AccountState {
    /// Not read yet.
    #[default]
    Unknown,
    SignedOut,
    /// Saved by a tool, being checked with Orbiters.
    Checking {
        username: Option<String>,
    },
    SignedIn {
        username: String,
    },
    /// The saved sign-in was refused: expired, or signed out everywhere.
    Expired {
        username: Option<String>,
    },
    /// Orbiters could not be reached to check it.
    Offline {
        username: Option<String>,
    },
    /// Finishing in the browser: Connect on the page that opened there.
    SigningIn,
}

impl AccountState {
    /// The account's name when one is saved.
    pub fn username(&self) -> Option<&str> {
        match self {
            AccountState::SignedIn { username } => Some(username),
            AccountState::Checking { username } | AccountState::Expired { username } | AccountState::Offline { username } => username.as_deref(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AccountStatus {
    pub state: AccountState,
    /// The Unity tools are set to the development server.
    pub development: bool,
    /// The last sign-in problem.
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SteamVrStatus {
    /// SteamVR's server runs.
    pub running: bool,
    /// Its process id while it runs (`vrserver`): a SteamVR quit and started again is another one.
    #[serde(default)]
    pub pid: Option<u32>,
    /// The Orbiters overlay is connected (a tab in SteamVR's dashboard).
    pub overlay: bool,
    /// Someone wears the headset (SteamVR's activity level, read by the overlay); `None` without the overlay.
    #[serde(default)]
    pub headset_in_use: Option<bool>,
    /// The latest T-pose calibration.
    pub calibration: Option<CalibrationInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalibrationInfo {
    pub head_height_m: f32,
    pub arm_span_m: f32,
    pub devices: usize,
    /// Unix time in milliseconds.
    pub utc_ms: u64,
}

/// The running reconstruction (one at a time).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReconstructionJob {
    pub session_dir: PathBuf,
    pub stage: Stage,
    /// Of the stage: decoded frames while loading, steps while training.
    pub progress: f32,
    pub iteration: u32,
    pub iterations: u32,
    pub gaussians: usize,
    /// The latest held-out PSNR (dB).
    pub psnr: Option<f32>,
    /// Training time so far.
    pub seconds: f32,
    /// On the integrated GPU, while VRChat runs (settings' `reconstruct_while_playing`).
    pub integrated: bool,
    /// Paused: its worker has gone or is going, a checkpoint holds where it stood, and it goes on by itself once what it
    /// gave way to is over.
    #[serde(default)]
    pub paused: bool,
    /// Going on from a pause: reading the session again, before its next step.
    #[serde(default)]
    pub resuming: bool,
    /// The share of the whole run a pause kept: what it shows never falls under this while it reads the session again.
    #[serde(default)]
    pub floor: f32,
}

impl ReconstructionJob {
    pub fn new(session_dir: PathBuf, integrated: bool) -> Self {
        Self { session_dir, stage: Stage::Loading, progress: 0.0, iteration: 0, iterations: 0, gaussians: 0, psnr: None, seconds: 0.0, integrated, paused: false, resuming: false, floor: 0.0 }
    }

    /// Where a checkpoint says a paused run stands.
    pub fn at(&mut self, iteration: u32, iterations: u32, gaussians: usize, psnr: Option<f32>, seconds: f32) {
        self.iteration = iteration;
        self.iterations = iterations;
        self.gaussians = gaussians;
        self.psnr = psnr.or(self.psnr);
        self.seconds = seconds;
        self.progress = iteration as f32 / iterations.max(1) as f32;
    }
}

/// What is sent to the Map explorer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadKind {
    Preview,
    Pictures,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UploadState {
    /// Building the web preview of the splats.
    Preparing,
    Sending,
    Done {
        added: usize,
        skipped: usize,
    },
    Failed {
        message: String,
    },
}

/// An upload to the Map explorer (the latest ones stay listed once finished).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UploadStatus {
    pub id: u64,
    pub world_id: String,
    pub world_name: String,
    pub kind: UploadKind,
    /// The session (preview) or the pictures sent.
    pub paths: Vec<PathBuf>,
    pub sent: u64,
    pub total: u64,
    pub state: UploadState,
}

impl UploadStatus {
    pub fn running(&self) -> bool {
        matches!(self.state, UploadState::Preparing | UploadState::Sending)
    }

    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.sent as f64 / self.total as f64) as f32
        }
    }
}

/// What this member added to the Map explorer (the tray service fetches it: the credential stays there).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MapsMine {
    pub worlds: Vec<orbiters_core::maps::MineWorld>,
    /// The last fetch failed (the worlds are the previous answer).
    pub error: Option<String>,
}

// N4N0's part of the snapshot, shared with the phone app (`orbiters_n4n0::status`).
pub use orbiters_n4n0::status::{
    AvatarView, CallContact, CallLink, CallPhase, CallView, CallsStatus, N4n0Confirmation, N4n0Face, N4n0Light, N4n0Line, N4n0Status, N4n0Who, ProcessUsage,
    StateAction, StateView, StatesView, VoiceStatus,
};

/// VRChat's frames the SteamVR overlay saw, about ten seconds of them (contracts `Docs/world-performance.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameChunk {
    /// Time between VRChat's frames, oldest first.
    pub intervals_ms: Vec<f32>,
    pub refresh_hz: Option<f32>,
    /// The headset's model (SteamVR's model number).
    pub headset: Option<String>,
    /// The graphics card driving the headset.
    pub gpu: Option<String>,
}

/// Updates of an installed app (controller/update.rs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateStatus {
    /// The setup installed this copy: it updates itself (a development build does not).
    pub enabled: bool,
    /// The release channel it follows.
    pub channel: String,
    pub state: UpdateState,
    /// The last look that reached Orbiters (Unix seconds).
    pub checked: Option<u64>,
    /// The person asked to restart once the running work ends ([`Snapshot::busy_with`]).
    pub restart_when_idle: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateState {
    /// Up to date, or not looked yet.
    #[default]
    Idle,
    Checking,
    Downloading {
        version: String,
        fraction: f32,
    },
    /// Downloaded and checked: a restart installs it.
    Ready {
        version: String,
        notes: String,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkerStatus {
    pub ready: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: String,
    pub vrchat: VrchatStatus,
    pub sources: Vec<SpoutSourceInfo>,
    /// Microphones the worker can record.
    pub mics: Vec<AudioDeviceInfo>,
    pub osc: OscStatus,
    #[serde(default)]
    pub lan: LanStatus,
    pub capture: CaptureStatus,
    pub settings: Settings,
    pub storage: StorageStatus,
    pub worker: WorkerStatus,
    pub steamvr: SteamVrStatus,
    pub reconstruction: Option<ReconstructionJob>,
    /// Sessions waiting to be reconstructed, next first.
    pub reconstruct_queue: Vec<PathBuf>,
    #[serde(default)]
    pub account: AccountStatus,
    pub vrchat_login: crate::controller::vrchat_login::Status,
    pub uploads: Vec<UploadStatus>,
    pub capture_shares: Vec<crate::controller::shares::ShareJob>,
    #[serde(default)]
    pub n4n0: N4n0Status,
    #[serde(default)]
    pub update: UpdateStatus,
    /// Space being freed in the library.
    #[serde(default)]
    pub cleanup: Option<CleanupJob>,
    /// What "When SteamVR starts" last did to Windows' sound devices (Settings).
    #[serde(default)]
    pub vr_audio: crate::controller::vr_audio::Status,
    /// The experimental VRChat bridge (`crate::bridge`): only filled in for an account the server says has it.
    #[serde(default)]
    pub bridge: crate::bridge::Status,
}

impl Snapshot {
    pub fn new(settings: Settings) -> Self {
        Self {
            version: orbiters_core::APP_VERSION.into(),
            vrchat: VrchatStatus {
                running: false,
                pid: None,
                world: WorldState::default(),
                log_found: orbiters_core::paths::vrchat_log_dir().is_some(),
                motion: None,
                remote: false,
            },
            sources: Vec::new(),
            mics: Vec::new(),
            osc: OscStatus { enabled: settings.osc.enabled, ..Default::default() },
            lan: LanStatus::default(),
            capture: CaptureStatus {
                state: CaptureState::Idle,
                session_dir: None,
                source: None,
                stats: None,
                error: None,
                trail: Vec::new(),
                from_osc: false,
                coverage: None,
                photo_3d: false,
                no_strip: None,
                picture: None,
                left_out: LeftOut::default(),
            },
            storage: StorageStatus { sessions_dir: settings.sessions_dir.clone(), limit_gb: settings.storage_limit_gb, ..Default::default() },
            settings,
            worker: WorkerStatus::default(),
            steamvr: SteamVrStatus::default(),
            reconstruction: None,
            reconstruct_queue: Vec::new(),
            account: AccountStatus::default(),
            vrchat_login: Default::default(),
            uploads: Vec::new(),
            capture_shares: Vec::new(),
            n4n0: N4n0Status::default(),
            update: UpdateStatus::default(),
            cleanup: None,
            vr_audio: Default::default(),
            bridge: Default::default(),
        }
    }

    pub fn is_capturing(&self) -> bool {
        matches!(self.capture.state, CaptureState::Starting | CaptureState::Recording | CaptureState::Paused | CaptureState::Importing)
    }

    /// What a restart (to update) would cut short now, as the interface names it: a recording being made or saved, a
    /// reconstruction running or waiting, a call, an upload to the Map explorer, the speech models downloading.
    pub fn busy_with(&self) -> Option<&'static str> {
        if self.is_capturing() || self.capture.state == CaptureState::Stopping {
            Some("the recording")
        } else if self.reconstruction.is_some() || !self.reconstruct_queue.is_empty() {
            Some("the reconstruction")
        } else if self.n4n0.calls.call.is_some() {
            Some("the call")
        } else if self.uploads.iter().any(UploadStatus::running) || self.capture_shares.iter().any(|j| j.outcome.is_none()) {
            Some("the upload")
        } else if self.n4n0.voice.downloading.is_some() {
            Some("the speech models' download")
        } else if self.cleanup.is_some() {
            Some("the clean-up")
        } else {
            None
        }
    }

    /// Adds a camera position to the trail when it moved enough (keeps the trail light).
    pub fn push_trail(&mut self, p: [f32; 3]) {
        let far_enough = self.capture.trail.last().map(|q| {
            let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > 0.0025
        });
        if far_enough.unwrap_or(true) {
            self.capture.trail.push(p);
            if self.capture.trail.len() > 6000 {
                // Thin the older half (every other point) instead of dropping the start of the path: the recent part
                // keeps its detail, the oldest is thinned most.
                let half = self.capture.trail.len() / 2;
                let mut keep: Vec<[f32; 3]> = self.capture.trail[..half].iter().step_by(2).copied().collect();
                keep.extend_from_slice(&self.capture.trail[half..]);
                self.capture.trail = keep;
            }
        }
    }
}
