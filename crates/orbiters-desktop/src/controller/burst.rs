//! 3D picture: a short recording during which VRChat's camera (`/usercamera/Pose`, VRChat 2025.3.3 and later) moves
//! on its own, then comes back where it was. The capture kit stamps every frame with the camera's pose, so the session
//! reconstructs into splats like any recording; the reconstruction starts as soon as the recording is saved. Three
//! lengths ([`Photo3dLength`]): a gentle 1 s sweep around what the camera aims at; 4 s that circle the starting point
//! looking at it (the person taking the picture) then turn in place looking straight up and down and fly 10 m forward,
//! to each side and up and back; 8 s that add four circles around points 8 m away.
//!
//! N4N0's camera rig (contracts' Docs/rig-frames.md) follows the camera, from the avatar's hips, and is on for the
//! whole flight (controller/rig.rs): about every other colour frame then carries six more views, one along each axis
//! around it.
//!
//! The 4 s and 8 s pictures first take close-ups ([`closeups`]): for about a second, before it flies, the camera stays
//! where it stands, zoomed in (`/usercamera/Zoom`) and turned over what it aims at, nine views that together cover the
//! picture's own framing at 2.35 times its pixels a degree, the rig off so each frame is a whole picture. Why: in the
//! 3D pictures Enzo took with friends on 2026-11-03 (Penthouse Palace, 4 and 8 s), the faces came out mushy because
//! the people moved while the camera flew (the start and the end frame, from the same place 4.3 s apart, differ by
//! 15 dB on them), only the first keyframe saw them as framed, and the model blurred their poses together (rendered at
//! that very keyframe, half its sharpness). The close-ups show them within a second, at more than twice the
//! resolution, and the reconstruction learns the picture's subject from them (orbiters_reconstruct's detail views:
//! never held out, a quarter of the colour steps on top): emulated on those pictures, the people in the start view went
//! from 22 to 36 dB (8 s) and 22 to 28 dB (4 s), the desktop repository's docs/3d-picture-close-ups-2026-11-03.md.
//! Every frame carries its exact pose and projection in its strip, zoomed or not, so nothing about VRChat's zoom has
//! to be assumed but which way to turn. VRChat 2026.4.1 neither sends nor lists `/usercamera/Zoom` (none of Enzo's picture's
//! frames of that night was zoomed: there were no close-ups), so the zoom now comes from the first frame's own projection.
//!
//! The same frame shows who is in the picture (`orbiters-reconstruct --find-faces`): the camera then goes to see up to
//! three faces closely, and the wearer's when they stand near, from three sides and above ([`face_steps`]). A face in a
//! 3D picture came out mushy above all for lack of steps and of angles: the reconstruction's extra training on faces
//! (orbiters_reconstruct::portraits) answers the first, these views the second, and they need no zoom. Measured in a
//! simulation of Enzo's capture: 2 dB more on his face from other places (docs/3d-picture-faces-2026-11-04.md).

use super::{NoticeKind, Shared};
use crate::state::CaptureState;
use orbiters_contracts::rig;
use orbiters_core::osc::{OscValue, PARAMETERS_PREFIX};
use orbiters_core::settings::Photo3dLength;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const POSE: &str = "/usercamera/Pose";
const FOCAL_DISTANCE: &str = "/usercamera/FocalDistance";
const ZOOM: &str = "/usercamera/Zoom";
pub(super) const SMOOTH: &str = "/usercamera/SmoothMovement";
/// How far the flight goes from where the camera stands, each way.
pub const REACH_M: f32 = 10.0;
/// Straight up or down, short of the pole where yaw means nothing.
const STRAIGHT: f32 = 88.0;
/// Poses a second.
const RATE_HZ: u32 = 90;
/// After the recording started, before the camera moves: Spout and the kit's first frames.
const SETTLE: Duration = Duration::from_millis(700);
/// VRChat's camera zoomed in as far as it goes: `/usercamera/Zoom` runs from 20 to 150 (its vertical field of view in
/// degrees; 45 by default).
pub const ZOOM_NARROWEST: f32 = 20.0;
/// How many times the close-ups magnify the picture's framing (the tangent of their half field of view against the
/// start's), at most: VRChat's default 45° zoomed to 20° is 2.35. Three rows of them then overlap by 30 % of their
/// height, so the band of each lower row's frames that the capture strip covers (6.7 to 22 % from their top,
/// contracts' Docs/capture-strip.md: training leaves it out) is seen clear in the row above.
const MAGNIFY: f32 = 2.4;
/// Less magnification than this adds too little to be worth the second (a camera already zoomed in): no close-ups.
/// Also what keeps the rows apart by at least the strip's 15 % of a frame's height (they are `(magnification − 1) / 2`
/// of it apart), so that a lower row's strip band never falls on the row above's own; [`MAGNIFY`] keeps them under the
/// 73 % beyond which it would fall below the row above.
const MAGNIFY_LEAST: f32 = 1.35;
/// How long the camera holds each close-up, the first one longer (VRChat zooms in meanwhile), and how long it then
/// stands zoomed back out before it flies. VRChat's camera streamed about 34 frames a second in the recordings of
/// 2026-11-03 (29 ms apart, 33 ms at the 90th percentile), a quarter of them depth frames: 0.1 s is three frames, at
/// least two colour ones, and the capture keeps the first that shows a close-up as a keyframe (orbiters_core::keyframes:
/// a zoom, or a turn by a third of the view). All told 0.2 + 8 × 0.1 + 0.1 = 1.1 s.
const CLOSEUP_S: f32 = 0.1;
const FIRST_CLOSEUP_S: f32 = 0.2;
const ZOOMED_OUT_S: f32 = 0.1;
/// The stream's frames are 16:9 (VRChat's camera in landscape).
const ASPECT: f32 = 16.0 / 9.0;

/// Eases in and out.
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// The yaw equal to `target` (mod 360) nearest to `from`, so turning never goes the long way round.
fn near(from: f32, target: f32) -> f32 {
    from + (target - from + 180.0).rem_euclid(360.0) - 180.0
}

/// A camera path being built at `rate` poses a second: each piece starts where the last one ended. Poses are VRChat's
/// (x, y, z in metres, then pitch, yaw, roll in degrees, Unity's conventions: +X right, +Y up, +Z forward, a positive
/// pitch looks down, a yaw turns forward towards +X).
struct Path {
    rate: u32,
    poses: Vec<[f32; 6]>,
    start: [f32; 6],
    forward: [f32; 3],
    right: [f32; 3],
}

impl Path {
    fn new(start: [f32; 6], rate: u32) -> Self {
        let w = start[4].to_radians();
        Self { rate, poses: vec![start], start, forward: [w.sin(), 0.0, w.cos()], right: [w.cos(), 0.0, -w.sin()] }
    }

    fn last(&self) -> [f32; 6] {
        *self.poses.last().expect("starts with a pose")
    }

    /// A point `right`, `up` and `forward` metres from the start, as the camera faced then.
    fn local(&self, right: f32, up: f32, forward: f32) -> [f32; 3] {
        let s = self.start;
        [s[0] + self.right[0] * right + self.forward[0] * forward, s[1] + up, s[2] + self.right[2] * right + self.forward[2] * forward]
    }

