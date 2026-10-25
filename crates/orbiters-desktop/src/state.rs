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
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StorageStatus {
    pub sessions_dir: PathBuf,
    pub used_bytes: u64,
    pub free_bytes: Option<u64>,
    pub limit_gb: f64,
    pub sessions: usize,
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
}

impl ReconstructionJob {
    pub fn new(session_dir: PathBuf, integrated: bool) -> Self {
        Self { session_dir, stage: Stage::Loading, progress: 0.0, iteration: 0, iterations: 0, gaussians: 0, psnr: None, seconds: 0.0, integrated }
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

/// What N4N0 shows (`N4N0/State`, contracts `Docs/orb.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrbFace {
    #[default]
    Idle,
    Listening,
    Thinking,
    Talking,
    Sleeping,
    Happy,
    Annoyed,
    Surprised,
}

/// Who said a line of N4N0's conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrbWho {
    You,
    Orb,
    /// Something N4N0 did or noticed (a tool ran, a video was saved).
    Note,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrbLine {
    pub who: OrbWho,
    pub text: String,
    /// Unix time in milliseconds.
    pub at_ms: u64,
}

/// An action of N4N0's (publishing an event…) waiting for the member's approval on Orbiters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrbConfirmation {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub approve_url: String,
}

/// The link of a confirmation the owner gives in Orbiters - N4N0 itself (what N4N0 asks before it does a public,
/// lasting or costly thing), rather than on the Orbiters website: the number of what waits follows it.
pub const LOCAL_APPROVAL: &str = "orbiters-desktop:orb/approve/";

impl OrbConfirmation {
    /// Approved in Orbiters - N4N0 (the N4N0 page's button, or a spoken yes), not on the Orbiters website.
    pub fn is_local(&self) -> bool {
        self.approve_url.starts_with(LOCAL_APPROVAL)
    }
}

/// The speech models and the voice worker.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VoiceStatus {
    /// The speech models are on this computer.
    pub installed: bool,
    /// The voice worker runs (listening for the wake word, or ready for the Talk button).
    pub running: bool,
    /// The models are downloading: the fraction done.
    pub downloading: Option<f32>,
    /// The microphone's level while N4N0 listens (0 to 1).
    pub level: f32,
    pub error: Option<String>,
    /// The models stopped loading (the worker stopped twice while loading them): installing again repairs them.
    #[serde(default)]
    pub repair: bool,
}

/// A process's share of this computer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessUsage {
    pub name: String,
    /// Percent of one core.
    pub cpu_percent: f32,
    pub memory_mb: f32,
}

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

/// N4N0, the companion on the avatar (controller/orb.rs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrbStatus {
    /// The avatar VRChat loaded.
    pub avatar_id: Option<String>,
    /// The avatar carries the N4N0 prop, with this many text sync values.
    pub prop_sync: Option<u8>,
    /// The avatar's manifest was found: its menu is known by label.
    pub manifest: bool,
    /// Controls N4N0 can reach on the avatar.
    pub controls: usize,
    pub face: OrbFace,
    /// What the panel shows now.
    pub panel: Option<String>,
    pub listening: bool,
    pub thinking: bool,
    pub afk: bool,
    /// The conversation as shown, the newest last.
    pub lines: Vec<OrbLine>,
    pub confirmations: Vec<OrbConfirmation>,
    /// The free quota of N4N0's assistant, as the last answer told it.
    pub quota: Option<orbiters_orb::assistant::Quota>,
    pub voice: VoiceStatus,
    /// Saved videos, the newest first.
    pub videos: Vec<orbiters_orb::videos::SavedVideo>,
    /// Orbiters' processes and VRChat's.
    pub resources: Vec<ProcessUsage>,
    /// Testing: the state N4N0 is held in, every state shown in turn, the owner acted as away.
    #[serde(default)]
    pub test_face: Option<OrbFace>,
    #[serde(default)]
    pub test_cycle: bool,
    #[serde(default)]
    pub test_afk: bool,
    /// Testing: the whole orb is hidden (`N4N0/Visible` sent false).
    #[serde(default)]
    pub test_hidden: bool,
    /// Calls with VRChat friends (controller/calls.rs).
    #[serde(default)]
    pub calls: CallsStatus,
    /// The avatar's N4N0 prop has the projected screen: N4N0's menu opens there instead of in the overlay.
    #[serde(default)]
    pub screen: bool,
    /// N4N0's menu is open on the avatar's screen.
    #[serde(default)]
    pub screen_open: bool,
    /// VRChat runs in desktop mode (as it says; until it does, while SteamVR doesn't run): no fast walk.
    #[serde(default)]
    pub desktop_mode: bool,
    /// Bumped when something asks the overlay to open N4N0's menu (the wake word alone, a hand on N4N0).
    #[serde(default)]
    pub menu_request: u32,
    /// The last request may close the overlay's menu when it is open (asked again).
    #[serde(default)]
    pub menu_toggle: bool,
    /// The last sentence for the overlay menu's search field, numbered.
    #[serde(default)]
    pub dictation: Option<(u32, String)>,
    /// Seconds before the 3D picture a menu's timer started.
    #[serde(default)]
    pub photo_countdown: Option<u32>,
    /// N4N0's light, when the avatar's N4N0 has one (the Light option).
    #[serde(default)]
    pub light: Option<OrbLight>,
}

