//! The Windows calls behind the bridge: finding the game's window, whether it has the foreground, sending it ordinary
//! input, closing it, starting VRChat, and hearing the person (`Listener`). Nothing here touches the game's process: no
//! handle to it is opened, nothing is read from or written to its memory, nothing is loaded into it. `SendInput` is the
//! same call an on-screen keyboard makes, and goes to whatever window has the foreground, which is why it is checked first.

use super::game::{Failure, GameWindow};
use super::input::{Input, MouseButton};

pub use imp::*;

/// `SendInput`'s scale for absolute positions: 0 to 65535 across the virtual desktop.
pub fn absolute(pixel: i32, origin: i32, size: i32) -> i32 {
    if size <= 1 {
        return 0;
    }
    // To the middle of the pixel: it does not round into the one before.
    (((pixel - origin) as i64 * 65536 + 32768) / size as i64).clamp(0, 65535) as i32
}

/// The button flags of `SendInput`'s mouse events, on Windows' numbering.
pub fn button_flags(button: MouseButton, down: bool) -> u32 {
    match (button, down) {
        (MouseButton::Left, true) => 0x0002,
        (MouseButton::Left, false) => 0x0004,
        (MouseButton::Right, true) => 0x0008,
        (MouseButton::Right, false) => 0x0010,
        (MouseButton::Middle, true) => 0x0020,
        (MouseButton::Middle, false) => 0x0040,
    }
}

