//! Draws N4N0's menu pages from their layout (`orbiters_orb::menu::layout`). The SteamVR overlay draws everything; the
//! avatar's projected screen shows a picture of each page's unchanging part (cards, buttons, icons, labels: drawn here
//! too, see [`draw_chrome`]) and draws the rest over it, at the same places, from what the desktop sends.
//!
//! What it is drawn with is a [`Skin`]: the colours (a theme's palette, `orbiters_orb::menu::theme`) and the texts of
//! the unchanging part (`orbiters_orb::menu::strings`, English unless told otherwise). A text wider than its place (see
//! [`Place`]) is drawn smaller until it fits, never cut.

use super::menu_retro::{self, RetroPalette};
use egui::epaint::RectShape;
use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Stroke, StrokeKind, TextureId};
use orbiters_orb::menu::layout::{self, Align, Element, Icon, Label, Layout, Look, ScreenPage, Style, Tint};
use orbiters_orb::menu::strings::{Strings, Text};
use orbiters_orb::menu::theme::{Colors, Rgb, Theme};
use orbiters_orb::menu::{Menu, Page};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, widgets};
use std::sync::OnceLock;

/// The page's background in Orbiters' own colours (the window's Home draws N4N0's eyes with it).
pub const CARD: Color32 = theme::INK_900;
/// How much of the card covers an element greyed out (the avatar's screen uses the same).
const UNAVAILABLE_VEIL: f32 = 0.6;
/// What a label keeps clear of its element's sides.
const PAD: f32 = 8.0;

/// What changes on a page, for the overlay.
pub struct View<'a> {
    pub menu: &'a Menu,
    /// Element indices of the page's layout.
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    /// A thumbnail by its key (a result's video id, the music's art key: `layout::picture_key`), once loaded.
    pub picture: &'a dyn Fn(&str) -> Option<TextureId>,
    /// Seconds, for what moves (N4N0 listening).
    pub time: f64,
    /// Its colours and texts.
    pub skin: &'a Skin,
}

/// How the menu is drawn: both looks' colours and the texts of the pages' unchanging part.
#[derive(Clone, Debug)]
pub struct Skin {
    /// Orbiters' own look.
    pub palette: Palette,
    /// The retro look.
    pub retro: RetroPalette,
    pub strings: Strings,
}

impl Skin {
    /// A theme's palette in both looks, with `strings`.
    pub fn new(theme: &Theme, strings: Strings) -> Skin {
        Skin { palette: Palette::new(&theme.colors), retro: RetroPalette::new(&theme.colors), strings }
    }

    /// Each look in its own colours (the base themes': [`Theme::MODERN`], [`Theme::RETRO`]), with `strings`.
    pub fn standard(strings: Strings) -> Skin {
        Skin { palette: Palette::new(&Theme::MODERN.colors), retro: RetroPalette::new(&Theme::RETRO.colors), strings }
    }

    /// Each look in its own colours, in English: what the overlay draws.
    pub fn english() -> &'static Skin {
        static SKIN: OnceLock<Skin> = OnceLock::new();
        SKIN.get_or_init(|| Skin::standard(Strings::english().clone()))
    }

    pub fn text(&self, text: Text) -> &str {
        self.strings.get(text)
    }
}

/// The colours Orbiters' own look draws with, from a theme's palette (the base theme gives Orbiters UI's own).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The page.
    pub card: Color32,
    /// Tiles, fields, chips, text areas.
    pub surface: Color32,
    /// Keys, round buttons, an icon's disc, a picture's place: a step from the surface towards the text.
    pub raised: Color32,
    /// Dark keys (switch, backspace, the music's buttons): a step further.
    pub raised2: Color32,
    pub text: Color32,
    /// What is written on chips and dark keys: between the text and the muted text.
    pub text2: Color32,
    pub muted: Color32,
    /// A row's arrow, a picture's placeholder icon: between the muted text and the page.
    pub faint: Color32,
    pub accent: Color32,
    /// What is written on the accent.
    pub on_accent: Color32,
    /// Hang up, decline: the red a step towards white.
    pub danger: Color32,
    pub green: Color32,
    pub red: Color32,
    pub blue: Color32,
    pub violet: Color32,
    pub cyan: Color32,
    pub amber: Color32,
    /// States: the red towards white; Tracking: between blue and violet; Suggest: the violet a step darker.
    pub pink: Color32,
    pub indigo: Color32,
    pub purple: Color32,
}

