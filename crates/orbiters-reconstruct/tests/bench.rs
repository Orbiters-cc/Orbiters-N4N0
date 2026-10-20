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
