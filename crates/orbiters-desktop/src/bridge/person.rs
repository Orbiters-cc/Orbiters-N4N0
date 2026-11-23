//! The person at the computer always wins. Any real keyboard or mouse input pauses the agent (it goes on by itself once
//! the person has left it alone for [`RESUME_AFTER`]); the hotkey pauses it until it is resumed; Stop in Settings ends it.
//!
//! Real input is told from ours through Raw Input, which hands the bridge a copy of what the devices send, whichever window
//! has the focus: what `SendInput` made comes with no device. This reads: unlike a low-level hook it adds no delay to the
//! person's typing, and cannot swallow anything. `GetLastInputInfo` is the second opinion, for what Raw Input misses
//! (Windows keeps input for an elevated foreground window from it): input of any kind that is newer than the last of ours
//! counts as the person's.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the person must leave the computer alone before the agent goes on.
pub const RESUME_AFTER: Duration = Duration::from_secs(30);
/// The hotkey, as the person reads it.
pub const HOTKEY: &str = "Ctrl+Alt+F12";

/// Why the agent is paused now.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Paused {
    /// The person's own input: over `resumes_in_s` after the last of it.
    Person { resumes_in_s: u64 },
    /// The hotkey or the window's Pause: over when resumed.
    Held,
}

/// When the person last touched anything, and whether the agent was told to stop until resumed.
#[derive(Debug, Default)]
pub struct Presence {
    last_input: Option<Instant>,
    held: bool,
}

impl Presence {
    pub fn person_input(&mut self, now: Instant) {
        self.last_input = Some(now);
    }

    pub fn set_held(&mut self, held: bool) {
        self.held = held;
    }

    pub fn held(&self) -> bool {
        self.held
    }

    /// Back to running: the person's last input is forgotten too (they just said go on).
    pub fn resume(&mut self) {
        self.held = false;
        self.last_input = None;
    }

    pub fn paused(&self, now: Instant) -> Option<Paused> {
        if self.held {
            return Some(Paused::Held);
        }
        let last = self.last_input?;
        let idle = now.duration_since(last);
        (idle < RESUME_AFTER).then(|| Paused::Person { resumes_in_s: (RESUME_AFTER - idle).as_secs() + 1 })
    }
}

/// When the bridge's own input last went out (milliseconds of Windows' tick count), for telling it from the person's.
#[derive(Default)]
pub struct OwnInput(AtomicU32);

impl OwnInput {
    pub fn sent(&self, tick_ms: u32) {
        self.0.store(tick_ms, Ordering::Relaxed);
    }

    /// When ours last went out, for tests.
    #[cfg(test)]
    pub fn last(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    /// Whether Windows' last input (at `last_input_ms`, its tick count) is someone else's: newer than the last of ours by
    /// more than the moment ours takes to count, the tick count wrapping around every 49 days.
    pub fn someone_else(&self, last_input_ms: u32, seen_before_ms: u32) -> bool {
        const MARGIN_MS: i32 = 150;
        let newer_than_seen = last_input_ms.wrapping_sub(seen_before_ms) as i32 > 0;
        newer_than_seen && last_input_ms.wrapping_sub(self.0.load(Ordering::Relaxed)) as i32 > MARGIN_MS
    }
}

/// What the listening thread tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heard {
    PersonInput,
    Hotkey,
}

/// A running listener; dropped, it stops listening.
pub struct Listener {
    #[cfg(windows)]
    thread: u32,
    /// Set when it is dropped: the Mac's listening thread ends.
    #[cfg(target_os = "macos")]
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// The hotkey was taken by another program.
    pub hotkey_taken: bool,
}

impl Listener {
    /// A listener that hears nothing, for the lab, where nobody is at a fake game.
    pub fn none() -> Self {
        Self {
            #[cfg(windows)]
            thread: 0,
            #[cfg(target_os = "macos")]
            stop: Arc::default(),
            hotkey_taken: false,
        }
    }
}

