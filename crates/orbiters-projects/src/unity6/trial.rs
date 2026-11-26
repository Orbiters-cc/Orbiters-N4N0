//! A Unity 6 trial of a project: a copy Orbiters makes beside the original, verifies byte for byte, and hands to the person,
//! who adds it in Creator Companion, which migrates it. The original stays on Unity 2022, is only ever read (`payload`), and
//! nothing Orbiters writes goes anywhere but the trial's own folder and app data (`gate`): **no file of Creator Companion's or
//! ALCOM's is ever written for a trial**, neither its list nor its database.
//!
//! What a trial is now is told by [`super::authority`] from the journal and the folder's identity; nothing in the trial's
//! folder is authority. Orbiters never changes a trial's packages or editor version: those are Creator Companion's
//! migration. It writes no file over another in it: the trial's project settings (its own product GUID) and the entries of
//! its local packages are written once, when the copy makes them.

use orbiters_core::locks::*;
use super::authority::{self, TrialState};
use super::gate::{self, Destination, Parents, Role, WriteRoot};
use super::journal::{self, Lease};
pub use super::journal::{jobs, read, set_aside, uncertain_text, Attempt, Health, Job, Phase, Place, Process, Record, Target};
use super::outcome::{self, Outcome};
use super::payload::{self, Making, Own, Payload};
use super::standing::{self, Standing, State};
use crate::copying::{self, between, Made, Step};
use crate::templates::{Marker, Progress, SharedProgress, INCOMPLETE};
use orbiters_i18n::{mark, tr};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Attempts (and their logs) kept per trial.
pub const KEEP_ATTEMPTS: usize = 10;
/// How long a launch reserved but never confirmed blocks another one.
pub const RESERVATION_SECS: u64 = 120;
/// The verified manifest of a trial, beside its logs.
const VERIFIED: &str = "verified-files.txt";
/// The most differences kept in the record.
const DIFFERENCES: usize = 200;

/// Whether Orbiters makes and changes trials on this computer: on Windows, where a folder can be held while it is worked on.
/// Elsewhere it only shows what is there and what VRChat's guide says.
pub fn available() -> bool {
    #[cfg(test)]
    if let Some(on) = AVAILABLE.with(|a| a.get()) {
        return on;
    }
    cfg!(windows)
}

#[cfg(test)]
thread_local! {
    /// A test's platform (the trial's own tests run on every one).
    pub(crate) static AVAILABLE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Asked first by everything that makes, finishes, takes, forgets or opens a trial.
fn capable() -> anyhow::Result<()> {
    anyhow::ensure!(available(), "{}", tr("Orbiters makes and changes Unity 6 trials on Windows only for now: here it shows what the project needs and VRChat's own guide."));
    Ok(())
}

/// The job `id` as recorded, when its record can be trusted.
fn job_of(dir: &Path, id: &str) -> anyhow::Result<Job> {
    let record = journal::read(dir);
    if let Some(why) = record.error.clone().or_else(|| record.uncertain(id)) {
        anyhow::bail!("{}", journal::uncertain_text(&why));
    }
    record.job(id).cloned().ok_or_else(|| anyhow::anyhow!("{}", journal::gone_text()))
}

/// Where a trial stands now, as it is shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Being worked on, by this window or another process.
    Running,
    /// Stopped in the middle (the app closed, the computer stopped): never taken up again.
    Interrupted,
    /// The copy failed, was stopped, or is not what the original holds; `kept`: a folder of it is still there.
    Failed { kept: bool },
    /// Verified; its last step (the copy's marker) is left.
    Unfinished,
    /// Verified and whole; Creator Companion has not migrated it yet. `vcc_copy`: a migrated copy VCC made beside it.
    Waiting { vcc_copy: Option<PathBuf> },
    /// Its files show Unity 6 with an SDK that needs it.
    OnUnity6,
    /// Its files show something else (an SDK half there, editor and SDK on different lines).
    Unexpected(State),
    /// Not at its path (a disk not connected, or moved).
    Missing,
    /// Another folder is at its path now: Orbiters does not act on it.
    Replaced,
    /// Part of the record about it cannot be read: Orbiters does not act on it.
    Uncertain(String),
    /// Taken off by the person: not shown.
    Abandoned,
}

impl Status {
    /// Whether the gallery shows it as a job rather than a project card.
    pub fn is_job(&self) -> bool {
        matches!(self, Status::Running | Status::Interrupted | Status::Failed { .. } | Status::Unfinished | Status::Missing | Status::Replaced | Status::Uncertain(_))
    }

    /// A stable name for machines (the bridge).
    pub fn name(&self) -> &'static str {
        match self {
            Status::Running => "preparing",
            Status::Interrupted => "interrupted",
            Status::Failed { .. } => "copy_failed",
            Status::Unfinished => "copied_verified",
            Status::Waiting { .. } => "waiting_for_creator_companion",
            Status::OnUnity6 => "on_unity6",
            Status::Unexpected(_) => "unexpected",
            Status::Missing => "missing",
            Status::Replaced => "replaced",
            Status::Uncertain(_) => "record_unreadable",
            Status::Abandoned => "abandoned",
        }
    }
}

