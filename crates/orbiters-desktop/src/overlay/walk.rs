//! Walk boost: while VRChat walks the avatar with the stick, the play space is dragged the same way (the working copy
//! of SteamVR's chaperone setup, previewed live like OVR Advanced Settings' space drag, never saved), so the avatar
//! covers more ground than VRChat's own walking. The chaperone's walls move the other way, so they stay on the room's
//! walls.
//!
//! The boost waits while the stick is not walking: Orb's menu or the calibration stands in the play space, a setting
//! of the boost was just changed (the Walk boost radial of VRChat's Action Menu turns with the same stick), VRChat says
//! the owner is away, and, when VRChat says how it moves the avatar, while it sits in a station or does not walk it. A
//! recenter, a seated reset, a room setup or another application's saved change becomes the new base of the carry
//! instead of being written over. The play space goes back where it was when VRChat loads another world (the avatar
//! respawns anyway), when VRChat closes and when the overlay ends, however it ends; a process killed outside leaves
//! nothing behind either, as SteamVR goes back to the saved chaperone once the process previewing one is gone.

use super::actions::Walk;
use orbiters_core::settings::{Follow, Hand, WalkBoostSettings};
use orbiters_openvr::chaperone::{ChaperoneSetup, Quad};
use orbiters_openvr::{self as vr, Matrix34, Pose, Universe, MAX_DEVICES};
use std::time::{Duration, Instant};

/// VRChat's running speed (m/s), reached with the stick pushed all the way.
pub const RUN_SPEED: f32 = 4.0;
/// Stick travel ignored around the centre.
const DEADZONE: f32 = 0.12;
/// A frame longer than this moves as far as this (no jump after a stall).
const MAX_STEP_S: f32 = 0.05;
/// After a setting of the boost changes, the stick is taken for a menu's this long: the Walk boost radial in
/// VRChat's Action Menu turns with it, and a toggle there is chosen with it.
const MENU_QUIET: Duration = Duration::from_millis(1500);
/// Room setup lets go of the play space until it is saved, this long at most (it can be left without saving).
const ROOM_SETUP: Duration = Duration::from_secs(180);
/// VRChat moves an avatar it walks at least this fast (m/s), before the boost adds anything.
const VRCHAT_WALKS: f32 = 0.4;
/// Slower than this on its own (the boost's drag taken out), VRChat stands the avatar still.
const VRCHAT_STILL: f32 = 0.15;
/// VRChat's speed reaches Orbiters this late (OSC, then the service): the boost's own drag shows in it after this.
const MOTION_LAG: Duration = Duration::from_millis(500);
/// VRChat's own walking this long below [`VRCHAT_STILL`] stops the carry (a change of push shows late, briefly).
const STILL_FOR: Duration = Duration::from_millis(300);
/// Setups this close (metres, and rotation terms) are the same one: SteamVR may hand its numbers back a little off.
const SAME_SETUP: f32 = 1e-3;

/// How far the stick is pushed (0 to 1, the dead zone removed; 0 for a reading that is not a number).
pub fn push(stick: [f32; 2]) -> f32 {
    if !(stick[0].is_finite() && stick[1].is_finite()) {
        return 0.0;
    }
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
        // Looking straight up or down, or a pose that is not a number: no way forward.
        if len >= 1e-4 {
            Some([v[0] / len, 0.0, v[2] / len])
        } else {
            None
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

/// A play space: where the standing and seated universes sit in the raw tracking space, and the chaperone's walls
/// (in the standing universe).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Setup {
    pub standing: Matrix34,
    pub seated: Option<Matrix34>,
    pub bounds: Vec<Quad>,
}

impl Setup {
    /// The same play space, to a millimetre.
    pub fn close_to(&self, other: &Setup) -> bool {
        let near = |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= SAME_SETUP);
        let matrix = |a: &Matrix34, b: &Matrix34| near(a.m.as_flattened(), b.m.as_flattened());
        let walls = |q: &[Quad]| q.iter().flat_map(|q| q.corners.as_flattened().iter().copied()).collect::<Vec<f32>>();
        matrix(&self.standing, &other.standing)
            && match (&self.seated, &other.seated) {
                (Some(a), Some(b)) => matrix(a, b),
                (a, b) => a.is_none() && b.is_none(),
            }
            && near(&walls(&self.bounds), &walls(&other.bounds))
    }
}

/// The play space as it was before walking, and how far the person was carried since (in the standing universe).
#[derive(Clone, Debug, PartialEq)]
pub struct Carried {
    pub base: Setup,
    pub offset: [f32; 3],
}

