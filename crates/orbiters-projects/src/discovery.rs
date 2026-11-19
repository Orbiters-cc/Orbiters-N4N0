use orbiters_i18n::tr;
use crate::avatars::{self, Avatar, SceneCache};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Kind {
    Avatars,
    Worlds,
    Vrchat,
    #[default]
    Unity,
}
impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Avatars => tr("Avatars"),
            Self::Worlds => tr("Worlds"),
            Self::Vrchat => "VRChat",
            Self::Unity => "Unity",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Project {
    pub path: PathBuf,
    pub name: String,
    pub unity: String,
    pub kind: Kind,
    pub modified: u64,
    pub registered: bool,
    pub missing: bool,
    pub avatars: Vec<Avatar>,
    /// The worlds its scenes were uploaded as (`wrld_…` blueprint IDs), with My World's thumbnail when it keeps one.
    #[serde(default)]
    pub worlds: Vec<Avatar>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Scan {
    pub projects: Vec<Project>,
    pub last_full: u64,
    pub full_scan_ms: u64,
    pub directories: u64,
    pub warnings: Vec<String>,
    #[serde(skip)]
    pub status: String,
    #[serde(default)]
    pub scenes: SceneCache,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}
pub fn stamp(path: &Path) -> u64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}
pub fn fingerprint(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos().min(u64::MAX as u128) as u64)
}
pub fn key(path: &Path) -> String {
    path.to_string_lossy().trim_start_matches(r"\\?\").replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

pub fn parse_version(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.trim().strip_prefix("m_EditorVersion:").map(str::trim).filter(|s| !s.is_empty()).map(ToOwned::to_owned))
}
pub fn manifest_kind(text: &str) -> Kind {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return Kind::Unity };
    let has = |name: &str| ["locked", "dependencies"].iter().any(|section| v[section].as_object().is_some_and(|m| m.contains_key(name)));
    if has("com.vrchat.avatars") {
        Kind::Avatars
    } else if has("com.vrchat.worlds") {
        Kind::Worlds
    } else if ["locked", "dependencies"].iter().any(|s| v[s].as_object().is_some_and(|m| m.keys().any(|k| k.starts_with("com.vrchat.")))) {
        Kind::Vrchat
    } else {
        Kind::Unity
    }
}
pub fn decode_registry_path(bytes: &[u8]) -> Option<PathBuf> {
    let bytes = &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())];
    let s = std::str::from_utf8(bytes).ok()?.trim();
    (!s.is_empty()).then(|| PathBuf::from(s))
}
pub fn skip_dir(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "windows" | "programdata" | "$recycle.bin" | "system volume information" | "appdata" | "library" | "temp" | "logs" | "obj" | "node_modules" | ".git" // i18n: not shown (folder names and a macOS query the scan skips or asks for)
    ) || n.starts_with("program files")
        // Dot-folders, on every system (PROJECTS-9, Enzo 2026-11-19: "Yes"): caches, tools' state, test stand-ins.
        || n.starts_with('.')
        || mac_only_skip(&n)
}
/// Hidden folders (the Trash, Spotlight's and Time Machine's, a shell's caches) and what the Finder shows as one file: an
/// application or a photo library, never a place for a Unity project. A Unity project's own `Library` is never reached,
/// since a walk stops at a project's folder.
fn mac_only_skip(name: &str) -> bool {
    cfg!(target_os = "macos") && (name.starts_with('.') || [".app", ".framework", ".bundle", ".photoslibrary", ".musiclibrary", ".tvlibrary"].iter().any(|x| name.ends_with(x)))
}
/// Whether reading this folder makes macOS ask the person for permission in the app's name: the folders of the home
/// folder it guards (its Library is left out anyway, and Spotlight answers for all of them without asking anybody).
fn asks_permission(path: &Path) -> bool {
    cfg!(target_os = "macos")
        && path.parent().is_some_and(|parent| dirs::home_dir().as_deref() == Some(parent))
        && path.file_name().is_some_and(|n| ["Desktop", "Documents", "Downloads", "Movies", "Music", "Pictures"].iter().any(|g| n == *g)) // i18n: not shown (folder names and a macOS query the scan skips or asks for)
}
pub fn is_link(m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        m.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        m.file_type().is_symlink()
    }
}
pub fn is_project(path: &Path) -> bool {
    path.join("ProjectSettings/ProjectVersion.txt").is_file() && !unfinished_copy(path)
}
/// Whether `path` is a copy still being made (or cut short): its marker names this very folder. An older version's
/// empty marker counts too; one naming another folder (copied, restored) does not.
fn unfinished_copy(path: &Path) -> bool {
    let marker = path.join(crate::templates::INCOMPLETE);
    match std::fs::read(&marker) {
        Ok(bytes) if bytes.is_empty() => true,
        // The very folder the copy made, by identity when the marker has it (a project restored at that place with the
        // marker, or a copy of the folder, is another folder).
        Ok(bytes) => serde_json::from_slice::<crate::templates::Marker>(&bytes)
            .is_ok_and(|m| key(&m.destination) == key(path) && m.folder.is_none_or(|id| crate::owned::is_same(path, id))),
        Err(_) => false,
    }
}
pub fn describe(path: PathBuf, registered: bool, _scenes: &mut SceneCache) -> Project {
    let unity = std::fs::read_to_string(path.join("ProjectSettings/ProjectVersion.txt")).ok().and_then(|s| parse_version(&s)).unwrap_or_default();
    let kind = std::fs::read_to_string(path.join("Packages/vpm-manifest.json")).map(|s| manifest_kind(&s)).unwrap_or_default();
    let modified = [path.clone(), path.join("Assets"), path.join("Packages/vpm-manifest.json"), path.join("ProjectSettings/ProjectVersion.txt")]
        .iter()
        .map(|p| stamp(p))
        .max()
        .unwrap_or(0);
    let avatars = vec![];
    Project {
        name: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        missing: !is_project(&path),
        path,
        unity,
        kind,
        modified,
        registered,
        avatars,
        worlds: vec![],
    }
}
pub fn cached() -> Scan {
    std::fs::read(crate::data_dir().join("unity-projects.json")).ok().and_then(|s| serde_json::from_slice(&s).ok()).unwrap_or_default()
}

