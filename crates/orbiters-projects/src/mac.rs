//! What the Mac asks for that Windows does not: volumes through `statfs`, application bundles and Launch Services, and
//! AppKit's view of the applications running. Nothing here needs a permission the system asks the person about:
//! `NSRunningApplication` and the window server's list of windows (without their titles, which do need one) are open to
//! any application, where AppleScript's System Events and the accessibility API would each put up a question.
use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// The startup disk's writable half, which holds `/Users`: the Finder and the person know it as `/`.
const DATA_VOLUME: &str = "/System/Volumes/Data";

fn c_path(path: &Path) -> Option<CString> {
    CString::new(path.as_os_str().as_bytes()).ok()
}

/// A mounted volume, as `statfs` names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// Where it is mounted: `/` for the startup disk (its data volume too), `/Volumes/<name>` for any other.
    pub mount: PathBuf,
    /// Its device (`/dev/disk3s5`).
    pub device: String,
    /// Its file system (`apfs`, `hfs`, `exfat`).
    pub kind: String,
    /// `statfs`' `f_flags`.
    pub flags: u32,
    /// What a program of this user may still write, in bytes.
    pub free: u64,
}

impl Volume {
    fn from_statfs(s: &libc::statfs) -> Self {
        let text = |c: &[libc::c_char]| unsafe { CStr::from_ptr(c.as_ptr()) }.to_string_lossy().into_owned();
        let mount = text(&s.f_mntonname);
        Self {
            mount: PathBuf::from(if mount == DATA_VOLUME { "/" } else { mount.as_str() }),
            device: text(&s.f_mntfromname),
            kind: text(&s.f_fstypename),
            flags: s.f_flags,
            free: s.f_bavail.saturating_mul(s.f_bsize as u64),
        }
    }
}

/// The volume that holds `path`, through links. `None` when `path` is not there.
pub fn volume_of(path: &Path) -> Option<Volume> {
    let path = c_path(path)?;
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid C string and a statfs to fill.
    (unsafe { libc::statfs(path.as_ptr(), &mut s) } == 0).then(|| Volume::from_statfs(&s))
}

/// The APFS container a device belongs to: `disk3` of `/dev/disk3s5` and `/dev/disk3s1s1`.
fn container(device: &str) -> Option<&str> {
    let name = device.strip_prefix("/dev/")?;
    let digits = name.strip_prefix("disk")?;
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    (end > 0).then(|| &name[..4 + end])
}

/// One name per pool of room: an APFS volume's container (its volumes share all its room), else the volume's mount.
pub fn room_key(v: &Volume) -> String {
    match (v.kind == "apfs").then(|| container(&v.device)).flatten() {
        Some(c) => c.to_string(),
        None => v.mount.display().to_string(),
    }
}

/// Whether moving files from one volume to the other frees nothing: the same volume, or two APFS volumes of a container,
/// which share all its room.
pub fn share_room(a: &Volume, b: &Volume) -> bool {
    a.mount == b.mount || (a.kind == "apfs" && b.kind == "apfs" && container(&a.device).is_some_and(|c| container(&b.device) == Some(c)))
}

/// Whether the Finder lists a mounted volume as a disk one can keep files on (and not a system volume hidden from it, one
/// mounted read-only such as an installer's image, or a network share).
fn browsable(mount: &Path, flags: u32) -> bool {
    let flag = |f: libc::c_int| flags & f as u32 != 0;
    mount.starts_with("/Volumes") && flag(libc::MNT_LOCAL) && !flag(libc::MNT_DONTBROWSE) && !flag(libc::MNT_RDONLY)
}

/// Whether `diskutil` calls the volume's disk removable: memory sticks, SD cards and disk images, what Windows does not
/// call a fixed disk either. An external SSD or hard disk is fixed. Asks nobody for anything.
fn removable(mount: &Path) -> bool {
    let Ok(out) = Command::new("/usr/sbin/diskutil").args(["info", "-plist"]).arg(mount).output() else { return true };
    let Ok(info) = plist::Value::from_reader(std::io::Cursor::new(out.stdout)) else { return true };
    info.as_dictionary().and_then(|d| d.get("RemovableMedia")).and_then(plist::Value::as_boolean).unwrap_or(true)
}

