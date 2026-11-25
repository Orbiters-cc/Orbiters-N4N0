//! A disk's free space, and what it is.
use std::path::Path;

/// Free bytes, for this person, on the disk holding `dir` (which must exist); `None` when the system would not say. One question
/// for that folder, not a list of every volume.
pub fn free_space(dir: &Path) -> Option<u64> {
    imp::free_space(dir)
}

/// What a disk is on, as far as the system says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Device {
    /// The same for two disks of one device.
    pub number: Option<u32>,
    /// Whether reading here and there costs it time (a hard disk).
    pub seeks: Option<bool>,
}

/// What the disk whose root is `root` (`C:\\`) is on: asked of it, which needs no right on it. Nothing is said off Windows.
pub fn device(root: &Path) -> Device {
    #[cfg(windows)]
    return imp::device(root);
    #[cfg(not(windows))]
    {
        let _ = root;
        Device::default()
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
        fn DeviceIoControl(
            device: *mut std::ffi::c_void,
            code: u32,
            given: *const u32,
            given_size: u32,
            answer: *mut u32,
            answer_size: u32,
            returned: *mut u32,
            overlapped: *mut std::ffi::c_void,
        ) -> i32;
    }

    pub fn device(root: &std::path::Path) -> super::Device {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        const IOCTL_STORAGE_GET_DEVICE_NUMBER: u32 = 0x002D_1080;
        const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
        const SEEK_PENALTY: u32 = 7;
        let name = root.to_string_lossy();
        let Some(letter) = name.strip_suffix('\\').filter(|n| n.len() == 2 && n.ends_with(':')) else { return Default::default() };
        // Opened to be asked about alone.
        let Ok(volume) = std::fs::OpenOptions::new().access_mode(0).share_mode(7).open(format!(r"\\.\{letter}")) else { return Default::default() };
        let ask = |code: u32, given: &[u32], answer: &mut [u32]| {
            let mut got = 0u32;
            let given_at = if given.is_empty() { std::ptr::null() } else { given.as_ptr() };
            // SAFETY: both buffers are as long as they are said to be, and the handle is open for the call.
            unsafe {
                DeviceIoControl(
                    volume.as_raw_handle().cast(),
                    code,
                    given_at,
                    std::mem::size_of_val(given) as u32,
                    answer.as_mut_ptr(),
                    std::mem::size_of_val(answer) as u32,
                    &mut got,
                    std::ptr::null_mut(),
                ) != 0
            }
        };
        // STORAGE_DEVICE_NUMBER: type, number, partition. STORAGE_PROPERTY_QUERY: property, query, a byte more;
        // DEVICE_SEEK_PENALTY_DESCRIPTOR: version, size, whether it seeks.
        let (mut number, mut seeks) = ([0u32; 3], [0u32; 3]);
        super::Device {
            number: ask(IOCTL_STORAGE_GET_DEVICE_NUMBER, &[], &mut number).then_some(number[1]),
            seeks: ask(IOCTL_STORAGE_QUERY_PROPERTY, &[SEEK_PENALTY, 0, 0], &mut seeks).then_some(seeks[2] & 0xFF != 0),
        }
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
