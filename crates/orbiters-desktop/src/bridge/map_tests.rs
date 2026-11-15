use super::*;
use std::sync::Arc;

/// A wall from (x0, z0) to (x1, z1), 3 m high.
pub(crate) type Wall = ([f32; 2], [f32; 2]);

fn crosses(a: [f32; 2], b: [f32; 2], w: &Wall) -> bool {
    let d = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let (c, e) = (w.0, w.1);
    let (d1, d2, d3, d4) = (d(c, e, a), d(c, e, b), d(a, b, c), d(a, b, e));
    ((d1 > 1e-4 && d2 < -1e-4) || (d1 < -1e-4 && d2 > 1e-4)) && ((d3 > 1e-4 && d4 < -1e-4) || (d3 < -1e-4 && d4 > 1e-4))
}

/// A capture of a world of walls and a floor at y 0, seen from `eye` (x, z) at 1.6 m: the points a camera there could see
/// (nothing behind a wall), and a short flight around it.
fn capture(name: &str, walls: &[Wall], floor: ([f32; 2], [f32; 2]), eye: [f32; 2], avatar: Option<Avatar>) -> Arc<Capture> {
    let (points, path) = world(walls, floor, eye);
    Arc::new(Capture {
        session: name.into(),
        world: Some("wrld_test".into()),
        world_name: Some("Test".into()),
        taken: 1,
        centre: [eye[0], 1.6, eye[1]],
        path: path.clone(),
        avatar,
        eye_height: EYE_HEIGHT,
        voxels: voxelize(&points, &path),
    })
}

/// A session folder as a 3D picture of such a world leaves it once reconstructed: frames.bin (the flight, starting at the
/// eye), skeleton.bin (the avatar's hips and heading) and reconstruction/splats.ply.
pub(crate) fn write_session(dir: &Path, walls: &[Wall], floor: ([f32; 2], [f32; 2]), eye: [f32; 2], hips: [f32; 3], heading: f32) {
    use orbiters_contracts::frame_record::{record_flags, FrameRecord};
    let (points, mut path) = world(walls, floor, eye);
    path.insert(0, [eye[0], 1.6, eye[1]]);
    std::fs::create_dir_all(dir.join("reconstruction")).unwrap();
    let mut frames = Vec::new();
    for p in &path {
        let r = FrameRecord { flags: record_flags::STRIP_OK, camera_to_world: [1.0, 0.0, 0.0, p[0], 0.0, 1.0, 0.0, p[1], 0.0, 0.0, 1.0, p[2]], ..Default::default() };
        frames.extend_from_slice(&r.to_bytes());
    }
    std::fs::write(dir.join("frames.bin"), frames).unwrap();
    let mut sk = b"OSKL".to_vec();
    sk.extend_from_slice(&1u16.to_le_bytes());
    sk.extend_from_slice(&916u16.to_le_bytes());
    sk.extend_from_slice(&[0; 8]);
    let mut rec = vec![0u8; 916];
    for (k, v) in hips.iter().enumerate() {
        rec[16 + k * 4..20 + k * 4].copy_from_slice(&v.to_le_bytes());
    }
    rec[28..36].copy_from_slice(&(1u64 << 10 | 1).to_le_bytes());
    let (s, c) = (heading.to_radians() / 2.0).sin_cos();
    for (k, v) in [0.0f32, s, 0.0, c].iter().enumerate() {
        rec[36 + 10 * 16 + k * 4..40 + 10 * 16 + k * 4].copy_from_slice(&v.to_le_bytes());
    }
    sk.extend_from_slice(&rec);
    std::fs::write(dir.join("skeleton.bin"), sk).unwrap();
    let rows: Vec<[f32; 23]> = points.iter().map(|p| splat(*p, 2.0, 0.05, 0.0, 9.0)).collect();
    std::fs::write(dir.join("reconstruction").join("splats.ply"), ply(&rows)).unwrap();
}

