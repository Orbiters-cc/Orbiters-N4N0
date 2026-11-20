//! `orbiters-desktop --world-sequence <out> [--sizes 300,2000,...] [--reference 300] [--batches 40] [--quality light|balanced|sharp]
//! [--max-growth 1.3]`: what showing one 3D capture after another costs, measured without a headset (request 18: "when I switch from
//! maps to maps with the 3D capture showing for each of them, my FPS gets lower and lower").
//!
//! It goes through what the overlay's 3D view goes through for each capture (view.rs's render thread): one render device kept for
//! them all, each capture loaded on it ([`FaceRenderer::load`]), its cube's twelve faces drawn whole, then wide batches as the head
//! turns ([`super::view::draw`], the batch the headset sees); and the captures shown lately kept in memory ([`cache::Memory`], its
//! real budget). The captures are made up (no one's files: [`made_up`]), of the sizes given in thousands of splats (by default those
//! the owner flipped through on 2026-11-20, in that order). Between two of them a small reference capture shows again: what it costs
//! the second time, against the first, is what was left behind by those shown in between.
//!
//! After each capture shown: the batch's wall time (median and p95, the GPU waited for), the renderer's sort room and splat buffers
//! ([`FaceRenderer::capacities`]), the graphics memory this process holds (DXGI), its private bytes and threads, the bytes the
//! memory cache counts. `sequence.json` holds them all, and the run fails when the reference's batch grew past `--max-growth` or
//! its sort room or graphics memory did not come back (a regression of request 18's fix).

use super::cache::{self, Key};
use super::catalog::Capture;
use super::render::{self, FaceRenderer};
use super::shots::{eye_views, right_tangents, round2, turn, wide_cameras, LEFT_TANGENTS};
use super::stereo::{self, Cube, Placement, StartView};
use super::view::{self, Job, Quality, Work, HALF_M, MARGIN};
use anyhow::Context as _;
use orbiters_openvr::Matrix34;
use orbiters_reconstruct::gaussians::Gaussians;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

/// The captures the owner flipped through in the 3D worlds' carousel on 2026-11-20 (thousands of splats, in the order shown).
const SESSION_SIZES: [usize; 16] = [303, 2027, 435, 582, 647, 668, 1102, 1261, 152, 2064, 250, 1091, 501, 1372, 745, 2021];

