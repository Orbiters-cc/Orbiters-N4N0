//! Timings on a real session (run by hand): ORBITERS_SESSION=<folder with reconstruction/splats.ply>
//! cargo test --release --test bench -- --ignored --nocapture

use orbiters_reconstruct::gpu::{Gpu, LearningRates, Target};
use orbiters_reconstruct::{dataset, ply};
use std::time::Instant;

#[test]
#[ignore]
fn step_timings() {
    let dir = std::path::PathBuf::from(std::env::var("ORBITERS_SESSION").expect("ORBITERS_SESSION"));
    let data = dataset::load(&dir, 1920, |_| {}).expect("session");
    let g = ply::read(&dir.join("reconstruction").join("splats.ply")).expect("ply");
    let mut gpu = Gpu::new().expect("gpu");
    gpu.upload(&g);
    gpu.set_mask(data.width, data.height, &data.mask);
    let v = &data.views[3];
    let px = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    let lr = LearningRates { means: 0.0, log_scales: 0.0, rotations: 0.0, opacity: 0.0, colors: 0.0 };
    for (name, train) in [("forward", false), ("forward+backward", true)] {
        let mut total = 0;
        let start = Instant::now();
        for _ in 0..10 {
            total = if train {
                gpu.step(&v.camera, [0.0; 3], Target::Color { rgba: &v.image, scene_pixels: px, weight: 1.0 }, Some(lr)).expect("step")
            } else {
                gpu.step(&v.camera, [0.0; 3], Target::None, None).expect("step")
            };
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        }
        eprintln!("{name}: {:.1} ms a step, {} Gaussians, {total} intersections", start.elapsed().as_secs_f64() * 100.0, g.len());
    }
}