/// The States tile's colour in the base palette.
pub const PINK: Color32 = Color32::from_rgb(0xff, 0x6e, 0xc7);

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// The colour that sits between `a` and `b` as `at` sits between `from` and `to` in the base palette, channel by
/// channel: exactly `at` for the base colours, the same step between a theme's own. A channel the base leaves no room
/// (`from` and `to` alike there) keeps `at`'s offset and moves as far as the other channels do.
pub fn between(from: Color32, to: Color32, at: Color32, a: Rgb, b: Rgb) -> Color32 {
    let (f, t, x) = (from.to_array(), to.to_array(), at.to_array());
    let steps: Vec<f64> = (0..3).filter(|&i| f[i] != t[i]).map(|i| (x[i] as f64 - f[i] as f64) / (t[i] as f64 - f[i] as f64)).collect();
    let mean = if steps.is_empty() { 0.0 } else { steps.iter().sum::<f64>() / steps.len() as f64 };
    let channel = |i: usize| {
        let (f, t, x, a, b) = (f[i] as f64, t[i] as f64, x[i] as f64, a[i] as f64, b[i] as f64);
        let v = if f != t { a + ((b - a) * (x - f) / (t - f)).round() } else { a + (x - f) + ((b - a) * mean).round() };
        v.clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(channel(0), channel(1), channel(2))
}

/// Whether two colours are too alike to tell apart (contrast ratio under 1.6): a light's swatch on a chip of nearly its
/// colour then gets a ring.
pub fn alike(a: Color32, b: Color32) -> bool {
    let (la, lb) = (luminance([a.r(), a.g(), a.b()]), luminance([b.r(), b.g(), b.b()]));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05) < 1.6
}

/// Relative luminance (sRGB to linear, Rec. 709 weights), 0 to 1.
fn luminance(c: Rgb) -> f32 {
    let linear = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2])
}

impl Palette {
    pub fn new(c: &Colors) -> Palette {
        let white = [255; 3];
        // Dark ink on a light accent (the base: Orbiters' near-black green), near white on a dark one.
        let on_accent = if luminance(c.accent) > 0.18 { between(theme::ACCENT, Color32::BLACK, theme::ON_ACCENT, c.accent, [0; 3]) } else { theme::FG };
        Palette {
            card: rgb(c.background),
            surface: rgb(c.surface),
            raised: between(theme::INK_800, theme::FG, theme::INK_750, c.surface, c.text),
            raised2: between(theme::INK_800, theme::FG, theme::INK_700, c.surface, c.text),
            text: rgb(c.text),
            text2: between(theme::FG, theme::FG_3, theme::FG_2, c.text, c.muted),
            muted: rgb(c.muted),
            faint: between(theme::FG_3, theme::INK_900, theme::FG_4, c.muted, c.background),
            accent: rgb(c.accent),
            on_accent,
            danger: between(theme::REC, Color32::WHITE, theme::DANGER, c.red, white),
            green: rgb(c.green),
            red: rgb(c.red),
            blue: rgb(c.blue),
            violet: rgb(c.violet),
            cyan: rgb(c.cyan),
            amber: rgb(c.amber),
            pink: between(theme::REC, Color32::WHITE, PINK, c.red, white),
            indigo: between(theme::BLUE, theme::VIOLET, theme::INDIGO, c.blue, c.violet),
            purple: between(theme::VIOLET, Color32::BLACK, theme::PURPLE, c.violet, [0; 3]),
        }
    }

    pub fn tint(&self, t: Tint) -> Color32 {
        match t {
            Tint::Green => self.green,
            Tint::Red => self.red,
            Tint::Blue => self.blue,
            Tint::Violet => self.violet,
            Tint::Cyan => self.cyan,
            Tint::Amber => self.amber,
            Tint::Pink => self.pink,
            Tint::Indigo => self.indigo,
            Tint::Purple => self.purple,
            Tint::Light(_) => tint(t),
        }
    }

    fn style(&self, style: Style) -> Color32 {
        match style {
            Style::Primary => self.text,
            Style::Secondary => self.muted,
            Style::Accent => self.accent,
        }
    }