/// The startup disk, then the other local disks mounted: `/`, `/Volumes/<name>`.
pub fn fixed_disks() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from("/")];
    // SAFETY: a null buffer asks how many there are; the second call fills at most the room it is told of.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count <= 0 {
        return out;
    }
    let mut all: Vec<libc::statfs> = vec![unsafe { std::mem::zeroed() }; count as usize];
    let bytes = (all.len() * std::mem::size_of::<libc::statfs>()) as libc::c_int;
    let filled = unsafe { libc::getfsstat(all.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
    for s in all.iter().take(filled.max(0) as usize) {
        let volume = Volume::from_statfs(s);
        if browsable(&volume.mount, volume.flags) && !removable(&volume.mount) {
            out.push(volume.mount);
        }
    }
    out
}

/// The application bundle a program is in: `/Applications/X.app` of `/Applications/X.app/Contents/MacOS/X`.
pub fn enclosing_app(program: &Path) -> Option<PathBuf> {
    program.ancestors().find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"))).map(Path::to_path_buf)
}

/// The program of an application bundle, as its `Info.plist` names it, else as the bundle is named.
pub fn bundle_program(app: &Path) -> PathBuf {
    let named = plist::Value::from_file(app.join("Contents/Info.plist"))
        .ok()
        .and_then(|v| v.as_dictionary()?.get("CFBundleExecutable")?.as_string().map(str::to_string));
    let name = named.or_else(|| app.file_stem().map(|s| s.to_string_lossy().into_owned())).unwrap_or_default();
    app.join("Contents/MacOS").join(name)
}

/// `open`, whose programs the system starts and answers for itself. Its address is fixed: an app started from the Dock
/// has a short `PATH`.
pub fn open_command() -> Command {
    Command::new("/usr/bin/open")
}

/// A new instance of `app`, started by the system rather than as a child of this process. Unity then asks for its own
/// permissions (a child's questions are put in its parent's name), and `-n` makes it a new one when that bundle already
/// has another project open: `open` would otherwise only bring that one forward.
pub fn start_app(app: &Path) -> Command {
    let mut c = open_command();
    c.args(["-n", "-a"]).arg(app);
    c
}

/// Everything Spotlight knows matching `query`, in at most a few seconds. Empty when it is turned off or does not answer.
pub fn spotlight(query: &str) -> Vec<PathBuf> {
    let query = query.to_string();
    crate::within(std::time::Duration::from_secs(10), move || Command::new("/usr/bin/mdfind").arg("-0").arg(query).output().ok())
        .flatten()
        .map_or_else(Vec::new, |out| out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| PathBuf::from(std::ffi::OsStr::from_bytes(p))).collect())
}

/// The application the system opens `file` with. The file has to exist: the system asks nothing about a name alone.
pub fn default_app(file: &Path) -> Option<PathBuf> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSString, NSURL};
    let path = file.to_str()?;
    objc2::rc::autoreleasepool(|_| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(path));
        let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url)?;
        app.path().map(|p| PathBuf::from(p.to_string()))
    })
}

fn running_app(pid: u32) -> Option<objc2::rc::Retained<objc2_app_kit::NSRunningApplication>> {
    objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid as libc::pid_t)
}

/// Brings the application `pid` to the front, with all its windows. `false` when the system knows no such application or
/// declined: an application in the background may not take the front from another.
pub fn activate(pid: u32) -> bool {
    objc2::rc::autoreleasepool(|_| running_app(pid).is_some_and(|a| a.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions::ActivateAllWindows)))
}

/// Asks the application `pid` to quit, as Cmd+Q and the Dock's Quit do: it may ask about unsaved work first, or refuse.
pub fn terminate(pid: u32) -> bool {
    objc2::rc::autoreleasepool(|_| running_app(pid).is_some_and(|a| a.terminate()))
}

/// The application bundle of the process `pid`.
pub fn bundle_of(pid: u32) -> Option<PathBuf> {
    objc2::rc::autoreleasepool(|_| running_app(pid)?.bundleURL()?.path().map(|p| PathBuf::from(p.to_string())))
}

/// How many windows `pid` has on screen (minimised ones are not).
pub fn windows_on_screen(pid: u32) -> usize {
    on_screen_owners().iter().filter(|p| **p == pid).count()
}

