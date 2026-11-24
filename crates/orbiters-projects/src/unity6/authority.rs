//! What may happen to a project now, decided in one place from the trials' record, the folder's identity and its files:
//! [`state`] is the only reading of where a project stands as a trial, and [`allow`] and [`upload`] are asked by everything
//! that opens a project, changes its packages, moves its Library, copies it or leads to an upload of it (the window's
//! actions, the Toolkit's and My Avatar's, the bridge's tools, whatever comes next). What cannot be read is refused, never
//! taken for "not a trial".
//!
//! What it was told is never enough for later: whoever acts asks again when it does.
//!
//! The upload warning follows the **content IDs**, not the folder: a duplicate of a trial, or a restored copy of one, has the
//! IDs of the original and gets the same answer.

use super::journal::{self, Job, Phase, Record};
use super::standing::{self, EditorStamp, State};
use crate::owned;
use orbiters_i18n::tr;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Where a project stands as a Unity 6 trial.
#[derive(Clone, Debug, PartialEq)]
pub enum TrialState {
    NotATrial,
    /// Being made now, by this process or another one.
    Copying(Job),
    /// The copy failed, stopped, or is not what the original holds: kept, never used.
    CopyFailed(Job),
    /// Every byte compared; its last step is left.
    CopiedVerified(Job),
    /// Ready; Creator Companion has not migrated it.
    WaitingForMigration(Job),
    /// Its files show the reviewed line of Unity 6.
    Migrated(Job),
    /// The first copy, once the copy Creator Companion made was taken in its place: it stays on Unity 2022, unused.
    Superseded(Job),
    /// Taken off by the person, and not on Unity 6. `whole`: it was verified, so it is an ordinary project now; else it never was
    /// complete. (One on Unity 6 stays [`TrialState::Migrated`], and the first copy of an adopted pair `Superseded`: taking a
    /// trial off takes none of its protections off.)
    Abandoned { job: Job, whole: bool },
    /// It cannot be told, and why.
    Unknown(String),
}

/// [`TrialState`] of the project at `path`, by the record in `dir`.
pub fn state(dir: &Path, path: &Path) -> TrialState {
    state_in(dir, &journal::read(dir), path)
}

/// [`state`] by a record already read.
pub fn state_in(dir: &Path, record: &Record, path: &Path) -> TrialState {
    if let Some(e) = &record.error {
        return TrialState::Unknown(tr!("The Unity 6 trials record cannot be read ({why}): Orbiters changes nothing.", why = e));
    }
    if let Some(why) = record.named(path) {
        return TrialState::Unknown(journal::uncertain_text(&why));
    }
    let key = crate::discovery::key(path);
    let resolved = std::fs::canonicalize(path);
    let id = match &resolved {
        Ok(real) => match owned::id_of(real) {
            Ok(id) => Some(id),
            // There, but not identifiable: it cannot be told from a trial.
            Err(e) if !record.jobs.is_empty() => return TrialState::Unknown(tr!("{path} cannot be identified on its disk: {error}", path = path.display(), error = e)),
            Err(_) => None,
        },
        Err(_) => None,
    };
    if let Some(why) = record.lost(path, id) {
        return TrialState::Unknown(journal::uncertain_text(&why));
    }
    let mut found: Option<(&Job, Which)> = None;
    for job in &record.jobs {
        // By identity (whatever its spelling or name now); by path only when its identity was never recorded.
        let is = |place: &journal::Place| match (place.id, id) {
            (Some(a), Some(b)) => a == b,
            // Not there now: a recorded path is still the trial's.
            (Some(_), None) => crate::discovery::key(&place.path) == key,
            (None, _) => crate::discovery::key(&place.path) == key,
        };
        if job.migrated.as_ref().is_some_and(is) {
            found = Some((job, Which::Migrated));
        } else if is(&job.trial) {
            found = Some((job, Which::Trial));
        }
    }
    let Some((job, which)) = found else { return TrialState::NotATrial };
    if let Some(why) = record.uncertain(&job.id) {
        return TrialState::Unknown(journal::uncertain_text(&why));
    }
    let job = job.clone();
    let first = which == Which::Trial && job.migrated.is_some();
    let on_unity6 = || standing::read(path, &super::matrix::Matrix::built_in()).state == State::OnUnity6;
    match job.phase {
        Phase::Abandoned { whole: false } => TrialState::Abandoned { job, whole: false },
        Phase::Abandoned { whole: true } if first => TrialState::Superseded(job),
        // Known to be on Unity 2022, as it was copied: an ordinary project. Anything else keeps a trial's protections.
        Phase::Abandoned { whole: true } if unity6_line(path) == Some(false) && !on_unity6() => TrialState::Abandoned { job, whole: true },
        Phase::Abandoned { whole: true } => TrialState::Migrated(job),
        Phase::Planned | Phase::Copying if journal::Lease::held(dir, &job.id) => TrialState::Copying(job),
        // Nobody works on it: it was cut short, and is never taken up again.
        Phase::Planned | Phase::Copying | Phase::Failed => TrialState::CopyFailed(job),
        Phase::Verified => TrialState::CopiedVerified(job),
        Phase::Ready if first => TrialState::Superseded(job),
        Phase::Ready if standing::read(&job.unity6().path, &super::matrix::Matrix::built_in()).state == State::OnUnity6 => TrialState::Migrated(job),
        Phase::Ready => TrialState::WaitingForMigration(job),
    }
}

