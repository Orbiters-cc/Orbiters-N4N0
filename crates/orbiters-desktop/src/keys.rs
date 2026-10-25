//! The computer's keyboard as N4N0's menu keyboard: while the menu's keyboard page shows (YouTube's search, a state's
//! name, a suggestion) and VRChat is the window in front, what is typed goes to the menu and not to VRChat (a low-level
//! keyboard hook, installed only for that time and removed as soon as the page goes). Characters come as the person's
//! keyboard layout writes them (Shift, AltGr, Caps Lock); Backspace, Enter (Search or Done), Escape (back) and Ctrl+V
//! (the clipboard's text) do what they do in a text field. Shift, Ctrl, Alt and the Windows keys go through untouched.

use orbiters_orb::menu::KeyInput;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};

/// Where typed keys go (the service hands them to N4N0), and VRChat's process.
static SINK: OnceLock<Mutex<mpsc::Sender<KeyInput>>> = OnceLock::new();
static VRCHAT: AtomicU32 = AtomicU32::new(0);
/// The hook's thread, and whether the hook should be in place.
static THREAD: AtomicU32 = AtomicU32::new(0);
static WANTED: AtomicBool = AtomicBool::new(false);

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

#[cfg(windows)]
unsafe extern "system" fn hook(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU};
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
            if !injected && vrchat != 0 && pid == vrchat && !passes(info.vkCode) {
                let message = wparam.0 as u32;
                if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
                    let ctrl = (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000) != 0;
                    let alt = (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0;
                    if let Some(input) = input_of(info.vkCode, info.scanCode, ctrl, alt) {
                        if let Some(sink) = SINK.get() {
                            let _ = sink.lock().map(|s| s.send(input));
                        }
                    }
                }
                // Down and up alike: VRChat sees none of it (a W walks nobody).
                return windows::Win32::Foundation::LRESULT(1);
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
                (true, None) => match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), None, 0) {
                    Ok(h) => hooked = Some(h),
                    Err(e) => eprintln!("keyboard: {e}"),
                },
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
