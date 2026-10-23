//! Where the owner's finger touches the avatar's screen. The screen carries four contact receivers per hand (local, so
//! they cost nothing in VRChat's performance rank), each telling how near that hand's index finger is (VRChat's
//! proximity: 1 at the receiver's centre, 0 at its radius). Three sit on the screen's corners and one behind its middle;
//! from the four distances the desktop finds the finger (and the finger contact's own size, which VRChat sets from
//! the avatar's hand), then which button it is on and whether it presses.
//!
//! Positions are in the screen's own space as the prop is built (metres at 1.6 m eye height; the avatar's scale
//! changes nothing, as receivers and fingers scale together): x to the right, y up, z away from the person (the screen's
//! face looks at −z).

use orbiters_contracts::orb;
use std::time::{Duration, Instant};

/// How long a finger may go unfound (a fit rejected on a mix of new and old readings, a hand moving fast) before it
/// counts as gone: shorter gaps change nothing, so they never let go of a press nor start another.
pub const LOST_GRACE: Duration = Duration::from_millis(100);

/// The screen's size.
pub const WIDTH: f64 = 0.48;
pub const HEIGHT: f64 = 0.27;
/// The receivers' radius.
pub const RADIUS: f64 = 0.8;
/// How far outside the screen's edges the receivers sit: a finger on the screen never reaches one's centre, where its
/// proximity would stop at 1.
pub const MARGIN: f64 = 0.05;
/// The receivers: past the top left and top right corners, under the bottom's middle, behind the middle.
pub const RECEIVERS: [[f64; 3]; 4] = [
    [-WIDTH / 2.0 - MARGIN, HEIGHT / 2.0 + MARGIN, 0.0],
    [WIDTH / 2.0 + MARGIN, HEIGHT / 2.0 + MARGIN, 0.0],
    [0.0, -HEIGHT / 2.0 - MARGIN, 0.0],
    [0.0, 0.0, 0.15],
];

/// A finger in front of (negative) or past (positive) the screen's face closer than this presses.
pub const PRESS_DEPTH: f64 = -0.012;
/// It lets go once this far in front again.
pub const RELEASE_DEPTH: f64 = -0.025;
/// Nearer than this it hovers.
pub const HOVER_DEPTH: f64 = -0.08;

/// Where a finger is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Finger {
    /// On the page, in pixels (0 to `layout::WIDTH`, 0 to `layout::HEIGHT`, top left first).
    pub x: f32,
    pub y: f32,
    /// Distance from the screen's face (negative in front).
    pub depth: f64,
}

/// The finger from the four proximities (0 to 1), or `None` when one is out of reach or they do not agree.
pub fn locate(proximity: [f32; 4]) -> Option<Finger> {
    let [x, y, depth] = point(proximity, [0.0, 0.0, -0.05])?;
    let (x, y) = on_page(x, y);
    Some(Finger { x, y, depth })
}

/// A point of the screen's plane on the page, in pixels.
fn on_page(x: f64, y: f64) -> (f32, f32) {
    (((x + WIDTH / 2.0) / WIDTH * super::layout::WIDTH as f64) as f32, ((HEIGHT / 2.0 - y) / HEIGHT * super::layout::HEIGHT as f64) as f32)
}

/// Where the owner looks on the page in desktop mode (contracts `Docs/orb.md`, Looking and clicking), in pixels: the
/// eyes and the gaze point from their receivers' proximities, and where the line from the eyes through the gaze point
/// meets the screen's plane. Eyes not found: the eyes at rest stand in. `None` when the gaze point is not found or the
/// line meets the plane nowhere ahead of the eyes.
pub fn look(eyes: [f32; 4], gaze: [f32; 4]) -> Option<(f32, f32)> {
    let gaze = point(gaze, [0.0, 0.1, -0.1])?;
    looked_at(point(eyes, GAZE_EYES), gaze)
}

/// The eyes at rest, in the screen's space.
const GAZE_EYES: [f64; 3] = orbiters_contracts::orb::params::GAZE_EYES;

