//! End to end over the real FFI: build fake SteamVR objects (an `IVRDriverContext` and an `IVRServerDriverHost`,
//! each a C++ object with its own function table), hand them to the provider that `HmdDriverFactory` returns, and
//! check that it touches nothing while smoothing is off, hooks the host's table on the first frame smoothing is on
//! (live, no restart), that a pose sent through the patched table reaches the fake host's original smoothed, that
//! turning smoothing off passes poses on untouched, and that `Cleanup` puts the table back.
//!
//! The control block is one of the test's own (`use_control_name_for_tests`), never the one of an Orbiters - N4N0
//! running on this computer.
//!
//! The context also gives a fake `IVRDriverLog` laid out as SteamVR's (an object whose first word is its table): the
//! driver's lines must reach its `Log`, as calling anything else crashes `vrserver.exe` (it did, up to 0.3.2-dev.5).
//!
//! Fast walk the same way: the overlay's side of the block asks, the host is hooked with smoothing off, a pose reaches the
//! host's original moved by the walk, and nothing is added once the walk is zero or its writer has gone.
//!
//! Runs on Windows only; the driver is Windows-only (it patches a vtable with `VirtualProtect`).
#![cfg(target_os = "windows")]

use driver_orbiters_smooth::openvr::{DriverPose, GetGenericInterfaceFn, Object, PoseUpdatedFn, Quaternion, HOST_POSE_UPDATED_SLOT};
use driver_orbiters_smooth::HmdDriverFactory;
use orbiters_smooth::{Control, MAX_DEVICES};
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

// ---- A fake IVRServerDriverHost: only TrackedDeviceAdded (slot 0) and TrackedDevicePoseUpdated (slot 1) matter ----

/// What the fake host's original `TrackedDevicePoseUpdated` received last.
static LAST_POSE: Mutex<Option<(u32, DriverPose)>> = Mutex::new(None);

#[repr(C)]
struct HostVtable {
    tracked_device_added: extern "system" fn(*mut c_void, *const c_char, i32, *mut c_void) -> bool,
    pose_updated: PoseUpdatedFn,
    // The real interface has more methods; the driver only reaches slot 1, so stubs beyond it are unneeded.
}

extern "system" fn host_tracked_device_added(_this: *mut c_void, _serial: *const c_char, _class: i32, _driver: *mut c_void) -> bool {
    true
}

extern "system" fn host_pose_updated(_this: *mut c_void, device: u32, pose: *const DriverPose, _size: u32) {
    // SAFETY: the driver passes a readable `DriverPose` of the right size.
    let pose = unsafe { *pose };
    *LAST_POSE.lock().unwrap() = Some((device, pose));
}

static HOST_VTABLE: HostVtable = HostVtable { tracked_device_added: host_tracked_device_added, pose_updated: host_pose_updated };

// ---- A fake IVRDriverLog: Log (slot 0) ----

/// Every line the driver logged.
static LOGGED: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[repr(C)]
struct LogVtable {
    log: extern "system" fn(*mut c_void, *const c_char),
}

extern "system" fn log_log(_this: *mut c_void, message: *const c_char) {
    // SAFETY: the driver passes a NUL-terminated line.
    let line = unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned();
    LOGGED.lock().unwrap().push(line);
}

static LOG_VTABLE: LogVtable = LogVtable { log: log_log };

// ---- A fake IVRDriverContext: GetGenericInterface (slot 0), GetDriverHandle (slot 1) ----

#[repr(C)]
struct ContextVtable {
    get_generic_interface: GetGenericInterfaceFn,
    get_driver_handle: extern "system" fn(*mut c_void) -> u64,
}

/// What interface version the fake context was last asked for (count, to prove the detour calls through).
static ASKED: AtomicU32 = AtomicU32::new(0);

extern "system" fn context_get_generic_interface(_this: *mut c_void, version: *const c_char, _error: *mut i32) -> *mut c_void {
    ASKED.fetch_add(1, Ordering::Relaxed);
    // SAFETY: a NUL-terminated version string.
    let version = unsafe { CStr::from_ptr(version) };
    let host = &HOST_OBJECT as *const _ as *mut c_void;
    match version.to_str() {
        Ok("IVRServerDriverHost_006") => host,
        Ok("IVRDriverLog_001") => &LOG_OBJECT as *const _ as *mut c_void,
        // Pretend _005 is absent, as a modern SteamVR would.
        _ => std::ptr::null_mut(),
    }
}

