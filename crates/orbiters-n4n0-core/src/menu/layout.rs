//! Where everything sits on the menu's pages, for both displays: the SteamVR overlay draws each page from this, and the
//! avatar's screen shows the same page as a picture rendered from it (the static parts: cards, buttons, icons, labels)
//! with the changing parts drawn over it from what the desktop sends (texts, thumbnails, which button is hovered,
//! pressed, on or hidden). A page is [`WIDTH`] × [`HEIGHT`] pixels; every position is on a 4-pixel grid, so the screen's
//! layout table holds it in a byte.

use super::strings::{self, Text};
use super::{
    Call, KeyboardLayout, Keys, Menu, Page, Pages, PhotoMode, Target, Typing, CAPTURES_PER_PAGE, CONTACTS_PER_PAGE, FRIENDS_PER_PAGE, INSTANCES_PER_PAGE,
    RESULTS_PER_PAGE, STATES_PER_PAGE, TIMELINE_STEPS, TIMERS, TRACKING_KINDS,
};
use crate::captures::{CaptureRow, Friends, Sharing};
use crate::feedback::{self, COMMENTS_PER_PAGE, TOP_LABELS};
use orbiters_i18n::{tr, tr_ctx, tr_n};
use std::sync::OnceLock;

pub const WIDTH: u16 = 1024;
pub const HEIGHT: u16 = 576;
/// The grid every position and size sits on.
pub const GRID: u16 = 4;
/// The page's corner radius.
pub const CARD_RADIUS: u16 = 28;

/// The pages as the screen numbers them (`N4N0/Screen` page byte; 0 shows nothing).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ScreenPage {
    Home = 1,
    SearchQwerty = 2,
    SearchAzerty = 3,
    SearchSymbols = 4,
    Results = 5,
    CallList = 6,
    CallRinging = 7,
    CallActive = 8,
    Photo = 9,
    /// Home with six tiles (music and light): screens of versions 3 and 4.
    HomeSix = 10,
    Music = 11,
    Light = 12,
    /// Home with seven tiles (States) and what N4N0 says in the eighth place: screens from version 5, and the overlay.
    HomeSeven = 13,
    /// The avatar's saved states (from version 5).
    States = 14,
    /// A link card dropped in the world (from version 5).
    Card = 15,
    /// Home with ten places (Tracking, Suggest features, what N4N0 says across three) and the versions: screens from
    /// version 6, and the overlay.
    HomeTen = 16,
    /// The keyboard for a state's name or a suggestion, in each of its keys (from version 6).
    TypeQwerty = 17,
    TypeAzerty = 18,
    TypeSymbols = 19,
    /// A state being made (from version 6).
    NewState = 20,
    /// SteamVR's tracking smoothed (from version 6).
    Tracking = 21,
    /// A feature to suggest (from version 6).
    Suggest = 22,
    /// The simple Home (from version 10): YouTube, 3D capture and More large, fast walk, calls and states under them, and
    /// what the computer plays at the bottom; [`ScreenPage::HomeQuiet`] while nothing plays.
    HomeSimple = 23,
    /// The simple Home with nothing playing: its tiles take the place of the music's.
    HomeQuiet = 24,
    /// More (from version 10): light, previous instances, smooth tracking, suggest.
    More = 25,
    /// The instances the player was in, each with Invite me (from version 10).
    Instances = 26,
    /// Add Orbiters' VRChat account as a friend first (from version 10).
    AddFriend = 27,
    /// More with five cards, the QR code scanner among them (from version 11).
    MoreQr = 28,
    /// The QR code scanner (from version 11).
    Qr = 29,
    /// The 3D picture with the last capture, its shares and History (from version 12).
    PhotoShare = 30,
    /// The 3D captures on the computer, each with Share publicly and Share to … (from version 12).
    History = 31,
    /// The friends a capture goes to (from version 12).
    ShareTo = 32,
    /// Sharing an explicit capture publicly: the warning (from version 12).
    Explicit = 33,
    /// A world's feedback: likes, the labels people gave most, the newest comment (from version 13).
    Feedback = 34,
    /// The world's labels, to give or take back (from version 13).
    FeedbackLabels = 35,
    /// The world's comments (from version 13).
    Comments = 36,
    /// More with room for eight cards, World feedback among them (from version 13).
    MoreFeedback = 37,
    /// The screen mirror: the camera's picture, Aim, Zoom in, Zoom out, Stop (from version 14).
    Mirror = 38,
    /// More with seven cards, the screen mirror among them (from version 14).
    MoreMirror = 39,
    /// The 3D picture with its two pictures as buttons, then the last capture as [`ScreenPage::PhotoShare`] has it (from
    /// version 15).
    PhotoTwo = 40,
    /// The simple Home with Stay here beside its close button (from version 16), with what plays and without.
    HomeSimpleStay = 41,
    HomeQuietStay = 42,
}

impl ScreenPage {
    pub const ALL: [ScreenPage; 42] = [
        ScreenPage::Home,
        ScreenPage::SearchQwerty,
        ScreenPage::SearchAzerty,
        ScreenPage::SearchSymbols,
        ScreenPage::Results,
        ScreenPage::CallList,
        ScreenPage::CallRinging,
        ScreenPage::CallActive,
        ScreenPage::Photo,
        ScreenPage::HomeSix,
        ScreenPage::Music,
        ScreenPage::Light,
        ScreenPage::HomeSeven,
        ScreenPage::States,
        ScreenPage::Card,
        ScreenPage::HomeTen,
        ScreenPage::TypeQwerty,
        ScreenPage::TypeAzerty,
        ScreenPage::TypeSymbols,
        ScreenPage::NewState,
        ScreenPage::Tracking,
        ScreenPage::Suggest,
        ScreenPage::HomeSimple,
        ScreenPage::HomeQuiet,
        ScreenPage::More,
        ScreenPage::Instances,
        ScreenPage::AddFriend,
        ScreenPage::MoreQr,
        ScreenPage::Qr,
        ScreenPage::PhotoShare,
        ScreenPage::History,
        ScreenPage::ShareTo,
        ScreenPage::Explicit,
        ScreenPage::Feedback,
        ScreenPage::FeedbackLabels,
        ScreenPage::Comments,
        ScreenPage::MoreFeedback,
        ScreenPage::Mirror,
        ScreenPage::MoreMirror,
        ScreenPage::PhotoTwo,
        ScreenPage::HomeSimpleStay,
        ScreenPage::HomeQuietStay,
    ];

    /// The page picture's file stem and the overlay screenshot's name.
    pub fn name(self) -> &'static str {
        match self {
            ScreenPage::Home => "home",
            ScreenPage::SearchQwerty => "search-qwerty",
            ScreenPage::SearchAzerty => "search-azerty",
            ScreenPage::SearchSymbols => "search-symbols",
            ScreenPage::Results => "results",
            ScreenPage::CallList => "call-list",
            ScreenPage::CallRinging => "call-ringing",
            ScreenPage::CallActive => "call-active",
            ScreenPage::Photo => "photo",
            ScreenPage::HomeSix => "home-six",
            ScreenPage::Music => "music",
            ScreenPage::Light => "light",
            ScreenPage::HomeSeven => "home-seven",
            ScreenPage::States => "states",
            ScreenPage::Card => "card",
            ScreenPage::HomeTen => "home-ten",
            ScreenPage::TypeQwerty => "type-qwerty",
            ScreenPage::TypeAzerty => "type-azerty",
            ScreenPage::TypeSymbols => "type-symbols",
            ScreenPage::NewState => "new-state",
            ScreenPage::Tracking => "tracking",
            ScreenPage::Suggest => "suggest",
            ScreenPage::HomeSimple => "home-simple",
            ScreenPage::HomeQuiet => "home-quiet",
            ScreenPage::More => "more",
            ScreenPage::Instances => "instances",
            ScreenPage::AddFriend => "add-friend",
            ScreenPage::MoreQr => "more-qr",
            ScreenPage::Qr => "qr",
            ScreenPage::PhotoShare => "photo-share",
            ScreenPage::History => "history",
            ScreenPage::ShareTo => "share-to",
            ScreenPage::Explicit => "explicit",
            ScreenPage::Feedback => "feedback",
            ScreenPage::FeedbackLabels => "feedback-labels",
            ScreenPage::Comments => "comments",
            ScreenPage::MoreFeedback => "more-feedback",
            ScreenPage::Mirror => "mirror",
            ScreenPage::MoreMirror => "more-mirror",
            ScreenPage::PhotoTwo => "photo-two",
            ScreenPage::HomeSimpleStay => "home-simple-stay",
            ScreenPage::HomeQuietStay => "home-quiet-stay",
        }
    }

    /// The Home of a display with `pages` (the simple Home as it is with nothing playing).
    pub fn home(pages: Pages) -> ScreenPage {
        match pages {
            Pages::Four => ScreenPage::Home,
            Pages::Six => ScreenPage::HomeSix,
            Pages::Seven => ScreenPage::HomeSeven,
            Pages::Ten | Pages::Timeline => ScreenPage::HomeTen,
            Pages::Simple | Pages::Qr | Pages::Share | Pages::Feedback | Pages::Mirror | Pages::TwoPictures => ScreenPage::HomeQuiet,
            Pages::StayHere => ScreenPage::HomeQuietStay,
        }
    }

    /// The Home `menu` shows: the simple one has what plays at its bottom, when something does.
    pub fn home_of(menu: &Menu) -> ScreenPage {
        if menu.pages.stay_here() && plays(menu) {
            ScreenPage::HomeSimpleStay
        } else if menu.pages.simple() && plays(menu) {
            ScreenPage::HomeSimple
        } else {
            ScreenPage::home(menu.pages)
        }
    }

    /// The page `menu` shows: a page the display does not have shows its Home.
    pub fn of(menu: &Menu) -> ScreenPage {
        match menu.page {
            Page::Music | Page::Light if !menu.pages.music_light() => ScreenPage::home(menu.pages),
            Page::States | Page::Card if !menu.pages.states_cards() => ScreenPage::home(menu.pages),
            Page::NewState | Page::Type | Page::Tracking | Page::Suggest if !menu.pages.ten() => ScreenPage::home(menu.pages),
            Page::More | Page::Instances | Page::AddFriend if !menu.pages.simple() => ScreenPage::home(menu.pages),
            Page::Qr if !menu.pages.qr() => ScreenPage::home_of(menu),
            Page::History | Page::ShareTo | Page::Explicit if !menu.pages.share() => ScreenPage::home_of(menu),
            Page::History => ScreenPage::History,
            Page::ShareTo => ScreenPage::ShareTo,
            Page::Explicit => ScreenPage::Explicit,
            Page::Feedback | Page::FeedbackLabels | Page::Comments if !menu.pages.feedback() => ScreenPage::home_of(menu),
            Page::Feedback => ScreenPage::Feedback,
            Page::FeedbackLabels => ScreenPage::FeedbackLabels,
            Page::Comments => ScreenPage::Comments,
            Page::Mirror if !menu.pages.mirror() => ScreenPage::home_of(menu),
            Page::Mirror => ScreenPage::Mirror,
            Page::More if menu.pages.mirror() => ScreenPage::MoreMirror,
            Page::More if menu.pages.feedback() => ScreenPage::MoreFeedback,
            Page::Photo if menu.pages.two_pictures() => ScreenPage::PhotoTwo,
            Page::Photo if menu.pages.share() => ScreenPage::PhotoShare,
            Page::More if menu.pages.qr() => ScreenPage::MoreQr,
            Page::More => ScreenPage::More,
            Page::Qr => ScreenPage::Qr,
            Page::Instances => ScreenPage::Instances,
            Page::AddFriend => ScreenPage::AddFriend,
            Page::NewState => ScreenPage::NewState,
            Page::Tracking => ScreenPage::Tracking,
            Page::Suggest => ScreenPage::Suggest,
            Page::Type => match (menu.keys, menu.layout) {
                (Keys::Symbols, _) => ScreenPage::TypeSymbols,
                (Keys::Letters, KeyboardLayout::Qwerty) => ScreenPage::TypeQwerty,
                (Keys::Letters, KeyboardLayout::Azerty) => ScreenPage::TypeAzerty,
            },
            Page::Home => ScreenPage::home_of(menu),
            Page::Music => ScreenPage::Music,
            Page::Light => ScreenPage::Light,
            Page::States => ScreenPage::States,
            Page::Card => ScreenPage::Card,
            Page::Search => match (menu.keys, menu.layout) {
                (Keys::Symbols, _) => ScreenPage::SearchSymbols,
                (Keys::Letters, KeyboardLayout::Qwerty) => ScreenPage::SearchQwerty,
                (Keys::Letters, KeyboardLayout::Azerty) => ScreenPage::SearchAzerty,
            },
            Page::Results => ScreenPage::Results,
            Page::Call => match menu.call {
                Some(Call::Ringing { .. }) => ScreenPage::CallRinging,
                Some(_) => ScreenPage::CallActive,
                None => ScreenPage::CallList,
            },
            Page::Photo => ScreenPage::Photo,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32 && x < (self.x + self.w) as f32 && y >= self.y as f32 && y < (self.y + self.h) as f32
    }

    pub fn right(&self) -> u16 {
        self.x + self.w
    }

    pub fn bottom(&self) -> u16 {
        self.y + self.h
    }
}

/// How an element looks (both displays draw these the same way).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Look {
    /// A big home tile with an icon tile, a title and a note.
    Tile = 1,
    /// A round icon button (back, close, previous, next).
    Round = 2,
    /// The main action: green.
    Accent = 3,
    /// The search field.
    Field = 4,
    /// A row of a list: shows only when hovered.
    Row = 5,
    /// A keyboard key.
    Key = 6,
    /// A keyboard key that is not a character (switch, backspace).
    KeyDark = 7,
    /// An option among others; green ring when chosen.
    Chip = 8,
    /// Hang up, decline: red.
    Danger = 9,
    /// A segment of a countdown bar: lit while its second is still to come, hidden once it went.
    Bar = 10,
    /// A text area: a dark panel the text sits on (a suggestion, a name being typed).
    Area = 11,
    /// A segment of the music's timeline (from screen version 9): a dim line, lit (switched on) once the music reached it.
    Track = 12,
}

/// The icons the pages use (Phosphor icons in Orbiters - N4N0's font).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Close,
    Back,
    Walk,
    YouTube,
    Phone,
    PhoneOff,
    Cube,
    Search,
    Backspace,
    Copy,
    Previous,
    Next,
    User,
    /// A suggestion goes into the field.
    Fill,
    Timer,
    Microphone,
    Music,
    Lightbulb,
    PlayPause,
    SkipBack,
    SkipForward,
    SpeakerLow,
    SpeakerHigh,
    SpeakerOff,
    /// Saved states (Home's seventh tile).
    States,
    /// Save how it is now.
    Save,
    /// Put back as it was.
    Apply,
    /// The default state.
    Star,
    /// Drop a copied link in the world.
    Drop,
    /// Grab a dropped card.
    Grab,
    Link,
    /// The Tracking tile.
    Tracking,
    /// The Suggest features tile.
    Suggest,
    /// Type a text.
    Keyboard,
    /// Make a new one (New state).
    Plus,
    /// Send a suggestion.
    Send,
    /// The N4N0 mark (its name, drawn), at the top left of Home.
    Mark,
    /// Done typing.
    Check,
    /// The state's size, its pose.
    Size,
    /// The simple Home's More.
    More,
    /// Previous instances.
    Instances,
    /// Open a page on the computer.
    Open,
    Pose,
    /// Watch a video on the computer.
    Watch,
    /// Take a dropped card back.
    TakeBack,
    /// The QR code scanner.
    Qr,
    /// Previous 3D captures.
    History,
    /// Share publicly.
    Public,
    /// Share to friends.
    Friends,
    /// The explicit warning.
    Warning,
    /// Like a world.
    Heart,
    /// A world's labels.
    Tag,
    /// Comments.
    Chat,
    /// Write a comment.
    Pencil,
    /// Delete one's own comment.
    Trash,
    /// The screen mirror's card, and on its page Aim, Zoom out and Stop (Zoom in is [`Icon::Plus`]).
    Mirror,
    Aim,
    Minus,
    Stop,
    /// A 3D picture of the people here, and of the world.
    People,
    World,
}

/// Home's features in their order (the first four as on the four-tile Home, the first six as on the six-tile one), each
/// with its icon, title and colour: every Home here, and the window's Home in Orbiters - N4N0 (in English).
pub const HOME_TILES: [(Target, Icon, Text, Tint); 9] = [
    (Target::FastWalk, Icon::Walk, Text::HomeFastWalk, Tint::Green),
    (Target::YouTube, Icon::YouTube, Text::HomeYouTube, Tint::Red),
    (Target::Call, Icon::Phone, Text::HomeCall, Tint::Blue),
    (Target::Photo, Icon::Cube, Text::HomePhoto, Tint::Violet),
    (Target::Music, Icon::Music, Text::HomeMusic, Tint::Cyan),
    (Target::Light, Icon::Lightbulb, Text::HomeLight, Tint::Amber),
    (Target::States, Icon::States, Text::HomeStates, Tint::Pink),
    (Target::Tracking, Icon::Tracking, Text::HomeTracking, Tint::Indigo),
    (Target::Suggest, Icon::Suggest, Text::HomeSuggest, Tint::Purple),
];

/// The simple Home's large tiles, then its small buttons (from screen version 10), as [`HOME_TILES`] are.
pub const SIMPLE_TILES: [(Target, Icon, Text, Tint); 3] = [
    (Target::YouTube, Icon::YouTube, Text::HomeYouTube, Tint::Red),
    (Target::Photo, Icon::Cube, Text::HomeCapture, Tint::Violet),
    (Target::More, Icon::More, Text::HomeMore, Tint::Cyan),
];
pub const SIMPLE_BUTTONS: [(Target, Icon, Text); 3] =
    [(Target::FastWalk, Icon::Walk, Text::HomeFastWalk), (Target::Call, Icon::Phone, Text::HomeCall), (Target::States, Icon::States, Text::HomeStates)];
/// More's cards (from screen version 10).
pub const MORE_CARDS: [(Target, Icon, Text, Tint); 4] = [
    (Target::Light, Icon::Lightbulb, Text::HomeLight, Tint::Amber),
    (Target::Instances, Icon::Instances, Text::MoreInstances, Tint::Blue),
    (Target::Tracking, Icon::Tracking, Text::MoreTracking, Tint::Indigo),
    (Target::Suggest, Icon::Suggest, Text::HomeSuggest, Tint::Purple),
];

/// More's cards from screen version 11: the QR code scanner before suggest.
pub const MORE_CARDS_QR: [(Target, Icon, Text, Tint); 5] = [
    (Target::Light, Icon::Lightbulb, Text::HomeLight, Tint::Amber),
    (Target::Instances, Icon::Instances, Text::MoreInstances, Tint::Blue),
    (Target::Tracking, Icon::Tracking, Text::MoreTracking, Tint::Indigo),
    (Target::Qr, Icon::Qr, Text::MoreQr, Tint::Green),
    (Target::Suggest, Icon::Suggest, Text::HomeSuggest, Tint::Purple),
];

/// More's cards from screen version 13, in two columns of four rows (the last row left for more): World feedback
/// beside suggest.
pub const MORE_CARDS_FEEDBACK: [(Target, Icon, Text, Tint); 6] = [
    (Target::Light, Icon::Lightbulb, Text::HomeLight, Tint::Amber),
    (Target::Instances, Icon::Instances, Text::MoreInstances, Tint::Blue),
    (Target::Tracking, Icon::Tracking, Text::MoreTracking, Tint::Indigo),
    (Target::Qr, Icon::Qr, Text::MoreQr, Tint::Green),
    (Target::WorldFeedback, Icon::Heart, Text::MoreFeedback, Tint::Pink),
    (Target::Suggest, Icon::Suggest, Text::HomeSuggest, Tint::Purple),
];

/// More's cards from screen version 14: the screen mirror beside World feedback, suggest alone in the last row.
pub const MORE_CARDS_MIRROR: [(Target, Icon, Text, Tint); 7] = [
    (Target::Light, Icon::Lightbulb, Text::HomeLight, Tint::Amber),
    (Target::Instances, Icon::Instances, Text::MoreInstances, Tint::Blue),
    (Target::Tracking, Icon::Tracking, Text::MoreTracking, Tint::Indigo),
    (Target::Qr, Icon::Qr, Text::MoreQr, Tint::Green),
    (Target::WorldFeedback, Icon::Heart, Text::MoreFeedback, Tint::Pink),
    (Target::Mirror, Icon::Mirror, Text::MoreMirror, Tint::Cyan),
    (Target::Suggest, Icon::Suggest, Text::HomeSuggest, Tint::Purple),
];

/// The screen mirror's picture place (thumbnail 0's, 16:9; the screen draws the camera's picture there) and its note's
/// cells.
pub const MIRROR_PICTURE: Rect = Rect::new(32, 96, 640, 360);
pub const MIRROR_NOTE_COLUMNS: usize = 92;

/// The labels page's chips: each label of the list (contracts `Schemas/world-feedback-labels.json`) in its order, by
/// its name's text.
pub const LABEL_TEXTS: [Text; 18] = [
    Text::LabelMeetup,
    Text::LabelChill,
    Text::LabelBright,
    Text::LabelDark,
    Text::LabelAiThumbnail,
    Text::LabelAiWorld,
    Text::LabelVideoPlayer,
    Text::LabelMirror,
    Text::LabelPens,
    Text::LabelPhotoSpot,
    Text::LabelMusic,
    Text::LabelGame,
    Text::LabelHorror,
    Text::LabelNature,
    Text::LabelCity,
    Text::LabelSleep,
    Text::LabelAvatars,
    Text::LabelQuest,
];

/// The tile notes ([`Content::TileNote`]) of the simple Home's More, of More's Previous instances, of its QR code
/// scanner and of its World feedback, past [`HOME_TILES`]' own.
pub const NOTE_MORE: u8 = 9;
pub const NOTE_INSTANCES: u8 = 10;
pub const NOTE_QR: u8 = 11;
pub const NOTE_FEEDBACK: u8 = 12;
pub const NOTE_MIRROR: u8 = 13;

/// The tile note of a feature's tile or card.
pub fn note_of(target: Target) -> u8 {
    match target {
        Target::More => NOTE_MORE,
        Target::Instances => NOTE_INSTANCES,
        Target::Qr => NOTE_QR,
        Target::WorldFeedback => NOTE_FEEDBACK,
        Target::Mirror => NOTE_MIRROR,
        _ => HOME_TILES.iter().position(|t| t.0 == target).unwrap_or(0) as u8,
    }
}

/// Whether the computer plays something the simple Home shows (a title, from Windows' media controls).
pub fn plays(menu: &Menu) -> bool {
    menu.media.as_ref().is_some_and(|m| !m.title.is_empty())
}

