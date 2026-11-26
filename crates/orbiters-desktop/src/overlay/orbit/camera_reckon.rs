//! Where VRChat's camera panel is, in a headset, without reading any picture (Enzo, 2026-11-13: tracking it by its looks "seems to work
//! pretty shit"; follow it as the Quick Menu and the wheel are followed). VRChat's camera panel lives in SteamVR's tracking space, not in the
//! world: walking and turning with the sticks leave it where it is in the play space (measured in the sandbox, under a millimetre). So what it
//! does can be replayed from what SteamVR already tells us:
//!
//! - **Opening.** It appears by the **left** controller, whichever hand opened the Action Menu, facing the head, its long side level. Its middle
//!   is the left controller's place moved by [`FROM_LEFT_HAND`] in that controller's frame (about where VRChat's hand is) and by [`BESIDE_HAND`]
//!   in the head's turn about the vertical. Fitted on 13 opens with real VRChat (camera4/m1): 1 to 4 mm and under 2 degrees for hands and head
//!   in reach of each other; 24 mm with the head 30 cm away from where the arms reach.
//! - **Grab.** A grip pressed (as VRChat's own binding clicks it: `vrchat_grip`) with VRChat's grab point ([`GRAB_POINT`], beside the
//!   controller, towards the palm) within [`GRAB_REACH_M`] of the panel's box ([`GRAB_BOX`], VRChat's own pickup box) takes it; it then moves rigidly with that controller (measured 2 mm, 2.5 degrees) until that grip is let go, and stays where it
//!   was let go. A grip pressed by the other hand while one holds it, within reach, takes it over (Enzo, 2026-11-16: "if we grab it with hand A,
//!   and while still grabbed we try to grab it with hand B, it should be transferred to hand B"); the first hand's grip, still held, does not take
//!   it back until it is let go and pressed again. Both grips pressed in the same frame: the right hand takes it. Its camera modes do not move it.
//! - **Measured, for the avatar worn** (Request 18). [`FROM_WRIST`] and [`PANEL_WIDTH_M`] were fitted in the sandbox, with its avatar and its
//!   person's height; in Enzo's headset (2026-11-20, his pictures 4 and 6) the tools stood 4 to 7 cm low and 1 to 3 cm in, about a tenth too large:
//!   where the panel opens from VRChat's wrist, and how large it is in the room, change with the avatar's size against the person's. So at each
//!   opening the picture reader looks at the panel lying where it opened; two sightings with both eyes that agree ([`Fit::agrees`]) re-lay it
//!   ([`Reckon::reanchor`]) and the offset and width they give ([`Fit::from_sighting`]) are kept for the next openings ([`Fit::load`]).
//! - **Close.** Its X closes it; VRChat's laser is the one measured on Touch controllers ([`space::vrchat_touch_laser`]). A trigger pressed while
//!   that laser is on the X is seen here ([`Reckon::step`]), with no overlay over VRChat's button, so VRChat gets its click whole.

use orbiters_core::locks::*;
use orbiters_openvr::Matrix34;

use super::super::floating::space;
use super::camera_track::{PANEL_SIZE, PANEL_X};

/// From the left controller's pose to the point the panel is laid from, in that controller's frame (metres).
pub const FROM_LEFT_HAND: [f32; 3] = [-0.0055, -0.0135, 0.0979];
/// From that point to the panel's middle, in the head's frame turned only about the vertical (metres): to the right of the hand.
pub const BESIDE_HAND: [f32; 3] = [0.1554, 0.0012, 0.0033];
/// The panel's width in the room, as both eyes measured it in the sandbox (0.2845 to 0.2885 m).
pub const PANEL_WIDTH_M: f32 = 0.288;
/// Where a grip takes it: a box in the panel's plane (left, right, bottom, top, metres from its middle, up positive), and how far from it,
/// measured from [`GRAB_POINT`]. The box is VRChat's own: the camera panel's pickup trigger in its scene data is 0.291 x 0.167 m about the
/// picture's middle (read from VRChat's installed files, camera5/re-report.md; its pickup "proximity" says 0.2, which no grab point fits).
pub const GRAB_BOX: [f32; 4] = [-0.1455, 0.1455, -0.0835, 0.0835];
pub const GRAB_REACH_M: f32 = 0.14;
/// Where VRChat measures a grab from, in the right controller's frame (metres; the left hand's has its x turned over): not the controller's
/// origin. With the controller 11 cm in front, VRChat took the panel with the hand level, not with it tilted 45 degrees down; it took it
/// from 15 cm tilted 45 degrees up and from 17 cm tilted 70 degrees up; turned 45 degrees about the vertical, one way took and the other
/// did not, mirrored between the hands (sandbox 2026-11-14, 24 grips, `testdata_camera/sandbox-2026-11-14/reach.json`). Fitted on those and
/// the 13 grips of 2026-11-13 with VRChat's box: all 37 agree, 14 mm from the nearest; the controller's origin got 8 of the 24 wrong.
pub const GRAB_POINT: [f32; 3] = [-0.07, -0.03, 0.035];
/// Where VRChat's X takes a laser, in the panel's units from its middle (y down): its picture's place [`PANEL_X`] one unit higher. With
/// VRChat's measured laser ([`space::vrchat_touch_laser`]) the 11 aims of 2026-11-13 (camera4 m1 and m4, from 11-12 cm) land 0.0 units from
/// [`PANEL_X`] for the four that closed it straight on, 15.2 above for the one at 67 degrees that closed it too, 14.7 below for 57 degrees and
/// 20 or more for the rest, which did not: the button reaches further up than down.
pub const CLOSE_AT: [f32; 2] = [PANEL_X[0], PANEL_X[1] - 1.0];
/// How near [`CLOSE_AT`] a laser's spot counts as on the X, in the panel's units (the button is about 30 across).
pub const CLOSE_RADIUS: f32 = 15.0;

/// VRChat's own left wrist, as it says over OSC (`/tracking/vrsystem/leftwrist/pose`, sent to Orbiters' port every frame): where its avatar's
/// hand really is. With an avatar whose arm cannot reach the controller, the panel opens by that hand, not by the controller (sandbox r2: 24 cm).
/// Both wrists (left, right), as VRChat said them lately.
static WRISTS: std::sync::Mutex<[Option<(std::time::Instant, [f32; 6])>; 2]> = std::sync::Mutex::new([None; 2]);

/// The controller heard VRChat say where a wrist is (0 left, 1 right): x, y, z (metres) and Euler x, y, z (degrees), in Unity's axes.
pub fn wrist_said(hand: usize, values: [f32; 6]) {
    WRISTS.locked()[hand.min(1)] = Some((std::time::Instant::now(), values));
}

