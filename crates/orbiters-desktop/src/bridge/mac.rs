//! What the bridge asks of macOS: the windows of the engine's window server (`wsd`) and which one is in front, a picture of
//! one window through the system's own `screencapture -l` (ScreenCaptureKit's single-window capture: the window alone, even
//! covered), the Screen Recording permission that needs, and how long the person has left the keyboard and mouse alone.
//! Nothing here sends input, and nothing opens the game's processes.
//!
//! The coordinate contract (Phase 0, 2026-11-22, VRChat started with `-screen-width 1280 -screen-height 720`): the engine
//! makes the game's window 1280 x 720 backing pixels, which at the display's scale 2 is 640 x 360 points of content under a
//! 32-point title bar (macOS lists the window 640 x 392). [`MacWindow`] keeps both: `GameWindow`'s width and height are
//! the content in backing pixels (what VRChat itself renders and what Unity's `-screen-*` asked), a screenshot is that
//! content alone (the title bar cut off), and its `scale` says how its JPEG's pixels map to them.

#[cfg(target_os = "macos")]
pub use imp::*;

/// A window of the engine's window server as macOS lists it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MacWindow {
    pub id: u32,
    pub owner: u32,
    /// The frame in points, title bar included.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub layer: i64,
    pub on_screen: bool,
    /// Its title, when macOS gives it (only to a process allowed to record the screen).
    pub titled: bool,
}

/// The title bar of the engine's windows, in points (measured: a 640 x 360 content in a 640 x 392 frame).
pub const TITLE_BAR_POINTS: f64 = 32.0;

/// The game's window among a window server's: the largest ordinary one (layer 0) at least 320 x 200 points, never the
/// engine's 400 x 225 splash (measured at every start), the crash box (195 x 138) or the 1470 x 33 strips it keeps off
/// screen. A window macOS does not show is not the game's yet.
pub fn game_window(windows: &[MacWindow], owner: u32) -> Option<MacWindow> {
    windows
        .iter()
        .filter(|w| w.owner == owner && w.layer == 0 && w.on_screen && w.height > TITLE_BAR_POINTS)
        .filter(|w| w.width >= 320.0 && w.height - TITLE_BAR_POINTS >= 200.0 && !(w.width == 400.0 && w.height == 225.0))
        .max_by(|a, b| (a.width * a.height).total_cmp(&(b.width * b.height)))
        .copied()
}

/// The content of `window` in backing pixels at `scale` (title bar left out).
pub fn content_pixels(window: &MacWindow, scale: f64) -> (u32, u32) {
    let w = (window.width * scale).round().max(0.0) as u32;
    let h = ((window.height - TITLE_BAR_POINTS) * scale).round().max(0.0) as u32;
    (w, h)
}

/// The rows of a window capture that are its title bar, at the capture's own size: a capture of a frame `frame_height`
/// points high `image_height` pixels tall.
pub fn title_rows(image_height: u32, frame_height: f64) -> u32 {
    if frame_height <= TITLE_BAR_POINTS {
        return 0;
    }
    ((image_height as f64) * TITLE_BAR_POINTS / frame_height).round() as u32
}

#[cfg(not(target_os = "macos"))]
mod portable {
    /// Elsewhere nothing of macOS is asked.
    pub fn screen_recording() -> Option<bool> {
        None
    }
    pub fn screen_locked() -> bool {
        false
    }
    pub fn ask_screen_recording() {}
}
#[cfg(not(target_os = "macos"))]
pub use portable::*;

#[cfg(target_os = "macos")]
mod imp {
    use super::MacWindow;
    use std::ffi::c_void;