extern "system" fn context_get_driver_handle(_this: *mut c_void) -> u64 {
    1
}

static CONTEXT_VTABLE: ContextVtable = ContextVtable { get_generic_interface: context_get_generic_interface, get_driver_handle: context_get_driver_handle };

// The fake objects live for the whole test (static, as SteamVR's do).
static HOST_OBJECT: Object<HostVtable> = Object { vtable: &HOST_VTABLE };
static CONTEXT_OBJECT: Object<ContextVtable> = Object { vtable: &CONTEXT_VTABLE };
static LOG_OBJECT: Object<LogVtable> = Object { vtable: &LOG_VTABLE };

/// The provider's function table, as laid out in `openvr::ProviderVtable`.
#[repr(C)]
struct ProviderVtable {
    init: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
    cleanup: unsafe extern "system" fn(*mut c_void),
    get_interface_versions: unsafe extern "system" fn(*mut c_void) -> *const *const c_char,
    run_frame: unsafe extern "system" fn(*mut c_void),
    should_block_standby_mode: unsafe extern "system" fn(*mut c_void) -> bool,
    enter_standby: unsafe extern "system" fn(*mut c_void),
    leave_standby: unsafe extern "system" fn(*mut c_void),
}

/// The host's current `TrackedDevicePoseUpdated` entry (to tell hooked from restored).
fn host_pose_entry() -> usize {
    // SAFETY: the host object's first word is its table; slot 1 is the pose update.
    unsafe {
        let vtable = *(&HOST_OBJECT as *const _ as *const *const usize);
        *vtable.add(HOST_POSE_UPDATED_SLOT)
    }
}

