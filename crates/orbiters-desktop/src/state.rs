//! Everything the interface shows lives in one [`Snapshot`], republished on every change.

use orbiters_core::protocol::{AudioDeviceInfo, CaptureStats, SpoutSourceInfo};
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
    Checking { username: Option<String> },
    SignedIn { username: String },
    /// The saved sign-in was refused: expired, or signed out everywhere.
    Expired { username: Option<String> },
    /// Orbiters could not be reached to check it.
    Offline { username: Option<String> },
    /// Finishing in the browser, where `code` is typed.
    SigningIn { code: String, provider: String },
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
    /// The Orbiters overlay is connected (a tab in SteamVR's dashboard).
    pub overlay: bool,
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
}

impl ReconstructionJob {
    pub fn new(session_dir: PathBuf) -> Self {
        Self { session_dir, stage: Stage::Loading, progress: 0.0, iteration: 0, iterations: 0, gaussians: 0, psnr: None, seconds: 0.0 }
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
    Done { added: usize, skipped: usize },
    Failed { message: String },
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
    #[serde(default)]
    pub account: AccountStatus,
    pub uploads: Vec<UploadStatus>,
}

impl Snapshot {
    pub fn new(settings: Settings) -> Self {
        Self {
            version: orbiters_core::APP_VERSION.into(),
            vrchat: VrchatStatus { running: false, pid: None, world: WorldState::default(), log_found: orbiters_core::paths::vrchat_log_dir().is_some() },
            sources: Vec::new(),
            mics: Vec::new(),
            osc: OscStatus { enabled: settings.osc.enabled, ..Default::default() },
            capture: CaptureStatus { state: CaptureState::Idle, session_dir: None, source: None, stats: None, error: None, trail: Vec::new(), from_osc: false },
            storage: StorageStatus { sessions_dir: settings.sessions_dir.clone(), limit_gb: settings.storage_limit_gb, ..Default::default() },
            settings,
            worker: WorkerStatus::default(),
            steamvr: SteamVrStatus::default(),
            reconstruction: None,
            account: AccountStatus::default(),
            uploads: Vec::new(),
        }
    }

    pub fn is_capturing(&self) -> bool {
        matches!(self.capture.state, CaptureState::Starting | CaptureState::Recording | CaptureState::Paused | CaptureState::Importing)
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
                // Thin the older half instead of dropping the start of the path.
                let keep: Vec<[f32; 3]> = self.capture.trail.iter().step_by(2).copied().collect();
                self.capture.trail = keep;
            }
        }
    }
}
