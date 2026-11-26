//! What Home's Play on a Mac asks of the Mac itself: the main display's drawing resolution and its panel's real pixels
//! (for the render scale VRChat is started with), and whether Low Power Mode is on while the Mac is on power (said to
//! the player, never changed: it is their battery policy).
use orbiters_core::locks::*;
use std::ffi::{c_char, c_void};
use std::sync::Mutex;
use std::time::{Duration, Instant};

type CFTypeRef = *const c_void;
type CFArrayRef = *const c_void;
type CFStringRef = *const c_void;
type CGDisplayModeRef = *const c_void;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
    fn CGDisplayCopyDisplayMode(display: u32) -> CGDisplayModeRef;
    fn CGDisplayCopyAllDisplayModes(display: u32, options: *const c_void) -> CFArrayRef;
    fn CGDisplayModeGetPixelWidth(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: CGDisplayModeRef) -> usize;
    fn CGDisplayModeGetIOFlags(mode: CGDisplayModeRef) -> u32;
    fn CGDisplayModeRelease(mode: CGDisplayModeRef);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetCount(array: CFArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, index: isize) -> *const c_void;
    fn CFRelease(value: CFTypeRef);
    fn CFStringGetCString(string: CFStringRef, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSGetProvidingPowerSourceType(snapshot: CFTypeRef) -> CFStringRef;
}

/// IOKit's flag of the display mode that is the panel's own (`kDisplayModeNativeFlag`).
const NATIVE: u32 = 0x0200_0000;

/// The main display: its drawing resolution in pixels (2940 x 1912 for this Mac's 1470 x 956 points at 2x) and its
/// panel's real pixels (2560 x 1664). `None` when macOS says nothing.
pub fn sizes() -> Option<((u32, u32), (u32, u32))> {
    // SAFETY: CoreGraphics' own calls on the main display, each copy released once.
    unsafe {
        let display = CGMainDisplayID();
        let current = CGDisplayCopyDisplayMode(display);
        if current.is_null() {
            return None;
        }
        let backing = (CGDisplayModeGetPixelWidth(current) as u32, CGDisplayModeGetPixelHeight(current) as u32);
        CGDisplayModeRelease(current);
        let mut panel = backing;
        let all = CGDisplayCopyAllDisplayModes(display, std::ptr::null());
        if !all.is_null() {
            for index in 0..CFArrayGetCount(all) {
                let mode = CFArrayGetValueAtIndex(all, index);
                if CGDisplayModeGetIOFlags(mode) & NATIVE != 0 {
                    panel = (CGDisplayModeGetPixelWidth(mode) as u32, CGDisplayModeGetPixelHeight(mode) as u32);
                    break;
                }
            }
            CFRelease(all);
        }
        (backing.0 > 0 && backing.1 > 0).then_some((backing, panel))
    }
}

/// Whether the Mac draws from its power adapter now.
fn on_power() -> bool {
    // SAFETY: IOKit's snapshot, released once; the source type is borrowed from it and copied out while it lives.
    unsafe {
        let snapshot = IOPSCopyPowerSourcesInfo();
        if snapshot.is_null() {
            return false;
        }
        let kind = IOPSGetProvidingPowerSourceType(snapshot);
        let mut buffer = [0 as c_char; 64];
        let read = !kind.is_null() && CFStringGetCString(kind, buffer.as_mut_ptr(), buffer.len() as isize, 0x0800_0100) != 0;
        CFRelease(snapshot);
        read && std::ffi::CStr::from_ptr(buffer.as_ptr()).to_bytes() == b"AC Power"
    }
}

/// Low Power Mode is on while the Mac is on power: VRChat may run slower than it could. Read again at most every five
/// seconds (Home draws it every frame).
pub fn low_power_on_power() -> bool {
    static SEEN: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut seen = SEEN.locked();
    if let Some((at, value)) = *seen {
        if at.elapsed() < Duration::from_secs(5) {
            return value;
        }
    }
    let value = objc2_foundation::NSProcessInfo::processInfo().isLowPowerModeEnabled() && on_power();
    *seen = Some((Instant::now(), value));
    value
}

/// Opens System Settings at Battery, where the person can change it themselves.
pub fn open_battery_settings() {
    let _ = std::process::Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.Battery-Settings.extension").spawn();
}
