//! Shell ↔ worker protocol: one JSON object per line on the worker's stdin (commands) and stdout (events).
//! The worker logs to stderr. Both sides must run the same app version (`Ready.version`).

use crate::contracts::manifest::{CaptureMode, ConsentInfo, SessionManifest, StorageQuality, WorldInfo};
use crate::settings::KeyframeSettings;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum WorkerCommand {
    /// Reply with `Sources`.
    ListSources,
    Start(StartCapture),
    /// Finish the current session (manifest written), keep the worker running.
    Stop,
    Pause,
    Resume,
    /// Update the world of the running session (from VRChat's log).
    SetWorld {
        world: Option<WorldInfo>,
    },
    Quit,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceSpec {
    /// A Spout sender; an empty name picks VRChat's sender when present, else the active one.
    Spout { name: String },
    /// A raw capture folder (Mixer's simulator). `realtime` paces frames at the capture's fps.
    RawSequence { path: PathBuf, realtime: bool },
    /// Generated frames with valid strips and depth frames, for checks without VRChat or Unity.
    TestPattern { width: u32, height: u32, fps: f32, depth_every: u32, frames: u32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartCapture {
    pub source: SourceSpec,
    pub sessions_dir: PathBuf,
    pub mode: CaptureMode,
    pub quality: StorageQuality,
    pub keyframes: KeyframeSettings,
    pub world: Option<WorldInfo>,
    pub consent: ConsentInfo,
    /// Folder name; defaults to "<world or source> <date time>".
    pub folder_name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SpoutSourceInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureStats {
    pub frames: u64,
    pub strip_ok: u64,
    pub strip_failed: u64,
    pub keyframes: u64,
    pub depth_frames: u64,
    pub gated: u64,
    /// Keyframes dropped because encoding fell behind.
    pub dropped: u64,
    /// Frames received per second (recent average).
    pub fps: f32,
    /// Average strip + depth decode time per frame, milliseconds.
    pub decode_ms: f32,
    pub bytes: u64,
    pub elapsed_s: f64,
    /// Camera position (Unity world) of the last decoded frame.
    pub position: Option<[f32; 3]>,
    /// Camera heading in degrees (0 = +Z).
    pub heading_deg: Option<f32>,
    pub width: u32,
    pub height: u32,
    pub flipped: bool,
    pub last_error: Option<String>,
    pub paused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WorkerEvent {
    Ready { version: String },
    Sources { spout: Vec<SpoutSourceInfo> },
    Started { session_dir: PathBuf, source: String },
    Stats(CaptureStats),
    Stopped { session_dir: PathBuf, manifest: Box<SessionManifest>, reason: String },
    Error { message: String },
    Log { level: String, message: String },
}

impl WorkerEvent {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("event serialises")
    }
}

impl WorkerCommand {
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("command serialises")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shapes() {
        let c = WorkerCommand::Start(StartCapture {
            source: SourceSpec::Spout { name: "VRChat".into() },
            sessions_dir: "C:/s".into(),
            mode: CaptureMode::RecordOnly,
            quality: StorageQuality::High,
            keyframes: KeyframeSettings::default(),
            world: None,
            consent: ConsentInfo::default(),
            folder_name: None,
        });
        let line = c.to_line();
        assert!(line.contains(r#""cmd":"start""#) && line.contains(r#""kind":"spout""#));
        assert_eq!(serde_json::from_str::<WorkerCommand>(&line).unwrap(), c);
        let e = WorkerEvent::Stats(CaptureStats { frames: 3, ..Default::default() });
        assert!(e.to_line().starts_with(r#"{"event":"stats","frames":3"#));
        assert_eq!(serde_json::from_str::<WorkerCommand>(r#"{"cmd":"stop"}"#).unwrap(), WorkerCommand::Stop);
    }
}
