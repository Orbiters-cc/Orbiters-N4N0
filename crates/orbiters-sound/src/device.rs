//! Sound devices (WASAPI shared mode on Windows): [`Output`] plays sounds on the default output, [`MicMonitor`]
//! sends a microphone straight to it (the settings' mic test). Both open their devices when needed, so a default
//! device that changes (SteamVR switching to the headset) is followed.

use crate::synth::Sound;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};

/// The rate everything is rendered and played at (Windows converts to the device's own).
pub const RATE: u32 = 48_000;

enum Command {
    Play(Arc<Sound>),
}

/// Plays sounds, mixed, on the default output device. The device is opened when a sound starts and let go a second
/// after the last one ends. Clones share the player.
#[derive(Clone)]
pub struct Output {
    tx: mpsc::Sender<Command>,
}

impl Output {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new().name("sound-output".into()).spawn(move || imp::output(rx));
        Self { tx }
    }

    pub fn play(&self, sound: &Arc<Sound>) {
        let _ = self.tx.send(Command::Play(sound.clone()));
    }
}

impl Default for Output {
    fn default() -> Self {
        Self::new()
    }
}

/// Plays a microphone on the default output while it lives, and measures its level.
pub struct MicMonitor {
    stop: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MicMonitor {
    /// `device`: a microphone id from the capture worker's list, `None` for Windows' default.
    pub fn start(device: Option<String>) -> anyhow::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = {
            let (stop, level) = (stop.clone(), level.clone());
            std::thread::Builder::new().name("mic-monitor".into()).spawn(move || {
                if let Err(e) = imp::monitor(device.as_deref(), &stop, &level, &ready_tx) {
                    let _ = ready_tx.try_send(Err(format!("{e:#}")));
                }
            })?
        };
        match ready_rx.recv_timeout(std::time::Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self { stop, level, thread: Some(thread) }),
            Ok(Err(e)) => anyhow::bail!("{e}"),
            Err(_) => anyhow::bail!("the microphone did not start"),
        }
    }

    /// The loudest sample since the last call (0..1).
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }
}

impl Drop for MicMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Mixes the playing sounds into `out` (interleaved stereo) and drops the finished ones.
fn mix(voices: &mut Vec<(Arc<Sound>, usize)>, out: &mut [f32]) {
    out.fill(0.0);
    for (sound, pos) in voices.iter_mut() {
        for (frame, src) in out.as_chunks_mut::<2>().0.iter_mut().zip(sound.frames[(*pos).min(sound.frames.len())..].iter()) {
            frame[0] += src[0];
            frame[1] += src[1];
        }
        *pos += out.len() / 2;
    }
    voices.retain(|(s, pos)| *pos < s.frames.len());
    out.iter_mut().for_each(|v| *v = v.clamp(-1.0, 1.0));
}

#[cfg(windows)]
mod imp {
    use super::{mix, Command, RATE};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{mpsc, Arc};
    use std::time::{Duration, Instant};
    use windows::core::HSTRING;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;

