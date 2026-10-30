//! Who points at what this turn: each hand's laser on the floating panel, its bar or the watch; the panel's pointer and
//! its press; a grab and where the held panel goes. Poses and buttons in, what to do out, so that it is tested without
//! SteamVR.
//!
//! Only the laser on the panel moves its pointer: a hand pointing elsewhere (at VRChat) presses nothing here, and the
//! trigger goes to VRChat as always. A press keeps its hand's laser on the panel's plane until the trigger is let go
//! (a slider dragged past the panel's edge, the Talk button held), and holds the pointer where it pressed until the
//! laser has gone [`STEADY_M`] from there: pulling a trigger shakes the hand. The panel is grabbed as N4N0's screen is,
//! by its handle (a hand within [`HANDLE_REACH_M`] of it gripping), or from afar with a grip while the laser is on the
//! panel or its bar, or with the trigger on the handle; held, it rides the hand from where it was taken, turned
//! towards the eyes ([`space::carry`]); let go, it stays.

use super::space::{self, Hit, Quad, V3};
use orbiters_openvr::Matrix34;

/// One hand this turn: its controller's pose, its laser (the tip), its trigger and its grip held.
#[derive(Clone, Copy, Debug)]
pub struct HandIn {
    pub grip: Matrix34,
    pub tip: Matrix34,
    pub trigger: bool,
    pub grab: bool,
}

/// What the lasers can point at this turn.
#[derive(Clone, Copy, Debug, Default)]
pub struct Targets {
    pub panel: Option<Quad>,
    pub bar: Option<Quad>,
    /// The bar's handle: its part of the bar (u0, v0, u1, v1) and its middle in the play space.
    pub handle: Option<([f32; 4], V3)>,
    /// The watch, and the hand it is on (the other one presses it).
    pub watch: Option<(usize, Quad)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Panel,
    Bar,
    Watch,
}

/// A hand this near the handle takes it with its grip, as N4N0's screen's handle is taken.
pub const HANDLE_REACH_M: f32 = 0.12;
/// A fingertip this near the watch is on it.
pub const WATCH_TOUCH_M: f32 = 0.035;
/// The watch is round and small: a laser this far past its edge is still on it.
const WATCH_MARGIN_M: f32 = 0.012;
/// A press holds the pointer where it pressed until the laser has gone this far from there on the panel.
pub const STEADY_M: f32 = 0.008;

/// The panel held: by which hand, taken with its trigger (on the handle) or its grip, the carry frame then and the
/// panel's place then.
#[derive(Clone, Copy, Debug)]
struct Hold {
    hand: usize,
    by_trigger: bool,
    carry: Matrix34,
    taken: Matrix34,
}

/// The panel's button held: by which hand, where it pressed, and whether the pointer still stays there.
#[derive(Clone, Copy, Debug)]
struct Press {
    hand: usize,
    uv: [f32; 2],
    point: V3,
    steady: bool,
}

#[derive(Debug)]
pub struct Interaction {
    /// Each hand's trigger and grip as they were last turn (taken as held when unknown: a pull is seen go down).
    trigger: [bool; 2],
    grab: [bool; 2],
    /// The hand the panel's pointer follows.
    active: Option<usize>,
    press: Option<Press>,
    hold: Option<Hold>,
    /// The pointer was on the panel last turn.
    pointing: bool,
}

/// What this turn does.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Outcome {
    /// Each hand's laser: what it is on and where (none: hidden).
    pub lasers: [Option<(Target, Hit)>; 2],
    /// The panel's pointer (u, v), none while no laser is on the panel.
    pub pointer: Option<[f32; 2]>,
    /// The panel's button went down (`true`) or up (`false`): at the pointer, or where it last was.
    pub button: Option<bool>,
    /// The pointer left the panel.
    pub gone: bool,
    /// A laser on the bar (u, v), and a trigger pulled there.
    pub bar_hover: Option<[f32; 2]>,
    pub bar_press: Option<[f32; 2]>,
    /// A laser or a fingertip on the watch, and a trigger pulled there.
    pub watch_hover: bool,
    pub watch_press: bool,
    /// Where the held panel is now; it was let go this turn (where it last was is to keep).
    pub moved: Option<Matrix34>,
    pub released: bool,
    /// The hand holding the panel.
    pub holding: Option<usize>,
}