/// The processes owning each window on screen. Reads what the window server tells any application; the windows' titles,
/// which it tells only with the Screen Recording permission, are never asked for.
fn on_screen_owners() -> Vec<u32> {
    use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};
    use objc2_core_graphics::{kCGWindowOwnerPID, CGWindowListCopyWindowInfo, CGWindowListOption};
    let Some(windows) = CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements, 0) else {
        return vec![];
    };
    // SAFETY: the window list is documented to be an array of dictionaries keyed by strings.
    let windows: &objc2_core_foundation::CFArray<CFDictionary<CFString, CFType>> = unsafe { windows.cast_unchecked() };
    windows
        .iter()
        .filter_map(|window: CFRetained<CFDictionary<CFString, CFType>>| {
            // SAFETY: an immutable string constant of the framework.
            let owner = window.get(unsafe { kCGWindowOwnerPID })?;
            let number: &CFNumber = owner.downcast_ref()?;
            number.as_i64().map(|n| n as u32)
        })
        .collect()
}

/// The windows each editor had on screen when it was asked to quit (see [`editor::close`](crate::editor::close)).
static BEFORE_QUIT: Mutex<Option<std::collections::HashMap<u32, usize>>> = Mutex::new(None);

/// Notes what `pid` has on screen before it is asked to quit.
pub fn note_windows_before_quit(pid: u32) {
    BEFORE_QUIT.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(pid, windows_on_screen(pid));
}

/// Whether `pid`, asked to quit, shows a window more than it had then: its question about unsaved work. The system draws
/// a modal alert at the level of any other window and titles are out of reach, so this is what tells it, and Unity's other
/// windows do not count as a question.
pub fn asking_before_quit(pid: u32) -> bool {
    let before = BEFORE_QUIT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&pid).copied());
    before.is_some_and(|n| windows_on_screen(pid) > n)
}

/// `pid`'s bundle, when no other process of `others` is of it: only then does `open -a` mean `pid` and no other.
pub fn bundle_if_alone(pid: u32, others: &[u32]) -> Option<PathBuf> {
    let mine = bundle_of(pid)?;
    others.iter().filter(|p| **p != pid).all(|p| bundle_of(*p).as_ref() != Some(&mine)).then_some(mine)
}

