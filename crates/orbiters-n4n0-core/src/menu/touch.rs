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

use orbiters_contracts::n4n0;
use std::time::{Duration, Instant};

/// How long a finger may go unfound (a reading VRChat's broad phase dropped for a few frames, a hand moving fast) before it
/// counts as gone: shorter gaps change nothing, so they never let go of a press nor start another.
pub const LOST_GRACE: Duration = Duration::from_millis(250);
/// How long the direction a finger last pointed along stands in for a sender VRChat stopped reporting, the other one of
/// that hand still reported: a blink of a sender (`Docs/n4n0.md`, Pointing, why the contacts have a room of their own) leaves the
/// ray where the hand is, and an avatar uploaded before that room keeps most of its presses.
pub const DIRECTION_KEPT: Duration = Duration::from_millis(1200);

/// The screen's size.
pub const WIDTH: f64 = 0.48;
pub const HEIGHT: f64 = 0.27;
/// The receivers' radius: the eyes', the gaze point's, everyone else's fingertips' (and every finger's before version 7).
pub const RADIUS: f64 = 0.8;
/// The owner's fingertips' and aims' receivers' radius (screen version 7): far enough to point from across a room.
pub const REACH: f64 = n4n0::POINTING_REACH;
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
/// A finger that reached the face without coming from in front may press once it is this far in front. Nearer to the face
/// than the release: a poke that went in beside the screen, or while the screen moved, asks for 6 mm back and not 13.
pub const ARM_DEPTH: f64 = -0.018;
/// Nearer than this it hovers.
pub const HOVER_DEPTH: f64 = -0.08;
/// The tip of a finger reaching through the screen still points along a line that meets it: the ray lasts this far past the
/// face, 5 cm, not 1. The screen hangs 0.45 m before the eyes, so a hand reaches through it without meaning anything.
pub const RAY_PAST_FACE: f64 = 0.05;
/// How far a reading of a sender may disagree with a position before it is no position: readings of one frame agree within
/// 5 microns (measured on real recordings, `reports/menu-hands.md`), and two frames of a moving hand by centimetres.
const SAME_FRAME: f64 = 0.003;
/// A contact's largest radius, whatever its scale: VRChat clamps it (`ContactBase`: `maxSize` 6, half of it the radius).
const LARGEST_CONTACT_RADIUS: f64 = 3.0;

/// The owner's senders and receivers as they are now, in the screen's own space. The screen's size scales the receivers and
/// not the senders, so the senders look smaller on a larger screen; the avatar's scale changes nothing, until the receivers
/// reach VRChat's largest contact, which then measures less far than they should.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// The fingertips' and aims' receivers' radius.
    pub reach: f64,
    /// The senders' radius.
    pub sender: f64,
    /// From the tip to the aim, along the finger.
    pub aim: f64,
}

impl Geometry {
    /// The prop as built, at the screen's size 1.
    pub const BUILT: Geometry = Geometry { reach: REACH, sender: n4n0::POINT_RADIUS, aim: n4n0::AIM_DISTANCE };

    /// `size`: how many times its own the screen is (0.6 to 1.8). `eye_height`: `/avatar/eyeheight` in metres, when known.
    pub fn of(size: f64, eye_height: Option<f64>) -> Geometry {
        let size = if size > 0.0 { size } else { 1.0 };
        let scale = eye_height.filter(|h| *h > 0.0).map_or(1.0, |h| h / 1.6) * size;
        Geometry { reach: REACH.min(LARGEST_CONTACT_RADIUS / scale), sender: n4n0::POINT_RADIUS / size, aim: n4n0::AIM_DISTANCE / size }
    }
}

/// What a sender's four readings say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sender {
    /// Found.
    Found,
    /// All four at 0: VRChat reports no contact at all, out of reach or dropped by its broad phase (never a fit's fault).
    Silent,
    /// Two or three at 0: too far from the receivers on one side to tell where.
    Partial,
    /// They do not fit one position: readings of two frames, or the screen moved between them.
    Mixed,
}

