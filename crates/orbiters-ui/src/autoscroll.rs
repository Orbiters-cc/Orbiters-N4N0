//! Scrolling with the mouse wheel's button, as browsers do: a middle press shows the origin mark where it was made, and
//! moving the pointer away scrolls what is under the mark (any [`egui::ScrollArea`], the innermost that can go that way)
//! at a speed growing with the distance, past a small dead zone; releasing after moving ends it. A press and release
//! without moving keeps it going until the next press of any button (or Escape, or the wheel), which does nothing
//! else. It works on the window's raw input ([`raw_input`], once a frame before egui reads it): it takes the middle
//! button's events, keeps the pointer egui sees at the origin (so the area under the mark scrolls wherever the pointer
//! goes, and nothing under the pointer reacts meanwhile) and adds wheel steps. Views that use the middle button
//! themselves (3D views turn and pan with it) say so each frame with [`keep_middle`]: a press there is theirs.
//! [`paint`] draws the mark and sets the pointer's arrow.

use crate::theme;
use egui::{vec2, Context, CursorIcon, Event, Id, LayerId, Modifiers, Order, PointerButton, Pos2, RawInput, Rect, Shape, Stroke, Vec2};

/// Around the origin, a circle where the pointer scrolls nothing (points).
pub const DEAD_ZONE: f32 = 12.0;

/// Points per second at `past` points beyond the dead zone (signed): slow near the mark, quick far from it.
pub fn speed(past: f32) -> f32 {
    let d = past.abs();
    past.signum() * (d * 6.0 + d * d * 0.035).min(6000.0)
}

/// The scrolling under way.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Active {
    origin: Pos2,
    /// Where the pointer really is (egui sees it at the origin).
    pointer: Pos2,
    /// The middle button is still down from the press that started it.
    held: bool,
    /// The pointer left the dead zone since that press: releasing ends it.
    moved: bool,
    /// The input's time of the last wheel step.
    time: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Autoscroll {
    active: Option<Active>,
    /// The pointer's position as the window last said it.
    pointer: Option<Pos2>,
    /// A button whose press ended the scrolling: its release is taken too.
    swallow: Option<PointerButton>,
}

fn id() -> Id {
    Id::unique("orbiters-autoscroll")
}

fn keep_id() -> Id {
    Id::unique("orbiters-autoscroll-keep")
}

/// Whether the mouse wheel's button scrolls now.
pub fn active(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<Autoscroll>(id())).is_some_and(|a| a.active.is_some())
}

/// `rect` uses the middle button itself this frame (a 3D view): a press there is left to it.
pub fn keep_middle(ctx: &Context, rect: Rect) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let kept = d.get_temp_mut_or_default::<Vec<(u64, Rect)>>(keep_id());
        kept.retain(|(p, _)| *p + 1 >= pass);
        kept.push((pass, rect));
    });
}

fn kept(ctx: &Context, at: Pos2) -> bool {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<Vec<(u64, Rect)>>(keep_id())).is_some_and(|k| k.iter().any(|(p, r)| *p + 2 >= pass && r.contains(at)))
}

/// Takes the middle button's events from the window's input and adds the wheel steps; call it once a frame with the
/// input egui is about to read (eframe's `raw_input_hook`).
pub fn raw_input(ctx: &Context, raw: &mut RawInput) {
    let mut s = ctx.data(|d| d.get_temp::<Autoscroll>(id())).unwrap_or_default();
    let started_kept = |at: Pos2| kept(ctx, at);
    let events = std::mem::take(&mut raw.events);
    let mut out = Vec::with_capacity(events.len() + 1);
    for event in events {
        match &event {
            Event::PointerMoved(p) => {
                s.pointer = Some(*p);
                if let Some(a) = s.active.as_mut() {
                    a.pointer = *p;
                    if a.held && (*p - a.origin).length() > DEAD_ZONE {
                        a.moved = true;
                    }
                    // egui keeps the pointer at the origin.
                    continue;
                }
                out.push(event);
            }
            Event::PointerButton { pos, button, pressed, .. } => {
                let (pos, button, pressed) = (*pos, *button, *pressed);
                if !pressed && s.swallow == Some(button) {
                    s.swallow = None;
                    continue;
                }
                match (s.active, button, pressed) {
                    // Any press ends it, and does nothing else.
                    (Some(_), _, true) => {
                        end(&mut s, &mut out);
                        s.swallow = Some(button);
                    }
                    (Some(a), PointerButton::Middle, false) if a.held => {
                        if a.moved {
                            end(&mut s, &mut out);
                        } else if let Some(a) = s.active.as_mut() {
                            // Pressed and let go where it was: it goes on until the next press.
                            a.held = false;
                        }
                    }
                    (None, PointerButton::Middle, true) if !started_kept(pos) => {
                        s.active = Some(Active { origin: pos, pointer: pos, held: true, moved: false, time: None });
                    }
                    _ => out.push(event),
                }
            }
            Event::MouseWheel { .. } if s.active.is_some() => {
                end(&mut s, &mut out);
                out.push(event);
            }
            Event::Key { key: egui::Key::Escape, pressed: true, .. } if s.active.is_some() => end(&mut s, &mut out),
            Event::PointerGone | Event::WindowFocused(false) if s.active.is_some() => {
                end(&mut s, &mut out);
                out.push(event);
            }
            _ => out.push(event),
        }
    }
    if let (Some(a), Some(time)) = (s.active.as_mut(), raw.time) {
        let dt = a.time.map_or(0.0, |t| (time - t).clamp(0.0, 0.1)) as f32;
        a.time = Some(time);
        let delta = step(a.pointer - a.origin, dt);
        if delta != Vec2::ZERO {
            // The content moves against the pointer: below the mark, the page goes down (its content up).
            out.push(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: -delta,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: Modifiers::NONE,
            });
        }
        ctx.request_repaint();
    }
    raw.events = out;
    ctx.data_mut(|d| d.insert_temp(id(), s));
}

