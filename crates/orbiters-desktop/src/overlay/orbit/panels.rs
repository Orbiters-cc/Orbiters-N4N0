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
use orbiters_n4n0_core::menu::{PhotoMode, Target};
use orbiters_ui::theme::{self, weight};
use orbiters_ui::{icons, widgets};
use std::path::PathBuf;

/// Orbiters' colours, at a cool dark temperature that sits beside VRChat's teal without copying it.
pub mod look {
    use egui::Color32;
    /// A panel's fill: Orbiters' ink, a little cool.
    pub const PANEL: Color32 = Color32::from_rgba_premultiplied(14, 18, 21, 248);
    pub const OPAQUE: Color32 = Color32::from_rgb(14, 18, 21);
    /// A band at a panel's head.
    pub const HEADER: Color32 = Color32::from_rgb(20, 25, 29);
    /// A tile.
    pub const TILE: Color32 = Color32::from_rgb(29, 35, 40);
    pub const TILE_HOVER: Color32 = Color32::from_rgb(38, 50, 54);
    pub const TILE_DOWN: Color32 = Color32::from_rgb(50, 66, 70);
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
}

/// A result of a YouTube search, as its list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoTile {
    pub title: String,
    pub channel: String,
    pub duration: Option<String>,
    pub picture: Option<TextureId>,
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
#[derive(Clone, Debug, PartialEq)]
pub struct WorldVisit {
    pub world_id: String,
    pub name: String,
    /// The last time: who may join, where, when.
    pub line: String,
    /// VRChat's location of the last time.
    pub location: String,
    /// The times before: location, and how long ago.
    pub earlier: Vec<(String, String)>,
    pub picture: Option<TextureId>,
}