/// The tiles of a Home with `pages`.
pub fn home_tiles(pages: Pages) -> &'static [(Target, Icon, Text, Tint)] {
    match pages {
        Pages::Four => &HOME_TILES[..4],
        Pages::Six => &HOME_TILES[..6],
        Pages::Seven => &HOME_TILES[..7],
        Pages::Ten | Pages::Timeline | Pages::Simple | Pages::Qr | Pages::Share | Pages::Feedback | Pages::Mirror | Pages::TwoPictures | Pages::StayHere => &HOME_TILES,
    }
}

/// The 3D picture's timers in English ([`super::TIMERS`] in order, [`strings::TIMERS`]), on the window's Home.
pub const TIMER_LABELS: [&str; 4] = [strings::TIMERS[0].en(), strings::TIMERS[1].en(), strings::TIMERS[2].en(), strings::TIMERS[3].en()];
/// Its camera flights in English ([`super::PhotoLength::ALL`] in order, [`strings::FLIGHTS`]).
pub const LENGTH_LABELS: [&str; 3] = [strings::FLIGHTS[0].en(), strings::FLIGHTS[1].en(), strings::FLIGHTS[2].en()];

/// A tinted icon tile's colour (home tiles).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tint {
    Green,
    Red,
    Blue,
    Violet,
    Cyan,
    Amber,
    Pink,
    Indigo,
    Purple,
    /// One of the light's colours (contracts `n4n0::LIGHT_COLORS`): a chip shows it as a swatch.
    Light(u8),
}

/// Something that can be pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Element {
    pub target: Target,
    pub rect: Rect,
    pub look: Look,
    pub icon: Option<Icon>,
    /// What is written on it (in the menu's language: [`strings::Strings`]).
    pub label: Option<Text>,
    /// Home tiles: the icon tile's colour.
    pub tint: Option<Tint>,
    /// A wide tile: what never changes under its title, on the page's picture (a [`Label`] would be under the tile, and a
    /// [`Slot`] takes the screen's text bytes and has no translation).
    pub note: Option<Text>,
}

impl Element {
    /// The corner radius both displays round it with.
    pub fn radius(&self) -> u16 {
        match self.look {
            Look::Tile | Look::Area => 24,
            Look::Round | Look::Field | Look::Bar | Look::Track => self.rect.h / 2,
            Look::Accent | Look::Danger | Look::Chip => self.rect.h.min(56) / 2,
            Look::Row => 16,
            Look::Key | Look::KeyDark => 12,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Style {
    /// Near white.
    Primary = 0,
    /// Grey.
    Secondary = 1,
    /// Orbiters green.
    Accent = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Align {
    Left = 0,
    Center = 1,
}

/// What a text slot shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Content {
    /// Home: listening, thinking, or what to do.
    Status,
    /// Home: under a tile's title.
    TileNote(u8),
    /// Home: what N4N0 heard last.
    Heard,
    Query,
    Suggestion(u8),
    /// Results: the search, or the notice.
    ResultsTitle,
    /// Results and calls: "2/4".
    PageLabel,
    ResultTitle(u8),
    /// Channel and duration.
    ResultMeta(u8),
    /// Calls: who can be called, or why not.
    CallsTitle,
    ContactName(u8),
    CallName,
    CallState,
    PhotoStatus,
    /// Music: what plays, who by, its album, then whether it plays, where and which app.
    MediaTitle,
    MediaArtist,
    MediaAlbum,
    MediaState,
    /// The computer's volume.
    VolumeLabel,
    /// Light: on or off, its colour and brightness; or why there is none.
    LightState,
    LightNote,
    /// Seven-tile Home: who said what N4N0's place shows ("N4N0 says", "You said"), and its lines.
    HeardCaption,
    HeardLine(u8),
    /// States: how many there are for this avatar, or the notice; each row's name and what it puts back.
    StatesTitle,
    StateName(u8),
    StateMeta(u8),
    /// A dropped card: its title on two lines, its link.
    CardTitle(u8),
    CardLink,
    /// The ten-tile Home: its lines of what N4N0 says (wider than the seven-tile Home's), and the versions.
    HeardWide(u8),
    Versions,
    /// The keyboard page: what it types for, and the text's lines (the last ones, the caret at its end).
    TypeTitle,
    TypeLine(u8),
    /// New state: its name, and what applying it puts back.
    DraftName,
    DraftNote,
    /// Tracking: whether it smooths now, and what to do (restart SteamVR, a kind's strength).
    TrackingState,
    TrackingNote,
    /// Suggest features: the suggestion's lines, and how it goes.
    SuggestLine(u8),
    SuggestNote,
    /// Music: the world's video (from screen version 6).
    WorldVideo,
    /// Music: where it is and how long it lasts, at either end of its timeline (from screen version 9).
    MediaPosition,
    MediaDuration,
    /// Previous instances (from screen version 10): how many, or the notice; each row's world and what it is (who may
    /// join, its region, when).
    InstancesTitle,
    InstanceWorld(u8),
    InstanceMeta(u8),
    /// Add Orbiters as a friend: the account's VRChat name, its page, and what was just done.
    FriendName,
    FriendLink,
    FriendNote,
    /// The QR code scanner (from screen version 11): what it does, the lines of how (or why it stopped), the last link
    /// it opened (the owner's: everyone else's screen leaves it out), and what was just done.
    QrState,
    QrNote(u8),
    QrLink,
    QrNotice,
    /// The 3D picture page of version 12: the last capture's world, where it stands, and the lines of what that means.
    LastWorld,
    LastState,
    LastNote(u8),
    /// History: how many captures (or the notice), each row's world and when it was taken with where it stands.
    HistoryTitle,
    CaptureWorld(u8),
    CaptureMeta(u8),
    /// Share to …: how many friends (or which part of a name is found), each row's name (the owner's: everyone else's
    /// screen leaves them out), and how many are chosen (or the notice).
    FriendsTitle,
    ShareFriend(u8),
    FriendsNote,
    /// The explicit warning: the capture's world and where it stands, why it is flagged (the owner's), the warning.
    ExplicitWorld,
    ExplicitState,
    ExplicitWhy(u8),
    ExplicitLine(u8),
    /// World feedback (from screen version 13): the world's name, why it is there, what Like's card says (how many like
    /// it), a label people gave most with its count, the comments' count, the newest comment's author and when, its
    /// lines, and what was just done (or why nothing can be).
    FeedbackWorld,
    FeedbackWhy,
    LikeNote,
    TopChip(u8),
    CommentsCount,
    NewestMeta,
    NewestLine(u8),
    FeedbackNote,
    /// The labels page: the world's name and its note.
    LabelsWorld,
    LabelsNote,
    /// Comments: how many (or the notice), each comment's author and when, and its lines (comment × 2 + line).
    CommentsTitle,
    CommentMeta(u8),
    CommentLine(u8),
    /// The screen mirror: where the camera is (a countdown while it aims, the zoom once it stays), and a note.
    MirrorState,
    MirrorNote,
}

/// A line of changing text: `cols` cells of `size` pixels high and half as wide, from (`x`, `y`). On the screen, cell
/// `i` is byte `offset + i` of the screen's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Slot {
    pub x: u16,
    pub y: u16,
    pub cols: u8,
    pub size: u8,
    pub style: Style,
    pub align: Align,
    pub offset: u16,
    pub content: Content,
}

impl Slot {
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.cols as u16 * self.size as u16 / 2, self.size as u16)
    }
}

/// A thumbnail: a result's, or the music's art (thumbnail 0 of the music page).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Picture {
    pub rect: Rect,
    /// The thumbnail's place in the screen's memory: the result on the page (0 to 4).
    pub result: u8,
}

/// Text that never changes on a page (titles, key captions are element labels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Label {
    pub x: u16,
    pub y: u16,
    pub size: u8,
    pub text: Text,
    pub style: Style,
    pub bold: bool,
    /// The room it has from `x`: a text wider than this (another language's) is drawn smaller to fit.
    pub width: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub page: ScreenPage,
    pub elements: Vec<Element>,
    pub slots: Vec<Slot>,
    pub pictures: Vec<Picture>,
    pub labels: Vec<Label>,
    /// Icons drawn without being pressable (a page's illustration).
    pub icons: Vec<(Rect, Icon, Option<Tint>)>,
    /// Panels drawn without being pressable: the seven-tile Home's place for what N4N0 says (N4N0's face at its top
    /// left, its texts are slots).
    pub boxes: Vec<Rect>,
}

/// The most elements a page has (the screen's masks and layout table hold this many).
pub const MAX_ELEMENTS: usize = 48;
/// The most text slots a page has.
pub const MAX_SLOTS: usize = 16;
/// The thumbnails of a results page.
pub const PICTURES: usize = RESULTS_PER_PAGE;
/// The screen's text: bytes for every page's slots.
pub const TEXT_BYTES: u16 = 360;

/// The page's layout, built once: the screen's steps and the overlay's frames read it many times a second.
pub fn cached(page: ScreenPage) -> &'static Layout {
    static LAYOUTS: OnceLock<Vec<Layout>> = OnceLock::new();
    let all = LAYOUTS.get_or_init(|| ScreenPage::ALL.iter().map(|&p| layout(p)).collect());
    &all[ScreenPage::ALL.iter().position(|&p| p == page).unwrap_or(0)]
}

/// Where something pressable is.
pub fn hit(layout: &Layout, x: f32, y: f32) -> Option<(usize, Target)> {
    // The last drawn wins (buttons sit on rows).
    layout.elements.iter().enumerate().rev().find(|(_, e)| e.rect.contains(x, y)).map(|(i, e)| (i, e.target))
}

/// What a pointer at (`x`, `y`) hovers and presses on `menu`'s page: the last drawn element there that shows and is
/// available now (a hidden button lets the row under it take the press: a result's Drop before its link is copied).
pub fn hit_pointable(layout: &Layout, menu: &Menu, x: f32, y: f32) -> Option<(usize, Target)> {
    layout.elements.iter().enumerate().rev().find(|(_, e)| e.rect.contains(x, y) && state(menu, e).pointable()).map(|(i, e)| (i, e.target))
}

/// How far from a button a finger still takes it, in pixels (13 mm on the screen): a hand trembles more than that at arm's
/// length, and a Back button is 56 pixels wide.
pub const HIT_SLOP: f32 = 28.0;

/// What a finger at (`x`, `y`) takes: what is exactly there, else the nearest button within [`HIT_SLOP`]. What it hovers and
/// what it presses are the same, so the highlight says what a press would do; rows beside each other share their border as
/// they always did, because the exact hit comes first.
pub fn hit_near(layout: &Layout, menu: &Menu, x: f32, y: f32) -> Option<(usize, Target)> {
    if let Some(hit) = hit_pointable(layout, menu, x, y) {
        return Some(hit);
    }
    let distance = |r: &Rect| {
        let dx = (r.x as f32 - x).max(x - (r.x as f32 + r.w as f32)).max(0.0);
        let dy = (r.y as f32 - y).max(y - (r.y as f32 + r.h as f32)).max(0.0);
        (dx * dx + dy * dy).sqrt()
    };
    layout
        .elements
        .iter()
        .enumerate()
        .filter(|(_, e)| state(menu, e).pointable())
        .map(|(i, e)| (distance(&e.rect), i, e.target))
        .filter(|(d, ..)| *d <= HIT_SLOP)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, i, target)| (i, target))
}

fn element(target: Target, rect: Rect, look: Look) -> Element {
    Element { target, rect, look, icon: None, label: None, tint: None, note: None }
}

fn with(mut e: Element, icon: Option<Icon>, label: Option<Text>) -> Element {
    e.icon = icon;
    e.label = label;
    e
}

fn round(target: Target, x: u16, icon: Icon) -> Element {
    with(element(target, Rect::new(x, 24, 56, 56), Look::Round), Some(icon), None)
}

/// Builds a page's slots with consecutive offsets in the screen's text.
struct Slots {
    slots: Vec<Slot>,
    next: u16,
}

impl Slots {
    fn new() -> Self {
        Self { slots: Vec::new(), next: 0 }
    }

    fn add(&mut self, x: u16, y: u16, cols: u8, size: u8, style: Style, content: Content) -> &mut Self {
        self.slots.push(Slot { x, y, cols, size, style, align: Align::Left, offset: self.next, content });
        self.next += cols as u16;
        self
    }

    fn centered(&mut self) -> &mut Self {
        if let Some(last) = self.slots.last_mut() {
            last.align = Align::Center;
        }
        self
    }
}

