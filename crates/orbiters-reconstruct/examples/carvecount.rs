//! Lab: how many Gaussians of a trained PLY lie behind every measured surface (orbiters_reconstruct::carve), with the
//! session's depth validated first (orbiters_reconstruct::depthfix); writes the PLY without them.
//!
//! ```text
//! cargo run --release --example carvecount -- <session> <splats.ply> <out.ply>
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::{carve, dataset, depthfix, ply};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let session = PathBuf::from(it.next().context("usage: carvecount <session> <splats.ply> <out.ply>")?);
    let input = PathBuf::from(it.next().context("splats.ply")?);
    let output = PathBuf::from(it.next().context("out.ply")?);
    let mut data = dataset::load(&session, 1920, |_| {})?;
    depthfix::mark_untrusted(&mut data, &depthfix::FixParams::default());
    let mut g = ply::read(&input)?;
    let t = std::time::Instant::now();
    let kill = carve::behind_surfaces(&g, &data, &carve::CarveParams::default());
    let n = g.len();
    let mut keep = vec![true; n];
    for &i in &kill {
        keep[i as usize] = false;
    }
    // How opaque the carved ones are (faint ones matter less).
    let opaque = kill.iter().filter(|&&i| orbiters_reconstruct::gaussians::sigmoid(g.opacity_logits[i as usize]) > 0.5).count();
    println!("{} of {n} Gaussians ({:.1} %) behind every measured surface ({opaque} of them opaque), {:.1} s", kill.len(), 100.0 * kill.len() as f64 / n as f64, t.elapsed().as_secs_f32());
    g.retain(&keep);
    ply::write(&output, &g)?;
    Ok(())
}
