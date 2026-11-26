//! A word when something changes in a folder, from the system, so that nobody looks again and again at files that stay as
//! they are (review 1124, XCT-23). On Windows (`FindFirstChangeNotification`): one sleeping thread for all the folders given,
//! each also followed from the folder above it, so that one moved away or made again is followed where it is now.
//! Elsewhere, and where Windows gives no notice (a folder on some network drives), the caller looks at its own pace, as before:
//! [`watch`] says so.

use std::path::PathBuf;

/// Calls `changed` whenever a file or folder is made, written, renamed or removed in one of `folders`, for as long as the
/// process runs; a folder that is not there yet is waited for, and followed once it is, and one moved away, removed or
/// replaced by another of its name is followed at the one at its path (told then too). It can come several times for one
/// change, and for a change that was undone. False when the system gives no such notice here: the caller looks by itself.
pub fn watch(name: &str, folders: Vec<PathBuf>, changed: impl Fn() + Send + 'static) -> bool {
    watch_as(name, folders, false, changed)
}

/// [`watch`], with everything under the folders too (VRChat's pictures go in a folder per month).
pub fn watch_tree(name: &str, folders: Vec<PathBuf>, changed: impl Fn() + Send + 'static) -> bool {
    watch_as(name, folders, true, changed)
}

fn watch_as(name: &str, folders: Vec<PathBuf>, tree: bool, changed: impl Fn() + Send + 'static) -> bool {
    #[cfg(windows)]
    {
        windows_watch::watch(name, folders, tree, changed)
    }
    #[cfg(not(windows))]
    {
        let _ = (name, folders, tree, changed);
        false
    }
}

#[cfg(windows)]
mod windows_watch {
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_NOTIFY_CHANGE, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Threading::{WaitForMultipleObjects, INFINITE};

    /// A folder followed: the one asked for, or the nearest one above it while it is not there (only for the folders made in it).
    /// One that is there is followed from the folder above it too (the names of the folders there), with which folder it is:
    /// its notice stays with the folder it was opened on, so one moved away or replaced by another of its name is followed
    /// anew, at the one there now (review 1124, W23-verify M1).
    struct Watched {
        wanted: PathBuf,
        there: bool,
        handle: HANDLE,
        above: Option<(Notice, Option<Identity>)>,
    }

    /// A change notification's handle, closed with it.
    struct Notice(HANDLE);

    // SAFETY: a change notification's handle belongs to the process, not to the thread that opened it.
    unsafe impl Send for Notice {}
    unsafe impl Send for Watched {}

    impl Drop for Notice {
        fn drop(&mut self) {
            // SAFETY: opened by `notice`, closed once.
            let _ = unsafe { FindCloseChangeNotification(self.0) };
        }
    }

    impl Drop for Watched {
        fn drop(&mut self) {
            // SAFETY: opened by `open`, closed once.
            let _ = unsafe { FindCloseChangeNotification(self.handle) };
        }
    }

