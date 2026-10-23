//! The sky the capture never saw. Training keeps the sky shell (crate::init::SKY out) only where some view looked;
//! above the highest gaze and in the directions nobody faced, nothing is left, and a viewer looking there sees black.
//! [`complete`] bins the far-field Gaussians by direction from the cameras' centre (5.6° cells) and backs every cell
//! from the horizon up that they do not cover whole with a large opaque Gaussian a tenth beyond the shell, coloured
//! like the cell's own sky where it was seen, else like the nearest seen sky of its elevation band (else the nearest
//! band that has any): the gradient of the seen sky carries on instead of a hole, and behind a sparsely seen cell its
//! gaps show sky instead of the black training background (the dark streaks along the edge of what was seen).
//!
//! By default the sky is a map instead ([`SkyMap`], `sky.png` beside the PLY): seeded from the keyframe pixels whose
//! rays hit nothing ([`seed_map`]), trained behind the splats (crate::gpu::Gpu::set_sky), and drawn behind them by
//! every viewer. The shell's discs give way to it only in the directions a parallax test confirms as sky
//! ([`give_way`]); the completion is for the shell alone.

use crate::dataset::{sample_rgb, Dataset, DepthGrid};
use crate::gaussians::{logit, sigmoid, Gaussians};
use anyhow::Context as _;
use std::path::Path;

const AZIMUTHS: usize = 64;
const ELEVATIONS: usize = 32;
/// Cells from this elevation (degrees) up are completed; lower ones lie behind the ground anyway.
const LOWEST_DEG: f32 = -6.0;
/// Opacity a cell needs (summed over its Gaussians) for its own colour to count as seen.
const SEEN: f32 = 0.5;
/// Coverage (opacity times footprint, 2π σ₁σ₂ a Gaussian, over the cell's solid angle) at which a cell needs no backing.
const COVERED: f32 = 2.0;
/// The backing lies this much farther out than the shell, so that it shows only where the shell does not cover.
const BEHIND: f32 = 1.1;

/// The colour of the sky at the horizon (encoded sRGB), for a viewer's fog (crate::gpu::FarField): the sky shell's
/// Gaussians from 2° below to 12° above the horizon, seen from the middle of the rest, by opacity; None without any.
pub fn horizon(g: &Gaussians) -> Option<[f32; 3]> {
    let ground: Vec<usize> = (0..g.len()).filter(|&i| !g.sky.get(i).copied().unwrap_or(false) && g.means[i].iter().all(|c| c.is_finite())).collect();
    if ground.is_empty() {
        return None;
    }
    let mut centre = [0f32; 3];
    for (axis, c) in centre.iter_mut().enumerate() {
        let mut v: Vec<f32> = ground.iter().map(|&i| g.means[i][axis]).collect();
        let k = v.len() / 2;
        *c = *v.select_nth_unstable_by(k, f32::total_cmp).1;
    }
    let (mut sum, mut weight) = ([0f64; 3], 0f64);
    for i in (0..g.len()).filter(|&i| g.sky.get(i).copied().unwrap_or(false)) {
        let d = [g.means[i][0] - centre[0], g.means[i][1] - centre[1], g.means[i][2] - centre[2]];
        let r = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if r <= 0.0 {
            continue;
        }
        let elevation = (d[1] / r).clamp(-1.0, 1.0).asin().to_degrees();
        if (-2.0..=12.0).contains(&elevation) {
            let w = sigmoid(g.opacity_logits[i]) as f64;
            for (s, c) in sum.iter_mut().zip(g.colors[i]) {
                *s += w * c.clamp(0.0, 1.0) as f64;
            }
            weight += w;
        }
    }
    (weight > 0.0).then(|| sum.map(|s| (s / weight) as f32))
}

