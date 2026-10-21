//! Lab: what a session's depth sees, in camera space. Per depth frame (a dozen spread over the session): the share of
//! samples with depth, and the median eye depth in the central third of the frame; then whether depth samples hold
//! still in the camera (something attached to the viewer) or in the world (the scene): for pairs of depth frames, the
//! share of samples whose eye depth stays within 2 % at the same pixel against the share whose world point stays put.
//!
//! ```text
//! cargo run --release --example depthstats -- <session>
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::dataset;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let session = PathBuf::from(std::env::args().nth(1).context("usage: depthstats <session>")?);
    let data = dataset::load(&session, 1920, |_| {})?;
    let dv = &data.depth_views;
    let (w, h) = (data.width as f32, data.height as f32);
    for d in dv.iter().step_by((dv.len() / 12).max(1)) {
        let mut centre: Vec<f32> = d.pixels.iter().zip(&d.depth).filter(|(p, z)| **z > 0.0 && p[0] > w / 3.0 && p[0] < 2.0 * w / 3.0 && p[1] > h / 3.0 && p[1] < 2.0 * h / 3.0).map(|(_, z)| *z).collect();
        centre.sort_by(f32::total_cmp);
        let med = centre.get(centre.len() / 2).copied().unwrap_or(0.0);
        let (lo, hi) = (centre.get(centre.len() / 10).copied().unwrap_or(0.0), centre.get(centre.len() * 9 / 10).copied().unwrap_or(0.0));
        println!("t {:8.2} s  {:5.1} % with depth; centre third: {} samples, eye depth p10 {lo:.2} m, median {med:.2} m, p90 {hi:.2} m", d.game_time, 100.0 * d.valid() as f64 / d.depth.len() as f64, centre.len());
    }
    // Camera-held against world-held samples, frames 0.5 s and more apart while the camera moved at least 0.5 m.
    let (mut cam_held, mut world_held, mut both, mut pairs) = (0usize, 0usize, 0usize, 0usize);
    let mut total = 0usize;
    for a in (0..dv.len()).step_by(3) {
        let Some(b) = (a + 1..dv.len()).find(|&b| dv[b].game_time - dv[a].game_time >= 0.5) else { continue };
        let (pa, pb) = (dv[a].camera.position(), dv[b].camera.position());
        let moved = ((pa[0] - pb[0]).powi(2) + (pa[1] - pb[1]).powi(2) + (pa[2] - pb[2]).powi(2)).sqrt();
        if moved < 0.5 || dv[a].depth.len() != dv[b].depth.len() {
            continue;
        }
        pairs += 1;
        for i in 0..dv[a].depth.len() {
            let (za, zb) = (dv[a].depth[i], dv[b].depth[i]);
            if za <= 0.0 {
                continue;
            }
            total += 1;
            let same_pixel = zb > 0.0 && (za - zb).abs() <= 0.02 * za;
            // Where frame a's world point lands in frame b, and whether b's depth there agrees.
            let p = dv[a].camera.unproject(dv[a].pixels[i][0], dv[a].pixels[i][1], za);
            let mut in_world = false;
            if let Some((u, v, z)) = dv[b].camera.project(p) {
                let near = dv[b].pixels.iter().enumerate().filter(|(_, q)| (q[0] - u).abs() <= dv[b].spacing && (q[1] - v).abs() <= dv[b].spacing).map(|(k, _)| dv[b].depth[k]);
                in_world = near.into_iter().any(|zz| zz > 0.0 && (zz - z).abs() <= 0.05f32.max(0.02 * z));
            }
            match (same_pixel, in_world) {
                (true, true) => both += 1,
                (true, false) => cam_held += 1,
                (false, true) => world_held += 1,
                _ => {}
            }
        }
        if pairs >= 12 {
            break;
        }
    }
    let pct = |n: usize| 100.0 * n as f64 / total.max(1) as f64;
    println!("{pairs} frame pairs >= 0.5 s apart with the camera >= 0.5 m apart: of {total} samples with depth, {:.1} % held still in the camera only (same pixel, same eye depth), {:.1} % in the world only, {:.1} % both", pct(cam_held), pct(world_held), pct(both));
    Ok(())
}
