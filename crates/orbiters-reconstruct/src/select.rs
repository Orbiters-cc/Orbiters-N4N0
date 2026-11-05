//! Selecting splats in the 3D view, in the world (crate::edit marks what is selected, by the splats' stable IDs): a
//! selection is a set of splats, not a shape on the screen, so it stays on them as the camera moves and is what Hide
//! and Delete take out of every output.
//!
//! - **Under the cursor** ([`View::pick`], [`View::under`]): the splats a pixel's colour comes from, front to back
//!   with their blending weights `α·T`, as the viewer draws them: the same projection as `project.wgsl` with a
//!   viewer's guards (sorted by distance from the eye, faded in over the first 20 cm, splats holding the eye left out),
//!   on the processor, so a click needs no readback from a GPU that may be busy beside VRChat. Hidden splats are not
//!   drawn, so a click goes through them. A pick also gives the 3D point under the cursor: where the pixel's
//!   transmittance falls through one half (the surface it shows), which places the sphere.
//! - **Brush** ([`View::visible_in_circle`]): every splat that gives at least [`BRUSH_WEIGHT`] of a pixel's colour
//!   within the circle, sampled at most 64 pixels across: what you see under it, never what it hides.
//! - **Box** ([`in_rectangle`]): every splat whose centre the screen rectangle holds, at every depth (the rectangle's
//!   frustum, like a box select seen through).
//! - **Sphere** ([`in_sphere`]): every splat whose centre lies within a world sphere.
//! - **Grow and shrink** ([`grow`], [`shrink`]): the selection widened by the splats whose centres lie within a step of
//!   a selected one, or narrowed by the selected ones that have an unselected one that close: three times the
//!   selection's median size ([`step`]), so a step follows how dense the splats are where they were picked.
//!
//! The geometry kept for this is 44 bytes a splat (132 MB for three million), made when the selection opens; a view's
//! projection is made once for a camera (about 10 ms for a million splats on 16 cores) and reused while it stands still.

use crate::camera::{Camera, dot};
use crate::edit::{DELETED, HIDDEN, SELECTED};
use crate::gaussians::{Gaussians, rotation_matrix, sigmoid};
use rayon::prelude::*;

/// The viewer's near plane, the distance over which splats fade in from it, the 2D filter (pixels²) and the alpha
/// bounds of the rasteriser (project.wgsl, common.wgsl).
const NEAR: f32 = crate::cpu::NEAR as f32;
const GUARD_FADE: f32 = 0.2;
const FILTER_2D: f32 = crate::cpu::FILTER_2D as f32;
const FLAT: f32 = crate::cpu::FLAT as f32;
const MIN_ALPHA: f32 = crate::cpu::MIN_ALPHA as f32;
const MAX_ALPHA: f32 = crate::cpu::MAX_ALPHA as f32;
const MIN_T: f32 = crate::cpu::MIN_T as f32;
/// A brush takes the splats that give at least this share of a pixel's colour: the surface seen, its thin front
/// splats included, not the faint ones behind it.
pub const BRUSH_WEIGHT: f32 = 0.08;
/// A click takes the strongest splat under the cursor and those giving at least this share of the pixel.
pub const CLICK_WEIGHT: f32 = 0.15;
/// Samples across a brush's circle at most (the finest step is a pixel).
const BRUSH_SAMPLES: f32 = 64.0;

/// Whether a splat of these marks is drawn (neither hidden nor deleted).
pub fn drawn(mark: u8) -> bool {
    mark & (HIDDEN | DELETED) == 0
}

/// The splats' geometry a selection reads: centres, world covariances (xx, xy, xz, yy, yz, zz), opacities and sizes
/// (the largest standard deviation, metres).
pub struct Splats {
    pub means: Vec<[f32; 3]>,
    cov: Vec<[f32; 6]>,
    opacity: Vec<f32>,
    pub size: Vec<f32>,
}

