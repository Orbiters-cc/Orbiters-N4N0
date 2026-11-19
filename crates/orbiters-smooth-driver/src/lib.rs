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
//! Normal local pose processing takes no blocking lock and allocates nothing. The foreign successor can block;
//! panic handling is best effort and does not catch access violations, stack overflow or arbitrary foreign exceptions.
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
// The release's version (`.cargo/config.toml` [env], set by `cargo xtask bump`): this DLL is a program of its own.
const DRIVER_VERSION: &str = env!("ORBITERS_VERSION");

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
    attached_mask: AtomicU32,
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
    pending_init: bool,
    init_us: u128,
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
    /// The last hook attempt's result said in SteamVR's log, so a retry a second that changes nothing says nothing (OVERLAY-5).
    hooks_said: Said,
    /// Said that a host's pose successor is ambiguous, per host (until a hook is installed again).
    ambiguous_said: [bool; MAX_HOOKS],
    /// The control block's owner, kept open so each frame's look is one wait, not open + wait + close.
    owner: OwnerWatch,
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
        struct Release<'a>(&'a AtomicBool);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _release = Release(busy);
        let result = f(unsafe { &mut *self.filters[index].get() });
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
            attached_mask: AtomicU32::new(0),
            get_interface_original: AtomicUsize::new(0),
            pose_originals: [ORIGINAL; MAX_HOOKS],
            context: AtomicPtr::new(ptr::null_mut()),
            log: AtomicPtr::new(ptr::null_mut()),
            setup: Mutex::new(Setup {
                pending_init: false,
                init_us: 0,
                control: None,
                last_open_attempt_ms: 0,
                last_hook_attempt_ms: 0,
                last_walk_check_ms: 0,
                walk_said: false,
                patched: Vec::new(),
                hooks_said: Said::new(),
                ambiguous_said: [false; MAX_HOOKS],
                owner: OwnerWatch::new(),
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
    static IN_POSE: Cell<*const PoseScope> = const { Cell::new(ptr::null()) };
}

struct PoseScope {
    previous: *const PoseScope,
    device: u32,
    pose: *const DriverPose,
}
impl Drop for PoseScope {
    fn drop(&mut self) {
        IN_POSE.with(|v| v.set(self.previous));
    }
}

fn forwarded_pose(device: u32, pose: *const DriverPose) -> bool {
    if pose.is_null() {
        return false;
    }
    let mut node = IN_POSE.with(Cell::get);
    for _ in 0..MAX_DEVICES {
        if node.is_null() {
            return false;
        }
        // Nodes and their poses live on this thread's synchronous callback stack.
        let scope = unsafe { &*node };
        if scope.device == device && !scope.pose.is_null() && unsafe { *scope.pose == *pose } {
            return true;
        }
        node = scope.previous;
    }
    true // bounded stack traversal; saturated forwarding passes through
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
/// x64 Windows convention C++ methods use; `this` comes first. Local processing allocates nothing and never waits;
/// the foreign successor and panic machinery have their own behavior.
extern "system" fn detour<const INDEX: usize>(this: *mut c_void, device: u32, pose: *const DriverPose, pose_size: u32) {
    let original: Option<PoseUpdatedFn> = {
        let bits = STATE.pose_originals[INDEX].load(Ordering::Acquire);
        // SAFETY: a function pointer this crate stored before it patched the table; never null once set.
        (bits != 0).then(|| unsafe { std::mem::transmute::<usize, PoseUpdatedFn>(bits) })
    };
    // Only our own struct layout is touched; any other size passes straight through.
    let sized = !pose.is_null() && pose_size as usize == std::mem::size_of::<DriverPose>();
    // Our own processing, guarded by itself: should it ever panic, the pose still reaches SteamVR, once, as it came, so a
    // mistake in smoothing or the walk costs smoothing, never tracking (REL-15).
    let processed = catch_unwind(AssertUnwindSafe(|| {
        processing_fault();
        let mut smoothed = None;
        let ours = sized && STATE.attached_mask.load(Ordering::Acquire) & (1 << INDEX) != 0 && !forwarded_pose(device, pose);
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
        smoothed
    }));
    let smoothed = processed.unwrap_or_else(|_| {
        if !PROCESSING_PANICKED.swap(true, Ordering::Relaxed) {
            log_line("Orbiters smoothing: processing a pose failed; poses go on to SteamVR as they came");
        }
        None
    });
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(call) = original {
            let outgoing = smoothed.as_ref().map_or(pose, |p| p as *const DriverPose);
            let scope = PoseScope { previous: IN_POSE.with(Cell::get), device, pose: if sized { outgoing } else { ptr::null() } };
            IN_POSE.with(|v| v.set(&scope));
            match &smoothed {
                // SAFETY: calling the host's own method with a pose of the size it gave us.
                Some(copy) => unsafe { call(this, device, copy, pose_size) },
                None => unsafe { call(this, device, pose, pose_size) },
            }
        }
    }));
}