/// Which of a job's folders.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Trial,
    Migrated,
}

/// Who asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The person, in the window.
    Window,
    /// A tool of the bridge.
    Bridge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Open it in Unity (or bring forward the editor that has it open).
    Open(Caller),
    /// Change its packages.
    Packages,
    /// Move its Library to another disk.
    Storage,
    /// Copy it into another project.
    Duplicate,
    /// Put it on the list VCC and ALCOM share.
    List,
    /// Take it off that list (the folder stays).
    Unlist,
}

/// How an allowed opening goes.
#[derive(Clone, Debug, PartialEq)]
pub enum Way {
    /// As any project.
    Plain,
    /// A trial on Unity 6: through its own launcher, with a log of its own.
    Trial(Box<Job>),
}

fn refused(state: &TrialState) -> String {
    match state {
        TrialState::Unknown(why) => why.clone(),
        TrialState::Copying(_) => tr("This Unity 6 trial is still being made.").into(),
        TrialState::CopyFailed(job) => tr!("This folder is a Unity 6 trial whose copy failed, not a project: it stays at {path}. Start a new trial.", path = job.trial.path.display()),
        TrialState::CopiedVerified(_) => tr("This Unity 6 trial is not finished: finish it in the Unity projects tab first.").into(),
        TrialState::WaitingForMigration(_) => tr("This trial waits for Creator Companion's migration: Orbiters does not open it or change its packages before.").into(),
        TrialState::Superseded(_) => tr("Creator Companion's migrated copy is this trial's Unity 6 copy now; this first copy stays on Unity 2022 and is not changed from here.").into(),
        TrialState::Abandoned { job, .. } => tr!("This folder is a Unity 6 trial that was taken off before it was complete: it stays at {path}, and is not a project to open.", path = job.trial.path.display()),
        TrialState::NotATrial | TrialState::Migrated(_) => String::new(),
    }
}

/// Whether the editor `path` is stamped with is of Unity 6 (6000 and later), and whether that could be read.
fn unity6_line(path: &Path) -> Option<bool> {
    let stamp = EditorStamp::read(path);
    let major = stamp.text.split('.').next()?.parse::<u32>().ok()?;
    stamp.parsed.then_some(major >= 6000)
}

/// Whether `action` may go on with the project at `path` (`mac`: this computer is one, for the tests of it). What is not
/// allowed says why, in words for the person.
pub fn allow(dir: &Path, path: &Path, action: Action) -> Result<Way, String> {
    allow_on(dir, path, action, cfg!(target_os = "macos"))
}

pub fn allow_on(dir: &Path, path: &Path, action: Action, mac: bool) -> Result<Way, String> {
    let state = state(dir, path);
    // The bridge opens nothing in Unity 6, trial or not, and says so instead of looking for an editor.
    if let Action::Open(Caller::Bridge) = action {
        if unity6_line(path) != Some(false) && !matches!(state, TrialState::Unknown(_)) {
            return Err(match unity6_line(path) {
                Some(_) => tr("This project is on Unity 6: no tool of the bridge opens it. Open it from the Unity projects tab, where its upload warning is.").into(),
                None => tr("This project does not say which Unity version it uses: no tool of the bridge opens it.").into(),
            });
        }
    }
    match (&state, action) {
        (TrialState::NotATrial | TrialState::Abandoned { whole: true, .. }, _) => Ok(Way::Plain),
        // A trial is in the gallery by its record: no other list has it, and only forgetting it takes it off.
        (state, Action::List | Action::Unlist) if !matches!(state, TrialState::Unknown(_)) => {
            Err(tr("This is a Unity 6 trial: it is on the page as long as Orbiters keeps its record. Forget the trial, in its panel, to take it off.").into())
        }
        // Where trials are only shown, nothing changes one.
        (TrialState::Migrated(_) | TrialState::WaitingForMigration(_) | TrialState::Superseded(_), Action::Packages | Action::Storage) if mac => {
            Err(tr("On the Mac Orbiters only shows a Unity 6 trial: it does not change one.").into())
        }
        (TrialState::Migrated(_), Action::Open(_)) if mac => Err(tr("On the Mac a Unity 6 trial is opened from Unity Hub: Orbiters shows where it is and what VRChat's guide says.").into()),
        (TrialState::Migrated(job), Action::Open(_)) => Ok(Way::Trial(Box::new(job.clone()))),
        (TrialState::Migrated(_), _) => Ok(Way::Plain),
        // A copy of a waiting trial is an ordinary project; what its content IDs mean is asked at each upload.
        (TrialState::WaitingForMigration(_) | TrialState::Superseded(_), Action::Duplicate) => Ok(Way::Plain),
        (TrialState::WaitingForMigration(_), Action::Storage) => Ok(Way::Plain),
        _ => Err(refused(&state)),
    }
}

