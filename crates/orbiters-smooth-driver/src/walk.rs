//! Fast walk's part of one device's pose, pure arithmetic. The walk is in SteamVR's raw tracking space, the one the
//! world-from-driver transform leads to: it goes on that translation, which also leaves the device's own position, as the
//! smoothing eased it, alone. The velocity goes with it, for SteamVR's prediction: every device, whatever its rate, is then
//! predicted on the same path, and a frame predicted ahead shows the walk ahead too. It is given in the driver's own space,
//! which that transform's rotation turns.

use crate::openvr::{DriverPose, Quaternion};
use orbiters_smooth::WalkNow;

/// `v` turned by the inverse of `q`.
fn turned_back(q: Quaternion, v: [f64; 3]) -> [f64; 3] {
    let n = (q.w * q.w + q.x * q.x + q.y * q.y + q.z * q.z).sqrt();
    if !(n > 1e-9) {
        return v;
    }
    // The conjugate, unit length.
    let (w, x, y, z) = (q.w / n, -q.x / n, -q.y / n, -q.z / n);
    // v + 2 w (u × v) + 2 u × (u × v)
    let c = [y * v[2] - z * v[1], z * v[0] - x * v[2], x * v[1] - y * v[0]];
    let d = [y * c[2] - z * c[1], z * c[0] - x * c[2], x * c[1] - y * c[0]];
    [v[0] + 2.0 * (w * c[0] + d[0]), v[1] + 2.0 * (w * c[1] + d[1]), v[2] + 2.0 * (w * c[2] + d[2])]
}

/// `pose` moved by `walk`.
pub fn apply(pose: &mut DriverPose, walk: &WalkNow) {
    for (t, o) in pose.vec_world_from_driver_translation.iter_mut().zip(walk.offset) {
        *t += o as f64;
    }
    let velocity = turned_back(pose.q_world_from_driver_rotation, walk.velocity.map(|v| v as f64));
    for (v, w) in pose.vec_velocity.iter_mut().zip(velocity) {
        *v += w;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn about_y(angle: f64) -> Quaternion {
        Quaternion { w: (angle / 2.0).cos(), x: 0.0, y: (angle / 2.0).sin(), z: 0.0 }
    }

    /// `q` turning `v`, as SteamVR does for a pose's position and velocity.
    fn turned(q: Quaternion, v: [f64; 3]) -> [f64; 3] {
        turned_back(Quaternion { w: q.w, x: -q.x, y: -q.y, z: -q.z }, v)
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    /// Where SteamVR puts a device, and how fast it goes, whatever its driver's own space: moved by the walk exactly.
    #[test]
    fn a_device_moves_by_the_walk_in_any_driver_space() {
        for (angle, translation) in [(0.0, [0.0; 3]), (1.1, [2.0, 0.5, -3.0]), (-2.8, [-7.0, 1.0, 0.0])] {
            let mut pose = DriverPose::at([0.3, 1.5, -0.2], about_y(0.4));
            pose.q_world_from_driver_rotation = about_y(angle);
            pose.vec_world_from_driver_translation = translation;
            pose.vec_velocity = [0.5, 0.0, 0.25];
            let world = |p: &DriverPose| {
                let r = turned(p.q_world_from_driver_rotation, p.vec_position);
                [r[0] + p.vec_world_from_driver_translation[0], r[1] + p.vec_world_from_driver_translation[1], r[2] + p.vec_world_from_driver_translation[2]]
            };
            let speed = |p: &DriverPose| turned(p.q_world_from_driver_rotation, p.vec_velocity);
            let (before, moving) = (world(&pose), speed(&pose));
            let (position, rotation) = (pose.vec_position, pose.q_rotation);
            apply(&mut pose, &WalkNow { offset: [10.0, 0.0, -4.0], velocity: [8.0, 0.0, 1.0] });
            assert!(close(world(&pose), [before[0] + 10.0, before[1], before[2] - 4.0]), "angle {angle}");
            assert!(close(speed(&pose), [moving[0] + 8.0, moving[1], moving[2] + 1.0]), "angle {angle}");
            assert_eq!((pose.vec_position, pose.q_rotation), (position, rotation), "the device's own pose stays as it is");
        }
    }

    /// The filter checks the driver's space for jumps: the walk, added after it, never reads as one.
    #[test]
    fn smoothing_never_sees_the_walk() {
        use crate::filter::DeviceFilter;
        let (mut with, mut without) = (DeviceFilter::new(), DeviceFilter::new());
        let mut walked = 0.0;
        for i in 0..400 {
            let t = i as f64 / 90.0;
            let device = [t.sin() * 0.2, 1.5, t.cos() * 0.2];
            let mut a = DriverPose::at(device, about_y(t));
            let mut b = a;
            assert!(with.apply(&mut a, 0.5, t) && without.apply(&mut b, 0.5, t));
            walked += 8.0 / 90.0;
            apply(&mut a, &WalkNow { offset: [walked as f32, 0.0, 0.0], velocity: [0.0; 3] });
            assert_eq!(a.vec_position, b.vec_position, "frame {i}: eased the same, with the walk or without");
            assert!((a.vec_world_from_driver_translation[0] - walked).abs() < 1e-3);
        }
    }
}
