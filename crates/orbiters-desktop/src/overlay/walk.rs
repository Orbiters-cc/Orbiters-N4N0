//! Walk boost: while the stick VRChat walks with is pushed, the play space is dragged the same way (the working copy
//! of SteamVR's chaperone setup, previewed live like OVR Advanced Settings' space drag), so the avatar covers more
//! ground than VRChat's own walking. The chaperone's walls move the other way, so they stay on the room's walls. The
//! play space goes back where it was when VRChat loads another world (the avatar respawns anyway), when VRChat
//! closes and when the overlay ends.

use super::actions::Walk;
use orbiters_core::settings::{Follow, Hand, WalkBoostSettings};
use orbiters_openvr::chaperone::Quad;
use orbiters_openvr::{self as vr, Matrix34, Pose, Universe, MAX_DEVICES};
use std::time::Instant;

/// VRChat's running speed (m/s), reached with the stick pushed all the way.
pub const RUN_SPEED: f32 = 4.0;
/// Stick travel ignored around the centre.
const DEADZONE: f32 = 0.12;
/// A frame longer than this moves as far as this (no jump after a stall).
const MAX_STEP_S: f32 = 0.05;

/// How far the stick is pushed (0 to 1, the dead zone removed).
pub fn push(stick: [f32; 2]) -> f32 {
    let len = (stick[0] * stick[0] + stick[1] * stick[1]).sqrt();
    ((len - DEADZONE) / (1.0 - DEADZONE)).clamp(0.0, 1.0)
}

/// The horizontal direction the stick points in the standing universe, scaled by [`push`]: forward is where `frame`
/// (the head or the walking hand) looks (its −Z), right its +X.
pub fn direction(stick: [f32; 2], frame: &Matrix34) -> [f32; 3] {
    let amount = push(stick);
    if amount == 0.0 {
        return [0.0; 3];
    }
    let flat = |v: [f32; 3]| {
        let len = (v[0] * v[0] + v[2] * v[2]).sqrt();
        if len < 1e-4 {
            None
        } else {
            Some([v[0] / len, 0.0, v[2] / len])
        }
    };
    let m = &frame.m;
    let Some(forward) = flat([-m[0][2], -m[1][2], -m[2][2]]) else { return [0.0; 3] };
    // Right is forward turned a quarter clockwise seen from above (the frame's own X may tilt with a rolled hand).
    let right = [-forward[2], 0.0, forward[0]];
    let len = (stick[0] * stick[0] + stick[1] * stick[1]).sqrt().max(1e-6);
    let (x, y) = (stick[0] / len, stick[1] / len);
    [(right[0] * x + forward[0] * y) * amount, 0.0, (right[2] * x + forward[2] * y) * amount]
}

/// The play space as it was before walking, and how far the person was carried since (in the standing universe).
#[derive(Clone, Debug, PartialEq)]
pub struct Carried {
    pub standing: Matrix34,
    pub seated: Option<Matrix34>,
    pub bounds: Vec<Quad>,
    pub offset: [f32; 3],
}

impl Carried {
    /// The working copy that carries the person `offset` metres: the universes' origins move by −offset in the room
    /// (so every tracked position reads +offset), and the walls' corners by +offset in the standing universe (so they
    /// stay where they are in the room).
    pub fn shifted(&self) -> (Matrix34, Option<Matrix34>, Vec<Quad>) {
        let r = &self.standing.m;
        let o = self.offset;
        // The offset in raw tracking space: the standing universe's rotation applied to it.
        let raw = [
            r[0][0] * o[0] + r[0][1] * o[1] + r[0][2] * o[2],
            r[1][0] * o[0] + r[1][1] * o[1] + r[1][2] * o[2],
            r[2][0] * o[0] + r[2][1] * o[1] + r[2][2] * o[2],
        ];
        let moved = |m: &Matrix34| {
            let mut out = *m;
            for (row, d) in out.m.iter_mut().zip(raw) {
                row[3] -= d;
            }
            out
        };
        let bounds = self.bounds.iter().map(|q| Quad { corners: q.corners.map(|c| [c[0] + o[0], c[1] + o[1], c[2] + o[2]]) }).collect();
        (moved(&self.standing), self.seated.as_ref().map(moved), bounds)
    }
}

/// What the overlay loop knows of VRChat for walking.
pub struct Moment<'a> {
    pub settings: &'a WalkBoostSettings,
    /// VRChat runs, is in a world and has the scene (not a loading screen, not the dashboard).
    pub walking_allowed: bool,
    /// The world and instance: another one puts the play space back.
    pub place: Option<&'a str>,
}

pub struct WalkBoost {
    actions: Option<Walk>,
    carried: Option<Carried>,
    place: Option<String>,
    last: Option<Instant>,
    /// The manifest could not be loaded: said once.
    failed: Option<String>,
}

impl WalkBoost {
    /// Walk boost with the overlay's actions (none when SteamVR's input could not load them: `failed` says why).
    pub fn new(actions: Option<Walk>, failed: Option<String>) -> Self {
        Self { actions, carried: None, place: None, last: None, failed }
    }