/// What the panels show of a recent capture.
#[derive(Clone, Debug, PartialEq)]
pub struct Recent {
    pub dir: PathBuf,
    pub title: String,
    /// "3D picture, 2 h ago".
    pub state: String,
    /// Reconstructed so far, while it is.
    pub progress: Option<f32>,
    pub picture: Option<TextureId>,
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
    pub recent: Vec<Recent>,
    /// What is being reconstructed: its share done (0 to 1), and how many wait after it.
    pub reconstructing: Option<f32>,
    pub waiting: usize,
    /// VRChat in desktop mode: no fast walk, no tracking, and a real keyboard.
    pub desktop: bool,
    pub fast_walk: bool,
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
    /// The computer's volume, and whether it is muted; where what plays is and how long it lasts.
    pub volume: Option<(u8, bool)>,
    pub music_times: Option<(u32, u32)>,
    /// What the world's video player has.
    pub world_video: Option<String>,
    /// YouTube's results, and what was searched.
    pub videos: Vec<VideoTile>,
    pub videos_for: String,
    /// Talking with N4N0: where it stands, the conversation (whether the person said it, what was said), what N4N0 says right now, and how to call it by name when that is on.
    pub talk: TalkNow,
    pub talk_lines: Vec<(bool, String)>,
    pub n4n0_says: Option<String>,
    pub wake_word: Option<String>,
    /// The ring's cells can be picked by the number keys: on the desktop's wheel.
    pub digits: bool,
    /// The wing or its strip lies beside the sheet: it has the name and what plays already.
    pub beside_wing: bool,
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
    /// A cell of the ring: its icon over its label.
    Cell { icon: &'static str, label: &'static str, badge: bool },
    /// A small heading over a group of a sheet's tiles.
    Heading(&'static str),
    /// A YouTube result.
    Video(usize),
    /// A small button on a YouTube result.
    VideoDrop,
    /// The back button of YouTube's results.
    Back,
    /// The one button that talks to N4N0.
    TalkButton,
    /// Starting the conversation over.
    NewTalk,
    /// A world the person was in: its picture, name and last time.
    Visit(usize),
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
        Zone::WheelWorlds => cell(Action::Worlds, icons::GLOBE_HEMISPHERE_WEST, "Worlds in 3D", true),
        Zone::WheelYouTube => cell(Action::Page(Target::YouTube), icons::YOUTUBE_LOGO, "YouTube", false),
        Zone::WheelPlaying => vec![Item { action: Action::PlayPause, rect: whole, look: Look::Playing }],
        Zone::WheelAll => cell(Action::All, icons::SQUARES_FOUR, "All tools", false),
        Zone::WheelInstances => cell(Action::Page(Target::Instances), icons::CLOCK, "Instances", false),
        Zone::WheelPictures => cell(Action::Pictures, icons::CUBE, "3D pictures", false),
        Zone::BloomPeople => vec![Item { action: Action::Picture(PhotoMode::People), rect: whole, look: Look::Picture(PhotoMode::People) }],
        Zone::BloomWorld => vec![Item { action: Action::Picture(PhotoMode::World), rect: whole, look: Look::Picture(PhotoMode::World) }],
        Zone::BloomRecord => vec![Item { action: Action::Record, rect: whole, look: Look::Record }],
        Zone::BloomPrevious => vec![Item { action: Action::Previous, rect: whole, look: Look::Transport(icons::SKIP_BACK) }],
        Zone::BloomNext => vec![Item { action: Action::Next, rect: whole, look: Look::Transport(icons::SKIP_FORWARD) }],
        Zone::WheelCaption | Zone::QmCaption => vec![Item { action: Action::Idle, rect: whole, look: Look::Caption }],
        Zone::WheelVeil => vec![Item { action: Action::Idle, rect: whole, look: Look::Blank }],
        Zone::WheelHide => vec![Item { action: Action::Hide, rect: whole, look: Look::Close }],
        // The wing's tiles, each a part of its own.
        Zone::WingName => {
            // One close at a time: the sheet has its own.
            if facts.vr_close && facts.sheet.is_none() {
                vec![Item { action: Action::Hide, rect: r(w - 28.0, 2.0, 26.0, 26.0), look: Look::Close }]
            } else {
                Vec::new()
            }
        }
        Zone::WingWorlds => tile(Action::Worlds, icons::GLOBE_HEMISPHERE_WEST, "Worlds in 3D", true),
        Zone::WingYouTube => tile(Action::Page(Target::YouTube), icons::YOUTUBE_LOGO, "YouTube", false),
        Zone::WingInstances => tile(Action::Page(Target::Instances), icons::CLOCK, "Instances", false),
        Zone::WingPictures => tile(Action::Pictures, icons::CUBE, "3D pictures", false),
        Zone::WingTalk => vec![Item { action: Action::Talk, rect: whole, look: Look::Talk }],
        Zone::WingAll => tile(Action::All, icons::SQUARES_FOUR, "All tools", false),
        Zone::WingPlaying => vec![Item { action: Action::PlayPause, rect: whole, look: Look::Playing }],
        Zone::QmStrip => strip(w, h),
        Zone::SheetBack => {
            let mut out = vec![Item { action: Action::CloseSheet, rect: r(w - 40.0, 11.0, 30.0, 30.0), look: Look::Close }];
            // All tools has what plays on its page.
            if facts.music.is_some() && !facts.beside_wing && status_now(facts).is_none() && facts.sheet != Some(Sheet::All) {
                // Alone, from the wheel: what plays, in its head.
                let (y, b) = (13.0, 26.0);
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

/// The wing's tiles in a row: its mark, then the six.
fn strip(w: f32, h: f32) -> Vec<Item> {
    const PAD: f32 = 5.0;
    const MARK: f32 = 34.0;
    let width = (w - MARK - PAD * 2.0 - GAP * 5.0) / 6.0;
    let at = |c: f32| r(MARK + PAD + c * (width + GAP), PAD, width, h - PAD * 2.0);
    let tile = |action, icon, label, badge, rect| Item { action, rect, look: Look::Tile { icon, label, badge } };
    vec![
        tile(Action::Worlds, icons::GLOBE_HEMISPHERE_WEST, "Worlds in 3D", true, at(0.0)),
        tile(Action::Page(Target::YouTube), icons::YOUTUBE_LOGO, "YouTube", false, at(1.0)),
        tile(Action::Page(Target::Instances), icons::CLOCK, "Instances", false, at(2.0)),
        tile(Action::Pictures, icons::CUBE, "3D pictures", false, at(3.0)),
        Item { action: Action::Talk, rect: at(4.0), look: Look::Talk },
        tile(Action::All, icons::SQUARES_FOUR, "All tools", false, at(5.0)),
    ]
}

/// What the sheet's page holds, in N4N0's page's units (1024 x 576).
pub fn sheet_items(sheet: Sheet, facts: &Facts) -> Vec<Item> {
    match sheet {
        Sheet::All => {
            let (x0, y0, gap) = (24.0, 10.0, 12.0);
            let width = 1024.0 - x0 * 2.0;
            // The accessories, small, and only what can work here: a line of icons and names.
            let mut small: Vec<(Action, &'static str, &'static str)> = vec![(Action::Talk, icons::MICROPHONE, "Ask N4N0")];
            if facts.can_states {
                small.push((Action::Page(Target::States), icons::BOOKMARKS_SIMPLE, "Saved looks"));
            }
            if facts.can_light {
                small.push((Action::Page(Target::Light), icons::LIGHTBULB, "Light"));
            }
            if !facts.desktop {
                small.push((Action::FastWalk, icons::PERSON_SIMPLE_RUN, "Fast walk"));
                small.push((Action::Page(Target::Tracking), icons::VIRTUAL_REALITY, "Smooth tracking"));
            }
            if facts.can_call {
                small.push((Action::Page(Target::Call), icons::PHONE, "Call"));
            }
            small.push((Action::Page(Target::WorldFeedback), icons::HEART, "World feedback"));
            small.push((Action::Page(Target::Qr), icons::QR_CODE, "QR code"));
            small.push((Action::Page(Target::Suggest), icons::CHAT_TEARDROP_TEXT, "Suggest"));
            small.push((Action::OpenApp, icons::N4N0_MARK, "Open Orbiters"));
            const PER_ROW: usize = 5;
            let (small_h, small_gap) = (56.0, 8.0);
            let small_rows = small.len().div_ceil(PER_ROW) as f32;
            let block = small_rows * small_h + (small_rows - 1.0) * small_gap;
            let free = 576.0 - y0 * 2.0 - gap * 2.0 - block;
            let (first_h, second_h) = (free * 0.56, free * 0.44);
            // Worlds in 3D largest, then what people come for; what plays with all its controls, there, with no page of its own.
            let first_w = (width - gap * 2.0) * 0.42;
            let rest_w = (width - gap * 2.0 - first_w) / 2.0;
            let hero = |action, rect, icon, label, line, badge| Item { action, rect, look: Look::Hero { icon, label, line, badge, tint: tool_tint(action) } };
            let row2 = y0 + first_h + gap;
            let mut out = vec![
                hero(Action::Worlds, r(x0, y0, first_w, first_h), icons::GLOBE_HEMISPHERE_WEST, "Worlds in 3D", "Search any world, and walk in the 3D captures of those that have one", true),
                hero(Action::Page(Target::YouTube), r(x0 + first_w + gap, y0, rest_w, first_h), icons::YOUTUBE_LOGO, "YouTube", "Find a video for the world's player", false),
                hero(Action::Page(Target::Instances), r(x0 + first_w + gap * 2.0 + rest_w, y0, rest_w, first_h), icons::CLOCK, "Previous instances", "Where you were lately: go back", false),
            ];
            let playing = r(x0, row2, first_w + gap + rest_w, second_h);
            out.push(Item { action: Action::Idle, rect: playing, look: Look::NowPlaying });
            out.extend(now_playing_controls(playing, facts));
            out.push(hero(Action::Pictures, r(playing.right() + gap, row2, rest_w, second_h), icons::CUBE, "3D pictures", "Take one, record, see your captures", false));
            let top = row2 + second_h + gap;
            for (i, row) in small.chunks(PER_ROW).enumerate() {
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
                out.push(Item { action: Action::VideoDrop(i), rect: r(rect.right() - 72.0, rect.bottom() - 40.0, 64.0, 32.0), look: Look::VideoDrop });
            }
            out.push(Item { action: Action::ResultsBack, rect: r(24.0, 8.0, 44.0, 44.0), look: Look::Back });
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
            let mut out = Vec::new();
            for (i, visit) in facts.worlds_visited.iter().enumerate() {
                let y = y0 + i as f32 * (height + gap) - facts.gallery_scroll;
                // Only what shows is laid on the page.
                if y + height < 0.0 || y > 576.0 {
                    continue;
                }
                out.push(Item { action: Action::Idle, rect: r(x0, y, 1024.0 - x0 * 2.0, height), look: Look::Visit(i) });
                // The last time first, the times before after it: each chip goes to that instance.
                let mut right = 1024.0 - x0 - 12.0;
                let width = 124.0;
                out.push(Item { action: Action::Invite(i, 0), rect: r(right - width, y + 24.0, width, 42.0), look: Look::Chip { group: i, visit: 0 } });
                right -= width + 8.0;
                for (n, _) in visit.earlier.iter().enumerate().take(3) {
                    let width = 98.0;
                    out.push(Item { action: Action::Invite(i, n + 1), rect: r(right - width, y + 24.0, width, 42.0), look: Look::Chip { group: i, visit: n + 1 } });
                    right -= width + 6.0;
                }
            }
            out
        }
        Sheet::Pictures => {
            let (x0, gap, y0, h) = (24.0, 12.0, 10.0, GALLERY_TOP - 22.0);
            let width = (1024.0 - x0 * 2.0 - gap * 2.0) / 3.0;
            let mut out = Vec::new();
            // Under the buttons, and so under them for the pointer too.
            let (rects, _) = gallery_rects(facts);
            for (i, rect) in rects.iter().enumerate() {
                let rect = rect.translate(vec2(x0, GALLERY_TOP - facts.gallery_scroll));
                if rect.bottom() > GALLERY_TOP && rect.top() < 576.0 {
                    out.push(Item { action: Action::Page(Target::History), rect, look: Look::Capture(i) });
                }
            }
            out.push(Item { action: Action::Picture(PhotoMode::People), rect: r(x0, y0, width, h), look: Look::Picture(PhotoMode::People) });
            out.push(Item { action: Action::Picture(PhotoMode::World), rect: r(x0 + width + gap, y0, width, h), look: Look::Picture(PhotoMode::World) });
            out.push(Item { action: Action::Record, rect: r(x0 + (width + gap) * 2.0, y0, width, h), look: Look::Record });
            out
        }
        Sheet::Menu | Sheet::Worlds => Vec::new(),
    }
}

/// Where the gallery starts on its page, under its three buttons.
pub const GALLERY_TOP: f32 = 106.0;

/// The gallery as the app's own Photos & captures lays it out, in the layout chosen there: each capture's place from the gallery's top, and its height. All 16:9 as the thumbnails are, so
/// that nothing moves as pictures arrive.
fn gallery_rects(facts: &Facts) -> (Vec<Rect>, f32) {
    if facts.recent.is_empty() {
        return (Vec::new(), 0.0);
    }
    crate::pages::gallery_layout::arrange(facts.gallery_layout, &vec![16.0 / 9.0; facts.recent.len()], 1024.0 - 48.0, 300.0, 0.0)
}

/// Where YouTube's results start on their page, under the search they are of.
const RESULTS_TOP: f32 = 60.0;

/// YouTube's results' grid: columns, a tile's width and height, and where the first lies.
fn video_grid() -> (usize, f32, f32, f32) {
    let width = (1024.0 - 48.0 - 24.0) / 3.0;
    (3, width, width * 9.0 / 16.0 + 62.0, RESULTS_TOP + 4.0)
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
const VISIT_TOP: f32 = 10.0;
const VISIT_HEIGHT: f32 = 90.0;
const VISIT_GAP: f32 = 8.0;

/// Which rows of the Instances sheet show, scrolled this far, for what fetches their pictures.
pub fn visits_shown(scroll: f32) -> std::ops::Range<usize> {
    let first = ((scroll - VISIT_TOP) / (VISIT_HEIGHT + VISIT_GAP)).floor().max(0.0) as usize;
    first..first + (576.0 / (VISIT_HEIGHT + VISIT_GAP)).ceil() as usize + 1
}

/// How far a sheet's list scrolls.
pub fn max_scroll(sheet: Sheet, facts: &Facts) -> f32 {
    match sheet {
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

/// Whether a press on this is the start of a drag as much as a press: what lies in a list that scrolls. It only acts if let go without moving.
pub fn scrolls_with_a_drag(sheet: Sheet, look: &Look) -> bool {
    matches!(sheet, Sheet::Pictures | Sheet::Instances | Sheet::Results) && matches!(look, Look::Capture(_) | Look::Visit(_) | Look::Chip { .. } | Look::Video(_) | Look::VideoDrop)
}

/// What plays's controls, on its panel on All tools: nothing about it needs a page of its own.
fn now_playing_controls(panel: Rect, facts: &Facts) -> Vec<Item> {
    let mut out = Vec::new();
    let x = panel.left() + panel.height();
    let y = panel.bottom() - 16.0;
    if facts.music.is_some() {
        let glyph = if facts.music.as_ref().is_some_and(|m| m.1) { icons::PAUSE } else { icons::PLAY };
        out.push(Item { action: Action::Previous, rect: r(x, y - 46.0, 46.0, 46.0), look: Look::Transport(icons::SKIP_BACK) });
        out.push(Item { action: Action::PlayPause, rect: r(x + 54.0, y - 54.0, 54.0, 54.0), look: Look::Transport(glyph) });
        out.push(Item { action: Action::Next, rect: r(x + 116.0, y - 46.0, 46.0, 46.0), look: Look::Transport(icons::SKIP_FORWARD) });
    }
    if let Some((_, muted)) = facts.volume {
        let right = panel.right() - 16.0;
        out.push(Item { action: Action::Volume(true), rect: r(right - 40.0, y - 40.0, 40.0, 40.0), look: Look::Transport(icons::PLUS) });
        out.push(Item { action: Action::Volume(false), rect: r(right - 174.0, y - 40.0, 40.0, 40.0), look: Look::Transport(icons::MINUS) });
        out.push(Item { action: Action::Mute, rect: r(right - 222.0, y - 40.0, 40.0, 40.0), look: Look::Transport(if muted { icons::SPEAKER_X } else { icons::SPEAKER_HIGH }) });
    }
    out
}

/// Each tool's colour on All tools, from the app's own palette, so that the eye finds a tool by its colour and its icon before reading (Enzo: "very gray and hard to parse"). Those
/// next to each other differ.
fn tool_tint(action: Action) -> Color32 {
    match action {
        Action::Worlds | Action::Talk | Action::FastWalk => theme::ACCENT,
        Action::Page(Target::Instances) | Action::Page(Target::Call) | Action::Page(Target::Tracking) => theme::BLUE,
        Action::Page(Target::YouTube) | Action::Page(Target::WorldFeedback) => theme::DANGER,
        Action::Page(Target::Music) => theme::VIOLET,
        Action::Pictures => theme::CYAN,
        Action::Page(Target::Qr) => theme::INDIGO,
        Action::Page(Target::States) => theme::PURPLE,
        Action::Page(Target::Light) | Action::Page(Target::Suggest) => theme::WARN,
        _ => theme::MARK,
    }
}

/// What is on the item at `p` (panel units): its index. The small close button lies on top of what it sits on.
pub fn hit(items: &[Item], p: Pos2) -> Option<usize> {
    items.iter().enumerate().rev().find(|(_, i)| i.rect.contains(p)).map(|(i, _)| i)
}

/// What the caption says of what is pointed at, in words for someone who has never seen any of this: a title and what it does.
pub fn caption(action: Action, facts: &Facts) -> Option<(String, String)> {
    let (a, b): (String, String) = match action {
        Action::Worlds => ("Worlds in 3D".into(), "Search any world. Walk in those with a 3D capture.".into()),
        Action::Picture(_) if facts.no_kit => ("This avatar has no capture kit".into(), "Add N4N0 to it in Orbiters, My Avatar, to take 3D pictures with it.".into()),
        Action::Picture(_) if facts.picture_wait.is_some() => ("Press again to call it off".into(), "Or open VRChat's camera: it then starts by itself.".into()),
        Action::Picture(mode) => {
            if let Some(s) = facts.countdown {
                (format!("3D picture in {s}"), "VRChat's camera then flies around by itself.".into())
            } else if facts.picture_flying {
                ("3D picture in progress".into(), "The camera is flying. Stay where you are for a moment.".into())
            } else if mode == PhotoMode::People {
                ("3D picture of people".into(), "Keeps those around you in 3D: look around them afterwards.".into())
            } else {
                ("3D picture of this world".into(), "Keeps this place in 3D, without you in it: walk in it afterwards.".into())
            }
        }
        Action::Invite(group, visit) => {
            let place = facts.worlds_visited.get(group);
            let when = place.and_then(|v| if visit == 0 { None } else { v.earlier.get(visit - 1).map(|e| e.1.clone()) });
            ("Invite me".into(), when.map_or_else(|| "Orbiters invites you to the instance you were last in here.".to_string(), |w| format!("To the instance you were in {w}.")))
        }
        Action::Volume(up) => (if up { "Louder" } else { "Quieter" }.into(), "Your computer's volume.".into()),
        Action::Mute => ("Mute".into(), "Your computer's sound, on or off.".into()),
        Action::Pictures => ("3D pictures".into(), "Take a 3D picture, record a scene, see your captures.".into()),
        Action::All => ("All tools".into(), "Everything Orbiters can do from here.".into()),
        Action::Record if facts.recording => ("Stop recording".into(), "Orbiters then turns it into a 3D scene.".into()),
        Action::Record => ("Record a 3D scene".into(), "Walk around with VRChat's camera; Orbiters makes a 3D scene of what it saw.".into()),
        Action::Page(Target::Instances) => ("Previous instances".into(), "Where you were lately. Go back with an invite.".into()),
        Action::Page(Target::History) => ("Your captures".into(), "Your 3D pictures and scenes: see them, share them.".into()),
        Action::Page(Target::YouTube) => ("YouTube".into(), "Find a video, and put its link in the world's video player.".into()),
        Action::Page(Target::Call) => ("Call".into(), "Talk with a friend, wherever they are in VRChat.".into()),
        Action::Page(Target::Music) => ("Now playing".into(), "What your computer plays, and its volume.".into()),
        Action::Page(Target::Light) => ("Light".into(), "N4N0 as a light that follows you.".into()),
        Action::Page(Target::States) => ("Saved looks".into(), "Your avatar's saved looks, applied in one go.".into()),
        Action::Page(Target::Tracking) => ("Smooth tracking".into(), "Steadier headset, controllers and trackers.".into()),
        Action::Page(Target::WorldFeedback) => ("World feedback".into(), "Like this world, label it, comment on it.".into()),
        Action::Page(Target::Qr) => ("QR code".into(), "Open a link shown in the world, through VRChat's camera.".into()),
        Action::Page(Target::Suggest) => ("Suggest".into(), "Tell the Orbiters team what you would like.".into()),
        Action::Page(_) => return None,
        Action::FastWalk => ("Fast walk".into(), if facts.fast_walk { "On: you walk faster. Press to turn it off.".into() } else { "Walk faster than VRChat lets you.".to_string() }),
        Action::PlayPause => match &facts.music {
            Some((title, playing)) => (title.clone(), if *playing { "Playing on your computer. Press to pause.".into() } else { "Paused on your computer. Press to play.".to_string() }),
            None => ("Nothing playing".into(), "What your computer plays shows here.".into()),
        },
        Action::Previous => ("Previous".into(), "The previous track on your computer.".into()),
        Action::Next => ("Next".into(), "The next track on your computer.".into()),
        Action::TalkToggle => match facts.talk {
            TalkNow::Listening(_) => ("Listening".into(), "Press when you are done.".into()),
            _ => ("Talk to N4N0".into(), "Press, speak, and press again.".into()),
        },
        Action::NewConversation => ("New conversation".into(), "N4N0 starts over.".into()),
        Action::Video(i) => (facts.videos.get(i).map_or(String::new(), |v| v.title.clone()), "Press to copy its link, for the world's video player.".into()),
        Action::VideoDrop(_) => ("Drop it in the world".into(), "A card with its link, that whoever grabs it can use.".into()),
        Action::ResultsBack => ("Back".into(), "To the search.".into()),
        Action::Talk if facts.n4n0 => ("Ask N4N0".into(), "Your assistant in VRChat: talk to it, and see all it said.".into()),
        Action::Talk => ("Ask N4N0".into(), "Turn N4N0 on in Orbiters first.".into()),
        Action::OpenApp => ("Open Orbiters".into(), "The full app, on your computer.".into()),
        Action::CloseSheet => ("Close".into(), "Back to VRChat's menu.".into()),
        Action::Hide => ("Hide".into(), "Hides these until VRChat's menu opens again.".into()),
        Action::Idle => return None,
    };
    Some((a, b))
}

fn text(p: &Painter, at: Pos2, align: Align2, s: &str, size: f32, w: f32, c: Color32) {
    p.text(at, align, s, theme::font(size, w), c);
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

/// `s` on at most two lines inside `width`, the second cut: a title that does not fit one line.
fn two_lines(p: &Painter, s: &str, font: &FontId, width: f32) -> (String, Option<String>) {
    let wide = |t: &str| p.layout_no_wrap(t.to_string(), font.clone(), Color32::WHITE).size().x;
    if wide(s) <= width {
        return (s.to_string(), None);
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut first = String::new();
    let mut i = 0;
    while i < words.len() {
        let next = if first.is_empty() { words[i].to_string() } else { format!("{first} {}", words[i]) };
        if wide(&next) > width {
            break;
        }
        first = next;
        i += 1;
    }
    if first.is_empty() {
        return (fit(p, s, font, width), None);
    }
    let rest = words[i..].join(" ");
    (first, (!rest.is_empty()).then(|| fit(p, &rest, font, width)))
}

/// A label in VRChat's manner: bold, with a soft dark edge, readable on anything behind it.
fn bold(p: &Painter, at: Pos2, align: Align2, s: &str, size: f32, c: Color32) {
    p.text(at + vec2(0.0, 1.0), align, s, theme::font(size, weight::BOLD), Color32::from_black_alpha(140));
    p.text(at, align, s, theme::font(size, weight::BOLD), c);
}

/// A little "3D" on an icon's corner.
fn badge(p: &Painter, icon_at: Pos2, icon_size: f32, fill: Color32, ink: Color32) {
    let b = Rect::from_center_size(icon_at + vec2(icon_size * 0.58, -icon_size * 0.36), vec2(20.0, 13.0));
    p.rect_filled(b, theme::radius(4.0), fill);
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
    let toward = |c: Color32| theme::mix(c, look::ACCENT, lift * 0.42);
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
    if !on {
        return;
    }
    let steps = 24;
    let line: Vec<Pos2> = (0..=steps).map(|i| center + zones::polar(ring.outer - 2.0, ring.from + (ring.to - ring.from) * i as f32 / steps as f32)).collect();
    p.add(Shape::line(line, Stroke::new(3.0, look::ACCENT)));
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

/// A line round a cell, a little inside its edge: a picture filling it keeps the cell's shape against any world.
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
            // A dark veil over VRChat's wheel, and whose turn it is at its hub.
            let c = panel.center();
            p.circle_filled(c, size.x / 2.0, Color32::from_rgba_premultiplied(6, 8, 10, 224));
            widgets::icon(p, c - vec2(0.0, 6.0), icons::N4N0_MARK, 30.0, look::LABEL);
            text(p, c + vec2(0.0, 18.0), Align2::CENTER_CENTER, "Pointing here", 10.0, weight::SEMIBOLD, look::LABEL);
            return;
        }
        Zone::WingName => {
            // The name on a pill of its own, with what runs when it does.
            p.rect_filled(panel, theme::radius(size.y / 2.0), look::PANEL);
            widgets::icon(p, pos2(19.0, size.y / 2.0), icons::N4N0_MARK, 16.0, look::LABEL);
            text(p, pos2(32.0, size.y / 2.0), Align2::LEFT_CENTER, "Orbiters", 13.0, weight::SEMIBOLD, look::LABEL);
            let right = size.x - if facts.vr_close && facts.sheet.is_none() { 34.0 } else { 10.0 };
            if facts.recording {
                text(p, pos2(right, size.y / 2.0), Align2::RIGHT_CENTER, &clock(facts.recording_seconds.unwrap_or(0)), 11.0, weight::SEMIBOLD, theme::REC);
            } else if let Some(done) = facts.reconstructing {
                text(p, pos2(right, size.y / 2.0), Align2::RIGHT_CENTER, &format!("3D {:.0}%", done * 100.0), 11.0, weight::SEMIBOLD, look::ACCENT);
            }
        }
        Zone::QmStrip => {
            p.rect_filled(panel, theme::radius(look::RADIUS), look::PANEL);
            top_light(p, panel);
            widgets::icon(p, pos2(20.0, size.y / 2.0), icons::N4N0_MARK, 20.0, look::LABEL);
        }
        Zone::SheetBack => {
            p.rect_filled(panel, theme::radius(look::RADIUS + 2.0), look::OPAQUE);
            top_light(p, panel);
            // Beside the wing, which has the name, the head only says the page.
            let mut x = 18.0;
            if !facts.beside_wing {
                widgets::icon(p, pos2(x + 10.0, 26.0), icons::N4N0_MARK, 20.0, look::LABEL);
                text(p, pos2(x + 28.0, 26.0), Align2::LEFT_CENTER, "Orbiters", 14.0, weight::SEMIBOLD, look::MUTED);
                x += 94.0;
            }
            text(p, pos2(x, 26.0), Align2::LEFT_CENTER, &facts.sheet_title, 18.0, weight::SEMIBOLD, look::LABEL);
            let title = p.layout_no_wrap(facts.sheet_title.clone(), theme::font(18.0, weight::SEMIBOLD), Color32::WHITE).size().x;
            let from = x + title + 18.0;
            // The head is where this sheet says what is going on: nothing lies over or under it.
            if let Some((first, second)) = status_now(facts) {
                let room = size.x - 50.0 - from;
                let big = theme::font(13.0, weight::BOLD);
                let small = theme::font(11.5, weight::MEDIUM);
                p.text(pos2(from, 18.0), Align2::LEFT_CENTER, fit(p, &first, &big, room), big, look::ACCENT);
                p.text(pos2(from, 35.0), Align2::LEFT_CENTER, fit(p, &second, &small, room), small, look::LABEL);
            } else if let Some((first, second)) = facts.pointed.filter(|_| matches!(facts.sheet, Some(Sheet::All | Sheet::Pictures))).and_then(|a| caption(a, facts)) {
                // What a small tile does, which it does not say itself.
                let room = size.x - 50.0 - from;
                let big = theme::font(13.0, weight::BOLD);
                let small = theme::font(11.5, weight::MEDIUM);
                p.text(pos2(from, 18.0), Align2::LEFT_CENTER, fit(p, &first, &big, room), big, look::LABEL);
                p.text(pos2(from, 35.0), Align2::LEFT_CENTER, fit(p, &second, &small, room), small, look::MUTED);
            } else if let (Some((name, playing)), false, false) = (&facts.music, facts.beside_wing, facts.sheet == Some(Sheet::All)) {
                let font = theme::font(12.0, weight::SEMIBOLD);
                let right = size.x - 56.0 - 26.0 * 3.0 - 8.0;
                let name = fit(p, name, &font, (right - from).max(0.0));
                p.text(pos2(right, 26.0), Align2::RIGHT_CENTER, name, font, if *playing { look::LABEL } else { look::MUTED });
            }
        }
        _ => {}
    }
    for (i, item) in items(zone, size, facts).iter().enumerate() {
        draw(p, zone, item, facts, hover == Some(i), pressed == Some(i));
    }
}

/// Draws a sheet's page (in N4N0's page's units): native to these menus, in VRChat's manner.
pub fn paint_sheet(p: &Painter, sheet: Sheet, facts: &Facts, hover: Option<usize>, pressed: Option<usize>) {
    let page = Rect::from_min_size(pos2(0.0, 0.0), vec2(1024.0, 576.0));
    p.rect_filled(page, 0.0, look::OPAQUE);
    if sheet == Sheet::Results {
        let font = theme::font(20.0, weight::SEMIBOLD);
        p.text(pos2(82.0, 30.0), Align2::LEFT_CENTER, fit(p, &facts.videos_for, &font, 880.0), font, look::LABEL);
    }
    if sheet == Sheet::Talk {
        draw_conversation(p, facts);
    }
    if sheet == Sheet::Instances && facts.worlds_visited.is_empty() {
        widgets::icon(p, page.center() - vec2(0.0, 30.0), icons::CLOCK, 54.0, look::MUTED);
        text(p, page.center() + vec2(0.0, 24.0), Align2::CENTER_CENTER, "Nowhere yet", 22.0, weight::BOLD, look::LABEL);
        text(p, page.center() + vec2(0.0, 54.0), Align2::CENTER_CENTER, "The worlds you visit show here, to go back to.", 16.0, weight::MEDIUM, look::MUTED);
    }
    if sheet == Sheet::Pictures && facts.recent.is_empty() {
        widgets::icon(p, pos2(512.0, 300.0), icons::CUBE, 54.0, look::MUTED);
        text(p, pos2(512.0, 354.0), Align2::CENTER_CENTER, "No captures yet", 22.0, weight::BOLD, look::LABEL);
        text(p, pos2(512.0, 384.0), Align2::CENTER_CENTER, "Your 3D pictures and recordings show here.", 16.0, weight::MEDIUM, look::MUTED);
    }
    for (i, item) in sheet_items(sheet, facts).iter().enumerate() {
        draw(p, Zone::Takeover, item, facts, hover == Some(i), pressed == Some(i));
    }
    // Where in the list this is.
    let max = max_scroll(sheet, facts);
    if max > 0.0 {
        {
            let top = match sheet {
                Sheet::Pictures => GALLERY_TOP,
                Sheet::Results => RESULTS_TOP,
                Sheet::Talk => TALK_TOP,
                _ => 8.0,
            };
            let track = 576.0 - top - 8.0;
            let length = (track * track / (track + max)).max(36.0);
            let y = top + (track - length) * (facts.gallery_scroll / max).clamp(0.0, 1.0);
            p.rect_filled(Rect::from_min_size(pos2(1024.0 - 13.0, y), vec2(5.0, length)), theme::radius(2.5), Color32::from_white_alpha(70));
        }
    }
}

/// The conversation with N4N0, newest at the foot, the person's lines to the right.
fn draw_conversation(p: &Painter, facts: &Facts) {
    text(p, pos2(TALK_LEFT + 2.0, 30.0), Align2::LEFT_CENTER, "Conversation", 20.0, weight::BOLD, look::LABEL);
    let area = Rect::from_min_max(pos2(TALK_LEFT, TALK_TOP), pos2(1024.0 - 20.0, 576.0));
    if facts.talk_lines.is_empty() {
        text(p, area.center() - vec2(0.0, 22.0), Align2::CENTER_CENTER, "Nothing said yet", 22.0, weight::BOLD, look::MUTED);
        text(p, area.center() + vec2(0.0, 12.0), Align2::CENTER_CENTER, "Press the button, and ask N4N0 anything.", 17.0, weight::MEDIUM, look::MUTED);
        return;
    }
    let clip = p.with_clip_rect(area);
    let mut y = TALK_TOP - facts.gallery_scroll;
    for (person, line) in &facts.talk_lines {
        let height = talk_line_height(line);
        if y + height >= TALK_TOP && y <= 576.0 {
            let wide = TALK_WIDTH * 0.86;
            let galley = clip.layout(line.clone(), theme::font(17.0, weight::MEDIUM), look::LABEL, wide - 28.0);
            let size = galley.size() + vec2(28.0, 20.0);
            let x = if *person { TALK_LEFT + TALK_WIDTH - size.x } else { TALK_LEFT };
            let bubble = Rect::from_min_size(pos2(x, y), size);
            clip.rect_filled(bubble, theme::radius(14.0), if *person { theme::mix(look::TILE, look::ACCENT, 0.22) } else { look::TILE });
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
    p.rect_filled(rect, theme::radius(look::TILE_RADIUS), fill);
    if hovered || selected {
        let width = if selected { 2.0 } else { 1.5 };
        p.rect_stroke(rect.shrink(width / 2.0), theme::radius(look::TILE_RADIUS), Stroke::new(width, look::ACCENT), egui::StrokeKind::Middle);
    }
}

/// A surface's top edge catches the light: a hairline that makes it read as an object on any world.
fn top_light(p: &Painter, panel: Rect) {
    let inset = look::RADIUS;
    p.line_segment([pos2(panel.left() + inset, panel.top() + 0.5), pos2(panel.right() - inset, panel.top() + 0.5)], Stroke::new(1.0, Color32::from_white_alpha(26)));
}

/// A small mark at a cell's outer edge: something more comes out of it.
fn bloom_mark(p: &Painter, ring: Ring, center: Pos2) {
    let middle = ring.middle();
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
    let font = theme::font(size, w);
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

/// The number key that picks a cell, in its corner.
fn digit(p: &Painter, ring: Ring, center: Pos2) {
    let at = center + zones::polar(ring.inner + 11.0, ring.from + (ring.to - ring.from) * 0.15);
    let n = match ring.middle().rem_euclid(360.0) as u32 {
        0 => "1",
        60 => "2",
        120 => "3",
        180 => "4",
        240 => "5",
        _ => "6",
    };
    text(p, at, Align2::CENTER_CENTER, n, 10.0, weight::BOLD, look::MUTED);
}

fn is_selected(item: &Item, facts: &Facts) -> bool {
    match (facts.sheet, item.action) {
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
            ring_fill(p, ring, center, if pressed { 0.65 } else if hovered { 0.38 } else { 0.0 });
            ring_rim(p, ring, center, hovered);
            // Its icon upright toward the wheel, its name bent along its outer edge.
            let size = 26.0;
            let at = center + zones::polar(ring.inner + 24.0, ring.middle());
            widgets::icon(p, at, icon, size, look::LABEL);
            if has_badge {
                badge(p, at, size, look::ACCENT, look::ON_ACCENT);
            }
            curved(p, center, ring.outer - 12.0, ring.middle(), label, 12.5, weight::BOLD, look::LABEL, true);
            if zone.blooms().is_some() {
                bloom_mark(p, ring, center);
            }
            if facts.digits {
                digit(p, ring, center);
            }
        }
        Look::Tile { icon, label, badge: has_badge } => {
            let selected = is_selected(item, facts);
            tile_back(p, rect, hovered, pressed, selected);
            let ink = if selected || hovered { look::ACCENT } else { look::LABEL };
            let size = (rect.height() * 0.42).clamp(18.0, 32.0);
            let top = rect.center().y - (size + 3.0 + 11.0) / 2.0;
            let at = pos2(rect.center().x, top + size / 2.0);
            widgets::icon(p, at, icon, size, ink);
            if has_badge {
                badge(p, at, size, look::ACCENT, look::ON_ACCENT);
            }
            let font_size = {
                let mut s = 11.0;
                while s > 8.5 && p.layout_no_wrap(label.to_string(), theme::font(s, weight::BOLD), Color32::WHITE).size().x > rect.width() - 6.0 {
                    s -= 0.5;
                }
                s
            };
            let words = theme::font(font_size, weight::SEMIBOLD);
            p.text(pos2(rect.center().x, top + size + 3.0 + 5.5), Align2::CENTER_CENTER, label, words, look::LABEL);
        }
        Look::Hero { icon, label, line, badge: has_badge, tint } => {
            let radius = theme::radius(look::TILE_RADIUS + 4.0);
            p.rect_filled(rect, radius, theme::mix(look::TILE, tint, if pressed { 0.30 } else if hovered { 0.24 } else { 0.16 }));
            if hovered {
                p.rect_stroke(rect.shrink(1.0), radius, Stroke::new(2.0, tint), egui::StrokeKind::Middle);
            }
            let chip = Rect::from_min_size(rect.min + vec2(18.0, 18.0), Vec2::splat(72.0));
            p.rect_filled(chip, theme::radius(18.0), theme::alpha(tint, 0.22));
            widgets::icon(p, chip.center(), icon, 42.0, tint);
            if has_badge {
                badge(p, chip.center(), 52.0, look::ACCENT, look::ON_ACCENT);
            }
            // What is true now says more than what it is for.
            let said = match (item.action, &facts.world_video) {
                (Action::Page(Target::YouTube), Some(title)) => format!("In this world's player: {title}"),
                _ => line.to_string(),
            };
            let room = rect.width() - 36.0;
            let small = theme::font(16.0, weight::MEDIUM);
            let (one, two) = two_lines(p, &said, &small, room);
            let lines = if two.is_some() { 2.0 } else { 1.0 };
            let mut y = rect.bottom() - 16.0 - lines * 21.0;
            let font = theme::font(27.0, weight::BOLD);
            p.text(pos2(rect.left() + 18.0, y - 20.0), Align2::LEFT_CENTER, fit(p, label, &font, room), font, look::LABEL);
            p.text(pos2(rect.left() + 18.0, y + 10.0), Align2::LEFT_CENTER, one, small.clone(), look::MUTED);
            if let Some(two) = two {
                y += 21.0;
                p.text(pos2(rect.left() + 18.0, y + 10.0), Align2::LEFT_CENTER, two, small, look::MUTED);
            }
        }
        Look::Small { icon, label, tint } => {
            let on = item.action == Action::FastWalk && facts.fast_walk;
            let radius = theme::radius(look::TILE_RADIUS);
            p.rect_filled(rect, radius, theme::mix(look::TILE, tint, if pressed { 0.26 } else if hovered { 0.20 } else { 0.10 }));
            if hovered || on {
                p.rect_stroke(rect.shrink(0.75), radius, Stroke::new(1.5, tint), egui::StrokeKind::Middle);
            }
            let at = pos2(rect.left() + 26.0, rect.center().y);
            widgets::icon(p, at, icon, 24.0, tint);
            let font = theme::font(16.0, weight::SEMIBOLD);
            p.text(pos2(rect.left() + 46.0, rect.center().y), Align2::LEFT_CENTER, fit(p, label, &font, rect.width() - 52.0), font, look::LABEL);
        }
        Look::NowPlaying => {
            let tint = theme::VIOLET;
            let radius = theme::radius(look::TILE_RADIUS + 4.0);
            p.rect_filled(rect, radius, theme::mix(look::TILE, tint, 0.14));
            let side = rect.height() - 24.0;
            let art = Rect::from_min_size(rect.min + vec2(12.0, 12.0), Vec2::splat(side));
            match (facts.art, &facts.music) {
                (Some(id), Some(_)) => {
                    p.add(RectShape::filled(art, theme::radius(12.0), Color32::WHITE).with_texture(id, cover_uv(art.size())));
                }
                _ => {
                    p.rect_filled(art, theme::radius(12.0), theme::alpha(tint, 0.22));
                    widgets::icon(p, art.center(), icons::MUSIC_NOTES, side * 0.4, tint);
                }
            }
            let x = rect.left() + rect.height();
            let room = rect.right() - x - 14.0;
            let font = theme::font(13.0, weight::SEMIBOLD);
            p.text(pos2(x, rect.top() + 24.0), Align2::LEFT_CENTER, "Now playing", font, tint);
            match &facts.music {
                Some((title, _)) => {
                    let font = theme::font(23.0, weight::BOLD);
                    p.text(pos2(x, rect.top() + 52.0), Align2::LEFT_CENTER, fit(p, title, &font, room), font, look::LABEL);
                    let font = theme::font(15.0, weight::MEDIUM);
                    p.text(pos2(x, rect.top() + 78.0), Align2::LEFT_CENTER, fit(p, &facts.music_by, &font, room), font, look::MUTED);
                }
                None => {
                    let font = theme::font(23.0, weight::BOLD);
                    p.text(pos2(x, rect.top() + 52.0), Align2::LEFT_CENTER, "Nothing yet", font, look::MUTED);
                    let font = theme::font(15.0, weight::MEDIUM);
                    p.text(pos2(x, rect.top() + 78.0), Align2::LEFT_CENTER, fit(p, "What your computer plays shows here, with its controls.", &font, room), font, look::MUTED);
                }
            }
            if let Some((at, of)) = facts.music_times.filter(|t| t.1 > 0) {
                let bar = Rect::from_min_size(pos2(x, rect.top() + 94.0), vec2(room, 4.0));
                p.rect_filled(bar, theme::radius(2.0), Color32::from_white_alpha(36));
                p.rect_filled(Rect::from_min_size(bar.min, vec2(room * (at as f32 / of as f32).clamp(0.0, 1.0), 4.0)), theme::radius(2.0), tint);
            }
            // The level between its minus and its plus.
            if let Some((percent, muted)) = facts.volume {
                let right = rect.right() - 16.0;
                let bar = Rect::from_min_size(pos2(right - 128.0, rect.bottom() - 16.0 - 22.0), vec2(82.0, 4.0));
                p.rect_filled(bar, theme::radius(2.0), Color32::from_white_alpha(36));
                let level = if muted { 0.0 } else { percent as f32 / 100.0 };
                p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * level, 4.0)), theme::radius(2.0), look::LABEL);
            }
        }
        Look::Picture(mode) => picture_button(p, zone, shape, rect, mode, facts, hovered, pressed),
        Look::Record => {
            let label = if facts.recording { "Stop" } else { "Record" };
            let dim = facts.capture_busy && !facts.recording;
            let time = facts.recording_seconds.filter(|_| facts.recording).map_or(label.to_string(), clock);
            let color = if dim { theme::FG_4 } else { look::LABEL };
            if let Some((ring, center)) = shape.ring {
                if facts.recording {
                    ring_mesh(p, ring, center, theme::mix(theme::REC, Color32::BLACK, 0.35), theme::mix(theme::REC, Color32::BLACK, 0.6));
                } else {
                    ring_fill(p, ring, center, if pressed { 0.65 } else if hovered { 0.38 } else { 0.0 });
                }
                ring_rim(p, ring, center, hovered);
                let at = center + zones::polar(ring.inner + 20.0, ring.middle());
                if facts.recording {
                    p.rect_filled(Rect::from_center_size(at, Vec2::splat(16.0)), theme::radius(3.0), color);
                } else {
                    p.circle_filled(at, 9.5, if dim { theme::FG_4 } else { theme::REC });
                }
                curved(p, center, ring.outer - 12.0, ring.middle(), &time, 12.0, weight::BOLD, color, true);
            } else {
                let tint = theme::REC;
                let radius = theme::radius(look::TILE_RADIUS + 2.0);
                p.rect_filled(rect, radius, theme::mix(look::TILE, tint, if pressed { 0.28 } else if hovered { 0.22 } else if dim { 0.04 } else { 0.14 }));
                if hovered {
                    p.rect_stroke(rect.shrink(1.0), radius, Stroke::new(1.5, tint), egui::StrokeKind::Middle);
                }
                let at = pos2(rect.left() + 44.0, rect.center().y);
                p.rect_filled(Rect::from_center_size(at, Vec2::splat(56.0)), theme::radius(14.0), theme::alpha(tint, 0.22));
                if facts.recording {
                    p.rect_filled(Rect::from_center_size(at, Vec2::splat(22.0)), theme::radius(4.0), tint);
                } else {
                    p.circle_filled(at, 13.0, if dim { theme::FG_4 } else { tint });
                }
                let x = rect.left() + 84.0;
                let room = rect.right() - x - 8.0;
                text(p, pos2(x, rect.center().y - 12.0), Align2::LEFT_CENTER, &time, 20.0, weight::BOLD, color);
                let small = theme::font(15.0, weight::MEDIUM);
                p.text(pos2(x, rect.center().y + 14.0), Align2::LEFT_CENTER, fit(p, "A 3D scene of where you walk", &small, room), small, look::MUTED);
            }
        }
        Look::Video(i) => {
            let Some(video) = facts.videos.get(i) else { return };
            let clip = p.with_clip_rect(Rect::from_min_max(pos2(0.0, RESULTS_TOP), pos2(1024.0, 576.0)));
            let radii = [theme::squircle(14.0); 4];
            let picture = Rect::from_min_size(rect.min, vec2(rect.width(), rect.width() * 9.0 / 16.0));
            match video.picture {
                Some(id) => orbiters_ui::paint::squircle_texture(&clip, picture, radii, id, cover_uv(picture.size()), if pressed { Color32::from_gray(200) } else { Color32::WHITE }),
                None => {
                    orbiters_ui::paint::fill_convex(&clip, &orbiters_ui::paint::squircle_points_corners(picture, radii), |_| theme::INK_750);
                    widgets::icon(&clip, picture.center(), icons::YOUTUBE_LOGO, 40.0, look::MUTED);
                }
            }
            if hovered {
                clip.rect_stroke(picture.shrink(1.0), theme::radius(14.0), Stroke::new(2.0, look::ACCENT), egui::StrokeKind::Middle);
            }
            if let Some(duration) = &video.duration {
                let font = theme::font(14.0, weight::SEMIBOLD);
                let wide = clip.layout_no_wrap(duration.clone(), font.clone(), Color32::WHITE).size().x;
                let chip = Rect::from_min_size(picture.right_bottom() - vec2(wide + 20.0, 32.0), vec2(wide + 12.0, 24.0));
                clip.rect_filled(chip, theme::radius(6.0), Color32::from_black_alpha(190));
                clip.text(chip.center(), Align2::CENTER_CENTER, duration, font, look::LABEL);
            }
            let room = rect.width() - 84.0;
            let font = theme::font(17.0, weight::BOLD);
            clip.text(pos2(rect.left() + 2.0, picture.bottom() + 18.0), Align2::LEFT_CENTER, fit(&clip, &video.title, &font, room), font, look::LABEL);
            let font = theme::font(14.0, weight::MEDIUM);
            clip.text(pos2(rect.left() + 2.0, picture.bottom() + 42.0), Align2::LEFT_CENTER, fit(&clip, &video.channel, &font, room), font, look::MUTED);
        }
        Look::VideoDrop => {
            let clip = p.with_clip_rect(Rect::from_min_max(pos2(0.0, RESULTS_TOP), pos2(1024.0, 576.0)));
            clip.rect_filled(rect, theme::radius(rect.height() / 2.0), if hovered { look::TILE_HOVER } else { look::TILE });
            clip.text(rect.center(), Align2::CENTER_CENTER, "Drop", theme::font(14.0, weight::SEMIBOLD), look::LABEL);
        }
        Look::Back => {
            p.circle_filled(rect.center(), rect.width() / 2.0, if hovered { look::TILE_HOVER } else { look::TILE });
            widgets::icon(p, rect.center(), icons::CARET_LEFT, 24.0, look::LABEL);
        }
        Look::NewTalk => {
            p.rect_filled(rect, theme::radius(rect.height() / 2.0), if hovered { look::TILE_HOVER } else { look::TILE });
            p.text(rect.center(), Align2::CENTER_CENTER, "New conversation", theme::font(15.0, weight::SEMIBOLD), look::LABEL);
        }
        Look::TalkButton => {
            // Clear as day: the one thing on this page that changes with whether N4N0 hears you.
            let center = pos2(rect.center().x, rect.top() + 170.0);
            let radius = 118.0;
            let (fill, ink, first, second): (Color32, Color32, &str, &str) = match facts.talk {
                TalkNow::Ready => (theme::mix(look::TILE, look::ACCENT, if pressed { 0.4 } else if hovered { 0.3 } else { 0.18 }), look::ACCENT, "Press to talk", "Press again when you are done"),
                TalkNow::Listening(_) => (look::ACCENT, look::ON_ACCENT, "Listening", "Press when you are done"),
                TalkNow::Thinking => (look::TILE, look::LABEL, "Thinking", "N4N0 answers on the right"),
                TalkNow::Off(why) => (look::TILE, theme::FG_4, "Not available", why),
            };
            if let TalkNow::Listening(level) = facts.talk {
                p.circle_filled(center, radius + 12.0 + level.clamp(0.0, 1.0) * 28.0, theme::alpha(look::ACCENT, 0.22));
            }
            p.circle_filled(center, radius, fill);
            if matches!(facts.talk, TalkNow::Ready) {
                p.circle_stroke(center, radius, Stroke::new(3.0, look::ACCENT));
            }
            widgets::icon(p, center, if matches!(facts.talk, TalkNow::Thinking) { icons::CHAT_TEARDROP_TEXT } else { icons::MICROPHONE }, 96.0, ink);
            let y = center.y + radius + 48.0;
            let color = if matches!(facts.talk, TalkNow::Listening(_)) { look::ACCENT } else { look::LABEL };
            text(p, pos2(rect.center().x, y), Align2::CENTER_CENTER, first, 30.0, weight::BOLD, color);
            let font = theme::font(16.0, weight::MEDIUM);
            p.text(pos2(rect.center().x, y + 34.0), Align2::CENTER_CENTER, fit(p, second, &font, rect.width() - 8.0), font, look::MUTED);
            if let Some(word) = &facts.wake_word {
                let font = theme::font(14.0, weight::MEDIUM);
                p.text(pos2(rect.center().x, y + 62.0), Align2::CENTER_CENTER, fit(p, &format!("You can also say \u{201c}{word}\u{201d}"), &font, rect.width() - 8.0), font, look::MUTED);
            }
        }
        Look::Heading(name) => {
            text(p, pos2(rect.left() + 2.0, rect.center().y), Align2::LEFT_CENTER, name, 16.0, weight::BOLD, look::LABEL);
        }
        Look::Visit(i) => {
            let Some(visit) = facts.worlds_visited.get(i) else { return };
            tile_back(p, rect, false, false, false);
            let picture = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2((rect.height() - 12.0) * 16.0 / 9.0, rect.height() - 12.0));
            match visit.picture {
                Some(id) => {
                    p.add(RectShape::filled(picture, theme::radius(5.0), Color32::WHITE).with_texture(id, cover_uv(picture.size())));
                }
                None => {
                    p.rect_filled(picture, theme::radius(5.0), look::HEADER);
                    widgets::icon(p, picture.center(), icons::GLOBE_HEMISPHERE_WEST, 30.0, look::MUTED);
                }
            }
            let x = picture.right() + 16.0;
            // The buttons lie on its right: the words stop short of them.
            let room = rect.right() - x - 156.0 - 104.0 * visit.earlier.len().min(3) as f32;
            let font = theme::font(20.0, weight::BOLD);
            p.text(pos2(x, rect.center().y - 14.0), Align2::LEFT_CENTER, fit(p, &visit.name, &font, room), font, look::LABEL);
            let font = theme::font(16.0, weight::MEDIUM);
            p.text(pos2(x, rect.center().y + 14.0), Align2::LEFT_CENTER, fit(p, &visit.line, &font, room), font, look::MUTED);
        }
        Look::Chip { group, visit } => {
            let primary = visit == 0;
            let words = if primary {
                "Invite me".to_string()
            } else {
                facts.worlds_visited.get(group).and_then(|v| v.earlier.get(visit - 1)).map_or(String::new(), |e| e.1.clone())
            };
            let radius = theme::radius(rect.height() / 2.0);
            p.rect_filled(rect, radius, if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::OPAQUE });
            let edge = if primary || hovered { look::ACCENT } else { theme::INK_600 };
            p.rect_stroke(rect, radius, Stroke::new(1.5, edge), egui::StrokeKind::Inside);
            let font = theme::font(if primary { 16.0 } else { 14.0 }, weight::SEMIBOLD);
            p.text(rect.center(), Align2::CENTER_CENTER, fit(p, &words, &font, rect.width() - 12.0), font, if primary { look::ACCENT } else { look::LABEL });
        }
        Look::Playing => {
            let none = facts.music.is_none();
            let (name, playing) = facts.music.clone().unwrap_or_default();
            let glyph = if playing { icons::PAUSE } else if none { icons::MUSIC_NOTES } else { icons::PLAY };
            if let Some((ring, center)) = shape.ring {
                ring_fill(p, ring, center, if pressed { 0.65 } else if hovered && !none { 0.38 } else { 0.0 });
                // Its picture as a hint behind it, no louder than the other cells: the name of the cell stays, the song under it, all of it in the caption.
                if let (Some(id), false) = (facts.art, none) {
                    p.add(RectShape::filled(rect, 0.0, Color32::from_rgb(70, 76, 78)).with_texture(id, cover_uv(rect.size())));
                    p.rect_filled(rect, 0.0, Color32::from_black_alpha(120));
                    cell_edge(p, ring, center, 3.0, look::CELL_FAR);
                }
                ring_rim(p, ring, center, hovered);
                let ink = if none { look::MUTED } else { look::LABEL };
                widgets::icon(p, center + zones::polar(ring.inner + 14.0, ring.middle()), glyph, 18.0, ink);
                curved(p, center, ring.outer - 11.0, ring.middle(), "Now playing", 12.5, weight::BOLD, ink, true);
                let font = theme::font(10.5, weight::MEDIUM);
                let under = if none { "Nothing yet".to_string() } else { fit(p, &name, &font, 112.0) };
                curved(p, center, ring.outer - 25.0, ring.middle(), &under, 10.5, weight::MEDIUM, look::LABEL, true);
                if !none {
                    bloom_mark(p, ring, center);
                }
                if facts.digits {
                    digit(p, ring, center);
                }
            } else {
                tile_back(p, rect, hovered && !none, pressed && !none, false);
                let side = rect.height() - 12.0;
                let art = Rect::from_min_size(rect.min + vec2(6.0, 6.0), Vec2::splat(side));
                if let (Some(id), false) = (facts.art, none) {
                    p.add(RectShape::filled(art, theme::radius(4.0), Color32::WHITE).with_texture(id, cover_uv(art.size())));
                } else {
                    p.rect_filled(art, theme::radius(4.0), look::HEADER);
                }
                widgets::icon(p, art.center(), glyph, side * 0.5, if none { look::MUTED } else { look::LABEL });
                let x = art.right() + 7.0;
                let room = rect.right() - x - 5.0;
                let font = theme::font(10.5, weight::BOLD);
                if none {
                    p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, fit(p, "Nothing playing", &font, room), font, look::MUTED);
                } else {
                    let (one, two) = two_lines(p, &name, &font, room);
                    p.text(pos2(x, rect.center().y - 7.0), Align2::LEFT_CENTER, one, font.clone(), look::LABEL);
                    if let Some(two) = two {
                        p.text(pos2(x, rect.center().y + 7.0), Align2::LEFT_CENTER, two, font, look::LABEL);
                    }
                }
            }
        }
        Look::Transport(glyph) => {
            if let Some((ring, center)) = shape.ring {
                ring_fill(p, ring, center, if pressed { 0.65 } else if hovered { 0.38 } else { 0.0 });
                ring_rim(p, ring, center, hovered);
                widgets::icon(p, shape.at, glyph, 20.0, look::LABEL);
            } else {
                p.circle_filled(rect.center(), rect.width() / 2.0, if hovered { look::TILE_HOVER } else { look::TILE });
                widgets::icon(p, rect.center(), glyph, rect.width() * 0.5, look::LABEL);
            }
        }
        Look::Talk => {
            let on = matches!(facts.talk, TalkNow::Listening(_));
            tile_back(p, rect, hovered, pressed, on);
            let color = if on {
                look::ACCENT
            } else if !facts.n4n0 {
                theme::FG_4
            } else if hovered {
                look::ACCENT
            } else {
                look::LABEL
            };
            let size = (rect.height() * 0.42).clamp(18.0, 32.0);
            let top = rect.center().y - (size + 3.0 + 11.0) / 2.0;
            widgets::icon(p, pos2(rect.center().x, top + size / 2.0), icons::MICROPHONE, size, color);
            p.text(pos2(rect.center().x, top + size + 3.0 + 5.5), Align2::CENTER_CENTER, if on { "Listening" } else { "Ask N4N0" }, theme::font(11.0, weight::SEMIBOLD), if on { look::ON_ACCENT } else if facts.n4n0 { look::LABEL } else { theme::FG_4 });
        }
        Look::Close => {
            p.circle_filled(rect.center(), rect.width() / 2.0, if hovered { look::TILE_HOVER } else { look::TILE });
            widgets::icon(p, rect.center(), icons::X, rect.width() * 0.55, look::LABEL);
        }
        Look::Caption => draw_caption(p, rect, facts),
        Look::Capture(n) => draw_capture(p, rect, n, facts, hovered, pressed),
    }
}

/// A 3D picture's button: says "3D picture" on itself, a 3D badge on its icon, and what of. Under way, it is the countdown.
#[allow(clippy::too_many_arguments)]
fn picture_button(p: &Painter, _zone: Zone, shape: Shaped, rect: Rect, mode: PhotoMode, facts: &Facts, hovered: bool, pressed: bool) {
    let mine = facts.picture_mode == Some(mode);
    let busy = facts.countdown.is_some() || facts.picture_flying;
    let lit = mine && busy;
    let dim = (busy && !mine) || facts.no_kit;
    match shape.ring {
        Some((ring, center)) => {
            if lit {
                p.rect_filled(rect, 0.0, theme::ACCENT);
            } else {
                ring_fill(p, ring, center, if pressed { 0.65 } else if hovered { 0.38 } else { 0.0 });
                ring_rim(p, ring, center, hovered);
            }
        }
        None => {
            if lit {
                p.rect_filled(rect, theme::radius(look::RADIUS), theme::ACCENT);
            } else {
                tile_back(p, rect, hovered, pressed, false);
            }
        }
    }
    let (ink, label) = if lit {
        (theme::ON_ACCENT, theme::ON_ACCENT)
    } else if dim {
        (theme::FG_4, theme::FG_4)
    } else {
        (look::LABEL, look::LABEL)
    };
    let Some((ring, center)) = shape.ring else {
        // On the sheet, a coloured tile as All tools': the two kinds of picture told apart, as on the app's own 3D capture page.
        let tint = if mode == PhotoMode::People { theme::VIOLET } else { theme::ACCENT };
        let radius = theme::radius(look::TILE_RADIUS + 2.0);
        if !lit {
            p.rect_filled(rect, radius, theme::mix(look::TILE, tint, if pressed { 0.28 } else if hovered { 0.22 } else if dim { 0.04 } else { 0.14 }));
            if hovered {
                p.rect_stroke(rect.shrink(1.0), radius, Stroke::new(1.5, tint), egui::StrokeKind::Middle);
            }
        }
        let at = pos2(rect.left() + 44.0, rect.center().y);
        let x = rect.left() + 84.0;
        let room = rect.right() - x - 8.0;
        if mine {
            if let Some(seconds) = facts.countdown {
                text(p, at, Align2::CENTER_CENTER, &seconds.to_string(), 44.0, weight::BOLD, ink);
                text(p, pos2(x, rect.center().y), Align2::LEFT_CENTER, "3D picture in", 20.0, weight::BOLD, ink);
                return;
            }
            if facts.picture_flying {
                widgets::icon(p, at, icons::CUBE, 36.0, ink);
                text(p, pos2(x, rect.center().y), Align2::LEFT_CENTER, "Flying…", 20.0, weight::BOLD, ink);
                return;
            }
        }
        let (glyph, of) = if mode == PhotoMode::People { (icons::USERS, "of people") } else { (icons::IMAGE, "of this world") };
        let of = if facts.no_kit {
            "needs the capture kit"
        } else if facts.picture_wait.is_some() && mine {
            "press to call off"
        } else {
            of
        };
        p.rect_filled(Rect::from_center_size(at, Vec2::splat(56.0)), theme::radius(14.0), theme::alpha(tint, 0.22));
        widgets::icon(p, at, glyph, 32.0, if dim { theme::FG_4 } else { tint });
        badge(p, at, 34.0, if dim { theme::INK_600 } else { look::ACCENT }, look::ON_ACCENT);
        text(p, pos2(x, rect.center().y - 12.0), Align2::LEFT_CENTER, "3D picture", 20.0, weight::BOLD, label);
        let small = theme::font(15.0, weight::MEDIUM);
        p.text(pos2(x, rect.center().y + 14.0), Align2::LEFT_CENTER, fit(p, of, &small, room), small, if dim { label } else { look::MUTED });
        return;
    };
    let middle = ring.middle();
    if mine {
        if let Some(seconds) = facts.countdown {
            curved(p, center, ring.outer - 11.0, middle, "3D picture in", 11.5, weight::BOLD, ink, false);
            text(p, center + zones::polar(ring.inner + 22.0, middle), Align2::CENTER_CENTER, &seconds.to_string(), 26.0, weight::BOLD, ink);
            return;
        }
        if facts.picture_flying {
            widgets::icon(p, center + zones::polar(ring.inner + 18.0, middle), icons::CUBE, 24.0, ink);
            curved(p, center, ring.outer - 12.0, middle, "Flying…", 12.0, weight::BOLD, ink, false);
            return;
        }
    }
    let (glyph, of) = if mode == PhotoMode::People { (icons::USERS, "of people") } else { (icons::IMAGE, "of this world") };
    let of = if facts.no_kit {
        "needs the kit"
    } else if facts.picture_wait.is_some() && mine {
        "press to call off"
    } else {
        of
    };
    let at = center + zones::polar(ring.inner + 14.0, middle);
    widgets::icon(p, at, glyph, 18.0, ink);
    badge(p, at, 18.0, if dim { theme::INK_600 } else { look::ACCENT }, look::ON_ACCENT);
    curved(p, center, ring.outer - 11.0, middle, "3D picture", 11.5, weight::BOLD, label, true);
    curved(p, center, ring.outer - 24.0, middle, of, 10.0, weight::SEMIBOLD, if lit || dim { label } else { look::MUTED }, true);
}

/// A capture in the gallery, as the app's own Photos & captures has its tiles: the picture alone, rounded, a dark foot with its name and what it is on it.
fn draw_capture(p: &Painter, rect: Rect, n: usize, facts: &Facts, hovered: bool, pressed: bool) {
    let clip = p.with_clip_rect(Rect::from_min_max(pos2(0.0, GALLERY_TOP), pos2(1024.0, 576.0)));
    let Some(recent) = facts.recent.get(n) else { return };
    const RADIUS: f32 = 16.0;
    let radii = [theme::squircle(RADIUS); 4];
    let points = orbiters_ui::paint::squircle_points_corners(rect, radii);
    match recent.picture {
        Some(id) => {
            let v = if pressed { 200 } else { 255 };
            orbiters_ui::paint::squircle_texture(&clip, rect, radii, id, cover_uv(rect.size()), Color32::from_rgb(v, v, v));
        }
        None => {
            orbiters_ui::paint::fill_convex(&clip, &points, |_| theme::INK_750);
            widgets::icon(&clip, rect.center() - vec2(0.0, 14.0), icons::CUBE, 36.0, look::MUTED);
        }
    }
    let foot = if hovered { 0.92 } else { 0.8 };
    orbiters_ui::paint::fill_gradient(
        &clip,
        &points,
        &orbiters_ui::paint::Gradient::vertical(rect, &[(0.0, Color32::TRANSPARENT), (0.5, Color32::TRANSPARENT), (1.0, theme::alpha(theme::INK_950, foot))]),
    );
    let room = rect.width() - 28.0;
    let font = theme::font(19.0, weight::BOLD);
    clip.text(pos2(rect.left() + 14.0, rect.bottom() - 37.0), Align2::LEFT_CENTER, fit(&clip, &recent.title, &font, room), font, look::LABEL);
    let font = theme::font(15.0, weight::MEDIUM);
    clip.text(pos2(rect.left() + 14.0, rect.bottom() - 15.0), Align2::LEFT_CENTER, fit(&clip, &recent.state, &font, room), font, theme::FG_2);
    if let Some(done) = recent.progress {
        let bar = Rect::from_min_size(pos2(rect.left() + 14.0, rect.bottom() - 6.0), vec2(room * done.clamp(0.0, 1.0), 3.0));
        clip.rect_filled(bar, theme::radius(1.5), theme::ACCENT);
    }
    if hovered {
        clip.rect_stroke(rect.shrink(1.0), theme::radius(RADIUS), Stroke::new(2.0, look::ACCENT), egui::StrokeKind::Middle);
    }
}

/// The words about what is pointed at, or what to do first, in a pill as wide as they are. Nothing to say: nothing shows.
fn draw_caption(p: &Painter, rect: Rect, facts: &Facts) {
    let words = said_now(facts).or_else(|| facts.pointed.and_then(|a| caption(a, facts))).or_else(|| facts.hint.map(|(a, b)| (a.to_string(), b.to_string())));
    let Some((first, second)) = words else { return };
    draw_words(p, rect, &first, &second);
}

/// What goes before anything else the caption would say: what stopped a 3D picture, or what one waits for.
fn said_now(facts: &Facts) -> Option<(String, String)> {
    facts.note.clone().or_else(|| facts.picture_wait.map(wait_words)).or_else(|| facts.n4n0_says.clone().map(|said| ("N4N0".to_string(), said)))
}

/// What a sheet's head says of what is going on.
fn status_now(facts: &Facts) -> Option<(String, String)> {
    if let Some(seconds) = facts.countdown {
        return Some((format!("3D picture in {seconds}"), "VRChat's camera then flies around by itself.".into()));
    }
    if facts.picture_flying {
        return Some(("3D picture in progress".into(), "The camera is flying. Stay where you are for a moment.".into()));
    }
    said_now(facts).or_else(|| facts.invite_said.clone().map(|said| (facts.sheet_title.clone(), said)))
}

/// What a 3D picture waits for, in the caption's two lines.
pub fn wait_words(wait: orbiters_n4n0::status::PhotoWait) -> (String, String) {
    match wait {
        orbiters_n4n0::status::PhotoWait::Camera => ("Open VRChat's camera: the 3D picture then starts".into(), "1 Quick Menu   2 Camera icon   3 Stream Camera".into()),
        orbiters_n4n0::status::PhotoWait::Move => ("Move VRChat's camera a little".into(), "The 3D picture starts as soon as Orbiters sees where it is.".into()),
    }
}

/// Two lines in a pill as wide as they are.
pub fn draw_words(p: &Painter, rect: Rect, first: &str, second: &str) {
    let big = theme::font(13.0, weight::BOLD);
    let small = theme::font(11.0, weight::MEDIUM);
    let room = rect.width() - 24.0;
    let first = fit(p, first, &big, room);
    let second = fit(p, second, &small, room);
    let width = p.layout_no_wrap(first.clone(), big.clone(), Color32::WHITE).size().x.max(p.layout_no_wrap(second.clone(), small.clone(), Color32::WHITE).size().x) + 24.0;
    let height = if second.is_empty() { 30.0 } else { rect.height() - 8.0 };
    let pill = Rect::from_center_size(rect.center(), vec2(width, height));
    p.rect_filled(pill, theme::radius(look::RADIUS + 2.0), look::PANEL);
    if second.is_empty() {
        p.text(pill.center(), Align2::CENTER_CENTER, first, big, look::MUTED);
        return;
    }
    p.text(pill.center() - vec2(0.0, 9.0), Align2::CENTER_CENTER, first, big, look::LABEL);
    p.text(pill.center() + vec2(0.0, 9.0), Align2::CENTER_CENTER, second, small, look::MUTED);
}

/// The part of a 16:9 thumbnail that fills a box of this shape without stretching it.
fn cover_uv(size: Vec2) -> Rect {
    let (box_aspect, picture_aspect) = (size.x / size.y.max(1.0), 16.0 / 9.0);
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
            recent: (0..4).map(|i| Recent { dir: PathBuf::from(format!("s{i}")), title: format!("World {i}"), state: "3D picture, 2 h ago".into(), progress: None, picture: None }).collect(),
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
        let desktop = Facts { desktop: true, ..facts() };
        let on = names(&desktop);
        for absent in ["Call", "Saved looks", "Light", "Fast walk", "Smooth tracking"] {
            assert!(!on.contains(&absent), "{absent} on a desktop with none of it");
        }
        for present in ["Worlds in 3D", "YouTube", "Previous instances", "Now playing", "3D pictures", "Open Orbiters"] {
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
        for main in ["YouTube", "Previous instances", "3D pictures"] {
            for accessory in ["QR code", "Suggest", "World feedback", "Open Orbiters", "Ask N4N0"] {
                assert!(area(&facts, main) > area(&facts, accessory) * 2.0, "{main} against {accessory}");
            }
        }
        // What plays has its controls where it is, and no page to go to.
        let items = sheet_items(Sheet::All, &Facts { music: Some(("A song".into(), true)), volume: Some((50, false)), ..facts.clone() });
        for action in [Action::PlayPause, Action::Previous, Action::Next, Action::Volume(true), Action::Volume(false), Action::Mute] {
            assert!(items.iter().any(|i| i.action == action), "{action:?}");
        }
        assert!(!items.iter().any(|i| i.action == Action::Page(Target::Music)));
    }

    #[test]
    fn the_gallery_scrolls_through_everything_and_keeps_its_buttons_on_top() {
        let many = |n: usize| Facts { recent: (0..n).map(|i| Recent { dir: PathBuf::from(format!("s{i}")), title: format!("W{i}"), state: String::new(), progress: None, picture: None }).collect(), desktop: true, ..facts() };
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
        let visit = |i: usize| WorldVisit { world_id: format!("wrld_{i}"), name: format!("W{i}"), line: String::new(), location: format!("wrld_{i}:1"), earlier: Vec::new(), picture: None };
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
    #[test]
    fn what_n4n0_says_shows_in_the_caption_and_a_sheet_s_head() {
        let facts = Facts { n4n0_says: Some("It is sunny".into()), ..facts() };
        assert_eq!(said_now(&facts), Some(("N4N0".to_string(), "It is sunny".to_string())));
        assert_eq!(status_now(&facts).unwrap().1, "It is sunny");
        // What stopped a picture goes before it.
        let stopped = Facts { note: Some(("a".into(), "b".into())), ..facts };
        assert_eq!(said_now(&stopped).unwrap().0, "a");
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