impl Splats {
    pub fn new(g: &Gaussians) -> Self {
        let n = g.len();
        let cov = (0..n)
            .into_par_iter()
            .map(|i| {
                let r = rotation_matrix(g.rotations[i]);
                let s2 = g.log_scales[i].map(|l| (2.0 * l).exp());
                let c = |a: usize, b: usize| (0..3).map(|k| r[a][k] * s2[k] * r[b][k]).sum::<f32>();
                [c(0, 0), c(0, 1), c(0, 2), c(1, 1), c(1, 2), c(2, 2)]
            })
            .collect();
        let opacity = g.opacity_logits.par_iter().map(|&l| sigmoid(l)).collect();
        let size = g.log_scales.par_iter().map(|s| s[0].max(s[1]).max(s[2]).exp()).collect();
        Self { means: g.means.clone(), cov, opacity, size }
    }

    pub fn len(&self) -> usize {
        self.means.len()
    }

    pub fn is_empty(&self) -> bool {
        self.means.is_empty()
    }

    /// What `camera` sees of the splats `marks` leaves drawn ([`drawn`]), front to back.
    pub fn view(&self, camera: &Camera, marks: &[u8]) -> View {
        let mut list: Vec<Footprint> =
            (0..self.len()).into_par_iter().filter(|&i| marks.get(i).is_none_or(|&m| drawn(m))).filter_map(|i| self.footprint(i, camera)).collect();
        list.par_sort_unstable_by(|a, b| a.distance.total_cmp(&b.distance).then(a.id.cmp(&b.id)));
        View { camera: *camera, list }
    }

