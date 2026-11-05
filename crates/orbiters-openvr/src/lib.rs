//! Minimal SteamVR client. Like Valve's `openvr_api` loader (BSD, `src/openvr_api_public.cpp` in the OpenVR repo),
//! it reads the runtime folder from `openvrpaths.vrpath`, loads that runtime's `vrclient_x64.dll`, asks its
//! `VRClientCoreFactory` for `IVRClientCore_003` and gets the C function tables of the interfaces it uses
//! (`FnTable:IVRSystem_026`, `FnTable:IVROverlay_028`; layouts from Valve's public `openvr_capi.h`). Nothing to
//! ship, and the runtime's own client is always the matching one.
//!
//! A [`Client`] of type [`AppType::Background`] never starts SteamVR: it fails when SteamVR is not running.

// The runtime loaded is `vrclient_x64.dll`, and `IVRClientCore`'s methods are called as `extern "system"`: right on
// x64, where C++ methods use the platform convention, wrong on 32-bit Windows (thiscall).
#[cfg(all(windows, not(target_arch = "x86_64")))]
compile_error!("orbiters-openvr supports 64-bit Windows (x86_64) only: it loads vrclient_x64.dll and calls IVRClientCore with the x64 convention");

use orbiters_contracts::performance::tracking::{self, Device, Sample};
pub mod applications;
pub mod chaperone;
pub mod compositor;
#[cfg(windows)]
pub mod gpu;
pub mod input;
#[cfg(windows)]
pub mod mirror;
use std::cell::OnceCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};

pub const MAX_DEVICES: usize = 64;

/// `EVRApplicationType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum AppType {
    /// Draws overlays; starts SteamVR when it is not running.
    Overlay = 2,
    /// Reads poses only; fails when SteamVR is not running.
    Background = 3,
}

/// `EDeviceActivityLevel`: whether a device is being used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    Unknown,
    /// No activity for the last 10 seconds.
    Idle,
    UserInteraction,
    /// No activity for the last half second.
    UserInteractionTimeout,
    /// Idle long enough for SteamVR's standby (its power management setting).
    Standby,
    IdleTimeout,
}

impl Activity {
    fn from_raw(v: i32) -> Self {
        match v {
            0 => Self::Idle,
            1 => Self::UserInteraction,
            2 => Self::UserInteractionTimeout,
            3 => Self::Standby,
            4 => Self::IdleTimeout,
            _ => Self::Unknown,
        }
    }

    /// Someone wears or moves it (or did half a second ago).
    pub fn in_use(self) -> bool {
        matches!(self, Self::UserInteraction | Self::UserInteractionTimeout)
    }
}

/// `ETrackingUniverseOrigin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Universe {
    Seated = 0,
    Standing = 1,
    Raw = 2,
}

/// `ETrackedDeviceClass`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceClass {
    Invalid,
    Hmd,
    Controller,
    GenericTracker,
    TrackingReference,
    DisplayRedirect,
    Other(i32),
}

impl DeviceClass {
    fn from_raw(v: i32) -> Self {
        match v {
            0 => Self::Invalid,
            1 => Self::Hmd,
            2 => Self::Controller,
            3 => Self::GenericTracker,
            4 => Self::TrackingReference,
            5 => Self::DisplayRedirect,
            other => Self::Other(other),
        }
    }
}

/// `ETrackedControllerRole`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControllerRole {
    Invalid,
    LeftHand,
    RightHand,
    OptOut,
    Treadmill,
    Stylus,
    Other(i32),
}

impl ControllerRole {
    fn from_raw(v: i32) -> Self {
        match v {
            0 => Self::Invalid,
            1 => Self::LeftHand,
            2 => Self::RightHand,
            3 => Self::OptOut,
            4 => Self::Treadmill,
            5 => Self::Stylus,
            other => Self::Other(other),
        }
    }

    fn raw(self) -> i32 {
        match self {
            Self::Invalid => 0,
            Self::LeftHand => 1,
            Self::RightHand => 2,
            Self::OptOut => 3,
            Self::Treadmill => 4,
            Self::Stylus => 5,
            Self::Other(v) => v,
        }
    }
}

/// `ETrackedDeviceProperty` values used here.
pub mod prop {
    pub const TRACKING_SYSTEM_NAME: i32 = 1000;
    pub const MODEL_NUMBER: i32 = 1001;
    /// The headset's: from a vsync to its picture lighting the displays.
    pub const SECONDS_FROM_VSYNC_TO_PHOTONS: i32 = 2001;
    pub const DISPLAY_FREQUENCY: i32 = 2002;
    pub const SERIAL_NUMBER: i32 = 1002;
    pub const MANUFACTURER_NAME: i32 = 1005;
    pub const CONTROLLER_ROLE_HINT: i32 = 3007;
    pub const CONTROLLER_TYPE: i32 = 7000;
}