/// Where `job` stands, by the record in `dir` (as read) and the disk.
pub fn status(dir: &Path, record: &Record, job: &Job) -> Status {
    if matches!(job.phase, Phase::Abandoned { .. }) {
        return Status::Abandoned;
    }
    if let Some(why) = record.error.clone().or_else(|| record.uncertain(&job.id)) {
        return Status::Uncertain(why);
    }
    if Lease::held(dir, &job.id) {
        return Status::Running;
    }
    let at = job.unity6();
    let exists = at.path.exists();
    match job.phase {
        Phase::Planned if !exists => return Status::Failed { kept: false },
        Phase::Planned | Phase::Copying if !exists => return Status::Missing,
        Phase::Planned | Phase::Copying if job.trial.verified() == Some(false) => return Status::Replaced,
        Phase::Planned | Phase::Copying => return Status::Interrupted,
        Phase::Failed => return Status::Failed { kept: exists && job.trial.verified() != Some(false) },
        _ => {}
    }
    if !exists {
        return Status::Missing;
    }
    // From here on a positive identity is needed: an unknown one is not taken for the trial.
    if at.verified() != Some(true) {
        return Status::Replaced;
    }
    if job.phase == Phase::Verified {
        return Status::Unfinished;
    }
    match standing::read(&at.path, &super::matrix::Matrix::built_in()).state {
        State::OnUnity6 => Status::OnUnity6,
        State::On2022 => Status::Waiting { vcc_copy: vcc_copy(job) },
        other => Status::Unexpected(other),
    }
}

/// The copy Creator Companion's migration makes beside a project when it is not told to work in place: exactly
/// `<trial>-Migrated`, on Unity 6, not one of the folders that were beside the trial when it was handed over (when those
/// could not all be read, none is offered), and the same project by its files ([`related`]).
fn vcc_copy(job: &Job) -> Option<PathBuf> {
    if job.migrated.is_some() {
        return None;
    }
    let before = job.beside.as_ref()?;
    let beside = job.trial.path.with_file_name(format!("{}-Migrated", job.trial.path.file_name()?.to_string_lossy()));
    let id = crate::owned::id_of(&beside).ok()?;
    (!before.contains(&id) && candidate(job, &beside).is_ok()).then_some(beside)
}

/// Whether the folder at `path` can be what Creator Companion made of the trial of `job`.
fn candidate(job: &Job, path: &Path) -> anyhow::Result<Place> {
    let place = Place::of(path)?;
    let no = |why: String| Err(anyhow::anyhow!("{}", tr!("{path} is not taken as this trial's migrated copy: {why}", path = path.display(), why = why)));
    if std::fs::symlink_metadata(path).map_or(true, |m| crate::owned::is_link(&m)) || !crate::discovery::is_project(path) {
        return no(tr("it is not a Unity project").into());
    }
    if place.id == job.trial.id || place.id == job.source.id {
        return no(tr("it is the trial or its original").into());
    }
    if standing::read(path, &super::matrix::Matrix::built_in()).state != State::OnUnity6 {
        return no(tr("it is not on Unity 6").into());
    }
    // The project itself first: the trial's own product GUID, which a migration keeps and another project (of the same template)
    // does not have.
    let own = std::fs::read_to_string(path.join("ProjectSettings/ProjectSettings.asset")).unwrap_or_default();
    let guid = own.lines().find_map(|l| l.trim().strip_prefix("productGUID:")).map(str::trim);
    if job.guid.is_empty() || guid != Some(job.guid.as_str()) {
        return no(tr("it is not the trial's project (its product GUID is another)").into());
    }
    if !related(&job.trial.path, path) {
        return no(tr("its Assets are not the trial's").into());
    }
    Ok(place)
}

/// Whether two projects' `Assets` are the same project's: no asset both have has another GUID (which no import or upgrade
/// changes), and the second has nine tenths of the first one's. Files without a GUID count only where nothing has one.
fn related(first: &Path, second: &Path) -> bool {
    let list = |root: &Path| -> Option<(BTreeMap<PathBuf, String>, BTreeSet<PathBuf>)> {
        let root = root.join("Assets");
        let (mut guids, mut files) = (BTreeMap::new(), BTreeSet::new());
        for e in walkdir::WalkDir::new(&root).follow_links(false) {
            let e = e.ok()?;
            if !e.file_type().is_file() {
                continue;
            }
            let at = e.path().strip_prefix(&root).ok()?.to_path_buf();
            if e.path().extension().is_some_and(|x| x == "meta") {
                let text = std::fs::read_to_string(e.path()).ok()?;
                if let Some(guid) = text.lines().find_map(|l| l.strip_prefix("guid:")) {
                    guids.insert(at, guid.trim().to_string());
                }
            } else {
                files.insert(at);
            }
        }
        Some((guids, files))
    };
    let (Some((a, a_files)), Some((b, b_files))) = (list(first), list(second)) else { return false };
    if a.iter().any(|(at, guid)| b.get(at).is_some_and(|other| other != guid)) {
        return false;
    }
    let (same, all) = match a.is_empty() {
        true => (a_files.intersection(&b_files).count(), a_files.len()),
        false => (a.keys().filter(|at| b.contains_key(*at)).count(), a.len()),
    };
    all > 0 && same * 10 >= all * 9
}

