//! The projects of the Unity projects page: Orbiters' own list (`unity-gallery.json` in its data folder), with those Creator
//! Companion and ALCOM list, which are only read. Adding, duplicating, creating and removing change Orbiters' list alone: nothing
//! of theirs is written, and what is there stays there.
use orbiters_core::locks::*;
use crate::discovery::{key, same};
use crate::theirs::{self, Part};
use crate::own;
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Orbiters' list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct List {
    /// Added, duplicated or created in Orbiters.
    projects: Vec<Entry>,
    /// Taken off in Orbiters, whoever else lists them: until they are added here again.
    #[serde(default)]
    removed: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    added: u64,
}

impl List {
    fn set(&mut self, path: &Path, add: bool) {
        self.removed.retain(|r| !same(Path::new(r), path));
        if !add {
            self.projects.retain(|p| !same(&p.path, path));
            self.removed.insert(path.to_string_lossy().into_owned());
        } else if !self.projects.iter().any(|p| same(&p.path, path)) {
            self.projects.push(Entry { path: path.into(), added: crate::discovery::now() });
        }
    }

    fn removes(&self, path: &Path) -> bool {
        self.removed.iter().any(|r| same(Path::new(r), path))
    }
}

/// Their lists as last read: what they are while one cannot be.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Seen {
    settings: Vec<PathBuf>,
    database: Vec<PathBuf>,
}

/// Every project of the page.
#[derive(Clone, Debug, Default)]
pub struct Listed {
    pub projects: Vec<PathBuf>,
    /// Those Creator Companion or ALCOM list ([`key`]).
    pub shared: HashSet<String>,
    /// Their project list is on this computer.
    pub companion: bool,
    /// Their list could not be read: what it had when it last could stands in for it.
    pub warning: Option<String>,
}

fn list_file(root: &Path) -> PathBuf {
    own::dir(root).join("unity-gallery.json")
}

fn seen_file(root: &Path) -> PathBuf {
    own::dir(root).join("unity-gallery-shared.json")
}

/// The projects the page listed before Orbiters kept a list: its last scan's.
fn before(root: &Path) -> Vec<PathBuf> {
    let scan: Option<crate::discovery::Scan> = std::fs::read(own::dir(root).join("unity-projects.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    scan.map(|s| s.projects.into_iter().filter(|p| p.registered).map(|p| p.path).collect()).unwrap_or_default()
}

/// Orbiters' list as it is kept. One that is there and cannot be read in either copy is what the page last showed.
fn kept(root: &Path, file: &Path) -> List {
    own::read(file).unwrap_or_else(|| {
        let damaged = file.exists();
        List { projects: if damaged { before(root) } else { vec![] }.into_iter().map(|path| Entry { path, added: 0 }).collect(), removed: BTreeSet::new() }
    })
}

/// Changes that could not be saved (a full disk): they count while the app runs, and are saved at the next chance.
static UNSAVED: Mutex<Vec<(PathBuf, PathBuf, bool)>> = Mutex::new(Vec::new());

fn set(root: &Path, path: &Path, add: bool) -> anyhow::Result<()> {
    let file = list_file(root);
    let mut waiting = UNSAVED.locked();
    let first = kept(root, &file);
    let saved = own::change(&file, || first, |list: &mut List| {
        for (_, p, add) in waiting.iter().filter(|(f, ..)| *f == file) {
            list.set(p, *add);
        }
        list.set(path, add);
    });
    match saved {
        Ok(()) => waiting.retain(|(f, ..)| *f != file),
        Err(e) => {
            waiting.push((file, path.into(), add));
            return Err(e.context(tr("Orbiters' project list could not be saved")));
        }
    }
    Ok(())
}

/// Puts the project at `path` on Orbiters' list. Nothing of it is read but that it is one, and nothing of it is written.
pub fn add(root: &Path, path: &Path) -> anyhow::Result<()> {
    use crate::unity6::authority::{allow, Action};
    allow(&crate::data_dir(), path, Action::List).map_err(|why| anyhow::anyhow!("{why}"))?;
    anyhow::ensure!(crate::discovery::is_project(path), "{}", tr("Choose a Unity project containing ProjectSettings/ProjectVersion.txt."));
    set(root, path, true)?;
    crate::copies::listed(&crate::data_dir(), path);
    Ok(())
}

/// Takes the project at `path` off the page: its files stay, and Creator Companion and ALCOM list it as before.
pub fn remove(root: &Path, path: &Path) -> anyhow::Result<()> {
    use crate::unity6::authority::{allow, Action};
    allow(&crate::data_dir(), path, Action::Unlist).map_err(|why| anyhow::anyhow!("{why}"))?;
    set(root, path, false)
}

/// Orbiters' list and theirs, put together; `root` is Creator Companion's folder.
pub fn listed(root: &Path) -> Listed {
    let file = list_file(root);
    let mut list = kept(root, &file);
    for (_, p, add) in UNSAVED.locked().iter().filter(|(f, ..)| *f == file) {
        list.set(p, *add);
    }
    let theirs = theirs::projects(root);
    let seen: Option<Seen> = own::read(&seen_file(root));
    let first = seen.is_none();
    let mut now = seen.clone().unwrap_or_default();
    if first {
        // Until it is read once, what the page showed before this list was kept.
        now.database = before(root);
    }
    let nothing = theirs.settings == Part::Silent && theirs.database == Part::Silent;
    let mut warning = None;
    let mut take = |part: &Part, kept: &mut Vec<PathBuf>, other: &Part| match part {
        Part::List(l) => *kept = l.clone(),
        // The other place is the list then; with neither, the last one stands.
        Part::Silent if matches!(other, Part::List(_)) => kept.clear(),
        Part::Silent => {}
        Part::Unreadable(why) => {
            warning.get_or_insert_with(|| tr!("Creator Companion's project list cannot be read now ({why}): the projects it had when it last could are shown. Orbiters changes nothing of it.", why = why));
        }
    };
    take(&theirs.settings, &mut now.settings, &theirs.database);
    take(&theirs.database, &mut now.database, &theirs.settings);
    // What was shown before stands in for the database alone: their settings are theirs as soon as they are read.
    if Some(&now) != seen.as_ref() && !(first && (nothing || warning.is_some())) {
        let _ = own::change(&seen_file(root), Seen::default, |s: &mut Seen| *s = now.clone());
    }
    let shared: HashSet<String> = now.settings.iter().chain(&now.database).map(|p| key(p)).collect();
    // Once each, however it is spelled; two folders whose names differ only by case (where a disk keeps those apart) are two.
    let mut had: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut projects = vec![];
    for p in list.projects.iter().map(|e| e.path.clone()).chain(now.database).chain(now.settings).filter(|p| !list.removes(p)) {
        let alike = had.entry(key(&p)).or_default();
        if !alike.iter().any(|other| same(other, &p)) {
            alike.push(p.clone());
            projects.push(p);
        }
    }
    Listed { projects, shared, companion: !nothing, warning }
}

/// Whether the project at `path` is on the page.
pub fn lists(root: &Path, path: &Path) -> bool {
    listed(root).projects.iter().any(|p| same(p, path))
}
