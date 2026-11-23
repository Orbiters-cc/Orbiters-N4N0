//! One entry of the project list Creator Companion and ALCOM share, put in or taken out so that it costs nobody else anything.
//!
//! The list is in two files: `vcc.liteDb` (the projects, with their favourites and dates) and, for older programs, `userProjects`
//! in `settings.json`. A reader that finds `userProjects` takes it for the whole list and removes every other project from the
//! database, so a list that is not there is **never made** when the database has projects: only they are the list then.
//!
//! `settings.json` is edited in its text (every other byte stays) and held alone from being read to being written, so that no
//! save of another program is lost. What is not in a form known here, or in use, refuses before either file is written.

use orbiters_i18n::tr;
use std::fs::File;
#[cfg(windows)]
use std::io::Seek;
use std::io::{Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};

const SETTINGS: &str = "settings.json";
const DATABASE: &str = "vcc.liteDb";
/// The changes LiteDB has not put in the database yet.
const LOG: &str = "vcc-log.liteDb";
/// What ALCOM recovers the settings from when their file is missing.
const ALCOMS_BACKUP: &str = "vrc-get/vcc-settings-backup.json";
/// The settings as they were before the last change made here.
const BACKUP: &str = "settings.json.before-orbiters";
const LIST: &str = "userProjects";
const PAGE: usize = 8192;
const MAGIC: &[u8] = b"** This is a LiteDB file **";

fn busy(file: &str) -> anyhow::Error {
    anyhow::anyhow!("{}", tr!("{file} of Creator Companion or ALCOM is in use: close them, then try again. Nothing was changed.", file = file))
}

fn unknown(file: &str, why: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{}", tr!("{file} of Creator Companion is not in a form Orbiters knows ({why}). Nothing was changed: add or remove the project in Creator Companion or ALCOM.", file = file, why = why))
}

/// `name` in `root`, which nobody else has open (Windows; elsewhere nothing keeps others out); `None` when it is not there.
fn alone(root: &Path, name: &str) -> anyhow::Result<Option<File>> {
    let mut options = File::options();
    options.read(true).write(true);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut options, 0);
    // Somebody may be reading it at this instant.
    for tries in 0.. {
        match options.open(root.join(name)) {
            Ok(file) => return Ok(Some(file)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) if tries < 20 => std::thread::sleep(std::time::Duration::from_millis(25)),
            Err(_) => return Err(busy(name)),
        }
    }
    unreachable!()
}

/// The database must be whole and nobody's now: its reader takes whole pages and leaves the rest out, so a file cut short would
/// be taken for a smaller database and written over.
pub(crate) fn database_is_whole(root: &Path) -> anyhow::Result<()> {
    if std::fs::metadata(root.join(LOG)).is_ok_and(|m| m.len() > 0) {
        return Err(unknown(DATABASE, tr("it was not closed: open and close Creator Companion once")));
    }
    let Some(mut file) = alone(root, DATABASE)? else { return Ok(()) };
    let mut head = vec![];
    (&mut file).take(64).read_to_end(&mut head)?;
    let len = file.metadata()?.len() as usize;
    if len == 0 || len % PAGE != 0 || head.get(32..32 + MAGIC.len()) != Some(MAGIC) {
        return Err(unknown(DATABASE, tr("it is not a whole database")));
    }
    Ok(())
}

/// The settings with the one entry changed, held until this is dropped.
pub(crate) struct Settings {
    root: PathBuf,
    #[cfg_attr(not(windows), allow(dead_code))]
    held: Option<File>,
    before: Option<Vec<u8>>,
    /// `None`: they need no change, or the database alone is the list.
    after: Option<Vec<u8>>,
}

impl Settings {
    /// Reads them and works the change out: `project` added to the list, or every entry `is` means taken out of it.
    /// `others`: whether the database has projects already.
    pub(crate) fn plan(root: &Path, project: &str, add: bool, others: bool, is: impl Fn(&str) -> bool) -> anyhow::Result<Self> {
        let mut held = alone(root, SETTINGS)?;
        let Some(file) = &mut held else {
            // ALCOM and VCC make theirs, from a backup or anew; a list put there now would be the whole list to them.
            let alone = add && !others && !root.join(ALCOMS_BACKUP).exists();
            let after = alone.then(|| serde_json::to_vec_pretty(&serde_json::json!({ LIST: [project] }))).transpose()?;
            return Ok(Self { root: root.into(), held, before: None, after });
        };
        let mut before = vec![];
        file.read_to_end(&mut before)?;
        let text = std::str::from_utf8(&before).map_err(|e| unknown(SETTINGS, e))?;
        let (mark, text) = text.strip_prefix('\u{feff}').map_or(("", text), |rest| ("\u{feff}", rest));
        // Left blank by a stop of the computer: both programs recover it, with the database as the list.
        let after = if text.bytes().all(|b| matches!(b, b' ' | 0)) {
            None
        } else {
            let edit = |list: &mut Vec<String>| {
                let had = list.len();
                if !add {
                    list.retain(|p| !is(p));
                } else if !list.iter().any(|p| is(p)) {
                    list.insert(0, project.to_string());
                }
                list.len() != had
            };
            edited(text, edit).map_err(|why| unknown(SETTINGS, why))?.map(|t| format!("{mark}{t}").into_bytes())
        };
        Ok(Self { root: root.into(), held, before: Some(before), after })
    }

