//! Springs and clocks. Every animated value is a damped spring kept in egui's memory under an id; it asks for a
//! repaint only while it moves, so a still interface costs nothing. While the window is not live (unfocused,
//! hidden, or reduce motion) springs jump to their targets and looping clocks stop where they are.

use egui::{Color32, Context, Id, Pos2, Rect, Vec2};
use std::time::Duration;

/// Spring constants (the web interface's motion presets).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub stiffness: f32,
    pub damping: f32,
    pub mass: f32,
}

impl Spring {
    /// Presses and small moves: quick, barely bouncing.
    pub const SNAPPY: Self = Self { stiffness: 560.0, damping: 32.0, mass: 0.7 };
    /// Pills, toggles and highlights that slide: visibly bouncy.
    pub const BOUNCY: Self = Self { stiffness: 420.0, damping: 17.0, mass: 0.8 };
    /// Entrances and layout changes.
    pub const SOFT: Self = Self { stiffness: 170.0, damping: 24.0, mass: 1.0 };
    /// Shape morphs (record glyph, session card to detail).
    pub const MORPH: Self = Self { stiffness: 300.0, damping: 26.0, mass: 0.9 };
    /// Counting numbers.
    pub const NUMBER: Self = Self { stiffness: 120.0, damping: 22.0, mass: 0.6 };
    /// Slow colour washes (the background mood), critically damped.
    pub const SLOW: Self = Self { stiffness: 30.0, damping: 11.0, mass: 1.0 };
}

#[derive(Clone, Copy)]
struct State<const N: usize> {
    value: [f32; N],
    velocity: [f32; N],
    target: [f32; N],
    time: f64,
    settled: bool,
}

fn live_id() -> Id {
    Id::unique("orbiters-anim-live")
}

/// Whether looping animations may run and springs animate (the window is focused and motion is allowed).
pub fn set_live(ctx: &Context, live: bool) {
    ctx.data_mut(|d| d.insert_temp(live_id(), live));
}

pub fn is_live(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp(live_id())).unwrap_or(true)
}

fn now(ctx: &Context) -> f64 {
    ctx.input(|i| i.time)
}

/// A spring towards `target`, starting at the target the first time it is seen.
pub fn spring<const N: usize>(ctx: &Context, id: Id, target: [f32; N], s: Spring) -> [f32; N] {
    spring_eps(ctx, id, target, target, s, 0.001)
}

/// A spring towards `target` that starts at `initial` the first time it is seen (entrances).
pub fn spring_from<const N: usize>(ctx: &Context, id: Id, initial: [f32; N], target: [f32; N], s: Spring) -> [f32; N] {
    spring_eps(ctx, id, initial, target, s, 0.001)
}

/// Like [`spring_from`], settling once within `eps` of the target.
pub fn spring_eps<const N: usize>(ctx: &Context, id: Id, initial: [f32; N], target: [f32; N], s: Spring, eps: f32) -> [f32; N] {
    let t = now(ctx);
    let live = is_live(ctx);
    let mut st =
        ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(id, || State { value: initial, velocity: [0.0; N], target, time: t, settled: initial == target }));
    if st.target != target {
        if st.settled {
            // Restart the clock: the first step is one frame long, whatever time passed while it rested.
            st.time = t - 1.0 / 60.0;
        }
        st.target = target;
        st.settled = false;
    }
    if !live {
        st.value = target;
        st.velocity = [0.0; N];
        st.settled = true;
    }
    if !st.settled {
        let mut dt = (t - st.time).clamp(0.0, 0.05) as f32;
        st.time = t;
        while dt > 0.0 {
            let h = dt.min(1.0 / 240.0);
            dt -= h;
            for ((x, v), goal) in st.value.iter_mut().zip(st.velocity.iter_mut()).zip(target) {
                let a = (-s.stiffness * (*x - goal) - s.damping * *v) / s.mass;
                *v += a * h;
                *x += *v * h;
            }
        }
        let still = (0..N).all(|k| (st.value[k] - target[k]).abs() < eps && st.velocity[k].abs() < eps * 10.0);
        if still {
            st.value = target;
            st.velocity = [0.0; N];
            st.settled = true;
        } else {
            ctx.request_repaint();
        }
    } else {
        st.time = t;
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
    st.value
}

/// Gives a spring a push (e.g. a little bump on a button).
pub fn kick<const N: usize>(ctx: &Context, id: Id, velocity: [f32; N]) {
    if !is_live(ctx) {
        return;
    }
    ctx.data_mut(|d| {
        if let Some(mut st) = d.get_temp::<State<N>>(id) {
            for (v, dv) in st.velocity.iter_mut().zip(velocity) {
                *v += dv;
            }
            st.settled = false;
            d.insert_temp(id, st);
        }
    });
    ctx.request_repaint();
}

/// Forgets a spring so it starts fresh next time (entrance animations that should replay).
pub fn reset<const N: usize>(ctx: &Context, id: Id) {
    ctx.data_mut(|d| d.remove::<State<N>>(id));
}

