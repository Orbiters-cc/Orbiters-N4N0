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
    /// N4N0, the Orbiters companion on the avatar.
    pub orb: OrbSettings,
    /// Share VRChat's frame rate in each world with Orbiters, a summary a minute (contracts `Docs/world-performance.md`).
    pub world_performance: bool,
    /// Walk boost: the play space moves along with the movement stick, so walking in VRChat goes faster.
    pub walk_boost: WalkBoostSettings,
    /// How long a 3D picture's camera flight lasts (more seconds, more of the place seen).
    pub photo_3d: Photo3dLength,
    /// Seconds before a 3D picture taken from N4N0's menu starts (0: at once).
    pub photo_3d_timer: u32,
    /// Where reconstructions train while VRChat runs.
    pub reconstruct_while_playing: WhilePlaying,
    /// The GPU the window draws on.
    pub window_gpu: WindowGpu,
    /// When the window's 3D view draws on the integrated GPU instead of the window's.
    pub view_gpu: ViewGpu,
    /// Starting with SteamVR (starting with Windows is the person's registry, read where it is shown).
    pub startup: StartupSettings,
    /// Orbiters' development environment instead of orbiters.cc: the server a developer runs on this computer, its
    /// account and its update feed (development builds of Orbiters - N4N0).
    pub development: bool,
    /// SteamVR's tracking smoothed by Orbiters - N4N0's driver (its Tracking page).
    pub tracking: TrackingSettings,
    /// While recording, N4N0's camera rig (contracts' `Docs/rig-frames.md`) turns on for a moment every few metres the
    /// camera goes: six more views around it (left, front, right, back, up, down) for the reconstruction.
    pub rig_views: bool,
    /// Where the SteamVR overlay shows what a recording has covered so far (the Capture page's camera path).
    pub coverage_overlay: CoverageOverlay,
    /// Experimental, VRC+: recordings also read VRChat's camera dolly's other streams (Spout `VRCSender2` to
    /// `VRCSender4`), and a 3D picture flies four cameras with the dolly (multi-stream) instead of moving the camera
    /// alone. The person sets the dolly's Streaming to Multi once in VRChat.
    pub dolly_streams: bool,
}

/// Where the SteamVR overlay shows a recording's coverage (the Capture page's camera path and its hints): above a
/// controller, as a watch shows the time, or nowhere. Never in VRChat's camera: SteamVR draws overlays in the headset
/// only, after VRChat rendered its frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageOverlay {
    Off,
    #[default]
    LeftHand,
    RightHand,
}

/// Tracking smoothing (Orbiters - N4N0's SteamVR driver): on or off, and each kind of device's strength as an index of
/// the menu's choices (`orbiters_orb::menu::SMOOTHING_STRENGTHS`: off, low, medium, high), headset, controllers,
/// trackers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackingSettings {
    pub enabled: bool,
    pub levels: [u8; 3],
}

impl Default for TrackingSettings {
    /// The headset untouched (smoothing it can make people sick), the controllers a little, the trackers more.
    fn default() -> Self {
        Self { enabled: false, levels: [0, 1, 2] }
    }
}

/// SteamVR starting Orbiters - N4N0 with itself, among its startup overlay apps. SteamVR keeps the truth; this is
/// what was last chosen here or seen there, and a choice still to tell it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StartupSettings {
    /// SteamVR starts Orbiters - N4N0 with itself.
    pub steamvr: bool,
    /// `steamvr` was chosen while SteamVR was not running (here, or in the setup): SteamVR is told the next time it runs.
    pub steamvr_pending: bool,
}

/// Where a reconstruction trains while VRChat runs. On the graphics card, unpaced, training took the GPU from the game:
/// on an RTX 4080, VRChat's GPU time per frame went from about 27 to 46 ms and a third of its frames came late while
/// Canine trained; so there it now asks for a low-priority queue, fits its budget to the graphics memory VRChat leaves,
/// and in VR waits for the idle GPU time SteamVR's frame timing shows between VRChat's frames (orbiters-reconstruct's
/// governor). The processor's integrated GPU left the game's frames as they were (within a few percent) but trains
/// about 40 times slower (Canine: 29 s on the RTX 4080, about 18 min on an Intel UHD 770). Or reconstructions wait for
/// VRChat to close, or for the headset to have been off for a minute.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhilePlaying {
    #[default]
    GraphicsCard,
    IntegratedGpu,
    AfterVrchat,
}

/// The GPU the window draws on (chosen when the window opens), its 3D view too unless [`ViewGpu`] moves it. On the processor's integrated GPU
/// the graphics card that drives the monitor only shows the window: Windows has it copy each frame, which takes a
/// GPU-bound game about twice what drawing an interface frame there does (scrolling at 60 frames a second: 3.6 % of
/// its frames instead of 1.6 %, on an RTX 4080), but the 3D view's splats stay off it (orbiting a million of them:
/// 2.6 % instead of 11.4 %, at 27 frames a second instead of 60). docs/igpu-experiment.md has the measurements. The
/// graphics card by default: drawing the interface there costs a game less than the copy does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowGpu {
    IntegratedGpu,
    #[default]
    GraphicsCard,
}

/// When the window's 3D view draws on the processor's integrated GPU while the window draws on the graphics card: only
/// its frames reach the graphics card, as pictures the window shows. Orbiting a million splats then takes a GPU-bound
/// game on an RTX 4080 about what the interface does instead of a tenth of its frames, but moves at about 27 frames a
/// second instead of 60 (docs/igpu-experiment.md). By default only while VRChat runs: without a game there is nothing
/// to spare the graphics card for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewGpu {
    #[default]
    WhilePlaying,
    Always,
    Never,
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

