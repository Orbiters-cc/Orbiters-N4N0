//! Depth samples that say nothing. Captures before the decoder marked unreadable tiles
//! (orbiters_contracts::depth::UNKNOWN) stored them as code 0 — the same as "nothing there" — and in worlds whose
//! post-processing smears the symbols (chromatic aberration, worst towards the frame's edges) misread symbols still
//! decode to wrong depths. Treated as measurements, the first become sky discs 400 m out painted with whatever the
//! keyframes show there (smears from other viewpoints), and the second seed discs in front of the camera (specks).
//! Marked unknown instead (DepthView::unknown), they are neither seeds nor depth targets. Three rules:
//! - tile positions left or right of the frame's centre (farther than `min_offset` of the half-width) that read
//!   nothing in any depth frame of the session (a tile looking at the sky now and then still reads now and then: a
//!   camera that turns sees something there), unless the world directions they look along read as nothing in the
//!   tiles that do decode, too (an avatar against an empty sky: those edges are sky, not failures). Only on frames
//!   whose zeros may be samples nobody read (`.d16` version 1), and only in sessions whose decoder never marked a
//!   tile unknown: one that does tells its failures from the sky by itself, as version 2 files always do, and there
//!   the rule could only turn real sky into unknown;
//! - tiles whose samples mostly disagree with their neighbours (a misread tile: real depth is piecewise smooth);
//! - samples nearer than `min_depth` (closer than any surface the player can stand at).
//!
//! Samples unknown already (tiles under the capture strip's area, which frames never carry, and the decoder's
//! unknowns) neither vote nor count as reads.

use crate::camera::Camera;
use crate::dataset::{Dataset, DepthView, Marks};
use orbiters_contracts::depth::{SAMPLES_PER_TILE, SAMPLES_X, SAMPLES_Y};
use rayon::prelude::*;

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
    /// Samples unknown after validation, the decoder's included (tiles frames never carry are not counted).
    pub unknown_samples: usize,
    /// Samples the frames could carry.
    pub samples: usize,
    /// Samples the capture's decoder marked unknown.
    pub decoder_unknown: usize,
    /// Whether the "never reads" rule ran (sessions whose decoder marks nothing unknown).
    pub never_read_rule: bool,
}

const SX: usize = SAMPLES_X as usize;
const SY: usize = SAMPLES_Y as usize;

/// Whether a depth frame's samples match its grid (anything else is left alone).
fn well_formed(d: &DepthView) -> bool {
    d.depth.len() == d.grid.len() && d.grid.len() == d.grid.width as usize * d.grid.height as usize
}

/// Tiles across and down a frame's grid (samples past whole tiles belong to none).
fn tiles_of(d: &DepthView) -> (usize, usize) {
    (d.grid.width as usize / SX, d.grid.height as usize / SY)
}

/// Sample indices of tile `t` of a grid `gw` samples wide with `tx` tiles across.
fn tile_samples(t: usize, tx: usize, gw: usize) -> impl Iterator<Item = usize> {
    let (ix, iy) = (t % tx, t / tx);
    (0..SY).flat_map(move |y| (0..SX).map(move |x| (iy * SY + y) * gw + ix * SX + x))
}

