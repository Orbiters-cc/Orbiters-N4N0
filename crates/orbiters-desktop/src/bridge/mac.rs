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

/// The content of `window` in backing pixels at `scale`, its title bar of `title` points left out.
pub fn content_pixels(window: &MacWindow, scale: f64, title: f64) -> (u32, u32) {
    let w = (window.width * scale).round().max(0.0) as u32;
    let h = ((window.height - title) * scale).round().max(0.0) as u32;
    (w, h)
}

/// The pixels the game itself draws in `window` at `scale` backing pixels per point, its title bar of `title` points left
/// out, when its engine draws at `render_percent` % of the display's drawing resolution (`obelisk-cli steam play
/// --render-percent`): the content's backing pixels at 100, fewer below (macOS scales the picture up to the window). A
/// capture is always the window's backing pixels; the bridge's own starts take no render percent (100), so for them the
/// game's pixels and a capture's are one and the same (`controller/obelisk_cli.rs`, `start_reserved`).
pub fn guest_pixels(window: &MacWindow, scale: f64, title: f64, render_percent: u32) -> (u32, u32) {
    content_pixels(window, scale * f64::from(render_percent.clamp(1, 100)) / 100.0, title)
}

/// The rows of a window capture that are its title bar of `title` points, at the capture's own size: a capture of a frame
/// `frame_height` points high `image_height` pixels tall.
pub fn title_rows(image_height: u32, frame_height: f64, title: f64) -> u32 {
    if frame_height <= title || title <= 0.0 {
        return 0;
    }
    ((image_height as f64) * title / frame_height).round() as u32
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
        fn CGGetDisplaysWithRect(rect: CGRect, max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGDisplayCopyDisplayMode(display: u32) -> CFTypeRef;
        fn CGDisplayBounds(display: u32) -> CGRect;
        fn CGDisplayModeGetWidth(mode: CFTypeRef) -> usize;
        fn CGDisplayModeGetPixelWidth(mode: CFTypeRef) -> usize;
        fn CGDisplayModeRelease(mode: CFTypeRef);
        static kCGWindowNumber: CFTypeRef;
        static kCGWindowOwnerPID: CFTypeRef;
        static kCGWindowBounds: CFTypeRef;
        static kCGWindowLayer: CFTypeRef;
        static kCGWindowIsOnscreen: CFTypeRef;
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

    /// The windows on screen, front to back, those of the desktop left out.
    pub fn on_screen() -> Vec<MacWindow> {
        list(ON_SCREEN_ONLY | EXCLUDE_DESKTOP)
    }

    /// The ordinary window in front of all others now.
    pub fn front_window() -> Option<u32> {
        list(ON_SCREEN_ONLY | EXCLUDE_DESKTOP).into_iter().find(|w| w.layer == 0 && w.width > 1.0 && w.height > 1.0).map(|w| w.id)
    }

    /// Backing pixels per point of the display that shows most of `window` (2 on a Retina display), from its display mode:
    /// `CGDisplayPixelsWide` and `CGDisplayBounds` both count points (1470 on this Mac's 2940-pixel panel).
    pub fn scale_of(window: &MacWindow) -> f64 {
        let Some(display) = display_of(window) else { return 1.0 };
        // SAFETY: plain queries; the mode is released.
        unsafe {
            let mode = CGDisplayCopyDisplayMode(display);
            if mode.is_null() {
                return 1.0;
            }
            let scale = super::mode_scale(CGDisplayModeGetPixelWidth(mode), CGDisplayModeGetWidth(mode));
            CGDisplayModeRelease(mode);
            scale
        }
    }

    /// The display that shows most of `window` (by the area of each display it covers), else the main one.
    fn display_of(window: &MacWindow) -> Option<u32> {
        let rect = CGRect { x: window.x, y: window.y, w: window.width.max(1.0), h: window.height.max(1.0) };
        let mut displays = [0u32; 8];
        let mut count = 0u32;
        // SAFETY: plain queries into buffers of ours.
        unsafe {
            if CGGetDisplaysWithRect(rect, displays.len() as u32, displays.as_mut_ptr(), &mut count) != 0 || count == 0 {
                return Some(CGMainDisplayID());
            }
            let bounds: Vec<(u32, super::Frame)> = displays[..count as usize].iter().map(|d| {
                let b = CGDisplayBounds(*d);
                (*d, (b.x, b.y, b.w, b.h))
            }).collect();
            super::most_covered((window.x, window.y, window.width, window.height), &bounds).or(Some(CGMainDisplayID()))
        }
    }

    /// The frame of the display that shows most of `window`, in points.
    pub fn display_frame(window: &MacWindow) -> Option<super::Frame> {
        let display = display_of(window)?;
        // SAFETY: a plain query.
        let b = unsafe { CGDisplayBounds(display) };
        Some((b.x, b.y, b.w, b.h))
    }

    /// Seconds since the last keyboard, mouse or trackpad event of the HID system, whoever's (the bridge sends none on a Mac).
    pub fn idle_seconds() -> f64 {
        // SAFETY: a plain query: `kCGEventSourceStateHIDSystemState` (1), `kCGAnyInputEventType` (!0).
        unsafe { CGEventSourceSecondsSinceLastEventType(1, u32::MAX) }
    }

    /// A picture of the window `id` alone as PNG bytes, through macOS's `screencapture -l` (its shadow left out, no sound):
    /// into a file of this call's own, refused unless the tool succeeded and wrote one, ended past [`super::CAPTURE_LIMIT`].
    pub fn capture_png(id: u32) -> Result<Vec<u8>, String> {
        let folder = std::env::temp_dir().join(format!("orbiters-bridge-shot-{}-{id}-{}", std::process::id(), super::next_shot()));
        std::fs::create_dir(&folder).map_err(|e| e.to_string())?;
        // Removed however this call ends (a failure, a panic): no picture is left behind.
        let _gone = super::Gone(folder.clone());
        let file = folder.join("shot.png");
        let mut child = std::process::Command::new("/usr/sbin/screencapture")
            .args(["-l", &id.to_string(), "-o", "-x", "-t", "png"])
            .arg(&file)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let status = super::wait_until(&mut child, super::CAPTURE_LIMIT)?;
        if !status.success() {
            return Err(format!("screencapture ended with {status}")); // i18n: not shown (an AI client reads it)
        }
        std::fs::read(&file).map_err(|e| e.to_string())
    }
}

/// A folder of this process's own, removed when dropped.
pub struct Gone(pub std::path::PathBuf);

impl Drop for Gone {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A frame in points: x, y, width, height.
pub type Frame = (f64, f64, f64, f64);

/// The display (of `displays`: id, frame) that `window`'s frame covers the most of; `None` when it covers none.
pub fn most_covered(window: Frame, displays: &[(u32, Frame)]) -> Option<u32> {
    let area = |d: &(f64, f64, f64, f64)| {
        let w = (window.0 + window.2).min(d.0 + d.2) - window.0.max(d.0);
        let h = (window.1 + window.3).min(d.1 + d.3) - window.1.max(d.1);
        w.max(0.0) * h.max(0.0)
    };
    displays.iter().map(|(id, d)| (*id, area(d))).filter(|(_, a)| *a > 0.0).max_by(|a, b| a.1.total_cmp(&b.1)).map(|(id, _)| id)
}

/// The title bar of a window, in points: none when the window covers its whole display (the game borderless or full
/// screen), else the engine's 32.
pub fn title_points(window: &MacWindow, display: Option<Frame>) -> f64 {
    let covers = display.is_some_and(|(x, y, w, h)| (window.x - x).abs() < 1.0 && (window.y - y).abs() < 1.0 && (window.width - w).abs() < 1.0 && (window.height - h).abs() < 1.0);
    if covers {
        0.0
    } else {
        TITLE_BAR_POINTS
    }
}

/// The title bar of a window whose game renders `guest_height` pixels at `scale` backing pixels per point at 100 %: none
/// or the engine's 32 points, when the frame says one of them exactly (borderless windows smaller than their display have
/// none); `None` when it says neither (the caller keeps its own rule).
pub fn title_from_guest(frame_height: f64, scale: f64, guest_height: u32) -> Option<f64> {
    if scale <= 0.0 {
        return None;
    }
    let extra = frame_height - guest_height as f64 / scale;
    if extra.abs() < 1.5 {
        Some(0.0)
    } else if (extra - TITLE_BAR_POINTS).abs() < 1.5 {
        Some(TITLE_BAR_POINTS)
    } else {
        None
    }
}

/// The title bar cut off a picture of a window, in points: the one its window server said it has, else the one the size the
/// game renders shows; where neither tells, nothing is cut. A name macOS gives the window says nothing of its decorations.
pub fn title_cut(said: Option<f64>, guest: Option<f64>) -> f64 {
    said.or(guest).unwrap_or(0.0)
}

/// What the engine's window server says in its log of the windows it shows: whether each has a title bar, the window's own
/// native fact, whatever size the game renders and however macOS stretches it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EngineWindows {
    /// The windows there are, oldest first.
    windows: Vec<EngineWindow>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct EngineWindow {
    id: u32,
    /// Whether its style has a title bar; `None` once it may have changed unsaid.
    titled: Option<bool>,
    full_screen: bool,
    /// Its content, in the window server's pixels, as last said.
    size: (f64, f64),
}

impl EngineWindows {
    /// A style's title bar: `WSD_STYLE_TITLED`, unless `WSD_STYLE_BORDERLESS` (which overrides every other).
    fn titled(style: &str) -> Option<bool> {
        let style = u32::from_str_radix(style.trim().strip_prefix("0x")?, 16).ok()?;
        Some(style & 1 != 0 && style & 0x10 == 0)
    }

    /// Takes one line of the engine's log, whole.
    pub fn read(&mut self, line: &str) {
        let Some((_, said)) = line.split_once("wsd: ") else { return }; // i18n: not shown (the engine's log lines)
        // `client 34529: window 1 800x450 px style 0x10`
        if let Some((client, made)) = said.strip_prefix("client ").and_then(|r| r.split_once(": window ")) {
            let (Some((id, rest)), true) = (made.split_once(' '), client.bytes().all(|b| b.is_ascii_digit())) else { return };
            let (Ok(id), Some((size, style))) = (id.parse(), rest.split_once(" px style ")) else { return };
            let Some(size) = Self::size(size) else { return };
            self.windows.retain(|w| w.id != id);
            self.windows.push(EngineWindow { id, titled: Self::titled(style), full_screen: false, size });
            let most = self.windows.len().saturating_sub(64);
            self.windows.drain(..most);
            return;
        }
        // `window 1 style 0x1`, `window 1 [fullscreen-on]: …`, `window 1 [destroy]: …`
        let Some((id, what)) = said.strip_prefix("window ").and_then(|r| r.split_once(' ')) else { return };
        let Ok(id) = id.parse::<u32>() else { return };
        if what.starts_with("[destroy]") {
            self.windows.retain(|w| w.id != id);
            return;
        }
        let Some(window) = self.windows.iter_mut().find(|w| w.id == id) else { return };
        if let Some(frame) = what.strip_prefix("frame px (") {
            // `(452,329 800x450) flags 0x1`
            if let Some(size) = frame.split_once(')').and_then(|(frame, _)| Self::size(frame.split_once(' ')?.1)) {
                window.size = size;
            }
        } else if let Some(style) = what.strip_prefix("style ") {
            window.titled = Self::titled(style);
        } else if what.starts_with("[fullscreen-on]") {
            window.full_screen = true;
        } else if what.starts_with("[fullscreen-off]") {
            // A style set while it was full screen applies now and was never said.
            window.full_screen = false;
            window.titled = None;
        }
    }

    fn size(said: &str) -> Option<(f64, f64)> {
        let (w, h) = said.split_once('x')?;
        Some((f64::from(w.parse::<u32>().ok()?), f64::from(h.parse::<u32>().ok()?)))
    }

    /// The title bar, in points, of the window whose frame is `width` x `height` points on a display of `scale` backing pixels
    /// per point, which it `covers` or not: what is said of the window of that very size, content and title bar together, and
    /// of no other. `None` when none of them is it, or more than one that differ are.
    pub fn title(&self, (width, height): (f64, f64), scale: f64, covers: bool) -> Option<f64> {
        if scale <= 0.0 {
            return None;
        }
        let near = |a: f64, b: f64| (a - b).abs() < 1.5;
        let mut titles: Vec<f64> = self
            .windows
            .iter()
            .filter_map(|w| {
                if w.full_screen {
                    // (its size was said before it went full screen)
                    return covers.then_some(0.0);
                }
                let title = if w.titled? { TITLE_BAR_POINTS } else { 0.0 };
                (near(w.size.0 / scale, width) && near(w.size.1 / scale + title, height)).then_some(title)
            })
            .collect();
        titles.dedup();
        match titles[..] {
            [title] => Some(title),
            _ => None,
        }
    }
}

/// How long one window capture may take.
pub const CAPTURE_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A number for each capture of this process: no two share a file.
pub fn next_shot() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Waits for `child` for at most `limit`: past it, the child is ended and waited for, and the wait fails.
pub fn wait_until(child: &mut std::process::Child, limit: std::time::Duration) -> Result<std::process::ExitStatus, String> {
    let until = std::time::Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if std::time::Instant::now() >= until => {
                end(child);
                return Err(format!("no answer within {} s", limit.as_secs())); // i18n: not shown (an AI client reads it)
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => {
                end(child);
                return Err(e.to_string());
            }
        }
    }
}

/// Ends `child` and reaps it, waiting a moment at most: a kill that failed never leads to a wait without end.
fn end(child: &mut std::process::Child) {
    let _ = child.kill();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < until {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Backing pixels per point of a display mode `pixel_width` pixels wide drawn `point_width` points wide (at least 1).
pub fn mode_scale(pixel_width: usize, point_width: usize) -> f64 {
    if point_width == 0 || pixel_width == 0 {
        return 1.0;
    }
    (pixel_width as f64 / point_width as f64).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: u32, width: f64, height: f64, layer: i64, on_screen: bool) -> MacWindow {
        MacWindow { id, owner: 7, x: 0.0, y: 0.0, width, height, layer, on_screen }
    }

    /// Fifth verification, finding 15: a swapchain that is not the window's size, stretched in a smaller borderless window,
    /// matches neither title height: the crop is the window's own native fact, as the engine's window server says it, and
    /// nothing of the content is cut off.
    #[test]
    fn the_crop_is_the_window_s_own_title_bar_whatever_the_game_renders() {
        // (the frame the window has on the screen: its content, and its title bar where it has one)
        let frame = |content: (f64, f64), title: f64| (content.0, content.1 + title);
        let mut engine = EngineWindows::default();
        assert_eq!(engine.title((1470.0, 956.0), 2.0, true), None);
        // As this Mac's engine wrote them on 2026-11-23 (the splash, then the game).
        for line in [
            "wsd: client 34529: window 1 800x450 px style 0x10",
            "wsd: window 1 frame px (452,329 800x450) flags 0x1",
            "wsd: window 1 [destroy]: visible=0 occlusion_visible=0",
            "wsd: client 34541: window 1 1705x1109 px style 0x10",
            "wsd: window 1 [fullscreen-on]: visible=1 occlusion_visible=0",
        ] {
            engine.read(line);
        }
        assert_eq!(engine.title((1470.0, 956.0), 2.0, true), Some(0.0));
        // Borderless, 900 x 500 points, drawn from a 1280 x 720 swapchain: neither title height fits it.
        let mut engine = EngineWindows::default();
        engine.read("wsd: client 1: window 4 1800x1000 px style 0x10");
        assert_eq!(title_from_guest(500.0, 2.0, 720), None, "what the fallback cut 32 points off for");
        assert_eq!(title_points(&w(4, 900.0, 500.0, 0, true), Some((0.0, 0.0, 1470.0, 956.0))), TITLE_BAR_POINTS);
        let (borderless, titled) = (frame((900.0, 500.0), 0.0), frame((900.0, 500.0), TITLE_BAR_POINTS));
        assert_eq!(engine.title(borderless, 2.0, false), Some(0.0));
        assert_eq!(title_rows(1000, 500.0, engine.title(borderless, 2.0, false).unwrap()), 0);
        // The same size with a title bar, restyled, a window above it gone.
        engine.read("wsd: window 4 style 0xf");
        assert_eq!(engine.title(titled, 2.0, false), Some(TITLE_BAR_POINTS));
        // (sixth verification: a newer window's style is not this one's)
        engine.read("wsd: client 1: window 5 100x100 px style 0x10");
        assert_eq!(engine.title(titled, 2.0, false), Some(TITLE_BAR_POINTS));
        assert_eq!(engine.title((50.0, 50.0), 2.0, false), Some(0.0));
        engine.read("wsd: window 5 [destroy]: x");
        assert_eq!(engine.title((50.0, 50.0), 2.0, false), None);
        assert_eq!(engine.title(titled, 2.0, false), Some(TITLE_BAR_POINTS));
        engine.read("wsd: window 4 style 0x1f");
        assert_eq!(engine.title(borderless, 2.0, false), Some(0.0), "borderless overrides the others");
        // Out of full screen its style may have changed unsaid; a line that is not one changes nothing.
        engine.read("wsd: window 4 [fullscreen-on]: x");
        engine.read("wsd: window 4 [fullscreen-off]: x");
        assert_eq!(engine.title(borderless, 2.0, false), None);
        for line in ["wsd: window 9 style 0x1", "wsd: window x style 0x1", "wsd: client 1: window", "window 4 style 0x1", ""] {
            engine.read(line);
        }
        assert_eq!(engine.title(borderless, 2.0, false), None);
    }

    /// Sixth verification, finding 15 (the verifier's fixture): a large borderless window and a newer small titled one, each
    /// with its own title bar; a frame that is neither's has none said.
    #[test]
    fn the_crop_is_the_captured_window_s_own_among_several() {
        let mut engine = EngineWindows::default();
        engine.read("wsd: client 4: window 1 1500x900 px style 0x10");
        engine.read("wsd: client 4: window 2 300x150 px style 0x1");
        assert_eq!(engine.title((750.0, 450.0), 2.0, false), Some(0.0));
        assert_eq!(engine.title((1500.0, 900.0), 1.0, false), Some(0.0));
        assert_eq!(engine.title((150.0, 75.0 + TITLE_BAR_POINTS), 2.0, false), Some(TITLE_BAR_POINTS));
        assert_eq!(engine.title((750.0, 450.0 + TITLE_BAR_POINTS), 2.0, false), None, "the title bar is not that one's");
        assert_eq!(engine.title((640.0, 392.0), 2.0, false), None);
        assert_eq!(engine.title((750.0, 450.0), 0.0, false), None);
        // Its size as last said; a name that says nothing readable changes nothing.
        engine.read("wsd: window 1 frame px (452,329 800x450) flags 0x1");
        engine.read("wsd: window 1 frame px (what");
        assert_eq!(engine.title((400.0, 225.0), 2.0, false), Some(0.0));
        assert_eq!(engine.title((750.0, 450.0), 2.0, false), None);
        // Two that differ in nothing but their title bar, and whose frames are one: not told apart.
        engine.read("wsd: client 4: window 3 800x386 px style 0x1");
        assert_eq!(engine.title((400.0, 225.0), 2.0, false), None);
        // Full screen it has none, and is the one that covers its display.
        engine.read("wsd: window 3 [fullscreen-on]: x");
        assert_eq!(engine.title((1470.0, 956.0), 2.0, true), Some(0.0));
        assert_eq!(engine.title((400.0, 225.0), 2.0, false), Some(0.0));
    }

    #[test]
    fn the_game_window_is_the_engine_s_big_shown_one() {
        // As macOS listed wsd's windows on 2026-11-22: strips off screen, the splash, the game.
        let listed = [w(1, 1470.0, 33.0, 0, false), w(2, 400.0, 225.0, 0, true), w(3, 640.0, 392.0, 0, true), w(4, 195.0, 138.0, 0, true)];
        assert_eq!(game_window(&listed, 7).map(|w| w.id), Some(3));
        assert_eq!(game_window(&listed[..2], 7), None, "the splash alone is no game yet");
        assert_eq!(game_window(&listed, 8), None, "another window server's");
        assert_eq!(game_window(&[w(3, 640.0, 392.0, 0, false)], 7), None, "not shown");
        assert_eq!(content_pixels(&listed[2], 2.0, TITLE_BAR_POINTS), (1280, 720));
        assert_eq!(title_rows(784, 392.0, TITLE_BAR_POINTS), 64, "the capture of a 640 x 392 frame at scale 2");
        assert_eq!(title_rows(392, 392.0, TITLE_BAR_POINTS), 32);
        // Finding 10: the title bar from the size the game renders: a 640 x 360 content in a 640 x 392 frame at scale 2
        // has the engine's 32; a borderless window smaller than its display has none; a size that says neither is not
        // guessed at.
        assert_eq!(title_from_guest(392.0, 2.0, 720), Some(TITLE_BAR_POINTS));
        assert_eq!(title_from_guest(800.0, 2.0, 1600), Some(0.0));
        assert_eq!(title_from_guest(800.0, 2.0, 1100), None);
        assert_eq!(title_from_guest(800.0, 0.0, 1600), None);
        // Seventh verification: a window whose style nothing tells is not cut (a name macOS gave it stood for a title bar).
        assert_eq!(title_cut(None, title_from_guest(800.0, 2.0, 1100)), 0.0);
        assert_eq!(title_cut(None, title_from_guest(392.0, 2.0, 720)), TITLE_BAR_POINTS);
        assert_eq!(title_cut(Some(0.0), Some(TITLE_BAR_POINTS)), 0.0, "what its window server says comes first");
        assert_eq!(title_cut(Some(TITLE_BAR_POINTS), None), TITLE_BAR_POINTS);
        // m17: a borderless game covering its display has no title bar to cut.
        let whole = MacWindow { x: 0.0, y: 0.0, width: 1470.0, height: 956.0, ..listed[2] };
        assert_eq!(title_points(&whole, Some((0.0, 0.0, 1470.0, 956.0))), 0.0);
        assert_eq!(title_points(&listed[2], Some((0.0, 0.0, 1470.0, 956.0))), TITLE_BAR_POINTS);
        assert_eq!(title_rows(1912, 956.0, 0.0), 0);
        assert_eq!(content_pixels(&whole, 2.0, 0.0), (2940, 1912));
        // A window across two displays is on the one it covers the most of.
        let displays = [(1, (0.0, 0.0, 1470.0, 956.0)), (2, (1470.0, 0.0, 1920.0, 1080.0))];
        assert_eq!(most_covered((1400.0, 100.0, 640.0, 392.0), &displays), Some(2), "570 points on the second, 70 on the first");
        assert_eq!(most_covered((100.0, 100.0, 640.0, 392.0), &displays), Some(1));
        assert_eq!(most_covered((5000.0, 5000.0, 10.0, 10.0), &displays), None);
    }

    /// R2: at both scales, the game's pixels, the window's points and a capture's pixels kept apart. Full screen on this
    /// Mac (1470 x 956 points at 2x): 2940 x 1912 drawn at 100 %, 1705 x 1109 at 58 % (Home's default), while a capture
    /// stays 2940 x 1912 and has no title bar to cut; a 640 x 360-point window at 100 % is 1280 x 720 either way.
    #[test]
    fn the_game_s_pixels_follow_the_render_percent_and_a_capture_s_do_not() {
        let whole = MacWindow { x: 0.0, y: 0.0, ..w(3, 1470.0, 956.0, 0, true) };
        let title = title_points(&whole, Some((0.0, 0.0, 1470.0, 956.0)));
        assert_eq!(title, 0.0);
        assert_eq!(guest_pixels(&whole, 2.0, title, 100), (2940, 1912));
        assert_eq!(guest_pixels(&whole, 2.0, title, 58), (1705, 1109));
        assert_eq!(content_pixels(&whole, 2.0, title), (2940, 1912), "the capture");
        assert_eq!(title_rows(1912, 956.0, title), 0);
        let windowed = w(3, 640.0, 392.0, 0, true);
        assert_eq!(guest_pixels(&windowed, 2.0, TITLE_BAR_POINTS, 100), content_pixels(&windowed, 2.0, TITLE_BAR_POINTS), "the bridge's starts");
        assert_eq!(guest_pixels(&windowed, 2.0, TITLE_BAR_POINTS, 100), (1280, 720));
    }

    /// The verifier's case: this Mac's 2940-pixel panel drawn as 1470 points. The window's content is 1280 x 720 pixels,
    /// not 640 x 360.
    #[test]
    fn a_retina_display_mode_gives_scale_2_and_the_content_its_pixels() {
        assert_eq!(mode_scale(2940, 1470), 2.0);
        assert_eq!(mode_scale(1920, 1920), 1.0);
        assert_eq!(mode_scale(0, 1470), 1.0, "a mode that says nothing");
        let game = w(3, 640.0, 392.0, 0, true);
        assert_eq!(content_pixels(&game, mode_scale(2940, 1470), TITLE_BAR_POINTS), (1280, 720));
    }

    #[cfg(unix)]
    #[test]
    fn a_helper_that_hangs_is_ended_at_its_limit() {
        let mut child = std::process::Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let begun = std::time::Instant::now();
        assert!(wait_until(&mut child, std::time::Duration::from_millis(300)).is_err());
        assert!(begun.elapsed() < std::time::Duration::from_secs(5));
        assert!(child.try_wait().unwrap().is_some(), "ended and reaped");
        let mut ok = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        assert!(wait_until(&mut ok, std::time::Duration::from_secs(5)).unwrap().success());
        assert_ne!(next_shot(), next_shot(), "each capture its own file");
    }
}
