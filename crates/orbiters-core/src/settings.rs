//! User settings, stored as JSON in the config folder. Unknown fields are kept out, missing ones defaulted.

use crate::contracts::manifest::{CaptureMode, StorageQuality};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub sessions_dir: PathBuf,
    pub raw_captures_dir: PathBuf,
    pub capture_mode: CaptureMode,
    pub storage_quality: StorageQuality,
    /// Spout sender to read; empty means "VRChat if present, else the active sender".
    pub spout_sender: String,
    pub keyframes: KeyframeSettings,
    /// Disk space (GB) the session library may use; capture stops when it is reached. 0 = unlimited.
    pub storage_limit_gb: f64,
    pub osc: OscSettings,
    pub performance: PerformanceSettings,
    /// Start a new session automatically when VRChat changes world while recording.
    pub split_on_world_change: bool,
    /// The person capturing acknowledged the consent notice (own worlds or creator consent, private results).
    pub consent_acknowledged: bool,
    /// Reduce motion in the interface.
    pub reduce_motion: bool,
    /// Show Orbiters in SteamVR's dashboard while SteamVR runs.
    pub steamvr_overlay: bool,
    /// Orb, the Orbiters companion on the avatar.
    pub orb: OrbSettings,
    /// Share VRChat's frame rate in each world with Orbiters, a summary a minute (contracts `Docs/world-performance.md`).
    pub world_performance: bool,
    /// Walk boost: the play space moves along with the movement stick, so walking in VRChat goes faster.
    pub walk_boost: WalkBoostSettings,
}

/// Walk boost (the SteamVR overlay): while the stick VRChat walks with is pushed, the play space is dragged the same
/// way, so the avatar covers more ground. The chaperone stays on the room's walls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WalkBoostSettings {
    pub enabled: bool,
    /// 0 to 1: how much faster, up to [`WalkBoostSettings::MAX_EXTRA`] times VRChat's running speed on top of it.
    pub strength: f32,
    /// The hand whose stick VRChat walks with.
    pub hand: Hand,
    /// Which way "forward" is: where the head looks (VRChat's default) or where that hand points.
    pub follow: Follow,
}

impl WalkBoostSettings {
    pub const MAX_EXTRA: f32 = 2.0;
}

