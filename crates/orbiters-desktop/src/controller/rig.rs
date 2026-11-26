//! N4N0's camera rig while recording (contracts' `Docs/rig-frames.md`, N4N0's capture kit).
//!
//! While the app drives the avatar during a recording, the rig follows VRChat's camera: `N4N0/Capture/Follow` on, and
//! `N4N0/Capture/RigX`, `RigY`, `RigZ` and `RigYaw` from where VRChat says its camera is (`/usercamera/Pose`), as offsets
//! from the avatar's hips: the rig hangs from them (its Parent Constraint's one source), and the capture worker reads
//! where they are from each colour frame's skeleton band (`WorkerEvent::Hips`). A 3D picture and a scan place the rig
//! themselves, in the same bundle as each camera pose they send. (N4N0 up to 1.0.15 meant world coordinates, from a
//! world-origin source VRChat leaves out of uploads: its rig, too, hangs from the hips in VRChat, so these offsets put it
//! at the camera as well.)
//!
//! Every few seconds (Settings, `RigViews`) the rig turns on for a moment (`N4N0/Capture/Rig`, [`PULSE`]) where the
//! camera stands, unless it already looked all around from there: about every other colour frame meanwhile is a rig
//! frame, six more views around the camera (left, front, right, back, up, down) that the capture worker records as
//! views of their own and the reconstruction trains on, as for a 3D picture, which holds the rig on for its whole flight.
//! Why these numbers: a pulse of 0.3 s is a dozen rig frames at VRChat's 90 frames a second (a quarter of the frames are
//! depth frames, half of the rest rig frames), enough for every face to be read even when a frame or two of them come
//! late; a new one only [`PULSE_AFTER_M`] from every place the rig already looked all around from, where the same six
//! views add nothing. Every 3 s, a walk at 1 m/s loses about 5 % of its colour frames to rig frames (never depth frames,
//! and the keyframes it keeps are a few a second), and VRChat renders the world six more times for 0.3 s in 3 s rather
//! than all along. The app's own flights (3D pictures and scans) leave the rig to themselves.

use orbiters_core::locks::*;
use super::burst::{floats, rig_parameters, rig_switch, POSE};
use super::Shared;
use crate::state::CaptureState;
use std::time::{Duration, Instant};

/// How often the follower looks at where VRChat's camera is.
const TICK: Duration = Duration::from_millis(33);
/// The rig is placed again once its place from the hips moved this far or turned this much, and at least this often.
const MOVE_M: f32 = 0.02;
const TURN_DEG: f32 = 1.0;
const RESEND: Duration = Duration::from_secs(1);
/// How long the rig stays on for each pulse.
pub const PULSE: Duration = Duration::from_millis(300);
/// How far the camera must be from every place of the recording's earlier pulses for a new one.
pub const PULSE_AFTER_M: f32 = 0.5;
/// The first pulse, once the recording settled (Spout, the kit's first frames, the rig placed).
pub const FIRST_PULSE_AFTER: Duration = Duration::from_secs(2);

/// Where the avatar's hips are (world position, rotation x y z w): the skeleton band's.
pub type Hips = ([f32; 3], [f32; 4]);

/// What VRChat was told about the rig, and who holds it.
#[derive(Default)]
pub struct RigRuntime {
    /// The follower of the current recording (a new recording's replaces it; stopping ends it).
    generation: u64,
    /// The rig follows the camera (`N4N0/Capture/Follow` sent on).
    following: bool,
    /// The rig is on (`N4N0/Capture/Rig` sent on).
    on: bool,
    /// A 3D picture holds the rig on for its flight.
    held: bool,
    /// The hips the recording's frames last showed: where the rig hangs from.
    hips: Option<Hips>,
}

/// When to switch the rig on for a moment.
#[derive(Default)]
pub struct Pulses {
    started: Option<Instant>,
    /// When the last pulse began.
    last: Option<Instant>,
    /// Where the camera was for each pulse of the recording.
    places: Vec<[f32; 3]>,
    until: Option<Instant>,
}