/// `EVREventType` values used here.
pub mod event {
    pub const TRACKED_DEVICE_ACTIVATED: u32 = 100;
    pub const TRACKED_DEVICE_DEACTIVATED: u32 = 101;
    pub const TRACKED_DEVICE_ROLE_CHANGED: u32 = 108;
    pub const PROPERTY_CHANGED: u32 = 111;
    pub const MOUSE_MOVE: u32 = 300;
    pub const MOUSE_BUTTON_DOWN: u32 = 301;
    pub const MOUSE_BUTTON_UP: u32 = 302;
    pub const FOCUS_ENTER: u32 = 303;
    pub const FOCUS_LEAVE: u32 = 304;
    pub const SCROLL_DISCRETE: u32 = 305;
    pub const SCROLL_SMOOTH: u32 = 309;
    pub const OVERLAY_SHOWN: u32 = 500;
    pub const OVERLAY_HIDDEN: u32 = 501;
    pub const DASHBOARD_ACTIVATED: u32 = 502;
    pub const DASHBOARD_DEACTIVATED: u32 = 503;
    pub const QUIT: u32 = 700;
    pub const PROCESS_QUIT: u32 = 701;
    /// The play space changed under the applications: another chaperone universe, a seated or standing zero pose
    /// reset (a recenter), a cache to read again, room setup starting or saved (`CommitWorkingCopy`, by any
    /// application). 800 and 802 no longer come with SteamVR's current chaperone.
    pub const CHAPERONE_UNIVERSE_CHANGED: u32 = 801;
    pub const SEATED_ZERO_POSE_RESET: u32 = 804;
    pub const CHAPERONE_FLUSH_CACHE: u32 = 805;
    pub const ROOM_SETUP_STARTING: u32 = 806;
    pub const ROOM_SETUP_COMMITTED: u32 = 807;
    pub const STANDING_ZERO_POSE_RESET: u32 = 808;
}

/// `HmdMatrix34_t`: row-major 3×4, right-handed, metres.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C)]
pub struct Matrix34 {
    pub m: [[f32; 4]; 3],
}

impl Matrix34 {
    pub const IDENTITY: Self = Self { m: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]] };
}

/// `TrackedDevicePose_t` (80 bytes).
#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct Pose {
    pub device_to_absolute: Matrix34,
    pub velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    /// `ETrackingResult`: 200 running, 101 calibrating out of range, 201 out of range, 300 rotation only, ...
    pub tracking_result: i32,
    pub pose_is_valid: bool,
    pub device_is_connected: bool,
}

/// `VREvent_t` (64 bytes on Windows: the data union is 8-aligned).
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Event {
    pub event_type: u32,
    pub device: u32,
    pub age_seconds: f32,
    pub data: [u64; 6],
}

impl Default for Event {
    fn default() -> Self {
        Self { event_type: 0, device: 0, age_seconds: 0.0, data: [0; 6] }
    }
}

impl Event {
    /// A mouse event as SteamVR's laser sends one (`MOUSE_MOVE`, `MOUSE_BUTTON_DOWN`, `MOUSE_BUTTON_UP`,
    /// `FOCUS_LEAVE`): `VREvent_Mouse_t` x, y (mouse scale units, origin bottom left) and button (1: left), for an
    /// overlay this process points at with lasers of its own ([`Overlay::inject_event`]).
    pub fn mouse_event(event_type: u32, x: f32, y: f32, button: u32) -> Self {
        let data = [x.to_bits() as u64 | (y.to_bits() as u64) << 32, button as u64, 0, 0, 0, 0];
        Self { event_type, device: 0, age_seconds: 0.0, data }
    }

    /// `VREvent_Mouse_t`: x, y (overlay mouse scale units, origin bottom left), button (1 left, 2 right, 4 middle).
    pub fn mouse(&self) -> (f32, f32, u32) {
        let w = self.words();
        (f32::from_bits(w[0]), f32::from_bits(w[1]), w[2])
    }

    /// `VREvent_Scroll_t`: x and y deltas.
    pub fn scroll(&self) -> (f32, f32) {
        let w = self.words();
        (f32::from_bits(w[0]), f32::from_bits(w[1]))
    }

    /// `VREvent_Chaperone_t`: the tracking universe before and after (`ChaperoneUniverseHasChanged`).
    pub fn chaperone_universes(&self) -> (u64, u64) {
        (self.data[0], self.data[1])
    }

    fn words(&self) -> [u32; 12] {
        let mut out = [0u32; 12];
        for (i, d) in self.data.iter().enumerate() {
            out[i * 2] = *d as u32;
            out[i * 2 + 1] = (*d >> 32) as u32;
        }
        out
    }
}

/// Why SteamVR could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    NotInstalled,
    /// `EVRInitError` from the runtime (121: SteamVR is not running, for background apps; 119: it is shutting down).
    Init(i32),
    /// The runtime lacks an interface version.
    Interface(&'static str, i32),
    Load(String),
}

impl Error {
    /// SteamVR is not running, or is shutting down (what a background application is told instead of starting it).
    pub fn not_running(&self) -> bool {
        matches!(self, Error::Init(121) | Error::Init(119))
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotInstalled => write!(f, "SteamVR is not installed"),
            Error::Init(121) => write!(f, "SteamVR is not running"),
            Error::Init(119) => write!(f, "SteamVR is shutting down"),
            Error::Init(code) => write!(f, "SteamVR refused the connection (error {code})"),
            Error::Interface(name, code) => write!(f, "SteamVR has no {name} (error {code})"),
            Error::Load(e) => write!(f, "could not load SteamVR's client: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// The SteamVR runtime folder registered in `openvrpaths.vrpath` (the first `runtime` entry that exists).
pub fn runtime_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    runtime_from_vrpath(&std::fs::read_to_string(base.join("openvr").join("openvrpaths.vrpath")).ok()?)
}

fn runtime_from_vrpath(text: &str) -> Option<PathBuf> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    json["runtime"].as_array()?.iter().filter_map(|v| v.as_str()).map(PathBuf::from).find(|p| p.is_dir())
}

/// Whether a SteamVR runtime is registered.
pub fn installed() -> bool {
    runtime_dir().is_some()
}

#[cfg(windows)]
mod platform {
    use std::path::Path;
    use windows::core::{HSTRING, PCSTR};
    use windows::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_WITH_ALTERED_SEARCH_PATH};

