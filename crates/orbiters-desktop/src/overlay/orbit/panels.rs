//! What each panel holds, in its own units (around the wheel a hundredth of its radius, around the Quick Menu its unit: a pixel of a 1280 x 720 window either way), so that the
//! desktop's windows and the headset's overlays draw and hit the same things. [`items`] says where everything is and [`paint`] draws it: nothing else decides a place, so what
//! the pointer hits is what is drawn.
//!
//! **For someone who has never heard of N4N0.** Each thing says what it does in its own words: the two 3D picture buttons say "3D picture" on themselves, with a 3D badge on their
//! icon and "of people", "of this world" under it, and [`caption`] says in a sentence what is pointed at, in one fixed place. While a picture is on its way the button
//! itself counts down.

use super::zones::{self, Ring, Zone};
use egui::epaint::RectShape;
use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, TextureId, Vec2};
use orbiters_n4n0_core::menu::{PhotoMode, Target};
use orbiters_ui::theme::{self, weight};
use orbiters_ui::{icons, widgets};
use std::path::PathBuf;

/// What pressing something does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// A 3D picture of people, of the world.
    Picture(PhotoMode),
    /// Starts or stops a recording.
    Record,
    /// N4N0's menu in front of the Quick Menu, on this page. `Target::Close`: on its Home.
    Page(Target),
    /// Fast walk on or off.
    FastWalk,
    /// What the computer plays.
    PlayPause,
    /// Talks to N4N0 while held.
    Talk,
    /// The Orbiters window.
    OpenApp,
    /// The panels go for this opening of VRChat's menu.
    Hide,
    /// Nothing, but it is a place the pointer can be on and shows it has no action now.
    Idle,
}

/// What the panels show of a recent capture.
#[derive(Clone, Debug, PartialEq)]
pub struct Recent {
    pub dir: PathBuf,
    pub title: String,
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
    /// The newest captures, at most four.
    pub recent: Vec<Recent>,
    /// What is being reconstructed: its share done (0 to 1), and how many wait after it.
    pub reconstructing: Option<f32>,
    pub waiting: usize,
    /// VRChat in desktop mode: no fast walk.
    pub desktop: bool,
    pub fast_walk: bool,
    /// N4N0 is on: Talk has someone to talk to.
    pub n4n0: bool,
    /// What plays on the computer.
    pub music: Option<(String, bool)>,
    /// A 3D picture's timer, in seconds.
    pub timer: u32,
    /// VRChat's wheel's own colour (RGB), when it was read: the ring takes after it.
    pub tint: Option<[u8; 3]>,
    /// What the caption says with nothing pointed at (a key to hold).
    pub hint: Option<(&'static str, &'static str)>,
    /// What is pointed at.
    pub pointed: Option<Action>,
}

/// Something to point at on a panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub action: Action,
    pub rect: Rect,
    pub look: Look,
}

/// How much a tile says of itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    /// On now (fast walk): filled.
    On,
}

/// What an item is drawn as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// A tile: an icon over a label.
    Tile { icon: &'static str, label: &'static str, tone: Tone },
    /// A 3D picture's button.
    Picture(PhotoMode),
    /// A recent capture's card, the `n`-th.
    Capture(usize),
    /// Record: its icon and state.
    Record,
    /// The status line's text.
    Status,
    /// What plays.
    Music,
    /// Talk.
    Talk,
    /// The small close button.
    Hide,
    /// Words about what is pointed at.
    Caption,
}

const PAD: f32 = 8.0;
const GAP: f32 = 6.0;
/// The Quick Menu's list of N4N0's pages, in the order of the menu's Home.
const PAGES: [(&str, &str, Target); 7] = [
    (icons::YOUTUBE_LOGO, "YouTube", Target::YouTube),
    (icons::PHONE, "Call", Target::Call),
    (icons::MUSIC_NOTES, "Music", Target::Music),
    (icons::LIGHTBULB, "Light", Target::Light),
    (icons::BOOKMARKS_SIMPLE, "States", Target::States),
    (icons::VIRTUAL_REALITY, "Tracking", Target::Tracking),
    (icons::CLOCK, "Captures", Target::History),
];

fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}

fn tile(icon: &'static str, label: &'static str, action: Action, rect: Rect) -> Item {
    Item { action, rect, look: Look::Tile { icon, label, tone: Tone::Plain } }
}

fn capture_item(n: usize, rect: Rect, facts: &Facts) -> Item {
    Item { action: if facts.recent.get(n).is_some() { Action::Page(Target::History) } else { Action::Idle }, rect, look: Look::Capture(n) }
}