pub fn seeds(registered: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = registered.to_vec();
    if let Some(dir) = crate::hub_data_dir() {
        if let Ok(text) = std::fs::read_to_string(dir.join("projects-v1.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                collect_paths(&v, &mut paths);
            }
        }
        // The folder Unity Hub makes new projects in: disks are not walked as widely here as on Windows.
        #[cfg(target_os = "macos")]
        if let Ok(text) = std::fs::read_to_string(dir.join("projectDir.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                collect_paths(&v, &mut paths);
            }
        }
    }
    // A harness's private Hub/VCC lists are its complete environment; never augment them from the real editor.
    if orbiters_core::paths::roots().is_some() {
        return paths;
    }
    #[cfg(target_os = "macos")]
    if let Some(prefs) = dirs::home_dir().map(|h| h.join("Library/Preferences/com.unity3d.UnityEditor5.x.plist")) {
        paths.extend(recent_projects(&prefs));
    }
    #[cfg(windows)]
    {
        if let Ok(k) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(r"Software\Unity Technologies\Unity Editor 5.x") {
            for (n, v) in k.enum_values().flatten() {
                if n.starts_with("RecentlyUsedProjectPaths-") && v.vtype == winreg::enums::REG_BINARY {
                    if let Some(p) = decode_registry_path(&v.bytes) {
                        paths.push(p);
                    }
                }
            }
        }
    }
    paths
}
/// What Unity's editor keeps of its recent projects, on the Mac in its preferences (`RecentlyUsedProjectPaths-<n>`).
#[cfg(target_os = "macos")]
fn recent_projects(prefs: &Path) -> Vec<PathBuf> {
    let Ok(all) = plist::Value::from_file(prefs) else { return vec![] };
    let Some(all) = all.into_dictionary() else { return vec![] };
    all.iter().filter(|(k, _)| k.starts_with("RecentlyUsedProjectPaths-")).filter_map(|(_, v)| v.as_string().map(PathBuf::from)).collect()
}
fn collect_paths(v: &serde_json::Value, paths: &mut Vec<PathBuf>) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, v) in m {
                let p = PathBuf::from(k);
                if p.is_absolute() {
                    paths.push(p);
                }
                collect_paths(v, paths);
            }
        }
        serde_json::Value::Array(a) => {
            for v in a {
                collect_paths(v, paths);
            }
        }
        serde_json::Value::String(s) => {
            let p = PathBuf::from(s);
            if p.is_absolute() {
                paths.push(p);
            }
        }
        _ => {}
    }
}
pub fn fixed_disks() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
        (0..26)
            .filter_map(|i| {
                if unsafe { GetLogicalDrives() } & (1 << i) == 0 {
                    return None;
                }
                let s = format!("{}:\\", (b'A' + i) as char);
                let w: Vec<u16> = s.encode_utf16().chain(Some(0)).collect();
                (unsafe { GetDriveTypeW(windows::core::PCWSTR(w.as_ptr())) } == 3).then(|| PathBuf::from(s))
            })
            .collect()
    }
    #[cfg(target_os = "macos")]
    {
        crate::mac::fixed_disks()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        vec![]
    }
}
/// Where a walk of the disks starts. On the Mac the startup disk holds nothing of the person's outside `/Users`, and
/// walking another volume makes the system ask for permission (it guards external disks): that waits for a press on
/// Refresh, Spotlight finding what it has indexed there meanwhile.
fn scan_roots(_force: bool) -> Vec<PathBuf> {
    if let Some(roots) = orbiters_core::paths::roots() {
        return vec![roots.documents.clone()];
    }
    #[cfg(target_os = "macos")]
    return std::iter::once(PathBuf::from("/Users")).chain(if _force { fixed_disks() } else { vec![] }.into_iter().filter(|d| d != Path::new("/"))).collect();
    #[cfg(not(target_os = "macos"))]
    fixed_disks()
}
/// The projects Spotlight knows of, wherever they are: what a walk would find, instantly and without a permission
/// question for the folders macOS guards. Reading a project found in one is what makes it ask, and only then.
#[cfg(target_os = "macos")]
fn spotlight_projects() -> Vec<PathBuf> {
    projects_of_version_files(crate::mac::spotlight("kMDItemFSName == 'ProjectVersion.txt'")) // i18n: not shown (folder names and a macOS query the scan skips or asks for)
}
/// The projects `<project>/ProjectSettings/ProjectVersion.txt` files name, without those a walk would not go to.
#[cfg(target_os = "macos")]
fn projects_of_version_files(files: Vec<PathBuf>) -> Vec<PathBuf> {
    files
        .into_iter()
        .filter(|f| f.ends_with("ProjectSettings/ProjectVersion.txt"))
        .filter_map(|f| f.parent()?.parent().map(Path::to_path_buf))
        .filter(|p| !p.components().any(|c| matches!(c, std::path::Component::Normal(n) if skip_dir(&n.to_string_lossy()))))
        .collect()
}
pub fn low_priority() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::*;
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
    // The class macOS gives work that is not waited for, as the Finder's copies. No thread is made slower than that: the
    // class below it can take minutes to read a folder while the computer is busy.
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
        }
        const QOS_CLASS_UTILITY: u32 = 0x11;
        // SAFETY: only changes the calling thread's class.
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
        }
    }
}