/// Per tile position of the frames of grid size `size` whose zeros may be unread: whether it reads nothing in any of
/// them although it is not entirely unknown, away from the frame's centre (rule 1).
fn never_read(views: &[DepthView], size: (u32, u32), p: &FixParams) -> Vec<bool> {
    let frames: Vec<&DepthView> = views.iter().filter(|d| well_formed(d) && d.zero_may_be_unread && (d.grid.width, d.grid.height) == size).collect();
    let Some(first) = frames.first() else { return Vec::new() };
    let (tx, ty) = tiles_of(first);
    let gw = size.0 as usize;
    // Per tile position: frames in which it read anything, and in which it had a sample that says something.
    let (reads, known) = frames
        .par_iter()
        .fold(
            || (vec![0usize; tx * ty], vec![0usize; tx * ty]),
            |(mut reads, mut known), d| {
                for t in 0..tx * ty {
                    let (mut read, mut any) = (false, false);
                    for i in tile_samples(t, tx, gw) {
                        if !d.is_unknown(i) {
                            any = true;
                            read |= d.depth[i] > 0.0;
                        }
                    }
                    reads[t] += usize::from(read);
                    known[t] += usize::from(any);
                }
                (reads, known)
            },
        )
        .reduce(|| (vec![0; tx * ty], vec![0; tx * ty]), |(mut a, mut b), (c, d)| {
            a.iter_mut().zip(c).for_each(|(x, y)| *x += y);
            b.iter_mut().zip(d).for_each(|(x, y)| *x += y);
            (a, b)
        });
    let centre = (tx as f32 - 1.0) / 2.0;
    (0..tx * ty).map(|t| frames.len() >= p.min_frames && reads[t] == 0 && known[t] > 0 && ((t % tx) as f32 - centre).abs() > p.min_offset * (centre + 0.5)).collect()
}

/// What validation found in one frame.
#[derive(Default)]
struct FrameCounts {
    sky: usize,
    misread: usize,
    near: usize,
    unknown: usize,
    samples: usize,
}

/// Marks untrusted samples of every depth frame as unknown (`unknown`), keeping marks already there.
pub fn mark_untrusted(data: &mut Dataset, p: &FixParams) -> FixStats {
    let mut stats = FixStats { decoder_unknown: data.depth_views.iter().map(|d| d.decoder_unknown).sum(), ..Default::default() };
    // Old files only, and only when their decoder marked no tile unknown (65535 in a version 1 file is the decoder's
    // unknown, or a depth of exactly 0.1 m from before contracts 0.4.0: hardly ever a whole tile of them).
    let old: Vec<&DepthView> = data.depth_views.iter().filter(|d| d.zero_may_be_unread).collect();
    stats.never_read_rule = !old.is_empty() && old.iter().map(|d| d.decoder_unknown).sum::<usize>() < SAMPLES_PER_TILE as usize;
    // Rule 1 per grid size: frames of one size share their tile positions.
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    for d in data.depth_views.iter().filter(|d| well_formed(d)) {
        if !sizes.contains(&(d.grid.width, d.grid.height)) {
            sizes.push((d.grid.width, d.grid.height));
        }
    }
    let dead: Vec<((u32, u32), Vec<bool>)> = sizes.iter().map(|&s| (s, if stats.never_read_rule { never_read(&data.depth_views, s, p) } else { Vec::new() })).collect();
    for (s, v) in &dead {
        stats.tiles += (s.0 as usize / SX) * (s.1 as usize / SY);
        stats.dead_tiles += v.iter().filter(|x| **x).count();
    }
    // What the decoding tiles saw along each world direction (1° bins): hits and votes.
    let (hits, votes) = if stats.dead_tiles > 0 {
        data.depth_views
            .par_iter()
            .filter(|d| well_formed(d))
            .fold(
                || (vec![0u32; DIR_BINS], vec![0u32; DIR_BINS]),
                |(mut hits, mut votes), d| {
                    let (dead, (tx, _)) = (dead_of_size(&dead, d), tiles_of(d));
                    let gw = d.grid.width as usize;
                    let origin = d.camera.position();
                    for i in 0..d.depth.len() {
                        let tile = (i / gw / SY) * tx + (i % gw) / SX;
                        if ((i % gw) / SX < tx && dead.get(tile).copied().unwrap_or(false)) || d.is_unknown(i) {
                            continue;
                        }
                        let b = direction_bin(&d.camera, origin, d.grid.pixels[i]);
                        votes[b] += 1;
                        hits[b] += u32::from(d.depth[i] > 0.0);
                    }
                    (hits, votes)
                },
            )
            .reduce(|| (vec![0; DIR_BINS], vec![0; DIR_BINS]), |(mut a, mut b), (c, d)| {
                a.iter_mut().zip(c).for_each(|(x, y)| *x += y);
                b.iter_mut().zip(d).for_each(|(x, y)| *x += y);
                (a, b)
            })
    } else {
        (Vec::new(), Vec::new())
    };
    let counts: Vec<FrameCounts> = data.depth_views.par_iter_mut().map(|d| validate(d, dead_of_size(&dead, d), &hits, &votes, p)).collect();
    for c in counts {
        stats.sky_samples += c.sky;
        stats.misread_tiles += c.misread;
        stats.near_samples += c.near;
        stats.unknown_samples += c.unknown;
        stats.samples += c.samples;
    }
    stats
}

