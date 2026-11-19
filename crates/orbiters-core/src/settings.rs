//! User settings, stored as JSON in the config folder. Unknown fields are kept out, missing ones defaulted.

use crate::contracts::manifest::{CaptureMode, StorageQuality};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The FPS booster's slider, at Off until a person moves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BoosterSettings {
    pub level: orbiters_booster::Level,
    /// Moving to Avatars asks once what that does to how other people look.
    pub avatars_told: bool,
}

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
    /// The window minimised for long while VRChat runs closes to the tray to give its memory back, after a countdown
    /// with Keep open (Enzo's decision of 2026-11-19): on unless turned off in Settings.
    pub close_minimized_window: bool,
    /// Show Orbiters in SteamVR's dashboard while SteamVR runs.
    pub steamvr_overlay: bool,
    /// N4N0, the Orbiters companion on the avatar.
    pub n4n0: N4n0Settings,
    /// Share VRChat's frame rate in each world with Orbiters, a summary a minute (contracts `Docs/world-performance.md`).
    pub world_performance: bool,
    /// Walk boost: the play space moves along with the movement stick, so walking in VRChat goes faster.
    pub walk_boost: WalkBoostSettings,
    /// The FPS booster's slider: remembered, and acting only while VRChat has the scene.
    pub booster: BoosterSettings,
    /// How long a 3D picture's camera flight lasts (more seconds, more of the place seen).
    pub photo_3d: Photo3dLength,
    /// What a 3D picture is of, the last one chosen: what N4N0's menu and the Action Menu take, which have one button.
    pub photo_3d_mode: PictureMode,
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
    /// While recording, N4N0's camera rig (contracts' `Docs/rig-frames.md`) turns on for a moment this often, wherever
    /// the camera stands that it has not looked all around from yet: six more views around it (left, front, right,
    /// back, up, down) for the reconstruction.
    pub rig_views: RigViews,
    /// Where the SteamVR overlay shows what a recording has covered so far (the Capture page's camera path).
    pub coverage_overlay: CoverageOverlay,
    /// The floating Orbiters panel in the play space (the SteamVR tab's interface without SteamVR's dashboard, so
    /// VRChat keeps the controllers) and the watch on a wrist that opens N4N0's menu.
    pub floating: FloatingSettings,
    /// Explorer points (REQUEST13): the notice in a world with no public 3D picture yet, and whether sharing publicly
    /// from beside the camera was confirmed once.
    pub explorer: ExplorerSettings,
    /// The language: the last the account gave or the person picked (a website tag: `ja`, `pt-BR`); `None`, this computer's.
    pub language: Option<String>,
    /// That pick has yet to reach the account.
    pub language_unsent: bool,
    /// How the Orbiters menu appears in VRChat: around VRChat's own menus, or one of the older ways.
    pub orbit: OrbitSettings,
    /// Experimental, VRC+: recordings also read VRChat's camera dolly's other streams (Spout `VRCSender2` to
    /// `VRCSender4`), and a 3D picture flies four cameras with the dolly (multi-stream) instead of moving the camera
    /// alone. The person sets the dolly's Streaming to Multi once in VRChat.
    pub dolly_streams: bool,
    /// How Photos & captures lays its pictures out (Rows unless chosen otherwise). Saved as `photos_layout` from
    /// 0.3.4-dev.4: up to then `gallery_layout` held Masonry for everyone (its default), and that key is left behind
    /// unread, so every gallery starts in Rows until the person picks another layout again.
    pub photos_layout: GalleryLayout,
    /// Additional picture folders, including archives on removable drives.
    pub picture_folders: Vec<PathBuf>,
    /// Windows' default sound output and input while SteamVR runs (`docs/vr-audio-devices.md`).
    pub vr_audio: VrAudioSettings,
    /// What this person turned on among the experimental features. A feature here only ever narrows what the server
    /// grants (`crate::features`): turning one on in this file does nothing for an account that does not have it.
    pub experimental: ExperimentalSettings,
    /// Live translation of the text seen in VR (the orbit menu's Translation): off until turned on.
    pub translate: TranslateSettings,
    /// The beta's usage statistics (which controls and pages are used): on unless turned off, told once.
    pub usage: UsageSettings,
    /// The beta's menu mismatch buttons (Settings, Beta feedback): off until turned on.
    pub mismatch: MismatchSettings,
    /// How Home's Play buttons start VRChat (the launcher's performance launch options).
    pub launch: crate::vrchat_launch::LaunchSettings,
}

/// The beta's menu mismatch buttons (`orbiters-desktop`'s `mismatch`): three buttons always in view that say N4N0's menus do not match
/// VRChat's, each press kept as a report with what led to it. Nothing is recorded while `buttons` is off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MismatchSettings {
    pub buttons: bool,
    /// A small, blurred picture of the game at the press goes with the report.
    pub picture: bool,
}

/// The beta's usage statistics (`orbiters-desktop`'s `usage`): the names of the pages and controls used and performance figures, never what
/// is typed, said or seen; linked to the Orbiters account when one is signed in and the notice saying so was shown (`told_account`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UsageSettings {
    pub enabled: bool,
    /// The notice that says what is recorded was shown (closed by the person).
    pub told: bool,
    /// The notice was shown since it also says performance figures and the hardware class are sent (2026-11-16): shown again once to
    /// those who closed the first one.
    pub told_perf: bool,
    /// The notice was shown since it says that, signed in, what is counted is linked to the Orbiters account (2026-11-19, Enzo:
    /// "identify the usages by user"): shown again once to everyone who closed an earlier one. Only what is counted after this is
    /// linked to an account; what was counted under the earlier notice never is (`orbiters-desktop`'s `usage::spool::Stamp`).
    pub told_account: bool,
}

impl Default for UsageSettings {
    fn default() -> Self {
        Self { enabled: true, told: false, told_perf: false, told_account: false }
    }
}

impl UsageSettings {
    /// Every version of the notice was shown and closed: the card is not drawn any more.
    pub fn told_all(&self) -> bool {
        self.told && self.told_perf && self.told_account
    }
}

