//! Lab: depth samples that say nothing. The capture's decoder stores a tile it could not read (post-processing such as
//! chromatic aberration smears its symbols, worst towards the frame's edges) as code 0 — the same as "nothing there"
//! — and misread symbols decode to wrong depths. Treated as measurements, the first become sky discs 400 m out painted
//! with whatever the keyframes show there (smears from other viewpoints), count as "seen through" for the transient
//! detector, and the second seed discs in front of the camera (specks). Marked unknown instead (DepthView::dynamic),
//! they are neither seeds, depth targets nor free-space evidence. Three rules:
//! - tile positions away from the frame's centre that read nothing in any depth frame of the session (a tile looking
//!   at the sky now and then still reads now and then: a camera that turns sees something there), unless the world
//!   directions they look along read as nothing in the tiles that do decode, too (an avatar against an empty sky:
//!   those edges are sky, not failures);
//! - tiles whose samples mostly disagree with their neighbours (a misread tile: real depth is piecewise smooth);
//! - samples nearer than `min_depth` (closer than any surface the player can stand at).

use crate::dataset::{Dataset, DepthView};
use orbiters_contracts::depth::{SAMPLES_X, SAMPLES_Y};

#[derive(Clone, Debug)]
pub struct FixParams {
    /// Tile positions whose centre lies farther than this share of the half-width from the frame's centre may be
    /// "never reads" positions.
    pub min_offset: f32,
    /// Fewest depth frames in the session before "never reads" means anything.
    pub min_frames: usize,
    /// A sample disagrees with a neighbour when they differ by more than this share of its depth; a tile is misread
    /// when more than `max_disagreeing` of its samples disagree with every valid neighbour.
    pub neighbour_tolerance: f32,
    pub max_disagreeing: f32,
    pub min_depth: f32,
    /// A never-read sample stays "nothing there" when the decoding tiles saw its world direction at least
    /// `min_direction_votes` times and hit something in at most `max_direction_hits` of them.
    pub min_direction_votes: u32,
    pub max_direction_hits: f32,
}

