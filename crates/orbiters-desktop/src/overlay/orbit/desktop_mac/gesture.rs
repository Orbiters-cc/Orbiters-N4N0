//! Picking a cell of the ring around VRChat's wheel with no key held, on a Mac.
//!
//! VRChat holds the mouse while its wheel is open (on Windows too) and picks a slice by where the mouse is pushed, stopping its own pointer at the rim. The ring lies outside that
//! rim, so it is reached the way its slices are: by pushing on. The moves are those the engine says it sent the game ([`super::channel`]): the game gets every one of them, and
//! nothing of this is sent to it.
//!
//! **What ordinary use of the wheel must never do is pick one of ours.** The rim is a wall here:
//! - a flick stops at it, however far: the stroke that brought the mouse to the rim never goes past it unless it goes on pushing, slowly, for [`SLOW`] and [`WALL_SLOW`];
//! - the next push outward ([`WALL`], within [`STRAIGHT`] of straight out, with its stroke begun at the rim) goes through, and lights the cell there. Sliding along the rim from
//!   slice to slice is no push outward, wherever it ends;
//! - a click is VRChat's (it presses the slice lit there): it puts the mouse back inside the rim, and nothing is lit;
//! - coming back inside the rim puts it out.
//!
//! What is lit is opened by one notch of the scroll wheel (which VRChat's wheel has no use for), or by resting on it: the rest is shown as it goes, and any move starts it again.
//! Both and either can be chosen ([`WheelPick`]).

use egui::{vec2, Vec2};
use orbiters_core::settings::WheelPick;

/// A pause in the moves this long ends a stroke.
const STROKE_GAP: f64 = 0.12;
/// How far past the rim a stroke begun at it must push to go through it, in the wheel's radii.
pub const WALL: f32 = 0.6;
/// And one that brought the mouse to the rim, if it goes on for [`SLOW`] seconds more.
pub const WALL_SLOW: f32 = 1.5;
const SLOW: f64 = 0.45;
/// A stroke is a push outward when its first movement ([`FIRST`] radii of it) is within so many degrees of straight out, and stays one while the mouse slides no further than
/// that along the rim.
pub const STRAIGHT: f32 = 35.0;
const FIRST: f32 = 0.15;
/// Where the mouse is once through the wall: well in the ring (from 1.06 to 1.72).
const THROUGH: f32 = 1.32;
/// As far as it goes: still in the ring (a push goes on past what took it through, and must not leave what it lit); in a second ring that is out (to 2.5), from the next push on.
const IN_THE_RING: f32 = 1.6;
const FURTHEST: f32 = 2.42;
/// Nothing goes past the rim this long after the wheel opened, nor after a click.
const SETTLES: f64 = 0.25;
/// Less movement than this, in points, is a hand at rest.
const JITTER: f32 = 4.0;
/// The rest shows from here on.
const SHOWS_AFTER: f64 = 0.1;
/// A cell must be lit this long before a notch counts, with the scroll wheel still for [`SCROLL_STILL`] before it (a trackpad's momentum goes on by itself), and no button
/// pressed for [`SCROLL_SURE`] after it (two fingers laid for a right click scroll by nothing first).
const LIT_FIRST: f64 = 0.12;
const SCROLL_STILL: f64 = 0.3;
const SCROLL_SURE: f64 = 0.16;

/// A move the game was sent: points, y down, and when, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moved {
    pub by: Vec2,
    pub at: f64,
}

/// What is so this turn besides the moves.
#[derive(Clone, Copy, Debug)]
pub struct Now {
    pub at: f64,
    /// The scroll wheel turned since the last turn, whatever way; a button of the mouse is down.
    pub scrolled: bool,
    pub button: bool,
    /// What is under [`Gesture::at`] can be pressed: told after the moves of this turn ([`Gesture::moves`]).
    pub pressable: bool,
}