/// The letters of the keyboard's rows, and where each row starts.
fn letter_rows(layout: KeyboardLayout) -> [(&'static str, u16); 3] {
    match layout {
        KeyboardLayout::Qwerty => [("qwertyuiop", 36), ("asdfghjkl", 84), ("'zxcvbnm", 36)],
        KeyboardLayout::Azerty => [("azertyuiop", 36), ("qsdfghjklm", 36), ("'wxcvbn-", 36)],
    }
}

const SYMBOL_ROWS: [(&str, u16); 3] = [("1234567890", 36), ("éèàçùêôïû&", 36), (".,?!:/@#", 36)];
/// The keyboard: four rows of keys 88 × 52, 8 apart.
const KEY_W: u16 = 88;
const KEY_H: u16 = 52;
const KEY_GAP: u16 = 8;
const KEY_ROWS_Y: [u16; 4] = [320, 380, 440, 500];

/// The keyboard's keys; its green key at the bottom right is `accent` (Search, or the keyboard page's Done).
fn keyboard(elements: &mut Vec<Element>, page: ScreenPage, layout: KeyboardLayout, accent: (Target, Icon, Text)) {
    let symbols = page == ScreenPage::SearchSymbols || page == ScreenPage::TypeSymbols;
    let rows = if symbols { SYMBOL_ROWS } else { letter_rows(layout) };
    for (r, (keys, x0)) in rows.iter().enumerate() {
        let y = KEY_ROWS_Y[r];
        for (i, c) in keys.chars().enumerate() {
            let x = x0 + i as u16 * (KEY_W + KEY_GAP);
            elements.push(element(Target::Key(c), Rect::new(x, y, KEY_W, KEY_H), Look::Key));
        }
        if r == 2 {
            // Backspace fills the third row to the right edge (988).
            let x = x0 + keys.chars().count() as u16 * (KEY_W + KEY_GAP);
            elements.push(with(element(Target::Backspace, Rect::new(x, y, 988 - x, KEY_H), Look::KeyDark), Some(Icon::Backspace), None));
        }
    }
    let y = KEY_ROWS_Y[3];
    let switch = if symbols { (Target::Letters, Text::KeyboardLetters) } else { (Target::Symbols, Text::KeyboardSymbols) };
    elements.push(with(element(switch.0, Rect::new(36, y, 184, KEY_H), Look::KeyDark), None, Some(switch.1)));
    elements.push(with(element(Target::Space, Rect::new(228, y, 568, KEY_H), Look::Key), None, Some(Text::KeyboardSpace)));
    elements.push(with(element(accent.0, Rect::new(804, y, 184, KEY_H), Look::Accent), Some(accent.1), Some(accent.2)));
}

/// The keyboard page's text: lines of this many characters, the last ones shown.
pub const TYPE_COLUMNS: usize = 74;
pub const TYPE_LINES: usize = 4;
/// The suggestion on Suggest features, the same way.
pub const SUGGEST_COLUMNS: usize = 72;
pub const SUGGEST_LINES: usize = 4;
/// What N4N0 says on the ten-tile Home: lines of so many characters, so many at a time.
pub const HEARD_WIDE_COLUMNS: usize = 44;
pub const HEARD_WIDE_LINES: usize = 3;

/// Where the music's timeline starts across the page, and its segments' pitch: sixteen of them reach 896.
const TIMELINE_X: u16 = 128;
const TIMELINE_STEP: u16 = 48;

/// The ten-tile Home's place `i` (0 to 11): three rows of four, 228 × 140 each, 16 apart across and 12 down.
fn ten_cell(i: usize) -> (u16, u16) {
    (32 + (i as u16 % 4) * 244, 96 + (i as u16 / 4) * 152)
}

/// The page's layout.
pub fn layout(page: ScreenPage) -> Layout {
    let mut elements = Vec::new();
    let mut slots = Slots::new();
    let mut pictures = Vec::new();
    let mut labels = Vec::new();
    let mut icons = Vec::new();
    let mut boxes = Vec::new();
    match page {
        ScreenPage::Home => {
            for (i, &(target, icon, title, tint)) in home_tiles(Pages::Four).iter().enumerate() {
                let x = 32 + i as u16 * 244;
                let mut tile = with(element(target, Rect::new(x, 104, 228, 324), Look::Tile), Some(icon), Some(title));
                tile.tint = Some(tint);
                elements.push(tile);
            }
            elements.push(round(Target::Close, 936, Icon::Close));
            icons.push((Rect::new(32, 24, 56, 56), Icon::Microphone, Some(Tint::Green)));
            icons.push((Rect::new(104, 28, 56, 48), Icon::Mark, None));
            slots.add(208, 40, 48, 24, Style::Accent, Content::Status);
            for i in 0..4u8 {
                slots.add(56 + i as u16 * 244, 376, 14, 24, Style::Secondary, Content::TileNote(i));
            }
            slots.add(32, 472, 64, 28, Style::Secondary, Content::Heard);
        }
        ScreenPage::SearchQwerty | ScreenPage::SearchAzerty | ScreenPage::SearchSymbols => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::Field, Rect::new(100, 24, 712, 56), Look::Field), Some(Icon::Search), None));
            elements.push(with(element(Target::Search, Rect::new(824, 24, 168, 56), Look::Accent), Some(Icon::Search), Some(Text::Search)));
            for i in 0..super::SUGGESTIONS as u8 {
                let row = Rect::new(32, 96 + i as u16 * 44, 960, 40);
                elements.push(with(element(Target::Suggestion(i), row, Look::Row), Some(Icon::Fill), None));
            }
            let layout = if page == ScreenPage::SearchAzerty { KeyboardLayout::Azerty } else { KeyboardLayout::Qwerty };
            keyboard(&mut elements, page, layout, (Target::Search, Icon::Search, Text::Search));
            slots.add(164, 40, super::QUERY_MAX as u8, 24, Style::Primary, Content::Query);
            for i in 0..super::SUGGESTIONS as u8 {
                slots.add(60, 104 + i as u16 * 44, 64, 24, Style::Primary, Content::Suggestion(i));
            }
        }
        ScreenPage::Results => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(round(Target::Previous, 808, Icon::Previous));
            elements.push(round(Target::Next, 936, Icon::Next));
            for i in 0..RESULTS_PER_PAGE as u8 {
                elements.push(element(Target::Copy(i), Rect::new(32, 96 + i as u16 * 92, 960, 84), Look::Row));
            }
            for i in 0..RESULTS_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92 + 20;
                elements.push(with(element(Target::Copy(i), Rect::new(820, y, 156, 48), Look::Accent), Some(Icon::Copy), Some(Text::LinkCopy)));
            }
            // After its Copy link, a row offers Drop (screens from version 5): between its title (which ends at 696) and its
            // Copy link. Last in the list, so the elements before keep their indices on older screens.
            for i in 0..RESULTS_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92 + 20;
                elements.push(with(element(Target::Drop(i), Rect::new(704, y, 108, 48), Look::KeyDark), Some(Icon::Drop), Some(Text::LinkDrop)));
            }
            // While the link card is out (screens from version 9): Take back, in the heading's row between the search's
            // words (which end at 536) and the pages' buttons.
            elements.push(with(element(Target::TakeBack, Rect::new(576, 28, 220, 48), Look::KeyDark), Some(Icon::TakeBack), Some(Text::CardTakeBack)));
            slots.add(104, 40, 36, 24, Style::Primary, Content::ResultsTitle);
            slots.add(868, 40, 5, 24, Style::Secondary, Content::PageLabel).centered();
            for i in 0..RESULTS_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92;
                pictures.push(Picture { rect: Rect::new(44, y + 4, 128, 72), result: i });
                slots.add(192, y + 12, 36, 28, Style::Primary, Content::ResultTitle(i));
                slots.add(192, y + 48, 26, 24, Style::Secondary, Content::ResultMeta(i));
            }
        }
        ScreenPage::CallList => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(round(Target::Previous, 808, Icon::Previous));
            elements.push(round(Target::Next, 936, Icon::Next));
            for i in 0..CONTACTS_PER_PAGE as u8 {
                // The row carries the friend's picture (at its left), so it goes with the row.
                elements.push(with(element(Target::Contact(i), Rect::new(32, 96 + i as u16 * 92, 960, 84), Look::Row), Some(Icon::User), None));
            }
            for i in 0..CONTACTS_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92;
                elements.push(with(element(Target::Contact(i), Rect::new(840, y + 20, 136, 48), Look::Accent), Some(Icon::Phone), Some(Text::CallsCall)));
            }
            slots.add(104, 40, 52, 24, Style::Primary, Content::CallsTitle);
            slots.add(868, 40, 5, 24, Style::Secondary, Content::PageLabel).centered();
            for i in 0..CONTACTS_PER_PAGE as u8 {
                slots.add(124, 96 + i as u16 * 92 + 28, 40, 28, Style::Primary, Content::ContactName(i));
            }
        }
        ScreenPage::CallRinging | ScreenPage::CallActive => {
            elements.push(round(Target::Back, 32, Icon::Back));
            if page == ScreenPage::CallRinging {
                elements.push(with(element(Target::Answer, Rect::new(292, 400, 200, 72), Look::Accent), Some(Icon::Phone), Some(Text::CallsAnswer)));
                elements.push(with(element(Target::HangUp, Rect::new(532, 400, 200, 72), Look::Danger), Some(Icon::PhoneOff), Some(Text::CallsDecline)));
            } else {
                elements.push(with(element(Target::HangUp, Rect::new(412, 400, 200, 72), Look::Danger), Some(Icon::PhoneOff), Some(Text::CallsHangUp)));
            }
            icons.push((Rect::new(464, 92, 96, 96), Icon::User, Some(Tint::Blue)));
            slots.add(272, 212, 20, 48, Style::Primary, Content::CallName).centered();
            slots.add(344, 288, 24, 28, Style::Secondary, Content::CallState).centered();
        }
        ScreenPage::Photo => {
            elements.push(round(Target::Back, 32, Icon::Back));
            for (i, label) in strings::TIMERS.into_iter().enumerate() {
                elements.push(with(element(Target::Timer(i as u8), Rect::new(100 + i as u16 * 136, 156, 124, 56), Look::Chip), None, Some(label)));
            }
            for (i, label) in strings::FLIGHTS.into_iter().enumerate() {
                elements.push(with(element(Target::Length(i as u8), Rect::new(100 + i as u16 * 136, 292, 124, 56), Look::Chip), None, Some(label)));
            }
            elements.push(with(element(Target::Take, Rect::new(100, 400, 444, 76), Look::Accent), Some(Icon::Cube), Some(Text::PhotoTake)));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::PhotoTitle, style: Style::Primary, bold: true, width: 888 });
            // Clear of the cube (from 712).
            labels.push(Label { x: 100, y: 116, size: 24, text: Text::PhotoTimer, style: Style::Secondary, bold: false, width: 600 });
            labels.push(Label { x: 100, y: 252, size: 24, text: Text::PhotoFlight, style: Style::Secondary, bold: false, width: 600 });
            // Clear of the chips (the last one ends at 632).
            icons.push((Rect::new(712, 140, 240, 240), Icon::Cube, Some(Tint::Violet)));
            slots.add(100, 504, 56, 28, Style::Accent, Content::PhotoStatus);
            elements.push(with(element(Target::PhotoMode, Rect::new(100, 488, 444, 56), Look::Row), None, None));
        }
        ScreenPage::HomeSix => {
            // Two rows of three: the first four as on the four-tile Home, then music and light.
            for (i, &(target, icon, title, tint)) in home_tiles(Pages::Six).iter().enumerate() {
                let (x, y) = (32 + (i as u16 % 3) * 324, 104 + (i as u16 / 3) * 176);
                let mut tile = with(element(target, Rect::new(x, y, 312, 160), Look::Tile), Some(icon), Some(title));
                tile.tint = Some(tint);
                elements.push(tile);
            }
            elements.push(round(Target::Close, 936, Icon::Close));
            icons.push((Rect::new(32, 24, 56, 56), Icon::Microphone, Some(Tint::Green)));
            icons.push((Rect::new(104, 28, 56, 48), Icon::Mark, None));
            slots.add(208, 40, 48, 24, Style::Accent, Content::Status);
            for i in 0..6u16 {
                slots.add(32 + (i % 3) * 324 + 116, 104 + (i / 3) * 176 + 92, 15, 24, Style::Secondary, Content::TileNote(i as u8));
            }
            slots.add(32, 472, 64, 28, Style::Secondary, Content::Heard);
        }
        ScreenPage::Music => {
            elements.push(round(Target::Back, 32, Icon::Back));
            // Playback in the middle under the art and the texts, the speakers' mute at the left, the volume at the right.
            elements.push(with(element(Target::Mute, Rect::new(32, 400, 88, 72), Look::KeyDark), Some(Icon::SpeakerOff), None));
            elements.push(with(element(Target::MediaPrevious, Rect::new(316, 388, 96, 96), Look::Round), Some(Icon::SkipBack), None));
            elements.push(with(element(Target::MediaPlayPause, Rect::new(424, 388, 176, 96), Look::Accent), Some(Icon::PlayPause), None));
            elements.push(with(element(Target::MediaNext, Rect::new(612, 388, 96, 96), Look::Round), Some(Icon::SkipForward), None));
            elements.push(with(element(Target::VolumeDown, Rect::new(800, 400, 92, 72), Look::KeyDark), Some(Icon::SpeakerLow), None));
            elements.push(with(element(Target::VolumeUp, Rect::new(900, 400, 92, 72), Look::KeyDark), Some(Icon::SpeakerHigh), None));
            // Clear of Take back (from 496) and the playing video's Drop (from 720).
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::MusicTitle, style: Style::Primary, bold: true, width: 376 });
            pictures.push(Picture { rect: Rect::new(32, 104, 448, 252), result: 0 });
            slots.add(512, 112, 30, 32, Style::Primary, Content::MediaTitle);
            slots.add(512, 160, 40, 24, Style::Primary, Content::MediaArtist);
            slots.add(512, 196, 40, 24, Style::Secondary, Content::MediaAlbum);
            slots.add(512, 244, 40, 24, Style::Accent, Content::MediaState);
            slots.add(800, 484, 16, 24, Style::Secondary, Content::VolumeLabel).centered();
            // The world's video (screens from version 6): last, so the elements before keep their indices on older ones.
            elements.push(with(element(Target::WatchVideo, Rect::new(512, 316, 196, 48), Look::KeyDark), Some(Icon::Watch), Some(Text::MusicWatch)));
            elements.push(with(element(Target::SaveVideo, Rect::new(720, 316, 156, 48), Look::KeyDark), Some(Icon::Save), Some(Text::MusicSave)));
            slots.add(512, 280, 40, 24, Style::Secondary, Content::WorldVideo);
            // The YouTube video the computer plays (screens from version 6), in the heading's row: its Copy link, then
            // its Drop to the left once copied, as on a result's row.
            elements.push(with(element(Target::CopyPlaying, Rect::new(836, 28, 156, 48), Look::Accent), Some(Icon::Copy), Some(Text::LinkCopy)));
            elements.push(with(element(Target::DropPlaying, Rect::new(720, 28, 108, 48), Look::KeyDark), Some(Icon::Drop), Some(Text::LinkDrop)));
            // The timeline (screens from version 9), under the playback: its segments lit up to where the music is (they
            // are switched on, so they come first: elements 11 to 26 of the 28 the screen switches on), then Take back
            // in the heading's row (never on), then the places over the segments that a finger presses or drags along
            // (shown only when hovered); the time at either end.
            for i in 0..TIMELINE_STEPS as u16 {
                elements.push(element(Target::Timeline(i as u8), Rect::new(TIMELINE_X + i * TIMELINE_STEP, 532, TIMELINE_STEP - 4, 4), Look::Track));
            }
            elements.push(with(element(Target::TakeBack, Rect::new(496, 28, 212, 48), Look::KeyDark), Some(Icon::TakeBack), Some(Text::CardTakeBack)));
            for i in 0..TIMELINE_STEPS as u16 {
                elements.push(element(Target::Seek(i as u8), Rect::new(TIMELINE_X + i * TIMELINE_STEP, 512, TIMELINE_STEP, 44), Look::Row));
            }
            slots.add(32, 524, 7, 24, Style::Secondary, Content::MediaPosition).centered();
            slots.add(908, 524, 7, 24, Style::Secondary, Content::MediaDuration).centered();
        }
        ScreenPage::Light => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::LightSwitch, Rect::new(100, 112, 236, 64), Look::Chip), Some(Icon::Lightbulb), Some(Text::LightSwitch)));
            for (i, name) in strings::LIGHT_COLORS.into_iter().enumerate() {
                let (x, y) = (100 + (i as u16 % 4) * 136, 248 + (i as u16 / 4) * 68);
                let mut chip = with(element(Target::LightColor(i as u8), Rect::new(x, y, 124, 56), Look::Chip), None, Some(name));
                chip.tint = Some(Tint::Light(i as u8));
                elements.push(chip);
            }
            for (i, name) in strings::LIGHT_LEVELS.into_iter().enumerate() {
                elements.push(with(element(Target::LightLevel(i as u8), Rect::new(100 + i as u16 * 136, 428, 124, 56), Look::Chip), None, Some(name)));
            }
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::LightTitle, style: Style::Primary, bold: true, width: 888 });
            // Clear of the bulb (from 712).
            labels.push(Label { x: 100, y: 212, size: 24, text: Text::LightColour, style: Style::Secondary, bold: false, width: 600 });
            labels.push(Label { x: 100, y: 392, size: 24, text: Text::LightBrightness, style: Style::Secondary, bold: false, width: 600 });
            // Clear of the chips (the last colour ends at 632).
            icons.push((Rect::new(712, 140, 240, 240), Icon::Lightbulb, Some(Tint::Amber)));
            slots.add(356, 132, 28, 24, Style::Accent, Content::LightState);
            slots.add(100, 512, 72, 24, Style::Secondary, Content::LightNote);
        }
        ScreenPage::HomeSeven => {
            // Two rows of four: the six tiles of the six-tile Home in their order, then States; the eighth place holds
            // what N4N0 says. The tiles are as wide as the four-tile Home's: the icon at the top, the title and the note
            // under it.
            for (i, &(target, icon, title, tint)) in home_tiles(Pages::Seven).iter().enumerate() {
                let (x, y) = seven_cell(i);
                let mut tile = with(element(target, Rect::new(x, y, 228, 216), Look::Tile), Some(icon), Some(title));
                tile.tint = Some(tint);
                elements.push(tile);
            }
            elements.push(round(Target::Close, 936, Icon::Close));
            icons.push((Rect::new(32, 24, 56, 56), Icon::Microphone, Some(Tint::Green)));
            icons.push((Rect::new(104, 28, 56, 48), Icon::Mark, None));
            slots.add(208, 40, 48, 24, Style::Accent, Content::Status);
            for i in 0..home_tiles(Pages::Seven).len() {
                let (x, y) = seven_cell(i);
                slots.add(x + 24, y + 168, 15, 24, Style::Secondary, Content::TileNote(i as u8));
            }
            let (x, y) = seven_cell(home_tiles(Pages::Seven).len());
            boxes.push(Rect::new(x, y, 228, 216));
            slots.add(x + 60, y + 24, 12, 24, Style::Secondary, Content::HeardCaption);
            for line in 0..HEARD_LINES as u16 {
                slots.add(x + 24, y + 68 + line * 32, HEARD_COLUMNS as u8, 24, Style::Primary, Content::HeardLine(line as u8));
            }
        }
        ScreenPage::States => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::StateSave, Rect::new(616, 24, 176, 56), Look::Accent), Some(Icon::Plus), Some(Text::StatesNew)));
            elements.push(round(Target::Previous, 808, Icon::Previous));
            elements.push(round(Target::Next, 936, Icon::Next));
            for i in 0..STATES_PER_PAGE as u8 {
                elements.push(element(Target::StateApply(i), Rect::new(32, 96 + i as u16 * 92, 960, 84), Look::Row));
            }
            for i in 0..STATES_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92 + 20;
                elements.push(with(element(Target::StateApply(i), Rect::new(820, y, 156, 48), Look::Accent), Some(Icon::Apply), Some(Text::StatesApply)));
            }
            // The default state's mark, between its name and its Apply.
            for i in 0..STATES_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92 + 20;
                elements.push(with(element(Target::StateMark(i), Rect::new(656, y, 148, 48), Look::Chip), Some(Icon::Star), Some(Text::StatesDefault)));
            }
            // Up to the count beside it (from 232).
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::StatesTitle, style: Style::Primary, bold: true, width: 120 });
            slots.add(232, 40, 30, 24, Style::Primary, Content::StatesTitle);
            slots.add(868, 40, 5, 24, Style::Secondary, Content::PageLabel).centered();
            for i in 0..STATES_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92;
                slots.add(60, y + 12, 34, 28, Style::Primary, Content::StateName(i));
                slots.add(60, y + 48, 30, 24, Style::Secondary, Content::StateMeta(i));
            }
        }
        ScreenPage::Card => {
            // The thumbnail at the top left; beside it the title and the link; under them, across the card, what to do
            // with it. The card stays out (screen version 7: an object of its own), so it counts no time down.
            pictures.push(Picture { rect: Rect::new(32, 32, 512, 288), result: 0 });
            slots.add(576, 40, 26, 32, Style::Primary, Content::CardTitle(0));
            slots.add(576, 84, 26, 32, Style::Primary, Content::CardTitle(1));
            slots.add(576, 140, 34, 24, Style::Accent, Content::CardLink);
            icons.push((Rect::new(240, 400, 96, 96), Icon::Grab, Some(Tint::Green)));
            labels.push(Label { x: 364, y: 408, size: 48, text: Text::CardGrabToCopy, style: Style::Primary, bold: true, width: 628 });
            labels.push(Label { x: 364, y: 472, size: 24, text: Text::CardAnyoneCanTake, style: Style::Secondary, bold: false, width: 628 });
        }
        ScreenPage::HomeTen => {
            // Three rows of four: the nine features, then what N4N0 says across the last row's three other places; the
            // versions at the bottom right, under them.
            for (i, &(target, icon, title, tint)) in home_tiles(Pages::Ten).iter().enumerate() {
                let (x, y) = ten_cell(i);
                let mut tile = with(element(target, Rect::new(x, y, 228, 140), Look::Tile), Some(icon), Some(title));
                tile.tint = Some(tint);
                elements.push(tile);
            }
            elements.push(round(Target::Close, 936, Icon::Close));
            icons.push((Rect::new(32, 24, 56, 56), Icon::Microphone, Some(Tint::Green)));
            icons.push((Rect::new(104, 28, 56, 48), Icon::Mark, None));
            slots.add(208, 40, 48, 24, Style::Accent, Content::Status);
            for i in 0..HOME_TILES.len() {
                let (x, y) = ten_cell(i);
                slots.add(x + 20, y + 96, 14, 24, Style::Secondary, Content::TileNote(i as u8));
            }
            let (x, y) = ten_cell(HOME_TILES.len());
            boxes.push(Rect::new(x, y, 3 * 228 + 2 * 16, 140));
            slots.add(x + 84, y + 20, 12, 24, Style::Secondary, Content::HeardCaption);
            for line in 0..HEARD_WIDE_LINES as u16 {
                slots.add(x + 84, y + 52 + line * 28, HEARD_WIDE_COLUMNS as u8, 24, Style::Primary, Content::HeardWide(line as u8));
            }
            slots.add(672, 548, 40, 16, Style::Secondary, Content::Versions);
        }
        ScreenPage::TypeQwerty | ScreenPage::TypeAzerty | ScreenPage::TypeSymbols => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::Done, Rect::new(824, 24, 168, 56), Look::Accent), Some(Icon::Check), Some(Text::TypeDone)));
            elements.push(element(Target::Field, Rect::new(32, 96, 960, 208), Look::Area));
            let layout = if page == ScreenPage::TypeAzerty { KeyboardLayout::Azerty } else { KeyboardLayout::Qwerty };
            keyboard(&mut elements, page, layout, (Target::Done, Icon::Check, Text::TypeDone));
            slots.add(104, 40, 52, 24, Style::Primary, Content::TypeTitle);
            for line in 0..TYPE_LINES as u16 {
                slots.add(56, 116 + line * 44, TYPE_COLUMNS as u8, 24, Style::Primary, Content::TypeLine(line as u8));
            }
        }
        ScreenPage::NewState => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::DraftName, Rect::new(100, 136, 824, 64), Look::Field), Some(Icon::Keyboard), None));
            elements.push(with(element(Target::DraftSize, Rect::new(100, 276, 236, 64), Look::Chip), Some(Icon::Size), Some(Text::NewStateSize)));
            elements.push(with(element(Target::DraftPose, Rect::new(352, 276, 236, 64), Look::Chip), Some(Icon::Pose), Some(Text::NewStatePose)));
            elements.push(with(element(Target::DraftDefault, Rect::new(604, 276, 236, 64), Look::Chip), Some(Icon::Star), Some(Text::NewStateDefault)));
            elements.push(with(element(Target::DraftSave, Rect::new(100, 444, 300, 76), Look::Accent), Some(Icon::Save), Some(Text::NewStateSave)));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::NewStateTitle, style: Style::Primary, bold: true, width: 888 });
            labels.push(Label { x: 100, y: 100, size: 24, text: Text::NewStateName, style: Style::Secondary, bold: false, width: 824 });
            labels.push(Label { x: 100, y: 240, size: 24, text: Text::NewStateOptions, style: Style::Secondary, bold: false, width: 824 });
            slots.add(164, 156, super::STATE_NAME_MAX as u8 + 1, 24, Style::Primary, Content::DraftName);
            slots.add(100, 372, 70, 24, Style::Secondary, Content::DraftNote);
        }
        ScreenPage::Tracking => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::TrackingSwitch, Rect::new(100, 104, 300, 64), Look::Chip), Some(Icon::Tracking), Some(Text::TrackingSmoothing)));
            for (k, _) in TRACKING_KINDS.iter().enumerate() {
                for (i, label) in strings::SMOOTHING.into_iter().enumerate() {
                    let rect = Rect::new(340 + i as u16 * 136, 200 + k as u16 * 92, 124, 56);
                    elements.push(with(element(Target::Smoothing((k * 4 + i) as u8), rect, Look::Chip), None, Some(label)));
                }
            }
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::TrackingTitle, style: Style::Primary, bold: true, width: 888 });
            for (k, kind) in strings::TRACKING_KINDS.into_iter().enumerate() {
                // Up to the strengths (from 340).
                labels.push(Label { x: 100, y: 214 + k as u16 * 92, size: 28, text: kind, style: Style::Primary, bold: false, width: 228 });
            }
            slots.add(424, 124, 46, 24, Style::Accent, Content::TrackingState);
            slots.add(100, 492, 72, 24, Style::Secondary, Content::TrackingNote);
        }
        ScreenPage::Suggest => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(element(Target::SuggestType, Rect::new(32, 96, 960, 196), Look::Area));
            elements.push(with(element(Target::SuggestType, Rect::new(32, 316, 220, 72), Look::KeyDark), Some(Icon::Keyboard), Some(Text::SuggestType)));
            elements.push(with(element(Target::SuggestTalk, Rect::new(268, 316, 220, 72), Look::KeyDark), Some(Icon::Microphone), Some(Text::SuggestTalk)));
            elements.push(with(element(Target::SuggestSend, Rect::new(772, 316, 220, 72), Look::Accent), Some(Icon::Send), Some(Text::SuggestSend)));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::SuggestTitle, style: Style::Primary, bold: true, width: 888 });
            labels.push(Label { x: 32, y: 488, size: 24, text: Text::SuggestHint, style: Style::Secondary, bold: false, width: 960 });
            for line in 0..SUGGEST_LINES as u16 {
                slots.add(56, 116 + line * 40, SUGGEST_COLUMNS as u8, 24, Style::Primary, Content::SuggestLine(line as u8));
            }
            slots.add(32, 420, 60, 24, Style::Accent, Content::SuggestNote);
        }
        ScreenPage::HomeSimple | ScreenPage::HomeQuiet | ScreenPage::HomeSimpleStay | ScreenPage::HomeQuietStay => {
            // Three large tiles (YouTube, 3D capture, More), three buttons under them (fast walk, calls, states), and at
            // the bottom what the computer plays; with nothing playing the tiles and buttons take its place.
            let quiet = matches!(page, ScreenPage::HomeQuiet | ScreenPage::HomeQuietStay);
            let (tile_h, buttons_y, buttons_h) = if quiet { (340, 452, 100) } else { (216, 328, 72) };
            for (i, &(target, icon, title, tint)) in SIMPLE_TILES.iter().enumerate() {
                let mut tile = with(element(target, Rect::new(32 + i as u16 * 324, 96, 312, tile_h), Look::Tile), Some(icon), Some(title));
                tile.tint = Some(tint);
                elements.push(tile);
            }
            // Switched on (fast walk) and greyed out (fast walk in desktop mode) among the first elements, as the screen
            // keeps them.
            for (i, &(target, icon, label)) in SIMPLE_BUTTONS.iter().enumerate() {
                elements.push(with(element(target, Rect::new(32 + i as u16 * 324, buttons_y, 312, buttons_h), Look::Chip), Some(icon), Some(label)));
            }
            elements.push(round(Target::Close, 936, Icon::Close));
            // Switched on while the screen stays where it is: its ring is the state at a glance, on every page that has it.
            if matches!(page, ScreenPage::HomeSimpleStay | ScreenPage::HomeQuietStay) {
                elements.push(with(element(Target::StayHere, Rect::new(812, 28, 116, 48), Look::Chip), None, Some(Text::HomeStay)));
            }
            icons.push((Rect::new(32, 24, 56, 56), Icon::Microphone, Some(Tint::Green)));
            icons.push((Rect::new(104, 28, 56, 48), Icon::Mark, None));
            slots.add(208, 28, 48, 24, Style::Accent, Content::Status);
            slots.add(208, 56, SIMPLE_HEARD_COLUMNS as u8, 20, Style::Secondary, Content::Heard);
            for (i, &(target, ..)) in SIMPLE_TILES.iter().enumerate() {
                let note = match target {
                    Target::More => NOTE_MORE,
                    _ => HOME_TILES.iter().position(|t| t.0 == target).unwrap_or(0) as u8,
                };
                slots.add(32 + i as u16 * 324 + 24, 96 + tile_h - 48, 22, 24, Style::Secondary, Content::TileNote(note));
            }
            if !quiet {
                // What plays: the whole strip opens Music; its art, title and artist, and the playback beside them.
                boxes.push(Rect::new(32, 416, 960, 136));
                elements.push(element(Target::Music, Rect::new(32, 416, 960, 136), Look::Row));
                elements.push(with(element(Target::MediaPrevious, Rect::new(756, 456, 64, 64), Look::Round), Some(Icon::SkipBack), None));
                elements.push(with(element(Target::MediaPlayPause, Rect::new(828, 448, 80, 80), Look::Accent), Some(Icon::PlayPause), None));
                elements.push(with(element(Target::MediaNext, Rect::new(916, 456, 64, 64), Look::Round), Some(Icon::SkipForward), None));
                pictures.push(Picture { rect: Rect::new(48, 432, 192, 108), result: 0 });
                slots.add(264, 444, 34, 28, Style::Primary, Content::MediaTitle);
                slots.add(264, 488, 40, 24, Style::Secondary, Content::MediaArtist);
            }
        }
        ScreenPage::More => {
            // Two rows of two wide cards: the icon at the left, the title and the note beside it.
            elements.push(round(Target::Back, 32, Icon::Back));
            for (i, &(target, icon, title, tint)) in MORE_CARDS.iter().enumerate() {
                let (x, y) = (32 + (i as u16 % 2) * 488, 96 + (i as u16 / 2) * 216);
                let mut card = with(element(target, Rect::new(x, y, 472, 200), Look::Tile), Some(icon), Some(title));
                card.tint = Some(tint);
                elements.push(card);
                let note = match target {
                    Target::Instances => NOTE_INSTANCES,
                    _ => HOME_TILES.iter().position(|t| t.0 == target).unwrap_or(0) as u8,
                };
                slots.add(x + 116, y + 92, 27, 24, Style::Secondary, Content::TileNote(note));
            }
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::MoreTitle, style: Style::Primary, bold: true, width: 800 });
            slots.add(672, 548, 40, 16, Style::Secondary, Content::Versions);
        }
        ScreenPage::MoreQr => {
            // Two columns of three rows of wide cards (the last place, at the bottom right, is the versions'): the icon at
            // the left, the title and the note beside it.
            elements.push(round(Target::Back, 32, Icon::Back));
            for (i, &(target, icon, title, tint)) in MORE_CARDS_QR.iter().enumerate() {
                let (x, y) = (32 + (i as u16 % 2) * 488, 96 + (i as u16 / 2) * 148);
                let mut card = with(element(target, Rect::new(x, y, 472, 136), Look::Tile), Some(icon), Some(title));
                card.tint = Some(tint);
                elements.push(card);
                slots.add(x + 116, y + 84, 27, 24, Style::Secondary, Content::TileNote(note_of(target)));
            }
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::MoreTitle, style: Style::Primary, bold: true, width: 800 });
            slots.add(672, 548, 40, 16, Style::Secondary, Content::Versions);
        }
        ScreenPage::Qr => {
            // The scanner's picture with what it does and how beside it, then the last link it opened and the buttons.
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::QrCopy, Rect::new(100, 396, 252, 72), Look::KeyDark), Some(Icon::Copy), Some(Text::QrCopy)));
            elements.push(with(element(Target::QrOpen, Rect::new(368, 396, 252, 72), Look::KeyDark), Some(Icon::Open), Some(Text::QrOpen)));
            elements.push(with(element(Target::QrAgain, Rect::new(636, 396, 288, 72), Look::Accent), Some(Icon::Qr), Some(Text::QrAgain)));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::QrTitle, style: Style::Primary, bold: true, width: 888 });
            labels.push(Label { x: 100, y: 300, size: 20, text: Text::QrLast, style: Style::Secondary, bold: false, width: 600 });
            icons.push((Rect::new(100, 112, 112, 112), Icon::Qr, Some(Tint::Green)));
            slots.add(236, 116, 42, 32, Style::Primary, Content::QrState);
            for i in 0..QR_NOTE_LINES as u8 {
                slots.add(236, 164 + i as u16 * 32, QR_NOTE_COLUMNS as u8, 24, Style::Secondary, Content::QrNote(i));
            }
            slots.add(100, 332, QR_LINK_COLUMNS as u8, 28, Style::Accent, Content::QrLink);
            slots.add(100, 500, 76, 20, Style::Accent, Content::QrNotice);
        }
        ScreenPage::PhotoShare => {
            // The 3D picture page with, at its right, the last capture: its picture, world, where it stands, its shares;
            // History at the top right.
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::History, Rect::new(820, 28, 172, 48), Look::KeyDark), Some(Icon::History), Some(Text::HistoryOpen)));
            for (i, label) in strings::TIMERS.into_iter().enumerate() {
                elements.push(with(element(Target::Timer(i as u8), Rect::new(100 + i as u16 * 136, 156, 124, 56), Look::Chip), None, Some(label)));
            }
            for (i, label) in strings::FLIGHTS.into_iter().enumerate() {
                elements.push(with(element(Target::Length(i as u8), Rect::new(100 + i as u16 * 136, 292, 124, 56), Look::Chip), None, Some(label)));
            }
            elements.push(with(element(Target::Take, Rect::new(100, 400, 444, 76), Look::Accent), Some(Icon::Cube), Some(Text::PhotoTake)));
            elements.push(with(element(Target::ShareLatest, Rect::new(672, 436, 320, 56), Look::Accent), Some(Icon::Public), Some(Text::SharePublic)));
            elements.push(with(element(Target::ShareLatestTo, Rect::new(672, 500, 320, 56), Look::KeyDark), Some(Icon::Friends), Some(Text::ShareTo)));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::PhotoTitle, style: Style::Primary, bold: true, width: 700 });
            labels.push(Label { x: 100, y: 116, size: 24, text: Text::PhotoTimer, style: Style::Secondary, bold: false, width: 540 });
            labels.push(Label { x: 100, y: 252, size: 24, text: Text::PhotoFlight, style: Style::Secondary, bold: false, width: 540 });
            labels.push(Label { x: 672, y: 100, size: 20, text: Text::ShareLast, style: Style::Secondary, bold: false, width: 320 });
            pictures.push(Picture { rect: Rect::new(672, 128, 320, 180), result: 0 });
            // Clear of the last capture's column (from 672).
            slots.add(100, 504, 40, 28, Style::Accent, Content::PhotoStatus);
            elements.push(with(element(Target::PhotoMode, Rect::new(100, 488, 444, 56), Look::Row), None, None));
            slots.add(672, 320, 22, 28, Style::Primary, Content::LastWorld);
            slots.add(672, 352, 26, 24, Style::Accent, Content::LastState);
            for i in 0..LAST_NOTE_LINES as u8 {
                slots.add(672, 384 + i as u16 * 24, LAST_NOTE_COLUMNS as u8, 20, Style::Secondary, Content::LastNote(i));
            }
        }
        ScreenPage::PhotoTwo => {
            // The two pictures large, as on the Capture page and the window's Home, what they take with under them, and the
            // last capture's column as page 30 has it. Each button takes its picture: nothing here says which is chosen.
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::History, Rect::new(820, 28, 172, 48), Look::KeyDark), Some(Icon::History), Some(Text::HistoryOpen)));
            for (i, (mode, icon, title, note, tint)) in [
                (PhotoMode::People, Icon::People, Text::PhotoPeople, Text::PhotoPeopleNote, Tint::Violet),
                (PhotoMode::World, Icon::World, Text::PhotoWorld, Text::PhotoWorldNote, Tint::Green),
            ]
            .into_iter()
            .enumerate()
            {
                let mut card = with(element(Target::TakeOf(mode), Rect::new(32, 96 + i as u16 * 124, 612, 112), Look::Tile), Some(icon), Some(title));
                card.tint = Some(tint);
                card.note = Some(note);
                elements.push(card);
            }
            // A label and its chips on one line: the four timers and three flights share their columns.
            for (i, label) in strings::TIMERS.into_iter().enumerate() {
                elements.push(with(element(Target::Timer(i as u8), Rect::new(240 + i as u16 * 104, 348, 92, 52), Look::Chip), None, Some(label)));
            }
            for (i, label) in strings::FLIGHTS.into_iter().enumerate() {
                elements.push(with(element(Target::Length(i as u8), Rect::new(240 + i as u16 * 104, 412, 92, 52), Look::Chip), None, Some(label)));
            }
            elements.push(with(element(Target::ShareLatest, Rect::new(672, 436, 320, 56), Look::Accent), Some(Icon::Public), Some(Text::SharePublic)));
            elements.push(with(element(Target::ShareLatestTo, Rect::new(672, 500, 320, 56), Look::KeyDark), Some(Icon::Friends), Some(Text::ShareTo)));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::PhotoTitle, style: Style::Primary, bold: true, width: 700 });
            labels.push(Label { x: 32, y: 360, size: 24, text: Text::PhotoTimer, style: Style::Secondary, bold: false, width: 204 });
            labels.push(Label { x: 32, y: 424, size: 24, text: Text::PhotoFlight, style: Style::Secondary, bold: false, width: 204 });
            labels.push(Label { x: 672, y: 100, size: 20, text: Text::ShareLast, style: Style::Secondary, bold: false, width: 320 });
            pictures.push(Picture { rect: Rect::new(672, 128, 320, 180), result: 0 });
            // What stops or warns about a picture, and a timer's count: clear of the last capture's column.
            slots.add(32, 488, 43, 28, Style::Accent, Content::PhotoStatus);
            slots.add(672, 320, 22, 28, Style::Primary, Content::LastWorld);
            slots.add(672, 352, 26, 24, Style::Accent, Content::LastState);
            for i in 0..LAST_NOTE_LINES as u8 {
                slots.add(672, 384 + i as u16 * 24, LAST_NOTE_COLUMNS as u8, 20, Style::Secondary, Content::LastNote(i));
            }
        }
        ScreenPage::History => {
            // The results' list: each capture's picture, world, when and where it stands, then its two shares.
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(round(Target::Previous, 808, Icon::Previous));
            elements.push(round(Target::Next, 936, Icon::Next));
            for i in 0..CAPTURES_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92 + 20;
                elements.push(with(element(Target::SharePublic(i), Rect::new(604, y, 196, 48), Look::Accent), Some(Icon::Public), Some(Text::SharePublic)));
            }
            for i in 0..CAPTURES_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92 + 20;
                elements.push(with(element(Target::ShareTo(i), Rect::new(808, y, 168, 48), Look::KeyDark), Some(Icon::Friends), Some(Text::ShareTo)));
            }
            // Up to how many there are (from 316).
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::HistoryTitle, style: Style::Primary, bold: true, width: 196 });
            slots.add(316, 40, 30, 24, Style::Secondary, Content::HistoryTitle);
            slots.add(868, 40, 5, 24, Style::Secondary, Content::PageLabel).centered();
            for i in 0..CAPTURES_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92;
                pictures.push(Picture { rect: Rect::new(44, y + 4, 128, 72), result: i });
                slots.add(192, y + 12, 28, 28, Style::Primary, Content::CaptureWorld(i));
                slots.add(192, y + 48, 33, 24, Style::Secondary, Content::CaptureMeta(i));
            }
        }
        ScreenPage::ShareTo => {
            // The friends, a row each (their mark at its right while chosen), Find a friend and Send at the bottom.
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(round(Target::Previous, 808, Icon::Previous));
            elements.push(round(Target::Next, 936, Icon::Next));
            for i in 0..FRIENDS_PER_PAGE as u8 {
                elements.push(with(element(Target::Friend(i), Rect::new(32, 96 + i as u16 * 80, 960, 72), Look::Row), Some(Icon::User), None));
            }
            for i in 0..FRIENDS_PER_PAGE as u8 {
                // Chosen: a green check, as the desktop's chosen friends.
                elements.push(with(element(Target::FriendMark(i), Rect::new(912, 104 + i as u16 * 80, 56, 56), Look::Accent), Some(Icon::Check), None));
            }
            elements.push(with(element(Target::FindFriend, Rect::new(32, 500, 280, 56), Look::KeyDark), Some(Icon::Search), Some(Text::FriendsFind)));
            elements.push(with(element(Target::SendShare, Rect::new(752, 500, 240, 56), Look::Accent), Some(Icon::Send), Some(Text::FriendsSend)));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::FriendsTitle, style: Style::Primary, bold: true, width: 400 });
            slots.add(520, 40, 22, 24, Style::Secondary, Content::FriendsTitle);
            slots.add(868, 40, 5, 24, Style::Secondary, Content::PageLabel).centered();
            for i in 0..FRIENDS_PER_PAGE as u8 {
                slots.add(124, 96 + i as u16 * 80 + 20, 40, 28, Style::Primary, Content::ShareFriend(i));
            }
            slots.add(328, 516, 34, 24, Style::Secondary, Content::FriendsNote);
        }
        ScreenPage::Explicit => {
            // The capture's picture, its world and why it is flagged, the warning under them, Cancel and Share anyway.
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::CancelShare, Rect::new(100, 440, 360, 72), Look::KeyDark), Some(Icon::Close), Some(Text::ExplicitCancel)));
            elements.push(with(element(Target::ShareAnyway, Rect::new(476, 440, 448, 72), Look::Accent), Some(Icon::Public), Some(Text::ExplicitAnyway)));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::ExplicitTitle, style: Style::Primary, bold: true, width: 888 });
            pictures.push(Picture { rect: Rect::new(100, 112, 320, 180), result: 0 });
            icons.push((Rect::new(100, 320, 48, 48), Icon::Warning, Some(Tint::Amber)));
            slots.add(452, 120, 38, 28, Style::Primary, Content::ExplicitWorld);
            slots.add(452, 160, 44, 24, Style::Secondary, Content::ExplicitState);
            for i in 0..EXPLICIT_WHY_LINES as u8 {
                slots.add(452, 200 + i as u16 * 28, EXPLICIT_WHY_COLUMNS as u8, 20, Style::Secondary, Content::ExplicitWhy(i));
            }
            for i in 0..EXPLICIT_LINES as u8 {
                slots.add(164, 316 + i as u16 * 36, EXPLICIT_COLUMNS as u8, 28, Style::Primary, Content::ExplicitLine(i));
            }
        }
        ScreenPage::Feedback => {
            // Like's card and the labels people gave most at the left (switched on: the player's), the newest comment's
            // panel, the labels and comments buttons and Write a comment at the right, the worlds to step through at the
            // top right.
            let mut like = with(element(Target::Like, Rect::new(32, 96, 472, 112), Look::Tile), Some(Icon::Heart), Some(Text::FeedbackLike));
            like.tint = Some(Tint::Pink);
            elements.push(like);
            for i in 0..TOP_LABELS as u16 {
                elements.push(element(Target::TopLabel(i as u8), Rect::new(32 + (i % 2) * 240, 256 + (i / 2) * 64, 232, 56), Look::Chip));
            }
            elements.push(element(Target::Comments, Rect::new(520, 96, 472, 240), Look::Area));
            elements.push(with(element(Target::Labels, Rect::new(520, 352, 232, 56), Look::KeyDark), Some(Icon::Tag), Some(Text::FeedbackLabels)));
            elements.push(with(element(Target::Comments, Rect::new(760, 352, 232, 56), Look::KeyDark), Some(Icon::Chat), Some(Text::FeedbackComments)));
            elements.push(with(element(Target::WriteComment, Rect::new(520, 424, 472, 64), Look::Accent), Some(Icon::Pencil), Some(Text::FeedbackWrite)));
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(round(Target::Previous, 808, Icon::Previous));
            elements.push(round(Target::Next, 936, Icon::Next));
            labels.push(Label { x: 32, y: 224, size: 20, text: Text::FeedbackPeople, style: Style::Secondary, bold: false, width: 472 });
            slots.add(104, 28, 30, 28, Style::Primary, Content::FeedbackWorld);
            slots.add(104, 60, 34, 20, Style::Secondary, Content::FeedbackWhy);
            slots.add(868, 40, 5, 24, Style::Secondary, Content::PageLabel).centered();
            slots.add(148, 168, 22, 24, Style::Secondary, Content::LikeNote);
            for i in 0..TOP_LABELS as u16 {
                slots.add(52 + (i % 2) * 240, 272 + (i / 2) * 64, 17, 24, Style::Primary, Content::TopChip(i as u8));
            }
            slots.add(544, 116, 18, 24, Style::Accent, Content::CommentsCount);
            slots.add(544, 156, 30, 20, Style::Secondary, Content::NewestMeta);
            for i in 0..NEWEST_LINES as u16 {
                slots.add(544, 192 + i * 36, NEWEST_COLUMNS as u8, 24, Style::Primary, Content::NewestLine(i as u8));
            }
            slots.add(32, 512, 48, 24, Style::Accent, Content::FeedbackNote);
        }
        ScreenPage::FeedbackLabels => {
            // Every label in three columns of six (switched on: the player's), the world's name by the title.
            for (i, &text) in LABEL_TEXTS.iter().enumerate() {
                let i = i as u16;
                elements.push(with(element(Target::LabelChip(i as u8), Rect::new(32 + (i % 3) * 324, 96 + (i / 3) * 64, 312, 56), Look::Chip), None, Some(text)));
            }
            elements.push(round(Target::Back, 32, Icon::Back));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::FeedbackLabelsTitle, style: Style::Primary, bold: true, width: 260 });
            slots.add(380, 40, 40, 24, Style::Secondary, Content::LabelsWorld);
            slots.add(32, 500, 76, 24, Style::Accent, Content::LabelsNote);
        }
        ScreenPage::Comments => {
            // Three comments a page, each on its panel, Delete beside the player's own (on the page itself: the screen covers a
            // hidden element with the page's colour), Write a comment under them.
            for i in 0..COMMENTS_PER_PAGE as u16 {
                elements.push(element(Target::CommentCard(i as u8), Rect::new(32, 96 + i * 128, 768, 120), Look::Area));
            }
            for i in 0..COMMENTS_PER_PAGE as u16 {
                elements.push(with(element(Target::DeleteComment(i as u8), Rect::new(816, 128 + i * 128, 176, 56), Look::KeyDark), Some(Icon::Trash), Some(Text::FeedbackDelete)));
            }
            elements.push(with(element(Target::WriteComment, Rect::new(32, 488, 400, 64), Look::Accent), Some(Icon::Pencil), Some(Text::FeedbackWrite)));
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(round(Target::Previous, 808, Icon::Previous));
            elements.push(round(Target::Next, 936, Icon::Next));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::FeedbackCommentsTitle, style: Style::Primary, bold: true, width: 300 });
            slots.add(420, 40, 30, 24, Style::Secondary, Content::CommentsTitle);
            slots.add(868, 40, 5, 24, Style::Secondary, Content::PageLabel).centered();
            for i in 0..COMMENTS_PER_PAGE as u16 {
                let y = 96 + i * 128;
                slots.add(56, y + 16, 22, 20, Style::Secondary, Content::CommentMeta(i as u8));
                for line in 0..COMMENT_LINES as u16 {
                    slots.add(56, y + 44 + line * 36, COMMENT_COLUMNS as u8, 28, Style::Primary, Content::CommentLine((i * COMMENT_LINES as u16 + line) as u8));
                }
            }
        }
        ScreenPage::MoreMirror => {
            // As More with World feedback, the screen mirror's card beside it and suggest alone in the last row.
            elements.push(round(Target::Back, 32, Icon::Back));
            for (i, &(target, icon, title, tint)) in MORE_CARDS_MIRROR.iter().enumerate() {
                let (x, y) = (32 + (i as u16 % 2) * 488, 96 + (i as u16 / 2) * 112);
                let mut card = with(element(target, Rect::new(x, y, 472, 100), Look::Tile), Some(icon), Some(title));
                card.tint = Some(tint);
                elements.push(card);
                slots.add(x + 116, y + 64, 27, 24, Style::Secondary, Content::TileNote(note_of(target)));
            }
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::MoreTitle, style: Style::Primary, bold: true, width: 800 });
            slots.add(672, 548, 40, 16, Style::Secondary, Content::Versions);
        }
        ScreenPage::Mirror => {
            // The camera's picture large at the left (its owner's screen draws it there), the camera's buttons beside it,
            // where it is and a note under it.
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::MirrorAim, Rect::new(696, 96, 296, 76), Look::Accent), Some(Icon::Aim), Some(Text::MirrorAim)));
            elements.push(with(element(Target::MirrorZoomIn, Rect::new(696, 188, 296, 64), Look::KeyDark), Some(Icon::Plus), Some(Text::MirrorZoomIn)));
            elements.push(with(element(Target::MirrorZoomOut, Rect::new(696, 264, 296, 64), Look::KeyDark), Some(Icon::Minus), Some(Text::MirrorZoomOut)));
            elements.push(with(element(Target::MirrorStop, Rect::new(696, 380, 296, 76), Look::Danger), Some(Icon::Stop), Some(Text::MirrorStop)));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::MirrorTitle, style: Style::Primary, bold: true, width: 568 });
            pictures.push(Picture { rect: MIRROR_PICTURE, result: 0 });
            slots.add(32, 472, 46, 28, Style::Accent, Content::MirrorState);
            slots.add(32, 516, MIRROR_NOTE_COLUMNS as u8, 20, Style::Secondary, Content::MirrorNote);
        }
        ScreenPage::MoreFeedback => {
            // Two columns of four rows of wide cards (the last row left for more): the icon at the left, the title and
            // the note beside it.
            elements.push(round(Target::Back, 32, Icon::Back));
            for (i, &(target, icon, title, tint)) in MORE_CARDS_FEEDBACK.iter().enumerate() {
                let (x, y) = (32 + (i as u16 % 2) * 488, 96 + (i as u16 / 2) * 112);
                let mut card = with(element(target, Rect::new(x, y, 472, 100), Look::Tile), Some(icon), Some(title));
                card.tint = Some(tint);
                elements.push(card);
                slots.add(x + 116, y + 64, 27, 24, Style::Secondary, Content::TileNote(note_of(target)));
            }
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::MoreTitle, style: Style::Primary, bold: true, width: 800 });
            slots.add(672, 548, 40, 16, Style::Secondary, Content::Versions);
        }
        ScreenPage::Instances => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(round(Target::Previous, 808, Icon::Previous));
            elements.push(round(Target::Next, 936, Icon::Next));
            for i in 0..INSTANCES_PER_PAGE as u8 {
                elements.push(element(Target::InviteMe(i), Rect::new(32, 96 + i as u16 * 92, 960, 84), Look::Row));
            }
            for i in 0..INSTANCES_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92 + 20;
                elements.push(with(element(Target::InviteMe(i), Rect::new(784, y, 192, 48), Look::Accent), Some(Icon::Send), Some(Text::InstancesInvite)));
            }
            // Up to how many there are (from 316).
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::InstancesTitle, style: Style::Primary, bold: true, width: 196 });
            slots.add(316, 40, 30, 24, Style::Primary, Content::InstancesTitle);
            slots.add(868, 40, 5, 24, Style::Secondary, Content::PageLabel).centered();
            for i in 0..INSTANCES_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92;
                slots.add(60, y + 12, 32, 28, Style::Primary, Content::InstanceWorld(i));
                slots.add(60, y + 48, 32, 24, Style::Secondary, Content::InstanceMeta(i));
            }
        }
        ScreenPage::AddFriend => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::CopyFriendName, Rect::new(100, 300, 252, 72), Look::KeyDark), Some(Icon::Copy), Some(Text::FriendCopy)));
            elements.push(with(element(Target::OpenFriendLink, Rect::new(368, 300, 252, 72), Look::KeyDark), Some(Icon::Open), Some(Text::FriendOpen)));
            elements.push(with(element(Target::InviteAgain, Rect::new(636, 300, 288, 72), Look::Accent), Some(Icon::Send), Some(Text::FriendRetry)));
            labels.push(Label { x: 104, y: 34, size: 32, text: Text::FriendTitle, style: Style::Primary, bold: true, width: 888 });
            labels.push(Label { x: 100, y: 104, size: 24, text: Text::FriendWhy, style: Style::Secondary, bold: false, width: 824 });
            labels.push(Label { x: 100, y: 404, size: 24, text: Text::FriendHow, style: Style::Secondary, bold: false, width: 824 });
            icons.push((Rect::new(100, 156, 96, 96), Icon::User, Some(Tint::Blue)));
            slots.add(220, 164, 28, 48, Style::Primary, Content::FriendName);
            slots.add(220, 228, FRIEND_LINK_COLUMNS as u8, 24, Style::Accent, Content::FriendLink);
            slots.add(100, 456, 76, 20, Style::Accent, Content::FriendNote);
        }
    }
    Layout { page, elements, slots: slots.slots, pictures, labels, icons, boxes }
}

