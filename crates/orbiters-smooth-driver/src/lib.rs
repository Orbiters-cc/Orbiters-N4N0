//! Orbiters - N4N0's SteamVR server driver for tracking smoothing (`driver_orbiters_smooth.dll`), an equivalent of
//! OVR-SmoothTracking. SteamVR loads it into `vrserver.exe`; it asks the server driver host for every driver's pose
//! updates by patching one entry of the host's function table (as OpenVR-SpaceCalibrator does to read poses), eases
//! the position and rotation of the devices the app marks (`orbiters_smooth::filter`) and damps the velocities
//! SteamVR predicts with, then hands the pose on. It reads what to do, without locks, from the control block the app
//! shares (`orbiters_smooth`), and smooths nothing while that block is missing, off, stale or of another version.
//!
//! Across the FFI boundary it never unwinds (every entry catches panics) and never blocks the pose thread (the hook
//! takes no lock that another pose or a frame could hold, and allocates nothing per pose).
//!
//! The vtable slot it patches, the interface version strings and the `DriverPose_t` layout come from Valve's
//! `openvr_driver.h` (`openvr.rs`); they can only be confirmed against a running SteamVR.

#![cfg(target_os = "windows")]

pub mod filter;
pub mod openvr;

use filter::DeviceFilter;
use openvr::{
    DriverPose, GetGenericInterfaceFn, LogFn, Object, PoseUpdatedFn, ProviderVtable, DRIVER_LOG, HOST_POSE_UPDATED_SLOT, INIT_INTERFACE_NOT_FOUND, INIT_NONE,
    INTERFACE_VERSIONS, MAX_DEVICES, SERVER_DRIVER_HOST_005, SERVER_DRIVER_HOST_006, SERVER_TRACKED_DEVICE_PROVIDER,
};
use orbiters_smooth::win::{tick_ms, VirtualProtect, PAGE_READWRITE};
use orbiters_smooth::{ControlBlock, DriverReport, Mapping, CONTROL_NAME};
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// The two server-driver-host versions whose pose updates we filter (`IVRServerDriverHost_006`, and `_005` for
/// older drivers). Each has its own function table; each is hooked at most once.
const MAX_HOOKS: usize = 2;

/// The crate version, reported in the control block's heartbeat.
const DRIVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Everything the driver keeps. One instance, [`STATE`]. The pose hook touches only the lock-free parts (the
/// settings cache, the filter bank and the counters); the rest is set up in `Init`/`RunFrame` and torn down in
/// `Cleanup`, one thread at a time.
struct State {
    /// Smoothing is on and the control block is usable: the hook does real work only then.
    active: AtomicBool,
    /// Per device, `f32` strength bits, copied from the control block each frame.
    strengths: [AtomicU32; MAX_DEVICES],
    /// One filter per device index, each reached only through [`FilterBank`].
    filters: FilterBank,
    /// Pose updates seen and smoothed, for the heartbeat.
    poses: AtomicU64,
    smoothed: AtomicU64,
    /// Host interfaces hooked.
    hooks_installed: AtomicU32,
    /// `GetGenericInterface`'s table entry while hooked (restored in `Cleanup`), and its original.
    get_interface_entry: AtomicPtr<*mut c_void>,
    get_interface_original: AtomicUsize,
    /// Pose-update originals the detours call through, one per hook index.
    pose_originals: [AtomicUsize; MAX_HOOKS],
    /// `IVRDriverContext*`, to fetch the hosts lazily (the first `GetGenericInterface` may precede them).
    context: AtomicPtr<c_void>,
    /// `IVRDriverLog`, when the host gave us one.
    log: AtomicPtr<c_void>,
    /// Set-up and tear-down bookkeeping (never touched by the pose hook).
    setup: Mutex<Setup>,
    /// The monotonic clock the filter times poses with, started in `Init`.
    clock: UnsafeCell<Option<Instant>>,
}

// SAFETY: `clock` is written once in `Init` and read on pose threads only after that; `Instant` is `Sync`. Every
// other field is atomic or a `Mutex`.
unsafe impl Sync for State {}

#[derive(Default)]
struct Setup {
    /// The control block, open while the app's lives.
    control: Option<Mapping>,
    /// `GetTickCount64()` at the last attempt to open it (retry about once a second).
    last_open_attempt_ms: u64,
    /// Patched vtable entries to restore: (entry address, the detour we wrote), for the hosts and `GetGenericInterface`.
    patched: Vec<(usize, usize)>,
}

/// The filter bank: a filter per device, each guarded by a try-lock so the pose hook never waits (two updates for
/// the same device at once — rare — let the second pass unfiltered rather than block).
struct FilterBank {
    busy: [AtomicBool; MAX_DEVICES],
    filters: [UnsafeCell<DeviceFilter>; MAX_DEVICES],
}

impl FilterBank {
    const fn new() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const BUSY: AtomicBool = AtomicBool::new(false);
        #[allow(clippy::declare_interior_mutable_const)]
        const FILTER: UnsafeCell<DeviceFilter> = UnsafeCell::new(DeviceFilter::new());
        Self { busy: [BUSY; MAX_DEVICES], filters: [FILTER; MAX_DEVICES] }
    }

    /// Runs `f` on one device's filter while holding its try-lock; `None` (does nothing) when that device's filter
    /// is busy on another thread.
    fn with<R>(&self, index: usize, f: impl FnOnce(&mut DeviceFilter) -> R) -> Option<R> {
        let busy = self.busy.get(index)?;
        if busy.swap(true, Ordering::Acquire) {
            return None;
        }
        // SAFETY: the try-lock gives this thread sole access to the cell until it is released below.
        let result = f(unsafe { &mut *self.filters[index].get() });
        busy.store(false, Ordering::Release);
        Some(result)
    }

    fn reset(&self, index: usize) {
        self.with(index, DeviceFilter::reset);
    }
}

