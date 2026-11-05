//! Seeing every driver's poses: `IVRServerDriverHost::TrackedDevicePoseUpdated` is the call through which each
//! driver hands SteamVR a device's new pose. Every driver gets a host object of the same class from vrserver, so
//! they share its table of methods: pointing that table's `TrackedDevicePoseUpdated` entry at [`pose_updated`]
//! (the page made writable for the one store, then protected again) puts this driver between all of them and
//! SteamVR. The hook copies the pose, smooths the copy and calls the original function with it, or with the pose as
//! it came when there is nothing to do, so a device SteamVR is not asked to smooth sees exactly the same call.
//!
//! The pose threads read only this module's atomics (never the shared memory, which `RunFrame` may unmap), take no
//! lock that can wait (a device busy on another thread passes untouched) and allocate nothing.

use crate::filter::DeviceFilter;
use crate::openvr::{DriverPose, PoseUpdatedFn, HOST_POSE_UPDATED_SLOT, MAX_DEVICES};
use orbiters_smooth::win;
use std::cell::{Cell, UnsafeCell};
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

/// Host tables that can be hooked at once (`IVRServerDriverHost_006` and `_005`).
pub const HOOKS: usize = 2;

/// The original function behind each hook. Never cleared: a call that read the hooked entry just before `Cleanup`
/// put the original back still reaches SteamVR.
static ORIGINALS: [AtomicUsize; HOOKS] = [const { AtomicUsize::new(0) }; HOOKS];
/// The table entry each hook was written to (0: unused).
static ENTRIES: [AtomicUsize; HOOKS] = [const { AtomicUsize::new(0) }; HOOKS];

static ACTIVE: AtomicBool = AtomicBool::new(false);
static STRENGTHS: [AtomicU32; MAX_DEVICES] = [const { AtomicU32::new(0) }; MAX_DEVICES];
static POSES: AtomicU64 = AtomicU64::new(0);
static SMOOTHED: AtomicU64 = AtomicU64::new(0);
static EPOCH: OnceLock<Instant> = OnceLock::new();

thread_local! {
    /// Inside a hooked call on this thread: a host that forwards to another hooked host is smoothed once.
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
}

/// One device's filter, taken by one thread at a time without waiting.
struct Slot {
    busy: AtomicBool,
    filter: UnsafeCell<DeviceFilter>,
}

// SAFETY: `filter` is only touched by the thread that set `busy`.
unsafe impl Sync for Slot {}

static SLOTS: [Slot; MAX_DEVICES] = [const { Slot { busy: AtomicBool::new(false), filter: UnsafeCell::new(DeviceFilter::new()) } }; MAX_DEVICES];

struct Taken<'a>(&'a Slot);

impl Sl