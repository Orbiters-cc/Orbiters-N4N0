//! What each part holds, in its own units (around the wheel a hundredth of its radius, around the Quick Menu its unit: a pixel of a 1280 x 720 window either way), so that the
//! desktop's windows and the headset's overlays draw and hit the same things. [`items`] says where everything is and [`paint`] draws it: nothing else decides a place, so what
//! the pointer hits is what is drawn.
//!
//! **Orbiters' own look** ([`look`], Enzo: "the UI shouldn't look exactly the same as the VRChat UI, that could even be a problem [...] it should just not look too much out of place [...]
//! Ideally it would fit the Orbiters design system and look a bit more modern than the VRChat default UI"): its filled surfaces with no outlines, its radii and line icons, its mint for
//! what is pointed at, chosen or under way, its mark where it speaks for itself. What it takes from VRChat is scale, placement, density and a cool dark temperature, so that it sits
//! well beside it without being taken for a part of it.
//!
//! **For someone who has never heard of any of this:** each thing says what it does in its own words, and [`caption`] says in a sentence what is pointed at, near it. What
//! cannot work here is not offered ([`Facts`]' `can_*`): no dead pages.

use super::zones::{self, Ring, Zone};
use egui::epaint::{Mesh, RectShape, Vertex, WHITE_UV};
use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, TextureId, Vec2};
use orbiters_i18n::{mark, tr, tr_ctx, tr_latin};
use orbiters_n4n0_core::menu::{PhotoMode, Target};
use orbiters_ui::theme::{self, weight};
use orbiters_ui::{icons, widgets};
use orbiters_core::settings::{MenuLook, OrbitLayout, OrbitSettings, VrMotion};
use std::path::PathBuf;

mod settings_sheet;
mod tracking_sheet;
pub use tracking_sheet::{smoothing_at, Smoothed};
use tracking_sheet::{draw_smoothing_bar, draw_tracking, tracking_caption, tracking_items};
use settings_sheet::{draw_settings, draw_stepper_name, hue_color, language_name, settings_items, settings_caption, sound_name};
pub use settings_sheet::{hue_at, language_after};
#[cfg(test)]
use settings_sheet::hue_bar;

/// Orbiters' colours, at a cool dark temperature that sits beside VRChat's teal without copying it.
pub mod look {
    use egui::{Color32, CornerRadius, FontFamily, FontId};
    use orbiters_ui::theme;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    static ACCENT_NOW: AtomicU32 = AtomicU32::new(0x00da6d);
    static RETRO_NOW: AtomicBool = AtomicBool::new(false);
    static DOTS_NOW: AtomicBool = AtomicBool::new(false);

    /// What is chosen in the sheet of Settings. With the retro look what is lit is a grey, as all of it goes through one colour afterwards: a dark colour must not dim it.
    pub(super) fn choose(accent: [u8; 3], retro: bool, dots: bool) {
        let accent = if retro { [185; 3] } else { accent };
        ACCENT_NOW.store(u32::from_be_bytes([0, accent[0], accent[1], accent[2]]), Ordering::Relaxed);
        RETRO_NOW.store(retro, Ordering::Relaxed);
        DOTS_NOW.store(retro && dots, Ordering::Relaxed);
    }

    /// Whether the retro look is chosen.
    pub fn retro() -> bool {
        RETRO_NOW.load(Ordering::Relaxed)
    }

    /// How pictures are drawn: in the retro menu's dots if they are not to keep their colours, as they are otherwise.
    pub fn picture_look() -> Option<orbiters_ui::software::PictureLook> {
        DOTS_NOW.load(Ordering::Relaxed).then_some(super::super::retro::dots as orbiters_ui::software::PictureLook)
    }

    /// Text of the look chosen, for what lays out its own: the retro menu's monospace, a little smaller as it is wider.
    pub fn txt(text: impl Into<String>, size: f32) -> theme::Txt {
        let txt = theme::Txt::new(text, size);
        if retro() {
            theme::Txt { size: size * 0.9, ..txt }.mono()
        } else {
            txt
        }
    }

    /// A corner of a picture: the look chosen has them or does not.
    pub fn picture_radii(radii: [f32; 4]) -> [f32; 4] {
        if retro() {
            [0.0; 4]
        } else {
            radii
        }
    }

    /// The accent chosen.
    pub fn accent() -> Color32 {
        let [_, r, g, b] = ACCENT_NOW.load(Ordering::Relaxed).to_be_bytes();
        Color32::from_rgb(r, g, b)
    }

    /// What reads on it: dark on a light colour, white on a dark one.
    pub fn on_accent() -> Color32 {
        let c = accent();
        if 0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32 > 110.0 {
            theme::ON_ACCENT
        } else {
            Color32::WHITE
        }
    }

    /// A font of the look chosen: the retro menu's monospace, a little smaller as it is wider.
    pub fn font(size: f32, weight: f32) -> FontId {
        if retro() {
            FontId::new(size * 0.9, FontFamily::Monospace)
        } else {
            theme::font(size, weight)
        }
    }

    /// A corner of the look chosen: the retro menu's are hard.
    pub fn radius(r: f32) -> CornerRadius {
        theme::radius(if retro() { r.min(3.0) } else { r })
    }

    /// A panel's fill: Orbiters' ink, a little cool.
    pub const PANEL: Color32 = Color32::from_rgba_premultiplied(14, 18, 21, 248);
    pub const OPAQUE: Color32 = Color32::from_rgb(14, 18, 21);
    /// A band at a panel's head.
    pub const HEADER: Color32 = Color32::from_rgb(20, 25, 29);
    /// A tile.
    pub const TILE: Color32 = Color32::from_rgb(29, 35, 40);
    pub const TILE_HOVER: Color32 = Color32::from_rgb(38, 50, 54);
    pub const TILE_DOWN: Color32 = Color32::from_rgb(50, 66, 70);
    pub const ACCENT_SOFT: Color32 = Color32::from_rgb(20, 66, 46);
    /// What is pointed at, chosen, or under way: the app's mint.
    pub const ACCENT: Color32 = orbiters_ui::theme::ACCENT;
    pub const ON_ACCENT: Color32 = orbiters_ui::theme::ON_ACCENT;
    pub const LABEL: Color32 = orbiters_ui::theme::FG;
    pub const MUTED: Color32 = orbiters_ui::theme::FG_2;
    /// A cell of the ring, near the wheel and at its outer edge.
    pub const CELL_NEAR: Color32 = Color32::from_rgb(36, 43, 49);
    pub const CELL_FAR: Color32 = Color32::from_rgb(21, 25, 29);
    /// A panel's corners, as the app's cards and tiles have them.
    pub const RADIUS: f32 = orbiters_ui::theme::CARD_RADIUS * 0.8;
    pub const TILE_RADIUS: f32 = orbiters_ui::theme::TILE_RADIUS * 0.8;
}

/// What pressing something does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// The sheet and all with it away for a moment, or back.
    Peek,
    /// The sheet of settings for these menus, and something chosen on it.
    Settings,
    Set(Pick),
    /// 3D world search.
    Worlds,
    /// A 3D picture of people, of the world.
    Picture(PhotoMode),
    /// Starts or stops a recording.
    Record,
    /// A page of N4N0's menu on the sheet.
    Page(Target),
    /// Everything, on the sheet.
    All,
    /// 3D pictures and captures, on the sheet.
    Pictures,
    /// Fast walk on or off.
    FastWalk,
    /// What the computer plays.
    PlayPause,
    Previous,
    Next,
    /// Talks to N4N0 while held.
    Talk,
    /// The Orbiters window.
    OpenApp,
    /// Moves in what plays: where is the pointer's.
    Timeline,
    /// A tile of the gallery: a capture shows in 3D, a picture large.
    Capture(usize),
    /// What the gallery shows.
    Filter(GalleryFilter),
    /// From the picture shown large, back to the gallery; to the picture after it, or before.
    PictureBack,
    PictureStep(bool),
    /// Captures show small before the person, or around them.
    SmallView(bool),
    /// The 3D view goes.
    HideView,
    /// The scroll bar: where it is grabbed.
    ScrollBar,
    /// Starts or stops listening to the person.
    TalkToggle,
    /// N4N0 forgets this conversation.
    NewConversation,
    /// A YouTube result, the `n`-th: its link, for the world's player.
    Video(usize),
    /// A YouTube result as a card dropped in the world.
    VideoDrop(usize),
    /// Back to searching.
    ResultsBack,
    /// Invites the person to a world's instance: the group's, and which of its visits.
    Invite(usize, usize),
    /// The computer's volume up or down, and muted or not.
    Volume(bool),
    Mute,
    /// The sheet goes.
    CloseSheet,
    /// The parts go for this opening of VRChat's menu.
    Hide,
    /// Nothing: a place the pointer can be on.
    Idle,
}

/// What is on the sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    All,
    Pictures,
    /// A page of N4N0's menu.
    Menu,
    /// 3D world search.
    Worlds,
    /// Where the person was, a row for each world.
    Instances,
    /// YouTube's results, all in one list.
    Results,
    /// Talking with N4N0: one button, and the conversation.
    Talk,
    /// How these menus lie, move, sound and look.
    Settings,
    /// Tracking smoothing, in milliseconds.
    Tracking,
}

/// What can be chosen on the sheet of settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Layout(OrbitLayout),
    Motion(VrMotion),
    Theme(MenuLook),
    /// One of [`SWATCHES`].
    Swatch(usize),
    /// Where on the bar of colours the pointer is.
    Hue,
    /// The retro look leaves pictures in their colours.
    Pictures(bool),
    /// The next sound device, or the one before.
    Sound(bool),
    /// The next place in the language picker, or the one before.
    Language(bool),
    /// Tracking smoothing on or off.
    Smoothing(bool),
    /// Along a smoothing slider.
    SmoothingBar(Smoothed),
}

/// The colours to choose from at a press.
pub const SWATCHES: [[u8; 3]; 8] = [[0x00, 0xda, 0x6d], [0x2b, 0xd9, 0xc8], [0x4f, 0x89, 0xff], [0xbd, 0x65, 0xff], [0xff, 0x5c, 0xa8], [0xff, 0x4d, 0x6d], [0xff, 0x8a, 0x3d], [0xf5, 0xc8, 0x4f]];

/// A result of a YouTube search, as its list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoTile {
    pub title: String,
    pub channel: String,
    pub duration: Option<String>,
    pub picture: Option<TextureId>,
}

/// An invite, as its button shows it right away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InviteChip {
    Asking,
    Sent,
    Failed,
}

/// Where talking with N4N0 stands: what its button shows, so that it is clear as day.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TalkNow {
    #[default]
    Ready,
    /// With how loud the microphone is, 0 to 1.
    Listening(f32),
    Thinking,
    /// It cannot now, and why.
    Off(&'static str),
}

/// A world the person was in, with its instances: how the Instances sheet shows them.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct WorldVisit {
    pub world_id: String,
    pub name: String,
    /// The last time: who may join, where, when.
    pub line: String,
    /// VRChat's location of the last time.
    pub location: String,
    /// The person is in that very instance now.
    pub here: bool,
    /// The times before: location, and how long ago.
    pub earlier: Vec<(String, String)>,
    pub picture: Option<TextureId>,
}

/// What the panels show of a recent capture.
/// What a tile of the gallery is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RecentKind {
    /// A capture that can show in 3D.
    #[default]
    Capture,
    /// A capture with no 3D made of it (yet).
    NoView,
    /// One of VRChat's pictures.
    Picture,
}

/// What the gallery shows, of what is on this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GalleryFilter {
    #[default]
    All,
    /// Only captures, as Enzo asked: "we can have a filter to only show the 3D ones".
    Captures,
}

/// The gallery's tiles and where each lies, made once when they change. Tens of thousands of pictures are nothing unusual, and this goes around with the facts each frame: it is
/// compared by being the same list.
#[derive(Clone, Debug, Default)]
pub struct Gallery(std::sync::Arc<GalleryList>);

#[derive(Debug, Default)]
struct GalleryList {
    items: Vec<Recent>,
    rects: Vec<Rect>,
    height: f32,
}

impl Gallery {
    pub fn new(items: Vec<Recent>, layout: orbiters_core::settings::GalleryLayout) -> Self {
        if items.is_empty() {
            return Self::default();
        }
        // A tall picture still gets a tile wide enough to tell what it is: it is cropped rather than a sliver.
        let aspects: Vec<f32> = items.iter().map(|r| r.shape().clamp(0.85, 2.4)).collect();
        let (rects, height) = crate::pages::gallery_layout::arrange(layout, &aspects, 1024.0 - 48.0, 300.0, 0.0);
        Self(std::sync::Arc::new(GalleryList { items, rects, height }))
    }
}

impl PartialEq for Gallery {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::ops::Deref for Gallery {
    type Target = [Recent];
    fn deref(&self) -> &[Recent] {
        &self.0.items
    }
}

impl FromIterator<Recent> for Gallery {
    fn from_iter<I: IntoIterator<Item = Recent>>(items: I) -> Self {
        Self::new(items.into_iter().collect(), Default::default())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Recent {
    pub dir: PathBuf,
    pub title: String,
    /// "3D picture, 2 h ago".
    pub state: String,
    /// Reconstructed so far, while it is.
    pub progress: Option<f32>,
    pub picture: Option<TextureId>,
    /// Its width over its height, 0 when not known.
    pub aspect: f32,
    pub kind: RecentKind,
}

impl Recent {
    fn shape(&self) -> f32 {
        if self.aspect > 0.0 { self.aspect } else { 16.0 / 9.0 }
    }
}

/// What the panels show of Orbiters' state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub recording: bool,
    /// Seconds of the recording under way.
    pub recording_seconds: Option<u32>,
    /// Starting, stopping, or a picture's flight under way: Record does nothing now.
    pub capture_busy: bool,
    /// Seconds to a 3D picture being taken.
    pub countdown: Option<u32>,
    /// A 3D picture flies now.
    pub picture_flying: bool,
    /// Which the last press asked for: its button shows the countdown.
    pub picture_mode: Option<PhotoMode>,
    /// The newest captures, at most six.
    pub recent: Gallery,
    /// What the gallery shows; whether it is still being read; the picture shown large, and its picture; each tile on the page's words (its number, its state, how far made) and picture.
    pub gallery_filter: GalleryFilter,
    pub gallery_reading: bool,
    pub opened: Option<usize>,
    pub opened_picture: Option<TextureId>,
    /// A capture shown on the sheet in 3D, on the desktop: how far it is, and its picture now.
    pub opened_view: Option<OpenedView>,
    /// The world's player's video's own picture.
    pub world_video_picture: Option<TextureId>,
    pub recent_words: Vec<(usize, String, Option<f32>)>,
    pub recent_pictures: Vec<(usize, TextureId)>,
    /// The band under VRChat's tabs is up, and says what is pointed at.
    pub hint_band: bool,
    /// The hole in the sheet what shows small is seen through, in the sheet's back's units.
    pub sheet_hole: Option<Rect>,
    /// What All tools' large tiles hold, for real: the newest 3D captures, the newest picture.
    pub hero_captures: Vec<TextureId>,
    pub hero_picture: Option<TextureId>,
    /// What the 3D view says of what was asked of it, and whether captures show small.
    pub view_said: Option<(String, String)>,
    pub view_small: bool,
    /// What is being reconstructed: its share done (0 to 1), and how many wait after it.
    pub reconstructing: Option<f32>,
    pub waiting: usize,
    /// VRChat in desktop mode: no fast walk, no tracking, and a real keyboard.
    pub desktop: bool,
    pub fast_walk: bool,
    /// What would make it the better way, when it is on and goes through the play space.
    pub fast_walk_lesser: Option<&'static str>,
    /// N4N0 is on: Ask has someone to talk to.
    pub n4n0: bool,
    /// What plays on the computer.
    pub music: Option<(String, bool)>,
    /// Its artist or application, under its title.
    pub music_by: String,
    /// Its picture, as a texture of the canvas being painted.
    pub art: Option<TextureId>,
    /// The avatar worn has no capture kit, as VRChat's parameters say: a 3D picture cannot be taken, and the buttons say so.
    pub no_kit: bool,
    /// A 3D picture's timer, in seconds.
    pub timer: u32,
    /// How far the sheet's list is scrolled, in its page's units, and the layout the app's own gallery uses.
    pub gallery_scroll: f32,
    pub gallery_layout: orbiters_core::settings::GalleryLayout,
    /// The computer's volume, and whether it is muted; where what plays is and how long it lasts; whether the app playing it lets others move in it.
    pub volume: Option<(u8, bool)>,
    pub music_times: Option<(u32, u32)>,
    pub can_seek: bool,
    /// What the world's video player has.
    pub world_video: Option<String>,
    /// A capture shows in 3D around the person, asked from here.
    pub view_showing: bool,
    /// Which instance was last asked for and how that is going, on the button pressed. Which result's link was just copied.
    pub invite_chip: Option<(String, InviteChip)>,
    pub video_copied: Option<usize>,
    /// YouTube's results, and what was searched.
    pub videos: Vec<VideoTile>,
    pub videos_for: String,
    /// Talking with N4N0: where it stands, the conversation (whether the person said it, what was said), what N4N0 says right now, and how to call it by name when that is on.
    pub talk: TalkNow,
    pub talk_lines: Vec<(bool, String)>,
    /// Tracking smoothing: on or off and each kind's milliseconds, and what the driver does now (`None`: not here).
    pub tracking: Option<(bool, [u16; 3], String)>,
    /// Why not, when it is not here.
    pub tracking_note: String,
    /// The smoothing slider held, and where.
    pub smoothing_held: Option<(Smoothed, u16)>,
    pub n4n0_says: Option<String>,
    pub wake_word: Option<String>,
    /// The ring's cells can be picked by the number keys: on the desktop's wheel.
    pub digits: bool,
    /// Enzo: the wheel's keys show only while Alt is held.
    pub key_caps: bool,
    /// The sheet is away for a moment.
    pub peek: bool,
    /// The wing or its strip lies beside the sheet: it has the name and what plays already.
    pub beside_wing: bool,
    /// The Quick Menu's parts are in a strip under it: its caption lies under that.
    pub strip_caption: bool,
    /// What the sheet of settings shows: as chosen, and the sound device for VR, by name.
    pub orbit: OrbitSettings,
    pub vr_output: Option<String>,
    /// The place the language picker is at.
    pub language_choice: Option<orbiters_i18n::Language>,
    /// What the Instances sheet lists, and what the last invite said.
    pub worlds_visited: Vec<WorldVisit>,
    pub invite_said: Option<String>,
    /// What the caption says with nothing pointed at (a key to hold).
    pub hint: Option<(&'static str, &'static str)>,
    /// A 3D picture is asked for and waits for this: said like a note, for as long as it waits.
    pub picture_wait: Option<orbiters_n4n0::status::PhotoWait>,
    /// What stopped a 3D picture: it goes before anything else the caption or the sheet's foot would say, for a few seconds.
    pub note: Option<(String, String)>,
    /// What is pointed at.
    pub pointed: Option<Action>,
    /// What the sheet shows, and the page's name.
    pub sheet: Option<Sheet>,
    pub sheet_title: String,
    /// Calls can be made here, a state chosen, a light set.
    pub can_call: bool,
    pub can_states: bool,
    pub can_light: bool,
    /// In VR, a close on the sheet and the wing.
    pub vr_close: bool,
}

/// A capture turning on a sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenedView {
    /// Its picture, once there is one; the plate it stands on, as a line over it (0 to 1).
    pub picture: Option<TextureId>,
    pub plate: Vec<[f32; 2]>,
    /// Why it cannot show.
    pub failed: Option<String>,
}

/// Something to point at on a panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub action: Action,
    pub rect: Rect,
    pub look: Look,
}

