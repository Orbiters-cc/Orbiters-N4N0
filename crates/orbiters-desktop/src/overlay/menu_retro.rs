//! N4N0's menu in its retro look (contracts `Docs/orb.md`, Retro look): a cassette-futurism terminal, the same pages and
//! layout as Orbiters' own look (so pointing, hovering and the screen's memory stay as they are) drawn as phosphor-green
//! line work on black. Panels have cut corners instead of round ones, labels are monospace capitals, icons sit in corner
//! brackets, a dot grid runs behind everything and the page has a double frame with tick marks. Pictures are green dots.

use super::menu_ui::glyph;
use egui::epaint::{PathShape, PathStroke};
use egui::{pos2, vec2, Align2, Color32, ColorImage, Painter, Pos2, Rect, Shape, Stroke, TextureId};
use orbiters_orb::menu::layout::{self, Align, Element, Icon, Layout, Look, ScreenPage, Style, Tint};
use orbiters_ui::theme::Txt;
use orbiters_ui::widgets;

/// The phosphor palette (sRGB). The avatar's retro screen draws its live parts with the same colours.
pub const BG: Color32 = Color32::from_rgb(2, 10, 4);
pub const GRID: Color32 = Color32::from_rgb(14, 44, 23);
pub const PANEL: Color32 = Color32::from_rgb(5, 24, 11);
pub const DIM: Color32 = Color32::from_rgb(24, 98, 50);
pub const LINE: Color32 = Color32::from_rgb(43, 227, 106);
pub const TEXT: Color32 = Color32::from_rgb(125, 255, 166);
pub const TEXT2: Color32 = Color32::from_rgb(47, 184, 94);
pub const HOT: Color32 = Color32::from_rgb(200, 255, 215);
pub const ALERT: Color32 = Color32::from_rgb(255, 77, 61);

/// How far the page's corners are cut.
pub const PAGE_CUT: f32 = 20.0;
/// The dot grid's pitch.
const GRID_STEP: f32 = 16.0;

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
fn segments(p: &Painter, at: Pos2, count: usize, lit: usize, size: egui::Vec2, gap: f32) {
    for i in 0..count {
        let r = Rect::from_min_size(at + vec2(i as f32 * (size.x + gap), 0.0), size);
        if i < lit {
            p.rect_filled(r, 0.0, LINE);
        } else {
            p.rect_stroke(r, 0.0, Stroke::new(1.0, DIM), egui::StrokeKind::Inside);
        }
    }
}

fn mono(text: impl AsRef<str>, size: f32, color: Color32) -> Txt {
    Txt::new(text.as_ref().to_uppercase(), size).mono().color(color).tracking(0.04)
}

fn style_color(style: Style) -> Color32 {
    match style {
        Style::Primary => TEXT,
        Style::Secondary => TEXT2,
        Style::Accent => LINE,
    }
}

/// Where the page has nothing of its own: decorations go only there.
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

