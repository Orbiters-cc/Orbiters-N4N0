//! Where each session's start view stands (orbiters_reconstruct::ground): the ground under the person who captured,
//! their eye height, and how far the recorded camera the view used to start from was from that eye height.
//!
//! ```text
//! cargo run --release --example ground -- <session> [<session> …]
//! ```

use orbiters_core::contracts::frame_record::record_flags;
use orbiters_core::{bundle, reconstruction};
use orbiters_reconstruct::{camera::Camera, ground, ply};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    for session in std::env::args().skip(1) {
        let dir = Path::new(&session);
        let g = ply::read(&reconstruction::result_dir(dir).join(reconstruction::PLY_FILE))?;
        let report = reconstruction::read_report(dir);
        let records = bundle::read_records(dir)?;
        let keyframes: Vec<_> = records.iter().filter(|r| r.has(record_flags::KEYFRAME) && r.has(record_flags::STRIP_OK) && r.keyframe_index >= 0).collect();
        let start = report.as_ref().and_then(|r| r.held_out_keyframes.first()).and_then(|k| keyframes.iter().position(|r| r.keyframe_index == *k)).unwrap_or(0);
        let r = keyframes[start];
        let camera = Camera::from_strip(&r.camera_to_world, &r.projection, r.width.max(1), r.height.max(1), 1.0).position();
        let found = ground::find(dir, &g, camera, Some(r.capture_time_ns));
        let hips = ground::hips(dir, Some(r.capture_time_ns));
        println!(
            "{}: {:?}, floor {:.3} m, eye height {:.2} m (recorded: {}), hips {}, camera at {:.2} m over the ground: the eye moves {:+.2} m",
            dir.file_name().unwrap_or_default().to_string_lossy(),
            found.source,
            found.floor,
            found.eye_height,
            ground::eye_height(dir, Some(r.capture_time_ns)).is_some(),
            hips.map_or("none".into(), |h| format!("{:.2} m over the ground", h[1] - found.floor)),
            camera[1] - found.floor,
            found.eye(camera)[1] - camera[1],
        );
    }
    Ok(())
}