    /// Writes the change; what was there is kept beside them first.
    pub(crate) fn write(&mut self) -> anyhow::Result<()> {
        let Some(after) = &self.after else { return Ok(()) };
        let Some(before) = &self.before else {
            let mut file = File::options().write(true).create_new(true).open(self.root.join(SETTINGS)).map_err(|_| busy(SETTINGS))?;
            file.write_all(after)?;
            return Ok(file.sync_all()?);
        };
        publish(&self.root, BACKUP, before)?;
        self.put(after.clone())
    }

    /// Puts them back as they were.
    pub(crate) fn put_back(&mut self) -> anyhow::Result<()> {
        match (&self.after, self.before.clone()) {
            (None, _) => Ok(()),
            (Some(_), Some(before)) => self.put(before),
            (Some(_), None) => Ok(std::fs::remove_file(self.root.join(SETTINGS))?),
        }
    }

    /// In place, through the handle nobody can come between: a file put at its name by another program would be lost.
    #[cfg(windows)]
    fn put(&mut self, bytes: Vec<u8>) -> anyhow::Result<()> {
        let file = self.held.as_mut().ok_or_else(|| busy(SETTINGS))?;
        file.seek(std::io::SeekFrom::Start(0))?;
        file.write_all(&bytes)?;
        file.set_len(bytes.len() as u64)?;
        Ok(file.sync_all()?)
    }

    /// Nothing keeps others out here: it must be what was read, right before it is replaced whole.
    #[cfg(not(windows))]
    fn put(&mut self, bytes: Vec<u8>) -> anyhow::Result<()> {
        let now = std::fs::read(self.root.join(SETTINGS))?;
        anyhow::ensure!(Some(&now) == self.before.as_ref() || Some(&now) == self.after.as_ref(), "{}", busy(SETTINGS));
        publish(&self.root, SETTINGS, &bytes)
    }
}

/// `bytes` as the file `name`, whole at once.
fn publish(root: &Path, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let temp = root.join(format!("{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let done = (|| -> anyhow::Result<()> {
        let mut file = File::options().write(true).create_new(true).open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        #[cfg(unix)]
        if let Ok(m) = std::fs::metadata(root.join(name)) {
            std::fs::set_permissions(&temp, m.permissions())?;
        }
        Ok(std::fs::rename(&temp, root.join(name))?)
    })();
    if done.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    done
}

/// Where a JSON text (an object at its top) has the value of [`LIST`]; `None` when it has none. Anything that is not one plain
/// object with it at most once is an error.
fn find(text: &str) -> Result<Option<Range<usize>>, String> {
    let bytes = text.as_bytes();
    let skip = |mut i: usize| {
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        i
    };
    let string = |from: usize| -> Result<usize, String> {
        let mut i = from + 1;
        loop {
            match bytes.get(i) {
                None => return Err("a text that does not end".into()), // i18n: not shown (chained under a sentence)
                Some(b'\\') => i += 2,
                Some(b'"') => return Ok(i + 1),
                Some(_) => i += 1,
            }
        }
    };
    let open = skip(0);
    if bytes.get(open) != Some(&b'{') {
        return Err("not an object".into()); // i18n: not shown (chained under a sentence)
    }
    let mut found = None;
    let mut i = skip(open + 1);
    while bytes.get(i) == Some(&b'"') {
        let end = string(i)?;
        let name: String = serde_json::from_str(&text[i..end]).map_err(|e| e.to_string())?;
        i = skip(end);
        if bytes.get(i) != Some(&b':') {
            return Err("no value".into()); // i18n: not shown (chained under a sentence)
        }
        i = skip(i + 1);
        let start = i;
        let mut depth = 0usize;
        loop {
            match bytes.get(i) {
                None => return Err("an object that does not end".into()), // i18n: not shown (chained under a sentence)
                Some(b'"') => {
                    i = string(i)?;
                    continue;
                }
                Some(b'{' | b'[') => depth += 1,
                Some(b'}' | b']') if depth > 0 => depth -= 1,
                Some(b',' | b'}') if depth == 0 => break,
                Some(_) => {}
            }
            i += 1;
        }
        let value = start..start + text[start..i].trim_end().len();
        if name == LIST {
            if found.is_some() {
                return Err("a list twice".into()); // i18n: not shown (chained under a sentence)
            }
            found = Some(value);
        }
        if bytes.get(i) == Some(&b',') {
            i = skip(i + 1);
        }
    }
    Ok(found)
}