/// The page's ground and frame: black with a dot grid, a double frame with cut corners, tick marks and its label plates.
fn frame(p: &Painter, origin: Pos2, layout: Option<&Layout>) {
    let card = Rect::from_min_size(origin, vec2(layout::WIDTH as f32, layout::HEIGHT as f32));
    panel(p, card, PAGE_CUT, BG, Stroke::NONE);
    let mut y = card.min.y + GRID_STEP / 2.0;
    while y < card.max.y {
        let mut x = card.min.x + GRID_STEP / 2.0;
        while x < card.max.x {
            p.rect_filled(Rect::from_center_size(pos2(x, y), vec2(2.0, 2.0)), 0.0, GRID);
            x += GRID_STEP;
        }
        y += GRID_STEP;
    }
    panel(p, card.shrink(4.0), PAGE_CUT - 2.0, Color32::TRANSPARENT, Stroke::new(2.0, LINE));
    panel(p, card.shrink(10.0), PAGE_CUT - 5.0, Color32::TRANSPARENT, Stroke::new(1.0, DIM));
    // Ticks along the top and bottom edges, between the two lines.
    let mut x = card.min.x + 64.0;
    while x < card.max.x - 64.0 {
        let long = ((x - card.min.x) as i32 / 32) % 4 == 2;
        let len = if long { 5.0 } else { 3.0 };
        p.line_segment([pos2(x, card.max.y - 10.0), pos2(x, card.max.y - 10.0 - len)], Stroke::new(1.0, DIM));
        x += 32.0;
    }
    // A plate on the top line: the system's name, and hatched tape beside it.
    let plate = Rect::from_center_size(pos2(card.center().x, card.min.y + 5.0), vec2(176.0, 12.0));
    p.rect_filled(plate.expand2(vec2(6.0, 2.0)), 0.0, BG);
    let system = format!("// N4N0·OS {} //", orbiters_contracts::orb::SCREEN_LATEST);
    widgets::text(p, plate.center(), Align2::CENTER_CENTER, &mono(&system, 11.0, TEXT2));
    for side in [-1.0f32, 1.0] {
        let tape = Rect::from_center_size(pos2(card.center().x + side * 128.0, card.min.y + 5.0), vec2(64.0, 7.0));
        p.rect_filled(tape.expand(1.0), 0.0, BG);
        hatch(p, tape, 6.0, Stroke::new(1.5, DIM));
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
            p.line_segment([corner - vec2(4.0, 0.0), corner + vec2(4.0, 0.0)], Stroke::new(1.0, TEXT2));
            p.line_segment([corner - vec2(0.0, 4.0), corner + vec2(0.0, 4.0)], Stroke::new(1.0, TEXT2));
        }
    }
    // The memory plate at the bottom right and a level bar at the bottom left, when the page leaves them room.
    let memory = Rect::from_min_size(card.right_bottom() + vec2(-178.0, -30.0), vec2(150.0, 14.0));
    if layout.is_none_or(|l| free(l, memory, origin)) {
        widgets::text(p, memory.right_center(), Align2::RIGHT_CENTER, &mono("MEM 0744 · SYS OK", 11.0, DIM));
    }
    let level = Rect::from_min_size(card.left_bottom() + vec2(30.0, -28.0), vec2(96.0, 10.0));
    if layout.is_none_or(|l| free(l, level, origin)) {
        segments(p, level.min, 8, 5, vec2(9.0, 8.0), 3.0);
    }
}

/// An icon in corner brackets.
fn bracketed_icon(p: &Painter, r: Rect, icon: Icon, size: f32, ink: Ink) {
    brackets(p, r, (r.width() * 0.22).max(6.0), Stroke::new(1.5, ink.stroke));
    widgets::icon(p, r.center(), glyph(icon), size, ink.icon);
}

/// An icon and a label side by side, centred in `r`.
fn icon_and_label(p: &Painter, r: Rect, icon: Option<Icon>, label: Option<&str>, size: f32, ink: Ink) {
    let icon_size = size * 1.1;
    match (icon, label) {
        (Some(i), None) => widgets::icon(p, r.center(), glyph(i), icon_size, ink.icon),
        (None, Some(l)) => {
            widgets::text(p, r.center(), Align2::CENTER_CENTER, &mono(l, size, ink.text));
        }
        (Some(i), Some(l)) => {
            let txt = mono(l, size, ink.text);
            let width = p.layout_job(txt.job(f32::INFINITY)).size().x;
            let gap = size * 0.45;
            let left = r.center().x - (icon_size + gap + width) / 2.0;
            widgets::icon(p, pos2(left + icon_size / 2.0, r.center().y), glyph(i), icon_size, ink.icon);
            widgets::text(p, pos2(left + icon_size + gap, r.center().y), Align2::LEFT_CENTER, &txt);
        }
        (None, None) => {}
    }
}

/// The resting colours of an element of `look`.
fn ink(look: Look) -> Ink {
    match look {
        Look::Accent | Look::Bar => Ink { fill: LINE, stroke: LINE, text: BG, icon: BG },
        Look::Danger => Ink { fill: ALERT, stroke: ALERT, text: BG, icon: BG },
        Look::KeyDark => Ink { fill: BG, stroke: DIM, text: TEXT2, icon: TEXT2 },
        Look::Row => Ink { fill: Color32::TRANSPARENT, stroke: DIM, text: TEXT2, icon: LINE },
        Look::Chip => Ink { fill: PANEL, stroke: DIM, text: TEXT2, icon: TEXT2 },
        Look::Key => Ink { fill: PANEL, stroke: DIM, text: TEXT, icon: TEXT },
        Look::Tile | Look::Field | Look::Round => Ink { fill: PANEL, stroke: LINE, text: TEXT, icon: LINE },
        Look::Area => Ink { fill: PANEL, stroke: DIM, text: TEXT, icon: LINE },
    }
}

