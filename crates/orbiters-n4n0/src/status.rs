//! What N4N0 shows the apps (their window, the SteamVR overlay, a phone's screen): [`N4n0Status`], republished on every
//! change, with what the person is told ([`NoticeKind`]).

use orbiters_core::slice::Slice;
use orbiters_i18n::mark;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Info,
    Success,
    Warning,
    Error,
}

/// What N4N0 shows (`N4N0/State`, contracts `Docs/n4n0.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum N4n0Face {
    #[default]
    Idle,
    Listening,
    Thinking,
    Talking,
    Sleeping,
    Happy,
    Annoyed,
    Surprised,
}

impl N4n0Face {
    /// Every state, in `N4N0/State`'s order.
    pub const ALL: [N4n0Face; 8] =
        [N4n0Face::Idle, N4n0Face::Listening, N4n0Face::Thinking, N4n0Face::Talking, N4n0Face::Sleeping, N4n0Face::Happy, N4n0Face::Annoyed, N4n0Face::Surprised];
}

/// Who said a line of N4N0's conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum N4n0Who {
    You,
    N4n0,
    /// Something N4N0 did or noticed (a tool ran, a video was saved).
    Note,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct N4n0Line {
    pub who: N4n0Who,
    pub text: String,
    /// Unix time in milliseconds.
    pub at_ms: u64,
    /// N4N0's answer as far as it has arrived: replaced as more comes, and by the whole answer at the end.
    #[serde(default)]
    pub streaming: bool,
}

/// An action of N4N0's (publishing an event…) waiting for the member's approval on Orbiters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct N4n0Confirmation {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub approve_url: String,
}

/// The link of a confirmation the owner gives in Orbiters - N4N0 itself (what N4N0 asks before it does a public,
/// lasting or costly thing), rather than on the Orbiters website: the number of what waits follows it.
pub const LOCAL_APPROVAL: &str = "orbiters-desktop:n4n0/approve/";

impl N4n0Confirmation {
    /// Approved in Orbiters - N4N0 (the N4N0 page's button, or a spoken yes), not on the Orbiters website.
    pub fn is_local(&self) -> bool {
        self.approve_url.starts_with(LOCAL_APPROVAL)
    }
}

/// The speech models and the voice worker.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VoiceStatus {
    /// Ask's preflight result; checked again before every capture.
    pub readiness: VoiceReadiness,
    /// This visit is open and defaults to automatic listening until manual takeover.
    pub ask_open: bool,
    pub automatic: bool,
    pub speech: bool,
    pub output_playing: bool,
    pub finalizing: bool,
    /// Replaceable recognition, never part of conversation history.
    pub provisional: Option<ProvisionalSpeech>,
    /// The speech models are on this device.
    pub installed: bool,
    /// The voice worker runs (listening for the wake word, or ready for the Talk button).
    pub running: bool,
    /// The models are downloading: the fraction done.
    pub downloading: Option<f32>,
    /// The microphone's level while N4N0 listens (0 to 1).
    pub level: f32,
    pub error: Option<String>,
    /// The models stopped loading (the worker stopped twice while loading them): installing again repairs them.
    #[serde(default)]
    pub repair: bool,
    /// The wake word was heard during a call: instead of listening, N4N0 offers "Talk with N4N0?" until then.
    #[serde(default)]
    pub wake_offer: Option<WakeOffer>,
}

/// "Talk with N4N0?", up for a few seconds after the wake word was heard during a call (the call's voices reach the
/// microphone: a caller saying the word starts nothing by themselves). Pressed, N4N0 listens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeOffer {
    /// Its number (a new one each time the word is heard).
    pub id: u64,
    /// When it goes by itself (Unix ms), and how long it shows in all (ms).
    pub until_ms: u64,
    pub for_ms: u32,
}

