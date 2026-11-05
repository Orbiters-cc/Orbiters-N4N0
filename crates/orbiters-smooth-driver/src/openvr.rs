//! What the driver needs of OpenVR's driver API, laid out as Valve's `openvr_driver.h` (BSD-3, ValveSoftware/openvr,
//! `headers/openvr_driver.h`) declares it. C++ interfaces are objects whose first field points at a table of
//! function pointers in declaration order (none of these interfaces has a virtual destructor); on x64 Windows a
//! method is a plain function taking `this` first (the platform convention, `extern "system"`).

use std::ffi::{c_char, c_void, CStr};

/// `k_unMaxTrackedDeviceCount`.
pub const MAX_DEVICES: usize = 64;

/// `EVRInitError::VRInitError_None`.
pub const INIT_NONE: i32 = 0;
/// `EVRInitError::VRInitError_Init_InterfaceNotFound`.
pub const INIT_INTERFACE_NOT_FOUND: i32 = 105;

/// `ETrackingResult::TrackingResult_Running_OK`.
pub const TRACKING_RUNNING_OK: i32 = 200;

pub const SERVER_TRACKED_DEVICE_PROVIDER: &CStr = c"IServerTrackedDeviceProvider_004";
pub const SERVER_DRIVER_HOST_006: &CStr = c"IVRServerDriverHost_006";
/// The host older drivers (OpenVR 1.x) ask for; its `TrackedDevicePoseUpdated` is in the same place.
pub const SERVER_DRIVER_HOST_005: &CStr = c"IVRServerDriverHost_005";
pub const DRIVER_LOG: &CStr = c"IVRDriverLog_001";

/// `IVRServerDriverHost::TrackedDevicePoseUpdated`'s place in its table (after `TrackedDeviceAdded`).
pub const HOST_POSE_UPDATED_SLOT: usize = 1;

/// `vr::HmdQuaternion_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quaternion {
    pub w: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Quaternion {
    pub const IDENTITY: Self = Self { w: 1.0, x: 0.0, y: 0.0, z: 0.0 };
}

/// `vr::DriverPose_t` (280 bytes). The flags are C++ `bool`s, read as bytes so a driver that leaves garbage in them
/// cannot make this crate misbehave (anything but 0 is true, as in C++).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriverPose {
    pub pose_time_offset: f64,
    pub q_world_from_driver_rotation: Quaternion,
    pub vec_world_from_driver_translation: [f64; 3],
    pub q_driver_from_head_rotation: Quaternion,
    pub vec_driver_from_head_translation: [f64; 3],
    pub vec_position: [f64; 3],
    pub vec_velocity: [f64; 3],
    pub vec_acceleration: [f64; 3],
    pub q_rotation: Quaternion,
    pub vec_angular_velocity: [f64; 3],
    pub vec_angular_acceleration: [f64; 3],
    pub result: i32,
    pub pose_is_valid: u8,
    pub will_drift_in_yaw: u8,
    pub should_apply_head_model: u8,
    pub device_is_connected: u8,
}

const _: () = assert!(std::mem::size_of::<DriverPose>() == 280);

impl DriverPose {
    /// A valid, connected pose at rest (tests, and the shape every driver fills in).
    pub fn at(position: [f64; 3], rotation: Quaternion) -> Self {
        Self {
            pose_time_offset: 0.0,
            q_world_from_driver_rotation: Quaternion::IDENTITY,
            vec_world_from_driver_translation: [0.0; 3],
            q_driver_from_head_rotation: Quaternion::IDENTITY,
            vec_driver_from_head_translation: [0.0; 3],
            vec_position: position,
            vec_velocity: [0.0; 3],
            vec_acceleration: [0.0; 3],
            q_rotation: rotation,
            vec_angular_velocity: [0.0; 3],
            vec_angular_acceleration: [0.0; 3],
            result: TRACKING_RUNNING_OK,
            pose_is_valid: 1,
            will_drift_in_yaw: 0,
            should_apply_head_model: 0,
            device_is_connected: 1,
        }
    }
}

