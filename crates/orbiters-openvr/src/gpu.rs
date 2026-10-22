//! Overlay pictures as D3D11 textures. `SetOverlayRaw` uploads a new texture on every call and the overlay
//! flickers when it is called every frame; a texture the overlay keeps (two per overlay, used in turn so SteamVR
//! never reads the one being written) does not. One small device on the headset's GPU, no swap chain. Only the part
//! of a picture that changed is uploaded.

use crate::{Overlay, OverlayHandle};
use std::collections::HashMap;
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION,
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
    behind: Behind,
}

/// Why a picture could not be shown through a texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresentError {
    /// The device is gone (a driver reset or update, a GPU removed): a new one may work at once.
    DeviceLost(String),
    /// A texture could not be made, or SteamVR refused it: trying again soon is likely to fail the same way.
    Refused(String),
}

impl std::fmt::Display for PresentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresentError::DeviceLost(e) | PresentError::Refused(e) => f.write_str(e),
        }
    }
}

/// What each of an overlay's two textures lacks of the newest picture (x0, y0, x1, y1): the changes made since it
/// was last written, as the other one was shown meanwhile.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Behind([Option<[u32; 4]>; 2]);

impl Behind {
    /// Two new textures: both lack the whole picture.
    fn whole(width: u32, height: u32) -> Self {
        Self([Some([0, 0, width, height]); 2])
    }

    /// The picture changed in `changed`; texture `i` is written now: what to send it.
    fn take(&mut self, i: usize, changed: [u32; 4]) -> Option<[u32; 4]> {
        if changed[0] < changed[2] && changed[1] < changed[3] {
            for part in &mut self.0 {
                *part = Some(match *part {
                    None => changed,
                    Some(p) => [p[0].min(changed[0]), p[1].min(changed[1]), p[2].max(changed[2]), p[3].max(changed[3])],
                });
            }
        }
        self.0[i].take()
    }
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
    pub fn present(&mut self, overlay: &Overlay, handle: OverlayHandle, rgba: &[u8], width: u32, height: u32) -> Result<(), PresentError> {
        self.present_part(overlay, handle, rgba, width, height, [0, 0, width, height])
    }

    /// Shows RGBA pixels on overlay `handle` where only `changed` (x0, y0, x1, y1) differs from the last picture
    /// shown there: only that part is uploaded, with what the texture written now missed of the one before.
    pub fn present_part(
        &mut self,
        overlay: &Overlay,
        handle: OverlayHandle,
        rgba: &[u8],
        width: u32,
        height: u32,
        changed: [u32; 4],
    ) -> Result<(), PresentError> {
        assert_eq!(rgba.len(), (width * height * 4) as usize);
        // SAFETY: a plain query of the device.
        if let Err(e) = unsafe { self.device.GetDeviceRemovedReason() } {
            return Err(PresentError::DeviceLost(format!("the overlay's graphics device was lost: {e}")));
        }
        if self.slots.get(&handle).is_none_or(|s| s.size != (width, height)) {
            let textures = [self.texture(width, height).map_err(PresentError::Refused)?, self.texture(width, height).map_err(PresentError::Refused)?];
            self.slots.insert(handle, Slot { textures, size: (width, height), next: 0, behind: Behind::whole(width, height) });
        }
        let changed = [changed[0].min(width), changed[1].min(height), changed[2].min(width), changed[3].min(height)];
        let slot = self.slots.get_mut(&handle).expect("inserted above");
        let i = slot.next;
        let texture = slot.textures[i].clone();
        if let Some([x0, y0, x1, y1]) = slot.behind.take(i, changed) {
            let part = D3D11_BOX { left: x0, top: y0, front: 0, right: x1, bottom: y1, back: 1 };
            let first = (y0 as usize * width as usize + x0 as usize) * 4;
            // SAFETY: the box lies inside the texture, and the buffer from its first pixel holds its rows at a
            // pitch of `width` pixels.
            unsafe {
                self.context.UpdateSubresource(&texture, 0, Some(std::ptr::from_ref(&part)), rgba[first..].as_ptr().cast(), width * 4, 0);
                self.context.Flush();
            }
        }
        slot.next ^= 1;
        overlay.set_texture_d3d11(handle, texture.as_raw()).map_err(|code| PresentError::Refused(format!("SetOverlayTexture: SteamVR overlay error {code}")))
    }

    /// Forgets the textures of an overlay that was destroyed.
    pub fn forget(&mut self, handle: OverlayHandle) {
        self.slots.remove(&handle);
    }
}

#[cfg(test)]
mod tests {
    use super::Behind;

    #[test]
    fn each_texture_gets_what_it_missed() {
        let mut b = Behind::whole(100, 50);
        // New textures are sent whole, the first one now and the second at its turn.
        assert_eq!(b.take(0, [10, 10, 20, 20]), Some([0, 0, 100, 50]));
        assert_eq!(b.take(1, [30, 5, 40, 8]), Some([0, 0, 100, 50]));
        // Texture 0 missed both changes since: their union.
        assert_eq!(b.take(0, [12, 12, 14, 14]), Some([12, 5, 40, 14]));
        // Texture 1 missed the last two.
        assert_eq!(b.take(1, [0, 40, 2, 42]), Some([0, 12, 14, 42]));
        // No change now: texture 0 still lacks the one before, texture 1 lacks nothing.
        assert_eq!(b.take(0, [50, 0, 50, 10]), Some([0, 40, 2, 42]));
        assert_eq!(b.take(1, [0, 0, 0, 0]), None);
    }
}
