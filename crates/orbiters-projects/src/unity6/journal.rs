//! The Unity 6 trials' record, in Orbiters' app data: an append-only journal. Every change is one whole job appended as a
//! line with its own checksum, under a lock held across processes; nothing is ever rewritten. A line that cannot be read
//! (torn, altered, of a newer version) hides only itself: every other line still counts, the jobs it may be about are
//! uncertain (nothing is done with them) until the person sets it aside, and the line itself always stays in the file.
//!
//! A [`Lease`] is held for the whole of one operation on a job (making, finishing, opening, forgetting it), so that two
//! windows, or a window and the service, never work on one job at once. Both locks are the system's: they end with the
//! process that holds them, so none is ever left behind by one that stopped.

use super::gate::{AppRoot, Lock};
use super::outcome::Outcome;
use crate::owned::{self, FileId};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const JOURNAL: &str = "unity6-trials.journal";
const LOCK: &str = "unity6-trials.lock";
const LEASES: &str = "unity6-leases";
/// Where each job's attempt logs and verified manifest are, in app data.
pub const LOGS: &str = "unity6-logs";
/// The journal's format. A line of a newer one is kept, shown as such and never acted on.
const VERSION: u32 = 1;

/// A folder as recorded: its path, its identity on its disk once it exists, its name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub path: PathBuf,
    pub id: Option<FileId>,
    pub name: String,
}

impl Place {
    /// `path` with its identity, which must be readable.
    pub fn of(path: &Path) -> anyhow::Result<Self> {
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
    /// Recorded; its folder is not made yet.
    Planned,
    /// Its folder exists and files are going in.
    Copying,
    /// The copy failed, was stopped, or is not what the original holds.
    Failed,
    /// Every byte compared with the original's; not on VCC's and ALCOM's list yet.
    Verified,
    /// Verified and listed: Creator Companion's migration comes next.
    Listed,
    /// The person took it off; kept so that its folder and its content IDs are still known. `whole`: it was verified.
    Abandoned { whole: bool },
}

/// The editor and SDK the trial is for (a built-in row of the release list).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub sdk: String,
    pub unity: String,
    pub changeset: String,
}

/// The very process an attempt started: a number alone is given again by the system.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    /// When it started, as the system says (seconds).
    pub started: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub n: u32,
    pub started: u64,
    /// Its log, always at its own place in app data (never read from the journal).
    #[serde(skip)]
    pub log: PathBuf,
    pub editor: PathBuf,
    pub unity: String,
    /// What was known at the start: it could not start, or quit at once.
    #[serde(default)]
    pub start: Option<Outcome>,
    /// The editor's process, when the system gave it.
    #[serde(default)]
    pub process: Option<Process>,
    /// Reserved before starting, not confirmed yet (a crash between the two leaves it so).
    #[serde(default)]
    pub reserved: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    // First in its line, so that a torn one still says which job it was about.
    pub id: String,
    pub created: u64,
    pub source: Place,
    pub trial: Place,
    pub phase: Phase,
    pub target: Target,
    #[serde(default)]
    pub error: Option<String>,
    /// What differs from the original, when a comparison found it.
    #[serde(default)]
    pub differences: Vec<String>,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    /// The copy Creator Companion made itself (`<trial>-Migrated`) when its migration made one and the person took it.
    #[serde(default)]
    pub migrated: Option<Place>,
    /// The folders beside the trial when it was handed over (`None`: they could not all be read, so nothing is offered as
    /// Creator Companion's copy by itself).
    #[serde(default)]
    pub beside: Option<Vec<FileId>>,
    /// Local (`file:`) packages made part of the trial, whose `Packages/manifest.json` entries name its own copies.
    #[serde(default)]
    pub localized: Vec<String>,
    /// Links of the original whose contents were copied as ordinary files.
    #[serde(default)]
    pub linked: Vec<String>,
    /// The content IDs (avatars, worlds) the original had: uploading one from Unity 6 ends its uploads from Unity 2022.
    #[serde(default)]
    pub content: Vec<String>,
    /// The trial's own product GUID, so that what its settings must hold is known.
    #[serde(default)]
    pub guid: String,
    /// The SHA-256 of its verified manifest, kept beside its logs.
    #[serde(default)]
    pub manifest: Option<String>,
    /// The high end of the first import's estimate.
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

    /// Whether its original is still the folder recorded (lineage needs a positive match).
    pub fn source_verified(&self) -> bool {
        self.source.verified() == Some(true)
    }

    /// Its folder of logs in app data, relative to it.
    pub fn logs(&self) -> PathBuf {
        Path::new(LOGS).join(&self.id)
    }

    fn validate(&self) -> Result<(), String> {
        let places = [Some(&self.source), Some(&self.trial), self.migrated.as_ref()];
        let absolute = places.iter().flatten().all(|p| p.path.is_absolute() && !p.path.components().any(|c| matches!(c, std::path::Component::ParentDir)));
        let (s, t) = (crate::discovery::key(&self.source.path), crate::discovery::key(&self.trial.path));
        let disjoint = s != t && !t.starts_with(&format!("{s}\\")) && !s.starts_with(&format!("{t}\\"));
        if !is_id(&self.id) {
            return Err(tr!("a trial's id is not one Orbiters makes ({id})", id = self.id));
        }
        if !absolute || !disjoint {
            return Err(tr!("the folders of the trial {id} are not two separate absolute folders", id = self.id));
        }
        Ok(())
    }
}

