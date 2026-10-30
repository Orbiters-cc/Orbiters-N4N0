//! The editor's own controls, in the look of `orbiters_ui::widgets`: a labelled slider the width of its panel (its
//! value on the right, its fill from zero for values either side of it, double-click for its default), a quiet
//! section label, colour swatches and a check row.

use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets;

/// A slider of `value` in `range` with its `label`, `format`ted on its right; back to `default` on a double-click.
/// Returns whether it changed.
#[allow(clippy::too_many_arguments)]
pub fn slider(ui: &mut Ui, id_salt: impl egui::AsIdSalt, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, default: f32, format: impl Fn(f32) -> String) -> bool {
    let id = ui.scope_id().with(("editor-slider", id_salt));
    let (min, max) = (*range.start(), *range.end());
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 46.0), Sense::hover());
    let track = Rect::from_min_max(pos2(rect.left(), rect.top() + 22.0), pos2(rect.right(), rect.bottom()));
    let resp = ui.interact(track, id, Sense::click_and_drag());
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
    widgets::text(p, pos2(rect.left(), rect.top() + 9.0), Align2::LEFT_CENTER, &Txt::new(label, 12.5).color(theme::mix(theme::FG_2, theme::FG, hover)));
    widgets::text(p, pos2(rect.right(), rect.top() + 9.0), Align2::RIGHT_CENTER, &Txt::new(format(*value), 12.0).color(theme::FG_3));
    let bar = Rect::from_min_max(pos2(track.left() + 8.0, track.center().y - 2.5), pos2(track.right() - 8.0, track.center().y + 2.5));
    p.rect_filled(bar, theme::radius(2.5), theme::white(0.09));
    let at = |v: f32| bar.left() + bar.width() * if max > min { ((v - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    let knob = anim::value(ctx, id.with("knob"), at(*value), Spring::SNAPPY);
    // From zero when the range has both sides of it (exposure, contrast), else from its start.
    let from = if min < 0.0 && max > 0.0 { at(0.0) } else { bar.left() };
    let (a, b) = if knob < from { (knob, from) } else { (from, knob) };
    if b - a > 0.5 {
        p.rect_filled(Rect::from_min_max(pos2(a, bar.top()), pos2(b, bar.bottom())), theme::radius(2.5), theme::ACCENT);
    }
    if min < 0.0 && max > 0.0 {
        p.circle_filled(pos2(from, bar.center().y), 2.0, theme::white(0.3));
    }
    p.circle_filled(pos2(knob, bar.center().y), 7.5 + 1.0 * hover, Color32::WHITE);
    resp.on_hover_cursor(CursorIcon::PointingHand);
    changed
}

/// A quiet caps label over a group of controls.
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
    let mut tu = ui.new_child(egui::UiBuilder::new().max_rect(t).id_salt(("editor-switch", label)));
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
            p.circle_stroke(cell.center(), size * 0.5 + 2.0, egui::Stroke::new(2.0, theme::ACCENT));
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
