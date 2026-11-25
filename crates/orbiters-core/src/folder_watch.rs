//! A word when something changes in a folder, from the system, so that nobody looks again and again at files that stay as
//! they are (review 1124, XCT-23). On Windows (`FindFirstChangeNotification`): one sleeping thread for all the folders given.
//! Elsewhere, and where Windows gives no notice (a folder on some network drives), the caller looks at its own pace, as before:
//! [`watch`] says so.

use std::path::PathBuf;

/// Calls `changed` whenever a file or folder is made, written, renamed or removed in one of `folders`, for as long as the
/// process runs; a folder that is not there yet is waited for, and followed once it is. It can come several times for one
/// change, and for a change that was undone. False when the system gives no such notice here: the caller looks by itself.
pub fn watch(name: &str, folders: Vec<PathBuf>, changed: impl Fn() + Send + 'static) -> bool {
    #[cfg(windows)]
    {
        windows_watch::watch(name, folders, changed)
    }
    #[cfg(not(windows))]
    {
        let _ = (name, folders, changed);
        false
    }
}

#[cfg(windows)]
mod windows_watch {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HANDLE, WAIT_FAILED, WAIT_OBJECT_0};
    use windows::Win32::Storage::FileSystem::{
        FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
        FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
    };
    use windows::Win32::System::Threading::{WaitForMultipleObjects, INFINITE};

    /// A folder followed: the one asked for, or the nearest one above it while it is not there (only for the folders made in it).
    struct Watched {
        wanted: PathBuf,
        there: bool,
        handle: HANDLE,
    }

    // SAFETY: a change notification's handle belongs to the process, not to the thread that opened it.
    unsafe impl Send for Watched {}

    impl Drop for Watched {
        fn drop(&mut self) {
            // SAFETY: opened by `open`, closed once.
            let _ = unsafe { FindCloseChangeNotification(self.handle) };
        }
    }

    fn open(wanted: &Path) -> Option<Watched> {
        let folder = wanted.ancestors().find(|f| f.is_dir())?;
        let there = folder == wanted;
        let what = if there { FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE } else { FILE_NOTIFY_CHANGE_DIR_NAME };
        let path: Vec<u16> = folder.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: a path ending in zero, alive for the call.
        let handle = unsafe { FindFirstChangeNotificationW(PCWSTR(path.as_ptr()), false, what) }.ok()?;
        Some(Watched { wanted: wanted.to_path_buf(), there, handle })
    }

    pub fn watch(name: &str, folders: Vec<PathBuf>, changed: impl Fn() + Send + 'static) -> bool {
        let opened: Option<Vec<Watched>> = folders.iter().map(|f| open(f)).collect();
        let Some(mut watched) = opened.filter(|w| !w.is_empty() && w.len() <= 64) else { return false };
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || loop {
                let handles: Vec<HANDLE> = watched.iter().map(|w| w.handle).collect();
                // SAFETY: handles that stay open for the wait.
                let woken = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                let i = woken.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
                let again = |i: usize| watched[i].there == watched[i].wanted.is_dir();
                // SAFETY: the handle that was signalled, still open.
                let followed = woken != WAIT_FAILED && i < watched.len() && again(i) && unsafe { FindNextChangeNotification(watched[i].handle) }.is_ok();
                if !followed {
                    // It came, it went, or the one above it did: followed anew, at the folder there now.
                    let wanted: Vec<PathBuf> = watched.iter().map(|w| w.wanted.clone()).collect();
                    watched.clear();
                    loop {
                        match wanted.iter().map(|f| open(f)).collect::<Option<Vec<Watched>>>() {
                            Some(opened) => {
                                watched = opened;
                                break;
                            }
                            // (a drive taken out: looked for now and then)
                            None => std::thread::sleep(Duration::from_secs(5)),
                        }
                    }
                }
                changed();
            })
            .is_ok()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn what_changes_in_a_folder_is_told_and_nothing_else_is() {
        let root = std::env::temp_dir().join(format!("orbiters-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (a, b) = (root.join("a"), root.join("later").join("b"));
        std::fs::create_dir_all(&a).unwrap();
        let (tx, rx) = mpsc::channel();
        let told = move || {
            let _ = tx.send(());
        };
        assert!(super::watch("watch-test", vec![a.clone(), b.clone()], told));
        let told = || {
            let first = rx.recv_timeout(Duration::from_secs(10)).is_ok();
            // (one change can be told more than once)
            while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}
            first
        };
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "nothing changed");
        std::fs::write(a.join("file"), "one").unwrap();
        assert!(told(), "a file made");
        std::fs::write(a.join("file"), "two!").unwrap();
        assert!(told(), "written again in place");
        std::fs::rename(a.join("file"), a.join("other")).unwrap();
        assert!(told(), "renamed");
        std::fs::remove_file(a.join("other")).unwrap();
        assert!(told(), "removed");
        // A folder that was not there: followed once it is.
        std::fs::create_dir_all(&b).unwrap();
        assert!(told(), "the folders above it were made");
        std::fs::write(b.join("file"), "one").unwrap();
        assert!(told(), "in the folder that came later");
        std::fs::write(root.join("beside"), "x").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "what is beside them is not theirs");
        let _ = std::fs::remove_dir_all(&root);
    }
}
