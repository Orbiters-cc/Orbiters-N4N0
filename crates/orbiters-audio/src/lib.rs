//! Sound in through WASAPI: a microphone, or what a process and its children play (process loopback, Windows 11).
//! [`capture`] hands every packet to its caller with the QueryPerformanceCounter time of its first frame on the
//! shared clock ([`orbiters_core::clock`]), as interleaved float samples; [`capture_packets`] also tells what the
//! device said about each packet, and why the capture ended. The capture worker writes them to session tracks; the
//! voice worker listens to them for Orb.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

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

/// One packet of sound, as [`capture_packets`] hands it over.
#[derive(Clone, Copy, Debug)]
pub struct Packet<'a> {
    /// Time of the packet's first frame (ns, shared clock).
    pub at_ns: u64,
    pub frames: u32,
    /// Interleaved samples; `None` when the packet is silent.
    pub samples: Option<&'a [f32]>,
    /// Sound was lost just before this packet (the reader was late and the device's buffer overflowed).
    pub discontinuity: bool,
    /// The device could not tell when the packet was captured: `at_ns` is not to be trusted.
    pub timestamp_error: bool,
}

/// How [`capture_packets`] captures.
#[derive(Clone, Debug)]
pub struct Options {
    /// A sample rate Windows converts to (`None`: a microphone's own, 48 kHz for a process).
    pub rate: Option<u32>,
    /// How much sound the device holds for a reader that is late: longer survives a busy machine.
    pub buffer: Duration,
    /// The capture thread runs in Windows' multimedia class "Audio" (MMCSS), which keeps it on time under load.
    pub pro_audio: bool,
    /// For the default microphone (`AudioSource::Mic(None)`): end with [`Ended::DefaultChanged`] once Windows'
    /// default microphone is another one.
    pub follow_default: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { rate: None, buffer: Duration::from_millis(100), pro_audio: false, follow_default: false }
    }
}

/// Why a capture that started ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Ended {
    /// `stop` was set.
    Stopped,
    /// The device went away or failed (headset unplugged, Bluetooth reconnecting, the process closed).
    Lost(String),
    /// Windows' default microphone changed.
    DefaultChanged,
    /// The packet callback returned an error.
    Refused(String),
}

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
    mut packet: impl FnMut(u64, u32, Option<&[f32]>) -> anyhow::Result<()>,
) -> anyhow::Result<String> {
    let options = Options { rate, ..Options::default() };
    let (name, ended) = imp::capture_packets(source, &options, stop, ready, |p| packet(p.at_ns, p.frames, p.samples))?;
    if let Ended::Lost(e) | Ended::Refused(e) = &ended {
        eprintln!("{name}: capture ended early: {e}");
    }
    Ok(name)
}

