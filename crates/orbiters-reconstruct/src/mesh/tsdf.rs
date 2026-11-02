//! A sparse truncated signed distance field from the capture's depth frames.
//!
//! Blocks of 8³ voxels are allocated only where a trusted depth surface (and its truncation band) lies in the
//! reachable region; a hash map finds them. Every voxel of a block is then projected into each depth frame that
//! allocated it: the frame's depth there, interpolated in its sample grid, minus the voxel's, is the distance along
//! the view ray; times the sine of the angle between the ray and the sample quad's surface it is the distance to that
//! surface (point to plane). Clamped to the truncation band in front (free space carves a passing avatar away) and
//! ignored past it behind (occluded). Values average with weights.
//!
//! Point to plane, not along the ray: a walking capture sees almost all of its floor at 3–20° off grazing, where a
//! voxel one step above the floor is already half a metre in front of it along the ray. Projective distances then jump
//! from −1 to +1 between neighbouring voxels and the surface between them is lost (first plaza run: 67 % of the
//! reachable ground truth covered, the floor missing beyond 10 m of the path).
//!
//! Interpolation: a depth frame's samples sit on a grid of quads (orbiters_contracts::depth: one sample every few
//! pixels in the middle, every 2, 4 or 8 grid cells in the larger tiles towards the edges). Inverse depth is affine on
//! a plane in pixel coordinates, so bilinear interpolation of 1/z inside a quad is exact on planes. A quad counts only
//! when its four samples are trusted (crate::depthfix), nearer than [`super::Settings::max_depth`], and lie on one
//! surface seen at least [`super::Settings::min_angle`] off grazing, both along each side and across the quad: a quad
//! across a depth edge (an object in front of a wall) is nearly parallel to the view ray and fails, so no skin joins
//! the two. A quad's weight is the sine of that angle (oblique views place a surface less well) times
//! `1 / (1 + (z / 10 m)^4)`: the 16-bit inverse-depth code steps 1.5 cm at 10 m, 6 cm at 20 m, as z².

use super::{roi::Roi, Settings};
use crate::{
    camera::{cross, dot, normalize, Camera},
    dataset::{Dataset, DepthGrid},
};
use rayon::prelude::*;
use std::collections::HashMap;

pub const SIDE: i32 = 8;
pub const N: usize = 512;
/// Distance at which a sample's weight halves (see the module's comment).
const NEAR: f32 = 10.0;

/// The step of the 16-bit inverse-depth code at depth `z` (orbiters_contracts::depth::quantize), metres.
pub fn code_step(z: f32) -> f32 {
    use orbiters_contracts::depth::{Z_MAX, Z_MIN};
    z * z * (1.0 / Z_MIN - 1.0 / Z_MAX) / 65534.0
}

pub fn local(l: [i32; 3]) -> usize {
    (l[0] + SIDE * (l[1] + SIDE * l[2])) as usize
}

/// Packs a block key (21 bits a coordinate) for sorting.
pub fn pack(k: [i32; 3]) -> u64 {
    k.iter().fold(0u64, |a, &c| (a << 21) | ((c + (1 << 20)) as u64 & 0x1f_ffff))
}

pub fn unpack(p: u64) -> [i32; 3] {
    [((p >> 42) & 0x1f_ffff) as i32 - (1 << 20), ((p >> 21) & 0x1f_ffff) as i32 - (1 << 20), (p & 0x1f_ffff) as i32 - (1 << 20)]
}

pub struct Volume {
    pub voxel: f32,
    pub trunc: f32,
    pub keys: Vec<[i32; 3]>,
    /// Per block, [`N`] values: signed distance over the truncation band (−1..1, positive in front) and weight
    /// (0: never observed).
    pub sdf: Vec<f32>,
    pub weight: Vec<f32>,
    pub index: HashMap<[i32; 3], u32>,
}

impl Volume {
    pub fn empty(voxel: f32, trunc: f32) -> Self {
        Self { voxel, trunc, keys: Vec::new(), sdf: Vec::new(), weight: Vec::new(), index: HashMap::new() }
    }

