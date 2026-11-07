//! The cube's pictures go from the render device (wgpu on D3D12) to SteamVR without leaving the GPU: each face texture
//! is a shared D3D12 resource (simultaneous access: it is back in the common state after each submission, as another
//! API needs it) that a small D3D11 device on the same adapter opens and hands to SteamVR (`SetOverlayTexture`, the
//! D3D11 texture type every overlay of the app uses). The render thread only writes a texture SteamVR no longer shows
//! (each face has two, used in turn) and waits, on the GPU, for a shared fence the D3D11 side signals after handing a
//! texture over (SteamVR copies it on that device's context), so it never overwrites one being read.

use anyhow::{anyhow, Context as _};
use orbiters_i18n::tr;
use eframe::wgpu;
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, LUID};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11Device1, ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Direct3D12::{
    ID3D12Device, ID3D12Fence, ID3D12Resource, D3D12_CPU_PAGE_PROPERTY_UNKNOWN, D3D12_FENCE_FLAG_SHARED, D3D12_HEAP_FLAG_SHARED, D3D12_HEAP_PROPERTIES,
    D3D12_HEAP_TYPE_DEFAULT, D3D12_MEMORY_POOL_UNKNOWN, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS, D3D12_RESOURCE_STATE_COMMON, D3D12_TEXTURE_LAYOUT_UNKNOWN,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory4};

/// `GENERIC_ALL`: what a shared handle lets its opener do.
const SHARED_ACCESS: u32 = 0x1000_0000;

/// A shared NT handle's value, to send to the thread that opens it (which closes it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shared(pub usize);

impl Shared {
    fn of(h: HANDLE) -> Self {
        Self(h.0 as usize)
    }

    fn handle(self) -> HANDLE {
        HANDLE(self.0 as *mut _)
    }
}

/// The render thread's side: the face textures as wgpu textures, and the fence it waits on.
pub struct RenderSide {
    pub textures: Vec<wgpu::Texture>,
    fence: ID3D12Fence,
}

/// What the D3D11 side opens: the textures' and the fence's handles, and the textures' width per eye.
#[derive(Clone, Debug)]
pub struct Handles {
    pub textures: Vec<Shared>,
    pub fence: Shared,
    pub width: u32,
}

fn raw_device(device: &wgpu::Device) -> anyhow::Result<ID3D12Device> {
    // SAFETY: the D3D12 device behind a DX12 wgpu device; only used to create shared resources and a fence.
    unsafe { device.as_hal::<wgpu::hal::api::Dx12>().map(|d| d.raw_device().clone()).with_context(|| tr("the 3D worlds device is not a D3D12 one")) }
}