impl Carried {
    /// The play space that carries the person `offset` metres: the universes' origins move by −offset in the room
    /// (so every tracked position reads +offset), and the walls' corners by +offset in the standing universe (so they
    /// stay where they are in the room).
    pub fn shifted(&self) -> Setup {
        let r = &self.base.standing.m;
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
        let bounds = self.base.bounds.iter().map(|q| Quad { corners: q.corners.map(|c| [c[0] + o[0], c[1] + o[1], c[2] + o[2]]) }).collect();
        Setup { standing: moved(&self.base.standing), seated: self.base.seated.as_ref().map(moved), bounds }
    }
}

/// SteamVR's chaperone setup as walk boost uses it (a stand-in in tests). While a preview shows, it is what SteamVR
/// calls the live setup: every application sees it, and the working copy reverts to it rather than to the saved one.
pub trait PlaySpace {
    /// The working copy back to the live setup: the preview showing, or else the saved setup.
    fn revert(&mut self);
    /// The working copy.
    fn read(&mut self) -> Option<Setup>;
    fn write(&mut self, setup: &Setup);
    /// The working copy shown to every application: a preview, never saved.
    fn show(&mut self);
    /// The preview hidden: the saved setup is the live one again.
    fn hide(&mut self);
}

impl PlaySpace for ChaperoneSetup<'_> {
    fn revert(&mut self) {
        self.revert_working_copy();
    }

    fn read(&mut self) -> Option<Setup> {
        let standing = self.working_standing_zero()?;
        Some(Setup { standing, seated: self.working_seated_zero(), bounds: self.working_collision_bounds().unwrap_or_default() })
    }

    fn write(&mut self, setup: &Setup) {
        self.set_working_standing_zero(&setup.standing);
        if let Some(seated) = &setup.seated {
            self.set_working_seated_zero(seated);
        }
        if !setup.bounds.is_empty() {
            self.set_working_collision_bounds(&setup.bounds);
        }
    }

    fn show(&mut self) {
        self.show_working_set_preview();
    }

    fn hide(&mut self) {
        self.hide_working_set_preview();
    }
}

/// What VRChat says of the avatar over OSC (its `InStation`, `Seated` and `VelocityMagnitude` parameters).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    /// In a station (a chair, a vehicle), seated or not: VRChat does not walk the avatar.
    pub in_station: bool,
    /// How fast the avatar moves (m/s): VRChat's walking, and the boost's own drag (room-scale motion counts).
    pub speed: f32,
}

/// Whether VRChat walks the avatar, as far as walk boost can tell, push by push.
#[derive(Clone, Copy, Debug, Default)]
pub struct Gate {
    /// Carrying since then, in this push.
    since: Option<Instant>,
    /// VRChat's own walking (its speed less the boost's) has stayed below walking since then.
    still_since: Option<Instant>,
}

impl Gate {
    /// Whether the boost carries the person at `now`: only while the stick is pushed (`pushed`), and, when VRChat
    /// says how the avatar moves, while it walks it. A push starts carrying once VRChat moves the avatar on its own
    /// (nothing of the boost in its speed yet). Carrying, VRChat's speed counts the boost's drag, `boost` (m/s, the
    /// last frame's): it is taken out once VRChat's reports caught up with it, and carrying stops when what is left
    /// stays below walking a moment (an Action Menu opened mid-walk took the stick, the world froze the avatar) or
    /// VRChat seats the avatar.
    pub fn carries(&mut self, now: Instant, pushed: bool, motion: Option<&Motion>, boost: f32) -> bool {
        let since = |t: Instant| now.saturating_duration_since(t);
        let carrying = match (pushed, motion, self.since) {
            (false, _, _) => false,
            (true, None, _) => true,
            (true, Some(m), _) if m.in_station => false,
            (true, Some(m), None) => m.speed >= VRCHAT_WALKS,
            (true, Some(m), Some(start)) => {
                if since(start) < MOTION_LAG || m.speed - boost >= VRCHAT_STILL {
                    self.still_since = None;
                    true
                } else {
                    since(*self.still_since.get_or_insert(now)) < STILL_FOR
                }
            }
        };
        if carrying {
            self.since.get_or_insert(now);
        } else {
            *self = Gate::default();
        }
        carrying
    }
}