impl Sender {
    pub fn name(self) -> &'static str {
        match self {
            Sender::Found => "found",
            Sender::Silent => "silent",
            Sender::Partial => "partial",
            Sender::Mixed => "mixed",
        }
    }
}

/// Why a hand's two readings give no point on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guard {
    Tip(Sender),
    Aim(Sender),
    /// The two are found, and not as far apart as the finger is long: not one pose.
    WrongLength,
    /// The tip is further than [`RAY_PAST_FACE`] past the screen's face.
    TipPast,
    /// The finger points away from the screen, or along it.
    Parallel,
}

impl Guard {
    pub fn name(self) -> String {
        match self {
            Guard::Tip(s) => format!("tip {}", s.name()),
            Guard::Aim(s) => format!("aim {}", s.name()),
            Guard::WrongLength => "wrong length".into(), // i18n: not shown
            Guard::TipPast => "tip past the face".into(), // i18n: not shown
            Guard::Parallel => "points away or along".into(), // i18n: not shown
        }
    }
}

/// A sender's centre from its four readings, its radius known. Subtracting the squared distances gives three linear
/// equations in the centre alone, with the same matrix wherever the hand is: no root to choose and no place where it
/// degrades. The free radius this replaces was singular on a surface through the space right behind the screen, where a
/// finger's aim point passes at 10 to 30 cm from the screen, and it turned 5 microns of noise into 11 mm there.
fn found(proximity: [f32; 4], geometry: &Geometry) -> Result<[f64; 3], Sender> {
    if proximity.iter().any(|p| !p.is_finite()) {
        return Err(Sender::Mixed);
    }
    let zero = proximity.iter().filter(|p| **p <= 0.0).count();
    match zero {
        4 => return Err(Sender::Silent),
        2 | 3 => return Err(Sender::Partial),
        _ => {}
    }
    let d = proximity.map(|p| geometry.reach * (1.0 - p as f64) + geometry.sender);
    let at = if zero == 0 {
        let c0 = RECEIVERS[0];
        let square = |a: [f64; 3]| a[0] * a[0] + a[1] * a[1] + a[2] * a[2];
        let matrix = [1, 2, 3].map(|i| [0, 1, 2].map(|axis| RECEIVERS[i][axis] - c0[axis]));
        solve(matrix, [1, 2, 3].map(|i| (square(RECEIVERS[i]) - square(c0) - d[i] * d[i] + d[0] * d[0]) * 0.5)).ok_or(Sender::Mixed)?
    } else {
        // Out of reach of one receiver: three spheres meet at two mirrored points; it is the one out of reach of the fourth.
        let out = proximity.iter().position(|p| *p <= 0.0).unwrap_or(0);
        three(out, d, geometry)?
    };
    let fits = RECEIVERS.iter().zip(d).enumerate().filter(|(i, _)| zero == 0 || proximity[*i] > 0.0).all(|(i, (c, distance))| {
        let along = ((at[0] - c[0]).powi(2) + (at[1] - c[1]).powi(2) + (at[2] - c[2]).powi(2)).sqrt();
        // At a receiver's very centre its reading stops at 1 and says only "this near".
        (along - distance).abs() <= SAME_FRAME || (proximity[i] >= 1.0 && along <= distance + SAME_FRAME)
    });
    if fits {
        Ok(at)
    } else {
        Err(Sender::Mixed)
    }
}

