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
    /// How long a 3D picture's camera flight lasts (more seconds, more of the place seen).
    pub photo_3d: Photo3dLength,
    /// Seconds before a 3D picture taken from Orb's menu starts (0: at once).
    pub photo_3d_timer: u32,
}

/// A 3D picture's length: a gentle 1 s sweep, 4 s that circle you and fly 10 m out every way, or 8 s that also circle
/// four points 8 m away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Photo3dLength {
    Quick,
    #[default]
    Standard,
    Full,
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
    /// Calls with VRChat friends who have Orbiters and Orb too (contracts `Docs/orb-calls.md`): presence and the
    /// friends this computer's VRCX and VRCNext know go to Orbiters (as hashes) while on.
    pub calls: bool,
    /// Calls may go straight between the two computers (a little faster on one network), each side then seeing the
    /// other's addresses. Off, they go through the relay, which shows the friend only its own.
    pub direct_calls: bool,
    /// Orb's menu (fast walk, YouTube, calls, a 3D picture): what opens it, the keyboard's letters.
    pub menu: MenuSettings,
}

/// What opens Orb's menu: in the SteamVR overlay, or on the avatar's projected screen when the Orb prop has one (the
/// "Replace Orbiters overlay" option of the orb in My Avatar). Opening it also starts Orb listening.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuSettings {
    /// The SteamVR controller binding "Open Orb's menu" (by default a double press of the left stick, changed in
    /// SteamVR's controller settings for Orbiters).
    pub button: bool,
    /// Saying the wake word on its own ("Orb!") while the wake word is on.
    pub wake_word: bool,
    /// Touching the orb with a hand.
    pub touch: bool,
    /// The search keyboard's letters.
    pub keyboard: KeyboardLayout,
}

impl Default for MenuSettings {
    fn default() -> Self {
        Self { button: true, wake_word: true, touch: true, keyboard: KeyboardLayout::Auto }
    }
}

/// The order of the letters on Orb's keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardLayout {
    /// As this computer's keyboard: AZERTY for French and Belgian layouts, QWERTY otherwise.
    #[default]
    Auto,
    Qwerty,
    Azerty,
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
            calls: false,
            direct_calls: false,
            menu: MenuSettings::default(),
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
            photo_3d: Photo3dLength::default(),
            photo_3d_timer: 0,
        }
    }
}

impl Settings {
    /// Reads the settings: a field that does not fit (hand-edited, or written by a newer version) gets its default
    /// and every other keeps its value ([`crate::json_lenient`]). Such a file is first kept beside as
    /// `settings.json.bad-<time>`, so that the next save loses nothing for good. No file gives the defaults.
    pub fn load_from(path: &Path) -> Self {
        let text = match crate::read_text(path) {
            Ok(Some(text)) => text,
            Ok(None) => return Self::default(),
            Err(e) => {
                // Unreadable for now: the defaults, and the file kept, as the next save will replace it.
                eprintln!("settings: {}: {e}", path.display());
                crate::keep_aside(path);
                return Self::default();
            }
        };
        let (settings, dropped) = crate::json_lenient::<Self>(&text);
        if !dropped.is_empty() {
            let aside = crate::keep_aside(path);
            eprintln!("settings: {} left at their default ({}), the file kept as {:?}", dropped.join(", "), path.display(), aside);
        }
        settings
    }

    pub fn load() -> Self {
        Self::load_from(&crate::paths::settings_file())
    }

    /// Writes the whole file at once ([`crate::write_atomic`]): a crash, a power cut or another save at the same time
    /// leaves the old settings or the new ones.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&crate::paths::settings_file())
    }

    /// These settings with what `patch` changes: a JSON object of the fields to change and their values, a section as
    /// an object of its own changed fields (`{"orb": {"wake_word": true}}`; a list, the pins say, is replaced whole).
    /// Each field goes onto the settings as they are, so whatever else changed meanwhile stays; a field the settings do
    /// not have, or a value that does not fit it, is left out. Returns the settings and the fields left out.
    pub fn patched(&self, patch: &serde_json::Value) -> (Settings, Vec<String>) {
        let (Ok(mut current), Some(fields)) = (serde_json::to_value(self), patch.as_object()) else { return (self.clone(), vec!["(not an object)".into()]) };
        // The values to set, by path: a section is walked into while the settings have a section there too.
        let mut changes = Vec::new();
        let mut todo: Vec<(Vec<String>, &serde_json::Value)> = fields.iter().map(|(k, v)| (vec![k.clone()], v)).collect();
        while let Some((path, value)) = todo.pop() {
            match (value.as_object(), field(&current, &path).is_some_and(serde_json::Value::is_object)) {
                (Some(inner), true) => todo.extend(inner.iter().map(|(k, v)| ([path.as_slice(), std::slice::from_ref(k)].concat(), v))),
                _ => changes.push((path, value)),
            }
        }
        changes.sort_by(|a, b| a.0.cmp(&b.0));
        let mut left_out = Vec::new();
        for (path, value) in changes {
            let mut candidate = current.clone();
            match field_mut(&mut candidate, &path) {
                Some(slot) => *slot = value.clone(),
                None => {
                    left_out.push(path.join("."));
                    continue;
                }
            }
            if serde_json::from_value::<Settings>(candidate.clone()).is_ok() {
                current = candidate;
            } else {
                left_out.push(path.join("."));
            }
        }
        (serde_json::from_value(current).unwrap_or_else(|_| self.clone()), left_out)
    }
}