impl RenderSide {
    /// `count` face textures (twice `width` across, `width` high) and a fence, all shared.
    pub fn new(device: &wgpu::Device, count: usize, width: u32) -> anyhow::Result<(Self, Handles)> {
        let d3d12 = raw_device(device)?;
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
            MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
            CreationNodeMask: 0,
            VisibleNodeMask: 0,
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: 2 * width as u64,
            Height: width,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS,
        };
        let mut textures = Vec::with_capacity(count);
        let mut handles = Vec::with_capacity(count);
        for _ in 0..count {
            let mut resource: Option<ID3D12Resource> = None;
            // SAFETY: valid descriptions; the out parameter is an Option of the asked interface.
            unsafe { d3d12.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_SHARED, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut resource) }
                .map_err(|e| anyhow!("{}", tr!("a shared face texture: {error}", error = e)))?;
            let resource = resource.with_context(|| tr("no shared face texture"))?;
            // SAFETY: a shared resource of this device; the handle goes to the D3D11 side, which closes it.
            let handle = unsafe { d3d12.CreateSharedHandle(&resource, None, SHARED_ACCESS, PCWSTR::null()) }.map_err(|e| anyhow!("{}", tr!("sharing a face texture: {error}", error = e)))?;
            handles.push(Shared::of(handle));
            let size = wgpu::Extent3d { width: 2 * width, height: width, depth_or_array_layers: 1 };
            // SAFETY: the resource matches the description given to wgpu (format, size, one mip, one sample). wgpu is told
            // it is a copy destination already: it is only ever copied into, and a simultaneous-access texture is promoted
            // to that state from the common one it decays to after each submission, so no barrier is needed.
            let texture = unsafe {
                let hal = wgpu::hal::dx12::Device::texture_from_raw(resource, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureDimension::D2, size, 1, 1);
                device.create_texture_from_hal::<wgpu::hal::api::Dx12>(
                    hal,
                    &wgpu::TextureDescriptor {
                        label: Some("3d-worlds shared face"), // i18n: not shown (a GPU debug label)
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    },
                    wgpu::TextureUses::COPY_DST,
                )
            };
            textures.push(texture);
        }
        // SAFETY: a plain fence, shared.
        let fence: ID3D12Fence = unsafe { d3d12.CreateFence(0, D3D12_FENCE_FLAG_SHARED) }.map_err(|e| anyhow!("{}", tr!("a shared fence: {error}", error = e)))?;
        // SAFETY: as the textures' handles.
        let fence_handle = unsafe { d3d12.CreateSharedHandle(&fence, None, SHARED_ACCESS, PCWSTR::null()) }.map_err(|e| anyhow!("{}", tr!("sharing a fence: {error}", error = e)))?;
        Ok((Self { textures, fence }, Handles { textures: handles, fence: Shared::of(fence_handle), width }))
    }

    /// The GPU waits, before what is submitted next on `queue`, until the D3D11 side signalled `value`.
    pub fn wait(&self, queue: &wgpu::Queue, value: u64) -> anyhow::Result<()> {
        if value == 0 {
            return Ok(());
        }
        // SAFETY: the D3D12 queue behind the wgpu queue, used from the thread that submits to it.
        unsafe {
            let q = queue.as_hal::<wgpu::hal::api::Dx12>().with_context(|| tr("the 3D worlds queue is not a D3D12 one"))?;
            q.as_raw().Wait(&self.fence, value).map_err(|e| anyhow!("{}", tr!("waiting on the shared fence: {error}", error = e)))
        }
    }
}

/// The D3D11 side, on the overlay's thread: the device SteamVR is handed textures from.
pub struct OverlaySide {
    device: ID3D11Device5,
    context: ID3D11DeviceContext4,
    pub textures: Vec<ID3D11Texture2D>,
    fence: Option<ID3D11Fence>,
    signalled: u64,
    pub width: u32,
}

/// The DXGI adapter at `index` (the headset's, `GetDXGIOutputInfo`), as a LUID number like [`super::render::adapter_luid`].
pub fn luid_of_index(index: u32) -> Option<u64> {
    // SAFETY: plain DXGI factory and adapter queries.
    unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory1().ok()?;
        let desc = factory.EnumAdapters1(index).ok()?.GetDesc1().ok()?;
        Some(((desc.AdapterLuid.HighPart as u32 as u64) << 32) | desc.AdapterLuid.LowPart as u64)
    }
}