/// What the overlay loop knows of VRChat for walking.
pub struct Moment<'a> {
    pub settings: &'a WalkBoostSettings,
    /// VRChat runs, is in a world and has the scene (not a loading screen, not the dashboard).
    pub walking_allowed: bool,
    /// The world and instance: another one puts the play space back.
    pub place: Option<&'a str>,
    /// Something stands in the play space where it was put (Orb's menu, the calibration's mirror): moving the play
    /// space would carry the person away from it.
    pub held: bool,
    /// VRChat says the owner is away.
    pub away: bool,
    /// How VRChat says the avatar moves, while it talks to Orbiters.
    pub motion: Option<Motion>,
    /// SteamVR's controllers were read this frame: a stick read after a failed read is the last one's.
    pub input_read: bool,
}

pub struct WalkBoost {
    actions: Option<Walk>,
    carried: Option<Carried>,
    place: Option<String>,
    last: Option<Instant>,
    /// The manifest could not be loaded: said once.
    failed: Option<String>,
    gate: Gate,
    /// How fast the boost dragged the play space on the last frame (m/s; 0 when it did not).
    boost: f32,
    /// The play space moved on the last frame.
    moving: bool,
    /// The boost's settings last seen (on, strength), and when they last changed.
    seen: Option<(bool, f32)>,
    changed: Option<Instant>,
    /// Room setup started then, and was not saved yet.
    room_setup: Option<Instant>,
}

impl WalkBoost {
    /// Walk boost with the overlay's actions (none when SteamVR's input could not load them: `failed` says why).
    pub fn new(actions: Option<Walk>, failed: Option<String>) -> Self {
        Self {
            actions,
            carried: None,
            place: None,
            last: None,
            failed,
            gate: Gate::default(),
            boost: 0.0,
            moving: false,
            seen: None,
            changed: None,
            room_setup: None,
        }
    }

    /// Why walk boost cannot work, if it cannot.
    pub fn unavailable(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    /// The play space moved on the last frame: the overlay loop then runs once per frame the compositor shows.
    pub fn moving(&self) -> bool {
        self.moving
    }

    /// The boost is on and nothing holds it back.
    fn active(&self, m: &Moment, now: Instant) -> bool {
        let since = |t: Instant| now.saturating_duration_since(t);
        self.actions.is_some()
            && m.settings.enabled
            && m.settings.strength > 0.0
            && m.walking_allowed
            && !m.held
            && !m.away
            && self.changed.is_none_or(|t| since(t) >= MENU_QUIET)
            && self.room_setup.is_none_or(|t| since(t) >= ROOM_SETUP)
    }

    /// One frame: reads the stick and carries the person further.
    pub fn update(&mut self, steamvr: &vr::Client, m: &Moment) {
        let Ok(mut setup) = steamvr.chaperone_setup() else { return };
        let actions = self.actions;
        // The overlay loop read every action set this frame (actions::Actions::update).
        let stick = || {
            let walk = actions?;
            let action = if m.settings.hand == Hand::Left { walk.left } else { walk.right };
            let stick = steamvr.input().ok()?.analog(action).ok()?;
            stick.active.then_some([stick.x, stick.y])
        };
        let frame = || {
            let mut poses = [Pose::default(); MAX_DEVICES];
            steamvr.poses(Universe::Standing, &mut poses);
            let device = match m.settings.follow {
                Follow::Head => 0,
                Follow::Hand => {
                    let role = if m.settings.hand == Hand::Left { vr::ControllerRole::LeftHand } else { vr::ControllerRole::RightHand };
                    steamvr.controller_index(role).unwrap_or(0)
                }
            };
            let pose = poses[device as usize];
            pose.pose_is_valid.then_some(pose.device_to_absolute)
        };
        self.step(&mut setup, m, stick, frame, Instant::now());
    }

    /// One frame at `now`, reading what SteamVR says only when needed: the stick (`None`: not active) and where
    /// forward points (the head or the walking hand, `None` while not tracked).
    pub fn step(
        &mut self,
        play: &mut dyn PlaySpace,
        m: &Moment,
        stick: impl FnOnce() -> Option<[f32; 2]>,
        frame: impl FnOnce() -> Option<Matrix34>,
        now: Instant,
    ) {
        let dt = self.last.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32()).min(MAX_STEP_S);
        self.last = Some(now);
        self.moving = false;
        if self.place.as_deref() != m.place {
            // Another world or instance (or VRChat closed): the play space goes back while the avatar respawns.
            self.place = m.place.map(str::to_string);
            self.put_back(play);
        }
        let seen = (m.settings.enabled, m.settings.strength);
        if self.seen.is_some_and(|s| s != seen) {
            self.changed = Some(now);
        }
        self.seen = Some(seen);
        let stick = if self.active(m, now) && m.input_read { stick() } else { None };
        let pushed = stick.is_some_and(|s| push(s) > 0.0);
        let carries = self.gate.carries(now, pushed, m.motion.as_ref(), std::mem::take(&mut self.boost));
        let Some(stick) = stick.filter(|_| carries && dt > 0.0) else { return };
        let Some(frame) = frame() else { return };
        let dir = direction(stick, &frame);
        if dir == [0.0; 3] {
            return;
        }
        let speed = RUN_SPEED * m.settings.strength.clamp(0.0, 1.0) * WalkBoostSettings::MAX_EXTRA;
        let step = dir.map(|d| d * speed * dt);
        if !step.iter().all(|s| s.is_finite()) {
            return;
        }
        if self.carried.is_none() {
            // No preview of the boost's shows now: the working copy reverts to the saved setup.
            play.revert();
            let Some(base) = play.read() else { return };
            self.carried = Some(Carried { base, offset: [0.0; 3] });
        }
        let carried = self.carried.as_mut().expect("set above");
        for (o, s) in carried.offset.iter_mut().zip(step) {
            *o += s;
        }
        play.write(&carried.shifted());
        play.show();
        self.boost = speed * dir[0].hypot(dir[2]);
        self.moving = true;
    }