/// Tests: a panic in the detour's processing, when asked for.
#[cfg(test)]
fn processing_fault() {
    if hook_tests::PANIC_IN_PROCESSING.load(Ordering::Relaxed) {
        panic!("test: processing panics");
    }
}

#[cfg(not(test))]
#[inline(always)]
fn processing_fault() {}

/// Said once in SteamVR's log that processing a pose failed.
static PROCESSING_PANICKED: AtomicBool = AtomicBool::new(false);

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
    let current = (&*entry.cast::<AtomicUsize>()).load(Ordering::Acquire);
    if setup.patched.iter().any(|&(address, detour)| address == entry as usize && detour == detour_ptr) {
        // An opaque foreign chain is not proof our callback is reachable.
        return current == detour_ptr;
    }
    if current == 0 || is_our_pose_detour(current) {
        return false; // This table (shared by every host of this version) is already hooked.
    }
    // Even successful restoration cannot revoke a pointer already fetched by another driver.
    // Reserve the successor forever, including when the installation CAS loses.
    if !reserve_successor(&STATE.pose_originals[index], current) {
        // Once per host, not at every retry a second.
        if !std::mem::replace(&mut setup.ambiguous_said[index], true) {
            log_line("Orbiters smoothing: ambiguous pose hook successor; restart SteamVR to reconnect");
        }
        return false;
    }
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

fn reserve_successor(original: &AtomicUsize, successor: usize) -> bool {
    match original.compare_exchange(0, successor, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => true,
        Err(saved) => saved == successor,
    }
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
    // Foreign page-protection writers must serialize with us. Pointer CAS cannot
    // coordinate their VirtualProtect calls; arbitrary add-on concurrency is not guaranteed.
    let (changed, fault) = exchange_protected(
        entry,
        expected,
        value,
        || {
            let mut info = MEMORY_BASIC_INFORMATION::zeroed();
            (VirtualQuery(entry.cast(), &mut info, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) != 0).then_some(info.Protect)
        },
        |protection| {
            let mut old = 0;
            (VirtualProtect(entry.cast(), size, protection, &mut old) != 0).then_some(old)
        },
    );
    if let Some(fault) = fault {
        log_line(&format!("Orbiters smoothing: hook page protection failure: {fault:?}"));
    }
    changed
}

#[derive(Debug, PartialEq)]
enum ProtectionFault {
    Query,
    Writable,
    Restore,
}

unsafe fn exchange_protected(
    entry: *mut *mut c_void,
    expected: usize,
    value: usize,
    query: impl FnOnce() -> Option<u32>,
    mut protect: impl FnMut(u32) -> Option<u32>,
) -> (bool, Option<ProtectionFault>) {
    let Some(protection) = query() else {
        return (false, Some(ProtectionFault::Query));
    };
    let executable = protection & (PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY) != 0;
    let Some(old) = protect(if executable { PAGE_EXECUTE_READWRITE } else { PAGE_READWRITE }) else {
        return (false, Some(ProtectionFault::Writable));
    };
    let changed = (&*entry.cast::<AtomicUsize>()).compare_exchange(expected, value, Ordering::AcqRel, Ordering::Acquire).is_ok();
    (changed, protect(old).is_none().then_some(ProtectionFault::Restore))
}

#[cfg(test)]
mod hook_tests {
    use super::*;
    static EMIT_OTHER_DEVICE: AtomicBool = AtomicBool::new(false);
    /// Makes the detour's own processing panic.
    pub(super) static PANIC_IN_PROCESSING: AtomicBool = AtomicBool::new(false);
    static RECEIVED: AtomicU32 = AtomicU32::new(0);
    static RECEIVED_X: AtomicU32 = AtomicU32::new(0);

