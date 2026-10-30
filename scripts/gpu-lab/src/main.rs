//! The measurements of `docs/igpu-experiment.md` (Windows only).
//!
//! - `adapters`: the DXGI adapters and their LUIDs (Windows' GPU counters name adapters by LUID).
//! - `sample <seconds> [--every <ms>] [--out <file>]`: Windows' GPU counters for every process, collected every
//!   `every` ms: each engine's utilisation per process and adapter, and each process's dedicated and shared GPU
//!   memory. Prints the averages; `--out` writes every collection as JSON lines.
//! - `load <adapter> <seconds> [--size <w>x<h>] [--work <n>] [--paced <hz>] [--out <file>]`: a GPU-bound stand-in for a
//!   game on the adapter whose name contains `adapter`: a heavy fragment shader over a `w`×`h` target, frames back to
//!   back (or `hz` a second), each timed with GPU timestamps. Prints a line a second and a summary.
//! - `overlay <adapter> <seconds> [--rate <hz>] [--rect <x0>,<y0>,<x1>,<y1>] [--alternate <s>]`: what the SteamVR
//!   overlay's D3D11 path does (orbiters-openvr `gpu.rs`), without SteamVR: two 1024×640 textures written in turn, the
//!   changed part with UpdateSubresource and a Flush, `hz` times a second (with `--alternate`, for that long, then
//!   nothing as long, and so on).

use anyhow::{bail, Context as _};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("adapters") => adapters(),
        Some("sample") => sample(&args[1..]),
        Some("load") => load(&args[1..]),
        Some("overlay") => overlay(&args[1..]),
        Some("cpu") => cpu(&args[1..]),
        _ => bail!("usage: gpu-lab adapters | sample <s> [--every ms] [--out f] | load <adapter> <s> [--size WxH] [--work n] [--paced hz] [--out f] | overlay <adapter> <s> [--rate hz] [--rect x0,y0,x1,y1] [--alternate s] | cpu alu|mem <s>"),
    }
}

/// `cpu alu|mem <seconds>`: one thread's speed, for what a busy integrated GPU takes from the processor it shares
/// power and memory with (a game's main thread): `alu` a chain of arithmetic, `mem` dependent reads over 256 MB.
/// Prints rounds a second each second and their mean.
fn cpu(args: &[String]) -> anyhow::Result<()> {
    like_a_game();
    let kind = args.first().context("cpu alu|mem <seconds>")?.clone();
    let seconds: f64 = args.get(1).context("cpu alu|mem <seconds>")?.parse()?;
    // A random cycle through 32 M entries (256 MB): each read depends on the one before.
    let next: Vec<u64> = if kind == "mem" {
        let n = 32usize << 20;
        let mut order: Vec<u64> = (0..n as u64).collect();
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        for i in (1..n).rev() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            order.swap(i, (x % (i as u64 + 1)) as usize);
        }
        let mut next = vec![0u64; n];
        for w in 0..n {
            next[order[w] as usize] = order[(w + 1) % n];
        }
        next
    } else {
        Vec::new()
    };
    println!("cpu {kind}, pid {}", std::process::id());
    let _ = std::io::stdout().flush();
    let start = Instant::now();
    let mut per_second = Vec::new();
    let (mut acc, mut at) = (1u64, 0u64);
    while start.elapsed().as_secs_f64() < seconds {
        let t0 = Instant::now();
        let mut rounds = 0u64;
        while t0.elapsed() < Duration::from_secs(1) {
            if kind == "mem" {
                for _ in 0..100_000 {
                    at = next[at as usize];
                }
            } else {
                for _ in 0..1_000_000 {
                    acc = acc.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) ^ (acc >> 29);
                }
            }
            rounds += 1;
        }
        let rate = rounds as f64 / t0.elapsed().as_secs_f64();
        println!("{:6.1} s  {rate:9.2} rounds a second", start.elapsed().as_secs_f64());
        let _ = std::io::stdout().flush();
        per_second.push(rate);
    }
    let mean = per_second.iter().sum::<f64>() / per_second.len().max(1) as f64;
    let sd = (per_second.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / per_second.len().max(1) as f64).sqrt();
    println!("summary: {mean:.2} rounds a second (sd {sd:.2}) over {} s [{}]", per_second.len(), acc ^ at);
    Ok(())
}