pub fn value(ctx: &Context, id: Id, target: f32, s: Spring) -> f32 {
    spring(ctx, id, [target], s)[0]
}

/// 0 → 1 (or back) as `on` changes.
pub fn toggle(ctx: &Context, id: Id, on: bool, s: Spring) -> f32 {
    value(ctx, id, if on { 1.0 } else { 0.0 }, s)
}

pub fn vec2(ctx: &Context, id: Id, target: Vec2, s: Spring) -> Vec2 {
    let [x, y] = spring(ctx, id, [target.x, target.y], s);
    Vec2::new(x, y)
}

pub fn pos2(ctx: &Context, id: Id, target: Pos2, s: Spring) -> Pos2 {
    vec2(ctx, id, target.to_vec2(), s).to_pos2()
}

pub fn rect(ctx: &Context, id: Id, target: Rect, s: Spring) -> Rect {
    let [a, b, c, d] = spring(ctx, id, [target.min.x, target.min.y, target.max.x, target.max.y], s);
    Rect::from_min_max(Pos2::new(a, b), Pos2::new(c, d))
}

/// A colour easing towards `target` (channels in gamma space, premultiplied).
pub fn color(ctx: &Context, id: Id, target: Color32, s: Spring) -> Color32 {
    let t = target.to_array().map(f32::from);
    let [r, g, b, a] = spring_eps(ctx, id, t, t, s, 0.5);
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_premultiplied(c(r), c(g), c(b), c(a))
}

#[derive(Clone, Copy)]
struct Clock {
    elapsed: f64,
    last: f64,
}

/// Seconds that advance only while the window is live: looping animations read it so they pause where they
/// are instead of jumping. Asks for the next frame at `fps` (decorative loops need no more than 30-60).
pub fn clock(ctx: &Context, fps: f32) -> f64 {
    let id = Id::unique("orbiters-anim-clock");
    let t = now(ctx);
    let live = is_live(ctx);
    let mut c = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(id, || Clock { elapsed: 0.0, last: t }));
    if live {
        c.elapsed += (t - c.last).clamp(0.0, 0.1);
        ctx.request_repaint_after(Duration::from_secs_f32(1.0 / fps.max(1.0)));
    }
    c.last = t;
    ctx.data_mut(|d| d.insert_temp(id, c));
    c.elapsed
}

/// Time since `start` (seconds of real time), asking for frames until `duration` has passed.
pub fn since(ctx: &Context, start: f64, duration: f64) -> f64 {
    let elapsed = now(ctx) - start;
    if elapsed < duration {
        if is_live(ctx) {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_secs_f64(duration - elapsed));
        }
    }
    elapsed.max(0.0)
}

/// The current time on egui's clock (for [`since`]).
pub fn time(ctx: &Context) -> f64 {
    now(ctx)
}

/// Easing curves.
pub mod ease {
    pub fn out_cubic(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(3)
    }

    /// The web's `ease-out-expo` (cubic-bezier(0.16, 1, 0.3, 1)), approximated.
    pub fn out_expo(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if t >= 1.0 {
            1.0
        } else {
            1.0 - 2f32.powf(-10.0 * t)
        }
    }

    pub fn in_out(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Progress of a segment `[start, start + duration]` at time `t`, eased.
    pub fn segment(t: f32, start: f32, duration: f32, curve: fn(f32) -> f32) -> f32 {
        curve(((t - start) / duration.max(1e-6)).clamp(0.0, 1.0))
    }

    /// Keyframes at even times, eased per segment (like the web's keyframe arrays).
    pub fn keyframes(t: f32, values: &[f32], curve: fn(f32) -> f32) -> f32 {
        match values.len() {
            0 => 0.0,
            1 => values[0],
            n => {
                let x = t.clamp(0.0, 1.0) * (n - 1) as f32;
                let i = (x.floor() as usize).min(n - 2);
                let k = curve(x - i as f32);
                values[i] + (values[i + 1] - values[i]) * k
            }
        }
    }

    /// Keyframes at given times.
    pub fn keyframes_at(t: f32, times: &[f32], values: &[f32], curve: fn(f32) -> f32) -> f32 {
        if t <= times[0] {
            return values[0];
        }
        for i in 1..times.len() {
            if t <= times[i] {
                let k = curve((t - times[i - 1]) / (times[i] - times[i - 1]).max(1e-6));
                return values[i - 1] + (values[i] - values[i - 1]) * k;
            }
        }
        values[values.len() - 1]
    }

    /// Ping-pong phase in 0..1 for a loop of `period` seconds (like `repeatType: "mirror"`).
    pub fn mirror(t: f64, period: f64) -> f32 {
        let cycle = (t / period).floor();
        let local = ((t / period) - cycle) as f32;
        if cycle as i64 % 2 == 0 {
            local
        } else {
            1.0 - local
        }
    }
}