fn dead_of_size<'a>(dead: &'a [((u32, u32), Vec<bool>)], d: &DepthView) -> &'a [bool] {
    dead.iter().find(|(s, _)| *s == (d.grid.width, d.grid.height)).map_or(&[], |(_, v)| v.as_slice())
}

/// Rules 1 to 3 on one frame.
fn validate(d: &mut DepthView, dead: &[bool], hits: &[u32], votes: &[u32], p: &FixParams) -> FrameCounts {
    let mut c = FrameCounts::default();
    let n = d.depth.len();
    let carried = |d: &DepthView, i: usize| !d.grid.unused.contains(i);
    c.samples = (0..n).filter(|&i| carried(d, i)).count();
    if !well_formed(d) {
        c.unknown = (0..n).filter(|&i| d.is_unknown(i) && carried(d, i)).count();
        return c;
    }
    let (tx, ty) = tiles_of(d);
    let gw = d.grid.width as usize;
    let (gx_end, gy_end) = ((tx * SX) as i64, (ty * SY) as i64);
    // Rule 2 reads the frame as it came: marks it adds do not change its neighbours' verdicts.
    let before: Marks = d.unknown.clone();
    let at = |x: i64, y: i64| -> Option<f32> {
        if x < 0 || y < 0 || x >= gx_end || y >= gy_end {
            return None;
        }
        let i = y as usize * gw + x as usize;
        let z = d.depth[i];
        (z > 0.0 && !before.contains(i)).then_some(z)
    };
    let origin = d.camera.position();
    let mut marks: Vec<usize> = Vec::new();
    for t in 0..tx * ty {
        if dead.get(t).copied().unwrap_or(false) {
            for i in tile_samples(t, tx, gw) {
                if before.contains(i) {
                    continue;
                }
                let b = direction_bin(&d.camera, origin, d.grid.pixels[i]);
                if votes.get(b).is_some_and(|&v| v >= p.min_direction_votes && hits[b] as f32 <= p.max_direction_hits * v as f32) {
                    c.sky += 1;
                } else {
                    marks.push(i);
                }
            }
            continue;
        }
        // Rule 2: a misread tile.
        let (mut valid, mut odd) = (0usize, 0usize);
        for i in tile_samples(t, tx, gw) {
            let (x, y) = ((i % gw) as i64, (i / gw) as i64);
            let Some(z) = at(x, y) else { continue };
            valid += 1;
            let (mut neighbours, mut agree) = (0, false);
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                if let Some(o) = at(x + dx, y + dy) {
                    neighbours += 1;
                    agree |= (o - z).abs() <= p.neighbour_tolerance * z;
                }
            }
            if neighbours > 0 && !agree {
                odd += 1;
            }
        }
        if valid >= 6 && odd as f32 > p.max_disagreeing * valid as f32 {
            c.misread += 1;
            marks.extend(tile_samples(t, tx, gw));
            continue;
        }
        // Rule 3: impossibly near.
        for i in tile_samples(t, tx, gw) {
            if d.depth[i] > 0.0 && d.depth[i] < p.min_depth && !before.contains(i) {
                c.near += 1;
                marks.push(i);
            }
        }
    }
    for i in marks {
        d.unknown.insert(i);
    }
    c.unknown = (0..n).filter(|&i| d.is_unknown(i) && carried(d, i)).count();
    c
}

const DIR_BINS: usize = 360 * 180;