impl Pulses {
    /// Whether the rig should be on at `now` with the camera at `camera`: during a pulse, or for a new one when it is
    /// `allowed` (recording, the rig following the camera, no flight of the app's), [`FIRST_PULSE_AFTER`] after the
    /// first step, then `every` at most and only [`PULSE_AFTER_M`] from every earlier pulse's place.
    pub fn step(&mut self, now: Instant, camera: Option<[f32; 3]>, allowed: bool, every: Duration) -> bool {
        let started = *self.started.get_or_insert(now);
        if !allowed {
            self.until = None;
            return false;
        }
        if let Some(until) = self.until {
            if now < until {
                return true;
            }
            self.until = None;
        }
        let Some(at) = camera else { return false };
        let due = match self.last {
            None => now.duration_since(started) >= FIRST_PULSE_AFTER,
            Some(when) => now.duration_since(when) >= every && self.places.iter().all(|p| distance(at, *p) >= PULSE_AFTER_M),
        };
        if due {
            self.last = Some(now);
            self.places.push(at);
            self.until = Some(now + PULSE);
        }
        due
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Whether the rig should be placed again for `values` (the four parameters), last sent as `sent`.
fn moved(values: &[f32; 4], sent: &[f32; 4]) -> bool {
    use orbiters_contracts::rig::{REACH_M, YAW_DEG};
    let step = |k: usize| (values[k] - sent[k]) * REACH_M;
    (step(0).powi(2) + step(1).powi(2) + step(2).powi(2)).sqrt() >= MOVE_M || ((values[3] - sent[3]) * YAW_DEG).abs() >= TURN_DEG
}

/// The four values for a camera `pose` with the hips at `hips`.
pub fn values(pose: &[f32; 6], hips: &Hips) -> [f32; 4] {
    orbiters_contracts::rig::follow_values(hips.0, hips.1, [pose[0], pose[1], pose[2]], pose[4])
}

impl super::Controller {
    /// Where VRChat last said its camera is (`/usercamera/Pose`: x, y, z in metres, pitch, yaw, roll in degrees).
    pub(super) fn camera_pose(&self) -> Option<[f32; 6]> {
        let osc = self.osc_values.locked();
        osc.latest.get(POSE).map(|(v, _)| floats(v)).filter(|v| v.len() == 6 && v.iter().any(|f| *f != 0.0)).and_then(|v| v.try_into().ok())
    }

    /// The capture worker read the avatar's hips from a frame's skeleton band.
    pub(super) fn rig_hips(&self, position: [f32; 3], rotation: [f32; 4]) {
        if position.iter().chain(&rotation).all(|v| v.is_finite()) {
            self.rig.locked().hips = Some((position, rotation));
        }
    }

    /// Puts the rig at a camera `pose`, with `camera` (the camera's own pose message, a flight's) in the same bundle:
    /// VRChat applies them together. Until a frame showed where the hips are, only the camera moves.
    pub(super) fn place_rig(&self, camera: Option<(String, Vec<rosc::OscType>)>, pose: &[f32; 6]) {
        let hips = self.rig.locked().hips;
        let mut messages: Vec<_> = camera.into_iter().collect();
        if let Some(hips) = &hips {
            messages.extend(rig_parameters(pose, hips));
        }
        if messages.is_empty() {
            return;
        }
        if self.with_osc(|o| o.send_bundle(messages)).is_some_and(|r| r.is_ok()) && hips.is_some() {
            self.rig.locked().following = true;
        }
    }

    /// Where the capture kit last said the avatar's hips are, during a recording.
    pub(super) fn hips_now(&self) -> Option<[f32; 3]> {
        self.rig.locked().hips.map(|h| h.0)
    }

    /// Switches the rig on or off when it is not already.
    fn switch_rig(&self, on: bool) {
        let mut rig = self.rig.locked();
        if rig.on != on && self.with_osc(|o| o.send_bundle(vec![rig_switch(on)])).is_some_and(|r| r.is_ok()) {
            rig.on = on;
        }
    }

    /// A 3D picture holds the rig on for its flight, or lets go of it.
    pub(super) fn hold_rig(&self, on: bool) {
        self.rig.locked().held = on;
        self.switch_rig(on);
    }

    /// Starts following VRChat's camera with the rig for the recording starting now.
    pub(super) fn start_rig(self: &Shared) {
        let generation = {
            let mut rig = self.rig.locked();
            rig.generation += 1;
            // Where the hips are, from this recording's frames.
            rig.hips = None;
            rig.generation
        };
        let ctl = self.clone();
        if let Err(e) = std::thread::Builder::new().name("n4n0-rig".into()).spawn(move || ctl.follow_camera(generation)) {
            log_warn!("rig: {e}", e = e);
        }
    }

    /// The rig off and back at the hips, its follower ended (the recording is over).
    pub(super) fn stop_rig(&self) {
        let (on, following) = {
            let mut rig = self.rig.locked();
            rig.generation += 1;
            rig.held = false;
            (std::mem::take(&mut rig.on), std::mem::take(&mut rig.following))
        };
        let mut messages = Vec::new();
        if on {
            messages.push(rig_switch(false));
        }
        if following {
            messages.push(follow(false));
        }
        if !messages.is_empty() {
            let _ = self.with_osc(|o| o.send_bundle(messages));
        }
    }

    /// A new avatar (or the same one reloaded) starts with N4N0's parameters at their defaults: the rig rests at its
    /// hips, off, until the follower places it again from where its frames show the new avatar's hips.
    pub(super) fn rig_avatar_changed(&self) {
        let mut rig = self.rig.locked();
        rig.following = false;
        rig.on = false;
        rig.hips = None;
    }

    /// The follower: the rig at the camera while recording (unless a flight places it), and its pulses.
    fn follow_camera(self: &Shared, generation: u64) {
        let mut pulses = Pulses::default();
        let mut sent: Option<([f32; 4], Instant)> = None;
        loop {
            std::thread::sleep(TICK);
            let (current, following, held, hips) = {
                let rig = self.rig.locked();
                (rig.generation == generation, rig.following, rig.held, rig.hips)
            };
            if !current || !self.is_capturing() {
                return;
            }
            let (state, position) = {
                let snap = self.snapshot.locked();
                (snap.capture.state, snap.capture.stats.as_ref().and_then(|s| s.position))
            };
            let flying = self.burst.locked().recording || self.scan.locked().recording;
            let pose = self.camera_pose();
            if !flying {
                if let (Some(p), Some(h)) = (pose, hips) {
                    let v = values(&p, &h);
                    let due = !following || sent.is_none_or(|(last, at)| moved(&v, &last) || at.elapsed() >= RESEND);
                    if due {
                        self.place_rig(None, &p);
                        sent = Some((v, Instant::now()));
                    }
                }
            }
            // Where the camera is: the strip's exact position, else what VRChat said.
            let camera = position.or(pose.map(|p| [p[0], p[1], p[2]]));
            let every = self.with_settings(|s| s.rig_views).every();
            let allowed = every.is_some() && !held && !flying && following && state == CaptureState::Recording;
            let pulse = pulses.step(Instant::now(), camera, allowed, every.unwrap_or(Duration::MAX));
            if !held {
                self.switch_rig(pulse);
            }
        }
    }
}

/// The rig follows the camera, or rests at the hips.
pub fn follow(on: bool) -> (String, Vec<rosc::OscType>) {
    (format!("{}{}", orbiters_core::osc::PARAMETERS_PREFIX, orbiters_contracts::rig::PARAMETER_FOLLOW), vec![rosc::OscType::Bool(on)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::settings::RigViews;

    const EVERY: Duration = Duration::from_secs(3);

    #[test]
    fn pulses_come_every_few_seconds_where_the_rig_has_not_looked_around() {
        let t0 = Instant::now();
        let at = |s: f32| t0 + Duration::from_secs_f32(s);
        let mut p = Pulses::default();
        assert!(!p.step(at(0.0), Some([0.0; 3]), true, EVERY), "not before the recording settled");
        assert!(!p.step(at(1.0), Some([0.0; 3]), true, EVERY));
        assert!(p.step(at(2.0), Some([0.0; 3]), true, EVERY), "the first one once it settled");
        assert!(p.step(at(2.2), Some([0.0; 3]), true, EVERY), "on for the pulse's length");
        assert!(!p.step(at(2.31), Some([0.0; 3]), true, EVERY), "then off");
        // Standing still: nothing new to see.
        assert!(!p.step(at(10.0), Some([0.3, 0.0, 0.0]), true, EVERY));
        // Half a metre on, but too soon after the last one.
        assert!(!p.step(at(4.0), Some([0.6, 0.0, 0.0]), true, EVERY));
        assert!(p.step(at(5.1), Some([0.6, 0.0, 0.0]), true, EVERY), "far enough and long enough after");
        // Back where the first one looked around: nothing new either.
        assert!(!p.step(at(9.0), Some([0.1, 0.0, 0.0]), true, EVERY));
        // Not allowed (paused, a flight of the app's, the rig not following): a pulse under way ends.
        assert!(p.step(at(12.0), Some([1.2, 0.0, 0.0]), true, EVERY));
        assert!(!p.step(at(12.1), Some([1.2, 0.0, 0.0]), false, EVERY));
        assert!(!p.step(at(12.15), Some([1.2, 0.0, 0.0]), true, EVERY), "and does not come back");
        // No camera position: no pulse.
        assert!(!p.step(at(20.0), None, true, EVERY));
        assert!(p.step(at(20.0), Some([5.0, 0.0, 0.0]), true, EVERY));
        // Off: never.
        assert_eq!(RigViews::Off.every(), None);
    }

    /// Stepped as the follower does, walking at `speed` m/s for a minute: the pulses, and the share of the colour
    /// frames that become rig frames (about half the frames while the rig is on).
    fn walk(speed: f32, every: Duration) -> (u32, f32) {
        let t0 = Instant::now();
        let mut p = Pulses::default();
        let (mut on, mut pulses, mut was) = (0u32, 0u32, false);
        let steps = 60_000 / 33;
        for i in 0..steps {
            let s = i as f32 * 0.033;
            let now = p.step(t0 + Duration::from_secs_f32(s), Some([s * speed, 0.0, 0.0]), true, every);
            on += now as u32;
            pulses += (now && !was) as u32;
            was = now;
        }
        (pulses, on as f32 / steps as f32 / 2.0)
    }

    #[test]
    fn a_walk_loses_few_frames_to_rig_frames() {
        // Every 3 s at 1 m/s: a pulse every 3 m.
        let (pulses, share) = walk(1.0, EVERY);
        assert!((19..=20).contains(&pulses), "{pulses}");
        assert!(share < 0.06, "{share}");
        // Every 2 s: under 8.5 %; every 5 s: under 3.5 %.
        assert!(walk(1.0, Duration::from_secs(2)).1 < 0.085);
        assert!(walk(1.0, Duration::from_secs(5)).1 < 0.035);
        // Standing (a camera held still): one pulse, nothing after.
        assert_eq!(walk(0.0, EVERY).0, 1);
    }

    #[test]
    fn the_rig_is_placed_again_as_its_place_from_the_hips_changes() {
        let hips: Hips = ([15.08, -3.12, 6.8], [0.0, 0.0, 0.0, 1.0]);
        let at = values(&[1.0, 1.5, 2.0, 10.0, 90.0, 0.0], &hips);
        assert!(!moved(&at, &at));
        assert!(moved(&values(&[1.03, 1.5, 2.0, 10.0, 90.0, 0.0], &hips), &at));
        assert!(moved(&values(&[1.0, 1.5, 2.0, 10.0, 91.5, 0.0], &hips), &at));
        assert!(!moved(&values(&[1.0, 1.5, 2.0, 10.0, -269.5, 0.0], &hips), &at), "the same heading a turn round");
        // Pitch and roll do not turn the rig (it stays level).
        assert!(!moved(&values(&[1.0, 1.5, 2.0, 40.0, 90.0, 20.0], &hips), &at));
        // The hips walked on: the same camera is somewhere else from them.
        assert!(moved(&values(&[1.0, 1.5, 2.0, 10.0, 90.0, 0.0], &([15.2, -3.12, 6.8], hips.1)), &at));
    }
}