    /// Which folder a path is now: its volume and its number there (another folder made under the same name has another).
    type Identity = (u32, u64);

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain([0]).collect()
    }

    fn identity(path: &Path) -> Option<Identity> {
        let path = wide(path);
        // SAFETY: a path ending in zero, alive for the call; asks for no access (attributes only), shares everything.
        let file = unsafe { CreateFileW(PCWSTR(path.as_ptr()), 0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, None) }.ok()?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: the handle just opened, closed right after.
        let read = unsafe { GetFileInformationByHandle(file, &mut info) };
        let _ = unsafe { CloseHandle(file) };
        read.ok()?;
        Some((info.dwVolumeSerialNumber, (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)))
    }

    fn notice(folder: &Path, what: FILE_NOTIFY_CHANGE, tree: bool) -> Option<HANDLE> {
        let path = wide(folder);
        // SAFETY: a path ending in zero, alive for the call.
        unsafe { FindFirstChangeNotificationW(PCWSTR(path.as_ptr()), tree, what) }.ok()
    }

    /// The notices of `wanted` (with what is under it when `tree`; never under a folder above it waited in).
    fn open(wanted: &Path, tree: bool) -> Option<Watched> {
        let folder = wanted.ancestors().find(|f| f.is_dir())?;
        let there = folder == wanted;
        // The folder above first: a folder put in its place from now on is told there, even before its own notice is open.
        let above = there.then(|| wanted.parent().and_then(|p| notice(p, FILE_NOTIFY_CHANGE_DIR_NAME, false))).flatten().map(|h| (Notice(h), identity(wanted)));
        let what = if there { FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_SIZE } else { FILE_NOTIFY_CHANGE_DIR_NAME };
        let handle = notice(folder, what, tree && there)?;
        Some(Watched { wanted: wanted.to_path_buf(), there, handle, above })
    }

    pub fn watch(name: &str, folders: Vec<PathBuf>, tree: bool, changed: impl Fn() + Send + 'static) -> bool {
        let opened: Option<Vec<Watched>> = folders.iter().map(|f| open(f, tree)).collect();
        // (Two notices each at most: what one wait can hold.)
        let Some(mut watched) = opened.filter(|w| !w.is_empty() && w.len() <= 32) else { return false };
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || loop {
                // Each folder's own notice, then the one of the folder above it.
                let handles: Vec<(usize, bool, HANDLE)> =
                    watched.iter().enumerate().flat_map(|(i, w)| std::iter::once((i, false, w.handle)).chain(w.above.as_ref().map(|(n, _)| (i, true, n.0)))).collect();
                let only: Vec<HANDLE> = handles.iter().map(|h| h.2).collect();
                // SAFETY: handles that stay open for the wait.
                let woken = unsafe { WaitForMultipleObjects(&only, false, INFINITE) };
                let k = woken.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
                let signalled = (woken != WAIT_FAILED).then(|| handles.get(k).copied()).flatten();
                let followed = match signalled {
                    // A folder's name changed beside it: nothing of its own while it is still the folder at its path.
                    Some((i, true, handle)) => {
                        let w = &watched[i];
                        let same = w.above.as_ref().is_some_and(|(_, id)| id.is_some() && identity(&w.wanted) == *id);
                        // SAFETY: the handle that was signalled, still open.
                        if same && unsafe { FindNextChangeNotification(handle) }.is_ok() {
                            continue;
                        }
                        false
                    }
                    // One followed from above is followed anew at each change there: a folder made below may be the next one down.
                    // SAFETY: the handle that was signalled, still open.
                    Some((i, false, handle)) => watched[i].there && watched[i].wanted.is_dir() && unsafe { FindNextChangeNotification(handle) }.is_ok(),
                    None => false,
                };
                if !followed {
                    // It came, it went, it was replaced, or the one above it did: followed anew, at the folder there now.
                    let wanted: Vec<PathBuf> = watched.iter().map(|w| w.wanted.clone()).collect();
                    watched.clear();
                    loop {
                        match wanted.iter().map(|f| open(f, tree)).collect::<Option<Vec<Watched>>>() {
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

    /// DPG-23: VRChat's pictures go in a folder per month under the folder followed.
    #[test]
    fn a_tree_is_followed_below_its_folder() {
        let root = std::env::temp_dir().join(format!("orbiters-watch-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let tree = root.join("tree");
        std::fs::create_dir_all(tree.join("2026-11")).unwrap();
        let (tx, rx) = mpsc::channel();
        assert!(super::watch_tree("watch-tree-test", vec![tree.clone()], move || {
            let _ = tx.send(());
        }));
        std::fs::write(tree.join("2026-11").join("picture.png"), "one").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok(), "a picture in a month's folder");
        while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
        std::fs::create_dir_all(tree.join("2026-12")).unwrap();
        while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
        std::fs::write(tree.join("2026-12").join("picture.png"), "two").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok(), "in a month's folder made later");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_replaced_by_another_of_its_name_is_followed_at_the_new_one() {
        let root = std::env::temp_dir().join(format!("orbiters-watch-replaced-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let requests = root.join("requests");
        std::fs::create_dir_all(&requests).unwrap();
        let (tx, rx) = mpsc::channel();
        assert!(super::watch("watch-replaced-test", vec![requests.clone()], move || {
            let _ = tx.send(());
        }));
        let drain = || while rx.recv_timeout(Duration::from_millis(300)).is_ok() {};
        std::fs::write(requests.join("first"), "one").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok(), "the folder is followed");
        drain();
        // Moved away and made again (W23-verify M1): what comes in the new one is told, and what the old one gets is not.
        std::fs::rename(&requests, root.join("old")).unwrap();
        std::fs::create_dir(&requests).unwrap();
        drain();
        std::fs::write(requests.join("second"), "two").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok(), "a file made in the folder that replaced it");
        drain();
        std::fs::write(root.join("old").join("third"), "three").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(500)).is_err(), "the folder moved away is no longer this one");
        // Removed, then made again.
        std::fs::remove_dir_all(&requests).unwrap();
        drain();
        std::fs::create_dir(&requests).unwrap();
        drain();
        std::fs::write(requests.join("fourth"), "four").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok(), "a file made in the folder made again");
        let _ = std::fs::remove_dir_all(&root);
    }
}
