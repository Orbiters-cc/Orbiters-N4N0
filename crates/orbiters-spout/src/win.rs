use crate::{formats, ChannelOrder, Frame, Result, SenderInfo, SpoutError};
use std::ffi::c_void;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use windows::core::{Interface, HSTRING, PCSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HMODULE, WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Query, ID3D11Resource, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_FLAG_DO_NOT_WAIT, D3D11_MAP_READ, D3D11_QUERY_DESC,
    D3D11_QUERY_EVENT, D3D11_RESOURCE_MISC_SHARED, D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter, IDXGIDevice, IDXGIFactory1, IDXGIKeyedMutex, IDXGIResource, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_ERROR_WAS_STILL_DRAWING,
};
use windows::Win32::System::Memory::{
    CreateFileMappingA, MapViewOfFile, OpenFileMappingA, UnmapViewOfFile, VirtualQuery, FILE_MAP, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, MEMORY_BASIC_INFORMATION,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};
use windows::Win32::System::Registry::{RegGetValueA, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Threading::{
    CreateMutexA, CreateSemaphoreA, OpenMutexW, OpenSemaphoreW, ReleaseMutex, ReleaseSemaphore, WaitForSingleObject, MUTEX_MODIFY_STATE,
    SEMAPHORE_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE,
};

const NAME_LEN: usize = 256;
const INFO_SIZE: usize = 280;
const ACCESS_TIMEOUT_MS: u32 = 67;
/// A frame counter that has not moved for this long is not one the sender keeps (another receiver of the same
/// sender, OBS or Resolume, created it), or the sender paused: meanwhile new frames are told by comparing pictures.
const COUNTER_STALE: Duration = Duration::from_millis(250);
/// How often the receiver makes sure its sender still exists: the info block it keeps open only vanishes once
/// nobody holds it, so it is let go and opened again this often.
const PRESENCE_EVERY: Duration = Duration::from_millis(250);
/// Longest a copy from the GPU may take before the receiver gives up on it (a hung or reset GPU).
const COPY_LIMIT: Duration = Duration::from_millis(500);
/// Without a frame counter, a picture whose probe shows no change is still taken this often: a frame with no
/// capture strip (a kit switched off, a camera fixed on the sky) shows as a frame without one, not as no frame.
const QUIET_MOST: Duration = Duration::from_secs(1);
/// The largest picture received: 8K. A bigger local sender would cost gigabytes of staging textures and buffers.
const MAX_SIDE: u32 = 8192;
const MAX_PIXELS: u64 = 7680 * 4320;

fn platform(context: &str, e: impl std::fmt::Display) -> SpoutError {
    SpoutError::Platform(format!("{context}: {e}"))
}

#[link(name = "kernel32")]
extern "system" {
    fn MultiByteToWideChar(code_page: u32, flags: u32, text: *const u8, len: i32, wide: *mut u16, wide_len: i32) -> i32;
    fn WideCharToMultiByte(code_page: u32, flags: u32, wide: *const u16, wide_len: i32, text: *mut u8, len: i32, default: *const u8, used: *mut i32) -> i32;
}

/// The ANSI code page: Spout's names are bytes in it (the SDK is an ANSI API, and so are the object names it makes).
const CP_ACP: u32 = 0;

/// A name as Spout stores it (bytes in the ANSI code page of the program that registered it), for display and for
/// the wide-character APIs, which turn ANSI object names into the same characters.
fn decode_name(bytes: &[u8]) -> String {
    if bytes.is_ascii() || bytes.len() > i32::MAX as usize {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0u16; bytes.len()];
    // SAFETY: both buffers are valid for the lengths given; at most one UTF-16 unit per byte comes out.
    let n = unsafe { MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), wide.len() as i32) };
    if n <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    String::from_utf16_lossy(&wide[..n as usize])
}

/// The bytes a name has in Spout's lists and in the names of its shared memory: its ANSI form ([`decode_name`] the
/// other way round, so a name read from the list opens its sender, whatever characters it holds).
fn encode_name(name: &str) -> Vec<u8> {
    let name = name.replace('\0', "");
    if name.is_ascii() {
        return name.into_bytes();
    }
    let wide: Vec<u16> = name.encode_utf16().collect();
    let none = std::ptr::null_mut();
    // SAFETY: the first call only measures; the second writes at most `n` bytes into a buffer of `n`.
    unsafe {
        let n = WideCharToMultiByte(CP_ACP, 0, wide.as_ptr(), wide.len() as i32, none, 0, std::ptr::null(), std::ptr::null_mut());
        if n <= 0 {
            return name.into_bytes();
        }
        let mut out = vec![0u8; n as usize];
        WideCharToMultiByte(CP_ACP, 0, wide.as_ptr(), wide.len() as i32, out.as_mut_ptr(), n, std::ptr::null(), std::ptr::null_mut());
        out
    }
}

/// A NUL-terminated ANSI name for the `…A` functions: `raw` (as [`encode_name`] gives it) and a suffix.
fn ansi(raw: &[u8], suffix: &str) -> Vec<u8> {
    let mut c: Vec<u8> = raw.iter().copied().filter(|&b| b != 0).collect();
    c.extend_from_slice(suffix.as_bytes());
    c.push(0);
    c
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

/// Opens a named mutex to wait on and release, all a receiver needs: full access fails against a sender running
/// elevated. `name` in characters (the kernel turned the creator's ANSI name into the same ones).
fn open_mutex(name: &str) -> Option<OwnedHandle> {
    unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE | MUTEX_MODIFY_STATE, false, &HSTRING::from(name)) }.ok().map(OwnedHandle)
}

fn create_mutex(raw: &[u8], suffix: &str) -> Result<OwnedHandle> {
    let c = ansi(raw, suffix);
    unsafe { CreateMutexA(None, false, PCSTR(c.as_ptr())) }.map(OwnedHandle).map_err(|e| platform("CreateMutex", e))
}

