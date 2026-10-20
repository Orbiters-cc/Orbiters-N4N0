//! Shell ↔ worker protocol: one JSON object per line on the worker's stdin (commands) and stdout (events).
//! The worker logs to stderr. Both sides must run the same app version (`Ready.version`).

use crate::contracts::manifest::{CaptureMode, ConsentInfo, SessionManifest, StorageQuality, WorldInfo};
use crate::osc::OscValue;
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
    /// OSC values VRChat sent, for the running session's `parameters.bin`.
    Osc {
        samples: Vec<OscSample>,
    },
    Quit,
}

/// One OSC value with the time it arrived ([`crate::clock::now_ns`]). Messages with several arguments
/// (`/tracking/vrsystem/head/pose`) give one sample per argument (`index`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OscSample {
    pub at_ns: u64,
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u8>,
    pub value: OscValue,
    /// Known before the session started (written at time 0).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub initial: bool,
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
    /// What to record besides the frames.
    #[serde(default)]
    pub performance: PerformanceSpec,
}

/// Performance streams of a session (avatar parameters arrive as `Osc` commands).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceSpec {
    /// Record a microphone: its endpoint id, empty for the Windows default.
    pub mic: Option<String>,
    /// Record what this process (VRChat) plays.
    pub game_audio_pid: Option<u32>,
    /// Record SteamVR device poses at this rate (waits for SteamVR when it is not running).
    pub tracking_hz: Option<f32>,
}

/// A microphone the worker can record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    /// The Windows default microphone.
    pub default: bool,
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
    /// Avatar parameter changes recorded from OSC.
    pub parameter_changes: u64,
    /// Peak level (0–1) of the microphone since the last stats; `None` when it is not recorded.
    pub mic_level: Option<f32>,
    /// Peak level of VRChat's sound since the last stats; `None` when it is not recorded.
    pub game_level: Option<f32>,
    /// SteamVR tracking: `None` when it is not recorded, else whether SteamVR is connected.
    pub tracking_connected: Option<bool>,
    pub tracking_devices: u64,
    pub tracking_samples: u64,
    /// T-poses kept as calibrations.
    pub calibrations: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WorkerEvent {
    Ready {
        version: String,
    },
    Sources {
        spout: Vec<SpoutSourceInfo>,
        #[serde(default)]
        mics: Vec<AudioDeviceInfo>,
    },
    Started {
        session_dir: PathBuf,
        source: String,
    },
    Stats(CaptureStats),
    Stopped {
        session_dir: PathBuf,
        manifest: Box<SessionManifest>,
        reason: String,
    },
    Error {
        message: String,
    },
    Log {
        level: String,
        message: String,
    },
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
            performance: PerformanceSpec::default(),
        });
        let line = c.to_line();
        assert!(line.contains(r#""cmd":"start""#) && line.contains(r#""kind":"spout""#));
        assert_eq!(serde_json::from_str::<WorkerCommand>(&line).unwrap(), c);
        let e = WorkerEvent::Stats(CaptureStats { frames: 3, ..Default::default() });
        assert!(e.to_line().starts_with(r#"{"event":"stats","frames":3"#));
        assert_eq!(serde_json::from_str::<WorkerCommand>(r#"{"cmd":"stop"}"#).unwrap(), WorkerCommand::Stop);
        let osc = WorkerCommand::Osc {
            samples: vec![
                OscSample { at_ns: 5, address: "/avatar/parameters/Viseme".into(), index: None, value: OscValue::Int(3), initial: true },
                OscSample { at_ns: 9, address: "/tracking/vrsystem/head/pose".into(), index: Some(4), value: OscValue::Float(90.0), initial: false },
            ],
        };
        let line = osc.to_line();
        assert_eq!(
            line,
            r#"{"cmd":"osc","samples":[{"at_ns":5,"address":"/avatar/parameters/Viseme","value":{"type":"int","value":3},"initial":true},{"at_ns":9,"address":"/tracking/vrsystem/head/pose","index":4,"value":{"type":"float","value":90.0}}]}"#
        );
        assert_eq!(serde_json::from_str::<WorkerCommand>(&line).unwrap(), osc);
    }
}