    type CFTypeRef = *const c_void;
    type CFIndex = isize;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct CGRect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
        fn CGWindowListCopyWindowInfo(option: u32, relative: u32) -> CFTypeRef;
        fn CGRectMakeWithDictionaryRepresentation(dict: CFTypeRef, rect: *mut CGRect) -> bool;
        fn CGEventSourceSecondsSinceLastEventType(state: i32, kind: u32) -> f64;
        fn CGSessionCopyCurrentDictionary() -> CFTypeRef;
        fn CGMainDisplayID() -> u32;
        fn CGDisplayPixelsWide(display: u32) -> usize;
        fn CGDisplayBounds(display: u32) -> CGRect;
        static kCGWindowNumber: CFTypeRef;
        static kCGWindowOwnerPID: CFTypeRef;
        static kCGWindowBounds: CFTypeRef;
        static kCGWindowLayer: CFTypeRef;
        static kCGWindowIsOnscreen: CFTypeRef;
        static kCGWindowName: CFTypeRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
        fn CFDictionaryGetValue(dict: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn CFNumberGetValue(number: CFTypeRef, kind: CFIndex, out: *mut c_void) -> bool;
        fn CFBooleanGetValue(boolean: CFTypeRef) -> bool;
        fn CFRelease(object: CFTypeRef);
        fn CFStringCreateWithCString(allocator: CFTypeRef, text: *const std::ffi::c_char, encoding: u32) -> CFTypeRef;
    }

    /// `kCGWindowListOptionAll`, `kCGWindowListOptionOnScreenOnly`, `kCGWindowListExcludeDesktopElements`.
    const LIST_ALL: u32 = 0;
    const ON_SCREEN_ONLY: u32 = 1;
    const EXCLUDE_DESKTOP: u32 = 16;
    /// `kCFNumberSInt64Type`.
    const SINT64: CFIndex = 4;

    /// Whether this process may record the screen (no question is shown).
    pub fn screen_recording() -> Option<bool> {
        // SAFETY: a plain query.
        Some(unsafe { CGPreflightScreenCaptureAccess() })
    }

    /// Asks macOS for Screen Recording for this process: its own question the first time, else nothing but the entry in
    /// System Settings, which is opened for the person to switch it on.
    pub fn ask_screen_recording() {
        // SAFETY: a plain call; macOS shows its own question at most once.
        let allowed = unsafe { CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() };
        if !allowed {
            let _ = std::process::Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture").spawn();
        }
    }

    fn number(dict: CFTypeRef, key: CFTypeRef) -> Option<i64> {
        let mut out = 0i64;
        // SAFETY: the dictionary is alive for the caller's list; the value is a CFNumber when present.
        unsafe {
            let value = CFDictionaryGetValue(dict, key);
            (!value.is_null() && CFNumberGetValue(value, SINT64, &mut out as *mut i64 as *mut c_void)).then_some(out)
        }
    }

    fn list(option: u32) -> Vec<MacWindow> {
        let mut out = Vec::new();
        // SAFETY: the list is ours (copied) and released once; its entries are read while it lives.
        unsafe {
            let array = CGWindowListCopyWindowInfo(option, 0);
            if array.is_null() {
                return out;
            }
            for i in 0..CFArrayGetCount(array) {
                let dict = CFArrayGetValueAtIndex(array, i);
                let (Some(id), Some(owner)) = (number(dict, kCGWindowNumber), number(dict, kCGWindowOwnerPID)) else { continue };
                let mut rect = CGRect::default();
                let bounds = CFDictionaryGetValue(dict, kCGWindowBounds);
                if bounds.is_null() || !CGRectMakeWithDictionaryRepresentation(bounds, &mut rect) {
                    continue;
                }
                let on = CFDictionaryGetValue(dict, kCGWindowIsOnscreen);
                out.push(MacWindow {
                    id: id as u32,
                    owner: owner as u32,
                    x: rect.x,
                    y: rect.y,
                    width: rect.w,
                    height: rect.h,
                    layer: number(dict, kCGWindowLayer).unwrap_or(-1),
                    on_screen: !on.is_null() && CFBooleanGetValue(on),
                    titled: !CFDictionaryGetValue(dict, kCGWindowName).is_null(),
                });
            }
            CFRelease(array);
        }
        out
    }

