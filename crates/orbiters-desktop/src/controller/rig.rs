//! N4N0's camera rig while recording (contracts' `Docs/rig-frames.md`, N4N0's capture kit).
//!
//! While the app drives the avatar during a recording, the rig follows VRChat's camera: `N4N0/Capture/Follow` on, and
//! `N4N0/Capture/RigX`, `RigY`, `RigZ` and `RigYaw` from where VRChat says its camera is (`/usercamera/Pose`). N4N0's
//! capture kit keeps its bounds around the rig, so it draws wherever the camera goes (without that, only while the
//! camera is within about 2 m of the avatar or sees it). A 3D picture and a scan place the rig themselves, in the same
//! bundle as each camera pose they send.
//!
//! Every few metres the camera goes, the rig turns on for a moment (`N4N0/Capture/Rig`, [`PULSE`]): about every
//! other colour frame meanwhile is a rig frame, six more views around the camera (left, front, right, back, up, down)
//! that the capture worker records as views of their own and the reconstruction trains on, as for a 3D picture, which
//! holds the rig on for its whole flight. Why these numbers: a pulse of 0.3 s is a dozen rig frames at VRChat's 90
//! frames a second (a quarter of the frames are depth frames, half of the rest rig frames), enough for every face to be
//! read even when a frame or two of them come late; at most one every [`PULSE_EVERY`] and only once the camera went
//! [`PULSE_AFTER_M`] from the last one (from where the rig already looked all around, the same six views add nothing),
//! so a walk at 1 m/s gives six views every 4 m while a recording loses under 5 % of its frames to rig frames, and
//! VRChat renders the world six more times for 0.3 s in 4 s rather than all along.

use super::burst::{floats, rig_parameters, rig_switch, POSE};
use super::Shared;
use crate::state::CaptureState;
use std::time::{Duration, Instant};

/// How often the follower looks at where VRChat's camera is.
const TICK: Duration = Duration::from_millis(33);
/// The rig is placed again once the camera moved this far or turned this much, and at least this often.
const MOVE_M: f32 = 0.02;
const TURN_DEG: f32 = 1.0;
const RESEND: Duration = Duration::from_secs(1);
/// How long the rig stays on for each pulse.
pub const PULSE: Duration = Duration::from_millis(300);
/// The shortest time between two pulses.
pub const PULSE_EVERY: Duration = Duration::from_secs(4);
/// How far the camera must be from where the last pulse was for the next one.
pub const PULSE_AFTER_M: f32 = 1.5;
/// The first pulse, once the recording settled (Spout, the kit's first frames, the rig placed).
pub const FIRST_PULSE_AFTER: Duration = Duration::from_secs(2);

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
}

/// When to switch the rig on for a moment.
#[derive(Default)]
pub struct Pulses {
    started: Option<Instant>,
    /// When and where (the camera's position) the last pulse began.
    last: Option<(Instant, [f32; 3])>,
    until: Option<Instant>,
}

impl Pulses {
    /// Whether the rig should be on at `now` with the camera at `camera`: during a pulse, or for a new one when
    /// `allowed` (recording, the rig following the camera, nothing else holding it), [`FIRST_PULSE_AFTER`] after the
    /// first step, then [`PULSE_EVERY`] at most and once the camera went [`PULSE_AFTER_M`] from the last one.
    pub fn step(&mut self, now: Instant, camera: Option<[f32; 3]>, allowed: bool) -> bool {
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
            Some((when, from)) => now.duration_since(when) >= PULSE_EVERY && distance(at, from) >= PULSE_AFTER_M,
        };
        if due {
            self.last = Some((now, at));
            self.until = Some(now + PULSE);
        }
        due
    }
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Whether the rig should be placed again for a camera at `pose`, last placed for `sent`.
fn moved(pose: &[f32; 6], sent: &[f32; 6]) -> bool {
    let turn = |a: f32, b: f32| ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs();
    distance([pose[0], pose[1], pose[2]], [sent[0], sent[1], sent[2]]) >= MOVE_M || turn(pose[4], sent[4]) >= TURN_DEG
}

impl super::Controller {
    /// Where VRChat last said its camera is (`/usercamera/Pose`: x, y, z in metres, pitch, yaw, roll in degrees).
    fn camera_pose(&self) -> Option<[f32; 6]> {
        let osc = self.osc_values.lock().unwrap();
        osc.latest.get(POSE).map(|(v, _)| floats(v)).filter(|v| v.len() == 6 && v.iter().any(|f| *f != 0.0)).and_then(|v| v.try_into().ok())
    }

    /// Puts the rig at a camera `pose`, with `camera` (the camera's own pose message, a flight's) in the same bundle:
    /// VRChat applies them together.
    pub(super) fn place_rig(&self, camera: Option<(String, Vec<rosc::OscType>)>, pose: &[f32; 6]) {
        let mut messages: Vec<_> = camera.into_iter().collect();
        messages.extend(rig_parameters(pose));
        if self.with_osc(|o| o.send_bundle(messages)).is_some_and(|r| r.is_ok()) {
            self.rig.lock().unwrap().following = true;
        }
    }

    /// Switches the rig on or off when it is not already.
    fn switch_rig(&self, on: bool) {
        let mut rig = self.rig.lock().unwrap();
        if rig.on != on && self.with_osc(|o| o.send_bundle(vec![rig_switch(on)])).is_some_and(|r| r.is_ok()) {
            rig.on = on;
        }
    }

    /// A 3D picture holds the rig on for its flight, or lets go of it.
    pub(super) fn hold_rig(&self, on: bool) {
        self.rig.lock().unwrap().held = on;
        self.switch_rig(on);
    }

