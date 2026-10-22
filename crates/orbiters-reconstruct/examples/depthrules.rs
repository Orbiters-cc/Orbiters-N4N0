//! Which depth samples each validation rule (orbiters_reconstruct::depthfix) marks unknown, drawn over the keyframe
//! nearest in time: to trace what a rule does to a session before changing it (the review's SPL-3, the floater at
//! Exoplanet's keyframe 60).
//!
//! ```text
//! cargo run --release --example depthrules -- <session> <out dir> [--width N] [--keyframes A..B]
//! ```
//! Each depth frame whose nearest keyframe lies in A..B (all by default) becomes `depth_<index>.png`: that keyframe
//! with each marked sample as a dot, yellow the decoder's unknowns, red "never reads" (rule 1, which runs only on
//! sessions whose decoder marks nothing), magenta misread tiles (rule 2), cyan impossibly near samples (rule 3). Each
//! rule runs alone on the frames as loaded; samples no rule marks, and tiles under the strip, are not drawn.

use anyhow::Context as _;
use orbiters_reconstruct::dataset::{self, Budget, Marks};
use orbiters_reconstruct::depthfix::{self, FixParams};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut session, mut out, mut width, mut range) = (None, None, 1920u32, (i32::MIN, i32::MAX));
    while let Some(a) = it.next() {
        match a.as_str() {
            "--width" => width = it.next().context("--width")?.parse()?,
            "--keyframes" => {
                let r = it.next().context("--keyframes A..B")?;
                let (a, b) = r.split_once("..").context("--keyframes A..B")?;
                range = (a.parse()?, b.parse()?);
            }
            _ if session.is_none() => session = Some(PathBuf::from(a)),
            _ => out = Some(PathBuf::from(a)),
        }
    }
    let (session, out) = (session.context("usage: depthrules <session> <out dir>")?, out.context("usage: depthrules <session> <out dir>")?);
    // Every depth frame (a trace wants them all), keyframes within this PC's budget.
    let mut data = dataset::load_with(&session, width, Budget { depth_samples: u64::MAX, ..Budget::default() }, |_| {})?;
    std::fs::create_dir_all(&out)?;
    let before: Vec<Marks> = data.depth_views.iter().map(|d| d.unknown.clone()).collect();
    let off = FixParams { min_frames: usize::MAX, max_disagreeing: f32::INFINITY, min_depth: 0.0, ..FixParams::default() };
    let rules = [
        ("never reads", [255u8, 40, 40], FixParams { min_frames: FixParams::default().min_frames, ..off.clone() }),
        ("misread", [255, 0, 255], FixParams { max_disagreeing: FixParams::default().max_disagreeing, ..off.clone() }),
        ("near", [0, 255, 255], FixParams { min_depth: FixParams::default().min_depth, ..off.clone() }),
    ];
    // Per rule, per depth frame, the samples it marks (beyond those unknown already).
    let mut marked: Vec<Vec<Vec<usize>>> = Vec::new();
    for (name, _, params) in &rules {
        let stats = depthfix::mark_untrusted(&mut data, params);
        println!("{name}: {} samples unknown with it ({} misread tiles, {} near samples, {} never-read tile positions)", stats.unknown_samples, stats.misread_tiles, stats.near_samples, stats.dead_tiles);
        marked.push(data.depth_views.iter().zip(&before).map(|(d, b)| (0..d.depth.len()).filter(|&i| d.unknown.contains(i) && !b.contains(i)).collect()).collect());
        for (d, b) in data.depth_views.iter_mut().zip(&before) {
            d.unknown = b.clone();
        }
    }
    let (w, h) = (data.width as usize, data.height as usize);
    let mut written = 0;
    for (k, d) in data.depth_views.iter().enumerate() {
        let Some(view) = data.views.iter().min_by(|a, b| (a.game_time - d.game_time).abs().total_cmp(&(b.game_time - d.game_time).abs())) else { continue };
        if view.keyframe < range.0 || view.keyframe >= range.1 {
            continue;
        }
        let mut rgb: Vec<u8> = view.image.iter().flat_map(|p| [0, 1, 2].map(|c| ((p >> (8 * c)) & 255) as u8)).collect();
        let mut dot = |i: usize, colour: [u8; 3]| {
            let [u, v] = d.grid.pixels[i];
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (x, y) = (u.round() as i64 + dx, v.round() as i64 + dy);
                    if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                        rgb[(y as usize * w + x as usize) * 3..][..3].copy_from_slice(&colour);
                    }
                }
            }
        };
        for i in (0..d.depth.len()).filter(|&i| before[k].contains(i) && !d.grid.unused.contains(i)) {
            dot(i, [255, 220, 0]);
        }
        for (r, (_, colour, _)) in rules.iter().enumerate() {
            for &i in &marked[r][k] {
                dot(i, *colour);
            }
        }
        let file = std::io::BufWriter::new(std::fs::File::create(out.join(format!("depth_{k:06}.png")))?);
        let mut enc = png::Encoder::new(file, data.width, data.height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&rgb)?;
        written += 1;
    }
    println!("{written} depth frames drawn into {}", out.display());
    Ok(())
}
