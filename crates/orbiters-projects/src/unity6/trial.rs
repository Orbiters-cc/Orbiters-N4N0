//! "Try on Unity 6": a trial copy of a 2022 project, handed to VRChat Creator Companion for the migration itself, then
//! opened in the exact Unity 6 editor with a log of its own per attempt.
//!
//! Orbiters copies (the existing duplication: Library, caches and Git history left out, installed packages kept at their
//! exact versions, local packages made part of the copy), registers the copy, and records everything in **app data**:
//! the operation, both folders by path and by filesystem identity, the phase reached. That record, never a file inside a
//! project, says what Orbiters made and may act on: a copy of a project carries its files along, so a marker found in
//! a project proves nothing. It is written before the first file is copied, so a crash, a full disk, a failed
//! registration or a disk taken away at any moment leaves an entry the gallery shows with its path and what can be done.
//!
//! Orbiters never writes the trial's `vpm-manifest.json`, `packages-lock.json` or `ProjectVersion.txt`: Creator
//! Companion migrates the trial (VRChat's own path). Orbiters reads the trial's files to see when that happened, and
//! never offers Open or package changes on a trial still waiting for it. Nothing here deletes a trial: unregistering
//! only takes it off the lists.

use super::outcome::{self, Outcome};
use super::standing::{self, Standing, State};
use crate::owned::{self, FileId};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// The record, in app data.
pub const STORE: &str = "unity6-trials.json";
/// Where each attempt's log goes, in app data.
pub const LOGS: &str = "unity6-logs";

/// A folder as recorded: its path, its identity on its disk once it exists, its name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub path: PathBuf,
    pub id: Option<FileId>,
    pub name: String,
}

impl Place {
    fn of(path: &Path) -> Self {
        Self { path: path.to_path_buf(), id: owned::id_of(path).ok(), name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() }
    }
}

/// How far Orbiters' part went.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// Recorded; nothing copied yet.
    Planned,
    /// The trial's folder exists and files are going in.
    Copying,
    /// Every file is in.
    Copied,
    /// Copied, but VCC's and ALCOM's list could not take it.
    RegistrationFailed,
    /// Listed; its local packages not made part of it yet.
    Registered,
    /// Listed and independent of the original: waiting for Creator Companion's migration.
    Prepared,
    /// The copy stopped or failed.
    Failed,
}

/// The editor and SDK the trial is for (a built-in row of the release list).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub sdk: String,
    pub unity: String,
    pub changeset: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub n: u32,
    pub started: u64,
    pub log: PathBuf,
    pub editor: PathBuf,
    pub unity: String,
    /// What was known at the start: it could not start, or quit at once.
    #[serde(default)]
    pub start: Option<Outcome>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub created: u64,
    pub source: Place,
    pub trial: Place,
    pub phase: Phase,
    pub target: Target,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    /// The copy Creator Companion made itself (`<trial>-Migrated`) when its migration made one and the person took it.
    #[serde(default)]
    pub migrated: Option<Place>,
}

impl Job {
    /// The folder that is (or will be) on Unity 6.
    pub fn unity6(&self) -> &Place {
        self.migrated.as_ref().unwrap_or(&self.trial)
    }

