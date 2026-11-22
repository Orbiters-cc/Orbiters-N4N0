//! A slider over a few named steps (the FPS booster's, REQUEST15 4): a track, the steps as stops along it with their
//! names under them, a thumb on the chosen one, the track filled up to it (each step includes the ones before it).
//! It is dragged (snapping to the nearest step, chosen when let go), pressed anywhere to jump there, and moved with the
//! arrow keys, Home and End once focused. [`paint`] draws the same slider for a surface that handles the pointer
//! itself (the overlay's booster sheet), with its own colours and sizes ([`Look`]).

use crate::anim::{self, Spring};
use crate::theme::{self, weight};
use crate::widgets::{note_control, note_press};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, FontId, Key, Painter, Rect, Sense, Stroke, Ui};

/// How the slider looks on a surface.
#[derive(Clone, Debug)]
pub struct Look {
    /// The track's rest, and the part up to the thumb.
    pub track: Color32,
    pub fill: Color32,
    pub thumb: Color32,
    /// The thumb's ring and a lit name: the accent.
    pub accent: Color32,
    pub label: Color32,
    pub label_on: Color32,
    pub font: FontId,
    pub font_on: FontId,
    pub track_h: f32,
    pub thumb_r: f32,
    pub stop_r: f32,
    /// From the track to the names' centre line.
    pub label_dy: f32,
    /// A step's number inside the thumb (what a monochrome look still tells apart).
    pub numbered: bool,
}

impl Look {
    /// The window's (Settings).
    pub fn window() -> Self {
        Self {
            track: theme::white(0.09),
            fill: theme::alpha(theme::ACCENT, 0.85),
            thumb: Color32::WHITE,
            accent: theme::ACCENT,
            label: theme::FG_3,
            label_on: theme::FG,
            font: theme::font(12.5, weight::MEDIUM),
            font_on: theme::font(12.5, weight::SEMIBOLD),
            track_h: 6.0,
            thumb_r: 10.0,
            stop_r: 3.5,
            label_dy: 28.0,
            numbered: false,
        }
    }
}

/// Stops at the centres of `n` equal columns of `rect`: each name is centred under its stop and fits its column.
pub fn stops(rect: Rect, n: usize) -> Vec<f32> {
    let n = n.max(1);
    let w = rect.width() / n as f32;
    (0..n).map(|i| rect.left() + (i as f32 + 0.5) * w).collect()
}

/// The stop nearest `x` among stops at `xs` (past the ends: the end).
pub fn nearest(xs: &[f32], x: f32) -> usize {
    xs.iter().enumerate().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map_or(0, |(i, _)| i)
}

/// The step a key moves to from `at` among `n`, if it is one that moves it.
pub fn stepped(at: usize, n: usize, key: Key) -> Option<usize> {
    let last = n.saturating_sub(1);
    match key {
        Key::ArrowLeft | Key::ArrowDown => Some(at.saturating_sub(1)),
        Key::ArrowRight | Key::ArrowUp => Some((at + 1).min(last)),
        Key::Home => Some(0),
        Key::End => Some(last),
        _ => None,
    }
}