/// What N4N0 says on the simple Home, under the status line: one line of so many characters at a time.
pub const SIMPLE_HEARD_COLUMNS: usize = 60;
/// Orbiters' account's page on Add Orbiters as a friend, in characters.
const FRIEND_LINK_COLUMNS: usize = 56;
/// The QR code scanner's lines of how (or why it stopped), and its last link, in characters.
pub const QR_NOTE_LINES: usize = 3;
pub const QR_NOTE_COLUMNS: usize = 56;
const QR_LINK_COLUMNS: usize = 58;
/// The 3D picture page's lines under its last capture's state, the explicit warning's lines of why a capture is flagged
/// and of the warning itself, in characters.
pub const LAST_NOTE_LINES: usize = 2;
pub const LAST_NOTE_COLUMNS: usize = 32;
pub const EXPLICIT_WHY_LINES: usize = 2;
pub const EXPLICIT_WHY_COLUMNS: usize = 54;
pub const EXPLICIT_LINES: usize = 2;
pub const EXPLICIT_COLUMNS: usize = 58;
/// World feedback's newest comment, and each comment on Comments: lines of so many characters (the last ending in "…"
/// when there is more).
pub const NEWEST_LINES: usize = 2;
pub const NEWEST_COLUMNS: usize = 34;
pub const COMMENT_LINES: usize = 2;
pub const COMMENT_COLUMNS: usize = 42;

