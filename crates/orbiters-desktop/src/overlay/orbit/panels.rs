//! What each part holds, in its own units (around the wheel a hundredth of its radius, around the Quick Menu its unit: a pixel of a 1280 x 720 window either way), so that the
//! desktop's windows and the headset's overlays draw and hit the same things. [`items`] says where everything is and [`paint`] draws it: nothing else decides a place, so what
//! the pointer hits is what is drawn.
//!
//! **VRChat's own look** ([`look`]): the colours are sampled from its real frames, tile edges and teal icons as its Quick Menu has them, the ring in the wheel's own radial
//! fade and bright rim. The app's mint is kept for what is under way (a countdown, a filled state), never to favour one feature over another.
//!
//! **For someone who has never heard of any of this:** each thing says what it does in its own words, and [`caption`] says in a sentence what is pointed at, near it. What
//! cannot work here is not offered ([`Facts`]' `can_*`): no dead pages.

use super::zones::{self, Ring, Zone};
use egui::epaint::{Mesh, RectShape, Vertex, WHITE_UV};
use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, TextureId, Vec2};
use orbiters_n4n0_core::menu::{PhotoMode, Target};
use orbiters_ui::theme::{self, weight};
use orbiters_ui::{icons, widgets};
use std::path::PathBuf;

/// VRChat's colours, sampled from real frames of its Quick Menu and wheel (2026-11-06).
pub mod look {
    use egui::Color32;
    /// Its panels' fill.
    pub const PANEL: Color32 = Color32::from_rgba_premultiplied(14, 19, 25, 246);
    /// Its wing's header band.
    pub const HEADER: Color32 = Color32::from_rgb(26, 31, 40);
    /// Its tiles.
    pub const TILE: Color32 = Color32::from_rgb(22, 47, 53);
    /// Its tiles' edge.
    pub const EDGE: Color32 = Color32::from_rgb(38, 82, 90);
    /// Its teal: icons, titles, a hovered edge.
    pub const TEAL: Color32 = Color32::from_rgb(52, 185, 205);
    /// A selected tab.
    pub const SELECTED: Color32 = Color32::from_rgb(0, 132, 139);
    /// Its labels.
    pub const LABEL: Color32 = Color32::from_rgb(232, 244, 246);
    pub const MUTED: Color32 = Color32::from_rgb(140, 176, 184);
    /// The wheel's slice colour, when it could not be read.
    pub const WHEEL: [u8; 3] = [7, 55, 58];
    pub const RADIUS: f32 = 6.0;
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
    /// A 3D picture's timer, in seconds.
    pub timer: u32,
    /// VRChat's wheel's own colour (RGB), when it was read: the ring takes after it.
    pub tint: Option<[u8; 3]>,
    /// What the caption says with nothing pointed at (a key to hold).
    pub hint: Option<(&'static str, &'static str)>,
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
    /// A tile as VRChat's: its icon over its label, edged.
    Tile { icon: &'static str, label: &'static str, badge: bool },
    /// A large tile on a sheet, with what it does.
    Wide { icon: &'static str, label: &'static str, line: &'static str, badge: bool },
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
    match zone {
        Zone::WheelWorlds => cell(Action::Worlds, icons::GLOBE_HEMISPHERE_WEST, "Worlds in 3D", true),
        Zone::WheelYouTube => cell(Action::Page(Target::YouTube), icons::YOUTUBE_LOGO, "YouTube", false),
        Zone::WheelPlaying => vec![Item { action: Action::PlayPause, rect: whole, look: Look::Playing }],
        Zone::WheelAll => cell(Action::All, icons::DOTS_THREE, "All tools", false),
        Zone::WheelInstances => cell(Action::Page(Target::Instances), icons::CLOCK, "Instances", false),
        Zone::WheelPictures => cell(Action::Pictures, icons::CUBE, "3D pictures", false),
        Zone::BloomPeople => vec![Item { action: Action::Picture(PhotoMode::People), rect: whole, look: Look::Picture(PhotoMode::People) }],
        Zone::BloomWorld => vec![Item { action: Action::Picture(PhotoMode::World), rect: whole, look: Look::Picture(PhotoMode::World) }],
        Zone::BloomRecord => vec![Item { action: Action::Record, rect: whole, look: Look::Record }],
        Zone::BloomRecent => vec![Item { action: Action::Pictures, rect: whole, look: Look::Capture(0) }],
        Zone::BloomPrevious => vec![Item { action: Action::Previous, rect: whole, look: Look::Transport(icons::SKIP_BACK) }],
        Zone::BloomPlay => vec![Item { action: Action::PlayPause, rect: whole, look: Look::Transport(if facts.music.as_ref().is_some_and(|m| m.1) { icons::PAUSE } else { icons::PLAY }) }],
        Zone::BloomNext => vec![Item { action: Action::Next, rect: whole, look: Look::Transport(icons::SKIP_FORWARD) }],
        Zone::WheelCaption | Zone::QmCaption => vec![Item { action: Action::Idle, rect: whole, look: Look::Caption }],
        Zone::WheelVeil => vec![Item { action: Action::Idle, rect: whole, look: Look::Blank }],
        Zone::WheelHide => vec![Item { action: Action::Hide, rect: whole, look: Look::Close }],
        Zone::QmWing => wing(w, h, facts),
        Zone::SheetBack => {
            let head = 48.0;
            let mut out = vec![Item { action: Action::CloseSheet, rect: r(w - 40.0, 9.0, 30.0, 30.0), look: Look::Close }];
            if facts.music.is_some() {
                // The foot: what plays, whichever page shows.
                let y = h - head + 8.0;
                let b = 32.0;
                out.push(Item { action: Action::Next, rect: r(w - 12.0 - b, y, b, b), look: Look::Transport(icons::SKIP_FORWARD) });
                out.push(Item { action: Action::PlayPause, rect: r(w - 12.0 - b * 2.0 - 6.0, y, b, b), look: Look::Transport(if facts.music.as_ref().is_some_and(|m| m.1) { icons::PAUSE } else { icons::PLAY }) });
                out.push(Item { action: Action::Previous, rect: r(w - 12.0 - b * 3.0 - 12.0, y, b, b), look: Look::Transport(icons::SKIP_BACK) });
            }
            out
        }
        // The sheet's page, as its sheet says ([`sheet_items`]) or N4N0's own.
        Zone::Takeover => facts.sheet.map_or_else(Vec::new, |s| sheet_items(s, facts)),
    }
}

/// The wing: VRChat's two columns of tiles under its header, what plays at its foot.
fn wing(w: f32, h: f32, facts: &Facts) -> Vec<Item> {
    const HEAD: f32 = 36.0;
    const PAD: f32 = 8.0;
    let foot = 58.0;
    let width = (w - PAD * 2.0 - GAP) / 2.0;
    let rows = 3.0;
    let height = (h - HEAD - PAD * 2.0 - foot - GAP * rows) / rows;
    let at = |c: f32, i: f32| r(PAD + c * (width + GAP), HEAD + PAD + i * (height + GAP), width, height);
    let tile = |action, icon, label, badge, rect| Item { action, rect, look: Look::Tile { icon, label, badge } };
    let mut out = vec![
        tile(Action::Worlds, icons::GLOBE_HEMISPHERE_WEST, "Worlds in 3D", true, at(0.0, 0.0)),
        tile(Action::Page(Target::YouTube), icons::YOUTUBE_LOGO, "YouTube", false, at(1.0, 0.0)),
        tile(Action::Page(Target::Instances), icons::CLOCK, "Instances", false, at(0.0, 1.0)),
        tile(Action::Pictures, icons::CUBE, "3D pictures", false, at(1.0, 1.0)),
        Item { action: Action::Talk, rect: at(0.0, 2.0), look: Look::Talk },
        tile(Action::All, icons::DOTS_THREE, "All tools", false, at(1.0, 2.0)),
        Item { action: Action::PlayPause, rect: r(PAD, h - PAD - foot, w - PAD * 2.0, foot), look: Look::Playing },
    ];
    if facts.vr_close {
        out.push(Item { action: Action::Hide, rect: r(w - 32.0, 5.0, 26.0, 26.0), look: Look::Close });
    }
    out
}

/// What the sheet's page holds, in N4N0's page's units (1024 x 576).
pub fn sheet_items(sheet: Sheet, facts: &Facts) -> Vec<Item> {
    match sheet {
        Sheet::All => {
            let mut list: Vec<(Action, &'static str, &'static str, &'static str, bool)> = vec![
                (Action::Worlds, icons::GLOBE_HEMISPHERE_WEST, "Worlds in 3D", "Search worlds, and walk in their 3D captures", true),
                (Action::Page(Target::YouTube), icons::YOUTUBE_LOGO, "YouTube", "Find a video for the world's player", false),
                (Action::Page(Target::Instances), icons::CLOCK, "Previous instances", "Where you were lately: go back", false),
                (Action::Page(Target::Music), icons::MUSIC_NOTES, "Now playing", "Your computer's music and volume", false),
                (Action::Pictures, icons::CUBE, "3D pictures", "Take one, record, see your captures", false),
                (Action::Talk, icons::MICROPHONE, "Ask N4N0", "Hold and speak to the assistant", false),
            ];
            if facts.can_call {
                list.push((Action::Page(Target::Call), icons::PHONE, "Call", "A friend, wherever they are in VRChat", false));
            }
            if facts.can_states {
                list.push((Action::Page(Target::States), icons::BOOKMARKS_SIMPLE, "Saved looks", "Your avatar's looks, in one press", false));
            }
            if facts.can_light {
                list.push((Action::Page(Target::Light), icons::LIGHTBULB, "Light", "N4N0 as a light that follows you", false));
            }
            if !facts.desktop {
                list.push((Action::FastWalk, icons::PERSON_SIMPLE_RUN, "Fast walk", "Walk faster than VRChat lets you", false));
                list.push((Action::Page(Target::Tracking), icons::VIRTUAL_REALITY, "Smooth tracking", "Steadier headset and trackers", false));
            }
            list.push((Action::Page(Target::WorldFeedback), icons::HEART, "World feedback", "Like this world, label it, comment", false));
            list.push((Action::Page(Target::Qr), icons::QR_CODE, "QR code", "Open a link from VRChat's camera", false));
            list.push((Action::Page(Target::Suggest), icons::CHAT_TEARDROP_TEXT, "Suggest", "Tell the team what you would like", false));
            list.push((Action::OpenApp, icons::N4N0_MARK, "Open Orbiters", "The full app on your computer", false));
            let cols = 4;
            let rows = list.len().div_ceil(cols).clamp(1, 4) as f32;
            let (x0, y0, gap) = (24.0, 14.0, 12.0);
            let width = (1024.0 - x0 * 2.0 - gap * (cols as f32 - 1.0)) / cols as f32;
            let height = (576.0 - y0 * 2.0 - gap * (rows - 1.0)) / rows;
            list.into_iter()
                .enumerate()
                .map(|(i, (action, icon, label, line, badge))| {
                    let (c, row) = ((i % cols) as f32, (i / cols) as f32);
                    Item { action, rect: r(x0 + c * (width + gap), y0 + row * (height + gap), width, height), look: Look::Wide { icon, label, line, badge } }
                })
                .collect()
        }
        Sheet::Pictures => {
            let (x0, gap) = (24.0, 12.0);
            let width = (1024.0 - x0 * 2.0 - gap * 2.0) / 3.0;
            let mut out = vec![
                Item { action: Action::Picture(PhotoMode::People), rect: r(x0, 14.0, width, 190.0), look: Look::Picture(PhotoMode::People) },
                Item { action: Action::Picture(PhotoMode::World), rect: r(x0 + width + gap, 14.0, width, 190.0), look: Look::Picture(PhotoMode::World) },
                Item { action: Action::Record, rect: r(x0 + (width + gap) * 2.0, 14.0, width, 190.0), look: Look::Record },
            ];
            // A grid of whole pictures, the last place to all of them.
            let (cols, rows) = (4usize, 2usize);
            let cw = (1024.0 - x0 * 2.0 - gap * (cols as f32 - 1.0)) / cols as f32;
            let top = 216.0;
            let ch = (576.0 - top - 14.0 - gap * (rows as f32 - 1.0)) / rows as f32;
            let at = |i: usize| r(x0 + (i % cols) as f32 * (cw + gap), top + (i / cols) as f32 * (ch + gap), cw, ch);
            let shown = facts.recent.len().min(cols * rows - 1);
            for i in 0..shown {
                out.push(Item { action: Action::Page(Target::History), rect: at(i), look: Look::Capture(i) });
            }
            out.push(Item { action: Action::Page(Target::History), rect: at(shown), look: Look::Wide { icon: icons::CLOCK, label: "All captures", line: "See them, share them", badge: false } });
            out
        }
        Sheet::Menu | Sheet::Worlds => Vec::new(),
    }
}

/// What is on the item at `p` (panel units): its index. The small close button lies on top of what it sits on.
pub fn hit(items: &[Item], p: Pos2) -> Option<usize> {
    items.iter().enumerate().rev().find(|(_, i)| i.rect.contains(p)).map(|(i, _)| i)
}

/// What the caption says of what is pointed at, in words for someone who has never seen any of this: a title and what it does.
pub fn caption(action: Action, facts: &Facts) -> Option<(String, String)> {
    let (a, b): (String, String) = match action {
        Action::Worlds => ("Worlds in 3D".into(), "Search any world. Those with a 3D capture, you can walk in before going.".into()),
        Action::Picture(mode) => {
            if let Some(s) = facts.countdown {
                (format!("3D picture in {s}"), "VRChat's camera opens, then flies around by itself.".into())
            } else if facts.picture_flying {
                ("3D picture in progress".into(), "The camera is flying. Stay where you are for a moment.".into())
            } else if mode == PhotoMode::People {
                ("3D picture of people".into(), "Keeps those around you in 3D: look around them afterwards.".into())
            } else {
                ("3D picture of this world".into(), "Keeps this place in 3D, without you in it: walk in it afterwards.".into())
            }
        }
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
        Action::Talk if facts.n4n0 => ("Ask N4N0".into(), "Hold and speak: N4N0 is your assistant in VRChat.".into()),
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

/// A cell in the wheel's own material: its slice colour fading outward as VRChat's does, more of it toward the wheel. `lift` lightens it (under the pointer).
fn ring_fill(p: &Painter, ring: Ring, center: Pos2, tint: [u8; 3], lift: f32) {
    let base = Color32::from_rgb(tint[0], tint[1], tint[2]);
    let (near, far) = (theme::mix(base, Color32::from_rgb(40, 200, 205), 0.14 + lift * 0.5), theme::mix(base, Color32::from_rgb(40, 200, 205), lift * 0.42));
    let far = theme::mix(far, Color32::BLACK, 0.12 * (1.0 - lift));
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

/// The bright line VRChat's wheel has at its rim, along a cell's outer edge.
fn ring_rim(p: &Painter, ring: Ring, center: Pos2, tint: [u8; 3], strong: bool) {
    let base = Color32::from_rgb(tint[0], tint[1], tint[2]);
    let color = theme::mix(base, Color32::from_rgb(120, 225, 230), if strong { 0.95 } else { 0.42 });
    let steps = 24;
    let line: Vec<Pos2> = (0..=steps).map(|i| center + zones::polar(ring.outer - 1.2, ring.from + (ring.to - ring.from) * i as f32 / steps as f32)).collect();
    p.add(Shape::line(line, Stroke::new(if strong { 2.2 } else { 1.6 }, color)));
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

fn tint_of(facts: &Facts) -> [u8; 3] {
    facts.tint.unwrap_or(look::WHEEL)
}

/// Draws a part (in units, its corner at the painter's origin): its fill and everything on it.
pub fn paint(p: &Painter, zone: Zone, size: Vec2, facts: &Facts, hover: Option<usize>, pressed: Option<usize>) {
    let panel = Rect::from_min_size(pos2(0.0, 0.0), size);
    match zone {
        Zone::WheelVeil => {
            // A dark veil over VRChat's wheel, and whose turn it is at its hub.
            let c = panel.center();
            p.circle_filled(c, size.x / 2.0, Color32::from_rgba_premultiplied(5, 12, 14, 178));
            widgets::icon(p, c - vec2(0.0, 6.0), icons::N4N0_MARK, 30.0, look::LABEL);
            text(p, c + vec2(0.0, 18.0), Align2::CENTER_CENTER, "Pointing here", 10.0, weight::SEMIBOLD, look::LABEL);
            return;
        }
        Zone::QmWing => {
            p.rect_filled(panel, theme::radius(look::RADIUS), look::PANEL);
            let header = Rect::from_min_size(pos2(0.0, 0.0), vec2(size.x, 36.0));
            p.rect_filled(header, egui::CornerRadius { nw: look::RADIUS as u8, ne: look::RADIUS as u8, sw: 0, se: 0 }, look::HEADER);
            widgets::icon(p, pos2(19.0, 18.0), icons::LIST, 20.0, look::TEAL);
            bold(p, pos2(36.0, 18.0), Align2::LEFT_CENTER, "Orbiters", 15.0, look::LABEL);
            if facts.recording {
                // Only while it runs: what it does, in the header.
                p.circle_filled(pos2(size.x - 52.0, 18.0), 4.5, theme::REC);
                text(p, pos2(size.x - 8.0, 18.0), Align2::RIGHT_CENTER, &clock(facts.recording_seconds.unwrap_or(0)), 11.5, weight::SEMIBOLD, look::LABEL);
            } else if let Some(done) = facts.reconstructing {
                text(p, pos2(size.x - 8.0, 18.0), Align2::RIGHT_CENTER, &format!("3D {:.0}%", done * 100.0), 11.5, weight::SEMIBOLD, look::TEAL);
            } else if !facts.vr_close {
                widgets::icon(p, pos2(size.x - 18.0, 18.0), icons::N4N0_MARK, 15.0, look::TEAL);
            }
        }
        Zone::SheetBack => {
            p.rect_filled(panel, theme::radius(look::RADIUS + 2.0), Color32::from_rgb(14, 19, 25));
            let head = Rect::from_min_size(pos2(0.0, 0.0), vec2(size.x, 48.0));
            p.rect_filled(head, egui::CornerRadius { nw: (look::RADIUS + 2.0) as u8, ne: (look::RADIUS + 2.0) as u8, sw: 0, se: 0 }, look::HEADER);
            widgets::icon(p, pos2(26.0, 24.0), icons::LIST, 22.0, look::TEAL);
            bold(p, pos2(46.0, 24.0), Align2::LEFT_CENTER, "Orbiters", 15.0, look::TEAL);
            bold(p, pos2(122.0, 24.0), Align2::LEFT_CENTER, &facts.sheet_title, 18.0, look::LABEL);
            // The foot: what plays, whichever page shows.
            let y = size.y - 24.0;
            if let Some((title, playing)) = &facts.music {
                widgets::icon_fill(p, pos2(26.0, y), icons::MUSIC_NOTES, 18.0, look::TEAL);
                let font = theme::font(13.0, weight::SEMIBOLD);
                let name = fit(p, title, &font, size.x - 200.0);
                p.text(pos2(46.0, y), Align2::LEFT_CENTER, name, font, if *playing { look::LABEL } else { look::MUTED });
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
    p.rect_filled(page, 0.0, Color32::from_rgb(14, 19, 25));
    for (i, item) in sheet_items(sheet, facts).iter().enumerate() {
        draw(p, Zone::Takeover, item, facts, hover == Some(i), pressed == Some(i));
    }
}

/// A tile as VRChat's: filled, edged, brighter under the pointer.
fn tile_back(p: &Painter, rect: Rect, hovered: bool, pressed: bool, selected: bool) {
    let fill = if selected {
        look::SELECTED
    } else if pressed {
        theme::mix(look::TILE, look::TEAL, 0.5)
    } else if hovered {
        theme::mix(look::TILE, look::TEAL, 0.25)
    } else {
        look::TILE
    };
    p.rect_filled(rect, theme::radius(look::RADIUS), fill);
    p.rect_stroke(rect, theme::radius(look::RADIUS), Stroke::new(1.5, if hovered || selected { look::TEAL } else { look::EDGE }), StrokeKind::Inside);
}

fn is_selected(item: &Item, facts: &Facts) -> bool {
    match (facts.sheet, item.action) {
        (Some(Sheet::Worlds), Action::Worlds) | (Some(Sheet::All), Action::All) | (Some(Sheet::Pictures), Action::Pictures) => true,
        (Some(Sheet::Menu), Action::Page(Target::YouTube)) => facts.sheet_title == "YouTube",
        (Some(Sheet::Menu), Action::Page(Target::Instances)) => facts.sheet_title == "Previous instances",
        _ => false,
    }
}

fn draw(p: &Painter, zone: Zone, item: &Item, facts: &Facts, hovered: bool, pressed: bool) {
    let rect = item.rect;
    let shape = shaped(zone, rect);
    let tint = tint_of(facts);
    match item.look {
        Look::Blank => {}
        Look::Cell { icon, label, badge: has_badge } => {
            let (ring, center) = shape.ring.expect("a cell is on a ring");
            ring_fill(p, ring, center, tint, if pressed { 0.65 } else if hovered { 0.38 } else { 0.0 });
            ring_rim(p, ring, center, tint, hovered);
            let size = 36.0;
            let top = shape.at.y - (size + 4.0 + 15.0) / 2.0;
            let at = pos2(shape.at.x, top + size / 2.0);
            if icon == icons::N4N0_MARK {
                widgets::icon(p, at, icon, size, look::LABEL);
            } else {
                widgets::icon_fill(p, at, icon, size, look::LABEL);
            }
            if has_badge {
                badge(p, at, size, look::TEAL, Color32::from_rgb(4, 24, 28));
            }
            bold(p, pos2(shape.at.x, top + size + 4.0 + 7.5), Align2::CENTER_CENTER, label, 14.0, look::LABEL);
        }
        Look::Tile { icon, label, badge: has_badge } => {
            let selected = is_selected(item, facts);
            tile_back(p, rect, hovered, pressed, selected);
            let ink = if selected { look::LABEL } else { look::TEAL };
            let size = (rect.height() * 0.42).clamp(18.0, 32.0);
            let top = rect.center().y - (size + 3.0 + 11.0) / 2.0;
            let at = pos2(rect.center().x, top + size / 2.0);
            if icon == icons::N4N0_MARK {
                widgets::icon(p, at, icon, size, ink);
            } else {
                widgets::icon_fill(p, at, icon, size, ink);
            }
            if has_badge {
                badge(p, at, size, look::TEAL, Color32::from_rgb(4, 24, 28));
            }
            let font_size = {
                let mut s = 11.0;
                while s > 8.5 && p.layout_no_wrap(label.to_string(), theme::font(s, weight::BOLD), Color32::WHITE).size().x > rect.width() - 6.0 {
                    s -= 0.5;
                }
                s
            };
            bold(p, pos2(rect.center().x, top + size + 3.0 + 5.5), Align2::CENTER_CENTER, label, font_size, look::LABEL);
        }
        Look::Wide { icon, label, line, badge: has_badge } => {
            tile_back(p, rect, hovered, pressed, false);
            let size = 38.0;
            let at = pos2(rect.left() + 36.0, rect.center().y);
            if icon == icons::N4N0_MARK {
                widgets::icon(p, at, icon, size - 8.0, look::TEAL);
            } else {
                widgets::icon_fill(p, at, icon, size, look::TEAL);
            }
            if has_badge {
                badge(p, at, size, look::TEAL, Color32::from_rgb(4, 24, 28));
            }
            let x = rect.left() + 66.0;
            let room = rect.right() - x - 8.0;
            let mut size = 19.0;
            while size > 14.0 && p.layout_no_wrap(label.to_string(), theme::font(size, weight::BOLD), Color32::WHITE).size().x > room {
                size -= 0.5;
            }
            let font = theme::font(size, weight::BOLD);
            let name = fit(p, label, &font, room);
            p.text(pos2(x, rect.center().y - 24.0), Align2::LEFT_CENTER, name, font, look::LABEL);
            let small = theme::font(14.0, weight::MEDIUM);
            let (one, two) = two_lines(p, line, &small, room);
            p.text(pos2(x, rect.center().y + 1.0), Align2::LEFT_CENTER, one, small.clone(), look::MUTED);
            if let Some(two) = two {
                p.text(pos2(x, rect.center().y + 20.0), Align2::LEFT_CENTER, two, small, look::MUTED);
            }
        }
        Look::Picture(mode) => picture_button(p, zone, shape, rect, mode, facts, hovered, pressed, tint),
        Look::Record => {
            let label = if facts.recording { "Stop" } else { "Record" };
            let dim = facts.capture_busy && !facts.recording;
            if let Some((ring, center)) = shape.ring {
                if facts.recording {
                    ring_fill(p, ring, center, [120, 28, 44], if hovered { 0.1 } else { 0.0 });
                } else {
                    ring_fill(p, ring, center, tint, if pressed { 0.65 } else if hovered { 0.38 } else { 0.0 });
                }
                ring_rim(p, ring, center, tint, hovered);
            } else {
                tile_back(p, rect, hovered, pressed, false);
            }
            let large = shape.ring.is_none();
            let top = shape.at.y - if large { 34.0 } else { 12.0 };
            let at = pos2(shape.at.x, top);
            let color = if dim { theme::FG_4 } else { look::LABEL };
            if facts.recording {
                p.rect_filled(Rect::from_center_size(at, Vec2::splat(if large { 30.0 } else { 16.0 })), theme::radius(3.0), color);
            } else {
                p.circle_filled(at, if large { 19.0 } else { 9.5 }, if dim { theme::FG_4 } else { theme::REC });
            }
            let time = facts.recording_seconds.filter(|_| facts.recording).map_or(label.to_string(), clock);
            bold(p, pos2(shape.at.x, top + if large { 38.0 } else { 20.0 }), Align2::CENTER_CENTER, &time, if large { 19.0 } else { 12.0 }, color);
            if large {
                let small = theme::font(14.0, weight::MEDIUM);
                p.text(pos2(shape.at.x, top + 62.0), Align2::CENTER_CENTER, "A 3D scene of where you walk", small, look::MUTED);
            }
        }
        Look::Playing => {
            let none = facts.music.is_none();
            let (name, playing) = facts.music.clone().unwrap_or_default();
            let glyph = if playing { icons::PAUSE } else if none { icons::MUSIC_NOTES } else { icons::PLAY };
            if let Some((ring, center)) = shape.ring {
                ring_fill(p, ring, center, tint, if pressed { 0.65 } else if hovered && !none { 0.38 } else { 0.0 });
                if let (Some(id), false) = (facts.art, none) {
                    p.add(RectShape::filled(rect, 0.0, Color32::from_rgb(120, 130, 132)).with_texture(id, cover_uv(rect.size())));
                    p.rect_filled(rect, 0.0, Color32::from_black_alpha(90));
                    cell_edge(p, ring, center, 3.0, Color32::from_rgb(tint[0], tint[1], tint[2]));
                }
                ring_rim(p, ring, center, tint, hovered);
                let ink = if none { look::MUTED } else { look::LABEL };
                let size = 26.0;
                widgets::icon_fill(p, shape.at - vec2(0.0, 22.0), glyph, size, ink);
                let font = theme::font(12.5, weight::BOLD);
                if none {
                    p.text(shape.at + vec2(0.0, 5.0), Align2::CENTER_CENTER, "Nothing playing", font, look::MUTED);
                } else {
                    let (one, two) = two_lines(p, &name, &font, shape.room * 0.8);
                    p.text(shape.at + vec2(0.0, 5.0), Align2::CENTER_CENTER, one, font.clone(), ink);
                    if let Some(two) = two {
                        p.text(shape.at + vec2(0.0, 21.0), Align2::CENTER_CENTER, two, font, ink);
                    }
                }
            } else {
                tile_back(p, rect, hovered && !none, pressed && !none, false);
                let side = rect.height() - 12.0;
                let art = Rect::from_min_size(rect.min + vec2(6.0, 6.0), Vec2::splat(side));
                if let (Some(id), false) = (facts.art, none) {
                    p.add(RectShape::filled(art, theme::radius(4.0), Color32::WHITE).with_texture(id, cover_uv(art.size())));
                } else {
                    p.rect_filled(art, theme::radius(4.0), theme::mix(look::TILE, Color32::BLACK, 0.3));
                }
                widgets::icon_fill(p, art.center(), glyph, side * 0.5, if none { look::MUTED } else { look::LABEL });
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
                ring_fill(p, ring, center, tint, if pressed { 0.65 } else if hovered { 0.38 } else { 0.0 });
                ring_rim(p, ring, center, tint, hovered);
                widgets::icon_fill(p, shape.at, glyph, 24.0, look::LABEL);
            } else {
                p.circle_filled(rect.center(), rect.width() / 2.0, if hovered { theme::mix(look::TILE, look::TEAL, 0.3) } else { look::TILE });
                widgets::icon_fill(p, rect.center(), glyph, rect.width() * 0.5, look::LABEL);
            }
        }
        Look::Talk => {
            let on = pressed && facts.n4n0;
            tile_back(p, rect, hovered, false, on);
            let color = if on { look::LABEL } else if facts.n4n0 { look::TEAL } else { theme::FG_4 };
            let size = (rect.height() * 0.42).clamp(18.0, 32.0);
            let top = rect.center().y - (size + 3.0 + 11.0) / 2.0;
            widgets::icon_fill(p, pos2(rect.center().x, top + size / 2.0), icons::MICROPHONE, size, color);
            bold(p, pos2(rect.center().x, top + size + 3.0 + 5.5), Align2::CENTER_CENTER, if on { "Listening" } else { "Ask N4N0" }, 11.0, if facts.n4n0 { look::LABEL } else { theme::FG_4 });
        }
        Look::Close => {
            p.circle_filled(rect.center(), rect.width() / 2.0, if hovered { theme::mix(look::TILE, look::TEAL, 0.35) } else { look::TILE });
            widgets::icon(p, rect.center(), icons::X, rect.width() * 0.55, look::LABEL);
        }
        Look::Caption => draw_caption(p, rect, facts),
        Look::Capture(n) => draw_capture(p, shape, rect, n, facts, hovered, pressed, tint),
    }
}

/// A 3D picture's button: says "3D picture" on itself, a 3D badge on its icon, and what of. Under way, it is the countdown.
#[allow(clippy::too_many_arguments)]
fn picture_button(p: &Painter, _zone: Zone, shape: Shaped, rect: Rect, mode: PhotoMode, facts: &Facts, hovered: bool, pressed: bool, tint: [u8; 3]) {
    let mine = facts.picture_mode == Some(mode);
    let busy = facts.countdown.is_some() || facts.picture_flying;
    let lit = mine && busy;
    let dim = busy && !mine;
    match shape.ring {
        Some((ring, center)) => {
            if lit {
                p.rect_filled(rect, 0.0, theme::ACCENT);
            } else {
                ring_fill(p, ring, center, tint, if pressed { 0.65 } else if hovered { 0.38 } else { 0.0 });
                ring_rim(p, ring, center, tint, hovered);
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
    let large = shape.ring.is_none();
    let (ink, label) = if lit {
        (theme::ON_ACCENT, theme::ON_ACCENT)
    } else if dim {
        (theme::FG_4, theme::FG_4)
    } else {
        (look::LABEL, look::LABEL)
    };
    let at = shape.at;
    if mine {
        if let Some(s) = facts.countdown {
            text(p, at - vec2(0.0, 16.0), Align2::CENTER_CENTER, "3D picture in", if large { 18.0 } else { 11.5 }, weight::SEMIBOLD, ink);
            text(p, at + vec2(0.0, if large { 28.0 } else { 12.0 }), Align2::CENTER_CENTER, &s.to_string(), if large { 62.0 } else { 34.0 }, weight::BOLD, ink);
            return;
        }
        if facts.picture_flying {
            widgets::icon_fill(p, at - vec2(0.0, 11.0), icons::CUBE, if large { 44.0 } else { 26.0 }, ink);
            text(p, at + vec2(0.0, if large { 28.0 } else { 14.0 }), Align2::CENTER_CENTER, "Flying…", if large { 20.0 } else { 12.0 }, weight::SEMIBOLD, ink);
            return;
        }
    }
    let (glyph, of) = if mode == PhotoMode::People { (icons::USERS, "of people") } else { (icons::GLOBE_HEMISPHERE_WEST, "of this world") };
    let (size, line, big, small) = if large { (54.0, 24.0, 19.0, 15.0) } else { (28.0, 13.5, 12.0, 10.5) };
    let top = at.y - (size + 4.0 + line * 2.0) / 2.0;
    let icon_at = pos2(at.x, top + size / 2.0);
    widgets::icon_fill(p, icon_at, glyph, size, ink);
    badge(p, icon_at, size, if dim { theme::INK_600 } else { look::TEAL }, Color32::from_rgb(4, 24, 28));
    bold(p, pos2(at.x, top + size + 4.0 + line / 2.0), Align2::CENTER_CENTER, "3D picture", big, label);
    text(p, pos2(at.x, top + size + 4.0 + line * 1.5), Align2::CENTER_CENTER, of, small, weight::SEMIBOLD, if lit || dim { label } else { look::MUTED });
}

/// A recent capture: a whole picture, its name and age under it; on the ring, its cell filled by it.
#[allow(clippy::too_many_arguments)]
fn draw_capture(p: &Painter, shape: Shaped, rect: Rect, n: usize, facts: &Facts, hovered: bool, pressed: bool, tint: [u8; 3]) {
    match shape.ring {
        Some((ring, center)) => {
            ring_fill(p, ring, center, tint, if hovered { 0.38 } else { 0.0 });
            let Some(recent) = facts.recent.get(n) else {
                widgets::icon_fill(p, shape.at - vec2(0.0, 12.0), icons::CUBE, 24.0, look::MUTED);
                text(p, shape.at + vec2(0.0, 14.0), Align2::CENTER_CENTER, "No captures", 11.0, weight::SEMIBOLD, look::MUTED);
                ring_rim(p, ring, center, tint, hovered);
                return;
            };
            if let Some(id) = recent.picture {
                let v = if pressed { 170 } else if hovered { 255 } else { 215 };
                p.add(RectShape::filled(rect, 0.0, Color32::from_rgb(v, v, v)).with_texture(id, cover_uv(rect.size())));
            }
            let band = Rect::from_center_size(shape.at + vec2(0.0, 16.0), vec2(rect.width(), 22.0));
            p.rect_filled(band, 0.0, Color32::from_black_alpha(165));
            text(p, band.center(), Align2::CENTER_CENTER, "Latest", 11.0, weight::BOLD, look::LABEL);
            cell_edge(p, ring, center, 4.0, if hovered { look::TEAL } else { Color32::from_rgb(tint[0], tint[1], tint[2]) });
            if let Some(done) = recent.progress {
                progress_arc(p, ring, center, done, theme::ACCENT);
            }
        }
        None => {
            tile_back(p, rect, hovered, pressed, false);
            let Some(recent) = facts.recent.get(n) else { return };
            let picture = Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(rect.width() - 8.0, rect.height() - 44.0));
            match recent.picture {
                Some(id) => {
                    p.add(RectShape::filled(picture, theme::radius(4.0), Color32::WHITE).with_texture(id, cover_uv(picture.size())));
                }
                None => {
                    p.rect_filled(picture, theme::radius(4.0), theme::mix(look::TILE, Color32::BLACK, 0.3));
                    widgets::icon_fill(p, picture.center(), icons::CUBE, picture.height() * 0.4, look::MUTED);
                }
            }
            let font = theme::font(14.0, weight::BOLD);
            p.text(pos2(rect.left() + 8.0, rect.bottom() - 28.0), Align2::LEFT_CENTER, fit(p, &recent.title, &font, rect.width() - 14.0), font, look::LABEL);
            let font = theme::font(12.0, weight::MEDIUM);
            p.text(pos2(rect.left() + 8.0, rect.bottom() - 12.0), Align2::LEFT_CENTER, fit(p, &recent.state, &font, rect.width() - 14.0), font, look::MUTED);
            if let Some(done) = recent.progress {
                let bar = Rect::from_min_size(pos2(picture.left(), picture.bottom() - 4.0), vec2(picture.width() * done.clamp(0.0, 1.0), 4.0));
                p.rect_filled(bar, 0.0, theme::ACCENT);
            }
        }
    }
}

/// The words about what is pointed at, or what to do first, in a pill as wide as they are. Nothing to say: nothing shows.
fn draw_caption(p: &Painter, rect: Rect, facts: &Facts) {
    let words = facts.pointed.and_then(|a| caption(a, facts)).or_else(|| facts.hint.map(|(a, b)| (a.to_string(), b.to_string())));
    let Some((first, second)) = words else { return };
    let big = theme::font(13.0, weight::BOLD);
    let small = theme::font(11.0, weight::MEDIUM);
    let room = rect.width() - 24.0;
    let first = fit(p, &first, &big, room);
    let second = fit(p, &second, &small, room);
    let width = p.layout_no_wrap(first.clone(), big.clone(), Color32::WHITE).size().x.max(p.layout_no_wrap(second.clone(), small.clone(), Color32::WHITE).size().x) + 24.0;
    let pill = Rect::from_center_size(rect.center(), vec2(width, rect.height() - 8.0));
    p.rect_filled(pill, theme::radius(look::RADIUS + 2.0), look::PANEL);
    p.rect_stroke(pill, theme::radius(look::RADIUS + 2.0), Stroke::new(1.0, look::EDGE), StrokeKind::Inside);
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
                    Look::Wide { label, .. } => Some(label),
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
                assert!(a.rect.left() >= 0.0 && a.rect.right() <= 1024.0 && a.rect.top() >= 0.0 && a.rect.bottom() <= 576.0, "{:?}", a.rect);
                for b in &list[i + 1..] {
                    assert!(!a.rect.intersects(b.rect), "{:?} {:?}", a.look, b.look);
                }
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
        let size = Zone::QmWing.units().size();
        let vr = Facts { vr_close: true, ..facts() };
        let list = items(Zone::QmWing, size, &vr);
        let close = list.iter().find(|i| i.action == Action::Hide).expect("in VR");
        assert_eq!(list[hit(&list, close.rect.center()).unwrap()].action, Action::Hide);
        assert!(items(Zone::QmWing, size, &facts()).iter().all(|i| i.action != Action::Hide), "the desktop's menu closes by itself");
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
