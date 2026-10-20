use crate::{formats, ChannelOrder, Frame, Result, SenderInfo, SpoutError};
use std::ffi::{c_void, CString};
use std::time::{Duration, Instant};
use windows::core::{Interface, HSTRING, PCSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Query, D3D11_QUERY_DESC, D3D11_QUERY_EVENT,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_RESOURCE_MISC_SHARED,
    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIKeyedMutex, IDXGIResource};
use windows::Win32::System::Memory::{
    CreateFileMappingA, MapViewOfFile, OpenFileMappingA, UnmapViewOfFile, VirtualQuery, FILE_MAP_ALL_ACCESS, MEMORY_BASIC_INFORMATION,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::Registry::{RegGetValueA, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Threading::{
    CreateMutexA, CreateSemaphoreA, OpenMutexW, OpenSemaphoreW, ReleaseMutex, ReleaseSemaphore, WaitForSingleObject, MUTEX_ALL_ACCESS, SEMAPHORE_ALL_ACCESS,
};

const NAME_LEN: usize = 256;
const INFO_SIZE: usize = 280;
const ACCESS_TIMEOUT_MS: u32 = 67;

fn platform(context: &str, e: impl std::fmt::Display) -> SpoutError {
    SpoutError::Platform(format!("{context}: {e}"))
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).expect("no interior NUL")
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

// Mutex and semaphore handles are process-wide kernel objects; moving them between threads is fine.
unsafe impl Send for OwnedHandle {}

fn open_mutex(name: &str) -> Option<OwnedHandle> {
    unsafe { OpenMutexW(MUTEX_ALL_ACCESS, false, &HSTRING::from(name)) }.ok().map(OwnedHandle)
}

fn create_mutex(name: &str) -> Result<OwnedHandle> {
    let c = cstring(name);
    unsafe { CreateMutexA(None, false, PCSTR(c.as_ptr() as *const u8)) }.map(OwnedHandle).map_err(|e| platform("CreateMutex", e))
}

fn open_semaphore(name: &str) -> Option<OwnedHandle> {
    unsafe { OpenSemaphoreW(SEMAPHORE_ALL_ACCESS, false, &HSTRING::from(name)) }.ok().map(OwnedHandle)
}

fn lock(handle: &OwnedHandle, ms: u32) -> bool {
    let r = unsafe { WaitForSingleObject(handle.0, ms) };
    r == WAIT_OBJECT_0 || r == WAIT_ABANDONED
}

fn unlock(handle: &OwnedHandle) {
    unsafe {
        let _ = ReleaseMutex(handle.0);
    }
}

/// A mapped view of a named file mapping (page-file backed).
struct SharedMemory {
    _mapping: OwnedHandle,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    size: usize,
}

unsafe impl Send for SharedMemory {}

impl SharedMemory {
    fn from_mapping(mapping: OwnedHandle) -> Option<Self> {
        let view = unsafe { MapViewOfFile(mapping.0, FILE_MAP_ALL_ACCESS, 0, 0, 0) };
        if view.Value.is_null() {
            return None;
        }
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let n = unsafe { VirtualQuery(Some(view.Value), &mut info, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) };
        let size = if n == 0 { 0 } else { info.RegionSize };
        Some(Self { _mapping: mapping, view, size })
    }

    fn open(name: &str) -> Option<Self> {
        let c = cstring(name);
        let h = unsafe { OpenFileMappingA(FILE_MAP_ALL_ACCESS.0, false, PCSTR(c.as_ptr() as *const u8)) }.ok()?;
        Self::from_mapping(OwnedHandle(h))
    }

    fn create(name: &str, size: usize) -> Result<Self> {
        let c = cstring(name);
        let h = unsafe { CreateFileMappingA(HANDLE(-1isize as *mut c_void), None, PAGE_READWRITE, 0, size as u32, PCSTR(c.as_ptr() as *const u8)) }
            .map_err(|e| platform("CreateFileMapping", e))?;
        Self::from_mapping(OwnedHandle(h)).ok_or_else(|| SpoutError::Platform(format!("MapViewOfFile failed for {name}")))
    }

    fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.view.Value as *const u8, self.size) }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.view.Value as *mut u8, self.size) }
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
        }
    }
}