/// What an item is drawn as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// A smoothing slider.
    SmoothingBar(Smoothed),
    /// What plays in a strip's tile: its picture, its play or pause.
    PlayingSmall,
    /// From a capture shown here, on to the app's window, where it shows in 3D.
    OpenWindow,
    /// Hides these parts, in so many words.
    HideWord,
    /// Back from a picture shown large.
    BackToPictures,
    /// Whether captures show small or around the person.
    SmallView,
    /// One choice of what the gallery shows.
    Filter(GalleryFilter),
    /// A picture of the gallery, as large as the page.
    BigPicture(usize),
    /// To the picture after this one, or before.
    Step(bool),
    /// A cell of the ring: its icon over its label.
    Cell { icon: &'static str, label: &'static str, badge: bool },
    /// A small heading over a group of a sheet's tiles.
    Heading(&'static str),
    /// A YouTube result.
    Video(usize),
    /// A small button on a YouTube result: its link, the `n`-th's, or dropping it.
    VideoCopy(usize),
    VideoDrop,
    /// Where in what plays this is, to press and drag along when the app lets it.
    Timeline,
    /// The bar that shows where in a list this is, and takes the pointer.
    ScrollBar,
    /// Hides the 3D view.
    HideView,
    /// The back button of YouTube's results.
    Back,
    /// The one button that talks to N4N0.
    TalkButton,
    /// Starting the conversation over.
    NewTalk,
    /// A world the person was in: its picture, name and last time.
    Visit(usize),
    /// Takes everything away for a moment, or brings it back.
    PeekWord,
    /// One of the tools, in a sheet's head: from any sheet to any other, with or without the wing.
    HeadTab { icon: &'static str, label: &'static str, on: bool },
    /// One of two ways, on the sheet of settings.
    Choice { icon: &'static str, label: &'static str, on: bool },
    /// The sound device chosen for VR.
    SoundName,
    LanguageName,
    /// A colour to choose, one of [`SWATCHES`], and the bar of all of them.
    Swatch(usize),
    HueBar,
    /// Over the Instances sheet's rows: what they offer, once.
    VisitHead,
    /// Where the Invite me of the instance the person is in would be.
    HereNow,
    /// A button to a world's instance: 0 the last time and the main one, else one of the times before.
    Chip { group: usize, visit: usize },
    /// A tile as VRChat's: its icon over its label, edged.
    Tile { icon: &'static str, label: &'static str, badge: bool },
    /// One of the main features on All tools: large, in its colour, with what it does.
    Hero { icon: &'static str, label: &'static str, line: &'static str, badge: bool, tint: Color32 },
    /// An accessory on All tools: an icon and a name, what it does in the head when pointed at.
    Small { icon: &'static str, label: &'static str, tint: Color32 },
    /// What plays on All tools: its picture, its names and where it is. Its controls are items of their own.
    NowPlaying,
    /// A 3D picture's button.
    Picture(PhotoMode),
    /// A recent capture, the `n`-th.
    Capture(usize),
    /// Record: its icon and state.
    Record,
    /// What plays: the ring's cell or the wing's foot.
    Playing,
    /// A control of what plays.
    Transport(&'static str),
    /// Ask N4N0.
    Talk,
    /// The small close button.
    Close,
    /// Words about what is pointed at.
    Caption,
    /// Nothing drawn.
    Blank,
}

const GAP: f32 = 6.0;
/// How wide Hide tiles is, of the wing's name's width.
const HIDE_TILES: f32 = 122.0;

fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}

/// The items of a part of `size` units.
pub fn items(zone: Zone, size: Vec2, facts: &Facts) -> Vec<Item> {
    let (w, h) = (size.x, size.y);
    let whole = r(0.0, 0.0, w, h);
    let cell = |action, icon, label, badge| vec![Item { action, rect: whole, look: Look::Cell { icon, label, badge } }];
    let tile = |action, icon, label, badge| vec![Item { action, rect: whole, look: Look::Tile { icon, label, badge } }];
    let transport_icon = if facts.music.as_ref().is_some_and(|m| m.1) { icons::PAUSE } else { icons::PLAY };
    match zone {
        Zone::WheelWorlds => cell(Action::Worlds, icons::GLOBE_HEMISPHERE_WEST, tr("Worlds in 3D"), true),
        Zone::WheelYouTube => cell(Action::Page(Target::YouTube), icons::YOUTUBE_LOGO, "YouTube", false),
        Zone::WheelPlaying => vec![Item { action: Action::PlayPause, rect: whole, look: Look::Playing }],
        Zone::WheelAll => cell(Action::All, icons::SQUARES_FOUR, tr("All tools"), false),
        Zone::WheelInstances => cell(Action::Page(Target::Instances), icons::CLOCK, tr("Instances"), false),
        // While a picture waits, this cell is what calls it off.
        Zone::WheelPictures => cell(if facts.picture_wait.is_some() { Action::Picture(facts.picture_mode.unwrap_or(PhotoMode::World)) } else { Action::Pictures }, icons::IMAGES, tr("Pictures"), false),
        Zone::BloomPeople => vec![Item { action: Action::Picture(PhotoMode::People), rect: whole, look: Look::Picture(PhotoMode::People) }],
        Zone::BloomWorld => vec![Item { action: Action::Picture(PhotoMode::World), rect: whole, look: Look::Picture(PhotoMode::World) }],
        Zone::BloomRecord => vec![Item { action: Action::Record, rect: whole, look: Look::Record }],
        Zone::BloomPrevious => vec![Item { action: Action::Previous, rect: whole, look: Look::Transport(icons::SKIP_BACK) }],
        Zone::BloomNext => vec![Item { action: Action::Next, rect: whole, look: Look::Transport(icons::SKIP_FORWARD) }],
        Zone::WheelCaption | Zone::QmCaption | Zone::CameraCaption => vec![Item { action: Action::Idle, rect: whole, look: Look::Caption }],
        // Enzo: what takes a picture or reads a code, at the camera that does it.
        Zone::CameraPeople => tile(Action::Picture(PhotoMode::People), icons::USERS, tr("3D of people"), false),
        Zone::CameraWorld => tile(Action::Picture(PhotoMode::World), icons::GLOBE_HEMISPHERE_WEST, tr("3D of world"), false),
        Zone::CameraQr => tile(Action::Page(Target::Qr), icons::QR_CODE, tr("QR code"), false),
        Zone::WheelVeil => vec![Item { action: Action::Idle, rect: whole, look: Look::Blank }],
        Zone::WheelHide => vec![Item { action: Action::Hide, rect: whole, look: Look::HideWord }],
        // The wing's tiles, each a part of its own.
        Zone::WingName => {
            // One close at a time: the sheet has its own.
            if facts.vr_close && facts.sheet.is_none() {
                vec![Item { action: Action::Hide, rect: r(w - HIDE_TILES, 0.0, HIDE_TILES, h), look: Look::HideWord }]
            } else {
                Vec::new()
            }
        }
        Zone::WingWorlds => tile(Action::Worlds, icons::GLOBE_HEMISPHERE_WEST, tr("Worlds in 3D"), true),
        Zone::WingYouTube => tile(Action::Page(Target::YouTube), icons::YOUTUBE_LOGO, "YouTube", false),
        Zone::WingInstances => tile(Action::Page(Target::Instances), icons::CLOCK, tr("Instances"), false),
        Zone::WingPictures => tile(Action::Pictures, icons::IMAGES, tr("Pictures"), false),
        Zone::WingTalk => vec![Item { action: Action::Talk, rect: whole, look: Look::Talk }],
        Zone::WingAll => tile(Action::All, icons::SQUARES_FOUR, tr_ctx("tile", "All tools"), false),
        Zone::WingPlaying => vec![Item { action: Action::PlayPause, rect: whole, look: Look::Playing }],
        Zone::QmStrip => strip(w, h, facts),
        Zone::SheetHint => Vec::new(),
        Zone::SheetPeek => vec![Item { action: Action::Peek, rect: whole, look: Look::PeekWord }],
        Zone::SheetBack => {
            let mut out = vec![Item { action: Action::CloseSheet, rect: r(w - 40.0, 7.0, 30.0, 30.0), look: Look::Close }];
            out.extend(head_tabs(facts).0);
            // While a capture shows around the person: it is one press from gone, wherever on the sheet.
            if facts.view_showing {
                out.push(Item { action: Action::HideView, rect: r(w - 40.0 - 8.0 - 96.0, 7.0, 96.0, 30.0), look: Look::HideView });
                // On Worlds in 3D, the world's side panel has it.
                if facts.sheet != Some(Sheet::Worlds) {
                    out.push(Item { action: Action::SmallView(!facts.view_small), rect: r(w - 40.0 - 8.0 - 96.0 - 6.0 - 96.0, 7.0, 96.0, 30.0), look: Look::SmallView });
                }
            }
            // All tools has what plays on its page.
            if facts.music.is_some() && !facts.beside_wing && !facts.view_showing && status_now(facts).is_none() && facts.sheet != Some(Sheet::All) {
                // Alone, from the wheel: what plays, in its head.
                let (y, b) = (9.0, 26.0);
                out.push(Item { action: Action::Next, rect: r(w - 48.0 - b, y, b, b), look: Look::Transport(icons::SKIP_FORWARD) });
                out.push(Item { action: Action::PlayPause, rect: r(w - 52.0 - b * 2.0, y, b, b), look: Look::Transport(transport_icon) });
                out.push(Item { action: Action::Previous, rect: r(w - 56.0 - b * 3.0, y, b, b), look: Look::Transport(icons::SKIP_BACK) });
            }
            out
        }
        // The sheet's page, as its sheet says ([`sheet_items`]) or N4N0's own.
        Zone::Takeover => facts.sheet.map_or_else(Vec::new, |s| sheet_items(s, facts)),
    }
}

/// All tools' accessories, and only what can work here.
fn accessories(facts: &Facts) -> Vec<(Action, &'static str, &'static str)> {
    let mut small: Vec<(Action, &'static str, &'static str)> = vec![(Action::Talk, icons::MICROPHONE, tr_ctx("tile", "Ask N4N0"))];
    if facts.can_states {
        small.push((Action::Page(Target::States), icons::BOOKMARKS_SIMPLE, tr("Saved looks")));
    }
    if facts.can_light {
        small.push((Action::Page(Target::Light), icons::LIGHTBULB, tr_ctx("lamp", "Light")));
    }
    if !facts.desktop {
        small.push((Action::FastWalk, icons::PERSON_SIMPLE_RUN, tr("Fast walk")));
        small.push((Action::Page(Target::Tracking), icons::VIRTUAL_REALITY, tr("Smooth tracking")));
    }
    if facts.can_call {
        small.push((Action::Page(Target::Call), icons::PHONE, tr_ctx("voice call", "Call")));
    }
    small.push((Action::Page(Target::WorldFeedback), icons::HEART, tr("World feedback")));
    small.push((Action::Page(Target::Qr), icons::QR_CODE, tr("QR code")));
    small.push((Action::Page(Target::Suggest), icons::CHAT_TEARDROP_TEXT, tr("Suggest")));
    small.push((Action::Settings, icons::GEAR_SIX, tr("Settings")));
    small.push((Action::OpenApp, icons::N4N0_MARK, tr("Open Orbiters")));
    small
}

/// The tools along a sheet's head, where the wing is not beside it, and where they end. Their names where there is room; with something else to say in the head, on a screen, icons but for the one that is open.
fn head_tabs(facts: &Facts) -> (Vec<Item>, f32) {
    const TABS: [(Action, &str, &str, f32); 6] = [
        (Action::Worlds, icons::GLOBE_HEMISPHERE_WEST, mark("Worlds"), 84.0),
        (Action::Page(Target::YouTube), icons::YOUTUBE_LOGO, "YouTube", 90.0),
        (Action::Page(Target::Instances), icons::CLOCK, mark("Instances"), 100.0),
        (Action::Pictures, icons::IMAGES, mark("Pictures"), 90.0),
        (Action::Talk, icons::MICROPHONE, mark("Ask"), 62.0),
        (Action::All, icons::SQUARES_FOUR, mark("All tools"), 90.0),
    ];
    // All tools has them, as large tiles, right under them.
    if facts.sheet == Some(Sheet::All) {
        return (Vec::new(), 10.0);
    }
    let busy = facts.view_showing || status_now(facts).is_some();
    // In a headset a tab is named or is not there: a row of small icons is nothing to aim a laser at.
    if !facts.desktop && busy {
        return (Vec::new(), 10.0);
    }
    let open = |action: Action| match (facts.sheet, action) {
        (Some(Sheet::Worlds), Action::Worlds) | (Some(Sheet::All), Action::All) | (Some(Sheet::Pictures), Action::Pictures) | (Some(Sheet::Talk), Action::Talk) => true,
        (Some(Sheet::Instances), Action::Page(Target::Instances)) | (Some(Sheet::Results), Action::Page(Target::YouTube)) => true,
        (Some(Sheet::Menu), Action::Page(Target::YouTube)) => facts.sheet_title == "YouTube",
        _ => false,
    };
    // A page that is none of them says its name after them: on a screen they make it room, as small icons. In a headset they are named, and the name has what is left.
    let unnamed = !TABS.iter().any(|t| open(t.0));
    let compact = busy || (unnamed && facts.desktop);
    let mut x = 10.0;
    let mut out = Vec::new();
    for (action, icon, label, wide) in TABS {
        let on = open(action);
        let label = tr_ctx("tile", label);
        // The Ask tab is cut for a word of three letters: where its name has more, it takes the room those need (English is as it was).
        let wide = if action == Action::Talk && label.chars().count() > 3 { wide + 10.0 } else { wide };
        let width = if compact && !on { 36.0 } else { wide };
        out.push(Item { action, rect: r(x, 7.0, width, 30.0), look: Look::HeadTab { icon, label, on } });
        x += width + 3.0;
    }
    (out, x)
}

/// The wing's tiles in a row: its mark, then the six.
fn strip(w: f32, h: f32, facts: &Facts) -> Vec<Item> {
    const PAD: f32 = 5.0;
    const MARK: f32 = 34.0;
    // What plays at its end, when anything does: the wing has it at its foot. And in a headset a way to put it away, as the wing has at its head.
    let hide = facts.vr_close && facts.sheet.is_none();
    let hide_wide = if hide { HIDE_TILES + GAP } else { 0.0 };
    let cells: f32 = if facts.music.is_some() { 7.7 } else { 6.0 };
    let width = (w - MARK - PAD * 2.0 - hide_wide - GAP * (cells.ceil() - 1.0)) / cells;
    let at = |c: f32| r(MARK + PAD + c * (width + GAP), PAD, width, h - PAD * 2.0);
    let tile = |action, icon, label, badge, rect| Item { action, rect, look: Look::Tile { icon, label, badge } };
    let mut items = vec![
        tile(Action::Worlds, icons::GLOBE_HEMISPHERE_WEST, tr("Worlds in 3D"), true, at(0.0)),
        tile(Action::Page(Target::YouTube), icons::YOUTUBE_LOGO, "YouTube", false, at(1.0)),
        tile(Action::Page(Target::Instances), icons::CLOCK, tr("Instances"), false, at(2.0)),
        tile(Action::Pictures, icons::IMAGES, tr("Pictures"), false, at(3.0)),
        Item { action: Action::Talk, rect: at(4.0), look: Look::Talk },
        tile(Action::All, icons::SQUARES_FOUR, tr_ctx("tile", "All tools"), false, at(5.0)),
    ];
    if facts.music.is_some() {
        items.push(Item { action: Action::PlayPause, rect: r(at(6.0).left(), PAD, width * 1.7, h - PAD * 2.0), look: Look::PlayingSmall });
    }
    if hide {
        items.push(Item { action: Action::Hide, rect: r(w - PAD - HIDE_TILES, PAD, HIDE_TILES, h - PAD * 2.0), look: Look::HideWord });
    }
    items
}

/// What the sheet's page holds, in N4N0's page's units (1024 x 576).
pub fn sheet_items(sheet: Sheet, facts: &Facts) -> Vec<Item> {
    let mut items = sheet_page_items(sheet, facts);
    if let Some((top, track, _)) = scroll_bar(sheet, facts) {
        items.push(Item { action: Action::ScrollBar, rect: r(1024.0 - SCROLL_BAR_GRAB, top, SCROLL_BAR_GRAB, track), look: Look::ScrollBar });
    }
    items
}

fn sheet_page_items(sheet: Sheet, facts: &Facts) -> Vec<Item> {
    match sheet {
        Sheet::All => {
            let (x0, y0, gap) = (24.0, 10.0, 12.0);
            let width = 1024.0 - x0 * 2.0;
            // Who comes here has the main features on the wing already, and came for the rest: those have real room and say what they do. The main features stay the largest.
            let small = accessories(facts);
            let (first_h, second_h) = (176.0, 170.0);
            let per_row = small.len().div_ceil(2).max(3);
            let small_rows = small.len().div_ceil(per_row) as f32;
            let small_gap = 8.0;
            let small_h = (576.0 - y0 * 2.0 - gap * 2.0 - first_h - second_h - gap - (small_rows - 1.0) * small_gap) / small_rows;
            // Worlds in 3D largest, then what people come for; what plays with all its controls, there, with no page of its own.
            let first_w = (width - gap * 2.0) * 0.42;
            let rest_w = (width - gap * 2.0 - first_w) / 2.0;
            let hero = |action, rect, icon, label, line, badge| Item { action, rect, look: Look::Hero { icon, label, line, badge, tint: tool_tint(action) } };
            let row2 = y0 + first_h + gap;
            let mut out = vec![
                hero(Action::Worlds, r(x0, y0, first_w, first_h), icons::GLOBE_HEMISPHERE_WEST, tr("Worlds in 3D"), tr("Search any world. Walk through the ones captured in 3D."), true),
                hero(Action::Page(Target::YouTube), r(x0 + first_w + gap, y0, rest_w, first_h), icons::YOUTUBE_LOGO, "YouTube", tr("Find a video for this world's player"), false),
                hero(Action::Page(Target::Instances), r(x0 + first_w + gap * 2.0 + rest_w, y0, rest_w, first_h), icons::CLOCK, tr("Instances"), tr("Where you were lately: go back"), false),
            ];
            let playing = r(x0, row2, first_w + gap + rest_w, second_h);
            out.push(Item { action: Action::Idle, rect: playing, look: Look::NowPlaying });
            out.extend(now_playing_controls(playing, facts));
            out.push(hero(Action::Pictures, r(playing.right() + gap, row2, rest_w, second_h), icons::IMAGES, tr("Pictures"), tr("Your pictures and 3D captures"), false));
            let top = row2 + second_h + gap;
            for (i, row) in small.chunks(per_row).enumerate() {
                let w = (width - small_gap * (row.len() as f32 - 1.0)) / row.len() as f32;
                let y = top + i as f32 * (small_h + small_gap);
                for (c, (action, icon, label)) in row.iter().enumerate() {
                    out.push(Item { action: *action, rect: r(x0 + c as f32 * (w + small_gap), y, w, small_h), look: Look::Small { icon, label, tint: tool_tint(*action) } });
                }
            }
            out
        }
        Sheet::Results => {
            let (columns, width, height, top) = video_grid();
            let mut out = Vec::new();
            for i in 0..facts.videos.len() {
                let (c, row) = ((i % columns) as f32, (i / columns) as f32);
                let y = top + row * (height + 12.0) - facts.gallery_scroll;
                if y + height < RESULTS_TOP || y > 576.0 {
                    continue;
                }
                let rect = r(24.0 + c * (width + 12.0), y, width, height);
                out.push(Item { action: Action::Video(i), rect, look: Look::Video(i) });
                // What the page is for first: what it puts in the world; its link beside it.
                let main = (rect.width() - 6.0) * 0.58;
                out.push(Item { action: Action::VideoDrop(i), rect: r(rect.left(), rect.bottom() - 42.0, main, 40.0), look: Look::VideoDrop });
                out.push(Item { action: Action::Video(i), rect: r(rect.left() + main + 6.0, rect.bottom() - 42.0, rect.width() - main - 6.0, 40.0), look: Look::VideoCopy(i) });
            }
            out.push(Item { action: Action::ResultsBack, rect: r(24.0, 8.0, 520.0, 44.0), look: Look::Back });
            out
        }
        Sheet::Talk => {
            let mut out = vec![Item { action: Action::TalkToggle, rect: r(24.0, 60.0, 320.0, 460.0), look: Look::TalkButton }];
            if !facts.talk_lines.is_empty() {
                out.push(Item { action: Action::NewConversation, rect: r(1024.0 - 24.0 - 170.0, 10.0, 170.0, 40.0), look: Look::NewTalk });
            }
            out
        }
        Sheet::Instances => {
            let (x0, y0, height, gap) = (24.0, VISIT_TOP, VISIT_HEIGHT, VISIT_GAP);
            let mut out = vec![Item { action: Action::Idle, rect: r(0.0, 0.0, 1024.0, VISIT_HEAD), look: Look::VisitHead }];
            for (i, visit) in facts.worlds_visited.iter().enumerate() {
                let y = y0 + i as f32 * (height + gap) - facts.gallery_scroll;
                // Only what shows is laid on the page.
                if y + height < VISIT_HEAD || y > 576.0 {
                    continue;
                }
                out.push(Item { action: Action::Idle, rect: r(x0, y, 1024.0 - x0 * 2.0, height), look: Look::Visit(i) });
                // The last time at the end, its large button; before it the times before, the newest first: each goes to that instance.
                let right = 1024.0 - x0 - 12.0;
                let width = INVITE_WIDTH;
                // A little under the row's middle: its one label lies over it.
                let top = y + (height - 50.0) / 2.0 + 5.0;
                // Where the person is now is nowhere to be invited to.
                if top >= VISIT_HEAD {
                    if visit.here {
                        out.push(Item { action: Action::Idle, rect: r(right - width, top, width, 50.0), look: Look::HereNow });
                    } else {
                        out.push(Item { action: Action::Invite(i, 0), rect: r(right - width, top, width, 50.0), look: Look::Chip { group: i, visit: 0 } });
                    }
                }
                let shown = visit.earlier.len().min(EARLIER_SHOWN);
                let left = right - width - 8.0 - shown as f32 * (EARLIER_WIDTH + 6.0);
                for n in (0..shown).filter(|_| top >= VISIT_HEAD) {
                    out.push(Item { action: Action::Invite(i, n + 1), rect: r(left + n as f32 * (EARLIER_WIDTH + 6.0), top + 2.0, EARLIER_WIDTH, 46.0), look: Look::Chip { group: i, visit: n + 1 } });
                }
            }
            out
        }
        Sheet::Pictures if facts.opened.is_some() => {
            let n = facts.opened.unwrap_or(0);
            let mut out = vec![
                Item { action: Action::Idle, rect: r(0.0, 0.0, 1024.0, 576.0), look: Look::BigPicture(n) },
                Item { action: Action::PictureBack, rect: r(20.0, 16.0, 188.0, 42.0), look: Look::BackToPictures },
            ];
            if picture_beside(facts, n, false).is_some() {
                out.push(Item { action: Action::PictureStep(false), rect: r(14.0, 262.0, 52.0, 52.0), look: Look::Step(false) });
            }
            if picture_beside(facts, n, true).is_some() {
                out.push(Item { action: Action::PictureStep(true), rect: r(958.0, 262.0, 52.0, 52.0), look: Look::Step(true) });
            }
            // On the desktop a capture turns here, and is walked in in the app's window.
            if facts.desktop && facts.recent.get(n).is_some_and(|r| r.kind == RecentKind::Capture) {
                out.push(Item { action: Action::OpenApp, rect: r(1024.0 - 20.0 - 316.0, 576.0 - 68.0, 316.0, 50.0), look: Look::OpenWindow });
            }
            out
        }
        Sheet::Pictures => {
            let (x0, gap, y0, h) = (24.0, 10.0, 8.0, GALLERY_TOP - 16.0);
            // One thin row, so that what is on it comes first: its three buttons, and at the end what it shows.
            let width = (1024.0 - x0 * 2.0 - gap * 2.0 - FILTER_ROOM) / 3.0;
            let mut out = Vec::new();
            // Under the buttons, and so under them for the pointer too.
            let (rects, _) = gallery_rects(facts);
            for (i, rect) in rects.iter().enumerate() {
                let rect = rect.translate(vec2(x0, GALLERY_TOP - facts.gallery_scroll));
                if rect.bottom() > GALLERY_TOP && rect.top() < 576.0 {
                    out.push(Item { action: Action::Capture(i), rect, look: Look::Capture(i) });
                }
            }
            out.push(Item { action: Action::Picture(PhotoMode::People), rect: r(x0, y0, width, h), look: Look::Picture(PhotoMode::People) });
            out.push(Item { action: Action::Picture(PhotoMode::World), rect: r(x0 + width + gap, y0, width, h), look: Look::Picture(PhotoMode::World) });
            out.push(Item { action: Action::Record, rect: r(x0 + (width + gap) * 2.0, y0, width, h), look: Look::Record });
            let track = filter_track();
            out.push(Item { action: Action::Filter(GalleryFilter::All), rect: r(track.left() + 4.0, y0 + 4.0, 62.0, h - 8.0), look: Look::Filter(GalleryFilter::All) });
            out.push(Item { action: Action::Filter(GalleryFilter::Captures), rect: r(track.left() + 68.0, y0 + 4.0, track.width() - 72.0, h - 8.0), look: Look::Filter(GalleryFilter::Captures) });
            out
        }
        Sheet::Settings => settings_items(facts),
        Sheet::Tracking => tracking_items(facts),
        Sheet::Menu | Sheet::Worlds => Vec::new(),
    }
}

/// What the gallery's filter takes of its top row, from the buttons on: it is another kind of thing, set apart with a line and a word, in a track of its own.
const FILTER_ROOM: f32 = 268.0;

/// The filter's track.
fn filter_track() -> Rect {
    Rect::from_min_max(pos2(1024.0 - 24.0 - 164.0, 8.0), pos2(1024.0 - 24.0, GALLERY_TOP - 8.0))
}

/// A picture, rounded: it is a picture for the retro look as well.
fn squircle_picture(p: &Painter, rect: Rect, radii: [f32; 4], id: TextureId, uv: Rect, tint: Color32) {
    keep_colour(p, rect);
    orbiters_ui::paint::squircle_texture(p, rect, look::picture_radii(radii), id, uv, tint);
}

thread_local! {
    /// What was painted as pictures since the last part was taken, in the points it was painted in.
    static PICTURES: std::cell::RefCell<Vec<Rect>> = const { std::cell::RefCell::new(Vec::new()) };
    /// What keeps its colours in any look: the colours to choose from, which would not be otherwise.
    static COLOURS: std::cell::RefCell<Vec<Rect>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Says that a picture lies here, as far as `p` shows it.
pub fn keep_colour(p: &Painter, rect: Rect) {
    let rect = rect.intersect(p.clip_rect());
    if rect.is_positive() {
        // A canvas whose frames are not taken (N4N0's own menu, say) must not fill this up.
        PICTURES.with(|l| {
            let mut list = l.borrow_mut();
            if list.len() < 256 {
                list.push(rect);
            }
        });
    }
}

/// A frame that changed nothing is not finished: what it said about its pictures must not reach the next part's.
pub fn forget_pictures() {
    PICTURES.with(|l| l.borrow_mut().clear());
    COLOURS.with(|l| l.borrow_mut().clear());
}

/// How luminance maps to the retro look's one colour: from nearly black through the colour to nearly white, as a phosphor does.
pub fn retro_ramp(color: [u8; 3]) -> Vec<[u8; 3]> {
    let mix = |a: f32, b: f32, t: f32| (a + (b - a) * t).round().clamp(0.0, 255.0) as u8;
    (0..256)
        .map(|i| {
            let l = i as f32 / 255.0;
            let mut out = [0u8; 3];
            for (c, slot) in out.iter_mut().enumerate() {
                let (dark, full, light) = (color[c] as f32 * 0.06, color[c] as f32, color[c] as f32 + (255.0 - color[c] as f32) * 0.72);
                // A curve at the dark end: the ground of a page stays nearly black, as on a phosphor screen.
                *slot = if l < 0.55 { mix(dark, full, (l / 0.55).powf(1.7)) } else { mix(full, light, (l - 0.55) / 0.45) };
            }
            out
        })
        .collect()
}

/// `pixels` (premultiplied RGBA) in `ramp`, in the region `changed` of a picture `width` across; their transparency stays.
pub fn in_ramp(pixels: &mut [u8], width: u32, changed: [u32; 4], ramp: &[[u8; 3]]) {
    for y in changed[1]..changed[3] {
        let (a, b) = (((y * width + changed[0]) * 4) as usize, ((y * width + changed[2]) * 4) as usize);
        let Some(row) = pixels.get_mut(a..b) else { return };
        for pixel in row.chunks_exact_mut(4) {
            let alpha = pixel[3] as u32;
            if alpha == 0 {
                continue;
            }
            let luma = (54 * pixel[0] as u32 + 183 * pixel[1] as u32 + 19 * pixel[2] as u32) >> 8;
            let shade = ramp[(luma * 255 / alpha).min(255) as usize];
            for c in 0..3 {
                pixel[c] = (shade[c] as u32 * alpha / 255) as u8;
            }
        }
    }
}

/// What the look chosen does to what was just painted: `out` holds it from `from`. With the retro look, in its one colour, but for its pictures if they keep theirs.
pub fn finish_picture(out: &mut [u8], from: &[u8], px: [u32; 2], zoom: f32, changed: [u32; 4]) {
    let pictures = PICTURES.with(|l| std::mem::take(&mut *l.borrow_mut()));
    let colours = COLOURS.with(|l| std::mem::take(&mut *l.borrow_mut()));
    let style = STYLE.lock().unwrap_or_else(|e| e.into_inner());
    let Some((ramp, keep)) = style.as_ref() else { return };
    let changed = [changed[0], changed[1], changed[2].min(px[0]), changed[3].min(px[1])];
    in_ramp(out, px[0], changed, ramp);
    let keeping = if *keep { pictures } else { Vec::new() };
    for rect in keeping.into_iter().chain(colours) {
        let (x0, x1) = (((rect.left() * zoom) as u32).max(changed[0]), ((rect.right() * zoom).ceil() as u32).min(changed[2]));
        for y in ((rect.top() * zoom) as u32).max(changed[1])..((rect.bottom() * zoom).ceil() as u32).min(changed[3]) {
            if x0 < x1 {
                let (a, b) = (((y * px[0] + x0) * 4) as usize, ((y * px[0] + x1) * 4) as usize);
                out[a..b].copy_from_slice(&from[a..b]);
            }
        }
    }
}

/// The retro look's ramp and whether pictures keep their colours: none with the modern look.
static STYLE: std::sync::Mutex<Option<(Vec<[u8; 3]>, bool)>> = std::sync::Mutex::new(None);

/// Takes what is chosen in Settings as how the parts look from now on.
pub fn set_style(o: &OrbitSettings) {
    let retro = o.theme == MenuLook::Retro;
    look::choose(o.accent, retro, !o.retro_pictures);
    *STYLE.lock().unwrap_or_else(|e| e.into_inner()) = retro.then(|| (retro_ramp(o.retro_color), o.retro_pictures));
}

/// How high a sheet's head is, above its page.
pub const SHEET_HEAD: f32 = 44.0;

/// How wide a list's scroll bar takes the pointer: 2 degrees at arm's length in the headset.
const SCROLL_BAR_GRAB: f32 = 34.0;

/// How many of a world's earlier visits its row has, and how wide each is.
const EARLIER_SHOWN: usize = 3;
const EARLIER_WIDTH: f32 = 100.0;
const INVITE_WIDTH: f32 = 136.0;

/// Where the gallery starts on its page, under its three buttons.
pub const GALLERY_TOP: f32 = 62.0;

/// The nearest thing to show after tile `n`, or before. A capture shows here on a screen, as a picture does; in a headset it shows around the person, so it is stepped over.
pub fn picture_beside(facts: &Facts, n: usize, after: bool) -> Option<usize> {
    let desktop = facts.desktop;
    let is_picture = |r: &Recent| r.kind == RecentKind::Picture || (desktop && r.kind == RecentKind::Capture);
    if after {
        facts.recent.get(n + 1..)?.iter().position(is_picture).map(|i| n + 1 + i)
    } else {
        facts.recent.get(..n)?.iter().rposition(is_picture)
    }
}

/// The gallery as the app's own Photos & captures lays it out, in the layout chosen there: each capture's place from the gallery's top, and its height. All 16:9 as the thumbnails are, so
/// that nothing moves as pictures arrive.
fn gallery_rects(facts: &Facts) -> (&[Rect], f32) {
    (&facts.recent.0.rects, facts.recent.0.height)
}

/// Under a result's picture: its title, who made it, and its two buttons.
const VIDEO_FOOT: f32 = 128.0;

/// Where YouTube's results start on their page, under the search they are of.
const RESULTS_TOP: f32 = 60.0;

/// YouTube's results' grid: columns, a tile's width and height, and where the first lies.
fn video_grid() -> (usize, f32, f32, f32) {
    let width = (1024.0 - 48.0 - 24.0) / 3.0;
    (3, width, width * 9.0 / 16.0 + VIDEO_FOOT, RESULTS_TOP + 4.0)
}

/// Which of YouTube's results show, scrolled this far: what fetches their pictures, and the next ones.
pub fn videos_shown(scroll: f32) -> std::ops::Range<usize> {
    let (columns, _, height, top) = video_grid();
    let row = (((scroll - top) / (height + 12.0)).floor().max(0.0)) as usize;
    (row * columns)..((row + 4) * columns)
}

/// Where the conversation is on the talking sheet, and how wide.
const TALK_LEFT: f32 = 368.0;
const TALK_TOP: f32 = 60.0;
const TALK_WIDTH: f32 = 1024.0 - TALK_LEFT - 24.0;

/// A line's height: as good as can be told with no painter to measure it with, and a little over.
fn talk_line_height(text: &str) -> f32 {
    let per_line = ((TALK_WIDTH * 0.86 - 28.0) / 9.0).max(10.0);
    let lines = (text.chars().count() as f32 / per_line).ceil().max(1.0);
    lines * 24.0 + 26.0 + 10.0
}

/// The Instances sheet's rows.
/// What stays over the rows as they scroll: one line on what they offer, and over their columns what those are.
const VISIT_HEAD: f32 = 40.0;
const VISIT_TOP: f32 = 44.0;
const VISIT_HEIGHT: f32 = 90.0;
const VISIT_GAP: f32 = 8.0;

/// Which rows of the Instances sheet show, scrolled this far, for what fetches their pictures.
pub fn visits_shown(scroll: f32) -> std::ops::Range<usize> {
    let first = ((scroll - VISIT_TOP) / (VISIT_HEIGHT + VISIT_GAP)).floor().max(0.0) as usize;
    first..first + (576.0 / (VISIT_HEIGHT + VISIT_GAP)).ceil() as usize + 1
}

/// Where a list comes to rest near `to`, its first row whole: a row's top, or its very end.
pub fn snap_scroll(sheet: Sheet, facts: &Facts, to: f32) -> f32 {
    let max = max_scroll(sheet, facts);
    if to >= max - 1.0 {
        return max.max(0.0);
    }
    let nearest = |tops: &mut dyn Iterator<Item = f32>| tops.map(|t| t.min(max)).min_by(|a, b| (a - to).abs().total_cmp(&(b - to).abs())).unwrap_or(to);
    match sheet {
        Sheet::Pictures => nearest(&mut gallery_rects(facts).0.iter().map(|r| r.top())),
        Sheet::Instances => nearest(&mut (0..facts.worlds_visited.len()).map(|i| i as f32 * (VISIT_HEIGHT + VISIT_GAP))),
        Sheet::Results => {
            let (_, _, height, _) = video_grid();
            nearest(&mut (0..facts.videos.len()).map(|i| i as f32 * (height + 12.0)))
        }
        _ => to,
    }
}

/// How far a sheet's list scrolls.
pub fn max_scroll(sheet: Sheet, facts: &Facts) -> f32 {
    match sheet {
        // A picture shown large leaves where the list was alone.
        Sheet::Pictures if facts.opened.is_some() => 0.0,
        Sheet::Pictures => (GALLERY_TOP + gallery_rects(facts).1 + 12.0 - 576.0).max(0.0),
        Sheet::Instances => (VISIT_TOP + facts.worlds_visited.len() as f32 * (VISIT_HEIGHT + VISIT_GAP) + 4.0 - 576.0).max(0.0),
        Sheet::Results => {
            let (columns, _, height, top) = video_grid();
            (top + facts.videos.len().div_ceil(columns) as f32 * (height + 12.0) + 8.0 - 576.0).max(0.0)
        }
        Sheet::Talk => (TALK_TOP + facts.talk_lines.iter().map(|(_, t)| talk_line_height(t)).sum::<f32>() + 8.0 - 576.0).max(0.0),
        _ => 0.0,
    }
}

/// The scroll bar of a sheet's list, when it has one: where its track starts, how long it is, how long its knob is.
pub fn scroll_bar(sheet: Sheet, facts: &Facts) -> Option<(f32, f32, f32)> {
    let max = max_scroll(sheet, facts);
    (max > 0.0).then(|| {
        let top = match sheet {
            Sheet::Pictures => GALLERY_TOP,
            Sheet::Results => RESULTS_TOP,
            Sheet::Talk => TALK_TOP,
            _ => 8.0,
        };
        let track = 576.0 - top - 8.0;
        (top, track, (track * track / (track + max)).max(48.0))
    })
}

/// Where the list goes with the scroll bar's knob held at `y`.
pub fn scroll_from_bar(sheet: Sheet, facts: &Facts, y: f32) -> f32 {
    scroll_bar(sheet, facts).map_or(0.0, |(top, track, knob)| ((y - top - knob / 2.0) / (track - knob).max(1.0)).clamp(0.0, 1.0) * max_scroll(sheet, facts))
}

/// Whether a press on this is the start of a drag as much as a press: what lies in a list that scrolls. It only acts if let go without moving.
pub fn scrolls_with_a_drag(sheet: Sheet, look: &Look) -> bool {
    matches!(sheet, Sheet::Pictures | Sheet::Instances | Sheet::Results) && matches!(look, Look::Capture(_) | Look::Visit(_) | Look::Chip { .. } | Look::Video(_) | Look::VideoDrop | Look::VideoCopy(_))
}

/// Where what plays's timeline lies on its panel, tall enough for a laser to find.
fn timeline_rect(panel: Rect) -> Rect {
    let x = panel.left() + panel.height();
    Rect::from_min_max(pos2(x, panel.top() + 72.0), pos2(panel.right() - 14.0, panel.top() + 108.0))
}

/// How far along what plays the pointer at `x` is, 0 to 1: `None` where there is no timeline to press.
pub fn timeline_fraction(facts: &Facts, x: f32) -> Option<f32> {
    let rect = sheet_items(Sheet::All, facts).into_iter().find(|i| matches!(i.look, Look::Timeline))?.rect;
    let bar = timeline_bar(rect);
    Some(((x - bar.left()) / bar.width().max(1.0)).clamp(0.0, 1.0))
}

/// The bar in a timeline's place: its time takes the end of it.
fn timeline_bar(rect: Rect) -> Rect {
    Rect::from_min_max(rect.min, pos2(rect.right() - 142.0, rect.bottom()))
}

/// A title as it should read, whatever a video's page separates its parts with.
pub fn clean_title(title: &str) -> String {
    title.split(['\\', '|']).map(str::trim).filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" \u{b7} ")
}

/// `seconds` as a clock's digits.
fn minutes(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// What plays's controls, on its panel on All tools: nothing about it needs a page of its own.
fn now_playing_controls(panel: Rect, facts: &Facts) -> Vec<Item> {
    let mut out = Vec::new();
    let x = panel.left() + panel.height();
    // On one line, all large enough for a laser: 3 degrees at arm's length and over.
    let y = panel.bottom() - 37.0;
    let at = |x: f32, side: f32| Rect::from_center_size(pos2(x + side / 2.0, y), Vec2::splat(side));
    if facts.music.is_some() {
        let glyph = if facts.music.as_ref().is_some_and(|m| m.1) { icons::PAUSE } else { icons::PLAY };
        out.push(Item { action: Action::Previous, rect: at(x, 50.0), look: Look::Transport(icons::SKIP_BACK) });
        out.push(Item { action: Action::PlayPause, rect: at(x + 56.0, 58.0), look: Look::Transport(glyph) });
        out.push(Item { action: Action::Next, rect: at(x + 120.0, 50.0), look: Look::Transport(icons::SKIP_FORWARD) });
    }
    if facts.music_times.is_some_and(|t| t.1 > 0) {
        out.push(Item { action: if facts.can_seek { Action::Timeline } else { Action::Idle }, rect: timeline_rect(panel), look: Look::Timeline });
    }
    if let Some((_, muted)) = facts.volume {
        let right = panel.right() - 16.0;
        out.push(Item { action: Action::Volume(true), rect: at(right - 48.0, 48.0), look: Look::Transport(icons::PLUS) });
        out.push(Item { action: Action::Volume(false), rect: at(right - 48.0 - 78.0 - 48.0, 48.0), look: Look::Transport(icons::MINUS) });
        out.push(Item { action: Action::Mute, rect: at(right - 48.0 - 78.0 - 48.0 - 54.0, 48.0), look: Look::Transport(if muted { icons::SPEAKER_X } else { icons::SPEAKER_HIGH }) });
    }
    out
}

/// Each tool's colour on All tools, from the app's own palette, so that the eye finds a tool by its colour and its icon before reading (Enzo: "very gray and hard to parse"). Those
/// next to each other differ.
fn tool_tint(action: Action) -> Color32 {
    match action {
        Action::Worlds | Action::Talk | Action::FastWalk => look::accent(),
        Action::Page(Target::Instances) | Action::Page(Target::Call) | Action::Page(Target::Tracking) => theme::BLUE,
        Action::Page(Target::YouTube) | Action::Page(Target::WorldFeedback) => theme::DANGER,
        Action::Page(Target::Music) => theme::VIOLET,
        Action::Pictures => theme::WARN,
        Action::Page(Target::Qr) => theme::INDIGO,
        Action::Page(Target::States) => theme::PURPLE,
        Action::Page(Target::Light) | Action::Page(Target::Suggest) => theme::WARN,
        _ => theme::MARK,
    }
}

/// What a large tile of All tools holds under its name.
fn hero_body(p: &Painter, body: Rect, action: Action, line: &str, facts: &Facts) {
    let words = |p: &Painter, at: Pos2, text: &str, rows: usize, width: f32| {
        let mut job = egui::text::LayoutJob::simple(text.to_string(), look::font(16.5, weight::MEDIUM), look::MUTED, width);
        job.wrap.max_rows = rows;
        job.wrap.overflow_character = Some('\u{2026}');
        p.galley(at, p.layout_job(job), look::MUTED);
    };
    let radii = [theme::squircle(10.0); 4];
    match action {
        Action::Worlds if !facts.hero_captures.is_empty() => {
            // The newest large, the two before it beside it: the place itself says what this is.
            let said = if body.height() > 118.0 { 48.0 } else { 0.0 };
            let height = (body.height() - said).max(40.0);
            let captures = &facts.hero_captures;
            let side = if captures.len() >= 3 { (height - 8.0) / 2.0 * 16.0 / 9.0 } else { 0.0 };
            let large = Rect::from_min_size(body.min, vec2(if side > 0.0 { body.width() - side - 8.0 } else { body.width() }, height));
            squircle_picture(p, large, radii, captures[0], cover_uv(large.size()), Color32::WHITE);
            if side > 0.0 {
                for (i, id) in captures.iter().skip(1).take(2).enumerate() {
                    let h = (height - 8.0) / 2.0;
                    let tile = Rect::from_min_size(pos2(large.right() + 8.0, body.top() + i as f32 * (h + 8.0)), vec2(side, h));
                    squircle_picture(p, tile, radii, *id, cover_uv(tile.size()), Color32::WHITE);
                }
            }
            if said > 0.0 {
                words(p, pos2(body.left(), body.top() + height + 6.0), line, 2, body.width());
            }
        }
        Action::Page(Target::Instances) if !facts.worlds_visited.is_empty() => {
            let row = ((body.height() + 6.0) / 3.0).clamp(36.0, 56.0);
            let rows = ((body.height() + 6.0) / row).floor().max(1.0) as usize;
            for (i, visit) in facts.worlds_visited.iter().take(rows).enumerate() {
                let y = body.top() + i as f32 * row;
                let side = row - 6.0;
                let tile = Rect::from_min_size(pos2(body.left(), y), vec2(side * 16.0 / 9.0, side));
                match visit.picture {
                    Some(id) => squircle_picture(p, tile, [theme::squircle(7.0); 4], id, cover_uv(tile.size()), Color32::WHITE),
                    None => {
                        p.rect_filled(tile, look::radius(7.0), look::HEADER);
                    }
                }
                let x = tile.right() + 10.0;
                let font = look::font(16.0, weight::SEMIBOLD);
                p.text(pos2(x, y + side / 2.0), Align2::LEFT_CENTER, fit(p, &visit.name, &font, body.right() - x), font, look::LABEL);
            }
        }
        Action::Pictures if facts.hero_picture.is_some() => {
            let height = body.height() - 26.0;
            let tile = Rect::from_min_size(body.min, vec2(body.width(), height.max(30.0)));
            if let Some(id) = facts.hero_picture {
                squircle_picture(p, tile, radii, id, cover_uv(tile.size()), Color32::WHITE);
            }
            let font = look::font(15.5, weight::MEDIUM);
            p.text(pos2(body.left(), body.bottom() - 9.0), Align2::LEFT_CENTER, fit(p, line, &font, body.width()), font, look::MUTED);
        }
        Action::Page(Target::YouTube) => {
            // What is in this world's player, as itself where its picture is known; and at its foot where this leads, as large as a button is.
            let foot = 46.0f32.min(body.height() * 0.42);
            let search = Rect::from_min_max(pos2(body.left(), body.bottom() - foot), body.max);
            p.rect_filled(search, look::radius(foot / 2.0), theme::mix(look::TILE, tool_tint(action), 0.6));
            let font = look::font(18.0, weight::SEMIBOLD);
            let wide = p.layout_no_wrap(tr("Search YouTube").to_string(), font.clone(), look::LABEL).size().x;
            let left = search.center().x - (wide + 28.0) / 2.0;
            widgets::icon(p, pos2(left + 10.0, search.center().y), icons::MAGNIFYING_GLASS, 20.0, look::LABEL);
            p.text(pos2(left + 28.0, search.center().y), Align2::LEFT_CENTER, tr("Search YouTube"), font, look::LABEL);
            let room = Rect::from_min_max(body.min, pos2(body.right(), search.top() - 8.0));
            let lines = |p: &Painter, at: Pos2, words: &str, size: f32, w: f32, ink: Color32, rows: usize| -> f32 {
                let mut job = egui::text::LayoutJob::simple(words.to_string(), look::font(size, w), ink, room.width() - 20.0);
                job.wrap.max_rows = rows;
                job.wrap.overflow_character = Some('\u{2026}');
                let galley = p.layout_job(job);
                let high = galley.size().y;
                p.galley(at, galley, ink);
                high
            };
            match (&facts.world_video, facts.world_video_picture) {
                (Some(title), Some(id)) => {
                    squircle_picture(p, room, radii, id, cover_uv(room.size()), Color32::WHITE);
                    let points = orbiters_ui::paint::squircle_points_corners(room, radii);
                    orbiters_ui::paint::fill_gradient(p, &points, &orbiters_ui::paint::Gradient::vertical(room, &[(0.0, theme::alpha(theme::INK_900, 0.55)), (1.0, theme::alpha(theme::INK_900, 0.92))]));
                    lines(p, pos2(room.left() + 10.0, room.top() + 3.0), title, 16.5, weight::SEMIBOLD, Color32::WHITE, ((room.height() / 19.0).floor() as usize).max(1));
                }
                (Some(title), None) => {
                    lines(p, room.min, title, 18.0, weight::SEMIBOLD, look::LABEL, ((room.height() / 21.0).floor() as usize).max(1));
                }
                (None, _) => {
                    lines(p, room.min, line, 18.0, weight::SEMIBOLD, look::LABEL, ((room.height() / 21.0).floor() as usize).max(1));
                }
            }
        }
        _ => {
            let rows = (body.height() / 22.0).floor().max(1.0) as usize;
            words(p, body.min, line, rows, body.width());
        }
    }
}

/// A sheet's title in the language now. It is kept in English while the sheet is open. Two are words that mean something else on their own: "Call" as a button is to call, and as a page it is the call.
fn sheet_name(title: &str) -> &str {
    match title {
        "Call" => tr_ctx("voice call", title), // i18n: not shown
        "Light" => tr_ctx("lamp", title), // i18n: not shown
        _ => tr(title),
    }
}

/// What a sheet is for, in a few words.
fn sheet_says(facts: &Facts) -> &'static str {
    match facts.sheet {
        Some(Sheet::Worlds) => tr("Search any world, and walk in those captured in 3D."),
        Some(Sheet::Pictures) => tr("Your pictures and 3D captures."),
        Some(Sheet::Instances) => tr("Where you were lately. Go back with an invite."),
        Some(Sheet::Results) => tr("Copy a link for the world's player, or drop it in the world."),
        Some(Sheet::Talk) => tr("Press the button, and ask N4N0 anything."),
        Some(Sheet::Settings) => tr("How these menus lie, move, sound and look."),
        Some(Sheet::Tracking) => tr("Less jitter, a little more lag, in milliseconds."),
        Some(Sheet::All) => tr("Everything Orbiters can do from here."),
        _ => tr("Orbiters, beside VRChat's menu."),
    }
}

/// The icon and colour of what a sheet is.
fn sheet_mark(facts: &Facts) -> (&'static str, Color32) {
    match facts.sheet {
        Some(Sheet::Worlds) => (icons::GLOBE_HEMISPHERE_WEST, tool_tint(Action::Worlds)),
        Some(Sheet::Pictures) => (icons::IMAGES, tool_tint(Action::Pictures)),
        Some(Sheet::Instances) => (icons::CLOCK, tool_tint(Action::Page(Target::Instances))),
        Some(Sheet::Results) => (icons::YOUTUBE_LOGO, tool_tint(Action::Page(Target::YouTube))),
        Some(Sheet::Talk) => (icons::MICROPHONE, look::LABEL),
        Some(Sheet::Settings) => (icons::GEAR_SIX, look::LABEL),
        Some(Sheet::Tracking) => (icons::VIRTUAL_REALITY, tool_tint(Action::Page(Target::Tracking))),
        Some(Sheet::All) => (icons::SQUARES_FOUR, look::LABEL),
        Some(Sheet::Menu) if facts.sheet_title == "YouTube" => (icons::YOUTUBE_LOGO, tool_tint(Action::Page(Target::YouTube))),
        _ => (icons::N4N0_MARK, look::LABEL),
    }
}

/// What is on the item at `p` (panel units): its index. The small close button lies on top of what it sits on.
pub fn hit(items: &[Item], p: Pos2) -> Option<usize> {
    items.iter().enumerate().rev().find(|(_, i)| i.rect.contains(p)).map(|(i, _)| i)
}

/// What the caption says of what is pointed at, in words for someone who has never seen any of this: a title and what it does.
pub fn caption(action: Action, facts: &Facts) -> Option<(String, String)> {
    if let Some(said) = settings_caption(action).or_else(|| tracking_caption(action)) {
        return Some(said);
    }
    let (a, b): (String, String) = match action {
        // Said above.
        Action::Settings => return None,
        Action::Peek => (tr("Hide Orbiters").into(), tr("Reach VRChat's own menus. Nothing here closes.").into()),
        Action::Worlds => (tr("Worlds in 3D").into(), tr("Search any world. Walk in those captured in 3D.").into()),
        Action::Picture(_) if facts.no_kit => (tr("This avatar has no capture kit").into(), tr("Add N4N0 to it in Orbiters, My Avatar, to take 3D pictures with it.").into()),
        Action::Picture(_) if facts.picture_wait.is_some() => (if facts.digits && facts.sheet.is_none() { tr("Press 6 to call it off") } else { tr("Press to call it off") }.into(), tr("Or open VRChat's camera: it then starts by itself.").into()),
        Action::Picture(mode) => {
            if let Some(s) = facts.countdown {
                (tr!("3D picture in {seconds}", seconds = s), tr("VRChat's camera then flies around by itself.").into())
            } else if facts.picture_flying {
                (tr("3D picture in progress").into(), tr("The camera is flying. Stay where you are for a moment.").into())
            } else if mode == PhotoMode::People {
                (tr("3D picture of people").into(), tr("Keeps those around you in 3D: look around them afterwards.").into())
            } else {
                (tr("3D picture of this world").into(), tr("Keeps this place in 3D, without you in it: walk in it afterwards.").into())
            }
        }
        Action::Invite(group, visit) => {
            let place = facts.worlds_visited.get(group);
            let when = place.and_then(|v| if visit == 0 { None } else { v.earlier.get(visit - 1).map(|e| e.1.clone()) });
            (tr("Invite me").into(), when.map_or_else(|| tr("Orbiters invites you to the instance you were last in here.").to_string(), |w| tr!("To the instance you were in {when}.", when = w)))
        }
        Action::Volume(up) => (if up { tr("Louder") } else { tr("Quieter") }.into(), tr("Your computer's volume.").into()),
        Action::Mute => (tr("Mute").into(), tr("Your computer's sound, on or off.").into()),
        Action::Pictures => (tr("Pictures").into(), tr("Take a 3D picture, record a scene, see your captures.").into()),
        Action::All => (tr("All tools").into(), tr("Everything Orbiters can do from here.").into()),
        Action::Record if facts.recording => (tr("Stop recording").into(), tr("Orbiters then turns it into a 3D scene.").into()),
        Action::Record => (tr("Record a 3D scene").into(), tr("Walk around with VRChat's camera; Orbiters makes a 3D scene of what it saw.").into()),
        Action::Page(Target::Instances) => (tr("Instances").into(), tr("Where you were lately. Go back with an invite.").into()),
        Action::Page(Target::History) => (tr("Your captures").into(), tr("Your 3D pictures and scenes: see them, share them.").into()),
        Action::Page(Target::YouTube) => ("YouTube".into(), tr("Find a video, and put its link in the world's video player.").into()),
        Action::Page(Target::Call) => (tr_ctx("voice call", "Call").into(), tr("Talk with a friend, wherever they are in VRChat.").into()),
        Action::Page(Target::Music) => (tr("Now playing").into(), tr("What your computer plays, and its volume.").into()),
        Action::Page(Target::Light) => (tr_ctx("lamp", "Light").into(), tr("N4N0 as a light that follows you.").into()),
        Action::Page(Target::States) => (tr("Saved looks").into(), tr("Your avatar's saved looks, applied in one go.").into()),
        Action::Page(Target::Tracking) => (tr("Smooth tracking").into(), tr("Steadier headset, controllers and trackers.").into()),
        Action::Page(Target::WorldFeedback) => (tr("World feedback").into(), tr("Like this world, label it, comment on it.").into()),
        Action::Page(Target::Qr) => (tr("QR code").into(), tr("Open a link shown in the world, through VRChat's camera.").into()),
        Action::Page(Target::Suggest) => (tr("Suggest").into(), tr("Tell the Orbiters team what you would like.").into()),
        Action::Page(_) => return None,
        Action::Set(_) => return None,
        Action::FastWalk => (tr("Fast walk").into(), if let Some(words) = facts.fast_walk_lesser.filter(|_| facts.fast_walk) { tr(words).to_string() } else if facts.fast_walk { tr("On: you walk faster. Press to turn it off.").into() } else { tr("Walk faster than VRChat lets you.").to_string() }),
        Action::PlayPause => match &facts.music {
            Some((title, playing)) => (title.clone(), if *playing { tr("Playing on your computer. Press to pause.").into() } else { tr("Paused on your computer. Press to play.").to_string() }),
            None => (tr("Nothing playing").into(), tr("What your computer plays shows here.").into()),
        },
        Action::Previous => (tr_ctx("track", "Previous").into(), tr("The previous track on your computer.").into()),
        Action::Next => (tr_ctx("track", "Next").into(), tr("The next track on your computer.").into()),
        Action::TalkToggle => match facts.talk {
            TalkNow::Listening(_) => (tr("Listening").into(), tr("Press when you are done.").into()),
            _ => (tr("Talk to N4N0").into(), tr("Press, speak, and press again.").into()),
        },
        Action::NewConversation => (tr("New conversation").into(), tr("N4N0 starts over.").into()),
        Action::HideView => (tr("Hide 3D").into(), tr("The capture around you goes.").into()),
        Action::SmallView(true) => (tr_ctx("view", "Small").into(), tr("Before you: grip it to take it, its hand's stick zooms.").into()),
        Action::SmallView(false) => (tr("Around you").into(), tr("Stand in it again.").into()),
        Action::Capture(n) => {
            let recent = facts.recent.get(n)?;
            let says = match recent.kind {
                RecentKind::Capture if facts.desktop => tr("Press to turn it in 3D, right here."),
                RecentKind::Capture => tr("Press to see it in 3D around you."),
                RecentKind::NoView => tr("It has no 3D yet. The Orbiters window can make it."),
                RecentKind::Picture => tr("Press to see it large."),
            };
            (recent.title.clone(), says.into())
        }
        Action::Filter(GalleryFilter::All) => (tr("Everything").into(), tr("VRChat's pictures and your 3D captures together.").into()),
        Action::Filter(GalleryFilter::Captures) => (tr("Only 3D").into(), tr("Only your 3D pictures and recordings.").into()),
        Action::PictureBack => (tr("Back").into(), tr("To the gallery.").into()),
        Action::PictureStep(after) => (if after { tr("Next picture") } else { tr("Previous picture") }.into(), tr("Pictures only: captures show in 3D.").into()),
        Action::Timeline => (tr("Move in what plays").into(), tr("Press or drag along it.").into()),
        Action::ScrollBar => (tr("Scroll").into(), tr("Hold and move it.").into()),
        Action::Video(i) => (facts.videos.get(i).map_or(String::new(), |v| v.title.clone()), tr("Press to copy its link, for the world's video player.").into()),
        Action::VideoDrop(_) => (tr("Drop in world").into(), tr("Puts a card with the link in front of you, for anyone to grab.").into()),
        Action::ResultsBack => (tr("Search again").into(), tr("Back to the search, to change what you looked for.").into()),
        Action::Talk if facts.n4n0 => (tr("Ask N4N0").into(), tr("Your assistant in VRChat: talk to it, and see all it said.").into()),
        Action::Talk => (tr("Ask N4N0").into(), tr("Turn N4N0 on in Orbiters first.").into()),
        Action::OpenApp => (tr("Open Orbiters").into(), tr("The full app, on your computer.").into()),
        Action::CloseSheet => (tr("Close").into(), tr("Back to VRChat's menu.").into()),
        Action::Hide => (tr_ctx("menu tiles", "Hide").into(), tr("Hides these until VRChat's menu opens again.").into()),
        Action::Idle => return None,
    };
    Some((a, b))
}

fn text(p: &Painter, at: Pos2, align: Align2, s: &str, size: f32, w: f32, c: Color32) {
    p.text(at, align, s, look::font(size, w), c);
}

/// `s` cut with an ellipsis so that it fits `width`.
fn fit(p: &Painter, s: &str, font: &FontId, width: f32) -> String {
    let wide = |t: &str| p.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x;
    if wide(s) <= width {
        return s.to_string();
    }
    let mut cut: Vec<char> = s.chars().collect();
    while !cut.is_empty() {
        cut.pop();
        let t: String = cut.iter().collect::<String>() + "…";
        if wide(&t) <= width {
            return t;
        }
    }
    String::new()
}


/// A label in VRChat's manner: bold, with a soft dark edge, readable on anything behind it.
fn bold(p: &Painter, at: Pos2, align: Align2, s: &str, size: f32, c: Color32) {
    p.text(at + vec2(0.0, 1.0), align, s, look::font(size, weight::BOLD), Color32::from_black_alpha(140));
    p.text(at, align, s, look::font(size, weight::BOLD), c);
}

/// A little "3D" on an icon's corner.
fn badge(p: &Painter, icon_at: Pos2, icon_size: f32, fill: Color32, ink: Color32) {
    let b = Rect::from_center_size(icon_at + vec2(icon_size * 0.58, -icon_size * 0.36), vec2(20.0, 13.0));
    p.rect_filled(b, look::radius(4.0), fill);
    text(p, b.center(), Align2::CENTER_CENTER, "3D", 9.0, weight::BOLD, ink);
}

/// The time in a recording's or a countdown's form.
fn clock(seconds: u32) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// The shape something is drawn into: a rectangle, or a cell of a ring (where to centre, how wide it is there, and where the wheel's centre lies from the panel's corner).
#[derive(Clone, Copy)]
struct Shaped {
    at: Pos2,
    room: f32,
    height: f32,
    ring: Option<(Ring, Pos2)>,
}

fn shaped(zone: Zone, rect: Rect) -> Shaped {
    match zone.ring() {
        Some(ring) => {
            let center = (-ring.bounds().min.to_vec2()).to_pos2();
            Shaped { at: center + ring.anchor(), room: ring.room(), height: ring.outer - ring.inner, ring: Some((ring, center)) }
        }
        None => Shaped { at: rect.center(), room: rect.width() - 8.0, height: rect.height(), ring: None },
    }
}

/// A cell of the ring: Orbiters' surface, a little lighter toward the wheel. `lift` takes it toward the accent (under the pointer).
fn ring_fill(p: &Painter, ring: Ring, center: Pos2, lift: f32) {
    let toward = |c: Color32| theme::mix(c, look::accent(), lift * 0.5);
    let (near, far) = (toward(look::CELL_NEAR), toward(look::CELL_FAR));
    ring_mesh(p, ring, center, near, far);
}

fn ring_mesh(p: &Painter, ring: Ring, center: Pos2, near: Color32, far: Color32) {
    let mut mesh = Mesh::default();
    let (a0, a1) = (ring.from - 2.0, ring.to + 2.0);
    let (steps_a, steps_r) = (((a1 - a0) / 3.0) as u32 + 1, 6u32);
    for i in 0..=steps_a {
        for j in 0..=steps_r {
            let t = j as f32 / steps_r as f32;
            let radius = ring.inner - 2.0 + (ring.outer - ring.inner + 4.0) * t;
            let angle = a0 + (a1 - a0) * i as f32 / steps_a as f32;
            mesh.vertices.push(Vertex { pos: center + zones::polar(radius, angle), uv: WHITE_UV, color: theme::mix(near, far, t) });
        }
    }
    let row = steps_r + 1;
    for i in 0..steps_a {
        for j in 0..steps_r {
            let (a, b, c, d) = (i * row + j, i * row + j + 1, (i + 1) * row + j, (i + 1) * row + j + 1);
            mesh.indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    p.add(Shape::mesh(mesh));
}

/// The accent along a cell's outer edge, under the pointer.
fn ring_rim(p: &Painter, ring: Ring, center: Pos2, on: bool) {
    if !on && look::retro() {
        // Line work: all round every cell, as round anything of the retro menu's that can be pressed.
        cell_edge(p, ring, center, 1.5, super::retro::LINE);
        return;
    }
    if !on {
        // At rest a hairline of Orbiters' own along its outer edge: it follows VRChat's wheel, and it is not VRChat's.
        let line: Vec<Pos2> = (0..=16).map(|i| center + zones::polar(ring.outer - 1.0, ring.from + (ring.to - ring.from) * i as f32 / 16.0)).collect();
        p.add(Shape::line(line, Stroke::new(1.5, theme::alpha(look::accent(), 0.5))));
        return;
    }
    // All the way round it: which cell it is cannot be in doubt, whatever VRChat lights near it.
    cell_edge(p, ring, center, 3.0, look::accent());
}

/// An arc along a cell's outer edge, for what is under way: `fraction` of the way across it.
fn progress_arc(p: &Painter, ring: Ring, center: Pos2, fraction: f32, color: Color32) {
    let radius = ring.outer - 4.5;
    let (from, span) = (ring.from + 2.0, (ring.to - ring.from - 4.0) * fraction.clamp(0.0, 1.0));
    let steps = 24;
    if span > 0.0 {
        p.add(Shape::line((0..=steps).map(|i| center + zones::polar(radius, from + span * i as f32 / steps as f32)).collect::<Vec<Pos2>>(), Stroke::new(3.0, color)));
    }
}

/// A line round a cell, a little inside its edge.
fn cell_edge(p: &Painter, ring: Ring, center: Pos2, width: f32, color: Color32) {
    let inset = width / 2.0;
    let (r0, r1) = (ring.inner + inset, ring.outer - inset);
    let steps = 16;
    let at = |r: f32, t: f32| center + zones::polar(r, ring.from + (ring.to - ring.from) * t);
    let mut line: Vec<Pos2> = (0..=steps).map(|i| at(r1, i as f32 / steps as f32)).collect();
    line.extend((0..=steps).rev().map(|i| at(r0, i as f32 / steps as f32)));
    p.add(Shape::closed_line(line, Stroke::new(width, color)));
}

/// Draws a part (in units, its corner at the painter's origin): its fill and everything on it.
pub fn paint(p: &Painter, zone: Zone, size: Vec2, facts: &Facts, hover: Option<usize>, pressed: Option<usize>) {
    let panel = Rect::from_min_size(pos2(0.0, 0.0), size);
    match zone {
        Zone::WheelVeil => {
            // Over VRChat's wheel while the ring is what is pointed at: smoked, and what is pointed at lies in it, right beside where the eyes are.
            let c = panel.center();
            // Opaque while it says something: VRChat's icons would show behind the words.
            let says = facts.picture_wait.is_some() || facts.pointed.and_then(|a| caption(a, facts)).is_some();
            p.circle_filled(c, size.x / 2.0, if says { Color32::from_rgb(11, 13, 16) } else { Color32::from_rgba_premultiplied(9, 11, 14, 236) });
            draw_hub(p, c, size.x, facts);
            return;
        }
        Zone::WingName => {
            // The name on a pill of its own, with what runs when it does.
            // Its Hide is a pill of its own beside it: the name is its mark then, and each has its room.
            let hide = facts.vr_close && facts.sheet.is_none();
            p.rect_filled(if hide { Rect::from_min_max(panel.min, pos2(size.x - HIDE_TILES - 4.0, size.y)) } else { panel }, look::radius(size.y / 2.0), look::PANEL);
            widgets::icon(p, pos2(19.0, size.y / 2.0), icons::N4N0_MARK, 16.0, look::LABEL);
            if !hide {
                text(p, pos2(32.0, size.y / 2.0), Align2::LEFT_CENTER, "Orbiters N4N0", 13.0, weight::SEMIBOLD, look::LABEL);
            }
            let right = size.x - if hide { HIDE_TILES + 12.0 } else { 10.0 };
            if facts.recording {
                text(p, pos2(right, size.y / 2.0), Align2::RIGHT_CENTER, &clock(facts.recording_seconds.unwrap_or(0)), 11.0, weight::SEMIBOLD, theme::REC);
            } else if let Some(done) = facts.reconstructing {
                text(p, pos2(right, size.y / 2.0), Align2::RIGHT_CENTER, &format!("3D {:.0}%", done * 100.0), 11.0, weight::SEMIBOLD, look::accent());
            }
        }
        Zone::QmStrip => {
            p.rect_filled(panel, look::radius(look::RADIUS), look::PANEL);
            top_light(p, panel);
            widgets::icon(p, pos2(20.0, size.y / 2.0), icons::N4N0_MARK, 20.0, look::LABEL);
        }
        Zone::SheetHint => {
            // Over VRChat's tooltip for what the sheet hides, where one appears.
            p.rect_filled(panel, look::radius(panel.height() / 2.0), look::OPAQUE);
            let (first, second) = facts.pointed.and_then(|a| caption(a, facts)).unwrap_or_else(|| (sheet_name(&facts.sheet_title).to_string(), sheet_says(facts).to_string()));
            let font = look::font(13.5, weight::MEDIUM);
            let words = format!("{first}: {second}");
            p.text(panel.center(), Align2::CENTER_CENTER, fit(p, &words, &font, panel.width() - 28.0), font, look::LABEL);
        }
        Zone::SheetBack => {
            match facts.sheet_hole {
                Some(hole) => super::super::worlds::panel::fill_around(p, panel, look::RADIUS + 2.0, hole, 10.0, look::OPAQUE),
                None => {
                    p.rect_filled(panel, look::radius(look::RADIUS + 2.0), look::OPAQUE);
                }
            }
            // Its head has the colour of what it is, as on All tools.
            let (glyph, tint) = sheet_mark(facts);
            let round = (look::RADIUS + 2.0) as u8;
            // Each sheet's head has its tool's colour, as All tools has it: the eye finds a tool by it, there and here. What has none of its own has the accent, not a grey.
            let band = if tint == theme::MARK { look::accent() } else { tint };
            p.rect_filled(Rect::from_min_max(panel.min, pos2(panel.right(), panel.top() + SHEET_HEAD)), egui::CornerRadius { nw: round, ne: round, sw: 0, se: 0 }, theme::mix(theme::alpha(tint, 0.11), band, 0.16));
            p.rect_filled(Rect::from_min_size(pos2(panel.left(), panel.top() + SHEET_HEAD - 2.0), vec2(panel.width(), 2.0)), 0.0, theme::alpha(band, 0.55));
            top_light(p, panel);
            // The tools along it say which page this is; a page of N4N0's own says so after them.
            let (tabs, mut x) = head_tabs(facts);
            if !tabs.iter().any(|t| matches!(t.look, Look::HeadTab { on: true, .. })) {
                widgets::icon(p, pos2(x + 10.0, 22.0), glyph, 21.0, tint);
                x += 28.0;
                let font = look::font(18.0, weight::SEMIBOLD);
                let title = fit(p, sheet_name(&facts.sheet_title), &font, size.x - 50.0 - x);
                x += p.layout_no_wrap(title.clone(), font.clone(), Color32::WHITE).size().x;
                p.text(pos2(x - p.layout_no_wrap(title.clone(), font.clone(), Color32::WHITE).size().x, 22.0), Align2::LEFT_CENTER, title, font, look::LABEL);
            }
            // Well clear of them, in a column of its own.
            let from = x + 12.0;
            // The head is where this sheet says what is going on: nothing lies over or under it.
            if let Some((first, second)) = status_now(facts) {
                let room = size.x - 50.0 - if facts.view_showing { 206.0 } else { 0.0 } - from;
                let big = look::font(13.0, weight::BOLD);
                let small = look::font(11.5, weight::MEDIUM);
                p.text(pos2(from, if second.is_empty() { 22.0 } else { 14.0 }), Align2::LEFT_CENTER, fit(p, &first, &big, room), big, look::accent());
                p.text(pos2(from, 31.0), Align2::LEFT_CENTER, fit(p, &second, &small, room), small, look::LABEL);
            // What is pointed at is said in one place: where the band under the tabs is up, there.
            } else if let Some((first, second)) = facts.pointed.filter(|_| !facts.hint_band && matches!(facts.sheet, Some(Sheet::All | Sheet::Pictures))).and_then(|a| caption(a, facts)) {
                // What a small tile does, which it does not say itself.
                let room = size.x - 50.0 - if facts.view_showing { 206.0 } else { 0.0 } - from;
                let big = look::font(13.0, weight::BOLD);
                let small = look::font(11.5, weight::MEDIUM);
                p.text(pos2(from, 14.0), Align2::LEFT_CENTER, fit(p, &first, &big, room), big, look::LABEL);
                p.text(pos2(from, 31.0), Align2::LEFT_CENTER, fit(p, &second, &small, room), small, look::MUTED);
            } else if let (Some((name, playing)), false, false) = (facts.music.as_ref().filter(|_| !facts.view_showing), facts.beside_wing, facts.sheet == Some(Sheet::All)) {
                let font = look::font(12.0, weight::SEMIBOLD);
                let right = size.x - 56.0 - 26.0 * 3.0 - 8.0;
                let name = fit(p, name, &font, (right - from).max(0.0));
                p.text(pos2(right, 22.0), Align2::RIGHT_CENTER, name, font, if *playing { look::LABEL } else { look::MUTED });
            }
        }
        _ => {}
    }
    for (i, item) in items(zone, size, facts).iter().enumerate() {
        draw(p, zone, item, facts, hover == Some(i), pressed == Some(i));
    }
    // A list goes on under its foot: it fades there, and is not cut.
    if zone == Zone::Takeover && facts.opened.is_none() {
        if let Some(sheet) = facts.sheet.filter(|s| matches!(s, Sheet::Pictures | Sheet::Instances | Sheet::Results)) {
            if facts.gallery_scroll < max_scroll(sheet, facts) - 1.0 {
                let foot = Rect::from_min_max(pos2(0.0, 576.0 - 56.0), pos2(1024.0 - SCROLL_BAR_GRAB, 576.0));
                orbiters_ui::paint::fill_gradient(p, &[foot.left_top(), foot.right_top(), foot.right_bottom(), foot.left_bottom()], &orbiters_ui::paint::Gradient::vertical(foot, &[(0.0, Color32::TRANSPARENT), (1.0, look::OPAQUE)]));
            }
        }
    }
}

/// Draws a sheet's page (in N4N0's page's units): native to these menus, in VRChat's manner.
pub fn paint_sheet(p: &Painter, sheet: Sheet, facts: &Facts, hover: Option<usize>, pressed: Option<usize>) {
    let page = Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 576.0));
    p.rect_filled(page, 0.0, look::OPAQUE);
    if sheet == Sheet::Pictures && facts.opened.is_none() {
        let track = filter_track();
        p.rect_filled(track, look::radius(track.height() / 2.0), look::TILE);
        p.text(pos2(track.left() - 10.0, track.center().y), Align2::RIGHT_CENTER, tr_ctx("gallery filter", "Show"), look::font(15.5, weight::MEDIUM), look::MUTED);
        let line = track.left() - 62.0;
        p.line_segment([pos2(line, track.top() + 6.0), pos2(line, track.bottom() - 6.0)], Stroke::new(1.5, Color32::from_white_alpha(44)));
    }
    if sheet == Sheet::Talk {
        draw_conversation(p, facts);
    }
    if sheet == Sheet::Settings {
        draw_settings(p, facts);
    }
    if sheet == Sheet::Tracking {
        draw_tracking(p, facts);
    }
    if sheet == Sheet::Instances && facts.worlds_visited.is_empty() {
        widgets::icon(p, page.center() - vec2(0.0, 30.0), icons::CLOCK, 54.0, look::MUTED);
        text(p, page.center() + vec2(0.0, 24.0), Align2::CENTER_CENTER, tr("Nowhere yet"), 22.0, weight::BOLD, look::LABEL);
        text(p, page.center() + vec2(0.0, 54.0), Align2::CENTER_CENTER, tr("The worlds you visit show here, to go back to."), 16.0, weight::MEDIUM, look::MUTED);
    }
    if sheet == Sheet::Pictures && facts.recent.is_empty() {
        widgets::icon(p, pos2(512.0, 300.0), icons::CUBE, 54.0, look::MUTED);
        let (first, second) = match (facts.gallery_reading, facts.gallery_filter) {
            (true, _) => (tr("Reading your pictures"), tr("They show here in a moment.")),
            (false, GalleryFilter::All) => (tr("Nothing here yet"), tr("VRChat's pictures and your 3D captures show here.")),
            (false, GalleryFilter::Captures) => (tr("No captures yet"), tr("Your 3D pictures and recordings show here.")),
        };
        text(p, pos2(512.0, 354.0), Align2::CENTER_CENTER, first, 22.0, weight::BOLD, look::LABEL);
        text(p, pos2(512.0, 384.0), Align2::CENTER_CENTER, second, 16.0, weight::MEDIUM, look::MUTED);
    }
    for (i, item) in sheet_items(sheet, facts).iter().enumerate() {
        draw(p, Zone::Takeover, item, facts, hover == Some(i), pressed == Some(i));
    }
}

/// The conversation with N4N0, newest at the foot, the person's lines to the right.
fn draw_conversation(p: &Painter, facts: &Facts) {
    text(p, pos2(TALK_LEFT + 2.0, 30.0), Align2::LEFT_CENTER, tr("Conversation"), 20.0, weight::BOLD, look::LABEL);
    let area = Rect::from_min_max(pos2(TALK_LEFT, TALK_TOP), pos2(1024.0 - 20.0, 576.0));
    if facts.talk_lines.is_empty() {
        let (first, second) = if matches!(facts.talk, TalkNow::Listening(_)) {
            (tr("Go ahead"), tr("N4N0 is listening. Press the button again when you are done."))
        } else {
            (tr("Nothing said yet"), tr("Press the button, and ask N4N0 anything."))
        };
        text(p, pos2(TALK_LEFT + 2.0, TALK_TOP + 22.0), Align2::LEFT_CENTER, first, 27.0, weight::BOLD, look::LABEL);
        let font = look::font(18.5, weight::MEDIUM);
        p.text(pos2(TALK_LEFT + 2.0, TALK_TOP + 58.0), Align2::LEFT_CENTER, fit(p, second, &font, TALK_WIDTH - 4.0), font, look::LABEL);
        // What it can do for real, in words that work.
        text(p, pos2(TALK_LEFT + 2.0, TALK_TOP + 112.0), Align2::LEFT_CENTER, tr("You could say"), 16.5, weight::MEDIUM, look::MUTED);
        for (i, (glyph, said)) in [(icons::PLAY, tr_latin("\u{201c}Pause the music\u{201d}")), (icons::CAMERA, tr_latin("\u{201c}Take a picture in five seconds\u{201d}")), (icons::LIGHTBULB, tr_latin("\u{201c}Turn your light on, in blue\u{201d}")), (icons::CHAT_TEARDROP_TEXT, tr_latin("\u{201c}Tell people I'll be back soon\u{201d}"))].into_iter().enumerate() {
            let row = Rect::from_min_size(pos2(TALK_LEFT, TALK_TOP + 134.0 + i as f32 * 70.0), vec2(TALK_WIDTH, 62.0));
            p.rect_filled(row, look::radius(look::TILE_RADIUS + 2.0), look::TILE);
            widgets::icon(p, pos2(row.left() + 32.0, row.center().y), glyph, 26.0, look::MUTED);
            let font = look::font(20.0, weight::SEMIBOLD);
            p.text(pos2(row.left() + 60.0, row.center().y), Align2::LEFT_CENTER, fit(p, said, &font, row.width() - 72.0), font, look::LABEL);
        }
        return;
    }
    let clip = p.with_clip_rect(area);
    let mut y = TALK_TOP - facts.gallery_scroll;
    for (person, line) in &facts.talk_lines {
        let height = talk_line_height(line);
        if y + height >= TALK_TOP && y <= 576.0 {
            let wide = TALK_WIDTH * 0.86;
            let galley = clip.layout(line.clone(), look::font(17.0, weight::MEDIUM), look::LABEL, wide - 28.0);
            let size = galley.size() + vec2(28.0, 20.0);
            let x = if *person { TALK_LEFT + TALK_WIDTH - size.x } else { TALK_LEFT };
            let bubble = Rect::from_min_size(pos2(x, y), size);
            clip.rect_filled(bubble, look::radius(14.0), if *person { theme::mix(look::TILE, look::accent(), 0.22) } else { look::TILE });
            clip.galley(bubble.min + vec2(14.0, 10.0), galley, look::LABEL);
        }
        y += height;
    }
}

/// A tile: filled, no edge, lighter under the pointer. Under the pointer or chosen it has the accent's edge, as a cell of the ring has: solid accent is for one thing only, the
/// picture under way.
fn tile_back(p: &Painter, rect: Rect, hovered: bool, pressed: bool, selected: bool) {
    let fill = if pressed {
        look::TILE_DOWN
    } else if hovered || selected {
        look::TILE_HOVER
    } else {
        look::TILE
    };
    p.rect_filled(rect, look::radius(look::TILE_RADIUS), fill);
    if hovered || selected {
        let width = if selected { 2.0 } else { 1.5 };
        p.rect_stroke(rect.shrink(width / 2.0), look::radius(look::TILE_RADIUS), Stroke::new(width, look::accent()), egui::StrokeKind::Middle);
    }
}

/// A surface's top edge catches the light: a hairline that makes it read as an object on any world.
fn top_light(p: &Painter, panel: Rect) {
    let inset = look::RADIUS;
    p.line_segment([pos2(panel.left() + inset, panel.top() + 0.5), pos2(panel.right() - inset, panel.top() + 0.5)], Stroke::new(1.0, Color32::from_white_alpha(26)));
}

/// A small mark at a cell's outer edge: something more comes out of it.
fn bloom_mark(p: &Painter, ring: Ring, center: Pos2) {
    // Past the cell's name, near its clockwise end: nothing to read lies there.
    let middle = ring.to - (ring.to - ring.from) * 0.07;
    let tip = center + zones::polar(ring.outer - 1.5, middle);
    let base = ring.outer - 5.5;
    let side = 1.8 * 100.0 / ring.outer;
    let points = vec![tip, center + zones::polar(base, middle - side), center + zones::polar(base, middle + side)];
    p.add(Shape::convex_polygon(points, look::MUTED, Stroke::NONE));
}

/// Where each letter of a line goes along a ring (Enzo: the text should "follow the curvature and direction of the edge"): for each letter's middle along the line, in `centers`, the
/// degrees it lies at and how far it turns, in radians. On the top half the line runs clockwise with its tops outward; on the bottom half it runs the other way with its tops inward:
/// always left to right, as far as can be from upside down.
fn arc_places(centers: &[f32], total: f32, radius: f32, middle: f32) -> Vec<(f32, f32)> {
    let top = zones::polar(1.0, middle).y <= 0.001;
    centers
        .iter()
        .map(|x| {
            let along = ((x - total / 2.0) / radius).to_degrees();
            if top {
                (middle + along, (middle + along).to_radians())
            } else {
                (middle - along, (middle - along - 180.0).to_radians())
            }
        })
        .collect()
}

/// A line of words bent along a ring, centred on the angle `middle`, its letters' middles at `radius`.
#[allow(clippy::too_many_arguments)]
fn curved(p: &Painter, center: Pos2, radius: f32, middle: f32, s: &str, size: f32, w: f32, color: Color32, shadow: bool) {
    let font = look::font(size, w);
    let wide = |t: String| p.layout_no_wrap(t, font.clone(), Color32::WHITE).size().x;
    let letters: Vec<char> = s.chars().collect();
    // Measured as a whole up to each letter, so that what fits closer stays closer.
    let edges: Vec<f32> = (0..=letters.len()).map(|i| wide(letters[..i].iter().collect())).collect();
    let centers: Vec<f32> = edges.windows(2).map(|e| (e[0] + e[1]) / 2.0).collect();
    let total = edges.last().copied().unwrap_or(0.0);
    for (letter, (degrees, turn)) in letters.iter().zip(arc_places(&centers, total, radius, middle)) {
        if letter.is_whitespace() {
            continue;
        }
        let galley = p.layout_no_wrap(letter.to_string(), font.clone(), color);
        let half = galley.size() / 2.0;
        let (sin, cos) = turn.sin_cos();
        let corner = center + zones::polar(radius, degrees) - vec2(half.x * cos - half.y * sin, half.x * sin + half.y * cos);
        if shadow {
            let dark = p.layout_no_wrap(letter.to_string(), font.clone(), Color32::from_black_alpha(140));
            p.add(egui::epaint::TextShape::new(corner + vec2(0.0, 1.0), dark, Color32::from_black_alpha(140)).with_angle(turn));
        }
        p.add(egui::epaint::TextShape::new(corner, galley, color).with_angle(turn));
    }
}

/// What the disc over VRChat's wheel says: the tool pointed at, with its colour and in a line what it does; what a 3D picture waits for; or only whose it is.
fn draw_hub(p: &Painter, c: Pos2, across: f32, facts: &Facts) {
    let wrapped = |p: &Painter, top: f32, words: &str, size: f32, w: f32, ink: Color32, rows: usize, room: f32| -> f32 {
        let mut job = egui::text::LayoutJob::simple(words.to_string(), look::font(size, w), ink, room);
        job.wrap.max_rows = rows;
        job.wrap.overflow_character = Some('\u{2026}');
        job.halign = egui::Align::Center;
        let galley = p.layout_job(job);
        let high = galley.size().y;
        p.galley(pos2(c.x, top), galley, ink);
        high
    };
    let room = across * 0.74;
    if let (Some(wait), None) = (facts.picture_wait, facts.pointed) {
        let _ = wait;
        widgets::icon(p, c - vec2(0.0, 32.0), icons::CAMERA, 34.0, theme::mix(tool_tint(Action::Pictures), Color32::WHITE, 0.3));
        let of = if facts.picture_mode == Some(PhotoMode::People) { tr("3D picture of people") } else { tr("3D picture of this world") };
        let high = wrapped(p, c.y - 6.0, of, 14.0, weight::BOLD, Color32::WHITE, 2, room);
        wrapped(p, c.y - 2.0 + high, tr("waits for the camera"), 12.0, weight::MEDIUM, look::LABEL, 1, room);
        return;
    }
    let Some((first, second)) = facts.pointed.and_then(|a| caption(a, facts)) else {
        // Whose it is, and no more.
        widgets::icon(p, c, icons::N4N0_MARK, 40.0, Color32::from_white_alpha(60));
        return;
    };
    let action = facts.pointed.unwrap_or(Action::Idle);
    if let (Action::PlayPause, Some(id)) = (action, facts.art) {
        let art = Rect::from_center_size(c - vec2(0.0, 44.0), Vec2::splat(52.0));
        keep_colour(p, art);
        p.add(RectShape::filled(art, look::radius(8.0), Color32::WHITE).with_texture(id, cover_uv(art.size())));
    } else if let Some(icon) = hub_icon(action) {
        widgets::icon(p, c - vec2(0.0, 44.0), icon, 40.0, part_ink(action));
    }
    let high = wrapped(p, c.y - 14.0, &first, 15.0, weight::BOLD, Color32::WHITE, 2, room);
    wrapped(p, c.y - 11.0 + high, &second, 11.5, weight::MEDIUM, look::LABEL, if high > 24.0 { 2 } else { 3 }, room);
}

/// The icon of what the disc says.
fn hub_icon(action: Action) -> Option<&'static str> {
    Some(match action {
        Action::Worlds => icons::GLOBE_HEMISPHERE_WEST,
        Action::Page(Target::YouTube) => icons::YOUTUBE_LOGO,
        Action::Page(Target::Instances) => icons::CLOCK,
        Action::Pictures => icons::IMAGES,
        Action::All => icons::SQUARES_FOUR,
        Action::PlayPause => icons::MUSIC_NOTES,
        Action::Previous => icons::SKIP_BACK,
        Action::Next => icons::SKIP_FORWARD,
        Action::Picture(PhotoMode::People) => icons::USERS,
        Action::Picture(PhotoMode::World) => icons::IMAGE,
        Action::Record => icons::RECORD,
        _ => return None,
    })
}