    fn steps(&self, seconds: f32) -> usize {
        ((seconds * self.rate as f32).round() as usize).max(1)
    }

    /// Moves to `at`, eased, turning to `pitch` and `yaw` (yaw as given: several turns are turned).
    fn move_to(&mut self, at: [f32; 3], pitch: f32, yaw: f32, seconds: f32) {
        let from = self.last();
        let n = self.steps(seconds);
        for i in 1..=n {
            let t = smooth(i as f32 / n as f32);
            let lerp = |a: f32, b: f32| a + (b - a) * t;
            self.poses.push([lerp(from[0], at[0]), lerp(from[1], at[1]), lerp(from[2], at[2]), lerp(from[3], pitch), lerp(from[4], yaw), from[5]]);
        }
    }

    /// Once around `center`, looking at it, rising and dipping `bob` metres twice. With `grow`, the circle grows from
    /// the camera's place to `radius` and back (circling the starting point); without, the camera is on the circle
    /// already (it starts and ends at the point facing the centre with its current heading).
    fn orbit(&mut self, center: [f32; 3], radius: f32, bob: f32, seconds: f32, grow: bool) {
        let from = self.last();
        let n = self.steps(seconds);
        // Facing the centre with the current heading: the camera is on the side the heading points away from.
        let start_angle = from[4] - 180.0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let r = if grow { radius * (1.0 - (std::f32::consts::TAU * t).cos()) / 2.0 } else { radius };
            let h = bob * (2.0 * std::f32::consts::TAU * t).sin();
            let angle = (start_angle + 360.0 * t).to_radians();
            let at = [center[0] + r * angle.sin(), center[1] + h, center[2] + r * angle.cos()];
            // Looking at the centre: down when above it (a short distance keeps it from flipping straight down).
            let pitch = (at[1] - center[1]).atan2(r.max(0.8)).to_degrees();
            self.poses.push([at[0], at[1], at[2], pitch, from[4] + 360.0 * t, from[5]]);
        }
    }
}

/// The 1 s sweep: ±15° around what the camera aims at, `focus` metres ahead, rising a little.
fn sweep(path: &mut Path, focus: f32) {
    let [x, y, z, pitch, yaw, roll] = path.start;
    let (p, w) = (pitch.to_radians(), yaw.to_radians());
    let forward = [w.sin() * p.cos(), -p.sin(), w.cos() * p.cos()];
    let subject = [x + forward[0] * focus, y + forward[1] * focus, z + forward[2] * focus];
    let back = [x - subject[0], y - subject[1], z - subject[2]];
    let n = path.steps(1.0);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let angle = (15.0 * (std::f32::consts::TAU * t).sin()).to_radians();
        let lift = 0.12 * (std::f32::consts::PI * t).sin();
        let (s, c) = angle.sin_cos();
        let offset = [back[0] * c + back[2] * s, back[1] + lift, -back[0] * s + back[2] * c];
        let flat = (offset[0] * offset[0] + offset[2] * offset[2]).sqrt();
        let look_yaw = (-offset[0]).atan2(-offset[2]).to_degrees();
        let look_pitch = offset[1].atan2(flat).to_degrees();
        path.poses.push([subject[0] + offset[0], subject[1] + offset[1], subject[2] + offset[2], look_pitch, near(yaw, look_yaw), roll]);
    }
}

/// Three seconds: a turn in place looking straight up then down, out 10 m forward and back looking around, to the right
/// and across to the left, up looking down, and home, three full turns in all.
fn flight(path: &mut Path) {
    // (right, up, forward, turn, pitch, share of the time); the last key keeps the camera's own pitch.
    const KEYS: [(f32, f32, f32, f32, f32, f32); 11] = [
        (0.0, 0.0, 0.0, 120.0, -STRAIGHT, 0.45),
        (0.0, 0.0, 0.0, 240.0, STRAIGHT, 0.45),
        (0.0, 0.0, 0.0, 360.0, 0.0, 0.45),
        (0.0, 0.0, REACH_M, 360.0, 0.0, 0.6),
        (0.0, 0.0, REACH_M, 540.0, 15.0, 0.45),
        (0.0, 0.0, 0.0, 540.0, 0.0, 0.6),
        (REACH_M, 0.0, 0.0, 630.0, 0.0, 0.6),
        (-REACH_M, 0.0, 0.0, 810.0, 0.0, 1.0),
        (0.0, 0.0, 0.0, 900.0, 0.0, 0.6),
        (0.0, REACH_M, 0.0, 990.0, STRAIGHT, 0.7),
        (0.0, 0.0, 0.0, 1080.0, f32::NAN, 0.7),
    ];
    let shares: f32 = KEYS.iter().map(|k| k.5).sum();
    let yaw = path.last()[4];
    for (right, up, forward, turn, pitch, share) in KEYS {
        let at = path.local(right, up, forward);
        let pitch = if pitch.is_nan() { path.start[3] } else { pitch };
        path.move_to(at, pitch, yaw + turn, share / shares * 3.0);
    }
}

/// The camera's poses for a 3D picture of `length`, from `start` (`focus`: how far ahead the camera aims, for the 1 s
/// sweep). The last pose is the first one, maybe some turns later.
pub fn path(start: [f32; 6], length: Photo3dLength, focus: f32, rate: u32) -> Vec<[f32; 6]> {
    let mut path = Path::new(start, rate);
    match length {
        Photo3dLength::Quick => sweep(&mut path, focus),
        Photo3dLength::Standard | Photo3dLength::Full => {
            // Around the person taking the picture first: the camera's own place, about 2 m out.
            let me = [start[0], start[1], start[2]];
            path.orbit(me, 2.0, 0.6, 1.0, true);
            flight(&mut path);
            if length == Photo3dLength::Full {
                // Four circles around points 8 m away, in front, to the right, behind and to the left.
                for (right, forward) in [(0.0, 8.0), (8.0, 0.0), (0.0, -8.0), (-8.0, 0.0)] {
                    let center = path.local(right, 0.0, forward);
                    let heading = path.last()[4];
                    let w = heading.to_radians();
                    let entry = [center[0] - 1.5 * w.sin(), center[1], center[2] - 1.5 * w.cos()];
                    path.move_to(entry, 0.0, heading, 0.25);
                    path.orbit(center, 1.5, 0.6, 0.7, false);
                }
                let home = path.local(0.0, 0.0, 0.0);
                let heading = path.last()[4];
                path.move_to(home, start[3], heading, 0.2);
            }
        }
    }
    let mut poses = path.poses;
    // Exactly where it started (the yaw some turns later).
    if let Some(last) = poses.last_mut() {
        let turns = ((last[4] - start[4]) / 360.0).round();
        *last = [start[0], start[1], start[2], start[3], start[4] + 360.0 * turns, start[5]];
    }
    poses
}

