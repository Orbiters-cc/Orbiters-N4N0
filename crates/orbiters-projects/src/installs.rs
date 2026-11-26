//! A package change that ends halfway (PRJ-23). The library that makes it (vrc-get) moves the folders it replaces to the project's
//! `Temp/vrc-get/<id>/remove`, moves the new ones in and saves the manifest last; on an error it moves everything back, but a process
//! that ends in between (a kill, a power cut) leaves the project without its packages, and Unity empties `Temp`. Each change is
//! therefore recorded, outside the project, until it is over. While the manifest is as it was, what the change had moved away is put
//! back: before the next change, and before Unity opens the project. Only where nothing is in its place: what is there stays, and
//! what the project had is kept beside, named. Costs one small file a change, no copy of a package.
use crate::owned::{id_of, try_is_same, FileId};
use crate::{own, repos};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    project: PathBuf,
    project_id: FileId,
    /// The exact previous manifest, or its confirmed absence. The manifest is published after the package moves.
    manifest: Option<String>,
    /// The packages it removes or replaces that were there.
    packages: Vec<String>,
    /// Packages whose folders were absent before the change.
    #[serde(default)]
    added: Vec<String>,
}

const MANIFEST: &str = "Packages/vpm-manifest.json";

fn dir(root: &Path) -> PathBuf {
    repos::dir(root).join("package-changes")
}

/// A file that is there and could not be read now (W1-2): what it says is not known, so nothing is decided on it.
pub(crate) fn unreadable(path: &Path, why: std::io::Error) -> std::io::Error {
    std::io::Error::new(why.kind(), tr!("{path} cannot be read right now ({why}). Try again in a moment.", path = path.display(), why = why))
}

/// Read the commit marker without treating a failed read or invalid JSON as a committed change.
fn manifest(path: &Path) -> std::io::Result<Option<String>> {
    let file = path.join(MANIFEST);
    let read = || -> std::io::Result<Option<String>> {
        let Some(bytes) = own::steady(&file)? else { return Ok(None) };
        let text = String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(text.trim_start_matches('\u{feff}'))?;
        Ok(Some(text))
    };
    read().map_err(|e| unreadable(&file, e))
}

/// Whether there is something at `path`; an error when that cannot be told now.
fn there(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(unreadable(path, e)),
    }
}

/// A change being made. Dropped before it is [`Installing::over`], its record stays.
pub(crate) struct Installing {
    file: PathBuf,
    held: Option<std::fs::File>,
}

impl Installing {
    /// Records that `packages` of the project at `path` are about to be added, removed or replaced. Nothing is changed when it
    /// cannot be.
    pub(crate) fn begin<'a>(root: &Path, path: &Path, packages: impl Iterator<Item = &'a str>) -> anyhow::Result<Installing> {
        let file = dir(root).join(format!("{}.json", uuid::Uuid::new_v4().simple()));
        let begun = (|| -> anyhow::Result<Installing> {
            let (mut there_before, mut added) = (vec![], vec![]);
            for name in packages {
                let list = if there(&path.join("Packages").join(name))? { &mut there_before } else { &mut added };
                list.push(name.to_owned());
            }
            let record = Record { project: path.into(), project_id: id_of(path)?, manifest: manifest(path)?, packages: there_before, added };
            let held = own::lock(&file)?;
            crate::write_json(&file, &record)?;
            Ok(Installing { file, held: Some(held) })
        })();
        begun.map_err(|e| e.context(tr("Could not record the package change before making it: nothing was changed")))
    }

    /// It is over, one way or the other.
    pub(crate) fn over(mut self) {
        self.held = None;
        forget(&self.file);
    }
}

fn forget(file: &Path) {
    let _ = crate::disk::remove_file(file);
    let _ = crate::disk::remove_file(file.with_extension("lock"));
}

/// Whether the change recorded in `file` is being made now, by this process or another; an error when that cannot be told now.
fn runs(file: &Path) -> std::io::Result<bool> {
    let lock = file.with_extension("lock");
    let opened = match std::fs::File::open(&lock) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        opened => opened.map_err(|e| unreadable(&lock, e))?,
    };
    match opened.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(e)) => Err(unreadable(&lock, e)),
    }
}