/// How large a cell's name is.
const CELL_WORDS: f32 = 13.5;

/// Where a cell's icon lies on its axis, and how large: between the key at its inner edge and its name at its outer.
fn cell_icon(ring: Ring, center: Pos2) -> (Pos2, f32) {
    (center + zones::polar(ring.inner + 30.0, ring.middle()), 32.0)
}

/// 3D, on an icon's lower corner: the name lies the other way.
fn corner_badge(p: &Painter, icon_at: Pos2, icon_size: f32) {
    let b = Rect::from_center_size(icon_at + vec2(icon_size * 0.46, icon_size * 0.26), vec2(23.0, 15.0));
    p.rect_filled(b.expand(1.5), look::radius(5.5), look::CELL_FAR);
    p.rect_filled(b, look::radius(4.5), look::accent());
    text(p, b.center(), Align2::CENTER_CENTER, "3D", 10.0, weight::BOLD, look::on_accent());
}

/// The colour of a part's icon: its tool's, from All tools, the eye finds it by; the name stays white.
fn part_ink(action: Action) -> Color32 {
    theme::mix(tool_tint(action), Color32::WHITE, 0.3)
}

/// The number key that picks a cell, quiet, on its axis at the cell's inner edge: always in the same place, the first thing past the wheel.
fn digit(p: &Painter, zone: Zone, ring: Ring, center: Pos2) {
    let Some(n) = key_of(zone) else { return };
    let at = center + zones::polar(ring.inner + 9.5, ring.middle());
    let cap = Rect::from_center_size(at, Vec2::splat(15.0));
    p.rect_stroke(cap, look::radius(4.0), Stroke::new(1.0, Color32::from_white_alpha(70)), egui::StrokeKind::Inside);
    text(p, at, Align2::CENTER_CENTER, n, 11.0, weight::SEMIBOLD, Color32::from_white_alpha(190));
}

