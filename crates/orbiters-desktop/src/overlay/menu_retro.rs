//! N4N0's menu in its retro look (contracts `Docs/orb.md`, Retro look): a cassette-futurism terminal, the same pages and
//! layout as Orbiters' own look (so pointing, hovering and the screen's memory stay as they are) drawn as phosphor-green
//! line work on black. Panels have cut corners instead of round ones, labels are monospace capitals, icons sit in corner
//! brackets, a dot grid runs behind everything and the page has a double frame with tick marks. Pictures are green dots.
//!
//! Its colours come from a theme's palette ([`RetroPalette`]: the retro base theme gives the phosphor's own), its texts
//! from the skin's strings, fitted to their places as in Orbiters' own look.

use super::menu_ui::{fit, glyph, Place, Skin, View};
use egui::epaint::{PathShape, PathStroke};
use egui::{pos2, vec2, Align2, Color32, ColorImage, Painter, Pos2, Rect, Shape, Stroke, TextureId};
use orbiters_orb::menu::layout::{self, Align, Element, Icon, Label, Layout, Look, ScreenPage, Style, Tint};
use orbiters_orb::menu::strings::Text;
use orbiters_orb::menu::theme::{Colors, Rgb};
use orbiters_ui::theme::Txt;
use orbiters_ui::widgets;

/// The phosphor palette (sRGB): the retro base theme's colours and those it derives. The avatar's retro screen draws its
/// live parts with the same colours.
pub const BG: Color32 = Color32::from_rgb(2, 10, 4);
pub const GRID: Color32 = Color32::from_rgb(14, 44, 23);
pub const PANEL: Color32 = Color32::from_rgb(5, 24, 11);
pub const DIM: Color32 = Color32::from_rgb(24, 98, 50);
pub const LINE: Color32 = Color32::from_rgb(43, 227, 106);
pub const TEXT: Color32 = Color32::from_rgb(125, 255, 166);
pub const TEXT2: Color32 = Color32::from_rgb(47, 184, 94);
pub const HOT: Color32 = Color32::from_rgb(200, 255, 215);
pub const ALERT: Color32 = Color32::from_rgb(255, 77, 61);
/// The phosphor's own palette: the retro base theme's ([`RetroPalette::new`] of `Theme::RETRO`).
pub const BASE: RetroPalette = RetroPalette { bg: BG, grid: GRID, panel: PANEL, dim: DIM, line: LINE, text: TEXT, text2: TEXT2, hot: HOT, alert: ALERT };

/// How far the page's corners are cut.
pub const PAGE_CUT: f32 = 20.0;
/// The dot grid's pitch.
const GRID_STEP: f32 = 16.0;
/// What a label keeps clear of its element's sides.
const PAD: f32 = 8.0;
/// A character of the monospace capitals, in ems of the text's size (Cascadia Mono's advance at 95 %, and the tracking).
pub const EM: f32 = 0.95 * 0.6 + 0.04;

/// The colours the retro look draws with, from a theme's palette: the ground (`background`), panels (`surface`), lines
/// (`accent`), texts (`text`, `muted`) and alerts (`red`); the dot grid and dim lines sit between the ground and the
/// lines, the hot colour (pressed, hovered, chosen) between the text and white.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetroPalette {
    pub bg: Color32,
    pub grid: Color32,
    pub panel: Color32,
    pub dim: Color32,
    pub line: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub hot: Color32,
    pub alert: Color32,
}

impl RetroPalette {
    pub fn new(c: &Colors) -> RetroPalette {
        let rgb = |c: Rgb| Color32::from_rgb(c[0], c[1], c[2]);
        let between = super::menu_ui::between;
        RetroPalette {
            bg: rgb(c.background),
            grid: between(BG, LINE, GRID, c.background, c.accent),
            panel: rgb(c.surface),
            dim: between(BG, LINE, DIM, c.background, c.accent),
            line: rgb(c.accent),
            text: rgb(c.text),
            text2: rgb(c.muted),
            hot: between(TEXT, Color32::WHITE, HOT, c.text, [255; 3]),
            alert: rgb(c.red),
        }
    }

    fn style(&self, style: Style) -> Color32 {
        match style {
            Style::Primary => self.text,
            Style::Secondary => self.text2,
            Style::Accent => self.line,
        }
    }

