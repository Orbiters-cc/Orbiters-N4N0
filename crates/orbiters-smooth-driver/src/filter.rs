//! The smoothing of one device's poses, pure arithmetic.
//!
//! A strength `s` in `0..=1` eases position and rotation toward each new pose with a time constant `s` × 120 ms
//! (exponential, from the time between poses, so the same at 90 and 1000 poses a second) and scales the
//! velocities and accelerations SteamVR predicts with by `1 − s`. The state starts again from the pose as it comes
//! when a device was invalid or disconnected, after a gap of more than half a second, or when its driver moves its
//! driver space (a jump in the world-from-driver transform: easing across it would fly the device through the room).

use crate::openvr::{DriverPose, Quaternion};

/// The time constant at strength 1, seconds.
pub const MAX_TIME_CONSTANT: f64 = orbiters_smooth::MAX_TIME_CONSTANT_MS as f64 / 1000.0;
/// Longer between two poses and the state starts again, seconds.
pub const RESET_GAP: f64 = 0.5;
/// A world-from-driver translation that moves more than this between two poses is a new driver space, metres.
const REBASE_DISTANCE: f64 = 0.01;
/// ... or a rotation of more than 1°: |dot| of unit quaternions below cos(0.5°).
const REBASE_MIN_DOT: f64 = 0.999_961_923_064_171;

/// One device's state.
#[derive(Clone, Copy, Debug)]
pub struct DeviceFilter {
    primed: bool,
    last_time: f64,
    position: [f64; 3],
    rotation: Quaternion,
    world_rotation: Quaternion,
    world_translation: [f64; 3],
}

impl Default for DeviceFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceFilter {
    pub const fn new() -> Self {
        Self {
            primed: false,
            last_time: 0.0,
            position: [0.0; 3],
            rotation: Quaternion { w: 1.0, x: 0.0, y: 0.0, z: 0.0 },
            world_rotation: Quaternion { w: 1.0, x: 0.0, y: 0.0, z: 0.0 },
            world_translation: [0.0; 3],
        }
    }

    /// Forgets the device: the next pose passes as it is (velocities still scaled) and starts the state.
    pub fn reset(&mut self) {
        self.primed = false;
    }

    /// Whether it has a state to ease from.
    pub fn primed(&self) -> bool {
        self.primed
    }

    /// Smooths `pose` in place for a strength (`0..=1`) and the time it arrived (seconds, any monotonic origin).
    /// Returns `false`, leaving `pose` as it was and forgetting the device, when the pose must pass untouched:
    /// strength 0, an invalid or disconnected pose, or numbers that are not finite.
    // The negated `>`/`<=` on floats are deliberate: they take the NaN side, which is what we want to leave alone.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    pub fn apply(&mut self, pose: &mut DriverPose, strength: f32, now: f64) -> bool {
        let s = strength as f64;
        // `s > 0.0` is false for 0, negatives and NaN alike, so this leaves all of them untouched.
        if !(s > 0.0) || pose.pose_is_valid == 0 || pose.device_is_connected == 0 {
            self.reset();
            return false;
        }
        let s = s.min(1.0);
        let Some(rotation_in) = normalized(pose.q_rotation) else {
            self.reset();
            return false;
        };
        let position_in = pose.vec_position;
        if !position_in.iter().all(|v| v.is_finite()) || !now.is_finite() {
            self.reset();
            return false;
        }

        let world_rotation = pose.q_world_from_driver_rotation;
        let world_translation = pose.vec_world_from_driver_translation;
        let rebased = self.primed
            && (distance(world_translation, self.world_translation) > REBASE_DISTANCE
                || dot(normalized(world_rotation).unwrap_or(Quaternion::IDENTITY), normalized(self.world_rotation).unwrap_or(Quaternion::IDENTITY)).abs()
                    < REBASE_MIN_DOT);
        let dt = now - self.last_time;
        // `!(dt <= RESET_GAP)` also starts again on a NaN.
        if !self.primed || rebased || !(dt <= RESET_GAP) {
            self.position = position_in;
            self.rotation = rotation_in;
        } else {
            let tau = MAX_TIME_CONSTANT * s;
            let alpha = 1.0 - (-dt.max(0.0) / tau).exp();
            for (p, &target) in self.position.iter_mut().zip(&position_in) {
                *p += alpha * (target - *p);
            }
            self.rotation = nlerp(self.rotation, rotation_in, alpha);
        }
        self.primed = true;
        self.last_time = now;
        self.world_rotation = world_rotation;
        self.world_translation = world_translation;

