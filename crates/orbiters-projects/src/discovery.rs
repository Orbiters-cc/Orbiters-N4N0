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
            Self::Avatars => "Avatars",
            Self::Worlds => "Worlds",
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
        "windows" | "programdata" | "$recycle.bin" | "system volume information" | "appdata" | "library" | "temp" | "logs" | "obj" | "node_modules" | ".git"
    ) || n.starts_with("program files")
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
    path.join("ProjectSettings/ProjectVersion.txt").is_file()
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
    }
}
pub fn cached() -> Scan {
    std::fs::read(crate::data_dir().join("unity-projects.json")).ok().and_then(|s| serde_json::from_slice(&s).ok()).unwrap_or_default()
}

pub fn seeds(registered: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = registered.to_vec();
    if let Some(dir) = dirs::config_dir() {
        if let Ok(text) = std::fs::read_to_string(dir.join("UnityHub/projects-v1.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                collect_paths(&v, &mut paths);
            }
        }
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
    #[cfg(not(windows))]
    {
        vec![]
    }
}
pub fn low_priority() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::*;
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

/// Streams quick results before walking disks. No project/store mutation occurs here.
pub fn discover(mut scan: Scan, force: bool, cancel: &AtomicBool, publish: impl FnMut(&Scan)) -> anyhow::Result<Scan> {
    low_priority();
    let mut notify = publish;
    let mut publish = |scan: &Scan| {
        // Keep useful results across a window close while a large disk is still being enumerated.
        if !cancel.load(Ordering::Relaxed) {
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
    scan.status = "Finding nearby projects".into();
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
        walk(&r, Some(2), cancel, &mut found, &AtomicU64::new(0), None);
    }
    rebuild(&mut scan, &found, &known);
    publish(&scan);
    let mut enriched = HashSet::new();
    enrich(&mut scan, cancel, &mut enriched, &mut publish);
    if force || now().saturating_sub(scan.last_full) >= 3600 {
        scan.status = "Scanning fixed local disks".into();
        publish(&scan);
        let start = Instant::now();
        let mut count = 0;
        for disk in fixed_disks() {
            scan.status = format!("Scanning {}", disk.display());
            publish(&scan);
            count += scan_disk(&disk, cancel, &mut found, &mut scan.warnings);
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
fn rebuild(scan: &mut Scan, found: &HashMap<String, PathBuf>, registered: &HashSet<String>) {
    let old: HashMap<_, _> = scan.projects.iter().map(|p| (key(&p.path), p.avatars.clone())).collect();
    scan.projects = found
        .iter()
        .filter(|(k, p)| registered.contains(*k) || is_project(p))
        .map(|(k, p)| {
            let mut project = describe(p.clone(), registered.contains(k), &mut scan.scenes);
            project.avatars = old.get(k).cloned().unwrap_or_default();
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
        if p.kind != Kind::Avatars || !enriched.insert(key(&p.path)) {
            continue;
        }
        let path = p.path.clone();
        scan.status = format!("Reading avatars in {}", p.name);
        publish(scan);
        scan.projects[i].avatars = avatars::in_project(&path, &mut scan.scenes);
        publish(scan);
    }
}
enum DiskEvent {
    Found(PathBuf),
    Done,
}
/// A damaged/offline volume must not hold every other disk hostage. Cancel an enumeration after 60 s without a
/// single directory completed, retain discoveries already received, and report that volume in the scan warnings.
fn scan_disk(root: &Path, cancel: &AtomicBool, found: &mut HashMap<String, PathBuf>, warnings: &mut Vec<String>) -> u64 {
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(AtomicU64::new(0));
    let worker = {
        let root = root.to_path_buf();
        let stop = stop.clone();
        let progress = progress.clone();
        std::thread::spawn(move || {
            low_priority();
            walk(&root, None, &stop, &mut HashMap::new(), &progress, Some(&tx));
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
            warnings.push(format!("{} stopped responding for 60 seconds. Its scan was interrupted; Refresh retries it.", root.display()));
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
fn walk(
    root: &Path,
    depth: Option<usize>,
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
            if e.file_type().is_ok_and(|t| t.is_dir()) && !skip_dir(&e.file_name().to_string_lossy()) {
                stack.push((e.path(), d + 1));
            }
        }
    }
}

pub fn find_editor(version: &str) -> Option<PathBuf> {
    // Search only the requested version: opening another editor could upgrade a project.
    let mut candidates = vec![PathBuf::from(format!(r"C:\Program Files\Unity\Hub\Editor\{version}\Editor\Unity.exe"))];
    if let Some(dir) = dirs::config_dir() {
        let hub = dir.join("UnityHub");
        if let Ok(s) = std::fs::read_to_string(hub.join("editors-v2.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                editor_entries(&v, version, &mut candidates);
            }
        }
        if let Ok(s) = std::fs::read_to_string(hub.join("secondaryInstallPath.json")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                for value in [v.as_str(), v["secondaryInstallPath"].as_str()] {
                    if let Some(p) = value {
                        candidates.push(PathBuf::from(p).join(version).join("Editor/Unity.exe"));
                    }
                }
            }
        }
    }
    if let Ok(s) = std::fs::read_to_string(crate::vcc_dir().join("settings.json")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(p) = v["preferredUnityEditors"][version].as_str() {
                candidates.push(PathBuf::from(p));
            }
        }
    }
    if let Some(p) = candidates.iter().find(|p| p.is_file()) {
        return Some(p.clone());
    }
    // Editors VCC or ALCOM found or were given (outside Unity Hub's lists).
    crate::runtime().ok()?.block_on(async {
        let io = vrc_get_vpm::io::DefaultEnvironmentIo::new(crate::vcc_dir().into());
        let db = vrc_get_vpm::environment::VccDatabaseConnection::connect(&io).await.ok()?;
        let wanted = vrc_get_vpm::version::UnityVersion::parse(version)?;
        db.get_unity_installations().iter().filter(|u| u.version() == Some(wanted)).filter_map(|u| u.path().map(PathBuf::from)).find(|p| p.is_file())
    })
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
                        let p = PathBuf::from(p);
                        out.push(if p.extension().is_some() { p } else { p.join("Editor/Unity.exe") });
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