/// `text` in at most `lines` lines of `cols` characters, cut between words, the last ending in "…" when there is more.
pub fn wrapped(text: &str, cols: usize, lines: usize) -> Vec<String> {
    let mut out = super::pieces(&shown(text), cols);
    if out.len() > lines {
        out.truncate(lines);
        let last = out.pop().unwrap_or_default();
        out.push(ellipsis(&format!("{last} …"), cols));
    }
    out
}

/// The seven-tile Home's place `i` (0 to 7): two rows of four, 228 × 216 each, 16 apart.
fn seven_cell(i: usize) -> (u16, u16) {
    (32 + (i as u16 % 4) * 244, 104 + (i as u16 / 4) * 232)
}

/// What N4N0 says on the seven-tile Home: lines of so many characters, so many at a time.
pub const HEARD_COLUMNS: usize = 15;
pub const HEARD_LINES: usize = 4;
/// A dropped card's title lines and its link, in characters.
const CARD_TITLE_COLUMNS: usize = 26;
const CARD_LINK_COLUMNS: usize = 34;

/// The thumbnail a page's picture shows, by the key the hosts keep pictures under (a video id, the music's art key).
pub fn picture_key(menu: &Menu, picture: &Picture) -> Option<String> {
    picture_key_for(menu, picture, false)
}

/// The key of the thumbnail `picture` shows to the owner or, `public`, to everyone else: a capture flagged explicit
/// shows its picture to its owner only.
pub fn picture_key_for(menu: &Menu, picture: &Picture, public: bool) -> Option<String> {
    let capture = |c: Option<&CaptureRow>| c.filter(|c| !(public && c.explicit)).map(CaptureRow::picture_key);
    match menu.page {
        Page::Results => menu.page_results().get(picture.result as usize).map(|v| v.id.clone()),
        Page::Music | Page::Home if picture.result == 0 => menu.media.as_ref().and_then(|m| m.art.clone()),
        Page::Card if picture.result == 0 => menu.card.as_ref().and_then(|c| c.video.clone()),
        Page::Photo if menu.pages.share() && picture.result == 0 => capture(menu.captures.latest.as_ref()),
        Page::History => capture(menu.page_captures().get(picture.result as usize)),
        Page::Explicit if picture.result == 0 => capture(menu.shared_capture()),
        _ => None,
    }
}

/// The cells a Previous instances row's line under its world has, and a History row's.
const INSTANCE_META_COLUMNS: usize = 32;
const CAPTURE_META_COLUMNS: usize = 33;

/// A tile's note on Home.
fn tile_note(menu: &Menu, tile: u8) -> String {
    match tile {
        0 if menu.desktop_mode => tr("Only in VR").into(),
        0 => (if menu.fast_walk { tr("On") } else { tr("Off") }).into(),
        1 => match menu.results.len() {
            0 => tr_ctx("panel", "Search videos").into(),
            _ => format!("\"{}\"", menu.results_for),
        },
        2 => match &menu.call {
            Some(Call::Ringing { name }) => tr!("{name} calls", name = name),
            Some(Call::Calling { .. }) => tr("Calling…").into(),
            Some(Call::Connected { seconds, .. }) => super::clock(*seconds),
            None if !menu.calls_ready => tr("Not available").into(),
            None => match menu.contacts.len() {
                0 => tr("No friend online").into(),
                n => tr_n("{count} friend", "{count} friends", n as u64),
            },
        },
        3 => match menu.countdown {
            Some(n) => tr!("In {seconds} s", seconds = n),
            None if menu.photo_timer == 0 => tr("No timer").into(),
            None => tr!("Timer {seconds} s", seconds = menu.photo_timer),
        },
        4 => match &menu.media {
            Some(m) if !m.title.is_empty() => m.title.clone(),
            _ => tr("Nothing playing").into(),
        },
        5 => match menu.light {
            Some(light) if light.on => tr!("On · {color}", color = light_color(light.color)),
            Some(_) => tr("Off").into(),
            None => tr("Not on this N4N0").into(),
        },
        6 if !menu.states_ready => tr("Avatar not known").into(),
        6 => match menu.states.len() {
            0 => tr("Save how it is").into(),
            1 => tr("1 saved").into(),
            n => tr!("{count} saved", count = n),
        },
        7 => match &menu.tracking {
            Some(t) if t.on => tr("Smoothing on").into(),
            Some(_) => tr("Smoothing off").into(),
            None => tr("Not here").into(),
        },
        NOTE_MORE => tr("Light, instances, more").into(),
        NOTE_INSTANCES if !menu.instances_ready && menu.instances.is_empty() => tr("Join them again").into(),
        NOTE_INSTANCES => match menu.instances.len() {
            0 => tr("None yet").into(),
            n => tr_n("{count} instance", "{count} instances", n as u64),
        },
        NOTE_QR if menu.qr.scanning() => tr("Scanning…").into(),
        NOTE_QR => tr("Open a link from the camera").into(),
        NOTE_MIRROR => match (menu.mirror.unavailable_note(), menu.mirror.phase) {
            (Some(why), _) => why.into(),
            (None, crate::mirror::Phase::Off) => tr("The world's screen, here").into(),
            (None, crate::mirror::Phase::Aiming { .. }) => tr("Aiming…").into(),
            (None, crate::mirror::Phase::Placed) => tr("Mirroring").into(),
        },
        NOTE_FEEDBACK => match menu.feedback.subjects.first() {
            Some(s) if s.why == feedback::Why::Here && !s.name.is_empty() => s.name.clone(),
            _ => tr("Like, label, comment").into(),
        },
        _ if menu.sending => tr("Sending…").into(),
        _ => tr("Tell the team").into(),
    }
}

/// A text kept as it was first written, drawn in the language now: a note or notice made of one fixed English text is its own key (`tr`); one
/// made with values was translated when it was made, and is no key.
fn said(text: &str) -> String {
    tr(text).to_string()
}

/// A light colour's name in the language now: the menu's own words for it (`menu-strings`), in the order of the contract's colours.
pub fn light_color(color: impl Into<usize>) -> &'static str {
    strings::LIGHT_COLORS.get(color.into()).map_or("", |t| strings::Strings::builtin(orbiters_i18n::language()).get(*t))
}

/// A light brightness choice's name in the language now (the order of the contract's levels).
fn light_level(choice: usize) -> &'static str {
    strings::LIGHT_LEVELS.get(choice).map_or("", |t| strings::Strings::builtin(orbiters_i18n::language()).get(*t))
}

/// The lines of what the seven-tile Home's place for N4N0's words shows (`text`, cut between words; the last ends
/// in "…" when there is more).
pub fn heard_lines(text: &str) -> Vec<String> {
    let mut lines = super::pieces(text, HEARD_COLUMNS);
    if lines.len() > HEARD_LINES {
        lines.truncate(HEARD_LINES);
        let last = &mut lines[HEARD_LINES - 1];
        let kept: String = last.chars().take(HEARD_COLUMNS - 1).collect();
        *last = format!("{}…", kept.trim_end());
    }
    lines
}

/// What N4N0 says, in the pieces the seven-tile Home's place shows one after the other (as many lines as it holds each).
pub fn heard_pages(text: &str) -> Vec<String> {
    super::pieces(&shown(text), HEARD_COLUMNS).chunks(HEARD_LINES).map(|lines| lines.join(" ")).collect()
}

/// A link as a dropped card writes it: without its scheme and `www.`, and cut to `cols` with "…".
pub fn short_link(link: &str, cols: usize) -> String {
    let bare = link.trim();
    let bare = bare.strip_prefix("https://").or_else(|| bare.strip_prefix("http://")).unwrap_or(bare);
    let bare = bare.strip_prefix("www.").unwrap_or(bare);
    ellipsis(bare.trim_end_matches('/'), cols)
}

/// What a state puts back, for its row.
fn state_meta(state: &super::StateRow) -> String {
    let what = match (state.pose, state.size) {
        (true, true) => tr("Parameters, pose and size"),
        (true, false) => tr("Parameters and pose"),
        (false, true) => tr("Parameters and size"),
        (false, false) => tr("Parameters"),
    };
    if state.id.is_empty() {
        tr("Saving…").into()
    } else {
        what.into()
    }
}

/// The time of what plays: `1:23 / 3:45`, or where it is alone, or nothing.
fn media_time(position: Option<u32>, duration: Option<u32>) -> String {
    match (position, duration) {
        (Some(at), Some(length)) if length > 0 => format!("{} / {}", super::clock(at.min(length) as u64), super::clock(length as u64)),
        (Some(at), _) => super::clock(at as u64),
        _ => String::new(),
    }
}

/// What a slot shows (not yet cut to its cells).
pub fn text(menu: &Menu, content: Content) -> String {
    text_for(menu, content, false)
}

/// What a slot shows, to the owner or, `public`, to everyone else around (the avatar's screen through the synced
/// channel): there the owner's friends are never named (the call list, who calls or is called), nor the owner's own
/// words to N4N0.
pub fn text_for(menu: &Menu, content: Content, public: bool) -> String {
    if public {
        match content {
            Content::ContactName(_) => return String::new(),
            Content::CallName if menu.call.is_some() => return tr("A friend").into(),
            Content::CallsTitle if menu.calls_ready => return tr("Call a friend").into(),
            Content::TileNote(2) => {
                return match &menu.call {
                    Some(Call::Ringing { .. }) => tr("Incoming call").into(),
                    Some(Call::Calling { .. }) => tr("Calling…").into(),
                    Some(Call::Connected { seconds, .. }) => super::clock(*seconds),
                    None => String::new(),
                }
            }
            Content::Heard | Content::HeardLine(_) | Content::HeardWide(_) | Content::HeardCaption if menu.heard_private => return String::new(),
            // What the owner writes to Orbiters is theirs: everyone else's screen shows the page without it.
            Content::SuggestLine(_) => return String::new(),
            // So are the links the owner's camera found.
            Content::QrLink | Content::QrNotice => return String::new(),
            Content::TypeLine(_) if matches!(menu.typing, Typing::Suggestion | Typing::Friend) => return String::new(),
            // The owner's friends, which part of a name they look for, and why their capture is flagged.
            Content::ShareFriend(_) | Content::ExplicitWhy(_) => return String::new(),
            Content::FriendsTitle => return tr("Friends").into(),
            _ => {}
        }
    }
    match content {
        Content::Status => {
            if menu.listening {
                tr("Listening… say what you want").into()
            } else {
                tr("Tap a tile, or call N4N0 again to talk").into()
            }
        }
        Content::TileNote(i) => tile_note(menu, i),
        Content::Heard => menu.heard.as_ref().map(|h| format!("\"{h}\"")).unwrap_or_default(),
        Content::Query => menu.query.clone(),
        Content::Suggestion(i) => menu.shown_suggestions().nth(i as usize).map(str::to_string).unwrap_or_default(),
        Content::ResultsTitle => {
            if let Some(notice) = &menu.notice {
                said(notice)
            } else if menu.loading() {
                tr!("Searching \"{query}\"…", query = menu.results_for)
            } else {
                format!("\"{}\"", menu.results_for)
            }
        }
        Content::PageLabel => match menu.page {
            // "+": YouTube has more than the pages fetched so far.
            Page::Results => format!("{}/{}{}", menu.result_page + 1, menu.result_pages(), if menu.more_available() { "+" } else { "" }),
            // One page of friends (or none) says nothing: on the avatar's screen it would cost a block of its own, and
            // the page would show whole a step later.
            Page::Call if menu.contacts.len() <= CONTACTS_PER_PAGE => String::new(),
            Page::Call => format!("{}/{}", menu.contact_page + 1, menu.contacts.len().div_ceil(CONTACTS_PER_PAGE)),
            Page::States => format!("{}/{}", menu.state_page + 1, menu.states.len().div_ceil(STATES_PER_PAGE).max(1)),
            Page::Instances => format!("{}/{}", menu.instance_page + 1, menu.instances.len().div_ceil(INSTANCES_PER_PAGE).max(1)),
            Page::History => format!("{}/{}", menu.capture_page + 1, menu.captures.history.len().div_ceil(CAPTURES_PER_PAGE).max(1)),
            Page::ShareTo => format!("{}/{}", menu.friend_page + 1, menu.found_friends().len().div_ceil(FRIENDS_PER_PAGE).max(1)),
            Page::Feedback if menu.feedback.subjects.len() > 1 => format!("{}/{}", menu.subject + 1, menu.feedback.subjects.len()),
            Page::Comments => format!("{}/{}", menu.comment_page + 1, menu.shown_feedback().map_or(0, |f| f.comments.len()).div_ceil(COMMENTS_PER_PAGE).max(1)),
            _ => String::new(),
        },
        Content::HeardCaption => match (&menu.heard, menu.heard_private) {
            (None, _) => String::new(),
            (Some(_), true) => tr("You said").into(),
            (Some(_), false) => tr("N4N0 says").into(),
        },
        Content::HeardLine(i) => menu.heard.as_deref().map(|h| heard_lines(&shown(h))).and_then(|lines| lines.into_iter().nth(i as usize)).unwrap_or_default(),
        Content::StatesTitle => {
            if let Some(notice) = &menu.notice {
                said(notice)
            } else if !menu.states_ready {
                said(&menu.states_note)
            } else {
                match menu.states.len() {
                    0 => tr("Nothing saved for this avatar").into(),
                    1 => tr("1 saved for this avatar").into(),
                    n => tr!("{count} saved for this avatar", count = n),
                }
            }
        }
        Content::StateName(i) => menu.page_states().get(i as usize).map(|s| s.name.clone()).unwrap_or_default(),
        Content::StateMeta(i) => menu.page_states().get(i as usize).map(state_meta).unwrap_or_default(),
        Content::CardTitle(line) => {
            let Some(card) = &menu.card else { return String::new() };
            let lines = super::pieces(&shown(&card.title), CARD_TITLE_COLUMNS);
            match line {
                0 => lines.first().cloned().unwrap_or_default(),
                // The second line ends in "…" when the title goes on.
                _ if lines.len() > 2 => ellipsis(&format!("{} {}", lines[1], lines[2]), CARD_TITLE_COLUMNS),
                _ => lines.get(1).cloned().unwrap_or_default(),
            }
        }
        Content::CardLink => menu.card.as_ref().map(|c| short_link(&c.link, CARD_LINK_COLUMNS)).unwrap_or_default(),
        Content::ResultTitle(i) => menu.page_results().get(i as usize).map(|v| v.title.clone()).unwrap_or_default(),
        // The row whose link was copied says so (the notice says it too, and the row offers Drop).
        Content::ResultMeta(i) if menu.page_results().get(i as usize).is_some_and(|v| menu.copied.as_deref() == Some(v.id.as_str())) => tr("✓ Link copied").into(),
        Content::ResultMeta(i) => menu
            .page_results()
            .get(i as usize)
            .map(|v| {
                let length = if v.live { tr("LIVE").to_string() } else { v.duration.clone().unwrap_or_default() };
                if length.is_empty() {
                    v.channel.clone()
                } else {
                    // The duration stays whole; the channel gives way.
                    let room = 26usize.saturating_sub(length.chars().count() + 3);
                    format!("{} · {}", ellipsis(&v.channel, room), length)
                }
            })
            .unwrap_or_default(),
        Content::CallsTitle => {
            if !menu.calls_ready {
                said(&menu.call_note)
            } else {
                match menu.contacts.len() {
                    // Short: on the avatar's screen it goes with the page's header and one more block.
                    0 => tr("No friend online").into(),
                    1 => tr("Call a friend · 1 online").into(),
                    n => tr!("Call a friend · {count} online", count = n),
                }
            }
        }
        Content::ContactName(i) => menu.page_contacts().get(i as usize).map(|c| c.name.clone()).unwrap_or_default(),
        Content::CallName => match &menu.call {
            Some(Call::Ringing { name } | Call::Calling { name } | Call::Connected { name, .. }) => name.clone(),
            None => String::new(),
        },
        Content::CallState => match &menu.call {
            Some(Call::Ringing { .. }) => tr("is calling you").into(),
            Some(Call::Calling { .. }) => tr("Calling…").into(),
            Some(Call::Connected { seconds, muted, .. }) if *muted => tr!("{time} · muted", time = super::clock(*seconds)),
            Some(Call::Connected { seconds, .. }) => super::clock(*seconds),
            None => String::new(),
        },
        Content::PhotoStatus => match menu.countdown {
            Some(n) => tr!("Starting in {seconds}… hold still", seconds = n),
            None => match &menu.notice {
                Some(notice) => said(notice),
                // The two pictures are buttons: nothing to say until something happens.
                None if menu.pages.two_pictures() => String::new(),
                // Shorter beside the last capture (from screen version 12).
                // Which of the two Take takes, and how to change it: the line itself is the button, the older pictures of
                // this page having room for one Take.
                None => match menu.photo_mode {
                    super::PhotoMode::People => tr("Of the people here. Tap: of the world").into(),
                    super::PhotoMode::World => tr("Of the world, no avatars. Tap: of people").into(),
                },
            },
        },
        Content::MediaTitle => match &menu.media {
            Some(m) if !m.title.is_empty() => m.title.clone(),
            _ => tr("Nothing is playing").into(),
        },
        Content::MediaArtist => match &menu.media {
            Some(m) if !m.title.is_empty() => m.artist.clone(),
            _ => tr("Play music on this computer: it shows here").into(),
        },
        // The YouTube video playing says its link was copied there (a YouTube page gives no album), as a result's row does.
        Content::MediaAlbum if menu.pages.ten() && menu.media.as_ref().is_some_and(|m| m.video.is_some() && m.video == menu.copied) => tr("✓ Link copied").into(),
        Content::MediaAlbum => menu.media.as_ref().filter(|m| !m.title.is_empty()).map(|m| m.album.clone()).unwrap_or_default(),
        // With the timeline (from screen version 9): what was just done, else whether it plays and which app (the time is
        // at the timeline's ends).
        Content::MediaState if menu.pages.timeline() => match (&menu.notice, &menu.media) {
            (Some(notice), _) => said(notice),
            (None, Some(m)) if !m.title.is_empty() || !m.app.is_empty() => {
                let parts = [if m.playing { tr("Playing") } else { tr("Paused") }, m.app.as_str()];
                parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" · ")
            }
            _ => String::new(),
        },
        Content::MediaPosition | Content::MediaDuration if !menu.pages.timeline() => String::new(),
        Content::MediaPosition => match &menu.media {
            Some(m) if m.duration.is_some_and(|d| d > 0) => super::clock(m.position.unwrap_or(0).min(m.duration.unwrap_or(0)) as u64),
            _ => String::new(),
        },
        Content::MediaDuration => match menu.media.as_ref().and_then(|m| m.duration).filter(|d| *d > 0) {
            Some(length) => super::clock(length as u64),
            None => String::new(),
        },
        Content::MediaState => match &menu.media {
            Some(m) if !m.title.is_empty() || !m.app.is_empty() => {
                let parts = [if m.playing { tr("Playing").to_string() } else { tr("Paused").to_string() }, media_time(m.position, m.duration), m.app.clone()];
                parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" · ")
            }
            _ => String::new(),
        },
        Content::VolumeLabel => match menu.volume {
            Some(v) if v.muted => tr("Muted").into(),
            Some(v) => tr!("Volume {percent} %", percent = v.percent),
            None => String::new(),
        },
        Content::LightState => match menu.light {
            Some(light) if light.on => {
                tr!("On · {color} · {level}", color = light_color(light.color), level = light_level(light.level_choice()))
            }
            Some(_) => tr("Off").into(),
            None => String::new(),
        },
        Content::LightNote => match menu.light {
            Some(_) => tr("Everyone around sees N4N0's light").into(),
            None => tr("Turn Light on for N4N0 in My Avatar (its card's options)").into(),
        },
        Content::HeardWide(i) => {
            menu.heard.as_deref().map(|h| wide_heard_lines(&shown(h))).and_then(|lines| lines.into_iter().nth(i as usize)).unwrap_or_default()
        }
        Content::Versions => menu.versions.clone(),
        Content::TypeTitle => match menu.typing {
            Typing::StateName => tr("Name the new state").into(),
            Typing::Suggestion => tr("Your idea for N4N0").into(),
            Typing::Friend => tr("Find a friend by name").into(),
            Typing::Comment => match menu.shown_subject() {
                Some(s) if !s.name.is_empty() => tr!("Comment on {name}", name = s.name),
                _ => tr("Your comment").into(),
            },
            Typing::Justification => {
                let words = orbiters_contracts::world_feedback::words(&menu.typed);
                match &menu.notice {
                    Some(notice) => said(notice),
                    None if words >= 10 => tr!("Why it is an AI world · {words} words", words = words),
                    None => tr!("Why it is an AI world · {words} of 10 words", words = words),
                }
            }
        },
        Content::TypeLine(i) => typed_lines(&menu.typed).into_iter().nth(i as usize).unwrap_or_default(),
        Content::DraftName => menu.draft.name.clone(),
        Content::DraftNote => {
            // Whole sentences, as languages order and join the three differently.
            let puts = match (menu.draft.size, menu.draft.pose) {
                (false, false) => tr("Puts back its parameters"),
                (true, false) => tr("Puts back its parameters and size"),
                (false, true) => tr("Puts back its parameters and pose"),
                (true, true) => tr("Puts back its parameters, size and pose"),
            };
            if menu.draft.default {
                tr!("{puts} · also after each world join", puts = puts)
            } else {
                puts.to_string()
            }
        }
        // The fixed pages have no sliders: what each kind really is, in the order of their rows.
        Content::TrackingState => match &menu.tracking {
            Some(t) if t.on => tr!("On: {headset} / {controllers} / {trackers} ms", headset = t.ms[0], controllers = t.ms[1], trackers = t.ms[2]),
            Some(_) => tr("Off").into(),
            None => String::new(),
        },
        Content::TrackingNote => match &menu.tracking {
            Some(t) => said(&t.note),
            None => said(&menu.tracking_note),
        },
        Content::SuggestLine(i) if menu.suggestion.trim().is_empty() => {
            if i == 0 {
                tr("Type or say what N4N0 should do next").into()
            } else {
                String::new()
            }
        }
        Content::SuggestLine(i) => suggestion_lines(&menu.suggestion).into_iter().nth(i as usize).unwrap_or_default(),
        Content::WorldVideo => match &menu.world_video {
            Some(video) if menu.pages.ten() => tr!("In the world: {title}", title = video.title.as_deref().unwrap_or(&video.url)),
            _ => String::new(),
        },
        Content::SuggestNote => {
            if let Some(notice) = &menu.notice {
                said(notice)
            } else if menu.dictating {
                tr("Listening: say your idea").into()
            } else {
                String::new()
            }
        }
        Content::InstancesTitle => {
            if let Some(notice) = &menu.notice {
                said(notice)
            } else if !menu.instances_ready {
                said(&menu.instances_note)
            } else {
                match menu.instances.len() {
                    0 => tr("None yet: join a world in VRChat").into(),
                    n => tr_n("{count} instance", "{count} instances", n as u64),
                }
            }
        }
        Content::InstanceWorld(i) => {
            menu.page_instances().get(i as usize).map(|r| if r.world.is_empty() { tr("A world").to_string() } else { r.world.clone() }).unwrap_or_default()
        }
        Content::InstanceMeta(i) => menu.page_instances().get(i as usize).map(instance_meta).unwrap_or_default(),
        Content::FriendName => menu.invite_account.as_ref().map(|a| a.name.clone()).unwrap_or_default(),
        Content::FriendLink => menu.invite_account.as_ref().map(|a| short_link(&a.link, FRIEND_LINK_COLUMNS)).unwrap_or_default(),
        Content::FriendNote => {
            if menu.invite_waiting {
                tr("Asking Orbiters…").into()
            } else {
                menu.notice.as_deref().map(said).unwrap_or_else(|| tr("Then press Try again").into())
            }
        }
        Content::QrState => qr_state(&menu.qr).into(),
        Content::QrNote(i) => super::pieces(&qr_note(&menu.qr), QR_NOTE_COLUMNS).into_iter().nth(i as usize).unwrap_or_default(),
        Content::QrLink => match &menu.qr.link {
            Some(link) => short_link(link, QR_LINK_COLUMNS),
            None => tr("None yet").into(),
        },
        Content::QrNotice => menu.notice.as_deref().map(said).unwrap_or_default(),
        Content::LastWorld => match &menu.captures.latest {
            Some(c) => world_name(c),
            None => tr("No 3D capture yet").into(),
        },
        Content::LastState => menu.captures.latest.as_ref().map(CaptureRow::state).unwrap_or_default(),
        Content::LastNote(i) => super::pieces(&last_note(menu.captures.latest.as_ref()), LAST_NOTE_COLUMNS).into_iter().nth(i as usize).unwrap_or_default(),
        Content::HistoryTitle => match &menu.notice {
            Some(notice) => said(notice),
            None if !menu.captures.listed => tr("Listing…").into(),
            None => match menu.captures.history.len() {
                0 if !menu.captures.note.is_empty() => said(&menu.captures.note),
                0 => tr("None on this computer yet").into(),
                n => tr_n("{count} capture", "{count} captures", n as u64),
            },
        },
        Content::CaptureWorld(i) => menu.page_captures().get(i as usize).map(world_name).unwrap_or_default(),
        Content::CaptureMeta(i) => menu.page_captures().get(i as usize).map(capture_meta).unwrap_or_default(),
        Content::FriendsTitle => match &menu.friends {
            Friends::None | Friends::Loading => tr("Loading friends…").into(),
            Friends::Failed { .. } => tr("Friends not found").into(),
            Friends::Ready { friends } if friends.is_empty() => tr("No friends found").into(),
            Friends::Ready { .. } if !menu.find.trim().is_empty() => format!("\"{}\": {}", menu.find.trim(), menu.found_friends().len()),
            Friends::Ready { friends } => tr_n("{count} friend", "{count} friends", friends.len() as u64),
        },
        Content::ShareFriend(i) => menu.page_friends().get(i as usize).map(|f| f.name.clone()).unwrap_or_default(),
        Content::FriendsNote => match (&menu.notice, &menu.friends) {
            (Some(notice), _) => said(notice),
            (None, Friends::Failed { message }) => said(message),
            (None, _) if menu.chosen.is_empty() => tr("Choose friends, then Send").into(),
            (None, _) => tr_n("{count} friend chosen", "{count} friends chosen", menu.chosen.len() as u64),
        },
        Content::ExplicitWorld => menu.shared_capture().map(world_name).unwrap_or_default(),
        Content::ExplicitState => tr("Flagged explicit content").into(),
        Content::ExplicitWhy(i) => {
            let reasons: Vec<String> =
                menu.shared_capture().map(|c| c.explicit_reasons.iter().map(|r| orbiters_core::explicit::reason_label(r)).collect()).unwrap_or_default();
            let why = if reasons.is_empty() { tr("Orbiters flagged it when it was shared").to_string() } else { tr!("Because of {reasons}", reasons = reasons.join(", ")) };
            super::pieces(&why, EXPLICIT_WHY_COLUMNS).into_iter().nth(i as usize).unwrap_or_default()
        }
        Content::ExplicitLine(i) => super::pieces(tr(crate::captures::EXPLICIT_WARNING), EXPLICIT_COLUMNS).into_iter().nth(i as usize).unwrap_or_default(),
        Content::FeedbackWorld | Content::LabelsWorld => match menu.shown_subject() {
            Some(s) if !s.name.is_empty() => s.name.clone(),
            Some(_) => tr("This world").into(),
            None => tr("No world yet").into(),
        },
        Content::FeedbackWhy => match menu.shown_subject() {
            Some(s) => feedback::why_text(s.why, now()),
            None => tr("Join a world to give it feedback").into(),
        },
        Content::LikeNote => match menu.shown_feedback() {
            Some(f) if f.loading && f.likes == 0 => tr("Reading…").into(),
            Some(f) if f.liked => tr!("{likes} · You like it", likes = feedback::likes_text(f.likes)),
            Some(f) => feedback::likes_text(f.likes),
            None => String::new(),
        },
        Content::TopChip(i) => match menu.shown_feedback().and_then(|f| f.top().get(i as usize).map(|l| (l.id.clone(), l.count))) {
            Some((id, count)) => format!("{} · {count}", orbiters_contracts::world_feedback::label(&id).map_or(id.as_str(), |l| l.name(orbiters_i18n::language().tag()))),
            None => String::new(),
        },
        Content::CommentsCount => menu.shown_feedback().map(|f| feedback::comments_text(f.comment_count)).unwrap_or_default(),
        Content::NewestMeta => menu.shown_feedback().and_then(|f| f.comments.first()).map(|c| comment_meta(c, 30)).unwrap_or_default(),
        Content::NewestLine(i) => menu
            .shown_feedback()
            .and_then(|f| f.comments.first())
            .and_then(|c| wrapped(&c.text, NEWEST_COLUMNS, NEWEST_LINES).into_iter().nth(i as usize))
            .unwrap_or_else(|| match (i, menu.shown_feedback()) {
                (0, Some(f)) if !f.loading && f.comments.is_empty() => tr("Be the first to say something").into(),
                _ => String::new(),
            }),
        Content::MirrorState => menu.mirror.state(),
        Content::MirrorNote => menu.mirror.note().into(),
        Content::FeedbackNote | Content::LabelsNote => match (&menu.notice, menu.shown_feedback()) {
            (Some(notice), _) => said(notice),
            _ if menu.shown_subject().is_none() => String::new(),
            _ if !menu.feedback.signed_in => tr("Sign in to Orbiters - N4N0 to give feedback").into(),
            (None, Some(f)) if f.error.is_some() => f.error.as_deref().map(said).unwrap_or_default(),
            (None, Some(f)) if f.pending > 0 => tr("Sending…").into(),
            (None, _) if content == Content::LabelsNote => tr("AI world asks why, in at least 10 words").into(),
            _ => String::new(),
        },
        Content::CommentsTitle => match (&menu.notice, menu.shown_feedback()) {
            (Some(notice), _) => said(notice),
            (None, Some(f)) if f.loading && f.comments.is_empty() => tr("Reading…").into(),
            (None, Some(f)) => feedback::comments_text(f.comment_count),
            (None, None) => String::new(),
        },
        Content::CommentMeta(i) => menu.page_comments().get(i as usize).map(|c| comment_meta(c, 22)).unwrap_or_default(),
        Content::CommentLine(i) => {
            let (comment, line) = (i as usize / COMMENT_LINES, i as usize % COMMENT_LINES);
            menu.page_comments().get(comment).and_then(|c| wrapped(&c.text, COMMENT_COLUMNS, COMMENT_LINES).into_iter().nth(line)).unwrap_or_default()
        }
    }
}

