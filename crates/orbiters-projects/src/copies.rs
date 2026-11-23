//! Copies of projects being made (Duplicate, a new project), recorded in Orbiters' app data before anything of one is made, until
//! it is on the project list: one that failed, was cut short or could not be listed stays there by its path, with why, to be
//! listed again or dismissed, wherever it is and whatever the list says. Nothing here deletes a copy.

use crate::owned::{self, FileId};
use crate::unity6::gate::{AppRoot, Lock};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One file per copy, and the lock its run holds.
const COPIES: &str = "project-copies";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    /// Files are going in.
    Copying,
    /// Every file is in; it is not on the list.
    Whole,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Copy {
    pub id: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    /// The folder made for it.
    pub folder: Option<FileId>,
    pub started: u64,
    pub state: State,
    pub error: Option<String>,
}

/// Where a copy stands now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Being made by this window or another one.
    Running,
    /// Cut short: never a project.
    Incomplete,
    /// Whole, and not on the list: it can be listed.
    Unlisted,
    /// Not at its place (a disk not connected, moved, deleted), or another folder is there now.
    Missing,
}

fn file(id: &str) -> PathBuf {
    Path::new(COPIES).join(format!("{id}.json"))
}

fn lock(id: &str) -> PathBuf {
    Path::new(COPIES).join(format!("{id}.lock"))
}

fn is_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn read(app: &AppRoot, id: &str) -> Option<Copy> {
    serde_json::from_slice::<Copy>(&app.read(&file(id)).ok()?).ok().filter(|c| c.id == id)
}

/// Every copy recorded in `dir` (app data). What cannot be read stays in its file.
fn all(dir: &Path) -> Vec<Copy> {
    let Ok(app) = AppRoot::existing(dir) else { return vec![] };
    let names = app.files(Path::new(COPIES)).into_iter().filter_map(|n| n.into_string().ok());
    let mut out: Vec<Copy> = names.filter_map(|n| read(&app, n.strip_suffix(".json").filter(|id| is_id(id))?)).collect();
    out.sort_by_key(|c| (c.started, c.id.clone()));
    out
}

/// A copy being made by this run.
pub(crate) struct Held {
    dir: PathBuf,
    copy: Copy,
    lock: Option<Lock>,
}

impl Held {
    /// Recorded before anything of it is made.
    pub(crate) fn begin(dir: &Path, source: &Path, destination: &Path) -> anyhow::Result<Self> {
        let app = AppRoot::open(dir)?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let copy = Copy { id: id.clone(), source: source.into(), destination: destination.into(), folder: None, started: crate::discovery::now(), state: State::Copying, error: None };
        let held = Held { dir: dir.into(), copy, lock: Some(app.lock(&lock(&id))?) };
        held.save()?;
        Ok(held)
    }

    fn save(&self) -> anyhow::Result<()> {
        AppRoot::existing(&self.dir)?.replace_json(&file(&self.copy.id), &self.copy)
    }

    pub(crate) fn made(&mut self, folder: FileId) -> anyhow::Result<()> {
        self.copy.folder = Some(folder);
        self.save()
    }

    pub(crate) fn whole(&mut self) -> anyhow::Result<()> {
        self.copy.state = State::Whole;
        self.save()
    }

    /// Kept, with why.
    pub(crate) fn failed(mut self, why: &anyhow::Error) {
        self.copy.error = Some(format!("{why:#}"));
        let _ = self.save();
    }

    /// It is on the list, or nothing of it is left.
    pub(crate) fn done(mut self) {
        remove(&self.dir, &self.copy.id, self.lock.take());
    }
}

fn remove(dir: &Path, id: &str, held: Option<Lock>) {
    let Ok(app) = AppRoot::existing(dir) else { return };
    let _ = app.remove_file(&file(id));
    drop(held);
    let _ = app.remove_file(&lock(id));
}

/// The copies to show: those recorded in `dir`, and where each stands. One of which nothing was made goes.
pub fn shown(dir: &Path) -> Vec<(Copy, Status)> {
    let mut out = vec![];
    for c in all(dir) {
        let held = AppRoot::existing(dir).and_then(|app| app.try_lock(&lock(&c.id)));
        let Ok(Some(held)) = held else {
            out.push((c, Status::Running));
            continue;
        };
        let there = c.folder.is_some_and(|id| owned::is_same(&c.destination, id));
        let status = match (c.folder, c.state) {
            (None, _) if std::fs::symlink_metadata(&c.destination).is_err() => {
                remove(dir, &c.id, Some(held));
                continue;
            }
            _ if !there => Status::Missing,
            (_, State::Whole) if crate::discovery::is_project(&c.destination) => Status::Unlisted,
            _ => Status::Incomplete,
        };
        out.push((c, status));
    }
    out
}

/// The whole copies that are not on the list, wherever they are: the gallery has them.
pub(crate) fn folders(dir: &Path) -> Vec<PathBuf> {
    shown(dir).into_iter().filter(|(_, s)| *s == Status::Unlisted).map(|(c, _)| c.destination).collect()
}

/// The copy `id`, when nobody works on it.
fn take(dir: &Path, id: &str) -> anyhow::Result<(Copy, Lock)> {
    anyhow::ensure!(is_id(id), "{}", tr("This copy is not recorded any more."));
    let app = AppRoot::existing(dir)?;
    let held = app.try_lock(&lock(id))?;
    let held = held.ok_or_else(|| anyhow::anyhow!("{}", tr("This copy is being made, in this window or another one.")))?;
    let copy = read(&app, id).ok_or_else(|| anyhow::anyhow!("{}", tr("This copy is not recorded any more.")))?;
    Ok((copy, held))
}

/// Puts a whole copy on the list VCC and ALCOM share (`root`); its record goes once it is there.
pub fn list(dir: &Path, root: &Path, id: &str) -> anyhow::Result<PathBuf> {
    let (mut copy, held) = take(dir, id)?;
    let there = copy.folder.is_some_and(|folder| owned::is_same(&copy.destination, folder));
    anyhow::ensure!(there && copy.state == State::Whole && crate::discovery::is_project(&copy.destination), "{}", tr!("The folder at {path} is not the whole copy Orbiters made.", path = copy.destination.display()));
    if let Err(e) = crate::runtime()?.block_on(crate::vpm::register(root, &copy.destination, true)) {
        copy.error = Some(format!("{e:#}"));
        let _ = AppRoot::existing(dir)?.replace_json(&file(id), &copy);
        return Err(e);
    }
    remove(dir, id, Some(held));
    Ok(copy.destination)
}

/// Takes a copy off what Orbiters shows; its folder stays. Answers where that is.
pub fn dismiss(dir: &Path, id: &str) -> anyhow::Result<PathBuf> {
    let (copy, held) = take(dir, id)?;
    remove(dir, id, Some(held));
    Ok(copy.destination)
}

/// The project at `path` was put on the list: a record of it as a copy that is not goes.
pub(crate) fn listed(dir: &Path, path: &Path) {
    let Ok(now) = owned::id_of(path) else { return };
    for c in all(dir).into_iter().filter(|c| c.folder == Some(now) && c.state == State::Whole) {
        if let Ok((_, held)) = take(dir, &c.id) {
            remove(dir, &c.id, Some(held));
        }
    }
}

#[cfg(test)]
mod tests;