/// A tile for fast walk where there is one; in desktop mode YouTube around the wheel, the light under the Quick Menu.
fn walk_or(facts: &Facts, rect: Rect, instead: (&'static str, &'static str, Target)) -> Item {
    if facts.desktop {
        tile(instead.0, instead.1, Action::Page(instead.2), rect)
    } else {
        Item { action: Action::FastWalk, rect, look: Look::Tile { icon: icons::PERSON_SIMPLE_RUN, label: "Fast walk", tone: if facts.fast_walk { Tone::On } else { Tone::Plain } } }
    }
}

/// The items of a panel of `size` units.
pub fn items(zone: Zone, size: Vec2, facts: &Facts) -> Vec<Item> {
    let (w, h) = (size.x, size.y);
    let whole = r(0.0, 0.0, w, h);
    match zone {
        // The ring's cells are one thing each, its shape cut by the platform.
        Zone::WheelPeople => vec![Item { action: Action::Picture(PhotoMode::People), rect: whole, look: Look::Picture(PhotoMode::People) }],
        Zone::WheelWorld => vec![Item { action: Action::Picture(PhotoMode::World), rect: whole, look: Look::Picture(PhotoMode::World) }],
        Zone::WheelRecord => vec![Item { action: Action::Record, rect: whole, look: Look::Record }],
        Zone::WheelWalk => vec![walk_or(facts, whole, (icons::YOUTUBE_LOGO, "YouTube", Target::YouTube))],
        Zone::WheelMusic => vec![Item { action: Action::PlayPause, rect: whole, look: Look::Music }],
        Zone::WheelTalk => vec![Item { action: Action::Talk, rect: whole, look: Look::Talk }],
        Zone::WheelMore => vec![tile(icons::N4N0_MARK, "More", Action::Page(Target::Close), whole)],
        Zone::WheelRecent0 => vec![capture_item(0, whole, facts)],
        Zone::WheelRecent1 => vec![capture_item(1, whole, facts)],
        Zone::WheelRecent2 => vec![capture_item(2, whole, facts)],
        Zone::WheelCaption | Zone::QmCaption => vec![Item { action: Action::Idle, rect: whole, look: Look::Caption }],
        Zone::WheelHide => vec![Item { action: Action::Hide, rect: whole, look: Look::Hide }],
        Zone::QmLeft => captures(r(PAD, 26.0, w - PAD * 2.0, h - 26.0 - PAD), 4, facts),
        Zone::QmRight => {
            let y0 = 26.0;
            let row = (h - y0 - PAD - GAP * (PAGES.len() as f32 - 1.0)) / PAGES.len() as f32;
            PAGES.iter().enumerate().map(|(i, (icon, label, target))| tile(icon, label, Action::Page(*target), r(PAD, y0 + i as f32 * (row + GAP), w - PAD * 2.0, row))).collect()
        }
        Zone::QmAbove => {
            let close = 26.0;
            vec![
                Item { action: Action::Record, rect: r(4.0, 4.0, 86.0, h - 8.0), look: Look::Record },
                Item { action: Action::Idle, rect: r(94.0, 4.0, w - 94.0 - close - 12.0, h - 8.0), look: Look::Status },
                Item { action: Action::Hide, rect: r(w - close - 8.0, (h - close) / 2.0, close, close), look: Look::Hide },
            ]
        }
        Zone::QmLowerLeft => vec![Item { action: Action::Picture(PhotoMode::People), rect: whole, look: Look::Picture(PhotoMode::People) }],
        Zone::QmLowerRight => vec![Item { action: Action::Picture(PhotoMode::World), rect: whole, look: Look::Picture(PhotoMode::World) }],
        Zone::QmUnder => {
            let width = (w - 8.0 - GAP * 3.0) / 4.0;
            let at = |i: usize| r(4.0 + i as f32 * (width + GAP), 4.0, width, h - 8.0);
            vec![
                Item { action: Action::Talk, rect: at(0), look: Look::Talk },
                walk_or(facts, at(1), (icons::LIGHTBULB, "Light", Target::Light)),
                Item { action: Action::PlayPause, rect: at(2), look: Look::Music },
                tile(icons::N4N0_MARK, "Orbiters", Action::OpenApp, at(3)),
            ]
        }
        // N4N0's own menu is drawn there, its presses its own.
        Zone::Takeover => Vec::new(),
    }
}

/// Cards of the newest captures, as many as `most` and the height allow, and an empty panel's one tile.
fn captures(area: Rect, most: usize, facts: &Facts) -> Vec<Item> {
    if facts.recent.is_empty() {
        return vec![Item { action: Action::Idle, rect: area, look: Look::Capture(usize::MAX) }];
    }
    let least = (area.width() * 0.46 + 28.0).max(60.0);
    let room = (((area.height() + GAP) / (least + GAP)) as usize).max(1);
    let count = most.min(facts.recent.len()).min(room);
    // As many as the panel holds, sharing all of it; with few captures they keep their size.
    let card = if count == room { (area.height() - GAP * (count as f32 - 1.0)) / count as f32 } else { least };
    (0..count).map(|i| capture_item(i, Rect::from_min_size(area.min + vec2(0.0, i as f32 * (card + GAP)), vec2(area.width(), card)), facts)).collect()
}