/// Live translation: the text the headset shows (signs, posters, chat bubbles) read a few times a second and its
/// translation laid over it in the room. Nothing of it runs while it is off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TranslateSettings {
    pub enabled: bool,
    /// The language to translate into (a website tag: `fr`, `pt-BR`); `None`: the app's language.
    pub target: Option<String>,
    pub provider: TranslateProvider,
    /// The AI server's address for [`TranslateProvider::Address`] (`http://192.168.1.20:1234`).
    pub server: String,
    /// The model to ask on an AI server; empty: the one that suits best of those it offers.
    pub model: String,
    /// English is left as it is (what VRChat's own menus and most names are in).
    pub keep_english: bool,
    /// The person read and accepted that, with Orbiters translating, the text seen in VR is sent to Orbiters.
    pub orbiters_agreed: bool,
}

impl Default for TranslateSettings {
    fn default() -> Self {
        Self { enabled: false, target: None, provider: TranslateProvider::default(), server: String::new(), model: String::new(), keep_english: true, orbiters_agreed: false }
    }
}

/// Who translates. Nothing leaves this computer (or its network) unless Orbiters is chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranslateProvider {
    /// An AI app on this computer (LM Studio, Ollama, llama.cpp's server), found by itself; else the model built in.
    #[default]
    ThisComputer,
    /// An AI server at an address of the network ([`TranslateSettings::server`]).
    Address,
    /// Orbiters' translation service (an Orbiters account; the text is sent to Orbiters).
    Orbiters,
}

/// The experimental features (Settings, Experimental), all off until chosen.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExperimentalSettings {
    /// The MCP bridge to VRChat, for an agent to start and control the game (`docs/vrchat-bridge.md`).
    pub vrchat_bridge: bool,
    /// How many VRChats the bridge may run at once (its sessions, `docs/vrchat-bridge.md`); 0: the default, 2.
    pub vrchat_bridge_sessions: u8,
    /// The developer VR sandbox's folder (`tools/vr-sandbox` of a checkout of this repository, with Python and psutil), for the
    /// bridge's VR sessions (`open_session` with `vr`, docs/vrchat-bridge.md): empty, none. Not shown in Settings.
    pub vrchat_bridge_vr_sandbox: String,
}

/// The sound devices Windows uses by default while SteamVR runs: set for all three of Windows' roles (console,
/// multimedia, communications) when SteamVR starts, and again over its first seconds, as Virtual Desktop and SteamVR
/// set their own; the ones before put back when it closes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VrAudioSettings {
    /// The output (a render endpoint); `None`: left as it is.
    pub output: Option<AudioEndpoint>,
    /// The input (a capture endpoint); `None`: left as it is.
    pub input: Option<AudioEndpoint>,
    /// Put back the devices Windows had before when SteamVR closes.
    pub restore: bool,
}

impl Default for VrAudioSettings {
    fn default() -> Self {
        Self { output: None, input: None, restore: true }
    }
}

/// A Windows sound device: its endpoint id, and its name, which finds it again when the id changed (a driver
/// installed again gives its devices new ids).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioEndpoint {
    pub id: String,
    pub name: String,
}

/// How Photos & captures lays its pictures out (its layout picker; each group of an instance laid out alone).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GalleryLayout {
    /// Columns of pictures each as tall as its shape, as the website's galleries.
    Masonry,
    /// Rows of one height filling the width, each picture as wide as its shape (the default).
    #[default]
    Rows,
    /// The same card for every item, its picture cropped to 16:9, with what it is and who sees it under it.
    Grid,
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

/// Where N4N0's own menu shows, the one opened by its dedicated button or the watch. It has nothing to do with the panels around VRChat's menus ([`OrbitSettings::wheel`],
/// [`OrbitSettings::quick_menu`]), which go on or off whatever this says (Enzo, 2026-11-07: "completely independent").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuDisplay {
    /// On the avatar's N4N0 screen when it has one, floating otherwise. (`around_vrchat`: what a setting from when this choice also held the panels says.)
    #[default]
    #[serde(alias = "around_vrchat")]
    AvatarScreen,
    /// Floating in the play space, even with an avatar that has N4N0's screen.
    Floating,
}

/// How the panels around VRChat's menus follow the hand in VR. VRChat draws its menu from a pose taken a frame ago, so it trails the hand by what depends on each computer's frame rate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VrMotion {
    /// Each tile around VRChat's menus a little behind the hand on its own spring: never in step with VRChat's menu, on purpose, on any computer.
    #[default]
    Springy,
    /// Rigid, following the hand as smoothly as [`OrbitSettings::vr_smoothing_quick_menu_ms`] and [`OrbitSettings::vr_smoothing_wheel_ms`] say, as what opens in front of VRChat's menu always is.
    Matched,
}

/// Where the panels around the Quick Menu lie.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrbitLayout {
    /// A wing of tiles beside it.
    #[default]
    Wing,
    /// A row of tiles under its tabs.
    Strip,
}

/// The panels around VRChat's own menus (`overlay::orbit`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OrbitSettings {
    pub display: MenuDisplay,
    /// Panels around the Action Menu's wheel.
    pub wheel: bool,
    /// Panels around the Quick Menu.
    pub quick_menu: bool,
    /// Tiles around VRChat's camera panel, in a headset: found on VRChat's window while its camera is open.
    pub camera_tiles: bool,
    /// Those tiles also find the camera's panel by looking for it on VRChat's picture. Off (the default), they follow VRChat's own rules
    /// with no picture read: where the panel opens by the left hand, the hand that grabs it, its X pressed (Enzo, 2026-11-13: the looking
    /// "seems to work pretty shit", make it an option, off by default).
    pub camera_pattern: bool,
    /// VRChat's turning stick turns what shows around the person, in a headset.
    pub immersive_turn: bool,
    /// How large the panels are in VR against where they are measured to be, one of [`OrbitSettings::VR_SIZES`]: VRChat
    /// scales its menus with things only the headset shows.
    pub vr_size: f32,
    pub vr_motion: VrMotion,
    /// How smoothly what is rigid follows the hand on the Quick Menu, and on the wheel: the time constants of their filters, in milliseconds. Enzo, in the headset: VRChat's own are about 35 ms and 20 ms. VRChat's menu moves as if smoothed, which no delay matched (Enzo: "even at 1.5 frames ahead").
    pub vr_smoothing_quick_menu_ms: f32,
    pub vr_smoothing_wheel_ms: f32,
    /// Where they lie around the Quick Menu.
    pub layout: OrbitLayout,
    /// How they look: Orbiters' own, or as N4N0's retro menu.
    pub theme: MenuLook,
    /// The modern look's accent.
    pub accent: [u8; 3],
    /// The retro look's one colour.
    pub retro_color: [u8; 3],
    /// The retro look leaves pictures in their colours.
    pub retro_pictures: bool,
}