    /// Whether the Mac's screen is locked now. Measured 2026-11-22: while it is, the engine's window server shows no frame of
    /// the game (`0 shown events`: macOS calls the window covered), so a capture is black whatever the game draws.
    pub fn screen_locked() -> bool {
        // SAFETY: the dictionary and the key are ours and released once; the value is read while the dictionary lives.
        unsafe {
            let session = CGSessionCopyCurrentDictionary();
            if session.is_null() {
                return false;
            }
            let key = CFStringCreateWithCString(std::ptr::null(), c"CGSSessionScreenIsLocked".as_ptr(), 0x0800_0100);
            let value = CFDictionaryGetValue(session, key);
            let locked = !value.is_null() && CFBooleanGetValue(value);
            CFRelease(key);
            CFRelease(session);
            locked
        }
    }

    /// Every window, on screen or not, of every process.
    pub fn windows() -> Vec<MacWindow> {
        list(LIST_ALL)
    }

    /// The process whose ordinary window is in front of all others now.
    pub fn front_owner() -> Option<u32> {
        list(ON_SCREEN_ONLY | EXCLUDE_DESKTOP).into_iter().find(|w| w.layer == 0 && w.width > 1.0 && w.height > 1.0).map(|w| w.owner)
    }

    /// Backing pixels per point of the main display (2 on a Retina display).
    pub fn scale() -> f64 {
        // SAFETY: plain queries.
        unsafe {
            let display = CGMainDisplayID();
            let points = CGDisplayBounds(display).w;
            if points <= 0.0 {
                return 1.0;
            }
            (CGDisplayPixelsWide(display) as f64 / points).max(1.0)
        }
    }

    /// Seconds since the last keyboard, mouse or trackpad event of the HID system, whoever's (the bridge sends none on a Mac).
    pub fn idle_seconds() -> f64 {
        // SAFETY: a plain query: `kCGEventSourceStateHIDSystemState` (1), `kCGAnyInputEventType` (!0).
        unsafe { CGEventSourceSecondsSinceLastEventType(1, u32::MAX) }
    }

    /// A picture of the window `id` alone as PNG bytes, through macOS's `screencapture -l` (its shadow left out, no sound).
    pub fn capture_png(id: u32) -> Result<Vec<u8>, String> {
        let file = std::env::temp_dir().join(format!("orbiters-bridge-shot-{}-{id}.png", std::process::id()));
        let ran = std::process::Command::new("/usr/sbin/screencapture")
            .args(["-l", &id.to_string(), "-o", "-x", "-t", "png"])
            .arg(&file)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| e.to_string())?;
        let bytes = std::fs::read(&file).map_err(|_| String::from_utf8_lossy(&ran.stderr).trim().to_string());
        let _ = std::fs::remove_file(&file);
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: u32, width: f64, height: f64, layer: i64, on_screen: bool) -> MacWindow {
        MacWindow { id, owner: 7, x: 0.0, y: 0.0, width, height, layer, on_screen, titled: true }
    }

    #[test]
    fn the_game_window_is_the_engine_s_big_shown_one() {
        // As macOS listed wsd's windows on 2026-11-22: strips off screen, the splash, the game.
        let listed = [w(1, 1470.0, 33.0, 0, false), w(2, 400.0, 225.0, 0, true), w(3, 640.0, 392.0, 0, true), w(4, 195.0, 138.0, 0, true)];
        assert_eq!(game_window(&listed, 7).map(|w| w.id), Some(3));
        assert_eq!(game_window(&listed[..2], 7), None, "the splash alone is no game yet");
        assert_eq!(game_window(&listed, 8), None, "another window server's");
        assert_eq!(game_window(&[w(3, 640.0, 392.0, 0, false)], 7), None, "not shown");
        assert_eq!(content_pixels(&listed[2], 2.0), (1280, 720));
        assert_eq!(title_rows(784, 392.0), 64, "the capture of a 640 x 392 frame at scale 2");
        assert_eq!(title_rows(392, 392.0), 32);
    }
}
