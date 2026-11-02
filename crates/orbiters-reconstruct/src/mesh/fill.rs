//! Floor hole filling, only where the evidence says floor.
//!
//! The depth leaves floors unobserved behind benches and posts, under the person's own feet (the camera looks ahead)
//! and where a quad was too oblique to trust. A collider with those holes drops people through it. This closes them
//! in the field itself (so marching cubes joins them to the observed floor seamlessly), as a thin synthetic band of
//! tiny weight, in columns no observation reached:
//!
//! 1. The walkable floor: on the voxel columns of the region, every upward surface crossing (solid below, free above,
//!    within 50° of level). From the floor under the walked path, a flood across neighbouring columns whose floor is
//!    within a stair step (0.36 m) of the last: one height per column, the floor a person reaches.
//! 2. Holes: columns not reached, within 2 m of it, with nothing solid between 0.1 and 0.3 m above the nearby floor
//!    (that would be a wall's foot or an obstacle), grouped by 4-connectivity. A hole is filled when it is enclosed
//!    (it touches neither the region's edge nor columns more than 2 m from floor) and at most [`MAX_HOLE`] m² (or a
//!    walked path point lies in it), or when it is an occlusion shadow (at most [`MAX_SHADOW`] m², at most
//!    [`MAX_OPEN`] of its rim open, half of it reached floor); and at least [`MIN_FLOOR_RIM`] of its rim is reached
//!    floor whose heights fit one plane ([`MIN_INLIERS`] of them within 3 cm RMS, none of those off by more than
//!    8 cm, at most 35° steep).
//! 3. The walked corridor: every column within [`CORRIDOR`] of the path not reached and not solid, at the plane of the
//!    reached floor within 1.5 m beyond it near the path's feet height (at least [`MIN_FLOOR_NEAR`] columns of it,
//!    [`MIN_INLIERS`] of them on the plane within 3 cm RMS: no floor evidence, no fill). A person walked there.
//!
//! Never filled: a column where the capture saw free space below the plane (it looked through: a stairwell, a drop,
//! a drain) or saw a floor more than 0.3 m below it; a hole with more than a tenth of its columns so is left open.
//! Never a wall: only level-ish planes are written, and only into unobserved voxels.

use super::{
    roi::Roi,
    tsdf::{local, Volume, N, SIDE},
    WalkPath,
};
use rayon::prelude::*;
use std::collections::VecDeque;

/// A hole at most this large (m²) is filled when enclosed by floor.
pub const MAX_HOLE: f32 = 4.0;
/// An occlusion shadow (the floor hidden behind a post, a tree, a bench from everywhere the capture looked) at most
/// this large (m²) is filled when at most [`MAX_OPEN`] of its rim opens onto unknown space and at least half of it is
/// reached floor.
pub const MAX_SHADOW: f32 = 40.0;
pub const MAX_OPEN: f32 = 0.15;
/// Share of a hole's rim that must be reached floor (the rest: obstacles).
pub const MIN_FLOOR_RIM: f32 = 0.3;
/// Reached floor columns near a path point needed to fill under it.
pub const MIN_FLOOR_NEAR: usize = 20;
/// Share of the floor points a plane must keep (crate::mesh::fill::robust_plane).
pub const MIN_INLIERS: f32 = 0.7;
/// Half-width of the walked corridor whose unobserved floor is filled, metres: someone stepping aside from the
/// recorded line (or a capsule wider than the camera) must not fall through.
pub const CORRIDOR: f32 = 1.5;
const CLIMB: f32 = 0.36;
const REACH: f32 = 2.0;
/// Weight of synthetic values: far below one observation's, so any real observation would dominate.
const FILL_WEIGHT: f32 = 0.01;

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct Stats {
    pub crossings: usize,
    pub floor_columns: usize,
    pub seeds: usize,
    pub holes: usize,
    pub holes_filled: usize,
    pub holes_open: usize,
    pub holes_too_large: usize,
    /// Of the holes filled, occlusion shadows (not enclosed, see [`MAX_SHADOW`]).
    pub shadows: usize,
    pub holes_not_planar: usize,
    pub holes_seen_through: usize,
    pub hole_columns: usize,
    pub path_columns: usize,
    pub path_points_without_floor: usize,
    pub seconds: f64,
}

