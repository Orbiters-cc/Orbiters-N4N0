//! Kernel timings on a trained result: steps over a session's keyframes (or depth frames) as the worker prepares it,
//! every pass timed. The learning rates are tiny but not zero (Adam skips a slot whose rate is zero, which would hide
//! its cost), so the parameters stay as they are.
//!
//! ```text
//! cargo run --release --example kbench -- <session> [--ply <splats.ply>] [--steps N] [--width N] [--depth]
//!     [--sums float|fixed|cas]
//! ```
//! Prints milliseconds per step per section and a gradient checksum of one fixed step (the gradient sums must agree).

use anyhow::Context as _;
use orbiters_reconstruct::gpu::{DeviceOptions, Gpu, GradientSums, LearningRates, Target};
use orbiters_reconstruct::{ply, prepare, train};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut session, mut ply_path, mut steps, mut width, mut depth_steps, mut sums) = (None, None, 40usize, 1920u32, false, None);
    let (mut stride, mut sparse, mut sort_subgroups, mut backward2, mut backward4, mut lazy, mut fused, mut forward2, mut ssim2, mut sort8, mut fused_dense) = (1u32, false, false, false, false, false, false, false, false, false, false);
    let mut bw2 = b'c';
    let mut split = false;
    let mut tile8 = 0u8;
    let (mut forward_b, mut dormant) = (false, false);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ply" => ply_path = Some(PathBuf::from(it.next().context("--ply")?)),
            "--steps" => steps = it.next().context("--steps")?.parse()?,
            "--width" => width = it.next().context("--width")?.parse()?,
            "--depth" => depth_steps = true,
            "--stride" => stride = it.next().context("--stride")?.parse()?,
            "--sparse" => sparse = true,
            "--sort-subgroups" => sort_subgroups = true,
            "--backward2" => backward2 = true,
            "--backward4" => backward4 = true,
            "--lazy" => lazy = true,
            "--fused" => fused = true,
            "--forward2" => forward2 = true,
            "--ssim2" => ssim2 = true,
            "--sort8" => sort8 = true,
            "--fused-dense" => fused_dense = true,
            "--split" => split = true,
            "--tile8" => tile8 = 1,
            "--tile8-all" => tile8 = 2,
            "--forward-b" => forward_b = true,
            "--dormant" => dormant = true,
            "--bw2" => bw2 = it.next().context("--bw2")?.bytes().next().unwrap_or(b'c'),
            "--sums" => {
                sums = Some(match it.next().context("--sums")?.as_str() {
                    "float" => GradientSums::Float,
                    "fixed" => GradientSums::FixedPoint,
                    "cas" => GradientSums::CompareAndSwap,
                    other => anyhow::bail!("--sums {other}: float, fixed or cas"),
                })
            }
            _ => session = Some(PathBuf::from(a)),
        }
    }
    let session = session.context("usage: kbench <session> [--ply p] [--steps n]")?;
    let prepared = prepare::prepare(&session, &prepare::Options { width, ..Default::default() }, |_| {})?;
    let data = &prepared.data;
    let g = ply::read(&ply_path.clone().unwrap_or_else(|| session.join("reconstruction").join("splats.ply")))?;
    let mut gpu = Gpu::with_options(DeviceOptions { sums, profile: true, ..Default::default() })?;
    gpu.upload(&g);
    // The sky map trained with the splats, when there is one: its passes are timed too.
    let sky_file = ply_path.as_ref().map_or_else(|| session.join("reconstruction"), |p| p.parent().map(PathBuf::from).unwrap_or_default()).join(orbiters_core::reconstruction::SKY_FILE);
    if sky_file.is_file() {
        let map = orbiters_reconstruct::sky::SkyMap::read(&sky_file)?;
        gpu.set_sky(Some((map.width, &map.rgb)))?;
        println!("with the sky map ({} across)", map.width);
    }
    gpu.set_mask(data.width, data.height, &data.mask)?;
    let px = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    gpu.sparse_adam = sparse;
    gpu.sort_subgroups = sort_subgroups;
    gpu.backward2 = backward2;
    gpu.backward4 = backward4;
    gpu.lazy_adam = lazy;
    gpu.fused_adam = lazy && fused;
    gpu.forward2 = forward2;
    gpu.ssim2 = ssim2;
    gpu.sort8 = sort8;
    gpu.fused_dense = fused_dense;
    gpu.backward2_variant = bw2;
    gpu.split_big = split;
    gpu.tile8 = tile8;
    gpu.forward_b = forward_b;
    gpu.dormant = dormant;
    // Strided colour steps (lab): every stride-th pixel from phase 0, the training resolution's footprint kept.
    let (sw, sh) = (data.width.div_ceil(stride), data.height.div_ceil(stride));
    let strided_mask: Vec<f32> = (0..sh).flat_map(|j| (0..sw).map(move |i| (j, i))).map(|(j, i)| data.mask[(stride * j * data.width + stride * i) as usize]).collect();
    let strided_px = strided_mask.iter().filter(|m| **m > 0.5).count() as f32;
    if stride > 1 {
        gpu.set_mask(sw, sh, &strided_mask)?;
    }
    let strided: Vec<(orbiters_reconstruct::camera::Camera, Vec<u32>)> = if stride > 1 {
        data.views
            .iter()
            .map(|v| {
                let c = &v.camera;
                let f = stride as f32;
                let cam = orbiters_reconstruct::camera::Camera { r: c.r, t: c.t, fx: c.fx / f, fy: c.fy / f, cx: c.cx / f, cy: c.cy / f, width: sw, height: sh };
                let img = (0..sh).flat_map(|j| (0..sw).map(move |i| (j, i))).map(|(j, i)| v.image[(stride * j * data.width + stride * i) as usize]).collect();
                (cam, img)
            })
            .collect()
    } else {
        Vec::new()
    };
    if stride > 1 {
        println!("strided colour steps: every {stride}th pixel, {sw}x{sh}");
    }
    let zero = LearningRates { means: 1e-20, log_scales: 1e-20, rotations: 1e-20, opacity: 1e-20, colors: 1e-20 };
    // Depth steps as training takes them: on the sample grid, trusted samples only.
    let depth: Vec<train::DepthTarget> = data.depth_views.iter().map(train::depth_target).collect();
    // Warm-up (pipelines, buffer growth), then timed steps.
    for k in 0..3 {
        let v = &data.views[k % data.views.len()];
        gpu.step(&v.camera, [0.0; 3], Target::Color { rgba: &v.image, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 }, Some(zero))?;
    }
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    gpu.profiler.as_ref().expect("timestamps").reset();
    let start = std::time::Instant::now();
    let mut entries = 0u64;
    for k in 0..steps {
        entries += if depth_steps && !depth.is_empty() {
            let t = &depth[k * 7 % depth.len()];
            let full = gpu.filter_2d;
            gpu.filter_2d = t.filter_2d;
            let stepped = gpu.step(&t.camera, [0.0; 3], Target::Depth { samples: &t.samples, weight: 0.3 }, Some(zero));
            gpu.filter_2d = full;
            stepped?
        } else if stride > 1 {
            let (cam, img) = &strided[k * 7 % strided.len()];
            let full = gpu.filter_2d;
            let f = (stride * stride) as f32;
            gpu.filter_2d = [full[0] / f, full[1] / f];
            let stepped = gpu.step(cam, [0.0; 3], Target::Color { rgba: img, scene_pixels: strided_px, weight: 1.0, ssim: 0.2, robust: 0.0 }, Some(zero));
            gpu.filter_2d = full;
            stepped?
        } else {
            let v = &data.views[k * 7 % data.views.len()];
            gpu.step(&v.camera, [0.0; 3], Target::Color { rgba: &v.image, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 }, Some(zero))?
        } as u64;
    }
    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
    let wall = start.elapsed().as_secs_f64() * 1000.0 / steps as f64;
    let p = gpu.profiler.as_ref().expect("timestamps");
    let (span, n) = p.span.get();
    println!("{}: {} splats, {}x{}, {:.0} entries/step, wall {wall:.2} ms/step (profiled), GPU span {:.2} ms", gpu.adapter, g.len(), data.width, data.height, entries as f64 / steps as f64, span / n.max(1) as f64);
    for (name, (ms, passes)) in p.totals.borrow().iter() {
        println!("  {name:<18} {:>8.3} ms  ({:.1} passes)", ms / n.max(1) as f64, *passes as f64 / n.max(1) as f64);
    }
    // Lab: how the projection's work spreads over the Gaussians (tile boxes walked by one thread each).
    if std::env::var_os("KBENCH_BOXES").is_some() {
        let v = &data.views[1];
        gpu.step(&v.camera, [0.0; 3], Target::None, None)?;
        let mut boxes: Vec<(u32, u32)> = gpu.read_tile_boxes()?.into_iter().filter(|b| b.0 > 0).collect();
        boxes.sort_unstable();
        let total_box: u64 = boxes.iter().map(|b| b.0 as u64).sum();
        let total_hits: u64 = boxes.iter().map(|b| b.1 as u64).sum();
        let at = |q: f64| boxes[((boxes.len() as f64 - 1.0) * q) as usize];
        println!("  boxes: {} shown, box tiles {total_box}, hits {total_hits}; box p50 {:?} p99 {:?} p99.9 {:?} max {:?}", boxes.len(), at(0.5), at(0.99), at(0.999), boxes.last());
        let big: Vec<&(u32, u32)> = boxes.iter().filter(|b| b.0 > 256).collect();
        println!("  over 256 tiles: {} Gaussians, {} box tiles", big.len(), big.iter().map(|b| b.0 as u64).sum::<u64>());
    }
    // Gradient checksum of one fixed colour step.
    let v = &data.views[1];
    gpu.step(&v.camera, [0.0; 3], Target::Color { rgba: &v.image, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 }, Some(zero))?;
    let grads = gpu.read_grads()?;
    let mut sums = [0f64; 5];
    for chunk in grads.chunks(16) {
        for (k, range) in [(0usize, 0..3usize), (1, 3..6), (2, 6..10), (3, 10..11), (4, 11..14)] {
            sums[k] += chunk[range].iter().map(|x| x.abs() as f64).sum::<f64>();
        }
    }
    println!("  grad |sum| means {:.6e} scales {:.6e} rot {:.6e} opacity {:.6e} colour {:.6e}", sums[0], sums[1], sums[2], sums[3], sums[4]);
    Ok(())
}
