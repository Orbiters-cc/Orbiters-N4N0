//! The sky the capture never saw. Training keeps the sky shell (crate::init::SKY out) only where some view looked;
//! above the highest gaze and in the directions nobody faced, nothing is left, and a viewer looking there sees black.
//! [`complete`] bins the far-field Gaussians by direction from the cameras' centre (5.6° cells) and backs every cell
//! from the horizon up that they do not cover whole with a large opaque Gaussian a tenth beyond the shell, coloured
//! like the cell's own sky where it was seen, else like the nearest seen sky of its elevation band (else the nearest
//! band that has any): the gradient of the seen sky carries on instead of a hole, and behind a sparsely seen cell its
//! gaps show sky instead of the black training background (the dark streaks along the edge of what was seen).

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

/// Lab: the sky as an equirectangular map behind the splats (crate::gpu::Gpu::set_sky, shaders/sky_lookup.wgsl) in
/// place of the shell of discs crate::init::SKY out, which slides by f·b/R pixels as the eye moves b (31 px over a 10 m
/// walk at 1233 px) and never densifies. `width` × `width / 2` texels of encoded sRGB, longitude `atan2(x, z)` across
/// from −180°, elevation from +90° down.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyMap {
    pub width: u32,
    pub rgb: Vec<[f32; 3]>,
}

/// What seeding a [`SkyMap`] found: keyframe pixels whose rays hit nothing in the depth frame nearest in time, the
/// texels they reached, and of the texels seen from both halves of the walk (split at its median position along its
/// longest extent) those whose colour differs between the halves: finite content without depth shows parallax.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkySeeding {
    pub pixels: usize,
    pub texels_seen: usize,
    pub texels_tested: usize,
    pub texels_parallax: usize,
}

/// A texel whose colour differs by more than this (0–1, any channel) between the walk's halves shows parallax.
const PARALLAX: f64 = 0.05;
/// Pixels a texel needs from each half of the walk to be tested.
const TESTED: f64 = 4.0;

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
    let mut sum = vec![[[0f64; 4]; 2]; w * h];
    let mut found = SkySeeding::default();
    let (iw, ih) = (data.width, data.height);
    for (i, v) in data.views.iter().enumerate() {
        if held_out.contains(&i) {
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
                let t = &mut sum[texel([far[0] - origin[0], far[1] - origin[1], far[2] - origin[2]], w)][half];
                for k in 0..3 {
                    t[k] += c[k] as f64;
                }
                t[3] += 1.0;
                found.pixels += 1;
            }
        }
    }
    let mut seen: Vec<Option<[f32; 3]>> = vec![None; w * h];
    for (t, s) in sum.iter().enumerate() {
        let n = s[0][3] + s[1][3];
        if n == 0.0 {
            continue;
        }
        seen[t] = Some([0, 1, 2].map(|k| ((s[0][k] + s[1][k]) / n) as f32));
        found.texels_seen += 1;
        if s[0][3] >= TESTED && s[1][3] >= TESTED {
            found.texels_tested += 1;
            if (0..3).any(|k| (s[0][k] / s[0][3] - s[1][k] / s[1][3]).abs() > PARALLAX) {
                found.texels_parallax += 1;
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

impl SkyMap {
    /// `sky.bin` beside the PLY: "OSKY", the width (u32, little-endian), then each texel's rgb as f32.
    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let mut bytes = Vec::with_capacity(8 + self.rgb.len() * 12);
        bytes.extend_from_slice(b"OSKY");
        bytes.extend_from_slice(&self.width.to_le_bytes());
        for c in &self.rgb {
            for v in c {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
    }

    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        anyhow::ensure!(bytes.len() >= 8 && &bytes[..4] == b"OSKY", "{} is no sky map", path.display());
        let width = u32::from_le_bytes(bytes[4..8].try_into().expect("four bytes"));
        let n = width as usize * (width as usize / 2);
        anyhow::ensure!(bytes.len() == 8 + n * 12, "{}: {} bytes for a map {width} across", path.display(), bytes.len());
        let f = |i: usize| f32::from_le_bytes(bytes[8 + 4 * i..12 + 4 * i].try_into().expect("four bytes"));
        Ok(Self { width, rgb: (0..n).map(|t| [f(3 * t), f(3 * t + 1), f(3 * t + 2)]).collect() })
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
        let map = SkyMap { width: 8, rgb: (0..32).map(|i| [i as f32, 0.5, -1.0]).collect() };
        let path = std::env::temp_dir().join(format!("orbiters-sky-{}.bin", std::process::id()));
        map.write(&path).unwrap();
        assert_eq!(SkyMap::read(&path).unwrap(), map);
        std::fs::remove_file(&path).ok();
    }
}
