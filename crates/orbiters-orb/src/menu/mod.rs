//! N4N0's menu (contracts `Docs/orb.md`, Menu): what opens when the person calls N4N0 (a controller button, the wake word
//! on its own, touching N4N0): fast walk, YouTube, calls, a 3D picture (and sharing 3D captures), the computer's music,
//! N4N0's light and the avatar's saved states. Orbiters - N4N0's SteamVR overlay shows it, or the avatar's projected screen when the N4N0 prop
//! has one; both draw the same pages from [`layout`]. A link copied from the results can be dropped in the world as a card
//! (contracts `Docs/orb.md`, Link card), which the avatar's screen shows.
//!
//! This module holds what the menu shows and what each press does, without IO: presses and typing return [`Effect`]s
//! that the host carries out (fetching suggestions and results, copying a link, starting a call), and the host hands the
//! answers back. [`screen`] turns a menu into what the avatar's screen is sent; [`touch`] finds where a finger touches it.

pub mod layout;
pub mod pictures;
pub mod retro;
pub mod screen;
pub mod strings;
pub mod theme;
pub mod touch;

use crate::captures::{CaptureRow, CapturesView, Friend, Friends, ShareAsk};
use crate::feedback::{FeedbackAsk, FeedbackView, Subject, WorldFeedback, COMMENTS_PER_PAGE};
use crate::youtube::{Results, Video};
use std::time::{Duration, Instant};

/// Results on a page of the list.
pub const RESULTS_PER_PAGE: usize = 5;
/// Suggestions under the search field.
pub const SUGGESTIONS: usize = 5;
/// Contacts on a page of the call list.
pub const CONTACTS_PER_PAGE: usize = 5;
/// Characters the search field holds (the screen's field shows them all).
pub const QUERY_MAX: usize = 40;
/// The 3D picture's timer choices, in seconds (0: at once).
pub const TIMERS: [u32; 4] = [0, 3, 5, 10];
/// How much a volume button turns the computer's volume, in percent.
pub const VOLUME_STEP: u8 = 5;
/// Saved states on a page of the States list.
pub const STATES_PER_PAGE: usize = 5;
/// The segments of the music page's timeline (from screen version 9): a press on segment `i` moves the music to `i` /
/// this of its length.
pub const TIMELINE_STEPS: usize = 16;
/// Previous instances on a page of their list.
pub const INSTANCES_PER_PAGE: usize = 5;
/// 3D captures on a page of History, and friends on a page of Share to ….
pub const CAPTURES_PER_PAGE: usize = 5;
pub const FRIENDS_PER_PAGE: usize = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Page {
    #[default]
    Home,
    /// The search field, its suggestions and the keyboard.
    Search,
    Results,
    /// Friends to call; a call under way shows instead.
    Call,
    Photo,
    /// What the computer plays (Windows' media controls) and its volume.
    Music,
    /// N4N0 as a light: on or off, its colour and brightness.
    Light,
    /// The avatar's saved states: apply one, make a new one.
    States,
    /// A link card dropped in the world (only the avatar's screen shows it, while the menu itself is closed).
    Card,
    /// A state being made: its name, what it puts back, Save.
    NewState,
    /// The keyboard for a text that is not a search: a state's name, a suggestion ([`Menu::typing`]).
    Type,
    /// SteamVR's tracking smoothed (headset, controllers, trackers).
    Tracking,
    /// A feature to suggest to Orbiters, typed or said.
    Suggest,
    /// The simple Home's More (from screen version 10): light, previous instances, smooth tracking, suggest.
    More,
    /// The instances the player was in, each with Invite me (Orbiters' VRChat account invites them there).
    Instances,
    /// Orbiters' VRChat account has to be the player's friend to invite them: its name to copy, its page to open.
    AddFriend,
    /// The QR code scanner (from screen version 11): VRChat's stream camera is read while it shows, a QR code's web link
    /// opens on the computer.
    Qr,
    /// The 3D captures on the computer, each with Share publicly and Share to … (from screen version 12).
    History,
    /// The friends a capture goes to: choose them, Send.
    ShareTo,
    /// Sharing a capture flagged explicit publicly: the warning, Share anyway or Cancel.
    Explicit,
    /// A world's feedback (from screen version 13): its likes and Like, the labels people gave it most, its newest
    /// comment, the worlds to step through.
    Feedback,
    /// The world's labels, to give or take back.
    FeedbackLabels,
    /// The world's comments, three a page: Write a comment, Delete on the player's own.
    Comments,
}

/// What the keyboard page ([`Page::Type`]) types.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Typing {
    /// The name of the state being made.
    #[default]
    StateName,
    /// A feature suggestion.
    Suggestion,
    /// Part of a friend's name, for Share to … (from screen version 12).
    Friend,
    /// A comment on the world World feedback shows (from screen version 13).
    Comment,
    /// Why the world is what the label waiting for it says (the AI world label's justification).
    Justification,
}

impl Typing {
    /// The most characters it holds.
    pub fn max(self) -> usize {
        match self {
            Typing::StateName => STATE_NAME_MAX,
            Typing::Suggestion => SUGGESTION_MAX,
            Typing::Friend => crate::captures::FIND_MAX,
            Typing::Comment => orbiters_contracts::world_feedback::max_comment(),
            Typing::Justification => orbiters_contracts::world_feedback::max_justification(),
        }
    }
}

/// A state's name, in characters.
pub const STATE_NAME_MAX: usize = 32;
/// A suggestion, in characters (the server takes up to 2000; the screen shows the end of a longer one).
pub const SUGGESTION_MAX: usize = 400;

/// The video the world's player plays now (VRChat's log), as the Music page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorldVideo {
    pub url: String,
    /// Its title, once known (YouTube's).
    pub title: Option<String>,
    /// When it started (Unix seconds), for watching it from where the world is.
    pub at: Option<u64>,
}

/// The link to watch `video` at, `now` (Unix seconds): a YouTube video from where the world's player is (it started
/// `now - at` ago, for up to six hours), anything else as it is.
pub fn watch_link(video: &WorldVideo, now: u64) -> String {
    let Some(id) = crate::videos::youtube_id(&video.url) else { return video.url.clone() };
    match video.at.map(|at| now.saturating_sub(at)) {
        Some(seconds) if (5..6 * 3600).contains(&seconds) => format!("https://www.youtube.com/watch?v={id}&t={seconds}s"),
        _ => format!("https://www.youtube.com/watch?v={id}"),
    }
}

/// A key of the computer's keyboard, for the menu's keyboard ([`Menu::key`]).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "key", content = "text", rename_all = "snake_case")]
pub enum KeyInput {
    /// What a key writes, in the keyboard's layout.
    Text(String),
    Backspace,
    /// Search, or the keyboard page's Done.
    Enter,
    /// Back.
    Escape,
    /// Ctrl+V: the clipboard's text.
    Paste,
}

/// The state the New state page makes: its name and what applying it puts back besides the parameters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub name: String,
    /// The avatar's size (its eye height): on unless the person turns it off.
    pub size: bool,
    /// GoGoLoco's pose.
    pub pose: bool,
    /// Applied after each world join and each switch to this avatar.
    pub default: bool,
}

/// SteamVR's tracking smoothed per kind of device (Orbiters - N4N0's smoothing driver), as the Tracking page shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tracking {
    /// Smoothing is on.
    pub on: bool,
    /// Each kind's strength, an index of [`SMOOTHING_LABELS`]: the headset, the controllers, the trackers.
    pub levels: [u8; 3],
    /// What the driver does now ("Smoothing your devices now", "Ready: smoothing starts with SteamVR"), or why it cannot.
    pub note: String,
}

/// The kinds of devices the Tracking page smooths, in its rows' order (in English: [`strings::TRACKING_KINDS`]).
pub const TRACKING_KINDS: [&str; 3] = [strings::TRACKING_KINDS[0].en(), strings::TRACKING_KINDS[1].en(), strings::TRACKING_KINDS[2].en()];
/// The strengths it offers, and what each is (0 leaves the device as SteamVR tracks it, 1 the strongest; in English:
/// [`strings::SMOOTHING`]).
pub const SMOOTHING_LABELS: [&str; 4] = [strings::SMOOTHING[0].en(), strings::SMOOTHING[1].en(), strings::SMOOTHING[2].en(), strings::SMOOTHING[3].en()];
pub const SMOOTHING_STRENGTHS: [f32; 4] = [0.0, 0.35, 0.65, 0.9];

/// The pages a display has (contracts `Docs/orb.md`, Menu): the avatar's screen by its version (`N4N0/Screen/Version`),
/// the overlay and the window all of them ([`Pages::ALL`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pages {
    /// Screens before version 3: the four-tile Home, no Music nor Light.
    #[default]
    Four,
    /// Versions 3 and 4: the six-tile Home, Music and Light.
    Six,
    /// From version 5: the seven-tile Home (States), the States page, Drop on the results and the link card.
    Seven,
    /// From version 6: the ten-tile Home (Tracking, Suggest features, the versions), New state, the keyboard for
    /// names and suggestions, Tracking and Suggest features.
    Ten,
    /// From version 9: the music page's timeline (tapped or dragged to move in what plays), and Take back on the
    /// results and the music page while the link card is out.
    Timeline,
    /// From version 10: the simple Home (YouTube, 3D capture and More large; fast walk, calls and states under them;
    /// what plays at the bottom when something does), More, Previous instances and Add Orbiters as a friend.
    Simple,
    /// From version 11: More with five cards (the QR code scanner among them) and the QR code scanner's page.
    Qr,
    /// From version 12: the 3D picture page with the last capture and its shares, History, Share to … and the explicit
    /// warning.
    Share,
    /// From version 13: World feedback (likes, labels, comments), its labels and comments, More with room for eight cards.
    Feedback,
}

impl Pages {
    /// Every page: the overlay's and the window's.
    pub const ALL: Pages = Pages::Feedback;

    /// The pages of a screen of `version` (`None`: a screen from before `N4N0/Screen/Version`).
    pub fn of_screen(version: Option<i32>) -> Pages {
        use orbiters_contracts::orb::{
            SCREEN_FEEDBACK, SCREEN_MUSIC_LIGHT, SCREEN_QR, SCREEN_SHARE, SCREEN_SIMPLE, SCREEN_STATES_CARDS, SCREEN_TEN_TILES, SCREEN_TIMELINE,
        };
        match version {
            Some(v) if v >= SCREEN_FEEDBACK => Pages::Feedback,
            Some(v) if v >= SCREEN_SHARE => Pages::Share,
            Some(v) if v >= SCREEN_QR => Pages::Qr,
            Some(v) if v >= SCREEN_SIMPLE => Pages::Simple,
            Some(v) if v >= SCREEN_TIMELINE => Pages::Timeline,
            Some(v) if v >= SCREEN_TEN_TILES => Pages::Ten,
            Some(v) if v >= SCREEN_STATES_CARDS => Pages::Seven,
            Some(v) if v >= SCREEN_MUSIC_LIGHT => Pages::Six,
            _ => Pages::Four,
        }
    }

    /// Home has the music and light tiles (and their pages).
    pub fn music_light(self) -> bool {
        self >= Pages::Six
    }

    /// Home has the States tile (and its page), the results Drop, and links drop as cards.
    pub fn states_cards(self) -> bool {
        self >= Pages::Seven
    }

    /// Home has the Tracking and Suggest features tiles and the versions; States makes a new state on its own page.
    pub fn ten(self) -> bool {
        self >= Pages::Ten
    }

    /// The music page has its timeline, and the results and the music page Take back.
    pub fn timeline(self) -> bool {
        self >= Pages::Timeline
    }

    /// Home is the simple one, with More, Previous instances and Add Orbiters as a friend.
    pub fn simple(self) -> bool {
        self >= Pages::Simple
    }

    /// More has the QR code scanner (and its page).
    pub fn qr(self) -> bool {
        self >= Pages::Qr
    }

    /// The 3D picture page shares the last capture and opens History (with Share to … and the explicit warning).
    pub fn share(self) -> bool {
        self >= Pages::Share
    }

    /// More has World feedback (with its labels and comments pages).
    pub fn feedback(self) -> bool {
        self >= Pages::Feedback
    }
}

/// An instance the player was in (VRChat's logs), as Previous instances lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstanceRow {
    /// VRChat's location, `wrld_…:<instance>`, which an invite names.
    pub location: String,
    /// The world's name, when its join said it.
    pub world: String,
    /// When it was joined last (Unix seconds).
    pub at: u64,
}

impl InstanceRow {
    /// Who may join it, as VRChat names it: Public, Friends+, Friends, Invite+, Invite, Group, Group+, Group Public.
    pub fn kind(&self) -> &'static str {
        let tags = self.location.split_once('~').map_or("", |(_, t)| t);
        let has = |tag: &str| tags.split('~').any(|t| t.starts_with(tag));
        if has("group(") {
            return match tags.split('~').find_map(|t| t.strip_prefix("groupAccessType(")).map(|t| t.trim_end_matches(')')) {
                Some("public") => "Group Public",
                Some("plus") => "Group+",
                _ => "Group",
            };
        }
        if has("private(") {
            return if has("canRequestInvite") { "Invite+" } else { "Invite" };
        }
        if has("friends(") {
            return "Friends";
        }
        if has("hidden(") {
            return "Friends+";
        }
        "Public"
    }

    /// Its region (`EU`, `JP`, `US W`, `US E`), when the location says.
    pub fn region(&self) -> Option<&'static str> {
        let tags = self.location.split_once('~').map_or("", |(_, t)| t);
        let region = tags.split('~').find_map(|t| t.strip_prefix("region(")).map(|r| r.trim_end_matches(')'))?;
        match region {
            "eu" => Some("EU"),
            "jp" => Some("JP"),
            "us" => Some("US W"),
            "use" => Some("US E"),
            _ => None,
        }
    }
}

/// Orbiters' VRChat account, which invites the player: its VRChat name and its page.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InviteAccount {
    pub name: String,
    pub link: String,
}

/// How an Invite me went (Orbiters' answer).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum InviteAnswer {
    /// VRChat has the invite.
    Sent,
    /// Orbiters' account is not the player's friend yet (or is accepting their friend request): add it, then try again.
    NeedsFriend { account: InviteAccount, message: String },
    /// Why it was not sent.
    Failed { message: String },
}

/// A saved state of the avatar worn, as the menu lists it (Orbiters - N4N0 keeps them: contracts `Docs/orb.md`, States).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StateRow {
    pub id: String,
    pub name: String,
    /// Applied after each world join and each switch to this avatar.
    pub default: bool,
    /// Applying it puts the GoGoLoco pose back too, and the avatar's size.
    pub pose: bool,
    pub size: bool,
}

/// A link card dropped in the world (contracts `Docs/orb.md`, Link card): its picture (a video id: the hosts keep
/// pictures by key), title and link, and the whole seconds it has left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub link: String,
    pub title: String,
    pub video: Option<String>,
    pub left: u32,
}

/// The letters' order on the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KeyboardLayout {
    #[default]
    Qwerty,
    Azerty,
}

/// The keyboard's keys: letters, or digits, symbols and accented letters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Keys {
    #[default]
    Letters,
    Symbols,
}

/// A 3D picture's camera flight (Orbiters - N4N0's `Photo3dLength`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PhotoLength {
    /// 1 s.
    Quick,
    /// 4 s.
    #[default]
    Standard,
    /// 8 s.
    Full,
}

impl PhotoLength {
    pub const ALL: [PhotoLength; 3] = [PhotoLength::Quick, PhotoLength::Standard, PhotoLength::Full];

    pub fn seconds(self) -> u32 {
        match self {
            PhotoLength::Quick => 1,
            PhotoLength::Standard => 4,
            PhotoLength::Full => 8,
        }
    }
}

/// What the computer plays, as Windows' media controls tell it (the app playing last, or playing now).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The app playing it ("Spotify").
    pub app: String,
    pub playing: bool,
    pub can_play_pause: bool,
    pub can_previous: bool,
    pub can_next: bool,
    /// The app lets others move in its timeline ([`MediaCommand::Seek`]).
    pub can_seek: bool,
    /// Where it is and how long it lasts, in seconds, when the app says.
    pub position: Option<u32>,
    pub duration: Option<u32>,
    /// The art's picture key (the hosts keep the pictures by key), once there is art.
    pub art: Option<String>,
    /// The YouTube video it is (its id), once found: a browser or YouTube's app plays it (`orbiters_n4n0::media` looks
    /// it up). Its link can be copied and dropped as the results' are.
    pub video: Option<String>,
}

impl Media {
    /// The link of the YouTube video playing, once found.
    pub fn link(&self) -> Option<String> {
        self.video.as_deref().map(crate::youtube::link)
    }
}

/// The computer's volume (its default speakers), in percent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Volume {
    pub percent: u8,
    pub muted: bool,
}

/// What the music page (or N4N0's assistant) asks of the computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Play,
    Pause,
    Previous,
    Next,
    /// To this many seconds from the start of what plays (when the app lets others move in its timeline).
    Seek(u32),
    /// The volume to this percent (unmuted).
    Volume(u8),
    Mute(bool),
}

/// N4N0's light as the avatar holds it (`N4N0/Light/On`, `N4N0/Light/Color`, `N4N0/Light/Level`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Light {
    pub on: bool,
    /// An index into contracts `orb::LIGHT_COLORS`.
    pub color: u8,
    /// 0 to 1.
    pub level: f32,
}

impl Light {
    /// The brightness choice (contracts `orb::LIGHT_LEVELS`) nearest the level.
    pub fn level_choice(&self) -> usize {
        let levels = orbiters_contracts::orb::LIGHT_LEVELS;
        (0..levels.len()).min_by(|&a, &b| (levels[a].1 - self.level).abs().total_cmp(&(levels[b].1 - self.level).abs())).unwrap_or(0)
    }
}

/// A friend who can be called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    pub id: String,
    pub name: String,
}

/// A call as the menu shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    /// A friend calls: answer or decline.
    Ringing { name: String },
    /// Our call rings at theirs, or connects.
    Calling { name: String },
    /// Talking, since `seconds` ago.
    Connected { name: String, seconds: u64, muted: bool },
}