    /// An element's fill.
    fn fill(&self, look: Look) -> Color32 {
        match look {
            Look::Tile | Look::Field | Look::Chip | Look::Area => self.surface,
            Look::Round | Look::Key => self.raised,
            Look::KeyDark => self.raised2,
            Look::Row => Color32::TRANSPARENT,
            Look::Accent | Look::Bar => self.accent,
            Look::Danger => self.danger,
        }
    }

    /// The colour of what is written on an element.
    fn ink(&self, look: Look) -> Color32 {
        match look {
            Look::Accent | Look::Danger => self.on_accent,
            Look::KeyDark | Look::Chip => self.text2,
            Look::Row => self.faint,
            _ => self.text,
        }
    }
}

fn rect(r: layout::Rect, origin: Pos2) -> Rect {
    Rect::from_min_size(origin + vec2(r.x as f32, r.y as f32), vec2(r.w as f32, r.h as f32))
}

pub fn glyph(icon: Icon) -> &'static str {
    match icon {
        Icon::Close => icons::X,
        Icon::Back => icons::ARROW_LEFT,
        Icon::Walk => icons::PERSON_SIMPLE_RUN,
        Icon::YouTube => icons::YOUTUBE_LOGO,
        Icon::Phone => icons::PHONE,
        Icon::PhoneOff => icons::PHONE_X,
        Icon::Cube => icons::CUBE,
        Icon::Search => icons::MAGNIFYING_GLASS,
        Icon::Backspace => icons::BACKSPACE,
        Icon::Copy => icons::COPY,
        Icon::Previous => icons::CARET_LEFT,
        Icon::Next => icons::CARET_RIGHT,
        Icon::User => icons::USER,
        Icon::Fill => icons::ARROW_UP_LEFT,
        Icon::Timer => icons::TIMER,
        Icon::Microphone => icons::MICROPHONE,
        Icon::Music => icons::MUSIC_NOTES,
        Icon::Lightbulb => icons::LIGHTBULB,
        Icon::PlayPause => icons::PLAY_PAUSE,
        Icon::SkipBack => icons::SKIP_BACK,
        Icon::SkipForward => icons::SKIP_FORWARD,
        Icon::SpeakerLow => icons::SPEAKER_LOW,
        Icon::SpeakerHigh => icons::SPEAKER_HIGH,
        Icon::SpeakerOff => icons::SPEAKER_X,
        Icon::States => icons::BOOKMARKS_SIMPLE,
        Icon::Save => icons::FLOPPY_DISK,
        Icon::Apply => icons::ARROW_COUNTER_CLOCKWISE,
        Icon::Star => icons::STAR,
        Icon::Drop => icons::HAND_DEPOSIT,
        Icon::Grab => icons::HAND_GRABBING,
        Icon::Link => icons::LINK_SIMPLE,
        Icon::Tracking => icons::VIRTUAL_REALITY,
        Icon::Suggest => icons::CHAT_TEARDROP_TEXT,
        Icon::Keyboard => icons::KEYBOARD,
        Icon::Plus => icons::PLUS,
        Icon::Send => icons::PAPER_PLANE_TILT,
        Icon::Check => icons::CHECK,
        Icon::Size => icons::ARROWS_OUT,
        Icon::Pose => icons::PERSON_SIMPLE_WALK,
        Icon::Watch => icons::MONITOR_PLAY,
        // Drawn as the N4N0 mark (`orbiters_ui::n4n0_logo`), never as a glyph.
        Icon::Mark => icons::STACK,
    }
}

/// A tile's colour in Orbiters' own palette (the window's Home uses these), or a light's colour.
pub fn tint(t: Tint) -> Color32 {
    match t {
        Tint::Light(i) => {
            let [r, g, b] = orbiters_contracts::orb::LIGHT_COLORS.get(i as usize).map_or([255; 3], |c| c.1);
            Color32::from_rgb(r, g, b)
        }
        _ => base_palette().tint(t),
    }
}

fn base_palette() -> &'static Palette {
    static BASE: OnceLock<Palette> = OnceLock::new();
    BASE.get_or_init(|| Palette::new(&Theme::MODERN.colors))
}

/// Where a text of a page's unchanging part goes: the room it has across and the size it is drawn at. A text wider than
/// the room (another language's) is drawn smaller until it fits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub room: f32,
    pub size: f32,
}

