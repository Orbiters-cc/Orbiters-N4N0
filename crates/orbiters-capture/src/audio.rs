//! Audio of a session (contracts `Docs/performance.md`): the person's microphone and what VRChat plays, each
//! captured with WASAPI on its own thread into a 32-bit float WAVE file. Every packet carries the
//! QueryPerformanceCounter time of its first frame, which places it on the session clock: `sync` keeps one
//! (frame, time) pair a second, and a gap (loopback delivers nothing while VRChat is silent) is filled with
//! silence so the file plays in step with the session. VRChat's sound comes from process loopback (Windows 11),
//! so music players, voice chat and notifications are not recorded.

use orbiters_contracts::performance::audio::{self, Track, TrackKind};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Where a track's sound comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioSource {
    /// A capture endpoint by id; `None` is the Windows default microphone.
    Mic(Option<String>),
    /// What a process and its children play.
    Process(u32),
}

/// Silence longer than this between packets is written as zeros; shorter offsets are clock drift, left to `sync`.
const GAP_NS: u64 = 20_000_000;

/// How often a track's header gets the frames written so far ([`TrackWriter::checkpoint`]).
const CHECKPOINT_EVERY: Duration = Duration::from_secs(2);

/// The track as its last checkpoint left it on disk, for `audio.json` while recording (`file`, `kind` and `source`
/// are the recorder's).
type Published = Arc<Mutex<Option<Track>>>;

/// Places packets on the session clock and writes them (format-independent, so it is tested without a device).
struct TrackWriter {
    file: BufWriter<File>,
    rate: u32,
    channels: u16,
    frames: u64,
    first_ns: Option<u64>,
    sync: Vec<[i64; 2]>,
    last_sync_ns: u64,
    zeros: Vec<f32>,
    published: Published,
    checkpointed: Instant,
}

impl TrackWriter {
    fn create(path: &Path, rate: u32, channels: u16, published: Published) -> anyhow::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = BufWriter::with_capacity(1 << 16, File::create(path)?);
        file.write_all(&audio::wav_header(rate, channels, 0))?;
        Ok(Self {
            file,
            rate,
            channels,
            frames: 0,
            first_ns: None,
            sync: Vec::new(),
            last_sync_ns: 0,
            zeros: Vec::new(),
            published,
            checkpointed: Instant::now(),
        })
    }

    fn write_zeros(&mut self, frames: u64) -> anyhow::Result<()> {
        let mut left = frames * self.channels as u64;
        while left > 0 {
            let n = left.min(48_000) as usize;
            if self.zeros.len() < n {
                self.zeros.resize(n, 0.0);
            }
            self.file.write_all(bytemuck_f32(&self.zeros[..n]))?;
            left -= n as u64;
        }
        Ok(())
    }

    /// One packet whose first frame was captured at session time `t_ns`; `samples` is `None` for silence.
    fn packet(&mut self, t_ns: u64, frames: u32, samples: Option<&[f32]>) -> anyhow::Result<()> {
        let first = *self.first_ns.get_or_insert(t_ns);
        let expected = first + self.frames * 1_000_000_000 / self.rate as u64;
        if t_ns > expected + GAP_NS {
            let missing = (t_ns - expected) * self.rate as u64 / 1_000_000_000;
            self.write_zeros(missing)?;
            self.frames += missing;
            self.sync.push([self.frames as i64, t_ns as i64]);
            self.last_sync_ns = t_ns;
        } else if self.sync.is_empty() || t_ns >= self.last_sync_ns + 1_000_000_000 {
            self.sync.push([self.frames as i64, t_ns as i64]);
            self.last_sync_ns = t_ns;
        }
        match samples {
            Some(s) => self.file.write_all(bytemuck_f32(&s[..frames as usize * self.channels as usize]))?,
            None => self.write_zeros(frames as u64)?,
        }
        self.frames += frames as u64;
        if self.checkpointed.elapsed() >= CHECKPOINT_EVERY {
            self.checkpoint()?;
        }
        Ok(())
    }

    /// Makes the file readable as it stands: its header gets the frames written so far, and `published` the track
    /// as `audio.json` gives it (the session writes that file at its own checkpoints). A recording that never
    /// finishes (the worker killed, Windows shut down) keeps its sound up to here.
    fn checkpoint(&mut self) -> anyhow::Result<()> {
        self.checkpointed = Instant::now();
        self.file.flush()?;
        let f = self.file.get_mut();
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&audio::wav_header(self.rate, self.channels, self.frames))?;
        f.seek(SeekFrom::End(0))?;
        *self.published.lock().unwrap() = Some(self.track());
        Ok(())
    }

    /// The track so far, without its file, kind and source.
    fn track(&self) -> Track {
        Track {
            sample_rate: self.rate,
            channels: self.channels,
            frames: self.frames,
            first_frame_ns: self.first_ns.unwrap_or(0) as i64,
            sync: self.sync.clone(),
            ..Default::default()
        }
    }

    fn finish(mut self, file: String, kind: TrackKind, source: String) -> anyhow::Result<Track> {
        self.file.flush()?;
        let track = self.track();
        let mut f = self.file.into_inner().map_err(|e| e.into_error())?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&audio::wav_header(self.rate, self.channels, self.frames))?;
        Ok(Track { file, kind, source, ..track })
    }
}

