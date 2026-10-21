//! Lab: world heights (y) of one depth frame's samples, as a histogram in 0.25 m bins, with the camera's height. A
//! standing avatar's feet mark the floor it stands on; if the depth's floor lies metres below them, the visible floor
//! is missing from depth and depth shows what lies under it.
//!
//! ```text
//! cargo run --release --example heights -- <session> <keyframe>
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::dataset;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let session = PathBuf::from(it.next().context("usage: heights <session> <keyframe>")?);
    let pick: i32 = it.next().context("keyframe")?.parse()?;
    let data = dataset::load(&session, 1920, |_| {})?;
    let v = data.views.iter().find(|v| v.keyframe == pick).context("no such keyframe")?;
    let d = data.depth_views.iter().min_by(|a, b| (a.game_time - v.game_time).abs().total_cmp(&(b.game_time - v.game_time).abs())).context("no depth")?;
    let eye = d.camera.position();
    println!("keyframe {pick}: camera at height {:.2} m", eye[1]);
    let mut bins = std::collections::BTreeMap::<i32, (usize, f32, f32)>::new();
    for (i, &z) in d.depth.iter().enumerate() {
        if z <= 0.0 {
            continue;
        }
        let p = d.camera.unproject(d.pixels[i][0], d.pixels[i][1], z);
        let b = (p[1] / 0.25).floor() as i32;
        let e = bins.entry(b).or_insert((0, f32::MAX, f32::MIN));
        e.0 += 1;
        e.1 = e.1.min(d.pixels[i][0]);
        e.2 = e.2.max(d.pixels[i][0]);
    }
    for (b, (n, x0, x1)) in bins {
        if n >= 20 {
            println!("  y {:6.2} .. {:6.2} m: {n:6} samples, pixel columns {x0:.0}..{x1:.0}", b as f32 * 0.25, (b + 1) as f32 * 0.25);
        }
    }
    Ok(())
}