/// Where three spheres meet (all but receiver `out`), on the side where receiver `out` is out of reach.
fn three(out: usize, d: [f64; 4], geometry: &Geometry) -> Result<[f64; 3], Sender> {
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let minus = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let kept: Vec<usize> = (0..4).filter(|i| *i != out).collect();
    let (p, q, r) = (RECEIVERS[kept[0]], RECEIVERS[kept[1]], RECEIVERS[kept[2]]);
    let (du, dv) = (minus(q, p), minus(r, p));
    let normal = cross(du, dv);
    let n2 = dot(normal, normal);
    if n2 < 1e-12 {
        return Err(Sender::Mixed);
    }
    let (a, b, c) = (d[kept[0]], d[kept[1]], d[kept[2]]);
    let (s, t) = ((a * a - b * b + dot(du, du)) * 0.5, (a * a - c * c + dot(dv, dv)) * 0.5);
    // In the plane of the three: p + u du + v dv, with u du·du + v du·dv = s and u du·dv + v dv·dv = t.
    let (uu, uv, vv) = (dot(du, du), dot(du, dv), dot(dv, dv));
    let det = uu * vv - uv * uv;
    if det.abs() < 1e-12 {
        return Err(Sender::Mixed);
    }
    let (u, v) = ((s * vv - t * uv) / det, (t * uu - s * uv) / det);
    let base = [0, 1, 2].map(|i| du[i] * u + dv[i] * v);
    let height2 = a * a - dot(base, base);
    if height2 < -SAME_FRAME {
        return Err(Sender::Mixed);
    }
    let height = height2.max(0.0).sqrt() / n2.sqrt();
    let candidates = [1.0, -1.0].map(|side: f64| [0, 1, 2].map(|i| p[i] + base[i] + normal[i] * height * side));
    let c0 = RECEIVERS[out];
    let beyond = |x: [f64; 3]| dot(minus(x, c0), minus(x, c0)).sqrt() - geometry.reach - geometry.sender;
    let first_beyond = beyond(candidates[0]);
    let second_beyond = beyond(candidates[1]);
    match (first_beyond >= -SAME_FRAME, second_beyond >= -SAME_FRAME) {
        (true, false) => Ok(candidates[0]),
        (false, true) => Ok(candidates[1]),
        // Either side would do: the person stands in front of the screen.
        (true, true) => Ok(if candidates[0][2] <= candidates[1][2] { candidates[0] } else { candidates[1] }),
        (false, false) => Err(Sender::Mixed),
    }
}

/// Where the screen's middle is from the owner's eyes, in a frame at the eyes that looks along the line of sight and stays level:
/// right, up, ahead, in metres at 1.6 m eye height, what `N4N0/Screen/PlaceX` to `Z` say (`Docs/n4n0.md`, What others see). `size`:
/// how many times its own the screen is. `None` when the eyes or the gaze point are not found. The screen stands upright, so its
/// up is the world's: the level frame needs nothing the head's roll could spoil.
pub fn place_from_eyes(eyes: [f32; 4], gaze: [f32; 4], size: f64) -> Option<[f64; 3]> {
    let size = if size > 0.0 { size } else { 1.0 };
    let geometry = Geometry { reach: RADIUS, sender: 0.01 / size, aim: 0.0 };
    let (e, g) = (found(eyes, &geometry).ok()?, found(gaze, &geometry).ok()?);
    let delta = [g[0] - e[0], g[1] - e[1], g[2] - e[2]];
    let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    if length < 1e-4 {
        return None;
    }
    let ahead = delta.map(|v| v / length);
    // right = up x ahead, up being the screen's own.
    let side = (ahead[2] * ahead[2] + ahead[0] * ahead[0]).sqrt();
    if side < 0.05 {
        return None;
    }
    let right = [ahead[2] / side, 0.0, -ahead[0] / side];
    let up = [ahead[1] * right[2] - ahead[2] * right[1], ahead[2] * right[0] - ahead[0] * right[2], ahead[0] * right[1] - ahead[1] * right[0]];
    let middle = e.map(|v| -v);
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    Some([dot(middle, right) * size, dot(middle, up) * size, dot(middle, ahead) * size])
}

/// The owner's fingertip from its four readings, on the page and in depth, what a touch reads.
pub fn fingertip(proximity: [f32; 4], geometry: &Geometry) -> Result<Finger, Sender> {
    let [x, y, depth] = found(proximity, geometry)?;
    let (x, y) = on_page(x, y);
    Ok(Finger { x, y, depth })
}

/// A hand's two senders at a reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub tip: Sender,
    pub aim: Sender,
    /// Where the finger points on the page, or why it is not known.
    pub point: Result<(f32, f32), Guard>,
    /// The point stands on one sender and the direction the finger last pointed along.
    pub kept: bool,
}

/// A hand's senders over time: the direction it last pointed along, to ride out a sender VRChat stops reporting.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pointer {
    along: Option<([f64; 3], Instant)>,
}

