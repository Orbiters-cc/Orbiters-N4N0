//! Orb's ears in the tray service: the voice worker (`orbiters-voice`, protocol `orbiters_core::voice`) runs while
//! VRChat does and Orb's assistant is on with its models installed, so a sentence is transcribed without waiting for
//! models to load. It opens the microphone only while Orb listens, or all along when the wake word is on.

use super::orb::Source;
use super::{Controller, NoticeKind, Shared};
use crate::state::OrbWho;
use orbiters_core::voice::{self, VoiceCommand, VoiceEvent};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

/// The service gives up on a sentence after this (the worker ends its own sooner).
pub const LISTEN_LIMIT: Duration = Duration::from_secs(35);
/// A worker that ended on its own is not started again before this.
const RESTART_AFTER: Duration = Duration::from_secs(30);

pub fn models_installed() -> bool {
    voice::installed(&orbiters_core::paths::models_dir())
}

struct VoiceHandle {
    child: Child,
    stdin: ChildStdin,
    /// The microphone and wake word it was started with.
    mic: String,
    wake_word: bool,
}

impl VoiceHandle {
    fn spawn(ctl: Shared) -> anyhow::Result<Self> {
        let path = crate::worker::sibling_exe("orbiters-voice");
        anyhow::ensure!(path.is_file(), "Orb's voice worker was not found at {}", path.display());
        let mut cmd = Command::new(&path);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let pid = child.id();
        std::thread::Builder::new().name("voice-stdout".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<VoiceEvent>(&line) {
                    Ok(e) => ctl.on_voice_event(e),
                    Err(_) => eprintln!("voice: unexpected output: {line}"),
                }
            }
            ctl.on_voice_exit(pid);
        })?;
        std::thread::Builder::new().name("voice-stderr".into()).spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                eprintln!("voice: {line}");
            }
        })?;
        Ok(Self { child, stdin, mic: String::new(), wake_word: false })
    }

    fn send(&mut self, command: &VoiceCommand) -> anyhow::Result<()> {
        writeln!(self.stdin, "{}", command.to_line())?;
        self.stdin.flush()?;
        Ok(())
    }
}

impl Drop for VoiceHandle {
    fn drop(&mut self) {
        let _ = self.send(&VoiceCommand::Quit);
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
    }
}

#[derive(Default)]
pub struct VoiceRuntime {
    handle: Option<VoiceHandle>,
    installer: Option<VoiceHandle>,
    ended_at: Option<Instant>,
}

impl Controller {
    /// Starts the voice worker when Orb should be able to hear (VRChat runs, the assistant is on, the models are
    /// here), stops it otherwise, and passes on a changed microphone or wake word. Called by the orb thread.
    pub(super) fn voice_follow(self: &Shared) {
        let settings = self.settings().orb;
        let vrchat = self.snapshot.lock().unwrap().vrchat.running;
        let installed = self.orb.lock().unwrap().voice.installed;
        let wanted = settings.enabled && settings.assistant && installed && vrchat;
        let mut rt = self.voice.lock().unwrap();
        if !wanted {
            if rt.handle.take().is_some() {
                drop(rt);
                self.voice_status(|v| v.running = false);
            }
            return;
        }
        if let Some(handle) = rt.handle.as_mut() {
            if handle.mic != settings.mic_device {
                // Another microphone: start over with it.
                rt.handle = None;
            } else if handle.wake_word != settings.wake_word {
                handle.wake_word = settings.wake_word;
                let _ = handle.send(&VoiceCommand::WakeWord { on: settings.wake_word });
                return;
            } else {
                return;
            }
        }
        if rt.ended_at.is_some_and(|t| t.elapsed() < RESTART_AFTER) {
            return;
        }
        self.voice_start(&mut rt, &settings.mic_device, settings.wake_word);
    }

