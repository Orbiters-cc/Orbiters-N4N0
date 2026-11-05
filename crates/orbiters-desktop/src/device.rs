//! This computer, as N4N0 sees it (`orbiters_n4n0::Platform`): Windows' clipboard, the default browser, the press sound
//! through WASAPI, the person's language and keyboard, the mouse for VRChat's desktop mode, the music Windows' media
//! controls tell, and N4N0's ears in a worker process of their own (`orbiters-voice`, below normal priority).

use orbiters_core::voice::{VoiceCommand, VoiceEvent};
use orbiters_n4n0::{Browser, Clipboard, Ears, Locale, Mouse, MouseState, Platform, Sounds, VoiceEnded, VoiceEvents, VoiceWorker};
use orbiters_n4n0_core::menu::KeyboardLayout;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// N4N0's platform on this computer.
pub fn platform() -> Platform {
    Platform {
        clipboard: Box::new(WindowsClipboard),
        browser: Box::new(DefaultBrowser),
        sounds: Box::new(PressSound),
        locale: Box::new(WindowsLocale),
        ears: Box::new(VoiceProcess),
        media: crate::media::watcher().clone(),
        mouse: Some(Box::new(WindowsMouse)),
    }
}

/// This computer's clipboard. Tests never touch it: what they copy is kept for them to read.
pub struct WindowsClipboard;

impl Clipboard for WindowsClipboard {
    #[cfg(not(test))]
    fn copy(&self, text: &str) -> Result<(), String> {
        arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())).map_err(|e| format!("Could not copy: {e}"))
    }

    #[cfg(test)]
    fn copy(&self, text: &str) -> Result<(), String> {
        COPIED.lock().unwrap().push(text.to_string());
        Ok(())
    }

    #[cfg(not(test))]
    fn paste(&self) -> Option<String> {
        arboard::Clipboard::new().and_then(|mut c| c.get_text()).ok().filter(|t| !t.is_empty())
    }
}

/// What the tests copied.
#[cfg(test)]
pub static COPIED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The person's default browser.
pub struct DefaultBrowser;

impl Browser for DefaultBrowser {
    fn open(&self, url: &str) -> anyhow::Result<()> {
        crate::controller::open_url(url)
    }
}

/// The overlay's press sound, computed the first time, as a copy's feedback in VR.
pub struct PressSound;

impl Sounds for PressSound {
    fn press(&self) {
        if cfg!(test) {
            return;
        }
        static SOUND: std::sync::OnceLock<(orbiters_sound::Output, Arc<orbiters_sound::Sound>)> = std::sync::OnceLock::new();
        let _ = std::thread::Builder::new().name("copy-sound".into()).spawn(|| {
            let (output, sound) = SOUND.get_or_init(|| {
                use orbiters_sound::{render, sounds, Output, RATE};
                (Output::new(), Arc::new(render(&sounds::ROCKER, RATE)))
            });
            output.play(sound);
        });
    }
}

/// Windows' language and keyboard for the person.
pub struct WindowsLocale;

impl Locale for WindowsLocale {
    /// This computer's language (`fr-FR`): Windows' user locale, else `LANG`.
    fn locale(&self) -> String {
        #[cfg(windows)]
        {
            use windows::Win32::Globalization::GetUserDefaultLocaleName;
            let mut buf = [0u16; 85];
            // SAFETY: the buffer is LOCALE_NAME_MAX_LENGTH wide, as the call requires.
            let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
            if n > 1 {
                return String::from_utf16_lossy(&buf[..n as usize - 1]);
            }
        }
        std::env::var("LANG").ok().and_then(|l| l.split('.').next().map(|s| s.replace('_', "-"))).unwrap_or_default()
    }

    /// AZERTY for French and Belgian French keyboards (Swiss and Canadian French keep QWERTZ/QWERTY), QWERTY otherwise.
    fn keyboard(&self) -> KeyboardLayout {
        #[cfg(windows)]
        {
            // SAFETY: no arguments that could be invalid; 0 is the calling thread.
            let layout = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout(0) };
            let language = (layout.0 as usize & 0xffff) as u16;
            if language & 0x3ff == 0x0c && matches!(language >> 10, 0x01 | 0x02) {
                return KeyboardLayout::Azerty;
            }
        }
        KeyboardLayout::Qwerty
    }
}

/// This computer's mouse.
pub struct WindowsMouse;

