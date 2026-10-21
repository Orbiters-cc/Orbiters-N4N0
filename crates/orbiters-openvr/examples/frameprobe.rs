//! Lab probe (not shipped): the scene application's frame timing as SteamVR's compositor saw it, for `seconds`, as one
//! line of statistics. `frameprobe <seconds> [label]`.
use orbiters_openvr::compositor::FrameTiming;
use orbiters_openvr::{AppType, Client, Universe, MAX_DEVICES};
use std::time::{Duration, Instant};

fn pct(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f32 * p).round() as usize]
}

fn main() {
    let seconds: f64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let label = std::env::args().nth(2).unwrap_or_default();
    let vr = Client::connect(AppType::Background).expect("SteamVR is not running");
    let compositor = vr.compositor().expect("compositor");
    let mut last: Option<u32> = None;
    let (mut interval, mut gpu, mut presents) = (Vec::new(), Vec::new(), Vec::new());
    let (mut dropped, mut mispresented, mut reproj_cpu, mut reproj_gpu, mut reproj_any, mut frames) = (0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
    let mut hmd = Vec::new();
    let mut focus = std::collections::BTreeSet::new();
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < seconds {
        let mut t = [FrameTiming::default(); 64];
        let n = compositor.frame_timings(&mut t);
        for f in &t[..n] {
            if last.is_some_and(|l| f.frame_index <= l) {
                continue;
            }
            frames += 1;
            if f.client_frame_interval_ms > 0.5 && f.client_frame_interval_ms < 2000.0 {
                interval.push(f.client_frame_interval_ms);
            }
            gpu.push(f.pre_submit_gpu_ms + f.post_submit_gpu_ms);
            presents.push(f.num_frame_presents as f32);
            dropped += f.num_dropped_frames;
            mispresented += f.num_mis_presented;
            reproj_cpu += (f.reprojection_flags & 0x01 != 0) as u32;
            reproj_gpu += (f.reprojection_flags & 0x02 != 0) as u32;
            reproj_any += (f.reprojection_flags & 0x0C != 0) as u32;
        }
        if let Some(m) = t[..n].iter().map(|f| f.frame_index).max() {
            last = Some(last.map_or(m, |l| l.max(m)));
        }
        let mut poses = [Default::default(); MAX_DEVICES];
        vr.poses(Universe::Standing, &mut poses);
        let m = poses[0].device_to_absolute.m;
        hmd.push([m[0][3], m[1][3], m[2][3]]);
        focus.insert(compositor.scene_focus_process());
        std::thread::sleep(Duration::from_millis(200));
    }
    let mean = |k: usize| hmd.iter().map(|p| p[k]).sum::<f32>() / hmd.len() as f32;
    let spread = (0..3)
        .map(|k| {
            let m = mean(k);
            (hmd.iter().map(|p| (p[k] - m).powi(2)).sum::<f32>() / hmd.len() as f32).sqrt()
        })
        .sum::<f32>();
    let slow = interval.iter().filter(|i| **i > 1.5 * 1000.0 / 90.0).count();
    println!(
        "{label}: {frames} frames, interval p50 {:.2} p95 {:.2} p99 {:.2} ms, {:.1}% over 1.5 periods; app GPU p50 {:.2} p95 {:.2} ms; presents>1 {:.1}%; dropped {dropped}, mispresented {mispresented}; reprojecting (cpu {reproj_cpu}, gpu {reproj_gpu}, async/motion {reproj_any}); hmd spread {:.3} m; focus {:?}",
        pct(&mut interval.clone(), 0.5), pct(&mut interval.clone(), 0.95), pct(&mut interval.clone(), 0.99),
        100.0 * slow as f32 / interval.len().max(1) as f32,
        pct(&mut gpu.clone(), 0.5), pct(&mut gpu.clone(), 0.95),
        100.0 * presents.iter().filter(|p| **p > 1.0).count() as f32 / presents.len().max(1) as f32,
        spread, focus
    );
}