    /// Splat `i` as `camera`'s pixels see it (project.wgsl with a viewer's guards), or None where it draws nothing.
    fn footprint(&self, i: usize, cam: &Camera) -> Option<Footprint> {
        let pc = cam.to_camera(self.means[i]);
        let z = pc[2];
        if z < NEAR {
            return None;
        }
        let [xx, xy, xz, yy, yz, zz] = self.cov[i];
        let sw = [[xx, xy, xz], [xy, yy, yz], [xz, yz, zz]];
        // Σc = W Σw Wᵀ.
        let w = cam.r;
        let mut sc = [[0f32; 3]; 3];
        for r in 0..3 {
            let swr = [0, 1, 2].map(|c| dot(sw[c], w[r]));
            for c in 0..3 {
                sc[r][c] = dot(w[c], swr);
            }
        }
        let (fx, fy) = (cam.fx, cam.fy);
        let lim_x = 1.3 * 0.5 * cam.width as f32 / fx;
        let lim_y = 1.3 * 0.5 * cam.height as f32 / fy;
        let u = (pc[0] / z).clamp(-lim_x, lim_x);
        let v = (pc[1] / z).clamp(-lim_y, lim_y);
        let j0 = [fx / z, 0.0, -fx * u / z];
        let j1 = [0.0, fy / z, -fy * v / z];
        let js0 = [dot(j0, sc[0]), dot(j0, sc[1]), dot(j0, sc[2])];
        let js1 = [dot(j1, sc[0]), dot(j1, sc[1]), dot(j1, sc[2])];
        let raw = [dot(js0, j0), dot(js0, j1), dot(js1, j1)];
        let (a, b, c) = (raw[0] + FILTER_2D, raw[1], raw[2] + FILTER_2D);
        let det = a * c - b * b;
        if det <= 0.0 {
            return None;
        }
        let det_a = raw[0] * raw[2] - raw[1] * raw[1];
        let mut op = self.opacity[i] * (det_a.max(FLAT * FILTER_2D * FILTER_2D) / det).sqrt();
        // The guards: a splat holding the eye is left out (its Mahalanobis distance from the eye within its reach).
        let eye = cam.position();
        let d = [eye[0] - self.means[i][0], eye[1] - self.means[i][1], eye[2] - self.means[i][2]];
        if let Some(inv) = inverse(&sw) {
            let m = (0..3).map(|r| d[r] * dot(inv[r], d)).sum::<f32>();
            if m < reach(op) {
                return None;
            }
        }
        let distance = (pc[0] * pc[0] + pc[1] * pc[1] + z * z).sqrt();
        op *= smoothstep(NEAR, GUARD_FADE, distance);
        let tau = reach(op);
        if tau <= 0.0 {
            return None;
        }
        let mean = [fx * pc[0] / z + cam.cx, fy * pc[1] / z + cam.cy];
        let half = [(tau * a).sqrt(), (tau * c).sqrt()];
        if mean[0] + half[0] < -0.5 || mean[1] + half[1] < -0.5 || mean[0] - half[0] > cam.width as f32 - 0.5 || mean[1] - half[1] > cam.height as f32 - 0.5 {
            return None;
        }
        Some(Footprint { mean, half, conic: [c / det, -b / det, a / det], opacity: op, distance, id: i as u32 })
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The largest quadratic form at which a splat of opacity `op` still reaches α = 1/255 (common.wgsl `alpha_reach`).
fn reach(op: f32) -> f32 {
    let s = (2.0 * (op / MIN_ALPHA).max(1.0).ln()).sqrt().min(3.33);
    s * s
}

fn inverse(m: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let c = |r: usize, k: usize| m[(r + 1) % 3][(k + 1) % 3] * m[(r + 2) % 3][(k + 2) % 3] - m[(r + 1) % 3][(k + 2) % 3] * m[(r + 2) % 3][(k + 1) % 3];
    let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
    if det.abs() < 1e-30 || !det.is_finite() {
        return None;
    }
    // The inverse is the transposed cofactors over the determinant.
    Some([0, 1, 2].map(|r| [0, 1, 2].map(|k| c(k, r) / det)))
}

/// A splat as a view's pixels see it.
#[derive(Clone, Copy, Debug)]
struct Footprint {
    mean: [f32; 2],
    /// Its extent along x and y (where it still reaches α = 1/255).
    half: [f32; 2],
    /// Inverse 2D covariance (A, B, C): power = −½(A·dx² + C·dy²) − B·dx·dy.
    conic: [f32; 3],
    opacity: f32,
    distance: f32,
    id: u32,
}

impl Footprint {
    fn covers(&self, x: f32, y: f32) -> bool {
        (x - self.mean[0]).abs() <= self.half[0] && (y - self.mean[1]).abs() <= self.half[1]
    }

    /// Its α at pixel (`x`, `y`), or None where it gives nothing.
    fn alpha(&self, x: f32, y: f32) -> Option<f32> {
        let (dx, dy) = (x - self.mean[0], y - self.mean[1]);
        let power = -0.5 * (self.conic[0] * dx * dx + self.conic[2] * dy * dy) - self.conic[1] * dx * dy;
        if power > 0.0 {
            return None;
        }
        let alpha = (self.opacity * power.exp()).min(MAX_ALPHA);
        (alpha >= MIN_ALPHA).then_some(alpha)
    }
}

/// What a pick found under the cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// The splat giving the most of the pixel.
    pub id: u32,
    /// The surface seen there (where the pixel's transmittance falls through one half; else the strongest splat's
    /// distance) in the world, and its distance from the eye.
    pub point: [f32; 3],
    pub distance: f32,
}

/// What a camera sees of the drawn splats, front to back.
pub struct View {
    pub camera: Camera,
    list: Vec<Footprint>,
}

impl View {
    /// Splats on screen.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The splats the colour of pixel (`x`, `y`) comes from, front to back: (ID, weight α·T, distance).
    pub fn under(&self, x: f32, y: f32) -> Vec<(u32, f32, f32)> {
        blend(self.list.iter().filter(|f| f.covers(x, y)), x, y)
    }

    /// The splat under pixel (`x`, `y`) and the surface it shows, or None where the pixel shows nothing.
    pub fn pick(&self, x: f32, y: f32) -> Option<Hit> {
        let under = self.under(x, y);
        let &(id, _, strongest) = under.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
        // Where the transmittance falls through one half, between the splats on either side of it.
        let mut t = 1.0f32;
        let mut distance = strongest;
        for &(_, w, d) in &under {
            if t - w <= 0.5 {
                distance = d;
                break;
            }
            t -= w;
        }
        let ray = self.ray(x, y);
        let eye = self.camera.position();
        Some(Hit { id, point: [eye[0] + ray[0] * distance, eye[1] + ray[1] * distance, eye[2] + ray[2] * distance], distance })
    }