impl Default for FixParams {
    fn default() -> Self {
        Self { min_offset: 0.4, min_frames: 20, neighbour_tolerance: 0.25, max_disagreeing: 0.3, min_depth: 0.3, min_direction_votes: 20, max_direction_hits: 0.2 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FixStats {
    pub dead_tiles: usize,
    pub tiles: usize,
    /// Samples of never-read tile positions kept as sky (their directions read nothing where tiles decode).
    pub sky_samples: usize,
    pub misread_tiles: usize,
    pub near_samples: usize,
    pub unknown_samples: usize,
    pub samples: usize,
}

fn tiles_of(d: &DepthView) -> (usize, usize) {
    (d.grid_width as usize / SAMPLES_X as usize, d.grid_height as usize / SAMPLES_Y as usize)
}

/// Marks untrusted samples of every depth frame as unknown (`dynamic`), keeping marks already there.
pub fn mark_untrusted(data: &mut Dataset, p: &FixParams) -> FixStats {
    let mut stats = FixStats::default();
    let Some(first) = data.depth_views.first() else { return stats };
    let (tx, ty) = tiles_of(first);
    let (sx, sy) = (SAMPLES_X as usize, SAMPLES_Y as usize);
    let gw = first.grid_width as usize;
    // Rule 1: per tile position, the frames in which it read anything.
    let mut reads = vec![0usize; tx * ty];
    let mut frames = 0usize;
    for d in &data.depth_views {
        if tiles_of(d) != (tx, ty) {
            continue;
        }
        frames += 1;
        for t in 0..tx * ty {
            let (ix, iy) = (t % tx, t / tx);
            let any = (0..sy).any(|y| (0..sx).any(|x| d.depth[(iy * sy + y) * gw + ix * sx + x] > 0.0));
            if any {
                reads[t] += 1;
            }
        }
    }
    let centre = (tx as f32 - 1.0) / 2.0;
    let dead: Vec<bool> = (0..tx * ty).map(|t| frames >= p.min_frames && reads[t] == 0 && ((t % tx) as f32 - centre).abs() > p.min_offset * (centre + 0.5)).collect();
    stats.tiles = tx * ty;
    stats.dead_tiles = dead.iter().filter(|x| **x).count();
    // What the decoding tiles saw along each world direction (1° bins): hits and votes.
    let tile_of = |i: usize| (i / gw / sy) * tx + (i % gw) / sx;
    let mut hits = vec![0u32; DIR_BINS];
    let mut votes = vec![0u32; DIR_BINS];
    if stats.dead_tiles > 0 {
        for d in &data.depth_views {
            if tiles_of(d) != (tx, ty) {
                continue;
            }
            for i in 0..d.depth.len() {
                if dead[tile_of(i)] || d.is_dynamic(i) {
                    continue;
                }
                let b = direction_bin(d, i);
                votes[b] += 1;
                hits[b] += u32::from(d.depth[i] > 0.0);
            }
        }
    }
    for d in data.depth_views.iter_mut() {
        let n = d.depth.len();
        if d.dynamic.len() != n {
            d.dynamic = vec![false; n];
        }
        stats.samples += n;
        if tiles_of(d) != (tx, ty) {
            continue;
        }
        let depth = d.depth.clone();
        let at = |x: i64, y: i64| -> Option<f32> {
            if x < 0 || y < 0 || x >= (tx * sx) as i64 || y >= (ty * sy) as i64 {
                return None;
            }
            let z = depth[y as usize * gw + x as usize];
            (z > 0.0).then_some(z)
        };
        for t in 0..tx * ty {
            let (ix, iy) = (t % tx, t / tx);
            let samples: Vec<usize> = (0..sy).flat_map(|y| (0..sx).map(move |x| (iy * sy + y) * gw + ix * sx + x)).collect();
            if dead[t] {
                for &i in &samples {
                    let b = direction_bin(d, i);
                    if votes[b] >= p.min_direction_votes && hits[b] as f32 <= p.max_direction_hits * votes[b] as f32 {
                        stats.sky_samples += 1;
                    } else {
                        d.dynamic[i] = true;
                    }
                }
                continue;
            }
            // Rule 2: a misread tile.
            let (mut valid, mut odd) = (0usize, 0usize);
            for &i in &samples {
                let Some(z) = at((i % gw) as i64, (i / gw) as i64) else { continue };
                valid += 1;
                let (x, y) = ((i % gw) as i64, (i / gw) as i64);
                let near: Vec<f32> = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().filter_map(|(dx, dy)| at(x + dx, y + dy)).collect();
                if !near.is_empty() && near.iter().all(|&o| (o - z).abs() > p.neighbour_tolerance * z) {
                    odd += 1;
                }
            }
            if valid >= 6 && odd as f32 > p.max_disagreeing * valid as f32 {
                stats.misread_tiles += 1;
                for &i in &samples {
                    d.dynamic[i] = true;
                }
                continue;
            }
            // Rule 3: impossibly near.
            for &i in &samples {
                if depth[i] > 0.0 && depth[i] < p.min_depth {
                    stats.near_samples += 1;
                    d.dynamic[i] = true;
                }
            }
        }
        stats.unknown_samples += d.dynamic.iter().filter(|x| **x).count();
    }
    stats
}

const DIR_BINS: usize = 360 * 180;

/// 1° azimuth × elevation bin of the world direction sample `i` of a depth frame looks along.
fn direction_bin(d: &DepthView, i: usize) -> usize {
    let o = d.camera.position();
    let q = d.camera.unproject(d.pixels[i][0], d.pixels[i][1], 1.0);
    let v = [q[0] - o[0], q[1] - o[1], q[2] - o[2]];
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    let az = v[2].atan2(v[0]).to_degrees() + 180.0;
    let el = (v[1] / len).clamp(-1.0, 1.0).asin().to_degrees() + 90.0;
    (el.floor() as usize).min(179) * 360 + (az.floor() as usize) % 360
}