/// The points seen from `eye` and a flight around it.
fn world(walls: &[Wall], floor: ([f32; 2], [f32; 2]), eye: [f32; 2]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let seen = |p: [f32; 2], own: Option<usize>| walls.iter().enumerate().all(|(k, w)| Some(k) == own || !crosses(eye, p, w));
    let mut points = Vec::new();
    let mut x = floor.0[0];
    while x <= floor.1[0] {
        let mut z = floor.0[1];
        while z <= floor.1[1] {
            if seen([x, z], None) {
                for _ in 0..3 {
                    points.push([x, 0.0, z]);
                }
            }
            z += 0.1;
        }
        x += 0.1;
    }
    for (k, w) in walls.iter().enumerate() {
        let length = dist2(w.0, w.1);
        let mut s = 0.0;
        while s <= length {
            let p = [w.0[0] + (w.1[0] - w.0[0]) * s / length, w.0[1] + (w.1[1] - w.0[1]) * s / length];
            // A wall is seen from its side towards the eye only where nothing else is in the way.
            if seen(p, Some(k)) {
                let mut y = 0.05;
                while y < 3.0 {
                    points.push([p[0], y, p[1]]);
                    points.push([p[0], y, p[1]]);
                    y += 0.1;
                }
            }
            s += 0.1;
        }
    }
    let path: Vec<[f32; 3]> = (0..60).map(|k| {
        let a = k as f32 / 60.0 * std::f32::consts::TAU;
        [eye[0] + a.cos() * 1.5, 1.6 + (k % 10) as f32 * 0.08, eye[1] + a.sin() * 1.5]
    }).collect();
    (points, path)
}

fn box_walls(x0: f32, z0: f32, x1: f32, z1: f32) -> Vec<Wall> {
    vec![([x0, z0], [x1, z0]), ([x1, z0], [x1, z1]), ([x1, z1], [x0, z1]), ([x0, z1], [x0, z0])]
}

/// Room 1 (0..12 by 0..10), room 2 (12..24 by 0..10), a door 1.5 m wide in the wall between at z 4.25..5.75.
pub(crate) fn two_rooms() -> Vec<Wall> {
    vec![
        ([0.0, 0.0], [24.0, 0.0]),
        ([24.0, 0.0], [24.0, 10.0]),
        ([24.0, 10.0], [0.0, 10.0]),
        ([0.0, 10.0], [0.0, 0.0]),
        ([12.0, 0.0], [12.0, 4.25]),
        ([12.0, 5.75], [12.0, 10.0]),
    ]
}

fn avatar_at(x: f32, z: f32, heading: f32) -> Option<Avatar> {
    Some(Avatar { hips: [x, 1.02, z], heading })
}

#[test]
fn the_plan_shows_walls_floor_and_what_no_picture_saw() {
    let map = Map::build(vec![capture("one", &two_rooms(), ([0.0, 0.0], [24.0, 10.0]), [4.0, 5.0], avatar_at(3.0, 5.0, 90.0))]);
    assert!((map.floor - 0.0).abs() <= 0.3, "the floor is found under the avatar: {}", map.floor);
    let cell = |x: f32, z: f32| map.cells[map.plan_cell([x, z]).unwrap()];
    assert_eq!(cell(6.0, 5.0), Cell::Floor);
    assert_eq!(cell(12.0, 2.0), Cell::Blocked, "the wall between the rooms");
    assert_eq!(cell(0.1, 5.0), Cell::Blocked, "the outer wall");
    assert!(cell(12.0, 5.0).walkable(), "the door: {:?}", cell(12.0, 5.0));
    assert_eq!(cell(20.0, 1.0), Cell::Unknown, "room 2's corner is out of sight through the door");
    assert!(cell(20.0, 5.0).walkable(), "room 2 seen through the door");
}

#[test]
fn a_camera_pose_is_told_free_solid_unknown_or_outside() {
    let map = Map::build(vec![capture("one", &two_rooms(), ([0.0, 0.0], [24.0, 10.0]), [4.0, 5.0], None)]);
    assert_eq!(map.check_point([6.0, 1.6, 5.0]).at, "free");
    assert_eq!(map.check_point([12.0, 1.5, 2.0]).at, "solid", "inside the wall between the rooms");
    assert_eq!(map.check_point([6.0, 1.6, 0.6]).at, "near_solid", "against the outer wall");
    assert_eq!(map.check_point([6.0, 1.6, 0.1]).at, "solid", "in the outer wall");
    assert_eq!(map.check_point([20.0, 1.6, 1.0]).at, "unknown", "the hidden corner of room 2");
    assert_eq!(map.check_point([400.0, 1.6, 5.0]).at, "outside");
    let floor = map.check_point([6.0, 1.6, 5.0]).ground_below_m.unwrap();
    assert!((floor - 1.6).abs() <= 0.3, "the ground under it: {floor}");
    assert!(map.check_point([18.0, 1.6, 2.5]).behind_something, "behind the wall between the rooms from the picture's centre");
    assert!(!map.check_point([8.0, 1.6, 5.0]).behind_something);
}