/// Opens a frame counter to wait on and release. A sender running elevated refuses the right to release: then there
/// is no counter for this receiver (taking a count without giving it back would steal frames from the others).
fn open_semaphore(name: &str) -> Option<OwnedHandle> {
    unsafe { OpenSemaphoreW(SYNCHRONIZATION_SYNCHRONIZE | SEMAPHORE_MODIFY_STATE, false, &HSTRING::from(name)) }.ok().map(OwnedHandle)
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

/// A mapped view of a named file mapping (page-file backed). Other processes write it whenever they like, so it is
/// never lent out as a Rust slice: bytes are copied in and out through volatile accesses.
struct SharedMemory {
    _mapping: OwnedHandle,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    size: usize,
}

unsafe impl Send for SharedMemory {}

impl SharedMemory {
    fn from_mapping(mapping: OwnedHandle, access: FILE_MAP) -> Option<Self> {
        let view = unsafe { MapViewOfFile(mapping.0, access, 0, 0, 0) };
        if view.Value.is_null() {
            return None;
        }
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let n = unsafe { VirtualQuery(Some(view.Value), &mut info, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) };
        let size = if n == 0 { 0 } else { info.RegionSize };
        Some(Self { _mapping: mapping, view, size })
    }

    /// Opens a mapping to read: enough for sender lists and info blocks, and write access fails against a sender
    /// running elevated.
    fn open_read(raw: &[u8]) -> Option<Self> {
        let c = ansi(raw, "");
        let h = unsafe { OpenFileMappingA(FILE_MAP_READ.0, false, PCSTR(c.as_ptr())) }.ok()?;
        Self::from_mapping(OwnedHandle(h), FILE_MAP_READ)
    }

    /// Opens a mapping to change it (the sender list, when this process registers a sender).
    fn open_write(raw: &[u8]) -> Option<Self> {
        let c = ansi(raw, "");
        let h = unsafe { OpenFileMappingA(FILE_MAP_ALL_ACCESS.0, false, PCSTR(c.as_ptr())) }.ok()?;
        Self::from_mapping(OwnedHandle(h), FILE_MAP_ALL_ACCESS)
    }

    /// Creates a mapping, or opens the one of that name; the flag tells which (`true`: it existed).
    fn create(raw: &[u8], size: usize) -> Result<(Self, bool)> {
        let c = ansi(raw, "");
        let h = unsafe { CreateFileMappingA(HANDLE(-1isize as *mut c_void), None, PAGE_READWRITE, 0, size as u32, PCSTR(c.as_ptr())) }
            .map_err(|e| platform("CreateFileMapping", e))?;
        // Read before anything else can set it.
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let memory = Self::from_mapping(OwnedHandle(h), FILE_MAP_ALL_ACCESS)
            .ok_or_else(|| SpoutError::Platform(format!("MapViewOfFile failed for {}", decode_name(raw))))?; // i18n: not shown (a graphics or Windows call named in an error detail)
        Ok((memory, existed))
    }

    /// Copies `len` bytes from `offset` (fewer past the end).
    fn read(&self, offset: usize, len: usize) -> Vec<u8> {
        let base = self.view.Value as *const u8;
        // SAFETY: inside the mapped region; volatile reads, since other processes write it concurrently.
        (offset.min(self.size)..(offset + len).min(self.size)).map(|i| unsafe { std::ptr::read_volatile(base.add(i)) }).collect()
    }

    /// Writes `bytes` at `offset` (cut at the end).
    fn write(&self, offset: usize, bytes: &[u8]) {
        let base = self.view.Value as *mut u8;
        for (i, &b) in bytes.iter().enumerate().take(self.size.saturating_sub(offset)) {
            // SAFETY: inside the mapped region, which was mapped writable by `create` or `open_write`.
            unsafe { std::ptr::write_volatile(base.add(offset + i), b) };
        }
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

/// The bytes of a NUL-terminated name slot.
fn name_bytes(slot: &[u8]) -> Vec<u8> {
    let end = slot.iter().position(|&b| b == 0).unwrap_or(slot.len());
    slot[..end].to_vec()
}

/// The names in Spout's sender list (as stored: see [`encode_name`]).
fn read_names(bytes: &[u8], max: usize) -> Vec<Vec<u8>> {
    let mut names = Vec::new();
    for i in 0..max.min(bytes.len() / NAME_LEN) {
        let slot = &bytes[i * NAME_LEN..(i + 1) * NAME_LEN];
        if slot[0] == 0 {
            break;
        }
        names.push(name_bytes(slot));
    }
    names
}

/// The sender list's slots holding `names`, the rest zeros.
fn names_block(max: usize, size: usize, names: &[Vec<u8>]) -> Vec<u8> {
    let slots = max.min(size / NAME_LEN);
    let mut out = vec![0u8; slots * NAME_LEN];
    for (i, n) in names.iter().take(slots).enumerate() {
        let len = n.len().min(NAME_LEN - 1);
        out[i * NAME_LEN..i * NAME_LEN + len].copy_from_slice(&n[..len]);
    }
    out
}

/// Names of the Spout senders currently registered on this machine.
pub fn list_senders() -> Vec<String> {
    let Some(mem) = SharedMemory::open_read(b"SpoutSenderNames") else { return Vec::new() };
    let mutex = open_mutex("SpoutSenderNames_mutex");
    let locked = mutex.as_ref().map(|m| lock(m, ACCESS_TIMEOUT_MS)).unwrap_or(false);
    let max = max_senders();
    let bytes = mem.read(0, max * NAME_LEN);
    if locked {
        unlock(mutex.as_ref().unwrap());
    }
    read_names(&bytes, max).iter().map(|n| decode_name(n)).collect()
}

/// Whether the sender list holds `raw` (a name as stored).
fn registered(raw: &[u8]) -> bool {
    let Some(mem) = SharedMemory::open_read(b"SpoutSenderNames") else { return false };
    let max = max_senders();
    read_names(&mem.read(0, max * NAME_LEN), max).iter().any(|n| n == raw)
}

/// The sender most recently made active (what Spout receivers pick by default).
pub fn active_sender() -> Option<String> {
    let mem = SharedMemory::open_read(b"ActiveSenderName")?;
    let name = decode_name(&name_bytes(&mem.read(0, NAME_LEN)));
    (!name.is_empty()).then_some(name)
}

/// A sender's info block as it wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Announced {
    share_handle: u32,
    width: u32,
    height: u32,
    format: u32,
    description: String,
}

/// Reads an info block; `locked` takes the SDK's lock on it (`<name>_mutex`) for a consistent read. Without it a
/// read can meet a sender rewriting its block (a new texture): at worst one reconnect too many.
fn read_announced(memory: &SharedMemory, raw: &[u8], locked: bool) -> Option<Announced> {
    if memory.size < INFO_SIZE {
        return None;
    }
    let guard = if locked { open_mutex(&format!("{}_mutex", decode_name(raw))) } else { None };
    let held = guard.as_ref().is_some_and(|m| lock(m, ACCESS_TIMEOUT_MS));
    let b = memory.read(0, INFO_SIZE);
    if held {
        unlock(guard.as_ref().expect("held"));
    }
    let u = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    Some(Announced { share_handle: u(0), width: u(4), height: u(8), format: u(12), description: decode_name(&name_bytes(&b[20..20 + NAME_LEN])) })
}

/// Shared info block of a sender.
pub fn sender_info(name: &str) -> Option<SenderInfo> {
    let raw = encode_name(name);
    let a = read_announced(&SharedMemory::open_read(&raw)?, &raw, true)?;
    Some(SenderInfo { name: name.to_string(), share_handle: a.share_handle, width: a.width, height: a.height, format: a.format, description: a.description })
}

/// A device on `adapter`, or on the default one (the adapter Windows' graphics settings give this program).
fn create_device(adapter: Option<&IDXGIAdapter>) -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    let levels: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    // A given adapter takes the "unknown" driver type.
    let driver = if adapter.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { D3D_DRIVER_TYPE_HARDWARE };
    unsafe {
        D3D11CreateDevice(
            adapter,
            driver,
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
    match (device, context) {
        (Some(device), Some(context)) => Ok((device, context)),
        _ => Err(SpoutError::Platform("D3D11CreateDevice returned nothing".into())), // i18n: not shown (a graphics call named in an error detail)
    }
}

/// The graphics adapters that can open a sender's texture (software ones left out), with their LUIDs.
fn hardware_adapters() -> Vec<(IDXGIAdapter, (u32, i32))> {
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return Vec::new() };
    let mut out = Vec::new();
    for i in 0..16u32 {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(i) }) else { break };
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        if let Ok(a) = adapter.cast::<IDXGIAdapter>() {
            out.push((a, (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart)));
        }
    }
    out
}