impl OverlaySide {
    /// A device on the adapter with LUID `luid`.
    pub fn new(luid: u64) -> anyhow::Result<Self> {
        // SAFETY: plain DXGI and D3D11 creation; out parameters are valid.
        unsafe {
            let factory: IDXGIFactory4 = CreateDXGIFactory1().map_err(|e| anyhow!("DXGI: {e}"))?;
            let adapter: IDXGIAdapter =
                factory.EnumAdapterByLuid(LUID { LowPart: luid as u32, HighPart: (luid >> 32) as i32 }).map_err(|e| anyhow!("{}", tr!("the headset's adapter: {error}", error = e)))?;
            let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
            let (mut device, mut context): (Option<ID3D11Device>, Option<ID3D11DeviceContext>) = (None, None);
            D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), Some(&levels), D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))
                .map_err(|e| anyhow!("D3D11CreateDevice: {e}"))?;
            let device: ID3D11Device5 = device.with_context(|| tr("no device"))?.cast().map_err(|e| anyhow!("{}", tr!("a D3D11.4 device: {error}", error = e)))?;
            let context: ID3D11DeviceContext4 = context.with_context(|| tr("no context"))?.cast().map_err(|e| anyhow!("{}", tr!("a D3D11.3 context: {error}", error = e)))?;
            Ok(Self { device, context, textures: Vec::new(), fence: None, signalled: 0, width: 0 })
        }
    }

    /// Opens the render side's textures and fence (closing their handles), in place of any opened before.
    pub fn open(&mut self, handles: &Handles) -> anyhow::Result<()> {
        let device1: ID3D11Device1 = self.device.cast().map_err(|e| anyhow!("{e}"))?;
        let mut textures = Vec::with_capacity(handles.textures.len());
        let mut failed = None;
        for h in &handles.textures {
            // SAFETY: a handle the render side made for this adapter; closed once opened.
            match unsafe { device1.OpenSharedResource1::<ID3D11Texture2D>(h.handle()) } {
                Ok(t) => textures.push(t),
                Err(e) => failed = failed.or(Some(anyhow!("{}", tr!("opening a face texture: {error}", error = e)))),
            }
            // SAFETY: the handle is ours to close.
            unsafe {
                let _ = CloseHandle(h.handle());
            }
        }
        let mut fence: Option<ID3D11Fence> = None;
        // SAFETY: as above.
        let opened = unsafe { self.device.OpenSharedFence(handles.fence.handle(), &mut fence) };
        unsafe {
            let _ = CloseHandle(handles.fence.handle());
        }
        if let Some(e) = failed {
            return Err(e);
        }
        opened.map_err(|e| anyhow!("{}", tr!("opening the shared fence: {error}", error = e)))?;
        self.textures = textures;
        self.fence = fence;
        self.signalled = 0;
        self.width = handles.width;
        Ok(())
    }

    /// After textures were handed to SteamVR: signals the next fence value once the context reaches it (SteamVR's copies
    /// before it), flushed; the value the render side waits for before writing those faces' other textures again.
    pub fn handed_over(&mut self) -> u64 {
        let Some(fence) = &self.fence else { return 0 };
        self.signalled += 1;
        // SAFETY: a fence of this device; the context is used on this thread only.
        unsafe {
            if self.context.Signal(fence, self.signalled).is_err() {
                self.signalled -= 1;
            }
            self.context.Flush();
        }
        self.signalled
    }

    /// Texture `i`'s pixels as the D3D11 side sees them (RGBA8, rows packed): what SteamVR would get (the offline check).
    pub fn read(&self, i: usize) -> anyhow::Result<Vec<u8>> {
        let texture = self.textures.get(i).with_context(|| tr("no such texture"))?;
        // SAFETY: a staging copy of a texture of this device, mapped for reading on this thread.
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut staging)).map_err(|e| anyhow!("{}", tr!("a staging texture: {error}", error = e)))?;
            let staging: ID3D11Texture2D = staging.with_context(|| tr("no staging texture"))?;
            self.context.CopyResource(&staging, texture);
            let mut mapped = Default::default();
            self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(|e| anyhow!("mapping: {e}"))?;
            let mut out = Vec::with_capacity((desc.Width * desc.Height * 4) as usize);
            for y in 0..desc.Height as usize {
                let row = std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), desc.Width as usize * 4);
                out.extend_from_slice(row);
            }
            self.context.Unmap(&staging, 0);
            Ok(out)
        }
    }

    /// The raw texture pointer of texture `i`, for `SetOverlayTexture`.
    pub fn raw(&self, i: usize) -> Option<*mut std::ffi::c_void> {
        self.textures.get(i).map(|t| t.as_raw())
    }
}