/// The stroke under way.
#[derive(Clone, Copy, Debug)]
struct Stroke {
    /// The way the mouse was pushed to as it began, in degrees, if it began at the rim.
    from: Option<f32>,
    /// Its movement so far, until it is told what kind it is; then whether it is a push outward.
    first: Vec2,
    outward: Option<bool>,
    last: f64,
    /// It is the one that went through the wall.
    through: bool,
}

#[derive(Clone, Copy, Debug, Default)]
enum Place {
    /// VRChat's: in the wheel, or against its rim.
    #[default]
    Inside,
    /// Through the wall: the ring's.
    Out,
}

#[derive(Clone, Debug, Default)]
pub struct Gesture {
    /// Where the mouse is, in radii from the wheel's middle (y down).
    place: Place,
    at: Vec2,
    stroke: Option<Stroke>,
    /// How far it pushed against the rim, since when, and where on it.
    push: f32,
    pushing: Option<(f64, f32)>,
    /// Nothing goes through before.
    holds_until: f64,
    /// When the mouse last went through, or moved enough to be no longer at rest.
    lit: f64,
    rest: f64,
    moved: Vec2,
    last_scroll: Option<f64>,
    scroll: Option<f64>,
    was_down: bool,
    /// It opened one: nothing more until it is back inside.
    done: bool,
    moves: u32,
}

/// Degrees between two ways, 0 to 180.
fn apart(a: f32, b: f32) -> f32 {
    ((a - b + 540.0).rem_euclid(360.0) - 180.0).abs()
}

fn way(v: Vec2) -> f32 {
    v.x.atan2(-v.y).to_degrees()
}

impl Gesture {
    /// The wheel opens: VRChat's pointer is in its middle, and so is this one.
    pub fn begin(&mut self, at: f64) {
        *self = Self { holds_until: at + SETTLES, rest: at, ..Self::default() };
    }

    /// The wheel closed, or something else has the mouse.
    pub fn end(&mut self) {
        *self = Self::default();
    }

    /// Where on the ring the mouse is, in radii from the wheel's middle; `None` while it is VRChat's.
    pub fn at(&self) -> Option<Vec2> {
        (matches!(self.place, Place::Out) && !self.done).then_some(self.at)
    }

    /// Moves followed since the wheel opened, for the log.
    pub fn counted(&self) -> u32 {
        self.moves
    }

    /// Back against the rim, where it was.
    fn inside(&mut self) {
        self.place = Place::Inside;
        self.at = if self.at.length() > 1.0 { self.at.normalized() } else { self.at };
        (self.push, self.pushing, self.scroll) = (0.0, None, None);
        if let Some(stroke) = self.stroke.as_mut() {
            // What is left of the stroke that came back in is not a push of its own.
            (stroke.from, stroke.outward) = (None, None);
        }
        self.done = false;
    }

    /// The moves since the last turn, with the wheel up for VRChat's `radius` in points. `may`: the ring shows (until it does, the mouse is only followed); `second`: so does a second
    /// ring past it.
    pub fn moves(&mut self, moves: &[Moved], radius: f32, may: bool, second: bool) {
        for m in moves {
            self.moves += 1;
            let by = m.by / radius.max(20.0);
            if self.stroke.is_none_or(|s| m.at - s.last > STROKE_GAP) {
                let at_rim = matches!(self.place, Place::Inside) && self.at.length() > 0.98;
                self.stroke = Some(Stroke { from: at_rim.then(|| way(self.at)), first: Vec2::ZERO, outward: None, last: m.at, through: false });
                // A push is one stroke's.
                (self.push, self.pushing) = (0.0, None);
            }
            let stroke = self.stroke.as_mut().expect("just made");
            stroke.last = m.at;
            if stroke.outward.is_none() {
                stroke.first += by;
                if stroke.first.length() >= FIRST {
                    stroke.outward = Some(stroke.from.is_some_and(|from| apart(way(stroke.first), from) <= STRAIGHT));
                }
            }
            match self.place {
                Place::Inside => self.against(by, m.at, may),
                Place::Out => {
                    let most = if second && !stroke.through { FURTHEST } else { IN_THE_RING.max(self.at.length().min(if second { FURTHEST } else { 0.0 })) };
                    self.at += by;
                    if self.at.length() > most {
                        self.at = self.at.normalized() * most;
                    }
                    if self.at.length() < 1.0 {
                        self.inside();
                    } else {
                        self.moved += m.by;
                        if self.moved.length() > JITTER {
                            (self.rest, self.moved) = (m.at, Vec2::ZERO);
                        }
                    }
                }
            }
        }
    }