    /// (signed distance, weight) of voxel `g` (global voxel coordinates), if its block exists.
    pub fn get(&self, g: [i32; 3]) -> Option<(f32, f32)> {
        let b = *self.index.get(&g.map(|c| c.div_euclid(SIDE)))? as usize;
        let i = b * N + local(g.map(|c| c.rem_euclid(SIDE)));
        Some((self.sdf[i], self.weight[i]))
    }

    /// The block holding key `k`, allocated (empty) if needed.
    pub fn ensure(&mut self, k: [i32; 3]) -> usize {
        if let Some(&b) = self.index.get(&k) {
            return b as usize;
        }
        let b = self.keys.len();
        self.keys.push(k);
        self.sdf.extend(std::iter::repeat_n(0.0, N));
        self.weight.extend(std::iter::repeat_n(0.0, N));
        self.index.insert(k, b as u32);
        b
    }

    /// Whether a world point lies in an observed voxel's cube (any of the 8 surrounding voxels has weight).
    pub fn observed(&self, p: [f32; 3]) -> bool {
        let g = p.map(|c| (c / self.voxel).floor() as i32);
        (0..8).any(|k| self.get([g[0] + (k & 1), g[1] + ((k >> 1) & 1), g[2] + ((k >> 2) & 1)]).is_some_and(|(_, w)| w > 0.0))
    }

    pub fn bytes(&self) -> usize {
        self.keys.len() * N * 8
    }
}

/// Where a frame's depth samples sit, and how to find the quad around a pixel.
struct Lattice {
    gw: usize,
    gh: usize,
    width: usize,
    height: usize,
    /// Pixel (training resolution) of each grid column and row.
    xs: Vec<f32>,
    ys: Vec<f32>,
    /// Per training pixel column / row: the nearest grid column / row.
    near_col: Vec<u32>,
    near_row: Vec<u32>,
    /// Per grid cell: the cell of its region's sample (u32::MAX: none), and per sample its step.
    sample_of: Vec<u32>,
    step: Vec<u8>,
}

impl Lattice {
    fn new(grid: &DepthGrid, width: u32, height: u32) -> Self {
        let (gw, gh) = (grid.width as usize, grid.height as usize);
        let xs: Vec<f32> = (0..gw).map(|x| grid.pixels[x][0]).collect();
        let ys: Vec<f32> = (0..gh).map(|y| grid.pixels[y * gw][1]).collect();
        let nearest = |v: &[f32], p: f32| -> u32 {
            let i = v.partition_point(|&x| x < p);
            if i == 0 {
                0
            } else if i >= v.len() || (p - v[i - 1]) <= (v[i] - p) {
                (i - 1).min(v.len() - 1) as u32
            } else {
                i as u32
            }
        };
        let near_col = (0..width).map(|u| nearest(&xs, u as f32)).collect();
        let near_row = (0..height).map(|v| nearest(&ys, v as f32)).collect();
        let step: Vec<u8> = (0..gw * gh).map(|i| if grid.unused.contains(i) { 0 } else { grid.step(i) as u8 }).collect();
        let mut sample_of = vec![u32::MAX; gw * gh];
        for i in 0..gw * gh {
            let s = step[i] as usize;
            if s == 0 {
                continue;
            }
            let (gx, gy) = (i % gw, i / gw);
            for y in gy.saturating_sub(s / 2)..(gy + s - s / 2).min(gh) {
                for x in gx.saturating_sub(s / 2)..(gx + s - s / 2).min(gw) {
                    sample_of[y * gw + x] = i as u32;
                }
            }
        }
        Self { gw, gh, width: width as usize, height: height as usize, xs, ys, near_col, near_row, sample_of, step }
    }