/// What can be pressed. The layout of each page lists where.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Close,
    Back,
    // Home's tiles.
    FastWalk,
    YouTube,
    Call,
    Photo,
    Music,
    Light,
    // Search.
    Field,
    Search,
    Suggestion(u8),
    Key(char),
    Space,
    Backspace,
    /// Switches to digits and symbols.
    Symbols,
    /// Back to letters.
    Letters,
    // Results.
    Copy(u8),
    Previous,
    Next,
    // Calls.
    Contact(u8),
    Answer,
    HangUp,
    // 3D picture.
    Timer(u8),
    Length(u8),
    Take,
    // Music.
    MediaPrevious,
    MediaPlayPause,
    MediaNext,
    VolumeDown,
    VolumeUp,
    Mute,
    // Light.
    LightSwitch,
    LightColor(u8),
    LightLevel(u8),
    // Home's seventh tile, and its page.
    States,
    /// A saved state's row, or its Apply button.
    StateApply(u8),
    /// The mark on the default state's row (it applies the state, as the row does).
    StateMark(u8),
    /// The States page's New state (on screens before version 6: saves how the avatar is now, named "State n").
    StateSave,
    // Results: a copied link dropped in the world.
    Drop(u8),
    /// A segment of a dropped card's countdown bar (nothing to press).
    // New state: its name (opens the keyboard), what it puts back, Save.
    DraftName,
    DraftSize,
    DraftPose,
    DraftDefault,
    DraftSave,
    /// The keyboard page's Done: the text goes where it was typed for.
    Done,
    // Home's eighth and ninth tiles.
    Tracking,
    Suggest,
    /// Tracking: smoothing on or off, and a kind's strength (kind × 4 + strength: [`TRACKING_KINDS`] ×
    /// [`SMOOTHING_LABELS`]).
    TrackingSwitch,
    Smoothing(u8),
    /// Suggest features: type it, say it, send it.
    SuggestType,
    SuggestTalk,
    SuggestSend,
    /// Music: the world's video, watched on the computer from where the world is, or saved in N4N0's videos.
    WatchVideo,
    SaveVideo,
    /// Music: the YouTube video the computer plays, its link copied, then dropped in the world as a card (as a result's).
    CopyPlaying,
    DropPlaying,
    /// Music's timeline (from screen version 9): its segment `i` of [`TIMELINE_STEPS`], lit once the music reached it, and
    /// the place over it that a finger presses or drags along. Either moves the music to the segment's start.
    Timeline(u8),
    Seek(u8),
    /// The link card back from the world (from screen version 9: on the results and the music page while it is out).
    TakeBack,
    // The simple Home (from screen version 10): More, and on More, Previous instances.
    More,
    Instances,
    /// A previous instance's row or its Invite me: Orbiters' VRChat account invites the player there.
    InviteMe(u8),
    /// Add Orbiters as a friend: its name in the clipboard, its VRChat page opened on the computer, the invite again.
    CopyFriendName,
    OpenFriendLink,
    InviteAgain,
    /// More's QR code scanner (from screen version 11), and on its page: scan again once it stopped, the last link
    /// copied, opened again.
    Qr,
    QrAgain,
    QrCopy,
    QrOpen,
    /// The 3D picture page's History (from screen version 12), and its last capture shared publicly or to friends.
    History,
    ShareLatest,
    ShareLatestTo,
    /// History: a capture's Share publicly and Share to ….
    SharePublic(u8),
    ShareTo(u8),
    /// Share to …: a friend chosen or no longer (their row, or its mark shown while they are chosen), Find a friend, Send.
    Friend(u8),
    FriendMark(u8),
    FindFriend,
    SendShare,
    /// The explicit warning: share publicly all the same, or not.
    ShareAnyway,
    CancelShare,
    /// More's World feedback (from screen version 13), and on its page: Like, one of the labels people gave most (the
    /// player gives it or takes it back), the labels page, the comments (the newest's panel, or the button), Write a
    /// comment. Previous and Next step through the worlds there, through the pages on Comments.
    WorldFeedback,
    Like,
    TopLabel(u8),
    Labels,
    Comments,
    WriteComment,
    /// The labels page's label `i` of the list (contracts `Schemas/world-feedback-labels.json`).
    LabelChip(u8),
    /// Comments: a comment's card (nothing to press), and Delete on the player's own.
    CommentCard(u8),
    DeleteComment(u8),
}

/// The timeline segment a target is (the segment, or the place over it), for a drag along the timeline.
pub fn timeline_segment(target: Target) -> Option<u8> {
    match target {
        Target::Timeline(i) | Target::Seek(i) => Some(i),
        _ => None,
    }
}

/// What the host does for the menu. Requests are numbered, and their answers carry the number back: an answer to an
/// older request (the same words searched again, a slower suggestion) never replaces a newer one's.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Suggestions for this text (asked once per text; see [`SuggestQueue`] for how the hosts send them).
    Suggest {
        id: u64,
        text: String,
    },
    /// Results for `query`: search `id`, whose next pages carry the same number.
    Search {
        id: u64,
        query: String,
    },
    /// The next page of search `id`.
    More {
        id: u64,
        query: String,
        token: String,
    },
    /// Thumbnails the page shows, then the next page's (video ids); the host fetches those it does not have.
    Thumbnails(Vec<String>),
    /// Put a video's link in the clipboard.
    Copy {
        link: String,
        title: String,
    },
    FastWalk(bool),
    CallContact {
        id: String,
    },
    Answer,
    HangUp,
    /// Take a 3D picture after `timer` seconds.
    Photo3d {
        timer: u32,
        length: PhotoLength,
    },
    /// Keep the picked timer and length.
    PhotoChoice {
        timer: u32,
        length: PhotoLength,
    },
    /// Tell the computer's media controls or its volume.
    Media(MediaCommand),
    /// Set N4N0's light so (its three parameters).
    Light(Light),
    /// Save how the avatar is now as a state named `name`, which puts back its size and GoGoLoco's pose too when they
    /// are on, and is the avatar's default when `default` is.
    SaveState {
        name: String,
        size: bool,
        pose: bool,
        default: bool,
    },
    /// Smooth SteamVR's tracking so (the Tracking page).
    Tracking {
        on: bool,
        levels: [u8; 3],
    },
    /// Listen to the person for the suggestion they dictate (Suggest features' Talk).
    Dictate,
    /// Send this suggestion to Orbiters (`voice`: some of it was said, not typed).
    SendSuggestion {
        text: String,
        voice: bool,
    },
    /// Open this video on the computer (the world's, from where its player is).
    WatchVideo {
        link: String,
    },
    /// Save the world's video in N4N0's videos.
    SaveVideo,
    /// Put the avatar back as state `id` (named `name`) holds it.
    ApplyState {
        id: String,
        name: String,
    },
    /// Drop a link card in the world: the avatar's screen shows it (contracts `Docs/orb.md`, Link card).
    Drop {
        link: String,
        title: String,
        /// The thumbnail's key (the video id).
        video: Option<String>,
    },
    /// Take the link card out in the world back (as when its time is up).
    TakeBack,
    /// The instances the player was in, for Previous instances (the host answers with [`Menu::set_instances`]).
    Instances,
    /// Orbiters' VRChat account invites the player to this location (the host answers with [`Menu::invite_answered`]).
    InviteMe {
        location: String,
    },
    /// Put a text in the clipboard (Orbiters' account's name).
    CopyText {
        text: String,
    },
    /// Open this page on the computer (Orbiters' account's VRChat page, the QR code scanner's last link).
    OpenLink {
        link: String,
    },
    /// Read VRChat's stream camera for QR codes (`true`: the scanner's page opened, Scan again), or no longer (it closed);
    /// the host answers with [`Menu::qr`].
    QrScan(bool),
    /// List the 3D captures again (History opened); the host answers with [`Menu::set_captures`].
    Captures,
    /// Read the VRChat friends a capture can go to (`true`: Share to … opened), or forget them (it closed); the host
    /// answers with [`Menu::set_friends`].
    Friends(bool),
    /// Share a 3D capture (`Docs/capture-sharing.md`): its progress comes back in its row ([`Menu::set_captures`]).
    ShareCapture(ShareAsk),
    /// Read or send world feedback (`Docs/world-feedback.md`): the host answers with [`Menu::set_feedback`].
    Feedback(FeedbackAsk),
    /// The menu closed.
    Close,
}

/// The menu as both displays draw it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Menu {
    pub open: bool,
    pub page: Page,
    pub keys: Keys,
    pub layout: KeyboardLayout,
    /// What is in the search field.
    pub query: String,
    /// Suggestions for `suggested_for`, the best first.
    pub suggestions: Vec<String>,
    suggested_for: String,
    /// The text suggestions were last asked for (so each text is asked once), the last request's number and the
    /// number of the one shown.
    asked_for: String,
    suggest_id: u64,
    suggested_id: u64,
    /// Results of `results_for`, every page fetched so far.
    pub results: Vec<Video>,
    pub results_for: String,
    continuation: Option<String>,
    /// The search's number (its answers carry it).
    search_id: u64,
    /// The page of results shown (from 0).
    pub result_page: usize,
    pub searching: bool,
    loading_more: bool,
    /// Next was pressed on the last page fetched: the page turns once its results arrive (and stays when none do).
    turning: bool,
    /// N4N0 listens to the person.
    pub listening: bool,
    pub fast_walk: bool,
    /// VRChat runs in desktop mode: fast walk (a move of SteamVR's play space) is greyed out, "Only in VR".
    pub desktop_mode: bool,
    pub contacts: Vec<Contact>,
    pub contact_page: usize,
    pub call: Option<Call>,
    /// Calls are available (signed in, VRChat account linked); otherwise `call_note` says why.
    pub calls_ready: bool,
    pub call_note: String,
    pub photo_timer: u32,
    pub photo_length: PhotoLength,
    /// Seconds left before the 3D picture starts.
    pub countdown: Option<u32>,
    /// A short line for the person: what was copied, what failed. Shown until the next change of page.
    pub notice: Option<String>,
    /// What N4N0 heard last, or what it says (shown on Home).
    pub heard: Option<String>,
    /// `heard` holds the owner's own words: everyone else's screen does not show them (a sentence for N4N0 may have
    /// been kept out of the world, VRChat's microphone muted while it was said).
    pub heard_private: bool,
    /// The pages this display has: the overlay and the window all of them, the avatar's screen those of its version
    /// (`N4N0/Screen/Version`): six tiles from version 3, seven (States, Drop, the link card) from version 5.
    pub pages: Pages,
    /// What the computer plays (`None`: nothing, or not known yet) and its volume.
    pub media: Option<Media>,
    pub volume: Option<Volume>,
    /// N4N0's light, when the avatar has it (the Light option on N4N0's card).
    pub light: Option<Light>,
    /// The avatar worn's saved states, newest last; `states_ready` once the avatar is known (else `states_note` says
    /// why there are none), and the page of them shown (from 0).
    pub states: Vec<StateRow>,
    pub states_ready: bool,
    pub states_note: String,
    pub state_page: usize,
    /// A save pressed here, shown in the list until the host's list has it (or [`SAVE_SHOWN`] passed).
    pending_save: Option<(String, Instant)>,
    /// The result whose link was copied last (its video id): its row offers Drop.
    pub copied: Option<String>,
    /// A copied link can be dropped as a card: the avatar's screen is of version 5 or later.
    pub can_drop: bool,
    /// The link card the avatar's screen shows (the menu's page is then [`Page::Card`]).
    pub card: Option<Card>,
    /// The owner's link card is out in the world: the results and the music page offer Take back (from screen version 9).
    pub card_out: bool,
    /// The state the New state page makes.
    pub draft: Draft,
    /// What the keyboard page types for, and what it holds.
    pub typing: Typing,
    pub typed: String,
    /// Tracking smoothing, when the host offers it (`None`: Tracking says why not, from `tracking_note`).
    pub tracking: Option<Tracking>,
    pub tracking_note: String,
    /// The suggestion being written, whether some of it was said, and whether it is on its way to Orbiters.
    pub suggestion: String,
    pub suggestion_voice: bool,
    pub sending: bool,
    /// N4N0 listens for the suggestion (Talk on Suggest features).
    pub dictating: bool,
    /// The versions at the bottom of Home: N4N0's on the avatar and the app's ("N4N0 1.0.9 · App 0.4.0").
    pub versions: String,
    /// The video the world's player plays (VRChat's log).
    pub world_video: Option<WorldVideo>,
    /// The instances the player was in, the newest first (`instances_ready` once the host read them, else
    /// `instances_note` says why there are none), and the page of them shown (from 0).
    pub instances: Vec<InstanceRow>,
    pub instances_ready: bool,
    pub instances_note: String,
    pub instance_page: usize,
    /// The location an Invite me was asked for (Try again on Add Orbiters as a friend asks it again), and whether
    /// Orbiters' answer is awaited.
    pub inviting: Option<String>,
    pub invite_waiting: bool,
    /// Orbiters' VRChat account to befriend first, once an invite said so.
    pub invite_account: Option<InviteAccount>,
    /// The QR code scanner, as the host runs it.
    pub qr: crate::qr::QrView,
    /// The 3D captures as the host lists them (the last one made, History's), and the page of History shown (from 0).
    pub captures: CapturesView,
    pub capture_page: usize,
    /// The VRChat friends Share to … lists, its page shown (from 0), the friends chosen (in the order chosen) and the
    /// part of a name it lists only those with ([`Typing::Friend`]).
    pub friends: Friends,
    pub friend_page: usize,
    pub chosen: Vec<Friend>,
    pub find: String,
    /// The capture Share to … and the explicit warning are for, and the page they go back to.
    pub sharing: Option<String>,
    pub share_from: Page,
    /// The share whose answer the menu waits for, and its capture's share state when it was asked (or last changed):
    /// the explicit warning shows when the server asks for it, a state left from an earlier share counting for nothing.
    asked: Option<(String, crate::captures::Sharing)>,
    /// World feedback as the host knows it, the world shown (an index of its subjects), the page of Comments shown (from
    /// 0), and the label waiting for its justification on the keyboard.
    pub feedback: FeedbackView,
    pub subject: usize,
    pub comment_page: usize,
    pub justifying: Option<String>,
}

/// How long a state saved here shows before the host's list has it.
const SAVE_SHOWN: Duration = Duration::from_secs(5);

impl Menu {
    pub fn new(layout: KeyboardLayout, photo_timer: u32, photo_length: PhotoLength) -> Self {
        Self { layout, photo_timer, photo_length, ..Default::default() }
    }

    /// Opens on Home (or where it was, when it is open already). A link card the screen showed goes: opening the menu
    /// again takes the card back.
    pub fn open(&mut self) {
        if self.page == Page::Card {
            self.card = None;
            self.open = false;
        }
        if !self.open {
            self.open = true;
            self.go(Page::Home);
        }
    }

    pub fn close(&mut self) -> Vec<Effect> {
        if !self.open {
            return Vec::new();
        }
        self.open = false;
        self.notice = None;
        if self.page == Page::Card {
            self.card = None;
            self.page = Page::Home;
        }
        // The scanner reads VRChat's camera only while its page shows; the friends are kept only while Share to … is up.
        if self.page == Page::Qr {
            return vec![Effect::QrScan(false), Effect::Close];
        }
        if self.choosing_friends() {
            return vec![Effect::Friends(false), Effect::Close];
        }
        vec![Effect::Close]
    }

    /// Share to … shows, or the keyboard finding a friend for it.
    fn choosing_friends(&self) -> bool {
        self.page == Page::ShareTo || (self.page == Page::Type && self.typing == Typing::Friend)
    }

    /// The avatar's screen shows `card`, dropped in the world: its own page, while the menu itself is closed (nothing
    /// on it can be pressed).
    pub fn show_card(&mut self, card: Card) {
        self.card = Some(card);
        self.open = true;
        self.page = Page::Card;
        self.notice = None;
    }

    /// Whether the screen shows a dropped card.
    pub fn card_shown(&self) -> bool {
        self.open && self.page == Page::Card && self.card.is_some()
    }

    /// The dropped card goes (its time is up): the screen shows nothing.
    pub fn end_card(&mut self) -> Option<Card> {
        let card = self.card.take()?;
        if self.page == Page::Card {
            self.open = false;
            self.page = Page::Home;
        }
        Some(card)
    }

    fn go(&mut self, page: Page) {
        self.page = page;
        self.notice = None;
        if page == Page::Search || page == Page::Type {
            self.keys = Keys::Letters;
        }
    }

    /// Opens the keyboard page for `typing`, holding `text`.
    fn type_for(&mut self, typing: Typing, text: String) {
        self.typing = typing;
        self.typed = cut(&text, typing.max());
        self.go(Page::Type);
    }

    /// The page the keyboard page came from, and goes back to.
    fn typed_from(&self) -> Page {
        match self.typing {
            Typing::StateName => Page::NewState,
            Typing::Suggestion => Page::Suggest,
            Typing::Friend => Page::ShareTo,
            Typing::Comment => Page::Comments,
            Typing::Justification => Page::FeedbackLabels,
        }
    }

    /// Saves the state as `draft` says: listed at once (until the host's list has it), the States page showing it.
    fn save_draft(&mut self, draft: Draft) -> Vec<Effect> {
        let name = if draft.name.trim().is_empty() { next_state_name(&self.states) } else { cut(draft.name.trim(), STATE_NAME_MAX) };
        self.pending_save = Some((name.clone(), Instant::now()));
        self.states.push(StateRow { name: name.clone(), size: draft.size, pose: draft.pose, default: draft.default, ..Default::default() });
        self.go(Page::States);
        // The page with the new one shows.
        self.state_page = (self.states.len() - 1) / STATES_PER_PAGE;
        self.notice = Some(format!("Saved as \"{name}\""));
        vec![Effect::SaveState { name, size: draft.size, pose: draft.pose, default: draft.default }]
    }

    /// The suggestion reached Orbiters (`Ok`), or did not (why): sent, it is cleared and thanked for; else it stays.
    pub fn suggestion_sent(&mut self, result: Result<(), String>) {
        self.sending = false;
        match result {
            Ok(()) => {
                self.suggestion.clear();
                self.suggestion_voice = false;
                self.notice = Some("Thank you! Your suggestion reached Orbiters".into());
            }
            Err(e) => self.notice = Some(format!("Not sent: {}", cut(&e, 60))),
        }
    }

