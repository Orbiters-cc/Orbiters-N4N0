//! Keyframe selection from exact strip poses: a frame becomes a keyframe when the camera moved or turned
//! enough since the last keyframe, when it zoomed in or out, or when too much time passed. Turning enough is a share of
//! the view: [`KeyframeSettings::min_rotation_deg`] at VRChat's default 45° field of view, a third of a narrower one
//! (`fov / 3`): a 3D picture starts with VRChat's camera zoomed in to 20° and turned from one part of its view to the
//! next (the desktop's controller/burst.rs), 8 to 23° apart, and every one of those detail views must be kept.

use crate::settings::KeyframeSettings;

#[derive(Clone, Debug)]
pub struct KeyframeSelector {
    settings: KeyframeSettings,
    /// Select every frame (lossless storage).
    every_frame: bool,
    /// The last keyframe's pose, game time, and projection P11 (NaN: not known).
    last: Option<([f32; 12], f32, f32)>,
}

/// A zoom this much in or out (the ratio of the projection's P11, the focal length) since the last keyframe makes a
/// keyframe: the same place seen at another resolution.
pub const MIN_ZOOM: f32 = 1.2;

/// The vertical field of view in degrees of a projection's P11 (`1 / tan(fov / 2)`): NaN when it is not a usable one.
pub fn fov_deg(p11: f32) -> f32 {
    if p11.is_finite() && p11 > 0.0 {
        2.0 * (1.0 / p11).atan().to_degrees()
    } else {
        f32::NAN
    }
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

    /// Decides for a frame with a valid pose whose projection is not known; `time` is game time in seconds.
    pub fn consider(&mut self, pose: &[f32; 12], time: f32) -> bool {
        self.consider_view(pose, f32::NAN, time)
    }

    /// Decides for a frame with a valid pose and its projection's P11 (`1 / tan(fov / 2)`, the strip's word 16).
    pub fn consider_view(&mut self, pose: &[f32; 12], p11: f32, time: f32) -> bool {
        let take = self.every_frame
            || match &self.last {
                None => true,
                Some((p, t, last_p11)) => {
                    // A third of the view's height, at most the setting (VRChat's default 45° gives 15°).
                    let fov = fov_deg(p11);
                    let turn = if fov.is_finite() { self.settings.min_rotation_deg.min(fov / 3.0) } else { self.settings.min_rotation_deg };
                    let zoom = p11 / last_p11;
                    translation(pose, p) >= self.settings.min_translation
                        || rotation_angle_deg(pose, p) >= turn
                        || (zoom.is_finite() && (zoom >= MIN_ZOOM || zoom <= 1.0 / MIN_ZOOM))
                        || (time - t).abs() >= self.settings.max_interval_s
                }
            };
        if take {
            self.last = Some((*pose, time, p11));
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

    /// P11 of a vertical field of view in degrees.
    fn p11(fov: f32) -> f32 {
        1.0 / (fov.to_radians() / 2.0).tan()
    }

    #[test]
    fn zoomed_in_views_are_kept_as_detail() {
        let mut k = KeyframeSelector::new(KeyframeSettings::default(), false);
        // At VRChat's default 45°, as before: 15° to turn.
        assert!(k.consider_view(&pose(0.0, 0.0), p11(45.0), 0.0));
        assert!(!k.consider_view(&pose(0.0, 14.0), p11(45.0), 0.03));
        assert!(k.consider_view(&pose(0.0, 15.5), p11(45.0), 0.06));
        // Zoomed in to 20° where it stands: the same pose at more than twice the resolution is a keyframe.
        assert!(k.consider_view(&pose(0.0, 15.5), p11(20.0), 0.1));
        assert!(!k.consider_view(&pose(0.0, 15.5), p11(20.0), 0.13), "nothing new");
        // Turned by a third of the 20° view: a keyframe; less is not.
        assert!(!k.consider_view(&pose(0.0, 21.5), p11(20.0), 0.16));
        assert!(k.consider_view(&pose(0.0, 22.5), p11(20.0), 0.2));
        // Zoomed back out (and a small zoom change is nothing).
        assert!(!k.consider_view(&pose(0.0, 22.5), p11(22.0), 0.23));
        assert!(k.consider_view(&pose(0.0, 22.5), p11(45.0), 0.26));
        // A wider view than the default keeps the setting's 15°.
        assert!(k.consider_view(&pose(0.0, 0.0), p11(90.0), 0.3));
        assert!(!k.consider_view(&pose(0.0, 14.0), p11(90.0), 0.33));
        // No projection (or a broken one): the setting's 15°, no zoom rule.
        assert!(!k.consider_view(&pose(0.0, 0.0), f32::NAN, 0.36));
        assert!((fov_deg(p11(47.3)) - 47.3).abs() < 1e-3 && fov_deg(0.0).is_nan() && fov_deg(f32::INFINITY).is_nan());
    }
}
