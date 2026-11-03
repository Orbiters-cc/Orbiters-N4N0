//! The cameras a session recorded at its keyframes, and where a view of its splats starts: the 3D view
//! (pages/splats.rs) and the web preview shared on the Map explorer (controller/maps.rs) open looking the way the same
//! camera looked, from its place but at the eye height of the person who captured over the ground they stood on
//! (orbiters_reconstruct::ground), orbiting what it looks at. A 3D picture with close-ups (controller/burst.rs: VRChat's
//! camera zoomed in on its framing before it flies) opens exactly as the camera framed it, where its people are
//! sharpest.

use orbiters_core::contracts::frame_record::record_flags;
use orbiters_core::{bundle, reconstruction};
use orbiters_reconstruct::camera::Camera;
use orbiters_reconstruct::gaussians::Gaussians;
use orbiters_reconstruct::ground::{self, Ground};
use std::path::Path;

/// A recorded keyframe's camera (Unity's world): pose and vertical field of view.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub fov_y: f32,
    /// When it was recorded (the session's clock), for a recorded keyframe.
    pub time_ns: Option<u64>,
    /// A 3D picture's own framing (the camera where it stood for its close-ups): a view opens exactly there, not at
    /// the eye height of the person who took it ([`eye_level`]).
    pub framed: bool,
}

/// Every keyframe's camera, and which one to start from: a 3D picture's framing when it took close-ups ([`framing`]),
/// else the keyframe the reconstruction's preview shows, else the first.
pub fn recorded(dir: &Path) -> anyhow::Result<(Vec<View>, usize)> {
    let report = reconstruction::read_report(dir);
    let records = bundle::read_records(dir)?;
    let keyframes: Vec<_> = records.iter().filter(|r| r.has(record_flags::KEYFRAME) && r.has(record_flags::STRIP_OK) && r.keyframe_index >= 0).collect();
    let cameras: Vec<Camera> = keyframes.iter().map(|r| Camera::from_strip(&r.camera_to_world, &r.projection, r.width.max(1), r.height.max(1), 1.0)).collect();
    let mut views: Vec<View> = cameras
        .iter()
        .zip(&keyframes)
        .map(|(cam, r)| View { position: cam.position(), forward: cam.forward(), fov_y: usable_fov(cam.fov_y()), time_ns: Some(r.capture_time_ns), framed: false })
        .collect();
    anyhow::ensure!(!views.is_empty(), "this session has no keyframe to look from");
    // Focal lengths of the frames only, of their most common size: a rig face's 90° square is no zoom (in a 3D
    // picture the faces outnumber the frames six to one), nor is a frame of another size.
    use orbiters_core::contracts::strip::flags;
    let face = |r: &&orbiters_core::contracts::frame_record::FrameRecord| flags::kind(r.strip_flags) == flags::KIND_RIG && flags::face(r.strip_flags) != 0;
    let sizes: Vec<Option<(u32, u32)>> = keyframes.iter().map(|r| (!face(r)).then_some((r.width, r.height))).collect();
    let main = sizes.iter().flatten().copied().max_by_key(|s| sizes.iter().filter(|t| **t == Some(*s)).count());
    let focals: Vec<f32> = cameras.iter().zip(&sizes).map(|(c, s)| if s.is_some() && *s == main { c.fy / c.height as f32 } else { f32::NAN }).collect();
    if let Some(i) = framing(&focals) {
        views[i].framed = true;
        return Ok((views, i));
    }
    let start = report.as_ref().and_then(|r| r.held_out_keyframes.first()).and_then(|k| keyframes.iter().position(|r| r.keyframe_index == *k)).unwrap_or(0);
    Ok((views, start))
}

/// Of keyframes with these focal lengths (in record order; NaN for those that are not frames of the session's size),
/// a 3D picture's framing when it took close-ups (orbiters_reconstruct::dataset::detail_views: the camera zoomed in where
/// it stood before it flew): the last keyframe before the first close-up that is not one, the picture as the camera
/// framed it (the close-ups that follow look over the same view at more than twice its resolution).
pub fn framing(focals: &[f32]) -> Option<usize> {
    let detail = orbiters_reconstruct::dataset::detail_views(focals);
    let first = *detail.first()?;
    (0..first).rev().find(|i| focals[*i].is_finite() && detail.binary_search(i).is_err())
}

/// Where a view of `dir`'s splats `g` starts, from its recorded view `v`: looking the same way from the same place, at
/// the eye height of the person who captured over the ground they stood on then.
pub fn eye_level(dir: &Path, g: &Gaussians, v: &View) -> (View, Ground) {
    let ground = ground::find(dir, g, v.position, v.time_ns);
    // A picture's framing stays where its camera was.
    let position = if v.framed { v.position } else { ground.eye(v.position) };
    (View { position, ..*v }, ground)
}