    /// What a click at pixel (`x`, `y`) selects: the strongest splat there and every one giving [`CLICK_WEIGHT`].
    pub fn clicked(&self, x: f32, y: f32) -> Vec<u32> {
        let under = self.under(x, y);
        let Some(&(best, _, _)) = under.iter().max_by(|a, b| a.1.total_cmp(&b.1)) else { return Vec::new() };
        let mut ids: Vec<u32> = under.iter().filter(|u| u.1 >= CLICK_WEIGHT).map(|u| u.0).collect();
        if !ids.contains(&best) {
            ids.push(best);
        }
        ids
    }

    /// The unit direction in the world of the ray through pixel (`x`, `y`).
    pub fn ray(&self, x: f32, y: f32) -> [f32; 3] {
        let c = &self.camera;
        let p = c.unproject(x, y, 1.0);
        let e = c.position();
        crate::camera::normalize([p[0] - e[0], p[1] - e[1], p[2] - e[2]])
    }

    /// The splats giving at least `weight` of some pixel within `radius` pixels of `centre` (sampled at most
    /// [`BRUSH_SAMPLES`] across): what the brush paints.
    pub fn visible_in_circle(&self, centre: [f32; 2], radius: f32, weight: f32) -> Vec<u32> {
        let radius = radius.max(0.5);
        let (x0, y0, x1, y1) = (centre[0] - radius, centre[1] - radius, centre[0] + radius, centre[1] + radius);
        // The splats that reach the circle's square, front to back, binned by cells of it.
        let near: Vec<&Footprint> = self
            .list
            .par_iter()
            .filter(|f| f.mean[0] + f.half[0] >= x0 && f.mean[0] - f.half[0] <= x1 && f.mean[1] + f.half[1] >= y0 && f.mean[1] - f.half[1] <= y1)
            .collect();
        let step = (2.0 * radius / BRUSH_SAMPLES).max(1.0);
        let cells = ((2.0 * radius / (8.0 * step)).ceil() as usize).clamp(1, 16);
        let cell = 2.0 * radius / cells as f32;
        let mut bins: Vec<Vec<&Footprint>> = vec![Vec::new(); cells * cells];
        for f in &near {
            let cx0 = (((f.mean[0] - f.half[0] - x0) / cell).floor().max(0.0) as usize).min(cells - 1);
            let cx1 = (((f.mean[0] + f.half[0] - x0) / cell).floor().max(0.0) as usize).min(cells - 1);
            let cy0 = (((f.mean[1] - f.half[1] - y0) / cell).floor().max(0.0) as usize).min(cells - 1);
            let cy1 = (((f.mean[1] + f.half[1] - y0) / cell).floor().max(0.0) as usize).min(cells - 1);
            for cy in cy0..=cy1 {
                for cx in cx0..=cx1 {
                    bins[cy * cells + cx].push(f);
                }
            }
        }
        let n = (2.0 * radius / step).ceil() as usize + 1;
        let samples: Vec<[f32; 2]> = (0..n * n)
            .map(|k| [x0 + (k % n) as f32 * step, y0 + (k / n) as f32 * step])
            .filter(|p| (p[0] - centre[0]).powi(2) + (p[1] - centre[1]).powi(2) <= radius * radius)
            .collect();
        let mut ids: Vec<u32> = samples
            .par_iter()
            .flat_map_iter(|&[x, y]| {
                let cx = (((x - x0) / cell) as usize).min(cells - 1);
                let cy = (((y - y0) / cell) as usize).min(cells - 1);
                blend(bins[cy * cells + cx].iter().copied().filter(|f| f.covers(x, y)), x, y).into_iter().filter(|u| u.1 >= weight).map(|u| u.0)
            })
            .collect();
        ids.par_sort_unstable();
        ids.dedup();
        ids
    }
}

/// Front to back over `list` (in distance order) at pixel (`x`, `y`): each splat's weight α·T and distance.
fn blend<'a>(list: impl Iterator<Item = &'a Footprint>, x: f32, y: f32) -> Vec<(u32, f32, f32)> {
    let mut t = 1.0f32;
    let mut out = Vec::new();
    for f in list {
        let Some(alpha) = f.alpha(x, y) else { continue };
        let next = t * (1.0 - alpha);
        if next < MIN_T {
            break;
        }
        out.push((f.id, alpha * t, f.distance));
        t = next;
    }
    out
}

