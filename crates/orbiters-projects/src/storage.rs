//! Project-specific Unity disk failures and recovery of regenerable Library data.
use anyhow::{bail, Context};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const IMPORT_RESERVE: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct Health {
    pub free: Option<u64>,
    pub disk_error: bool,
    pub open: bool,
}
impl Health {
    pub fn needs_repair(&self) -> bool {
        self.free.is_some_and(|n| n < IMPORT_RESERVE)
    }
}

pub fn matches_disk_error(log: &str, project: &Path) -> bool {
    let path = project.to_string_lossy().replace('\\', "/").to_lowercase();
    log.lines().any(|line| {
        let line = line.replace('\\', "/").to_lowercase();
        (line.contains("enospc") || line.contains("no space left on device")) && line.contains(&format!("{path}/"))
    })
}

pub fn inspect(project: &Path) -> Health {
    let library = project.join("Library");
    let location = if library.is_dir() { &library } else { project };
    let log = dirs::data_local_dir().map(|p| p.join("Unity/Editor/Editor.log"));
    let mut text = Vec::new();
    if let Some(mut file) = log.and_then(|p| std::fs::File::open(p).ok()) {
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let _ = file.seek(SeekFrom::Start(len.saturating_sub(1024 * 1024)));
        let _ = file.take(1024 * 1024).read_to_end(&mut text);
    }
    Health {
        free: crate::size::free_space(location),
        disk_error: matches_disk_error(&String::from_utf8_lossy(&text), project),
        open: crate::unity_open(project),
    }
}

/// Copy all cache bytes before replacing its location. No project assets, settings or package manifests are edited.
pub fn prepare(project: &Path) -> anyhow::Result<Option<PathBuf>> {
    let health = inspect(project);
    if !health.needs_repair() {
        return Ok(None);
    }
    anyhow::ensure!(!health.open, "Unity ran out of disk space. Quit this project's Unity window, then choose Repair storage in Orbiters.");
    #[cfg(not(windows))]
    bail!("Not enough space for Unity imports. Free at least 8 GB on the project's disk.");
    #[cfg(windows)]
    {
        let project = project.canonicalize()?;
        let _lock = project_lock(&project)?;
        let library = project.join("Library");
        if std::fs::symlink_metadata(&library).is_ok_and(|m| crate::discovery::is_link(&m)) {
            bail!("The project's Library is already redirected and its disk is full. Free space on that disk before retrying.");
        }
        let mut files = Vec::new();
        let mut bytes = 0u64;
        if library.is_dir() {
            for entry in walkdir::WalkDir::new(&library).follow_links(false) {
                let entry = entry?;
                anyhow::ensure!(
                    !crate::discovery::is_link(&entry.metadata()?),
                    "Library contains a linked folder; automatic relocation stopped without changing it."
                );
                if entry.file_type().is_file() {
                    bytes = bytes.saturating_add(entry.metadata()?.len());
                    files.push(entry.path().to_path_buf());
                }
            }
        }
        let needed = bytes.saturating_add(IMPORT_RESERVE);
        let mut disks = crate::discovery::fixed_disks();
        let local = dirs::data_local_dir();
        disks.sort_by_key(|d| (!local.as_ref().is_some_and(|p| p.starts_with(d)), std::cmp::Reverse(crate::size::free_space(d).unwrap_or(0))));
        let disk = disks
            .into_iter()
            .find(|d| crate::size::free_space(d).is_some_and(|free| free >= needed))
            .context("No local disk has enough space for automatic recovery. Free disk space and try again.")?;
        let cache_root = local.filter(|p| p.starts_with(&disk)).unwrap_or(disk).join("Orbiters/UnityCache");
        let target = cache_root.join(uuid::Uuid::new_v4().simple().to_string());
        relocate(&project, &target, &files, |target, library| junction::create(target, library))?;
        Ok(Some(target))
    }
}

#[cfg(windows)]
fn project_lock(project: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::create_dir_all(project.join("Temp"))?;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(project.join("Temp/UnityLockfile"))
        .context("Close this project's Unity window before repairing storage")
}