/// VRChat's wrist (0 left, 1 right) in SteamVR's standing space, if it said so lately.
pub fn wrist(hand: usize, within: std::time::Duration) -> Option<Matrix34> {
    let (at, values) = WRISTS.locked()[hand.min(1)]?;
    (at.elapsed() <= within).then(|| unity_pose(values))
}

/// VRChat's left wrist, for where the panel opens.
pub fn left_wrist(within: std::time::Duration) -> Option<Matrix34> {
    wrist(0, within)
}

/// A pose as VRChat's OSC gives it (Unity: Y up, Z forward, left-handed; rotation Y then X then Z, degrees) in SteamVR's axes (Z back).
pub fn unity_pose(v: [f32; 6]) -> Matrix34 {
    let (x, y, z) = (v[3].to_radians(), v[4].to_radians(), v[5].to_radians());
    let rx = [[1.0, 0.0, 0.0], [0.0, x.cos(), -x.sin()], [0.0, x.sin(), x.cos()]];
    let ry = [[y.cos(), 0.0, y.sin()], [0.0, 1.0, 0.0], [-y.sin(), 0.0, y.cos()]];
    let rz = [[z.cos(), -z.sin(), 0.0], [z.sin(), z.cos(), 0.0], [0.0, 0.0, 1.0]];
    let mul = |a: [[f32; 3]; 3], b: [[f32; 3]; 3]| -> [[f32; 3]; 3] { std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum())) };
    let r = mul(mul(ry, rx), rz);
    // Z flipped on both sides: S R S.
    let s = |i: usize| if i == 2 { -1.0 } else { 1.0 };
    Matrix34 { m: std::array::from_fn(|i| [r[i][0] * s(i) * s(0), r[i][1] * s(i) * s(1), r[i][2] * s(i) * s(2), v[i] * s(i)]) }
}

fn column(m: &Matrix34, c: usize) -> [f32; 3] {
    [m.m[0][c], m.m[1][c], m.m[2][c]]
}
fn turn(m: &Matrix34, v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| m.m[i][0] * v[0] + m.m[i][1] * v[1] + m.m[i][2] * v[2])
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn unit(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = dot(a, a).sqrt();
    (l > 1e-6).then(|| [a[0] / l, a[1] / l, a[2] / l])
}
fn frame(x: [f32; 3], y: [f32; 3], z: [f32; 3], at: [f32; 3]) -> Matrix34 {
    Matrix34 { m: std::array::from_fn(|i| [x[i], y[i], z[i], at[i]]) }
}
/// `a` then `b`, as frames.
pub fn then(a: &Matrix34, b: &Matrix34) -> Matrix34 {
    Matrix34 { m: std::array::from_fn(|i| { let r = |c: usize| a.m[i][0] * b.m[0][c] + a.m[i][1] * b.m[1][c] + a.m[i][2] * b.m[2][c]; [r(0), r(1), r(2), r(3) + a.m[i][3]] }) }
}
/// What undoes a frame with no scale in it.
pub fn undone(a: &Matrix34) -> Matrix34 {
    let r = |i: usize, c: usize| a.m[c][i];
    Matrix34 { m: std::array::from_fn(|i| [r(i, 0), r(i, 1), r(i, 2), -(r(i, 0) * a.m[0][3] + r(i, 1) * a.m[1][3] + r(i, 2) * a.m[2][3])]) }
}

/// Where the panel appears when the camera opens: from the left controller and the head at that moment. Its frame as camera_track has it: X to
/// its right, Y up, Z towards who looks, at its middle.
pub fn opening(head: &Matrix34, left: &Matrix34) -> Matrix34 {
    let forward = column(head, 2); // the head looks along −Z
    let yaw = f32::atan2(forward[0], forward[2]);
    let (s, c) = yaw.sin_cos();
    let beside = [c * BESIDE_HAND[0] + s * BESIDE_HAND[2], BESIDE_HAND[1], -s * BESIDE_HAND[0] + c * BESIDE_HAND[2]];
    let at = add(add(column(left, 3), turn(left, FROM_LEFT_HAND)), beside);
    facing(at, column(head, 3))
}

/// From VRChat's left wrist (its OSC pose) to the panel's middle, in the wrist's frame (metres).
/// Fitted on 10 opens (camera4/m3): 2.4 to 9.8 mm, 23 mm with the head 30 cm back. VRChat's OSC poses are in SteamVR's standing space (its
/// head said (-0.05, 1.75, -0.05) where SteamVR had it), so nothing is registered between them.
pub const FROM_WRIST: [f32; 3] = [0.0424, -0.1735, -0.0884];

/// Where the panel appears, from VRChat's own left wrist: right even when the avatar's arm cannot reach the controller (sandbox: 24 cm off by the
/// controller rule with the head moved back and the hand tilted, 4.4 mm by the wrist).
pub fn opening_by_wrist(head: &Matrix34, wrist: &Matrix34) -> Matrix34 {
    facing(add(column(wrist, 3), turn(wrist, FROM_WRIST)), column(head, 3))
}

/// A panel at `at` turned to face `eye`, its long side level.
pub fn facing(at: [f32; 3], eye: [f32; 3]) -> Matrix34 {
    let z = unit(sub(eye, at)).unwrap_or([0.0, 0.0, 1.0]);
    let x = unit(cross([0.0, 1.0, 0.0], z)).unwrap_or([1.0, 0.0, 0.0]);
    frame(x, cross(z, x), z, at)
}

/// Whether a grip of `hand` (0 left, 1 right) with its controller at `pose` takes the panel, as VRChat decides it.
pub fn within_grab(panel: &Matrix34, pose: &Matrix34, hand: usize) -> bool {
    within_grab_scaled(panel, pose, hand, 1.0)
}

/// [`within_grab`] for a panel `scale` times the sandbox's: its box scales with it.
pub fn within_grab_scaled(panel: &Matrix34, pose: &Matrix34, hand: usize, scale: f32) -> bool {
    let point = [if hand == 0 { -GRAB_POINT[0] } else { GRAB_POINT[0] }, GRAB_POINT[1], GRAB_POINT[2]];
    within_reach_scaled(panel, add(column(pose, 3), turn(pose, point)), GRAB_REACH_M, scale)
}

/// Whether a point is within `reach` of the panel's grab box.
pub fn within_reach(panel: &Matrix34, point: [f32; 3], reach: f32) -> bool {
    within_reach_scaled(panel, point, reach, 1.0)
}

fn within_reach_scaled(panel: &Matrix34, point: [f32; 3], reach: f32, scale: f32) -> bool {
    let p = undone(panel);
    let at = add(turn(&p, point), column(&p, 3));
    let out = |v: f32, lo: f32, hi: f32| (lo - v).max(v - hi).max(0.0);
    let d = [out(at[0], GRAB_BOX[0] * scale, GRAB_BOX[1] * scale), out(at[1], GRAB_BOX[2] * scale, GRAB_BOX[3] * scale), at[2]];
    dot(d, d).sqrt() <= reach
}