/// What is in the way of a trial of a project, if anything: checked before anything is recorded, and again just before the
/// copy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Blocker {
    /// Not a VRChat project on the 2022 line with its SDK installed.
    NotOn2022(State),
    /// Unity has it open (VRChat's guide: close it first).
    Open,
    /// Packages its lock names are missing or not at their locked version.
    Incomplete,
    /// Packages only its Library holds (from Git or another registry): a copy without Library would lose them.
    OnlyInLibrary(Vec<String>),
    /// A local (`file:`) package that is not where the project says.
    LocalMissing(Vec<String>),
    /// A layout the copy cannot keep whole, by the files or packages concerned.
    Unsupported(Vec<String>),
    /// Part of it cannot be read.
    Unreadable(String),
    /// The project is itself a trial, or it cannot be told whether it is.
    Trial(String),
}

/// [`Blocker`]s of `source` from its own files, as `payload` read them.
pub fn blockers(dir: &Path, source: &Path, standing: &Standing, open: bool, payload: Option<&Payload>) -> Vec<Blocker> {
    let mut out = vec![];
    match authority::state(dir, source) {
        TrialState::NotATrial | TrialState::Abandoned { whole: true, .. } => {}
        TrialState::Unknown(why) => out.push(Blocker::Trial(why)),
        _ => out.push(Blocker::Trial(tr("This project is itself a Unity 6 trial.").into())),
    }
    if standing.state != State::On2022 {
        out.push(Blocker::NotOn2022(standing.state));
    }
    if open {
        out.push(Blocker::Open);
    }
    if !crate::templates::installed_dependencies_complete(source).unwrap_or(false) {
        out.push(Blocker::Incomplete);
    }
    let library = crate::templates::packages_only_in_library(source);
    if !library.is_empty() {
        out.push(Blocker::OnlyInLibrary(library));
    }
    if let Some(p) = payload {
        if !p.missing.is_empty() {
            out.push(Blocker::LocalMissing(p.missing.iter().map(|(name, _)| name.clone()).collect()));
        }
        if !p.problems.is_empty() {
            out.push(Blocker::Unsupported(p.problems.clone()));
        }
    }
    out
}

/// Whether Unity has `project` open (its lock held, or an editor whose command line names it).
pub fn is_open(project: &Path) -> bool {
    super::gate::unity_holds(project) || crate::editor::of(&crate::editor::running(), project, false).is_some()
}

/// The default name of a trial of `name`.
pub fn trial_name(name: &str) -> String {
    format!("{name} (Unity 6)") // i18n: not shown (a folder's name, the same in every language)
}

/// What looking at a place for a trial tells, before anything is made: the sheet's check, and the first half of [`prepare`].
pub struct Look {
    /// The trial's folder as it would be made (its real path), or why the place is refused.
    pub place: Result<PathBuf, String>,
    pub bytes: u64,
    pub blockers: Vec<Blocker>,
    pub linked: Vec<payload::Linked>,
    pub outside: Vec<payload::Outside>,
}

/// How a trial of `source` is made.
fn making<'a>(name: &'a str, from: Option<&'a str>, guid: &'a str) -> Making<'a> {
    Making { own: Some(Own { name, from, guid }), strict: true, ..Default::default() }
}

fn folder_name(source: &Path) -> Option<String> {
    source.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Looks at making a trial of `source` at `parent`/`name`, by reading only. `own`: where Orbiters keeps its files.
pub fn look(dir: &Path, own: &[&Path], source: &Path, parent: &Path, name: &str, open: bool, cancel: &AtomicBool) -> Look {
    let standing = standing::read(source, &super::matrix::Matrix::built_in());
    match payload::scan(source, making(name, folder_name(source).as_deref(), ""), cancel) {
        Ok(p) => Look {
            place: gate::apart(own, &p.protected).and_then(|()| Destination::check(parent, name, &p.protected, Parents::Exist)).map(|d| d.path()).map_err(|e| e.text()),
            bytes: p.bytes,
            blockers: blockers(dir, source, &standing, open, Some(&p)),
            linked: p.linked,
            outside: p.outside,
        },
        Err(e) => {
            let mut b = blockers(dir, source, &standing, open, None);
            b.push(Blocker::Unreadable(format!("{e:#}")));
            Look { place: Err(format!("{e:#}")), bytes: 0, blockers: b, linked: vec![], outside: vec![] }
        }
    }
}

/// What [`prepare`] needs.
pub struct Prepare<'a> {
    /// App data: the record and the logs.
    pub dir: &'a Path,
    pub source: &'a Path,
    pub parent: &'a Path,
    pub name: &'a str,
    pub target: Target,
    /// Whether Unity has a project open.
    pub open: &'a dyn Fn(&Path) -> bool,
    /// The disk check again, given the trial's path and the copy's measured size: an error stops before anything is
    /// recorded. Answers the high end of the first import's estimate.
    pub budget: &'a dyn Fn(&Path, u64) -> anyhow::Result<u64>,
    /// Called once the record exists (the window shows the job from then on).
    pub recorded: &'a dyn Fn(&Job),
}