fn max_senders() -> usize {
    let mut value: u32 = 0;
    let mut size: u32 = 4;
    let r = unsafe {
        RegGetValueA(
            HKEY_CURRENT_USER,
            windows::core::s!("Software\\Leading Edge\\Spout"),
            windows::core::s!("MaxSenders"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    if r.is_ok() && (1..=1024).contains(&value) {
        value as usize
    } else {
        64
    }
}

fn read_name(slot: &[u8]) -> String {
    let end = slot.iter().position(|&b| b == 0).unwrap_or(slot.len());
    String::from_utf8_lossy(&slot[..end]).into_owned()
}

fn read_names(bytes: &[u8], max: usize) -> Vec<String> {
    let mut names = Vec::new();
    for i in 0..max.min(bytes.len() / NAME_LEN) {
        let slot = &bytes[i * NAME_LEN..(i + 1) * NAME_LEN];
        if slot[0] == 0 {
            break;
        }
        names.push(read_name(slot));
    }
    names
}

fn write_names(bytes: &mut [u8], max: usize, names: &[String]) {
    let slots = max.min(bytes.len() / NAME_LEN);
    for i in 0..slots {
        let slot = &mut bytes[i * NAME_LEN..(i + 1) * NAME_LEN];
        slot.fill(0);
        if let Some(n) = names.get(i) {
            let b = n.as_bytes();
            let len = b.len().min(NAME_LEN - 1);
            slot[..len].copy_from_slice(&b[..len]);
        }
    }
}

/// Names of the Spout senders currently registered on this machine.
pub fn list_senders() -> Vec<String> {
    let Some(mem) = SharedMemory::open("SpoutSenderNames") else { return Vec::new() };
    let mutex = open_mutex("SpoutSenderNames_mutex");
    let locked = mutex.as_ref().map(|m| lock(m, ACCESS_TIMEOUT_MS)).unwrap_or(false);
    let names = read_names(mem.bytes(), max_senders());
    if locked {
        unlock(mutex.as_ref().unwrap());
    }
    names
}

/// The sender most recently made active (what Spout receivers pick by default).
pub fn active_sender() -> Option<String> {
    let mem = SharedMemory::open("ActiveSenderName")?;
    let name = read_name(&mem.bytes()[..NAME_LEN.min(mem.size)]);
    (!name.is_empty()).then_some(name)
}

/// Shared info block of a sender.
pub fn sender_info(name: &str) -> Option<SenderInfo> {
    let mem = SharedMemory::open(name)?;
    if mem.size < INFO_SIZE {
        return None;
    }
    let b = mem.bytes();
    let u = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    Some(SenderInfo { name: name.to_string(), share_handle: u(0), width: u(4), height: u(8), format: u(12), description: read_name(&b[20..20 + NAME_LEN]) })
}

fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    let levels: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|e| platform("D3D11CreateDevice", e))?;
    Ok((device.expect("device"), context.expect("context")))
}

fn half_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1F) as u32;
    let frac = (h & 0x3FF) as u32;
    let bits = if exp == 0 {
        if frac == 0 {
            sign << 31
        } else {
            let mut e: i32 = 127 - 15 + 1;
            let mut f = frac;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            (sign << 31) | ((e as u32) << 23) | ((f & 0x3FF) << 13)
        }
    } else if exp == 0x1F {
        (sign << 31) | 0x7F80_0000 | (frac << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13)
    };
    f32::from_bits(bits)
}

fn linear_to_srgb8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0 + 0.5) as u8
}

/// Reads frames from one Spout sender into CPU memory.
pub struct Receiver {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    info: Option<SenderInfo>,
    shared: Option<ID3D11Texture2D>,
    keyed: Option<IDXGIKeyedMutex>,
    staging: Option<ID3D11Texture2D>,
    access: Option<OwnedHandle>,
    counter: Option<OwnedHandle>,
    last_count: Option<i64>,
    /// Fingerprint of the last picture, for senders without a frame counter (VRChat's).
    last_fingerprint: Option<u64>,
    convert: Vec<u8>,
}

unsafe impl Send for Receiver {}