    pub struct Library(HMODULE);

    // The module handle is only used to look up symbols and to free it once.
    unsafe impl Send for Library {}
    unsafe impl Sync for Library {}

    impl Library {
        pub fn open(runtime: &Path) -> Result<Self, String> {
            let path = runtime.join("bin").join("vrclient_x64.dll");
            // SAFETY: loading SteamVR's client; its dependencies sit next to it (altered search path).
            unsafe { LoadLibraryExW(&HSTRING::from(path.as_os_str()), None, LOAD_WITH_ALTERED_SEARCH_PATH) }.map(Library).map_err(|e| e.to_string())
        }

        pub fn symbol(&self, name: &std::ffi::CStr) -> Option<*const std::ffi::c_void> {
            // SAFETY: valid module handle and NUL-terminated name.
            unsafe { GetProcAddress(self.0, PCSTR(name.as_ptr().cast())) }.map(|f| f as *const std::ffi::c_void)
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            // SAFETY: loaded by `open`, freed once.
            unsafe {
                let _ = FreeLibrary(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    pub struct Library;

    impl Library {
        pub fn open(_runtime: &Path) -> Result<Self, String> {
            Err("only Windows is supported for now".into())
        }

        pub fn symbol(&self, _name: &std::ffi::CStr) -> Option<*const std::ffi::c_void> {
            None
        }
    }
}

type FactoryFn = unsafe extern "C" fn(name: *const c_char, code: *mut i32) -> *mut c_void;

/// `IVRClientCore_003` virtual methods, in vtable order.
#[repr(C)]
struct ClientCoreVtbl {
    init: unsafe extern "system" fn(this: *mut c_void, app: i32, startup_info: *const c_char) -> i32,
    cleanup: unsafe extern "system" fn(this: *mut c_void),
    is_interface_version_valid: unsafe extern "system" fn(this: *mut c_void, version: *const c_char) -> i32,
    get_generic_interface: unsafe extern "system" fn(this: *mut c_void, name: *const c_char, error: *mut i32) -> *mut c_void,
}

/// Reads entry `index` of a C function table as a function pointer of type `F`.
///
/// # Safety
/// `table` must point to a live function table with at least `index + 1` entries of type `F` at `index`.
unsafe fn entry<F: Copy>(table: *const usize, index: usize) -> F {
    debug_assert_eq!(std::mem::size_of::<F>(), std::mem::size_of::<usize>());
    // SAFETY: per the caller; function pointers are pointer-sized.
    unsafe { std::mem::transmute_copy(&*table.add(index)) }
}

/// A function table asked for on first use, then kept (or why the runtime has none).
type Table = OnceCell<Result<*const usize, Error>>;

/// A connection to SteamVR. Dropping it disconnects (`IVRClientCore::Cleanup`) and unloads the client.
pub struct Client {
    core: *mut c_void,
    system: *const usize,
    overlay: Option<*const usize>,
    /// The other interfaces, looked up once each: a frame of the overlay reads several of them, and each lookup
    /// builds a name and asks the runtime.
    input_table: Table,
    compositor_table: Table,
    chaperone_setup_table: Table,
    applications_table: Table,
    _library: platform::Library,
}

// SteamVR's client interfaces may be called from any thread; the pointers stay valid until `Cleanup`. A client is
// used by one thread at a time (not `Sync`: its tables fill on first use).
unsafe impl Send for Client {}

pub const SYSTEM_VERSION: &str = "IVRSystem_026";
pub const OVERLAY_VERSION: &str = "IVROverlay_028";

impl Client {
    pub fn connect(app: AppType) -> Result<Self, Error> {
        let runtime = runtime_dir().ok_or(Error::NotInstalled)?;
        Self::connect_to(&runtime, app)
    }

    pub fn connect_to(runtime: &Path, app: AppType) -> Result<Self, Error> {
        let library = platform::Library::open(runtime).map_err(Error::Load)?;
        let factory = library.symbol(c"VRClientCoreFactory").ok_or_else(|| Error::Load("no VRClientCoreFactory".into()))?;
        // SAFETY: the documented factory signature of vrclient.
        let factory: FactoryFn = unsafe { std::mem::transmute(factory) };
        let mut code = 0;
        // SAFETY: NUL-terminated interface name and a valid out-parameter.
        let core = unsafe { factory(c"IVRClientCore_003".as_ptr(), &mut code) };
        if core.is_null() {
            return Err(Error::Interface("IVRClientCore_003", code));
        }
        let vtbl = Self::vtbl(core);
        // SAFETY: `core` is a live IVRClientCore; an empty startup info is allowed.
        let err = unsafe { (vtbl.init)(core, app as i32, c"".as_ptr()) };
        if err != 0 {
            return Err(Error::Init(err));
        }
        let mut client = Client {
            core,
            system: std::ptr::null(),
            overlay: None,
            input_table: OnceCell::new(),
            compositor_table: OnceCell::new(),
            chaperone_setup_table: OnceCell::new(),
            applications_table: OnceCell::new(),
            _library: library,
        };
        client.system = client.interface(SYSTEM_VERSION)?;
        if app == AppType::Overlay {
            client.overlay = Some(client.interface(OVERLAY_VERSION)?);
        }
        Ok(client)
    }

    fn vtbl(core: *mut c_void) -> &'static ClientCoreVtbl {
        // SAFETY: a C++ object starts with its vtable pointer; IVRClientCore's first four methods are declared above.
        unsafe { &**(core as *const *const ClientCoreVtbl) }
    }

    fn interface(&self, version: &'static str) -> Result<*const usize, Error> {
        let name = CString::new(format!("FnTable:{version}")).expect("no NUL");
        let mut code = 0;
        // SAFETY: live core, NUL-terminated name.
        let table = unsafe { (Self::vtbl(self.core).get_generic_interface)(self.core, name.as_ptr(), &mut code) };
        if table.is_null() || code != 0 {
            return Err(Error::Interface(version, code));
        }
        Ok(table as *const usize)
    }

    /// `version`'s function table from `cell`, looked up the first time only.
    fn table(&self, cell: &Table, version: &'static str) -> Result<*const usize, Error> {
        cell.get_or_init(|| self.interface(version)).clone()
    }

    // ------------------------------------------------------------------------------------------------ IVRSystem

    /// Poses of all devices now (no prediction), in `universe`.
    pub fn poses(&self, universe: Universe, out: &mut [Pose; MAX_DEVICES]) {
        type F = unsafe extern "system" fn(i32, f32, *mut Pose, u32);
        // SAFETY: IVRSystem_026 entry 12 is GetDeviceToAbsoluteTrackingPose.
        unsafe { entry::<F>(self.system, 12)(universe as i32, 0.0, out.as_mut_ptr(), MAX_DEVICES as u32) }
    }

    /// Poses of all devices as SteamVR predicts them `seconds` from now (when a picture made now lights the displays).
    pub fn predicted_poses(&self, universe: Universe, seconds: f32, out: &mut [Pose; MAX_DEVICES]) {
        type F = unsafe extern "system" fn(i32, f32, *mut Pose, u32);
        // SAFETY: IVRSystem_026 entry 12 is GetDeviceToAbsoluteTrackingPose.
        unsafe { entry::<F>(self.system, 12)(universe as i32, seconds, out.as_mut_ptr(), MAX_DEVICES as u32) }
    }

    /// Where `eye` sits in the headset's space (half the lens distance to its side, any canting).
    pub fn eye_to_head(&self, eye: compositor::Eye) -> Matrix34 {
        type F = unsafe extern "system" fn(i32) -> Matrix34;
        // SAFETY: entry 5 is GetEyeToHeadTransform (HmdMatrix34_t returned by value).
        unsafe { entry::<F>(self.system, 5)(eye as i32) }
    }

    /// `eye`'s field of view as the tangents of its sides (left, right, top, bottom; top is negative, y pointing down).
    pub fn projection_raw(&self, eye: compositor::Eye) -> [f32; 4] {
        type F = unsafe extern "system" fn(i32, *mut f32, *mut f32, *mut f32, *mut f32);
        let mut t = [0f32; 4];
        let [l, r, top, b] = &mut t;
        // SAFETY: entry 2 is GetProjectionRaw; the four out parameters are valid.
        unsafe { entry::<F>(self.system, 2)(eye as i32, l, r, top, b) };
        t
    }

    /// Seconds since the headset's last vsync, when SteamVR knows it.
    pub fn time_since_vsync(&self) -> Option<f32> {
        type F = unsafe extern "system" fn(*mut f32, *mut u64) -> bool;
        let (mut seconds, mut frame) = (0f32, 0u64);
        // SAFETY: entry 6 is GetTimeSinceLastVsync; both out parameters are valid.
        unsafe { entry::<F>(self.system, 6)(&mut seconds, &mut frame) }.then_some(seconds)
    }

    pub fn device_class(&self, device: u32) -> DeviceClass {
        type F = unsafe extern "system" fn(u32) -> i32;
        // SAFETY: entry 20 is GetTrackedDeviceClass.
        DeviceClass::from_raw(unsafe { entry::<F>(self.system, 20)(device) })
    }

    pub fn controller_role(&self, device: u32) -> ControllerRole {
        type F = unsafe extern "system" fn(u32) -> i32;
        // SAFETY: entry 19 is GetControllerRoleForTrackedDeviceIndex.
        ControllerRole::from_raw(unsafe { entry::<F>(self.system, 19)(device) })
    }

    /// The device that has `role` now (the left or the right hand), if one has it: one question instead of one per
    /// device.
    pub fn controller_index(&self, role: ControllerRole) -> Option<u32> {
        type F = unsafe extern "system" fn(i32) -> u32;
        // SAFETY: entry 18 is GetTrackedDeviceIndexForControllerRole (k_unTrackedDeviceIndexInvalid when none).
        let index = unsafe { entry::<F>(self.system, 18)(role.raw()) };
        (index < MAX_DEVICES as u32).then_some(index)
    }

    pub fn is_connected(&self, device: u32) -> bool {
        type F = unsafe extern "system" fn(u32) -> bool;
        // SAFETY: entry 21 is IsTrackedDeviceConnected.
        unsafe { entry::<F>(self.system, 21)(device) }
    }

    /// Whether someone is using `device` (`EDeviceActivityLevel`): the headset's proximity sensor and motion.
    pub fn activity(&self, device: u32) -> Activity {
        type F = unsafe extern "system" fn(u32) -> i32;
        // SAFETY: entry 16 is GetTrackedDeviceActivityLevel.
        Activity::from_raw(unsafe { entry::<F>(self.system, 16)(device) })
    }

    pub fn int_property(&self, device: u32, property: i32) -> Option<i32> {
        type F = unsafe extern "system" fn(u32, i32, *mut i32) -> i32;
        let mut err = 0;
        // SAFETY: entry 24 is GetInt32TrackedDeviceProperty.
        let v = unsafe { entry::<F>(self.system, 24)(device, property, &mut err) };
        (err == 0).then_some(v)
    }

    pub fn float_property(&self, device: u32, property: i32) -> Option<f32> {
        type F = unsafe extern "system" fn(u32, i32, *mut i32) -> f32;
        let mut err = 0;
        // SAFETY: entry 23 is GetFloatTrackedDeviceProperty.
        let v = unsafe { entry::<F>(self.system, 23)(device, property, &mut err) };
        (err == 0).then_some(v)
    }

    pub fn string_property(&self, device: u32, property: i32) -> Option<String> {
        type F = unsafe extern "system" fn(u32, i32, *mut c_char, u32, *mut i32) -> u32;
        let mut buf = vec![0u8; 256];
        let mut err = 0;
        // SAFETY: entry 28 is GetStringTrackedDeviceProperty; the buffer size is passed along.
        let mut n = unsafe { entry::<F>(self.system, 28)(device, property, buf.as_mut_ptr().cast(), buf.len() as u32, &mut err) };
        if err == 3 && n as usize > buf.len() {
            buf.resize(n as usize, 0);
            // SAFETY: as above, with the size the runtime asked for.
            n = unsafe { entry::<F>(self.system, 28)(device, property, buf.as_mut_ptr().cast(), buf.len() as u32, &mut err) };
        }
        if err != 0 || n == 0 {
            return None;
        }
        CStr::from_bytes_until_nul(&buf).ok().map(|s| s.to_string_lossy().into_owned())
    }

    /// Next event for this application, if any.
    pub fn poll_event(&self) -> Option<Event> {
        type F = unsafe extern "system" fn(*mut Event, u32) -> bool;
        let mut e = Event::default();
        // SAFETY: entry 30 is PollNextEvent; the struct size is passed along.
        unsafe { entry::<F>(self.system, 30)(&mut e, std::mem::size_of::<Event>() as u32) }.then_some(e)
    }

    /// Tells SteamVR this application exits after a quit event.
    pub fn acknowledge_quit(&self) {
        type F = unsafe extern "system" fn();
        // SAFETY: entry 47 is AcknowledgeQuit_Exiting.
        unsafe { entry::<F>(self.system, 47)() }
    }

    /// What a device is (class, role, serial, ...), for `tracking.json`; `None` for an empty slot.
    pub fn describe(&self, index: u32) -> Option<Device> {
        let raw_class = self.device_class(index);
        let class = match raw_class {
            DeviceClass::Hmd => tracking::DeviceClass::Hmd,
            DeviceClass::Controller => tracking::DeviceClass::Controller,
            DeviceClass::GenericTracker => tracking::DeviceClass::Tracker,
            DeviceClass::TrackingReference => tracking::DeviceClass::TrackingReference,
            DeviceClass::Invalid => return None,
            _ => tracking::DeviceClass::Other,
        };
        let controller_type = self.string_property(index, prop::CONTROLLER_TYPE);
        let s = |p| self.string_property(index, p).unwrap_or_default();
        Some(Device {
            index: index as u8,
            class,
            role: role_name(raw_class, self.controller_role(index), controller_type.as_deref()),
            serial: s(prop::SERIAL_NUMBER),
            model: s(prop::MODEL_NUMBER),
            manufacturer: s(prop::MANUFACTURER_NAME),
            controller_type: controller_type.unwrap_or_default(),
            tracking_system: s(prop::TRACKING_SYSTEM_NAME),
            first_seen_ns: 0,
        })
    }

    /// The DXGI adapter index of the GPU driving the headset (`GetDXGIOutputInfo`), when SteamVR knows it.
    pub fn dxgi_adapter_index(&self) -> Option<u32> {
        type F = unsafe extern "system" fn(*mut i32);
        let mut index = -1;
        // SAFETY: entry 8 is GetDXGIOutputInfo.
        unsafe { entry::<F>(self.system, 8)(&mut index) };
        u32::try_from(index).ok()
    }

    // ------------------------------------------------------------------------------------------------ IVROverlay

    pub fn overlay(&self) -> Option<Overlay<'_>> {
        self.overlay.map(|table| Overlay { table, _client: self, injected: Default::default() })
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: live core; nothing uses the interfaces afterwards.
        unsafe { (Self::vtbl(self.core).cleanup)(self.core) }
    }
}

/// `VROverlayHandle_t`.
pub type OverlayHandle = u64;

/// `IVROverlay_028`, for clients connected as [`AppType::Overlay`]. Methods return the `EVROverlayError` (0 = none).
pub struct Overlay<'a> {
    table: *const usize,
    _client: &'a Client,
    /// Events this process made for its own overlays (its own lasers' mouse), handed out by [`Overlay::poll_event`]
    /// before SteamVR's.
    injected: std::cell::RefCell<std::collections::VecDeque<(OverlayHandle, Event)>>,
}

/// `VROverlayFlags` used here (bit masks).
pub mod overlay_flag {
    pub const NO_DASHBOARD_TAB: i32 = 1 << 3;
    pub const SEND_VR_DISCRETE_SCROLL_EVENTS: i32 = 1 << 6;
    /// The texture's left half goes to the left eye, its right half to the right one.
    pub const SIDE_BY_SIDE_PARALLEL: i32 = 1 << 10;
    /// SteamVR's laser works on the overlay whenever it is shown, dashboard open or not (a menu in the play space).
    pub const MAKE_OVERLAYS_INTERACTIVE_IF_VISIBLE: i32 = 1 << 16;
    pub const SEND_VR_SMOOTH_SCROLL_EVENTS: i32 = 1 << 17;
    /// The pixels are premultiplied by their alpha (as the CPU painter makes them).
    pub const IS_PREMULTIPLIED: i32 = 1 << 21;
    pub const ENABLE_CLICK_STABILIZATION: i32 = 1 << 27;
}

fn overlay_result(code: i32) -> Result<(), i32> {
    if code == 0 {
        Ok(())
    } else {
        Err(code)
    }
}

impl Overlay<'_> {
    /// A dashboard overlay (a tab in SteamVR's dashboard) and its thumbnail overlay.
    pub fn create_dashboard(&self, key: &str, name: &str) -> Result<(OverlayHandle, OverlayHandle), i32> {
        type F = unsafe extern "system" fn(*const c_char, *const c_char, *mut u64, *mut u64) -> i32;
        let (key, name) = (CString::new(key).expect("no NUL"), CString::new(name).expect("no NUL"));
        let (mut main, mut thumb) = (0, 0);
        // SAFETY: entry 67 is CreateDashboardOverlay.
        overlay_result(unsafe { entry::<F>(self.table, 67)(key.as_ptr(), name.as_ptr(), &mut main, &mut thumb) })?;
        Ok((main, thumb))
    }

