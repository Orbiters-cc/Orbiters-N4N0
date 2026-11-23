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
//! The record is one transaction at a time across processes (a lock file beside it), a damaged record is never written
//! over (it is shown, its last good copy used, and set aside only on request), and every library entry point that opens a
//! project or changes its packages asks [`guard`] first, so a trial keeps its protections whoever acts on it.
//!
//! Orbiters never writes the trial's `vpm-manifest.json`, `packages-lock.json` or `ProjectVersion.txt`. Making local
//! packages part of the trial rewrites their `file:` entries in `Packages/manifest.json` to the trial's own copies (the
//! duplication contract), and the names are recorded and shown. Creator Companion migrates the trial (VRChat's own
//! path). Nothing here deletes a trial: unregistering only takes it off the lists.

use super::outcome::{self, Outcome};
use super::standing::{self, Standing, State};
use crate::owned::{self, FileId};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// The record, in app data, its last good copy, and the lock taken for each transaction.
pub const STORE: &str = "unity6-trials.json";
pub const BACKUP: &str = "unity6-trials.backup.json";
const LOCK: &str = "unity6-trials.lock";
/// Where each attempt's log goes, in app data.
pub const LOGS: &str = "unity6-logs";
/// Attempts (and their logs) kept per trial.
pub const KEEP_ATTEMPTS: usize = 10;
/// How long a launch reserved but never confirmed blocks another one.
pub const RESERVATION_SECS: u64 = 120;

/// A folder as recorded: its path, its identity on its disk once it exists, its name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub path: PathBuf,
    pub id: Option<FileId>,
    pub name: String,
}

impl Place {
    /// `path` with its identity, which must be readable.
    fn of(path: &Path) -> anyhow::Result<Self> {
        let id = owned::id_of(path).map_err(|e| anyhow::anyhow!("{}", tr!("{path} cannot be identified on its disk: {error}", path = path.display(), error = e)))?;
        Ok(Self { path: path.to_path_buf(), id: Some(id), name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() })
    }

    /// Whether the folder at its path is the very one recorded (`None`: no identity was recorded, so it cannot be told).
    pub fn verified(&self) -> Option<bool> {
        Some(owned::is_same(&self.path, self.id?))
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
    /// Listed; not independent of the original yet (local packages, settings): not ready for Creator Companion.
    Registered,
    /// Listed, independent of the original, its packages verified: waiting for Creator Companion's migration.
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
    /// The editor's process, when the system gave it.
    #[serde(default)]
    pub pid: Option<u32>,
    /// Reserved before starting, not confirmed yet (a crash between the two leaves it so).
    #[serde(default)]
    pub reserved: bool,
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
    /// Names beside the trial when it was handed over: one of them is never taken for Creator Companion's copy.
    #[serde(default)]
    pub siblings: Vec<String>,
    /// Local (`file:`) packages made part of the trial, whose `Packages/manifest.json` entries now name its own copies.
    #[serde(default)]
    pub localized: Vec<String>,
    /// The high end of the first import's estimate, checked again before the first opening.
    #[serde(default)]
    pub import_high: u64,
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

    /// Whether its original is still the folder recorded (lineage needs a positive match, verification's M10).
    pub fn source_verified(&self) -> bool {
        self.source.verified() == Some(true)
    }

    fn validate(&self) -> Result<(), String> {
        let hex = self.id.len() == 32 && self.id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
        let places = [Some(&self.source), Some(&self.trial), self.migrated.as_ref()];
        let absolute = places.iter().flatten().all(|p| p.path.is_absolute() && !p.path.components().any(|c| matches!(c, std::path::Component::ParentDir)));
        let (s, t) = (crate::discovery::key(&self.source.path), crate::discovery::key(&self.trial.path));
        let disjoint = s != t && !t.starts_with(&format!("{s}\\")) && !s.starts_with(&format!("{t}\\"));
        if !hex {
            return Err(tr!("a trial's id is not one Orbiters makes ({id})", id = self.id));
        }
        if !absolute || !disjoint {
            return Err(tr!("the folders of the trial {id} are not two separate absolute folders", id = self.id));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Store {
    schema: u32,
    jobs: Vec<Job>,
}

/// Whether the record can be trusted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    Fine,
    /// The record cannot be read (`why`); `backup`: its last good copy is what is shown. Nothing writes over it until the
    /// person sets it aside.
    Damaged { why: String, backup: bool },
}

/// The record as read.
#[derive(Clone, Debug)]
pub struct Record {
    pub jobs: Vec<Job>,
    pub health: Health,
    /// The damaged file's text, to recognise a project it names when nothing else is readable.
    raw: Option<String>,
}

fn parse(bytes: &[u8]) -> Result<Vec<Job>, String> {
    let store: Store = serde_json::from_slice(bytes).map_err(|e| tr!("it is not a record Orbiters can read ({error})", error = e))?;
    if store.schema != 1 {
        return Err(tr!("it is of format {schema}, which this version of Orbiters does not read", schema = store.schema));
    }
    for j in &store.jobs {
        j.validate()?;
    }
    Ok(store.jobs)
}

fn log_of(folder: &Path, n: u32) -> PathBuf {
    folder.join(format!("attempt-{n}.log"))
}

/// Every attempt's log is at its own place under app data, whatever the record says: a record edited by hand never makes
/// Orbiters read, show or remove a file elsewhere.
fn own_logs(dir: &Path, jobs: &mut [Job]) {
    for j in jobs {
        let folder = logs_of(dir, &j.id);
        for a in &mut j.attempts {
            a.log = log_of(&folder, a.n);
        }
    }
}

/// The record in `dir` (app data): missing is an empty record; damaged is said, with its last good copy when there is one.
pub fn record(dir: &Path) -> Record {
    let mut record = read_record(dir);
    own_logs(dir, &mut record.jobs);
    record
}

fn read_record(dir: &Path) -> Record {
    let main = std::fs::read(dir.join(STORE));
    let backup = std::fs::read(dir.join(BACKUP)).ok().and_then(|b| parse(&b).ok());
    match main {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => match backup {
            // The record went, its last good copy did not: someone removed it. Shown from the copy, never written over.
            Some(jobs) if !jobs.is_empty() => Record { jobs, health: Health::Damaged { why: tr("the record is missing").into(), backup: true }, raw: None },
            _ => Record { jobs: vec![], health: Health::Fine, raw: None },
        },
        Err(e) => Record { jobs: backup.clone().unwrap_or_default(), health: Health::Damaged { why: e.to_string(), backup: backup.is_some() }, raw: None },
        Ok(bytes) => match parse(&bytes) {
            Ok(jobs) => Record { jobs, health: Health::Fine, raw: None },
            Err(why) => Record { health: Health::Damaged { why, backup: backup.is_some() }, jobs: backup.unwrap_or_default(), raw: Some(String::from_utf8_lossy(&bytes).into_owned()) },
        },
    }
}

/// Every trial recorded in `dir` (app data), from its last good copy when the record is damaged.
pub fn jobs(dir: &Path) -> Vec<Job> {
    record(dir).jobs
}

/// The lock of one transaction on the record, across every process of this computer.
struct Locked(#[allow(dead_code)] std::fs::File);

fn lock(dir: &Path) -> anyhow::Result<Locked> {
    std::fs::create_dir_all(dir)?;
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(dir.join(LOCK))?;
    file.lock()?;
    Ok(Locked(file))
}

/// One read-modify-write of the record, the lock held throughout; refused while the record is damaged.
fn transact<R>(dir: &Path, f: impl FnOnce(&mut Vec<Job>) -> anyhow::Result<R>) -> anyhow::Result<R> {
    let _locked = lock(dir)?;
    let rec = record(dir);
    if let Health::Damaged { why, .. } = &rec.health {
        anyhow::bail!("{}", tr!("The Unity 6 trials record cannot be read ({why}): Orbiters changes nothing until it is set aside in the Unity projects tab.", why = why));
    }
    let mut jobs = rec.jobs;
    let out = f(&mut jobs)?;
    for j in &jobs {
        j.validate().map_err(|e| anyhow::anyhow!(e))?;
    }
    let store = Store { schema: 1, jobs };
    crate::write_json(&dir.join(STORE), &store)?;
    crate::write_json(&dir.join(BACKUP), &store)?;
    Ok(out)
}

/// Changes the record of `id`.
fn update(dir: &Path, id: &str, f: impl FnOnce(&mut Job)) -> anyhow::Result<Job> {
    transact(dir, |jobs| {
        let job = jobs.iter_mut().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
        f(job);
        Ok(job.clone())
    })
}

fn insert(dir: &Path, job: &Job) -> anyhow::Result<()> {
    job.validate().map_err(|e| anyhow::anyhow!(e))?;
    transact(dir, |jobs| {
        jobs.retain(|j| j.id != job.id);
        jobs.push(job.clone());
        Ok(())
    })
}

fn logs_of(dir: &Path, id: &str) -> PathBuf {
    dir.join(LOGS).join(id)
}

/// Takes the trial off Orbiters' record and removes its attempts' logs (Orbiters' own files in app data); the trial's
/// folder and its listing stay as they are.
pub fn forget(dir: &Path, id: &str) -> anyhow::Result<()> {
    let known = transact(dir, |jobs| {
        let before = jobs.len();
        jobs.retain(|j| j.id != id);
        Ok(jobs.len() < before)
    })?;
    // Only a trial that was recorded, whose id was validated then: its folder of logs is ours, under app data.
    if known {
        let _ = std::fs::remove_dir_all(logs_of(dir, id));
    }
    Ok(())
}

/// Sets a damaged record aside (renamed beside it, never deleted) and puts its last good copy back when there is one.
pub fn set_aside(dir: &Path) -> anyhow::Result<PathBuf> {
    let _locked = lock(dir)?;
    let rec = record(dir);
    let Health::Damaged { .. } = rec.health else { anyhow::bail!("{}", tr("The Unity 6 trials record is not damaged.")) };
    let aside = dir.join(format!("unity6-trials.damaged-{}.json", crate::discovery::now()));
    if dir.join(STORE).exists() {
        std::fs::rename(dir.join(STORE), &aside)?;
    }
    crate::write_json(&dir.join(STORE), &Store { schema: 1, jobs: rec.jobs })?;
    Ok(aside)
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
    /// Listed, but not independent of the original yet: Orbiters finishes it before anything else.
    Unfinished,
    /// Listed and independent; Creator Companion has not migrated it yet. `vcc_copy`: a migrated copy VCC made beside it.
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
        matches!(self, Status::Running | Status::Interrupted { .. } | Status::Failed { .. } | Status::RegistrationFailed | Status::Unfinished | Status::Missing | Status::Replaced)
    }

    /// A stable name for machines (the bridge).
    pub fn name(&self) -> &'static str {
        match self {
            Status::Running => "preparing",
            Status::Interrupted { complete: false } => "interrupted_incomplete",
            Status::Interrupted { complete: true } => "interrupted_complete",
            Status::Failed { .. } => "failed",
            Status::RegistrationFailed => "registration_failed",
            Status::Unfinished => "unfinished",
            Status::Waiting { .. } => "waiting_for_creator_companion",
            Status::OnUnity6 => "on_unity6",
            Status::Unexpected(_) => "unexpected",
            Status::Missing => "missing",
            Status::Replaced => "replaced",
        }
    }
}

/// Where `job` stands: `running` when this window is preparing it now. Policy reads the built-in release list only.
pub fn status(job: &Job, running: bool) -> Status {
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
            if job.trial.verified() == Some(false) {
                return Status::Replaced;
            }
            return Status::Interrupted { complete: crate::discovery::is_project(&job.trial.path) && job.trial.id.is_some() };
        }
        Phase::Failed => return Status::Failed { kept: exists && job.trial.verified() != Some(false) },
        Phase::RegistrationFailed | Phase::Registered | Phase::Prepared => {}
    }
    if !exists {
        return Status::Missing;
    }
    // From here on a positive identity is needed: an unknown one is not taken for the trial.
    if at.verified() != Some(true) {
        return Status::Replaced;
    }
    match job.phase {
        Phase::RegistrationFailed => return Status::RegistrationFailed,
        Phase::Registered => return Status::Unfinished,
        _ => {}
    }
    let s = standing::read(&at.path, &super::matrix::Matrix::built_in());
    match s.state {
        State::OnUnity6 => Status::OnUnity6,
        State::On2022 => Status::Waiting { vcc_copy: vcc_copy(job) },
        other => Status::Unexpected(other),
    }
}