fn is_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// One line.
#[derive(Serialize, Deserialize)]
struct Line {
    v: u32,
    /// CRC-32 of `body`'s text.
    crc: String,
    body: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Body {
    Put(Box<Job>),
    /// Unreadable lines the person set aside, by their SHA-256.
    Aside(Vec<String>),
}

/// A line that cannot be read and was not set aside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unread {
    pub line: usize,
    pub why: String,
    /// Written by a newer version of Orbiters (which wrote it on purpose), rather than damaged.
    pub newer: bool,
    /// The job it names, when that much of it is readable.
    pub about: Option<String>,
    text: String,
    sha: String,
}

/// Whether the record can be trusted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    Fine,
    /// Some lines cannot be read; `newer`: a newer version of Orbiters wrote them.
    Damaged { lines: usize, why: String, newer: bool },
    /// The journal itself cannot be read at all.
    Unreadable(String),
}

/// The record as read.
#[derive(Clone, Debug)]
pub struct Record {
    pub jobs: Vec<Job>,
    pub unread: Vec<Unread>,
    /// The journal could not be read at all.
    pub error: Option<String>,
    /// Where each job's last line is.
    at: HashMap<String, usize>,
}

impl Record {
    pub fn health(&self) -> Health {
        if let Some(e) = &self.error {
            return Health::Unreadable(e.clone());
        }
        match self.unread.first() {
            None => Health::Fine,
            Some(u) => Health::Damaged { lines: self.unread.len(), why: u.why.clone(), newer: self.unread.iter().any(|u| u.newer) },
        }
    }

    /// Why nothing is done with the job `id` now: a later line that may be about it cannot be read.
    pub fn uncertain(&self, id: &str) -> Option<String> {
        let last = self.at.get(id).copied().unwrap_or(0);
        self.unread.iter().find(|u| u.line > last && u.about.as_deref().is_none_or(|a| a == id)).map(|u| u.why.clone())
    }

    /// Why nothing is known of the folder at `path`, when an unreadable line names it.
    pub fn named(&self, path: &Path) -> Option<String> {
        let key = crate::discovery::key(path);
        let squeeze = |text: &str| text.to_lowercase().replace('/', "\\").replace("\\\\", "\\").replace("\\\\", "\\");
        self.unread.iter().find(|u| squeeze(&u.text).contains(&key)).map(|u| u.why.clone())
    }

    pub fn job(&self, id: &str) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |mut crc, b| {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
        crc
    })
}

