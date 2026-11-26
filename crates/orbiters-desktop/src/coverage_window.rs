//! What a recording has covered, in desktop mode: VRChat without SteamVR has no headset to put the SteamVR overlay
//! (overlay/coverage.rs) in, so the same panel shows in a corner of the screen VRChat is on, above every window (VRChat's
//! borderless full screen included), click-through and never taking the focus from VRChat. The tray service paints it
//! on the processor (as the SteamVR overlay does) while a recording runs and VRChat runs without SteamVR.
//!
//! It is never in what Orbiters records either: the frames are VRChat's camera's own render target, shared over Spout
//! inside VRChat's process, not a picture of the screen.

use crate::overlay::coverage::{paint_panel, wanted};
use crate::overlay::Canvas;
use crate::state::Snapshot;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The panel's size in pixels, three quarters of the SteamVR one.
const PX: [u32; 2] = [450, 338];
/// From the screen's corner.
const MARGIN: i32 = 24;

/// Whether the desktop panel shows: a recording the overlay would show, with VRChat running without SteamVR.
pub fn wanted_on_desktop(snap: &Snapshot) -> bool {
    snap.vrchat.running && !snap.steamvr.running && wanted(snap).is_some()
}

/// Watches the service's state and shows the panel while [`wanted_on_desktop`].
pub fn start(ctl: crate::controller::Shared) {
    if orbiters_core::here::LINUX && !orbiters_core::here::HERE.desktop_panels {
        return;
    }
    let spawned = std::thread::Builder::new().name("coverage-window".into()).spawn(move || {
        let started = Instant::now();
        let mut window: Option<win::Panel> = None;
        let mut canvas: Option<Canvas> = None;
        let mut shown: Option<Arc<Snapshot>> = None;
        let mut last_paint: Option<Instant> = None;
        let rest = ctl.rest("coverage-window", wanted_on_desktop);
        loop {
            let snap = ctl.view();
            if !wanted_on_desktop(&snap) {
                if let Some(w) = window.take() {
                    w.close();
                }
                shown = None;
                rest.wait(None);
                continue;
            }
            if window.is_none() {
                match win::Panel::open(PX[0], PX[1], MARGIN) {
                    Ok(w) => window = Some(w),
                    Err(e) => {
                        log_warn!("coverage panel: {e}", e = e);
                        std::thread::sleep(Duration::from_secs(5));
                        continue;
                    }
                }
            }
            let canvas = canvas.get_or_insert_with(|| Canvas::new(PX[0], PX[1]));
            let changed = !shown.as_ref().is_some_and(|s| Arc::ptr_eq(s, &snap));
            let settling = last_paint.is_some_and(|t| t.elapsed() < Duration::from_secs(2));
            if changed || settling {
                shown = Some(snap.clone());
                last_paint = Some(Instant::now());
                orbiters_ui::anim::set_live(canvas.ctx(), !snap.settings.reduce_motion);
                let size = egui::vec2(PX[0] as f32, PX[1] as f32);
                canvas.paint_ui(started.elapsed().as_secs_f64(), |ui| paint_panel(ui, egui::Rect::from_min_size(egui::pos2(0.0, 0.0), size), &snap));
                if let Some(w) = window.as_mut() {
                    w.show(canvas.pixels());
                }
            }
            if let Some(w) = window.as_ref() {
                w.pump();
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
    if let Err(e) = spawned {
        log_warn!("coverage panel: {e}", e = e);
    }
}

#[cfg(windows)]
mod win {
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW, MonitorFromWindow, ReleaseDC, SelectObject, AC_SRC_ALPHA,
        AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, GetForegroundWindow, PeekMessageW, ShowWindow, TranslateMessage, UpdateLayeredWindow, MSG, PM_REMOVE,
        SW_SHOWNOACTIVATE, ULW_ALPHA, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    /// A click-through, always-on-top, layered window at the bottom right of the screen the foreground window (VRChat)
    /// is on, showing premultiplied pixels.
    pub struct Panel {
        hwnd: HWND,
        dc: HDC,
        bitmap: HBITMAP,
        bits: *mut u8,
        size: SIZE,
        at: POINT,
    }

    // The window and its bitmap stay on the thread that made them.
    impl Panel {
        pub fn open(width: u32, height: u32, margin: i32) -> anyhow::Result<Self> {
            // SAFETY: plain Win32 calls; every handle made here is released in `close`.
            unsafe {
                let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                let monitor = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
                let work = if GetMonitorInfoW(monitor, &mut info).as_bool() { info.rcWork } else { Default::default() };
                let at = POINT { x: work.right - width as i32 - margin, y: work.bottom - height as i32 - margin };
                // The system's STATIC class: nothing to register, and the window takes no input anyway.
                let hwnd = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("STATIC"),
                    w!("Orbiters coverage"), // i18n: not shown
                    WS_POPUP,
                    at.x,
                    at.y,
                    width as i32,
                    height as i32,
                    None,
                    None,
                    None,
                    None,
                )?;
                let screen = GetDC(None);
                let dc = CreateCompatibleDC(Some(screen));
                ReleaseDC(None, screen);
                let header = BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    // Negative: top-down rows, as the canvas paints them.
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                };
                let bmi = BITMAPINFO { bmiHeader: header, ..Default::default() };
                let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
                let bitmap = match CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = DeleteDC(dc);
                        let _ = DestroyWindow(hwnd);
                        return Err(e.into());
                    }
                };
                SelectObject(dc, bitmap.into());
                Ok(Self { hwnd, dc, bitmap, bits: bits.cast(), size: SIZE { cx: width as i32, cy: height as i32 }, at })
            }
        }

        /// Shows `rgba` (premultiplied, top-down, the window's size).
        pub fn show(&mut self, rgba: &[u8]) {
            let n = (self.size.cx * self.size.cy) as usize;
            if rgba.len() < n * 4 {
                return;
            }
            // SAFETY: the DIB section holds `n` 32-bit pixels for as long as the panel lives.
            let out = unsafe { std::slice::from_raw_parts_mut(self.bits, n * 4) };
            for (o, i) in out.as_chunks_mut::<4>().0.iter_mut().zip(rgba.as_chunks::<4>().0) {
                // Windows wants BGRA, premultiplied.
                o[0] = i[2];
                o[1] = i[1];
                o[2] = i[0];
                o[3] = i[3];
            }
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let origin = POINT { x: 0, y: 0 };
            // SAFETY: live handles; the structures outlive the call.
            unsafe {
                let screen = GetDC(None);
                let _ = UpdateLayeredWindow(
                    self.hwnd,
                    Some(screen),
                    Some(&self.at),
                    Some(&self.size),
                    Some(self.dc),
                    Some(&origin),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
                ReleaseDC(None, screen);
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
        }

        /// Handles the window's messages (it takes no input; the system still talks to it).
        pub fn pump(&self) {
            let mut msg = MSG::default();
            // SAFETY: messages of this thread's window only.
            unsafe {
                while PeekMessageW(&mut msg, Some(self.hwnd), 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }

        pub fn close(self) {
            // SAFETY: made in `open`, released once.
            unsafe {
                let _ = DeleteDC(self.dc);
                let _ = DeleteObject(self.bitmap.into());
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[path = "linux/coverage.rs"]
mod win;

#[cfg(not(any(windows, target_os = "linux")))]
mod win {
    pub struct Panel;

    impl Panel {
        pub fn open(_: u32, _: u32, _: i32) -> anyhow::Result<Self> {
            anyhow::bail!("{}", orbiters_i18n::tr("only on Windows"))
        }
        pub fn show(&mut self, _: &[u8]) {}
        pub fn pump(&self) {}
        pub fn close(self) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CaptureState;

    #[test]
    fn it_shows_in_desktop_mode_only() {
        let mut snap = Snapshot::new(orbiters_core::settings::Settings::default());
        snap.capture.state = CaptureState::Recording;
        assert!(!wanted_on_desktop(&snap), "VRChat not running");
        snap.vrchat.running = true;
        assert!(wanted_on_desktop(&snap));
        snap.steamvr.running = true;
        assert!(!wanted_on_desktop(&snap), "the SteamVR overlay shows it");
    }
}
