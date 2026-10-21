//! Lab: depth where the depth frames have none (unlit, transparent or shadow-less surfaces that Unity's depth texture
//! leaves out), from the keyframes' colours and their exact poses: a plane sweep. For every keyframe, each sample of
//! its nearest depth frame without depth tries `hypotheses` depths (uniform in inverse depth); at each, a
//! fronto-parallel patch of the keyframe is warped into a few nearby keyframes by the plane's exact homography and
//! compared by normalised cross-correlation (NCC). A depth is kept when its score is high, clearly better than the
//! scores of depths far from it (texture-less patches score alike everywhere), and not at the far end (sky). Kept
//! depths become extra depth frames: seeds and depth targets like measured ones.

use crate::camera::Camera;
use crate::dataset::{channel, Dataset, DepthView};
use crate::transients::grid;
use rayon::prelude::*;

#[derive(Clone, Debug)]
pub struct MvsParams {
    pub hypotheses: usize,
    pub z_min: f32,
    pub z_max: f32,
    /// Patch half-size in pixels (radius 3: 7 × 7).
    pub radius: i32,
    /// Patch pixels are taken every `step` pixels (a wider window for the same cost).
    pub step: i32,
    pub sources: usize,
    /// Source keyframes must be this far from the reference (metres) and look within `max_angle` degrees of it.
    pub min_baseline: f32,
    pub max_baseline: f32,
    pub max_angle: f32,
    /// Keep a depth when the mean NCC of the best two sources reaches `min_ncc` and beats the best score at least two
    /// hypotheses away by `min_margin`.
    pub min_ncc: f32,
    pub min_margin: f32,
    /// Patches with less luminance spread than this (0–1) are texture-less: skipped.
    pub min_contrast: f32,
}

impl Default for MvsParams {
    fn default() -> Self {
        Self { hypotheses: 48, z_min: 0.3, z_max: 120.0, radius: 3, step: 2, sources: 4, min_baseline: 0.12, max_baseline: 3.0, max_angle: 35.0, min_ncc: 0.75, min_margin: 0.1, min_contrast: 0.02 }
    }
}

struct Grey {
    w: u32,
    h: u32,
    px: Vec<f32>,
}

impl Grey {
    fn new(image: &[u32], w: u32, h: u32) -> Self {
        let px = image.iter().map(|&p| 0.299 * channel(p, 0) + 0.587 * channel(p, 1) + 0.114 * channel(p, 2)).collect();
        Self { w, h, px }
    }
    fn at(&self, x: f32, y: f32) -> Option<f32> {
        if !(x >= 0.0 && y >= 0.0 && x <= (self.w - 1) as f32 && y <= (self.h - 1) as f32) {
            return None;
        }
        let (x0, y0) = (x.floor() as u32, y.floor() as u32);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let g = |xx: u32, yy: u32| self.px[(yy * self.w + xx) as usize];
        Some((g(x0, y0) * (1.0 - fx) + g(x1, y0) * fx) * (1.0 - fy) + (g(x0, y1) * (1.0 - fx) + g(x1, y1) * fx) * fy)
    }
}

/// Homography from reference pixels to source pixels for the plane at eye depth `z` facing the reference camera:
/// x_s ~ K_s (R + t·nᵀ/z) K_r⁻¹ x_r with (R, t) the reference-to-source motion and n = (0, 0, 1).
fn homography(reference: &Camera, source: &Camera, z: f32) -> [[f32; 3]; 3] {
    // R = R_s R_rᵀ, t = t_s − R t_r (cameras map world to camera: p_c = r p_w + t).
    let mut r = [[0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| source.r[i][k] * reference.r[j][k]).sum();
        }
    }
    let rt: [f32; 3] = [0, 1, 2].map(|i| (0..3).map(|k| r[i][k] * reference.t[k]).sum::<f32>());
    let t = [source.t[0] - rt[0], source.t[1] - rt[1], source.t[2] - rt[2]];
    // M = R + t nᵀ / z: only the third column changes.
    let mut m = r;
    for (i, row) in m.iter_mut().enumerate() {
        row[2] += t[i] / z;
    }
    // K_r⁻¹ x = ((x − cx)/fx, (y − cy)/fy, 1); then K_s.
    let kinv = [[1.0 / reference.fx, 0.0, -reference.cx / reference.fx], [0.0, 1.0 / reference.fy, -reference.cy / reference.fy], [0.0, 0.0, 1.0]];
    let mut mk = [[0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            mk[i][j] = (0..3).map(|k| m[i][k] * kinv[k][j]).sum();
        }
    }
    let ks = [[source.fx, 0.0, source.cx], [0.0, source.fy, source.cy], [0.0, 0.0, 1.0]];
    let mut h = [[0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            h[i][j] = (0..3).map(|k| ks[i][k] * mk[k][j]).sum();
        }
    }
    h
}

fn ncc(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut sab, mut saa, mut sbb) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        let (dx, dy) = (x - ma, y - mb);
        sab += dx * dy;
        saa += dx * dx;
        sbb += dy * dy;
    }
    if saa <= 1e-12 || sbb <= 1e-12 {
        return -1.0;
    }
    sab / (saa * sbb).sqrt()
}

/// Statistics of one sweep.
#[derive(Clone, Debug, Default)]
pub struct MvsStats {
    pub tried: usize,
    pub textureless: usize,
    pub kept: usize,
    pub seconds: f32,
}