/// Makes the trial: checks again, records, copies, compares every byte. Every failure after the record leaves it in
/// a phase [`status`] shows; the error says where the trial is. Nothing is made before every check passed.
pub fn prepare(p: Prepare, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<Job> {
    capable()?;
    let guid = uuid::Uuid::new_v4().simple().to_string();
    let from = folder_name(p.source);
    let how = making(p.name, from.as_deref(), &guid);
    // Read once, held while the trial is made: what it links to and takes packages from included.
    let payload = payload::scan(p.source, how, cancel)?;
    let standing = standing::read(p.source, &super::matrix::Matrix::built_in());
    let blocked = blockers(p.dir, p.source, &standing, (p.open)(p.source), Some(&payload));
    anyhow::ensure!(blocked.is_empty(), "{}", super::describe_blockers(&blocked));
    // Nothing is written anywhere the original can see: not the trial, not the record.
    gate::apart(&[p.dir], &payload.protected)?;
    let destination = Destination::check(p.parent, p.name, &payload.protected, Parents::Exist)?;
    let path = destination.path();
    let content = payload.content(cancel)?;
    let import_high = (p.budget)(&path, payload.bytes)?;
    let mut job = Job {
        id: uuid::Uuid::new_v4().simple().to_string(),
        created: crate::discovery::now(),
        source: Place::of(p.source)?,
        trial: Place { path, id: None, name: p.name.to_string() },
        phase: Phase::Planned,
        target: p.target.clone(),
        error: None,
        differences: vec![],
        attempts: vec![],
        migrated: None,
        beside: None,
        localized: payload.localized.clone(),
        linked: payload.linked.iter().map(|l| format!("{} → {}", l.at, l.to.display())).collect(),
        content: content.ids.iter().cloned().collect(),
        content_unsure: content.unsure.clone(),
        guid: guid.clone(),
        manifest: None,
        import_high,
    };
    let _lease = Lease::take(p.dir, &job.id)?;
    journal::insert(p.dir, &job)?;
    (p.recorded)(&job);
    let failed = |id: &str, text: String, differences: Vec<String>| -> anyhow::Error {
        let _ = journal::set(p.dir, id, |j| {
            j.phase = Phase::Failed;
            j.error = Some(text.clone());
            j.differences = differences.iter().take(DIFFERENCES).cloned().collect();
        });
        anyhow::anyhow!("{text}")
    };
    if let Err(e) = between(Step::Recorded) {
        return Err(failed(&job.id, format!("{e:#}"), vec![]));
    }
    // The source checked again at the start of copying: the person may have opened it since.
    if (p.open)(p.source) {
        return Err(failed(&job.id, tr("Unity opened the original before the copy began: nothing was copied.").into(), vec![]));
    }
    let new = match destination.make() {
        Ok(new) => new,
        Err(e) => return Err(failed(&job.id, tr!("The trial's folder could not be made: {error}", error = e), vec![])),
    };
    // Its identity is recorded before anything is written in it, and what is written goes through the proof of it only.
    job.trial.id = Some(new.id);
    let (recorded, id) = (job.trial.clone(), new.id);
    if let Err(e) = journal::set(p.dir, &job.id, |j| {
        j.trial = recorded;
        j.phase = Phase::Copying;
    }) {
        let _ = new.remove();
        return Err(failed(&job.id, format!("{e:#}"), vec![]));
    }
    let root = match new.root() {
        Ok(r) => r,
        Err(e) => return Err(failed(&job.id, format!("{e:#}"), vec![])),
    };
    // The trial is not among what the original can see, now that it exists.
    if payload.protected.iter().any(|x| x.id == Some(id)) {
        return Err(failed(&job.id, tr("Create the copy outside the source project.").into(), vec![]));
    }
    let mut made = Made::default();
    let marker = Marker { copy: job.id.clone(), destination: job.trial.path.clone(), started: job.created, folder: Some(id) };
    let copied = between(Step::Made).and_then(|()| copying::fill(&payload, &root, &marker, &mut made, cancel, progress)).and_then(|()| between(Step::Copied));
    if let Err(e) = copied {
        if cancel.load(Ordering::Relaxed) {
            return Err(failed(&job.id, copying::undo(root, new, made), vec![]));
        }
        return Err(failed(&job.id, format!("{e:#}"), vec![]));
    }
    // Every byte, read again on both sides; the original is listed anew, so what was added or removed meanwhile is seen too.
    let step = comparing(progress, payload.items.len() * 2);
    let wanted = (|| -> anyhow::Result<payload::Digests> {
        let again = payload::scan(p.source, how, cancel)?;
        anyhow::ensure!(Place::of(p.source)?.id == job.source.id, "{}", tr!("The original at {path} is not the folder the trial was copied from.", path = p.source.display()));
        again.digests(cancel, &step)
    })();
    let compared = wanted.and_then(|wanted| {
        let (found, odd) = payload::trial_digests(&root, cancel, &step)?;
        Ok((payload::differences(&wanted, &found, &odd), wanted))
    });
    // What it was uploaded as, from what is in it: every package made part of it, whatever its first look at the original saw.
    let compared = compared.and_then(|(d, w)| Ok((d, w, payload::copy_content(&root, cancel)?)));
    let (differences, wanted, inside) = match compared {
        Ok(d) => d,
        Err(_) if cancel.load(Ordering::Relaxed) => return Err(failed(&job.id, copying::undo(root, new, made), vec![])),
        Err(e) => return Err(failed(&job.id, tr!("The copy at {path} could not be compared with the original: {error}", path = job.trial.path.display(), error = format!("{e:#}")), vec![])),
    };
    if !differences.is_empty() {
        let text = tr!("The copy at {path} is not what the original holds ({count} differences): {first}", path = job.trial.path.display(), count = differences.len(), first = differences.iter().take(3).cloned().collect::<Vec<_>>().join("; "));
        return Err(failed(&job.id, text, differences));
    }
    if (p.open)(p.source) {
        return Err(failed(&job.id, tr!("Unity opened the original while it was copied: the copy at {path} may mix two states of it. Start a new trial with the original closed.", path = job.trial.path.display()), vec![]));
    }
    if let Err(e) = between(Step::Compared) {
        return Err(failed(&job.id, format!("{e:#}"), vec![]));
    }
    let text = payload::to_text(&wanted);
    let kept = super::gate::AppRoot::open(p.dir).and_then(|app| app.write_new(&job.logs().join(VERIFIED), text.as_bytes()));
    if let Err(e) = kept {
        return Err(failed(&job.id, format!("{e:#}"), vec![]));
    }
    let mut all = content;
    all.add(inside);
    let job = journal::set(p.dir, &job.id, |j| {
        j.phase = Phase::Verified;
        j.manifest = Some(journal::sha256(text.as_bytes()));
        j.error = None;
        j.content = all.ids.iter().cloned().collect();
        j.content_unsure = all.unsure.clone();
    })?;
    drop(made);
    between(Step::Verified)?;
    // `new` is held until it is handed over: the folder verified is the folder handed over.
    hand_over(p.dir, &job, &root, &wanted, progress)
}

/// The progress of a comparison of `total` entries, one step at a time.
fn comparing(progress: &SharedProgress, total: usize) -> impl Fn(&Path) + '_ {
    *progress.locked() = Progress { copied: 0, total, file: String::new(), phase: mark("Comparing every file with the original").into(), ..Default::default() };
    move |at| {
        let mut p = progress.locked();
        p.copied = (p.copied + 1).min(p.total);
        p.file = at.display().to_string();
    }
}