/// A text as wide as it may be: `txt` itself when it fits `room`, else smaller until it does (never cut).
pub struct Fitted {
    pub txt: Txt,
    /// Its size on the page.
    pub size: egui::Vec2,
    /// Half the height it lost: what keeps a smaller text where the whole one's middle was.
    pub dy: f32,
}

pub fn fit(p: &Painter, txt: Txt, room: f32) -> Fitted {
    let room = room.max(1.0);
    let measure = |t: &Txt| p.layout_job(t.job(f32::INFINITY)).size();
    let whole = measure(&txt);
    let full = txt.size;
    let (mut txt, mut size) = (txt, whole);
    for _ in 0..8 {
        if size.x <= room {
            break;
        }
        txt.size *= (room / size.x).min(0.99);
        size = measure(&txt);
    }
    if txt.size < full {
        SHRUNK.with(|s| {
            if let Some(list) = s.borrow_mut().as_mut() {
                list.push((txt.text.clone(), txt.size / full));
            }
        });
    }
    Fitted { txt, size, dy: (whole.y - size.y) / 2.0 }
}

thread_local! {
    /// The texts [`fit`] drew smaller on this thread, and how much smaller, while [`watch_shrinking`] is on.
    static SHRUNK: std::cell::RefCell<Option<Vec<(String, f32)>>> = const { std::cell::RefCell::new(None) };
}

/// Starts noting the texts drawn smaller to fit their place on this thread ([`shrunk`] hands them over).
pub fn watch_shrinking() {
    SHRUNK.with(|s| *s.borrow_mut() = Some(Vec::new()));
}

/// The texts drawn smaller since [`watch_shrinking`], each once with the most it was shrunk (its scale, below 1), in
/// the order first drawn; noting stops.
pub fn shrunk() -> Vec<(String, f32)> {
    let all = SHRUNK.with(|s| s.borrow_mut().take()).unwrap_or_default();
    let mut out: Vec<(String, f32)> = Vec::new();
    for (text, scale) in all {
        match out.iter_mut().find(|(t, _)| *t == text) {
            Some(seen) => seen.1 = seen.1.min(scale),
            None => out.push((text, scale)),
        }
    }
    out
}

/// The room beside an icon of a label of `size` in an element `width` wide (the icon `size × 1.1`, then a gap).
fn beside_icon(width: f32, icon: bool, size: f32) -> f32 {
    width - 2.0 * PAD - if icon { size * 1.1 + size * 0.45 } else { 0.0 }
}

/// Where an element's label goes in Orbiters' own look (none when it has none).
pub fn element_place(e: &Element) -> Option<Place> {
    e.label.and(element_metrics(e))
}

/// The room and size of what is written on an element of its look, its icon sized from it too (none for looks that
/// write nothing).
fn element_metrics(e: &Element) -> Option<Place> {
    let w = e.rect.w as f32;
    let icon = e.icon.is_some();
    Some(match e.look {
        Look::Tile if tile_is_compact(e.rect) => Place { room: w - 98.0, size: 25.0 },
        Look::Tile if tile_is_wide(e.rect) => Place { room: w - 132.0, size: 30.0 },
        Look::Tile => Place { room: w - 40.0, size: 30.0 },
        Look::Key => Place { room: beside_icon(w, icon, 24.0), size: 24.0 },
        Look::KeyDark => Place { room: beside_icon(w, icon, 22.0), size: 22.0 },
        // A colour's chip: its swatch before its name.
        Look::Chip if e.tint.is_some() => Place { room: w - 2.0 * PAD - 18.0 - 10.0, size: 22.0 },
        Look::Chip => Place { room: beside_icon(w, icon, 22.0), size: 22.0 },
        Look::Accent | Look::Danger => {
            let size = if e.rect.h >= 64 { 26.0 } else { 21.0 };
            Place { room: beside_icon(w, icon, size), size }
        }
        Look::Round | Look::Field | Look::Row | Look::Bar | Look::Area => return None,
    })
}

/// Where a page's label goes.
pub fn label_place(label: &Label) -> Place {
    Place { room: label.width as f32, size: label.size as f32 }
}

/// The back's title and hint: centred, clear of the card's sides.
pub const BACK_PLACES: [(Text, Place); 2] =
    [(Text::BackTitle, Place { room: layout::WIDTH as f32 - 96.0, size: 52.0 }), (Text::BackHint, Place { room: layout::WIDTH as f32 - 96.0, size: 28.0 })];

