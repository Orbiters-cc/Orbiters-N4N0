//! The way back of everything that grows out of a source (a card, a button) and shrinks back into it: closing is the
//! opening run backwards on the same progress spring (a press during either reverses it from where it is), then a
//! landing. In the last stretch the source takes over: it shows under the shrinking shape, which fades out over it
//! ([`shape_alpha`], [`source_alpha`]), and once the shape is back on it the source gives a short spring bounce
//! ([`Travel::landing`]), as if it caught it.
//!
//! [`travel`] gives the progress and the landing. A morph that lands says so for its source's rect ([`land`]); the
//! controls in [`crate::widgets`] and [`crate::popup`] bounce on their own when something lands on them
//! ([`bounce`]), pages call [`bounce`] for their own cards, or draw the source themselves where it flies ([`ride`]).

use crate::anim::{self, Spring};
use egui::emath::TSTransform;
use egui::{Context, Id, Rect};

/// Where a morph is between its source (0) and where it opens (1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Travel {
    /// 0 on the source, 1 open (a little past 1 while it settles open); never below 0.
    pub t: f32,
    /// Scale of the source while the morph lands on it (1 otherwise).
    pub landing: f32,
    /// Back on its source, which bounces.
    pub landed: bool,
    /// Closed and landed: drop it (draw this frame as usual, the source at rest).
    pub done: bool,
}

/// How long the source's landing bounce lasts, in seconds.
pub const LANDING: f64 = 0.38;

/// Progress below which the shape hands over to its source (fading out over it as it lands, in as it leaves).
pub const HANDOVER: f32 = 0.2;

/// The progress of the morph `id` towards open (`open`) or back to its source, on `spring`.
pub fn travel(ctx: &Context, id: Id, open: bool, spring: Spring) -> Travel {
    let raw = anim::spring_from(ctx, id.with("morph-travel"), [0.0], [if open { 1.0 } else { 0.0 }], spring)[0];
    let (key, out) = (id.with("morph-landed"), id.with("morph-out"));
    let travelling = Travel { t: raw.max(0.0), landing: 1.0, landed: false, done: false };
    if open {
        ctx.data_mut(|d| {
            d.remove_temp::<f64>(key);
            if raw > 0.02 {
                d.insert_temp(out, true);
            }
        });
        return travelling;
    }
    let now = anim::time(ctx);
    let landed_at = match ctx.data(|d| d.get_temp::<f64>(key)) {
        Some(at) => at,
        // Back on the source: the spring would overshoot past it; the source bounces instead (when it was ever away
        // from it: a list never opened is simply closed).
        None if raw <= 0.0 => {
            if !ctx.data_mut(|d| d.remove_temp::<bool>(out)).unwrap_or(false) {
                return Travel { t: 0.0, landing: 1.0, landed: false, done: true };
            }
            ctx.data_mut(|d| d.insert_temp(key, now));
            now
        }
        None => return travelling,
    };
    if !anim::is_live(ctx) {
        return Travel { t: 0.0, landing: 1.0, landed: true, done: true };
    }
    let since = anim::since(ctx, landed_at, LANDING);
    Travel { t: 0.0, landing: bounce_at(since as f32), landed: true, done: since >= LANDING }
}

/// The source's scale `seconds` after a morph landed on it: it gives a little under the arrival, springs a little
/// past its size and settles (a damped spring started by the arrival's speed).
pub fn bounce_at(seconds: f32) -> f32 {
    if !(0.0..LANDING as f32).contains(&seconds) {
        return 1.0;
    }
    const AMPLITUDE: f32 = 0.075;
    const DECAY: f32 = 9.5;
    const OMEGA: f32 = std::f32::consts::TAU / 0.29;
    1.0 - AMPLITUDE * (-DECAY * seconds).exp() * (OMEGA * seconds).sin()
}

/// 0 below `edge0`, 1 above `edge1`, smooth between.
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Hand-over of a shape much larger than its source that does not ride with it (a dialog landing on its button): it
/// fades only once it is nearly the button's size.
pub const HANDOVER_LATE: f32 = 0.1;