    /// The fork day: fixes made to the original after it are not in the trial.
    pub fn forked(&self) -> u64 {
        self.created
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Store {
    schema: u32,
    jobs: Vec<Job>,
}

static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn store_path(dir: &Path) -> PathBuf {
    dir.join(STORE)
}

/// Every trial recorded in `dir` (app data).
pub fn jobs(dir: &Path) -> Vec<Job> {
    std::fs::read(store_path(dir)).ok().and_then(|b| serde_json::from_slice::<Store>(&b).ok()).map(|s| s.jobs).unwrap_or_default()
}

/// Changes the record of `id` (one writer at a time in this process).
fn update(dir: &Path, id: &str, f: impl FnOnce(&mut Job)) -> anyhow::Result<Job> {
    let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut list = jobs(dir);
    let job = list.iter_mut().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
    f(job);
    let out = job.clone();
    crate::write_json(&store_path(dir), &Store { schema: 1, jobs: list })?;
    Ok(out)
}

fn insert(dir: &Path, job: &Job) -> anyhow::Result<()> {
    let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut list = jobs(dir);
    list.retain(|j| j.id != job.id);
    list.push(job.clone());
    crate::write_json(&store_path(dir), &Store { schema: 1, jobs: list })
}

/// Takes the trial off Orbiters' record (its files and its listing stay as they are).
pub fn forget(dir: &Path, id: &str) -> anyhow::Result<()> {
    let _one = ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut list = jobs(dir);
    list.retain(|j| j.id != id);
    crate::write_json(&store_path(dir), &Store { schema: 1, jobs: list })
}

/// Where a trial stands now, from the record and the folders' identities and files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// Being prepared by this window.
    Running,
    /// Stopped before it was listed (the app closed, the computer stopped). `complete`: every file is in.
    Interrupted { complete: bool },
    /// The copy failed or was stopped; `kept`: a folder of it is still there.
    Failed { kept: bool },
    /// Copied, not listed.
    RegistrationFailed,
    /// Listed; Creator Companion has not migrated it yet. `vcc_copy`: a migrated copy VCC made beside it.
    Waiting { vcc_copy: Option<PathBuf> },
    /// Its files show Unity 6 with an SDK that needs it.
    OnUnity6,
    /// Its files show something else (an SDK half there, editor and SDK on different lines).
    Unexpected(State),
    /// Not at its path (a disk not connected, or moved).
    Missing,
    /// Another folder is at its path now: Orbiters does not act on it.
    Replaced,
}

impl Status {
    /// Whether the gallery shows it as a job rather than a project card.
    pub fn is_job(&self) -> bool {
        matches!(self, Status::Running | Status::Interrupted { .. } | Status::Failed { .. } | Status::RegistrationFailed | Status::Missing | Status::Replaced)
    }
}

fn identity_ok(place: &Place) -> Option<bool> {
    let id = place.id?;
    Some(owned::is_same(&place.path, id))
}

/// Where `job` stands: `running` when this window is preparing it now.
pub fn status(job: &Job, running: bool, matrix: &super::matrix::Matrix) -> Status {
    if running {
        return Status::Running;
    }
    let at = job.unity6();
    let exists = at.path.exists();
    match job.phase {
        Phase::Planned | Phase::Copying | Phase::Copied => {
            if !exists {
                return if job.phase == Phase::Planned { Status::Failed { kept: false } } else { Status::Missing };
            }
            if identity_ok(&job.trial) == Some(false) {
                return Status::Replaced;
            }
            return Status::Interrupted { complete: crate::discovery::is_project(&job.trial.path) };
        }
        Phase::Failed => return Status::Failed { kept: exists && identity_ok(&job.trial) != Some(false) },
        Phase::RegistrationFailed | Phase::Registered | Phase::Prepared => {}
    }
    if !exists {
        return Status::Missing;
    }
    if identity_ok(at) == Some(false) {
        return Status::Replaced;
    }
    if job.phase == Phase::RegistrationFailed {
        return Status::RegistrationFailed;
    }
    let s = standing::read(&at.path, matrix);
    match s.state {
        State::OnUnity6 => Status::OnUnity6,
        State::On2022 => Status::Waiting { vcc_copy: vcc_copy(&job.trial.path, matrix) },
        other => Status::Unexpected(other),
    }
}

/// The copy Creator Companion's migration makes beside a project when it is not told to work in place.
fn vcc_copy(trial: &Path, matrix: &super::matrix::Matrix) -> Option<PathBuf> {
    let name = trial.file_name()?.to_string_lossy();
    let beside = trial.with_file_name(format!("{name}-Migrated"));
    (crate::discovery::is_project(&beside) && standing::read(&beside, matrix).state == State::OnUnity6).then_some(beside)
}

/// What is in the way of a trial of `source`, if anything: checked before anything is recorded, and again just before
/// the copy.
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
}

pub fn blockers(source: &Path, standing: &Standing, open: bool) -> Vec<Blocker> {
    let mut out = vec![];
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
    let local = local_missing(source);
    if !local.is_empty() {
        out.push(Blocker::LocalMissing(local));
    }
    out
}

fn local_missing(project: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(project.join("Packages/manifest.json")) else { return vec![] };
    let Ok(m) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return vec![] };
    m["dependencies"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, v)| v.as_str()?.strip_prefix("file:").map(|p| (name, p)))
        .filter(|(_, p)| !project.join("Packages").join(p).exists())
        .map(|(n, _)| n.clone())
        .collect()
}

/// Whether Unity has `project` open (its lock held, or an editor whose command line names it).
pub fn is_open(project: &Path) -> bool {
    crate::unity_open(project) || crate::editor::of(&crate::editor::running(), project, false).is_some()
}

/// The default name of a trial of `name`.
pub fn trial_name(name: &str) -> String {
    format!("{name} (Unity 6)") // i18n: not shown (a folder's name, the same in every language)
}