fn bytemuck_f32(s: &[f32]) -> &[u8] {
    // SAFETY: f32 has no padding and any byte pattern is a valid u8; WAVE data is little-endian like the host.
    unsafe { std::slice::from_raw_parts(s.as_ptr().cast::<u8>(), std::mem::size_of_val(s)) }
}

/// A track being recorded. Dropping it without [`AudioRecorder::finish`] stops the thread too.
pub struct AudioRecorder {
    pub kind: TrackKind,
    pub source_name: String,
    /// Relative to the bundle (`audio/mic.wav`).
    file: String,
    published: Published,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    /// Peak level since the last [`AudioRecorder::take_level`], as f32 bits.
    level: Arc<AtomicU32>,
    thread: Option<JoinHandle<anyhow::Result<Track>>>,
}

impl AudioRecorder {
    /// Opens the device and starts recording into `<dir>/audio/<kind>.wav`; `epoch_ns` is the session start on
    /// [`orbiters_core::clock`]. Fails when the device cannot be opened.
    pub fn start(kind: TrackKind, source: AudioSource, dir: &Path, epoch_ns: u64) -> anyhow::Result<Self> {
        let name = match kind {
            TrackKind::Mic => "mic",
            TrackKind::Game => "game",
        };
        let file = format!("{}/{name}.wav", orbiters_contracts::performance::AUDIO_DIR);
        let path = dir.join(&file);
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let published = Published::default();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = {
            let (stop, paused, level, published, file) = (stop.clone(), paused.clone(), level.clone(), published.clone(), file.clone());
            std::thread::Builder::new().name(format!("audio-{name}")).spawn(move || {
                let (writer, source_name) = match imp::run(&source, &path, epoch_ns, &stop, &paused, &level, &published, &ready_tx) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = ready_tx.try_send(Err(format!("{e:#}")));
                        return Err(e);
                    }
                };
                writer.finish(file, kind, source_name)
            })?
        };
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(source_name)) => Ok(Self { kind, source_name, file, published, stop, paused, level, thread: Some(thread) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                anyhow::bail!("{e}")
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                anyhow::bail!("the audio device did not start")
            }
        }
    }

    /// While paused, silence is written instead of the sound (the timeline stays continuous).
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// Peak level (0–1) since the last call.
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }

    /// The track as its file on disk holds it (its last checkpoint), for `audio.json` while recording; `None`
    /// before the first.
    pub fn track(&self) -> Option<Track> {
        let track = self.published.lock().unwrap().clone()?;
        Some(Track { file: self.file.clone(), kind: self.kind, source: self.source_name.clone(), ..track })
    }

    /// Stops recording and completes the file.
    pub fn finish(mut self) -> anyhow::Result<Track> {
        self.stop.store(true, Ordering::Relaxed);
        match self.thread.take().expect("running").join() {
            Ok(r) => r,
            Err(_) => anyhow::bail!("the audio thread panicked"),
        }
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Microphones for the settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub default: bool,
}