/// [`capture`] with its [`Options`], every packet's flags, and why it ended: errors before the device started are
/// returned, and after that the capture ends with the source's name and an [`Ended`].
pub fn capture_packets(
    source: &AudioSource,
    options: &Options,
    stop: &AtomicBool,
    ready: impl FnMut(&str, Format) -> anyhow::Result<()>,
    packet: impl FnMut(&Packet<'_>) -> anyhow::Result<()>,
) -> anyhow::Result<(String, Ended)> {
    imp::capture_packets(source, options, stop, ready, packet)
}

/// The id of Windows' default microphone, `None` when there is none.
pub fn default_mic_id() -> Option<String> {
    imp::default_mic_id()
}

#[cfg(windows)]
mod imp {
    use super::{AudioSource, Device, Ended, Format, Options, Packet};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use windows::core::{implement, Interface, Ref, HRESULT, HSTRING, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, PROPERTYKEY, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, BLOB, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
    use windows::Win32::System::Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW, WaitForSingleObject};
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

    pub(crate) fn format(tag: u16, rate: u32, channels: u16) -> WAVEFORMATEX {
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

    pub fn default_mic_id() -> Option<String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            enumerator.GetDefaultAudioEndpoint(eCapture, eConsole).ok().and_then(|d| device_id(&d))
        }
    }

    /// How often a capture of the default microphone looks whether Windows' default changed.
    const DEFAULT_CHECK_EVERY: Duration = Duration::from_secs(1);

    pub fn capture_packets(
        source: &AudioSource,
        options: &Options,
        stop: &AtomicBool,
        mut ready: impl FnMut(&str, Format) -> anyhow::Result<()>,
        mut packet: impl FnMut(&Packet<'_>) -> anyhow::Result<()>,
    ) -> anyhow::Result<(String, Ended)> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            // The default microphone, followed: its enumerator and the id it had when opened.
            let mut follow: Option<(IMMDeviceEnumerator, Option<String>)> = None;
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
                    if id.is_none() && options.follow_default {
                        follow = Some((enumerator, device_id(&device)));
                    }
                    // A voice: mono at the device's rate (or the one asked for).
                    (client, name, options.rate.unwrap_or(own), 1u16, false)
                }
                AudioSource::Process(pid) => (process_loopback(*pid)?, format!("VRChat (process {pid})"), options.rate.unwrap_or(48_000), 2u16, true),
            };
            let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
                | if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
            // The buffer lets the thread be late without losing sound (in 100 ns units, 10 ms to 2 s).
            let buffer = (options.buffer.as_nanos() / 100).clamp(100_000, 20_000_000) as i64;
            let mut tag = WAVE_FORMAT_IEEE_FLOAT;
            if client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, buffer, 0, &format(tag, rate, channels), None).is_err() {
                tag = WAVE_FORMAT_PCM;
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, buffer, 0, &format(tag, rate, channels), None)?;
            }
            let event = CreateEventW(None, false, false, None)?;
            let result = (|| -> anyhow::Result<Ended> {
                client.SetEventHandle(event)?;
                let capture: IAudioCaptureClient = client.GetService()?;
                ready(&name, Format { rate, channels })?;
                client.Start()?;
                // On time while the machine is busy (VRChat, a reconstruction): Windows' "Audio" task class.
                let mmcss = if options.pro_audio {
                    let mut index = 0u32;
                    AvSetMmThreadCharacteristicsW(windows::core::w!("Audio"), &mut index).ok()
                } else {
                    None
                };
                let mut samples: Vec<f32> = Vec::new();
                let mut checked = Instant::now();
                // A device that goes away ends the capture; what was delivered stays good.
                let captured = (|| -> Result<Ended, Ended> {
                    let lost = |e: windows::core::Error| Ended::Lost(format!("{e}"));
                    while !stop.load(Ordering::Relaxed) {
                        if let Some((enumerator, opened)) = &follow {
                            if checked.elapsed() >= DEFAULT_CHECK_EVERY {
                                checked = Instant::now();
                                let now = enumerator.GetDefaultAudioEndpoint(eCapture, eConsole).ok().and_then(|d| device_id(&d));
                                if now.is_some() && now != *opened {
                                    return Ok(Ended::DefaultChanged);
                                }
                            }
                        }
                        if WaitForSingleObject(event, 100) != WAIT_OBJECT_0 {
                            continue;
                        }
                        while capture.GetNextPacketSize().map_err(lost)? > 0 {
                            let (mut data, mut frames, mut buffer_flags, mut qpc) = (std::ptr::null_mut(), 0u32, 0u32, 0u64);
                            capture.GetBuffer(&mut data, &mut frames, &mut buffer_flags, None, Some(&mut qpc)).map_err(lost)?;
                            let at = if qpc > 0 { orbiters_core::clock::from_qpc_100ns(qpc) } else { orbiters_core::clock::now_ns() };
                            let flag = |f: _AUDCLNT_BUFFERFLAGS| buffer_flags & f.0 as u32 != 0;
                            let n = frames as usize * channels as usize;
                            let silent = flag(AUDCLNT_BUFFERFLAGS_SILENT) || data.is_null();
                            if !silent {
                                samples.clear();
                                if tag == WAVE_FORMAT_IEEE_FLOAT {
                                    samples.extend_from_slice(std::slice::from_raw_parts(data.cast::<f32>(), n));
                                } else {
                                    samples.extend(std::slice::from_raw_parts(data.cast::<i16>(), n).iter().map(|v| *v as f32 / 32768.0));
                                }
                            }
                            let delivered = packet(&Packet {
                                at_ns: at,
                                frames,
                                samples: (!silent).then_some(samples.as_slice()),
                                discontinuity: flag(AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY),
                                // A device that gives no QPC time at all is timed on arrival (`at` above), as always.
                                timestamp_error: flag(AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR),
                            });
                            capture.ReleaseBuffer(frames).map_err(lost)?;
                            delivered.map_err(|e| Ended::Refused(format!("{e:#}")))?;
                        }
                    }
                    Ok(Ended::Stopped)
                })();
                if let Some(handle) = mmcss {
                    let _ = AvRevertMmThreadCharacteristics(handle);
                }
                let _ = client.Stop();
                Ok(captured.unwrap_or_else(|ended| ended))
            })();
            let _ = CloseHandle(event);
            Ok((name, result?))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{AudioSource, Device, Ended, Format, Options, Packet};
    use std::sync::atomic::AtomicBool;

    pub fn list_mics() -> Vec<Device> {
        Vec::new()
    }

    pub fn default_mic_id() -> Option<String> {
        None
    }

    pub fn capture_packets(
        _source: &AudioSource,
        _options: &Options,
        _stop: &AtomicBool,
        _ready: impl FnMut(&str, Format) -> anyhow::Result<()>,
        _packet: impl FnMut(&Packet<'_>) -> anyhow::Result<()>,
    ) -> anyhow::Result<(String, Ended)> {
        anyhow::bail!("sound capture is only available on Windows for now")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_that_does_not_exist_fails_before_anything_starts() {
        let stop = AtomicBool::new(false);
        let mut opened = false;
        let source = AudioSource::Mic(Some("{0.0.1.00000000}.{orbiters-no-such-device}".into()));
        let r = capture_packets(
            &source,
            &Options::default(),
            &stop,
            |_, _| {
                opened = true;
                Ok(())
            },
            |_| Ok(()),
        );
        assert!(r.is_err() && !opened);
        assert!(capture(&source, None, &stop, |_, _| Ok(()), |_, _, _| Ok(())).is_err());
    }

    #[test]
    fn the_default_options_are_what_capture_always_did() {
        let o = Options::default();
        assert_eq!((o.rate, o.buffer, o.pro_audio, o.follow_default), (None, Duration::from_millis(100), false, false));
    }

    #[cfg(windows)]
    #[test]
    fn wave_formats_add_up() {
        let f = imp::format(3, 48_000, 2);
        let (block, rate, bits) = (f.nBlockAlign, f.nAvgBytesPerSec, f.wBitsPerSample);
        assert_eq!((block, rate, bits), (8, 384_000, 32));
        let f = imp::format(1, 44_100, 1);
        let (block, rate, bits) = (f.nBlockAlign, f.nAvgBytesPerSec, f.wBitsPerSample);
        assert_eq!((block, rate, bits), (2, 88_200, 16));
    }
}