/// VRChat's laser from a controller (its own pose): where it starts and the way it goes. Measured on Touch controllers; assumed the same on
/// others (VRChat's laser there was not measured).
pub fn laser(hand: &Matrix34) -> ([f32; 3], [f32; 3]) {
    space::ray(&space::vrchat_touch_laser(hand))
}

/// Where a laser lands on the panel, in its units from its middle (y down, as [`PANEL_X`]), if it lands on its front.
pub fn laser_spot(panel: &Matrix34, panel_unit: f32, origin: [f32; 3], along: [f32; 3]) -> Option<[f32; 2]> {
    let p = undone(panel);
    let o = add(turn(&p, origin), column(&p, 3));
    let d = turn(&p, along);
    if d[2] >= -1e-4 || o[2] <= 0.0 {
        return None;
    }
    let t = -o[2] / d[2];
    Some([(o[0] + d[0] * t) / panel_unit, -(o[1] + d[1] * t) / panel_unit])
}

/// How near VRChat's X (its middle, [`CLOSE_AT`]) a laser leaves the click to VRChat alone: our camera tools beside the panel take neither
/// hover nor press from it (Enzo, 2026-11-16, picture D: one pull closed VRChat's camera and pressed our "3D of people" too). 4 cm: the
/// X's own reach (1 cm) and the whole error picture D showed before our laser followed VRChat's, so that even then the click would have been
/// VRChat's alone. Only the corner of the column's top tile nearest the X lies that near (about its inner 16 of 92 units).
pub const BY_THE_X_M: f32 = 0.04;

/// Whether a laser (`origin`, `along`) is within [`BY_THE_X_M`] of VRChat's X on the panel: a click there is VRChat's.
pub fn by_the_close(panel: &Matrix34, panel_unit: f32, origin: [f32; 3], along: [f32; 3]) -> bool {
    laser_spot(panel, panel_unit, origin, along).is_some_and(|s| (s[0] - CLOSE_AT[0]).hypot(s[1] - CLOSE_AT[1]) * panel_unit <= BY_THE_X_M)
}

/// Whether a laser from `hand` is on the panel's X.
pub fn on_close(panel: &Matrix34, panel_unit: f32, hand: &Matrix34) -> bool {
    let (o, d) = laser(hand);
    laser_spot(panel, panel_unit, o, d).is_some_and(|s| (s[0] - CLOSE_AT[0]).hypot(s[1] - CLOSE_AT[1]) <= CLOSE_RADIUS)
}

/// Where VRChat's panel opens from its left wrist, and how wide it is: the sandbox's ([`Fit::SANDBOX`]) or what the picture measured for the avatar worn.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Fit {
    /// From VRChat's left wrist to the panel's middle, in the wrist's frame (metres).
    pub from_wrist: [f32; 3],
    /// The panel's width in the room (metres).
    pub width_m: f32,
}

/// Two sightings agree when they give offsets this near (metres) and widths within this share.
pub const FIT_AGREE_M: f32 = 0.012;
pub const FIT_AGREE_WIDTH: f32 = 0.04;
/// A measure further than this from the sandbox's, or a width outside these, is a misreading, not the panel.
pub const FIT_MOST_OFF_M: f32 = 0.25;
pub const FIT_WIDTHS_M: [f32; 2] = [0.15, 0.5];

impl Fit {
    pub const SANDBOX: Fit = Fit { from_wrist: FROM_WRIST, width_m: PANEL_WIDTH_M };

    /// What a sighting of the panel lying where it opened (`seen`, `width_m` measured with both eyes) says of this fit, from VRChat's wrist at the
    /// opening. None when it is too far from anything VRChat does (a misreading).
    pub fn from_sighting(wrist: &Matrix34, seen: &Matrix34, width_m: f32) -> Option<Fit> {
        let w = undone(wrist);
        let from_wrist = add(turn(&w, column(seen, 3)), column(&w, 3));
        let off = sub(from_wrist, FROM_WRIST);
        (dot(off, off).sqrt() <= FIT_MOST_OFF_M && (FIT_WIDTHS_M[0]..=FIT_WIDTHS_M[1]).contains(&width_m)).then_some(Fit { from_wrist, width_m })
    }

    /// Whether two measures are of the same panel.
    pub fn agrees(&self, other: &Fit) -> bool {
        let d = sub(self.from_wrist, other.from_wrist);
        dot(d, d).sqrt() <= FIT_AGREE_M && (self.width_m / other.width_m - 1.0).abs() <= FIT_AGREE_WIDTH
    }

    /// The mean of two that agree.
    pub fn mean(&self, other: &Fit) -> Fit {
        Fit { from_wrist: std::array::from_fn(|i| (self.from_wrist[i] + other.from_wrist[i]) / 2.0), width_m: (self.width_m + other.width_m) / 2.0 }
    }

    /// Where the panel opens by this fit: from VRChat's wrist, facing the head, its long side level.
    pub fn opening(&self, head: &Matrix34, wrist: &Matrix34) -> Matrix34 {
        facing(add(column(wrist, 3), turn(wrist, self.from_wrist)), column(head, 3))
    }

    /// The panel's unit (its picture is [`PANEL_SIZE`] units across).
    pub fn unit(&self) -> f32 {
        self.width_m / PANEL_SIZE[0]
    }

    fn file() -> std::path::PathBuf {
        orbiters_core::paths::local_data_root().join("camera-panel-fit.json")
    }

    /// The fit measured at an earlier opening, if any.
    pub fn load() -> Option<Fit> {
        let fit: Fit = serde_json::from_str(&std::fs::read_to_string(Self::file()).ok()?).ok()?;
        Fit::from_parts(fit.from_wrist, fit.width_m)
    }

    fn from_parts(from_wrist: [f32; 3], width_m: f32) -> Option<Fit> {
        let off = sub(from_wrist, FROM_WRIST);
        (from_wrist.iter().all(|v| v.is_finite()) && dot(off, off).sqrt() <= FIT_MOST_OFF_M && (FIT_WIDTHS_M[0]..=FIT_WIDTHS_M[1]).contains(&width_m)).then_some(Fit { from_wrist, width_m })
    }

    /// Kept for the next openings.
    pub fn save(&self) {
        if let Err(e) = serde_json::to_string_pretty(self).map_err(|e| e.to_string()).and_then(|text| std::fs::write(Self::file(), text).map_err(|e| e.to_string())) {
            log_warn!("{}: {e}", Self::file().display(), e = e);
        }
    }
}

