//! Tracking smoothing for SteamVR, an equivalent of OVR-SmoothTracking: a SteamVR driver (`orbiters-smooth-driver`,
//! `driver_orbiters_smooth.dll`) sees every pose any driver gives SteamVR, eases the position and rotation of the
//! devices the app asks for and damps the velocities SteamVR predicts with, so trackers, controllers and the headset
//! jitter less. How strong it is comes per device index (0 leaves a device untouched).
//!
//! The app and the driver (inside `vrserver.exe`) share a small named memory, the [`ControlBlock`]
//! (`Local\OrbitersSmoothTracking`): the app creates it with [`Control::open_or_create`] and writes whether smoothing
//! is on and each device's strength; the driver opens it (again every second while it is missing), reads it without
//! locks (a sequence counter, [`ControlBlock::settings`]) and answers with a heartbeat ([`Control::driver_heartbeat`]).
//! The driver smooths nothing while the block is missing, off, of another version, or once the process that created
//! it has exited: smoothing lasts as long as the app that holds the [`Control`], so hold it in the long-lived process.
//!
//! [`install`] puts the driver where SteamVR finds it (`%LOCALAPPDATA%\Orbiters\SteamVR\drivers\orbiters_smooth`,
//! registered in `openvrpaths.vrpath`). SteamVR loads drivers only when it starts, so the app registers it once, at
//! its first start, and leaves it registered: loaded at every SteamVR start and doing nothing until the block says on,
//! the driver then follows every change live (as OVR-SmoothTracking's, which its setup puts in SteamVR's own drivers
//! folder). Only the very first registration, if SteamVR was already running, waits for SteamVR's next start. After
//! SteamVR crashes it starts in Safe Mode, which turns add-on drivers like this one off until the person turns them
//! on again (SteamVR, Settings, Startup / Shutdown, Manage Add-ons).

use std::sync::atomic::{fence, AtomicU32, AtomicU64, AtomicU8, Ordering};

mod control;
#[cfg(feature = "install")]
pub mod install;
#[doc(hidden)]
#[cfg(windows)]
pub mod win;

pub use control::{Control, Mapping};
#[cfg(feature = "install")]
pub use install::{install, installed, uninstall, AddOnState, Installation};

/// The named memory the app creates and the driver opens (this Windows session's namespace).
pub const CONTROL_NAME: &str = r"Local\OrbitersSmoothTracking";
/// [`ControlBlock::magic`], once the block is set up: "OSMT".
pub const MAGIC: u32 = u32::from_le_bytes(*b"OSMT");
/// The layout's version. The driver smooths nothing with a block of another version.
pub const VERSION: u32 = 1;
/// SteamVR's device indices (`k_unMaxTrackedDeviceCount`).
pub const MAX_DEVICES: usize = 64;
/// The driver counts as running while its heartbeat is at most this old.
pub const HEARTBEAT_TIMEOUT_MS: u64 = 3000;

/// The memory shared by the app and the driver. Every field is an atomic: two processes read and write it at once.
///
/// The app writes the first part (`magic` last, once the rest is set up); the settings (`enabled`, `strengths`) are
/// written inside the `sequence` counter, odd while a write is under way, so a reader takes all of them from one
/// write. The driver writes the `driver_*` fields, on cache lines of their own.
#[repr(C, align(64))]
pub struct ControlBlock {
    /// [`MAGIC`] once set up.
    pub magic: AtomicU32,
    /// [`VERSION`].
    pub version: AtomicU32,
    /// `size_of::<ControlBlock>()`.
    pub size: AtomicU32,
    /// The process whose exit ends smoothing (the app that last opened the block).
    pub owner_pid: AtomicU32,
    /// Odd while the app writes the settings.
    pub sequence: AtomicU32,
    /// Smoothing on (1) or off (0).
    pub enabled: AtomicU32,
    _app_reserved: [AtomicU32; 10],
    /// Each device index's strength, `f32` bits in `0.0..=1.0`.
    pub strengths: [AtomicU32; MAX_DEVICES],
    /// Bumped by the driver on each of SteamVR's frames.
    pub driver_heartbeat: AtomicU64,
    /// `GetTickCount64()` (milliseconds since Windows started, the same in every process) at the last heartbeat.
    pub driver_tick_ms: AtomicU64,
    /// Pose updates the driver has seen.
    pub driver_poses: AtomicU64,
    /// Pose updates it smoothed.
    pub driver_smoothed: AtomicU64,
    /// The layout version the driver was built with.
    pub driver_protocol: AtomicU32,
    /// SteamVR host interfaces whose pose updates it filters; 0: loaded but nothing hooked.
    pub driver_hooks: AtomicU32,
    /// `vrserver.exe`'s process.
    pub driver_pid: AtomicU32,
    _driver_reserved: AtomicU32,
    /// The driver's version (its crate version), UTF-8, NUL padded.
    pub driver_version: [AtomicU8; 32],
}