impl Receiver {
    pub fn new() -> Result<Self> {
        let (device, context) = create_device()?;
        Ok(Self { device, context, info: None, shared: None, keyed: None, staging: None, access: None, counter: None, last_count: None, last_fingerprint: None, convert: Vec::new() })
    }

    pub fn info(&self) -> Option<&SenderInfo> {
        self.info.as_ref()
    }

    pub fn disconnect(&mut self) {
        self.info = None;
        self.shared = None;
        self.keyed = None;
        self.staging = None;
        self.access = None;
        self.counter = None;
        self.last_count = None;
        self.last_fingerprint = None;
    }

    /// Opens a sender's shared texture.
    pub fn connect(&mut self, name: &str) -> Result<SenderInfo> {
        self.disconnect();
        let info = sender_info(name).ok_or_else(|| SpoutError::NotFound(name.to_string()))?;
        if formats::eight_bit_order(info.format).is_none() && info.format != formats::R16G16B16A16_FLOAT && info.format != 0 {
            return Err(SpoutError::UnsupportedFormat(info.format));
        }
        let handle = HANDLE(info.share_handle as usize as *mut c_void);
        let mut shared: Option<ID3D11Texture2D> = None;
        unsafe { self.device.OpenSharedResource(handle, &mut shared) }.map_err(|e| platform("OpenSharedResource", e))?;
        let shared = shared.ok_or_else(|| SpoutError::Platform("OpenSharedResource returned nothing".into()))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { shared.GetDesc(&mut desc) };
        let format = desc.Format.0 as u32;
        if formats::eight_bit_order(format).is_none() && format != formats::R16G16B16A16_FLOAT {
            return Err(SpoutError::UnsupportedFormat(format));
        }
        let keyed = if desc.MiscFlags & (D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32) != 0 { shared.cast::<IDXGIKeyedMutex>().ok() } else { None };
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
        unsafe { self.device.CreateTexture2D(&staging_desc, None, Some(&mut staging)) }.map_err(|e| platform("CreateTexture2D (staging)", e))?;
        let info = SenderInfo { width: desc.Width, height: desc.Height, format, ..info };
        self.access = open_mutex(&format!("{name}_SpoutAccessMutex"));
        self.counter = open_semaphore(&format!("{name}_Count_Semaphore"));
        self.shared = Some(shared);
        self.keyed = keyed;
        self.staging = staging;
        self.info = Some(info.clone());
        Ok(info)
    }

    /// New frame count when the sender signalled one since the last call (`Some(None)` when the sender has no
    /// counter: every poll counts as new), `None` when nothing new arrived.
    fn poll_new_frame(&mut self) -> Option<Option<i64>> {
        let Some(counter) = &self.counter else { return Some(None) };
        unsafe {
            let _ = WaitForSingleObject(counter.0, 0);
            let mut previous: i32 = 0;
            if ReleaseSemaphore(counter.0, 1, Some(&mut previous)).is_err() {
                return Some(None);
            }
            let count = previous as i64;
            match self.last_count {
                Some(last) if last == count => None,
                _ => {
                    self.last_count = Some(count);
                    Some(Some(count))
                }
            }
        }
    }

    /// Checks that the sender still exists with the same texture; reconnects when it was recreated.
    fn ensure_connected(&mut self) -> Result<()> {
        let Some(current) = self.info.clone() else { return Err(SpoutError::Platform("not connected".into())) };
        match sender_info(&current.name) {
            None => {
                self.disconnect();
                Err(SpoutError::NotFound(current.name))
            }
            Some(now) if now.share_handle != current.share_handle || now.width != current.width || now.height != current.height => {
                self.connect(&current.name).map(|_| ())
            }
            Some(_) => Ok(()),
        }
    }