/// How far to scroll over `dt` seconds with the pointer `from` the origin (each axis past the dead zone).
pub fn step(from: Vec2, dt: f32) -> Vec2 {
    let along = |v: f32| if v.abs() <= DEAD_ZONE { 0.0 } else { speed(v - DEAD_ZONE * v.signum()) * dt };
    vec2(along(from.x), along(from.y))
}

/// Ends the scrolling: egui sees the pointer where it really is again.
fn end(s: &mut Autoscroll, out: &mut Vec<Event>) {
    if let Some(a) = s.active.take() {
        out.push(Event::PointerMoved(a.pointer));
    }
}

/// The origin mark while it scrolls (a disc with four arrows, the one it goes towards lit), and the pointer's arrow
/// for the way it scrolls; call it once a frame after the interface.
pub fn paint(ctx: &Context) {
    let Some(a) = ctx.data(|d| d.get_temp::<Autoscroll>(id())).and_then(|s| s.active) else { return };
    let p = ctx.layer_painter(LayerId::new(Order::Tooltip, id()));
    let c = a.origin;
    let r = 15.0;
    p.circle_filled(c, r, theme::black(0.62));
    p.circle_stroke(c, r, Stroke::new(1.2, theme::white(0.55)));
    p.circle_filled(c, 2.2, theme::white(0.9));
    let from = a.pointer - c;
    let going = step(from, 1.0);
    for dir in [vec2(0.0, -1.0), vec2(0.0, 1.0), vec2(-1.0, 0.0), vec2(1.0, 0.0)] {
        let on = going.dot(dir) > 0.0;
        let tip = c + dir * 11.0;
        let side = vec2(-dir.y, dir.x) * 3.6;
        let base = c + dir * 6.5;
        let color = if on { theme::ACCENT } else { theme::white(0.8) };
        p.add(Shape::convex_polygon(vec![tip, base + side, base - side], color, Stroke::NONE));
    }
    ctx.set_cursor_icon(cursor(going));
}