    fn format(channels: u16) -> WAVEFORMATEX {
        let block = channels * 4;
        WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: channels,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * block as u32,
            nBlockAlign: block,
            wBitsPerSample: 32,
            cbSize: 0,
        }
    }

    /// An event-driven shared-mode client of 32-bit float at 48 kHz (`buffer` in 100 ns units).
    struct Stream {
        client: IAudioClient,
        event: HANDLE,
    }

    impl Stream {
        unsafe fn open(device: &IMMDevice, channels: u16, buffer: i64) -> anyhow::Result<Self> {
            unsafe {
                let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
                let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, buffer, 0, &format(channels), None)?;
                let event = CreateEventW(None, false, false, None)?;
                let stream = Self { client, event };
                stream.client.SetEventHandle(event)?;
                Ok(stream)
            }
        }

        fn wait(&self, ms: u32) -> bool {
            unsafe { WaitForSingleObject(self.event, ms) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe {
                let _ = self.client.Stop();
                let _ = CloseHandle(self.event);
            }
        }
    }

    unsafe fn enumerator() -> anyhow::Result<IMMDeviceEnumerator> {
        unsafe { Ok(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?) }
    }

    /// Writes as much as the device takes: `fill` gets interleaved stereo to fill.
    unsafe fn feed(stream: &Stream, render: &IAudioRenderClient, size: u32, fill: &mut dyn FnMut(&mut [f32])) -> anyhow::Result<()> {
        unsafe {
            let free = size - stream.client.GetCurrentPadding()?;
            if free == 0 {
                return Ok(());
            }
            let data = render.GetBuffer(free)?;
            fill(std::slice::from_raw_parts_mut(data.cast::<f32>(), free as usize * 2));
            render.ReleaseBuffer(free, 0)?;
            Ok(())
        }
    }

    pub fn output(rx: mpsc::Receiver<Command>) {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let mut voices: Vec<(Arc<super::Sound>, usize)> = Vec::new();
        // Idle: wait for a sound, then play until a second after the last one ended.
        while let Ok(Command::Play(sound)) = rx.recv() {
            voices.push((sound, 0));
            let played = unsafe {
                (|| -> anyhow::Result<()> {
                    let device = enumerator()?.GetDefaultAudioEndpoint(eRender, eConsole)?;
                    // 30 ms of buffer: clicks follow the press closely.
                    let stream = Stream::open(&device, 2, 300_000)?;
                    let render: IAudioRenderClient = stream.client.GetService()?;
                    let size = stream.client.GetBufferSize()?;
                    feed(&stream, &render, size, &mut |out| mix(&mut voices, out))?;
                    stream.client.Start()?;
                    let mut quiet_since = None::<Instant>;
                    loop {
                        stream.wait(100);
                        while let Ok(Command::Play(s)) = rx.try_recv() {
                            voices.push((s, 0));
                        }
                        feed(&stream, &render, size, &mut |out| mix(&mut voices, out))?;
                        if voices.is_empty() {
                            let since = *quiet_since.get_or_insert_with(Instant::now);
                            if since.elapsed() > Duration::from_secs(1) {
                                return Ok(());
                            }
                        } else {
                            quiet_since = None;
                        }
                    }
                })()
            };
            if let Err(e) = played {
                eprintln!("sound output: {e:#}");
                voices.clear();
            }
        }
    }

    pub fn monitor(device: Option<&str>, stop: &AtomicBool, level: &AtomicU32, ready: &mpsc::SyncSender<Result<(), String>>) -> anyhow::Result<()> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            let devices = enumerator()?;
            let mic = match device {
                Some(id) => devices.GetDevice(&HSTRING::from(id))?,
                None => devices.GetDefaultAudioEndpoint(eCapture, eConsole)?,
            };
            let input = Stream::open(&mic, 1, 200_000)?;
            let capture: IAudioCaptureClient = input.client.GetService()?;
            let speaker = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let output = Stream::open(&speaker, 2, 300_000)?;
            let render: IAudioRenderClient = output.client.GetService()?;
            let size = output.client.GetBufferSize()?;
            let mut queue: VecDeque<f32> = VecDeque::new();
            // About 20 ms gathered before the output starts; more than 80 ms waiting is dropped (latency stays low).
            let (start_at, keep) = (RATE as usize / 50, RATE as usize * 8 / 100);
            let mut started = false;
            input.client.Start()?;
            let _ = ready.try_send(Ok(()));
            while !stop.load(Ordering::Relaxed) {
                if !input.wait(100) {
                    continue;
                }
                while capture.GetNextPacketSize()? > 0 {
                    let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
                    capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
                    if silent {
                        queue.extend(std::iter::repeat_n(0.0, frames as usize));
                    } else {
                        let samples = std::slice::from_raw_parts(data.cast::<f32>(), frames as usize);
                        let peak = samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
                        // Non-negative floats order like their bits.
                        level.fetch_max(peak.min(1.0).to_bits(), Ordering::Relaxed);
                        queue.extend(samples);
                    }
                    capture.ReleaseBuffer(frames)?;
                }
                while queue.len() > keep {
                    queue.pop_front();
                }
                if !started && queue.len() >= start_at {
                    started = true;
                    feed(&output, &render, size, &mut |out| take(&mut queue, out))?;
                    output.client.Start()?;
                } else if started {
                    feed(&output, &render, size, &mut |out| take(&mut queue, out))?;
                }
            }
            Ok(())
        }
    }

    /// Mono queue to interleaved stereo; silence when it runs dry.
    fn take(queue: &mut VecDeque<f32>, out: &mut [f32]) {
        for frame in out.as_chunks_mut::<2>().0 {
            let v = queue.pop_front().unwrap_or(0.0);
            frame[0] = v;
            frame[1] = v;
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Command;
    use std::sync::atomic::{AtomicBool, AtomicU32};
    use std::sync::mpsc;

    pub fn output(rx: mpsc::Receiver<Command>) {
        while rx.recv().is_ok() {}
    }

    pub fn monitor(_device: Option<&str>, _stop: &AtomicBool, _level: &AtomicU32, _ready: &mpsc::SyncSender<Result<(), String>>) -> anyhow::Result<()> {
        anyhow::bail!("the mic test needs Windows for now")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voices_mix_and_finish() {
        let a = Arc::new(Sound { rate: RATE, frames: vec![[0.25, -0.25]; 3] });
        let b = Arc::new(Sound { rate: RATE, frames: vec![[0.5, 0.5]; 5] });
        let mut voices = vec![(a, 0), (b, 0)];
        let mut out = [0f32; 8];
        mix(&mut voices, &mut out);
        assert_eq!(out, [0.75, 0.25, 0.75, 0.25, 0.75, 0.25, 0.5, 0.5]);
        assert_eq!(voices.len(), 1);
        mix(&mut voices, &mut out);
        assert_eq!(out[..2], [0.5, 0.5]);
        assert_eq!(out[2..], [0.0; 6]);
        assert!(voices.is_empty());
    }
}