/// Every place a text has in Orbiters' own look: the back, and each page's labels and elements.
pub fn places() -> Vec<(Text, Place)> {
    let mut out = BACK_PLACES.to_vec();
    for page in ScreenPage::ALL {
        let l = layout::cached(page);
        out.extend(l.labels.iter().map(|label| (label.text, label_place(label))));
        out.extend(l.elements.iter().filter_map(|e| Some((e.label?, element_place(e)?))));
    }
    out
}

/// An icon and a label side by side, centred in `r`, the label fitted to its place.
fn icon_and_label(p: &Painter, r: Rect, icon: Option<Icon>, label: Option<&str>, place: Place, weight_: f32, color: Color32) {
    let size = place.size;
    let icon_size = size * 1.1;
    match (icon, label) {
        (Some(i), None) => widgets::icon(p, r.center(), glyph(i), icon_size, color),
        (None, Some(l)) => {
            let fitted = fit(p, Txt::new(l, size).weight(weight_).color(color), place.room);
            widgets::text(p, r.center(), Align2::CENTER_CENTER, &fitted.txt);
        }
        (Some(i), Some(l)) => {
            let fitted = fit(p, Txt::new(l, size).weight(weight_).color(color), place.room);
            let gap = size * 0.45;
            let total = icon_size + gap + fitted.size.x;
            let left = r.center().x - total / 2.0;
            widgets::icon(p, pos2(left + icon_size / 2.0, r.center().y), glyph(i), icon_size, color);
            widgets::text(p, pos2(left + icon_size + gap, r.center().y), Align2::LEFT_CENTER, &fitted.txt);
        }
        (None, None) => {}
    }
}

