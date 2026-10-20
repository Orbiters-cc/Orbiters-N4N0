//! The capture host's monotonic clock, shared by every process of the app: nanoseconds read from
//! `QueryPerformanceCounter` (Windows) or `CLOCK_MONOTONIC` (Linux, macOS). A session time is the difference of
//! two readings, so streams written by different processes (frames, OSC, SteamVR, audio) line up without any
//! correction (contracts `Docs/performance.md`).

#[cfg(windows)]
pub fn now_ns() -> u64 {
    use std::sync::OnceLock;
    #[link(name = "kernel32")]
    extern "system" {
        fn QueryPerformanceCounter(count: *mut i64) -> i32;
        fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
    }
    static FREQUENCY: OnceLock<i64> = OnceLock::new();
    let frequency = *FREQUENCY.get_or_init(|| {
        let mut f = 0;
        // SAFETY: plain out-parameter; the call cannot fail on Windows XP and later.
        unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1)
    });
    let mut count = 0;
    // SAFETY: as above.
    unsafe { QueryPerformanceCounter(&mut count) };
    (count as i128 * 1_000_000_000 / frequency as i128) as u64
}

#[cfg(unix)]
pub fn now_ns() -> u64 {
    #[repr(C)]
    struct Timespec {
        tv_sec: i64,
        tv_nsec: i64,
    }
    extern "C" {
        fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
    }
    const CLOCK_MONOTONIC: i32 = if cfg!(target_os = "macos") { 6 } else { 1 };
    let mut ts = Timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: plain out-parameter of the C library.
    unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// A QPC reading in 100 ns units (what WASAPI reports for audio packets) on the same scale as [`now_ns`].
pub fn from_qpc_100ns(value: u64) -> u64 {
    value.saturating_mul(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_and_in_nanoseconds() {
        let a = now_ns();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = now_ns();
        assert!(b > a);
        let ms = (b - a) as f64 / 1e6;
        assert!((15.0..500.0).contains(&ms), "{ms} ms");
    }
}
