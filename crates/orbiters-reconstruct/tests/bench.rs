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
    gpu.set_mask(data.width, data.height, &data.mask).expect("mask");
    let v = &data.views[3];
    let px = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    let lr = LearningRates { means: 0.0, log_scales: 0.0, rotations: 0.0, opacity: 0.0, colors: 0.0 };
    for (name, train) in [("forward", false), ("forward+backward", true)] {
        let mut total = 0;
        let start = Instant::now();
        for _ in 0..10 {
            total = if train {
                gpu.step(&v.camera, [0.0; 3], Target::Color { rgba: &v.image, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 }, Some(lr)).expect("step")
            } else {
                gpu.step(&v.camera, [0.0; 3], Target::None, None).expect("step")
            };
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        }
        eprintln!("{name}: {:.1} ms a step, {} Gaussians, {total} intersections", start.elapsed().as_secs_f64() * 100.0, g.len());
    }
}

#[test]
#[ignore]
fn renderer_setup_time() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
    let adapter = orbiters_reconstruct::gpu::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let (device, queue) = orbiters_reconstruct::gpu::block_on(adapter.request_device(&Default::default())).expect("device");
    let start = Instant::now();
    let _gpu = Gpu::renderer(device, queue);
    eprintln!("{:?}: renderer pipelines in {:?}", adapter.get_info().backend, start.elapsed());
}

#[test]
#[ignore]
fn shader_compile_times() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
    let adapter = orbiters_reconstruct::gpu::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let (device, _queue) = orbiters_reconstruct::gpu::block_on(adapter.request_device(&Default::default())).expect("device");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shaders");
    let common = std::fs::read_to_string(dir.join("common.wgsl")).unwrap();
    let raster = std::fs::read_to_string(dir.join("raster.wgsl")).unwrap();
    for (name, entries, with_common, extra) in [
        ("project", &["main"][..], true, ""),
        ("scan", &["scan_blocks", "add_sums", "total"][..], false, ""),
        ("duplicate", &["main"][..], true, ""),
        ("sort", &["count", "scatter"][..], false, ""),
        ("ranges", &["main"][..], true, ""),
        ("raster_forward", &["forward"][..], true, "raster"),
        ("present", &["main"][..], true, ""),
    ] {
        let src = std::fs::read_to_string(dir.join(format!("{name}.wgsl"))).unwrap();
        let code = [if with_common { common.as_str() } else { "" }, if extra == "raster" { raster.as_str() } else { "" }, src.as_str()].join("\n");
        let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name), source: wgpu::ShaderSource::Wgsl(code.into()) });
        for e in entries {
            let start = Instant::now();
            let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &m, entry_point: Some(e), compilation_options: wgpu::PipelineCompilationOptions { zero_initialize_workgroup_memory: false, ..Default::default() }, cache: None });
            eprintln!("{name}::{e}: {:?}", start.elapsed());
        }
    }
}

/// Compiles every `*.wgsl` of ORBITERS_SHADER_VARIANTS (entry `forward`) and prints how long each takes.
#[test]
#[ignore]
fn shader_variant_times() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
    let adapter = orbiters_reconstruct::gpu::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let (device, _queue) = orbiters_reconstruct::gpu::block_on(adapter.request_device(&Default::default())).expect("device");
    let dir = std::path::PathBuf::from(std::env::var("ORBITERS_SHADER_VARIANTS").expect("ORBITERS_SHADER_VARIANTS"));
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "wgsl")).collect();
    files.sort();
    for f in files {
        let code = std::fs::read_to_string(&f).unwrap();
        let m = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(code.into()) });
        let start = Instant::now();
        let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &m, entry_point: Some("forward"), compilation_options: Default::default(), cache: None });
        eprintln!("{}: {:?}", f.file_name().unwrap().to_string_lossy(), start.elapsed());
    }
}

/// Writes the HLSL wgpu would give D3D for the forward kernel (ORBITERS_HLSL_OUT).
#[test]
#[ignore]
fn forward_hlsl() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shaders");
    let code = ["common.wgsl", "raster.wgsl", "raster_forward.wgsl"].map(|f| std::fs::read_to_string(dir.join(f)).unwrap()).join("\n");
    let module = naga::front::wgsl::parse_str(&code).expect("parse");
    let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).expect("valid");
    let mut out = String::new();
    let options = naga::back::hlsl::Options { shader_model: naga::back::hlsl::ShaderModel::V5_1, ..Default::default() };
    let pipeline = naga::back::hlsl::PipelineOptions { entry_point: Some((naga::ShaderStage::Compute, "forward".into())) };
    naga::back::hlsl::Writer::new(&mut out, &options, &pipeline).write(&module, &info, None).expect("hlsl");
    std::fs::write(std::env::var("ORBITERS_HLSL_OUT").expect("ORBITERS_HLSL_OUT"), out).unwrap();
}