/// The key that picks a part of the wheel.
pub fn key_of(zone: Zone) -> Option<&'static str> {
    Some(match zone {
        Zone::WheelWorlds => "1",
        Zone::WheelYouTube => "2",
        Zone::WheelPlaying => "3",
        Zone::WheelAll => "4",
        Zone::WheelInstances => "5",
        Zone::WheelPictures => "6",
        Zone::BloomPeople => "7",
        Zone::BloomWorld => "8",
        Zone::BloomRecord => "9",
        _ => return None,
    })
}

fn is_selected(item: &Item, facts: &Facts) -> bool {
    match (facts.sheet, item.action) {
        // A 3D picture waiting for VRChat's camera stays in sight on what calls it off.
        (_, Action::Pictures) if facts.picture_wait.is_some() => true,
        (Some(Sheet::Worlds), Action::Worlds) | (Some(Sheet::All), Action::All) | (Some(Sheet::Pictures), Action::Pictures) => true,
        (Some(Sheet::Menu), Action::Page(Target::YouTube)) => facts.sheet_title == "YouTube",
        (Some(Sheet::Instances), Action::Page(Target::Instances)) | (Some(Sheet::Results), Action::Page(Target::YouTube)) => true,
        _ => false,
    }
}

fn draw(p: &Painter, zone: Zone, item: &Item, facts: &Facts, hovered: bool, pressed: bool) {
    let rect = item.rect;
    let shape = shaped(zone, rect);
    match item.look {
        Look::Blank => {}
        Look::Cell { icon, label, badge: has_badge } => {
            let (ring, center) = shape.ring.expect("a cell is on a ring");
            let waiting = zone == Zone::WheelPictures && facts.picture_wait.is_some();
            ring_fill(p, ring, center, if pressed { 1.0 } else if hovered { 0.8 } else if waiting { 0.5 } else { 0.0 });
            ring_rim(p, ring, center, hovered || waiting);
            let label = if waiting { tr("Waiting for the camera") } else { label };
            // Its icon upright toward the wheel, its name bent along its outer edge.
            let (at, size) = cell_icon(ring, center);
            widgets::icon(p, at, icon, size, if hovered && !waiting { Color32::WHITE } else { part_ink(if waiting { Action::Pictures } else { item.action }) });
            if has_badge {
                corner_badge(p, at, size);
            }
            curved(p, center, ring.outer - 11.5, ring.middle(), label, CELL_WORDS, weight::BOLD, look::LABEL, true);
            if zone.blooms().is_some() {
                bloom_mark(p, ring, center);
            }
            if facts.key_caps {
                digit(p, zone, ring, center);
            }
        }
        Look::Tile { icon, label, badge: has_badge } => {
            let selected = is_selected(item, facts);
            tile_back(p, rect, hovered, pressed, selected);
            let ink = part_ink(item.action);
            let size = (rect.height() * 0.42).clamp(18.0, 32.0);
            let top = rect.center().y - (size + 3.0 + 11.0) / 2.0;
            let at = pos2(rect.center().x, top + size / 2.0);
            widgets::icon(p, at, icon, size, ink);
            if has_badge {
                badge(p, at, size, look::accent(), look::on_accent());
            }
            let font_size = {
                let mut s = 11.0;
                while s > 8.5 && p.layout_no_wrap(label.to_string(), look::font(s, weight::BOLD), Color32::WHITE).size().x > rect.width() - 6.0 {
                    s -= 0.5;
                }
                s
            };
            let words = look::font(font_size, weight::SEMIBOLD);
            p.text(pos2(rect.center().x, top + size + 3.0 + 5.5), Align2::CENTER_CENTER, label, words, look::LABEL);
        }
        Look::Hero { icon, label, line, badge: has_badge, tint } => {
            let radius = look::radius(look::TILE_RADIUS + 4.0);
            p.rect_filled(rect, radius, theme::mix(look::TILE, tint, if pressed { 0.30 } else if hovered { 0.24 } else { 0.16 }));
            if hovered {
                p.rect_stroke(rect.shrink(1.0), radius, Stroke::new(2.0, tint), egui::StrokeKind::Middle);
            }
            // Its name beside its icon, and under them what it holds: real pictures and places wherever there are any.
            let chip = Rect::from_min_size(rect.min + vec2(14.0, 14.0), Vec2::splat(52.0));
            p.rect_filled(chip, look::radius(14.0), theme::alpha(tint, 0.22));
            widgets::icon(p, chip.center(), icon, 31.0, tint);
            if has_badge {
                badge(p, chip.center(), 38.0, look::accent(), look::on_accent());
            }
            let font = look::font(25.0, weight::BOLD);
            let room = rect.right() - chip.right() - 22.0;
            // Whose it is, where that is not plain: it is not what plays on this computer.
            if item.action == Action::Page(Target::YouTube) && facts.world_video.is_some() {
                p.text(pos2(chip.right() + 12.0, chip.center().y - 11.0), Align2::LEFT_CENTER, fit(p, label, &font, room), font, look::LABEL);
                let font = look::font(15.0, weight::MEDIUM);
                p.text(pos2(chip.right() + 12.0, chip.center().y + 15.0), Align2::LEFT_CENTER, fit(p, tr("This world's player"), &font, room), font, look::LABEL);
            } else {
                p.text(pos2(chip.right() + 12.0, chip.center().y), Align2::LEFT_CENTER, fit(p, label, &font, room), font, look::LABEL);
            }
            let body = Rect::from_min_max(pos2(rect.left() + 14.0, chip.bottom() + 10.0), rect.max - vec2(14.0, 12.0));
            hero_body(p, body, item.action, line, facts);
        }
        Look::Small { icon, label, tint } => {
            let on = item.action == Action::FastWalk && facts.fast_walk;
            let radius = look::radius(look::TILE_RADIUS + 2.0);
            p.rect_filled(rect, radius, theme::mix(look::TILE, tint, if pressed { 0.28 } else if hovered { 0.2 } else { 0.08 }));
            if hovered || on {
                p.rect_stroke(rect.shrink(0.75), radius, Stroke::new(2.0, tint), egui::StrokeKind::Middle);
            }
            // Many of them, in a headset: its icon over its name.
            if rect.width() < 200.0 {
                widgets::icon(p, pos2(rect.center().x, rect.top() + 28.0), icon, 30.0, tint);
                let font = look::font(16.5, weight::BOLD);
                p.text(pos2(rect.center().x, rect.bottom() - 21.0), Align2::CENTER_CENTER, fit(p, label, &font, rect.width() - 10.0), font, Color32::WHITE);
                return;
            }
            let chip = Rect::from_center_size(pos2(rect.left() + 38.0, rect.center().y), Vec2::splat(52.0));
            p.rect_filled(chip, look::radius(14.0), theme::alpha(tint, 0.2));
            widgets::icon(p, chip.center(), icon, 31.0, tint);
            let x = chip.right() + 12.0;
            let room = rect.right() - x - 10.0;
            // What it does, where there is the room to say it: it is not on a tile anywhere else.
            let said = caption(item.action, facts).map(|c| c.1).filter(|_| room >= 140.0);
            let font = look::font(20.0, weight::BOLD);
            match said {
                Some(said) => {
                    p.text(pos2(x, rect.top() + 25.0), Align2::LEFT_CENTER, fit(p, label, &font, room), font, Color32::WHITE);
                    let mut job = egui::text::LayoutJob::simple(said, look::font(15.0, weight::MEDIUM), look::LABEL, room);
                    job.wrap.max_rows = 2;
                    job.wrap.overflow_character = Some('\u{2026}');
                    p.galley(pos2(x, rect.top() + 39.0), p.layout_job(job), look::LABEL);
                }
                None => {
                    p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, fit(p, label, &font, room), font, Color32::WHITE);
                }
            }
        }
        Look::NowPlaying => {
            let tint = theme::VIOLET;
            let radius = look::radius(look::TILE_RADIUS + 4.0);
            p.rect_filled(rect, radius, theme::mix(look::TILE, tint, 0.14));
            let side = rect.height() - 24.0;
            let art = Rect::from_min_size(rect.min + vec2(12.0, 12.0), Vec2::splat(side));
            match (facts.art, &facts.music) {
                (Some(id), Some(_)) => {
                    keep_colour(p, art);
                    p.add(RectShape::filled(art, look::radius(12.0), Color32::WHITE).with_texture(id, cover_uv(art.size())));
                }
                _ => {
                    p.rect_filled(art, look::radius(12.0), theme::alpha(tint, 0.22));
                    widgets::icon(p, art.center(), icons::MUSIC_NOTES, side * 0.4, tint);
                }
            }
            let x = rect.left() + rect.height();
            let room = rect.right() - x - 14.0;
            let font = look::font(13.0, weight::SEMIBOLD);
            p.text(pos2(x, rect.top() + 20.0), Align2::LEFT_CENTER, tr("Now playing on this computer"), font, tint);
            match &facts.music {
                Some((title, _)) => {
                    let font = look::font(23.0, weight::BOLD);
                    p.text(pos2(x, rect.top() + 44.0), Align2::LEFT_CENTER, fit(p, title, &font, room), font, look::LABEL);
                    let font = look::font(15.5, weight::MEDIUM);
                    p.text(pos2(x, rect.top() + 68.0), Align2::LEFT_CENTER, fit(p, &facts.music_by, &font, room), font, look::MUTED);
                }
                None => {
                    let font = look::font(23.0, weight::BOLD);
                    p.text(pos2(x, rect.top() + 52.0), Align2::LEFT_CENTER, tr("Nothing yet"), font, look::MUTED);
                    let font = look::font(15.0, weight::MEDIUM);
                    p.text(pos2(x, rect.top() + 78.0), Align2::LEFT_CENTER, fit(p, tr("What your computer plays shows here, with its controls."), &font, room), font, look::MUTED);
                }
            }
            // The level between its minus and its plus.
            if let Some((percent, muted)) = facts.volume {
                let right = rect.right() - 16.0;
                let bar = Rect::from_center_size(pos2(right - 48.0 - 39.0, rect.bottom() - 37.0), vec2(66.0, 7.0));
                p.rect_filled(bar, look::radius(3.5), Color32::from_white_alpha(70));
                let level = if muted { 0.0 } else { percent as f32 / 100.0 };
                p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * level, 7.0)), look::radius(3.5), look::LABEL);
                p.circle_filled(pos2(bar.left() + bar.width() * level, bar.center().y), 8.0, look::LABEL);
            }
        }
        Look::Picture(mode) => picture_button(p, zone, shape, rect, mode, facts, hovered, pressed),
        Look::Record => {
            let label = if facts.recording { tr("Stop") } else { tr_ctx("start recording", "Record") };
            let dim = facts.capture_busy && !facts.recording;
            let time = facts.recording_seconds.filter(|_| facts.recording).map_or(label.to_string(), clock);
            let color = if dim { theme::FG_4 } else { look::LABEL };
            if let Some((ring, center)) = shape.ring {
                if facts.recording {
                    ring_mesh(p, ring, center, theme::mix(theme::REC, Color32::BLACK, 0.35), theme::mix(theme::REC, Color32::BLACK, 0.6));
                } else {
                    ring_fill(p, ring, center, if pressed { 1.0 } else if hovered { 0.8 } else { 0.0 });
                }
                ring_rim(p, ring, center, hovered);
                let (at, _) = cell_icon(ring, center);
                if facts.recording {
                    p.rect_filled(Rect::from_center_size(at, Vec2::splat(18.0)), look::radius(3.0), color);
                } else {
                    p.circle_filled(at, 11.0, if dim { theme::FG_4 } else { theme::REC });
                }
                curved(p, center, ring.outer - 11.5, ring.middle(), &time, CELL_WORDS - 1.0, weight::BOLD, color, true);
                if facts.key_caps {
                    digit(p, zone, ring, center);
                }
            } else {
                let tint = theme::REC;
                let radius = look::radius(if rect.height() < 60.0 { rect.height() / 2.0 } else { look::TILE_RADIUS + 2.0 });
                if rect.height() < 60.0 {
                    p.rect_filled(rect, radius, if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { theme::alpha(look::TILE, 0.7) });
                } else {
                p.rect_filled(rect, radius, theme::mix(look::TILE, tint, if pressed { 0.28 } else if hovered { 0.22 } else if dim { 0.04 } else { 0.14 }));
                }
                if hovered {
                    p.rect_stroke(rect.shrink(1.0), radius, Stroke::new(1.5, tint), egui::StrokeKind::Middle);
                }
                if rect.height() < 60.0 {
                    let at = pos2(rect.left() + 26.0, rect.center().y);
                    if facts.recording {
                        p.rect_filled(Rect::from_center_size(at, Vec2::splat(15.0)), look::radius(3.0), tint);
                    } else {
                        p.circle_filled(at, 8.5, if dim { theme::FG_4 } else { tint });
                    }
                    let font = look::font(15.5, weight::SEMIBOLD);
                    let words = if facts.recording { time.clone() } else { tr_ctx("start recording", "Record").to_string() };
                    p.text(pos2(rect.left() + 48.0, rect.center().y), Align2::LEFT_CENTER, fit(p, &words, &font, rect.width() - 56.0), font, color);
                    return;
                }
                let at = pos2(rect.left() + 44.0, rect.center().y);
                p.rect_filled(Rect::from_center_size(at, Vec2::splat(56.0)), look::radius(14.0), theme::alpha(tint, 0.22));
                if facts.recording {
                    p.rect_filled(Rect::from_center_size(at, Vec2::splat(22.0)), look::radius(4.0), tint);
                } else {
                    p.circle_filled(at, 13.0, if dim { theme::FG_4 } else { tint });
                }
                let x = rect.left() + 84.0;
                let room = rect.right() - x - 8.0;
                text(p, pos2(x, rect.center().y - 12.0), Align2::LEFT_CENTER, &time, 20.0, weight::BOLD, color);
                let small = look::font(15.0, weight::MEDIUM);
                p.text(pos2(x, rect.center().y + 14.0), Align2::LEFT_CENTER, fit(p, tr("A 3D scene of where you walk"), &small, room), small, look::MUTED);
            }
        }
        Look::Video(i) => {
            let Some(video) = facts.videos.get(i) else { return };
            let clip = p.with_clip_rect(Rect::from_min_max(pos2(0.0, RESULTS_TOP), pos2(1024.0, 576.0)));
            let radii = [theme::squircle(14.0); 4];
            let picture = Rect::from_min_size(rect.min, vec2(rect.width(), rect.width() * 9.0 / 16.0));
            match video.picture {
                Some(id) => squircle_picture(&clip, picture, radii, id, cover_uv(picture.size()), if pressed { Color32::from_gray(200) } else { Color32::WHITE }),
                None => {
                    orbiters_ui::paint::fill_convex(&clip, &orbiters_ui::paint::squircle_points_corners(picture, radii), |_| theme::INK_750);
                    widgets::icon(&clip, picture.center(), icons::YOUTUBE_LOGO, 40.0, look::MUTED);
                }
            }
            if hovered {
                clip.rect_stroke(picture.shrink(1.0), look::radius(14.0), Stroke::new(2.0, look::accent()), egui::StrokeKind::Middle);
            }
            if let Some(duration) = &video.duration {
                let font = look::font(14.0, weight::SEMIBOLD);
                let wide = clip.layout_no_wrap(duration.clone(), font.clone(), Color32::WHITE).size().x;
                let chip = Rect::from_min_size(picture.right_bottom() - vec2(wide + 20.0, 32.0), vec2(wide + 12.0, 24.0));
                clip.rect_filled(chip, look::radius(6.0), Color32::from_black_alpha(190));
                clip.text(chip.center(), Align2::CENTER_CENTER, duration, font, look::LABEL);
            }
            // Two lines: results for one search look alike, and their words are what tells them apart.
            let mut job = egui::text::LayoutJob::simple(video.title.clone(), look::font(16.5, weight::BOLD), look::LABEL, rect.width() - 4.0);
            job.wrap.max_rows = 2;
            job.wrap.overflow_character = Some('\u{2026}');
            clip.galley(pos2(rect.left() + 2.0, picture.bottom() + 6.0), clip.layout_job(job), look::LABEL);
            let font = look::font(15.0, weight::MEDIUM);
            clip.text(pos2(rect.left() + 2.0, picture.bottom() + 62.0), Align2::LEFT_CENTER, fit(&clip, &video.channel, &font, rect.width() - 4.0), font, look::MUTED);
        }
        Look::VideoCopy(i) => {
            let clip = p.with_clip_rect(Rect::from_min_max(pos2(0.0, RESULTS_TOP), pos2(1024.0, 576.0)));
            // The main one, in the accent. Copied says so on it, lighter still: the accent alone would not show the change.
            let copied = facts.video_copied == Some(i);
            let radius = look::radius(rect.height() / 2.0);
            let fill = if copied {
                theme::mix(look::accent(), Color32::WHITE, 0.6)
            } else if pressed {
                theme::mix(look::accent(), Color32::BLACK, 0.25)
            } else if hovered {
                theme::mix(look::accent(), Color32::WHITE, 0.15)
            } else {
                look::accent()
            };
            clip.rect_filled(rect, radius, fill);
            clip.text(rect.center(), Align2::CENTER_CENTER, if copied { tr("Copied") } else { tr("Copy link") }, look::font(16.0, weight::BOLD), look::on_accent());
        }
        Look::Timeline => {
            let Some((at, of)) = facts.music_times.filter(|t| t.1 > 0) else { return };
            let done = (at as f32 / of as f32).clamp(0.0, 1.0);
            let tall = if (hovered || pressed) && facts.can_seek { 16.0 } else { 12.0 };
            let lane = timeline_bar(rect);
            let bar = Rect::from_center_size(lane.center(), vec2(lane.width(), tall));
            p.rect_filled(bar, look::radius(tall / 2.0), Color32::from_white_alpha(36));
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * done, tall)), look::radius(tall / 2.0), theme::VIOLET);
            if facts.can_seek {
                p.circle_filled(pos2(bar.left() + bar.width() * done, bar.center().y), if hovered || pressed { 15.0 } else { 12.0 }, look::LABEL);
            }
            text(p, pos2(rect.right(), bar.center().y), Align2::RIGHT_CENTER, &format!("{} / {}", minutes(at), minutes(of)), 19.0, weight::SEMIBOLD, look::LABEL);
        }
        Look::ScrollBar => {
            let Some(sheet) = facts.sheet else { return };
            let Some((top, track, knob)) = scroll_bar(sheet, facts) else { return };
            let max = max_scroll(sheet, facts);
            // Past an end its knob is squeezed against that end, as far as the list has gone.
            let past = (-facts.gallery_scroll).max(facts.gallery_scroll - max).max(0.0);
            let knob = (knob - past * 0.6).max(14.0);
            let y = top + (track - knob) * (facts.gallery_scroll / max).clamp(0.0, 1.0);
            let (width, alpha) = if hovered || pressed { (16.0, 200) } else { (12.0, 120) };
            p.rect_filled(Rect::from_min_size(pos2(rect.center().x - width / 2.0, y), vec2(width, knob)), look::radius(width / 2.0), Color32::from_white_alpha(alpha));
        }
        Look::SmallView => {
            let radius = look::radius(rect.height() / 2.0);
            p.rect_filled(rect, radius, if hovered { look::TILE_HOVER } else { look::TILE });
            p.text(rect.center(), Align2::CENTER_CENTER, if facts.view_small { tr("Around you") } else { tr_ctx("view", "Small") }, look::font(13.0, weight::SEMIBOLD), look::LABEL);
        }
        Look::HideView => {
            let radius = look::radius(rect.height() / 2.0);
            p.rect_filled(rect, radius, if hovered { look::ACCENT_SOFT } else { look::TILE });
            p.rect_stroke(rect, radius, Stroke::new(1.5, look::accent()), egui::StrokeKind::Inside);
            p.text(rect.center(), Align2::CENTER_CENTER, tr("Hide 3D"), look::font(13.0, weight::SEMIBOLD), look::accent());
        }
        Look::VideoDrop => {
            let clip = p.with_clip_rect(Rect::from_min_max(pos2(0.0, RESULTS_TOP), pos2(1024.0, 576.0)));
            clip.rect_filled(rect, look::radius(rect.height() / 2.0), if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::TILE });
            clip.text(rect.center(), Align2::CENTER_CENTER, tr("Drop in world"), look::font(16.0, weight::SEMIBOLD), look::LABEL);
        }
        Look::BackToPictures => {
            p.rect_filled(rect, look::radius(rect.height() / 2.0), if hovered { theme::alpha(theme::INK_900, 0.98) } else { theme::alpha(theme::INK_900, 0.8) });
            widgets::icon(p, pos2(rect.left() + 22.0, rect.center().y), icons::ARROW_LEFT, 20.0, look::LABEL);
            p.text(pos2(rect.left() + 42.0, rect.center().y), Align2::LEFT_CENTER, tr("Back to pictures"), look::font(15.5, weight::SEMIBOLD), look::LABEL);
        }
        Look::Back => {
            // What was searched, as a field: it can be changed from here.
            let radius = look::radius(rect.height() / 2.0);
            p.rect_filled(rect, radius, if hovered { look::TILE_HOVER } else { look::TILE });
            p.rect_stroke(rect.shrink(0.5), radius, Stroke::new(1.0, Color32::from_white_alpha(if hovered { 110 } else { 44 })), egui::StrokeKind::Inside);
            widgets::icon(p, pos2(rect.left() + 24.0, rect.center().y), icons::MAGNIFYING_GLASS, 21.0, look::LABEL);
            let font = look::font(18.0, weight::SEMIBOLD);
            let shown = fit(p, &facts.videos_for, &font, rect.width() - 170.0);
            p.text(pos2(rect.left() + 46.0, rect.center().y), Align2::LEFT_CENTER, shown, font, look::LABEL);
            p.text(pos2(rect.right() - 18.0, rect.center().y), Align2::RIGHT_CENTER, tr("Search again"), look::font(14.5, weight::MEDIUM), look::MUTED);
        }
        Look::NewTalk => {
            p.rect_filled(rect, look::radius(rect.height() / 2.0), if hovered { look::TILE_HOVER } else { look::TILE });
            p.text(rect.center(), Align2::CENTER_CENTER, tr("New conversation"), look::font(15.0, weight::SEMIBOLD), look::LABEL);
        }
        Look::TalkButton => {
            // Clear as day: the one thing on this page that changes with whether N4N0 hears you.
            let center = pos2(rect.center().x, rect.top() + 170.0);
            let radius = 118.0;
            let (fill, ink, first, second): (Color32, Color32, &str, &str) = match facts.talk {
                // Green is for when it hears you, and for nothing else on this page.
                TalkNow::Ready => (if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::TILE }, look::LABEL, tr("Press to talk"), tr("Press again when you are done")),
                TalkNow::Listening(_) => (look::accent(), look::on_accent(), tr("Listening"), tr("Press when you are done")),
                TalkNow::Thinking => (look::TILE, look::LABEL, tr("Thinking"), tr("N4N0 answers on the right")),
                TalkNow::Off(why) => (look::TILE, theme::FG_4, tr("Not available"), tr(why)),
            };
            if let TalkNow::Listening(level) = facts.talk {
                p.circle_filled(center, radius + 12.0 + level.clamp(0.0, 1.0) * 28.0, theme::alpha(look::accent(), 0.22));
            }
            p.circle_filled(center, radius, fill);
            if matches!(facts.talk, TalkNow::Ready) {
                p.circle_stroke(center, radius, Stroke::new(3.0, Color32::from_white_alpha(if hovered { 110 } else { 54 })));
            }
            widgets::icon(p, center, if matches!(facts.talk, TalkNow::Thinking) { icons::CHAT_TEARDROP_TEXT } else { icons::MICROPHONE }, 96.0, ink);
            let y = center.y + radius + 48.0;
            let color = if matches!(facts.talk, TalkNow::Listening(_)) { look::accent() } else { look::LABEL };
            text(p, pos2(rect.center().x, y), Align2::CENTER_CENTER, first, 30.0, weight::BOLD, color);
            let font = look::font(16.0, weight::MEDIUM);
            p.text(pos2(rect.center().x, y + 34.0), Align2::CENTER_CENTER, fit(p, second, &font, rect.width() - 8.0), font, look::MUTED);
            if let Some(word) = &facts.wake_word {
                let font = look::font(14.0, weight::MEDIUM);
                p.text(pos2(rect.center().x, y + 62.0), Align2::CENTER_CENTER, fit(p, &tr!("You can also say \u{201c}{word}\u{201d}", word = word), &font, rect.width() - 8.0), font, look::MUTED);
            }
        }
        Look::Heading(name) => {
            text(p, pos2(rect.left() + 2.0, rect.center().y), Align2::LEFT_CENTER, name, 16.0, weight::BOLD, look::LABEL);
        }
        Look::HeadTab { icon, label, on } => {
            let radius = look::radius(rect.height() / 2.0);
            let tint = part_ink(item.action);
            if on {
                p.rect_filled(rect, radius, theme::alpha(tool_tint(item.action), 0.3));
                p.rect_stroke(rect.shrink(0.5), radius, Stroke::new(1.0, theme::alpha(tint, 0.8)), egui::StrokeKind::Inside);
            } else if hovered || pressed {
                p.rect_filled(rect, radius, look::TILE_HOVER);
            }
            if rect.width() > 40.0 {
                widgets::icon(p, pos2(rect.left() + 16.0, rect.center().y), icon, 17.0, tint);
                p.text(pos2(rect.left() + 29.0, rect.center().y), Align2::LEFT_CENTER, label, look::font(13.5, if on { weight::BOLD } else { weight::MEDIUM }), if on { Color32::WHITE } else { look::LABEL });
            } else {
                widgets::icon(p, rect.center(), icon, 18.0, tint);
            }
        }
        Look::Choice { icon, label, on } => {
            let radius = look::radius(14.0);
            p.rect_filled(rect, radius, if on { look::accent() } else if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::TILE });
            let ink = if on { look::on_accent() } else { look::LABEL };
            let font = look::font(19.0, weight::SEMIBOLD);
            let wide = p.layout_no_wrap(label.to_string(), font.clone(), ink).size().x;
            let left = rect.center().x - (wide + 34.0) / 2.0;
            widgets::icon(p, pos2(left + 12.0, rect.center().y), icon, 24.0, ink);
            p.text(pos2(left + 34.0, rect.center().y), Align2::LEFT_CENTER, label, font, ink);
        }
        Look::SmoothingBar(which) => draw_smoothing_bar(p, rect, which, facts, hovered),
        Look::SoundName => draw_stepper_name(p, rect, icons::SPEAKER_HIGH, sound_name(facts)),
        Look::LanguageName => draw_stepper_name(p, rect, icons::GLOBE, &language_name(facts)),
        Look::Swatch(i) => {
            COLOURS.with(|l| l.borrow_mut().push(rect));
            let [red, green, blue] = SWATCHES[i];
            let chosen = if facts.orbit.theme == MenuLook::Retro { facts.orbit.retro_color } else { facts.orbit.accent };
            let radius = rect.width() / 2.0 - if hovered { 2.0 } else { 5.0 };
            p.circle_filled(rect.center(), radius, Color32::from_rgb(red, green, blue));
            if chosen == SWATCHES[i] {
                p.circle_stroke(rect.center(), rect.width() / 2.0 - 1.0, Stroke::new(3.0, Color32::WHITE));
            }
        }
        Look::HueBar => {
            COLOURS.with(|l| l.borrow_mut().push(rect.expand(3.0)));
            const STEPS: usize = 36;
            for n in 0..STEPS {
                let [red, green, blue] = hue_color(n as f32 / STEPS as f32);
                let x = rect.left() + rect.width() * n as f32 / STEPS as f32;
                p.rect_filled(Rect::from_min_size(pos2(x, rect.top()), vec2(rect.width() / STEPS as f32 + 0.6, rect.height())), 0.0, Color32::from_rgb(red, green, blue));
            }
            p.rect_stroke(rect, 0.0, Stroke::new(if hovered { 3.0 } else { 1.0 }, Color32::from_white_alpha(if hovered { 255 } else { 70 })), egui::StrokeKind::Outside);
        }
        Look::VisitHead => {
            p.text(pos2(26.0, 20.0), Align2::LEFT_CENTER, tr("Invite me: your last visit. Or pick an earlier time."), look::font(17.0, weight::MEDIUM), look::LABEL);
            // Over their columns.
            let right = 1024.0 - 24.0 - 12.0;
            let font = look::font(15.0, weight::MEDIUM);
            p.text(pos2(right - INVITE_WIDTH - 8.0 - 6.0, 22.0), Align2::RIGHT_CENTER, tr("Earlier visits"), font.clone(), look::MUTED);

        }
        Look::HereNow => {
            let clip = p.with_clip_rect(Rect::from_min_max(pos2(0.0, VISIT_HEAD), pos2(1024.0, 576.0)));
            clip.circle_filled(pos2(rect.left() + 18.0, rect.center().y), 5.0, look::accent());
            clip.text(pos2(rect.left() + 32.0, rect.center().y), Align2::LEFT_CENTER, tr("You are here"), look::font(16.5, weight::SEMIBOLD), look::LABEL);
        }
        Look::Visit(i) => {
            let p = &p.with_clip_rect(Rect::from_min_max(pos2(0.0, VISIT_HEAD), pos2(1024.0, 576.0)));
            let Some(visit) = facts.worlds_visited.get(i) else { return };
            tile_back(p, rect, false, false, false);
            // The world itself across its row, fading out under what is to read.
            if let Some(id) = visit.picture {
                let points = orbiters_ui::paint::squircle_points_corners(rect, [theme::squircle(look::TILE_RADIUS); 4]);
                // A small part of its picture spread across the row: its colours, and nothing that could be read.
                let spread = Rect::from_center_size(pos2(0.5, 0.5), vec2(0.05, 0.05));
                squircle_picture(p, rect, [theme::squircle(look::TILE_RADIUS); 4], id, spread, Color32::from_gray(70));
                orbiters_ui::paint::fill_gradient(p, &points, &orbiters_ui::paint::Gradient::horizontal(rect, &[(0.0, theme::alpha(look::TILE, 0.6)), (0.5, theme::alpha(look::TILE, 0.9)), (1.0, theme::alpha(look::TILE, 0.96))]));
            }
            let picture = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2((rect.height() - 12.0) * 16.0 / 9.0, rect.height() - 12.0));
            match visit.picture {
                Some(id) => {
                    keep_colour(p, picture);
                    p.add(RectShape::filled(picture, look::radius(5.0), Color32::WHITE).with_texture(id, cover_uv(picture.size())));
                }
                None => {
                    p.rect_filled(picture, look::radius(5.0), look::HEADER);
                    widgets::icon(p, picture.center(), icons::GLOBE_HEMISPHERE_WEST, 30.0, look::MUTED);
                }
            }
            let x = picture.right() + 16.0;
            // The buttons lie on its right: the words stop short of them.
            let shown = visit.earlier.len().min(EARLIER_SHOWN);
            let earlier_from = rect.right() - 12.0 - INVITE_WIDTH - 8.0 - shown as f32 * (EARLIER_WIDTH + 6.0);
            let room = earlier_from - x - 10.0;
            let font = look::font(20.0, weight::BOLD);
            p.text(pos2(x, rect.center().y - 14.0), Align2::LEFT_CENTER, fit(p, &visit.name, &font, room), font, look::LABEL);
            let font = look::font(16.0, weight::MEDIUM);
            p.text(pos2(x, rect.center().y + 14.0), Align2::LEFT_CENTER, fit(p, &visit.line, &font, room), font, look::MUTED);
        }
        Look::Chip { group, visit } => {
            let p = &p.with_clip_rect(Rect::from_min_max(pos2(0.0, VISIT_HEAD), pos2(1024.0, 576.0)));
            let primary = visit == 0;
            let words = if primary {
                tr("Invite me").to_string()
            } else {
                facts.worlds_visited.get(group).and_then(|v| v.earlier.get(visit - 1)).map_or(String::new(), |e| e.1.clone())
            };
            // What it did shows on it, at once: from the press, not when the answer comes.
            let visit_place = facts.worlds_visited.get(group).and_then(|v| if primary { Some(v.location.as_str()) } else { v.earlier.get(visit - 1).map(|e| e.0.as_str()) });
            let state = facts.invite_chip.as_ref().filter(|(at, _)| Some(at.as_str()) == visit_place).map(|(_, state)| *state);
            let radius = look::radius(rect.height() / 2.0);
            let (fill, ink, edge, words): (Color32, Color32, Color32, String) = match state {
                Some(InviteChip::Asking) => (theme::mix(look::OPAQUE, look::accent(), 0.3), look::LABEL, look::accent(), tr("Asking…").to_string()),
                Some(InviteChip::Sent) => (look::accent(), look::on_accent(), look::accent(), tr("Invited").to_string()),
                Some(InviteChip::Failed) => (theme::mix(look::OPAQUE, theme::DANGER, 0.3), look::LABEL, theme::DANGER, tr("Failed").to_string()),
                // Only the first on offer is solid: a list of them would shout. Invited, solid, then says at once what it did.
                None if primary && facts.worlds_visited.iter().position(|v| !v.here) == Some(group) => (if pressed { theme::mix(look::accent(), Color32::BLACK, 0.25) } else if hovered { theme::mix(look::accent(), Color32::WHITE, 0.15) } else { look::accent() }, look::on_accent(), look::accent(), words),
                None if primary => (if pressed { theme::mix(look::OPAQUE, look::accent(), 0.5) } else if hovered { theme::mix(look::OPAQUE, look::accent(), 0.3) } else { look::OPAQUE }, look::LABEL, look::accent(), words),
                None => (if pressed { theme::mix(look::OPAQUE, look::accent(), 0.35) } else if hovered { look::TILE_HOVER } else { look::OPAQUE }, look::LABEL, if hovered { look::accent() } else { theme::INK_600 }, words),
            };
            p.rect_filled(rect, radius, fill);
            p.rect_stroke(rect, radius, Stroke::new(1.5, edge), egui::StrokeKind::Inside);
            // Why this one is solid: it is where the person was last.
            if primary && state.is_none() && facts.worlds_visited.iter().position(|v| !v.here) == Some(group) {
                p.text(pos2(rect.center().x, rect.top() - 9.0), Align2::CENTER_CENTER, tr("Last visit"), look::font(13.5, weight::MEDIUM), look::MUTED);
            }
            let font = look::font(if primary { 18.0 } else { 16.5 }, if primary { weight::BOLD } else { weight::SEMIBOLD });
            p.text(rect.center(), Align2::CENTER_CENTER, fit(p, &words, &font, rect.width() - 12.0), font, ink);
            // A time says what it does, on it.
            if hovered && !primary && state.is_none() {
                let said = tr!("Invite to {time}", time = words);
                let font = look::font(15.5, weight::SEMIBOLD);
                let wide = p.layout_no_wrap(said.clone(), font.clone(), look::on_accent()).size().x + 22.0;
                let tip = Rect::from_center_size(pos2(rect.center().x, rect.top() - 18.0), vec2(wide, 28.0));
                p.rect_filled(tip, look::radius(14.0), look::accent());
                p.text(tip.center(), Align2::CENTER_CENTER, said, font, look::on_accent());
            }
        }
        Look::Playing => {
            let none = facts.music.is_none();
            let (name, playing) = facts.music.clone().unwrap_or_default();
            let glyph = if playing { icons::PAUSE } else if none { icons::MUSIC_NOTES } else { icons::PLAY };
            if let Some((ring, center)) = shape.ring {
                ring_fill(p, ring, center, if pressed { 1.0 } else if hovered && !none { 0.8 } else { 0.0 });
                ring_rim(p, ring, center, hovered);
                let ink = if none { look::MUTED } else { look::LABEL };
                // A plain play or pause, of the size, weight and colouring of the other cells' icons: its picture and the song are for when it is pointed at.
                let (at, size) = cell_icon(ring, center);
                widgets::icon(p, at, glyph, size, if none { look::MUTED } else if hovered { Color32::WHITE } else { part_ink(Action::Page(Target::Music)) });
                curved(p, center, ring.outer - 11.5, ring.middle(), tr("Now playing"), CELL_WORDS, weight::BOLD, ink, true);
                if !none {
                    bloom_mark(p, ring, center);
                }
                if facts.key_caps {
                    digit(p, zone, ring, center);
                }
            } else {
                tile_back(p, rect, hovered && !none, pressed && !none, false);
                let side = rect.height() - 12.0;
                let art = Rect::from_min_size(rect.min + vec2(6.0, 6.0), Vec2::splat(side));
                if let (Some(id), false) = (facts.art, none) {
                    keep_colour(p, art);
                    p.add(RectShape::filled(art, look::radius(4.0), Color32::WHITE).with_texture(id, cover_uv(art.size())));
                } else {
                    p.rect_filled(art, look::radius(4.0), look::HEADER);
                }
                // Its play or pause plain on the picture, on a disc of its own; beside it what plays, and what a press does.
                if !none {
                    p.circle_filled(art.center(), side * 0.36, Color32::from_black_alpha(190));
                }
                widgets::icon(p, art.center(), glyph, side * 0.42, if none { look::MUTED } else { look::LABEL });
                let x = art.right() + 7.0;
                let room = rect.right() - x - 5.0;
                let font = look::font(10.5, weight::BOLD);
                if none {
                    p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, fit(p, tr("Nothing playing"), &font, room), font, look::MUTED);
                } else {
                    p.text(pos2(x, rect.center().y - 7.5), Align2::LEFT_CENTER, fit(p, &name, &font, room), font, look::LABEL);
                    let font = look::font(10.5, weight::MEDIUM);
                    p.text(pos2(x, rect.center().y + 7.5), Align2::LEFT_CENTER, if playing { tr_ctx("tile", "Press to pause") } else { tr_ctx("tile", "Press to play") }, font, look::MUTED);
                }
            }
        }
        Look::Transport(glyph) => {
            if let Some((ring, center)) = shape.ring {
                ring_fill(p, ring, center, if pressed { 1.0 } else if hovered { 0.8 } else { 0.0 });
                ring_rim(p, ring, center, hovered);
                let (at, size) = cell_icon(ring, center);
                widgets::icon(p, at, glyph, size * 0.8, Color32::WHITE);
            } else {
                p.circle_filled(rect.center(), rect.width() / 2.0, if hovered { look::TILE_HOVER } else { look::TILE });
                widgets::icon(p, rect.center(), glyph, rect.width() * 0.5, look::LABEL);
            }
        }
        Look::Talk => {
            let on = matches!(facts.talk, TalkNow::Listening(_));
            tile_back(p, rect, hovered, pressed, on);
            let color = if on {
                look::accent()
            } else if !facts.n4n0 {
                theme::FG_4
            } else if hovered {
                look::accent()
            } else {
                look::LABEL
            };
            let size = (rect.height() * 0.42).clamp(18.0, 32.0);
            let top = rect.center().y - (size + 3.0 + 11.0) / 2.0;
            widgets::icon(p, pos2(rect.center().x, top + size / 2.0), icons::MICROPHONE, size, color);
            p.text(pos2(rect.center().x, top + size + 3.0 + 5.5), Align2::CENTER_CENTER, if on { tr("Listening") } else { tr_ctx("tile", "Ask N4N0") }, look::font(11.0, weight::SEMIBOLD), if on { look::accent() } else if facts.n4n0 { look::LABEL } else { theme::FG_4 });
        }
        Look::PlayingSmall => {
            tile_back(p, rect, hovered, pressed, false);
            let playing = facts.music.as_ref().is_some_and(|m| m.1);
            // Its play or pause, and what would play: the one tile that means nothing without its name.
            let side = rect.height() - 12.0;
            let art = Rect::from_min_size(rect.min + vec2(6.0, 6.0), Vec2::splat(side));
            match facts.art {
                Some(id) => {
                    keep_colour(p, art);
                    p.add(RectShape::filled(art, look::radius(5.0), Color32::from_gray(140)).with_texture(id, cover_uv(art.size())));
                }
                None => {
                    p.rect_filled(art, look::radius(5.0), look::HEADER);
                }
            }
            p.circle_filled(art.center(), side * 0.36, Color32::from_black_alpha(190));
            widgets::icon(p, art.center(), if playing { icons::PAUSE } else { icons::PLAY }, side * 0.42, look::LABEL);
            let x = art.right() + 6.0;
            let room = rect.right() - x - 4.0;
            let font = look::font(10.0, weight::BOLD);
            let name = facts.music.as_ref().map_or("", |m| m.0.as_str());
            p.text(pos2(x, rect.center().y - 7.0), Align2::LEFT_CENTER, fit(p, name, &font, room), font, look::LABEL);
            p.text(pos2(x, rect.center().y + 7.5), Align2::LEFT_CENTER, if playing { tr_ctx("tile", "Press to pause") } else { tr_ctx("tile", "Press to play") }, look::font(9.5, weight::MEDIUM), look::MUTED);
        }
        Look::OpenWindow => {
            p.rect_filled(rect, look::radius(rect.height() / 2.0), if pressed { theme::mix(look::accent(), Color32::BLACK, 0.25) } else if hovered { theme::mix(look::accent(), Color32::WHITE, 0.15) } else { look::accent() });
            widgets::icon(p, pos2(rect.left() + 26.0, rect.center().y), icons::ARROW_SQUARE_OUT, 22.0, look::on_accent());
            p.text(pos2(rect.left() + 46.0, rect.center().y), Align2::LEFT_CENTER, tr("Walk around it, in Orbiters"), look::font(19.0, weight::BOLD), look::on_accent());
        }
        Look::PeekWord => {
            let radius = look::radius(rect.height() / 2.0);
            p.rect_filled(rect, radius, if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::TILE });
            p.rect_stroke(rect.shrink(0.5), radius, Stroke::new(1.0, Color32::from_white_alpha(if hovered { 120 } else { 54 })), egui::StrokeKind::Inside);
            let (icon, words) = if facts.peek { (icons::EYE, tr("Show Orbiters")) } else { (icons::EYE_SLASH, tr("Hide Orbiters")) };
            let font = look::font(13.0, weight::SEMIBOLD);
            let wide = p.layout_no_wrap(words.to_string(), font.clone(), look::LABEL).size().x;
            let left = rect.center().x - (wide + 22.0) / 2.0;
            widgets::icon(p, pos2(left + 8.0, rect.center().y), icon, 16.0, look::LABEL);
            p.text(pos2(left + 22.0, rect.center().y), Align2::LEFT_CENTER, words, font, look::LABEL);
        }
        Look::HideWord => {
            // A real pill: it presses, and it says what goes.
            let radius = look::radius(rect.height() / 2.0);
            p.rect_filled(rect, radius, if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::TILE });
            p.rect_stroke(rect.shrink(0.5), radius, Stroke::new(1.0, Color32::from_white_alpha(if hovered { 120 } else { 54 })), egui::StrokeKind::Inside);
            let words = if zone == Zone::WheelHide { tr("Hide ring") } else { tr("Hide tiles") };
            let font = look::font(13.0, weight::SEMIBOLD);
            let wide = p.layout_no_wrap(words.to_string(), font.clone(), look::LABEL).size().x;
            let left = rect.center().x - (wide + 22.0) / 2.0;
            widgets::icon(p, pos2(left + 8.0, rect.center().y), icons::EYE_SLASH, 16.0, look::LABEL);
            p.text(pos2(left + 22.0, rect.center().y), Align2::LEFT_CENTER, words, font, look::LABEL);
        }
        Look::Close => {
            p.circle_filled(rect.center(), rect.width() / 2.0, if hovered { look::TILE_HOVER } else { look::TILE });
            widgets::icon(p, rect.center(), icons::X, rect.width() * 0.55, look::LABEL);
        }
        Look::Caption => draw_caption(p, zone, rect, facts),
        Look::Capture(n) => draw_capture(p, rect, n, facts, hovered, pressed),
        Look::BigPicture(n) => draw_big_picture(p, rect, n, facts),
        Look::Step(after) => {
            p.circle_filled(rect.center(), rect.width() / 2.0, if hovered { theme::INK_750 } else { theme::alpha(theme::INK_900, 0.88) });
            p.circle_stroke(rect.center(), rect.width() / 2.0, Stroke::new(1.5, Color32::from_white_alpha(70)));
            widgets::icon(p, rect.center(), if after { icons::CARET_RIGHT } else { icons::CARET_LEFT }, 26.0, look::LABEL);
        }
        Look::Filter(filter) => {
            let on = facts.gallery_filter == filter;
            // As the world lists' choice is shown.
            let fill = if on { Color32::WHITE } else if hovered { look::TILE_HOVER } else { theme::alpha(look::TILE, 0.7) };
            p.rect_filled(rect, look::radius(rect.height() / 2.0), fill);
            let words = if filter == GalleryFilter::All { tr_ctx("gallery filter", "All") } else { tr("Only 3D") };
            p.text(rect.center(), Align2::CENTER_CENTER, words, look::font(15.0, weight::SEMIBOLD), if on { theme::INK_900 } else { look::LABEL });
        }
    }
}

