//! Held-out metrics of a splat PLY on a session (the app's result or a lab run's): PSNR, PSNR without the worst
//! tenth of pixels, SSIM, per held-out keyframe (the trainer's choice: index % 8 == 4).
//!
//! ```text
//! cargo run --release --example eval -- <session> <splats.ply> [--width N] [--json <out.json>]
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::{dataset, gpu, metrics, ply, train};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut session, mut splats, mut width, mut json) = (None, None, 1920u32, None);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--width" => width = it.next().context("--width")?.parse()?,
            "--json" => json = Some(PathBuf::from(it.next().context("--json")?)),
            _ if session.is_none() => session = Some(PathBuf::from(a)),
            _ => splats = Some(PathBuf::from(a)),
        }
    }
    let session = session.context("usage: eval <session> <splats.ply>")?;
    let data = dataset::load(&session, width, |_| {})?;
    let g = ply::read(&splats.context("usage: eval <session> <splats.ply>")?)?;
    let mut gpu = gpu::Gpu::new()?;
    gpu.upload(&g);
    let h = train::Settings::default().hold_out_every;
    let held: Vec<usize> = (0..data.views.len()).filter(|i| i % h == h / 2).collect();
    let (mut sf, mut st, mut ss) = (0f64, 0f64, 0f64);
    let mut rows = Vec::new();
    for &i in &held {
        let img = train::render(&mut gpu, &data.views[i].camera, [0.0; 3])?;
        let (f, t, s) = metrics::metrics(&img, &data.views[i].image, &data.mask, data.width as usize, data.height as usize);
        println!("  keyframe {:>4}: PSNR {f:.2} dB, trimmed-90% {t:.2} dB, SSIM {s:.4}", data.views[i].keyframe);
        rows.push(serde_json::json!({ "keyframe": data.views[i].keyframe, "psnr": f, "psnr_trimmed90": t, "ssim": s }));
        sf += f;
        st += t;
        ss += s;
    }
    let n = held.len().max(1) as f64;
    println!("{} splats, {} held-out keyframes: PSNR {:.2} dB, trimmed-90% PSNR {:.2} dB, SSIM {:.4}", g.len(), held.len(), sf / n, st / n, ss / n);
    if let Some(path) = json {
        std::fs::write(path, serde_json::to_string_pretty(&serde_json::json!({ "splats": g.len(), "psnr": sf / n, "psnr_trimmed90": st / n, "ssim": ss / n, "per_view": rows }))?)?;
    }
    Ok(())
}
