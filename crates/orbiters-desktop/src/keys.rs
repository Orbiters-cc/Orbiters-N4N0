//! The computer's keyboard as N4N0's menu keyboard: while the menu's keyboard page shows (YouTube's search, a state's
//! name, a suggestion) and VRChat is the window in front, what is typed goes to the menu and not to VRChat (a low-level
//! keyboard hook, installed only for that time and removed as soon as the page goes). Characters come as the person's
//! keyboard layout writes them (Shift, AltGr, Caps Lock); Backspace, Enter (Search or Done), Escape (back) and Ctrl+V
//! (the clipboard's text) do what they do in a text field. Shift, Ctrl, Alt and the Windows keys go through untouched.

use orbiters_n4n0_core::menu::KeyInput;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};

/// Where typed keys go (the service hands them to N4N0), and VRChat's process.
static SINK: OnceLock<Mutex<mpsc::Sender<KeyInput>>> = OnceLock::new();
static VRCHAT: AtomicU32 = AtomicU32::new(0);
/// The hook's thread, and whether the hook should be in place.
static THREAD: AtomicU32 = AtomicU32::new(0);
static WANTED: AtomicBool = AtomicBool::new(false);
/// Which key's down and up is whose while the hook is in place (only the hook's thread touches it).
#[cfg_attr(not(windows), allow(dead_code))]
static KEYS: Mutex<Owners> = Mutex::new(Owners::new());

/// Whose each key is (OVERLAY-12): a key already down when the keyboard was taken stays VRChat's until it is let go (its repeats
/// and its up reach VRChat: a W held to walk is not stuck walking), and an up is swallowed only for a down that was swallowed.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Owners {
    /// Keys whose down the menu took.
    taken: [u64; 4],
    /// Keys that were down when the keyboard was taken, or whose down went on (to VRChat, or to another window): not the menu's
    /// until their up.
    held: [u64; 4],
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Owners {
    const fn new() -> Self {
        Self { taken: [0; 4], held: [0; 4] }
    }

    fn bit(set: &[u64; 4], vk: u32) -> bool {
        vk < 256 && set[(vk / 64) as usize] & (1 << (vk % 64)) != 0
    }

    fn set(set: &mut [u64; 4], vk: u32, on: bool) {
        if vk < 256 {
            let mask = 1u64 << (vk % 64);
            if on {
                set[(vk / 64) as usize] |= mask;
            } else {
                set[(vk / 64) as usize] &= !mask;
            }
        }
    }

    /// The keyboard is taken now, with these keys down.
    fn taken_with(down: impl IntoIterator<Item = u32>) -> Self {
        let mut owners = Self::new();
        for vk in down {
            Self::set(&mut owners.held, vk, true);
        }
        owners
    }

    /// Whether the menu takes this key event (`false`: it goes on to VRChat). `menu_key`: a key the menu would take (not a
    /// modifier, not a shortcut of Windows').
    ///
    /// A key's first down decides whose it is, and its repeats and its up follow it (verify-vr: a repeat re-decided, so a
    /// shortcut's key repeating after its modifier was let go became the menu's and its up was swallowed).
    fn take(&mut self, vk: u32, down: bool, menu_key: bool) -> bool {
        if Self::bit(&self.held, vk) {
            if !down {
                Self::set(&mut self.held, vk, false);
            }
            return false;
        }
        if Self::bit(&self.taken, vk) {
            if !down {
                Self::set(&mut self.taken, vk, false);
            }
            return true;
        }
        if down {
            Self::set(if menu_key { &mut self.taken } else { &mut self.held }, vk, true);
            return menu_key;
        }
        false
    }

    /// A key event the menu does not look at (another window in front, a key that always goes on): only who holds it changes. A down
    /// there is not the menu's; an up ends whatever held it (verify-vr: a W let go in another window stayed "held" and the next W
    /// typed in the page walked).
    ///
    /// Returns whether the event is still the menu's to swallow: a key the menu took, held across a change of window, keeps its first
    /// down's routing, its repeats and its up included (verify2: they were typed into the window now in front).
    fn elsewhere(&mut self, vk: u32, down: bool) -> bool {
        if Self::bit(&self.taken, vk) {
            if !down {
                Self::set(&mut self.taken, vk, false);
            }
            return true;
        }
        if down {
            Self::set(&mut self.held, vk, true);
        } else {
            Self::set(&mut self.held, vk, false);
        }
        false
    }
}

#[cfg(windows)]
const WM_APP_HOOK: u32 = 0x8000 + 1;

