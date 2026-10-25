//! What N4N0 shows the apps (their window, the SteamVR overlay, a phone's screen): [`OrbStatus`], republished on every
//! change, with what the person is told ([`NoticeKind`]).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    Info,
    Success,
    Warning,
    Error,
}

/// What N4N0 shows (`N4N0/State`, contracts `Docs/orb.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrbFace {
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

impl OrbFace {
    /// Every state, in `N4N0/State`'s order.
    pub const ALL: [OrbFace; 8] =
        [OrbFace::Idle, OrbFace::Listening, OrbFace::Thinking, OrbFace::Talking, OrbFace::Sleeping, OrbFace::Happy, OrbFace::Annoyed, OrbFace::Surprised];
}

/// Who said a line of N4N0's conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrbWho {
    You,
    Orb,
    /// Something N4N0 did or noticed (a tool ran, a video was saved).
    Note,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrbLine {
    pub who: OrbWho,
    pub text: String,
    /// Unix time in milliseconds.
    pub at_ms: u64,
}

/// An action of N4N0's (publishing an event…) waiting for the member's approval on Orbiters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OrbConfirmation {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub approve_url: String,
}

/// The link of a confirmation the owner gives in Orbiters - N4N0 itself (what N4N0 asks before it does a public,
/// lasting or costly thing), rather than on the Orbiters website: the number of what waits follows it.
pub const LOCAL_APPROVAL: &str = "orbiters-desktop:orb/approve/";

impl OrbConfirmation {
    /// Approved in Orbiters - N4N0 (the N4N0 page's button, or a spoken yes), not on the Orbiters website.
    pub fn is_local(&self) -> bool {
        self.approve_url.starts_with(LOCAL_APPROVAL)
    }
}

/// The speech models and the voice worker.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VoiceStatus {
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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrbStatus {
    /// The avatar VRChat loaded.
    pub avatar_id: Option<String>,
    /// The avatar carries the N4N0 prop, with this many text sync values.
    pub prop_sync: Option<u8>,
    /// The avatar's manifest was found: its menu is known by label.
    pub manifest: bool,
    /// Controls N4N0 can reach on the avatar.
    pub controls: usize,
    pub face: OrbFace,
    /// What the panel shows now.
    pub panel: Option<String>,
    pub listening: bool,
    pub thinking: bool,
    pub afk: bool,
    /// The conversation as shown, the newest last.
    pub lines: Vec<OrbLine>,
    pub confirmations: Vec<OrbConfirmation>,
    /// The free quota of N4N0's assistant, as the last answer told it.
    pub quota: Option<orbiters_orb::assistant::Quota>,
    pub voice: VoiceStatus,
    /// Saved videos, the newest first.
    pub videos: Vec<orbiters_orb::videos::SavedVideo>,
    /// Orbiters' processes and VRChat's.
    pub resources: Vec<ProcessUsage>,
    /// Testing: the state N4N0 is held in, every state shown in turn, the owner acted as away.
    #[serde(default)]
    pub test_face: Option<OrbFace>,
    #[serde(default)]
    pub test_cycle: bool,
    #[serde(default)]
    pub test_afk: bool,
    /// Testing: the whole orb is hidden (`N4N0/Visible` sent false).
    #[serde(default)]
    pub test_hidden: bool,
    /// Calls with VRChat friends (the app's own: [`crate::Host::calls`]).
    #[serde(default)]
    pub calls: CallsStatus,
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
    /// N4N0's light, when the avatar's N4N0 has one (the Light option).
    #[serde(default)]
    pub light: Option<OrbLight>,
    /// The avatar worn's saved states.
    #[serde(default)]
    pub states: StatesView,
    /// A copied link can be dropped in the world as a card: the avatar's N4N0 screen is of version 5 or later.
    #[serde(default)]
    pub can_drop: bool,
    /// The link card the avatar's screen shows now, dropped in the world.
    #[serde(default)]
    pub card: Option<CardView>,
}

/// The saved states of the avatar worn, as the window and the menus show them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatesView {
    /// The avatar they are for (`None`: VRChat's avatar is not known yet).
    pub avatar_id: Option<String>,
    /// What VRChat's log calls the avatar.
    pub avatar_name: Option<String>,
    pub states: Vec<StateView>,
    /// What the last save, apply or change said (an error too).
    pub note: Option<StatesNote>,
    /// A save is under way.
    pub saving: bool,
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
    /// Save how the avatar is now (`None`: named "State n").
    Save {
        name: Option<String>,
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

/// N4N0's light as the avatar holds it (`N4N0/Light`, `N4N0/Light/Color`, `N4N0/Light/Level`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OrbLight {
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
    #[default]
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

impl CallsStatus {
    /// Whether N4N0's menu can call friends here and, when not, what it says instead.
    pub fn menu_ready(&self) -> (bool, &'static str) {
        match &self.link {
            CallLink::Linked { .. } => (true, ""),
            CallLink::Off => (false, "Calls are off: Orbiters - N4N0 › N4N0 › Calls"),
            CallLink::SignedOut => (false, "Sign in to Orbiters to call friends"),
            CallLink::NoAccount => (false, "VRChat's account here is unknown: open VRChat"),
            CallLink::NotLinked { .. } => (false, "Link this VRChat account on Orbiters to call"),
        }
    }

    /// The friends who can be called, as N4N0's menu lists them.
    pub fn menu_contacts(&self) -> Vec<orbiters_orb::menu::Contact> {
        self.contacts.iter().map(|c| orbiters_orb::menu::Contact { id: c.id.clone(), name: c.name.clone() }).collect()
    }

    /// The call as N4N0's menu shows it, at Unix time `now_ms`.
    pub fn menu_call(&self, now_ms: u64) -> Option<orbiters_orb::menu::Call> {
        use orbiters_orb::menu::Call;
        self.call.as_ref().map(|c| match c.phase {
            CallPhase::Ringing => Call::Ringing { name: c.name.clone() },
            CallPhase::Calling | CallPhase::Connecting => Call::Calling { name: c.name.clone() },
            CallPhase::Connected => {
                Call::Connected { name: c.name.clone(), seconds: c.connected_at_ms.map_or(0, |at| now_ms.saturating_sub(at) / 1000), muted: c.muted }
            }
        })
    }
}