fn cell(dir: [f32; 3]) -> (usize, usize) {
    let az = dir[0].atan2(dir[2]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
    let el = dir[1].clamp(-1.0, 1.0).asin() / std::f32::consts::PI + 0.5;
    (((az * AZIMUTHS as f32) as usize).min(AZIMUTHS - 1), ((el * ELEVATIONS as f32) as usize).min(ELEVATIONS - 1))
}

/// Adds Gaussians where the sky shell is empty; returns how many. `far_field` is the distance from `centre` beyond
/// which Gaussians count as sky (or the far horizon), `distance` where the new ones go.
pub fn complete(g: &mut Gaussians, centre: [f32; 3], far_field: f32, distance: f32) -> usize {
    let mut sum = vec![[0f32; 3]; AZIMUTHS * ELEVATIONS];
    let mut weight = vec![0f32; AZIMUTHS * ELEVATIONS];
    let mut cover = vec![0f32; AZIMUTHS * ELEVATIONS];
    for i in 0..g.len() {
        let d = [g.means[i][0] - centre[0], g.means[i][1] - centre[1], g.means[i][2] - centre[2]];
        let r = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if r < far_field {
            continue;
        }
        let (a, e) = cell([d[0] / r, d[1] / r, d[2] / r]);
        let w = sigmoid(g.opacity_logits[i]);
        for (total, colour) in sum[e * AZIMUTHS + a].iter_mut().zip(g.colors[i]) {
            *total += w * colour;
        }
        weight[e * AZIMUTHS + a] += w;
        // Its footprint's solid angle on the shell, by its two largest axes.
        let mut s = g.log_scales[i].map(f32::exp);
        s.sort_by(|x, y| y.total_cmp(x));
        cover[e * AZIMUTHS + a] += w * std::f32::consts::TAU * s[0] * s[1] / (r * r);
    }
    // Each cell's solid angle.
    let solid = |e: usize| {
        let el = ((e as f32 + 0.5) / ELEVATIONS as f32 - 0.5) * std::f32::consts::PI;
        (std::f32::consts::TAU / AZIMUTHS as f32) * (std::f32::consts::PI / ELEVATIONS as f32) * el.cos().max(0.05)
    };
    let covered = |a: usize, e: usize| cover[e * AZIMUTHS + a] >= COVERED * solid(e);
    let seen = |a: usize, e: usize| weight[e * AZIMUTHS + a] >= SEEN;
    let colour = |a: usize, e: usize| sum[e * AZIMUTHS + a].map(|s| s / weight[e * AZIMUTHS + a]);
    // Each band's colour: its seen cells' mean; a band with none takes the nearest band that has some.
    let mut band: Vec<Option<[f32; 3]>> = (0..ELEVATIONS)
        .map(|e| {
            let cells: Vec<[f32; 3]> = (0..AZIMUTHS).filter(|&a| seen(a, e)).map(|a| colour(a, e)).collect();
            (!cells.is_empty()).then(|| [0, 1, 2].map(|c| cells.iter().map(|x| x[c]).sum::<f32>() / cells.len() as f32))
        })
        .collect();
    if band.iter().all(Option::is_none) {
        return 0;
    }
    let known = band.clone();
    for (e, b) in band.iter_mut().enumerate() {
        if b.is_none() {
            *b = (1..ELEVATIONS).flat_map(|k| [e.checked_sub(k), Some(e + k)]).flatten().filter(|&j| j < ELEVATIONS).find_map(|j| known[j]);
        }
    }
    let lowest = (((LOWEST_DEG / 180.0 + 0.5) * ELEVATIONS as f32) as usize).min(ELEVATIONS - 1);
    let mut added = 0;
    for (e, band_colour) in band.iter().enumerate().skip(lowest) {
        let el = ((e as f32 + 0.5) / ELEVATIONS as f32 - 0.5) * std::f32::consts::PI;
        for a in 0..AZIMUTHS {
            if covered(a, e) {
                continue;
            }
            // The cell's own sky; else the nearest seen cell of the band, fading into the band's colour over a quarter
            // turn.
            let nearest = (0..=AZIMUTHS / 2)
                .find_map(|k| [(a + k) % AZIMUTHS, (a + AZIMUTHS - k) % AZIMUTHS].into_iter().find(|&j| seen(j, e)).map(|j| (k, colour(j, e))));
            let base = band_colour.expect("every band has a colour");
            let c = match nearest {
                Some((k, near)) => {
                    let t = (k as f32 / (AZIMUTHS as f32 / 4.0)).min(1.0);
                    [0, 1, 2].map(|i| near[i] + (base[i] - near[i]) * t)
                }
                None => base,
            };
            let az = (a as f32 + 0.5) / AZIMUTHS as f32 * std::f32::consts::TAU;
            let dir = [el.cos() * az.sin(), el.sin(), el.cos() * az.cos()];
            let r = distance * BEHIND;
            let p = [centre[0] + dir[0] * r, centre[1] + dir[1] * r, centre[2] + dir[2] * r];
            // Wide enough to overlap its neighbours: the cell's larger side.
            let size = (std::f32::consts::PI / ELEVATIONS as f32).max(std::f32::consts::TAU / AZIMUTHS as f32 * el.cos()) * r * 0.7;
            g.push(p, [size.ln(); 3], [1.0, 0.0, 0.0, 0.0], logit(0.98), c);
            // Sky, as the seeds on the shell are (crate::gaussians): never a relocation source, fitted as sky.
            *g.sky.last_mut().expect("pushed") = true;
            added += 1;
        }
    }
    added
}

/// The sky as an equirectangular map behind the splats (crate::gpu::Gpu::set_sky, shaders/sky_lookup.wgsl) in place
/// of the shell of discs crate::init::SKY out, which slides by f·b/R pixels as the eye moves b (31 px over a 10 m walk
/// at 1233 px) and never densifies. `width` × `width / 2` texels of encoded sRGB, longitude `atan2(x, z)` across from
/// −180°, elevation from +90° down. On the off-path benchmark (2026-10-22, three runs each) it gained 0.10 dB off-path,
/// 1.26 dB looking up and 0.56 dB on held-out keyframes on the plaza walk, 0.13 dB on VRChat Home and 0.96 dB on
/// Canine with half the splats; Exoplanet was neutral (−0.06 dB).
#[derive(Clone, Debug, PartialEq)]
pub struct SkyMap {
    pub width: u32,
    pub rgb: Vec<[f32; 3]>,
}

/// What seeding a [`SkyMap`] found: keyframe pixels whose rays hit nothing in the depth frame nearest in time, the
/// texels they reached, and of the 5.6° cells (a map [`PARALLAX_CELLS`] across) seen from both halves of the walk
/// (split at its median position along its longest extent) those whose colour differs between the halves: finite
/// content without depth shows parallax. `sky_cells` marks the cells (a map [`PARALLAX_CELLS`] across) the test found
/// the same from both halves: true sky, which the map may take over. `texels` marks the texels seeding saw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkySeeding {
    pub pixels: usize,
    pub texels_seen: usize,
    pub cells_tested: usize,
    pub cells_parallax: usize,
    pub sky_cells: Vec<bool>,
    pub texels: Vec<bool>,
}

/// A cell whose colour differs by more than this (0–1, any channel) between the walk's halves shows parallax.
const PARALLAX: f64 = 0.05;
/// Pixels a cell needs from each half of the walk to be tested.
const TESTED: f64 = 16.0;
/// Cells across of the parallax test (texels are too fine: few are seen from both halves).
pub const PARALLAX_CELLS: usize = 64;

/// The texel holding direction `d` on a map `width` texels across (the one whose square holds it).
pub fn texel(d: [f32; 3], width: usize) -> usize {
    let h = width / 2;
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-12);
    let u = (d[0].atan2(d[2]) / std::f32::consts::TAU + 0.5) * width as f32;
    let v = (0.5 - (d[1] / len).clamp(-1.0, 1.0).asin() / std::f32::consts::PI) * h as f32;
    let x = (u.floor() as isize).rem_euclid(width as isize) as usize;
    let y = (v.floor().max(0.0) as usize).min(h - 1);
    y * width + x
}

/// A depth grid's samples as a regular grid: first pixel, spacing, size (crate::train::depth_target reads it alike).
struct Regular {
    x0: f32,
    y0: f32,
    sx: f32,
    sy: f32,
    w: usize,
    h: usize,
}

impl Regular {
    fn of(g: &DepthGrid) -> Option<Self> {
        let (w, h) = (g.width as usize, g.height as usize);
        if w < 2 || h < 2 || g.pixels.len() != w * h {
            return None;
        }
        let (x0, y0) = (g.pixels[0][0], g.pixels[0][1]);
        let (sx, sy) = ((g.pixels[w - 1][0] - x0) / (w - 1) as f32, (g.pixels[(h - 1) * w][1] - y0) / (h - 1) as f32);
        (sx > 0.0 && sy > 0.0).then_some(Self { x0, y0, sx, sy, w, h })
    }
}