    pub fn create(&self, key: &str, name: &str) -> Result<OverlayHandle, i32> {
        type F = unsafe extern "system" fn(*const c_char, *const c_char, *mut u64) -> i32;
        let (key, name) = (CString::new(key).expect("no NUL"), CString::new(name).expect("no NUL"));
        let mut handle = 0;
        // SAFETY: entry 1 is CreateOverlay.
        overlay_result(unsafe { entry::<F>(self.table, 1)(key.as_ptr(), name.as_ptr(), &mut handle) })?;
        Ok(handle)
    }

    pub fn destroy(&self, handle: OverlayHandle) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64) -> i32;
        // SAFETY: entry 3 is DestroyOverlay.
        overlay_result(unsafe { entry::<F>(self.table, 3)(handle) })
    }

    pub fn set_flag(&self, handle: OverlayHandle, flag: i32, on: bool) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64, i32, bool) -> i32;
        // SAFETY: entry 11 is SetOverlayFlag.
        overlay_result(unsafe { entry::<F>(self.table, 11)(handle, flag, on) })
    }

    pub fn set_width_in_meters(&self, handle: OverlayHandle, width: f32) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64, f32) -> i32;
        // SAFETY: entry 22 is SetOverlayWidthInMeters.
        overlay_result(unsafe { entry::<F>(self.table, 22)(handle, width) })
    }

    pub fn set_sort_order(&self, handle: OverlayHandle, order: u32) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64, u32) -> i32;
        // SAFETY: entry 20 is SetOverlaySortOrder.
        overlay_result(unsafe { entry::<F>(self.table, 20)(handle, order) })
    }

    /// Places the overlay relative to a device (`0` is the headset).
    pub fn set_transform_device_relative(&self, handle: OverlayHandle, device: u32, transform: &Matrix34) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64, u32, *const Matrix34) -> i32;
        // SAFETY: entry 35 is SetOverlayTransformTrackedDeviceRelative.
        overlay_result(unsafe { entry::<F>(self.table, 35)(handle, device, transform) })
    }

    pub fn show(&self, handle: OverlayHandle) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64) -> i32;
        // SAFETY: entry 43 is ShowOverlay.
        overlay_result(unsafe { entry::<F>(self.table, 43)(handle) })
    }

    pub fn hide(&self, handle: OverlayHandle) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64) -> i32;
        // SAFETY: entry 44 is HideOverlay.
        overlay_result(unsafe { entry::<F>(self.table, 44)(handle) })
    }

    pub fn is_visible(&self, handle: OverlayHandle) -> bool {
        type F = unsafe extern "system" fn(u64) -> bool;
        // SAFETY: entry 45 is IsOverlayVisible.
        unsafe { entry::<F>(self.table, 45)(handle) }
    }

    /// Waits until the compositor starts its next frame, `timeout_ms` at most (a frame's length is the advice: a
    /// signal can be missed while the scene application is slow). A loop that waits here runs once per frame shown.
    pub fn wait_frame_sync(&self, timeout_ms: u32) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u32) -> i32;
        // SAFETY: entry 47 is WaitFrameSync.
        overlay_result(unsafe { entry::<F>(self.table, 47)(timeout_ms) })
    }

    /// The next event for `handle`: first those [`Overlay::inject_event`] queued for it, then SteamVR's.
    pub fn poll_event(&self, handle: OverlayHandle) -> Option<Event> {
        {
            let mut injected = self.injected.borrow_mut();
            if let Some(i) = injected.iter().position(|(h, _)| *h == handle) {
                return injected.remove(i).map(|(_, e)| e);
            }
        }
        type F = unsafe extern "system" fn(u64, *mut Event, u32) -> bool;
        let mut e = Event::default();
        // SAFETY: entry 48 is PollNextOverlayEvent.
        unsafe { entry::<F>(self.table, 48)(handle, &mut e, std::mem::size_of::<Event>() as u32) }.then_some(e)
    }

    /// Queues `event` for `handle`, as SteamVR's laser would send it: the overlays this process points at with its own
    /// lasers (no SteamVR input method, so SteamVR never takes the controllers for them) read the same events.
    pub fn inject_event(&self, handle: OverlayHandle, event: Event) {
        self.injected.borrow_mut().push_back((handle, event));
    }

    /// Laser-pointer mouse input (`VROverlayInputMethod_Mouse`).
    pub fn set_mouse_input(&self, handle: OverlayHandle) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64, i32) -> i32;
        // SAFETY: entry 50 is SetOverlayInputMethod.
        overlay_result(unsafe { entry::<F>(self.table, 50)(handle, 1) })
    }

    /// Size of the mouse coordinate space (the texture size in pixels, usually).
    pub fn set_mouse_scale(&self, handle: OverlayHandle, width: f32, height: f32) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64, *const [f32; 2]) -> i32;
        // SAFETY: entry 52 is SetOverlayMouseScale (HmdVector2_t).
        overlay_result(unsafe { entry::<F>(self.table, 52)(handle, &[width, height]) })
    }

    /// Shows a D3D11 texture (`ID3D11Texture2D*`, gamma-encoded) on the overlay. Keep it alive while it is shown.
    pub fn set_texture_d3d11(&self, handle: OverlayHandle, texture: *mut c_void) -> Result<(), i32> {
        #[repr(C)]
        struct Texture {
            handle: *mut c_void,
            kind: i32,
            color_space: i32,
        }
        type F = unsafe extern "system" fn(u64, *const Texture) -> i32;
        // TextureType_DirectX = 0, ColorSpace_Gamma = 1.
        let t = Texture { handle: texture, kind: 0, color_space: 1 };
        // SAFETY: entry 60 is SetOverlayTexture.
        overlay_result(unsafe { entry::<F>(self.table, 60)(handle, &t) })
    }

    /// The part of the texture the overlay shows, u and v from its top left: a minimum above its maximum flips it.
    pub fn set_texture_bounds(&self, handle: OverlayHandle, u_min: f32, v_min: f32, u_max: f32, v_max: f32) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64, *const [f32; 4]) -> i32;
        // SAFETY: entry 30 is SetOverlayTextureBounds (VRTextureBounds_t: uMin, vMin, uMax, vMax).
        overlay_result(unsafe { entry::<F>(self.table, 30)(handle, &[u_min, v_min, u_max, v_max]) })
    }

    /// Places the overlay in the standing play space (the overlay's +Z faces the viewer).
    pub fn set_transform_absolute(&self, handle: OverlayHandle, transform: &Matrix34) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64, i32, *const Matrix34) -> i32;
        // SAFETY: entry 33 is SetOverlayTransformAbsolute; TrackingUniverseStanding = 1.
        overlay_result(unsafe { entry::<F>(self.table, 33)(handle, 1, transform) })
    }

    pub fn set_alpha(&self, handle: OverlayHandle, alpha: f32) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64, f32) -> i32;
        // SAFETY: entry 16 is SetOverlayAlpha.
        overlay_result(unsafe { entry::<F>(self.table, 16)(handle, alpha) })
    }

    /// How wide a texel is against its height (1: square). The overlay's height is its width × the texture's height
    /// over its width, divided by this: a picture of a fixed size stretched to any length (a laser's beam).
    pub fn set_texel_aspect(&self, handle: OverlayHandle, aspect: f32) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64, f32) -> i32;
        // SAFETY: entry 18 is SetOverlayTexelAspect (openvr_capi.h: after SetOverlayAlpha 16, GetOverlayAlpha 17).
        overlay_result(unsafe { entry::<F>(self.table, 18)(handle, aspect) })
    }

    /// Uploads RGBA pixels (top row first). SteamVR copies them; it flickers when called every frame.
    pub fn set_raw(&self, handle: OverlayHandle, rgba: &[u8], width: u32, height: u32) -> Result<(), i32> {
        type F = unsafe extern "system" fn(u64, *mut c_void, u32, u32, u32) -> i32;
        assert_eq!(rgba.len(), (width * height * 4) as usize);
        // SAFETY: entry 62 is SetOverlayRaw; the runtime only reads the buffer.
        overlay_result(unsafe { entry::<F>(self.table, 62)(handle, rgba.as_ptr() as *mut c_void, width, height, 4) })
    }

    pub fn is_dashboard_visible(&self) -> bool {
        type F = unsafe extern "system" fn() -> bool;
        // SAFETY: entry 68 is IsDashboardVisible.
        unsafe { entry::<F>(self.table, 68)() }
    }

    pub fn is_active_dashboard_overlay(&self, handle: OverlayHandle) -> bool {
        type F = unsafe extern "system" fn(u64) -> bool;
        // SAFETY: entry 69 is IsActiveDashboardOverlay.
        unsafe { entry::<F>(self.table, 69)(handle) }
    }
}