/// What is on the item at `p` (panel units): its index. The small close button lies on top of what it sits on.
pub fn hit(items: &[Item], p: Pos2) -> Option<usize> {
    items.iter().enumerate().rev().find(|(_, i)| i.rect.contains(p)).map(|(i, _)| i)
}

/// What the caption says of what is pointed at, in words for someone who has never seen any of this: a title and what it does.
pub fn caption(action: Action, facts: &Facts) -> Option<(String, String)> {
    let (a, b): (String, String) = match action {
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
        Action::Record if facts.recording => ("Stop recording".into(), "Orbiters then turns it into a 3D scene.".into()),
        Action::Record => ("Record a 3D scene".into(), "Walk around with VRChat's camera; Orbiters makes a 3D scene of what it saw.".into()),
        Action::Page(Target::Close) => ("More".into(), "Everything in N4N0's menu: calls, light, saved states, your captures.".into()),
        Action::Page(Target::History) => ("Your captures".into(), "Your 3D pictures and scenes: see them, share them.".into()),
        Action::Page(Target::YouTube) => ("YouTube".into(), "Find a video, and put its link in the world's video player.".into()),
        Action::Page(Target::Call) => ("Call".into(), "Talk with a friend, wherever they are in VRChat.".into()),
        Action::Page(Target::Music) => ("Music".into(), "What your computer plays, and its volume.".into()),
        Action::Page(Target::Light) => ("Light".into(), "N4N0 as a light that follows you.".into()),
        Action::Page(Target::States) => ("States".into(), "Your avatar's saved looks, applied in one go.".into()),
        Action::Page(Target::Tracking) => ("Tracking".into(), "Smooths your headset, controllers and trackers.".into()),
        Action::Page(_) => return None,
        Action::FastWalk => ("Fast walk".into(), if facts.fast_walk { "On: you walk faster. Press to turn it off.".into() } else { "Walk faster than VRChat lets you.".to_string() }),
        Action::PlayPause => match &facts.music {
            Some((title, playing)) => (title.clone(), if *playing { "Playing on your computer. Press to pause.".into() } else { "Paused on your computer. Press to play.".to_string() }),
            None => ("No music".into(), "Nothing is playing on your computer.".into()),
        },
        Action::Talk if facts.n4n0 => ("Talk to N4N0".into(), "Hold and speak: N4N0 is your assistant in VRChat.".into()),
        Action::Talk => ("Talk to N4N0".into(), "Turn N4N0 on in Orbiters first.".into()),
        Action::OpenApp => ("Orbiters".into(), "Opens Orbiters on your computer.".into()),
        Action::Hide => ("Hide".into(), "Hides these until VRChat's menu opens again.".into()),
        Action::Idle => return None,
    };
    Some((a, b))
}

/// The cells' and panels' fill: opaque enough to read over any world.
pub const FILL: Color32 = Color32::from_rgba_premultiplied(15, 15, 17, 244);
const TILE: Color32 = theme::INK_800;
const TILE_HOVER: Color32 = theme::INK_700;

fn text(p: &Painter, at: Pos2, align: Align2, s: &str, size: f32, w: f32, c: Color32) {
    p.text(at, align, s, theme::font(size, w), c);
}