    /// Listening for the suggestion ended (heard or not).
    pub fn dictation_ended(&mut self) {
        self.dictating = false;
    }

    /// Whether a sentence N4N0 hears now is for the menu's page (what to search for, a suggestion, a state's name)
    /// rather than for N4N0's assistant.
    pub fn takes_speech(&self) -> bool {
        self.open && matches!(self.page, Page::Search | Page::Suggest | Page::NewState | Page::Type)
    }

    /// Whether the menu shows its keyboard (YouTube's search, a name or a suggestion being typed): the computer's
    /// keyboard types there then.
    pub fn shows_keyboard(&self) -> bool {
        self.open && matches!(self.page, Page::Search | Page::Type)
    }

    /// A key of the computer's keyboard while the menu's keyboard shows (`paste`: the clipboard's text, for Ctrl+V).
    pub fn key(&mut self, input: KeyInput, paste: impl FnOnce() -> Option<String>) -> Vec<Effect> {
        if !self.shows_keyboard() {
            return Vec::new();
        }
        match input {
            KeyInput::Text(text) => self.type_text(&text),
            KeyInput::Backspace => self.backspace(),
            KeyInput::Enter => self.press(if self.page == Page::Type { Target::Done } else { Target::Search }),
            KeyInput::Escape => self.press(Target::Back),
            KeyInput::Paste => match paste() {
                Some(text) => self.type_text(&text.replace(['\r', '\n'], " ")),
                None => Vec::new(),
            },
        }
    }

    /// The person pressed `target`. Leaving the QR code scanner's page stops it.
    pub fn press(&mut self, target: Target) -> Vec<Effect> {
        let scanner = self.page == Page::Qr && self.open;
        let friends = self.choosing_friends() && self.open;
        let mut effects = self.press_on(target);
        if scanner && self.open && self.page != Page::Qr {
            effects.push(Effect::QrScan(false));
        }
        // Leaving Share to … (but for its keyboard) forgets the friends.
        if friends && self.open && !self.choosing_friends() {
            effects.push(Effect::Friends(false));
        }
        effects
    }

    fn press_on(&mut self, target: Target) -> Vec<Effect> {
        // A dropped card is not the menu: nothing on it is pressed.
        if !self.open || self.page == Page::Card {
            return Vec::new();
        }
        match target {
            Target::Close => self.close(),
            Target::Back => {
                let to = match self.page {
                    Page::Results => Page::Search,
                    Page::Type => self.typed_from(),
                    Page::NewState => Page::States,
                    Page::AddFriend => Page::Instances,
                    Page::History => Page::Photo,
                    Page::ShareTo | Page::Explicit => self.share_from,
                    Page::FeedbackLabels | Page::Comments => Page::Feedback,
                    Page::Feedback if self.pages.simple() => Page::More,
                    // The simple Home's More holds these.
                    Page::Light | Page::Tracking | Page::Suggest | Page::Instances | Page::Qr if self.pages.simple() => Page::More,
                    _ => Page::Home,
                };
                self.go(to);
                Vec::new()
            }
            Target::FastWalk if self.desktop_mode => Vec::new(),
            Target::FastWalk => {
                self.fast_walk = !self.fast_walk;
                vec![Effect::FastWalk(self.fast_walk)]
            }
            Target::YouTube => {
                self.go(Page::Search);
                Vec::new()
            }
            Target::Call => {
                self.go(Page::Call);
                Vec::new()
            }
            Target::Photo => {
                self.go(Page::Photo);
                self.capture_thumbnails()
            }
            Target::Music => {
                self.go(Page::Music);
                Vec::new()
            }
            Target::Light => {
                self.go(Page::Light);
                Vec::new()
            }
            Target::Field => Vec::new(),
            Target::Search => self.search(),
            Target::Suggestion(i) => {
                let picked = self.shown_suggestions().nth(i as usize).map(str::to_string);
                match picked {
                    Some(text) => {
                        self.query = cut(&text, QUERY_MAX);
                        self.search()
                    }
                    None => Vec::new(),
                }
            }
            Target::Key(c) => self.type_text(&c.to_string()),
            Target::Space => self.type_text(" "),
            Target::Backspace => self.backspace(),
            Target::Symbols => {
                self.keys = Keys::Symbols;
                Vec::new()
            }
            Target::Letters => {
                self.keys = Keys::Letters;
                Vec::new()
            }
            Target::Copy(i) => match self.page_results().get(i as usize) {
                Some(video) => {
                    let effect = Effect::Copy { link: video.link(), title: video.title.clone() };
                    self.copied = Some(video.id.clone());
                    // The row now offers Drop, where the display can drop it.
                    let notice = if self.can_drop { "Link copied: paste it, or Drop it" } else { "Link copied: paste it in the video player" };
                    self.notice = Some(notice.into());
                    vec![effect]
                }
                None => Vec::new(),
            },
            Target::Drop(i) => match self.page_results().get(i as usize) {
                Some(video) if self.can_drop && self.copied.as_deref() == Some(video.id.as_str()) => {
                    let effect = Effect::Drop { link: video.link(), title: video.title.clone(), video: Some(video.id.clone()) };
                    self.notice = Some("Dropped: grab the card to copy its link".into());
                    vec![effect]
                }
                _ => Vec::new(),
            },
            Target::States => {
                self.go(Page::States);
                Vec::new()
            }
            Target::StateApply(i) | Target::StateMark(i) => match self.page_states().get(i as usize) {
                // A state saved here but not yet listed by the host has no id: nothing to apply yet.
                Some(state) if !state.id.is_empty() => {
                    let effect = Effect::ApplyState { id: state.id.clone(), name: state.name.clone() };
                    self.notice = Some(format!("Applied \"{}\"", state.name));
                    vec![effect]
                }
                _ => Vec::new(),
            },
            // New state: its own page from screen version 6; older screens save at once, size included.
            Target::StateSave if self.states_ready && self.pages.ten() => {
                self.draft = Draft { name: next_state_name(&self.states), size: true, pose: false, default: false };
                self.go(Page::NewState);
                Vec::new()
            }
            Target::StateSave if self.states_ready => self.save_draft(Draft { name: String::new(), size: true, pose: false, default: false }),
            Target::StateSave => Vec::new(),
            Target::DraftName => {
                self.type_for(Typing::StateName, self.draft.name.clone());
                Vec::new()
            }
            Target::DraftSize => {
                self.draft.size = !self.draft.size;
                Vec::new()
            }
            Target::DraftPose => {
                self.draft.pose = !self.draft.pose;
                Vec::new()
            }
            Target::DraftDefault => {
                self.draft.default = !self.draft.default;
                Vec::new()
            }
            Target::DraftSave if self.states_ready && self.page == Page::NewState => self.save_draft(self.draft.clone()),
            Target::DraftSave => Vec::new(),
            Target::Done => {
                let text = self.typed.trim().to_string();
                match self.typing {
                    Typing::StateName if !text.is_empty() => self.draft.name = text,
                    Typing::StateName => {}
                    Typing::Suggestion => self.suggestion = cut(&text, SUGGESTION_MAX),
                    Typing::Friend => {
                        self.find = text;
                        self.friend_page = 0;
                    }
                    Typing::Comment => return self.send_comment(&text),
                    Typing::Justification => return self.send_justification(&text),
                }
                self.go(self.typed_from());
                Vec::new()
            }
            Target::Tracking => {
                self.go(Page::Tracking);
                Vec::new()
            }
            Target::TrackingSwitch | Target::Smoothing(_) => match self.tracking.as_mut() {
                Some(tracking) => {
                    match target {
                        Target::Smoothing(i) if ((i / 4) as usize) < TRACKING_KINDS.len() => {
                            tracking.levels[(i / 4) as usize] = i % 4;
                            // Choosing a strength turns smoothing on (Off for every kind leaves it on, doing nothing).
                            tracking.on = true;
                        }
                        Target::TrackingSwitch => tracking.on = !tracking.on,
                        _ => return Vec::new(),
                    }
                    vec![Effect::Tracking { on: tracking.on, levels: tracking.levels }]
                }
                None => Vec::new(),
            },
            Target::Suggest => {
                self.go(Page::Suggest);
                Vec::new()
            }
            Target::SuggestType => {
                self.type_for(Typing::Suggestion, self.suggestion.clone());
                Vec::new()
            }
            Target::SuggestTalk if self.dictating => {
                self.dictating = false;
                Vec::new()
            }
            Target::SuggestTalk => {
                self.dictating = true;
                self.notice = Some("Listening: say your idea".into());
                vec![Effect::Dictate]
            }
            Target::SuggestSend if !self.sending && !self.suggestion.trim().is_empty() => {
                self.sending = true;
                self.notice = Some("Sending…".into());
                vec![Effect::SendSuggestion { text: self.suggestion.trim().to_string(), voice: self.suggestion_voice }]
            }
            Target::SuggestSend => Vec::new(),
            Target::WatchVideo => match &self.world_video {
                Some(video) => {
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                    self.notice = Some("Opened on the computer".into());
                    vec![Effect::WatchVideo { link: watch_link(video, now) }]
                }
                None => Vec::new(),
            },
            Target::SaveVideo if self.world_video.is_some() => {
                self.notice = Some("Saved in N4N0's videos".into());
                vec![Effect::SaveVideo]
            }
            Target::SaveVideo => Vec::new(),
            // The YouTube video the computer plays: copied, then dropped, as a result is.
            Target::CopyPlaying => match self.media.as_ref().and_then(|m| Some((m.link()?, m.title.clone(), m.video.clone()?))) {
                Some((link, title, video)) => {
                    self.copied = Some(video);
                    let notice = if self.can_drop { "Link copied: paste it, or Drop it" } else { "Link copied: paste it in the video player" };
                    self.notice = Some(notice.into());
                    vec![Effect::Copy { link, title }]
                }
                None => Vec::new(),
            },
            Target::DropPlaying => match self.media.as_ref().and_then(|m| Some((m.link()?, m.title.clone(), m.video.clone()?))) {
                Some((link, title, video)) if self.can_drop && self.copied.as_deref() == Some(video.as_str()) => {
                    self.notice = Some("Dropped: grab the card to copy its link".into());
                    vec![Effect::Drop { link, title, video: Some(video) }]
                }
                _ => Vec::new(),
            },
            Target::Previous => self.turn(-1),
            Target::Next => self.turn(1),
            Target::Timeline(i) | Target::Seek(i) => self.seek(i),
            Target::TakeBack if self.card_out => {
                self.card_out = false;
                self.notice = Some("Card taken back".into());
                vec![Effect::TakeBack]
            }
            Target::TakeBack => Vec::new(),
            Target::More => {
                self.go(Page::More);
                Vec::new()
            }
            Target::Instances => {
                self.go(Page::Instances);
                self.instance_page = 0;
                vec![Effect::Instances]
            }
            Target::InviteMe(i) => match self.page_instances().get(i as usize) {
                Some(row) if !self.invite_waiting => {
                    let location = row.location.clone();
                    self.notice = Some(format!("Asking Orbiters to invite you to {}…", if row.world.is_empty() { "this instance" } else { &row.world }));
                    self.ask_invite(location)
                }
                _ => Vec::new(),
            },
            Target::InviteAgain => match self.inviting.clone() {
                Some(location) if !self.invite_waiting => {
                    self.notice = Some("Asking Orbiters again…".into());
                    self.ask_invite(location)
                }
                _ => Vec::new(),
            },
            Target::CopyFriendName => match &self.invite_account {
                Some(account) => {
                    let text = account.name.clone();
                    self.notice = Some(format!("Copied \"{text}\": search it in VRChat's Social menu"));
                    vec![Effect::CopyText { text }]
                }
                None => Vec::new(),
            },
            Target::Qr => {
                self.go(Page::Qr);
                vec![Effect::QrScan(true)]
            }
            Target::QrAgain if !self.qr.scanning() => vec![Effect::QrScan(true)],
            Target::QrAgain => Vec::new(),
            Target::QrCopy => match &self.qr.link {
                Some(link) => {
                    let text = link.clone();
                    self.notice = Some("Link copied".into());
                    vec![Effect::CopyText { text }]
                }
                None => Vec::new(),
            },
            Target::QrOpen => match &self.qr.link {
                Some(link) => {
                    let link = link.clone();
                    self.notice = Some("Opened again on the computer".into());
                    vec![Effect::OpenLink { link }]
                }
                None => Vec::new(),
            },
            Target::OpenFriendLink => match &self.invite_account {
                Some(account) => {
                    let link = account.link.clone();
                    self.notice = Some("Opened on the computer: send the friend request there".into());
                    vec![Effect::OpenLink { link }]
                }
                None => Vec::new(),
            },
            // Sharing 3D captures (from screen version 12).
            Target::History if self.pages.share() => {
                self.go(Page::History);
                self.capture_page = 0;
                let mut effects = vec![Effect::Captures];
                effects.extend(self.capture_thumbnails());
                effects
            }
            Target::ShareLatest => match self.captures.latest.as_ref().map(|c| c.key.clone()) {
                Some(key) if self.pages.share() => self.share_public(key),
                _ => Vec::new(),
            },
            Target::SharePublic(i) => match self.page_captures().get(i as usize).map(|c| c.key.clone()) {
                Some(key) => self.share_public(key),
                None => Vec::new(),
            },
            Target::ShareLatestTo => match self.captures.latest.as_ref().map(|c| c.key.clone()) {
                Some(key) if self.pages.share() => self.share_to(key),
                _ => Vec::new(),
            },
            Target::ShareTo(i) => match self.page_captures().get(i as usize).map(|c| c.key.clone()) {
                Some(key) => self.share_to(key),
                None => Vec::new(),
            },
            Target::Friend(i) | Target::FriendMark(i) => {
                let Some(friend) = self.page_friends().get(i as usize).map(|f| (*f).clone()) else { return Vec::new() };
                if let Some(at) = self.chosen.iter().position(|f| f.id == friend.id) {
                    self.chosen.remove(at);
                    self.notice = None;
                } else if self.chosen.len() >= crate::captures::RECIPIENTS_MAX {
                    self.notice = Some(format!("At most {} friends at a time", crate::captures::RECIPIENTS_MAX));
                } else {
                    self.chosen.push(friend);
                    self.notice = None;
                }
                Vec::new()
            }
            Target::FindFriend if self.page == Page::ShareTo => {
                self.type_for(Typing::Friend, self.find.clone());
                Vec::new()
            }
            Target::SendShare if self.page == Page::ShareTo && !self.chosen.is_empty() => {
                let Some(key) = self.sharing.clone() else { return Vec::new() };
                let recipients = std::mem::take(&mut self.chosen);
                let n = recipients.len();
                self.asked = Some((key.clone(), self.sharing_of(&key)));
                self.go(self.share_from);
                self.notice = Some(if n == 1 { "Sharing with 1 friend…".into() } else { format!("Sharing with {n} friends…") });
                vec![Effect::ShareCapture(ShareAsk { key, public: false, recipients, acknowledge_explicit: false })]
            }
            Target::ShareAnyway if self.page == Page::Explicit => {
                let Some(key) = self.sharing.clone() else { return Vec::new() };
                self.asked = Some((key.clone(), self.sharing_of(&key)));
                self.go(self.share_from);
                self.notice = Some("Sharing publicly…".into());
                vec![Effect::ShareCapture(ShareAsk { key, public: true, recipients: Vec::new(), acknowledge_explicit: true })]
            }
            Target::CancelShare if self.page == Page::Explicit => {
                self.go(self.share_from);
                Vec::new()
            }
            Target::History | Target::FindFriend | Target::SendShare | Target::ShareAnyway | Target::CancelShare => Vec::new(),
            // World feedback (from screen version 13).
            Target::WorldFeedback if self.pages.feedback() => {
                self.subject = 0;
                self.go(Page::Feedback);
                self.show_feedback()
            }
            Target::Like => match self.shown_world() {
                Some(world) if self.feedback.signed_in => {
                    let entry = self.feedback.world_mut(&world);
                    let on = !entry.liked;
                    entry.set_like(on);
                    self.notice = None;
                    vec![Effect::Feedback(FeedbackAsk::Like { world, on })]
                }
                Some(_) => self.sign_in_first(),
                None => Vec::new(),
            },
            Target::TopLabel(i) => {
                let id = self.shown_feedback().and_then(|f| f.top().get(i as usize).map(|l| l.id.clone()));
                match id {
                    Some(id) => self.toggle_label(&id),
                    None => Vec::new(),
                }
            }
            Target::LabelChip(i) => match crate::feedback::label_at(i as usize) {
                Some(label) => self.toggle_label(&label.id.clone()),
                None => Vec::new(),
            },
            Target::Labels if self.shown_world().is_some() => {
                self.go(Page::FeedbackLabels);
                Vec::new()
            }
            Target::Comments if self.shown_world().is_some() => {
                self.comment_page = 0;
                self.go(Page::Comments);
                Vec::new()
            }
            Target::WriteComment => match self.shown_world() {
                Some(_) if self.feedback.signed_in => {
                    self.type_for(Typing::Comment, String::new());
                    Vec::new()
                }
                Some(_) => self.sign_in_first(),
                None => Vec::new(),
            },
            Target::DeleteComment(i) => {
                let Some(world) = self.shown_world() else { return Vec::new() };
                let Some(id) = self.page_comments().get(i as usize).filter(|c| c.mine && c.id != 0).map(|c| c.id) else { return Vec::new() };
                let entry = self.feedback.world_mut(&world);
                entry.comments.retain(|c| c.id != id);
                entry.comment_count = entry.comment_count.saturating_sub(1);
                self.notice = Some("Comment deleted".into());
                vec![Effect::Feedback(FeedbackAsk::Uncomment { world, comment: id })]
            }
            Target::WorldFeedback | Target::Labels | Target::Comments | Target::CommentCard(_) => Vec::new(),
            Target::Contact(i) => match self.page_contacts().get(i as usize) {
                Some(contact) if self.call.is_none() => vec![Effect::CallContact { id: contact.id.clone() }],
                _ => Vec::new(),
            },
            Target::Answer => match self.call {
                Some(Call::Ringing { .. }) => vec![Effect::Answer],
                _ => Vec::new(),
            },
            Target::HangUp => match self.call {
                Some(_) => vec![Effect::HangUp],
                None => Vec::new(),
            },
            Target::Timer(i) => match TIMERS.get(i as usize) {
                Some(&timer) => {
                    self.photo_timer = timer;
                    vec![Effect::PhotoChoice { timer, length: self.photo_length }]
                }
                None => Vec::new(),
            },
            Target::Length(i) => match PhotoLength::ALL.get(i as usize) {
                Some(&length) => {
                    self.photo_length = length;
                    vec![Effect::PhotoChoice { timer: self.photo_timer, length }]
                }
                None => Vec::new(),
            },
            Target::Take => {
                if self.countdown.is_some() {
                    return Vec::new();
                }
                vec![Effect::Photo3d { timer: self.photo_timer, length: self.photo_length }]
            }
            // What the music page shows changes at once; the computer follows.
            Target::MediaPlayPause => match self.media.as_mut() {
                Some(media) if media.can_play_pause => {
                    media.playing = !media.playing;
                    vec![Effect::Media(MediaCommand::PlayPause)]
                }
                _ => Vec::new(),
            },
            Target::MediaPrevious => match self.media.as_mut() {
                Some(media) if media.can_previous => vec![Effect::Media(MediaCommand::Previous)],
                // An app with nothing before (a single video): back to its start, where it lets others move in it.
                Some(media) if media.can_seek && media.duration.is_some() => {
                    media.position = Some(0);
                    vec![Effect::Media(MediaCommand::Seek(0))]
                }
                _ => Vec::new(),
            },
            Target::MediaNext => match &self.media {
                Some(media) if media.can_next => vec![Effect::Media(MediaCommand::Next)],
                _ => Vec::new(),
            },
            Target::VolumeDown | Target::VolumeUp => match self.volume.as_mut() {
                Some(volume) => {
                    let percent = if target == Target::VolumeUp {
                        volume.percent.saturating_add(VOLUME_STEP).min(100)
                    } else {
                        volume.percent.saturating_sub(VOLUME_STEP)
                    };
                    *volume = Volume { percent, muted: false };
                    vec![Effect::Media(MediaCommand::Volume(percent))]
                }
                None => Vec::new(),
            },
            Target::Mute => match self.volume.as_mut() {
                Some(volume) => {
                    volume.muted = !volume.muted;
                    vec![Effect::Media(MediaCommand::Mute(volume.muted))]
                }
                None => Vec::new(),
            },
            // Choosing a colour or a brightness turns the light on.
            Target::LightSwitch | Target::LightColor(_) | Target::LightLevel(_) => match self.light.as_mut() {
                Some(light) => {
                    match target {
                        Target::LightColor(i) if (i as usize) < orbiters_contracts::orb::LIGHT_COLORS.len() => {
                            light.color = i;
                            light.on = true;
                        }
                        Target::LightLevel(i) => match orbiters_contracts::orb::LIGHT_LEVELS.get(i as usize) {
                            Some(&(_, level)) => {
                                light.level = level;
                                light.on = true;
                            }
                            None => return Vec::new(),
                        },
                        Target::LightSwitch => light.on = !light.on,
                        _ => return Vec::new(),
                    }
                    vec![Effect::Light(*light)]
                }
                None => Vec::new(),
            },
        }
    }

