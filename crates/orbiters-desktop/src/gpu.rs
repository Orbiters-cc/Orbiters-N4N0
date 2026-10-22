//! GPU setup for the window. wgpu's defaults suit games: the "performance" memory hint reserves 128 MB device and
//! 64 MB host blocks (system RAM on an integrated GPU), and the D3D12 backend maps a million-entry descriptor heap
//! (32 MB). The window asks for small blocks and a small heap instead (it then costs about the driver's own floor),
//! renders on the adapter that drives the primary display (no copy between GPUs; on a laptop that is the integrated
//! GPU that owns the panel), on Windows loads only the DX12 drivers (no Vulkan or OpenGL probing), and reports GPU
//! errors instead of closing ([`report_errors`]).

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetupCreateNew};
use eframe::wgpu;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// wgpu answers an error nobody was set to catch with a panic: one frame of the 3D view the GPU refused (a sort over
/// its limits, an allocation the driver turned down, a device lost under VRChat's load) closed the whole window.
/// They are printed instead (the first few, then one in a hundred) and the window carries on; the 3D view says
/// what it could not draw.
pub fn report_errors(device: &wgpu::Device) {
    let count = Arc::new(AtomicU64::new(0));
    device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
        let n = count.fetch_add(1, Ordering::Relaxed);
        if n < 5 || n.is_multiple_of(100) {
            eprintln!("window GPU error ({}): {e}", n + 1);
        }
    }));
}

/// The window's device, for pages that draw their own GPU content (the 3D view of splats), and its renderer, whose
/// textures the progressive blur under the title bar draws with.
#[derive(Clone)]
pub struct Handles {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub textures: orbiters_ui::blur::Textures,
}

pub fn options() -> WgpuConfiguration {
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    if wgpu::Backends::from_env().is_none() {
        setup.instance_descriptor.backends = if cfg!(windows) { wgpu::Backends::DX12 } else { wgpu::Backends::VULKAN | wgpu::Backends::GL };
    }
    setup.native_adapter_selector = Some(Arc::new(|adapters: &[wgpu::Adapter], _surface: Option<&wgpu::Surface<'_>>| select(adapters)));
    setup.device_descriptor = Arc::new(|adapter: &wgpu::Adapter| {
        let base = if adapter.get_info().backend == wgpu::Backend::Gl { wgpu::Limits::downlevel_webgl2_defaults() } else { wgpu::Limits::default() };
        // The 3D view sorts millions of tile entries: buffers as large as the GPU allows (a limit, not an allocation).
        let gpu = adapter.limits();
        let base = wgpu::Limits { max_buffer_size: gpu.max_buffer_size, max_storage_buffer_binding_size: gpu.max_storage_buffer_binding_size, ..base };
        wgpu::DeviceDescriptor {
            label: Some("orbiters window"),
            required_limits: limits(base),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            ..Default::default()
        }
    });
    WgpuConfiguration { wgpu_setup: setup.into(), ..Default::default() }
}

/// Limits for egui: big enough textures, and a D3D12 view heap for thousands of bind groups instead of the default
/// million (32 MB mapped at device creation).
pub fn limits(base: wgpu::Limits) -> wgpu::Limits {
    wgpu::Limits { max_texture_dimension_2d: 8192, max_non_sampler_bindings: 16_384, ..base }
}

/// The adapter driving the primary display, else the best other GPU (discrete, integrated, virtual), never the
/// software rasterizer unless it is all there is.
fn select(adapters: &[wgpu::Adapter]) -> Result<wgpu::Adapter, String> {
    #[cfg(windows)]
    if let Some((vendor, device)) = primary_display_adapter() {
        if let Some(a) = adapters.iter().find(|a| {
            let info = a.get_info();
            info.vendor == vendor && info.device == device
        }) {
            return Ok(a.clone());
        }
    }
    adapters
        .iter()
        .min_by_key(|a| match a.get_info().device_type {
            wgpu::DeviceType::DiscreteGpu => 0,
            wgpu::DeviceType::IntegratedGpu => 1,
            wgpu::DeviceType::VirtualGpu => 2,
            wgpu::DeviceType::Other => 3,
            wgpu::DeviceType::Cpu => 4,
        })
        .cloned()
        .ok_or_else(|| "no GPU adapter".to_string())
}

/// PCI vendor and device id of the adapter whose output shows the primary monitor.
#[cfg(windows)]
fn primary_display_adapter() -> Option<(u32, u32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
    unsafe {
        let primary = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            let mut j = 0;
            while let Ok(output) = adapter.EnumOutputs(j) {
                if output.GetDesc().is_ok_and(|d| d.Monitor == primary) {
                    let desc = adapter.GetDesc1().ok()?;
                    return Some((desc.VendorId, desc.DeviceId));
                }
                j += 1;
            }
            i += 1;
        }
    }
    None
}
