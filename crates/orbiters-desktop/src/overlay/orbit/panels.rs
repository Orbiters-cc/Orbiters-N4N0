//! What each panel holds, in its own units (around the wheel a hundredth of its radius, around the Quick Menu its unit: a pixel of a
//! 1280 x 720 window either way), so that the desktop's windows and the headset's overlays draw and hit the same things. Large zones hold
//! what needs room (3D pictures, recent captures, N4N0's pages), small ones one action each. [`items`] says where everything is and
//! [`paint`] draws it: nothing else decides a place, so what the pointer hits is what is drawn.

use super::zones::Zone;
use egui::epaint::RectShape;
use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, TextureId, Vec2};
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
    /// N4N0's menu in front of the Quick Menu, on this page.
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
    /// What the panel is there for: its icon in the accent, filled with it under the pointer. A filled accent at rest would shout over the game's own menu.
    Primary,
    /// On now (fast walk): filled.
    On,
}

/// What an item is drawn as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// A tile: an icon over a label.
    Tile { icon: &'static str, label: &'static str, tone: Tone },
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
}

const PAD: f32 = 8.0;
const GAP: f32 = 6.0;
/// N4N0's pages, in the order of the menu's Home.
const PAGES: [(&str, &str, Target); 7] = [
    (icons::YOUTUBE_LOGO, "YouTube", Target::YouTube),
    (icons::PHONE, "Call", Target::Call),
    (icons::MUSIC_NOTES, "Music", Target::Music),
    (icons::LIGHTBULB, "Light", Target::Light),
    (icons::BOOKMARKS_SIMPLE, "States", Target::States),
    (icons::VIRTUAL_REALITY, "Tracking", Target::Tracking),
    (icons::CLOCK, "Captures", Target::History),
];

/// Around the wheel there is room for six: tracking stays in the Quick Menu's longer list.
const WHEEL_PAGES: [(&str, &str, Target); 6] = [PAGES[0], PAGES[1], PAGES[2], PAGES[3], PAGES[4], PAGES[6]];
/// Where the first row of the lower right panel starts, past the wheel's bite.
const BESIDE_BITE: f32 = 52.0;

fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(x, y), vec2(w, h))
}

fn tile(icon: &'static str, label: &'static str, action: Action, rect: Rect) -> Item {
    Item { action, rect, look: Look::Tile { icon, label, tone: Tone::Plain } }
}

fn capture_action() -> Action {
    Action::Page(Target::History)
}

/// A tile for where fast walk would be: in desktop mode (no fast walk in it) the light.
fn walk_or_light(facts: &Facts, rect: Rect) -> Item {
    if facts.desktop {
        tile(icons::LIGHTBULB, "Light", Action::Page(Target::Light), rect)
    } else {
        Item { action: Action::FastWalk, rect, look: Look::Tile { icon: icons::PERSON_SIMPLE_RUN, label: "Fast walk", tone: if facts.fast_walk { Tone::On } else { Tone::Plain } } }
    }
}