#[test]
fn a_walk_is_told_how_far_it_is_clear() {
    let map = Map::build(vec![capture("one", &two_rooms(), ([0.0, 0.0], [24.0, 10.0]), [4.0, 5.0], None)]);
    // West from (3, 5): the outer wall at x 0.
    let west = map.check_walk([3.0, 5.0], 270.0, 10.0);
    assert_eq!(west.stops, Some("blocked"));
    assert!((2.0..=3.2).contains(&west.clear_m), "{west:?}");
    // North (+z) from (6, 5): 5 m to the wall.
    let north = map.check_walk([6.0, 5.0], 0.0, 3.0);
    assert_eq!(north.stops, None, "{north:?}");
}

#[test]
fn another_room_is_suggested_first_and_far_enough_with_a_walk_through_the_door() {
    let map = Map::build(vec![capture("one", &two_rooms(), ([0.0, 0.0], [24.0, 10.0]), [4.0, 5.0], avatar_at(3.0, 5.0, 90.0))]);
    let size = map.size();
    assert!(size.areas >= 2, "{size:?}");
    assert!(size.openings >= 1, "{size:?}");
    let verdict = map.verdict(&size);
    assert_eq!(verdict.pictures, 3, "{verdict:?}");
    let suggestions = map.suggestions(&size);
    assert!(!suggestions.is_empty());
    let first = &suggestions[0];
    assert_eq!(first.kind, "another_area", "{suggestions:?}");
    assert!(first.stand[0] > 12.0, "in room 2: {first:?}");
    for s in &suggestions {
        assert!(dist2(s.stand, [4.0, 5.0]) >= APART_M, "every suggestion 10 m from the centre: {s:?}");
        assert_eq!(map.check_point([s.pose[0], s.pose[1], s.pose[2]]).at, "free", "{s:?}");
    }
    let route = map.route([3.0, 5.0], first.stand).expect("a way through the door");
    let through = route.windows(2).any(|w| {
        let t = (12.0 - w[0][0]) / (w[1][0] - w[0][0]);
        (0.0..=1.0).contains(&t) && (4.2..=5.8).contains(&(w[0][1] + (w[1][1] - w[0][1]) * t))
    });
    assert!(through, "the walk goes through the door: {route:?}");
    let steps = walk_steps(&route, 90.0, 4.0);
    let walked: f32 = steps.iter().map(|s| s.walk_m).sum();
    assert!(walked >= dist2([3.0, 5.0], first.stand) - 0.5, "{steps:?}");
    // None of the suggestions is near another.
    for (i, a) in suggestions.iter().enumerate() {
        for b in &suggestions[i + 1..] {
            assert!(dist2(a.stand, b.stand) >= 8.0, "{a:?} {b:?}");
        }
    }
}

#[test]
fn a_small_closed_empty_room_needs_one_picture() {
    let map = Map::build(vec![capture("box", &box_walls(0.0, 0.0, 10.0, 8.0), ([0.0, 0.0], [10.0, 8.0]), [5.0, 4.0], avatar_at(5.0, 3.0, 0.0))]);
    let size = map.size();
    let verdict = map.verdict(&size);
    assert!(verdict.simple, "{size:?} {verdict:?}");
    assert_eq!(verdict.pictures, 1);
    assert_eq!(size.areas, 1, "{size:?}");
    assert!((size.extent_m[0] - 10.0).abs() <= 1.5 && (size.extent_m[1] - 8.0).abs() <= 1.5, "{size:?}");
}

#[test]
fn pictures_of_two_rooms_merge_into_one_plan() {
    let walls = two_rooms();
    let one = capture("one", &walls, ([0.0, 0.0], [24.0, 10.0]), [4.0, 5.0], avatar_at(3.0, 5.0, 90.0));
    let two = capture("two", &walls, ([0.0, 0.0], [24.0, 10.0]), [18.0, 5.0], avatar_at(18.0, 5.0, 270.0));
    let map = Map::build(vec![one, two]);
    let cell = |x: f32, z: f32| map.cells[map.plan_cell([x, z]).unwrap()];
    assert!(cell(20.0, 1.0).walkable(), "the second picture saw room 2's corner");
    assert!(cell(6.0, 5.0).walkable());
    let size = map.size();
    let suggestions = map.suggestions(&size);
    for s in &suggestions {
        for c in [[4.0, 5.0], [18.0, 5.0]] {
            assert!(dist2(s.stand, c) >= APART_M - 0.01, "{s:?}");
        }
    }
}