pub fn sha256(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The job id a line that cannot be read still names.
fn about(text: &str) -> Option<String> {
    // Inside the body's own quotes the field is escaped.
    ["\"id\":\"", "\\\"id\\\":\\\""].iter().find_map(|field| {
        let rest = &text[text.find(field)? + field.len()..];
        let id = rest.get(..32)?;
        is_id(id).then(|| id.to_string())
    })
}

/// The record in `dir` (app data): missing is an empty record.
pub fn read(dir: &Path) -> Record {
    let mut record = Record { jobs: vec![], unread: vec![], error: None, at: HashMap::new() };
    let bytes = match AppRoot::existing(dir).and_then(|app| app.read(Path::new(JOURNAL))) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return record,
        Err(e) => {
            record.error = Some(e.to_string());
            return record;
        }
    };
    let mut aside: Vec<String> = vec![];
    for (i, raw) in bytes.split(|b| *b == b'\n').enumerate() {
        if raw.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let line = i + 1;
        let mut unread = |why: String, newer: bool| {
            let text = String::from_utf8_lossy(raw).into_owned();
            record.unread.push(Unread { line, why, newer, about: about(&text), text, sha: sha256(raw) });
        };
        // Its version first: what a newer one holds besides is not known here.
        let v = serde_json::from_slice::<serde_json::Value>(raw).ok().and_then(|l| l["v"].as_u64()).unwrap_or(0);
        if v > u64::from(VERSION) {
            unread(tr!("it is of format {schema}, which this version of Orbiters does not read", schema = v), true);
            continue;
        }
        let parsed: Line = match serde_json::from_slice(raw) {
            Ok(l) => l,
            Err(e) => {
                unread(tr!("it is not a record Orbiters can read ({error})", error = e), false);
                continue;
            }
        };
        if parsed.crc != format!("{:08x}", crc32(parsed.body.as_bytes())) {
            unread(tr("its checksum does not match what it says").into(), false);
            continue;
        }
        match serde_json::from_str::<Body>(&parsed.body) {
            Ok(Body::Put(job)) => match job.validate() {
                Ok(()) => {
                    record.at.insert(job.id.clone(), line);
                    record.jobs.retain(|j| j.id != job.id);
                    record.jobs.push(*job);
                }
                Err(why) => unread(why, false),
            },
            Ok(Body::Aside(lines)) => aside.extend(lines),
            Err(e) => unread(tr!("it is not a record Orbiters can read ({error})", error = e), false),
        }
    }
    record.unread.retain(|u| !aside.contains(&u.sha));
    for j in &mut record.jobs {
        let folder = dir.join(j.logs());
        for a in &mut j.attempts {
            a.log = folder.join(log_name(a.n));
        }
    }
    record
}

pub fn log_name(n: u32) -> String {
    format!("attempt-{n}.log")
}

/// Every trial recorded in `dir` (app data), the abandoned ones included.
pub fn jobs(dir: &Path) -> Vec<Job> {
    read(dir).jobs
}

fn lock(app: &AppRoot) -> anyhow::Result<Lock> {
    Ok(app.lock(Path::new(LOCK))?)
}

