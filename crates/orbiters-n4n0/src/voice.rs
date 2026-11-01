//! N4N0's ears: the voice worker (protocol `orbiters_core::voice`, started through the platform's [`Ears`]) runs while
//! VRChat does and N4N0's assistant is on with its models installed, so a sentence is transcribed without waiting for
//! models to load. It opens the microphone only while N4N0 listens, or all along when the wake word is on, and keeps
//! only its small VAD loaded between sentences. On the desktop it is a process below normal priority: VRChat comes
//! first.
//!
//! [`Ears`]: crate::Ears

use crate::platform::VoiceWorker;
use crate::runtime::{N4n0, Shared, Source};
use crate::status::{NoticeKind, N4n0Who, VoiceStatus};
use orbiters_core::voice::{self, VoiceCommand, VoiceEvent};
use std::time::{Duration, Instant};

/// The app gives up on a sentence after this (the worker ends its own sooner).
pub const LISTEN_LIMIT: Duration = Duration::from_secs(35);
/// A worker that ended on its own is not started again before this, doubled each time it does so again.
const RESTART_AFTER: Duration = Duration::from_secs(30);
/// After this many ends in a row, the worker is started again only when someone presses Talk.
const CRASHES_MOST: u32 = 3;
/// A worker that ran this long before it ended starts the count of ends over.
const CRASH_WINDOW: Duration = Duration::from_secs(10 * 60);
/// A worker started for a sentence while VRChat is closed (the window's Talk button) stays this long after it, so
/// the next press does not load the models again.
const LISTEN_LEASE: Duration = Duration::from_secs(120);
/// Cancelled listens remembered, so that a sentence answering one is dropped.
const CANCELLED_KEPT: usize = 32;
/// Windows' "the program could not find a library it needs" (STATUS_DLL_NOT_FOUND).
const DLL_NOT_FOUND: u32 = 0xC000_0135;
/// What the worker says before it loads the recogniser: a worker that stops with this as its last line stopped while
/// loading the models.
const LOADING_LINE: &str = "loading the speech recogniser";

/// A running worker, and what it was started with.
struct Worker {
    inner: Box<dyn VoiceWorker>,
    /// Its number: its end is told by it.
    serial: u64,
    /// The microphone and wake words it was started with (or switched to since).
    mic: String,
    wake_word: bool,
    wake_words: Vec<String>,
    started: Instant,
}

#[derive(Default)]
pub(crate) struct VoiceRuntime {
    handle: Option<Worker>,
    installer: Option<Worker>,
    /// The last worker's number.
    workers: u64,
    ended_at: Option<Instant>,
    /// Workers that ended on their own in a row (each start after one waits longer), and how many of them while
    /// loading the models.
    crashes: u32,
    load_crashes: u32,
    /// The models load no more (the worker stopped twice while loading them): the next install repairs them.
    repair: bool,
    /// The last listen's number, and whether it still waits for its sentence.
    next_listen: u64,
    current: Option<u64>,
    /// Listens cancelled before their sentence came (newest last).
    cancelled: std::collections::VecDeque<u64>,
    /// Keeps the worker while a sentence asked for with VRChat closed is under way.
    lease_until: Option<Instant>,
}

impl VoiceRuntime {
    /// Whether a sentence answering listen `id` should be dropped (that listen was cancelled), forgetting it.
    fn cancelled_take(&mut self, id: u64) -> bool {
        match self.cancelled.iter().position(|&c| c == id) {
            Some(i) => {
                self.cancelled.remove(i);
                true
            }
            None => false,
        }
    }

    /// How long to wait before starting a worker again after `crashes` ends in a row: 30 s, then twice as long each
    /// time, up to 8 minutes.
    fn restart_delay(&self) -> Duration {
        RESTART_AFTER.saturating_mul(1 << self.crashes.saturating_sub(1).min(4))
    }

    /// A worker that ran for `ran` ended on its own (exit `code`, its last line `last`): counted, and what the person is
    /// told. True in the answer when the models seem damaged.
    fn ended(&mut self, ran: Duration, code: Option<i32>, last: Option<&str>) -> (String, bool) {
        self.ended_at = Some(Instant::now());
        self.crashes = if ran > CRASH_WINDOW { 1 } else { self.crashes + 1 };
        if last.is_some_and(|l| l.contains(LOADING_LINE)) {
            self.load_crashes += 1;
        } else {
            self.load_crashes = 0;
        }
        let damaged = self.load_crashes >= 2;
        self.repair |= damaged;
        let message = if code.map(|c| c as u32) == Some(DLL_NOT_FOUND) {
            "N4N0's ears could not start: a library they need is missing next to Orbiters - N4N0 (onnxruntime.dll or sherpa-onnx-c-api.dll). Reinstall Orbiters - N4N0.".to_string()
        } else if damaged {
            "N4N0's ears stopped twice while loading the speech models: they may be damaged. Install them again on the N4N0 page.".to_string()
        } else {
            let code = code.map(|c| format!(" (exit code {:#x})", c as u32)).unwrap_or_default();
            let after = if self.crashes >= CRASHES_MOST { " They stay off until you press Talk." } else { "" };
            match last {
                Some(line) => format!("N4N0's ears stopped{code}: {line}.{after}"),
                None => format!("N4N0's ears stopped{code}.{after}"),
            }
        };
        (message, damaged)
    }
}