/// Starts the hook's thread (idle until [`capture`] asks for the keyboard); typed keys go to `sink`.
pub fn start(sink: mpsc::Sender<KeyInput>) {
    if SINK.set(Mutex::new(sink)).is_err() {
        return;
    }
    #[cfg(windows)]
    {
        let _ = std::thread::Builder::new().name("keyboard".into()).spawn(windows_thread);
    }
}

/// Takes the keyboard while `on` (N4N0's keyboard page shows) and VRChat (`vrchat`, its process) is in front.
pub fn capture(on: bool, vrchat: Option<u32>) {
    VRCHAT.store(vrchat.unwrap_or(0), Ordering::Relaxed);
    if WANTED.swap(on, Ordering::AcqRel) == on {
        return;
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
        let thread = THREAD.load(Ordering::Acquire);
        if thread != 0 {
            // SAFETY: a message to the hook's own thread, no pointers in it.
            unsafe {
                let _ = PostThreadMessageW(thread, WM_APP_HOOK, WPARAM(0), LPARAM(0));
            }
        }
    }
}

/// What a key does to a text field, from its virtual key and the keyboard's state (`None`: not one for the menu).
#[cfg(windows)]
fn input_of(vk: u32, scan: u32, ctrl: bool, alt: bool) -> Option<KeyInput> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, GetKeyState, GetKeyboardLayout, ToUnicodeEx, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU,
        VK_RCONTROL, VK_RETURN, VK_RMENU, VK_RSHIFT, VK_SHIFT, VK_SPACE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    match vk {
        v if v == VK_BACK.0 as u32 => return Some(KeyInput::Backspace),
        v if v == VK_RETURN.0 as u32 => return Some(KeyInput::Enter),
        v if v == VK_ESCAPE.0 as u32 => return Some(KeyInput::Escape),
        v if v == VK_SPACE.0 as u32 => return Some(KeyInput::Text(" ".into())),
        // Ctrl+V (not AltGr+V): the clipboard's text.
        0x56 if ctrl && !alt => return Some(KeyInput::Paste),
        _ => {}
    }
    if ctrl && !alt {
        // Other shortcuts type nothing.
        return None;
    }
    // SAFETY: plain reads of the keyboard's state into a local array, and of the foreground window's layout.
    unsafe {
        let mut state = [0u8; 256];
        let down = |k: u16| (GetAsyncKeyState(k as i32) as u16 & 0x8000) != 0;
        for k in [VK_SHIFT, VK_LSHIFT, VK_RSHIFT, VK_CONTROL, VK_LCONTROL, VK_RCONTROL, VK_MENU, VK_LMENU, VK_RMENU] {
            if down(k.0) {
                state[k.0 as usize] = 0x80;
            }
        }
        if GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0 {
            state[VK_CAPITAL.0 as usize] = 0x01;
        }
        let window = GetForegroundWindow();
        let thread = GetWindowThreadProcessId(window, None);
        let layout = GetKeyboardLayout(thread);
        let mut buf = [0u16; 8];
        // Flag 4: the keyboard's dead-key state is left as it is.
        let n = ToUnicodeEx(vk, scan, &state, &mut buf, 4, Some(layout));
        if n <= 0 {
            return None;
        }
        let text: String = String::from_utf16_lossy(&buf[..n as usize]).chars().filter(|c| !c.is_control()).collect();
        (!text.is_empty()).then_some(KeyInput::Text(text))
    }
}

/// Keys that never go to the menu (Shift, Ctrl, Alt, the Windows keys, the locks, the function keys).
#[cfg(windows)]
fn passes(vk: u32) -> bool {
    matches!(vk, 0x10..=0x12 | 0x14 | 0x5B | 0x5C | 0x5D | 0x90 | 0x91 | 0xA0..=0xA5 | 0x70..=0x87 | 0x2C)
}

/// Windows' own shortcuts, which go on while the menu has the keyboard: anything with a Windows key down, Alt+Tab and Alt+Esc
/// (Alt without Ctrl: Ctrl+Alt is AltGr, which types), and Ctrl+Shift+Esc (OVERLAY-12: Alt+Tab reached Windows as Alt alone).
#[cfg(windows)]
fn windows_shortcut(vk: u32, ctrl: bool, alt: bool, shift: bool, windows_key: bool) -> bool {
    const TAB: u32 = 0x09;
    const ESCAPE: u32 = 0x1B;
    windows_key || (alt && !ctrl && matches!(vk, TAB | ESCAPE)) || (vk == ESCAPE && ctrl && shift)
}