/// Draws the slider: the track at `y` from the first stop to the last, filled up to `at` (the thumb's place, in stops,
/// sprung between them), the stops, the names under them (`chosen` lit), a halo on `hover`, a ring while `focus`.
#[allow(clippy::too_many_arguments)]
pub fn paint(p: &Painter, xs: &[f32], y: f32, at: f32, chosen: usize, labels: &[&str], hover: Option<usize>, focus: f32, look: &Look) {
    let (Some(&first), Some(&last)) = (xs.first(), xs.last()) else { return };
    let x_at = |t: f32| {
        let t = t.clamp(0.0, (xs.len() - 1) as f32);
        let i = (t.floor() as usize).min(xs.len() - 1);
        let j = (i + 1).min(xs.len() - 1);
        xs[i] + (xs[j] - xs[i]) * (t - i as f32)
    };
    let thumb_x = x_at(at);
    let h = look.track_h;
    p.rect_filled(Rect::from_min_max(pos2(first - h * 0.5, y - h * 0.5), pos2(last + h * 0.5, y + h * 0.5)), theme::radius(h * 0.5), look.track);
    if thumb_x > first {
        p.rect_filled(Rect::from_min_max(pos2(first - h * 0.5, y - h * 0.5), pos2(thumb_x, y + h * 0.5)), theme::radius(h * 0.5), look.fill);
    }
    for (i, x) in xs.iter().enumerate() {
        let passed = *x <= thumb_x + 0.5;
        p.circle_filled(pos2(*x, y), look.stop_r, if passed { look.thumb } else { theme::mix(look.track, look.label, 0.5) });
        if hover == Some(i) && i != chosen {
            p.circle_filled(pos2(*x, y), look.thumb_r + 6.0, theme::alpha(look.accent, 0.16));
        }
        if let Some(label) = labels.get(i) {
            let on = i == chosen;
            let font = if on { look.font_on.clone() } else { look.font.clone() };
            let w = if xs.len() > 1 { (xs[1] - xs[0]).abs() - 6.0 } else { 200.0 };
            let galley = p.layout(label.to_string(), font, if on { look.label_on } else { look.label }, w.max(20.0));
            // The ends' names stay over their stop as long as they fit the column; the rows are centred.
            let at = pos2(*x - galley.size().x * 0.5, y + look.label_dy - galley.rows.first().map_or(0.0, |r| r.height()) * 0.5);
            p.galley(at, galley, look.label);
        }
    }
    let c = pos2(thumb_x, y);
    if focus > 0.01 {
        p.circle_stroke(c, look.thumb_r + 5.0, Stroke::new(2.0, theme::alpha(look.accent, 0.8 * focus)));
    }
    p.circle_filled(c + vec2(0.0, 1.5), look.thumb_r + 1.0, theme::black(0.25));
    p.circle_filled(c, look.thumb_r, look.thumb);
    p.circle_stroke(c, look.thumb_r - 1.0, Stroke::new(2.0, look.accent));
    if look.numbered {
        p.text(c, Align2::CENTER_CENTER, chosen.to_string(), FontId::new(look.thumb_r * 0.9, look.font_on.family.clone()), look.accent);
    }
}