/// The items of a panel of `size` units.
pub fn items(zone: Zone, size: Vec2, facts: &Facts) -> Vec<Item> {
    let (w, h) = (size.x, size.y);
    let whole = r(0.0, 0.0, w, h);
    match zone {
        // Two big buttons in the part the wheel leaves: the panel's edge bends round the wheel under them.
        Zone::WheelTop => {
            let title = 26.0;
            let bottom = (h - 34.0).max(title + 40.0);
            let width = (w - PAD * 2.0 - GAP) / 2.0;
            vec![
                Item { action: Action::Picture(PhotoMode::People), rect: r(PAD, title, width, bottom - title), look: Look::Tile { icon: icons::USERS, label: "People", tone: Tone::Primary } },
                Item { action: Action::Picture(PhotoMode::World), rect: r(PAD + width + GAP, title, width, bottom - title), look: Look::Tile { icon: icons::GLOBE_HEMISPHERE_WEST, label: "World", tone: Tone::Primary } },
            ]
        }
        // Cards down the panel's left, clear of the wheel's bite on its right.
        Zone::WheelLeft => captures(r(PAD, 26.0, (w - 40.0 - PAD).max(40.0), h - 26.0 - PAD), 3, facts),
        Zone::QmLeft => captures(r(PAD, 26.0, w - PAD * 2.0, h - 26.0 - PAD), 4, facts),
        // The wheel's bite takes the panel's top left: three narrower tiles beside it, three wider under them.
        Zone::WheelLowerRight => {
            let y0 = 28.0;
            let row = (h - y0 - GAP - PAD) / 2.0;
            let (left, narrow) = (BESIDE_BITE, (w - BESIDE_BITE - PAD - GAP * 2.0) / 3.0);
            let wide = (w - PAD * 2.0 - GAP * 2.0) / 3.0;
            WHEEL_PAGES
                .iter()
                .enumerate()
                .map(|(i, (icon, label, target))| {
                    let rect = if i < 3 { r(left + i as f32 * (narrow + GAP), y0, narrow, row) } else { r(PAD + (i - 3) as f32 * (wide + GAP), y0 + row + GAP, wide, row) };
                    tile(icon, label, Action::Page(*target), rect)
                })
                .collect()
        }
        Zone::QmRight => {
            let y0 = 26.0;
            let row = (h - y0 - PAD - GAP * (PAGES.len() as f32 - 1.0)) / PAGES.len() as f32;
            PAGES.iter().enumerate().map(|(i, (icon, label, target))| tile(icon, label, Action::Page(*target), r(PAD, y0 + i as f32 * (row + GAP), w - PAD * 2.0, row))).collect()
        }
        // One action: the panel is its tile, edge to edge.
        Zone::WheelTopCorner => vec![Item { action: Action::Record, rect: whole, look: Look::Record }],
        Zone::WheelRightCorner => vec![tile(icons::N4N0_MARK, "Orbiters", Action::OpenApp, whole)],
        Zone::WheelRight => vec![walk_or_light(facts, whole)],
        Zone::WheelBelow => vec![Item { action: Action::PlayPause, rect: whole, look: Look::Music }],
        Zone::WheelBelowLeft => vec![Item { action: Action::Talk, rect: whole, look: Look::Talk }],
        // What Orbiters is doing, and the panels' close button.
        Zone::WheelFarLeft => {
            let close = 22.0;
            vec![Item { action: Action::Idle, rect: whole, look: Look::Status }, Item { action: Action::Hide, rect: r(w - close - 5.0, 5.0, close, close), look: Look::Hide }]
        }
        Zone::QmAbove => {
            let close = 26.0;
            vec![
                Item { action: Action::Record, rect: r(4.0, 4.0, 86.0, h - 8.0), look: Look::Record },
                Item { action: Action::Idle, rect: r(94.0, 4.0, w - 94.0 - close - 12.0, h - 8.0), look: Look::Status },
                Item { action: Action::Hide, rect: r(w - close - 8.0, (h - close) / 2.0, close, close), look: Look::Hide },
            ]
        }
        Zone::QmLowerLeft => vec![Item { action: Action::Picture(PhotoMode::People), rect: whole, look: Look::Tile { icon: icons::USERS, label: "3D people", tone: Tone::Primary } }],
        Zone::QmLowerRight => {
            vec![Item { action: Action::Picture(PhotoMode::World), rect: whole, look: Look::Tile { icon: icons::GLOBE_HEMISPHERE_WEST, label: "3D world", tone: Tone::Primary } }]
        }
        Zone::QmUnder => {
            let width = (w - 8.0 - GAP * 3.0) / 4.0;
            let at = |i: usize| r(4.0 + i as f32 * (width + GAP), 4.0, width, h - 8.0);
            vec![
                Item { action: Action::Talk, rect: at(0), look: Look::Talk },
                walk_or_light(facts, at(1)),
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
    (0..count)
        .map(|i| Item { action: capture_action(), rect: Rect::from_min_size(area.min + vec2(0.0, i as f32 * (card + GAP)), vec2(area.width(), card)), look: Look::Capture(i) })
        .collect()
}

/// What is on the item at `p` (panel units): its index. The small close button lies on top of what it sits on.
pub fn hit(items: &[Item], p: Pos2) -> Option<usize> {
    items.iter().enumerate().rev().find(|(_, i)| i.rect.contains(p)).map(|(i, _)| i)
}

/// The panel's fill: opaque enough to read over any world.
pub const FILL: Color32 = Color32::from_rgba_premultiplied(15, 15, 17, 244);
const TILE: Color32 = theme::INK_800;
const TILE_HOVER: Color32 = theme::INK_700;
const TILE_DOWN: Color32 = theme::INK_600;

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

fn tile_fill(hovered: bool, pressed: bool, accent: bool) -> Color32 {
    match (pressed, hovered, accent) {
        (true, _, true) => theme::mix(theme::ACCENT, theme::INK_900, 0.25),
        (false, true, true) => theme::ACCENT_HOVER,
        (false, false, true) => theme::ACCENT,
        (true, _, false) => TILE_DOWN,
        (false, true, false) => TILE_HOVER,
        (false, false, false) => TILE,
    }
}

/// An icon over its label, the two together in the middle of `rect`; a label too long is drawn smaller, then cut.
fn icon_over_label(p: &Painter, rect: Rect, icon: &str, label: &str, icon_color: Color32, label_color: Color32) {
    let icon_size = (rect.height() * 0.44).min(rect.width() * 0.56).clamp(14.0, 44.0);
    let mut size = (rect.height() * 0.15).clamp(9.5, 13.0);
    let room = rect.width() - 6.0;
    let wide = |size: f32| p.layout_no_wrap(label.to_string(), theme::font(size, weight::MEDIUM), Color32::WHITE).size().x;
    while size > 7.5 && wide(size) > room {
        size -= 0.5;
    }
    let total = icon_size + 3.0 + size;
    let top = rect.center().y - total / 2.0;
    widgets::icon(p, pos2(rect.center().x, top + icon_size / 2.0), icon, icon_size, icon_color);
    let font = theme::font(size, weight::MEDIUM);
    let fitted = fit(p, label, &font, room);
    p.text(pos2(rect.center().x, top + icon_size + 3.0 + size / 2.0), Align2::CENTER_CENTER, fitted, font, label_color);
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
        return ("3D picture flying".into(), theme::ACCENT);
    }
    if facts.recording {
        return (format!("Recording {}", clock(facts.recording_seconds.unwrap_or(0))), theme::REC);
    }
    if let Some(done) = facts.reconstructing {
        let more = if facts.waiting > 0 { format!(", {} waiting", facts.waiting) } else { String::new() };
        return (format!("Reconstructing {:.0}%{more}", done * 100.0), theme::FG);
    }
    if facts.waiting > 0 {
        return (format!("{} waiting to reconstruct", facts.waiting), theme::FG_2);
    }
    ("Nothing running".into(), theme::FG_3)
}

/// Draws a panel (in units, its corner at the painter's origin): its fill and everything on it.
pub fn paint(p: &Painter, zone: Zone, size: Vec2, facts: &Facts, hover: Option<usize>, pressed: Option<usize>) {
    let panel = Rect::from_min_size(pos2(0.0, 0.0), size);
    // A panel that is one tile is that tile's own shape; the others a larger card around theirs.
    let single = matches!(zone, Zone::WheelTopCorner | Zone::WheelRightCorner | Zone::WheelRight | Zone::WheelBelow | Zone::WheelBelowLeft | Zone::QmLowerLeft | Zone::QmLowerRight);
    let radius = if single { theme::TILE_RADIUS * 0.8 } else { theme::CARD_RADIUS * 0.8 };
    p.rect_filled(panel, theme::radius(radius), FILL);
    match zone {
        Zone::WheelTop => {
            title(p, pos2(PAD + 4.0, 14.0), "3D picture");
            let detail = if let Some(s) = facts.countdown { format!("in {s} s") } else if facts.picture_flying { "flying".to_string() } else { format!("in {} s", facts.timer) };
            text(p, pos2(size.x - PAD - 4.0, 14.0), Align2::RIGHT_CENTER, &detail, 11.0, weight::REGULAR, theme::FG_3);
        }
        Zone::WheelLeft | Zone::QmLeft => title(p, pos2(PAD + 4.0, 14.0), "Recent"),
        Zone::WheelLowerRight => title(p, pos2(size.x - PAD - 4.0 - 26.0, 14.0), "N4N0"),
        Zone::QmRight => title(p, pos2(PAD + 4.0, 14.0), "N4N0"),
        _ => {}
    }
    for (i, item) in items(zone, size, facts).iter().enumerate() {
        draw(p, item, facts, hover == Some(i), pressed == Some(i));
    }
}

fn draw(p: &Painter, item: &Item, facts: &Facts, hovered: bool, pressed: bool) {
    let rect = item.rect;
    let corner = theme::TILE_RADIUS * 0.8;
    match item.look {
        Look::Tile { icon, label, tone } => {
            let filled = (tone == Tone::Primary && (hovered || pressed)) || tone == Tone::On;
            let fill = if filled { tile_fill(hovered, pressed, true) } else { tile_fill(hovered, pressed, false) };
            p.rect_filled(rect, theme::radius(corner), fill);
            let (icon_color, label_color) = match (filled, tone) {
                (true, _) => (theme::ON_ACCENT, theme::ON_ACCENT),
                (false, Tone::Primary) => (theme::ACCENT, theme::FG),
                _ => (theme::FG, theme::FG),
            };
            if rect.width() > rect.height() * 2.2 {
                // A long row: the icon then its label.
                widgets::icon(p, pos2(rect.left() + rect.height() * 0.45, rect.center().y), icon, rect.height() * 0.5, icon_color);
                text(p, pos2(rect.left() + rect.height() * 0.86, rect.center().y), Align2::LEFT_CENTER, label, 11.0, weight::MEDIUM, label_color);
            } else {
                icon_over_label(p, rect, icon, label, icon_color, label_color);
            }
        }
        Look::Record => {
            let (fill, label) = if facts.recording { (theme::mix(theme::REC, theme::INK_900, 0.25), "Stop") } else { (tile_fill(hovered, pressed, false), "Record") };
            p.rect_filled(rect, theme::radius(corner), if facts.recording && hovered { theme::REC } else { fill });
            let dim = facts.capture_busy && !facts.recording;
            let color = if dim { theme::FG_4 } else { theme::FG };
            let wide = rect.width() > rect.height() * 1.3;
            let (dot_at, label_at, align) = if wide {
                (pos2(rect.left() + 16.0, rect.center().y), pos2(rect.left() + 28.0, rect.center().y), Align2::LEFT_CENTER)
            } else {
                (pos2(rect.center().x, rect.center().y - 7.0), pos2(rect.center().x, rect.center().y + 12.0), Align2::CENTER_CENTER)
            };
            if facts.recording {
                p.rect_filled(Rect::from_center_size(dot_at, Vec2::splat(11.0)), theme::radius(2.0), color);
            } else {
                p.circle_filled(dot_at, 6.5, if dim { theme::FG_4 } else { theme::REC });
            }
            text(p, label_at, align, label, if wide { 11.5 } else { 9.5 }, weight::MEDIUM, color);
        }
        Look::Status => {
            let (s, color) = status(facts);
            let font = theme::font(11.0, weight::MEDIUM);
            let narrow = rect.width() < 130.0;
            if narrow {
                // Two lines where it is narrow: what it does, and how far it is.
                let mut words = s.splitn(2, ' ');
                let (first, rest) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
                text(p, pos2(rect.left() + 8.0, rect.center().y - 7.0), Align2::LEFT_CENTER, first, 11.0, weight::MEDIUM, color);
                let cut = fit(p, rest, &font, rect.width() - 14.0);
                text(p, pos2(rect.left() + 8.0, rect.center().y + 8.0), Align2::LEFT_CENTER, &cut, 11.0, weight::REGULAR, theme::FG_2);
            } else {
                let cut = fit(p, &s, &font, rect.width() - 10.0);
                p.text(pos2(rect.left() + 4.0, rect.center().y), Align2::LEFT_CENTER, cut, font, color);
            }
            if let Some(done) = facts.reconstructing {
                let bar = Rect::from_min_size(pos2(rect.left() + 4.0, rect.bottom() - 7.0), vec2(rect.width() - 8.0, 3.0));
                p.rect_filled(bar, theme::radius(1.5), theme::INK_700);
                p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * done.clamp(0.0, 1.0), 3.0)), theme::radius(1.5), theme::ACCENT);
            }
        }
        Look::Music => {
            p.rect_filled(rect, theme::radius(corner), tile_fill(hovered, pressed, false));
            let (name, playing) = facts.music.clone().unwrap_or_default();
            let none = facts.music.is_none();
            let (icon_color, label_color) = if none { (theme::FG_4, theme::FG_4) } else { (theme::FG, theme::FG_2) };
            let glyph = if playing { icons::PAUSE } else { icons::PLAY };
            icon_over_label(p, rect, glyph, if none { "No music" } else { &name }, icon_color, label_color);
        }
        Look::Talk => {
            let fill = if pressed { theme::ACCENT } else { tile_fill(hovered, false, false) };
            p.rect_filled(rect, theme::radius(corner), fill);
            let color = if pressed { theme::ON_ACCENT } else if facts.n4n0 { theme::FG } else { theme::FG_4 };
            icon_over_label(p, rect, icons::MICROPHONE, if pressed { "Listening" } else { "Talk" }, color, color);
        }
        Look::Hide => {
            p.circle_filled(rect.center(), rect.width() / 2.0, if hovered { TILE_HOVER } else { theme::INK_850 });
            widgets::icon(p, rect.center(), icons::X, rect.width() * 0.55, theme::FG_2);
        }
        Look::Capture(n) => {
            let Some(recent) = facts.recent.get(n) else {
                text(p, rect.center() - vec2(0.0, 9.0), Align2::CENTER_CENTER, "No captures", 11.0, weight::MEDIUM, theme::FG_3);
                text(p, rect.center() + vec2(0.0, 7.0), Align2::CENTER_CENTER, "yet", 11.0, weight::REGULAR, theme::FG_4);
                return;
            };
            p.rect_filled(rect, theme::radius(theme::TILE_RADIUS * 0.7), if pressed { TILE_DOWN } else if hovered { TILE_HOVER } else { TILE });
            let picture = Rect::from_min_size(rect.min, vec2(rect.width(), (rect.height() - 28.0).max(16.0)));
            match recent.picture {
                Some(id) => {
                    p.add(RectShape::filled(picture, theme::radius(theme::TILE_RADIUS * 0.7), Color32::WHITE).with_texture(id, cover_uv(picture.size())));
                }
                None => {
                    p.rect_filled(picture, theme::radius(theme::TILE_RADIUS * 0.7), theme::INK_700);
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
                p.rect_stroke(rect, theme::radius(theme::TILE_RADIUS * 0.7), Stroke::new(1.5, theme::ACCENT), StrokeKind::Inside);
            }
        }
    }
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

/// Gives every panel's edge next to the wheel the wheel's curve: pixels of `rgba` (premultiplied, `size` wide) nearer than `radius` to `center`
/// (both in pixels of it) go, with a soft pixel at the edge. Only inside `within` (x0, y0, x1, y1).
pub fn bite(rgba: &mut [u8], size: [u32; 2], center: Pos2, radius: f32, within: [u32; 4]) {
    let [w, h] = size;
    for y in within[1]..within[3].min(h) {
        for x in within[0]..within[2].min(w) {
            let d = (pos2(x as f32 + 0.5, y as f32 + 0.5) - center).length();
            if d >= radius + 0.5 {
                continue;
            }
            let keep = (d - radius + 0.5).clamp(0.0, 1.0);
            let i = (y * w + x) as usize * 4;
            for v in &mut rgba[i..i + 4] {
                *v = (*v as f32 * keep) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::orbit::zones;

    fn facts() -> Facts {
        Facts {
            recent: (0..4).map(|i| Recent { dir: PathBuf::from(format!("s{i}")), title: format!("World {i}"), state: "3D view".into(), picture: None }).collect(),
            timer: 3,
            n4n0: true,
            ..Default::default()
        }
    }

    #[test]
    fn everything_lies_inside_its_panel_and_nothing_under_the_wheel() {
        for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (960.0, 540.0), (1000.0, 900.0), (1700.0, 600.0)] {
            for around in [zones::Around::Wheel, zones::Around::QuickMenu] {
                for placed in zones::layout(around, vec2(w, h)) {
                    let size = placed.logical();
                    for item in items(placed.zone, size, &facts()) {
                        assert!(Rect::from_min_size(pos2(0.0, 0.0), size).expand(0.5).contains_rect(item.rect), "{w}x{h} {:?}: {:?} in {size:?}", placed.zone, item.rect);
                        // Each corner of it is on the panel: the wheel's bite takes none of it.
                        if placed.zone.bite().is_some() {
                            for corner in [item.rect.left_top(), item.rect.right_top(), item.rect.left_bottom(), item.rect.right_bottom()] {
                                let at = placed.rect.min + corner.to_vec2() * placed.unit;
                                assert!((at - placed.center).length() >= zones::BITE * placed.unit - 1.0, "{w}x{h} {:?}: {:?} under the bite", placed.zone, item.action);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn every_action_named_is_there() {
        let around = |list: &[Zone]| -> Vec<Action> { list.iter().flat_map(|z| items(*z, z.units().size(), &facts())).map(|i| i.action).collect() };
        for actions in [around(&Zone::WHEEL), around(&Zone::QUICK_MENU)] {
            for needed in [Action::Picture(PhotoMode::People), Action::Picture(PhotoMode::World), Action::Record, Action::PlayPause, Action::Talk, Action::OpenApp, Action::Hide, Action::Page(Target::YouTube), Action::Page(Target::History)] {
                assert!(actions.contains(&needed), "{needed:?}");
            }
        }
    }

    #[test]
    fn the_close_button_is_hit_before_what_it_sits_on() {
        let size = Zone::WheelFarLeft.units().size();
        let list = items(Zone::WheelFarLeft, size, &facts());
        let on_close = list[1].rect.center();
        assert_eq!(list[hit(&list, on_close).unwrap()].action, Action::Hide);
        assert_eq!(list[hit(&list, pos2(10.0, size.y - 10.0)).unwrap()].action, Action::Idle);
        assert_eq!(hit(&list, pos2(-5.0, -5.0)), None);
    }

    #[test]
    fn fast_walk_is_not_offered_where_it_does_nothing() {
        let size = Zone::WheelRight.units().size();
        let desktop = Facts { desktop: true, ..facts() };
        assert_eq!(items(Zone::WheelRight, size, &desktop)[0].action, Action::Page(Target::Light));
        assert_eq!(items(Zone::WheelRight, size, &facts())[0].action, Action::FastWalk);
    }

    #[test]
    fn the_status_says_the_most_urgent_thing() {
        assert_eq!(status(&Facts { countdown: Some(2), recording: true, ..Default::default() }).0, "3D picture in 2 s");
        assert_eq!(status(&Facts { recording: true, recording_seconds: Some(75), reconstructing: Some(0.5), ..Default::default() }).0, "Recording 01:15");
        assert_eq!(status(&Facts { reconstructing: Some(0.42), waiting: 2, ..Default::default() }).0, "Reconstructing 42%, 2 waiting");
        assert_eq!(status(&Facts::default()).0, "Nothing running");
    }

    #[test]
    fn the_bite_cuts_only_what_is_inside_its_circle() {
        let mut rgba = vec![200u8; 4 * 100];
        bite(&mut rgba, [10, 10], pos2(0.0, 0.0), 5.0, [0, 0, 10, 10]);
        assert_eq!(rgba[0..4], [0, 0, 0, 0], "inside");
        assert_eq!(rgba[(9 * 10 + 9) * 4..(9 * 10 + 9) * 4 + 4], [200, 200, 200, 200], "outside");
    }
}