/// Now, in Unix seconds.
fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A comment's author and when in `cols` characters, the time always whole: `Enzo · 2 h ago`, `Lyra Nightf… · 3 d`,
/// `You · Sending…`.
fn comment_meta(c: &feedback::CommentRow, cols: usize) -> String {
    let seconds = now().saturating_sub(c.at);
    let long = feedback::ago(seconds);
    let by = if c.by.trim().is_empty() { tr("Someone") } else { c.by.trim() };
    let when = if c.id == 0 {
        tr("Sending…").to_string()
    } else if by.chars().count() + 3 + long.chars().count() <= cols {
        long
    } else {
        feedback::ago_short(seconds)
    };
    format!("{} · {when}", ellipsis(by, cols.saturating_sub(3 + when.chars().count()).max(1)))
}

/// A capture's world, as its rows name it.
fn world_name(capture: &CaptureRow) -> String {
    if capture.world.trim().is_empty() {
        tr("Unknown world").into()
    } else {
        capture.world.clone()
    }
}

/// A History row's line under its world: when it was taken, then where it stands.
fn capture_meta(capture: &CaptureRow) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let seconds = now.saturating_sub(capture.taken);
    let long = format!("{} · {}", ago(seconds), capture.state());
    // The long time ("2 h ago") gives way to the short one where a language needs more room than the row has.
    if long.chars().count() <= CAPTURE_META_COLUMNS {
        long
    } else {
        format!("{} · {}", feedback::ago_short(seconds), capture.state())
    }
}

/// What the last capture's state means on the 3D picture page, in a sentence or two.
fn last_note(capture: Option<&CaptureRow>) -> String {
    use crate::captures::CapturePhase;
    let Some(capture) = capture else { return tr("Take a 3D picture here, or record one from the avatar's menu").into() };
    match &capture.sharing {
        Sharing::Waiting { .. } => return tr("Orbiters - N4N0 makes its 3D view, then shares it").into(),
        Sharing::Busy { .. } => return tr("Preparing and uploading its 3D view").into(),
        Sharing::Done { public: true, .. } => return tr("On orbiters.cc/maps and in the world's viewer").into(),
        Sharing::Done { .. } => return tr("Your friends find it in their Orbiters gallery").into(),
        Sharing::Explicit => return tr("Press Share publicly to see why").into(),
        Sharing::Failed { message } => return said(message),
        Sharing::Idle => {}
    }
    match capture.phase {
        CapturePhase::Recording | CapturePhase::Saving => tr("Share it once it is saved").into(),
        CapturePhase::Waiting | CapturePhase::Making { .. } | CapturePhase::Paused { .. } | CapturePhase::NoView => tr("Share it now: it goes once its 3D view is made").into(),
        CapturePhase::Ready if capture.explicit => tr("Flagged explicit: sharing it publicly asks first").into(),
        CapturePhase::Ready => tr("Publicly, or with the friends you choose").into(),
    }
}

/// What the QR code scanner does, in a line.
pub fn qr_state(qr: &crate::qr::QrView) -> &'static str {
    use crate::qr::QrPhase;
    match qr.phase {
        QrPhase::Off => tr("Not scanning"),
        QrPhase::Starting => tr("Starting…"),
        QrPhase::NoCamera => tr("Waiting for VRChat's camera"),
        QrPhase::Looking if qr.link.is_some() => tr("Opened · looking for more"),
        QrPhase::Looking => tr("Point the stream camera at a QR code"),
        QrPhase::Stopped => tr("Stopped"),
    }
}

/// The scanner's lines under what it does: how to turn the stream camera on, why it stopped, what a code held, or what
/// it does with what it finds.
pub fn qr_note(qr: &crate::qr::QrView) -> String {
    if !qr.note.is_empty() {
        return said(&qr.note);
    }
    match qr.phase {
        crate::qr::QrPhase::Off => tr("Press Scan again to read VRChat's stream camera").into(),
        _ => tr("Its web link opens in the computer's browser at once").into(),
    }
}

/// A previous instance's line under its world: who may join it, its region, when it was joined.
fn instance_meta(row: &super::InstanceRow) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let seconds = now.saturating_sub(row.at);
    let mut parts = vec![tr_ctx("instance", row.kind()).to_string()];
    if let Some(region) = row.region() {
        parts.push(region.to_string());
    }
    parts.push(ago(seconds));
    let long = parts.join(" · ");
    // The long time ("2 h ago") gives way to the short one where a language needs more room than the row has.
    if long.chars().count() <= INSTANCE_META_COLUMNS {
        return long;
    }
    parts.pop();
    parts.push(feedback::ago_short(seconds));
    parts.join(" · ")
}

/// How long ago, in a few characters (a row's meta holds "Group Public · US E · 12 d ago"): "just now", "5 min ago",
/// "3 h ago", "2 d ago".
pub fn ago(seconds: u64) -> String {
    match seconds {
        0..60 => tr("just now").into(),
        60..3600 => tr!("{minutes} min ago", minutes = seconds / 60),
        3600..86400 => tr!("{hours} h ago", hours = seconds / 3600),
        _ => tr!("{days} d ago", days = seconds / 86400),
    }
}

/// What N4N0 says on the ten-tile Home, in its lines (cut between words; the last ends in "…" when there is more).
pub fn wide_heard_lines(text: &str) -> Vec<String> {
    let mut lines = super::pieces(text, HEARD_WIDE_COLUMNS);
    if lines.len() > HEARD_WIDE_LINES {
        lines.truncate(HEARD_WIDE_LINES);
        let last = &mut lines[HEARD_WIDE_LINES - 1];
        let kept: String = last.chars().take(HEARD_WIDE_COLUMNS - 1).collect();
        *last = format!("{}…", kept.trim_end());
    }
    lines
}

/// What N4N0 says, in the pieces the ten-tile Home's place shows one after the other.
pub fn wide_heard_pages(text: &str) -> Vec<String> {
    super::pieces(&shown(text), HEARD_WIDE_COLUMNS).chunks(HEARD_WIDE_LINES).map(|lines| lines.join(" ")).collect()
}

/// The keyboard page's text in its lines, the caret at its end: cut every [`TYPE_COLUMNS`] characters (the caret stays
/// where the typing is), the last [`TYPE_LINES`] lines.
pub fn typed_lines(typed: &str) -> Vec<String> {
    let text: Vec<char> = shown_keep_spaces(typed).chars().chain(std::iter::once('|')).collect();
    let lines: Vec<String> = text.chunks(TYPE_COLUMNS).map(|c| c.iter().collect()).collect();
    let skip = lines.len().saturating_sub(TYPE_LINES);
    lines.into_iter().skip(skip).collect()
}

/// A suggestion in its lines on Suggest features (cut between words), the last [`SUGGEST_LINES`] when it is longer.
pub fn suggestion_lines(text: &str) -> Vec<String> {
    let lines = super::pieces(&shown(text), SUGGEST_COLUMNS);
    let skip = lines.len().saturating_sub(SUGGEST_LINES);
    lines.into_iter().skip(skip).collect()
}

/// `text` as the screen's font writes it, its spaces kept (a text being typed ends in the space just typed).
fn shown_keep_spaces(text: &str) -> String {
    orbiters_contracts::n4n0::panel_text(&crate::text::fold_more(text)).replace('\n', " ")
}