/// The LUID of the adapter a device runs on.
fn device_luid(device: &ID3D11Device) -> Option<(u32, i32)> {
    let dxgi: IDXGIDevice = device.cast().ok()?;
    let desc = unsafe { dxgi.GetAdapter().and_then(|a| a.GetDesc()) }.ok()?;
    Some((desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart))
}

/// The typed format of a typeless one (resolving a multisampled texture needs one).
fn typed(format: u32) -> u32 {
    match format {
        formats::R8G8B8A8_TYPELESS => formats::R8G8B8A8_UNORM,
        formats::B8G8R8A8_TYPELESS => formats::B8G8R8A8_UNORM,
        formats::B8G8R8X8_TYPELESS => formats::B8G8R8X8_UNORM,
        f => f,
    }
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

/// Every half float's 8-bit sRGB code (negative and NaN give 0, above 1 gives 255), built once: a 16-bit sender's
/// frame then costs a table lookup per channel instead of a `powf`.
fn half_to_srgb8_table() -> &'static [u8; 65536] {
    static TABLE: OnceLock<Box<[u8; 65536]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = Box::new([0u8; 65536]);
        for (h, code) in table.iter_mut().enumerate() {
            *code = linear_to_srgb8(half_to_f32(h as u16));
        }
        table
    })
}

