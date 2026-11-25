//! The editor's own controls, in the look of `orbiters_ui::widgets`: a labelled slider the width of its panel (its
//! value on the right, its fill from zero for values either side of it, double-click for its default; a colour or tone
//! slider's track drawn as what it does, with a small icon in its colour, [`Look`]), a quiet section label, colour
//! swatches and a check row.

use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Pos2, Rect, Sense, Stroke, Ui};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, widgets};

/// Phosphor icons the editor's sliders use beyond [`orbiters_ui::icons`].
mod glyphs {
    pub const PLUS_MINUS: &str = "\u{E3D8}";
    pub const SUN_DIM: &str = "\u{E474}";
    pub const MOON: &str = "\u{E330}";
    pub const THERMOMETER: &str = "\u{E5CC}";
    pub const DROP_HALF: &str = "\u{EB40}";
    pub const RAINBOW: &str = "\u{E598}";
}

/// What a colour or tone slider's track shows (what its value does to a picture, left to right) and its icon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// The accent fill from zero, no icon.
    Plain,
    /// Dark to light.
    Exposure,
    Brightness,
    /// Grey parting into black and white.
    Contrast,
    /// The light tones, dimmer to brighter.
    Highlights,
    /// The dark tones, deeper to lifted.
    Shadows,
    /// A faded black to a deep one.
    BlackPoint,
    /// Blue to amber.
    Temperature,
    /// Green to magenta.
    Tint,
    /// Grey to vivid.
    Saturation,
    Vibrance,
    /// The hues red turns to.
    Hue,
    /// Lighter edges to darker ones.
    Vignette,
    /// Red and blue parting.
    Aberration,
    /// More ink.
    Ink,
}

/// A gradient track: its colours left to right, and below them another row (two tones parting, two hues).
struct Track {
    top: Vec<Color32>,
    bottom: Option<Vec<Color32>>,
}

fn grey(v: u8) -> Color32 {
    Color32::from_gray(v)
}

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// A hue (degrees) at full saturation, a little softened.
fn hue(degrees: f32) -> Color32 {
    let h = degrees.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let c = |v: f32| (40.0 + v * 215.0) as u8;
    Color32::from_rgb(c(r), c(g), c(b))
}

/// Between the colours of `stops` (evenly spread) at `t` of 0 to 1.
fn along(stops: &[Color32], t: f32) -> Color32 {
    if stops.len() < 2 {
        return stops.first().copied().unwrap_or(Color32::WHITE);
    }
    let x = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let i = (x as usize).min(stops.len() - 2);
    theme::mix(stops[i], stops[i + 1], x - i as f32)
}

impl Look {
    fn track(self) -> Option<Track> {
        let one = |top: Vec<Color32>| Some(Track { top, bottom: None });
        let two = |top: Vec<Color32>, bottom: Vec<Color32>| Some(Track { top, bottom: Some(bottom) });
        match self {
            Look::Plain => None,
            Look::Exposure => one(vec![grey(18), grey(120), rgb(0xfff4dc)]),
            Look::Brightness => one(vec![grey(34), grey(128), grey(240)]),
            Look::Contrast => two(vec![grey(128), grey(4)], vec![grey(128), grey(250)]),
            Look::Highlights => one(vec![grey(120), grey(190), Color32::WHITE]),
            Look::Shadows => one(vec![grey(0), grey(48), grey(140)]),
            Look::BlackPoint => one(vec![grey(96), grey(40), grey(0)]),
            Look::Temperature => one(vec![rgb(0x3f7fff), rgb(0x9dbbf0), rgb(0xe6e6e6), rgb(0xffcb7a), rgb(0xff9a2e)]),
            Look::Tint => one(vec![rgb(0x2fd27a), rgb(0x9fe0b8), rgb(0xe6e6e6), rgb(0xf3a3e3), rgb(0xff3fd2)]),
            Look::Saturation => two(vec![grey(130), rgb(0xff3b5c)], vec![grey(130), rgb(0x1fb8ff)]),
            Look::Vibrance => two(vec![grey(130), rgb(0xffa51f)], vec![grey(130), rgb(0x9a5cff)]),
            Look::Hue => one((0..=12).map(|k| hue(-180.0 + 30.0 * k as f32)).collect()),
            Look::Vignette => one(vec![grey(235), grey(130), grey(0)]),
            Look::Aberration => two(vec![grey(140), rgb(0xff3040)], vec![grey(140), rgb(0x2f8fff)]),
            Look::Ink => one(vec![grey(200), grey(90), grey(0)]),
        }
    }

