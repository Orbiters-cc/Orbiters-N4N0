//! The control block in named shared memory: [`Mapping`] (both sides) and [`Control`] (the app).

use crate::{sanitize_strength, ControlBlock, DriverStatus, Settings, WalkNow, CONTROL_NAME, MAGIC, MAX_DEVICES, VERSION};
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

/// A view of a [`ControlBlock`] in named shared memory; unmapped and closed on drop.
pub struct Mapping {
    #[cfg(windows)]
    handle: crate::win::HANDLE,
    block: *const ControlBlock,
    created: bool,
}

// SAFETY: the view stays mapped until drop and the block is made of atomics only.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    /// Creates the named block (zeroed) or, when it exists, opens it; [`Mapping::created`] tells which.
    pub fn create(name: &str) -> io::Result<Mapping> {
        #[cfg(windows)]
        {
            use crate::win::*;
            let wide_name = wide(name);
            let size = std::mem::size_of::<ControlBlock>() as u32;
            // SAFETY: a page-file backed mapping (no file handle), default security, NUL-terminated name.
            let handle = unsafe { CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE, 0, size, wide_name.as_ptr()) };
            if handle == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: right after the call that set it.
            let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            Self::map(handle, !existed)
        }
        #[cfg(not(windows))]
        {
            let _ = name;
            Err(io::Error::new(io::ErrorKind::Unsupported, "tracking smoothing needs Windows"))
        }
    }

    /// Opens an existing block for reading and writing (the driver). `name` is NUL-terminated UTF-16
    /// (`win::wide`), made once so that retrying allocates nothing.
    pub fn open(name: &[u16]) -> io::Result<Mapping> {
        if name.last() != Some(&0) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "the name must end with a NUL"));
        }
        #[cfg(windows)]
        {
            use crate::win::*;
            // SAFETY: NUL-terminated name (checked above).
            let handle = unsafe { OpenFileMappingW(FILE_MAP_READ | FILE_MAP_WRITE, 0, name.as_ptr()) };
            if handle == 0 {
                return Err(io::Error::last_os_error());
            }
            Self::map(handle, false)
        }
        #[cfg(not(windows))]
        {
            Err(io::Error::new(io::ErrorKind::Unsupported, "tracking smoothing needs Windows"))
        }
    }

    #[cfg(windows)]
    fn map(handle: crate::win::HANDLE, created: bool) -> io::Result<Mapping> {
        use crate::win::*;
        let close = |e: io::Error| {
            // SAFETY: the handle opened by the caller, closed once here on failure.
            unsafe { CloseHandle(handle) };
            e
        };
        // SAFETY: a valid mapping handle; the whole mapping is mapped.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, 0) };
        if view.is_null() {
            return Err(close(io::Error::last_os_error()));
        }
        let mut info = MEMORY_BASIC_INFORMATION::zeroed();
        // SAFETY: querying the view just mapped.
        let ok = unsafe { VirtualQuery(view, &mut info, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) } != 0;
        if !ok || info.RegionSize < std::mem::size_of::<ControlBlock>() {
            // SAFETY: the view mapped above.
            unsafe { UnmapViewOfFile(view) };
            return Err(close(io::Error::new(io::ErrorKind::InvalidData, "the smoothing control block is smaller than this version's")));
        }
        // Views start on an allocation granularity boundary (64 KB): aligned for the block.
        Ok(Mapping { handle, block: view as *const ControlBlock, created })
    }

    /// The shared block.
    pub fn block(&self) -> &ControlBlock {
        // SAFETY: mapped (at least the block's size, checked in `map`) until drop.
        unsafe { &*self.block }
    }

    /// Whether this call created the block (it was zeroed then).
    pub fn created(&self) -> bool {
        self.created
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: the view and handle this mapping owns, released once.
        unsafe {
            crate::win::UnmapViewOfFile(self.block as *const _);
            crate::win::CloseHandle(self.handle);
        }
    }
}

/// The app's side of the control block: what smoothing does, and whether the driver runs.
///
/// The driver smooths only while the process that last opened the block runs; hold the `Control` in the process
/// that stays (the tray service), for as long as smoothing should work.
pub struct Control {
    mapping: Mapping,
    writer: Mutex<()>,
}

impl Control {
    /// Creates `Local\OrbitersSmoothTracking` (smoothing off, every strength 0), or opens it when another of the
    /// app's processes made it (keeping its settings), and makes this process its owner.
    pub fn open_or_create() -> io::Result<Control> {
        Self::open_or_create_named(CONTROL_NAME)
    }

