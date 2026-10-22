//! Seeding: every exact depth sample becomes a flat disc on the surface it hit (the plan's spawning rule): normal
//! from the neighbouring samples, size from the sample's footprint, colour from the training keyframes nearest in
//! time that see it. Samples closer than a disc to one already kept are merged away. Samples that hit nothing (the
//! sky) become discs facing the camera on a far shell. Samples that say nothing (DepthView::unknown) seed nothing.
//!
//! The merge runs first, on every depth frame at once (each frame's candidates in parallel, merged in frame order so
//! the first sample of a cell wins as it always did); only the seeds kept within the budget then get their normal
//! and colour.

use crate::camera::{dot, Camera};
use crate::dataset::{sample_rgb, Dataset, DepthView};
use crate::gaussians::{logit, quat_from_z, Gaussians};
use rayon::prelude::*;
use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hasher};

/// Distance of the sky shell, well inside the far plane of the sort keys.
pub const SKY: f32 = 400.0;

/// Depth frames whose candidates are found at once (their lists are held until merged).
const CHUNK: usize = 64;

/// Seeds at most `budget` Gaussians (a random subset when there are more), coloured from the keyframes that are not
/// `held_out` (those score the result). `progress` (0..1) follows the depth frames.
pub fn from_depth(data: &Dataset, budget: usize, held_out: &[usize], mut progress: impl FnMut(f32)) -> Gaussians {
    // One candidate per merge cell: the first sample, in depth-frame order, that falls in it.
    let mut seen: HashSet<u64, BuildHasherDefault<Mixed>> = HashSet::default();
    let mut candidates: Vec<(u32, u32)> = Vec::new();
    for (c, chunk) in data.depth_views.chunks(CHUNK).enumerate() {
        let found: Vec<Vec<(u64, u32)>> = chunk.par_iter().map(|dv| frame_candidates(data, dv)).collect();
        for (k, list) in found.into_iter().enumerate() {
            let v = (c * CHUNK + k) as u32;
            candidates.extend(list.into_iter().filter(|(key, _)| seen.insert(*key)).map(|(_, i)| (v, i)));
        }
        progress(((c + 1) * CHUNK).min(data.depth_views.len()) as f32 / data.depth_views.len().max(1) as f32);
    }
    drop(seen);
    if candidates.len() > budget {
        // The same draw as ever: one per candidate, in order.
        let mut rng = fastrand::Rng::with_seed(7);
        let keep_p = budget as f64 / candidates.len() as f64;
        candidates.retain(|_| rng.f64() < keep_p);
    }
    // Normals and colours, only for the seeds kept: colours from the four training keyframes nearest in time.
    let held: Vec<bool> = (0..data.views.len()).map(|i| held_out.contains(&i)).collect();
    let mut times: Vec<(f32, usize)> = (0..data.views.len()).filter(|&i| !held[i]).map(|i| (data.views[i].game_time, i)).collect();
    times.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let nearest: Vec<Vec<usize>> = data.depth_views.par_iter().map(|dv| nearest_in_time(&times, dv.game_time, 4)).collect();
    let seeds: Vec<Seed> = candidates.par_iter().map(|&(v, i)| seed(data, &data.depth_views[v as usize], i as usize, &nearest[v as usize])).collect();
    let mut g = Gaussians::default();
    let opacity = logit(0.6);
    for s in seeds {
        g.push(s.mean, s.log_scale, s.rotation, opacity, s.color);
        *g.sky.last_mut().expect("pushed") = s.sky;
    }
    g
}

/// Depths of [`add_from_rays`]' seeds: spread evenly in inverse depth between these, metres.
pub const RAY_NEAREST: f32 = 0.5;
pub const RAY_FARTHEST: f32 = 50.0;