/// `SendInput`'s keyboard flags: by scan code (what games read), an arrow key's extended prefix, and going up.
pub fn key_flags(extended: bool, down: bool) -> u32 {
    let mut flags = 0x0008; // KEYEVENTF_SCANCODE
    if extended {
        flags |= 0x0001;
    }
    if !down {
        flags |= 0x0002;
    }
    flags
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn window_of(_pid: u32, _title: Option<&str>) -> Option<GameWindow> {
        None
    }
    pub fn is_foreground(_handle: isize) -> bool {
        false
    }
    pub fn bring_forward(_handle: isize) {}
    pub fn resize_client(_handle: isize, _width: u32, _height: u32) {}
    pub fn close_window(_handle: isize) -> Result<(), Failure> {
        Err(Failure::new("not_windows", "The bridge is for Windows.", "Run it on Windows.")) // i18n: not shown (an AI client reads it in the tool's answer)
    }
    pub fn inject(_window: &GameWindow, _input: &[Input]) -> Result<(), Failure> {
        Err(Failure::new("not_windows", "The bridge is for Windows.", "Run it on Windows.")) // i18n: not shown (an AI client reads it in the tool's answer)
    }
    pub fn release(_input: &[Input]) {}
    pub fn start_vrchat(_args: &[String]) -> Result<(), Failure> {
        Err(Failure::new("not_windows", "The bridge is for Windows.", "Run it on Windows.")) // i18n: not shown (an AI client reads it in the tool's answer)
    }
    pub fn start_vrchat_beside(_args: &[String]) -> Result<(), Failure> {
        Err(Failure::new("not_windows", "The bridge is for Windows.", "Run it on Windows.")) // i18n: not shown (an AI client reads it in the tool's answer)
    }
    pub fn udp_owners() -> std::collections::HashMap<u16, u32> {
        std::collections::HashMap::new()
    }
    pub fn foreground_pid() -> Option<u32> {
        None
    }
    pub fn service_delete_pending(_service: &str) -> bool {
        false
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::core::{BOOL, HSTRING};
    use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK,
        MOUSEINPUT, MOUSE_EVENT_FLAGS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClientRect, GetForegroundWindow, GetSystemMetrics, GetWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
        IsWindowVisible, IsZoomed, PostMessageW, SetForegroundWindow, SetWindowPos, ShowWindow, SwitchToThisWindow, GW_OWNER, SM_CXVIRTUALSCREEN,
        SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOZORDER, SW_RESTORE,
        SW_SHOWNOACTIVATE, WM_CLOSE,
    };

    fn hwnd(handle: isize) -> HWND {
        HWND(handle as *mut _)
    }

    /// The top-level, visible, unowned, titled window of the process `pid` (whose title holds `title`, when given), its
    /// largest when there are several: Unity's window, not a splash or a helper's.
    pub fn window_of(pid: u32, title: Option<&str>) -> Option<GameWindow> {
        struct Find<'a> {
            pid: u32,
            title: Option<&'a str>,
            best: Option<GameWindow>,
        }
        unsafe extern "system" fn each(h: HWND, data: LPARAM) -> BOOL {
            // SAFETY: `data` is the `Find` below, alive and only used on this thread for the call to `EnumWindows`.
            let find = unsafe { &mut *(data.0 as *mut Find<'_>) };
            let mut pid = 0;
            // SAFETY: `h` was handed over by `EnumWindows`; a window gone meanwhile only makes these calls fail.
            unsafe {
                GetWindowThreadProcessId(h, Some(&mut pid));
                if pid != find.pid || !IsWindowVisible(h).as_bool() || GetWindow(h, GW_OWNER).is_ok_and(|o| !o.0.is_null()) {
                    return BOOL(1);
                }
                let mut text = [0u16; 256];
                let n = GetWindowTextW(h, &mut text);
                if n <= 0 || find.title.is_some_and(|t| !String::from_utf16_lossy(&text[..n as usize]).contains(t)) {
                    return BOOL(1);
                }
                let mut rect = RECT::default();
                if GetClientRect(h, &mut rect).is_err() {
                    return BOOL(1);
                }
                let (width, height) = ((rect.right - rect.left).max(0) as u32, (rect.bottom - rect.top).max(0) as u32);
                if find.best.is_none_or(|b| b.width * b.height < width * height) {
                    find.best = Some(GameWindow { handle: h.0 as isize, width, height });
                }
            }
            BOOL(1)
        }
        let mut find = Find { pid, title, best: None };
        // SAFETY: the callback reads `find` through the pointer for the duration of this call only.
        unsafe {
            let _ = EnumWindows(Some(each), LPARAM(&mut find as *mut Find<'_> as isize));
        }
        find.best
    }

    pub fn is_foreground(handle: isize) -> bool {
        // SAFETY: a plain query.
        unsafe { GetForegroundWindow() == hwnd(handle) }
    }

    /// Windows may well say no to `SetForegroundWindow` (a background process does not get to take the foreground); then
    /// what Alt+Tab calls. Nobody is at the computer when this is called: their input would have paused the agent.
    pub fn bring_forward(handle: isize) {
        // SAFETY: plain calls; a window gone meanwhile only makes them fail.
        unsafe {
            let h = hwnd(handle);
            if IsIconic(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            let _ = SetForegroundWindow(h);
            if GetForegroundWindow() != h {
                SwitchToThisWindow(h, true);
            }
        }
    }

    /// Gives the window a client area of `width` by `height` where it is, never activating it, never changing what is in
    /// front: what the person sees in front stays in front, and the foreground stays theirs. A maximized window is restored
    /// first (without activation); a minimized one is left alone.
    pub fn resize_client(handle: isize, width: u32, height: u32) {
        // SAFETY: plain calls on a window handle; a window gone meanwhile only makes them fail.
        unsafe {
            let h = hwnd(handle);
            if IsIconic(h).as_bool() {
                return;
            }
            if IsZoomed(h).as_bool() {
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            }
            let (mut outer, mut inner) = (RECT::default(), RECT::default());
            if GetWindowRect(h, &mut outer).is_err() || GetClientRect(h, &mut inner).is_err() {
                return;
            }
            // The frame stays as it is: the new outer size is the client asked for plus what the frame takes now.
            let frame_w = (outer.right - outer.left) - (inner.right - inner.left);
            let frame_h = (outer.bottom - outer.top) - (inner.bottom - inner.top);
            let _ = SetWindowPos(
                h,
                None,
                0,
                0,
                width as i32 + frame_w,
                height as i32 + frame_h,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
        }
    }

    pub fn close_window(handle: isize) -> Result<(), Failure> {
        // SAFETY: posting a message; a window gone meanwhile only makes this fail.
        unsafe { PostMessageW(Some(hwnd(handle)), WM_CLOSE, WPARAM(0), LPARAM(0)) }.map_err(|e| {
            Failure::new("close_failed", format!("Windows did not take the close request: {e}"), "Call get_state: the game may have gone already.") // i18n: not shown
        })
    }

    fn key_input(scan: u16, extended: bool, down: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wScan: scan, dwFlags: KEYBD_EVENT_FLAGS(key_flags(extended, down)), ..Default::default() } },
        }
    }

    fn mouse_input(dx: i32, dy: i32, flags: u32) -> INPUT {
        INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, dwFlags: MOUSE_EVENT_FLAGS(flags), ..Default::default() } } }
    }

    /// `input` as `SendInput` takes it, positions in the window's pixels on the screen as it is now.
    fn events(window: &GameWindow, input: &[Input]) -> Vec<INPUT> {
        let mut origin = POINT::default();
        // SAFETY: plain queries; a failure leaves the origin at the screen's corner, and the foreground check comes after.
        let (vx, vy, vw, vh) = unsafe {
            let _ = ClientToScreen(hwnd(window.handle), &mut origin);
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        input
            .iter()
            .map(|event| match *event {
                Input::Key { scan, extended, down } => key_input(scan, extended, down),
                Input::MoveBy { dx, dy } => mouse_input(dx, dy, MOUSEEVENTF_MOVE.0),
                Input::MoveTo { x, y } => {
                    let (sx, sy) = (origin.x + x.min(window.width.saturating_sub(1)) as i32, origin.y + y.min(window.height.saturating_sub(1)) as i32);
                    mouse_input(absolute(sx, vx, vw), absolute(sy, vy, vh), MOUSEEVENTF_MOVE.0 | MOUSEEVENTF_ABSOLUTE.0 | MOUSEEVENTF_VIRTUALDESK.0)
                }
                Input::Button { button, down } => mouse_input(0, 0, button_flags(button, down)),
            })
            .collect()
    }

    /// Sends `input`, checking right before that `window` has the foreground. A person or a dialog taking the focus
    /// between the check and the call is a window of microseconds Windows gives no way to close.
    pub fn inject(window: &GameWindow, input: &[Input]) -> Result<(), Failure> {
        let events = events(window, input);
        if !is_foreground(window.handle) {
            return Err(Failure::new(
                "not_foreground",
                "VRChat is not the foreground window, so no keyboard or mouse input was sent.", // i18n: not shown (an AI client reads it in the tool's answer)
                "Moving with walk and turn works without the focus. For menus call focus_game; if that does not help, the person must click VRChat.", // i18n: not shown (an AI client reads it in the tool's answer)
            ));
        }
        // SAFETY: `events` lives for the call and the size is the structure's own.
        let sent = unsafe { SendInput(&events, std::mem::size_of::<INPUT>() as i32) };
        if sent as usize != events.len() {
            return Err(Failure::new(
                "input_blocked",
                format!("Windows took {sent} of {} input events: something blocks input (a window running as administrator, the lock screen).", events.len()), // i18n: not shown (an AI client reads it in the tool's answer)
                "Ask the person to look at the computer.", // i18n: not shown (an AI client reads it in the tool's answer)
            ));
        }
        Ok(())
    }

    /// Brings keys and buttons up wherever the focus is.
    pub fn release(input: &[Input]) {
        let events: Vec<INPUT> = input
            .iter()
            .filter_map(|event| match *event {
                Input::Key { scan, extended, down: false } => Some(key_input(scan, extended, false)),
                Input::Button { button, down: false } => Some(mouse_input(0, 0, button_flags(button, false))),
                _ => None,
            })
            .collect();
        if !events.is_empty() {
            // SAFETY: as in `inject`.
            unsafe { SendInput(&events, std::mem::size_of::<INPUT>() as i32) };
        }
    }

    /// A text value of the registry.
    fn registry(root: windows::Win32::System::Registry::HKEY, key: &str, value: Option<&str>) -> Option<String> {
        let mut buffer = [0u16; 1024];
        let mut size = std::mem::size_of_val(&buffer) as u32;
        let value = value.map(HSTRING::from).unwrap_or_default();
        // SAFETY: the buffer and its size are passed; the names live for the call.
        let status = unsafe { RegGetValueW(root, &HSTRING::from(key), &value, RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&mut size)) };
        status.is_ok().then(|| String::from_utf16_lossy(&buffer[..(size as usize / 2).saturating_sub(1)]))
    }

    /// Where Steam is, as Steam writes it.
    pub(super) fn steam_exe() -> Option<std::path::PathBuf> {
        registry(HKEY_CURRENT_USER, r"Software\Valve\Steam", Some("SteamExe")).map(std::path::PathBuf::from).filter(|p| p.is_file())
    }

    /// VRChat's own launcher, from the `vrchat://` link it registers.
    pub(super) fn vrchat_launcher() -> Option<std::path::PathBuf> {
        let command = registry(HKEY_CLASSES_ROOT, r"VRChat\shell\open\command", None)?;
        launcher_of(&command).filter(|p| p.is_file())
    }

    /// Starts another VRChat beside those running: through VRChat's own `launch.exe` with `--profile` (measured 2026-11-15:
    /// it then starts a process of its own every time, in 1.3 to 3.2 s, and never hands the link to a running VRChat).
    /// Not Steam's `-applaunch`, which starts a game once. Never `VRChat.exe` itself: Easy Anti-Cheat starts from the
    /// launcher.
    pub fn start_vrchat_beside(args: &[String]) -> Result<(), Failure> {
        let Some(launcher) = vrchat_launcher() else {
            return Err(Failure::new(
                "vrchat_not_installed",
                "VRChat's launcher was not found on this computer.", // i18n: not shown (an AI client reads it in the tool's answer)
                "Ask the person to start VRChat once, so that it registers itself.", // i18n: not shown (an AI client reads it in the tool's answer)
            ));
        };
        let mut command = std::process::Command::new(launcher);
        // As the registered link: the link first (as measured), then the options.
        let (links, options): (Vec<&String>, Vec<&String>) = args.iter().partition(|a| a.starts_with("vrchat://"));
        command.args(links).args(options).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        command.spawn().map(drop).map_err(|e| Failure::new("start_failed", format!("VRChat did not start: {e}"), "Ask the person to look at the computer.")) // i18n: not shown
    }

    /// Which process owns each IPv4 UDP port on this computer (`GetExtendedUdpTable`, what `netstat -ano` reads): a plain
    /// query of the system's table, nothing opened in any process.
    pub fn udp_owners() -> std::collections::HashMap<u16, u32> {
        #[link(name = "iphlpapi")]
        extern "system" {
            fn GetExtendedUdpTable(table: *mut core::ffi::c_void, size: *mut u32, order: i32, family: u32, class: i32, reserved: u32) -> u32;
        }
        const AF_INET: u32 = 2;
        const UDP_TABLE_OWNER_PID: i32 = 1;
        let mut owners = std::collections::HashMap::new();
        let mut size = 0u32;
        // SAFETY: a size query with no buffer, then a call into a buffer of the size asked for, read within its length.
        unsafe {
            GetExtendedUdpTable(std::ptr::null_mut(), &mut size, 0, AF_INET, UDP_TABLE_OWNER_PID, 0);
            for _ in 0..3 {
                let mut buffer = vec![0u32; (size as usize).div_ceil(4) + 64];
                let mut have = (buffer.len() * 4) as u32;
                let status = GetExtendedUdpTable(buffer.as_mut_ptr().cast(), &mut have, 0, AF_INET, UDP_TABLE_OWNER_PID, 0);
                if status == 122 {
                    // ERROR_INSUFFICIENT_BUFFER: sockets came meanwhile.
                    size = have;
                    continue;
                }
                if status != 0 {
                    break;
                }
                // MIB_UDPTABLE_OWNER_PID: a count, then rows of (address, port in network order, process id).
                let count = (buffer[0] as usize).min((buffer.len() - 1) / 3);
                for row in buffer[1..1 + count * 3].as_chunks::<3>().0 {
                    let port = u16::from_be((row[1] & 0xFFFF) as u16);
                    owners.insert(port, row[2]);
                }
                break;
            }
        }
        owners
    }

    /// Whether the service's registry key says Windows marked it for deletion (`DeleteFlag` 1): read from the registry, never
    /// through the service manager, so that the bridge holds no handle of the service at any moment.
    pub fn service_delete_pending(service: &str) -> bool {
        use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
        let mut value = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let key = format!(r"SYSTEM\CurrentControlSet\Services\{service}");
        // SAFETY: a u32 and its size are passed; the names live for the call.
        let status = unsafe {
            RegGetValueW(HKEY_LOCAL_MACHINE, &HSTRING::from(key), &HSTRING::from("DeleteFlag"), RRF_RT_REG_DWORD, None, Some(&mut value as *mut u32 as *mut _), Some(&mut size))
        };
        status.is_ok() && value != 0
    }

    /// The process whose window is the foreground now.
    pub fn foreground_pid() -> Option<u32> {
        let mut pid = 0;
        // SAFETY: plain queries.
        unsafe {
            let window = GetForegroundWindow();
            if window.0.is_null() {
                return None;
            }
            GetWindowThreadProcessId(window, Some(&mut pid));
        }
        (pid != 0).then_some(pid)
    }

    /// Starts VRChat the way a person does, so that its anti-cheat starts as ever: through Steam (`-applaunch`, which unlike
    /// a `steam://` link asks nothing and passes the arguments on), else through the launcher VRChat registers for its
    /// links. Never `VRChat.exe` itself.
    pub fn start_vrchat(args: &[String]) -> Result<(), Failure> {
        let mut command = if let Some(steam) = steam_exe() {
            let mut command = std::process::Command::new(steam);
            command.args(["-applaunch", "438100"]).args(args);
            command
        } else if let Some(launcher) = vrchat_launcher() {
            let mut command = std::process::Command::new(launcher);
            // As the registered link: the link first.
            command.args(args.iter().rev().take(1)).args(&args[..args.len().saturating_sub(1)]);
            command
        } else {
            return Err(Failure::new(
                "vrchat_not_installed",
                "Neither Steam nor VRChat's launcher was found on this computer.", // i18n: not shown (an AI client reads it in the tool's answer)
                "Ask the person to install VRChat, or to start it once so that it registers itself.", // i18n: not shown (an AI client reads it in the tool's answer)
            ));
        };
        command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        command.spawn().map(drop).map_err(|e| Failure::new("start_failed", format!("VRChat did not start: {e}"), "Ask the person to look at the computer.")) // i18n: not shown
    }
}