/// What was verified of a trial, as kept beside its logs; `None` when that is not what the job's record says it is.
fn verified(dir: &Path, job: &Job) -> Option<payload::Digests> {
    let kept = gate::AppRoot::existing(dir).and_then(|app| app.read(&job.logs().join(VERIFIED))).ok()?;
    (Some(journal::sha256(&kept)) == job.manifest).then(|| payload::from_text(&String::from_utf8_lossy(&kept)))?
}

/// The trial is not what was verified: it is a failed copy from now on.
fn altered(dir: &Path, job: &Job, differences: Vec<String>) -> anyhow::Error {
    let text = tr!("The copy at {path} is not what was verified any more ({count} differences): {first}", path = job.trial.path.display(), count = differences.len(), first = differences.iter().take(3).cloned().collect::<Vec<_>>().join("; "));
    let _ = journal::set(dir, &job.id, |j| {
        j.phase = Phase::Failed;
        j.error = Some(text.clone());
        j.differences = differences.iter().take(DIFFERENCES).cloned().collect();
    });
    anyhow::anyhow!("{text}")
}

/// From a verified copy (`wanted`: what it is, byte for byte): its marker goes, and it is compared once more. Only then is it
/// handed to the person, for Creator Companion.
fn hand_over(dir: &Path, job: &Job, root: &WriteRoot, wanted: &payload::Digests, progress: &SharedProgress) -> anyhow::Result<Job> {
    anyhow::ensure!(job.phase == Phase::Verified && job.trial.id == Some(root.id), "{}", tr!("The folder at {path} is not the trial Orbiters made.", path = job.trial.path.display()));
    let unfinished = |error: String| -> anyhow::Error {
        let text = tr!("The trial is at {path}, but it could not be finished: {error}", path = job.trial.path.display(), error = error);
        let _ = journal::set(dir, &job.id, |j| j.error = Some(text.clone()));
        anyhow::anyhow!("{text}")
    };
    let marker = Path::new(INCOMPLETE);
    match root.file(marker) {
        Ok(mut file) => {
            let id = file.id()?;
            let mut bytes = vec![];
            file.read_to_end(&mut bytes)?;
            drop(file);
            let ours = serde_json::from_slice::<Marker>(&bytes).is_ok_and(|m| m.copy == job.id);
            if !ours {
                return Err(unfinished(tr!("{path} could not be removed.", path = job.trial.path.join(INCOMPLETE).display())));
            }
            copying::unmark(root, id).map_err(|e| unfinished(format!("{e:#}")))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(unfinished(e.to_string())),
    }
    if let Err(e) = between(Step::MarkerGone) {
        return Err(unfinished(format!("{e:#}")));
    }
    let (found, odd) = payload::trial_digests(root, &AtomicBool::new(false), comparing(progress, wanted.len())).map_err(|e| unfinished(format!("{e:#}")))?;
    let differences = payload::differences(wanted, &found, &odd);
    if !differences.is_empty() {
        return Err(altered(dir, job, differences));
    }
    if let Err(e) = between(Step::Checked) {
        return Err(unfinished(format!("{e:#}")));
    }
    let beside = job.trial.path.parent().and_then(folders_in);
    journal::set(dir, &job.id, |j| {
        j.phase = Phase::Ready;
        j.error = None;
        j.beside = beside;
    })
}

/// The identities of everything in `folder`; `None` when any of it cannot be read.
fn folders_in(folder: &Path) -> Option<Vec<crate::owned::FileId>> {
    let mut out = vec![];
    for e in std::fs::read_dir(folder).ok()? {
        out.push(crate::owned::id_of(&e.ok()?.path()).ok()?);
    }
    Some(out)
}

/// Finishes a verified trial whose last step was cut short, after comparing every one of its files again, by content, with what was
/// verified (its packages included): anything altered, added or missing since makes it a failed copy, never a ready one.
pub fn finish(dir: &Path, id: &str, cancel: &AtomicBool, progress: &SharedProgress) -> anyhow::Result<Job> {
    capable()?;
    let _lease = Lease::take(dir, id)?;
    let job = job_of(dir, id)?;
    anyhow::ensure!(job.phase == Phase::Verified, "{}", tr!("The copy at {path} is not a verified one: start a new trial.", path = job.trial.path.display()));
    let root = WriteRoot::of_job(dir, id, Role::Trial)?;
    let Some(wanted) = verified(dir, &job) else {
        anyhow::bail!("{}", tr!("What was verified of the copy at {path} cannot be read any more: start a new trial.", path = job.trial.path.display()));
    };
    let (found, odd) = payload::trial_digests(&root, cancel, comparing(progress, wanted.len()))?;
    let mut differences = payload::differences(&wanted, &found, &odd);
    if differences.is_empty() && !crate::templates::installed_dependencies_complete(&job.trial.path).unwrap_or(false) {
        differences.push(tr("its installed packages are not the ones its lock names").into());
    }
    if !differences.is_empty() {
        return Err(altered(dir, &job, differences));
    }
    hand_over(dir, &job, &root, &wanted, progress)
}

/// Takes the copy Creator Companion made of the trial, at `path`, as the trial on Unity 6: chosen by the person, and checked
/// to be the same project ([`candidate`]).
pub fn adopt_vcc_copy(dir: &Path, id: &str, path: &Path) -> anyhow::Result<Job> {
    capable()?;
    let _lease = Lease::take(dir, id)?;
    let job = job_of(dir, id)?;
    anyhow::ensure!(job.phase == Phase::Ready && job.migrated.is_none(), "{}", tr("This trial does not wait for Creator Companion."));
    drop(WriteRoot::of_job(dir, id, Role::Trial)?);
    let place = candidate(&job, path)?;
    journal::set(dir, id, |j| j.migrated = Some(place.clone()))
}

/// What taking a trial off left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forgotten {
    /// Where the trial's folder is.
    pub at: PathBuf,
    pub there: bool,
}