impl State {
    const fn new() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const STRENGTH: AtomicU32 = AtomicU32::new(0);
        #[allow(clippy::declare_interior_mutable_const)]
        const ORIGINAL: AtomicUsize = AtomicUsize::new(0);
        Self {
            active: AtomicBool::new(false),
            strengths: [STRENGTH; MAX_DEVICES],
            filters: FilterBank::new(),
            poses: AtomicU64::new(0),
            smoothed: AtomicU64::new(0),
            hooks_installed: AtomicU32::new(0),
            get_interface_entry: AtomicPtr::new(ptr::null_mut()),
            get_interface_original: AtomicUsize::new(0),
            pose_originals: [ORIGINAL; MAX_HOOKS],
            context: AtomicPtr::new(ptr::null_mut()),
            log: AtomicPtr::new(ptr::null_mut()),
            setup: Mutex::new(Setup { control: None, last_open_attempt_ms: 0, patched: Vec::new() }),
            clock: UnsafeCell::new(None),
        }
    }

    fn now_seconds(&self) -> f64 {
        // SAFETY: `clock` is set in `Init`, before poses flow, and only read afterward.
        match unsafe { &*self.clock.get() } {
            Some(start) => start.elapsed().as_secs_f64(),
            None => 0.0,
        }
    }
}

static STATE: State = State::new();

/// The pose hook for one host index: copy the pose, smooth it, call the host's original. `extern "system"` is the
/// x64 Windows convention C++ methods use; `this` comes first. Never unwinds, never blocks, allocates nothing.
extern "system" fn detour<const INDEX: usize>(this: *mut c_void, device: u32, pose: *const DriverPose, pose_size: u32) {
    let original: Option<PoseUpdatedFn> = {
        let bits = STATE.pose_originals[INDEX].load(Ordering::Acquire);
        // SAFETY: a function pointer this crate stored when it patched the table; never null once set.
        (bits != 0).then(|| unsafe { std::mem::transmute::<usize, PoseUpdatedFn>(bits) })
    };
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut smoothed = None;
        // Only our own struct layout is touched; any other size passes straight through.
        if STATE.active.load(Ordering::Relaxed) && !pose.is_null() && pose_size as usize == std::mem::size_of::<DriverPose>() {
            STATE.poses.fetch_add(1, Ordering::Relaxed);
            let index = device as usize;
            let strength = STATE.strengths.get(index).map(|s| f32::from_bits(s.load(Ordering::Relaxed))).unwrap_or(0.0);
            // SAFETY: SteamVR passes a readable `DriverPose_t` of the size checked above.
            let mut copy = unsafe { ptr::read(pose) };
            let now = STATE.now_seconds() + copy.pose_time_offset;
            // `Some(false)` (untouched: strength 0 or a bad pose) and `None` (the device's filter was busy on
            // another thread) both send the pose on as it came.
            if let Some(true) = STATE.filters.with(index, |f| f.apply(&mut copy, strength, now)) {
                STATE.smoothed.fetch_add(1, Ordering::Relaxed);
                smoothed = Some(copy);
            }
        }
        if let Some(call) = original {
            match &smoothed {
                // SAFETY: calling the host's own method with a pose of the size it gave us.
                Some(copy) => unsafe { call(this, device, copy, pose_size) },
                None => unsafe { call(this, device, pose, pose_size) },
            }
        }
    }));
}

