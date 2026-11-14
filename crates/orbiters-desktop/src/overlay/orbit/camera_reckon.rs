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
//!   was let go. A grip pressed by the other hand while one holds it does nothing, and it does not pass to that hand when the first lets go
//!   (a new press is needed). Its camera modes do not move it.
//! - **Close.** Its X closes it; VRChat's laser from the controller points [`LASER_DOWN_DEG`] below the controller's −Z. A trigger pressed while
//!   that laser is on the X is seen here ([`Reckon::step`]), with no overlay over VRChat's button, so VRChat gets its click whole.

use orbiters_openvr::Matrix34;

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
/// How far below the controller's −Z VRChat's laser points: VRChat closed its camera for aims 62 and 67 degrees up from 12 cm in front, not 57
/// or 75 (m4), and for 62 not 55 (m1).
pub const LASER_DOWN_DEG: f32 = 64.5;
/// How near the X a laser's spot counts as on it, in the panel's units (the button is about 30 across, the next one 40 away; 5 degrees of aim
/// are 15 units there).
pub const CLOSE_RADIUS: f32 = 13.0;

/// VRChat's own left wrist, as it says over OSC (`/tracking/vrsystem/leftwrist/pose`, sent to Orbiters' port every frame): where its avatar's
/// hand really is. With an avatar whose arm cannot reach the controller, the panel opens by that hand, not by the controller (sandbox r2: 24 cm).
/// Both wrists (left, right), as VRChat said them lately.
static WRISTS: std::sync::Mutex<[Option<(std::time::Instant, [f32; 6])>; 2]> = std::sync::Mutex::new([None; 2]);

/// The controller heard VRChat say where a wrist is (0 left, 1 right): x, y, z (metres) and Euler x, y, z (degrees), in Unity's axes.
pub fn wrist_said(hand: usize, values: [f32; 6]) {
    if let Ok(mut wrists) = WRISTS.lock() {
        wrists[hand.min(1)] = Some((std::time::Instant::now(), values));
    }
}

/// VRChat's wrist (0 left, 1 right) in SteamVR's standing space, if it said so lately.
pub fn wrist(hand: usize, within: std::time::Duration) -> Option<Matrix34> {
    let (at, values) = WRISTS.lock().ok()?[hand.min(1)]?;
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
    let point = [if hand == 0 { -GRAB_POINT[0] } else { GRAB_POINT[0] }, GRAB_POINT[1], GRAB_POINT[2]];
    within_reach(panel, add(column(pose, 3), turn(pose, point)), GRAB_REACH_M)
}

/// Whether a point is within `reach` of the panel's grab box.
pub fn within_reach(panel: &Matrix34, point: [f32; 3], reach: f32) -> bool {
    let p = undone(panel);
    let at = add(turn(&p, point), column(&p, 3));
    let out = |v: f32, lo: f32, hi: f32| (lo - v).max(v - hi).max(0.0);
    let d = [out(at[0], GRAB_BOX[0], GRAB_BOX[1]), out(at[1], GRAB_BOX[2], GRAB_BOX[3]), at[2]];
    dot(d, d).sqrt() <= reach
}

/// VRChat's laser from a controller: where it starts and the way it goes.
pub fn laser(hand: &Matrix34) -> ([f32; 3], [f32; 3]) {
    let (s, c) = LASER_DOWN_DEG.to_radians().sin_cos();
    (column(hand, 3), turn(hand, [0.0, -s, -c]))
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

/// Whether a laser from `hand` is on the panel's X.
pub fn on_close(panel: &Matrix34, panel_unit: f32, hand: &Matrix34) -> bool {
    let (o, d) = laser(hand);
    laser_spot(panel, panel_unit, o, d).is_some_and(|s| (s[0] - PANEL_X[0]).hypot(s[1] - PANEL_X[1]) <= CLOSE_RADIUS)
}

/// What a step says happened.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Happened {
    pub taken: Option<usize>,
    pub let_go: Option<usize>,
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
        let panel = wrist.map_or_else(|| opening(head, left), |wrist| opening_by_wrist(head, wrist));
        *self = Self { at: Some(panel), grips: self.grips, triggers: self.triggers, ..Self::default() };
        panel
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
            if !grips[hand] {
                self.held = None;
                self.at = Some(room);
                happened.let_go = Some(hand);
            }
        } else {
            for hand in 0..2 {
                if grips[hand] && !was_grips[hand] {
                    if let Some(pose) = hands[hand].filter(|pose| within_grab(&room, pose, hand)) {
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
        let unit = PANEL_WIDTH_M / PANEL_SIZE[0];
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

    #[test]
    fn the_second_hand_neither_takes_it_nor_gets_it_handed_over() {
        let panel = facing([0.0, 1.34, -0.35], [0.0, 1.65, 0.0]);
        let near = |dx: f32| Some(Matrix34 { m: [[1.0, 0.0, 0.0, dx], [0.0, 1.0, 0.0, 1.32], [0.0, 0.0, 1.0, -0.31]] });
        let mut reckon = Reckon::lying_at(panel);
        reckon.step([near(-0.1), near(0.0)], [false; 2], [false; 2]);
        assert_eq!(reckon.step([near(-0.1), near(0.0)], [false, true], [false; 2]).1.taken, Some(1));
        // The left grips too, while the right holds: nothing; the left moving does not move it.
        let (a, h) = reckon.step([near(-0.1), near(0.0)], [true, true], [false; 2]);
        assert_eq!(h.taken, None);
        let (b, _) = reckon.step([near(-0.3), near(0.0)], [true, true], [false; 2]);
        assert!(apart(&a.unwrap(), &b.unwrap()) < 1e-6);
        // The right lets go: it stays, and the left (still gripping) does not get it.
        reckon.step([near(-0.3), near(0.0)], [true, false], [false; 2]);
        let (c, _) = reckon.step([near(-0.5), near(0.0)], [true, false], [false; 2]);
        assert!(apart(&b.unwrap(), &c.unwrap()) < 1e-6);
        assert_eq!(reckon.held_hand(), None);
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
