//! Everything the interface shows lives in one [`Snapshot`], republished on every change. Its parts are
//! [`Slice`]s (`orbiters_core::slice`): a publish copies none of them, and a link sends the window and the overlay only
//! the parts that changed (`crate::ipc`).

use orbiters_core::protocol::{AudioDeviceInfo, CaptureStats, CoverageMap, SpoutSourceInfo};
use orbiters_core::reconstruction::Stage;
use orbiters_core::settings::Settings;
use orbiters_core::slice::Slice;
use orbiters_i18n::{mark, tr};
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

/// What the panels around VRChat's menus can do now: for Settings to tell the person.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrbitStatus {
    /// What is in the way, in words, none when they can show (desktop: exclusive full screen; VR: no trigger block).
    pub note: Option<String>,
    /// SteamVR's "Experimental overlay input overrides": `None` where it is not known (no SteamVR).
    pub steamvr_overrides: Option<bool>,
    /// Why the tiles around VRChat's camera cannot show, in words, as `note`.
    #[serde(default)]
    pub camera_note: Option<String>,
    pub camera_provisional: bool,
    pub camera_panel_seen_ns: Option<u64>,
    pub camera_scale_measured: bool,
}

/// What live translation in VR is doing (the orbit menu's Translation and Settings show it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TranslateStatus {
    pub state: TranslateState,
    /// Translations shown now, and lines of text followed.
    pub shown: u32,
    pub lines: u32,
    /// What a look costs on average (reading, placing, painting), milliseconds of the reading thread.
    pub look_ms: Option<u32>,
    /// The AI server in use or found, and the models it offers (for Settings' choice of model).
    pub server: Option<String>,
    pub models: Vec<String>,
    /// How long the batch on its way has been waited for, seconds.
    #[serde(default)]
    pub waited_s: Option<u32>,
    /// The AI server's model in use, when it is too small to translate well.
    #[serde(default)]
    pub small_model: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum TranslateState {
    /// Turned off, or the overlay not running.
    #[default]
    Off,
    /// Downloading the text reader (bytes so far, in all).
    Downloading { done: u64, total: u64 },
    /// Downloading the translator built into the app, no AI app answering on this computer (bytes so far, in all).
    DownloadingTranslator { done: u64, total: u64 },
    Starting,
    /// Reading what the headset shows; nothing waits for a translation.
    Looking,
    /// Lines waiting for the translator.
    Translating(u32),
    /// The translator getting ready (an AI server loading its model) before its first batch.
    Waking,
    /// A menu or SteamVR's dashboard covers the view.
    Paused,
    /// Orbiters is chosen, but its notice was not accepted yet.
    NeedsAgreement,
    /// An address is chosen, but none was typed.
    NeedsAddress,
    /// No AI app answers on this computer.
    NoLocalTranslator,
    /// Something in the way: `key` is an English text (`mark`ed) the surfaces translate, `detail` a technical detail.
    Problem { key: String, detail: Option<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VrchatStatus {
    pub running: bool,
    /// Process id while it runs (its sound can be recorded).
    #[serde(default)]
    pub pid: Option<u32>,
    /// A slice: the people in the instance travel when the world's state changes, not with the avatar's speed.
    pub world: Slice<WorldState>,
    pub log_found: bool,
    /// How VRChat moves the avatar, once its parameters said so: the SteamVR tab's walk boost carries only while VRChat
    /// walks the avatar itself.
    #[serde(default)]
    pub motion: Option<AvatarMotion>,
    /// It runs on another device of the network ([`LanStatus`]): it takes nothing of this computer's GPU or processor.
    #[serde(default)]
    pub remote: bool,
    /// Every VRChat running on this computer, in the order they started; one of them is the target (the one followed, which
    /// pictures are taken in). Empty while none runs; one entry when only one does.
    #[serde(default)]
    pub runs: Vec<VrchatRunShown>,
}

orbiters_core::parts!(VrchatStatus { plain: [running, pid, log_found, motion, remote, runs], slices: [world], identity: [], nested: [] });

/// One VRChat running on this computer, as the interface shows it (controller/vrchats.rs).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VrchatRunShown {
    pub pid: u32,
    /// VRChat's profile (`--profile`): each holds its own account.
    pub profile: u32,
    /// Whether `profile` is known (its log said its arguments); until then it says 0 and means "not known yet".
    #[serde(default)]
    pub profile_known: bool,
    /// The account signed in, once its log said.
    pub account: Option<String>,
    /// That account's VRChat user id, while its log says it is the one signed in and its command line names one profile.
    #[serde(default)]
    pub account_id: Option<String>,
    /// Its log says nobody is signed in (its sign-in page).
    #[serde(default)]
    pub signed_out: bool,
    pub world: Option<String>,
    /// Desktop or VR, once known.
    pub desktop: Option<bool>,
    /// Whether its camera is open (its own OSCQuery tree), when it could be asked.
    pub camera: Option<bool>,
    pub driver: VrchatDriverShown,
    /// The one the app follows and takes pictures in.
    pub target: bool,
    /// The person chose it (rather than it being the default, their own).
    pub chosen: bool,
}

/// Who drives a VRChat.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum VrchatDriverShown {
    /// The person's own.
    #[default]
    Person,
    /// An agent through the VRChat bridge, in this session (shortened id, `main` for the bridge's main session).
    Agent { session: String },
    /// Started by the bridge for an agent, held by no session now.
    Left,
}

