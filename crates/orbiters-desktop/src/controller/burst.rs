//! 3D picture: a short recording during which VRChat's camera (`/usercamera/Pose`, VRChat 2025.3.3 and later) moves
//! on its own, then comes back where it was. The capture kit stamps every frame with the camera's pose, so the session
//! reconstructs into splats like any recording; the reconstruction starts as soon as the recording is saved. Three
//! lengths ([`Photo3dLength`]): a gentle 1 s sweep around what the camera aims at; 4 s that circle the starting point
//! looking at it (the person taking the picture) then turn in place looking straight up and down and fly 10 m forward,
//! to each side and up and back; 8 s that add four circles around points 8 m away.
//!
//! A capture kit with the camera rig (contracts' Docs/rig-frames.md) is switched to it for the flight and follows the
//! camera: about every other colour frame then carries six more views, one along each axis around the camera.

use super::{NoticeKind, Shared};
use crate::state::CaptureState;
use orbiters_contracts::rig;
use orbiters_core::osc::{OscValue, PARAMETERS_PREFIX};
use orbiters_core::settings::Photo3dLength;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const POSE: &str = "/usercamera/Pose";
const FOCAL_DISTANCE: &str = "/usercamera/FocalDistance";
pub(super) const SMOOTH: &str = "/usercamera/SmoothMovement";
/// How far the flight goes from where the camera stands, each way.
pub const REACH_M: f32 = 10.0;
/// Straight up or down, short of the pole where yaw means nothing.
const STRAIGHT: f32 = 88.0;
/// Poses a second.
const RATE_HZ: u32 = 90;
/// After the recording started, before the camera moves: Spout and the kit's first frames.
const SETTLE: Duration = Duration::from_millis(700);

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

/// The capture rig's parameters for a camera pose: where it stands (each axis over [`rig::REACH_M`]) and its heading
/// (over [`rig::YAW_DEG`]), level. VRChat takes OSC floats from -1 to 1.
pub fn rig_parameters(pose: &[f32; 6]) -> Vec<(String, Vec<rosc::OscType>)> {
    let at = |v: f32| rosc::OscType::Float((v / rig::REACH_M).clamp(-1.0, 1.0));
    let yaw = (pose[4] + 180.0).rem_euclid(360.0) - 180.0;
    [(rig::PARAMETER_X, at(pose[0])), (rig::PARAMETER_Y, at(pose[1])), (rig::PARAMETER_Z, at(pose[2])), (rig::PARAMETER_YAW, rosc::OscType::Float(yaw / rig::YAW_DEG))]
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
    /// The capture rig was switched on for it: switched off when VRChat is put back, whatever ends the recording.
    pub(super) rig: bool,
}

pub(super) fn floats(values: &[OscValue]) -> Vec<f32> {
    values.iter().filter_map(|v| if let OscValue::Float(f) = v { Some(*f) } else { None }).collect()
}

impl super::Controller {
    /// Takes a 3D picture with VRChat's camera where it stands now.
    pub fn take_3d_picture(self: &Shared) -> anyhow::Result<()> {
        anyhow::ensure!(self.snapshot.lock().unwrap().vrchat.running, "Start VRChat and open its camera first.");
        self.ensure_capture_idle()?;
        let (pose, focus, smooth) = {
            let osc = self.osc_values.lock().unwrap();
            let pose = osc.latest.get(POSE).map(|(v, _)| floats(v)).filter(|v| v.len() == 6 && v.iter().any(|f| *f != 0.0));
            let focus = osc.latest.get(FOCAL_DISTANCE).and_then(|(v, _)| floats(v).first().copied()).unwrap_or(2.0).clamp(0.5, 8.0);
            let smooth = osc.latest.get(SMOOTH).and_then(|(v, _)| v.first().map(OscValue::truthy));
            (pose, focus, smooth)
        };
        let length = self.settings().photo_3d;
        let Some(pose) = pose else {
            anyhow::bail!("Move VRChat's camera once so Orbiters knows where it is, then try again.");
        };
        let pose: [f32; 6] = pose.try_into().expect("six values");
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
            // The rig where the camera starts, then on (an avatar without it ignores both).
            let _ = ctl.with_osc(|o| o.send_bundle(rig_parameters(&pose)));
            ctl.burst.lock().unwrap().rig = true;
            let _ = ctl.with_osc(|o| o.send_bundle(vec![rig_switch(true)]));
            let tick = Duration::from_secs_f32(1.0 / RATE_HZ as f32);
            let begin = Instant::now();
            for (i, p) in path(pose, length, focus, RATE_HZ).into_iter().enumerate() {
                // The camera and the rig in one bundle: VRChat applies them together.
                let mut messages = vec![(POSE.to_string(), p.iter().map(|v| rosc::OscType::Float(*v)).collect())];
                messages.extend(rig_parameters(&p));
                let _ = ctl.with_osc(|o| o.send_bundle(messages));
                let next = tick * (i as u32 + 1);
                if let Some(wait) = next.checked_sub(begin.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            send(POSE, pose.iter().map(|v| rosc::OscType::Float(*v)).collect());
            ctl.switch_rig_off();
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

    /// The capture rig off, when a 3D picture switched it on.
    pub(super) fn switch_rig_off(&self) {
        if std::mem::take(&mut self.burst.lock().unwrap().rig) {
            let _ = self.with_osc(|o| o.send_bundle(vec![rig_switch(false)]));
        }
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

    #[test]
    fn the_rig_follows_the_camera_within_vrchats_range() {
        let values = |pose: [f32; 6]| -> Vec<f32> {
            rig_parameters(&pose).into_iter().map(|(_, v)| if let rosc::OscType::Float(f) = v[0] { f } else { panic!() }).collect()
        };
        assert_eq!(rig_parameters(&POSE).iter().map(|(a, _)| a.as_str()).collect::<Vec<_>>(), [
            "/avatar/parameters/OrbitersRig/X",
            "/avatar/parameters/OrbitersRig/Y",
            "/avatar/parameters/OrbitersRig/Z",
            "/avatar/parameters/OrbitersRig/Yaw"
        ]);
        let v = values([2048.0, 1.5, -409.6, 10.0, 270.0, 0.0]);
        assert_eq!(v[0], 0.5);
        assert!((v[1] * rig::REACH_M - 1.5).abs() < 1e-4 && (v[2] + 0.1).abs() < 1e-6);
        // 270° is -90°: half a turn left of straight ahead.
        assert!((v[3] + 0.5).abs() < 1e-6);
        // Beyond the reach: at its edge.
        assert_eq!(values([9000.0, 0.0, -9000.0, 0.0, -540.0, 0.0])[..3], [1.0, 0.0, -1.0]);
        assert!((values([0.0; 6])[3]).abs() < 1e-6);
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