fn append(app: &AppRoot, body: &Body) -> anyhow::Result<()> {
    let body = serde_json::to_string(body)?;
    let line = serde_json::to_string(&Line { v: VERSION, crc: format!("{:08x}", crc32(body.as_bytes())), body })?;
    let torn = match app.read(Path::new(JOURNAL)) {
        Ok(bytes) => bytes.last().is_some_and(|b| *b != b'\n'),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    // A line cut by a crash stays as it is, on a line of its own.
    let text = format!("{}{line}\n", if torn { "\n" } else { "" });
    Ok(app.append(Path::new(JOURNAL), text.as_bytes())?)
}

/// One read-decide-append on the journal, its lock held throughout. `f` changes the job `id` (or refuses).
pub fn update<R>(dir: &Path, id: &str, f: impl FnOnce(&mut Job) -> anyhow::Result<R>) -> anyhow::Result<(Job, R)> {
    let app = AppRoot::open(dir)?;
    let _locked = lock(&app)?;
    let record = read(dir);
    if let Some(e) = &record.error {
        anyhow::bail!("{}", tr!("The Unity 6 trials record cannot be read ({why}): Orbiters changes nothing.", why = e));
    }
    if let Some(why) = record.uncertain(id) {
        anyhow::bail!("{}", uncertain_text(&why));
    }
    let mut job = record.job(id).cloned().ok_or_else(|| anyhow::anyhow!("{}", gone_text()))?;
    let out = f(&mut job)?;
    job.validate().map_err(|e| anyhow::anyhow!(e))?;
    append(&app, &Body::Put(Box::new(job.clone())))?;
    Ok((job, out))
}

/// [`update`] that always goes through.
pub fn set(dir: &Path, id: &str, f: impl FnOnce(&mut Job)) -> anyhow::Result<Job> {
    Ok(update(dir, id, |j| {
        f(j);
        Ok(())
    })?
    .0)
}

/// Records a new job.
pub fn insert(dir: &Path, job: &Job) -> anyhow::Result<()> {
    job.validate().map_err(|e| anyhow::anyhow!(e))?;
    let app = AppRoot::open(dir)?;
    let _locked = lock(&app)?;
    let record = read(dir);
    if let Some(e) = &record.error {
        anyhow::bail!("{}", tr!("The Unity 6 trials record cannot be read ({why}): Orbiters changes nothing.", why = e));
    }
    anyhow::ensure!(record.job(&job.id).is_none(), "{}", tr!("a trial's id is not one Orbiters makes ({id})", id = job.id));
    append(&app, &Body::Put(Box::new(job.clone())))
}

pub fn gone_text() -> &'static str {
    tr("This Unity 6 trial is not recorded any more.")
}

pub fn uncertain_text(why: &str) -> String {
    tr!("Part of the Unity 6 trials record cannot be read ({why}): nothing is done with this trial until that part is set aside in the Unity projects tab.", why = why)
}

/// Sets the lines that cannot be read aside: they are written to a file of their own beside the journal, and stay in the
/// journal too, where they are not asked about any more. Lines of a newer version are never set aside.
pub fn set_aside(dir: &Path) -> anyhow::Result<PathBuf> {
    let app = AppRoot::open(dir)?;
    let _locked = lock(&app)?;
    let record = read(dir);
    let lines: Vec<&Unread> = record.unread.iter().filter(|u| !u.newer).collect();
    anyhow::ensure!(!lines.is_empty(), "{}", tr("No part of the Unity 6 trials record can be set aside."));
    let name = format!("unity6-trials.unreadable-{}-{}.txt", crate::discovery::now(), uuid::Uuid::new_v4().simple());
    let text: String = lines.iter().map(|u| format!("{}\n", u.text)).collect();
    app.write_new(Path::new(&name), text.as_bytes())?;
    append(&app, &Body::Aside(lines.iter().map(|u| u.sha.clone()).collect()))?;
    Ok(dir.join(name))
}

/// One operation on a job, held across this computer's processes until dropped.
pub struct Lease(#[allow(dead_code)] Lock);

impl Lease {
    fn name(id: &str) -> anyhow::Result<PathBuf> {
        anyhow::ensure!(is_id(id), "{}", tr!("a trial's id is not one Orbiters makes ({id})", id = id));
        Ok(Path::new(LEASES).join(format!("{id}.lock")))
    }

    /// The job's lease, when nobody works on it.
    pub fn take(dir: &Path, id: &str) -> anyhow::Result<Lease> {
        let app = AppRoot::open(dir)?;
        match app.try_lock(&Self::name(id)?)? {
            Some(lock) => Ok(Lease(lock)),
            None => anyhow::bail!("{}", tr("Orbiters is already working on this trial, in this window or another one. Wait for that to end.")),
        }
    }

    /// Whether somebody works on the job now.
    pub fn held(dir: &Path, id: &str) -> bool {
        let Ok(name) = Self::name(id) else { return false };
        // Nothing was ever leased: nothing is made to ask.
        if !dir.join(&name).exists() {
            return false;
        }
        AppRoot::existing(dir).and_then(|app| app.try_lock(&name)).map_or(true, |l| l.is_none())
    }
}

#[cfg(test)]
pub(crate) mod tests;