impl Default for Interaction {
    fn default() -> Self {
        Self { trigger: [true; 2], grab: [true; 2], active: None, press: None, hold: None, pointing: false }
    }
}

impl Interaction {
    /// The hand holding the panel.
    pub fn holding(&self) -> Option<usize> {
        self.hold.map(|h| h.hand)
    }

    /// Everything let go (the dashboard took the controllers, nothing to point at). Returns whether the panel's button
    /// was held (its end is the caller's to send). The buttons are then taken as held: one held meanwhile presses
    /// nothing when there is something to point at again, only a new pull does.
    pub fn reset(&mut self) -> bool {
        let pressed = self.press.is_some();
        *self = Self::default();
        pressed
    }

    pub fn step(&mut self, head: &Matrix34, hands: [Option<HandIn>; 2], targets: &Targets) -> Outcome {
        let mut out = Outcome::default();
        let trigger = hands.map(|h| h.is_some_and(|h| h.trigger));
        let grab = hands.map(|h| h.is_some_and(|h| h.grab));
        let (was_trigger, was_grab) = (self.trigger, self.grab);
        let pulled = |i: usize| trigger[i] && !was_trigger[i];
        let gripped = |i: usize| grab[i] && !was_grab[i];

        // The held panel follows its hand, until that hand lets go (or is lost, or the panel closed).
        if let Some(hold) = self.hold {
            let holds = if hold.by_trigger { trigger[hold.hand] } else { grab[hold.hand] };
            match hands[hold.hand].filter(|_| holds && targets.panel.is_some()) {
                Some(h) => out.moved = Some(space::carried(&space::carry(space::position(&h.grip), head), &hold.carry, &hold.taken)),
                None => {
                    self.hold = None;
                    out.released = true;
                }
            }
        }

        // Where each laser is: the nearest of what it meets; a press keeps its laser on the panel's plane.
        for (i, hand) in hands.iter().enumerate() {
            let Some(h) = hand else { continue };
            if self.hold.is_some_and(|hold| hold.hand == i) {
                continue;
            }
            let (o, d) = space::ray(&h.tip);
            if self.press.is_some_and(|p| p.hand == i) {
                out.lasers[i] = targets.panel.and_then(|q| q.plane_hit(o, d)).map(|hit| (Target::Panel, hit));
                continue;
            }
            let mut best: Option<(Target, Hit)> = None;
            let mut consider = |target: Target, hit: Option<Hit>| {
                if let Some(hit) = hit {
                    if best.is_none_or(|(_, b)| hit.distance < b.distance) {
                        best = Some((target, hit));
                    }
                }
            };
            consider(Target::Panel, targets.panel.and_then(|q| q.hit(o, d, 0.0)));
            consider(Target::Bar, targets.bar.and_then(|q| q.hit(o, d, 0.0)));
            if let Some((_, watch)) = targets.watch.filter(|(on, _)| *on != i) {
                let (at, gap) = watch.nearest(o);
                if gap < WATCH_TOUCH_M {
                    best = Some((Target::Watch, at));
                } else {
                    consider(Target::Watch, watch.hit(o, d, WATCH_MARGIN_M));
                }
            }
            out.lasers[i] = best;
        }

        // A grip on the panel or its bar, or at the handle, takes the panel (from the other hand, too).
        for i in 0..2 {
            let (Some(h), Some(panel)) = (hands[i], targets.panel) else { continue };
            if !gripped(i) || self.press.is_some_and(|p| p.hand == i) {
                continue;
            }
            let on = matches!(out.lasers[i], Some((Target::Panel | Target::Bar, _)));
            let near = targets.handle.is_some_and(|(_, at)| space::length(space::sub(space::position(&h.grip), at)) < HANDLE_REACH_M);
            if on || near {
                let at = out.moved.unwrap_or(panel.transform);
                self.take(&mut out, i, false, &h, head, at);
            }
        }

        // A trigger pulled: on the watch, a bar's button (the handle takes the panel), the panel's button.
        for i in 0..2 {
            let Some(h) = hands[i] else { continue };
            if !pulled(i) || self.hold.is_some_and(|hold| hold.hand == i) {
                continue;
            }
            match out.lasers[i] {
                Some((Target::Watch, _)) => out.watch_press = true,
                Some((Target::Bar, hit)) => match (targets.handle, targets.panel) {
                    (Some((r, _)), Some(panel)) if (r[0]..=r[2]).contains(&hit.uv[0]) && (r[1]..=r[3]).contains(&hit.uv[1]) => {
                        let at = out.moved.unwrap_or(panel.transform);
                        self.take(&mut out, i, true, &h, head, at);
                    }
                    _ => out.bar_press = Some(hit.uv),
                },
                Some((Target::Panel, hit)) if self.press.is_none() => {
                    self.active = Some(i);
                    self.press = Some(Press { hand: i, uv: hit.uv, point: hit.point, steady: true });
                    out.button = Some(true);
                }
                _ => {}
            }
        }

        // The pointer: the pressing hand's, else the hand it followed while that one stays on the panel, else whichever
        // is on it (the right first).
        let on_panel = |i: usize| matches!(out.lasers[i], Some((Target::Panel, _)));
        self.active = match self.press {
            Some(p) => Some(p.hand),
            None => self.active.filter(|&i| on_panel(i)).or_else(|| [1, 0].into_iter().find(|&i| on_panel(i))),
        };
        if let Some((_, hit)) = self.active.and_then(|i| out.lasers[i]) {
            out.pointer = Some(hit.uv);
            if let Some(p) = self.press.as_mut().filter(|p| p.steady) {
                if space::length(space::sub(hit.point, p.point)) < STEADY_M {
                    out.pointer = Some(p.uv);
                } else {
                    p.steady = false;
                }
            }
        }
        // The press ends as its trigger is let go (or its hand is lost).
        if let Some(p) = self.press {
            if !trigger[p.hand] || targets.panel.is_none() {
                self.press = None;
                out.button = Some(false);
            }
        }
        out.gone = self.pointing && out.pointer.is_none();
        self.pointing = out.pointer.is_some();

        let hover = |t: Target| self.active.into_iter().chain([1, 0]).find_map(|i| out.lasers[i].filter(|(on, _)| *on == t)).map(|(_, hit)| hit.uv);
        out.bar_hover = hover(Target::Bar);
        out.watch_hover = hover(Target::Watch).is_some();
        out.holding = self.holding();
        self.trigger = trigger;
        self.grab = grab;
        out
    }