/// Streams quick results before walking disks. No project/store mutation occurs here.
pub fn discover(mut scan: Scan, force: bool, cancel: &AtomicBool, publish: impl FnMut(&Scan)) -> anyhow::Result<Scan> {
    low_priority();
    let mut notify = publish;
    let mut checkpoint = Checkpoint::new(CHECKPOINT_EVERY);
    let mut publish = |scan: &Scan| {
        // Keep useful results across a window close while a large disk is still being enumerated: written now and then
        // (PROJECTS-7: it was the whole file, flushed to disk, at every step: ~740 times a scan with 400 projects), and
        // in full at the end.
        if !cancel.load(Ordering::Relaxed) && checkpoint.due(Instant::now()) {
            let _ = crate::write_json(&crate::data_dir().join("unity-projects.json"), scan);
        }
        notify(scan);
    };
    let shared = crate::runtime()?.block_on(crate::vpm::registered(&crate::vcc_dir()));
    let registered = match shared {
        Ok(p) => p,
        Err(e) => {
            scan.warnings = vec![e.to_string()];
            scan.projects.iter().filter(|p| p.registered).map(|p| p.path.clone()).collect()
        }
    };
    let known: HashSet<_> = registered.iter().map(|p| key(p)).collect();
    let mut found: HashMap<String, PathBuf> = scan.projects.iter().map(|p| (key(&p.path), p.path.clone())).collect();
    for p in &registered {
        found.insert(key(p), p.clone());
    }
    rebuild(&mut scan, &found, &known);
    scan.status = tr("Finding nearby projects").into();
    publish(&scan);
    let seeds = seeds(&registered);
    let mut roots = HashSet::new();
    for p in &seeds {
        if is_project(p) {
            found.insert(key(p), p.clone());
        }
        if let Some(parent) = p.parent() {
            roots.insert(parent.to_path_buf());
        }
    }
    for r in roots {
        walk(&r, Some(2), !force, cancel, &mut found, &AtomicU64::new(0), None);
    }
    rebuild(&mut scan, &found, &known);
    publish(&scan);
    let mut enriched = HashSet::new();
    enrich(&mut scan, cancel, &mut enriched, &mut publish);
    if force || now().saturating_sub(scan.last_full) >= 3600 {
        scan.status = tr("Scanning fixed local disks").into();
        publish(&scan);
        let start = Instant::now();
        let mut count = 0;
        #[cfg(target_os = "macos")]
        {
            for p in if orbiters_core::paths::roots().is_none() { spotlight_projects() } else { Vec::new() } {
                found.insert(key(&p), p);
            }
            rebuild(&mut scan, &found, &known);
            publish(&scan);
        }
        for disk in scan_roots(force) {
            scan.status = tr!("Scanning {disk}", disk = disk.display());
            publish(&scan);
            count += scan_disk(&disk, !force, cancel, &mut found, &mut scan.warnings);
            if cancel.load(Ordering::Relaxed) {
                break;
            }
        }
        scan.full_scan_ms = start.elapsed().as_millis() as u64;
        scan.directories = count;
        if !cancel.load(Ordering::Relaxed) {
            scan.last_full = now();
        }
        rebuild(&mut scan, &found, &known);
        publish(&scan);
        enrich(&mut scan, cancel, &mut enriched, &mut publish);
    }
    scan.status.clear();
    if !cancel.load(Ordering::Relaxed) {
        crate::write_json(&crate::data_dir().join("unity-projects.json"), &scan)?;
    }
    Ok(scan)
}
/// How often a scan writes what it found so far.
const CHECKPOINT_EVERY: Duration = Duration::from_secs(5);

/// Whether a scan's results are due to be written: the first time, then once per period.
struct Checkpoint {
    every: Duration,
    last: Option<Instant>,
}
impl Checkpoint {
    fn new(every: Duration) -> Self {
        Self { every, last: None }
    }
    fn due(&mut self, now: Instant) -> bool {
        let due = self.last.is_none_or(|last| now.saturating_duration_since(last) >= self.every);
        if due {
            self.last = Some(now);
        }
        due
    }
}

/// Whether a folder of `path` (it included) is a dot-folder, which the walk skips (PROJECTS-9).
fn in_dot_folder(path: &Path) -> bool {
    path.components().any(|c| matches!(c, std::path::Component::Normal(n) if n.to_string_lossy().starts_with('.')))
}