/// N4N0's light as the avatar holds it (`N4N0/Light`, `N4N0/Light/Color`, `N4N0/Light/Level`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrbLight {
    pub on: bool,
    pub color: u8,
    pub level: f32,
}

/// N4N0 calls as the interface shows them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CallsStatus {
    pub link: CallLink,
    /// Friends who can be called now.
    pub contacts: Vec<CallContact>,
    pub call: Option<CallView>,
    /// N4N0's panel lists the contacts, and which one is chosen.
    pub menu_open: bool,
    pub picked: usize,
}

/// Whether this member can use calls.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CallLink {
    #[default]
    Off,
    SignedOut,
    /// Neither VRCX, VRCNext nor VRChat's log says which VRChat account is used here.
    NoAccount,
    /// This VRChat account is not the member's verified link on Orbiters yet.
    NotLinked {
        account: String,
        source: String,
    },
    Linked {
        account: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallContact {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallPhase {
    /// Our call rings at the friend's.
    Calling,
    /// A friend's call rings here.
    Ringing,
    Connecting,
    Connected,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallView {
    pub name: String,
    pub phase: CallPhase,
    pub incoming: bool,
    /// When it connected (Unix ms).
    pub connected_at_ms: Option<u64>,
    pub muted: bool,
    pub mic_level: f32,
    pub remote_level: f32,
}

impl OrbFace {
    /// Every state, in `N4N0/State`'s order.
    pub const ALL: [OrbFace; 8] =
        [OrbFace::Idle, OrbFace::Listening, OrbFace::Thinking, OrbFace::Talking, OrbFace::Sleeping, OrbFace::Happy, OrbFace::Annoyed, OrbFace::Surprised];
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
    pub uploads: Vec<UploadStatus>,
    #[serde(default)]
    pub orb: OrbStatus,
    #[serde(default)]
    pub update: UpdateStatus,
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
            },
            sources: Vec::new(),
            mics: Vec::new(),
            osc: OscStatus { enabled: settings.osc.enabled, ..Default::default() },
            capture: CaptureStatus {
                state: CaptureState::Idle,
                session_dir: None,
                source: None,
                stats: None,
                error: None,
                trail: Vec::new(),
                from_osc: false,
                coverage: None,
            },
            storage: StorageStatus { sessions_dir: settings.sessions_dir.clone(), limit_gb: settings.storage_limit_gb, ..Default::default() },
            settings,
            worker: WorkerStatus::default(),
            steamvr: SteamVrStatus::default(),
            reconstruction: None,
            reconstruct_queue: Vec::new(),
            account: AccountStatus::default(),
            uploads: Vec::new(),
            orb: OrbStatus::default(),
            update: UpdateStatus::default(),
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
        } else if self.orb.calls.call.is_some() {
            Some("the call")
        } else if self.uploads.iter().any(UploadStatus::running) {
            Some("the upload")
        } else if self.orb.voice.downloading.is_some() {
            Some("the speech models' download")
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