    /// The resting colours of an element of `look`.
    fn ink(&self, look: Look) -> Ink {
        let p = self;
        match look {
            Look::Accent | Look::Bar => Ink { fill: p.line, stroke: p.line, text: p.bg, icon: p.bg },
            Look::Danger => Ink { fill: p.alert, stroke: p.alert, text: p.bg, icon: p.bg },
            Look::KeyDark => Ink { fill: p.bg, stroke: p.dim, text: p.text2, icon: p.text2 },
            Look::Row => Ink { fill: Color32::TRANSPARENT, stroke: p.dim, text: p.text2, icon: p.line },
            Look::Chip => Ink { fill: p.panel, stroke: p.dim, text: p.text2, icon: p.text2 },
            Look::Key => Ink { fill: p.panel, stroke: p.dim, text: p.text, icon: p.text },
            Look::Tile | Look::Field | Look::Round => Ink { fill: p.panel, stroke: p.line, text: p.text, icon: p.line },
            Look::Area => Ink { fill: p.panel, stroke: p.dim, text: p.text, icon: p.line },
        }
    }

    /// The same element pressed: lit, what is written on it in the ground's colour.
    fn pressed(&self, look: Look) -> Ink {
        match look {
            Look::Danger => Ink { fill: self.hot, stroke: self.hot, text: self.alert, icon: self.alert },
            _ => Ink { fill: self.line, stroke: self.hot, text: self.bg, icon: self.bg },
        }
    }
}

/// The colours an element is drawn with: as it rests, or inverted while pressed.
#[derive(Clone, Copy)]
struct Ink {
    fill: Color32,
    stroke: Color32,
    text: Color32,
    icon: Color32,
}

fn rect(r: layout::Rect, origin: Pos2) -> Rect {
    Rect::from_min_size(origin + vec2(r.x as f32, r.y as f32), vec2(r.w as f32, r.h as f32))
}

/// A rectangle's outline with its corners cut by `cut`, clockwise from the top left.
pub fn chamfer(r: Rect, cut: f32) -> Vec<Pos2> {
    let c = cut.min(r.width() / 2.0).min(r.height() / 2.0);
    vec![
        pos2(r.min.x + c, r.min.y),
        pos2(r.max.x - c, r.min.y),
        pos2(r.max.x, r.min.y + c),
        pos2(r.max.x, r.max.y - c),
        pos2(r.max.x - c, r.max.y),
        pos2(r.min.x + c, r.max.y),
        pos2(r.min.x, r.max.y - c),
        pos2(r.min.x, r.min.y + c),
    ]
}

fn panel(p: &Painter, r: Rect, cut: f32, fill: Color32, stroke: Stroke) {
    p.add(Shape::Path(PathShape { points: chamfer(r, cut), closed: true, fill, stroke: PathStroke::from(stroke) }));
}

/// L-shaped marks on a rectangle's four corners.
fn brackets(p: &Painter, r: Rect, length: f32, stroke: Stroke) {
    for (corner, dx, dy) in [(r.left_top(), 1.0, 1.0), (r.right_top(), -1.0, 1.0), (r.right_bottom(), -1.0, -1.0), (r.left_bottom(), 1.0, -1.0)] {
        p.add(Shape::line(vec![corner + vec2(dx * length, 0.0), corner, corner + vec2(0.0, dy * length)], stroke));
    }
}

/// Diagonal stripes across `r` (a warning tape, a level bar's empty part).
fn hatch(p: &Painter, r: Rect, spacing: f32, stroke: Stroke) {
    let clipped = p.with_clip_rect(r.intersect(p.clip_rect()));
    let mut x = r.min.x - r.height();
    while x < r.max.x {
        clipped.line_segment([pos2(x, r.max.y), pos2(x + r.height(), r.min.y)], stroke);
        x += spacing;
    }
}

/// A row of small level segments, the first `lit` of them lit.
fn segments(p: &Painter, at: Pos2, count: usize, lit: usize, size: egui::Vec2, gap: f32, pal: &RetroPalette) {
    for i in 0..count {
        let r = Rect::from_min_size(at + vec2(i as f32 * (size.x + gap), 0.0), size);
        if i < lit {
            p.rect_filled(r, 0.0, pal.line);
        } else {
            p.rect_stroke(r, 0.0, Stroke::new(1.0, pal.dim), egui::StrokeKind::Inside);
        }
    }
}

fn mono(text: impl AsRef<str>, size: f32, color: Color32) -> Txt {
    Txt::new(text.as_ref().to_uppercase(), size).mono().color(color).tracking(0.04)
}