/// Where the line from `eyes` (else the eyes at rest) through `gaze` meets the screen's plane, in pixels.
fn looked_at(eyes: Option<[f64; 3]>, gaze: [f64; 3]) -> Option<(f32, f32)> {
    let c = eyes.unwrap_or(GAZE_EYES);
    let along = gaze[2] - c[2];
    if c[2] >= 0.0 || along <= 1e-6 {
        return None;
    }
    let t = -c[2] / along;
    Some(on_page(c[0] + (gaze[0] - c[0]) * t, c[1] + (gaze[1] - c[1]) * t))
}

/// Where a hand's pointer dot is on the page (screen version 4, contracts `Docs/orb.md`, Pointing), in pixels, from its
/// proximities to the first three touch points (0 to 1), or `None` when one is out of reach or they do not agree. The
/// dot lies on the screen's face, so this is exact: the first receiver's sphere taken from the two others leaves two
/// straight lines meeting at the dot, and the first distance must then agree within 2 cm (else they are not one dot's).
pub fn pointed(proximity: [f32; 3]) -> Option<(f32, f32)> {
    if proximity.iter().any(|&p| !(p > 0.0 && p <= 1.0)) {
        return None;
    }
    // Centre to centre: the receiver's reach less the proximity, and the dot's own radius.
    let d = proximity.map(|p| RADIUS * (1.0 - p as f64) + orb::POINT_RADIUS);
    let c = &RECEIVERS;
    let line = |i: usize| {
        let a = 2.0 * (c[0][0] - c[i][0]);
        let b = 2.0 * (c[0][1] - c[i][1]);
        let r = d[i] * d[i] - d[0] * d[0] - (c[i][0] * c[i][0] + c[i][1] * c[i][1]) + (c[0][0] * c[0][0] + c[0][1] * c[0][1]);
        (a, b, r)
    };
    let ((a1, b1, r1), (a2, b2, r2)) = (line(1), line(2));
    let det = a1 * b2 - b1 * a2;
    if det.abs() < 1e-12 {
        return None;
    }
    let x = (r1 * b2 - b1 * r2) / det;
    let y = (a1 * r2 - r1 * a2) / det;
    if !x.is_finite() || !y.is_finite() || ((x - c[0][0]).hypot(y - c[0][1]) - d[0]).abs() > 0.02 {
        return None;
    }
    Some(on_page(x, y))
}

/// A contact's centre in the screen's space from the four proximities (0 to 1), fitted from `start`, or `None` when
/// one is out of reach or they do not agree.
fn point(proximity: [f32; 4], start: [f64; 3]) -> Option<[f64; 3]> {
    if proximity.iter().any(|&p| !(p > 0.0 && p <= 1.0)) {
        return None;
    }
    // Distance from each receiver's centre to the contact's surface.
    let d: Vec<f64> = proximity.iter().map(|&p| RADIUS * (1.0 - p as f64)).collect();
    // Unknowns: the contact's centre (x, y, z) and its radius r: |f − c_i| = d_i + r. Gauss–Newton from `start`.
    let mut u = [start[0], start[1], start[2], 0.01];
    for _ in 0..30 {
        let mut jtj = [[0.0f64; 4]; 4];
        let mut jtr = [0.0f64; 4];
        for (c, &di) in RECEIVERS.iter().zip(&d) {
            let delta = [u[0] - c[0], u[1] - c[1], u[2] - c[2]];
            let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt().max(1e-9);
            let residual = length - di - u[3];
            let row = [delta[0] / length, delta[1] / length, delta[2] / length, -1.0];
            for a in 0..4 {
                jtr[a] += row[a] * residual;
                for b in 0..4 {
                    jtj[a][b] += row[a] * row[b];
                }
            }
        }
        let step = solve(jtj, jtr)?;
        for a in 0..4 {
            u[a] -= step[a];
        }
        if step.iter().map(|s| s.abs()).fold(0.0, f64::max) < 1e-7 {
            break;
        }
    }
    // The four distances must agree (two hands on one set of receivers do not).
    let worst = RECEIVERS
        .iter()
        .zip(&d)
        .map(|(c, &di)| (((u[0] - c[0]).powi(2) + (u[1] - c[1]).powi(2) + (u[2] - c[2]).powi(2)).sqrt() - di - u[3]).abs())
        .fold(0.0, f64::max);
    if !worst.is_finite() || worst > 0.01 || !(-0.01..0.06).contains(&u[3]) {
        return None;
    }
    Some([u[0], u[1], u[2]])
}