impl Settings {
    /// What changed from these settings to `other`, as a patch for [`Settings::patched`]: the changed fields, a section
    /// as an object of its own changed fields, a list or a text whole, and `null` for a value `other` no longer has.
    /// A window that changed its copy sends this rather than the copy: `base.patched(&base.diff(&mine))` is `mine`,
    /// and on settings that changed meanwhile it changes only what the window did.
    pub fn diff(&self, other: &Settings) -> serde_json::Value {
        let empty = || serde_json::Value::Object(serde_json::Map::new());
        match (serde_json::to_value(self), serde_json::to_value(other)) {
            (Ok(a), Ok(b)) => changed(&a, &b).unwrap_or_else(empty),
            _ => empty(),
        }
    }
}

/// `b` where it differs from `a`: objects field by field, keeping only what changed; anything else whole.
fn changed(a: &serde_json::Value, b: &serde_json::Value) -> Option<serde_json::Value> {
    use serde_json::Value;
    if a == b {
        return None;
    }
    let (Value::Object(a), Value::Object(b)) = (a, b) else { return Some(b.clone()) };
    let mut fields: serde_json::Map<String, Value> =
        b.iter().filter_map(|(k, v)| match a.get(k) { Some(old) => changed(old, v).map(|c| (k.clone(), c)), None => Some((k.clone(), v.clone())) }).collect();
    fields.extend(a.keys().filter(|k| !b.contains_key(*k)).map(|k| (k.clone(), Value::Null)));
    Some(Value::Object(fields))
}

/// The value at `path` in a JSON object, when there is one.
fn field<'v>(value: &'v serde_json::Value, path: &[String]) -> Option<&'v serde_json::Value> {
    path.iter().try_fold(value, |v, key| v.as_object()?.get(key))
}

fn field_mut<'v>(value: &'v mut serde_json::Value, path: &[String]) -> Option<&'v mut serde_json::Value> {
    path.iter().try_fold(value, |v, key| v.as_object_mut()?.get_mut(key))
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

    #[test]
    fn a_patch_changes_its_fields_only() {
        let mut now = Settings::default();
        // Changed by the service since the window read its copy.
        now.walk_boost.strength = 0.9;
        now.orb.pins = vec![Pin { parameter: "Hoodie".into(), value: crate::osc::OscValue::Bool(true) }];
        let patch = serde_json::json!({
            "storage_limit_gb": 50,
            "orb": { "wake_word": true, "menu": { "keyboard": "azerty", "layout": "dvorak" }, "pins": [] },
            "walk_boost": { "enabled": true, "hand": "both" },
            "osc": { "vrchat_port": 70000 },
            "no_such_setting": 1,
        });
        let (patched, left_out) = now.patched(&patch);
        assert_eq!(patched.storage_limit_gb, 50.0);
        assert!(patched.orb.wake_word && patched.walk_boost.enabled);
        assert_eq!(patched.orb.menu.keyboard, KeyboardLayout::Azerty);
        assert!(patched.orb.pins.is_empty(), "a list is replaced whole");
        assert_eq!(patched.walk_boost.strength, 0.9, "what changed meanwhile stays");
        assert_eq!((patched.walk_boost.hand, patched.osc.vrchat_port), (Hand::Left, 9000), "values that do not fit are left out");
        assert_eq!(left_out, vec!["no_such_setting", "orb.menu.layout", "osc.vrchat_port", "walk_boost.hand"]);
        assert_eq!(now.patched(&serde_json::json!([1])).0, now, "not an object: nothing changes");
    }

    #[test]
    fn a_diff_carries_only_what_changed() {
        let base = Settings::default();
        // A window flips a switch, types an AFK message and replaces the pins on its copy.
        let mut mine = base.clone();
        mine.reduce_motion = !base.reduce_motion;
        mine.orb.afk_message = "Back soon".into();
        mine.orb.pins = vec![Pin { parameter: "Hoodie".into(), value: crate::osc::OscValue::Bool(true) }];
        let patch = base.diff(&mine);
        assert_eq!(
            patch,
            serde_json::json!({ "reduce_motion": mine.reduce_motion, "orb": { "afk_message": "Back soon", "pins": serde_json::to_value(&mine.orb.pins).unwrap() } })
        );
        assert_eq!(base.patched(&patch), (mine.clone(), vec![]));
        // Meanwhile the avatar's radial moved: the patch keeps that.
        let mut now = base.clone();
        now.walk_boost.strength = 0.8;
        let (patched, left_out) = now.patched(&patch);
        assert!(left_out.is_empty());
        assert_eq!((patched.walk_boost.strength, patched.reduce_motion, patched.orb.afk_message.as_str()), (0.8, mine.reduce_motion, "Back soon"));
        // Nothing changed: an empty patch.
        assert_eq!(base.diff(&base), serde_json::json!({}));
    }

    #[test]
    fn a_field_that_does_not_fit_keeps_the_others() {
        let dir = std::env::temp_dir().join(format!("orbiters-settings-lenient-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        // A port out of range, a keyboard from a newer version, a byte order mark from an editor.
        let text = "\u{feff}{\"consent_acknowledged\": true, \"storage_limit_gb\": 7, \
                    \"osc\": {\"vrchat_port\": 70000, \"record_parameter\": \"MyRecord\"}, \
                    \"orb\": {\"wake_word\": true, \"menu\": {\"keyboard\": \"dvorak\"}}}";
        std::fs::write(&path, text).unwrap();
        let s = Settings::load_from(&path);
        assert!(s.consent_acknowledged, "consent survives a bad field elsewhere");
        assert_eq!(s.storage_limit_gb, 7.0);
        assert_eq!(s.osc.record_parameter, "MyRecord");
        assert_eq!(s.osc.vrchat_port, OscSettings::default().vrchat_port);
        assert!(s.orb.wake_word);
        assert_eq!(s.orb.menu, MenuSettings::default());
        let kept = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().starts_with("settings.json.bad-"));
        assert!(kept, "the file that needed repair is kept beside");
        std::fs::remove_dir_all(dir).ok();
    }
}
