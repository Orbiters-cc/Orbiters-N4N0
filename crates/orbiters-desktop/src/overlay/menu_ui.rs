//! Draws Orb's menu pages from their layout (`orbiters_orb::menu::layout`). The SteamVR overlay draws everything; the
//! avatar's projected screen shows a picture of each page's unchanging part (cards, buttons, icons, labels: drawn here
//! too, see [`draw_chrome`]) and draws the rest over it, at the same places, from what the desktop sends.

use egui::epaint::RectShape;
use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Stroke, StrokeKind, TextureId};
use orbiters_orb::menu::layout::{self, Align, Element, Icon, Layout, Look, ScreenPage, Style, Tint};
use orbiters_orb::menu::Menu;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, widgets};

/// The page's background.
pub const CARD: Color32 = theme::INK_900;

/// What changes on a page, for the overlay.
pub struct View<'a> {
    pub menu: &'a Menu,
    /// Element indices of the page's layout.
    pub hover: Option<usize>,
    pub pressed: Option<usize>,
    /// A result's thumbnail by video id, once loaded.
    pub picture: &'a dyn Fn(&str) -> Option<TextureId>,
    /// Seconds, for what moves (Orb listening).
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
    }
}

pub fn tint(t: Tint) -> Color32 {
    match t {
        Tint::Green => theme::ACCENT,
        Tint::Red => theme::REC,
        Tint::Blue => theme::BLUE,
        Tint::Violet => theme::VIOLET,
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
        Look::Accent => theme::ACCENT,
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
            let tile = Rect::from_min_size(r.min + vec2(24.0, 24.0), vec2(72.0, 72.0));
            p.rect_filled(tile, theme::radius(20.0), theme::alpha(tint, 0.16));
            if let Some(icon) = e.icon {
                widgets::icon(p, tile.center(), glyph(icon), 40.0, tint);
            }
            if let Some(title) = e.label {
                widgets::text(p, r.min + vec2(24.0, 232.0), Align2::LEFT_TOP, &Txt::new(title, 30.0).weight(weight::SEMIBOLD).color(theme::FG));
            }
        }
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
        Look::Chip => icon_and_label(p, r, e.icon, e.label, 22.0, weight::MEDIUM, ink(e.look)),
        Look::Accent | Look::Danger => icon_and_label(p, r, e.icon, e.label, if e.rect.h >= 64 { 26.0 } else { 21.0 }, weight::SEMIBOLD, ink(e.look)),
    }
    // A key's character (its label when it has none).
    if let (Look::Key, orbiters_orb::menu::Target::Key(c), None) = (e.look, e.target, e.label) {
        widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(c.to_string(), 28.0).color(theme::FG));
    }
}

/// The card, the page's pictures and its fixed labels.
fn backdrop(p: &Painter, origin: Pos2, layout: &Layout, listening: Option<f64>) {
    let card = Rect::from_min_size(origin, vec2(layout::WIDTH as f32, layout::HEIGHT as f32));
    p.rect_filled(card, theme::radius(layout::CARD_RADIUS as f32), CARD);
    for (r, icon, t) in &layout.icons {
        let r = rect(*r, origin);
        let color = t.map(tint);
        p.circle_filled(r.center(), r.width() / 2.0, color.map_or(theme::INK_750, |c| theme::alpha(c, 0.16)));
        widgets::icon(p, r.center(), glyph(*icon), r.width() * 0.5, color.unwrap_or(theme::FG_3));
        // Orb listening: the microphone breathes.
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
        if view.pressed == Some(i) {
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
        if let Some(video) = menu.page_results().get(pic.result as usize) {
            picture(p, rect(pic.rect, origin), (view.picture)(&video.id));
        }
    }
}