/// The close-ups of a 3D picture taken from `start` with VRChat's camera at `zoom` (its vertical field of view in
/// degrees): the camera where it stands, zoomed to the returned field of view (at most [`MAGNIFY`] times the start's
/// pixels a degree, no narrower than [`ZOOM_NARROWEST`]), turned to each of nine points of its framing in turn: three
/// rows from the top, each row the other way from the last so the camera never swings back across. The rows and
/// columns are spread so that the close-ups together cover the start's view, each lower row's strip band inside the
/// row above. None when they would magnify less than [`MAGNIFY_LEAST`] (the camera is zoomed in already) or the zoom
/// is not a usable one.
pub fn closeups(start: [f32; 6], zoom: f32) -> Option<(f32, Vec<[f32; 6]>)> {
    if !(ZOOM_NARROWEST..=150.0).contains(&zoom) {
        return None;
    }
    let half = |fov: f32| (fov.to_radians() / 2.0).tan();
    let wide = half(zoom);
    let narrow = (wide / MAGNIFY).max(half(ZOOM_NARROWEST));
    if wide / narrow < MAGNIFY_LEAST {
        return None;
    }
    let fov = 2.0 * narrow.atan().to_degrees();
    // Centres in the start view's image plane (tangents): the outer close-ups' edges on the start's.
    let (dy, dx) = (wide - narrow, (wide - narrow) * ASPECT);
    let [right, up, forward] = super::dolly::axes(&start);
    let mut poses = Vec::with_capacity(9);
    for row in 0..3 {
        for k in 0..3 {
            let col = if row % 2 == 0 { k } else { 2 - k };
            let (x, y) = ((col as f32 - 1.0) * dx, (1.0 - row as f32) * dy);
            let d: [f32; 3] = std::array::from_fn(|i| forward[i] + right[i] * x + up[i] * y);
            let flat = (d[0] * d[0] + d[2] * d[2]).sqrt();
            let yaw = near(start[4], d[0].atan2(d[2]).to_degrees());
            let pitch = -d[1].atan2(flat).to_degrees();
            poses.push([start[0], start[1], start[2], pitch, yaw, start[5]]);
        }
    }
    Some((fov, poses))
}

/// The close-ups of [`closeups`] as steps of `rate` a second, each a pose and the zoom to send with it: each close-up
/// held [`CLOSEUP_S`] (the first [`FIRST_CLOSEUP_S`]), then the start with its own `zoom` for [`ZOOMED_OUT_S`]. Empty
/// when there are none.
pub fn closeup_steps(start: [f32; 6], zoom: f32, rate: u32) -> Vec<([f32; 6], f32)> {
    let Some((fov, poses)) = closeups(start, zoom) else { return Vec::new() };
    let ticks = |s: f32| ((s * rate as f32).round() as usize).max(1);
    let mut steps = Vec::new();
    for (i, p) in poses.iter().enumerate() {
        steps.extend(std::iter::repeat_n((*p, fov), ticks(if i == 0 { FIRST_CLOSEUP_S } else { CLOSEUP_S })));
    }
    steps.extend(std::iter::repeat_n((start, zoom), ticks(ZOOMED_OUT_S)));
    steps
}

/// What the worker read from the frame the recording started on (`orbiters-reconstruct --find-faces`).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
pub struct StartFrame {
    pub width: u32,
    pub height: u32,
    /// The strip's projection terms: the frame's exact field of view.
    pub p00: f32,
    pub p11: f32,
    /// The faces' boxes in the frame's pixels: x0, y0, x1, y1, confidence.
    pub boxes: Vec<[f32; 5]>,
    /// The hips of the avatar wearing the capture kit, from its skeleton band.
    pub hips: Option<[f32; 3]>,
}

/// VRChat's camera field of view at the slider's default of 45 (`/usercamera/Zoom`): 47.27° vertically, in every frame
/// the recordings of 2026-11-03 show (P11 = 2.286). VRChat 2026.4.1's OSC neither sends nor lists the slider, however
/// its documentation has it, so the zoom a picture must put back is worked out from the frame's own projection.
const DEFAULT_ZOOM_FOV: f32 = 47.27;
const DEFAULT_ZOOM: f32 = 45.0;

/// The slider's value for a frame's vertical field of view (degrees), the two taken as proportional around the default,
/// the only point measured.
pub fn zoom_of_fov(fov: f32) -> f32 {
    fov * DEFAULT_ZOOM / DEFAULT_ZOOM_FOV
}

impl StartFrame {
    /// Its vertical field of view in degrees; `None` from a frame with no usable projection.
    pub fn fov(&self) -> Option<f32> {
        (self.p11 > 0.0 && self.p11.is_finite()).then(|| 2.0 * (1.0 / self.p11).atan().to_degrees())
    }
}

/// How wide a head is taken to be, metres, to tell how far a box is.
const HEAD_M: f32 = 0.25;
/// How much of the frame's height a face takes in a face's views (from the 47° default, about 0.7 m from it: nearer
/// than the flight usually comes, and close enough to give a face more than twice the pixels of one at 1.5 m).
const FACE_SHARE: f32 = 0.4;
/// Faces flown around at most (each takes [`FACE_HOLD_S`] × [`FACE_ARC`]'s views), the largest first.
const MOST_FACES: usize = 3;
/// A box narrower than this is too far to be the picture's subject.
const LEAST_FACE_PX: f32 = 40.0;
/// Detections weaker than this do not make the camera go anywhere: a poster's face scores up to 0.84 too, but a wrong
/// visit costs only half a second.
const FACE_CONFIDENCE: f32 = 0.5;
/// The wearer is flown around too when their head is within this of the camera (the camera held in front of them).
const WEARER_REACH_M: f32 = 3.0;
/// What the avatar's hips are of its eye height above the ground (orbiters_reconstruct::ground::HIPS_PER_EYE_HEIGHT).
const HIPS_PER_EYE_HEIGHT: f32 = 0.62;
/// How long each view of a face is held: three frames of VRChat's stream (see [`CLOSEUP_S`]).
const FACE_HOLD_S: f32 = 0.1;
/// A face's views: turned this far around it, left and right of the way the camera sees it, degrees, and raised.
/// Measured on the 3D picture of 2026-11-03 with Enzo's face (one colour view close and from the front): see the desktop
/// repository's docs/3d-picture-faces-2026-11-04.md.
const FACE_ARC: [(f32, f32); 4] = [(0.0, 0.0), (-40.0, 0.0), (40.0, 0.0), (0.0, 25.0)];

/// The camera at `eye` looking at `at`, level (`near_yaw`: the yaw it turns from, so it never goes the long way round).
fn looking_at(eye: [f32; 3], at: [f32; 3], near_yaw: f32) -> [f32; 6] {
    let d = [at[0] - eye[0], at[1] - eye[1], at[2] - eye[2]];
    let flat = (d[0] * d[0] + d[2] * d[2]).sqrt();
    [eye[0], eye[1], eye[2], -d[1].atan2(flat).to_degrees(), near(near_yaw, d[0].atan2(d[2]).to_degrees()), 0.0]
}