/// Adds `range` to a sorted list of disjoint row ranges, merging those that touch.
fn add_rows(list: &mut Vec<(u32, u32)>, range: (u32, u32)) {
    list.push(range);
    list.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(list.len());
    for &(a, b) in list.iter() {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    *list = merged;
}

/// The parts of `range` a sorted list of disjoint ranges does not cover.
fn missing_rows(range: (u32, u32), list: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut from = range.0;
    for &(a, b) in list {
        if b <= from {
            continue;
        }
        if a >= range.1 {
            break;
        }
        if a > from {
            out.push((from, a));
        }
        from = from.max(b);
        if from >= range.1 {
            break;
        }
    }
    if from < range.1 {
        out.push((from, range.1));
    }
    out
}

/// A sender's frame counter (Spout's `<name>_Count_Semaphore`), whose count the sender raises by one a frame.
struct Counter {
    handle: OwnedHandle,
    /// The highest count read, whether the count rose since it was first read, and when it last rose.
    highest: Option<i64>,
    risen: bool,
    rose: Instant,
    /// The count could not be given back: the counter is no use any more.
    dead: bool,
}

impl Counter {
    fn new(handle: OwnedHandle) -> Self {
        Self { handle, highest: None, risen: false, rose: Instant::now(), dead: false }
    }
}

enum Tick {
    /// The counter rose: a new frame.
    New(i64),
    /// Nothing new by the counter.
    Same,
    /// No count to go by (a count of 0, or one that stopped moving): pictures are compared.
    Uncounted,
}

impl Counter {
    fn poll(&mut self) -> Tick {
        if self.dead {
            return Tick::Uncounted;
        }
        // Taking one off and giving it back reads the count. A count of 0 cannot be taken (the wait times out): no
        // sender raises this counter (another receiver created it), and giving one back anyway would invent a
        // frame for every receiver of the sender.
        if unsafe { WaitForSingleObject(self.handle.0, 0) } != WAIT_OBJECT_0 {
            return Tick::Uncounted;
        }
        let mut previous = 0i32;
        if unsafe { ReleaseSemaphore(self.handle.0, 1, Some(&mut previous)) }.is_err() {
            self.dead = true;
            return Tick::Uncounted;
        }
        // The wait took one off: the count is one more than the release found.
        let count = previous as i64 + 1;
        match self.highest {
            // The first reading says nothing yet, and a counter that never rose may be one no sender raises (OBS's
            // receiver made it): pictures decide until it does.
            None => {
                self.highest = Some(count);
                Tick::Uncounted
            }
            Some(highest) if count > highest => {
                (self.highest, self.risen, self.rose) = (Some(count), true, Instant::now());
                Tick::New(count)
            }
            // The same, or lower (another receiver between its own wait and release): not new either.
            Some(_) if self.risen && self.rose.elapsed() < COUNTER_STALE => Tick::Same,
            Some(_) => Tick::Uncounted,
        }
    }
}

/// What a receiver holds of the sender it reads.
struct Connection {
    /// The sender's name as Spout stores it (see [`encode_name`]).
    raw: Vec<u8>,
    /// Its info block, kept open: read on every receive (no system call) to notice a sender recreated with a new
    /// texture or size. `None` for a moment while the receiver lets it go to see whether the sender still exists.
    memory: Option<SharedMemory>,
    /// The block as it was when the texture was opened.
    announced: Announced,
    present_checked: Instant,
    /// The texture as it is (its size may differ from the announced one).
    info: SenderInfo,
    shared: ID3D11Resource,
    /// The texture is multisampled: frames are resolved, in this format.
    resolve: Option<DXGI_FORMAT>,
    keyed: Option<IDXGIKeyedMutex>,
    access: Option<OwnedHandle>,
    counter: Option<Counter>,
    /// The frame being read: copied from the sender's texture while its lock is held, so the sender can draw the next
    /// frame while this one is read back.
    snapshot: ID3D11Resource,
    /// CPU-readable copies: the frame's rows asked for, and the probe.
    staging: ID3D11Resource,
    probe_staging: ID3D11Resource,
    bytes_per_pixel: usize,
}

impl Connection {
    /// Takes the sender's texture for a copy: `Ok(false)` when the sender held it past the timeout.
    fn lock(&self) -> Result<bool> {
        if let Some(k) = &self.keyed {
            // AcquireSync's waits come back as success codes (WAIT_TIMEOUT 0x102, WAIT_ABANDONED 0x80) that the
            // bindings take for success: only S_OK means the texture is held.
            let hr = unsafe { (Interface::vtable(k).AcquireSync)(Interface::as_raw(k), 0, ACCESS_TIMEOUT_MS) };
            return match hr.0 {
                0 => Ok(true),
                0x80 => Err(SpoutError::Platform(orbiters_i18n::tr("the sender's texture was abandoned (its program closed or its GPU reset)").into())),
                _ if hr.is_err() => Err(platform("AcquireSync", windows::core::Error::from(hr))),
                _ => Ok(false),
            };
        }
        Ok(self.access.as_ref().is_none_or(|m| lock(m, ACCESS_TIMEOUT_MS)))
    }

    fn unlock(&self) {
        if let Some(k) = &self.keyed {
            unsafe {
                let _ = k.ReleaseSync(0);
            }
        } else if let Some(m) = &self.access {
            unlock(m);
        }
    }
}

/// Reads frames from one Spout sender into CPU memory.
pub struct Receiver {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    conn: Option<Connection>,
    /// Rectangles (x0, y0, x1, y1 in pixels, top-down as stored) compared to tell a new frame from the last one when
    /// the sender keeps no frame counter; empty: the whole picture.
    probe: Vec<[u32; 4]>,
    /// The probe's bytes in the last frame lent out (`None` before one was).
    reference: Option<Vec<u8>>,
    /// When the last new frame was seen (else when the sender was opened), and the time between frames: frames are
    /// looked for when one is due.
    last_new: Option<Instant>,
    connected_at: Instant,
    interval: Duration,
    /// When the sender was last looked at for a new frame.
    last_look: Option<Instant>,
    convert: Vec<u8>,
    /// Why the last copy or mapping failed: the next receive opens the sender again.
    broken: Option<String>,
}

unsafe impl Send for Receiver {}

impl Receiver {
    pub fn new() -> Result<Self> {
        let (device, context) = create_device(None)?;
        Ok(Self {
            device,
            context,
            conn: None,
            probe: Vec::new(),
            reference: None,
            last_new: None,
            connected_at: Instant::now(),
            interval: Duration::from_millis(8),
            last_look: None,
            convert: Vec::new(),
            broken: None,
        })
    }

    pub fn info(&self) -> Option<&SenderInfo> {
        self.conn.as_ref().map(|c| &c.info)
    }

    pub fn disconnect(&mut self) {
        self.conn = None;
        self.reference = None;
        self.last_new = None;
        self.broken = None;
    }

    /// Sets the rectangles (x0, y0, x1, y1 in pixels, top-down as stored) compared to tell a new frame from the last
    /// one when the sender keeps no frame counter. Rows that change with every frame (the capture strip's time) make
    /// a small probe; with none set the whole picture is compared.
    pub fn set_probe(&mut self, rects: Vec<[u32; 4]>) {
        if rects != self.probe {
            self.probe = rects;
            self.reference = None;
        }
    }

    fn texture(&self, desc: &D3D11_TEXTURE2D_DESC, what: &str) -> Result<ID3D11Resource> {
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe { self.device.CreateTexture2D(desc, None, Some(&mut texture)) }.map_err(|e| platform(&format!("CreateTexture2D ({what})"), e))?;
        texture.ok_or_else(|| SpoutError::Platform(format!("CreateTexture2D ({what}) returned nothing")))?.cast().map_err(|e| platform("cast", e)) // i18n: not shown (a graphics or Windows call named in an error detail)
    }

    fn open_shared(&self, handle: HANDLE) -> Result<ID3D11Texture2D> {
        let mut shared: Option<ID3D11Texture2D> = None;
        unsafe { self.device.OpenSharedResource(handle, &mut shared) }.map_err(|e| platform("OpenSharedResource", e))?;
        shared.ok_or_else(|| SpoutError::Platform("OpenSharedResource returned nothing".into())) // i18n: not shown (a graphics or Windows call named in an error detail)
    }

    /// Legacy share handles open only on the adapter that made them: with VRChat on a laptop's discrete GPU and this
    /// program given the integrated one, each other adapter is tried, and the one that opens the texture is kept.
    fn open_on_another_adapter(&mut self, handle: HANDLE) -> Option<ID3D11Texture2D> {
        let current = device_luid(&self.device);
        for (adapter, luid) in hardware_adapters() {
            if Some(luid) == current {
                continue;
            }
            let Ok((device, context)) = create_device(Some(&adapter)) else { continue };
            let mut shared: Option<ID3D11Texture2D> = None;
            if unsafe { device.OpenSharedResource(handle, &mut shared) }.is_ok() {
                if let Some(shared) = shared {
                    eprintln!("spout: the sender's texture lives on another graphics adapter: reading it there");
                    (self.device, self.context) = (device, context);
                    return Some(shared);
                }
            }
        }
        None
    }

    /// Opens a sender's shared texture.
    pub fn connect(&mut self, name: &str) -> Result<SenderInfo> {
        self.disconnect();
        // A GPU reset or a driver update ends the device: a new one.
        if unsafe { self.device.GetDeviceRemovedReason() }.is_err() {
            eprintln!("spout: the graphics device was reset; making a new one");
            (self.device, self.context) = create_device(None)?;
        }
        let raw = encode_name(name);
        let memory = SharedMemory::open_read(&raw).ok_or_else(|| SpoutError::NotFound(name.to_string()))?;
        let announced = read_announced(&memory, &raw, true).ok_or_else(|| SpoutError::NotFound(name.to_string()))?;
        if formats::eight_bit_order(announced.format).is_none() && announced.format != formats::R16G16B16A16_FLOAT && announced.format != 0 {
            return Err(SpoutError::UnsupportedFormat(announced.format));
        }
        let handle = HANDLE(announced.share_handle as usize as *mut c_void);
        let shared = match self.open_shared(handle) {
            Ok(t) => t,
            Err(e) => self.open_on_another_adapter(handle).ok_or(e)?,
        };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { shared.GetDesc(&mut desc) };
        let format = desc.Format.0 as u32;
        if formats::eight_bit_order(format).is_none() && format != formats::R16G16B16A16_FLOAT {
            return Err(SpoutError::UnsupportedFormat(format));
        }
        let (w, h) = (desc.Width, desc.Height);
        if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE || w as u64 * h as u64 > MAX_PIXELS {
            return Err(SpoutError::Platform(orbiters_i18n::tr!("the sender's picture is {width}×{height}: Orbiters reads pictures up to 8K (7680×4320)", width = w, height = h)));
        }
        let keyed = if desc.MiscFlags & (D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32) != 0 { shared.cast::<IDXGIKeyedMutex>().ok() } else { None };
        // Mip levels and array slices are left out (the copies take the first subresource); samples are resolved.
        let resolve = (desc.SampleDesc.Count > 1).then(|| DXGI_FORMAT(typed(format) as i32));
        let single = |usage, cpu: u32| D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: desc.Format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: usage,
            BindFlags: 0,
            CPUAccessFlags: cpu,
            MiscFlags: 0,
        };
        let snapshot = self.texture(&single(D3D11_USAGE_DEFAULT, 0), "snapshot")?;
        let staging = self.texture(&single(D3D11_USAGE_STAGING, D3D11_CPU_ACCESS_READ.0 as u32), "staging")?;
        let probe_staging = self.texture(&single(D3D11_USAGE_STAGING, D3D11_CPU_ACCESS_READ.0 as u32), "probe")?;
        let display = decode_name(&raw);
        let info = SenderInfo {
            name: name.to_string(),
            share_handle: announced.share_handle,
            width: w,
            height: h,
            format,
            description: announced.description.clone(),
        };
        self.conn = Some(Connection {
            access: open_mutex(&format!("{display}_SpoutAccessMutex")),
            counter: open_semaphore(&format!("{display}_Count_Semaphore")).map(Counter::new),
            raw,
            memory: Some(memory),
            announced,
            present_checked: Instant::now(),
            info: info.clone(),
            shared: shared.cast().map_err(|e| platform("cast", e))?,
            resolve,
            keyed,
            snapshot,
            staging,
            probe_staging,
            bytes_per_pixel: if formats::eight_bit_order(format).is_some() { 4 } else { 8 },
        });
        self.connected_at = Instant::now();
        Ok(info)
    }

    /// Makes sure the sender still exists with the texture it had; reconnects when it was recreated (a new size).
    fn ensure_connected(&mut self) -> Result<()> {
        if let Some(why) = self.broken.take() {
            // The last copy or mapping failed (a GPU reset, a texture gone): everything is opened again.
            let Some(name) = self.conn.as_ref().map(|c| c.info.name.clone()) else { return Err(SpoutError::Platform(why)) };
            eprintln!("spout: {why}; opening \"{name}\" again");
            return self.connect(&name).map(|_| ());
        }
        let Some(conn) = self.conn.as_mut() else { return Err(SpoutError::Platform(orbiters_i18n::tr("not connected").into())) };
        if conn.present_checked.elapsed() >= PRESENCE_EVERY {
            conn.present_checked = Instant::now();
            // Let go first: held, the block would outlive its sender.
            conn.memory = None;
            conn.memory = SharedMemory::open_read(&conn.raw);
        }
        let now = conn.memory.as_ref().and_then(|m| read_announced(m, &conn.raw, false));
        let then = &conn.announced;
        let recreated = now.as_ref().map(|a| (a.share_handle, a.width, a.height) != (then.share_handle, then.width, then.height));
        let name = conn.info.name.clone();
        match recreated {
            None => {
                self.disconnect();
                Err(SpoutError::NotFound(name))
            }
            Some(true) => self.connect(&name).map(|_| ()),
            Some(false) => Ok(()),
        }
    }

    /// Maps a staging texture once the GPU has filled it, without blocking in the driver: a busy GPU (VRChat's frames
    /// come first) is waited out a quarter millisecond at a time, up to [`COPY_LIMIT`].
    fn map(&self, staging: &ID3D11Resource) -> Result<D3D11_MAPPED_SUBRESOURCE> {
        let start = Instant::now();
        loop {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            match unsafe { self.context.Map(staging, 0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32, Some(&mut mapped)) } {
                Ok(()) => return Ok(mapped),
                Err(e) if e.code() == DXGI_ERROR_WAS_STILL_DRAWING => {
                    if start.elapsed() >= COPY_LIMIT {
                        return Err(SpoutError::Platform(orbiters_i18n::tr("the GPU did not finish copying the frame").into()));
                    }
                    std::thread::sleep(Duration::from_micros(250));
                }
                Err(e) => return Err(platform("Map", e)), // i18n: not shown (a graphics call named in an error detail)
            }
        }
    }

    /// The probe's rectangles in a `width × height` frame, as copy boxes (the whole frame when none is set).
    fn probe_boxes(&self, width: u32, height: u32) -> Vec<D3D11_BOX> {
        let boxes: Vec<D3D11_BOX> = self
            .probe
            .iter()
            .map(|r| D3D11_BOX { left: r[0].min(width), top: r[1].min(height), front: 0, right: r[2].min(width), bottom: r[3].min(height), back: 1 })
            .filter(|b| b.left < b.right && b.top < b.bottom)
            .collect();
        if boxes.is_empty() {
            vec![D3D11_BOX { left: 0, top: 0, front: 0, right: width, bottom: height, back: 1 }]
        } else {
            boxes
        }
    }

    /// Copies the probe out of the sender's texture and compares it with the last frame's: whether the picture changed.
    fn probe_changed(&mut self) -> Result<bool> {
        let Some(conn) = self.conn.as_ref() else { return Ok(false) };
        let boxes = self.probe_boxes(conn.info.width, conn.info.height);
        if !conn.lock()? {
            return Ok(false);
        }
        unsafe {
            let source = match conn.resolve {
                // A multisampled texture is resolved first, into the snapshot (free until a frame is taken).
                Some(format) => {
                    self.context.ResolveSubresource(&conn.snapshot, 0, &conn.shared, 0, format);
                    &conn.snapshot
                }
                None => &conn.shared,
            };
            for b in &boxes {
                self.context.CopySubresourceRegion(&conn.probe_staging, 0, b.left, b.top, 0, source, 0, Some(b as *const D3D11_BOX));
            }
        }
        conn.unlock();
        unsafe { self.context.Flush() };
        let mapped = self.map(&conn.probe_staging)?;
        let bytes = probe_bytes(&mapped, &boxes, conn.bytes_per_pixel);
        unsafe { self.context.Unmap(&conn.probe_staging, 0) };
        Ok(self.reference.as_ref() != Some(&bytes))
    }

    /// Copies the frame out of the sender's texture into the snapshot and reads the probe from it: `Ok(false)` when
    /// the sender held its texture. The lock is held until the GPU did the copy, so a sender that takes it cannot
    /// draw the next frame into this one meanwhile (VRChat's takes none: its frames can still tear).
    fn take_snapshot(&mut self) -> Result<bool> {
        let Some(conn) = self.conn.as_ref() else { return Ok(false) };
        let boxes = self.probe_boxes(conn.info.width, conn.info.height);
        if !conn.lock()? {
            return Ok(false);
        }
        unsafe {
            match conn.resolve {
                Some(format) => self.context.ResolveSubresource(&conn.snapshot, 0, &conn.shared, 0, format),
                None => self.context.CopySubresourceRegion(&conn.snapshot, 0, 0, 0, 0, &conn.shared, 0, None),
            }
            for b in &boxes {
                self.context.CopySubresourceRegion(&conn.probe_staging, 0, b.left, b.top, 0, &conn.snapshot, 0, Some(b as *const D3D11_BOX));
            }
            self.context.Flush();
        }
        let mapped = self.map(&conn.probe_staging);
        conn.unlock();
        let mapped = mapped?;
        let bytes = probe_bytes(&mapped, &boxes, conn.bytes_per_pixel);
        unsafe { self.context.Unmap(&conn.probe_staging, 0) };
        self.reference = Some(bytes);
        Ok(true)
    }

    /// How long no frame is due: about half a frame interval after the last one.
    fn not_due(&self) -> Option<Duration> {
        let since = self.last_new?.elapsed();
        let quiet = self.interval / 2;
        (since < quiet).then(|| quiet - since)
    }

    /// The pause between two looks once a frame is due: 1 ms, growing to 8 ms while none comes (a paused game, a
    /// camera closed), so a quiet sender costs little.
    /// Whether looking for a new frame is worth it now: one is due, and the sender was not looked at within the pause
    /// an overdue frame waits. For readers of several senders, which look at each in turn without waiting
    /// ([`Receiver::receive_with`] with no timeout still looks once).
    pub fn due(&self) -> bool {
        self.not_due().is_none() && self.last_look.is_none_or(|t| t.elapsed() >= self.overdue_pause())
    }

    fn overdue_pause(&self) -> Duration {
        let since = self.last_new.unwrap_or(self.connected_at).elapsed().saturating_sub(self.interval / 2);
        (since / 8).clamp(Duration::from_millis(1), Duration::from_millis(8))
    }

    /// Waits until `deadline` for a new frame: `Some` with the sender's frame count (when it keeps one) once one came.
    fn wait_for_frame(&mut self, deadline: Instant) -> Result<Option<Option<i64>>> {
        if let Some(wait) = self.not_due() {
            std::thread::sleep(wait.min(deadline.saturating_duration_since(Instant::now())));
        }
        loop {
            self.last_look = Some(Instant::now());
            let tick = self.conn.as_mut().and_then(|c| c.counter.as_mut()).map_or(Tick::Uncounted, Counter::poll);
            match tick {
                Tick::New(count) => return Ok(Some(Some(count))),
                Tick::Same => {}
                Tick::Uncounted => {
                    let quiet = self.last_new.unwrap_or(self.connected_at).elapsed() >= QUIET_MOST;
                    if self.probe_changed()? || quiet {
                        return Ok(Some(None));
                    }
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            std::thread::sleep(self.overdue_pause().min(deadline - now));
        }
    }

    /// Waits up to `timeout` for a new frame and lends it to `f`, which reads back the rows it needs
    /// ([`FrameReader::rows`]). `Ok(None)` when no new frame arrived. A sender without a frame counter (VRChat's) is
    /// looked at about when a frame is due, and a frame is new when the probe's pixels ([`Receiver::set_probe`])
    /// differ from the last frame's.
    pub fn receive_with<R>(&mut self, timeout: Duration, f: impl FnOnce(&mut FrameReader<'_>) -> R) -> Result<Option<R>> {
        self.ensure_connected()?;
        let deadline = Instant::now() + timeout;
        let found = match self.wait_for_frame(deadline) {
            Ok(found) => found,
            Err(e) => {
                self.broken = Some(e.to_string());
                return Err(e);
            }
        };
        let Some(frame_count) = found else { return Ok(None) };
        let seen = Instant::now();
        match self.take_snapshot() {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(e) => {
                self.broken = Some(e.to_string());
                return Err(e);
            }
        }
        if let Some(last) = self.last_new {
            let dt = seen.saturating_duration_since(last);
            if dt < Duration::from_millis(250) {
                // Follows a change of frame rate within a few frames.
                self.interval = ((self.interval * 3 + dt) / 4).clamp(Duration::from_millis(4), Duration::from_millis(100));
            }
        }
        self.last_new = Some(seen);
        let mut reader = FrameReader { receiver: self, seen, frame_count, fresh: Vec::new(), converted: Vec::new(), mapped: None };
        Ok(Some(f(&mut reader)))
    }

    /// Waits up to `timeout` for a new frame and lends the whole of it to `f`. `Ok(None)` when no new frame arrived.
    pub fn receive<R>(&mut self, timeout: Duration, f: impl FnOnce(Frame<'_>) -> R) -> Result<Option<R>> {
        match self.receive_with(timeout, |r| r.full().map(f))? {
            Some(Ok(v)) => Ok(Some(v)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }
}

/// The bytes of `boxes` in a mapped texture, row after row.
fn probe_bytes(mapped: &D3D11_MAPPED_SUBRESOURCE, boxes: &[D3D11_BOX], bytes_per_pixel: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for b in boxes {
        let len = (b.right - b.left) as usize * bytes_per_pixel;
        for y in b.top..b.bottom {
            let start = y as usize * mapped.RowPitch as usize + b.left as usize * bytes_per_pixel;
            // SAFETY: the texture is mapped and the box lies within it.
            out.extend_from_slice(unsafe { std::slice::from_raw_parts((mapped.pData as *const u8).add(start), len) });
        }
    }
    out
}

/// A frame lent by [`Receiver::receive_with`]. Rows are read back from the GPU as they are asked for: decoding the
/// capture strip reads a sixth of the picture, a keyframe all of it.
pub struct FrameReader<'r> {
    receiver: &'r mut Receiver,
    /// When the frame was seen: just before it was copied from the sender's texture.
    pub seen: Instant,
    /// The sender's frame count, when it keeps one.
    pub frame_count: Option<i64>,
    /// Rows of the staging texture that hold this frame, and (16-bit senders) of the conversion buffer.
    fresh: Vec<(u32, u32)>,
    converted: Vec<(u32, u32)>,
    mapped: Option<D3D11_MAPPED_SUBRESOURCE>,
}

impl FrameReader<'_> {
    pub fn width(&self) -> u32 {
        self.receiver.conn.as_ref().map_or(0, |c| c.info.width)
    }

    pub fn height(&self) -> u32 {
        self.receiver.conn.as_ref().map_or(0, |c| c.info.height)
    }

    /// The frame with at least these ranges of rows (top-down as stored, ends excluded) read back. Only those rows
    /// are this frame's: the others may still hold an older one.
    pub fn rows(&mut self, ranges: &[(u32, u32)]) -> Result<Frame<'_>> {
        let rx = &mut *self.receiver;
        let Some(conn) = rx.conn.as_ref() else { return Err(SpoutError::Platform(orbiters_i18n::tr("not connected").into())) };
        let (w, h, format, bpp) = (conn.info.width, conn.info.height, conn.info.format, conn.bytes_per_pixel);
        let wanted: Vec<(u32, u32)> = ranges.iter().map(|&(a, b)| (a.min(h), b.min(h))).filter(|(a, b)| a < b).collect();
        let missing: Vec<(u32, u32)> = wanted.iter().flat_map(|&r| missing_rows(r, &self.fresh)).collect();
        if !missing.is_empty() {
            unsafe {
                if self.mapped.take().is_some() {
                    rx.context.Unmap(&conn.staging, 0);
                }
                for &(a, b) in &missing {
                    let area = D3D11_BOX { left: 0, top: a, front: 0, right: w, bottom: b, back: 1 };
                    rx.context.CopySubresourceRegion(&conn.staging, 0, 0, a, 0, &conn.snapshot, 0, Some(&area as *const D3D11_BOX));
                }
                rx.context.Flush();
            }
            for r in missing {
                add_rows(&mut self.fresh, r);
            }
        }
        if self.mapped.is_none() {
            match rx.map(&conn.staging) {
                Ok(m) => self.mapped = Some(m),
                Err(e) => {
                    rx.broken = Some(e.to_string());
                    return Err(e);
                }
            }
        }
        let m = self.mapped.expect("mapped");
        let stride = m.RowPitch as usize;
        // `height` rows `stride` apart, the last one only as long as the picture is wide.
        let len = stride * (h as usize - 1) + w as usize * bpp;
        if stride < w as usize * bpp || (m.DepthPitch != 0 && (m.DepthPitch as usize) < len) {
            return Err(SpoutError::Platform(orbiters_i18n::tr("the mapped frame is smaller than the picture").into()));
        }
        // SAFETY: the staging texture stays mapped until this reader unmaps it, and `len` bytes lie within it.
        let raw = unsafe { std::slice::from_raw_parts(m.pData as *const u8, len) };
        if let Some(order) = formats::eight_bit_order(format) {
            return Ok(Frame { width: w, height: h, stride, data: raw, order, frame_count: self.frame_count, seen: self.seen });
        }
        // 16-bit float (linear): the rows asked for become 8-bit sRGB codes, the form every decoder expects.
        let table = half_to_srgb8_table();
        let row_out = w as usize * 4;
        rx.convert.resize(row_out * h as usize, 0);
        let todo: Vec<(u32, u32)> = wanted.iter().flat_map(|&r| missing_rows(r, &self.converted)).collect();
        for (a, b) in todo {
            for y in a as usize..b as usize {
                let row = &raw[y * stride..y * stride + w as usize * 8];
                let out = &mut rx.convert[y * row_out..(y + 1) * row_out];
                for (px, o) in row.as_chunks::<8>().0.iter().zip(out.as_chunks_mut::<4>().0) {
                    o[0] = table[u16::from_le_bytes([px[0], px[1]]) as usize];
                    o[1] = table[u16::from_le_bytes([px[2], px[3]]) as usize];
                    o[2] = table[u16::from_le_bytes([px[4], px[5]]) as usize];
                    o[3] = 255;
                }
            }
            add_rows(&mut self.converted, (a, b));
        }
        Ok(Frame { width: w, height: h, stride: row_out, data: &rx.convert, order: ChannelOrder::Rgba, frame_count: self.frame_count, seen: self.seen })
    }

    /// The whole frame.
    pub fn full(&mut self) -> Result<Frame<'_>> {
        let h = self.height();
        self.rows(&[(0, h)])
    }
}

impl Drop for FrameReader<'_> {
    fn drop(&mut self) {
        if self.mapped.take().is_some() {
            if let Some(conn) = self.receiver.conn.as_ref() {
                unsafe { self.receiver.context.Unmap(&conn.staging, 0) };
            }
        }
    }
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
        let (device, context) = create_device(None)?;
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
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }.map_err(|e| platform("CreateTexture2D (shared)", e))?; // i18n: not shown (a graphics or Windows call named in an error detail)
        let texture: ID3D11Texture2D = texture.expect("texture");
        let resource: IDXGIResource = texture.cast().map_err(|e| platform("cast", e))?;
        let handle = unsafe { resource.GetSharedHandle() }.map_err(|e| platform("GetSharedHandle", e))?;

        let raw = encode_name(name);
        let (info, existed) = SharedMemory::create(&raw, INFO_SIZE)?;
        if existed && registered(&raw) {
            // Another program sends under this name: taking its info block over would redirect its receivers. (A
            // block that outlived its sender, held by a receiver, is taken over: that is how a sender restarts.)
            return Err(SpoutError::Platform(orbiters_i18n::tr!("a Spout sender named \"{name}\" already exists", name = name)));
        }
        {
            let mut b = [0u8; INFO_SIZE];
            b[0..4].copy_from_slice(&(handle.0 as usize as u32).to_le_bytes());
            b[4..8].copy_from_slice(&width.to_le_bytes());
            b[8..12].copy_from_slice(&height.to_le_bytes());
            b[12..16].copy_from_slice(&formats::B8G8R8A8_UNORM.to_le_bytes());
            let exe = encode_name(&std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default());
            let len = exe.len().min(NAME_LEN - 1);
            b[20..20 + len].copy_from_slice(&exe[..len]);
            info.write(0, &b);
        }
        let access = create_mutex(&raw, "_SpoutAccessMutex")?;
        let c = ansi(&raw, "_Count_Semaphore");
        let counter = unsafe { CreateSemaphoreA(None, 1, i32::MAX, PCSTR(c.as_ptr())) }.map(OwnedHandle).map_err(|e| platform("CreateSemaphore", e))?;
        let registration = register_name(&raw, true)?.expect("registration when adding");
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
            return Err(SpoutError::Platform(orbiters_i18n::tr("frame smaller than the sender").into()));
        }
        self.bgra.resize(n, 0);
        for (dst, src) in self.bgra.as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        let resource: ID3D11Resource = self.texture.cast().map_err(|e| platform("cast", e))?;
        if !lock(&self.access, ACCESS_TIMEOUT_MS) {
            // A receiver held the texture: this frame is not sent, and the counter says nothing new.
            return Ok(());
        }
        unsafe {
            self.context.UpdateSubresource(&resource, 0, None, self.bgra.as_ptr() as *const c_void, self.width * 4, 0);
        }
        self.finish_gpu();
        unlock(&self.access);
        // One more frame: take one, give two back (the Spout SDK's way, which keeps the count above 0).
        unsafe {
            let _ = WaitForSingleObject(self.counter.0, 0);
            let _ = ReleaseSemaphore(self.counter.0, 2, None);
        }
        Ok(())
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        let _ = register_name(&encode_name(&self.name), false);
    }
}

