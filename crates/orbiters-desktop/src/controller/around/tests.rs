//! The flight's logic over real poses and depth: Enzo's 4 s picture of 2026-11-04 (ちま部屋 12-14-49). `chima-room.bin` holds that
//! capture's own depth frames at the start pose, in bins, and what is really there: every depth sample of the three pictures
//! taken in that room, in voxels of 15 cm. VRChat is not needed: frames the flight would see from poses that were never flown
//! are drawn by casting rays through the voxels.

use super::*;
use orbiters_core::keyframes::KeyframeSelector;
use orbiters_core::settings::{KeyframeSettings, Settings};
use std::collections::HashSet;

const P11: f32 = 2.2857;
const P00: f32 = 1.2857;
const RATE: u32 = 90;
/// The picture's start pose as VRChat echoed it, its face's box and the avatar's hips and eye height: the session's own.
const START: Pose = [-1.2393, 1.9721, -0.97, 4.59, 116.25, 0.0];
const FACE: [f32; 5] = [872.06, 493.41, 982.48, 603.83, 0.719];
const HIPS: V = [1.1319, 1.0603, -2.0635];
const EYE_HEIGHT: f32 = 1.797;
const VOXEL: f32 = 0.15;

struct Room {
    start_frames: Vec<SeenFrame>,
    voxels: HashSet<(i16, i16, i16)>,
}