/// A title in the panel's corner.
fn title(p: &Painter, at: Pos2, s: &str) {
    text(p, at, Align2::LEFT_CENTER, s, 12.0, weight::SEMIBOLD, theme::FG_2);
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

/// An icon over its label, the two together around `at`; a label too long is drawn smaller, then cut.
#[allow(clippy::too_many_arguments)]
fn icon_over_label(p: &Painter, at: Pos2, room: f32, height: f32, icon: &str, label: &str, icon_color: Color32, label_color: Color32) {
    let icon_size = (height * 0.27).min(room * 0.5).clamp(14.0, 40.0);
    let mut size = (height * 0.15).clamp(10.0, 13.0);
    let wide = |size: f32| p.layout_no_wrap(label.to_string(), theme::font(size, weight::MEDIUM), Color32::WHITE).size().x;
    while size > 8.0 && wide(size) > room {
        size -= 0.5;
    }
    let total = icon_size + 3.0 + size;
    let top = at.y - total / 2.0;
    widgets::icon(p, pos2(at.x, top + icon_size / 2.0), icon, icon_size, icon_color);
    let font = theme::font(size, weight::MEDIUM);
    let fitted = fit(p, label, &font, room);
    p.text(pos2(at.x, top + icon_size + 3.0 + size / 2.0), Align2::CENTER_CENTER, fitted, font, label_color);
}

/// The time in a recording's or a countdown's form.
fn clock(seconds: u32) -> String {
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

/// What the status line says.
pub fn status(facts: &Facts) -> (String, Color32) {
    if let Some(s) = facts.countdown {
        return (format!("3D picture in {s} s"), theme::ACCENT);
    }
    if facts.picture_flying {
        return ("3D picture in progress".into(), theme::ACCENT);
    }
    if facts.recording {
        return (format!("Recording {}", clock(facts.recording_seconds.unwrap_or(0))), theme::REC);
    }
    if let Some(done) = facts.reconstructing {
        let more = if facts.waiting > 0 { format!(", {} waiting", facts.waiting) } else { String::new() };
        return (format!("Making 3D {:.0}%{more}", done * 100.0), theme::FG);
    }
    if facts.waiting > 0 {
        return (format!("{} waiting to become 3D", facts.waiting), theme::FG_2);
    }
    ("Nothing running".into(), theme::FG_3)
}

/// The shape something is drawn into: a rectangle, or a cell of the ring (where to centre, how wide it is there, and where the wheel's centre lies from the panel's corner).
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
            // A panel's corner is its bounds' corner.
            let center = (-ring.bounds().min.to_vec2()).to_pos2();
            Shaped { at: center + ring.anchor(), room: ring.room(), height: ring.outer - ring.inner, ring: Some((ring, center)) }
        }
        None => Shaped { at: rect.center(), room: rect.width() - 8.0, height: rect.height(), ring: None },
    }
}

/// An arc along a cell's outer edge, for what is under way: `fraction` of the way across it.
fn progress_arc(p: &Painter, ring: Ring, center: Pos2, fraction: f32, color: Color32) {
    let radius = ring.outer - 5.0;
    let (from, span) = (ring.from + 2.0, (ring.to - ring.from - 4.0) * fraction.clamp(0.0, 1.0));
    let steps = 24;
    let line = |to: f32| (0..=steps).map(|i| center + zones::polar(radius, from + to * i as f32 / steps as f32)).collect::<Vec<Pos2>>();
    p.add(Shape::line(line(ring.to - ring.from - 4.0), Stroke::new(2.5, theme::white(0.08))));
    if span > 0.0 {
        p.add(Shape::line(line(span), Stroke::new(2.5, color)));
    }
}

/// Draws a panel (in units, its corner at the painter's origin): its fill and everything on it.
pub fn paint(p: &Painter, zone: Zone, size: Vec2, facts: &Facts, hover: Option<usize>, pressed: Option<usize>) {
    let panel = Rect::from_min_size(pos2(0.0, 0.0), size);
    if zone.ring().is_some() {
        // Filled to its corners: the platform cuts the cell's shape.
        p.rect_filled(panel, 0.0, cell_fill(facts));
    } else if !zone.passes_clicks() {
        let single = matches!(zone, Zone::QmLowerLeft | Zone::QmLowerRight);
        p.rect_filled(panel, theme::radius(if single { theme::TILE_RADIUS * 0.8 } else { theme::CARD_RADIUS * 0.8 }), FILL);
    }
    match zone {
        Zone::QmLeft => title(p, pos2(PAD + 4.0, 14.0), "Your captures"),
        Zone::QmRight => title(p, pos2(PAD + 4.0, 14.0), "N4N0"),
        _ => {}
    }
    for (i, item) in items(zone, size, facts).iter().enumerate() {
        draw(p, zone, item, facts, hover == Some(i), pressed == Some(i));
    }
}

/// A ring's cell at rest: dark, with a little of VRChat's wheel's own colour, so that it reads as more of the wheel.
fn cell_fill(facts: &Facts) -> Color32 {
    match facts.tint {
        Some([r, g, b]) => theme::mix(theme::INK_900, Color32::from_rgb(r, g, b), 0.42),
        None => theme::INK_900,
    }
}

fn state_fill(base: Color32, hovered: bool, pressed: bool) -> Color32 {
    if pressed {
        theme::mix(base, Color32::WHITE, 0.22)
    } else if hovered {
        theme::mix(base, Color32::WHITE, 0.13)
    } else {
        base
    }
}

