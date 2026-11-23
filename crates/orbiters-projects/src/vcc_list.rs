//! The project list Creator Companion and ALCOM share, one entry at a time: `userProjects` of their `settings.json` and the
//! `projects` collection of `vcc.liteDb`. Listing a project **changes no byte of the project** (vrc-get's own way loads it
//! first, and its loader rewrites a manifest with an old range spelling): what the list says of it is read from its files.
//!
//! Each file is read, has one entry put in or taken out, is checked to be otherwise the same, and is replaced whole at once
//! through its folder's handle ([`ListRoot`]): every other byte of `settings.json` stays, and every other document of the
//! database. A file somebody holds open, one that changed meanwhile, and one of a format not known here are refused. Both
//! steps can be done again: one that was done already changes nothing.
//!
//! An entry is taken out by what folder it is (its identity), never by a spelling that another folder could have.

use crate::owned::{self, FileId};
use crate::unity6::gate::ListRoot;
use crate::unity6::standing::EditorStamp;
use orbiters_i18n::tr;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use vrc_get_litedb::bson::{DateTime, Document, Value};
use vrc_get_litedb::expression::BsonExpression;
use vrc_get_litedb::file_io::{BsonAutoId, LiteDBFile};
use vrc_get_vpm::ProjectType;

const SETTINGS: &str = "settings.json";
const DATABASE: &str = "vcc.liteDb";
const LOG: &str = "vcc-log.liteDb";
const PAGE: usize = 8192;
const MAGIC: &[u8] = b"** This is a LiteDB file **";
const LIST: &str = "userProjects";
const PROJECTS: &str = "projects";

/// What an entry of the list is taken out by.
#[derive(Clone, Debug)]
pub(crate) enum Which {
    /// The folder with this identity, whose last name is this (an entry of another name is not even asked: it may be on a disk
    /// that does not answer).
    Folder(FileId, String),
    /// Exactly this text, when no folder answers to it any more.
    Gone(String),
}

fn last(path: &Path) -> String {
    plain(path).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

impl Which {
    /// What the project at `path` is taken out by: its folder while it is there; else the very text, never a spelling like it.
    pub(crate) fn of(path: &Path) -> Which {
        match std::fs::canonicalize(path).and_then(|p| owned::id_of(&p)) {
            Ok(id) => Which::Folder(id, last(path)),
            Err(_) => Which::Gone(path.to_string_lossy().into_owned()),
        }
    }

    /// The folder `id`, known to be at `path`.
    pub(crate) fn folder(path: &Path, id: FileId) -> Which {
        Which::Folder(id, last(path))
    }

    fn is(&self, entry: &str) -> bool {
        match self {
            Which::Folder(id, name) => last(Path::new(entry)) == *name && std::fs::canonicalize(entry).and_then(|p| owned::id_of(&p)).is_ok_and(|now| now == *id),
            Which::Gone(text) => plain(Path::new(entry)) == plain(Path::new(text)) && !Path::new(entry).exists(),
        }
    }
}

fn unknown(file: &str, why: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{}", tr!("{file} of Creator Companion is not in a form Orbiters knows ({why}): nothing was changed in it. Add the project in Creator Companion yourself.", file = file, why = why))
}

/// What a project's own files say of it for the list.
struct Facts {
    unity: Option<String>,
    revision: Option<String>,
    kind: ProjectType,
}

fn facts(project: &Path) -> Facts {
    let stamp = EditorStamp::read(project);
    let json = |rel: &str| std::fs::read(project.join(rel)).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).unwrap_or_default();
    let (vpm, upm) = (json("Packages/vpm-manifest.json"), json("Packages/manifest.json"));
    let locked = |name: &str| vpm["locked"].get(name).is_some();
    let depends = |name: &str| upm["dependencies"].get(name).is_some();
    let any = |section: &str| vpm[section].as_object().is_some_and(|m| !m.is_empty());
    let has = |rel: &str| project.join(rel).is_file();
    let kind = if locked("com.vrchat.avatars") {
        ProjectType::Avatars
    } else if locked("com.vrchat.worlds") {
        ProjectType::Worlds
    } else if any("dependencies") || any("locked") {
        ProjectType::VpmStarter
    } else if depends("com.vrchat.avatars") {
        ProjectType::UpmAvatars
    } else if depends("com.vrchat.worlds") {
        ProjectType::UpmWorlds
    } else if depends("com.vrchat.base") {
        ProjectType::UpmStarter
    } else if has("Assets/VRCSDK/Plugins/VRCSDK2.dll") {
        ProjectType::LegacySdk2
    } else if has("Assets/VRCSDK/Plugins/VRCSDK3.dll") {
        ProjectType::LegacyWorlds
    } else if has("Assets/VRCSDK/Plugins/VRCSDK3A.dll") {
        ProjectType::LegacyAvatars
    } else {
        ProjectType::Unknown
    };
    Facts { unity: stamp.parsed.then_some(stamp.text), revision: stamp.revision, kind }
}

