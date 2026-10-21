//! Lab: a session's depth frames as pictures. For each chosen keyframe, writes the keyframe beside the decoded depth of
//! its nearest depth frame (inverse depth as grey, no depth in dark red) at the keyframe's size.
//!
//! ```text
//! cargo run --release --example depthview -- <session> <out dir> <keyframe>...
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::dataset;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let session = PathBuf::from(it.next().context("usage: depthview <session> <out dir> <keyframe>...")?);
    let out = PathBuf::from(it.next().context("out dir")?);
    let picks: Vec<i32> = it.map(|k| k.parse()).collect::<Result<_, _>>()?;
    std::fs::create_dir_all(&out)?;
    let data = dataset::load(&session, 1920, |_| {})?;
    let (w, h) = (data.width as usize, data.height as usize);
    let valid: usize = data.depth_views.iter().map(|d| d.valid()).sum();
    let total: usize = data.depth_views.iter().map(|d| d.depth.len()).sum();
    println!("{} depth frames, {:.1} % of samples with depth", data.depth_views.len(), 100.0 * valid as f64 / total.max(1) as f64);
    for v in data.views.iter().filter(|v| picks.contains(&v.keyframe)) {
        let d = data.depth_views.iter().min_by(|a, b| (a.game_time - v.game_time).abs().total_cmp(&(b.game_time - v.game_time).abs())).context("no depth frames")?;
        let mut rgb = vec![0u8; 2 * w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let p = v.image[y * w + x];
                let o = (y * 2 * w + x) * 3;
                rgb[o] = (p & 255) as u8;
                rgb[o + 1] = ((p >> 8) & 255) as u8;
                rgb[o + 2] = ((p >> 16) & 255) as u8;
            }
        }
        // Each sample paints the block of pixels around its own pixel (sample spacing).
        let half = (d.spacing / 2.0).ceil() as i64;
        let (mut with, mut seen) = (0, 0);
        for (i, px) in d.pixels.iter().enumerate() {
            let (cx, cy) = (px[0].round() as i64, px[1].round() as i64);
            let z = d.depth[i];
            let c = if z > 0.0 {
                with += 1;
                let g = (255.0 * (1.0 / z).min(1.0).powf(0.5)) as u8;
                [g, g, g]
            } else {
                [90, 0, 0]
            };
            seen += 1;
            for yy in (cy - half).max(0)..(cy + half).min(h as i64) {
                for xx in (cx - half).max(0)..(cx + half).min(w as i64) {
                    let o = (yy as usize * 2 * w + w + xx as usize) * 3;
                    rgb[o..o + 3].copy_from_slice(&c);
                }
            }
        }
        println!("keyframe {} (t {:.2} s) beside depth frame at t {:.2} s: {:.1} % of its samples with depth", v.keyframe, v.game_time, d.game_time, 100.0 * with as f64 / seen.max(1) as f64);
        let file = std::io::BufWriter::new(std::fs::File::create(out.join(format!("depth_{:06}.png", v.keyframe)))?);
        let mut enc = png::Encoder::new(file, 2 * w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgb)?;
    }
    Ok(())
}
