//! Filling a copy of a project (a trial, a duplicate, a new project) through the proof of its folder
//! ([`crate::unity6::gate`]), and taking back exactly what was put in it.

use crate::owned::FileId;
use crate::templates::{Marker, Progress, SharedProgress, INCOMPLETE};
use crate::unity6::gate::{HeldDir, NewFolder, WriteRoot};
use crate::unity6::payload::{Item, Payload};
use orbiters_i18n::{mark, tr};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The moments between the steps of making a copy. A test stops there, fails there, or changes something there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// A trial is recorded.
    Recorded,
    /// A duplicate's missing packages were restored, beside it.
    Restored,
    /// Its folder is made.
    Made,
    /// Its folders are made, its files are not.
    Folders,
    Copied,
    Compared,
    Verified,
    MarkerGone,
    Listed,
    /// Listed, and looked at again.
    Checked,
}

#[cfg(test)]
pub(crate) type Between = Box<dyn Fn(Step) -> anyhow::Result<()>>;
#[cfg(test)]
thread_local! {
    pub(crate) static BETWEEN: std::cell::RefCell<Option<Between>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn between(step: Step) -> anyhow::Result<()> {
    #[cfg(test)]
    return BETWEEN.with(|b| b.borrow().as_ref().map_or(Ok(()), |f| f(step)));
    #[cfg(not(test))]
    {
        let _ = step;
        Ok(())
    }
}

/// What one run made: each folder held since it was made, each thing by its identity.
#[derive(Default)]
pub(crate) struct Made {
    items: Vec<(PathBuf, FileId, Option<HeldDir>)>,
}

impl Made {
    pub(crate) fn marker(&self) -> Option<FileId> {
        self.items.first().map(|(_, id, _)| *id)
    }
}

/// Puts `payload` in the folder, the marker first: a copy that is not complete is never taken for a project.
pub(crate) fn fill(payload: &Payload, root: &WriteRoot, marker: &Marker, made: &mut Made, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<()> {
    let mut file = root.make_file(Path::new(INCOMPLETE))?;
    made.items.push((PathBuf::from(INCOMPLETE), file.id, None));
    file.write_all(&serde_json::to_vec(marker)?)?;
    drop(file);
    let total = payload.files();
    progress.lock().unwrap().total = total;
    let stop = || anyhow::anyhow!("{}", tr("Stopped."));
    let wrote = |at: &Path, e: std::io::Error| anyhow::anyhow!("{}", tr!("Could not copy {path}: {error}", path = at.display(), error = e));
    for at in payload.items.iter().filter(|(_, i)| matches!(i, Item::Dir)).map(|(at, _)| at) {
        if cancel.load(Ordering::Relaxed) {
            return Err(stop());
        }
        let (id, held) = root.make_dir(at).map_err(|e| wrote(at, e))?;
        made.items.push((at.clone(), id, Some(held)));
    }
    between(Step::Folders)?;
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut copied = 0;
    for (at, item) in &payload.items {
        if cancel.load(Ordering::Relaxed) {
            return Err(stop());
        }
        let wrote = |e: std::io::Error| wrote(at, e);
        match item {
            Item::Dir => continue,
            Item::Made(bytes) => {
                let mut out = root.make_file(at).map_err(wrote)?;
                made.items.push((at.clone(), out.id, None));
                out.write_all(bytes).map_err(wrote)?;
            }
            Item::File(origin) => {
                let mut input = payload.open(origin).map_err(wrote)?;
                let mut out = root.make_file(at).map_err(wrote)?;
                made.items.push((at.clone(), out.id, None));
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(stop());
                    }
                    let n = input.read(&mut buffer).map_err(wrote)?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buffer[..n]).map_err(wrote)?;
                }
                // Its modification time: Unity compares it with what it recorded, and a copy with Library then has nothing
                // to look at again.
                out.like(&input).map_err(wrote)?;
            }
        }
        copied += 1;
        *progress.lock().unwrap() = Progress { copied, total, file: at.display().to_string(), phase: mark("Copying project").into() };
    }
    Ok(())
}

/// Takes away the copy's own marker: from here on it is a project.
pub(crate) fn unmark(root: &WriteRoot, id: FileId) -> anyhow::Result<()> {
    let gone = root.remove(Path::new(INCOMPLETE), id, false).unwrap_or(false);
    anyhow::ensure!(gone, "{}", tr!("{path} could not be removed.", path = root.path.join(INCOMPLETE).display()));
    Ok(())
}

/// A Stop: what this run made goes, each thing by its identity; anything else found there stays and is named. What is said.
pub(crate) fn undo(root: WriteRoot, new: NewFolder, made: Made) -> String {
    let path = root.path.clone();
    let mut left = vec![];
    // The marker, made first, goes last: until then the folder is never taken for a project.
    for (rel, id, held) in made.items.into_iter().rev() {
        let dir = held.is_some();
        drop(held);
        if !root.remove(&rel, id, dir).unwrap_or(false) {
            left.push(rel.display().to_string());
        }
    }
    // What is there besides is not the copy's.
    left.extend(root.entries(Path::new("")).into_iter().flatten().map(|e| e.name.to_string_lossy().into_owned()).filter(|n| !left.iter().any(|l| l == n)).collect::<Vec<_>>());
    let inside = deepest(&root, &left);
    drop(root);
    match new.remove() {
        Ok(true) => tr("Copy stopped. The incomplete copy was removed.").into(),
        _ if left.is_empty() => {
            tr!("Copy stopped. The incomplete copy at {path} could not be removed ({why}); it is not listed as a project.", path = path.display(), why = tr("it is not the folder this copy made"))
        }
        _ => tr!("Copy stopped. What it copied was removed; {path} keeps what was not the copy's: {files}", path = path.display(), files = inside.iter().take(5).cloned().collect::<Vec<_>>().join(", ")),
    }
}

/// The files in what stayed, by their paths (what stayed itself when it holds none).
fn deepest(root: &WriteRoot, left: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for at in left {
        let files: Vec<String> = root.entries(Path::new(at)).into_iter().flatten().map(|e| Path::new(at).join(e.name).display().to_string()).collect();
        for f in if files.is_empty() { vec![at.clone()] } else { files } {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}