/// What a step says happened.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Happened {
    pub taken: Option<usize>,
    pub let_go: Option<usize>,
    /// The other hand pressed its grip in the frame the holding hand let go, or out of reach: that press does nothing.
    pub ignored: Option<usize>,
    /// It passed from this hand to the other, which pressed its grip within reach while this one held it (`taken` is the other).
    pub passed_from: Option<usize>,
    /// A trigger was pressed on its X: VRChat closes its camera panel (in stream mode its camera may stay on, and OSC then says nothing).
    pub closed: bool,
    /// A trigger pressed (hand), and where the laser this assumes for VRChat landed on the panel then, in its units (the X is at [`PANEL_X`]).
    pub pressed: Option<(usize, Option<[f32; 2]>)>,
}

/// VRChat's camera panel as its own rules move it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reckon {
    /// In the room, or in the holding hand's frame while held.
    at: Option<Matrix34>,
    /// Its width against the sandbox's (1 there): VRChat's grab box and X scale with it.
    scale: f32,
    /// A hand took it since it opened.
    taken_since_open: bool,
    held: Option<usize>,
    /// Where it was last shown, in the room.
    last: Option<Matrix34>,
    grips: [bool; 2],
    triggers: [bool; 2],
    closed: bool,
}

impl Reckon {
    /// The camera opened: lay the panel where VRChat does. Nothing until the head and the left controller are tracked.
    /// `wrist`: VRChat's own left wrist when it said it lately ([`left_wrist`]); the controller's pose otherwise.
    pub fn open(&mut self, head: &Matrix34, left: &Matrix34, wrist: Option<&Matrix34>) -> Matrix34 {
        self.open_fitted(head, left, wrist, &Fit::SANDBOX)
    }

    /// [`Reckon::open`] by `fit` (measured for the avatar worn) where VRChat said its wrist.
    pub fn open_fitted(&mut self, head: &Matrix34, left: &Matrix34, wrist: Option<&Matrix34>, fit: &Fit) -> Matrix34 {
        let (panel, scale) = match wrist {
            Some(wrist) => (fit.opening(head, wrist), fit.width_m / PANEL_WIDTH_M),
            None => (opening(head, left), 1.0),
        };
        *self = Self { at: Some(panel), scale, grips: self.grips, triggers: self.triggers, ..Self::default() };
        panel
    }

    /// The picture saw the panel at `frame`, `width_m` wide, while it lies where it opened: laid there. Not while a hand has it, nor once one took it
    /// (where it lies then is the hand's doing, not the opening's). Whether it was.
    pub fn reanchor(&mut self, frame: Matrix34, width_m: f32) -> bool {
        if self.at.is_none() || self.held.is_some() || self.taken_since_open {
            return false;
        }
        self.at = Some(frame);
        self.last = Some(frame);
        self.scale = width_m / PANEL_WIDTH_M;
        true
    }

    /// Where it lies in the room, while no hand has it.
    pub fn lying(&self) -> Option<Matrix34> {
        self.at.filter(|_| self.held.is_none())
    }

    /// Whether a hand took it since it opened.
    pub fn taken_since_open(&self) -> bool {
        self.taken_since_open
    }

    /// One of its units in metres (its picture is [`PANEL_SIZE`] units across).
    pub fn unit(&self) -> f32 {
        PANEL_WIDTH_M * self.scale() / PANEL_SIZE[0]
    }

    fn scale(&self) -> f32 {
        if self.scale > 0.0 { self.scale } else { 1.0 }
    }
    pub fn opened(&self) -> bool {
        self.at.is_some() || self.closed
    }
    pub fn clear(&mut self) {
        *self = Self { grips: self.grips, triggers: self.triggers, ..Self::default() };
    }
    pub fn held_hand(&self) -> Option<usize> {
        self.held
    }
    /// Its X was pressed: hidden until the camera opens again.
    #[cfg(test)]
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// One frame: each controller's pose (left, right), its grip and its trigger held. Where the panel is now, in the room.
    pub fn step(&mut self, hands: [Option<Matrix34>; 2], grips: [bool; 2], triggers: [bool; 2]) -> (Option<Matrix34>, Happened) {
        self.step_inner(hands, grips, triggers)
    }

    fn step_inner(&mut self, hands: [Option<Matrix34>; 2], grips: [bool; 2], triggers: [bool; 2]) -> (Option<Matrix34>, Happened) {
        let mut happened = Happened::default();
        let (was_grips, was_triggers) = (self.grips, self.triggers);
        self.grips = grips;
        self.triggers = triggers;
        let Some(at) = self.at else { return (None, happened) };
        let mut room = match self.held {
            Some(hand) => match hands[hand] {
                Some(pose) => then(&pose, &at),
                // Out of tracking for a moment: where it last was, still held.
                None => return (self.last, happened),
            },
            None => at,
        };
        if let Some(hand) = self.held {
            let other = 1 - hand;
            let pressed = grips[other] && !was_grips[other];
            if !grips[hand] {
                self.held = None;
                self.at = Some(room);
                happened.let_go = Some(hand);
                // Pressed in the very frame the holder let go: nothing (sandbox 2026-11-16: VRChat left it lying).
                if pressed {
                    happened.ignored = Some(other);
                }
            } else if pressed {
                // The other hand takes it over while this one still holds it (Enzo, 2026-11-16, in his headset). This hand's grip, still held,
                // is no new press: it gets it back only by letting go and pressing again. Out of reach, the press does nothing.
                match hands[other].filter(|pose| within_grab_scaled(&room, pose, other, self.scale())) {
                    Some(pose) => {
                        self.held = Some(other);
                        self.at = Some(then(&undone(&pose), &room));
                        happened.taken = Some(other);
                        happened.passed_from = Some(hand);
                    }
                    None => happened.ignored = Some(other),
                }
            }
        } else {
            // Both grips pressed in the same frame: VRChat gives it to the right hand, wherever the hands are (sandbox 2026-11-16, 10 of 11
            // tries, the left 0 to 12 cm nearer its middle). Ours gave it to the left: VRChat's panel then went with one hand and the tools
            // with the other, and stayed so until the camera was reopened (Enzo: "on our camera overlay it does grab the overlay").
            for hand in [1, 0] {
                if grips[hand] && !was_grips[hand] {
                    if let Some(pose) = hands[hand].filter(|pose| within_grab_scaled(&room, pose, hand, self.scale())) {
                        self.taken_since_open = true;
                        self.held = Some(hand);
                        self.at = Some(then(&undone(&pose), &room));
                        happened.taken = Some(hand);
                        break;
                    }
                }
            }
        }
        room = match (self.held, self.at) {
            (Some(hand), Some(at)) => hands[hand].map_or(room, |pose| then(&pose, &at)),
            (_, Some(at)) => at,
            _ => room,
        };
        let unit = self.unit();
        for hand in 0..2 {
            if triggers[hand] && !was_triggers[hand] && self.held != Some(hand) {
                if let Some(pose) = hands[hand] {
                    let (o, d) = laser(&pose);
                    happened.pressed = Some((hand, laser_spot(&room, unit, o, d)));
                    happened.closed |= on_close(&room, unit, &pose);
                }
            }
        }
        if happened.closed {
            *self = Self { grips, triggers, closed: true, ..Self::default() };
            return (None, happened);
        }
        self.last = Some(room);
        (Some(room), happened)
    }
}

