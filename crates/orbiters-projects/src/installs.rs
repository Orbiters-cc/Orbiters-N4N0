//! A package change that ends halfway (PRJ-23). The library that makes it (vrc-get) moves the folders it replaces to the project's
//! `Temp/vrc-get/<id>/remove`, moves the new ones in and saves the manifest last; on an error it moves everything back, but a process
//! that ends in between (a kill, a power cut) leaves the project without its packages, and Unity empties `Temp`. Each change is
//! therefore recorded, outside the project, until it is over. While the manifest is as it was, what the change had moved away is put
//! back: before the next change, and before Unity opens the project. Only where nothing is in its place: what is there stays, and
//! what the project had is kept beside, named. Costs one small file a change, no copy of a package.
use crate::owned::{id_of, is_same, FileId};
use crate::{own, repos};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    project: PathBuf,
    project_id: FileId,
    /// `Packages/vpm-manifest.json` as it was: saved last, it says whether the change went through.
    manifest: String,
    /// The packages it removes or replaces that were there.
    packages: Vec<String>,
}

const MANIFEST: &str = "Packages/vpm-manifest.json";

fn dir(root: &Path) -> PathBuf {
    repos::dir(root).join("package-changes")
}

/// A change being made. Dropped before it is [`Installing::over`], its record stays.
pub(crate) struct Installing {
    file: PathBuf,
    held: Option<std::fs::File>,
}

impl Installing {
    /// Records that `packages` of the project at `path` are about to be removed or replaced. Nothing is changed when it cannot be.
    pub(crate) fn begin<'a>(root: &Path, path: &Path, packages: impl Iterator<Item = &'a str>) -> anyhow::Result<Installing> {
        let file = dir(root).join(format!("{}.json", uuid::Uuid::new_v4().simple()));
        let begun = (|| -> anyhow::Result<Installing> {
            let held = own::lock(&file)?;
            let record = Record {
                project: path.into(),
                project_id: id_of(path)?,
                manifest: std::fs::read_to_string(path.join(MANIFEST)).unwrap_or_default(),
                packages: packages.filter(|p| path.join("Packages").join(p).exists()).map(ToOwned::to_owned).collect(),
            };
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

/// Whether the change recorded in `file` is being made now, by this process or another.
fn runs(file: &Path) -> bool {
    std::fs::File::open(file.with_extension("lock")).is_ok_and(|f| matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)))
}

/// Puts back what changes of the project at `path` that ended halfway had moved away. An error says what could not be, and where what
/// the project had is; it is said once.
pub(crate) fn recover(root: &Path, path: &Path) -> anyhow::Result<()> {
    let mut problems = vec![];
    for file in std::fs::read_dir(dir(root)).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "json")) {
        let Some(record) = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Record>(&b).ok()) else { continue };
        if !crate::discovery::same(&record.project, path) || !is_same(path, record.project_id) || runs(&file) {
            continue;
        }
        // Saved: the change went through, and what it moved away is what it replaced.
        if std::fs::read_to_string(path.join(MANIFEST)).unwrap_or_default() == record.manifest {
            match put_back(path, &record) {
                Ok(said) => problems.extend(said),
                // Still in use, say: tried again the next time.
                Err(e) => {
                    problems.push(e.to_string());
                    continue;
                }
            }
        }
        forget(&file);
    }
    anyhow::ensure!(problems.is_empty(), "{}", tr!("A package change of this project ended halfway (Orbiters closed during it). {problems}", problems = problems.join(" ")));
    Ok(())
}

/// What `record`'s change had moved away, back where nothing is in its place; else set aside, and said.
fn put_back(path: &Path, record: &Record) -> std::io::Result<Vec<String>> {
    let mut said = vec![];
    for name in record.packages.iter().filter(|n| !n.contains(['/', '\\']) && *n != "..") {
        let at = path.join("Packages").join(name);
        let Some(had) = moved(path, name) else { continue };
        let failed = |e: std::io::Error, to: &Path| std::io::Error::new(e.kind(), tr!("{name} could not be put back from {from} to {to}: {why}", name = name, from = had.display(), to = to.display(), why = e));
        if std::fs::symlink_metadata(&at).is_err() {
            crate::disk::rename(&had, &at).map_err(|e| failed(e, &at))?;
        } else {
            let beside = path.join(format!(".orbiters-before-{}", uuid::Uuid::new_v4().simple()));
            crate::disk::create_dir(&beside).and_then(|()| crate::disk::rename(&had, beside.join(name))).map_err(|e| failed(e, &beside))?;
            said.push(tr!("{name} may be the new one: what the project had is kept in {path}. Restore installs the version the project's list has.", name = name, path = beside.display()));
        }
    }
    Ok(said)
}

/// The folder vrc-get moved the package `name` of the project at `path` to.
fn moved(path: &Path, name: &str) -> Option<PathBuf> {
    let all = std::fs::read_dir(path.join("Temp/vrc-get")).ok()?.flatten().map(|e| e.path().join("remove").join(name));
    all.filter(|p| std::fs::symlink_metadata(p).is_ok()).max_by_key(|p| crate::discovery::fingerprint(p))
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
    fn begun_is_over_leaves_nothing(root: &Path, p: &Path) {
        let before = records(root);
        Installing::begin(root, p, ["com.a"].into_iter()).unwrap().over();
        assert_eq!(records(root), before);
    }
}