/// Where nothing of the page's own is: decorations go only there.
fn free(layout: &Layout, r: Rect, origin: Pos2) -> bool {
    let busy = layout
        .elements
        .iter()
        .map(|e| rect(e.rect, origin))
        .chain(layout.slots.iter().map(|s| rect(s.rect(), origin)))
        .chain(layout.pictures.iter().map(|pic| rect(pic.rect, origin)))
        .chain(layout.icons.iter().map(|(r, _, _)| rect(*r, origin)))
        .chain(layout.boxes.iter().map(|b| rect(*b, origin)))
        .any(|b| b.expand(4.0).intersects(r));
    !busy
}

/// The plate on the frame's top line (the system's name and the screen's version), between its hatched tapes.
const SYSTEM_PLACE: Place = Place { room: 188.0, size: 11.0 };
/// The memory plate at the bottom right.
const MEMORY_PLACE: Place = Place { room: 150.0, size: 11.0 };

/// The page's ground and frame: black with a dot grid, a double frame with cut corners, tick marks and its label plates.
fn frame(p: &Painter, origin: Pos2, layout: Option<&Layout>, skin: &Skin) {
    let pal = &skin.retro;
    let card = Rect::from_min_size(origin, vec2(layout::WIDTH as f32, layout::HEIGHT as f32));
    panel(p, card, PAGE_CUT, pal.bg, Stroke::NONE);
    let mut y = card.min.y + GRID_STEP / 2.0;
    while y < card.max.y {
        let mut x = card.min.x + GRID_STEP / 2.0;
        while x < card.max.x {
            p.rect_filled(Rect::from_center_size(pos2(x, y), vec2(2.0, 2.0)), 0.0, pal.grid);
            x += GRID_STEP;
        }
        y += GRID_STEP;
    }
    panel(p, card.shrink(4.0), PAGE_CUT - 2.0, Color32::TRANSPARENT, Stroke::new(2.0, pal.line));
    panel(p, card.shrink(10.0), PAGE_CUT - 5.0, Color32::TRANSPARENT, Stroke::new(1.0, pal.dim));
    // Ticks along the top and bottom edges, between the two lines.
    let mut x = card.min.x + 64.0;
    while x < card.max.x - 64.0 {
        let long = ((x - card.min.x) as i32 / 32) % 4 == 2;
        let len = if long { 5.0 } else { 3.0 };
        p.line_segment([pos2(x, card.max.y - 10.0), pos2(x, card.max.y - 10.0 - len)], Stroke::new(1.0, pal.dim));
        x += 32.0;
    }
    // A plate on the top line: the system's name, and hatched tape beside it.
    let plate = Rect::from_center_size(pos2(card.center().x, card.min.y + 5.0), vec2(176.0, 12.0));
    p.rect_filled(plate.expand2(vec2(6.0, 2.0)), 0.0, pal.bg);
    let system = format!("// {} {} //", skin.text(Text::RetroSystem), orbiters_contracts::orb::SCREEN_LATEST);
    widgets::text(p, plate.center(), Align2::CENTER_CENTER, &fit(p, mono(&system, SYSTEM_PLACE.size, pal.text2), SYSTEM_PLACE.room).txt);
    for side in [-1.0f32, 1.0] {
        let tape = Rect::from_center_size(pos2(card.center().x + side * 128.0, card.min.y + 5.0), vec2(64.0, 7.0));
        p.rect_filled(tape.expand(1.0), 0.0, pal.bg);
        hatch(p, tape, 6.0, Stroke::new(1.5, pal.dim));
    }
    // Small crosshairs in the corners, where nothing of the page reaches.
    for corner in [
        card.left_top() + vec2(17.0, 17.0),
        card.right_top() + vec2(-17.0, 17.0),
        card.left_bottom() + vec2(17.0, -17.0),
        card.right_bottom() + vec2(-17.0, -17.0),
    ] {
        let r = Rect::from_center_size(corner, vec2(10.0, 10.0));
        if layout.is_none_or(|l| free(l, r, origin)) {
            p.line_segment([corner - vec2(4.0, 0.0), corner + vec2(4.0, 0.0)], Stroke::new(1.0, pal.text2));
            p.line_segment([corner - vec2(0.0, 4.0), corner + vec2(0.0, 4.0)], Stroke::new(1.0, pal.text2));
        }
    }
    // The memory plate at the bottom right and a level bar at the bottom left, when the page leaves them room.
    let memory = Rect::from_min_size(card.right_bottom() + vec2(-178.0, -30.0), vec2(150.0, 14.0));
    if layout.is_none_or(|l| free(l, memory, origin)) {
        let txt = fit(p, mono(skin.text(Text::RetroMemory), MEMORY_PLACE.size, pal.dim), MEMORY_PLACE.room).txt;
        widgets::text(p, memory.right_center(), Align2::RIGHT_CENTER, &txt);
    }
    let level = Rect::from_min_size(card.left_bottom() + vec2(30.0, -28.0), vec2(96.0, 10.0));
    if layout.is_none_or(|l| free(l, level, origin)) {
        segments(p, level.min, 8, 5, vec2(9.0, 8.0), 3.0, pal);
    }
}