    unsafe extern "system" fn receiver(_this: *mut c_void, _device: u32, pose: *const DriverPose, _size: u32) {
        RECEIVED.fetch_add(1, Ordering::Relaxed);
        RECEIVED_X.store(((*pose).vec_position[0] as f32).to_bits(), Ordering::Relaxed);
    }

    /// REL-15: processing that panics still hands SteamVR the pose, exactly once and as it came.
    #[test]
    fn a_panic_in_processing_still_forwards_the_raw_pose_once() {
        let _one = TEST_STATE.lock().unwrap();
        let previous = STATE.pose_originals[0].swap(receiver as *const () as usize, Ordering::AcqRel);
        STATE.attached_mask.store(1, Ordering::Release);
        STATE.clock.get_or_init(Instant::now);
        STATE.active.store(true, Ordering::Release);
        let pose = DriverPose::at([1.5, 2.0, 3.0], openvr::Quaternion::IDENTITY);
        RECEIVED.store(0, Ordering::Relaxed);
        PANIC_IN_PROCESSING.store(true, Ordering::Relaxed);
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        detour::<0>(ptr::null_mut(), 3, &pose, std::mem::size_of::<DriverPose>() as u32);
        std::panic::set_hook(hook);
        PANIC_IN_PROCESSING.store(false, Ordering::Relaxed);
        assert_eq!(RECEIVED.load(Ordering::Relaxed), 1, "the successor ran once");
        assert_eq!(f32::from_bits(RECEIVED_X.load(Ordering::Relaxed)), 1.5, "with the pose as it came");
        assert!(IN_POSE.with(Cell::get).is_null(), "scope resets after forwarding");
        // And without a panic, once too.
        detour::<0>(ptr::null_mut(), 3, &pose, std::mem::size_of::<DriverPose>() as u32);
        assert_eq!(RECEIVED.load(Ordering::Relaxed), 2);
        STATE.active.store(false, Ordering::Release);
        STATE.attached_mask.store(0, Ordering::Release);
        STATE.pose_originals[0].store(previous, Ordering::Release);
    }
    static TEST_STATE: Mutex<()> = Mutex::new(());

    unsafe extern "system" fn forward(this: *mut c_void, device: u32, pose: *const DriverPose, size: u32) {
        if EMIT_OTHER_DEVICE.load(Ordering::Relaxed) {
            detour::<1>(this, device + 1, pose, size);
        }
        detour::<1>(this, device, pose, size);
    }

    unsafe extern "system" fn terminal(_this: *mut c_void, _device: u32, _pose: *const DriverPose, _size: u32) {}

    #[test]
    fn saved_callback_cannot_acquire_a_successor_from_a_changed_host_table() {
        let _one = TEST_STATE.lock().unwrap();
        let previous = STATE.pose_originals[0].swap(0, Ordering::AcqRel);
        let mut table_a = [0usize, terminal as *const () as usize];
        let mut table_b = [0usize, forward as *const () as usize];
        let mut host_a = table_a.as_mut_ptr();
        let mut host_b = table_b.as_mut_ptr();
        let mut setup = Setup::default();
        let callback = pose_detours()[0];
        assert!(unsafe { hook_host(&mut setup, (&mut host_a as *mut *mut usize).cast(), 0, callback) });
        let saved = table_a[1];
        assert!(unsafe { restore_entry(table_a.as_mut_ptr().add(1).cast(), callback, terminal as *const () as usize) });
        setup.patched.clear(); // successful restoration cannot revoke `saved`
        assert_ne!(table_a.as_ptr(), table_b.as_ptr());
        assert!(!unsafe { hook_host(&mut setup, (&mut host_b as *mut *mut usize).cast(), 0, callback) });
        assert_eq!(STATE.pose_originals[0].load(Ordering::Acquire), terminal as *const () as usize);
        let call: PoseUpdatedFn = unsafe { std::mem::transmute(saved) };
        let pose = DriverPose::at([0.0; 3], openvr::Quaternion::IDENTITY);
        unsafe { call((&mut host_a as *mut *mut usize).cast(), 0, &pose, std::mem::size_of::<DriverPose>() as u32) };
        STATE.pose_originals[0].store(previous, Ordering::Release);
    }

