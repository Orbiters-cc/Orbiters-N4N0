//! The sky the capture never saw. Training keeps the sky shell (crate::init::SKY out) only where some view looked;
//! above the highest gaze and in the directions nobody faced, nothing is left, and a viewer looking there sees black.
//! [`complete`] bins the far-field Gaussians by direction from the cameras' centre (5.6° cells) and covers every empty
//! cell from the horizon up with a large opaque Gaussian on the shell, coloured like the nearest seen sky of its
//! elevation band (else the nearest band that has any): the gradient of the seen sky carries on instead of a hole.

use crate::gaussians::{logit, sigmoid, Gaussians};

const AZIMUTHS: usize = 64;
const ELEVATIONS: usize = 32;
/// Cells from this elevation (degrees) up are completed; lower ones lie behind the ground anyway.
const LOWEST_DEG: f32 = -6.0;
/// Opacity a cell needs (summed over its Gaussians) to count as seen.
const SEEN: f32 = 0.5;

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
    for i in 0..g.len() {
        let d = [g.means[i][0] - centre[0], g.means[i][1] - centre[1], g.means[i][2] - centre[2]];
        let r = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if r < far_field {
            continue;
        }
        let (a, e) = cell([d[0] / r, d[1] / r, d[2] / r]);
        let w = sigmoid(g.opacity_logits[i]);
        for c in 0..3 {
            sum[e * AZIMUTHS + a][c] += w * g.colors[i][c];
        }
        weight[e * AZIMUTHS + a] += w;
    }
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
    for e in lowest..ELEVATIONS {
        let el = ((e as f32 + 0.5) / ELEVATIONS as f32 - 0.5) * std::f32::consts::PI;
        for a in 0..AZIMUTHS {
            if seen(a, e) {
                continue;
            }
            // The nearest seen cell of the band, fading into the band's colour over a quarter turn.
            let nearest = (1..=AZIMUTHS / 2)
                .find_map(|k| [(a + k) % AZIMUTHS, (a + AZIMUTHS - k) % AZIMUTHS].into_iter().find(|&j| seen(j, e)).map(|j| (k, colour(j, e))));
            let base = band[e].expect("every band has a colour");
            let c = match nearest {
                Some((k, near)) => {
                    let t = (k as f32 / (AZIMUTHS as f32 / 4.0)).min(1.0);
                    [0, 1, 2].map(|i| near[i] + (base[i] - near[i]) * t)
                }
                None => base,
            };
            let az = (a as f32 + 0.5) / AZIMUTHS as f32 * std::f32::consts::TAU;
            let dir = [el.cos() * az.sin(), el.sin(), el.cos() * az.cos()];
            let p = [centre[0] + dir[0] * distance, centre[1] + dir[1] * distance, centre[2] + dir[2] * distance];
            // Wide enough to overlap its neighbours: the cell's larger side.
            let size = (std::f32::consts::PI / ELEVATIONS as f32).max(std::f32::consts::TAU / AZIMUTHS as f32 * el.cos()) * distance * 0.7;
            g.push(p, [size.ln(); 3], [1.0, 0.0, 0.0, 0.0], logit(0.98), c);
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
        // Nothing far below the horizon.
        assert!((before..g.len()).all(|i| g.means[i][1] > -400.0 * (LOWEST_DEG - 6.0).to_radians().sin().abs()));
    }
}
