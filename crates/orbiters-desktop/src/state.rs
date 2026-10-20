//! Everything the interface shows lives in one [`Snapshot`], republished on every change.

use orbiters_core::protocol::{AudioDeviceInfo, CaptureStats, SpoutSourceInfo};
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
