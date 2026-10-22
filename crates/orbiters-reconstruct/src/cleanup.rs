//! Export-time cleanup (crate::train, once training ends): the Gaussians the capture's exact depth says are not there
//! leave the result.
//!
//! - Floaters: a Gaussian that reaches α = 1/255 at no surface a trusted depth sample saw (splat-transform's floater
//!   filter, its solid voxels taken from exact depth instead of the splats' own density), which the depth frames place
//!   in free space, in front of the surface they measured, more often than at one.
//! - Islands: of what no depth sample saw and no depth frame placed at a surface, the clusters that do not connect to
//!   the ground under the walked path (splat-transform's cluster filter, seeded on the path).
//!
//! Spared: the sky shell; what lies in front of rays that hit nothing (glass, water and particles never reach Unity's
//! depth texture, so in front of the sky they cannot be told from floaters there); and what many keyframes saw from
//! several sides (glass in front of a wall lies in free space for depth too, but every view sees it there).

use crate::camera::Camera;
use crate::dataset::DepthView;
use crate::gaussians::{rotation_matrix, sigmoid, Gaussians, Seen};
use rayon::prelude::*;
use std::collections::{HashSet, VecDeque};

/// Voxels of level L are VOXEL · 2^L wide; a depth sample counts in the level whose voxels reach RELATIVE of its
/// distance, and in every coarser one.
const VOXEL: f32 = 0.05;
const LEVELS: u32 = 11;
const RELATIVE: f32 = 0.015;
/// A Gaussian sits at a measured surface within this share of the sample's depth plus twice its largest axis.
const AT_SURFACE: f32 = 0.03;
/// Seen from several sides: in this many training keyframes, from this many compass sectors.
const WELL_SEEN_VIEWS: u32 = 8;
const WELL_SEEN_SECTORS: u32 = 3;
/// Islands: cells of this size, joined across up to this many cells; the ground under the path is searched this far
/// below each camera.
const CELL: f32 = 1.0;
const REACH: i32 = 2;
const GROUND: f32 = 3.0;

/// What the cleanup left out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cleaned {
    pub floaters: usize,
    pub islands: usize,
}

/// A voxel of `level` holding `p`: the level in the top bits, then three 20-bit coordinates (±26 km at 5 cm); none
/// past them.
fn key(level: u32, p: [f32; 3]) -> Option<u64> {
    let size = VOXEL * (1u32 << level) as f32;
    let mut k = (level as u64) << 60;
    for (axis, v) in p.iter().enumerate() {
        let c = (v / size).floor();
        if !(-524_288.0..524_288.0).contains(&c) {
            return None;
        }
        k |= ((c as i64 + 524_288) as u64) << (20 * axis);
    }
    Some(k)
}

/// The finest level whose voxels reach RELATIVE of `distance`.
fn level_for(distance: f32) -> u32 {
    let mut level = 0;
    while level + 1 < LEVELS && VOXEL * ((1u32 << level) as f32) < RELATIVE * distance {
        level += 1;
    }
    level
}

/// Every level's voxels that hold a trusted depth sample (a hit), from each sample's own level up.
fn solid(depth: &[DepthView]) -> HashSet<u64> {
    depth
        .par_iter()
        .map(|d| {
            let mut set = HashSet::new();
            let eye = d.camera.position();
            for (i, &z) in d.depth.iter().enumerate() {
                if z <= 0.0 || d.is_unknown(i) || i >= d.grid.pixels.len() {
                    continue;
                }
                let [u, v] = d.grid.pixels[i];
                let p = d.camera.unproject(u, v, z);
                let distance = ((p[0] - eye[0]).powi(2) + (p[1] - eye[1]).powi(2) + (p[2] - eye[2]).powi(2)).sqrt();
                for level in level_for(distance)..LEVELS {
                    if let Some(k) = key(level, p) {
                        set.insert(k);
                    }
                }
            }
            set
        })
        .reduce(HashSet::new, |mut a, b| {
            if a.len() < b.len() {
                return b.into_iter().chain(a).collect();
            }
            a.extend(b);
            a
        })
}

