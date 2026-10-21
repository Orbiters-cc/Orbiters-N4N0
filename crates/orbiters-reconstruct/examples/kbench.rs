//! Kernel timings on a trained result: steps with zero learning rates over a session's keyframes, every pass timed.
//!
//! ```text
//! cargo run --release --example kbench -- <session> [--ply <splats.ply>] [--steps N] [--width N] [--depth]
//!     [--sums float|fixed|cas]
//! ```
//! Prints milliseconds per step per section and a gradient checksum of one fixed step (the gradient sums must agree).

use anyhow::Context as _;
use orbiters_reconstruct::gpu::{DeviceOptions, Gpu, GradientSums, LearningRates, Target};
use orbiters_reconstruct::{dataset, ply};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut session, mut ply_path, mut steps, mut width, mut depth, mut sums, mut loose) = (None, None, 40usize, 1920u32, false, None, false);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ply" => ply_path = Some(PathBuf::from(it.next().context("--ply")?)),
            "--steps" => steps = it.next().context("--steps")?.parse()?,
            "--width" => width = it.next().context("--width")?.parse()?,
            "--depth" => depth = true,
            "--loose" => loose = true,
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
    let data = dataset::load(&session, width, |_| {})?;
    let g = ply::read(&ply_path.unwrap_or_else(|| session.join("reconstruction").join("splats.ply")))?;
    let mut gpu = Gpu::with_options(DeviceOptions { sums, profile: true })?;
    gpu.exact_tiles = !loose;
    gpu.upload(&g);
    gpu.set_mask(data.width, data.height, &data.mask);
    let px = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    let zero = LearningRates { means: 0.0, log_scales: 0.0, rotations: 0.0, opacity: 0.0, colors: 0.0 };
    let samples: Vec<Vec<[f32; 4]>> = data
        .depth_views
        .iter()
        .map(|d| d.pixels.iter().zip(&d.depth).filter(|(_, z)| **z > 0.0).map(|(p, z)| [p[0], p[1], *z, 0.0]).collect())
        .collect();
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
        entries += if depth && !data.depth_views.is_empty() {
            let d = k * 7 % data.depth_views.len();
            gpu.step(&data.depth_views[d].camera, [0.0; 3], Target::Depth { samples: &samples[d], weight: 0.3 }, Some(zero))?
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
    // Gradient checksum of one fixed colour step.
    let v = &data.views[1];
    gpu.step(&v.camera, [0.0; 3], Target::Color { rgba: &v.image, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 }, Some(zero))?;
    let grads = gpu.read_grads();
    let mut sums = [0f64; 5];
    for chunk in grads.chunks(16) {
        for (k, range) in [(0usize, 0..3usize), (1, 3..6), (2, 6..10), (3, 10..11), (4, 11..14)] {
            sums[k] += chunk[range].iter().map(|x| x.abs() as f64).sum::<f64>();
        }
    }
    println!("  grad |sum| means {:.6e} scales {:.6e} rot {:.6e} opacity {:.6e} colour {:.6e}", sums[0], sums[1], sums[2], sums[3], sums[4]);
    Ok(())
}
