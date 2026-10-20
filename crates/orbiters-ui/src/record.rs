//! The record button, flat: a dark disc with a thin ring (red while recording), the record dot morphing into a stop
//! square, two quiet orbits (the Orbiters ring motif) that speed up while recording, thin ripples while frames
//! arrive, and an arc showing how many frames carry a readable pose strip.

use crate::anim::{self, ease, Spring};
use crate::paint;
use crate::theme;
use egui::{Color32, CursorIcon, Rect, Response, Sense, Ui, Vec2};
use std::f32::consts::{FRAC_PI_2, TAU};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecState {
    Idle,
    Starting,
    Recording,
    Paused,
    Stopping,
    Importing,
}

/// Size of the whole widget (orbits included).
pub const SIZE: f32 = 252.0;

#[derive(Clone, Copy, Default)]
struct Spin {
    orbit_a: f32,
    orbit_b: f32,
    rim: f32,
    spinner: f32,
    last: f64,
    rec_since: Option<f64>,
}

/// Draws the button and returns its response; `toggled` is true on the press that starts or stops.
pub struct RecordButton {
    pub state: RecState,
    pub disabled: bool,
    /// Share of frames whose strip decoded (0..1).
    pub health: f32,
}

impl RecordButton {
    pub fn show(&self, ui: &mut Ui) -> (Response, bool) {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(SIZE), Sense::hover());
        self.show_at(ui, rect)
    }

    pub fn show_at(&self, ui: &mut Ui, rect: Rect) -> (Response, bool) {
        let id = ui.scope_id().with("orbiters-record");
        let ctx = ui.ctx().clone();
        let c = rect.center();
        let s = rect.width() / SIZE;
        let recording = matches!(self.state, RecState::Recording | RecState::Paused);
        let paused = self.state == RecState::Paused;
        let busy = matches!(self.state, RecState::Starting | RecState::Stopping | RecState::Importing);
        let live = anim::is_live(&ctx);

        let button = Rect::from_center_size(c, Vec2::splat(140.0 * s));
        let resp = ui.interact(button, id, if self.disabled || busy { Sense::hover() } else { Sense::click() });
        let toggled = !self.disabled && !busy && crate::widgets::pressed(&resp);

        // Rotations integrate their speed, so speeding up (recording) never makes them jump.
        let clock = anim::clock(&ctx, 30.0);
        let mut spin = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(id.with("spin"), || Spin { last: clock, ..Default::default() }));
        let dt = (clock - spin.last).clamp(0.0, 0.1) as f32;
        spin.last = clock;
        let period_a = if recording {
            if paused {
                60.0
            } else {
                9.0
            }
        } else {
            38.0
        };
        let period_b = if recording {
            if paused {
                80.0
            } else {
                14.0
            }
        } else {
            52.0
        };
        let [speed_a, speed_b, speed_rim] =
            anim::spring(&ctx, id.with("speeds"), [TAU / period_a, -TAU / period_b, TAU / if recording { 3.0 } else { 8.0 }], Spring::SOFT);
        spin.orbit_a = (spin.orbit_a + speed_a * dt) % TAU;
        spin.orbit_b = (spin.orbit_b + speed_b * dt) % TAU;
        spin.rim = (spin.rim + speed_rim * dt) % TAU;
        spin.spinner = (spin.spinner + TAU / 0.9 * dt) % TAU;
        match (recording && !paused, spin.rec_since) {
            (true, None) => spin.rec_since = Some(clock),
            (false, Some(_)) => spin.rec_since = None,
            _ => {}
        }
        ctx.data_mut(|d| d.insert_temp(id.with("spin"), spin));

        let p = ui.painter();
        let rec_k = anim::toggle(&ctx, id.with("rec"), recording, Spring::SOFT).clamp(0.0, 1.0);

        // Ripples.
        if let (Some(since), true) = (spin.rec_since, live) {
            for i in 0..3 {
                let t = clock - since - i as f64 * 0.8;
                if t < 0.0 {
                    continue;
                }
                let k = ((t % 2.4) / 2.4) as f32;
                let e = ease::out_cubic(k);
                p.circle_stroke(c, 70.0 * s * (1.0 + 0.6 * e), egui::Stroke::new(1.0, theme::alpha(theme::REC, 0.35 * (1.0 - e))));
            }
        }

        // Outer orbit: a faint ring and its dot; inner orbit: a dotted ring and a smaller dot, the other way.
        let r_a = 118.0 * s;
        paint::ring(p, c, r_a, 1.0, theme::white(0.06));
        let dot_a = c + Vec2::angled(spin.orbit_a - FRAC_PI_2) * r_a;
        p.circle_filled(dot_a, 4.0 * s, theme::mix(theme::FG_3, theme::REC, rec_k));
        let r_b = 100.0 * s;
        paint::dashed_circle(p, c, r_b, [2.0 * s, 7.0 * s], 1.0, theme::white(0.05), spin.orbit_b);
        let dot_b = c + Vec2::angled(spin.orbit_b) * r_b;
        p.circle_filled(dot_b, 3.0 * s, theme::mix(theme::FG_4, theme::alpha(theme::REC, 0.6), rec_k));

        // Decode health arc.
        let [sweep, arc_alpha] =
            // Only when decoding has a problem: the strip tile already shows a healthy share.
            anim::spring(&ctx, id.with("health"), [if recording { self.health.max(0.02) } else { 0.0 }, if recording && self.health <= 0.97 { 0.9 } else { 0.0 }], Spring::SOFT);
        if sweep > 0.002 && arc_alpha > 0.01 {
            let hc = if self.health > 0.97 {
                theme::OK
            } else if self.health > 0.8 {
                theme::WARN
            } else {
                theme::DANGER
            };
            paint::arc(p, c, 84.0 * s, -FRAC_PI_2, TAU * sweep.clamp(0.0, 1.0), 2.2 * s, theme::alpha(hc, arc_alpha.clamp(0.0, 1.0)));
        }

        // The button itself: hover grows it a little, a press squeezes it.
        let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered() && !self.disabled && !busy, Spring::BOUNCY);
        let press = anim::toggle(&ctx, id.with("press"), resp.is_pointer_button_down_on() && !self.disabled && !busy, Spring::BOUNCY);
        let scale = s * (1.0 + 0.035 * hover - 0.1 * press);
        let disabled_k = anim::toggle(&ctx, id.with("disabled"), self.disabled, Spring::SOFT);

        // The face: flat, a step lighter on hover, ringed in red while recording.
        let face = theme::mix(theme::mix(theme::INK_800, theme::INK_750, hover), theme::mix(theme::INK_800, theme::REC, 0.12), rec_k);
        let mut face_painter = p.clone();
        face_painter.set_opacity(1.0 - 0.5 * disabled_k);
        face_painter.circle_filled(c, 70.0 * scale, face);
        paint::ring(&face_painter, c, 69.0 * scale, 2.0 * scale, theme::mix(theme::INK_600, theme::REC, rec_k));

        // Glyph: dot ↔ square, or a spinner while busy.
        let busy_k = anim::toggle(&ctx, id.with("busy"), busy, Spring::SNAPPY).clamp(0.0, 1.0);
        if busy_k > 0.01 {
            let r = 24.0 * scale * (0.6 + 0.4 * busy_k);
            paint::ring(p, c, r - 1.5, 3.0, theme::white(0.15 * busy_k));
            paint::arc(p, c, r - 1.5, spin.spinner - FRAC_PI_2, FRAC_PI_2, 3.0, theme::white(busy_k));
        }
        if busy_k < 0.99 {
            let [size, radius] = anim::spring(&ctx, id.with("glyph"), if recording { [42.0, 12.0] } else { [50.0, 25.0] }, Spring::MORPH);
            let target = if recording {
                if paused {
                    theme::WARN
                } else {
                    Color32::WHITE
                }
            } else {
                theme::REC
            };
            let color = anim::color(&ctx, id.with("glyph-color"), target, Spring::MORPH);
            let k = 1.0 - busy_k;
            let g = Rect::from_center_size(c, Vec2::splat(size * scale * (0.4 + 0.6 * k)));
            p.rect_filled(g, theme::radius(radius * scale * (0.4 + 0.6 * k)), theme::alpha(color, k));
        }

        let resp = if self.disabled || busy { resp.on_hover_cursor(CursorIcon::NotAllowed) } else { resp.on_hover_cursor(CursorIcon::PointingHand) };
        (resp, toggled)
    }
}