/// The drawn splats whose centres project into the screen rectangle `min`..`max` of `camera` (pixels), at every
/// depth in front of it.
pub fn in_rectangle(splats: &Splats, marks: &[u8], camera: &Camera, min: [f32; 2], max: [f32; 2]) -> Vec<u32> {
    (0..splats.len() as u32)
        .into_par_iter()
        .filter(|&i| {
            marks.get(i as usize).is_none_or(|&m| drawn(m))
                && camera.project(splats.means[i as usize]).is_some_and(|(x, y, z)| z >= NEAR && x >= min[0] && x <= max[0] && y >= min[1] && y <= max[1])
        })
        .collect()
}

/// The drawn splats whose centres lie within `radius` metres of `centre`.
pub fn in_sphere(splats: &Splats, marks: &[u8], centre: [f32; 3], radius: f32) -> Vec<u32> {
    let r2 = radius * radius;
    (0..splats.len() as u32)
        .into_par_iter()
        .filter(|&i| marks.get(i as usize).is_none_or(|&m| drawn(m)) && distance2(splats.means[i as usize], centre) <= r2)
        .collect()
}

/// Splat `i`'s mark (0 past the end of `marks`).
fn mark(marks: &[u8], i: usize) -> u8 {
    marks.get(i).copied().unwrap_or(0)
}

fn distance2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// A grow or shrink step for the selection of `marks`: three times its splats' median size, from half a centimetre to
/// half a metre (a capture's surface splats are a centimetre or two across; its floaters and far splats much more).
pub fn step(splats: &Splats, marks: &[u8]) -> f32 {
    let mut sizes: Vec<f32> = (0..splats.len()).filter(|&i| mark(marks, i) & SELECTED != 0).map(|i| splats.size[i]).filter(|s| s.is_finite()).collect();
    if sizes.is_empty() {
        return 0.05;
    }
    let mid = sizes.len() / 2;
    let (_, median, _) = sizes.select_nth_unstable_by(mid, f32::total_cmp);
    (3.0 * *median).clamp(0.005, 0.5)
}