#[cfg(windows)]
unsafe extern "system" fn hook(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetForegroundWindow, GetWindowThreadProcessId, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, WM_KEYDOWN, WM_SYSKEYDOWN,
    };
    // SAFETY: Windows hands a valid KBDLLHOOKSTRUCT for HC_ACTION; everything else is passed on as it came.
    unsafe {
        if code == HC_ACTION as i32 && WANTED.load(Ordering::Relaxed) {
            let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
            let injected = info.flags.0 & LLKHF_INJECTED.0 != 0;
            let mut pid = 0u32;
            GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
            let vrchat = VRCHAT.load(Ordering::Relaxed);
            let message = wparam.0 as u32;
            if !injected && !(vrchat != 0 && pid == vrchat && !passes(info.vkCode)) {
                // Not the menu's to look at: only who holds the key is kept.
                let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
                if KEYS.lock().map(|mut keys| keys.elsewhere(info.vkCode, down)).unwrap_or(false) {
                    return windows::Win32::Foundation::LRESULT(1);
                }
            }
            if !injected && vrchat != 0 && pid == vrchat && !passes(info.vkCode) {
                let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
                let key_down = |k: u16| (GetAsyncKeyState(k as i32) as u16 & 0x8000) != 0;
                let (ctrl, alt) = (key_down(VK_CONTROL.0), key_down(VK_MENU.0));
                let shortcut = windows_shortcut(info.vkCode, ctrl, alt, key_down(VK_SHIFT.0), key_down(VK_LWIN.0) || key_down(VK_RWIN.0));
                // A down the menu takes, and that down's up: VRChat sees none of it (a W walks nobody). Any other up, a
                // key held before the menu took the keyboard, and Windows' shortcuts go on.
                let taken = KEYS.lock().map(|mut k| k.take(info.vkCode, down, !shortcut)).unwrap_or(!shortcut);
                if taken {
                    if down {
                        if let Some(input) = input_of(info.vkCode, info.scanCode, ctrl, alt) {
                            if let Some(sink) = SINK.get() {
                                let _ = sink.lock().map(|s| s.send(input));
                            }
                        }
                    }
                    return windows::Win32::Foundation::LRESULT(1);
                }
            }
        }
        CallNextHookEx(Some(HHOOK::default()), code, wparam, lparam)
    }
}

/// The hook's thread: installs the hook while it is wanted, and pumps the messages a low-level hook needs.
#[cfg(windows)]
fn windows_thread() {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, PeekMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, MSG, PM_NOREMOVE, WH_KEYBOARD_LL};
    // SAFETY: the hook is set and removed on this thread, which pumps its messages; the message queue exists once
    // PeekMessage ran, before anyone may post to it.
    unsafe {
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        THREAD.store(GetCurrentThreadId(), Ordering::Release);
        let mut hooked: Option<HHOOK> = None;
        loop {
            let wanted = WANTED.load(Ordering::Acquire);
            match (wanted, hooked) {
                (true, None) => {
                    // The keys down now stay VRChat's until they are let go: read once the hook is in place (no event reaches
                    // it before this thread pumps again), so a key pressed between a reading and the hook is not missed.
                    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
                    match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), None, 0) {
                        Ok(h) => hooked = Some(h),
                        Err(e) => eprintln!("keyboard: {e}"),
                    }
                    let down = (1u32..256).filter(|&vk| (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0);
                    if let Ok(mut keys) = KEYS.lock() {
                        *keys = Owners::taken_with(down);
                    }
                }
                (false, Some(h)) => {
                    let _ = UnhookWindowsHookEx(h);
                    hooked = None;
                }
                _ => {}
            }
            if GetMessageW(&mut msg, None, 0, 0).0 <= 0 {
                break;
            }
        }
        if let Some(h) = hooked {
            let _ = UnhookWindowsHookEx(h);
        }
    }
}

/// OVERLAY-12: whose key events are whose. Pure, on every platform.
#[cfg(test)]
mod owner_tests {
    use super::Owners;

    #[test]
    fn a_key_held_when_the_keyboard_is_taken_stays_vrchats_until_its_up() {
        let w = 0x57;
        let mut keys = Owners::taken_with([w]);
        assert!(!keys.take(w, true, true), "its repeats go on to VRChat");
        assert!(!keys.take(w, false, true), "and its up: the avatar stops walking");
        assert!(keys.take(w, true, true), "pressed again, the menu takes it");
        assert!(keys.take(w, false, true), "and its up");
    }