    /// OVERLAY-5: a hook attempt a second with no host to hook says so once, not once a second; a change is said, with the count.
    #[test]
    fn repeated_hook_attempts_with_the_same_result_are_said_once() {
        let mut said = Said::new();
        assert_eq!(said.note(0), Some(0), "the first attempt is said");
        for _ in 0..3600 {
            assert_eq!(said.note(0), None);
        }
        assert_eq!(said.note(1), Some(3600), "a change is said, with the attempts not said");
        assert_eq!(said.note(1), None);
        assert_eq!(said.note(0), Some(1));
    }

    #[test]
    fn the_owner_is_opened_once_and_its_exit_still_reads_as_gone() {
        let mut watch = OwnerWatch::new();
        assert!(watch.alive(0), "no owner yet counts as alive");
        assert_eq!(watch.opens, 0);
        let me = std::process::id();
        for _ in 0..100 {
            assert!(watch.alive(me));
        }
        assert_eq!(watch.opens, 1, "one OpenProcess for a hundred frames");
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "pause"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("cmd starts");
        let pid = child.id();
        assert!(watch.alive(pid), "a running owner is alive");
        assert!(watch.alive(pid));
        assert_eq!(watch.opens, 2, "a new owner pid opens its own handle once");
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!watch.alive(pid), "an owner that exited reads as gone on the next frame");
        assert!(!watch.alive(pid));
        assert_eq!(watch.opens, 2);
        assert!(!watch.alive(u32::MAX - 3), "a pid that cannot be opened is gone");
        assert!(watch.held.is_none());
        assert!(watch.alive(0));
    }

    #[test]
    fn losing_installation_cas_does_not_release_the_reserved_successor() {
        let original = AtomicUsize::new(0);
        let mut slot = 111usize;
        assert!(reserve_successor(&original, slot));
        slot = 222; // another add-on publishes after the read, before our CAS
        assert!(!unsafe { patch_entry((&mut slot as *mut usize).cast(), 111, 333) });
        assert_eq!(slot, 222);
        assert!(!reserve_successor(&original, 222));
        assert_eq!(original.load(Ordering::Acquire), 111);
    }

    #[test]
    fn protection_query_fails_closed_and_restore_failure_is_not_lost() {
        let mut slot = 123usize;
        let entry = (&mut slot as *mut usize).cast();
        assert_eq!(
            unsafe { exchange_protected(entry, 123, 456, || None, |_| panic!("query failure must not change page protection")) },
            (false, Some(ProtectionFault::Query))
        );
        assert_eq!(slot, 123);
        let mut calls = Vec::new();
        let result = unsafe {
            exchange_protected(
                entry,
                123,
                456,
                || Some(PAGE_EXECUTE_READ),
                |mode| {
                    calls.push(mode);
                    (calls.len() == 1).then_some(PAGE_EXECUTE_READ)
                },
            )
        };
        assert_eq!(result, (true, Some(ProtectionFault::Restore)), "publication stays recorded even when restoring protection fails");
        assert_eq!(slot, 456);
        assert_eq!(calls, [PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_READ]);
    }

    #[test]
    fn published_successor_cannot_change_after_restore_changed_table_or_lost_cas() {
        let successor = AtomicUsize::new(0);
        assert!(reserve_successor(&successor, 123));
        assert!(reserve_successor(&successor, 123));
        // These represent a saved old callback, a different table for the same host
        // version, and an installation that lost its CAS. None grants quiescence.
        for candidate in [456, 789, 456] {
            assert!(!reserve_successor(&successor, candidate));
        }
        assert_eq!(successor.load(Ordering::Acquire), 123);
    }

    #[test]
    fn independent_nested_device_and_pose_are_processed_and_filter_panic_releases_slot() {
        let pose = DriverPose::at([1.0, 2.0, 3.0], openvr::Quaternion::IDENTITY);
        let scope = PoseScope { previous: ptr::null(), device: 1, pose: &pose };
        IN_POSE.with(|v| v.set(&scope));
        assert!(forwarded_pose(1, &pose));
        assert!(!forwarded_pose(2, &pose));
        let different = DriverPose::at([4.0, 2.0, 3.0], openvr::Quaternion::IDENTITY);
        assert!(!forwarded_pose(1, &different));
        drop(scope);
        let bank = FilterBank::new();
        assert!(catch_unwind(AssertUnwindSafe(|| bank.with(1, |_| panic!("test filter panic")))).is_err());
        assert!(bank.with(1, |_| ()).is_some());
    }

    #[test]
    fn versioned_hosts_forward_without_filtering_twice_and_restore_does_not_clobber_an_addon() {
        let _one = TEST_STATE.lock().unwrap();
        STATE.attached_mask.store(3, Ordering::Release);
        STATE.pose_originals[0].store(forward as *const () as usize, Ordering::Release);
        STATE.pose_originals[1].store(terminal as *const () as usize, Ordering::Release);
        STATE.clock.get_or_init(Instant::now);
        STATE.active.store(true, Ordering::Release);
        let pose = DriverPose::at([1.0, 2.0, 3.0], openvr::Quaternion::IDENTITY);
        let before = STATE.poses.load(Ordering::Relaxed);
        detour::<0>(ptr::null_mut(), 2, &pose, std::mem::size_of::<DriverPose>() as u32);
        assert_eq!(STATE.poses.load(Ordering::Relaxed), before + 1);
        EMIT_OTHER_DEVICE.store(true, Ordering::Relaxed);
        detour::<0>(ptr::null_mut(), 2, &pose, std::mem::size_of::<DriverPose>() as u32);
        EMIT_OTHER_DEVICE.store(false, Ordering::Relaxed);
        assert_eq!(STATE.poses.load(Ordering::Relaxed), before + 3, "a distinct nested device is processed too");
        assert!(IN_POSE.with(Cell::get).is_null(), "scope resets after forwarding");
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
    let mut hooked = 0;
    let mut mask = 0;
    for (index, version) in [SERVER_DRIVER_HOST_006, SERVER_DRIVER_HOST_005].into_iter().enumerate() {
        let host = get_interface(version);
        if !host.is_null() && hook_host(setup, host, index, detours[index]) {
            hooked += 1;
            mask |= 1 << index;
        }
    }
    STATE.hooks_installed.store(hooked, Ordering::Relaxed);
    STATE.attached_mask.store(mask, Ordering::Release);
    if hooked > 0 {
        setup.ambiguous_said = [false; MAX_HOOKS];
    }
    // The first attempt, and then only an attempt whose result differs from the last said (with how many were the same).
    if let Some(same) = setup.hooks_said.note(mask) {
        let again = if same > 0 { format!(" ({same} attempt(s) since with the same result)") } else { String::new() };
        log_line(&format!("Orbiters smoothing driver {DRIVER_VERSION}: smoothing or Fast walk turned on, {hooked} host hook(s) installed{again}"));
    }
}