impl VrchatStatus {
    /// The VRChat followed (pictures are taken in it), when several run: `None` with one VRChat or none, where nothing
    /// needs saying.
    pub fn several_target(&self) -> Option<&VrchatRunShown> {
        if self.runs.len() < 2 {
            return None;
        }
        self.runs.iter().find(|r| r.target)
    }
}

impl VrchatStatus {
    /// Whether a VRChat runs on this very computer: what shares its GPU, and what its log, Spout and sound come from.
    pub fn runs_here(&self) -> bool {
        self.running && !self.remote
    }
}

/// How the app stands with VRChat on the network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanLink {
    /// Not looking: VRChat runs on this computer, or OSC is off.
    #[default]
    Off,
    /// This computer is on no local network.
    NoNetwork,
    /// The system keeps the app off the local network: it is asking, or was told no.
    NotAllowed,
    /// No VRChat found yet.
    Looking,
    /// Several found: the person chooses.
    Choose,
    /// A VRChat is chosen but neither answers nor sends anything.
    Waiting,
    /// VRChat answers or sends.
    Connected,
}

/// VRChat on the network, where VRChat does not run on this computer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LanStatus {
    pub link: LanLink,
    /// Every VRChat on the network now.
    pub found: Vec<orbiters_core::lan::FoundVrchat>,
    /// The one talked with: its address, and what to call it.
    pub peer: Option<std::net::Ipv4Addr>,
    pub peer_label: Option<String>,
    /// This computer's address on that network, for what a PC's launch options must say.
    pub this_computer: Option<std::net::Ipv4Addr>,
}

/// What VRChat's own avatar parameters (`InStation`, `Seated`, `VelocityMagnitude`) say of its moving. The speed is
/// kept unrounded, with the local receive time. OSC can send only changes: age alone is not a stop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AvatarMotion {
    pub in_station: bool,
    pub seated: bool,
    /// Metres a second.
    pub speed: f32,
    pub speed_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// VRChat sent something within the last minute. It sends avatar parameters only to OSC services it found:
    /// off while VRChat runs, the avatar's buttons (Talk, a hand on N4N0) do not reach Orbiters.
    #[serde(default)]
    pub receiving: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureStatus {
    pub live: bool,
    pub state: CaptureState,
    pub session_dir: Option<PathBuf>,
    pub source: Option<String>,
    pub stats: Option<CaptureStats>,
    pub error: Option<String>,
    /// Camera positions (Unity world) of the running capture, thinned out.
    pub trail: Vec<[f32; 3]>,
    /// Capture started by the Action Menu (OSC) rather than the interface.
    pub from_osc: bool,
    /// What the running capture's depth frames have seen so far, with hints.
    #[serde(default)]
    pub coverage: Option<CoverageMap>,
    /// The running capture is a 3D picture (its camera flies on its own: no coverage to follow).
    #[serde(default)]
    pub photo_3d: bool,
    /// Why frames come without the capture strip, while they do and after a recording that never had it.
    #[serde(default)]
    pub no_strip: Option<NoStrip>,
    /// What the running or last 3D picture is of: `None` for a recording.
    #[serde(default)]
    pub picture: Option<orbiters_core::settings::PictureMode>,
    /// Who the running or last capture left out, and what the person should know about it.
    #[serde(default)]
    pub left_out: LeftOut,
}

/// Who a capture left out of the picture.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeftOut {
    pub wearer: orbiters_core::picture::Left,
    pub others: orbiters_core::picture::Left,
    /// Why somebody is in the picture after all, and what to do about it.
    pub why: Option<String>,
}

/// Why frames come without the capture strip: no camera pose, so no keyframes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoStrip {
    /// VRChat's camera has Local User off, which culls the capture kit with the avatar.
    MaskOff,
    /// The worn avatar has no capture kit.
    NoKit,
    /// The kit and the mask are on: a world's depth of field or motion blur, or an old upload.
    NotDrawn,
    /// VRChat does not say (OSC is off, or it did not answer).
    Unknown,
}

