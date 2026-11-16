//! Who points at what this turn: each hand's laser on the floating panel, N4N0's menu (and what hangs under it), the
//! panel's bar or the watch; each surface's pointer and its press; a grab and where the held panel or menu goes. Poses
//! and buttons in, what to do out, so that it is tested without SteamVR.
//!
//! Only a laser on a surface moves its pointer: a hand pointing elsewhere (at VRChat) presses nothing here, and the
//! trigger goes to VRChat as always. A press keeps its hand's laser on its surface's plane until the trigger is let go
//! (a slider dragged past the panel's edge, the Talk button held, the music's timeline), and holds the pointer where it
//! pressed until the laser has gone [`STEADY_M`] from there: pulling a trigger shakes the hand. The panel and the menu
//! are grabbed as N4N0's screen is: by a grip with the hand at them (the panel's handle within [`HANDLE_REACH_M`], or
//! their face within [`GRAB_REACH_M`]), from afar with a grip while the laser is on them (or on what hangs under them),
//! or with the trigger on the panel's handle; held, it rides the hand from where it was taken, turned towards the eyes
//! ([`space::carry`]); let go, it stays.

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

/// The surfaces a laser points and presses on: the floating panel, N4N0's menu, and what hangs under the menu (the
/// 3D worlds' pill or bar).
pub const SURFACES: usize = 4;
pub const PANEL: usize = 0;
pub const MENU: usize = 1;
/// What a grip moves: the panel or the menu (a surface's [`Surface::carries`]).
pub const CARRIED: usize = 2;

/// A surface to point at: where it is, and what a grip on it moves ([`PANEL`] or [`MENU`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub quad: Quad,
    pub carries: usize,
}

/// What the lasers can point at this turn.
#[derive(Clone, Copy, Debug, Default)]
pub struct Targets {
    pub surfaces: [Option<Surface>; SURFACES],
    /// The panel's bar (its buttons; a grip on it moves the panel).
    pub bar: Option<Quad>,
    /// The bar's handle: its part of the bar (u0, v0, u1, v1) and its middle in the play space.
    pub handle: Option<([f32; 4], V3)>,
    /// The watch, and the hand it is on (the other one presses it).
    pub watch: Option<(usize, Quad)>,
    /// The menu mismatch strip, and the hand it is on (the other one presses it).
    pub strip: Option<(usize, Quad)>,
}

impl Targets {
    /// Where `carried` (the panel or the menu) is: its own surface.
    fn carried(&self, carried: usize) -> Option<Quad> {
        let own = if carried == PANEL { PANEL } else { MENU };
        self.surfaces[own].map(|s| s.quad)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Surface(usize),
    Bar,
    Watch,
    Strip,
}

/// A hand this near the handle takes it with its grip, as N4N0's screen's handle is taken.
pub const HANDLE_REACH_M: f32 = 0.12;
/// A hand this near a surface (its face, or past its edge) takes what it moves with its grip.
pub const GRAB_REACH_M: f32 = 0.08;
/// A fingertip this near the watch is on it.
pub const WATCH_TOUCH_M: f32 = 0.035;
/// The watch is round and small: a laser this far past its edge is still on it.
const WATCH_MARGIN_M: f32 = 0.012;
/// A press holds the pointer where it pressed until the laser has gone this far from there on the surface.
pub const STEADY_M: f32 = 0.008;

/// The panel or the menu held: by which hand, which, taken with its trigger (on the handle) or its grip, the carry
/// frame then and its place then.
#[derive(Clone, Copy, Debug)]
struct Hold {
    hand: usize,
    carried: usize,
    by_trigger: bool,
    carry: Matrix34,
    taken: Matrix34,
}

/// A surface's button held: by which hand, on which, where it pressed, and whether the pointer still stays there.
#[derive(Clone, Copy, Debug)]
struct Press {
    hand: usize,
    surface: usize,
    uv: [f32; 2],
    point: V3,
    steady: bool,
}

#[derive(Debug)]
pub struct Interaction {
    /// Each hand's trigger and grip as they were last turn (taken as held when unknown: a pull is seen go down).
    trigger: [bool; 2],
    grab: [bool; 2],
    /// The hand each surface's pointer follows.
    active: [Option<usize>; SURFACES],
    press: Option<Press>,
    hold: Option<Hold>,
    /// Each surface's pointer was on it last turn.
    pointing: [bool; SURFACES],
}

/// What this turn does.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Outcome {
    /// Each hand's laser: what it is on and where (none: hidden).
    pub lasers: [Option<(Target, Hit)>; 2],
    /// Each surface's pointer (u, v), none while no laser is on it.
    pub pointer: [Option<[f32; 2]>; SURFACES],
    /// A surface's button went down (`true`) or up (`false`): at its pointer, or where it last was.
    pub button: [Option<bool>; SURFACES],
    /// The pointer left the surface.
    pub gone: [bool; SURFACES],
    /// A laser on the bar (u, v), and a trigger pulled there.
    pub bar_hover: Option<[f32; 2]>,
    pub bar_press: Option<[f32; 2]>,
    /// A laser or a fingertip on the watch, and a trigger pulled there.
    pub watch_hover: bool,
    pub watch_press: bool,
    /// A laser or a fingertip on the mismatch strip (u, v), and a trigger pulled there.
    pub strip_hover: Option<[f32; 2]>,
    pub strip_press: Option<[f32; 2]>,
    /// Where the held panel or menu is now; it was let go this turn (where it last was is to keep).
    pub moved: [Option<Matrix34>; CARRIED],
    pub released: [bool; CARRIED],
    /// The hand holding the panel or the menu.
    pub holding: Option<usize>,
}