/// `--name value` from `args`.
fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

// ---------------------------------------------------------------------------------------------------------------
// Adapters

struct DxgiAdapter {
    index: u32,
    name: String,
    luid: String,
    vendor: u32,
    device: u32,
    dedicated_mb: u64,
    shared_mb: u64,
    outputs: u32,
    software: bool,
}

fn dxgi_adapters() -> anyhow::Result<Vec<DxgiAdapter>> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }?;
    let mut list = Vec::new();
    let mut i = 0;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(i) } {
        let desc = unsafe { adapter.GetDesc1() }?;
        let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
        let mut outputs = 0;
        while unsafe { adapter.EnumOutputs(outputs) }.is_ok() {
            outputs += 1;
        }
        list.push(DxgiAdapter {
            index: i,
            name: String::from_utf16_lossy(&desc.Description[..end]).trim().to_string(),
            luid: format!("0x{:08x}_0x{:08x}", desc.AdapterLuid.HighPart as u32, desc.AdapterLuid.LowPart),
            vendor: desc.VendorId,
            device: desc.DeviceId,
            dedicated_mb: desc.DedicatedVideoMemory as u64 >> 20,
            shared_mb: desc.SharedSystemMemory as u64 >> 20,
            outputs,
            software: desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
        });
        i += 1;
    }
    Ok(list)
}

