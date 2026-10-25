//! Draws N4N0's menu pages from their layout (`orbiters_orb::menu::layout`). The SteamVR overlay draws everything; the
//! avatar's projected screen shows a picture of each page's unchanging part (cards, buttons, icons, labels: drawn here
//! too, see [`draw_chrome`]) and draws the rest over it, at the same places, from what the desktop sends.

use egui::epaint::RectShape;
use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Stroke, StrokeKind, TextureId};
use orbiters_orb::menu::layout::{self, Align, Element, Icon, Layout, Look, ScreenPage, Style, Tint};
use orbiters_orb::menu::{Menu, Page};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, widgets};

/// The page's background.
pub const CARD: Color32 = theme::INK_900;
/// How much of the card covers an element greyed out (the avatar's screen uses the same).
const UNAVAILABLE_VEIL: f32 = 0.6;

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
    }
}

/// The States tile's colour.
pub const PINK: Color32 = Color32::from_rgb(0xff, 0x6e, 0xc7);

pub fn tint(t: Tint) -> Color32 {
    match t {
        Tint::Green => theme::ACCENT,
        Tint::Red => theme::REC,
        Tint::Blue => theme::BLUE,
        Tint::Violet => theme::VIOLET,
        Tint::Cyan => theme::CYAN,
        Tint::Amber => theme::WARN,
        Tint::Pink => PINK,
        Tint::Light(i) => {
            let [r, g, b] = orbiters_contracts::orb::LIGHT_COLORS.get(i as usize).map_or([255; 3], |c| c.1);
            Color32::from_rgb(r, g, b)
        }
    }
}

fn style_color(style: Style) -> Color32 {
    match style {
        Style::Primary => theme::FG,
        Style::Secondary => theme::FG_3,
        Style::Accent => theme::ACCENT,
    }
}

/// An element's fill.
fn fill(look: Look) -> Color32 {
    match look {
        Look::Tile | Look::Field | Look::Chip => theme::INK_800,
        Look::Round | Look::Key => theme::INK_750,
        Look::KeyDark => theme::INK_700,
        Look::Row => Color32::TRANSPARENT,
        Look::Accent | Look::Bar => theme::ACCENT,
        Look::Danger => theme::DANGER,
    }
}

/// The colour of what is written on an element.
fn ink(look: Look) -> Color32 {
    match look {
        Look::Accent | Look::Danger => theme::ON_ACCENT,
        Look::KeyDark | Look::Chip => theme::FG_2,
        Look::Row => theme::FG_4,
        _ => theme::FG,
    }
}

/// An icon and a label side by side, centred in `r`.
fn icon_and_label(p: &Painter, r: Rect, icon: Option<Icon>, label: Option<&str>, size: f32, weight_: f32, color: Color32) {
    let icon_size = size * 1.1;
    match (icon, label) {
        (Some(i), None) => widgets::icon(p, r.center(), glyph(i), icon_size, color),
        (None, Some(l)) => {
            widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(l, size).weight(weight_).color(color));
        }
        (Some(i), Some(l)) => {
            let txt = Txt::new(l, size).weight(weight_).color(color);
            let width = p.layout_job(txt.job(f32::INFINITY)).size().x;
            let gap = size * 0.45;
            let total = icon_size + gap + width;
            let left = r.center().x - total / 2.0;
            widgets::icon(p, pos2(left + icon_size / 2.0, r.center().y), glyph(i), icon_size, color);
            widgets::text(p, pos2(left + icon_size + gap, r.center().y), Align2::LEFT_CENTER, &txt);
        }
        (None, None) => {}
    }
}