const _: () = assert!(std::mem::size_of::<ControlBlock>() == 448);

/// What the app asks for, as the driver reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub strengths: [f32; MAX_DEVICES],
}

/// What the driver reports about itself.
#[derive(Clone, Copy, Debug)]
pub struct DriverReport<'a> {
    pub version: &'a str,
    pub hooks: u32,
    pub pid: u32,
    pub poses: u64,
    pub smoothed: u64,
}

/// The driver, as the app sees it ([`Control::driver_heartbeat`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverStatus {
    /// The driver's version (its crate version).
    pub version: String,
    /// The layout version it was built with; it smooths nothing when this is not [`VERSION`].
    pub protocol: u32,
    /// SteamVR host interfaces it filters; 0 means SteamVR loaded it but it could not hook anything.
    pub hooks: u32,
    /// `vrserver.exe`'s process.
    pub pid: u32,
    /// Its heartbeat counter.
    pub heartbeat: u64,
    /// Pose updates it has seen since SteamVR started.
    pub poses: u64,
    /// Pose updates it smoothed.
    pub smoothed: u64,
}

/// A strength as stored: in `0.0..=1.0`, anything else (NaN included) clamped, NaN to 0.
pub fn sanitize_strength(s: f32) -> f32 {
    if s.is_nan() {
        0.0
    } else {
        s.clamp(0.0, 1.0)
    }
}

impl ControlBlock {
    /// Set up and of this layout: the only blocks the driver uses.
    pub fn is_valid(&self) -> bool {
        self.magic.load(Ordering::Acquire) == MAGIC
            && self.version.load(Ordering::Relaxed) == VERSION
            && self.size.load(Ordering::Relaxed) as usize == std::mem::size_of::<Self>()
    }

    /// The settings from one write, or `None` while the app is writing them (a few tries).
    pub fn settings(&self) -> Option<Settings> {
        for _ in 0..8 {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let enabled = self.enabled.load(Ordering::Relaxed) != 0;
            let mut strengths = [0.0f32; MAX_DEVICES];
            for (s, bits) in strengths.iter_mut().zip(&self.strengths) {
                *s = sanitize_strength(f32::from_bits(bits.load(Ordering::Relaxed)));
            }
            fence(Ordering::Acquire);
            if self.sequence.load(Ordering::Relaxed) == before {
                return Some(Settings { enabled, strengths });
            }
        }
        None
    }

    /// Writes settings inside the sequence counter. A counter that stays odd for 100 ms belongs to a writer that died
    /// mid-write: this one takes over from it.
    fn write_settings(&self, write: impl FnOnce(&Self)) {
        let since = std::time::Instant::now();
        let start = loop {
            let s = self.sequence.load(Ordering::Relaxed);
            if s & 1 == 0 {
                if self.sequence.compare_exchange_weak(s, s.wrapping_add(1), Ordering::Acquire, Ordering::Relaxed).is_ok() {
                    break s;
                }
            } else if since.elapsed() > std::time::Duration::from_millis(100) {
                // Keep it odd (s + 2) while this write runs, then even.
                if self.sequence.compare_exchange(s, s.wrapping_add(2), Ordering::Acquire, Ordering::Relaxed).is_ok() {
                    break s.wrapping_add(1);
                }
            } else {
                std::thread::yield_now();
            }
        };
        fence(Ordering::Release);
        write(self);
        self.sequence.store(start.wrapping_add(2), Ordering::Release);
    }