pub use imp::list_mics;

#[cfg(windows)]
mod imp {
    use super::{AudioSource, Device, Published, TrackWriter};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

    /// Records until `stop`; returns the writer to complete and the source's name.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        source: &AudioSource,
        path: &Path,
        epoch_ns: u64,
        stop: &AtomicBool,
        paused: &AtomicBool,
        level: &AtomicU32,
        published: &Published,
        ready: &mpsc::SyncSender<Result<String, String>>,
    ) -> anyhow::Result<(TrackWriter, String)> {
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
                    let rate = (*mix).nSamplesPerSec;
                    CoTaskMemFree(Some(mix as _));
                    // A voice: mono at the device's rate.
                    (client, name, rate, 1u16, false)
                }
                AudioSource::Process(pid) => (process_loopback(*pid)?, format!("VRChat (process {pid})"), 48_000, 2u16, true),
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
            let result = (|| -> anyhow::Result<TrackWriter> {
                client.SetEventHandle(event)?;
                let capture: IAudioCaptureClient = client.GetService()?;
                let mut writer = TrackWriter::create(path, rate, channels, published.clone())?;
                client.Start()?;
                let _ = ready.try_send(Ok(name.clone()));
                let mut samples: Vec<f32> = Vec::new();
                // A device that goes away (headset unplugged, VRChat closed) ends the track; the file stays valid.
                let recorded = (|| -> anyhow::Result<()> {
                    while !stop.load(Ordering::Relaxed) {
                        if WaitForSingleObject(event, 100) != WAIT_OBJECT_0 {
                            continue;
                        }
                        while capture.GetNextPacketSize()? > 0 {
                            let (mut data, mut frames, mut buffer_flags, mut qpc) = (std::ptr::null_mut(), 0u32, 0u32, 0u64);
                            capture.GetBuffer(&mut data, &mut frames, &mut buffer_flags, None, Some(&mut qpc))?;
                            let at = if qpc > 0 { orbiters_core::clock::from_qpc_100ns(qpc) } else { orbiters_core::clock::now_ns() };
                            let t = at.saturating_sub(epoch_ns);
                            let silent = buffer_flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || paused.load(Ordering::Relaxed);
                            let n = frames as usize * channels as usize;
                            if silent || data.is_null() {
                                writer.packet(t, frames, None)?;
                            } else {
                                samples.clear();
                                if tag == WAVE_FORMAT_IEEE_FLOAT {
                                    samples.extend_from_slice(std::slice::from_raw_parts(data.cast::<f32>(), n));
                                } else {
                                    samples.extend(std::slice::from_raw_parts(data.cast::<i16>(), n).iter().map(|v| *v as f32 / 32768.0));
                                }
                                let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                                // Non-negative floats order like their bits.
                                level.fetch_max(peak.to_bits(), Ordering::Relaxed);
                                writer.packet(t, frames, Some(&samples))?;
                            }
                            capture.ReleaseBuffer(frames)?;
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = recorded {
                    eprintln!("{name}: recording ended early: {e:#}");
                }
                let _ = client.Stop();
                Ok(writer)
            })();
            let _ = CloseHandle(event);
            Ok((result?, name))
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{AudioSource, Device, Published, TrackWriter};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicU32};
    use std::sync::mpsc;

    pub fn list_mics() -> Vec<Device> {
        Vec::new()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run(
        _source: &AudioSource,
        _path: &Path,
        _epoch_ns: u64,
        _stop: &AtomicBool,
        _paused: &AtomicBool,
        _level: &AtomicU32,
        _published: &Published,
        _ready: &mpsc::SyncSender<Result<String, String>>,
    ) -> anyhow::Result<(TrackWriter, String)> {
        anyhow::bail!("audio recording is only available on Windows for now")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_land_on_the_session_clock() {
        let dir = std::env::temp_dir().join(format!("orbiters-audio-{}", std::process::id()));
        let path = dir.join("audio/mic.wav");
        let mut w = TrackWriter::create(&path, 1000, 1, Published::default()).unwrap();
        let tone: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        // Continuous 100 ms packets from t = 5 ms (the second one silent): a sync point a second.
        for i in 0..12u64 {
            w.packet(5_000_000 + i * 100_000_000, 100, (i != 1).then_some(tone.as_slice())).unwrap();
        }
        // Then 300 ms without packets (loopback while VRChat is silent): filled with zeros.
        w.packet(1_505_000_000, 100, Some(&tone)).unwrap();
        let track = w.finish("audio/mic.wav".into(), TrackKind::Mic, "Test mic".into()).unwrap();
        assert_eq!(track.first_frame_ns, 5_000_000);
        assert_eq!(track.frames, 1600);
        assert_eq!(track.sync, vec![[0, 5_000_000], [1000, 1_005_000_000], [1500, 1_505_000_000]]);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..58], &audio::wav_header(1000, 1, 1600));
        assert_eq!(bytes.len(), 58 + 1600 * 4);
        let sample = |i: usize| f32::from_le_bytes(bytes[58 + i * 4..62 + i * 4].try_into().unwrap());
        assert_eq!((sample(50), sample(150), sample(250), sample(1300), sample(1550)), (0.5, 0.0, 0.5, 0.0, 0.5));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_track_that_never_finishes_reads_up_to_its_last_checkpoint() {
        let dir = std::env::temp_dir().join(format!("orbiters-audio-unfinished-{}", std::process::id()));
        let path = dir.join("audio/game.wav");
        let published = Published::default();
        let mut w = TrackWriter::create(&path, 1000, 2, published.clone()).unwrap();
        let tone = vec![0.25f32; 200];
        for i in 0..12u64 {
            w.packet(i * 100_000_000, 100, Some(&tone)).unwrap();
        }
        assert!(published.lock().unwrap().is_none(), "no checkpoint yet");
        w.checkpoint().unwrap();
        // Read while recording (or after the worker was killed): the header covers what is on disk.
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..58], &audio::wav_header(1000, 2, 1200));
        assert_eq!(bytes.len(), 58 + 1200 * 2 * 4);
        let track = published.lock().unwrap().clone().unwrap();
        assert_eq!((track.frames, track.sample_rate, track.channels, track.sync.len()), (1200, 1000, 2, 2));
        // More sound after the checkpoint: the file grows past its header, which still reads as the first 1.2 s.
        w.packet(1_200_000_000, 100, Some(&tone)).unwrap();
        w.file.flush().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!((&bytes[..58], bytes.len()), (&audio::wav_header(1000, 2, 1200)[..], 58 + 1300 * 2 * 4));
        // Every couple of seconds on its own.
        w.checkpointed = Instant::now().checked_sub(CHECKPOINT_EVERY).unwrap();
        w.packet(1_300_000_000, 100, Some(&tone)).unwrap();
        assert_eq!(published.lock().unwrap().as_ref().map(|t| t.frames), Some(1400));
        let track = w.finish("audio/game.wav".into(), TrackKind::Game, "VRChat".into()).unwrap();
        assert_eq!((track.frames, track.file.as_str()), (1400, "audio/game.wav"));
        assert_eq!(&std::fs::read(&path).unwrap()[..58], &audio::wav_header(1000, 2, 1400));
        std::fs::remove_dir_all(&dir).ok();
    }
}