#[test]
fn aim_camera_calls_reach_the_pose_from_the_camera() {
    // From heading 90 (+x): 10 m east is 10 forward; 3 m north (+z) is 3 left.
    let steps = aim_steps([0.0, 1.6, 0.0, 0.0, 90.0], [10.0, 5.6, 3.0, 20.0, 180.0], 24.0);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["forward_m"], 10.0);
    assert_eq!(steps[0]["right_m"], -3.0);
    assert_eq!(steps[0]["up_m"], 4.0);
    assert_eq!(steps[0]["turn_deg"], 90.0);
    assert_eq!(steps[0]["pitch_deg"], 20.0);
    // Further than a call goes: several, the turn on the last.
    let far = aim_steps([0.0, 0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 40.0, 0.0, 0.0], 24.0);
    assert_eq!(far.len(), 2);
    assert_eq!(far[0]["forward_m"], 20.0);
    assert!(far[0].get("turn_deg").is_none());
    // aim_camera's own maths takes them where they say.
    let (s, c) = 90f32.to_radians().sin_cos();
    let (f, r) = (10.0, -3.0);
    assert!(((0.0 + f * s + r * c) - 10.0).abs() < 1e-4 && ((0.0 + f * c - r * s) - 3.0).abs() < 1e-4);
}

#[test]
fn walk_steps_turn_from_the_heading_before() {
    let steps = walk_steps(&[[0.0, 0.0], [0.0, 8.0], [6.0, 8.0]], 90.0, 4.0);
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].turn_deg, -90.0, "from east to north is 90 left");
    assert_eq!(steps[0].walk_m, 8.0);
    assert_eq!(steps[0].walk_s, 2.0);
    assert_eq!(steps[1].turn_deg, 90.0);
    assert_eq!(steps[1].heading, 90.0);
}

#[test]
fn a_capture_keeps_through_its_file() {
    let c = capture("one", &two_rooms(), ([0.0, 0.0], [24.0, 10.0]), [4.0, 5.0], avatar_at(3.0, 5.0, 90.0));
    let text = serde_json::to_string(&*c).unwrap();
    let back: Capture = serde_json::from_str(&text).unwrap();
    assert_eq!(back, *c);
    assert!(text.len() < c.voxels.len() * 12 + 20_000, "compact: {} bytes for {} voxels", text.len(), c.voxels.len());
}

#[test]
fn the_plan_picture_is_a_small_png_with_its_marks() {
    let map = Map::build(vec![capture("one", &two_rooms(), ([0.0, 0.0], [24.0, 10.0]), [4.0, 5.0], avatar_at(3.0, 5.0, 90.0))]);
    let size = map.size();
    let suggestions = map.suggestions(&size);
    let marks = Marks {
        centres: map.captures.iter().map(|c| c.centre).collect(),
        camera: Some(([4.0, 1.6, 5.0], 90.0)),
        avatar: Some(([3.0, 5.0], 90.0)),
        route: map.route([3.0, 5.0], suggestions[0].stand).unwrap(),
        suggestions,
    };
    let png = map.picture(&marks, 360);
    let decoder = png::Decoder::new(std::io::Cursor::new(&png));
    let reader = decoder.read_info().unwrap();
    let info = reader.info();
    assert!(info.width <= 380 && info.height <= 380 && info.width >= 100, "{}x{}", info.width, info.height);
    assert!(png.len() < 60_000, "{} bytes", png.len());
}