/// Open handles on the shared sender list and active sender slot.
struct Registration {
    _names: SharedMemory,
    _active: Option<SharedMemory>,
}

/// A name slot holding `name` (cut to fit, NUL-terminated).
fn name_slot(name: &[u8]) -> [u8; NAME_LEN] {
    let mut slot = [0u8; NAME_LEN];
    let len = name.len().min(NAME_LEN - 1);
    slot[..len].copy_from_slice(&name[..len]);
    slot
}

/// Adds or removes a name (as stored, see [`encode_name`]) in the shared sender list and the active sender slot.
/// When adding, the returned registration must stay alive as long as the sender exists.
fn register_name(name: &[u8], add: bool) -> Result<Option<Registration>> {
    let max = max_senders();
    let mem = match SharedMemory::open_write(b"SpoutSenderNames") {
        Some(m) => m,
        None if add => SharedMemory::create(b"SpoutSenderNames", max * NAME_LEN)?.0,
        None => return Ok(None),
    };
    let mutex = create_mutex(b"SpoutSenderNames_mutex", "")?;
    let locked = lock(&mutex, ACCESS_TIMEOUT_MS);
    let mut names = read_names(&mem.read(0, max * NAME_LEN), max);
    if add {
        if !names.iter().any(|n| n == name) {
            names.push(name.to_vec());
        }
    } else {
        names.retain(|n| n != name);
    }
    mem.write(0, &names_block(max, mem.size, &names));
    if locked {
        unlock(&mutex);
    }

    let active = match SharedMemory::open_write(b"ActiveSenderName") {
        Some(m) => Some(m),
        None if add => Some(SharedMemory::create(b"ActiveSenderName", NAME_LEN)?.0),
        None => None,
    };
    if let Some(active) = active.as_ref() {
        let current = name_bytes(&active.read(0, NAME_LEN));
        if add && (current.is_empty() || !names.contains(&current)) {
            active.write(0, &name_slot(name));
        } else if !add && current == name {
            active.write(0, &name_slot(names.first().map_or(&[][..], |n| n.as_slice())));
        }
    }
    Ok(add.then_some(Registration { _names: mem, _active: active }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Foundation::WAIT_TIMEOUT;

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
        receiver.conn.as_mut().unwrap().counter = None;
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

    /// OBS's Spout2 plugin, receiving VRChat's sender too, creates a frame counter that VRChat never raises: pictures
    /// are compared (from the first frame on, not 250 ms later), and reading the counter never changes its count.
    #[test]
    fn a_counter_nobody_raises_falls_back_to_comparing_pictures() {
        let name = format!("OrbitersSpoutStuckCounter-{}", std::process::id());
        let (w, h) = (320u32, 180u32);
        let mut sender = Sender::new(&name, w, h).expect("sender");
        let mut receiver = Receiver::new().expect("receiver");
        receiver.connect(&name).expect("connect");
        let stuck = unsafe { CreateSemaphoreA(None, 1, i32::MAX, PCSTR::null()) }.unwrap();
        receiver.conn.as_mut().unwrap().counter = Some(Counter::new(OwnedHandle(stuck)));
        let mut frame = vec![90u8; (w * h * 4) as usize];
        sender.send_rgba(&frame).unwrap();
        assert!(receiver.receive(Duration::from_millis(200), |_| ()).unwrap().is_some(), "the first frame");
        frame[..4].copy_from_slice(&[10, 20, 30, 255]);
        sender.send_rgba(&frame).unwrap();
        let got = receiver.receive(COUNTER_STALE * 4, |f| f.data[..4].to_vec()).unwrap();
        assert_eq!(got.as_deref(), Some(&[30, 20, 10, 255][..]), "the new picture behind a counter that never moves");
        // Still exactly 1: one count to take, none after.
        let handle = receiver.conn.as_ref().unwrap().counter.as_ref().unwrap().handle.0;
        assert_eq!(unsafe { WaitForSingleObject(handle, 0) }, WAIT_OBJECT_0);
        assert_eq!(unsafe { WaitForSingleObject(handle, 0) }, WAIT_TIMEOUT);
    }

    #[test]
    fn counters_are_read_without_changing_them() {
        let empty = unsafe { CreateSemaphoreA(None, 0, i32::MAX, PCSTR::null()) }.unwrap();
        let mut c = Counter::new(OwnedHandle(empty));
        // At 0 (another receiver's counter before any frame): nothing to go by, and no count invented.
        assert!(matches!(c.poll(), Tick::Uncounted));
        assert!(matches!(c.poll(), Tick::Uncounted));
        assert_eq!(unsafe { WaitForSingleObject(c.handle.0, 0) }, WAIT_TIMEOUT);
        // A count that has not risen yet says nothing: pictures decide (a counter OBS made would stall frames).
        unsafe { ReleaseSemaphore(c.handle.0, 3, None) }.unwrap();
        assert!(matches!(c.poll(), Tick::Uncounted));
        assert!(matches!(c.poll(), Tick::Uncounted));
        // One its sender raises: new, the same, new again, and stale once it stops.
        unsafe { ReleaseSemaphore(c.handle.0, 1, None) }.unwrap();
        assert!(matches!(c.poll(), Tick::New(4)));
        assert!(matches!(c.poll(), Tick::Same));
        unsafe { ReleaseSemaphore(c.handle.0, 1, None) }.unwrap();
        assert!(matches!(c.poll(), Tick::New(5)));
        c.rose = Instant::now().checked_sub(COUNTER_STALE).unwrap();
        assert!(matches!(c.poll(), Tick::Uncounted));
        // The count is as the sender left it.
        for _ in 0..5 {
            assert_eq!(unsafe { WaitForSingleObject(c.handle.0, 0) }, WAIT_OBJECT_0);
        }
        assert_eq!(unsafe { WaitForSingleObject(c.handle.0, 0) }, WAIT_TIMEOUT);
    }

    #[test]
    fn only_the_rows_asked_for_are_read_back() {
        let name = format!("OrbitersSpoutRows-{}", std::process::id());
        let (w, h) = (256u32, 128u32);
        let mut sender = Sender::new(&name, w, h).expect("sender");
        let mut receiver = Receiver::new().expect("receiver");
        receiver.connect(&name).expect("connect");
        // Row y is grey y + shift.
        let picture =
            |shift: u8| -> Vec<u8> { (0..h).flat_map(|y| (0..w).flat_map(move |_| [(y as u8).wrapping_add(shift); 3].into_iter().chain([255]))).collect() };
        sender.send_rgba(&picture(0)).unwrap();
        receiver.receive(Duration::from_millis(500), |_| ()).unwrap().expect("a first frame");
        sender.send_rgba(&picture(100)).unwrap();
        let (part, whole) = receiver
            .receive_with(Duration::from_millis(500), |r| {
                let part: Vec<u8> = {
                    let f = r.rows(&[(10, 20), (100, 110)]).unwrap();
                    [10usize, 19, 100, 109].iter().map(|&y| f.data[y * f.stride]).collect()
                };
                let f = r.full().unwrap();
                let whole: Vec<u8> = (0..h as usize).map(|y| f.data[y * f.stride + (w as usize - 1) * 4]).collect();
                (part, whole)
            })
            .unwrap()
            .expect("a second frame");
        assert_eq!(part, vec![110, 119, 200, 209]);
        assert_eq!(whole, (0..h).map(|y| (y as u8).wrapping_add(100)).collect::<Vec<_>>());
    }

    #[test]
    fn a_probe_tells_frames_apart_by_its_pixels() {
        let name = format!("OrbitersSpoutProbe-{}", std::process::id());
        let (w, h) = (320u32, 180u32);
        let mut sender = Sender::new(&name, w, h).expect("sender");
        let mut receiver = Receiver::new().expect("receiver");
        receiver.connect(&name).expect("connect");
        receiver.conn.as_mut().unwrap().counter = None;
        // The top four rows are where this "sender" changes something every frame.
        receiver.set_probe(vec![[0, 0, w, 4]]);
        let mut frame = vec![90u8; (w * h * 4) as usize];
        sender.send_rgba(&frame).unwrap();
        assert!(receiver.receive(Duration::from_millis(200), |_| ()).unwrap().is_some());
        let row = (w * 4) as usize;
        frame[50 * row..50 * row + 4].copy_from_slice(&[1, 2, 3, 255]);
        sender.send_rgba(&frame).unwrap();
        assert!(receiver.receive(Duration::from_millis(60), |_| ()).unwrap().is_none(), "a change outside the probe is not looked at");
        frame[2 * row..2 * row + 4].copy_from_slice(&[4, 5, 6, 255]);
        sender.send_rgba(&frame).unwrap();
        let got = receiver.receive(Duration::from_millis(200), |f| (f.data[2 * f.stride], f.data[50 * f.stride])).unwrap();
        assert_eq!(got, Some((6, 3)), "the frame, with both changes");
    }

    #[test]
    fn half_floats_become_srgb_codes_through_the_table() {
        let t = half_to_srgb8_table();
        assert_eq!((t[0x0000], t[0x3C00], t[0x3800]), (0, 255, 188), "0, 1 and 0.5");
        assert_eq!((t[0xBC00], t[0x7C00], t[0x7E00]), (0, 255, 0), "-1, infinity and NaN");
        for h in (0..=u16::MAX).step_by(97) {
            assert_eq!(t[h as usize], linear_to_srgb8(half_to_f32(h)));
        }
    }

    #[test]
    fn row_ranges_merge_and_subtract() {
        let mut fresh = Vec::new();
        add_rows(&mut fresh, (10, 20));
        add_rows(&mut fresh, (30, 40));
        add_rows(&mut fresh, (20, 25));
        assert_eq!(fresh, vec![(10, 25), (30, 40)]);
        assert_eq!(missing_rows((0, 50), &fresh), vec![(0, 10), (25, 30), (40, 50)]);
        assert_eq!(missing_rows((12, 22), &fresh), Vec::<(u32, u32)>::new());
        assert_eq!(missing_rows((22, 35), &fresh), vec![(25, 30)]);
    }

    #[test]
    fn shared_memory_is_copied_in_and_out() {
        let raw = format!("OrbitersSpoutMapping-{}", std::process::id()).into_bytes();
        let (a, existed) = SharedMemory::create(&raw, INFO_SIZE).unwrap();
        assert!(!existed);
        a.write(0, b"orbiters");
        let (b, existed) = SharedMemory::create(&raw, INFO_SIZE).unwrap();
        assert!(existed, "a second create opens the same block");
        assert_eq!(b.read(0, 8), b"orbiters");
        let r = SharedMemory::open_read(&raw).unwrap();
        assert_eq!(r.read(0, 8), b"orbiters");
        assert_eq!(r.read(r.size - 2, 8).len(), 2, "reads stop at the end");
        drop((a, b, r));
        assert!(SharedMemory::open_read(&raw).is_none(), "gone with its last handle");
    }

    #[test]
    fn sender_names_are_kept_as_stored() {
        let names = vec![b"VRChat".to_vec(), b"Caf\xe9".to_vec()];
        let block = names_block(4, 4 * NAME_LEN, &names);
        assert_eq!(block.len(), 4 * NAME_LEN);
        assert_eq!(read_names(&block, 4), names);
        assert_eq!((encode_name("VRChat"), decode_name(b"VRChat")), (b"VRChat".to_vec(), "VRChat".to_string()));
        // A name read from the list opens its sender again: back to the same bytes, whatever the code page shows.
        let stored = b"Caf\xe9 Cam".to_vec();
        let shown = decode_name(&stored);
        if !shown.contains('\u{fffd}') {
            assert_eq!(encode_name(&shown), stored);
        }
    }
}