/// A 3D picture's button: says "3D picture" on itself, a 3D badge on its icon, and what of. Under way, it is the countdown.
#[allow(clippy::too_many_arguments)]
fn picture_button(p: &Painter, zone: Zone, shape: Shaped, rect: Rect, mode: PhotoMode, facts: &Facts, hovered: bool, pressed: bool) {
    let mine = facts.picture_mode == Some(mode);
    let busy = facts.countdown.is_some() || facts.picture_flying;
    let lit = mine && busy;
    let dim = (busy && !mine) || facts.no_kit;
    match shape.ring {
        Some((ring, center)) => {
            if lit {
                p.rect_filled(rect, 0.0, look::accent());
            } else {
                ring_fill(p, ring, center, if pressed { 1.0 } else if hovered { 0.8 } else { 0.0 });
                ring_rim(p, ring, center, hovered);
            }
        }
        None => {
            if lit {
                p.rect_filled(rect, look::radius(look::RADIUS), look::accent());
            } else {
                tile_back(p, rect, hovered, pressed, false);
            }
        }
    }
    let (ink, label) = if lit {
        (look::on_accent(), look::on_accent())
    } else if dim {
        (theme::FG_4, theme::FG_4)
    } else {
        (look::LABEL, look::LABEL)
    };
    let Some((ring, center)) = shape.ring else {
        // On the sheet, a coloured tile as All tools': the two kinds of picture told apart, as on the app's own 3D capture page. In the gallery's
        // thin row, its icon alone is: what is under it comes first.
        let tint = if mode == PhotoMode::People { theme::VIOLET } else { look::accent() };
        let radius = look::radius(look::TILE_RADIUS + 2.0);
        if !lit && rect.height() < 60.0 {
            p.rect_filled(rect, look::radius(rect.height() / 2.0), if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { theme::alpha(look::TILE, 0.7) });
        } else if !lit {
            p.rect_filled(rect, radius, theme::mix(look::TILE, tint, if pressed { 0.28 } else if hovered { 0.22 } else if dim { 0.04 } else { 0.14 }));
            if hovered {
                p.rect_stroke(rect.shrink(1.0), radius, Stroke::new(1.5, tint), egui::StrokeKind::Middle);
            }
        }
        if rect.height() < 60.0 {
            let glyph = if mode == PhotoMode::People { icons::USERS } else { icons::IMAGE };
            let words = match facts.countdown.filter(|_| mine) {
                Some(seconds) => tr!("3D picture in {seconds}", seconds = seconds),
                None if mine && facts.picture_flying => tr("Flying\u{2026}").to_string(),
                None if facts.no_kit => tr("Needs the capture kit").to_string(),
                None if mine && facts.picture_wait.is_some() => tr("Press to call it off").to_string(),
                None if mode == PhotoMode::People => tr("3D picture of people").to_string(),
                None => tr("3D picture of this world").to_string(),
            };
            widgets::icon(p, pos2(rect.left() + 26.0, rect.center().y), glyph, 22.0, if lit || dim { ink } else { theme::mix(tint, Color32::WHITE, 0.2) });
            let font = look::font(15.5, weight::SEMIBOLD);
            p.text(pos2(rect.left() + 48.0, rect.center().y), Align2::LEFT_CENTER, fit(p, &words, &font, rect.width() - 56.0), font, label);
            return;
        }
        let at = pos2(rect.left() + 44.0, rect.center().y);
        let x = rect.left() + 84.0;
        let room = rect.right() - x - 8.0;
        if mine {
            if let Some(seconds) = facts.countdown {
                text(p, at, Align2::CENTER_CENTER, &seconds.to_string(), 44.0, weight::BOLD, ink);
                text(p, pos2(x, rect.center().y), Align2::LEFT_CENTER, tr("3D picture in"), 20.0, weight::BOLD, ink);
                return;
            }
            if facts.picture_flying {
                widgets::icon(p, at, icons::CUBE, 36.0, ink);
                text(p, pos2(x, rect.center().y), Align2::LEFT_CENTER, tr("Flying…"), 20.0, weight::BOLD, ink);
                return;
            }
        }
        let (glyph, of) = if mode == PhotoMode::People { (icons::USERS, tr("of people")) } else { (icons::IMAGE, tr("of this world")) };
        let of = if facts.no_kit {
            tr("needs the capture kit")
        } else if facts.picture_wait.is_some() && mine {
            tr("press to call off")
        } else {
            of
        };
        p.rect_filled(Rect::from_center_size(at, Vec2::splat(56.0)), look::radius(14.0), theme::alpha(tint, 0.22));
        widgets::icon(p, at, glyph, 32.0, if dim { theme::FG_4 } else { tint });
        badge(p, at, 34.0, if dim { theme::INK_600 } else { look::accent() }, look::on_accent());
        text(p, pos2(x, rect.center().y - 12.0), Align2::LEFT_CENTER, tr("3D picture"), 20.0, weight::BOLD, label);
        let small = look::font(15.0, weight::MEDIUM);
        p.text(pos2(x, rect.center().y + 14.0), Align2::LEFT_CENTER, fit(p, of, &small, room), small, if dim { label } else { look::MUTED });
        return;
    };
    let middle = ring.middle();
    if mine {
        if let Some(seconds) = facts.countdown {
            curved(p, center, ring.outer - 11.0, middle, tr("3D picture in"), 11.5, weight::BOLD, ink, false);
            text(p, center + zones::polar(ring.inner + 22.0, middle), Align2::CENTER_CENTER, &seconds.to_string(), 26.0, weight::BOLD, ink);
            return;
        }
        if facts.picture_flying {
            widgets::icon(p, center + zones::polar(ring.inner + 18.0, middle), icons::CUBE, 24.0, ink);
            curved(p, center, ring.outer - 12.0, middle, tr("Flying…"), 12.0, weight::BOLD, ink, false);
            return;
        }
    }
    let (glyph, of) = if mode == PhotoMode::People { (icons::USERS, tr("3D people")) } else { (icons::IMAGE, tr("3D world")) };
    let of = if facts.no_kit {
        tr("Needs the kit")
    } else if facts.picture_wait.is_some() && mine {
        tr("Call it off")
    } else {
        of
    };
    let (at, size) = cell_icon(ring, center);
    widgets::icon(p, at, glyph, size * 0.9, if lit || dim { ink } else if hovered { Color32::WHITE } else { theme::mix(look::accent(), Color32::WHITE, 0.3) });
    curved(p, center, ring.outer - 11.5, middle, of, CELL_WORDS - 1.0, weight::BOLD, label, true);
    if facts.key_caps {
        digit(p, zone, ring, center);
    }
}

