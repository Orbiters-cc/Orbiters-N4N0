//! Held-out metrics of a splat PLY on a session (the app's result or a lab run's): PSNR, PSNR without the worst
//! tenth of pixels, SSIM, per held-out keyframe (the trainer's choice: index % 8 == 4).
//!
//! ```text
//! cargo run --release --example eval -- <session> <splats.ply> [--width N] [--json <out.json>]
//!     [--out <dir>] [--kept <threshold>]
//! ```
//! `--out` writes every held-out render; with `--kept`, also the pixels a robust colour step would keep (white) or
//! leave out (black; strip and band grey), so models can be compared on the pixels both keep.

use anyhow::Context as _;
use orbiters_reconstruct::{dataset, gpu, metrics, ply, train};
use std::path::PathBuf;

fn write_png(path: &std::path::Path, img: &[[f32; 4]], w: u32, h: u32) -> anyhow::Result<()> {
    let rgb: Vec<u8> = img.iter().flat_map(|p| [0, 1, 2].map(|c| (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgb)?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut session, mut splats, mut width, mut json, mut out, mut kept) = (None, None, 1920u32, None, None, 0f32);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--width" => width = it.next().context("--width")?.parse()?,
            "--json" => json = Some(PathBuf::from(it.next().context("--json")?)),
            "--out" => out = Some(PathBuf::from(it.next().context("--out")?)),
            "--kept" => kept = it.next().context("--kept")?.parse()?,
            _ if session.is_none() => session = Some(PathBuf::from(a)),
            _ => splats = Some(PathBuf::from(a)),
        }
    }
    let session = session.context("usage: eval <session> <splats.ply>")?;
    let data = dataset::load(&session, width, |_| {})?;
    let g = ply::read(&splats.context("usage: eval <session> <splats.ply>")?)?;
    let mut gpu = gpu::Gpu::new()?;
    gpu.upload(&g);
    gpu.set_mask(data.width, data.height, &data.mask);
    if let Some(dir) = &out {
        std::fs::create_dir_all(dir)?;
    }
    let h = train::Settings::default().hold_out_every;
    let held: Vec<usize> = (0..data.views.len()).filter(|i| i % h == h / 2).collect();
    let (mut sf, mut st, mut ss) = (0f64, 0f64, 0f64);
    let mut rows = Vec::new();
    let scene_pixels = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    for &i in &held {
        let img = train::render(&mut gpu, &data.views[i].camera, [0.0; 3])?;
        let (f, t, s) = metrics::metrics(&img, &data.views[i].image, &data.mask, data.width as usize, data.height as usize);
        println!("  keyframe {:>4}: PSNR {f:.2} dB, trimmed-90% {t:.2} dB, SSIM {s:.4}", data.views[i].keyframe);
        rows.push(serde_json::json!({ "keyframe": data.views[i].keyframe, "psnr": f, "psnr_trimmed90": t, "ssim": s }));
        sf += f;
        st += t;
        ss += s;
        if let Some(dir) = &out {
            write_png(&dir.join(format!("render_{:06}.png", data.views[i].keyframe)), &img, data.width, data.height)?;
            if kept > 0.0 {
                gpu.step(&data.views[i].camera, [0.0; 3], gpu::Target::Color { rgba: &data.views[i].image, scene_pixels, weight: 1.0, ssim: 0.0, robust: kept }, None)?;
                let k = gpu.read_kept();
                let vis: Vec<[f32; 4]> = k.iter().zip(&data.mask).map(|(k, m)| if *m < 0.5 { [0.5, 0.5, 0.5, 0.0] } else { [*k, *k, *k, 0.0] }).collect();
                write_png(&dir.join(format!("kept_{:06}.png", data.views[i].keyframe)), &vis, data.width, data.height)?;
            }
        }
    }
    let n = held.len().max(1) as f64;
    println!("{} splats, {} held-out keyframes: PSNR {:.2} dB, trimmed-90% PSNR {:.2} dB, SSIM {:.4}", g.len(), held.len(), sf / n, st / n, ss / n);
    if let Some(path) = json {
        std::fs::write(path, serde_json::to_string_pretty(&serde_json::json!({ "splats": g.len(), "psnr": sf / n, "psnr_trimmed90": st / n, "ssim": ss / n, "per_view": rows }))?)?;
    }
    Ok(())
}