/// A path as the database keeps it: its separators the system's own, without `.` and `..`.
fn plain(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Puts `project` on the list in `root`. The caller holds the locks of both files ([`crate::vpm`]).
pub(crate) fn add(root: &ListRoot, project: &Path, here: &Which) -> anyhow::Result<()> {
    let text = project.to_str().ok_or_else(|| anyhow::anyhow!("{}", tr("Project path is not Unicode")))?;
    let facts = facts(project);
    let database = edit_database(root, |db| {
        if db.get_all(PROJECTS).any(|d| d["Path"].as_str().is_some_and(|p| here.is(p))) {
            return Ok(false);
        }
        let now = DateTime::now();
        let mut doc = Document::new();
        doc.insert("Path", plain(project).to_str().unwrap_or(text));
        doc.insert("UnityVersion", facts.unity.clone().map_or(Value::Null, Value::from));
        doc.insert("CreatedAt", now);
        doc.insert("LastModified", now);
        doc.insert("Type", facts.kind as i32);
        doc.insert("Favorite", false);
        if let Some(unity) = &facts.unity {
            let mut own = Document::new();
            own.insert("cached_unity_version", unity.as_str());
            own.insert("unity_revision", facts.revision.clone().map_or(Value::Null, Value::from));
            doc.insert("vrc-get", own);
        }
        db.insert(PROJECTS, vec![doc], BsonAutoId::ObjectId).map_err(|e| unknown(DATABASE, e))?;
        Ok(true)
    })?;
    let settings = edit_settings(root, |list| {
        if list.iter().any(|p| here.is(p)) {
            return false;
        }
        list.insert(0, text.to_string());
        true
    })?;
    write(root, [database, settings])
}

/// One file's next contents: its name, what it holds now, what it will.
type Edit = Option<(&'static str, Option<Vec<u8>>, Vec<u8>)>;

/// Both were worked out before either is written: one in a form not known here, or in use, leaves both as they are.
fn write(root: &ListRoot, edits: [Edit; 2]) -> anyhow::Result<()> {
    root.replace(&edits.into_iter().flatten().collect::<Vec<_>>())
}

/// Takes every entry `which` means off the list in `root`.
pub(crate) fn remove(root: &ListRoot, which: &Which) -> anyhow::Result<()> {
    let database = edit_database(root, |db| {
        let ids: Vec<Value> = db.get_all(PROJECTS).filter(|d| d["Path"].as_str().is_some_and(|p| which.is(p))).map(|d| d["_id"].clone()).collect();
        db.delete(PROJECTS, &ids);
        Ok(!ids.is_empty())
    })?;
    let settings = edit_settings(root, |list| {
        let before = list.len();
        list.retain(|p| !which.is(p));
        list.len() != before
    })?;
    write(root, [database, settings])
}

/// Everything of the database but one entry's: what an edit must leave as it was.
fn documents(db: &LiteDBFile) -> Vec<(String, Vec<Document>)> {
    let mut names = db.get_collection_names();
    names.sort();
    names
        .into_iter()
        .map(|n| {
            let all = db.get_all(&n).cloned().collect();
            (n, all)
        })
        .collect()
}

fn edit_database(root: &ListRoot, edit: impl FnOnce(&mut LiteDBFile) -> anyhow::Result<bool>) -> anyhow::Result<Edit> {
    let name = Path::new(DATABASE);
    // Changes the database has not taken in yet are in a log beside it: what is written here would be lost under them.
    if root.read(Path::new(LOG))?.is_some_and(|log| !log.is_empty()) {
        return Err(unknown(DATABASE, tr("it was not closed: open and close Creator Companion once")));
    }
    let before = root.read(name)?;
    let mut db = match &before {
        // The parser takes whole pages and leaves the rest out: what is not all of it is not taken for an empty one.
        Some(bytes) if bytes.is_empty() || bytes.len() % PAGE != 0 || bytes.get(32..32 + MAGIC.len()) != Some(MAGIC) => return Err(unknown(DATABASE, tr("it is not a database"))),
        Some(bytes) => LiteDBFile::parse(bytes).map_err(|e| unknown(DATABASE, e))?,
        None => LiteDBFile::new(),
    };
    // As both programs do on opening it.
    db.drop_indexes_and_update_collation_if_collation_not_supported();
    let path = BsonExpression::create("$.Path").map_err(|e| unknown(DATABASE, e))?;
    db.ensure_index(PROJECTS, "Path", path, false).map_err(|e| unknown(DATABASE, e))?;
    let was = documents(&db);
    if !edit(&mut db)? {
        return Ok(None);
    }
    let after = db.serialize();
    // What was written reads back, with every other document as it was.
    let check = LiteDBFile::parse(&after).map_err(|e| unknown(DATABASE, e))?;
    let now = documents(&check);
    let same = was.iter().zip(&now).all(|((a, old), (b, new))| {
        let kept = |d: &Document| old.contains(d);
        a == b && if a == PROJECTS { new.iter().filter(|d| !kept(d)).count() <= 1 && old.iter().filter(|d| !new.contains(d)).all(|d| d["Path"].as_str().is_some()) } else { old == new }
    });
    anyhow::ensure!(same && was.len() == now.len(), "{}", unknown(DATABASE, tr("it does not read back the same")));
    Ok(Some((DATABASE, before, after)))
}

/// Where a JSON text (an object at its top) has the value of `key`, or `None`; the place right after its `{` besides.
/// Anything that is not one plain object with that key at most once is an error.
fn find(text: &str, key: &str) -> Result<(Option<Range<usize>>, usize), String> {
    let bytes = text.as_bytes();
    let skip = |mut i: usize| {
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
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
        if name == key {
            if found.is_some() {
                return Err("a key twice".into()); // i18n: not shown (chained under a sentence)
            }
            found = Some(value);
        }
        if bytes.get(i) == Some(&b',') {
            i = skip(i + 1);
        }
    }
    if bytes.get(i) != Some(&b'}') {
        return Err("an object that does not end".into()); // i18n: not shown (chained under a sentence)
    }
    Ok((found, open + 1))
}

/// `text` with the list of projects edited by `edit`, every other byte as it was; `None` when nothing changes.
fn edited(text: &str, edit: impl FnOnce(&mut Vec<String>) -> bool) -> Result<Option<String>, String> {
    let whole: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let (span, first) = find(text, LIST)?;
    let mut list: Vec<String> = match &span {
        Some(at) => serde_json::from_str(&text[at.clone()]).map_err(|e| e.to_string())?,
        None => vec![],
    };
    if !edit(&mut list) {
        return Ok(None);
    }
    let pretty = text.contains('\n');
    let array = match (pretty, list.is_empty()) {
        (true, false) => format!("[\n    {}\n  ]", list.iter().map(|p| serde_json::to_string(p).unwrap_or_default()).collect::<Vec<_>>().join(",\n    ")),
        _ => serde_json::to_string(&list).map_err(|e| e.to_string())?,
    };
    let out = match span {
        Some(at) => format!("{}{array}{}", &text[..at.start], &text[at.end..]),
        None => {
            let alone = whole.as_object().is_some_and(serde_json::Map::is_empty);
            let (before, comma) = (if pretty { "\n  " } else { "" }, if alone { "" } else { "," });
            format!("{}{before}\"{LIST}\":{}{array}{comma}{}", &text[..first], if pretty { " " } else { "" }, &text[first..])
        }
    };
    // It reads back as it was, with that list.
    let mut now: serde_json::Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
    let mut was = whole;
    let (Some(a), Some(b)) = (now.as_object_mut(), was.as_object_mut()) else { return Err("not an object".into()) }; // i18n: not shown (chained under a sentence)
    b.remove(LIST);
    if a.remove(LIST) != serde_json::to_value(&list).ok() || a != b {
        return Err("it does not read back the same".into()); // i18n: not shown (chained under a sentence)
    }
    Ok(Some(out))
}

fn edit_settings(root: &ListRoot, edit: impl FnOnce(&mut Vec<String>) -> bool) -> anyhow::Result<Edit> {
    let name = Path::new(SETTINGS);
    let before = root.read(name)?;
    let text = match &before {
        Some(bytes) => std::str::from_utf8(bytes).map_err(|e| unknown(SETTINGS, e))?,
        None => "{}",
    };
    // A byte-order mark stays where it is.
    let (mark, text) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let after = edited(text, edit).map_err(|why| unknown(SETTINGS, why))?.map(|after| format!("{mark}{after}").into_bytes());
    Ok(after.map(|after| (SETTINGS, before, after)))
}

#[cfg(test)]
mod tests;
