//! Orbiters - N4N0's SteamVR server driver for tracking smoothing (`driver_orbiters_smooth.dll`), an equivalent of
//! OVR-SmoothTracking. SteamVR loads it into `vrserver.exe` at every start (the app registers it once, and it stays
//! registered); it asks the server driver host for every driver's pose updates by patching one entry of the host's
//! function table (as OpenVR-SpaceCalibrator does to read poses), eases the position and rotation of the devices the
//! app marks (`orbiters_smooth::filter`) and damps the velocities SteamVR predicts with, then hands the pose on. It
//! reads what to do, without locks, from the control block the app shares (`orbiters_smooth`), every SteamVR frame:
//! the app turns smoothing on and off and changes its strengths live, never a restart.
//!
//! It also moves every device for Fast walk: the SteamVR overlay writes how far it carried the person and how fast
//! (`orbiters_smooth::WalkLink`), and each pose gets that added after the smoothing, so the smoothing never sees it. Applications
//! that move SteamVR's play space (OVR Advanced Settings) then simply add up with the walk.
//!
//! Until smoothing is first turned on, or Fast walk first asks, it touches nothing of SteamVR's: no table is patched, every
//! pose goes from driver to SteamVR as if it were not loaded. The first frame either is on (from the control block) it hooks
//! the hosts, and stays hooked, passing poses on untouched while off. Cleanup restores exposed entries; if another
//! add-on chained ours, the pinned DLL remains a passive forwarding link until vrserver exits.
//!
//! Across the FFI boundary it never unwinds (every entry catches panics) and never blocks the pose thread (the hook
//! takes no lock that another pose or a frame could hold, and allocates nothing per pose).
//!
//! The vtable slot it patches, the interface version strings and the `DriverPose_t` layout come from Valve's
//! `openvr_driver.h` (`openvr.rs`); they can only be confirmed against a running SteamVR.

#![cfg(target_os = "windows")]

pub mod filter;
pub mod openvr;
pub mod walk;

use filter::DeviceFilter;
use openvr::{
    DriverPose, GetGenericInterfaceFn, LogFn, Object, PoseUpdatedFn, ProviderVtable, DRIVER_LOG, HOST_POSE_UPDATED_SLOT, INIT_INTERFACE_NOT_FOUND, INIT_NONE,
    INTERFACE_VERSIONS, MAX_DEVICES, SERVER_DRIVER_HOST_005, SERVER_DRIVER_HOST_006, SERVER_TRACKED_DEVICE_PROVIDER,
};
use orbiters_smooth::win::{
    tick_ms, VirtualProtect, VirtualQuery, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY,
    PAGE_READWRITE,
};
use orbiters_smooth::{ControlBlock, DriverReport, Mapping, CONTROL_NAME};
use std::cell::{Cell, UnsafeCell};
use std::ffi::{c_char, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The two server-driver-host versions whose pose updates we filter (`IVRServerDriverHost_006`, and `_005` for
/// older drivers). Each has its own function table; each is hooked at most once.
const MAX_HOOKS: usize = 2;

/// Hooking found no host to patch: tried again this often (milliseconds) while smoothing is on.
const HOOK_RETRY_MS: u64 = 1000;

/// Whether the process writing Fast walk still runs is looked at this often (milliseconds).
const WALK_WRITER_CHECK_MS: u64 = 500;

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
    /// Poses moved for Fast walk.
    walked: AtomicU64,
    /// The control block for the pose hook, which can take no lock: a view of it that stays mapped until the process ends.
    walk_block: AtomicPtr<ControlBlock>,
    /// The process writing the walk runs: without it, nothing is added.
    walk_alive: AtomicBool,
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
    clock: OnceLock<Instant>,
}

// SAFETY: filters guard their cells; every other field is atomic, OnceLock or Mutex.
unsafe impl Sync for State {}

#[derive(Default)]
struct Setup {
    /// The control block, open while the app's lives.
    control: Option<Mapping>,
    /// `GetTickCount64()` at the last attempt to open it (retry about once a second).
    last_open_attempt_ms: u64,
    /// `GetTickCount64()` at the last attempt to hook the hosts (0: never).
    last_hook_attempt_ms: u64,
    /// `GetTickCount64()` at the last look at the process writing the walk.
    last_walk_check_ms: u64,
    /// Said in SteamVR's log that Fast walk moves poses.
    walk_said: bool,
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
            walked: AtomicU64::new(0),
            walk_block: AtomicPtr::new(ptr::null_mut()),
            walk_alive: AtomicBool::new(false),
            hooks_installed: AtomicU32::new(0),
            get_interface_original: AtomicUsize::new(0),
            pose_originals: [ORIGINAL; MAX_HOOKS],
            context: AtomicPtr::new(ptr::null_mut()),
            log: AtomicPtr::new(ptr::null_mut()),
            setup: Mutex::new(Setup {
                control: None,
                last_open_attempt_ms: 0,
                last_hook_attempt_ms: 0,
                last_walk_check_ms: 0,
                walk_said: false,
                patched: Vec::new(),
            }),
            clock: OnceLock::new(),
        }
    }

    fn now_seconds(&self) -> f64 {
        match self.clock.get() {
            Some(start) => start.elapsed().as_secs_f64(),
            None => 0.0,
        }
    }
}