/// A file's clone family on APFS: files with the same one hold the same blocks, however many there are. `None` for a file
/// that shares nothing or a file system that does not say.
pub fn clone_family(path: &Path) -> Option<u64> {
    #[repr(C, packed(4))]
    struct Answer {
        length: u32,
        returned: libc::attribute_set_t,
        clone: u64,
        flags: u64,
    }
    const CLONE_ID: u32 = 0x0000_0100;
    const EXT_FLAGS: u32 = 0x0000_0200;
    const EF_MAY_SHARE_BLOCKS: u64 = 1;
    const RETURNED_ATTRS: u32 = 0x8000_0000;
    let path = c_path(path)?;
    let mut request = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: RETURNED_ATTRS,
        volattr: 0,
        dirattr: 0,
        fileattr: 0,
        forkattr: CLONE_ID | EXT_FLAGS,
    };
    let mut answer: Answer = unsafe { std::mem::zeroed() };
    // SAFETY: a valid C string, the request and a buffer of the size given. The extended option selects these attributes.
    let done = unsafe {
        libc::getattrlist(
            path.as_ptr(),
            &mut request as *mut libc::attrlist as *mut libc::c_void,
            &mut answer as *mut Answer as *mut libc::c_void,
            std::mem::size_of::<Answer>(),
            libc::FSOPT_ATTR_CMN_EXTENDED,
        )
    };
    if done != 0 {
        return None;
    }
    let returned = { answer.returned };
    let (clone, flags) = ({ answer.clone }, { answer.flags });
    (returned.forkattr & (CLONE_ID | EXT_FLAGS) == CLONE_ID | EXT_FLAGS && flags & EF_MAY_SHARE_BLOCKS != 0).then_some(clone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_names_its_program_and_a_program_its_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("Blender 4.2.app");
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        assert_eq!(bundle_program(&app), app.join("Contents/MacOS/Blender 4.2"), "without an Info.plist: as the bundle is named");
        let mut info = plist::Dictionary::new();
        info.insert("CFBundleExecutable".into(), plist::Value::String("Blender".into()));
        plist::Value::Dictionary(info).to_file_xml(app.join("Contents/Info.plist")).unwrap();
        assert_eq!(bundle_program(&app), app.join("Contents/MacOS/Blender"));
        assert_eq!(enclosing_app(&app.join("Contents/MacOS/Blender")), Some(app.clone()));
        assert_eq!(enclosing_app(&app), Some(app));
        assert_eq!(enclosing_app(Path::new("/usr/local/bin/blender")), None);
    }

    #[test]
    fn volumes_of_a_container_share_their_room() {
        let volume = |mount: &str, device: &str, kind: &str| Volume { mount: mount.into(), device: device.into(), kind: kind.into(), flags: 0, free: 0 };
        assert_eq!(container("/dev/disk3s5"), Some("disk3"));
        assert_eq!(container("/dev/disk3s1s1"), Some("disk3"));
        assert_eq!(container("/dev/disk12"), Some("disk12"));
        assert_eq!(container("map auto_home"), None);
        let startup = volume("/", "/dev/disk3s5", "apfs");
        assert!(share_room(&startup, &volume("/Volumes/More", "/dev/disk3s7", "apfs")), "a second APFS volume of the same container");
        assert!(!share_room(&startup, &volume("/Volumes/SSD", "/dev/disk5s1", "apfs")), "another disk");
        assert!(!share_room(&volume("/Volumes/A", "/dev/disk6s2", "hfs"), &volume("/Volumes/B", "/dev/disk6s3", "hfs")), "partitions have room of their own");
    }

    #[test]
    fn the_startup_disk_reads_as_itself_and_is_measured() {
        let temp = tempfile::tempdir().unwrap();
        let here = volume_of(temp.path()).unwrap();
        assert!(here.free > 0);
        assert!(!here.device.is_empty() && !here.kind.is_empty());
        let home = volume_of(&dirs::home_dir().unwrap()).unwrap();
        assert_eq!(home.mount, PathBuf::from("/"), "the home folder is on the startup disk's data volume, which is not how anyone names it");
        assert!(volume_of(&temp.path().join("not here")).is_none());
        assert_eq!(fixed_disks().first().map(PathBuf::as_path), Some(Path::new("/")));
    }

    #[test]
    fn only_a_disk_the_finder_shows_is_one_to_keep_files_on() {
        let local = libc::MNT_LOCAL as u32;
        assert!(browsable(Path::new("/Volumes/SSD"), local));
        assert!(!browsable(Path::new("/Volumes/Recovery"), local | libc::MNT_DONTBROWSE as u32));
        assert!(!browsable(Path::new("/Volumes/Installer"), local | libc::MNT_RDONLY as u32));
        assert!(!browsable(Path::new("/Volumes/Share"), 0), "a network share");
        assert!(!browsable(Path::new("/System/Volumes/Preboot"), local));
    }

    #[test]
    fn clones_are_told_apart_from_plain_files() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a.bin");
        let plain = temp.path().join("plain.bin");
        std::fs::write(&a, vec![7u8; 1 << 16]).unwrap();
        std::fs::write(&plain, vec![7u8; 1 << 16]).unwrap();
        assert_eq!(clone_family(&plain), None, "shares nothing");
        // std::fs::copy clones on APFS.
        let b = temp.path().join("b.bin");
        std::fs::copy(&a, &b).unwrap();
        if volume_of(temp.path()).unwrap().kind == "apfs" {
            assert!(clone_family(&a).is_some());
            assert_eq!(clone_family(&a), clone_family(&b));
        }
    }

    #[test]
    fn what_opens_a_file_is_asked_of_the_system() {
        let temp = tempfile::tempdir().unwrap();
        let text = temp.path().join("note.txt");
        std::fs::write(&text, "x").unwrap();
        let app = default_app(&text).expect("the system opens text files with something");
        assert_eq!(app.extension().and_then(|e| e.to_str()), Some("app"), "{}", app.display());
        assert_eq!(default_app(&temp.path().join("not there.txt")), None);
    }

    #[test]
    fn no_application_means_no_answer() {
        // A process number no application has: this test program is not one Launch Services knows.
        let pid = std::process::id();
        assert!(!activate(pid));
        assert!(!terminate(pid));
        assert_eq!(bundle_of(pid), None);
    }
}
