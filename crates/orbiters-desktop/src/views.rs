//! The cameras a session recorded at its keyframes, and where a view of its splats starts: the 3D view
//! (pages/splats.rs) and the web preview shared on the Map explorer (controller/maps.rs) open on the same camera,
//! orbiting what it looks at.

use orbiters_core::contracts::frame_record::record_flags;
use orbiters_core::{bundle, reconstruction};
use orbiters_reconstruct::camera::Camera;
use orbiters_reconstruct::gaussians::Gaussians;
use std::path::Path;

/// A recorded keyframe's camera (Unity's world): pose and vertical field of view.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub fov_y: f32,
}

/// Every keyframe's camera, and which one to start from: the keyframe the reconstruction's preview shows, else the
/// first.
pub fn recorded(dir: &Path) -> anyhow::Result<(Vec<View>, usize)> {
    let report = reconstruction::read_report(dir);
    let records = bundle::read_records(dir)?;
    let keyframes: Vec<_> = records.iter().filter(|r| r.has(record_flags::KEYFRAME) && r.has(record_flags::STRIP_OK) && r.keyframe_index >= 0).collect();
    let views: Vec<View> = keyframes
        .iter()
        .map(|r| {
            let cam = Camera::from_strip(&r.camera_to_world, &r.projection, r.width.max(1), r.height.max(1), 1.0);
            View { position: cam.position(), forward: cam.forward(), fov_y: cam.fov_y() }
        })
        .collect();
    anyhow::ensure!(!views.is_empty(), "this session has no keyframe to look from");
    let start = report.as_ref().and_then(|r| r.held_out_keyframes.first()).and_then(|k| keyframes.iter().position(|r| r.keyframe_index == *k)).unwrap_or(0);
    Ok((views, start))
}

/// How far ahead of `v` its subject is: the median distance of the splats in front of it (3 m when none are).
pub fn depth_ahead(g: &Gaussians, v: &View) -> f32 {
    let mut ahead: Vec<f32> = g
        .means
        .iter()
        .map(|m| (m[0] - v.position[0]) * v.forward[0] + (m[1] - v.position[1]) * v.forward[1] + (m[2] - v.position[2]) * v.forward[2])
        .filter(|d| *d > 0.1 && *d < 100.0)
        .collect();
    if ahead.is_empty() {
        return 3.0;
    }
    let k = ahead.len() / 2;
    *ahead.select_nth_unstable_by(k, f32::total_cmp).1
}