/// Takes a trial off what Orbiters shows, never deleting any of it, and changing nothing of Creator Companion's: where the person
/// added it there, it stays on its list. Its folders and its content IDs stay known, with their protections.
pub fn abandon(dir: &Path, id: &str) -> anyhow::Result<Forgotten> {
    capable()?;
    let _lease = Lease::take(dir, id)?;
    let job = job_of(dir, id)?;
    let whole = match job.phase {
        Phase::Ready => true,
        // It may have been changed since: an ordinary project only when it is still what was verified.
        Phase::Verified => verified(dir, &job).zip(WriteRoot::of_job(dir, id, Role::Trial).ok()).is_some_and(|(wanted, root)| {
            let found = payload::trial_digests(&root, &AtomicBool::new(false), |_| {});
            found.is_ok_and(|(found, odd)| payload::differences(&wanted, &found, &odd).is_empty())
        }),
        Phase::Abandoned { whole } => whole,
        Phase::Planned | Phase::Copying | Phase::Failed => false,
    };
    let job = journal::set(dir, id, |j| j.phase = Phase::Abandoned { whole })?;
    let at = job.unity6().path.clone();
    let there = at.exists();
    Ok(Forgotten { at, there })
}

/// The folders of the trials shown as projects (whole, wherever they are): the gallery has them without any list of Creator
/// Companion's.
pub fn folders(dir: &Path) -> Vec<PathBuf> {
    let record = journal::read(dir);
    let shown = record.jobs.iter().filter(|j| j.phase == Phase::Ready && record.uncertain(&j.id).is_none());
    shown.flat_map(|j| [Some(&j.trial), j.migrated.as_ref()]).flatten().filter(|p| p.verified() == Some(true)).map(|p| p.path.clone()).collect()
}

/// What a launch needs: the editor's program, the project, the attempt's log; answers the process id when it has one.
pub type Start<'a> = &'a dyn Fn(&Path, &Path, &Path) -> anyhow::Result<Option<u32>>;

/// The editor a trial opens with (its version and its changeset), found and probed now.
pub type Find<'a> = &'a dyn Fn(&str, &str) -> super::editors::Found;

/// What is asked of the system's processes.
pub struct Processes<'a> {
    /// Whether the very process an attempt started still runs.
    pub alive: &'a dyn Fn(&Process) -> bool,
    /// The process started with this log, when one runs.
    pub of_log: &'a dyn Fn(&Path) -> Option<Process>,
}

/// This computer's.
pub const PROCESSES: Processes<'static> = Processes { alive: &alive, of_log: &of_log };

