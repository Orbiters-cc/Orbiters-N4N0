//! A disk's free space.
use std::path::Path;

/// Free bytes, for this person, on the disk holding `dir` (which must exist); `None` when the system would not say. One question
/// for that folder, not a list of every volume.
pub fn free_space(dir: &Path) -> Option<u64> {
    imp::free_space(dir)
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }

    pub fn free_space(dir: &std::path::Path) -> Option<u64> {
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
        let mut available = 0u64;
        // SAFETY: a NUL-terminated path and an out-parameter; the totals are not asked for.
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
        (ok != 0).then_some(available)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::os::unix::ffi::OsStrExt;

    pub fn free_space(dir: &std::path::Path) -> Option<u64> {
        let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        // SAFETY: plain integers, all of them valid as zero.
        let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: a NUL-terminated path and an out-parameter.
        let failed = unsafe { libc::statfs(path.as_ptr(), &mut stats) };
        (failed == 0).then(|| stats.f_bavail * u64::from(stats.f_bsize))
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::os::unix::ffi::OsStrExt;

    pub fn free_space(dir: &std::path::Path) -> Option<u64> {
        let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
        // SAFETY: plain integers, all of them valid as zero.
        let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: a NUL-terminated path and an out-parameter.
        let failed = unsafe { libc::statvfs(path.as_ptr(), &mut stats) };
        // (`f_bavail`: what is left to a person, not to root; counted in fragments.)
        (failed == 0).then(|| (stats.f_bavail as u64).saturating_mul(stats.f_frsize as u64))
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    pub fn free_space(_dir: &std::path::Path) -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    fn a_disk_says_how_much_room_it_has() {
        assert!(super::free_space(&std::env::temp_dir()).is_some_and(|free| free > 0));
        assert_eq!(super::free_space(&std::env::temp_dir().join("orbiters-no-such-folder").join("x")), None);
    }
}