/// Seeds for a capture whose depth frames say too little: on the plaza walk under heavy bloom, before decoders took the
/// glow out (contracts, 2026-10-22), 99.97 % of the samples could not be read, depth seeded about a thousand Gaussians,
/// and training stayed there (8–10 dB off-path). Adds
/// `count` along the rays of training keyframes' scene pixels (not `held_out`), 3DGS's random initialisation: depths
/// spread evenly in inverse depth between [`RAY_NEAREST`] and [`RAY_FARTHEST`], the pixel's colour, a round footprint
/// as wide as the seeds' spacing, opacity 0.1. Training keeps those the views agree on.
pub fn add_from_rays(g: &mut Gaussians, data: &Dataset, count: usize, held_out: &[usize], seed: u64) {
    let views: Vec<usize> = (0..data.views.len()).filter(|i| !held_out.contains(i)).collect();
    let scene: Vec<u32> = (0..data.mask.len()).filter(|&p| data.mask[p] > 0.5).map(|p| p as u32).collect();
    if views.is_empty() || scene.is_empty() || count == 0 {
        return;
    }
    let mut rng = fastrand::Rng::with_seed(seed);
    // The seeds' spacing in a view, pixels: the views share them out over their scene pixels.
    let spacing = (scene.len() as f32 * views.len() as f32 / count as f32).sqrt();
    let opacity = logit(0.1);
    let (near, far) = (1.0 / RAY_NEAREST, 1.0 / RAY_FARTHEST);
    for _ in 0..count {
        let v = &data.views[views[rng.usize(..views.len())]];
        let p = scene[rng.usize(..scene.len())];
        let (x, y) = ((p % data.width) as f32, (p / data.width) as f32);
        let z = 1.0 / (far + rng.f32() * (near - far));
        let Some(color) = sample_rgb(&v.image, data.width, data.height, x, y) else { continue };
        let s = (z * spacing / v.camera.fx).max(1e-4).ln();
        g.push(v.camera.unproject(x, y, z), [s; 3], [1.0, 0.0, 0.0, 0.0], opacity, color);
    }
}

/// A sample's point and the footprint of its disc (the spacing of its tile's samples): on the sky shell when it hit
/// nothing.
fn point(dv: &DepthView, i: usize) -> ([f32; 3], f32, bool) {
    let [u, v] = dv.grid.pixels[i];
    let z = dv.depth[i];
    let cam_pos = dv.camera.position();
    let spacing = dv.grid.spacing * dv.grid.step(i) as f32;
    if z <= 0.0 {
        (add(cam_pos, scale(normalize(sub(dv.camera.unproject(u, v, 1.0), cam_pos)), SKY)), SKY * spacing / dv.camera.fx, true)
    } else {
        (dv.camera.unproject(u, v, z), z * spacing / dv.camera.fx, false)
    }
}

/// A depth frame's merge cells (key, sample), the first sample of each cell in row-major order. Sky samples count
/// only over the scene (not under the strip or the skeleton band).
fn frame_candidates(data: &Dataset, dv: &DepthView) -> Vec<(u64, u32)> {
    let mut local: HashSet<u64, BuildHasherDefault<Mixed>> = HashSet::default();
    let mut out = Vec::new();
    for i in 0..dv.depth.len().min(dv.grid.len()) {
        if dv.is_unknown(i) || (dv.depth[i] <= 0.0 && !in_scene(data, dv.grid.pixels[i][0], dv.grid.pixels[i][1])) {
            continue;
        }
        let (p, footprint, _) = point(dv, i);
        // Merge: one disc per cell of about its own size (levels in powers of two, so sizes stay comparable).
        let level = footprint.max(1e-3).log2().round() as i32;
        let cell = 2f32.powi(level);
        let key = cell_key((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32, (p[2] / cell).floor() as i32, level);
        if local.insert(key) {
            out.push((key, i as u32));
        }
    }
    out
}

/// A merge cell as 64 well-mixed bits (two cells share them about once in 2⁶⁴ pairs, which merges two discs).
fn cell_key(x: i32, y: i32, z: i32, level: i32) -> u64 {
    let a = (x as u32 as u64) | ((y as u32 as u64) << 32);
    let b = (z as u32 as u64) | ((level as u32 as u64) << 32);
    mix(a ^ mix(b ^ 0x9e37_79b9_7f4a_7c15))
}

/// SplitMix64's finaliser.
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// The hasher of keys that are mixed already ([`cell_key`]): their bits as they are.
#[derive(Default)]
struct Mixed(u64);

impl Hasher for Mixed {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = mix(self.0 ^ b as u64);
        }
    }

    fn write_u64(&mut self, v: u64) {
        self.0 = v;
    }
}