fn draw(p: &Painter, zone: Zone, item: &Item, facts: &Facts, hovered: bool, pressed: bool) {
    let rect = item.rect;
    let corner = theme::TILE_RADIUS * 0.8;
    let shape = shaped(zone, rect);
    let on_ring = shape.ring.is_some();
    // The ring's cells fill wholly (cut after); tiles in a panel are rounded.
    let fill = |p: &Painter, c: Color32| {
        p.rect_filled(rect, theme::radius(if on_ring { 0.0 } else { corner }), c);
    };
    let plain = if on_ring { cell_fill(facts) } else { TILE };
    match item.look {
        Look::Tile { icon, label, tone } => {
            let on = tone == Tone::On;
            fill(p, if on { if hovered { theme::ACCENT_HOVER } else { theme::ACCENT } } else { state_fill(plain, hovered, pressed) });
            let color = if on { theme::ON_ACCENT } else { theme::FG };
            if !on_ring && rect.width() > rect.height() * 2.2 {
                // A long row: the icon then its label.
                widgets::icon(p, pos2(rect.left() + rect.height() * 0.45, rect.center().y), icon, rect.height() * 0.5, color);
                text(p, pos2(rect.left() + rect.height() * 0.86, rect.center().y), Align2::LEFT_CENTER, label, 11.0, weight::MEDIUM, color);
            } else {
                icon_over_label(p, shape.at, shape.room, shape.height, icon, label, color, color);
            }
        }
        Look::Picture(mode) => picture_button(p, shape, rect, mode, facts, hovered, pressed, plain, &fill),
        Look::Record => {
            let (base, label) = if facts.recording { (theme::mix(theme::REC, theme::INK_900, 0.25), "Stop") } else { (plain, "Record") };
            fill(p, if facts.recording && hovered { theme::REC } else { state_fill(base, hovered, pressed) });
            let dim = facts.capture_busy && !facts.recording;
            let color = if dim { theme::FG_4 } else { theme::FG };
            if !on_ring && rect.width() > rect.height() * 1.3 {
                let at = pos2(rect.left() + 16.0, rect.center().y);
                if facts.recording {
                    p.rect_filled(Rect::from_center_size(at, Vec2::splat(11.0)), theme::radius(2.0), color);
                } else {
                    p.circle_filled(at, 6.5, if dim { theme::FG_4 } else { theme::REC });
                }
                text(p, pos2(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER, label, 11.5, weight::MEDIUM, color);
            } else {
                // The dot where an icon would be, the time under it while it records.
                let top = shape.at.y - 10.0;
                if facts.recording {
                    p.rect_filled(Rect::from_center_size(pos2(shape.at.x, top), Vec2::splat(17.0)), theme::radius(3.0), color);
                } else {
                    p.circle_filled(pos2(shape.at.x, top), 10.0, if dim { theme::FG_4 } else { theme::REC });
                }
                let label = facts.recording_seconds.filter(|_| facts.recording).map_or(label.to_string(), clock);
                text(p, pos2(shape.at.x, top + 21.0), Align2::CENTER_CENTER, &label, 12.0, weight::MEDIUM, color);
            }
        }
        Look::Status => {
            let (s, color) = status(facts);
            let font = theme::font(11.0, weight::MEDIUM);
            let cut = fit(p, &s, &font, rect.width() - 10.0);
            p.text(pos2(rect.left() + 4.0, rect.center().y), Align2::LEFT_CENTER, cut, font, color);
            if let Some(done) = facts.reconstructing {
                let bar = Rect::from_min_size(pos2(rect.left() + 4.0, rect.bottom() - 7.0), vec2(rect.width() - 8.0, 3.0));
                p.rect_filled(bar, theme::radius(1.5), theme::INK_700);
                p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * done.clamp(0.0, 1.0), 3.0)), theme::radius(1.5), theme::ACCENT);
            }
        }
        Look::Music => {
            fill(p, state_fill(plain, hovered && facts.music.is_some(), pressed && facts.music.is_some()));
            let (name, playing) = facts.music.clone().unwrap_or_default();
            let none = facts.music.is_none();
            let (icon_color, label_color) = if none { (theme::FG_4, theme::FG_4) } else { (theme::FG, theme::FG_2) };
            let glyph = if playing { icons::PAUSE } else if none { icons::MUSIC_NOTES } else { icons::PLAY };
            icon_over_label(p, shape.at, shape.room, shape.height, glyph, if none { "No music" } else { &name }, icon_color, label_color);
        }
        Look::Talk => {
            fill(p, if pressed && facts.n4n0 { theme::ACCENT } else { state_fill(plain, hovered, false) });
            let color = if pressed && facts.n4n0 { theme::ON_ACCENT } else if facts.n4n0 { theme::FG } else { theme::FG_4 };
            let label = if pressed && facts.n4n0 { "Listening" } else { "Talk" };
            icon_over_label(p, shape.at, shape.room, shape.height, icons::MICROPHONE, label, color, color);
        }
        Look::Hide => {
            p.circle_filled(rect.center(), rect.width() / 2.0, if hovered { TILE_HOVER } else { theme::INK_850 });
            widgets::icon(p, rect.center(), icons::X, rect.width() * 0.55, theme::FG_2);
        }
        Look::Caption => draw_caption(p, rect, facts),
        Look::Capture(n) => draw_capture(p, shape, rect, n, facts, hovered, pressed, plain, &fill),
    }
}