#[cfg(test)]
impl Reckon {
    /// A panel already lying at `frame`, for replaying recorded moves from there.
    pub fn lying_at(frame: Matrix34) -> Self {
        Self { at: Some(frame), last: Some(frame), ..Self::default() }
    }
}

/// Replays what real VRChat did in the SteamVR sandbox (2026-11-13, camera4/m1): the poses SteamVR had, and where the pattern reader then
/// saw the panel with both eyes (its ground truth, 0.3 mm spread when still).
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn recorded() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata_camera/sandbox-2026-11-13/reckon.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }
    fn m(v: &Value) -> Matrix34 {
        Matrix34 { m: std::array::from_fn(|i| std::array::from_fn(|j| v[i][j].as_f64().unwrap() as f32)) }
    }
    fn at(p: &Value) -> Matrix34 {
        Matrix34 { m: std::array::from_fn(|i| { let mut r = [0.0; 4]; r[i] = 1.0; r[3] = p[i].as_f64().unwrap() as f32; r }) }
    }
    fn apart(a: &Matrix34, b: &Matrix34) -> f32 {
        dot(sub(column(a, 3), column(b, 3)), sub(column(a, 3), column(b, 3))).sqrt()
    }
    fn turned(a: &Matrix34, b: &Matrix34) -> f32 {
        let tr: f32 = (0..3).map(|c| dot(column(a, c), column(b, c))).sum();
        ((tr - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees()
    }

    #[test]
    fn the_panel_opens_where_vrchat_opened_it() {
        let data = recorded();
        let opens = data["opens"].as_array().unwrap();
        assert!(opens.len() >= 12);
        for open in opens {
            let name = open["name"].as_str().unwrap();
            let predicted = opening(&m(&open["head"]), &m(&open["left"]));
            let truth = m(&open["panel"]);
            let (e, a) = (apart(&predicted, &truth), turned(&predicted, &truth));
            // b3: the head 30 cm from where the arms reach (VRChat's hand stops short); c2: head and hand both turned 30 degrees.
            let (most_m, most_deg) = match name { "spawn2-b3" => (0.03, 2.5), "spawn2-c2" => (0.009, 9.0), _ => (0.005, 2.5) };
            assert!(e <= most_m && a <= most_deg, "{name}: {:.1} mm, {a:.1} degrees", e * 1000.0);
        }
    }

    #[test]
    fn a_grip_takes_it_within_reach_and_it_moves_rigidly_with_that_hand() {
        let data = recorded();
        let mut checked = 0;
        for grab in data["grabs"].as_array().unwrap() {
            let name = grab["name"].as_str().unwrap();
            let before = m(&grab["before"]);
            let hand = if grab["hand"] == "left" { 0 } else { 1 };
            let at_grip = at(&grab["at_grip"]);
            let after = grab["after"].as_array().map(|_| m(&grab["after"]));
            let taken_in_vrchat = after.is_some_and(|a| apart(&a, &before) > 0.02);
            assert_eq!(within_grab(&before, &at_grip, hand), taken_in_vrchat, "{name}: reach");
            let mut reckon = Reckon::lying_at(before);
            let mut hands = [None; 2];
            hands[hand] = Some(at_grip);
            reckon.step(hands, [false; 2], [false; 2]);
            let mut grips = [false; 2];
            grips[hand] = true;
            let (_, happened) = reckon.step(hands, grips, [false; 2]);
            assert_eq!(happened.taken, taken_in_vrchat.then_some(hand), "{name}");
            hands[hand] = Some(m(&grab["moved_hand"]));
            let (moved, _) = reckon.step(hands, grips, [false; 2]);
            let moved = moved.unwrap();
            // Only a taken panel's sighting while held: z16's was read with the hand over its glyphs and is 1.8 cm off a panel that never moved.
            if let Some(during) = grab["during"].as_array().filter(|_| taken_in_vrchat).map(|_| m(&grab["during"])) {
                assert!(apart(&moved, &during) < 0.012 && turned(&moved, &during) < 4.0, "{name}: held {:.1} mm off", apart(&moved, &during) * 1000.0);
            }
            let (released, happened) = reckon.step(hands, [false; 2], [false; 2]);
            assert_eq!(happened.let_go, taken_in_vrchat.then_some(hand));
            hands[hand] = None;
            let (stays, _) = reckon.step(hands, [false; 2], [false; 2]);
            assert!(apart(&stays.unwrap(), &released.unwrap()) < 1e-6, "{name}: let go, it stays");
            if let Some(after) = after {
                assert!(apart(&stays.unwrap(), &after) < 0.012, "{name}: let go at {:.1} mm from VRChat's", apart(&stays.unwrap(), &after) * 1000.0);
            }
            checked += 1;
        }
        assert!(checked >= 8);
    }

    /// Enzo, 2026-11-16 (REQUEST9): "if we grab it with hand A, and while still grabbed we try to grab it with hand B, it should be transferred
    /// to hand B and not stay in hand A". B takes it, A lets go of it although its grip is still held, and A gets it back only with a new press.
    #[test]
    fn a_second_hand_takes_it_over_and_the_first_needs_a_new_press() {
        let panel = facing([0.0, 1.34, -0.35], [0.0, 1.65, 0.0]);
        let near = |dx: f32| Some(Matrix34 { m: [[1.0, 0.0, 0.0, dx], [0.0, 1.0, 0.0, 1.32], [0.0, 0.0, 1.0, -0.31]] });
        for (a, b) in [(1usize, 0usize), (0, 1)] {
            let grips = |a_held: bool, b_held: bool| { let mut g = [false; 2]; g[a] = a_held; g[b] = b_held; g };
            let hands = |a_at: f32, b_at: f32| { let mut h = [None; 2]; h[a] = near(a_at); h[b] = near(b_at); h };
            let mut reckon = Reckon::lying_at(panel);
            reckon.step(hands(0.0, -0.1), [false; 2], [false; 2]);
            assert_eq!(reckon.step(hands(0.0, -0.1), grips(true, false), [false; 2]).1.taken, Some(a));
            // B grips while A holds: B takes it over.
            let (taken, h) = reckon.step(hands(0.0, -0.1), grips(true, true), [false; 2]);
            assert_eq!((h.taken, h.passed_from, h.ignored, h.let_go), (Some(b), Some(a), None, None), "{a} to {b}");
            assert_eq!(reckon.held_hand(), Some(b));
            // B moving moves it; A moving (its grip still held) does not.
            let (moved, _) = reckon.step(hands(0.0, -0.3), grips(true, true), [false; 2]);
            assert!((apart(&taken.unwrap(), &moved.unwrap()) - 0.2).abs() < 1e-5);
            let (still, _) = reckon.step(hands(0.25, -0.3), grips(true, true), [false; 2]);
            assert!(apart(&moved.unwrap(), &still.unwrap()) < 1e-6, "the first hand's held grip does not take it back");
            // B lets go: it lies there; A's grip, held all along, is no new press.
            reckon.step(hands(0.25, -0.3), grips(true, false), [false; 2]);
            let (lying, _) = reckon.step(hands(0.4, -0.3), grips(true, false), [false; 2]);
            assert!(apart(&still.unwrap(), &lying.unwrap()) < 1e-6);
            assert_eq!(reckon.held_hand(), None);
            // A presses again (near it): A takes it.
            reckon.step(hands(-0.3, -0.3), grips(false, false), [false; 2]);
            assert_eq!(reckon.step(hands(-0.3, -0.3), grips(true, false), [false; 2]).1.taken, Some(a));
        }
        // A press out of reach while one holds it does nothing, and is told.
        let mut reckon = Reckon::lying_at(panel);
        reckon.step([near(-0.1), near(0.0)], [false; 2], [false; 2]);
        reckon.step([near(-1.0), near(0.0)], [false, true], [false; 2]);
        let (_, h) = reckon.step([near(-1.0), near(0.0)], [true, true], [false; 2]);
        assert_eq!((h.taken, h.ignored), (None, Some(0)));
        assert_eq!(reckon.held_hand(), Some(1));
        // Pressed in the very frame the holder lets go: it lies there (as VRChat did in the sandbox).
        let mut reckon = Reckon::lying_at(panel);
        reckon.step([near(-0.1), near(0.0)], [false; 2], [false; 2]);
        reckon.step([near(-0.1), near(0.0)], [false, true], [false; 2]);
        let (_, h) = reckon.step([near(-0.1), near(0.0)], [true, false], [false; 2]);
        assert_eq!((h.taken, h.let_go, h.ignored), (None, Some(1), Some(0)));
        assert_eq!(reckon.held_hand(), None);
    }

    /// Every two-hand sequence measured on VRChat's real panel in the sandbox (2026-11-16, `testdata_camera/sandbox-2026-11-16/hands.json`):
    /// replayed through VRChat's grip thresholds as Enzo's binding has them and this reckoning, each hand's move must carry the panel
    /// exactly when it carried VRChat's. Before this change the same-frame presses gave it to the left hand, where VRChat gave it to the right.
    /// Where Enzo's correction of 2026-11-16 changes the verdict (a second hand's press while one holds it takes it over; the sandbox's VRChat
    /// had ignored it), the step's `owner` verdict is the one expected: 13 of the 50 moves.
    #[test]
    fn two_hands_take_and_leave_it_as_vrchat_did() {
        use super::super::vrchat_grip::{Grips, STEAMVR_CLICK, VRCHAT_SHIPPED};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata_camera/sandbox-2026-11-16/hands.json");
        let data: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let panel = m(&data["panel"]);
        let (side, up, nz) = (column(&panel, 0), column(&panel, 1), column(&panel, 2));
        let along = |v: &Value| -> [f32; 3] { let c = |i: usize| v[i].as_f64().unwrap() as f32; std::array::from_fn(|i| side[i] * c(0) + up[i] * c(1) + nz[i] * c(2)) };
        let clicks = [Some(VRCHAT_SHIPPED), Some(STEAMVR_CLICK)];
        let sequences = data["sequences"].as_array().unwrap();
        assert!(sequences.len() >= 20);
        let (mut moves, mut owner) = (0, 0);
        for sequence in sequences {
            let name = sequence["name"].as_str().unwrap();
            let offsets = [sequence["offsets"][0].as_f64().unwrap() as f32, sequence["offsets"][1].as_f64().unwrap() as f32];
            let front = add(add(column(&panel, 3), nz.map(|v| v * 0.05)), up.map(|v| v * -0.02));
            let mut at = [add(front, side.map(|v| -v * offsets[0])), add(front, side.map(|v| v * offsets[1]))];
            let level = |p: [f32; 3]| Matrix34 { m: std::array::from_fn(|i| { let mut r = [0.0; 4]; r[i] = 1.0; r[3] = p[i]; r }) };
            let (mut grips, mut reckon, mut pulls) = (Grips::default(), Reckon::lying_at(panel), [0.0f32; 2]);
            let mut step = |at: &[[f32; 3]; 2], pulls: [f32; 2]| {
                let held = grips.step(pulls.map(Some), clicks, [false; 2]);
                reckon.step([Some(level(at[0])), Some(level(at[1]))], held, [false; 2]).0.unwrap()
            };
            let mut now = step(&at, pulls);
            for s in sequence["steps"].as_array().unwrap() {
                if let Some(set) = s["pull"].as_object() {
                    for (hand, value) in set {
                        pulls[if hand == "left" { 0 } else { 1 }] = value.as_f64().unwrap() as f32;
                    }
                    now = step(&at, pulls);
                } else if s.get("frame").is_some() {
                    now = step(&at, pulls);
                } else {
                    let hand = if s["move"] == "left" { 0 } else { 1 };
                    let by = along(&s["by"]);
                    at[hand] = add(at[hand], by);
                    let after = step(&at, pulls);
                    let moved = sub(column(&after, 3), column(&now, 3));
                    let ours = if dot(sub(moved, by), sub(moved, by)).sqrt() < 0.002 { "followed" } else if dot(moved, moved).sqrt() < 0.001 { "stayed" } else { "other" };
                    let expected = s.get("owner").unwrap_or(&s["vrchat"]).as_str().unwrap();
                    assert_eq!(ours, expected, "{name}: the {} hand moved", s["move"]);
                    owner += s.get("owner").is_some() as usize;
                    now = after;
                    moves += 1;
                }
            }
        }
        assert!(moves >= 50, "{moves}");
        assert_eq!(owner, 13, "the steps Enzo's correction changes");
    }

    #[test]
    fn a_trigger_with_vrchats_laser_on_the_x_closes_it_and_nothing_else_does() {
        let data = recorded();
        for aim in data["aims"].as_array().unwrap().iter().chain(data["aims_m4"].as_array().unwrap()) {
            let name = aim["name"].as_str().unwrap();
            let (panel, unit, hand) = (m(&aim["panel"]), aim["unit"].as_f64().unwrap() as f32, m(&aim["hand"]));
            assert_eq!(on_close(&panel, unit, &hand), aim["closed_in_vrchat"].as_bool().unwrap(), "{name}");
        }
        // Through a step: the press, not the holding of a trigger.
        let aim = data["aims"].as_array().unwrap().iter().find(|a| a["name"] == "cxc60").unwrap().clone();
        let (panel, hand) = (m(&aim["panel"]), m(&aim["hand"]));
        let mut reckon = Reckon::lying_at(panel);
        assert!(!reckon.step([None, Some(hand)], [false; 2], [false; 2]).1.closed);
        let (shown, happened) = reckon.step([None, Some(hand)], [false; 2], [false, true]);
        assert!(happened.closed && shown.is_none() && reckon.closed());
        assert!(reckon.step([None, Some(hand)], [false; 2], [false, true]).0.is_none(), "closed until it opens again");
        // A trigger already held when the laser comes onto the X is not a press.
        let mut reckon = Reckon::lying_at(panel);
        reckon.step([None, None], [false; 2], [false, true]);
        assert!(!reckon.step([None, Some(hand)], [false; 2], [false, true]).1.closed);
    }

    /// Enzo's picture D (2026-11-16): with the laser on VRChat's X, our "3D of people" beside it took the click too. A laser within 4 cm of the
    /// X is VRChat's alone; the column's tiles further away keep theirs.
    #[test]
    fn a_laser_by_vrchats_x_is_left_to_vrchat() {
        use super::super::zones::{self, Zone};
        let panel = facing([0.0, 1.34, -0.35], [0.0, 1.65, 0.0]);
        let unit = PANEL_WIDTH_M / PANEL_SIZE[0];
        let (side, up, nz) = (column(&panel, 0), column(&panel, 1), column(&panel, 2));
        // A laser straight at a place of the panel's plane, in its units from its middle (y down), from 30 cm in front.
        let at = |x: f32, y: f32| -> ([f32; 3], [f32; 3]) {
            let p = add(add(column(&panel, 3), side.map(|v| v * x * unit)), up.map(|v| v * -y * unit));
            (add(p, nz.map(|v| v * 0.3)), nz.map(|v| -v))
        };
        let (o, d) = at(CLOSE_AT[0], CLOSE_AT[1]);
        assert!(by_the_close(&panel, unit, o, d));
        // Picture D's spot: the top right corner of our top tile, the one nearest the X.
        let people = zones::in_units(Zone::CameraPeople).rect;
        let (o, d) = at(people.max.x - 4.0, people.min.y + 4.0);
        assert!(by_the_close(&panel, unit, o, d), "picture D's spot is left to VRChat");
        // The middle of every tile of the column, and of the line under the panel, stays ours.
        for zone in [Zone::CameraPeople, Zone::CameraWorld, Zone::CameraRecord, Zone::CameraQr, Zone::CameraCaption, Zone::CameraAct] {
            let c = zones::in_units(zone).rect.center();
            let (o, d) = at(c.x, c.y);
            assert!(!by_the_close(&panel, unit, o, d), "{zone:?}");
        }
        // How much of the top tile is given up: under a fifth of it.
        let mut given = 0;
        for i in 0..92 {
            for j in 0..60 {
                let (o, d) = at(people.min.x + i as f32 + 0.5, people.min.y + j as f32 + 0.5);
                given += by_the_close(&panel, unit, o, d) as usize;
            }
        }
        assert!(given * 5 < 92 * 60, "{given} of {}", 92 * 60);
        // Every recorded press that closed VRChat's camera was by the X.
        let data = recorded();
        for aim in data["aims"].as_array().unwrap().iter().chain(data["aims_m4"].as_array().unwrap()).filter(|a| a["closed_in_vrchat"].as_bool().unwrap()) {
            let (o, d) = laser(&m(&aim["hand"]));
            assert!(by_the_close(&m(&aim["panel"]), aim["unit"].as_f64().unwrap() as f32, o, d), "{}", aim["name"]);
        }
    }

    /// VRChat's own wrist gives where the panel opens (m3, poses in SteamVR's standing space); the controller rule too, within reach.
    #[test]
    fn the_panel_opens_by_vrchats_own_wrist_whatever_the_avatar() {
        let data = recorded();
        let opens = data["wrist_opens"].as_array().unwrap();
        assert!(opens.len() >= 9);
        for open in opens {
            let name = open["name"].as_str().unwrap();
            let wrist: Vec<f32> = open["wrist"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
            let wrist = unity_pose(wrist.try_into().unwrap());
            let (head, truth) = (m(&open["head"]), m(&open["panel"]));
            let e = apart(&opening_by_wrist(&head, &wrist), &truth);
            assert!(e <= if name == "spawn4-w3" { 0.025 } else { 0.011 }, "{name}: {:.1} mm", e * 1000.0);
            let a = turned(&opening_by_wrist(&head, &wrist), &truth);
            assert!(a < 5.5, "{name}: {a:.1} degrees");
            let by_controller = apart(&opening(&head, &m(&open["left"])), &truth);
            assert!(by_controller <= if name == "spawn4-w3" { 0.025 } else { 0.015 }, "{name}: controller rule {:.1} mm", by_controller * 1000.0);
        }
    }

    /// Request 18: what one opening's sighting measures (the offset from VRChat's wrist, the width) lays the panel right at the next openings, from
    /// wherever the wrist is then: the sandbox's real opens (m3), one measured, the others predicted from it within what the sandbox's own fit does.
    /// A panel measured larger than the sandbox's (an avatar small against its person) moves and grows the parts with it; a misreading is refused.
    #[test]
    fn a_measured_fit_lays_the_next_openings() {
        let data = recorded();
        let opens = data["wrist_opens"].as_array().unwrap();
        let wrist_of = |open: &Value| {
            let v: Vec<f32> = open["wrist"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
            unity_pose(v.try_into().unwrap())
        };
        let first = &opens[0];
        let fit = Fit::from_sighting(&wrist_of(first), &m(&first["panel"]), PANEL_WIDTH_M).unwrap();
        assert!(fit.agrees(&Fit::SANDBOX), "the sandbox's own opens give the sandbox's fit: {fit:?}");
        for open in opens.iter().skip(1) {
            let name = open["name"].as_str().unwrap();
            let e = apart(&fit.opening(&m(&open["head"]), &wrist_of(open)), &m(&open["panel"]));
            assert!(e <= if name == "spawn4-w3" { 0.03 } else { 0.015 }, "{name}: {:.1} mm", e * 1000.0);
        }
        // Another avatar: VRChat lays its panel 5 cm lower and 2 cm in, from its wrist, 10 % smaller. Measured once, the next opening is there.
        let truth = Fit { from_wrist: [FROM_WRIST[0] + 0.02, FROM_WRIST[1] - 0.05, FROM_WRIST[2]], width_m: PANEL_WIDTH_M * 0.9 };
        let (head, wrist) = (m(&first["head"]), wrist_of(first));
        let seen = truth.opening(&head, &wrist);
        let measured = Fit::from_sighting(&wrist, &seen, truth.width_m).unwrap();
        assert!(measured.agrees(&truth) && !measured.agrees(&Fit::SANDBOX), "{measured:?}");
        let mut reckon = Reckon::default();
        let other = &opens[3];
        let laid = reckon.open_fitted(&m(&other["head"]), &m(&other["left"]), Some(&wrist_of(other)), &measured);
        assert!(apart(&laid, &truth.opening(&m(&other["head"]), &wrist_of(other))) < 0.001);
        assert!((reckon.unit() - truth.width_m / PANEL_SIZE[0]).abs() < 1e-6, "its unit follows its width");
        // A sighting 40 cm off, or 1 m wide, is no panel of VRChat's.
        let far = Matrix34 { m: std::array::from_fn(|i| { let mut r = seen.m[i]; r[3] += if i == 0 { 0.4 } else { 0.0 }; r }) };
        assert!(Fit::from_sighting(&wrist, &far, PANEL_WIDTH_M).is_none());
        assert!(Fit::from_sighting(&wrist, &seen, 1.0).is_none());
    }

    /// Request 18: a sighting re-lays the panel only while it lies where it opened: not while a hand has it, nor after one took it.
    #[test]
    fn a_sighting_re_lays_the_panel_only_where_it_opened() {
        let data = recorded();
        // A grab VRChat took (its panel moved).
        let grab = data["grabs"].as_array().unwrap().iter().find(|g| g["after"].as_array().is_some_and(|_| apart(&m(&g["after"]), &m(&g["before"])) > 0.02)).unwrap();
        let before = m(&grab["before"]);
        let side = if grab["hand"] == "left" { 0 } else { 1 };
        let mut reckon = Reckon::lying_at(before);
        let moved = Matrix34 { m: std::array::from_fn(|i| { let mut r = before.m[i]; r[3] += if i == 1 { -0.05 } else { 0.0 }; r }) };
        assert!(reckon.reanchor(moved, PANEL_WIDTH_M * 1.1));
        let (now, _) = reckon.step([None, None], [false; 2], [false; 2]);
        assert!(apart(&now.unwrap(), &moved) < 1e-6);
        assert!((reckon.unit() - PANEL_WIDTH_M * 1.1 / PANEL_SIZE[0]).abs() < 1e-6);
        // Taken: no more re-laying from the opening's sightings.
        let mut reckon = Reckon::lying_at(before);
        let mut hands = [None; 2];
        hands[side] = Some(at(&grab["at_grip"]));
        reckon.step(hands, [false; 2], [false; 2]);
        assert!(!reckon.taken_since_open());
        let mut grips = [false; 2];
        grips[side] = true;
        let (_, happened) = reckon.step(hands, grips, [false; 2]);
        assert_eq!(happened.taken, Some(side));
        assert!(!reckon.reanchor(moved, PANEL_WIDTH_M), "held");
        let (_, happened) = reckon.step(hands, [false; 2], [false; 2]);
        assert_eq!(happened.let_go, Some(side));
        assert!(reckon.taken_since_open() && !reckon.reanchor(moved, PANEL_WIDTH_M), "let go where the hand left it: not the opening's");
    }

    /// m3's grips, with the controller where SteamVR had it (the sandbox commanded it 12 cm off, by the standing offset): the same rule as m1's.
    #[test]
    fn m3s_grips_follow_the_same_reach() {
        let data = recorded();
        for grab in data["grabs_m3"].as_array().unwrap() {
            let name = grab["name"].as_str().unwrap();
            let (before, after) = (m(&grab["before"]), m(&grab["after"]));
            assert_eq!(within_grab(&before, &at(&grab["at_grip"]), 1), apart(&before, &after) > 0.02, "{name}");
        }
    }

    /// 2026-11-14 (camera5/r): 24 grips with the controller 8 to 17 cm in front of the panel, level, tilted and turned, each hand; whether
    /// VRChat took the panel, judged on the eye pictures. The rule agrees on every one (the controller's origin as the grab point did not on 8 to 10).
    #[test]
    fn vrchat_measures_a_grab_from_beside_the_controller() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata_camera/sandbox-2026-11-14/reach.json");
        let data: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let probes = data["probes"].as_array().unwrap();
        assert_eq!(probes.len(), 24);
        let mut by_origin_wrong = 0;
        for probe in probes {
            let name = probe["name"].as_str().unwrap();
            let (panel, controller) = (m(&probe["panel"]), m(&probe["controller"]));
            let hand = probe["hand"].as_u64().unwrap() as usize;
            let took = probe["vrchat_took"].as_bool().unwrap();
            assert_eq!(within_grab(&panel, &controller, hand), took, "{name}");
            by_origin_wrong += usize::from(within_reach(&panel, column(&controller, 3), 0.14) != took);
        }
        // The controller's origin took 8 of them wrongly, 10 with the two exactly at its 14 cm.
        assert!(by_origin_wrong >= 8, "what the old rule got wrong: {by_origin_wrong}");
    }

    /// VRChat said its head was at (0, 1.65, 0) turned (20, 0, 0) when SteamVR's head was at (0, 1.65, 0) looking 20 degrees down (sandbox).
    #[test]
    fn vrchats_poses_turn_into_steamvrs_axes() {
        let head = unity_pose([0.0, 1.65, 0.0, 20.0, 0.0, 0.0]);
        let forward = turn(&head, [0.0, 0.0, -1.0]);
        assert!((forward[1] + 20f32.to_radians().sin()).abs() < 1e-4 && forward[2] < -0.9, "{forward:?}");
        assert!((column(&head, 3)[1] - 1.65).abs() < 1e-6);
        let ahead = unity_pose([0.0, 0.0, 0.5, 0.0, 30.0, 0.0]);
        assert!((column(&ahead, 3)[2] + 0.5).abs() < 1e-6, "Unity's forward is SteamVR's -Z");
        // Turned 30 degrees about Y in Unity (to its right) looks to SteamVR's right too.
        assert!(turn(&ahead, [0.0, 0.0, -1.0])[0] > 0.45);
    }

    #[test]
    fn a_hand_out_of_tracking_keeps_it_where_it_was() {
        let panel = facing([0.0, 1.34, -0.35], [0.0, 1.65, 0.0]);
        let hand = Some(Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.32], [0.0, 0.0, 1.0, -0.31]] });
        let mut reckon = Reckon::lying_at(panel);
        reckon.step([None, hand], [false; 2], [false; 2]);
        let (held, _) = reckon.step([None, hand], [false, true], [false; 2]);
        let (lost, h) = reckon.step([None, None], [false, true], [false; 2]);
        assert_eq!(h.let_go, None);
        assert!(apart(&held.unwrap(), &lost.unwrap()) < 1e-6);
    }
}