/// A recorded vertical field of view as a view can use it: 10° to 150° (a degenerate projection in a record gave
/// none, or one past 180°, and the 3D view's pan and fly speeds came out huge or backwards), 60° when it is not a
/// number.
pub fn usable_fov(fov_y: f32) -> f32 {
    if fov_y.is_finite() {
        fov_y.clamp(10f32.to_radians(), 150f32.to_radians())
    } else {
        60f32.to_radians()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_3d_picture_with_close_ups_opens_as_its_camera_framed_it() {
        // Focal length over the image height at 47.3 degrees and at VRChat's narrowest, 20.
        let (wide, zoomed) = (1.143, 2.836);
        // The start, nine close-ups, the start zoomed back out, then the flight (a rig face is no frame of the size).
        let mut picture = vec![wide];
        picture.extend([zoomed; 9]);
        picture.extend([wide; 30]);
        picture.insert(20, f32::NAN);
        assert_eq!(framing(&picture), Some(0));
        // The start keyframe lost: nothing before the close-ups, so no framing to open.
        assert_eq!(framing(&picture[1..]), None);
        // A recording without close-ups, or one zoomed in all along: none (the preview's keyframe, as before).
        assert_eq!(framing(&[wide; 40]), None);
        assert_eq!(framing(&[zoomed; 40]), None);
        // A slight zoom is no close-up.
        let mut slight = vec![wide; 40];
        slight[5] = wide * 1.2;
        assert_eq!(framing(&slight), None);
    }

    /// A 3D picture's frames.bin as the capture writes it: the start, nine close-ups (20°), the start zoomed back out,
    /// then a flight whose every other frame is a rig frame with six 90° faces (keyframes of their own, 420 px square,
    /// outnumbering the frames). It opens at the start, exactly where the camera stood; without close-ups, at the first
    /// held-out keyframe as before.
    #[test]
    fn a_session_with_close_ups_opens_at_its_start() {
        use orbiters_core::contracts::frame_record::{FrameRecord, RECORD_SIZE};
        use orbiters_core::contracts::strip::flags;
        let p11 = |fov: f32| 1.0 / (fov.to_radians() / 2.0).tan();
        let frame = |i: usize, x: f32, fov: f32, key: i32| FrameRecord {
            flags: record_flags::STRIP_OK | record_flags::KEYFRAME,
            frame_index: i as u64,
            camera_to_world: [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 1.2, 0.0, 0.0, 1.0, 0.0],
            projection: [p11(fov) * 9.0 / 16.0, p11(fov), 0.0, 0.0],
            width: 1920,
            height: 1080,
            keyframe_index: key,
            ..Default::default()
        };
        let write = |dir: &std::path::Path, close_ups: bool| {
            let mut records = vec![frame(0, 0.0, 47.3, 0)];
            if close_ups {
                records.extend((1..=9).map(|k| frame(k, 0.0, 20.0, k as i32)));
                records.push(frame(10, 0.0, 47.3, 10));
            }
            for k in 0..30 {
                let key = records.len() as i32;
                records.push(frame(records.len(), 0.5 + k as f32, 47.3, key));
                for face in 1..=6 {
                    let key = records.len() as i32;
                    let mut f = frame(records.len(), 0.5 + k as f32, 90.0, key);
                    (f.width, f.height, f.projection) = (420, 420, [1.0, 1.0, 0.0, 0.0]);
                    f.strip_flags = flags::compose(flags::KIND_RIG, 0, 0, face, 0);
                    records.push(f);
                }
            }
            std::fs::create_dir_all(dir).unwrap();
            let bytes: Vec<u8> = records.iter().flat_map(|r| r.to_bytes()).collect();
            assert_eq!(bytes.len(), records.len() * RECORD_SIZE);
            std::fs::write(dir.join(orbiters_core::contracts::manifest::FRAMES_FILE), bytes).unwrap();
            // The reconstruction's preview is a held-out keyframe of the flight (keyframe 25).
            let result = reconstruction::result_dir(dir);
            std::fs::create_dir_all(&result).unwrap();
            let report = reconstruction::Report { held_out_keyframes: vec![25], ..Default::default() };
            std::fs::write(result.join(reconstruction::REPORT_FILE), serde_json::to_vec(&report).unwrap()).unwrap();
        };
        let root = std::env::temp_dir().join(format!("orbiters-views-close-ups-{}", std::process::id()));
        write(&root.join("picture"), true);
        let (views, start) = recorded(&root.join("picture")).unwrap();
        assert_eq!(start, 0, "the start keyframe");
        assert!(views[0].framed && views.iter().filter(|v| v.framed).count() == 1);
        assert!((views[0].fov_y.to_degrees() - 47.3).abs() < 0.01, "its own field of view, not a close-up's");
        write(&root.join("plain"), false);
        let (views, start) = recorded(&root.join("plain")).unwrap();
        assert_eq!(start, 25, "the preview's keyframe, as before");
        assert!(views.iter().all(|v| !v.framed));
        let _ = std::fs::remove_dir_all(&root);
    }
}
