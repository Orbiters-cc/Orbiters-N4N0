//! Reconstruction experiments that never touch the session: trains with the given settings and writes the timings,
//! held-out metrics, a few held-out renders and the PLY to `--out`.
//!
//! ```text
//! cargo run --release --example lab -- <session> --out <dir> [--width N] [--iterations N] [--budget N] [--seed N]
//!     [--keyframes-every N] [--renders N] [--sums float|fixed|cas] [--profile] [--set key=value]...
//! ```

use anyhow::{bail, Context as _};
use orbiters_reconstruct::gpu::{DeviceOptions, Gpu, GradientSums};
use orbiters_reconstruct::{dataset, init, metrics, ply, priority, train};
use std::path::PathBuf;
use std::time::Instant;

struct Args {
    session: PathBuf,
    out: PathBuf,
    width: u32,
    keyframes_every: usize,
    renders: usize,
    loose: bool,
    device: DeviceOptions,
    settings: train::Settings,
    sets: Vec<String>,
}

fn args() -> anyhow::Result<Args> {
    let mut it = std::env::args().skip(1);
    let mut a = Args {
        session: PathBuf::new(),
        out: PathBuf::new(),
        width: 1920,
        keyframes_every: 1,
        renders: 3,
        loose: false,
        device: DeviceOptions::default(),
        settings: train::Settings::default(),
        sets: Vec::new(),
    };
    let (mut session, mut out) = (None, None);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().with_context(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            "--width" => a.width = value("--width")?.parse()?,
            "--iterations" => a.settings.iterations = value("--iterations")?.parse()?,
            "--budget" => a.settings.budget = value("--budget")?.parse()?,
            "--seed" => a.settings.seed = value("--seed")?.parse()?,
            "--keyframes-every" => a.keyframes_every = value("--keyframes-every")?.parse()?,
            "--renders" => a.renders = value("--renders")?.parse()?,
            "--profile" => a.device.profile = true,
            "--loose" => a.loose = true,
            "--sums" => {
                a.device.sums = Some(match value("--sums")?.as_str() {
                    "float" => GradientSums::Float,
                    "fixed" => GradientSums::FixedPoint,
                    "cas" => GradientSums::CompareAndSwap,
                    other => bail!("--sums {other}: float, fixed or cas"),
                })
            }
            "--set" => {
                let kv = value("--set")?;
                let (k, v) = kv.split_once('=').with_context(|| format!("--set {kv}: key=value"))?;
                a.settings.set(k, v)?;
                a.sets.push(kv);
            }
            _ if arg.starts_with("--") => bail!("unknown option {arg}"),
            _ => session = Some(PathBuf::from(arg)),
        }
    }
    a.session = session.context("usage: lab <session> --out <dir> ...")?;
    a.out = out.context("--out is required")?;
    Ok(a)
}

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
    let mut a = args()?;
    priority::govern();
    std::fs::create_dir_all(&a.out)?;
    let t0 = Instant::now();
    let mut data = dataset::load(&a.session, a.width, |_| {})?;
    let load_seconds = t0.elapsed().as_secs_f32();
    let all_views = data.views.len();
    // Fewer-views experiments: every n-th training keyframe plus the usual held-out ones (which keep their role).
    if a.keyframes_every > 1 {
        let h = a.settings.hold_out_every;
        let is_held = |i: usize| h > 1 && i % h == h / 2;
        let (mut views, mut held) = (Vec::new(), Vec::new());
        for (i, v) in std::mem::take(&mut data.views).into_iter().enumerate() {
            if is_held(i) || i % a.keyframes_every == 0 {
                if is_held(i) {
                    held.push(views.len());
                }
                views.push(v);
            }
        }
        data.views = views;
        a.settings.held_out = Some(held);
    }
    let t1 = Instant::now();
    let seeded = init::from_depth(&data, a.settings.budget * 2 / 3);
    let seed_seconds = t1.elapsed().as_secs_f32();
    let mut gpu = Gpu::with_options(a.device)?;
    gpu.exact_tiles = !a.loose;
    eprintln!("{}: {} keyframes ({} in the session), {} depth frames, {} seeds, load {load_seconds:.1} s, seed {seed_seconds:.1} s, {}", a.session.display(), data.views.len(), all_views, data.depth_views.len(), seeded.len(), gpu.adapter);
    let mut curve = Vec::new();
    let outcome = train::train(&data, &mut gpu, seeded, &a.settings, |p| {
        if let Some(psnr) = p.psnr {
            eprintln!("  it {:>6}/{} {:>8.1} s {:>8} splats {:>10} entries  held-out {psnr:.2} dB", p.iteration, p.iterations, p.seconds, p.gaussians, p.intersections);
            curve.push(serde_json::json!({ "iteration": p.iteration, "seconds": p.seconds, "psnr": psnr, "gaussians": p.gaussians, "intersections": p.intersections }));
        }
    })?;
    let mut sections = serde_json::Map::new();
    if let Some(p) = &gpu.profiler {
        let (span, steps) = p.span.get();
        let steps = steps.max(1) as f64;
        for (name, (ms, _)) in p.totals.borrow().iter() {
            sections.insert(name.to_string(), serde_json::json!(ms / steps));
        }
        sections.insert("span".into(), serde_json::json!(span / steps));
    }
    eprintln!("done: {:.1} s ({:.2} ms/step), {} splats, held-out {:.2} dB, train {:.2} dB, overflows {}", outcome.seconds, outcome.seconds as f64 * 1000.0 / outcome.iterations as f64, outcome.gaussians.len(), outcome.psnr_held_out.unwrap_or(0.0), outcome.psnr_train, gpu.overflows);
    ply::write(&a.out.join("splats.ply"), &outcome.gaussians)?;
    gpu.upload(&outcome.gaussians);
    let n = outcome.held_out.len();
    let picks: Vec<usize> = if n == 0 { Vec::new() } else { (0..a.renders.min(n)).map(|k| outcome.held_out[k * (n - 1) / (a.renders.min(n) - 1).max(1)]).collect() };
    let (mut sf, mut st, mut ss, mut per_view) = (0f64, 0f64, 0f64, Vec::new());
    for &i in &outcome.held_out {
        let img = train::render(&mut gpu, &data.views[i].camera, a.settings.background)?;
        let (f, t, s) = metrics::metrics(&img, &data.views[i].image, &data.mask, data.width as usize, data.height as usize);
        sf += f;
        st += t;
        ss += s;
        per_view.push(serde_json::json!({ "keyframe": data.views[i].keyframe, "psnr": f, "psnr_trimmed90": t, "ssim": s }));
        if picks.contains(&i) {
            write_png(&a.out.join(format!("render_{:06}.png", data.views[i].keyframe)), &img, data.width, data.height)?;
        }
    }
    let nh = n.max(1) as f64;
    eprintln!("held-out: PSNR {:.2} dB, trimmed-90% PSNR {:.2} dB, SSIM {:.4}", sf / nh, st / nh, ss / nh);
    let result = serde_json::json!({
        "session": a.session.display().to_string(),
        "sets": a.sets,
        "keyframes": data.views.len(),
        "iterations": outcome.iterations,
        "seconds": outcome.seconds,
        "load_seconds": load_seconds,
        "seed_seconds": seed_seconds,
        "gaussians": outcome.gaussians.len(),
        "psnr_held_out": outcome.psnr_held_out,
        "psnr_held_out_trimmed90": st / nh,
        "ssim_held_out": ss / nh,
        "psnr_train": outcome.psnr_train,
        "overflows": gpu.overflows,
        "gpu_ms_per_step": sections,
        "per_view": per_view,
        "curve": curve,
        "adapter": gpu.adapter,
    });
    std::fs::write(a.out.join("result.json"), serde_json::to_string_pretty(&result)?)?;
    Ok(())
}
