//! Reconstruction worker of Orbiters desktop:
//!
//! ```text
//! orbiters-reconstruct <session folder> [--width N] [--iterations N] [--budget N] [--robust T]
//! ```
//!
//! Prints one [`Event`] per line on stdout for the app and writes the session's `reconstruction/` folder: the splats
//! (PLY), the report, a preview and a render of every held-out keyframe. The folder is replaced only once everything
//! is written.

use anyhow::{bail, Context as _};
use orbiters_contracts::qoi;
use orbiters_core::reconstruction::{self as rec, Event, Report, Stage};
use orbiters_reconstruct::{dataset, gpu, init, ply, train};
use std::io::Write as _;
use std::path::{Path, PathBuf};

fn emit(e: &Event) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", e.to_line());
    let _ = out.flush();
}

fn stage(stage: Stage, progress: f32) {
    emit(&Event::Stage { stage, progress });
}

struct Args {
    session: PathBuf,
    width: u32,
    settings: train::Settings,
}

fn args() -> anyhow::Result<Args> {
    let mut it = std::env::args().skip(1);
    let mut session = None;
    let mut width = 1920;
    let mut settings = train::Settings::default();
    while let Some(a) = it.next() {
        let mut number = |name: &str| -> anyhow::Result<u64> { it.next().with_context(|| format!("{name} needs a value"))?.parse().with_context(|| format!("{name} takes a number")) };
        match a.as_str() {
            "--width" => width = number("--width")? as u32,
            "--iterations" => settings.iterations = number("--iterations")? as u32,
            "--budget" => settings.budget = number("--budget")? as usize,
            "--robust" => settings.robust = it.next().context("--robust needs a value")?.parse().context("--robust takes a number (0–1)")?,
            _ if a.starts_with("--") => bail!("unknown option {a}"),
            _ => session = Some(PathBuf::from(a)),
        }
    }
    Ok(Args { session: session.context("usage: orbiters-reconstruct <session folder> [--width N] [--iterations N] [--budget N] [--robust T]")?, width, settings })
}

fn rgb8(img: &[[f32; 4]]) -> Vec<u8> {
    img.iter().flat_map(|p| [0, 1, 2].map(|c| (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect()
}

fn run(args: Args) -> anyhow::Result<Report> {
    let dir = &args.session;
    stage(Stage::Loading, 0.0);
    let data = dataset::load(dir, args.width, |p| stage(Stage::Loading, p))?;
    stage(Stage::Seeding, 0.0);
    let seeded = init::from_depth(&data, args.settings.budget * 2 / 3);
    if seeded.is_empty() {
        bail!("This session has no depth frames to start from: record with the capture kit's depth frames on.");
    }
    let mut gpu = gpu::Gpu::new()?;
    stage(Stage::Training, 0.0);
    let outcome = train::train(&data, &mut gpu, seeded, &args.settings, |p| {
        emit(&Event::Progress { iteration: p.iteration, iterations: p.iterations, gaussians: p.gaussians, intersections: p.intersections, psnr: p.psnr, seconds: p.seconds });
    })?;

    stage(Stage::Saving, 0.0);
    let partial = dir.join(rec::PARTIAL_DIR);
    if partial.exists() {
        std::fs::remove_dir_all(&partial).with_context(|| format!("clearing {}", partial.display()))?;
    }
    std::fs::create_dir_all(partial.join(rec::HELD_OUT_DIR))?;
    ply::write(&partial.join(rec::PLY_FILE), &outcome.gaussians)?;
    gpu.upload(&outcome.gaussians);
    let previews: Vec<usize> = if outcome.held_out.is_empty() { vec![0] } else { outcome.held_out.clone() };
    for (k, &i) in previews.iter().enumerate() {
        let view = &data.views[i];
        let img = train::render(&mut gpu, &view.camera, args.settings.background)?;
        let encoded = qoi::encode(&rgb8(&img), data.width, data.height, 3);
        if k == 0 {
            std::fs::write(partial.join(rec::PREVIEW_FILE), &encoded)?;
        }
        std::fs::write(partial.join(rec::HELD_OUT_DIR).join(format!("{:06}.qoi", view.keyframe)), &encoded)?;
    }
    let report = Report {
        gaussians: outcome.gaussians.len(),
        iterations: outcome.iterations,
        seconds: outcome.seconds,
        psnr_held_out: outcome.psnr_held_out,
        psnr_train: outcome.psnr_train,
        held_out_keyframes: outcome.held_out.iter().map(|&i| data.views[i].keyframe).collect(),
        width: data.width,
        height: data.height,
        adapter: gpu.adapter.clone(),
        created_utc: orbiters_core::now_utc_iso(),
    };
    std::fs::write(partial.join(rec::REPORT_FILE), serde_json::to_string_pretty(&report)?)?;
    replace(dir, &partial)?;
    Ok(report)
}

fn replace(dir: &Path, partial: &Path) -> anyhow::Result<()> {
    let out = rec::dir(dir);
    if out.exists() {
        std::fs::remove_dir_all(&out).with_context(|| format!("removing the previous {}", out.display()))?;
    }
    std::fs::rename(partial, &out).with_context(|| format!("moving the result to {}", out.display()))
}

/// `orbiters-reconstruct --web-preview <splats.ply> <out.splat> [--max N]`: the reconstruction as a web preview
/// (orbiters_reconstruct::web), started from the scene's centre.
fn web_preview(args: &[String]) -> anyhow::Result<()> {
    let (Some(ply_path), Some(out)) = (args.first(), args.get(1)) else { bail!("usage: orbiters-reconstruct --web-preview <splats.ply> <out.splat> [--max N]") };
    let max = match args.get(2).map(String::as_str) {
        Some("--max") => args.get(3).context("--max N")?.parse()?,
        _ => orbiters_reconstruct::web::DEFAULT_MAX,
    };
    let g = ply::read(std::path::Path::new(ply_path))?;
    let n = g.len().max(1) as f32;
    let centre = g.means.iter().fold([0.0f32; 3], |a, m| [a[0] + m[0] / n, a[1] + m[1] / n, a[2] + m[2] / n]);
    let web = orbiters_reconstruct::web::splat_file(&g, max, centre);
    std::fs::write(out, &web.bytes)?;
    println!("{} of {} splats, {:.1} MB", web.count, g.len(), web.bytes.len() as f64 / 1e6);
    Ok(())
}

fn main() {
    let all: Vec<String> = std::env::args().skip(1).collect();
    if all.first().map(String::as_str) == Some("--web-preview") {
        if let Err(e) = web_preview(&all[1..]) {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
        return;
    }
    match args().and_then(run) {
        Ok(report) => emit(&Event::Done { report }),
        Err(e) => {
            emit(&Event::Error { message: format!("{e:#}") });
            std::process::exit(1);
        }
    }
}