/// A 3D picture's button: says "3D picture" on itself, a 3D badge on its icon, and what of. Under way, it is the countdown.
#[allow(clippy::too_many_arguments)]
fn picture_button(p: &Painter, shape: Shaped, rect: Rect, mode: PhotoMode, facts: &Facts, hovered: bool, pressed: bool, plain: Color32, fill: &dyn Fn(&Painter, Color32)) {
    let mine = facts.picture_mode == Some(mode);
    let busy = facts.countdown.is_some() || facts.picture_flying;
    let lit = (hovered && !busy) || pressed || (mine && busy);
    let base = if lit {
        if pressed {
            theme::mix(theme::ACCENT, theme::INK_900, 0.2)
        } else if hovered {
            theme::ACCENT_HOVER
        } else {
            theme::ACCENT
        }
    } else {
        plain
    };
    fill(p, base);
    let dim = busy && !mine;
    let (ink, soft) = if lit {
        (theme::ON_ACCENT, theme::ON_ACCENT)
    } else if dim {
        (theme::FG_4, theme::FG_4)
    } else {
        (theme::ACCENT, theme::FG)
    };
    let at = shape.at;
    // Under way, its own number takes the middle.
    if mine {
        if let Some(s) = facts.countdown {
            text(p, at - vec2(0.0, 9.0), Align2::CENTER_CENTER, &s.to_string(), 34.0, weight::BOLD, ink);
            text(p, at + vec2(0.0, 19.0), Align2::CENTER_CENTER, "3D picture in", 11.0, weight::MEDIUM, ink);
            return;
        }
        if facts.picture_flying {
            widgets::icon(p, at - vec2(0.0, 11.0), icons::CUBE, 30.0, ink);
            text(p, at + vec2(0.0, 16.0), Align2::CENTER_CENTER, "Flying…", 12.0, weight::SEMIBOLD, ink);
            return;
        }
    }
    let (glyph, of) = if mode == PhotoMode::People { (icons::USERS, "of people") } else { (icons::GLOBE_HEMISPHERE_WEST, "of this world") };
    let icon_size = (shape.height * 0.34).clamp(22.0, 46.0);
    let line = 15.0;
    let top = at.y - (icon_size + 5.0 + line * 2.0) / 2.0;
    let icon_at = pos2(at.x, top + icon_size / 2.0);
    widgets::icon(p, icon_at, glyph, icon_size, ink);
    // A little "3D" on the icon's corner.
    let badge = Rect::from_center_size(icon_at + vec2(icon_size * 0.56, -icon_size * 0.34), vec2(20.0, 13.0));
    p.rect_filled(badge, theme::radius(4.0), if lit { theme::ON_ACCENT } else if dim { theme::INK_600 } else { theme::ACCENT });
    text(p, badge.center(), Align2::CENTER_CENTER, "3D", 9.0, weight::BOLD, if lit { theme::ACCENT } else { theme::ON_ACCENT });
    let room = shape.room.min(rect.width() - 8.0);
    let font = theme::font(13.0, weight::SEMIBOLD);
    let words = fit(p, "3D picture", &font, room);
    p.text(pos2(at.x, top + icon_size + 5.0 + line / 2.0), Align2::CENTER_CENTER, words, font, soft);
    let font = theme::font(12.0, weight::REGULAR);
    let words = fit(p, of, &font, room);
    p.text(pos2(at.x, top + icon_size + 5.0 + line * 1.5), Align2::CENTER_CENTER, words, font, if lit { soft } else { theme::FG_2 });
}

