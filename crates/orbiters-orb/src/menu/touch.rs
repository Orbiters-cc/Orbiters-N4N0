//! Where the owner's fingers point at and touch the avatar's screen. The screen carries four contact receivers per
//! sender (local, so they cost nothing in VRChat's performance rank), each telling how near the sender is (VRChat's
//! proximity: 1 at the receiver's centre, 0 at its radius). Three sit past the screen's corners and one behind its
//! middle; from the four distances the desktop finds the sender (and its contact's own size). From screen version 7 each
//! index finger has two senders, at its tip and further along it: the line through both is the ray it points along,
//! hovered where it meets the screen and pressed with the trigger, and the tip itself presses when it touches the
//! screen. Everyone else's fingertips have four receivers of their own: a touch presses too.
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
/// The receivers' radius: the eyes', the gaze point's, everyone else's fingertips' (and every finger's before version 7).
pub const RADIUS: f64 = 0.8;
/// The owner's fingertips' and aims' receivers' radius (screen version 7): far enough to point from across a room.
pub const REACH: f64 = orb::POINTING_REACH;
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

/// The finger from the four proximities (0 to 1) of receivers of `radius`, or `None` when one is out of reach or they do
/// not agree.
pub fn locate(proximity: [f32; 4], radius: f64) -> Option<Finger> {
    let [x, y, depth] = point(proximity, radius, [0.0, 0.0, -0.05])?;
    let (x, y) = on_page(x, y);
    Some(Finger { x, y, depth })
}