    /// [`Control::open_or_create`] under another name (tests).
    pub fn open_or_create_named(name: &str) -> io::Result<Control> {
        let mapping = Mapping::create(name)?;
        let block = mapping.block();
        if block.magic.load(Ordering::Acquire) == MAGIC {
            if !block.is_valid() {
                return Err(io::Error::other(format!(
                    "the tracking smoothing control is in use by another version of Orbiters - N4N0 (layout {}, this one {VERSION})",
                    block.version.load(Ordering::Relaxed)
                )));
            }
        } else {
            // New (zeroed), or made by a process that exited before setting it up.
            block.version.store(VERSION, Ordering::Relaxed);
            block.size.store(std::mem::size_of::<ControlBlock>() as u32, Ordering::Relaxed);
            block.enabled.store(0, Ordering::Relaxed);
            for s in &block.strengths {
                s.store(0f32.to_bits(), Ordering::Relaxed);
            }
            let seq = block.sequence.load(Ordering::Relaxed);
            block.sequence.store(seq.wrapping_add(seq & 1), Ordering::Relaxed);
            block.magic.store(MAGIC, Ordering::Release);
        }
        #[cfg(windows)]
        block.owner_pid.store(crate::win::current_pid(), Ordering::Release);
        Ok(Control { mapping, writer: Mutex::new(()) })
    }

    /// The process whose exit ends smoothing (the one that last opened or claimed the block).
    pub fn owner_pid(&self) -> u32 {
        self.mapping.block().owner_pid.load(Ordering::Acquire)
    }