/// Seeds a map `width` texels across from the training keyframes (not `held_out`): each pixel (every second one each
/// way) whose ray hit nothing at the four samples around it in the depth frame nearest in time (far along the ray, a
/// depth frame a few centimetres away sees the same direction) adds its colour to its texel. Texels nobody saw take
/// their row's mean (a sky changes most with elevation), rows nobody saw the nearest seen row's.
pub fn seed_map(data: &Dataset, width: u32, held_out: &[usize]) -> (SkyMap, SkySeeding) {
    let (w, h) = (width as usize, width as usize / 2);
    let positions: Vec<[f32; 3]> = data.views.iter().map(|v| v.camera.position()).collect();
    let extent = |k: usize| {
        let (lo, hi) = positions.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[k]), hi.max(p[k])));
        hi - lo
    };
    let axis = if extent(0) >= extent(2) { 0 } else { 2 };
    let mut along: Vec<f32> = positions.iter().map(|p| p[axis]).collect();
    along.sort_by(f32::total_cmp);
    let split = along.get(along.len() / 2).copied().unwrap_or(0.0);
    let mut sum = vec![[0f64; 4]; w * h];
    let mut cells = vec![[[0f64; 4]; 2]; PARALLAX_CELLS * PARALLAX_CELLS / 2];
    let mut found = SkySeeding::default();
    let (iw, ih) = (data.width, data.height);
    let parallel = std::env::var_os("S4_PAR_SKY").is_some();
    for (i, v) in data.views.iter().enumerate() {
        if held_out.contains(&i) {
            continue;
        }
        if parallel {
            // Lab: each row's sky pixels found in parallel, then added up row by row, pixel by pixel, as below.
            let Some(d) = data.depth_views.iter().min_by(|a, b| (a.game_time - v.game_time).abs().total_cmp(&(b.game_time - v.game_time).abs())) else { continue };
            let Some(g) = Regular::of(&d.grid) else { continue };
            let origin = v.camera.position();
            let half = usize::from(positions[i][axis] > split);
            use rayon::prelude::*;
            let rows: Vec<Vec<(usize, usize, [f32; 3])>> = (0..ih)
                .into_par_iter()
                .step_by(2)
                .map(|y| {
                    let mut hits = Vec::new();
                    for x in (0..iw).step_by(2) {
                        if data.mask[(y * iw + x) as usize] < 0.5 {
                            continue;
                        }
                        let far = v.camera.unproject(x as f32, y as f32, 1e4);
                        let Some((u, vv, _)) = d.camera.project(far) else { continue };
                        let (fx, fy) = ((u - g.x0) / g.sx, (vv - g.y0) / g.sy);
                        if !(fx >= 0.0 && fy >= 0.0 && fx <= (g.w - 1) as f32 && fy <= (g.h - 1) as f32) {
                            continue;
                        }
                        let (i0, j0) = ((fx as usize).min(g.w - 2), (fy as usize).min(g.h - 2));
                        let sky = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().all(|&(di, dj)| {
                            let k = (j0 + dj) * g.w + i0 + di;
                            d.depth[k] <= 0.0 && !d.is_unknown(k)
                        });
                        if !sky {
                            continue;
                        }
                        let Some(c) = sample_rgb(&v.image, iw, ih, x as f32, y as f32) else { continue };
                        let dir = [far[0] - origin[0], far[1] - origin[1], far[2] - origin[2]];
                        hits.push((texel(dir, w), texel(dir, PARALLAX_CELLS), c));
                    }
                    hits
                })
                .collect();
            for (t0, t1, c) in rows.into_iter().flatten() {
                for t in [&mut sum[t0], &mut cells[t1][half]] {
                    for k in 0..3 {
                        t[k] += c[k] as f64;
                    }
                    t[3] += 1.0;
                }
                found.pixels += 1;
            }
            continue;
        }
        let Some(d) = data.depth_views.iter().min_by(|a, b| (a.game_time - v.game_time).abs().total_cmp(&(b.game_time - v.game_time).abs())) else { continue };
        let Some(g) = Regular::of(&d.grid) else { continue };
        let origin = v.camera.position();
        let half = usize::from(positions[i][axis] > split);
        for y in (0..ih).step_by(2) {
            for x in (0..iw).step_by(2) {
                if data.mask[(y * iw + x) as usize] < 0.5 {
                    continue;
                }
                let far = v.camera.unproject(x as f32, y as f32, 1e4);
                let Some((u, vv, _)) = d.camera.project(far) else { continue };
                let (fx, fy) = ((u - g.x0) / g.sx, (vv - g.y0) / g.sy);
                if !(fx >= 0.0 && fy >= 0.0 && fx <= (g.w - 1) as f32 && fy <= (g.h - 1) as f32) {
                    continue;
                }
                let (i0, j0) = ((fx as usize).min(g.w - 2), (fy as usize).min(g.h - 2));
                let sky = [(0, 0), (1, 0), (0, 1), (1, 1)].iter().all(|&(di, dj)| {
                    let k = (j0 + dj) * g.w + i0 + di;
                    d.depth[k] <= 0.0 && !d.is_unknown(k)
                });
                if !sky {
                    continue;
                }
                let Some(c) = sample_rgb(&v.image, iw, ih, x as f32, y as f32) else { continue };
                let dir = [far[0] - origin[0], far[1] - origin[1], far[2] - origin[2]];
                for t in [&mut sum[texel(dir, w)], &mut cells[texel(dir, PARALLAX_CELLS)][half]] {
                    for k in 0..3 {
                        t[k] += c[k] as f64;
                    }
                    t[3] += 1.0;
                }
                found.pixels += 1;
            }
        }
    }
    let mut seen: Vec<Option<[f32; 3]>> = vec![None; w * h];
    for (t, s) in sum.iter().enumerate() {
        if s[3] > 0.0 {
            seen[t] = Some([0, 1, 2].map(|k| (s[k] / s[3]) as f32));
            found.texels_seen += 1;
        }
    }
    found.texels = seen.iter().map(Option::is_some).collect();
    found.sky_cells = vec![false; cells.len()];
    for (c, s) in cells.iter().enumerate() {
        if s[0][3] >= TESTED && s[1][3] >= TESTED {
            found.cells_tested += 1;
            if (0..3).any(|k| (s[0][k] / s[0][3] - s[1][k] / s[1][3]).abs() > PARALLAX) {
                found.cells_parallax += 1;
            } else {
                found.sky_cells[c] = true;
            }
        }
    }
    let row_mean: Vec<Option<[f32; 3]>> = (0..h)
        .map(|y| {
            let row: Vec<[f32; 3]> = seen[y * w..(y + 1) * w].iter().flatten().copied().collect();
            (!row.is_empty()).then(|| [0, 1, 2].map(|k| row.iter().map(|c| c[k]).sum::<f32>() / row.len() as f32))
        })
        .collect();
    let fill: Vec<[f32; 3]> = (0..h).map(|y| (0..h).filter_map(|r| row_mean[r].map(|m| (r.abs_diff(y), m))).min_by_key(|(dist, _)| *dist).map_or([0.5; 3], |(_, m)| m)).collect();
    let rgb = seen.iter().enumerate().map(|(t, c)| c.unwrap_or(fill[t / w])).collect();
    (SkyMap { width, rgb }, found)
}

