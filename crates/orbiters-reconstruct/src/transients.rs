//! Lab: transient content (passing avatars, panels, anything that was somewhere for a while) found from exact depth
//! alone. A depth sample is transient when other depth frames saw *through* its point: projected into another depth
//! frame, every neighbouring sample there lies clearly farther away (or hit nothing), so at that moment nothing was
//! there. Comparing against the other frame's own depth (not marking voxels along rays) keeps surfaces seen at grazing
//! angles consistent. Colour keyframes carry no depth; their transient pixels come from the transient samples of the
//! depth frames closest in time, projected into the keyframe.

use crate::dataset::{Dataset, DepthView};
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct Params {
    /// Test every n-th grid sample along each axis (its neighbours inherit its verdict).
    pub sample_stride: usize,
    /// Compare against every n-th depth frame.
    pub frame_stride: usize,
    /// A neighbour is clearly farther when beyond the point by max(margin_abs, margin_rel · depth).
    pub margin_abs: f32,
    pub margin_rel: f32,
    /// Transient when at least this many frames saw through the point, and they are at least this share of the frames
    /// that saw it or through it.
    pub min_votes: u32,
    pub min_share: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self { sample_stride: 2, frame_stride: 2, margin_abs: 0.05, margin_rel: 0.03, min_votes: 2, min_share: 0.3 }
    }
}

/// The regular sample grid of a depth frame: origin and spacing in pixels (see train::depth_target).
pub(crate) struct Grid {
    pub gw: usize,
    pub gh: usize,
    pub x0: f32,
    pub y0: f32,
    pub sx: f32,
    pub sy: f32,
}

pub(crate) fn grid(d: &DepthView) -> Option<Grid> {
    let (gw, gh) = (d.grid_width as usize, d.grid_height as usize);
    if gw < 2 || gh < 2 {
        return None;
    }
    let (x0, y0) = (d.pixels[0][0], d.pixels[0][1]);
    let sx = (d.pixels[gw - 1][0] - x0) / (gw - 1) as f32;
    let sy = (d.pixels[(gh - 1) * gw][1] - y0) / (gh - 1) as f32;
    (sx > 0.0 && sy > 0.0).then_some(Grid { gw, gh, x0, y0, sx, sy })
}

/// Whether a depth frame's sample measured anything: samples under the capture strip or the skeleton band carry no
/// depth symbols and read as "no depth" like the sky, so they say nothing.
pub(crate) fn known(g: &DepthView, i: usize, mask: &[f32], width: u32, height: u32) -> bool {
    let (x, y) = (g.pixels[i][0].round(), g.pixels[i][1].round());
    // Samples marked unknown (crate::depthfix: unreadable or misread tiles) say nothing either.
    !g.is_dynamic(i) && x >= 0.0 && y >= 0.0 && (x as u32) < width && (y as u32) < height && mask[(y as u32 * width + x as u32) as usize] > 0.5
}

/// What depth frame `g` says about a world point at its depth `z` seen at pixel (u, v): `Some(true)` it saw through
/// (all four neighbouring samples farther, or no hit), `Some(false)` it saw the point (a neighbour at about its depth),
/// `None` occluded, outside, or under the strip.
#[allow(clippy::too_many_arguments)]
pub(crate) fn verdict(g: &DepthView, gg: &Grid, u: f32, v: f32, z: f32, p: &Params, mask: &[f32], size: (u32, u32)) -> Option<bool> {
    let fx = (u - gg.x0) / gg.sx;
    let fy = (v - gg.y0) / gg.sy;
    if !(fx >= 0.0 && fy >= 0.0) || fx > (gg.gw - 1) as f32 || fy > (gg.gh - 1) as f32 {
        return None;
    }
    let (i0, j0) = ((fx as usize).min(gg.gw - 2), (fy as usize).min(gg.gh - 2));
    let m = p.margin_abs.max(p.margin_rel * z);
    let (mut farther, mut near) = (0, false);
    for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let k = (j0 + dj) * gg.gw + i0 + di;
        if !known(g, k, mask, size.0, size.1) {
            return None;
        }
        let d = g.depth[k];
        if d <= 0.0 || d > z + m {
            farther += 1;
        } else if d >= z - m {
            near = true;
        }
    }
    if farther == 4 {
        Some(true)
    } else if near {
        Some(false)
    } else {
        None
    }
}