    /// Its icon, in the colour of the value at `t` (0 to 1 along the track) for the colour ones.
    fn icon(self, t: f32) -> Option<(&'static str, Color32)> {
        Some(match self {
            Look::Plain => return None,
            Look::Exposure => (glyphs::PLUS_MINUS, rgb(0xffe6a8)),
            Look::Brightness => (icons::SUN, rgb(0xffd36b)),
            Look::Contrast => (icons::CIRCLE_HALF, theme::fg()),
            Look::Highlights => (glyphs::SUN_DIM, rgb(0xfff1c2)),
            Look::Shadows => (glyphs::MOON, rgb(0x9aa8ff)),
            Look::BlackPoint => (icons::CIRCLE, theme::fg_2()),
            Look::Temperature => (glyphs::THERMOMETER, along(&[rgb(0x5f95ff), rgb(0xd6dbe4), rgb(0xffa940)], t)),
            Look::Tint => (icons::DROP, along(&[rgb(0x3ddc84), rgb(0xd6dbe4), rgb(0xff5fd8)], t)),
            Look::Saturation => (glyphs::DROP_HALF, theme::mix(grey(150), rgb(0xff4d6d), t)),
            Look::Vibrance => (icons::SPARKLE, theme::mix(grey(150), rgb(0xffb340), t)),
            Look::Hue => (glyphs::RAINBOW, hue(-180.0 + 360.0 * t)),
            Look::Vignette => (icons::APERTURE, theme::fg_2()),
            Look::Aberration => (icons::CIRCLES_THREE, rgb(0xff6b7a)),
            Look::Ink => (icons::PENCIL_SIMPLE, theme::fg_2()),
        })
    }
}

/// The track `bar` filled with `track`'s colours, its ends round.
fn paint_track(p: &egui::Painter, bar: Rect, track: &Track) {
    let r = bar.height() * 0.5;
    let inner = bar.shrink2(vec2(r, 0.0));
    let rows: Vec<(&Vec<Color32>, f32, f32)> = match &track.bottom {
        Some(b) => vec![(&track.top, bar.top(), bar.center().y), (b, bar.center().y, bar.bottom())],
        None => vec![(&track.top, bar.top(), bar.bottom())],
    };
    let mut mesh = egui::Mesh::default();
    for (stops, y0, y1) in &rows {
        let n = 48;
        let base = mesh.vertices.len() as u32;
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let x = inner.left() + inner.width() * t;
            let c = along(stops, t);
            mesh.colored_vertex(pos2(x, *y0), c);
            mesh.colored_vertex(pos2(x, *y1), c);
        }
        for i in 0..n {
            let k = base + i * 2;
            mesh.add_triangle(k, k + 1, k + 2);
            mesh.add_triangle(k + 1, k + 2, k + 3);
        }
    }
    // The round ends, in the end colours (two halves when there are two rows).
    for (end, t, x) in [(0, 0.0, inner.left()), (1, 1.0, inner.right())] {
        let c = pos2(x, bar.center().y);
        let dir = if end == 0 { -1.0 } else { 1.0 };
        for (stops, y0, y1) in &rows {
            let color = along(stops, t);
            let mut pts = vec![pos2(x, *y0), pos2(x, *y1)];
            let steps = 8;
            let (a0, a1) = (((*y1 - c.y) / r).clamp(-1.0, 1.0).asin(), ((*y0 - c.y) / r).clamp(-1.0, 1.0).asin());
            for s in 0..=steps {
                let a = a0 + (a1 - a0) * s as f32 / steps as f32;
                pts.push(c + vec2(dir * a.cos() * r, a.sin() * r));
            }
            let base = mesh.vertices.len() as u32;
            for q in &pts {
                mesh.colored_vertex(*q, color);
            }
            for i in 1..pts.len() as u32 - 1 {
                mesh.add_triangle(base, base + i, base + i + 1);
            }
        }
    }
    p.add(egui::Shape::mesh(mesh));
    p.rect_stroke(bar, theme::radius(r), Stroke::new(1.0, theme::white(0.1)), egui::StrokeKind::Outside);
}