/// The `k` keyframes of `times` (game time, view; sorted) nearest to `time`, ties to the lower index.
fn nearest_in_time(times: &[(f32, usize)], time: f32, k: usize) -> Vec<usize> {
    let at = times.partition_point(|t| t.0 < time);
    let mut window: Vec<(f32, usize)> = times[at.saturating_sub(k)..(at + k).min(times.len())].iter().map(|&(t, i)| ((t - time).abs(), i)).collect();
    window.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    window.into_iter().take(k).map(|(_, i)| i).collect()
}

struct Seed {
    mean: [f32; 3],
    log_scale: [f32; 3],
    rotation: [f32; 4],
    color: [f32; 3],
    sky: bool,
}

/// The disc of sample `i` of a depth frame: facing its neighbours' surface (or the camera), coloured from `views`.
fn seed(data: &Dataset, dv: &DepthView, i: usize, views: &[usize]) -> Seed {
    let (gw, gh) = (dv.grid.width as i32, dv.grid.height as i32);
    let world = |gx: i32, gy: i32| -> Option<[f32; 3]> {
        if gx < 0 || gy < 0 || gx >= gw || gy >= gh {
            return None;
        }
        let i = (gy * gw + gx) as usize;
        let z = *dv.depth.get(i)?;
        (z > 0.0 && !dv.is_unknown(i)).then(|| dv.camera.unproject(dv.grid.pixels[i][0], dv.grid.pixels[i][1], z))
    };
    let (p, footprint, sky) = point(dv, i);
    let view = normalize(sub(dv.camera.position(), p));
    let (gx, gy) = ((i % gw as usize) as i32, (i / gw as usize) as i32);
    let normal = if sky { None } else { surface_normal(&world, gx, gy, dv.grid.step(i) as i32, p, dv.depth[i]) }.map(|n| if dot(n, view) < 0.0 { neg(n) } else { n }).unwrap_or(view);
    let s = (footprint * 0.75).max(1e-3).ln();
    Seed { mean: p, log_scale: [s, s, (footprint * 0.1).max(1e-4).ln()], rotation: quat_from_z(normal), color: color_at(data, p, views).unwrap_or([0.5, 0.5, 0.5]), sky }
}

/// Whether a training pixel shows the world (not the strip or the skeleton band).
fn in_scene(data: &Dataset, u: f32, v: f32) -> bool {
    let (x, y) = (u.round(), v.round());
    x >= 0.0 && y >= 0.0 && (x as u32) < data.width && (y as u32) < data.height && data.mask[(y as u32 * data.width + x as u32) as usize] > 0.5
}

/// Normal from the neighbouring samples, `step` grid cells away (those of the same tile), unless they sit across a
/// depth edge.
fn surface_normal(world: &impl Fn(i32, i32) -> Option<[f32; 3]>, gx: i32, gy: i32, step: i32, p: [f32; 3], z: f32) -> Option<[f32; 3]> {
    let near = |q: [f32; 3]| (dot(sub(q, p), sub(q, p))).sqrt() < 0.15 * z * step as f32;
    let pick = |a: Option<[f32; 3]>, b: Option<[f32; 3]>| match (a.filter(|q| near(*q)), b.filter(|q| near(*q))) {
        (Some(a), Some(b)) => Some(sub(a, b)),
        (Some(a), None) => Some(sub(a, p)),
        (None, Some(b)) => Some(sub(p, b)),
        (None, None) => None,
    };
    let du = pick(world(gx + step, gy), world(gx - step, gy))?;
    let dv = pick(world(gx, gy + step), world(gx, gy - step))?;
    let n = cross(du, dv);
    let len = dot(n, n).sqrt();
    (len > 1e-12).then(|| n.map(|c| c / len))
}