/// Opacity of the flying shape at progress `t`: it fades out over its source in the last stretch of its way back (and
/// in over it as it leaves).
pub fn shape_alpha(t: f32) -> f32 {
    shape_alpha_by(t, HANDOVER)
}

/// Opacity of the source under the flying shape at progress `t`: whole once the shape starts fading over it, gone
/// while the shape is open.
pub fn source_alpha(t: f32) -> f32 {
    source_alpha_by(t, HANDOVER)
}

/// [`shape_alpha`] for a hand-over below `handover`.
pub fn shape_alpha_by(t: f32, handover: f32) -> f32 {
    smoothstep(0.0, handover, t)
}

/// [`source_alpha`] for a hand-over below `handover`.
pub fn source_alpha_by(t: f32, handover: f32) -> f32 {
    1.0 - smoothstep(handover, 2.0 * handover, t)
}

/// The transform that draws a source laid out at `home` where the morph has it now: `home_part` (its picture, its
/// whole rect) moved and scaled as one onto `now` (that part in the flying shape, matched by width), times the landing
/// bounce about the source's centre.
pub fn ride(home: Rect, home_part: Rect, now: Rect, landing: f32) -> TSTransform {
    let s = if home_part.width() > 0.5 && now.width().is_finite() { now.width() / home_part.width() } else { 1.0 };
    let flight = TSTransform::new(now.min.to_vec2() - home_part.min.to_vec2() * s, s);
    let c = home.center().to_vec2();
    TSTransform::new(c * (1.0 - landing), landing) * flight
}

fn landings_id() -> Id {
    Id::unique("orbiters-morph-landings")
}

/// Says that a morph lands on `rect` this frame, its source scaled by `scale`.
pub fn land(ctx: &Context, rect: Rect, scale: f32) {
    if !rect.is_finite() {
        return;
    }
    // The pass is read first: the context cannot be read while its memory is locked for writing.
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let list = d.get_temp_mut_or_default::<Vec<(Rect, f32, u64)>>(landings_id());
        list.retain(|(r, _, p)| p + 1 >= pass && !same_place(*r, rect));
        list.push((rect, scale, pass));
    });
}

/// The scale of what sits at `rect`: 1 unless a morph lands on it (said this frame or the last; sources draw before
/// what flies over them).
pub fn bounce(ctx: &Context, rect: Rect) -> f32 {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<Vec<(Rect, f32, u64)>>(landings_id()))
        .and_then(|list| list.iter().rev().find(|(r, _, p)| p + 1 >= pass && same_place(*r, rect)).map(|(_, s, _)| *s))
        .unwrap_or(1.0)
}

fn same_place(a: Rect, b: Rect) -> bool {
    (a.min - b.min).length() < 1.5 && (a.max - b.max).length() < 1.5
}