    /// Why walk boost cannot work, if it cannot.
    pub fn unavailable(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    /// The boost is on and VRChat may be walking: the overlay loop should run fast.
    pub fn active(&self, m: &Moment) -> bool {
        self.actions.is_some() && m.settings.enabled && m.settings.strength > 0.0 && m.walking_allowed
    }

    /// One frame: reads the stick and carries the person further.
    pub fn update(&mut self, steamvr: &vr::Client, m: &Moment) {
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |t| now.duration_since(t).as_secs_f32()).min(MAX_STEP_S);
        self.last = Some(now);
        if self.place.as_deref() != m.place {
            // Another world or instance (or VRChat closed): the play space goes back while the avatar respawns.
            self.place = m.place.map(str::to_string);
            self.reset(steamvr);
        }
        let Some(walk) = self.actions else { return };
        if !self.active(m) || dt == 0.0 {
            return;
        }
        // The overlay loop read every action set this frame (actions::Actions::update).
        let Ok(input) = steamvr.input() else { return };
        let action = if m.settings.hand == Hand::Left { walk.left } else { walk.right };
        let Ok(stick) = input.analog(action) else { return };
        if !stick.active {
            return;
        }
        let mut poses = [Pose::default(); MAX_DEVICES];
        steamvr.poses(Universe::Standing, &mut poses);
        let frame = match m.settings.follow {
            Follow::Head => poses[0],
            Follow::Hand => {
                let role = if m.settings.hand == Hand::Left { vr::ControllerRole::LeftHand } else { vr::ControllerRole::RightHand };
                match (0..MAX_DEVICES as u32).find(|&i| steamvr.controller_role(i) == role) {
                    Some(i) => poses[i as usize],
                    None => poses[0],
                }
            }
        };
        if !frame.pose_is_valid {
            return;
        }
        let dir = direction([stick.x, stick.y], &frame.device_to_absolute);
        if dir == [0.0; 3] {
            return;
        }
        let speed = RUN_SPEED * m.settings.strength.clamp(0.0, 1.0) * WalkBoostSettings::MAX_EXTRA;
        let Ok(setup) = steamvr.chaperone_setup() else { return };
        if self.carried.is_none() {
            setup.revert_working_copy();
            let Some(standing) = setup.working_standing_zero() else { return };
            self.carried =
                Some(Carried { standing, seated: setup.working_seated_zero(), bounds: setup.working_collision_bounds().unwrap_or_default(), offset: [0.0; 3] });
        }
        let carried = self.carried.as_mut().expect("set above");
        for (o, d) in carried.offset.iter_mut().zip(dir) {
            *o += d * speed * dt;
        }
        let (standing, seated, bounds) = carried.shifted();
        setup.set_working_standing_zero(&standing);
        if let Some(seated) = seated {
            setup.set_working_seated_zero(&seated);
        }
        if !bounds.is_empty() {
            setup.set_working_collision_bounds(&bounds);
        }
        setup.show_working_set_preview();
    }

    /// Puts the play space back where it was.
    pub fn reset(&mut self, steamvr: &vr::Client) {
        if self.carried.take().is_some() {
            if let Ok(setup) = steamvr.chaperone_setup() {
                setup.revert_working_copy();
                setup.hide_working_set_preview();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaw(degrees: f32) -> Matrix34 {
        let (s, c) = degrees.to_radians().sin_cos();
        // A rotation about +Y (a head turned left by `degrees`).
        Matrix34 { m: [[c, 0.0, s, 0.0], [0.0, 1.0, 0.0, 1.7], [-s, 0.0, c, 0.0]] }
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn the_stick_points_where_the_head_looks() {
        // Looking down −Z: forward is −Z, right is +X.
        assert!(close(direction([0.0, 1.0], &yaw(0.0)), [0.0, 0.0, -1.0]));
        assert!(close(direction([1.0, 0.0], &yaw(0.0)), [1.0, 0.0, 0.0]));
        // Turned left a quarter: forward is −X.
        assert!(close(direction([0.0, 1.0], &yaw(90.0)), [-1.0, 0.0, 0.0]));
        // The dead zone, and half a push.
        assert_eq!(direction([0.05, 0.05], &yaw(0.0)), [0.0; 3]);
        let half = direction([0.0, 0.56], &yaw(0.0));
        assert!((half[2] + 0.5).abs() < 1e-3, "{half:?}");
    }

    #[test]
    fn carrying_moves_tracking_and_keeps_the_walls_in_the_room() {
        let standing = yaw(30.0);
        let wall = Quad { corners: [[1.0, 0.0, 1.0], [1.0, 2.0, 1.0], [1.0, 2.0, -1.0], [1.0, 0.0, -1.0]] };
        let carried = Carried { standing, seated: None, bounds: vec![wall], offset: [0.5, 0.0, -2.0] };
        let (moved, _, bounds) = carried.shifted();
        // A device fixed in the room (raw p) reads `offset` further in the standing universe.
        let raw = [0.3, 1.2, 0.4];
        let standing_of = |z: &Matrix34, p: [f32; 3]| {
            let d = [p[0] - z.m[0][3], p[1] - z.m[1][3], p[2] - z.m[2][3]];
            [0, 1, 2].map(|c| z.m[0][c] * d[0] + z.m[1][c] * d[1] + z.m[2][c] * d[2])
        };
        let before = standing_of(&standing, raw);
        let after = standing_of(&moved, raw);
        assert!(close([after[0] - before[0], after[1] - before[1], after[2] - before[2]], [0.5, 0.0, -2.0]));
        // A wall corner sits at the same place in the room.
        let room = |z: &Matrix34, c: [f32; 3]| [0, 1, 2].map(|r| z.m[r][0] * c[0] + z.m[r][1] * c[1] + z.m[r][2] * c[2] + z.m[r][3]);
        assert!(close(room(&standing, wall.corners[0]), room(&moved, bounds[0].corners[0])));
    }
}