    /// The quad around pixel (`u`, `v`): its first corner's cell, its step and the position inside it (0..1 each way).
    #[inline]
    fn quad(&self, u: f32, v: f32) -> Option<(usize, usize, f32, f32)> {
        if !(u >= 0.0 && v >= 0.0) || u >= self.width as f32 || v >= self.height as f32 {
            return None;
        }
        let (gx, gy) = (self.near_col[u as usize] as usize, self.near_row[v as usize] as usize);
        let r = self.sample_of[gy * self.gw + gx];
        if r == u32::MAX {
            return None;
        }
        let r = r as usize;
        let s = self.step[r] as usize;
        let (rx, ry) = (r % self.gw, r / self.gw);
        let x0 = if u >= self.xs[rx] { rx } else { rx.checked_sub(s)? };
        let y0 = if v >= self.ys[ry] { ry } else { ry.checked_sub(s)? };
        if x0 + s >= self.gw || y0 + s >= self.gh {
            return None;
        }
        let tx = (u - self.xs[x0]) / (self.xs[x0 + s] - self.xs[x0]);
        let ty = (v - self.ys[y0]) / (self.ys[y0 + s] - self.ys[y0]);
        if !(-1e-3..=1.001).contains(&tx) || !(-1e-3..=1.001).contains(&ty) {
            return None;
        }
        Some((y0 * self.gw + x0, s, tx.clamp(0.0, 1.0), ty.clamp(0.0, 1.0)))
    }
}

/// A frame's trusted inverse depths (0: none), its quads' weights (0: not a surface) and the sine of the angle
/// between their surface and the view ray, by first corner.
struct Frame {
    q: Vec<f32>,
    w: Vec<f32>,
    sine: Vec<f32>,
    /// Per quad: its truncation band (metres).
    band: Vec<f32>,
}