/// The same element pressed: lit, what is written on it in the ground's colour.
fn pressed(look: Look) -> Ink {
    match look {
        Look::Danger => Ink { fill: HOT, stroke: HOT, text: ALERT, icon: ALERT },
        _ => Ink { fill: LINE, stroke: HOT, text: BG, icon: BG },
    }
}

/// One element's look, with `ink`'s colours.
fn element(p: &Painter, e: &Element, origin: Pos2, ink: Ink) {
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
    match e.look {
        Look::Tile if super::menu_ui::tile_is_compact(e.rect) => {
            // A small tile: the icon at its top left, the title beside it.
            let box_ = Rect::from_min_size(r.min + vec2(20.0, 20.0), vec2(56.0, 56.0));
            if let Some(icon) = e.icon {
                let ink = if e.tint == Some(Tint::Red) && ink.icon == LINE { Ink { icon: ALERT, ..ink } } else { ink };
                bracketed_icon(p, box_, icon, 30.0, ink);
            }
            if let Some(title) = e.label {
                widgets::text(p, r.min + vec2(90.0, 34.0), Align2::LEFT_TOP, &mono(title, 21.0, ink.text));
            }
        }
        Look::Tile => {
            // As Orbiters' own tile: the icon at its top (a tall tile) or left (a wide one), the title below or beside it.
            let wide = super::menu_ui::tile_is_wide(e.rect);
            let box_ = Rect::from_min_size(r.min + if wide { vec2(24.0, (r.height() - 72.0) / 2.0) } else { vec2(24.0, 24.0) }, vec2(72.0, 72.0));
            if let Some(icon) = e.icon {
                let ink = if e.tint == Some(Tint::Red) && ink.icon == LINE { Ink { icon: ALERT, ..ink } } else { ink };
                bracketed_icon(p, box_, icon, 40.0, ink);
            }
            if let Some(title) = e.label {
                let at = r.min + if wide { vec2(116.0, 38.0) } else { vec2(24.0, r.height() - 96.0) };
                widgets::text(p, at, Align2::LEFT_TOP, &mono(title, 27.0, ink.text));
            }
            // A level bar in the top right corner: the tile's own instrument.
            segments(p, pos2(r.max.x - 66.0, r.min.y + 14.0), 4, 2, vec2(9.0, 6.0), 3.0);
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
        Look::Key => icon_and_label(p, r, e.icon, e.label, 20.0, ink),
        Look::KeyDark => icon_and_label(p, r, e.icon, e.label, 18.0, ink),
        Look::Round => icon_and_label(p, r, e.icon, None, 24.0, ink),
        Look::Chip => match (e.tint, e.label) {
            // A light colour's chip keeps its swatch: the colour is what it is about.
            (Some(t), Some(label)) => {
                let txt = mono(label, 18.0, ink.text);
                let width = p.layout_job(txt.job(f32::INFINITY)).size().x;
                let (dot, gap) = (14.0, 10.0);
                let left = r.center().x - (dot + gap + width) / 2.0;
                p.rect_filled(Rect::from_center_size(pos2(left + dot / 2.0, r.center().y), vec2(dot, dot)), 0.0, super::menu_ui::tint(t));
                widgets::text(p, pos2(left + dot + gap, r.center().y), Align2::LEFT_CENTER, &txt);
            }
            _ => icon_and_label(p, r, e.icon, e.label, 18.0, ink),
        },
        Look::Accent | Look::Danger => icon_and_label(p, r, e.icon, e.label, if e.rect.h >= 64 { 22.0 } else { 18.0 }, ink),
        // A countdown's segment: lit, nothing on it; a text area is its panel.
        Look::Bar | Look::Area => {}
    }
    if let (Look::Key, orbiters_orb::menu::Target::Key(c), None) = (e.look, e.target, e.label) {
        widgets::text(p, r.center(), Align2::CENTER_CENTER, &Txt::new(c.to_string(), 28.0).mono().color(ink.text));
    }
}

/// The page's own icons (N4N0's microphone, the 3D picture's cube), its panels and labels.
fn furniture(p: &Painter, origin: Pos2, layout: &Layout) {
    // What N4N0 says: a panel as a tile's, N4N0 as a ring with its eyes at its top left.
    for b in &layout.boxes {
        let r = rect(*b, origin);
        panel(p, r, 14.0, PANEL, Stroke::new(1.5, DIM));
        let orb = r.min + vec2(38.0, 36.0);
        p.circle_stroke(orb, 13.0, Stroke::new(1.5, LINE));
        for side in [-1.0, 1.0] {
            p.rect_filled(Rect::from_center_size(orb + vec2(side * 4.5, -1.0), vec2(3.0, 6.0)), 0.0, LINE);
        }
    }
    for (r, icon, t) in &layout.icons {
        let r = rect(*r, origin);
        if *icon == Icon::Mark {
            orbiters_ui::n4n0_logo::paint(p, Rect::from_center_size(r.center(), orbiters_ui::n4n0_logo::size_for_width(r.width())), LINE);
            continue;
        }
        let color = if *t == Some(Tint::Red) { ALERT } else { LINE };
        if r.width() > 120.0 {
            // A big icon is a schematic: its crosshair and a dashed ring around it.
            let c = r.center();
            for (a, b) in
                [(c - vec2(r.width() * 0.62, 0.0), c + vec2(r.width() * 0.62, 0.0)), (c - vec2(0.0, r.height() * 0.62), c + vec2(0.0, r.height() * 0.62))]
            {
                p.line_segment([a, b], Stroke::new(1.0, DIM));
            }
            let radius = r.width() * 0.56;
            for i in 0..36 {
                if i % 3 == 2 {
                    continue;
                }
                let (a0, a1) = (i as f32 / 36.0 * std::f32::consts::TAU, (i as f32 + 0.6) / 36.0 * std::f32::consts::TAU);
                p.line_segment([c + vec2(a0.cos(), a0.sin()) * radius, c + vec2(a1.cos(), a1.sin()) * radius], Stroke::new(1.5, DIM));
            }
            widgets::icon(p, c, glyph(*icon), r.width() * 0.5, color);
            widgets::text(p, pos2(c.x, c.y + radius + 14.0), Align2::CENTER_TOP, &mono("MODEL // 3D", 12.0, TEXT2));
        } else {
            bracketed_icon(p, r, *icon, r.width() * 0.5, Ink { fill: PANEL, stroke: DIM, text: TEXT, icon: color });
        }
    }
    for label in &layout.labels {
        let size = label.size as f32 * 0.9;
        let color = if label.bold { TEXT } else { style_color(label.style) };
        widgets::text(p, origin + vec2(label.x as f32, label.y as f32), Align2::LEFT_TOP, &mono(label.text, size, color));
    }
}

/// Everything on a page that never changes, in the retro look: the avatar's retro screen has it as the page's picture.
pub fn draw_chrome(p: &Painter, origin: Pos2, layout: &Layout) {
    frame(p, origin, Some(layout));
    furniture(p, origin, layout);
    for e in &layout.elements {
        element(p, e, origin, ink(e.look));
    }
    if layout.page == ScreenPage::Music {
        for pic in &layout.pictures {
            no_picture(p, rect(pic.rect, origin));
        }
    }
}

/// A picture's place while it has none: hatched, "NO SIGNAL".
fn no_picture(p: &Painter, r: Rect) {
    p.rect_filled(r, 0.0, PANEL);
    hatch(p, r, 10.0, Stroke::new(1.0, GRID));
    brackets(p, r, 10.0, Stroke::new(1.5, DIM));
    widgets::text(p, r.center(), Align2::CENTER_CENTER, &mono("NO SIGNAL", (r.height() * 0.14).clamp(10.0, 22.0), DIM));
}

/// A slot's text, as the overlay writes it: in the monospace font the avatar's screen writes it in.
fn slot(p: &Painter, s: &layout::Slot, text: &str, origin: Pos2) {
    if text.is_empty() {
        return;
    }
    let r = rect(s.rect(), origin);
    let txt = Txt::new(text, s.size as f32 * 0.9).mono().color(style_color(s.style));
    match s.align {
        Align::Left => widgets::text_truncated(p, pos2(r.min.x, r.center().y), Align2::LEFT_CENTER, &txt, r.width()),
        Align::Center => widgets::text_truncated(p, r.center(), Align2::CENTER_CENTER, &txt, r.width()),
    };
}

/// A picture, already turned into green dots ([`dots`]), in its brackets.
fn picture(p: &Painter, r: Rect, texture: Option<TextureId>) {
    match texture {
        Some(id) => {
            p.add(egui::epaint::RectShape::filled(r, 0.0, Color32::WHITE).with_texture(id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
            brackets(p, r.expand(3.0), 10.0, Stroke::new(1.5, LINE));
        }
        None => no_picture(p, r),
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
pub fn draw(p: &Painter, origin: Pos2, view: &super::menu_ui::View) {
    let menu = view.menu;
    let page = ScreenPage::of(menu);
    let layout = layout::cached(page);
    frame(p, origin, Some(layout));
    furniture(p, origin, layout);
    for (i, e) in layout.elements.iter().enumerate() {
        let state = layout::state(menu, e);
        if state.hidden {
            continue;
        }
        let r = rect(e.rect, origin);
        let down = view.pressed == Some(i);
        element(p, e, origin, if down { pressed(e.look) } else { ink(e.look) });
        if state.unavailable {
            p.rect_filled(r, 0.0, BG.gamma_multiply(0.6));
            hatch(p, r, 12.0, Stroke::new(1.0, GRID));
        } else if !down && view.hover == Some(i) {
            panel(p, r.expand(3.0), if r.height() < 64.0 { 8.0 } else { 15.0 }, LINE.gamma_multiply(0.10), Stroke::new(2.5, HOT));
        }
        if state.on {
            brackets(p, r.expand(6.0), 14.0, Stroke::new(3.0, HOT));
        }
    }
    for s in &layout.slots {
        slot(p, s, &layout::slot_text(menu, s), origin);
    }
    for pic in &layout.pictures {
        let r = rect(pic.rect, origin);
        match (layout::picture_key(menu, pic), menu.page) {
            (Some(key), _) => picture(p, r, (view.picture)(key)),
            (None, orbiters_orb::menu::Page::Music) => no_picture(p, r),
            (None, _) => {}
        }
    }
    scanlines(p, origin);
}

/// The back, in the retro look: "N4N0 menu, look at the other side".
pub fn draw_back(p: &Painter, origin: Pos2) {
    frame(p, origin, None);
    let card = Rect::from_min_size(origin, vec2(layout::WIDTH as f32, layout::HEIGHT as f32));
    let c = card.center();
    // N4N0 as a schematic: a ring with its eyes, in brackets.
    let orb = c - vec2(0.0, 92.0);
    p.circle_stroke(orb, 30.0, Stroke::new(2.0, LINE));
    p.circle_filled(orb, 22.0, PANEL);
    for side in [-1.0, 1.0] {
        p.rect_filled(Rect::from_center_size(orb + vec2(side * 9.0, -2.0), vec2(6.0, 12.0)), 0.0, LINE);
    }
    brackets(p, Rect::from_center_size(orb, vec2(84.0, 84.0)), 12.0, Stroke::new(1.5, DIM));
    widgets::text(p, pos2(c.x, c.y + 2.0), Align2::CENTER_CENTER, &mono("// N4N0 MENU //", 44.0, TEXT));
    widgets::text(p, pos2(c.x, c.y + 54.0), Align2::CENTER_CENTER, &mono("> look at the other side_", 22.0, TEXT2));
    hatch(p, Rect::from_center_size(pos2(c.x, c.y + 100.0), vec2(360.0, 10.0)), 8.0, Stroke::new(1.5, DIM));
}

/// A thumbnail as the retro look shows it (contracts `Docs/orb.md`, Retro look): its light, levels stretched, as 96 × 54
/// green dots in four steps with an ordered 4 × 4 dither, each dot square with a dark gap, `scale` pixels apart.
pub fn dots(image: &ColorImage, scale: usize) -> ColorImage {
    let (w, h) = (orbiters_orb::menu::retro::DOTS_X, orbiters_orb::menu::retro::DOTS_Y);
    let light = orbiters_orb::menu::retro::lights(image.size[0], image.size[1], |x, y| {
        let c = image.pixels[y * image.size[0] + x];
        [c.r(), c.g(), c.b()]
    });
    let steps = [BG, DIM, LINE, HOT];
    let mut out = ColorImage::filled([w * scale, h * scale], BG);
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