/// What [`prepare`] needs.
pub struct Prepare<'a> {
    /// App data: the record and the logs.
    pub dir: &'a Path,
    /// VCC's and ALCOM's environment (the project list).
    pub vcc: &'a Path,
    pub source: &'a Path,
    pub parent: &'a Path,
    pub name: &'a str,
    pub target: Target,
    /// Whether Unity has a project open.
    pub open: &'a dyn Fn(&Path) -> bool,
    /// The disk check again, given the copy's measured size: an error stops before anything is recorded.
    pub budget: &'a dyn Fn(u64) -> anyhow::Result<()>,
    /// Called once the record exists (the window shows the job from then on).
    pub recorded: &'a dyn Fn(&Job),
}

/// Makes the trial: checks again, records, copies, lists it, makes its local packages part of it. Every failure after
/// the record leaves it in a phase [`status`] shows; the error says where the trial is.
pub fn prepare(p: Prepare, cancel: &AtomicBool, progress: &crate::templates::SharedProgress) -> anyhow::Result<Job> {
    let matrix = super::matrix::Matrix::built_in();
    let source = p.source.to_path_buf();
    let blocked = blockers(&source, &standing::read(&source, &matrix), (p.open)(&source));
    anyhow::ensure!(blocked.is_empty(), "{}", super::describe_blockers(&blocked));
    crate::templates::validate_name(p.name)?;
    anyhow::ensure!(p.parent.is_absolute(), "{}", tr("Choose an absolute destination folder."));
    let destination = p.parent.join(p.name);
    anyhow::ensure!(!destination.exists(), "{}", tr!("{path} exists already: choose another name.", path = destination.display()));
    let copy = crate::templates::duplication_bytes(&source, crate::templates::Keep::default())?;
    (p.budget)(copy)?;

    let mut job = Job {
        id: uuid::Uuid::new_v4().simple().to_string(),
        created: crate::discovery::now(),
        source: Place::of(&source),
        trial: Place { path: destination.clone(), id: None, name: p.name.to_string() },
        phase: Phase::Planned,
        target: p.target.clone(),
        error: None,
        attempts: vec![],
        migrated: None,
    };
    insert(p.dir, &job)?;
    (p.recorded)(&job);
    let id = job.id.clone();
    let dir = p.dir.to_path_buf();
    let copied = crate::templates::copy_keeping_then(&source, &destination, crate::templates::Keep::default(), true, cancel, progress, |made| {
        let folder = owned::id_of(made).ok();
        let _ = update(&dir, &id, |j| {
            j.trial.id = folder;
            j.phase = Phase::Copying;
        });
    });
    if let Err(e) = copied {
        let text = format!("{e:#}");
        update(p.dir, &job.id, |j| {
            j.phase = Phase::Failed;
            j.error = Some(text.clone());
        })?;
        return Err(e);
    }
    job = update(p.dir, &job.id, |j| j.phase = Phase::Copied)?;
    finish(p.dir, p.vcc, &job.id, &source, cancel, progress)
}

/// From a complete copy: lists it, then makes its local packages part of it and gives it its own product GUID.
fn finish(dir: &Path, vcc: &Path, id: &str, source: &Path, cancel: &AtomicBool, progress: &crate::templates::SharedProgress) -> anyhow::Result<Job> {
    let job = jobs(dir).into_iter().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
    let at = job.trial.path.clone();
    anyhow::ensure!(identity_ok(&job.trial) != Some(false), "{}", tr!("Another folder is at {path} now: Orbiters does not change it.", path = at.display()));
    progress.lock().unwrap().phase = orbiters_i18n::mark("Registering project").into();
    let listed = crate::runtime().and_then(|rt| rt.block_on(crate::vpm::register(vcc, &at, true)));
    if let Err(e) = listed {
        let text = tr!("The trial is at {path}, but it could not be listed: {error}", path = at.display(), error = format!("{e:#}"));
        update(dir, id, |j| {
            j.phase = Phase::RegistrationFailed;
            j.error = Some(text.clone());
        })?;
        anyhow::bail!("{text}");
    }
    update(dir, id, |j| {
        j.phase = Phase::Registered;
        j.error = None;
    })?;
    let local = crate::templates::copy_local_packages(source, &at, cancel, progress);
    if let Err(e) = local {
        let text = tr!("The trial is listed at {path}, but its local packages could not be copied: {error}", path = at.display(), error = format!("{e:#}"));
        update(dir, id, |j| j.error = Some(text.clone()))?;
        anyhow::bail!("{text}");
    }
    let settings = at.join("ProjectSettings/ProjectSettings.asset");
    if let Ok(s) = std::fs::read_to_string(&settings) {
        let from = source.file_name().map(|n| n.to_string_lossy().into_owned());
        let _ = std::fs::write(&settings, crate::templates::retarget(&s, &job.trial.name, from.as_deref()));
    }
    update(dir, id, |j| j.phase = Phase::Prepared)
}