/// The rows of the band the horizon's colour is read from: from 2° below the horizon to 12° above, as
/// [`horizon`] reads the shell.
const HORIZON_DEG: std::ops::RangeInclusive<f32> = -2.0..=12.0;

/// A row of the map counts in [`SkyMap::profile`] when this many of its texels are known (and a hundredth of the row
/// at least).
const ROW_KNOWN: usize = 8;
/// [`SkyMap::profile`] averages the known texels of the rows within this many degrees of each row.
const SMOOTH_DEG: f32 = 2.0;
/// [`SkyMap::profile`] reads how the sky's brightness changes with elevation from the top this many degrees of the
/// rows that set it...
const TREND_DEG: f32 = 8.0;
/// ...and carries that change on above them, fading it out over this many degrees...
const TREND_FADE_DEG: f32 = 10.0;
/// ...by a factor between e^−0.3 and e^0.3 at most.
const TREND_MAX: f32 = 0.3;
/// [`SkyMap::fill`] carries the known texels' differences from the sky's colour this many degrees from them,
/// fading out (e^−1 at this distance).
const CARRY_DEG: f32 = 3.0;
/// Rounds of neighbour averaging per pyramid level in [`spread`].
const RELAX: usize = 24;

/// Each texel's distance from the nearest `known` one, in texels of a map `w` × `h` (chamfer 3-4 over two sweeps
/// each way, around in longitude; none known: infinite).
fn distance_to_known(known: &[bool], w: usize, h: usize) -> Vec<f32> {
    let far = u32::MAX / 2;
    let mut d: Vec<u32> = known.iter().map(|&k| if k { 0 } else { far }).collect();
    let left = |x: usize| (x + w - 1) % w;
    let right = |x: usize| (x + 1) % w;
    for _ in 0..2 {
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let mut v = d[i].min(d[y * w + left(x)] + 3);
                if y > 0 {
                    let up = (y - 1) * w;
                    v = v.min(d[up + left(x)] + 4).min(d[up + x] + 3).min(d[up + right(x)] + 4);
                }
                d[i] = v;
            }
        }
        for y in (0..h).rev() {
            for x in (0..w).rev() {
                let i = y * w + x;
                let mut v = d[i].min(d[y * w + right(x)] + 3);
                if y + 1 < h {
                    let down = (y + 1) * w;
                    v = v.min(d[down + right(x)] + 4).min(d[down + x] + 3).min(d[down + left(x)] + 4);
                }
                d[i] = v;
            }
        }
    }
    d.into_iter().map(|v| if v >= far { f32::INFINITY } else { v as f32 / 3.0 }).collect()
}

/// One level of [`spread`]'s pyramid: each texel's value and how much of it is known (0 to 1).
struct Level {
    w: usize,
    h: usize,
    rgb: Vec<[f32; 3]>,
    weight: Vec<f32>,
}

impl Level {
    /// The value at texel coordinates (`x`, `y`), bilinear, around in longitude and clamped at the poles.
    fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let col = |i: f32| (i as isize).rem_euclid(self.w as isize) as usize;
        let row = |j: f32| (j.max(0.0) as usize).min(self.h - 1);
        let at = |i: f32, j: f32| self.rgb[row(j) * self.w + col(i)];
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
        [0, 1, 2].map(|k| (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy)
    }
}

/// The values of a map `w` × `h` texels whose `known` ones are set, carried smoothly into the others: a pyramid of the
/// known values averaged two by two, then from its top down each texel keeps its own value by the share of it that is
/// known and takes the coarser level's (bilinear, around in longitude) for the rest, and the texels nothing reached
/// take their neighbours' mean a few times over. Known texels keep their values.
fn spread(values: Vec<[f32; 3]>, known: &[bool], w: usize, h: usize) -> Vec<[f32; 3]> {
    let mut levels = vec![Level { w, h, rgb: values, weight: known.iter().map(|&k| if k { 1.0 } else { 0.0 }).collect() }];
    while let Some(p) = levels.last().filter(|l| l.w >= 4 && l.h >= 2) {
        let (cw, ch) = (p.w / 2, p.h / 2);
        let (mut rgb, mut weight) = (vec![[0f32; 3]; cw * ch], vec![0f32; cw * ch]);
        for y in 0..ch {
            for x in 0..cw {
                let (mut sum, mut total) = ([0f32; 3], 0f32);
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let i = (2 * y + dy) * p.w + 2 * x + dx;
                    for (s, v) in sum.iter_mut().zip(p.rgb[i]) {
                        *s += v * p.weight[i];
                    }
                    total += p.weight[i];
                }
                if total > 0.0 {
                    rgb[y * cw + x] = sum.map(|s| s / total);
                }
                weight[y * cw + x] = total.min(1.0);
            }
        }
        levels.push(Level { w: cw, h: ch, rgb, weight });
    }
    // The top: what no known texel reaches takes the known texels' mean (nothing known: zero).
    let top = levels.last_mut().expect("level 0");
    let total: f32 = top.weight.iter().sum();
    let mean = if total > 0.0 { [0, 1, 2].map(|k| top.rgb.iter().zip(&top.weight).map(|(c, a)| c[k] * a).sum::<f32>() / total) } else { [0.0; 3] };
    for (c, a) in top.rgb.iter_mut().zip(&mut top.weight) {
        *c = [0, 1, 2].map(|k| *a * c[k] + (1.0 - *a) * mean[k]);
        *a = 1.0;
    }
    for l in (0..levels.len() - 1).rev() {
        let (fine, coarse) = levels.split_at_mut(l + 1);
        let (f, c) = (&mut fine[l], &coarse[0]);
        let mut unknown = Vec::new();
        for y in 0..f.h {
            for x in 0..f.w {
                let i = y * f.w + x;
                let a = f.weight[i];
                if a < 1.0 {
                    let up = c.sample((x as f32 + 0.5) / 2.0 - 0.5, (y as f32 + 0.5) / 2.0 - 0.5);
                    f.rgb[i] = [0, 1, 2].map(|k| a * f.rgb[i][k] + (1.0 - a) * up[k]);
                    f.weight[i] = 1.0;
                    if a == 0.0 {
                        unknown.push(i);
                    }
                }
            }
        }
        // The coarser levels' averages reach into the known texels: next to them the values would step. Each texel
        // nothing reached at this level takes its neighbours' mean a few times over, so the fill starts where the known
        // texels end.
        for _ in 0..RELAX {
            for &i in &unknown {
                let (x, y) = (i % f.w, i / f.w);
                let (left, right) = (y * f.w + (x + f.w - 1) % f.w, y * f.w + (x + 1) % f.w);
                let (up, down) = (y.saturating_sub(1) * f.w + x, (y + 1).min(f.h - 1) * f.w + x);
                f.rgb[i] = [0, 1, 2].map(|k| 0.25 * (f.rgb[left][k] + f.rgb[right][k] + f.rgb[up][k] + f.rgb[down][k]));
            }
        }
    }
    std::mem::take(&mut levels[0].rgb)
}

