//! 3D picture: a recording of three seconds during which VRChat's camera (`/usercamera/Pose`, VRChat 2025.3.3 and
//! later) turns in place looking straight up and straight down, then flies 10 m forward, to each side and up and back,
//! turning as it goes, so the place is seen from every side; then it is back where it was. The capture kit stamps every
//! frame with the camera's pose, so the session reconstructs into splats like any recording; the reconstruction starts
//! as soon as the recording is saved.

use super::{NoticeKind, Shared};
use crate::state::CaptureState;
use orbiters_core::osc::OscValue;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const POSE: &str = "/usercamera/Pose";
const SMOOTH: &str = "/usercamera/SmoothMovement";
/// How far the camera flies from where it stands, each way.
pub const REACH_M: f32 = 10.0;
/// Straight up or down, short of the pole where yaw means nothing.
const STRAIGHT: f32 = 88.0;
/// Poses a second while flying, and how long the flight takes.
const RATE_HZ: u32 = 90;
pub const FLIGHT_SECONDS: f32 = 3.0;
/// After the recording started, before the camera moves: Spout and the kit's first frames.
const SETTLE: Duration = Duration::from_millis(700);

/// A point of the flight: where the camera is (metres right, up and forward of where it started, as it faced), how far
/// it turned (degrees, added to its yaw), its pitch (degrees, positive looks down; the first and last keys keep the
/// camera's own) and its share of the flight's time to get there.
struct Key {
    right: f32,
    up: f32,
    forward: f32,
    turn: f32,
    pitch: f32,
    seconds: f32,
}

const fn key(right: f32, up: f32, forward: f32, turn: f32, pitch: f32, seconds: f32) -> Key {
    Key { right, up, forward, turn, pitch, seconds }
}

