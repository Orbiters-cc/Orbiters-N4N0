//! `orbiters-desktop --gpu-bench backgrounds [--gpu integrated|graphics-card] [--size <w>x<h>] [--frames <n>]
//! [--pictures <folder> [--at <s>]] `: what one picture of each animated background costs a GPU (timestamps around its passes,
//! the median of `n`), without a window. With `--pictures`, each is saved instead: the scene alone as the website's canvas
//! has it, its still frame, and as it lies behind a page, to put beside the website's.

use anyhow::Context as _;
use eframe::wgpu;
use orbiters_ui::background::{self, Alone, Backdrop, Laid, Scene, Time};
use std::path::PathBuf;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut integrated = false;
    let mut sizes = vec![[1280.0f32, 820.0f32], [2560.0, 1440.0]];
    let mut frames = 120u32;
    let mut pictures: Option<PathBuf> = None;
    let mut at = 8.0f64;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--gpu" => integrated = it.next().map(String::as_str) == Some("integrated"),
            "--size" => {
                let (w, h) = it.next().and_then(|s| s.split_once('x')).context("--size <w>x<h>")?;
                sizes = vec![[w.parse()?, h.parse()?]];
            }
            "--frames" => frames = it.next().context("--frames <n>")?.parse()?,
            "--pictures" => pictures = Some(it.next().context("--pictures <folder>")?.into()),
            "--at" => at = it.next().context("--at <s>")?.parse()?,
            other => anyhow::bail!("unknown option {other}"),
        }
    }
    if let Some(folder) = pictures {
        return save(&folder, sizes[0], at);
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let power_preference = if integrated { wgpu::PowerPreference::LowPower } else { wgpu::PowerPreference::HighPerformance };
    let adapter = orbiters_ui::offscreen::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference, compatible_surface: None, force_fallback_adapter: false, apply_limit_buckets: false })).context("no GPU")?;
    let info = adapter.get_info();
    let timed = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    anyhow::ensure!(adapter.features().contains(timed), "{} cannot time its passes", info.name);
    let (device, queue) = orbiters_ui::offscreen::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_features: timed, required_limits: wgpu::Limits { max_texture_dimension_2d: 8192, ..Default::default() }, ..Default::default() }))?;
    println!("adapter {} ({:?}, {:?})", info.name, info.device_type, info.backend);
    let period = f64::from(queue.get_timestamp_period());
    let queries = device.create_query_set(&wgpu::QuerySetDescriptor { label: None, ty: wgpu::QueryType::Timestamp, count: 2 });
    let resolved = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let read = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    for size in sizes {
        println!("{}x{}", size[0], size[1]);
        for scene in Scene::ALL.into_iter().skip(1) {
            let backdrop = Backdrop::of(scene);
            let mut alone = Alone::new(&device);
            let mut times = Vec::new();
            for n in 0..frames + 20 {
                let time = Time { moved: 6.0 + f64::from(n) / scene.fps(), still: false };
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.write_timestamp(&queries, 0);
                alone.draw(&device, &queue, &mut encoder, &backdrop, time, size, 1.0).context("the scene was not drawn")?;
                encoder.write_timestamp(&queries, 1);
                encoder.resolve_query_set(&queries, 0..2, &resolved, 0);
                encoder.copy_buffer_to_buffer(&resolved, 0, &read, 0, 16);
                queue.submit([encoder.finish()]);
                read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device.poll(wgpu::PollType::wait_indefinitely())?;
                let ticks: [u64; 2] = {
                    let bytes = read.slice(..).get_mapped_range()?;
                    [u64::from_le_bytes(bytes[0..8].try_into()?), u64::from_le_bytes(bytes[8..16].try_into()?)]
                };
                read.unmap();
                if n >= 20 {
                    times.push(ticks[1].saturating_sub(ticks[0]) as f64 * period / 1e6);
                }
            }
            times.sort_by(f64::total_cmp);
            let of = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
            println!("{:>16}  median {:.3} ms  p95 {:.3} ms  at its {} a second {:.2} % of the GPU", scene.id(), of(0.5), of(0.95), scene.fps(), of(0.5) * scene.fps() / 10.0);
        }
    }
    Ok(())
}

fn save(folder: &std::path::Path, size: [f32; 2], at: f64) -> anyhow::Result<()> {
    use orbiters_ui::offscreen::{save_png, Offscreen};
    std::fs::create_dir_all(folder)?;
    let mut gpu = Offscreen::new()?;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0], size[1]));
    for scene in Scene::ALL.into_iter().skip(1) {
        let b = Backdrop::of(scene);
        let moving = Time { moved: at, still: false };
        let veil = b.under(&orbiters_ui::look::Tokens::ORBITERS).0;
        let veiled = Laid { veil, ..Laid::PAGE };
        for (name, time, laid) in [("scene", moving, Laid { picture: true, ..Laid::PAGE }), ("still", Time::STILL, Laid { picture: true, ..Laid::PAGE }), ("behind-a-page", moving, Laid::PAGE), ("behind-a-page-with-the-veil", moving, veiled)] {
            let ctx = egui::Context::default();
            orbiters_ui::install(&ctx);
            let mut image = None;
            for n in 0..2 {
                let input = egui::RawInput { screen_rect: Some(rect), time: Some(f64::from(n)), ..Default::default() };
                let draw = |ui: &mut egui::Ui| {
                    ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
                    background::paint(ui.painter(), egui::Id::unique(("picture", name)), rect, &b, time, laid);
                };
                image = gpu.frame(&ctx, input, draw, n == 1)?;
            }
            save_png(&folder.join(format!("{}-{name}.png", scene.id())), &image.context("no picture")?)?;
        }
        println!("{} (veil {veil:.2})", scene.id());
    }
    Ok(())
}