/// Whether Gaussian `i` reaches α = 1/255 at a solid voxel, its centre within the Gaussian widened by half a voxel: at
/// a level as coarse as half the Gaussian's reach and as the samples of a frame `distance` away (its nearest), or one
/// or two coarser (the frames that saw it may have been farther).
fn supported(g: &Gaussians, i: usize, solid: &HashSet<u64>, distance: f32) -> bool {
    let op = sigmoid(g.opacity_logits[i]) as f64;
    let reach = crate::cpu::extent_sigmas(op).powi(2) as f32;
    if reach <= 0.0 {
        return false;
    }
    let s = g.log_scales[i].map(f32::exp);
    let largest = s[0].max(s[1]).max(s[2]);
    let radius = reach.sqrt() * largest;
    let mut first = level_for(distance);
    while first + 1 < LEVELS && VOXEL * ((1u32 << first) as f32) < 0.5 * radius {
        first += 1;
    }
    let r = rotation_matrix(g.rotations[i]);
    let m = g.means[i];
    (first..(first + 3).min(LEVELS)).any(|level| {
        let size = VOXEL * (1u32 << level) as f32;
        let widened = s.map(|v| v * v + 0.25 * size * size);
        let lo = m.map(|c| ((c - radius) / size).floor() as i64);
        let hi = m.map(|c| ((c + radius) / size).floor() as i64);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let centre = [(x as f32 + 0.5) * size, (y as f32 + 0.5) * size, (z as f32 + 0.5) * size];
                    if !key(level, centre).is_some_and(|k| solid.contains(&k)) {
                        continue;
                    }
                    let d = [centre[0] - m[0], centre[1] - m[1], centre[2] - m[2]];
                    // In the Gaussian's own axes (the columns of its rotation).
                    let q: f32 = (0..3).map(|k| (r[0][k] * d[0] + r[1][k] * d[1] + r[2][k] * d[2]).powi(2) / widened[k]).sum();
                    if q <= reach {
                        return true;
                    }
                }
            }
        }
        false
    })
}

/// Per Gaussian, how the depth frames place its centre: in front of the surface they measured, at it, and in front of
/// a ray that hit nothing.
#[derive(Clone, Copy, Debug, Default)]
struct Votes {
    free: u32,
    surface: u32,
    nothing: u32,
}

/// The depth frame's sample nearest to pixel (`u`, `v`): its grid is regular up to a pixel's rounding.
struct Lookup<'a> {
    view: &'a DepthView,
    x0: f32,
    y0: f32,
    sx: f32,
    sy: f32,
}

impl<'a> Lookup<'a> {
    fn new(view: &'a DepthView) -> Option<Self> {
        let (gw, gh) = (view.grid.width as usize, view.grid.height as usize);
        let px = &view.grid.pixels;
        if gw < 2 || gh < 2 || px.len() != gw * gh || view.depth.len() != px.len() {
            return None;
        }
        let (x0, y0) = (px[0][0], px[0][1]);
        let sx = (px[gw - 1][0] - x0) / (gw - 1) as f32;
        let sy = (px[(gh - 1) * gw][1] - y0) / (gh - 1) as f32;
        (sx > 0.0 && sy > 0.0).then_some(Self { view, x0, y0, sx, sy })
    }

    /// The sample under (`u`, `v`): its depth (0: it hit nothing), or None where the frame says nothing there.
    fn at(&self, u: f32, v: f32) -> Option<f32> {
        let (gw, gh) = (self.view.grid.width as i64, self.view.grid.height as i64);
        let gx = ((u - self.x0) / self.sx).round() as i64;
        let gy = ((v - self.y0) / self.sy).round() as i64;
        if gx < 0 || gy < 0 || gx >= gw || gy >= gh {
            return None;
        }
        // Unknown takes in the cells no tile carries.
        let i = (gy * gw + gx) as usize;
        if self.view.is_unknown(i) {
            return None;
        }
        Some(self.view.depth[i])
    }
}