fn rebuild(scan: &mut Scan, found: &HashMap<String, PathBuf>, registered: &HashSet<String>) {
    let old: HashMap<_, _> = scan.projects.iter().map(|p| (key(&p.path), (p.avatars.clone(), p.worlds.clone()))).collect();
    scan.projects = found
        .iter()
        // A project found earlier inside a dot-folder (a cache from before PROJECTS-9, a seed) is not proposed again;
        // one the person registered stays wherever it is.
        .filter(|(k, p)| registered.contains(*k) || (is_project(p) && !in_dot_folder(p)))
        .map(|(k, p)| {
            let mut project = describe(p.clone(), registered.contains(k), &mut scan.scenes);
            (project.avatars, project.worlds) = old.get(k).cloned().unwrap_or_default();
            project
        })
        .collect();
    scan.projects.sort_by_key(|p| (std::cmp::Reverse(p.registered), std::cmp::Reverse(p.modified)));
}
fn enrich(scan: &mut Scan, cancel: &AtomicBool, enriched: &mut HashSet<String>, publish: &mut impl FnMut(&Scan)) {
    for i in 0..scan.projects.len() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let p = &scan.projects[i];
        if !matches!(p.kind, Kind::Avatars | Kind::Worlds) || !enriched.insert(key(&p.path)) {
            continue;
        }
        let path = p.path.clone();
        scan.status = if p.kind == Kind::Worlds { tr!("Reading worlds in {project}", project = p.name) } else { tr!("Reading avatars in {project}", project = p.name) };
        publish(scan);
        let (items, complete) = avatars::in_project_checked(&path, &mut scan.scenes);
        // A readable Assets root does not guarantee its subfolders, scenes or prefabs were readable.
        if !complete {
            continue;
        }
        let (worlds, avatars): (Vec<Avatar>, Vec<Avatar>) = items.into_iter().partition(Avatar::is_world);
        scan.projects[i].avatars = avatars;
        scan.projects[i].worlds = worlds;
        publish(scan);
    }
}
enum DiskEvent {
    Found(PathBuf),
    Done,
}
/// A damaged/offline volume must not hold every other disk hostage. Cancel an enumeration after 60 s without a
/// single directory completed, retain discoveries already received, and report that volume in the scan warnings.
fn scan_disk(root: &Path, quiet: bool, cancel: &AtomicBool, found: &mut HashMap<String, PathBuf>, warnings: &mut Vec<String>) -> u64 {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(AtomicU64::new(0));
    let worker = {
        let root = root.to_path_buf();
        let stop = stop.clone();
        let progress = progress.clone();
        std::thread::spawn(move || {
            low_priority();
            walk(&root, None, quiet, &stop, &mut HashMap::new(), &progress, Some(&tx));
            let _ = tx.send(DiskEvent::Done);
        })
    };
    let mut last = 0;
    let mut advanced = Instant::now();
    let mut finished = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(DiskEvent::Found(p)) => {
                found.insert(key(&p), p);
            }
            Ok(DiskEvent::Done) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                finished = true;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let n = progress.load(Ordering::Relaxed);
        if n != last {
            last = n;
            advanced = Instant::now();
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if advanced.elapsed() > Duration::from_secs(60) {
            warnings.push(tr!("{folder} stopped responding for 60 seconds. Its scan was interrupted; Refresh retries it.", folder = root.display()));
            break;
        }
    }
    stop.store(true, Ordering::Relaxed);
    if finished {
        let _ = worker.join();
    } else {
        #[cfg(windows)]
        unsafe {
            use std::os::windows::io::AsRawHandle;
            let _ = windows::Win32::System::IO::CancelSynchronousIo(windows::Win32::Foundation::HANDLE(worker.as_raw_handle()));
        }
    }
    progress.load(Ordering::Relaxed)
}
/// `quiet`: leaving out the folders reading which makes the system ask the person something ([`asks_permission`]).
fn walk(
    root: &Path,
    depth: Option<usize>,
    quiet: bool,
    cancel: &AtomicBool,
    found: &mut HashMap<String, PathBuf>,
    count: &AtomicU64,
    events: Option<&mpsc::Sender<DiskEvent>>,
) {
    let mut stack = vec![(root.to_path_buf(), 0)];
    while let Some((p, d)) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if is_link(&m) || !m.is_dir() {
            continue;
        }
        count.fetch_add(1, Ordering::Relaxed);
        if is_project(&p) {
            if let Some(tx) = events {
                let _ = tx.send(DiskEvent::Found(p.clone()));
            }
            found.insert(key(&p), p);
            continue;
        }
        if depth.is_some_and(|max| d >= max) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&p) else { continue };
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) && !skip_dir(&e.file_name().to_string_lossy()) && !(quiet && asks_permission(&e.path())) {
                stack.push((e.path(), d + 1));
            }
        }
    }
}

pub fn find_editor(version: &str) -> Option<PathBuf> {
    find_editor_in(version, crate::hub_data_dir().as_deref(), &default_editor_folders(), &crate::vcc_dir())
}

/// The folders under which Unity Hub puts editors by itself.
fn default_editor_folders() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        let roots = [Some(PathBuf::from("/Applications")), dirs::home_dir().map(|h| h.join("Applications"))];
        roots.into_iter().flatten().map(|r| r.join("Unity/Hub/Editor")).collect()
    } else {
        vec![PathBuf::from(r"C:\Program Files\Unity\Hub\Editor")]
    }
}