/// An icon in corner brackets.
fn bracketed_icon(p: &Painter, r: Rect, icon: Icon, size: f32, ink: Ink) {
    brackets(p, r, (r.width() * 0.22).max(6.0), Stroke::new(1.5, ink.stroke));
    widgets::icon(p, r.center(), glyph(icon), size, ink.icon);
}

/// An icon and a label side by side, centred in `r`, the label fitted to its place.
fn icon_and_label(p: &Painter, r: Rect, icon: Option<Icon>, label: Option<&str>, place: Place, ink: Ink) {
    let size = place.size;
    let icon_size = size * 1.1;
    match (icon, label) {
        (Some(i), None) => widgets::icon(p, r.center(), glyph(i), icon_size, ink.icon),
        (None, Some(l)) => {
            widgets::text(p, r.center(), Align2::CENTER_CENTER, &fit(p, mono(l, size, ink.text), place.room).txt);
        }
        (Some(i), Some(l)) => {
            let fitted = fit(p, mono(l, size, ink.text), place.room);
            let gap = size * 0.45;
            let left = r.center().x - (icon_size + gap + fitted.size.x) / 2.0;
            widgets::icon(p, pos2(left + icon_size / 2.0, r.center().y), glyph(i), icon_size, ink.icon);
            widgets::text(p, pos2(left + icon_size + gap, r.center().y), Align2::LEFT_CENTER, &fitted.txt);
        }
        (None, None) => {}
    }
}

/// The room beside an icon of a label of `size` in an element `width` wide (the icon `size × 1.1`, then a gap).
fn beside_icon(width: f32, icon: bool, size: f32) -> f32 {
    width - 2.0 * PAD - if icon { size * 1.1 + size * 0.45 } else { 0.0 }
}

/// Where an element's label goes in the retro look (none when it has none).
pub fn element_place(e: &Element) -> Option<Place> {
    e.label.and(element_metrics(e))
}

/// The room and size of what is written on an element of its look, its icon sized from it too (none for looks that
/// write nothing).
fn element_metrics(e: &Element) -> Option<Place> {
    let w = e.rect.w as f32;
    let icon = e.icon.is_some();
    Some(match e.look {
        Look::Tile if super::menu_ui::tile_is_compact(e.rect) => Place { room: w - 98.0, size: 21.0 },
        Look::Tile if super::menu_ui::tile_is_wide(e.rect) => Place { room: w - 132.0, size: 27.0 },
        Look::Tile => Place { room: w - 40.0, size: 27.0 },
        Look::Key => Place { room: beside_icon(w, icon, 20.0), size: 20.0 },
        Look::KeyDark => Place { room: beside_icon(w, icon, 18.0), size: 18.0 },
        // A light colour's chip: its swatch before its name.
        Look::Chip if e.tint.is_some() => Place { room: w - 2.0 * PAD - 14.0 - 10.0, size: 18.0 },
        Look::Chip => Place { room: beside_icon(w, icon, 18.0), size: 18.0 },
        Look::Accent | Look::Danger => {
            let size = if e.rect.h >= 64 { 22.0 } else { 18.0 };
            Place { room: beside_icon(w, icon, size), size }
        }
        Look::Round | Look::Field | Look::Row | Look::Bar | Look::Area => return None,
    })
}

/// Where a page's label goes in the retro look (a tenth smaller than in Orbiters' own).
pub fn label_place(label: &Label) -> Place {
    Place { room: label.width as f32, size: label.size as f32 * 0.9 }
}

/// The back's title (in slashes) and hint (a prompt), centred, clear of the card's sides.
const BACK_TITLE: Place = Place { room: layout::WIDTH as f32 - 96.0, size: 44.0 };
const BACK_HINT: Place = Place { room: layout::WIDTH as f32 - 96.0, size: 22.0 };