impl OrbitSettings {
    pub const VR_SIZES: [f32; 3] = [0.8, 1.0, 1.25];

    /// The wheel's panels are wanted.
    pub fn around_wheel(&self) -> bool {
        self.wheel
    }

    /// The Quick Menu's panels are wanted.
    pub fn around_quick_menu(&self) -> bool {
        self.quick_menu
    }

    /// Any of the panels are wanted.
    pub fn any(&self) -> bool {
        self.around_wheel() || self.around_quick_menu()
    }
}

impl Default for OrbitSettings {
    fn default() -> Self {
        Self { camera_tiles: true, camera_pattern: false, immersive_turn: true, layout: OrbitLayout::Wing, theme: MenuLook::Modern, accent: [0x00, 0xda, 0x6d], retro_color: [0x2b, 0xe3, 0x6a], retro_pictures: false, display: MenuDisplay::AvatarScreen, wheel: true, quick_menu: true, vr_size: 1.0, vr_motion: VrMotion::Springy, vr_smoothing_quick_menu_ms: 35.0, vr_smoothing_wheel_ms: 20.0 }
    }
}

/// The floating Orbiters panel (the SteamVR overlay): the dashboard tab's interface on a panel in the play space, used
/// with the controllers' own lasers instead of SteamVR's dashboard, so VRChat keeps its controls meanwhile; moved by
/// its handle as N4N0's screen is; opened by the tab's Float button. The watch, a round Orbiters mark on a wrist, opens
/// and closes N4N0's menu with one press of the other hand's trigger.
/// Explorer points' settings (REQUEST13).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExplorerSettings {
    /// "No 3D picture of this world yet" on arriving in such a world (on by default).
    pub world_notices: bool,
    /// "Share publicly" beside VRChat's camera was confirmed once: it asks no more.
    pub public_confirmed: bool,
}

impl Default for ExplorerSettings {
    fn default() -> Self {
        Self { world_notices: true, public_confirmed: false }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FloatingSettings {
    /// The watch shows on a wrist (it fades in as the wrist turns to the eyes).
    pub watch: bool,
    /// The wrist it is on.
    pub watch_hand: Hand,
    /// Where the panel was last left in the play space (SteamVR's standing universe): its centre and turn, a 3 × 4
    /// row-major matrix (its +Z towards the viewer). None: it opens before the eyes.
    pub placement: Option<[[f32; 4]; 3]>,
    /// The panel's width in metres, one of [`FloatingSettings::WIDTHS`].
    pub width_m: f32,
}

impl FloatingSettings {
    /// The panel's widths, its bar's smaller and larger steps through them.
    pub const WIDTHS: [f32; 5] = [0.5, 0.65, 0.8, 1.0, 1.25];
}

impl Default for FloatingSettings {
    fn default() -> Self {
        Self { watch: true, watch_hand: Hand::Left, placement: None, width_m: 0.8 }
    }
}

/// Tracking smoothing (Orbiters - N4N0's SteamVR driver): on or off, and each kind of device's time constant in
/// milliseconds, headset, controllers, trackers (0 leaves it as SteamVR tracks it). The driver eases each pose toward the
/// new one over that time whatever the frame rate, so a device in steady motion trails by that many milliseconds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "SavedTracking")]
pub struct TrackingSettings {
    pub enabled: bool,
    pub ms: [u16; 3],
}

impl TrackingSettings {
    /// The driver's longest (`orbiters_smooth::MAX_TIME_CONSTANT_MS`).
    pub const MAX_MS: u16 = 120;
    /// What Off, Low, Medium and High were: strengths 0, 0.35, 0.65 and 0.9 of [`TrackingSettings::MAX_MS`], kept for the
    /// settings saved with them, which so smooth exactly as they did.
    pub const LEVEL_MS: [u16; 4] = [0, 42, 78, 108];
}

impl Default for TrackingSettings {
    /// The headset untouched (smoothing it can make people sick), the controllers a little, the trackers more.
    fn default() -> Self {
        Self { enabled: false, ms: [0, Self::LEVEL_MS[1], Self::LEVEL_MS[2]] }
    }
}

/// [`TrackingSettings`] as a file may hold them: in milliseconds, or as the index of Off, Low, Medium and High that
/// versions up to 0.3.5-dev.15 saved (`levels`).
#[derive(Default, Deserialize)]
#[serde(default)]
struct SavedTracking {
    enabled: bool,
    ms: Option<[u16; 3]>,
    levels: Option<[u8; 3]>,
}

impl From<SavedTracking> for TrackingSettings {
    fn from(saved: SavedTracking) -> Self {
        let from_level = |l: u8| Self::LEVEL_MS.get(l as usize).copied().unwrap_or(0);
        let ms = saved.ms.or(saved.levels.map(|l| l.map(from_level))).unwrap_or(Self::default().ms);
        Self { enabled: saved.enabled, ms: ms.map(|m| m.min(Self::MAX_MS)) }
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

/// A 3D picture's length: a gentle 1 s sweep, or 4 or 8 s around the people in it and through the room (with no depth frames to go by, 4 s that circle you and fly 10 m out every way, or 8 s that also circle
/// four points 8 m away).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Photo3dLength {
    Quick,
    #[default]
    Standard,
    Full,
}

/// What a 3D picture is of (`crate::picture`): the flight's time, who is left out of the picture, and what training
/// spends its steps on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PictureMode {
    /// Most of the flight around the people in the picture.
    #[default]
    People,
    /// Most of the flight through the place, the avatars left out of the picture.
    World,
}

/// How often N4N0's camera rig takes its six views during a recording (the app's own flights, 3D pictures and scans,
/// leave it alone).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RigViews {
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "every_2s")]
    Every2s,
    #[default]
    #[serde(rename = "every_3s")]
    Every3s,
    #[serde(rename = "every_5s")]
    Every5s,
}

impl RigViews {
    /// The shortest time between two of the rig's moments, `None` when it is off.
    pub fn every(self) -> Option<std::time::Duration> {
        let seconds = match self {
            RigViews::Off => return None,
            RigViews::Every2s => 2,
            RigViews::Every3s => 3,
            RigViews::Every5s => 5,
        };
        Some(std::time::Duration::from_secs(seconds))
    }
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
    /// Through Orbiters - N4N0's SteamVR driver, which moves every device's pose, when it runs: other applications that move
    /// the play space then add up with the walk. Off, or without the driver, the play space itself moves, as it used to.
    pub through_driver: bool,
}