/// The splats near the selection: the drawn unselected splats within `radius` of a selected one (grow), or the
/// selected splats within `radius` of a drawn unselected one (shrink).
fn border(splats: &Splats, marks: &[u8], radius: f32, inward: bool) -> Vec<u32> {
    let selected: Vec<u32> = (0..splats.len() as u32).into_par_iter().filter(|&i| mark(marks, i as usize) & SELECTED != 0).collect();
    if selected.is_empty() || !(radius > 0.0) {
        return Vec::new();
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for &i in &selected {
        for k in 0..3 {
            lo[k] = lo[k].min(splats.means[i as usize][k] - radius);
            hi[k] = hi[k].max(splats.means[i as usize][k] + radius);
        }
    }
    let others: Vec<u32> = (0..splats.len() as u32)
        .into_par_iter()
        .filter(|&i| {
            let m = mark(marks, i as usize);
            let p = splats.means[i as usize];
            drawn(m) && m & SELECTED == 0 && (0..3).all(|k| p[k] >= lo[k] && p[k] <= hi[k])
        })
        .collect();
    if others.is_empty() {
        return Vec::new();
    }
    let (grid, probes) = if inward { (Grid::new(splats, &others, radius), &selected) } else { (Grid::new(splats, &selected, radius), &others) };
    probes.par_iter().copied().filter(|&i| grid.any_within(splats, splats.means[i as usize], radius)).collect()
}

/// The drawn unselected splats within `radius` of a selected one: what Grow adds.
pub fn grow(splats: &Splats, marks: &[u8], radius: f32) -> Vec<u32> {
    border(splats, marks, radius, false)
}

/// The selected splats within `radius` of a drawn unselected one: what Shrink takes away.
pub fn shrink(splats: &Splats, marks: &[u8], radius: f32) -> Vec<u32> {
    border(splats, marks, radius, true)
}

/// Splats sorted into cubes of a side (cells of 21 bits a coordinate, packed in a u64), found again by binary search.
struct Grid {
    side: f32,
    keys: Vec<(u64, u32)>,
}

impl Grid {
    const OFFSET: i64 = 1 << 20;

    fn new(splats: &Splats, ids: &[u32], side: f32) -> Self {
        let mut keys: Vec<(u64, u32)> = ids.par_iter().map(|&i| (Self::key(Self::cell(splats.means[i as usize], side)), i)).collect();
        keys.par_sort_unstable();
        Self { side, keys }
    }

    fn cell(p: [f32; 3], side: f32) -> [i64; 3] {
        p.map(|v| (v / side).floor() as i64)
    }

    fn key(c: [i64; 3]) -> u64 {
        let k = |v: i64| ((v + Self::OFFSET).clamp(0, (1 << 21) - 1)) as u64;
        k(c[0]) << 42 | k(c[1]) << 21 | k(c[2])
    }

    fn any_within(&self, splats: &Splats, p: [f32; 3], radius: f32) -> bool {
        let c = Self::cell(p, self.side);
        let r2 = radius * radius;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let key = Self::key([c[0] + dx, c[1] + dy, c[2] + dz]);
                    let start = self.keys.partition_point(|e| e.0 < key);
                    if self.keys[start..].iter().take_while(|e| e.0 == key).any(|e| distance2(splats.means[e.1 as usize], p) <= r2) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;

    /// A camera at the origin looking down +Z (Unity's world), 200 × 100 pixels.
    fn camera() -> Camera {
        Camera::look([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, 200, 100)
    }

    fn splat(g: &mut Gaussians, at: [f32; 3], size: f32, opacity: f32) {
        g.push(at, [size.ln(); 3], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(opacity), [0.5; 3]);
    }

    /// What each tool costs on a real reconstruction, from its first recorded camera at 1920 pixels across, then on
    /// four copies of it side by side (about three million splats for VRChat Home): `ORBITERS_TEST_SESSION=<a
    /// reconstructed session> cargo test --release -p orbiters-reconstruct selection_timings -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn selection_timings() {
        use std::time::Instant;
        let session = std::path::PathBuf::from(std::env::var("ORBITERS_TEST_SESSION").expect("ORBITERS_TEST_SESSION"));
        let g = crate::ply::read(&orbiters_core::reconstruction::result_dir(&session).join(orbiters_core::reconstruction::PLY_FILE)).unwrap();
        let c = crate::tree_tune::session_cameras(&session).unwrap()[0];
        let k = 1920.0 / c.width as f32;
        let cam = Camera { fx: c.fx * k, fy: c.fy * k, cx: (c.cx + 0.5) * k - 0.5, cy: (c.cy + 0.5) * k - 0.5, width: 1920, height: (c.height as f32 * k).round() as u32, ..c };
        for copies in [1usize, 4] {
            let mut big = g.clone();
            for k in 1..copies {
                let mut more = g.clone();
                for m in &mut more.means {
                    m[0] += 200.0 * k as f32;
                }
                for i in 0..more.len() {
                    big.push(more.means[i], more.log_scales[i], more.rotations[i], more.opacity_logits[i], more.colors[i]);
                }
            }
            let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
            let t = Instant::now();
            let s = Splats::new(&big);
            let made = ms(t);
            let mut marks = vec![0u8; s.len()];
            let t = Instant::now();
            let view = s.view(&cam, &marks);
            let projected = ms(t);
            let (x, y) = (cam.width as f32 / 2.0, cam.height as f32 / 2.0);
            let t = Instant::now();
            let clicked = view.clicked(x, y);
            let click = ms(t);
            let t = Instant::now();
            let painted = view.visible_in_circle([x, y], 48.0, BRUSH_WEIGHT);
            let brush = ms(t);
            let t = Instant::now();
            let boxed = in_rectangle(&s, &marks, &cam, [x - 200.0, y - 150.0], [x + 200.0, y + 150.0]);
            let rectangle = ms(t);
            let hit = view.pick(x, y);
            let t = Instant::now();
            let sphered = hit.map(|h| in_sphere(&s, &marks, h.point, 0.5)).unwrap_or_default();
            let sphere = ms(t);
            for &i in &painted {
                marks[i as usize] = SELECTED;
            }
            let radius = step(&s, &marks);
            let t = Instant::now();
            let grown = grow(&s, &marks, radius);
            let grow_ms = ms(t);
            let t = Instant::now();
            let shrunk = shrink(&s, &marks, radius);
            let shrink_ms = ms(t);
            println!(
                "selection_timings {}",
                serde_json::json!({
                    "splats": s.len(), "on_screen": view.len(), "make_ms": made, "project_ms": projected,
                    "click_ms": click, "clicked": clicked.len(), "brush48_ms": brush, "painted": painted.len(),
                    "box_ms": rectangle, "boxed": boxed.len(), "sphere_ms": sphere, "sphered": sphered.len(),
                    "step_m": radius, "grow_ms": grow_ms, "grown": grown.len(), "shrink_ms": shrink_ms, "shrunk": shrunk.len(),
                })
            );
        }
    }

    /// A wall of opaque discs at 3 m, a second wall behind it at 5 m, a floater at 2 m off to the side.
    fn scene() -> Gaussians {
        let mut g = Gaussians::default();
        for wall in [3.0f32, 5.0] {
            for y in -10..=10 {
                for x in -20..=20 {
                    splat(&mut g, [x as f32 * 0.1, y as f32 * 0.1, wall], 0.06, 0.98);
                }
            }
        }
        splat(&mut g, [0.6, 0.0, 2.0], 0.05, 0.9);
        g
    }

    fn px(cam: &Camera, p: [f32; 3]) -> [f32; 2] {
        let (x, y, _) = cam.project(p).unwrap();
        [x, y]
    }

    #[test]
    fn a_click_takes_what_it_sees_and_goes_through_what_is_hidden() {
        let g = scene();
        let s = Splats::new(&g);
        let cam = camera();
        let mut marks = vec![0u8; g.len()];
        let floater = (g.len() - 1) as u32;
        let at = px(&cam, g.means[floater as usize]);
        let hit = s.view(&cam, &marks).pick(at[0], at[1]).unwrap();
        assert_eq!(hit.id, floater, "the nearest splat over the wall");
        assert!((hit.point[2] - 2.0).abs() < 0.1, "{hit:?}");
        assert!(s.view(&cam, &marks).clicked(at[0], at[1]).contains(&floater));
        // Hidden, it is not drawn: the click reaches the wall behind it, and its point is on the wall.
        marks[floater as usize] = HIDDEN;
        let hit = s.view(&cam, &marks).pick(at[0], at[1]).unwrap();
        assert_ne!(hit.id, floater);
        assert!((g.means[hit.id as usize][2] - 3.0).abs() < 1e-3 && (hit.point[2] - 3.0).abs() < 0.15, "{hit:?}");
        // The first wall hidden too: the second.
        for (i, m) in marks.iter_mut().enumerate() {
            if g.means[i][2] == 3.0 {
                *m = DELETED;
            }
        }
        let hit = s.view(&cam, &marks).pick(at[0], at[1]).unwrap();
        assert!((g.means[hit.id as usize][2] - 5.0).abs() < 1e-3, "{hit:?}");
        // Past the walls: nothing.
        assert!(s.view(&cam, &marks).pick(1.0, 1.0).is_none());
    }

    /// The weights under a pixel are the viewer's: their sum is one less the transmittance the CPU reference renders.
    #[test]
    fn the_weights_under_a_pixel_match_the_reference_renderer() {
        let mut g = Gaussians::default();
        let mut rng = fastrand::Rng::with_seed(7);
        for _ in 0..300 {
            let at = [rng.f32() * 2.0 - 1.0, rng.f32() - 0.5, 2.0 + rng.f32() * 3.0];
            g.push(
                at,
                [(0.02 + rng.f32() * 0.1).ln(), (0.02 + rng.f32() * 0.1).ln(), (0.01 + rng.f32() * 0.05).ln()],
                [rng.f32(), rng.f32(), rng.f32(), rng.f32()],
                rng.f32() * 4.0 - 2.0,
                [0.5; 3],
            );
        }
        let cam = camera();
        let reference = crate::cpu::render(&g, &cam, [0.0; 3], crate::cpu::Rules::GPU);
        let view = Splats::new(&g).view(&cam, &[]);
        for (x, y) in [(100u32, 50u32), (60, 30), (150, 70), (20, 90)] {
            let shown: f32 = view.under(x as f32, y as f32).iter().map(|u| u.1).sum();
            let t = reference.transmittance[(y * cam.width + x) as usize] as f32;
            assert!((1.0 - shown - t).abs() < 2e-3, "pixel {x},{y}: 1 − Σw = {} against {t}", 1.0 - shown);
        }
    }

    #[test]
    fn a_brush_paints_the_surface_it_sees_and_not_what_lies_behind() {
        let g = scene();
        let s = Splats::new(&g);
        let cam = camera();
        let marks = vec![0u8; g.len()];
        let centre = px(&cam, [-0.5, 0.0, 3.0]);
        let ids = s.view(&cam, &marks).visible_in_circle(centre, 12.0, BRUSH_WEIGHT);
        assert!(ids.len() >= 4, "{} splats", ids.len());
        for &i in &ids {
            let m = g.means[i as usize];
            assert_eq!(m[2], 3.0, "the front wall only: {m:?}");
            let p = px(&cam, m);
            assert!(((p[0] - centre[0]).powi(2) + (p[1] - centre[1]).powi(2)).sqrt() < 12.0 + 6.0, "near the circle: {p:?}");
        }
        // A box goes through: both walls.
        let b = in_rectangle(&s, &marks, &cam, [centre[0] - 10.0, centre[1] - 10.0], [centre[0] + 10.0, centre[1] + 10.0]);
        assert!(b.iter().any(|&i| g.means[i as usize][2] == 3.0) && b.iter().any(|&i| g.means[i as usize][2] == 5.0));
        assert!(b.iter().all(|&i| px(&cam, g.means[i as usize])[0] <= centre[0] + 10.0));
    }

    #[test]
    fn a_sphere_holds_the_centres_within_it_and_grow_and_shrink_follow_neighbours() {
        let g = scene();
        let s = Splats::new(&g);
        let mut marks = vec![0u8; g.len()];
        let ids = in_sphere(&s, &marks, [0.0, 0.0, 3.0], 0.15);
        // The disc's centre and its four neighbours 10 cm away, and the four diagonals at 14 cm.
        assert_eq!(ids.len(), 9, "{ids:?}");
        let centre = ids.iter().copied().find(|&i| g.means[i as usize] == [0.0, 0.0, 3.0]).unwrap();
        marks[centre as usize] = SELECTED;
        assert!((step(&s, &marks) - 0.18).abs() < 1e-3, "three sizes: {}", step(&s, &marks));
        let grown = grow(&s, &marks, 0.11);
        assert_eq!(grown.len(), 4, "the four neighbours");
        for &i in &grown {
            marks[i as usize] = SELECTED;
        }
        let grown = grow(&s, &marks, 0.11);
        assert_eq!(grown.len(), 8, "the next ring");
        // Of the five selected, shrink takes back the four at the border (each has an unselected neighbour), not the
        // centre (all of its neighbours are selected).
        let shrunk = shrink(&s, &marks, 0.11);
        assert_eq!(shrunk.len(), 4, "{shrunk:?}");
        assert!(!shrunk.contains(&centre));
        // A hidden neighbour is no neighbour: grow never brings it in.
        let left = ids.iter().copied().find(|&i| g.means[i as usize] == [-0.1, 0.0, 3.0]).unwrap();
        let mut only = vec![0u8; g.len()];
        only[centre as usize] = SELECTED;
        only[left as usize] = HIDDEN;
        assert!(!grow(&s, &only, 0.11).contains(&left));
        assert!(grow(&s, &vec![0u8; g.len()], 0.11).is_empty(), "nothing selected, nothing grows");
    }
}