    /// The driver's heartbeat, with what it reports. `tick_ms` is `GetTickCount64()`.
    pub fn report_driver(&self, report: &DriverReport, tick_ms: u64) {
        self.driver_protocol.store(VERSION, Ordering::Relaxed);
        self.driver_hooks.store(report.hooks, Ordering::Relaxed);
        self.driver_pid.store(report.pid, Ordering::Relaxed);
        self.driver_poses.store(report.poses, Ordering::Relaxed);
        self.driver_smoothed.store(report.smoothed, Ordering::Relaxed);
        let bytes = report.version.as_bytes();
        for (i, cell) in self.driver_version.iter().enumerate() {
            // The last byte stays NUL.
            let b = if i + 1 < self.driver_version.len() { bytes.get(i).copied().unwrap_or(0) } else { 0 };
            if cell.load(Ordering::Relaxed) != b {
                cell.store(b, Ordering::Relaxed);
            }
        }
        self.driver_heartbeat.fetch_add(1, Ordering::Relaxed);
        self.driver_tick_ms.store(tick_ms, Ordering::Release);
    }

    /// The driver, when its heartbeat is at most [`HEARTBEAT_TIMEOUT_MS`] old at `now_tick_ms` (`GetTickCount64()`).
    pub fn driver_status(&self, now_tick_ms: u64) -> Option<DriverStatus> {
        let tick = self.driver_tick_ms.load(Ordering::Acquire);
        if tick == 0 || now_tick_ms.saturating_sub(tick) > HEARTBEAT_TIMEOUT_MS {
            return None;
        }
        let bytes: Vec<u8> = self.driver_version.iter().map(|b| b.load(Ordering::Relaxed)).take_while(|&b| b != 0).collect();
        Some(DriverStatus {
            version: String::from_utf8_lossy(&bytes).into_owned(),
            protocol: self.driver_protocol.load(Ordering::Relaxed),
            hooks: self.driver_hooks.load(Ordering::Relaxed),
            pid: self.driver_pid.load(Ordering::Relaxed),
            heartbeat: self.driver_heartbeat.load(Ordering::Relaxed),
            poses: self.driver_poses.load(Ordering::Relaxed),
            smoothed: self.driver_smoothed.load(Ordering::Relaxed),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    #[test]
    fn layout_is_fixed() {
        assert_eq!(std::mem::align_of::<ControlBlock>(), 64);
        assert_eq!(offset_of!(ControlBlock, magic), 0);
        assert_eq!(offset_of!(ControlBlock, version), 4);
        assert_eq!(offset_of!(ControlBlock, size), 8);
        assert_eq!(offset_of!(ControlBlock, owner_pid), 12);
        assert_eq!(offset_of!(ControlBlock, sequence), 16);
        assert_eq!(offset_of!(ControlBlock, enabled), 20);
        assert_eq!(offset_of!(ControlBlock, strengths), 64);
        assert_eq!(offset_of!(ControlBlock, driver_heartbeat), 320);
        assert_eq!(offset_of!(ControlBlock, driver_tick_ms), 328);
        assert_eq!(offset_of!(ControlBlock, driver_poses), 336);
        assert_eq!(offset_of!(ControlBlock, driver_smoothed), 344);
        assert_eq!(offset_of!(ControlBlock, driver_protocol), 352);
        assert_eq!(offset_of!(ControlBlock, driver_hooks), 356);
        assert_eq!(offset_of!(ControlBlock, driver_pid), 360);
        assert_eq!(offset_of!(ControlBlock, driver_version), 368);
    }

    #[test]
    fn strengths_are_sanitized() {
        assert_eq!(sanitize_strength(f32::NAN), 0.0);
        assert_eq!(sanitize_strength(-1.0), 0.0);
        assert_eq!(sanitize_strength(2.0), 1.0);
        assert_eq!(sanitize_strength(0.25), 0.25);
        assert_eq!(sanitize_strength(f32::INFINITY), 1.0);
    }
}
