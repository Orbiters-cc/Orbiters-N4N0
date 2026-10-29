//! Where the ground was under the person who captured, and how high their eyes were above it: what a 3D view's start
//! eye stands on, so the ground of every capture meets a viewer's own (OrbitersContracts Docs/capture-sharing.md, the
//! start view). A recorded camera can be anywhere (held low, raised, flown around the avatar by N4N0), so its height
//! says nothing of the ground.
//!
//! The person: the hips VRChat gave the avatar at that moment (`skeleton.bin`, from the capture kit's band), else the
//! camera. The ground: the most common height of the flat, level, opaque splats below them, in the smallest disc
//! around them that holds enough (the floor around the feet; the avatar's own body, reconstructed when it stood still,
//! is upright and rarely level). The eye height: the avatar's, as VRChat reported it over OSC (`EyeHeightAsMeters`,
//! `/avatar/eyeheight`, with the avatar's scale), at that moment.

use crate::gaussians::{rotation_matrix, sigmoid, Gaussians};
use orbiters_contracts::performance::{self, parameters, skeleton_frames};
use std::path::Path;

/// The eye height a view uses when the session does not say: about VRChat's default avatar.
pub const EYE_HEIGHT: f32 = 1.6;
/// Discs around the person searched in turn (metres), and the flat splats one needs to be believed.
const DISCS: [f32; 4] = [0.8, 1.5, 3.0, 6.0];
const ENOUGH: usize = 24;
/// Heights searched below the hips (a sitting avatar's hips are a quarter metre up) or the camera, metres.
const BELOW_HIPS: (f32, f32) = (0.05, 3.0);
const BELOW_CAMERA: (f32, f32) = (0.2, 3.0);
/// Histogram bins of the heights, metres.
const BIN: f32 = 0.025;
/// Without a floor in the splats, the hips stand this many eye heights above the ground (standing avatars measured
/// 0.6 to 0.7 on four captures).
const HIPS_PER_EYE_HEIGHT: f32 = 0.62;

/// What the ground was found from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Flat splats under the avatar's hips.
    Hips,
    /// Flat splats under the recorded camera (no capture kit's band in the session).
    Camera,
    /// The hips, the avatar's eye height and how high hips usually stand: no flat splats under them.
    HipsOnly,
    /// Nothing: the camera stood at eye height.
    Guess,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ground {
    /// Unity world height of the ground (the same in the right-handed files).
    pub floor: f32,
    /// Metres from the ground to the eyes.
    pub eye_height: f32,
    pub source: Source,
}

impl Ground {
    /// Eye height above the ground at `p`'s place.
    pub fn eye(&self, p: [f32; 3]) -> [f32; 3] {
        [p[0], self.floor + self.eye_height, p[2]]
    }
}

/// The ground under the person at `time_ns` (the session clock; None: the start) of `session`, whose splats are `g`
/// and whose camera stood at `camera` then.
pub fn find(session: &Path, g: &Gaussians, camera: [f32; 3], time_ns: Option<u64>) -> Ground {
    let eye = eye_height(session, time_ns);
    let hips = hips(session, time_ns);
    let eye_height = eye.unwrap_or(EYE_HEIGHT);
    if let Some(h) = hips {
        if let Some(floor) = surface(g, h, BELOW_HIPS) {
            return Ground { floor, eye_height, source: Source::Hips };
        }
        return Ground { floor: h[1] - HIPS_PER_EYE_HEIGHT * eye_height, eye_height, source: Source::HipsOnly };
    }
    match surface(g, camera, BELOW_CAMERA) {
        Some(floor) => Ground { floor, eye_height, source: Source::Camera },
        None => Ground { floor: camera[1] - eye_height, eye_height, source: Source::Guess },
    }
}

/// The avatar's eye height at `time_ns`: the last value VRChat sent before it (else the first one after), in metres.
pub fn eye_height(session: &Path, time_ns: Option<u64>) -> Option<f32> {
    let table: parameters::Table = serde_json::from_str(&std::fs::read_to_string(session.join(performance::PARAMETERS_TABLE)).ok()?).ok()?;
    let changes = parameters::read(&std::fs::read(session.join(performance::PARAMETERS_FILE)).ok()?).ok()?;
    // VRChat's built-in parameter first, then the OSC address it also sends.
    ["EyeHeightAsMeters", "/avatar/eyeheight"].iter().find_map(|name| {
        let id = table.parameters.iter().find(|p| p.name == *name)?.id;
        let values: Vec<(u64, f32)> =
            changes.iter().filter(|c| c.id == id).map(|c| (c.time_ns, c.value.as_f32())).filter(|(_, v)| v.is_finite() && (0.1..=50.0).contains(v)).collect();
        let t = time_ns.unwrap_or(0);
        values.iter().rev().find(|(at, _)| *at <= t).or(values.first()).map(|(_, v)| *v)
    })
}