impl WalkBoostSettings {
    pub const MAX_EXTRA: f32 = 2.0;
}

impl Default for WalkBoostSettings {
    fn default() -> Self {
        Self { enabled: false, strength: 0.5, hand: Hand::Left, follow: Follow::Head, through_driver: true }
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

/// N4N0, the Orbiters companion (contracts `Docs/n4n0.md`): its helpers, and its assistant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct N4n0Settings {
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
    /// Calls with VRChat friends who have Orbiters and N4N0 too (contracts `Docs/n4n0-calls.md`): presence and the
    /// friends this computer's VRCX and VRCNext know go to Orbiters (as hashes) while on.
    pub calls: bool,
    /// Calls may go straight between the two computers (a little faster on one network), each side then seeing the
    /// other's addresses. Off, they go through the relay, which shows the friend only its own.
    pub direct_calls: bool,
    /// N4N0's menu (fast walk, YouTube, calls, a 3D picture): what opens it, the keyboard's letters.
    pub menu: MenuSettings,
    /// The controllers' touchpads and sticks go to the avatar's N4N0 input parameters (contracts `Docs/n4n0-input.md`),
    /// when it has them.
    pub controller_input: bool,
}

/// What opens N4N0's menu: in the SteamVR overlay, or on the avatar's projected screen when the N4N0 prop has one (the
/// "Replace Orbiters overlay" option of N4N0 in My Avatar). Opening it also starts N4N0 listening.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuSettings {
    /// The SteamVR controller binding "Open N4N0's menu" (by default two quick taps on the left thumbrest or trackpad,
    /// or a double press of X on controllers with neither; changed in SteamVR's controller settings for Orbiters).
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

/// How N4N0's menu looks: Orbiters' own, or a green phosphor terminal (`orbiters_n4n0_core::menu::retro`).
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

/// This computer's locale name (`ja-JP`), empty when it says none.
fn computer_locale() -> String {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetUserDefaultLocaleName(name: *mut u16, length: i32) -> i32;
        }
        let mut name = [0u16; 85];
        // SAFETY: the buffer holds LOCALE_NAME_MAX_LENGTH (85) UTF-16 units, the length the call is given.
        let n = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
        if n > 1 {
            return String::from_utf16_lossy(&name[..(n - 1) as usize]);
        }
    }
    ["LC_ALL", "LC_CTYPE", "LANG"].iter().find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty())).unwrap_or_default()
}