/// One capture shown, measured.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Shown {
    pub what: String,
    pub splats: usize,
    pub load_ms: f64,
    pub cube_ms: f64,
    pub batch_ms_p50: f64,
    pub batch_ms_p95: f64,
    pub sort_room: usize,
    pub splat_room: usize,
    pub gpu_mb: Option<f64>,
    pub private_mb: Option<f64>,
    pub threads: Option<u32>,
    pub cache_counted_mb: f64,
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let out = PathBuf::from(args.first().context("--world-sequence <out> [--sizes 300,2000,...] [--reference 300] [--batches 40]")?);
    std::fs::create_dir_all(&out)?;
    let option = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    let sizes: Vec<usize> = match option("--sizes") {
        Some(s) => s.split(',').map(|k| k.trim().parse::<usize>().map(|k| k * 1000)).collect::<Result<_, _>>().context("--sizes: thousands of splats, by commas")?,
        None => SESSION_SIZES.iter().map(|k| k * 1000).collect(),
    };
    let reference = option("--reference").map(|r| r.parse::<usize>()).transpose()?.unwrap_or(300) * 1000;
    let batches = option("--batches").map(|b| b.parse::<usize>()).transpose()?.unwrap_or(40);
    let quality = match option("--quality").map(String::as_str) {
        Some("light") => Quality::Performance,
        Some("sharp") => Quality::High,
        _ => Quality::Balanced,
    };
    let max_growth = option("--max-growth").map(|g| g.parse::<f64>()).transpose()?.unwrap_or(1.3);

    let mut bench = Bench::new(quality, batches)?;
    eprintln!("on {}, {} captures, reference {} splats, {} batches each at {}", bench.adapter, sizes.len(), reference, batches, quality.label());
    let reference_capture = Arc::new(made_up(1, reference));
    let mut steps = Vec::new();
    let first = bench.show("reference", Key { id: 1, slot: None, key: 0, splats: reference as u32, sky: false }, reference_capture.clone())?;
    eprintln!("{}", line(&first));
    steps.push(first.clone());
    for (i, &n) in sizes.iter().enumerate() {
        let id = 100 + i as u64;
        let capture = Arc::new(made_up(id, n));
        let shown = bench.show(&format!("capture {}", i + 1), Key { id, slot: None, key: 0, splats: n as u32, sky: false }, capture)?;
        eprintln!("{}", line(&shown));
        steps.push(shown);
        let again = bench.show("reference", Key { id: 1, slot: None, key: 0, splats: reference as u32, sky: false }, reference_capture.clone())?;
        eprintln!("{}", line(&again));
        steps.push(again);
    }
    let references: Vec<&Shown> = steps.iter().filter(|s| s.what == "reference").collect();
    let last = references.last().copied().expect("shown first");
    let growth = last.batch_ms_p50 / first.batch_ms_p50.max(0.01);
    let worst = references.iter().map(|s| s.batch_ms_p50).fold(0.0, f64::max) / first.batch_ms_p50.max(0.01);
    let sort_back = last.sort_room <= sort_room_bound(first.sort_room);
    let gpu_back = match (first.gpu_mb, last.gpu_mb) {
        (Some(a), Some(b)) => b <= a + GPU_SLACK_MB,
        _ => true,
    };
    let summary = json!({
        "adapter": bench.adapter,
        "quality": quality.label(),
        "batches_per_capture": batches,
        "reference_splats": reference,
        "reference_batch_ms_p50_first": first.batch_ms_p50,
        "reference_batch_ms_p50_last": last.batch_ms_p50,
        "reference_growth_last": round2(growth),
        "reference_growth_worst": round2(worst),
        "reference_sort_room_first": first.sort_room,
        "reference_sort_room_last": last.sort_room,
        "reference_gpu_mb_first": first.gpu_mb,
        "reference_gpu_mb_last": last.gpu_mb,
        "max_growth": max_growth,
        "passed": growth <= max_growth && sort_back && gpu_back,
        "measurement": "real GPU, the overlay's FaceRenderer and batches on one device kept for every capture, made-up captures; no SteamVR, no compositor",
        "steps": steps,
    });
    std::fs::write(out.join("sequence.json"), serde_json::to_string_pretty(&summary)?)?;
    eprintln!("reference batch {:.2} ms first, {:.2} ms last ({growth:.2}x, worst {worst:.2}x); sort room {} -> {}; GPU {:?} -> {:?} MB", first.batch_ms_p50, last.batch_ms_p50, first.sort_room, last.sort_room, first.gpu_mb, last.gpu_mb); // i18n: not shown (a developer's bench)
    anyhow::ensure!(growth <= max_growth, "the reference capture's batch grew {growth:.2}x after the others (more than {max_growth}x)"); // i18n: not shown (a developer's bench)
    anyhow::ensure!(sort_back, "the renderer kept a sort room of {} entries for a capture that needed {}", last.sort_room, first.sort_room); // i18n: not shown (a developer's bench)
    anyhow::ensure!(gpu_back, "the process kept {:?} MB of graphics memory against {:?} MB the first time", last.gpu_mb, first.gpu_mb); // i18n: not shown (a developer's bench)
    Ok(())
}

/// What a capture's sort room may be the second time it shows, against the first: as large again, or the smallest room a sort gets.
pub fn sort_room_bound(first: usize) -> usize {
    (first * 2).max(1 << 17)
}

/// Graphics memory a capture shown again may hold over its first showing (allocator rounding, the driver's own).
pub const GPU_SLACK_MB: f64 = 48.0;

fn line(s: &Shown) -> String {
    format!(
        "{:<12} {:>8} splats  load {:>7.1} ms  cube {:>7.1} ms  batch p50 {:>6.2} p95 {:>6.2} ms  sort room {:>10}  splat room {:>9}  gpu {:>7} MB  private {:>7} MB  threads {:>3}  cache {:>7.1} MB",
        s.what,
        s.splats,
        s.load_ms,
        s.cube_ms,
        s.batch_ms_p50,
        s.batch_ms_p95,
        s.sort_room,
        s.splat_room,
        s.gpu_mb.map_or("-".into(), |m| format!("{m:.0}")),
        s.private_mb.map_or("-".into(), |m| format!("{m:.0}")),
        s.threads.unwrap_or(0),
        s.cache_counted_mb,
    )
}

/// One render device for every capture, as the overlay's 3D view keeps one (view.rs `Thread`), and the memory cache.
pub struct Bench {
    pub adapter: String,
    #[cfg_attr(not(windows), allow(dead_code))]
    adapter_handle: eframe::wgpu::Adapter,
    device: eframe::wgpu::Device,
    renderer: FaceRenderer,
    kept: cache::Memory,
    quality: Quality,
    batches: usize,
}