/// Leave to prepare an upload of one project from VRChat's SDK panel. Made by [`upload`] only, so nothing that ends in the panel
/// can be reached without it, and asked again when it is used ([`Handoff::still`]).
#[derive(Debug)]
pub struct Handoff {
    dir: PathBuf,
    project: PathBuf,
    /// The very folder it is for.
    id: owned::FileId,
    /// What the person is told before and with the upload, when there is something to tell.
    pub warning: Option<String>,
}

impl Handoff {
    /// Whether it is still leave for the project at `path`: the same folder, and the same answer now.
    pub fn still(&self, path: &Path) -> Result<(), String> {
        if crate::discovery::key(&self.project) != crate::discovery::key(path) {
            return Err(tr("Orbiters has no leave to prepare an upload from this project.").into());
        }
        let now = upload(&self.dir, path)?;
        if now.id != self.id || now.warning != self.warning {
            return Err(tr("This project, or what it uploads, changed since its upload was prepared: start again.").into());
        }
        Ok(())
    }
}

/// The one-way rule of an upload from Unity 6, for a project that shares its content IDs.
fn locked_out(shared: &BTreeSet<String>) -> String {
    tr!(
        "This project is on Unity 6 and has the content IDs of a Unity 6 trial or its original ({ids}): uploading from it would end their uploads from Unity 2022. Orbiters does not prepare an upload from it.",
        ids = shared.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
    )
}

/// Leave to prepare an upload of the project at `path`, or why not. Reads its scenes and prefabs: call on a worker.
pub fn upload(dir: &Path, path: &Path) -> Result<Handoff, String> {
    let record = journal::read(dir);
    let state = state_in(dir, &record, path);
    if !matches!(state, TrialState::NotATrial | TrialState::Abandoned { whole: true, .. } | TrialState::Migrated(_)) {
        return Err(refused(&state));
    }
    let line = unity6_line(path);
    let id = owned::id_of(path).map_err(|e| tr!("{path} cannot be identified on its disk: {error}", path = path.display(), error = e))?;
    let leave = |warning: Option<String>| Ok(Handoff { dir: dir.to_path_buf(), project: path.to_path_buf(), id, warning });
    let known: BTreeSet<String> = record.jobs.iter().flat_map(|j| j.content.iter().cloned()).collect();
    // The original itself, on Unity 2022, is where its content is uploaded from.
    if line == Some(false) && record.jobs.iter().any(|j| j.source.id == Some(id)) {
        return leave(None);
    }
    // Files of a trial that may carry an ID, and parts of the record that may name one.
    let unsure: Vec<&String> = record.jobs.iter().flat_map(|j| &j.content_unsure).chain(record.unread.iter().map(|u| &u.why)).collect();
    if line == Some(false) && known.is_empty() && unsure.is_empty() {
        return leave(None);
    }
    let content = super::payload::content_of(path, &AtomicBool::new(false)).map_err(|e| {
        tr!("Orbiters could not read which content this project uploads ({error}), so it prepares no upload from it.", error = format!("{e:#}"))
    })?;
    let shared: BTreeSet<String> = content.ids.intersection(&known).cloned().collect();
    let may = content.unsure.iter().chain(unsure.iter().copied()).next().map(|what| {
        tr!(
            "Orbiters cannot read everything that may name this project's content ({what}): it may be the content of a Unity 6 trial or its original, whose uploads from Unity 2022 end once it is uploaded from Unity 6.",
            what = what
        )
    });
    let warning = match line {
        // A line that cannot be read may be Unity 6.
        None => return Err(tr("This project does not say which Unity version it uses, so Orbiters prepares no upload from it.").into()),
        Some(true) if !shared.is_empty() || matches!(state, TrialState::Migrated(_)) => return Err(locked_out(&shared)),
        // Part of the record cannot be read: whether its content is a trial's cannot be told.
        Some(true) if !record.unread.is_empty() => return Err(journal::uncertain_text(&record.unread[0].why)),
        Some(true) => {
            let always = tr("This project is on Unity 6: once its content is uploaded from Unity 6, VRChat no longer lets it be uploaded from Unity 2022.");
            Some(may.map_or(always.to_string(), |may| format!("{always} {may}")))
        }
        Some(false) if !shared.is_empty() => Some(tr!(
            "This project has the content IDs of a Unity 6 trial or its original ({ids}): once one of them is uploaded from Unity 6, VRChat no longer lets it be uploaded from Unity 2022.",
            ids = shared.iter().take(4).cloned().collect::<Vec<_>>().join(", ")
        )),
        Some(false) => may,
    };
    leave(warning)
}

#[cfg(test)]
mod tests;