/// The copy Creator Companion's migration makes beside a project when it is not told to work in place: exactly
/// `<trial>-Migrated`, made after the hand-over (not among the names there then, not older than the trial), on Unity 6.
fn vcc_copy(job: &Job) -> Option<PathBuf> {
    if job.migrated.is_some() {
        return None;
    }
    let name = format!("{}-Migrated", job.trial.path.file_name()?.to_string_lossy());
    if job.siblings.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
        return None;
    }
    let beside = job.trial.path.with_file_name(&name);
    let made = std::fs::metadata(&beside).ok()?.created().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let ok = made >= job.created && crate::discovery::is_project(&beside) && standing::read(&beside, &super::matrix::Matrix::built_in()).state == State::OnUnity6;
    ok.then_some(beside)
}

/// What Orbiters may do with the project at `path`, by the record (identity-bound): every entry point that opens a
/// project or changes its packages asks this first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Guard {
    /// Not a trial: as usual.
    Free,
    /// A trial that must not be opened or changed now, and why.
    Blocked(String),
    /// A trial on Unity 6: it opens through [`launch`], with its own log.
    Unity6(Job),
}

/// [`Guard`] of `project`, from the record in `dir`.
pub fn guard(dir: &Path, project: &Path) -> Guard {
    let rec = record(dir);
    let key = crate::discovery::key(project);
    let resolved = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());
    if let (Health::Damaged { backup: false, .. }, Some(raw)) = (&rec.health, &rec.raw) {
        // Nothing readable: a project the damaged text names is treated as a trial.
        let text = raw.to_lowercase().replace("\\\\", "\\").replace('/', "\\");
        if text.contains(&key) {
            return Guard::Blocked(tr("The Unity 6 trials record is damaged and names this project: set the record aside in the Unity projects tab first.").into());
        }
    }
    for job in &rec.jobs {
        // By identity first (another spelling, a junction, a folder moved since), by path only when no identity was
        // recorded: another folder at a recorded path is not the trial.
        let is = |place: &Place| match place.id {
            Some(id) => owned::id_of(&resolved).is_ok_and(|now| now == id),
            None => crate::discovery::key(&place.path) == key,
        };
        let first = is(&job.trial);
        let migrated = job.migrated.as_ref().is_some_and(is);
        if !first && !migrated {
            continue;
        }
        if first && job.migrated.is_some() {
            return Guard::Blocked(tr("Creator Companion's migrated copy is this trial's Unity 6 copy now; this first copy stays on Unity 2022 and is not changed from here.").into());
        }
        return match status(job, false) {
            Status::OnUnity6 => Guard::Unity6(job.clone()),
            Status::Waiting { .. } => Guard::Blocked(tr("This trial waits for Creator Companion's migration: Orbiters does not open it or change its packages before.").into()),
            _ => Guard::Blocked(tr("This Unity 6 trial is not ready: finish it or look at it in the Unity projects tab first.").into()),
        };
    }
    Guard::Free
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
    /// A layout the copy cannot keep whole: a local package with local references of its own, a folder in Packages
    /// shadowing a local package of the same name, or a Unity package lock that cannot be read.
    Unsupported(Vec<String>),
    /// The project is itself a recorded trial, or its record is damaged.
    Trial,
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
    let (missing, unsupported) = local_packages(source);
    if !missing.is_empty() {
        out.push(Blocker::LocalMissing(missing));
    }
    if !unsupported.is_empty() {
        out.push(Blocker::Unsupported(unsupported));
    }
    out
}

/// The project's local (`file:`) packages that are missing, and the layouts the copy cannot keep whole.
fn local_packages(project: &Path) -> (Vec<String>, Vec<String>) {
    let (mut missing, mut unsupported) = (vec![], vec![]);
    if let Ok(bytes) = std::fs::read(project.join("Packages/packages-lock.json")) {
        if serde_json::from_slice::<serde_json::Value>(&bytes).is_err() {
            unsupported.push("Packages/packages-lock.json".to_string());
        }
    }
    let Ok(bytes) = std::fs::read(project.join("Packages/manifest.json")) else { return (missing, unsupported) };
    let Ok(m) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        unsupported.push("Packages/manifest.json".to_string());
        return (missing, unsupported);
    };
    for (name, v) in m["dependencies"].as_object().into_iter().flatten() {
        let Some(local) = v.as_str().and_then(|s| s.strip_prefix("file:")) else { continue };
        let at = project.join("Packages").join(local);
        if !at.exists() {
            missing.push(name.clone());
            continue;
        }
        // Inside the project's Packages: copied with it. Outside: copied in as Packages/<name>, which must be free.
        let inside = at.canonicalize().ok().zip(project.canonicalize().ok()).is_some_and(|(a, p)| a.starts_with(p));
        if !inside && project.join("Packages").join(name).exists() {
            unsupported.push(name.clone());
            continue;
        }
        if at.is_file() && !inside {
            let taken = project.join("Packages").join(format!("{name}.tgz"));
            if taken.exists() && std::fs::read(&taken).ok() != std::fs::read(&at).ok() {
                unsupported.push(name.clone());
            }
        }
        if at.is_dir() {
            let deps = std::fs::read(at.join("package.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).unwrap_or_default();
            if deps["dependencies"].as_object().into_iter().flatten().any(|(_, v)| v.as_str().is_some_and(|s| s.starts_with("file:"))) {
                unsupported.push(name.clone());
            }
        }
    }
    (missing, unsupported)
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
    /// The disk check again, given the copy's measured size: an error stops before anything is recorded. Answers the
    /// high end of the first import's estimate.
    pub budget: &'a dyn Fn(u64) -> anyhow::Result<u64>,
    /// Called once the record exists (the window shows the job from then on).
    pub recorded: &'a dyn Fn(&Job),
}

/// Makes the trial: checks again, records, copies, lists it, makes it independent of the original. Every failure after
/// the record leaves it in a phase [`status`] shows; the error says where the trial is. Nothing is created before every
/// check passed, and nothing is ever written in the original.
pub fn prepare(p: Prepare, cancel: &AtomicBool, progress: &crate::templates::SharedProgress) -> anyhow::Result<Job> {
    let matrix = super::matrix::Matrix::built_in();
    let source = p.source.to_path_buf();
    if !matches!(guard(p.dir, &source), Guard::Free) {
        anyhow::bail!("{}", super::describe_blockers(&[Blocker::Trial]));
    }
    let blocked = blockers(&source, &standing::read(&source, &matrix), (p.open)(&source));
    anyhow::ensure!(blocked.is_empty(), "{}", super::describe_blockers(&blocked));
    crate::templates::validate_name(p.name)?;
    anyhow::ensure!(p.parent.is_absolute(), "{}", tr("Choose an absolute destination folder."));
    let destination = p.parent.join(p.name);
    let canonical = std::fs::canonicalize(&source)?;
    crate::templates::check_destination(&canonical, &destination, true)?;
    anyhow::ensure!(!destination.exists(), "{}", tr!("{path} exists already: choose another name.", path = destination.display()));
    let copy = crate::templates::duplication_bytes(&source, crate::templates::Keep::default())?;
    let import_high = (p.budget)(copy)?;
    let before = crate::templates::inventory(&source)?;

    let job = Job {
        id: uuid::Uuid::new_v4().simple().to_string(),
        created: crate::discovery::now(),
        source: Place::of(&source)?,
        trial: Place { path: destination.clone(), id: None, name: p.name.to_string() },
        phase: Phase::Planned,
        target: p.target.clone(),
        error: None,
        attempts: vec![],
        migrated: None,
        siblings: vec![],
        localized: vec![],
        import_high,
    };
    insert(p.dir, &job)?;
    (p.recorded)(&job);
    let id = job.id.clone();
    let dir = p.dir.to_path_buf();
    // The source checked again at the start of copying: the person may have opened it since.
    if (p.open)(&source) {
        update(p.dir, &id, |j| {
            j.phase = Phase::Failed;
            j.error = Some(tr("Unity opened the original before the copy began: nothing was copied.").into());
        })?;
        anyhow::bail!("{}", tr("Unity opened the original before the copy began: nothing was copied."));
    }
    let copied = crate::templates::copy_keeping_then(&source, &destination, crate::templates::Keep::default(), true, cancel, progress, |made| {
        let folder = owned::id_of(made).ok();
        let _ = update(&dir, &id, |j| {
            j.trial.id = folder;
            j.phase = Phase::Copying;
        });
    });
    if let Err(e) = copied {
        let text = format!("{e:#}");
        update(p.dir, &id, |j| {
            j.phase = Phase::Failed;
            j.error = Some(text.clone());
        })?;
        return Err(e);
    }
    // The original changed or was opened while it was copied: the copy may mix two states of it.
    if (p.open)(&source) || crate::templates::inventory(&source).ok().as_ref() != Some(&before) {
        let text = tr!("The original changed while it was copied: the copy at {path} may mix two states of it. Start a new trial with the original closed.", path = destination.display());
        update(p.dir, &id, |j| {
            j.phase = Phase::Failed;
            j.error = Some(text.clone());
        })?;
        anyhow::bail!("{text}");
    }
    update(p.dir, &id, |j| j.phase = Phase::Copied)?;
    finish(p.dir, p.vcc, &id, cancel, progress)
}

/// Whether `path` is a link, a junction or another reparse point.
fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| owned::is_link(&m))
}