/// Where the heads are that a 3D picture from `start` should see closely: the faces the start frame shows (the largest
/// first, at the distance their boxes' widths say), then the wearer's own when it is near and not among them (from the
/// capture kit's hips and the avatar's eye height: the bones' lengths are unknown, a head stands at about this height).
pub fn heads_to_visit(start: [f32; 6], frame: &StartFrame, eye_height: Option<f32>) -> Vec<[f32; 3]> {
    let [right, up, forward] = super::dolly::axes(&start);
    let eye = [start[0], start[1], start[2]];
    let (fx, fy) = (frame.p00 * frame.width as f32 / 2.0, frame.p11 * frame.height as f32 / 2.0);
    let (cx, cy) = (frame.width as f32 / 2.0, frame.height as f32 / 2.0);
    let mut heads = Vec::new();
    if fx > 0.0 && fy > 0.0 {
        let mut boxes: Vec<[f32; 5]> = frame.boxes.iter().copied().filter(|b| b[4] >= FACE_CONFIDENCE && b[2] - b[0] >= LEAST_FACE_PX).collect();
        boxes.sort_by(|a, b| (b[2] - b[0]).total_cmp(&(a[2] - a[0])));
        for b in boxes.iter().take(MOST_FACES) {
            let (x, y) = (((b[0] + b[2]) / 2.0 - cx) / fx, (cy - (b[1] + b[3]) / 2.0) / fy);
            let d: [f32; 3] = std::array::from_fn(|i| forward[i] + right[i] * x + up[i] * y);
            let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let distance = HEAD_M * fx / (b[2] - b[0]).max(1.0);
            heads.push(std::array::from_fn(|i| eye[i] + d[i] / length * distance));
        }
    }
    if let (Some(hips), Some(height)) = (frame.hips, eye_height) {
        let wearer = [hips[0], hips[1] + (1.0 - HIPS_PER_EYE_HEIGHT) * height, hips[2]];
        let apart = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        if apart(wearer, eye) <= WEARER_REACH_M && heads.iter().all(|h| apart(*h, wearer) > 0.5) && heads.len() < MOST_FACES {
            heads.push(wearer);
        }
    }
    heads
}

/// The camera's poses around the heads of [`heads_to_visit`], from `start`: each head from the side the camera is on,
/// at the distance where a face takes [`FACE_SHARE`] of the frame (farther only when the camera is already nearer than that
/// half a metre), straight and turned ([`FACE_ARC`]), each held [`FACE_HOLD_S`]; last, the start again. The flight then
/// goes on as before. Moving the camera rather than zooming gives a face more pixels AND the angles its depth needs; and it
/// asks nothing of VRChat's zoom.
pub fn face_steps(start: [f32; 6], heads: &[[f32; 3]], p11: f32, height: u32, rate: u32) -> Vec<[f32; 6]> {
    if heads.is_empty() || !(p11 > 0.0) {
        return Vec::new();
    }
    let fy = p11 * height as f32 / 2.0;
    let radius = (HEAD_M * fy / (FACE_SHARE * height as f32)).max(0.5);
    let hold = ((FACE_HOLD_S * rate as f32).round() as usize).max(1);
    let [_, _, forward] = super::dolly::axes(&start);
    let mut steps = Vec::new();
    for head in heads {
        // From the head towards the camera, level; straight ahead of the camera's own view when it stands in the head.
        let flat = [start[0] - head[0], start[2] - head[2]];
        let length = (flat[0] * flat[0] + flat[1] * flat[1]).sqrt();
        let base = if length > 0.1 { flat[0].atan2(flat[1]) } else { (-forward[0]).atan2(-forward[2]) };
        for (turn, rise) in FACE_ARC {
            let (a, r) = (base + turn.to_radians(), rise.to_radians());
            let eye = [head[0] + radius * r.cos() * a.sin(), head[1] + radius * r.sin(), head[2] + radius * r.cos() * a.cos()];
            steps.extend(std::iter::repeat_n(looking_at(eye, *head, start[4]), hold));
        }
    }
    steps.extend(std::iter::repeat_n(start, hold));
    steps
}