/// A value said in a log only when it differs from the last said, counting the repeats not said.
#[derive(Default)]
struct Said {
    last: Option<u32>,
    repeats: u32,
}

impl Said {
    const fn new() -> Self {
        Self { last: None, repeats: 0 }
    }

    /// `Some(repeats not said since the last line)` when `value` is to be said now.
    fn note(&mut self, value: u32) -> Option<u32> {
        if self.last == Some(value) {
            self.repeats = self.repeats.saturating_add(1);
            return None;
        }
        self.last = Some(value);
        Some(std::mem::take(&mut self.repeats))
    }
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
    let began = Instant::now();
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
        let Ok(mut setup) = STATE.setup.lock() else {
            return INIT_NONE;
        };
        setup.pending_init = true;
        setup.init_us = began.elapsed().as_micros();
        INIT_NONE
    })
    .unwrap_or(INIT_INTERFACE_NOT_FOUND)
}

extern "system" fn provider_cleanup(_this: *mut c_void) {
    let _ = catch_unwind(|| {
        STATE.active.store(false, Ordering::Release);
        STATE.walk_alive.store(false, Ordering::Release);
        STATE.attached_mask.store(0, Ordering::Release);
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
            setup.owner.forget();
            setup.hooks_said = Said::new();
            setup.ambiguous_said = [false; MAX_HOOKS];
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
            if setup.pending_init {
                setup.pending_init = false;
                let began = Instant::now();
                STATE.log.store(unsafe { get_interface(DRIVER_LOG) }, Ordering::Release);
                try_open_control(&mut setup);
                refresh_from_control(&mut setup);
                log_line(&format!(
                    "Orbiters smoothing driver {DRIVER_VERSION}: loaded; Init {} us; first-frame setup {} us",
                    setup.init_us,
                    began.elapsed().as_micros()
                ));
                return;
            }
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

/// Revalidate exposed callbacks during the active lifecycle, not just Init.
/// An opaque/bypassed chain is not proof of reachability. No pointer is written;
/// retained records prevent install_host_hooks from blindly splicing it back in.
/// SAFETY: provider lifecycle owns these host tables until Cleanup.
unsafe fn revalidate_host_hooks(setup: &Setup) {
    let detours = pose_detours();
    let mut mask = 0u32;
    for &(address, detour) in &setup.patched {
        let current = (&*(address as *const AtomicUsize)).load(Ordering::Acquire);
        if current == detour {
            if let Some(index) = detours.iter().position(|&p| p == detour) {
                mask |= 1 << index;
            }
        }
    }
    let prior = STATE.attached_mask.swap(mask, Ordering::AcqRel);
    STATE.hooks_installed.store(mask.count_ones(), Ordering::Relaxed);
    if prior & !mask != 0 {
        log_line("Orbiters smoothing: live pose hook detached or obscured; that interface is passive until directly reachable");
    }
}

/// Reads the app's settings into the lock-free cache, hooks the hosts the first time smoothing is on, and bumps the
/// heartbeat. Smoothing goes inactive when the block is gone, invalid, disabled, or its owner process has exited.
fn refresh_from_control(setup: &mut Setup) {
    // SAFETY: RunFrame holds setup and provider lifecycle keeps host tables live.
    unsafe { revalidate_host_hooks(setup) };
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
    let alive = setup.owner.alive(block.owner_pid.load(Ordering::Acquire));
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

/// `WaitForSingleObject`'s "not signalled" (winerror.h).
const WAIT_TIMEOUT: u32 = 0x102;

/// Whether the process that owns the control block is still running (so a crashed app stops smoothing), asked every
/// SteamVR frame. The owner's handle is opened once per owner pid and kept: a frame's look is then one
/// `WaitForSingleObject(h, 0)` instead of `OpenProcess` + wait + `CloseHandle` (audit O10, LIGHT-VR-6). The answer is the
/// same on every frame as opening anew would give, and a pid reused after the owner died can no longer read as alive (the
/// held handle stays signalled). An owner of 0 (not yet set) counts as alive.
#[derive(Default)]
struct OwnerWatch {
    /// The pid the handle is for, and the handle (`SYNCHRONIZE` only).
    held: Option<(u32, orbiters_smooth::win::HANDLE)>,
    /// How many times a handle was opened (tests).
    opens: u32,
}

// SAFETY: a process handle is a kernel object reference, usable from any thread; it is only touched under `STATE.setup`.
unsafe impl Send for OwnerWatch {}

impl OwnerWatch {
    const fn new() -> Self {
        Self { held: None, opens: 0 }
    }

    fn alive(&mut self, pid: u32) -> bool {
        use orbiters_smooth::win::*;
        if pid == 0 {
            self.forget();
            return true;
        }
        if let Some((held, handle)) = self.held {
            if held == pid {
                // SAFETY: a handle this watch opened and has not closed.
                match unsafe { WaitForSingleObject(handle, 0) } {
                    WAIT_OBJECT_0 => return false,
                    WAIT_TIMEOUT => return true,
                    // A wait that failed says nothing: the handle goes and the owner is looked at anew, as every frame did.
                    _ => {}
                }
            }
        }
        self.forget();
        // SAFETY: OpenProcess with a pid and SYNCHRONIZE; the handle is closed in `forget`.
        let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
        if handle == 0 {
            return false; // Gone, or not ours to open: treat as gone (and try again next frame, as before).
        }
        self.opens += 1;
        // SAFETY: the handle just opened. Any answer but "signalled" was "alive" before, and still is.
        let alive = unsafe { WaitForSingleObject(handle, 0) } != WAIT_OBJECT_0;
        // While it is held the owner's pid cannot be given to another process, so a later owner with the same number is
        // the same process.
        self.held = Some((pid, handle));
        alive
    }

    /// Closes the held handle, if any.
    fn forget(&mut self) {
        if let Some((_, handle)) = self.held.take() {
            // SAFETY: a handle this watch opened, closed once.
            unsafe { orbiters_smooth::win::CloseHandle(handle) };
        }
    }
}

impl Drop for OwnerWatch {
    fn drop(&mut self) {
        self.forget();
    }
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