/// The avatar's hips (Unity world) in the capture kit's frame nearest `time_ns`.
pub fn hips(session: &Path, time_ns: Option<u64>) -> Option<[f32; 3]> {
    let frames = skeleton_frames::read(&std::fs::read(session.join(performance::SKELETON_FILE)).ok()?).ok()?;
    let t = time_ns.unwrap_or(0);
    frames.iter().filter(|f| f.hips.iter().all(|v| v.is_finite())).min_by_key(|f| f.time_ns.abs_diff(t)).map(|f| f.hips)
}

/// The height of the flat, level ground under `at`: from `below.0` to `below.1` metres below it, in the smallest of
/// [`DISCS`] around it holding [`ENOUGH`] flat splats, the most common height (the median of the splats within a bin
/// of it).
pub fn surface(g: &Gaussians, at: [f32; 3], below: (f32, f32)) -> Option<f32> {
    let (top, bottom) = (at[1] - below.0, at[1] - below.1);
    let widest = DISCS[DISCS.len() - 1];
    // (horizontal distance, height) of every flat, level, opaque splat in the widest disc.
    let flat: Vec<(f32, f32)> = (0..g.len())
        .filter_map(|i| {
            let m = g.means[i];
            if !(bottom..=top).contains(&m[1]) || g.sky.get(i).copied().unwrap_or(false) {
                return None;
            }
            let d = ((m[0] - at[0]).powi(2) + (m[2] - at[2]).powi(2)).sqrt();
            if !(d <= widest) || sigmoid(g.opacity_logits[i]) < 0.3 {
                return None;
            }
            let s = g.log_scales[i];
            let thin = (0..3).min_by(|&a, &b| s[a].total_cmp(&s[b]))?;
            let mut rest: Vec<f32> = (0..3).filter(|&k| k != thin).map(|k| s[k]).collect();
            rest.sort_by(f32::total_cmp);
            // Thinner than a third of its middle axis, at most half a metre across, its normal within 25° of vertical.
            if rest[0] - s[thin] < 3f32.ln() || rest[1].exp() > 0.5 {
                return None;
            }
            let r = rotation_matrix(g.rotations[i]);
            (r[1][thin].abs() >= 0.9).then_some((d, m[1]))
        })
        .collect();
    let disc = DISCS.iter().find(|&&r| flat.iter().filter(|(d, _)| *d <= r).count() >= ENOUGH)?;
    let heights: Vec<f32> = flat.iter().filter(|(d, _)| d <= disc).map(|(_, y)| *y).collect();
    let bins = ((top - bottom) / BIN).ceil() as usize + 1;
    let mut counts = vec![0u32; bins];
    for y in &heights {
        counts[(((y - bottom) / BIN) as usize).min(bins - 1)] += 1;
    }
    // Smoothed over neighbouring bins: a surface between two bins counts whole.
    let smooth = |i: usize| 2 * counts[i] + if i > 0 { counts[i - 1] } else { 0 } + counts.get(i + 1).copied().unwrap_or(0);
    let best = (0..bins).max_by_key(|&i| (smooth(i), i))?;
    let centre = bottom + (best as f32 + 0.5) * BIN;
    let mut near: Vec<f32> = heights.iter().copied().filter(|y| (y - centre).abs() <= 2.0 * BIN).collect();
    near.sort_by(f32::total_cmp);
    near.get(near.len() / 2).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gaussians::logit;

    /// A level disc of radius `r` at `p` (its thin axis vertical), or an upright one (thin axis along x).
    fn disc(g: &mut Gaussians, p: [f32; 3], r: f32, level: bool) {
        let s = if level { [r.ln(), (0.002f32).ln(), r.ln()] } else { [(0.002f32).ln(), r.ln(), r.ln()] };
        g.push(p, s, [1.0, 0.0, 0.0, 0.0], logit(0.9), [0.5; 3]);
    }

    fn scene() -> Gaussians {
        let mut g = Gaussians::default();
        // A floor at y = -4 around the origin, every 0.25 m out to 7 m.
        for i in -28..=28 {
            for j in -28..=28 {
                disc(&mut g, [i as f32 * 0.25, -4.0, j as f32 * 0.25], 0.15, true);
            }
        }
        // A large table top 0.75 m above it, 2 to 4 m away.
        for i in 8..16 {
            for j in -4..4 {
                disc(&mut g, [i as f32 * 0.25, -3.25, j as f32 * 0.25], 0.15, true);
            }
        }
        // The avatar standing at the origin: upright discs from the floor to its head, and a few level ones (shoulders).
        for k in 0..40 {
            disc(&mut g, [0.1, -4.0 + k as f32 * 0.04, 0.05], 0.08, false);
        }
        for k in 0..6 {
            disc(&mut g, [0.05 * k as f32, -2.6, 0.0], 0.05, true);
        }
        g
    }

    #[test]
    fn finds_the_floor_under_the_hips_past_the_avatar_and_a_table() {
        let g = scene();
        let floor = surface(&g, [0.0, -3.0, 0.0], BELOW_HIPS).unwrap();
        assert!((floor + 4.0).abs() < 0.01, "{floor}");
        // A camera held low over the table looks down on it: the table is the ground under it.
        let under_camera = surface(&g, [3.0, -2.5, 0.0], (0.2, 1.0)).unwrap();
        assert!((under_camera + 3.25).abs() < 0.01, "{under_camera}");
        // Nothing level below: none.
        assert!(surface(&g, [0.0, -4.5, 0.0], BELOW_HIPS).is_none());
    }

    #[test]
    fn the_eye_stands_at_eye_height_over_the_ground() {
        let ground = Ground { floor: -4.0, eye_height: 1.66, source: Source::Hips };
        let eye = ground.eye([2.0, -3.1, 5.0]);
        assert_eq!([eye[0], eye[2]], [2.0, 5.0]);
        assert!((eye[1] + 2.34).abs() < 1e-5);
    }

    #[test]
    fn reads_the_eye_height_and_hips_of_a_session() {
        use parameters::{Change, Kind, ParameterInfo, Table, Value};
        let dir = std::env::temp_dir().join(format!("orbiters-ground-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut table = Table::new();
        table.parameters.push(ParameterInfo {
            id: 3,
            name: "EyeHeightAsMeters".into(),
            address: "/avatar/parameters/EyeHeightAsMeters".into(),
            kind: Kind::Float,
        });
        std::fs::write(dir.join(performance::PARAMETERS_TABLE), serde_json::to_string(&table).unwrap()).unwrap();
        let mut bytes = parameters::header().to_vec();
        for (t, v) in [(0u64, 1.66f32), (5_000_000_000, 2.0)] {
            bytes.extend_from_slice(&Change { time_ns: t, id: 3, flags: 0, value: Value::Float(v) }.to_bytes());
        }
        std::fs::write(dir.join(performance::PARAMETERS_FILE), bytes).unwrap();
        assert_eq!(eye_height(&dir, Some(1_000_000_000)), Some(1.66));
        assert_eq!(eye_height(&dir, Some(6_000_000_000)), Some(2.0));
        let mut bytes = skeleton_frames::header().to_vec();
        for (t, y) in [(0u64, -3.0f32), (2_000_000_000, -2.0)] {
            let frame =
                skeleton_frames::Frame { time_ns: t, game_time: 0.0, avatar: 1, hips: [0.0, y, 0.0], rotations: [None; orbiters_contracts::skeleton::BONES] };
            bytes.extend_from_slice(&frame.to_bytes());
        }
        std::fs::write(dir.join(performance::SKELETON_FILE), bytes).unwrap();
        assert_eq!(hips(&dir, Some(1_500_000_000)).map(|h| h[1]), Some(-2.0));
        // The splats: the floor 1 m under those hips.
        let g = scene();
        let ground = find(&dir, &g, [3.0, -2.5, 0.0], Some(0));
        assert_eq!(ground.source, Source::Hips);
        assert!((ground.floor + 4.0).abs() < 0.01 && ground.eye_height == 1.66, "{ground:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