/// A capture in the gallery, as the app's own Photos & captures has its tiles: the picture alone, rounded, a dark foot with its name and what it is on it.
fn draw_capture(p: &Painter, rect: Rect, n: usize, facts: &Facts, hovered: bool, pressed: bool) {
    let clip = p.with_clip_rect(Rect::from_min_max(pos2(0.0, GALLERY_TOP), pos2(1024.0, 576.0)));
    let Some(recent) = facts.recent.get(n) else { return };
    const RADIUS: f32 = 16.0;
    let radii = [theme::squircle(RADIUS); 4];
    let points = orbiters_ui::paint::squircle_points_corners(rect, radii);
    let (state, progress) = facts.recent_words.iter().find(|w| w.0 == n).map_or_else(|| (recent.state.clone(), recent.progress), |w| (w.1.clone(), w.2));
    match tile_picture(facts, n) {
        Some(id) => {
            let v = if pressed { 200 } else { 255 };
            squircle_picture(&clip, rect, radii, id, cover_uv_of(rect.size(), recent.shape()), Color32::from_rgb(v, v, v));
        }
        None => {
            orbiters_ui::paint::fill_convex(&clip, &points, |_| theme::INK_750);
            let glyph = if recent.kind == RecentKind::Picture { icons::IMAGE } else { icons::CUBE };
            widgets::icon(&clip, rect.center() - vec2(0.0, 14.0), glyph, 36.0, look::MUTED);
        }
    }
    let foot = if hovered { 0.97 } else { 0.92 };
    orbiters_ui::paint::fill_gradient(
        &clip,
        &points,
        &orbiters_ui::paint::Gradient::vertical(rect, &[(0.0, Color32::TRANSPARENT), (0.34, Color32::TRANSPARENT), (0.62, theme::alpha(theme::INK_900, 0.76)), (1.0, theme::alpha(theme::INK_950, foot))]),
    );
    let room = rect.width() - 28.0;
    let font = look::font(19.0, weight::BOLD);
    clip.text(pos2(rect.left() + 14.0, rect.bottom() - 37.0), Align2::LEFT_CENTER, fit(&clip, &recent.title, &font, room), font, look::LABEL);
    let font = look::font(15.0, weight::MEDIUM);
    clip.text(pos2(rect.left() + 14.0, rect.bottom() - 15.0), Align2::LEFT_CENTER, fit(&clip, &state, &font, room), font, theme::FG_2);
    // In one gallery with VRChat's pictures, what can show in 3D says so.
    if recent.kind == RecentKind::Capture {
        let badge = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(68.0, 32.0));
        clip.rect_filled(badge, look::radius(16.0), theme::alpha(theme::INK_900, 0.94));
        widgets::icon(&clip, badge.min + vec2(21.0, 16.0), icons::CUBE, 19.0, look::accent());
        clip.text(badge.min + vec2(36.0, 16.0), Align2::LEFT_CENTER, "3D", look::font(16.0, weight::BOLD), Color32::WHITE);
    }
    if let Some(done) = progress {
        let bar = Rect::from_min_size(pos2(rect.left() + 14.0, rect.bottom() - 6.0), vec2(room * done.clamp(0.0, 1.0), 3.0));
        clip.rect_filled(bar, look::radius(1.5), look::accent());
    }
    if hovered {
        clip.rect_stroke(rect.shrink(1.0), look::radius(RADIUS), Stroke::new(2.0, look::accent()), egui::StrokeKind::Middle);
    }
}