/// Where an index finger points (screen version 7), in pixels: its fingertip and the point [`orb::AIM_DISTANCE`] along it,
/// found from their receivers ([`REACH`]), and where the line from the tip through that point meets the screen's plane.
/// `None` when either is not found, the finger points away from the screen or along it, or the tip is past the screen.
pub fn aimed(tip: [f32; 4], aim: [f32; 4]) -> Option<(f32, f32)> {
    let t = point(tip, REACH, [0.0, 0.0, -0.2])?;
    let a = point(aim, REACH, [0.0, 0.0, 0.0])?;
    let along = [a[0] - t[0], a[1] - t[1], a[2] - t[2]];
    let length = (along[0] * along[0] + along[1] * along[1] + along[2] * along[2]).sqrt();
    // Two points this near (the receivers found one sender twice) or a line along the screen tell nothing.
    if length < orb::AIM_DISTANCE * 0.25 || along[2] / length < 0.02 || t[2] > 0.01 {
        return None;
    }
    let k = -t[2] / along[2];
    Some(on_page(t[0] + along[0] * k, t[1] + along[1] * k))
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
    let gaze = point(gaze, RADIUS, [0.0, 0.1, -0.1])?;
    looked_at(point(eyes, RADIUS, GAZE_EYES), gaze)
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

/// A contact's centre from four surface distances. Subtracting the squared distance equations makes the centre
/// linear in the unknown sender radius; substituting it back gives a quadratic. Unlike an iterative fit, this also
/// finds hands beside the screen where the initial guess's Jacobian is singular or converges to the wrong root.
/// `start` selects the nearer solution if both satisfy the physical radius and residual limits.
fn point(proximity: [f32; 4], radius: f64, start: [f64; 3]) -> Option<[f64; 3]> {
    if proximity.iter().any(|&p| !(p > 0.0 && p <= 1.0)) {
        return None;
    }
    // Distance from each receiver's centre to the contact's surface.
    let d = proximity.map(|p| radius * (1.0 - p as f64));
    let dot = |a: [f64; 3], b: [f64; 3]| (0..3).map(|i| a[i] * b[i]).sum::<f64>();
    let c0 = RECEIVERS[0];
    let matrix = [1, 2, 3].map(|i| [0, 1, 2].map(|axis| RECEIVERS[i][axis] - c0[axis]));
    let base = solve(matrix, [1, 2, 3].map(|i| (dot(RECEIVERS[i], RECEIVERS[i]) - dot(c0, c0) - d[i] * d[i] + d[0] * d[0]) * 0.5))?;
    let slope = solve(matrix, [1, 2, 3].map(|i| d[0] - d[i]))?;
    let offset = [0, 1, 2].map(|i| base[i] - c0[i]);
    let a = dot(slope, slope) - 1.0;
    let b = 2.0 * (dot(offset, slope) - d[0]);
    let c = dot(offset, offset) - d[0] * d[0];
    let radii = if a.abs() < 1e-12 {
        if b.abs() < 1e-12 {
            return None;
        }
        [-c / b; 2]
    } else {
        let discriminant = b * b - 4.0 * a * c;
        if discriminant < 0.0 {
            return None;
        }
        // This form avoids subtracting nearly equal numbers for the small (physical) radius.
        let q = -0.5 * (b + discriminant.sqrt().copysign(b));
        if q == 0.0 {
            [-b / (2.0 * a); 2]
        } else {
            [q / a, c / q]
        }
    };
    radii
        .into_iter()
        .filter_map(|r| {
            if !(-0.01..0.06).contains(&r) {
                return None;
            }
            let p = [0, 1, 2].map(|i| base[i] + slope[i] * r);
            if p.iter().any(|v| !v.is_finite()) {
                return None;
            }
            // Keep the existing consistency check: mixed readings must not invent a finger.
            let worst = RECEIVERS
                .iter()
                .zip(d)
                .map(|(receiver, distance)| {
                    let delta = [0, 1, 2].map(|i| p[i] - receiver[i]);
                    (dot(delta, delta).sqrt() - distance - r).abs()
                })
                .fold(0.0, f64::max);
            (worst <= 0.01).then_some(p)
        })
        .min_by(|p, q| {
            let distance = |p: &[f64; 3]| (0..3).map(|i| (p[i] - start[i]).powi(2)).sum::<f64>();
            distance(p).total_cmp(&distance(q))
        })
}

/// `a × x = b` (Gaussian elimination with pivoting).
fn solve<const N: usize>(mut a: [[f64; N]; N], mut b: [f64; N]) -> Option<[f64; N]> {
    for col in 0..N {
        let pivot = (col..N).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        let pivot_row = a[col];
        for row in col + 1..N {
            let f = a[row][col] / pivot_row[col];
            for (value, p) in a[row][col..].iter_mut().zip(&pivot_row[col..]) {
                *value -= f * p;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; N];
    for row in (0..N).rev() {
        let sum: f64 = (row + 1..N).map(|k| a[row][k] * x[k]).sum();
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

/// A hand's pointing ray (screen version 7): hovering where the finger points, pressing on the trigger's pull while it
/// already pointed at the screen (a trigger held as the finger comes over presses nothing), holding until the trigger
/// is let go (wherever the finger points meanwhile). A reading that finds nothing keeps the ray where it was for
/// [`LOST_GRACE`], then it is gone.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ray {
    pressing: bool,
    /// Where the finger points on the page.
    pub at: Option<(f32, f32)>,
    /// The trigger at the last reading, and whether that reading pointed at the screen.
    trigger: bool,
    over: bool,
    /// A pull during an incomplete reading waits for a valid aim, at most LOST_GRACE.
    pending_pull: bool,
    /// Since when the ray is not found.
    lost_since: Option<Instant>,
}

impl Ray {
    /// The hand's reading at `now`: where it points (`None`: not found) and whether its trigger is pulled.
    pub fn update(&mut self, aim: Option<(f32, f32)>, trigger: bool, now: Instant) -> Gesture {
        if self.lost_since.is_some_and(|since| now.duration_since(since) >= LOST_GRACE) {
            *self = Ray { trigger: self.trigger, ..Ray::default() };
        }
        let pulled = trigger && !self.trigger && self.over;
        self.trigger = trigger;
        self.pending_pull = trigger && (self.pending_pull || pulled);
        let Some((x, y)) = aim else { return self.unfound(now) };
        let pulled = std::mem::take(&mut self.pending_pull);
        self.lost_since = None;
        self.at = Some((x, y));
        if self.pressing {
            if trigger {
                return Gesture::Hold { x, y };
            }
            self.pressing = false;
        }
        let inside = x >= 0.0 && y >= 0.0 && x < super::layout::WIDTH as f32 && y < super::layout::HEIGHT as f32;
        self.over = inside;
        if !inside {
            return Gesture::Away;
        }
        if pulled {
            self.pressing = true;
            return Gesture::Press { x, y };
        }
        Gesture::Hover { x, y }
    }

    /// No new reading came: a ray unfound for [`LOST_GRACE`] is gone (`Some(Away)`), starting afresh.
    pub fn tick(&mut self, now: Instant) -> Option<Gesture> {
        let gone = self.lost_since.is_some_and(|since| now.duration_since(since) >= LOST_GRACE);
        (gone && (self.at.is_some() || self.pressing)).then(|| {
            *self = Ray { trigger: self.trigger, ..Ray::default() };
            Gesture::Away
        })
    }

    fn unfound(&mut self, now: Instant) -> Gesture {
        let since = *self.lost_since.get_or_insert(now);
        if now.duration_since(since) >= LOST_GRACE {
            *self = Ray { lost_since: Some(since), trigger: self.trigger, ..Ray::default() };
            return Gesture::Away;
        }
        match self.at {
            Some((x, y)) if self.pressing && self.trigger => Gesture::Hold { x, y },
            Some((x, y)) if !self.pressing => Gesture::Hover { x, y },
            _ => {
                self.pressing = false;
                Gesture::Away
            }
        }
    }
}

/// What a hand does when its fingertip and its ray both say something: a touch's press or hold first, then the ray's,
/// then a touch's hover (the fingertip is right at the screen), then the ray's hover.
pub fn either(touch: Option<Gesture>, ray: Option<Gesture>) -> Option<Gesture> {
    let pressing = |g: &Option<Gesture>| matches!(g, Some(Gesture::Press { .. } | Gesture::Hold { .. }));
    if pressing(&touch) {
        return touch;
    }
    if pressing(&ray) {
        return ray;
    }
    if matches!(touch, Some(Gesture::Hover { .. })) {
        return touch;
    }
    match (touch, ray) {
        (_, Some(g)) => Some(g),
        (g, None) => g,
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
    /// The hand's new proximities to receivers of `radius`, read at `now`.
    pub fn update(&mut self, proximity: [f32; 4], radius: f64, now: Instant) -> Gesture {
        let Some(f) = locate(proximity, radius) else { return self.unfound(now) };
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

    /// A touch read by receivers of the eyes' and everyone else's radius.
    trait At {
        fn update_at(&mut self, proximity: [f32; 4], now: Instant) -> Gesture;
    }

    impl At for Touch {
        fn update_at(&mut self, proximity: [f32; 4], now: Instant) -> Gesture {
            self.update(proximity, RADIUS, now)
        }
    }

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
                        let found = locate(read([x, y, z], r), RADIUS).unwrap_or_else(|| panic!("{x} {y} {z} {r}"));
                        let px = ((x + WIDTH / 2.0) / WIDTH * 1024.0) as f32;
                        let py = ((HEIGHT / 2.0 - y) / HEIGHT * 576.0) as f32;
                        // Within 2 px on the page and 1 mm deep (float proximities).
                        assert!((found.x - px).abs() < 2.0 && (found.y - py).abs() < 2.0, "{x} {y} {z} {r}: {found:?} vs {px} {py}");
                        assert!((found.depth - z).abs() < 0.001, "{x} {y} {z} {r}: depth {}", found.depth);
                    }
                }
            }
        }
        assert_eq!(locate([0.0, 0.5, 0.5, 0.5], RADIUS), None, "a receiver that does not see the finger");
        // Two fingers, one near the left corner and one near the right: the readings do not agree.
        let left = read([-0.2, 0.1, -0.01], 0.01);
        let right = read([0.2, -0.1, -0.01], 0.01);
        assert_eq!(locate([left[0], right[1], left[2], right[3]], RADIUS), None);
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
        assert!(matches!(touch.update_at(at(-0.2), clock.next()), Gesture::Away), "too far");
        assert!(matches!(touch.update_at(at(-0.05), clock.next()), Gesture::Hover { .. }));
        assert!(matches!(touch.update_at(at(-0.005), clock.next()), Gesture::Press { .. }));
        assert!(matches!(touch.update_at(at(0.01), clock.next()), Gesture::Hold { .. }), "pushing through holds");
        assert!(matches!(touch.update_at(at(-0.018), clock.next()), Gesture::Hold { .. }), "between the depths it still holds");
        assert!(matches!(touch.update_at(at(-0.03), clock.next()), Gesture::Hover { .. }));
        assert!(matches!(touch.update_at(at(-0.005), clock.next()), Gesture::Press { .. }), "and presses again");
    }

    #[test]
    fn a_hand_from_the_side_or_behind_presses_nothing() {
        let mut touch = Touch::default();
        let mut clock = Clock(Instant::now());
        // Beside the screen, at its face, then over it: no approach from the front.
        assert!(matches!(touch.update_at(read([0.4, 0.0, -0.005], 0.01), clock.next()), Gesture::Away));
        assert!(matches!(touch.update_at(read([0.2, 0.0, -0.005], 0.01), clock.next()), Gesture::Hover { .. }));
        // From behind, coming through.
        assert!(!matches!(touch.update_at(read([0.0, 0.0, 0.05], 0.01), clock.next()), Gesture::Press { .. }));
        assert!(!matches!(touch.update_at(read([0.0, 0.0, -0.005], 0.01), clock.next()), Gesture::Press { .. }));
        assert!(!matches!(touch.update_at(read([0.0, 0.0, -0.018], 0.01), clock.next()), Gesture::Press { .. }), "not clearly out yet");
        assert!(!matches!(touch.update_at(read([0.0, 0.0, -0.005], 0.01), clock.next()), Gesture::Press { .. }));
        // Clearly in front, then in: a press.
        assert!(matches!(touch.update_at(read([0.0, 0.0, -0.04], 0.01), clock.next()), Gesture::Hover { .. }));
        assert!(matches!(touch.update_at(read([0.0, 0.0, -0.005], 0.01), clock.next()), Gesture::Press { .. }));
    }

    #[test]
    fn deep_pushes_and_short_gaps_keep_one_press() {
        let mut touch = Touch::default();
        let mut clock = Clock(Instant::now());
        let at = |z: f64| read([0.05, 0.02, z], 0.01);
        touch.update_at(at(-0.05), clock.next());
        assert!(matches!(touch.update_at(at(-0.005), clock.next()), Gesture::Press { .. }));
        // Far through the screen and back: one press.
        for z in [0.05, 0.12, 0.2, 0.09, 0.03, -0.01] {
            assert!(matches!(touch.update_at(at(z), clock.next()), Gesture::Hold { .. }), "{z}");
        }
        // A reading that finds nothing (two hands on the receivers), then the finger again: still the same press.
        assert!(matches!(touch.update_at([0.0, 0.5, 0.5, 0.5], clock.next()), Gesture::Hold { .. }));
        assert_eq!(touch.tick(clock.next()), None, "a short gap");
        assert!(matches!(touch.update_at(at(-0.005), clock.next()), Gesture::Hold { .. }));
        // Gone for longer than the grace: the touch ends, once.
        let lost = clock.next();
        assert!(matches!(touch.update_at([0.0; 4], lost), Gesture::Hold { .. }));
        assert_eq!(touch.tick(lost + LOST_GRACE), Some(Gesture::Away));
        assert_eq!(touch.tick(lost + LOST_GRACE * 2), None);
        // Back at the face without an approach: nothing.
        assert!(!matches!(touch.update_at(at(-0.005), lost + LOST_GRACE * 3), Gesture::Press { .. }));
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

    /// What receivers of `radius` read for a contact at `f` of radius `r`.
    fn read_far(f: [f64; 3], r: f64, radius: f64) -> [f32; 4] {
        let mut out = [0f32; 4];
        for (o, c) in out.iter_mut().zip(RECEIVERS) {
            let d = ((f[0] - c[0]).powi(2) + (f[1] - c[1]).powi(2) + (f[2] - c[2]).powi(2)).sqrt() - r;
            *o = (1.0 - (d / radius)).clamp(0.0, 1.0) as f32;
        }
        out
    }

    #[test]
    fn a_finger_s_ray_lands_where_it_points() {
        // Fingertips from close to arm's length and further, pointing at points over the screen from aside, above, below.
        for tip in [[0.0, -0.1, -0.15], [-0.25, 0.05, -0.35], [0.3, -0.25, -0.5], [0.1, 0.3, -1.1]] {
            for target in [[0.0, 0.0, 0.0], [-0.2, 0.11, 0.0], [0.21, -0.12, 0.0], [0.05, 0.02, 0.0]] {
                let along: [f64; 3] = [target[0] - tip[0], target[1] - tip[1], target[2] - tip[2]];
                let length = (along[0] * along[0] + along[1] * along[1] + along[2] * along[2]).sqrt();
                let aim = [0, 1, 2].map(|i| tip[i] + along[i] / length * orb::AIM_DISTANCE);
                let r = orb::POINT_RADIUS;
                let found = aimed(read_far(tip, r, REACH), read_far(aim, r, REACH)).unwrap_or_else(|| panic!("{tip:?} at {target:?}"));
                let wanted = on_page(target[0], target[1]);
                // Within 1 mm on the screen (float proximities).
                let mm = super::super::layout::WIDTH as f32 / (WIDTH as f32 * 1000.0);
                assert!((found.0 - wanted.0).abs() < mm && (found.1 - wanted.1).abs() < mm, "{tip:?} at {target:?}: {found:?} vs {wanted:?}");
            }
        }
        // Pointing away, along the screen, or from behind it: nowhere.
        let r = orb::POINT_RADIUS;
        assert_eq!(aimed(read_far([0.0, 0.0, -0.3], r, REACH), read_far([0.0, 0.0, -0.6], r, REACH)), None, "away");
        assert_eq!(aimed(read_far([0.0, 0.0, -0.3], r, REACH), read_far([0.3, 0.0, -0.3], r, REACH)), None, "along");
        assert_eq!(aimed(read_far([0.0, 0.0, 0.1], r, REACH), read_far([0.0, 0.0, 0.4], r, REACH)), None, "from behind");
        assert_eq!(aimed([0.0; 4], read_far([0.0, 0.0, 0.0], r, REACH)), None, "no fingertip");
    }

    #[test]
    fn the_trigger_presses_where_the_ray_points_and_holds_until_let_go() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut ray = Ray::default();
        assert_eq!(ray.update(Some((300.0, 200.0)), true, ms(0)), Gesture::Hover { x: 300.0, y: 200.0 }, "a trigger already pulled presses nothing");
        assert_eq!(ray.update(Some((300.0, 200.0)), false, ms(10)), Gesture::Hover { x: 300.0, y: 200.0 });
        assert_eq!(ray.update(Some((302.0, 200.0)), true, ms(20)), Gesture::Press { x: 302.0, y: 200.0 });
        assert_eq!(ray.update(Some((900.0, 500.0)), true, ms(30)), Gesture::Hold { x: 900.0, y: 500.0 }, "held wherever it points");
        assert_eq!(ray.update(Some((900.0, 500.0)), false, ms(40)), Gesture::Hover { x: 900.0, y: 500.0 }, "let go");
        assert_eq!(ray.update(Some((-5.0, 500.0)), false, ms(50)), Gesture::Away);
        assert_eq!(ray.update(Some((-5.0, 500.0)), true, ms(55)), Gesture::Away, "off the screen: no press");
        assert_eq!(ray.update(Some((5.0, 500.0)), true, ms(60)), Gesture::Hover { x: 5.0, y: 500.0 }, "still the same pull");
        assert_eq!(ray.update(Some((5.0, 500.0)), false, ms(62)), Gesture::Hover { x: 5.0, y: 500.0 });
        // Lost a moment: it stays; longer: gone.
        assert_eq!(ray.update(None, false, ms(70)), Gesture::Hover { x: 5.0, y: 500.0 });
        assert_eq!(ray.tick(ms(70) + LOST_GRACE), Some(Gesture::Away));
        assert_eq!(ray.tick(ms(70) + LOST_GRACE * 2), None);
    }

    #[test]
    fn pointing_keeps_working_as_the_hand_moves_around_the_screen() {
        for x in [-0.6, -0.3, 0.0, 0.3, 0.6] {
            for y in [-0.5, -0.2, 0.0, 0.2, 0.5] {
                for z in [-1.0, -0.6, -0.3, -0.1] {
                    let tip = [x, y, z];
                    for target in [[0.0, 0.0, 0.0], [-0.2, 0.11, 0.0], [0.2, -0.11, 0.0]] {
                        let delta: [f64; 3] = [0, 1, 2].map(|i| target[i] - tip[i]);
                        let length = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
                        let aim = [0, 1, 2].map(|i| tip[i] + delta[i] / length * orb::AIM_DISTANCE);
                        let found = aimed(read_far(tip, orb::POINT_RADIUS, REACH), read_far(aim, orb::POINT_RADIUS, REACH))
                            .unwrap_or_else(|| panic!("lost {tip:?} pointing at {target:?}"));
                        assert!(near(found, on_page(target[0], target[1])), "{tip:?} at {target:?}: {found:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_trigger_pull_survives_a_brief_incomplete_contact_reading() {
        let now = Instant::now();
        let mut ray = Ray::default();
        ray.update(Some((300.0, 200.0)), false, now);
        ray.update(None, false, now + Duration::from_millis(10));
        let press = ray.update(None, true, now + Duration::from_millis(20));
        let recovered = ray.update(Some((302.0, 200.0)), true, now + Duration::from_millis(30));
        assert!(matches!(press, Gesture::Press { .. }) || matches!(recovered, Gesture::Press { .. }), "a short reading gap swallowed the click");
        assert!(!matches!(press, Gesture::Press { .. }) || !matches!(recovered, Gesture::Press { .. }), "one pull cannot click twice");
    }

    #[test]
    fn a_pending_pull_is_cancelled_on_release_loss_or_leaving_the_page() {
        for scenario in 0..3 {
            let now = Instant::now();
            let mut ray = Ray::default();
            ray.update(Some((300.0, 200.0)), false, now);
            ray.update(None, true, now + Duration::from_millis(10));
            let recovered = match scenario {
                0 => ray.update(Some((300.0, 200.0)), false, now + Duration::from_millis(20)),
                1 => ray.update(Some((300.0, 200.0)), true, now + LOST_GRACE * 2),
                _ => ray.update(Some((-10.0, 200.0)), true, now + Duration::from_millis(20)),
            };
            assert!(!matches!(recovered, Gesture::Press { .. } | Gesture::Hold { .. }));
            let still_held = ray.update(Some((300.0, 200.0)), scenario != 0, now + LOST_GRACE * 3);
            assert!(!matches!(still_held, Gesture::Press { .. } | Gesture::Hold { .. }));
        }
    }

    #[test]
    fn a_touch_wins_over_the_ray() {
        let hover = Some(Gesture::Hover { x: 1.0, y: 1.0 });
        let press = Some(Gesture::Press { x: 2.0, y: 2.0 });
        let ray_hover = Some(Gesture::Hover { x: 3.0, y: 3.0 });
        assert_eq!(either(press, ray_hover), press);
        assert_eq!(either(hover, Some(Gesture::Press { x: 3.0, y: 3.0 })), Some(Gesture::Press { x: 3.0, y: 3.0 }), "a trigger press over a near finger");
        assert_eq!(either(hover, ray_hover), hover, "the fingertip at the screen");
        assert_eq!(either(Some(Gesture::Away), ray_hover), ray_hover);
        assert_eq!(either(Some(Gesture::Away), None), Some(Gesture::Away));
        assert_eq!(either(None, None), None);
    }
}
