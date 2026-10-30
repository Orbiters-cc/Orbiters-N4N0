//! How much room a project takes on disk, and how much of it is Library (what Unity makes again from the project's own
//! files when it is missing). Walking a large project takes seconds, so each answer is kept
//! (`Orbiters/project-sizes.json`) with what it was measured against: the modification times of the folders a change
//! of size shows in (the project's own, Assets, Packages, ProjectSettings and Library, which Unity touches each time
//! it opens the project). A project whose stamp is the same, measured less than a day ago, is not walked again.
use crate::discovery;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// A measured answer is walked again after this long even when its stamp did not change (files changed deep in
/// Assets leave the folders above them as they were).
pub const FRESH_SECS: u64 = 24 * 3600;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Size {
    /// Every file's length, in bytes.
    pub total: u64,
    /// Of `total`, what Library holds.
    pub library: u64,
    pub files: u64,
}

impl Size {
    /// What a copy without Library takes (Temp, Logs and obj are small next to it).
    pub fn without_library(&self) -> u64 {
        self.total.saturating_sub(self.library)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Entry {
    stamp: u64,
    /// Unix seconds.
    measured: u64,
    size: Size,
}

/// What the cache says of a project: its last size (if it was ever measured) and whether that still holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lookup {
    pub size: Option<Size>,
    pub fresh: bool,
}

fn cache_file() -> PathBuf {
    crate::data_dir().join("project-sizes.json")
}

/// One read-modify-write of the cache at a time.
static CACHE: Mutex<()> = Mutex::new(());

fn read(file: &Path) -> HashMap<String, Entry> {
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// What a change of size shows in: the latest modification time of the project's folder and the folders under it
/// that hold its files.
pub fn stamp(project: &Path) -> u64 {
    ["", "Assets", "Packages", "ProjectSettings", "Library", "Packages/vpm-manifest.json"]
        .iter()
        .map(|p| discovery::fingerprint(&project.join(p)))
        .fold(0, u64::max)
}

/// The cached size of `project`. Reads one small file; call on a worker.
pub fn lookup(project: &Path) -> Lookup {
    lookup_in(&cache_file(), project, discovery::now())
}

fn lookup_in(file: &Path, project: &Path, now: u64) -> Lookup {
    let _one = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match read(file).remove(&discovery::key(project)) {
        Some(e) => Lookup { size: Some(e.size), fresh: e.stamp == stamp(project) && now.saturating_sub(e.measured) < FRESH_SECS },
        None => Lookup::default(),
    }
}

/// Walks `project` (never following links or reparse points) and keeps the answer. Seconds for a large project: call
/// on a worker; `cancel` stops it.
pub fn measure(project: &Path, cancel: &AtomicBool) -> anyhow::Result<Size> {
    measure_in(&cache_file(), project, cancel, discovery::now())
}

fn measure_in(file: &Path, project: &Path, cancel: &AtomicBool, now: u64) -> anyhow::Result<Size> {
    anyhow::ensure!(project.is_dir(), "The project's folder is missing.");
    // Stamped before the walk: a change while it runs makes the answer stale, not the next one wrong.
    let stamp = stamp(project);
    let size = walk(project, cancel)?;
    let _one = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read(file);
    all.insert(discovery::key(project), Entry { stamp, measured: now, size });
    // A cache that cannot be written only means walking again next time.
    let _ = crate::write_json(file, &all);
    Ok(size)
}

fn walk(project: &Path, cancel: &AtomicBool) -> anyhow::Result<Size> {
    let library = project.join("Library");
    let mut size = Size::default();
    let entries = walkdir::WalkDir::new(project)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.metadata().ok().is_some_and(|m| discovery::is_link(&m)));
    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("Cancelled");
        }
        // A file that went away or cannot be read while walking: left out.
        let Ok(e) = entry else { continue };
        if !e.file_type().is_file() {
            continue;
        }
        let len = e.metadata().map(|m| m.len()).unwrap_or(0);
        size.total += len;
        size.files += 1;
        if e.path().starts_with(&library) {
            size.library += len;
        }
    }
    Ok(size)
}

/// The room left on the disk that holds `path` (or the nearest folder above it that exists), for the user running
/// this.
pub fn free_space(path: &Path) -> Option<u64> {
    let mut dir = path;
    while !dir.exists() {
        dir = dir.parent()?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free = 0u64;
        // SAFETY: a valid, null-terminated path and a valid out pointer.
        unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free as *mut u64), None, None) }.ok()?;
        Some(free)
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(root: &Path) {
        for (file, len) in [("Assets/a.png", 1000), ("Assets/Sub/b.fbx", 500), ("Library/ArtifactDB", 4000), ("ProjectSettings/ProjectVersion.txt", 24)] {
            let p = root.join(file);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![0u8; len]).unwrap();
        }
    }

    #[test]
    fn measures_a_project_and_its_library() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("Project");
        project(&p);
        let cache = temp.path().join("sizes.json");
        let size = measure_in(&cache, &p, &AtomicBool::new(false), 1000).unwrap();
        assert_eq!(size, Size { total: 5524, library: 4000, files: 4 });
        assert_eq!(size.without_library(), 1524);
        assert!(measure_in(&cache, &p, &AtomicBool::new(true), 1000).is_err(), "cancelled");
    }

    #[test]
    fn a_kept_size_holds_until_the_project_changes_or_a_day_passed() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("Project");
        project(&p);
        let cache = temp.path().join("sizes.json");
        assert_eq!(lookup_in(&cache, &p, 1000), Lookup::default());
        let size = measure_in(&cache, &p, &AtomicBool::new(false), 1000).unwrap();
        assert_eq!(lookup_in(&cache, &p, 1000 + 60), Lookup { size: Some(size), fresh: true });
        assert_eq!(lookup_in(&cache, &p, 1000 + FRESH_SECS), Lookup { size: Some(size), fresh: false });
        // A file added at the project's top changes its folder's stamp: the size shown stays until measured again.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(p.join("Added.txt"), "new").unwrap();
        assert_eq!(lookup_in(&cache, &p, 1000 + 60), Lookup { size: Some(size), fresh: false });
        // Another spelling of the same folder finds the same entry.
        let other = PathBuf::from(p.to_string_lossy().replace('\\', "/").to_uppercase());
        if cfg!(windows) {
            assert!(lookup_in(&cache, &other, 1000).size.is_some());
        }
    }

    #[test]
    fn free_space_is_read_from_the_nearest_folder() {
        let temp = tempfile::tempdir().unwrap();
        if cfg!(windows) {
            assert!(free_space(&temp.path().join("not/made/yet")).is_some_and(|f| f > 0));
        }
    }
}