/// The pointer's arrow for scrolling `going` (each axis's sign).
fn cursor(going: Vec2) -> CursorIcon {
    match (going.x.partial_cmp(&0.0), going.y.partial_cmp(&0.0)) {
        (Some(std::cmp::Ordering::Equal), Some(std::cmp::Ordering::Less)) => CursorIcon::ResizeNorth,
        (Some(std::cmp::Ordering::Equal), Some(std::cmp::Ordering::Greater)) => CursorIcon::ResizeSouth,
        (Some(std::cmp::Ordering::Less), Some(std::cmp::Ordering::Equal)) => CursorIcon::ResizeWest,
        (Some(std::cmp::Ordering::Greater), Some(std::cmp::Ordering::Equal)) => CursorIcon::ResizeEast,
        (Some(std::cmp::Ordering::Less), Some(std::cmp::Ordering::Less)) => CursorIcon::ResizeNorthWest,
        (Some(std::cmp::Ordering::Greater), Some(std::cmp::Ordering::Less)) => CursorIcon::ResizeNorthEast,
        (Some(std::cmp::Ordering::Less), Some(std::cmp::Ordering::Greater)) => CursorIcon::ResizeSouthWest,
        (Some(std::cmp::Ordering::Greater), Some(std::cmp::Ordering::Greater)) => CursorIcon::ResizeSouthEast,
        _ => CursorIcon::AllScroll,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn press(button: PointerButton, pressed: bool, pos: Pos2) -> Event {
        Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }
    }

    /// Runs one frame of `events` at `time` through [`raw_input`] and egui; returns what egui got.
    fn frame(ctx: &Context, time: f64, events: Vec<Event>) -> Vec<Event> {
        let mut raw = RawInput { time: Some(time), events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), ..Default::default() };
        raw_input(ctx, &mut raw);
        let seen = raw.events.clone();
        run(ctx, raw, |_| {});
        seen
    }

    fn run(ctx: &Context, raw: RawInput, f: impl FnMut(&mut egui::Ui)) {
        ctx.run_ui(raw, f).textures_delta.clear();
    }

    fn wheel(events: &[Event]) -> Vec2 {
        events.iter().map(|e| if let Event::MouseWheel { delta, .. } = e { *delta } else { Vec2::ZERO }).fold(Vec2::ZERO, |a, b| a + b)
    }

    #[test]
    fn the_speed_grows_with_the_distance_past_the_dead_zone() {
        assert_eq!(step(vec2(5.0, -DEAD_ZONE), 1.0), Vec2::ZERO);
        let near = step(vec2(0.0, DEAD_ZONE + 20.0), 1.0).y;
        let far = step(vec2(0.0, DEAD_ZONE + 200.0), 1.0).y;
        assert!(near > 0.0 && far > near * 10.0, "{near} {far}");
        assert!(step(vec2(-60.0, 0.0), 0.5).x < 0.0);
    }

    #[test]
    fn a_middle_drag_scrolls_and_its_release_ends_it() {
        let ctx = Context::default();
        let o = pos2(400.0, 300.0);
        let seen = frame(&ctx, 0.0, vec![Event::PointerMoved(o), press(PointerButton::Middle, true, o)]);
        assert!(active(&ctx));
        assert!(!seen.iter().any(|e| matches!(e, Event::PointerButton { .. })), "the press is taken");
        let seen = frame(&ctx, 0.05, vec![Event::PointerMoved(pos2(400.0, 420.0))]);
        assert!(!seen.iter().any(|e| matches!(e, Event::PointerMoved(_))), "egui keeps the pointer at the mark");
        let seen = frame(&ctx, 0.1, vec![]);
        assert!(wheel(&seen).y < 0.0, "below the mark the content goes up: {:?}", wheel(&seen));
        let seen = frame(&ctx, 0.15, vec![press(PointerButton::Middle, false, pos2(400.0, 420.0))]);
        assert!(!active(&ctx));
        assert!(seen.iter().any(|e| matches!(e, Event::PointerMoved(p) if p.y == 420.0)), "egui sees the pointer again");
    }

    #[test]
    fn a_click_without_moving_sticks_until_the_next_press_which_does_nothing_else() {
        let ctx = Context::default();
        let o = pos2(200.0, 200.0);
        frame(&ctx, 0.0, vec![press(PointerButton::Middle, true, o)]);
        frame(&ctx, 0.05, vec![press(PointerButton::Middle, false, o)]);
        assert!(active(&ctx), "sticky");
        frame(&ctx, 0.1, vec![Event::PointerMoved(pos2(200.0, 120.0))]);
        assert!(wheel(&frame(&ctx, 0.15, vec![])).y > 0.0, "above the mark it goes up");
        let seen = frame(&ctx, 0.2, vec![press(PointerButton::Primary, true, pos2(200.0, 120.0))]);
        assert!(!active(&ctx));
        assert!(!seen.iter().any(|e| matches!(e, Event::PointerButton { .. })), "the click that ends it clicks nothing");
        let seen = frame(&ctx, 0.25, vec![press(PointerButton::Primary, false, pos2(200.0, 120.0))]);
        assert!(!seen.iter().any(|e| matches!(e, Event::PointerButton { .. })), "nor its release");
        let seen = frame(&ctx, 0.3, vec![press(PointerButton::Primary, true, pos2(200.0, 120.0))]);
        assert!(seen.iter().any(|e| matches!(e, Event::PointerButton { pressed: true, .. })), "the next click is a click");
    }

    #[test]
    fn views_that_use_the_middle_button_keep_it() {
        let ctx = Context::default();
        let view = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 300.0));
        run(&ctx, RawInput::default(), |ui| keep_middle(ui.ctx(), view));
        let seen = frame(&ctx, 0.0, vec![press(PointerButton::Middle, true, pos2(100.0, 100.0))]);
        assert!(!active(&ctx));
        assert!(seen.iter().any(|e| matches!(e, Event::PointerButton { button: PointerButton::Middle, .. })), "the view gets its press");
    }
}