impl Bench {
    pub fn new(quality: Quality, batches: usize) -> anyhow::Result<Self> {
        let (adapter, device, queue) = render::device(None)?;
        let renderer = FaceRenderer::new(device.clone(), queue);
        Ok(Self { adapter: adapter.get_info().name, adapter_handle: adapter, device, renderer, kept: cache::Memory::with_default_budget(), quality, batches })
    }

    /// Shows `capture` as the 3D view does after a flip: kept in memory, loaded, its cube drawn whole, then wide batches as the head
    /// turns; measured once its batches are done.
    pub fn show(&mut self, what: &str, key: Key, capture: Arc<Capture>) -> anyhow::Result<Shown> {
        self.kept.put(key.clone(), capture.clone(), Some(&key));
        let (loaded, load_ms) = render::timed(|| -> anyhow::Result<bool> {
            self.renderer.load(&capture)?;
            self.renderer.finish()
        });
        loaded?;
        let head0 = Matrix34 { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1.62], [0.0, 0.0, 1.0, 0.0]] };
        let placement = Placement::at(&capture.view, &head0);
        let views0 = eye_views(&head0);
        let eyes0 = [stereo::position(&views0[0]), stereo::position(&views0[1])];
        let cube = Cube { center: stereo::scale(stereo::add(eyes0[0], eyes0[1]), 0.5), half: HALF_M, pixels: self.quality.pixels(), margin: MARGIN };
        let targets: Vec<eframe::wgpu::Texture> = (0..12).map(|_| render::face_texture(&self.device, cube.width())).collect();
        let target_refs: Vec<&eframe::wgpu::Texture> = targets.iter().collect();
        let count = capture.gaussians.len();
        let whole = Job { id: 0, cube, cube_index: 0, placement, eyes: eyes0, work: Work::Faces((0..6).map(|f| (f, 0)).collect()), count, clip: None, opening: None, wait: 0 };
        let (drawn, cube_ms) = render::timed(|| view::draw(&mut self.renderer, &whole, &target_refs));
        drawn?;
        let tangents = [LEFT_TANGENTS, right_tangents()];
        let mut times = Vec::with_capacity(self.batches);
        for b in 0..self.batches + 2 {
            // The head turns side to side and sways a little: each batch a new view, as a person looking around the capture.
            let yaw = (b as f32 * 0.35).sin() * 30f32.to_radians();
            let mut head = turn(&head0, yaw, (b as f32 * 0.21).sin() * 8f32.to_radians());
            head.m[0][3] += (b as f32 * 0.5).sin() * 0.03;
            let views = eye_views(&head);
            let eyes = [stereo::position(&views[0]), stereo::position(&views[1])];
            let cameras = wide_cameras(&views, tangents, self.quality, false);
            let faces: Vec<(usize, usize)> = (0..6).filter(|&f| cameras.iter().any(|c| cube.face_in(f, c))).map(|f| (f, b % 2)).collect();
            let job = Job { id: b as u64 + 1, cube, cube_index: 0, placement, eyes, work: Work::Wide { cameras, faces }, count, clip: None, opening: None, wait: 0 };
            let (drawn, ms) = render::timed(|| view::draw(&mut self.renderer, &job, &target_refs));
            drawn?;
            // The first two settle the sort's room for this capture (the first frames at a size wait for their count).
            if b >= 2 {
                times.push(ms);
            }
        }
        drop(target_refs);
        drop(targets);
        // What the GPU holds once the textures of this showing went (the view keeps its twelve across captures).
        self.renderer.finish()?;
        times.sort_by(f64::total_cmp);
        let (sort_room, splat_room) = self.renderer.capacities();
        let me = std::process::id();
        let own = crate::perf::scan().into_iter().find(|p| p.pid == me);
        Ok(Shown {
            what: what.to_string(),
            splats: count,
            load_ms: round2(load_ms),
            cube_ms: round2(cube_ms),
            batch_ms_p50: round2(times[times.len() / 2]),
            batch_ms_p95: round2(times[(times.len() * 95 / 100).min(times.len() - 1)]),
            sort_room,
            splat_room,
            gpu_mb: self.gpu_memory().map(|b| round2(b as f64 / 1048576.0)),
            private_mb: own.as_ref().map(|p| round2(p.private as f64 / 1048576.0)),
            threads: own.map(|p| p.threads),
            cache_counted_mb: round2(self.kept.bytes() as f64 / 1048576.0),
        })
    }

    /// The graphics memory this process holds on the adapter, by DXGI (what the driver counts against its budget).
    pub fn gpu_memory(&self) -> Option<u64> {
        #[cfg(windows)]
        {
            // SAFETY: a read-only query of the adapter's DXGI object.
            unsafe {
                self.adapter_handle.as_hal::<eframe::wgpu::hal::api::Dx12>().and_then(|a| {
                    let mut info = windows::Win32::Graphics::Dxgi::DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                    a.as_raw().QueryVideoMemoryInfo(0, windows::Win32::Graphics::Dxgi::DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info).ok().map(|()| info.CurrentUsage)
                })
            }
        }
        #[cfg(not(windows))]
        None
    }

    pub fn capacities(&self) -> (usize, usize) {
        self.renderer.capacities()
    }
}