/// A big icon's caption, under it.
fn model_place(r: Rect) -> Place {
    Place { room: r.width(), size: 12.0 }
}

/// "No signal" in a picture's place.
fn no_signal_place(r: Rect) -> Place {
    Place { room: r.width() - 20.0, size: (r.height() * 0.14).clamp(10.0, 22.0) }
}

/// Every place a text has in the retro look, for the room its key's own text has (the slashes, the prompt and the
/// version around some take part of theirs).
pub fn places() -> Vec<(Text, Place)> {
    let less = |place: Place, chars: f32| Place { room: place.room - chars * EM * place.size, size: place.size };
    let mut out = vec![
        (Text::BackTitle, less(BACK_TITLE, 6.0)),
        (Text::BackHint, less(BACK_HINT, 3.0)),
        (Text::RetroSystem, less(SYSTEM_PLACE, 8.0)),
        (Text::RetroMemory, MEMORY_PLACE),
    ];
    for page in ScreenPage::ALL {
        let l = layout::cached(page);
        out.extend(l.labels.iter().map(|label| (label.text, label_place(label))));
        out.extend(l.elements.iter().filter_map(|e| Some((e.label?, element_place(e)?))));
        out.extend(l.icons.iter().map(|(r, _, _)| rect(*r, Pos2::ZERO)).filter(|r| r.width() > 120.0).map(|r| (Text::RetroModel, model_place(r))));
        out.extend(l.pictures.iter().map(|pic| (Text::RetroNoSignal, no_signal_place(rect(pic.rect, Pos2::ZERO)))));
    }
    out
}

/// One element's look, with `ink`'s colours.
fn element(p: &Painter, e: &Element, origin: Pos2, ink: Ink, skin: &Skin) {
    let pal = &skin.retro;
    let r = rect(e.rect, origin);
    let small = r.height() < 64.0;
    let cut = if small { 7.0 } else { 14.0 };
    match e.look {
        Look::Row => {
            // A dotted rule under the row.
            let mut x = r.min.x + 8.0;
            while x < r.max.x - 8.0 {
                p.rect_filled(Rect::from_min_size(pos2(x, r.max.y - 2.0), vec2(3.0, 1.5)), 0.0, ink.stroke);
                x += 7.0;
            }
            if ink.fill != Color32::TRANSPARENT {
                p.rect_filled(r, 0.0, ink.fill);
            }
        }
        _ => panel(p, r, cut, ink.fill, Stroke::new(if e.look == Look::Tile { 2.0 } else { 1.5 }, ink.stroke)),
    }
    let label = e.label.map(|t| skin.text(t));
    let place = element_metrics(e).unwrap_or(Place { room: r.width(), size: 18.0 });
    match e.look {
        Look::Tile if super::menu_ui::tile_is_compact(e.rect) => {
            // A small tile: the icon at its top left, the title beside it.
            let box_ = Rect::from_min_size(r.min + vec2(20.0, 20.0), vec2(56.0, 56.0));
            if let Some(icon) = e.icon {
                let ink = if e.tint == Some(Tint::Red) && ink.icon == pal.line { Ink { icon: pal.alert, ..ink } } else { ink };
                bracketed_icon(p, box_, icon, 30.0, ink);
            }
            if let Some(title) = label {
                let fitted = fit(p, mono(title, place.size, ink.text), place.room);
                widgets::text(p, r.min + vec2(90.0, 34.0 + fitted.dy), Align2::LEFT_TOP, &fitted.txt);
            }
        }
        Look::Tile => {
            // As Orbiters' own tile: the icon at its top (a tall tile) or left (a wide one), the title below or beside it.
            let wide = super::menu_ui::tile_is_wide(e.rect);
            let box_ = Rect::from_min_size(r.min + if wide { vec2(24.0, (r.height() - 72.0) / 2.0) } else { vec2(24.0, 24.0) }, vec2(72.0, 72.0));
            if let Some(icon) = e.icon {
                let ink = if e.tint == Some(Tint::Red) && ink.icon == pal.line { Ink { icon: pal.alert, ..ink } } else { ink };
                bracketed_icon(p, box_, icon, 40.0, ink);
            }
            if let Some(title) = label {
                let fitted = fit(p, mono(title, place.size, ink.text), place.room);
                let at = r.min + if wide { vec2(116.0, 38.0) } else { vec2(24.0, r.height() - 96.0) };
                widgets::text(p, at + vec2(0.0, fitted.dy), Align2::LEFT_TOP, &fitted.txt);
            }
            // A level bar in the top right corner: the tile's own instrument.
            segments(p, pos2(r.max.x - 66.0, r.min.y + 14.0), 4, 2, vec2(9.0, 6.0), 3.0, pal);
        }
        Look::Field => {
            // A terminal prompt where Orbiters' own look has its magnifier.
            widgets::text(p, pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, &mono(">", 28.0, ink.icon));
        }
        Look::Row => match e.icon {
            Some(Icon::User) => {
                let at = Rect::from_center_size(pos2(r.min.x + 44.0, r.center().y), vec2(44.0, 44.0));
                bracketed_icon(p, at, Icon::User, 24.0, ink);
            }
            Some(icon) => widgets::icon(p, pos2(r.max.x - 28.0, r.center().y), glyph(icon), 22.0, ink.icon),
            None => {}
        },
        Look::Key | Look::KeyDark => icon_and_label(p, r, e.icon, label, place, ink),
        Look::Round => icon_and_label(p, r, e.icon, None, Place { room: r.width(), size: 24.0 }, ink),
        Look::Chip => match (e.tint, label) {
            // A light colour's chip keeps its swatch: the colour is what it is about.
            (Some(t), Some(label)) => {
                let fitted = fit(p, mono(label, place.size, ink.text), place.room);
                let (dot, gap) = (14.0, 10.0);
                let left = r.center().x - (dot + gap + fitted.size.x) / 2.0;
                let (square, swatch) = (Rect::from_center_size(pos2(left + dot / 2.0, r.center().y), vec2(dot, dot)), super::menu_ui::tint(t));
                p.rect_filled(square, 0.0, swatch);
                if super::menu_ui::alike(swatch, ink.fill) {
                    p.rect_stroke(square, 0.0, Stroke::new(1.0, pal.dim), egui::StrokeKind::Inside);
                }
                widgets::text(p, pos2(left + dot + gap, r.center().y), Align2::LEFT_CENTER, &fitted.txt);
            }
            _ => icon_and_label(p, r, e.icon, label, place, ink),
        },
        Look::Accent | Look::Danger => icon_and_label(p, r, e.icon, label, place, ink),
        // A countdown's segment: lit, nothing on it; a text area is its panel.
        Look::Bar | Look::Area => {}
    }
    if let (Look::Key, orbiters_orb::menu::Target::Key(c), None) = (e.look, e.target, e.label) {
        widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(c.to_string(), 28.0).mono().color(ink.text));
    }
}