struct Columns {
    gx0: i32,
    gz0: i32,
    w: usize,
    h: usize,
    inside: Vec<bool>,
}

impl Columns {
    fn index(&self, gx: i32, gz: i32) -> Option<usize> {
        let (x, z) = (gx - self.gx0, gz - self.gz0);
        (x >= 0 && z >= 0 && (x as usize) < self.w && (z as usize) < self.h).then(|| z as usize * self.w + x as usize)
    }
    fn coords(&self, i: usize) -> (i32, i32) {
        ((i % self.w) as i32 + self.gx0, (i / self.w) as i32 + self.gz0)
    }
    fn neighbours(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let (gx, gz) = self.coords(i);
        [(1, 0), (-1, 0), (0, 1), (0, -1)].into_iter().filter_map(move |(dx, dz)| self.index(gx + dx, gz + dz))
    }
}

/// A plane through most of `points`: least squares, then again without the points more than max(3 cm, 2 RMS) off,
/// twice (a rim touching a bench leg or a curb keeps the floor's plane). With the inliers' RMS and largest residual;
/// none when fewer than [`MIN_INLIERS`] of the points are inliers.
fn robust_plane(points: &[(f32, f32, f32)]) -> Option<([f64; 3], f64, f64)> {
    let mut kept: Vec<(f32, f32, f32)> = points.to_vec();
    let mut fit = plane(&kept)?;
    for _ in 0..2 {
        let p = fit.0;
        let cut = (2.0 * fit.1).max(0.03);
        kept = points.iter().copied().filter(|q| (q.2 as f64 - (p[0] * q.0 as f64 + p[1] * q.1 as f64 + p[2])).abs() <= cut).collect();
        if (kept.len() as f32) < MIN_INLIERS * points.len() as f32 {
            return None;
        }
        fit = plane(&kept)?;
    }
    Some(fit)
}

/// Radius of the capturing person's body carved out of the field, metres (arms at rest, a tail, a coat).
pub const BODY_RADIUS: f32 = 0.35;

/// Carves the capturing person out of the field: wherever their hips went (the capture kit's skeleton frames), the
/// vertical cylinder of [`BODY_RADIUS`] from 8 cm above the floor under them to 0.3 m over their eyes held their body,
/// not the world. The depth sees that body (a camera flown around the avatar, a mirror's reflection is not involved:
/// the avatar itself), and a person standing still is never seen through, so without this a statue of them stands in
/// the collider where they stood (VRChat Home, first run). Observed voxels there become free space; the floor under
/// their feet is then closed by the corridor fill. The floor: the median of the lowest upward crossing of each column
/// within 0.5 m, between 0.5 m below and 0.4 m above the feet estimate. Returns the voxels carved.
pub fn carve_walker(vol: &mut Volume, path: &WalkPath) -> usize {
    let v = vol.voxel;
    let r = (BODY_RADIUS / v).ceil() as i32;
    let disc = (0.5 / v).ceil() as i32;
    let mut carved = 0;
    for p in path.resampled(0.05) {
        let (cx, cz) = ((p[0] / v).floor() as i32, (p[2] / v).floor() as i32);
        let (lo, hi) = (((p[1] - 0.5) / v).floor() as i32, ((p[1] + 0.4) / v).ceil() as i32);
        let mut lowest = Vec::new();
        for dz in -disc..=disc {
            for dx in -disc..=disc {
                let (gx, gz) = (cx + dx, cz + dz);
                let found = (lo..hi).find(|&gy| {
                    let below = vol.get([gx, gy, gz]).filter(|s| s.1 > 0.0);
                    let above = vol.get([gx, gy + 1, gz]).filter(|s| s.1 > 0.0);
                    matches!((below, above), (Some((a, _)), Some((b, _))) if a < 0.0 && b >= 0.0)
                });
                if let Some(gy) = found {
                    lowest.push(gy);
                }
            }
        }
        lowest.sort_unstable();
        let floor = lowest.get(lowest.len() / 2).map_or(p[1], |&gy| (gy as f32 + 0.5) * v);
        let (bottom, top) = (((floor + 0.08) / v).ceil() as i32, ((floor + path.eye_height + 0.3) / v).ceil() as i32);
        for dz in -r..=r {
            for dx in -r..=r {
                if dx * dx + dz * dz > r * r {
                    continue;
                }
                for gy in bottom..=top {
                    let g = [cx + dx, gy, cz + dz];
                    let Some(&b) = vol.index.get(&g.map(|c| c.div_euclid(SIDE))) else { continue };
                    let i = b as usize * N + local(g.map(|c| c.rem_euclid(SIDE)));
                    if vol.weight[i] > 0.0 && vol.sdf[i] < 1.0 {
                        vol.sdf[i] = 1.0;
                        carved += 1;
                    }
                }
            }
        }
    }
    carved
}