/// The program of the editor installed in `folder`, a folder Unity Hub made for it (`.../Hub/Editor/<version>`).
fn editor_in_folder(folder: &Path) -> PathBuf {
    folder.join(if cfg!(target_os = "macos") { "Unity.app/Contents/MacOS/Unity" } else { "Editor/Unity.exe" })
}

/// The program of an editor named as Unity Hub's files, VCC or ALCOM do. On the Mac they name the program, the `Unity.app`
/// bundle or the folder that holds it; elsewhere a path with an extension is the program and one without its folder.
fn editor_program(path: PathBuf) -> PathBuf {
    if cfg!(target_os = "macos") {
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("app")) {
            path.join("Contents/MacOS/Unity")
        } else if path.join("Unity.app").is_dir() {
            editor_in_folder(&path)
        } else {
            path
        }
    } else if path.extension().is_some() {
        path
    } else {
        editor_in_folder(&path)
    }
}

/// [`find_editor`] with its places given.
fn find_editor_in(version: &str, hub: Option<&Path>, folders: &[PathBuf], vcc: &Path) -> Option<PathBuf> {
    // Search only the requested version: opening another editor could upgrade a project.
    let mut candidates = vec![];
    // Unity Hub names the folder after the version alone, with the architecture too when two are installed side by side
    // (`-arm64`, `-x86_64`). Unity for Apple silicon first: the other one runs under Rosetta, if at all.
    let suffixes: &[&str] = if cfg!(target_os = "macos") { &["-arm64", "", "-x86_64"] } else { &[""] };
    for folder in folders {
        candidates.extend(suffixes.iter().map(|s| editor_in_folder(&folder.join(format!("{version}{s}")))));
    }
    if let Some(hub) = hub {
        if let Ok(s) = std::fs::read_to_string(hub.join("editors-v2.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                editor_entries(&v, version, &mut candidates);
            }
        }
        if let Ok(s) = std::fs::read_to_string(hub.join("secondaryInstallPath.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                for value in [v.as_str(), v["secondaryInstallPath"].as_str()] {
                    if let Some(p) = value {
                        for suffix in suffixes {
                            candidates.push(editor_in_folder(&PathBuf::from(p).join(format!("{version}{suffix}"))));
                        }
                    }
                }
            }
        }
        // The install folder given to every user of the computer, in Hub's settings or its machine-wide file.
        #[cfg(target_os = "macos")]
        for file in [hub.join("settings.json"), PathBuf::from("/Library/Application Support/Unity/config/services-config.json")] {
            let settings: serde_json::Value = std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            if let Some(p) = settings["machineWideSecondaryInstallLocation"].as_str().filter(|p| !p.is_empty()) {
                candidates.extend(suffixes.iter().map(|s| editor_in_folder(&PathBuf::from(p).join(format!("{version}{s}")))));
            }
        }
    }
    if let Ok(s) = std::fs::read_to_string(vcc.join("settings.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(p) = v["preferredUnityEditors"][version].as_str() {
                candidates.push(if cfg!(target_os = "macos") { editor_program(PathBuf::from(p)) } else { PathBuf::from(p) });
            }
        }
    }
    if let Some(p) = candidates.iter().find(|p| p.is_file()) {
        return Some(p.clone());
    }
    // Editors VCC or ALCOM found or were given (outside Unity Hub's lists).
    // VCC's database can be held by VCC itself: a wait there must not hold up Open in Unity, whose answer is then "not
    // installed" as for any version not in the lists above.
    let version = version.to_string();
    let vcc = vcc.to_path_buf();
    crate::within(std::time::Duration::from_secs(5), move || {
        crate::runtime().ok()?.block_on(async {
            let io = vrc_get_vpm::io::DefaultEnvironmentIo::new(vcc.into());
            let db = vrc_get_vpm::environment::VccDatabaseConnection::connect(&io).await.ok()?;
            let wanted = vrc_get_vpm::version::UnityVersion::parse(&version)?;
            db.get_unity_installations()
                .iter()
                .filter(|u| u.version() == Some(wanted))
                .filter_map(|u| u.path().map(PathBuf::from))
                .map(|p| if cfg!(target_os = "macos") { editor_program(p) } else { p })
                .find(|p| p.is_file())
        })
    })
    .flatten()
}
fn editor_entries(v: &serde_json::Value, version: &str, out: &mut Vec<PathBuf>) {
    match v {
        serde_json::Value::Array(a) => {
            for v in a {
                editor_entries(v, version, out);
            }
        }
        serde_json::Value::Object(m) => {
            if v["version"].as_str() == Some(version) {
                for k in ["location", "path"] {
                    if let Some(p) = v[k].as_str() {
                        out.push(editor_program(PathBuf::from(p)));
                    }
                    // Unity Hub on the Mac gives a list.
                    #[cfg(target_os = "macos")]
                    if let Some(list) = v[k].as_array() {
                        out.extend(list.iter().filter_map(|p| p.as_str()).map(|p| editor_program(PathBuf::from(p))));
                    }
                }
            }
            for (k, v) in m {
                if k == version {
                    if let Some(p) = v.as_str() {
                        out.push(PathBuf::from(p));
                    }
                }
                editor_entries(v, version, out);
            }
        }
        _ => {}
    }
}