/// The process whose command line names the log at `log` (each attempt has its own).
pub fn of_log(log: &Path) -> Option<Process> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always));
    let wanted = crate::discovery::key(log);
    let names = |p: &sysinfo::Process| p.cmd().iter().any(|a| crate::discovery::key(Path::new(a)) == wanted);
    system.processes().values().find(|p| names(p)).map(|p| Process { pid: p.pid().as_u32(), started: p.start_time() })
}

/// When the process `pid` started, as the system says.
fn started_at(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut system = System::new();
    let pid = Pid::from_u32(pid);
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).map(|p| p.start_time())
}

/// Whether the very process an attempt started still runs: the same number, started at the same moment.
pub fn alive(process: &Process) -> bool {
    started_at(process.pid) == Some(process.started)
}

/// The SDK packages of `project` that are not all at the version of the reviewed row, or that say nothing.
fn sdk_reviewed(project: &Path, sdk: &str) -> bool {
    let read = |rel: &str| std::fs::read(project.join(rel)).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let Some(manifest) = read("Packages/vpm-manifest.json") else { return false };
    let version = |v: &serde_json::Value| v.as_str().or_else(|| v["version"].as_str()).map(ToOwned::to_owned);
    let mut names: BTreeSet<&str> = [standing::SDK_BASE].into();
    for name in [standing::SDK_AVATARS, standing::SDK_WORLDS] {
        if manifest["locked"].get(name).is_some() || manifest["dependencies"].get(name).is_some() || project.join("Packages").join(name).exists() {
            names.insert(name);
        }
    }
    names.iter().all(|name| {
        let installed = read(&format!("Packages/{name}/package.json")).and_then(|v| version(&v["version"]));
        version(&manifest["locked"][*name]).as_deref() == Some(sdk) && installed.as_deref() == Some(sdk)
    })
}

/// What the first import may still take of the trial's disk: its estimate, less what its Library holds already; nothing once
/// an opening's log says it finished, while the Library still has what it imported (a person may have cleared it since).
fn import_left(job: &Job) -> u64 {
    let kept = |name: &str| std::fs::metadata(job.unity6().path.join("Library").join(name)).is_ok_and(|m| m.is_file() && m.len() > 0);
    let imported = kept("ArtifactDB") && kept("SourceAssetDB");
    if imported && job.attempts.iter().any(|a| matches!(outcome::read(&a.log, false), Outcome::Imported { .. })) {
        return 0;
    }
    let library = walkdir::WalkDir::new(job.unity6().path.join("Library")).follow_links(false).into_iter().flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum::<u64>();
    job.import_high.saturating_sub(library)
}

#[cfg(test)]
thread_local! {
    /// A test's room on the trial's disk.
    pub(crate) static FREE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

fn free(root: &WriteRoot) -> u64 {
    #[cfg(test)]
    if let Some(free) = FREE.with(|f| f.get()) {
        return free;
    }
    root.free().unwrap_or(0)
}

fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e9)
}