/// Lists an interrupted or unlisted trial again: only the very folder recorded, and only once every file is in.
pub fn register_again(dir: &Path, vcc: &Path, id: &str, cancel: &AtomicBool, progress: &crate::templates::SharedProgress) -> anyhow::Result<Job> {
    let job = jobs(dir).into_iter().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
    anyhow::ensure!(job.trial.id.is_some() && identity_ok(&job.trial) == Some(true), "{}", tr!("The folder at {path} is not the trial Orbiters made.", path = job.trial.path.display()));
    anyhow::ensure!(crate::discovery::is_project(&job.trial.path), "{}", tr!("The copy at {path} is incomplete: start a new trial.", path = job.trial.path.display()));
    if matches!(job.phase, Phase::Copying | Phase::Planned) {
        update(dir, id, |j| j.phase = Phase::Copied)?;
    }
    finish(dir, vcc, id, &job.source.path, cancel, progress)
}

/// Takes the copy Creator Companion made beside the trial as the trial on Unity 6, by its identity.
pub fn adopt_vcc_copy(dir: &Path, vcc: &Path, id: &str) -> anyhow::Result<Job> {
    let matrix = super::matrix::Matrix::built_in();
    let job = jobs(dir).into_iter().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
    let beside = vcc_copy(&job.trial.path, &matrix).ok_or_else(|| anyhow::anyhow!("{}", tr("Creator Companion's migrated copy is not there.")))?;
    crate::runtime()?.block_on(crate::vpm::register(vcc, &beside, true))?;
    update(dir, id, |j| j.migrated = Some(Place::of(&beside)))
}

/// Takes the trial off VCC's and ALCOM's list and off Orbiters' record: its files stay where they are.
pub fn unregister(dir: &Path, vcc: &Path, id: &str) -> anyhow::Result<PathBuf> {
    let job = jobs(dir).into_iter().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
    let rt = crate::runtime()?;
    for place in [Some(&job.trial), job.migrated.as_ref()].into_iter().flatten() {
        rt.block_on(crate::vpm::register(vcc, &place.path, false))?;
    }
    forget(dir, id)?;
    Ok(job.unity6().path.clone())
}

/// What a launch needs: the editor's program, the project, the attempt's log; answers the process id when it has one.
pub type Start<'a> = &'a dyn Fn(&Path, &Path, &Path) -> anyhow::Result<Option<u32>>;

/// Opens the trial in Unity 6, checked again at this moment: the very folder recorded, on Unity 6 by its files now, not
/// open already, and an editor that is exactly the version its `ProjectVersion.txt` names. Each attempt writes a log of
/// its own in app data.
pub fn launch(dir: &Path, id: &str, editor: &super::editors::Candidate, open: &dyn Fn(&Path) -> bool, start: Start) -> anyhow::Result<Attempt> {
    let matrix = super::matrix::Matrix::built_in();
    let job = jobs(dir).into_iter().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
    let at = job.unity6().clone();
    anyhow::ensure!(at.id.is_some() && identity_ok(&at) == Some(true), "{}", tr!("The folder at {path} is not the trial Orbiters made.", path = at.path.display()));
    let s = standing::read(&at.path, &matrix);
    anyhow::ensure!(s.state == State::OnUnity6, "{}", tr("The trial is not on Unity 6 yet: migrate it with Creator Companion first."));
    anyhow::ensure!(!open(&at.path), "{}", tr("Unity has the trial open already."));
    let wanted = s.editor.text.clone();
    let check = super::editors::judge(editor.identity.as_ref(), &wanted);
    let allowed = check == super::editors::Check::Verified || (editor.hint == super::editors::Hint::Located && check == super::editors::Check::Unverified);
    anyhow::ensure!(allowed, "{}", tr!("{path} is not verified as Unity {version}: locate the editor to use it.", path = editor.program.display(), version = wanted));
    let n = job.attempts.iter().map(|a| a.n).max().unwrap_or(0) + 1;
    let folder = dir.join(LOGS).join(&job.id);
    std::fs::create_dir_all(&folder)?;
    let log = folder.join(format!("attempt-{n}.log"));
    anyhow::ensure!(!log.exists(), "{}", tr("This attempt's log exists already."));
    let mut attempt = Attempt { n, started: crate::discovery::now(), log: log.clone(), editor: editor.program.clone(), unity: wanted, start: None };
    let started = start(&editor.program, &at.path, &log);
    if let Err(e) = &started {
        attempt.start = Some(Outcome::DidNotStart(format!("{e:#}")));
    }
    let recorded = attempt.clone();
    update(dir, id, |j| j.attempts.push(recorded))?;
    started.map(|_| attempt)
}