/// The page's own icons (N4N0's microphone, the 3D picture's cube), its panels and labels.
fn furniture(p: &Painter, origin: Pos2, layout: &Layout, skin: &Skin) {
    let pal = &skin.retro;
    // What N4N0 says: a panel as a tile's, N4N0 as a ring with its eyes at its top left.
    for b in &layout.boxes {
        let r = rect(*b, origin);
        panel(p, r, 14.0, pal.panel, Stroke::new(1.5, pal.dim));
        let orb = r.min + vec2(38.0, 36.0);
        p.circle_stroke(orb, 13.0, Stroke::new(1.5, pal.line));
        for side in [-1.0, 1.0] {
            p.rect_filled(Rect::from_center_size(orb + vec2(side * 4.5, -1.0), vec2(3.0, 6.0)), 0.0, pal.line);
        }
    }
    for (r, icon, t) in &layout.icons {
        let r = rect(*r, origin);
        if *icon == Icon::Mark {
            orbiters_ui::n4n0_logo::paint(p, Rect::from_center_size(r.center(), orbiters_ui::n4n0_logo::size_for_width(r.width())), pal.line);
            continue;
        }
        let color = if *t == Some(Tint::Red) { pal.alert } else { pal.line };
        if r.width() > 120.0 {
            // A big icon is a schematic: its crosshair and a dashed ring around it.
            let c = r.center();
            for (a, b) in
                [(c - vec2(r.width() * 0.62, 0.0), c + vec2(r.width() * 0.62, 0.0)), (c - vec2(0.0, r.height() * 0.62), c + vec2(0.0, r.height() * 0.62))]
            {
                p.line_segment([a, b], Stroke::new(1.0, pal.dim));
            }
            let radius = r.width() * 0.56;
            for i in 0..36 {
                if i % 3 == 2 {
                    continue;
                }
                let (a0, a1) = (i as f32 / 36.0 * std::f32::consts::TAU, (i as f32 + 0.6) / 36.0 * std::f32::consts::TAU);
                p.line_segment([c + vec2(a0.cos(), a0.sin()) * radius, c + vec2(a1.cos(), a1.sin()) * radius], Stroke::new(1.5, pal.dim));
            }
            widgets::icon(p, c, glyph(*icon), r.width() * 0.5, color);
            let place = model_place(r);
            let caption = fit(p, mono(skin.text(Text::RetroModel), place.size, pal.text2), place.room);
            widgets::text(p, pos2(c.x, c.y + radius + 14.0), Align2::CENTER_TOP, &caption.txt);
        } else {
            bracketed_icon(p, r, *icon, r.width() * 0.5, Ink { fill: pal.panel, stroke: pal.dim, text: pal.text, icon: color });
        }
    }
    for label in &layout.labels {
        let place = label_place(label);
        let color = if label.bold { pal.text } else { pal.style(label.style) };
        let fitted = fit(p, mono(skin.text(label.text), place.size, color), place.room);
        widgets::text(p, origin + vec2(label.x as f32, label.y as f32 + fitted.dy), Align2::LEFT_TOP, &fitted.txt);
    }
}