impl NoStrip {
    /// The sentence the person reads, once a stretch.
    pub fn message(self) -> &'static str {
        tr(self.message_english())
    }

    /// The same in English, for what AI clients read (the bridge's state): never translated.
    pub fn message_english(self) -> &'static str {
        match self {
            Self::MaskOff => mark("VRChat's camera has Local User off, which hides your avatar from the camera and N4N0's capture kit with it, so there is no camera pose and there are no keyframes. Turn Local User on in the camera's Mask menu."),
            Self::NoKit => mark("The avatar you are wearing has no N4N0 capture kit, so there is no camera pose and there are no keyframes. Wear an avatar with N4N0, or add N4N0 to this one in My Avatar and upload it."),
            Self::NotDrawn => mark("N4N0's capture kit and Local User are on, but the capture strip is not in the picture, so there are no keyframes. A world's depth of field or motion blur can wipe it. If it never shows, update N4N0 in My Avatar and upload the avatar again."),
            Self::Unknown => mark("No capture strip in the picture, so there is no camera pose and there are no keyframes. Check that you wear an avatar with N4N0 and that Local User is on in the camera's Mask menu."),
        }
    }

    /// The few words the page's tile has room for.
    pub fn short(self) -> &'static str {
        tr(match self {
            Self::MaskOff => mark("Local User is off"),
            Self::NoKit => mark("no capture kit on this avatar"),
            Self::NotDrawn => mark("the world may wipe it"),
            Self::Unknown => mark("no camera pose"),
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StorageStatus {
    pub sessions_dir: PathBuf,
    pub used_bytes: u64,
    pub free_bytes: Option<u64>,
    pub limit_gb: f64,
    pub sessions: usize,
    /// Of `used_bytes`, the capture files of reconstructed sessions (their frames, depth and sound), which keeping only
    /// their 3D views frees, in that many sessions.
    #[serde(default)]
    pub capture_bytes: u64,
    #[serde(default)]
    pub capture_sessions: usize,
}

/// Space being freed (controller/cleanup.rs), for good: the capture files of sessions whose 3D views stay, and sessions
/// that go whole.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CleanupJob {
    /// Waiting, in order: sessions keeping only their 3D view, then sessions going whole.
    pub trim: Vec<PathBuf>,
    pub delete: Vec<PathBuf>,
    /// The session being done now, and whether it goes whole.
    pub current: Option<(PathBuf, bool)>,
    /// Bytes freed so far, of what the whole job frees (as measured when it was asked).
    pub freed: u64,
    pub total: u64,
}

impl CleanupJob {
    /// Whether the job has `dir` still to do or doing: `Some(true)` when it goes whole.
    pub fn of(&self, dir: &std::path::Path) -> Option<bool> {
        match &self.current {
            Some((current, whole)) if current == dir => Some(*whole),
            _ if self.trim.iter().any(|d| d == dir) => Some(false),
            _ if self.delete.iter().any(|d| d == dir) => Some(true),
            _ => None,
        }
    }

    /// Adds sessions to do, each once (one already waiting or being done keeps its place).
    pub fn add(&mut self, trim: &[PathBuf], delete: &[PathBuf]) {
        for dir in trim {
            if self.of(dir).is_none() {
                self.trim.push(dir.clone());
            }
        }
        for dir in delete {
            if self.of(dir).is_none() {
                self.delete.push(dir.clone());
            }
        }
    }

    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.freed as f64 / self.total as f64).min(1.0) as f32
        }
    }
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
    Checking {
        username: Option<String>,
    },
    SignedIn {
        username: String,
    },
    /// The saved sign-in was refused: expired, or signed out everywhere.
    Expired {
        username: Option<String>,
    },
    /// Orbiters could not be reached to check it.
    Offline {
        username: Option<String>,
    },
    /// Finishing in the browser: Connect on the page that opened there.
    SigningIn,
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AccountStatus {
    pub state: AccountState,
    /// The Unity tools are set to the development server.
    pub development: bool,
    /// The last sign-in problem.
    pub error: Option<String>,
    /// The member signed in, as pages key what is theirs: the environment and the id ("production/42"), never a name; none
    /// while nobody is, or who it is is not known.
    #[serde(default)]
    pub member: Option<String>,
}

/// How Fast walk carries the person now, as the SteamVR overlay says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalkPath {
    /// Orbiters - N4N0's SteamVR driver moves every device: other applications that move the play space add up with it.
    Driver,
    /// The play space itself moves, the lesser way: another application that moves it puts the person back.
    PlaySpace(WhyPlaySpace),
}

/// Why Fast walk moves the play space rather than going through the driver: what would change that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhyPlaySpace {
    /// SteamVR did not load the driver: its next start will.
    NotRunning,
    /// SteamVR started before this version was installed and runs its older driver: its next start loads this one.
    OldDriver,
    /// SteamVR's Safe Mode turned the driver off.
    SafeMode,
    /// Turned off in SteamVR's Manage Add-ons.
    TurnedOff,
    /// The person turned it off in Settings.
    Setting,
    /// The driver did not move every device here.
    Refused,
    /// Some devices that move with the person come from another tracking system than the headset's (Vive trackers with a
    /// Quest, aligned by Space Calibrator): a calibration add-on moves those devices' poses after the driver's walk, turned by
    /// its calibration, so the play space is moved instead, which SteamVR applies to every device after every driver.
    OtherTrackingSystem,
}