/// Least-squares plane y = a·x + b·z + c through (x, z, y); with its RMS and largest residual.
fn plane(points: &[(f32, f32, f32)]) -> Option<([f64; 3], f64, f64)> {
    if points.len() < 3 {
        return None;
    }
    let n = points.len() as f64;
    let (mx, mz, my) = points.iter().fold((0.0, 0.0, 0.0), |a, p| (a.0 + p.0 as f64 / n, a.1 + p.1 as f64 / n, a.2 + p.2 as f64 / n));
    let (mut sxx, mut sxz, mut szz, mut sxy, mut szy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for p in points {
        let (x, z, y) = (p.0 as f64 - mx, p.1 as f64 - mz, p.2 as f64 - my);
        sxx += x * x;
        sxz += x * z;
        szz += z * z;
        sxy += x * y;
        szy += z * y;
    }
    let det = sxx * szz - sxz * sxz;
    let (a, b) = if det.abs() > 1e-12 { ((sxy * szz - szy * sxz) / det, (szy * sxx - sxy * sxz) / det) } else { (0.0, 0.0) };
    let c = my - a * mx - b * mz;
    let (mut sum, mut worst) = (0.0f64, 0.0f64);
    for p in points {
        let r = (p.2 as f64 - (a * p.0 as f64 + b * p.1 as f64 + c)).abs();
        sum += r * r;
        worst = worst.max(r);
    }
    Some(([a, b, c], (sum / n).sqrt(), worst))
}

/// Closes floor holes in `vol` (see the module's comment).
pub fn floors(vol: &mut Volume, path: &WalkPath, roi: &Roi) -> Stats {
    let start = std::time::Instant::now();
    let mut stats = Stats::default();
    let v = vol.voxel;
    let gx0 = ((roi.x0 as f32 * roi.cell) / v).floor() as i32;
    let gz0 = ((roi.z0 as f32 * roi.cell) / v).floor() as i32;
    let w = ((roi.w as f32 * roi.cell) / v).ceil() as usize + 1;
    let h = ((roi.h as f32 * roi.cell) / v).ceil() as usize + 1;
    let inside: Vec<bool> = (0..w * h)
        .into_par_iter()
        .map(|i| {
            let (x, z) = ((i % w) as i32 + gx0, (i / w) as i32 + gz0);
            roi.contains([(x as f32 + 0.5) * v, roi.y_min + 0.01, (z as f32 + 0.5) * v])
        })
        .collect();
    let cols = Columns { gx0, gz0, w, h, inside };
    let at = |vol: &Volume, g: [i32; 3]| vol.get(g).filter(|(_, wt)| *wt > 0.0).map(|(s, _)| s);
    // 1. Upward crossings per column.
    let level = 50f32.to_radians().cos();
    let mut crossings: Vec<(u32, f32)> = (0..vol.keys.len())
        .into_par_iter()
        .flat_map_iter(|b| {
            let key = vol.keys[b];
            let mut out = Vec::new();
            for lz in 0..SIDE {
                for lx in 0..SIDE {
                    let (gx, gz) = (key[0] * SIDE + lx, key[2] * SIDE + lz);
                    let Some(ci) = cols.index(gx, gz) else { continue };
                    if !cols.inside[ci] {
                        continue;
                    }
                    for ly in 0..SIDE {
                        let i = b * N + local([lx, ly, lz]);
                        if vol.weight[i] <= 0.0 || vol.sdf[i] >= 0.0 {
                            continue;
                        }
                        let gy = key[1] * SIDE + ly;
                        let Some(s1) = at(vol, [gx, gy + 1, gz]) else { continue };
                        let s0 = vol.sdf[i];
                        if s1 < 0.0 {
                            continue;
                        }
                        let diff = |dx: i32, dz: i32| -> f32 {
                            let mut sum = 0.0;
                            for y in [gy, gy + 1] {
                                match (at(vol, [gx + dx, y, gz + dz]), at(vol, [gx - dx, y, gz - dz])) {
                                    (Some(a), Some(b)) => sum += 0.5 * (a - b),
                                    (Some(a), None) => sum += a - at(vol, [gx, y, gz]).unwrap_or(a),
                                    (None, Some(b)) => sum += at(vol, [gx, y, gz]).unwrap_or(b) - b,
                                    _ => {}
                                }
                            }
                            0.5 * sum
                        };
                        let g = [diff(1, 0), s1 - s0, diff(0, 1)];
                        let l = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
                        if l > 0.0 && g[1] >= level * l {
                            out.push((ci as u32, (gy as f32 + s0 / (s0 - s1)) * v));
                        }
                    }
                }
            }
            out
        })
        .collect();
    crossings.par_sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
    stats.crossings = crossings.len();
    let mut first = vec![0u32; w * h + 1];
    for &(c, _) in &crossings {
        first[c as usize + 1] += 1;
    }
    for i in 0..w * h {
        first[i + 1] += first[i];
    }
    let heights = |c: usize| crossings[first[c] as usize..first[c + 1] as usize].iter().map(|p| p.1);
    // 2. The reached floor, flooded from the path.
    let mut floor = vec![f32::NAN; w * h];
    let mut queue = VecDeque::new();
    let points = path.resampled(0.25);
    for p in &points {
        let (cx, cz) = ((p[0] / v).floor() as i32, (p[2] / v).floor() as i32);
        let r = (0.3 / v).ceil() as i32;
        let mut best: Option<(f32, usize, f32)> = None;
        for dz in -r..=r {
            for dx in -r..=r {
                let Some(c) = cols.index(cx + dx, cz + dz) else { continue };
                for y in heights(c) {
                    let cost = (y - p[1]).abs() + 0.5 * v * ((dx * dx + dz * dz) as f32).sqrt();
                    if (y - p[1]).abs() <= 0.6 && best.is_none_or(|b| cost < b.0) {
                        best = Some((cost, c, y));
                    }
                }
            }
        }
        if let Some((_, c, y)) = best {
            if floor[c].is_nan() {
                floor[c] = y;
                queue.push_back(c);
                stats.seeds += 1;
            }
        }
    }
    while let Some(c) = queue.pop_front() {
        let y = floor[c];
        for n in cols.neighbours(c) {
            if !floor[n].is_nan() || !cols.inside[n] {
                continue;
            }
            if let Some(best) = heights(n).filter(|h| (h - y).abs() <= CLIMB).min_by(|a, b| (a - y).abs().total_cmp(&(b - y).abs())) {
                floor[n] = best;
                queue.push_back(n);
            }
        }
    }
    stats.floor_columns = floor.iter().filter(|f| !f.is_nan()).count();
    // 3. Columns near the floor: their distance to it and its height there.
    let reach = (REACH / v).ceil() as u16;
    let mut dist = vec![u16::MAX; w * h];
    let mut near_height = vec![f32::NAN; w * h];
    for c in 0..w * h {
        if !floor[c].is_nan() {
            dist[c] = 0;
            near_height[c] = floor[c];
            queue.push_back(c);
        }
    }
    while let Some(c) = queue.pop_front() {
        if dist[c] >= reach {
            continue;
        }
        for n in cols.neighbours(c) {
            if dist[n] == u16::MAX && cols.inside[n] {
                dist[n] = dist[c] + 1;
                near_height[n] = near_height[c];
                queue.push_back(n);
            }
        }
    }
    let vol_ref: &Volume = vol;
    let column_has = |c: usize, lo: f32, hi: f32, test: &dyn Fn(f32) -> bool| -> bool {
        let (gx, gz) = cols.coords(c);
        ((lo / v).floor() as i32..=(hi / v).ceil() as i32).any(|gy| at(vol_ref, [gx, gy, gz]).is_some_and(test))
    };
    // Solid at a foot's height above the nearby floor: an obstacle or a wall's foot.
    let solid: Vec<bool> = (0..w * h).into_par_iter().map(|c| floor[c].is_nan() && dist[c] != u16::MAX && column_has(c, near_height[c] + 0.1, near_height[c] + 0.3, &|s| s < 0.0)).collect();
    // The capture saw through a column at height y: free space clearly below it within the band, or a floor far below.
    let trunc = vol_ref.trunc;
    let seen_through = |c: usize, y: f32| -> bool { column_has(c, y - trunc, y - 2.0 * v, &|s| s > 0.3) || heights(c).any(|h| h < y - 0.3 && h > y - 3.0) };
    let mut fill: Vec<(usize, f32)> = Vec::new();
    let mut filled = vec![false; w * h];
    // 4. Enclosed holes.
    let mut label = vec![u32::MAX; w * h];
    let path_cells: std::collections::HashSet<usize> = path.resampled(0.1).iter().filter_map(|p| cols.index((p[0] / v).floor() as i32, (p[2] / v).floor() as i32)).collect();
    for s in 0..w * h {
        if label[s] != u32::MAX || !floor[s].is_nan() || dist[s] == u16::MAX || solid[s] || !cols.inside[s] {
            continue;
        }
        stats.holes += 1;
        let id = stats.holes as u32;
        let mut members = vec![s];
        label[s] = id;
        let (mut open, mut rim, mut obstacles) = (0usize, Vec::new(), 0usize);
        let mut k = 0;
        while k < members.len() {
            let c = members[k];
            k += 1;
            let (gx, gz) = cols.coords(c);
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let Some(n) = cols.index(gx + dx, gz + dz) else {
                    open += 1;
                    continue;
                };
                if !cols.inside[n] || dist[n] == u16::MAX {
                    open += 1;
                } else if !floor[n].is_nan() {
                    let (nx, nz) = cols.coords(n);
                    rim.push(((nx as f32 + 0.5) * v, (nz as f32 + 0.5) * v, floor[n]));
                } else if solid[n] {
                    obstacles += 1;
                } else if label[n] == u32::MAX {
                    label[n] = id;
                    members.push(n);
                }
            }
        }
        let walked = members.iter().any(|c| path_cells.contains(c));
        let area = members.len() as f32 * v * v;
        let around = (rim.len() + obstacles + open) as f32;
        let enclosed = open == 0 && (area <= MAX_HOLE || walked);
        let shadow = area <= MAX_SHADOW && open as f32 <= MAX_OPEN * around && rim.len() as f32 >= 0.5 * around;
        if !enclosed && !shadow {
            if open > 0 {
                stats.holes_open += 1;
            } else {
                stats.holes_too_large += 1;
            }
            continue;
        }
        if !enclosed {
            stats.shadows += 1;
        }
        let planar = (rim.len() as f32 >= MIN_FLOOR_RIM * (rim.len() + obstacles) as f32)
            .then(|| robust_plane(&rim))
            .flatten()
            .filter(|(p, rms, worst)| *rms <= 0.03 && *worst <= 0.08 && (p[0] * p[0] + p[1] * p[1]).sqrt() <= 35f64.to_radians().tan());
        let Some((p, _, _)) = planar else {
            stats.holes_not_planar += 1;
            continue;
        };
        let y_of = |c: usize| {
            let (gx, gz) = cols.coords(c);
            (p[0] * ((gx as f64 + 0.5) * v as f64) + p[1] * ((gz as f64 + 0.5) * v as f64) + p[2]) as f32
        };
        let through = members.iter().filter(|&&c| seen_through(c, y_of(c))).count();
        if through * 10 > members.len() {
            stats.holes_seen_through += 1;
            continue;
        }
        stats.holes_filled += 1;
        for &c in &members {
            let y = y_of(c);
            if !seen_through(c, y) {
                fill.push((c, y));
                filled[c] = true;
                stats.hole_columns += 1;
            }
        }
    }
    // 5. The walked corridor: unobserved columns within CORRIDOR of the path, at the plane of the reached floor around.
    let r = (CORRIDOR / v).ceil() as i32;
    let near = ((CORRIDOR + 1.5) / v).ceil() as i32;
    for p in path.resampled(0.25) {
        let (cx, cz) = ((p[0] / v).floor() as i32, (p[2] / v).floor() as i32);
        let needs: Vec<usize> = (-r..=r)
            .flat_map(|dz| (-r..=r).map(move |dx| (dx, dz)))
            .filter(|(dx, dz)| dx * dx + dz * dz <= r * r)
            .filter_map(|(dx, dz)| cols.index(cx + dx, cz + dz))
            .filter(|&c| cols.inside[c] && floor[c].is_nan() && !filled[c] && !solid[c])
            .collect();
        if needs.is_empty() {
            continue;
        }
        let mut around = Vec::new();
        for dz in (-near..=near).step_by(2) {
            for dx in (-near..=near).step_by(2) {
                if let Some(c) = cols.index(cx + dx, cz + dz) {
                    if !floor[c].is_nan() && (floor[c] - p[1]).abs() <= 0.5 {
                        around.push((((cx + dx) as f32 + 0.5) * v, ((cz + dz) as f32 + 0.5) * v, floor[c]));
                    }
                }
            }
        }
        let fit = (around.len() >= MIN_FLOOR_NEAR).then(|| robust_plane(&around)).flatten().filter(|(_, rms, _)| *rms <= 0.03);
        let Some((pl, _, _)) = fit else {
            stats.path_points_without_floor += 1;
            continue;
        };
        for c in needs {
            let (gx, gz) = cols.coords(c);
            let y = (pl[0] * ((gx as f64 + 0.5) * v as f64) + pl[1] * ((gz as f64 + 0.5) * v as f64) + pl[2]) as f32;
            if !seen_through(c, y) && !column_has(c, y + 0.1, y + 0.3, &|s| s < 0.0) {
                fill.push((c, y));
                filled[c] = true;
                stats.path_columns += 1;
            }
        }
    }
    if let Ok(file) = std::env::var("ORBITERS_FILL_MAP") {
        // Lab: a top view of the columns: reached floor grey, filled holes green, the corridor's fill blue, solid orange,
        // near but unfilled red.
        let mut rgb = vec![20u8; w * h * 3];
        for c in 0..w * h {
            let col = if !floor[c].is_nan() {
                [110, 110, 110]
            } else if filled[c] {
                [60, 200, 90]
            } else if solid[c] {
                [220, 140, 40]
            } else if dist[c] != u16::MAX {
                [200, 40, 40]
            } else {
                continue;
            };
            // Mirror x so +Z is up and the map matches the mesh_bench top views.
            let (x, z) = (w - 1 - c % w, h - 1 - c / w);
            rgb[(z * w + x) * 3..(z * w + x) * 3 + 3].copy_from_slice(&col);
        }
        let _ = (|| -> anyhow::Result<()> {
            let mut e = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&file)?), w as u32, h as u32);
            e.set_color(png::ColorType::Rgb);
            e.set_depth(png::BitDepth::Eight);
            e.write_header()?.write_image_data(&rgb)?;
            Ok(())
        })();
    }
    // 6. Write the band into unobserved voxels.
    let trunc = vol.trunc;
    for (c, y) in fill {
        let (gx, gz) = cols.coords(c);
        for gy in ((y - trunc - v) / v).floor() as i32..=((y + trunc + v) / v).ceil() as i32 {
            let g = [gx, gy, gz];
            let b = vol.ensure(g.map(|c| c.div_euclid(SIDE)));
            let i = b * N + local(g.map(|c| c.rem_euclid(SIDE)));
            if vol.weight[i] <= 0.0 {
                vol.sdf[i] = ((gy as f32 * v - y) / trunc).clamp(-1.0, 1.0);
                vol.weight[i] = FILL_WEIGHT;
            }
        }
    }
    stats.seconds = start.elapsed().as_secs_f64();
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{mc, roi::WalkPath};

    /// A floor at y = 0 over x, z in [-3, 3) (observed through its band and 0.5 m of free space above), with `hole`
    /// columns unobserved, or with a lower floor at y = -1 seen through them (`below`).
    fn scene(hole: impl Fn(f32, f32) -> bool, below: bool) -> Volume {
        let v = 0.05;
        let mut vol = Volume::empty(v, 3.0 * v);
        let t = vol.trunc;
        for gx in -60..60 {
            for gz in -60..60 {
                let (x, z) = (gx as f32 * v, gz as f32 * v);
                let open = hole(x, z);
                for gy in -30..12 {
                    let y = gy as f32 * v;
                    let (observed, d) = match (open, below) {
                        (false, _) => (y >= -t && y <= 0.5, y),
                        (true, true) => (y >= -1.0 - t && y <= 0.5, y + 1.0),
                        (true, false) => (false, 0.0),
                    };
                    if !observed {
                        continue;
                    }
                    let g: [i32; 3] = [gx, gy, gz];
                    let b = vol.ensure(g.map(|c| c.div_euclid(SIDE)));
                    let i = b * N + local(g.map(|c| c.rem_euclid(SIDE)));
                    vol.sdf[i] = (d / t).clamp(-1.0, 1.0);
                    vol.weight[i] = 1.0;
                }
            }
        }
        vol
    }

    fn roi() -> (WalkPath, Roi) {
        let path = WalkPath { feet: vec![[-2.0, 0.0, 0.0], [1.5, 0.0, 0.0]], eye_height: 1.6, source: "test" };
        let roi = Roi::new(&path, 2.8, 1.5, 3.0);
        (path, roi)
    }

    fn holes(vol: &Volume) -> usize {
        let m = mc::extract(vol);
        m.audit().boundary_edges
    }

    #[test]
    fn closes_a_small_enclosed_hole_and_the_path() {
        let (path, roi) = roi();
        // A 0.6 m hole off the path, and the floor under the path's start never observed.
        let mut vol = scene(|x, z| (x + 0.5).abs() < 0.3 && (z - 1.5).abs() < 0.3 || (x + 2.0).abs() < 0.25 && z.abs() < 0.25, false);
        let before = holes(&vol);
        let s = floors(&mut vol, &path, &roi);
        assert!(s.holes_filled >= 1 && s.hole_columns > 100, "{s:?}");
        let after = holes(&vol);
        assert!(after < before, "{before} -> {after}");
        // Filled at the floor's height.
        let g = [(-0.5f32 / 0.05) as i32, 0, (1.5f32 / 0.05) as i32];
        assert!(vol.get([g[0], -1, g[2]]).unwrap().0 < 0.0 && vol.get([g[0], 1, g[2]]).unwrap().0 > 0.0);
    }

    #[test]
    fn leaves_a_hole_it_saw_through() {
        let (path, roi) = roi();
        let mut vol = scene(|x, z| (x + 0.5).abs() < 0.3 && (z - 1.5).abs() < 0.3, true);
        let s = floors(&mut vol, &path, &roi);
        assert_eq!(s.holes_filled, 0, "{s:?}");
        assert!(s.holes_seen_through >= 1, "{s:?}");
    }

    #[test]
    fn plane_fit() {
        let pts: Vec<_> = (0..10).flat_map(|x| (0..10).map(move |z| (x as f32, z as f32, 0.1 * x as f32 - 0.2 * z as f32 + 3.0))).collect();
        let (p, rms, worst) = plane(&pts).unwrap();
        assert!((p[0] - 0.1).abs() < 1e-6 && (p[1] + 0.2).abs() < 1e-6 && (p[2] - 3.0).abs() < 1e-5 && rms < 1e-6 && worst < 1e-5);
    }
}
