//! Projects the person hid from the Unity projects page (`Orbiters/hidden-projects.json`): this computer's choice for
//! that page only. VCC and ALCOM keep listing them, and nothing in the projects changes.
use crate::{discovery, own};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct File {
    /// [`discovery::key`] of each hidden project's folder.
    hidden: BTreeSet<String>,
}

fn file() -> PathBuf {
    crate::data_dir().join("hidden-projects.json")
}

/// The hidden projects, by [`discovery::key`]. Reads one small file; call on a worker.
pub fn load() -> BTreeSet<String> {
    load_from(&file())
}

fn load_from(file: &Path) -> BTreeSet<String> {
    own::read::<File>(file).unwrap_or_default().hidden
}

/// One project hidden or shown again, asked for now and saved on a worker. The window and the tray service (the bridge) both change
/// the list: what is saved is the one change, never the list as one of them has it (PRJ-36).
pub struct Change {
    key: String,
    hide: bool,
    number: u64,
}

impl Change {
    pub fn new(path: &Path, hide: bool) -> Change {
        static NUMBER: AtomicU64 = AtomicU64::new(1);
        Change { key: discovery::key(path), hide, number: NUMBER.fetch_add(1, Ordering::Relaxed) }
    }

    /// Call on a worker.
    pub fn save(self) -> anyhow::Result<()> {
        self.save_to(&file())
    }

    fn save_to(self, file: &Path) -> anyhow::Result<()> {
        // Workers may finish out of order: an older change of a project must never land over its newer one.
        static LAST: Mutex<Option<HashMap<(PathBuf, String), u64>>> = Mutex::new(None);
        let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let last = last.get_or_insert_default().entry((file.into(), self.key.clone())).or_default();
        if self.number < *last {
            return Ok(());
        }
        own::change(file, File::default, |list: &mut File| if self.hide { list.hidden.insert(self.key.clone()) } else { list.hidden.remove(&self.key) })?;
        *last = self.number;
        Ok(())
    }
}

/// Whether the project in `path` is in `hidden`.
pub fn contains(hidden: &BTreeSet<String>, path: &Path) -> bool {
    !hidden.is_empty() && hidden.contains(&discovery::key(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_projects_are_kept_by_folder_whatever_its_spelling() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("hidden.json");
        assert!(load_from(&file).is_empty());
        let old = Path::new(r"H:\Unity projects\Old avatar");
        let older = Change::new(old, false);
        Change::new(old, true).save_to(&file).unwrap();
        let back = load_from(&file);
        assert!(contains(&back, old));
        #[cfg(windows)]
        assert!(contains(&back, Path::new("h:/unity projects/old avatar/")));
        assert!(!contains(&back, Path::new(r"H:\Unity projects\Old avatar copy")));
        // An older change finishing late leaves the newer one.
        older.save_to(&file).unwrap();
        assert_eq!(load_from(&file), back);
        Change::new(old, false).save_to(&file).unwrap();
        assert!(load_from(&file).is_empty());
        std::fs::write(&file, "not json").unwrap();
        assert_eq!(load_from(&file), back, "damaged: what it was before its last change");
    }

    /// PRJ-36: a project hidden through the bridge while the window is open stays hidden when the window hides another.
    #[test]
    fn what_another_process_hides_is_not_written_over() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("hidden.json");
        Change::new(Path::new("A"), true).save_to(&file).unwrap();
        // Another process.
        own::change(&file, File::default, |l: &mut File| l.hidden.insert(discovery::key(Path::new("B")))).unwrap();
        Change::new(Path::new("C"), true).save_to(&file).unwrap();
        Change::new(Path::new("A"), false).save_to(&file).unwrap();
        assert_eq!(load_from(&file), [Path::new("B"), Path::new("C")].map(discovery::key).into());
    }
}