impl N4n0 {
    /// Whether the speech models are on this device.
    pub(crate) fn models_installed(&self) -> bool {
        voice::installed(&self.files.models)
    }

    /// Starts a worker through the platform, numbered: what it says comes to [`N4n0::on_voice_event`], its end to
    /// [`N4n0::on_voice_exit`].
    fn voice_spawn(self: &Shared, rt: &mut VoiceRuntime) -> anyhow::Result<Worker> {
        rt.workers += 1;
        let serial = rt.workers;
        let events = {
            let n = self.clone();
            Box::new(move |e: VoiceEvent| n.on_voice_event(e))
        };
        let ended = {
            let n = self.clone();
            Box::new(move || n.on_voice_exit(serial))
        };
        let inner = self.device.ears.spawn(events, ended)?;
        Ok(Worker { inner, serial, mic: String::new(), wake_word: false, wake_words: Vec::new(), started: Instant::now() })
    }

    /// Starts the voice worker when N4N0 should be able to hear (VRChat runs, or a sentence was asked for meanwhile;
    /// the assistant is on, the models are here), stops it otherwise, and passes on a changed microphone or wake
    /// word (without loading anything again). Called once a second.
    pub(crate) fn voice_follow(self: &Shared) {
        let settings = self.with_settings(|s| s.n4n0.clone());
        let vrchat = self.host.scene().vrchat;
        let installed = self.n4n0.lock().unwrap().voice.installed;
        let mut rt = self.voice.lock().unwrap();
        let leased = rt.lease_until.is_some_and(|t| Instant::now() < t);
        let wanted = settings.enabled && settings.assistant && installed && (vrchat || leased);
        if !wanted {
            if let Some(handle) = rt.handle.take() {
                drop(rt);
                // Its end waits a moment for the worker: not under the lock.
                drop(handle);
                self.voice_status(|v| v.running = false);
            }
            return;
        }
        if let Some(handle) = rt.handle.as_mut() {
            if handle.mic != settings.mic_device {
                handle.mic = settings.mic_device.clone();
                let mic = (!handle.mic.is_empty()).then(|| handle.mic.clone());
                let _ = handle.inner.send(&VoiceCommand::Mic { mic });
            }
            if handle.wake_word != settings.wake_word || handle.wake_words != settings.wake_words {
                handle.wake_word = settings.wake_word;
                handle.wake_words = settings.wake_words.clone();
                let _ = handle.inner.send(&VoiceCommand::WakeWord { on: settings.wake_word, words: settings.wake_words.clone() });
            }
            return;
        }
        // A worker that keeps stopping on its own is started less and less often, then only for a press of Talk.
        if rt.crashes >= CRASHES_MOST && !leased {
            return;
        }
        if rt.ended_at.is_some_and(|t| t.elapsed() < rt.restart_delay()) {
            return;
        }
        self.voice_start(&mut rt, &settings.mic_device, settings.wake_word, &settings.wake_words);
    }

    fn voice_start(self: &Shared, rt: &mut VoiceRuntime, mic: &str, wake_word: bool, wake_words: &[String]) {
        match self.voice_spawn(rt) {
            Ok(mut handle) => {
                handle.mic = mic.to_string();
                handle.wake_word = wake_word;
                handle.wake_words = wake_words.to_vec();
                let start = VoiceCommand::Start {
                    models: self.files.models.clone(),
                    mic: (!mic.is_empty()).then(|| mic.to_string()),
                    wake_word,
                    wake_words: wake_words.to_vec(),
                };
                if let Err(e) = handle.inner.send(&start) {
                    eprintln!("voice: {e:#}");
                }
                rt.handle = Some(handle);
            }
            Err(e) => {
                rt.ended_at = Some(Instant::now());
                let message = format!("{e:#}");
                self.voice_status(move |v| v.error = Some(message));
            }
        }
    }