/// Everything on a page that never changes, in the retro look: the avatar's retro screen has it as the page's picture.
pub fn draw_chrome(p: &Painter, origin: Pos2, layout: &Layout, skin: &Skin) {
    frame(p, origin, Some(layout), skin);
    furniture(p, origin, layout, skin);
    for e in &layout.elements {
        element(p, e, origin, skin.retro.ink(e.look), skin);
    }
    if layout.page == ScreenPage::Music {
        for pic in &layout.pictures {
            no_picture(p, rect(pic.rect, origin), skin);
        }
    }
}

/// A picture's place while it has none: hatched, "NO SIGNAL".
fn no_picture(p: &Painter, r: Rect, skin: &Skin) {
    let pal = &skin.retro;
    p.rect_filled(r, 0.0, pal.panel);
    hatch(p, r, 10.0, Stroke::new(1.0, pal.grid));
    brackets(p, r, 10.0, Stroke::new(1.5, pal.dim));
    let place = no_signal_place(r);
    widgets::text(p, r.center(), Align2::CENTER_CENTER, &fit(p, mono(skin.text(Text::RetroNoSignal), place.size, pal.dim), place.room).txt);
}

/// A slot's text, as the overlay writes it: in the monospace font the avatar's screen writes it in.
fn slot(p: &Painter, s: &layout::Slot, text: &str, origin: Pos2, pal: &RetroPalette) {
    if text.is_empty() {
        return;
    }
    let r = rect(s.rect(), origin);
    let txt = Txt::new(text, s.size as f32 * 0.9).mono().color(pal.style(s.style));
    match s.align {
        Align::Left => widgets::text_truncated(p, pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, &txt, r.width()),
        Align::Center => widgets::text_truncated(p, r.center(), Align2::CENTER_CENTER, &txt, r.width()),
    };
}