#[cfg(all(test, target_os = "macos"))]
mod mac_tests {
    use super::*;

    /// An application bundle holding a stand-in for the program.
    fn fake_editor(folder: &Path) -> PathBuf {
        let program = folder.join("Unity.app/Contents/MacOS/Unity");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        program
    }

    #[test]
    fn an_editor_is_found_in_every_place_unity_hub_keeps_one() {
        let temp = tempfile::tempdir().unwrap();
        let vcc = temp.path().join("vcc");
        let hub = temp.path().join("hub");
        std::fs::create_dir_all(&hub).unwrap();
        let usual = temp.path().join("Applications/Unity/Hub/Editor");
        let find = |version: &str| find_editor_in(version, Some(&hub), std::slice::from_ref(&usual), &vcc);
        assert_eq!(find("2022.3.22f1"), None, "nothing installed");

        // Where Unity Hub installs them, an editor for Apple silicon before one for Intel.
        let intel = fake_editor(&usual.join("2022.3.22f1-x86_64"));
        assert_eq!(find("2022.3.22f1").as_ref(), Some(&intel));
        let plain = fake_editor(&usual.join("2022.3.22f1"));
        assert_eq!(find("2022.3.22f1").as_ref(), Some(&plain));
        let apple = fake_editor(&usual.join("2022.3.22f1-arm64"));
        assert_eq!(find("2022.3.22f1").as_ref(), Some(&apple));
        assert_eq!(find("2022.3.21f1"), None, "only the version asked for: another could upgrade the project");

        // Located by hand: Unity Hub's list, naming the bundle, its folder or the program, as a string or a list.
        let located = fake_editor(&temp.path().join("Elsewhere/2021.3.8f1"));
        let bundle = located.parent().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
        for location in [
            serde_json::json!(bundle),
            serde_json::json!([bundle]),
            serde_json::json!(bundle.parent().unwrap()),
            serde_json::json!(located),
        ] {
            let editors = serde_json::json!({"schema_version": "v2", "data": [{"version": "2021.3.8f1", "location": location, "architecture": "arm64"}]});
            std::fs::write(hub.join("editors-v2.json"), editors.to_string()).unwrap();
            assert_eq!(find("2021.3.8f1").as_ref(), Some(&located), "{location}");
        }

        // The install folder chosen in Unity Hub's preferences, a JSON string.
        let secondary = temp.path().join("External/Editors");
        let there = fake_editor(&secondary.join("2020.3.1f1"));
        std::fs::write(hub.join("secondaryInstallPath.json"), serde_json::json!(secondary).to_string()).unwrap();
        assert_eq!(find("2020.3.1f1").as_ref(), Some(&there));

        // The one given to every user of the computer.
        let wide = temp.path().join("Shared/Editors");
        let shared = fake_editor(&wide.join("2019.4.31f1"));
        std::fs::write(hub.join("settings.json"), serde_json::json!({"machineWideSecondaryInstallLocation": wide}).to_string()).unwrap();
        assert_eq!(find("2019.4.31f1").as_ref(), Some(&shared));

        // What VCC's settings prefer, by its bundle.
        let preferred = fake_editor(&temp.path().join("Preferred"));
        std::fs::create_dir_all(&vcc).unwrap();
        let settings = serde_json::json!({"preferredUnityEditors": {"2018.4.20f1": preferred.parent().unwrap().parent().unwrap().parent().unwrap()}});
        std::fs::write(vcc.join("settings.json"), settings.to_string()).unwrap();
        assert_eq!(find("2018.4.20f1").as_ref(), Some(&preferred));
    }