/// Every local package of the trial named by a `file:` entry is a folder or archive inside its own `Packages`.
fn closure_inside(trial: &Path) -> anyhow::Result<()> {
    let Ok(bytes) = std::fs::read(trial.join("Packages/manifest.json")) else { return Ok(()) };
    let m: serde_json::Value = serde_json::from_slice(&bytes)?;
    let packages = std::fs::canonicalize(trial.join("Packages"))?;
    for (name, v) in m["dependencies"].as_object().into_iter().flatten() {
        let Some(local) = v.as_str().and_then(|s| s.strip_prefix("file:")) else { continue };
        let at = trial.join("Packages").join(local);
        let inside = at.canonicalize().is_ok_and(|a| a.starts_with(&packages));
        anyhow::ensure!(inside, "{}", tr!("The trial's package {name} is not inside the trial: it would still depend on another folder.", name = name));
    }
    Ok(())
}

/// From a complete copy: lists it, makes its local packages part of it (verified), gives it its own product GUID,
/// verifies its installed packages, then — only then — Prepared. Every step checks both folders by identity first.
fn finish(dir: &Path, vcc: &Path, id: &str, cancel: &AtomicBool, progress: &crate::templates::SharedProgress) -> anyhow::Result<Job> {
    let job = jobs(dir).into_iter().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
    let at = job.trial.path.clone();
    anyhow::ensure!(job.trial.verified() == Some(true), "{}", tr!("The folder at {path} is not the trial Orbiters made.", path = at.display()));
    anyhow::ensure!(job.source_verified(), "{}", tr!("The original at {path} is not the folder the trial was copied from: Orbiters does not take packages from it.", path = job.source.path.display()));
    for sub in ["Packages", "ProjectSettings"] { // i18n: not shown (folder names)
        anyhow::ensure!(!is_link(&at.join(sub)), "{}", tr!("{path} is a link: Orbiters does not write through it.", path = at.join(sub).display()));
    }
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
    let unfinished = |text: String| -> anyhow::Error {
        let _ = update(dir, id, |j| j.error = Some(text.clone()));
        anyhow::anyhow!("{text}")
    };
    let manifest_before = std::fs::read(at.join("Packages/manifest.json")).ok();
    // Room for the packages that come from outside the original, checked just before they are copied.
    let outside = outside_bytes(&job.source.path, &at);
    let free = free_space(&at).unwrap_or(0);
    if outside > free {
        return Err(unfinished(tr!("The trial is listed at {path}, but its disk has {free} GB free and its local packages need {need} GB.", path = at.display(), free = gb(free), need = gb(outside))));
    }
    if let Err(e) = crate::templates::copy_local_packages(&job.source.path, &at, cancel, progress) {
        return Err(unfinished(tr!("The trial is listed at {path}, but its local packages could not be copied: {error}", path = at.display(), error = format!("{e:#}"))));
    }
    if let Err(e) = closure_inside(&at) {
        return Err(unfinished(format!("{e:#}")));
    }
    let localized = local_names(manifest_before.as_deref());
    // Both folders again, right before the last write: nothing was put at their place since.
    if job.trial.verified() != Some(true) || is_link(&at.join("ProjectSettings")) || is_link(&at.join("ProjectSettings/ProjectSettings.asset")) {
        return Err(unfinished(tr!("The folder at {path} is not the trial Orbiters made.", path = at.display())));
    }
    let settings = at.join("ProjectSettings/ProjectSettings.asset");
    if settings.exists() {
        let s = match std::fs::read_to_string(&settings) {
            Ok(s) => s,
            Err(e) => return Err(unfinished(tr!("The trial is listed at {path}, but its project settings could not be written: {error}", path = at.display(), error = e))),
        };
        let from = job.source.path.file_name().map(|n| n.to_string_lossy().into_owned());
        if let Err(e) = std::fs::write(&settings, crate::templates::retarget(&s, &job.trial.name, from.as_deref())) {
            return Err(unfinished(tr!("The trial is listed at {path}, but its project settings could not be written: {error}", path = at.display(), error = e)));
        }
    }
    if !crate::templates::installed_dependencies_complete(&at).unwrap_or(false) {
        return Err(unfinished(tr!("The trial is listed at {path}, but its installed packages are not the ones its lock names.", path = at.display())));
    }
    let siblings = at
        .parent()
        .and_then(|p| std::fs::read_dir(p).ok())
        .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    update(dir, id, |j| {
        j.phase = Phase::Prepared;
        j.error = None;
        j.localized = localized;
        j.siblings = siblings;
    })
}

#[cfg(test)]
thread_local! {
    /// A test's room on the trial's disk.
    static FREE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e9)
}

fn free_space(path: &Path) -> Option<u64> {
    #[cfg(test)]
    if let Some(free) = FREE.with(|f| f.get()) {
        return Some(free);
    }
    crate::size::free_space(path)
}

