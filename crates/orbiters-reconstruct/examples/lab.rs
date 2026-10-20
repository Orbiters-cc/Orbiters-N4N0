//! Reconstruction experiments that never touch the session: trains with the given settings and writes the timings,
//! held-out PSNR and a few held-out renders to `--out`.
//!
//! ```text
//! cargo run --release --example lab -- <session> --out <dir> [--width N] [--iterations N] [--budget N]
//!     [--seed N] [--keyframes-every N] [--profile] [--set key=value]...
//! ```

use anyhow::{bail, Context as _};
use orbiters_reconstruct::{dataset, gpu, init, train};
use std::path::PathBuf;
use std::time::Instant;

struct Args {
    session: PathBuf,
    out: PathBuf,
    width: u32,
    profile: bool,
    keyframes_every: usize,
    renders: usize,
    settings: train::Settings,
    sets: Vec<(String, String)>,
}

fn args() -> anyhow::Result<Args> {
    let mut it = std::env::args().skip(1);
    let mut a = Args { session: PathBuf::new(), out: PathBuf::new(), width: 1920, profile: false, keyframes_every: 1, renders: 3, settings: train::Settings::default(), sets: Vec::new() };
    let mut session = None;
    let mut out = None;
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
            "--profile" => a.profile = true,
            "--set" => {
                let kv = value("--set")?;
                let (k, v) = kv.split_once('=').with_context(|| format!("--set {kv}: key=value"))?;
                a.sets.push((k.to_string(), v.to_string()));
            }
            _ if arg.starts_with("--") => bail!("unknown option {arg}"),
            _ => session = Some(PathBuf::from(arg)),
        }
    }
    a.session = session.context("usage: lab <session> --out <dir> ...")?;
    a.out = out.context("--out is required")?;
    for (k, v) in &a.sets {
        a.settings.set(k, v)?;
    }
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
    std::fs::create_dir_all(&a.out)?;
    let t0 = Instant::now();
    let mut data = dataset::load(&a.session, a.width, |_| {})?;
    let load_seconds = t0.elapsed().as_secs_f32();
    let all_views = data.views.len();
    // Fewer-views experiments: keep every n-th training keyframe but always the held-out ones, which keep their
    // indices' role (held out = index % hold_out_every == hold_out_every / 2 in the full set).
    if a.keyframes_every > 1 {
        let h = a.settings.hold_out_every;
        let keep: Vec<bool> = (0..all_views).map(|i| (h > 1 && i % h == h / 2) || (i % a.keyframes_every == 0)).collect();
        let held: Vec<usize> = (0..all_views).filter(|&i| h > 1 && i % h == h / 2).collect();
        let mut views = Vec::new();
        let mut held_out = Vec::new();
        for (i, v) in std::mem::take(&mut data.views).into_iter().enumerate() {
            if keep[i] {
                if held.contains(&i) {
                    held_out.push(views.len());
                }
                views.push(v);
            }
        }
        data.views = views;
        a.settings.held_out.replace(held_out);
    }
    let t1 = Instant::now();
    let seeded = init::from_depth(&data, a.settings.budget * 2 / 3);
    let seed_seconds = t1.elapsed().as_secs_f32();
    let mut gpu = if a.profile { gpu::Gpu::new_profiled()? } else { gpu::Gpu::new()? };
    eprintln!(
        "{}: {} keyframes ({} in the session), {} depth frames, {}x{}, {} seeds, load {load_seconds:.1} s, seed {seed_seconds:.1} s, {}",
        a.session.display(),
        data.views.len(),
        all_views,
        data.depth_views.len(),
        data.width,
        data.height,
        seeded.len(),
        gpu.adapter
    );
    let mut curve = Vec::new();
    let outcome = train::train(&data, &mut gpu, seeded, &a.settings, |p| {
        if let Some(psnr) = p.psnr {
            eprintln!("  it {:>6}/{} {:>8.1} s {:>8} splats {:>10} entries  held-out {psnr:.2} dB", p.iteration, p.iterations, p.seconds, p.gaussians, p.intersections);
            curve.push(serde_json::json!({ "iteration": p.iteration, "seconds": p.seconds, "psnr": psnr, "gaussians": p.gaussians, "intersections": p.intersections }));
        }
    })?;
    let host = gpu.host.borrow().clone();
    eprintln!("sort overflows (pipelined): {}", gpu.overflows);
    orbiters_reconstruct::ply::write(&a.out.join("splats.ply"), &outcome.gaussians)?;
    let mut sections = serde_json::Map::new();
    if let Some(p) = &gpu.profiler {
        let (span, steps) = p.span.get();
        let steps = steps.max(1) as f64;
        eprintln!("GPU per step ({steps} profiled steps): span {:.2} ms", span / steps);
        for (name, (ms, passes)) in p.totals.borrow().iter() {
            eprintln!("  {name:<18} {:>8.3} ms  ({:.1} passes)", ms / steps, *passes as f64 / steps);
            sections.insert(name.to_string(), serde_json::json!(ms / steps));
        }
        sections.insert("span".into(), serde_json::json!(span / steps));
    }
    let steps = host.steps.max(1) as f64;
    eprintln!(
        "done: {:.1} s ({:.2} ms/step), relocation {:.1} s, evaluation {:.1} s, host wait {:.2} ms/step, encode {:.3} ms/step; {} splats; held-out {:.2} dB, train {:.2} dB",
        outcome.seconds,
        outcome.seconds as f64 * 1000.0 / outcome.iterations as f64,
        outcome.relocation_seconds,
        outcome.evaluation_seconds,
        host.wait.as_secs_f64() * 1000.0 / steps,
        host.encode.as_secs_f64() * 1000.0 / steps,
        outcome.gaussians.len(),
        outcome.psnr_held_out.unwrap_or(0.0),
        outcome.psnr_train
    );
    // A few held-out renders (first, middle, last) for visual checks.
    let picks: Vec<usize> = if outcome.held_out.is_empty() {
        vec![0]
    } else {
        let n = outcome.held_out.len();
        let mut v: Vec<usize> = (0..a.renders.min(n)).map(|k| outcome.held_out[if a.renders <= 1 { 0 } else { k * (n - 1) / (a.renders - 1).max(1) }]).collect();
        v.dedup();
        v
    };
    gpu.upload(&outcome.gaussians);
    let mut per_view = Vec::new();
    let (mut sum_full, mut sum_trim, mut sum_ssim) = (0f64, 0f64, 0f64);
    for &i in &outcome.held_out {
        let img = train::render(&mut gpu, &data.views[i].camera, a.settings.background)?;
        let (full, trimmed, ssim) = orbiters_reconstruct::metrics::metrics(&img, &data.views[i].image, &data.mask, data.width as usize, data.height as usize);
        sum_full += full;
        sum_trim += trimmed;
        sum_ssim += ssim;
        per_view.push(serde_json::json!({ "keyframe": data.views[i].keyframe, "psnr": full, "psnr_trimmed90": trimmed, "ssim": ssim }));
        if picks.contains(&i) {
            write_png(&a.out.join(format!("render_{:06}.png", data.views[i].keyframe)), &img, data.width, data.height)?;
            if a.settings.robust > 0.0 {
                // What a robust step would leave out of this view now (no update).
                gpu.robust = a.settings.robust;
                let px = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
                gpu.step(&data.views[i].camera, a.settings.background, gpu::Target::Color { rgba: &data.views[i].image, scene_pixels: px, weight: 1.0 }, None)?;
                let kept = gpu.read_kept();
                let vis: Vec<[f32; 4]> = kept.iter().zip(&data.mask).map(|(k, m)| if *m < 0.5 { [0.5, 0.5, 0.5, 0.0] } else { [*k, *k, *k, 0.0] }).collect();
                write_png(&a.out.join(format!("kept_{:06}.png", data.views[i].keyframe)), &vis, data.width, data.height)?;
            }
        }
    }
    let nh = outcome.held_out.len().max(1) as f64;
    eprintln!("held-out: PSNR {:.2} dB, trimmed-90% PSNR {:.2} dB, SSIM {:.4}", sum_full / nh, sum_trim / nh, sum_ssim / nh);
    let result = serde_json::json!({
        "session": a.session.display().to_string(),
        "sets": a.sets.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>(),
        "width": data.width,
        "keyframes": data.views.len(),
        "keyframes_in_session": all_views,
        "iterations": outcome.iterations,
        "budget": a.settings.budget,
        "seconds": outcome.seconds,
        "load_seconds": load_seconds,
        "seed_seconds": seed_seconds,
        "relocation_seconds": outcome.relocation_seconds,
        "evaluation_seconds": outcome.evaluation_seconds,
        "host_wait_ms_per_step": host.wait.as_secs_f64() * 1000.0 / steps,
        "host_encode_ms_per_step": host.encode.as_secs_f64() * 1000.0 / steps,
        "gpu_ms_per_step": sections,
        "gaussians": outcome.gaussians.len(),
        "psnr_held_out": outcome.psnr_held_out,
        "psnr_held_out_trimmed90": sum_trim / nh,
        "ssim_held_out": sum_ssim / nh,
        "per_view": per_view,
        "psnr_train": outcome.psnr_train,
        "held_out_keyframes": outcome.held_out.iter().map(|&i| data.views[i].keyframe).collect::<Vec<_>>(),
        "curve": curve,
        "adapter": gpu.adapter,
    });
    std::fs::write(a.out.join("result.json"), serde_json::to_string_pretty(&result)?)?;
    Ok(())
}