/// One element's unchanging look.
fn element(p: &Painter, e: &Element, origin: Pos2) {
    let r = rect(e.rect, origin);
    let radius = theme::radius(e.radius() as f32);
    let color = fill(e.look);
    if color != Color32::TRANSPARENT {
        p.rect_filled(r, radius, color);
    }
    match e.look {
        Look::Tile => {
            let tint = e.tint.map(tint).unwrap_or(theme::ACCENT);
            // A wide tile (six tiles): the icon at its left, the title beside it (its note under the title); one as wide
            // as it is tall or taller (four and seven tiles): the icon at its top, the title low (its note under it).
            let wide = tile_is_wide(e.rect);
            let tile = Rect::from_min_size(r.min + if wide { vec2(24.0, (r.height() - 72.0) / 2.0) } else { vec2(24.0, 24.0) }, vec2(72.0, 72.0));
            p.rect_filled(tile, theme::radius(20.0), theme::alpha(tint, 0.16));
            if let Some(icon) = e.icon {
                widgets::icon(p, tile.center(), glyph(icon), 40.0, tint);
            }
            if let Some(title) = e.label {
                let at = r.min + if wide { vec2(116.0, 40.0) } else { vec2(24.0, r.height() - 92.0) };
                widgets::text(p, at, Align2::LEFT_TOP, &Txt::new(title, 30.0).weight(weight::SEMIBOLD).color(theme::FG));
            }
        }
        // A countdown's segment: its colour is all of it.
        Look::Bar => {}
        Look::Field => {
            if let Some(icon) = e.icon {
                widgets::icon(p, pos2(r.min.x + 34.0, r.center().y), glyph(icon), 26.0, theme::FG_3);
            }
        }
        Look::Row => match e.icon {
            // A friend's picture at the row's left.
            Some(Icon::User) => {
                let at = pos2(r.min.x + 44.0, r.center().y);
                p.circle_filled(at, 24.0, theme::INK_750);
                widgets::icon(p, at, glyph(Icon::User), 24.0, theme::FG_3);
            }
            Some(icon) => widgets::icon(p, pos2(r.max.x - 28.0, r.center().y), glyph(icon), 22.0, ink(e.look)),
            None => {}
        },
        Look::Key => icon_and_label(p, r, e.icon, e.label, 24.0, weight::REGULAR, ink(e.look)),
        Look::KeyDark => icon_and_label(p, r, e.icon, e.label, 22.0, weight::MEDIUM, ink(e.look)),
        Look::Round => icon_and_label(p, r, e.icon, None, 24.0, weight::REGULAR, theme::FG),
        Look::Chip => match (e.tint, e.label) {
            // A colour's chip: its swatch before its name.
            (Some(t), Some(label)) => {
                let txt = Txt::new(label, 22.0).weight(weight::MEDIUM).color(ink(e.look));
                let width = p.layout_job(txt.job(f32::INFINITY)).size().x;
                let (dot, gap) = (18.0, 10.0);
                let left = r.center().x - (dot + gap + width) / 2.0;
                p.circle_filled(pos2(left + dot / 2.0, r.center().y), dot / 2.0, tint(t));
                widgets::text(p, pos2(left + dot + gap, r.center().y), Align2::LEFT_CENTER, &txt);
            }
            _ => icon_and_label(p, r, e.icon, e.label, 22.0, weight::MEDIUM, ink(e.look)),
        },
        Look::Accent | Look::Danger => icon_and_label(p, r, e.icon, e.label, if e.rect.h >= 64 { 26.0 } else { 21.0 }, weight::SEMIBOLD, ink(e.look)),
    }
    // A key's character (its label when it has none).
    if let (Look::Key, orbiters_orb::menu::Target::Key(c), None) = (e.look, e.target, e.label) {
        widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(c.to_string(), 28.0).color(theme::FG));
    }
}

/// Whether a home tile lays its icon beside its title (a tile half as wide again as it is tall, or wider) rather than
/// above it.
pub fn tile_is_wide(r: layout::Rect) -> bool {
    2 * r.w as u32 > 3 * r.h as u32
}

/// N4N0 itself, `radius` large: its green dot with its eyes, as the menu's back shows it.
pub fn orb_face(p: &Painter, center: Pos2, radius: f32) {
    let s = radius / 26.0;
    p.circle_filled(center, radius, theme::ACCENT);
    for side in [-1.0, 1.0] {
        p.rect_filled(Rect::from_center_size(center + vec2(side * 9.0 * s, -2.0 * s), vec2(7.0 * s, 13.0 * s)), theme::radius(3.5 * s), CARD);
    }
}

/// The card, the page's pictures and its fixed labels.
fn backdrop(p: &Painter, origin: Pos2, layout: &Layout, listening: Option<f64>) {
    let card = Rect::from_min_size(origin, vec2(layout::WIDTH as f32, layout::HEIGHT as f32));
    p.rect_filled(card, theme::radius(layout::CARD_RADIUS as f32), CARD);
    // What N4N0 says has a place of its own on the seven-tile Home: a panel as a tile's, N4N0 at its top left.
    for b in &layout.boxes {
        let r = rect(*b, origin);
        p.rect_filled(r, theme::radius(24.0), theme::INK_800);
        orb_face(p, r.min + vec2(38.0, 36.0), 14.0);
    }
    for (r, icon, t) in &layout.icons {
        let r = rect(*r, origin);
        let color = t.map(tint);
        p.circle_filled(r.center(), r.width() / 2.0, color.map_or(theme::INK_750, |c| theme::alpha(c, 0.16)));
        widgets::icon(p, r.center(), glyph(*icon), r.width() * 0.5, color.unwrap_or(theme::FG_3));
        // N4N0 listening: the microphone breathes.
        if let (Icon::Microphone, Some(time)) = (icon, listening) {
            let pulse = (0.5 + 0.5 * (time * 4.0).sin()) as f32;
            p.circle_stroke(r.center(), r.width() / 2.0 + 3.0 + 4.0 * pulse, Stroke::new(2.0, theme::alpha(theme::ACCENT, 0.7 - 0.5 * pulse)));
        }
    }
    for label in &layout.labels {
        let txt = Txt::new(label.text, label.size as f32).weight(if label.bold { weight::SEMIBOLD } else { weight::MEDIUM }).color(style_color(label.style));
        widgets::text(p, origin + vec2(label.x as f32, label.y as f32), Align2::LEFT_TOP, &txt);
    }
}

