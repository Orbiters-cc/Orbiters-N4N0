//! Body poses from tracked devices (play space, Unity conventions): the T-pose that calibrates them, shared by the
//! capture worker (a T-pose held while recording) and the VR overlay (calibrating on request).

use crate::contracts::performance::calibration::{Calibration, DevicePose};
use crate::contracts::performance::tracking::{Device, Sample};

/// Fires once when the person holds a T-pose still: hands level with the shoulders and far apart, head between
/// them, nothing moving for half a second. Re-arms after the pose was left for a second.
#[derive(Default)]
pub struct TPoseDetector {
    held_since: Option<u64>,
    left_since: Option<u64>,
    fired: bool,
}

/// Which parts of a T-pose are there (feedback while calibrating).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TPoseCheck {
    /// Hands at the same height.
    pub level: bool,
    /// Hands at shoulder height.
    pub shoulders: bool,
    /// Arms stretched out (hands far apart for the person's height).
    pub wide: bool,
    /// Head between the hands.
    pub centered: bool,
    /// Nothing moving.
    pub still: bool,
}

impl TPoseCheck {
    pub fn of(head: &Sample, left: &Sample, right: &Sample) -> Self {
        let speed = |s: &Sample| (s.velocity[0].powi(2) + s.velocity[1].powi(2) + s.velocity[2].powi(2)).sqrt();
        let eye = head.position[1];
        let (l, r) = (left.position, right.position);
        let hands_y = (l[1] + r[1]) / 2.0;
        let span = ((l[0] - r[0]).powi(2) + (l[2] - r[2]).powi(2)).sqrt();
        let mid = [(l[0] + r[0]) / 2.0, (l[2] + r[2]) / 2.0];
        let off_center = ((head.position[0] - mid[0]).powi(2) + (head.position[2] - mid[1]).powi(2)).sqrt();
        Self {
            level: (l[1] - r[1]).abs() < 0.15,
            shoulders: hands_y > eye - 0.55 && hands_y < eye + 0.05,
            wide: span >= (0.75 * eye).max(0.9),
            centered: off_center < 0.25,
            still: [head, left, right].iter().all(|s| s.pose_valid() && speed(s) < 0.2),
        }
    }

    pub fn all(&self) -> bool {
        self.level && self.shoulders && self.wide && self.centered && self.still
    }
}

impl TPoseDetector {
    pub fn is_t_pose(head: &Sample, left: &Sample, right: &Sample) -> bool {
        TPoseCheck::of(head, left, right).all()
    }

    /// `true` at the moment a held T-pose counts.
    pub fn update(&mut self, time_ns: u64, in_pose: bool) -> bool {
        if in_pose {
            self.left_since = None;
            let since = *self.held_since.get_or_insert(time_ns);
            if !self.fired && time_ns - since >= 500_000_000 {
                self.fired = true;
                return true;
            }
        } else {
            self.held_since = None;
            let since = *self.left_since.get_or_insert(time_ns);
            if time_ns - since >= 1_000_000_000 {
                self.fired = false;
            }
        }
        false
    }
}

/// The sample of the device with `role` (`head`, `left_hand`, ...), devices numbered as in `devices`.
pub fn by_role<'a>(role: &str, devices: &[Device], current: &'a [Sample]) -> Option<&'a Sample> {
    let number = devices.iter().position(|d| d.role == role)?;
    current.iter().find(|s| s.device as usize == number)
}

/// Every device's pose at a calibration.
pub fn calibration_of(time_ns: i64, source: &str, devices: &[Device], current: &[Sample]) -> Calibration {
    let pose_of = |role: &str| by_role(role, devices, current);
    let head_height_m = pose_of("head").map(|s| s.position[1]).unwrap_or(0.0);
    let arm_span_m = match (pose_of("left_hand"), pose_of("right_hand")) {
        (Some(l), Some(r)) => {
            ((l.position[0] - r.position[0]).powi(2) + (l.position[1] - r.position[1]).powi(2) + (l.position[2] - r.position[2]).powi(2)).sqrt()
        }
        _ => 0.0,
    };
    Calibration {
        time_ns,
        pose: "t_pose".into(),
        source: source.into(),
        head_height_m,
        arm_span_m,
        devices: current
            .iter()
            .filter_map(|s| {
                let d = devices.get(s.device as usize)?;
                Some(DevicePose { index: s.device, serial: d.serial.clone(), class: d.class, role: d.role.clone(), position: s.position, rotation: s.rotation })
            })
            .collect(),
    }
}

/// Numbers a calibration's devices like `devices` (by serial); a device the session did not see gets 255.
pub fn renumber(calibration: &mut Calibration, devices: &[Device]) {
    for d in &mut calibration.devices {
        d.index = devices.iter().position(|e| !d.serial.is_empty() && e.serial == d.serial).and_then(|n| u8::try_from(n).ok()).unwrap_or(u8::MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::performance::tracking::{flags, DeviceClass};

    fn at(device: u8, p: [f32; 3]) -> Sample {
        Sample { device, flags: flags::POSE_VALID, position: p, rotation: [0.0, 0.0, 0.0, 1.0], ..Default::default() }
    }

    #[test]
    fn t_pose_and_renumbering() {
        let (head, left, right) = (at(0, [0.0, 1.62, 0.0]), at(1, [-0.7, 1.4, 0.05]), at(2, [0.7, 1.38, 0.0]));
        assert!(TPoseDetector::is_t_pose(&head, &left, &right));
        assert!(!TPoseDetector::is_t_pose(&head, &at(1, [-0.25, 0.85, 0.0]), &at(2, [0.25, 0.85, 0.0])), "arms down");
        let mut moving = right;
        moving.velocity = [0.5, 0.0, 0.0];
        assert!(!TPoseDetector::is_t_pose(&head, &left, &moving), "moving");
        let mut d = TPoseDetector::default();
        assert!(!d.update(0, true) && d.update(500_000_000, true) && !d.update(900_000_000, true), "fires once per hold");
        assert!(!d.update(1_000_000_000, false) && !d.update(2_100_000_000, false) && !d.update(2_200_000_000, true) && d.update(2_700_000_000, true));

        let devices = vec![
            Device { serial: "H".into(), class: DeviceClass::Hmd, role: "head".into(), ..Default::default() },
            Device { serial: "L".into(), class: DeviceClass::Controller, role: "left_hand".into(), ..Default::default() },
            Device { serial: "R".into(), class: DeviceClass::Controller, role: "right_hand".into(), ..Default::default() },
        ];
        let mut c = calibration_of(-5, "overlay", &devices, &[head, left, right]);
        assert!((c.head_height_m - 1.62).abs() < 1e-6 && (c.arm_span_m - 1.4).abs() < 0.01);
        // The session saw only the right hand, first.
        renumber(&mut c, &[devices[2].clone()]);
        assert_eq!(c.devices.iter().map(|d| (d.serial.as_str(), d.index)).collect::<Vec<_>>(), [("H", 255), ("L", 255), ("R", 0)]);
    }
}