/// Opens the trial in Unity 6, checked again at this moment: the very folder recorded; on exactly the reviewed tuple of its
/// SDK packages (every one of them), editor and changeset; not open; no other opening reserved or running; room for what its
/// first import may still take; and an editor found and probed now as exactly that version, build and processor. The attempt
/// is reserved in the journal before anything starts, with a log of its own in app data; folder, files and program are looked
/// at once more right before the start.
pub fn launch(dir: &Path, id: &str, find: Find, open: &dyn Fn(&Path) -> bool, processes: &Processes, start: Start) -> anyhow::Result<Attempt> {
    capable()?;
    let _lease = Lease::take(dir, id)?;
    let matrix = super::matrix::Matrix::built_in();
    let record = journal::read(dir);
    let job = reconcile(dir, job_of(dir, id)?, processes)?;
    let at = job.unity6().clone();
    match authority::state_in(dir, &record, &at.path) {
        TrialState::Migrated(j) if j.id == id => {}
        TrialState::WaitingForMigration(_) => anyhow::bail!("{}", tr("The trial is not on Unity 6 yet: migrate it with Creator Companion first.")),
        TrialState::Unknown(why) => anyhow::bail!("{why}"),
        _ => anyhow::bail!("{}", tr!("The folder at {path} is not the trial Orbiters made.", path = at.path.display())),
    }
    let role = if job.migrated.is_some() { Role::Migrated } else { Role::Trial };
    let root = WriteRoot::of_job(dir, id, role)?;
    let s = standing::read(&at.path, &matrix);
    let row = s.sdk.version().and_then(|v| matrix.row_for(v)).filter(|r| matrix.actionable(r)).cloned();
    let Some(row) = row.filter(|r| r.unity == s.editor.text && s.editor.revision.as_deref() == Some(r.changeset.as_str()) && sdk_reviewed(&at.path, &r.sdk)) else {
        anyhow::bail!(
            "{}",
            tr!("The trial is on Unity {editor} with SDK {sdk}, which this version of Orbiters has not reviewed: open it from Unity Hub yourself.", editor = s.editor.text, sdk = s.sdk.version().unwrap_or_default())
        );
    };
    anyhow::ensure!(!open(&at.path), "{}", tr("Unity has the trial open already."));
    let exact = || -> anyhow::Result<super::editors::Candidate> {
        match find(&s.editor.text, &row.changeset) {
            super::editors::Found::Exact(c) if c.identity.as_ref().is_some_and(|i| i.changeset.as_deref() == Some(row.changeset.as_str())) => Ok(c),
            super::editors::Found::Exact(c) => anyhow::bail!(
                "{}",
                tr!("{path} is Unity {version} of another build ({changeset}).", path = c.program.display(), version = s.editor.text, changeset = c.identity.and_then(|i| i.changeset).unwrap_or_default())
            ),
            _ => anyhow::bail!("{}", tr!("Unity {version} could not be verified on this computer: install it with Unity Hub, or locate a program that says it is that version.", version = s.editor.text)),
        }
    };
    let editor = exact()?;
    let (need, room) = (import_left(&job), free(&root));
    anyhow::ensure!(
        room >= need,
        "{}",
        tr!("The trial's disk has {free} GB free, and its first import may need up to {need} GB: free some space first.", free = gb(room), need = gb(need))
    );
    // Reserve the attempt: its number and its log are taken under the journal's lock.
    let now = crate::discovery::now();
    let app = super::gate::AppRoot::open(dir)?;
    let logs = job.logs();
    let (_, attempt) = journal::update(dir, id, |j| {
        let busy = j.attempts.iter().any(|a| (a.reserved && now.saturating_sub(a.started) < RESERVATION_SECS) || a.process.as_ref().is_some_and(processes.alive));
        anyhow::ensure!(!busy, "{}", tr("Another opening of this trial is starting or running."));
        // A log left by an opening whose record could not be saved keeps its number.
        let taken: BTreeSet<std::ffi::OsString> = app.files(&logs).into_iter().collect();
        let mut n = j.attempts.iter().map(|a| a.n).max().unwrap_or(0) + 1;
        while taken.contains(std::ffi::OsStr::new(&journal::log_name(n))) {
            n += 1;
        }
        app.write_new(&logs.join(journal::log_name(n)), b"").map_err(|_| anyhow::anyhow!("{}", tr("This attempt's log exists already.")))?;
        let a = Attempt { n, started: now, log: dir.join(&logs).join(journal::log_name(n)), editor: editor.program.clone(), unity: s.editor.text.clone(), start: None, process: None, reserved: true };
        j.attempts.push(a.clone());
        Ok(a)
    })?;
    // The last look before starting: still the very folder, still that version, still closed, still that program (held so
    // that nothing replaces it meanwhile).
    let held = super::gate::hold_program(&editor.program);
    let still = held.is_ok() && WriteRoot::of_job(dir, id, role).is_ok() && standing::EditorStamp::read(&at.path) == s.editor && !open(&at.path) && exact().is_ok_and(|c| c.program == editor.program && c.identity == editor.identity);
    let begun = if still { start(&editor.program, &at.path, &attempt.log) } else { Err(anyhow::anyhow!("{}", tr("The trial or its editor changed just before it was opened: look at it again."))) };
    let process = begun.as_ref().ok().copied().flatten().and_then(|pid| Some(Process { pid, started: started_at(pid)? }));
    let outcome = begun.as_ref().err().map(|e| match e.downcast_ref::<crate::QuitAtOnce>() {
        Some(q) => Outcome::ExitedAtOnce(q.0.clone()),
        None => Outcome::DidNotStart(format!("{e:#}")),
    });
    let (n, mut old) = (attempt.n, vec![]);
    let job = journal::set(dir, id, |j| {
        if let Some(a) = j.attempts.iter_mut().find(|a| a.n == n) {
            a.process = process;
            a.start = outcome.clone();
            a.reserved = false;
        }
        // Kept: the last few attempts.
        while j.attempts.len() > KEEP_ATTEMPTS {
            old.push(j.attempts.remove(0).n);
        }
    })?;
    // Their logs go once the journal no longer names them.
    for n in old {
        let _ = app.remove_file(&logs.join(journal::log_name(n)));
    }
    begun?;
    Ok(job.attempts.into_iter().find(|a| a.n == n).unwrap_or(attempt))
}

/// An attempt reserved by a run that ended before it could say what it started (a crash) is bound to its editor, when one runs
/// with that attempt's own log, however late it came up.
fn reconcile(dir: &Path, job: Job, processes: &Processes) -> anyhow::Result<Job> {
    let found: Vec<(u32, Process)> = job.attempts.iter().filter(|a| a.reserved).filter_map(|a| Some((a.n, (processes.of_log)(&a.log)?))).collect();
    if found.is_empty() {
        return Ok(job);
    }
    journal::set(dir, &job.id, |j| {
        for a in &mut j.attempts {
            if let Some((_, p)) = found.iter().find(|(n, _)| *n == a.n) {
                a.process = Some(p.clone());
                a.reserved = false;
            }
        }
    })
}

/// Whether an attempt's own editor still runs: its very process, or (where the system gave none) Unity on the trial.
pub fn attempt_running(attempt: &Attempt, open: bool) -> bool {
    match &attempt.process {
        Some(p) => alive(p),
        None => open,
    }
}

/// What an attempt shows now: `running` is [`attempt_running`].
pub fn attempt_outcome(attempt: &Attempt, running: bool) -> Outcome {
    if let Some(start) = &attempt.start {
        return start.clone();
    }
    outcome::read(&attempt.log, running)
}

#[cfg(test)]
pub(crate) mod tests;