impl Default for N4n0Settings {
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
            afk_message: "Away for a moment, back soon!".into(), // i18n: not shown (a saved text the person edits, sent to VRChat in the chatbox: it stays what was typed)
            carry_parameters: GOGOLOCO_PARAMETERS.iter().map(|s| s.to_string()).collect(),
            pins: Vec::new(),
            calls: false,
            direct_calls: false,
            menu: MenuSettings::default(),
            controller_input: true,
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
    /// Which VRChat on the network the app talks with, where VRChat does not run on this computer
    /// ([`crate::osc::VRCHAT_RUNS_HERE`]).
    pub remote: crate::lan::Wanted,
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
        Self { enabled: true, vrchat_port: 9000, record_parameters: true, drive_camera: true, remote: Default::default() }
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
            close_minimized_window: true,
            steamvr_overlay: true,
            n4n0: N4n0Settings::default(),
            world_performance: false,
            walk_boost: WalkBoostSettings::default(),
            booster: BoosterSettings::default(),
            photo_3d: Photo3dLength::default(),
            photo_3d_mode: PictureMode::default(),
            photo_3d_timer: 0,
            reconstruct_while_playing: WhilePlaying::default(),
            window_gpu: WindowGpu::default(),
            view_gpu: ViewGpu::default(),
            startup: StartupSettings::default(),
            development: false,
            tracking: TrackingSettings::default(),
            rig_views: RigViews::default(),
            coverage_overlay: CoverageOverlay::default(),
            floating: FloatingSettings::default(),
            explorer: ExplorerSettings::default(),
            language: None,
            language_unsent: false,
            orbit: OrbitSettings::default(),
            dolly_streams: false,
            photos_layout: GalleryLayout::default(),
            picture_folders: Vec::new(),
            vr_audio: VrAudioSettings::default(),
            experimental: ExperimentalSettings::default(),
            translate: TranslateSettings::default(),
            usage: UsageSettings::default(),
            mismatch: MismatchSettings::default(),
            launch: crate::vrchat_launch::LaunchSettings::default(),
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
                // Unreadable for now: the defaults, until a save, which puts its changes on the file as it reads then
                // ([`Settings::save_to`]) rather than these defaults over it.
                eprintln!("settings: {}: {e}", path.display());
                crate::keep_aside(path);
                loaded_unreadable(path);
                return Self::unread();
            }
        };
        // A later read that works does NOT end the unreadable state: what this process holds (the service's live
        // settings) is still the defaults, and a save must go onto the person's file ([`Settings::save_to`]).
        let migrated = migrated(&text);
        let (settings, dropped) = Self::parse(migrated.as_deref().unwrap_or(&text));
        known_until_unreadable(path, &settings);
        if !dropped.is_empty() {
            let aside = crate::keep_aside(path);
            eprintln!("settings: {} left at their default ({}), the file kept as {:?}", dropped.join(", "), path.display(), aside);
        }
        if migrated.is_some() {
            // Written again at once under today's names: the earlier one is read this once only.
            if let Err(e) = settings.save_to(path) {
                eprintln!("settings: {}: {e:#}", path.display());
            }
        }
        settings
    }

    pub fn load() -> Self {
        Self::load_from(&crate::paths::settings_file())
    }

    /// The settings in `text` as [`crate::json_lenient`] reads them, a statistics switch that did not read off, never on
    /// by default (the person may have turned it off; PRIV-12); with the fields left out.
    fn parse(text: &str) -> (Self, Vec<String>) {
        let (mut settings, dropped) = crate::json_lenient::<Self>(text);
        if dropped.iter().any(|d| d == "usage" || d == "usage.enabled" || d == "(not JSON)") {
            settings.usage.enabled = false;
        }
        (settings, dropped)
    }

    /// The language a program of its own (the call, capture and speech workers) speaks: the one these settings hold (what the account gave
    /// or the person picked), else this computer's, English where the website has no such language. The app does the same for itself
    /// (`orbiters-desktop`'s `language` module) and follows a change at once; a program started here speaks the language it started in,
    /// so that what it reports (`tr!` in its messages) reads as the window it reports to does.
    pub fn language_wanted(&self) -> orbiters_i18n::Language {
        let from_settings = self.language.as_deref().and_then(orbiters_i18n::normalize);
        from_settings.or_else(|| orbiters_i18n::normalize(&computer_locale())).unwrap_or(orbiters_i18n::Language::En)
    }

    /// Makes this program speak the language the saved settings ask for ([`Settings::language_wanted`]).
    pub fn speak_saved_language() {
        orbiters_i18n::set_language(Self::load().language_wanted());
    }

    /// Writes the whole file at once ([`crate::write_atomic`]): a crash, a power cut or another save at the same time
    /// leaves the old settings or the new ones. When this process could not read the file (these settings started
    /// from the defaults), what differs from the defaults is put on the file as it reads now, and while it still
    /// cannot be read nothing is written.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let rebased;
        let settings = if let Some(kept) = was_loaded_unreadable(path) {
            match crate::read_text(path) {
                Ok(Some(text)) => {
                    // The person's file is kept beside, whole, before anything is written over it the first time;
                    // without that copy nothing is written.
                    if !kept {
                        let Some(aside) = crate::keep_aside(path) else {
                            let why = orbiters_i18n::tr("no copy of it could be kept beside it");
                            anyhow::bail!("{}", orbiters_i18n::tr!("{path} could not be read, and is not replaced by defaults: {error}", path = path.display(), error = why));
                        };
                        eprintln!("settings: {} kept as {} before the first save onto it", path.display(), aside.display());
                        set_kept_aside(path);
                    }
                    let migrated = migrated(&text);
                    let (on_disk, _) = Self::parse(migrated.as_deref().unwrap_or(&text));
                    rebased = self.changes_onto(on_disk, &baseline(path));
                    &rebased
                }
                Ok(None) => self,
                Err(e) => anyhow::bail!("{}", orbiters_i18n::tr!("{path} could not be read, and is not replaced by defaults: {error}", path = path.display(), error = e)),
            }
        } else {
            self
        };
        crate::write_atomic(path, serde_json::to_string_pretty(settings)?.as_bytes())?;
        // What this process holds is now on the file: the next save puts on it what changed since THIS one (a switch
        // turned on and then off again is off; night4, verify2).
        saved(path, self);
        Ok(())
    }

    /// What a process holds when its settings file could not be read: the defaults, the statistics off (a person
    /// may have turned them off, and nothing tells it now; PRIV-12).
    fn unread() -> Self {
        let mut s = Self::default();
        s.usage.enabled = false;
        s
    }

    /// `base` with what these settings change from `start` (what this process last knew the file to hold: its last
    /// read before the file became unreadable, what it last saved, else [`Settings::unread`]), at any depth.
    fn changes_onto(&self, base: Self, start: &serde_json::Value) -> Self {
        let (Ok(mine), defaults, Ok(mut out)) = (serde_json::to_value(self), start.clone(), serde_json::to_value(&base)) else {
            return base;
        };
        /// Puts onto `out` each leaf of `mine` that differs from `start`, at any depth (one menu choice changed leaves
        /// the menu's others as the file has them).
        fn merge(mine: &serde_json::Value, start: Option<&serde_json::Value>, out: &mut serde_json::Value) {
            if start == Some(mine) {
                return;
            }
            match (mine.as_object(), start.and_then(|s| s.as_object()), out.as_object_mut()) {
                (Some(mine), Some(start), Some(target)) => {
                    for (key, value) in mine {
                        let slot = target.entry(key.clone()).or_insert(serde_json::Value::Null);
                        if slot.is_null() && !start.contains_key(key) {
                            *slot = value.clone();
                        } else {
                            merge(value, start.get(key), slot);
                        }
                    }
                }
                _ => *out = mine.clone(),
            }
        }
        merge(&mine, Some(&defaults), &mut out);
        serde_json::from_value(out).unwrap_or(base)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&crate::paths::settings_file())
    }

    /// These settings with what `patch` changes: a JSON object of the fields to change and their values, a section as
    /// an object of its own changed fields (`{"n4n0": {"wake_word": true}}`; a list, the pins say, is replaced whole).
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

/// The key N4N0's section was saved under before the companion took its name (0.3.4-dev.8 and earlier).
const LEGACY_N4N0_KEY: &str = "orb";

/// The settings' text with N4N0's section moved from its earlier key to `n4n0` (today's wins when both are there),
/// or `None` when the text has no earlier key (or is not a JSON object: [`crate::json_lenient`] deals with that).
/// The settings files this process could not read when it loaded them ([`Settings::load_from`]): it holds defaults for
/// them, which must never be saved over the real ones.
/// For the life of the process (whatever is read afterwards): the settings it started from are not the file's. The
/// value: the file has been kept beside since.
static LOADED_UNREADABLE: std::sync::Mutex<std::collections::BTreeMap<PathBuf, bool>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

fn loaded_unreadable(path: &Path) {
    LOADED_UNREADABLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(path.to_path_buf()).or_insert(false);
}

/// What this process last knew each settings file to hold: what it read while the file could still be read, then what
/// it saved. What a save after a failed read puts on the file is what changed since then (night4, verify2: never "since
/// the start-up defaults", which turned a switch the person turned back off on again).
static KNOWN: std::sync::Mutex<std::collections::BTreeMap<PathBuf, serde_json::Value>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// A read that worked: what the file holds, while it never failed to read in this process (a read after a failure is
/// no knowledge of what this process holds).
fn known_until_unreadable(path: &Path, settings: &Settings) {
    if was_loaded_unreadable(path).is_some() {
        return;
    }
    if let Ok(value) = serde_json::to_value(settings) {
        KNOWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(path.to_path_buf(), value);
    }
}

fn saved(path: &Path, settings: &Settings) {
    if let Ok(value) = serde_json::to_value(settings) {
        KNOWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(path.to_path_buf(), value);
    }
}