fn adapters() -> anyhow::Result<()> {
    for a in dxgi_adapters()? {
        println!(
            "{}  luid {}  {:04x}:{:04x}  {}  dedicated {} MB, shared {} MB, {} output(s){}",
            a.index,
            a.luid,
            a.vendor,
            a.device,
            a.name,
            a.dedicated_mb,
            a.shared_mb,
            a.outputs,
            if a.software { ", software" } else { "" }
        );
    }
    let instance = wgpu_instance();
    for a in pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12)) {
        let info = a.get_info();
        println!("wgpu: {} ({:?}, {:?}, {:04x}:{:04x})", info.name, info.device_type, info.backend, info.vendor, info.device);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// Counters

struct Pdh {
    query: windows::Win32::System::Performance::PDH_HQUERY,
    counters: Vec<(&'static str, windows::Win32::System::Performance::PDH_HCOUNTER)>,
}

impl Pdh {
    fn new(paths: &[(&'static str, &str)]) -> anyhow::Result<Self> {
        use windows::Win32::System::Performance::*;
        let mut query = PDH_HQUERY::default();
        let status = unsafe { PdhOpenQueryW(windows::core::PCWSTR::null(), 0, &mut query) };
        if status != 0 {
            bail!("PdhOpenQuery: 0x{status:08x}");
        }
        let mut counters = Vec::new();
        for (key, path) in paths {
            let w = wide(path);
            let mut c = PDH_HCOUNTER::default();
            let status = unsafe { PdhAddEnglishCounterW(query, windows::core::PCWSTR(w.as_ptr()), 0, &mut c) };
            if status != 0 {
                bail!("PdhAddEnglishCounter {path}: 0x{status:08x}");
            }
            counters.push((*key, c));
        }
        unsafe { PdhCollectQueryData(query) };
        Ok(Self { query, counters })
    }

    /// Collects once; every counter's instances and values.
    fn collect(&self) -> Vec<(&'static str, Vec<(String, f64)>)> {
        use windows::Win32::System::Performance::*;
        unsafe { PdhCollectQueryData(self.query) };
        let mut out = Vec::new();
        for (key, c) in &self.counters {
            let (mut size, mut count) = (0u32, 0u32);
            let status = unsafe { PdhGetFormattedCounterArrayW(*c, PDH_FMT_DOUBLE, &mut size, &mut count, None) };
            if status != PDH_MORE_DATA || size == 0 {
                out.push((*key, Vec::new()));
                continue;
            }
            // u64 words: the items hold f64s and pointers.
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            let status = unsafe { PdhGetFormattedCounterArrayW(*c, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) };
            let mut values = Vec::new();
            if status == 0 {
                for i in 0..count as usize {
                    let item = unsafe { &*items.add(i) };
                    if item.FmtValue.CStatus != PDH_CSTATUS_VALID_DATA && item.FmtValue.CStatus != PDH_CSTATUS_NEW_DATA {
                        continue;
                    }
                    let name = unsafe { item.szName.to_string() }.unwrap_or_default();
                    values.push((name, unsafe { item.FmtValue.Anonymous.doubleValue }));
                }
            }
            out.push((*key, values));
        }
        out
    }
}

impl Drop for Pdh {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::Performance::PdhCloseQuery(self.query) };
    }
}

/// Process names by id, from a ToolHelp snapshot.
fn process_names() -> BTreeMap<u32, String> {
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut names = BTreeMap::new();
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else { return names };
    let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
    let mut ok = unsafe { Process32FirstW(snap, &mut entry) }.is_ok();
    while ok {
        let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
        names.insert(entry.th32ProcessID, String::from_utf16_lossy(&entry.szExeFile[..end]));
        ok = unsafe { Process32NextW(snap, &mut entry) }.is_ok();
    }
    let _ = unsafe { windows::Win32::Foundation::CloseHandle(snap) };
    names
}

/// `pid_1234_luid_0x00000000_0x00011772_phys_0_eng_0_engtype_3D` → (1234, luid, "3D").
fn engine_instance(name: &str) -> Option<(u32, String, String)> {
    let rest = name.strip_prefix("pid_")?;
    let (pid, rest) = rest.split_once("_luid_")?;
    let (luid, rest) = rest.split_once("_phys_")?;
    let engtype = rest.split_once("_engtype_").map(|(_, t)| t).unwrap_or("");
    Some((pid.parse().ok()?, luid.to_string(), if engtype.is_empty() { "?".into() } else { engtype.to_string() }))
}

/// `pid_1234_luid_0x00000000_0x00011772_phys_0` → (1234, luid).
fn memory_instance(name: &str) -> Option<(u32, String)> {
    let rest = name.strip_prefix("pid_")?;
    let (pid, rest) = rest.split_once("_luid_")?;
    let luid = rest.split_once("_phys_").map(|(l, _)| l).unwrap_or(rest);
    Some((pid.parse().ok()?, luid.to_string()))
}

fn sample(args: &[String]) -> anyhow::Result<()> {
    let seconds: f64 = args.first().context("sample <seconds>")?.parse()?;
    let every = Duration::from_millis(opt(args, "--every").unwrap_or("500").parse()?);
    let mut out = match opt(args, "--out") {
        Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(p)?)),
        None => None,
    };
    let pdh = Pdh::new(&[
        ("util", r"\GPU Engine(*)\Utilization Percentage"),
        ("dedicated", r"\GPU Process Memory(*)\Dedicated Usage"),
        ("shared", r"\GPU Process Memory(*)\Shared Usage"),
    ])?;
    let adapters: BTreeMap<String, String> = dxgi_adapters()?.into_iter().map(|a| (a.luid, a.name)).collect();
    let short = |luid: &str| adapters.get(luid).cloned().unwrap_or_else(|| luid.to_string());
    let mut names = process_names();
    // Sums of utilisation per (pid, luid, engine type), and how many collections.
    let mut util: BTreeMap<(u32, String, String), f64> = BTreeMap::new();
    let mut memory: BTreeMap<(u32, String), (f64, f64)> = BTreeMap::new();
    let mut collections = 0u32;
    let start = Instant::now();
    let mut next = start + every;
    while start.elapsed().as_secs_f64() < seconds {
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
        next += every;
        let data = pdh.collect();
        collections += 1;
        let t = start.elapsed().as_secs_f64();
        let mut line: BTreeMap<String, f64> = BTreeMap::new();
        for (key, values) in data {
            for (inst, v) in values {
                match key {
                    "util" => {
                        if let Some((pid, luid, eng)) = engine_instance(&inst) {
                            *util.entry((pid, luid.clone(), eng.clone())).or_default() += v;
                            if v > 0.0 {
                                *line.entry(format!("{pid}|{luid}|{eng}")).or_default() += v;
                            }
                        }
                    }
                    "dedicated" | "shared" => {
                        if let Some((pid, luid)) = memory_instance(&inst) {
                            let e = memory.entry((pid, luid)).or_default();
                            if key == "dedicated" {
                                e.0 = v;
                            } else {
                                e.1 = v;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(f) = out.as_mut() {
            let fields: Vec<String> = line.iter().map(|(k, v)| format!("\"{k}\":{v:.4}")).collect();
            writeln!(f, "{{\"t\":{t:.3},\"util\":{{{}}}}}", fields.join(","))?;
        }
    }
    for pid in util.keys().map(|k| k.0).chain(memory.keys().map(|k| k.0)) {
        if !names.contains_key(&pid) {
            names = process_names();
            break;
        }
    }
    let name = |pid: u32| names.get(&pid).cloned().unwrap_or_else(|| "?".into());
    println!("{collections} collections over {:.1} s", start.elapsed().as_secs_f64());
    println!("utilisation (% of the engine's time, mean over the collections), above 0.01 %:");
    let mut rows: Vec<_> = util.iter().map(|(k, v)| (k, v / collections.max(1) as f64)).filter(|(_, v)| *v >= 0.01).collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    for ((pid, luid, eng), v) in &rows {
        println!("  {v:7.3}  {pid:>6} {:<28} {:<32} {eng}", name(*pid), short(luid));
    }
    println!("per adapter and engine type, all processes:");
    let mut totals: BTreeMap<(String, String), f64> = BTreeMap::new();
    for ((_, luid, eng), v) in &rows {
        *totals.entry((short(luid), eng.clone())).or_default() += v;
    }
    for ((adapter, eng), v) in &totals {
        println!("  {v:7.3}  {adapter:<32} {eng}");
    }
    println!("GPU memory at the end (MB), above 1 MB:");
    for ((pid, luid), (d, s)) in &memory {
        if d + s >= 1e6 {
            println!("  {pid:>6} {:<28} {:<32} dedicated {:8.1}  shared {:8.1}", name(*pid), short(luid), d / 1e6, s / 1e6);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// A GPU-bound load

fn wgpu_instance() -> wgpu::Instance {
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::DX12,
        flags: wgpu::InstanceFlags::default(),
        backend_options: wgpu::BackendOptions::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        display: None,
    })
}

const LOAD: &str = r#"
struct Params { iterations: u32, frame: u32, a: u32, b: u32 }
@group(0) @binding(0) var<uniform> p: Params;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    var x = pos.x * 0.0013 + f32(p.frame) * 0.003;
    var y = pos.y * 0.0017;
    for (var i = 0u; i < p.iterations; i++) {
        let t = x;
        x = sin(x * 1.7 + y) * 0.9 + cos(y * 1.3) * 0.1;
        y = cos(t * 1.1 - y) * 0.9 + sin(t * 0.7) * 0.1;
    }
    return vec4<f32>(x * 0.5 + 0.5, y * 0.5 + 0.5, 0.5, 1.0);
}
"#;

fn pick_adapter(instance: &wgpu::Instance, name: &str) -> anyhow::Result<wgpu::Adapter> {
    let all = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12));
    let lower = name.to_lowercase();
    all.into_iter().find(|a| a.get_info().name.to_lowercase().contains(&lower)).with_context(|| format!("no adapter named like {name}"))
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// Runs this process as a game in front would: above normal priority, and out of Windows' power throttling (EcoQoS),
/// which slows background processes on the processor's efficiency cores.
fn like_a_game() {
    use windows::Win32::System::Threading::{
        GetCurrentProcess, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation, ABOVE_NORMAL_PRIORITY_CLASS,
        PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    };
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: 0,
    };
    unsafe {
        let me = GetCurrentProcess();
        let _ = SetPriorityClass(me, ABOVE_NORMAL_PRIORITY_CLASS);
        let _ = SetProcessInformation(me, ProcessPowerThrottling, std::ptr::from_ref(&state).cast(), std::mem::size_of_val(&state) as u32);
    }
}

fn load(args: &[String]) -> anyhow::Result<()> {
    like_a_game();
    let name = args.first().context("load <adapter> <seconds>")?;
    let seconds: f64 = args.get(1).context("load <adapter> <seconds>")?.parse()?;
    let (w, h) = match opt(args, "--size") {
        Some(s) => {
            let (a, b) = s.split_once('x').context("--size WxH")?;
            (a.parse()?, b.parse()?)
        }
        None => (2560u32, 1440u32),
    };
    let iterations: u32 = opt(args, "--work").unwrap_or("64").parse()?;
    let paced: Option<f64> = opt(args, "--paced").map(str::parse).transpose()?;
    let feedback = args.iter().any(|s| s == "--feedback");
    anyhow::ensure!(!feedback || paced.is_some(), "--feedback requires --paced <hz>");
    let mut out = match opt(args, "--out") {
        Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(p)?)),
        None => None,
    };
    let instance = wgpu_instance();
    let adapter = pick_adapter(&instance, name)?;
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gpu-lab load"),
        required_features: wgpu::Features::TIMESTAMP_QUERY,
        ..Default::default()
    }))?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("load target"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("load"), source: wgpu::ShaderSource::Wgsl(LOAD.into()) });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("load"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        }],
    });
    let pipeline_layout =
        device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("load"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("load"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        multiview_mask: None,
        cache: None,
    });
    let params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("load params"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("load"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() }],
    });
    let queries = device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("load timestamps"), ty: wgpu::QueryType::Timestamp, count: 2 });
    let resolve = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("load resolve"),
        size: 16,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("load readback"),
        size: 16,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let period = queue.get_timestamp_period() as f64;
    println!("load on {} ({:?}), {w}x{h}, {iterations} iterations{}", info.name, info.device_type, paced.map(|hz| format!(", {hz} Hz")).unwrap_or_default());
    println!("pid {}", std::process::id());
    let _ = std::io::stdout().flush();

    let mut gpu_ms: Vec<f64> = Vec::new();
    let mut wall_ms: Vec<f64> = Vec::new();
    let start = Instant::now();
    let mut last = Instant::now();
    let mut second = (Instant::now(), 0u32, 0.0f64);
    let mut frame = 0u32;
    let interval = paced.map(|hz| Duration::from_secs_f64(1.0 / hz));
    let mut due = Instant::now();
    while start.elapsed().as_secs_f64() < seconds {
        let scheduled = due;
        if let Some(interval) = interval {
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
            due += interval;
        }
        queue.write_buffer(&params, 0, &[iterations.to_le_bytes(), frame.to_le_bytes(), [0; 4], [0; 4]].concat());
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("load"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: Some(wgpu::RenderPassTimestampWrites {
                    query_set: &queries,
                    beginning_of_pass_write_index: Some(0),
                    end_of_pass_write_index: Some(1),
                }),
                multiview_mask: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        enc.resolve_query_set(&queries, 0..2, &resolve, 0);
        enc.copy_buffer_to_buffer(&resolve, 0, &readback, 0, 16);
        queue.submit([enc.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).context("GPU wait")?;
        let ticks: [u64; 2] = {
            let view = readback.get_mapped_range(..).context("readback")?;
            [u64::from_le_bytes(view[0..8].try_into()?), u64::from_le_bytes(view[8..16].try_into()?)]
        };
        readback.unmap();
        let g = ticks[1].saturating_sub(ticks[0]) as f64 * period / 1e6;
        let now = Instant::now();
        let wall = (now - last).as_secs_f64() * 1e3;
        last = now;
        frame += 1;
        // The first frames compile and warm up.
        if frame > 10 {
            gpu_ms.push(g);
            wall_ms.push(wall);
            if let Some(f) = out.as_mut() {
                // End-to-end ready time from this frame's intended start, including CPU scheduling and GPU queue
                // waits. Compare it with the paced interval; wall-to-wall gaps alone are not deadline misses.
                if feedback {
                    let ready_ms = (now - scheduled).as_secs_f64() * 1e3;
                    writeln!(f, "{:.4},{g:.4},{wall:.4},{ready_ms:.4}", start.elapsed().as_secs_f64())?;
                    f.flush()?;
                } else {
                    writeln!(f, "{:.4},{g:.4},{wall:.4}", start.elapsed().as_secs_f64())?;
                }
            }
        }
        second.1 += 1;
        second.2 += g;
        if second.0.elapsed() >= Duration::from_secs(1) {
            println!("{:6.1} s  {:4} frames  GPU {:6.3} ms a frame", start.elapsed().as_secs_f64(), second.1, second.2 / second.1 as f64);
            let _ = std::io::stdout().flush();
            if let Some(f) = out.as_mut() {
                f.flush()?;
            }
            second = (Instant::now(), 0, 0.0);
        }
    }
    let n = gpu_ms.len();
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let (gm, wm) = (mean(&gpu_ms), mean(&wall_ms));
    let sd = (gpu_ms.iter().map(|x| (x - gm).powi(2)).sum::<f64>() / n.max(1) as f64).sqrt();
    let mut gs = gpu_ms.clone();
    gs.sort_by(f64::total_cmp);
    let mut ws = wall_ms.clone();
    ws.sort_by(f64::total_cmp);
    println!(
        "summary: {n} frames, {:.2} frames/s; GPU ms mean {gm:.4} sd {sd:.4} p50 {:.4} p95 {:.4} p99 {:.4} max {:.4}; wall ms mean {wm:.4} p50 {:.4} p99 {:.4} max {:.4}",
        n as f64 / (wall_ms.iter().sum::<f64>() / 1e3),
        percentile(&gs, 0.5),
        percentile(&gs, 0.95),
        percentile(&gs, 0.99),
        gs.last().copied().unwrap_or(f64::NAN),
        percentile(&ws, 0.5),
        percentile(&ws, 0.99),
        ws.last().copied().unwrap_or(f64::NAN),
    );
    if let Some(budget) = interval {
        let late = wall_ms.iter().filter(|&&w| w > budget.as_secs_f64() * 1e3 * 1.5).count();
        println!("paced: {late} frames took over 1.5 intervals");
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------
// The overlay's texture uploads

fn overlay(args: &[String]) -> anyhow::Result<()> {
    use windows::core::Interface;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_11_0};
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Texture2D, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
        D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1};
    let name = args.first().context("overlay <adapter> <seconds>")?.to_lowercase();
    let seconds: f64 = args.get(1).context("overlay <adapter> <seconds>")?.parse()?;
    let rate: f64 = opt(args, "--rate").unwrap_or("45").parse()?;
    let alternate: f64 = opt(args, "--alternate").unwrap_or("0").parse()?;
    let (width, height) = (1024u32, 640u32);
    let rect: [u32; 4] = match opt(args, "--rect") {
        Some(r) => {
            let v: Vec<u32> = r.split(',').map(str::parse).collect::<Result<_, _>>()?;
            [v[0], v[1], v[2], v[3]]
        }
        None => [0, 0, width, height],
    };
    let index = dxgi_adapters()?.into_iter().find(|a| a.name.to_lowercase().contains(&name)).context("no such adapter")?.index;
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }?;
    let adapter: IDXGIAdapter = unsafe { factory.EnumAdapters1(index) }?.cast()?;
    let levels = [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_0];
    let (mut device, mut context) = (None, None);
    unsafe {
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }?;
    let (device, context) = (device.context("no device")?, context.context("no context")?);
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut textures: Vec<ID3D11Texture2D> = Vec::new();
    for _ in 0..2 {
        let mut t = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut t)) }?;
        textures.push(t.context("no texture")?);
    }
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    println!("overlay uploads on adapter {index}, {rate} a second, part {rect:?} of {width}x{height}");
    println!("pid {}", std::process::id());
    let _ = std::io::stdout().flush();
    let start = Instant::now();
    let interval = Duration::from_secs_f64(1.0 / rate);
    let mut due = Instant::now();
    let mut n = 0u64;
    let mut cpu = Duration::ZERO;
    while start.elapsed().as_secs_f64() < seconds {
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }
        due += interval;
        if alternate > 0.0 && (start.elapsed().as_secs_f64() / alternate) as u64 % 2 == 1 {
            continue;
        }
        // A new picture: the part changes (as a spring animation or a scroll would).
        let v = (n % 251) as u8;
        for y in rect[1]..rect[3] {
            let row = (y * width) as usize * 4;
            pixels[row + rect[0] as usize * 4..row + rect[2] as usize * 4].fill(v);
        }
        let t0 = Instant::now();
        let part = D3D11_BOX { left: rect[0], top: rect[1], front: 0, right: rect[2], bottom: rect[3], back: 1 };
        let first = (rect[1] as usize * width as usize + rect[0] as usize) * 4;
        unsafe {
            context.UpdateSubresource(&textures[(n % 2) as usize], 0, Some(&part), pixels[first..].as_ptr().cast(), width * 4, 0);
            context.Flush();
        }
        cpu += t0.elapsed();
        n += 1;
    }
    println!("summary: {n} uploads in {:.1} s, {:.3} ms of CPU each", start.elapsed().as_secs_f64(), cpu.as_secs_f64() * 1e3 / n.max(1) as f64);
    Ok(())
}