impl Pointer {
    pub fn read(&mut self, tip: [f32; 4], aim: [f32; 4], geometry: &Geometry, now: Instant) -> Reading {
        let (t, a) = (found(tip, geometry), found(aim, geometry));
        let (tip_state, aim_state) = (t.err().unwrap_or(Sender::Found), a.err().unwrap_or(Sender::Found));
        let kept = self.along.filter(|(_, at)| now.duration_since(*at) <= DIRECTION_KEPT).map(|(d, _)| d);
        let (from, to, standing) = match (t, a, kept) {
            (Ok(t), Ok(a), _) => (t, a, false),
            (Ok(t), Err(_), Some(d)) => (t, [0, 1, 2].map(|i| t[i] + d[i] * geometry.aim), true),
            (Err(_), Ok(a), Some(d)) => ([0, 1, 2].map(|i| a[i] - d[i] * geometry.aim), a, true),
            (Err(s), _, _) => return Reading { tip: tip_state, aim: aim_state, point: Err(Guard::Tip(s)), kept: false },
            (_, Err(s), _) => return Reading { tip: tip_state, aim: aim_state, point: Err(Guard::Aim(s)), kept: false },
        };
        let delta = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
        let guard = if (length - geometry.aim).abs() > SAME_FRAME * 2.0 {
            Some(Guard::WrongLength)
        } else if delta[2] / length < 0.02 {
            Some(Guard::Parallel)
        } else if from[2] > RAY_PAST_FACE {
            Some(Guard::TipPast)
        } else {
            None
        };
        // A pose of the two senders together: the direction from now on, pointing at the screen or not.
        if !standing && guard != Some(Guard::WrongLength) {
            self.along = Some((delta.map(|v| v / length), now));
        }
        let point = match guard {
            Some(g) => Err(g),
            None => {
                let k = -from[2] / delta[2];
                Ok(on_page(from[0] + delta[0] * k, from[1] + delta[1] * k))
            }
        };
        Reading { tip: tip_state, aim: aim_state, point, kept: standing }
    }
}

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

/// A point of the screen's plane on the page, in pixels.
fn on_page(x: f64, y: f64) -> (f32, f32) {
    (((x + WIDTH / 2.0) / WIDTH * super::layout::WIDTH as f64) as f32, ((HEIGHT / 2.0 - y) / HEIGHT * super::layout::HEIGHT as f64) as f32)
}

/// Where the owner looks on the page in desktop mode (contracts `Docs/n4n0.md`, Looking and clicking), in pixels: the
/// eyes and the gaze point from their receivers' proximities, and where the line from the eyes through the gaze point
/// meets the screen's plane. Eyes not found: the eyes at rest stand in. `None` when the gaze point is not found or the
/// line meets the plane nowhere ahead of the eyes.
pub fn look(eyes: [f32; 4], gaze: [f32; 4]) -> Option<(f32, f32)> {
    let gaze = point(gaze, RADIUS, [0.0, 0.1, -0.1])?;
    looked_at(point(eyes, RADIUS, GAZE_EYES), gaze)
}

/// The eyes at rest, in the screen's space.
const GAZE_EYES: [f64; 3] = orbiters_contracts::n4n0::params::GAZE_EYES;

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