/// The shell's discs give way to the map in the cells the parallax test found to be sky (`found.sky_cells`, by
/// direction from `centre`, the cameras' mean position); elsewhere (finite content without depth, directions the test
/// could not judge) they stay in front of it. Returns how many left.
pub fn give_way(g: &mut Gaussians, found: &SkySeeding, centre: [f32; 3]) -> usize {
    let keep: Vec<bool> = (0..g.len())
        .map(|i| {
            let m = g.means[i];
            !g.sky[i] || !found.sky_cells.get(texel([m[0] - centre[0], m[1] - centre[1], m[2] - centre[2]], PARALLAX_CELLS)).copied().unwrap_or(false)
        })
        .collect();
    let before = g.len();
    g.retain(&keep);
    before - g.len()
}

impl SkyMap {
    /// Fills the texels neither seeding nor training saw (`known` false) from those that were, without the edges
    /// seeding's row means left (the trained sky met them at an edge above the highest view, and in each row beside
    /// what the views saw). Each texel takes the sky's colour at its elevation ([`SkyMap::profile`]) plus the known
    /// texels' differences from it, carried smoothly into the rest ([`spread`]) and fading out away from them. Below the
    /// lowest rows seen the colour stays theirs: a fade to black there cost 1.35 dB on Canine's held-out keyframes and
    /// 0.16 dB on the plaza's (2026-10-22), whose views see the map through the floor's last transparency.
    pub fn fill(&mut self, known: &[bool]) {
        let (w, h) = (self.width as usize, self.width as usize / 2);
        assert_eq!(known.len(), w * h, "a known flag per texel");
        let Some(profile) = self.profile(known) else { return };
        let residual = (0..w * h).map(|t| if known[t] { [0, 1, 2].map(|k| self.rgb[t][k] - profile[t / w][k]) } else { [0.0; 3] }).collect();
        let carried = spread(residual, known, w, h);
        let distance = distance_to_known(known, w, h);
        let degrees = 180.0 / h as f32;
        for t in 0..w * h {
            if !known[t] {
                let keep = (-distance[t] * degrees / CARRY_DEG).exp();
                self.rgb[t] = [0, 1, 2].map(|k| (profile[t / w][k] + keep * carried[t][k]).clamp(0.0, 1.0));
            }
        }
    }