/// `a × x = b` for a 4 × 4 system (Gaussian elimination with pivoting).
fn solve(mut a: [[f64; 4]; 4], mut b: [f64; 4]) -> Option<[f64; 4]> {
    for col in 0..4 {
        let pivot = (col..4).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let pivot_row = a[col];
        for row in col + 1..4 {
            let f = a[row][col] / pivot_row[col];
            for (value, p) in a[row][col..].iter_mut().zip(&pivot_row[col..]) {
                *value -= f * p;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; 4];
    for row in (0..4).rev() {
        let sum: f64 = (row + 1..4).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - sum) / a[row][row];
    }
    Some(x)
}

/// A hand's touch: hovering, pressing, letting go. A press needs the finger seen in front of the screen first (a hand
/// that comes in from the side or from behind presses nothing until it is seen in front again), lasts while the finger
/// stays at or past the face however deep it goes, and ends only once it is back in front.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Touch {
    pressing: bool,
    pub finger: Option<Finger>,
    /// The finger's last reading was in front of the screen's face, over it: what may press next.
    hovering: bool,
    /// The finger reached the face without coming from in front: it presses nothing until it is clearly in front again.
    blocked: bool,
    /// Since when the finger is not found.
    lost_since: Option<Instant>,
}

/// A hand's pointing ray (screen version 4): hovering where its dot is, pressing when the fingertip comes within
/// [`orb::RAY_PRESS`] of the screen along the ray. A press needs the fingertip seen farther away first (a hand that
/// enters already close presses nothing until it backs off), and ends once it is back to [`orb::RAY_RELEASE`], or once
/// the ray has missed for [`LOST_GRACE`] (a finger pushed through the face).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pointer {
    pressing: bool,
    /// Where the dot is on the page.
    pub at: Option<(f32, f32)>,
    /// The fingertip was seen at least [`orb::RAY_RELEASE`] away over the screen: what may press next.
    armed: bool,
    /// Since when the dot is not found.
    lost_since: Option<Instant>,
}

impl Pointer {
    /// The hand's new reading at `now`: its ray meets the screen (`hit`) at `ratio` of its length, and the dot's
    /// proximities to the first three touch points.
    pub fn update(&mut self, hit: bool, ratio: f32, proximity: [f32; 3], now: Instant) -> Gesture {
        let Some((x, y)) = hit.then(|| pointed(proximity)).flatten() else { return self.unfound(now) };
        self.lost_since = None;
        self.at = Some((x, y));
        let inside = x >= 0.0 && y >= 0.0 && x < super::layout::WIDTH as f32 && y < super::layout::HEIGHT as f32;
        if self.pressing {
            if ratio < orb::RAY_RELEASE {
                return Gesture::Hold { x, y };
            }
            self.pressing = false;
        }
        if ratio <= orb::RAY_PRESS {
            if inside && self.armed {
                self.pressing = true;
                self.armed = false;
                return Gesture::Press { x, y };
            }
            return if inside { Gesture::Hover { x, y } } else { Gesture::Away };
        }
        if ratio >= orb::RAY_RELEASE {
            self.armed = inside;
        }
        if inside {
            Gesture::Hover { x, y }
        } else {
            Gesture::Away
        }
    }

    /// No new reading came: a dot unfound for [`LOST_GRACE`] is gone (`Some(Away)`), the pointer starting afresh.
    pub fn tick(&mut self, now: Instant) -> Option<Gesture> {
        let gone = self.lost_since.is_some_and(|since| now.duration_since(since) >= LOST_GRACE);
        (gone && (self.at.is_some() || self.pressing || self.armed)).then(|| {
            *self = Pointer::default();
            Gesture::Away
        })
    }