    /// Puts the play space back where it was.
    pub fn reset(&mut self, steamvr: &vr::Client) {
        match steamvr.chaperone_setup() {
            Ok(mut setup) => self.put_back(&mut setup),
            // Nothing can be put back without the interface (SteamVR drops the preview with this process).
            Err(_) => self.carried = None,
        }
    }

    /// The same with `play`: the preview hidden first, so that the working copy reverts to the saved setup.
    pub fn put_back(&mut self, play: &mut dyn PlaySpace) {
        self.moving = false;
        self.boost = 0.0;
        if self.carried.take().is_some() {
            play.hide();
            play.revert();
        }
    }

    /// SteamVR's play space may have changed under the boost (a recenter or a seated reset, a room setup or another
    /// application's change saved): the carry goes on from a new setup instead of writing the old one back.
    pub fn play_space_changed(&mut self, steamvr: &vr::Client) {
        if let Ok(mut setup) = steamvr.chaperone_setup() {
            self.rebase(&mut setup);
        }
    }

    /// The same with `play`. What every application sees is read first: the boost's own preview there means nothing
    /// changed it (the boost's showing raises some of these events), and nothing is touched, so no event loops. Else
    /// the setup saved beneath, read with the preview hidden for a moment (while one shows, the working copy reverts
    /// to it, not to the saved one), becomes the base if it is neither the old base nor what the boost showed, and
    /// the boost shows its carry again at once.
    pub fn rebase(&mut self, play: &mut dyn PlaySpace) {
        let Some(carried) = self.carried.as_mut() else { return };
        play.revert();
        let Some(live) = play.read() else { return };
        let shown = carried.shifted();
        if live.close_to(&shown) || live.close_to(&carried.base) {
            return;
        }
        play.hide();
        play.revert();
        if let Some(saved) = play.read() {
            if !saved.close_to(&carried.base) && !saved.close_to(&shown) {
                carried.base = saved;
            }
        }
        play.write(&carried.shifted());
        play.show();
    }

    /// Room setup starts: the play space goes back and stays put until the setup is saved.
    pub fn room_setup_started(&mut self, steamvr: &vr::Client) {
        self.reset(steamvr);
        self.room_setup = Some(Instant::now());
    }

