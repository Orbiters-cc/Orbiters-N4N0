//! Overlay pictures as D3D11 textures. `SetOverlayRaw` uploads a new texture on every call and the overlay
//! flickers when it is called every frame; a texture the overlay keeps (two per overlay, used in turn so SteamVR
//! never reads the one being written) does not. One small device on the headset's GPU, no swap chain.

use crate::{Overlay, OverlayHandle};
use std::collections::HashMap;
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1};

/// The name of DXGI adapter `adapter` (the first one when unknown), as Windows reports it ("NVIDIA GeForce RTX 4080").
pub fn adapter_name(adapter: Option<u32>) -> Option<String> {
    // SAFETY: plain DXGI factory and adapter queries.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    let desc = unsafe { factory.EnumAdapters1(adapter.unwrap_or(0)).ok()?.GetDesc1().ok()? };
    let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
    let name = String::from_utf16_lossy(&desc.Description[..end]).trim().to_string();
    (!name.is_empty()).then_some(name)
}

struct Slot {
    textures: [ID3D11Texture2D; 2],
    size: (u32, u32),
    next: usize,
}

pub struct OverlayTextures {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    slots: HashMap<OverlayHandle, Slot>,
}

impl OverlayTextures {
    /// A device on DXGI adapter `adapter` (the headset's, from [`crate::Client::dxgi_adapter_index`]), or on the
    /// default one when that adapter is unknown.
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
        Ok(Self { device: device.ok_or("no device")?, context: context.ok_or("no context")?, slots: HashMap::new() })
    }

    fn texture(&self, width: u32, height: u32) -> Result<ID3D11Texture2D, String> {
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
        let mut t = None;
        // SAFETY: valid description and out parameter.
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut t)) }.map_err(|e| format!("CreateTexture2D: {e}"))?;
        t.ok_or_else(|| "no texture".into())
    }

    /// Shows RGBA pixels (top row first, gamma-encoded) on overlay `handle`.
    pub fn present(&mut self, overlay: &Overlay, handle: OverlayHandle, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
        assert_eq!(rgba.len(), (width * height * 4) as usize);
        if self.slots.get(&handle).is_none_or(|s| s.size != (width, height)) {
            let textures = [self.texture(width, height)?, self.texture(width, height)?];
            self.slots.insert(handle, Slot { textures, size: (width, height), next: 0 });
        }
        let slot = self.slots.get_mut(&handle).expect("inserted above");
        let texture = slot.textures[slot.next].clone();
        slot.next ^= 1;
        // SAFETY: the buffer holds width × height RGBA pixels; the texture matches that size.
        unsafe {
            self.context.UpdateSubresource(&texture, 0, None, rgba.as_ptr().cast(), width * 4, 0);
            self.context.Flush();
        }
        overlay.set_texture_d3d11(handle, texture.as_raw()).map_err(|code| format!("SetOverlayTexture: SteamVR overlay error {code}"))
    }

    /// Forgets the textures of an overlay that was destroyed.
    pub fn forget(&mut self, handle: OverlayHandle) {
        self.slots.remove(&handle);
    }
}
