//! A stand-in for a game's frames, to measure what other GPU work does to them: renders a reconstruction at 1920×1080
//! from a camera orbiting its centre once every 11.1 ms (90 Hz) on a device of normal priority, as a game would, and
//! prints how long each frame took from its start until the GPU finished it (a frame over 11.1 ms would be late at
//! 90 Hz). The picture stays on the GPU, as a game's does.
//!
//! ```text
//! cargo run --release --example framegame -- <splats.ply> [--seconds S] [--json out.json]
//! ```

use anyhow::Context as _;
use orbiters_reconstruct::gpu::{self, Target};
use orbiters_reconstruct::{camera::Camera, ply};
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let mut it = std::env::args().skip(1);
    let (mut path, mut seconds, mut json) = (None, 20.0f64, None);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seconds" => seconds = it.next().context("--seconds")?.parse()?,
            "--json" => json = Some(it.next().context("--json")?),
            _ => path = Some(a),
        }
    }
    let g = ply::read(std::path::Path::new(&path.context("usage: framegame <splats.ply>")?))?;
    let mut gpu = gpu::Gpu::new()?;
    gpu.upload(&g);
    let n = g.len().max(1) as f32;
    let centre = g.means.iter().fold([0.0f32; 3], |a, m| [a[0] + m[0] / n, a[1] + m[1] / n, a[2] + m[2] / n]);
    let frame = Duration::from_secs_f64(1.0 / 90.0);
    let start = Instant::now();
    let mut times = Vec::new();
    let mut next = start;
    while start.elapsed().as_secs_f64() < seconds {
        let t = start.elapsed().as_secs_f32();
        let eye = [centre[0] + 3.0 * (0.3 * t).cos(), centre[1] + 0.5, centre[2] + 3.0 * (0.3 * t).sin()];
        let forward = [centre[0] - eye[0], -0.5, centre[2] - eye[2]];
        let len = forward.iter().map(|v| v * v).sum::<f32>().sqrt();
        let cam = Camera::look(eye, forward.map(|v| v / len), [0.0, 1.0, 0.0], 60f32.to_radians(), 1920, 1080);
        let begun = Instant::now();
        gpu.step(&cam, [0.0; 3], Target::None, None)?;
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("{e}"))?;
        times.push(begun.elapsed().as_secs_f64() * 1000.0);
        next += frame;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    let mut sorted = times.clone();
    sorted.sort_by(f64::total_cmp);
    let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
    let late = times.iter().filter(|t| **t > 1000.0 / 90.0).count();
    let summary = serde_json::json!({
        "splats": g.len(), "frames": times.len(), "p50_ms": at(0.5), "p95_ms": at(0.95), "p99_ms": at(0.99), "max_ms": at(1.0),
        "late": late, "late_share": late as f64 / times.len().max(1) as f64, "adapter": gpu.adapter,
    });
    println!("{summary}");
    if let Some(out) = json {
        std::fs::write(out, serde_json::to_vec_pretty(&serde_json::json!({ "summary": summary, "frames_ms": times }))?)?;
    }
    Ok(())
}