/// What an attempt shows now.
pub fn attempt_outcome(attempt: &Attempt, running: bool) -> Outcome {
    if let Some(start) = &attempt.start {
        return start.clone();
    }
    outcome::read(&attempt.log, running)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unity6::standing::tests::{project, U2022, U6};
    use crate::unity6::standing::{SDK_AVATARS, SDK_BASE};

    /// A 2022 avatar project with its SDK installed, and the record's folder.
    fn source(root: &Path) -> PathBuf {
        let p = root.join("Projects/My avatar");
        project(&p, Some(U2022), &[(SDK_BASE, "3.10.6"), (SDK_AVATARS, "3.10.6")], &[(SDK_BASE, "3.10.6", "2022.3"), (SDK_AVATARS, "3.10.6", "2022.3")]);
        std::fs::write(p.join("Packages/manifest.json"), r#"{"dependencies":{"com.unity.timeline":"1.7.5"}}"#).unwrap();
        std::fs::write(p.join("Packages/packages-lock.json"), r#"{"dependencies":{"com.unity.timeline":{"version":"1.7.5","source":"registry","url":"https://packages.unity.com"}}}"#).unwrap();
        std::fs::write(p.join("Assets/Avatar.unity"), "scene").unwrap();
        std::fs::write(p.join("ProjectSettings/ProjectSettings.asset"), "PlayerSettings:\n  productName: My avatar\n  productGUID: 0123\n").unwrap();
        p
    }

    fn target() -> Target {
        Target { sdk: "3.11.0-beta.1".into(), unity: "6000.0.67f1".into(), changeset: "78a1c2bbeb6a".into() }
    }

    struct Env {
        _temp: tempfile::TempDir,
        root: PathBuf,
        app: PathBuf,
        vcc: PathBuf,
        source: PathBuf,
    }

    fn env() -> Env {
        let temp = tempfile::Builder::new().prefix("unity6-trial").tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let source = source(&root);
        Env { app: root.join("app"), vcc: root.join("vcc"), source, root, _temp: temp }
    }

    fn run(e: &Env, budget: &dyn Fn(u64) -> anyhow::Result<()>) -> anyhow::Result<Job> {
        let closed = |_: &Path| false;
        let recorded = |_: &Job| {};
        prepare(
            Prepare { dir: &e.app, vcc: &e.vcc, source: &e.source, parent: &e.root.join("Projects"), name: &trial_name("My avatar"), target: target(), open: &closed, budget, recorded: &recorded },
            &AtomicBool::new(false),
            &Default::default(),
        )
    }

    /// The files Orbiters never writes on a trial.
    fn guarded(project: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        ["Packages/vpm-manifest.json", "Packages/packages-lock.json", "Packages/manifest.json", "ProjectSettings/ProjectVersion.txt"]
            .iter()
            .map(|f| (f.to_string(), std::fs::read(project.join(f)).ok()))
            .collect()
    }

    fn ok(_: u64) -> anyhow::Result<()> {
        Ok(())
    }

    #[test]
    fn a_trial_is_recorded_copied_listed_and_waits_for_creator_companion() {
        let e = env();
        let before = guarded(&e.source);
        let job = run(&e, &ok).unwrap();
        assert_eq!(job.phase, Phase::Prepared);
        assert_eq!(job.trial.path, e.root.join("Projects/My avatar (Unity 6)"));
        assert!(job.trial.id.is_some() && job.source.id.is_some(), "both bound to their identity");
        // The record is in app data, nowhere in either project.
        assert!(e.app.join(STORE).is_file());
        assert!(!e.source.join(STORE).exists() && !job.trial.path.join(STORE).exists());
        // Listed for VCC and ALCOM.
        let listed = crate::runtime().unwrap().block_on(crate::vpm::registered(&e.vcc)).unwrap();
        assert!(listed.iter().any(|p| crate::discovery::key(p) == crate::discovery::key(&job.trial.path)));
        // Its manifest, lock and editor stamp are the original's bytes: Orbiters wrote none of them.
        assert_eq!(guarded(&job.trial.path), before);
        assert_eq!(guarded(&e.source), before, "the original untouched");
        let m = super::super::matrix::Matrix::built_in();
        assert_eq!(status(&job, false, &m), Status::Waiting { vcc_copy: None });
        // Nothing to open while it waits.
        let editor = fake_editor(&e.root, "6000.0.67f1");
        assert!(launch(&e.app, &job.id, &editor, &|_| false, &|_, _, _| panic!("started")).is_err());
    }

    #[test]
    fn nothing_is_recorded_when_the_source_is_open_or_the_disk_is_short() {
        let e = env();
        let open = |_: &Path| true;
        let recorded = |_: &Job| panic!("recorded");
        let refused = prepare(
            Prepare { dir: &e.app, vcc: &e.vcc, source: &e.source, parent: &e.root.join("Projects"), name: "T", target: target(), open: &open, budget: &ok, recorded: &recorded },
            &AtomicBool::new(false),
            &Default::default(),
        );
        assert!(format!("{:#}", refused.unwrap_err()).contains("Unity"));
        let short = |need: u64| -> anyhow::Result<()> { anyhow::bail!("{need} bytes do not fit") };
        assert!(run(&e, &short).is_err());
        assert!(jobs(&e.app).is_empty(), "nothing recorded");
        assert!(!e.root.join("Projects/My avatar (Unity 6)").exists(), "nothing copied");
    }

    #[test]
    fn the_source_open_check_sees_unitys_lock() {
        let e = env();
        assert!(!is_open(&e.source));
        std::fs::create_dir_all(e.source.join("Temp")).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let _held = std::fs::OpenOptions::new().write(true).create(true).share_mode(0).open(e.source.join("Temp/UnityLockfile")).unwrap();
            assert!(is_open(&e.source), "the lock held as Unity holds it");
            assert!(blockers(&e.source, &standing::read(&e.source, &super::super::matrix::Matrix::built_in()), is_open(&e.source)).contains(&Blocker::Open));
        }
    }

    #[test]
    fn a_project_not_on_2022_or_missing_packages_is_blocked() {
        let e = env();
        let m = super::super::matrix::Matrix::built_in();
        assert!(blockers(&e.source, &standing::read(&e.source, &m), false).is_empty());
        std::fs::remove_dir_all(e.source.join("Packages/com.vrchat.avatars")).unwrap();
        let b = blockers(&e.source, &standing::read(&e.source, &m), false);
        assert!(b.contains(&Blocker::NotOn2022(State::SdkIncomplete)) && b.contains(&Blocker::Incomplete), "{b:?}");
        std::fs::write(e.source.join("Packages/manifest.json"), r#"{"dependencies":{"my.local":"file:../Local/my.local"}}"#).unwrap();
        std::fs::write(e.source.join("Packages/packages-lock.json"), r#"{"dependencies":{"my.git":{"version":"https://example.com/x.git","source":"git"}}}"#).unwrap();
        let b = blockers(&e.source, &standing::read(&e.source, &m), false);
        assert!(b.contains(&Blocker::LocalMissing(vec!["my.local".into()])) && b.contains(&Blocker::OnlyInLibrary(vec!["my.git".into()])), "{b:?}");
    }

    /// B4/M5: every point where preparing can stop leaves an entry the gallery shows, with what can be done.
    #[test]
    fn every_interruption_leaves_a_visible_actionable_entry() {
        let e = env();
        let m = super::super::matrix::Matrix::built_in();
        let job = run(&e, &ok).unwrap();
        let trial = job.trial.path.clone();
        let rewind = |phase: Phase| update(&e.app, &job.id, |j| j.phase = phase).unwrap();

        // The app stopped while files were going in: the marker is still at the copy's top.
        let marker = trial.join(crate::templates::INCOMPLETE);
        std::fs::write(&marker, serde_json::to_vec(&crate::templates::Marker { copy: "x".into(), destination: trial.clone(), started: 0, folder: job.trial.id }).unwrap()).unwrap();
        let j = rewind(Phase::Copying);
        assert_eq!(status(&j, false, &m), Status::Interrupted { complete: false });
        assert!(status(&j, false, &m).is_job());
        assert!(register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).is_err(), "an incomplete copy is not listed");

        // Stopped after the marker went, before the record said so: complete, listed again on request.
        std::fs::remove_file(&marker).unwrap();
        assert_eq!(status(&j, false, &m), Status::Interrupted { complete: true });
        let j = register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).unwrap();
        assert_eq!(j.phase, Phase::Prepared);

        // Registration failed (VCC's list unwritable): the copy is named, and listing it can be tried again.
        let blocked_vcc = e.root.join("vcc-blocked");
        std::fs::write(&blocked_vcc, b"not a folder").unwrap();
        rewind(Phase::Copied);
        let err = register_again(&e.app, &blocked_vcc, &job.id, &AtomicBool::new(false), &Default::default()).unwrap_err();
        assert!(format!("{err:#}").contains(&trial.display().to_string()), "{err:#}");
        let j = jobs(&e.app).into_iter().next().unwrap();
        assert_eq!((j.phase, status(&j, false, &m)), (Phase::RegistrationFailed, Status::RegistrationFailed));
        assert!(j.error.as_deref().unwrap_or_default().contains(&trial.display().to_string()));
        assert_eq!(register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).unwrap().phase, Phase::Prepared);

        // Restarted with the disk away: the entry stays, said missing, with its path; forgetting it touches no file.
        let away = Job { trial: Place { path: PathBuf::from(if cfg!(windows) { r"Q:\Detached\My avatar (Unity 6)" } else { "/Volumes/Detached/My avatar (Unity 6)" }), ..j.trial.clone() }, ..j.clone() };
        insert(&e.app, &away).unwrap();
        let reread = jobs(&e.app).into_iter().find(|x| x.id == away.id).unwrap();
        assert_eq!(status(&reread, false, &m), Status::Missing);
        assert!(status(&reread, false, &m).is_job());
        forget(&e.app, &away.id).unwrap();
        assert!(jobs(&e.app).is_empty());
        assert!(trial.join("ProjectSettings/ProjectVersion.txt").is_file(), "forgetting deletes nothing");
    }

    /// B4: the record is bound to the folder's identity: another folder put at the trial's path is not the trial, and a
    /// record found inside a project means nothing.
    #[test]
    fn the_record_is_bound_to_identity_and_a_copied_marker_is_inert() {
        let e = env();
        let m = super::super::matrix::Matrix::built_in();
        let job = run(&e, &ok).unwrap();
        // A record and a marker inside the original (as a copy or a restore would carry them) are not read.
        let planted = Job { id: "planted".into(), trial: Place { path: e.root.join("Projects/Elsewhere"), id: None, name: "Elsewhere".into() }, ..job.clone() };
        std::fs::write(e.source.join(STORE), serde_json::to_vec(&Store { schema: 1, jobs: vec![planted.clone()] }).unwrap()).unwrap();
        std::fs::write(job.trial.path.join(STORE), serde_json::to_vec(&Store { schema: 1, jobs: vec![planted] }).unwrap()).unwrap();
        assert_eq!(jobs(&e.app), vec![job.clone()], "only app data is the record: what a project carries is not read");
        // The trial moved away and another folder put at its place.
        let moved = e.root.join("Projects/Moved");
        std::fs::rename(&job.trial.path, &moved).unwrap();
        project(&job.trial.path, Some(U6), &[(SDK_BASE, "3.11.0-beta.1")], &[(SDK_BASE, "3.11.0-beta.1", "6000.0")]);
        assert_eq!(status(&job, false, &m), Status::Replaced);
        let editor = fake_editor(&e.root, "6000.0.67f1");
        assert!(launch(&e.app, &job.id, &editor, &|_| false, &|_, _, _| panic!("started on another folder")).is_err());
        assert!(register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).is_err());
    }

    fn fake_editor(root: &Path, version: &str) -> super::super::editors::Candidate {
        let program = root.join("Editors").join(version).join("Editor/Unity.exe");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"stand-in").unwrap();
        super::super::editors::Candidate {
            program,
            hint: super::super::editors::Hint::HubFolder,
            check: super::super::editors::Check::Verified,
            identity: Some(super::super::editors::Identity { version: version.into(), changeset: None, arch: vec![super::super::editors::Arch::here()] }),
            modules: Default::default(),
        }
    }

    /// What Creator Companion's migration leaves in the trial (the test stands in for it: Orbiters never does this).
    fn vcc_migrates(trial: &Path) {
        std::fs::write(trial.join("ProjectSettings/ProjectVersion.txt"), U6).unwrap();
        project(trial, Some(U6), &[(SDK_BASE, "3.11.0-beta.1"), (SDK_AVATARS, "3.11.0-beta.1")], &[(SDK_BASE, "3.11.0-beta.1", "6000.0"), (SDK_AVATARS, "3.11.0-beta.1", "6000.0")]);
    }

    #[test]
    fn after_the_migration_each_launch_has_its_own_log_and_the_wrong_editor_is_refused() {
        let e = env();
        let m = super::super::matrix::Matrix::built_in();
        let job = run(&e, &ok).unwrap();
        vcc_migrates(&job.trial.path);
        assert_eq!(status(&job, false, &m), Status::OnUnity6);
        let after_vcc = guarded(&job.trial.path);
        // The wrong editor, or one not verified and not located: refused before anything starts.
        let wrong = fake_editor(&e.root, "2022.3.22f1");
        assert!(launch(&e.app, &job.id, &wrong, &|_| false, &|_, _, _| panic!("2022 on a Unity 6 project")).is_err());
        let mut unverified = fake_editor(&e.root, "6000.0.67f1");
        (unverified.identity, unverified.check) = (None, super::super::editors::Check::Unverified);
        assert!(launch(&e.app, &job.id, &unverified, &|_| false, &|_, _, _| panic!("unverified")).is_err());
        // Open already: refused.
        let editor = fake_editor(&e.root, "6000.0.67f1");
        assert!(launch(&e.app, &job.id, &editor, &|_| true, &|_, _, _| panic!("open twice")).is_err());
        // Each attempt: a new log path in app data, passed to the editor.
        let seen = std::cell::RefCell::new(vec![]);
        let start = |program: &Path, project: &Path, log: &Path| -> anyhow::Result<Option<u32>> {
            seen.borrow_mut().push((program.to_path_buf(), project.to_path_buf(), log.to_path_buf()));
            Ok(Some(42))
        };
        let a1 = launch(&e.app, &job.id, &editor, &|_| false, &start).unwrap();
        std::fs::write(&a1.log, "Assets/A.cs(3,1): error CS1002: ; expected\n").unwrap();
        let a2 = launch(&e.app, &job.id, &editor, &|_| false, &start).unwrap();
        assert_ne!(a1.log, a2.log);
        assert!(a1.log.starts_with(e.app.join(LOGS)) && a2.log.starts_with(e.app.join(LOGS)));
        assert_eq!(seen.borrow()[1], (editor.program.clone(), job.trial.path.clone(), a2.log.clone()));
        assert!(matches!(attempt_outcome(&a1, false), Outcome::CompilerErrors { .. }));
        assert_eq!(attempt_outcome(&a2, false), Outcome::Undetermined(outcome::Unknown::NoLog), "its own log, not the first one's");
        // One that cannot start is recorded so.
        let failing = |_: &Path, _: &Path, _: &Path| -> anyhow::Result<Option<u32>> { anyhow::bail!("no such program") };
        assert!(launch(&e.app, &job.id, &editor, &|_| false, &failing).is_err());
        let j = jobs(&e.app).into_iter().next().unwrap();
        assert_eq!(j.attempts.len(), 3);
        assert!(matches!(attempt_outcome(&j.attempts[2], false), Outcome::DidNotStart(_)));
        // Through it all, after Creator Companion's change, Orbiters wrote none of the guarded files.
        assert_eq!(guarded(&job.trial.path), after_vcc);
    }

    #[test]
    fn creator_companions_own_migrated_copy_can_be_taken_as_the_trial() {
        let e = env();
        let m = super::super::matrix::Matrix::built_in();
        let job = run(&e, &ok).unwrap();
        // VCC's default: a migrated copy beside the trial, the trial itself left on 2022.
        let beside = job.trial.path.with_file_name("My avatar (Unity 6)-Migrated");
        project(&beside, Some(U6), &[(SDK_BASE, "3.11.0-beta.1")], &[(SDK_BASE, "3.11.0-beta.1", "6000.0")]);
        assert_eq!(status(&job, false, &m), Status::Waiting { vcc_copy: Some(beside.clone()) });
        let j = adopt_vcc_copy(&e.app, &e.vcc, &job.id).unwrap();
        assert_eq!(j.unity6().path, beside);
        assert_eq!(status(&j, false, &m), Status::OnUnity6);
        // Unregistering takes both off the lists and the record; every file stays.
        let left = unregister(&e.app, &e.vcc, &job.id).unwrap();
        assert_eq!(left, beside);
        assert!(beside.is_dir() && job.trial.path.is_dir());
        assert!(jobs(&e.app).is_empty());
        let listed = crate::runtime().unwrap().block_on(crate::vpm::registered(&e.vcc)).unwrap();
        assert!(!listed.iter().any(|p| crate::discovery::key(p) == crate::discovery::key(&beside) || crate::discovery::key(p) == crate::discovery::key(&job.trial.path)));
    }

    #[test]
    fn the_record_survives_a_restart() {
        let e = env();
        let job = run(&e, &ok).unwrap();
        // A new process reads the same file: nothing kept in memory is needed.
        let again: Store = serde_json::from_slice(&std::fs::read(e.app.join(STORE)).unwrap()).unwrap();
        assert_eq!(again.jobs, vec![job]);
    }
}