/// A made-up capture of `splats` splats, the same for the same `seed`: a room's floor, walls and ceiling around its start view, and things
/// standing in it, of splats a few centimetres across (as a capture's), facing every way.
pub fn made_up(seed: u64, splats: usize) -> Capture {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    let mut r = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32
    };
    let mut g = Gaussians::default();
    for i in 0..splats {
        let mean = match i % 5 {
            // The floor and the ceiling, 16 m across.
            0 => [r() * 16.0 - 8.0, r() * 0.03, r() * 16.0 - 8.0],
            1 => [r() * 16.0 - 8.0, 4.0 - r() * 0.03, r() * 16.0 - 8.0],
            // Walls.
            2 => {
                let along = r() * 16.0 - 8.0;
                let side = if r() < 0.5 { -8.0 } else { 8.0 };
                if r() < 0.5 { [along, r() * 4.0, side] } else { [side, r() * 4.0, along] }
            }
            // Things standing in the room: a few dozen columns and boxes.
            _ => {
                let thing = (r() * 40.0) as u32;
                let cx = ((thing * 7919) % 13) as f32 - 6.0;
                let cz = ((thing * 104_729) % 11) as f32 - 5.5;
                [cx + (r() - 0.5) * 0.8, r() * 2.2, cz + (r() - 0.5) * 0.8]
            }
        };
        let size = 0.008 + r() * r() * 0.06;
        let q = [r() - 0.5, r() - 0.5, r() - 0.5, r() - 0.5];
        let norm = (q.iter().map(|v| v * v).sum::<f32>()).sqrt().max(1e-6);
        g.push(mean, [size.ln(), (size * (0.4 + r())).ln(), (size * 0.2).ln()], q.map(|v| v / norm), r() * 5.0 - 1.0, [r() - 0.5, r() - 0.5, r() - 0.5]);
    }
    let view = StartView { position: [0.0, 1.6, 0.0], target: [0.0, 1.6, -1.0], floor: Some(0.0), eye_height: None };
    Capture::new(g, None, view)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Request 18: a capture shown again after larger ones costs what it did the first time. Before the fix the renderer kept the sort
    /// room and splat buffers of the largest capture it had ever drawn, and every later frame of every capture sorted over that room.
    /// Runs on this computer's GPU (a few seconds), skipped without one.
    #[test]
    fn a_capture_shown_again_after_larger_ones_gets_its_own_room_back() {
        let Ok(mut bench) = Bench::new(Quality::Performance, 4) else {
            eprintln!("no GPU adapter here: request 18's sequence is not measured"); // i18n: not shown (a test's note)
            return;
        };
        let key = |id: u64, n: usize| Key { id, slot: None, key: 0, splats: n as u32, sky: false };
        let small = Arc::new(made_up(1, 20_000));
        let first = bench.show("reference", key(1, 20_000), small.clone()).unwrap();
        for (i, n) in [400_000usize, 250_000].into_iter().enumerate() {
            let shown = bench.show("larger", key(10 + i as u64, n), Arc::new(made_up(10 + i as u64, n))).unwrap();
            assert!(shown.sort_room > first.sort_room, "a larger capture needs a larger sort: {shown:?} against {first:?}");
        }
        let again = bench.show("reference", key(1, 20_000), small).unwrap();
        assert!(again.sort_room <= sort_room_bound(first.sort_room), "the sort room stayed the larger captures': {again:?} against {first:?}");
        assert!(again.splat_room <= 2 * first.splats, "the splat buffers stayed the larger captures': {again:?} against {first:?}");
        if let (Some(a), Some(b)) = (first.gpu_mb, again.gpu_mb) {
            assert!(b <= a + GPU_SLACK_MB, "graphics memory {b} MB against {a} MB the first time");
        }
    }

    #[test]
    fn a_made_up_capture_is_the_same_for_the_same_seed() {
        let (a, b) = (made_up(7, 500), made_up(7, 500));
        assert_eq!(a.gaussians.means, b.gaussians.means);
        assert_eq!(a.gaussians.len(), 500);
        assert_ne!(made_up(8, 500).gaussians.means, a.gaussians.means);
    }
}