    /// Waits up to `timeout` for a new frame and lends it to `f`. `Ok(None)` when no new frame arrived. A sender
    /// without a frame counter (VRChat's) is polled every few milliseconds and a picture counts as new when it
    /// differs from the last one.
    pub fn receive<R>(&mut self, timeout: Duration, f: impl FnOnce(Frame<'_>) -> R) -> Result<Option<R>> {
        self.ensure_connected()?;
        let deadline = Instant::now() + timeout;
        let (count, dst, mapped, info) = loop {
            let count = loop {
                if let Some(c) = self.poll_new_frame() {
                    break c;
                }
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                std::thread::sleep(Duration::from_millis(1));
            };
            let (Some(shared), Some(staging), Some(info)) = (self.shared.clone(), self.staging.clone(), self.info.clone()) else {
                return Ok(None);
            };
            let locked = match (&self.keyed, &self.access) {
                (Some(k), _) => unsafe { k.AcquireSync(0, ACCESS_TIMEOUT_MS) }.is_ok(),
                (None, Some(m)) => lock(m, ACCESS_TIMEOUT_MS),
                _ => true,
            };
            if !locked {
                return Ok(None);
            }
            let src: ID3D11Resource = shared.cast().map_err(|e| platform("cast", e))?;
            let dst: ID3D11Resource = staging.cast().map_err(|e| platform("cast", e))?;
            unsafe { self.context.CopyResource(&dst, &src) };
            match (&self.keyed, &self.access) {
                (Some(k), _) => unsafe {
                    let _ = k.ReleaseSync(0);
                },
                (None, Some(m)) => unlock(m),
                _ => {}
            }
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            unsafe { self.context.Map(&dst, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.map_err(|e| platform("Map", e))?;
            if count.is_some() {
                break (count, dst, mapped, info);
            }
            let raw = unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, mapped.RowPitch as usize * info.height as usize) };
            let bytes_per_pixel = if formats::eight_bit_order(info.format).is_some() { 4 } else { 8 };
            let print = fingerprint(raw, mapped.RowPitch as usize, info.width as usize, info.height as usize, bytes_per_pixel);
            if self.last_fingerprint != Some(print) {
                self.last_fingerprint = Some(print);
                break (count, dst, mapped, info);
            }
            unsafe { self.context.Unmap(&dst, 0) };
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let height = info.height as usize;
        let stride = mapped.RowPitch as usize;
        let raw = unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, stride * height) };
        let result = if let Some(order) = formats::eight_bit_order(info.format) {
            f(Frame { width: info.width, height: info.height, stride, data: raw, order, frame_count: count })
        } else {
            // 16-bit float (linear): convert to 8-bit sRGB codes, the form every decoder expects.
            let w = info.width as usize;
            self.convert.resize(w * height * 4, 0);
            for y in 0..height {
                let row = &raw[y * stride..y * stride + w * 8];
                let out = &mut self.convert[y * w * 4..(y + 1) * w * 4];
                for x in 0..w {
                    for c in 0..3 {
                        let h = u16::from_le_bytes([row[x * 8 + c * 2], row[x * 8 + c * 2 + 1]]);
                        out[x * 4 + c] = linear_to_srgb8(half_to_f32(h));
                    }
                    out[x * 4 + 3] = 255;
                }
            }
            f(Frame { width: info.width, height: info.height, stride: w * 4, data: &self.convert, order: ChannelOrder::Rgba, frame_count: count })
        };
        unsafe { self.context.Unmap(&dst, 0) };
        Ok(Some(result))
    }
}