/// `this` is a host object; patch slot [`HOST_POSE_UPDATED_SLOT`] of its table to `detour::<INDEX>`, keeping the
/// original in `STATE.pose_originals[INDEX]`. Does nothing when already pointing at one of our detours.
///
/// SAFETY: `host` must be a live C++ object whose first word is its function table.
unsafe fn hook_host(setup: &mut Setup, host: *mut c_void, index: usize, detour_ptr: usize) -> bool {
    if host.is_null() {
        return false;
    }
    let entry = vtable_entry(host, HOST_POSE_UPDATED_SLOT);
    let current = *entry as usize;
    if is_our_pose_detour(current) {
        return false; // This table (shared by every host of this version) is already hooked.
    }
    if !patch_entry(entry, detour_ptr) {
        return false;
    }
    STATE.pose_originals[index].store(current, Ordering::Release);
    setup.patched.push((entry as usize, detour_ptr));
    true
}

/// The address of function-table slot `slot` of a C++ object.
///
/// SAFETY: `object` must point at a C++ object whose first word is its table.
unsafe fn vtable_entry(object: *mut c_void, slot: usize) -> *mut *mut c_void {
    let vtable = *(object as *const *mut *mut c_void);
    vtable.add(slot)
}

/// Writes `value` into a function-table slot, making the page writable around the write and putting its protection
/// back. Returns whether it succeeded.
///
/// SAFETY: `entry` must be a writable-after-`VirtualProtect` table slot.
unsafe fn patch_entry(entry: *mut *mut c_void, value: usize) -> bool {
    let size = std::mem::size_of::<usize>();
    let mut old = 0u32;
    if VirtualProtect(entry as *const c_void, size, PAGE_READWRITE, &mut old) == 0 {
        return false;
    }
    *entry = value as *mut c_void;
    let mut back = 0u32;
    // Tables sit in executable, read-only pages; restore whatever protection the loader gave them.
    VirtualProtect(entry as *const c_void, size, old, &mut back);
    true
}

/// Puts `original` back into a patched slot if it still holds our detour.
///
/// SAFETY: `entry` must be a table slot this crate patched.
unsafe fn restore_entry(entry: *mut *mut c_void, detour_ptr: usize, original: usize) {
    if *entry as usize == detour_ptr {
        patch_entry(entry, original);
    }
}

fn pose_detours() -> [usize; MAX_HOOKS] {
    [detour::<0> as *const () as usize, detour::<1> as *const () as usize]
}

fn is_our_pose_detour(ptr: usize) -> bool {
    pose_detours().contains(&ptr)
}

/// `GetGenericInterface(version)` through the saved context, calling the original if we hooked it.
///
/// SAFETY: a valid, NUL-terminated `version`.
unsafe fn get_interface(version: &CStr) -> *mut c_void {
    let context = STATE.context.load(Ordering::Acquire);
    let original = STATE.get_interface_original.load(Ordering::Acquire);
    if context.is_null() || original == 0 {
        return ptr::null_mut();
    }
    let call: GetGenericInterfaceFn = std::mem::transmute::<usize, GetGenericInterfaceFn>(original);
    call(context, version.as_ptr(), ptr::null_mut())
}

/// Hooks whichever server-driver hosts the context has (both share the one pose-update table entry we care about).
///
/// SAFETY: called from `Init` or the `GetGenericInterface` detour, with the context set.
unsafe fn install_host_hooks(setup: &mut Setup) {
    let detours = pose_detours();
    let mut hooked = STATE.hooks_installed.load(Ordering::Relaxed);
    for (index, version) in [SERVER_DRIVER_HOST_006, SERVER_DRIVER_HOST_005].into_iter().enumerate() {
        let host = get_interface(version);
        if !host.is_null() && hook_host(setup, host, index, detours[index]) {
            hooked += 1;
        }
    }
    STATE.hooks_installed.store(hooked, Ordering::Relaxed);
    if STATE.log.load(Ordering::Acquire).is_null() {
        STATE.log.store(get_interface(DRIVER_LOG), Ordering::Release);
    }
}

/// A line in SteamVR's `vrserver.txt`, prefixed with the driver name, when the host gave us a log. Best effort.
fn log_line(message: &str) {
    let log = STATE.log.load(Ordering::Acquire);
    if log.is_null() {
        return;
    }
    if let Ok(text) = std::ffi::CString::new(message) {
        // SAFETY: `IVRDriverLog::Log` is slot 0; `log` is the interface the host returned.
        unsafe {
            let entry = *(log as *const *mut c_void);
            let call: LogFn = std::mem::transmute::<*mut c_void, LogFn>(entry);
            call(log, text.as_ptr());
        }
    }
}

