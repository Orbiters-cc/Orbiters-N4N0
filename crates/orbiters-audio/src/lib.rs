//! Sound in through WASAPI: a microphone, or what a process and its children play (process loopback, Windows 11).
//! [`capture`] hands every packet to its caller with the QueryPerformanceCounter time of its first frame on the
//! shared clock ([`orbiters_core::clock`]), as interleaved float samples. The capture worker writes them to session
//! tracks; the voice worker listens to them for Orb.

use std::sync::atomic::AtomicBool;

/// Where the sound comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioSource {
    /// A capture endpoint by id; `None` is the Windows default microphone.
    Mic(Option<String>),
    /// What a process and its children play.
    Process(u32),
}

/// A capture device, for the settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// The samples' rate and channels as delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub rate: u32,
    pub channels: u16,
}

pub use imp::list_mics;

/// Captures `source` until `stop`. `rate` asks for a sample rate Windows converts to (`None`: a microphone's own,
/// 48 kHz for a process); a microphone comes mono, a process stereo. `ready` gets the source's name and the format
/// once the device opened, before the first packet (an error from it ends the capture); `packet` gets each packet's
/// time (ns, shared clock), its frames, and its samples unless the packet is silent. A device that goes away
/// (headset unplugged, process closed) or a `packet` error ends the capture early, with the source's name; errors
/// before it started are returned.
pub fn capture(
    source: &AudioSource,
    rate: Option<u32>,
    stop: &AtomicBool,
    ready: impl FnMut(&str, Format) -> anyhow::Result<()>,
    packet: impl FnMut(u64, u32, Option<&[f32]>) -> anyhow::Result<()>,
) -> anyhow::Result<String> {
    imp::capture(source, rate, stop, ready, packet)
}

#[cfg(windows)]
mod imp {
    use super::{AudioSource, Device, Format};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;
    use windows::core::{implement, Interface, Ref, HRESULT, HSTRING, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, PROPERTYKEY, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, BLOB, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows::Win32::System::Variant::{VT_BLOB, VT_LPWSTR};

    const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };
    const WAVE_FORMAT_PCM: u16 = 1;
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;

    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct Activated(mpsc::SyncSender<()>);

    impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
        fn ActivateCompleted(&self, _operation: Ref<IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
            let _ = self.0.try_send(());
            Ok(())
        }
    }

    unsafe fn friendly_name(device: &IMMDevice) -> Option<String> {
        unsafe {
            let store = device.OpenPropertyStore(STGM_READ).ok()?;
            // Cleared when dropped.
            let value: PROPVARIANT = store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME).ok()?;
            let inner = &value.Anonymous.Anonymous;
            if inner.vt == VT_LPWSTR {
                inner.Anonymous.pwszVal.to_string().ok()
            } else {
                None
            }
        }
    }

    unsafe fn device_id(device: &IMMDevice) -> Option<String> {
        unsafe {
            let id: PWSTR = device.GetId().ok()?;
            let s = id.to_string().ok();
            CoTaskMemFree(Some(id.0 as _));
            s
        }
    }

    pub fn list_mics() -> Vec<Device> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(enumerator) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return Vec::new() };
            let default = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole).ok().and_then(|d| device_id(&d));
            let Ok(collection) = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else { return Vec::new() };
            let mut out = Vec::new();
            for i in 0..collection.GetCount().unwrap_or(0) {
                let Ok(device) = collection.Item(i) else { continue };
                let Some(id) = device_id(&device) else { continue };
                out.push(Device { name: friendly_name(&device).unwrap_or_else(|| id.clone()), default: default.as_deref() == Some(id.as_str()), id });
            }
            out
        }
    }

    unsafe fn process_loopback(pid: u32) -> anyhow::Result<IAudioClient> {
        unsafe {
            let params = AUDIOCLIENT_ACTIVATION_PARAMS {
                ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                    ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                        TargetProcessId: pid,
                        ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                    },
                },
            };
            // `PROPVARIANT` clears itself on drop, which would free the borrowed blob: never drop this one.
            let mut variant = std::mem::ManuallyDrop::new(PROPVARIANT::default());
            {
                let inner = &mut *variant.Anonymous.Anonymous;
                inner.vt = VT_BLOB;
                inner.Anonymous.blob = BLOB { cbSize: std::mem::size_of_val(&params) as u32, pBlobData: &params as *const _ as *mut u8 };
            }
            let (tx, rx) = mpsc::sync_channel(1);
            let handler: IActivateAudioInterfaceCompletionHandler = Activated(tx).into();
            let operation = ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, &IAudioClient::IID, Some(&*variant), &handler)?;
            rx.recv_timeout(Duration::from_secs(5)).map_err(|_| anyhow::anyhow!("process loopback did not activate"))?;
            let mut result = HRESULT(0);
            let mut unknown = None;
            operation.GetActivateResult(&mut result, &mut unknown)?;
            result.ok()?;
            Ok(unknown.ok_or_else(|| anyhow::anyhow!("no audio client"))?.cast()?)
        }
    }

    fn format(tag: u16, rate: u32, channels: u16) -> WAVEFORMATEX {
        let bits = if tag == WAVE_FORMAT_IEEE_FLOAT { 32 } else { 16 };
        let block = channels * bits / 8;
        WAVEFORMATEX {
            wFormatTag: tag,
            nChannels: channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * block as u32,
            nBlockAlign: block,
            wBitsPerSample: bits,
            cbSize: 0,
        }
    }

    pub fn capture(
        source: &AudioSource,
        rate: Option<u32>,
        stop: &AtomicBool,
        mut ready: impl FnMut(&str, Format) -> anyhow::Result<()>,
        mut packet: impl FnMut(u64, u32, Option<&[f32]>) -> anyhow::Result<()>,
    ) -> anyhow::Result<String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            let (client, name, rate, channels, loopback) = match source {
                AudioSource::Mic(id) => {
                    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                    let device = match id {
                        Some(id) => enumerator.GetDevice(&HSTRING::from(id.as_str()))?,
                        None => enumerator.GetDefaultAudioEndpoint(eCapture, eConsole)?,
                    };
                    let name = friendly_name(&device).unwrap_or_else(|| "Microphone".into());
                    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
                    let mix = client.GetMixFormat()?;
                    let own = (*mix).nSamplesPerSec;
                    CoTaskMemFree(Some(mix as _));
                    // A voice: mono at the device's rate (or the one asked for).
                    (client, name, rate.unwrap_or(own), 1u16, false)
                }
                AudioSource::Process(pid) => (process_loopback(*pid)?, format!("VRChat (process {pid})"), rate.unwrap_or(48_000), 2u16, true),
            };
            let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
                | if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
            // 100 ms of buffer: the thread may be late without losing sound.
            let mut tag = WAVE_FORMAT_IEEE_FLOAT;
            if client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 1_000_000, 0, &format(tag, rate, channels), None).is_err() {
                tag = WAVE_FORMAT_PCM;
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 1_000_000, 0, &format(tag, rate, channels), None)?;
            }
            let event = CreateEventW(None, false, false, None)?;
            let result = (|| -> anyhow::Result<()> {
                client.SetEventHandle(event)?;
                let capture: IAudioCaptureClient = client.GetService()?;
                ready(&name, Format { rate, channels })?;
                client.Start()?;
                let mut samples: Vec<f32> = Vec::new();
                // A device that goes away ends the capture; what was delivered stays good.
                let captured = (|| -> anyhow::Result<()> {
                    while !stop.load(Ordering::Relaxed) {
                        if WaitForSingleObject(event, 100) != WAIT_OBJECT_0 {
                            continue;
                        }
                        while capture.GetNextPacketSize()? > 0 {
                            let (mut data, mut frames, mut buffer_flags, mut qpc) = (std::ptr::null_mut(), 0u32, 0u32, 0u64);
                            capture.GetBuffer(&mut data, &mut frames, &mut buffer_flags, None, Some(&mut qpc))?;
                            let at = if qpc > 0 { orbiters_core::clock::from_qpc_100ns(qpc) } else { orbiters_core::clock::now_ns() };
                            let silent = buffer_flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                            let n = frames as usize * channels as usize;
                            let delivered = if silent || data.is_null() {
                                packet(at, frames, None)
                            } else {
                                samples.clear();
                                if tag == WAVE_FORMAT_IEEE_FLOAT {
                                    samples.extend_from_slice(std::slice::from_raw_parts(data.cast::<f32>(), n));
                                } else {
                                    samples.extend(std::slice::from_raw_parts(data.cast::<i16>(), n).iter().map(|v| *v as f32 / 32768.0));
                                }
                                packet(at, frames, Some(&samples))
                            };
                            capture.ReleaseBuffer(frames)?;
                            delivered?;
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = captured {
                    eprintln!("{name}: capture ended early: {e:#}");
                }
                let _ = client.Stop();
                Ok(())
            })();
            let _ = CloseHandle(event);
            result?;
            Ok(name)
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{AudioSource, Device, Format};
    use std::sync::atomic::AtomicBool;

    pub fn list_mics() -> Vec<Device> {
        Vec::new()
    }

    pub fn capture(
        _source: &AudioSource,
        _rate: Option<u32>,
        _stop: &AtomicBool,
        _ready: impl FnMut(&str, Format) -> anyhow::Result<()>,
        _packet: impl FnMut(u64, u32, Option<&[f32]>) -> anyhow::Result<()>,
    ) -> anyhow::Result<String> {
        anyhow::bail!("sound capture is only available on Windows for now")
    }
}