/// A cheap fingerprint of a picture: every other pixel of every eighth row (FNV-1a over their bytes). Enough to tell
/// a new frame from the same one read again.
fn fingerprint(raw: &[u8], stride: usize, width: usize, height: usize, bytes_per_pixel: usize) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for y in (0..height).step_by(8) {
        let row = &raw[y * stride..y * stride + width * bytes_per_pixel];
        for px in row.chunks_exact(bytes_per_pixel).step_by(2) {
            for &b in px {
                h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    h
}

/// Publishes frames as a Spout sender (test patterns, replays).
pub struct Sender {
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    texture: ID3D11Texture2D,
    name: String,
    width: u32,
    height: u32,
    _info: SharedMemory,
    access: OwnedHandle,
    counter: OwnedHandle,
    bgra: Vec<u8>,
    // Named mappings disappear with their last handle: the sender keeps the shared lists alive while it runs.
    _registration: Registration,
}

unsafe impl Send for Sender {}

impl Sender {
    pub fn new(name: &str, width: u32, height: u32) -> Result<Self> {
        let (device, context) = create_device()?;
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT(formats::B8G8R8A8_UNORM as i32),
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
        };
        let mut texture = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }.map_err(|e| platform("CreateTexture2D (shared)", e))?;
        let texture: ID3D11Texture2D = texture.expect("texture");
        let resource: IDXGIResource = texture.cast().map_err(|e| platform("cast", e))?;
        let handle = unsafe { resource.GetSharedHandle() }.map_err(|e| platform("GetSharedHandle", e))?;

        let mut info = SharedMemory::create(name, INFO_SIZE)?;
        {
            let b = info.bytes_mut();
            b[..INFO_SIZE].fill(0);
            b[0..4].copy_from_slice(&(handle.0 as usize as u32).to_le_bytes());
            b[4..8].copy_from_slice(&width.to_le_bytes());
            b[8..12].copy_from_slice(&height.to_le_bytes());
            b[12..16].copy_from_slice(&formats::B8G8R8A8_UNORM.to_le_bytes());
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            let e = exe.as_bytes();
            let len = e.len().min(NAME_LEN - 1);
            b[20..20 + len].copy_from_slice(&e[..len]);
        }
        let access = create_mutex(&format!("{name}_SpoutAccessMutex"))?;
        let c = cstring(&format!("{name}_Count_Semaphore"));
        let counter =
            unsafe { CreateSemaphoreA(None, 1, i32::MAX, PCSTR(c.as_ptr() as *const u8)) }.map(OwnedHandle).map_err(|e| platform("CreateSemaphore", e))?;
        let registration = register_name(name, true)?.expect("registration when adding");
        Ok(Self {
            _device: device,
            context,
            texture,
            name: name.to_string(),
            width,
            height,
            _info: info,
            access,
            counter,
            bgra: Vec::new(),
            _registration: registration,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sends a tightly packed RGBA frame of the sender's size.
    /// Waits until the GPU has done the queued work: a receiver on another device copies the shared texture with no
    /// other synchronisation.
    fn finish_gpu(&self) {
        let mut query: Option<ID3D11Query> = None;
        let desc = D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 };
        if unsafe { self._device.CreateQuery(&desc, Some(&mut query)) }.is_err() {
            unsafe { self.context.Flush() };
            return;
        }
        let Some(query) = query else { return };
        unsafe {
            self.context.End(&query);
            self.context.Flush();
        }
        let start = Instant::now();
        let mut done: i32 = 0;
        while start.elapsed() < Duration::from_millis(100) {
            let hr = unsafe { self.context.GetData(&query, Some(&mut done as *mut i32 as *mut c_void), 4, 0) };
            if hr.is_ok() && done != 0 {
                return;
            }
            std::thread::yield_now();
        }
    }

    pub fn send_rgba(&mut self, rgba: &[u8]) -> Result<()> {
        let n = self.width as usize * self.height as usize * 4;
        if rgba.len() < n {
            return Err(SpoutError::Platform("frame smaller than the sender".into()));
        }
        self.bgra.resize(n, 0);
        for (dst, src) in self.bgra.as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        let resource: ID3D11Resource = self.texture.cast().map_err(|e| platform("cast", e))?;
        if lock(&self.access, ACCESS_TIMEOUT_MS) {
            unsafe {
                self.context.UpdateSubresource(&resource, 0, None, self.bgra.as_ptr() as *const c_void, self.width * 4, 0);
            }
            self.finish_gpu();
            unlock(&self.access);
        }
        unsafe {
            let _ = WaitForSingleObject(self.counter.0, 0);
            let _ = ReleaseSemaphore(self.counter.0, 2, None);
        }
        Ok(())
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        let _ = register_name(&self.name, false);
    }
}

/// Open handles on the shared sender list and active sender slot.
struct Registration {
    _names: SharedMemory,
    _active: Option<SharedMemory>,
}

/// Adds or removes a name in the shared sender list (and the active sender slot). When adding, the returned
/// registration must stay alive as long as the sender exists.
fn register_name(name: &str, add: bool) -> Result<Option<Registration>> {
    let max = max_senders();
    let mut mem = match SharedMemory::open("SpoutSenderNames") {
        Some(m) => m,
        None if add => SharedMemory::create("SpoutSenderNames", max * NAME_LEN)?,
        None => return Ok(None),
    };
    let mutex = create_mutex("SpoutSenderNames_mutex")?;
    let locked = lock(&mutex, ACCESS_TIMEOUT_MS);
    let mut names = read_names(mem.bytes(), max);
    if add {
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    } else {
        names.retain(|n| n != name);
    }
    write_names(mem.bytes_mut(), max, &names);
    if locked {
        unlock(&mutex);
    }

    let mut active = match SharedMemory::open("ActiveSenderName") {
        Some(m) => Some(m),
        None if add => Some(SharedMemory::create("ActiveSenderName", NAME_LEN)?),
        None => None,
    };
    if let Some(active) = active.as_mut() {
        let current = read_name(&active.bytes()[..NAME_LEN.min(active.size)]);
        let b = active.bytes_mut();
        if add && (current.is_empty() || !names.contains(&current)) {
            b[..NAME_LEN].fill(0);
            let n = name.as_bytes();
            b[..n.len().min(NAME_LEN - 1)].copy_from_slice(&n[..n.len().min(NAME_LEN - 1)]);
        } else if !add && current == name {
            b[..NAME_LEN].fill(0);
            if let Some(first) = names.first() {
                let n = first.as_bytes();
                b[..n.len().min(NAME_LEN - 1)].copy_from_slice(&n[..n.len().min(NAME_LEN - 1)]);
            }
        }
    }
    Ok(add.then_some(Registration { _names: mem, _active: active }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_to_receiver_round_trip() {
        let name = format!("OrbitersSpoutTest-{}", std::process::id());
        let (w, h) = (640u32, 360u32);
        let mut sender = Sender::new(&name, w, h).expect("sender");
        assert!(list_senders().contains(&name), "registered in the sender list");
        let info = sender_info(&name).expect("info");
        assert_eq!((info.width, info.height, info.format), (w, h, formats::B8G8R8A8_UNORM));

        let mut receiver = Receiver::new().expect("receiver");
        receiver.connect(&name).expect("connect");
        let mut frame = vec![0u8; (w * h * 4) as usize];
        for (i, px) in frame.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            px.copy_from_slice(&[(i % 251) as u8, 40, 200, 255]);
        }
        sender.send_rgba(&frame).unwrap();
        let got = receiver
            .receive(Duration::from_millis(500), |f| {
                assert_eq!(f.order, ChannelOrder::Bgra);
                let row0 = &f.data[..16];
                (row0.to_vec(), f.width, f.height)
            })
            .unwrap()
            .expect("a frame");
        assert_eq!((got.1, got.2), (w, h));
        // BGRA: blue first.
        assert_eq!(&got.0[..4], &[200, 40, 0, 255]);
        assert_eq!(&got.0[4..8], &[200, 40, 1, 255]);
        // Nothing new until the next send.
        assert!(receiver.receive(Duration::from_millis(20), |_| ()).unwrap().is_none());
        drop(sender);
        assert!(!list_senders().contains(&name), "unregistered on drop");
    }
    #[test]
    fn without_a_frame_counter_only_changed_pictures_are_new() {
        // VRChat's sender has no frame counter: the receiver must not take the same picture again and again.
        let name = format!("OrbitersSpoutNoCounter-{}", std::process::id());
        let (w, h) = (320u32, 180u32);
        let mut sender = Sender::new(&name, w, h).expect("sender");
        let mut receiver = Receiver::new().expect("receiver");
        receiver.connect(&name).expect("connect");
        receiver.counter = None;
        let mut frame = vec![90u8; (w * h * 4) as usize];
        sender.send_rgba(&frame).unwrap();
        assert!(receiver.receive(Duration::from_millis(200), |_| ()).unwrap().is_some());
        // The first copy may land before the sender's write does: give it two reads to settle.
        let settled = (0..3).any(|_| receiver.receive(Duration::from_millis(50), |_| ()).unwrap().is_none());
        assert!(settled, "the same picture keeps coming back as new");
        assert!(receiver.receive(Duration::from_millis(50), |_| ()).unwrap().is_none(), "the same picture again");
        frame[..4].copy_from_slice(&[10, 20, 30, 255]);
        sender.send_rgba(&frame).unwrap();
        assert!(receiver.receive(Duration::from_millis(200), |_| ()).unwrap().is_some(), "a changed picture");
    }
}
