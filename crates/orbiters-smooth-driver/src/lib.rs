//! Orbiters - N4N0's SteamVR server driver for tracking smoothing (`driver_orbiters_smooth.dll`), an equivalent of
//! OVR-SmoothTracking. SteamVR loads it into `vrserver.exe` at every start (the app registers it once, and it stays
//! registered); it asks the server driver host for every driver's pose updates by patching one entry of the host's
//! function table (as OpenVR-SpaceCalibrator does to read poses), eases the position and rotation of the devices the
//! app marks (`orbiters_smooth::filter`) and damps the velocities SteamVR predicts with, then hands the pose on. It
//! reads what to do, without locks, from the control block the app shares (`orbiters_smooth`), every SteamVR frame:
//! the app turns smoothing on and off and changes its strengths live, never a restart.
//!
//! Until smoothing is first turned on it touches nothing of SteamVR's: no table is patched, every pose goes from
//! driver to SteamVR as if it were not loaded. The first frame smoothing is on (from the control block) it hooks the
//! hosts, and stays hooked, passing poses on untouched while off, until SteamVR ends it (`Cleanup`).
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

/// Hooking found no host to patch: tried again this often (milliseconds) while smoothing is on.
const HOOK_RETRY_MS: u64 = 1000;

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
    /// The context's `GetGenericInterface` (slot 0 of its table, read in `Init`, never patched).
    get_interface_original: AtomicUsize,
    /// Pose-update originals the detours call through, one per hook index. Stored before the table entry is
    /// patched, so a pose that reaches a detour always finds its original.
    pose_originals: [AtomicUsize; MAX_HOOKS],
    /// `IVRDriverContext*`, to fetch the hosts when smoothing is first turned on.
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
    /// `GetTickCount64()` at the last attempt to hook the hosts (0: never).
    last_hook_attempt_ms: u64,
    /// Patched vtable entries to restore: (entry address, the detour we wrote).
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
            get_interface_original: AtomicUsize::new(0),
            pose_originals: [ORIGINAL; MAX_HOOKS],
            context: AtomicPtr::new(ptr::null_mut()),
            log: AtomicPtr::new(ptr::null_mut()),
            setup: Mutex::new(Setup { control: None, last_open_attempt_ms: 0, last_hook_attempt_ms: 0, patched: Vec::new() }),
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

/// The control block's name (`CONTROL_NAME`), NUL-terminated UTF-16, unless a test chose another.
static CONTROL_NAME_OVERRIDE: Mutex<Option<Vec<u16>>> = Mutex::new(None);

/// Tests only: the driver opens the control block under `name` instead of `Local\OrbitersSmoothTracking`, so a test
/// never reaches the block of an Orbiters - N4N0 running on the same computer.
#[doc(hidden)]
pub fn use_control_name_for_tests(name: &str) {
    *CONTROL_NAME_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()) = Some(orbiters_smooth::win::wide(name));
}