/// The program in a registered command line: `"C:\…\launch.exe" "%1" %*`.
pub fn launcher_of(command: &str) -> Option<std::path::PathBuf> {
    let command = command.trim();
    let program = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => command.split(' ').next()?,
    };
    (!program.is_empty()).then(|| std::path::PathBuf::from(program))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_positions_land_on_their_pixels() {
        // A 2560 wide desktop starting 1920 left of the primary screen's corner.
        let (origin, size) = (-1920, 4480);
        for pixel in [-1920, -1, 0, 100, 2559] {
            let scaled = absolute(pixel, origin, size);
            assert!((0..=65535).contains(&scaled));
            // Windows' own mapping back.
            let back = origin + (scaled as i64 * size as i64 / 65536) as i32;
            assert_eq!(back, pixel, "{pixel}");
        }
        assert_eq!(absolute(5, 0, 1), 0);
    }

    #[test]
    fn keys_go_by_scan_code_and_buttons_by_their_flags() {
        assert_eq!(key_flags(false, true), 0x0008);
        assert_eq!(key_flags(false, false), 0x000A);
        assert_eq!(key_flags(true, true), 0x0009);
        assert_eq!(button_flags(MouseButton::Left, true), 0x0002);
        assert_eq!(button_flags(MouseButton::Left, false), 0x0004);
        assert_eq!(button_flags(MouseButton::Right, false), 0x0010);
    }

    #[test]
    fn the_registered_launcher_is_read_from_its_command_line() {
        assert_eq!(
            launcher_of(r#""C:\Program Files (x86)\Steam\steamapps\common\VRChat\launch.exe" "%1" %*"#).unwrap(),
            std::path::PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\VRChat\launch.exe")
        );
        assert_eq!(launcher_of(r"C:\VRChat\launch.exe %1").unwrap(), std::path::PathBuf::from(r"C:\VRChat\launch.exe"));
        assert_eq!(launcher_of(""), None);
    }
}