/// 1° azimuth × elevation bin of the world direction a depth sample at `pixel` looks along (`origin`: the camera's
/// position).
fn direction_bin(camera: &Camera, origin: [f32; 3], pixel: [f32; 2]) -> usize {
    let q = camera.unproject(pixel[0], pixel[1], 1.0);
    let v = [q[0] - origin[0], q[1] - origin[1], q[2] - origin[2]];
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    let az = v[2].atan2(v[0]).to_degrees() + 180.0;
    let el = (v[1] / len).clamp(-1.0, 1.0).asin().to_degrees() + 90.0;
    // NaN (a broken pose) lands in bin 0, which nothing else uses alone.
    (el.max(0.0).floor() as usize).min(179) * 360 + (az.max(0.0).floor() as usize) % 360
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{Camera, Geometry};
    use crate::dataset::DepthGrid;
    use std::sync::Arc;

    const W: u32 = 960;
    const H: u32 = 500;
    /// 16 × 10 tiles of 6 × 5 samples.
    const GW: usize = 96;
    const GH: usize = 50;

    /// The 96 × 50 sample grid, with `unused` tiles (column, row) that frames never carry.
    fn grid(unused: impl Fn(usize, usize) -> bool) -> Arc<DepthGrid> {
        let mut g = DepthGrid { width: GW as u32, height: GH as u32, spacing: 10.0, ..Default::default() };
        for gy in 0..GH {
            for gx in 0..GW {
                g.pixels.push([(gx as f32 + 0.5) * W as f32 / GW as f32, (gy as f32 + 0.5) * H as f32 / GH as f32]);
                if unused(gx / SX, gy / SY) {
                    g.unused.insert(gy * GW + gx);
                }
            }
        }
        Arc::new(g)
    }

    /// A camera at eye height 1.6 m over a floor (and under a ceiling 4 m up, with `ceiling`), turning 1° a frame:
    /// samples below the horizon hit the floor, the others the sky or the ceiling. (The edge tiles that only ever see
    /// sky do not vote, so the sky directions need enough turns through the central tiles.)
    fn frame(k: usize, grid: &Arc<DepthGrid>, ceiling: bool) -> DepthView {
        let yaw = (k as f32).to_radians();
        let camera = Camera::look([0.0, 1.6, 0.0], [yaw.sin(), 0.0, yaw.cos()], [0.0, 1.0, 0.0], 1.0, W, H);
        let depth = grid
            .pixels
            .iter()
            .map(|&[u, v]| {
                // At eye depth 1 the ray has dropped `1.6 - y` metres: the floor is 1.6 / that away (eye depth).
                let drop = 1.6 - camera.unproject(u, v, 1.0)[1];
                if drop > 1e-3 {
                    (1.6 / drop).min(900.0)
                } else if ceiling && drop < -1e-3 {
                    (2.4 / -drop).min(900.0)
                } else {
                    0.0
                }
            })
            .collect();
        // As old sessions stored them (`.d16` version 1: a tile that did not decode left zeros).
        DepthView { camera, grid: grid.clone(), depth, game_time: k as f32, unknown: grid.unused.clone(), decoder_unknown: 0, zero_may_be_unread: true }
    }

    fn dataset(depth_views: Vec<DepthView>) -> Dataset {
        Dataset {
            dir: Default::default(),
            views: Vec::new(),
            depth_views,
            width: W,
            height: H,
            geometry: Geometry::new(W, H, 1),
            mask: vec![1.0; (W * H) as usize],
            loaded: Default::default(),
        }
    }

    /// The rightmost tile column never decodes (as chromatic aberration does it).
    fn kill_right_column(views: &mut [DepthView]) {
        for d in views.iter_mut() {
            for gy in 0..GH {
                for gx in GW - 6..GW {
                    d.depth[gy * GW + gx] = 0.0;
                }
            }
        }
    }

    #[test]
    fn edges_that_never_read_floor_are_unknown_sky_stays_sky() {
        let g = grid(|_, _| false);
        let mut views: Vec<DepthView> = (0..360).map(|k| frame(k, &g, false)).collect();
        kill_right_column(&mut views);
        // One misread tile (symbol errors scale depths by up to 20× either way), one impossibly near sample.
        let misread = 10;
        let mut rng = 3u32;
        for gy in 40..45 {
            for gx in 24..30 {
                rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let factor = 20f32.powf((rng >> 16) as f32 / 32768.0 - 1.0);
                views[misread].depth[gy * GW + gx] *= factor;
            }
        }
        views[20].depth[45 * GW + 50] = 0.2;
        let mut data = dataset(views);
        let stats = mark_untrusted(&mut data, &FixParams::default());
        assert!(stats.never_read_rule && stats.dead_tiles >= 10, "{stats:?}");
        let d = &data.depth_views[180];
        let floor = d.depth.iter().position(|z| *z > 0.0).unwrap() / GW;
        // The dead column: unknown where the floor is, still sky above the horizon.
        assert!((floor + 1..GH).all(|gy| d.is_unknown(gy * GW + GW - 2)), "below the horizon");
        assert!((0..floor.saturating_sub(5)).all(|gy| !d.is_unknown(gy * GW + GW - 2) && d.depth[gy * GW + GW - 2] == 0.0), "above the horizon");
        // Elsewhere the floor is trusted.
        assert!(!d.is_unknown(45 * GW + 40));
        assert!((40..45).all(|gy| (24..30).all(|gx| data.depth_views[misread].is_unknown(gy * GW + gx))), "misread tile");
        assert!(!data.depth_views[misread + 1].is_unknown(42 * GW + 26));
        assert!(data.depth_views[20].is_unknown(45 * GW + 50), "nearer than 0.3 m");
        assert_eq!(stats.unknown_samples, data.depth_views.iter().map(|d| d.unknown.len()).sum::<usize>());
        assert_eq!(stats.samples, 360 * GW * GH);
    }

    /// A session whose decoder marks unreadable tiles 65535 tells failures from the sky itself, as `.d16` version 2
    /// files always do: the "never reads" rule stays off (it could only turn real sky into unknown), the other rules
    /// still run.
    #[test]
    fn a_decoder_that_marks_unknowns_turns_the_never_read_rule_off() {
        let g = grid(|_, _| false);
        let mut views: Vec<DepthView> = (0..360).map(|k| frame(k, &g, false)).collect();
        kill_right_column(&mut views);
        // One tile the decoder could not read.
        for i in tile_samples(0, 16, GW) {
            views[7].unknown.insert(i);
        }
        views[7].decoder_unknown = SAMPLES_PER_TILE as usize;
        views[9].depth[45 * GW + 50] = 0.2;
        let mut data = dataset(views);
        let stats = mark_untrusted(&mut data, &FixParams::default());
        assert!(!stats.never_read_rule && stats.dead_tiles == 0, "{stats:?}");
        assert_eq!(stats.decoder_unknown, 30);
        let d = &data.depth_views[180];
        assert!((0..GH).all(|gy| !d.is_unknown(gy * GW + GW - 2)));
        assert!(data.depth_views[9].is_unknown(45 * GW + 50), "rule 3 still runs");
        assert_eq!(stats.unknown_samples, 31);
        // Version 2 files: zeros are "nothing there", whatever the decoder marked.
        let mut views: Vec<DepthView> = (0..360).map(|k| DepthView { zero_may_be_unread: false, ..frame(k, &g, false) }).collect();
        kill_right_column(&mut views);
        let mut data = dataset(views);
        let stats = mark_untrusted(&mut data, &FixParams::default());
        assert!(!stats.never_read_rule && stats.dead_tiles == 0, "{stats:?}");
        assert!(!data.depth_views[180].is_unknown(45 * GW + GW - 2));
    }

    /// Tiles under the capture strip's area read nothing in every frame: they are not a failing edge, and their
    /// zeros are no votes for "nothing there" (which would keep the dead edge's misses along those directions as sky).
    #[test]
    fn strip_tiles_cast_no_votes() {
        // A floor and a ceiling: every direction hits something. The strip covers tile columns 1–14 of rows 0–3.
        let strip = |c: usize, r: usize| (1..=14).contains(&c) && r <= 3;
        let with = grid(strip);
        let mut views: Vec<DepthView> = (0..360)
            .map(|k| {
                let mut d = frame(k, &with, true);
                for i in 0..d.depth.len() {
                    if d.grid.unused.contains(i) {
                        d.depth[i] = 0.0;
                    }
                }
                d
            })
            .collect();
        kill_right_column(&mut views);
        let mut data = dataset(views);
        let stats = mark_untrusted(&mut data, &FixParams::default());
        assert!(stats.dead_tiles >= 10, "{stats:?}");
        let d = &data.depth_views[90];
        // The dead edge's misses under the ceiling are unknown, not sky.
        assert!((5..13).all(|gy| d.is_unknown(gy * GW + GW - 3)), "{stats:?}");
        // The same frames without the strip's tiles marked: their zeros vote, and the edge's misses turn into sky.
        let plain = grid(|_, _| false);
        let views: Vec<DepthView> = data.depth_views.iter().map(|d| DepthView { depth: d.depth.clone(), ..frame(d.game_time as usize, &plain, true) }).collect();
        let mut data = dataset(views);
        mark_untrusted(&mut data, &FixParams::default());
        assert!((5..13).any(|gy| !data.depth_views[90].is_unknown(gy * GW + GW - 3)), "the mechanism this guards against");
    }

    /// Fewer depth frames than `min_frames`: "never reads" means nothing yet.
    #[test]
    fn a_short_session_has_no_dead_tiles() {
        let g = grid(|_, _| false);
        let mut views: Vec<DepthView> = (0..10).map(|k| frame(k, &g, false)).collect();
        kill_right_column(&mut views);
        let mut data = dataset(views);
        let stats = mark_untrusted(&mut data, &FixParams::default());
        assert_eq!(stats.dead_tiles, 0);
        assert!(!data.depth_views[5].is_unknown(45 * GW + GW - 2));
    }

    /// Frames of two grid sizes are validated per size, and a frame whose samples do not match its grid is left as
    /// it is instead of being read out of bounds.
    #[test]
    fn mixed_and_malformed_grids() {
        let big = grid(|_, _| false);
        let small = Arc::new(DepthGrid {
            width: 48,
            height: 25,
            pixels: (0..25).flat_map(|y| (0..48).map(move |x| [x as f32 * 20.0 + 10.0, y as f32 * 20.0 + 10.0])).collect(),
            spacing: 20.0,
            unused: Marks::default(),
        });
        let mut views: Vec<DepthView> = (0..30).map(|k| frame(k, &big, false)).collect();
        views.extend((0..30).map(|k| frame(k, &small, false)));
        views[40].depth[12 * 48 + 20] = 0.1;
        // Too few samples for its grid, then too many.
        let mut short = frame(3, &big, false);
        short.depth.truncate(100);
        views.push(short);
        let mut long = frame(4, &small, false);
        long.depth.extend([1.0; 7]);
        views.push(long);
        let mut data = dataset(views);
        let stats = mark_untrusted(&mut data, &FixParams::default());
        assert_eq!(stats.tiles, 16 * 10 + 8 * 5);
        assert!(data.depth_views[40].is_unknown(12 * 48 + 20), "rule 3 on the second grid");
        assert!(data.depth_views[60].unknown.is_empty() && data.depth_views[61].unknown.is_empty());
    }

    /// A session whose every sample the decoder could not read: nothing to vote with, nothing to mark, nothing breaks.
    #[test]
    fn an_all_unknown_session() {
        let g = grid(|_, _| false);
        let mut views: Vec<DepthView> = (0..40).map(|k| frame(k, &g, false)).collect();
        for d in views.iter_mut() {
            for i in 0..d.depth.len() {
                d.depth[i] = 0.0;
                d.unknown.insert(i);
            }
            d.decoder_unknown = d.depth.len();
        }
        let mut data = dataset(views);
        let stats = mark_untrusted(&mut data, &FixParams::default());
        assert!(!stats.never_read_rule);
        assert_eq!((stats.dead_tiles, stats.sky_samples, stats.unknown_samples), (0, 0, stats.samples));
        assert!(data.depth_views.iter().all(|d| d.valid() == 0));
    }
}