impl WakeOffer {
    /// What is left of its time, from 1 (it just came) to 0 (gone), at `now_ms` (Unix ms).
    pub fn left(&self, now_ms: u64) -> f32 {
        (self.until_ms.saturating_sub(now_ms) as f32 / self.for_ms.max(1) as f32).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProvisionalSpeech {
    pub listen: u64,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceReadiness {
    Ready,
    SignIn,
    ModelsMissing,
    ModelsDownloading,
    #[default]
    AssistantOff,
    MicrophoneProblem,
}

/// A process's share of this computer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProcessUsage {
    pub name: String,
    /// Percent of one core.
    pub cpu_percent: f32,
    pub memory_mb: f32,
}

/// N4N0, the companion on the avatar.
/// What a 3D picture asked for from around VRChat's menus still waits for. On the desktop these menus cannot be used with the camera open, as opening one closes
/// it and R does nothing over it: so the picture is asked for first, and starts when the camera is ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhotoWait {
    /// VRChat's camera to be opened.
    Camera,
    /// The camera to move once: VRChat only says where it is when it moves.
    Move,
}

/// N4N0's state as the apps show it. It is published whole on every change, so its lists are [`Slice`]s (shared by every
/// copy until one changes, and sent to the window and the overlay only when they did), and the rest is small values that
/// travel one by one when they differ ([`orbiters_core::slice::Parts`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct N4n0Status {
    /// The avatar VRChat loaded.
    pub avatar_id: Option<String>,
    /// The avatar carries the N4N0 prop, with this many text sync values.
    pub prop_sync: Option<u8>,
    /// The avatar's manifest was found: its menu is known by label.
    pub manifest: bool,
    /// Controls N4N0 can reach on the avatar.
    pub controls: usize,
    pub face: N4n0Face,
    /// What the panel shows now.
    pub panel: Option<String>,
    pub listening: bool,
    pub thinking: bool,
    pub afk: bool,
    /// The conversation as shown, the newest last.
    pub lines: Slice<Vec<N4n0Line>>,
    pub confirmations: Vec<N4n0Confirmation>,
    /// The free quota of N4N0's assistant, as the last answer told it.
    pub quota: Option<orbiters_n4n0_core::assistant::Quota>,
    pub voice: VoiceStatus,
    /// Saved videos, the newest first.
    pub videos: Slice<Vec<orbiters_n4n0_core::videos::SavedVideo>>,
    /// Orbiters' processes and VRChat's.
    pub resources: Slice<Vec<ProcessUsage>>,
    /// Testing: the state N4N0 is held in, every state shown in turn, the owner acted as away.
    #[serde(default)]
    pub test_face: Option<N4n0Face>,
    #[serde(default)]
    pub test_cycle: bool,
    #[serde(default)]
    pub test_afk: bool,
    /// Testing: all of N4N0 is hidden (`N4N0/Visible` sent false).
    #[serde(default)]
    pub test_hidden: bool,
    /// Testing: the avatar's screen shows the full pictures' diagnostics.
    #[serde(default)]
    pub test_pictures: bool,
    /// Calls with VRChat friends (the app's own: [`crate::Host::calls`]).
    #[serde(default)]
    pub calls: Slice<CallsStatus>,
    /// The avatar's N4N0 prop has the projected screen: N4N0's menu opens there instead of in the overlay.
    #[serde(default)]
    pub screen: bool,
    /// N4N0's menu is open on the avatar's screen.
    #[serde(default)]
    pub screen_open: bool,
    /// VRChat runs in desktop mode (as it says; until it does, while SteamVR doesn't run): no fast walk.
    #[serde(default)]
    pub desktop_mode: bool,
    /// Bumped when something asks the overlay to open N4N0's menu (the wake word alone, a hand on N4N0).
    #[serde(default)]
    pub menu_request: u32,
    /// The last request may close the overlay's menu when it is open (asked again).
    #[serde(default)]
    pub menu_toggle: bool,
    /// The last sentence for the overlay menu's search field, numbered.
    #[serde(default)]
    pub dictation: Option<(u32, String)>,
    /// Seconds before the 3D picture a menu's timer started.
    #[serde(default)]
    pub photo_countdown: Option<u32>,
    /// A 3D picture is asked for, and waits for this.
    #[serde(default)]
    pub photo_wait: Option<PhotoWait>,
    /// N4N0's light, when the avatar's N4N0 has one (the Light option).
    #[serde(default)]
    pub light: Option<N4n0Light>,
    /// The avatar worn's saved states.
    #[serde(default)]
    pub states: StatesView,
    /// A copied link can be dropped in the world as a card: the avatar's N4N0 screen is of version 5 or later.
    #[serde(default)]
    pub can_drop: bool,
    /// The link card the avatar's screen shows now, dropped in the world.
    #[serde(default)]
    pub card: Option<CardView>,
    /// Tracking smoothing as the menus show it; `None` where the app has none (`tracking_note` says why).
    #[serde(default)]
    pub tracking: Option<TrackingView>,
    #[serde(default)]
    pub tracking_note: String,
    /// The last suggestion a menu sent, numbered, and why it did not reach Orbiters (`None`: it did).
    #[serde(default)]
    pub suggestion: Option<(u32, Option<String>)>,
    /// The avatar's N4N0 version and the app's, as the bottom of Home shows them ("N4N0 1.0.9 · App 0.4.0").
    #[serde(default)]
    pub versions: String,
    /// The computer's keys typed for the overlay menu's keyboard, numbered, the last few.
    #[serde(default)]
    pub keys: Vec<(u32, orbiters_n4n0_core::menu::KeyInput)>,
    /// The video the world's player plays (VRChat's log), its title once known, and when it started (Unix seconds).
    #[serde(default)]
    pub world_video: Option<(String, Option<String>, Option<u64>)>,
    /// What stopped the last 3D picture asked for ("Open the camera before": VRChat's camera was closed), numbered: the
    /// menus show it on their 3D picture page.
    #[serde(default)]
    pub photo_note: Option<(u32, String)>,
    /// How the last Invite me a menu other than the screen's asked went (Previous instances), numbered.
    #[serde(default)]
    pub invite: Option<(u32, orbiters_n4n0_core::menu::InviteAnswer)>,
    /// The QR code scanner, as every menu shows it.
    #[serde(default)]
    pub qr: orbiters_n4n0_core::qr::QrView,
    /// The 3D captures as the app keeps them (the last one made, History's), and the VRChat friends a capture goes to
    /// while a menu's Share to … shows (contracts `Docs/n4n0.md`, Menu, from screen version 12).
    #[serde(default)]
    pub captures: Slice<orbiters_n4n0_core::captures::CapturesView>,
    #[serde(default)]
    pub friends: Slice<orbiters_n4n0_core::captures::Friends>,
    /// The worlds' feedback as the app keeps it, the player's actions not sent yet included (contracts `Docs/n4n0.md`,
    /// Menu, from screen version 13).
    #[serde(default)]
    pub feedback: orbiters_n4n0_core::feedback::FeedbackView,
    /// How many of N4N0's controller input parameters (contracts `Docs/n4n0-input.md`) the worn avatar lists.
    #[serde(default)]
    pub input_params: u8,
    /// The screen mirror as the overlay's menu shows it (from screen version 14: its card says why it is not there).
    #[serde(default)]
    pub mirror: orbiters_n4n0_core::mirror::MirrorView,
}