/// `text` as the screen's font writes it (folded, one space between words): what text is wrapped from, so that the lines
/// keep their length on the screen.
fn shown(text: &str) -> String {
    orbiters_contracts::n4n0::panel_text(&crate::text::fold_more(text)).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` cut to `cols` characters, ending in "…" when cut.
pub fn ellipsis(text: &str, cols: usize) -> String {
    let count = text.chars().count();
    if count <= cols {
        return text.to_string();
    }
    if cols == 0 {
        return String::new();
    }
    let mut cut: String = text.chars().take(cols - 1).collect();
    cut = cut.trim_end().to_string();
    cut.push('…');
    cut
}

/// What a slot shows, cut to its cells: the screen shows exactly this, and the overlay too (characters the screen's
/// font has no glyph for are folded or dropped, as on N4N0's panel).
pub fn slot_text(menu: &Menu, slot: &Slot) -> String {
    slot_text_for(menu, slot, false)
}

/// What a slot shows, cut to its cells, to the owner or to everyone else (`public`, see [`text_for`]).
pub fn slot_text_for(menu: &Menu, slot: &Slot, public: bool) -> String {
    // The avatar's atlas has no Japanese: what a slot shows is in Latin letters (`latin`).
    let text = orbiters_contracts::n4n0::panel_text(&crate::text::fold_more(&crate::text::latin(|| text_for(menu, slot.content, public)))).replace('\n', " ");
    match slot.content {
        // The field keeps the end in view, with the caret.
        Content::Query => {
            let shown: Vec<char> = text.chars().collect();
            let room = slot.cols as usize - 1;
            let start = shown.len().saturating_sub(room);
            shown[start..].iter().collect::<String>() + "|"
        }
        // The text being typed keeps its spaces (its lines are cut where the caret falls).
        Content::TypeLine(_) => ellipsis(&text, slot.cols as usize),
        // The versions end at the slot's right edge.
        Content::Versions => format!("{:>1$}", ellipsis(text.trim(), slot.cols as usize), slot.cols as usize),
        // One space where a dropped character (an emoji) left two.
        _ => ellipsis(&text.split_whitespace().collect::<Vec<_>>().join(" "), slot.cols as usize),
    }
}

/// How an element shows now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Chosen or switched on: a green ring.
    pub on: bool,
    /// Not there now (no such result, no page before): not drawn.
    pub hidden: bool,
    /// Shown greyed out, not available now (fast walk in desktop mode): nothing happens on it.
    pub unavailable: bool,
}

impl State {
    /// Whether a pointer hovers and presses it.
    pub fn pointable(self) -> bool {
        !self.hidden && !self.unavailable
    }
}

pub fn state(menu: &Menu, element: &Element) -> State {
    target_state(menu, element.target)
}

/// How an element shows to the owner or, `public`, to everyone else (the avatar's screen through the synced channel).
pub fn state_for(menu: &Menu, element: &Element, _public: bool) -> State {
    target_state(menu, element.target)
}

/// What pressing `target` would find, wherever it is drawn (the window's Home draws its own controls for the same
/// targets).
pub fn target_state(menu: &Menu, target: Target) -> State {
    let on = match target {
        Target::FastWalk => menu.fast_walk,
        Target::StayHere => menu.pinned,
        Target::Timer(i) => TIMERS.get(i as usize) == Some(&menu.photo_timer),
        Target::Length(i) => super::PhotoLength::ALL.get(i as usize) == Some(&menu.photo_length),
        Target::Light | Target::LightSwitch => menu.light.is_some_and(|l| l.on),
        Target::LightColor(i) => menu.light.is_some_and(|l| l.color == i),
        Target::LightLevel(i) => menu.light.is_some_and(|l| l.level_choice() == i as usize),
        Target::Mute => menu.volume.is_some_and(|v| v.muted),
        // The default state's mark: chosen, ringed in green.
        Target::StateMark(i) => menu.page_states().get(i as usize).is_some_and(|s| s.default),
        Target::DraftSize => menu.draft.size,
        Target::DraftPose => menu.draft.pose,
        Target::DraftDefault => menu.draft.default,
        Target::TrackingSwitch => menu.tracking.as_ref().is_some_and(|t| t.on),
        Target::Smoothing(i) => menu.tracking.as_ref().is_some_and(|t| t.on && t.ms.get((i / 4) as usize).is_some_and(|&ms| super::smoothing_choice(ms) == (i % 4) as usize)),
        Target::SuggestTalk => menu.dictating,
        // The timeline's segments the music reached.
        Target::Timeline(i) => menu.media.as_ref().is_some_and(|m| timeline_lit(m, i)),
        // A friend chosen on Share to ….
        Target::FriendMark(i) => menu.page_friends().get(i as usize).is_some_and(|f| menu.is_chosen(f)),
        // World feedback: the player's like and labels.
        Target::Like => menu.shown_feedback().is_some_and(|f| f.liked),
        Target::TopLabel(i) => menu.shown_feedback().is_some_and(|f| f.top().get(i as usize).is_some_and(|l| l.mine)),
        Target::LabelChip(i) => feedback::label_at(i as usize).is_some_and(|l| menu.shown_feedback().is_some_and(|f| f.has(&l.id))),
        // The screen mirror's Aim while the camera follows the eyes.
        Target::MirrorAim => matches!(menu.mirror.phase, crate::mirror::Phase::Aiming { .. }),
        _ => false,
    };
    let hidden = match target {
        // What stays is the avatar's screen: the overlay's menu always comes to the headset.
        Target::StayHere => !menu.on_screen,
        Target::Suggestion(i) => menu.shown_suggestions().nth(i as usize).is_none(),
        Target::Copy(i) => i as usize >= menu.page_results().len(),
        Target::Contact(i) => i as usize >= menu.page_contacts().len() || menu.call.is_some(),
        Target::Previous => !menu.has_previous(),
        Target::Next => !menu.has_next(),
        Target::Take => menu.countdown.is_some(),
        Target::MediaPlayPause => !menu.media.as_ref().is_some_and(|m| m.can_play_pause),
        // Previous, or back to the start of what has nothing before it.
        Target::MediaPrevious => !menu.media.as_ref().is_some_and(|m| m.can_previous || (m.can_seek && m.duration.is_some())),
        Target::MediaNext => !menu.media.as_ref().is_some_and(|m| m.can_next),
        Target::Mute | Target::VolumeDown | Target::VolumeUp => menu.volume.is_none(),
        Target::LightSwitch | Target::LightColor(_) | Target::LightLevel(_) => menu.light.is_none(),
        // Only the copied result offers Drop, where a card can be dropped.
        Target::Drop(i) => !menu.can_drop || menu.page_results().get(i as usize).is_none_or(|v| menu.copied.as_deref() != Some(v.id.as_str())),
        Target::StateApply(i) => i as usize >= menu.page_states().len(),
        Target::StateMark(i) => !menu.page_states().get(i as usize).is_some_and(|s| s.default),
        Target::StateSave => !menu.states_ready,
        Target::WatchVideo | Target::SaveVideo => menu.world_video.is_none() || !menu.pages.ten(),
        // The YouTube video the computer plays, once found; its Drop once its link was copied, where cards drop.
        Target::CopyPlaying => !menu.pages.ten() || menu.media.as_ref().is_none_or(|m| m.video.is_none()),
        Target::DropPlaying => {
            !menu.pages.ten() || !menu.can_drop || menu.media.as_ref().and_then(|m| m.video.as_ref()).is_none_or(|v| menu.copied.as_ref() != Some(v))
        }
        // The timeline, for what has a length (from screen version 9).
        Target::Timeline(_) | Target::Seek(_) => !menu.pages.timeline() || menu.media.as_ref().and_then(|m| m.duration).is_none_or(|d| d == 0),
        // While the link card is out.
        Target::TakeBack => !menu.pages.timeline() || !menu.card_out,
        Target::InviteMe(i) => i as usize >= menu.page_instances().len(),
        Target::CopyFriendName | Target::OpenFriendLink => menu.invite_account.is_none(),
        // The scanner's buttons: Scan again once it stopped, the last link's once there is one.
        Target::QrAgain => !menu.pages.qr() || menu.qr.scanning(),
        Target::QrCopy | Target::QrOpen => !menu.pages.qr() || menu.qr.link.is_none(),
        Target::InviteAgain => menu.inviting.is_none(),
        // Sharing 3D captures (from screen version 12): the last capture's shares once there is one, History's rows', a
        // friend's row and their mark while they are chosen.
        Target::History => !menu.pages.share(),
        Target::ShareLatest | Target::ShareLatestTo => !menu.pages.share() || menu.captures.latest.is_none(),
        Target::SharePublic(i) | Target::ShareTo(i) => i as usize >= menu.page_captures().len(),
        Target::Friend(i) => i as usize >= menu.page_friends().len(),
        Target::FriendMark(i) => !menu.page_friends().get(i as usize).is_some_and(|f| menu.is_chosen(f)),
        // World feedback (from screen version 13): what needs a world, the labels people gave, the page's comments and
        // Delete on the player's own once Orbiters has it.
        Target::WorldFeedback => !menu.pages.feedback(),
        Target::Like | Target::Labels | Target::Comments | Target::WriteComment => menu.shown_subject().is_none(),
        Target::TopLabel(i) => menu.shown_feedback().is_none_or(|f| i as usize >= f.top().len()),
        Target::CommentCard(i) => i as usize >= menu.page_comments().len(),
        Target::DeleteComment(i) => !menu.page_comments().get(i as usize).is_some_and(|c| c.mine && c.id != 0),
        _ => false,
    };
    let unavailable = match target {
        Target::FastWalk => menu.desktop_mode,
        // A timer is counting: both stay, greyed out, over the line that counts (hiding them would empty the page).
        Target::TakeOf(_) => menu.countdown.is_some(),
        Target::DraftSave => !menu.states_ready,
        Target::TrackingSwitch | Target::Smoothing(_) => menu.tracking.is_none(),
        Target::SuggestSend => menu.sending || menu.suggestion.trim().is_empty(),
        Target::InviteMe(_) | Target::InviteAgain => menu.invite_waiting,
        // A capture still being recorded or saved, or with a share on its way, waits.
        Target::ShareLatest | Target::ShareLatestTo => !menu.captures.latest.as_ref().is_some_and(CaptureRow::shareable),
        Target::SharePublic(i) | Target::ShareTo(i) => !menu.page_captures().get(i as usize).is_some_and(CaptureRow::shareable),
        Target::SendShare => menu.chosen.is_empty(),
        // Feedback is given signed in.
        Target::Like | Target::TopLabel(_) | Target::LabelChip(_) | Target::WriteComment => !menu.feedback.signed_in,
        // The screen mirror shows only on the avatar's screen, its zoom within its steps.
        Target::Mirror => !menu.mirror.available(),
        Target::MirrorZoomIn => !menu.mirror.can_zoom_in(),
        Target::MirrorZoomOut => !menu.mirror.can_zoom_out(),
        _ => false,
    };
    State { on: on && !unavailable, hidden, unavailable }
}

/// Whether timeline segment `i` is lit: the music reached its start (and played on from there).
pub fn timeline_lit(media: &super::Media, i: u8) -> bool {
    match (media.position, media.duration) {
        (Some(at), Some(length)) if length > 0 => at as u64 * TIMELINE_STEPS as u64 > i as u64 * length as u64,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// From screen version 15 the page has the two pictures as buttons, a press taking that picture, and no line to press:
    /// a press anywhere on the line's old place is nothing. Older screens keep the picture that they have.
    #[test]
    fn the_two_pictures_are_buttons_on_the_new_page_and_stay_a_line_on_older_ones() {
        use super::super::{Effect, Page, PhotoLength, PhotoMode};
        let shows = |version: i32| {
            let mut menu = Menu::new(KeyboardLayout::Qwerty, 0, PhotoLength::Standard);
            menu.pages = Pages::of_screen(Some(version));
            menu.page = Page::Photo;
            ScreenPage::of(&menu)
        };
        assert_eq!(shows(11), ScreenPage::Photo);
        assert_eq!(shows(12), ScreenPage::PhotoShare);
        assert_eq!(shows(14), ScreenPage::PhotoShare);
        assert_eq!(shows(15), ScreenPage::PhotoTwo);
        assert_eq!(ScreenPage::PhotoTwo as u8, 40, "the page's byte, and the slice and row of its picture");

        let l = layout(ScreenPage::PhotoTwo);
        assert!(!l.elements.iter().any(|e| matches!(e.target, Target::Take | Target::PhotoMode)), "one Take and its line are gone");
        assert_eq!(hit(&l, 300.0, 150.0).map(|(_, t)| t), Some(Target::TakeOf(PhotoMode::People)));
        assert_eq!(hit(&l, 300.0, 280.0).map(|(_, t)| t), Some(Target::TakeOf(PhotoMode::World)));
        assert_eq!(hit(&l, 300.0, 500.0), None, "the status line presses nothing");

        // Pressing takes that picture with the timer and the flight as they are, whatever was last chosen.
        let mut menu = Menu::new(KeyboardLayout::Qwerty, 0, PhotoLength::Standard);
        menu.open = true;
        menu.pages = Pages::ALL;
        menu.page = Page::Photo;
        menu.photo_timer = 5;
        menu.photo_length = PhotoLength::Full;
        for (mode, y) in [(PhotoMode::World, 280.0), (PhotoMode::People, 150.0), (PhotoMode::World, 280.0)] {
            let (element, target) = hit_pointable(&l, &menu, 300.0, y).expect("a button there");
            assert_eq!(l.elements[element].target, target);
            assert_eq!(menu.press(target), vec![Effect::Photo3d { timer: 5, length: PhotoLength::Full, mode }]);
            assert_eq!(menu.photo_mode, mode, "the Action Menu's old button takes this one next");
        }

        // Nothing in the line until there is something to say, which then fits.
        let slot = l.slots.iter().find(|s| s.content == Content::PhotoStatus).expect("the line");
        assert_eq!(text(&menu, Content::PhotoStatus), "");
        menu.notice = Some("The 40 characters a menu's line may have.".chars().take(40).collect());
        assert!(text(&menu, Content::PhotoStatus).chars().count() <= slot.cols as usize);
        menu.notice = None;

        // While a timer counts both stay, greyed out, and take no press; the line counts.
        menu.countdown = Some(3);
        for mode in [PhotoMode::People, PhotoMode::World] {
            let e = l.elements.iter().find(|e| e.target == Target::TakeOf(mode)).unwrap();
            let s = state(&menu, e);
            assert!(s.unavailable && !s.hidden && !s.pointable(), "{mode:?}");
        }
        assert_eq!(hit_pointable(&l, &menu, 300.0, 150.0), None);
        assert_eq!(text(&menu, Content::PhotoStatus), "Starting in 3… hold still");
        assert_eq!(menu.press(Target::TakeOf(PhotoMode::World)), Vec::new());
    }

    /// The line under Take is what chooses the picture on a screen whose page has one Take, and it fits both pages' slots.
    #[test]
    fn the_line_under_take_says_which_picture_and_presses() {
        for (page, pages) in [(ScreenPage::Photo, Pages::Mirror), (ScreenPage::PhotoShare, Pages::Mirror)] {
            let l = layout(page);
            let slot = l.slots.iter().find(|s| s.content == Content::PhotoStatus).expect("the line");
            let mut menu = Menu::new(KeyboardLayout::Qwerty, 0, super::super::PhotoLength::Standard);
            menu.pages = pages;
            for mode in [super::super::PhotoMode::People, super::super::PhotoMode::World] {
                menu.photo_mode = mode;
                let line = text(&menu, Content::PhotoStatus);
                assert!(line.chars().count() <= slot.cols as usize, "{page:?}: {line}");
            }
            assert!(l.elements.iter().any(|e| e.target == Target::PhotoMode), "{page:?}");
            assert_eq!(hit(&l, 200.0, 512.0).map(|(_, t)| t), Some(Target::PhotoMode), "{page:?}");
        }
    }

    /// A label's room stays on the page, clear of its edge, and every Home tile's title is a Home text.
    #[test]
    fn labels_have_room_on_the_page() {
        for page in ScreenPage::ALL {
            for label in &layout(page).labels {
                assert!(label.width > 0 && label.x + label.width <= WIDTH - 32, "{page:?}: {:?} leaves the page", label.text);
            }
        }
        for (_, _, title, _) in HOME_TILES {
            assert!(title.key().starts_with("home."));
        }
    }

    #[test]
    fn every_page_fits_the_screen_and_its_tables() {
        for page in ScreenPage::ALL {
            let l = layout(page);
            assert!(l.elements.len() <= MAX_ELEMENTS, "{page:?}: {} elements", l.elements.len());
            assert!(l.slots.len() <= MAX_SLOTS, "{page:?}: {} slots", l.slots.len());
            assert!(l.pictures.len() <= PICTURES);
            let rects =
                l.elements.iter().map(|e| e.rect).chain(l.pictures.iter().map(|p| p.rect)).chain(l.icons.iter().map(|i| i.0)).chain(l.boxes.iter().copied());
            for r in rects {
                assert!(r.right() <= WIDTH && r.bottom() <= HEIGHT, "{page:?}: {r:?} leaves the page");
                for v in [r.x, r.y, r.w, r.h] {
                    assert_eq!(v % GRID, 0, "{page:?}: {r:?} is off the grid");
                    assert!(v / GRID <= 255, "{page:?}: {r:?} does not fit the screen's layout table (a byte in 4-pixel units)");
                }
            }
            for s in &l.slots {
                assert!(s.rect().right() <= WIDTH && s.rect().bottom() <= HEIGHT, "{page:?}: {s:?} leaves the page");
                assert_eq!((s.x % GRID, s.y % GRID), (0, 0), "{page:?}: {s:?} is off the grid");
                assert_eq!(s.size % 2, 0, "cells are half as wide as high, in whole pixels");
            }
            let used: u16 = l.slots.iter().map(|s| s.cols as u16).sum();
            assert!(used <= TEXT_BYTES, "{page:?}: {used} text bytes");
        }
    }

    #[test]
    fn keyboards_have_every_letter_once() {
        for page in [ScreenPage::SearchQwerty, ScreenPage::SearchAzerty] {
            let l = layout(page);
            let mut letters: Vec<char> =
                l.elements.iter().filter_map(|e| if let Target::Key(c) = e.target { Some(c) } else { None }).filter(|c| c.is_ascii_lowercase()).collect();
            letters.sort();
            letters.dedup();
            assert_eq!(letters.len(), 26, "{page:?}");
            // Keys do not overlap.
            let keys: Vec<Rect> =
                l.elements.iter().filter(|e| matches!(e.look, Look::Key | Look::KeyDark) || e.target == Target::Search).map(|e| e.rect).collect();
            for (i, a) in keys.iter().enumerate() {
                for b in &keys[i + 1..] {
                    let apart = a.right() <= b.x || b.right() <= a.x || a.bottom() <= b.y || b.bottom() <= a.y;
                    assert!(apart, "{page:?}: {a:?} and {b:?} overlap");
                }
            }
        }
        let symbols = layout(ScreenPage::SearchSymbols);
        assert!(symbols.elements.iter().any(|e| e.target == Target::Key('é')));
        assert!(symbols.elements.iter().any(|e| e.target == Target::Letters));
    }

    #[test]
    fn hits_buttons_over_rows() {
        let l = layout(ScreenPage::Results);
        assert_eq!(hit(&l, 900.0, 140.0).map(|h| h.1), Some(Target::Copy(0)), "the copy button");
        assert_eq!(hit(&l, 400.0, 140.0).map(|h| h.1), Some(Target::Copy(0)), "the row copies too");
        assert_eq!(hit(&l, 60.0, 50.0).map(|h| h.1), Some(Target::Back));
        assert_eq!(hit(&l, 512.0, 570.0), None);
        // Where a result's Drop would be: the row copies while Drop is hidden, Drop drops once it shows.
        let mut menu = Menu { pages: super::super::Pages::Seven, can_drop: true, ..Default::default() };
        menu.open();
        menu.search_for("cats");
        let video = |i: usize| crate::youtube::Video { id: format!("video{i:06}"), title: format!("T{i}"), channel: "C".into(), duration: None, live: false };
        menu.results_arrived(1, crate::youtube::Results { videos: (0..5).map(video).collect(), continuation: None }, false);
        assert_eq!(hit_pointable(&l, &menu, 760.0, 140.0).map(|h| h.1), Some(Target::Copy(0)));
        menu.press(Target::Copy(0));
        assert_eq!(hit_pointable(&l, &menu, 760.0, 140.0).map(|h| h.1), Some(Target::Drop(0)));
        assert_eq!(hit_pointable(&l, &menu, 760.0, 232.0).map(|h| h.1), Some(Target::Copy(1)), "the next row has none");
    }

    #[test]
    fn texts_fit_their_slots() {
        let mut menu = Menu::default();
        menu.open();
        menu.press(Target::YouTube);
        menu.type_text("a really long query that will never fit in the field at all");
        let l = layout(ScreenPage::of(&menu));
        let query = l.slots.iter().find(|s| s.content == Content::Query).unwrap();
        let shown = slot_text(&menu, query);
        assert_eq!(menu.query.chars().count(), crate::menu::QUERY_MAX, "the field holds 40 characters");
        assert_eq!(shown.chars().count(), query.cols as usize);
        assert!(shown.ends_with("fit |") && !shown.starts_with("a "), "the field shows the end being typed: {shown}");
        assert_eq!(ellipsis("Lofi Girl and friends", 8), "Lofi Gi…");
        assert_eq!(ellipsis("Lofi", 8), "Lofi");
        menu.search_for("cats");
        menu.results_arrived(
            1,
            crate::youtube::Results {
                videos: vec![crate::youtube::Video {
                    id: "n61ULEU7CO0".into(),
                    title: "x".into(),
                    channel: "A channel with a very long name indeed".into(),
                    duration: Some("6:10:58".into()),
                    live: false,
                }],
                continuation: None,
            },
            false,
        );
        let meta = text(&menu, Content::ResultMeta(0));
        assert!(meta.ends_with(" · 6:10:58") && meta.chars().count() <= 26, "{meta}");
        let l = layout(ScreenPage::Results);
        let copy = l.elements.iter().rev().find(|e| e.target == Target::Copy(1)).unwrap();
        assert!(state(&menu, copy).hidden, "one result: the second row is hidden");
        assert!(state(&menu, &l.elements[2]).hidden, "no next page");
        assert_eq!(cached(ScreenPage::Results), &l, "built once, the same");
    }

    #[test]
    fn six_tiles_only_where_the_display_has_them() {
        let mut menu = Menu::default();
        menu.open();
        assert_eq!(ScreenPage::of(&menu), ScreenPage::Home, "an older screen: four tiles");
        menu.pages = Pages::Six;
        assert_eq!(ScreenPage::of(&menu), ScreenPage::HomeSix);
        let home = cached(ScreenPage::HomeSix);
        let four = cached(ScreenPage::Home);
        let targets = |l: &Layout| l.elements.iter().map(|e| e.target).collect::<Vec<_>>();
        assert_eq!(targets(home)[..4], targets(four)[..4], "the first four tiles keep their order (and their element indices)");
        assert_eq!(hit(home, 900.0, 400.0).map(|h| h.1), Some(Target::Light));
        menu.press(Target::Music);
        assert_eq!(ScreenPage::of(&menu), ScreenPage::Music);
        menu.pages = Pages::Four;
        assert_eq!(ScreenPage::of(&menu), ScreenPage::Home, "a screen without the page shows Home");
    }

    #[test]
    fn seven_tiles_from_version_5_and_older_screens_keep_their_pages() {
        assert_eq!(Pages::of_screen(None), Pages::Four);
        assert_eq!(Pages::of_screen(Some(3)), Pages::Six);
        assert_eq!(Pages::of_screen(Some(4)), Pages::Six, "version 4 points with rays, its pages are version 3's");
        assert_eq!(Pages::of_screen(Some(5)), Pages::Seven);
        assert_eq!(Pages::of_screen(Some(6)), Pages::Ten);
        assert_eq!(Pages::of_screen(Some(8)), Pages::Ten, "version 8 added no page");
        assert_eq!(Pages::of_screen(Some(9)), Pages::Timeline);
        assert_eq!(Pages::of_screen(Some(10)), Pages::Simple);
        assert_eq!(Pages::of_screen(Some(11)), Pages::Qr);
        assert_eq!(Pages::of_screen(Some(12)), Pages::Share);
        assert_eq!(Pages::of_screen(Some(13)), Pages::Feedback);
        assert_eq!(Pages::of_screen(Some(14)), Pages::Mirror);
        assert_eq!(Pages::of_screen(Some(15)), Pages::TwoPictures);
        assert_eq!(Pages::of_screen(Some(16)), Pages::StayHere);
        assert_eq!(Pages::of_screen(Some(17)), Pages::StayHere);
        let mut menu = Menu { pages: Pages::Seven, ..Default::default() };
        menu.open();
        assert_eq!(ScreenPage::of(&menu), ScreenPage::HomeSeven);
        let seven = cached(ScreenPage::HomeSeven);
        let six = cached(ScreenPage::HomeSix);
        let tiles = |l: &Layout| l.elements.iter().filter(|e| e.look == Look::Tile).map(|e| e.target).collect::<Vec<_>>();
        assert_eq!(tiles(seven)[..6], tiles(six)[..], "the six tiles keep their order, States last");
        assert_eq!(tiles(seven).last(), Some(&Target::States));
        // Two rows of four: the eighth place is what N4N0 says, beside the States tile.
        let states = seven.elements.iter().find(|e| e.target == Target::States).unwrap();
        assert_eq!(seven.boxes.len(), 1);
        assert_eq!((seven.boxes[0].y, seven.boxes[0].w, seven.boxes[0].h), (states.rect.y, states.rect.w, states.rect.h));
        assert!(seven.boxes[0].x > states.rect.right());
        menu.press(Target::States);
        assert_eq!(ScreenPage::of(&menu), ScreenPage::States);
        // An older screen has no States page: it shows its Home; and the six-tile Home's layout is as it was.
        menu.pages = Pages::Six;
        assert_eq!(ScreenPage::of(&menu), ScreenPage::HomeSix);
        assert_eq!(six.elements.len(), 7, "six tiles and Close, as screens of version 3 and 4 have them");
        let results = cached(ScreenPage::Results);
        let drops: Vec<usize> = results.elements.iter().enumerate().filter(|(_, e)| matches!(e.target, Target::Drop(_))).map(|(i, _)| i).collect();
        assert_eq!(drops, (13..18).collect::<Vec<_>>(), "Drop comes after every element older screens know, which keep their indices");
    }

    #[test]
    fn what_n4n0_says_fills_its_place_line_by_line() {
        let mut menu = Menu { pages: Pages::Seven, ..Default::default() };
        menu.open();
        let seven = cached(ScreenPage::HomeSeven);
        let texts = |menu: &Menu, public: bool| {
            seven
                .slots
                .iter()
                .filter(|s| matches!(s.content, Content::HeardCaption | Content::HeardLine(_)))
                .map(|s| slot_text_for(menu, s, public))
                .collect::<Vec<_>>()
        };
        assert!(texts(&menu, false).iter().all(String::is_empty), "nothing heard: an empty place");
        menu.heard = Some("Voilà ton sweat, va voir tes potes maintenant au lieu de parler à une boule".into());
        menu.heard_private = false;
        let shown = texts(&menu, false);
        assert_eq!(shown[0], "N4N0 says");
        assert_eq!(&shown[1..], ["Voilà ton", "sweat, va voir", "tes potes", "maintenant au…"].map(String::from).as_slice(), "{shown:?}");
        assert!(shown.iter().all(|line| line.chars().count() <= HEARD_COLUMNS));
        assert_eq!(texts(&menu, true), shown, "what N4N0 says is for everyone");
        menu.heard("put my hoodie on");
        assert_eq!(texts(&menu, false)[0], "You said");
        assert!(texts(&menu, true).iter().all(String::is_empty), "the owner's own words stay theirs");
    }

    #[test]
    fn states_list_marks_the_default_and_pages_like_the_others() {
        let mut menu = Menu { pages: Pages::Seven, ..Default::default() };
        menu.open();
        menu.press(Target::States);
        let l = cached(ScreenPage::States);
        let shown = |menu: &Menu| l.elements.iter().filter(|e| !state(menu, e).hidden).map(|e| e.target).collect::<Vec<_>>();
        menu.set_states(Vec::new(), false, "VRChat's avatar is not known yet");
        assert_eq!(shown(&menu), vec![Target::Back], "no avatar: nothing to save");
        assert_eq!(text(&menu, Content::StatesTitle), "VRChat's avatar is not known yet");
        let row = |n: usize, default: bool| super::super::StateRow { id: format!("s{n}"), name: format!("State {n}"), default, pose: n == 2, size: false };
        menu.set_states((1..=7).map(|n| row(n, n == 6)).collect(), true, "");
        assert_eq!(text(&menu, Content::StatesTitle), "7 saved for this avatar");
        assert_eq!(text(&menu, Content::PageLabel), "1/2");
        assert_eq!(text(&menu, Content::StateMeta(1)), "Parameters and pose");
        assert!(!shown(&menu).contains(&Target::StateMark(0)), "no default on the first page");
        menu.press(Target::Next);
        assert_eq!(menu.page_states().len(), 2);
        assert!(shown(&menu).contains(&Target::StateMark(0)), "the default's mark, on its row");
        assert!(!shown(&menu).contains(&Target::StateApply(2)) && !shown(&menu).contains(&Target::Next));
        assert_eq!(menu.press(Target::StateMark(0)), vec![super::super::Effect::ApplyState { id: "s6".into(), name: "State 6".into() }]);
        assert_eq!(text(&menu, Content::StatesTitle), "Applied \"State 6\"");
        assert_eq!(text(&menu, Content::TileNote(6)), "7 saved");
    }

    #[test]
    fn a_dropped_card_shows_its_picture_title_and_link() {
        let mut menu = Menu { pages: Pages::Seven, ..Default::default() };
        menu.show_card(super::super::Card {
            link: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
            title: "Rick Astley - Never Gonna Give You Up (Official Music Video) 4K Remaster".into(),
            video: Some("dQw4w9WgXcQ".into()),
            left: 14,
        });
        assert_eq!(ScreenPage::of(&menu), ScreenPage::Card);
        let l = cached(ScreenPage::Card);
        let texts: Vec<String> = l.slots.iter().map(|s| slot_text(&menu, s)).collect();
        assert_eq!(texts, ["Rick Astley - Never Gonna", "Give You Up (Official Mus…", "youtube.com/watch?v=dQw4w9WgXcQ"].map(String::from), "{texts:?}");
        assert_eq!(picture_key(&menu, &l.pictures[0]).as_deref(), Some("dQw4w9WgXcQ"));
        assert!(l.elements.is_empty() && menu.press(Target::Close).is_empty(), "nothing on a card is pressed: it counts no time down");
        assert_eq!(short_link("http://example.com/a/very/long/path/that/goes/on/and/on/", 20), "example.com/a/very/…");
        // Opening the menu takes the card back.
        menu.open();
        assert_eq!((menu.page, menu.card.is_none()), (Page::Home, true));
    }

    #[test]
    fn music_shows_what_plays_and_hides_what_it_cannot_do() {
        let mut menu = Menu { pages: Pages::Six, ..Default::default() };
        menu.open();
        menu.press(Target::Music);
        let l = cached(ScreenPage::Music);
        let shown = |menu: &Menu| l.elements.iter().filter(|e| !state(menu, e).hidden).map(|e| e.target).collect::<Vec<_>>();
        assert_eq!(shown(&menu), vec![Target::Back], "nothing plays, no volume known");
        assert_eq!(text(&menu, Content::MediaTitle), "Nothing is playing");
        menu.media = Some(super::super::Media {
            title: "Midnight City".into(),
            artist: "M83".into(),
            album: "Hurry Up, We're Dreaming".into(),
            app: "Spotify".into(),
            playing: true,
            can_play_pause: true,
            can_next: true,
            position: Some(83),
            duration: Some(243),
            ..Default::default()
        });
        menu.volume = Some(super::super::Volume { percent: 40, muted: false });
        assert!(!shown(&menu).contains(&Target::MediaPrevious) && shown(&menu).contains(&Target::MediaNext));
        assert_eq!(text(&menu, Content::MediaState), "Playing · 1:23 / 4:03 · Spotify");
        assert_eq!(text(&menu, Content::VolumeLabel), "Volume 40 %");
        assert_eq!(text(&menu, Content::TileNote(4)), "Midnight City");
        assert_eq!(picture_key(&menu, &l.pictures[0]), None, "no art yet");
        menu.media.as_mut().unwrap().art = Some("art:1".into());
        assert_eq!(picture_key(&menu, &l.pictures[0]).as_deref(), Some("art:1"));
    }

    #[test]
    fn the_music_timeline_lights_up_to_where_it_is_from_version_9() {
        let l = cached(ScreenPage::Music);
        let index = |t: Target| l.elements.iter().position(|e| e.target == t).unwrap();
        // The segments are switched on (the screen switches elements 0 to 27 on); Take back and the places over them
        // never are, and come after.
        for i in 0..TIMELINE_STEPS as u8 {
            assert!(index(Target::Timeline(i)) < 28 && index(Target::Timeline(i)) > index(Target::DropPlaying), "segment {i}");
            assert!(index(Target::Seek(i)) > index(Target::TakeBack), "place {i}");
            let (segment, place) = (l.elements[index(Target::Timeline(i))].rect, l.elements[index(Target::Seek(i))].rect);
            assert!(segment.x >= place.x && segment.right() <= place.right() && segment.y > place.y && segment.bottom() < place.bottom());
        }
        assert_eq!(l.elements.len(), 44);
        // A finger on the timeline finds the place over the segment, not the segment.
        let place = l.elements[index(Target::Seek(3))].rect;
        let mut menu = Menu { pages: Pages::Timeline, ..Default::default() };
        menu.open();
        menu.press(Target::Music);
        let shown = |menu: &Menu| l.elements.iter().filter(|e| !state(menu, e).hidden).map(|e| e.target).collect::<Vec<_>>();
        assert!(!shown(&menu).contains(&Target::Seek(0)), "nothing plays: no timeline");
        menu.media = Some(super::super::Media { title: "Song".into(), app: "Spotify".into(), playing: true, position: Some(61), duration: Some(240), ..Default::default() });
        assert!(shown(&menu).contains(&Target::Seek(0)) && shown(&menu).contains(&Target::Timeline(15)));
        let (cx, cy) = (place.x as f32 + 10.0, place.y as f32 + 30.0);
        assert_eq!(hit_pointable(l, &menu, cx, cy).map(|(_, t)| t), Some(Target::Seek(3)));
        // 61 s of 240: four segments reached (their starts at 0, 15, 30 and 45 s), the fifth at 60 s too.
        let lit: Vec<u8> = (0..TIMELINE_STEPS as u8).filter(|&i| target_state(&menu, Target::Timeline(i)).on).collect();
        assert_eq!(lit, vec![0, 1, 2, 3, 4]);
        assert!(!target_state(&menu, Target::Seek(0)).on, "the places are never switched on");
        assert_eq!((text(&menu, Content::MediaPosition), text(&menu, Content::MediaDuration)), ("1:01".into(), "4:00".into()));
        assert_eq!(text(&menu, Content::MediaState), "Playing · Spotify", "the time is at the timeline's ends");
        menu.notice = Some("Spotify lets no app move in its timeline".into());
        assert_eq!(text(&menu, Content::MediaState), "Spotify lets no app move in its timeline");
        // A screen of version 6 to 8 keeps its music page as it was.
        menu.pages = Pages::Ten;
        menu.notice = None;
        assert!(!shown(&menu).contains(&Target::Seek(0)) && !shown(&menu).contains(&Target::Timeline(0)));
        assert_eq!(text(&menu, Content::MediaState), "Playing · 1:01 / 4:00 · Spotify");
        assert_eq!(text(&menu, Content::MediaPosition), "");
        // The results' Take back, after every element older screens have.
        let results = cached(ScreenPage::Results);
        assert_eq!(results.elements.last().map(|e| e.target), Some(Target::TakeBack));
    }

    #[test]
    fn light_chips_show_the_light_as_the_avatar_holds_it() {
        let mut menu = Menu { pages: Pages::Six, ..Default::default() };
        menu.open();
        menu.press(Target::Light);
        let l = cached(ScreenPage::Light);
        assert!(l.elements.iter().filter(|e| e.target != Target::Back).all(|e| state(&menu, e).hidden), "no light on this avatar");
        assert!(text(&menu, Content::LightNote).contains("My Avatar"));
        menu.light = Some(super::super::Light { on: true, color: 1, level: 0.62 });
        let on = l.elements.iter().filter(|e| state(&menu, e).on).map(|e| e.target).collect::<Vec<_>>();
        assert_eq!(on, vec![Target::LightSwitch, Target::LightColor(1), Target::LightLevel(1)]);
        assert_eq!(text(&menu, Content::LightState), "On · Warm · Medium");
        assert_eq!(text(&menu, Content::TileNote(5)), "On · Warm");
    }

    #[test]
    fn fast_walk_is_greyed_out_in_desktop_mode() {
        let mut menu = Menu::default();
        menu.fast_walk = true;
        menu.desktop_mode = true;
        for page in [ScreenPage::Home, ScreenPage::HomeSix] {
            let l = layout(page);
            let walk = l.elements.iter().find(|e| e.target == Target::FastWalk).unwrap();
            let s = state(&menu, walk);
            assert!(s.unavailable && !s.on && !s.pointable(), "{page:?}");
            let note = l.slots.iter().find(|s| s.content == Content::TileNote(0)).unwrap();
            assert_eq!(slot_text(&menu, note), "Only in VR", "{page:?}: it fits its slot");
        }
        menu.desktop_mode = false;
        assert!(state(&menu, layout(ScreenPage::Home).elements.iter().find(|e| e.target == Target::FastWalk).unwrap()).pointable());
    }

    #[test]
    fn everyone_else_never_reads_the_owner_s_friends() {
        let mut menu = Menu::default();
        menu.open();
        menu.set_contacts(vec![super::super::Contact { id: "u1".into(), name: "Lyra Nightfall".into() }], true, "");
        menu.press(Target::Call);
        let list = cached(ScreenPage::of(&menu));
        let texts = |public: bool| list.slots.iter().map(|s| slot_text_for(&menu, s, public)).collect::<Vec<_>>().join("|");
        assert!(texts(false).contains("Lyra Nightfall") && texts(false).contains("1 online"));
        assert!(!texts(true).contains("Lyra") && !texts(true).contains("online"), "{}", texts(true));
        menu.set_call(Some(Call::Ringing { name: "Lyra Nightfall".into() }));
        let ringing = cached(ScreenPage::of(&menu));
        let shown: Vec<String> = ringing.slots.iter().map(|s| slot_text_for(&menu, s, true)).collect();
        assert_eq!(shown, vec!["A friend", "is calling you"]);
        assert_eq!(text_for(&menu, Content::TileNote(2), true), "Incoming call");
        assert_eq!(text_for(&menu, Content::TileNote(2), false), "Lyra Nightfall calls");
        // The owner's words to N4N0 stay theirs; what N4N0 says is for everyone.
        menu.heard("put my hoodie on");
        assert_eq!(text_for(&menu, Content::Heard, true), "");
        menu.heard = Some("Voilà ton sweat".into());
        menu.heard_private = false;
        assert_eq!(text_for(&menu, Content::Heard, true), "\"Voilà ton sweat\"");
    }

    /// Menus with something in each place, for the language tests below.
    fn menus_with_something_everywhere() -> Vec<Menu> {
        use super::super::{Call, Contact, InstanceRow, Light, Media, StateRow, Tracking, Volume};
        use crate::captures::{CapturePhase, CaptureRow, Sharing};
        let base = || {
            let mut menu = Menu::new(KeyboardLayout::Qwerty, 3, super::super::PhotoLength::Standard);
            menu.pages = Pages::ALL;
            menu.open();
            menu
        };
        let taken = now().saturating_sub(7200);
        let capture = |phase: CapturePhase, sharing: Sharing| CaptureRow { key: "k".into(), world: "Moonlit Garden".into(), taken, phase, sharing, ..Default::default() };
        let mut out = vec![base()];
        let mut calling = base();
        calling.contacts = (0..9).map(|i| Contact { id: format!("u{i}"), name: format!("Friend {i}") }).collect();
        calling.calls_ready = true;
        calling.call = Some(Call::Ringing { name: "Lyra Nightfall".into() });
        out.push(calling.clone());
        calling.call = Some(Call::Connected { name: "Lyra Nightfall".into(), seconds: 75, muted: true });
        out.push(calling.clone());
        calling.call = Some(Call::Calling { name: "Lyra Nightfall".into() });
        out.push(calling);
        let mut playing = base();
        playing.media = Some(Media { title: "Midnight City".into(), artist: "M83".into(), app: "Spotify".into(), playing: true, position: Some(83), duration: Some(243), ..Default::default() });
        playing.volume = Some(Volume { percent: 40, muted: false });
        playing.light = Some(Light { on: true, color: 2, level: 0.62 });
        playing.tracking = Some(Tracking { on: true, ms: [42, 42, 42], note: String::new() });
        playing.fast_walk = true;
        out.push(playing);
        let mut stated = base();
        stated.states = (0..4).map(|i| StateRow { id: format!("s{i}"), name: format!("State {i}"), default: i == 0, pose: i % 2 == 0, size: i < 2 }).collect();
        stated.states_ready = true;
        stated.draft.size = true;
        stated.draft.pose = true;
        stated.draft.default = true;
        out.push(stated);
        let mut shared = base();
        shared.instances_ready = true;
        shared.instances = ["~group(g)~groupAccessType(public)~region(us)", "~private(u)~canRequestInvite~region(jp)", "~friends(u)", "~hidden(u)~region(eu)", ""]
            .into_iter()
            .map(|tags| InstanceRow { location: format!("wrld_x:1{tags}"), world: "Moonlit Garden".into(), at: taken })
            .collect();
        let sharings = [Sharing::Idle, Sharing::Waiting { public: true, people: 0 }, Sharing::Busy { public: false, people: 3 }, Sharing::Done { public: true, people: 0 }, Sharing::Done { public: true, people: 12 }, Sharing::Done { public: false, people: 1 }, Sharing::Explicit, Sharing::Failed { message: "x".into() }];
        let phases = [CapturePhase::Recording, CapturePhase::Saving, CapturePhase::Waiting, CapturePhase::Making { percent: 42 }, CapturePhase::Paused { percent: 42 }, CapturePhase::NoView, CapturePhase::Ready];
        shared.captures.listed = true;
        shared.captures.history = phases.iter().map(|p| capture(*p, Sharing::Idle)).chain(sharings.iter().map(|s| capture(CapturePhase::Ready, s.clone()))).collect();
        shared.captures.latest = shared.captures.history.first().cloned();
        out.push(shared.clone());
        shared.captures.latest = shared.captures.history.last().cloned();
        out.push(shared);
        // The other things a tile's note can say.
        let mut alone = base();
        alone.photo_timer = 0;
        alone.desktop_mode = true;
        alone.light = Some(Light { on: false, color: 0, level: 0.5 });
        alone.tracking = Some(Tracking { on: false, ms: [0, 0, 0], note: String::new() });
        alone.contacts = vec![Contact { id: "u".into(), name: "Lyra".into() }];
        alone.calls_ready = true;
        out.push(alone.clone());
        alone.countdown = Some(5);
        alone.sending = true;
        alone.calls_ready = false;
        alone.call_note = "Sign in to Orbiters to call friends".into();
        alone.states_ready = false;
        alone.states_note = "VRChat's avatar is not known yet".into();
        alone.instances_ready = false;
        alone.qr.phase = crate::qr::QrPhase::Looking;
        alone.mirror = crate::mirror::MirrorView { phase: crate::mirror::Phase::Aiming { seconds: 3 }, zoom: 2, reach: crate::mirror::Reach::Screen };
        out.push(alone.clone());
        alone.qr.phase = crate::qr::QrPhase::NoCamera;
        alone.qr.note = "No picture from VRChat's camera: choose Stream and turn Spout Stream on in its settings".into();
        alone.mirror = crate::mirror::MirrorView { phase: crate::mirror::Phase::Placed, zoom: 2, reach: crate::mirror::Reach::Overlay };
        out.push(alone.clone());
        alone.mirror.reach = crate::mirror::Reach::NoMirror;
        alone.qr.phase = crate::qr::QrPhase::Stopped;
        out.push(alone);
        // Every notice the menu makes by itself, as it is said at the time.
        for notice in ["Link copied: paste it, or Drop it", "Saved as \"Party\"", "Sharing with 3 friends…", "Thank you! Your suggestion reached Orbiters", "Not invited: x", "Comment deleted", "This player lets no app move in its timeline", "Sign in to Orbiters - N4N0 to give feedback", "Opened on the computer: send the friend request there"] {
            let mut menu = base();
            menu.notice = Some(notice.to_string());
            out.push(menu);
        }
        out
    }

    /// Every slot of every page, in menus with something in each place, in each language: the avatar's screen holds Latin letters only (English in
    /// Japanese), no `{name}` is left unfilled, and a translation is not cut where the English is not.
    #[test]
    fn the_slots_follow_the_language_and_fit_where_english_does() {
        use orbiters_i18n::{in_language, Language};
        let mut cut = Vec::new();
        for (m, menu) in menus_with_something_everywhere().iter().enumerate() {
            for page in ScreenPage::ALL {
                for (i, slot) in cached(page).slots.iter().enumerate() {
                    let english = in_language(Language::En, || slot_text(menu, slot));
                    for language in Language::TRANSLATED {
                        let said = in_language(language, || slot_text(menu, slot));
                        assert!(!said.contains('{'), "{language:?} {page:?} {i}: {said:?}");
                        if language == Language::Ja {
                            assert_eq!(said, english, "Japanese keeps the avatar's screen in Latin letters: {page:?} {i}");
                        }
                        let (mine, theirs) = (said.matches('?').count(), english.matches('?').count());
                        assert!(mine <= theirs, "{language:?} {page:?} {i}: a character the atlas does not hold, {said:?}");
                        if said.ends_with('…') && !english.ends_with('…') {
                            cut.push(format!("menu {m} {page:?} slot {i} ({} columns) {language:?}: {said:?} (English {english:?})", slot.cols));
                        }
                    }
                }
            }
        }
        assert!(cut.is_empty(), "{} texts are cut in a language and not in English:\n{}", cut.len(), cut.join("\n"));
    }

    /// Texts that go on several lines (a page's sentence cut between words) have lines enough for each language, or the end of the sentence is lost.
    #[test]
    fn the_sentences_that_wrap_have_the_lines_they_need() {
        use crate::captures::{CapturePhase, CaptureRow, Sharing};
        use orbiters_i18n::{in_language, Language};
        let lines_of = |content: fn(u8) -> Content| (0..8u8).filter(|i| ScreenPage::ALL.iter().any(|p| cached(*p).slots.iter().any(|s| s.content == content(*i)))).count();
        let (last_lines, qr_lines, why_lines, warning_lines) = (lines_of(Content::LastNote), lines_of(Content::QrNote), lines_of(Content::ExplicitWhy), lines_of(Content::ExplicitLine));
        assert_eq!((last_lines, qr_lines, why_lines, warning_lines), (2, 3, 2, 2), "the pages' lines: if a page changes, so do these");
        let sharings = [Sharing::Idle, Sharing::Waiting { public: true, people: 0 }, Sharing::Busy { public: true, people: 0 }, Sharing::Done { public: true, people: 1 }, Sharing::Done { public: false, people: 1 }, Sharing::Explicit];
        let phases = [CapturePhase::Recording, CapturePhase::Waiting, CapturePhase::Ready];
        for language in Language::TRANSLATED {
            in_language(language, || {
                use crate::text::latin;
                let mut checks: Vec<(String, usize, usize)> = Vec::new();
                // What the text needs in this language, against the lines the page has (or the English, where it already needs more).
                let mut add = |text: &dyn Fn() -> String, columns: usize, room: usize| {
                    let mine = latin(|| text());
                    let english = in_language(Language::En, || super::super::pieces(&text(), columns).len());
                    let needs = super::super::pieces(&mine, columns).len();
                    checks.push((mine, needs, room.max(english)));
                };
                for sharing in &sharings {
                    for phase in phases {
                        for explicit in [false, true] {
                            let capture = CaptureRow { sharing: sharing.clone(), phase, explicit, ..Default::default() };
                            add(&|| last_note(Some(&capture)), LAST_NOTE_COLUMNS, last_lines);
                        }
                    }
                }
                add(&|| last_note(None), LAST_NOTE_COLUMNS, last_lines);
                for phase in [crate::qr::QrPhase::Off, crate::qr::QrPhase::Looking] {
                    let qr = crate::qr::QrView { phase, ..Default::default() };
                    add(&|| qr_note(&qr), QR_NOTE_COLUMNS, qr_lines);
                }
                for note in [crate::qr::no_camera_note(Some(false)), crate::qr::no_camera_note(None)] {
                    add(&|| tr(&note).to_string(), QR_NOTE_COLUMNS, qr_lines);
                }
                add(&|| tr(crate::captures::EXPLICIT_WARNING).to_string(), EXPLICIT_COLUMNS, warning_lines);
                add(&|| tr("Orbiters flagged it when it was shared").to_string(), EXPLICIT_WHY_COLUMNS, why_lines);
                let wrong: Vec<String> = checks.into_iter().filter(|(_, n, room)| n > room).map(|(t, n, room)| format!("{t:?}: {n} lines for {room}")).collect();
                assert!(wrong.is_empty(), "{language:?}:\n{}", wrong.join("\n"));
            });
        }
    }

    /// The same texts on a page that draws them with a real font (the window's Home): in the language, with no value left unfilled.
    #[test]
    fn the_window_reads_the_texts_in_the_language() {
        use orbiters_i18n::{in_language, Language};
        let menus = menus_with_something_everywhere();
        let all: Vec<Content> = ScreenPage::ALL.iter().flat_map(|p| cached(*p).slots.iter().map(|s| s.content)).collect();
        for menu in &menus {
            for content in &all {
                for language in Language::TRANSLATED {
                    let said = in_language(language, || text_for(menu, *content, false));
                    assert!(!said.contains('{'), "{language:?} {content:?}: {said:?}");
                }
            }
        }
        let menu = &menus[1];
        in_language(Language::Ja, || assert_eq!(text(menu, Content::TileNote(2)), "Lyra Nightfallから着信"));
        in_language(Language::Fr, || assert_eq!(text(menu, Content::TileNote(2)), "Lyra Nightfall appelle"));
        in_language(Language::En, || assert_eq!(text(menu, Content::TileNote(2)), "Lyra Nightfall calls"));
    }

    /// A count takes the plural of its language: French and Portuguese say 0 in the singular, Japanese has one form.
    #[test]
    fn counts_take_their_languages_plural() {
        use orbiters_i18n::{in_language, Language};
        let mut menu = Menu::default();
        menu.states_ready = true;
        let said = |language, menu: &Menu| in_language(language, || text(menu, Content::StatesTitle));
        assert_eq!(said(Language::En, &menu), "Nothing saved for this avatar");
        menu.states = vec![super::super::StateRow::default(); 2];
        assert_eq!(said(Language::En, &menu), "2 saved for this avatar");
        assert_eq!(said(Language::Fr, &menu), "2 enregistrés pour cet avatar");
        assert_eq!(said(Language::Es, &menu), "2 guardados para este avatar");
        assert_eq!(said(Language::PtBr, &menu), "2 salvos para este avatar");
        assert_eq!(said(Language::Ja, &menu), "このアバターで2件保存済み");
        menu.states = vec![super::super::StateRow::default()];
        assert_eq!(said(Language::Fr, &menu), "1 enregistré pour cet avatar");
    }
}