    /// Hand `i` takes the panel where it is (`at`), with its trigger or its grip.
    fn take(&mut self, out: &mut Outcome, i: usize, by_trigger: bool, h: &HandIn, head: &Matrix34, at: Matrix34) {
        self.hold = Some(Hold { hand: i, by_trigger, carry: space::carry(space::position(&h.grip), head), taken: at });
        out.lasers[i] = None;
        out.moved = Some(at);
        if self.active == Some(i) {
            self.active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use space::{before_eyes, facing, from_axes, position};

    const PANEL_W: f32 = 0.8;
    const PANEL_H: f32 = 0.5;

    fn head() -> Matrix34 {
        from_axes([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.6, 0.0])
    }

    fn targets(panel: Matrix34) -> Targets {
        let bar = space::offset(&panel, [0.0, -(PANEL_H / 2.0 + 0.035), 0.0]);
        Targets {
            panel: Some(Quad { transform: panel, width: PANEL_W, height: PANEL_H }),
            bar: Some(Quad { transform: bar, width: 0.3, height: 0.045 }),
            handle: Some(([0.3, 0.0, 0.7, 1.0], position(&bar))),
            watch: None,
        }
    }

    /// A hand at `at` pointing at `towards`.
    fn hand(at: V3, towards: V3, trigger: bool, grab: bool) -> HandIn {
        let tip = facing(at, space::add(at, space::sub(at, towards)), [0.0, 1.0, 0.0]).unwrap();
        HandIn { grip: tip, tip, trigger, grab }
    }

    #[test]
    fn a_laser_on_the_panel_points_and_presses_there_only() {
        let panel = before_eyes(&head(), 0.7, 0.1);
        let t = targets(panel);
        let mut i = Interaction::default();
        let middle = position(&panel);
        let right_hand = [0.2, 1.2, -0.2];
        // Pointing at its middle: the pointer there, the right hand's laser shown, nothing pressed.
        let out = i.step(&head(), [None, Some(hand(right_hand, middle, false, false))], &t);
        let uv = out.pointer.unwrap();
        assert!((uv[0] - 0.5).abs() < 0.01 && (uv[1] - 0.5).abs() < 0.01, "{uv:?}");
        assert!(matches!(out.lasers[1], Some((Target::Panel, _))) && out.lasers[0].is_none() && out.button.is_none());
        // The trigger pulled there: down.
        let out = i.step(&head(), [None, Some(hand(right_hand, middle, true, false))], &t);
        assert_eq!(out.button, Some(true));
        // Let go: up, a click.
        let out = i.step(&head(), [None, Some(hand(right_hand, middle, false, false))], &t);
        assert_eq!(out.button, Some(false));
        // Pointing at VRChat, away from the panel: the trigger presses nothing here and no laser shows.
        let away = [3.0, 1.0, 1.0];
        let out = i.step(&head(), [None, Some(hand(right_hand, away, false, false))], &t);
        assert!(out.gone && out.pointer.is_none() && out.lasers[1].is_none());
        for trigger in [true, false, true] {
            let out = i.step(&head(), [None, Some(hand(right_hand, away, trigger, false))], &t);
            assert!(out.button.is_none() && out.pointer.is_none() && !out.gone);
        }
        // Coming back with the trigger still held clicks nothing (only a pull on it does).
        let out = i.step(&head(), [None, Some(hand(right_hand, middle, true, false))], &t);
        assert!(out.pointer.is_some() && out.button.is_none());
    }

    #[test]
    fn a_press_holds_still_then_drags_past_the_edge() {
        let panel = before_eyes(&head(), 0.7, 0.1);
        let t = targets(panel);
        let mut i = Interaction::default();
        let at = [0.1, 1.2, -0.1];
        let middle = position(&panel);
        i.step(&head(), [None, Some(hand(at, middle, false, false))], &t);
        let down = i.step(&head(), [None, Some(hand(at, middle, true, false))], &t).pointer.unwrap();
        // The pull shakes the laser 4 mm: the pointer stays where it pressed.
        let shaken = i.step(&head(), [None, Some(hand(at, space::add(middle, [0.004, 0.0, 0.0]), true, false))], &t);
        assert_eq!(shaken.pointer, Some(down));
        // Dragged on, past the panel's right edge: the pointer follows, past 1.
        let past = i.step(&head(), [None, Some(hand(at, space::add(middle, [0.6, 0.0, 0.0]), true, false))], &t);
        assert!(past.pointer.unwrap()[0] > 1.0 && matches!(past.lasers[1], Some((Target::Panel, _))), "{past:?}");
        // Back near where it pressed: it follows now (no longer held still).
        let back = i.step(&head(), [None, Some(hand(at, space::add(middle, [0.004, 0.0, 0.0]), true, false))], &t);
        assert_ne!(back.pointer, Some(down));
        // Let go out there: up, then the pointer is gone with the laser.
        i.step(&head(), [None, Some(hand(at, space::add(middle, [0.6, 0.0, 0.0]), true, false))], &t);
        let up = i.step(&head(), [None, Some(hand(at, space::add(middle, [0.6, 0.0, 0.0]), false, false))], &t);
        assert_eq!(up.button, Some(false));
        let after = i.step(&head(), [None, Some(hand(at, space::add(middle, [0.6, 0.0, 0.0]), false, false))], &t);
        assert!(after.gone && after.lasers[1].is_none());
    }

    #[test]
    fn a_trigger_held_before_there_was_anything_to_point_at_presses_nothing() {
        // The watch's press opened the panel with the trigger still held, or a trigger held in VRChat (a pickup) as the
        // laser comes onto the panel: only a new pull presses.
        let panel = before_eyes(&head(), 0.7, 0.1);
        let t = targets(panel);
        let (at, middle) = ([0.1, 1.2, -0.1], position(&panel));
        let mut i = Interaction::default();
        let out = i.step(&head(), [None, Some(hand(at, middle, true, true))], &t);
        assert!(out.button.is_none() && out.holding.is_none() && out.pointer.is_some());
        i.step(&head(), [None, Some(hand(at, middle, true, true))], &t);
        // After a reset (SteamVR's dashboard came and went) as well.
        i.step(&head(), [None, Some(hand(at, middle, false, false))], &t);
        i.reset();
        assert!(i.step(&head(), [None, Some(hand(at, middle, true, false))], &t).button.is_none());
        i.step(&head(), [None, Some(hand(at, middle, false, false))], &t);
        assert_eq!(i.step(&head(), [None, Some(hand(at, middle, true, false))], &t).button, Some(true), "a new pull");
    }

    #[test]
    fn a_lost_hand_lets_go_of_its_press() {
        let panel = before_eyes(&head(), 0.7, 0.1);
        let t = targets(panel);
        let mut i = Interaction::default();
        let (at, middle) = ([0.1, 1.2, -0.1], position(&panel));
        i.step(&head(), [None, Some(hand(at, middle, false, false))], &t);
        assert_eq!(i.step(&head(), [None, Some(hand(at, middle, true, false))], &t).button, Some(true));
        let out = i.step(&head(), [None, None], &t);
        assert_eq!(out.button, Some(false));
        assert!(out.gone);
        // And reset with a press held says so.
        i.step(&head(), [None, Some(hand(at, middle, false, false))], &t);
        i.step(&head(), [None, Some(hand(at, middle, true, false))], &t);
        assert!(i.reset());
        assert!(!i.reset());
    }

    #[test]
    fn the_nearer_of_two_hands_on_the_panel_keeps_the_pointer_until_the_other_presses() {
        let panel = before_eyes(&head(), 0.7, 0.1);
        let t = targets(panel);
        let mut i = Interaction::default();
        let middle = position(&panel);
        let left_spot = space::apply(&panel, [-0.2, 0.1, 0.0]);
        let right_spot = space::apply(&panel, [0.2, -0.1, 0.0]);
        let (l, r) = ([-0.2, 1.2, -0.1], [0.2, 1.2, -0.1]);
        // The left hand points first: it has the pointer; the right one joining does not take it.
        let a = i.step(&head(), [Some(hand(l, left_spot, false, false)), None], &t).pointer.unwrap();
        let b = i.step(&head(), [Some(hand(l, left_spot, false, false)), Some(hand(r, right_spot, false, false))], &t);
        assert_eq!(b.pointer, Some(a));
        assert!(b.lasers[0].is_some() && b.lasers[1].is_some(), "both lasers show");
        // The right one pulls its trigger: it takes the pointer and presses where it points.
        let c = i.step(&head(), [Some(hand(l, left_spot, false, false)), Some(hand(r, right_spot, true, false))], &t);
        assert_eq!(c.button, Some(true));
        assert!(c.pointer.unwrap()[0] > 0.6, "{c:?}");
        // The left one pulls meanwhile: nothing (one press at a time).
        let d = i.step(&head(), [Some(hand(l, middle, true, false)), Some(hand(r, right_spot, true, false))], &t);
        assert!(d.button.is_none() && d.pointer.unwrap()[0] > 0.6);
    }

    #[test]
    fn the_handle_takes_the_panel_which_rides_the_hand_and_stays() {
        let panel = before_eyes(&head(), 0.7, 0.1);
        let t = targets(panel);
        let mut i = Interaction::default();
        let handle = t.handle.unwrap().1;
        // A hand at the handle (5 cm off it) grips: taken, its laser hidden.
        let at = space::add(handle, [0.0, -0.05, 0.0]);
        i.step(&head(), [Some(hand(at, [0.0, 0.0, -3.0], false, false)), None], &t);
        let out = i.step(&head(), [Some(hand(at, [0.0, 0.0, -3.0], false, true)), None], &t);
        assert_eq!(out.holding, Some(0));
        assert!(out.lasers[0].is_none());
        assert!(space::near(&out.moved.unwrap(), &panel, 1e-4, 1e-5), "taken where it is");
        // The hand moves 20 cm left and 10 cm up: the panel goes along.
        let moved_hand = space::add(at, [-0.2, 0.1, 0.0]);
        let out = i.step(&head(), [Some(hand(moved_hand, [0.0, 0.0, -3.0], false, true)), None], &t);
        let now = out.moved.unwrap();
        assert!(position(&now)[0] < -0.15 && position(&now)[1] > position(&panel)[1] + 0.08, "{:?}", position(&now));
        // Let go: released there (the caller keeps that place); nothing more moves it.
        let out = i.step(&head(), [Some(hand(moved_hand, [0.0, 0.0, -3.0], false, false)), None], &t);
        assert!(out.released && out.moved.is_none() && out.holding.is_none());
        // Far from the handle and pointing elsewhere, a grip takes nothing.
        let far = [0.6, 1.0, 0.3];
        i.step(&head(), [Some(hand(far, [3.0, 1.0, 0.0], false, false)), None], &t);
        assert!(i.step(&head(), [Some(hand(far, [3.0, 1.0, 0.0], false, true)), None], &t).holding.is_none());
    }

    #[test]
    fn a_grip_on_the_panel_from_afar_and_a_trigger_on_the_handle_take_it_too() {
        let panel = before_eyes(&head(), 0.7, 0.1);
        let t = targets(panel);
        let at = [0.3, 1.1, 0.1];
        // Grip with the laser on the panel.
        let mut i = Interaction::default();
        i.step(&head(), [None, Some(hand(at, position(&panel), false, false))], &t);
        assert_eq!(i.step(&head(), [None, Some(hand(at, position(&panel), false, true))], &t).holding, Some(1));
        // The trigger on the handle: held while the trigger is.
        let mut i = Interaction::default();
        let handle = t.handle.unwrap().1;
        i.step(&head(), [None, Some(hand(at, handle, false, false))], &t);
        let out = i.step(&head(), [None, Some(hand(at, handle, true, false))], &t);
        assert_eq!(out.holding, Some(1));
        assert!(out.bar_press.is_none() && out.button.is_none());
        assert_eq!(i.step(&head(), [None, Some(hand(at, handle, true, false))], &t).holding, Some(1));
        assert!(i.step(&head(), [None, Some(hand(at, handle, false, false))], &t).released);
        // The trigger on the bar beside the handle: a button.
        let mut i = Interaction::default();
        let bar = t.bar.unwrap();
        let smaller = space::apply(&bar.transform, [-0.13, 0.0, 0.0]);
        i.step(&head(), [None, Some(hand(at, smaller, false, false))], &t);
        let out = i.step(&head(), [None, Some(hand(at, smaller, true, false))], &t);
        assert!(out.bar_press.unwrap()[0] < 0.2 && out.holding.is_none());
    }

    #[test]
    fn the_other_hand_presses_the_watch_by_laser_or_fingertip() {
        let watch = Quad { transform: facing([-0.2, 1.2, -0.3], [0.0, 1.6, 0.0], [0.0, 1.0, 0.0]).unwrap(), width: 0.045, height: 0.045 };
        let t = Targets { watch: Some((0, watch)), ..Default::default() };
        let mut i = Interaction::default();
        let centre = position(&watch.transform);
        // The watch's own hand pointing at it: nothing.
        let own = hand([-0.25, 1.1, -0.1], centre, false, false);
        assert!(i.step(&head(), [Some(own), None], &t).lasers[0].is_none());
        // The other hand's laser on it: hovered; one pull: pressed, once.
        let other = |trigger| hand([0.15, 1.25, -0.15], centre, trigger, false);
        let out = i.step(&head(), [Some(own), Some(other(false))], &t);
        assert!(out.watch_hover && !out.watch_press && matches!(out.lasers[1], Some((Target::Watch, _))));
        assert!(i.step(&head(), [Some(own), Some(other(true))], &t).watch_press);
        assert!(!i.step(&head(), [Some(own), Some(other(true))], &t).watch_press, "held: once");
        // The fingertip on it (2 cm off its face, pointing anywhere): on it too.
        let mut i = Interaction::default();
        let tip_at = space::apply(&watch.transform, [0.0, 0.0, 0.02]);
        let poke = |trigger| hand(tip_at, [5.0, 0.0, 0.0], trigger, false);
        assert!(i.step(&head(), [None, Some(poke(false))], &t).watch_hover);
        assert!(i.step(&head(), [None, Some(poke(true))], &t).watch_press);
        // Pointing beside it, 3 cm past its edge: missed.
        let mut i = Interaction::default();
        let beside = space::apply(&watch.transform, [0.055, 0.0, 0.0]);
        let miss = |trigger| hand([0.15, 1.25, -0.15], beside, trigger, false);
        i.step(&head(), [None, Some(miss(false))], &t);
        let out = i.step(&head(), [None, Some(miss(true))], &t);
        assert!(!out.watch_hover && !out.watch_press);
    }
}
