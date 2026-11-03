//! Held-out metrics of a splat PLY on a session (the app's result or a lab run's): PSNR, PSNR without the worst
//! tenth of pixels, SSIM, per held-out keyframe (the trainer's choice: index % 8 == 4), on the session as the worker
//! prepares it (orbiters_reconstruct::prepare: the memory budget, depth validation, the aberration undone).
//!
//! ```text
//! cargo run --release --example eval -- <session> <splats.ply> [--width N] [--json <out.json>]
//!     [--out <dir>] [--kept <threshold>] [--ca auto|off]
//! ```
//! `--out` writes every held-out render; with `--kept`, also the pixels a robust colour step would keep (white) or
//! leave out (black; strip and band grey), so models can be compared on the pixels both keep. With the world's
//! chromatic aberration measured (`--ca auto`, the default), renders pass through it and score against the keyframes
//! as captured, as the worker scores them; `--ca off` scores plain renders against the keyframes as captured.

use anyhow::Context as _;
use orbiters_reconstruct::{aberration, dataset, gpu, metrics, ply, prepare, train};
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
    let (mut session, mut splats, mut width, mut json, mut out, mut kept, mut ca) = (None, None, 1920u32, None, None, 0f32, true);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--width" => width = it.next().context("--width")?.parse()?,
            "--json" => json = Some(PathBuf::from(it.next().context("--json")?)),
            "--out" => out = Some(PathBuf::from(it.next().context("--out")?)),
            "--kept" => kept = it.next().context("--kept")?.parse()?,
            "--ca" => {
                ca = match it.next().as_deref() {
                    Some("auto") => true,
                    Some("off") => false,
                    _ => anyhow::bail!("--ca takes auto or off"),
                }
            }
            _ if session.is_none() => session = Some(PathBuf::from(a)),
            _ => splats = Some(PathBuf::from(a)),
        }
    }
    let session = session.context("usage: eval <session> <splats.ply>")?;
    let prepared = prepare::prepare(&session, &prepare::Options { width, aberration: ca, ..Default::default() }, |_| {})?;
    for line in prepared.describe() {
        println!("{line}");
    }
    let data = &prepared.data;
    let (w, h) = (data.width as usize, data.height as usize);
    let splats = splats.context("usage: eval <session> <splats.ply>")?;
    let g = ply::read(&splats)?;
    let mut gpu = gpu::Gpu::new()?;
    gpu.upload(&g);
    // The sky map beside the PLY (orbiters_reconstruct::sky::SkyMap), drawn behind the splats as every viewer does.
    let map = splats.with_file_name(orbiters_core::reconstruction::SKY_FILE);
    if map.is_file() {
        let sky = orbiters_reconstruct::sky::SkyMap::read(&map)?;
        gpu.set_sky(Some((sky.width, &sky.rgb)))?;
    }
    gpu.set_mask(data.width, data.height, &data.mask)?;
    if let Some(dir) = &out {
        std::fs::create_dir_all(dir)?;
    }
    // The keyframes the run held out, from the report beside the splats (else the session's). Picked here, they would
    // follow the views loaded, which follow this PC's memory and the width: keyframes the run trained on, maybe.
    use orbiters_core::reconstruction;
    let report = splats
        .parent()
        .and_then(|dir| std::fs::read_to_string(dir.join(reconstruction::REPORT_FILE)).ok())
        .and_then(|text| serde_json::from_str::<reconstruction::Report>(&text).ok())
        .or_else(|| reconstruction::read_report(&session));
    let reported = report.map(|r| r.held_out_keyframes).unwrap_or_default();
    let held: Vec<usize> = if reported.is_empty() {
        train::held_out_views(data, train::Settings::default().hold_out_every)
    } else {
        let found: Vec<usize> = reported.iter().filter_map(|&k| data.views.iter().position(|v| v.keyframe == k)).collect();
        if found.len() < reported.len() {
            eprintln!("{} of the run's {} held-out keyframes are not loaded here (this PC's memory budget)", reported.len() - found.len(), reported.len());
        }
        found
    };
    anyhow::ensure!(!held.is_empty(), "no held-out keyframe to score here");
    let (mut sf, mut st, mut ss) = (0f64, 0f64, 0f64);
    let mut rows = Vec::new();
    let scene_pixels = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    for &i in &held {
        let img = train::render(&mut gpu, &data.views[i].camera, [0.0; 3])?;
        // Scored as the worker scores: through the world's aberration against the keyframe as captured, if any.
        let (f, t, s) = match &prepared.undo {
            Some(u) => {
                let captured = dataset::keyframe_image(&data.dir, data.views[i].keyframe, &data.geometry)?;
                metrics::metrics(&aberration::aberrate(&img, w, h, u.intensity, u.factor), &captured, &u.mask, w, h)
            }
            None => metrics::metrics(&img, &data.views[i].image, &data.mask, w, h),
        };
        println!("  keyframe {:>4}: PSNR {f:.2} dB, trimmed-90% {t:.2} dB, SSIM {s:.4}", data.views[i].keyframe);
        rows.push(serde_json::json!({ "keyframe": data.views[i].keyframe, "psnr": f, "psnr_trimmed90": t, "ssim": s }));
        sf += f;
        st += t;
        ss += s;
        if let Some(dir) = &out {
            write_png(&dir.join(format!("render_{:06}.png", data.views[i].keyframe)), &img, data.width, data.height)?;
            if kept > 0.0 {
                gpu.step(&data.views[i].camera, [0.0; 3], gpu::Target::Color { rgba: &data.views[i].image, scene_pixels, weight: 1.0, ssim: 0.0, robust: kept }, None)?;
                let k = gpu.read_kept()?;
                let vis: Vec<[f32; 4]> = k.iter().zip(&data.mask).map(|(k, m)| if *m < 0.5 { [0.5, 0.5, 0.5, 0.0] } else { [*k, *k, *k, 0.0] }).collect();
                write_png(&dir.join(format!("kept_{:06}.png", data.views[i].keyframe)), &vis, data.width, data.height)?;
            }
        }
    }
    let n = held.len().max(1) as f64;
    println!("{} splats, {} held-out keyframes: PSNR {:.2} dB, trimmed-90% PSNR {:.2} dB, SSIM {:.4}", g.len(), held.len(), sf / n, st / n, ss / n);
    if let Some(path) = json {
        let aberration = prepared.undo.as_ref().map(|u| u.intensity);
        std::fs::write(path, serde_json::to_string_pretty(&serde_json::json!({ "splats": g.len(), "psnr": sf / n, "psnr_trimmed90": st / n, "ssim": ss / n, "chromatic_aberration": aberration, "per_view": rows }))?)?;
    }
    Ok(())
}
