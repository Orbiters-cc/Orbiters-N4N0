//! Everything the interface shows lives in one [`Snapshot`], republished on every change.

use orbiters_core::protocol::{CaptureStats, SpoutSourceInfo};
use orbiters_core::settings::Settings;
use orbiters_core::vrchat_log::WorldState;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureState {
    Idle,
    Starting,
    Recording,
    Paused,
    Stopping,
    Importing,
}

#[derive(Clone, Debug)]
pub struct VrchatStatus {
    pub running: bool,
    pub world: WorldState,
    pub log_found: bool,
}

#[derive(Clone, Debug, Default)]
pub struct OscStatus {
    pub enabled: bool,
    pub listening: bool,
    pub advertising: bool,
    pub port: Option<u16>,
    pub last_event: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug, Default)]
pub struct StorageStatus {
    pub sessions_dir: PathBuf,
    pub used_bytes: u64,
    pub free_bytes: Option<u64>,
    pub limit_gb: f64,
    pub sessions: usize,
}

#[derive(Clone, Debug, Default)]
pub struct WorkerStatus {
    pub ready: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub version: String,
    pub vrchat: VrchatStatus,
    pub sources: Vec<SpoutSourceInfo>,
    pub osc: OscStatus,
    pub capture: CaptureStatus,
    pub settings: Settings,
    pub storage: StorageStatus,
    pub worker: WorkerStatus,
}

impl Snapshot {
    pub fn new(settings: Settings) -> Self {
        Self {
            version: orbiters_core::APP_VERSION.into(),
            vrchat: VrchatStatus { running: false, world: WorldState::default(), log_found: orbiters_core::paths::vrchat_log_dir().is_some() },
            sources: Vec::new(),
            osc: OscStatus { enabled: settings.osc.enabled, ..Default::default() },
            capture: CaptureStatus { state: CaptureState::Idle, session_dir: None, source: None, stats: None, error: None, trail: Vec::new(), from_osc: false },
            storage: StorageStatus { sessions_dir: settings.sessions_dir.clone(), limit_gb: settings.storage_limit_gb, ..Default::default() },
            settings,
            worker: WorkerStatus::default(),
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
