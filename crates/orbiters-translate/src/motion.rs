//! Where VRChat's world is in SteamVR's tracking space now, as the person walks and turns with the sticks.
//!
//! A translation is laid over a sign in SteamVR's standing space. Walking with the stick and turning with it move
//! VRChat's world through that space (the play area goes with the player), so an anchor kept in tracking space slides off
//! its sign as soon as the person moves: Enzo's "it doesn't stick well to the vrchat world space as we move around"
//! (2026-11-13). [`Locomotion`] follows that motion and keeps translations in an *anchor space* that moves with the world:
//! `to_tracking` takes a point of the anchor space to where it is in tracking space now.
//!
//! What it follows (measured in the VR sandbox with real VRChat, see done4-translate.md):
//! - walking: the walking stick's direction, about the head's heading (VRChat walks where the head faces), at the speed
//!   VRChat says over OSC (the avatar's `VelocityX`/`VelocityZ`: their length does not depend on which way the avatar's
//!   body faces), divided by the world's scale; without OSC, at VRChat's default speed for that push of the stick;
//!   up and down (stairs, jumps, falls) from `VelocityY`;
//! - turning: VRChat's turning stick, smoothly or in steps as VRChat's comfort setting says, as the immersive 3D view
//!   already does ([`TurnStick`]), about the head (VRChat turns the play area about the player);
//! - what it gets wrong (another walking speed, a world's own movers, a scale guess) is corrected when a look finds a
//!   followed line again ([`Locomotion::correct`]): the whole anchor space moves by what that re-reading showed, and the
//!   walking speed learns from it ([`Locomotion::gain`]).

use crate::geometry::{self, add, axis, compose, normalized, origin, scale, V3, IDENTITY, M34};

/// Under this a stick is at rest (VRChat's own dead zone is about this).
pub const STICK_REST: f32 = 0.15;
/// VRChat's speeds when a world leaves them as they are (`VRCPlayerApi` defaults): run 4 m/s, walk 2 m/s, strafe 2 m/s.
/// A stick pushed all the way runs.
pub const DEFAULT_RUN: f32 = 4.0;
pub const DEFAULT_STRAFE: f32 = 2.0;
/// VRChat's speeds come over OSC a little late: after the stick is let go they still say how far the player went
/// (the sandbox, 2026-11-13: a half-second push went 1.97 m; counting VRChat's speed only while the stick was pushed
/// made 1.3 m of it). They count this long after a stick was let go.
pub const COAST_S: f64 = 0.6;
/// OSC speeds older than this no longer count (VRChat sends a value when it changes: a link that went quiet while the
/// stick is still pushed must not walk the anchors on and on).
const SPEED_FRESH_S: f64 = 0.5;
/// How fast a smooth turn goes with the stick all the way, radians a second; how far a step goes. The immersive view's
/// figures, corrected by re-reading (measured in the sandbox, done4-translate.md).
pub const TURNS_PER_S: f32 = 1.75;
pub const TURN_STEP: f32 = std::f32::consts::FRAC_PI_6;
const TURN_REST: f32 = 0.2;
const TURN_PUSHED: f32 = 0.7;

/// VRChat's turning stick: smoothly, or a step each push (the same rule as the immersive 3D view's
/// `overlay::worlds::stereo::TurnStick`).
#[derive(Clone, Copy, Debug, Default)]
pub struct TurnStick {
    pushed: bool,
}

impl TurnStick {
    /// How far the world turns now, in radians about up: pushed right, what is ahead goes to the left.
    pub fn step(&mut self, stick: f32, seconds: f32, in_steps: bool, per_s: f32) -> f32 {
        if in_steps {
            if stick.abs() < TURN_REST {
                self.pushed = false;
            } else if stick.abs() >= TURN_PUSHED && !self.pushed {
                self.pushed = true;
                return TURN_STEP * stick.signum();
            }
            return 0.0;
        }
        if stick.abs() < TURN_REST {
            return 0.0;
        }
        (stick.abs() - TURN_REST) / (1.0 - TURN_REST) * stick.signum() * per_s * seconds
    }
}

