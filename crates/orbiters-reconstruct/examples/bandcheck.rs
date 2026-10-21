//! Lab: does a session's picture carry the skeleton band, decoded or not? The trainer masks the band only when the
//! session decoded skeleton frames; worlds whose post-processing breaks the band's symbols (chromatic aberration at
//! the bottom of the frame) then train its codes as scene. Prints the strip's source and, per sampled keyframe, how
//! well the band tiles' pilot rows (levels 0, 1, 2, 3 repeated) correlate with the expected ramp, in the red channel.
//!
//! ```text
//! cargo run --release --example bandcheck -- <session> [every-nth-keyframe]
//! ```

use anyhow::Context as _;
use orbiters_contracts::depth::{LEVELS, TILE_CELLS};
use orbiters_contracts::frame_record::record_flags;
use orbiters_contracts::{skeleton, strip};
use orbiters_core::bundle;
use orbiters_reconstruct::dataset;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let session = PathBuf::from(it.next().context("usage: bandcheck <session> [every]")?);
    let every: usize = it.next().map(|s| s.parse()).transpose()?.unwrap_or(10);
    let manifest = bundle::read_manifest(&session)?;
    let records = bundle::read_records(&session)?;
    let mut sources = std::collections::BTreeMap::<u32, usize>::new();
    for r in records.iter().filter(|r| r.flags & record_flags::STRIP_OK != 0) {
        *sources.entry(strip::flags::source(r.strip_flags)).or_default() += 1;
    }
    println!("{}: skeleton frames {}, strip sources {:?} (0 avatar kit, 1 simulator, 2 test)", session.display(), manifest.performance.skeleton_frames, sources);
    let data = dataset::load(&session, 1920, |_| {})?;
    let (w, h) = (data.width, data.height);
    let band = skeleton::BandLayout::new(w, h).context("no band layout at this size")?;
    let masked = (0..(w * h) as usize).filter(|&i| data.mask[i] < 0.5 && (i as u32 / w) >= band.y0 - band.cell).count();
    println!("  band at y {}..{}, x {}..{}; mask covers {} pixels there", band.y0, h, band.x0, band.x0 + band.per_row * band.tile_px, masked);
    let ramp: Vec<f32> = (0..TILE_CELLS).map(|j| (j % LEVELS as u32) as f32).collect();
    let mut present = 0usize;
    let mut sampled = 0usize;
    for v in data.views.iter().step_by(every.max(1)) {
        let red = |x: u32, y: u32| (v.image[(y * w + x) as usize] & 255) as f32;
        let mut sum = 0.0;
        let mut n = 0;
        for p in 0..(band.per_row * band.rows) as usize {
            let (x0, y0) = band.tile_origin(p);
            let cells: Vec<f32> = (0..TILE_CELLS).map(|j| red(x0 + j * band.cell + band.cell / 2, y0 + band.cell / 2)).collect();
            sum += correlation(&cells, &ramp);
            n += 1;
        }
        let c = sum / n as f32;
        sampled += 1;
        if c > 0.5 {
            present += 1;
        }
        println!("  keyframe {:4}: mean pilot correlation {c:.2}", v.keyframe);
    }
    println!("  band present in {present} of {sampled} sampled keyframes");
    Ok(())
}

fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        ab += (x - ma) * (y - mb);
        aa += (x - ma) * (x - ma);
        bb += (y - mb) * (y - mb);
    }
    if aa <= 1e-6 || bb <= 1e-6 {
        0.0
    } else {
        ab / (aa * bb).sqrt()
    }
}