fn f32s(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn room() -> Room {
    let d: &[u8] = include_bytes!("chima-room.bin");
    let mut at = 0;
    let mut u32_at = |at: &mut usize| {
        let v = u32::from_le_bytes([d[*at], d[*at + 1], d[*at + 2], d[*at + 3]]);
        *at += 4;
        v
    };
    let mut lists = Vec::new();
    for _ in 0..2 {
        let n = u32_at(&mut at);
        let mut frames = Vec::new();
        for _ in 0..n {
            let m: [f32; 12] = f32s(&d[at..at + 48]).try_into().unwrap();
            at += 48;
            let p: [f32; 4] = f32s(&d[at..at + 16]).try_into().unwrap();
            at += 16;
            let bins = d[at..at + 2 * BINS_X * BINS_Y]
                .chunks_exact(2)
                .map(|c| match u16::from_le_bytes([c[0], c[1]]) {
                    0 => f32::NAN,
                    65535 => f32::INFINITY,
                    cm => cm as f32 / 100.0,
                })
                .collect();
            at += 2 * BINS_X * BINS_Y;
            frames.push(SeenFrame::from_bins(&m, &p, bins));
        }
        lists.push(frames);
    }
    let n = u32_at(&mut at) as usize;
    let voxels = d[at..at + 6 * n]
        .chunks_exact(6)
        .map(|c| (i16::from_le_bytes([c[0], c[1]]), i16::from_le_bytes([c[2], c[3]]), i16::from_le_bytes([c[4], c[5]])))
        .collect();
    Room { start_frames: lists.swap_remove(0).into_iter().take(4).collect(), voxels }
}

fn voxel(p: V) -> (i16, i16, i16) {
    ((p[0] / VOXEL).floor() as i16, (p[1] / VOXEL).floor() as i16, (p[2] / VOXEL).floor() as i16)
}

fn matrix(p: &Pose) -> [f32; 12] {
    let [r, u, f] = axes(p);
    [r[0], u[0], f[0], p[0], r[1], u[1], f[1], p[1], r[2], u[2], f[2], p[2]]
}

/// A depth frame the capture would have recorded at a pose: rays through each bin's middle, stopping at the first voxel.
fn draw_frame(voxels: &HashSet<(i16, i16, i16)>, p: &Pose) -> SeenFrame {
    let [r, u, f] = axes(p);
    let eye = place(p);
    let mut bins = vec![f32::INFINITY; BINS_X * BINS_Y];
    for by in 0..BINS_Y {
        for bx in 0..BINS_X {
            let (x, y) = ((((bx as f32 + 0.5) / BINS_X as f32) * 2.0 - 1.0) / P00, (1.0 - ((by as f32 + 0.5) / BINS_Y as f32) * 2.0) / P11);
            let d: V = std::array::from_fn(|k| f[k] + r[k] * x + u[k] * y);
            let mut z = 0.1;
            while z < 25.0 {
                if voxels.contains(&voxel(std::array::from_fn(|k| eye[k] + d[k] * z))) {
                    bins[by * BINS_X + bx] = z;
                    break;
                }
                z += 0.1;
            }
        }
    }
    SeenFrame::from_bins(&matrix(p), &[P00, P11, 0.0, 0.0], bins)
}

/// What the flight knows after its opening turn: the capture's real frames at the start, then the turn's frames.
fn after_look(room: &Room, start: Pose) -> Seen {
    let mut seen = Seen::default();
    for f in &room.start_frames {
        seen.add(f.clone());
    }
    // About one frame in four of 37 a second.
    for p in look(start, RATE).iter().step_by(11) {
        seen.add(draw_frame(&room.voxels, p));
    }
    seen
}

/// Flies a whole flight, a piece at a time, each piece adding the depth frames it would have shown.
fn fly(room: &Room, start: Pose, length: Photo3dLength, people: &[Person], seen: &mut Seen) -> Vec<Piece> {
    fly_for(room, start, length, PictureMode::People, people, seen)
}

fn fly_for(room: &Room, start: Pose, length: Photo3dLength, mode: PictureMode, people: &[Person], seen: &mut Seen) -> Vec<Piece> {
    let mut flight = Flight::new(start, P11, RATE, length, mode, people, true);
    let mut pieces = Vec::new();
    while let Some(piece) = flight.next(seen) {
        for p in piece.poses.iter().step_by(11) {
            seen.add(draw_frame(&room.voxels, p));
        }
        pieces.push(piece);
    }
    pieces
}

fn start_people(seen: &Seen) -> Vec<Person> {
    people(START, Framing { width: 1920, height: 1080, p00: P00, p11: P11 }, &[FACE], Some(HIPS), Some(EYE_HEIGHT), seen)
}

/// Whether a pose stands inside something real (the person's own voxels aside): a wall or furniture goes on below a place, a
/// door's lintel, passed under in an opening the frames did see through, does not, however coarse these voxels are.
fn in_something(voxels: &HashSet<(i16, i16, i16)>, p: V, person: V) -> bool {
    if flat(p, person) < 0.6 {
        return false;
    }
    let (x, y, z) = voxel(p);
    voxels.contains(&(x, y, z)) && voxels.contains(&(x, y - 1, z))
}

/// The pixel a world point falls on at a pose, with its eye depth.
fn project(p: &Pose, world: V) -> Option<(f32, f32, f32)> {
    let [r, u, f] = axes(p);
    let d = sub(world, place(p));
    let z = dot(d, f);
    (z > 0.1).then(|| (960.0 + 960.0 * P00 * dot(d, r) / z, 540.0 - 540.0 * P11 * dot(d, u) / z, z))
}

/// The poses a capture would keep as keyframes, one frame every 27 ms as in the flights of 2026-11-04, every fourth a depth
/// frame.
fn kept(pieces: &[Piece]) -> Vec<(Pose, bool)> {
    let mut selector = KeyframeSelector::new(KeyframeSettings::default(), false);
    let mut out = Vec::new();
    let all: Vec<(Pose, bool)> = pieces.iter().flat_map(|piece| piece.poses.iter().map(move |p| (*p, piece.rig))).collect();
    let (mut t, mut frame) = (0.0f32, 0u32);
    while ((t * RATE as f32) as usize) < all.len() {
        let (p, rig) = all[(t * RATE as f32) as usize];
        if frame % 4 != 0 && selector.consider_view(&matrix(&p), P11, t) {
            out.push((p, rig));
        }
        t += 0.027;
        frame += 1;
    }
    out
}

#[test]
fn the_real_picture_finds_its_one_person_where_he_stood() {
    let room = room();
    let seen = after_look(&room, START);
    let people = start_people(&seen);
    assert_eq!(people.len(), 1, "{people:?}");
    let head = people[0].head;
    assert!(flat(head, HIPS) < 0.25, "over his hips: {head:?}");
    assert!((head[1] - (HIPS[1] + 0.38 * EYE_HEIGHT)).abs() < 0.25, "{head:?}");
    assert!((people[0].ground - (HIPS[1] - 0.62 * EYE_HEIGHT)).abs() < 1e-3, "the wearer's own ground");
    assert!((Person { head, ground: people[0].ground }.height() - 2.0).abs() < 0.3, "{}", people[0].height());
}

#[test]
fn the_flight_goes_around_him_enters_no_wall_and_comes_home() {
    let room = room();
    for length in [Photo3dLength::Standard, Photo3dLength::Full] {
        let mut seen = after_look(&room, START);
        let people = start_people(&seen);
        let head = people[0].head;
        let pieces = fly(&room, START, length, &people, &mut seen);
        let poses: Vec<Pose> = pieces.iter().flat_map(|p| p.poses.clone()).collect();

        // How long: the picture's length (the opening turn comes before these).
        let seconds = poses.len() as f32 / RATE as f32;
        let wanted = if length == Photo3dLength::Full { 8.0 } else { 4.0 } - LOOK_S;
        assert!(seconds <= wanted + 0.2 && seconds >= 0.7 * wanted, "{length:?}: {seconds} s of {wanted}");

        // No jump between poses (the poses go out 90 a second), inside nothing real.
        let mut from = START;
        for p in &poses {
            let d = length_of(p, &from);
            assert!(d < 0.7 && (p[4] - from[4]).abs() < 30.0 && (p[3] - from[3]).abs() < 30.0, "{from:?} -> {p:?}");
            from = *p;
        }
        let inside: Vec<&Pose> = poses.iter().filter(|p| in_something(&room.voxels, place(p), head)).collect();
        assert!(inside.is_empty(), "{length:?}: {} of {} poses inside something: {:?}", inside.len(), poses.len(), inside.first());

        // Home: the very pose it started from, some turns later. Nothing here can touch the zoom: a pose has none.
        let last = poses.last().unwrap();
        for k in [0, 1, 2, 3, 5] {
            assert!((last[k] - START[k]).abs() < 1e-3, "{last:?}");
        }
        assert!(((last[4] - START[4]) / 360.0 - ((last[4] - START[4]) / 360.0).round()).abs() < 1e-3, "{last:?}");

        // Rig off around people, on for the room, and the room's hops at the end.
        let first_rig = pieces.iter().position(|p| p.rig).unwrap();
        assert!(pieces[first_rig..].iter().all(|p| p.rig) && first_rig >= 2, "{first_rig} of {}", pieces.len());
    }
}

fn length_of(a: &Pose, b: &Pose) -> f32 {
    length(sub(place(a), place(b)))
}

#[test]
fn he_is_seen_whole_from_every_side_at_two_heights() {
    let room = room();
    let mut seen = after_look(&room, START);
    let people = start_people(&seen);
    let person = people[0];
    let pieces = fly(&room, START, Photo3dLength::Standard, &people, &mut seen);
    let around: Vec<Piece> = pieces.iter().filter(|p| !p.rig).cloned().collect();
    assert_eq!(around.len(), 2, "whole body and close");
    let views = kept(&around);
    assert!(views.len() >= 40, "{} keyframes around him", views.len());
    // 30° bins of azimuth around him: what the kept views show of him, in the part of the frame training keeps.
    let az0 = (START[0] - HIPS[0]).atan2(START[2] - HIPS[2]).to_degrees();
    let (mut head, mut feet, mut close, mut pixels) = ([false; 12], [false; 12], [false; 12], 0.0f32);
    for (v, _) in &views {
        let az = ((v[0] - HIPS[0]).atan2(v[2] - HIPS[2]).to_degrees() - az0).rem_euclid(360.0);
        let bin = (az / 30.0) as usize % 12;
        let top = project(v, [HIPS[0], person.head[1] + CROWN, HIPS[2]]).unwrap();
        let bottom = project(v, [HIPS[0], person.ground + 0.1, HIPS[2]]).unwrap();
        assert!((0.0..1920.0).contains(&top.0), "he is in the frame from {v:?}: {top:?}");
        head[bin] |= (200.0..1050.0).contains(&top.1);
        feet[bin] |= (200.0..1060.0).contains(&bottom.1);
        if top.2 < 2.0 {
            close[bin] = true;
            pixels = pixels.max(1234.0 / top.2);
        }
    }
    let count = |bins: &[bool; 12]| bins.iter().filter(|b| **b).count();
    assert!(count(&head) >= 11, "the head from nearly every 30° of the way round: {head:?}");
    assert!(count(&feet) >= 7, "the feet from over half, however tight the room: {feet:?}");
    assert!(count(&close) >= 10, "close from most: {close:?}");
    assert!(pixels > 700.0, "at {pixels} pixels a metre on the head; 473 in the start frame");
}

#[test]
fn a_person_at_a_wall_gets_the_arc_in_front_there_and_back() {
    // A wall of voxels at z = 1 across a floor, a person half a metre from it; everything else is open.
    let mut voxels = HashSet::new();
    for x in -80..80i16 {
        for y in -1..30i16 {
            voxels.insert((x, y, voxel([0.0, 0.0, 1.0]).2));
        }
    }
    for x in -80..80i16 {
        for z in -80..7i16 {
            voxels.insert((x, -1, z));
        }
    }
    let room = Room { start_frames: Vec::new(), voxels };
    let start: Pose = [0.0, 1.6, -3.0, 5.0, 0.0, 0.0];
    let person = Person { head: [0.0, 1.5, 0.5], ground: 0.0 };
    let mut seen = after_look(&room, start);
    let pieces = fly(&room, start, Photo3dLength::Standard, &[person], &mut seen);
    let poses: Vec<Pose> = pieces.iter().flat_map(|p| p.poses.clone()).collect();
    assert!(poses.iter().all(|p| p[2] < 1.0 - 0.2), "nothing beyond the wall: {:?}", poses.iter().map(|p| p[2]).fold(f32::MIN, f32::max));
    // Both ways from the front, well past the sides of what the start sees.
    let azimuths: Vec<f32> = pieces[0].poses.iter().map(|p| (p[0] - person.head[0]).atan2(person.head[2] - p[2]).to_degrees()).collect();
    let (least, most) = (azimuths.iter().copied().fold(f32::MAX, f32::min), azimuths.iter().copied().fold(f32::MIN, f32::max));
    assert!(least < -50.0 && most > 50.0, "{least}..{most}");
    let last = poses.last().unwrap();
    assert!(length_of(last, &start) < 1e-3, "home: {last:?}");
}

#[test]
fn what_no_depth_has_seen_is_not_flown_to() {
    let seen = Seen::default();
    let person = Person { head: [0.0, 1.5, 3.0], ground: 0.0 };
    let start: Pose = [0.0, 1.6, 0.0, 0.0, 0.0, 0.0];
    let pieces = plan(start, P11, RATE, Photo3dLength::Standard, &[person], &seen);
    // No sweep, no hop: only the way home from where it stands.
    assert_eq!(pieces.len(), 1);
    assert!(pieces[0].rig);
    assert!(pieces[0].poses.iter().all(|p| length_of(p, &start) < 1e-3), "{:?}", pieces[0].poses.first());
}

#[test]
fn a_poster_is_nobody_and_a_small_or_weak_face_too() {
    // A flat wall 3 m ahead with a face on it.
    let mut voxels = HashSet::new();
    for x in -40..40i16 {
        for y in -10..40i16 {
            voxels.insert((x, y, voxel([0.0, 0.0, 3.0]).2));
        }
    }
    let start: Pose = [0.0, 1.5, 0.0, 0.0, 0.0, 0.0];
    let seen = Seen { frames: vec![draw_frame(&voxels, &start)] , ..Seen::default() };
    let framing = Framing { width: 1920, height: 1080, p00: P00, p11: P11 };
    let poster = [860.0, 440.0, 1060.0, 640.0, 0.84];
    assert!(people(start, framing, &[poster], None, None, &seen).is_empty(), "flat on the wall");
    assert!(
        people(start, framing, &[[100.0, 100.0, 130.0, 130.0, 0.9], [100.0, 300.0, 300.0, 500.0, 0.3]], None, None, &Seen::default()).is_empty(),
        "small, weak"
    );
    // The same box with nothing known of depth: by its width, a head of 25 cm.
    let found = people(start, framing, &[poster], None, None, &Seen::default());
    assert_eq!(found.len(), 1);
    assert!((found[0].head[2] - 0.25 * P00 * 960.0 / 200.0).abs() < 0.05, "{found:?}");
    assert!((found[0].ground - (found[0].head[1] - HEAD_ABOVE_GROUND)).abs() < 1e-3);
}

#[test]
fn people_together_are_one_subject_and_time_is_shared_out() {
    let a = Person { head: [0.0, 1.5, 0.0], ground: 0.0 };
    let b = Person { head: [0.8, 1.6, 0.0], ground: 0.0 };
    let c = Person { head: [5.0, 1.5, 0.0], ground: 0.0 };
    let g = groups(&[a, b, c]);
    assert_eq!(g.len(), 2);
    assert!((g[0].spread - 0.4).abs() < 1e-3 && (g[0].height - 1.8).abs() < 1e-3);
    // Every second of the picture is spent.
    for (length, total) in [(Photo3dLength::Standard, 4.0), (Photo3dLength::Full, 8.0)] {
        for n in 0..4 {
            let s = Split::of(length, PictureMode::People, n);
            assert!((s.people + s.room + LOOK_S - total).abs() < 1e-4, "{length:?}, {n}: {s:?}");
            assert_eq!(s.people == 0.0, n == 0);
        }
    }
    assert_eq!(sweeps_of(2.2).len(), 2, "one person in a 4 s picture");
    assert_eq!(sweeps_of(4.0).len(), 3, "one person in an 8 s picture");
    assert_eq!(sweeps_of(2.2 / 3.0).len(), 1, "three people in a 4 s picture");
    assert!(sweeps_of(2.2).iter().map(|s| s.1).sum::<f32>() - 2.2 < 1e-4);
}

/// The way a flight gets its depth while recording: a real depth file and record, a record half written, and again.
#[test]
fn depth_frames_are_read_as_they_are_written() {
    use orbiters_contracts::frame_record::FrameRecord;
    let dir = std::env::temp_dir().join(format!("orbiters-around-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join(DEPTH_DIR)).unwrap();
    let layout = DepthLayout::new(1920, 1080).unwrap();
    let (gw, gh) = layout.grid_size();
    // A wall 3 m ahead on the whole grid; the strip's cells read nothing.
    let steps = layout.grid_steps();
    let codes: Vec<u16> = steps.iter().map(|s| if *s == 0 { depth::UNKNOWN } else { depth::quantize(3.0) }).collect();
    std::fs::write(dir.join(DEPTH_DIR).join(orbiters_core::bundle::depth_file(0)), depth_file::write(gw, gh, 1920, 1080, &codes)).unwrap();
    let pose: Pose = [0.0, 1.5, 0.0, 0.0, 0.0, 0.0];
    let record = |flags: u16, index: i32| FrameRecord {
        flags,
        width: 1920,
        height: 1080,
        depth_index: index,
        camera_to_world: matrix(&pose),
        projection: [P00, P11, 0.0, 0.0],
        ..Default::default()
    };
    let file = dir.join(FRAMES_FILE);
    let colour = record(record_flags::STRIP_OK, -1).to_bytes();
    let mut bytes = colour.to_vec();
    bytes.extend_from_slice(&record(record_flags::STRIP_OK | record_flags::DEPTH, 0).to_bytes()[..100]);
    std::fs::write(&file, &bytes).unwrap();
    let (mut live, mut seen) = (LiveDepth::new(&dir), Seen::default());
    live.poll(&mut seen);
    assert!(seen.frames.is_empty(), "half a record waits");
    let mut bytes = colour.to_vec();
    bytes.extend_from_slice(&record(record_flags::STRIP_OK | record_flags::DEPTH, 0).to_bytes());
    std::fs::write(&file, &bytes).unwrap();
    live.poll(&mut seen);
    assert_eq!(seen.frames.len(), 1);
    assert_eq!(seen.at([0.0, 1.5, 1.5], EYE_MARGIN), Clear::Free);
    assert_eq!(seen.at([0.0, 1.5, 3.0], EYE_MARGIN), Clear::Surface);
    assert_eq!(seen.at([0.0, 1.5, 4.0], EYE_MARGIN), Clear::Unknown);
    live.poll(&mut seen);
    assert_eq!(seen.frames.len(), 1, "read once");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_depth_frame_says_free_surface_or_nothing() {
    let mut voxels = HashSet::new();
    voxels.insert(voxel([0.0, 1.5, 3.0]));
    let start: Pose = [0.0, 1.5, 0.0, 0.0, 0.0, 0.0];
    let f = draw_frame(&voxels, &start);
    assert_eq!(f.at([0.0, 1.5, 1.5], 0.2), Clear::Free, "before the surface");
    assert_eq!(f.at([0.0, 1.5, 3.0], 0.2), Clear::Surface);
    assert_eq!(f.at([0.0, 1.5, 4.5], 0.2), Clear::Unknown, "behind it");
    assert_eq!(f.at([0.0, 1.5, -1.0], 0.2), Clear::Unknown, "behind the camera");
    assert_eq!(f.at([50.0, 1.5, 1.0], 0.2), Clear::Unknown, "outside the frame");
    let seen = Seen { frames: vec![f.clone(), draw_frame(&voxels, &[0.0, 1.5, 5.0, 0.0, 180.0, 0.0])] , ..Seen::default() };
    assert_eq!(seen.at([0.0, 1.5, 3.0], 0.2), Clear::Surface, "a surface wins");
    assert_eq!(seen.at([0.0, 1.5, 4.5], 0.2), Clear::Free, "the other side sees it");
}

/// Writes the planned flights for the report's images: `cargo test dump_flight -- --ignored`, `ORBITERS_FLIGHT_DUMP=<file>`.
#[test]
#[ignore]
fn dump_flight() {
    let room = room();
    let out = std::env::var("ORBITERS_FLIGHT_DUMP").expect("ORBITERS_FLIGHT_DUMP");
    let mut all = serde_json::Map::new();
    for (name, length, mode) in [
        ("people-4s", Photo3dLength::Standard, PictureMode::People),
        ("world-4s", Photo3dLength::Standard, PictureMode::World),
        ("people-8s", Photo3dLength::Full, PictureMode::People),
        ("world-8s", Photo3dLength::Full, PictureMode::World),
    ] {
        let mut seen = after_look(&room, START);
        let people = start_people(&seen);
        let lookp = look(START, RATE);
        let pieces = fly_for(&room, START, length, mode, &people, &mut seen);
        let mut list = vec![serde_json::json!({"rig": false, "kind": "look", "poses": lookp})];
        list.extend(pieces.iter().map(|p| serde_json::json!({"rig": p.rig, "poses": p.poses})));
        all.insert(
            name.to_string(),
            serde_json::json!({"people": people.iter().map(|p| serde_json::json!({"head": p.head, "ground": p.ground})).collect::<Vec<_>>(), "pieces": list}),
        );
    }
    all.insert("voxels".to_string(), serde_json::json!(room.voxels.iter().map(|v| [v.0, v.1, v.2]).collect::<Vec<_>>()));
    std::fs::write(out, serde_json::to_string(&all).unwrap()).unwrap();
}

fn seconds_of(pieces: &[Piece]) -> f32 {
    pieces.iter().map(|p| p.poses.len()).sum::<usize>() as f32 / RATE as f32
}

fn poses_of(pieces: &[Piece]) -> Vec<Pose> {
    pieces.iter().flat_map(|p| p.poses.clone()).collect()
}

fn ends_home(poses: &[Pose], start: Pose) {
    let last = poses.last().expect("a flight");
    for k in [0, 1, 2, 3, 5] {
        assert!((last[k] - start[k]).abs() < 1e-3, "{last:?}");
    }
    assert!(((last[4] - start[4]) / 360.0 - ((last[4] - start[4]) / 360.0).round()).abs() < 1e-3, "{last:?}");
}

/// How much of the place the depth frames have seen: voxels of 15 cm with a surface measured in them.
fn surface_seen(seen: &Seen) -> usize {
    seen.frames.iter().flat_map(|f| (0..BINS_X * BINS_Y).filter_map(|bin| f.surface_point(bin))).map(voxel).collect::<HashSet<_>>().len()
}

/// What the rig's faces add to what the main views saw, in voxels of surface: the six axes around each place, about every other
/// colour frame where the rig is on. Its faces are wider than the main view drawn here (90° against 47°), so this counts low.
fn surface_with_rig(room: &Room, pieces: &[Piece], seen: &Seen) -> usize {
    let mut all = seen.clone();
    for piece in pieces.iter().filter(|p| p.rig) {
        for p in piece.poses.iter().step_by(22) {
            for (pitch, yaw) in [(0.0, 0.0), (0.0, 90.0), (0.0, 180.0), (0.0, 270.0), (85.0, 0.0), (-85.0, 0.0)] {
                all.frames.push(draw_frame(&room.voxels, &[p[0], p[1], p[2], pitch, yaw, 0.0]));
            }
        }
    }
    surface_seen(&all)
}

#[test]
fn the_seconds_go_where_the_picture_is_of() {
    let split = |length, mode, groups| Split::of(length, mode, groups);
    use Photo3dLength::{Full, Standard};
    use PictureMode::{People, World};
    assert_eq!(split(Standard, People, 1), Split { people: 2.9, room: 4.0 - LOOK_S - 2.9 });
    assert_eq!(split(Full, People, 2), Split { people: 5.8, room: 8.0 - LOOK_S - 5.8 });
    // Nobody in a picture of people: its seconds are the room's, as in a picture of the world.
    assert_eq!(split(Standard, People, 0), split(Standard, World, 0));
    for groups in 0..4 {
        assert_eq!(split(Standard, World, groups), Split { people: 0.0, room: 4.0 - LOOK_S });
        assert_eq!(split(Full, World, groups), Split { people: 0.0, room: 8.0 - LOOK_S });
    }
    assert_eq!(split(Photo3dLength::Quick, People, 1).people, 0.0);
}

/// World mode over the real room: nobody is flown around whoever stands there, no wall is entered, and it ends on the very pose.
#[test]
fn a_wearer_who_will_be_taken_out_stands_in_no_flights_way() {
    let room = room();
    let frame = room.start_frames[0].clone();
    // A point this frame measured a surface on, and a wearer standing right there.
    let (bin, point) = (0..frame.bins.len()).find_map(|bin| frame.surface_point(bin).map(|p| (bin, p))).expect("a surface");
    let wearer = LeftOut { hips: point, eye_height: EYE_HEIGHT };
    let mut seen = Seen::default();
    seen.add(frame.clone());
    assert_eq!(seen.at(point, FLIGHT_MARGIN), Clear::Surface, "as an obstacle");
    let mut without = Seen { left_out: Some(wearer), ..Seen::default() };
    without.add(frame.clone());
    assert!(without.frames[0].bins[bin].is_nan(), "that bin read nothing now");
    assert_ne!(without.at(point, 0.01), Clear::Surface, "no obstacle");
    // Everything else the frame saw stays.
    let (mut kept, mut gone) = (0, 0);
    for b in 0..frame.bins.len() {
        let Some(p) = frame.surface_point(b) else { continue };
        if flat(p, point) > LeftOut::RADIUS * EYE_HEIGHT {
            kept += usize::from(without.frames[0].bins[b] == frame.bins[b]);
            gone += usize::from(without.frames[0].bins[b] != frame.bins[b]);
        }
    }
    assert!(kept > 100, "{kept}");
    assert_eq!(gone, 0);
}

#[test]
fn a_picture_of_the_world_goes_around_nobody_enters_no_wall_and_comes_home() {
    let room = room();
    for length in [Photo3dLength::Standard, Photo3dLength::Full] {
        let mut seen = after_look(&room, START);
        let people = start_people(&seen);
        let head = people[0].head;
        let pieces = fly_for(&room, START, length, PictureMode::World, &people, &mut seen);
        let poses = poses_of(&pieces);
        assert!(pieces.iter().all(|p| p.rig), "the rig all the way, alone in the instance");
        let wanted = if length == Photo3dLength::Full { 8.0 } else { 4.0 } - LOOK_S;
        assert!(seconds_of(&pieces) <= wanted + 0.2 && seconds_of(&pieces) >= 0.7 * wanted, "{length:?}: {} s of {wanted}", seconds_of(&pieces));
        let mut from = START;
        for p in &poses {
            assert!(length_of(p, &from) < 0.7 && (p[4] - from[4]).abs() < 30.0 && (p[3] - from[3]).abs() < 30.0, "{from:?} -> {p:?}");
            from = *p;
        }
        let inside: Vec<&Pose> = poses.iter().filter(|p| in_something(&room.voxels, place(p), head)).collect();
        assert!(inside.is_empty(), "{length:?}: {} of {} poses inside something: {:?}", inside.len(), poses.len(), inside.first());
        ends_home(&poses, START);
    }
}

/// The main views alone see about as much of this tight room either way: a sweep around a person looks across it too. What World
/// adds is the rig all the way, and nobody in the picture; what People adds is the seconds around them.
#[test]
fn a_picture_of_the_world_sees_more_of_the_place_and_one_of_people_more_of_them() {
    let room = room();
    for length in [Photo3dLength::Standard, Photo3dLength::Full] {
        let mut seen = after_look(&room, START);
        let people = start_people(&seen);
        let them = fly_for(&room, START, length, PictureMode::People, &people, &mut seen);
        let of_them = (surface_seen(&seen), surface_with_rig(&room, &them, &seen));
        let mut seen = after_look(&room, START);
        let world = fly_for(&room, START, length, PictureMode::World, &people, &mut seen);
        let of_world = (surface_seen(&seen), surface_with_rig(&room, &world, &seen));
        let around = |pieces: &[Piece]| pieces.iter().filter(|p| !p.rig).map(|p| p.poses.len()).sum::<usize>() as f32 / RATE as f32;
        let wanted = if length == Photo3dLength::Full { 4.0 } else { 2.2 };
        assert!(around(&them) > wanted, "{length:?}: {} s around him, more than the {wanted} s of one split for all", around(&them));
        assert_eq!(around(&world), 0.0);
        println!("{length:?}: voxels of surface seen, main views and with the rig: {of_world:?} in World, {of_them:?} in People");
        assert!(of_world.0 * 10 > of_them.0 * 9, "{length:?}: {of_world:?} in World, {of_them:?} in People");
        assert!(of_world.1 > of_them.1, "{length:?}: {of_world:?} in World, {of_them:?} in People");
    }
}

/// Among other players the rig stays off: its cameras have no mask for them. Nothing else changes.
#[test]
fn among_other_players_a_picture_of_the_world_flies_without_the_rig() {
    let room = room();
    let mut seen = after_look(&room, START);
    let mut flight = Flight::new(START, P11, RATE, Photo3dLength::Standard, PictureMode::World, &[], false);
    let mut pieces = Vec::new();
    while let Some(piece) = flight.next(&seen) {
        for p in piece.poses.iter().step_by(11) {
            seen.add(draw_frame(&room.voxels, p));
        }
        pieces.push(piece);
    }
    assert!(pieces.iter().all(|p| !p.rig));
    ends_home(&poses_of(&pieces), START);
}

#[test]
fn a_picture_of_the_world_flies_nowhere_no_depth_has_seen() {
    let start: Pose = [0.0, 1.6, 0.0, 0.0, 0.0, 0.0];
    let mut flight = Flight::new(start, P11, RATE, Photo3dLength::Full, PictureMode::World, &[], true);
    let pieces: Vec<Piece> = std::iter::from_fn(|| flight.next(&Seen::default())).collect();
    assert!(pieces.iter().flat_map(|p| &p.poses).all(|p| length_of(p, &start) < 1e-3), "it only turns where it stands");
}

/// What a recording of Enzo's says: its depth frames, what stood where, and the wearer.
struct Recorded {
    room: Room,
    start: Pose,
    hips: Option<V>,
    eye_height: Option<f32>,
}

fn recorded(dir: &Path) -> Option<Recorded> {
    use orbiters_contracts::performance::{self, parameters, skeleton_frames};
    let bytes = std::fs::read(dir.join(FRAMES_FILE)).ok()?;
    let (mut start, mut first_ns, mut layout, mut moved) = (None::<Pose>, None, None::<DepthLayout>, false);
    let mut start_frames = Vec::new();
    let mut voxels = HashSet::new();
    for chunk in bytes.chunks_exact(RECORD_SIZE) {
        let Ok(r) = FrameRecord::from_bytes(chunk) else { continue };
        if !r.has(record_flags::STRIP_OK) {
            continue;
        }
        let m = r.camera_to_world;
        if start.is_none() {
            let (f, flat) = ([m[2], m[6], m[10]], (m[2] * m[2] + m[10] * m[10]).sqrt());
            start = Some([m[3], m[7], m[11], (-f[1]).atan2(flat).to_degrees(), f[0].atan2(f[2]).to_degrees(), 0.0]);
            first_ns = Some(r.capture_time_ns);
        }
        if !r.has(record_flags::DEPTH) || r.depth_index < 0 {
            continue;
        }
        let Ok(data) = std::fs::read(dir.join(DEPTH_DIR).join(orbiters_core::bundle::depth_file(r.depth_index))) else { continue };
        let Ok(file) = depth_file::read_for_frame(&data, r.width, r.height) else { continue };
        if layout.as_ref().is_none_or(|l| (l.width, l.height) != (r.width, r.height)) {
            layout = DepthLayout::new(r.width, r.height);
        }
        let Some(frame) = layout.as_ref().and_then(|l| SeenFrame::new(&m, &r.projection, l, &file.codes)) else { continue };
        for bin in 0..BINS_X * BINS_Y {
            if let Some(p) = frame.surface_point(bin) {
                voxels.insert(voxel(p));
            }
        }
        // What this flight knew after its opening turn: what the camera saw from where it started, up to its first move.
        if !moved && length(sub(frame.eye, place(&start.unwrap_or_default()))) < 0.05 {
            start_frames.push(frame);
        } else {
            moved = true;
        }
    }
    let t0 = first_ns?;
    let hips = skeleton_frames::read(&std::fs::read(dir.join(performance::SKELETON_FILE)).ok()?).ok().and_then(|frames| {
        frames.iter().filter(|f| f.hips.iter().all(|v| v.is_finite())).min_by_key(|f| f.time_ns.abs_diff(t0)).map(|f| f.hips)
    });
    let table: parameters::Table = serde_json::from_slice(&std::fs::read(dir.join(performance::PARAMETERS_TABLE)).ok()?).ok()?;
    let eye = table.parameters.iter().find(|p| p.name == "/avatar/eyeheight").map(|p| p.id);
    let changes = parameters::read(&std::fs::read(dir.join(performance::PARAMETERS_FILE)).ok()?).ok()?;
    let eye_height = eye.and_then(|id| changes.iter().find(|c| c.id == id).map(|c| c.value.as_f32()));
    Some(Recorded { room: Room { start_frames, voxels }, start: start?, hips, eye_height })
}

/// Both pictures over Enzo's own recordings of 2026-11-04: his Documents folder, so not a regular test. `cargo test -- --ignored
/// flights_over_his_recordings --nocapture` prints reports/capture-modes.md's table. Each recording's own depth frames say what
/// stands where; what they never saw counts as free, so entering a wall here means entering one a depth frame saw.
#[test]
#[ignore = "reads Enzo's recordings"]
fn flights_over_his_recordings() {
    let sessions = Settings::default().sessions_dir;
    let mut names: Vec<String> = std::fs::read_dir(&sessions).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.contains("2026-11-04")).collect();
    names.sort();
    println!("| Recording | Length | Picture | Depth frames at the look | Seconds | Around people | Surface voxels seen | With the rig | Rig | Inside something | Home |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    let mut flown = 0;
    let only = std::env::var("ORBITERS_TEST_SESSION").ok();
    for name in names {
        if only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
            continue;
        }
        let Some(rec) = recorded(&sessions.join(&name)) else { continue };
        let framing = Framing { width: 1920, height: 1080, p00: P00, p11: P11 };
        for length in [Photo3dLength::Standard, Photo3dLength::Full] {
            for mode in [PictureMode::People, PictureMode::World] {
                let mut seen = Seen::default();
                for f in &rec.room.start_frames {
                    seen.add(f.clone());
                }
                if seen.frames.len() < 3 {
                    println!("| {name} | {length:?} | {mode:?} | {} | blind flight: under 3 depth frames | | | | | |", seen.frames.len());
                    continue;
                }
                let at_look = seen.frames.len();
                let people = people(rec.start, framing, &[], rec.hips, rec.eye_height, &seen);
                let pieces = fly_for(&rec.room, rec.start, length, mode, &people, &mut seen);
                let poses = poses_of(&pieces);
                let head = people.first().map_or([f32::MAX; 3], |p| p.head);
                let inside = poses.iter().filter(|p| in_something(&rec.room.voxels, place(p), head)).count();
                let around = pieces.iter().filter(|p| !p.rig).map(|p| p.poses.len()).sum::<usize>() as f32 / RATE as f32;
                println!("| {name} | {length:?} | {mode:?} | {at_look} | {:.2} | {around:.2} | {} | {} | {} | {inside} | yes |", seconds_of(&pieces), surface_seen(&seen), surface_with_rig(&rec.room, &pieces, &seen), pieces.iter().any(|p| p.rig));
                ends_home(&poses, rec.start);
                let wanted = if length == Photo3dLength::Full { 8.0 } else { 4.0 } - LOOK_S;
                assert!(seconds_of(&pieces) <= wanted + 0.7, "{name} {length:?} {mode:?}: {} s", seconds_of(&pieces));
                if mode == PictureMode::World {
                    assert_eq!(around, 0.0, "{name}: nobody flown around");
                }
                flown += 1;
            }
        }
    }
    assert!(flown > 0);
}