/// One turn of the overlay: the sticks and the head now.
#[derive(Clone, Copy, Debug)]
pub struct Input {
    /// Seconds since the start, monotonic.
    pub now: f64,
    /// The head in tracking space.
    pub head: M34,
    /// The walking stick (x right, y forward), -1 to 1.
    pub walk: [f32; 2],
    /// The turning stick's side to side, -1 to 1.
    pub turn: f32,
    /// VRChat turns in steps (its comfort turning).
    pub turn_in_steps: bool,
    /// A menu of VRChat's is up: the sticks then move in its lists, not the player.
    pub menu_up: bool,
}

/// What VRChat said over OSC lately.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Osc {
    /// The avatar's velocity, its own axes (x right, y up, z forward), world metres a second, and when it came.
    pub velocity: Option<([f32; 3], f64)>,
    /// The avatar's eye height in the world, metres (`/avatar/eyeheight`).
    pub eye_height: Option<f32>,
    /// The avatar's turning speed about up (`AngularY`, degrees a second, positive turning right), and when it came.
    pub angular_y: Option<(f32, f64)>,
}

#[derive(Clone, Debug)]
pub struct Locomotion {
    /// Anchor space to tracking space now.
    pub to_tracking: M34,
    pub osc: Osc,
    /// The person's real height (VRChat's Player Height), metres: with the avatar's eye height, the world's scale.
    pub player_height: Option<f32>,
    /// What re-reading taught: how much further than estimated the world moves for the speed VRChat said.
    pub gain: f32,
    turn: TurnStick,
    last: Option<f64>,
    /// Walked and turned since the last correction (metres in tracking space, radians), for its log and to learn `gain`.
    pub walked: f32,
    pub turned: f32,
    walked_by: V3,
    /// Walked (metres) and turned (radians) in all, for [`crate::track::Tracker::moved`].
    pub total: [f32; 2],
    /// The walking stick's last push and when; the turning stick's last push time.
    walk_last: Option<([f32; 2], f64)>,
    turn_last: Option<f64>,
    /// The head's heading at the last turn (radians about up), to take the head's own turning out of `AngularY`.
    heading: Option<f32>,
}

impl Default for Locomotion {
    fn default() -> Self {
        Self { to_tracking: IDENTITY, osc: Osc::default(), player_height: None, gain: 1.0, turn: TurnStick::default(), last: None, walked: 0.0, turned: 0.0, walked_by: [0.0; 3], total: [0.0; 2], walk_last: None, turn_last: None, heading: None }
    }
}

/// The horizontal of a direction, unit (or none when it points straight up or down).
fn level(v: V3) -> Option<V3> {
    let h = [v[0], 0.0, v[2]];
    (geometry::norm(h) > 1e-3).then(|| normalized(h))
}

/// A turn of `radians` about the vertical through `at` (positive: what is ahead, -Z, goes to the left).
pub fn turn_about(at: V3, radians: f32) -> M34 {
    let r = geometry::rotation([0.0, 1.0, 0.0], radians, [0.0; 3]);
    let moved = geometry::rotate(&r, scale(at, -1.0));
    let mut m = r;
    for i in 0..3 {
        m[i][3] = moved[i] + at[i];
    }
    m
}

pub fn moved_by(d: V3) -> M34 {
    let mut m = IDENTITY;
    for i in 0..3 {
        m[i][3] = d[i];
    }
    m
}

impl Locomotion {
    /// World metres a tracking metre makes: an avatar taller than the person makes the world move faster under them.
    pub fn world_scale(&self) -> f32 {
        match (self.osc.eye_height, self.player_height) {
            // VRChat fits the person's real height to the avatar's; eyes are about 93 % of a person's height.
            (Some(eye), Some(height)) if eye > 0.05 && height > 0.5 => (eye / (0.93 * height)).clamp(0.05, 20.0),
            _ => 1.0,
        }
    }