    /// Room setup was saved: the boost may move the new play space.
    pub fn room_setup_ended(&mut self) {
        self.room_setup = None;
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
        let carried = Carried { base: Setup { standing, seated: None, bounds: vec![wall] }, offset: [0.5, 0.0, -2.0] };
        let Setup { standing: moved, bounds, .. } = carried.shifted();
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

    #[test]
    fn readings_that_are_not_numbers_do_not_move() {
        assert_eq!(push([f32::NAN, 0.5]), 0.0);
        assert_eq!(direction([f32::NAN, 1.0], &yaw(0.0)), [0.0; 3]);
        let mut broken = yaw(0.0);
        broken.m[0][2] = f32::NAN;
        assert_eq!(direction([0.0, 1.0], &broken), [0.0; 3]);
    }

    #[test]
    fn carries_only_while_vrchat_walks() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let speed = |speed: f32| Motion { in_station: false, speed };
        // Nothing from VRChat: the stick alone.
        let mut g = Gate::default();
        assert!(g.carries(at(0), true, None, 0.0) && !g.carries(at(10), false, None, 0.0));
        // A push starts carrying once VRChat moves the avatar itself.
        let mut g = Gate::default();
        assert!(!g.carries(at(0), true, Some(&speed(0.0)), 0.0));
        assert!(g.carries(at(10), true, Some(&speed(3.0)), 0.0));
        // Carrying, VRChat's speed counts the boost's own drag: 4.05 m/s with 4 of boost is VRChat standing still.
        assert!(g.carries(at(100), true, Some(&speed(4.05)), 4.0), "its reports lag the boost at first");
        assert!(g.carries(at(700), true, Some(&speed(4.05)), 4.0), "a moment below walking is not a stop");
        assert!(!g.carries(at(1100), true, Some(&speed(4.05)), 4.0), "the Action Menu took the stick: the boost stops");
        // Walking: VRChat's own 4 m/s on top of the boost's 4.
        let mut g = Gate::default();
        assert!(g.carries(at(0), true, Some(&speed(3.0)), 0.0));
        assert!(g.carries(at(2000), true, Some(&speed(8.0)), 4.0));
        // In a station, never; a speed that is not a number starts nothing.
        assert!(!g.carries(at(2010), true, Some(&Motion { in_station: true, speed: 3.0 }), 4.0));
        assert!(!Gate::default().carries(at(0), true, Some(&speed(f32::NAN)), 0.0));
    }

    /// SteamVR's chaperone as walk boost meets it: a saved setup, the preview that shows (the live setup while it
    /// does: the working copy reverts to it), and the working copy.
    #[derive(Default)]
    struct Room {
        saved: Setup,
        preview: Option<Setup>,
        working: Setup,
        shows: usize,
        hides: usize,
    }

    impl Room {
        fn new(standing: Matrix34) -> Self {
            Self { saved: Setup { standing, ..Default::default() }, ..Default::default() }
        }

        /// What every application sees.
        fn live(&self) -> &Setup {
            self.preview.as_ref().unwrap_or(&self.saved)
        }
    }

    impl PlaySpace for Room {
        fn revert(&mut self) {
            self.working = self.live().clone();
        }

        fn read(&mut self) -> Option<Setup> {
            Some(self.working.clone())
        }

        fn write(&mut self, setup: &Setup) {
            self.working = setup.clone();
        }

        fn show(&mut self) {
            self.preview = Some(self.working.clone());
            self.shows += 1;
        }

        fn hide(&mut self) {
            self.preview = None;
            self.hides += 1;
        }
    }

    fn settings() -> WalkBoostSettings {
        WalkBoostSettings { enabled: true, strength: 0.5, ..Default::default() }
    }

    fn moment<'a>(settings: &'a WalkBoostSettings, place: Option<&'a str>) -> Moment<'a> {
        Moment { settings, walking_allowed: true, place, held: false, away: false, motion: None, input_read: true }
    }

    fn boost() -> WalkBoost {
        WalkBoost::new(Some(Walk { set: 1, left: 2, right: 3 }), None)
    }

    const FORWARD: Option<[f32; 2]> = Some([0.0, 1.0]);

    /// The play space's standing origin, moved by the carry, as `room` shows it.
    fn carried_by(room: &Room) -> [f32; 3] {
        let (live, saved) = (&room.live().standing.m, &room.saved.standing.m);
        [0, 1, 2].map(|r| saved[r][3] - live[r][3])
    }

    #[test]
    fn carries_and_puts_back() {
        let (s, mut room, mut walk) = (settings(), Room::new(Matrix34::IDENTITY), boost());
        let t0 = Instant::now();
        let frame = |ms: u64| t0 + Duration::from_millis(ms);
        let m = moment(&s, Some("wrld_1"));
        walk.step(&mut room, &m, || FORWARD, || Some(yaw(0.0)), frame(0));
        assert!(!walk.moving(), "the first frame only starts the clock");
        walk.step(&mut room, &m, || FORWARD, || Some(yaw(0.0)), frame(10));
        assert!(walk.moving() && room.preview.is_some());
        // 10 ms at the full push: 4 m/s × 0.5 × 2 more, ahead (−Z) of the head.
        assert!(close(carried_by(&room), [0.0, 0.0, -0.04]), "{:?}", carried_by(&room));
        // Controllers that could not be read this frame: no step on a stale stick.
        walk.step(&mut room, &Moment { input_read: false, ..moment(&s, Some("wrld_1")) }, || FORWARD, || Some(yaw(0.0)), frame(20));
        assert!(!walk.moving());
        // Let go: the play space stays where it was carried.
        walk.step(&mut room, &m, || Some([0.0, 0.0]), || Some(yaw(0.0)), frame(30));
        assert!(!walk.moving() && room.preview.is_some() && room.hides == 0);
        // Another instance: back where it was, the working copy the saved setup again.
        walk.step(&mut room, &moment(&s, Some("wrld_2")), || None, || None, frame(40));
        assert!(room.preview.is_none() && room.hides == 1 && room.working == room.saved);
    }