    #[test]
    fn the_walk_keeps_out_of_what_is_not_the_persons_and_finds_projects_in_folders_named_like_it() {
        for skipped in [".Trash", ".Spotlight-V100", ".fseventsd", ".cache", "Blender.app", "Photos Library.photoslibrary", "Library"] {
            assert!(skip_dir(skipped), "{skipped}");
        }
        let temp = tempfile::tempdir().unwrap();
        let project = |at: &str| {
            let p = temp.path().join(at);
            std::fs::create_dir_all(p.join("ProjectSettings")).unwrap();
            std::fs::write(p.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
            // A project's own Library is not what the skip is about: the walk stops at the project's folder.
            std::fs::create_dir_all(p.join("Library/PackageCache")).unwrap();
            key(&p)
        };
        let wanted = [project("Unity Projects/Avatar"), project("Deep/er/still/World")];
        project(".Trash/Thrown away");
        project("Library/Application Support/Orbiters/templates/base");
        project("Applications/Some.app/Contents/Project");
        project("Unity Projects/Avatar/Assets/Nested");
        let mut found = HashMap::new();
        walk(temp.path(), None, true, &AtomicBool::new(false), &mut found, &AtomicU64::new(0), None);
        let mut keys: Vec<&String> = found.keys().collect();
        keys.sort();
        let mut expected: Vec<&String> = wanted.iter().collect();
        expected.sort();
        assert_eq!(keys, expected);
    }

    /// PROJECTS-9: a project inside a dot-folder is not found by the walk; one beside it is.
    #[test]
    fn the_walk_skips_dot_folders() {
        let temp = tempfile::tempdir().unwrap();
        let project = |at: &str| {
            let p = temp.path().join(at);
            std::fs::create_dir_all(p.join("ProjectSettings")).unwrap();
            std::fs::write(p.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
            key(&p)
        };
        let wanted = project("Projects/Mine");
        project(".orbiters-build/runs/Stand-in");
        project("Projects/.cache/Other");
        let mut found = HashMap::new();
        walk(temp.path(), None, false, &AtomicBool::new(false), &mut found, &AtomicU64::new(0), None);
        assert_eq!(found.keys().collect::<Vec<_>>(), [&wanted]);
    }

    #[test]
    fn folders_the_system_guards_are_not_walked_unless_refresh_was_pressed() {
        let home = dirs::home_dir().unwrap();
        assert!(asks_permission(&home.join("Documents")));
        assert!(asks_permission(&home.join("Desktop")));
        assert!(asks_permission(&home.join("Downloads")));
        assert!(!asks_permission(&home.join("Documents/Unity")), "inside it, a project already named there got us here");
        assert!(!asks_permission(&home.join("Unity Projects")));
        assert!(!asks_permission(Path::new("/Volumes/SSD/Documents")));
        // /Users always; another disk only for Refresh.
        assert_eq!(scan_roots(false), [PathBuf::from("/Users")]);
        assert_eq!(scan_roots(true)[0], PathBuf::from("/Users"));
        assert!(!scan_roots(true).contains(&PathBuf::from("/")));
    }

    /// What a walk of this computer's `/Users` finds and how long it takes, printed (reads only, never a folder that makes
    /// the system ask): `cargo test -p orbiters-projects real_walk -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_walk() {
        let start = Instant::now();
        let (mut found, count) = (HashMap::new(), AtomicU64::new(0));
        walk(Path::new("/Users"), None, true, &AtomicBool::new(false), &mut found, &count, None);
        println!("{} folders in {:?}: {} projects", count.load(Ordering::Relaxed), start.elapsed(), found.len());
        let start = Instant::now();
        let spotlight = spotlight_projects();
        println!("Spotlight in {:?}: {} projects", start.elapsed(), spotlight.len());
        for p in found.values().chain(&spotlight) {
            println!("  {}", p.display());
        }
    }

    #[test]
    fn spotlights_version_files_name_their_projects() {
        let files = [
            "/Users/enzo/Documents/Avatar/ProjectSettings/ProjectVersion.txt",
            "/Volumes/SSD/World/ProjectSettings/ProjectVersion.txt",
            "/Users/enzo/.Trash/Old/ProjectSettings/ProjectVersion.txt",
            "/Users/enzo/Library/Application Support/Orbiters/templates/base/ProjectSettings/ProjectVersion.txt",
            "/Users/enzo/Avatar/Library/PackageCache/x/ProjectSettings/ProjectVersion.txt",
            "/Users/enzo/Notes/ProjectVersion.txt",
        ]
        .map(PathBuf::from);
        assert_eq!(projects_of_version_files(files.to_vec()), [PathBuf::from("/Users/enzo/Documents/Avatar"), PathBuf::from("/Volumes/SSD/World")]);
    }

    #[test]
    fn unity_hub_and_the_editors_preferences_propose_projects() {
        let temp = tempfile::tempdir().unwrap();
        let prefs = temp.path().join("com.unity3d.UnityEditor5.x.plist");
        let mut all = plist::Dictionary::new();
        all.insert("RecentlyUsedProjectPaths-0".into(), plist::Value::String("/Users/enzo/Avatar".into()));
        all.insert("RecentlyUsedProjectPaths-1".into(), plist::Value::String("/Volumes/SSD/World".into()));
        all.insert("kAutoRefresh".into(), plist::Value::Integer(1.into()));
        // Written as the system does, in binary.
        plist::Value::Dictionary(all).to_file_binary(&prefs).unwrap();
        let mut recent = recent_projects(&prefs);
        recent.sort();
        assert_eq!(recent, [PathBuf::from("/Users/enzo/Avatar"), PathBuf::from("/Volumes/SSD/World")]);
        assert!(recent_projects(&temp.path().join("not there")).is_empty());
        // Unity Hub's two files: every absolute path in them is a proposal.
        let mut paths = vec![];
        collect_paths(&serde_json::json!({"schema_version": "v1", "data": {"/Users/enzo/Hub Avatar": {"path": "/Users/enzo/Hub Avatar", "title": "Hub Avatar"}}}), &mut paths);
        collect_paths(&serde_json::json!({"directoryPath": "/Users/enzo/Unity Projects"}), &mut paths);
        assert!(paths.contains(&PathBuf::from("/Users/enzo/Hub Avatar")) && paths.contains(&PathBuf::from("/Users/enzo/Unity Projects")));
    }
}

#[cfg(test)]
mod retained_lists {
    use super::*;