/// Puts back what changes of the project at `path` that ended halfway had moved away, and takes out what they had moved in. An
/// error says what could not be, and where what the project had is; it is said once. A record, a manifest or a folder that cannot
/// be read now is an error too, and the record stays for the next time (W1-2): it is the only one of what was moved where.
pub(crate) fn recover(root: &Path, path: &Path) -> anyhow::Result<()> {
    let records = dir(root);
    let entries = match std::fs::read_dir(&records) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        entries => entries.map_err(|e| unreadable(&records, e))?,
    };
    let mut problems = vec![];
    for entry in entries {
        let file = match entry {
            Ok(e) => e.path(),
            Err(e) => {
                problems.push(unreadable(&records, e).to_string());
                continue;
            }
        };
        if file.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let record = match own::steady(&file).and_then(|bytes| bytes.map(|b| serde_json::from_slice::<Record>(&b)).transpose().map_err(Into::into)) {
            Ok(Some(record)) => record,
            Ok(None) => continue,
            Err(e) => {
                problems.push(unreadable(&file, e).to_string());
                continue;
            }
        };
        if !crate::discovery::same(&record.project, path) {
            continue;
        }
        let undone = (|| -> std::io::Result<_> {
            if !try_is_same(path, record.project_id).map_err(|e| unreadable(path, e))? || runs(&file)? {
                return Ok(None);
            }
            let now = manifest(path)?;
            if now == record.manifest {
                return undo(path, &file, &record).map(Some);
            }
            if now.is_none() {
                let marker = path.join(MANIFEST);
                return Err(unreadable(&marker, std::io::ErrorKind::NotFound.into()));
            }
            Ok(Some(vec![]))
        })();
        match undone {
            // Being made now: its own process ends it.
            Ok(None) => continue,
            Ok(Some(said)) => problems.extend(said),
            // Still in use, or not readable now, say: tried again the next time.
            Err(e) => {
                problems.push(e.to_string());
                continue;
            }
        }
        forget(&file);
    }
    anyhow::ensure!(problems.is_empty(), "{}", tr!("A package change of this project ended halfway (Orbiters closed during it). {problems}", problems = problems.join(" ")));
    Ok(())
}

/// Restore missing old folders and keep ambiguous contents outside Unity's temporary directory.
fn undo(path: &Path, file: &Path, record: &Record) -> std::io::Result<Vec<String>> {
    let named = |n: &&String| {
        let mut parts = Path::new(n).components();
        matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none()
    };
    let id = file.file_stem().unwrap_or_default().to_string_lossy();
    let out = path.join(format!(".orbiters-added-{id}"));
    let mut said = vec![];
    for name in record.added.iter().filter(named) {
        let at = path.join("Packages").join(name);
        let kept = out.join(name);
        if there(&at)? {
            crate::disk::create_dir_all(&out)?;
            if there(&kept)? {
                return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, kept.display().to_string()));
            }
            crate::disk::rename(&at, &kept)?;
        }
        if there(&kept)? {
            said.push(tr!("{name} was added before the change finished. Its files are kept in {path}.", name = name, path = kept.display()));
        }
    }
    for name in record.packages.iter().filter(named) {
        let at = path.join("Packages").join(name);
        let Some(had) = moved(path, name)? else { continue };
        let failed = |e: std::io::Error, to: &Path| std::io::Error::new(e.kind(), tr!("{name} could not be put back from {from} to {to}: {why}", name = name, from = had.display(), to = to.display(), why = e));
        if !there(&at)? {
            crate::disk::rename(&had, &at).map_err(|e| failed(e, &at))?;
        } else {
            let beside = path.join(format!(".orbiters-before-{}", uuid::Uuid::new_v4().simple()));
            crate::disk::create_dir(&beside).and_then(|()| crate::disk::rename(&had, beside.join(name))).map_err(|e| failed(e, &beside))?;
            said.push(tr!("{name} may be the new one: what the project had is kept in {path}. Restore installs the version the project's list has.", name = name, path = beside.display()));
        }
    }
    Ok(said)
}

