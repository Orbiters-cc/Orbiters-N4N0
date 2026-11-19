//! VRChat's camera dolly as three more cameras for a 3D picture (experimental; VRC+; Settings, "Camera dolly streams").
//!
//! VRChat's Camera Dolly (VRC+) animates the camera along paths of points, and in its multi-stream mode (dolly menu,
//! Streaming: Multi; Windows) it spawns up to three more cameras, one for each path after the first, each with a Spout
//! sender of its own: `VRCSender2` to `VRCSender4` beside the camera's `VRCSender1`, only while the animation plays.
//! The first path moves the camera itself. Since VRChat 2026.3.2 the extra cameras also animate zoom, focus, exposure,
//! the greenscreen colour and the Look-At-Me offsets (before, only position and rotation). OSC imports paths
//! (`/dolly/Import` with the path of a JSON file: an array of points with `PathIndex`, `Index`, `Position`, `Rotation`,
//! `Duration`, …; `IsLocal` false for world coordinates) and plays them (`/dolly/Play`). The dolly menu's Streaming and
//! Motion Control settings are not part of an import: the person sets Multi (and time-based motion) once in VRChat.
//!
//! A 3D picture with them: the flight's camera poses become the first path, and three cameras fly the same flight
//! offset left, right and above the camera, each looking at what the camera looks at, so every frame shows the place
//! from four points at once. The capture reads the four senders (orbiters-capture's `MultiSource`); whatever frames of
//! the extra cameras carry a capture strip (the avatar's capture kit drawn in their renders too, which only a try in
//! VRChat tells) become views of their own in the session, as the camera's own do.

use std::path::PathBuf;

/// The camera's own Spout sender and the dolly's other three.
pub const MAIN_SENDER: &str = "VRCSender1";
pub const EXTRA_SENDERS: [&str; 3] = ["VRCSender2", "VRCSender3", "VRCSender4"];
pub const IMPORT: &str = "/dolly/Import";
pub const PLAY: &str = "/dolly/Play";
/// Points a path may have: VRChat on Windows takes 100 in all, 50 a path; four paths of 25.
pub const POINTS: usize = 25;
/// Where the three extra cameras stand around the camera, in its own axes (right, up), metres: left, right and above.
/// Within 2 m of it, where N4N0's capture kit, its bounds 4 m around the rig at the camera, is drawn for them too.
pub const OFFSETS: [[f32; 2]; 3] = [[-1.2, 0.0], [1.2, 0.0], [0.0, 1.2]];

/// A camera pose as `/usercamera/Pose` has it: x, y, z (m), pitch, yaw, roll (degrees, Unity's Euler angles).
pub type Pose = [f32; 6];

/// The camera's right, up and forward axes for a pose (Unity: `Quaternion.Euler(pitch, yaw, roll)`).
pub fn axes(p: &Pose) -> [[f32; 3]; 3] {
    let (sp, cp) = p[3].to_radians().sin_cos();
    let (sy, cy) = p[4].to_radians().sin_cos();
    let (sr, cr) = p[5].to_radians().sin_cos();
    let right = [cr * cy + sr * sp * sy, sr * cp, -cr * sy + sr * sp * cy];
    let up = [-sr * cy + cr * sp * sy, cr * cp, sr * sy + cr * sp * cy];
    let forward = [cp * sy, -sp, cp * cy];
    [right, up, forward]
}

/// A camera at `p` offset by `offset` (right, up) in `p`'s axes, looking at the point `focus` metres ahead of `p`, level.
pub fn offset_pose(p: &Pose, offset: [f32; 2], focus: f32) -> Pose {
    let [right, up, forward] = axes(p);
    let at: [f32; 3] = std::array::from_fn(|k| p[k] + right[k] * offset[0] + up[k] * offset[1]);
    let target: [f32; 3] = std::array::from_fn(|k| p[k] + forward[k] * focus);
    let d: [f32; 3] = std::array::from_fn(|k| target[k] - at[k]);
    let flat = (d[0] * d[0] + d[2] * d[2]).sqrt();
    let yaw = d[0].atan2(d[2]).to_degrees();
    let pitch = -(d[1].atan2(flat)).to_degrees();
    [at[0], at[1], at[2], pitch, yaw, 0.0]
}

/// `n` poses of `flight` evenly through it, the first and the last included.
fn sample(flight: &[Pose], n: usize) -> Vec<Pose> {
    if flight.len() <= n {
        return flight.to_vec();
    }
    (0..n).map(|i| flight[i * (flight.len() - 1) / (n - 1)]).collect()
}

/// One dolly point (VRChat's export format).
fn point(path: usize, index: usize, p: &Pose, duration: f32, zoom: f32, focus: f32) -> serde_json::Value {
    serde_json::json!({
        "IsLocal": false,
        "Position": { "X": p[0], "Y": p[1], "Z": p[2] },
        "Rotation": { "X": p[3], "Y": p[4], "Z": p[5] },
        "Zoom": zoom,
        "FocalDistance": focus,
        "Aperture": 15.0,
        "Hue": 120.0,
        "Saturation": 100.0,
        "Lightness": 50.0,
        "LookAtMeXOffset": 0.0,
        "LookAtMeYOffset": 0.0,
        "Exposure": 0.0,
        "Speed": 3.0,
        "Duration": duration,
        "Index": index,
        "PathIndex": path,
    })
}