    /// One turn: how far the world moved through tracking space since the last, from the sticks and VRChat's speed.
    /// Returns the world's motion this turn (tracking space), identity when it did not move.
    pub fn step(&mut self, input: &Input) -> M34 {
        let seconds = self.last.map_or(0.0, |t| (input.now - t).clamp(0.0, 0.1)) as f32;
        self.last = Some(input.now);
        if seconds == 0.0 || input.menu_up {
            // A menu up: VRChat does not walk (the sticks scroll its lists); a turn step must not count on release.
            self.turn = TurnStick::default();
            return IDENTITY;
        }
        let head = origin(&input.head);
        let mut motion = IDENTITY;
        // Walking: where the head faces, as VRChat walks.
        let push = (input.walk[0] * input.walk[0] + input.walk[1] * input.walk[1]).sqrt();
        let (forward, right) = (level(scale(axis(&input.head, 2), -1.0)), level(axis(&input.head, 0)));
        let fresh = self.osc.velocity.filter(|(_, at)| input.now - at < SPEED_FRESH_S).map(|(v, _)| v);
        let mut step = [0.0f32; 3];
        let pushed = push > STICK_REST;
        if pushed {
            self.walk_last = Some((input.walk, input.now));
        }
        // Pushed, or let go a moment ago while VRChat still says the player moves.
        let stick = if pushed { Some(input.walk) } else { self.walk_last.filter(|(_, at)| input.now - at < COAST_S && fresh.is_some()).map(|(s, _)| s) };
        if let (Some(stick), Some(f), Some(r)) = (stick, forward, right) {
            let dir = normalized(add(scale(r, stick[0]), scale(f, stick[1])));
            let speed = match fresh {
                Some(v) => (v[0] * v[0] + v[2] * v[2]).sqrt() / self.world_scale(),
                // No OSC: VRChat's default speeds for this push (forward runs, sideways strafes).
                None => {
                    let along = (stick[1].abs() * DEFAULT_RUN).max(stick[0].abs() * DEFAULT_STRAFE);
                    along.min(DEFAULT_RUN) / self.world_scale()
                }
            };
            step = scale(dir, speed * self.gain * seconds);
        }
        // Up and down: VRChat's own vertical speed (stairs, jumps, falls), whatever the stick does.
        if let Some(v) = fresh {
            step[1] = v[1] / self.world_scale() * seconds;
        }
        if geometry::norm(step) > 0.0 {
            // The player moved by `step` through the world: the world moves the other way through tracking space.
            motion = moved_by(scale(step, -1.0));
            self.walked += geometry::norm([step[0], 0.0, step[2]]);
            self.total[0] += geometry::norm([step[0], 0.0, step[2]]);
            self.walked_by = add(self.walked_by, step);
        }
        // Turning: VRChat's own turning speed while the turning stick is pushed (and a moment after), less the head's own
        // turning; in steps, or without OSC, the stick's push at VRChat's usual speed.
        let heading = forward.map(|f| f[0].atan2(-f[2]));
        let head_turn = match (heading, self.heading) {
            // Positive turning right, as AngularY.
            (Some(now), Some(before)) => {
                let mut d = now - before;
                if d > std::f32::consts::PI {
                    d -= std::f32::consts::TAU;
                } else if d < -std::f32::consts::PI {
                    d += std::f32::consts::TAU;
                }
                d
            }
            _ => 0.0,
        };
        self.heading = heading.or(self.heading);
        if input.turn.abs() >= TURN_REST {
            self.turn_last = Some(input.now);
        }
        let angular = self.osc.angular_y.filter(|(_, at)| input.now - at < SPEED_FRESH_S).map(|(a, _)| a);
        let turning = self.turn_last.is_some_and(|at| input.now - at < COAST_S);
        let by_stick = self.turn.step(input.turn, seconds, input.turn_in_steps, TURNS_PER_S);
        let turned = match angular {
            Some(a) if !input.turn_in_steps && turning && a.abs() < 720.0 => a.to_radians() * seconds - head_turn,
            _ => by_stick,
        };
        if turned != 0.0 {
            motion = compose(&turn_about(head, turned), &motion);
            self.turned += turned.abs();
            self.total[1] += turned.abs();
        }
        self.to_tracking = compose(&motion, &self.to_tracking);
        motion
    }