/// A slider of `value` in `range` with its `label`, `format`ted on its right; back to `default` on a double-click.
/// Returns whether it changed.
#[allow(clippy::too_many_arguments)]
pub fn slider(ui: &mut Ui, id_salt: impl egui::AsIdSalt, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, default: f32, format: impl Fn(f32) -> String) -> bool {
    slider_look(ui, id_salt, label, value, range, default, format, Look::Plain)
}

/// [`slider`] looking as `look`: its track a gradient of what it does, its icon beside its label.
#[allow(clippy::too_many_arguments)]
pub fn slider_look(
    ui: &mut Ui,
    id_salt: impl egui::AsIdSalt,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    format: impl Fn(f32) -> String,
    look: Look,
) -> bool {
    let id = ui.scope_id().with(("editor-slider", id_salt));
    let (min, max) = (*range.start(), *range.end());
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 46.0), Sense::hover());
    let track = Rect::from_min_max(pos2(rect.left(), rect.top() + 22.0), pos2(rect.right(), rect.bottom()));
    let resp = ui.interact(track, id, Sense::click_and_drag());
    let held = resp.is_pointer_button_down_on() || resp.dragged();
    ui.data_mut(|d| d.insert_temp(id.with("held"), held));
    let mut changed = false;
    if resp.double_clicked() {
        changed = (*value - default).abs() > f32::EPSILON;
        *value = default;
    } else if let (true, Some(p)) = (resp.is_pointer_button_down_on() || resp.dragged(), resp.interact_pointer_pos()) {
        let t = ((p.x - track.left() - 8.0) / (track.width() - 16.0)).clamp(0.0, 1.0);
        let v = (min + t * (max - min)).clamp(min, max);
        if (v - *value).abs() > f32::EPSILON {
            *value = v;
            changed = true;
        }
    }
    let p = ui.painter();
    let ctx = ui.ctx();
    let hover = anim::toggle(ctx, id.with("hover"), resp.hovered() || resp.dragged(), Spring::SNAPPY);
    let share = |v: f32| if max > min { ((v - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    let mut text_x = rect.left();
    if let Some((glyph, color)) = look.icon(share(*value)) {
        let c = pos2(rect.left() + 8.0, rect.top() + 9.0);
        if look == Look::BlackPoint {
            p.circle_filled(c, 6.0, Color32::BLACK);
            p.circle_stroke(c, 6.0, Stroke::new(1.4, theme::fg_2()));
        } else {
            widgets::icon(p, c, glyph, 15.0, color);
        }
        text_x += 22.0;
    }
    widgets::text(p, pos2(text_x, rect.top() + 9.0), Align2::LEFT_CENTER, &Txt::new(label, 12.5).color(theme::mix(theme::fg_2(), theme::fg(), hover)));
    widgets::text(p, pos2(rect.right(), rect.top() + 9.0), Align2::RIGHT_CENTER, &Txt::new(format(*value), 12.0).color(theme::fg_3()));
    let gradient = look.track();
    let half = if gradient.is_some() { 3.0 } else { 2.5 };
    let bar = Rect::from_min_max(pos2(track.left() + 8.0, track.center().y - half), pos2(track.right() - 8.0, track.center().y + half));
    let at = |v: f32| bar.left() + bar.width() * share(v);
    let knob = anim::value(ctx, id.with("knob"), at(*value), Spring::SNAPPY);
    let from = if min < 0.0 && max > 0.0 { at(0.0) } else { bar.left() };
    match &gradient {
        Some(t) => paint_track(p, bar, t),
        None => {
            p.rect_filled(bar, theme::radius(2.5), theme::white(0.09));
            // From zero when the range has both sides of it (exposure, contrast), else from its start.
            let (a, b) = if knob < from { (knob, from) } else { (from, knob) };
            if b - a > 0.5 {
                p.rect_filled(Rect::from_min_max(pos2(a, bar.top()), pos2(b, bar.bottom())), theme::radius(2.5), theme::accent());
            }
        }
    }
    if min < 0.0 && max > 0.0 {
        let mark = if gradient.is_some() { theme::black(0.55) } else { theme::white(0.3) };
        p.line_segment([pos2(from, bar.top() - 3.0), pos2(from, bar.bottom() + 3.0)], Stroke::new(1.5, mark));
    }
    let k = Pos2::new(knob, bar.center().y);
    p.circle_filled(k, 7.5 + 1.0 * hover, Color32::WHITE);
    if let Some(t) = &gradient {
        // The knob shows the colour it is on.
        let x = ((knob - bar.left()) / bar.width().max(1.0)).clamp(0.0, 1.0);
        let c = match &t.bottom {
            Some(b) => theme::mix(along(&t.top, x), along(b, x), 0.5),
            None => along(&t.top, x),
        };
        p.circle_filled(k, 4.5 + 0.6 * hover, c);
        p.circle_stroke(k, 4.5 + 0.6 * hover, Stroke::new(1.0, theme::black(0.18)));
    }
    resp.on_hover_cursor(CursorIcon::PointingHand);
    changed
}

/// Whether the [`slider`] of `id_salt` just drawn in `ui` is held (pressed or dragged).
pub fn slider_held(ui: &Ui, id_salt: impl egui::AsIdSalt) -> bool {
    let id = ui.scope_id().with(("editor-slider", id_salt));
    ui.data(|d| d.get_temp(id.with("held")).unwrap_or(false))
}

// A quiet caps label over a group of controls.
pub fn label(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    widgets::text(ui.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, &Txt::caps(text));
}

/// A row with a label and a switch on its right.
pub fn switch(ui: &mut Ui, id_salt: impl egui::AsIdSalt, label: &str, on: &mut bool) -> bool {
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
    widgets::text(ui.painter(), pos2(r.left(), r.center().y), Align2::LEFT_CENTER, &Txt::new(label, 13.0).weight(weight::MEDIUM));
    let t = Rect::from_min_size(pos2(r.right() - 48.0, r.center().y - 14.0), vec2(48.0, 28.0));
    let mut tu = ui.new_child(egui::UiBuilder::new().max_rect(t).id_salt(("editor-switch", orbiters_i18n::english_of(label))));
    widgets::toggle(&mut tu, id_salt, on)
}

/// Round colour swatches in rows; returns the one pressed. `chosen` is ringed.
pub fn swatches(ui: &mut Ui, id: Id, colors: &[[u8; 3]], chosen: [u8; 3]) -> Option<[u8; 3]> {
    let w = ui.available_width();
    let size = 26.0;
    let per_row = ((w + 8.0) / (size + 8.0)).floor().max(1.0) as usize;
    let rows = colors.len().div_ceil(per_row);
    let (r, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (size + 8.0)), Sense::hover());
    let mut picked = None;
    for (i, c) in colors.iter().enumerate() {
        let (row, col) = (i / per_row, i % per_row);
        let cell = Rect::from_min_size(pos2(r.left() + col as f32 * (size + 8.0), r.top() + row as f32 * (size + 8.0)), vec2(size, size));
        let resp = ui.interact(cell, id.with(i), Sense::click());
        let hover = anim::toggle(ui.ctx(), id.with((i, "hover")), resp.hovered(), Spring::SNAPPY);
        let p = ui.painter();
        let color = Color32::from_rgb(c[0], c[1], c[2]);
        p.circle_filled(cell.center(), size * 0.5 * (0.86 + 0.06 * hover), color);
        p.circle_stroke(cell.center(), size * 0.5 * (0.86 + 0.06 * hover), egui::Stroke::new(1.0, theme::white(0.18)));
        if *c == chosen {
            p.circle_stroke(cell.center(), size * 0.5 + 2.0, egui::Stroke::new(2.0, theme::accent()));
        }
        if widgets::pressed(&resp) {
            picked = Some(*c);
        }
        resp.on_hover_cursor(CursorIcon::PointingHand);
    }
    picked
}

/// Percent of 0 to 1.
pub fn percent(v: f32) -> String {
    format!("{:.0} %", v * 100.0)
}

/// A signed value of -1 to 1 as -100 to +100.
pub fn signed(v: f32) -> String {
    let n = (v * 100.0).round() as i32;
    if n > 0 {
        format!("+{n}")
    } else {
        format!("{n}")
    }
}