    /// A move inside the rim: VRChat's, and what of it pushes against the rim.
    fn against(&mut self, by: Vec2, at: f64, may: bool) {
        let to = self.at + by;
        let length = to.length();
        if length <= 1.0 {
            self.at = to;
            if length < 0.9 {
                (self.push, self.pushing) = (0.0, None);
            }
            return;
        }
        self.at = to / length;
        let (here, over) = (way(self.at), length - 1.0);
        // Along the rim, it is another push.
        match self.pushing {
            Some((_, from)) if apart(from, here) <= STRAIGHT => {}
            _ => (self.push, self.pushing) = (0.0, Some((at, here))),
        }
        self.push += over;
        let since = self.pushing.map_or(at, |(since, _)| since);
        let outward = self.stroke.is_some_and(|s| s.outward == Some(true) && s.from.is_some_and(|from| apart(from, here) <= STRAIGHT));
        let through = outward && self.push >= WALL || at - since >= SLOW && self.push >= WALL_SLOW;
        if !may || at < self.holds_until {
            (self.push, self.pushing) = (0.0, None);
        } else if through {
            self.place = Place::Out;
            self.at = self.at.normalized() * THROUGH;
            if let Some(stroke) = self.stroke.as_mut() {
                stroke.through = true;
            }
            (self.lit, self.rest, self.moved, self.scroll) = (at, at, Vec2::ZERO, None);
            (self.push, self.pushing) = (0.0, None);
        }
    }

    /// The rest of the turn: whether what is lit is opened now, and how much of the rest it opens after has passed (0 to 1).
    pub fn turn(&mut self, now: &Now, pick: WheelPick, rest: f64) -> (bool, f32) {
        let scrolled = now.scrolled.then(|| {
            let still = self.last_scroll.is_none_or(|t| now.at - t >= SCROLL_STILL);
            self.last_scroll = Some(now.at);
            still
        });
        let pressed = now.button && !self.was_down;
        self.was_down = now.button;
        if pressed {
            // VRChat's.
            self.inside();
            self.holds_until = now.at + SETTLES;
        }
        if now.button {
            self.rest = now.at;
        }
        if self.at().is_none() || !now.pressable {
            self.scroll = None;
            self.rest = self.rest.max(now.at - SHOWS_AFTER);
            return (false, 0.0);
        }
        if scrolled == Some(true) && pick.scrolls() && now.at - self.lit >= LIT_FIRST {
            self.scroll.get_or_insert(now.at);
        }
        let rested = now.at - self.rest;
        let opens = self.scroll.is_some_and(|t| now.at - t >= SCROLL_SURE) || pick.rests() && rested >= rest;
        if opens {
            (self.done, self.scroll) = (true, None);
        }
        let shown = if pick.rests() && rested >= SHOWS_AFTER { ((rested / rest.max(0.01)) as f32).clamp(0.0, 1.0) } else { 0.0 };
        (opens, if opens { 1.0 } else { shown })
    }
}