    /// N4N0 listens for a sentence from `source` (the worker starts if it was not running, as for the window's Talk
    /// button with VRChat closed, and stays a while after).
    pub(crate) fn voice_listen(self: &Shared, source: Source) {
        let settings = self.with_settings(|s| s.n4n0.clone());
        let mut rt = self.voice.lock().unwrap();
        rt.lease_until = Some(Instant::now() + LISTEN_LIMIT + LISTEN_LEASE);
        if rt.handle.is_none() {
            rt.ended_at = None;
            self.voice_start(&mut rt, &settings.mic_device, settings.wake_word, &settings.wake_words);
        }
        let hold = matches!(source, Source::Menu | Source::Window | Source::Overlay);
        rt.next_listen += 1;
        let id = rt.next_listen;
        rt.current = Some(id);
        if let Some(handle) = rt.handle.as_mut() {
            if let Err(e) = handle.inner.send(&VoiceCommand::Listen { hold, id }) {
                eprintln!("voice: {e:#}");
            }
        }
    }

    /// The held sentence ended (the Talk button was let go): the worker transcribes it.
    pub(crate) fn voice_finish(self: &Shared) {
        if let Some(handle) = self.voice.lock().unwrap().handle.as_mut() {
            let _ = handle.inner.send(&VoiceCommand::Finish);
        }
    }

    /// The sentence being listened to is dropped: nothing the worker heard for it goes anywhere.
    pub(crate) fn voice_cancel(self: &Shared) {
        let mut rt = self.voice.lock().unwrap();
        let Some(id) = rt.current.take() else { return };
        rt.cancelled.push_back(id);
        while rt.cancelled.len() > CANCELLED_KEPT {
            rt.cancelled.pop_front();
        }
        if let Some(handle) = rt.handle.as_mut() {
            let _ = handle.inner.send(&VoiceCommand::Cancel);
        }
    }

    /// Downloads the speech models in a worker of their own, its progress on the N4N0 page: what is missing or cut
    /// short, or all of it when they stopped loading (a repair).
    pub(crate) fn voice_install(self: &Shared) {
        let mut rt = self.voice.lock().unwrap();
        let repair = rt.repair;
        if rt.installer.is_some() || (self.models_installed() && !repair) {
            return;
        }
        match self.voice_spawn(&mut rt) {
            Ok(mut handle) => {
                let _ = handle.inner.send(&VoiceCommand::Install { models: self.files.models.clone(), repair });
                rt.installer = Some(handle);
                drop(rt);
                self.voice_status(|v| {
                    v.downloading = Some(0.0);
                    v.error = None;
                });
            }
            Err(e) => {
                drop(rt);
                self.notify(NoticeKind::Error, format!("Could not start the download: {e:#}"));
            }
        }
    }

    /// Stops the speech models' download (the N4N0 page's Cancel): the installing worker removes what it unpacked and
    /// leaves; its end puts the Install button back.
    pub(crate) fn voice_cancel_install(&self) {
        if let Some(installer) = self.voice.lock().unwrap().installer.as_mut() {
            if let Err(e) = installer.inner.send(&VoiceCommand::Cancel) {
                eprintln!("voice install: {e:#}");
            }
        }
    }

    /// Stops the voice workers (the app quits).
    pub(crate) fn voice_stop(&self) {
        let (handle, installer) = {
            let mut rt = self.voice.lock().unwrap();
            (rt.handle.take(), rt.installer.take())
        };
        // Each waits a moment for its worker: not under the lock.
        drop(handle);
        drop(installer);
    }

    fn voice_status(&self, f: impl FnOnce(&mut VoiceStatus)) {
        f(&mut self.n4n0.lock().unwrap().voice);
        self.publish();
    }