/// One element's unchanging look.
fn element(p: &Painter, e: &Element, origin: Pos2, skin: &Skin) {
    let pal = &skin.palette;
    let r = rect(e.rect, origin);
    let radius = theme::radius(e.radius() as f32);
    let color = pal.fill(e.look);
    if color != Color32::TRANSPARENT {
        p.rect_filled(r, radius, color);
    }
    let label = e.label.map(|t| skin.text(t));
    let place = element_metrics(e).unwrap_or(Place { room: r.width(), size: 24.0 });
    match e.look {
        Look::Tile if tile_is_compact(e.rect) => {
            // A small tile (ten tiles): a smaller icon at its top left, the title beside it, its note under both.
            let tint = e.tint.map(|t| pal.tint(t)).unwrap_or(pal.accent);
            let tile = Rect::from_min_size(r.min + vec2(20.0, 20.0), vec2(56.0, 56.0));
            p.rect_filled(tile, theme::radius(16.0), theme::alpha(tint, 0.16));
            if let Some(icon) = e.icon {
                widgets::icon(p, tile.center(), glyph(icon), 30.0, tint);
            }
            if let Some(title) = label {
                let fitted = fit(p, Txt::new(title, place.size).weight(weight::SEMIBOLD).color(pal.text), place.room);
                widgets::text(p, r.min + vec2(90.0, 34.0 + fitted.dy), Align2::LEFT_TOP, &fitted.txt);
            }
        }
        Look::Tile => {
            let tint = e.tint.map(|t| pal.tint(t)).unwrap_or(pal.accent);
            // A wide tile (six tiles): the icon at its left, the title beside it (its note under the title); one as wide
            // as it is tall or taller (four and seven tiles): the icon at its top, the title low (its note under it).
            let wide = tile_is_wide(e.rect);
            let tile = Rect::from_min_size(r.min + if wide { vec2(24.0, (r.height() - 72.0) / 2.0) } else { vec2(24.0, 24.0) }, vec2(72.0, 72.0));
            p.rect_filled(tile, theme::radius(20.0), theme::alpha(tint, 0.16));
            if let Some(icon) = e.icon {
                widgets::icon(p, tile.center(), glyph(icon), 40.0, tint);
            }
            if let Some(title) = label {
                let fitted = fit(p, Txt::new(title, place.size).weight(weight::SEMIBOLD).color(pal.text), place.room);
                let at = r.min + if wide { vec2(116.0, 40.0) } else { vec2(24.0, r.height() - 92.0) };
                widgets::text(p, at + vec2(0.0, fitted.dy), Align2::LEFT_TOP, &fitted.txt);
            }
        }
        // A countdown's segment: its colour is all of it; a text area is its panel, its text slots over it.
        Look::Bar | Look::Area => {}
        Look::Field => {
            if let Some(icon) = e.icon {
                widgets::icon(p, pos2(r.min.x + 34.0, r.center().y), glyph(icon), 26.0, pal.muted);
            }
        }
        Look::Row => match e.icon {
            // A friend's picture at the row's left.
            Some(Icon::User) => {
                let at = pos2(r.min.x + 44.0, r.center().y);
                p.circle_filled(at, 24.0, pal.raised);
                widgets::icon(p, at, glyph(Icon::User), 24.0, pal.muted);
            }
            Some(icon) => widgets::icon(p, pos2(r.max.x - 28.0, r.center().y), glyph(icon), 22.0, pal.ink(e.look)),
            None => {}
        },
        Look::Key => icon_and_label(p, r, e.icon, label, place, weight::REGULAR, pal.ink(e.look)),
        Look::KeyDark => icon_and_label(p, r, e.icon, label, place, weight::MEDIUM, pal.ink(e.look)),
        Look::Round => icon_and_label(p, r, e.icon, None, Place { room: r.width(), size: 24.0 }, weight::REGULAR, pal.text),
        Look::Chip => match (e.tint, label) {
            // A colour's chip: its swatch before its name.
            (Some(t), Some(label)) => {
                let fitted = fit(p, Txt::new(label, place.size).weight(weight::MEDIUM).color(pal.ink(e.look)), place.room);
                let (dot, gap) = (18.0, 10.0);
                let left = r.center().x - (dot + gap + fitted.size.x) / 2.0;
                let (at, swatch) = (pos2(left + dot / 2.0, r.center().y), pal.tint(t));
                p.circle_filled(at, dot / 2.0, swatch);
                if alike(swatch, pal.fill(e.look)) {
                    p.circle_stroke(at, dot / 2.0, Stroke::new(1.0, pal.muted));
                }
                widgets::text(p, pos2(left + dot + gap, r.center().y), Align2::LEFT_CENTER, &fitted.txt);
            }
            _ => icon_and_label(p, r, e.icon, label, place, weight::MEDIUM, pal.ink(e.look)),
        },
        Look::Accent | Look::Danger => icon_and_label(p, r, e.icon, label, place, weight::SEMIBOLD, pal.ink(e.look)),
    }
    // A key's character (its label when it has none).
    if let (Look::Key, orbiters_orb::menu::Target::Key(c), None) = (e.look, e.target, e.label) {
        widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(c.to_string(), 28.0).color(pal.text));
    }
}

/// Whether a home tile lays its icon beside its title (a tile half as wide again as it is tall, or wider) rather than
/// above it.
pub fn tile_is_wide(r: layout::Rect) -> bool {
    2 * r.w as u32 > 3 * r.h as u32
}

/// Whether a home tile is one of the ten-tile Home's small ones: its icon smaller, its title beside it.
pub fn tile_is_compact(r: layout::Rect) -> bool {
    r.h <= 160 && r.w < 300
}

/// N4N0 itself, `radius` large: its dot (the accent) with its eyes (the page's colour), as the menu's back shows it.
fn orb_face(p: &Painter, center: Pos2, radius: f32, pal: &Palette) {
    let s = radius / 26.0;
    p.circle_filled(center, radius, pal.accent);
    for side in [-1.0, 1.0] {
        p.rect_filled(Rect::from_center_size(center + vec2(side * 9.0 * s, -2.0 * s), vec2(7.0 * s, 13.0 * s)), theme::radius(3.5 * s), pal.card);
    }
}