    /// The music to the start of timeline segment `i`, shown there at once; an app that lets no one move in its timeline
    /// is said so.
    fn seek(&mut self, i: u8) -> Vec<Effect> {
        let Some(media) = self.media.as_mut() else { return Vec::new() };
        let Some(duration) = media.duration.filter(|d| *d > 0) else { return Vec::new() };
        if !media.can_seek {
            let app = if media.app.is_empty() { "This player" } else { media.app.as_str() };
            self.notice = Some(format!("{app} lets no app move in its timeline"));
            return Vec::new();
        }
        let at = (duration as u64 * (i as u64).min(TIMELINE_STEPS as u64 - 1) / TIMELINE_STEPS as u64) as u32;
        media.position = Some(at);
        vec![Effect::Media(MediaCommand::Seek(at))]
    }

    /// A press held and moved along the music's timeline (a drag) from segment `from` onto `target`: the music moves to
    /// that segment when it is another. Returns the segment the drag is on now, and what follows.
    pub fn drag(&mut self, from: u8, target: Target) -> (u8, Vec<Effect>) {
        match timeline_segment(target) {
            Some(to) if to != from && self.open && self.page == Page::Music => (to, self.seek(to)),
            _ => (from, Vec::new()),
        }
    }

    /// Text typed into the search field, or the keyboard page's text (a key, or several characters at once).
    pub fn type_text(&mut self, text: &str) -> Vec<Effect> {
        if self.page == Page::Type {
            let max = self.typing.max();
            for c in text.chars() {
                if self.typed.chars().count() >= max {
                    break;
                }
                if c == ' ' && (self.typed.is_empty() || self.typed.ends_with(' ')) {
                    continue;
                }
                if !c.is_control() {
                    self.typed.push(c);
                }
            }
            return Vec::new();
        }
        if self.page != Page::Search {
            return Vec::new();
        }
        for c in text.chars() {
            if self.query.chars().count() >= QUERY_MAX {
                break;
            }
            // No leading space and no double space: the field shows what will be searched.
            if c == ' ' && (self.query.is_empty() || self.query.ends_with(' ')) {
                continue;
            }
            if !c.is_control() {
                self.query.push(c);
            }
        }
        self.suggest()
    }

    /// What a text field shows becomes the search field's text (a display with a keyboard of its own: the window's
    /// Home), with the same limit and the same suggestions as typing.
    pub fn set_query(&mut self, text: &str) -> Vec<Effect> {
        let text = cut(text.trim_start(), QUERY_MAX);
        if text == self.query {
            return Vec::new();
        }
        self.query = text;
        self.suggest()
    }

    pub fn backspace(&mut self) -> Vec<Effect> {
        if self.page == Page::Type {
            self.typed.pop();
            return Vec::new();
        }
        if self.page != Page::Search {
            return Vec::new();
        }
        self.query.pop();
        self.suggest()
    }

    fn suggest(&mut self) -> Vec<Effect> {
        let text = self.query.trim_start().to_string();
        if text.trim().is_empty() {
            self.suggestions.clear();
            self.suggested_for.clear();
            self.asked_for.clear();
            return Vec::new();
        }
        if text == self.asked_for {
            return Vec::new();
        }
        self.asked_for = text.clone();
        self.suggest_id += 1;
        vec![Effect::Suggest { id: self.suggest_id, text }]
    }

