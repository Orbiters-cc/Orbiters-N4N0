//! Lab (gpu-stacks experiment): dumps one real colour step's rasteriser buffers (crate::gpu::Gpu::lab_raster_dump)
//! for ports of the forward and backward kernels to other GPU languages to be checked against and timed on.
//!
//! ```text
//! cargo run --release --example rasterdump -- <session> --ply <splats.ply> --out <folder> [--view K] [--width N]
//!     [--time N] [--nested-forward]
//! ```
//! Writes raw little-endian arrays (`frame.u32`, `proj.f32`, `values.u32`, `ranges.u32`, `out_color.f32`,
//! `out_state.u32`, `d_pixel.f32`, `grad2d.f32`, `sky.f32`) and `meta.json` (sizes, tiles, splats, entries).
//! `--time N`: then times the WGSL forward and backward on that step N times (GPU timestamps), into `wgsl_times.json`;
//! a second backward's sums go to `grad2d_again.f32` (the atomics' run-to-run difference, the comparison's floor).

use anyhow::Context as _;
use orbiters_reconstruct::gpu::{DeviceOptions, Gpu, LearningRates, Target};
use orbiters_reconstruct::{ply, prepare};
use std::path::PathBuf;

fn write<T: bytemuck::Pod>(dir: &std::path::Path, name: &str, v: &[T]) -> anyhow::Result<()> {
    std::fs::write(dir.join(name), bytemuck::cast_slice(v)).with_context(|| name.to_string())
}

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut session, mut ply_path, mut out, mut view, mut width, mut reps) = (None, None, None, 1usize, 1920u32, 0usize);
    let mut nested = false;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ply" => ply_path = Some(PathBuf::from(it.next().context("--ply")?)),
            "--out" => out = Some(PathBuf::from(it.next().context("--out")?)),
            "--view" => view = it.next().context("--view")?.parse()?,
            "--width" => width = it.next().context("--width")?.parse()?,
            "--time" => reps = it.next().context("--time")?.parse()?,
            "--nested-forward" => nested = true,
            _ => session = Some(PathBuf::from(a)),
        }
    }
    let session = session.context("usage: rasterdump <session> --ply p --out dir [--view k]")?;
    let (ply_path, out) = (ply_path.context("--ply")?, out.context("--out")?);
    std::fs::create_dir_all(&out)?;
    let prepared = prepare::prepare(&session, &prepare::Options { width, ..Default::default() }, |_| {})?;
    let data = &prepared.data;
    let g = ply::read(&ply_path)?;
    let mut gpu = Gpu::with_options(DeviceOptions { profile: reps > 0, ..Default::default() })?;
    gpu.upload(&g);
    gpu.nested_forward = nested;
    let sky_file = ply_path.parent().map(PathBuf::from).unwrap_or_default().join(orbiters_core::reconstruction::SKY_FILE);
    if sky_file.is_file() {
        let map = orbiters_reconstruct::sky::SkyMap::read(&sky_file)?;
        gpu.set_sky(Some((map.width, &map.rgb)))?;
    }
    gpu.set_mask(data.width, data.height, &data.mask)?;
    let px = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    // As kbench: tiny rates (the parameters stay), a few warm-up steps so the sort is sized from real counts.
    let zero = LearningRates { means: 1e-20, log_scales: 1e-20, rotations: 1e-20, opacity: 1e-20, colors: 1e-20 };
    for k in 0..3 {
        let v = &data.views[k % data.views.len()];
        gpu.step(&v.camera, [0.0; 3], Target::Color { rgba: &v.image, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 }, Some(zero))?;
    }
    let v = &data.views[view % data.views.len()];
    let d = gpu.lab_raster_dump(&v.camera, Target::Color { rgba: &v.image, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 })?;
    write(&out, "frame.u32", &d.frame)?;
    write(&out, "proj.f32", &d.proj)?;
    write(&out, "values.u32", &d.values)?;
    write(&out, "ranges.u32", &d.ranges)?;
    write(&out, "out_color.f32", &d.out_color)?;
    write(&out, "out_state.u32", &d.out_state)?;
    write(&out, "d_pixel.f32", &d.d_pixel)?;
    write(&out, "grad2d.f32", &d.grad2d)?;
    write(&out, "sky.f32", &d.sky)?;
    let meta = serde_json::json!({
        "session": session, "ply": ply_path, "view": view, "adapter": gpu.adapter,
        "width": v.camera.width, "height": v.camera.height, "tiles": [d.tiles.0, d.tiles.1],
        "splats": d.count, "entries": d.values.len(), "sky_width": (d.sky.len() as f64 * 2.0).sqrt() as u32,
    });
    std::fs::write(out.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;
    println!("{meta}");
    if reps > 0 {
        let times = gpu.lab_time_raster(&v.camera, reps)?;
        let again = gpu.lab_raster_dump(&v.camera, Target::Color { rgba: &v.image, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 })?;
        write(&out, "grad2d_again.f32", &again.grad2d)?;
        let stat = |xs: Vec<f64>| {
            let n = xs.len() as f64;
            let m = xs.iter().sum::<f64>() / n;
            let sd = (xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (n - 1.0).max(1.0)).sqrt();
            serde_json::json!({"mean_ms": m, "sd_ms": sd, "n": xs.len(), "min_ms": xs.iter().cloned().fold(f64::MAX, f64::min)})
        };
        let j = serde_json::json!({
            "forward": stat(times.iter().map(|t| t.0).collect()), "backward": stat(times.iter().map(|t| t.1).collect()),
            "raw": times,
        });
        std::fs::write(out.join("wgsl_times.json"), serde_json::to_string_pretty(&j)?)?;
        println!("wgsl forward {} backward {}", j["forward"], j["backward"]);
    }
    Ok(())
}
