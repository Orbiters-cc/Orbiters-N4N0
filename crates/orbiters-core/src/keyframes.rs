//! Keyframe selection from exact strip poses: a frame becomes a keyframe when the camera moved or turned
//! enough since the last keyframe, or when too much time passed.

use crate::settings::KeyframeSettings;

#[derive(Clone, Debug)]
pub struct KeyframeSelector {
    settings: KeyframeSettings,
    /// Select every frame (lossless storage).
    every_frame: bool,
    last: Option<([f32; 12], f32)>,
}

/// Angle in degrees between two camera rotations (rows of row-major 3×4 camera-to-world matrices).
pub fn rotation_angle_deg(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    // trace(Ra^T Rb) = sum over elements of Ra ∘ Rb.
    let mut trace = 0.0f32;
    for r in 0..3 {
        for c in 0..3 {
            trace += a[r * 4 + c] * b[r * 4 + c];
        }
    }
    ((trace - 1.0) * 0.5).clamp(-1.0, 1.0).acos().to_degrees()
}

pub fn translation(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let d = [a[3] - b[3], a[7] - b[7], a[11] - b[11]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

impl KeyframeSelector {
    pub fn new(settings: KeyframeSettings, every_frame: bool) -> Self {
        Self { settings, every_frame, last: None }
    }

    /// Decides for a frame with a valid pose; `time` is game time in seconds.
    pub fn consider(&mut self, pose: &[f32; 12], time: f32) -> bool {
        let take = self.every_frame
            || match &self.last {
                None => true,
                Some((p, t)) => {
                    translation(pose, p) >= self.settings.min_translation
                        || rotation_angle_deg(pose, p) >= self.settings.min_rotation_deg
                        || (time - t).abs() >= self.settings.max_interval_s
                }
            };
        if take {
            self.last = Some((*pose, time));
        }
        take
    }

    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(x: f32, yaw_deg: f32) -> [f32; 12] {
        let (s, c) = yaw_deg.to_radians().sin_cos();
        [c, 0.0, s, x, 0.0, 1.0, 0.0, 1.6, -s, 0.0, c, 0.0]
    }

    #[test]
    fn selection() {
        let mut k = KeyframeSelector::new(KeyframeSettings::default(), false);
        assert!(k.consider(&pose(0.0, 0.0), 0.0));
        assert!(!k.consider(&pose(0.1, 5.0), 0.1));
        assert!(k.consider(&pose(0.35, 0.0), 0.2)); // moved 0.35 m
        assert!(!k.consider(&pose(0.4, 10.0), 0.3));
        assert!(k.consider(&pose(0.4, 20.0), 0.4)); // turned 20 degrees
        assert!(k.consider(&pose(0.4, 20.0), 2.5)); // time
        assert!((rotation_angle_deg(&pose(0.0, 0.0), &pose(0.0, 90.0)) - 90.0).abs() < 1e-3);
    }
}
