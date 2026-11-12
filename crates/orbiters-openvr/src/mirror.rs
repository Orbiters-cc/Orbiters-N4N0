//! The headset's view as SteamVR's compositor has it (`GetMirrorTextureD3D11`, as cnlohr's openvr-screengrab and Haï's
//! h-view read it): one eye's picture of the scene application, read back to the CPU. For N4N0's phone camera: a picture
//! of the world as its owner sees it, without VRChat's camera.

use crate::compositor::{Compositor, Eye, MirrorSource};
use std::ffi::c_void;
use std::cell::RefCell;
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11ShaderResourceView, ID3D11Texture2D, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_FLAG, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_TYPELESS, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB, DXGI_FORMAT_R8G8B8A8_TYPELESS, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1};

/// An eye's picture: RGBA, gamma-encoded, top row first.
pub struct EyePicture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Where this lies in the whole eye's picture, and how large that is.
    pub from: [u32; 2],
    pub whole: [u32; 2],
}

/// Reads eyes' pictures through one small D3D11 device on the headset's GPU (DXGI adapter `adapter`, the default one
/// when unknown).
pub struct EyeReader {
    mirrors: RefCell<MirrorViews>,
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
        Ok(Self { mirrors:RefCell::new(MirrorViews::default()),device: device.ok_or("no device")?, context: context.ok_or("no context")? }) // i18n: not shown (a graphics call named in an error detail)
    }

    /// `eye`'s picture now, copied whole and read back (a few milliseconds for a 2k × 2k eye).
    pub fn read(&self, compositor: &Compositor, eye: Eye) -> Result<EyePicture, String> {
        self.read_part(compositor.mirror_source(), eye, None)
    }

    /// The same, of only `area` (x, y, width, height) if given, as a look at something whose place is known needs.
    pub fn read_part(&self, source: MirrorSource, eye: Eye, area: Option<[u32; 4]>) -> Result<EyePicture, String> {
        // SAFETY: this reader's live device; the view is released once copied, and not used after.
        let raw=self.mirror(source,eye)?;
        self.copy(&raw,area)
    }

    fn copy(&self, raw: &*mut c_void, area: Option<[u32; 4]>) -> Result<EyePicture, String> {
        let queued=self.stage(raw,area)?;
        self.finish(queued)
    }

    /// Queue both eyes before either CPU Map waits on the GPU. Sequential
    /// read_part calls can straddle a compositor update on every attempt.
    /// The index brackets submission of the copies, not their later readback.
    pub fn read_pair(&self,source:MirrorSource,areas:[Option<[u32;4]>;2]) -> Result<([EyePicture;2],Option<u32>),String> {
        let before=source.frame_index();
        let left=self.mirror(source,Eye::Left)?;
        let right=self.mirror(source,Eye::Right)?;
        let staged=self.stage(&left,areas[0]).and_then(|left|self.stage(&right,areas[1]).map(|right|[left,right]));
        let after=source.frame_index();
        let [left,right]=staged?;
        Ok(([self.finish(left)?,self.finish(right)?],before.filter(|_|before==after)))
    }

    // SteamVR fills a newly requested mirror on subsequent compositor frames.
    // Keep each stream alive; releasing it after every read repeatedly tears down
    // acquisition. This reader must be dropped before its source Client dies.
    fn mirror(&self,source:MirrorSource,eye:Eye) -> Result<*mut c_void,String> {
        self.mirrors.borrow_mut().get(source,eye,||unsafe {source.mirror_d3d11(eye,self.device.as_raw())}.map_err(|e|format!("GetMirrorTextureD3D11: compositor error {e}"))) // i18n: not shown (graphics API diagnostics)
    }

    fn stage(&self, raw: &*mut c_void, area: Option<[u32; 4]>) -> Result<QueuedEye, String> {
        // SAFETY: SteamVR's view, alive until it is released after this call; borrowed, never released by us.
        let view = unsafe { ID3D11ShaderResourceView::from_raw_borrowed(raw) }.ok_or("no mirror view")?; // i18n: not shown (a graphics call named in an error detail)
        // SAFETY: a valid view.
        let resource: ID3D11Resource = unsafe { view.GetResource() }.map_err(|e| format!("mirror texture: {e}"))?; // i18n: not shown (a graphics call named in an error detail)
        let texture: ID3D11Texture2D = resource.cast().map_err(|e| format!("mirror texture: {e}"))?; // i18n: not shown (a graphics call named in an error detail)
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: valid texture and out parameter.
        unsafe { texture.GetDesc(&mut desc) };
        let swap = match desc.Format {
            // Typeless is how SteamVR hands it over on some headsets (on Enzo's, through Virtual Desktop): the same bytes.
            DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_R8G8B8A8_TYPELESS => false,
            DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_TYPELESS => true,
            other => return Err(orbiters_i18n::tr!("the mirror texture's format {format} is not 8-bit RGBA", format = other.0)),
        };
        let [x0, y0, part_w, part_h] = area.unwrap_or([0, 0, desc.Width, desc.Height]);
        let (x0, y0) = (x0.min(desc.Width.saturating_sub(1)), y0.min(desc.Height.saturating_sub(1)));
        let (part_w, part_h) = (part_w.clamp(1, desc.Width - x0), part_h.clamp(1, desc.Height - y0));
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: part_w,
            Height: part_h,
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
        let staging = staging.ok_or("no staging texture")?; // i18n: not shown (a graphics call named in an error detail)
        let part = D3D11_BOX { left: x0, top: y0, front: 0, right: x0 + part_w, bottom: y0 + part_h, back: 1 };
        // SAFETY: both textures are this device's, of one format, and the box lies within the source and fits the copy; Map waits for the copy.
        unsafe {
            self.context.CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture, 0, Some(&part));
        }
        Ok(QueuedEye {staging,from:[x0,y0],size:[part_w,part_h],whole:[desc.Width,desc.Height],swap})
    }

    fn finish(&self,queued:QueuedEye) -> Result<EyePicture,String> {
        let QueuedEye {staging,from:[x0,y0],size:[part_w,part_h],whole,swap}=queued;
        let mut mapped=D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {self.context.Map(&staging,0,D3D11_MAP_READ,0,Some(&mut mapped))}.map_err(|e|format!("Map: {e}"))?; // i18n: not shown (a graphics call named in an error detail)
        let (w, h) = (part_w as usize, part_h as usize);
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
        Ok(EyePicture { width: part_w, height: part_h, rgba, from: [x0, y0], whole })
    }
}

