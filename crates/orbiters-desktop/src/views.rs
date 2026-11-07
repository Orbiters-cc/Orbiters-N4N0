//! The cameras a session recorded at its keyframes, and where a view of its splats starts: the 3D view
//! (pages/splats.rs) and the web preview shared on the Map explorer (controller/maps.rs) open looking the way the same
//! camera looked, from its place but at the eye height of the person who captured over the ground they stood on
//! (orbiters_reconstruct::ground), orbiting what it looks at. A 3D picture (controller/burst.rs) opens exactly as its camera
//! framed it: that moment is what its owner chose.

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
    /// A 3D picture's own framing (the camera where it stood when the picture was taken): a view opens exactly there, not
    /// at the eye height of the person who took it ([`eye_level`]).
    pub framed: bool,
}

/// How long a recording can be to count as a 3D picture here, game seconds: an 8 s flight, the pause before it and what the
/// capture adds around them came to 11 in the pictures of 2026-11-04.
const PICTURE_S: f32 = 15.0;

/// Every keyframe's camera, and which one to start from: a 3D picture's first frame, else the keyframe the reconstruction's
/// preview shows, else the first. A short recording of another kind opens on its first frame too, which is as good a view.
pub fn recorded(dir: &Path) -> anyhow::Result<(Vec<View>, usize)> {
    let report = reconstruction::read_report(dir);
    let records = bundle::read_records(dir)?;
    let keyframes: Vec<_> = records.iter().filter(|r| r.has(record_flags::KEYFRAME) && r.has(record_flags::STRIP_OK) && r.keyframe_index >= 0).collect();
    let cameras: Vec<Camera> = keyframes.iter().map(|r| Camera::from_strip(&r.camera_to_world, &r.projection, r.width.max(1), r.height.max(1), 1.0)).collect();
    let mut views: Vec<View> = cameras
        .iter()
        .zip(&keyframes)
        .map(|(cam, r)| View {
            position: cam.position(),
            forward: cam.forward(),
            fov_y: usable_fov(cam.fov_y()),
            time_ns: Some(r.capture_time_ns),
            framed: false,
        })
        .collect();
    anyhow::ensure!(!views.is_empty(), "{}", orbiters_i18n::tr("this session has no keyframe to look from"));
    // Of the frames, a rig face's 90° square being no view of the picture.
    use orbiters_core::contracts::strip::flags;
    let frame = keyframes.iter().position(|r| !(flags::kind(r.strip_flags) == flags::KIND_RIG && flags::face(r.strip_flags) != 0));
    let times = keyframes.iter().map(|r| r.game_time);
    let span = times.clone().fold(f32::MIN, f32::max) - times.fold(f32::MAX, f32::min);
    if let Some(i) = frame.filter(|_| span <= PICTURE_S) {
        views[i].framed = true;
        return Ok((views, i));
    }
    let start = report.as_ref().and_then(|r| r.held_out_keyframes.first()).and_then(|k| keyframes.iter().position(|r| r.keyframe_index == *k)).unwrap_or(0);
    Ok((views, start))
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

    /// A 3D picture's frames.bin as the capture writes it: the start, then a flight whose every other frame is a rig frame
    /// with six 90° faces (keyframes of their own, 420 px square, outnumbering the frames). It opens at the start, exactly
    /// where the camera stood, even when a rig face comes first; a longer recording, at the first held-out keyframe as before.
    #[test]
    fn a_3d_picture_opens_at_its_start() {
        use orbiters_core::contracts::frame_record::{FrameRecord, RECORD_SIZE};
        use orbiters_core::contracts::strip::flags;
        let p11 = |fov: f32| 1.0 / (fov.to_radians() / 2.0).tan();
        let frame = |i: usize, x: f32, fov: f32, key: i32, time: f32| FrameRecord {
            flags: record_flags::STRIP_OK | record_flags::KEYFRAME,
            frame_index: i as u64,
            game_time: time,
            camera_to_world: [1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, 1.2, 0.0, 0.0, 1.0, 0.0],
            projection: [p11(fov) * 9.0 / 16.0, p11(fov), 0.0, 0.0],
            width: 1920,
            height: 1080,
            keyframe_index: key,
            ..Default::default()
        };
        let write = |dir: &std::path::Path, seconds_apart: f32| {
            let mut records = vec![frame(0, 0.0, 47.3, 0, 0.0)];
            for k in 0..30 {
                let key = records.len() as i32;
                let time = k as f32 * seconds_apart;
                records.push(frame(records.len(), 0.5 + k as f32, 47.3, key, time));
                for face in 1..=6 {
                    let key = records.len() as i32;
                    let mut f = frame(records.len(), 0.5 + k as f32, 90.0, key, time);
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
        let root = std::env::temp_dir().join(format!("orbiters-views-picture-{}", std::process::id()));
        write(&root.join("picture"), 0.2);
        let (views, start) = recorded(&root.join("picture")).unwrap();
        assert_eq!(start, 0, "the start keyframe");
        assert!(views[0].framed && views.iter().filter(|v| v.framed).count() == 1);
        assert!((views[0].fov_y.to_degrees() - 47.3).abs() < 0.01);
        write(&root.join("long"), 2.0);
        let (views, start) = recorded(&root.join("long")).unwrap();
        assert_eq!(start, 25, "the preview's keyframe, as before");
        assert!(views.iter().all(|v| !v.framed));
        let _ = std::fs::remove_dir_all(&root);
    }
}
