//! 3D picture: a recording of two seconds during which VRChat's camera flies a full circle around what it looks at
//! (`/usercamera/Pose`, VRChat 2025.3.3 and later), rising and dipping, so the place is seen from every side; then it
//! goes back where it was. The capture kit stamps every
//! frame with the camera's pose, so the session reconstructs into splats like any recording; the reconstruction starts
//! as soon as the recording is saved.

use super::{NoticeKind, Shared};
use crate::state::CaptureState;
use orbiters_core::osc::OscValue;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub const POSE: &str = "/usercamera/Pose";
const FOCAL_DISTANCE: &str = "/usercamera/FocalDistance";
const SMOOTH: &str = "/usercamera/SmoothMovement";
/// How far the camera rises and dips along the circle, and how long the circle takes.
const RISE_M: f32 = 0.5;
const SWING: Duration = Duration::from_millis(2000);
/// Poses a second while swinging.
const RATE_HZ: u32 = 60;
/// After the recording started, before the camera moves: Spout and the kit's first frames.
const SETTLE: Duration = Duration::from_millis(700);

/// The camera's poses along the swing: `pose` is VRChat's (x, y, z in metres, then pitch, yaw, roll in degrees, Unity's
/// conventions), `focus` how far ahead the subject is. The camera flies once around the subject at that distance (a
/// circle `2 × focus` wide), rising and dipping twice, and keeps looking at it, so over the turn it looks out in every
/// direction; the last pose is the first one.
pub fn swing(pose: [f32; 6], focus: f32, steps: usize) -> Vec<[f32; 6]> {
    let [x, y, z, pitch, yaw, roll] = pose;
    let (p, w) = (pitch.to_radians(), yaw.to_radians());
    // Unity's forward for a pitch and yaw (+X right, +Y up, +Z forward; a positive pitch looks down).
    let forward = [w.sin() * p.cos(), -p.sin(), w.cos() * p.cos()];
    let subject = [x + forward[0] * focus, y + forward[1] * focus, z + forward[2] * focus];
    let back = [x - subject[0], y - subject[1], z - subject[2]];
    (0..steps)
        .map(|i| {
            let t = if steps > 1 { i as f32 / (steps - 1) as f32 } else { 0.0 };
            let angle = std::f32::consts::TAU * t;
            let lift = RISE_M * (2.0 * std::f32::consts::TAU * t).sin();
            let (s, c) = angle.sin_cos();
            let offset = [back[0] * c + back[2] * s, back[1] + lift, -back[0] * s + back[2] * c];
            let at = [subject[0] + offset[0], subject[1] + offset[1], subject[2] + offset[2]];
            let look = [-offset[0], -offset[1], -offset[2]];
            let flat = (look[0] * look[0] + look[2] * look[2]).sqrt();
            let new_yaw = look[0].atan2(look[2]).to_degrees();
            let new_pitch = (-look[1]).atan2(flat).to_degrees();
            if i == 0 || i + 1 == steps {
                pose
            } else {
                [at[0], at[1], at[2], new_pitch, new_yaw, roll]
            }
        })
        .collect()
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
        let (pose, focus, smooth) = {
            let osc = self.osc_values.lock().unwrap();
            let pose = osc.latest.get(POSE).map(|(v, _)| floats(v)).filter(|v| v.len() == 6 && v.iter().any(|f| *f != 0.0));
            let focus = osc.latest.get(FOCAL_DISTANCE).and_then(|(v, _)| floats(v).first().copied()).unwrap_or(2.0);
            let smooth = osc.latest.get(SMOOTH).and_then(|(v, _)| v.first().map(OscValue::truthy));
            (pose, focus, smooth)
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
            let steps = (SWING.as_secs_f32() * RATE_HZ as f32) as usize + 1;
            let tick = Duration::from_secs_f32(1.0 / RATE_HZ as f32);
            let begin = Instant::now();
            for (i, p) in swing(pose, focus.clamp(0.5, 8.0), steps).into_iter().enumerate() {
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

    fn looks_at(p: [f32; 6], subject: [f32; 3]) -> f32 {
        let (pitch, yaw) = (p[3].to_radians(), p[4].to_radians());
        let f = [yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos()];
        let d = [subject[0] - p[0], subject[1] - p[1], subject[2] - p[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        (f[0] * d[0] + f[1] * d[1] + f[2] * d[2]) / len
    }

    #[test]
    fn the_camera_circles_its_subject_and_comes_back() {
        // At (1, 1.5, 0), looking along +Z, slightly down; the subject 2 m ahead.
        let pose = [1.0, 1.5, 0.0, 10.0, 0.0, 0.0];
        let subject = [1.0, 1.5 - 2.0 * 10f32.to_radians().sin(), 2.0 * 10f32.to_radians().cos()];
        let poses = swing(pose, 2.0, 61);
        assert_eq!(poses.first(), Some(&pose));
        assert_eq!(poses.last(), Some(&pose));
        for p in &poses {
            assert!(looks_at(*p, subject) > 0.9999, "{p:?} looks away");
        }
        // Half way it is across the circle, about 4 m from where it started, looking back; over the turn it faces every way.
        let horizontal = |p: [f32; 6]| ((p[0] - subject[0]).powi(2) + (p[2] - subject[2]).powi(2)).sqrt();
        let across = poses[30];
        assert!((horizontal(across) - horizontal(pose)).abs() < 1e-3, "keeps its distance");
        assert!(((across[0] - pose[0]).powi(2) + (across[2] - pose[2]).powi(2)).sqrt() > 3.8, "{across:?}");
        let mut headings: Vec<i32> = poses.iter().map(|p| (p[4].rem_euclid(360.0) / 30.0) as i32).collect();
        headings.sort();
        headings.dedup();
        assert_eq!(headings.len(), 12, "looks every way: {headings:?}");
        let (low, high) = poses.iter().fold((f32::MAX, f32::MIN), |(l, h), p| (l.min(p[1]), h.max(p[1])));
        assert!(high > pose[1] + 0.4 && low < pose[1] - 0.4, "rises and dips: {low} {high}");
    }
}