/// A contact's centre from four surface distances when its radius is not known: everyone else's fingertips, and the eyes
/// and gaze point whose radius the desktop mode's lookup does not depend on. Subtracting the squared distance equations
/// makes the centre linear in the unknown sender radius; substituting it back gives a quadratic. `start` selects the
/// nearer solution if both satisfy the physical radius and residual limits. The owner's fingers use [`found`] instead:
/// their radius is known, and this has a surface where the two roots meet and noise turns into centimetres.
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
            // A contact has no negative radius: 2 mm of slack for noise, not the 10 it had, which let the wrong root win.
            if !(-0.002..0.06).contains(&r) {
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

/// A hand's touch: hovering, pressing, letting go. A press needs the finger seen in front of the screen, over it, at the
/// reading before (a hand that comes in from the side or from behind presses nothing until it is seen in front again; a
/// fast poke, nearer than [`HOVER_DEPTH`] only for that one reading, presses), lasts while the finger stays at or past the
/// face however deep it goes, and ends only once it is back in front.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Touch {
    pressing: bool,
    pub finger: Option<Finger>,
    /// The finger's last reading was in front of the screen's face, over it, at whatever distance: what may press next.
    armed: bool,
    /// The last reading was near enough to hover (what a short gap keeps showing).
    hovering: bool,
    /// The finger reached the face without coming from in front: it presses nothing until it is back at [`ARM_DEPTH`].
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
        // A pull counts when the ray pointed at the page, or was lost a moment ago: the reading that comes back says where.
        let pulled = trigger && !self.trigger && (self.over || self.lost_since.is_some());
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
/// then the ray's hover where its dot is drawn and what the trigger would press, then a touch's hover. `poking`: the
/// fingertip is so near the face ([`RELEASE_DEPTH`]) that what is under it is about to be pressed, and what shows is that.
pub fn either(touch: Option<Gesture>, ray: Option<Gesture>, poking: bool) -> Option<Gesture> {
    let pressing = |g: &Option<Gesture>| matches!(g, Some(Gesture::Press { .. } | Gesture::Hold { .. }));
    if pressing(&touch) {
        return touch;
    }
    if pressing(&ray) {
        return ray;
    }
    if poking && matches!(touch, Some(Gesture::Hover { .. })) {
        return touch;
    }
    match (ray, touch) {
        (Some(Gesture::Hover { x, y }), _) => Some(Gesture::Hover { x, y }),
        (_, Some(Gesture::Hover { x, y })) => Some(Gesture::Hover { x, y }),
        (Some(g), _) => Some(g),
        (None, g) => g,
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
        self.feed(locate(proximity, radius), true, now)
    }

    /// The finger as found (`None`: not found), at `now`. `still`: the screen is not moving by itself: a screen that glides
    /// onto a resting finger is no press, and arms nothing until it stops.
    pub fn feed(&mut self, finger: Option<Finger>, still: bool, now: Instant) -> Gesture {
        let Some(f) = finger else { return self.unfound(now) };
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
            if inside && self.armed && !self.blocked && still {
                self.pressing = true;
                self.armed = false;
                self.hovering = false;
                return Gesture::Press { x: f.x, y: f.y };
            }
            self.blocked = true;
            self.armed = false;
            self.hovering = false;
            return if inside && f.depth <= 0.08 { Gesture::Hover { x: f.x, y: f.y } } else { Gesture::Away };
        }
        // In front of the face: hovering when over the screen and near enough. A blocked finger is free once it is
        // back in front.
        if f.depth < ARM_DEPTH {
            self.blocked = false;
        }
        self.armed = inside && still;
        self.hovering = inside && f.depth >= HOVER_DEPTH;
        if self.hovering {
            Gesture::Hover { x: f.x, y: f.y }
        } else {
            Gesture::Away
        }
    }

    /// The fingertip is so near the face that what is under it is about to be pressed.
    pub fn poking(&self) -> bool {
        self.finger.is_some_and(|f| f.depth >= RELEASE_DEPTH)
    }

    /// The touch refuses a press until the finger is back in front ([`ARM_DEPTH`]): for the readout of what stops a press.
    pub fn blocked(&self) -> bool {
        self.blocked
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

    /// Where a finger points by the two readings alone.
    fn aimed(tip: [f32; 4], aim: [f32; 4]) -> Option<(f32, f32)> {
        Pointer::default().read(tip, aim, &Geometry::BUILT, Instant::now()).point.ok()
    }

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
        assert!(!matches!(touch.update_at(read([0.0, 0.0, -0.016], 0.01), clock.next()), Gesture::Press { .. }), "not clearly out yet");
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
                let aim = [0, 1, 2].map(|i| tip[i] + along[i] / length * n4n0::AIM_DISTANCE);
                let r = n4n0::POINT_RADIUS;
                let found = aimed(read_far(tip, r, REACH), read_far(aim, r, REACH)).unwrap_or_else(|| panic!("{tip:?} at {target:?}"));
                let wanted = on_page(target[0], target[1]);
                // Within 1 mm on the screen (float proximities).
                let mm = super::super::layout::WIDTH as f32 / (WIDTH as f32 * 1000.0);
                assert!((found.0 - wanted.0).abs() < mm && (found.1 - wanted.1).abs() < mm, "{tip:?} at {target:?}: {found:?} vs {wanted:?}");
            }
        }
        // Pointing away, along the screen, or from behind it: nowhere.
        let r = n4n0::POINT_RADIUS;
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
                        let aim = [0, 1, 2].map(|i| tip[i] + delta[i] / length * n4n0::AIM_DISTANCE);
                        let found = aimed(read_far(tip, n4n0::POINT_RADIUS, REACH), read_far(aim, n4n0::POINT_RADIUS, REACH))
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
        assert_eq!(either(press, ray_hover, false), press);
        assert_eq!(either(hover, Some(Gesture::Press { x: 3.0, y: 3.0 }), false), Some(Gesture::Press { x: 3.0, y: 3.0 }), "a trigger press over a near finger");
        assert_eq!(either(hover, ray_hover, true), hover, "the fingertip at the screen");
        assert_eq!(either(hover, ray_hover, false), ray_hover, "the dot it draws is what the trigger would press");
        assert_eq!(either(hover, Some(Gesture::Away), false), hover, "pointing off the page with the fingertip over it");
        assert_eq!(either(Some(Gesture::Away), ray_hover, false), ray_hover);
        assert_eq!(either(Some(Gesture::Away), None, false), Some(Gesture::Away));
        assert_eq!(either(None, None, false), None);
    }

    /// What the receivers read for a finger of the prop as built, with `noise` metres of error on each reading.
    fn reads(f: [f64; 3], g: &Geometry, noise: f64, seed: &mut u64) -> [f32; 4] {
        let mut out = [0f32; 4];
        for (o, c) in out.iter_mut().zip(RECEIVERS) {
            *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let wobble = ((*seed >> 33) as f64 / (1u64 << 31) as f64 - 0.5) * 2.0 * noise;
            let d = ((f[0] - c[0]).powi(2) + (f[1] - c[1]).powi(2) + (f[2] - c[2]).powi(2)).sqrt() - g.sender + wobble;
            *o = (1.0 - d / g.reach).clamp(0.0, 1.0) as f32;
        }
        out
    }

    /// Every target of the screen from a fingertip 0 to 50 cm from it, from where each hand is, with what real readings carry
    /// (5 microns, `reports/menu-hands.md`): always found, and within 3 pixels of where the finger points. The free radius this
    /// replaced lost or moved up to 9 % of these near the surface where its two roots meet, right behind the screen.
    #[test]
    fn every_target_is_found_at_any_distance_within_half_a_metre() {
        let g = Geometry::BUILT;
        let mut seed = 7u64;
        for shoulder in [[-0.2, -0.3, -0.45], [0.2, -0.3, -0.45], [0.0, 0.1, -0.5], [-0.5, 0.0, -0.3], [0.5, 0.0, -0.3]] {
            for tenth in 0..=10 {
                let dist = 0.5 * tenth as f64 / 10.0;
                for py in 0..18 {
                    for px in 0..32 {
                        let target = [(px as f64 + 0.5) / 32.0 * WIDTH - WIDTH / 2.0, HEIGHT / 2.0 - (py as f64 + 0.5) / 18.0 * HEIGHT, 0.0];
                        let delta: [f64; 3] = [0, 1, 2].map(|i| target[i] - shoulder[i]);
                        let length = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
                        let along = delta.map(|v| v / length);
                        let tip = [0, 1, 2].map(|i| target[i] - along[i] * dist);
                        let aim = [0, 1, 2].map(|i| tip[i] + along[i] * g.aim);
                        let r = Pointer::default().read(reads(tip, &g, 5e-6, &mut seed), reads(aim, &g, 5e-6, &mut seed), &g, Instant::now());
                        let (x, y) = r.point.unwrap_or_else(|e| panic!("lost at {dist} m, {px} {py}: {e:?}"));
                        let (wx, wy) = on_page(target[0], target[1]);
                        assert!((x - wx).abs() < 3.0 && (y - wy).abs() < 3.0, "{dist} m, {px} {py}: {x} {y} vs {wx} {wy}");
                        assert!(!r.kept);
                    }
                }
            }
        }
    }

    /// The reading at the singular place of the real recording (After The Rain, 3.0 s: the aim point 11 cm from the receiver
    /// behind the screen, the hand still): the free radius said -9 mm and put it 11 mm from where the known radius does.
    #[test]
    fn the_aim_near_the_receiver_behind_the_screen_is_where_it_is() {
        let g = Geometry::BUILT;
        let mut seed = 11u64;
        for aim in [[0.026, -0.1, 0.17], [0.03, -0.104, 0.184], [0.0, 0.0, 0.17], [0.0, 0.01, 0.152], [-0.289, 0.184, 0.002], [0.289, 0.185, -0.001]] {
            let [x, y, z] = found(reads(aim, &g, 5e-6, &mut seed), &g).unwrap_or_else(|e| panic!("{aim:?}: {e:?}"));
            assert!((x - aim[0]).abs() < 2e-4 && (y - aim[1]).abs() < 2e-4 && (z - aim[2]).abs() < 2e-4, "{aim:?}: {x} {y} {z}");
        }
    }

    /// An example of a reading of two frames: the fit took it for a place the finger never was. It is no place now.
    #[test]
    fn readings_of_two_frames_are_no_position() {
        let g = Geometry::BUILT;
        let mut seed = 3u64;
        let before = reads([0.15, -0.08, -0.04], &g, 0.0, &mut seed);
        let after = reads([0.15, -0.08, -0.02], &g, 0.0, &mut seed);
        assert!(found(before, &g).is_ok() && found(after, &g).is_ok());
        assert_eq!(found([before[0], before[1], before[2], after[3]], &g), Err(Sender::Mixed));
        assert_eq!(found([after[0], after[1], after[2], before[3]], &g), Err(Sender::Mixed));
    }

    /// VRChat's broad phase drops a whole sender (all four readings at 0): the other one and the direction the finger last
    /// pointed along keep the ray going for a moment, said as kept, and the guard names which went.
    #[test]
    fn a_silent_sender_rides_on_the_other_one_for_a_moment_and_is_named() {
        let g = Geometry::BUILT;
        let mut seed = 5u64;
        let t0 = Instant::now();
        let tip = [0.1, -0.1, -0.2];
        let along = [-0.05f64, 0.1, 0.2817];
        let length = along.iter().map(|v| v * v).sum::<f64>().sqrt();
        let aim = [0, 1, 2].map(|i| tip[i] + along[i] / length * g.aim);
        let mut p = Pointer::default();
        assert_eq!(p.read([0.0; 4], reads(aim, &g, 0.0, &mut seed), &g, t0).point, Err(Guard::Tip(Sender::Silent)), "nothing known of its direction yet");
        let both = p.read(reads(tip, &g, 0.0, &mut seed), reads(aim, &g, 0.0, &mut seed), &g, t0).point.unwrap();
        let r = p.read(reads(tip, &g, 0.0, &mut seed), [0.0; 4], &g, t0 + Duration::from_millis(30));
        assert_eq!((r.tip, r.aim, r.kept), (Sender::Found, Sender::Silent, true));
        let kept = r.point.unwrap();
        assert!((kept.0 - both.0).abs() < 2.0 && (kept.1 - both.1).abs() < 2.0, "{kept:?} {both:?}");
        let r = p.read([0.0; 4], reads(aim, &g, 0.0, &mut seed), &g, t0 + Duration::from_millis(60));
        assert_eq!((r.tip, r.aim, r.kept), (Sender::Silent, Sender::Found, true));
        assert!(r.point.is_ok());
        let r = p.read(reads(tip, &g, 0.0, &mut seed), [0.0; 4], &g, t0 + DIRECTION_KEPT * 2);
        assert_eq!(r.point, Err(Guard::Aim(Sender::Silent)), "too long ago");
        assert_eq!(p.read([0.0; 4], [0.0; 4], &g, t0).point, Err(Guard::Tip(Sender::Silent)));
    }

    /// Out of the receiver behind the screen's reach, the three others still say where the finger is: the beam's reach is 2.5 m.
    #[test]
    fn a_fingertip_out_of_one_receiver_s_reach_is_still_found() {
        let g = Geometry::BUILT;
        let mut seed = 9u64;
        for tip in [[0.0, 0.0, -1.5], [0.2, -0.1, -1.46], [-0.1, 0.1, -1.49]] {
            let read = reads(tip, &g, 0.0, &mut seed);
            assert_eq!(read[3], 0.0, "{tip:?}");
            let [x, y, z] = found(read, &g).unwrap_or_else(|e| panic!("{tip:?}: {e:?}"));
            assert!((x - tip[0]).abs() < 1e-3 && (y - tip[1]).abs() < 1e-3 && (z - tip[2]).abs() < 1e-3, "{tip:?}: {x} {y} {z}");
        }
        assert_eq!(found(reads([0.0, 0.0, -3.0], &g, 0.0, &mut seed), &g), Err(Sender::Silent));
    }

    /// The screen's size scales the receivers, not the senders; an avatar large enough reaches VRChat's largest contact.
    #[test]
    fn the_geometry_follows_the_screen_s_size_and_vrchat_s_largest_contact() {
        assert_eq!(Geometry::of(1.0, None), Geometry::BUILT);
        let g = Geometry::of(1.8, Some(1.6));
        assert_eq!(g.reach, REACH, "1.6 m x 1.8 is under 3 m");
        assert!((g.sender - n4n0::POINT_RADIUS / 1.8).abs() < 1e-9 && (g.aim - n4n0::AIM_DISTANCE / 1.8).abs() < 1e-9);
        let big = Geometry::of(1.8, Some(1.8));
        assert!((big.reach - 3.0 / (1.8 / 1.6 * 1.8)).abs() < 1e-9, "1.6 m x 1.125 x 1.8 is clamped to 3 m: {big:?}");
        let mut seed = 1u64;
        let target = [0.1, 0.05, -0.2];
        let [x, y, z] = found(reads(target, &big, 0.0, &mut seed), &big).unwrap();
        assert!((x - target[0]).abs() < 1e-3 && (y - target[1]).abs() < 1e-3 && (z - target[2]).abs() < 1e-3);
    }

    /// A poke between two readings presses; a hand that was in front does not need to be seen hovering first.
    #[test]
    fn a_fast_poke_presses_and_the_way_back_is_short() {
        let g = Geometry::BUILT;
        let mut seed = 2u64;
        let mut touch = Touch::default();
        let mut clock = Clock(Instant::now());
        let mut at = |touch: &mut Touch, z: f64, still: bool, clock: &mut Clock| {
            touch.feed(fingertip(reads([0.0, 0.0, z], &g, 0.0, &mut seed), &g).ok(), still, clock.next())
        };
        assert_eq!(at(&mut touch, -0.25, true, &mut clock), Gesture::Away);
        assert!(matches!(at(&mut touch, -0.005, true, &mut clock), Gesture::Press { .. }), "from a quarter of a metre in one reading");
        assert!(matches!(at(&mut touch, -0.02, true, &mut clock), Gesture::Hold { .. }));
        assert!(matches!(at(&mut touch, -0.04, true, &mut clock), Gesture::Hover { .. }));
        // A screen that glides onto a resting finger presses nothing, and the finger is not blocked for long.
        assert!(matches!(at(&mut touch, -0.04, false, &mut clock), Gesture::Hover { .. }));
        assert!(matches!(at(&mut touch, -0.005, false, &mut clock), Gesture::Hover { .. }));
        assert!(touch.blocked());
        assert!(matches!(at(&mut touch, -0.019, true, &mut clock), Gesture::Hover { .. }));
        assert!(!touch.blocked(), "6 mm back");
        assert!(matches!(at(&mut touch, -0.005, true, &mut clock), Gesture::Press { .. }));
    }
}
