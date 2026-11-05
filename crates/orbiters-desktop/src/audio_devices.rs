//! Windows' sound devices for "When SteamVR starts" (Settings, `controller/vr_audio.rs`): the outputs and inputs
//! (WASAPI's endpoints, `IMMDeviceEnumerator`), the defaults of each of Windows' roles, setting a default, and a test
//! sound played on one output.
//!
//! Windows has no public call that sets the default device. Its Sound settings use `IPolicyConfig` from
//! `CPolicyConfigClient`, undocumented but stable since Windows 7 and what SoundSwitch, EarTrumpet and nircmd call: it is
//! declared here by hand, slot by slot ([`PolicyConfigVtbl`]), and only `SetDefaultEndpoint` is called. The device's
//! name is read through its property store, declared the same way, so that no more of the `windows` crate is built.
//!
//! Each call opens COM on its thread (multithreaded) and blocks briefly (a few milliseconds; [`play_test`] about two
//! seconds): call them from a worker thread, never the interface's.
//!
//! On macOS the devices are Core Audio's (`orbiters_sound::coreaudio`), for the microphone pickers and the tests of a
//! device. Nothing sets a default there: setting them is for SteamVR, which does not run on a Mac.

pub use orbiters_core::settings::AudioEndpoint;

/// Which way the sound goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Speakers, headphones (render endpoints).
    Output,
    /// Microphones (capture endpoints).
    Input,
}

/// Windows' three default devices per flow: games and system sounds (console), music and videos (multimedia), calls
/// (communications). The Sound settings set the first two together; "Default communication device" is the third.
pub const ROLES: [Role; 3] = [Role::Console, Role::Multimedia, Role::Communications];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Console,
    Multimedia,
    Communications,
}

/// The devices of `flow` that are there and on (unplugged and turned-off ones are not), as Windows names them.
pub fn list(flow: Flow) -> Result<Vec<AudioEndpoint>, String> {
    if let Some((outputs, inputs)) = STAND_IN.get() {
        return Ok(if flow == Flow::Output { outputs.clone() } else { inputs.clone() });
    }
    imp::list(flow).map_err(|e| e.to_string())
}

/// Devices [`list`] gives instead of this computer's (screenshots).
static STAND_IN: std::sync::OnceLock<(Vec<AudioEndpoint>, Vec<AudioEndpoint>)> = std::sync::OnceLock::new();

pub fn stand_in(outputs: Vec<AudioEndpoint>, inputs: Vec<AudioEndpoint>) {
    let _ = STAND_IN.set((outputs, inputs));
}

/// The default device of each of [`ROLES`] for `flow` (`None` where Windows has none).
pub fn defaults(flow: Flow) -> [Option<AudioEndpoint>; 3] {
    imp::defaults(flow)
}

/// Makes the device `id` Windows' default for `role` (its flow is the device's own).
pub fn set_default(id: &str, role: Role) -> Result<(), String> {
    imp::set_default(id, role).map_err(|e| e.to_string())
}

/// Plays a short sound on the output `id` (whatever the default is), returning once it has played.
pub fn play_test(id: &str) -> Result<(), String> {
    imp::play_test(id).map_err(|e| e.to_string())
}

