//! Shell ↔ worker protocol: one JSON object per line on the worker's stdin (commands) and stdout (events).
//! The worker logs to stderr. Both sides must run the same app version (`Ready.version`).

use crate::contracts::manifest::{CaptureMode, ConsentInfo, SessionManifest, StorageQuality, WorldInfo};
use crate::contracts::performance::calibration::Calibration;
use crate::osc::OscValue;
use crate::settings::KeyframeSettings;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum WorkerCommand {
    /// Reply with `Sources`.
    ListSources,
    Start(Box<StartCapture>),
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
    /// A T-pose calibration made now (`time_ns` is a [`crate::clock::now_ns`] reading), for the running session.
    Calibration {
        calibration: Box<Calibration>,
    },
    /// VRChat runs as process `pid` now (it started after the session did, or restarted): the running session records
    /// what it plays from now on. Sent while recording with VRChat's sound on.
    GameAudio {
        pid: u32,
    },
    /// On a Mac: the folder in which the engine of the person's own VRChat gives its stream camera to programs outside
    /// it (`obelisk::host_folder`); none while it does not run. Nothing elsewhere.
    Host {
        #[serde(default)]
        folder: Option<PathBuf>,
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
    /// A Spout sender; an empty name picks VRChat's sender when present, else the active one. `extra` senders are read
    /// beside it whenever they send (VRChat's camera dolly's other streams, `VRCSender2` to `VRCSender4`): their frames
    /// carry the capture strip of their own camera and join the session as views of their own.
    Spout {
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extra: Vec<String>,
    },
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
    /// The latest T-pose calibration, made before the session (`time_ns` is a [`crate::clock::now_ns`] reading).
    pub calibration: Option<Calibration>,
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

/// What a capture's depth frames have seen, from above (the capture page's coverage map): cells of `cell_m` metres on
/// the horizontal plane (Unity world x, z), row-major from (`x0`, `z0`), `width` along x and `depth` along z. Two bytes
/// a cell: how many of the 16 compass directions it was seen from (0: not seen), then [`coverage`] bits.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CoverageMap {
    pub cell_m: f32,
    pub x0: f32,
    pub z0: f32,
    pub width: u32,
    pub depth: u32,
    #[serde(with = "hex")]
    pub cells: Vec<u8>,
    /// The floor's height under the walked path (Unity world y), once found.
    pub floor_y: Option<f32>,
    /// What to do next, the most useful first (at most three).
    pub hints: Vec<String>,
}

/// The second byte of a [`CoverageMap`] cell: from how high the camera saw it, and what lies there.
pub mod coverage {
    /// Seen from below (the camera more than 10° under it).
    pub const FROM_BELOW: u8 = 1 << 0;
    /// Seen within 10° of level: a floor seen like this was only seen from afar.
    pub const GRAZING: u8 = 1 << 1;
    /// Seen from 10–35° above.
    pub const LOW: u8 = 1 << 2;
    /// From 35–60° above.
    pub const HIGH: u8 = 1 << 3;
    /// From more than 60° above.
    pub const OVERHEAD: u8 = 1 << 4;
    /// Points about 0.4–2.5 m above the floor: something a camera at eye height would run into.
    pub const OBSTACLE: u8 = 1 << 6;
    /// Points at the floor's height.
    pub const FLOOR: u8 = 1 << 7;
}

impl CoverageMap {
    /// Compass directions and [`coverage`] bits of the cell holding world (`x`, `z`); `None` outside the map.
    pub fn at(&self, x: f32, z: f32) -> Option<(u8, u8)> {
        let (fx, fz) = ((x - self.x0) / self.cell_m, (z - self.z0) / self.cell_m);
        if fx < 0.0 || fz < 0.0 || fx >= self.width as f32 || fz >= self.depth as f32 {
            return None;
        }
        let i = 2 * (fz as usize * self.width as usize + fx as usize);
        Some((*self.cells.get(i)?, *self.cells.get(i + 1)?))
    }
}

/// Bytes as a hex string: half the size of a JSON array of numbers.
mod hex {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let text: String = bytes.iter().flat_map(|b| [DIGITS[(b >> 4) as usize] as char, DIGITS[(b & 15) as usize] as char]).collect();
        s.serialize_str(&text)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(text.get(2 * i..2 * i + 2).ok_or_else(|| D::Error::custom("hex"))?, 16).map_err(D::Error::custom))
            .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureStats {
    pub frames: u64,
    pub strip_ok: u64,
    pub strip_failed: u64,
    pub keyframes: u64,
    pub depth_frames: u64,
    /// Depth frames whose capture kit's depth probe ran (their stamp says so: contracts' `depth::probe_stamp`): depth
    /// where the world leaves Unity's depth texture empty. 0 with a kit from before Orbiters Toolkit 0.5.2.
    pub depth_probed: u64,
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
    /// Frames whose capture-kit skeleton band was read.
    pub skeleton_frames: u64,
    /// Depth frames dropped because decoding them fell behind.
    pub depth_dropped: u64,
    /// Faces of the capture rig's frames recorded as views of their own (contracts' Docs/rig-frames.md), and faces left
    /// out because their camera rendered in another frame than the strip's (or their stamp could not be read).
    pub rig_faces: u64,
    pub rig_faces_late: u64,
    /// Frames from the extra senders (the camera dolly's other streams) and the keyframes they gave; their frames of
    /// another size than the main stream's, left out.
    #[serde(default)]
    pub extra_frames: u64,
    #[serde(default)]
    pub extra_keyframes: u64,
    #[serde(default)]
    pub extra_other_size: u64,
    /// Seconds since a frame's strip was last read, or since the first frame when none was yet (0 while it is read):
    /// frames without it have no camera pose, so no keyframe. The service says why (`controller::hide_self::NoStrip`).
    pub strip_missing_s: f32,
    /// What went wrong that the recording goes on without (a microphone that could not be opened, files that could
    /// not be written), the latest of each kind. The strip's state is in `last_error`. The worker also sends each as
    /// an `Error` event, once.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
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
    /// The avatar's hips as the latest colour frame's skeleton band gave them (world position, and rotation x, y, z,
    /// w), at most about 30 times a second: where N4N0's camera rig hangs from (contracts' `Docs/rig-frames.md`).
    Hips {
        position: [f32; 3],
        rotation: [f32; 4],
    },
    /// About once a second while recording: what the depth frames have seen so far.
    Coverage(CoverageMap),
    Stopped {
        session_dir: PathBuf,
        manifest: Box<SessionManifest>,
        reason: String,
    },
    /// `Start` could not open its source or create its session: nothing records (a previous session was finished).
    StartFailed {
        message: String,
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

    /// Reads one line of the worker's output. A number the worker could not write (serde_json writes NaN as `null`,
    /// which no float reads) costs that field, not the event: a `Stopped` lost to it would leave the app saving the
    /// session until the worker is restarted. Other events must read whole.
    pub fn from_line(line: &str) -> Option<WorkerEvent> {
        if let Ok(event) = serde_json::from_str(line) {
            return Some(event);
        }
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        match value["event"].as_str()? {
            "stats" => Some(WorkerEvent::Stats(crate::json_lenient::<CaptureStats>(line).0)),
            "coverage" => Some(WorkerEvent::Coverage(crate::json_lenient::<CoverageMap>(line).0)),
            "stopped" => Some(WorkerEvent::Stopped {
                session_dir: serde_json::from_value(value["session_dir"].clone()).ok()?,
                manifest: Box::new(crate::json_lenient::<SessionManifest>(&value["manifest"].to_string()).0),
                reason: value["reason"].as_str().unwrap_or_default().to_string(),
            }),
            _ => None,
        }
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
        let c = WorkerCommand::Start(Box::new(StartCapture {
            source: SourceSpec::Spout { name: "VRChat".into(), extra: Vec::new() },
            sessions_dir: "C:/s".into(),
            mode: CaptureMode::RecordOnly,
            quality: StorageQuality::High,
            keyframes: KeyframeSettings::default(),
            world: None,
            consent: ConsentInfo::default(),
            folder_name: None,
            performance: PerformanceSpec::default(),
        }));
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

    #[test]
    fn a_number_written_as_null_costs_the_field_not_the_event() {
        let stats = WorkerEvent::Stats(CaptureStats { frames: 3, fps: f32::NAN, ..Default::default() }).to_line();
        assert!(stats.contains("\"fps\":null"), "{stats}");
        assert!(serde_json::from_str::<WorkerEvent>(&stats).is_err(), "strictly, the line is lost");
        assert!(matches!(WorkerEvent::from_line(&stats), Some(WorkerEvent::Stats(s)) if s.frames == 3 && s.fps == 0.0));
        let mut manifest = SessionManifest::new("id".into(), "2026-10-21T00:00:00Z".into(), "0.1.0");
        manifest.stats.duration_s = f64::NAN;
        manifest.stats.frames = 9;
        let stopped = WorkerEvent::Stopped { session_dir: "C:/s/x".into(), manifest: Box::new(manifest), reason: "stopped".into() }.to_line();
        match WorkerEvent::from_line(&stopped) {
            Some(WorkerEvent::Stopped { session_dir, manifest, reason }) => {
                assert_eq!((session_dir, manifest.stats.frames, reason.as_str()), (PathBuf::from("C:/s/x"), 9, "stopped"));
                assert_eq!(manifest.id, "id");
            }
            other => panic!("{other:?}"),
        }
        assert!(WorkerEvent::from_line("{\"event\":\"error\",\"message\":7}").is_none());
    }
}