        pose.vec_position = self.position;
        // On the input's side of the double cover: the same rotation, and no sign flips for whoever reads it.
        pose.q_rotation = if dot(self.rotation, pose.q_rotation) < 0.0 { negate(self.rotation) } else { self.rotation };
        let keep = 1.0 - s;
        for v in [&mut pose.vec_velocity, &mut pose.vec_acceleration, &mut pose.vec_angular_velocity, &mut pose.vec_angular_acceleration] {
            for c in v.iter_mut() {
                *c *= keep;
            }
        }
        true
    }
}

fn dot(a: Quaternion, b: Quaternion) -> f64 {
    a.w * b.w + a.x * b.x + a.y * b.y + a.z * b.z
}

fn negate(q: Quaternion) -> Quaternion {
    Quaternion { w: -q.w, x: -q.x, y: -q.y, z: -q.z }
}

fn normalized(q: Quaternion) -> Option<Quaternion> {
    let n = dot(q, q).sqrt();
    (n.is_finite() && n > 1e-9).then(|| Quaternion { w: q.w / n, x: q.x / n, y: q.y / n, z: q.z / n })
}

/// From `a` toward `b` by `t`, on the shorter way (either sign of `b`), normalised.
fn nlerp(a: Quaternion, b: Quaternion, t: f64) -> Quaternion {
    let b = if dot(a, b) < 0.0 { negate(b) } else { b };
    let q = Quaternion { w: a.w + t * (b.w - a.w), x: a.x + t * (b.x - a.x), y: a.y + t * (b.y - a.y), z: a.z + t * (b.z - a.z) };
    normalized(q).unwrap_or(b)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic noise source (xorshift), uniform in `-1..1`.
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 52) as f64 - 1.0
        }
    }

    fn about_y(angle: f64) -> Quaternion {
        Quaternion { w: (angle / 2.0).cos(), x: 0.0, y: (angle / 2.0).sin(), z: 0.0 }
    }

    fn angle_between(a: Quaternion, b: Quaternion) -> f64 {
        2.0 * dot(a, b).abs().min(1.0).acos()
    }

    fn rms(values: &[f64]) -> f64 {
        (values.iter().map(|v| v * v).sum::<f64>() / values.len() as f64).sqrt()
    }

    /// Feeds a still device with ±1 mm and ±0.2° of jitter at `rate` poses a second; returns the RMS position and
    /// angle error before and after, over the last 3 seconds of 4.
    fn jitter(strength: f32, rate: f64) -> ((f64, f64), (f64, f64)) {
        let mut filter = DeviceFilter::new();
        let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
        let (mut raw_p, mut raw_r, mut out_p, mut out_r) = (vec![], vec![], vec![], vec![]);
        let n = (4.0 * rate) as usize;
        for i in 0..n {
            let t = i as f64 / rate;
            let offset = 0.001 * noise.next();
            let tilt = 0.2f64.to_radians() * noise.next();
            let mut pose = DriverPose::at([1.0 + offset, 1.5, -0.5], about_y(tilt));
            assert!(filter.apply(&mut pose, strength, t));
            if t >= 1.0 {
                raw_p.push(offset);
                raw_r.push(tilt);
                out_p.push(pose.vec_position[0] - 1.0);
                out_r.push(angle_between(pose.q_rotation, Quaternion::IDENTITY));
            }
        }
        ((rms(&raw_p), rms(&raw_r)), (rms(&out_p), rms(&out_r)))
    }

    #[test]
    fn jitter_is_reduced() {
        for rate in [90.0, 250.0, 1000.0] {
            let ((raw_p, raw_r), (out_p, out_r)) = jitter(1.0, rate);
            assert!(out_p < raw_p / 3.0, "{rate} Hz: position {out_p} vs {raw_p}");
            assert!(out_r < raw_r / 3.0, "{rate} Hz: rotation {out_r} vs {raw_r}");
            let ((_, _), (half_p, _)) = jitter(0.25, rate);
            assert!(half_p > out_p && half_p < raw_p, "{rate} Hz: a weaker strength smooths less ({half_p})");
        }
    }

    /// Moving at a steady 1 m/s and 90°/s, the eased pose follows at a steady lag of about one time constant,
    /// whatever the pose rate.
    #[test]
    fn steady_motion_is_followed_at_the_same_lag_at_any_rate() {
        for rate in [90.0, 1000.0] {
            for strength in [0.5f32, 1.0] {
                let tau = MAX_TIME_CONSTANT * strength as f64;
                let mut filter = DeviceFilter::new();
                let mut last = DriverPose::at([0.0; 3], Quaternion::IDENTITY);
                let n = (2.0 * rate) as usize;
                for i in 0..=n {
                    let t = i as f64 / rate;
                    let mut pose = DriverPose::at([t, 0.0, 0.0], about_y(t * std::f64::consts::FRAC_PI_2));
                    pose.vec_velocity = [1.0, 0.0, 0.0];
                    pose.vec_angular_velocity = [0.0, std::f64::consts::FRAC_PI_2, 0.0];
                    assert!(filter.apply(&mut pose, strength, t));
                    last = pose;
                }
                let t = n as f64 / rate;
                // The discrete lag of an exponential filter fed at interval h: h / (1 − e^(−h/τ)) − h, → τ.
                let h = 1.0 / rate;
                let expected = h / (1.0 - (-h / tau).exp()) - h;
                let lag = t - last.vec_position[0];
                assert!((lag - expected).abs() < 0.002, "{rate} Hz, s {strength}: lag {lag}, expected {expected}");
                assert!((lag - tau).abs() < 0.6 * h + 0.002, "{rate} Hz, s {strength}: lag {lag} vs τ {tau}");
                let angle_lag = angle_between(last.q_rotation, about_y(t * std::f64::consts::FRAC_PI_2));
                assert!((angle_lag - expected * std::f64::consts::FRAC_PI_2).abs() < 0.01, "{rate} Hz: angle lag {angle_lag}");
                assert_eq!(last.vec_velocity[0], 1.0 * (1.0 - strength as f64));
                assert_eq!(last.vec_angular_velocity[1], std::f64::consts::FRAC_PI_2 * (1.0 - strength as f64));
            }
        }
    }

    /// What a slider says in milliseconds is what a device in steady motion trails by, at 72 poses a second as at 1000.
    #[test]
    fn a_slider_s_milliseconds_are_the_lag_at_any_rate() {
        for ms in [20.0f64, 35.0, 78.0, 120.0] {
            let strength = (ms / orbiters_smooth::MAX_TIME_CONSTANT_MS as f64) as f32;
            for rate in [72.0, 90.0, 120.0, 1000.0] {
                let mut filter = DeviceFilter::new();
                let mut last = DriverPose::at([0.0; 3], Quaternion::IDENTITY);
                let n = (2.0 * rate) as usize;
                for i in 0..=n {
                    let mut pose = DriverPose::at([i as f64 / rate, 0.0, 0.0], Quaternion::IDENTITY);
                    assert!(filter.apply(&mut pose, strength, i as f64 / rate));
                    last = pose;
                }
                let lag_ms = (n as f64 / rate - last.vec_position[0]) * 1000.0;
                // The discrete filter sits within half a pose of the time constant.
                assert!((lag_ms - ms).abs() < 0.6 * 1000.0 / rate + 2.0, "{ms} ms at {rate} Hz: lags {lag_ms} ms");
            }
        }
    }

    #[test]
    fn a_device_that_stops_converges_to_where_it_is() {
        let mut filter = DeviceFilter::new();
        let mut pose = DriverPose::at([0.0; 3], Quaternion::IDENTITY);
        filter.apply(&mut pose, 1.0, 0.0);
        let target = [0.3, -0.2, 0.1];
        let mut t = 0.0;
        let mut out = pose;
        while t < 10.0 * MAX_TIME_CONSTANT {
            t += 0.002;
            out = DriverPose::at(target, about_y(1.0));
            assert!(filter.apply(&mut out, 1.0, t));
        }
        for (o, x) in out.vec_position.iter().zip(&target) {
            assert!((o - x).abs() < 1e-4 * 0.3 + 1e-6, "{:?}", out.vec_position);
        }
        assert!(angle_between(out.q_rotation, about_y(1.0)) < 1e-4);
    }

    #[test]
    fn strength_zero_and_bad_poses_pass_untouched() {
        let mut filter = DeviceFilter::new();
        let mut pose = DriverPose::at([1.0, 2.0, 3.0], about_y(0.3));
        pose.vec_velocity = [1.0, 2.0, 3.0];
        let original = pose;
        assert!(!filter.apply(&mut pose, 0.0, 0.0));
        assert_eq!(pose, original);
        assert!(!filter.apply(&mut pose, f32::NAN, 0.0));
        assert!(!filter.apply(&mut pose, -1.0, 0.0));
        assert_eq!(pose, original);

        for broken in [
            DriverPose { pose_is_valid: 0, ..original },
            DriverPose { device_is_connected: 0, ..original },
            DriverPose { q_rotation: Quaternion { w: 0.0, x: 0.0, y: 0.0, z: 0.0 }, ..original },
            DriverPose { vec_position: [f64::NAN, 0.0, 0.0], ..original },
        ] {
            let mut p = broken;
            assert!(!filter.apply(&mut p, 1.0, 0.0));
            assert_eq!(format!("{p:?}"), format!("{broken:?}"));
            assert!(!filter.primed());
        }
    }

    /// After an invalid pose, a disconnect, a long gap or a new driver space, the next pose comes as it is.
    #[test]
    fn resets_start_from_the_pose() {
        let start = |filter: &mut DeviceFilter| {
            filter.reset();
            let mut p = DriverPose::at([0.0; 3], Quaternion::IDENTITY);
            assert!(filter.apply(&mut p, 1.0, 10.0));
            let mut p = DriverPose::at([0.0; 3], Quaternion::IDENTITY);
            assert!(filter.apply(&mut p, 1.0, 10.01));
        };
        let far = DriverPose::at([1.0, 0.0, 0.0], about_y(1.0));
        let mut filter = DeviceFilter::new();

        // Eased: a jump of a metre within 10 ms moves the output only a little.
        start(&mut filter);
        let mut p = far;
        filter.apply(&mut p, 1.0, 10.02);
        assert!(p.vec_position[0] > 0.0 && p.vec_position[0] < 0.2, "{:?}", p.vec_position);

        start(&mut filter);
        let mut invalid = DriverPose { pose_is_valid: 0, ..far };
        assert!(!filter.apply(&mut invalid, 1.0, 10.02));
        let mut p = far;
        filter.apply(&mut p, 1.0, 10.03);
        assert_eq!(p.vec_position, far.vec_position, "after an invalid pose");

        start(&mut filter);
        let mut gone = DriverPose { device_is_connected: 0, ..far };
        assert!(!filter.apply(&mut gone, 1.0, 10.02));
        let mut p = far;
        filter.apply(&mut p, 1.0, 10.03);
        assert_eq!(p.vec_position, far.vec_position, "after a disconnect");

        start(&mut filter);
        let mut p = far;
        filter.apply(&mut p, 1.0, 10.01 + RESET_GAP + 0.01);
        assert_eq!(p.vec_position, far.vec_position, "after a gap");

        start(&mut filter);
        let mut p = DriverPose { vec_world_from_driver_translation: [0.0, 0.0, 0.5], ..far };
        filter.apply(&mut p, 1.0, 10.02);
        assert_eq!(p.vec_position, far.vec_position, "after the driver moved its space");

        start(&mut filter);
        let mut p = DriverPose { q_world_from_driver_rotation: about_y(0.1), ..far };
        filter.apply(&mut p, 1.0, 10.02);
        assert_eq!(p.vec_position, far.vec_position, "after the driver turned its space");

        // A slowly drifting driver space (continuous calibration) is not a new one.
        start(&mut filter);
        let mut p = DriverPose { vec_world_from_driver_translation: [0.0, 0.0, 0.001], ..far };
        filter.apply(&mut p, 1.0, 10.02);
        assert!(p.vec_position[0] < 0.2);
    }

    /// The same orientation sent as q and −q (both are valid) eases nowhere, and comes out on the input's side.
    #[test]
    fn quaternion_sign_flips_are_the_same_rotation() {
        let mut filter = DeviceFilter::new();
        let q = about_y(0.7);
        for i in 0..200 {
            let mut p = DriverPose::at([0.0; 3], if i % 2 == 0 { q } else { negate(q) });
            assert!(filter.apply(&mut p, 1.0, i as f64 * 0.001));
            assert!(angle_between(p.q_rotation, q) < 1e-9, "step {i}");
            assert!(dot(p.q_rotation, if i % 2 == 0 { q } else { negate(q) }) > 0.0, "step {i}: on the input's side");
        }
    }

    #[test]
    fn time_going_backwards_holds_still() {
        let mut filter = DeviceFilter::new();
        let mut p = DriverPose::at([0.0; 3], Quaternion::IDENTITY);
        filter.apply(&mut p, 1.0, 5.0);
        let mut p = DriverPose::at([1.0, 0.0, 0.0], Quaternion::IDENTITY);
        assert!(filter.apply(&mut p, 1.0, 4.0));
        assert_eq!(p.vec_position, [0.0; 3]);
    }
}
