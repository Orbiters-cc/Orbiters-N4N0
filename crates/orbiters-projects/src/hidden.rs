//! Projects the person hid from the Unity projects page (`Orbiters/hidden-projects.json`): this computer's choice for
//! that page only. VCC and ALCOM keep listing them, and nothing in the projects changes.
use crate::discovery;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Default, Serialize, Deserialize)]
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
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice::<File>(&b).ok()).map(|f| f.hidden).unwrap_or_default()
}

/// The newest list saved so far, by its `generation`: saves started one after the other on workers may finish out of
/// order, and an older list must never land over a newer one.
static SAVED: Mutex<u64> = Mutex::new(0);

/// Keeps `hidden` (the whole list, numbered by `generation`, which grows with each change). Call on a worker.
pub fn save(hidden: &BTreeSet<String>, generation: u64) -> anyhow::Result<()> {
    save_to(&file(), hidden, generation)
}

fn save_to(file: &Path, hidden: &BTreeSet<String>, generation: u64) -> anyhow::Result<()> {
    let mut saved = SAVED.lock().unwrap_or_else(|e| e.into_inner());
    if generation < *saved {
        return Ok(());
    }
    crate::write_json(file, &File { hidden: hidden.clone() })?;
    *saved = generation;
    Ok(())
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
        let mut hidden = BTreeSet::new();
        hidden.insert(discovery::key(Path::new(r"H:\Unity projects\Old avatar")));
        save_to(&file, &hidden, 1_000_001).unwrap();
        let back = load_from(&file);
        assert_eq!(back, hidden);
        assert!(contains(&back, Path::new("h:/unity projects/old avatar/")));
        assert!(!contains(&back, Path::new(r"H:\Unity projects\Old avatar copy")));
        // An older save finishing late leaves the newer list.
        save_to(&file, &BTreeSet::new(), 1_000_000).unwrap();
        assert_eq!(load_from(&file), hidden);
        std::fs::write(&file, "not json").unwrap();
        assert!(load_from(&file).is_empty());
    }
}