// The avatar's states and the world feedback stay plain values for now (copied with the state, compared by value):
// `overlay/orbit` and `overlay/worlds`, which another change is rewriting, assign and keep them by their own type.
orbiters_core::parts!(N4n0Status {
    plain: [
        avatar_id, prop_sync, manifest, controls, face, panel, listening, thinking, afk, confirmations, quota, voice, test_face, test_cycle, test_afk,
        test_hidden, test_pictures, screen, screen_open, desktop_mode, menu_request, menu_toggle, dictation, photo_countdown, photo_wait, light, states,
        can_drop, card, tracking, tracking_note, suggestion, versions, keys, world_video, photo_note, invite, qr, feedback, input_params, mirror,
    ],
    slices: [lines, videos, resources, calls, captures, friends],
    identity: [],
    nested: [],
});

/// Tracking smoothing (the app's SteamVR driver), as the Tracking page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackingView {
    pub on: bool,
    /// Each kind's time constant in milliseconds: headset, controllers, trackers.
    pub ms: [u16; 3],
    /// What the driver does now.
    pub note: String,
}

/// The saved states of the avatar worn, as the window and the menus show them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatesView {
    /// The avatar they are for (`None`: VRChat's avatar is not known yet).
    pub avatar_id: Option<String>,
    /// The avatar's name: as set in its SDK (VRChat's OSCQuery `/avatar/name`), else as remembered with its states, else
    /// as VRChat's log calls it.
    pub avatar_name: Option<String>,
    pub states: Vec<StateView>,
    /// What the last save, apply or change said (an error too).
    pub note: Option<StatesNote>,
    /// A save is under way.
    pub saving: bool,
    /// Every avatar with saved states, and the avatar worn (with or without): the worn one first, then the one saved in
    /// last.
    #[serde(default)]
    pub avatars: Vec<AvatarView>,
}

/// An avatar's saved states, as the window's States page shows it: a card per avatar, opening its states.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AvatarView {
    pub id: String,
    /// Its name as set in its SDK (VRChat's OSCQuery `/avatar/name`), remembered with its states.
    pub name: Option<String>,
    /// The avatar worn now: only its states are applied and saved.
    pub worn: bool,
    pub states: Vec<StateView>,
}

/// A line about the states, numbered (each new one shows again, even when it says the same).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatesNote {
    pub id: u64,
    pub text: String,
    pub error: bool,
}

/// One saved state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StateView {
    pub id: String,
    pub name: String,
    pub saved_utc: String,
    /// Applied after each world join and each switch to this avatar.
    pub default: bool,
    /// Applying it puts the GoGoLoco pose back too, and the avatar's size.
    pub pose: bool,
    pub size: bool,
    /// The parameters it keeps, how many of them are GoGoLoco's pose, and the eye height it keeps (metres).
    pub parameters: usize,
    pub pose_parameters: usize,
    pub eye_height: Option<f32>,
}