fn votes(g: &Gaussians, i: usize, lookups: &[Lookup<'_>]) -> Votes {
    let mut out = Votes::default();
    let largest = g.log_scales[i].iter().fold(f32::MIN, |a, s| a.max(*s)).exp();
    for l in lookups {
        let cam = &l.view.camera;
        let Some((u, v, z)) = cam.project(g.means[i]) else { continue };
        if z <= crate::cpu::NEAR as f32 || u < -0.5 || v < -0.5 || u > cam.width as f32 - 0.5 || v > cam.height as f32 - 0.5 {
            continue;
        }
        let Some(d) = l.at(u, v) else { continue };
        if d <= 0.0 {
            out.nothing += 1;
            continue;
        }
        let margin = AT_SURFACE * d + 2.0 * largest;
        if z < d - margin {
            out.free += 1;
        } else if z <= d + margin {
            out.surface += 1;
        }
    }
    out
}

/// Each Gaussian's distance beyond what the depth frames reached (metres, 0 within it): the union of balls around
/// every frame's camera, as far out as nine tenths of its trusted hits reach (the walked path's own surroundings where
/// a frame hit nothing). The sky shell's stays 0: it is the far field's backdrop, not part of it.
pub fn beyond(g: &Gaussians, depth: &[DepthView]) -> Vec<f32> {
    const NEAR: f32 = 2.0;
    let balls: Vec<([f32; 3], f32)> = depth
        .par_iter()
        .map(|d| {
            let eye = d.camera.position();
            let mut far: Vec<f32> = (0..d.depth.len().min(d.grid.pixels.len()))
                .filter(|&i| d.depth[i] > 0.0 && !d.is_unknown(i))
                .map(|i| {
                    let [u, v] = d.grid.pixels[i];
                    let p = d.camera.unproject(u, v, d.depth[i]);
                    ((p[0] - eye[0]).powi(2) + (p[1] - eye[1]).powi(2) + (p[2] - eye[2]).powi(2)).sqrt()
                })
                .collect();
            let reach = if far.is_empty() {
                NEAR
            } else {
                let k = (far.len() * 9 / 10).min(far.len() - 1);
                far.select_nth_unstable_by(k, f32::total_cmp).1.max(NEAR)
            };
            (eye, reach)
        })
        .collect();
    (0..g.len())
        .into_par_iter()
        .map(|i| {
            if balls.is_empty() || g.sky.get(i).copied().unwrap_or(false) {
                return 0.0;
            }
            let p = g.means[i];
            balls.iter().map(|(c, r)| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt() - r).fold(f32::MAX, f32::min).max(0.0)
        })
        .collect()
}

fn well_seen(s: Seen) -> bool {
    s.weight >= 0.0 && s.views >= WELL_SEEN_VIEWS && s.sectors.count_ones() >= WELL_SEEN_SECTORS
}

/// The Gaussians to keep (see the module), from the depth frames `depth` and the positions of the training cameras
/// `path`; `focal` turns a Gaussian's finest sampling rate (crate::gaussians::Seen) into the distance of its nearest
/// keyframe (without one, the nearest depth frame's).
pub fn clean(g: &Gaussians, depth: &[DepthView], path: &[Camera], focal: f32) -> (Vec<bool>, Cleaned) {
    let n = g.len();
    let solid = solid(depth);
    let lookups: Vec<Lookup<'_>> = depth.iter().filter_map(Lookup::new).collect();
    let eyes: Vec<[f32; 3]> = depth.iter().map(|d| d.camera.position()).collect();
    let nearest = |p: [f32; 3]| eyes.iter().map(|e| (p[0] - e[0]).powi(2) + (p[1] - e[1]).powi(2) + (p[2] - e[2]).powi(2)).fold(f32::MAX, f32::min).sqrt();
    // 0 keep, 1 floater, 2 island candidate.
    let verdict: Vec<u8> = (0..n)
        .into_par_iter()
        .map(|i| {
            let seen = g.seen(i);
            if g.sky.get(i).copied().unwrap_or(false) || well_seen(seen) {
                return 0;
            }
            let distance = if seen.rate > 0.0 { focal / seen.rate } else { nearest(g.means[i]) };
            if supported(g, i, &solid, distance) {
                return 0;
            }
            let v = votes(g, i, &lookups);
            if v.nothing > 0 {
                2
            } else if v.free >= 2 && v.free > v.surface {
                1
            } else if v.surface == 0 {
                2
            } else {
                0
            }
        })
        .collect();
    let mut keep: Vec<bool> = verdict.iter().map(|v| *v != 1).collect();
    let mut cleaned = Cleaned { floaters: verdict.iter().filter(|v| **v == 1).count(), islands: 0 };
    // Islands: what is left, by cell; reached from the ground under the path.
    let cell = |p: [f32; 3]| p.map(|c| (c / CELL).floor() as i32);
    let occupied: HashSet<[i32; 3]> = (0..n).filter(|&i| keep[i] && g.means[i].iter().all(|c| c.abs() < 1e6)).map(|i| cell(g.means[i])).collect();
    let mut reached: HashSet<[i32; 3]> = HashSet::new();
    let mut queue = VecDeque::new();
    for cam in path {
        let p = cam.position();
        let mut below = 0.0;
        while below <= GROUND {
            let c = cell([p[0], p[1] - below, p[2]]);
            if occupied.contains(&c) && reached.insert(c) {
                queue.push_back(c);
            }
            below += 0.5 * CELL;
        }
    }
    if queue.is_empty() {
        // Nothing under the path (a flight above everything): no seed to tell islands from the world by.
        return (keep, cleaned);
    }
    while let Some(c) = queue.pop_front() {
        for dx in -REACH..=REACH {
            for dy in -REACH..=REACH {
                for dz in -REACH..=REACH {
                    let next = [c[0] + dx, c[1] + dy, c[2] + dz];
                    if occupied.contains(&next) && reached.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }
    }
    for i in 0..n {
        if verdict[i] == 2 && !reached.contains(&cell(g.means[i])) {
            keep[i] = false;
            cleaned.islands += 1;
        }
    }
    (keep, cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{DepthGrid, Marks};
    use std::sync::Arc;

    /// A depth frame looking along +Z from `eye` at a wall `wall` metres ahead, its samples every 4 pixels.
    fn wall_frame(eye: [f32; 3], wall: f32) -> DepthView {
        let cam = Camera::look(eye, [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, 64, 48);
        let (gw, gh) = (16u32, 12u32);
        let pixels: Vec<[f32; 2]> = (0..gw * gh).map(|k| [(k % gw) as f32 * 4.0 + 1.5, (k / gw) as f32 * 4.0 + 1.5]).collect();
        let grid = DepthGrid { width: gw, height: gh, pixels, spacing: 4.0, unused: Marks::default(), steps: Vec::new() };
        let n = (gw * gh) as usize;
        DepthView {
            camera: cam,
            grid: Arc::new(grid),
            depth: vec![wall - eye[2]; n],
            game_time: 0.0,
            unknown: Marks::default(),
            decoder_unknown: 0,
            zero_may_be_unread: false,
        }
    }

    fn splat(g: &mut Gaussians, at: [f32; 3], size: f32) {
        g.push(at, [size.ln(); 3], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.8), [0.5; 3]);
    }

    #[test]
    fn floaters_in_front_of_measured_surfaces_go_and_surfaces_stay() {
        // A wall at z = 5 seen from three places along the path.
        let frames: Vec<DepthView> = [-0.5f32, 0.0, 0.5].iter().map(|x| wall_frame([*x, 1.6, 0.0], 5.0)).collect();
        let path: Vec<Camera> = frames.iter().map(|f| f.camera).collect();
        let mut g = Gaussians::default();
        splat(&mut g, [0.0, 1.6, 5.0], 0.05); // on the wall
        splat(&mut g, [0.2, 1.7, 2.5], 0.03); // floating half way to it
        splat(&mut g, [0.1, 1.6, 4.98], 0.02); // just in front of it
        let (keep, cleaned) = clean(&g, &frames, &path, path[0].fx);
        assert_eq!(keep, vec![true, false, true]);
        assert_eq!(cleaned, Cleaned { floaters: 1, islands: 0 });
        // On the sky shell, or seen well from several sides: spared.
        g.sky[1] = true;
        assert!(clean(&g, &frames, &path, path[0].fx).0[1]);
        g.sky[1] = false;
        g.seen[1] = Seen { weight: 0.9, views: 20, sectors: 0b111, rate: 300.0, beyond: 0.0 };
        assert!(clean(&g, &frames, &path, path[0].fx).0[1]);
    }

    /// Within the reach of the depth frames: 0; past it, the distance to the nearest frame's reach; the sky: 0.
    #[test]
    fn beyond_measures_from_what_the_depth_frames_reached() {
        let frames: Vec<DepthView> = [-0.5f32, 0.5].iter().map(|x| wall_frame([*x, 1.6, 0.0], 5.0)).collect();
        let mut g = Gaussians::default();
        splat(&mut g, [0.0, 1.6, 4.0], 0.05);
        splat(&mut g, [0.0, 1.6, 30.0], 0.05);
        splat(&mut g, [0.0, 1.6, 300.0], 0.05);
        g.sky[2] = true;
        let b = beyond(&g, &frames);
        assert_eq!(b[0], 0.0);
        // The wall's hits reach about 5 m (a little more off the axis).
        assert!(b[1] > 23.0 && b[1] < 25.5, "{}", b[1]);
        assert_eq!(b[2], 0.0);
    }

    #[test]
    fn what_hides_in_front_of_the_sky_stays_where_it_joins_the_world() {
        // Frames whose rays hit nothing (sky) where they do not hit the floor under the path.
        let mut frames: Vec<DepthView> = [-0.5f32, 0.5].iter().map(|x| wall_frame([*x, 1.6, 0.0], 5.0)).collect();
        for f in &mut frames {
            f.depth.fill(0.0);
        }
        let path: Vec<Camera> = frames.iter().map(|f| f.camera).collect();
        let mut g = Gaussians::default();
        // The floor under the path and a lamp post rising from it, in front of the sky; a blob far out in the sky.
        for k in 0..12 {
            splat(&mut g, [0.0, 0.0, k as f32 * 0.5], 0.2);
        }
        for k in 0..6 {
            splat(&mut g, [0.0, k as f32 * 0.8, 6.0], 0.1);
        }
        splat(&mut g, [3.0, 9.0, 30.0], 0.3);
        let (keep, cleaned) = clean(&g, &frames, &path, path[0].fx);
        assert!(keep[..18].iter().all(|k| *k), "{keep:?}");
        assert!(!keep[18]);
        assert_eq!(cleaned, Cleaned { floaters: 0, islands: 1 });
    }
}