    /// Verifier, point 8: a stand-in found inside a dot-folder before (kept in the cache) is not proposed again after a
    /// Refresh; the same place registered by the person stays.
    #[test]
    fn cached_finds_in_dot_folders_are_dropped_but_registered_ones_stay() {
        // A base whose own name has no dot (a default temporary folder's does: ".tmp…").
        let temp = tempfile::Builder::new().prefix("dots").tempdir().unwrap();
        let make = |at: &str| {
            let p = temp.path().join(at);
            std::fs::create_dir_all(p.join("ProjectSettings")).unwrap();
            std::fs::write(p.join("ProjectSettings/ProjectVersion.txt"), "m_EditorVersion: 2022.3.22f1\n").unwrap();
            p
        };
        let stand_in = make(".orbiters-build/runs/Stand-in");
        let mine = make(".hidden/Mine");
        let plain = make("Projects/Plain");
        let found: HashMap<String, PathBuf> = [&stand_in, &mine, &plain].into_iter().map(|p| (key(p), p.clone())).collect();
        let registered: HashSet<String> = [key(&mine)].into_iter().collect();
        let mut scan = Scan::default();
        rebuild(&mut scan, &found, &registered);
        let mut kept: Vec<PathBuf> = scan.projects.iter().map(|p| p.path.clone()).collect();
        kept.sort();
        let mut wanted = vec![mine.clone(), plain.clone()];
        wanted.sort();
        assert_eq!(kept, wanted);
    }

    /// PROJECTS-7: 740 publishes over a scan of a minute write the cache a dozen times, not 740.
    #[test]
    fn a_scan_writes_its_cache_now_and_then() {
        let mut checkpoint = Checkpoint::new(CHECKPOINT_EVERY);
        let start = Instant::now();
        let writes = (0..740).filter(|i| checkpoint.due(start + Duration::from_millis(i * 60_000 / 740))).count();
        assert!((12..=13).contains(&writes), "{writes}");
        assert!(Checkpoint::new(CHECKPOINT_EVERY).due(start), "the first one at once");
    }

    const AVATAR: &str = "avtr_11111111-2222-3333-4444-555555555555";
    const OTHER_AVATAR: &str = "avtr_77777777-2222-3333-4444-555555555555";
    const WORLD: &str = "wrld_99999999-2222-3333-4444-555555555555";

    /// A scene with one uploaded avatar or world.
    fn scene(name: &str, id: &str) -> String {
        format!(
            "%YAML 1.1\n--- !u!1 &100\nGameObject:\n  m_Name: {name}\n--- !u!4 &101\nTransform:\n  m_GameObject: {{fileID: 100}}\n  m_Father: {{fileID: 0}}\n--- !u!114 &102\nMonoBehaviour:\n  m_GameObject: {{fileID: 100}}\n  blueprintId: {id}\n"
        )
    }

    /// What a project shows, its uploaded avatars and its worlds, stays what it showed while one of its scenes cannot
    /// be read: a scene with bytes that are no text (damaged, or caught half written), or one with a line that is read
    /// and longer than is looked at of a line. Such a scene answering "nothing uploaded" as a complete answer took the
    /// avatar off its project, and the world with it. Once the scene reads again, the project shows what it holds.
    #[test]
    fn a_project_keeps_its_avatars_and_worlds_while_a_scene_cannot_be_read() {
        let dir = tempfile::tempdir().unwrap();
        let assets = dir.path().join("Assets");
        std::fs::create_dir(&assets).unwrap();
        let (avatar, world) = (assets.join("Avatar.unity"), assets.join("World.unity"));
        std::fs::write(&avatar, scene("Fox", AVATAR)).unwrap();
        std::fs::write(&world, scene("VRCWorld", WORLD)).unwrap();
        let mut scan = Scan { projects: vec![Project { path: dir.path().to_path_buf(), name: "Project".into(), kind: Kind::Avatars, ..Default::default() }], ..Default::default() };
        let cancel = AtomicBool::new(false);
        let read_again = |scan: &mut Scan| enrich(scan, &cancel, &mut HashSet::new(), &mut |_: &Scan| {});
        let shown = |scan: &Scan| {
            let ids = |list: &[Avatar]| list.iter().map(|a| a.id.clone()).collect::<Vec<_>>();
            (ids(&scan.projects[0].avatars), ids(&scan.projects[0].worlds))
        };
        read_again(&mut scan);
        assert_eq!(shown(&scan), (vec![AVATAR.to_string()], vec![WORLD.to_string()]));

        // The avatar's scene with a byte that is no text in its blueprint ID.
        let mut damaged = scene("Fox", AVATAR).into_bytes();
        let at = damaged.windows(AVATAR.len()).position(|w| w == AVATAR.as_bytes()).unwrap();
        damaged[at + 5] = 0xff;
        damaged.push(b'\n');
        std::fs::write(&avatar, &damaged).unwrap();
        read_again(&mut scan);
        assert_eq!(shown(&scan), (vec![AVATAR.to_string()], vec![WORLD.to_string()]), "a damaged scene took the project's avatar off it");

        // The world's scene with a name longer than is looked at of a line, the avatar's still damaged.
        std::fs::write(&world, scene(&"W".repeat(100_000), WORLD)).unwrap();
        read_again(&mut scan);
        assert_eq!(shown(&scan), (vec![AVATAR.to_string()], vec![WORLD.to_string()]), "a scene read short changed what the project shows");

        // Both read again: the project shows what they hold now.
        std::fs::write(&avatar, scene("Another fox", OTHER_AVATAR)).unwrap();
        std::fs::write(&world, scene("VRCWorld again", WORLD)).unwrap();
        read_again(&mut scan);
        assert_eq!(shown(&scan), (vec![OTHER_AVATAR.to_string()], vec![WORLD.to_string()]));
    }
}