static STATE: State = State::new();

thread_local! {
    // A versioned host may forward to another host we hooked. Apply smoothing/walk once.
    static IN_POSE: Cell<bool> = const { Cell::new(false) };
}

struct PoseScope(bool);
impl Drop for PoseScope {
    fn drop(&mut self) {
        IN_POSE.with(|v| v.set(self.0));
    }
}

/// What Fast walk adds now, for a pose hook: nothing unless the process writing it runs and it is not zero.
fn walk_now() -> Option<orbiters_smooth::WalkNow> {
    if !STATE.walk_alive.load(Ordering::Relaxed) {
        return None;
    }
    let block = STATE.walk_block.load(Ordering::Relaxed);
    if block.is_null() {
        return None;
    }
    // SAFETY: a view leaked when it was made: it stays mapped until the process ends. Every field is atomic.
    let (walk, at) = unsafe { &*block }.walk()?;
    let now = walk.after(orbiters_smooth::win::micros().wrapping_sub(at));
    (!now.is_zero()).then_some(now)
}

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
        let nested = IN_POSE.with(|v| v.replace(true));
        let _scope = PoseScope(nested);
        let mut smoothed = None;
        // Only our own struct layout is touched; any other size passes straight through.
        let ours = !nested && !pose.is_null() && pose_size as usize == std::mem::size_of::<DriverPose>();
        if ours && STATE.active.load(Ordering::Relaxed) {
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
        // Fast walk, after the smoothing: it eases each device's own motion, and the walk is not a device's motion.
        if ours {
            if let Some(now) = walk_now() {
                // SAFETY: SteamVR passes a readable `DriverPose_t` of the size checked above.
                let mut moved = smoothed.unwrap_or_else(|| unsafe { ptr::read(pose) });
                walk::apply(&mut moved, &now);
                STATE.walked.fetch_add(1, Ordering::Relaxed);
                smoothed = Some(moved);
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
    // A later add-on may still chain our detour after Cleanup. Its original must stay immutable.
    if setup.patched.iter().any(|&(address, detour)| address == entry as usize && detour == detour_ptr) {
        return true;
    }
    let current = (&*entry.cast::<AtomicUsize>()).load(Ordering::Acquire);
    if current == 0 || is_our_pose_detour(current) {
        return false; // This table (shared by every host of this version) is already hooked.
    }
    STATE.pose_originals[index].store(current, Ordering::Release);
    // A chained callback can outlive Cleanup: pin this small DLL until vrserver exits.
    use orbiters_smooth::win::{GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN};
    let mut module = 0;
    if GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN, detour_ptr as *const u16, &mut module) == 0 {
        return false;
    }
    if !patch_entry(entry, current, detour_ptr) {
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

/// Replaces `expected` with `value` atomically in a function-table slot, so a competing add-on is not overwritten.
/// A thread calling through it sees the old function or the new one. Makes the page writable around the write,
/// then puts its protection back.
/// Returns whether it succeeded.
///
/// SAFETY: `entry` must be a writable-after-`VirtualProtect` table slot.
unsafe fn patch_entry(entry: *mut *mut c_void, expected: usize, value: usize) -> bool {
    let size = std::mem::size_of::<usize>();
    // A table on a page that also runs code (one another hooking driver made) keeps running while it is written.
    let mut info = MEMORY_BASIC_INFORMATION::zeroed();
    let executable = VirtualQuery(entry as *const c_void, &mut info, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) != 0
        && info.Protect & (PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY) != 0;
    let writable = if executable { PAGE_EXECUTE_READWRITE } else { PAGE_READWRITE };
    let mut old = 0u32;
    if VirtualProtect(entry as *const c_void, size, writable, &mut old) == 0 {
        return false;
    }
    let changed = (&*entry.cast::<AtomicUsize>()).compare_exchange(expected, value, Ordering::AcqRel, Ordering::Acquire).is_ok();
    let mut back = 0u32;
    // Tables sit in executable, read-only pages; restore whatever protection the loader gave them.
    VirtualProtect(entry as *const c_void, size, old, &mut back);
    changed
}

#[cfg(test)]
mod hook_tests {
    use super::*;

    unsafe extern "system" fn forward(this: *mut c_void, device: u32, pose: *const DriverPose, size: u32) {
        detour::<1>(this, device, pose, size);
    }

    unsafe extern "system" fn terminal(_this: *mut c_void, _device: u32, _pose: *const DriverPose, _size: u32) {}

    #[test]
    fn versioned_hosts_forward_without_filtering_twice_and_restore_does_not_clobber_an_addon() {
        STATE.pose_originals[0].store(forward as *const () as usize, Ordering::Release);
        STATE.pose_originals[1].store(terminal as *const () as usize, Ordering::Release);
        STATE.clock.get_or_init(Instant::now);
        STATE.active.store(true, Ordering::Release);
        let pose = DriverPose::at([1.0, 2.0, 3.0], openvr::Quaternion::IDENTITY);
        let before = STATE.poses.load(Ordering::Relaxed);
        detour::<0>(ptr::null_mut(), 2, &pose, std::mem::size_of::<DriverPose>() as u32);
        assert_eq!(STATE.poses.load(Ordering::Relaxed), before + 1);
        assert!(!IN_POSE.with(Cell::get), "scope resets after forwarding");
        STATE.active.store(false, Ordering::Release);
        let mut slot = 123usize;
        let entry = (&mut slot as *mut usize).cast();
        assert!(!unsafe { restore_entry(entry, 456, 789) });
        assert_eq!(slot, 123, "a later add-on is preserved");
        assert!(unsafe { restore_entry(entry, 123, 789) });
        assert_eq!(slot, 789);
    }
}

/// Puts `original` back into a patched slot if it still holds our detour.
///
/// SAFETY: `entry` must be a table slot this crate patched.
unsafe fn restore_entry(entry: *mut *mut c_void, detour_ptr: usize, original: usize) -> bool {
    patch_entry(entry, detour_ptr, original)
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
    log_line(&format!("Orbiters smoothing driver {DRIVER_VERSION}: smoothing or Fast walk turned on, {hooked} host hook(s) installed"));
}

/// A line in SteamVR's `vrserver.txt`, prefixed with the driver name, when the host gave us a log. Best effort.
fn log_line(message: &str) {
    let log = STATE.log.load(Ordering::Acquire);
    if log.is_null() {
        return;
    }
    if let Ok(text) = std::ffi::CString::new(message) {
        // SAFETY: `log` is the interface the host returned: its first word is its table, `IVRDriverLog::Log` slot 0.
        // (The function is the table's first entry, not the table: calling the table itself ran SteamVR's read-only
        // data and crashed vrserver.exe at the driver's first line, sending SteamVR to Safe Mode up to 0.3.2-dev.5.)
        unsafe {
            let entry = *vtable_entry(log, 0);
            if entry.is_null() {
                return;
            }
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
        STATE.clock.get_or_init(Instant::now);
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
        STATE.walk_alive.store(false, Ordering::Release);
        if let Ok(mut setup) = STATE.setup.lock() {
            let pose = pose_detours();
            setup.patched.retain(|&(entry_addr, detour_ptr)| {
                let entry = entry_addr as *mut *mut c_void;
                let original = pose.iter().position(|&d| d == detour_ptr).map(|i| STATE.pose_originals[i].load(Ordering::Acquire)).unwrap_or(0);
                if original != 0 {
                    // SAFETY: an entry this crate patched; restored only if it still holds our detour.
                    return !unsafe { restore_entry(entry, detour_ptr, original) };
                }
                true
            });
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
        // A second view for the pose hook, which cannot take the lock `control` is behind and may run during `Cleanup`:
        // never unmapped. One small view, once.
        if STATE.walk_block.load(Ordering::Acquire).is_null() {
            if let Ok(view) = Mapping::open(&name) {
                let view: &'static Mapping = Box::leak(Box::new(view));
                STATE.walk_block.store(view.block() as *const ControlBlock as *mut ControlBlock, Ordering::Release);
            }
        }
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
    // Fast walk has its own writer, the overlay: when that process goes, whatever it left is no longer added.
    if now.saturating_sub(setup.last_walk_check_ms) >= WALK_WRITER_CHECK_MS {
        setup.last_walk_check_ms = now;
        let pid = block.walk_pid.load(Ordering::Acquire);
        let writer = block.is_valid() && pid != 0 && process_alive(pid);
        if STATE.walk_alive.swap(writer, Ordering::Release) && !writer {
            log_line("Orbiters smoothing: the process writing Fast walk is gone, nothing is added to poses any more");
        }
    }
    let walk_wanted = block.walk_on.load(Ordering::Relaxed) != 0 && STATE.walk_alive.load(Ordering::Relaxed);
    // Turned on: the hosts are hooked now (once; again a second later if none could be).
    if (wanted || walk_wanted)
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
    let walked = STATE.walked.load(Ordering::Relaxed);
    if walked > 0 && !setup.walk_said {
        setup.walk_said = true;
        log_line("Orbiters smoothing: Fast walk moves poses");
    }
    let Some(mapping) = setup.control.as_ref() else { return };
    let report = DriverReport {
        version: DRIVER_VERSION,
        hooks: STATE.hooks_installed.load(Ordering::Relaxed),
        pid: orbiters_smooth::win::current_pid(),
        poses: STATE.poses.load(Ordering::Relaxed),
        smoothed: STATE.smoothed.load(Ordering::Relaxed),
        walked,
    };
    mapping.block().report_driver(&report, now);
}

/// Whether the process that owns the control block is still running (so a crashed app stops smoothing). An owner of
/// 0 (not yet set) counts as alive.
fn owner_alive(block: &ControlBlock) -> bool {
    let pid = block.owner_pid.load(Ordering::Acquire);
    pid == 0 || process_alive(pid)
}

/// Whether process `pid` runs.
fn process_alive(pid: u32) -> bool {
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