/// Everything on a page that never changes: what the avatar's screen has as the page's picture.
pub fn draw_chrome(p: &Painter, origin: Pos2, layout: &Layout) {
    backdrop(p, origin, layout, None);
    for e in &layout.elements {
        element(p, e, origin);
    }
    // The music's place for its art, which the art covers once it came (the screen leaves this page's placeholder to
    // the picture).
    if layout.page == ScreenPage::Music {
        for pic in &layout.pictures {
            art_placeholder(p, rect(pic.rect, origin));
        }
    }
}

/// Music without art: a note on the picture's place.
fn art_placeholder(p: &Painter, r: Rect) {
    p.rect_filled(r, theme::radius(10.0), theme::INK_750);
    widgets::icon(p, r.center(), glyph(Icon::Music), r.height() * 0.32, theme::FG_4);
}

/// A slot's text, as the overlay writes it: the same characters the screen gets, in Orbiters' font.
fn slot(p: &Painter, s: &layout::Slot, text: &str, origin: Pos2) {
    if text.is_empty() {
        return;
    }
    let r = rect(s.rect(), origin);
    let txt = Txt::new(text, s.size as f32 * 0.82).weight(if s.size >= 40 { weight::SEMIBOLD } else { weight::REGULAR }).color(style_color(s.style));
    match s.align {
        Align::Left => widgets::text_truncated(p, pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, &txt, r.width()),
        Align::Center => widgets::text_truncated(p, r.center(), Align2::CENTER_CENTER, &txt, r.width()),
    };
}

/// A thumbnail with the page's rounded corners, or its place while it loads.
fn picture(p: &Painter, r: Rect, texture: Option<TextureId>) {
    let radius = theme::radius(10.0);
    match texture {
        Some(id) => {
            p.add(RectShape::filled(r, radius, Color32::WHITE).with_texture(id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
        }
        None => {
            p.rect_filled(r, radius, theme::INK_750);
        }
    }
}

/// The overlay seen from behind: N4N0 (a green dot with its eyes) over "N4N0 menu" and "look at the other side", as the
/// avatar's screen shows its back. The menu is on the side its owner faces.
pub fn draw_back(p: &Painter, origin: Pos2) {
    let card = Rect::from_min_size(origin, vec2(layout::WIDTH as f32, layout::HEIGHT as f32));
    p.rect_filled(card, theme::radius(layout::CARD_RADIUS as f32), CARD);
    let centre = card.center().x;
    let orb = pos2(centre, card.center().y - 86.0);
    p.circle_filled(orb, 40.0, theme::alpha(theme::ACCENT, 0.16));
    p.circle_filled(orb, 26.0, theme::ACCENT);
    for side in [-1.0, 1.0] {
        p.rect_filled(Rect::from_center_size(orb + vec2(side * 9.0, -2.0), vec2(7.0, 13.0)), theme::radius(3.5), CARD);
    }
    let title = Txt::new("N4N0 menu", 52.0).weight(weight::SEMIBOLD).color(theme::FG);
    widgets::text(p, pos2(centre, card.center().y + 6.0), Align2::CENTER_CENTER, &title);
    let hint = Txt::new("look at the other side", 28.0).weight(weight::REGULAR).color(theme::FG_3);
    widgets::text(p, pos2(centre, card.center().y + 62.0), Align2::CENTER_CENTER, &hint);
}

/// The whole page as the overlay shows it.
pub fn draw(p: &Painter, origin: Pos2, view: &View) {
    let menu = view.menu;
    let page = ScreenPage::of(menu);
    let layout = layout::cached(page);
    backdrop(p, origin, layout, menu.listening.then_some(view.time));
    for (i, e) in layout.elements.iter().enumerate() {
        let state = layout::state(menu, e);
        if state.hidden {
            continue;
        }
        element(p, e, origin);
        let r = rect(e.rect, origin);
        let radius = theme::radius(e.radius() as f32);
        if state.unavailable {
            // Greyed out: the card shows through, as on the avatar's screen.
            p.rect_filled(r, radius, theme::alpha(CARD, UNAVAILABLE_VEIL));
        } else if view.pressed == Some(i) {
            p.rect_filled(r, radius, if matches!(e.look, Look::Accent | Look::Danger) { theme::black(0.16) } else { theme::white(0.12) });
        } else if view.hover == Some(i) {
            p.rect_filled(r, radius, if matches!(e.look, Look::Accent | Look::Danger) { theme::white(0.14) } else { theme::white(0.06) });
        }
        if state.on {
            p.rect_stroke(r.shrink(1.0), radius, Stroke::new(3.0, theme::ACCENT), StrokeKind::Inside);
        }
    }
    for s in &layout.slots {
        slot(p, s, &layout::slot_text(menu, s), origin);
    }
    for pic in &layout.pictures {
        let r = rect(pic.rect, origin);
        match (layout::picture_key(menu, pic), menu.page) {
            (Some(key), _) => match (view.picture)(key) {
                Some(texture) => picture(p, r, Some(texture)),
                None if menu.page == Page::Music => art_placeholder(p, r),
                None => picture(p, r, None),
            },
            (None, Page::Music) => art_placeholder(p, r),
            (None, _) => {}
        }
    }
}
