//! What Creator Companion and ALCOM keep, read and never written: their settings, their project list (in two places) and the Unity
//! editors they know. Files are opened for reading alone, shared with every other use, with no lock of theirs taken; one that is
//! being written, damaged or unknown is an answer of its own, never repaired.
use crate::own::steady;
use serde_json::Value;
use std::path::{Path, PathBuf};
use vrc_get_litedb::file_io::LiteDBFile;

/// What one place says the projects are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    /// Not there, or with nothing in it: it says nothing.
    Silent,
    /// Read: exactly these.
    List(Vec<PathBuf>),
    /// There, and not readable now.
    Unreadable(String),
}

/// The projects as settings.json and the database have them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Lists {
    pub settings: Part,
    pub database: Part,
}

/// A JSON object of theirs: `None` when the file is not there or blank (a computer stopped while it was written).
pub(crate) fn json(file: &Path) -> anyhow::Result<Option<Value>> {
    let Some(bytes) = steady(file)? else { return Ok(None) };
    let text = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    if text.iter().all(|b| *b == 0 || b.is_ascii_whitespace()) {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(text)?;
    anyhow::ensure!(value.is_object(), "not an object"); // i18n: not shown (chained under a sentence)
    Ok(Some(value))
}

/// Their settings; where the file is not there or blank, the copy ALCOM keeps.
pub(crate) fn settings(root: &Path) -> anyhow::Result<Option<Value>> {
    Ok(match json(&root.join("settings.json"))? {
        Some(v) => Some(v),
        None => json(&root.join("vrc-get/vcc-settings-backup.json"))?,
    })
}

/// What is read of their database.
struct Database {
    collections: Vec<String>,
    projects: Vec<PathBuf>,
    editors: Vec<(String, PathBuf)>,
    bytes: usize,
}

/// The database as its last finished write left it, in memory alone.
fn database(root: &Path) -> anyhow::Result<Option<Database>> {
    let Some(bytes) = steady(&root.join("vcc.liteDb"))? else { return Ok(None) };
    if bytes.is_empty() {
        return Ok(None);
    }
    // A damaged one is not this program's end.
    let read = std::panic::catch_unwind(|| {
        let mut db = LiteDBFile::parse(&bytes)?;
        // As both programs do once it is read; here it stays in memory.
        db.drop_indexes_and_update_collation_if_collation_not_supported();
        let path = |d: &vrc_get_litedb::bson::Document| d["Path"].as_str().map(PathBuf::from); // i18n: not shown (a field of their database)
        let read: anyhow::Result<Database> = Ok(Database {
            collections: db.get_collection_names(),
            projects: db.get_all("projects").filter_map(path).collect(),
            editors: db.get_all("unityVersions").filter_map(|d| Some((d["Version"].as_str()?.to_string(), path(d)?))).collect(), // i18n: not shown (a field of their database)
            bytes: bytes.len(),
        });
        read
    });
    Ok(Some(read.map_err(|_| anyhow::anyhow!("damaged"))??)) // i18n: not shown (chained under a sentence)
}

/// The most a database nothing was put in yet takes.
const NEW: usize = 2 * 8192;

pub(crate) fn projects(root: &Path) -> Lists {
    let settings = match json(&root.join("settings.json")) {
        Ok(None) => Part::Silent,
        // Without the key the database is the list.
        Ok(Some(v)) => match &v["userProjects"] {
            Value::Null => Part::Silent,
            Value::Array(list) if list.iter().all(Value::is_string) => Part::List(list.iter().filter_map(|p| p.as_str().map(PathBuf::from)).collect()),
            _ => Part::Unreadable("userProjects".into()), // i18n: not shown (a key of their file)
        },
        Err(e) => Part::Unreadable(e.to_string()),
    };
    let database = match database(root) {
        Ok(None) => Part::Silent,
        // A new one has no collection yet; a larger one without any has lost its directory.
        Ok(Some(db)) if db.collections.is_empty() && db.bytes > NEW => Part::Unreadable("no collections".into()), // i18n: not shown (chained under a sentence)
        Ok(Some(db)) if !db.collections.iter().any(|n| n == "projects") => Part::Silent,
        Ok(Some(db)) => Part::List(db.projects),
        Err(e) => Part::Unreadable(e.to_string()),
    };
    Lists { settings, database }
}

/// The Unity editors their database records: versions and paths.
pub(crate) fn editors(root: &Path) -> Vec<(String, PathBuf)> {
    database(root).ok().flatten().map(|db| db.editors).unwrap_or_default()
}

/// The repositories ALCOM was told to leave out.
pub(crate) fn hidden_repositories(root: &Path) -> Vec<String> {
    let Ok(Some(v)) = json(&root.join("vrc-get/gui-config.json")) else { return vec![] };
    v["guiHiddenRepositories"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(ToOwned::to_owned)).collect()).unwrap_or_default()
}