#[cfg(windows)]
mod imp {
    use super::{AudioEndpoint, Flow, Role, ROLES};
    use std::ffi::c_void;
    use std::time::{Duration, Instant};
    use windows::core::{IUnknown, IUnknown_Vtbl, Interface, GUID, HRESULT, HSTRING, PCWSTR};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::{
        eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, ERole, IAudioClient, IAudioRenderClient, IMMDevice,
        IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, DEVICE_STATE_ACTIVE,
        WAVEFORMATEX,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
    use windows::Win32::System::Variant::VT_LPWSTR;

    /// `CPolicyConfigClient`, the Sound settings' own object.
    const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    /// `IPolicyConfig` (Windows 7 to 11; the one SoundSwitch and EarTrumpet use).
    #[repr(transparent)]
    #[derive(Clone)]
    struct PolicyConfig(IUnknown);

    unsafe impl Interface for PolicyConfig {
        type Vtable = PolicyConfigVtbl;
        const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
    }

    /// `IPolicyConfig`'s slots in order. Only `SetDefaultEndpoint` is called; the others hold their place.
    #[repr(C)]
    #[allow(non_snake_case)]
    struct PolicyConfigVtbl {
        base: IUnknown_Vtbl,
        GetMixFormat: usize,
        GetDeviceFormat: usize,
        ResetDeviceFormat: usize,
        SetDeviceFormat: usize,
        GetProcessingPeriod: usize,
        SetProcessingPeriod: usize,
        GetShareMode: usize,
        SetShareMode: usize,
        GetPropertyValue: usize,
        SetPropertyValue: usize,
        /// `HRESULT SetDefaultEndpoint(PCWSTR wszDeviceId, ERole eRole)`.
        SetDefaultEndpoint: unsafe extern "system" fn(this: *mut c_void, id: PCWSTR, role: ERole) -> HRESULT,
        SetEndpointVisibility: usize,
    }

    /// `IMMDevice` as far as its property store, declared here so that reading a name needs no more of the `windows`
    /// crate (its `OpenPropertyStore` comes with the Shell's property system).
    #[repr(transparent)]
    #[derive(Clone)]
    struct DeviceProperties(IUnknown);

    unsafe impl Interface for DeviceProperties {
        type Vtable = DevicePropertiesVtbl;
        const IID: GUID = IMMDevice::IID;
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct DevicePropertiesVtbl {
        base: IUnknown_Vtbl,
        Activate: usize,
        /// `HRESULT OpenPropertyStore(DWORD stgmAccess, IPropertyStore **ppProperties)`.
        OpenPropertyStore: unsafe extern "system" fn(this: *mut c_void, access: u32, store: *mut *mut c_void) -> HRESULT,
        GetId: usize,
        GetState: usize,
    }

    /// `IPropertyStore`.
    #[repr(transparent)]
    #[derive(Clone)]
    struct PropertyStore(IUnknown);

    unsafe impl Interface for PropertyStore {
        type Vtable = PropertyStoreVtbl;
        const IID: GUID = GUID::from_u128(0x886d8eeb_8cf2_4446_8d02_cdba1dbdcf99);
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct PropertyStoreVtbl {
        base: IUnknown_Vtbl,
        GetCount: usize,
        GetAt: usize,
        /// `HRESULT GetValue(REFPROPERTYKEY key, PROPVARIANT *pv)`.
        GetValue: unsafe extern "system" fn(this: *mut c_void, key: *const PROPERTYKEY, value: *mut PROPVARIANT) -> HRESULT,
        SetValue: usize,
        Commit: usize,
    }

    /// `PKEY_Device_FriendlyName`: "Speakers (Valve Index)".
    const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };
    const STGM_READ: u32 = 0;

    fn data_flow(flow: Flow) -> EDataFlow {
        match flow {
            Flow::Output => eRender,
            Flow::Input => eCapture,
        }
    }

    fn role(role: Role) -> ERole {
        match role {
            Role::Console => eConsole,
            Role::Multimedia => eMultimedia,
            Role::Communications => eCommunications,
        }
    }

    fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
        // SAFETY: COM for this thread, multithreaded (a thread already in another apartment keeps it, and the calls
        // below work there too); the enumerator is Windows' own.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        }
    }

    fn id(device: &IMMDevice) -> Option<String> {
        // SAFETY: `GetId` hands over a string this frees.
        unsafe {
            let id = device.GetId().ok()?;
            let text = id.to_string().ok();
            CoTaskMemFree(Some(id.0 as *const c_void));
            text
        }
    }

    fn name(device: &IMMDevice) -> Option<String> {
        let device: DeviceProperties = device.cast().ok()?;
        // SAFETY: the slots as declared above; the store is released when dropped, the value cleared when dropped.
        unsafe {
            let mut raw = std::ptr::null_mut();
            (device.vtable().OpenPropertyStore)(device.as_raw(), STGM_READ, &mut raw).ok().ok()?;
            if raw.is_null() {
                return None;
            }
            let store = PropertyStore::from_raw(raw);
            let mut value = PROPVARIANT::default();
            (store.vtable().GetValue)(store.as_raw(), &PKEY_DEVICE_FRIENDLY_NAME, &mut value).ok().ok()?;
            let inner = &value.Anonymous.Anonymous;
            if inner.vt == VT_LPWSTR {
                inner.Anonymous.pwszVal.to_string().ok()
            } else {
                None
            }
        }
    }

    fn endpoint(device: &IMMDevice) -> Option<AudioEndpoint> {
        let id = id(device)?;
        Some(AudioEndpoint { name: name(device).unwrap_or_else(|| id.clone()), id })
    }

    pub fn list(flow: Flow) -> windows::core::Result<Vec<AudioEndpoint>> {
        let devices = enumerator()?;
        // SAFETY: plain calls on Windows' objects.
        unsafe {
            let collection = devices.EnumAudioEndpoints(data_flow(flow), DEVICE_STATE_ACTIVE)?;
            let mut out = Vec::new();
            for i in 0..collection.GetCount()? {
                if let Some(e) = collection.Item(i).ok().as_ref().and_then(endpoint) {
                    out.push(e);
                }
            }
            Ok(out)
        }
    }

    pub fn defaults(flow: Flow) -> [Option<AudioEndpoint>; 3] {
        let Ok(devices) = enumerator() else { return Default::default() };
        // SAFETY: plain calls on Windows' objects; no default is an error, read as none.
        ROLES.map(|r| unsafe { devices.GetDefaultAudioEndpoint(data_flow(flow), role(r)).ok().as_ref().and_then(endpoint) })
    }

    pub fn set_default(id: &str, r: Role) -> windows::core::Result<()> {
        // SAFETY: as `enumerator`; the policy object is Windows' own, called through the slot declared above with a
        // string that outlives the call.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let policy: PolicyConfig = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)?;
            let id = HSTRING::from(id);
            (policy.vtable().SetDefaultEndpoint)(policy.as_raw(), PCWSTR(id.as_ptr()), role(r)).ok()
        }
    }

