//! Springs and clocks. Every animated value is a damped spring kept in egui's memory under an id; it asks for a
//! repaint only while it moves, so a still interface costs nothing. While the window is not live (unfocused,
//! hidden, or reduce motion) springs jump to their targets and looping clocks stop where they are. Looping
//! decorations share one modest frame rate and rest after half a minute without input.

use egui::{Color32, Context, Id};
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
    /// A card growing into its window and back, and the window following its content (the website's
    /// `expandSpring`): restrained, a few percent of overshoot.
    pub const EXPAND: Self = Self { stiffness: 340.0, damping: 27.0, mass: 0.85 };
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

/// Whether looping animations may run and springs animate (the window is focused and motion is allowed). Call it
/// every frame: it also notes input, which keeps looping decorations going (see [`clock`]).
pub fn set_live(ctx: &Context, live: bool) {
    let (t, input) = ctx.input(|i| (i.time, had_input(i)));
    ctx.data_mut(|d| {
        d.insert_temp(live_id(), live);
        if input {
            d.insert_temp(input_id(), t);
        }
    });
}

fn input_id() -> Id {
    Id::unique("orbiters-anim-input")
}

/// Something the person did this frame: a key, a click, a scroll, the pointer moving.
fn had_input(i: &egui::InputState) -> bool {
    !i.events.is_empty() || i.pointer.is_moving() || i.pointer.any_down()
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

/// Like [`spring_from`], settling once within `eps` of the target. One lock of egui's memory, the spring changed
/// where it lies (no copy put back each frame).
pub fn spring_eps<const N: usize>(ctx: &Context, id: Id, initial: [f32; N], target: [f32; N], s: Spring, eps: f32) -> [f32; N] {
    let t = now(ctx);
    let (value, moving) = ctx.data_mut(|d| {
        let live = d.get_temp::<bool>(live_id()).unwrap_or(true);
        let st = d.get_temp_mut_or_insert_with(id, || {
            // A first value that is not a number starts at the target (or at rest at zero).
            let start: [f32; N] = std::array::from_fn(|k| if initial[k].is_finite() { initial[k] } else { finite_or(target[k], 0.0) });
            let goal = std::array::from_fn(|k| finite_or(target[k], start[k]));
            State { value: start, velocity: [0.0; N], target: goal, time: t, settled: start == goal }
        });
        step(st, target, t, live, s, eps)
    });
    if moving {
        ctx.request_repaint();
    }
    value
}

fn finite_or(v: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

/// Advances a spring to time `t`; returns its value and whether it still moves. A target that is not a number
/// holds the spring where it is: it would never settle and the window would repaint forever.
fn step<const N: usize>(st: &mut State<N>, target: [f32; N], t: f64, live: bool, s: Spring, eps: f32) -> ([f32; N], bool) {
    let target: [f32; N] = std::array::from_fn(|k| finite_or(target[k], finite_or(st.value[k], 0.0)));
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
    if st.settled {
        st.time = t;
        return (st.value, false);
    }
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
    if still || st.value.iter().any(|v| !v.is_finite()) {
        st.value = target;
        st.velocity = [0.0; N];
        st.settled = true;
    }
    (st.value, !st.settled)
}

pub fn value(ctx: &Context, id: Id, target: f32, s: Spring) -> f32 {
    spring(ctx, id, [target], s)[0]
}

/// 0 → 1 (or back) as `on` changes.
pub fn toggle(ctx: &Context, id: Id, on: bool, s: Spring) -> f32 {
    value(ctx, id, if on { 1.0 } else { 0.0 }, s)
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

/// Frames per second of every looping decoration (the glow's drift, status dots, the record button's orbits, the
/// trail's pulse): one shared rate, so that they all move on the same frames instead of each asking for its own.
pub const DECOR_FPS: f32 = 15.0;

/// Looping decorations rest this long after the last input: a window left focused while its owner is in VR then
/// draws nothing until something changes.
pub const REST_AFTER: f64 = 30.0;

/// Nothing was done in the window for [`REST_AFTER`]: looping decorations rest ([`clock`] stands).
pub fn is_resting(ctx: &Context) -> bool {
    let t = now(ctx);
    ctx.data_mut(|d| t - *d.get_temp_mut_or_insert_with(input_id(), || t) >= REST_AFTER)
}

/// Seconds that advance only while the window is live and was used in the last [`REST_AFTER`] seconds: looping
/// animations read it, so they pause where they are instead of jumping. Asks for the next frame at the shared
/// [`DECOR_FPS`] (or `fps` when that is lower).
pub fn clock(ctx: &Context, fps: f32) -> f64 {
    let id = Id::unique("orbiters-anim-clock");
    let (t, input) = ctx.input(|i| (i.time, had_input(i)));
    let (elapsed, running) = ctx.data_mut(|d| {
        if input {
            d.insert_temp(input_id(), t);
        }
        let live = d.get_temp::<bool>(live_id()).unwrap_or(true);
        let last_input = *d.get_temp_mut_or_insert_with(input_id(), || t);
        let running = live && t - last_input < REST_AFTER;
        let c = d.get_temp_mut_or_insert_with(id, || Clock { elapsed: 0.0, last: t });
        if running {
            c.elapsed += (t - c.last).clamp(0.0, 0.1);
        }
        c.last = t;
        (c.elapsed, running)
    });
    if running {
        let fps = if fps.is_finite() { fps.clamp(1.0, DECOR_FPS) } else { DECOR_FPS };
        ctx.request_repaint_after(Duration::from_secs_f32(1.0 / fps));
    }
    elapsed
}

/// Time since `start` (seconds of real time), asking for frames until `duration` has passed.
pub fn since(ctx: &Context, start: f64, duration: f64) -> f64 {
    since_every(ctx, start, duration, f32::INFINITY)
}

/// [`since`] for long, slow changes (a toast's countdown bar): frames come at most `fps` times a second, not at
/// the monitor's 144 or 240 Hz.
pub fn since_every(ctx: &Context, start: f64, duration: f64, fps: f32) -> f64 {
    let elapsed = now(ctx) - start;
    if elapsed < duration {
        if !is_live(ctx) {
            ctx.request_repaint_after(Duration::from_secs_f64(duration - elapsed));
        } else if fps.is_finite() {
            ctx.request_repaint_after(Duration::from_secs_f64((1.0 / fps.max(1.0) as f64).min(duration - elapsed)));
        } else {
            ctx.request_repaint();
        }
    }
    elapsed.max(0.0)
}

/// The current time on egui's clock (for [`since`]).
pub fn time(ctx: &Context) -> f64 {
    now(ctx)
}

/// How long a [`wake`] waits for others: what lands within it shares one frame.
pub const WAKE: Duration = Duration::from_millis(8);

/// One frame, once, for something that changed outside the interface (the service's news, a picture a worker decoded),
/// shared with what else lands within [`WAKE`]. `request_repaint` runs two, to let layouts settle: a window told of
/// changes 30 times a second drew 60 frames, each one GPU time and, on the processor's integrated GPU, a copy to the
/// graphics card that drives the monitor. Never an animation's way to its next frame: that is [`clock`], a spring, or,
/// for something in progress, [`working`].
pub fn wake(ctx: &Context) {
    ctx.request_repaint_after(WAKE);
}

/// Something is in progress, and the one rule for every spot that says so (a spinner, a sweep, a waiting card): what
/// moves there reads this clock, at the decorations' shared low rate while the window is live and in use, standing
/// still with no frame asked for otherwise. What ends it, or moves it on, brings its own frame ([`wake`]): no spot
/// draws to look for that ([`working_read`] where nothing can say it).
pub fn working(ctx: &Context) -> f64 {
    clock(ctx, DECOR_FPS)
}

/// How often something in progress that has to be looked at is ([`working_read`]): in a window in use, and in one that
/// only shows.
pub const READ_EVERY: Duration = Duration::from_millis(200);
pub const READ_AWAY: Duration = Duration::from_secs(1);

/// [`working`], for what is known only by looking at it (a worker's percentage, a list read by a thread that has no
/// window to wake): looked at now and then while the window shows.
pub fn working_read(ctx: &Context) {
    let used = is_live(ctx) && !is_resting(ctx);
    if ctx.input(|i| i.viewport().visible()) != Some(false) {
        ctx.request_repaint_after(if used { READ_EVERY } else { READ_AWAY });
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one frame at `t` (with `events`) and returns how soon it asked for the next.
    fn frame(ctx: &Context, t: f64, events: Vec<egui::Event>, mut f: impl FnMut(&Context)) -> Duration {
        let input = egui::RawInput { time: Some(t), events, ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| f(ui.ctx()));
        // No renderer takes the font atlas here.
        out.textures_delta.clear();
        out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay).unwrap_or(Duration::MAX)
    }

    #[test]
    fn springs_settle_and_a_target_that_is_not_a_number_holds_them() {
        let ctx = Context::default();
        let id = Id::unique("spring");
        let mut v = [0f32; 2];
        for k in 0..240 {
            frame(&ctx, k as f64 / 60.0, vec![], |ctx| v = spring_from(ctx, id, [0.0, 0.0], [1.0, -2.0], Spring::BOUNCY));
        }
        assert_eq!(v, [1.0, -2.0]);
        // NaN (a degenerate layout) keeps the last value and asks for no frames.
        let delay = frame(&ctx, 5.0, vec![], |ctx| v = spring(ctx, id, [f32::NAN, -2.0], Spring::BOUNCY));
        assert_eq!(v, [1.0, -2.0]);
        let delay2 = frame(&ctx, 5.1, vec![], |ctx| v = spring(ctx, id, [f32::NAN, f32::INFINITY], Spring::BOUNCY));
        assert_eq!(v, [1.0, -2.0]);
        assert!(delay > Duration::from_secs(1) && delay2 > Duration::from_secs(1), "{delay:?} {delay2:?}");
        // A first value that is not a number starts at the target.
        frame(&ctx, 5.2, vec![], |ctx| v = spring_from(ctx, Id::unique("fresh"), [f32::NAN, 3.0], [0.5, 3.0], Spring::SOFT));
        assert_eq!(v, [0.5, 3.0]);
    }

    /// DPG-15: what a worker lands is one frame, and what lands together the same one. `request_repaint` is two.
    #[test]
    fn what_lands_from_outside_is_one_frame() {
        let ctx = Context::default();
        let mut t = 0.0;
        let mut frames = |land: &dyn Fn(&Context)| {
            for _ in 0..5 {
                t += 0.01;
                frame(&ctx, t, vec![], |_| {});
            }
            land(&ctx);
            let mut drawn = 1;
            t += 0.01;
            while frame(&ctx, t, vec![], |_| {}) < Duration::from_secs(1) {
                t += 0.01;
                drawn += 1;
            }
            drawn
        };
        assert_eq!(frames(&|ctx| ctx.request_repaint()), 2);
        assert_eq!(frames(&wake), 1);
        assert_eq!(frames(&|ctx| (0..10).for_each(|_| wake(ctx))), 1);
    }

    /// DPG-2: something in progress moves at the decorations' rate in a window in use, and asks for nothing in one that
    /// is not; what has to be looked at is, once a second, while the window shows.
    #[test]
    fn something_in_progress_keeps_to_the_shared_rate_and_rests() {
        let ctx = Context::default();
        for k in 0..5 {
            frame(&ctx, k as f64 * 0.01, vec![], |ctx| set_live(ctx, true));
        }
        let shared = Duration::from_secs_f32(1.0 / DECOR_FPS);
        let at = |t: f64, live: bool, read: bool| {
            frame(&ctx, t, vec![], |ctx| {
                set_live(ctx, live);
                if read {
                    working_read(ctx);
                } else {
                    working(ctx);
                }
            })
        };
        // (egui takes a predicted frame off what is asked.)
        let about = |asked: Duration, wanted: Duration| asked <= wanted && asked + Duration::from_millis(40) > wanted;
        let (moves, reads) = (at(1.0, true, false), at(1.1, true, true));
        assert!(about(moves, shared) && about(reads, READ_EVERY), "in use: {moves:?}, {reads:?}");
        for (t, live) in [(2.0, false), (2.0 + REST_AFTER + 1.0, true)] {
            assert_eq!(at(t, live, false), Duration::MAX, "no frame, live {live}");
            let looked = at(t + 0.1, live, true);
            assert!(about(looked, READ_AWAY), "{looked:?}, live {live}");
        }
    }

    #[test]
    fn looping_decorations_share_a_rate_and_rest_without_input() {
        let ctx = Context::default();
        // egui's own first frames ask for more frames: let them pass.
        for k in 0..5 {
            frame(&ctx, k as f64 * 0.01, vec![], |ctx| set_live(ctx, true));
        }
        let mut t = 0.0;
        let first = frame(&ctx, 0.05, vec![], |ctx| {
            set_live(ctx, true);
            t = clock(ctx, 60.0);
        });
        assert!(first <= Duration::from_secs_f32(1.0 / DECOR_FPS) + Duration::from_millis(1), "{first:?}");
        // Half a minute later, nothing touched: the loops stand still and ask for nothing.
        let resting = frame(&ctx, 0.05 + REST_AFTER + 1.0, vec![], |ctx| {
            set_live(ctx, true);
            t = clock(ctx, 60.0);
        });
        assert_eq!(t, 0.0);
        assert!(resting > Duration::from_secs(1), "{resting:?}");
        // The pointer moves: they go on from where they stood.
        let moved = vec![egui::Event::PointerMoved(egui::pos2(10.0, 10.0))];
        let woken = frame(&ctx, 0.05 + REST_AFTER + 2.0, moved, |ctx| {
            set_live(ctx, true);
            t = clock(ctx, 60.0);
        });
        assert!(t > 0.0 && t <= 0.1 + 1e-9, "{t}");
        assert!(woken <= Duration::from_secs_f32(1.0 / DECOR_FPS) + Duration::from_millis(1), "{woken:?}");
    }
}
