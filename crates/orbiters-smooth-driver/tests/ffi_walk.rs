//! Fast walk end to end over the real FFI, as `ffi.rs` does for smoothing, in a process of its own (the driver has one state
//! per process): fake SteamVR objects, the provider `HmdDriverFactory` returns, the overlay's real `WalkLink`.
//!
//! With smoothing off and Fast walk not asking, nothing of SteamVR's is patched. When the overlay asks, the host is hooked
//! with smoothing still off; a pose reaches the fake host's original moved by exactly what the overlay wrote, with its
//! speed, on any device; with smoothing on too, each does its own part. When the writer goes (a killed overlay), nothing is
//! added any more, and `Cleanup` puts the table back.
#![cfg(target_os = "windows")]

use driver_orbiters_smooth::openvr::{DriverPose, GetGenericInterfaceFn, Object, PoseUpdatedFn, Quaternion, HOST_POSE_UPDATED_SLOT};
use driver_orbiters_smooth::HmdDriverFactory;
use orbiters_smooth::{Control, WalkLink, WalkNow, MAX_DEVICES};
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::Duration;

static LAST_POSE: Mutex<Option<(u32, DriverPose)>> = Mutex::new(None);

#[repr(C)]
struct HostVtable {
    tracked_device_added: extern "system" fn(*mut c_void, *const c_char, i32, *mut c_void) -> bool,
    pose_updated: PoseUpdatedFn,
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

#[repr(C)]
struct ContextVtable {
    get_generic_interface: GetGenericInterfaceFn,
}

extern "system" fn context_get_generic_interface(_this: *mut c_void, version: *const c_char, _error: *mut i32) -> *mut c_void {
    // SAFETY: a NUL-terminated version string.
    match unsafe { CStr::from_ptr(version) }.to_str() {
        Ok("IVRServerDriverHost_006") => &HOST_OBJECT as *const _ as *mut c_void,
        _ => std::ptr::null_mut(),
    }
}

static CONTEXT_VTABLE: ContextVtable = ContextVtable { get_generic_interface: context_get_generic_interface };
static HOST_OBJECT: Object<HostVtable> = Object { vtable: &HOST_VTABLE };
static CONTEXT_OBJECT: Object<ContextVtable> = Object { vtable: &CONTEXT_VTABLE };

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

fn host_pose_entry() -> usize {
    // SAFETY: the host object's first word is its table; slot 1 is the pose update.
    unsafe { *(*(&HOST_OBJECT as *const _ as *const *const usize)).add(HOST_POSE_UPDATED_SLOT) }
}

fn send_pose(device: u32, pose: DriverPose) -> DriverPose {
    // SAFETY: slot 1 of the live host table, called with the host as `this` and a real pose.
    unsafe {
        let call: PoseUpdatedFn = std::mem::transmute::<usize, PoseUpdatedFn>(host_pose_entry());
        call(&HOST_OBJECT as *const _ as *mut c_void, device, &pose, std::mem::size_of::<DriverPose>() as u32);
    }
    LAST_POSE.lock().unwrap().unwrap().1
}

/// The driver looks at the walk's writer every half second.
const WRITER_CHECK: Duration = Duration::from_millis(600);

#[test]
fn the_walk_reaches_every_device_through_the_hook() {
    let name = format!(r"Local\OrbitersSmoothWalkFfiTest-{}", std::process::id());
    driver_orbiters_smooth::use_control_name_for_tests(&name);
    let control = Control::open_or_create_named(&name).expect("open control block");
    control.set_all(&[0.0; MAX_DEVICES]);

    let provider = unsafe { HmdDriverFactory(c"IServerTrackedDeviceProvider_004".as_ptr(), std::ptr::null_mut()) };
    assert!(!provider.is_null());
    // SAFETY: `provider` points at an object whose first word is its ProviderVtable.
    let vtable = unsafe { &**(provider as *const *const ProviderVtable) };
    let frame = || unsafe { (vtable.run_frame)(provider) };
    let unhooked = host_pose_entry();
    assert_eq!(unsafe { (vtable.init)(provider, &CONTEXT_OBJECT as *const _ as *mut c_void) }, 0);
    frame();

    // Nobody asks: nothing patched, and the driver says it can walk.
    assert_eq!(host_pose_entry(), unhooked);
    let status = control.driver_heartbeat().expect("the driver beats");
    assert!(status.walk && status.hooks == 0, "{status:?}");

    // The overlay opens its side and asks, smoothing still off: hooked, live.
    let link = WalkLink::open_named(&name).expect("the overlay's side");
    link.set_wanted(true);
    std::thread::sleep(WRITER_CHECK);
    frame();
    assert_ne!(host_pose_entry(), unhooked, "hooked for the walk alone");
    assert_eq!(control.driver_heartbeat().unwrap().hooks, 1);

    // Nothing carried yet: a pose goes on bit for bit.
    let pose = DriverPose::at([1.0, 1.5, 0.0], Quaternion::IDENTITY);
    assert_eq!(send_pose(0, pose).vec_world_from_driver_translation, [0.0; 3]);

    // Carried: every device moves by what was written, with its speed; each device's own position stays as it came.
    link.send(&WalkNow { offset: [10.0, 0.0, -2.0], velocity: [0.0; 3] });
    for device in [0, 1, 7, 63] {
        let moved = send_pose(device, pose);
        assert_eq!(moved.vec_world_from_driver_translation, [10.0, 0.0, -2.0], "device {device}");
        assert_eq!(moved.vec_position, pose.vec_position, "device {device}");
    }
    let moving = WalkNow { offset: [10.0, 0.0, -2.0], velocity: [4.0, 0.0, 0.0] };
    link.send(&moving);
    let moved = send_pose(3, pose);
    assert_eq!(moved.vec_velocity, [4.0, 0.0, 0.0], "SteamVR predicts with the walk too");
    let on = moved.vec_world_from_driver_translation[0];
    assert!((10.0..10.0 + 4.0 * 0.051).contains(&on), "on from what was written, not past its coasting: {on}");
    // The overlay stalls: it goes on a moment, then holds, and stops predicting.
    std::thread::sleep(Duration::from_millis(80));
    let held = send_pose(3, pose);
    assert!((held.vec_world_from_driver_translation[0] - 10.2).abs() < 1e-5, "{:?}", held.vec_world_from_driver_translation);
    assert_eq!(held.vec_velocity, [0.0; 3]);
    frame();
    assert!(control.driver_heartbeat().unwrap().walked > 0, "the heartbeat counts them");

    // Smoothing on too, on one device: it eases that device's own motion, the walk comes on top, untouched.
    link.send(&WalkNow { offset: [10.0, 0.0, -2.0], velocity: [0.0; 3] });
    control.set_strength(2, 1.0);
    control.set_enabled(true);
    frame();
    for _ in 0..3 {
        send_pose(2, DriverPose::at([0.0; 3], Quaternion::IDENTITY));
    }
    let eased = send_pose(2, DriverPose::at([1.0, 0.0, 0.0], Quaternion::IDENTITY));
    assert!(eased.vec_position[0] < 1.0, "eased: {:?}", eased.vec_position);
    assert_eq!(eased.vec_world_from_driver_translation, [10.0, 0.0, -2.0], "and carried");

    // The walk set back to nothing: a pose goes on as it came.
    control.set_enabled(false);
    frame();
    link.send(&WalkNow::default());
    assert_eq!(send_pose(0, pose).vec_world_from_driver_translation, [0.0; 3]);

    // A killed overlay leaves its walk written, and is noticed: nothing is added any more.
    link.send(&WalkNow { offset: [10.0, 0.0, -2.0], velocity: [0.0; 3] });
    assert_eq!(send_pose(0, pose).vec_world_from_driver_translation, [10.0, 0.0, -2.0]);
    control.block().walk_pid.store(u32::MAX - 3, Ordering::Release);
    std::thread::sleep(WRITER_CHECK);
    frame();
    assert_eq!(send_pose(0, pose).vec_world_from_driver_translation, [0.0; 3], "its writer is gone");

    unsafe { (vtable.cleanup)(provider) };
    assert_eq!(host_pose_entry(), unhooked, "Cleanup put the host's original back");
}