/// What a save onto the file as it reads puts what changed from: what this process last knew it to hold, else the
/// defaults it started from.
fn baseline(path: &Path) -> serde_json::Value {
    KNOWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(path).cloned().unwrap_or_else(|| serde_json::to_value(Settings::unread()).unwrap_or_default())
}

fn set_kept_aside(path: &Path) {
    LOADED_UNREADABLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(path.to_path_buf(), true);
}

/// Whether this process loaded `path` unreadable, and if so whether the file was kept beside since.
fn was_loaded_unreadable(path: &Path) -> Option<bool> {
    LOADED_UNREADABLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(path).copied()
}

fn migrated(text: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut value: serde_json::Value = serde_json::from_str(text).ok()?;
    let fields = value.as_object_mut()?;
    let legacy = fields.remove(LEGACY_N4N0_KEY)?;
    fields.entry("n4n0").or_insert(legacy);
    serde_json::to_string(&value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finding VRChat's camera by its picture is an option, off by default, and off for settings saved before it existed (Enzo, 2026-11-13).
    #[test]
    fn camera_pattern_tracking_is_off_unless_turned_on() {
        assert!(!OrbitSettings::default().camera_pattern);
        let saved: Settings = serde_json::from_value(serde_json::json!({ "orbit": { "camera_tiles": true } })).unwrap();
        assert!(saved.orbit.camera_tiles && !saved.orbit.camera_pattern);
        let on: Settings = serde_json::from_value(serde_json::json!({ "orbit": { "camera_pattern": true } })).unwrap();
        assert!(on.orbit.camera_pattern);
    }

    /// Enzo's saved settings: `levels` became milliseconds, and each smooths exactly as it did (strength × 120 ms).
    #[test]
    fn smoothing_saved_as_levels_keeps_its_time() {
        let read = |json: serde_json::Value| serde_json::from_value::<Settings>(serde_json::json!({ "tracking": json })).unwrap().tracking;
        let old = read(serde_json::json!({ "enabled": true, "levels": [0, 2, 0] }));
        assert_eq!((old.enabled, old.ms), (true, [0, 78, 0]));
        assert_eq!(read(serde_json::json!({ "levels": [1, 2, 3] })).ms, [42, 78, 108]);
        assert_eq!(read(serde_json::json!({ "levels": [9, 2, 3] })).ms, [0, 78, 108], "a level there never was is off");
        for (level, ms) in [0.0f32, 0.35, 0.65, 0.9].into_iter().zip(TrackingSettings::LEVEL_MS) {
            assert_eq!(ms, (level * TrackingSettings::MAX_MS as f32).round() as u16);
        }
        // Saved again: in milliseconds only, and the same when read back.
        let saved = serde_json::to_value(&old).unwrap();
        assert_eq!(saved, serde_json::json!({ "enabled": true, "ms": [0, 78, 0] }));
        assert_eq!(read(saved), old);
        assert_eq!(read(serde_json::json!({ "ms": [35, 200, 0] })).ms, [35, 120, 0], "no longer than the driver's longest");
        assert_eq!(read(serde_json::json!({})), TrackingSettings { enabled: false, ..Default::default() });
        // A slider's change saved over them as a patch, as every window and sheet does.
        let (patched, left_out) = Settings { tracking: old, ..Default::default() }.patched(&serde_json::json!({ "tracking": { "ms": [20, 35, 0] } }));
        assert!(left_out.is_empty(), "{left_out:?}");
        assert_eq!((patched.tracking.enabled, patched.tracking.ms), (true, [20, 35, 0]));
    }

    #[test]
    fn n4n0_s_section_moves_from_its_earlier_key_once() {
        let dir = std::env::temp_dir().join(format!("orbiters-settings-n4n0-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let earlier = format!(
            "{{\"storage_limit_gb\": 7, \"{LEGACY_N4N0_KEY}\": {{\"wake_word\": true, \"afk_message\": \"Back soon\", \"menu\": {{\"keyboard\": \"azerty\"}}}}}}"
        );
        std::fs::write(&path, &earlier).unwrap();
        let s = Settings::load_from(&path);
        assert!(s.n4n0.wake_word, "N4N0's settings kept");
        assert_eq!((s.n4n0.afk_message.as_str(), s.n4n0.menu.keyboard, s.storage_limit_gb), ("Back soon", KeyboardLayout::Azerty, 7.0));
        // Saved again under today's key at once.
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(saved.get(LEGACY_N4N0_KEY).is_none() && saved["n4n0"]["wake_word"] == true, "{saved}");
        assert_eq!(Settings::load_from(&path), s, "and read the same way afterwards");
        assert!(migrated(&std::fs::read_to_string(&path).unwrap()).is_none(), "nothing left to move");
        // Both keys (an earlier version wrote the file after a newer one): today's wins.
        let both = format!("{{\"{LEGACY_N4N0_KEY}\": {{\"wake_word\": true}}, \"n4n0\": {{\"wake_word\": false}}}}");
        std::fs::write(&path, both).unwrap();
        assert!(!Settings::load_from(&path).n4n0.wake_word);
        std::fs::remove_dir_all(dir).ok();
    }

    /// Settings another program held while they were loaded (a backup, an antivirus, a sync client) are not saved over
    /// with the defaults the app started from: nothing is written while they cannot be read, then the change made
    /// meanwhile goes onto them.
    #[cfg(windows)]
    #[test]
    fn settings_that_could_not_be_read_are_never_saved_over() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-settings-locked-{}", uuid::Uuid::new_v4()));
        let path = dir.join("settings.json");
        let real = Settings { picture_folders: vec![PathBuf::from("I:/VRChat pics")], storage_limit_gb: 7.0, ..Default::default() };
        // The person's file from an earlier run (written by another process, not this one: a save of this process is
        // what it knows the file to hold).
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&real).unwrap()).unwrap();
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded, Settings { usage: UsageSettings { enabled: false, ..Default::default() }, ..Default::default() }, "unreadable: the defaults for now, statistics off");
        let changed = Settings { dolly_streams: true, n4n0: N4n0Settings { wake_word: !N4n0Settings::default().wake_word, ..loaded.n4n0.clone() }, ..loaded };
        assert!(changed.save_to(&path).is_err(), "not written while it cannot be read");
        drop(lock);
        changed.save_to(&path).unwrap();
        let after = Settings::load_from(&path);
        assert_eq!((after.picture_folders.clone(), after.storage_limit_gb), (real.picture_folders.clone(), 7.0), "the real settings kept");
        assert!(after.dolly_streams && after.n4n0.wake_word != N4n0Settings::default().wake_word, "and the change made meanwhile");
        assert!(after.usage.enabled, "the statistics off for that run only: the file's own choice kept");
        std::fs::remove_dir_all(dir).ok();
    }

    /// Verifier (CORE-4): another reader of the same process reading the file once it is free does not end the
    /// protection; the first save keeps the person's file beside; one menu choice changed leaves the file's other menu
    /// choices; a statistics switch on the file that does not read stays off.
    #[cfg(windows)]
    #[test]
    fn a_second_reader_does_not_let_defaults_over_the_file() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-settings-reader-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let file = r#"{"storage_limit_gb": 7, "picture_folders": ["I:/VRChat pics"], "usage": {"enabled": "maybe"}, "n4n0": {"menu": {"keyboard": "azerty"}}}"#;
        std::fs::write(&path, file).unwrap();
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
        let live = Settings::load_from(&path);
        assert_eq!(live.storage_limit_gb, Settings::default().storage_limit_gb, "the defaults for now");
        drop(lock);
        // A worker of the service reads the file now (the picture folders): it reads it whole.
        assert_eq!(Settings::load_from(&path).picture_folders, [PathBuf::from("I:/VRChat pics")]);
        // The service then saves its live settings, one menu choice changed.
        let mut changed = live.clone();
        changed.n4n0.menu.touch = !live.n4n0.menu.touch;
        changed.save_to(&path).unwrap();
        let after = Settings::load_from(&path);
        assert_eq!((after.storage_limit_gb, after.picture_folders.clone()), (7.0, vec![PathBuf::from("I:/VRChat pics")]), "the person's settings kept");
        assert_eq!((after.n4n0.menu.keyboard, after.n4n0.menu.touch), (KeyboardLayout::Azerty, changed.n4n0.menu.touch), "the other menu choice kept");
        assert!(!after.usage.enabled, "a switch that did not read is written off, not on");
        let kept: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("settings.json.bad-")).collect();
        assert!(kept.iter().any(|e| std::fs::read_to_string(e.path()).unwrap() == file), "the person's file kept beside, as it was");
        // Saved again: still onto the file.
        changed.dolly_streams = true;
        changed.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).storage_limit_gb, 7.0);
        std::fs::remove_dir_all(dir).ok();
    }

    /// Night4 (verify2): after a recovery, a switch turned on and then off again in the same run is off at the next start;
    /// the statistics too (a consent switch never comes back on by itself).
    #[cfg(windows)]
    #[test]
    fn a_switch_turned_back_off_after_a_recovery_stays_off() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-settings-toggle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"storage_limit_gb": 7, "usage": {"enabled": false, "told": true}}"#).unwrap();
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
        let mut live = Settings::load_from(&path);
        drop(lock);
        // On, saved; then off again, saved.
        live.dolly_streams = true;
        live.usage.enabled = true;
        live.save_to(&path).unwrap();
        let on = Settings::load_from(&path);
        assert!(on.dolly_streams && on.usage.enabled && on.storage_limit_gb == 7.0);
        live.dolly_streams = false;
        live.usage.enabled = false;
        live.save_to(&path).unwrap();
        let after = Settings::load_from(&path);
        assert!(!after.dolly_streams, "turned back off: off at the next start");
        assert!(!after.usage.enabled, "the statistics: the last choice, off");
        assert_eq!(after.storage_limit_gb, 7.0, "the person's other settings kept");
        std::fs::remove_dir_all(dir).ok();
    }

    /// Night4 (verify2): another reader of the same process failing to read the file does not make the writer, which
    /// read it fine at start, put its changes on it as "changed from the defaults": turning statistics off sticks.
    #[cfg(windows)]
    #[test]
    fn a_readers_failure_does_not_undo_the_writers_choice() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("orbiters-settings-reader-fail-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"usage": {"enabled": true, "told": true}}"#).unwrap();
        let mut live = Settings::load_from(&path);
        assert!(live.usage.enabled);
        // A worker of the same process cannot read it for a moment.
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
        let _ = Settings::load_from(&path);
        drop(lock);
        // The person turns the statistics off.
        live.usage.enabled = false;
        live.save_to(&path).unwrap();
        assert!(!Settings::load_from(&path).usage.enabled, "off, as chosen");
        std::fs::remove_dir_all(dir).ok();
    }

    /// PRIV-12: a statistics switch that did not read is off, not on by default.    /// PRIV-12: a statistics switch that did not read is off, not on by default.
    #[test]
    fn statistics_that_did_not_read_are_off() {
        let dir = std::env::temp_dir().join(format!("orbiters-settings-usage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"storage_limit_gb": 3, "usage": {"enabled": "maybe"}}"#).unwrap();
        let s = Settings::load_from(&path);
        assert!(!s.usage.enabled && s.storage_limit_gb == 3.0);
        std::fs::write(&path, r#"{"storage_limit_gb": 3}"#).unwrap();
        assert!(Settings::load_from(&path).usage.enabled, "absent from a file of before: today's default");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_change_from_the_defaults_goes_onto_the_file_as_it_reads() {
        let on_disk = Settings { picture_folders: vec![PathBuf::from("I:/VRChat pics")], storage_limit_gb: 7.0, ..Default::default() };
        let mine = Settings { storage_limit_gb: 3.0, n4n0: N4n0Settings { afk_message: "Back soon".into(), ..Default::default() }, ..Default::default() };
        let out = mine.changes_onto(on_disk.clone(), &serde_json::to_value(Settings::unread()).unwrap());
        assert_eq!((out.picture_folders, out.storage_limit_gb, out.n4n0.afk_message.as_str()), (on_disk.picture_folders, 3.0, "Back soon"));
        assert_eq!(out.n4n0.menu, on_disk.n4n0.menu, "a section changes field by field");
    }

    #[test]
    fn round_trip_and_defaults() {
        let dir = std::env::temp_dir().join(format!("orbiters-settings-{}", std::process::id()));
        let path = dir.join("settings.json");
        let s = Settings { picture_folders: vec![PathBuf::from("I:/VRChat pics")], storage_limit_gb: 12.5, capture_mode: CaptureMode::LiveRefine, ..Default::default() };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
        std::fs::write(&path, r#"{"storage_limit_gb": 3}"#).unwrap();
        let partial = Settings::load_from(&path);
        assert_eq!(partial.storage_limit_gb, 3.0);
        assert!(partial.rig_views == RigViews::Every3s && partial.coverage_overlay == CoverageOverlay::LeftHand, "the rig's views and the coverage overlay on");
        assert_eq!(partial.n4n0, N4n0Settings::default(), "settings from before N4N0 get its defaults");
        assert!(partial.floating.watch && partial.floating.watch_hand == Hand::Left && partial.floating.placement.is_none(), "the watch on the left wrist");
        assert!(FloatingSettings::WIDTHS.contains(&partial.floating.width_m));
        assert_eq!(partial.window_gpu, WindowGpu::GraphicsCard, "the window draws on the graphics card unless chosen otherwise");
        assert_eq!(partial.view_gpu, ViewGpu::WhilePlaying, "its 3D view on the integrated GPU while VRChat runs unless chosen otherwise");
        assert!(!partial.translate.enabled && partial.translate.provider == TranslateProvider::ThisComputer && !partial.translate.orbiters_agreed, "live translation off, local, nothing agreed");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_patch_changes_its_fields_only() {
        let mut now = Settings::default();
        // Changed by the service since the window read its copy.
        now.walk_boost.strength = 0.9;
        now.n4n0.pins = vec![Pin { parameter: "Hoodie".into(), value: crate::osc::OscValue::Bool(true) }];
        let patch = serde_json::json!({
            "storage_limit_gb": 50,
            "n4n0": { "wake_word": true, "menu": { "keyboard": "azerty", "layout": "dvorak" }, "pins": [] },
            "walk_boost": { "enabled": true, "hand": "both" },
            "osc": { "vrchat_port": 70000 },
            "no_such_setting": 1,
        });
        let (patched, left_out) = now.patched(&patch);
        assert_eq!(patched.storage_limit_gb, 50.0);
        assert!(patched.n4n0.wake_word && patched.walk_boost.enabled);
        assert_eq!(patched.n4n0.menu.keyboard, KeyboardLayout::Azerty);
        assert!(patched.n4n0.pins.is_empty(), "a list is replaced whole");
        assert_eq!(patched.walk_boost.strength, 0.9, "what changed meanwhile stays");
        assert_eq!((patched.walk_boost.hand, patched.osc.vrchat_port), (Hand::Left, 9000), "values that do not fit are left out");
        assert_eq!(left_out, vec!["n4n0.menu.layout", "no_such_setting", "osc.vrchat_port", "walk_boost.hand"]);
        assert_eq!(now.patched(&serde_json::json!([1])).0, now, "not an object: nothing changes");
    }

    #[test]
    fn a_diff_carries_only_what_changed() {
        let base = Settings::default();
        // A window flips a switch, types an AFK message and replaces the pins on its copy.
        let mut mine = base.clone();
        mine.reduce_motion = !base.reduce_motion;
        mine.n4n0.afk_message = "Back soon".into();
        mine.n4n0.pins = vec![Pin { parameter: "Hoodie".into(), value: crate::osc::OscValue::Bool(true) }];
        let patch = base.diff(&mine);
        assert_eq!(
            patch,
            serde_json::json!({ "reduce_motion": mine.reduce_motion, "n4n0": { "afk_message": "Back soon", "pins": serde_json::to_value(&mine.n4n0.pins).unwrap() } })
        );
        assert_eq!(base.patched(&patch), (mine.clone(), vec![]));
        // Meanwhile the avatar's radial moved: the patch keeps that.
        let mut now = base.clone();
        now.walk_boost.strength = 0.8;
        let (patched, left_out) = now.patched(&patch);
        assert!(left_out.is_empty());
        assert_eq!((patched.walk_boost.strength, patched.reduce_motion, patched.n4n0.afk_message.as_str()), (0.8, mine.reduce_motion, "Back soon"));
        // Nothing changed: an empty patch.
        assert_eq!(base.diff(&base), serde_json::json!({}));
    }

    #[test]
    fn photos_and_captures_start_in_rows_whatever_was_saved_before() {
        // 0.3.4-dev.3 saved its default, Masonry, for everyone: that key is left behind.
        let (old, dropped) = crate::json_lenient::<Settings>(r#"{"gallery_layout": "masonry", "storage_limit_gb": 3}"#);
        assert!(dropped.is_empty());
        assert_eq!((old.photos_layout, old.storage_limit_gb), (GalleryLayout::Rows, 3.0));
        assert_eq!(Settings::default().photos_layout, GalleryLayout::Rows);
        // Masonry picked again after the change stays, saved under the new key only.
        let (picked, left_out) = old.patched(&serde_json::json!({ "photos_layout": "masonry" }));
        assert!(left_out.is_empty());
        assert_eq!(picked.photos_layout, GalleryLayout::Masonry);
        let saved = serde_json::to_value(&picked).unwrap();
        assert_eq!(saved["photos_layout"], "masonry");
        assert!(saved.get("gallery_layout").is_none());
        assert_eq!(crate::json_lenient::<Settings>(&saved.to_string()).0.photos_layout, GalleryLayout::Masonry);
    }

    #[test]
    fn a_field_that_does_not_fit_keeps_the_others() {
        let dir = std::env::temp_dir().join(format!("orbiters-settings-lenient-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        // A port out of range, a keyboard from a newer version, a byte order mark from an editor.
        let text = "\u{feff}{\"consent_acknowledged\": true, \"storage_limit_gb\": 7, \
                    \"osc\": {\"vrchat_port\": 70000, \"drive_camera\": false}, \
                    \"n4n0\": {\"wake_word\": true, \"menu\": {\"keyboard\": \"dvorak\"}}}";
        std::fs::write(&path, text).unwrap();
        let s = Settings::load_from(&path);
        assert!(s.consent_acknowledged, "consent survives a bad field elsewhere");
        assert_eq!(s.storage_limit_gb, 7.0);
        assert!(!s.osc.drive_camera);
        assert_eq!(s.osc.vrchat_port, OscSettings::default().vrchat_port);
        assert!(s.n4n0.wake_word);
        assert_eq!(s.n4n0.menu, MenuSettings::default());
        let kept = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().starts_with("settings.json.bad-"));
        assert!(kept, "the file that needed repair is kept beside");
        std::fs::remove_dir_all(dir).ok();
    }
}