fn frame(data: &Dataset, vi: usize, lat: &Lattice, s: &Settings) -> Frame {
    let dv = &data.depth_views[vi];
    let cam = &dv.camera;
    let q: Vec<f32> = (0..dv.depth.len())
        .map(|i| {
            let z = dv.depth[i];
            if !z.is_finite() || z <= 0.1 || z > s.max_depth || lat.step[i] == 0 || dv.is_unknown(i) {
                return 0.0;
            }
            let [u, v] = dv.grid.pixels[i];
            let (x, y) = (u.round() as i64, v.round() as i64);
            if x < 0 || y < 0 || x >= data.width as i64 || y >= data.height as i64 || data.mask[(y as u32 * data.width + x as u32) as usize] <= 0.0 {
                return 0.0;
            }
            1.0 / z
        })
        .collect();
    let tan_min = s.min_angle.to_radians().tan();
    let sin_min = s.min_angle.to_radians().sin();
    let centre = cam.position();
    let (w, sine): (Vec<f32>, Vec<f32>) = (0..q.len())
        .map(|i| {
            let st = lat.step[i] as usize;
            let (gx, gy) = (i % lat.gw, i / lat.gw);
            if st == 0 || q[i] <= 0.0 || gx + st >= lat.gw || gy + st >= lat.gh {
                return (0.0, 0.0);
            }
            let c = [i, i + st, i + st * lat.gw, i + st * lat.gw + st];
            if c.iter().any(|&k| lat.step[k] as usize != st || q[k] <= 0.0) {
                return (0.0, 0.0);
            }
            let z = c.map(|k| 1.0 / q[k]);
            let (x0, x1, y0, y1) = (lat.xs[gx], lat.xs[gx + st], lat.ys[gy], lat.ys[gy + st]);
            // Each side: the depth step over the angle between its rays, at most that of a surface `min_angle` off grazing.
            let ax = (x1 - x0) / cam.fx;
            let ay = (y1 - y0) / cam.fy;
            for (a, b, angle) in [(0, 1, ax), (2, 3, ax), (0, 2, ay), (1, 3, ay)] {
                if (z[a] - z[b]).abs() * tan_min > angle * z[a].min(z[b]) {
                    return (0.0, 0.0);
                }
            }
            let p = [cam.unproject(x0, y0, z[0]), cam.unproject(x1, y0, z[1]), cam.unproject(x0, y1, z[2]), cam.unproject(x1, y1, z[3])];
            let n = normalize(cross(sub(p[3], p[0]), sub(p[2], p[1])));
            let mid: [f32; 3] = std::array::from_fn(|k| 0.25 * (p[0][k] + p[1][k] + p[2][k] + p[3][k]));
            let sine = dot(n, normalize(sub(mid, centre))).abs();
            if sine < sin_min {
                return (0.0, 0.0);
            }
            let zm = 0.25 * (z[0] + z[1] + z[2] + z[3]);
            (sine / (1.0 + (zm / NEAR).powi(4)), sine)
        })
        .unzip();
    let band = (0..q.len())
        .map(|i| if w[i] > 0.0 { (s.trunc_voxels * s.voxel).max(s.quant_trunc * code_step(1.0 / q[i])) } else { 0.0 })
        .collect();
    Frame { q, w, sine, band }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

/// The block keys (packed, sorted, unique) a frame's trusted surface and its truncation band touch in the region:
/// each quad sampled at most half a block apart, each sample along its normal across the band one voxel at a time, every
/// block holding a corner of the cube around each point (so marching cubes finds its neighbours).
fn allocate(data: &Dataset, vi: usize, lat: &Lattice, f: &Frame, roi: &Roi, voxel: f32) -> Vec<u64> {
    let cam = &data.depth_views[vi].camera;
    let mut keys = Vec::new();
    let mut last = u64::MAX;
    let block = voxel * SIDE as f32;
    for i in 0..f.w.len() {
        if f.w[i] <= 0.0 {
            continue;
        }
        let reach = (f.band[i] / voxel).ceil() as i32 + 1;
        let st = lat.step[i] as usize;
        let (gx, gy) = (i % lat.gw, i / lat.gw);
        let c = [i, i + st, i + st * lat.gw, i + st * lat.gw + st];
        let (x0, x1, y0, y1) = (lat.xs[gx], lat.xs[gx + st], lat.ys[gy], lat.ys[gy + st]);
        let p = [cam.unproject(x0, y0, 1.0 / f.q[c[0]]), cam.unproject(x1, y0, 1.0 / f.q[c[1]]), cam.unproject(x0, y1, 1.0 / f.q[c[2]]), cam.unproject(x1, y1, 1.0 / f.q[c[3]])];
        let side = [sub(p[1], p[0]), sub(p[3], p[2]), sub(p[2], p[0]), sub(p[3], p[1])].iter().map(|d| dot(*d, *d).sqrt()).fold(0.0, f32::max);
        // The band runs along the surface's normal (the field is point to plane).
        let dir = normalize(cross(sub(p[3], p[0]), sub(p[2], p[1])));
        let n = ((side / (0.5 * block)).ceil() as usize).clamp(1, 32);
        for a in 0..=n {
            for b in 0..=n {
                let (s, t) = (a as f32 / n as f32, b as f32 / n as f32);
                let q: [f32; 3] = std::array::from_fn(|k| (1.0 - s) * (1.0 - t) * p[0][k] + s * (1.0 - t) * p[1][k] + (1.0 - s) * t * p[2][k] + s * t * p[3][k]);
                if !roi.contains(q) {
                    continue;
                }
                for k in -reach..=reach {
                    let x: [f32; 3] = std::array::from_fn(|j| q[j] + dir[j] * k as f32 * voxel);
                    let g = x.map(|c| (c / voxel).floor() as i32);
                    let lo = g.map(|c| c.div_euclid(SIDE));
                    let hi = g.map(|c| (c + 1).div_euclid(SIDE));
                    for bz in lo[2]..=hi[2] {
                        for by in lo[1]..=hi[1] {
                            for bx in lo[0]..=hi[0] {
                                let key = pack([bx, by, bz]);
                                if key != last {
                                    keys.push(key);
                                    last = key;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// Fuses the session's depth frames in the region (see the module's comment).
pub fn fuse(data: &Dataset, roi: &Roi, s: &Settings) -> Volume {
    let voxel = s.voxel;
    let trunc = s.trunc_voxels * voxel;
    let mut lattices: Vec<(*const DepthGrid, Lattice)> = Vec::new();
    let lattice_of: Vec<usize> = data
        .depth_views
        .iter()
        .map(|dv| {
            let ptr = std::sync::Arc::as_ptr(&dv.grid);
            if let Some(k) = lattices.iter().position(|(p, _)| *p == ptr) {
                return k;
            }
            lattices.push((ptr, Lattice::new(&dv.grid, dv.camera.width, dv.camera.height)));
            lattices.len() - 1
        })
        .collect();
    let lattices: Vec<Lattice> = lattices.into_iter().map(|(_, l)| l).collect();
    let frames: Vec<Frame> = (0..data.depth_views.len()).into_par_iter().map(|vi| frame(data, vi, &lattices[lattice_of[vi]], s)).collect();
    let per_view: Vec<Vec<u64>> =
        (0..data.depth_views.len()).into_par_iter().map(|vi| allocate(data, vi, &lattices[lattice_of[vi]], &frames[vi], roi, voxel)).collect();
    let mut pairs: Vec<(u64, u32)> = per_view.iter().enumerate().flat_map(|(vi, keys)| keys.iter().map(move |&k| (k, vi as u32))).collect();
    drop(per_view);
    pairs.par_sort_unstable();
    let mut keys = Vec::new();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < pairs.len() {
        let mut j = i;
        while j < pairs.len() && pairs[j].0 == pairs[i].0 {
            j += 1;
        }
        keys.push(unpack(pairs[i].0));
        ranges.push(i..j);
        i = j;
    }
    let mut v = Volume::empty(voxel, trunc);
    v.sdf = vec![0.0; keys.len() * N];
    v.weight = vec![0.0; keys.len() * N];
    v.sdf.par_chunks_mut(N).zip(v.weight.par_chunks_mut(N)).zip(keys.par_iter().zip(ranges.par_iter())).for_each(|((sdf, weight), (key, range))| {
        for &(_, vi) in &pairs[range.clone()] {
            let vi = vi as usize;
            let cam: &Camera = &data.depth_views[vi].camera;
            let lat = &lattices[lattice_of[vi]];
            let f = &frames[vi];
            for z in 0..SIDE {
                for y in 0..SIDE {
                    for x in 0..SIDE {
                        let p = [(key[0] * SIDE + x) as f32 * voxel, (key[1] * SIDE + y) as f32 * voxel, (key[2] * SIDE + z) as f32 * voxel];
                        let c = cam.to_camera(p);
                        if c[2] < 0.1 {
                            continue;
                        }
                        let Some((base, st, tx, ty)) = lat.quad(cam.fx * c[0] / c[2] + cam.cx, cam.fy * c[1] / c[2] + cam.cy) else { continue };
                        let w = f.w[base];
                        if w <= 0.0 {
                            continue;
                        }
                        let q = (1.0 - tx) * (1.0 - ty) * f.q[base]
                            + tx * (1.0 - ty) * f.q[base + st]
                            + (1.0 - tx) * ty * f.q[base + st * lat.gw]
                            + tx * ty * f.q[base + st * lat.gw + st];
                        // Along the ray (eye depth difference times the ray's length per unit depth), then to the plane.
                        let d = (1.0 / q - c[2]) * (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt() / c[2] * f.sine[base];
                        let band = f.band[base];
                        if d < -band {
                            continue;
                        }
                        let t = (d / band).min(1.0);
                        let i = local([x, y, z]);
                        let total = weight[i] + w;
                        sdf[i] = (sdf[i] * weight[i] + t * w) / total;
                        weight[i] = total;
                    }
                }
            }
        }
    });
    v.index = keys.iter().enumerate().map(|(b, k)| (*k, b as u32)).collect();
    v.keys = keys;
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::{DepthView, Loaded, Marks};
    use crate::mesh::roi::WalkPath;

    /// A camera at the origin looking down +Z at a wall 2 m away, sampled every pixel.
    pub(crate) fn wall(depth: impl Fn(usize, usize) -> f32) -> Dataset {
        let (w, h) = (48u32, 32u32);
        let camera = Camera::look([0., 1., 0.], [0., 0., 1.], [0., 1., 0.], 1.0, w, h);
        let pixels = (0..h).flat_map(|y| (0..w).map(move |x| [x as f32, y as f32])).collect();
        let grid = std::sync::Arc::new(DepthGrid { width: w, height: h, pixels, spacing: 1., ..Default::default() });
        let d = (0..(w * h) as usize).map(|i| depth(i % w as usize, i / w as usize)).collect();
        let dv = DepthView { camera, grid, depth: d, game_time: 0., unknown: Marks::default(), decoder_unknown: 0, zero_may_be_unread: false };
        Dataset { dir: Default::default(), views: vec![], depth_views: vec![dv], faces: None, width: w, height: h, geometry: Default::default(), mask: vec![1.; (w * h) as usize], loaded: Loaded::default() }
    }

    fn roi() -> Roi {
        Roi::new(&WalkPath { feet: vec![[0., 0., 0.]], eye_height: 1.6, source: "test" }, 10.0, 2.0, 4.0)
    }

    #[test]
    fn a_plane_fuses_at_its_depth_and_unknown_does_not() {
        let s = Settings { voxel: 0.05, ..Default::default() };
        let data = wall(|_, _| 2.0);
        let v = fuse(&data, &roi(), &s);
        assert!(!v.keys.is_empty());
        // The zero crossing along the optical axis lies at z = 2: voxels just in front are positive, behind negative.
        let (front, wf) = v.get([0, 20, 39]).unwrap();
        let (behind, wb) = v.get([0, 20, 41]).unwrap();
        assert!(wf > 0.0 && wb > 0.0 && front > 0.0 && behind < 0.0, "{front} {behind}");
        assert!((v.get([0, 20, 40]).unwrap().0).abs() < 0.05);
        // Far behind the wall: never observed.
        assert!(v.get([0, 20, 48]).is_none_or(|(_, w)| w == 0.0));
        let mut unknown = wall(|_, _| 2.0);
        for i in 0..unknown.depth_views[0].depth.len() {
            unknown.depth_views[0].unknown.insert(i);
        }
        assert!(fuse(&unknown, &roi(), &s).keys.is_empty());
        let mut masked = wall(|_, _| 2.0);
        masked.mask.fill(0.0);
        assert!(fuse(&masked, &roi(), &s).keys.is_empty());
        let mut sky = wall(|_, _| 0.0);
        assert!(fuse(&sky, &roi(), &s).keys.is_empty());
        sky.depth_views[0].depth.fill(40.0);
        assert!(fuse(&sky, &roi(), &s).keys.is_empty(), "beyond max_depth");
    }

    #[test]
    fn a_depth_edge_makes_no_skin() {
        // Left half at 2 m, right half at 6 m: the quads across the edge are rejected, so no voxel between the two
        // surfaces (at 4 m, on the edge's ray) is called a surface.
        let s = Settings { voxel: 0.05, ..Default::default() };
        let data = wall(|x, _| if x < 24 { 2.0 } else { 6.0 });
        let v = fuse(&data, &roi(), &s);
        let cam = &data.depth_views[0].camera;
        let p = cam.unproject(23.5, 16.0, 4.0);
        let g = p.map(|c| (c / s.voxel).round() as i32);
        assert!(v.get(g).is_none_or(|(d, w)| w == 0.0 || d > 0.5), "{:?}", v.get(g));
    }

    #[test]
    fn keys_pack_and_unpack() {
        for k in [[0, 0, 0], [-5, 7, -1_000_000], [1_000_000, -1, 3]] {
            assert_eq!(unpack(pack(k)), k);
        }
        assert!(pack([0, 0, 1]) > pack([0, 0, 0]) && pack([1, 0, 0]) > pack([0, 9, 9]));
    }
}
