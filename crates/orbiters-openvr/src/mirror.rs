//! The headset's view as SteamVR's compositor has it (`GetMirrorTextureD3D11`, as cnlohr's openvr-screengrab and Haï's
//! h-view read it): one eye's picture of the scene application, read back to the CPU. For Orb's phone camera: a picture
//! of the world as its owner sees it, without VRChat's camera.

use crate::compositor::{Compositor, Eye};
use std::ffi::c_void;
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11ShaderResourceView, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_FLAG, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1};

/// An eye's picture: RGBA, gamma-encoded, top row first.
pub struct EyePicture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Reads eyes' pictures through one small D3D11 device on the headset's GPU (DXGI adapter `adapter`, the default one
/// when unknown).
pub struct EyeReader {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}

impl EyeReader {
    pub fn new(adapter: Option<u32>) -> Result<Self, String> {
        let chosen: Option<IDXGIAdapter> = adapter.and_then(|i| {
            // SAFETY: plain DXGI factory and adapter queries.
            let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
            unsafe { factory.EnumAdapters1(i) }.ok().and_then(|a| a.cast().ok())
        });
        let levels = [D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_10_0];
        let (mut device, mut context) = (None, None);
        let driver = if chosen.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { D3D_DRIVER_TYPE_HARDWARE };
        // SAFETY: out parameters are valid; the adapter (if any) lives through the call.
        unsafe {
            D3D11CreateDevice(
                chosen.as_ref(),
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|e| format!("D3D11CreateDevice: {e}"))?;
        Ok(Self { device: device.ok_or("no device")?, context: context.ok_or("no context")? })
    }

    /// `eye`'s picture now, copied whole and read back (a few milliseconds for a 2k × 2k eye).
    pub fn read(&self, compositor: &Compositor, eye: Eye) -> Result<EyePicture, String> {
        // SAFETY: this reader's live device; the view is released once copied, and not used after.
        let raw = unsafe { compositor.mirror_d3d11(eye, self.device.as_raw()) }.map_err(|e| format!("GetMirrorTextureD3D11: compositor error {e}"))?;
        let picture = self.copy(&raw);
        unsafe { compositor.release_mirror_d3d11(raw) };
        picture
    }

    fn copy(&self, raw: &*mut c_void) -> Result<EyePicture, String> {
        // SAFETY: SteamVR's view, alive until it is released after this call; borrowed, never released by us.
        let view = unsafe { ID3D11ShaderResourceView::from_raw_borrowed(raw) }.ok_or("no mirror view")?;
        // SAFETY: a valid view.
        let resource: ID3D11Resource = unsafe { view.GetResource() }.map_err(|e| format!("mirror texture: {e}"))?;
        let texture: ID3D11Texture2D = resource.cast().map_err(|e| format!("mirror texture: {e}"))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: valid texture and out parameter.
        unsafe { texture.GetDesc(&mut desc) };
        let swap = match desc.Format {
            DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => false,
            DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => true,
            other => return Err(format!("the mirror texture's format {} is not 8-bit RGBA", other.0)),
        };
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.Width,
            Height: desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: desc.Format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut staging = None;
        // SAFETY: valid description and out parameter.
        unsafe { self.device.CreateTexture2D(&staging_desc, None, Some(&mut staging)) }.map_err(|e| format!("CreateTexture2D: {e}"))?;
        let staging = staging.ok_or("no staging texture")?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: both textures are this device's, of the same size and format; Map waits for the copy.
        unsafe {
            self.context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture, 0, None);
            self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|e| format!("Map: {e}"))?;
        let (w, h) = (desc.Width as usize, desc.Height as usize);
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            // SAFETY: the mapped rows are RowPitch bytes apart, each at least w × 4 bytes long.
            let row = unsafe { std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), w * 4) };
            let out = &mut rgba[y * w * 4..(y + 1) * w * 4];
            out.copy_from_slice(row);
            if swap {
                out.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p.swap(0, 2));
            }
        }
        // SAFETY: mapped above.
        unsafe { self.context.Unmap(&staging, 0) };
        Ok(EyePicture { width: desc.Width, height: desc.Height, rgba })
    }
}