    /// Makes this process the block's owner again, should another one have opened it since (it ended, and the driver
    /// stopped smoothing with it). Returns whether it had to.
    pub fn claim(&self) -> bool {
        #[cfg(windows)]
        {
            let me = crate::win::current_pid();
            self.mapping.block().owner_pid.swap(me, Ordering::AcqRel) != me
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    fn write(&self, write: impl FnOnce(&ControlBlock)) {
        let _one_writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        self.mapping.block().write_settings(write);
    }

    /// Turns smoothing on or off for every device (the strengths stay).
    pub fn set_enabled(&self, on: bool) {
        self.write(|b| b.enabled.store(on as u32, Ordering::Relaxed));
    }

    /// Whether smoothing is on.
    pub fn enabled(&self) -> bool {
        self.mapping.block().enabled.load(Ordering::Relaxed) != 0
    }

    /// One device index's strength, `0.0` (untouched) to `1.0` (strongest), clamped. Indices past 63 are ignored.
    pub fn set_strength(&self, index: usize, strength: f32) {
        if index < MAX_DEVICES {
            self.write(|b| b.strengths[index].store(sanitize_strength(strength).to_bits(), Ordering::Relaxed));
        }
    }

    /// Every device index's strength at once (the driver sees all of them or none of them).
    pub fn set_all(&self, strengths: &[f32; MAX_DEVICES]) {
        self.write(|b| {
            for (cell, &s) in b.strengths.iter().zip(strengths) {
                cell.store(sanitize_strength(s).to_bits(), Ordering::Relaxed);
            }
        });
    }

    /// What the driver reads now.
    pub fn settings(&self) -> Settings {
        let _one_writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let block = self.mapping.block();
        let mut strengths = [0.0; MAX_DEVICES];
        for (s, cell) in strengths.iter_mut().zip(&block.strengths) {
            *s = f32::from_bits(cell.load(Ordering::Relaxed));
        }
        Settings { enabled: block.enabled.load(Ordering::Relaxed) != 0, strengths }
    }

    /// The driver, while it runs: its heartbeat moved in the last [`crate::HEARTBEAT_TIMEOUT_MS`]. `None` when SteamVR
    /// is not running, did not load the driver (not installed, installed after SteamVR started, or turned off by
    /// Safe Mode), or the driver is of a version that does not know this block.
    pub fn driver_heartbeat(&self) -> Option<DriverStatus> {
        #[cfg(windows)]
        {
            self.mapping.block().driver_status(crate::win::tick_ms())
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// The shared block itself.
    pub fn block(&self) -> &ControlBlock {
        self.mapping.block()
    }
}

/// The SteamVR overlay's side of the block: Fast walk's offset. The tray service owns the block and its settings; this
/// writes only the walk's fields, so that the two processes never meet.
pub struct WalkLink {
    mapping: Mapping,
}

impl WalkLink {
    /// Opens the block the service made, with nothing carried, this process its writer. An error until the service made
    /// it (or everywhere but Windows, where the walk stays with the play space).
    pub fn open() -> io::Result<WalkLink> {
        Self::open_named(CONTROL_NAME)
    }

    /// [`WalkLink::open`] under another name (tests).
    pub fn open_named(name: &str) -> io::Result<WalkLink> {
        #[cfg(windows)]
        {
            let mapping = Mapping::open(&crate::win::wide(name))?;
            if !mapping.block().is_valid() {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "the control block is not set up yet")); // i18n: not shown
            }
            let link = WalkLink { mapping };
            link.send(&WalkNow::default());
            link.mapping.block().walk_pid.store(crate::win::current_pid(), Ordering::Release);
            Ok(link)
        }
        #[cfg(not(windows))]
        {
            let _ = name;
            Err(io::Error::new(io::ErrorKind::Unsupported, "the driver needs Windows")) // i18n: not shown
        }
    }

    /// Fast walk may use the driver: it hooks SteamVR's poses from its next frame on, live.
    pub fn set_wanted(&self, on: bool) {
        self.mapping.block().walk_on.store(on as u32, Ordering::Release);
    }

    /// The driver now, as [`Control::driver_heartbeat`].
    pub fn driver(&self) -> Option<DriverStatus> {
        #[cfg(windows)]
        {
            self.mapping.block().driver_status(crate::win::tick_ms())
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// Where the walk is now, and how fast it goes on.
    pub fn send(&self, walk: &WalkNow) {
        #[cfg(windows)]
        self.mapping.block().write_walk(walk, crate::win::micros());
        #[cfg(not(windows))]
        let _ = walk;
    }
}

impl Drop for WalkLink {
    /// Carried nowhere any more. A process killed outside does not get here: the driver sees it gone.
    fn drop(&mut self) {
        self.send(&WalkNow::default());
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::{win, DriverReport};
    use std::sync::atomic::{AtomicBool, AtomicU32};
    use std::sync::Arc;

    fn unique_name(what: &str) -> String {
        static N: AtomicU32 = AtomicU32::new(0);
        format!(r"Local\OrbitersSmoothTest-{what}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
    }

    #[test]
    fn round_trip_through_a_second_view() {
        let name = unique_name("round-trip");
        let control = Control::open_or_create_named(&name).unwrap();
        let driver = Mapping::open(&win::wide(&name)).unwrap();
        assert!(!driver.created());
        let block = driver.block();
        assert!(block.is_valid());
        assert_eq!(block.owner_pid.load(Ordering::Relaxed), std::process::id());
        assert_eq!(block.settings(), Some(Settings { enabled: false, strengths: [0.0; MAX_DEVICES] }));

        control.set_enabled(true);
        control.set_strength(3, 0.5);
        control.set_strength(70, 1.0);
        control.set_strength(4, f32::NAN);
        control.set_strength(5, 7.0);
        let s = block.settings().unwrap();
        assert!(s.enabled);
        assert_eq!((s.strengths[3], s.strengths[4], s.strengths[5]), (0.5, 0.0, 1.0));
        assert_eq!(s, control.settings());

        let mut all = [0.25f32; MAX_DEVICES];
        all[0] = 0.0;
        all[63] = -3.0;
        control.set_all(&all);
        let s = block.settings().unwrap();
        assert_eq!(s.strengths[0], 0.0);
        assert_eq!(s.strengths[1], 0.25);
        assert_eq!(s.strengths[63], 0.0);
        control.set_enabled(false);
        assert!(!block.settings().unwrap().enabled);
        assert_eq!(block.settings().unwrap().strengths[1], 0.25, "turning off keeps the strengths");

        // A second app process keeps the settings and becomes the owner.
        let again = Control::open_or_create_named(&name).unwrap();
        assert_eq!(again.settings().strengths[1], 0.25);
    }

    #[test]
    fn the_app_claims_the_block_back() {
        let name = unique_name("claim");
        let control = Control::open_or_create_named(&name).unwrap();
        assert_eq!(control.owner_pid(), std::process::id());
        assert!(!control.claim(), "already its own");
        // Another process opened it (and, say, ended): the app takes it back on its next look.
        control.block().owner_pid.store(4242, Ordering::Release);
        assert!(control.claim());
        assert_eq!(control.owner_pid(), std::process::id());
    }

    #[test]
    fn heartbeat_reports_the_driver() {
        let name = unique_name("heartbeat");
        let control = Control::open_or_create_named(&name).unwrap();
        assert_eq!(control.driver_heartbeat(), None);
        let driver = Mapping::open(&win::wide(&name)).unwrap();
        let report = DriverReport { version: "1.2.3-test", hooks: 2, pid: 1234, walked: 0, poses: 10, smoothed: 7 };
        driver.block().report_driver(&report, win::tick_ms());
        let status = control.driver_heartbeat().unwrap();
        assert_eq!(status.version, "1.2.3-test");
        assert_eq!((status.protocol, status.hooks, status.pid, status.poses, status.smoothed, status.heartbeat), (VERSION, 2, 1234, 10, 7, 1));
        // A heartbeat older than the timeout is a driver that stopped.
        driver.block().report_driver(&report, win::tick_ms().saturating_sub(crate::HEARTBEAT_TIMEOUT_MS + 1000));
        assert_eq!(control.driver_heartbeat(), None);
        // A version too long for the block is cut, never unterminated.
        let long = "x".repeat(80);
        driver.block().report_driver(&DriverReport { version: &long, ..report }, win::tick_ms());
        assert_eq!(control.driver_heartbeat().unwrap().version.len(), 31);
    }

    #[test]
    fn open_fails_when_missing_and_accepts_only_this_layout() {
        let name = unique_name("missing");
        assert!(Mapping::open(&win::wide(&name)).is_err());
        assert!(Mapping::open(&[b'x' as u16]).is_err(), "names without a NUL are refused");

        let control = Control::open_or_create_named(&name).unwrap();
        let block = control.block();
        block.version.store(VERSION + 1, Ordering::Relaxed);
        assert!(!block.is_valid());
        assert!(Control::open_or_create_named(&name).is_err(), "another version's block is not taken over");
        block.version.store(VERSION, Ordering::Relaxed);
        assert!(block.is_valid());
    }

    #[test]
    fn readers_never_see_half_a_write() {
        let name = unique_name("seqlock");
        let control = Arc::new(Control::open_or_create_named(&name).unwrap());
        let reader = Mapping::open(&win::wide(&name)).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (control, stop) = (control.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut i = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    let v = (i % 100) as f32 / 100.0;
                    control.set_all(&[v; MAX_DEVICES]);
                    i += 1;
                }
                i
            })
        };
        let (mut seen, mut busy) = (0, 0);
        let until = std::time::Instant::now() + std::time::Duration::from_millis(300);
        while std::time::Instant::now() < until {
            match reader.block().settings() {
                Some(s) => {
                    assert!(s.strengths.iter().all(|&x| x == s.strengths[0]), "a torn read: {:?}", s.strengths);
                    seen += 1;
                }
                None => busy += 1,
            }
        }
        stop.store(true, Ordering::Relaxed);
        let writes = writer.join().unwrap();
        assert!(writes > 100 && seen > 100, "writes {writes}, reads {seen}, busy {busy}");
    }

    #[test]
    fn a_writer_that_died_mid_write_is_taken_over() {
        let name = unique_name("dead-writer");
        let control = Control::open_or_create_named(&name).unwrap();
        // As if a writer died with the counter odd.
        control.block().sequence.fetch_add(1, Ordering::Relaxed);
        assert_eq!(control.block().settings(), None);
        control.set_strength(0, 1.0);
        assert_eq!(control.block().sequence.load(Ordering::Relaxed) & 1, 0);
        assert_eq!(control.block().settings().unwrap().strengths[0], 1.0);
    }

    #[test]
    fn the_overlay_writes_the_walk_beside_what_the_service_owns() {
        let name = unique_name("walk");
        assert!(WalkLink::open_named(&name).is_err(), "before the service made the block");
        let control = Control::open_or_create_named(&name).unwrap();
        control.set_enabled(true);
        let link = WalkLink::open_named(&name).unwrap();
        assert_eq!(control.block().walk_pid.load(Ordering::Relaxed), std::process::id());
        link.set_wanted(true);
        assert_eq!(control.block().walk_on.load(Ordering::Relaxed), 1);
        let walk = WalkNow { offset: [1.0, 0.0, 2.0], velocity: [3.0, 0.0, 0.0] };
        link.send(&walk);
        assert_eq!(control.block().walk().unwrap().0, walk);
        assert!(control.settings().enabled, "the service's own stays");
        // Gone: nothing carried.
        drop(link);
        assert!(control.block().walk().unwrap().0.is_zero());
    }
}