/// A tile's picture, when it has come.
fn tile_picture(facts: &Facts, n: usize) -> Option<TextureId> {
    facts.recent_pictures.iter().find(|w| w.0 == n).map(|w| w.1).or_else(|| facts.recent.get(n).and_then(|r| r.picture))
}

/// A picture of the gallery whole on its page, on its small picture until the large one has come.
fn draw_big_picture(p: &Painter, rect: Rect, n: usize, facts: &Facts) {
    let Some(recent) = facts.recent.get(n) else { return };
    if let Some(view) = &facts.opened_view {
        draw_turning(p, rect, n, facts, view);
        return;
    }
    p.rect_filled(rect, look::radius(12.0), theme::alpha(theme::INK_900, 0.92));
    let room = rect.shrink(8.0);
    let shape = recent.shape();
    let size = if room.width() / room.height() > shape { vec2(room.height() * shape, room.height()) } else { vec2(room.width(), room.width() / shape) };
    let shown = Rect::from_center_size(room.center(), size);
    match facts.opened_picture.or_else(|| tile_picture(facts, n)) {
        Some(id) => {
            squircle_picture(p, shown, [theme::squircle(10.0); 4], id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => widgets::icon(p, shown.center(), icons::IMAGE, 54.0, look::MUTED),
    }
    let state = facts.recent_words.iter().find(|w| w.0 == n).map_or(recent.state.as_str(), |w| w.1.as_str());
    let words = format!("{}, {}", recent.title, state);
    let font = look::font(16.0, weight::SEMIBOLD);
    let words = fit(p, &words, &font, 760.0);
    let wide = p.layout_no_wrap(words.clone(), font.clone(), look::LABEL).size().x + 28.0;
    let pill = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 30.0), vec2(wide, 34.0));
    p.rect_filled(pill, look::radius(17.0), theme::alpha(theme::INK_900, 0.82));
    p.text(pill.center(), Align2::CENTER_CENTER, words, font, look::LABEL);
}