/// A picture, already turned into green dots ([`dots`]), in its brackets.
fn picture(p: &Painter, r: Rect, texture: Option<TextureId>, skin: &Skin) {
    match texture {
        Some(id) => {
            p.add(egui::epaint::RectShape::filled(r, 0.0, Color32::WHITE).with_texture(id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
            brackets(p, r.expand(3.0), 10.0, Stroke::new(1.5, skin.retro.line));
        }
        None => no_picture(p, r, skin),
    }
}

/// Thin dark lines over every third row, as on a cathode-ray tube (the avatar's screen draws them itself).
fn scanlines(p: &Painter, origin: Pos2) {
    let mut y = origin.y + 2.0;
    while y < origin.y + layout::HEIGHT as f32 {
        p.rect_filled(Rect::from_min_size(pos2(origin.x, y), vec2(layout::WIDTH as f32, 1.0)), 0.0, Color32::from_black_alpha(70));
        y += 3.0;
    }
}

/// The whole page as the overlay shows it, in the retro look.
pub fn draw(p: &Painter, origin: Pos2, view: &View) {
    let (menu, skin) = (view.menu, view.skin);
    let pal = &skin.retro;
    let page = ScreenPage::of(menu);
    let layout = layout::cached(page);
    frame(p, origin, Some(layout), skin);
    furniture(p, origin, layout, skin);
    for (i, e) in layout.elements.iter().enumerate() {
        let state = layout::state(menu, e);
        if state.hidden {
            continue;
        }
        let r = rect(e.rect, origin);
        let down = view.pressed == Some(i);
        element(p, e, origin, if down { pal.pressed(e.look) } else { pal.ink(e.look) }, skin);
        if state.unavailable {
            p.rect_filled(r, 0.0, pal.bg.gamma_multiply(0.6));
            hatch(p, r, 12.0, Stroke::new(1.0, pal.grid));
        } else if !down && view.hover == Some(i) {
            panel(p, r.expand(3.0), if r.height() < 64.0 { 8.0 } else { 15.0 }, pal.line.gamma_multiply(0.10), Stroke::new(2.5, pal.hot));
        }
        if state.on {
            brackets(p, r.expand(6.0), 14.0, Stroke::new(3.0, pal.hot));
        }
    }
    for s in &layout.slots {
        slot(p, s, &layout::slot_text(menu, s), origin, pal);
    }
    for pic in &layout.pictures {
        let r = rect(pic.rect, origin);
        match (layout::picture_key(menu, pic), menu.page) {
            (Some(key), _) => picture(p, r, (view.picture)(key), skin),
            (None, orbiters_orb::menu::Page::Music) => no_picture(p, r, skin),
            (None, _) => {}
        }
    }
    scanlines(p, origin);
}

/// The back, in the retro look: "N4N0 menu, look at the other side".
pub fn draw_back(p: &Painter, origin: Pos2, skin: &Skin) {
    let pal = &skin.retro;
    frame(p, origin, None, skin);
    let card = Rect::from_min_size(origin, vec2(layout::WIDTH as f32, layout::HEIGHT as f32));
    let c = card.center();
    // N4N0 as a schematic: a ring with its eyes, in brackets.
    let orb = c - vec2(0.0, 92.0);
    p.circle_stroke(orb, 30.0, Stroke::new(2.0, pal.line));
    p.circle_filled(orb, 22.0, pal.panel);
    for side in [-1.0, 1.0] {
        p.rect_filled(Rect::from_center_size(orb + vec2(side * 9.0, -2.0), vec2(6.0, 12.0)), 0.0, pal.line);
    }
    brackets(p, Rect::from_center_size(orb, vec2(84.0, 84.0)), 12.0, Stroke::new(1.5, pal.dim));
    let title = fit(p, mono(format!("// {} //", skin.text(Text::BackTitle)), BACK_TITLE.size, pal.text), BACK_TITLE.room);
    widgets::text(p, pos2(c.x, c.y + 2.0), Align2::CENTER_CENTER, &title.txt);
    let hint = fit(p, mono(format!("> {}_", skin.text(Text::BackHint)), BACK_HINT.size, pal.text2), BACK_HINT.room);
    widgets::text(p, pos2(c.x, c.y + 54.0), Align2::CENTER_CENTER, &hint.txt);
    hatch(p, Rect::from_center_size(pos2(c.x, c.y + 100.0), vec2(360.0, 10.0)), 8.0, Stroke::new(1.5, pal.dim));
}

/// A thumbnail as the retro look shows it (contracts `Docs/orb.md`, Retro look): its light, levels stretched, as 96 × 54
/// green dots in four steps with an ordered 4 × 4 dither, each dot square with a dark gap, `scale` pixels apart (in the
/// phosphor's own colours).
pub fn dots(image: &ColorImage, scale: usize) -> ColorImage {
    let (w, h) = (orbiters_orb::menu::retro::DOTS_X, orbiters_orb::menu::retro::DOTS_Y);
    let light = orbiters_orb::menu::retro::lights(image.size[0], image.size[1], |x, y| {
        let c = image.pixels[y * image.size[0] + x];
        [c.r(), c.g(), c.b()]
    });
    let steps = [BASE.bg, BASE.dim, BASE.line, BASE.hot];
    let mut out = ColorImage::filled([w * scale, h * scale], BASE.bg);
    for y in 0..h {
        for x in 0..w {
            let level = orbiters_orb::menu::retro::step(light[y * w + x], x, y);
            for dy in 0..scale.saturating_sub(1).max(1) {
                for dx in 0..scale.saturating_sub(1).max(1) {
                    out.pixels[(y * scale + dy) * w * scale + x * scale + dx] = steps[level];
                }
            }
        }
    }
    out
}