/// The flight, from the start: a turn in place looking up then down, out 10 m forward and back looking around, to the
/// right and across to the left, up looking down, and home, three full turns in all.
const FLIGHT: &[Key] = &[
    key(0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
    key(0.0, 0.0, 0.0, 120.0, -STRAIGHT, 0.45),
    key(0.0, 0.0, 0.0, 240.0, STRAIGHT, 0.45),
    key(0.0, 0.0, 0.0, 360.0, 0.0, 0.45),
    key(0.0, 0.0, REACH_M, 360.0, 0.0, 0.6),
    key(0.0, 0.0, REACH_M, 540.0, 15.0, 0.45),
    key(0.0, 0.0, 0.0, 540.0, 0.0, 0.6),
    key(REACH_M, 0.0, 0.0, 630.0, 0.0, 0.6),
    key(-REACH_M, 0.0, 0.0, 810.0, 0.0, 1.0),
    key(0.0, 0.0, 0.0, 900.0, 0.0, 0.6),
    key(0.0, REACH_M, 0.0, 990.0, STRAIGHT, 0.7),
    key(0.0, 0.0, 0.0, 1080.0, 0.0, 0.7),
];

/// Eases in and out between two keys.
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// The camera's poses along the flight at `rate` a second: `pose` is VRChat's (x, y, z in metres, then pitch, yaw, roll
/// in degrees, Unity's conventions). Forward and right are where the camera faced at the start, kept level; the last
/// pose is the first one, three turns later.
pub fn flight(pose: [f32; 6], rate: u32) -> Vec<[f32; 6]> {
    let [x, y, z, pitch, yaw, roll] = pose;
    let shares: f32 = FLIGHT.iter().map(|k| k.seconds).sum();
    let last = FLIGHT.len() - 1;
    let pitch_of = |i: usize| if i == 0 || i == last { pitch } else { FLIGHT[i].pitch };
    let w = yaw.to_radians();
    // Unity: +X right, +Y up, +Z forward; a yaw turns forward towards +X.
    let (forward, right) = ([w.sin(), 0.0, w.cos()], [w.cos(), 0.0, -w.sin()]);
    let at = |k: &Key| -> [f32; 3] { [x + right[0] * k.right + forward[0] * k.forward, y + k.up, z + right[2] * k.right + forward[2] * k.forward] };
    let mut poses = vec![pose];
    for (n, pair) in FLIGHT.windows(2).enumerate() {
        let (a, b) = (&pair[0], &pair[1]);
        let (pitch_a, pitch_b) = (pitch_of(n), pitch_of(n + 1));
        let steps = ((b.seconds / shares * FLIGHT_SECONDS * rate as f32).round() as usize).max(1);
        for i in 1..=steps {
            let t = smooth(i as f32 / steps as f32);
            let (pa, pb) = (at(a), at(b));
            let lerp = |u: f32, v: f32| u + (v - u) * t;
            poses.push([lerp(pa[0], pb[0]), lerp(pa[1], pb[1]), lerp(pa[2], pb[2]), lerp(pitch_a, pitch_b), yaw + lerp(a.turn, b.turn), roll]);
        }
    }
    poses
}

/// What a 3D picture waits for once its recording stops.
#[derive(Default)]
pub struct BurstRuntime {
    /// A 3D picture's recording is under way (its reconstruction starts when it is saved).
    pub(super) recording: bool,
}

fn floats(values: &[OscValue]) -> Vec<f32> {
    values.iter().filter_map(|v| if let OscValue::Float(f) = v { Some(*f) } else { None }).collect()
}

impl super::Controller {
    /// Takes a 3D picture with VRChat's camera where it stands now.
    pub fn take_3d_picture(self: &Shared) -> anyhow::Result<()> {
        anyhow::ensure!(self.snapshot.lock().unwrap().vrchat.running, "Start VRChat and open its camera first.");
        anyhow::ensure!(!self.is_capturing(), "A recording is under way: stop it first.");
        let (pose, smooth) = {
            let osc = self.osc_values.lock().unwrap();
            let pose = osc.latest.get(POSE).map(|(v, _)| floats(v)).filter(|v| v.len() == 6 && v.iter().any(|f| *f != 0.0));
            let smooth = osc.latest.get(SMOOTH).and_then(|(v, _)| v.first().map(OscValue::truthy));
            (pose, smooth)
        };
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
            let tick = Duration::from_secs_f32(1.0 / RATE_HZ as f32);
            let begin = Instant::now();
            for (i, p) in flight(pose, RATE_HZ).into_iter().enumerate() {
                send(POSE, p.iter().map(|v| rosc::OscType::Float(*v)).collect());
                let next = tick * (i as u32 + 1);
                if let Some(wait) = next.checked_sub(begin.elapsed()) {
                    std::thread::sleep(wait);
                }
            }
            send(POSE, pose.iter().map(|v| rosc::OscType::Float(*v)).collect());
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

    /// A recording was saved: a 3D picture's goes on to its reconstruction.
    pub(super) fn burst_saved(self: &Shared, session: Option<PathBuf>) {
        let was = std::mem::take(&mut self.burst.lock().unwrap().recording);
        let Some(dir) = session.filter(|_| was) else { return };
        match self.reconstruct(dir) {
            Ok(()) => self.notify(NoticeKind::Success, "3D picture taken: it is being reconstructed."),
            Err(e) => self.notify(NoticeKind::Warning, format!("3D picture taken, not reconstructed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_camera_looks_every_way_and_flies_ten_metres_out() {
        // At (1, 1.5, 0), facing +X (yaw 90).
        let pose = [1.0, 1.5, 0.0, 10.0, 90.0, 0.0];
        let poses = flight(pose, 90);
        assert_eq!(poses.first(), Some(&pose));
        let end = poses.last().unwrap();
        assert!(end.iter().zip(pose).enumerate().all(|(i, (a, b))| if i == 4 { ((a - b) % 360.0).abs() < 1e-3 } else { (a - b).abs() < 1e-4 }), "{end:?}");
        assert!((poses.len() as f32 - FLIGHT_SECONDS * 90.0).abs() <= 12.0, "{} poses", poses.len());
        // Straight up and straight down.
        let pitches = poses.iter().map(|p| p[3]);
        assert!(pitches.clone().fold(f32::MAX, f32::min) <= -85.0 && pitches.fold(f32::MIN, f32::max) >= 85.0);
        // Every heading.
        let mut headings: Vec<i32> = poses.iter().map(|p| (p[4].rem_euclid(360.0) / 30.0) as i32).collect();
        headings.sort();
        headings.dedup();
        assert_eq!(headings.len(), 12, "{headings:?}");
        // 10 m forward (+X here), 10 m to each side (±Z: right of +X is −Z) and 10 m up.
        let reach = |f: &dyn Fn(&[f32; 6]) -> f32| poses.iter().map(f).fold(f32::MIN, f32::max);
        assert!(reach(&|p| p[0] - pose[0]) > 9.9, "forward");
        assert!(reach(&|p| pose[2] - p[2]) > 9.9, "right");
        assert!(reach(&|p| p[2] - pose[2]) > 9.9, "left");
        assert!(reach(&|p| p[1] - pose[1]) > 9.9, "up");
        // No jump between two poses (90 a second): under a metre and 20°.
        for w in poses.windows(2) {
            let d = ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2) + (w[1][2] - w[0][2]).powi(2)).sqrt();
            assert!(d < 1.0 && (w[1][4] - w[0][4]).abs() < 20.0 && (w[1][3] - w[0][3]).abs() < 20.0, "{w:?}");
        }
    }
}