    /// The sky's colour by elevation, one per row, from the `known` texels of the rows with [`ROW_KNOWN`] of them (a
    /// hundredth of the row at least): from the highest such row to the lowest, the mean of those within
    /// [`SMOOTH_DEG`] (every texel counting once, so a sparse row at the edge of what the views saw sets neither a line
    /// across the map nor the zenith: the plaza walk's two highest rows were 12 codes darker than the rows below them),
    /// linear across wider gaps; below, the lowest row's colour; above, the highest row's colour carried on with the
    /// change of brightness the top [`TREND_DEG`] show (a weighted fit of the logarithm of the mean of R, G and B
    /// against elevation), fading that change out over [`TREND_FADE_DEG`] and holding it within e^±[`TREND_MAX`]: a
    /// daylight sky keeps darkening towards the zenith, where the row means stopped at the highest view's colour (the
    /// plaza's true sky is 0.84 times as bright 24° above it). `None` without such a row.
    fn profile(&self, known: &[bool]) -> Option<Vec<[f32; 3]>> {
        let (w, h) = (self.width as usize, self.width as usize / 2);
        let elevation = |y: usize| 90.0 - (y as f32 + 0.5) / h as f32 * 180.0;
        let enough = ROW_KNOWN.max(w / 100);
        let rows: Vec<Option<([f32; 3], usize)>> = (0..h)
            .map(|y| {
                let (mut sum, mut n) = ([0f64; 3], 0usize);
                for (c, _) in self.rgb[y * w..(y + 1) * w].iter().zip(&known[y * w..(y + 1) * w]).filter(|(_, k)| **k) {
                    for (s, v) in sum.iter_mut().zip(c) {
                        *s += *v as f64;
                    }
                    n += 1;
                }
                (n >= enough).then(|| (sum.map(|s| (s / n as f64) as f32), n))
            })
            .collect();
        let top = rows.iter().position(Option::is_some)?;
        let bottom = rows.iter().rposition(Option::is_some)?;
        let span = (SMOOTH_DEG * h as f32 / 180.0).round() as usize;
        let smoothed: Vec<Option<[f32; 3]>> = (top..=bottom)
            .map(|y| {
                let (mut sum, mut n) = ([0f64; 3], 0usize);
                for (c, k) in rows[y.saturating_sub(span).max(top)..=(y + span).min(bottom)].iter().flatten() {
                    for j in 0..3 {
                        sum[j] += c[j] as f64 * *k as f64;
                    }
                    n += k;
                }
                (n > 0).then(|| sum.map(|s| (s / n as f64) as f32))
            })
            .collect();
        let mut out = vec![[0f32; 3]; h];
        let mut last: Option<(usize, [f32; 3])> = None;
        for (i, row) in smoothed.iter().enumerate() {
            let y = top + i;
            let Some(c) = *row else { continue };
            if let Some((l, lc)) = last {
                for (r, o) in out.iter_mut().enumerate().take(y).skip(l + 1) {
                    let f = (r - l) as f32 / (y - l) as f32;
                    *o = [0, 1, 2].map(|k| lc[k] + (c[k] - lc[k]) * f);
                }
            }
            out[y] = c;
            last = Some((y, c));
        }
        for y in bottom + 1..h {
            out[y] = out[bottom];
        }
        let summit = elevation(top);
        let out_top = out[top];
        let (mut sw, mut sx, mut sy, mut sxx, mut sxy, mut fitted) = (0f64, 0f64, 0f64, 0f64, 0f64, 0usize);
        for (y, row) in rows.iter().enumerate().skip(top) {
            let e = elevation(y);
            if e < summit - TREND_DEG {
                break;
            }
            let Some((c, n)) = *row else { continue };
            let b = ((c[0] + c[1] + c[2]) / 3.0).max(1e-3).ln() as f64;
            let (x, wt) = (e as f64, n as f64);
            sw += wt;
            sx += wt * x;
            sy += wt * b;
            sxx += wt * x * x;
            sxy += wt * x * b;
            fitted += 1;
        }
        let det = sw * sxx - sx * sx;
        let slope = if fitted >= 3 && det > 0.0 { ((sw * sxy - sx * sy) / det) as f32 } else { 0.0 };
        let change = (slope * TREND_FADE_DEG).clamp(-TREND_MAX, TREND_MAX);
        for (y, o) in out.iter_mut().enumerate().take(top) {
            let gain = (change * (1.0 - (-(elevation(y) - summit) / TREND_FADE_DEG).exp())).exp();
            *o = out_top.map(|v| (v * gain).clamp(0.0, 1.0));
        }
        Some(out)
    }

    /// The colour of the sky at the horizon (encoded sRGB), for a viewer's fog (crate::gpu::FarField): the mean of the
    /// texel rows from 2° below the horizon to 12° above.
    pub fn horizon(&self) -> [f32; 3] {
        let (w, h) = (self.width as usize, self.width as usize / 2);
        let (mut sum, mut n) = ([0f64; 3], 0usize);
        for y in 0..h {
            let elevation = 90.0 - (y as f32 + 0.5) / h as f32 * 180.0;
            if !HORIZON_DEG.contains(&elevation) {
                continue;
            }
            for c in &self.rgb[y * w..(y + 1) * w] {
                for (s, v) in sum.iter_mut().zip(c) {
                    *s += v.clamp(0.0, 1.0) as f64;
                }
            }
            n += w;
        }
        sum.map(|s| (s / n.max(1) as f64) as f32)
    }

