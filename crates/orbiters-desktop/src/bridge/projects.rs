//! The bridge's Unity projects tools: what the Unity projects tab does, for an agent. They call what the tab's buttons call
//! (`orbiters_projects`: the shared VCC environment through vrc-get, the projects found on this computer, Unity's editors,
//! the Library move, the templates), in the tray service, so they work with the window closed and behave as the tab does.
//! Each change tells the window, whose tab then looks again ([`Host::changed`]).
//!
//! - Reading is immediate (`unity_list_projects`, `unity_get_project`, `unity_find_packages`, ...).
//! - What takes long runs as a job on a thread of its own (package changes, a scan, a copy, the Library move, a size):
//!   the tool waits for it up to `wait_s`, then answers with the job, which `unity_get_job` follows and `unity_cancel_job`
//!   stops where the tab's own Stop or Cancel does. One package or project change at a time, as in the tab; the Library
//!   move, a scan and a size each have their own lane.
//! - The person's rules stay: nothing changes while they paused the bridge (the hotkey, Settings), nothing opens a window
//!   while they are using the computer, and Stop ends what can be ended.
//! - Nothing here deletes a project's files: the tab has no such thing. Remove from the list wants the exact path of a
//!   listed project and `confirm`, and its files stay. Kill Unity wants `confirm` too.

use super::game::Failure;
use super::person::Paused;
use super::tools::{paused_failure, Answer};
use super::Call;
use orbiters_core::account::Server;
use orbiters_projects::discovery::{self, Kind, Project, Scan};
use orbiters_projects::{blender, coherent, editor, git, hidden, missing, network, size, storage, templates, unity6, updates, vpm};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The longest a call waits for its job: clients give up on a request after about a minute.
const LONGEST_WAIT_S: f64 = 50.0;
/// Finished jobs kept for `unity_get_job`.
const JOBS_KEPT: usize = 40;
/// Mixer's package, from the Orbiters repository (as the tab's Add Mixer).
const MIXER: &str = "orbiters.mixer";
const TOOLKIT: &str = "orbiters.toolkit";

/// What only the tray service can do for these tools.
pub trait Host: Send + Sync {
    /// Shares pictures of a world on Orbiters' Map explorer (the Worlds card's Pictures and Files).
    fn upload_pictures(&self, world_id: String, world_name: String, paths: Vec<PathBuf>) -> Result<(), String>;
    /// Shares a reconstructed capture as the world's 3D preview (the Worlds card's 3D capture).
    fn upload_preview(&self, world_id: String, world_name: String, session: PathBuf) -> Result<(), String>;
    /// The world's uploads, as the service lists them.
    fn uploads(&self, world_id: &str) -> Value;
    /// What this computer has of a world: its captures (`dir`, `recorded_utc`, `splats` once reconstructed) and how many
    /// pictures VRChat took there.
    fn world_here(&self, world_id: &str) -> Value;
    /// Something the Unity projects tab shows changed: its window looks again.
    fn changed(&self);
}

/// Which jobs may run beside which: one of each at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lane {
    /// Package, repository and project changes: the tab's one action at a time.
    Action,
    /// The Library move (and its plan), which the tab runs beside everything else.
    Storage,
    /// The look for projects on this computer.
    Scan,
    /// Measuring a project's size.
    Measure,
}

impl Lane {
    fn name(self) -> &'static str {
        match self {
            Lane::Action => "action",
            Lane::Storage => "storage",
            Lane::Scan => "scan",
            Lane::Measure => "measure",
        }
    }
}

/// How far a job got, when it says.
#[derive(Clone)]
enum Progress {
    None,
    Copy(templates::SharedProgress),
    Storage(storage::SharedProgress),
}

impl Progress {
    fn json(&self) -> Value {
        match self {
            Progress::None => Value::Null,
            Progress::Copy(p) => {
                let p = p.lock().unwrap_or_else(|e| e.into_inner()).clone();
                json!({ "phase": p.phase, "copied": p.copied, "total": p.total, "file": p.file })
            }
            Progress::Storage(p) => {
                let p = p.lock().unwrap_or_else(|e| e.into_inner()).clone();
                json!({ "fraction": p.fraction(), "cancellable": p.cancellable(), "detail": p })
            }
        }
    }
}

/// A job's end: its result, or its failure.
type Outcome = Result<Value, Failure>;

struct Job {
    id: u64,
    lane: Lane,
    label: String,
    project: Option<PathBuf>,
    started_at: u64,
    begun: Instant,
    cancel: Arc<AtomicBool>,
    /// Whether `unity_cancel_job` stops it (a package change cannot be stopped half way).
    cancellable: bool,
    progress: Progress,
    end: Mutex<Option<(Outcome, Instant)>>,
    /// What is asked again each time the job is looked at once it ended (N4N0's setup result, written by Unity).
    probe: Option<Box<dyn Fn() -> Value + Send + Sync>>,
}

impl Job {
    fn done(&self) -> bool {
        self.end.lock().unwrap().is_some()
    }

    fn json(&self) -> Value {
        let end = self.end.lock().unwrap();
        let (state, result, failure, seconds) = match end.as_ref() {
            None => ("running", Value::Null, Value::Null, self.begun.elapsed().as_secs_f64()),
            Some((Ok(result), at)) => ("done", result.clone(), Value::Null, at.duration_since(self.begun).as_secs_f64()),
            Some((Err(failure), at)) => ("failed", Value::Null, super::tools::failure_json(failure), at.duration_since(self.begun).as_secs_f64()),
        };
        let mut job = json!({
            "job": self.id,
            "state": state,
            "label": self.label,
            "lane": self.lane.name(),
            "project": self.project,
            "started_at": self.started_at,
            "seconds": (seconds * 10.0).round() / 10.0,
            "cancellable": self.cancellable && end.is_none(),
            "cancelling": self.cancel.load(Ordering::Relaxed) && end.is_none(),
        });
        if end.is_none() {
            job["progress"] = self.progress.json();
            job["next"] = json!("Call unity_get_job with this job and wait_s to wait for it.");
        } else if !failure.is_null() {
            job["failure"] = failure;
        } else {
            job["result"] = result;
            if let Some(probe) = &self.probe {
                job["result"]["now"] = probe();
            }
        }
        job
    }
}

#[derive(Default)]
struct Jobs {
    next: u64,
    list: Vec<Arc<Job>>,
}

/// The Unity projects side of the bridge.
#[derive(Default)]
pub struct Projects {
    host: OnceLock<Arc<dyn Host>>,
    jobs: Mutex<Jobs>,
    /// The scan as the running look last published it.
    scan: Arc<Mutex<Option<Scan>>>,
    /// By project (`discovery::key`): packages no enabled repository lists, from the last package change that failed for
    /// them, and that change (run again by `unity_missing_packages` add).
    missing: Arc<Mutex<HashMap<String, (vpm::Change, Vec<missing::Missing>)>>>,
    /// The person's pause as it is at this moment (the bridge's), looked at again right before what ends an editor or changes
    /// a project it has open: they may come back after the call was dispatched (verify-n4n0 6).
    presence: OnceLock<Live>,
}

/// The person's pause now.
pub type Live = Arc<dyn Fn() -> Option<Paused> + Send + Sync>;

/// The tools' names, in the order `tools/list` gives them.
pub const NAMES: [&str; 16] = [
    "unity_list_projects",
    "unity_get_project",
    "unity_find_packages",
    "unity_change_packages",
    "unity_missing_packages",
    "unity_repositories",
    "unity_editor",
    "unity_manage_project",
    "unity_list_templates",
    "unity_create_project",
    "unity_duplicate_project",
    "unity_storage",
    "unity_add_orbiters",
    "unity_world",
    "unity_get_job",
    "unity_cancel_job",
];

/// The tools that only read: never logged, and allowed while the person paused the bridge.
pub fn reads(name: &str) -> bool {
    matches!(name, "unity_list_projects" | "unity_get_project" | "unity_find_packages" | "unity_list_templates" | "unity_get_job")
}

/// What the server's instructions add for these tools.
pub const INSTRUCTIONS: &str = "It also does what Orbiters - N4N0's Unity projects tab does (the unity_ tools): projects, VPM packages and repositories, Unity's editors, new projects and copies, a project's disk, Orbiters' packages and world uploads. Read the resource bridge://unity-projects before using them.";
/// The resource that holds [`GUIDE`].
pub const GUIDE_URI: &str = "bridge://unity-projects";
/// A resource asked for that there is not.
pub const NO_SUCH_RESOURCE: &str = "There is no such resource: bridge://guide and bridge://unity-projects are the ones there are.";

/// The resource as `resources/list` gives it.
pub fn resource() -> Value {
    json!({ "uri": GUIDE_URI, "name": "unity-projects", "title": "How to use the Unity projects tools", "description": "What the unity_ tools do, how their jobs go, and what to ask the person first.", "mimeType": "text/markdown" })
}

/// How the Unity projects tools go, for a model.
pub const GUIDE: &str = "# Unity projects\n\n\
These tools do what Orbiters - N4N0's Unity projects tab does, in the same shared VCC environment as Creator Companion and ALCOM, so what you change shows there and in the tab.\n\n\
- `unity_list_projects` lists the projects found on this computer (`listed` ones are in VCC's list, the others were found on a disk). `refresh: true` looks again in a job.\n\
- `unity_get_project` with a project's `path`: its installed packages, updates (and those held back, with why), Update all's plan, Unity's editor, its disk, Git, Blender files, previews and worlds.\n\
- Unity 6: each project's `unity_line` says where it stands from its own files (`on_2022`, `on_unity6`, `editor_and_sdk_differ`, `sdk_missing`, ...), its SDK, whether that is a beta, and the Unity 6 trials Orbiters recorded of it (`trials`) or as it (`trial_of`). These are facts to read: no tool migrates a project, opens Unity 6 or uploads. A trial keeps its original's content IDs (`upload_warning`): once uploaded from Unity 6, that content can no longer be uploaded from 2022, so say so before suggesting an upload. The person starts a trial in the tab (Try on Unity 6), and Creator Companion migrates it.\n\
- Packages: `unity_find_packages` for what the repositories offer; `unity_change_packages` to install (a version or the newest), remove, update one, update all, or restore what is missing. A change that conflicts comes back with the plan that makes it coherent: show it to the person if it removes anything, then `apply_plan`. A change that fails because no repository lists a package: `unity_missing_packages` looks for the repositories that have it and adds them.\n\
- `unity_repositories` lists, adds (by URL), removes, turns on and off, and downloads again the VPM repositories; `suggest` searches Orbiters' catalog of known ones.\n\
- `unity_editor` opens a project in Unity, brings it forward, asks it to Save & Exit, or (with `confirm`) kills it.\n\
- `unity_manage_project`: add a found project to the list, remove one from it (its files stay; exact path and `confirm`), hide or show it in the tab, open its folder, Unity Hub or one of its Blender files.\n\
- New projects: `unity_list_templates`, then `unity_create_project`; `unity_duplicate_project` copies one with its exact packages (check_only says what it needs on the disk first).\n\
- `unity_storage`: a project's disk, and moving its Library to a disk with room (`plan` first, then `repair`).\n\
- `unity_add_orbiters`: Mixer, the Toolkit, My Avatar, or N4N0 on one of the project's avatars.\n\
- `unity_world`: what a world project's worlds have on Orbiters' Map explorer, and adding pictures or a 3D capture to one.\n\n\
Long work runs as a job: the tool waits up to `wait_s` (default 30) and returns its `job`; `unity_get_job` follows it, `unity_cancel_job` stops a copy, a Library move, a scan or a size. One package or project change runs at a time: a second is refused with `busy`.\n\n\
Ask the person before removing packages others depend on, removing a project from the list, killing Unity (unsaved work is lost), or moving a Library. Nothing here deletes a project's files.\n";

fn number(args: &Value, key: &str, default: f64) -> f64 {
    args[key].as_f64().unwrap_or(default)
}

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args[key].as_str().map(str::trim).filter(|s| !s.is_empty())
}

/// Whether `path` is a plain path on a drive of this computer (`C:\…`): never a network share (`\\host\share`, whose
/// first look opens a connection to a host the text chose) nor a device path (`\\?\`, `\\.\`) (BRIDGE-7).
pub(crate) fn on_a_local_drive(path: &Path) -> bool {
    #[cfg(unix)]
    {
        lexical_or_absolute_local(path)
    }
    #[cfg(not(unix))]
    {
        use std::path::{Component, Prefix};
        let lexical = path.is_absolute() && matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)));
        on_a_windows_disk(path, lexical)
    }
}

/// A Mac or Linux: an absolute path that resolves, link by link, to a place on a volume the system calls local (`MNT_LOCAL`
/// on a Mac, and not a network or automounted file system): never a network share (SMB, AFP, NFS, WebDAV) or a folder
/// linked to one. No part of the path under a mount that is not local is ever touched (a share that stopped answering
/// holds any look at it for minutes).
#[cfg(unix)]
fn lexical_or_absolute_local(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let mounts = mounts();
    resolve_locally(path, &mounts, &|p: &Path| look(p)).is_some_and(|real| mount_is_local(&real, &mounts) != Some(false))
}

/// What one part of a path is, looked at without following it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Missing,
    Plain,
    Link(PathBuf),
    Unreadable,
}

#[cfg(unix)]
fn look(path: &Path) -> Part {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::read_link(path).map_or(Part::Unreadable, Part::Link),
        Ok(_) => Part::Plain,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Part::Missing,
        Err(_) => Part::Unreadable,
    }
}

/// Resolves `path` (absolute) part by part with `look`, following its links, checking each part against the mount table
/// `mounts` BEFORE it is looked at: `None` as soon as a part is under a mount that is not local (never looked at), a link
/// loops, or a part cannot be read. The parts past the first one missing are taken as written (a project to make).
pub(crate) fn resolve_locally(path: &Path, mounts: &[(PathBuf, bool)], look: &dyn Fn(&Path) -> Part) -> Option<PathBuf> {
    use std::path::Component;
    let mut todo: Vec<std::ffi::OsString> = path.components().rev().filter_map(|c| match c {
        Component::Normal(n) => Some(n.to_os_string()),
        Component::ParentDir => Some("..".into()),
        _ => None,
    }).collect();
    let mut real = PathBuf::from("/");
    let mut links = 0;
    let mut missing = false;
    while let Some(part) = todo.pop() {
        if part == ".." {
            real.pop();
            continue;
        }
        let next = real.join(&part);
        if mount_is_local(&next, mounts) == Some(false) {
            return None;
        }
        if missing {
            real = next;
            continue;
        }
        match look(&next) {
            Part::Plain => real = next,
            Part::Missing => {
                missing = true;
                real = next;
            }
            Part::Unreadable => return None,
            Part::Link(target) => {
                links += 1;
                if links > 40 {
                    return None;
                }
                // The link's target, then what was left of the path, are resolved in turn.
                let rest: Vec<std::ffi::OsString> = target.components().rev().filter_map(|c| match c {
                    Component::Normal(n) => Some(n.to_os_string()),
                    Component::ParentDir => Some("..".into()),
                    _ => None,
                }).collect();
                if target.is_absolute() {
                    real = PathBuf::from("/");
                }
                todo.extend(rest);
            }
        }
    }
    Some(real)
}

/// Whether the volume `path` is on, by its mount point alone (the longest one it is under), is local; `None` when no mount
/// is known for it.
pub(crate) fn mount_is_local(path: &Path, mounts: &[(PathBuf, bool)]) -> Option<bool> {
    mounts.iter().filter(|(point, _)| path.starts_with(point)).max_by_key(|(point, _)| point.as_os_str().len()).map(|(_, local)| *local)
}

/// The file systems that are never local, whatever their flags say (an automounted point contacts a server when entered).
const REMOTE_KINDS: [&str; 7] = ["smbfs", "nfs", "afpfs", "webdav", "autofs", "ftp", "cifs"];