#[test]
fn factory_init_hook_filter_and_cleanup() {
    // The app side: a control block of the test's own, smoothing off for now but strong on device 2.
    let name = format!(r"Local\OrbitersSmoothFfiTest-{}", std::process::id());
    driver_orbiters_smooth::use_control_name_for_tests(&name);
    let control = Control::open_or_create_named(&name).expect("open control block");
    control.set_all(&[0.0; MAX_DEVICES]);
    control.set_strength(2, 1.0);
    control.set_enabled(false);

    // SteamVR asks the factory for the provider.
    let provider = unsafe { HmdDriverFactory(c"IServerTrackedDeviceProvider_004".as_ptr(), std::ptr::null_mut()) };
    assert!(!provider.is_null(), "the factory returns the provider");
    assert!(
        unsafe { HmdDriverFactory(c"IVRWatchdogProvider_001".as_ptr(), std::ptr::null_mut()) }.is_null(),
        "and nothing for interfaces it does not implement"
    );
    // SAFETY: `provider` points at an object whose first word is its ProviderVtable.
    let vtable = unsafe { &**(provider as *const *const ProviderVtable) };

    let unhooked = host_pose_entry();

    // Init with smoothing off: nothing of SteamVR's is patched, not even the context's own table.
    let context_entry = unsafe { *(*(&CONTEXT_OBJECT as *const _ as *const *const usize)) };
    let init = unsafe { (vtable.init)(provider, &CONTEXT_OBJECT as *const _ as *mut c_void) };
    assert_eq!(init, 0, "Init returns VRInitError_None");
    assert!(LOGGED.lock().unwrap().iter().any(|l| l.contains("loaded")), "Init's line reached the log's Log: {:?}", LOGGED.lock().unwrap());
    for _ in 0..3 {
        unsafe { (vtable.run_frame)(provider) };
    }
    assert_eq!(host_pose_entry(), unhooked, "off: the host's table is untouched");
    assert_eq!(unsafe { *(*(&CONTEXT_OBJECT as *const _ as *const *const usize)) }, context_entry, "and so is the context's");
    let status = control.driver_heartbeat().expect("the driver beats while off");
    assert_eq!(status.hooks, 0);
    send_pose(2, DriverPose::at([4.0, 0.0, 0.0], Quaternion::IDENTITY));
    assert_eq!(LAST_POSE.lock().unwrap().unwrap().1.vec_position, [4.0, 0.0, 0.0], "a pose goes to SteamVR as it came");

    // The app turns smoothing on: the next frame hooks the host (asked through the context) and smooths.
    control.set_enabled(true);
    unsafe { (vtable.run_frame)(provider) };
    assert!(ASKED.load(Ordering::Relaxed) > 0, "the driver asked the context for the host");
    assert_ne!(host_pose_entry(), unhooked, "the host's pose-update slot now points at the driver");
    assert_eq!(control.driver_heartbeat().unwrap().hooks, 1, "the heartbeat says it hooked the host");
    assert!(LOGGED.lock().unwrap().iter().any(|l| l.contains("1 host hook")), "the hook is logged: {:?}", LOGGED.lock().unwrap());

    // A device the app did not mark (strength 0) passes through bit for bit.
    send_pose(0, DriverPose::at([3.0, 0.0, 0.0], Quaternion::IDENTITY));
    let (device, pose) = LAST_POSE.lock().unwrap().unwrap();
    assert_eq!(device, 0);
    assert_eq!(pose.vec_position, [3.0, 0.0, 0.0]);

    // Device 2 (strength 1): a sudden jump is eased, and the velocity SteamVR would predict with is damped to 0.
    let settle = DriverPose::at([0.0, 0.0, 0.0], Quaternion::IDENTITY);
    for _ in 0..3 {
        send_pose(2, settle);
    }
    let mut jump = DriverPose::at([1.0, 0.0, 0.0], Quaternion::IDENTITY);
    jump.vec_velocity = [5.0, 0.0, 0.0];
    send_pose(2, jump);
    let (device, pose) = LAST_POSE.lock().unwrap().unwrap();
    assert_eq!(device, 2);
    assert!(pose.vec_position[0] < 1.0, "the jump is eased, not passed as 1.0: {}", pose.vec_position[0]);
    assert_eq!(pose.vec_velocity, [0.0, 0.0, 0.0], "prediction velocity damped at full strength");

    // Turned off live: the hook stays but passes every pose on untouched; on again, it smooths again.
    control.set_enabled(false);
    unsafe { (vtable.run_frame)(provider) };
    let mut fast = DriverPose::at([2.0, 0.0, 0.0], Quaternion::IDENTITY);
    fast.vec_velocity = [5.0, 0.0, 0.0];
    send_pose(2, fast);
    let (_, pose) = LAST_POSE.lock().unwrap().unwrap();
    assert_eq!((pose.vec_position, pose.vec_velocity), ([2.0, 0.0, 0.0], [5.0, 0.0, 0.0]), "off: untouched");
    control.set_enabled(true);
    unsafe { (vtable.run_frame)(provider) };
    send_pose(2, fast);
    assert_eq!(LAST_POSE.lock().unwrap().unwrap().1.vec_velocity, [0.0, 0.0, 0.0], "on again: damped");

    // Cleanup restores the host's table.
    unsafe { (vtable.cleanup)(provider) };
    assert_eq!(host_pose_entry(), unhooked, "Cleanup put the host's original back");

    // After Cleanup the slot is the real original: a pose arrives untouched again.
    send_pose(2, DriverPose::at([9.0, 0.0, 0.0], Quaternion::IDENTITY));
    assert_eq!(LAST_POSE.lock().unwrap().unwrap().1.vec_position, [9.0, 0.0, 0.0]);

    control.set_enabled(false);
}

/// Calls the host's current `TrackedDevicePoseUpdated` through its table, as any SteamVR driver would — so while
/// hooked it goes through the driver's detour first.
fn send_pose(device: u32, pose: DriverPose) {
    // SAFETY: slot 1 of the live host table, called with the host as `this` and a real pose.
    unsafe {
        let vtable = *(&HOST_OBJECT as *const _ as *const *const usize);
        let entry = *vtable.add(HOST_POSE_UPDATED_SLOT);
        let call: PoseUpdatedFn = std::mem::transmute::<usize, PoseUpdatedFn>(entry);
        call(&HOST_OBJECT as *const _ as *mut c_void, device, &pose, std::mem::size_of::<DriverPose>() as u32);
    }
}