/// What the window and the menus ask of the saved states.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum StateAction {
    /// Save how the avatar is now (`None`: named "State n"); applying it also puts back the avatar's size (`size`) and
    /// GoGoLoco's pose (`pose`), and it is the avatar's default (`default`).
    Save {
        name: Option<String>,
        #[serde(default)]
        size: bool,
        #[serde(default)]
        pose: bool,
        #[serde(default)]
        default: bool,
    },
    Apply {
        id: String,
    },
    /// Make `id` the default state (`None`: no default).
    Default {
        id: Option<String>,
    },
    Rename {
        id: String,
        name: String,
    },
    Delete {
        id: String,
    },
    /// Whether applying `id` puts the GoGoLoco pose and the avatar's size back too.
    Options {
        id: String,
        pose: bool,
        size: bool,
    },
}

/// A link card dropped in the world, as the window shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CardView {
    pub title: String,
    pub link: String,
    /// Whole seconds left.
    pub left: u32,
}

/// N4N0's light as the avatar holds it (`N4N0/Light/On`, `N4N0/Light/Color`, `N4N0/Light/Level`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct N4n0Light {
    pub on: bool,
    pub color: u8,
    pub level: f32,
}

/// N4N0 calls as the interface shows them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CallsStatus {
    pub link: CallLink,
    /// Friends who can be called now.
    pub contacts: Vec<CallContact>,
    pub call: Option<CallView>,
    /// N4N0's panel lists the contacts, and which one is chosen.
    pub menu_open: bool,
    pub picked: usize,
}

/// Whether this member can use calls.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CallLink {
    /// The Orbiters account has not N4N0 calls (`orbiters_core::features::N4N0_CALLS`), or that is not known yet (signed
    /// out, offline with nothing known, before the first look): every way into a call is closed, and Call shows greyed
    /// out as "Coming soon" wherever it is. What the service starts with.
    #[default]
    ComingSoon,
    Off,
    SignedOut,
    /// Neither VRCX, VRCNext nor VRChat's log says which VRChat account is used here.
    NoAccount,
    /// This VRChat account is not the member's verified link on Orbiters yet.
    NotLinked {
        account: String,
        source: String,
    },
    Linked {
        account: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallContact {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallPhase {
    /// Our call rings at the friend's.
    Calling,
    /// A friend's call rings here.
    Ringing,
    Connecting,
    Connected,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallView {
    pub name: String,
    pub phase: CallPhase,
    pub incoming: bool,
    /// When it connected (Unix ms).
    pub connected_at_ms: Option<u64>,
    pub muted: bool,
    pub mic_level: f32,
    pub remote_level: f32,
}

/// What Call says where it is greyed out, on its tile and when pressed (the shared menu's words).
pub use orbiters_n4n0_core::menu::{CALLS_SOON, CALLS_SOON_WHY};

impl CallsStatus {
    /// Call is greyed out as "Coming soon" ([`CallLink::ComingSoon`]): nothing about calls can be used.
    pub fn coming_soon(&self) -> bool {
        self.link == CallLink::ComingSoon
    }

    /// Whether N4N0's menu can call friends here and, when not, what it says instead.
    pub fn menu_ready(&self) -> (bool, &'static str) {
        match &self.link {
            CallLink::Linked { .. } => (true, ""),
            CallLink::ComingSoon => (false, CALLS_SOON_WHY),
            // Kept as English: the menu draws what it holds through `tr`, so the note follows the language.
            CallLink::Off => (false, mark("Calls are off: Orbiters - N4N0 › N4N0 › Calls")),
            CallLink::SignedOut => (false, mark("Sign in to Orbiters to call friends")),
            CallLink::NoAccount => (false, mark("VRChat's account here is unknown: open VRChat")),
            CallLink::NotLinked { .. } => (false, mark("Link this VRChat account on Orbiters to call")),
        }
    }

    /// The friends who can be called, as N4N0's menu lists them.
    pub fn menu_contacts(&self) -> Vec<orbiters_n4n0_core::menu::Contact> {
        self.contacts.iter().map(|c| orbiters_n4n0_core::menu::Contact { id: c.id.clone(), name: c.name.clone() }).collect()
    }

    /// The call as N4N0's menu shows it, at Unix time `now_ms`.
    pub fn menu_call(&self, now_ms: u64) -> Option<orbiters_n4n0_core::menu::Call> {
        use orbiters_n4n0_core::menu::Call;
        self.call.as_ref().map(|c| match c.phase {
            CallPhase::Ringing => Call::Ringing { name: c.name.clone() },
            CallPhase::Calling | CallPhase::Connecting => Call::Calling { name: c.name.clone() },
            CallPhase::Connected => {
                Call::Connected { name: c.name.clone(), seconds: c.connected_at_ms.map_or(0, |at| now_ms.saturating_sub(at) / 1000), muted: c.muted }
            }
        })
    }
}