    fn unfound(&mut self, now: Instant) -> Gesture {
        let since = *self.lost_since.get_or_insert(now);
        if now.duration_since(since) >= LOST_GRACE {
            *self = Pointer { lost_since: Some(since), ..Pointer::default() };
            return Gesture::Away;
        }
        match self.at {
            Some((x, y)) if self.pressing => Gesture::Hold { x, y },
            Some((x, y)) => Gesture::Hover { x, y },
            None => Gesture::Away,
        }
    }
}

/// What a hand did this update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    /// Near the screen at (x, y): show what it would press.
    Hover { x: f32, y: f32 },
    /// Touched at (x, y).
    Press { x: f32, y: f32 },
    /// Still touching.
    Hold { x: f32, y: f32 },
    /// Left the screen or went away.
    Away,
}

impl Touch {
    /// The hand's new proximities, read at `now`.
    pub fn update(&mut self, proximity: [f32; 4], now: Instant) -> Gesture {
        let Some(f) = locate(proximity) else { return self.unfound(now) };
        self.lost_since = None;
        self.finger = Some(f);
        let inside = f.x >= 0.0 && f.y >= 0.0 && f.x < super::layout::WIDTH as f32 && f.y < super::layout::HEIGHT as f32;
        if self.pressing {
            if f.depth >= RELEASE_DEPTH {
                // Still at the face, or past it however deep: the same press.
                return Gesture::Hold { x: f.x, y: f.y };
            }
            self.pressing = false;
        }
        if f.depth > PRESS_DEPTH {
            // At the face or past it. A press only when the finger came from in front, over the screen.
            if inside && self.hovering && !self.blocked {
                self.pressing = true;
                self.hovering = false;
                return Gesture::Press { x: f.x, y: f.y };
            }
            self.blocked = true;
            self.hovering = false;
            return if inside && f.depth <= 0.08 { Gesture::Hover { x: f.x, y: f.y } } else { Gesture::Away };
        }
        // In front of the face: hovering when over the screen and near enough. A blocked finger is free once it is
        // clearly back in front.
        if f.depth < RELEASE_DEPTH {
            self.blocked = false;
        }
        self.hovering = inside && f.depth >= HOVER_DEPTH;
        if self.hovering {
            Gesture::Hover { x: f.x, y: f.y }
        } else {
            Gesture::Away
        }
    }

    /// No new reading came: a finger unfound for [`LOST_GRACE`] is gone (`Some(Away)`), the touch starting afresh.
    pub fn tick(&mut self, now: Instant) -> Option<Gesture> {
        let gone = self.lost_since.is_some_and(|since| now.duration_since(since) >= LOST_GRACE);
        (gone && (self.finger.is_some() || self.pressing || self.hovering || self.blocked)).then(|| {
            *self = Touch::default();
            Gesture::Away
        })
    }