/// The `GetGenericInterface` detour: call the original, and the first time the server-driver host appears, hook it.
extern "system" fn get_interface_detour(this: *mut c_void, version: *const c_char, error: *mut i32) -> *mut c_void {
    let original: Option<GetGenericInterfaceFn> = {
        let bits = STATE.get_interface_original.load(Ordering::Acquire);
        (bits != 0).then(|| unsafe { std::mem::transmute::<usize, GetGenericInterfaceFn>(bits) })
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        let result = match original {
            // SAFETY: the context's own method, arguments passed straight through.
            Some(call) => unsafe { call(this, version, error) },
            None => ptr::null_mut(),
        };
        if !version.is_null() {
            // SAFETY: SteamVR passes a NUL-terminated interface version string.
            let asked = unsafe { CStr::from_ptr(version) };
            if asked == SERVER_DRIVER_HOST_006 || asked == SERVER_DRIVER_HOST_005 {
                if let Ok(mut setup) = STATE.setup.lock() {
                    // SAFETY: the context is set; hooking reads live host objects.
                    unsafe { install_host_hooks(&mut setup) };
                }
            }
        }
        result
    }));
    result.unwrap_or(ptr::null_mut())
}

// ---- IServerTrackedDeviceProvider, as a C++ object with its own table ----

extern "system" fn provider_init(_this: *mut c_void, context: *mut c_void) -> i32 {
    catch_unwind(|| {
        if context.is_null() {
            return INIT_INTERFACE_NOT_FOUND;
        }
        // SAFETY: `clock` is set here, once, before any pose flows.
        unsafe { *STATE.clock.get() = Some(Instant::now()) };
        STATE.context.store(context, Ordering::Release);
        let Ok(mut setup) = STATE.setup.lock() else {
            return openvr::INIT_NONE;
        };
        // Hook the context's own `GetGenericInterface` (slot 0) so we catch the hosts whenever they are requested,
        // then try the hosts now in case they already exist.
        // SAFETY: `context` is a live `IVRDriverContext`; its first word is its table.
        unsafe {
            let entry = vtable_entry(context, 0);
            let current = *entry as usize;
            if current != get_interface_detour as *const () as usize {
                STATE.get_interface_original.store(current, Ordering::Release);
                if patch_entry(entry, get_interface_detour as *const () as usize) {
                    STATE.get_interface_entry.store(entry, Ordering::Release);
                    setup.patched.push((entry as usize, get_interface_detour as *const () as usize));
                }
            }
            install_host_hooks(&mut setup);
        }
        try_open_control(&mut setup, true);
        log_line(&format!("Orbiters smoothing driver {DRIVER_VERSION}: {} host hook(s) installed", STATE.hooks_installed.load(Ordering::Relaxed)));
        INIT_NONE
    })
    .unwrap_or(INIT_INTERFACE_NOT_FOUND)
}

extern "system" fn provider_cleanup(_this: *mut c_void) {
    let _ = catch_unwind(|| {
        STATE.active.store(false, Ordering::Release);
        if let Ok(mut setup) = STATE.setup.lock() {
            let get_interface = get_interface_detour as *const () as usize;
            let pose = pose_detours();
            for (entry_addr, detour_ptr) in setup.patched.drain(..) {
                let entry = entry_addr as *mut *mut c_void;
                let original = if detour_ptr == get_interface {
                    STATE.get_interface_original.load(Ordering::Acquire)
                } else {
                    pose.iter().position(|&d| d == detour_ptr).map(|i| STATE.pose_originals[i].load(Ordering::Acquire)).unwrap_or(0)
                };
                if original != 0 {
                    // SAFETY: an entry this crate patched; restored only if it still holds our detour.
                    unsafe { restore_entry(entry, detour_ptr, original) };
                }
            }
            setup.control = None;
        }
        STATE.context.store(ptr::null_mut(), Ordering::Release);
        STATE.log.store(ptr::null_mut(), Ordering::Release);
        STATE.hooks_installed.store(0, Ordering::Relaxed);
    });
}

extern "system" fn provider_get_interface_versions(_this: *mut c_void) -> *const *const c_char {
    INTERFACE_VERSIONS.0.as_ptr()
}

extern "system" fn provider_run_frame(_this: *mut c_void) {
    let _ = catch_unwind(|| {
        if let Ok(mut setup) = STATE.setup.lock() {
            if setup.control.is_none() {
                let now = tick_ms();
                if now.saturating_sub(setup.last_open_attempt_ms) >= 1000 {
                    try_open_control(&mut setup, false);
                }
            }
            refresh_from_control(&setup);
        }
    });
}