/// Colour of a world point in the first of `views` that sees it.
fn color_at(data: &Dataset, p: [f32; 3], views: &[usize]) -> Option<[f32; 3]> {
    for &i in views {
        let v = &data.views[i];
        let cam: &Camera = &v.camera;
        if let Some((u, vv, _)) = cam.project(p) {
            let (x, y) = (u.round() as i64, vv.round() as i64);
            if x >= 0 && y >= 0 && (x as u32) < data.width && (y as u32) < data.height && data.mask[(y as u32 * data.width + x as u32) as usize] > 0.5 {
                if let Some(c) = sample_rgb(&v.image, data.width, data.height, u, vv) {
                    return Some(c);
                }
            }
        }
    }
    None
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    a.map(|c| c * s)
}

fn neg(a: [f32; 3]) -> [f32; 3] {
    [-a[0], -a[1], -a[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-12);
    a.map(|c| c / l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Geometry;
    use crate::dataset::{DepthGrid, View};
    use std::sync::Arc;

    const W: u32 = 160;
    const H: u32 = 90;

    fn camera(x: f32) -> Camera {
        Camera::look([x, 1.6, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, W, H)
    }

    /// 40 × 20 samples, 4 px apart.
    fn grid() -> Arc<DepthGrid> {
        let pixels = (0..20).flat_map(|y| (0..40).map(move |x| [x as f32 * 4.0 + 1.5, y as f32 * 4.0 + 2.0])).collect();
        Arc::new(DepthGrid { width: 40, height: 20, pixels, spacing: 4.0, ..Default::default() })
    }

    /// A bumpy wall about 3 m ahead; the top four rows see the sky.
    fn depth_frame(x: f32, time: f32, g: &Arc<DepthGrid>, rng: &mut fastrand::Rng) -> DepthView {
        let depth = (0..g.len()).map(|i| if i < 4 * 40 { 0.0 } else { 3.0 + 0.2 * rng.f32() }).collect();
        DepthView { camera: camera(x), grid: g.clone(), depth, game_time: time, unknown: Default::default(), decoder_unknown: 0, zero_may_be_unread: false }
    }

    fn keyframe(x: f32, time: f32, k: i32, rgb: [u32; 3]) -> View {
        View { camera: camera(x), image: vec![rgb[0] | rgb[1] << 8 | rgb[2] << 16 | 255 << 24; (W * H) as usize], game_time: time, keyframe: k }
    }

    fn dataset(views: Vec<View>, depth_views: Vec<DepthView>) -> Dataset {
        // The bottom rows are not scene (a skeleton band).
        let mask = (0..W * H).map(|p| if p / W >= 80 { 0.0 } else { 1.0 }).collect();
        Dataset { dir: Default::default(), views, depth_views, width: W, height: H, geometry: Geometry::new(W, H, 1), mask, loaded: Default::default() }
    }

    /// Depth frames a second apart, each `step` metres to the right of the one before; a red keyframe at the start and
    /// a green one long after.
    fn session(frames: usize, step: f32) -> Dataset {
        let (g, mut rng) = (grid(), fastrand::Rng::with_seed(4));
        let depth_views = (0..frames).map(|k| depth_frame(step * k as f32, k as f32, &g, &mut rng)).collect();
        dataset(vec![keyframe(0.0, 0.0, 0, [200, 10, 10]), keyframe(0.0, 100.0, 1, [10, 200, 10])], depth_views)
    }

    /// A capture without depth: seeds along the training keyframes' rays, inside their view at depths between the
    /// bounds, coloured from them (never from the held-out one), through scene pixels only.
    #[test]
    fn a_capture_without_depth_seeds_along_the_rays() {
        let data = dataset(vec![keyframe(0.0, 0.0, 0, [200, 10, 10]), keyframe(1.0, 1.0, 1, [10, 200, 10]), keyframe(2.0, 2.0, 2, [10, 10, 200])], Vec::new());
        assert!(from_depth(&data, usize::MAX, &[], |_| {}).is_empty());
        let mut g = Gaussians::default();
        add_from_rays(&mut g, &data, 500, &[1], 9);
        assert_eq!(g.len(), 500);
        for i in 0..g.len() {
            let c = g.colors[i];
            let from = if c[0] > 0.5 { 0.0 } else { 2.0 };
            assert!(c[1] < 0.5, "never the held-out keyframe's green: {c:?}");
            let (u, v, z) = camera(from).project(g.means[i]).expect("in front of its keyframe");
            assert!((RAY_NEAREST * 0.999..=RAY_FARTHEST * 1.001).contains(&z), "depth {z}");
            assert!((-0.01..W as f32).contains(&u) && (-0.01..80.0).contains(&v), "a scene pixel ({u}, {v}), not the band");
            assert!(g.opacity_logits[i] < 0.0 && !g.sky[i]);
        }
        // Depths spread in inverse depth: as many seeds nearer than 1 m as between 1 and 50 m.
        let near = (0..g.len()).filter(|&i| camera(if g.colors[i][0] > 0.5 { 0.0 } else { 2.0 }).project(g.means[i]).unwrap().2 < 1.0).count();
        assert!((200..300).contains(&near), "{near}");
    }

    /// The merge in parallel keeps exactly the discs the one-sample-at-a-time merge kept, in the same order.
    #[test]
    fn the_parallel_merge_keeps_what_the_sequential_one_did() {
        let data = session(150, 0.05);
        let mut seen = std::collections::HashSet::new();
        let mut expected = Vec::new();
        for dv in &data.depth_views {
            for i in 0..dv.depth.len() {
                if dv.is_unknown(i) || (dv.depth[i] <= 0.0 && !in_scene(&data, dv.grid.pixels[i][0], dv.grid.pixels[i][1])) {
                    continue;
                }
                let (p, footprint, sky) = point(dv, i);
                let level = footprint.max(1e-3).log2().round() as i32;
                let cell = 2f32.powi(level);
                if seen.insert(((p[0] / cell).floor() as i32, (p[1] / cell).floor() as i32, (p[2] / cell).floor() as i32, level)) {
                    expected.push((p, sky));
                }
            }
        }
        let mut last = 0.0;
        let g = from_depth(&data, usize::MAX, &[], |p| last = p);
        assert_eq!(last, 1.0);
        assert_eq!(g.means.iter().copied().zip(g.sky.iter().copied()).collect::<Vec<_>>(), expected);
        assert!(g.sky.iter().any(|s| *s) && g.sky.iter().any(|s| !*s));
        // Within a budget: a random subset of about that size, the same every time.
        let some = from_depth(&data, expected.len() / 4, &[], |_| {});
        assert!((some.len() as f32 - expected.len() as f32 / 4.0).abs() < 0.1 * expected.len() as f32, "{} of {}", some.len(), expected.len());
        assert_eq!(some, from_depth(&data, expected.len() / 4, &[], |_| {}));
    }

    /// Samples that say nothing seed nothing.
    #[test]
    fn unknown_samples_seed_nothing() {
        let mut data = session(1, 0.0);
        let n = data.depth_views[0].depth.len();
        for i in (0..n).filter(|i| i % 2 == 0) {
            data.depth_views[0].unknown.insert(i);
        }
        let dv = &data.depth_views[0];
        let unknown: Vec<[f32; 3]> = (0..n).filter(|i| i % 2 == 0).map(|i| point(dv, i).0).collect();
        let g = from_depth(&data, usize::MAX, &[], |_| {});
        assert!(!g.is_empty());
        assert!(g.means.iter().all(|m| !unknown.contains(m)));
        for i in 0..n {
            data.depth_views[0].unknown.insert(i);
        }
        assert!(from_depth(&data, usize::MAX, &[], |_| {}).is_empty());
    }

    /// Colours come from the training keyframes: a held-out keyframe nearer in time does not paint the seeds.
    #[test]
    fn seeds_take_no_colour_from_held_out_keyframes() {
        let data = session(3, 0.0);
        let green = |c: &[f32; 3]| c[1] > 0.7 && c[0] < 0.1;
        let red = |c: &[f32; 3]| c[0] > 0.7 && c[1] < 0.1;
        let walls = |g: &Gaussians| g.colors.iter().zip(&g.sky).filter(|(_, s)| !**s).map(|(c, _)| *c).collect::<Vec<_>>();
        // Keyframe 0 (red) is nearest in time to every depth frame; held out, keyframe 1 (green) colours the wall.
        assert!(walls(&from_depth(&data, usize::MAX, &[], |_| {})).iter().all(red));
        assert!(walls(&from_depth(&data, usize::MAX, &[0], |_| {})).iter().all(green));
    }
}