/// `rect` scaled by `s` about its centre (a bouncing source).
pub fn scaled(rect: Rect, s: f32) -> Rect {
    if s == 1.0 {
        rect
    } else {
        Rect::from_center_size(rect.center(), rect.size() * s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn frame(ctx: &Context, t: f64, mut f: impl FnMut(&Context)) {
        let input = egui::RawInput { time: Some(t), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| f(ui.ctx()));
        out.textures_delta.clear();
    }

    #[test]
    fn a_morph_closes_on_its_way_out_lands_with_a_bounce_and_ends() {
        let ctx = Context::default();
        let id = Id::unique("morph-test");
        let mut seen = Vec::new();
        for k in 0..60 {
            frame(&ctx, k as f64 / 60.0, |ctx| seen.push(travel(ctx, id, true, Spring::EXPAND)));
        }
        assert!((seen.last().unwrap().t - 1.0).abs() < 0.01, "{:?}", seen.last());
        // Closing: back to 0, never below, then the landing bounce, then done.
        let mut closing = Vec::new();
        for k in 60..140 {
            frame(&ctx, k as f64 / 60.0, |ctx| closing.push(travel(ctx, id, false, Spring::EXPAND)));
        }
        assert!(closing.iter().all(|c| c.t >= 0.0));
        let landed = closing.iter().position(|c| c.landed).expect("lands");
        assert!(landed > 5 && landed < 30, "{landed}");
        let scales: Vec<f32> = closing[landed..].iter().map(|c| c.landing).collect();
        let low = scales.iter().cloned().fold(f32::MAX, f32::min);
        let high = scales.iter().cloned().fold(f32::MIN, f32::max);
        assert!(low < 0.98 && low > 0.94, "{low}");
        assert!(high > 1.003 && high < 1.03, "{high}");
        let done = closing.iter().position(|c| c.done).expect("ends");
        assert!(((done - landed) as f64) < LANDING * 60.0 + 2.0);
        assert_eq!(closing[done].landing, 1.0);
        // Opened again while landing: it leaves from where it is.
        let mut again = None;
        frame(&ctx, 140.0 / 60.0, |ctx| again = Some(travel(ctx, id, true, Spring::EXPAND)));
        assert!(!again.unwrap().landed && again.unwrap().t < 0.1);
    }

    #[test]
    fn a_morph_never_opened_is_closed_without_landing() {
        let ctx = Context::default();
        let mut seen = None;
        for k in 0..3 {
            frame(&ctx, k as f64 / 60.0, |ctx| seen = Some(travel(ctx, Id::unique("never"), false, Spring::BOUNCY)));
        }
        let seen = seen.unwrap();
        assert!(seen.done && !seen.landed && seen.landing == 1.0, "{seen:?}");
    }

    #[test]
    fn reversing_midway_continues_from_where_it_is() {
        let ctx = Context::default();
        let id = Id::unique("morph-reverse");
        let mut last = 0.0;
        for k in 0..6 {
            frame(&ctx, k as f64 / 60.0, |ctx| last = travel(ctx, id, true, Spring::EXPAND).t);
        }
        let mut next = 0.0;
        frame(&ctx, 6.0 / 60.0, |ctx| next = travel(ctx, id, false, Spring::EXPAND).t);
        // Still moving out this frame (its speed carries), no jump back to the source.
        assert!(next >= last - 0.01 && last > 0.2, "{last} {next}");
    }

    #[test]
    fn the_shape_hands_over_to_its_source() {
        assert_eq!(shape_alpha(0.0), 0.0);
        assert_eq!(shape_alpha(HANDOVER), 1.0);
        assert_eq!(source_alpha(HANDOVER), 1.0);
        assert_eq!(source_alpha(2.0 * HANDOVER), 0.0);
        // Wherever the shape is see-through, the source is whole under it.
        for k in 0..=20 {
            let t = k as f32 / 20.0;
            assert!(shape_alpha(t) >= 1.0 - 1e-6 || source_alpha(t) >= 1.0 - 1e-6, "{t}");
        }
    }

    #[test]
    fn a_riding_source_sits_on_its_part_of_the_flight() {
        let home = Rect::from_min_size(pos2(100.0, 200.0), vec2(300.0, 240.0));
        let picture = Rect::from_min_size(home.min, vec2(300.0, 169.0));
        let now = Rect::from_min_size(pos2(50.0, 80.0), vec2(600.0, 338.0));
        let t = ride(home, picture, now, 1.0);
        assert!((t * picture.min - now.min).length() < 0.01 && (t * picture.max - now.max).length() < 0.5);
        // At home, only the bounce: about the source's centre.
        let b = ride(home, picture, picture, 0.9);
        assert!((b * home.center() - home.center()).length() < 0.01);
        assert!(((b * home.max - b * home.min).x - 270.0).abs() < 0.01);
    }

    #[test]
    fn sources_bounce_where_a_morph_lands() {
        let ctx = Context::default();
        let r = Rect::from_min_size(pos2(10.0, 10.0), vec2(120.0, 40.0));
        let mut s = (0.0, 0.0, 0.0);
        frame(&ctx, 0.0, |ctx| land(ctx, r, 0.95));
        frame(&ctx, 0.016, |ctx| s = (bounce(ctx, r), bounce(ctx, r.translate(vec2(0.5, 0.5))), bounce(ctx, r.translate(vec2(40.0, 0.0)))));
        assert_eq!(s, (0.95, 0.95, 1.0));
        // Nothing said for two frames: at rest.
        frame(&ctx, 0.032, |_| {});
        frame(&ctx, 0.048, |ctx| s.0 = bounce(ctx, r));
        assert_eq!(s.0, 1.0);
    }
}