/// The slider in a window, `width` wide, over `steps` (a value and its name each). True when the value changed: on a
/// press, on letting go of a drag, or with a key; a drag shows where it would land meanwhile.
pub fn slider<T: Copy + PartialEq>(ui: &mut Ui, id_salt: impl egui::AsIdSalt, value: &mut T, steps: &[(T, &str)], width: f32) -> bool {
    let look = Look::window();
    let key = crate::widgets::salt_key(&id_salt);
    let id = ui.scope_id().with(id_salt);
    let height = look.thumb_r * 2.0 + 6.0 + look.label_dy + 4.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    crate::widgets::mark_control(&key, "step slider", rect);
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let n = steps.len();
    let xs = stops(rect, n);
    let y = rect.top() + look.thumb_r + 4.0;
    let chosen = steps.iter().position(|(v, _)| v == value).unwrap_or(0);
    let held_id = id.with("held");
    let ctx = ui.ctx().clone();
    let mut changed = false;
    let set = |i: usize, value: &mut T| {
        if steps[i].0 != *value {
            *value = steps[i].0;
            note_press(steps[i].1);
            true
        } else {
            false
        }
    };
    // A press or a drag shows where it lands; it is chosen when let go.
    let pointer = resp.interact_pointer_pos().map(|p| nearest(&xs, p.x));
    if resp.is_pointer_button_down_on() || resp.dragged() {
        if let Some(i) = pointer {
            ctx.data_mut(|d| d.insert_temp(held_id, i));
        }
        resp.request_focus();
    }
    if resp.drag_stopped() || resp.clicked() {
        let held = ctx.data_mut(|d| d.remove_temp::<usize>(held_id)).or(pointer);
        if let Some(i) = held {
            changed |= set(i, value);
        }
    }
    // Keys, once focused: the arrows stay with the slider rather than moving the focus.
    if resp.has_focus() {
        ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() }));
        let pressed: Vec<Key> = ui.input(|i| [Key::ArrowLeft, Key::ArrowRight, Key::ArrowUp, Key::ArrowDown, Key::Home, Key::End].into_iter().filter(|k| i.key_pressed(*k)).collect());
        let mut at = steps.iter().position(|(v, _)| v == value).unwrap_or(0);
        for key in pressed {
            if let Some(next) = stepped(at, n, key) {
                at = next;
                changed |= set(next, value);
            }
        }
    }
    let shown = ctx.data(|d| d.get_temp::<usize>(held_id)).filter(|_| resp.is_pointer_button_down_on() || resp.dragged()).unwrap_or_else(|| steps.iter().position(|(v, _)| v == value).unwrap_or(chosen));
    let at = anim::value(&ctx, id.with("at"), shown as f32, Spring::SNAPPY);
    let hover = resp.hover_pos().filter(|_| !resp.dragged()).map(|p| nearest(&xs, p.x));
    let focus = anim::toggle(&ctx, id.with("focus"), resp.has_focus() && !resp.is_pointer_button_down_on(), Spring::SNAPPY);
    let labels: Vec<&str> = steps.iter().map(|(_, l)| *l).collect();
    for (i, (_, label)) in steps.iter().enumerate() {
        note_control(&ctx, label, Rect::from_center_size(pos2(xs[i], rect.center().y), vec2(rect.width() / n.max(1) as f32, rect.height())), true);
    }
    paint(ui.painter(), &xs, y, at, shown, &labels, hover, focus, &look);
    resp.on_hover_cursor(CursorIcon::PointingHand);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_snaps_to_the_nearest_step_and_past_the_ends_to_the_end() {
        let xs = stops(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 50.0)), 4);
        assert_eq!(xs, [50.0, 150.0, 250.0, 350.0]);
        assert_eq!(nearest(&xs, -30.0), 0);
        assert_eq!(nearest(&xs, 99.0), 0);
        assert_eq!(nearest(&xs, 101.0), 1);
        assert_eq!(nearest(&xs, 260.0), 2);
        assert_eq!(nearest(&xs, 900.0), 3);
    }

    #[test]
    fn keys_move_one_step_and_home_and_end_to_the_ends() {
        assert_eq!(stepped(0, 4, Key::ArrowLeft), Some(0), "never before the first");
        assert_eq!(stepped(1, 4, Key::ArrowRight), Some(2));
        assert_eq!(stepped(3, 4, Key::ArrowUp), Some(3), "never past the last");
        assert_eq!(stepped(2, 4, Key::ArrowDown), Some(1));
        assert_eq!(stepped(2, 4, Key::Home), Some(0));
        assert_eq!(stepped(0, 4, Key::End), Some(3));
        assert_eq!(stepped(1, 4, Key::A), None);
    }

    /// A drag lands where it is let go, a press where it is pressed, keys one step: driven through egui itself.
    #[test]
    fn the_slider_is_pressed_dragged_and_keyed() {
        let ctx = egui::Context::default();
        crate::install(&ctx);
        let steps = [(0u8, "Off"), (1, "Background"), (2, "Picture"), (3, "Avatars")];
        let mut value = 0u8;
        let mut changes = 0;
        let mut time = 0.0;
        let mut frame = |events: Vec<egui::Event>, value: &mut u8, changes: &mut i32| {
            time += 1.0 / 60.0;
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(500.0, 200.0))), time: Some(time), events, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| {
                ui.scope_builder(egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 80.0))), |ui| {
                    if slider(ui, "steps", value, &steps, 400.0) {
                        *changes += 1;
                    }
                });
            });
            out.textures_delta.clear();
        };
        let button = |at: egui::Pos2, pressed: bool| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let y = 14.0;
        frame(vec![], &mut value, &mut changes);
        // A press on the third stop: chosen on the release.
        frame(vec![egui::Event::PointerMoved(pos2(250.0, y)), button(pos2(250.0, y), true)], &mut value, &mut changes);
        frame(vec![button(pos2(250.0, y), false)], &mut value, &mut changes);
        assert_eq!((value, changes), (2, 1));
        // A drag from there to the left end: nothing while it moves, the first step when let go.
        frame(vec![egui::Event::PointerMoved(pos2(250.0, y)), button(pos2(250.0, y), true)], &mut value, &mut changes);
        for x in [220.0, 160.0, 90.0, 20.0] {
            frame(vec![egui::Event::PointerMoved(pos2(x, y))], &mut value, &mut changes);
        }
        assert_eq!(value, 2, "not yet: it lands when let go");
        frame(vec![button(pos2(20.0, y), false)], &mut value, &mut changes);
        assert_eq!(value, 0);
        // Focused by that press: the arrows move it one step, End to the last.
        frame(vec![egui::Event::Key { key: Key::ArrowRight, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }], &mut value, &mut changes);
        assert_eq!(value, 1);
        frame(vec![egui::Event::Key { key: Key::End, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }], &mut value, &mut changes);
        assert_eq!(value, 3);
    }
}