#[cfg(windows)]
fn relocate(project: &Path, target: &Path, files: &[PathBuf], link: impl FnOnce(&Path, &Path) -> std::io::Result<()>) -> anyhow::Result<()> {
    let library = project.join("Library");
    anyhow::ensure!(!target.exists(), "Cache destination already exists");
    std::fs::create_dir_all(target)?;
    crate::write_json(&target.with_extension("json"), &serde_json::json!({"project":project,"library":target}))?;
    for file in files {
        let relative = file.strip_prefix(&library)?;
        let to = target.join(relative);
        std::fs::create_dir_all(to.parent().unwrap())?;
        std::fs::copy(file, &to).with_context(|| format!("Cache relocation incomplete; original is intact. Partial copy: {}", target.display()))?;
        anyhow::ensure!(same_bytes(file, &to)?, "Cache copy verification failed; original is intact");
    }
    let backup = project.join(format!("Library.orbiters-relocation-{}", uuid::Uuid::new_v4().simple()));
    let had_library = library.is_dir();
    if had_library {
        std::fs::rename(&library, &backup)?;
    }
    if let Err(error) = link(target, &library) {
        // A failed reparse-point creation can leave an empty directory behind.
        if junction::exists(&library).unwrap_or(false) {
            junction::delete(&library)?;
        }
        if library.exists() {
            std::fs::remove_dir(&library).context("Could not remove an empty failed Library redirect")?;
        }
        if had_library {
            std::fs::rename(&backup, &library).context("Could not restore Library after relocation failure")?;
        }
        return Err(error).context("Could not redirect Unity's cache");
    }
    // The backup is exactly the Library just renamed above, with links rejected before copying.
    if had_library {
        std::fs::remove_dir_all(&backup)
            .with_context(|| format!("Cache moved to {}, but old cache cleanup failed at {}", target.display(), backup.display()))?;
    }

    Ok(())
}

fn same_bytes(a: &Path, b: &Path) -> std::io::Result<bool> {
    if std::fs::metadata(a)?.len() != std::fs::metadata(b)?.len() {
        return Ok(false);
    }
    let mut a = std::io::BufReader::new(std::fs::File::open(a)?);
    let mut b = std::io::BufReader::new(std::fs::File::open(b)?);
    let mut left = [0u8; 65536];
    let mut right = [0u8; 65536];
    loop {
        let n = a.read(&mut left)?;
        if n == 0 {
            return Ok(true);
        }
        b.read_exact(&mut right[..n])?;
        if left[..n] != right[..n] {
            return Ok(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn another_editor_or_repair_cannot_acquire_the_project_lock() {
        let temp = tempfile::tempdir().unwrap();
        let first = project_lock(temp.path()).unwrap();
        assert!(project_lock(temp.path()).is_err());
        drop(first);
        assert!(project_lock(temp.path()).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn relocation_preserves_cache_and_rolls_back_failed_redirect() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        let library = project.join("Library");
        std::fs::create_dir_all(&library).unwrap();
        let file = library.join("cached.bin");
        std::fs::write(&file, b"cached content").unwrap();
        let failed = temp.path().join("failed");
        assert!(relocate(&project, &failed, &[file.clone()], |_, l| {
            std::fs::create_dir(l)?;
            Err(std::io::Error::other("injected failure"))
        })
        .is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"cached content");
        let target = temp.path().join("relocated");
        relocate(&project, &target, &[file.clone()], |t, l| junction::create(t, l)).unwrap();
        assert!(junction::exists(&library).unwrap());
        assert_eq!(std::fs::read(&file).unwrap(), b"cached content");
        assert_eq!(std::fs::read(target.join("cached.bin")).unwrap(), b"cached content");
        junction::delete(&library).unwrap();
    }

    #[test]
    fn disk_errors_are_bound_to_the_exact_project() {
        let log = "com.unity.burst: ENOSPC: no space left on device, copyfile 'cache' -> 'H:\\Projects\\Avatar\\Library\\PackageCache\\.tmp\\file'";
        assert!(matches_disk_error(log, Path::new("H:/Projects/Avatar")));
        assert!(!matches_disk_error(log, Path::new("H:/Projects/Avatar copy")));
        assert!(!matches_disk_error("missing dependency", Path::new("H:/Projects/Avatar")));
    }
    #[test]
    fn historical_error_does_not_relocate_a_disk_with_space() {
        assert!(!Health { free: Some(IMPORT_RESERVE), disk_error: true, open: false }.needs_repair());
        assert!(Health { free: Some(1024), disk_error: false, open: false }.needs_repair());
        assert!(!Health { free: None, disk_error: true, open: false }.needs_repair());
    }
}