/// The four paths of a 3D picture's flight (`flight`: its poses at a steady rate over `seconds`): the camera's own, then
/// the three offset cameras ([`OFFSETS`]) looking at the point `focus` metres ahead of the camera. Each path has
/// [`POINTS`] points: the flight sampled, and its first and last points again at its ends, since VRChat's paths only
/// steer by their end points (it goes from the second to the second to last); each step lasts the same time.
pub fn paths(flight: &[Pose], seconds: f32, zoom: f32, focus: f32) -> serde_json::Value {
    let poses = sample(flight, POINTS - 2);
    let steps = poses.len().saturating_sub(1).max(1);
    let duration = (seconds / steps as f32).clamp(0.1, 60.0);
    let mut points = Vec::new();
    for path in 0..=OFFSETS.len() {
        let pose = |p: &Pose| if path == 0 { *p } else { offset_pose(p, OFFSETS[path - 1], focus) };
        let mut list: Vec<Pose> = poses.iter().map(pose).collect();
        if let (Some(first), Some(last)) = (list.first().copied(), list.last().copied()) {
            list.insert(0, first);
            list.push(last);
        }
        points.extend(list.iter().enumerate().map(|(i, p)| point(path, i, p, duration, zoom, focus)));
    }
    serde_json::Value::Array(points)
}

/// Writes the paths where VRChat can read them; `/dolly/Import` takes the file's path.
/// A file of its own per flight: two flights close together (or two builds, or the bridge's VRChats) could otherwise import
/// each other's paths from one shared file (verify2-controller). Files of earlier flights older than an hour go.
pub fn write(paths: &serde_json::Value) -> anyhow::Result<PathBuf> {
    let temp = std::env::temp_dir();
    for entry in std::fs::read_dir(&temp).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 3600);
        if old && name.starts_with("orbiters-n4n0-dolly-") && name.ends_with(".json") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let file = temp.join(format!("orbiters-n4n0-dolly-{}.json", uuid::Uuid::new_v4().simple()));
    std::fs::write(&file, serde_json::to_vec(paths)?)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_follow_unity_euler_angles() {
        let [right, up, forward] = axes(&[0.0, 0.0, 0.0, 0.0, 90.0, 0.0]);
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        assert!(near(forward, [1.0, 0.0, 0.0]) && near(right, [0.0, 0.0, -1.0]) && near(up, [0.0, 1.0, 0.0]));
        // Pitch down 30°: forward dips.
        let [_, _, f] = axes(&[0.0, 0.0, 0.0, 30.0, 0.0, 0.0]);
        assert!((f[1] + 0.5).abs() < 1e-5 && (f[2] - 0.866).abs() < 1e-3);
    }

    #[test]
    fn offset_cameras_look_at_what_the_camera_looks_at() {
        let p = [1.0, 1.5, 2.0, 10.0, 45.0, 0.0];
        let focus = 3.0;
        let [_, _, forward] = axes(&p);
        let target: [f32; 3] = std::array::from_fn(|k| p[k] + forward[k] * focus);
        for offset in OFFSETS {
            let q = offset_pose(&p, offset, focus);
            let moved = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) + (q[2] - p[2]).powi(2)).sqrt();
            assert!((moved - 1.2).abs() < 1e-4, "1.2 m off: {moved}");
            let [_, _, f] = axes(&q);
            let d: [f32; 3] = std::array::from_fn(|k| target[k] - q[k]);
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let cos = (f[0] * d[0] + f[1] * d[1] + f[2] * d[2]) / len;
            assert!(cos > 0.9999, "looks at the target: {cos}");
            assert_eq!(q[5], 0.0, "level");
        }
    }

    #[test]
    fn four_paths_of_twenty_five_points_within_vrchats_limits() {
        let flight: Vec<Pose> = (0..360).map(|i| [i as f32 * 0.01, 1.5, 0.0, 0.0, i as f32 * 0.25, 0.0]).collect();
        let json = paths(&flight, 4.0, 45.0, 2.0);
        let points = json.as_array().unwrap();
        assert_eq!(points.len(), 4 * POINTS, "VRChat on Windows takes 100 points");
        for path in 0..4 {
            let mine: Vec<_> = points.iter().filter(|p| p["PathIndex"] == path).collect();
            assert_eq!(mine.len(), POINTS);
            assert!(mine.iter().enumerate().all(|(i, p)| p["Index"] == i && p["IsLocal"] == false));
            // The ends repeated: VRChat steers by them and goes from the second point to the second to last.
            assert_eq!(mine[0]["Position"], mine[1]["Position"]);
            assert_eq!(mine[POINTS - 1]["Position"], mine[POINTS - 2]["Position"]);
        }
        // The camera's own path follows the flight; its steps take the flight's time between them.
        let first = &points[1];
        assert_eq!((first["Position"]["X"].as_f64(), first["Rotation"]["Y"].as_f64()), (Some(0.0), Some(0.0)));
        let step = points[1]["Duration"].as_f64().unwrap();
        assert!((step * 22.0 - 4.0).abs() < 1e-4, "{step}");
        assert_eq!(points[POINTS - 2]["Position"]["X"].as_f64().map(|x| (x * 100.0).round()), Some(359.0));
    }
}