    /// A reading that did not find the finger: a short gap keeps the touch as it was, a longer one ends it.
    fn unfound(&mut self, now: Instant) -> Gesture {
        let since = *self.lost_since.get_or_insert(now);
        if now.duration_since(since) >= LOST_GRACE {
            *self = Touch { lost_since: Some(since), ..Touch::default() };
            return Gesture::Away;
        }
        match self.finger {
            Some(f) if self.pressing => Gesture::Hold { x: f.x, y: f.y },
            Some(f) if self.hovering => Gesture::Hover { x: f.x, y: f.y },
            _ => Gesture::Away,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the receivers read for a finger at `f` whose contact has radius `r`.
    fn read(f: [f64; 3], r: f64) -> [f32; 4] {
        let mut out = [0f32; 4];
        for (o, c) in out.iter_mut().zip(RECEIVERS) {
            let d = ((f[0] - c[0]).powi(2) + (f[1] - c[1]).powi(2) + (f[2] - c[2]).powi(2)).sqrt() - r;
            *o = (1.0 - (d / RADIUS)).clamp(0.0, 1.0) as f32;
        }
        out
    }

    #[test]
    fn finds_the_finger_anywhere_on_the_screen() {
        for &r in &[0.0, 0.008, 0.02] {
            for &x in &[-0.23, -0.1, 0.0, 0.17, 0.235] {
                for &y in &[-0.13, -0.05, 0.0, 0.12] {
                    for &z in &[-0.07, -0.02, -0.005, 0.0, 0.01] {
                        let found = locate(read([x, y, z], r)).unwrap_or_else(|| panic!("{x} {y} {z} {r}"));
                        let px = ((x + WIDTH / 2.0) / WIDTH * 1024.0) as f32;
                        let py = ((HEIGHT / 2.0 - y) / HEIGHT * 576.0) as f32;
                        // Within 2 px on the page and 1 mm deep (float proximities).
                        assert!((found.x - px).abs() < 2.0 && (found.y - py).abs() < 2.0, "{x} {y} {z} {r}: {found:?} vs {px} {py}");
                        assert!((found.depth - z).abs() < 0.001, "{x} {y} {z} {r}: depth {}", found.depth);
                    }
                }
            }
        }
        assert_eq!(locate([0.0, 0.5, 0.5, 0.5]), None, "a receiver that does not see the finger");
        // Two fingers, one near the left corner and one near the right: the readings do not agree.
        let left = read([-0.2, 0.1, -0.01], 0.01);
        let right = read([0.2, -0.1, -0.01], 0.01);
        assert_eq!(locate([left[0], right[1], left[2], right[3]]), None);
    }

    /// Readings 14 ms apart, as the host takes them.
    struct Clock(Instant);

    impl Clock {
        fn next(&mut self) -> Instant {
            self.0 += Duration::from_millis(14);
            self.0
        }
    }

    #[test]
    fn presses_once_and_lets_go() {
        let mut touch = Touch::default();
        let mut clock = Clock(Instant::now());
        let at = |z: f64| read([0.0, 0.0, z], 0.01);
        assert!(matches!(touch.update(at(-0.2), clock.next()), Gesture::Away), "too far");
        assert!(matches!(touch.update(at(-0.05), clock.next()), Gesture::Hover { .. }));
        assert!(matches!(touch.update(at(-0.005), clock.next()), Gesture::Press { .. }));
        assert!(matches!(touch.update(at(0.01), clock.next()), Gesture::Hold { .. }), "pushing through holds");
        assert!(matches!(touch.update(at(-0.018), clock.next()), Gesture::Hold { .. }), "between the depths it still holds");
        assert!(matches!(touch.update(at(-0.03), clock.next()), Gesture::Hover { .. }));
        assert!(matches!(touch.update(at(-0.005), clock.next()), Gesture::Press { .. }), "and presses again");
    }

    #[test]
    fn a_hand_from_the_side_or_behind_presses_nothing() {
        let mut touch = Touch::default();
        let mut clock = Clock(Instant::now());
        // Beside the screen, at its face, then over it: no approach from the front.
        assert!(matches!(touch.update(read([0.4, 0.0, -0.005], 0.01), clock.next()), Gesture::Away));
        assert!(matches!(touch.update(read([0.2, 0.0, -0.005], 0.01), clock.next()), Gesture::Hover { .. }));
        // From behind, coming through.
        assert!(!matches!(touch.update(read([0.0, 0.0, 0.05], 0.01), clock.next()), Gesture::Press { .. }));
        assert!(!matches!(touch.update(read([0.0, 0.0, -0.005], 0.01), clock.next()), Gesture::Press { .. }));
        assert!(!matches!(touch.update(read([0.0, 0.0, -0.018], 0.01), clock.next()), Gesture::Press { .. }), "not clearly out yet");
        assert!(!matches!(touch.update(read([0.0, 0.0, -0.005], 0.01), clock.next()), Gesture::Press { .. }));
        // Clearly in front, then in: a press.
        assert!(matches!(touch.update(read([0.0, 0.0, -0.04], 0.01), clock.next()), Gesture::Hover { .. }));
        assert!(matches!(touch.update(read([0.0, 0.0, -0.005], 0.01), clock.next()), Gesture::Press { .. }));
    }

    #[test]
    fn deep_pushes_and_short_gaps_keep_one_press() {
        let mut touch = Touch::default();
        let mut clock = Clock(Instant::now());
        let at = |z: f64| read([0.05, 0.02, z], 0.01);
        touch.update(at(-0.05), clock.next());
        assert!(matches!(touch.update(at(-0.005), clock.next()), Gesture::Press { .. }));
        // Far through the screen and back: one press.
        for z in [0.05, 0.12, 0.2, 0.09, 0.03, -0.01] {
            assert!(matches!(touch.update(at(z), clock.next()), Gesture::Hold { .. }), "{z}");
        }
        // A reading that finds nothing (two hands on the receivers), then the finger again: still the same press.
        assert!(matches!(touch.update([0.0, 0.5, 0.5, 0.5], clock.next()), Gesture::Hold { .. }));
        assert_eq!(touch.tick(clock.next()), None, "a short gap");
        assert!(matches!(touch.update(at(-0.005), clock.next()), Gesture::Hold { .. }));
        // Gone for longer than the grace: the touch ends, once.
        let lost = clock.next();
        assert!(matches!(touch.update([0.0; 4], lost), Gesture::Hold { .. }));
        assert_eq!(touch.tick(lost + LOST_GRACE), Some(Gesture::Away));
        assert_eq!(touch.tick(lost + LOST_GRACE * 2), None);
        // Back at the face without an approach: nothing.
        assert!(!matches!(touch.update(at(-0.005), lost + LOST_GRACE * 3), Gesture::Press { .. }));
    }

    /// `v` turned down by `pitch` then right by `yaw` (radians), as the head bone turns.
    fn turned(v: [f64; 3], pitch: f64, yaw: f64) -> [f64; 3] {
        let (y, z) = (v[1] * pitch.cos() - v[2] * pitch.sin(), v[1] * pitch.sin() + v[2] * pitch.cos());
        [v[0] * yaw.cos() + z * yaw.sin(), y, -v[0] * yaw.sin() + z * yaw.cos()]
    }

    /// Where the line of sight from `eyes` along `ahead` meets the screen's plane, on the page.
    fn sight(eyes: [f64; 3], ahead: [f64; 3]) -> (f32, f32) {
        let t = -eyes[2] / ahead[2];
        on_page(eyes[0] + ahead[0] * t, eyes[1] + ahead[1] * t)
    }

    /// Within 1 mm on the screen.
    fn near(found: (f32, f32), wanted: (f32, f32)) -> bool {
        let mm = super::super::layout::WIDTH as f32 / (WIDTH as f32 * 1000.0);
        (found.0 - wanted.0).abs() < mm && (found.1 - wanted.1).abs() < mm
    }

    #[test]
    fn the_gaze_point_follows_the_line_of_sight_onto_the_screen() {
        // The head bone turns about the neck, below and behind the eyes; the gaze point sits 0.38 m ahead of them.
        let neck = [GAZE_EYES[0], GAZE_EYES[1] - 0.1, GAZE_EYES[2] + 0.08];
        for pitch in [5.0f64, 15.0, 25.0, 32.0, 40.0] {
            for yaw in [-25.0f64, -10.0, 0.0, 12.0, 25.0] {
                let (pitch, yaw) = (pitch.to_radians(), yaw.to_radians());
                let offset = turned([0.0, 0.1, -0.08], pitch, yaw);
                let eyes = [neck[0] + offset[0], neck[1] + offset[1], neck[2] + offset[2]];
                let ahead = turned([0.0, 0.0, 1.0], pitch, yaw);
                let gaze = [eyes[0] + 0.38 * ahead[0], eyes[1] + 0.38 * ahead[1], eyes[2] + 0.38 * ahead[2]];
                assert!(gaze[2] < 0.0, "the gaze point stays in front of the screen");
                let found = look(read(eyes, 0.01), read(gaze, 0.01)).unwrap_or_else(|| panic!("{pitch} {yaw}"));
                let wanted = sight(eyes, ahead);
                assert!(near(found, wanted), "pitch {pitch}, yaw {yaw}: {found:?} vs {wanted:?}");
            }
        }
    }

    #[test]
    fn eyes_not_found_stand_at_rest() {
        for (pitch, yaw) in [(25.0f64, 0.0f64), (20.0, -15.0), (35.0, 20.0)] {
            let ahead = turned([0.0, 0.0, 1.0], pitch.to_radians(), yaw.to_radians());
            let gaze = [GAZE_EYES[0] + 0.38 * ahead[0], GAZE_EYES[1] + 0.38 * ahead[1], GAZE_EYES[2] + 0.38 * ahead[2]];
            let found = look([0.0; 4], read(gaze, 0.01)).unwrap();
            assert!(near(found, sight(GAZE_EYES, ahead)), "{pitch} {yaw}: {found:?}");
        }
        // No gaze point: nowhere; looking away from the screen: nowhere.
        assert_eq!(look(read(GAZE_EYES, 0.01), [0.0; 4]), None);
        assert_eq!(looked_at(Some(GAZE_EYES), [0.0, 0.2, -0.5]), None);
    }

    /// What a hand's three touch points read for its dot at page pixel (`px`, `py`), on the screen's face.
    fn dot(px: f32, py: f32) -> [f32; 3] {
        let x = px as f64 / super::super::layout::WIDTH as f64 * WIDTH - WIDTH / 2.0;
        let y = HEIGHT / 2.0 - py as f64 / super::super::layout::HEIGHT as f64 * HEIGHT;
        let mut out = [0f32; 3];
        for (o, c) in out.iter_mut().zip(&RECEIVERS[..3]) {
            let d = (x - c[0]).hypot(y - c[1]) - orb::POINT_RADIUS;
            *o = (1.0 - d / RADIUS) as f32;
        }
        out
    }

    #[test]
    fn a_dot_is_found_exactly_on_the_face() {
        for (x, y) in [(10.0, 10.0), (512.0, 288.0), (1000.0, 560.0), (700.0, 120.0)] {
            let (fx, fy) = pointed(dot(x, y)).expect("found");
            assert!((fx - x).abs() < 0.2 && (fy - y).abs() < 0.2, "({x}, {y}) found at ({fx}, {fy})");
        }
        // Readings no single dot gives (8 cm from three corners 0.58 m apart) do not agree.
        assert_eq!(pointed([0.9, 0.9, 0.9]), None);
        assert_eq!(pointed([0.0, 0.5, 0.5]), None, "out of reach");
    }

    #[test]
    fn the_fingertip_presses_near_the_dot_after_coming_from_farther() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut p = Pointer::default();
        assert_eq!(p.update(true, 0.02, dot(300.0, 200.0), ms(0)), Gesture::Hover { x: p.at.unwrap().0, y: p.at.unwrap().1 }, "close at once: no press");
        assert!(matches!(p.update(true, 0.3, dot(300.0, 200.0), ms(10)), Gesture::Hover { .. }));
        assert!(matches!(p.update(true, 0.05, dot(300.0, 200.0), ms(20)), Gesture::Hover { .. }), "between: hover");
        assert!(matches!(p.update(true, 0.03, dot(300.0, 200.0), ms(30)), Gesture::Press { .. }));
        assert!(matches!(p.update(true, 0.05, dot(302.0, 200.0), ms(40)), Gesture::Hold { .. }), "not back far enough");
        // Pushed through the face: the ray misses, the press holds a moment, then lets go.
        assert!(matches!(p.update(false, 0.0, [0.0; 3], ms(50)), Gesture::Hold { .. }));
        assert_eq!(p.tick(ms(200)), Some(Gesture::Away));
        assert!(matches!(p.update(true, 0.02, dot(300.0, 200.0), ms(210)), Gesture::Hover { .. }), "back out close: no new press");
        assert!(matches!(p.update(true, 0.07, dot(300.0, 200.0), ms(220)), Gesture::Hover { .. }));
        assert!(matches!(p.update(true, 0.07, dot(300.0, 200.0), ms(230)), Gesture::Hover { .. }));
        assert!(matches!(p.update(true, 0.035, dot(300.0, 200.0), ms(240)), Gesture::Press { .. }), "a second press");
    }
}