/// The mounted volumes, each with whether the system calls it local (`getfsstat` into a buffer of this call's own, without
/// waiting on any of them: thread-safe, unlike `getmntinfo`'s shared one).
#[cfg(target_os = "macos")]
fn mounts() -> Vec<(PathBuf, bool)> {
    use std::os::unix::ffi::OsStrExt;
    const MNT_LOCAL: u32 = 0x0000_1000;
    const MNT_NOWAIT: i32 = 2;
    let field = |chars: &[libc::c_char]| chars.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect::<Vec<u8>>();
    for _ in 0..4 {
        // SAFETY: a null buffer asks only how many there are.
        let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, MNT_NOWAIT) };
        if count <= 0 {
            return Vec::new();
        }
        // Room for a few mounted meanwhile; a full buffer is asked again.
        let room = count as usize + 8;
        let mut entries: Vec<libc::statfs> = Vec::with_capacity(room);
        let bytes = (room * std::mem::size_of::<libc::statfs>()) as libc::c_int;
        // SAFETY: the buffer is ours, `bytes` long; the call writes at most that many and says how many entries it wrote.
        let got = unsafe { libc::getfsstat(entries.as_mut_ptr(), bytes, MNT_NOWAIT) };
        if got < 0 {
            return Vec::new();
        }
        let got = got as usize;
        if got >= room {
            continue;
        }
        // SAFETY: the call initialised the first `got` entries.
        unsafe { entries.set_len(got) };
        return entries
            .iter()
            .map(|e| {
                let kind = String::from_utf8_lossy(&field(&e.f_fstypename)).into_owned();
                let local = e.f_flags & MNT_LOCAL != 0 && !REMOTE_KINDS.contains(&kind.as_str());
                (PathBuf::from(std::ffi::OsStr::from_bytes(&field(&e.f_mntonname))), local)
            })
            .collect();
    }
    Vec::new()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn mounts() -> Vec<(PathBuf, bool)> {
    Vec::new()
}

#[cfg(test)]
#[test]
fn a_path_on_a_network_mount_is_told_from_the_mount_table_alone() {
    let table = [(PathBuf::from("/"), true), (PathBuf::from("/Volumes/Share"), false), (PathBuf::from("/Volumes/Share/Inner"), true)];
    assert_eq!(mount_is_local(Path::new("/Volumes/Share/Projects/MyWorld"), &table), Some(false));
    assert_eq!(mount_is_local(Path::new("/Volumes/Share/Inner/x"), &table), Some(true), "the longest mount point holds it");
    assert_eq!(mount_is_local(Path::new("/Users/me/Projects"), &table), Some(true));
    assert_eq!(mount_is_local(Path::new("/Volumes/Shared"), &table), Some(true), "a name that only begins like a mount point is not under it");
    assert_eq!(mount_is_local(Path::new("/x"), &[]), None);
    #[cfg(target_os = "macos")]
    assert!(mounts().iter().any(|(point, local)| point == Path::new("/") && *local), "this Mac's own disk");
}

/// B2: a local-looking path whose link leads into a share is refused without the share ever being looked at; links are
/// followed part by part, loops and unreadable parts refused, a missing tail taken as written.
#[cfg(test)]
#[test]
fn a_link_into_a_share_is_refused_without_touching_the_share() {
    let table = [(PathBuf::from("/"), true), (PathBuf::from("/Volumes/Share"), false)];
    let looked = std::sync::Mutex::new(Vec::<PathBuf>::new());
    let fs = |p: &Path| {
        looked.lock().unwrap().push(p.to_path_buf());
        match p.to_str().unwrap() {
            "/Users" | "/Users/me" | "/Users/me/Projects" => Part::Plain,
            "/Users/me/Linked" => Part::Link(PathBuf::from("/Volumes/Share/World")),
            "/Users/me/Up" => Part::Link(PathBuf::from("../me/Projects")),
            "/Users/me/Loop" => Part::Link(PathBuf::from("/Users/me/Loop")),
            "/Users/me/Locked" => Part::Unreadable,
            _ => Part::Missing,
        }
    };
    assert_eq!(resolve_locally(Path::new("/Users/me/Linked/Assets"), &table, &fs), None);
    assert!(looked.lock().unwrap().iter().all(|p| !p.starts_with("/Volumes/Share")), "the share was never looked at: {:?}", looked.lock().unwrap());
    assert_eq!(resolve_locally(Path::new("/Volumes/Share/x"), &table, &fs), None);
    assert_eq!(resolve_locally(Path::new("/Users/me/Up/World"), &table, &fs), Some(PathBuf::from("/Users/me/Projects/World")), "a relative link, then a part to make");
    assert_eq!(resolve_locally(Path::new("/Users/me/Loop"), &table, &fs), None, "a loop");
    assert_eq!(resolve_locally(Path::new("/Users/me/Locked/x"), &table, &fs), None);
    assert_eq!(resolve_locally(Path::new("/Users/me/New/Deeper"), &table, &fs), Some(PathBuf::from("/Users/me/New/Deeper")));
    assert_eq!(resolve_locally(Path::new("/Users/me/New/../../me/Linked"), &table, &fs), Some(PathBuf::from("/Users/me/Linked")), "past a missing part, the rest is taken as written, never looked at");
}

/// The listed project `given` names: the one of that very path; else, of the one alike (another case, other separators), only when
/// it is the only one and is the same folder (or neither is there, and nothing tells them apart).
fn the_listed_one<'a>(listed: &'a [PathBuf], given: &Path) -> Option<&'a PathBuf> {
    if let Some(exact) = listed.iter().find(|p| p.as_path() == given) {
        return Some(exact);
    }
    let mut alike = listed.iter().filter(|p| discovery::key(p) == discovery::key(given));
    let (one, more) = (alike.next()?, alike.next().is_some());
    let same = match (orbiters_projects::owned::id_of(one), orbiters_projects::owned::id_of(given)) {
        (Ok(a), Ok(b)) => a == b,
        // Where a disk keeps names apart by case, one that is there is not one that is not.
        (Err(_), Err(_)) => true,
        _ => false,
    };
    (!more && same).then_some(one)
}