#[cfg(windows)]
impl Drop for Listener {
    fn drop(&mut self) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        if self.thread == 0 {
            return;
        }
        // SAFETY: a plain call: the thread's message loop ends on it; a thread gone already only makes it fail.
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

/// Starts hearing the person on a thread of its own. `None` when Raw Input could not be registered: the caller fails
/// closed then, as an agent that cannot hear the person may not run.
#[cfg(windows)]
pub fn listen(own: Arc<OwnInput>, heard: impl Fn(Heard) + Send + 'static) -> Option<Listener> {
    use std::sync::mpsc;
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::Builder::new().name("bridge-person".into()).spawn(move || imp::run(own, heard, ready_tx)).ok()?;
    ready_rx.recv_timeout(Duration::from_secs(5)).ok().flatten().map(|(thread, hotkey_taken)| Listener { thread, hotkey_taken })
}

/// A Mac: the HID system's time since its last keyboard, mouse or trackpad event, looked at ten times a second
/// (`CGEventSourceSecondsSinceLastEventType`, which needs no permission and sees every app's input). The bridge sends no
/// input on a Mac (caps.rs), so every event is the person's. No hotkey: Settings' Pause and Stop hold it.
#[cfg(target_os = "macos")]
pub fn listen(_own: Arc<OwnInput>, heard: impl Fn(Heard) + Send + 'static) -> Option<Listener> {
    let stop: Arc<std::sync::atomic::AtomicBool> = Arc::default();
    let ending = stop.clone();
    std::thread::Builder::new()
        .name("bridge-person".into())
        .spawn(move || {
            let begun = Instant::now();
            let mut clock = IdleClock::default();
            while !ending.load(Ordering::Relaxed) {
                if clock.sample(begun.elapsed().as_secs_f64(), super::mac::idle_seconds()) {
                    heard(Heard::PersonInput);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
        .ok()?;
    Some(Listener { stop, hotkey_taken: false })
}

#[cfg(target_os = "macos")]
impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The person's last input, on this listener's own monotonic clock, from the HID system's idle time sampled again and
/// again: each sample places the last event at `now - idle`. An event later than the last one known is new input, so a
/// mouse that never stops moving (every sample near 0 s idle) is heard at every sample, not once. The first sample counts
/// an event within [`RESUME_AFTER`]: a listener started while the person is at the computer hears them at once.
#[derive(Debug, Default)]
pub struct IdleClock {
    last_event: Option<f64>,
}

impl IdleClock {
    /// How far apart two readings of one event may land (the idle time's own granularity).
    const JITTER: f64 = 0.02;

    /// One sample at `now` (seconds on a monotonic clock) of the idle time `idle` (seconds): whether it says input came.
    pub fn sample(&mut self, now: f64, idle: f64) -> bool {
        if !idle.is_finite() || idle < 0.0 {
            return false;
        }
        let event = now - idle;
        let new = match self.last_event {
            None => idle < RESUME_AFTER.as_secs_f64(),
            Some(last) => event > last + Self::JITTER,
        };
        self.last_event = Some(self.last_event.map_or(event, |last| last.max(event)));
        new
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn listen(_own: Arc<OwnInput>, _heard: impl Fn(Heard) + Send + 'static) -> Option<Listener> {
    None
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::sync::mpsc::Sender;
    use windows::core::w;
    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetLastInputInfo, RegisterHotKey, UnregisterHotKey, LASTINPUTINFO, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, VK_F12,
    };
    use windows::Win32::UI::Input::{GetRawInputData, RegisterRawInputDevices, HRAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RID_HEADER};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, RegisterClassW, HWND_MESSAGE, MSG,
        MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS, PM_REMOVE, QS_ALLINPUT, WINDOW_EX_STYLE, WINDOW_STYLE, WM_HOTKEY, WM_INPUT, WM_QUIT, WNDCLASSW,
    };

    const HOTKEY_ID: i32 = 0x4E34;

    fn tick() -> u32 {
        // SAFETY: a plain query.
        unsafe { GetTickCount() }
    }

    /// Windows' last input of any kind.
    fn last_input() -> Option<u32> {
        let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        // SAFETY: the structure and its size are passed.
        unsafe { GetLastInputInfo(&mut info) }.as_bool().then_some(info.dwTime)
    }

    /// Whether a `WM_INPUT` came from a device: `SendInput`'s, the bridge's own among them, come without one.
    fn from_a_device(lparam: LPARAM) -> bool {
        let mut header = RAWINPUTHEADER::default();
        let mut size = std::mem::size_of::<RAWINPUTHEADER>() as u32;
        // SAFETY: the buffer is one header, its size passed; asking for RID_HEADER writes no more.
        let read = unsafe {
            GetRawInputData(
                HRAWINPUT(lparam.0 as *mut _),
                RID_HEADER,
                Some(&mut header as *mut RAWINPUTHEADER as *mut _),
                &mut size,
                std::mem::size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        read != u32::MAX && !header.hDevice.0.is_null()
    }

    /// `WM_INPUT` must go through `DefWindowProc` for Windows to clean up after it.
    unsafe extern "system" fn procedure(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // SAFETY: passing on what Windows passed.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    pub(super) fn run(own: Arc<OwnInput>, heard: impl Fn(Heard), ready: Sender<Option<(u32, bool)>>) {
        // SAFETY: registers a class and makes a message-only window on this thread: no window to show, nothing else
        // reads it; the devices' registration is this process's and ends with the window.
        unsafe {
            let class = w!("OrbitersBridgePerson");
            let instance = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(procedure), hInstance: instance, lpszClassName: class, ..Default::default() });
            let window = match CreateWindowExW(WINDOW_EX_STYLE(0), class, class, WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(instance), None) {
                Ok(window) => window,
                Err(_) => {
                    let _ = ready.send(None);
                    return;
                }
            };
            // Generic desktop: mouse (2) and keyboard (6), whichever window has the focus.
            let devices = [
                RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: RIDEV_INPUTSINK, hwndTarget: window },
                RAWINPUTDEVICE { usUsagePage: 1, usUsage: 6, dwFlags: RIDEV_INPUTSINK, hwndTarget: window },
            ];
            if RegisterRawInputDevices(&devices, std::mem::size_of::<RAWINPUTDEVICE>() as u32).is_err() {
                let _ = DestroyWindow(window);
                let _ = ready.send(None);
                return;
            }
            let hotkey = RegisterHotKey(Some(window), HOTKEY_ID, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_F12.0 as u32).is_ok();
            let _ = ready.send(Some((GetCurrentThreadId(), !hotkey)));
            let mut seen = last_input().unwrap_or_else(tick);
            'running: loop {
                // Wakes at least ten times a second for the second opinion.
                let _ = MsgWaitForMultipleObjectsEx(Some(&[] as &[HANDLE]), 100, QS_ALLINPUT, MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(0));
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    match msg.message {
                        WM_QUIT => break 'running,
                        WM_INPUT if from_a_device(msg.lParam) => heard(Heard::PersonInput),
                        WM_HOTKEY if msg.wParam.0 as i32 == HOTKEY_ID => heard(Heard::Hotkey),
                        _ => {}
                    }
                    if msg.message == WM_INPUT {
                        DispatchMessageW(&msg);
                    }
                }
                if let Some(last) = last_input() {
                    if own.someone_else(last, seen) {
                        heard(Heard::PersonInput);
                    }
                    seen = last;
                }
            }
            if hotkey {
                let _ = UnregisterHotKey(Some(window), HOTKEY_ID);
            }
            let _ = DestroyWindow(window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_input_pauses_until_the_person_has_left_it_alone() {
        let start = Instant::now();
        let mut presence = Presence::default();
        assert_eq!(presence.paused(start), None, "nobody touched anything");
        presence.person_input(start);
        assert_eq!(presence.paused(start), Some(Paused::Person { resumes_in_s: RESUME_AFTER.as_secs() + 1 }));
        // More input starts the wait again.
        presence.person_input(start + Duration::from_secs(10));
        assert!(presence.paused(start + RESUME_AFTER).is_some());
        assert_eq!(presence.paused(start + Duration::from_secs(10) + RESUME_AFTER), None);
    }

    #[test]
    fn the_hotkey_holds_until_resumed_whatever_time_passes() {
        let start = Instant::now();
        let mut presence = Presence::default();
        presence.set_held(true);
        assert_eq!(presence.paused(start + Duration::from_secs(36_000)), Some(Paused::Held));
        presence.person_input(start);
        presence.resume();
        assert_eq!(presence.paused(start), None, "going on forgives the input that came with it");
    }

    #[test]
    fn input_newer_than_our_own_is_someone_elses() {
        let own = OwnInput::default();
        own.sent(1_000);
        assert!(!own.someone_else(1_100, 900), "ours, a moment after it went out");
        assert!(own.someone_else(1_200, 1_100), "after ours by more than the moment");
        assert!(!own.someone_else(1_200, 1_200), "nothing new since the last look");
        // Across the tick count's wrap.
        own.sent(u32::MAX - 50);
        assert!(!own.someone_else(u32::MAX.wrapping_add(20), u32::MAX - 60));
        assert!(own.someone_else(200, u32::MAX - 60));
    }

    #[test]
    fn an_event_later_than_the_last_one_is_new_input() {
        let mut clock = IdleClock::default();
        assert!(!clock.sample(0.0, 600.0), "nobody for ten minutes");
        assert!(!clock.sample(0.1, 600.1), "nobody: the idle time goes on");
        assert!(clock.sample(0.2, 0.05), "someone touched the trackpad");
        assert!(!clock.sample(0.3, 0.14), "the same event read again, jitter included");
        assert!(!clock.sample(0.4, f64::NAN));
    }

    /// The verifier's case: a mouse moving without a break gives an idle time near 0 at every sample. The person stays
    /// heard for as long as it moves, well past the pause's 30 s, and the pause ends 30 s after it stops.
    #[test]
    fn continuous_input_longer_than_the_pause_keeps_the_agent_paused() {
        let start = Instant::now();
        let mut clock = IdleClock::default();
        let mut presence = Presence::default();
        let mut paused_every_sample = true;
        for step in 0..450 {
            let at = step as f64 * 0.1;
            // Idle between 0 and 8 ms: always moving.
            if clock.sample(at, (step % 9) as f64 * 0.001) {
                presence.person_input(start + Duration::from_secs_f64(at));
            }
            paused_every_sample &= presence.paused(start + Duration::from_secs_f64(at)).is_some();
        }
        assert!(paused_every_sample, "45 s of movement: paused all along");
        let stopped = start + Duration::from_secs(45);
        assert!(presence.paused(stopped + Duration::from_secs(29)).is_some());
        assert_eq!(presence.paused(stopped + Duration::from_secs(31)), None);
    }

    #[test]
    fn a_listener_started_while_the_person_moves_hears_them_at_once() {
        let mut clock = IdleClock::default();
        assert!(clock.sample(0.0, 0.004), "the first reading is presence");
        let mut later = IdleClock::default();
        assert!(later.sample(0.0, 12.0), "an event 12 s ago is within the pause: still the person's time");
        let mut away = IdleClock::default();
        assert!(!away.sample(0.0, 31.0), "31 s ago: they had left");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_mac_listener_starts_and_stops() {
        let listener = listen(Arc::new(OwnInput::default()), |_| {}).expect("it always starts on a Mac");
        assert!(!listener.hotkey_taken);
        drop(listener);
    }

    #[cfg(windows)]
    #[test]
    fn the_listener_starts_and_stops_with_nothing_left_behind() {
        let listener = listen(Arc::new(OwnInput::default()), |_| {}).expect("raw input registers");
        drop(listener);
        // Again: the class is there, the devices and the hotkey went with the last one.
        std::thread::sleep(Duration::from_millis(300));
        let again = listen(Arc::new(OwnInput::default()), |_| {}).expect("raw input registers again");
        drop(again);
    }
}