impl Default for Interaction {
    fn default() -> Self {
        Self { trigger: [true; 2], grab: [true; 2], active: [None; SURFACES], press: None, hold: None, pointing: [false; SURFACES] }
    }
}

impl Interaction {
    /// The hand holding the panel or the menu.
    pub fn holding(&self) -> Option<usize> {
        self.hold.map(|h| h.hand)
    }

    /// Everything let go (the dashboard took the controllers, nothing to point at). Returns the surface whose button
    /// was held (its end is the caller's to send). The buttons are then taken as held: one held meanwhile presses
    /// nothing when there is something to point at again, only a new pull does.
    pub fn reset(&mut self) -> Option<usize> {
        let pressed = self.press.map(|p| p.surface);
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

        // The held panel or menu follows its hand, until that hand lets go (or is lost, or it closed).
        if let Some(hold) = self.hold {
            let holds = if hold.by_trigger { trigger[hold.hand] } else { grab[hold.hand] };
            match hands[hold.hand].filter(|_| holds && targets.carried(hold.carried).is_some()) {
                Some(h) => out.moved[hold.carried] = Some(space::carried(&space::carry(space::position(&h.grip), head), &hold.carry, &hold.taken)),
                None => {
                    self.hold = None;
                    out.released[hold.carried] = true;
                }
            }
        }

        // Where each laser is: the nearest of what it meets; a press keeps its laser on its surface's plane.
        for (i, hand) in hands.iter().enumerate() {
            let Some(h) = hand else { continue };
            if self.hold.is_some_and(|hold| hold.hand == i) {
                continue;
            }
            let (o, d) = space::ray(&h.tip);
            if let Some(p) = self.press.filter(|p| p.hand == i) {
                out.lasers[i] = targets.surfaces[p.surface].and_then(|s| s.quad.plane_hit(o, d)).map(|hit| (Target::Surface(p.surface), hit));
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
            for (s, surface) in targets.surfaces.iter().enumerate() {
                consider(Target::Surface(s), surface.and_then(|q| q.quad.hit(o, d, 0.0)));
            }
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
            // The mismatch strip as the watch: its fingertip near it wins, else a laser on it when nearer than the rest.
            if let Some((_, strip)) = targets.strip.filter(|(on, _)| *on != i) {
                let (at, gap) = strip.nearest(o);
                if gap < super::strip::TOUCH_M {
                    out.lasers[i] = Some((Target::Strip, at));
                } else if let Some(hit) = strip.hit(o, d, 0.0).filter(|h| out.lasers[i].is_none_or(|(_, b)| h.distance < b.distance)) {
                    out.lasers[i] = Some((Target::Strip, hit));
                }
            }
        }

        // A grip with the laser on a surface (or the panel's bar), at the panel's handle or at a surface takes what it
        // moves (from the other hand, too).
        for (i, hand) in hands.iter().enumerate() {
            let Some(h) = *hand else { continue };
            if !gripped(i) || self.press.is_some_and(|p| p.hand == i) {
                continue;
            }
            let hand = space::position(&h.grip);
            let carried = match out.lasers[i] {
                Some((Target::Surface(s), _)) => targets.surfaces[s].map(|s| s.carries),
                Some((Target::Bar, _)) => Some(PANEL),
                _ => None,
            }
            .or_else(|| targets.handle.filter(|(_, at)| space::length(space::sub(hand, *at)) < HANDLE_REACH_M).map(|_| PANEL))
            .or_else(|| targets.surfaces.iter().flatten().find(|s| s.quad.nearest(hand).1 < GRAB_REACH_M).map(|s| s.carries));
            let at = carried.and_then(|c| Some((c, out.moved[c].or(targets.carried(c).map(|q| q.transform))?)));
            if let Some((carried, at)) = at {
                self.take(&mut out, i, carried, false, &h, head, at);
            }
        }

        // A trigger pulled: on the watch, a bar's button (the handle takes the panel), a surface's button.
        for (i, hand) in hands.iter().enumerate() {
            let Some(h) = *hand else { continue };
            if !pulled(i) || self.hold.is_some_and(|hold| hold.hand == i) {
                continue;
            }
            match out.lasers[i] {
                Some((Target::Watch, _)) => out.watch_press = true,
                Some((Target::Strip, hit)) => out.strip_press = Some(hit.uv),
                Some((Target::Bar, hit)) => match (targets.handle, targets.carried(PANEL)) {
                    (Some((r, _)), Some(panel)) if (r[0]..=r[2]).contains(&hit.uv[0]) && (r[1]..=r[3]).contains(&hit.uv[1]) => {
                        let at = out.moved[PANEL].unwrap_or(panel.transform);
                        self.take(&mut out, i, PANEL, true, &h, head, at);
                    }
                    _ => out.bar_press = Some(hit.uv),
                },
                Some((Target::Surface(s), hit)) if self.press.is_none() => {
                    self.active[s] = Some(i);
                    self.press = Some(Press { hand: i, surface: s, uv: hit.uv, point: hit.point, steady: true });
                    out.button[s] = Some(true);
                }
                _ => {}
            }
        }

        // Each surface's pointer: the pressing hand's, else the hand it followed while that one stays on it, else
        // whichever is on it (the right first).
        let on = |i: usize, s: usize| matches!(out.lasers[i], Some((Target::Surface(t), _)) if t == s);
        for s in 0..SURFACES {
            self.active[s] = match self.press.filter(|p| p.surface == s) {
                Some(p) => Some(p.hand),
                None => self.active[s].filter(|&i| on(i, s)).or_else(|| [1, 0].into_iter().find(|&i| on(i, s))),
            };
            if let Some((_, hit)) = self.active[s].and_then(|i| out.lasers[i]) {
                out.pointer[s] = Some(hit.uv);
                if let Some(p) = self.press.as_mut().filter(|p| p.surface == s && p.steady) {
                    if space::length(space::sub(hit.point, p.point)) < STEADY_M {
                        out.pointer[s] = Some(p.uv);
                    } else {
                        p.steady = false;
                    }
                }
            }
        }
        // The press ends as its trigger is let go (or its hand is lost, or its surface went).
        if let Some(p) = self.press {
            if !trigger[p.hand] || targets.surfaces[p.surface].is_none() {
                self.press = None;
                out.button[p.surface] = Some(false);
            }
        }
        for s in 0..SURFACES {
            out.gone[s] = self.pointing[s] && out.pointer[s].is_none();
            self.pointing[s] = out.pointer[s].is_some();
        }

        let first = self.active[PANEL];
        let hover = |t: Target| first.into_iter().chain([1, 0]).find_map(|i| out.lasers[i].filter(|(on, _)| *on == t)).map(|(_, hit)| hit.uv);
        out.bar_hover = hover(Target::Bar);
        out.watch_hover = hover(Target::Watch).is_some();
        out.strip_hover = hover(Target::Strip);
        out.holding = self.holding();
        self.trigger = trigger;
        self.grab = grab;
        out
    }

    /// Hand `i` takes the panel or the menu (`carried`) where it is (`at`), with its trigger or its grip.
    #[allow(clippy::too_many_arguments)]
    fn take(&mut self, out: &mut Outcome, i: usize, carried: usize, by_trigger: bool, h: &HandIn, head: &Matrix34, at: Matrix34) {
        if let Some(was) = self.hold.filter(|hold| hold.carried != carried) {
            // The other hand held the other one: it stays where it was.
            out.released[was.carried] = true;
        }
        self.hold = Some(Hold { hand: i, carried, by_trigger, carry: space::carry(space::position(&h.grip), head), taken: at });
        out.lasers[i] = None;
        out.moved[carried] = Some(at);
        for active in &mut self.active {
            if *active == Some(i) {
                *active = None;
            }
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
            surfaces: [Some(Surface { quad: Quad { transform: panel, width: PANEL_W, height: PANEL_H }, carries: PANEL }), None, None, None],
            bar: Some(Quad { transform: bar, width: 0.3, height: 0.045 }),
            handle: Some(([0.3, 0.0, 0.7, 1.0], position(&bar))),
            watch: None,
            strip: None,
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
        let uv = out.pointer[PANEL].unwrap();
        assert!((uv[0] - 0.5).abs() < 0.01 && (uv[1] - 0.5).abs() < 0.01, "{uv:?}");
        assert!(matches!(out.lasers[1], Some((Target::Surface(PANEL), _))) && out.lasers[0].is_none() && out.button[PANEL].is_none());
        // The trigger pulled there: down.
        let out = i.step(&head(), [None, Some(hand(right_hand, middle, true, false))], &t);
        assert_eq!(out.button[PANEL], Some(true));
        // Let go: up, a click.
        let out = i.step(&head(), [None, Some(hand(right_hand, middle, false, false))], &t);
        assert_eq!(out.button[PANEL], Some(false));
        // Pointing at VRChat, away from the panel: the trigger presses nothing here and no laser shows.
        let away = [3.0, 1.0, 1.0];
        let out = i.step(&head(), [None, Some(hand(right_hand, away, false, false))], &t);
        assert!(out.gone[PANEL] && out.pointer[PANEL].is_none() && out.lasers[1].is_none());
        for trigger in [true, false, true] {
            let out = i.step(&head(), [None, Some(hand(right_hand, away, trigger, false))], &t);
            assert!(out.button[PANEL].is_none() && out.pointer[PANEL].is_none() && !out.gone[PANEL]);
        }
        // Coming back with the trigger still held clicks nothing (only a pull on it does).
        let out = i.step(&head(), [None, Some(hand(right_hand, middle, true, false))], &t);
        assert!(out.pointer[PANEL].is_some() && out.button[PANEL].is_none());
    }

    #[test]
    fn a_press_holds_still_then_drags_past_the_edge() {
        let panel = before_eyes(&head(), 0.7, 0.1);
        let t = targets(panel);
        let mut i = Interaction::default();
        let at = [0.1, 1.2, -0.1];
        let middle = position(&panel);
        i.step(&head(), [None, Some(hand(at, middle, false, false))], &t);
        let down = i.step(&head(), [None, Some(hand(at, middle, true, false))], &t).pointer[PANEL].unwrap();
        // The pull shakes the laser 4 mm: the pointer stays where it pressed.
        let shaken = i.step(&head(), [None, Some(hand(at, space::add(middle, [0.004, 0.0, 0.0]), true, false))], &t);
        assert_eq!(shaken.pointer[PANEL], Some(down));
        // Dragged on, past the panel's right edge: the pointer follows, past 1.
        let past = i.step(&head(), [None, Some(hand(at, space::add(middle, [0.6, 0.0, 0.0]), true, false))], &t);
        assert!(past.pointer[PANEL].unwrap()[0] > 1.0 && matches!(past.lasers[1], Some((Target::Surface(PANEL), _))), "{past:?}");
        // Back near where it pressed: it follows now (no longer held still).
        let back = i.step(&head(), [None, Some(hand(at, space::add(middle, [0.004, 0.0, 0.0]), true, false))], &t);
        assert_ne!(back.pointer[PANEL], Some(down));
        // Let go out there: up, then the pointer is gone with the laser.
        i.step(&head(), [None, Some(hand(at, space::add(middle, [0.6, 0.0, 0.0]), true, false))], &t);
        let up = i.step(&head(), [None, Some(hand(at, space::add(middle, [0.6, 0.0, 0.0]), false, false))], &t);
        assert_eq!(up.button[PANEL], Some(false));
        let after = i.step(&head(), [None, Some(hand(at, space::add(middle, [0.6, 0.0, 0.0]), false, false))], &t);
        assert!(after.gone[PANEL] && after.lasers[1].is_none());
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
        assert!(out.button[PANEL].is_none() && out.holding.is_none() && out.pointer[PANEL].is_some());
        i.step(&head(), [None, Some(hand(at, middle, true, true))], &t);
        // After a reset (SteamVR's dashboard came and went) as well.
        i.step(&head(), [None, Some(hand(at, middle, false, false))], &t);
        i.reset();
        assert!(i.step(&head(), [None, Some(hand(at, middle, true, false))], &t).button[PANEL].is_none());
        i.step(&head(), [None, Some(hand(at, middle, false, false))], &t);
        assert_eq!(i.step(&head(), [None, Some(hand(at, middle, true, false))], &t).button[PANEL], Some(true), "a new pull");
    }

    #[test]
    fn a_lost_hand_lets_go_of_its_press() {
        let panel = before_eyes(&head(), 0.7, 0.1);
        let t = targets(panel);
        let mut i = Interaction::default();
        let (at, middle) = ([0.1, 1.2, -0.1], position(&panel));
        i.step(&head(), [None, Some(hand(at, middle, false, false))], &t);
        assert_eq!(i.step(&head(), [None, Some(hand(at, middle, true, false))], &t).button[PANEL], Some(true));
        let out = i.step(&head(), [None, None], &t);
        assert_eq!(out.button[PANEL], Some(false));
        assert!(out.gone[PANEL]);
        // And reset with a press held says so.
        i.step(&head(), [None, Some(hand(at, middle, false, false))], &t);
        i.step(&head(), [None, Some(hand(at, middle, true, false))], &t);
        assert_eq!(i.reset(), Some(PANEL));
        assert_eq!(i.reset(), None);
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
        let a = i.step(&head(), [Some(hand(l, left_spot, false, false)), None], &t).pointer[PANEL].unwrap();
        let b = i.step(&head(), [Some(hand(l, left_spot, false, false)), Some(hand(r, right_spot, false, false))], &t);
        assert_eq!(b.pointer[PANEL], Some(a));
        assert!(b.lasers[0].is_some() && b.lasers[1].is_some(), "both lasers show");
        // The right one pulls its trigger: it takes the pointer and presses where it points.
        let c = i.step(&head(), [Some(hand(l, left_spot, false, false)), Some(hand(r, right_spot, true, false))], &t);
        assert_eq!(c.button[PANEL], Some(true));
        assert!(c.pointer[PANEL].unwrap()[0] > 0.6, "{c:?}");
        // The left one pulls meanwhile: nothing (one press at a time).
        let d = i.step(&head(), [Some(hand(l, middle, true, false)), Some(hand(r, right_spot, true, false))], &t);
        assert!(d.button[PANEL].is_none() && d.pointer[PANEL].unwrap()[0] > 0.6);
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
        assert!(space::near(&out.moved[PANEL].unwrap(), &panel, 1e-4, 1e-5), "taken where it is");
        // The hand moves 20 cm left and 10 cm up: the panel goes along.
        let moved_hand = space::add(at, [-0.2, 0.1, 0.0]);
        let out = i.step(&head(), [Some(hand(moved_hand, [0.0, 0.0, -3.0], false, true)), None], &t);
        let now = out.moved[PANEL].unwrap();
        assert!(position(&now)[0] < -0.15 && position(&now)[1] > position(&panel)[1] + 0.08, "{:?}", position(&now));
        // Let go: released there (the caller keeps that place); nothing more moves it.
        let out = i.step(&head(), [Some(hand(moved_hand, [0.0, 0.0, -3.0], false, false)), None], &t);
        assert!(out.released[PANEL] && out.moved[PANEL].is_none() && out.holding.is_none());
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
        assert!(out.bar_press.is_none() && out.button[PANEL].is_none());
        assert_eq!(i.step(&head(), [None, Some(hand(at, handle, true, false))], &t).holding, Some(1));
        assert!(i.step(&head(), [None, Some(hand(at, handle, false, false))], &t).released[PANEL]);
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

    /// The menu mismatch strip on the left wrist, over the watch: the right hand presses one of its rows by laser or fingertip, the left
    /// hand's own laser does nothing on it, and the press says which row (its v) whatever N4N0's menu does.
    #[test]
    fn the_other_hand_presses_a_row_of_the_mismatch_strip() {
        use super::super::strip;
        let watch = Quad { transform: facing([-0.2, 1.2, -0.3], [0.0, 1.6, 0.0], [0.0, 1.0, 0.0]).unwrap(), width: 0.045, height: 0.045 };
        // Just over the watch as the eyes see it, facing them (`space::over_watch`).
        let rise = 0.045 / 2.0 + strip::GAP_M + strip::height_m() / 2.0;
        let band = Quad { transform: space::offset(&watch.transform, [0.0, rise, strip::LIFT_M]), width: strip::WIDTH_M, height: strip::height_m() };
        let t = Targets { watch: Some((0, watch)), strip: Some((0, band)), ..Default::default() };
        // The last row (Quick menu should be on), by the right hand's laser.
        let row = space::apply(&band.transform, [0.0, -band.height / 3.0, 0.0]);
        let own = hand([-0.25, 1.1, -0.1], row, false, false);
        let mut i = Interaction::default();
        assert!(i.step(&head(), [Some(own), None], &t).strip_hover.is_none(), "its own hand: nothing");
        let other = |trigger| hand([0.15, 1.25, -0.15], row, trigger, false);
        let out = i.step(&head(), [Some(own), Some(other(false))], &t);
        assert_eq!(out.strip_hover.and_then(|uv| strip::row_at(uv[1])), Some(crate::mismatch::Kind::QuickMenuShouldBeOn));
        let out = i.step(&head(), [Some(own), Some(other(true))], &t);
        assert_eq!(out.strip_press.and_then(|uv| strip::row_at(uv[1])), Some(crate::mismatch::Kind::QuickMenuShouldBeOn));
        assert!(!out.watch_press);
        assert!(i.step(&head(), [Some(own), Some(other(true))], &t).strip_press.is_none(), "held: once");
        // The fingertip on the first row, 2 cm off its face, pointing anywhere.
        let mut i = Interaction::default();
        let tip_at = space::apply(&band.transform, [0.0, band.height / 3.0, 0.02]);
        let poke = |trigger| hand(tip_at, [5.0, 0.0, 0.0], trigger, false);
        i.step(&head(), [None, Some(poke(false))], &t);
        let out = i.step(&head(), [None, Some(poke(true))], &t);
        assert_eq!(out.strip_press.and_then(|uv| strip::row_at(uv[1])), Some(crate::mismatch::Kind::MenuShouldBeOff));
    }

    /// The panel to the left and N4N0's menu ahead, with the 3D worlds' pill under the menu.
    fn with_menu() -> (Targets, Matrix34, Matrix34) {
        let panel = space::facing([-0.9, 1.4, -0.6], [0.0, 1.6, 0.0], [0.0, 1.0, 0.0]).unwrap();
        let menu = before_eyes(&head(), 0.8, 0.24);
        let mut t = targets(panel);
        t.surfaces[MENU] = Some(Surface { quad: Quad { transform: menu, width: 0.9, height: 0.5 }, carries: MENU });
        let pill = space::offset(&menu, [0.0, -0.3, 0.0]);
        t.surfaces[2] = Some(Surface { quad: Quad { transform: pill, width: 0.2, height: 0.05 }, carries: MENU });
        (t, panel, menu)
    }

    #[test]
    fn the_menu_points_and_presses_beside_the_panel() {
        let (t, panel, menu) = with_menu();
        let mut i = Interaction::default();
        let at = [0.15, 1.2, -0.1];
        let middle = position(&menu);
        let out = i.step(&head(), [None, Some(hand(at, middle, false, false))], &t);
        assert!(matches!(out.lasers[1], Some((Target::Surface(MENU), _))) && out.pointer[PANEL].is_none());
        let uv = out.pointer[MENU].unwrap();
        assert!((uv[0] - 0.5).abs() < 0.01 && (uv[1] - 0.5).abs() < 0.01, "{uv:?}");
        assert_eq!(i.step(&head(), [None, Some(hand(at, middle, true, false))], &t).button, [None, Some(true), None, None]);
        // Dragged onto the panel while pressed: the press stays on the menu's plane.
        let dragged = i.step(&head(), [None, Some(hand(at, position(&panel), true, false))], &t);
        assert!(matches!(dragged.lasers[1], Some((Target::Surface(MENU), _))) && dragged.pointer[PANEL].is_none());
        let up = i.step(&head(), [None, Some(hand(at, position(&panel), false, false))], &t);
        assert_eq!(up.button[MENU], Some(false));
        // Let go, the laser is on the panel now: the menu's pointer leaves, the panel's comes.
        let on_panel = i.step(&head(), [None, Some(hand(at, position(&panel), false, false))], &t);
        assert!(on_panel.gone[MENU] && on_panel.pointer[PANEL].is_some() && on_panel.pointer[MENU].is_none());
        // The pill under the menu is a surface of its own.
        let pill = position(&t.surfaces[2].unwrap().quad.transform);
        let out = i.step(&head(), [None, Some(hand(at, pill, false, false))], &t);
        assert!(out.pointer[2].is_some() && out.pointer[MENU].is_none());
        assert_eq!(i.step(&head(), [None, Some(hand(at, pill, true, false))], &t).button[2], Some(true));
    }

    #[test]
    fn the_menu_is_grabbed_from_afar_by_its_pill_or_at_hand_and_stays() {
        let (t, panel, menu) = with_menu();
        let at = [0.15, 1.2, -0.1];
        // A grip with the laser on it: the menu rides the hand, the panel stays.
        let mut i = Interaction::default();
        i.step(&head(), [None, Some(hand(at, position(&menu), false, false))], &t);
        let out = i.step(&head(), [None, Some(hand(at, position(&menu), false, true))], &t);
        assert_eq!((out.holding, out.moved[PANEL]), (Some(1), None));
        assert!(space::near(&out.moved[MENU].unwrap(), &menu, 1e-4, 1e-5), "taken where it is");
        let moved = i.step(&head(), [None, Some(hand(space::add(at, [0.3, 0.0, 0.0]), position(&menu), false, true))], &t).moved[MENU].unwrap();
        assert!(position(&moved)[0] > position(&menu)[0] + 0.2, "{:?}", position(&moved));
        assert!(space::dot(space::axis(&moved, 2), space::sub(position(&head()), position(&moved))) > 0.0, "turned toward the eyes");
        let out = i.step(&head(), [None, Some(hand(space::add(at, [0.3, 0.0, 0.0]), position(&menu), false, false))], &t);
        assert!(out.released[MENU] && !out.released[PANEL] && out.moved == [None, None]);
        // A grip with the laser on the pill takes the menu too, where the menu is.
        let mut i = Interaction::default();
        let pill = position(&t.surfaces[2].unwrap().quad.transform);
        i.step(&head(), [None, Some(hand(at, pill, false, false))], &t);
        assert!(space::near(&i.step(&head(), [None, Some(hand(at, pill, false, true))], &t).moved[MENU].unwrap(), &menu, 1e-4, 1e-5));
        // A hand at its edge (5 cm off it), pointing away, takes it with a grip; 20 cm away it takes nothing.
        let mut i = Interaction::default();
        let near = space::apply(&menu, [0.47, 0.0, 0.03]);
        i.step(&head(), [Some(hand(near, [0.0, 5.0, 0.0], false, false)), None], &t);
        assert_eq!(i.step(&head(), [Some(hand(near, [0.0, 5.0, 0.0], false, true)), None], &t).holding, Some(0));
        let mut i = Interaction::default();
        let far = space::apply(&menu, [0.0, 0.0, 0.2]);
        i.step(&head(), [Some(hand(far, [0.0, 5.0, 0.0], false, false)), None], &t);
        assert!(i.step(&head(), [Some(hand(far, [0.0, 5.0, 0.0], false, true)), None], &t).holding.is_none());
        // The menu gone while held (closed): let go.
        let mut i = Interaction::default();
        i.step(&head(), [None, Some(hand(at, position(&menu), false, false))], &t);
        i.step(&head(), [None, Some(hand(at, position(&menu), false, true))], &t);
        let mut closed = t;
        closed.surfaces[MENU] = None;
        closed.surfaces[2] = None;
        assert!(i.step(&head(), [None, Some(hand(at, position(&menu), false, true))], &closed).released[MENU]);
        let _ = panel;
    }
}