/// `IVRDriverContext::GetGenericInterface` (slot 0).
pub type GetGenericInterfaceFn = unsafe extern "system" fn(this: *mut c_void, version: *const c_char, error: *mut i32) -> *mut c_void;
/// `IVRServerDriverHost::TrackedDevicePoseUpdated` (slot 1); the pose is a C++ reference, a pointer here.
pub type PoseUpdatedFn = unsafe extern "system" fn(this: *mut c_void, device: u32, pose: *const DriverPose, pose_size: u32);
/// `IVRDriverLog::Log` (slot 0).
pub type LogFn = unsafe extern "system" fn(this: *mut c_void, message: *const c_char);

/// `IServerTrackedDeviceProvider`'s table, in declaration order.
#[repr(C)]
pub struct ProviderVtable {
    pub init: unsafe extern "system" fn(this: *mut c_void, context: *mut c_void) -> i32,
    pub cleanup: unsafe extern "system" fn(this: *mut c_void),
    pub get_interface_versions: unsafe extern "system" fn(this: *mut c_void) -> *const *const c_char,
    pub run_frame: unsafe extern "system" fn(this: *mut c_void),
    pub should_block_standby_mode: unsafe extern "system" fn(this: *mut c_void) -> bool,
    pub enter_standby: unsafe extern "system" fn(this: *mut c_void),
    pub leave_standby: unsafe extern "system" fn(this: *mut c_void),
}

/// A C++ object: its table first.
#[repr(C)]
pub struct Object<V: 'static> {
    pub vtable: &'static V,
}

/// `vr::k_InterfaceVersions` of the header this follows, NUL-terminated: the versions a driver built with it
/// implements, which `GetInterfaceVersions` returns.
pub struct InterfaceVersions(pub [*const c_char; 13]);

// SAFETY: pointers to static C strings, never written.
unsafe impl Sync for InterfaceVersions {}

pub static INTERFACE_VERSIONS: InterfaceVersions = InterfaceVersions([
    c"IVRSettings_003".as_ptr(),
    c"ITrackedDeviceServerDriver_005".as_ptr(),
    c"IVRDisplayComponent_003".as_ptr(),
    c"IVRDriverDirectModeComponent_009".as_ptr(),
    c"IVRCameraComponent_003".as_ptr(),
    SERVER_TRACKED_DEVICE_PROVIDER.as_ptr(),
    c"IVRWatchdogProvider_001".as_ptr(),
    c"IVRVirtualDisplay_002".as_ptr(),
    c"IVRDriverManager_001".as_ptr(),
    c"IVRResources_001".as_ptr(),
    c"IVRCompositorPluginProvider_001".as_ptr(),
    c"IVRIPCResourceManagerClient_003".as_ptr(),
    std::ptr::null(),
]);

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    #[test]
    fn driver_pose_matches_the_header() {
        assert_eq!(std::mem::align_of::<DriverPose>(), 8);
        assert_eq!(offset_of!(DriverPose, q_world_from_driver_rotation), 8);
        assert_eq!(offset_of!(DriverPose, vec_world_from_driver_translation), 40);
        assert_eq!(offset_of!(DriverPose, q_driver_from_head_rotation), 64);
        assert_eq!(offset_of!(DriverPose, vec_driver_from_head_translation), 96);
        assert_eq!(offset_of!(DriverPose, vec_position), 120);
        assert_eq!(offset_of!(DriverPose, vec_velocity), 144);
        assert_eq!(offset_of!(DriverPose, vec_acceleration), 168);
        assert_eq!(offset_of!(DriverPose, q_rotation), 192);
        assert_eq!(offset_of!(DriverPose, vec_angular_velocity), 224);
        assert_eq!(offset_of!(DriverPose, vec_angular_acceleration), 248);
        assert_eq!(offset_of!(DriverPose, result), 272);
        assert_eq!(offset_of!(DriverPose, pose_is_valid), 276);
        assert_eq!(offset_of!(DriverPose, device_is_connected), 279);
    }

    #[test]
    fn interface_versions_end_with_null() {
        let list = &INTERFACE_VERSIONS.0;
        assert!(list[list.len() - 1].is_null());
        assert!(list[..list.len() - 1].iter().all(|p| !p.is_null()));
    }
}