    /// A look found followed lines again `by` away from where the anchors had them (tracking space; the lines were moved
    /// by it in anchor space, [`crate::track::Tracker::update`]): the walking speed learns how far off it was.
    pub fn learn(&mut self, by: V3) {
        // Walking stick pushed since the last correction: the part of `by` along that walk says the speed was off.
        let walked = geometry::norm([self.walked_by[0], 0.0, self.walked_by[2]]);
        if walked > 0.5 {
            let dir = normalized([self.walked_by[0], 0.0, self.walked_by[2]]);
            // `by` is how much further the world must move against the walk.
            let extra = -geometry::dot(by, dir);
            let ratio = ((walked + extra) / walked).clamp(0.5, 2.0);
            self.gain = (self.gain * (0.6 + 0.4 * ratio)).clamp(0.25, 4.0);
        }
        self.walked = 0.0;
        self.turned = 0.0;
        self.walked_by = [0.0; 3];
    }

    /// A pose in tracking space now, in anchor space.
    pub fn anchor(&self, tracking: &M34) -> M34 {
        compose(&geometry::inverse(&self.to_tracking), tracking)
    }

    /// A pose in anchor space, in tracking space now.
    pub fn tracking(&self, anchored: &M34) -> M34 {
        compose(&self.to_tracking, anchored)
    }

    /// The person's real height from VRChat's settings (its `PlayerHeight`, metres), when it says.
    pub fn read_player_height(&mut self, metres: Option<f32>) {
        self.player_height = metres.filter(|m| (0.5..3.0).contains(m));
    }
}