/// The card, the page's pictures and its fixed labels.
fn backdrop(p: &Painter, origin: Pos2, layout: &Layout, listening: Option<f64>, skin: &Skin) {
    let pal = &skin.palette;
    let card = Rect::from_min_size(origin, vec2(layout::WIDTH as f32, layout::HEIGHT as f32));
    p.rect_filled(card, theme::radius(layout::CARD_RADIUS as f32), pal.card);
    // What N4N0 says has a place of its own on the seven-tile Home: a panel as a tile's, N4N0 at its top left.
    for b in &layout.boxes {
        let r = rect(*b, origin);
        p.rect_filled(r, theme::radius(24.0), pal.surface);
        orb_face(p, r.min + vec2(38.0, 36.0), 14.0, pal);
    }
    for (r, icon, t) in &layout.icons {
        let r = rect(*r, origin);
        if *icon == Icon::Mark {
            orbiters_ui::n4n0_logo::paint(p, Rect::from_center_size(r.center(), orbiters_ui::n4n0_logo::size_for_width(r.width())), pal.text);
            continue;
        }
        let color = t.map(|t| pal.tint(t));
        p.circle_filled(r.center(), r.width() / 2.0, color.map_or(pal.raised, |c| theme::alpha(c, 0.16)));
        widgets::icon(p, r.center(), glyph(*icon), r.width() * 0.5, color.unwrap_or(pal.muted));
        // N4N0 listening: the microphone breathes.
        if let (Icon::Microphone, Some(time)) = (icon, listening) {
            let pulse = (0.5 + 0.5 * (time * 4.0).sin()) as f32;
            p.circle_stroke(r.center(), r.width() / 2.0 + 3.0 + 4.0 * pulse, Stroke::new(2.0, theme::alpha(pal.accent, 0.7 - 0.5 * pulse)));
        }
    }
    for label in &layout.labels {
        let place = label_place(label);
        let txt = Txt::new(skin.text(label.text), place.size).weight(if label.bold { weight::SEMIBOLD } else { weight::MEDIUM }).color(pal.style(label.style));
        let fitted = fit(p, txt, place.room);
        widgets::text(p, origin + vec2(label.x as f32, label.y as f32 + fitted.dy), Align2::LEFT_TOP, &fitted.txt);
    }
}

/// Everything on a page that never changes: what the avatar's screen has as the page's picture.
pub fn draw_chrome(p: &Painter, origin: Pos2, layout: &Layout, skin: &Skin) {
    backdrop(p, origin, layout, None, skin);
    for e in &layout.elements {
        element(p, e, origin, skin);
    }
    // The music's place for its art, which the art covers once it came (the screen leaves this page's placeholder to
    // the picture).
    if layout.page == ScreenPage::Music {
        for pic in &layout.pictures {
            art_placeholder(p, rect(pic.rect, origin), &skin.palette);
        }
    }
}

/// Music without art: a note on the picture's place.
fn art_placeholder(p: &Painter, r: Rect, pal: &Palette) {
    p.rect_filled(r, theme::radius(10.0), pal.raised);
    widgets::icon(p, r.center(), glyph(Icon::Music), r.height() * 0.32, pal.faint);
}

/// A slot's text, as the overlay writes it: the same characters the screen gets, in Orbiters' font.
fn slot(p: &Painter, s: &layout::Slot, text: &str, origin: Pos2, pal: &Palette) {
    if text.is_empty() {
        return;
    }
    let r = rect(s.rect(), origin);
    let txt = Txt::new(text, s.size as f32 * 0.82).weight(if s.size >= 40 { weight::SEMIBOLD } else { weight::REGULAR }).color(pal.style(s.style));
    match s.align {
        Align::Left => widgets::text_truncated(p, pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, &txt, r.width()),
        Align::Center => widgets::text_truncated(p, r.center(), Align2::CENTER_CENTER, &txt, r.width()),
    };
}