    const TEST_RATE: u32 = 48_000;

    pub fn play_test(id: &str) -> anyhow::Result<()> {
        let sound = orbiters_sound::render(&orbiters_sound::sounds::REVEAL, TEST_RATE);
        let devices = enumerator()?;
        // SAFETY: Windows' objects; the buffer it hands out is written within the frames it gave, then released.
        unsafe {
            let device = devices.GetDevice(&HSTRING::from(id))?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let block = 2 * 4;
            let format = WAVEFORMATEX {
                wFormatTag: 3, // IEEE float
                nChannels: 2,
                nSamplesPerSec: TEST_RATE,
                nAvgBytesPerSec: TEST_RATE * block,
                nBlockAlign: block as u16,
                wBitsPerSample: 32,
                cbSize: 0,
            };
            // Converted to the device's own format by Windows; 200 ms of buffer, topped up every 20 ms.
            const SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | SRC_DEFAULT_QUALITY, 2_000_000, 0, &format, None)?;
            let render: IAudioRenderClient = client.GetService()?;
            let size = client.GetBufferSize()?;
            let mut at = 0usize;
            let mut started = false;
            let deadline = Instant::now() + Duration::from_secs_f64(sound.seconds() + 2.0);
            loop {
                let padding = client.GetCurrentPadding()?;
                if at >= sound.frames.len() {
                    if padding == 0 || Instant::now() > deadline {
                        break;
                    }
                } else {
                    let n = ((size - padding) as usize).min(sound.frames.len() - at);
                    if n > 0 {
                        let data = render.GetBuffer(n as u32)?.cast::<[f32; 2]>();
                        std::ptr::copy_nonoverlapping(sound.frames[at..].as_ptr(), data, n);
                        render.ReleaseBuffer(n as u32, 0)?;
                        at += n;
                    }
                    if !started {
                        client.Start()?;
                        started = true;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = client.Stop();
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{AudioEndpoint, Flow, Role};
    use orbiters_sound::coreaudio as ca;

    fn flow_of(flow: Flow) -> ca::Flow {
        match flow {
            Flow::Output => ca::Flow::Output,
            Flow::Input => ca::Flow::Input,
        }
    }

    fn endpoint(device: ca::DeviceInfo) -> AudioEndpoint {
        AudioEndpoint { id: device.uid, name: device.name }
    }

    pub fn list(flow: Flow) -> Result<Vec<AudioEndpoint>, &'static str> {
        Ok(ca::devices(flow_of(flow)).into_iter().map(endpoint).collect())
    }

    /// macOS has one default for each direction, whatever the role.
    pub fn defaults(flow: Flow) -> [Option<AudioEndpoint>; 3] {
        let default = ca::default_uid(flow_of(flow)).and_then(|uid| ca::device(&uid)).map(endpoint);
        [default.clone(), default.clone(), default]
    }

    pub fn set_default(_: &str, _: Role) -> Result<(), &'static str> {
        Err("the default devices change for SteamVR only, which does not run on a Mac")
    }

    pub fn play_test(id: &str) -> Result<(), String> {
        let sound = orbiters_sound::render(&orbiters_sound::sounds::REVEAL, orbiters_sound::RATE);
        ca::play_on(id, &sound)
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use super::{AudioEndpoint, Flow, Role};

    pub fn list(_: Flow) -> Result<Vec<AudioEndpoint>, &'static str> {
        Ok(Vec::new())
    }

    pub fn defaults(_: Flow) -> [Option<AudioEndpoint>; 3] {
        Default::default()
    }

    pub fn set_default(_: &str, _: Role) -> Result<(), &'static str> {
        Err("only on Windows")
    }

    pub fn play_test(_: &str) -> Result<(), &'static str> {
        Err("only on Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This computer's devices and defaults, read only (nothing is set):
    /// `cargo test -p orbiters-desktop --bin orbiters-desktop lists_this_computers -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn lists_this_computers_devices() {
        for flow in [Flow::Output, Flow::Input] {
            let devices = list(flow).unwrap();
            println!("{flow:?}: {devices:#?}");
            println!("defaults (console, multimedia, communications): {:#?}", defaults(flow));
            assert!(devices.iter().all(|d| !d.id.is_empty() && !d.name.is_empty()));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_mac_changes_no_default_and_its_lists_read() {
        for flow in [Flow::Output, Flow::Input] {
            let devices = list(flow).unwrap();
            assert!(devices.iter().all(|d| !d.id.is_empty() && !d.name.is_empty()));
            let [console, multimedia, communications] = defaults(flow);
            assert_eq!(console, multimedia);
            assert_eq!(console, communications);
            if let Some(default) = console {
                assert!(devices.contains(&default), "the default is among the devices");
            }
        }
        assert!(set_default("anything", Role::Console).is_err());
        assert!(play_test("orbiters-no-such-device").is_err());
    }
}