/// The middle of a set of displacements, coordinate by coordinate (what a few re-read lines agree on).
pub fn median(by: &[V3]) -> Option<V3> {
    if by.is_empty() {
        return None;
    }
    Some(std::array::from_fn(|k| {
        let mut v: Vec<f32> = by.iter().map(|b| b[k]).collect();
        v.sort_by(|a, b| a.total_cmp(b));
        let n = v.len();
        if n % 2 == 1 {
            v[n / 2]
        } else {
            0.5 * (v[n / 2 - 1] + v[n / 2])
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{apply, norm, sub};

    fn input(now: f64, head: M34, walk: [f32; 2], turn: f32) -> Input {
        Input { now, head, walk, turn, turn_in_steps: false, menu_up: false }
    }
    fn standing(x: f32, z: f32, yaw: f32) -> M34 {
        let mut m = geometry::rotation([0.0, 1.0, 0.0], yaw, [0.0; 3]);
        m[0][3] = x;
        m[1][3] = 1.6;
        m[2][3] = z;
        m
    }

    #[test]
    fn walking_forward_brings_a_sign_ahead_closer() {
        let mut l = Locomotion::default();
        l.osc.velocity = Some(([0.0, 0.0, 4.0], 0.0));
        let sign = [0.0, 1.6, -5.0];
        let head = standing(0.0, 0.0, 0.0);
        for k in 0..=100 {
            let now = k as f64 * 0.01;
            l.osc.velocity = Some(([0.0, 0.0, 4.0], now));
            l.step(&input(now, head, [0.0, 1.0], 0.0));
        }
        // One second at 4 m/s: the sign is 1 m ahead now.
        let now = apply(&l.to_tracking, sign);
        assert!((now[2] - -1.0).abs() < 0.05, "{now:?}");
        assert!(now[0].abs() < 1e-3);
    }

    #[test]
    fn walking_goes_where_the_head_faces() {
        let mut l = Locomotion::default();
        // Facing 90 degrees to the left (-X), stick forward: the world comes along +X.
        let head = standing(0.0, 0.0, std::f32::consts::FRAC_PI_2);
        for k in 0..=50 {
            let now = k as f64 * 0.01;
            l.osc.velocity = Some(([0.0, 0.0, 2.0], now));
            l.step(&input(now, head, [0.0, 1.0], 0.0));
        }
        let moved = apply(&l.to_tracking, [0.0; 3]);
        assert!((moved[0] - 1.0).abs() < 0.05 && moved[2].abs() < 0.01, "{moved:?}");
    }

    #[test]
    fn the_head_walking_in_the_room_moves_nothing() {
        // Walking for real, stick at rest: the world stays where it is in tracking space, whatever speed VRChat says.
        let mut l = Locomotion::default();
        for k in 0..=100 {
            let now = k as f64 * 0.01;
            l.osc.velocity = Some(([0.0, 0.0, 1.2], now));
            l.step(&input(now, standing(0.0, -0.012 * k as f32, 0.0), [0.0, 0.05], 0.0));
        }
        assert_eq!(l.to_tracking, IDENTITY);
    }

    #[test]
    fn a_quiet_osc_link_falls_back_to_vrchats_default_speed() {
        let mut l = Locomotion::default();
        l.osc.velocity = Some(([0.0, 0.0, 9.0], 0.0));
        for k in 0..=100 {
            l.step(&input(k as f64 * 0.01, standing(0.0, 0.0, 0.0), [0.0, 1.0], 0.0));
        }
        let moved = apply(&l.to_tracking, [0.0; 3]);
        // 9 m/s for the first half second, then the default run.
        assert!((moved[2] - (0.5 * 9.0 + 0.5 * DEFAULT_RUN)).abs() < 0.15, "{moved:?}");
    }

    #[test]
    fn turning_turns_the_world_about_the_head() {
        let mut l = Locomotion::default();
        let head = standing(0.3, 0.2, 0.0);
        for k in 0..=100 {
            l.step(&input(k as f64 * 0.01, head, [0.0; 2], 1.0));
        }
        // The head stays where it is; a sign ahead went to the left (turning right).
        let h = apply(&l.to_tracking, origin(&head));
        assert!(norm(sub(h, origin(&head))) < 1e-4);
        let ahead = apply(&l.to_tracking, [0.3, 1.6, -2.0 + 0.2]);
        assert!(ahead[0] < 0.3 - 1.0, "{ahead:?}");
        assert!((norm(sub(ahead, origin(&head))) - 2.0).abs() < 1e-3);
        assert!((l.turned - TURNS_PER_S).abs() < 0.05);
    }

    #[test]
    fn steps_turn_once_a_push() {
        let mut l = Locomotion::default();
        let head = standing(0.0, 0.0, 0.0);
        let mut i = input(0.0, head, [0.0; 2], 0.0);
        i.turn_in_steps = true;
        for (k, stick) in [0.0, 0.9, 0.9, 0.9, 0.0, -0.9, 0.0].into_iter().enumerate() {
            i.now = k as f64 * 0.02;
            i.turn = stick;
            l.step(&i);
        }
        // One step right, one left: back where it was.
        assert!(norm(sub(apply(&l.to_tracking, [0.0, 0.0, -2.0]), [0.0, 0.0, -2.0])) < 1e-4);
        assert!((l.turned - 2.0 * TURN_STEP).abs() < 1e-4);
    }

    #[test]
    fn nothing_moves_while_a_menu_is_up() {
        let mut l = Locomotion::default();
        let mut i = input(0.0, standing(0.0, 0.0, 0.0), [0.0, 1.0], 1.0);
        i.menu_up = true;
        for k in 0..50 {
            i.now = k as f64 * 0.01;
            l.step(&i);
        }
        assert_eq!(l.to_tracking, IDENTITY);
    }

    #[test]
    fn anchors_follow_the_world_and_back() {
        let mut l = Locomotion::default();
        let sign = standing(1.0, -3.0, 0.4);
        let anchored = l.anchor(&sign);
        for k in 0..=60 {
            let now = k as f64 * 0.01;
            l.osc.velocity = Some(([1.0, 0.0, 3.0], now));
            l.step(&input(now, standing(0.0, 0.0, 0.0), [0.3, 1.0], 0.6));
        }
        let now = l.tracking(&anchored);
        assert!(norm(sub(origin(&now), origin(&sign))) > 1.0, "it moved with the world");
        let back = l.anchor(&now);
        assert!(norm(sub(origin(&back), origin(&anchored))) < 1e-4);
    }

    #[test]
    fn a_re_reading_teaches_the_speed() {
        let mut l = Locomotion::default();
        // VRChat said 4 m/s but the world went 5 m/s (a world's own speed, or the scale guess): 2 s forward.
        for k in 0..=200 {
            let now = k as f64 * 0.01;
            l.osc.velocity = Some(([0.0, 0.0, 4.0], now));
            l.step(&input(now, standing(0.0, 0.0, 0.0), [0.0, 1.0], 0.0));
        }
        // Seen 2 m nearer than estimated.
        l.learn([0.0, 0.0, 2.0]);
        assert!(l.gain > 1.05 && l.gain < 1.25, "{}", l.gain);
        assert_eq!(l.walked, 0.0);
        // Walking again the same: the world now goes 10 % further each second.
        let before = apply(&l.to_tracking, [0.0; 3]);
        for k in 201..=301 {
            let now = k as f64 * 0.01;
            l.osc.velocity = Some(([0.0, 0.0, 4.0], now));
            l.step(&input(now, standing(0.0, 0.0, 0.0), [0.0, 1.0], 0.0));
        }
        let went = apply(&l.to_tracking, [0.0; 3])[2] - before[2];
        assert!((went - 4.0 * l.gain).abs() < 0.1, "{went}");
    }

    #[test]
    fn vrchats_speed_counts_a_moment_after_the_stick_is_let_go() {
        // VRChat's speed comes 0.2 s late: pushed 0.5 s, the speed says 4 m/s from 0.2 s to 0.7 s.
        let mut l = Locomotion::default();
        for k in 0..=150 {
            let now = k as f64 * 0.01;
            let v = if (0.2..0.7).contains(&now) { 4.0 } else { 0.0 };
            l.osc.velocity = Some(([0.0, 0.0, v], now));
            l.step(&input(now, standing(0.0, 0.0, 0.0), [0.0, if now < 0.5 { 1.0 } else { 0.0 }], 0.0));
        }
        assert!((l.total[0] - 2.0).abs() < 0.1, "{}", l.total[0]);
    }

    #[test]
    fn vrchats_turning_speed_is_taken_while_the_stick_turns() {
        let mut l = Locomotion::default();
        // 120 degrees a second for 0.3 s, as VRChat says, the stick pushed a little.
        for k in 0..=100 {
            let now = k as f64 * 0.01;
            l.osc.angular_y = Some((if now < 0.3 { 120.0 } else { 0.0 }, now));
            l.step(&input(now, standing(0.0, 0.0, 0.0), [0.0; 2], if now < 0.3 { 0.5 } else { 0.0 }));
        }
        assert!((l.total[1].to_degrees() - 36.0).abs() < 2.0, "{}", l.total[1].to_degrees());
        // The head turning by itself, stick at rest: nothing, whatever AngularY says.
        let before = l.total[1];
        for k in 101..=200 {
            let now = k as f64 * 0.01;
            l.osc.angular_y = Some((90.0, now));
            l.step(&input(now, standing(0.0, 0.0, -(k - 100) as f32 * 0.01), [0.0; 2], 0.0));
        }
        assert_eq!(l.total[1], before);
    }

    #[test]
    fn a_taller_avatar_moves_the_world_faster_per_metre() {
        let mut l = Locomotion::default();
        l.read_player_height(Some(1.73));
        l.osc.eye_height = Some(1.73 * 0.93 * 2.0);
        assert!((l.world_scale() - 2.0).abs() < 1e-4);
    }

    #[test]
    fn the_median_ignores_one_odd_line() {
        let m = median(&[[0.1, 0.0, 0.0], [0.12, 0.0, 0.01], [3.0, 1.0, 1.0]]).unwrap();
        assert!((m[0] - 0.12).abs() < 1e-6 && m[1] == 0.0 && (m[2] - 0.01).abs() < 1e-6);
    }
}