struct QueuedEye {staging:ID3D11Texture2D,from:[u32;2],size:[u32;2],whole:[u32;2],swap:bool}

struct MirrorView {source:MirrorSource,raw:*mut c_void}
impl Drop for MirrorView {
    fn drop(&mut self) {unsafe {self.source.release_mirror_d3d11(self.raw)};}
}

#[derive(Default)]
struct MirrorViews([Option<MirrorView>;2]);
impl MirrorViews {
    fn get(&mut self,source:MirrorSource,eye:Eye,acquire:impl FnOnce()->Result<*mut c_void,String>) -> Result<*mut c_void,String> {
        let slot=&mut self.0[eye as usize];
        if slot.is_none() {*slot=Some(MirrorView {source,raw:acquire()?});}
        Ok(slot.as_ref().unwrap().raw)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize,Ordering};
    static RELEASES:AtomicUsize=AtomicUsize::new(0);
    #[test]
    fn eye_mirror_streams_survive_reads_and_release_before_client_teardown() {
        unsafe extern "system" fn release(raw:*mut c_void) {assert!([1,2].contains(&(raw as usize)));RELEASES.fetch_add(1,Ordering::SeqCst);}
        let mut table=[0usize;37];table[36]=release as *const () as usize;
        let source=MirrorSource::test_table(table.as_ptr());
        let acquisitions=AtomicUsize::new(0);RELEASES.store(0,Ordering::SeqCst);
        {
            let mut mirrors=MirrorViews::default();
            for _ in 0..200 {for eye in [Eye::Left,Eye::Right] {
                let raw=mirrors.get(source,eye,||{acquisitions.fetch_add(1,Ordering::SeqCst);Ok((eye as usize+1) as *mut c_void)}).unwrap();
                assert_eq!(raw as usize,eye as usize+1);
            }}
            assert_eq!(acquisitions.load(Ordering::SeqCst),2,"one continuous SteamVR stream per eye, not a new stream per look");
            assert_eq!(RELEASES.load(Ordering::SeqCst),0,"do not end either stream between frames");
        }
        assert_eq!(RELEASES.load(Ordering::SeqCst),2,"release both while the borrowed compositor table is alive");
    }
}