    /// What a voice worker said.
    pub(crate) fn on_voice_event(self: &Shared, event: VoiceEvent) {
        match event {
            VoiceEvent::Ready { engine, load_ms } => {
                eprintln!("voice: {engine} ready in {load_ms} ms");
                self.voice_status(|v| {
                    v.running = true;
                    v.error = None;
                });
            }
            VoiceEvent::Mic { .. } | VoiceEvent::Speech { .. } => {}
            VoiceEvent::Level { level } => {
                // Coarse steps: the meter moves, and an unchanged level is not published again.
                let level = (level.clamp(0.0, 1.0) * 20.0).round() / 20.0;
                let changed = {
                    let mut n4n0 = self.n4n0.lock().unwrap();
                    let changed = n4n0.voice.level != level;
                    n4n0.voice.level = level;
                    changed
                };
                if changed {
                    self.publish();
                }
            }
            VoiceEvent::Heard { text, for_n4n0, seconds, decode_ms, listen } => {
                if for_n4n0 {
                    eprintln!("voice: {seconds:.1} s transcribed in {decode_ms} ms");
                }
                // A sentence for a cancelled listen goes nowhere; one for the latest listen ends N4N0's listening.
                let (dropped, current) = {
                    let mut rt = self.voice.lock().unwrap();
                    let dropped = for_n4n0 && listen != 0 && rt.cancelled_take(listen);
                    let current = for_n4n0 && (listen == 0 || rt.current == Some(listen));
                    if current {
                        rt.current = None;
                    }
                    // It heard: it works.
                    rt.crashes = 0;
                    (dropped, current)
                };
                self.voice_status(|v| v.level = 0.0);
                if !dropped {
                    self.heard(text, for_n4n0, current);
                }
            }
            VoiceEvent::Installing { fraction } => self.voice_status(|v| v.downloading = Some(fraction)),
            VoiceEvent::Installed => {
                {
                    let mut rt = self.voice.lock().unwrap();
                    rt.repair = false;
                    rt.crashes = 0;
                    rt.load_crashes = 0;
                    rt.ended_at = None;
                }
                self.voice_status(|v| {
                    v.downloading = None;
                    v.installed = true;
                    v.error = None;
                    v.repair = false;
                });
                self.notify(NoticeKind::Success, "N4N0 can hear now.");
            }
            VoiceEvent::Error { message } => {
                self.n4n0.lock().unwrap().line(N4n0Who::Error, message.clone());
                self.voice_status(|v| v.error = Some(message));
            }
        }
    }

    /// A voice worker ended (number `serial`): the installer's job is over, or the listening one stopped on its own
    /// (crashed). One that was replaced or stopped on purpose ends without a word.
    fn on_voice_exit(self: &Shared, serial: u64) {
        let (handle, installer) = {
            let mut rt = self.voice.lock().unwrap();
            if rt.installer.as_ref().is_some_and(|h| h.serial == serial) {
                (None, rt.installer.take())
            } else if rt.handle.as_ref().is_some_and(|h| h.serial == serial) {
                (rt.handle.take(), None)
            } else {
                (None, None)
            }
        };
        if let Some(installer) = installer {
            drop(installer);
            let installed = self.models_installed();
            self.voice_status(|v| {
                v.downloading = None;
                v.installed = installed;
            });
            return;
        }
        let Some(mut handle) = handle else { return };
        let code = handle.inner.exit_code();
        let last = handle.inner.last_line();
        let (message, damaged) = {
            let mut rt = self.voice.lock().unwrap();
            // Its listen will never be answered.
            rt.current = None;
            rt.ended(handle.started.elapsed(), code, last.as_deref())
        };
        drop(handle);
        eprintln!("voice: {message}");
        {
            let mut n4n0 = self.n4n0.lock().unwrap();
            n4n0.listening = None;
            n4n0.line(N4n0Who::Error, message.clone());
        }
        self.voice_status(|v| {
            v.running = false;
            v.level = 0.0;
            v.error = Some(message);
            if damaged {
                // The Install button shows again: it repairs them.
                v.installed = false;
                v.repair = true;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_that_keeps_stopping_is_started_less_often_then_not() {
        let mut rt = VoiceRuntime::default();
        let soon = Duration::from_secs(5);
        let (message, damaged) = rt.ended(soon, Some(3), Some("something went wrong"));
        assert!(!damaged);
        assert_eq!(message, "N4N0's ears stopped (exit code 0x3): something went wrong.");
        assert_eq!(rt.restart_delay(), RESTART_AFTER);
        rt.ended(soon, Some(3), None);
        assert_eq!(rt.restart_delay(), RESTART_AFTER * 2);
        let (message, _) = rt.ended(soon, Some(3), None);
        assert!(message.ends_with("They stay off until you press Talk."), "{message}");
        assert_eq!(rt.crashes, CRASHES_MOST);
        // One that ran a long while starts the count over.
        rt.ended(CRASH_WINDOW + soon, None, None);
        assert_eq!(rt.crashes, 1);
    }

    #[test]
    fn stopping_while_loading_twice_asks_for_a_repair() {
        let mut rt = VoiceRuntime::default();
        let soon = Duration::from_secs(5);
        assert!(!rt.ended(soon, Some(0xC000_0409_u32 as i32), Some(LOADING_LINE)).1);
        let (message, damaged) = rt.ended(soon, Some(0xC000_0409_u32 as i32), Some(LOADING_LINE));
        assert!(damaged && rt.repair);
        assert!(message.contains("Install them again"), "{message}");
        // A missing library is said as such (reinstalling the models would not help).
        let mut rt = VoiceRuntime::default();
        let (message, damaged) = rt.ended(soon, Some(DLL_NOT_FOUND as i32), None);
        assert!(!damaged);
        assert!(message.contains("onnxruntime.dll"), "{message}");
    }
}