    /// The suggestions to show: those for what is typed, or for an earlier text while the new ones are on their way
    /// (only while they still start like it, so they never contradict the field). Read in place: the screen's steps ask
    /// for them many times a second.
    pub fn shown_suggestions(&self) -> impl Iterator<Item = &str> + '_ {
        let typed = self.query.trim();
        let fresh = same_ignoring_case(self.suggested_for.trim(), typed);
        self.suggestions
            .iter()
            .map(String::as_str)
            .filter(move |s| !typed.is_empty() && (fresh || starts_ignoring_case(s, typed)))
            .filter(move |s| !fresh || !same_ignoring_case(s.trim(), typed))
            .take(SUGGESTIONS)
    }

    /// Suggestions arrived for request `id`, asked for `text`.
    pub fn suggestions_arrived(&mut self, id: u64, text: &str, suggestions: Vec<String>) {
        // An older request's answer never replaces a newer one's, and an answer to a text the field no longer starts
        // with is stale.
        if id <= self.suggested_id || !starts_ignoring_case(self.query.trim_start(), text.trim()) {
            return;
        }
        self.suggested_id = id;
        self.suggested_for = text.to_string();
        self.suggestions = suggestions;
    }

    fn search(&mut self) -> Vec<Effect> {
        let query = self.query.trim().to_string();
        if query.is_empty() {
            return Vec::new();
        }
        self.searching = true;
        self.loading_more = false;
        self.turning = false;
        self.results.clear();
        self.copied = None;
        self.continuation = None;
        self.results_for = query.clone();
        self.result_page = 0;
        self.search_id += 1;
        self.go(Page::Results);
        vec![Effect::Search { id: self.search_id, query }]
    }

    /// Search for `query` straight away (said aloud, or asked of N4N0).
    pub fn search_for(&mut self, query: &str) -> Vec<Effect> {
        self.open = true;
        self.query = cut(query.trim(), QUERY_MAX);
        self.search()
    }

    /// Results arrived for search `id`: its first page, or the next one (`more`).
    pub fn results_arrived(&mut self, id: u64, results: Results, more: bool) -> Vec<Effect> {
        // Another search's (an older one for the same words included) is dropped.
        if id != self.search_id {
            return Vec::new();
        }
        self.searching = false;
        self.loading_more = false;
        if !more {
            self.results.clear();
            self.result_page = 0;
        }
        for video in results.videos {
            if !self.results.iter().any(|v| v.id == video.id) {
                self.results.push(video);
            }
        }
        self.continuation = results.continuation;
        // The page asked for turns now that it has results; when none came, it stays where it was.
        if std::mem::take(&mut self.turning) && (self.result_page + 1) * RESULTS_PER_PAGE < self.results.len() {
            self.result_page += 1;
        }
        if self.results.is_empty() {
            self.notice = Some(format!("Nothing found for \"{}\"", self.results_for));
        }
        self.thumbnails()
    }

    /// Search `id` or its next page failed: the page stays where it was (Next tries again).
    pub fn results_failed(&mut self, id: u64, error: &str) {
        if id != self.search_id {
            return;
        }
        self.searching = false;
        self.loading_more = false;
        self.turning = false;
        self.notice = Some(format!("YouTube did not answer: {}", cut(error, 60)));
    }

    /// The results of the page shown.
    pub fn page_results(&self) -> &[Video] {
        let start = (self.result_page * RESULTS_PER_PAGE).min(self.results.len());
        &self.results[start..(start + RESULTS_PER_PAGE).min(self.results.len())]
    }

    /// Pages of results, counting one more while YouTube has more.
    pub fn result_pages(&self) -> usize {
        self.results.len().div_ceil(RESULTS_PER_PAGE).max(1)
    }

    /// YouTube has results past those fetched.
    pub fn more_available(&self) -> bool {
        self.continuation.is_some()
    }

    pub fn has_previous(&self) -> bool {
        match self.page {
            Page::Results => self.result_page > 0,
            Page::Call => self.contact_page > 0,
            Page::States => self.state_page > 0,
            Page::Instances => self.instance_page > 0,
            Page::History => self.capture_page > 0,
            Page::ShareTo => self.friend_page > 0,
            Page::Feedback => self.subject > 0,
            Page::Comments => self.comment_page > 0,
            _ => false,
        }
    }

    pub fn has_next(&self) -> bool {
        match self.page {
            Page::Results => (self.result_page + 1) * RESULTS_PER_PAGE < self.results.len() || self.continuation.is_some(),
            Page::Call => (self.contact_page + 1) * CONTACTS_PER_PAGE < self.contacts.len(),
            Page::States => (self.state_page + 1) * STATES_PER_PAGE < self.states.len(),
            Page::Instances => (self.instance_page + 1) * INSTANCES_PER_PAGE < self.instances.len(),
            Page::History => (self.capture_page + 1) * CAPTURES_PER_PAGE < self.captures.history.len(),
            Page::ShareTo => (self.friend_page + 1) * FRIENDS_PER_PAGE < self.found_friends().len(),
            Page::Feedback => self.subject + 1 < self.feedback.subjects.len(),
            Page::Comments => (self.comment_page + 1) * COMMENTS_PER_PAGE < self.shown_feedback().map_or(0, |f| f.comments.len()),
            _ => false,
        }
    }

    fn turn(&mut self, by: i32) -> Vec<Effect> {
        match self.page {
            Page::Results => {
                if by < 0 && self.result_page > 0 {
                    self.result_page -= 1;
                    self.notice = None;
                    return self.thumbnails();
                }
                if by > 0 && self.has_next() {
                    self.notice = None;
                    if (self.result_page + 1) * RESULTS_PER_PAGE < self.results.len() {
                        self.result_page += 1;
                        let mut effects = self.thumbnails();
                        // Keep a page ahead: fetch more when the next page would not be full.
                        if (self.result_page + 2) * RESULTS_PER_PAGE > self.results.len() {
                            effects.extend(self.fetch_more());
                        }
                        return effects;
                    }
                    // The next page is not fetched yet. A full page turns once its results arrive; one still filling
                    // up waits for them, so no result is skipped.
                    let more = self.fetch_more();
                    if self.loading_more && self.page_results().len() == RESULTS_PER_PAGE {
                        self.turning = true;
                    }
                    return more;
                }
                Vec::new()
            }
            Page::Call => {
                let pages = self.contacts.len().div_ceil(CONTACTS_PER_PAGE).max(1);
                let next = (self.contact_page as i32 + by).clamp(0, pages as i32 - 1) as usize;
                self.contact_page = next;
                Vec::new()
            }
            Page::States => {
                let pages = self.states.len().div_ceil(STATES_PER_PAGE).max(1);
                self.state_page = (self.state_page as i32 + by).clamp(0, pages as i32 - 1) as usize;
                self.notice = None;
                Vec::new()
            }
            Page::Instances => {
                let pages = self.instances.len().div_ceil(INSTANCES_PER_PAGE).max(1);
                self.instance_page = (self.instance_page as i32 + by).clamp(0, pages as i32 - 1) as usize;
                self.notice = None;
                Vec::new()
            }
            Page::History => {
                let pages = self.captures.history.len().div_ceil(CAPTURES_PER_PAGE).max(1);
                self.capture_page = (self.capture_page as i32 + by).clamp(0, pages as i32 - 1) as usize;
                self.notice = None;
                self.capture_thumbnails()
            }
            Page::ShareTo => {
                let pages = self.found_friends().len().div_ceil(FRIENDS_PER_PAGE).max(1);
                self.friend_page = (self.friend_page as i32 + by).clamp(0, pages as i32 - 1) as usize;
                Vec::new()
            }
            Page::Feedback => {
                let last = self.feedback.subjects.len().max(1) - 1;
                let next = (self.subject as i32 + by).clamp(0, last as i32) as usize;
                if next == self.subject {
                    return Vec::new();
                }
                self.subject = next;
                self.notice = None;
                self.show_feedback()
            }
            Page::Comments => {
                let pages = self.shown_feedback().map_or(0, |f| f.comments.len()).div_ceil(COMMENTS_PER_PAGE).max(1);
                self.comment_page = (self.comment_page as i32 + by).clamp(0, pages as i32 - 1) as usize;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn fetch_more(&mut self) -> Vec<Effect> {
        if self.loading_more {
            return Vec::new();
        }
        match self.continuation.clone() {
            Some(token) => {
                self.loading_more = true;
                vec![Effect::More { id: self.search_id, query: self.results_for.clone(), token }]
            }
            None => Vec::new(),
        }
    }

    /// Thumbnails of the results page shown, then of the next page (fetched ahead, so they show as it turns).
    pub fn thumbnails(&self) -> Vec<Effect> {
        let start = (self.result_page * RESULTS_PER_PAGE).min(self.results.len());
        let ids: Vec<String> = self.results[start..].iter().take(2 * RESULTS_PER_PAGE).map(|v| v.id.clone()).collect();
        if ids.is_empty() {
            Vec::new()
        } else {
            vec![Effect::Thumbnails(ids)]
        }
    }

    /// Whether a search, or the page asked for, is being fetched (shown as loading).
    pub fn loading(&self) -> bool {
        self.searching || self.turning || (self.loading_more && self.page_results().is_empty())
    }

    /// Calls the friend at `index` of the whole list (a display that shows them all): as pressing them on their page.
    pub fn call_contact(&mut self, index: usize) -> Vec<Effect> {
        if index >= self.contacts.len() {
            return Vec::new();
        }
        self.contact_page = index / CONTACTS_PER_PAGE;
        self.press(Target::Contact((index % CONTACTS_PER_PAGE) as u8))
    }

    pub fn page_contacts(&self) -> &[Contact] {
        let start = (self.contact_page * CONTACTS_PER_PAGE).min(self.contacts.len());
        &self.contacts[start..(start + CONTACTS_PER_PAGE).min(self.contacts.len())]
    }

    /// The friends who can be called changed.
    pub fn set_contacts(&mut self, contacts: Vec<Contact>, ready: bool, note: &str) {
        self.contacts = contacts;
        self.calls_ready = ready;
        self.call_note = note.to_string();
        let pages = self.contacts.len().div_ceil(CONTACTS_PER_PAGE).max(1);
        self.contact_page = self.contact_page.min(pages - 1);
    }

    /// The saved states of the page shown.
    pub fn page_states(&self) -> &[StateRow] {
        let start = (self.state_page * STATES_PER_PAGE).min(self.states.len());
        &self.states[start..(start + STATES_PER_PAGE).min(self.states.len())]
    }

    /// The avatar worn's saved states changed, as the host keeps them (`ready`: the avatar is known; else `note` says
    /// why there are none). A state saved here stays in the list until the host's has it, or a few seconds.
    pub fn set_states(&mut self, mut states: Vec<StateRow>, ready: bool, note: &str) {
        if let Some((name, at)) = &self.pending_save {
            if !ready || states.iter().any(|s| s.name == *name) || at.elapsed() >= SAVE_SHOWN {
                self.pending_save = None;
            } else {
                states.push(StateRow { name: name.clone(), ..Default::default() });
            }
        }
        self.states = states;
        self.states_ready = ready;
        self.states_note = note.to_string();
        let pages = self.states.len().div_ceil(STATES_PER_PAGE).max(1);
        self.state_page = self.state_page.min(pages - 1);
    }

    /// Whether a state saved here waits for the host's list.
    pub fn saving(&self) -> bool {
        self.pending_save.is_some()
    }

    /// The previous instances of the page shown.
    pub fn page_instances(&self) -> &[InstanceRow] {
        let start = (self.instance_page * INSTANCES_PER_PAGE).min(self.instances.len());
        &self.instances[start..(start + INSTANCES_PER_PAGE).min(self.instances.len())]
    }

    /// History's captures on the page shown.
    pub fn page_captures(&self) -> &[CaptureRow] {
        let history = &self.captures.history;
        let start = (self.capture_page * CAPTURES_PER_PAGE).min(history.len());
        &history[start..(start + CAPTURES_PER_PAGE).min(history.len())]
    }

    /// The capture Share to … or the explicit warning is for.
    pub fn shared_capture(&self) -> Option<&CaptureRow> {
        self.sharing.as_deref().and_then(|key| self.captures.find(key))
    }

    /// The friends Share to … lists: those whose name has [`Menu::find`] (case aside), every one without it.
    pub fn found_friends(&self) -> Vec<&Friend> {
        let find = self.find.trim().to_lowercase();
        self.friends.list().iter().filter(|f| find.is_empty() || f.name.to_lowercase().contains(&find)).collect()
    }

    /// Share to …'s friends on the page shown.
    pub fn page_friends(&self) -> Vec<&Friend> {
        self.found_friends().into_iter().skip(self.friend_page * FRIENDS_PER_PAGE).take(FRIENDS_PER_PAGE).collect()
    }

    /// Whether `friend` is chosen on Share to ….
    pub fn is_chosen(&self, friend: &Friend) -> bool {
        self.chosen.iter().any(|f| f.id == friend.id)
    }

    /// The pictures of the captures the page shows (the last capture, History's page, the warning's capture), which
    /// the host fetches when it has not got them.
    pub fn capture_thumbnails(&self) -> Vec<Effect> {
        if !self.pages.share() {
            return Vec::new();
        }
        let keys: Vec<String> = match self.page {
            Page::Photo => self.captures.latest.iter().map(CaptureRow::picture_key).collect(),
            Page::History => self.page_captures().iter().map(CaptureRow::picture_key).collect(),
            Page::Explicit => self.shared_capture().map(CaptureRow::picture_key).into_iter().collect(),
            _ => Vec::new(),
        };
        if keys.is_empty() {
            Vec::new()
        } else {
            vec![Effect::Thumbnails(keys)]
        }
    }

    /// The 3D captures as the host lists them now. A share asked here that the server wants acknowledged (explicit)
    /// opens the warning, from the page it was asked on. Returns the pictures to fetch.
    pub fn set_captures(&mut self, captures: CapturesView) -> Vec<Effect> {
        if self.captures == captures {
            return Vec::new();
        }
        self.captures = captures;
        let pages = self.captures.history.len().div_ceil(CAPTURES_PER_PAGE).max(1);
        self.capture_page = self.capture_page.min(pages - 1);
        if let Some((key, was)) = self.asked.clone() {
            use crate::captures::Sharing;
            match self.captures.find(&key).map(|c| c.sharing.clone()) {
                // The host has not taken it yet.
                Some(now) if now == was => {}
                Some(Sharing::Explicit) => {
                    self.asked = None;
                    if self.open && matches!(self.page, Page::Photo | Page::History) {
                        self.sharing = Some(key);
                        self.share_from = self.page;
                        self.go(Page::Explicit);
                    }
                }
                Some(now @ (Sharing::Waiting { .. } | Sharing::Busy { .. })) => self.asked = Some((key, now)),
                // Why it failed, where the page says what was just done; done, its row says so.
                Some(Sharing::Failed { message }) => {
                    self.asked = None;
                    self.notice = Some(format!("Not shared: {message}"));
                }
                Some(_) | None => {
                    self.asked = None;
                    self.notice = None;
                }
            }
        }
        self.capture_thumbnails()
    }

    /// The VRChat friends as the host read them (`Friends::None`: forgotten).
    pub fn set_friends(&mut self, friends: Friends) {
        if self.friends == friends {
            return;
        }
        self.friends = friends;
        let pages = self.found_friends().len().div_ceil(FRIENDS_PER_PAGE).max(1);
        self.friend_page = self.friend_page.min(pages - 1);
    }

    /// Share publicly: at once, or after the warning for a capture flagged explicit.
    fn share_public(&mut self, key: String) -> Vec<Effect> {
        let Some(capture) = self.captures.find(&key) else { return Vec::new() };
        if !capture.shareable() {
            return Vec::new();
        }
        if capture.explicit {
            self.sharing = Some(key);
            self.share_from = self.page;
            self.go(Page::Explicit);
            return self.capture_thumbnails();
        }
        self.asked = Some((key.clone(), self.sharing_of(&key)));
        self.notice = Some("Sharing publicly…".into());
        vec![Effect::ShareCapture(ShareAsk { key, public: true, recipients: Vec::new(), acknowledge_explicit: false })]
    }

    /// Where capture `key`'s shares stand now.
    fn sharing_of(&self, key: &str) -> crate::captures::Sharing {
        self.captures.find(key).map(|c| c.sharing.clone()).unwrap_or_default()
    }

    /// Share to …: the friends to choose for capture `key`, read again when they are not.
    fn share_to(&mut self, key: String) -> Vec<Effect> {
        if !self.captures.find(&key).is_some_and(CaptureRow::shareable) {
            return Vec::new();
        }
        self.sharing = Some(key);
        self.share_from = self.page;
        self.chosen.clear();
        self.find.clear();
        self.friend_page = 0;
        self.go(Page::ShareTo);
        if !matches!(self.friends, Friends::Ready { .. }) {
            self.friends = Friends::Loading;
        }
        vec![Effect::Friends(true)]
    }

    /// The instances the player was in, the newest first, as the host read them (`ready`: read; else `note` says why
    /// there are none).
    /// The world World feedback shows, when there is one.
    pub fn shown_subject(&self) -> Option<&Subject> {
        self.feedback.subjects.get(self.subject).or_else(|| self.feedback.subjects.last())
    }

    pub fn shown_world(&self) -> Option<String> {
        self.shown_subject().map(|s| s.world.clone())
    }

    /// The feedback of the world shown, once the host has any.
    pub fn shown_feedback(&self) -> Option<&WorldFeedback> {
        self.shown_subject().and_then(|s| self.feedback.world(&s.world))
    }

    /// The comments on the page of Comments shown.
    pub fn page_comments(&self) -> &[crate::feedback::CommentRow] {
        let Some(f) = self.shown_feedback() else { return &[] };
        let start = (self.comment_page * COMMENTS_PER_PAGE).min(f.comments.len());
        &f.comments[start..(start + COMMENTS_PER_PAGE).min(f.comments.len())]
    }

    /// Asks the host for the shown world's feedback.
    fn show_feedback(&mut self) -> Vec<Effect> {
        match self.shown_world() {
            Some(world) => vec![Effect::Feedback(FeedbackAsk::Show { world })],
            None => Vec::new(),
        }
    }

    fn sign_in_first(&mut self) -> Vec<Effect> {
        self.notice = Some("Sign in to Orbiters - N4N0 to give feedback".into());
        Vec::new()
    }

    /// The player gives the label (or takes it back); one that needs a justification asks for it on the keyboard first.
    fn toggle_label(&mut self, id: &str) -> Vec<Effect> {
        let Some(world) = self.shown_world() else { return Vec::new() };
        if !self.feedback.signed_in {
            return self.sign_in_first();
        }
        let Some(label) = orbiters_contracts::world_feedback::label(id) else { return Vec::new() };
        let entry = self.feedback.world_mut(&world);
        let on = !entry.has(id);
        if on && label.justify.is_some() {
            let kept = entry.justification.clone().unwrap_or_default();
            self.justifying = Some(id.to_string());
            self.type_for(Typing::Justification, kept);
            return Vec::new();
        }
        entry.set_label(id, on);
        self.notice = None;
        vec![Effect::Feedback(FeedbackAsk::Label { world, label: id.to_string(), on, text: None })]
    }

    /// Done on the keyboard of a justification: given with it once it has enough words, else the keyboard stays.
    fn send_justification(&mut self, text: &str) -> Vec<Effect> {
        use orbiters_contracts::world_feedback as contract;
        let (Some(world), Some(id)) = (self.shown_world(), self.justifying.clone()) else {
            self.go(self.typed_from());
            return Vec::new();
        };
        let Some(label) = contract::label(&id) else { return Vec::new() };
        match contract::justification(label, text) {
            Ok(text) => {
                let entry = self.feedback.world_mut(&world);
                entry.set_label(&id, true);
                entry.justification = Some(text.clone());
                self.justifying = None;
                self.go(Page::FeedbackLabels);
                self.notice = Some(format!("{} given, with your reason", label.name("en")));
                vec![Effect::Feedback(FeedbackAsk::Label { world, label: id, on: true, text: Some(text) })]
            }
            Err(refusal) => {
                self.notice = Some(refusal.text().trim_end_matches('.').to_string());
                Vec::new()
            }
        }
    }

    /// Done on the keyboard of a comment: it shows first in Comments at once, on its way.
    fn send_comment(&mut self, text: &str) -> Vec<Effect> {
        let Some(world) = self.shown_world() else {
            self.go(self.typed_from());
            return Vec::new();
        };
        match orbiters_contracts::world_feedback::comment(text) {
            Ok(text) => {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                let entry = self.feedback.world_mut(&world);
                entry.comments.insert(0, crate::feedback::CommentRow { id: 0, by: "You".into(), text: text.clone(), at: now, mine: true });
                entry.comment_count += 1;
                self.comment_page = 0;
                self.typed.clear();
                self.go(Page::Comments);
                self.notice = Some("Sending your comment…".into());
                vec![Effect::Feedback(FeedbackAsk::Comment { world, text })]
            }
            Err(refusal) => {
                self.notice = Some(refusal.text().trim_end_matches('.').to_string());
                Vec::new()
            }
        }
    }

    /// The host's world feedback: the worlds and what it read and sent. The world shown stays the same world when it
    /// is still among them.
    pub fn set_feedback(&mut self, view: FeedbackView) {
        let shown = self.shown_world();
        self.feedback = view;
        self.subject = shown.and_then(|w| self.feedback.subjects.iter().position(|s| s.world == w)).unwrap_or(0);
        let pages = self.shown_feedback().map_or(0, |f| f.comments.len()).div_ceil(COMMENTS_PER_PAGE).max(1);
        self.comment_page = self.comment_page.min(pages - 1);
    }

    pub fn set_instances(&mut self, instances: Vec<InstanceRow>, ready: bool, note: &str) {
        self.instances = instances;
        self.instances_ready = ready;
        self.instances_note = note.to_string();
        let pages = self.instances.len().div_ceil(INSTANCES_PER_PAGE).max(1);
        self.instance_page = self.instance_page.min(pages - 1);
    }

    fn ask_invite(&mut self, location: String) -> Vec<Effect> {
        self.inviting = Some(location.clone());
        self.invite_waiting = true;
        vec![Effect::InviteMe { location }]
    }

    /// Orbiters answered the last Invite me: sent, the page says so; its account to befriend first, Add Orbiters as a
    /// friend shows; failed, why.
    pub fn invite_answered(&mut self, answer: InviteAnswer) {
        self.invite_waiting = false;
        match answer {
            InviteAnswer::Sent => {
                if self.page == Page::AddFriend {
                    self.go(Page::Instances);
                }
                self.invite_account = None;
                self.notice = Some("Invite sent: open it in VRChat's notifications".into());
            }
            InviteAnswer::NeedsFriend { account, message } => {
                self.invite_account = Some(account);
                if self.open && matches!(self.page, Page::Instances | Page::AddFriend) {
                    self.go(Page::AddFriend);
                }
                self.notice = Some(cut(&message, 80));
            }
            InviteAnswer::Failed { message } => self.notice = Some(format!("Not invited: {}", cut(&message, 60))),
        }
    }

    /// A call started, changed or ended. A ringing call opens the call page.
    pub fn set_call(&mut self, call: Option<Call>) {
        if matches!(call, Some(Call::Ringing { .. })) && self.open && !matches!(self.call, Some(Call::Ringing { .. })) {
            self.go(Page::Call);
        }
        self.call = call;
    }

    /// The sentence N4N0 heard while the menu is open: on the search page it is what to search for, and the menu takes
    /// it (`Some`); elsewhere it is for N4N0's assistant (`None`).
    pub fn heard(&mut self, text: &str) -> Option<Vec<Effect>> {
        let text = text.trim().trim_end_matches(['.', '!', '?']).trim();
        if !self.open || text.is_empty() {
            return None;
        }
        if self.page == Page::Search {
            self.query = cut(text, QUERY_MAX);
            return Some(self.search());
        }
        // Said on Suggest features (Talk, or just spoken there), or on the keyboard page: the words go in the text.
        if self.page == Page::Suggest || (self.page == Page::Type && self.typing == Typing::Suggestion) {
            let spoken = text.trim();
            let target = if self.page == Page::Type { &mut self.typed } else { &mut self.suggestion };
            let joined = if target.trim().is_empty() { spoken.to_string() } else { format!("{} {spoken}", target.trim_end()) };
            *target = cut(&joined, SUGGESTION_MAX);
            self.suggestion_voice = true;
            self.dictating = false;
            self.notice = None;
            return Some(Vec::new());
        }
        if self.page == Page::NewState || (self.page == Page::Type && self.typing == Typing::StateName) {
            let name = cut(text, STATE_NAME_MAX);
            if self.page == Page::Type {
                self.typed = name;
            } else {
                self.draft.name = name;
            }
            return Some(Vec::new());
        }
        self.heard = Some(cut(text, 60));
        self.heard_private = true;
        None
    }
}

/// `text`'s first `max` characters.
pub fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// The name a state saved without one gets: "State n", `n` one past how many there are (and past any taken name).
pub fn next_state_name(states: &[StateRow]) -> String {
    let mut n = states.len() + 1;
    loop {
        let name = format!("State {n}");
        if !states.iter().any(|s| s.name.trim().eq_ignore_ascii_case(&name)) {
            return name;
        }
        n += 1;
    }
}

fn lower(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().flat_map(char::to_lowercase)
}

/// `text` starts with `prefix`, whatever their case (no copy made).
fn starts_ignoring_case(text: &str, prefix: &str) -> bool {
    let mut text = lower(text);
    lower(prefix).all(|c| text.next() == Some(c))
}

fn same_ignoring_case(a: &str, b: &str) -> bool {
    lower(a).eq(lower(b))
}

/// `text` in lines of at most `cols` characters, cut between words (a longer word is cut): what N4N0 says, a line at a
/// time on the screen's Home.
pub fn pieces(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let word = cut(word, cols);
        let width = line.chars().count();
        if width > 0 && width + 1 + word.chars().count() > cols {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// How long a line of `text` stays up: time to read its words.
pub fn reading_time(text: &str) -> Duration {
    Duration::from_millis((800 + 300 * text.split_whitespace().count() as u64).max(2500))
}

/// The pause after a keystroke before suggestions are asked for: typing a word asks once, not once a letter.
pub const SUGGEST_PAUSE: Duration = Duration::from_millis(130);
/// A request that has not answered by then is taken as lost (its own time limit is shorter).
const SUGGEST_LOST: Duration = Duration::from_secs(5);

/// How the hosts send the menu's [`Effect::Suggest`]s: the latest text only, once typing pauses for [`SUGGEST_PAUSE`],
/// one request at a time (the next waits for the answer, then asks for what the field holds by then).
#[derive(Debug, Default)]
pub struct SuggestQueue {
    wanted: Option<(u64, String, Instant)>,
    in_flight: Option<(u64, Instant)>,
}

impl SuggestQueue {
    /// The menu asked for suggestions for `text` (request `id`) at `now`: it replaces any not sent yet.
    pub fn want(&mut self, id: u64, text: String, now: Instant) {
        self.wanted = Some((id, text, now));
    }

    /// The request to send now, if one is due.
    pub fn due(&mut self, now: Instant) -> Option<(u64, String)> {
        if self.in_flight.is_some_and(|(_, since)| now.duration_since(since) < SUGGEST_LOST) {
            return None;
        }
        if self.wanted.as_ref().is_none_or(|(_, _, at)| now.duration_since(*at) < SUGGEST_PAUSE) {
            return None;
        }
        let (id, text, _) = self.wanted.take()?;
        self.in_flight = Some((id, now));
        Some((id, text))
    }

    /// Request `id` answered (or failed).
    pub fn answered(&mut self, id: u64) {
        if self.in_flight.is_some_and(|(i, _)| i == id) {
            self.in_flight = None;
        }
    }

    /// When a request may next be due (for a host that sleeps until then).
    pub fn next_due(&self) -> Option<Instant> {
        let (_, _, at) = self.wanted.as_ref()?;
        let after_pause = *at + SUGGEST_PAUSE;
        Some(match self.in_flight {
            Some((_, since)) => after_pause.max(since + SUGGEST_LOST),
            None => after_pause,
        })
    }
}

/// A duration as a call shows it: `0:42`, `12:05`, `1:02:03`.
pub fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn music_and_light_presses_show_at_once_and_ask_the_hosts() {
        let mut menu = Menu { pages: Pages::Six, ..Default::default() };
        menu.open();
        menu.press(Target::Music);
        assert!(menu.press(Target::MediaPlayPause).is_empty(), "nothing to play or pause");
        menu.media = Some(Media { title: "A song".into(), playing: true, can_play_pause: true, ..Default::default() });
        menu.volume = Some(Volume { percent: 98, muted: true });
        assert_eq!(menu.press(Target::MediaPlayPause), vec![Effect::Media(MediaCommand::PlayPause)]);
        assert!(!menu.media.as_ref().unwrap().playing, "paused at once");
        assert!(menu.press(Target::MediaNext).is_empty(), "the app can't go to the next one");
        assert_eq!(menu.press(Target::VolumeUp), vec![Effect::Media(MediaCommand::Volume(100))]);
        assert_eq!(menu.volume, Some(Volume { percent: 100, muted: false }), "turning it up unmutes");
        assert_eq!(menu.press(Target::Mute), vec![Effect::Media(MediaCommand::Mute(true))]);
        menu.press(Target::Back);
        assert_eq!(menu.page, Page::Home);

        menu.press(Target::Light);
        assert!(menu.press(Target::LightSwitch).is_empty(), "no light on this avatar");
        menu.light = Some(Light { on: false, color: 0, level: 0.6 });
        let effects = menu.press(Target::LightColor(3));
        assert_eq!(effects, vec![Effect::Light(Light { on: true, color: 3, level: 0.6 })], "a colour turns it on");
        assert_eq!(menu.press(Target::LightLevel(2)), vec![Effect::Light(Light { on: true, color: 3, level: 1.0 })]);
        assert_eq!(menu.press(Target::LightSwitch), vec![Effect::Light(Light { on: false, color: 3, level: 1.0 })]);
        assert!(menu.press(Target::LightColor(8)).is_empty(), "no ninth colour");
    }

    #[test]
    fn the_timeline_moves_the_music_where_it_is_tapped_or_dragged_to() {
        let mut menu = Menu { pages: Pages::Timeline, ..Default::default() };
        menu.open();
        menu.press(Target::Music);
        assert!(menu.press(Target::Seek(4)).is_empty(), "nothing plays");
        menu.media = Some(Media { title: "A song".into(), app: "Chrome".into(), can_seek: true, position: Some(10), duration: Some(160), ..Default::default() });
        // Segment 4 of 16 starts at a quarter: 40 s, shown there at once.
        assert_eq!(menu.press(Target::Seek(4)), vec![Effect::Media(MediaCommand::Seek(40))]);
        assert_eq!(menu.media.as_ref().unwrap().position, Some(40));
        assert_eq!(menu.press(Target::Timeline(0)), vec![Effect::Media(MediaCommand::Seek(0))], "a segment itself too");
        assert_eq!(menu.press(Target::Seek(200)), vec![Effect::Media(MediaCommand::Seek(150))], "never past the last segment");
        // A drag: each new segment under the finger moves the music once.
        assert_eq!(menu.drag(4, Target::Seek(4)), (4, vec![]));
        assert_eq!(menu.drag(4, Target::Seek(6)), (6, vec![Effect::Media(MediaCommand::Seek(60))]));
        assert_eq!(menu.drag(6, Target::MediaNext), (6, vec![]), "off the timeline: it stays where it was");
        // An app that lets no one move in its timeline says so; nothing to move in without a length.
        menu.media.as_mut().unwrap().can_seek = false;
        assert!(menu.press(Target::Seek(2)).is_empty());
        assert_eq!(menu.notice.as_deref(), Some("Chrome lets no app move in its timeline"));
        menu.media = Some(Media { title: "Radio".into(), can_seek: true, ..Default::default() });
        assert!(menu.press(Target::Seek(2)).is_empty());
        // Elsewhere than the music page, a drag does nothing.
        menu.media = Some(Media { title: "A song".into(), can_seek: true, duration: Some(160), ..Default::default() });
        menu.press(Target::Back);
        assert_eq!(menu.drag(1, Target::Seek(3)), (1, vec![]));
    }

    #[test]
    fn previous_goes_back_to_the_start_of_what_has_nothing_before() {
        let mut menu = Menu { pages: Pages::Timeline, ..Default::default() };
        menu.open();
        menu.press(Target::Music);
        menu.media = Some(Media { title: "A video".into(), can_seek: true, position: Some(95), duration: Some(300), ..Default::default() });
        assert_eq!(menu.press(Target::MediaPrevious), vec![Effect::Media(MediaCommand::Seek(0))]);
        assert_eq!(menu.media.as_ref().unwrap().position, Some(0));
        assert!(!layout::target_state(&menu, Target::MediaPrevious).hidden);
        menu.media.as_mut().unwrap().can_previous = true;
        assert_eq!(menu.press(Target::MediaPrevious), vec![Effect::Media(MediaCommand::Previous)], "the app's own previous first");
        menu.media = Some(Media { title: "A video".into(), duration: Some(300), ..Default::default() });
        assert!(menu.press(Target::MediaPrevious).is_empty());
        assert!(layout::target_state(&menu, Target::MediaPrevious).hidden);
    }

    #[test]
    fn take_back_shows_while_the_card_is_out_on_screens_that_have_it() {
        let mut menu = Menu { pages: Pages::Timeline, ..Default::default() };
        menu.open();
        menu.press(Target::Music);
        assert!(layout::target_state(&menu, Target::TakeBack).hidden, "no card out");
        assert!(menu.press(Target::TakeBack).is_empty());
        menu.card_out = true;
        assert!(!layout::target_state(&menu, Target::TakeBack).hidden);
        assert_eq!(menu.press(Target::TakeBack), vec![Effect::TakeBack]);
        assert!(!menu.card_out && layout::target_state(&menu, Target::TakeBack).hidden, "gone at once");
        assert_eq!(menu.notice.as_deref(), Some("Card taken back"));
        assert!(menu.press(Target::TakeBack).is_empty(), "once");
        // A screen before version 9 has no Take back (its picture has none there).
        menu.pages = Pages::Ten;
        menu.card_out = true;
        assert!(layout::target_state(&menu, Target::TakeBack).hidden);
    }

    fn video(id: &str) -> Video {
        Video { id: format!("{id:0>11}"), title: format!("Title {id}"), channel: "Channel".into(), duration: Some("3:21".into()), live: false }
    }

    fn results(ids: std::ops::Range<usize>, more: bool) -> Results {
        Results { videos: ids.map(|i| video(&i.to_string())).collect(), continuation: more.then(|| "token".to_string()) }
    }

    fn suggest(id: u64, text: &str) -> Effect {
        Effect::Suggest { id, text: text.into() }
    }

    #[test]
    fn types_suggests_and_searches() {
        let mut menu = Menu::default();
        assert!(menu.press(Target::YouTube).is_empty(), "closed: presses do nothing");
        menu.open();
        menu.press(Target::YouTube);
        assert_eq!(menu.page, Page::Search);
        assert!(menu.type_text(" ").is_empty(), "no leading space");
        assert_eq!(menu.press(Target::Key('l')), vec![suggest(1, "l")]);
        assert_eq!(menu.type_text("ofi"), vec![suggest(2, "lofi")]);
        assert_eq!(menu.type_text(" "), vec![suggest(3, "lofi ")]);
        assert!(menu.type_text(" ").is_empty(), "no double space");
        menu.suggestions_arrived(2, "lofi", vec!["lofi hip hop".into(), "lofi girl".into(), "LOFI".into()]);
        assert_eq!(
            menu.shown_suggestions().collect::<Vec<_>>(),
            vec!["lofi hip hop", "lofi girl"],
            "suggestions start like the field; the field itself is not repeated"
        );
        menu.suggestions_arrived(1, "l", vec!["lol".into()]);
        assert_eq!(menu.suggestions.len(), 3, "an older request's answer never replaces a newer one's");
        menu.suggestions_arrived(4, "lol", vec!["lol".into()]);
        assert_eq!(menu.suggestions.len(), 3, "an answer for another text is dropped");
        menu.backspace();
        assert_eq!(menu.query, "lofi");
        let effects = menu.press(Target::Suggestion(1));
        assert_eq!(effects, vec![Effect::Search { id: 1, query: "lofi girl".into() }]);
        assert_eq!((menu.page, menu.searching, menu.query.as_str()), (Page::Results, true, "lofi girl"));
        for _ in 0..60 {
            menu.type_text("x");
        }
        assert_eq!(menu.query, "lofi girl", "typing goes nowhere outside the search page");
    }

    #[test]
    fn suggestions_wait_for_a_pause_one_request_at_a_time() {
        let mut queue = SuggestQueue::default();
        let t0 = Instant::now();
        assert_eq!(queue.due(t0), None);
        queue.want(1, "l".into(), t0);
        queue.want(2, "lo".into(), t0 + Duration::from_millis(60));
        assert_eq!(queue.due(t0 + Duration::from_millis(100)), None, "still typing");
        assert_eq!(queue.next_due(), Some(t0 + Duration::from_millis(60) + SUGGEST_PAUSE));
        let sent = t0 + Duration::from_millis(200);
        assert_eq!(queue.due(sent), Some((2, "lo".into())), "the latest text only");
        queue.want(3, "lof".into(), sent);
        assert_eq!(queue.due(sent + Duration::from_millis(300)), None, "one request at a time");
        queue.answered(2);
        assert_eq!(queue.due(sent + Duration::from_millis(300)), Some((3, "lof".into())));
        assert_eq!(queue.due(sent + Duration::from_secs(1)), None, "nothing more wanted");
    }

    #[test]
    fn pages_through_results_and_fetches_ahead() {
        let mut menu = Menu::default();
        menu.open();
        let effects = menu.search_for("cats");
        assert_eq!(effects, vec![Effect::Search { id: 1, query: "cats".into() }]);
        assert!(menu.results_arrived(2, results(0..20, true), false).is_empty(), "results of another search are dropped");
        let effects = menu.results_arrived(1, results(0..12, true), false);
        assert_eq!(menu.page_results().len(), 5);
        assert!(matches!(&effects[..], [Effect::Thumbnails(ids)] if ids.len() == 10), "this page's and the next's: {effects:?}");
        assert!(!menu.has_previous() && menu.has_next());
        menu.press(Target::Next);
        assert_eq!(menu.result_page, 1);
        menu.press(Target::Next);
        // Page 3 has two results; there is no page after them yet, so more are fetched.
        assert_eq!(menu.result_page, 2);
        assert_eq!(menu.page_results().len(), 2);
        let effects = menu.press(Target::Next);
        assert!(effects.is_empty(), "already fetching");
        assert_eq!(menu.result_page, 2, "a page still filling up waits for its results");
        let effects = menu.results_arrived(1, results(12..20, false), true);
        assert!(!effects.is_empty());
        assert_eq!(menu.page_results().len(), 5);
        assert_eq!(menu.result_pages(), 4);
        let copy = menu.press(Target::Copy(0));
        assert_eq!(copy, vec![Effect::Copy { link: "https://www.youtube.com/watch?v=00000000010".into(), title: "Title 10".into() }]);
        assert!(menu.notice.is_some());
        menu.press(Target::Back);
        assert_eq!(menu.page, Page::Search, "back from results is the search");
        assert_eq!(menu.press(Target::Close), vec![Effect::Close]);
        assert!(!menu.open);
    }

    #[test]
    fn a_failed_or_empty_next_page_leaves_the_page_where_it_was() {
        let mut menu = Menu::default();
        menu.open();
        menu.search_for("cats");
        menu.results_arrived(1, results(0..10, true), false);
        menu.press(Target::Next);
        assert_eq!(menu.result_page, 1);
        // The last full page: Next fetches, the page turns once results come.
        let effects = menu.press(Target::Next);
        assert!(effects.is_empty() || matches!(&effects[..], [Effect::More { id: 1, .. }]), "{effects:?}");
        assert!(menu.loading());
        menu.results_failed(1, "timed out");
        assert_eq!((menu.result_page, menu.loading()), (1, false), "no page past the end");
        assert_eq!(layout::text(&menu, layout::Content::PageLabel), "2/2+");
        // Next again: this time results come, and the next page shows them from its first.
        assert!(matches!(&menu.press(Target::Next)[..], [Effect::More { id: 1, .. }]));
        menu.results_arrived(1, results(10..20, true), true);
        assert_eq!(menu.result_page, 2);
        assert_eq!(menu.page_results()[0].id, video("10").id, "nothing skipped");
        // An empty next page: the page stays, and there is no "+" any more.
        menu.press(Target::Next);
        menu.press(Target::Next);
        menu.results_arrived(1, Results::default(), true);
        assert_eq!(menu.result_page, 3);
        assert_eq!(layout::text(&menu, layout::Content::PageLabel), "4/4");
        // An older search's next page, for the same words, lands nowhere.
        menu.search_for("cats");
        assert!(menu.results_arrived(1, results(40..45, true), true).is_empty());
        assert!(menu.results.is_empty() && menu.searching);
    }

    #[test]
    fn cuts_what_orb_says_into_lines() {
        assert_eq!(pieces("Voilà ton sweat, va voir tes potes maintenant", 16), vec!["Voilà ton sweat,", "va voir tes", "potes maintenant"]);
        assert_eq!(pieces("abcdefghijklmnopqrstuvwxyz", 10), vec!["abcdefghij"]);
        assert!(pieces("  ", 10).is_empty());
        assert!(reading_time("one two") >= Duration::from_millis(2500));
    }

    #[test]
    fn walks_calls_and_takes_pictures() {
        let mut menu = Menu::new(KeyboardLayout::Azerty, 3, PhotoLength::Quick);
        menu.open();
        assert_eq!(menu.press(Target::FastWalk), vec![Effect::FastWalk(true)]);
        assert_eq!(menu.press(Target::FastWalk), vec![Effect::FastWalk(false)]);
        menu.desktop_mode = true;
        assert!(menu.press(Target::FastWalk).is_empty(), "no fast walk in desktop mode");
        menu.desktop_mode = false;
        menu.set_contacts((0..7).map(|i| Contact { id: format!("u{i}"), name: format!("Friend {i}") }).collect(), true, "");
        menu.press(Target::Call);
        menu.press(Target::Next);
        assert_eq!(menu.page_contacts().len(), 2);
        assert_eq!(menu.press(Target::Contact(1)), vec![Effect::CallContact { id: "u6".into() }]);
        menu.set_call(Some(Call::Calling { name: "Friend 6".into() }));
        assert!(menu.press(Target::Contact(0)).is_empty(), "one call at a time");
        assert_eq!(menu.press(Target::HangUp), vec![Effect::HangUp]);
        menu.set_call(None);
        menu.press(Target::Back);
        menu.set_call(Some(Call::Ringing { name: "Ana".into() }));
        assert_eq!(menu.page, Page::Call, "a ringing call opens the call page");
        assert_eq!(menu.press(Target::Answer), vec![Effect::Answer]);
        menu.press(Target::Back);
        menu.press(Target::Photo);
        assert_eq!(menu.press(Target::Timer(3)), vec![Effect::PhotoChoice { timer: 10, length: PhotoLength::Quick }]);
        assert_eq!(menu.press(Target::Length(2)), vec![Effect::PhotoChoice { timer: 10, length: PhotoLength::Full }]);
        assert_eq!(menu.press(Target::Take), vec![Effect::Photo3d { timer: 10, length: PhotoLength::Full }]);
        menu.countdown = Some(8);
        assert!(menu.press(Target::Take).is_empty(), "one picture at a time");
    }

    #[test]
    fn hears_a_search_on_the_search_page_only() {
        let mut menu = Menu::default();
        assert_eq!(menu.heard("cats"), None, "closed");
        menu.open();
        assert_eq!(menu.heard("what time is it?"), None, "on Home it is for N4N0");
        assert_eq!(menu.heard.as_deref(), Some("what time is it"));
        assert!(menu.heard_private, "the owner's own words");
        menu.press(Target::YouTube);
        assert_eq!(menu.heard("Lofi hip hop."), Some(vec![Effect::Search { id: 1, query: "Lofi hip hop".into() }]));
        assert_eq!(clock(42), "0:42");
        assert_eq!(clock(3723), "1:02:03");
    }

    #[test]
    fn a_display_with_its_own_field_and_list_drives_the_same_menu() {
        let mut menu = Menu::default();
        menu.open();
        menu.press(Target::YouTube);
        // A text field: what it shows becomes the query, its start trimmed and cut as typing would.
        assert_eq!(menu.set_query("  lofi"), vec![suggest(1, "lofi")]);
        assert!(menu.set_query("lofi").is_empty(), "the same text asks nothing");
        menu.set_query(&"x".repeat(60));
        assert_eq!(menu.query.chars().count(), QUERY_MAX);
        assert!(menu.set_query("").is_empty());
        assert!(menu.query.is_empty() && menu.suggestions.is_empty());
        // The whole friend list: the seventh friend is called as on its page.
        menu.set_contacts((0..7).map(|i| Contact { id: format!("u{i}"), name: format!("Friend {i}") }).collect(), true, "");
        assert_eq!(menu.call_contact(6), vec![Effect::CallContact { id: "u6".into() }]);
        assert_eq!(menu.contact_page, 1);
        assert!(menu.call_contact(7).is_empty(), "no such friend");
    }

    #[test]
    fn a_copied_link_offers_drop_where_cards_can_be_dropped() {
        let mut menu = Menu { pages: Pages::Seven, ..Default::default() };
        menu.open();
        menu.search_for("cats");
        menu.results_arrived(1, results(0..5, false), false);
        assert!(menu.press(Target::Drop(1)).is_empty(), "nothing copied yet");
        menu.press(Target::Copy(1));
        assert_eq!(menu.copied.as_deref(), Some(video("1").id.as_str()));
        assert!(layout::target_state(&menu, Target::Drop(1)).hidden, "an older screen, or none: no Drop");
        assert!(menu.press(Target::Drop(1)).is_empty());
        menu.can_drop = true;
        assert!(!layout::target_state(&menu, Target::Drop(1)).hidden, "the copied row offers it");
        assert!(layout::target_state(&menu, Target::Drop(0)).hidden, "only that row");
        assert_eq!(
            menu.press(Target::Drop(1)),
            vec![Effect::Drop { link: "https://www.youtube.com/watch?v=00000000001".into(), title: "Title 1".into(), video: Some(video("1").id) }]
        );
        assert_eq!(menu.page, Page::Results, "the hosts decide where the card goes (the screen shows it, the overlay closes)");
        // A new search forgets what was copied.
        menu.search_for("dogs");
        assert!(menu.copied.is_none());
    }

    #[test]
    fn a_dropped_card_is_not_the_menu_and_opening_the_menu_takes_it_back() {
        let mut menu = Menu { pages: Pages::Seven, ..Default::default() };
        menu.show_card(Card { link: "https://example.com".into(), title: "Example".into(), video: None, left: 15 });
        assert!(menu.card_shown() && menu.open && menu.page == Page::Card);
        assert!(menu.press(Target::States).is_empty(), "nothing on a card is pressed");
        assert!(menu.heard("search cats").is_none());
        // Its time is up: nothing shows.
        assert!(menu.end_card().is_some());
        assert!(!menu.open && !menu.card_shown() && menu.page == Page::Home);
        assert!(menu.end_card().is_none());
        // Dropped again, then the menu opens: Home, the card gone. Closing a card's screen ends it too.
        menu.show_card(Card { link: "https://example.com".into(), title: "Example".into(), video: None, left: 9 });
        menu.open();
        assert_eq!((menu.open, menu.page, menu.card.is_none()), (true, Page::Home, true));
        menu.show_card(Card { link: "https://example.com".into(), title: "Example".into(), video: None, left: 9 });
        assert_eq!(menu.close(), vec![Effect::Close]);
        assert!(menu.card.is_none() && menu.page == Page::Home);
    }

    #[test]
    fn a_state_saved_here_shows_until_the_host_lists_it() {
        let mut menu = Menu { pages: Pages::Seven, ..Default::default() };
        menu.open();
        menu.press(Target::States);
        assert!(menu.press(Target::StateSave).is_empty(), "no avatar known: nothing to save");
        let row = |id: &str, name: &str| StateRow { id: id.into(), name: name.into(), ..Default::default() };
        menu.set_states(vec![row("s1", "State 1"), row("s2", "Hoodie on")], true, "");
        assert_eq!(menu.press(Target::StateSave), vec![Effect::SaveState { name: "State 3".into(), size: true, pose: false, default: false }]);
        assert!(menu.saving());
        assert_eq!(layout::text(&menu, layout::Content::StatesTitle), "Saved as \"State 3\"");
        assert_eq!(menu.states.last().map(|s| (s.id.as_str(), s.name.as_str())), Some(("", "State 3")), "at once, before the host answers");
        assert!(menu.press(Target::StateApply(2)).is_empty(), "not listed by the host yet: nothing to apply");
        // The host's list, without it yet: it stays. With it: the host's.
        menu.set_states(vec![row("s1", "State 1"), row("s2", "Hoodie on")], true, "");
        assert_eq!(menu.states.len(), 3);
        menu.set_states(vec![row("s1", "State 1"), row("s2", "Hoodie on"), row("s3", "State 3")], true, "");
        assert_eq!(menu.states.len(), 3);
        assert!(!menu.saving());
        assert_eq!(menu.press(Target::StateApply(2)), vec![Effect::ApplyState { id: "s3".into(), name: "State 3".into() }]);
        assert_eq!(next_state_name(&[row("a", "State 2"), row("b", "state 3")]), "State 4", "past a name taken, whatever its case");
        // Twelve states: three pages of five.
        menu.set_states((1..=12).map(|i| row(&format!("s{i}"), &format!("State {i}"))).collect(), true, "");
        menu.press(Target::Next);
        menu.press(Target::Next);
        assert_eq!((menu.state_page, menu.page_states().len(), menu.has_next()), (2, 2, false));
        menu.set_states(vec![row("s1", "State 1")], true, "");
        assert_eq!(menu.state_page, 0, "fewer states: back to a page that exists");
    }

    #[test]
    fn a_new_state_is_named_and_chosen_on_its_page() {
        let mut menu = Menu { pages: Pages::Ten, ..Default::default() };
        menu.open();
        menu.set_states(vec![StateRow { id: "s1".into(), name: "State 1".into(), ..Default::default() }], true, "");
        menu.press(Target::States);
        assert!(menu.press(Target::StateSave).is_empty(), "New state opens its page, saving nothing yet");
        assert_eq!(menu.page, Page::NewState);
        assert_eq!(menu.draft, Draft { name: "State 2".into(), size: true, pose: false, default: false }, "the size on by default");
        // The name typed on the keyboard page: Done brings it back, Back would not.
        menu.press(Target::DraftName);
        assert_eq!((menu.page, menu.typing, menu.typed.as_str()), (Page::Type, Typing::StateName, "State 2"));
        for _ in 0..7 {
            menu.backspace();
        }
        menu.type_text("  Party  outfit");
        assert_eq!(menu.typed, "Party outfit", "no leading or double space");
        assert_eq!(layout::text(&menu, layout::Content::TypeLine(0)), "Party outfit|");
        menu.press(Target::Back);
        assert_eq!((menu.page, menu.draft.name.as_str()), (Page::NewState, "State 2"));
        menu.press(Target::DraftName);
        menu.set_states(menu.states.clone(), true, "");
        menu.typed = "Party outfit".into();
        menu.press(Target::Done);
        assert_eq!((menu.page, menu.draft.name.as_str()), (Page::NewState, "Party outfit"));
        menu.press(Target::DraftPose);
        menu.press(Target::DraftDefault);
        menu.press(Target::DraftSize);
        assert_eq!(layout::text(&menu, layout::Content::DraftNote), "Puts back its parameters and pose · also after each world join");
        assert_eq!(menu.press(Target::DraftSave), vec![Effect::SaveState { name: "Party outfit".into(), size: false, pose: true, default: true }]);
        assert_eq!(menu.page, Page::States);
        assert_eq!(menu.states.last().map(|s| (s.name.as_str(), s.pose, s.size, s.default)), Some(("Party outfit", true, false, true)));
        // A screen before version 6 has no New state page: its button saves at once, the size included.
        let mut old = Menu { pages: Pages::Seven, ..Default::default() };
        old.open();
        old.set_states(Vec::new(), true, "");
        old.press(Target::States);
        assert_eq!(old.press(Target::StateSave), vec![Effect::SaveState { name: "State 1".into(), size: true, pose: false, default: false }]);
        assert_eq!(
            layout::ScreenPage::of(&Menu { pages: Pages::Seven, page: Page::NewState, open: true, ..Default::default() }),
            layout::ScreenPage::HomeSeven
        );
    }

    #[test]
    fn the_world_video_is_watched_from_where_the_world_is() {
        let video = |url: &str, at: Option<u64>| WorldVideo { url: url.into(), title: None, at };
        assert_eq!(watch_link(&video("https://youtu.be/36YnV9STBqc", Some(1000)), 1090), "https://www.youtube.com/watch?v=36YnV9STBqc&t=90s");
        assert_eq!(watch_link(&video("https://www.youtube.com/watch?v=36YnV9STBqc", Some(1000)), 1002), "https://www.youtube.com/watch?v=36YnV9STBqc", "just started");
        assert_eq!(watch_link(&video("https://example.com/stream.m3u8", Some(1000)), 1090), "https://example.com/stream.m3u8");
        let mut menu = Menu { pages: Pages::Ten, ..Default::default() };
        menu.open();
        menu.press(Target::Music);
        assert!(menu.press(Target::WatchVideo).is_empty() && layout::target_state(&menu, Target::SaveVideo).hidden, "no video in the world");
        menu.world_video = Some(video("https://youtu.be/36YnV9STBqc", None));
        assert_eq!(menu.press(Target::SaveVideo), vec![Effect::SaveVideo]);
        assert!(matches!(&menu.press(Target::WatchVideo)[..], [Effect::WatchVideo { .. }]));
        menu.pages = Pages::Seven;
        assert!(layout::target_state(&menu, Target::WatchVideo).hidden, "older screens have no such buttons");
    }

    #[test]
    fn the_youtube_video_playing_is_copied_then_dropped_as_a_result() {
        let mut menu = Menu { pages: Pages::Ten, ..Default::default() };
        menu.open();
        menu.press(Target::Music);
        let shown = |menu: &Menu, target: Target| !layout::target_state(menu, target).hidden;
        menu.media = Some(Media { title: "Never Gonna Give You Up".into(), artist: "Rick Astley".into(), app: "Chrome".into(), ..Default::default() });
        assert!(!shown(&menu, Target::CopyPlaying) && menu.press(Target::CopyPlaying).is_empty(), "not found on YouTube (yet)");
        menu.media.as_mut().unwrap().video = Some("dQw4w9WgXcQ".into());
        assert!(shown(&menu, Target::CopyPlaying) && !shown(&menu, Target::DropPlaying));
        assert!(menu.press(Target::DropPlaying).is_empty(), "nothing copied yet");
        assert_eq!(
            menu.press(Target::CopyPlaying),
            vec![Effect::Copy { link: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(), title: "Never Gonna Give You Up".into() }]
        );
        assert_eq!(layout::text(&menu, layout::Content::MediaAlbum), "✓ Link copied");
        assert!(!shown(&menu, Target::DropPlaying) && menu.press(Target::DropPlaying).is_empty(), "no card can be dropped here");
        menu.can_drop = true;
        assert!(shown(&menu, Target::DropPlaying));
        assert_eq!(
            menu.press(Target::DropPlaying),
            vec![Effect::Drop {
                link: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
                title: "Never Gonna Give You Up".into(),
                video: Some("dQw4w9WgXcQ".into())
            }]
        );
        // Another video plays: its own link is to copy first.
        menu.media.as_mut().unwrap().video = Some("36YnV9STBqc".into());
        assert!(!shown(&menu, Target::DropPlaying) && layout::text(&menu, layout::Content::MediaAlbum).is_empty());
        // Older screens have no such buttons.
        menu.pages = Pages::Seven;
        assert!(!shown(&menu, Target::CopyPlaying));
        // They sit apart from the page's other buttons (the timeline's segments lie under the places a finger presses).
        let music = layout::cached(layout::ScreenPage::Music);
        for (i, a) in music.elements.iter().enumerate() {
            for b in &music.elements[i + 1..] {
                if let (Target::Timeline(s), Target::Seek(p)) = (a.target, b.target) {
                    if s == p {
                        continue;
                    }
                }
                let apart = a.rect.right() <= b.rect.x || b.rect.right() <= a.rect.x || a.rect.bottom() <= b.rect.y || b.rect.bottom() <= a.rect.y;
                assert!(apart, "{:?} and {:?} overlap", a.target, b.target);
            }
        }
    }

    #[test]
    fn tracking_strengths_turn_smoothing_on() {
        let mut menu = Menu { pages: Pages::Ten, ..Default::default() };
        menu.open();
        menu.press(Target::Tracking);
        assert_eq!(menu.page, Page::Tracking);
        assert!(menu.press(Target::Smoothing(5)).is_empty(), "nothing to set without the host's smoothing");
        assert!(layout::target_state(&menu, Target::Smoothing(5)).unavailable);
        menu.tracking = Some(Tracking { on: false, levels: [0, 1, 2], note: String::new() });
        // Controllers (kind 1) to High (3).
        assert_eq!(menu.press(Target::Smoothing(7)), vec![Effect::Tracking { on: true, levels: [0, 3, 2] }]);
        assert!(layout::target_state(&menu, Target::Smoothing(7)).on && !layout::target_state(&menu, Target::Smoothing(5)).on);
        assert_eq!(menu.press(Target::TrackingSwitch), vec![Effect::Tracking { on: false, levels: [0, 3, 2] }]);
        assert!(!layout::target_state(&menu, Target::Smoothing(7)).on, "off: no strength ringed");
        assert!(menu.press(Target::Smoothing(12)).is_empty(), "no fourth kind");
    }

    #[test]
    fn a_suggestion_is_typed_or_said_then_sent() {
        let mut menu = Menu { pages: Pages::Ten, ..Default::default() };
        menu.open();
        menu.press(Target::Suggest);
        assert!(menu.press(Target::SuggestSend).is_empty(), "nothing to send yet");
        assert!(layout::target_state(&menu, Target::SuggestSend).unavailable);
        assert_eq!(menu.press(Target::SuggestTalk), vec![Effect::Dictate]);
        assert!(menu.dictating && layout::target_state(&menu, Target::SuggestTalk).on);
        assert_eq!(menu.heard("A dance mode for N4N0."), Some(Vec::new()), "said on the page: it is the suggestion's");
        assert_eq!(menu.suggestion, "A dance mode for N4N0");
        assert!(!menu.dictating && menu.suggestion_voice);
        menu.press(Target::SuggestType);
        menu.type_text(" with music");
        menu.press(Target::Done);
        assert_eq!((menu.page, menu.suggestion.as_str()), (Page::Suggest, "A dance mode for N4N0 with music"));
        assert_eq!(
            menu.press(Target::SuggestSend),
            vec![Effect::SendSuggestion { text: "A dance mode for N4N0 with music".into(), voice: true }]
        );
        assert!(menu.press(Target::SuggestSend).is_empty(), "once while it is on its way");
        menu.suggestion_sent(Err("offline".into()));
        assert_eq!(menu.suggestion, "A dance mode for N4N0 with music", "kept when it failed");
        menu.press(Target::SuggestSend);
        menu.suggestion_sent(Ok(()));
        assert!(menu.suggestion.is_empty() && layout::text(&menu, layout::Content::SuggestNote).starts_with("Thank you"));
        // Everyone else's screen never shows what the owner writes.
        menu.suggestion = "private idea".into();
        assert_eq!(layout::text_for(&menu, layout::Content::SuggestLine(0), true), "");
    }

    fn instance(i: u64) -> InstanceRow {
        InstanceRow { location: format!("wrld_99123966-6b37-4b5b-9f3c-0a1b2c3d4e5f:{i}~region(eu)"), world: format!("World {i}"), at: 1000 + i }
    }

    #[test]
    fn the_simple_home_shows_what_plays_and_more_holds_the_rest() {
        use layout::ScreenPage;
        let mut menu = Menu { pages: Pages::Simple, ..Default::default() };
        menu.open();
        assert_eq!(ScreenPage::of(&menu), ScreenPage::HomeQuiet, "nothing plays: the tiles take its place");
        menu.media = Some(Media { title: "Midnight City".into(), ..Default::default() });
        assert_eq!(ScreenPage::of(&menu), ScreenPage::HomeSimple);
        menu.press(Target::More);
        assert_eq!(ScreenPage::of(&menu), ScreenPage::More);
        for (target, page) in [(Target::Light, Page::Light), (Target::Tracking, Page::Tracking), (Target::Suggest, Page::Suggest)] {
            menu.press(target);
            assert_eq!(menu.page, page);
            menu.press(Target::Back);
            assert_eq!(menu.page, Page::More, "{page:?} goes back to More");
        }
        menu.press(Target::Back);
        assert_eq!(menu.page, Page::Home);
        // A screen of version 9 keeps its ten-tile Home, and never shows More's pages.
        let mut old = Menu { pages: Pages::Timeline, ..Default::default() };
        old.open();
        old.press(Target::More);
        assert_eq!(ScreenPage::of(&old), ScreenPage::HomeTen);
        old.page = Page::Light;
        old.press(Target::Back);
        assert_eq!(old.page, Page::Home, "its Light is on Home");
    }

    #[test]
    fn more_has_the_qr_code_scanner_from_version_11_and_its_page_scans_while_it_shows() {
        use crate::qr::{QrPhase, QrView};
        use layout::{cached, target_state, text, text_for, Content, Look, ScreenPage};
        let mut menu = Menu { pages: Pages::Qr, ..Default::default() };
        menu.open();
        menu.press(Target::More);
        assert_eq!(ScreenPage::of(&menu), ScreenPage::MoreQr);
        let cards = |page: ScreenPage| cached(page).elements.iter().filter(|e| e.look == Look::Tile).map(|e| e.target).collect::<Vec<_>>();
        assert_eq!(cards(ScreenPage::MoreQr), [Target::Light, Target::Instances, Target::Tracking, Target::Qr, Target::Suggest]);
        assert_eq!(cards(ScreenPage::More), [Target::Light, Target::Instances, Target::Tracking, Target::Suggest], "version 10's More as it was");
        // Opening the page starts the scanner; its buttons show as it stands.
        assert_eq!(menu.press(Target::Qr), [Effect::QrScan(true)]);
        assert_eq!((menu.page, ScreenPage::of(&menu)), (Page::Qr, ScreenPage::Qr));
        menu.qr = QrView { phase: QrPhase::Starting, ..Default::default() };
        assert!(target_state(&menu, Target::QrAgain).hidden && target_state(&menu, Target::QrCopy).hidden);
        assert_eq!(text(&menu, Content::QrState), "Starting…");
        assert_eq!(text(&menu, Content::TileNote(layout::NOTE_QR)), "Scanning…");
        menu.qr = QrView { phase: QrPhase::NoCamera, note: crate::qr::no_camera_note(Some(false)), ..Default::default() };
        assert_eq!(text(&menu, Content::QrState), "Waiting for VRChat's camera");
        let note: Vec<String> = (0..layout::QR_NOTE_LINES as u8).map(|i| text(&menu, Content::QrNote(i))).collect();
        assert_eq!(note.iter().filter(|l| !l.is_empty()).cloned().collect::<Vec<_>>().join(" "), crate::qr::no_camera_note(Some(false)));
        assert!(note[0].starts_with("Open VRChat's camera") && note.iter().all(|l| l.chars().count() <= layout::QR_NOTE_COLUMNS), "{note:?}");
        // A link opened: shown to the owner only, copied, opened again.
        menu.qr = QrView { phase: QrPhase::Looking, link: Some("https://orbiters.cc/maps".into()), opened: 1, note: String::new() };
        assert_eq!(text(&menu, Content::QrState), "Opened · looking for more");
        assert_eq!(text(&menu, Content::QrLink), "orbiters.cc/maps");
        assert_eq!(text_for(&menu, Content::QrLink, true), "", "everyone else's screen leaves the link out");
        assert_eq!(menu.press(Target::QrCopy), [Effect::CopyText { text: "https://orbiters.cc/maps".into() }]);
        assert_eq!(text(&menu, Content::QrNotice), "Link copied");
        assert_eq!(menu.press(Target::QrOpen), [Effect::OpenLink { link: "https://orbiters.cc/maps".into() }]);
        assert!(menu.press(Target::QrAgain).is_empty(), "it scans already");
        // Stopped on its own: Scan again starts it.
        menu.qr.phase = QrPhase::Stopped;
        assert!(!target_state(&menu, Target::QrAgain).hidden);
        assert_eq!(menu.press(Target::QrAgain), [Effect::QrScan(true)]);
        // Leaving the page stops it: back to More, or the menu closing.
        assert_eq!(menu.press(Target::Back), [Effect::QrScan(false)]);
        assert_eq!(menu.page, Page::More);
        menu.press(Target::Qr);
        assert_eq!(menu.press(Target::Close), [Effect::QrScan(false), Effect::Close]);
        menu.open();
        assert_eq!(menu.page, Page::Home);
        // A screen of version 10 has no scanner: its More is version 10's.
        let mut old = Menu { pages: Pages::Simple, ..Default::default() };
        old.open();
        old.press(Target::More);
        assert_eq!(ScreenPage::of(&old), ScreenPage::More);
        old.page = Page::Qr;
        assert_eq!(ScreenPage::of(&old), ScreenPage::HomeQuiet);
        assert!(target_state(&old, Target::QrAgain).hidden);
    }

    #[test]
    fn the_3d_picture_page_shares_the_last_capture_and_history_from_version_12() {
        use crate::captures::{CapturePhase, CaptureRow, CapturesView, Friend, Friends, ShareAsk, Sharing, EXPLICIT_WARNING};
        use layout::{cached, picture_key_for, target_state, text, text_for, Content, ScreenPage};
        let row = |key: &str, world: &str| CaptureRow { key: key.into(), world: world.into(), phase: CapturePhase::Ready, ..Default::default() };
        let mut menu = Menu { pages: Pages::Share, ..Default::default() };
        menu.open();
        // No capture yet: the page and its History, no shares.
        assert!(menu.press(Target::Photo).is_empty());
        assert_eq!(ScreenPage::of(&menu), ScreenPage::PhotoShare);
        assert!(target_state(&menu, Target::ShareLatest).hidden && target_state(&menu, Target::ShareLatestTo).hidden);
        assert!(!target_state(&menu, Target::History).hidden);
        assert_eq!(text(&menu, Content::LastWorld), "No 3D capture yet");
        // One being recorded: its picture asked for, its shares greyed out until it is saved.
        let mut latest = row("C:/s/2026-10-29_10-00-00", "Rooftop");
        latest.phase = CapturePhase::Recording;
        let view = |latest: &CaptureRow| CapturesView { latest: Some(latest.clone()), ..Default::default() };
        assert_eq!(menu.set_captures(view(&latest)), [Effect::Thumbnails(vec!["capture:C:/s/2026-10-29_10-00-00".into()])]);
        assert!(target_state(&menu, Target::ShareLatest).unavailable && target_state(&menu, Target::ShareLatestTo).unavailable);
        assert_eq!((text(&menu, Content::LastWorld), text(&menu, Content::LastState)), ("Rooftop".into(), "Recording…".into()));
        // Its 3D view being made: a share is asked at once and waits for it, one at a time.
        latest.phase = CapturePhase::Making { percent: 40 };
        menu.set_captures(view(&latest));
        assert!(target_state(&menu, Target::ShareLatest).pointable());
        let public = ShareAsk { key: latest.key.clone(), public: true, ..Default::default() };
        assert_eq!(menu.press(Target::ShareLatest), [Effect::ShareCapture(public.clone())]);
        latest.sharing = Sharing::Waiting { public: true, people: 0 };
        menu.set_captures(view(&latest));
        assert_eq!(text(&menu, Content::LastState), "Shares when ready");
        assert!(target_state(&menu, Target::ShareLatest).unavailable);
        // The server wants its explicit content acknowledged: the warning, then Share anyway asks again.
        latest.sharing = Sharing::Explicit;
        menu.set_captures(view(&latest));
        assert_eq!((menu.page, ScreenPage::of(&menu)), (Page::Explicit, ScreenPage::Explicit));
        assert_eq!(format!("{} {}", text(&menu, Content::ExplicitLine(0)), text(&menu, Content::ExplicitLine(1))), EXPLICIT_WARNING);
        assert_eq!(menu.press(Target::ShareAnyway), [Effect::ShareCapture(ShareAsk { acknowledge_explicit: true, ..public })]);
        assert_eq!(menu.page, Page::Photo);
        // The answer before, still in its row until the host takes the new share, opens no warning again.
        menu.set_captures(CapturesView { note: "again".into(), ..view(&latest) });
        assert_eq!(menu.page, Page::Photo);
        latest.sharing = Sharing::Busy { public: true, people: 0 };
        // History: listed as it opens, five a page, both shares on each row.
        let mut flagged = row("C:/s/b", "Club");
        flagged.explicit = true;
        flagged.explicit_reasons = vec!["osc-parameter:NSFW/Toggle".into()];
        let mut history = vec![flagged];
        history.extend((0..7).map(|i| row(&format!("C:/s/{i}"), "World")));
        assert_eq!(menu.press(Target::History), [Effect::Captures]);
        assert_eq!(text(&menu, Content::HistoryTitle), "Listing…");
        let listed = CapturesView { latest: Some(latest.clone()), history: history.clone(), listed: true, note: String::new() };
        let keys = |rows: &[CaptureRow]| rows.iter().map(CaptureRow::picture_key).collect::<Vec<_>>();
        assert_eq!(menu.set_captures(listed.clone()), [Effect::Thumbnails(keys(&history[..5]))]);
        assert_eq!((text(&menu, Content::HistoryTitle), text(&menu, Content::PageLabel)), ("8 captures".into(), "1/2".into()));
        assert!(text(&menu, Content::CaptureMeta(1)).ends_with("· Ready to share"), "{}", text(&menu, Content::CaptureMeta(1)));
        assert_eq!(menu.press(Target::Next), [Effect::Thumbnails(keys(&history[5..]))]);
        assert!(target_state(&menu, Target::SharePublic(2)).pointable() && target_state(&menu, Target::ShareTo(3)).hidden);
        menu.press(Target::Previous);
        // A capture flagged explicit warns before it is shared publicly, and shows its picture to its owner only.
        assert_eq!(menu.press(Target::SharePublic(0)), [Effect::Thumbnails(vec!["capture:C:/s/b".into()])]);
        assert_eq!(menu.page, Page::Explicit);
        assert_eq!(text(&menu, Content::ExplicitWhy(0)), "Because of NSFW/Toggle");
        assert_eq!(text_for(&menu, Content::ExplicitWhy(0), true), "", "why is the owner's");
        let picture = &cached(ScreenPage::Explicit).pictures[0];
        assert_eq!(picture_key_for(&menu, picture, false).as_deref(), Some("capture:C:/s/b"));
        assert_eq!(picture_key_for(&menu, picture, true), None, "everyone else's screen never shows it");
        assert!(menu.press(Target::CancelShare).is_empty());
        assert_eq!(menu.page, Page::History);
        // Share to …: the friends read, chosen, found by name, sent to.
        assert_eq!(menu.press(Target::ShareTo(1)), [Effect::Friends(true)]);
        assert_eq!((menu.page, text(&menu, Content::FriendsTitle)), (Page::ShareTo, "Loading friends…".into()));
        let f = |id: &str, name: &str| Friend { id: id.into(), name: name.into() };
        let names = ["Alice", "bob", "Carol", "Dave", "Eve", "Bobby", "Zed"];
        menu.set_friends(Friends::ready(names.iter().enumerate().map(|(i, n)| f(&format!("usr_{i}"), n)).collect()));
        assert_eq!((text(&menu, Content::FriendsTitle), text(&menu, Content::ShareFriend(1))), ("7 friends".into(), "bob".into()));
        assert_eq!(text_for(&menu, Content::ShareFriend(1), true), "", "the owner's friends are never named to everyone");
        assert!(target_state(&menu, Target::SendShare).unavailable);
        menu.press(Target::Friend(0));
        menu.press(Target::Friend(1));
        assert!(target_state(&menu, Target::FriendMark(1)).on && target_state(&menu, Target::FriendMark(2)).hidden);
        assert_eq!(text(&menu, Content::FriendsNote), "2 friends chosen");
        menu.press(Target::FriendMark(0));
        assert_eq!(menu.chosen, [f("usr_1", "bob")], "its mark unchooses a friend");
        // Finding "bo": the keyboard, then only those, the chosen ones kept.
        assert!(menu.press(Target::FindFriend).is_empty(), "the friends stay while their keyboard shows");
        assert_eq!((menu.page, text(&menu, Content::TypeTitle)), (Page::Type, "Find a friend by name".into()));
        menu.type_text("bo");
        assert_eq!(text_for(&menu, Content::TypeLine(0), true), "");
        menu.press(Target::Done);
        assert_eq!((menu.page, text(&menu, Content::FriendsTitle)), (Page::ShareTo, "\"bo\": 2".into()));
        menu.press(Target::Friend(1));
        let sent = ShareAsk { key: "C:/s/0".into(), public: false, recipients: vec![f("usr_1", "bob"), f("usr_5", "Bobby")], acknowledge_explicit: false };
        assert_eq!(menu.press(Target::SendShare), [Effect::ShareCapture(sent), Effect::Friends(false)]);
        assert_eq!((menu.page, text(&menu, Content::HistoryTitle)), (Page::History, "Sharing with 2 friends…".into()));
        // It failed: why, where the page says what was just done; its row says it in short.
        let mut failed = listed.clone();
        failed.history[1].sharing = Sharing::Failed { message: "Sign in to Orbiters first".into() };
        menu.set_captures(failed);
        assert_eq!(text(&menu, Content::HistoryTitle), "Not shared: Sign in to Orbiters first");
        assert!(text(&menu, Content::CaptureMeta(1)).ends_with("· Not shared"));
        // Closing the menu on Share to … forgets the friends too.
        menu.press(Target::ShareTo(2));
        assert_eq!(menu.press(Target::Close), [Effect::Friends(false), Effect::Close]);
        // A screen of version 11 keeps its 3D picture page, without History.
        let mut old = Menu { pages: Pages::Qr, captures: listed, ..Default::default() };
        old.open();
        old.press(Target::Photo);
        assert_eq!(ScreenPage::of(&old), ScreenPage::Photo);
        assert!(old.press(Target::History).is_empty() && old.page == Page::Photo);
        old.page = Page::History;
        assert_eq!(ScreenPage::of(&old), ScreenPage::HomeQuiet);
    }

    #[test]
    fn previous_instances_page_and_invite_through_orbiters() {
        let mut menu = Menu { pages: Pages::Simple, ..Default::default() };
        menu.open();
        menu.press(Target::More);
        assert_eq!(menu.press(Target::Instances), [Effect::Instances], "the host reads them");
        assert_eq!(menu.page, Page::Instances);
        menu.set_instances((0..7).rev().map(instance).collect(), true, "");
        assert_eq!(menu.page_instances().len(), INSTANCES_PER_PAGE);
        assert!(menu.has_next() && !menu.has_previous());
        menu.press(Target::Next);
        assert_eq!(menu.page_instances().len(), 2);
        assert!(layout::target_state(&menu, Target::InviteMe(2)).hidden, "no third row on the last page");
        // Invite me: asked once at a time, Orbiters' account to befriend first.
        let location = menu.page_instances()[1].location.clone();
        assert_eq!(menu.press(Target::InviteMe(1)), [Effect::InviteMe { location: location.clone() }]);
        assert!(menu.invite_waiting && layout::target_state(&menu, Target::InviteMe(0)).unavailable);
        assert!(menu.press(Target::InviteMe(0)).is_empty(), "one at a time");
        let account = InviteAccount { name: "Orbiters".into(), link: "https://vrchat.com/home/user/usr_1".into() };
        menu.invite_answered(InviteAnswer::NeedsFriend { account: account.clone(), message: "Add it first".into() });
        assert_eq!(menu.page, Page::AddFriend);
        assert_eq!(layout::text(&menu, layout::Content::FriendName), "Orbiters");
        assert_eq!(layout::text(&menu, layout::Content::FriendLink), "vrchat.com/home/user/usr_1");
        assert_eq!(menu.press(Target::CopyFriendName), [Effect::CopyText { text: "Orbiters".into() }]);
        assert_eq!(menu.press(Target::OpenFriendLink), [Effect::OpenLink { link: account.link.clone() }]);
        // Try again asks for the same instance; sent, the list shows again.
        assert_eq!(menu.press(Target::InviteAgain), [Effect::InviteMe { location }]);
        menu.invite_answered(InviteAnswer::Sent);
        assert_eq!(menu.page, Page::Instances);
        assert!(!menu.invite_waiting);
        assert_eq!(layout::text(&menu, layout::Content::InstancesTitle), "Invite sent: open it in VRChat's notifications");
        menu.press(Target::InviteMe(0));
        menu.invite_answered(InviteAnswer::Failed { message: "Link your VRChat account on Orbiters first.".into() });
        assert_eq!(menu.page, Page::Instances);
        assert!(menu.notice.as_deref().is_some_and(|n| n.starts_with("Not invited: Link your VRChat account")));
        menu.press(Target::Back);
        assert_eq!(menu.page, Page::More);
    }

    #[test]
    fn an_instance_says_who_may_join_and_where() {
        let row = |tags: &str| InstanceRow { location: format!("wrld_a:1{tags}"), ..Default::default() };
        assert_eq!((row("").kind(), row("").region()), ("Public", None));
        assert_eq!(row("~hidden(usr_x)~region(eu)").kind(), "Friends+");
        assert_eq!(row("~friends(usr_x)").kind(), "Friends");
        assert_eq!(row("~private(usr_x)").kind(), "Invite");
        assert_eq!(row("~private(usr_x)~canRequestInvite~region(jp)").kind(), "Invite+");
        assert_eq!(row("~group(grp_x)~groupAccessType(public)").kind(), "Group Public");
        assert_eq!(row("~group(grp_x)~groupAccessType(plus)").kind(), "Group+");
        assert_eq!(row("~group(grp_x)~groupAccessType(members)").kind(), "Group");
        assert_eq!(row("~region(use)").region(), Some("US E"));
        assert_eq!(row("~region(us)").region(), Some("US W"));
        assert_eq!((layout::ago(30), layout::ago(300), layout::ago(7200), layout::ago(3 * 86400)), ("just now".into(), "5 min ago".into(), "2 h ago".into(), "3 d ago".into()));
    }

    /// World feedback (screen version 13): More's card opens the world's page, Like and the labels show at once and ask
    /// the host, AI world asks for its reason on the keyboard (enough words or it stays), a comment shows first while
    /// on its way, the player's own can be deleted, the worlds are stepped through; older screens have none of it.
    #[test]
    fn world_feedback_flows_through_the_menu() {
        use crate::feedback::{FeedbackView, LabelCount, Subject, Why, WorldFeedback};
        use layout::{target_state, ScreenPage};
        let w = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";
        let other = "wrld_9936b6ff-e523-4066-9e40-a333320ff5d3";
        let mut menu = Menu { pages: Pages::Feedback, ..Default::default() };
        menu.open();
        menu.set_feedback(FeedbackView {
            signed_in: true,
            subjects: vec![Subject { world: w.into(), name: "Home".into(), why: Why::Here }, Subject { world: other.into(), name: "Shrine".into(), why: Why::Capture }],
            worlds: vec![WorldFeedback { world: w.into(), likes: 2, labels: vec![LabelCount { id: "mirror".into(), count: 3, mine: false }], ..Default::default() }],
        });
        menu.press(Target::More);
        assert_eq!(ScreenPage::of(&menu), ScreenPage::MoreFeedback);
        assert_eq!(menu.press(Target::WorldFeedback), [Effect::Feedback(FeedbackAsk::Show { world: w.into() })]);
        assert_eq!(ScreenPage::of(&menu), ScreenPage::Feedback);
        assert_eq!(menu.press(Target::Like), [Effect::Feedback(FeedbackAsk::Like { world: w.into(), on: true })]);
        assert_eq!(menu.shown_feedback().map(|f| (f.likes, f.liked)), Some((3, true)));
        assert!(target_state(&menu, Target::Like).on);
        assert_eq!(layout::text(&menu, layout::Content::TopChip(0)), "Mirror · 3");
        assert_eq!(menu.press(Target::TopLabel(0)), [Effect::Feedback(FeedbackAsk::Label { world: w.into(), label: "mirror".into(), on: true, text: None })]);
        assert!(target_state(&menu, Target::TopLabel(0)).on);
        // AI world: its reason first.
        menu.press(Target::Labels);
        let ai = orbiters_contracts::world_feedback::labels().iter().position(|l| l.id == "ai-world").unwrap() as u8;
        assert!(menu.press(Target::LabelChip(ai)).is_empty());
        assert_eq!((menu.page, menu.typing), (Page::Type, Typing::Justification));
        menu.type_text("looks generated");
        assert!(menu.press(Target::Done).is_empty(), "not enough words");
        assert_eq!(menu.page, Page::Type);
        assert!(layout::text(&menu, layout::Content::TypeTitle).contains("10 words"));
        menu.type_text(" because the signs are unreadable and the textures melt everywhere");
        let effects = menu.press(Target::Done);
        assert!(matches!(&effects[..], [Effect::Feedback(FeedbackAsk::Label { label, on: true, text: Some(t), .. })] if label == "ai-world" && t.starts_with("looks generated")));
        assert_eq!(menu.page, Page::FeedbackLabels);
        assert!(target_state(&menu, Target::LabelChip(ai)).on);
        assert_eq!(menu.press(Target::LabelChip(ai)), [Effect::Feedback(FeedbackAsk::Label { world: w.into(), label: "ai-world".into(), on: false, text: None })]);
        // A comment shows first, on its way; the player's own once Orbiters has it can be deleted.
        menu.press(Target::Back);
        menu.press(Target::WriteComment);
        assert_eq!((menu.page, menu.typing), (Page::Type, Typing::Comment));
        menu.type_text("Great mirror");
        assert_eq!(menu.press(Target::Done), [Effect::Feedback(FeedbackAsk::Comment { world: w.into(), text: "Great mirror".into() })]);
        assert_eq!(menu.page, Page::Comments);
        assert_eq!(menu.page_comments()[0].text, "Great mirror");
        assert!(target_state(&menu, Target::DeleteComment(0)).hidden, "not sent yet");
        let mut view = menu.feedback.clone();
        view.worlds[0].comments[0].id = 12;
        menu.set_feedback(view);
        assert!(!target_state(&menu, Target::DeleteComment(0)).hidden);
        assert_eq!(menu.press(Target::DeleteComment(0)), [Effect::Feedback(FeedbackAsk::Uncomment { world: w.into(), comment: 12 })]);
        assert!(menu.page_comments().is_empty());
        // The worlds to step through.
        menu.press(Target::Back);
        assert_eq!(menu.press(Target::Next), [Effect::Feedback(FeedbackAsk::Show { world: other.into() })]);
        assert_eq!(layout::text(&menu, layout::Content::FeedbackWorld), "Shrine");
        assert_eq!(layout::text(&menu, layout::Content::PageLabel), "2/2");
        // Signed out: nothing is sent, the page says why.
        let mut out = menu.feedback.clone();
        out.signed_in = false;
        menu.set_feedback(out);
        assert!(menu.press(Target::Like).is_empty());
        assert!(target_state(&menu, Target::Like).unavailable);
        // A screen of version 12 has no World feedback.
        let mut old = Menu { pages: Pages::of_screen(Some(12)), ..Default::default() };
        old.open();
        old.press(Target::More);
        assert_eq!(ScreenPage::of(&old), ScreenPage::MoreQr);
        assert!(old.press(Target::WorldFeedback).is_empty());
        assert_ne!(old.page, Page::Feedback);
    }
}