/// Per depth frame, per grid sample: transient.
pub fn detect(data: &Dataset, p: &Params) -> Vec<Vec<bool>> {
    let grids: Vec<Option<Grid>> = data.depth_views.iter().map(grid).collect();
    let others: Vec<usize> = (0..data.depth_views.len()).step_by(p.frame_stride.max(1)).collect();
    data.depth_views
        .par_iter()
        .enumerate()
        .map(|(f, d)| {
            let Some(gf) = &grids[f] else { return vec![false; d.depth.len()] };
            let mut out = vec![false; d.depth.len()];
            let s = p.sample_stride.max(1);
            for gy in (0..gf.gh).step_by(s) {
                for gx in (0..gf.gw).step_by(s) {
                    let i = gy * gf.gw + gx;
                    let z = d.depth[i];
                    if z <= 0.0 {
                        continue;
                    }
                    let point = d.camera.unproject(d.pixels[i][0], d.pixels[i][1], z);
                    let (mut votes, mut seen) = (0u32, 0u32);
                    for &g in &others {
                        if g == f {
                            continue;
                        }
                        let (Some(gg), dg) = (&grids[g], &data.depth_views[g]) else { continue };
                        let Some((u, v, zg)) = dg.camera.project(point) else { continue };
                        match verdict(dg, gg, u, v, zg, p, &data.mask, (data.width, data.height)) {
                            Some(true) => votes += 1,
                            Some(false) => seen += 1,
                            None => {}
                        }
                    }
                    if votes >= p.min_votes && votes as f32 >= p.min_share * (votes + seen) as f32 {
                        for yy in gy..(gy + s).min(gf.gh) {
                            for xx in gx..(gx + s).min(gf.gw) {
                                out[yy * gf.gw + xx] = d.depth[yy * gf.gw + xx] > 0.0;
                            }
                        }
                    }
                }
            }
            out
        })
        .collect()
}