    /// Writes the map as an 8-bit sRGB PNG (orbiters_core::reconstruction::SKY_FILE, contracts Docs/session-bundle.md).
    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let file = std::fs::File::create(path).with_context(|| format!("writing {}", path.display()))?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.width / 2);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let bytes: Vec<u8> = self.rgb.iter().flat_map(|c| c.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
        let mut writer = encoder.write_header().with_context(|| format!("writing {}", path.display()))?;
        writer.write_image_data(&bytes).with_context(|| format!("writing {}", path.display()))?;
        writer.finish().with_context(|| format!("writing {}", path.display()))
    }

    /// Reads a map [`SkyMap::write`] wrote (any 8-bit PNG twice as wide as high, a width a multiple of 4).
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let file = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
        let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().with_context(|| format!("reading {}", path.display()))?;
        let mut buf = vec![0; reader.output_buffer_size().context("a sky map too large")?];
        let info = reader.next_frame(&mut buf).with_context(|| format!("reading {}", path.display()))?;
        let (width, height) = (info.width, info.height);
        anyhow::ensure!(width >= 4 && width % 4 == 0 && width == 2 * height, "{}: a sky map is twice as wide as high ({width} × {height})", path.display());
        let channels = match info.color_type {
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            other => anyhow::bail!("{}: a sky map is RGB, not {other:?}", path.display()),
        };
        let rgb = buf[..info.buffer_size()].chunks_exact(channels).map(|p| [0, 1, 2].map(|c| p[c] as f32 / 255.0)).collect();
        Ok(Self { width, rgb })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_cells_above_the_horizon_get_the_seen_sky_band_colour() {
        let mut g = Gaussians::default();
        // A seen ring of blue sky at 10° elevation, all the way round.
        for a in 0..AZIMUTHS * 2 {
            let az = a as f32 / (AZIMUTHS * 2) as f32 * std::f32::consts::TAU;
            let el = 10f32.to_radians();
            g.push([el.cos() * az.sin() * 400.0, el.sin() * 400.0, el.cos() * az.cos() * 400.0], [1.0; 3], [1.0, 0.0, 0.0, 0.0], logit(0.99), [0.3, 0.5, 0.9]);
        }
        let before = g.len();
        let added = complete(&mut g, [0.0; 3], 200.0, 400.0);
        assert!(added > AZIMUTHS * 10, "{added}");
        // Straight up is now sky, coloured like the seen band.
        let up = (before..g.len()).find(|&i| g.means[i][1] > 399.0).expect("a Gaussian near the zenith");
        assert!((g.colors[up][2] - 0.9).abs() < 1e-4 && (g.colors[up][0] - 0.3).abs() < 1e-4);
        // Nothing far below the horizon, and all behind the shell.
        assert!((before..g.len()).all(|i| g.means[i][1] > -440.0 * (LOWEST_DEG - 6.0).to_radians().sin().abs()));
        assert!((before..g.len()).all(|i| (g.means[i].iter().map(|c| c * c).sum::<f32>().sqrt() - 440.0).abs() < 1.0));
    }

    /// A cell whose Gaussians are opaque enough to count as seen but cover a sliver of it is backed in its own colour:
    /// its gaps showed the black training background.
    #[test]
    fn sparsely_seen_cells_are_backed_in_their_own_colour() {
        let mut g = Gaussians::default();
        // A covering ring at 10° elevation, a single small red disc at 40°.
        for a in 0..AZIMUTHS * 2 {
            let az = a as f32 / (AZIMUTHS * 2) as f32 * std::f32::consts::TAU;
            let el = 10f32.to_radians();
            g.push(
                [el.cos() * az.sin() * 400.0, el.sin() * 400.0, el.cos() * az.cos() * 400.0],
                [(30.0f32).ln(); 3],
                [1.0, 0.0, 0.0, 0.0],
                logit(0.99),
                [0.3, 0.5, 0.9],
            );
        }
        let el = 40f32.to_radians();
        g.push([0.0, el.sin() * 400.0, el.cos() * 400.0], [(2.0f32).ln(); 3], [1.0, 0.0, 0.0, 0.0], logit(0.99), [0.9, 0.1, 0.1]);
        let before = g.len();
        complete(&mut g, [0.0; 3], 200.0, 400.0);
        let (a, e) = cell([0.0, el.sin(), el.cos()]);
        let backing = (before..g.len())
            .find(|&i| {
                let m = g.means[i];
                let r = m.iter().map(|c| c * c).sum::<f32>().sqrt();
                cell([m[0] / r, m[1] / r, m[2] / r]) == (a, e)
            })
            .expect("the red cell is backed");
        assert!((g.colors[backing][0] - 0.9).abs() < 1e-4, "{:?}", g.colors[backing]);
        // The covered ring needs none.
        let ring = cell([0.0, 10f32.to_radians().sin(), 10f32.to_radians().cos()]);
        assert!(!(before..g.len()).any(|i| {
            let m = g.means[i];
            let r = m.iter().map(|c| c * c).sum::<f32>().sqrt();
            cell([m[0] / r, m[1] / r, m[2] / r]) == ring
        }));
    }

    /// The map's texels as the shader's bilinear taps find them: a direction at a texel's centre lands in that texel.
    #[test]
    fn map_texels_follow_longitude_and_elevation() {
        let w = 64;
        // Straight ahead (+z) is the middle column; up is row 0; west (−x) the first quarter.
        assert_eq!(texel([0.0, 0.0, 1.0], w) % w, w / 2);
        assert_eq!(texel([0.0, 1.0, 0.0], w) / w, 0);
        assert_eq!(texel([0.0, -1.0, 0.0], w) / w, w / 2 - 1);
        assert_eq!(texel([-1.0, 0.0, 0.0], w) % w, w / 4);
        assert_eq!(texel([0.0, 0.0, -1.0], w) % w, 0);
    }

    /// sky.png keeps each texel to the nearest of 256 levels (out of range clamped), rows from the top.
    #[test]
    fn a_sky_map_round_trips_through_its_png() {
        let map = SkyMap { width: 8, rgb: (0..32).map(|i| [i as f32 / 31.0, 0.5, if i < 8 { -1.0 } else { 2.0 }]).collect() };
        let path = std::env::temp_dir().join(format!("orbiters-sky-{}.png", std::process::id()));
        map.write(&path).unwrap();
        let back = SkyMap::read(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(back.width, 8);
        for (a, b) in map.rgb.iter().zip(&back.rgb) {
            for c in 0..3 {
                assert!((a[c].clamp(0.0, 1.0) - b[c]).abs() <= 0.5 / 255.0 + 1e-6, "{a:?} {b:?}");
            }
        }
    }

    /// The filled sky carries on from what was seen without an edge, and what was seen stays as it was.
    #[test]
    fn the_fill_carries_on_from_what_was_seen() {
        let (w, h) = (64usize, 32usize);
        // Rows 8 to 15 seen (45° down to the horizon), lighter towards the horizon; the zenith and the ground not.
        let value = |y: usize| 0.3 + 0.02 * (y as f32 - 8.0);
        let known: Vec<bool> = (0..w * h).map(|t| (8..16).contains(&(t / w))).collect();
        let rgb = (0..w * h).map(|t| if known[t] { [value(t / w); 3] } else { [0.9, 0.0, 0.9] }).collect();
        let mut map = SkyMap { width: w as u32, rgb };
        map.fill(&known);
        for t in (0..w * h).filter(|&t| known[t]) {
            assert_eq!(map.rgb[t], [value(t / w); 3], "seen texels stay");
        }
        // Just above the seen rows, the sky goes on as the highest seen row has it.
        for x in 0..w {
            let above = map.rgb[7 * w + x][0];
            assert!((above - value(8)).abs() < 0.03, "row 7, column {x}: {above}");
        }
        // Straight up, the seen sky's colour, never the seeding placeholder.
        assert!(map.rgb.iter().take(w).all(|c| (0.25..0.5).contains(&c[0]) && c[1] == c[0]), "{:?}", map.rgb[0]);
    }

    /// Below the lowest rows seen, what nobody saw keeps their colour, never fading out (a fade to black cost 1.35 dB on
    /// Canine's held-out keyframes, which see the map through the floor); a texel seen far below stays as it was.
    #[test]
    fn below_the_lowest_rows_seen_the_sky_keeps_their_colour() {
        let (w, h) = (64usize, 32usize);
        let elevation = |y: usize| 90.0 - (y as f32 + 0.5) / h as f32 * 180.0;
        let mut known: Vec<bool> = (0..w * h).map(|t| elevation(t / w) > 0.0).collect();
        let mut rgb: Vec<[f32; 3]> = (0..w * h).map(|t| if known[t] { [0.6; 3] } else { [0.0; 3] }).collect();
        // One texel seen far below the horizon (a void seen through a gap).
        let deep = 28 * w + 5;
        known[deep] = true;
        rgb[deep] = [0.9, 0.2, 0.1];
        let mut map = SkyMap { width: w as u32, rgb };
        map.fill(&known);
        assert_eq!(map.rgb[deep], [0.9, 0.2, 0.1]);
        for y in 0..h {
            let c = map.rgb[y * w + 37];
            if elevation(y) > 0.0 {
                assert_eq!(c, [0.6; 3]);
            } else {
                assert!(c.iter().all(|v| (v - 0.6).abs() < 0.05), "row {y} at {}°: {c:?}", elevation(y));
            }
        }
    }

    /// A sky that darkens towards the top of the views keeps darkening above them by the same change of brightness,
    /// fading out (the plaza's true sky goes on darkening; the row means stopped at the highest view's colour); a
    /// sky that seems to brighten fast at the top of the views brightens by e^0.3 at most.
    #[test]
    fn the_zenith_carries_the_trend_of_the_highest_rows_on() {
        let (w, h) = (128usize, 64usize);
        let elevation = |y: usize| 90.0 - (y as f32 + 0.5) / h as f32 * 180.0;
        let known: Vec<bool> = (0..w * h).map(|t| (0.0..30.0).contains(&elevation(t / w))).collect();
        let top = (0..h).find(|&y| (0.0..30.0).contains(&elevation(y))).expect("a seen row");
        let span = (SMOOTH_DEG * h as f32 / 180.0).round() as usize;
        for (rate, change) in [(-0.015f32, -0.15f32), (0.08, 0.3)] {
            let value = |e: f32| if rate < 0.0 { 0.5 } else { 0.05 } * (rate * e).exp();
            let rgb: Vec<[f32; 3]> = (0..w * h).map(|t| if known[t] { [value(elevation(t / w)); 3] } else { [0.9, 0.0, 0.9] }).collect();
            let mut map = SkyMap { width: w as u32, rgb };
            map.fill(&known);
            for t in (0..w * h).filter(|&t| known[t]) {
                assert_eq!(map.rgb[t], [value(elevation(t / w)); 3], "seen texels stay");
            }
            // Above the views: the top rows' colour (the rows within SMOOTH_DEG, every row full), then the trend.
            let base = (top..=top + span).map(|y| value(elevation(y))).sum::<f32>() / (span + 1) as f32;
            let mut previous = base;
            for y in (0..top).rev() {
                let c = map.rgb[y * w + 3];
                assert!(c[0] == c[1] && c[1] == c[2], "row {y}: {c:?}");
                assert!(if rate < 0.0 { c[0] <= previous + 1e-6 } else { c[0] >= previous - 1e-6 }, "row {y}: {} after {previous}", c[0]);
                previous = c[0];
            }
            let zenith = map.rgb[3][0];
            let expected = base * (change * (1.0 - (-(elevation(0) - elevation(top)) / TREND_FADE_DEG).exp())).exp();
            assert!((zenith - expected).abs() < 0.01 * expected, "rate {rate}: zenith {zenith}, expected {expected}");
        }
    }

    /// A sparse row at the top of what the views saw, darker than the rows below it (the plaza walk's two highest rows
    /// were 12 codes darker), sets neither a line across the map nor the sky above: the rest of its row and the rows
    /// above take the colour of the rows below, near enough.
    #[test]
    fn a_sparse_edge_row_sets_neither_a_line_nor_the_zenith() {
        let (w, h) = (512usize, 256usize);
        let elevation = |y: usize| 90.0 - (y as f32 + 0.5) / h as f32 * 180.0;
        let band = |y: usize| (0.0..20.0).contains(&elevation(y));
        let top = (0..h).find(|&y| band(y)).expect("a seen row");
        // The band's rows fully seen in 0.6, but its top row seen in 12 texels only, in 0.45.
        let known: Vec<bool> = (0..w * h).map(|t| band(t / w) && (t / w != top || t % w < 12)).collect();
        let shade = |t: usize| match (known[t], t / w == top) {
            (false, _) => 0.0,
            (true, true) => 0.45,
            (true, false) => 0.6,
        };
        let rgb: Vec<[f32; 3]> = (0..w * h).map(|t| [shade(t); 3]).collect();
        let mut map = SkyMap { width: w as u32, rgb };
        map.fill(&known);
        // Far from the 12 dark texels, the top row and the zenith take the band's colour (the trend is flat).
        for y in [top, top - 1, 0] {
            let c = map.rgb[y * w + w / 2][0];
            assert!((c - 0.6).abs() < 0.01, "row {y}: {c}");
        }
    }

    /// The horizon's colour is the band's around the horizon, not the zenith's or the ground's.
    #[test]
    fn the_horizon_is_read_from_the_band_around_it() {
        let (w, h) = (64usize, 32usize);
        let rgb = (0..w * h)
            .map(|t| {
                let elevation = 90.0 - (((t / w) as f32 + 0.5) / h as f32) * 180.0;
                if (-2.0..=12.0).contains(&elevation) {
                    [0.8, 0.6, 0.4]
                } else if elevation > 0.0 {
                    [0.1, 0.2, 0.9]
                } else {
                    [0.0; 3]
                }
            })
            .collect();
        let c = SkyMap { width: w as u32, rgb }.horizon();
        assert!((c[0] - 0.8).abs() < 1e-5 && (c[1] - 0.6).abs() < 1e-5 && (c[2] - 0.4).abs() < 1e-5, "{c:?}");
    }

    /// Discs on the shell leave in the cells the parallax test confirmed as sky; elsewhere, and the scene's own
    /// Gaussians everywhere, stay.
    #[test]
    fn shell_discs_give_way_only_where_the_test_found_sky() {
        let mut g = Gaussians::default();
        let up = [0.0, 400.0, 0.0];
        let ahead = [0.0, 0.0, 400.0];
        for p in [up, ahead] {
            g.push(p, [1.0; 3], [1.0, 0.0, 0.0, 0.0], logit(0.9), [0.5; 3]);
            *g.sky.last_mut().unwrap() = true;
        }
        g.push([0.0, 0.0, 5.0], [0.0; 3], [1.0, 0.0, 0.0, 0.0], logit(0.9), [0.5; 3]);
        let mut found = SkySeeding { sky_cells: vec![false; PARALLAX_CELLS * PARALLAX_CELLS / 2], ..Default::default() };
        found.sky_cells[texel(up, PARALLAX_CELLS)] = true;
        found.sky_cells[texel([0.0, 0.0, 1.0], PARALLAX_CELLS)] = true;
        assert_eq!(give_way(&mut g, &found, [0.0; 3]), 2, "both shell discs are in sky cells");
        assert_eq!(g.len(), 1);
        assert_eq!(g.means[0], [0.0, 0.0, 5.0], "the scene's own Gaussian stays, in a sky cell too");
    }
}