/// N4N0, the Orbiters companion (contracts `Docs/orb.md`): its helpers, and its assistant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OrbSettings {
    /// N4N0's helpers run: the text panel, the AFK message, held and carried parameters, saved videos.
    pub enabled: bool,
    /// N4N0's assistant: listening and answers. Off, N4N0 is a tool without AI: nothing is transcribed or sent.
    pub assistant: bool,
    /// Listen for the wake word in everything the microphone hears. Sentences are transcribed on this computer; only
    /// those that start with a wake word go further.
    pub wake_word: bool,
    /// The words that wake N4N0 at the start of a sentence ("hey", "ok" or "dis" before them are fine).
    pub wake_words: Vec<String>,
    /// Mute VRChat's microphone while the menu's Talk button is held, so the sentence for N4N0 stays out of the world.
    pub mute_while_talking: bool,
    /// Microphone N4N0 listens to (endpoint id; empty for the Windows default).
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
    /// Calls with VRChat friends who have Orbiters and N4N0 too (contracts `Docs/orb-calls.md`): presence and the
    /// friends this computer's VRCX and VRCNext know go to Orbiters (as hashes) while on.
    pub calls: bool,
    /// Calls may go straight between the two computers (a little faster on one network), each side then seeing the
    /// other's addresses. Off, they go through the relay, which shows the friend only its own.
    pub direct_calls: bool,
    /// N4N0's menu (fast walk, YouTube, calls, a 3D picture): what opens it, the keyboard's letters.
    pub menu: MenuSettings,
}

/// What opens N4N0's menu: in the SteamVR overlay, or on the avatar's projected screen when the N4N0 prop has one (the
/// "Replace Orbiters overlay" option of N4N0 in My Avatar). Opening it also starts N4N0 listening.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuSettings {
    /// The SteamVR controller binding "Open N4N0's menu" (by default a double press of the left stick, changed in
    /// SteamVR's controller settings for Orbiters).
    pub button: bool,
    /// Saying the wake word on its own ("Nano!", N4N0 said aloud) while the wake word is on.
    pub wake_word: bool,
    /// Touching N4N0 with a hand.
    pub touch: bool,
    /// The search keyboard's letters.
    pub keyboard: KeyboardLayout,
    /// How the overlay's menu looks (N4N0's screen has its own Look in My Avatar).
    pub look: MenuLook,
    /// How large N4N0's projected screen is (screens of version 7: `N4N0/Screen/Size`).
    pub screen_size: ScreenSize,
}

impl Default for MenuSettings {
    fn default() -> Self {
        Self { button: true, wake_word: true, touch: true, keyboard: KeyboardLayout::Auto, look: MenuLook::Modern, screen_size: ScreenSize::Medium }
    }
}

/// How large N4N0's projected screen is, everyone seeing it so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenSize {
    Small,
    #[default]
    Medium,
    Large,
    Larger,
}

impl ScreenSize {
    /// Times the screen's own size.
    pub fn scale(self) -> f32 {
        match self {
            ScreenSize::Small => 0.75,
            ScreenSize::Medium => 1.0,
            ScreenSize::Large => 1.3,
            ScreenSize::Larger => 1.6,
        }
    }
}

/// How N4N0's menu looks: Orbiters' own, or a green phosphor terminal (`orbiters_orb::menu::retro`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuLook {
    #[default]
    Modern,
    Retro,
}

/// The order of the letters on N4N0's keyboard.
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
            wake_words: vec!["nano".into()],
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
        Self { enabled: true, vrchat_port: 9000, record_parameters: true, drive_camera: true }
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
            reconstruct_while_playing: WhilePlaying::default(),
            window_gpu: WindowGpu::default(),
            view_gpu: ViewGpu::default(),
            startup: StartupSettings::default(),
            development: false,
            tracking: TrackingSettings::default(),
            rig_views: true,
            coverage_overlay: CoverageOverlay::default(),
            dolly_streams: false,
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
        assert!(partial.rig_views && partial.coverage_overlay == CoverageOverlay::LeftHand, "the rig's views and the coverage overlay on");
        assert_eq!(partial.orb, OrbSettings::default(), "settings from before N4N0 get its defaults");
        assert_eq!(partial.window_gpu, WindowGpu::GraphicsCard, "the window draws on the graphics card unless chosen otherwise");
        assert_eq!(partial.view_gpu, ViewGpu::WhilePlaying, "its 3D view on the integrated GPU while VRChat runs unless chosen otherwise");
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
                    \"osc\": {\"vrchat_port\": 70000, \"drive_camera\": false}, \
                    \"orb\": {\"wake_word\": true, \"menu\": {\"keyboard\": \"dvorak\"}}}";
        std::fs::write(&path, text).unwrap();
        let s = Settings::load_from(&path);
        assert!(s.consent_acknowledged, "consent survives a bad field elsewhere");
        assert_eq!(s.storage_limit_gb, 7.0);
        assert!(!s.osc.drive_camera);
        assert_eq!(s.osc.vrchat_port, OscSettings::default().vrchat_port);
        assert!(s.orb.wake_word);
        assert_eq!(s.orb.menu, MenuSettings::default());
        let kept = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().starts_with("settings.json.bad-"));
        assert!(kept, "the file that needed repair is kept beside");
        std::fs::remove_dir_all(dir).ok();
    }
}