extern "system" fn provider_should_block_standby(_this: *mut c_void) -> bool {
    false
}

extern "system" fn provider_enter_standby(_this: *mut c_void) {}

extern "system" fn provider_leave_standby(_this: *mut c_void) {}

static PROVIDER_VTABLE: ProviderVtable = ProviderVtable {
    init: provider_init,
    cleanup: provider_cleanup,
    get_interface_versions: provider_get_interface_versions,
    run_frame: provider_run_frame,
    should_block_standby_mode: provider_should_block_standby,
    enter_standby: provider_enter_standby,
    leave_standby: provider_leave_standby,
};

static PROVIDER: Object<ProviderVtable> = Object { vtable: &PROVIDER_VTABLE };

/// Opens the control block if the app has made it. `initial` is the first attempt in `Init`.
fn try_open_control(setup: &mut Setup, initial: bool) {
    setup.last_open_attempt_ms = tick_ms();
    let name = orbiters_smooth::win::wide(CONTROL_NAME);
    match Mapping::open(&name) {
        Ok(mapping) => {
            let valid = mapping.block().is_valid();
            setup.control = Some(mapping);
            if !valid {
                log_line("Orbiters smoothing: the control block is of another version; smoothing stays off");
            }
        }
        Err(_) if initial => {} // The app may not have started yet; RunFrame retries.
        Err(_) => {}
    }
}

/// Reads the app's settings into the lock-free cache and bumps the heartbeat. Smoothing goes inactive when the block
/// is gone, invalid, disabled, or its owner process has exited.
fn refresh_from_control(setup: &Setup) {
    let now = tick_ms();
    let Some(mapping) = setup.control.as_ref() else {
        STATE.active.store(false, Ordering::Release);
        return;
    };
    let block = mapping.block();
    let settings = block.settings();
    if let Some(settings) = &settings {
        for (cell, &s) in STATE.strengths.iter().zip(&settings.strengths) {
            cell.store(s.to_bits(), Ordering::Relaxed);
        }
    }
    let alive = owner_alive(block);
    let active = block.is_valid() && alive && settings.map(|s| s.enabled).unwrap_or(false);
    let was = STATE.active.swap(active, Ordering::Release);
    if was && !active {
        for i in 0..MAX_DEVICES {
            STATE.filters.reset(i);
        }
    }
    let report = DriverReport {
        version: DRIVER_VERSION,
        hooks: STATE.hooks_installed.load(Ordering::Relaxed),
        pid: orbiters_smooth::win::current_pid(),
        poses: STATE.poses.load(Ordering::Relaxed),
        smoothed: STATE.smoothed.load(Ordering::Relaxed),
    };
    block.report_driver(&report, now);
}

/// Whether the process that owns the control block is still running (so a crashed app stops smoothing). An owner of
/// 0 (not yet set) counts as alive.
fn owner_alive(block: &ControlBlock) -> bool {
    let pid = block.owner_pid.load(Ordering::Acquire);
    if pid == 0 {
        return true;
    }
    // SAFETY: OpenProcess with a pid and SYNCHRONIZE; the handle is closed before returning.
    unsafe {
        use orbiters_smooth::win::*;
        let handle = OpenProcess(SYNCHRONIZE, 0, pid);
        if handle == 0 {
            return false; // Gone, or not ours to open: treat as gone.
        }
        let alive = WaitForSingleObject(handle, 0) != WAIT_OBJECT_0;
        CloseHandle(handle);
        alive
    }
}

/// SteamVR's entry point: returns the `IServerTrackedDeviceProvider` when asked for it. `extern "system"` is the
/// x64 Windows calling convention. Never unwinds.
///
/// # Safety
/// `interface_name` is a NUL-terminated string from SteamVR; `return_code` is null or writable.
#[no_mangle]
pub unsafe extern "system" fn HmdDriverFactory(interface_name: *const c_char, return_code: *mut i32) -> *mut c_void {
    let result = catch_unwind(|| {
        if interface_name.is_null() {
            return ptr::null_mut();
        }
        // SAFETY: SteamVR passes a NUL-terminated interface name.
        let asked = unsafe { CStr::from_ptr(interface_name) };
        if asked == SERVER_TRACKED_DEVICE_PROVIDER {
            return &PROVIDER as *const _ as *mut c_void;
        }
        ptr::null_mut()
    })
    .unwrap_or(ptr::null_mut());
    if result.is_null() && !return_code.is_null() {
        // SAFETY: the caller promises `return_code` is writable when not null.
        unsafe { *return_code = INIT_INTERFACE_NOT_FOUND };
    }
    result
}