/// A pose in Unity conventions (`tracking.bin`), for device number `number` of `tracking.json`.
pub fn sample_of(time_ns: u64, number: usize, p: &Pose) -> Sample {
    let (position, rotation) = tracking::from_openvr(&p.device_to_absolute.m);
    let [vx, vy, vz] = p.velocity;
    // Angular velocity is an axis (pseudovector): mirroring Z flips its x and y.
    let [wx, wy, wz] = p.angular_velocity;
    Sample {
        time_ns,
        device: number as u8,
        flags: if p.pose_is_valid { tracking::flags::POSE_VALID } else { 0 } | if p.device_is_connected { tracking::flags::CONNECTED } else { 0 },
        result: p.tracking_result.clamp(0, u16::MAX as i32) as u16,
        position,
        rotation,
        velocity: [vx, vy, -vz],
        angular_velocity: [-wx, -wy, wz],
    }
}

/// The tracker role SteamVR's tracker management gave a device, from its controller type (`vive_tracker_left_foot`
/// → `left_foot`); hands from the controller role.
pub fn role_name(class: DeviceClass, role: ControllerRole, controller_type: Option<&str>) -> String {
    match (class, role) {
        (DeviceClass::Hmd, _) => "head".into(),
        (_, ControllerRole::LeftHand) => "left_hand".into(),
        (_, ControllerRole::RightHand) => "right_hand".into(),
        _ => match controller_type.and_then(|t| t.strip_prefix("vive_tracker_")) {
            Some("handed") | None => String::new(),
            Some(role) => role.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_openvr() {
        assert_eq!(std::mem::size_of::<Pose>(), 80);
        assert_eq!(std::mem::size_of::<Matrix34>(), 48);
        assert_eq!(std::mem::size_of::<Event>(), 64);
        assert_eq!(std::mem::offset_of!(Event, data), 16);
        assert_eq!(std::mem::offset_of!(Pose, tracking_result), 72);
        assert_eq!(std::mem::offset_of!(Pose, pose_is_valid), 76);
    }

    #[test]
    fn event_payloads() {
        let mut e = Event { event_type: event::MOUSE_MOVE, ..Default::default() };
        e.data[0] = (0.25f32.to_bits() as u64) | ((0.75f32.to_bits() as u64) << 32);
        e.data[1] = 1;
        assert_eq!(e.mouse(), (0.25, 0.75, 1));
        // One made for an overlay this process points at reads back as SteamVR's would.
        let made = Event::mouse_event(event::MOUSE_BUTTON_DOWN, 512.5, 3.0, 1);
        assert_eq!((made.event_type, made.mouse()), (event::MOUSE_BUTTON_DOWN, (512.5, 3.0, 1)));
        assert_eq!(made.data, Event { data: [(512.5f32.to_bits() as u64) | ((3.0f32.to_bits() as u64) << 32), 1, 0, 0, 0, 0], ..Default::default() }.data);
    }

    #[test]
    fn roles_and_errors() {
        for role in [ControllerRole::Invalid, ControllerRole::LeftHand, ControllerRole::RightHand, ControllerRole::Stylus, ControllerRole::Other(9)] {
            assert_eq!(ControllerRole::from_raw(role.raw()), role);
        }
        // Shutting down counts as not running: a background client is refused instead of starting SteamVR.
        assert!(Error::Init(121).not_running() && Error::Init(119).not_running() && !Error::Init(108).not_running());
        assert_eq!(Error::Init(119).to_string(), "SteamVR is shutting down");
    }

    #[test]
    fn runtime_path_and_roles() {
        let dir = std::env::temp_dir();
        let text = format!(r#"{{ "runtime": ["Z:\\nowhere", {:?}], "jsonid": "vrpathreg" }}"#, dir.to_string_lossy());
        assert_eq!(runtime_from_vrpath(&text), Some(dir));
        assert_eq!(runtime_from_vrpath("{}"), None);
        assert_eq!(role_name(DeviceClass::GenericTracker, ControllerRole::Invalid, Some("vive_tracker_left_foot")), "left_foot");
        assert_eq!(role_name(DeviceClass::Controller, ControllerRole::RightHand, Some("knuckles")), "right_hand");
        assert_eq!(role_name(DeviceClass::Hmd, ControllerRole::Invalid, None), "head");
        assert_eq!(role_name(DeviceClass::GenericTracker, ControllerRole::Invalid, Some("vive_tracker_handed")), "");
    }

    #[test]
    #[ignore = "connects to SteamVR when it runs"]
    fn background_client_never_starts_steamvr() {
        // With SteamVR closed this fails with "not running" (121) instead of launching it; with SteamVR open it
        // connects and lists the headset.
        match Client::connect(AppType::Background) {
            Ok(c) => {
                let mut poses = [Pose::default(); MAX_DEVICES];
                c.poses(Universe::Standing, &mut poses);
                eprintln!("connected; device 0: {:?} {:?}", c.device_class(0), c.string_property(0, prop::MODEL_NUMBER));
            }
            Err(e) => {
                eprintln!("{e}");
                assert!(matches!(e, Error::NotInstalled | Error::Init(_) | Error::Load(_)), "{e:?}");
            }
        }
    }
}