/// Per keyframe, the pixels showing transient content (1 there), from the transient samples of the depth frames within
/// `max_dt` seconds, each a disc of `scale` sample spacings, or `None` when there are none.
pub fn keyframe_masks(data: &Dataset, transient: &[Vec<bool>], max_dt: f32, scale: f32) -> Vec<Option<Vec<f32>>> {
    let (w, h) = (data.width as i64, data.height as i64);
    data.views
        .par_iter()
        .map(|view| {
            let mut mask: Option<Vec<f32>> = None;
            for (f, d) in data.depth_views.iter().enumerate() {
                if (d.game_time - view.game_time).abs() > max_dt || !transient[f].iter().any(|t| *t) {
                    continue;
                }
                let m = mask.get_or_insert_with(|| vec![0.0; (w * h) as usize]);
                for (i, t) in transient[f].iter().enumerate() {
                    if !t {
                        continue;
                    }
                    let point = d.camera.unproject(d.pixels[i][0], d.pixels[i][1], d.depth[i]);
                    let Some((u, v, z)) = view.camera.project(point) else { continue };
                    // The sample's footprint in this keyframe.
                    let r = (scale * d.spacing * d.depth[i] / z * view.camera.fx / d.camera.fx).clamp(2.0, 200.0);
                    let (x0, x1) = (((u - r).floor() as i64).max(0), ((u + r).ceil() as i64).min(w - 1));
                    let (y0, y1) = (((v - r).floor() as i64).max(0), ((v + r).ceil() as i64).min(h - 1));
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            let (dx, dy) = (x as f32 - u, y as f32 - v);
                            if dx * dx + dy * dy <= r * r {
                                m[(y * w + x) as usize] = 1.0;
                            }
                        }
                    }
                }
            }
            mask
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct ColorParams {
    /// Test every n-th sample of each keyframe's nearest depth frame along each axis.
    pub point_stride: usize,
    /// Visibility grid cell in pixels (each keyframe's nearest depth frame, splatted at its minimum depth).
    pub cell: u32,
    pub margin_abs: f32,
    pub margin_rel: f32,
    /// Mean absolute colour difference (0–1) beyond which this keyframe disagrees with the others' median.
    pub outlier: f32,
    /// Spread (median absolute deviation, 0–1) under which the other views agree with each other; beyond `unstable`
    /// the surface shows different pictures at different times.
    pub agree: f32,
    pub unstable: f32,
    /// Fewest other keyframes that must see the point.
    pub min_views: usize,
}

impl Default for ColorParams {
    fn default() -> Self {
        Self { point_stride: 2, cell: 8, margin_abs: 0.05, margin_rel: 0.03, outlier: 0.15, agree: 0.05, unstable: 0.12, min_views: 3 }
    }
}

/// Per keyframe, pixels (u, v, radius) to leave out of training: `outliers` (this keyframe's colour disagrees with the
/// other views, which agree: something without depth passed by) and `unstable` (the views disagree with each other: a
/// screen or rotating picture; every keyframe but the closest one leaves it out, so training sees one snapshot).
pub struct ColorTransients {
    pub outliers: Vec<Vec<[f32; 3]>>,
    pub unstable: Vec<Vec<[f32; 3]>>,
}

fn median(v: &mut [f32]) -> f32 {
    let k = v.len() / 2;
    *v.select_nth_unstable_by(k, f32::total_cmp).1
}

pub fn color_transients(data: &Dataset, p: &ColorParams) -> ColorTransients {
    let (w, h) = (data.width, data.height);
    let (cw, ch) = (w.div_ceil(p.cell) as usize, h.div_ceil(p.cell) as usize);
    // Each keyframe's nearest depth frame in time, and its visibility grid.
    let nearest: Vec<usize> = data
        .views
        .iter()
        .map(|v| (0..data.depth_views.len()).min_by(|&a, &b| (data.depth_views[a].game_time - v.game_time).abs().total_cmp(&(data.depth_views[b].game_time - v.game_time).abs())).unwrap_or(0))
        .collect();
    let grids: Vec<Vec<f32>> = data
        .views
        .par_iter()
        .zip(&nearest)
        .map(|(v, &d)| {
            let mut z = vec![f32::MAX; cw * ch];
            let dv = &data.depth_views[d];
            for (i, &depth) in dv.depth.iter().enumerate() {
                if depth <= 0.0 || !known(dv, i, &data.mask, w, h) {
                    continue;
                }
                let point = dv.camera.unproject(dv.pixels[i][0], dv.pixels[i][1], depth);
                if let Some((u, vv, zz)) = v.camera.project(point) {
                    if u >= 0.0 && vv >= 0.0 && u < w as f32 && vv < h as f32 {
                        let c = (vv as u32 / p.cell) as usize * cw + (u as u32 / p.cell) as usize;
                        z[c] = z[c].min(zz);
                    }
                }
            }
            z
        })
        .collect();
    let scene = |u: f32, v: f32| u >= 0.0 && v >= 0.0 && u <= (w - 1) as f32 && v <= (h - 1) as f32 && data.mask[(v.round() as u32 * w + u.round() as u32) as usize] > 0.5;
    let visible = |j: usize, u: f32, v: f32, z: f32| {
        let g = grids[j][(v as u32 / p.cell) as usize * cw + (u as u32 / p.cell) as usize];
        g < f32::MAX && (z - g).abs() <= p.margin_abs.max(p.margin_rel * z)
    };
    // Per keyframe: outlier pixels, and unstable pixels with the keyframe that keeps them.
    let found: Vec<(Vec<[f32; 3]>, Vec<(usize, [f32; 3])>)> = (0..data.views.len())
        .into_par_iter()
        .map(|k| {
            let (mut outliers, mut unstable) = (Vec::new(), Vec::new());
            let dv = &data.depth_views[nearest[k]];
            let Some(gd) = grid(dv) else { return (outliers, unstable) };
            let s = p.point_stride.max(1);
            let mut others: Vec<(usize, f32, [f32; 3])> = Vec::new();
            for gy in (0..gd.gh).step_by(s) {
                for gx in (0..gd.gw).step_by(s) {
                    let i = gy * gd.gw + gx;
                    if dv.depth[i] <= 0.0 || !known(dv, i, &data.mask, w, h) {
                        continue;
                    }
                    let point = dv.camera.unproject(dv.pixels[i][0], dv.pixels[i][1], dv.depth[i]);
                    let Some((uk, vk, zk)) = data.views[k].camera.project(point) else { continue };
                    if !scene(uk, vk) || !visible(k, uk, vk, zk) {
                        continue;
                    }
                    let Some(ck) = crate::dataset::sample_rgb(&data.views[k].image, w, h, uk, vk) else { continue };
                    others.clear();
                    for (j, view) in data.views.iter().enumerate() {
                        if j == k {
                            continue;
                        }
                        let Some((u, v, z)) = view.camera.project(point) else { continue };
                        if scene(u, v) && visible(j, u, v, z) {
                            if let Some(c) = crate::dataset::sample_rgb(&view.image, w, h, u, v) {
                                others.push((j, z, c));
                            }
                        }
                    }
                    if others.len() < p.min_views {
                        continue;
                    }
                    let mut med = [0f32; 3];
                    let mut spread = 0f32;
                    for c in 0..3 {
                        let mut values: Vec<f32> = others.iter().map(|o| o.2[c]).collect();
                        med[c] = median(&mut values);
                        let mut dev: Vec<f32> = values.iter().map(|x| (x - med[c]).abs()).collect();
                        spread += median(&mut dev) / 3.0;
                    }
                    let off = (0..3).map(|c| (ck[c] - med[c]).abs()).sum::<f32>() / 3.0;
                    // The footprint of a tested sample (every point_stride-th) in this keyframe.
                    let r = (dv.spacing * s as f32 * dv.depth[i] / zk * data.views[k].camera.fx / dv.camera.fx).clamp(2.0, 60.0);
                    if spread <= p.agree && off > p.outlier {
                        outliers.push([uk, vk, r]);
                    } else if spread > p.unstable {
                        // Kept by the closest view (most detail), this one included.
                        let closest = others.iter().map(|o| (o.0, o.1)).chain(std::iter::once((k, zk))).min_by(|a, b| a.1.total_cmp(&b.1)).map(|c| c.0).unwrap_or(k);
                        if closest != k {
                            unstable.push((closest, [uk, vk, r]));
                        }
                    }
                }
            }
            (outliers, unstable)
        })
        .collect();
    let mut out = ColorTransients { outliers: Vec::new(), unstable: Vec::new() };
    for (o, u) in found {
        out.outliers.push(o);
        out.unstable.push(u.into_iter().map(|(_, px)| px).collect());
    }
    out
}

/// A mask (1 = leave out) of discs (u, v, radius).
pub fn disc_mask(discs: &[[f32; 3]], width: u32, height: u32) -> Vec<f32> {
    let (w, h) = (width as i64, height as i64);
    let mut m = vec![0f32; (w * h) as usize];
    for &[u, v, r] in discs {
        let (x0, x1) = (((u - r).floor() as i64).max(0), ((u + r).ceil() as i64).min(w - 1));
        let (y0, y1) = (((v - r).floor() as i64).max(0), ((v + r).ceil() as i64).min(h - 1));
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f32 - u, y as f32 - v);
                if dx * dx + dy * dy <= r * r {
                    m[(y * w + x) as usize] = 1.0;
                }
            }
        }
    }
    m
}