/// What each kind of gradient sums this adapter can do costs a training step on a real session's splats, and what the
/// device says of its limits and memory.
#[test]
#[ignore]
fn gradient_sums_timings() {
    use orbiters_reconstruct::gpu::{DeviceOptions, GradientSums};
    let dir = std::path::PathBuf::from(std::env::var("ORBITERS_SESSION").expect("ORBITERS_SESSION"));
    let data = dataset::load(&dir, 1920, |_| {}).expect("session");
    let g = ply::read(&dir.join("reconstruction").join("splats.ply")).expect("ply");
    let px = data.mask.iter().filter(|m| **m > 0.5).count() as f32;
    let lr = LearningRates { means: 1e-4, log_scales: 0.005, rotations: 0.001, opacity: 0.05, colors: 0.0025 };
    for sums in GradientSums::ALL {
        let Ok(mut gpu) = Gpu::with_options(DeviceOptions { sums: Some(sums), ..Default::default() }) else {
            eprintln!("{sums:?}: not on this device");
            continue;
        };
        let limits = gpu.device.limits();
        eprintln!("{}: largest buffer {} MB, {} storage buffers a stage, free {:?} MB", gpu.adapter, limits.max_buffer_size >> 20, limits.max_storage_buffers_per_shader_stage, gpu.free_memory().map(|b| b >> 20));
        gpu.upload(&g);
        gpu.set_mask(data.width, data.height, &data.mask).expect("mask");
        let (views, mut total) = (data.views.len(), 0);
        for i in 0..10 {
            let v = &data.views[i % views];
            gpu.step(&v.camera, [0.0; 3], Target::Color { rgba: &v.image, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 }, Some(lr)).expect("step");
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        let start = Instant::now();
        for i in 0..40 {
            let v = &data.views[i % views];
            total = gpu.step(&v.camera, [0.0; 3], Target::Color { rgba: &v.image, scene_pixels: px, weight: 1.0, ssim: 0.2, robust: 0.0 }, Some(lr)).expect("step");
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        eprintln!("{sums:?}: {:.1} ms a step, {} Gaussians, {total} intersections, free {:?} MB", start.elapsed().as_secs_f64() * 25.0, g.len(), gpu.free_memory().map(|b| b >> 20));
    }
}

/// What a viewer's frame costs on a device of its own at the sizes a window draws, each frame waited for: the recorded
/// views of a real session's splats.
#[test]
#[ignore]
fn viewer_frame_times() {
    let dir = std::path::PathBuf::from(std::env::var("ORBITERS_SESSION").expect("ORBITERS_SESSION"));
    let data = dataset::load(&dir, 1920, |_| {}).expect("session");
    let g = ply::read(&dir.join("reconstruction").join("splats.ply")).expect("ply");
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
    let adapter = orbiters_reconstruct::gpu::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let limits = adapter.limits();
    let desc = wgpu::DeviceDescriptor { required_limits: wgpu::Limits { max_buffer_size: limits.max_buffer_size, max_storage_buffer_binding_size: limits.max_storage_buffer_binding_size, ..Default::default() }, ..Default::default() };
    let (device, queue) = orbiters_reconstruct::gpu::block_on(adapter.request_device(&desc)).expect("device");
    let mut gpu = Gpu::renderer(device, queue);
    gpu.upload(&g);
    let views = data.views.len();
    for (width, height) in [(828u32, 483u32), (1280, 780), (1920, 1170), (2560, 1560)] {
        let (mut times, mut entries) = (Vec::new(), 0);
        for i in 0..60 {
            let v = data.views[i * 7 % views].camera;
            let (kx, ky) = (width as f32 / v.width as f32, height as f32 / v.height as f32);
            let camera = orbiters_reconstruct::camera::Camera { fx: v.fx * kx, fy: v.fy * ky, cx: v.cx * kx, cy: v.cy * ky, width, height, ..v };
            let start = Instant::now();
            entries = gpu.step(&camera, [0.0; 3], Target::None, None).expect("step");
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
            if i >= 10 {
                times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        times.sort_by(f64::total_cmp);
        eprintln!("{width}x{height} ({:.1} MP): median {:.1} ms, 90 % {:.1} ms, {} splats, {entries} tile entries", (width * height) as f32 / 1e6, times[times.len() / 2], times[times.len() * 9 / 10], g.len());
    }
}

/// How long one dispatch may run before the system stops it, and whether anyone hears of it: Metal reports a failed
/// command buffer on the command buffer alone, and wgpu does not look. Each round doubles the work and says what the
/// kernel's last write left in the buffer (ORBITERS_LONGEST_S: up to how many seconds).
#[test]
#[ignore]
fn how_long_a_dispatch_may_run() {
    let longest: f64 = std::env::var("ORBITERS_LONGEST_S").ok().and_then(|s| s.parse().ok()).unwrap_or(16.0);
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
    let adapter = orbiters_reconstruct::gpu::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let (device, queue) = orbiters_reconstruct::gpu::block_on(adapter.request_device(&Default::default())).expect("device");
    let lost = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    device.on_uncaptured_error({
        let lost = lost.clone();
        std::sync::Arc::new(move |e: wgpu::Error| lost.lock().unwrap().push(e.to_string()))
    });
    device.set_device_lost_callback({
        let lost = lost.clone();
        move |reason, message| lost.lock().unwrap().push(format!("{reason:?}: {message}"))
    });
    let code = "
        @group(0) @binding(0) var<storage, read_write> out: array<u32>;
        @group(0) @binding(1) var<uniform> rounds: vec4<u32>;
        @compute @workgroup_size(64)
        fn main(@builtin(global_invocation_id) id: vec3<u32>) {
            var x = id.x * 2654435761u + 1u;
            for (var i = 0u; i < rounds.x; i++) {
                x ^= x << 13u;
                x ^= x >> 17u;
                x ^= x << 5u;
            }
            out[id.x % arrayLength(&out)] = x | 1u;
        }";
    let module = unsafe { device.create_shader_module_trusted(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(code.into()) }, wgpu::ShaderRuntimeChecks::unchecked()) };
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: None, module: &module, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
    let bytes = 1u64 << 20;
    let out = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: bytes, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let read = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: bytes, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let info = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: out.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: info.as_entire_binding() }],
    });
    let mut rounds = 1u32 << 14;
    loop {
        let mut enc = device.create_command_encoder(&Default::default());
        enc.clear_buffer(&out, 0, None);
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            // Dispatched whole: 16,384 groups of 64 threads.
            pass.dispatch_workgroups(16_384, 1, 1);
        }
        enc.copy_buffer_to_buffer(&out, 0, &read, 0, bytes);
        queue.write_buffer(&info, 0, bytemuck::cast_slice(&[rounds, 0, 0, 0]));
        let start = Instant::now();
        queue.submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        read.slice(..).map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().unwrap().expect("map");
        let seconds = start.elapsed().as_secs_f64();
        let written = {
            let view = read.slice(..).get_mapped_range().expect("mapped");
            bytemuck::cast_slice::<u8, u32>(&view).iter().filter(|v| **v != 0).count()
        };
        read.unmap();
        eprintln!("{rounds} rounds: {seconds:.2} s, {written} of {} words written, heard: {:?}", bytes / 4, lost.lock().unwrap());
        if seconds > longest || written == 0 {
            break;
        }
        rounds = rounds.saturating_mul(2);
    }
}

/// How long a small piece of GPU work, an interface frame's size, waits from another process while something else uses
/// the GPU: run it beside `orbiters-reconstruct` to see what training leaves the window.
#[test]
#[ignore]
fn gpu_latency_beside_other_work() {
    let seconds: u64 = std::env::var("ORBITERS_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(15);
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
    let adapter = orbiters_reconstruct::gpu::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let (device, queue) = orbiters_reconstruct::gpu::block_on(adapter.request_device(&Default::default())).expect("device");
    let size = 2048u32;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut waits = Vec::new();
    let start = Instant::now();
    while start.elapsed().as_secs() < seconds {
        let at = Instant::now();
        let mut enc = device.create_command_encoder(&Default::default());
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            ..Default::default()
        }));
        queue.submit([enc.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        waits.push(at.elapsed().as_secs_f64() * 1000.0);
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    waits.sort_by(f64::total_cmp);
    let at = |q: f64| waits[((waits.len() - 1) as f64 * q) as usize];
    eprintln!("{} frames: median {:.1} ms, 90 % {:.1} ms, 99 % {:.1} ms, worst {:.1} ms", waits.len(), at(0.5), at(0.9), at(0.99), at(1.0));
}