    /// verify2: a key the menu took, held while another window comes in front, keeps its routing: its repeats and its up are swallowed.
    #[test]
    fn a_menu_key_held_across_a_window_change_stays_the_menus() {
        let a = 0x41;
        let mut keys = Owners::new();
        assert!(keys.take(a, true, true));
        assert!(keys.elsewhere(a, true), "its repeat in the other window is not typed there");
        assert!(keys.elsewhere(a, false), "nor its up");
        assert!(!keys.elsewhere(a, true), "a fresh press there is that window's");
        assert!(!keys.elsewhere(a, false));
        assert_eq!(keys, Owners::new());
    }

    /// verify-vr: a key's repeats follow its first down; a key let go in another window is no longer held for VRChat.
    #[test]
    fn repeats_follow_the_first_down_and_ups_elsewhere_end_a_hold() {
        let tab = 0x09;
        let mut keys = Owners::new();
        assert!(!keys.take(tab, true, false), "Alt+Tab's Tab goes on");
        assert!(!keys.take(tab, true, true), "its repeat after Alt was let go still goes on");
        assert!(!keys.take(tab, false, true), "and so does its up");
        assert_eq!(keys, Owners::new());
        let a = 0x41;
        assert!(keys.take(a, true, true));
        assert!(keys.take(a, true, false), "a repeat of the menu's key stays the menu's");
        assert!(keys.take(a, false, false));
        let w = 0x57;
        let mut keys = Owners::taken_with([w]);
        keys.elsewhere(w, false);
        assert!(keys.take(w, true, true), "W let go in another window: the next W is typed");
        assert!(keys.take(w, false, true));
        keys.elsewhere(w, true);
        assert!(!keys.take(w, true, true), "pressed in another window: its repeats back in VRChat are not typed");
        assert!(!keys.take(w, false, true));
    }

    #[test]
    fn an_up_is_swallowed_only_for_a_down_that_was() {
        let mut keys = Owners::new();
        assert!(!keys.take(0x41, false, true), "an up whose down VRChat had");
        assert!(keys.take(0x41, true, true));
        assert!(keys.take(0x41, false, true));
        assert!(!keys.take(0x41, false, true), "once");
        // A shortcut of Windows' (Alt+Tab): its down goes on, so does its up.
        assert!(!keys.take(0x09, true, false));
        assert!(!keys.take(0x09, false, true));
        assert_eq!(keys, Owners::new());
    }

    #[cfg(windows)]
    #[test]
    fn windows_shortcuts_go_on_and_altgr_still_types() {
        use super::windows_shortcut;
        assert!(windows_shortcut(0x09, false, true, false, false), "Alt+Tab");
        assert!(windows_shortcut(0x1B, true, false, true, false), "Ctrl+Shift+Esc");
        assert!(windows_shortcut(0x44, false, false, false, true), "Windows+D");
        assert!(windows_shortcut(0x1B, false, true, false, false), "Alt+Esc");
        assert!(!windows_shortcut(0x45, true, true, false, false), "AltGr+E types");
        assert!(!windows_shortcut(0x45, false, true, false, false), "Alt+E is the menu's, as before");
        assert!(!windows_shortcut(0x1B, false, false, false, false), "Escape is the menu's");
        assert!(!windows_shortcut(0x56, true, false, false, false), "Ctrl+V is the menu's");
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn modifiers_and_function_keys_pass_through() {
        for vk in [0x10, 0x11, 0x12, 0xA0, 0xA5, 0x5B, 0x14, 0x70, 0x7B] {
            assert!(passes(vk), "{vk:#x}");
        }
        for vk in [0x41, 0x57, 0x20, 0x08, 0x0D, 0x1B, 0x30] {
            assert!(!passes(vk), "{vk:#x}");
        }
    }

    #[test]
    fn editing_keys_are_the_field_s() {
        assert_eq!(input_of(0x08, 0x0E, false, false), Some(KeyInput::Backspace));
        assert_eq!(input_of(0x0D, 0x1C, false, false), Some(KeyInput::Enter));
        assert_eq!(input_of(0x1B, 0x01, false, false), Some(KeyInput::Escape));
        assert_eq!(input_of(0x56, 0x2F, true, false), Some(KeyInput::Paste));
        assert_eq!(input_of(0x43, 0x2E, true, false), None, "Ctrl+C types nothing");
    }
}