/// A PLY of the reference layout with Orbiters' extras, as orbiters_reconstruct::ply::write makes it (Z negated).
fn ply(rows: &[[f32; 23]]) -> Vec<u8> {
    let names = ["x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0", "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3", "sky", "seen_weight", "seen_views", "seen_sectors", "seen_rate", "seen_beyond"];
    let mut out = format!("ply\nformat binary_little_endian 1.0\nelement vertex {}\n", rows.len()).into_bytes();
    for n in names {
        out.extend_from_slice(format!("property float {n}\n").as_bytes());
    }
    out.extend_from_slice(b"end_header\n");
    for r in rows {
        for v in r {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn splat(unity: [f32; 3], opacity: f32, scale: f32, sky: f32, views: f32) -> [f32; 23] {
    let mut r = [0.0; 23];
    r[0] = unity[0];
    r[1] = unity[1];
    r[2] = -unity[2];
    r[9] = opacity;
    r[10] = scale.ln();
    r[11] = scale.ln();
    r[12] = scale.ln();
    r[13] = 1.0;
    r[17] = sky;
    r[19] = views;
    r
}

#[test]
fn splats_are_read_in_unity_s_world_and_only_those_worth_a_map() {
    let rows = [
        splat([1.0, 2.0, 3.0], 2.0, 0.05, 0.0, 10.0),
        splat([1.0, 2.0, 3.0], -3.0, 0.05, 0.0, 10.0),
        splat([1.0, 2.0, 3.0], 2.0, 3.0, 0.0, 10.0),
        splat([1.0, 2.0, 3.0], 2.0, 0.05, 1.0, 10.0),
        splat([1.0, 2.0, 3.0], 2.0, 0.05, 0.0, 1.0),
    ];
    let points = ply_points(&ply(&rows)).unwrap();
    assert_eq!(points, vec![[1.0, 2.0, 3.0]], "only the opaque, small, seen, not-sky one, with z back to Unity's");
    assert!(ply_points(b"not a ply").is_err());
}

#[test]
fn a_session_folder_gives_its_centre_path_avatar_and_voxels() {
    use orbiters_contracts::frame_record::{record_flags, FrameRecord};
    let dir = std::env::temp_dir().join(format!("orbiters-map-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("reconstruction")).unwrap();
    std::fs::write(dir.join("manifest.json"), r#"{"world":{"id":"WRLD_ABC","name":"A world"}}"#).unwrap();
    let mut frames = Vec::new();
    for k in 0..10 {
        let mut r = FrameRecord { flags: record_flags::STRIP_OK, ..Default::default() };
        r.camera_to_world = [1.0, 0.0, 0.0, -9.74 + k as f32 * 0.1, 0.0, 1.0, 0.0, 3.64, 0.0, 0.0, 1.0, 2.47];
        frames.extend_from_slice(&r.to_bytes());
    }
    // A rig face repeats its frame's position, and a gated frame is not used.
    let mut face = FrameRecord { flags: record_flags::STRIP_OK, strip_flags: 3 << 8, ..Default::default() };
    face.camera_to_world[3] = 500.0;
    frames.extend_from_slice(&face.to_bytes());
    let mut gated = FrameRecord { flags: record_flags::STRIP_OK | record_flags::GATED, ..Default::default() };
    gated.camera_to_world[3] = 600.0;
    frames.extend_from_slice(&gated.to_bytes());
    std::fs::write(dir.join("frames.bin"), frames).unwrap();
    // skeleton.bin: header, one record with hips and a head turned to heading 90.
    let mut sk = b"OSKL".to_vec();
    sk.extend_from_slice(&1u16.to_le_bytes());
    sk.extend_from_slice(&916u16.to_le_bytes());
    sk.extend_from_slice(&[0; 8]);
    let mut rec = vec![0u8; 916];
    for (k, v) in [-9.77f32, 1.0, -9.05].iter().enumerate() {
        rec[16 + k * 4..20 + k * 4].copy_from_slice(&v.to_le_bytes());
    }
    rec[28..36].copy_from_slice(&(1u64 << 10 | 1).to_le_bytes());
    let h = std::f32::consts::FRAC_1_SQRT_2;
    for (k, v) in [0.0f32, h, 0.0, h].iter().enumerate() {
        rec[36 + 10 * 16 + k * 4..40 + 10 * 16 + k * 4].copy_from_slice(&v.to_le_bytes());
    }
    sk.extend_from_slice(&rec);
    std::fs::write(dir.join("skeleton.bin"), sk).unwrap();
    let rows: Vec<[f32; 23]> = (0..20).map(|k| splat([-5.0 + k as f32 * 0.01, 0.1, 1.0], 2.0, 0.05, 0.0, 9.0)).chain([splat([20000.0, 0.0, 0.0], 2.0, 0.05, 0.0, 9.0)]).collect();
    std::fs::write(dir.join("reconstruction").join("splats.ply"), ply(&rows)).unwrap();
    let c = Capture::read(&dir).unwrap();
    assert_eq!(c.world.as_deref(), Some("wrld_abc"));
    assert_eq!(c.centre, [-9.74, 3.64, 2.47], "the first frame's camera: where aim_camera put it");
    assert_eq!(c.path.len(), 10, "no rig face, no gated frame");
    let a = c.avatar.unwrap();
    assert_eq!(a.hips, [-9.77, 1.0, -9.05]);
    assert!((a.heading - 90.0).abs() < 0.01, "{}", a.heading);
    assert_eq!(c.voxels.iter().map(|v| v[3]).sum::<i32>(), 20, "the stray point 20 km out is left out");
    assert_eq!(c.eye_height, EYE_HEIGHT);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Runs the map on real captures (`ORBITERS_MAP_SESSIONS`: session folders separated by `|`, one world per `;`), writing
/// each world's plan picture and answer to `ORBITERS_MAP_OUT`, and the pictures' shares (their path thinned) as fixtures when
/// `ORBITERS_MAP_FIXTURES` names a folder. For tuning and the report; not part of the suite.
#[test]
#[ignore]
fn real_captures() {
    let Ok(worlds) = std::env::var("ORBITERS_MAP_SESSIONS") else { return };
    let out = std::path::PathBuf::from(std::env::var("ORBITERS_MAP_OUT").unwrap_or_else(|_| ".".into()));
    std::fs::create_dir_all(&out).unwrap();
    for (n, world) in worlds.split(';').filter(|w| !w.is_empty()).enumerate() {
        let t = std::time::Instant::now();
        let captures: Vec<Arc<Capture>> = world.split('|').map(|d| Arc::new(Capture::read(std::path::Path::new(d)).unwrap())).collect();
        let read = t.elapsed();
        if let Ok(dir) = std::env::var("ORBITERS_MAP_FIXTURES") {
            for c in &captures {
                let mut small = (**c).clone();
                small.path = thin(&small.path, 60);
                std::fs::write(std::path::Path::new(&dir).join(format!("{}.json", c.session)), serde_json::to_vec(&small).unwrap()).unwrap();
            }
        }
        let (captures, elsewhere) = one_place(captures);
        let map = Map::build(captures);
        let built = t.elapsed() - read;
        let size = map.size();
        let verdict = map.verdict(&size);
        let suggestions = map.suggestions(&size);
        let avatar = map.captures.iter().rev().find_map(|c| c.avatar);
        let route = avatar.and_then(|a| suggestions.first().and_then(|s| map.route([a.hips[0], a.hips[2]], s.stand))).unwrap_or_default();
        let marks = Marks {
            centres: map.captures.iter().map(|c| c.centre).collect(),
            camera: None,
            avatar: avatar.map(|a| ([a.hips[0], a.hips[2]], a.heading)),
            suggestions: suggestions.clone(),
            route: route.clone(),
        };
        std::fs::write(out.join(format!("world{n}.png")), map.picture(&marks, 360)).unwrap();
        let checks: Vec<_> = map.captures.iter().map(|c| (c.session.clone(), map.check_point(c.centre))).collect();
        let answer = serde_json::json!({
            "pictures": map.captures.iter().map(|c| (c.session.clone(), c.centre, c.avatar, c.voxels.len())).collect::<Vec<_>>(),
            "floor": map.floor, "voxel": map.voxel, "plan": map.plan_dims,
            "size": size, "verdict": verdict, "suggestions": suggestions,
            "route": route, "walk": avatar.map(|a| walk_steps(&route, a.heading, 4.0)),
            "centre_checks": checks,
            "openings": map.openings().iter().map(|o| (map.cell_centre(o[o.len() / 2]), o.len())).collect::<Vec<_>>(),
            "read_ms": read.as_millis(), "build_ms": built.as_millis(), "elsewhere": elsewhere.iter().map(|c| c.session.clone()).collect::<Vec<_>>(),
        });
        std::fs::write(out.join(format!("world{n}.json")), serde_json::to_vec_pretty(&answer).unwrap()).unwrap();
    }
}

/// A picture's share as the tour left it on 2026-11-15 (its voxels; 60 positions of its path, not 240).
fn fixture(json: &str) -> Arc<Capture> {
    Arc::new(serde_json::from_str(json).unwrap())
}

/// Prismic's Avatar Search, two pictures: an empty grey box about 20 m across, walled all round, open to the sky (the tour's
/// screenshots of 2026-11-15 06:00), the avatar standing in its south-west corner. One picture is enough.
#[test]
fn a_real_empty_walled_box_is_one_picture() {
    let map = Map::build(vec![fixture(include_str!("map_fixtures/prismic-1.json")), fixture(include_str!("map_fixtures/prismic-2.json"))]);
    let size = map.size();
    assert!((17.0..=25.0).contains(&size.extent_m[0]) && (17.0..=25.0).contains(&size.extent_m[1]), "{size:?}");
    assert_eq!(size.areas, 1, "{size:?}");
    let verdict = map.verdict(&size);
    assert!(verdict.simple && verdict.pictures == 1, "{size:?} {verdict:?}");
    assert!((map.floor - 0.0).abs() <= 0.2, "the floor 1.66 m (eye height) under where the camera opened at 1.64: {}", map.floor);
    // aim_camera put the second picture's camera at x -9.74, against the west wall.
    assert_eq!(map.check_point(map.captures[1].centre).at, "near_solid");
    assert_eq!(map.check_point([0.0, 1.6, 0.0]).at, "free", "the middle of the box");
    assert_eq!(map.check_point([-10.1, 2.0, 0.0]).at, "solid", "in the west wall (x -10.25 to -9.75)");
    assert_eq!(map.check_point([0.0, 1.6, 40.0]).at, "outside");
}

/// Japan Street: an L of station corridors with gates (the tour's thumbnail: a corridor with ticket gates); the picture saw two
/// arms from their corner. More than one picture; a suggestion down an arm, 10 m or more away, walkable from the avatar.
#[test]
fn real_corridors_lead_on_and_a_walk_goes_down_them() {
    let map = Map::build(vec![fixture(include_str!("map_fixtures/japan-street.json"))]);
    let size = map.size();
    let verdict = map.verdict(&size);
    assert_eq!(verdict.pictures, 3, "{size:?} {verdict:?}");
    assert!((map.floor - 0.98).abs() <= 0.2, "{}", map.floor);
    let suggestions = map.suggestions(&size);
    let first = suggestions.first().expect("somewhere to go");
    assert!(first.from_centres_m >= APART_M, "{first:?}");
    let a = map.captures[0].avatar.unwrap();
    let route = map.route([a.hips[0], a.hips[2]], first.stand).expect("down the corridor");
    let steps = walk_steps(&route, a.heading, 4.0);
    assert!(steps.len() <= 5, "a few turns and walks: {steps:?}");
    assert!(steps.iter().map(|s| s.walk_m).sum::<f32>() >= 9.0, "{steps:?}");
    for w in route.windows(2) {
        assert!(map.check_walk(w[0], heading_of(w[1][0] - w[0][0], w[1][1] - w[0][1]), dist2(w[0], w[1])).stops != Some("blocked"), "{route:?}");
    }
}

/// Blonix Dungeon: the spawn is a closed rock cell (the tour: "cell corner with a torch", its door would not open). The plan
/// says it saw only a small closed space, and to look for the way out, rather than calling the world simple.
#[test]
fn a_real_closed_cell_is_said_to_be_small_not_the_world() {
    let map = Map::build(vec![fixture(include_str!("map_fixtures/blonix-cell.json"))]);
    let size = map.size();
    let verdict = map.verdict(&size);
    assert!(!verdict.simple, "{verdict:?}");
    assert!(verdict.why.contains("small closed space"), "{verdict:?}");
}

#[test]
fn pictures_kilometres_apart_are_other_parts_of_the_world() {
    let near = capture("near", &two_rooms(), ([0.0, 0.0], [24.0, 10.0]), [4.0, 5.0], None);
    let mut far = (*capture("far", &two_rooms(), ([0.0, 0.0], [24.0, 10.0]), [4.0, 5.0], None)).clone();
    far.centre = [900.0, 1.6, 3000.0];
    let (here, elsewhere) = one_place(vec![Arc::new(far), near]);
    assert_eq!((here.len(), elsewhere.len()), (1, 1));
    assert_eq!(here[0].session, "near", "the plan is of where the newest picture is");
}