#[cfg(test)]
#[test]
fn a_project_to_take_off_is_the_very_one_named() {
    let temp = std::env::temp_dir().join(format!("orbiters-bridge-listed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let (a, b) = (temp.join("Avatar"), temp.join("Other"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let upper = PathBuf::from(a.to_string_lossy().replace("Avatar", "AVATAR"));
    let listed = vec![a.clone(), b.clone()];
    assert_eq!(the_listed_one(&listed, &a), Some(&a));
    // Another spelling of it, where that is the same folder; on a disk that keeps them apart it names nothing.
    assert_eq!(the_listed_one(&listed, &upper), upper.exists().then_some(&a));
    // Two that differ by case only: neither is guessed at by a third spelling.
    let mixed = PathBuf::from(a.to_string_lossy().replace("Avatar", "aVATAR"));
    let both = vec![a.clone(), upper.clone()];
    assert_eq!(the_listed_one(&both, &mixed), None);
    assert_eq!(the_listed_one(&both, &upper), Some(&upper));
    // Not there any more: by the only spelling like it.
    let gone = temp.join("Gone");
    assert_eq!(the_listed_one(std::slice::from_ref(&gone), &temp.join("GONE")), Some(&gone));
    assert_eq!(the_listed_one(&listed, &temp.join("Nothing")), None);
    let _ = std::fs::remove_dir_all(&temp);
}

/// B2: the mount table is read by each thread into its own buffer: many at once give the same table.
#[cfg(all(test, target_os = "macos"))]
#[test]
fn the_mount_table_is_read_by_many_threads_at_once() {
    let first = mounts();
    assert!(!first.is_empty());
    let threads: Vec<_> = (0..16).map(|_| std::thread::spawn(|| (0..50).map(|_| mounts()).collect::<Vec<_>>())).collect();
    for t in threads {
        for table in t.join().unwrap() {
            assert_eq!(table, first);
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn local_volume(path: &Path) -> Option<bool> {
    use std::os::unix::ffi::OsStrExt;
    /// `MNT_LOCAL` of `<sys/mount.h>`.
    const MNT_LOCAL: u32 = 0x0000_1000;
    let real = std::fs::canonicalize(path).ok()?;
    let name = std::ffi::CString::new(real.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid C string and a zeroed structure of the call's own type, written by the call.
    let ok = unsafe { libc::statfs(name.as_ptr(), &mut stats) } == 0;
    ok.then_some(stats.f_flags & MNT_LOCAL != 0)
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn local_volume(path: &Path) -> Option<bool> {
    // Linux: what /proc/mounts says of the filesystem's type would be needed; absolute is what was checked before.
    Some(path.is_absolute())
}

#[cfg(not(unix))]
fn on_a_windows_disk(path: &Path, lexical: bool) -> bool {
    use std::path::{Component, Prefix};
    // A drive letter can be a mapped network share, and a folder a link to one (verify-n4n0 6): what the nearest existing
    // folder resolves to must be a disk of this computer.
    lexical && {
        let mut at = Some(path);
        let existing = std::iter::from_fn(|| {
            let here = at?;
            at = here.parent();
            Some(here)
        })
        .find(|p| p.exists());
        // Nothing of it exists (a drive not there): nothing to resolve, nothing reached; what uses it fails by itself.
        existing.is_none_or(|p| std::fs::canonicalize(p).ok().is_some_and(|real| matches!(real.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::VerbatimDisk(_) | Prefix::Disk(_)))))
    }
}

/// `path` as the disk has it (links and aliases followed), in its plain `C:\…` spelling; as given when it cannot be read.
fn real_path(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(real) => {
            let text = real.to_string_lossy();
            match text.strip_prefix(r"\\?\") {
                Some(plain) if plain.as_bytes().get(1) == Some(&b':') => PathBuf::from(plain),
                _ => real,
            }
        }
        Err(_) => path.to_path_buf(),
    }
}

fn bad(message: impl Into<String>, next: impl Into<String>) -> Failure {
    Failure::new("bad_arguments", message, next)
}

fn failed(code: &'static str, error: anyhow::Error, next: &str) -> Failure {
    Failure::new(code, format!("{error:#}"), next)
}

fn root() -> PathBuf {
    orbiters_projects::vcc_dir()
}

fn block_on<T>(f: impl std::future::Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    orbiters_projects::runtime()?.block_on(f)
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Avatars => "avatars",
        Kind::Worlds => "worlds",
        Kind::Vrchat => "vrchat",
        Kind::Unity => "unity",
    }
}

/// Unity's editors, and which project each has open, as the tab sees them.
struct Editors {
    running: Vec<editor::Editor>,
    unread: bool,
}

impl Editors {
    fn look() -> Self {
        // Each editor's project as the disk has it: an alias of a path (another spelling, a link) never hides an open one.
        let running: Vec<editor::Editor> = editor::running()
            .into_iter()
            .map(|mut e| {
                if !e.project.as_os_str().is_empty() {
                    e.project = real_path(&e.project);
                }
                e
            })
            .collect();
        let unread = running.iter().any(|e| e.project.as_os_str().is_empty());
        Self { running, unread }
    }

    fn of(&self, path: &Path) -> Option<&editor::Editor> {
        let path = &real_path(path);
        // The lock file is read only for an editor whose command line could not be.
        let locked = self.unread && orbiters_projects::unity_open(path);
        editor::of(&self.running, path, locked)
    }

    fn json(&self, path: &Path) -> Value {
        match self.of(path) {
            Some(e) => json!({ "open": true, "pid": e.pid, "title": e.title, "waiting_on_a_dialog": e.asking }),
            None => json!({ "open": false }),
        }
    }
}

/// The Unity 6 transition's read-only facts of the project at `path` (REQUEST22): its standing from its own files, and
/// the trials Orbiters recorded of it or as it. No tool migrates, opens Unity 6 or uploads.
fn unity_line_json(path: &Path, matrix: &unity6::matrix::Matrix, trials: &[(unity6::trial::Job, unity6::trial::Status)]) -> Value {
    use unity6::standing::{Content, State};
    // Policy facts come from the built-in release list only (verification's major 7).
    let _ = matrix;
    let s = unity6::standing::read(path, &unity6::matrix::Matrix::built_in());
    let state = match s.state {
        State::NotVrchat => "not_vrchat",
        State::LegacySdk => "legacy_sdk_in_assets",
        State::EditorUnknown => "editor_unknown",
        State::SdkMissing => "sdk_missing",
        State::SdkIncomplete => "sdk_incomplete",
        State::On2022 => "on_2022",
        State::OnUnity6 => "on_unity6",
        State::Disagree => "editor_and_sdk_differ",
        State::OtherLine => "other_line",
        State::LineUnknown => "line_unknown",
    };
    // By identity, wherever it is now and whatever it is called (the window asks the same): a folder replaced at a recorded
    // path is not the one recorded, and one renamed since still is.
    let now = std::fs::canonicalize(path).ok().and_then(|p| orbiters_projects::owned::id_of(&p).ok());
    let here = |place: &unity6::trial::Place| now.is_some() && place.id == now;
    let as_trial = trials.iter().find(|(j, s)| !matches!(s, unity6::trial::Status::Replaced) && (here(j.unity6()) || here(&j.trial)));
    let of_it: Vec<Value> = trials
        .iter()
        .filter(|(j, _)| here(&j.source))
        .map(|(j, st)| json!({ "trial": j.unity6().path, "status": st.name(), "made": j.created }))
        .collect();
    let mut out = json!({
        "state": state,
        "editor": s.editor.text,
        "editor_revision": s.editor.revision,
        "sdk_installed": s.sdk.installed,
        "sdk_locked": s.sdk.locked,
        "sdk_requested": s.sdk.requested,
        "sdk_needs_line": s.sdk.needs,
        "sdk_missing": s.sdk.missing,
        "sdk_not_at_locked_version": s.sdk.mismatched,
        "beta": s.beta,
        "content": s.sdk.content.map(|c| match c { Content::Avatars => "avatars", Content::Worlds => "worlds", Content::Base => "base" }),
        "history": if as_trial.is_some() { "a Unity 6 trial Orbiters recorded" } else { "not recorded by Orbiters" },
        "trial_of": as_trial.map(|(j, st)| json!({
            "original": j.source.path,
            "original_is_still_there": match j.source.verified() { Some(true) => "yes", Some(false) if j.source.path.exists() => "replaced", Some(false) => "missing", None => "cannot_tell" },
            "status": st.name(),
            "made": j.created,
            "can_be_opened_or_changed": matches!(st, unity6::trial::Status::OnUnity6) && j.migrated.as_ref().is_none_or(|m| here(m)),
        })),
        "trials": of_it,
    });
    if as_trial.is_some() {
        out["upload_warning"] = json!("This trial keeps its original's avatar and world IDs: once one is uploaded from Unity 6, VRChat no longer lets it be uploaded from Unity 2022. Test with Build & Test, or give the trial a test ID of its own first.");
    }
    out
}

fn project_json(project: &Project, hidden: &std::collections::BTreeSet<String>, editors: &Editors) -> Value {
    let size = size::lookup(&project.path).size;
    let things = |list: &[orbiters_projects::avatars::Avatar]| -> Vec<Value> {
        list.iter().map(|a| json!({ "blueprint_id": a.id, "name": a.name, "scene": a.scene })).collect()
    };
    json!({
        "name": project.name,
        "path": project.path,
        "unity": project.unity,
        "kind": kind_name(project.kind),
        "listed": project.registered && !project.own,
        "folder_missing": project.missing,
        "hidden": hidden::contains(hidden, &project.path),
        "modified": project.modified,
        "avatars": things(&project.avatars),
        "worlds": things(&project.worlds),
        "unity_editor": editors.json(&project.path),
        "size": size.map(|s| json!({ "total": s.total, "library": s.library, "files": s.files })),
    })
}

/// The trial target and the dated facts of the transition, the same in both tools (verification's minor 1).
fn unity6_target_json(matrix: &unity6::matrix::Matrix) -> Value {
    let Some(t) = matrix.unity6_target() else { return Value::Null };
    json!({
        "sdk": t.sdk,
        "unity": t.unity,
        "changeset": t.changeset,
        "channel": match t.channel { unity6::matrix::Channel::Beta => "beta", unity6::matrix::Channel::Stable => "stable" },
        "hub_link": t.hub_link(),
        "guide": "https://vrc-beta-docs.netlify.app/sdk/upgrade/unity-6000/",
        "unity6_stable_since": matrix.data.events.unity6_stable,
        "unity2022_uploads_end": matrix.data.events.unity2022_retirement,
        "release_list_checked": matrix.data.verified,
    })
}

/// The release list kept in app data and the trials recorded there (read only, no network).
fn unity6_facts() -> (unity6::matrix::Matrix, Vec<(unity6::trial::Job, unity6::trial::Status)>) {
    let dir = orbiters_projects::data_dir();
    let matrix = unity6::matrix::load(&dir, unity6::key().as_ref());
    let record = unity6::trial::read(&dir);
    let trials = record.jobs.iter().map(|j| (j.clone(), unity6::trial::status(&dir, &record, j))).filter(|(_, s)| !matches!(s, unity6::trial::Status::Abandoned)).collect();
    (matrix, trials)
}

fn version_is_prerelease(version: &str) -> bool {
    version.contains('-')
}

fn package_json(p: &vpm::Package, prerelease: bool) -> Value {
    let versions: Vec<&String> = p.versions.iter().filter(|v| prerelease || !version_is_prerelease(v)).collect();
    json!({
        "name": p.name,
        "display_name": p.display_name,
        "installed": p.installed,
        "newest": versions.first(),
        "versions": versions,
        "repository": p.repository,
    })
}

impl Projects {
    /// The service's side of the tools: once, when the service starts the bridge.
    pub fn set_host(&self, host: Arc<dyn Host>) {
        let _ = self.host.set(host);
    }

    fn changed(&self) {
        if let Some(host) = self.host.get() {
            host.changed();
        }
    }

    fn host(&self) -> Result<&Arc<dyn Host>, Failure> {
        self.host.get().ok_or_else(|| Failure::new("not_available", "This needs the tray service of Orbiters - N4N0, which is not here.", "Tell the person."))
    }

    /// How to ask for the person's pause at any moment (set once, by the bridge).
    pub fn set_presence(&self, live: Live) {
        let _ = self.presence.set(live);
    }

    fn live(&self) -> Live {
        self.presence.get().cloned().unwrap_or_else(|| Arc::new(|| None))
    }

    /// The person took control back: what can be stopped stops.
    pub fn stop_all(&self) {
        for job in &self.jobs.lock().unwrap().list {
            if job.cancellable && !job.done() {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    /// One tool call. `paused`: the person's pause, if any.
    pub fn call(&self, name: &str, args: &Value, call: &Call, paused: Option<Paused>) -> Result<Answer, Failure> {
        // A pause held with the hotkey or from Settings stops every change; the person's own input only what opens windows.
        if let Some(paused) = paused.as_ref().filter(|p| matches!(p, Paused::Held)) {
            if !reads(name) {
                return Err(paused_failure(paused));
            }
        }
        let shows_a_window = match name {
            "unity_editor" => matches!(text(args, "action"), Some("open" | "show")),
            "unity_manage_project" => matches!(text(args, "action"), Some("reveal" | "open_hub" | "open_blender")),
            _ => false,
        };
        if let Some(paused) = paused.as_ref().filter(|_| shows_a_window) {
            return Err(paused_failure(paused));
        }
        // The person at the computer may be working in Unity (BRIDGE-8): their input also stops what ends an editor, and what
        // changes a project an editor has open (Unity takes the change at once: a reload under their hands).
        if let Some(paused) = paused.as_ref() {
            let ends_an_editor = name == "unity_editor" && matches!(text(args, "action"), Some("save_and_exit" | "kill"));
            let changes_a_project = match name {
                "unity_change_packages" => text(args, "action") != Some("solve"),
                "unity_missing_packages" => text(args, "action") == Some("add"),
                "unity_add_orbiters" => true,
                "unity_storage" => text(args, "action") == Some("repair"),
                _ => false,
            };
            // Unity opens after add_orbiters unless asked not to, for the Toolkit (its own default, below).
            let opens_unity = name == "unity_add_orbiters" && args["open_unity"].as_bool().unwrap_or(text(args, "what") == Some("toolkit"));
            // A path is looked at only once it is a local one (no probe of a network or device path).
            let open_here = || text(args, "path").map(Path::new).filter(|p| on_a_local_drive(p)).is_some_and(|p| Editors::look().of(p).is_some());
            if ends_an_editor || (changes_a_project && (opens_unity || open_here())) {
                return Err(paused_failure(paused));
            }
        }
        let data = match name {
            "unity_list_projects" => self.list_projects(args, call)?,
            "unity_get_project" => self.get_project(args)?,
            "unity_find_packages" => self.find_packages(args)?,
            "unity_change_packages" => self.change_packages(args, call)?,
            "unity_missing_packages" => self.missing_packages(args, call)?,
            "unity_repositories" => self.repositories(args, call)?,
            "unity_editor" => self.editor(args)?,
            "unity_manage_project" => self.manage_project(args)?,
            "unity_list_templates" => self.list_templates()?,
            "unity_create_project" => self.create_project(args, call)?,
            "unity_duplicate_project" => self.duplicate_project(args, call)?,
            "unity_storage" => self.storage(args, call)?,
            "unity_add_orbiters" => self.add_orbiters(args, call)?,
            "unity_world" => self.world(args)?,
            "unity_get_job" => self.get_job(args, call)?,
            "unity_cancel_job" => self.cancel_job(args)?,
            other => return Err(Failure::new("unknown_tool", format!("There is no tool called {other}."), "Call tools/list.")),
        };
        Ok(Answer::Data(data))
    }

    // -----------------------------------------------------------------------------------------------------------------
    // Jobs
    // -----------------------------------------------------------------------------------------------------------------

    /// Starts `work` as a job on its lane, then waits for it up to `wait_s`: its whole state when it ended by then.
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &self,
        lane: Lane,
        label: String,
        project: Option<PathBuf>,
        cancellable: bool,
        progress: Progress,
        cancel: Arc<AtomicBool>,
        probe: Option<Box<dyn Fn() -> Value + Send + Sync>>,
        args: &Value,
        call: &Call,
        work: impl FnOnce() -> Outcome + Send + 'static,
    ) -> Result<Value, Failure> {
        let job = {
            let mut jobs = self.jobs.lock().unwrap();
            if let Some(running) = jobs.list.iter().find(|j| j.lane == lane && !j.done()) {
                return Err(Failure::new(
                    "busy",
                    format!("Job {} runs ({}): one {} at a time, as in the Unity projects tab.", running.id, running.label, lane.name()),
                    format!("Call unity_get_job with job {} and wait_s to wait for it, then try again.", running.id),
                ));
            }
            jobs.next += 1;
            let job = Arc::new(Job {
                id: jobs.next,
                lane,
                label,
                project,
                started_at: orbiters_core::visited::now(),
                begun: Instant::now(),
                cancel,
                cancellable,
                progress,
                end: Mutex::new(None),
                probe,
            });
            jobs.list.push(job.clone());
            // The oldest finished ones go.
            while jobs.list.len() > JOBS_KEPT {
                let Some(i) = jobs.list.iter().position(|j| j.done()) else { break };
                jobs.list.remove(i);
            }
            job
        };
        let host = self.host.get().cloned();
        let running = job.clone();
        let spawned = std::thread::Builder::new().name("bridge-unity-job".into()).spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .unwrap_or_else(|_| Err(Failure::new("crashed", "The job stopped on an error of its own.", "Look at the project with unity_get_project, and tell the person.")));
            *running.end.lock().unwrap() = Some((outcome, Instant::now()));
            if let Some(host) = host {
                host.changed();
            }
        });
        if spawned.is_err() {
            *job.end.lock().unwrap() = Some((Err(Failure::new("no_thread", "The job could not start.", "Try again in a moment.")), Instant::now()));
        }
        let wait = Duration::from_secs_f64(number(args, "wait_s", 30.0).clamp(0.0, LONGEST_WAIT_S));
        Self::wait(&job, wait, call);
        Ok(job.json())
    }

    fn wait(job: &Job, limit: Duration, call: &Call) {
        let begin = Instant::now();
        while !job.done() && begin.elapsed() < limit && !call.client_gone() {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn job(&self, id: u64) -> Result<Arc<Job>, Failure> {
        self.jobs.lock().unwrap().list.iter().find(|j| j.id == id).cloned().ok_or_else(|| {
            Failure::new("no_such_job", format!("There is no job {id} (finished jobs are kept for a while only)."), "Call unity_get_job without a job to list them.")
        })
    }

    fn get_job(&self, args: &Value, call: &Call) -> Result<Value, Failure> {
        let Some(id) = args["job"].as_u64() else {
            let jobs = self.jobs.lock().unwrap();
            return Ok(json!({ "jobs": jobs.list.iter().rev().map(|j| j.json()).collect::<Vec<_>>() }));
        };
        let job = self.job(id)?;
        let wait = Duration::from_secs_f64(number(args, "wait_s", 0.0).clamp(0.0, LONGEST_WAIT_S));
        Self::wait(&job, wait, call);
        Ok(job.json())
    }

    fn cancel_job(&self, args: &Value) -> Result<Value, Failure> {
        let id = args["job"].as_u64().ok_or_else(|| bad("job is needed.", "Pass the job's number from unity_get_job."))?;
        let job = self.job(id)?;
        if job.done() {
            return Ok(json!({ "job": id, "cancelled": false, "note": "It had already ended.", "status": job.json() }));
        }
        if !job.cancellable {
            return Err(Failure::new(
                "not_cancellable",
                "A package or repository change cannot be stopped half way: it would leave the project between two states.",
                "Wait for it with unity_get_job.",
            ));
        }
        job.cancel.store(true, Ordering::Relaxed);
        Ok(json!({ "job": id, "cancelled": true, "next": "It stops at its next safe point: call unity_get_job with wait_s to see how it ended." }))
    }

    /// A Library move of this project running.
    fn moving_library(&self, path: &Path) -> Option<u64> {
        let key = discovery::key(path);
        self.jobs.lock().unwrap().list.iter().find(|j| j.lane == Lane::Storage && !j.done() && j.project.as_deref().is_some_and(|p| discovery::key(p) == key)).map(|j| j.id)
    }

    // -----------------------------------------------------------------------------------------------------------------
    // Projects
    // -----------------------------------------------------------------------------------------------------------------

    /// The projects as the last look found them: the running look's, or the one kept on disk.
    fn scan(&self) -> Scan {
        self.scan.lock().unwrap().clone().unwrap_or_else(discovery::cached)
    }

    /// The project at `path` (as the scan lists it, else read from its folder), or why not.
    fn project(&self, args: &Value) -> Result<Project, Failure> {
        let path = text(args, "path").ok_or_else(|| bad("path is needed: the project's folder.", "Take it from unity_list_projects."))?;
        let path = PathBuf::from(path);
        if !on_a_local_drive(&path) {
            return Err(bad(format!("{} is not an absolute path on a drive of this computer.", path.display()), "Pass the project's path as unity_list_projects gives it."));
        }
        let key = discovery::key(&path);
        if let Some(found) = self.scan().projects.into_iter().find(|p| discovery::key(&p.path) == key) {
            return Ok(found);
        }
        if !discovery::is_project(&path) {
            return Err(Failure::new(
                "not_a_project",
                format!("{} is not a Unity project (it has no ProjectSettings/ProjectVersion.txt).", path.display()),
                "Call unity_list_projects for the projects on this computer.",
            ));
        }
        let listed = block_on(vpm::registered(&root())).unwrap_or_default().iter().any(|p| discovery::key(p) == key);
        Ok(discovery::describe(path, listed, &mut Default::default()))
    }

    fn present(project: &Project) -> Result<(), Failure> {
        if project.missing || !discovery::is_project(&project.path) {
            return Err(Failure::new(
                "folder_missing",
                format!("{} is not at {} any more.", project.name, project.path.display()),
                "If it is on a disk that is not connected, connect it; else remove it from the list with unity_manage_project.",
            ));
        }
        Ok(())
    }

    fn list_projects(&self, args: &Value, call: &Call) -> Result<Value, Failure> {
        let mut started = None;
        if args["refresh"].as_bool().unwrap_or(false) {
            let cancel = Arc::new(AtomicBool::new(false));
            let (scan_now, scanning, stop) = (self.scan(), self.scan.clone(), cancel.clone());
            let job = self.spawn(Lane::Scan, "Looking for Unity projects on this computer".into(), None, true, Progress::None, cancel, None, &json!({ "wait_s": number(args, "wait_s", 0.0) }), call, move || {
                let publish = |s: &Scan| *scanning.lock().unwrap() = Some(s.clone());
                let found = discovery::discover(scan_now, true, &stop, publish).map_err(|e| failed("scan_failed", e, "Try again; the projects found so far are listed."))?;
                let count = found.projects.len();
                *scanning.lock().unwrap() = Some(found);
                Ok(json!({ "projects": count }))
            })?;
            started = Some(job);
        }
        let scan = self.scan();
        let hidden = hidden::load();
        let include_hidden = args["include_hidden"].as_bool().unwrap_or(false);
        let query = text(args, "query").map(str::to_lowercase);
        let editors = Editors::look();
        let (matrix, trials) = unity6_facts();
        let projects: Vec<Value> = scan
            .projects
            .iter()
            .filter(|p| include_hidden || !hidden::contains(&hidden, &p.path))
            .filter(|p| query.as_ref().is_none_or(|q| orbiters_core::search::Query::new(q).matches(&p.name) || p.path.to_string_lossy().to_lowercase().contains(q)))
            .map(|p| {
                let mut v = project_json(p, &hidden, &editors);
                if !p.missing {
                    v["unity_line"] = unity_line_json(&p.path, &matrix, &trials);
                }
                v
            })
            .collect();
        let scanning = self.jobs.lock().unwrap().list.iter().any(|j| j.lane == Lane::Scan && !j.done());
        Ok(json!({
            "projects": projects,
            "hidden_left_out": if include_hidden { 0 } else { scan.projects.iter().filter(|p| hidden::contains(&hidden, &p.path)).count() },
            "last_full_scan": scan.last_full,
            "scanning": scanning,
            "scan_job": started,
            "unity6_target": unity6_target_json(&matrix),
            "vcc_folder": root(),
        }))
    }

    fn get_project(&self, args: &Value) -> Result<Value, Failure> {
        let project = self.project(args)?;
        let prerelease = args["prerelease"].as_bool().unwrap_or(false);
        let hidden = hidden::load();
        let editors = Editors::look();
        let mut out = project_json(&project, &hidden, &editors);
        if !project.missing {
            let (matrix, trials) = unity6_facts();
            out["unity_line"] = unity_line_json(&project.path, &matrix, &trials);
            out["unity6_target"] = unity6_target_json(&matrix);
        }
        if project.missing {
            out["next"] = json!("Its folder is missing: connect its disk, or remove it from the list with unity_manage_project.");
            return Ok(out);
        }
        let root = root();
        let path = project.path.clone();
        match block_on(vpm::view(&root, Some(&path), false)) {
            Ok(view) => {
                let installed: Vec<Value> = view.packages.iter().filter(|p| p.installed.is_some()).map(|p| package_json(p, prerelease)).collect();
                out["packages"] = json!(installed);
            }
            Err(e) => out["packages_error"] = json!(format!("{e:#}")),
        }
        match block_on(updates::check(&root, &path)) {
            Ok(check) => {
                let ups = check.get(prerelease);
                out["updates"] = json!({
                    "prerelease": prerelease,
                    "available": ups.updates,
                    "held": ups.held,
                    "update_all": ups.all,
                    "skipped": check.skipped,
                });
            }
            Err(e) => out["updates_error"] = json!(format!("{e:#}")),
        }
        let health = storage::inspect(&path);
        out["storage"] = json!({ "health": health, "notice": health.notice(), "needs_repair": health.needs_repair(), "library_moving_job": self.moving_library(&path) });
        out["git"] = json!(git::read(&path));
        let scenes: Vec<&str> = project.avatars.iter().chain(&project.worlds).map(|a| a.scene.as_str()).collect();
        out["blender_files"] = json!(blender::links(&path, &scenes)
            .into_iter()
            .map(|l| json!({ "blend": l.blend, "scene": l.scene, "root": l.root, "tool": l.tool, "modified": l.modified }))
            .collect::<Vec<_>>());
        out["previews"] = json!(orbiters_projects::preview::list(&path)
            .into_iter()
            .map(|e| json!({
                "file": e.path,
                "bytes": e.size,
                "written_utc": e.header.written_utc,
                "unity": e.header.unity,
                "scene": e.header.scene,
                "kind": e.header.kind,
                "stats": e.header.stats,
            }))
            .collect::<Vec<_>>());
        if let (Some(host), false) = (self.host.get(), project.worlds.is_empty()) {
            out["worlds_here"] = json!(project.worlds.iter().map(|w| json!({ "world_id": w.id, "here": host.world_here(&w.id), "uploads": host.uploads(&w.id) })).collect::<Vec<_>>());
        }
        if let Some((change, packages)) = self.missing.lock().unwrap().get(&discovery::key(&path)) {
            out["missing_packages"] = json!({ "packages": packages, "after": format!("{change:?}"), "next": "Call unity_missing_packages to find and add their repositories." });
        }
        Ok(out)
    }

    fn find_packages(&self, args: &Value) -> Result<Value, Failure> {
        let path = match text(args, "path") {
            Some(_) => Some(self.project(args)?.path),
            None => None,
        };
        let prerelease = args["prerelease"].as_bool().unwrap_or(false);
        let query = text(args, "query").map(str::to_lowercase);
        let limit = args["limit"].as_u64().unwrap_or(50).clamp(1, 500) as usize;
        let view = block_on(vpm::view(&root(), path.as_deref(), false)).map_err(|e| failed("vpm_failed", e, "Try unity_repositories refresh, then again."))?;
        let matching: Vec<&vpm::Package> = view
            .packages
            .iter()
            .filter(|p| !p.versions.is_empty() || p.installed.is_some())
            .filter(|p| query.as_ref().is_none_or(|q| orbiters_core::search::Query::new(q).matches_any([p.name.as_str(), p.display_name.as_str()])))
            .collect();
        Ok(json!({
            "total": matching.len(),
            "packages": matching.iter().take(limit).map(|p| package_json(p, prerelease)).collect::<Vec<_>>(),
            "prerelease": prerelease,
            "note": "Only the enabled repositories' packages are listed: unity_repositories turns others on.",
        }))
    }

    // -----------------------------------------------------------------------------------------------------------------
    // Package changes
    // -----------------------------------------------------------------------------------------------------------------

    /// Runs a package change as the tab does (`vpm::change`), as a job: a conflict comes back with its plan, missing
    /// packages are remembered for `unity_missing_packages`.
    fn package_job(&self, project: &Project, change: vpm::Change, label: String, args: &Value, call: &Call) -> Result<Value, Failure> {
        let path = project.path.clone();
        let missing = self.missing.clone();
        let key = discovery::key(&path);
        let live = self.live();
        self.spawn(Lane::Action, label, Some(path.clone()), false, Progress::None, Arc::default(), None, args, call, move || {
            // The person may be in Unity now, though nobody was when the call came: a project an editor has open is not
            // changed under their hands.
            if let Some(paused) = live().filter(|_| Editors::look().of(&path).is_some()) {
                return Err(paused_failure(&paused));
            }
            let root = root();
            let done = block_on(vpm::change(&root, &path, change.clone()));
            Self::package_outcome(done, &root, &path, &key, change, &missing)
        })
    }

    fn package_outcome(
        done: anyhow::Result<()>,
        root: &Path,
        path: &Path,
        key: &str,
        change: vpm::Change,
        missing: &Mutex<HashMap<String, (vpm::Change, Vec<missing::Missing>)>>,
    ) -> Outcome {
        match done {
            Ok(()) => {
                missing.lock().unwrap().remove(key);
                let installed = block_on(vpm::view(root, Some(path), false))
                    .map(|v| v.packages.into_iter().filter_map(|p| Some(json!({ "name": p.name, "version": p.installed? }))).collect::<Vec<_>>())
                    .unwrap_or_default();
                Ok(json!({ "changed": true, "installed": installed, "next": "Unity, if it has the project open, imports the change by itself." }))
            }
            Err(e) => match e.downcast::<coherent::Conflicted>() {
                Ok(conflicted) => Ok(json!({
                    "changed": false,
                    "conflict": true,
                    "plan": conflicted.plan,
                    "solved": conflicted.plan.solved(),
                    "next": if conflicted.plan.solved() {
                        "Nothing was changed. The plan makes it coherent: if it removes packages or changes others, tell the person, then call unity_change_packages with action apply_plan and the plan's install and remove."
                    } else {
                        "Nothing was changed, and no plan solves it as it is: its blockers say which installed packages stand in the way. unity_change_packages with action solve and remove can try without them, except a blocker with embedded set: that one is a folder of the project's Packages, never removed here, and its own package.json holds the range."
                    },
                })),
                Err(e) => {
                    let message = format!("{e:#}");
                    let packages = missing::parse(&message);
                    if !packages.is_empty() {
                        missing.lock().unwrap().insert(key.to_string(), (change, packages.clone()));
                        return Err(Failure::new(
                            "packages_not_listed",
                            message,
                            "No enabled repository lists these packages: call unity_missing_packages with action lookup, then add.",
                        ));
                    }
                    Err(Failure::new("change_failed", message, "Look at the project with unity_get_project; restore puts its locked packages back."))
                }
            },
        }
    }

    fn change_packages(&self, args: &Value, call: &Call) -> Result<Value, Failure> {
        let project = self.project(args)?;
        Self::present(&project)?;
        let prerelease = args["prerelease"].as_bool().unwrap_or(false);
        let action = text(args, "action").unwrap_or_default();
        let package = text(args, "package");
        let needs_package = || package.map(str::to_string).ok_or_else(|| bad(format!("{action} needs package."), "Pass the package's name, as com.vrchat.avatars."));
        let (change, label) = match action {
            "install" => {
                let name = needs_package()?;
                let version = text(args, "version").map(str::to_string);
                let label = format!("Installing {name} {} in {}", version.as_deref().unwrap_or("(newest)"), project.name);
                (vpm::Change::Install { names: vec![(name, version)], prerelease }, label)
            }
            "remove" => {
                let name = needs_package()?;
                (vpm::Change::Remove(name.clone()), format!("Removing {name} from {}", project.name))
            }
            "restore" => (vpm::Change::Resolve, format!("Restoring the missing packages of {}", project.name)),
            "update" | "update_all" => {
                let check = block_on(updates::check(&root(), &project.path)).map_err(|e| failed("updates_failed", e, "Try unity_repositories refresh, then again."))?;
                let ups = check.get(prerelease);
                if action == "update" {
                    let name = needs_package()?;
                    let Some(update) = ups.of(&name) else {
                        let held = ups.held.iter().find(|h| h.name == name);
                        return Err(Failure::new(
                            "no_update",
                            match held {
                                Some(h) => format!("{name} {} exists but cannot be installed as the project is: {}", h.version, h.why),
                                None => format!("{name} has no update{}.", if prerelease { "" } else { " (pre-releases left out)" }),
                            },
                            "unity_get_project lists the updates and those held back.",
                        ));
                    };
                    let label = format!("Updating {name} to {} in {}", update.to, project.name);
                    (vpm::Change::Install { names: vec![(name, Some(update.to.clone()))], prerelease }, label)
                } else {
                    if ups.all.targets.is_empty() {
                        return Ok(json!({ "changed": false, "note": "Everything Update all would change is up to date.", "left_out": ups.all.left_out }));
                    }
                    let names = ups.all.targets.iter().map(|(n, v)| (n.clone(), Some(v.clone()))).collect();
                    (vpm::Change::Install { names, prerelease }, format!("Updating {} packages in {}", ups.all.targets.len(), project.name))
                }
            }
            "apply_plan" => {
                let install = plan_pairs(&args["install"]).ok_or_else(|| bad("Each of install is {name, version} or [name, version].", "Pass the plan's install as it came."))?;
                let remove: Vec<String> = args["remove"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                if install.is_empty() && remove.is_empty() {
                    return Err(bad("apply_plan needs the plan's install and remove.", "Pass them as a conflict's plan gave them."));
                }
                let label = format!("Applying a plan to {}: {} to install, {} to remove", project.name, install.len(), remove.len());
                (vpm::Change::Plan { install, remove, prerelease }, label)
            }
            "solve" => {
                let requested = plan_pairs(&args["install"]).ok_or_else(|| bad("Each of install is {name, version} or [name, version].", "Pass the plan's requested packages."))?;
                if requested.is_empty() {
                    return Err(bad("solve needs install: the {name, version} asked for.", "Pass the plan's requested packages."));
                }
                let remove: Vec<String> = args["remove"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                let plan = block_on(coherent::solve(&root(), &project.path, &requested, &remove, prerelease)).map_err(|e| failed("solve_failed", e, "Look at the project with unity_get_project."))?;
                return Ok(json!({ "plan": plan, "solved": plan.solved(), "next": "Nothing was changed. To apply it: unity_change_packages with action apply_plan and the plan's install and remove." }));
            }
            _ => {
                return Err(bad(
                    "action must be install, remove, update, update_all, restore, apply_plan or solve.",
                    "install with package (and version) adds or changes a version; update and update_all take the updates unity_get_project lists.",
                ))
            }
        };
        self.package_job(&project, change, label, args, call)
    }

    fn missing_packages(&self, args: &Value, call: &Call) -> Result<Value, Failure> {
        let project = self.project(args)?;
        let key = discovery::key(&project.path);
        let (change, packages) = match text(args, "error") {
            Some(error) => (vpm::Change::Resolve, missing::parse(error)),
            None => self.missing.lock().unwrap().get(&key).cloned().unwrap_or((vpm::Change::Resolve, vec![])),
        };
        if packages.is_empty() {
            return Err(Failure::new(
                "nothing_missing",
                "No package change of this project failed for packages no repository lists (or the error given names none).",
                "Pass the error's text in error, or run the change again with unity_change_packages.",
            ));
        }
        let server = Server::current();
        match text(args, "action").unwrap_or("lookup") {
            "lookup" => {
                let lookup = block_on(missing::lookup(&root(), server, &packages)).map_err(|e| failed("lookup_failed", e, "Try again later, or add the repository by URL with unity_repositories."))?;
                Ok(json!({
                    "missing": packages,
                    "found": lookup.found,
                    "not_listed": lookup.not_listed,
                    "not_provided": lookup.not_provided,
                    "repositories_to_add": lookup.additions(),
                    "complete": lookup.complete(),
                    "next": "Call again with action add to add these repositories and run the change again.",
                }))
            }
            "add" => {
                let path = project.path.clone();
                let missing_now = self.missing.clone();
                self.spawn(Lane::Action, format!("Adding the repositories {} needs", project.name), Some(path.clone()), false, Progress::None, Arc::default(), None, args, call, move || {
                    let root = root();
                    let lookup = block_on(missing::lookup(&root, server, &packages)).map_err(|e| failed("lookup_failed", e, "Try again later."))?;
                    let additions = lookup.additions();
                    if additions.is_empty() {
                        return Err(Failure::new(
                            "no_repository_found",
                            "No repository Orbiters knows has these packages at the versions needed.",
                            "Ask the person where they come from; unity_repositories add takes a repository's URL.",
                        ));
                    }
                    if let Err(e) = block_on(missing::add(&root, &additions)) {
                        return Err(failed("add_failed", e, "Look at unity_repositories list."));
                    }
                    let done = block_on(vpm::change(&root, &path, change.clone()));
                    let mut outcome = Self::package_outcome(done, &root, &path, &key, change, &missing_now)?;
                    outcome["repositories_added"] = json!(additions);
                    Ok(outcome)
                })
            }
            _ => Err(bad("action must be lookup or add.", "lookup first, then add.")),
        }
    }

    // -----------------------------------------------------------------------------------------------------------------
    // Repositories
    // -----------------------------------------------------------------------------------------------------------------

    fn repos_json(view: &vpm::View) -> Vec<Value> {
        view.repos.iter().map(|r| json!({ "id": r.id, "name": r.name, "url": r.url, "enabled": r.enabled, "builtin": r.builtin })).collect()
    }

    fn repositories(&self, args: &Value, call: &Call) -> Result<Value, Failure> {
        let action = text(args, "action").unwrap_or("list");
        let root = root();
        let repo = |id: &str| -> Result<vpm::Repository, Failure> {
            let view = block_on(vpm::view(&root, None, false)).map_err(|e| failed("vpm_failed", e, "Try again."))?;
            view.repos.into_iter().find(|r| r.id == id || r.url == id).ok_or_else(|| Failure::new("no_such_repository", format!("No repository has the id or URL {id}."), "Call unity_repositories list."))
        };
        let needs_id = || text(args, "id").ok_or_else(|| bad(format!("{action} needs id."), "Pass the repository's id (or URL) from list."));
        match action {
            "list" => {
                let view = block_on(vpm::view(&root, None, false)).map_err(|e| failed("vpm_failed", e, "Try again."))?;
                Ok(json!({ "repositories": Self::repos_json(&view), "warnings": view.warnings }))
            }
            "add" => {
                let url = text(args, "url").ok_or_else(|| bad("add needs url: the repository's listing (index.json).", "Pass its https URL."))?.to_string();
                let headers: BTreeMap<String, String> =
                    args["headers"].as_object().map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
                let label = format!("Adding the repository {url}");
                self.spawn(Lane::Action, label, None, false, Progress::None, Arc::default(), None, args, call, move || {
                    let root = orbiters_projects::vcc_dir();
                    block_on(vpm::add_repo(&root, &url, headers)).map_err(|e| failed("add_failed", e, "Check the URL: it must be a VPM listing over HTTPS."))?;
                    let view = block_on(vpm::view(&root, None, false)).map_err(|e| failed("vpm_failed", e, "Call unity_repositories list."))?;
                    Ok(json!({ "added": url, "repositories": Self::repos_json(&view) }))
                })
            }
            "remove" | "enable" | "disable" => {
                let r = repo(needs_id()?)?;
                if action == "remove" && r.builtin {
                    return Err(Failure::new("builtin_repository", format!("{} is one of VRChat's own repositories: it can be turned off, not removed.", r.name), "Use disable."));
                }
                let (id, name) = (r.id.clone(), r.name.clone());
                let label = format!("{} {name}", match action { "remove" => "Removing the repository", "enable" => "Turning on", _ => "Turning off" });
                let action = action.to_string();
                self.spawn(Lane::Action, label, None, false, Progress::None, Arc::default(), None, args, call, move || {
                    let root = orbiters_projects::vcc_dir();
                    let done = match action.as_str() {
                        "remove" => block_on(vpm::remove_repo(&root, &id)),
                        a => block_on(vpm::set_enabled(&root, &id, a == "enable")),
                    };
                    done.map_err(|e| failed("change_failed", e, "Call unity_repositories list."))?;
                    Ok(json!({ "repository": name, "done": action }))
                })
            }
            "refresh" => self.spawn(Lane::Action, "Downloading the repositories again".into(), None, false, Progress::None, Arc::default(), None, args, call, move || {
                let view = block_on(vpm::view(&orbiters_projects::vcc_dir(), None, true)).map_err(|e| failed("refresh_failed", e, "Try again later."))?;
                Ok(json!({ "repositories": Self::repos_json(&view), "packages": view.packages.len() }))
            }),
            "suggest" => {
                let catalog = network::catalog(Server::current());
                let mut suggestions = orbiters_projects::suggestions::Suggestions::new(&catalog);
                suggestions.set_query(text(args, "query").unwrap_or_default());
                let listed: Vec<String> = block_on(vpm::view(&root, None, false)).map(|v| v.repos.into_iter().map(|r| r.url).collect()).unwrap_or_default();
                let limit = args["limit"].as_u64().unwrap_or(20).clamp(1, 100) as usize;
                let hits: Vec<Value> = suggestions
                    .hits()
                    .iter()
                    .take(limit)
                    .map(|hit| {
                        let l = suggestions.listing(hit);
                        json!({
                            "name": l.name,
                            "url": l.url,
                            "description": l.description,
                            "owner": l.owner,
                            "packages": l.package_total(),
                            "found_through": hit.because,
                            "added": listed.iter().any(|u| missing::same_url(u, &l.url)),
                        })
                    })
                    .collect();
                Ok(json!({ "total": suggestions.total(), "repositories": hits, "errors": catalog.errors, "next": "unity_repositories add with a url adds one." }))
            }
            _ => Err(bad("action must be list, add, remove, enable, disable, refresh or suggest.", "Pass one of those.")),
        }
    }

    // -----------------------------------------------------------------------------------------------------------------
    // Unity's editor and the project in the list
    // -----------------------------------------------------------------------------------------------------------------

    fn editor(&self, args: &Value) -> Result<Value, Failure> {
        let project = self.project(args)?;
        let action = text(args, "action").unwrap_or("status");
        let editors = Editors::look();
        let open = editors.of(&project.path).cloned();
        let not_open = || Failure::new("unity_not_open", format!("Unity does not have {} open.", project.name), "unity_editor open opens it.");
        match action {
            "status" => Ok(json!({ "project": project.path, "unity_editor": editors.json(&project.path) })),
            "open" => {
                Self::present(&project)?;
                if !project.registered {
                    return Err(Failure::new(
                        "not_listed",
                        format!("{} was found on a disk but is not in the project list, so the tab offers Add before Open.", project.name),
                        "Call unity_manage_project with action add first.",
                    ));
                }
                if let Some(job) = self.moving_library(&project.path) {
                    return Err(Failure::new("library_moving", format!("Job {job} is moving the Library of {}.", project.name), "Unity can open it once that ends (unity_get_job)."));
                }
                let opened = orbiters_projects::open_project_from_bridge(&project).map_err(|e| failed("open_failed", e, "Tell the person what it says: a Unity version to install with Unity Hub, a disk to connect."))?;
                Ok(json!({
                    "opened": true,
                    "already_open": opened.already_open,
                    "library_moved_to": opened.moved,
                    "next": "Unity takes a while to start: unity_editor status says when it has the project open.",
                }))
            }
            "show" => {
                let e = open.ok_or_else(not_open)?;
                editor::show(e.pid, &project.path).map_err(|e| failed("show_failed", e, "Try again."))?;
                Ok(json!({ "shown": true, "pid": e.pid }))
            }
            "save_and_exit" => {
                if let Some(paused) = (self.live())() {
                    return Err(paused_failure(&paused));
                }
                let e = open.ok_or_else(not_open)?;
                editor::close(e.pid, &project.path).map_err(|e| failed("close_failed", e, "Try again, or tell the person."))?;
                Ok(json!({
                    "asked": true,
                    "pid": e.pid,
                    "next": "Unity asks whether to save modified scenes when it has any: that question is the person's. unity_editor status says when it closed.",
                }))
            }
            "kill" => {
                if let Some(paused) = (self.live())() {
                    return Err(paused_failure(&paused));
                }
                let e = open.ok_or_else(not_open)?;
                if args["confirm"].as_bool() != Some(true) {
                    return Err(Failure::new(
                        "confirm_needed",
                        format!("Kill ends Unity at once: what {} has not saved is lost.", project.name),
                        "Prefer save_and_exit. Only if the person agreed, call again with confirm true.",
                    ));
                }
                editor::kill(e.pid, &project.path).map_err(|e| failed("kill_failed", e, "Tell the person."))?;
                Ok(json!({ "killed": true, "pid": e.pid }))
            }
            _ => Err(bad("action must be status, open, show, save_and_exit or kill.", "Pass one of those.")),
        }
    }

    fn manage_project(&self, args: &Value) -> Result<Value, Failure> {
        let action = text(args, "action").unwrap_or_default();
        if action == "open_hub" {
            orbiters_projects::open_hub().map_err(|e| failed("hub_failed", e, "Tell the person: Unity Hub may not be installed."))?;
            return Ok(json!({ "opened": "Unity Hub" }));
        }
        let root = root();
        if action == "remove_from_list" {
            let result = Self::remove_from_list(&root, args)?;
            self.changed();
            return Ok(result);
        }
        let project = self.project(args)?;
        let result = match action {
            "add" => {
                Self::present(&project)?;
                block_on(vpm::register(&root, &project.path, true)).map_err(|e| failed("add_failed", e, "Check that it is a Unity project."))?;
                json!({ "listed": true, "note": "VCC and ALCOM list it too." })
            }
            "hide" | "unhide" => {
                let mut list = hidden::load();
                let key = discovery::key(&project.path);
                if action == "hide" {
                    list.insert(key);
                } else {
                    list.remove(&key);
                }
                // Numbered by time: the window keeps its own count, and saves made here must win over older ones.
                let generation = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
                hidden::save(&list, generation).map_err(|e| failed("save_failed", e, "Try again."))?;
                json!({ "hidden": action == "hide", "note": "Only the tab's lists change: the project stays where it is and in VCC's list." })
            }
            "reveal" => {
                Self::present(&project)?;
                orbiters_projects::reveal(&project.path).map_err(|e| failed("reveal_failed", e, "Tell the person."))?;
                json!({ "opened": project.path })
            }
            "open_blender" => {
                let blend = text(args, "blend").ok_or_else(|| bad("open_blender needs blend: one of the project's Blender files.", "unity_get_project lists them in blender_files."))?;
                let scenes: Vec<&str> = project.avatars.iter().chain(&project.worlds).map(|a| a.scene.as_str()).collect();
                let links = blender::links(&project.path, &scenes);
                let Some(link) = links.iter().find(|l| discovery::key(&l.blend) == discovery::key(Path::new(blend))) else {
                    return Err(Failure::new("not_a_blender_file_of_it", format!("{blend} is not one of {}'s Blender files.", project.name), "unity_get_project lists them in blender_files."));
                };
                let by = blender::open(&link.blend).map_err(|e| failed("blender_failed", e, "Tell the person: Blender may not be installed."))?;
                json!({ "opened": link.blend, "with": by })
            }
            _ => return Err(bad("action must be add, remove_from_list, hide, unhide, reveal, open_hub or open_blender.", "Pass one of those.")),
        };
        self.changed();
        Ok(result)
    }

    /// Takes a listed project out of the shared list: only by the path of one listed (never one guessed at), only with
    /// `confirm`, and its files stay. A project whose folder is gone can be taken out too.
    fn remove_from_list(root: &Path, args: &Value) -> Result<Value, Failure> {
        let given = text(args, "path").ok_or_else(|| bad("remove_from_list needs path: the listed project's own folder.", "Take it from unity_list_projects."))?;
        let given = Path::new(given);
        if !on_a_local_drive(given) {
            return Err(bad(format!("{} is not an absolute path on a drive of this computer.", given.display()), "Pass the listed project's path as unity_list_projects gives it."));
        }
        let listed = block_on(vpm::registered(root)).map_err(|e| failed("vpm_failed", e, "Try again."))?;
        let Some(exact) = the_listed_one(&listed, given) else {
            return Err(Failure::new("not_listed", format!("{} is not in the project list.", given.display()), "unity_list_projects says which projects are listed."));
        };
        let name = exact.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if args["confirm"].as_bool() != Some(true) {
            return Err(Failure::new(
                "confirm_needed",
                format!("This takes {name} out of the list VCC, ALCOM and the tab share. Its files stay where they are."),
                "If the person agreed, call again with confirm true.",
            ));
        }
        block_on(vpm::register(root, exact, false)).map_err(|e| failed("remove_failed", e, "Try again."))?;
        Ok(json!({ "listed": false, "files_kept_at": exact, "note": "Removed from the shared project list. Its files stay." }))
    }

    // -----------------------------------------------------------------------------------------------------------------
    // New projects and copies
    // -----------------------------------------------------------------------------------------------------------------

    fn list_templates(&self) -> Result<Value, Failure> {
        let root = root();
        let list = templates::list(&root);
        let default_location = block_on(vpm::view(&root, None, false)).map(|v| v.default_path).ok();
        Ok(json!({
            "templates": list.iter().map(|t| json!({
                "id": t.id,
                "name": t.name,
                "unity": t.unity,
                "builtin": t.builtin,
                "unity_installed": (!t.unity.is_empty()).then(|| discovery::find_editor(&t.unity).is_some()),
            })).collect::<Vec<_>>(),
            "default_location": default_location,
            "note": "A template without its own Unity version needs unity: the exact editor version installed, e.g. 2022.3.22f1.",
        }))
    }

    fn create_project(&self, args: &Value, call: &Call) -> Result<Value, Failure> {
        let root = root();
        let name = text(args, "name").ok_or_else(|| bad("name is needed: the new project's folder name.", "Pass a name."))?.to_string();
        templates::validate_name(&name).map_err(|e| bad(e.to_string(), "Choose another name."))?;
        let location = match text(args, "location") {
            Some(l) => PathBuf::from(l),
            None => block_on(vpm::view(&root, None, false)).map(|v| v.default_path).map_err(|e| failed("vpm_failed", e, "Pass location."))?,
        };
        if !on_a_local_drive(&location) {
            return Err(bad("location must be an absolute folder on a drive of this computer.", "Pass where the project's folder goes."));
        }
        if location.join(&name).exists() {
            return Err(Failure::new("exists", format!("{} has a folder named {name} already.", location.display()), "Choose another name."));
        }
        let wanted = text(args, "template").ok_or_else(|| bad("template is needed.", "Pass a template's id or name from unity_list_templates."))?;
        let list = templates::list(&root);
        let template = list
            .iter()
            .find(|t| t.id == wanted)
            .or_else(|| list.iter().find(|t| t.name.eq_ignore_ascii_case(wanted)))
            .ok_or_else(|| Failure::new("no_such_template", format!("No template is called {wanted}."), "Call unity_list_templates."))?;
        let unity = if template.unity.is_empty() { text(args, "unity").unwrap_or_default().to_string() } else { template.unity.clone() };
        if unity.is_empty() {
            return Err(bad(format!("{} names no Unity version: pass unity.", template.name), "The exact editor version installed, e.g. 2022.3.22f1."));
        }
        let source = template.path.clone();
        let progress = templates::SharedProgress::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let (p, c) = (progress.clone(), cancel.clone());
        let label = format!("Creating {name} from {}", template.name);
        self.spawn(Lane::Action, label, Some(location.join(&name)), true, Progress::Copy(progress), cancel, None, args, call, move || {
            let path = block_on(templates::create(&orbiters_projects::vcc_dir(), &source, &location, &name, &unity, false, &c, &p))
                .map_err(|e| failed("create_failed", e, "Read the message: a folder kept is said there."))?;
            Ok(json!({ "path": path, "listed": true, "next": "unity_editor open opens it in Unity." }))
        })
    }

    fn duplicate_project(&self, args: &Value, call: &Call) -> Result<Value, Failure> {
        let project = self.project(args)?;
        Self::present(&project)?;
        if let Some(job) = self.moving_library(&project.path) {
            return Err(Failure::new("library_moving", format!("Job {job} is moving this project's Library."), "Duplicate it once that ends."));
        }
        let name = text(args, "name").map(str::to_string).unwrap_or_else(|| format!("{} copy", project.name));
        templates::validate_name(&name).map_err(|e| bad(e.to_string(), "Choose another name."))?;
        let location = text(args, "location").map(PathBuf::from).or_else(|| project.path.parent().map(Path::to_path_buf)).unwrap_or_default();
        if !on_a_local_drive(&location) {
            return Err(bad("location must be an absolute folder on a drive of this computer.", "Pass where the copy goes."));
        }
        let keep = templates::Keep { library: args["library"].as_bool().unwrap_or(false) };
        // What the tab's dialog checks before it lets Duplicate be pressed (the copy checks again before copying).
        let exists = location.join(&name).exists();
        let needed = templates::duplication_bytes(&project.path, keep).map_err(|e| failed("measure_failed", e, "Try again."))?;
        let free = size::free_space(&location);
        let check = json!({ "copy_at": location.join(&name), "exists": exists, "bytes_needed": needed, "bytes_free": free, "library": keep.library });
        let problem = if exists {
            Some(format!("{} has a folder named {name} already.", location.display()))
        } else if free.is_none() {
            Some("The free space of that disk cannot be read: choose an accessible destination.".to_string())
        } else if free.is_some_and(|f| f < needed) {
            Some(format!("The copy needs about {needed} bytes, and that disk has {} left.", free.unwrap_or(0)))
        } else {
            None
        };
        if args["check_only"].as_bool().unwrap_or(false) {
            let mut check = check;
            check["ok"] = json!(problem.is_none());
            check["problem"] = json!(problem);
            return Ok(check);
        }
        if let Some(problem) = problem {
            return Err(Failure::new("cannot_duplicate", problem, "Choose another name, or another disk with location."));
        }
        let source = project.path.clone();
        let progress = templates::SharedProgress::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let (p, c) = (progress.clone(), cancel.clone());
        let label = format!("Duplicating {} as {name}", project.name);
        self.spawn(Lane::Action, label, Some(location.join(&name)), true, Progress::Copy(progress), cancel, None, args, call, move || {
            let path = block_on(templates::duplicate(&orbiters_projects::vcc_dir(), &source, &location, &name, keep, &c, &p))
                .map_err(|e| failed("duplicate_failed", e, "Read the message: a copy kept and listed is said there, and unity_list_projects shows it."))?;
            Ok(json!({ "path": path, "listed": true, "note": "Its scenes keep their blueprint IDs: uploads from it update the same avatars and worlds." }))
        })
    }

    // -----------------------------------------------------------------------------------------------------------------
    // Storage
    // -----------------------------------------------------------------------------------------------------------------

    fn storage(&self, args: &Value, call: &Call) -> Result<Value, Failure> {
        let project = self.project(args)?;
        Self::present(&project)?;
        let path = project.path.clone();
        match text(args, "action").unwrap_or("check") {
            "check" => {
                let health = storage::inspect(&path);
                Ok(json!({ "health": health, "notice": health.notice(), "needs_repair": health.needs_repair(), "size": size::lookup(&path).size, "library_moving_job": self.moving_library(&path) }))
            }
            "plan" => {
                let progress = storage::SharedProgress::default();
                let cancel = Arc::new(AtomicBool::new(false));
                let (p, c) = (progress.clone(), cancel.clone());
                self.spawn(Lane::Storage, format!("Measuring the Library of {}", project.name), Some(path.clone()), true, Progress::Storage(progress), cancel, None, args, call, move || {
                    match storage::plan(&path, &p, &c).map_err(|e| failed("plan_failed", e, "Try again."))? {
                        Some(plan) => Ok(json!({ "plan": plan, "next": "If its verdict is Ready and the person agrees, unity_storage repair moves it." })),
                        None => Ok(json!({ "cancelled": true })),
                    }
                })
            }
            "repair" => {
                let progress = storage::SharedProgress::default();
                let cancel = Arc::new(AtomicBool::new(false));
                let (p, c) = (progress.clone(), cancel.clone());
                self.spawn(Lane::Storage, format!("Moving the Library of {}", project.name), Some(path.clone()), true, Progress::Storage(progress), cancel, None, args, call, move || {
                    let end = storage::repair(&path, &p, &c).map_err(|e| failed("repair_failed", e, "The Library stays where it was: read the message."))?;
                    Ok(json!({ "end": end, "next": "Open it in Unity when ready (unity_editor open)." }))
                })
            }
            "measure" => {
                let cancel = Arc::new(AtomicBool::new(false));
                let c = cancel.clone();
                self.spawn(Lane::Measure, format!("Measuring {}", project.name), Some(path.clone()), true, Progress::None, cancel, None, args, call, move || {
                    let s = size::measure(&path, &c).map_err(|e| failed("measure_failed", e, "Try again."))?;
                    Ok(json!({ "total": s.total, "library": s.library, "files": s.files, "without_library": s.without_library() }))
                })
            }
            _ => Err(bad("action must be check, plan, repair or measure.", "Pass one of those.")),
        }
    }

    // -----------------------------------------------------------------------------------------------------------------
    // Orbiters' packages
    // -----------------------------------------------------------------------------------------------------------------

    fn add_orbiters(&self, args: &Value, call: &Call) -> Result<Value, Failure> {
        let project = self.project(args)?;
        Self::present(&project)?;
        let what = text(args, "what").unwrap_or_default().to_string();
        let path = project.path.clone();
        let name = project.name.clone();
        let key = discovery::key(&path);
        let missing = self.missing.clone();
        let server = Server::current();
        let live = self.live();
        let open_unity = args["open_unity"].as_bool().unwrap_or(what == "toolkit");
        let (label, avatar) = match what.as_str() {
            "mixer" => (format!("Adding Mixer to {name}"), None),
            "toolkit" => (format!("Updating Orbiters Toolkit in {name}"), None),
            "my_avatar" => (format!("Installing My Avatar in {name}"), None),
            "n4n0" => {
                if !project.registered {
                    return Err(Failure::new("not_listed", format!("{name} is not in the project list."), "unity_manage_project add lists it first."));
                }
                if server.n4n0_gallery_asset_id().is_none() {
                    return Err(Failure::new("n4n0_not_published", "N4N0 has not been published on this server's gallery yet. My Avatar can still be installed.", "Use what my_avatar."));
                }
                let id = text(args, "blueprint_id").ok_or_else(|| bad("n4n0 needs blueprint_id: one of the project's avatars.", "unity_get_project lists them under avatars."))?;
                let avatar = project.avatars.iter().find(|a| a.id == id).cloned().ok_or_else(|| {
                    Failure::new("not_in_the_project", format!("{id} is not one of {name}'s avatars."), "unity_get_project lists them under avatars.")
                })?;
                (format!("Installing My Avatar in {name} and preparing N4N0 for {}", avatar.name), Some(avatar))
            }
            _ => return Err(bad("what must be mixer, toolkit, my_avatar or n4n0.", "Pass one of those.")),
        };
        // Everything that would be refused at the end is refused before anything is done.
        let dir = orbiters_projects::data_dir();
        use unity6::authority::{allow, Action, Caller};
        let refuse = |why: String| Failure::new("refused", why, "Tell the person: nothing was done.");
        allow(&dir, &path, Action::Packages).map_err(refuse)?;
        if open_unity {
            allow(&dir, &path, Action::Open(Caller::Bridge)).map_err(refuse)?;
        }
        let request: Arc<Mutex<Option<String>>> = Arc::default();
        let probe = {
            let (request, path) = (request.clone(), path.clone());
            Box::new(move || match request.lock().unwrap().as_deref() {
                Some(id) => match orbiters_projects::setup::read_result(&path, id) {
                    Ok(Some(r)) => json!({ "setup": r.status, "message": r.message, "installed": r.installed.iter().map(|i| json!({ "asset_id": i.asset_id, "version": i.version })).collect::<Vec<_>>() }),
                    Ok(None) => json!({ "setup": "waiting", "unity_open": orbiters_projects::unity_open(&path), "next": "The Toolkit in Unity finishes it: open the project (unity_editor open) if it is not, and look again." }),
                    Err(e) => json!({ "setup": "unreadable", "error": e.to_string() }),
                },
                None => Value::Null,
            }) as Box<dyn Fn() -> Value + Send + Sync>
        };
        let project_now = project.clone();
        self.spawn(Lane::Action, label, Some(path.clone()), false, Progress::None, Arc::default(), Some(probe), args, call, move || {
            let root = root();
            // What leads to an upload from VRChat's SDK panel asks leave for it first (Mixer does not).
            let leave = match what.as_str() {
                "mixer" => None,
                _ => Some(unity6::authority::upload(&dir, &path).map_err(|why| Failure::new("refused", why, "Tell the person: nothing was done."))?),
            };
            let installed = match what.as_str() {
                "mixer" => block_on(vpm::install_orbiters(&root, &path, &[MIXER])),
                "toolkit" => block_on(vpm::install_orbiters(&root, &path, &[TOOLKIT])),
                _ => block_on(vpm::install_my_avatar(&root, &path)),
            };
            let names: &[&str] = match what.as_str() {
                "mixer" => &[MIXER],
                "toolkit" => &[TOOLKIT],
                _ => &[TOOLKIT, "orbiters.myavatar"],
            };
            let change = vpm::Change::Install { names: names.iter().map(|n| (n.to_string(), None)).collect(), prerelease: false };
            let mut outcome = Self::package_outcome(installed, &root, &path, &key, change, &missing)?;
            if outcome["conflict"] == json!(true) {
                return Ok(outcome);
            }
            // Asked again now that its packages changed.
            if let Some(leave) = &leave {
                leave.still(&path).map_err(|why| Failure::new("refused", why, "Its packages are installed; nothing else was done. Tell the person."))?;
            }
            if let Some(warning) = leave.as_ref().and_then(|l| l.warning.as_ref()) {
                outcome["upload_warning"] = json!(warning);
            }
            if let (Some(avatar), Some(asset), Some(leave)) = (avatar, server.n4n0_gallery_asset_id(), &leave) {
                let r = orbiters_projects::setup::request(avatar, asset, server.api(), env!("ORBITERS_VERSION")).map_err(|e| failed("setup_failed", e, "Tell the person."))?;
                orbiters_projects::setup::write(&path, &r, leave).map_err(|e| failed("setup_failed", e, "Tell the person."))?;
                outcome["setup_request"] = json!(r.id);
                outcome["next"] = json!("The Toolkit in Unity adds N4N0 to the avatar and saves the scene: open the project (unity_editor open), then unity_get_job shows the setup's result under result.now.");
                *request.lock().unwrap() = Some(r.id);
            }
            if open_unity && live().is_some() {
                // The person came back meanwhile: Unity is not opened in front of them.
                outcome["unity"] = json!({ "opened": false, "error": "the person is using the computer: open it later with unity_editor open" });
            } else if open_unity {
                match orbiters_projects::open_project_from_bridge(&project_now) {
                    Ok(opened) => outcome["unity"] = json!({ "opened": true, "already_open": opened.already_open }),
                    Err(e) => outcome["unity"] = json!({ "opened": false, "error": format!("{e:#}") }),
                }
            }
            Ok(outcome)
        })
    }

    // -----------------------------------------------------------------------------------------------------------------
    // Worlds
    // -----------------------------------------------------------------------------------------------------------------

    fn world(&self, args: &Value) -> Result<Value, Failure> {
        let project = self.project(args)?;
        let host = self.host()?;
        let id = text(args, "world_id").ok_or_else(|| bad("world_id is needed.", "unity_get_project lists the project's worlds."))?;
        let world = project.worlds.iter().find(|w| w.id == id).ok_or_else(|| {
            Failure::new("not_in_the_project", format!("{id} is not one of {}'s uploaded worlds.", project.name), "unity_get_project lists them under worlds.")
        })?;
        let here = host.world_here(id);
        let name = here["name"].as_str().filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| world.name.clone());
        match text(args, "action").unwrap_or("status") {
            "status" => Ok(json!({ "world_id": id, "name": name, "here": here, "uploads": host.uploads(id), "page": format!("https://vrchat.com/home/world/{id}") })),
            "add_pictures" => {
                let files: Vec<PathBuf> = args["files"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(PathBuf::from)).collect()).unwrap_or_default();
                if files.is_empty() {
                    return Err(bad("add_pictures needs files: pictures of this world on this computer.", "Pass their paths."));
                }
                // Only pictures VRChat took in this world, as this computer lists them (BRIDGE-7): never any other file.
                let theirs: Vec<PathBuf> = here["picture_files"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).filter_map(|p| std::fs::canonicalize(p).ok()).collect())
                    .unwrap_or_default();
                if let Some(f) = files.iter().find(|f| !on_a_local_drive(f) || std::fs::canonicalize(f).ok().is_none_or(|c| !theirs.contains(&c))) {
                    return Err(Failure::new(
                        "not_a_picture_of_it",
                        format!("{} is not one of the pictures VRChat took in this world on this computer.", f.display()),
                        "unity_world status lists them in here.picture_files.",
                    ));
                }
                // The verified files as the disk has them (the gallery's own paths), never the spelling given.
                let files: Vec<PathBuf> = files.iter().map(|f| real_path(f)).collect();
                host.upload_pictures(id.to_string(), name, files.clone()).map_err(|e| Failure::new("upload_failed", e, "Tell the person: they may have to sign in to Orbiters."))?;
                self.changed();
                Ok(json!({ "sending": files.len(), "next": "unity_world status shows how the upload goes." }))
            }
            "set_preview" => {
                let session = text(args, "session").ok_or_else(|| bad("set_preview needs session: one of this world's captures.", "unity_world status lists them in here.captures."))?;
                let known = here["captures"].as_array().is_some_and(|c| c.iter().any(|c| c["dir"].as_str().is_some_and(|d| discovery::key(Path::new(d)) == discovery::key(Path::new(session)))));
                if !known {
                    return Err(Failure::new("not_a_capture_of_it", format!("{session} is not a capture recorded in this world."), "unity_world status lists them in here.captures."));
                }
                host.upload_preview(id.to_string(), name, PathBuf::from(session)).map_err(|e| Failure::new("upload_failed", e, "Tell the person: reconstruct it first, or sign in to Orbiters."))?;
                self.changed();
                Ok(json!({ "sending": session, "note": "Its splats become the world's 3D preview on Orbiters, in place of an earlier one of this account.", "next": "unity_world status shows how the upload goes." }))
            }
            _ => Err(bad("action must be status, add_pictures or set_preview.", "Pass one of those.")),
        }
    }
}

/// The tools as `tools/list` gives them, after the VRChat ones.
/// The packages of `install` as the schema takes them: each {name, version}, or [name, version] as a conflict's plan gives
/// it back (`coherent::Plan::install`); none when it is absent. `None` when an item is neither.
pub(crate) fn plan_pairs(install: &Value) -> Option<Vec<(String, String)>> {
    let Some(items) = install.as_array() else { return Some(Vec::new()) };
    items
        .iter()
        .map(|v| match v {
            Value::Object(_) => Some((v["name"].as_str()?.to_string(), v["version"].as_str()?.to_string())),
            Value::Array(pair) if pair.len() == 2 => Some((pair[0].as_str()?.to_string(), pair[1].as_str()?.to_string())),
            _ => None,
        })
        .collect()
}

pub fn list() -> Vec<Value> {
    let read_only = json!({ "readOnlyHint": true, "destructiveHint": false, "openWorldHint": false });
    let acts = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false });
    let destructive = json!({ "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false });
    let path = json!({ "type": "string", "description": "The project's folder, as unity_list_projects gives it." });
    let wait = json!({ "type": "number", "minimum": 0, "maximum": 50, "description": "How long to wait for the job, in seconds. Default 30." });
    let prerelease = json!({ "type": "boolean", "description": "With pre-release versions (the tab's switch). Default false." });
    // Each pair as {name, version}, or as the conflict's plan gives it back: [name, version] (`coherent::Plan::install`).
    let pairs = json!({ "type": "array", "items": { "anyOf": [
        { "type": "object", "properties": { "name": { "type": "string" }, "version": { "type": "string" } }, "required": ["name", "version"] },
        { "type": "array", "items": { "type": "string" }, "minItems": 2, "maxItems": 2 },
    ] } });
    vec![
        json!({
            "name": "unity_list_projects",
            "title": "List the Unity projects",
            "description": "The Unity projects on this computer, as the Unity projects tab lists them: name, path, Unity version, kind (avatars, worlds, vrchat, unity), whether VCC lists it (listed) or it was only found on a disk, hidden, its avatars and worlds with their blueprint ids, whether Unity has it open, and its size when measured. refresh true looks through the disks again in a job.",
            "inputSchema": { "type": "object", "properties": {
                "refresh": { "type": "boolean", "description": "Look for projects again (a job; the list returned is what is known now)." },
                "include_hidden": { "type": "boolean", "description": "With the projects hidden in the tab." },
                "query": { "type": "string", "description": "Words in the name or path." },
                "wait_s": wait,
            }, "additionalProperties": false },
            "annotations": read_only,
        }),
        json!({
            "name": "unity_get_project",
            "title": "Look at a Unity project",
            "description": "Everything the tab's panel shows of one project: installed packages with their versions and the versions available, updates and the updates held back (with why), Update all's plan, Unity's editor, its disk and whether its Library should move, its Git repository and last commits, its Blender files, its 3D previews, its worlds' captures and uploads, and missing packages a change failed for.",
            "inputSchema": { "type": "object", "properties": { "path": path, "prerelease": prerelease }, "required": ["path"], "additionalProperties": false },
            "annotations": read_only,
        }),
        json!({
            "name": "unity_find_packages",
            "title": "Find VPM packages",
            "description": "The packages the enabled VPM repositories offer, with their versions (newest first) and, with path, the version the project has. Use it before installing with unity_change_packages.",
            "inputSchema": { "type": "object", "properties": {
                "query": { "type": "string", "description": "Words in the package's name or display name." },
                "path": path,
                "prerelease": prerelease,
                "limit": { "type": "integer", "minimum": 1, "maximum": 500, "description": "Default 50." },
            }, "additionalProperties": false },
            "annotations": read_only,
        }),
        json!({
            "name": "unity_change_packages",
            "title": "Change a project's packages",
            "description": "Changes a project's VPM packages through vrc-get, as the tab does, in a job: install (package, with version or the newest: also changes an installed one's version), remove, update (one package to its update), update_all (Update all's plan, the VRChat SDK left out), restore (the locked packages missing from Packages). A change that conflicts changes nothing and returns the plan that makes it coherent: apply_plan with its install and remove applies it (rolled back if it fails); solve makes a plan without applying it.",
            "inputSchema": { "type": "object", "properties": {
                "path": path,
                "action": { "type": "string", "enum": ["install", "remove", "update", "update_all", "restore", "apply_plan", "solve"] },
                "package": { "type": "string", "description": "The package's name, as com.vrchat.avatars." },
                "version": { "type": "string", "description": "For install: an exact version. Default the newest compatible." },
                "prerelease": prerelease,
                "install": pairs,
                "remove": { "type": "array", "items": { "type": "string" }, "description": "For apply_plan and solve: packages to remove." },
                "wait_s": wait,
            }, "required": ["path", "action"], "additionalProperties": false },
            "annotations": destructive,
        }),
        json!({
            "name": "unity_missing_packages",
            "title": "Find the repositories of missing packages",
            "description": "After a package change failed with packages_not_listed (no enabled repository has them): lookup finds which repositories have them (Orbiters' catalog of known ones, the person's turned-off ones, Orbiters listings) and at which versions; add adds them (turning the person's back on) and runs the change that failed again, in a job.",
            "inputSchema": { "type": "object", "properties": {
                "path": path,
                "action": { "type": "string", "enum": ["lookup", "add"], "description": "Default lookup." },
                "error": { "type": "string", "description": "The failure's message, when it did not come from these tools." },
                "wait_s": wait,
            }, "required": ["path"], "additionalProperties": false },
            "annotations": acts,
        }),
        json!({
            "name": "unity_repositories",
            "title": "VPM repositories",
            "description": "The VPM repositories VCC, ALCOM and the tab share: list; add a listing by its URL (with headers if it needs them); remove one of the person's; enable or disable one (disabled ones stay in VCC's settings, their packages are left out); refresh downloads every repository again; suggest searches Orbiters' catalog of known repositories.",
            "inputSchema": { "type": "object", "properties": {
                "action": { "type": "string", "enum": ["list", "add", "remove", "enable", "disable", "refresh", "suggest"], "description": "Default list." },
                "url": { "type": "string", "description": "For add: the listing's URL (index.json)." },
                "headers": { "type": "object", "additionalProperties": { "type": "string" }, "description": "For add: HTTP headers the repository needs." },
                "id": { "type": "string", "description": "For remove, enable, disable: the repository's id or URL from list." },
                "query": { "type": "string", "description": "For suggest: words in a repository's name, owner or packages." },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 },
                "wait_s": wait,
            }, "additionalProperties": false },
            "annotations": acts,
        }),
        json!({
            "name": "unity_editor",
            "title": "Unity's editor",
            "description": "A project in Unity: status (whether Unity has it open, and is waiting on a dialog); open (in its own Unity version, as Open in Unity: a Library on a full disk that barely exists moves first; refused while the person uses the computer); show (brings it forward); save_and_exit (asks Unity to quit as its close button does: it offers to save modified scenes); kill (ends it at once, unsaved work lost: needs confirm).",
            "inputSchema": { "type": "object", "properties": {
                "path": path,
                "action": { "type": "string", "enum": ["status", "open", "show", "save_and_exit", "kill"], "description": "Default status." },
                "confirm": { "type": "boolean", "description": "For kill: the person agreed." },
            }, "required": ["path"], "additionalProperties": false },
            "annotations": destructive,
        }),
        json!({
            "name": "unity_manage_project",
            "title": "Manage a project in the list",
            "description": "add: puts a project found on a disk in the list VCC, ALCOM and the tab share. remove_from_list: takes a listed project out of it (its files stay; needs its exact path and confirm). hide, unhide: in the tab's lists only. reveal: its folder in Explorer. open_hub: Unity Hub. open_blender: one of its Blender files (blend) in Blender.",
            "inputSchema": { "type": "object", "properties": {
                "path": path,
                "action": { "type": "string", "enum": ["add", "remove_from_list", "hide", "unhide", "reveal", "open_hub", "open_blender"] },
                "confirm": { "type": "boolean", "description": "For remove_from_list: the person agreed." },
                "blend": { "type": "string", "description": "For open_blender: the file, from unity_get_project's blender_files." },
            }, "required": ["action"], "additionalProperties": false },
            "annotations": destructive,
        }),
        json!({
            "name": "unity_list_templates",
            "title": "List the project templates",
            "description": "The templates a new project can start from (Creator Companion's and ALCOM's, built-in and the person's), with their Unity version and whether that version is installed, and the default folder for new projects. For unity_create_project.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read_only,
        }),
        json!({
            "name": "unity_create_project",
            "title": "Create a Unity project",
            "description": "A new project from a template, in location/name: copied, its packages restored, and listed in VCC and ALCOM, in a job (unity_cancel_job stops the copy).",
            "inputSchema": { "type": "object", "properties": {
                "name": { "type": "string", "description": "The folder's name." },
                "template": { "type": "string", "description": "A template's id or name from unity_list_templates." },
                "location": { "type": "string", "description": "The folder it goes in. Default the default folder of new projects." },
                "unity": { "type": "string", "description": "For a template without its own: the exact Unity version, e.g. 2022.3.22f1." },
                "wait_s": wait,
            }, "required": ["name", "template"], "additionalProperties": false },
            "annotations": acts,
        }),
        json!({
            "name": "unity_duplicate_project",
            "title": "Duplicate a Unity project",
            "description": "An independent copy of a project with its installed packages at their exact versions (repositories that are gone included), listed in VCC and ALCOM, as the tab's Duplicate: \"<name> copy\" beside it unless told otherwise, Library taken along with library true. check_only says what it needs and the room on that disk without copying. Its scenes keep their blueprint IDs. A job; unity_cancel_job stops the copy.",
            "inputSchema": { "type": "object", "properties": {
                "path": path,
                "name": { "type": "string" },
                "location": { "type": "string", "description": "The folder the copy goes in. Default the one the project is in." },
                "library": { "type": "boolean", "description": "Copy Library too: opens at once instead of importing everything again. Default false." },
                "check_only": { "type": "boolean" },
                "wait_s": wait,
            }, "required": ["path"], "additionalProperties": false },
            "annotations": acts,
        }),
        json!({
            "name": "unity_storage",
            "title": "A project's disk and Library",
            "description": "check: the room on the project's disk, whether Unity ran out of it, whether its Library should move. plan: measures the Library and says where it could go (a job, nothing changed). repair: moves the Library to a disk with room, copied and checked before the switch, the original kept on failure (a job that can take long; cancellable until the switch). measure: the project's size on disk (a job).",
            "inputSchema": { "type": "object", "properties": {
                "path": path,
                "action": { "type": "string", "enum": ["check", "plan", "repair", "measure"], "description": "Default check." },
                "wait_s": wait,
            }, "required": ["path"], "additionalProperties": false },
            "annotations": acts,
        }),
        json!({
            "name": "unity_add_orbiters",
            "title": "Add Orbiters' packages",
            "description": "From the Orbiters repository (added or turned on first): mixer adds Mixer; toolkit installs the current Orbiters Toolkit and opens the project in Unity; my_avatar installs the Toolkit and My Avatar; n4n0 does that and asks the Toolkit in Unity to add N4N0 to the avatar blueprint_id (the job's result.now follows the setup). A job.",
            "inputSchema": { "type": "object", "properties": {
                "path": path,
                "what": { "type": "string", "enum": ["mixer", "toolkit", "my_avatar", "n4n0"] },
                "blueprint_id": { "type": "string", "description": "For n4n0: the avatar's avtr_ id, from unity_get_project's avatars." },
                "open_unity": { "type": "boolean", "description": "Open the project in Unity afterwards. Default true for toolkit only." },
                "wait_s": wait,
            }, "required": ["path", "what"], "additionalProperties": false },
            "annotations": acts,
        }),
        json!({
            "name": "unity_world",
            "title": "A world project's world on Orbiters",
            "description": "For a world a world project was uploaded as: status (what this computer has of it: captures and VRChat pictures, and its uploads to Orbiters' Map explorer); add_pictures (pictures VRChat took in this world on this computer, as status lists them in here.picture_files); set_preview (a reconstructed capture recorded there, by its session folder, becomes the world's 3D preview, in place of an earlier one). Needs the person signed in to Orbiters.",
            "inputSchema": { "type": "object", "properties": {
                "path": path,
                "world_id": { "type": "string", "description": "wrld_… from unity_get_project's worlds." },
                "action": { "type": "string", "enum": ["status", "add_pictures", "set_preview"], "description": "Default status." },
                "files": { "type": "array", "items": { "type": "string" } },
                "session": { "type": "string", "description": "For set_preview: a capture's dir from status." },
            }, "required": ["path", "world_id"], "additionalProperties": false },
            "annotations": acts,
        }),
        json!({
            "name": "unity_get_job",
            "title": "Follow a Unity projects job",
            "description": "A job's state (running, done, failed), progress, and result or failure; without job, the recent ones. wait_s waits for it to end.",
            "inputSchema": { "type": "object", "properties": {
                "job": { "type": "integer", "minimum": 1 },
                "wait_s": { "type": "number", "minimum": 0, "maximum": 50, "description": "Default 0." },
            }, "additionalProperties": false },
            "annotations": read_only,
        }),
        json!({
            "name": "unity_cancel_job",
            "title": "Stop a Unity projects job",
            "description": "Stops a copy (new project, duplicate), a Library move before its switch, a scan or a size, where the tab's Stop or Cancel does; a package or repository change cannot be stopped half way.",
            "inputSchema": { "type": "object", "properties": { "job": { "type": "integer", "minimum": 1 } }, "required": ["job"], "additionalProperties": false },
            "annotations": acts,
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn only_paths_on_a_drive_of_this_computer_are_taken() {
        // BRIDGE-7: a network path's first look opens a connection to a host the agent's text chose.
        assert!(on_a_local_drive(Path::new(r"C:\Users\Enzo\Projects\World")));
        assert!(on_a_local_drive(Path::new(r"C:\no\such\folder\yet")), "a folder to make, on a local disk");
        for path in [r"\\evil.example\share\x", r"\\?\C:\x", r"\\.\pipe\x", r"\\?\UNC\host\share", r"relative\x", r"C:relative"] {
            assert!(!on_a_local_drive(Path::new(path)), "{path}");
        }
    }

    fn call() -> Call {
        Call { client_gone: Arc::new(AtomicBool::new(false)), stop_at_start: 0 }
    }

    #[test]
    fn the_projects_tools_are_listed_in_order_and_say_when_to_use_them() {
        let tools = list();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, NAMES);
        for tool in &tools {
            assert!(tool["description"].as_str().unwrap().len() > 60, "{tool}");
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            let read_only = tool["annotations"]["readOnlyHint"].as_bool().unwrap();
            assert_eq!(read_only, reads(tool["name"].as_str().unwrap()), "{}", tool["name"]);
        }
        for words in ["unity_change_packages", "apply_plan", "unity_get_job", "confirm", "Nothing here deletes"] {
            assert!(GUIDE.contains(words), "{words}");
        }
    }

    /// The person's pause: a held one stops every change and leaves reading; their own input stops only what opens a window.
    #[test]
    fn a_pause_stops_changes_and_windows_but_not_reading() {
        let projects = Projects::default();
        let held = projects.call("unity_change_packages", &json!({ "path": "C:/x", "action": "restore" }), &call(), Some(Paused::Held));
        assert_eq!(held.err().unwrap().code, "paused_held");
        let busy = Some(Paused::Person { resumes_in_s: 10 });
        let opening = projects.call("unity_editor", &json!({ "path": "C:/x", "action": "open" }), &call(), busy.clone());
        assert_eq!(opening.err().unwrap().code, "paused_by_person");
        // Reading goes on (and fails here only for the made-up path).
        let reading = projects.call("unity_get_project", &json!({ "path": "Z:/no/such/project" }), &call(), Some(Paused::Held));
        assert_eq!(reading.err().unwrap().code, "not_a_project");
        let jobs = projects.call("unity_get_job", &json!({}), &call(), Some(Paused::Held));
        assert!(jobs.is_ok());
        // verify-n4n0 (6): nobody at the computer when the call came, the person back before it acts: still refused.
        let dir = std::env::temp_dir().join(format!("orbiters-unity-live-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("ProjectSettings")).unwrap();
        std::fs::write(dir.join("ProjectSettings").join("ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
        let live = Projects::default();
        live.set_presence(Arc::new(|| Some(Paused::Person { resumes_in_s: 10 })));
        let path = dir.to_string_lossy().to_string();
        for action in ["kill", "save_and_exit"] {
            let ending = live.call("unity_editor", &json!({ "path": path, "action": action, "confirm": true }), &call(), None);
            assert_eq!(ending.err().unwrap().code, "paused_by_person", "{action}: looked at again when it acts");
        }
        // The Toolkit opens Unity unless asked not to: the person's input stops that too, without open_unity said.
        let toolkit = projects.call("unity_add_orbiters", &json!({ "path": "C:/x", "what": "toolkit" }), &call(), busy.clone());
        assert_eq!(toolkit.err().unwrap().code, "paused_by_person");
        let _ = std::fs::remove_dir_all(&dir);
        // BRIDGE-8: the person at the computer may be in Unity: what ends an editor waits for them too, and so does a change
        // that would open Unity.
        for action in ["kill", "save_and_exit"] {
            let ending = projects.call("unity_editor", &json!({ "path": "C:/x", "action": action, "confirm": true }), &call(), busy.clone());
            assert_eq!(ending.err().unwrap().code, "paused_by_person", "{action}");
        }
        let opening_unity = projects.call("unity_add_orbiters", &json!({ "path": "C:/x", "open_unity": true }), &call(), busy.clone());
        assert_eq!(opening_unity.err().unwrap().code, "paused_by_person");
    }

    #[test]
    fn a_project_is_named_by_its_absolute_folder() {
        let projects = Projects::default();
        for (args, code) in [
            (json!({}), "bad_arguments"),
            (json!({ "path": "relative/project" }), "bad_arguments"),
            (json!({ "path": "Z:/no/such/project" }), "not_a_project"),
        ] {
            assert_eq!(projects.project(&args).err().unwrap().code, code, "{args}");
        }
    }

    /// One change at a time on a lane, as the tab: a second is refused with the first's job; another lane runs beside it.
    #[test]
    fn jobs_run_one_at_a_time_on_a_lane_and_are_followed() {
        let projects = Projects::default();
        let gate = Arc::new(AtomicBool::new(false));
        let g = gate.clone();
        let first = projects
            .spawn(Lane::Action, "first".into(), None, false, Progress::None, Arc::default(), None, &json!({ "wait_s": 0 }), &call(), move || {
                while !g.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(json!({ "ok": 1 }))
            })
            .unwrap();
        assert_eq!(first["state"], "running");
        let id = first["job"].as_u64().unwrap();
        let second = projects.spawn(Lane::Action, "second".into(), None, false, Progress::None, Arc::default(), None, &json!({ "wait_s": 0 }), &call(), || Ok(json!({})));
        let refused = second.err().unwrap();
        assert_eq!(refused.code, "busy");
        assert!(refused.next.contains(&id.to_string()));
        let beside = projects.spawn(Lane::Measure, "beside".into(), None, true, Progress::None, Arc::default(), None, &json!({ "wait_s": 5 }), &call(), || Ok(json!({ "size": 3 }))).unwrap();
        assert_eq!(beside["state"], "done");
        assert_eq!(beside["result"]["size"], 3);
        // Not cancellable: a package change is never stopped half way.
        assert_eq!(projects.cancel_job(&json!({ "job": id })).err().unwrap().code, "not_cancellable");
        gate.store(true, Ordering::Relaxed);
        let ended = projects.get_job(&json!({ "job": id, "wait_s": 5 }), &call()).unwrap();
        assert_eq!(ended["state"], "done");
        assert_eq!(ended["result"]["ok"], 1);
        assert_eq!(projects.get_job(&json!({}), &call()).unwrap()["jobs"].as_array().unwrap().len(), 2);
        assert_eq!(projects.get_job(&json!({ "job": 99 }), &call()).err().unwrap().code, "no_such_job");
    }

    /// Stop ends what can be stopped; a failure and a panic come back as failures, not as a stuck job.
    #[test]
    fn stop_cancels_copies_and_failures_are_reported() {
        let projects = Projects::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        let copy = projects
            .spawn(Lane::Action, "copy".into(), None, true, Progress::None, cancel, None, &json!({ "wait_s": 0 }), &call(), move || {
                while !c.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(Failure::new("cancelled", "Stopped.", "Nothing."))
            })
            .unwrap();
        projects.stop_all();
        let id = copy["job"].as_u64().unwrap();
        let ended = projects.get_job(&json!({ "job": id, "wait_s": 5 }), &call()).unwrap();
        assert_eq!(ended["state"], "failed");
        assert_eq!(ended["failure"]["code"], "cancelled");
        let crashed = projects.spawn(Lane::Scan, "crash".into(), None, true, Progress::None, Arc::default(), None, &json!({ "wait_s": 5 }), &call(), || panic!("a file nobody foresaw")).unwrap();
        assert_eq!(crashed["state"], "failed");
        assert_eq!(crashed["failure"]["code"], "crashed");
    }

    /// Removing from the list and killing Unity are never done on a guess: they want confirm, and the exact listed path.
    #[test]
    fn what_cannot_be_undone_wants_confirmation() {
        let dir = tempfile_dir();
        let project = dir.join("A project");
        std::fs::create_dir_all(project.join("ProjectSettings")).unwrap();
        std::fs::write(project.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
        let projects = Projects::default();
        // Not listed (in this computer's real list, a made-up folder never is): refused before anything is asked.
        let removed = projects.manage_project(&json!({ "path": project, "action": "remove_from_list", "confirm": true }));
        assert_eq!(removed.err().unwrap().code, "not_listed");
        let killed = projects.editor(&json!({ "path": project, "action": "kill" }));
        assert_eq!(killed.err().unwrap().code, "unity_not_open");
        assert!(project.join("ProjectSettings/ProjectVersion.txt").is_file(), "nothing deleted");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_conflict_comes_back_with_its_plan_and_missing_packages_are_remembered() {
        let missing = Mutex::new(HashMap::new());
        let plan = coherent::Plan { install: vec![("a".into(), "1.0.0".into())], steps: vec![updates::Step { name: "a".into(), ..Default::default() }], ..Default::default() };
        let conflict = Projects::package_outcome(Err(coherent::Conflicted { plan }.into()), Path::new("Z:/vcc"), Path::new("Z:/p"), "k", vpm::Change::Resolve, &missing).unwrap();
        assert_eq!(conflict["conflict"], true);
        assert_eq!(conflict["solved"], true);
        assert_eq!(conflict["plan"]["install"][0][0], "a");
        let error = anyhow::anyhow!("Following dependencies are not found: com.example.thing@1.2.3");
        let failure = Projects::package_outcome(Err(error), Path::new("Z:/vcc"), Path::new("Z:/p"), "k", vpm::Change::Resolve, &missing).err().unwrap();
        assert_eq!(failure.code, "packages_not_listed");
        assert_eq!(missing.lock().unwrap()["k"].1[0].name, "com.example.thing");
        let other = Projects::package_outcome(Err(anyhow::anyhow!("the disk is full")), Path::new("Z:/vcc"), Path::new("Z:/p"), "j", vpm::Change::Resolve, &missing).err().unwrap();
        assert_eq!(other.code, "change_failed");
        assert!(!missing.lock().unwrap().contains_key("j"));
    }

    #[test]
    fn versions_leave_pre_releases_out_unless_asked() {
        let p = vpm::Package { name: "a".into(), display_name: "A".into(), versions: vec!["2.0.0-beta.1".into(), "1.1.0".into(), "1.0.0".into()], ..Default::default() };
        assert_eq!(package_json(&p, false)["newest"], "1.1.0");
        assert_eq!(package_json(&p, true)["newest"], "2.0.0-beta.1");
        assert_eq!(package_json(&p, false)["versions"].as_array().unwrap().len(), 2);
    }

    /// Bridge verification 3, finding 4 (shared code, Windows' Unity workflows): a header map is checked value by value,
    /// not refused whole; a conflict's plan passed back as it came (install as [name, version] pairs) fits the schema and
    /// is read as the same packages, as are {name, version} objects; anything else is refused before anything is done.
    #[test]
    fn header_maps_and_a_solver_s_plan_round_trip_through_the_validator() {
        let schema = |name: &str| list().into_iter().find(|t| t["name"] == name).unwrap()["inputSchema"].clone();
        let refusal = |name: &str, args: Value| crate::bridge::tools::schema_refusal(name, &schema(name), &args);
        let repo = json!({ "action": "add", "url": "https://vpm.example/index.json", "headers": { "Authorization": "Bearer x", "X-Api": "y" } });
        assert!(refusal("unity_repositories", repo).is_none(), "a map of strings");
        let bad = json!({ "action": "add", "url": "https://vpm.example/index.json", "headers": { "X-Api": 3 } });
        assert!(refusal("unity_repositories", bad).is_some_and(|f| f.message.contains("headers.X-Api")), "each value checked");
        // what the solver returns, serialized as it goes to the client
        let plan = coherent::Plan { requested: vec![("a".into(), "1.0.0".into())], install: vec![("a".into(), "1.0.0".into()), ("b".into(), "2.1.0".into())], remove: vec!["c".into()], ..Default::default() };
        let given = serde_json::to_value(&plan).unwrap();
        let args = json!({ "path": "C:/p", "action": "apply_plan", "install": given["install"], "remove": given["remove"] });
        assert!(refusal("unity_change_packages", args.clone()).is_none(), "the plan as it came");
        assert_eq!(plan_pairs(&args["install"]).unwrap(), plan.install);
        let objects = json!({ "path": "C:/p", "action": "solve", "install": [{ "name": "a", "version": "1.0.0" }] });
        assert!(refusal("unity_change_packages", objects.clone()).is_none());
        assert_eq!(plan_pairs(&objects["install"]).unwrap(), [("a".to_string(), "1.0.0".to_string())]);
        for wrong in [json!([["a"]]), json!([["a", "1", "x"]]), json!([[1, 2]]), json!([{ "name": "a" }]), json!(["a"])] {
            let args = json!({ "path": "C:/p", "action": "apply_plan", "install": wrong });
            assert!(refusal("unity_change_packages", args.clone()).is_some(), "{args}");
            assert!(plan_pairs(&args["install"]).is_none(), "{args}");
        }
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-bridge-projects-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

#[cfg(test)]
mod unity6_tests {
    use super::*;

    fn call() -> Call {
        Call { client_gone: Arc::new(AtomicBool::new(false)), stop_at_start: 0 }
    }
    use unity6::trial::{Job, Phase, Place, Status, Target};

    fn project(at: &Path, editor: &str, sdk: &str, line: &str) {
        std::fs::create_dir_all(at.join("ProjectSettings")).unwrap();
        std::fs::create_dir_all(at.join("Packages/com.vrchat.base")).unwrap();
        std::fs::create_dir_all(at.join("Packages/com.vrchat.avatars")).unwrap();
        std::fs::write(at.join("ProjectSettings/ProjectVersion.txt"), format!("m_EditorVersion: {editor}\n")).unwrap();
        let lock = json!({ "dependencies": { "com.vrchat.avatars": { "version": sdk } }, "locked": { "com.vrchat.base": { "version": sdk }, "com.vrchat.avatars": { "version": sdk } } });
        std::fs::write(at.join("Packages/vpm-manifest.json"), lock.to_string()).unwrap();
        for name in ["com.vrchat.base", "com.vrchat.avatars"] {
            std::fs::write(at.join("Packages").join(name).join("package.json"), json!({ "name": name, "version": sdk, "unity": line }).to_string()).unwrap();
        }
    }

    /// M14: the bridge reads the same standing as the tab, and the trials' record; it changes nothing.
    #[test]
    fn a_projects_unity_line_comes_from_its_files_and_the_trials_record() {
        let temp = std::env::temp_dir().join(format!("orbiters-bridge-unity6-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let original = temp.join("Avatar");
        let trial = temp.join("Avatar (Unity 6)");
        project(&original, "2022.3.22f1", "3.10.6", "2022.3");
        project(&trial, "6000.0.67f1", "3.11.0-beta.1", "6000.0");
        let m = unity6::matrix::Matrix::built_in();
        let v = unity_line_json(&original, &m, &[]);
        assert_eq!((v["state"].as_str(), v["sdk_installed"].as_str(), v["beta"].as_bool(), v["content"].as_str()), (Some("on_2022"), Some("3.10.6"), Some(false), Some("avatars")));
        assert_eq!(v["history"], "not recorded by Orbiters");
        assert!(v["upload_warning"].is_null() && v["trial_of"].is_null());
        let place = |p: &Path, name: &str| Place { path: p.to_path_buf(), id: orbiters_projects::owned::id_of(p).ok(), name: name.into() };
        let job = Job {
            id: "1".repeat(32),
            created: 1_763_800_000,
            source: place(&original, "Avatar"),
            trial: place(&trial, "Avatar (Unity 6)"),
            phase: Phase::Ready,
            target: Target { sdk: "3.11.0-beta.1".into(), unity: "6000.0.67f1".into(), changeset: "78a1c2bbeb6a".into() },
            error: None,
            differences: vec![],
            attempts: vec![],
            migrated: None,
            beside: None,
            localized: vec![],
            linked: vec![],
            content: vec![],
            content_unsure: vec![],
            guid: String::new(),
            manifest: None,
            import_high: 0,
        };
        let trials = vec![(job.clone(), Status::OnUnity6)];
        let v = unity_line_json(&original, &m, &trials);
        assert_eq!(v["trials"].as_array().unwrap().len(), 1);
        assert_eq!(v["trials"][0]["status"], "on_unity6", "a stable name, not a debug string");
        let v = unity_line_json(&trial, &m, &trials);
        assert_eq!((v["state"].as_str(), v["beta"].as_bool()), (Some("on_unity6"), Some(true)));
        assert_eq!(v["trial_of"]["original"], json!(original));
        assert_eq!(v["trial_of"]["original_is_still_there"], "yes");
        assert!(v["upload_warning"].as_str().unwrap().contains("Unity 2022"));
        // Verification major 10: another folder put at the original's path is not its original any more.
        std::fs::rename(&original, temp.join("Avatar moved")).unwrap();
        project(&original, "2022.3.22f1", "3.10.6", "2022.3");
        assert!(unity_line_json(&original, &m, &trials)["trials"].as_array().unwrap().is_empty());
        assert_eq!(unity_line_json(&trial, &m, &trials)["trial_of"]["original_is_still_there"], "replaced");
        // Second verification major 10: the original, renamed, keeps its trials here as in the window.
        assert_eq!(unity_line_json(&temp.join("Avatar moved"), &m, &trials)["trials"].as_array().unwrap().len(), 1);
        // Verification minor 1: both tools say the target and its dated facts the same way.
        let projects = Projects::default();
        *projects.scan.lock().unwrap() = Some(Scan { projects: vec![Project { path: original.clone(), name: "Avatar".into(), registered: true, ..Default::default() }], ..Default::default() });
        let data = |a: Answer| match a {
            Answer::Data(v) => v,
            _ => panic!("data"),
        };
        let list = data(projects.call("unity_list_projects", &json!({}), &call(), None).unwrap());
        let get = data(projects.call("unity_get_project", &json!({ "path": original }), &call(), None).unwrap());
        assert_eq!(list["unity6_target"], get["unity6_target"]);
        assert_eq!((list["unity6_target"]["channel"].as_str(), list["unity6_target"]["unity2022_uploads_end"].is_null()), (Some("beta"), true));
        assert_eq!(list["projects"][0]["unity_line"]["state"], "on_2022");
        assert!(GUIDE.contains("unity_line") && GUIDE.contains("no tool migrates"));
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Verification blocker 3: the bridge's own commands against a trial that waits for Creator Companion: its packages
    /// are not changed and Unity is not opened, and what it answers says why.
    #[test]
    fn the_bridges_commands_refuse_a_waiting_trial() {
        let temp = std::env::temp_dir().join(format!("orbiters-bridge-trial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let original = temp.join("Avatar");
        project(&original, "2022.3.22f1", "3.10.6", "2022.3");
        std::fs::create_dir_all(original.join("Assets")).unwrap();
        std::fs::write(original.join("Packages/manifest.json"), r#"{"dependencies":{}}"#).unwrap();
        std::fs::write(original.join("ProjectSettings/ProjectSettings.asset"), "PlayerSettings:\n  productGUID: 0\n").unwrap();
        let target = Target { sdk: "3.11.0-beta.1".into(), unity: "6000.0.67f1".into(), changeset: "78a1c2bbeb6a".into() };
        let dir = orbiters_projects::data_dir();
        let job = unity6::trial::prepare(
            unity6::trial::Prepare { dir: &dir, source: &original, parent: &temp, name: "Avatar (Unity 6)", target, open: &|_| false, budget: &|_, _| Ok(0), recorded: &|_| {} },
            &AtomicBool::new(false),
            &Default::default(),
        )
        .unwrap();
        let projects = Projects::default();
        let at = job.trial.path.clone();
        *projects.scan.lock().unwrap() = Some(Scan { projects: vec![Project { path: at.clone(), name: "Avatar (Unity 6)".into(), unity: "2022.3.22f1".into(), registered: true, ..Default::default() }], ..Default::default() });
        let manifest = std::fs::read(at.join("Packages/vpm-manifest.json")).unwrap();
        let said = |r: Result<Answer, Failure>| match r {
            Err(f) => format!("{} {}", f.code, f.message),
            Ok(Answer::Data(v)) => v.to_string(),
            Ok(_) => panic!("data"),
        };
        let changed = said(projects.call("unity_change_packages", &json!({ "path": at, "action": "restore", "wait_s": 20 }), &call(), None));
        assert!(changed.contains("Creator Companion"), "{changed}");
        let opened = said(projects.call("unity_editor", &json!({ "path": at, "action": "open" }), &call(), None));
        assert!(opened.contains("open_failed") && opened.contains("Creator Companion"), "{opened}");
        assert_eq!(std::fs::read(at.join("Packages/vpm-manifest.json")).unwrap(), manifest, "nothing changed");
        // Toolkit, My Avatar, Mixer: refused before anything is done.
        for what in ["toolkit", "my_avatar", "mixer"] {
            let said = said(projects.call("unity_add_orbiters", &json!({ "path": at, "what": what, "wait_s": 20 }), &call(), None));
            assert!(said.contains("refused") && said.contains("Creator Companion"), "{what}: {said}");
        }
        assert_eq!(std::fs::read(at.join("Packages/vpm-manifest.json")).unwrap(), manifest, "nothing changed");
        unity6::trial::abandon(&dir, &job.id).unwrap();
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// Second verification N4: no tool opens a project on Unity 6, trial or not, and the optional open of another tool is
    /// refused before that tool does anything. (No such editor exists: nothing can start.)
    #[test]
    fn no_tool_opens_a_unity6_project() {
        let temp = std::env::temp_dir().join(format!("orbiters-bridge-u6-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        let at = temp.join("Six");
        project(&at, "6000.0.99f9", "3.11.0-beta.1", "6000.0");
        std::fs::create_dir_all(at.join("Assets")).unwrap();
        let projects = Projects::default();
        *projects.scan.lock().unwrap() = Some(Scan { projects: vec![Project { path: at.clone(), name: "Six".into(), unity: "6000.0.99f9".into(), registered: true, ..Default::default() }], ..Default::default() });
        let manifest = std::fs::read(at.join("Packages/vpm-manifest.json")).unwrap();
        let said = |r: Result<Answer, Failure>| match r {
            Err(f) => format!("{} {}", f.code, f.message),
            Ok(Answer::Data(v)) => v.to_string(),
            Ok(_) => panic!("data"),
        };
        let opened = said(projects.call("unity_editor", &json!({ "path": at, "action": "open" }), &call(), None));
        assert!(opened.contains("open_failed") && opened.contains("Unity 6") && !opened.contains("not installed"), "{opened}");
        for what in ["mixer", "toolkit", "my_avatar"] {
            let added = said(projects.call("unity_add_orbiters", &json!({ "path": at, "what": what, "open_unity": true, "wait_s": 20 }), &call(), None));
            assert!(added.contains("refused") && added.contains("Unity 6"), "{what}: {added}");
        }
        assert_eq!(std::fs::read(at.join("Packages/vpm-manifest.json")).unwrap(), manifest, "nothing changed");
        let _ = std::fs::remove_dir_all(&temp);
    }
}
