//! Lab: free-space transient detection on a session (orbiters_reconstruct::transients): statistics, and keyframes with
//! their transient pixels tinted red.
//!
//! ```text
//! cargo run --release --example transients -- <session> <out dir> [keyframe]...
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::{dataset, transients};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let session = PathBuf::from(it.next().context("usage: transients <session> <out dir> [keyframe]...")?);
    let out = PathBuf::from(it.next().context("out dir")?);
    let picks: Vec<i32> = it.map(|k| k.parse()).collect::<Result<_, _>>()?;
    std::fs::create_dir_all(&out)?;
    let data = dataset::load(&session, 1920, |_| {})?;
    // How far apart in time colour keyframes and depth frames are.
    let mut gaps: Vec<f32> = data.views.iter().map(|v| data.depth_views.iter().map(|d| (d.game_time - v.game_time).abs()).fold(f32::MAX, f32::min)).collect();
    gaps.sort_by(f32::total_cmp);
    println!("{} keyframes, {} depth frames; keyframe to nearest depth frame: median {:.3} s, max {:.3} s", data.views.len(), data.depth_views.len(), gaps[gaps.len() / 2], gaps[gaps.len() - 1]);
    let params = transients::Params::default();
    let t = Instant::now();
    let flags = transients::detect(&data, &params);
    let valid: usize = data.depth_views.iter().map(|d| d.valid()).sum();
    let found: usize = flags.iter().map(|f| f.iter().filter(|x| **x).count()).sum();
    let frames_with = flags.iter().filter(|f| f.iter().any(|x| *x)).count();
    println!("detect {:.1} s: {found} transient of {valid} samples ({:.2} %), in {frames_with} of {} depth frames", t.elapsed().as_secs_f32(), 100.0 * found as f64 / valid.max(1) as f64, data.depth_views.len());
    let t = Instant::now();
    let masks = transients::keyframe_masks(&data, &flags, 0.06, 1.5);
    let masked: Vec<f64> = masks.iter().map(|m| m.as_ref().map_or(0.0, |m| m.iter().filter(|x| **x > 0.5).count() as f64 / m.len() as f64)).collect();
    println!("masks {:.1} s: {} keyframes with transient pixels; share of pixels per keyframe: mean {:.2} %, max {:.2} %", t.elapsed().as_secs_f32(), masks.iter().filter(|m| m.is_some()).count(), 100.0 * masked.iter().sum::<f64>() / masked.len() as f64, 100.0 * masked.iter().cloned().fold(0.0, f64::max));
    let t = Instant::now();
    let colors = transients::color_transients(&data, &transients::ColorParams::default());
    let outl: usize = colors.outliers.iter().map(|o| o.len()).sum();
    let unst: usize = colors.unstable.iter().map(|o| o.len()).sum();
    println!("colour check {:.1} s: {outl} outlier points, {unst} unstable points left out", t.elapsed().as_secs_f32());
    for (i, v) in data.views.iter().enumerate() {
        if !picks.contains(&v.keyframe) {
            continue;
        }
        let (w, h) = (data.width, data.height);
        let outlier = transients::disc_mask(&colors.outliers[i], w, h);
        let unstable = transients::disc_mask(&colors.unstable[i], w, h);
        let share = |m: &[f32]| 100.0 * m.iter().filter(|x| **x > 0.5).count() as f64 / m.len() as f64;
        println!("keyframe {}: depth {:.2} %, colour outliers {:.2} %, unstable {:.2} %", v.keyframe, 100.0 * masked[i], share(&outlier), share(&unstable));
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for (k, p) in v.image.iter().enumerate() {
            let c = [p & 255, (p >> 8) & 255, (p >> 16) & 255];
            // Red: depth transients, yellow: colour outliers, blue: unstable (kept by another keyframe).
            let tint = if masks[i].as_ref().is_some_and(|m| m[k] > 0.5) {
                Some([255.0, 0.0, 0.0])
            } else if outlier[k] > 0.5 {
                Some([255.0, 230.0, 0.0])
            } else if unstable[k] > 0.5 {
                Some([0.0, 120.0, 255.0])
            } else {
                None
            };
            for (ch, value) in c.iter().enumerate() {
                let x = *value as f32;
                rgb.push(match tint { Some(t) => (0.4 * x + 0.6 * t[ch]) as u8, None => x as u8 });
            }
        }
        let file = std::io::BufWriter::new(std::fs::File::create(out.join(format!("transient_{:06}.png", v.keyframe)))?);
        let mut enc = png::Encoder::new(file, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgb)?;
    }
    Ok(())
}