/// Where the ring shows `at` (radii) around a wheel of `radius` pixels.
pub fn shown_at(at: Vec2, radius: f32) -> Vec2 {
    vec2(at.x * radius, at.y * radius)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This Mac's: 0.139 of 956 points.
    const R: f32 = 133.0;

    /// A person at the mouse: what they do, 8 ms at a time, and what comes of it.
    struct Hand {
        g: Gesture,
        t: f64,
        pick: WheelPick,
        rest: f64,
        opened: Vec<f64>,
        lit: bool,
        shown: f32,
        second: bool,
    }

    impl Hand {
        fn new() -> Self {
            let mut g = Gesture::default();
            g.begin(0.0);
            Self { g, t: 1.0, pick: WheelPick::Both, rest: 0.7, opened: Vec::new(), lit: false, shown: 0.0, second: false }
        }

        fn turn(&mut self, by: Vec2, scrolled: bool, button: bool) {
            self.t += 0.008;
            let moves = [Moved { by, at: self.t }];
            self.g.moves(if by == Vec2::ZERO { &[] } else { &moves }, R, true, self.second);
            let pressable = self.g.at().is_some();
            self.lit |= pressable;
            let (opens, shown) = self.g.turn(&Now { at: self.t, scrolled, button, pressable }, self.pick, self.rest);
            self.shown = shown;
            if opens {
                self.opened.push(self.t);
            }
        }

        /// A movement of `by` radii over `seconds`, fast in its middle as a hand's is.
        fn stroke(&mut self, by: Vec2, seconds: f64) {
            let n = (seconds / 0.008).ceil().max(1.0) as usize;
            for i in 0..n {
                let s = |i: usize| (1.0 - (std::f32::consts::PI * i as f32 / n as f32).cos()) / 2.0;
                self.turn(by * R * (s(i + 1) - s(i)), false, false);
            }
        }

        fn wait(&mut self, seconds: f64) {
            for _ in 0..(seconds / 0.008).ceil() as usize {
                self.turn(Vec2::ZERO, false, false);
            }
        }

        fn click(&mut self) {
            for _ in 0..10 {
                self.turn(Vec2::ZERO, false, true);
            }
            self.turn(Vec2::ZERO, false, false);
        }
    }

    fn to(degrees: f32, radii: f32) -> Vec2 {
        super::super::super::zones::polar(radii, degrees)
    }

    /// Pushed to the rim, then past it: the cell there lights, with no key; a rest opens it, shown as it goes.
    #[test]
    fn a_push_past_the_rim_lights_a_cell_and_a_rest_opens_it() {
        for degrees in (0..360).step_by(15) {
            let mut h = Hand::new();
            h.stroke(to(degrees as f32, 1.3), 0.18);
            h.wait(0.3);
            assert!(!h.lit, "{degrees}: VRChat's");
            h.stroke(to(degrees as f32, 0.7), 0.15);
            let at = h.g.at().expect("through");
            assert!(apart(way(at), degrees as f32) < 8.0, "{degrees}: {at:?}");
            assert!((1.06..1.72).contains(&at.length()), "{degrees}: in the ring, {}", at.length());
            h.wait(0.3);
            assert!(h.shown > 0.2 && h.shown < 0.7 && h.opened.is_empty(), "{degrees}: {}", h.shown);
            h.wait(0.6);
            assert_eq!(h.opened.len(), 1, "{degrees}");
            h.wait(3.0);
            assert_eq!(h.opened.len(), 1, "{degrees}: once");
        }
    }

    /// One slow push goes through as well: it need not be let go of at the rim.
    #[test]
    fn one_slow_push_goes_through_too() {
        let mut h = Hand::new();
        h.stroke(to(30.0, 4.0), 1.6);
        assert!(h.g.at().is_some());
    }

    /// One notch opens it at once, or nearly; a click never does.
    #[test]
    fn a_notch_opens_and_a_click_does_not() {
        let mut h = Hand::new();
        h.stroke(to(90.0, 1.2), 0.18);
        h.wait(0.3);
        h.stroke(to(90.0, 0.7), 0.15);
        h.wait(0.2);
        let before = h.t;
        h.turn(Vec2::ZERO, true, false);
        h.wait(0.25);
        assert_eq!(h.opened.len(), 1);
        assert!(h.opened[0] - before < 0.2);

        let mut h = Hand::new();
        h.stroke(to(90.0, 1.2), 0.18);
        h.wait(0.3);
        h.stroke(to(90.0, 0.7), 0.15);
        h.wait(0.2);
        h.click();
        assert!(h.g.at().is_none(), "VRChat's slice was pressed: ours is out");
        h.wait(3.0);
        assert!(h.opened.is_empty());
        // Two fingers laid for a right click: a scroll of nothing, then the button.
        let mut h = Hand::new();
        h.stroke(to(90.0, 1.2), 0.18);
        h.wait(0.3);
        h.stroke(to(90.0, 0.7), 0.15);
        h.wait(0.2);
        h.turn(Vec2::ZERO, true, false);
        h.wait(0.09);
        h.click();
        h.wait(3.0);
        assert!(h.opened.is_empty());
        // A trackpad's momentum, begun before the cell lit, goes on over it.
        let mut h = Hand::new();
        h.pick = WheelPick::Scroll;
        h.stroke(to(90.0, 1.2), 0.18);
        h.wait(0.3);
        for i in 0..60 {
            h.turn(if i < 18 { to(90.0, 0.7 * R / 18.0) } else { Vec2::ZERO }, i % 2 == 0, false);
        }
        assert!(h.g.at().is_some());
        h.wait(0.2);
        assert!(h.opened.is_empty());
    }

    /// What VRChat's wheel is used for, never ours: flicks to a slice of every strength and speed, the hand left there as long as one likes; from slice to slice along the rim;
    /// to and fro; round and round; clicks; and all of it once more after a page of VRChat's wheel opened. Nothing lights, so nothing can open.
    #[test]
    fn ordinary_use_of_the_wheel_never_picks_one_of_ours() {
        let mut tried = 0;
        for degrees in (0..360).step_by(20) {
            for far in [0.4, 0.8, 1.2, 2.0, 3.5, 6.0] {
                for seconds in [0.05, 0.1, 0.2, 0.3] {
                    let mut h = Hand::new();
                    h.stroke(to(degrees as f32, far), seconds);
                    h.wait(2.5);
                    h.click();
                    h.wait(0.4);
                    // The same slice's next page, the other way, and back.
                    h.stroke(to(degrees as f32 + 180.0, far), seconds);
                    h.wait(1.0);
                    h.stroke(to(degrees as f32 + 140.0, far * 0.6), seconds);
                    h.wait(1.0);
                    h.click();
                    assert!(!h.lit && h.opened.is_empty(), "{degrees} {far} {seconds}");
                    tried += 1;
                }
            }
            // Along the rim: from the slice it is on to another, by the straight way there or round.
            for turn in [60.0, 90.0, 120.0, -60.0, -90.0, -120.0] {
                for seconds in [0.12, 0.25, 0.4] {
                    let mut h = Hand::new();
                    h.stroke(to(degrees as f32, 1.4), 0.15);
                    h.wait(0.6);
                    h.stroke(to(degrees as f32 + turn, 1.6), seconds);
                    h.wait(2.0);
                    h.stroke((to(degrees as f32 + 2.0 * turn, 1.0) - to(degrees as f32 + turn, 1.0)) * 1.3, seconds);
                    h.wait(2.0);
                    assert!(!h.lit, "{degrees} {turn} {seconds}");
                    tried += 1;
                }
            }
            // Round and round, as one looks for a slice: never straight out.
            let mut h = Hand::new();
            h.stroke(to(degrees as f32, 1.1), 0.15);
            for step in 0..120 {
                let here = degrees as f32 + step as f32 * 6.0;
                h.turn((to(here + 6.0, 1.05) - to(here, 1.0)) * R, false, false);
            }
            h.wait(2.0);
            assert!(!h.lit, "{degrees} round");
        }
        assert!(tried > 700, "{tried}");
    }

    /// Back inside: out, and the rest begins anew the next time.
    #[test]
    fn moving_back_inside_the_rim_puts_it_out() {
        let mut h = Hand::new();
        h.stroke(to(0.0, 1.2), 0.18);
        h.wait(0.3);
        h.stroke(to(0.0, 0.7), 0.15);
        h.wait(0.4);
        assert!(h.g.at().is_some());
        h.stroke(to(180.0, 0.9), 0.15);
        assert!(h.g.at().is_none());
        h.wait(2.0);
        assert!(h.opened.is_empty());
        // Along the ring to the next cell: the rest is the next cell's.
        h.stroke(to(0.0, 1.0), 0.15);
        h.wait(0.3);
        h.stroke(to(0.0, 0.7), 0.15);
        h.wait(0.5);
        h.stroke(to(90.0, 1.2), 0.2);
        let at = h.g.at().expect("still out");
        assert!(apart(way(at), 45.0) < 25.0, "{at:?}");
        h.wait(0.45);
        assert!(h.opened.is_empty(), "the rest before the move does not count");
        h.wait(0.5);
        assert_eq!(h.opened.len(), 1);
    }

    /// The second ring lies past the first: pushed on, it is reached, and no further.
    #[test]
    fn the_second_ring_is_reached_by_pushing_on() {
        let mut h = Hand::new();
        h.second = true;
        h.stroke(to(300.0, 1.2), 0.18);
        h.wait(0.3);
        // However far the push that lit the cell goes on, it stays on it.
        h.stroke(to(300.0, 3.0), 0.15);
        assert!((1.06..1.72).contains(&h.g.at().unwrap().length()));
        h.wait(0.15);
        h.stroke(to(300.0, 0.5), 0.2);
        assert!(super::super::super::zones::Zone::BloomPeople.ring().unwrap().contains(h.g.at().unwrap() * 100.0), "{:?}", h.g.at());
        h.stroke(to(300.0, 5.0), 0.2);
        assert!(h.g.at().unwrap().length() < 2.5);
        // With none out, the ring is as far as it goes, push as one may.
        let mut h = Hand::new();
        h.stroke(to(0.0, 1.2), 0.18);
        h.wait(0.3);
        for _ in 0..3 {
            h.stroke(to(0.0, 2.0), 0.15);
            h.wait(0.2);
            assert!((1.06..1.72).contains(&h.g.at().unwrap().length()));
        }
    }

    /// Which of the two opens is the person's to choose.
    #[test]
    fn only_what_was_chosen_opens() {
        for (pick, scrolls, rests) in [(WheelPick::Both, 1, 1), (WheelPick::Scroll, 1, 0), (WheelPick::Rest, 0, 1)] {
            for (by_scroll, want) in [(true, scrolls), (false, rests)] {
                let mut h = Hand::new();
                (h.pick, h.rest) = (pick, 1.2);
                h.stroke(to(150.0, 1.2), 0.18);
                h.wait(0.3);
                h.stroke(to(150.0, 0.7), 0.15);
                h.wait(0.2);
                if by_scroll {
                    h.turn(Vec2::ZERO, true, false);
                    h.wait(0.3);
                } else {
                    h.wait(2.0);
                }
                assert_eq!(h.opened.len(), want, "{pick:?} {by_scroll}");
            }
        }
    }

    /// The wheel just opened, or the ring not there yet: the mouse is followed and goes nowhere.
    #[test]
    fn nothing_goes_through_before_the_ring_shows() {
        let mut g = Gesture::default();
        g.begin(0.0);
        let mut t = 0.0;
        for push in 0..4 {
            for _ in 0..20 {
                t += 0.008;
                g.moves(&[Moved { by: vec2(20.0, 0.0), at: t }], R, false, false);
            }
            t += 0.3;
            assert!(g.at().is_none(), "{push}");
        }
        // It shows: as from any place at the rim.
        for _ in 0..8 {
            t += 0.008;
            g.moves(&[Moved { by: vec2(20.0, 0.0), at: t }], R, true, false);
        }
        assert!(g.at().is_some());
        g.end();
        assert!(g.at().is_none());
    }
}