/// A capture turning on the sheet: on a dark stage, on its plate, live; its own picture until that is ready, and if it cannot be.
fn draw_turning(p: &Painter, rect: Rect, n: usize, facts: &Facts, view: &OpenedView) {
    let Some(recent) = facts.recent.get(n) else { return };
    let stage = rect.shrink(4.0);
    p.rect_filled(stage, look::radius(12.0), Color32::from_rgb(15, 17, 21));
    let waiting = |p: &Painter, words: &str| {
        if let Some(id) = facts.opened_picture.or_else(|| tile_picture(facts, n)) {
            squircle_picture(p, stage, [theme::squircle(12.0); 4], id, cover_uv_of(stage.size(), recent.shape()), Color32::from_gray(120));
        }
        let font = look::font(17.0, weight::SEMIBOLD);
        let wide = p.layout_no_wrap(words.to_string(), font.clone(), look::LABEL).size().x + 34.0;
        let pill = Rect::from_center_size(stage.center(), vec2(wide, 40.0));
        p.rect_filled(pill, look::radius(20.0), theme::alpha(theme::INK_900, 0.9));
        p.text(pill.center(), Align2::CENTER_CENTER, words, font, look::LABEL);
    };
    match (&view.failed, view.picture) {
        (Some(_), _) => waiting(p, tr("Its 3D cannot show here. It can in Orbiters.")),
        (None, Some(id)) => {
            // The plate under it, as in a headset.
            let points: Vec<Pos2> = view.plate.iter().map(|uv| pos2(rect.left() + uv[0] * rect.width(), rect.top() + uv[1] * rect.height())).collect();
            if points.len() > 2 {
                p.add(Shape::convex_polygon(points.clone(), Color32::from_rgb(25, 28, 34), Stroke::NONE));
                p.add(Shape::closed_line(points, Stroke::new(2.5, theme::alpha(look::accent(), 0.85))));
            }
            keep_colour(p, rect);
            p.add(RectShape::filled(rect, 0.0, Color32::WHITE).with_texture(id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
        }
        (None, None) => waiting(p, tr("Getting its 3D ready\u{2026}")),
    }
    // What it is, and what can be done with it, on the side the button leaves.
    let state = facts.recent_words.iter().find(|w| w.0 == n).map_or(recent.state.as_str(), |w| w.1.as_str());
    let title = format!("{}, {}", recent.title, state);
    let foot = Rect::from_min_size(pos2(rect.left() + 20.0, rect.bottom() - 68.0), vec2(640.0, 50.0));
    let (big, small) = (look::font(17.0, weight::BOLD), look::font(14.5, weight::MEDIUM));
    let title = fit(p, &title, &big, foot.width() - 28.0);
    let said = tr("Drag to turn it. The mouse's wheel brings it closer.");
    let wide = p.layout_no_wrap(title.clone(), big.clone(), look::LABEL).size().x.max(p.layout_no_wrap(said.to_string(), small.clone(), look::LABEL).size().x) + 28.0;
    let pill = Rect::from_min_size(foot.min, vec2(wide, foot.height()));
    p.rect_filled(pill, look::radius(14.0), theme::alpha(theme::INK_900, 0.82));
    p.text(pos2(pill.left() + 14.0, pill.top() + 15.0), Align2::LEFT_CENTER, title, big, look::LABEL);
    p.text(pos2(pill.left() + 14.0, pill.top() + 35.0), Align2::LEFT_CENTER, said, small, look::LABEL);
}

/// The words about what is pointed at, or what to do first, in a pill as wide as they are. Nothing to say: nothing shows.
fn draw_caption(p: &Painter, zone: Zone, rect: Rect, facts: &Facts) {
    let on_the_wheel = zone == Zone::WheelCaption;
    // On a desktop's wheel what is pointed at is said in the disc in its middle, nearer than any caption.
    // And on a sheet with its band up, in that: one place says it.
    let pointed = facts.pointed.filter(|_| !(on_the_wheel && facts.digits) && !(facts.hint_band && facts.sheet.is_some()));
    let words = said_now(facts).or_else(|| pointed.and_then(|a| caption(a, facts))).or_else(|| facts.hint.map(|(a, b)| (tr(a).to_string(), tr(b).to_string())));
    if on_the_wheel && facts.digits && facts.pointed.is_some() && words.is_none() {
        return;
    }
    // On the wheel, whose it is, whatever it has to say.
    let words = if on_the_wheel { Some(words.unwrap_or_else(|| ("Orbiters".to_string(), String::new()))) } else { words };
    let Some((first, second)) = words else { return };
    // What plays's own picture beside its words, where it is what is pointed at.
    let on_the_wing = zone == Zone::QmCaption && !facts.strip_caption;
    if let (Some(Action::PlayPause), Some(id), false) = (pointed, facts.art, on_the_wing) {
        draw_words_with_art(p, rect, &first, &second, id, false);
        return;
    }
    if on_the_wing {
        draw_words_left(p, rect, &first, &second);
        return;
    }
    draw_words_marked(p, rect, &first, &second, on_the_wheel);
}

/// How a waiting picture is called off, from what is up now.
fn wait_off(facts: &Facts) -> &'static str {
    match facts.sheet {
        Some(Sheet::Pictures) => tr("Press its button again to call it off"),
        Some(_) => tr("Pictures calls it off"),
        None if facts.digits => tr("Press 6 again to call it off"),
        None => tr("Press Pictures again to call it off"),
    }
}

/// What goes before anything else the caption would say: what stopped a 3D picture, or what one waits for.
fn said_now(facts: &Facts) -> Option<(String, String)> {
    facts.note.clone().or_else(|| facts.picture_wait.map(|wait| wait_words(wait, facts.picture_mode, wait_off(facts)))).or_else(|| facts.n4n0_says.clone().map(|said| ("N4N0".to_string(), said)))
}

/// What a sheet's head says of what is going on.
fn status_now(facts: &Facts) -> Option<(String, String)> {
    if let Some(seconds) = facts.countdown {
        return Some((tr!("3D picture in {seconds}", seconds = seconds), tr("VRChat's camera then flies around by itself.").into()));
    }
    if facts.picture_flying {
        return Some((tr("3D picture in progress").into(), tr("The camera is flying. Stay where you are for a moment.").into()));
    }
    said_now(facts)
        .or_else(|| facts.invite_said.clone().map(|said| (sheet_name(&facts.sheet_title).to_string(), said)))
        .or_else(|| facts.view_said.clone().filter(|_| facts.sheet == Some(Sheet::Pictures)))
}

/// What a 3D picture waits for, in the caption's two lines: which picture, the steps, and how to call it off where this is said.
pub fn wait_words(wait: orbiters_n4n0::status::PhotoWait, mode: Option<PhotoMode>, off: &str) -> (String, String) {
    match wait {
        // One whole sentence for each picture: the picture's name does not go in front of a fixed part in every language.
        orbiters_n4n0::status::PhotoWait::Camera => {
            let first = if mode == Some(PhotoMode::People) { tr("3D picture of people: waiting for the camera") } else { tr("3D picture of this world: waiting for the camera") };
            (first.to_string(), tr!("Esc, the camera tab, then Stream Camera  \u{b7}  {off}", off = off))
        }
        orbiters_n4n0::status::PhotoWait::Move => (tr("Move VRChat's camera a little").into(), tr("The 3D picture starts as soon as Orbiters sees where it is.").into()),
    }
}

/// Two lines in a pill as wide as they are.
pub fn draw_words(p: &Painter, rect: Rect, first: &str, second: &str) {
    draw_words_marked(p, rect, first, second, false);
}

/// Its words on the wing: as wide as the wing is, right over its name, as high as they take.
fn draw_words_left(p: &Painter, rect: Rect, first: &str, second: &str) {
    let room = rect.width() - 20.0;
    let lines = |words: &str, font: FontId, ink: Color32, rows: usize| {
        let mut job = egui::text::LayoutJob::simple(words.to_string(), font, ink, room);
        job.wrap.max_rows = rows;
        job.wrap.overflow_character = Some('\u{2026}');
        p.layout_job(job)
    };
    let (big, small) = (lines(first, look::font(13.0, weight::BOLD), look::LABEL, 2), lines(second, look::font(11.5, weight::MEDIUM), look::MUTED, 4));
    let high = big.size().y + if second.is_empty() { 0.0 } else { small.size().y + 2.0 } + 14.0;
    let pill = Rect::from_min_max(pos2(rect.left(), rect.bottom() - high), rect.max);
    p.rect_filled(pill, look::radius(12.0), look::PANEL);
    let below = big.size().y;
    p.galley(pill.min + vec2(10.0, 7.0), big, look::LABEL);
    if !second.is_empty() {
        p.galley(pill.min + vec2(10.0, 9.0 + below), small, look::MUTED);
    }
}

/// The same, with what plays's picture at its start.
fn draw_words_with_art(p: &Painter, rect: Rect, first: &str, second: &str, art: TextureId, from_left: bool) {
    let (big, small) = (look::font(13.0, weight::BOLD), look::font(11.5, weight::MEDIUM));
    let room = rect.width() - 66.0;
    let (first, second) = (fit(p, first, &big, room), fit(p, second, &small, room));
    let wide = |s: &str, f: &FontId| p.layout_no_wrap(s.to_string(), f.clone(), Color32::WHITE).size().x;
    let width = wide(&first, &big).max(wide(&second, &small)) + 66.0;
    let left = if from_left { rect.left() } else { rect.center().x - width / 2.0 };
    let pill = Rect::from_min_size(pos2(left, rect.bottom() - 46.0), vec2(width, 46.0));
    p.rect_filled(pill, look::radius(12.0), look::PANEL);
    let picture = Rect::from_min_size(pill.min + vec2(6.0, 6.0), Vec2::splat(34.0));
    keep_colour(p, picture);
    p.add(RectShape::filled(picture, look::radius(6.0), Color32::WHITE).with_texture(art, cover_uv(picture.size())));
    p.text(pos2(pill.left() + 50.0, pill.top() + 15.0), Align2::LEFT_CENTER, first, big, look::LABEL);
    p.text(pos2(pill.left() + 50.0, pill.top() + 31.0), Align2::LEFT_CENTER, second, small, look::MUTED);
}

/// [`draw_words`], with Orbiters' mark in front when `mark`: what is on the wheel does not otherwise say whose it is.
fn draw_words_marked(p: &Painter, rect: Rect, first: &str, second: &str, mark: bool) {
    let big = look::font(13.0, weight::BOLD);
    let small = look::font(11.0, weight::MEDIUM);
    let room = rect.width() - 24.0;
    let first = fit(p, first, &big, room);
    let second = fit(p, second, &small, room);
    let width = p.layout_no_wrap(first.clone(), big.clone(), Color32::WHITE).size().x.max(p.layout_no_wrap(second.clone(), small.clone(), Color32::WHITE).size().x) + 24.0;
    let height = if second.is_empty() { 30.0 } else { rect.height() - 8.0 };
    let pill = Rect::from_center_size(rect.center(), vec2(width + if mark { 22.0 } else { 0.0 }, height));
    p.rect_filled(pill, look::radius(look::RADIUS + 2.0), look::PANEL);
    if second.is_empty() {
        p.text(pill.center() + vec2(if mark { 11.0 } else { 0.0 }, 0.0), Align2::CENTER_CENTER, first, big, look::LABEL);
        if mark {
            widgets::icon(p, pill.left_center() + vec2(15.0, 0.0), icons::N4N0_MARK, 15.0, look::LABEL);
        }
        return;
    }
    let shift = vec2(if mark { 11.0 } else { 0.0 }, 0.0);
    if mark {
        widgets::icon(p, pill.left_center() + vec2(16.0, 0.0), icons::N4N0_MARK, 16.0, look::LABEL);
    }
    p.text(pill.center() + shift - vec2(0.0, 9.0), Align2::CENTER_CENTER, first, big, look::LABEL);
    p.text(pill.center() + shift + vec2(0.0, 9.0), Align2::CENTER_CENTER, second, small, look::MUTED);
}

/// The part of a 16:9 thumbnail that fills a box of this shape without stretching it.
fn cover_uv(size: Vec2) -> Rect {
    cover_uv_of(size, 16.0 / 9.0)
}

/// The part of a picture of this shape that fills a box of this size.
fn cover_uv_of(size: Vec2, picture_aspect: f32) -> Rect {
    let box_aspect = size.x / size.y.max(1.0);
    if box_aspect > picture_aspect {
        let v = picture_aspect / box_aspect;
        Rect::from_min_max(pos2(0.0, (1.0 - v) / 2.0), pos2(1.0, (1.0 + v) / 2.0))
    } else {
        let u = box_aspect / picture_aspect;
        Rect::from_min_max(pos2((1.0 - u) / 2.0, 0.0), pos2((1.0 + u) / 2.0, 1.0))
    }
}

/// A cell's shape as one byte a pixel (0 to 255), for a picture of `px` pixels at `pixels_per_unit`, the cell's bounds at its corner: worked out once per size and multiplied
/// into each picture, so that painting costs what it cost.
pub fn cell_mask(ring: Ring, px: [u32; 2], pixels_per_unit: f32) -> Vec<u8> {
    let min = ring.bounds().min.to_vec2();
    let soft = 1.0 / pixels_per_unit;
    let mut out = Vec::with_capacity(px[0] as usize * px[1] as usize);
    for y in 0..px[1] {
        for x in 0..px[0] {
            let p = vec2(x as f32 + 0.5, y as f32 + 0.5) * soft + min;
            out.push((ring.coverage(p, soft) * 255.0 + 0.5) as u8);
        }
    }
    out
}

/// `mask` multiplied into the part `within` (x0, y0, x1, y1) of premultiplied `rgba`.
pub fn apply_mask(rgba: &mut [u8], mask: &[u8], size: [u32; 2], within: [u32; 4]) {
    let [w, h] = size;
    for y in within[1]..within[3].min(h) {
        for x in within[0]..within[2].min(w) {
            let i = (y * w + x) as usize;
            let m = mask[i] as u32;
            if m == 255 {
                continue;
            }
            for v in &mut rgba[i * 4..i * 4 + 4] {
                *v = (*v as u32 * m / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::orbit::zones::{self, Around};

    fn facts() -> Facts {
        Facts {
            recent: (0..4).map(|i| Recent { dir: PathBuf::from(format!("s{i}")), title: format!("World {i}"), state: "3D picture, 2 h ago".into(), progress: None, picture: None, ..Default::default() }).collect(),
            timer: 3,
            n4n0: true,
            ..Default::default()
        }
    }

    #[test]
    fn everything_lies_inside_its_part() {
        for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (960.0, 540.0), (1000.0, 900.0), (1700.0, 600.0)] {
            for around in [Around::Wheel, Around::QuickMenu] {
                for placed in zones::layout(around, vec2(w, h)) {
                    let size = placed.logical();
                    for item in items(placed.zone, size, &facts()) {
                        assert!(Rect::from_min_size(pos2(0.0, 0.0), size).expand(0.5).contains_rect(item.rect), "{w}x{h} {:?}: {:?} in {size:?}", placed.zone, item.rect);
                    }
                }
            }
        }
    }

    #[test]
    fn what_a_cell_says_stays_inside_its_shape() {
        for zone in Zone::WHEEL {
            let Some(ring) = zone.ring() else { continue };
            let anchor = ring.anchor();
            let (s, c) = ring.middle().to_radians().sin_cos();
            let across = vec2(c, s) * (ring.room() / 2.0);
            assert!(ring.contains(anchor), "{zone:?}");
            assert!(ring.contains(anchor + across * 0.98) && ring.contains(anchor - across * 0.98), "{zone:?}: {}", ring.room());
        }
    }

    /// What the first level offers around each menu.
    fn first_level(list: &[Zone]) -> Vec<Action> {
        list.iter().filter(|z| z.group().is_none()).flat_map(|z| items(*z, z.units().size(), &facts())).map(|i| i.action).collect()
    }

    #[test]
    fn world_search_instances_youtube_and_what_plays_come_first_on_both_menus_and_capture_is_one_among_them() {
        for list in [&Zone::WHEEL[..], &Zone::QUICK_MENU[..]] {
            let actions = first_level(list);
            for needed in [Action::Worlds, Action::Page(Target::Instances), Action::Page(Target::YouTube), Action::PlayPause, Action::Pictures, Action::All] {
                assert!(actions.contains(&needed), "{needed:?}");
            }
            // Taking a picture is inside 3D pictures, not beside the others.
            assert!(!actions.contains(&Action::Picture(PhotoMode::People)) && !actions.contains(&Action::Picture(PhotoMode::World)));
        }
        // And the two choices are a hover away on the wheel, whose second ring holds them.
        let bloom: Vec<Action> = zones::Group::Pictures.cells().iter().flat_map(|z| items(*z, z.units().size(), &facts())).map(|i| i.action).collect();
        assert!(bloom.contains(&Action::Picture(PhotoMode::People)) && bloom.contains(&Action::Picture(PhotoMode::World)) && bloom.contains(&Action::Record));
    }

    #[test]
    fn all_tools_offers_only_what_can_work_here() {
        let names = |f: &Facts| -> Vec<&'static str> {
            sheet_items(Sheet::All, f)
                .iter()
                .filter_map(|i| match i.look {
                    Look::Hero { label, .. } | Look::Small { label, .. } => Some(label),
                    Look::NowPlaying => Some("Now playing"),
                    _ => None,
                })
                .collect()
        };
        let desktop = Facts { desktop: true, sheet: Some(Sheet::All), ..facts() };
        let on = names(&desktop);
        // The tabs would only repeat the large tiles: Enzo, and nowhere else.
        assert!(!items(Zone::SheetBack, vec2(704.0, 445.5), &desktop).iter().any(|i| matches!(i.look, Look::HeadTab { .. })));
        for absent in ["Call", "Saved looks", "Light", "Fast walk", "Smooth tracking"] {
            assert!(!on.contains(&absent), "{absent} on a desktop with none of it");
        }
        for present in ["Worlds in 3D", "YouTube", "Instances", "Now playing", "Pictures", "Open Orbiters"] {
            assert!(on.contains(&present), "{present}");
        }
        let vr = names(&Facts { desktop: false, can_call: true, can_states: true, can_light: true, ..facts() });
        for present in ["Call", "Saved looks", "Light", "Fast walk", "Smooth tracking"] {
            assert!(vr.contains(&present), "{present}");
        }
        // They all fit the page, without overlapping.
        for list in [sheet_items(Sheet::All, &Facts { can_call: true, can_states: true, can_light: true, ..facts() }), sheet_items(Sheet::Pictures, &facts())] {
            for (i, a) in list.iter().enumerate() {
                // The gallery runs past the page's foot, clipped, and scrolls.
                let scrolls = matches!(a.look, Look::Capture(_));
                assert!(a.rect.left() >= 0.0 && a.rect.right() <= 1024.0 && a.rect.top() >= 0.0 && (scrolls || a.rect.bottom() <= 576.0), "{:?}", a.rect);
                for b in &list[i + 1..] {
                    assert!(!a.rect.intersects(b.rect), "{:?} {:?}", a.look, b.look);
                }
            }
        }
    }

    /// Round 8's judges: over cells whose keys work, numbered steps read as keys to press.
    #[test]
    fn what_a_picture_waits_for_numbers_no_step() {
        let (_, steps) = wait_words(orbiters_n4n0::status::PhotoWait::Camera, None, "");
        assert!(!steps.chars().any(|c| c.is_ascii_digit()), "{steps}");
    }

    /// Enzo: a retro look, of one colour or with its pictures in theirs.
    #[test]
    fn the_retro_ramp_goes_through_the_one_colour_and_keeps_what_is_transparent() {
        let ramp = retro_ramp([0x20, 0xe0, 0x60]);
        assert_eq!(ramp.len(), 256);
        assert!(ramp[0][1] < 30 && ramp[140][1] >= 0xd0 && ramp[255][0] > 0x90, "dark, the colour, nearly white");
        let mut pixels = vec![128, 128, 128, 255, 0, 0, 0, 0, 128, 128, 128, 128];
        in_ramp(&mut pixels, 3, [0, 0, 3, 1], &ramp);
        assert_eq!(&pixels[4..8], &[0, 0, 0, 0], "transparent stays");
        assert!(pixels[1] > pixels[0], "tinted");
        assert_eq!(pixels[11], 128, "its alpha stays");
        assert!(pixels[9] <= 128 && pixels[8] <= 128, "still premultiplied");
    }

    /// What the sheet of settings sends is what the app's settings take, or a press would do nothing, silently.
    #[test]
    fn what_the_sheet_of_settings_saves_is_taken_by_the_settings() {
        use orbiters_core::settings::{AudioEndpoint, Settings};
        let patch = serde_json::json!({ "orbit": { "layout": OrbitLayout::Strip, "vr_motion": VrMotion::Matched, "theme": MenuLook::Retro, "accent": [1, 2, 3], "retro_color": [4, 5, 6], "retro_pictures": true } });
        let (saved, left_out) = Settings::default().patched(&patch);
        assert!(left_out.is_empty(), "{left_out:?}");
        let o = saved.orbit;
        assert_eq!((o.layout, o.vr_motion, o.theme, o.accent, o.retro_color, o.retro_pictures), (OrbitLayout::Strip, VrMotion::Matched, MenuLook::Retro, [1, 2, 3], [4, 5, 6], true));
        let device = AudioEndpoint { id: "a".into(), name: "Speakers".into() };
        let (saved, left_out) = saved.patched(&serde_json::json!({ "vr_audio": { "output": Some(&device) } }));
        assert!(left_out.is_empty(), "{left_out:?}");
        assert_eq!(saved.vr_audio.output.as_ref(), Some(&device));
        let none: Option<&AudioEndpoint> = None;
        let (saved, left_out) = saved.patched(&serde_json::json!({ "vr_audio": { "output": none } }));
        assert!(left_out.is_empty(), "{left_out:?}");
        assert_eq!(saved.vr_audio.output, None, "leave it as it is");
        // Settings saved before these fields keep what they had, with the defaults for the new.
        let old: orbiters_core::settings::OrbitSettings = serde_json::from_str("{}").unwrap();
        assert_eq!((old.layout, old.theme, old.accent), (OrbitLayout::Wing, MenuLook::Modern, [0x00, 0xda, 0x6d]));
    }

    /// Enzo: the tabs are a good idea, only All tools has them already. Beside the wing as well; in a headset named or not there, when the head has something else to say.
    #[test]
    fn head_tabs_are_on_every_sheet_but_all_tools() {
        let tabs = |facts: &Facts| items(Zone::SheetBack, vec2(704.0, 445.5), facts).iter().filter(|i| matches!(i.look, Look::HeadTab { .. })).count();
        for desktop in [true, false] {
            for beside_wing in [true, false] {
                for sheet in [Sheet::Pictures, Sheet::Instances, Sheet::Worlds, Sheet::Settings, Sheet::Menu] {
                    let facts = Facts { desktop, beside_wing, sheet: Some(sheet), ..facts() };
                    assert_eq!(tabs(&facts), 6, "{sheet:?} on a screen: {desktop}, beside the wing: {beside_wing}");
                }
                assert_eq!(tabs(&Facts { desktop, beside_wing, sheet: Some(Sheet::All), ..facts() }), 0);
            }
        }
        let busy = Facts { desktop: false, sheet: Some(Sheet::Pictures), view_showing: true, ..facts() };
        assert_eq!(tabs(&busy), 0, "a headset's are named or not there");
        // A page with no tab of its own keeps room for its name, on a screen.
        let (_, x) = head_tabs(&Facts { desktop: true, sheet: Some(Sheet::Settings), ..facts() });
        assert!(x < 300.0, "{x}");
    }

    /// Under the menu, in a headset, as on the wing: a way to put the tiles away. Not where the sheet has its own close, nor on a screen.
    #[test]
    fn the_strip_can_be_hidden_in_a_headset() {
        let hide = |facts: &Facts| strip(600.0, 52.0, facts).into_iter().filter(|i| i.action == Action::Hide).collect::<Vec<_>>();
        let vr = Facts { vr_close: true, sheet: None, ..facts() };
        let found = hide(&vr);
        assert_eq!(found.len(), 1);
        let all = strip(600.0, 52.0, &vr);
        for (i, a) in all.iter().enumerate() {
            assert!(a.rect.right() <= 600.0 && a.rect.width() >= 50.0, "{:?}", a.rect);
            for b in &all[i + 1..] {
                assert!(!a.rect.intersects(b.rect), "{:?} {:?}", a.rect, b.rect);
            }
        }
        assert!(hide(&Facts { vr_close: false, sheet: None, ..facts() }).is_empty());
        assert!(hide(&Facts { vr_close: true, sheet: Some(Sheet::Pictures), ..facts() }).is_empty());
    }

    /// Dragging along the bar goes through the colours and stops at its ends, wherever the pointer goes.
    #[test]
    fn a_drag_along_the_colour_bar_follows_it_and_stays_on_it() {
        let bar = hue_bar();
        assert_eq!(hue_at(bar.left() - 200.0), hue_at(bar.left()));
        assert_eq!(hue_at(bar.right() + 200.0), hue_at(bar.right()));
        let along: Vec<[u8; 3]> = (0..=8).map(|n| hue_at(bar.left() + bar.width() * n as f32 / 8.0)).collect();
        for pair in along.windows(2) {
            assert_ne!(pair[0], pair[1], "every step of it is another colour");
        }
    }

    #[test]
    fn the_sheet_of_settings_offers_what_was_asked_and_fits() {
        let modern = Facts { sheet: Some(Sheet::Settings), ..facts() };
        let retro = Facts { orbit: OrbitSettings { theme: MenuLook::Retro, ..Default::default() }, ..modern.clone() };
        let picks = |f: &Facts| sheet_items(Sheet::Settings, f).iter().filter_map(|i| if let Action::Set(p) = i.action { Some(p) } else { None }).collect::<Vec<_>>();
        for pick in [Pick::Layout(OrbitLayout::Strip), Pick::Motion(VrMotion::Springy), Pick::Motion(VrMotion::Matched), Pick::Theme(MenuLook::Retro), Pick::Swatch(0), Pick::Hue, Pick::Sound(true)] {
            assert!(picks(&modern).contains(&pick), "{pick:?}");
        }
        assert!(!picks(&modern).contains(&Pick::Pictures(true)), "of the retro look alone");
        assert!(picks(&retro).contains(&Pick::Pictures(true)));
        for list in [sheet_items(Sheet::Settings, &modern), sheet_items(Sheet::Settings, &retro)] {
            for (i, a) in list.iter().enumerate() {
                assert!(a.rect.right() <= 1024.0 && a.rect.bottom() <= 576.0, "{:?}", a.rect);
                assert!(list[i + 1..].iter().all(|b| !a.rect.intersects(b.rect)), "{:?}", a.look);
            }
        }
        assert!(sheet_items(Sheet::All, &facts()).iter().any(|i| i.action == Action::Settings), "from All tools");
    }

    #[test]
    fn a_title_reads_without_what_its_page_separates_its_parts_with() {
        assert_eq!(clean_title("Laurent Colson - Memories Of The Future \\\\ Wave \\\\"), "Laurent Colson - Memories Of The Future \u{b7} Wave");
        assert_eq!(clean_title("A | B"), "A \u{b7} B");
        assert_eq!(clean_title("Plain"), "Plain");
    }

    #[test]
    fn all_tools_shows_what_matters_most_largest() {
        let area = |facts: &Facts, name: &str| {
            sheet_items(Sheet::All, facts)
                .iter()
                .find(|i| matches!(i.look, Look::Hero { label, .. } | Look::Small { label, .. } if label == name))
                .map(|i| i.rect.area())
                .unwrap_or_else(|| panic!("{name}"))
        };
        let facts = Facts { desktop: true, ..facts() };
        assert!(area(&facts, "Worlds in 3D") > area(&facts, "YouTube"), "the first thing of all");
        for main in ["YouTube", "Instances", "Pictures"] {
            for accessory in ["QR code", "Suggest", "World feedback", "Open Orbiters", "Ask N4N0"] {
                assert!(area(&facts, main) > area(&facts, accessory) * 1.1, "{main} against {accessory}");
            }
        }
        // What plays has its controls where it is, and no page to go to.
        let items = sheet_items(Sheet::All, &Facts { music: Some(("A song".into(), true)), volume: Some((50, false)), ..facts.clone() });
        for action in [Action::PlayPause, Action::Previous, Action::Next, Action::Volume(true), Action::Volume(false), Action::Mute] {
            assert!(items.iter().any(|i| i.action == action), "{action:?}");
        }
        assert!(!items.iter().any(|i| i.action == Action::Page(Target::Music)));
        // Its timeline takes the pointer only when the app playing it lets it: a press then says where.
        let playing = Facts { music: Some(("A song".into(), true)), music_times: Some((30, 120)), ..facts.clone() };
        assert!(sheet_items(Sheet::All, &playing).iter().any(|i| matches!(i.look, Look::Timeline) && i.action == Action::Idle));
        assert_eq!(timeline_fraction(&Facts { can_seek: true, ..playing.clone() }, 0.0), Some(0.0));
        let movable = Facts { can_seek: true, ..playing };
        let rect = sheet_items(Sheet::All, &movable).into_iter().find(|i| matches!(i.look, Look::Timeline)).unwrap();
        assert_eq!(rect.action, Action::Timeline);
        assert!((timeline_fraction(&movable, timeline_bar(rect.rect).center().x).unwrap() - 0.5).abs() < 0.001);
        assert_eq!(timeline_fraction(&Facts { music_times: None, ..movable }, 300.0), None);
    }

    #[test]
    fn the_gallery_scrolls_through_everything_and_keeps_its_buttons_on_top() {
        let many = |n: usize| Facts { recent: (0..n).map(|i| Recent { dir: PathBuf::from(format!("s{i}")), title: format!("W{i}"), state: String::new(), progress: None, picture: None, ..Default::default() }).collect(), desktop: true, ..facts() };
        let captures = |f: &Facts| sheet_items(Sheet::Pictures, f).iter().filter_map(|i| if let Look::Capture(n) = i.look { Some(n) } else { None }).collect::<Vec<_>>();
        let start = many(200);
        let shown = captures(&start);
        assert!(shown.contains(&0) && shown.len() < 30, "only what shows is laid on the page: {}", shown.len());
        let max = max_scroll(Sheet::Pictures, &start);
        assert!(max > 1000.0);
        let end = Facts { gallery_scroll: max, ..start.clone() };
        assert!(captures(&end).contains(&199), "the last one can be reached");
        assert!(!captures(&end).contains(&0));
        // A few captures do not scroll at all.
        assert_eq!(max_scroll(Sheet::Pictures, &many(2)), 0.0);
        // A capture passing under the buttons never takes their press.
        let middle = Facts { gallery_scroll: 90.0, ..start };
        let items = sheet_items(Sheet::Pictures, &middle);
        let button = items.iter().find(|i| i.action == Action::Picture(PhotoMode::People)).unwrap().rect;
        let on = hit(&items, button.center()).unwrap();
        assert_eq!(items[on].action, Action::Picture(PhotoMode::People));
    }

    /// Round 3's judges, of the headset: "11-12 px tiles, thin scroll handles, tight chips". Every target on a sheet is at least 2 degrees across its
    /// short side, at the arm's length a sheet is used at: page pixels are 0.62 mm there, with 0.9 mm a unit of the menu.
    #[test]
    fn what_can_be_pressed_on_a_sheet_is_large_enough_for_a_laser() {
        const MM_A_PIXEL: f32 = 0.0009 * 0.6875 * 1000.0;
        const AT_MM: f32 = 550.0;
        let degrees = |pixels: f32| 2.0 * (pixels * MM_A_PIXEL / 2.0 / AT_MM).atan().to_degrees();
        let tile = |n: usize| (0..n).map(|i| Recent { title: format!("W{i}"), kind: RecentKind::Picture, ..Default::default() }).collect::<Gallery>();
        let vr = Facts { desktop: false, can_call: true, can_states: true, can_light: true, music: Some(("A song".into(), true)), music_times: Some((30, 120)), can_seek: true, volume: Some((50, false)), recent: tile(60), ..Facts::default() };
        let visits = vec![WorldVisit { here: false, name: "A world".into(), earlier: vec![("a".into(), "Sat 06:33".into()); 3], ..Default::default() }; 20];
        let videos = vec![VideoTile { title: "A".into(), channel: "B".into(), duration: None, picture: None }; 12];
        let sheets = [
            (Sheet::All, vr.clone()),
            (Sheet::Pictures, vr.clone()),
            (Sheet::Instances, Facts { worlds_visited: visits, ..vr.clone() }),
            (Sheet::Results, Facts { videos, ..vr.clone() }),
            (Sheet::Talk, vr.clone()),
        ];
        for (sheet, facts) in sheets {
            let mut smallest = (f32::MAX, String::new());
            for item in sheet_items(sheet, &Facts { sheet: Some(sheet), ..facts.clone() }) {
                if item.action == Action::Idle {
                    continue;
                }
                let side = item.rect.width().min(item.rect.height());
                if side < smallest.0 {
                    smallest = (side, format!("{:?}", item.look));
                }
            }
            println!("{sheet:?}: smallest target {} px = {:.1} degrees ({})", smallest.0, degrees(smallest.0), smallest.1);
            assert!(degrees(smallest.0) >= 2.0, "{sheet:?}: {} {} px", smallest.1, smallest.0);
        }
    }

    /// Round 3's judges: "the first row cut mid-picture at rest".
    #[test]
    fn a_list_comes_to_rest_with_its_first_row_whole() {
        let visits = vec![WorldVisit::default(); 40];
        let facts = Facts { worlds_visited: visits, sheet: Some(Sheet::Instances), ..Facts::default() };
        assert_eq!(snap_scroll(Sheet::Instances, &facts, 0.0), 0.0);
        let row = VISIT_HEIGHT + VISIT_GAP;
        assert_eq!(snap_scroll(Sheet::Instances, &facts, row * 3.0 + 20.0), row * 3.0);
        assert_eq!(snap_scroll(Sheet::Instances, &facts, row * 3.0 + 80.0), row * 4.0);
        let max = max_scroll(Sheet::Instances, &facts);
        assert_eq!(snap_scroll(Sheet::Instances, &facts, max), max, "its very end stays its end");
    }

    /// Enzo: "when I click on a capture, instead of showing me the capture in 3D, it teleports me in a weird page".
    #[test]
    fn a_tile_acts_for_itself_and_a_picture_shows_large_with_a_way_back() {
        let kinds = [RecentKind::Capture, RecentKind::Picture, RecentKind::NoView, RecentKind::Picture];
        let recent: Gallery = kinds.iter().map(|kind| Recent { title: "A world".into(), kind: *kind, ..Default::default() }).collect();
        let facts = Facts { recent, sheet: Some(Sheet::Pictures), ..Facts::default() };
        let items = sheet_items(Sheet::Pictures, &facts);
        for n in 0..kinds.len() {
            assert!(items.iter().any(|i| i.action == Action::Capture(n) && matches!(i.look, Look::Capture(m) if m == n)), "{n}");
            assert!(caption(Action::Capture(n), &facts).is_some(), "says what it does");
        }
        assert!(!items.iter().any(|i| matches!(i.action, Action::Page(_))), "no page of captures to share");
        // What it shows can be chosen, on the page itself.
        assert!(items.iter().any(|i| i.action == Action::Filter(GalleryFilter::Captures)));
        assert!(caption(Action::Filter(GalleryFilter::Captures), &facts).is_some());
        // Large: back is there, the list keeps its place, and the steps go from picture to picture, over the captures.
        let opened = Facts { opened: Some(1), gallery_scroll: 40.0, ..facts.clone() };
        let large = sheet_items(Sheet::Pictures, &opened);
        assert!(large.iter().any(|i| i.action == Action::PictureBack));
        assert!(!large.iter().any(|i| matches!(i.action, Action::Capture(_) | Action::Filter(_) | Action::ScrollBar)));
        assert_eq!(picture_beside(&opened, 1, true), Some(3));
        assert_eq!(picture_beside(&opened, 1, false), None);
        assert!(!large.iter().any(|i| i.action == Action::PictureStep(false)), "nothing before it");
        assert!(large.iter().any(|i| i.action == Action::PictureStep(true)));
        assert_eq!(picture_beside(&opened, 3, true), None);
        assert_eq!(max_scroll(Sheet::Pictures, &opened), 0.0);
        // The same list is the same list, however large: nothing compares its tiles each frame.
        let again = facts.recent.clone();
        assert!(again == facts.recent);
        assert!(facts.recent != kinds.iter().map(|_| Recent::default()).collect::<Gallery>());
    }

    /// Enzo, in the headset: "when I grab a tile to scroll down, it just opens it".
    #[test]
    fn what_lies_in_a_list_waits_to_see_whether_it_is_a_drag_and_what_is_fixed_does_not() {
        let list = Look::Capture(0);
        assert!(scrolls_with_a_drag(Sheet::Pictures, &list));
        assert!(scrolls_with_a_drag(Sheet::Instances, &Look::Chip { group: 0, visit: 0 }));
        assert!(!scrolls_with_a_drag(Sheet::Pictures, &Look::Picture(PhotoMode::World)), "its buttons on top act at once");
        assert!(!scrolls_with_a_drag(Sheet::Pictures, &Look::Record));
    }

    #[test]
    fn previous_instances_scroll_through_every_world_and_lay_only_what_shows() {
        let visit = |i: usize| WorldVisit { here: false, world_id: format!("wrld_{i}"), name: format!("W{i}"), line: String::new(), location: format!("wrld_{i}:1"), earlier: Vec::new(), picture: None };
        let facts = Facts { worlds_visited: (0..100).map(visit).collect(), desktop: true, ..facts() };
        let rows = |f: &Facts| sheet_items(Sheet::Instances, f).iter().filter_map(|i| if let Look::Visit(n) = i.look { Some(n) } else { None }).collect::<Vec<_>>();
        assert!(rows(&facts).len() <= 8);
        let max = max_scroll(Sheet::Instances, &facts);
        assert!(rows(&Facts { gallery_scroll: max, ..facts.clone() }).contains(&99), "the last can be reached");
        for scroll in [0.0, 500.0, max] {
            let shown = rows(&Facts { gallery_scroll: scroll, ..facts.clone() });
            let range = visits_shown(scroll);
            assert!(shown.iter().all(|n| range.contains(n)), "{scroll}: {shown:?} {range:?}");
        }
    }

    #[test]
    fn youtube_results_are_one_list_and_lay_only_what_shows() {
        let video = |i: usize| VideoTile { title: format!("V{i}"), channel: "C".into(), duration: None, picture: None };
        let facts = Facts { videos: (0..200).map(video).collect(), desktop: true, ..facts() };
        let tiles = |f: &Facts| sheet_items(Sheet::Results, f).iter().filter_map(|i| if let Look::Video(n) = i.look { Some(n) } else { None }).collect::<Vec<_>>();
        assert!(tiles(&facts).contains(&0) && tiles(&facts).len() < 16);
        let max = max_scroll(Sheet::Results, &facts);
        assert!(tiles(&Facts { gallery_scroll: max, ..facts.clone() }).contains(&199), "the last can be reached");
        assert!(scrolls_with_a_drag(Sheet::Results, &Look::Video(0)));
        assert!(scrolls_with_a_drag(Sheet::Results, &Look::VideoCopy(0)));
        assert!(!scrolls_with_a_drag(Sheet::Results, &Look::Back), "its back acts at once");
        // Its Drop lies on top of its tile, for the pointer too.
        let items = sheet_items(Sheet::Results, &facts);
        let chip = items.iter().find(|i| i.action == Action::VideoDrop(0)).unwrap().rect;
        assert_eq!(items[hit(&items, chip.center()).unwrap()].action, Action::VideoDrop(0));
        for scroll in [0.0, 900.0, max] {
            let shown = tiles(&Facts { gallery_scroll: scroll, ..facts.clone() });
            assert!(shown.iter().all(|n| videos_shown(scroll).contains(n)), "{scroll}: {shown:?}");
        }
    }

    /// Enzo: "there should be an easy button to speak to it where it's clear as day when it listens or not".
    #[test]
    fn the_talking_page_has_one_button_says_where_it_stands_and_scrolls_its_conversation() {
        let items = sheet_items(Sheet::Talk, &facts());
        assert_eq!(items.iter().filter(|i| i.action == Action::TalkToggle).count(), 1);
        assert!(!items.iter().any(|i| i.action == Action::NewConversation), "nothing to start over yet");
        let listening = Facts { talk: TalkNow::Listening(0.5), ..facts() };
        assert_eq!(caption(Action::TalkToggle, &listening).unwrap().0, "Listening");
        assert_ne!(caption(Action::TalkToggle, &facts()).unwrap().0, "Listening");
        let talked = Facts { talk_lines: (0..40).map(|i| (i % 2 == 0, "A sentence of a fair length said here. ".repeat(3))).collect(), ..facts() };
        assert!(sheet_items(Sheet::Talk, &talked).iter().any(|i| i.action == Action::NewConversation));
        assert!(max_scroll(Sheet::Talk, &talked) > 1000.0);
        assert_eq!(max_scroll(Sheet::Talk, &Facts { talk_lines: vec![(true, "Hello".into())], ..facts() }), 0.0);
    }

    /// When N4N0 speaks it shows, wherever the person is in these menus.
    /// In a headset, where the one thing to do about the lesser Fast walk is seen: on its own tile.
    #[test]
    fn fast_walk_s_tile_says_what_would_make_it_better() {
        let on = Facts { fast_walk: true, ..facts() };
        let said = |f: &Facts| caption(Action::FastWalk, f).unwrap().1;
        assert!(said(&on).contains("On: you walk faster"));
        let lesser = Facts { fast_walk_lesser: Some("Restart SteamVR once for the better Fast walk."), ..on.clone() };
        assert_eq!(said(&lesser), "Restart SteamVR once for the better Fast walk.");
        assert!(said(&Facts { fast_walk: false, ..lesser }).contains("Walk faster"), "off: nothing to do about it");
    }

    #[test]
    fn what_n4n0_says_shows_in_the_caption_and_a_sheet_s_head() {
        let facts = Facts { n4n0_says: Some("It is sunny".into()), ..facts() };
        assert_eq!(said_now(&facts), Some(("N4N0".to_string(), "It is sunny".to_string())));
        assert_eq!(status_now(&facts).unwrap().1, "It is sunny");
        // What stopped a picture goes before it.
        let stopped = Facts { note: Some(("a".into(), "b".into())), ..facts };
        assert_eq!(said_now(&stopped).unwrap().0, "a");
    }

    /// Enzo: "be able to grab the scrolling bar too".
    #[test]
    fn a_list_that_scrolls_has_a_bar_to_grab_and_it_reaches_both_ends() {
        let facts = Facts { recent: (0..120).map(|i| Recent { dir: PathBuf::from(format!("s{i}")), title: String::new(), state: String::new(), progress: None, picture: None, ..Default::default() }).collect(), sheet: Some(Sheet::Pictures), desktop: true, ..facts() };
        let bar = sheet_items(Sheet::Pictures, &facts).into_iter().find(|i| i.action == Action::ScrollBar).expect("a bar");
        assert!(bar.rect.right() <= 1024.0 && bar.rect.left() >= 980.0, "mostly off the tiles");
        assert_eq!(scroll_from_bar(Sheet::Pictures, &facts, 0.0), 0.0);
        assert_eq!(scroll_from_bar(Sheet::Pictures, &facts, 576.0), max_scroll(Sheet::Pictures, &facts));
        // A few captures have none.
        let few = Facts { recent: facts.recent[..2].iter().cloned().collect(), ..facts.clone() };
        assert!(!sheet_items(Sheet::Pictures, &few).iter().any(|i| i.action == Action::ScrollBar));
    }

    /// Enzo: "I don't see a copy link button anymore".
    #[test]
    fn a_youtube_result_says_it_copies_its_link_and_shows_when_it_did() {
        let video = VideoTile { title: "V".into(), channel: "C".into(), duration: None, picture: None };
        let facts = Facts { videos: vec![video], desktop: true, ..facts() };
        let items = sheet_items(Sheet::Results, &facts);
        let copy = items.iter().find(|i| matches!(i.look, Look::VideoCopy(0))).expect("a button");
        assert_eq!(copy.action, Action::Video(0));
        assert_eq!(items[hit(&items, copy.rect.center()).unwrap()].action, Action::Video(0));
        let drop = items.iter().find(|i| i.action == Action::VideoDrop(0)).unwrap();
        assert!(!copy.rect.intersects(drop.rect));
    }

    /// While a capture shows there is a way to hide it from any sheet.
    #[test]
    fn a_sheet_has_hide_3d_while_a_capture_shows() {
        let size = Zone::SheetBack.units().size();
        assert!(!items(Zone::SheetBack, size, &facts()).iter().any(|i| i.action == Action::HideView));
        assert!(items(Zone::SheetBack, size, &Facts { view_showing: true, ..facts() }).iter().any(|i| i.action == Action::HideView));
    }

    #[test]
    fn a_line_bent_on_the_ring_reads_left_to_right_and_stays_centred_on_its_cell() {
        for middle in [0.0, 60.0, 300.0, 120.0, 180.0, 240.0] {
            let places = arc_places(&[10.0, 30.0, 50.0], 60.0, 160.0, middle);
            // Centred on the cell, and well inside its 60 degrees.
            assert!((places[1].0 - middle).abs() < 0.001, "{middle}");
            assert!(places.iter().all(|(d, _)| (d - middle).abs() < 30.0));
            // Read in order, each letter across the way the last was: x grows along the line.
            let at = |d: f32| zones::polar(160.0, d).x;
            assert!(at(places[0].0) < at(places[2].0), "{middle}: right to left");
            // Never more than a quarter turn from upright, so never upside down.
            for (_, turn) in &places {
                let upright = turn.to_degrees().rem_euclid(360.0);
                assert!(upright <= 95.0 || upright >= 265.0, "{middle}: turned {upright}");
            }
        }
    }

    #[test]
    fn each_3d_picture_button_says_what_it_is_for_someone_new() {
        let (title, line) = caption(Action::Picture(PhotoMode::People), &facts()).unwrap();
        assert!(title.contains("3D picture") && title.contains("people"), "{title}");
        assert!(line.contains("3D"), "{line}");
        let (title, _) = caption(Action::Picture(PhotoMode::World), &facts()).unwrap();
        assert!(title.contains("3D picture") && title.contains("world"), "{title}");
        let coming = Facts { countdown: Some(2), picture_mode: Some(PhotoMode::World), ..facts() };
        assert_eq!(caption(Action::Picture(PhotoMode::World), &coming).unwrap().0, "3D picture in 2");
    }

    #[test]
    fn everything_that_does_something_says_what_in_a_caption() {
        let all = Facts { can_call: true, can_states: true, can_light: true, music: Some(("A".into(), true)), ..facts() };
        let mut list: Vec<Item> = Zone::WHEEL.iter().chain(&Zone::QUICK_MENU).flat_map(|z| items(*z, z.units().size(), &all)).collect();
        list.extend(sheet_items(Sheet::All, &all));
        list.extend(sheet_items(Sheet::Pictures, &all));
        for item in list {
            if item.action != Action::Idle {
                assert!(caption(item.action, &all).is_some(), "{:?}", item.action);
            }
        }
    }

    #[test]
    fn the_close_button_is_hit_before_what_it_sits_on() {
        let size = Zone::WingName.units().size();
        let vr = Facts { vr_close: true, ..facts() };
        let list = items(Zone::WingName, size, &vr);
        let close = list.iter().find(|i| i.action == Action::Hide).expect("in VR");
        assert_eq!(list[hit(&list, close.rect.center()).unwrap()].action, Action::Hide);
        assert!(items(Zone::WingName, size, &facts()).iter().all(|i| i.action != Action::Hide), "the desktop's menu closes by itself");
        assert_eq!(hit(&list, pos2(-5.0, -5.0)), None);
    }

    #[test]
    fn the_mask_cuts_a_cell_s_shape_out_of_its_rectangle() {
        let ring = Zone::WheelAll.ring().unwrap();
        let size = ring.bounds().size();
        let px = [size.x.round() as u32, size.y.round() as u32];
        let mask = cell_mask(ring, px, 1.0);
        let at = |x: u32, y: u32| mask[(y * px[0] + x) as usize];
        let middle = ring.anchor() - ring.bounds().min.to_vec2();
        assert_eq!(at(middle.x as u32, middle.y as u32), 255);
        assert!([at(0, 0), at(px[0] - 1, 0), at(0, px[1] - 1), at(px[0] - 1, px[1] - 1)].contains(&0));
        let mut rgba = vec![200u8; mask.len() * 4];
        apply_mask(&mut rgba, &mask, px, [0, 0, px[0], px[1]]);
        let i = (middle.y as u32 * px[0] + middle.x as u32) as usize * 4;
        assert_eq!(rgba[i..i + 4], [200, 200, 200, 200]);
        assert!(rgba.chunks_exact(4).any(|c| c == [0, 0, 0, 0]));
    }
}