/// The pose hook for one host index: copy the pose, smooth it, call the host's original. `extern "system"` is the
/// x64 Windows convention C++ methods use; `this` comes first. Never unwinds, never blocks, allocates nothing.
extern "system" fn detour<const INDEX: usize>(this: *mut c_void, device: u32, pose: *const DriverPose, pose_size: u32) {
    let original: Option<PoseUpdatedFn> = {
        let bits = STATE.pose_originals[INDEX].load(Ordering::Acquire);
        // SAFETY: a function pointer this crate stored before it patched the table; never null once set.
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
/// original in `STATE.pose_originals[INDEX]` (stored first: poses flow on other threads while this runs). Does
/// nothing when already pointing at one of our detours.
///
/// SAFETY: `host` must be a live C++ object whose first word is its function table.
unsafe fn hook_host(setup: &mut Setup, host: *mut c_void, index: usize, detour_ptr: usize) -> bool {
    if host.is_null() {
        return false;
    }
    let entry = vtable_entry(host, HOST_POSE_UPDATED_SLOT);
    let current = *entry as usize;
    if current == 0 || is_our_pose_detour(current) {
        return false; // This table (shared by every host of this version) is already hooked.
    }
    STATE.pose_originals[index].store(current, Ordering::Release);
    if !patch_entry(entry, detour_ptr) {
        return false;
    }
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

/// Writes `value` into a function-table slot (one aligned pointer-sized store, so a thread calling through it sees
/// the old function or the new one), making the page writable around the write and putting its protection back.
/// Returns whether it succeeded.
///
/// SAFETY: `entry` must be a writable-after-`VirtualProtect` table slot.
unsafe fn patch_entry(entry: *mut *mut c_void, value: usize) -> bool {
    let size = std::mem::size_of::<usize>();
    let mut old = 0u32;
    if VirtualProtect(entry as *const c_void, size, PAGE_READWRITE, &mut old) == 0 {
        return false;
    }
    std::ptr::write_volatile(entry, value as *mut c_void);
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

/// `GetGenericInterface(version)` through the saved context.
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

/// Hooks whichever server-driver hosts the context has. Every driver's host of one version is an object of the same
/// class, so patching the table of the one we are given puts the hook between all of them and SteamVR.
///
/// SAFETY: called from `Init` or `RunFrame`, with the context set.
unsafe fn install_host_hooks(setup: &mut Setup) {
    setup.last_hook_attempt_ms = tick_ms().max(1);
    let detours = pose_detours();
    let mut hooked = STATE.hooks_installed.load(Ordering::Relaxed);
    for (index, version) in [SERVER_DRIVER_HOST_006, SERVER_DRIVER_HOST_005].into_iter().enumerate() {
        let host = get_interface(version);
        if !host.is_null() && hook_host(setup, host, index, detours[index]) {
            hooked += 1;
        }
    }
    STATE.hooks_installed.store(hooked, Ordering::Relaxed);
    log_line(&format!("Orbiters smoothing driver {DRIVER_VERSION}: smoothing turned on, {hooked} host hook(s) installed"));
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

// ---- IServerTrackedDeviceProvider, as a C++ object with its own table ----

extern "system" fn provider_init(_this: *mut c_void, context: *mut c_void) -> i32 {
    catch_unwind(|| {
        if context.is_null() {
            return INIT_INTERFACE_NOT_FOUND;
        }
        // SAFETY: `clock` is set here, once, before any pose flows.
        unsafe { *STATE.clock.get() = Some(Instant::now()) };
        // SAFETY: `context` is a live `IVRDriverContext`; its first word is its table, `GetGenericInterface` slot 0.
        // Only read: nothing of SteamVR's is patched until smoothing is turned on.
        let get_interface_fn = unsafe { *vtable_entry(context, 0) as usize };
        STATE.get_interface_original.store(get_interface_fn, Ordering::Release);
        STATE.context.store(context, Ordering::Release);
        // SAFETY: the context is set.
        STATE.log.store(unsafe { get_interface(DRIVER_LOG) }, Ordering::Release);
        let Ok(mut setup) = STATE.setup.lock() else {
            return INIT_NONE;
        };
        try_open_control(&mut setup);
        log_line(&format!("Orbiters smoothing driver {DRIVER_VERSION}: loaded, waiting for Orbiters - N4N0 to turn smoothing on"));
        refresh_from_control(&mut setup);
        INIT_NONE
    })
    .unwrap_or(INIT_INTERFACE_NOT_FOUND)
}

extern "system" fn provider_cleanup(_this: *mut c_void) {
    let _ = catch_unwind(|| {
        STATE.active.store(false, Ordering::Release);
        if let Ok(mut setup) = STATE.setup.lock() {
            let pose = pose_detours();
            for (entry_addr, detour_ptr) in setup.patched.drain(..) {
                let entry = entry_addr as *mut *mut c_void;
                let original = pose.iter().position(|&d| d == detour_ptr).map(|i| STATE.pose_originals[i].load(Ordering::Acquire)).unwrap_or(0);
                if original != 0 {
                    // SAFETY: an entry this crate patched; restored only if it still holds our detour.
                    unsafe { restore_entry(entry, detour_ptr, original) };
                }
            }
            setup.control = None;
            setup.last_hook_attempt_ms = 0;
        }
        STATE.context.store(ptr::null_mut(), Ordering::Release);
        STATE.get_interface_original.store(0, Ordering::Release);
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
                    try_open_control(&mut setup);
                }
            }
            refresh_from_control(&mut setup);
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

/// Opens the control block if the app has made it (else `RunFrame` tries again a second later).
fn try_open_control(setup: &mut Setup) {
    setup.last_open_attempt_ms = tick_ms();
    let name = CONTROL_NAME_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| orbiters_smooth::win::wide(CONTROL_NAME));
    if let Ok(mapping) = Mapping::open(&name) {
        let valid = mapping.block().is_valid();
        setup.control = Some(mapping);
        if !valid {
            log_line("Orbiters smoothing: the control block is of another version; smoothing stays off");
        }
    }
}

/// Reads the app's settings into the lock-free cache, hooks the hosts the first time smoothing is on, and bumps the
/// heartbeat. Smoothing goes inactive when the block is gone, invalid, disabled, or its owner process has exited.
fn refresh_from_control(setup: &mut Setup) {
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
    let wanted = block.is_valid() && alive && settings.map(|s| s.enabled).unwrap_or(false);
    // Turned on: the hosts are hooked now (once; again a second later if none could be).
    if wanted
        && STATE.hooks_installed.load(Ordering::Relaxed) == 0
        && (setup.last_hook_attempt_ms == 0 || now.saturating_sub(setup.last_hook_attempt_ms) >= HOOK_RETRY_MS)
    {
        // SAFETY: the context is set from `Init` until `Cleanup`, which runs under the same lock.
        unsafe { install_host_hooks(setup) };
    }
    let active = wanted && STATE.hooks_installed.load(Ordering::Relaxed) > 0;
    let was = STATE.active.swap(active, Ordering::Release);
    if was && !active {
        for i in 0..MAX_DEVICES {
            STATE.filters.reset(i);
        }
    }
    let Some(mapping) = setup.control.as_ref() else { return };
    let report = DriverReport {
        version: DRIVER_VERSION,
        hooks: STATE.hooks_installed.load(Ordering::Relaxed),
        pid: orbiters_smooth::win::current_pid(),
        poses: STATE.poses.load(Ordering::Relaxed),
        smoothed: STATE.smoothed.load(Ordering::Relaxed),
    };
    mapping.block().report_driver(&report, now);
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