/// `text` with its list edited, every other byte as it was; `None` when nothing changes, or it has no list.
fn edited(text: &str, edit: impl FnOnce(&mut Vec<String>) -> bool) -> Result<Option<String>, String> {
    let mut was: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let Some(at) = find(text)? else { return Ok(None) };
    if &text[at.clone()] == "null" {
        return Ok(None);
    }
    let mut list: Vec<String> = serde_json::from_str(&text[at.clone()]).map_err(|e| e.to_string())?;
    if !edit(&mut list) {
        return Ok(None);
    }
    let array = match (text.contains('\n'), list.is_empty()) {
        (true, false) => format!("[\n    {}\n  ]", list.iter().map(|p| serde_json::to_string(p).unwrap_or_default()).collect::<Vec<_>>().join(",\n    ")),
        _ => serde_json::to_string(&list).map_err(|e| e.to_string())?,
    };
    let out = format!("{}{array}{}", &text[..at.start], &text[at.end..]);
    // It reads back as it was, with that list.
    let mut now: serde_json::Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
    let (Some(a), Some(b)) = (now.as_object_mut(), was.as_object_mut()) else { return Err("not an object".into()) }; // i18n: not shown (chained under a sentence)
    b.remove(LIST);
    if a.remove(LIST) != serde_json::to_value(&list).ok() || a != b {
        return Err("it does not read back the same".into()); // i18n: not shown (chained under a sentence)
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(root: &Path, add: bool) -> anyhow::Result<Settings> {
        Settings::plan(root, "Z:\\New", add, true, |p| p == "Z:\\New")
    }

    /// From being read to being written, the settings are nobody else's: a save put at their name meanwhile is refused to the one
    /// making it, never lost (Windows); elsewhere it is seen, and refuses this one.
    #[test]
    fn no_save_is_lost_between_reading_and_writing() {
        let temp = tempfile::tempdir().unwrap();
        let (root, at) = (temp.path(), temp.path().join(SETTINGS));
        std::fs::write(&at, r#"{"userProjects":["Z:\\Old"],"a":1}"#).unwrap();
        let mut mine = plan(root, true).unwrap();
        let theirs = root.join("theirs.tmp");
        std::fs::write(&theirs, r#"{"userProjects":["Z:\\Old"],"a":2}"#).unwrap();
        let replaced = std::fs::rename(&theirs, &at).is_ok();
        let written = std::fs::write(&at, r#"{"userProjects":[],"a":3}"#).is_ok();
        assert_eq!(replaced || written, !cfg!(windows));
        let done = mine.write();
        drop(mine);
        let now = std::fs::read_to_string(&at).unwrap();
        if cfg!(windows) {
            done.unwrap();
            assert_eq!(now, r#"{"userProjects":["Z:\\New","Z:\\Old"],"a":1}"#);
        } else {
            assert!(done.is_err() && now.contains(r#""a":3"#), "{now}");
        }
    }

    /// The database could not be saved: the settings are as they were, byte for byte; and what was there is kept beside them.
    #[test]
    fn a_change_that_is_not_kept_is_put_back() {
        let temp = tempfile::tempdir().unwrap();
        let (root, at) = (temp.path(), temp.path().join(SETTINGS));
        for before in ["\u{feff}{ \"userProjects\" : [\"Z:\\\\Old\"] }\r\n", "{\"userProjects\":[]}"] {
            std::fs::write(&at, before).unwrap();
            let mut s = plan(root, true).unwrap();
            s.write().unwrap();
            s.put_back().unwrap();
            drop(s);
            assert_eq!(std::fs::read_to_string(&at).unwrap(), before);
            assert_eq!(std::fs::read_to_string(root.join(BACKUP)).unwrap(), before);
        }
        // Made by this: gone again. One made by another program meanwhile: not written over.
        std::fs::remove_file(&at).unwrap();
        let mut s = Settings::plan(root, "Z:\\New", true, false, |_| false).unwrap();
        s.write().unwrap();
        s.put_back().unwrap();
        assert!(!at.exists());
        let mut s = Settings::plan(root, "Z:\\New", true, false, |_| false).unwrap();
        std::fs::write(&at, "theirs").unwrap();
        assert!(s.write().is_err());
        assert_eq!(std::fs::read_to_string(&at).unwrap(), "theirs");
    }

    /// A list is made only where there is nothing at all: else the database is the list.
    #[test]
    fn a_list_is_never_made_beside_a_database() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let makes = |others: bool| Settings::plan(root, "Z:\\New", true, others, |_| false).unwrap().after.is_some();
        assert!(makes(false) && !makes(true));
        std::fs::create_dir_all(root.join("vrc-get")).unwrap();
        std::fs::write(root.join(ALCOMS_BACKUP), "{}").unwrap();
        assert!(!makes(false), "ALCOM recovers its settings from there");
        for text in ["{}", r#"{"userProjects":null}"#, "", " ", "\0\0\0"] {
            std::fs::write(root.join(SETTINGS), text).unwrap();
            assert!(!makes(false) && !makes(true), "{text:?}");
        }
    }
}