impl Default for WalkBoostSettings {
    fn default() -> Self {
        Self { enabled: false, strength: 0.5, hand: Hand::Left, follow: Follow::Head }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Hand {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Follow {
    #[default]
    Head,
    Hand,
}

/// Orb, the Orbiters companion (contracts `Docs/orb.md`): its helpers, and its assistant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OrbSettings {
    /// Orb's helpers run: the text panel, the AFK message, held and carried parameters, saved videos.
    pub enabled: bool,
    /// Orb's assistant: listening and answers. Off, Orb is a tool without AI: nothing is transcribed or sent.
    pub assistant: bool,
    /// Listen for the wake word in everything the microphone hears. Sentences are transcribed on this computer; only
    /// those that start with a wake word go further.
    pub wake_word: bool,
    /// The words that wake Orb at the start of a sentence ("hey", "ok" or "dis" before them are fine).
    pub wake_words: Vec<String>,
    /// Mute VRChat's microphone while the menu's Talk button is held, so the sentence for Orb stays out of the world.
    pub mute_while_talking: bool,
    /// Microphone Orb listens to (endpoint id; empty for the Windows default).
    pub mic_device: String,
    /// Milliseconds between two steps of the text panel (VRChat loses faster ones for other players).
    pub text_step_ms: u64,
    /// While VRChat says you are away, the panel shows `afk_message`.
    pub afk_message_enabled: bool,
    pub afk_message: String,
    /// Parameters whose values follow you to your next avatar (GoGoLoco's pose and height by default).
    pub carry_parameters: Vec<String>,
    /// Parameters held at a value: put back whenever something else changes them.
    pub pins: Vec<Pin>,
}

/// A parameter held at a value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    pub parameter: String,
    pub value: crate::osc::OscValue,
}

/// GoGoLoco's parameters that hold a pose, its height and its puppet: carried to the next avatar. `VRCEmote` (the
/// pose itself) is carried only to an avatar that has GoGoLoco too.
pub const GOGOLOCO_PARAMETERS: &[&str] = &[
    "VRCEmote",
    "Go/Stationary",
    "Go/StandIdle",
    "Go/CrouchIdle",
    "Go/ProneIdle",
    "Go/Float",
    "Go/Height",
    "Go/PuppetX",
    "Go/PuppetY",
    "Go/Horizon",
    "Go/Locomotion",
    "Go/PoseRadius",
    "Go/PoseY",
];

impl Default for OrbSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            assistant: true,
            wake_word: false,
            wake_words: vec!["orb".into(), "orbe".into()],
            mute_while_talking: true,
            mic_device: String::new(),
            text_step_ms: 300,
            afk_message_enabled: false,
            afk_message: "Away for a moment, back soon!".into(),
            carry_parameters: GOGOLOCO_PARAMETERS.iter().map(|s| s.to_string()).collect(),
            pins: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyframeSettings {
    /// Metres the camera must move before the next keyframe.
    pub min_translation: f32,
    /// Degrees the camera must turn before the next keyframe.
    pub min_rotation_deg: f32,
    /// Seconds after which a keyframe is taken anyway.
    pub max_interval_s: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OscSettings {
    pub enabled: bool,
    /// Avatar parameter (bool) that toggles recording from the Action Menu.
    pub record_parameter: String,
    /// Port VRChat listens on (camera and HUD messages are sent there).
    pub vrchat_port: u16,
    /// Record every avatar parameter VRChat sends (face tracking, gestures, visemes, locomotion) with sessions.
    pub record_parameters: bool,
    /// While recording from Spout, open VRChat's camera in Stream mode with Spout Stream on (OSC), and put it back
    /// as it was afterwards.
    pub drive_camera: bool,
}

/// What a session records besides the frames (avatar parameters are under [`OscSettings`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceSettings {
    /// Record the microphone (the person's voice).
    pub record_mic: bool,
    /// Microphone endpoint id; empty for the Windows default.
    pub mic_device: String,
    /// Record what VRChat plays (world sounds, other players' voices).
    pub record_game_audio: bool,
    /// Record SteamVR headset, controller and tracker poses (body movement).
    pub record_tracking: bool,
    /// Samples per second of the tracking stream.
    pub tracking_rate_hz: f32,
}

impl Default for PerformanceSettings {
    fn default() -> Self {
        Self { record_mic: false, mic_device: String::new(), record_game_audio: false, record_tracking: true, tracking_rate_hz: 90.0 }
    }
}

impl Default for KeyframeSettings {
    fn default() -> Self {
        Self { min_translation: 0.3, min_rotation_deg: 15.0, max_interval_s: 2.0 }
    }
}

impl Default for OscSettings {
    fn default() -> Self {
        Self { enabled: true, record_parameter: "OrbitersRecord".into(), vrchat_port: 9000, record_parameters: true, drive_camera: true }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sessions_dir: crate::paths::default_sessions_dir(),
            raw_captures_dir: crate::paths::default_raw_captures_dir(),
            capture_mode: CaptureMode::RecordOnly,
            storage_quality: StorageQuality::High,
            spout_sender: String::new(),
            keyframes: KeyframeSettings::default(),
            storage_limit_gb: 200.0,
            osc: OscSettings::default(),
            performance: PerformanceSettings::default(),
            split_on_world_change: true,
            consent_acknowledged: false,
            reduce_motion: false,
            steamvr_overlay: true,
            orb: OrbSettings::default(),
            world_performance: false,
            walk_boost: WalkBoostSettings::default(),
        }
    }
}

impl Settings {
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn load() -> Self {
        Self::load_from(&crate::paths::settings_file())
    }

    /// Writes atomically (temporary file, then rename).
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&crate::paths::settings_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("orbiters-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let s = Settings { storage_limit_gb: 12.5, capture_mode: CaptureMode::LiveRefine, ..Default::default() };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        std::fs::write(&path, r#"{"storage_limit_gb": 3}"#).unwrap();
        let partial = Settings::load_from(&path);
        assert_eq!(partial.storage_limit_gb, 3.0);
        assert_eq!(partial.osc.record_parameter, "OrbitersRecord");
        assert_eq!(partial.orb, OrbSettings::default(), "settings from before Orb get its defaults");
        std::fs::remove_dir_all(dir).ok();
    }
}