/// A thumbnail with the page's rounded corners, or its place while it loads.
fn picture(p: &Painter, r: Rect, texture: Option<TextureId>, pal: &Palette) {
    let radius = theme::radius(10.0);
    match texture {
        Some(id) => {
            p.add(RectShape::filled(r, radius, Color32::WHITE).with_texture(id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
        }
        None => {
            p.rect_filled(r, radius, pal.raised);
        }
    }
}

/// The overlay seen from behind: N4N0 (a green dot with its eyes) over "N4N0 menu" and "look at the other side", as the
/// avatar's screen shows its back. The menu is on the side its owner faces.
pub fn draw_back(p: &Painter, origin: Pos2, skin: &Skin) {
    let pal = &skin.palette;
    let card = Rect::from_min_size(origin, vec2(layout::WIDTH as f32, layout::HEIGHT as f32));
    p.rect_filled(card, theme::radius(layout::CARD_RADIUS as f32), pal.card);
    let centre = card.center().x;
    let orb = pos2(centre, card.center().y - 86.0);
    p.circle_filled(orb, 40.0, theme::alpha(pal.accent, 0.16));
    p.circle_filled(orb, 26.0, pal.accent);
    for side in [-1.0, 1.0] {
        p.rect_filled(Rect::from_center_size(orb + vec2(side * 9.0, -2.0), vec2(7.0, 13.0)), theme::radius(3.5), pal.card);
    }
    let [(title_text, title), (hint_text, hint)] = BACK_PLACES;
    let title = fit(p, Txt::new(skin.text(title_text), title.size).weight(weight::SEMIBOLD).color(pal.text), title.room);
    widgets::text(p, pos2(centre, card.center().y + 6.0), Align2::CENTER_CENTER, &title.txt);
    let hint = fit(p, Txt::new(skin.text(hint_text), hint.size).weight(weight::REGULAR).color(pal.muted), hint.room);
    widgets::text(p, pos2(centre, card.center().y + 62.0), Align2::CENTER_CENTER, &hint.txt);
}

/// The whole page as the overlay shows it.
pub fn draw(p: &Painter, origin: Pos2, view: &View) {
    let (menu, skin) = (view.menu, view.skin);
    let pal = &skin.palette;
    let page = ScreenPage::of(menu);
    let layout = layout::cached(page);
    backdrop(p, origin, layout, menu.listening.then_some(view.time), skin);
    for (i, e) in layout.elements.iter().enumerate() {
        let state = layout::state(menu, e);
        if state.hidden {
            continue;
        }
        element(p, e, origin, skin);
        let r = rect(e.rect, origin);
        let radius = theme::radius(e.radius() as f32);
        if state.unavailable {
            // Greyed out: the card shows through, as on the avatar's screen.
            p.rect_filled(r, radius, theme::alpha(pal.card, UNAVAILABLE_VEIL));
        } else if view.pressed == Some(i) {
            p.rect_filled(r, radius, if matches!(e.look, Look::Accent | Look::Danger) { theme::black(0.16) } else { theme::white(0.12) });
        } else if view.hover == Some(i) {
            p.rect_filled(r, radius, if matches!(e.look, Look::Accent | Look::Danger) { theme::white(0.14) } else { theme::white(0.06) });
        }
        if state.on {
            p.rect_stroke(r.shrink(1.0), radius, Stroke::new(3.0, pal.accent), StrokeKind::Inside);
        }
    }
    for s in &layout.slots {
        slot(p, s, &layout::slot_text(menu, s), origin, pal);
    }
    for pic in &layout.pictures {
        let r = rect(pic.rect, origin);
        match (layout::picture_key(menu, pic), menu.page) {
            (Some(key), _) => match (view.picture)(key) {
                Some(texture) => picture(p, r, Some(texture), pal),
                None if menu.page == Page::Music => art_placeholder(p, r, pal),
                None => picture(p, r, None, pal),
            },
            (None, Page::Music) => art_placeholder(p, r, pal),
            (None, _) => {}
        }
    }
}

/// An average character of Orbiters' font, in ems (Segoe UI's lower-case letters with a capital now and then).
pub const MODERN_EM: f32 = 0.5;
/// How far a text may shrink and still be within the `max` of its key (`orbiters-desktop --orb-menu-strings`).
pub const MIN_SCALE: f32 = 0.8;

/// Every place a text of the unchanging part has in both looks, with the width of an average character there in ems.
pub fn all_places() -> Vec<(Text, Place, f32)> {
    let modern = places().into_iter().map(|(t, p)| (t, p, MODERN_EM));
    modern.chain(menu_retro::places().into_iter().map(|(t, p)| (t, p, menu_retro::EM))).collect()
}

/// The most characters a key's text should have: what fits each of its places at [`MIN_SCALE`] of its size in
/// characters of average width (never fewer than the English text has). A longer text is still drawn, smaller.
pub fn max_length(text: Text, places: &[(Text, Place, f32)]) -> usize {
    let fits = places
        .iter()
        .filter(|(t, _, _)| *t == text)
        .map(|(_, place, em)| (place.room / (place.size * em * MIN_SCALE)).floor().max(1.0) as usize)
        .min()
        .unwrap_or(2 * text.en().chars().count());
    fits.max(text.en().chars().count())
}
