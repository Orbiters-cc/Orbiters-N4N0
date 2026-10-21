//! Reconstruction experiments that never touch the session: trains with the given settings and writes the timings,
//! held-out metrics, a few held-out renders and the PLY to `--out`.
//!
//! ```text
//! cargo run --release --example lab -- <session> --out <dir> [--width N] [--iterations N] [--budget N] [--seed N]
//!     [--keyframes-every N] [--renders N] [--sums float|fixed|cas] [--profile] [--loose] [--transients] [--key-bits 24|32]
//!     [--sky N] [--sky-lr X]
//!     [--set key=value]...
//! ```
//!
//! Every run detects transients (orbiters_reconstruct::transients) to score the held-out views on their static pixels
//! too; `--transients` also trains without them (keyframe pixels left out, depth samples neither seeds nor targets),
//! `--transients-depth` without the depth transients only (no colour outliers or unstable surfaces).

use anyhow::{bail, Context as _};
use orbiters_reconstruct::gpu::{DeviceOptions, Gpu, GradientSums};
use orbiters_reconstruct::{dataset, init, metrics, ply, priority, sky, train, transients};
use std::path::PathBuf;
use std::time::Instant;

struct Args {
    session: PathBuf,
    out: PathBuf,
    width: u32,
    keyframes_every: usize,
    select: usize,
    select_coverage: bool,
    renders: usize,
    loose: bool,
    transients: bool,
    transients_depth: bool,
    key_bits: u32,
    sky: u32,
    sky_lr: f32,
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
        select: 0,
        select_coverage: false,
        renders: 3,
        loose: false,
        transients: false,
        transients_depth: false,
        key_bits: 32,
        sky: 0,
        sky_lr: 0.01,
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
            "--select" => a.select = value("--select")?.parse()?,
            "--select-mode" => {
                a.select_coverage = match value("--select-mode")?.as_str() {
                    "uniform" => false,
                    "coverage" => true,
                    other => bail!("--select-mode {other}: uniform or coverage"),
                }
            }
            "--renders" => a.renders = value("--renders")?.parse()?,
            "--profile" => a.device.profile = true,
            "--loose" => a.loose = true,
            "--transients" => a.transients = true,
            "--transients-depth" => a.transients_depth = true,
            "--key-bits" => a.key_bits = value("--key-bits")?.parse()?,
            "--sky" => a.sky = value("--sky")?.parse()?,
            "--sky-lr" => a.sky_lr = value("--sky-lr")?.parse()?,
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

/// Per keyframe, the pixels to leave out (1): depth transients, colour outliers and unstable surfaces; and the depth
/// transients alone.
#[allow(clippy::type_complexity)]
fn leave_out(data: &dataset::Dataset) -> (Vec<Vec<bool>>, Vec<Vec<f32>>, Vec<Vec<f32>>, serde_json::Value) {
    let t = Instant::now();
    let flags = transients::detect(data, &transients::Params::default());
    let depth_masks = transients::keyframe_masks(data, &flags, 0.06, 1.5);
    let colors = transients::color_transients(data, &transients::ColorParams::default());
    let (w, h) = (data.width, data.height);
    let masks: Vec<Vec<f32>> = (0..data.views.len())
        .map(|i| {
            let mut m = transients::disc_mask(&colors.outliers[i], w, h);
            let unstable = transients::disc_mask(&colors.unstable[i], w, h);
            for (k, v) in m.iter_mut().enumerate() {
                let depth = depth_masks[i].as_ref().is_some_and(|d| d[k] > 0.5);
                if depth || unstable[k] > 0.5 {
                    *v = 1.0;
                }
            }
            m
        })
        .collect();
    let depth_only: Vec<Vec<f32>> = depth_masks.into_iter().map(|m| m.unwrap_or_else(|| vec![0.0; (w * h) as usize])).collect();
    let scene = data.mask.iter().filter(|m| **m > 0.5).count().max(1) as f64;
    let share = masks.iter().map(|m| m.iter().zip(&data.mask).filter(|(t, s)| **t > 0.5 && **s > 0.5).count() as f64 / scene).sum::<f64>() / masks.len().max(1) as f64;
    let samples: usize = flags.iter().map(|f| f.iter().filter(|x| **x).count()).sum();
    eprintln!("transients {:.1} s: {samples} dynamic depth samples, {:.2} % of scene pixels left out per keyframe", t.elapsed().as_secs_f32(), 100.0 * share);
    (flags, masks, depth_only, serde_json::json!({ "seconds": t.elapsed().as_secs_f32(), "dynamic_samples": samples, "left_out_share": share }))
}

fn main() -> anyhow::Result<()> {
    let mut a = args()?;
    priority::govern();
    std::fs::create_dir_all(&a.out)?;
    let t0 = Instant::now();
    let mut data = dataset::load(&a.session, a.width, |_| {})?;
    let load_seconds = t0.elapsed().as_secs_f32();
    let all_views = data.views.len();
    let (flags, mut left_out, depth_only, transient_stats) = leave_out(&data);
    if a.transients || a.transients_depth {
        let train_masks = if a.transients { &left_out } else { &depth_only };
        for (v, m) in data.views.iter_mut().zip(train_masks) {
            for (p, t) in v.image.iter_mut().zip(m) {
                if *t > 0.5 {
                    *p &= 0x00ff_ffff;
                }
            }
        }
        for (d, f) in data.depth_views.iter_mut().zip(flags) {
            d.dynamic = f;
        }
    }
    // Fewer-views experiments: every n-th training keyframe, or `select` of them (spread evenly, or most new coverage
    // first), plus the usual held-out ones (which keep their role).
    let mut selection = serde_json::Value::Null;
    if a.keyframes_every > 1 || a.select > 0 {
        let h = a.settings.hold_out_every;
        let is_held = |i: usize| h > 1 && i % h == h / 2;
        let candidates: Vec<usize> = (0..data.views.len()).filter(|&i| !is_held(i)).collect();
        let chosen: Vec<usize> = if a.select > 0 && a.select_coverage {
            orbiters_reconstruct::coverage::select(&data, &candidates, a.select)
        } else if a.select > 0 {
            let k = a.select.min(candidates.len());
            (0..k).map(|j| candidates[j * candidates.len() / k]).collect()
        } else {
            candidates.iter().copied().filter(|i| i % a.keyframes_every == 0).collect()
        };
        selection = serde_json::json!(chosen.iter().map(|&i| data.views[i].keyframe).collect::<Vec<_>>());
        let (mut views, mut masks, mut held) = (Vec::new(), Vec::new(), Vec::new());
        for (i, (v, m)) in std::mem::take(&mut data.views).into_iter().zip(std::mem::take(&mut left_out)).enumerate() {
            if is_held(i) || chosen.contains(&i) {
                if is_held(i) {
                    held.push(views.len());
                }
                views.push(v);
                masks.push(m);
            }
        }
        data.views = views;
        left_out = masks;
        a.settings.held_out = Some(held);
    }
    let t1 = Instant::now();
    let mut seeded = init::from_depth(&data, a.settings.budget * 2 / 3);
    // With a sky cubemap, the far shell of discs goes.
    let sky = (a.sky > 0).then(|| sky::seed(&data, a.sky as usize));
    if sky.is_some() {
        let n = data.views.len() as f32;
        let c = data.views.iter().map(|v| v.camera.position()).fold([0f32; 3], |s, p| [s[0] + p[0] / n, s[1] + p[1] / n, s[2] + p[2] / n]);
        let keep: Vec<bool> = seeded.means.iter().map(|m| (0..3).map(|k| (m[k] - c[k]).powi(2)).sum::<f32>().sqrt() < init::SKY * 0.5).collect();
        seeded.retain(&keep);
    }
    let seed_seconds = t1.elapsed().as_secs_f32();
    let mut gpu = Gpu::with_options(a.device)?;
    gpu.exact_tiles = !a.loose;
    gpu.key_bits = a.key_bits;
    gpu.sky_lr = a.sky_lr;
    gpu.set_sky(sky.as_deref().map(|t| (a.sky, t)))?;
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
    eprintln!(
        "done: {:.1} s ({:.2} ms/step; relocation {:.1} s, evaluation {:.1} s), {} splats, held-out {:.2} dB, train {:.2} dB, overflows {}",
        outcome.seconds,
        outcome.seconds as f64 * 1000.0 / outcome.iterations as f64,
        outcome.relocate_seconds,
        outcome.evaluate_seconds,
        outcome.gaussians.len(),
        outcome.psnr_held_out.unwrap_or(0.0),
        outcome.psnr_train,
        gpu.overflows
    );
    eprintln!("inside steps {:.1} s, of which waiting for the GPU {:.1} s", gpu.step_seconds, gpu.wait_seconds);
    let (step_seconds, wait_seconds) = (gpu.step_seconds, gpu.wait_seconds);
    ply::write(&a.out.join("splats.ply"), &outcome.gaussians)?;
    if let Some((n, texels)) = gpu.read_sky() {
        // The six faces side by side.
        let n = n as usize;
        let mut img = vec![[0f32; 4]; 6 * n * n];
        for f in 0..6 {
            for y in 0..n {
                for x in 0..n {
                    img[y * 6 * n + f * n + x] = texels[f * n * n + y * n + x];
                }
            }
        }
        write_png(&a.out.join("sky.png"), &img, 6 * n as u32, n as u32)?;
    }
    gpu.upload(&outcome.gaussians);
    let n = outcome.held_out.len();
    let picks: Vec<usize> = if n == 0 { Vec::new() } else { (0..a.renders.min(n)).map(|k| outcome.held_out[k * (n - 1) / (a.renders.min(n) - 1).max(1)]).collect() };
    let (mut sf, mut st, mut ss, mut sp, mut per_view) = (0f64, 0f64, 0f64, 0f64, Vec::new());
    let (w, h) = (data.width as usize, data.height as usize);
    for &i in &outcome.held_out {
        let img = train::render(&mut gpu, &data.views[i].camera, a.settings.background)?;
        let (f, t, s) = metrics::metrics(&img, &data.views[i].image, &data.mask, w, h);
        // Static pixels: the scene less this keyframe's transients (the same pixels for every run).
        let still: Vec<f32> = data.mask.iter().zip(&left_out[i]).map(|(m, t)| if *t > 0.5 { 0.0 } else { *m }).collect();
        let (fs, _, _) = metrics::metrics(&img, &data.views[i].image, &still, w, h);
        sf += f;
        st += t;
        ss += s;
        sp += fs;
        per_view.push(serde_json::json!({ "keyframe": data.views[i].keyframe, "psnr": f, "psnr_trimmed90": t, "ssim": s, "psnr_static": fs }));
        if picks.contains(&i) {
            write_png(&a.out.join(format!("render_{:06}.png", data.views[i].keyframe)), &img, data.width, data.height)?;
        }
    }
    let nh = n.max(1) as f64;
    eprintln!("held-out: PSNR {:.2} dB, trimmed-90% PSNR {:.2} dB, SSIM {:.4}, static-pixel PSNR {:.2} dB", sf / nh, st / nh, ss / nh, sp / nh);
    let result = serde_json::json!({
        "session": a.session.display().to_string(),
        "sets": a.sets,
        "transients": a.transients,
        "transients_depth": a.transients_depth,
        "key_bits": a.key_bits,
        "sky": a.sky,
        "sky_lr": a.sky_lr,
        "transient_detection": transient_stats,
        "keyframes": data.views.len(),
        "selected_keyframes": selection,
        "iterations": outcome.iterations,
        "seconds": outcome.seconds,
        "relocate_seconds": outcome.relocate_seconds,
        "evaluate_seconds": outcome.evaluate_seconds,
        "step_seconds": step_seconds,
        "gpu_wait_seconds": wait_seconds,
        "load_seconds": load_seconds,
        "seed_seconds": seed_seconds,
        "gaussians": outcome.gaussians.len(),
        "psnr_held_out": outcome.psnr_held_out,
        "psnr_held_out_trimmed90": st / nh,
        "ssim_held_out": ss / nh,
        "psnr_held_out_static": sp / nh,
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