    /// Starts following VRChat's camera with the rig for the recording starting now.
    pub(super) fn start_rig(self: &Shared) {
        let generation = {
            let mut rig = self.rig.lock().unwrap();
            rig.generation += 1;
            rig.generation
        };
        let ctl = self.clone();
        if let Err(e) = std::thread::Builder::new().name("n4n0-rig".into()).spawn(move || ctl.follow_camera(generation)) {
            eprintln!("rig: {e}");
        }
    }

    /// The rig off and back at the hips, its follower ended (the recording is over).
    pub(super) fn stop_rig(&self) {
        let (on, following) = {
            let mut rig = self.rig.lock().unwrap();
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
    /// hips, off, until the follower places it again.
    pub(super) fn rig_avatar_changed(&self) {
        let mut rig = self.rig.lock().unwrap();
        rig.following = false;
        rig.on = false;
    }

    /// The follower: the rig at the camera while recording (unless a flight places it), and its pulses.
    fn follow_camera(self: &Shared, generation: u64) {
        let mut pulses = Pulses::default();
        let mut sent: Option<([f32; 6], Instant)> = None;
        loop {
            std::thread::sleep(TICK);
            let (current, following, held) = {
                let rig = self.rig.lock().unwrap();
                (rig.generation == generation, rig.following, rig.held)
            };
            if !current || !self.is_capturing() {
                return;
            }
            let (state, position) = {
                let snap = self.snapshot.lock().unwrap();
                (snap.capture.state, snap.capture.stats.as_ref().and_then(|s| s.position))
            };
            let flying = self.burst.lock().unwrap().recording || self.scan.lock().unwrap().recording;
            let pose = self.camera_pose();
            if !flying {
                if let Some(p) = pose {
                    let due = !following || sent.is_none_or(|(last, at)| moved(&p, &last) || at.elapsed() >= RESEND);
                    if due {
                        self.place_rig(None, &p);
                        sent = Some((p, Instant::now()));
                    }
                }
            }
            // Where the camera is: the strip's exact position, else what VRChat said.
            let camera = position.or(pose.map(|p| [p[0], p[1], p[2]]));
            let allowed = self.with_settings(|s| s.rig_views) && !held && following && state == CaptureState::Recording;
            let pulse = pulses.step(Instant::now(), camera, allowed);
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

    #[test]
    fn pulses_come_every_few_metres_and_never_standing_still() {
        let t0 = Instant::now();
        let at = |s: f32| t0 + Duration::from_secs_f32(s);
        let mut p = Pulses::default();
        assert!(!p.step(at(0.0), Some([0.0; 3]), true), "not before the recording settled");
        assert!(!p.step(at(1.0), Some([0.0; 3]), true));
        assert!(p.step(at(2.0), Some([0.0; 3]), true), "the first one once it settled");
        assert!(p.step(at(2.2), Some([0.0; 3]), true), "on for the pulse's length");
        assert!(!p.step(at(2.31), Some([0.0; 3]), true), "then off");
        // Standing still: nothing new to see.
        assert!(!p.step(at(10.0), Some([0.5, 0.0, 0.0]), true));
        // 1.5 m on, but too soon after the last one.
        assert!(!p.step(at(5.0), Some([1.6, 0.0, 0.0]), true));
        assert!(p.step(at(6.1), Some([1.6, 0.0, 0.0]), true), "far enough and long enough after");
        // Not allowed (paused, a 3D picture holding the rig, the rig not following): a pulse under way ends.
        assert!(!p.step(at(6.2), Some([1.6, 0.0, 0.0]), false));
        assert!(!p.step(at(6.25), Some([1.6, 0.0, 0.0]), true), "and does not come back");
        // No camera position: no pulse.
        assert!(!p.step(at(20.0), None, true));
        assert!(p.step(at(20.0), Some([5.0, 0.0, 0.0]), true));
    }

    #[test]
    fn a_walk_loses_few_frames_to_rig_frames() {
        // Walking at 1 m/s for a minute, stepped as the follower does.
        let t0 = Instant::now();
        let mut p = Pulses::default();
        let (mut on, mut pulses, mut was) = (0u32, 0u32, false);
        for i in 0..(60_000 / 33) {
            let s = i as f32 * 0.033;
            let now = p.step(t0 + Duration::from_secs_f32(s), Some([s, 0.0, 0.0]), true);
            on += now as u32;
            pulses += (now && !was) as u32;
            was = now;
        }
        assert!((14..=15).contains(&pulses), "a pulse every 4 m: {pulses}");
        // About half the frames of the rig's time on are rig frames: under 5 % of the recording.
        let share = on as f32 / (60_000 / 33) as f32 / 2.0;
        assert!(share < 0.05, "{share}");
    }

    #[test]
    fn the_rig_is_placed_again_as_the_camera_moves() {
        let at = [1.0, 1.5, 2.0, 10.0, 90.0, 0.0];
        assert!(!moved(&at, &at));
        assert!(moved(&[1.03, 1.5, 2.0, 10.0, 90.0, 0.0], &at));
        assert!(moved(&[1.0, 1.5, 2.0, 10.0, 91.5, 0.0], &at));
        assert!(!moved(&[1.0, 1.5, 2.0, 10.0, -269.5, 0.0], &at), "the same heading a turn round");
        // Pitch and roll do not turn the rig (it stays level).
        assert!(!moved(&[1.0, 1.5, 2.0, 40.0, 90.0, 20.0], &at));
    }
}
