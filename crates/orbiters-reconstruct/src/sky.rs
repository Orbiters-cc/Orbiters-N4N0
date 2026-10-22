//! The sky the capture never saw. Training keeps the sky shell (crate::init::SKY out) only where some view looked;
//! above the highest gaze and in the directions nobody faced, nothing is left, and a viewer looking there sees black.
//! [`complete`] bins the far-field Gaussians by direction from the cameras' centre (5.6° cells) and backs every cell
//! from the horizon up that they do not cover whole with a large opaque Gaussian a tenth beyond the shell, coloured
//! like the cell's own sky where it was seen, else like the nearest seen sky of its elevation band (else the nearest
//! band that has any): the gradient of the seen sky carries on instead of a hole, and behind a sparsely seen cell its
//! gaps show sky instead of the black training background (the dark streaks along the edge of what was seen).

use crate::gaussians::{logit, sigmoid, Gaussians};

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
}