/// The folder vrc-get moved the package `name` of the project at `path` to; an error when its folder cannot be looked through now.
fn moved(path: &Path, name: &str) -> std::io::Result<Option<PathBuf>> {
    let temp = path.join("Temp/vrc-get");
    let entries = match std::fs::read_dir(&temp) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        entries => entries.map_err(|e| unreadable(&temp, e))?,
    };
    let mut found = vec![];
    for entry in entries {
        let candidate = entry.map_err(|e| unreadable(&temp, e))?.path().join("remove").join(name);
        if there(&candidate)? {
            found.push(candidate);
        }
    }
    Ok(found.into_iter().max_by_key(|p| crate::discovery::fingerprint(p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(p: &Path) {
        for (name, file) in [("com.a", "mine"), ("com.b", "b")] {
            std::fs::create_dir_all(p.join("Packages").join(name)).unwrap();
            std::fs::write(p.join("Packages").join(name).join("package.json"), file).unwrap();
        }
        std::fs::write(p.join(MANIFEST), "{}").unwrap();
    }
    fn records(root: &Path) -> usize {
        std::fs::read_dir(dir(root)).into_iter().flatten().count()
    }
    /// The state a kill leaves: what vrc-get had moved away when it came.
    fn cut(root: &Path, p: &Path, moved: &[&str]) {
        let begun = Installing::begin(root, p, ["com.a", "com.b", "com.new"].into_iter()).unwrap();
        let away = p.join("Temp/vrc-get/x/remove");
        std::fs::create_dir_all(&away).unwrap();
        for name in moved {
            std::fs::rename(p.join("Packages").join(name), away.join(name)).unwrap();
        }
        drop(begun);
    }

    /// PRJ-23: the app ended between "the old out" and "the new in": the project has its packages again, local edits included.
    #[test]
    fn a_change_cut_by_a_kill_is_put_back() {
        let temp = tempfile::tempdir().unwrap();
        let (root, p) = (temp.path().join("vcc"), temp.path().join("Project"));
        project(&p);
        cut(&root, &p, &["com.a", "com.b"]);
        assert!(!p.join("Packages/com.a").exists());
        // Another project's is not looked at.
        let other = temp.path().join("Other");
        project(&other);
        recover(&root, &other).unwrap();
        assert!(records(&root) == 2 && !p.join("Packages/com.a").exists());
        recover(&root, &p).unwrap();
        assert_eq!(std::fs::read_to_string(p.join("Packages/com.a/package.json")).unwrap(), "mine");
        assert!(p.join("Packages/com.b/package.json").is_file());
        assert_eq!(records(&root), 0);
        recover(&root, &p).unwrap();
    }

    /// What is in a package's place is never replaced: what the project had is set aside, and said once.
    #[test]
    fn what_is_in_a_packages_place_stays() {
        let temp = tempfile::tempdir().unwrap();
        let (root, p) = (temp.path().join("vcc"), temp.path().join("Project"));
        project(&p);
        cut(&root, &p, &["com.a", "com.b"]);
        std::fs::create_dir_all(p.join("Packages/com.a")).unwrap();
        std::fs::write(p.join("Packages/com.a/package.json"), "newer work").unwrap();
        let said = recover(&root, &p).unwrap_err().to_string();
        let beside = std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path()).find(|p| p.file_name().unwrap().to_string_lossy().starts_with(".orbiters-before-")).unwrap();
        assert!(said.contains("com.a") && said.contains(&beside.display().to_string()), "{said}");
        assert_eq!(std::fs::read_to_string(p.join("Packages/com.a/package.json")).unwrap(), "newer work");
        assert_eq!(std::fs::read_to_string(beside.join("com.a/package.json")).unwrap(), "mine");
        assert!(p.join("Packages/com.b/package.json").is_file(), "the other one is put back");
        recover(&root, &p).unwrap();
    }

    /// A change that went through, one that is being made, one of a folder that was here before: nothing is put back.
    #[test]
    fn only_a_change_that_ended_halfway_is_put_back() {
        let temp = tempfile::tempdir().unwrap();
        let (root, p) = (temp.path().join("vcc"), temp.path().join("Project"));
        project(&p);
        let begun = Installing::begin(&root, &p, ["com.a"].into_iter()).unwrap();
        std::fs::create_dir_all(p.join("Temp/vrc-get/x/remove")).unwrap();
        std::fs::rename(p.join("Packages/com.a"), p.join("Temp/vrc-get/x/remove/com.a")).unwrap();
        recover(&root, &p).unwrap();
        assert!(!p.join("Packages/com.a").exists(), "being made");
        std::fs::write(p.join(MANIFEST), r#"{"locked":{}}"#).unwrap();
        drop(begun);
        recover(&root, &p).unwrap();
        assert!(!p.join("Packages/com.a").exists() && records(&root) == 0, "it went through");
        cut(&root, &p, &["com.b"]);
        std::fs::rename(&p, temp.path().join("Moved")).unwrap();
        project(&p);
        recover(&root, &p).unwrap();
        assert_eq!(records(&root), 2, "another folder at that place: left for its own");
        begun_is_over_leaves_nothing(&root, &p);
    }
    /// W1-2: the manifest held a moment by another program (an antivirus) is not a manifest that changed: the record stays, the
    /// recovery is an error (Open does not go on), and the next one puts the package back.
    #[cfg(windows)]
    #[test]
    fn a_manifest_that_cannot_be_read_now_keeps_the_record() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = tempfile::tempdir().unwrap();
        let (root, p) = (temp.path().join("vcc"), temp.path().join("Project"));
        project(&p);
        cut(&root, &p, &["com.a"]);
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(p.join(MANIFEST)).unwrap();
        let result = recover(&root, &p);
        drop(held);
        let said = format!("{:#}", result.unwrap_err());
        assert!(said.contains("vpm-manifest.json") && records(&root) != 0, "{said}");
        assert!(!p.join("Packages/com.a").exists());
        std::fs::remove_file(p.join(MANIFEST)).unwrap();
        assert!(recover(&root, &p).is_err() && records(&root) == 2);
        std::fs::write(p.join(MANIFEST), b"{").unwrap();
        assert!(recover(&root, &p).is_err() && records(&root) == 2);
        std::fs::write(p.join(MANIFEST), b"{}").unwrap();
        recover(&root, &p).unwrap();
        assert_eq!(std::fs::read_to_string(p.join("Packages/com.a/package.json")).unwrap(), "mine");
        assert_eq!(records(&root), 0);
        // Nor is one that cannot be read when a change begins: nothing is recorded, nothing changed.
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(p.join(MANIFEST)).unwrap();
        assert!(Installing::begin(&root, &p, ["com.a"].into_iter()).is_err());
        drop(held);
        assert_eq!(records(&root), 0);
    }

    /// W1-5: a package added for the first time, moved in before the manifest was saved and cut there: the project is as it was
    /// with the unlisted folder kept outside Packages and outside Unity's Temp cleanup.
    #[test]
    fn a_first_addition_cut_before_the_manifest_is_saved_is_taken_out() {
        let temp = tempfile::tempdir().unwrap();
        let (root, p) = (temp.path().join("vcc"), temp.path().join("Project"));
        project(&p);
        let begun = Installing::begin(&root, &p, ["com.a", "com.new"].into_iter()).unwrap();
        let (away, staged) = (p.join("Temp/vrc-get/x/remove"), p.join("Temp/vrc-get/x/install"));
        std::fs::create_dir_all(&away).unwrap();
        std::fs::create_dir_all(staged.join("com.new")).unwrap();
        std::fs::write(staged.join("com.new/package.json"), r#"{"name":"com.new"}"#).unwrap();
        std::fs::rename(p.join("Packages/com.a"), away.join("com.a")).unwrap();
        std::fs::rename(staged.join("com.new"), p.join("Packages/com.new")).unwrap();
        drop(begun);
        let said = recover(&root, &p).unwrap_err().to_string();
        let kept = std::fs::read_dir(&p).unwrap().flatten().map(|e| e.path())
            .find(|p| p.file_name().unwrap().to_string_lossy().starts_with(".orbiters-added-")).unwrap()
            .join("com.new");
        assert!(said.contains(&kept.display().to_string()), "{said}");
        assert_eq!(std::fs::read_to_string(kept.join("package.json")).unwrap(), r#"{"name":"com.new"}"#);
        assert!(!p.join("Packages/com.new").exists(), "the unlisted package is out");
        assert_eq!(std::fs::read_to_string(p.join("Packages/com.a/package.json")).unwrap(), "mine");
        assert_eq!(records(&root), 0);
        // Once saved, the addition went through and stays.
        let begun = Installing::begin(&root, &p, ["com.new"].into_iter()).unwrap();
        std::fs::create_dir_all(p.join("Packages/com.new")).unwrap();
        std::fs::write(p.join(MANIFEST), r#"{"locked":{"com.new":{}}}"#).unwrap();
        drop(begun);
        recover(&root, &p).unwrap();
        assert!(p.join("Packages/com.new").is_dir() && records(&root) == 0);
    }

    fn begun_is_over_leaves_nothing(root: &Path, p: &Path) {
        let before = records(root);
        Installing::begin(root, p, ["com.a"].into_iter()).unwrap().over();
        assert_eq!(records(root), before);
    }
}