impl WalkPath {
    /// The same in a few words, for where there is one line for it.
    pub fn lesser_short(self) -> Option<&'static str> {
        use orbiters_i18n::mark;
        match self {
            WalkPath::Driver | WalkPath::PlaySpace(WhyPlaySpace::Setting) => None,
            WalkPath::PlaySpace(WhyPlaySpace::NotRunning | WhyPlaySpace::OldDriver) => Some(mark("Restart SteamVR once for the better Fast walk.")),
            WalkPath::PlaySpace(WhyPlaySpace::SafeMode | WhyPlaySpace::TurnedOff) => Some(mark("Turn orbiters_smooth on in SteamVR for the better Fast walk.")),
            WalkPath::PlaySpace(WhyPlaySpace::Refused) => Some(mark("The driver could not carry your devices here.")),
            WalkPath::PlaySpace(WhyPlaySpace::OtherTrackingSystem) => Some(mark("Fast walk moves the play space to keep your trackers aligned.")),
        }
    }

    /// What to say and do about it: `None` while the walk goes through the driver, or the person chose not to.
    pub fn lesser_words(self) -> Option<&'static str> {
        use orbiters_i18n::mark;
        match self {
            WalkPath::Driver | WalkPath::PlaySpace(WhyPlaySpace::Setting) => None,
            WalkPath::PlaySpace(WhyPlaySpace::NotRunning | WhyPlaySpace::OldDriver) => {
                Some(mark("Restart SteamVR once for the better Fast walk. Until then it moves the play space, so OVR Advanced Settings' drag and turn put you back."))
            }
            WalkPath::PlaySpace(WhyPlaySpace::SafeMode) => {
                Some(mark("SteamVR's Safe Mode turned the driver off: turn orbiters_smooth on in Manage Add-ons for the better Fast walk. Until then OVR Advanced Settings' drag and turn put you back."))
            }
            WalkPath::PlaySpace(WhyPlaySpace::TurnedOff) => {
                Some(mark("Turn orbiters_smooth on in SteamVR's Manage Add-ons for the better Fast walk. Until then OVR Advanced Settings' drag and turn put you back."))
            }
            WalkPath::PlaySpace(WhyPlaySpace::Refused) => {
                Some(mark("The driver could not carry all your devices, so Fast walk moves the play space. OVR Advanced Settings' drag and turn put you back. See walk-boost.log."))
            }
            WalkPath::PlaySpace(WhyPlaySpace::OtherTrackingSystem) => Some(mark(
                "Your trackers use another tracking system than your headset (aligned by Space Calibrator, for instance), so Fast walk moves the play space: that keeps every tracker on your body. OVR Advanced Settings' drag and turn put you back.",
            )),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SteamVrStatus {
    /// SteamVR's server runs.
    pub running: bool,
    /// Its process id while it runs (`vrserver`): a SteamVR quit and started again is another one.
    #[serde(default)]
    pub pid: Option<u32>,
    /// The Orbiters overlay is connected (a tab in SteamVR's dashboard).
    pub overlay: bool,
    /// How Fast walk carries the person, as the overlay last said.
    #[serde(default)]
    pub walk_path: Option<WalkPath>,
    /// Someone wears the headset (SteamVR's activity level, read by the overlay); `None` without the overlay.
    #[serde(default)]
    pub headset_in_use: Option<bool>,
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
    /// On the integrated GPU, while VRChat runs (settings' `reconstruct_while_playing`).
    pub integrated: bool,
    /// Paused: its worker has gone or is going, a checkpoint holds where it stood, and it goes on by itself once what it
    /// gave way to is over.
    #[serde(default)]
    pub paused: bool,
    /// Going on from a pause: reading the session again, before its next step.
    #[serde(default)]
    pub resuming: bool,
    /// The share of the whole run a pause kept: what it shows never falls under this while it reads the session again.
    #[serde(default)]
    pub floor: f32,
}

impl ReconstructionJob {
    pub fn new(session_dir: PathBuf, integrated: bool) -> Self {
        Self { session_dir, stage: Stage::Loading, progress: 0.0, iteration: 0, iterations: 0, gaussians: 0, psnr: None, seconds: 0.0, integrated, paused: false, resuming: false, floor: 0.0 }
    }

    /// Where a checkpoint says a paused run stands.
    pub fn at(&mut self, iteration: u32, iterations: u32, gaussians: usize, psnr: Option<f32>, seconds: f32) {
        self.iteration = iteration;
        self.iterations = iterations;
        self.gaussians = gaussians;
        self.psnr = psnr.or(self.psnr);
        self.seconds = seconds;
        self.progress = iteration as f32 / iterations.max(1) as f32;
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
    Done {
        added: usize,
        skipped: usize,
    },
    Failed {
        message: String,
    },
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

// N4N0's part of the snapshot, shared with the phone app (`orbiters_n4n0::status`).
pub use orbiters_n4n0::status::{
    AvatarView, CallContact, CallLink, CallPhase, CallView, CallsStatus, N4n0Confirmation, N4n0Face, N4n0Light, N4n0Line, N4n0Status, N4n0Who, ProcessUsage,
    StateAction, StateView, StatesView, VoiceStatus,
};

/// VRChat's frames the SteamVR overlay saw, about ten seconds of them (contracts `Docs/world-performance.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrameChunk {
    /// Time between VRChat's frames, oldest first.
    pub intervals_ms: Vec<f32>,
    pub refresh_hz: Option<f32>,
    /// The headset's model (SteamVR's model number).
    pub headset: Option<String>,
    /// The graphics card driving the headset.
    pub gpu: Option<String>,
}

/// Updates of an installed app (controller/update.rs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateStatus {
    /// The setup installed this copy: it updates itself (a development build does not).
    pub enabled: bool,
    /// The release channel it follows.
    pub channel: String,
    pub state: UpdateState,
    /// The last look that reached Orbiters (Unix seconds).
    pub checked: Option<u64>,
    /// The person asked to restart once the running work ends ([`Snapshot::busy_with`]).
    pub restart_when_idle: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum UpdateState {
    /// Up to date, or not looked yet.
    #[default]
    Idle,
    Checking,
    Downloading {
        version: String,
        fraction: f32,
    },
    /// Downloaded and checked: a restart installs it.
    Ready {
        version: String,
        notes: String,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerStatus {
    pub ready: bool,
    pub error: Option<String>,
}

/// The published state. A field that is a [`Slice`] is read as before (`snapshot.settings.sessions_dir`) and written as
/// before where the state is the controller's own (`s.capture.state = …`); what it adds is that a copy of the snapshot
/// shares it, and that [`Slice::same`] tells whether it changed between two snapshots without looking at it. The few
/// plain fields are small, and compared by value.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub smoothing_revision: u64,
    pub version: String,
    pub vrchat: VrchatStatus,
    pub sources: Slice<Vec<SpoutSourceInfo>>,
    /// Microphones the worker can record.
    pub mics: Slice<Vec<AudioDeviceInfo>>,
    pub osc: Slice<OscStatus>,
    #[serde(default)]
    pub lan: Slice<LanStatus>,
    pub capture: Slice<CaptureStatus>,
    pub settings: Slice<Settings>,
    pub storage: Slice<StorageStatus>,
    pub worker: Slice<WorkerStatus>,
    pub steamvr: Slice<SteamVrStatus>,
    pub reconstruction: Option<ReconstructionJob>,
    /// Sessions waiting to be reconstructed, next first.
    pub reconstruct_queue: Slice<Vec<PathBuf>>,
    #[serde(default)]
    pub account: Slice<AccountStatus>,
    /// A plain value for now, though it can hold a picture: the screenshots' scenarios assign it whole.
    pub vrchat_login: crate::controller::vrchat_login::Status,
    pub uploads: Slice<Vec<UploadStatus>>,
    /// Its jobs cannot be compared (what a share came to holds Orbiters' answers): sent whenever it was written to.
    pub capture_shares: Slice<Vec<crate::controller::shares::ShareJob>>,
    #[serde(default)]
    pub n4n0: N4n0Status,
    #[serde(default)]
    pub update: Slice<UpdateStatus>,
    /// Space being freed in the library.
    #[serde(default)]
    pub cleanup: Option<CleanupJob>,
    /// What "When SteamVR starts" last did to Windows' sound devices (Settings).
    #[serde(default)]
    pub vr_audio: Slice<crate::controller::vr_audio::Status>,
    /// The experimental VRChat bridge (`crate::bridge`): only filled in for an account the server says has it.
    #[serde(default)]
    pub bridge: crate::bridge::Status,
    /// What the panels around VRChat's menus last said of themselves.
    #[serde(default)]
    pub orbit: Slice<OrbitStatus>,
    /// Live translation in VR, as the overlay last said.
    #[serde(default)]
    pub translate: Slice<TranslateStatus>,
    /// The FPS booster, as both surfaces show it.
    #[serde(default)]
    pub booster: Slice<crate::booster::Status>,
    /// Whether the avatar worn has N4N0's capture kit, as VRChat's parameters say: `None` when they do not say.
    #[serde(default)]
    pub worn_kit: Option<bool>,
    /// Whether VRChat's camera is open, as VRChat last said: `None` when it has not said.
    #[serde(default)]
    pub camera_open_said: Option<bool>,
    /// VRChat's own wrists (left, right) as its OSC last said them (`/tracking/vrsystem/*wrist/pose`: x, y, z, Euler x, y, z in Unity's axes),
    /// and when (`clock::now_ns`): VRChat's camera panel opens by its avatar's left hand and is grabbed from either (overlay `camera_reckon`).
    #[serde(default)]
    pub vrchat_wrists: [Option<[f32; 6]>; 2],
    #[serde(default)]
    pub vrchat_wrists_ns: [u64; 2],
    pub camera_tools: crate::controller::camera_tools::Status,
    /// Explorer points: the notice on arriving in a world with no public 3D picture, the share buttons after a 3D
    /// picture, the member's points (controller/explorer.rs).
    #[serde(default)]
    pub explorer: crate::controller::explorer::Status,
    /// Bumped each time the VRChat bridge's Unity projects tools change something the Unity projects tab shows: the tab
    /// looks again.
    #[serde(default)]
    pub projects_changed: u64,
    /// The performance self-check's hint and the last Performance report (`perf`).
    #[serde(default)]
    pub perf: Slice<crate::perf::PerfView>,
    /// Home's launcher: friends, groups, invites, boops, the game (controller/launcher.rs).
    #[serde(default)]
    pub launcher: Slice<orbiters_core::launcher::LauncherState>,
    /// The window minimised for long while VRChat runs offers to close to the tray (`crate::window_rest`): its offer,
    /// for the VR band to show, and what the band said of it.
    #[serde(default)]
    pub window_rest: WindowRest,
    /// The shared 3D view (contracts `Docs/n4n0-shared-view.md`, `controller/shared_view.rs`): friends here to show the
    /// small view to, the share this side makes, an offer to look at a friend's, the friend's view being looked at.
    #[serde(default)]
    pub shared_view: Slice<SharedViewStatus>,
}

/// The shared 3D view as the service holds it: looking at the same capture together, each copy in front of its own viewer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SharedViewStatus {
    /// Friends in this instance who run Orbiters - N4N0 and whose own log sees this player too.
    pub targets: Vec<CallContact>,
    /// This side shows its small view to a friend.
    pub outgoing: Option<ShareOut>,
    /// A friend offers to show theirs (the first of those waiting).
    pub offer: Option<ShareOffer>,
    /// A friend's view, accepted and followed.
    pub watching: Option<ShareWatching>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShareOutPhase {
    /// The offer is on its way to Orbiters.
    #[default]
    Offering,
    /// The friend is asked.
    Asked,
    /// The friend looks at it.
    Watching,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShareOut {
    /// The friend's name and the capture's listing id.
    pub to: String,
    pub capture: u64,
    pub phase: ShareOutPhase,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShareOffer {
    /// This side's number for it (the band's id), counting from 1.
    pub id: u64,
    pub from: String,
    /// The capture's world.
    pub title: String,
    /// When it goes (Unix ms), and how long it was up for.
    pub until_ms: u64,
    pub for_ms: u64,
}

impl ShareOffer {
    /// The share of its time left at `now_ms`.
    pub fn left(&self, now_ms: u64) -> f32 {
        if self.for_ms == 0 {
            return 0.0;
        }
        (self.until_ms.saturating_sub(now_ms) as f32 / self.for_ms as f32).clamp(0.0, 1.0)
    }
}

/// A capture's start view as Orbiters gives it (right-handed, Unity's world with Z negated).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShareStart {
    pub position: [f32; 3],
    pub target: [f32; 3],
    pub floor: Option<f32>,
    pub eye_height: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShareWatching {
    /// This side's number for it: a new one is a new view to open.
    pub id: u64,
    pub from: String,
    pub title: String,
    pub capture: u64,
    pub revision: u64,
    /// The capture's file once downloaded (signed in, never from a public address), and its start view.
    pub file: Option<PathBuf>,
    pub start: Option<ShareStart>,
    /// The friend's view: zoom (capture metres around the middle) and pan, the last state applied.
    pub view: Option<(f32, [f32; 2])>,
    /// The download failed: what to say.
    pub failed: Option<String>,
}

/// "Orbiters - N4N0 will minimize to the tray to save memory" (LIGHT-DESKTOP-8, Enzo's decision of 2026-11-19), as the
/// window offers it: an id for each offer, and its countdown once the person can see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestOffer {
    pub id: u64,
    /// When the window goes (Unix ms); 0 while the countdown is paused (not seen yet, or hidden).
    pub until_ms: u64,
    pub for_ms: u64,
    /// What is left of the countdown while it is paused (ms).
    #[serde(default)]
    pub left_ms: u64,
}

impl RestOffer {
    /// The share of its countdown left at `now_ms` (while paused, what was left when it paused).
    pub fn left(&self, now_ms: u64) -> f32 {
        if self.for_ms == 0 {
            return 1.0;
        }
        if self.until_ms == 0 {
            return (self.left_ms as f32 / self.for_ms as f32).clamp(0.0, 1.0);
        }
        (self.until_ms.saturating_sub(now_ms) as f32 / self.for_ms as f32).clamp(0.0, 1.0)
    }

    /// Whole seconds left at `now_ms`, as said on it ("in 8 s").
    pub fn seconds(&self, now_ms: u64) -> u64 {
        if self.until_ms == 0 {
            return self.left_ms.div_ceil(1000);
        }
        self.until_ms.saturating_sub(now_ms).div_ceil(1000)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRest {
    /// The offer up now.
    pub offer: Option<RestOffer>,
    /// The offer the VR band shows (it said so).
    pub shown: Option<u64>,
    /// The offer someone pressed Keep open on in VR.
    pub kept: Option<u64>,
}

orbiters_core::parts!(Snapshot {
    plain: [smoothing_revision, version, reconstruction, vrchat_login, cleanup, bridge, worn_kit, camera_open_said, vrchat_wrists, vrchat_wrists_ns, camera_tools, explorer, projects_changed, window_rest],
    slices: [sources, mics, osc, lan, capture, settings, storage, worker, steamvr, reconstruct_queue, account, uploads, update, vr_audio, orbit, booster, translate, perf, launcher, shared_view],
    identity: [capture_shares],
    nested: [vrchat, n4n0],
});

impl Snapshot {
    /// Live worker activity only; imports and the test pattern are excluded by their source flags.
    pub fn live_recording_running(&self) -> bool {
        matches!(self.capture.state, CaptureState::Starting | CaptureState::Recording | CaptureState::Paused)
            && self.capture.live
    }

    /// Three-state kit contract. Reason keys are translated by the consuming UI.
    pub fn capture_kit_reason(&self) -> &'static str {
        match self.worn_kit {
            Some(true) => "ready",
            Some(false) => "avatar_has_no_capture_kit",
            None => "avatar_capture_kit_unknown",
        }
    }
    pub fn can_switch_demo_avatar(&self) -> bool {
        let playing = self.vrchat.world.local_player.as_ref().and_then(|name| self.vrchat.world.players.iter().find(|(_,n)| *n == name).map(|(id,_)| id));
        self.camera_tools.demo_avatar.is_some() && self.vrchat.running && self.vrchat_login.allowed
            && self.vrchat_login.user.as_ref().is_some_and(|u| playing == Some(&u.id))
            && !self.is_capturing() && self.capture.state != CaptureState::Stopping
            && self.camera_tools.demo != crate::controller::camera_tools::Demo::Busy
    }
    pub fn new(settings: Settings) -> Self {
        Self::with(settings.into())
    }

    /// [`Snapshot::new`] with settings another snapshot holds: shared with it, not copied.
    pub fn with(settings: Slice<Settings>) -> Self {
        Self {
            smoothing_revision: 0,
            orbit: Default::default(),
            translate: Default::default(),
            booster: Default::default(),
            worn_kit: None,
            camera_open_said: None,
            vrchat_wrists: [None; 2],
            vrchat_wrists_ns: [0; 2],
            camera_tools: Default::default(),
            explorer: Default::default(),
            projects_changed: 0,
            perf: Default::default(),
            launcher: Default::default(),
            window_rest: Default::default(),
            shared_view: Default::default(),
            version: orbiters_core::app_version().into(),
            vrchat: VrchatStatus {
                running: false,
                pid: None,
                world: Default::default(),
                log_found: orbiters_core::paths::vrchat_log_dir().is_some(),
                motion: None,
                remote: false,
                runs: Vec::new(),
            },
            sources: Default::default(),
            mics: Default::default(),
            osc: OscStatus { enabled: settings.osc.enabled, ..Default::default() }.into(),
            lan: Default::default(),
            capture: CaptureStatus {
                live: false,
                state: CaptureState::Idle,
                session_dir: None,
                source: None,
                stats: None,
                error: None,
                trail: Vec::new(),
                from_osc: false,
                coverage: None,
                photo_3d: false,
                no_strip: None,
                picture: None,
                left_out: LeftOut::default(),
            }
            .into(),
            storage: StorageStatus { sessions_dir: settings.sessions_dir.clone(), limit_gb: settings.storage_limit_gb, ..Default::default() }.into(),
            settings,
            worker: Default::default(),
            steamvr: Default::default(),
            reconstruction: None,
            reconstruct_queue: Default::default(),
            account: Default::default(),
            vrchat_login: Default::default(),
            uploads: Default::default(),
            capture_shares: Default::default(),
            n4n0: N4n0Status::default(),
            update: Default::default(),
            cleanup: None,
            vr_audio: Default::default(),
            bridge: Default::default(),
        }
    }

    pub fn is_capturing(&self) -> bool {
        matches!(self.capture.state, CaptureState::Starting | CaptureState::Recording | CaptureState::Paused | CaptureState::Importing)
    }

    /// What a restart (to update) would cut short now, as the interface names it: a recording being made or saved, a
    /// reconstruction running or waiting, a call, an upload to the Map explorer, the speech models downloading.
    pub fn busy_with(&self) -> Option<&'static str> {
        if self.is_capturing() || self.capture.state == CaptureState::Stopping {
            Some(mark("the recording"))
        } else if self.reconstruction.is_some() || !self.reconstruct_queue.is_empty() {
            Some(mark("the reconstruction"))
        } else if self.n4n0.calls.call.is_some() {
            Some(mark("the call"))
        } else if self.uploads.iter().any(UploadStatus::running) || self.capture_shares.iter().any(|j| j.outcome.is_none()) {
            Some(mark("the upload"))
        } else if self.n4n0.voice.downloading.is_some() {
            Some(mark("the speech models' download"))
        } else if self.cleanup.is_some() {
            Some(mark("the clean-up"))
        } else {
            None
        }
    }

}

/// The running capture's camera path as the service keeps it, up to 6000 points. The snapshot's
/// [`CaptureStatus::trail`] is [`Trail::shown`] of it, 1500 points at most: what the interface draws, and all that is
/// copied or sent when the recording's state changes.
#[derive(Default)]
pub struct Trail(Vec<[f32; 3]>);

impl Trail {
    /// Points the interface gets at most.
    pub const SHOWN: usize = 1500;

    pub fn of(points: Vec<[f32; 3]>) -> Self {
        Self(points)
    }

    /// Adds a camera position when it moved enough (keeps the trail light). Returns whether it was added.
    pub fn push(&mut self, p: [f32; 3]) -> bool {
        let far_enough = self.0.last().map(|q| {
            let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > 0.0025
        });
        if !far_enough.unwrap_or(true) {
            return false;
        }
        self.0.push(p);
        if self.0.len() > 6000 {
            // Thin the older half (every other point) instead of dropping the start of the path: the recent part
            // keeps its detail, the oldest is thinned most.
            let half = self.0.len() / 2;
            let mut keep: Vec<[f32; 3]> = self.0[..half].iter().step_by(2).copied().collect();
            keep.extend_from_slice(&self.0[half..]);
            self.0 = keep;
        }
        true
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The path thinned evenly to [`Trail::SHOWN`] points at most.
    pub fn shown(&self) -> Vec<[f32; 3]> {
        let step = self.0.len().div_ceil(Self::SHOWN).max(1);
        self.0.iter().step_by(step).copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The camera path keeps what moved, thins its older half past 6000 points, and shows 1500 at most.
    #[test]
    fn the_camera_path_stays_light() {
        let mut trail = Trail::default();
        assert!(trail.push([0.0, 0.0, 0.0]));
        assert!(!trail.push([0.01, 0.0, 0.0]), "not moved enough");
        assert!(trail.push([0.1, 0.0, 0.0]));
        assert_eq!(trail.shown(), vec![[0.0, 0.0, 0.0], [0.1, 0.0, 0.0]]);
        for i in 2..6001 {
            assert!(trail.push([i as f32 * 0.1, 0.0, 0.0]));
        }
        // 6001 points: the older half (3000) thinned to 1500, the newer 3001 kept.
        assert_eq!(trail.len(), 4501);
        let shown = trail.shown();
        assert!(shown.len() <= Trail::SHOWN + 1 && shown.len() > 1000, "{}", shown.len());
        assert_eq!(shown[0], [0.0, 0.0, 0.0]);
        trail.clear();
        assert!(trail.shown().is_empty());
    }

    /// Every reason the lesser way is in use says what to do about it, and the better way, or the person's own choice, nothing.
    #[test]
    fn the_lesser_way_says_what_would_change_it() {
        use WhyPlaySpace::*;
        for why in [NotRunning, OldDriver, SafeMode, TurnedOff, Refused, OtherTrackingSystem] {
            assert!(WalkPath::PlaySpace(why).lesser_words().is_some() && WalkPath::PlaySpace(why).lesser_short().is_some(), "{why:?}");
        }
        for path in [WalkPath::Driver, WalkPath::PlaySpace(Setting)] {
            assert!(path.lesser_words().is_none() && path.lesser_short().is_none(), "{path:?}");
        }
        let restart = WalkPath::PlaySpace(OldDriver).lesser_short().unwrap();
        assert!(restart.contains("Restart SteamVR"), "one step: {restart}");
    }
}