    #[test]
    fn its_own_preview_changes_nothing() {
        let (s, mut room, mut walk) = (settings(), Room::new(Matrix34::IDENTITY), boost());
        let t0 = Instant::now();
        let m = moment(&s, None);
        walk.step(&mut room, &m, || FORWARD, || Some(yaw(0.0)), t0);
        walk.step(&mut room, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_millis(20));
        let (carried, shows) = (carried_by(&room), room.shows);
        // An event its own showing raised (SteamVR's working copy reverts to the preview, not to the saved setup):
        // the play space stays as it is, nothing hidden or shown again, so no event follows.
        for _ in 0..3 {
            walk.rebase(&mut room);
        }
        assert!(close(carried_by(&room), carried), "{:?} then {:?}", carried, carried_by(&room));
        assert_eq!((room.shows, room.hides), (shows, 0));
    }

    #[test]
    fn a_new_saved_setup_becomes_the_base() {
        let (s, mut room, mut walk) = (settings(), Room::new(Matrix34::IDENTITY), boost());
        let t0 = Instant::now();
        let m = moment(&s, None);
        walk.step(&mut room, &m, || FORWARD, || Some(yaw(0.0)), t0);
        walk.step(&mut room, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_millis(20));
        assert!(close(carried_by(&room), [0.0, 0.0, -0.08]));
        // A recenter saved a new standing zero and SteamVR shows it in place of the preview: the carry goes on from it,
        // as far as before (20 ms at 4 m/s).
        room.saved.standing = yaw(90.0);
        room.preview = None;
        walk.rebase(&mut room);
        let expected = Carried { base: room.saved.clone(), offset: [0.0, 0.0, -0.08] }.shifted();
        assert!(room.live().close_to(&expected), "{:?}", room.live());
        // Another application's change saved under the preview while it shows: read with the preview hidden.
        room.saved.standing.m[1][3] = 1.65;
        room.preview.as_mut().unwrap().standing.m[0][3] += 0.3;
        walk.rebase(&mut room);
        assert!((room.live().standing.m[1][3] - 1.65).abs() < 1e-5, "the new saved setup is the base");
        assert!(room.preview.is_some(), "and the carry shows again");
    }

    #[test]
    fn waits_while_the_stick_drives_a_menu() {
        let (mut s, mut room, mut walk) = (settings(), Room::default(), boost());
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        walk.step(&mut room, &moment(&s, None), || FORWARD, || Some(yaw(0.0)), at(0));
        // The Walk boost radial turned: the stick is on the Action Menu for a moment.
        s.strength = 0.6;
        walk.step(&mut room, &moment(&s, None), || FORWARD, || Some(yaw(0.0)), at(10));
        walk.step(&mut room, &moment(&s, None), || FORWARD, || Some(yaw(0.0)), at(1000));
        assert!(!walk.moving() && room.preview.is_none());
        walk.step(&mut room, &moment(&s, None), || FORWARD, || Some(yaw(0.0)), at(1600));
        assert!(walk.moving());
        // Orb's menu in the play space, or the owner away: it waits too.
        let held = Moment { held: true, ..moment(&s, None) };
        walk.step(&mut room, &held, || FORWARD, || Some(yaw(0.0)), at(1610));
        assert!(!walk.moving());
        let away = Moment { away: true, ..moment(&s, None) };
        walk.step(&mut room, &away, || FORWARD, || Some(yaw(0.0)), at(1620));
        assert!(!walk.moving());
        // VRChat says the avatar stands still: a push does not start the carry.
        let still = Moment { motion: Some(Motion { in_station: false, speed: 0.0 }), ..moment(&s, None) };
        walk.step(&mut room, &still, || FORWARD, || Some(yaw(0.0)), at(1630));
        assert!(!walk.moving());
    }
}