/// A recent capture: its picture filling its cell or card, its name over a dark band.
#[allow(clippy::too_many_arguments)]
fn draw_capture(p: &Painter, shape: Shaped, rect: Rect, n: usize, facts: &Facts, hovered: bool, pressed: bool, plain: Color32, fill: &dyn Fn(&Painter, Color32)) {
    let radius = theme::radius(if shape.ring.is_some() { 0.0 } else { theme::TILE_RADIUS * 0.7 });
    let Some(recent) = facts.recent.get(n) else {
        fill(p, plain);
        // With nothing yet, the cell nearest the 3D pictures says where they will be.
        if n == 0 || n == usize::MAX {
            icon_over_label(p, shape.at, shape.room, shape.height, icons::CUBE, "No captures yet", theme::FG_4, theme::FG_3);
        }
        return;
    };
    fill(p, plain);
    match shape.ring {
        // On the ring the picture fills the whole cell, as the wheel's own tiles do.
        Some((ring, center)) => {
            if let Some(id) = recent.picture {
                p.add(RectShape::filled(rect, radius, tint_for(hovered, pressed)).with_texture(id, cover_uv(rect.size())));
            } else {
                widgets::icon(p, shape.at - vec2(0.0, 14.0), icons::CUBE, 26.0, theme::FG_4);
            }
            let band = Rect::from_center_size(shape.at + vec2(0.0, 22.0), vec2(rect.width(), 30.0));
            p.rect_filled(band, 0.0, theme::black(0.62));
            let font = theme::font(10.5, weight::SEMIBOLD);
            let name = fit(p, &recent.title, &font, shape.room);
            p.text(band.center() - vec2(0.0, 6.5), Align2::CENTER_CENTER, name, font, theme::FG);
            let font = theme::font(9.0, weight::REGULAR);
            let state = fit(p, &recent.state, &font, shape.room);
            p.text(band.center() + vec2(0.0, 6.5), Align2::CENTER_CENTER, state, font, theme::FG_2);
            if let Some(done) = recent.progress {
                progress_arc(p, ring, center, done, theme::ACCENT);
            }
        }
        None => {
            let picture = Rect::from_min_size(rect.min, vec2(rect.width(), (rect.height() - 28.0).max(16.0)));
            match recent.picture {
                Some(id) => {
                    p.add(RectShape::filled(picture, radius, Color32::WHITE).with_texture(id, cover_uv(picture.size())));
                }
                None => {
                    p.rect_filled(picture, radius, theme::INK_700);
                    widgets::icon(p, picture.center(), icons::CUBE, picture.height() * 0.45, theme::FG_4);
                }
            }
            let font = theme::font(10.0, weight::MEDIUM);
            let title = fit(p, &recent.title, &font, rect.width() - 10.0);
            p.text(pos2(rect.left() + 5.0, rect.bottom() - 20.0), Align2::LEFT_CENTER, title, font, theme::FG);
            let font = theme::font(9.0, weight::REGULAR);
            let state = fit(p, &recent.state, &font, rect.width() - 10.0);
            p.text(pos2(rect.left() + 5.0, rect.bottom() - 8.0), Align2::LEFT_CENTER, state, font, theme::FG_3);
            if hovered {
                p.rect_stroke(rect, radius, Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
            }
        }
    }
}

/// How bright a picture shows: a little dimmed at rest, so that the pointer on it lights it.
fn tint_for(hovered: bool, pressed: bool) -> Color32 {
    let v = if pressed {
        170
    } else if hovered {
        255
    } else {
        205
    };
    Color32::from_rgb(v, v, v)
}

/// The words about what is pointed at, or what to do first, in a pill as wide as they are. Nothing to say: nothing shows.
fn draw_caption(p: &Painter, rect: Rect, facts: &Facts) {
    let words = facts.pointed.and_then(|a| caption(a, facts)).or_else(|| facts.hint.map(|(a, b)| (a.to_string(), b.to_string())));
    let Some((first, second)) = words else { return };
    let big = theme::font(13.0, weight::SEMIBOLD);
    let small = theme::font(11.0, weight::REGULAR);
    let room = rect.width() - 24.0;
    let first = fit(p, &first, &big, room);
    let second = fit(p, &second, &small, room);
    let width = p.layout_no_wrap(first.clone(), big.clone(), Color32::WHITE).size().x.max(p.layout_no_wrap(second.clone(), small.clone(), Color32::WHITE).size().x) + 24.0;
    let pill = Rect::from_center_size(rect.center(), vec2(width, rect.height() - 8.0));
    p.rect_filled(pill, theme::radius(theme::CARD_RADIUS * 0.8), FILL);
    p.text(pill.center() - vec2(0.0, 9.0), Align2::CENTER_CENTER, first, big, theme::FG);
    p.text(pill.center() + vec2(0.0, 9.0), Align2::CENTER_CENTER, second, small, theme::FG_2);
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
            recent: (0..4).map(|i| Recent { dir: PathBuf::from(format!("s{i}")), title: format!("World {i}"), state: "3D view".into(), progress: None, picture: None }).collect(),
            timer: 3,
            n4n0: true,
            ..Default::default()
        }
    }

    #[test]
    fn everything_lies_inside_its_panel() {
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
            // Around its anchor, as wide as `room`.
            let anchor = ring.anchor();
            let (s, c) = ring.middle().to_radians().sin_cos();
            let across = vec2(c, s) * (ring.room() / 2.0);
            assert!(ring.contains(anchor), "{zone:?}");
            assert!(ring.contains(anchor + across * 0.98) && ring.contains(anchor - across * 0.98), "{zone:?}: {}", ring.room());
            assert!(ring.room() > 60.0, "{zone:?} has room for a word: {}", ring.room());
        }
    }

    #[test]
    fn every_action_is_within_reach_of_each_menu() {
        let around = |list: &[Zone]| -> Vec<Action> { list.iter().flat_map(|z| items(*z, z.units().size(), &facts())).map(|i| i.action).collect() };
        let wheel = around(&Zone::WHEEL);
        for needed in [Action::Picture(PhotoMode::People), Action::Picture(PhotoMode::World), Action::Record, Action::PlayPause, Action::Talk, Action::Page(Target::Close), Action::Page(Target::History)] {
            assert!(wheel.contains(&needed), "the wheel: {needed:?}");
        }
        let quick = around(&Zone::QUICK_MENU);
        for needed in [Action::Picture(PhotoMode::People), Action::Picture(PhotoMode::World), Action::Record, Action::PlayPause, Action::Talk, Action::OpenApp, Action::Hide, Action::Page(Target::YouTube), Action::Page(Target::History)] {
            assert!(quick.contains(&needed), "the Quick Menu: {needed:?}");
        }
    }

    #[test]
    fn each_3d_picture_button_says_what_it_is_for_someone_new() {
        let (title, line) = caption(Action::Picture(PhotoMode::People), &facts()).unwrap();
        assert!(title.contains("3D picture") && title.contains("people"), "{title}");
        assert!(line.contains("3D"), "{line}");
        let (title, _) = caption(Action::Picture(PhotoMode::World), &facts()).unwrap();
        assert!(title.contains("3D picture") && title.contains("world"), "{title}");
        // While one is coming it says when.
        let coming = Facts { countdown: Some(2), picture_mode: Some(PhotoMode::World), ..facts() };
        assert_eq!(caption(Action::Picture(PhotoMode::World), &coming).unwrap().0, "3D picture in 2");
    }

    #[test]
    fn everything_that_does_something_says_what_in_a_caption() {
        for list in [&Zone::WHEEL[..], &Zone::QUICK_MENU[..]] {
            for zone in list {
                for item in items(*zone, zone.units().size(), &facts()) {
                    if item.action != Action::Idle {
                        assert!(caption(item.action, &facts()).is_some(), "{:?}", item.action);
                    }
                }
            }
        }
    }

    #[test]
    fn a_capture_cell_only_presses_when_there_is_a_capture() {
        let size = Zone::WheelRecent2.units().size();
        assert_eq!(items(Zone::WheelRecent2, size, &facts())[0].action, Action::Page(Target::History));
        assert_eq!(items(Zone::WheelRecent2, size, &Facts::default())[0].action, Action::Idle);
    }

    #[test]
    fn the_close_button_is_hit_before_what_it_sits_on() {
        let size = Zone::QmAbove.units().size();
        let list = items(Zone::QmAbove, size, &facts());
        assert_eq!(list[hit(&list, list[2].rect.center()).unwrap()].action, Action::Hide);
        assert_eq!(list[hit(&list, list[0].rect.center()).unwrap()].action, Action::Record);
        assert_eq!(hit(&list, pos2(-5.0, -5.0)), None);
    }

    #[test]
    fn fast_walk_is_not_offered_where_it_does_nothing() {
        let size = Zone::WheelWalk.units().size();
        let desktop = Facts { desktop: true, ..facts() };
        assert_eq!(items(Zone::WheelWalk, size, &desktop)[0].action, Action::Page(Target::YouTube));
        assert_eq!(items(Zone::WheelWalk, size, &facts())[0].action, Action::FastWalk);
    }

    #[test]
    fn the_status_says_the_most_urgent_thing() {
        assert_eq!(status(&Facts { countdown: Some(2), recording: true, ..Default::default() }).0, "3D picture in 2 s");
        assert_eq!(status(&Facts { recording: true, recording_seconds: Some(75), reconstructing: Some(0.5), ..Default::default() }).0, "Recording 01:15");
        assert_eq!(status(&Facts { reconstructing: Some(0.42), waiting: 2, ..Default::default() }).0, "Making 3D 42%, 2 waiting");
        assert_eq!(status(&Facts::default()).0, "Nothing running");
    }

    #[test]
    fn the_mask_cuts_a_cell_s_shape_out_of_its_rectangle() {
        let ring = Zone::WheelRecord.ring().unwrap();
        let size = ring.bounds().size();
        let px = [size.x.round() as u32, size.y.round() as u32];
        let mask = cell_mask(ring, px, 1.0);
        let at = |x: u32, y: u32| mask[(y * px[0] + x) as usize];
        // Its middle is whole; somewhere in its rectangle's corners nothing is left.
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