/// Reads the frame the recording started on through the worker (its own process: the app itself carries no detector),
/// `None` when that fails or takes too long: the picture then goes on without.
fn read_start_frame(session: &std::path::Path) -> Option<StartFrame> {
    let file = session.join(orbiters_contracts::manifest::KEYFRAMES_DIR).join("000000.qoi");
    let begin = Instant::now();
    while !file.is_file() {
        if begin.elapsed() > Duration::from_millis(500) {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // The file may be a moment from whole: the capture writes it on a thread of its own.
    while begin.elapsed() < Duration::from_millis(READ_START_MS) {
        let mut command = std::process::Command::new(crate::worker::sibling_exe("orbiters-reconstruct"));
        command.arg("--find-faces").arg(&file).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().ok()?;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let output = child.wait_with_output().ok()?;
                    if let Ok(frame) = serde_json::from_slice(&output.stdout) {
                        return Some(frame);
                    }
                    break;
                }
                Ok(None) if begin.elapsed() < Duration::from_millis(READ_START_MS) => std::thread::sleep(Duration::from_millis(10)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                Err(_) => return None,
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// How long the picture waits for [`read_start_frame`], milliseconds: the people in it are waiting too.
const READ_START_MS: u64 = 1500;

/// N4N0's camera rig's parameters for a camera pose, with the avatar's hips at `hips`: following it, where it stands
/// from the hips (each axis over [`rig::REACH_M`]) and its turn from them (over [`rig::YAW_DEG`]), level on upright
/// hips (`rig::follow_values`). VRChat takes OSC floats from -1 to 1.
pub fn rig_parameters(pose: &[f32; 6], hips: &super::rig::Hips) -> Vec<(String, Vec<rosc::OscType>)> {
    let [x, y, z, yaw] = super::rig::values(pose, hips);
    [
        (rig::PARAMETER_FOLLOW, rosc::OscType::Bool(true)),
        (rig::PARAMETER_X, rosc::OscType::Float(x)),
        (rig::PARAMETER_Y, rosc::OscType::Float(y)),
        (rig::PARAMETER_Z, rosc::OscType::Float(z)),
        (rig::PARAMETER_YAW, rosc::OscType::Float(yaw)),
    ]
    .into_iter()
    .map(|(name, value)| (format!("{PARAMETERS_PREFIX}{name}"), vec![value]))
    .collect()
}

/// The capture rig on or off.
pub fn rig_switch(on: bool) -> (String, Vec<rosc::OscType>) {
    (format!("{PARAMETERS_PREFIX}{}", rig::PARAMETER), vec![rosc::OscType::Bool(on)])
}

/// What a 3D picture waits for once its recording stops.
#[derive(Default)]
pub struct BurstRuntime {
    /// A 3D picture's recording is under way (its reconstruction starts when it is saved).
    pub(super) recording: bool,
}

pub(super) fn floats(values: &[OscValue]) -> Vec<f32> {
    values.iter().filter_map(|v| if let OscValue::Float(f) = v { Some(*f) } else { None }).collect()
}

impl super::Controller {
    /// Whether VRChat's camera is open (`/usercamera/Mode` not 0): `None` when VRChat does not say (not found, or before
    /// 2025.3.3, whose tree has no camera).
    pub fn camera_open(&self) -> Option<bool> {
        // VRChat's camera node alone (its tree), else what VRChat sent when it last changed.
        let mode = self.camera_values().into_iter().find(|(p, _)| p == super::CAMERA_MODE).map(|(_, v)| v);
        let mode = mode.or_else(|| self.osc_values.lock().unwrap().latest.get(super::CAMERA_MODE).and_then(|(v, _)| v.first().cloned()));
        match mode {
            Some(OscValue::Int(mode)) => Some(mode != 0),
            _ => None,
        }
    }

    /// VRChat's camera's values as its OSCQuery tree has them now (`/usercamera/Mode`, `/usercamera/Zoom`…): empty when
    /// VRChat does not say (not found, or before 2025.3.3, whose tree has no camera).
    fn camera_values(&self) -> Vec<(String, OscValue)> {
        let query = self.with_osc(|o| o.vrchat_query()).flatten();
        query.map(|addr| orbiters_core::osc::vrchat_camera_values(addr, Duration::from_millis(500))).unwrap_or_default()
    }

    /// Takes a 3D picture with VRChat's camera where it stands now.
    pub fn take_3d_picture(self: &Shared) -> anyhow::Result<()> {
        anyhow::ensure!(self.snapshot.lock().unwrap().vrchat.running, "Start VRChat and open its camera first.");
        self.ensure_capture_idle()?;
        let (pose, focus, smooth, zoom, eye_height) = {
            let osc = self.osc_values.lock().unwrap();
            let pose = osc.latest.get(POSE).map(|(v, _)| floats(v)).filter(|v| v.len() == 6 && v.iter().any(|f| *f != 0.0));
            let focus = osc.latest.get(FOCAL_DISTANCE).and_then(|(v, _)| floats(v).first().copied()).unwrap_or(2.0).clamp(0.5, 8.0);
            let smooth = osc.latest.get(SMOOTH).and_then(|(v, _)| v.first().map(OscValue::truthy));
            let zoom = osc.latest.get(ZOOM).and_then(|(v, _)| floats(v).first().copied());
            let eye_height = osc.latest.get("/avatar/eyeheight").and_then(|(v, _)| floats(v).first().copied());
            (pose, focus, smooth, zoom, eye_height)
        };
        // The zoom as VRChat last sent it, else as its tree has it now: the close-ups put it back after them, so without
        // it known there are none.
        let usable = |z: &f32| (ZOOM_NARROWEST..=150.0).contains(z);
        let zoom = zoom.filter(usable).or_else(|| {
            let values = self.camera_values();
            values.into_iter().find(|(p, _)| p == ZOOM).and_then(|(_, v)| floats(&[v]).first().copied()).filter(usable)
        });
        let length = self.settings().photo_3d;
        // VRChat's camera must be open: OSC switches its mode once it is (to Stream, for Spout), nothing opens it
        // (VRChat's `/usercamera/Mode` changes nothing while the camera is closed, and there is no endpoint to open it).
        anyhow::ensure!(self.camera_open() != Some(false), orbiters_n4n0::platform::CAMERA_CLOSED);
        let Some(pose) = pose else {
            anyhow::bail!("Move VRChat's camera once so Orbiters knows where it is, then try again.");
        };
        let pose: [f32; 6] = pose.try_into().expect("six values");
        // N4N0's menu and link card stand in front of their owner: while the capture kit records (N4N0/Capture/Record),
        // the avatar hides them on its owner's client, out of the picture, and shows them again after it.
        self.burst.lock().unwrap().recording = true;
        if let Err(e) = self.start_capture(false) {
            self.burst.lock().unwrap().recording = false;
            return Err(e);
        }
        let ctl = self.clone();
        std::thread::Builder::new().name("3d-picture".into()).spawn(move || {
            let started = Instant::now();
            while ctl.snapshot.lock().unwrap().capture.state != CaptureState::Recording {
                if started.elapsed() > Duration::from_secs(8) || !ctl.is_capturing() {
                    ctl.burst.lock().unwrap().recording = false;
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            std::thread::sleep(SETTLE);
            let send = |address: &str, args: Vec<rosc::OscType>| {
                let _ = ctl.with_osc(|o| o.send(address, args));
            };
            // Smoothing would trail the swing behind.
            send(SMOOTH, vec![rosc::OscType::Bool(false)]);
            // The rig where the camera starts (an avatar without it ignores it), still off for the close-ups.
            ctl.place_rig(None, &pose);
            let tick = Duration::from_secs_f32(1.0 / RATE_HZ as f32);
            // The 4 and 8 s pictures' close-ups: zoomed in on the picture's framing from where the camera stands, each
            // pose with its zoom in one bundle (VRChat applies them together), back at the start's zoom at the end.
            let picture = matches!(length, Photo3dLength::Standard | Photo3dLength::Full);
            // The frame the recording started on: the faces to go and see, and the camera's real field of view, which
            // tells its zoom when VRChat does not (2026.4.1 sends no /usercamera/Zoom and its tree has none).
            let session = ctl.snapshot.lock().unwrap().capture.session_dir.clone();
            let frame = session.filter(|_| picture).and_then(|dir| read_start_frame(&dir));
            let zoom = zoom.or_else(|| frame.as_ref().and_then(StartFrame::fov).map(zoom_of_fov).filter(|z| (ZOOM_NARROWEST..=150.0).contains(z)));
            let closeups = match (length, zoom) {
                (Photo3dLength::Standard | Photo3dLength::Full, Some(zoom)) => closeup_steps(pose, zoom, RATE_HZ),
                _ => Vec::new(),
            };
            let begin = Instant::now();
            for (i, (p, z)) in closeups.iter().enumerate() {
                let camera = (POSE.to_string(), p.iter().map(|v| rosc::OscType::Float(*v)).collect());
                let _ = ctl.with_osc(|o| o.send_bundle(vec![camera, (ZOOM.to_string(), vec![rosc::OscType::Float(*z)])]));
                if let Some(wait) = (tick * (i as u32 + 1)).checked_sub(begin.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            // Around the faces, the camera at its own zoom: the picture's people from more sides within a second.
            let faces = frame.as_ref().map_or_else(Vec::new, |f| face_steps(pose, &heads_to_visit(pose, f, eye_height), f.p11, f.height, RATE_HZ));
            let begin = Instant::now();
            for (i, p) in faces.iter().enumerate() {
                send(POSE, p.iter().map(|v| rosc::OscType::Float(*v)).collect());
                if let Some(wait) = (tick * (i as u32 + 1)).checked_sub(begin.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            // The rig on for the flight; put back off when VRChat is, whatever ends the recording.
            ctl.hold_rig(true);
            let flight = path(pose, length, focus, RATE_HZ);
            // Experimental (VRC+): the camera dolly flies the camera and three more cameras around it (controller/dolly.rs).
            let dolly = ctl.with_settings(|s| s.dolly_streams) && ctl.start_dolly(&flight, zoom.unwrap_or(45.0), focus);
            let begin = Instant::now();
            for (i, p) in flight.into_iter().enumerate() {
                // The camera and the rig in one bundle: VRChat applies them together. The dolly moves the camera
                // itself: the rig only follows.
                let camera = (!dolly).then(|| (POSE.to_string(), p.iter().map(|v| rosc::OscType::Float(*v)).collect()));
                ctl.place_rig(camera, &p);
                let next = tick * (i as u32 + 1);
                if let Some(wait) = next.checked_sub(begin.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            if dolly {
                ctl.stop_dolly();
            }
            send(POSE, pose.iter().map(|v| rosc::OscType::Float(*v)).collect());
            if let (false, Some(zoom)) = (closeups.is_empty(), zoom) {
                send(ZOOM, vec![rosc::OscType::Float(zoom)]);
            }
            ctl.hold_rig(false);
            if let Some(smooth) = smooth.filter(|s| *s) {
                send(SMOOTH, vec![rosc::OscType::Bool(smooth)]);
            }
            std::thread::sleep(Duration::from_millis(250));
            if let Err(e) = ctl.stop_capture() {
                ctl.notify(NoticeKind::Error, e.to_string());
            }
        })?;
        Ok(())
    }

    /// Hands the flight to VRChat's camera dolly as four paths and plays them (`/dolly/Import`, `/dolly/Play`): false
    /// when that could not be done (the flight then moves the camera alone).
    fn start_dolly(&self, flight: &[[f32; 6]], zoom: f32, focus: f32) -> bool {
        let seconds = flight.len() as f32 / RATE_HZ as f32;
        let file = match super::dolly::write(&super::dolly::paths(flight, seconds, zoom, focus)) {
            Ok(file) => file,
            Err(e) => {
                self.notify(NoticeKind::Warning, format!("The camera dolly's paths could not be written ({e}): the 3D picture uses the camera alone."));
                return false;
            }
        };
        let imported = self.with_osc(|o| o.send(super::dolly::IMPORT, vec![rosc::OscType::String(file.display().to_string())]));
        if !imported.is_some_and(|r| r.is_ok()) {
            return false;
        }
        // VRChat reads the file before it plays.
        std::thread::sleep(Duration::from_millis(300));
        self.with_osc(|o| o.send(super::dolly::PLAY, vec![rosc::OscType::Bool(true)])).is_some_and(|r| r.is_ok())
    }

    /// Stops the dolly's animation and tells what its extra streams gave.
    fn stop_dolly(&self) {
        let _ = self.with_osc(|o| o.send(super::dolly::PLAY, vec![rosc::OscType::Bool(false)]));
        let stats = self.snapshot.lock().unwrap().capture.stats.clone().unwrap_or_default();
        let message = if stats.extra_frames == 0 {
            "The camera dolly's other cameras sent nothing: in VRChat set Camera › Dolly › Streaming to Multi (VRC+), then try again.".to_string()
        } else if stats.extra_keyframes == 0 {
            format!(
                "The camera dolly's other cameras sent {} frames, none with your capture kit's strip: VRChat does not draw it in them, so they add nothing.",
                stats.extra_frames
            )
        } else {
            format!("The camera dolly's other cameras sent {} frames: {} more views for the reconstruction.", stats.extra_frames, stats.extra_keyframes)
        };
        self.notify(NoticeKind::Info, message);
    }

    /// A recording was saved: a 3D picture's goes on to its reconstruction.
    pub(super) fn burst_saved(self: &Shared, session: Option<PathBuf>) {
        let was = std::mem::take(&mut self.burst.lock().unwrap().recording);
        let Some(dir) = session.filter(|_| was) else { return };
        match self.queue_reconstruction(dir) {
            Ok(false) => self.notify(NoticeKind::Success, "3D picture taken: it is being reconstructed."),
            Ok(true) => self.notify(NoticeKind::Success, format!("3D picture taken: its reconstruction starts {}.", self.reconstruction_waits_for())),
            Err(e) => self.notify(NoticeKind::Warning, format!("3D picture taken, not reconstructed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ends_where_it_started(poses: &[[f32; 6]], pose: [f32; 6]) {
        assert_eq!(poses.first(), Some(&pose));
        let end = poses.last().unwrap();
        assert!(end.iter().zip(pose).enumerate().all(|(i, (a, b))| if i == 4 { ((a - b) % 360.0).abs() < 1e-3 } else { (a - b).abs() < 1e-4 }), "{end:?}");
    }

    /// No jump between two poses (90 a second): under a metre and 20°.
    fn smooth_all_along(poses: &[[f32; 6]]) {
        for w in poses.windows(2) {
            let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt();
            assert!(d < 1.0 && (w[1][4] - w[0][4]).abs() < 20.0 && (w[1][3] - w[0][3]).abs() < 20.0, "{w:?}");
        }
    }

    fn headings(poses: &[[f32; 6]]) -> usize {
        let mut h: Vec<i32> = poses.iter().map(|p| (p[4].rem_euclid(360.0) / 30.0) as i32).collect();
        h.sort();
        h.dedup();
        h.len()
    }

    // At (1, 1.5, 0), facing +X (yaw 90), slightly down.
    const POSE: [f32; 6] = [1.0, 1.5, 0.0, 10.0, 90.0, 0.0];

    fn frame(boxes: Vec<[f32; 5]>, hips: Option<[f32; 3]>) -> StartFrame {
        StartFrame { width: 1920, height: 1080, p00: 1.286, p11: 2.286, boxes, hips }
    }

    #[test]
    fn the_zoom_vrchat_does_not_report_comes_from_the_frames_projection() {
        // Enzo's picture of 2026-11-03: P11 2.286, the slider at its default.
        let fov = frame(Vec::new(), None).fov().unwrap();
        assert!((fov - 47.27).abs() < 0.05, "{fov}");
        assert!((zoom_of_fov(fov) - 45.0).abs() < 0.1);
        assert_eq!(StartFrame::default().fov(), None);
    }

    #[test]
    fn a_face_in_the_frame_is_seen_closely_from_three_sides_and_the_camera_comes_back() {
        // Facing +Z, a face in the middle of the frame 2 m ahead (125 px... its width says how far).
        let start = [0.0, 1.5, 0.0, 0.0, 0.0, 0.0];
        let fx = 1.286 * 960.0;
        let w = 0.25 * fx / 2.0;
        let heads = heads_to_visit(start, &frame(vec![[960.0 - w / 2.0, 540.0 - w / 2.0, 960.0 + w / 2.0, 540.0 + w / 2.0, 0.8]], None), None);
        assert_eq!(heads.len(), 1);
        assert!((heads[0][2] - 2.0).abs() < 0.01 && (heads[0][1] - 1.5).abs() < 0.01 && heads[0][0].abs() < 0.01, "{heads:?}");
        let steps = face_steps(start, &heads, 2.286, 1080, 90);
        assert_eq!(steps.last(), Some(&start), "ends where it began");
        assert!(steps.len() as f32 / 90.0 <= 0.5 + 1e-3, "{} steps", steps.len());
        let (head, mut seen) = (heads[0], Vec::new());
        for s in &steps[..steps.len() - 9] {
            // Close, and aimed at the head.
            let to = [head[0] - s[0], head[1] - s[1], head[2] - s[2]];
            let d = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
            assert!((0.5..1.0).contains(&d), "{d}");
            let [_, _, f] = crate::controller::dolly::axes(s);
            let aim = (f[0] * to[0] + f[1] * to[1] + f[2] * to[2]) / d;
            assert!(aim > 0.999, "{aim}");
            if seen.last() != Some(s) {
                seen.push(*s);
            }
        }
        assert_eq!(seen.len(), FACE_ARC.len());
        // Far enough apart for the capture to keep each as a keyframe, and the same way round as the camera came.
        let mut selector = orbiters_core::keyframes::KeyframeSelector::new(orbiters_core::settings::KeyframeSettings::default(), false);
        let kept = seen.iter().enumerate().filter(|(i, s)| selector.consider_view(&camera_to_world(s), 2.286, *i as f32 * 0.1)).count();
        assert_eq!(kept, FACE_ARC.len());
    }

    #[test]
    fn weak_small_or_too_many_boxes_make_no_detour() {
        let start = [0.0, 1.5, 0.0, 0.0, 0.0, 0.0];
        let weak = [100.0, 100.0, 300.0, 300.0, 0.3];
        let small = [100.0, 100.0, 130.0, 130.0, 0.9];
        assert!(heads_to_visit(start, &frame(vec![weak, small], None), None).is_empty());
        assert!(face_steps(start, &[], 2.286, 1080, 90).is_empty(), "no faces: as before");
        let many: Vec<[f32; 5]> = (0..6).map(|i| [100.0 + 200.0 * i as f32, 400.0, 250.0 + 200.0 * i as f32, 550.0, 0.8]).collect();
        assert_eq!(heads_to_visit(start, &frame(many, None), None).len(), MOST_FACES);
    }

    #[test]
    fn the_wearer_near_the_camera_is_visited_unless_already_in_the_frame() {
        // Enzo's picture: hips (-8.543, 1.055, -7.389), eye height 1.797, the camera at (-8.47, 1.75, -7.76): he stood
        // 38 cm from it, and the head worked out from these is the one the flight's views show.
        let start = [-8.47, 1.75, -7.76, 1.7, 168.5, 0.0];
        let heads = heads_to_visit(start, &frame(Vec::new(), Some([-8.543, 1.055, -7.389])), Some(1.797));
        assert_eq!(heads.len(), 1);
        assert!((heads[0][1] - 1.738).abs() < 0.01, "{heads:?}");
        // Too far: a picture of something else.
        assert!(heads_to_visit(start, &frame(Vec::new(), Some([-1.0, 1.0, 3.0])), Some(1.797)).is_empty());
        // Unknown eye height or no band: nothing to go by.
        assert!(heads_to_visit(start, &frame(Vec::new(), Some([-8.5, 1.0, -7.4])), None).is_empty());
        assert!(heads_to_visit(start, &frame(Vec::new(), None), Some(1.797)).is_empty());
    }

    #[test]
    fn the_rig_follows_the_camera_from_the_hips_within_vrchats_range() {
        // Hips at the world's origin, upright, facing +Z: the values are the camera's world place.
        let origin: super::super::rig::Hips = ([0.0; 3], [0.0, 0.0, 0.0, 1.0]);
        let values = |pose: [f32; 6], hips: &super::super::rig::Hips| -> Vec<f32> {
            rig_parameters(&pose, hips).into_iter().skip(1).map(|(_, v)| if let rosc::OscType::Float(f) = v[0] { f } else { panic!() }).collect()
        };
        assert_eq!(
            rig_parameters(&POSE, &origin).iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(),
            [
                "/avatar/parameters/N4N0/Capture/Follow",
                "/avatar/parameters/N4N0/Capture/RigX",
                "/avatar/parameters/N4N0/Capture/RigY",
                "/avatar/parameters/N4N0/Capture/RigZ",
                "/avatar/parameters/N4N0/Capture/RigYaw"
            ]
        );
        assert_eq!(rig_parameters(&POSE, &origin)[0].1, vec![rosc::OscType::Bool(true)], "following the camera");
        let v = values([2048.0, 1.5, -409.6, 10.0, 270.0, 0.0], &origin);
        assert_eq!(v[0], 0.5);
        assert!((v[1] * rig::REACH_M - 1.5).abs() < 1e-4 && (v[2] + 0.1).abs() < 1e-6);
        // 270° is -90°: half a turn left of straight ahead.
        assert!((v[3] + 0.5).abs() < 1e-6);
        // Beyond the reach: at its edge.
        assert_eq!(values([9000.0, 0.0, -9000.0, 0.0, -540.0, 0.0], &origin)[..3], [1.0, 0.0, -1.0]);
        assert!((values([0.0; 6], &origin)[3]).abs() < 1e-6);
        // Hips 15 m away, turned 90° (facing +X): the camera 2 m in front of them is 2 m along their +Z, heading 0.
        let (s, c) = (45f32.to_radians().sin(), 45f32.to_radians().cos());
        let hips = ([15.0, 1.0, 6.0], [0.0, s, 0.0, c]);
        let v = values([17.0, 2.5, 6.0, 0.0, 90.0, 0.0], &hips);
        let m = |k: usize| v[k] * rig::REACH_M;
        assert!(m(0).abs() < 1e-3 && (m(1) - 1.5).abs() < 1e-3 && (m(2) - 2.0).abs() < 1e-3, "{v:?}");
        assert!(v[3].abs() < 1e-5, "the hips' heading");
    }

    #[test]
    fn one_second_sweeps_gently() {
        let poses = path(POSE, Photo3dLength::Quick, 2.0, 90);
        ends_where_it_started(&poses, POSE);
        smooth_all_along(&poses);
        assert!((poses.len() as i32 - 91).abs() <= 1);
        let side = poses.iter().map(|p| (p[2] - POSE[2]).abs()).fold(0.0, f32::max);
        assert!(side > 0.4 && side < 0.7, "about half a metre each way: {side}");
    }

    #[test]
    fn four_seconds_circle_me_then_look_every_way_and_fly_ten_metres_out() {
        let poses = path(POSE, Photo3dLength::Standard, 2.0, 90);
        ends_where_it_started(&poses, POSE);
        smooth_all_along(&poses);
        assert!((poses.len() as f32 - 4.0 * 90.0).abs() <= 12.0, "{} poses", poses.len());
        // The first second circles the camera's place about 2 m out, looking at it.
        let first: Vec<_> = poses[..=90].to_vec();
        let far = first.iter().map(|p| ((p[0] - POSE[0]).powi(2) + (p[2] - POSE[2]).powi(2)).sqrt()).fold(0.0, f32::max);
        assert!((far - 2.0).abs() < 0.05, "{far}");
        assert_eq!(headings(&first), 12, "looks at it from every side");
        // Straight up and down, every heading, 10 m forward (+X), to each side (±Z) and up.
        let pitches = poses.iter().map(|p| p[3]);
        assert!(pitches.clone().fold(f32::MAX, f32::min) <= -85.0 && pitches.fold(f32::MIN, f32::max) >= 85.0);
        assert_eq!(headings(&poses), 12);
        let reach = |f: &dyn Fn(&[f32; 6]) -> f32| poses.iter().map(f).fold(f32::MIN, f32::max);
        assert!(reach(&|p| p[0] - POSE[0]) > 9.9, "forward");
        assert!(reach(&|p| POSE[2] - p[2]) > 9.9, "right");
        assert!(reach(&|p| p[2] - POSE[2]) > 9.9, "left");
        assert!(reach(&|p| p[1] - POSE[1]) > 9.9, "up");
    }

    /// The camera-to-world of a pose (Unity's, rows of a 3×4 matrix: its right, up and forward axes as columns).
    fn camera_to_world(p: &[f32; 6]) -> [f32; 12] {
        let [r, u, f] = super::super::dolly::axes(p);
        [r[0], u[0], f[0], p[0], r[1], u[1], f[1], p[1], r[2], u[2], f[2], p[2]]
    }

    /// Whether the point of the start view at (u, v) (fractions of its width and height, top-down) shows clear (outside
    /// the capture strip's and the skeleton band's areas, which training leaves out) in the close-up `c` at `fov`.
    fn clear_in(start: &[f32; 6], zoom: f32, u: f32, v: f32, c: &[f32; 6], fov: f32) -> bool {
        let half = |f: f32| (f.to_radians() / 2.0).tan();
        let [r0, u0, f0] = super::super::dolly::axes(start);
        let (x, y) = ((2.0 * u - 1.0) * half(zoom) * ASPECT, (1.0 - 2.0 * v) * half(zoom));
        let d: [f32; 3] = std::array::from_fn(|i| f0[i] + r0[i] * x + u0[i] * y);
        let [r, up, f] = super::super::dolly::axes(c);
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let z = dot(d, f);
        if z <= 0.0 {
            return false;
        }
        let (cu, cv) = (0.5 + dot(d, r) / z / (2.0 * half(fov) * ASPECT), 0.5 - dot(d, up) / z / (2.0 * half(fov)));
        let (w, h) = (1920.0, 1080.0);
        let (px, py) = (cu * w, cv * h);
        let strip = orbiters_contracts::strip::StripLayout::new(1920, 1080).unwrap().reserved;
        let band = orbiters_contracts::skeleton::BandLayout::new(1920, 1080).unwrap().reserved();
        let inside = |x0: u32, y0: u32, x1: u32, y1: u32| px >= x0 as f32 && px < x1 as f32 && py >= y0 as f32 && py < y1 as f32;
        (0.0..w).contains(&px) && (0.0..h).contains(&py) && !inside(strip.x0, strip.y0, strip.x1, strip.y1) && !inside(band.x0, band.y0, band.x1, band.y1)
    }

    #[test]
    fn close_ups_zoom_in_on_the_whole_framing_from_where_the_camera_stands() {
        for (start, zoom) in [(POSE, 45.0), ([3.0, 1.2, -4.0, -12.0, 200.0, 0.0], 47.3), (POSE, 60.0), (POSE, 90.0)] {
            let (fov, poses) = closeups(start, zoom).expect("close-ups");
            assert_eq!(poses.len(), 9);
            // As narrow as VRChat goes from its default (2.35 times); from 45.9° on, 2.4 times.
            let magnify = (zoom.to_radians() / 2.0).tan() / (fov.to_radians() / 2.0).tan();
            if zoom <= 45.0 {
                assert_eq!(fov, ZOOM_NARROWEST, "{zoom}");
                assert!(magnify > 2.3, "{magnify}");
            } else {
                assert!((magnify - MAGNIFY).abs() < 1e-3 && fov > ZOOM_NARROWEST, "{zoom}: {fov}");
            }
            for c in &poses {
                assert_eq!(c[..3], start[..3], "from where it stands");
                assert_eq!(c[5], start[5]);
            }
            // Every point of the framing below the top's strip band shows clear in a close-up, and nearly all of it.
            let (mut clear, mut all) = (0, 0);
            for j in 0..60 {
                for i in 0..80 {
                    let (u, v) = ((i as f32 + 0.5) / 80.0, (j as f32 + 0.5) / 60.0);
                    let seen = poses.iter().any(|c| clear_in(&start, zoom, u, v, c, fov));
                    if (0.03..0.97).contains(&u) && (0.25..0.93).contains(&v) {
                        assert!(seen, "{zoom}: ({u}, {v}) seen in no close-up");
                    }
                    clear += seen as u32;
                    all += 1;
                }
            }
            assert!(clear as f32 > 0.9 * all as f32, "{zoom}: {clear} of {all}");
        }
        // Zoomed in already, or a zoom VRChat does not have: none.
        assert!(closeups(POSE, 25.0).is_none() && closeups(POSE, 10.0).is_none() && closeups(POSE, f32::NAN).is_none());
        assert_eq!(closeups(POSE, 30.0).map(|(fov, _)| fov), Some(ZOOM_NARROWEST), "1.5 times: worth it");
    }

    #[test]
    fn close_ups_take_about_a_second_and_the_capture_keeps_each_one() {
        let steps = closeup_steps(POSE, 45.0, 90);
        let seconds = steps.len() as f32 / 90.0;
        assert!((seconds - 1.1).abs() < 0.02, "{seconds}");
        assert_eq!(steps.last(), Some(&(POSE, 45.0)), "back at the start's zoom where it started");
        assert!(steps[..steps.len() - 9].iter().all(|(_, z)| *z == ZOOM_NARROWEST));
        // The first close-up is held longer while VRChat zooms in.
        assert!(steps[..18].iter().all(|s| s.0 == steps[0].0) && steps[18].0 != steps[0].0);
        // Frames as the capture sees them (the strip's pose and P11), one every 33 ms: every close-up becomes a keyframe
        // (after the start's own), and so does the start zoomed back out.
        let mut selector = orbiters_core::keyframes::KeyframeSelector::new(orbiters_core::settings::KeyframeSettings::default(), false);
        let p11 = |fov: f32| 1.0 / (fov.to_radians() / 2.0).tan();
        assert!(selector.consider_view(&camera_to_world(&POSE), p11(45.0), 0.0));
        let mut kept = Vec::new();
        let mut t = 0.0;
        while t < seconds {
            let (p, z) = steps[((t * 90.0) as usize).min(steps.len() - 1)];
            if selector.consider_view(&camera_to_world(&p), p11(z), 1.0 + t) {
                kept.push((p, z));
            }
            t += 0.033;
        }
        let (_, poses) = closeups(POSE, 45.0).unwrap();
        for c in &poses {
            assert!(kept.iter().any(|(p, _)| p == c), "close-up {c:?} kept");
        }
        assert_eq!(kept.last(), Some(&(POSE, 45.0)));
        // No close-ups for the 1 s picture's caller to send, nor without a usable zoom.
        assert!(closeup_steps(POSE, 22.0, 90).is_empty());
    }

    #[test]
    fn eight_seconds_add_four_distant_circles() {
        let poses = path(POSE, Photo3dLength::Full, 2.0, 90);
        ends_where_it_started(&poses, POSE);
        smooth_all_along(&poses);
        assert!((poses.len() as f32 - 8.0 * 90.0).abs() <= 20.0, "{} poses", poses.len());
        // Behind the start (−X) too, 8 m out plus the circle.
        let back = poses.iter().map(|p| POSE[0] - p[0]).fold(f32::MIN, f32::max);
        assert!(back > 9.0, "{back}");
    }
}