/// Extra depth frames (one per keyframe, on its nearest depth frame's sample grid; samples not kept are marked
/// dynamic, so they are neither seeds nor targets, and never "no hit").
pub fn pseudo_depth(data: &Dataset, p: &MvsParams) -> (Vec<DepthView>, MvsStats) {
    let start = std::time::Instant::now();
    let (w, h) = (data.width, data.height);
    let greys: Vec<Grey> = data.views.par_iter().map(|v| Grey::new(&v.image, w, h)).collect();
    let inv = |k: usize| {
        let t = k as f32 / (p.hypotheses - 1).max(1) as f32;
        1.0 / (1.0 / p.z_max + t * (1.0 / p.z_min - 1.0 / p.z_max))
    };
    let depths: Vec<f32> = (0..p.hypotheses).map(inv).collect();
    let results: Vec<(Option<DepthView>, MvsStats)> = (0..data.views.len())
        .into_par_iter()
        .map(|k| {
            let mut stats = MvsStats::default();
            let view = &data.views[k];
            let Some(d) = data.depth_views.iter().min_by(|a, b| (a.game_time - view.game_time).abs().total_cmp(&(b.game_time - view.game_time).abs())) else { return (None, stats) };
            if grid(d).is_none() {
                return (None, stats);
            }
            // Sources: nearby keyframes looking the same way, the widest useful baselines first.
            let (eye, fwd) = (view.camera.position(), view.camera.forward());
            let mut cands: Vec<(usize, f32)> = data
                .views
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != k)
                .filter_map(|(j, o)| {
                    let e = o.camera.position();
                    let b = ((e[0] - eye[0]).powi(2) + (e[1] - eye[1]).powi(2) + (e[2] - eye[2]).powi(2)).sqrt();
                    let f = o.camera.forward();
                    let cos = f[0] * fwd[0] + f[1] * fwd[1] + f[2] * fwd[2];
                    (b >= p.min_baseline && b <= p.max_baseline && cos >= p.max_angle.to_radians().cos()).then_some((j, b * cos))
                })
                .collect();
            cands.sort_by(|a, b| b.1.total_cmp(&a.1));
            cands.truncate(p.sources);
            if cands.len() < 2 {
                return (None, stats);
            }
            let homos: Vec<Vec<[[f32; 3]; 3]>> = cands.iter().map(|&(j, _)| depths.iter().map(|&z| homography(&view.camera, &data.views[j].camera, z)).collect()).collect();
            let mut out = d.depth.clone();
            let mut dynamic = vec![true; d.depth.len()];
            let mut patch = Vec::with_capacity(64);
            let mut other = Vec::with_capacity(64);
            for (i, &z) in d.depth.iter().enumerate() {
                out[i] = 0.0;
                let (u, v) = (d.pixels[i][0], d.pixels[i][1]);
                let ix = (v.round() as u32).min(h - 1) * w + (u.round() as u32).min(w - 1);
                if z > 0.0 || data.mask[ix as usize] < 0.5 {
                    continue;
                }
                stats.tried += 1;
                patch.clear();
                let mut pts = Vec::with_capacity(64);
                for dy in -p.radius..=p.radius {
                    for dx in -p.radius..=p.radius {
                        let (x, y) = (u + (dx * p.step) as f32, v + (dy * p.step) as f32);
                        if let Some(g) = greys[k].at(x, y) {
                            patch.push(g);
                            pts.push((x, y));
                        }
                    }
                }
                if patch.len() < 9 {
                    continue;
                }
                let (lo, hi) = patch.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &g| (lo.min(g), hi.max(g)));
                if hi - lo < p.min_contrast {
                    stats.textureless += 1;
                    continue;
                }
                let mut scores = vec![-1f32; p.hypotheses];
                for (hk, score) in scores.iter_mut().enumerate() {
                    let mut per_source: Vec<f32> = Vec::with_capacity(cands.len());
                    for (s, &(j, _)) in cands.iter().enumerate() {
                        let hm = &homos[s][hk];
                        other.clear();
                        let mut ok = true;
                        for &(x, y) in &pts {
                            let wz = hm[2][0] * x + hm[2][1] * y + hm[2][2];
                            if wz <= 1e-6 {
                                ok = false;
                                break;
                            }
                            let (xs, ys) = ((hm[0][0] * x + hm[0][1] * y + hm[0][2]) / wz, (hm[1][0] * x + hm[1][1] * y + hm[1][2]) / wz);
                            match greys[j].at(xs, ys) {
                                Some(g) => other.push(g),
                                None => {
                                    ok = false;
                                    break;
                                }
                            }
                        }
                        if ok {
                            per_source.push(ncc(&patch, &other));
                        }
                    }
                    if per_source.len() >= 2 {
                        per_source.sort_by(|a, b| b.total_cmp(a));
                        *score = 0.5 * (per_source[0] + per_source[1]);
                    }
                }
                let (best, &top) = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
                if top < p.min_ncc || best == 0 {
                    continue;
                }
                let rival = scores.iter().enumerate().filter(|(hk, _)| hk.abs_diff(best) > 2).map(|(_, s)| *s).fold(-1.0, f32::max);
                if top - rival < p.min_margin {
                    continue;
                }
                out[i] = depths[best];
                dynamic[i] = false;
                stats.kept += 1;
            }
            let mut extra = DepthView { camera: view.camera, grid_width: d.grid_width, grid_height: d.grid_height, pixels: d.pixels.clone(), depth: out, game_time: view.game_time, spacing: d.spacing, dynamic };
            // The keyframe's camera (the depth frame's pixels are the same rays to within its few milliseconds).
            extra.camera = view.camera;
            (Some(extra), stats)
        })
        .collect();
    let mut total = MvsStats::default();
    let mut views = Vec::new();
    for (v, s) in results {
        total.tried += s.tried;
        total.textureless += s.textureless;
        total.kept += s.kept;
        if let Some(v) = v {
            if v.dynamic.iter().any(|x| !x) {
                views.push(v);
            }
        }
    }
    total.seconds = start.elapsed().as_secs_f32();
    (views, total)
}