    fn voice_start(self: &Shared, rt: &mut VoiceRuntime, mic: &str, wake_word: bool) {
        match VoiceHandle::spawn(self.clone()) {
            Ok(mut handle) => {
                handle.mic = mic.to_string();
                handle.wake_word = wake_word;
                let start = VoiceCommand::Start { models: orbiters_core::paths::models_dir(), mic: (!mic.is_empty()).then(|| mic.to_string()), wake_word };
                if let Err(e) = handle.send(&start) {
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

    /// Orb listens for a sentence from `source` (the worker starts if it was not running, as for the window's Talk
    /// button with VRChat closed).
    pub(super) fn voice_listen(self: &Shared, source: Source) {
        let settings = self.settings().orb;
        let mut rt = self.voice.lock().unwrap();
        if rt.handle.is_none() {
            rt.ended_at = None;
            self.voice_start(&mut rt, &settings.mic_device, settings.wake_word);
        }
        let hold = matches!(source, Source::Menu | Source::App);
        if let Some(handle) = rt.handle.as_mut() {
            if let Err(e) = handle.send(&VoiceCommand::Listen { hold }) {
                eprintln!("voice: {e:#}");
            }
        }
    }

    /// The held sentence ended (the Talk button was let go).
    pub(super) fn voice_finish(self: &Shared) {
        if let Some(handle) = self.voice.lock().unwrap().handle.as_mut() {
            let _ = handle.send(&VoiceCommand::Finish);
        }
    }

    /// Downloads the speech models in a worker of their own, its progress on the Orb page.
    pub(super) fn voice_install(self: &Shared) {
        let mut rt = self.voice.lock().unwrap();
        if rt.installer.is_some() || models_installed() {
            return;
        }
        match VoiceHandle::spawn(self.clone()) {
            Ok(mut handle) => {
                let _ = handle.send(&VoiceCommand::Install { models: orbiters_core::paths::models_dir() });
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

    /// Stops the voice worker (the app quits).
    pub(super) fn voice_stop(&self) {
        let mut rt = self.voice.lock().unwrap();
        rt.handle = None;
        rt.installer = None;
    }

    fn voice_status(&self, f: impl FnOnce(&mut crate::state::VoiceStatus)) {
        f(&mut self.orb.lock().unwrap().voice);
        self.orb_publish();
    }

    fn on_voice_event(self: &Shared, event: VoiceEvent) {
        match event {
            VoiceEvent::Ready { engine, load_ms } => {
                eprintln!("voice: {engine} ready in {load_ms} ms");
                self.voice_status(|v| {
                    v.running = true;
                    v.error = None;
                });
            }
            VoiceEvent::Mic { .. } | VoiceEvent::Speech { .. } => {}
            VoiceEvent::Level { level } => self.voice_status(|v| v.level = level),
            VoiceEvent::Heard { text, for_orb, seconds, decode_ms } => {
                if for_orb {
                    eprintln!("voice: {seconds:.1} s transcribed in {decode_ms} ms");
                }
                self.voice_status(|v| v.level = 0.0);
                self.orb_heard(text, !for_orb);
            }
            VoiceEvent::Installing { fraction } => self.voice_status(|v| v.downloading = Some(fraction)),
            VoiceEvent::Installed => {
                self.voice_status(|v| {
                    v.downloading = None;
                    v.installed = true;
                });
                self.notify(NoticeKind::Success, "Orb can hear now.");
            }
            VoiceEvent::Error { message } => {
                self.orb.lock().unwrap().line(OrbWho::Error, message.clone());
                self.voice_status(|v| v.error = Some(message));
            }
        }
    }

    /// A voice worker ended: the installer's job is over, or the listening one stopped (crashed, or was asked to).
    fn on_voice_exit(self: &Shared, pid: u32) {
        let mut rt = self.voice.lock().unwrap();
        if rt.installer.as_ref().is_some_and(|h| h.child.id() == pid) {
            rt.installer = None;
            drop(rt);
            let installed = models_installed();
            self.voice_status(|v| {
                v.downloading = None;
                v.installed = installed;
            });
            return;
        }
        if rt.handle.as_ref().is_some_and(|h| h.child.id() == pid) {
            rt.handle = None;
            rt.ended_at = Some(Instant::now());
        }
        drop(rt);
        self.orb.lock().unwrap().listening = None;
        self.voice_status(|v| {
            v.running = false;
            v.level = 0.0;
        });
    }
}