impl Mouse for WindowsMouse {
    fn read(&mut self, vrchat: u32) -> MouseState {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::{POINT, RECT};
            use windows::Win32::Graphics::Gdi::ClientToScreen;
            use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
            use windows::Win32::UI::WindowsAndMessaging::{
                GetClientRect, GetCursorInfo, GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, CURSORINFO, CURSOR_SHOWING, SM_SWAPBUTTON,
            };
            // SAFETY: plain reads of the input state; the process id, the cursor's state and the window's client area are
            // written to locals that outlive the calls.
            unsafe {
                // The left button as the owner set it up (swapped: the physical right one).
                let button = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
                let down = (GetAsyncKeyState(button.0 as i32) as u16) & 0x8000 != 0;
                let window = GetForegroundWindow();
                let mut pid = 0u32;
                let front = !window.is_invalid() && GetWindowThreadProcessId(window, Some(&mut pid as *mut u32)) != 0 && pid == vrchat;
                // In mouse-look Unity hides the cursor by giving it no picture (Windows still flags it as showing) and
                // holds it at the middle of the window: either says the cursor is not there to click with.
                let cursor = front && {
                    let mut info = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
                    let shown = GetCursorInfo(&mut info).is_ok() && info.flags.0 & CURSOR_SHOWING.0 != 0 && !info.hCursor.is_invalid();
                    let mut client = RECT::default();
                    let mut corner = POINT::default();
                    let held = GetClientRect(window, &mut client).is_ok() && ClientToScreen(window, &mut corner).as_bool() && {
                        let (cx, cy) = (corner.x + (client.right - client.left) / 2, corner.y + (client.bottom - client.top) / 2);
                        (info.ptScreenPos.x - cx).abs() <= 2 && (info.ptScreenPos.y - cy).abs() <= 2
                    };
                    shown && !held
                };
                MouseState { down, front, cursor }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = vrchat;
            MouseState::default()
        }
    }
}

/// N4N0's ears as a worker process: `orbiters-voice` next to the app, speaking the voice protocol in JSON lines, below
/// normal priority (transcribing never takes VRChat's frames), ended with the service.
pub struct VoiceProcess;

impl Ears for VoiceProcess {
    fn spawn(&self, events: VoiceEvents, ended: VoiceEnded) -> anyhow::Result<Box<dyn VoiceWorker>> {
        let path = crate::worker::sibling_exe("orbiters-voice");
        anyhow::ensure!(path.is_file(), "N4N0's voice worker was not found at {}", path.display());
        let mut cmd = Command::new(&path);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW, BELOW_NORMAL_PRIORITY_CLASS.
            cmd.creation_flags(0x0800_0000 | 0x0000_4000);
        }
        let mut child = cmd.spawn()?;
        crate::worker::keep_with_service(&child);
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        std::thread::Builder::new().name("voice-stdout".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<VoiceEvent>(&line) {
                    Ok(e) => events(e),
                    // Not the line: it may hold what someone said.
                    Err(e) => eprintln!("voice: unexpected output ({} bytes): {e}", line.len()),
                }
            }
            ended();
        })?;
        let last_line = Arc::new(Mutex::new(None));
        let last = last_line.clone();
        std::thread::Builder::new().name("voice-stderr".into()).spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                let line = line.strip_prefix("voice: ").unwrap_or(&line).to_string();
                eprintln!("voice: {line}");
                *last.lock().unwrap() = Some(line);
            }
        })?;
        Ok(Box::new(VoiceChild { child, stdin, last_line }))
    }
}

/// A running `orbiters-voice`.
struct VoiceChild {
    child: Child,
    stdin: ChildStdin,
    /// The last line it wrote on stderr: what it was doing when it stopped.
    last_line: Arc<Mutex<Option<String>>>,
}

impl VoiceWorker for VoiceChild {
    fn send(&mut self, command: &VoiceCommand) -> anyhow::Result<()> {
        writeln!(self.stdin, "{}", command.to_line())?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Its exit code, waiting up to a second for it (its output ends a moment before it does).
    fn exit_code(&mut self) -> Option<i32> {
        for _ in 0..20 {
            if let Ok(Some(status)) = self.child.try_wait() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    fn last_line(&self) -> Option<String> {
        self.last_line.lock().unwrap().clone()
    }
}

impl Drop for VoiceChild {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_computer_s_language_and_a_missing_worker() {
        assert_eq!(WindowsLocale.language().len(), 2);
        // No `orbiters-voice` beside the test program: said so, and nothing starts.
        let ended = Box::new(|| {});
        let err = VoiceProcess.spawn(Box::new(|_| {}), ended).err().expect("no worker beside the tests");
        assert!(err.to_string().contains("voice worker was not found"), "{err}");
        WindowsClipboard.copy("kept for the test").unwrap();
        assert!(COPIED.lock().unwrap().iter().any(|t| t == "kept for the test"));
    }
}