/// What the original's local packages from outside it take, those the trial already has left out.
fn outside_bytes(source: &Path, trial: &Path) -> u64 {
    let m: serde_json::Value = std::fs::read(source.join("Packages/manifest.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    m["dependencies"]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, v)| v.as_str()?.strip_prefix("file:").map(|p| (name, source.join("Packages").join(p))))
        .filter(|(name, _)| !trial.join("Packages").join(name).exists())
        .map(|(_, at)| walkdir::WalkDir::new(at).follow_links(true).into_iter().flatten().filter(|e| e.file_type().is_file()).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum::<u64>())
        .sum()
}

/// The names of `file:` packages in a `Packages/manifest.json` (as it was before they were made part of the copy).
fn local_names(manifest: Option<&[u8]>) -> Vec<String> {
    let m: serde_json::Value = manifest.and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default();
    m["dependencies"].as_object().into_iter().flatten().filter(|(_, v)| v.as_str().is_some_and(|s| s.starts_with("file:"))).map(|(n, _)| n.clone()).collect()
}

/// Finishes an interrupted, unlisted or unfinished trial: only the very folders recorded, and only once every file is in.
pub fn register_again(dir: &Path, vcc: &Path, id: &str, cancel: &AtomicBool, progress: &crate::templates::SharedProgress) -> anyhow::Result<Job> {
    let job = jobs(dir).into_iter().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
    anyhow::ensure!(job.trial.verified() == Some(true), "{}", tr!("The folder at {path} is not the trial Orbiters made.", path = job.trial.path.display()));
    anyhow::ensure!(job.source_verified(), "{}", tr!("The original at {path} is not the folder the trial was copied from: Orbiters does not take packages from it.", path = job.source.path.display()));
    anyhow::ensure!(crate::discovery::is_project(&job.trial.path), "{}", tr!("The copy at {path} is incomplete: start a new trial.", path = job.trial.path.display()));
    if matches!(job.phase, Phase::Copying | Phase::Planned) {
        update(dir, id, |j| j.phase = Phase::Copied)?;
    }
    finish(dir, vcc, id, cancel, progress)
}

/// Takes the copy Creator Companion made beside the trial as the trial on Unity 6, by its identity.
pub fn adopt_vcc_copy(dir: &Path, vcc: &Path, id: &str) -> anyhow::Result<Job> {
    let job = jobs(dir).into_iter().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
    anyhow::ensure!(job.trial.verified() == Some(true), "{}", tr!("The folder at {path} is not the trial Orbiters made.", path = job.trial.path.display()));
    let beside = vcc_copy(&job).ok_or_else(|| anyhow::anyhow!("{}", tr("Creator Companion's migrated copy is not there.")))?;
    let place = Place::of(&beside)?;
    crate::runtime()?.block_on(crate::vpm::register(vcc, &beside, true))?;
    update(dir, id, |j| j.migrated = Some(place.clone()))
}

/// Takes the trial off VCC's and ALCOM's list and off Orbiters' record: its files stay where they are. A folder that is
/// no longer the one recorded is not taken off the list (it is someone else's now).
pub fn unregister(dir: &Path, vcc: &Path, id: &str) -> anyhow::Result<PathBuf> {
    let job = jobs(dir).into_iter().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
    let rt = crate::runtime()?;
    for place in [Some(&job.trial), job.migrated.as_ref()].into_iter().flatten() {
        if place.verified() == Some(true) || !place.path.exists() {
            rt.block_on(crate::vpm::register(vcc, &place.path, false))?;
        }
    }
    forget(dir, id)?;
    Ok(job.unity6().path.clone())
}

/// What a launch needs: the editor's program, the project, the attempt's log; answers the process id when it has one.
pub type Start<'a> = &'a dyn Fn(&Path, &Path, &Path) -> anyhow::Result<Option<u32>>;

/// The editor a trial opens with (its version and its changeset), found and probed now.
pub type Find<'a> = &'a dyn Fn(&str, &str) -> super::editors::Found;

/// Whether a process still runs.
pub fn alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut system = System::new();
    let pid = Pid::from_u32(pid);
    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    // A number the system gave to another program since is not the editor.
    system.process(pid).is_some_and(|p| p.name().to_string_lossy().to_ascii_lowercase().starts_with("unity"))
}

/// Opens the trial in Unity 6, checked again at this moment: the very folder recorded; on exactly the reviewed tuple of
/// its target (the built-in release list's editor, changeset and SDK, from its files now); not open; no other opening
/// reserved or running; room for the first import; and an editor found and probed now as exactly that version and
/// revision (never one that cannot be verified). The attempt is reserved in the record before anything starts, with a
/// log of its own in app data.
pub fn launch(dir: &Path, id: &str, find: Find, open: &dyn Fn(&Path) -> bool, running: &dyn Fn(u32) -> bool, start: Start) -> anyhow::Result<Attempt> {
    let matrix = super::matrix::Matrix::built_in();
    let job = jobs(dir).into_iter().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
    let at = job.unity6().clone();
    anyhow::ensure!(at.verified() == Some(true), "{}", tr!("The folder at {path} is not the trial Orbiters made.", path = at.path.display()));
    let s = standing::read(&at.path, &matrix);
    anyhow::ensure!(s.state == State::OnUnity6, "{}", tr("The trial is not on Unity 6 yet: migrate it with Creator Companion first."));
    // The reviewed tuple: the built-in row of its SDK, exactly its editor and (when the file names one) its changeset.
    let row = s.sdk.version().and_then(|v| matrix.row_for(v)).filter(|r| matrix.actionable(r));
    let reviewed = row.is_some_and(|r| r.unity == s.editor.text && s.editor.revision.as_deref().is_none_or(|rev| rev == r.changeset));
    anyhow::ensure!(
        reviewed,
        "{}",
        tr!("The trial is on Unity {editor} with SDK {sdk}, which this version of Orbiters has not reviewed: open it from Unity Hub yourself.", editor = s.editor.text, sdk = s.sdk.version().unwrap_or_default())
    );
    let row = row.unwrap();
    anyhow::ensure!(!open(&at.path), "{}", tr("Unity has the trial open already."));
    let editor = match find(&s.editor.text, &row.changeset) {
        super::editors::Found::Exact(c) => c,
        _ => anyhow::bail!("{}", tr!("Unity {version} could not be verified on this computer: install it with Unity Hub, or locate a program that says it is that version.", version = s.editor.text)),
    };
    if let Some(changeset) = editor.identity.as_ref().and_then(|i| i.changeset.as_deref()) {
        anyhow::ensure!(changeset == row.changeset, "{}", tr!("{path} is Unity {version} of another build ({changeset}).", path = editor.program.display(), version = s.editor.text, changeset = changeset));
    }
    if job.attempts.is_empty() && job.import_high > 0 {
        let free = free_space(&at.path).unwrap_or(0);
        anyhow::ensure!(
            free >= job.import_high,
            "{}",
            tr!("The trial's disk has {free} GB free, and its first import may need up to {need} GB: free some space first.", free = gb(free), need = gb(job.import_high))
        );
    }
    // Reserve the attempt: its number and its log file are taken under the record's lock.
    let now = crate::discovery::now();
    let folder = logs_of(dir, &job.id);
    let attempt = transact(dir, |jobs| {
        let j = jobs.iter_mut().find(|j| j.id == id).ok_or_else(|| anyhow::anyhow!("{}", tr("This Unity 6 trial is not recorded any more.")))?;
        let busy = j.attempts.iter().any(|a| (a.reserved && now.saturating_sub(a.started) < RESERVATION_SECS) || a.pid.is_some_and(running));
        anyhow::ensure!(!busy, "{}", tr("Another opening of this trial is starting or running."));
        std::fs::create_dir_all(&folder)?;
        // A log left by an opening whose record could not be saved keeps its number.
        let mut n = j.attempts.iter().map(|a| a.n).max().unwrap_or(0) + 1;
        while log_of(&folder, n).exists() {
            n += 1;
        }
        let log = log_of(&folder, n);
        std::fs::OpenOptions::new().write(true).create_new(true).open(&log).map_err(|_| anyhow::anyhow!("{}", tr("This attempt's log exists already.")))?;
        let a = Attempt { n, started: now, log, editor: editor.program.clone(), unity: s.editor.text.clone(), start: None, pid: None, reserved: true };
        j.attempts.push(a.clone());
        Ok(a)
    })?;
    // The last look before starting: still the very folder, still that version, still closed.
    let still = at.verified() == Some(true) && standing::EditorStamp::read(&at.path) == s.editor && !open(&at.path);
    let started = if still { start(&editor.program, &at.path, &attempt.log) } else { Err(anyhow::anyhow!("{}", tr("The trial changed just before it was opened: look at it again."))) };
    let (pid, outcome) = match &started {
        Ok(pid) => (*pid, None),
        Err(e) => (None, Some(match e.downcast_ref::<crate::QuitAtOnce>() {
            Some(q) => Outcome::ExitedAtOnce(q.0.clone()),
            None => Outcome::DidNotStart(format!("{e:#}")),
        })),
    };
    let n = attempt.n;
    let job = update(dir, id, |j| {
        if let Some(a) = j.attempts.iter_mut().find(|a| a.n == n) {
            a.pid = pid;
            a.start = outcome.clone();
            a.reserved = false;
        }
        // Kept: the last few attempts; older ones go with their logs (Orbiters' own files in app data).
        while j.attempts.len() > KEEP_ATTEMPTS {
            let old = j.attempts.remove(0);
            let _ = std::fs::remove_file(log_of(&folder, old.n));
        }
    })?;
    started?;
    Ok(job.attempts.into_iter().find(|a| a.n == n).unwrap_or(attempt))
}

/// Whether an attempt's own editor still runs: its process, or (where the system gave none) Unity on the trial.
pub fn attempt_running(attempt: &Attempt, open: bool) -> bool {
    match attempt.pid {
        Some(pid) => alive(pid),
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
pub(crate) mod tests {
    use super::*;
    use crate::unity6::editors::{Arch, Candidate, Check, Found, Hint, Identity};
    use crate::unity6::standing::tests::{project, U2022, U6};
    use crate::unity6::standing::{SDK_AVATARS, SDK_BASE};

    /// A 2022 avatar project with its SDK installed.
    pub(crate) fn source(root: &Path) -> PathBuf {
        let p = root.join("Projects/My avatar");
        project(&p, Some(U2022), &[(SDK_BASE, "3.10.6"), (SDK_AVATARS, "3.10.6")], &[(SDK_BASE, "3.10.6", "2022.3"), (SDK_AVATARS, "3.10.6", "2022.3")]);
        std::fs::write(p.join("Packages/manifest.json"), r#"{"dependencies":{"com.unity.timeline":"1.7.5"}}"#).unwrap();
        std::fs::write(p.join("Packages/packages-lock.json"), r#"{"dependencies":{"com.unity.timeline":{"version":"1.7.5","source":"registry","url":"https://packages.unity.com"}}}"#).unwrap();
        std::fs::create_dir_all(p.join("Assets/Empty")).unwrap();
        std::fs::write(p.join("Assets/Avatar.unity"), "scene").unwrap();
        std::fs::write(p.join("ProjectSettings/ProjectSettings.asset"), "PlayerSettings:\n  productName: My avatar\n  productGUID: 0123\n").unwrap();
        p
    }

    pub(crate) fn target() -> Target {
        Target { sdk: "3.11.0-beta.1".into(), unity: "6000.0.67f1".into(), changeset: "78a1c2bbeb6a".into() }
    }

    pub(crate) struct Env {
        pub _temp: tempfile::TempDir,
        pub root: PathBuf,
        pub app: PathBuf,
        pub vcc: PathBuf,
        pub source: PathBuf,
    }

    pub(crate) fn env() -> Env {
        let temp = tempfile::Builder::new().prefix("unity6-trial").tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let source = source(&root);
        Env { app: root.join("app"), vcc: root.join("vcc"), source, root, _temp: temp }
    }

    fn ok(_: u64) -> anyhow::Result<u64> {
        Ok(10)
    }

    fn run_at(e: &Env, parent: &Path, name: &str, open: &dyn Fn(&Path) -> bool, budget: &dyn Fn(u64) -> anyhow::Result<u64>) -> anyhow::Result<Job> {
        let recorded = |_: &Job| {};
        prepare(Prepare { dir: &e.app, vcc: &e.vcc, source: &e.source, parent, name, target: target(), open, budget, recorded: &recorded }, &AtomicBool::new(false), &Default::default())
    }

    fn run(e: &Env) -> anyhow::Result<Job> {
        run_at(e, &e.root.join("Projects"), &trial_name("My avatar"), &|_| false, &ok)
    }

    /// The files Orbiters never writes on a trial.
    fn guarded(project: &Path) -> Vec<(String, Option<Vec<u8>>)> {
        ["Packages/vpm-manifest.json", "Packages/packages-lock.json", "Packages/manifest.json", "ProjectSettings/ProjectVersion.txt"]
            .iter()
            .map(|f| (f.to_string(), std::fs::read(project.join(f)).ok()))
            .collect()
    }

    pub(crate) fn exact(root: &Path, version: &str, changeset: Option<&str>) -> Candidate {
        let program = root.join("Editors").join(version).join("Editor/Unity.exe");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"stand-in").unwrap();
        Candidate {
            program,
            hint: Hint::HubFolder,
            check: Check::Verified,
            identity: Some(Identity { version: version.into(), changeset: changeset.map(Into::into), arch: vec![Arch::here()] }),
            modules: Default::default(),
        }
    }

    /// What Creator Companion's migration leaves in the trial (the test stands in for it: Orbiters never does this).
    pub(crate) fn vcc_migrates(trial: &Path) {
        project(trial, Some(U6), &[(SDK_BASE, "3.11.0-beta.1"), (SDK_AVATARS, "3.11.0-beta.1")], &[(SDK_BASE, "3.11.0-beta.1", "6000.0"), (SDK_AVATARS, "3.11.0-beta.1", "6000.0")]);
    }

    #[test]
    fn a_trial_is_recorded_copied_listed_and_waits_for_creator_companion() {
        let e = env();
        let before = guarded(&e.source);
        let job = run(&e).unwrap();
        assert_eq!(job.phase, Phase::Prepared);
        assert!(job.trial.id.is_some() && job.source.id.is_some(), "both bound to their identity");
        assert!(e.app.join(STORE).is_file() && e.app.join(BACKUP).is_file());
        assert!(!e.source.join(STORE).exists() && !job.trial.path.join(STORE).exists());
        let listed = crate::runtime().unwrap().block_on(crate::vpm::registered(&e.vcc)).unwrap();
        assert!(listed.iter().any(|p| crate::discovery::key(p) == crate::discovery::key(&job.trial.path)));
        assert_eq!(guarded(&job.trial.path), before);
        assert_eq!(guarded(&e.source), before, "the original untouched");
        assert_eq!(status(&job, false), Status::Waiting { vcc_copy: None });
        assert!(matches!(guard(&e.app, &job.trial.path), Guard::Blocked(_)), "nothing opens or changes it while it waits");
        assert_eq!(guard(&e.app, &e.source), Guard::Free, "the original is not a trial");
        let editor = exact(&e.root, "6000.0.67f1", None);
        assert!(launch(&e.app, &job.id, &|_, _| Found::Exact(editor.clone()), &|_| false, &|_| false, &|_, _, _| panic!("started")).is_err());
    }

    /// Verification blocker 1: every path of the flow, its refusals and its failures, against a generated original whose
    /// whole tree (every file's bytes and every folder, empty ones included) is the same before and after.
    #[test]
    fn the_original_is_untouched_on_every_path() {
        let e = env();
        let other = e.root.join("Other project");
        project(&other, Some(U2022), &[], &[]);
        let snapshot = |p: &Path| crate::unity6::tests::snapshot(p);
        let original = snapshot(&e.source);
        let projects = e.root.join("Projects");
        let check = |label: &str| assert_eq!(snapshot(&e.source), original, "{label} changed the original");
        // Refusals.
        assert!(run_at(&e, &e.source.join("Assets/New folder/Deeper"), "T", &|_| false, &ok).is_err());
        check("a destination inside the original that does not exist yet");
        assert!(!e.source.join("Assets/New folder").exists());
        assert!(run_at(&e, &e.source.join("Assets"), "T", &|_| false, &ok).is_err());
        check("a destination in the original's Assets");
        let other_before = snapshot(&other);
        assert!(run_at(&e, &other.join("Assets/Sub"), "T", &|_| false, &ok).is_err());
        assert_eq!(snapshot(&other), other_before, "inside another Unity project: refused, nothing made there");
        assert!(run_at(&e, &projects, "My avatar", &|_| false, &ok).is_err(), "the original itself");
        check("the original as destination");
        assert!(run_at(&e, Path::new("relative"), "T", &|_| false, &ok).is_err());
        assert!(run_at(&e, &projects, "T", &|_| true, &ok).is_err());
        check("the original open");
        assert!(run_at(&e, &projects, "T", &|_| false, &|_| anyhow::bail!("no room")).is_err());
        check("no room");
        // A failure while finishing: a local package whose archive name is taken in the copy.
        let ext = e.root.join("External/com.example.tool");
        std::fs::create_dir_all(&ext).unwrap();
        std::fs::write(ext.join("package.json"), r#"{"name":"com.example.tool","version":"1.0.0"}"#).unwrap();
        // The whole flow, a refusal on retry, and every later action.
        let job = run(&e).unwrap();
        check("a whole trial");
        let vcc_missing = e.root.join("vcc-file");
        std::fs::write(&vcc_missing, b"x").unwrap();
        update(&e.app, &job.id, |j| j.phase = Phase::Copied).unwrap();
        assert!(register_again(&e.app, &vcc_missing, &job.id, &AtomicBool::new(false), &Default::default()).is_err());
        check("a failed registration");
        register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).unwrap();
        check("listing again");
        let _ = adopt_vcc_copy(&e.app, &e.vcc, &job.id);
        vcc_migrates(&job.trial.path);
        let editor = exact(&e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
        let _ = launch(&e.app, &job.id, &|_, _| Found::Exact(editor.clone()), &|_| false, &|_| false, &|_, _, _| Ok(Some(1)));
        let _ = launch(&e.app, &job.id, &|_, _| Found::Missing { wrong: vec![] }, &|_| false, &|_| false, &|_, _, _| Ok(None));
        check("openings");
        let m = crate::unity6::matrix::Matrix::built_in();
        let _ = crate::unity6::look(&e.app, std::slice::from_ref(&e.source), &[]);
        let _ = crate::unity6::compat::packages(&e.source, &m, "3.11.0-beta.1");
        let _ = crate::unity6::compat::checklist(&e.source, standing::Content::Avatars, &AtomicBool::new(false));
        let _ = blockers(&e.source, &standing::read(&e.source, &m), is_open(&e.source));
        let _ = guard(&e.app, &e.source);
        check("looking at it");
        unregister(&e.app, &e.vcc, &job.id).unwrap();
        check("unregistering the trial");
        // A place Creator Companion's list cannot be written at, from the very start: kept, listed here, the original same.
        assert!(prepare(
            Prepare { dir: &e.app, vcc: &vcc_missing, source: &e.source, parent: &projects, name: "Not listed", target: target(), open: &|_| false, budget: &ok, recorded: &|_| {} },
            &AtomicBool::new(false),
            &Default::default(),
        )
        .is_err());
        check("a trial that could not be listed");
        let unlisted = jobs(&e.app).into_iter().find(|j| j.trial.name == "Not listed").unwrap();
        assert!(unlisted.trial.path.join("Assets/Avatar.unity").is_file(), "its copy is kept");
        forget(&e.app, &unlisted.id).unwrap();
        check("forgetting a trial");
        assert!(unlisted.trial.path.join("Assets/Avatar.unity").is_file(), "forgetting deletes nothing of it");
        // Creator Companion's own copy taken as the trial's.
        let second = run_at(&e, &projects, "Second", &|_| false, &ok).unwrap();
        let migrated = projects.join("Second-Migrated");
        for f in walkdir::WalkDir::new(&second.trial.path).into_iter().flatten().filter(|f| f.file_type().is_file()) {
            let to = migrated.join(f.path().strip_prefix(&second.trial.path).unwrap());
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(f.path(), to).unwrap();
        }
        vcc_migrates(&migrated);
        adopt_vcc_copy(&e.app, &e.vcc, &second.id).unwrap();
        check("taking Creator Companion's copy");
        // A damaged record: refused, then set aside.
        std::fs::write(e.app.join(STORE), "{").unwrap();
        assert!(run_at(&e, &projects, "While damaged", &|_| false, &ok).is_err());
        check("a damaged record");
        set_aside(&e.app).unwrap();
        check("setting the damaged record aside");
        assert!(guarded(&e.source).iter().all(|(_, b)| b.is_some()));
        // Stopped at once: the partial copy is removed, the original stays.
        let recorded = |_: &Job| {};
        let stopped = prepare(
            Prepare { dir: &e.app, vcc: &e.vcc, source: &e.source, parent: &projects, name: "Stopped", target: target(), open: &|_| false, budget: &ok, recorded: &recorded },
            &AtomicBool::new(true),
            &Default::default(),
        );
        assert!(stopped.is_err());
        check("a stopped copy");
        // A trial with a local package from outside: copied into the trial, the original untouched.
        std::fs::write(e.source.join("Packages/manifest.json"), format!(r#"{{"dependencies":{{"com.example.tool":"file:{}"}}}}"#, ext.display().to_string().replace('\\', "/"))).unwrap();
        let original = snapshot(&e.source);
        let external = snapshot(&ext);
        run_at(&e, &projects, "With a local package", &|_| false, &ok).unwrap();
        assert_eq!(snapshot(&ext), external, "the local package's own folder");
        // The same package gone while copying: a failure after the copy.
        let away = e.root.join("External/away");
        let recorded = |_: &Job| std::fs::rename(&ext, &away).unwrap();
        let failed = prepare(
            Prepare { dir: &e.app, vcc: &e.vcc, source: &e.source, parent: &projects, name: "Package gone", target: target(), open: &|_| false, budget: &ok, recorded: &recorded },
            &AtomicBool::new(false),
            &Default::default(),
        );
        assert!(failed.is_err());
        assert_eq!(snapshot(&e.source), original, "a failure after the copy");
        std::fs::rename(&away, &ext).unwrap();
        // A file that cannot be read in the middle of the copy.
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(e.source.join("Assets/Avatar.unity")).unwrap();
            assert!(run_at(&e, &projects, "Unreadable file", &|_| false, &ok).is_err());
            drop(held);
            assert_eq!(snapshot(&e.source), original, "a copy that failed in the middle");
        }
        // A junction into the original as the trial's place, and one into it on the way there.
        #[cfg(windows)]
        {
            let link = e.root.join("link-into-original");
            junction::create(e.source.join("Assets"), &link).unwrap();
            assert!(run_at(&e, &link, "T", &|_| false, &ok).is_err());
            assert!(run_at(&e, &link.join("Deeper"), "T", &|_| false, &ok).is_err());
            junction::delete(&link).unwrap();
            let _ = std::fs::remove_dir(&link);
            assert_eq!(snapshot(&e.source), original, "a place reached through a junction into the original");
        }
        // A copy asked for directly (Duplicate): the same refusal, nothing made in the original.
        let cancel = AtomicBool::new(false);
        let copy = |to: PathBuf| crate::templates::copy(&e.source, &to, &cancel, &Default::default());
        assert!(copy(e.source.join("Assets/Made/Copy")).is_err());
        assert!(copy(other.join("Assets/Made/Copy")).is_err());
        assert_eq!(snapshot(&e.source), original, "Duplicate into the original");
        assert!(!other.join("Assets/Made").exists());
        // A local archive whose name is taken by another in Packages: said before anything is copied.
        std::fs::write(e.root.join("External/tool.tgz"), b"one").unwrap();
        std::fs::write(e.source.join("Packages/com.example.tool.tgz"), b"another").unwrap();
        std::fs::write(e.source.join("Packages/manifest.json"), format!(r#"{{"dependencies":{{"com.example.tool":"file:{}"}}}}"#, e.root.join("External/tool.tgz").display().to_string().replace('\\', "/"))).unwrap();
        let original = snapshot(&e.source);
        assert!(format!("{:#}", run_at(&e, &projects, "Archive taken", &|_| false, &ok).unwrap_err()).contains("com.example.tool"));
        assert!(!projects.join("Archive taken").exists());
        assert_eq!(snapshot(&e.source), original, "a local archive whose name is taken");
    }

    /// Verification blocker 2: a failure making the trial independent leaves it unfinished (a job with its error), never
    /// waiting for Creator Companion; listing it again checks what is there before taking it.
    #[test]
    fn an_unfinished_trial_is_never_handed_to_creator_companion() {
        let e = env();
        let ext = e.root.join("External/com.example.tool");
        std::fs::create_dir_all(&ext).unwrap();
        std::fs::write(ext.join("package.json"), r#"{"name":"com.example.tool","version":"1.0.0"}"#).unwrap();
        std::fs::write(e.source.join("Packages/manifest.json"), format!(r#"{{"dependencies":{{"com.example.tool":"file:{}"}}}}"#, ext.display().to_string().replace('\\', "/"))).unwrap();
        // The package goes away between the checks and the copy: listed, not independent, its error kept.
        let away = e.root.join("External/away");
        let recorded = |_: &Job| std::fs::rename(&ext, &away).unwrap();
        let failed = prepare(
            Prepare { dir: &e.app, vcc: &e.vcc, source: &e.source, parent: &e.root.join("Projects"), name: &trial_name("My avatar"), target: target(), open: &|_| false, budget: &ok, recorded: &recorded },
            &AtomicBool::new(false),
            &Default::default(),
        );
        assert!(failed.is_err());
        let j = jobs(&e.app).into_iter().next().unwrap();
        assert_eq!((j.phase, status(&j, false)), (Phase::Registered, Status::Unfinished));
        assert!(j.error.as_deref().unwrap_or_default().contains("com.example.tool"), "{:?}", j.error);
        assert!(matches!(guard(&e.app, &j.trial.path), Guard::Blocked(_)));
        // Put back: no room for it yet, then finished.
        std::fs::rename(&away, &ext).unwrap();
        FREE.with(|f| f.set(Some(1)));
        assert!(register_again(&e.app, &e.vcc, &j.id, &AtomicBool::new(false), &Default::default()).is_err());
        assert_eq!(status(&jobs(&e.app)[0], false), Status::Unfinished, "the trial is kept, unfinished, when its disk is full");
        FREE.with(|f| f.set(None));
        let job = register_again(&e.app, &e.vcc, &j.id, &AtomicBool::new(false), &Default::default()).unwrap();
        assert_eq!(job.localized, vec!["com.example.tool".to_string()], "said, not hidden");
        // Its copy of the package changed behind Orbiters' back, then the trial is finished again.
        let copy = job.trial.path.join("Packages/com.example.tool");
        std::fs::write(copy.join("extra.txt"), b"not the package").unwrap();
        update(&e.app, &job.id, |j| j.phase = Phase::Copied).unwrap();
        assert!(register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).is_err());
        let j = jobs(&e.app).into_iter().next().unwrap();
        assert_eq!(j.phase, Phase::Registered);
        assert!(j.error.as_deref().unwrap_or_default().contains("com.example.tool"), "{:?}", j.error);
        assert_eq!(status(&j, false), Status::Unfinished);
        assert!(status(&j, false).is_job(), "shown in the gallery as a job, with its error");
        assert!(matches!(guard(&e.app, &j.trial.path), Guard::Blocked(_)));
        std::fs::remove_file(copy.join("extra.txt")).unwrap();
        assert_eq!(register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).unwrap().phase, Phase::Prepared);
    }

    #[test]
    fn nothing_is_recorded_when_the_source_is_open_or_the_disk_is_short() {
        let e = env();
        assert!(run_at(&e, &e.root.join("Projects"), "T", &|_| true, &ok).is_err());
        assert!(run_at(&e, &e.root.join("Projects"), "T", &|_| false, &|need| anyhow::bail!("{need} bytes do not fit")).is_err());
        assert!(jobs(&e.app).is_empty(), "nothing recorded");
        assert!(!e.root.join("Projects/T").exists(), "nothing copied");
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
    fn a_project_not_on_2022_or_with_an_unsupported_layout_is_blocked() {
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
        // Verification major 3: a shadowing folder, nested local references, an unreadable lock.
        let ext = e.root.join("External/my.local");
        std::fs::create_dir_all(&ext).unwrap();
        std::fs::write(ext.join("package.json"), r#"{"name":"my.local","dependencies":{"my.other":"file:../my.other"}}"#).unwrap();
        std::fs::write(e.source.join("Packages/manifest.json"), format!(r#"{{"dependencies":{{"my.local":"file:{}"}}}}"#, ext.display().to_string().replace('\\', "/"))).unwrap();
        std::fs::write(e.source.join("Packages/packages-lock.json"), "{ not json").unwrap();
        let b = blockers(&e.source, &standing::read(&e.source, &m), false);
        let unsupported = b.iter().find_map(|x| if let Blocker::Unsupported(v) = x { Some(v.clone()) } else { None }).unwrap();
        assert!(unsupported.contains(&"my.local".to_string()) && unsupported.contains(&"Packages/packages-lock.json".to_string()), "{unsupported:?}");
        std::fs::write(ext.join("package.json"), r#"{"name":"my.local"}"#).unwrap();
        std::fs::create_dir_all(e.source.join("Packages/my.local")).unwrap();
        let b = blockers(&e.source, &standing::read(&e.source, &m), false);
        assert!(b.iter().any(|x| matches!(x, Blocker::Unsupported(v) if v.contains(&"my.local".to_string()))), "a folder shadowing it: {b:?}");
    }

    /// B4/M5: every point where preparing can stop leaves an entry the gallery shows, with what can be done.
    #[test]
    fn every_interruption_leaves_a_visible_actionable_entry() {
        let e = env();
        let job = run(&e).unwrap();
        let trial = job.trial.path.clone();
        let rewind = |phase: Phase| update(&e.app, &job.id, |j| j.phase = phase).unwrap();
        let marker = trial.join(crate::templates::INCOMPLETE);
        std::fs::write(&marker, serde_json::to_vec(&crate::templates::Marker { copy: "x".into(), destination: trial.clone(), started: 0, folder: job.trial.id }).unwrap()).unwrap();
        let j = rewind(Phase::Copying);
        assert_eq!(status(&j, false), Status::Interrupted { complete: false });
        assert!(register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).is_err(), "an incomplete copy is not listed");
        std::fs::remove_file(&marker).unwrap();
        assert_eq!(status(&j, false), Status::Interrupted { complete: true });
        assert_eq!(register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).unwrap().phase, Phase::Prepared);
        let blocked_vcc = e.root.join("vcc-blocked");
        std::fs::write(&blocked_vcc, b"not a folder").unwrap();
        rewind(Phase::Copied);
        let err = register_again(&e.app, &blocked_vcc, &job.id, &AtomicBool::new(false), &Default::default()).unwrap_err();
        assert!(format!("{err:#}").contains(&trial.display().to_string()), "{err:#}");
        let j = jobs(&e.app).into_iter().next().unwrap();
        assert_eq!((j.phase, status(&j, false)), (Phase::RegistrationFailed, Status::RegistrationFailed));
        assert_eq!(register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).unwrap().phase, Phase::Prepared);
        let away = Job { id: uuid::Uuid::new_v4().simple().to_string(), trial: Place { path: PathBuf::from(if cfg!(windows) { r"Q:\Detached\My avatar (Unity 6)" } else { "/Volumes/Detached/My avatar (Unity 6)" }), ..j.trial.clone() }, ..j.clone() };
        insert(&e.app, &away).unwrap();
        let reread = jobs(&e.app).into_iter().find(|x| x.id == away.id).unwrap();
        assert_eq!(status(&reread, false), Status::Missing);
        forget(&e.app, &away.id).unwrap();
        assert!(trial.join("ProjectSettings/ProjectVersion.txt").is_file(), "forgetting deletes nothing");
    }

    /// Verification blocker 5: a damaged record is never written over; its last good copy is shown and keeps the
    /// trials' protections; it is set aside only on request; an unsupported format counts as damaged.
    #[test]
    fn a_damaged_record_keeps_its_trials_and_is_never_written_over() {
        let e = env();
        let job = run(&e).unwrap();
        let good = std::fs::read(e.app.join(STORE)).unwrap();
        for bad in [&good[..good.len() / 2], br#"{"schema":9,"jobs":[]}"#.as_slice(), b"".as_slice()] {
            std::fs::write(e.app.join(STORE), bad).unwrap();
            let r = record(&e.app);
            assert!(matches!(r.health, Health::Damaged { backup: true, .. }), "{:?}", r.health);
            assert_eq!(r.jobs.iter().map(|j| j.id.clone()).collect::<Vec<_>>(), vec![job.id.clone()], "the last good copy is shown");
            assert!(matches!(guard(&e.app, &job.trial.path), Guard::Blocked(_)), "still protected");
            assert!(forget(&e.app, "00000000000000000000000000000000").is_err(), "no write over a damaged record");
            assert_eq!(std::fs::read(e.app.join(STORE)).unwrap(), bad);
        }
        // Nothing readable at all: a project the damaged text names is still treated as a trial.
        std::fs::remove_file(e.app.join(BACKUP)).unwrap();
        std::fs::write(e.app.join(STORE), &good[..good.len() - 3]).unwrap();
        assert!(record(&e.app).jobs.is_empty());
        assert!(matches!(guard(&e.app, &job.trial.path), Guard::Blocked(_)));
        // Set aside on request: kept beside it, never deleted.
        let aside = set_aside(&e.app).unwrap();
        assert!(aside.is_file() && record(&e.app).health == Health::Fine);
        // A forged record naming a folder outside app data for its logs is refused as damaged.
        let mut forged: serde_json::Value = serde_json::from_slice(&good).unwrap();
        forged["jobs"][0]["id"] = "../../outside".into();
        std::fs::write(e.app.join(STORE), forged.to_string()).unwrap();
        assert!(matches!(record(&e.app).health, Health::Damaged { .. }));
    }

    /// Verification blocker 5: writers in two threads (each its own handle, as two processes) lose nothing.
    #[test]
    fn concurrent_writers_lose_no_update() {
        let e = env();
        let job = run(&e).unwrap();
        let threads: Vec<_> = (0..4)
            .map(|t| {
                let (app, base) = (e.app.clone(), job.clone());
                std::thread::spawn(move || {
                    for i in 0..10 {
                        let j = Job { id: format!("{:0>32x}", t * 100 + i + 1), trial: Place { path: base.trial.path.with_file_name(format!("T{t}-{i}")), ..base.trial.clone() }, ..base.clone() };
                        insert(&app, &j).unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(jobs(&e.app).len(), 41);
    }

    #[test]
    fn the_record_is_bound_to_identity_and_a_copied_marker_is_inert() {
        let e = env();
        let job = run(&e).unwrap();
        let planted = Job { id: "f".repeat(32), trial: Place { path: e.root.join("Projects/Elsewhere"), id: None, name: "Elsewhere".into() }, ..job.clone() };
        std::fs::write(job.trial.path.join(STORE), serde_json::to_vec(&Store { schema: 1, jobs: vec![planted] }).unwrap()).unwrap();
        assert_eq!(jobs(&e.app), vec![job.clone()], "only app data is the record");
        let moved = e.root.join("Projects/Moved");
        std::fs::rename(&job.trial.path, &moved).unwrap();
        project(&job.trial.path, Some(U6), &[(SDK_BASE, "3.11.0-beta.1")], &[(SDK_BASE, "3.11.0-beta.1", "6000.0")]);
        assert_eq!(status(&job, false), Status::Replaced);
        assert_eq!(guard(&e.app, &job.trial.path), Guard::Free, "another folder there now is not the trial");
        let editor = exact(&e.root, "6000.0.67f1", None);
        assert!(launch(&e.app, &job.id, &|_, _| Found::Exact(editor.clone()), &|_| false, &|_| false, &|_, _, _| panic!("started on another folder")).is_err());
        assert!(register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).is_err());
    }

    /// Verification major 2/10: a source replaced at its path is not the original any more.
    #[test]
    fn a_replaced_original_is_not_read_again() {
        let e = env();
        let job = run(&e).unwrap();
        update(&e.app, &job.id, |j| j.phase = Phase::Copied).unwrap();
        std::fs::rename(&e.source, e.root.join("Projects/Old")).unwrap();
        source(&e.root);
        assert!(!jobs(&e.app)[0].source_verified());
        let err = register_again(&e.app, &e.vcc, &job.id, &AtomicBool::new(false), &Default::default()).unwrap_err();
        assert!(format!("{err:#}").contains("not the folder the trial was copied from"), "{err:#}");
    }

    /// Verification blocker 4, major 5 and 7: the editor is found and probed at the launch, exactly the reviewed tuple
    /// and build, one opening at a time, each with its own log; a reservation left by a crash expires.
    #[test]
    fn each_launch_is_reserved_verified_and_logged() {
        let e = env();
        let job = run(&e).unwrap();
        vcc_migrates(&job.trial.path);
        assert!(matches!(guard(&e.app, &job.trial.path), Guard::Unity6(_)));
        let after_vcc = guarded(&job.trial.path);
        let good = exact(&e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
        let launch_with = |found: Found, start: Start| launch(&e.app, &job.id, &move |_, _| found.clone(), &|_| false, &|_| false, start);
        let started = |_: &Path, _: &Path, _: &Path| -> anyhow::Result<Option<u32>> { Ok(Some(4242)) };
        // Not verified now, another build, or none: refused before anything is reserved.
        let unverified = Candidate { check: Check::Unverified, identity: None, hint: Hint::Located, ..good.clone() };
        assert!(launch_with(Found::Unverified(unverified), &started).is_err());
        let other_build = exact(&e.root, "6000.0.67f1", Some("61dfb374e36f"));
        assert!(launch_with(Found::Exact(other_build), &started).is_err());
        assert!(jobs(&e.app)[0].attempts.is_empty());
        // The first one is reserved and bound to its process; a second while it runs is refused.
        let a1 = launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| true, &started).unwrap();
        assert_eq!((a1.pid, a1.reserved), (Some(4242), false));
        assert!(a1.log.starts_with(e.app.join(LOGS)));
        assert!(launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|pid| pid == 4242, &started).is_err());
        // A reservation a crash left (no process recorded) blocks for a while, then not.
        update(&e.app, &job.id, |j| {
            j.attempts[0].pid = None;
            j.attempts[0].reserved = true;
        })
        .unwrap();
        assert!(launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &started).is_err());
        update(&e.app, &job.id, |j| j.attempts[0].started = 0).unwrap();
        let a2 = launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &started).unwrap();
        assert_ne!(a1.log, a2.log);
        // Typed failures.
        let quit = |_: &Path, _: &Path, _: &Path| -> anyhow::Result<Option<u32>> { Err(anyhow::Error::new(crate::QuitAtOnce(Some("exit code: 3".into()), "quit".into()))) };
        assert!(launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &quit).is_err());
        let spawn = |_: &Path, _: &Path, _: &Path| -> anyhow::Result<Option<u32>> { anyhow::bail!("no such program") };
        assert!(launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &spawn).is_err());
        let j = jobs(&e.app).into_iter().next().unwrap();
        assert!(matches!(attempt_outcome(&j.attempts[2], false), Outcome::ExitedAtOnce(_)));
        assert!(matches!(attempt_outcome(&j.attempts[3], false), Outcome::DidNotStart(_)));
        // Kept: the last few, older logs removed with them.
        for _ in 0..KEEP_ATTEMPTS {
            launch(&e.app, &job.id, &|_, _| Found::Exact(good.clone()), &|_| false, &|_| false, &|_, _, _| Ok(None)).unwrap();
        }
        let j = jobs(&e.app).into_iter().next().unwrap();
        assert_eq!(j.attempts.len(), KEEP_ATTEMPTS);
        assert!(!a1.log.exists());
        assert_eq!(guarded(&job.trial.path), after_vcc, "no write by Orbiters on the trial's files");
    }

    /// Verification major 7: a coherent Unity 6 project of a tuple this version of Orbiters has not reviewed is shown, never
    /// opened through the trial's launcher.
    #[test]
    fn an_unreviewed_tuple_is_never_launched() {
        let e = env();
        let job = run(&e).unwrap();
        project(&job.trial.path, Some("m_EditorVersion: 6000.1.1f1\n"), &[(SDK_BASE, "4.0.0")], &[(SDK_BASE, "4.0.0", "6000.1")]);
        assert_eq!(status(&job, false), Status::OnUnity6, "shown as what its files say");
        let editor = exact(&e.root, "6000.1.1f1", None);
        let err = launch(&e.app, &job.id, &|_, _| Found::Exact(editor.clone()), &|_| false, &|_| false, &|_, _, _| panic!("unreviewed")).unwrap_err();
        assert!(format!("{err:#}").contains("not reviewed"), "{err:#}");
    }

    /// Verification major 1: a folder of Creator Companion's name that was there before the hand-over is never taken.
    #[test]
    fn a_sibling_that_was_there_before_is_never_taken_for_creator_companions_copy() {
        let e = env();
        let beside = e.root.join("Projects/My avatar (Unity 6)-Migrated");
        project(&beside, Some(U6), &[(SDK_BASE, "3.11.0-beta.1")], &[(SDK_BASE, "3.11.0-beta.1", "6000.0")]);
        let job = run(&e).unwrap();
        assert_eq!(status(&job, false), Status::Waiting { vcc_copy: None });
        assert!(adopt_vcc_copy(&e.app, &e.vcc, &job.id).is_err());
    }

    #[test]
    fn creator_companions_own_migrated_copy_can_be_taken_as_the_trial() {
        let e = env();
        let job = run(&e).unwrap();
        let beside = job.trial.path.with_file_name("My avatar (Unity 6)-Migrated");
        project(&beside, Some(U6), &[(SDK_BASE, "3.11.0-beta.1")], &[(SDK_BASE, "3.11.0-beta.1", "6000.0")]);
        assert_eq!(status(&job, false), Status::Waiting { vcc_copy: Some(beside.clone()) });
        let j = adopt_vcc_copy(&e.app, &e.vcc, &job.id).unwrap();
        assert_eq!(j.unity6().path, beside);
        assert_eq!(status(&j, false), Status::OnUnity6);
        assert!(matches!(guard(&e.app, &job.trial.path), Guard::Blocked(_)), "the first copy keeps its guard");
        assert!(matches!(guard(&e.app, &beside), Guard::Unity6(_)));
        let left = unregister(&e.app, &e.vcc, &job.id).unwrap();
        assert_eq!(left, beside);
        assert!(beside.is_dir() && job.trial.path.is_dir() && jobs(&e.app).is_empty());
    }

    #[test]
    fn the_record_survives_a_restart() {
        let e = env();
        let job = run(&e).unwrap();
        let again: Store = serde_json::from_slice(&std::fs::read(e.app.join(STORE)).unwrap()).unwrap();
        assert_eq!(again.jobs, vec![job]);
        assert_eq!(record(&e.app).health, Health::Fine);
    }
    /// Verification blocker 3: every library entry point refuses a trial that waits for Creator Companion, whoever calls
    /// it (the page, the bridge, the Toolkit's update): package changes, the N4N0 setup request and Open.
    #[test]
    fn the_library_refuses_to_change_or_open_a_waiting_trial() {
        let mut e = env();
        e.app = crate::data_dir();
        let job = run(&e).unwrap();
        let rt = crate::runtime().unwrap();
        let changed = rt.block_on(crate::vpm::change(&e.vcc, &job.trial.path, crate::vpm::Change::Resolve));
        assert!(format!("{:#}", changed.unwrap_err()).contains("Creator Companion"));
        let avatar = crate::avatars::Avatar { id: "avtr_00000000-0000-4000-8000-000000000001".into(), ..Default::default() };
        let request = crate::setup::request(avatar, 1, "https://orbiters.cc/api/", "0.0.0").unwrap();
        assert!(crate::setup::write(&job.trial.path, &request).is_err());
        assert!(!job.trial.path.join("UserSettings/Orbiters").exists(), "nothing written in it");
        let project = crate::discovery::Project { path: job.trial.path.clone(), name: job.trial.name.clone(), unity: "2022.3.22f1".into(), ..Default::default() };
        assert!(format!("{:#}", crate::open_project(&project).unwrap_err()).contains("Creator Companion"));
        // On Unity 6: what ends in the SDK's build panel (ready to upload) is still refused, with the reason.
        vcc_migrates(&job.trial.path);
        assert!(matches!(guard(&e.app, &job.trial.path), Guard::Unity6(_)));
        assert!(format!("{:#}", crate::setup::write(&job.trial.path, &request).unwrap_err()).contains("uploads from Unity 2022"));
        assert!(!job.trial.path.join("UserSettings/Orbiters").exists(), "nothing written in it");
        forget(&e.app, &job.id).unwrap();
    }

    /// Verification blocker 1: the original checked again when the copy starts and once it ends.
    #[test]
    fn a_change_of_the_original_during_the_copy_fails_the_trial() {
        let e = env();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        // Closed for the checks, opened by the time the copy ended.
        let open = |_: &Path| calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 2;
        let err = run_at(&e, &e.root.join("Projects"), "T", &open, &ok).unwrap_err();
        assert!(format!("{err:#}").contains("changed while it was copied"), "{err:#}");
        let j = jobs(&e.app).into_iter().next().unwrap();
        assert_eq!(j.phase, Phase::Failed);
        assert_eq!(status(&j, false), Status::Failed { kept: true }, "kept and shown, never handed over");
    }

    /// Verification blocker 1: an edit in place while copying (which leaves every folder's time as it was) is seen too.
    #[test]
    fn an_edit_in_place_during_the_copy_is_seen() {
        let e = env();
        let recorded = |_: &Job| std::fs::write(e.source.join("Assets/Avatar.unity"), "another scene").unwrap();
        let failed = prepare(
            Prepare { dir: &e.app, vcc: &e.vcc, source: &e.source, parent: &e.root.join("Projects"), name: "T", target: target(), open: &|_| false, budget: &ok, recorded: &recorded },
            &AtomicBool::new(false),
            &Default::default(),
        );
        assert!(format!("{:#}", failed.unwrap_err()).contains("changed while it was copied"));
        assert_eq!(status(&jobs(&e.app)[0], false), Status::Failed { kept: true });
    }

    /// Verification blocker 3: the guard follows the folder, not its spelling: through a junction, or moved since.
    #[test]
    fn the_guard_follows_the_folder_not_its_path() {
        let e = env();
        let job = run(&e).unwrap();
        #[cfg(windows)]
        {
            let link = e.root.join("alias");
            junction::create(&job.trial.path, &link).unwrap();
            assert!(matches!(guard(&e.app, &link), Guard::Blocked(_)), "through a junction");
            junction::delete(&link).unwrap();
            let _ = std::fs::remove_dir(&link);
        }
        let moved = e.root.join("Projects/Renamed");
        std::fs::rename(&job.trial.path, &moved).unwrap();
        assert!(matches!(guard(&e.app, &moved), Guard::Blocked(_)), "renamed since");
    }

    /// Verification blocker 5: a log path written in the record by hand is not followed, and only a recorded trial's
    /// logs are ever removed.
    #[test]
    fn a_recorded_log_path_is_never_followed() {
        let e = env();
        let job = run(&e).unwrap();
        let outside = e.root.join("outside.log");
        std::fs::write(&outside, "Entering Safe Mode").unwrap();
        update(&e.app, &job.id, |j| j.attempts.push(Attempt { n: 1, started: 1, log: outside.clone(), editor: PathBuf::from("x"), unity: "6000.0.67f1".into(), start: None, pid: None, reserved: false })).unwrap();
        let j = jobs(&e.app).into_iter().next().unwrap();
        assert_eq!(j.attempts[0].log, e.app.join(LOGS).join(&job.id).join("attempt-1.log"));
        assert!(matches!(attempt_outcome(&j.attempts[0], false), Outcome::Undetermined { .. }));
        let bystander = e.app.join("keep");
        std::fs::create_dir_all(&bystander).unwrap();
        forget(&e.app, "../keep").unwrap();
        assert!(bystander.is_dir(), "an id nobody recorded removes nothing");
    }

    /// Verification blocker 4: what changed between the checks and the start stops the opening, and is recorded so.
    #[test]
    fn a_trial_swapped_just_before_the_start_is_not_opened() {
        let e = env();
        let job = run(&e).unwrap();
        vcc_migrates(&job.trial.path);
        let good = exact(&e.root, "6000.0.67f1", Some("78a1c2bbeb6a"));
        // The folder is put aside while the editor is looked for.
        let find = |_: &str, _: &str| {
            std::fs::write(job.trial.path.join("ProjectSettings/ProjectVersion.txt"), U2022).unwrap();
            Found::Exact(good.clone())
        };
        assert!(launch(&e.app, &job.id, &find, &|_| false, &|_| false, &|_, _, _| panic!("started on another version")).is_err());
        assert!(matches!(jobs(&e.app)[0].attempts[0].start, Some(Outcome::DidNotStart(_))));
        assert!(!jobs(&e.app)[0].attempts[0].reserved, "the reservation is given back");
    }

    /// Verification major 10: a trial is paired with its original only while that folder is still the one recorded.
    #[test]
    fn a_trial_is_paired_only_with_its_very_original() {
        let e = env();
        run(&e).unwrap();
        assert_eq!(crate::unity6::look(&e.app, std::slice::from_ref(&e.source), &[]).pairs.len(), 1);
        std::fs::rename(&e.source, e.root.join("Projects/Old")).unwrap();
        source(&e.root);
        assert!(crate::unity6::look(&e.app, std::slice::from_ref(&e.source), &[]).pairs.is_empty());
    }

    /// Verification major 5: an attempt runs while its own editor does: not another program that was given its number
    /// since, and Unity on the trial counts only where the system gave no number.
    #[test]
    fn an_attempt_runs_while_its_own_editor_does() {
        let attempt = |pid| Attempt { n: 1, started: 1, log: PathBuf::from("x"), editor: PathBuf::from("x"), unity: "6000.0.67f1".into(), start: None, pid, reserved: false };
        let not_unity = attempt(Some(std::process::id()));
        assert!(!attempt_running(&not_unity, true), "this process is no editor");
        assert!(attempt_running(&attempt(None), true) && !attempt_running(&attempt(None), false));
    }
}
