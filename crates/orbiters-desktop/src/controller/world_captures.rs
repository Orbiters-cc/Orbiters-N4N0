//! World captures from Unity (contracts' Docs/world-captures.md): My World, in a world's Unity project, leaves requests
//! in `<local data>/Orbiters/World captures/requests/`. The tray service imports the raw captures it rendered as
//! sessions of that world and queues their reconstruction, shares or unshares a session publicly (the same jobs as
//! Photos & captures), and keeps `<world id>.json` current (the world's sessions, their state and upload, the answers)
//! for the worlds asked about in the last 10 minutes.

use super::shares::{self, Entry, Local, Operation};
use super::Shared;
use crate::state::{AccountState, CaptureState, Snapshot};
use orbiters_core::account::Server;
use orbiters_core::bundle::{self, SessionSummary};
use orbiters_core::explicit::Content;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SCHEMA: u32 = 1;
/// A world's file is kept current this long after its last request.
const WATCH_FOR: Duration = Duration::from_secs(600);
const ANSWERS_KEPT: usize = 20;
const TICK: Duration = Duration::from_secs(2);

/// `<local data>/Orbiters/World captures`.
pub fn root() -> PathBuf {
    orbiters_core::paths::shared_with_unity_root().join("World captures")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    schema: u32,
    id: String,
    #[serde(default)]
    action: String,
    #[serde(default)]
    world: RequestWorld,
    #[serde(default)]
    capture: Option<PathBuf>,
    #[serde(default)]
    session: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct RequestWorld {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    id: String,
    action: String,
    status: &'static str,
    message: String,
    session: Option<PathBuf>,
    finished_utc: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    schema: u32,
    world_id: String,
    updated_utc: String,
    app: String,
    signed_in: bool,
    development: bool,
    captures: Vec<Capture>,
    answers: Vec<Answer>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Capture {
    session: PathBuf,
    name: String,
    created_utc: String,
    source: String,
    state: &'static str,
    progress: Option<f32>,
    splats: Option<usize>,
    preview: Option<PathBuf>,
    explicit: bool,
    shared: Option<Upload>,
    busy: Option<&'static str>,
}

#[derive(Debug, PartialEq, Serialize)]
struct Upload {
    id: u64,
    public: bool,
    official: bool,
    slot: Option<u32>,
}

/// An import My World asked for, from its start until its session is saved.
struct Import {
    request: String,
    world: String,
    file: PathBuf,
    /// The raw capture's frames, for the progress.
    frames: u32,
    /// Ticks the capture was idle without the session being saved (a start that failed).
    idle: u32,
}

/// What the Stopped event and the requests' thread share.
#[derive(Default)]
pub struct WorldCapturesRuntime {
    importing: Option<Import>,
    /// The last answers of each world, newest first.
    answers: HashMap<String, VecDeque<Answer>>,
}

impl WorldCapturesRuntime {
    fn answer(&mut self, world: &str, answer: Answer, file: Option<&Path>) {
        if let Some(file) = file {
            let _ = std::fs::remove_file(file);
        }
        let list = self.answers.entry(world.to_ascii_lowercase()).or_default();
        list.push_front(answer);
        list.truncate(ANSWERS_KEPT);
    }
}

fn answer(id: &str, action: &str, done: bool, message: impl Into<String>, session: Option<PathBuf>) -> Answer {
    Answer {
        id: id.to_string(),
        action: action.to_string(),
        status: if done { "done" } else { "failed" },
        message: message.into(),
        session,
        finished_utc: orbiters_core::now_utc_iso(),
    }
}

fn is_world_id(id: &str) -> bool {
    id.len() <= 64 && id.starts_with("wrld_") && id[5..].chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// The world a session was recorded in, lowercased.
fn world_of(s: &SessionSummary) -> Option<String> {
    s.manifest.world.as_ref().and_then(|w| w.id.as_deref()).map(str::to_ascii_lowercase)
}

/// A share or unshare this thread started: its request, to answer once the job is done.
struct Job {
    world: String,
    action: &'static str,
    session: PathBuf,
    file: PathBuf,
}

/// The requests' thread: every [`TICK`], while the app runs.
pub(super) fn spawn(ctl: Shared) {
    let _ = std::thread::Builder::new().name("world-captures".into()).spawn(move || {
        let mut taken: HashSet<String> = HashSet::new();
        let mut imports: VecDeque<(Request, PathBuf)> = VecDeque::new();
        let mut jobs: HashMap<String, Job> = HashMap::new();
        let mut watched: HashMap<String, Instant> = HashMap::new();
        let mut written: HashMap<String, String> = HashMap::new();
        while !ctl.quitting.load(std::sync::atomic::Ordering::Relaxed) {
            let requests = root().join("requests");
            for (file, request) in read_requests(&requests) {
                let id = match &request {
                    Ok(r) => r.id.clone(),
                    Err(_) => file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                };
                if !taken.insert(id.clone()) {
                    continue;
                }
                match request {
                    Ok(r) => {
                        watched.insert(r.world.id.to_ascii_lowercase(), Instant::now());
                        take(&ctl, r, file, &mut imports, &mut jobs);
                    }
                    Err(message) => {
                        eprintln!("world captures: {}: {message}", file.display());
                        let _ = std::fs::remove_file(&file);
                    }
                }
            }
            step_import(&ctl, &mut imports);
            finish_jobs(&ctl, &mut jobs);
            watched.retain(|_, at| at.elapsed() < WATCH_FOR);
            if !watched.is_empty() {
                let snapshot = ctl.view();
                let sessions = bundle::list_sessions(&snapshot.settings.sessions_dir);
                let uploads = shares::uploads(Server::current());
                for world in watched.keys() {
                    let status = status(&ctl, &snapshot, world, &sessions, &uploads, &jobs);
                    write_status(world, status, &mut written);
                }
            }
            // A request whose file is gone (answered, or taken back) may come again under the same id: only ids still
            // on disk stay taken.
            let on_disk: HashSet<String> = read_requests(&requests)
                .into_iter()
                .filter_map(|(file, r)| r.ok().map(|r| r.id).or_else(|| file.file_stem().map(|s| s.to_string_lossy().into_owned())))
                .collect();
            taken.retain(|id| on_disk.contains(id));
            std::thread::sleep(TICK);
        }
    });
}

/// The requests waiting, oldest first; `Err` for a file that is not a request of this schema.
fn read_requests(dir: &Path) -> Vec<(PathBuf, Result<Request, String>)> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| (std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p))
        .collect();
    found.sort();
    found
        .into_iter()
        .map(|(_, p)| {
            let read = std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| serde_json::from_slice::<Request>(&b).map_err(|e| e.to_string()));
            let checked = read.and_then(|r| if r.schema == SCHEMA { Ok(r) } else { Err(format!("schema {} is not {SCHEMA}", r.schema)) });
            (p, checked)
        })
        .collect()
}

/// Takes one request: answers the ones that cannot be done at once, queues imports, starts shares.
fn take(ctl: &Shared, r: Request, file: PathBuf, imports: &mut VecDeque<(Request, PathBuf)>, jobs: &mut HashMap<String, Job>) {
    let world = r.world.id.to_ascii_lowercase();
    let fail = |message: String| {
        ctl.world_captures.lock().unwrap().answer(&world, answer(&r.id, &r.action, false, message, r.session.clone()), Some(&file));
    };
    if !is_world_id(&world) {
        return fail("The request names no VRChat world: upload the world once, then publish its captures.".into());
    }
    match r.action.as_str() {
        "watch" => {
            let _ = std::fs::remove_file(&file);
        }
        "import" => {
            let Some(capture) = r.capture.clone() else { return fail("The request names no raw capture.".into()) };
            match raw_capture(&capture) {
                Some((id, _)) if id == world => imports.push_back((r, file)),
                Some(_) => fail(format!("{} is a capture of another world.", capture.display())),
                None => fail(format!("{} is not a raw capture of this world.", capture.display())),
            }
        }
        "share" | "unshare" => {
            let sessions_dir = ctl.settings().sessions_dir;
            let Some(dir) = r.session.clone() else { return fail("The request names no session.".into()) };
            let Some(summary) = bundle::list_sessions(&sessions_dir).into_iter().find(|s| same_path(&s.dir, &dir)) else {
                return fail(format!("{} is not a session in Orbiters - N4N0's sessions folder.", dir.display()));
            };
            if world_of(&summary).as_deref() != Some(world.as_str()) {
                return fail("This session was recorded in another world.".into());
            }
            let server = Server::current();
            let remote = shares::uploads(server).into_iter().find(|(p, _)| same_path(p, &summary.dir)).map(|(_, item)| item);
            let share = r.action == "share";
            if share && summary.reconstruction.is_none() {
                return fail("Its 3D view is not made yet: share it once its reconstruction is done.".into());
            }
            if !share && remote.is_none() {
                return fail("This capture was never shared.".into());
            }
            let entry = session_entry(&summary, &world, &r.world.name, remote);
            if share && entry.content.explicit {
                return fail("This capture is marked explicit: share it from Orbiters - N4N0's Photos & captures, which asks you to confirm.".into());
            }
            let operation = if share {
                Operation::Share(orbiters_core::shares::Share { public: true, ..Default::default() })
            } else {
                Operation::Unshare(orbiters_core::shares::Unshare { public: true, ..Default::default() })
            };
            jobs.insert(r.id.clone(), Job { world: world.clone(), action: if share { "share" } else { "unshare" }, session: summary.dir.clone(), file });
            ctl.capture_share(shares::Request { id: r.id, server, entry, extra: Vec::new(), operation });
        }
        other => fail(format!("Orbiters - N4N0 does not know the action \"{other}\": update it.")),
    }
}

/// The session a share request acts on, as Photos & captures lists it.
fn session_entry(s: &SessionSummary, world: &str, world_name: &str, remote: Option<orbiters_core::shares::Item>) -> Entry {
    Entry {
        key: s.dir.to_string_lossy().into(),
        world_id: world.to_string(),
        world_name: s.manifest.world.as_ref().and_then(|w| w.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| world_name.to_string()),
        taken: shares::unix(&s.manifest.created_utc),
        instance: s.manifest.world.as_ref().and_then(|w| w.instance.clone()),
        source_key: s.dir.file_name().map(|n| n.to_string_lossy().into_owned()),
        people: Vec::new(),
        local: Some(Local::Session(s.dir.clone())),
        remote,
        content: Content::read(&s.dir),
        ready: s.reconstruction.is_some(),
        no_keyframes: s.has_no_keyframes(),
        no_depth: s.manifest.stats.depth_frames == 0,
        trimmed: s.trimmed,
        result: s.reconstruction.as_ref().map(|r| r.created_utc.clone()),
        picture: None,
        size: None,
        layers: Vec::new(),
        edits: None,
        uploaded_edits: None,
    }
}

/// The world id (lowercased) a raw capture's `capture.json` names (contracts' Docs/raw-capture.md), and its frames.
fn raw_capture(dir: &Path) -> Option<(String, u32)> {
    let text = std::fs::read_to_string(dir.join(orbiters_contracts::raw_capture::RAW_FILE)).ok()?;
    let meta: orbiters_contracts::raw_capture::RawCapture = serde_json::from_str(&text).ok()?;
    (meta.format == orbiters_contracts::raw_capture::RAW_FORMAT).then_some(())?;
    let id = meta.world?.id.map(|id| id.to_ascii_lowercase()).filter(|id| is_world_id(id))?;
    Some((id, meta.frame_count))
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Starts the next import once no capture runs; answers one whose start failed.
fn step_import(ctl: &Shared, imports: &mut VecDeque<(Request, PathBuf)>) {
    let state = ctl.view().capture.state;
    let mut rt = ctl.world_captures.lock().unwrap();
    if let Some(import) = rt.importing.as_mut() {
        // Saved, the session is answered by the Stopped event (`world_import_saved`); idle twice without that, it
        // never started.
        import.idle = if state == CaptureState::Idle { import.idle + 1 } else { 0 };
        if import.idle >= 2 {
            let import = rt.importing.take().unwrap();
            let error = ctl.view().capture.error.clone().unwrap_or_else(|| "The import stopped before its session was saved.".into());
            rt.answer(&import.world, answer(&import.request, "import", false, error, None), Some(&import.file));
        }
        return;
    }
    if state != CaptureState::Idle {
        return;
    }
    let Some((r, file)) = imports.pop_front() else { return };
    let capture = r.capture.clone().unwrap_or_default();
    let frames = raw_capture(&capture).map_or(0, |(_, frames)| frames);
    // Known before it starts: a short import can be saved before `import_raw_capture` returns.
    rt.importing = Some(Import { request: r.id, world: r.world.id.to_ascii_lowercase(), file, frames, idle: 0 });
    drop(rt);
    if let Err(e) = ctl.import_raw_capture(capture) {
        let mut rt = ctl.world_captures.lock().unwrap();
        if let Some(import) = rt.importing.take() {
            rt.answer(&import.world, answer(&import.request, "import", false, e.to_string(), None), Some(&import.file));
        }
    }
}

/// Answers the shares and unshares whose job is done.
fn finish_jobs(ctl: &Shared, jobs: &mut HashMap<String, Job>) {
    let snapshot = ctl.view();
    jobs.retain(|id, job| {
        let Some(outcome) = snapshot.capture_shares.iter().find(|j| &j.id == id).and_then(|j| j.outcome.clone()) else {
            // Not in the list: refused as a duplicate, or trimmed before this tick saw it.
            return snapshot.capture_shares.iter().any(|j| &j.id == id);
        };
        let (done, message) = match outcome {
            shares::Outcome::Failed { explicit: true, .. } => {
                (false, "This capture is marked explicit: share it from Orbiters - N4N0's Photos & captures, which asks you to confirm.".to_string())
            }
            shares::Outcome::Failed { message, .. } => (false, message),
            _ if job.action == "share" => (true, "Shared publicly.".to_string()),
            _ => (true, "Taken off the public listing.".to_string()),
        };
        ctl.world_captures.lock().unwrap().answer(&job.world, answer(id, job.action, done, message, Some(job.session.clone())), Some(&job.file));
        false
    });
}

/// The world's file as it should read now.
fn status(
    ctl: &Shared,
    snapshot: &Snapshot,
    world: &str,
    sessions: &[SessionSummary],
    uploads: &[(PathBuf, orbiters_core::shares::Item)],
    jobs: &HashMap<String, Job>,
) -> Status {
    let frames = ctl.world_captures.lock().unwrap().importing.as_ref().map_or(0, |i| i.frames);
    let captures = sessions
        .iter()
        .filter(|s| world_of(s).as_deref() == Some(world))
        .map(|s| {
            let dir = &s.dir;
            let running = snapshot.reconstruction.as_ref().filter(|j| same_path(&j.session_dir, dir));
            let replaying = snapshot.capture.state == CaptureState::Importing && snapshot.capture.session_dir.as_deref().is_some_and(|d| same_path(d, dir));
            let (state, progress) = if replaying {
                let read = snapshot.capture.stats.as_ref().map(|st| st.frames as f32).filter(|_| frames > 0);
                ("importing", read.map(|n| (n / frames as f32).min(1.0)))
            } else if let Some(job) = running {
                ("reconstructing", Some(if job.iterations > 0 { job.iteration as f32 / job.iterations as f32 } else { 0.0 }))
            } else if snapshot.reconstruct_queue.iter().any(|d| same_path(d, dir)) {
                ("waiting", None)
            } else if s.reconstruction.is_some() {
                ("ready", None)
            } else {
                ("captured", None)
            };
            // With splat edits, only a picture drawn for them (orbiters_reconstruct::edit::picture_state): never the
            // preview as trained nor the recording's thumbnail of what they removed.
            let picture = orbiters_reconstruct::edit::picture_state(&orbiters_core::reconstruction::result_dir(dir));
            let preview = [(dir.join("reconstruction").join("preview.qoi"), picture.preview_ok()), (dir.join("thumbnail.qoi"), picture.thumbnail_ok())]
                .into_iter()
                .find(|(p, ok)| *ok && p.is_file())
                .map(|(p, _)| p);
            let shared = uploads.iter().find(|(p, _)| same_path(p, dir)).map(|(_, item)| Upload {
                id: item.id,
                public: item.public,
                official: item.official,
                slot: item.capture.as_ref().and_then(|c| c.vrchat_slot),
            });
            let key = dir.to_string_lossy();
            let busy = jobs.values().find(|j| same_path(&j.session, dir)).map(|j| if j.action == "share" { "sharing" } else { "unsharing" }).or_else(|| {
                snapshot.capture_shares.iter().any(|j| j.outcome.is_none() && j.key == key).then_some("sharing")
            });
            Capture {
                session: dir.clone(),
                name: dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                created_utc: s.manifest.created_utc.clone(),
                source: s.manifest.capture.source.clone(),
                state,
                progress,
                splats: s.reconstruction.as_ref().map(|r| r.gaussians),
                preview,
                explicit: Content::read(dir).explicit,
                shared,
                busy,
            }
        })
        .collect();
    Status {
        schema: SCHEMA,
        world_id: world.to_string(),
        updated_utc: String::new(),
        app: format!("Orbiters - N4N0 {}", orbiters_core::APP_VERSION),
        signed_in: matches!(snapshot.account.state, AccountState::SignedIn { .. }),
        development: Server::current() == Server::Development,
        captures,
        answers: ctl.world_captures.lock().unwrap().answers.get(world).map(|a| a.iter().cloned().collect()).unwrap_or_default(),
    }
}

/// Writes the world's file when it changed (its time aside).
fn write_status(world: &str, mut status: Status, written: &mut HashMap<String, String>) {
    let Ok(same) = serde_json::to_string(&status) else { return };
    if written.get(world) == Some(&same) {
        return;
    }
    status.updated_utc = orbiters_core::now_utc_iso();
    let Ok(bytes) = serde_json::to_vec_pretty(&status) else { return };
    let dir = root();
    let _ = std::fs::create_dir_all(&dir);
    match orbiters_core::write_atomic(&dir.join(format!("{world}.json")), &bytes) {
        Ok(()) => {
            written.insert(world.to_string(), same);
        }
        Err(e) => eprintln!("world captures: {world}.json: {e}"),
    }
}

impl super::Controller {
    /// A recording was saved: an import My World asked for goes on to its reconstruction, and is answered.
    pub(super) fn world_import_saved(self: &Shared, session: Option<PathBuf>) {
        let Some(import) = self.world_captures.lock().unwrap().importing.take() else { return };
        let answered = match session {
            Some(dir) => match self.queue_reconstruction(dir.clone()) {
                Ok(false) => answer(&import.request, "import", true, "Imported: it is being reconstructed.", Some(dir)),
                Ok(true) => answer(&import.request, "import", true, format!("Imported: its reconstruction starts {}.", self.reconstruction_waits_for()), Some(dir)),
                Err(e) => answer(&import.request, "import", false, format!("Imported, not reconstructed: {e}"), Some(dir)),
            },
            None => answer(&import.request, "import", false, "The import was not saved.", None),
        };
        self.world_captures.lock().unwrap().answer(&import.world, answered, Some(&import.file));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_read_oldest_first_and_other_schemas_refused() {
        let dir = std::env::temp_dir().join(format!("orbiters-world-requests-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let world = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";
        std::fs::write(dir.join("a.json"), format!(r#"{{"schema":1,"id":"a","action":"watch","world":{{"id":"{world}","name":"W"}}}}"#)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(dir.join("b.json"), r#"{"schema":2,"id":"b","action":"watch"}"#).unwrap();
        std::fs::write(dir.join("c.json.tmp"), "half a file").unwrap();
        let read = read_requests(&dir);
        assert_eq!(read.len(), 2, "a .tmp file is never read");
        let first = read[0].1.as_ref().unwrap();
        assert_eq!((first.id.as_str(), first.action.as_str(), first.world.id.as_str()), ("a", "watch", world));
        assert!(read[1].1.as_ref().is_err_and(|e| e.contains("schema 2")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn world_ids_and_raw_captures_of_a_world() {
        assert!(is_world_id("wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd"));
        assert!(!is_world_id("wrld_../../x"));
        assert!(!is_world_id("avtr_4432ea9b"));
        let dir = std::env::temp_dir().join(format!("orbiters-world-raw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(raw_capture(&dir), None, "no capture.json");
        let json = |id: &str| {
            format!(
                r#"{{"format":"orbiters.rawcapture","version":1,"source":"unity-simulator","created_utc":"2026-10-29T10:00:00Z","scene":null,"world":{{"id":{id},"name":"W","author_name":null,"instance":null}},"width":64,"height":36,"fps":30.0,"frame_count":0,"depth_every":4,"post_profile":null,"has_ground_truth":true}}"#
            )
        };
        std::fs::write(dir.join("capture.json"), json("\"wrld_4432EA9B-729c-46e3-8eaf-846aa0a37fdd\"")).unwrap();
        assert_eq!(raw_capture(&dir), Some(("wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd".to_string(), 0)));
        std::fs::write(dir.join("capture.json"), json("null")).unwrap();
        assert_eq!(raw_capture(&dir), None, "a capture that names no world");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn answers_keep_the_last_twenty_newest_first_and_delete_their_request() {
        let mut rt = WorldCapturesRuntime::default();
        let file = std::env::temp_dir().join(format!("orbiters-world-answer-{}.json", std::process::id()));
        std::fs::write(&file, "{}").unwrap();
        for i in 0..25 {
            rt.answer("WRLD_A", answer(&i.to_string(), "watch", true, "", None), (i == 0).then_some(file.as_path()));
        }
        assert!(!file.exists());
        let list = &rt.answers["wrld_a"];
        assert_eq!(list.len(), ANSWERS_KEPT);
        assert_eq!(list[0].id, "24");
    }

    #[test]
    fn a_world_file_is_written_only_when_it_changed() {
        let world = format!("wrld_00000000-0000-0000-0000-{:012}", std::process::id());
        let path = root().join(format!("{world}.json"));
        let mut written = HashMap::new();
        let make = |signed_in| Status {
            schema: SCHEMA,
            world_id: world.clone(),
            updated_utc: String::new(),
            app: "test".into(),
            signed_in,
            development: false,
            captures: Vec::new(),
            answers: Vec::new(),
        };
        write_status(&world, make(false), &mut written);
        let first = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(30));
        write_status(&world, make(false), &mut written);
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), first, "the same content is not written again");
        write_status(&world, make(true), &mut written);
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!((v["schema"].as_u64(), v["signedIn"].as_bool(), v["worldId"].as_str()), (Some(1), Some(true), Some(world.as_str())));
        assert!(!v["updatedUtc"].as_str().unwrap().is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_world_file_lists_the_worlds_sessions_with_their_state_and_upload() {
        let root = std::env::temp_dir().join(format!("orbiters-world-sessions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let world = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";
        let session = |name: &str, id: &str, source: &str| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let mut m = orbiters_contracts::manifest::SessionManifest::new(name.into(), "2026-10-29T10:00:00Z".into(), "test");
            m.world = Some(orbiters_contracts::manifest::WorldInfo { id: Some(id.into()), name: Some("Viewer".into()), ..Default::default() });
            m.capture.source = source.into();
            std::fs::write(dir.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
            dir
        };
        let ready = session("ready", world, "raw_sequence");
        std::fs::create_dir_all(ready.join("reconstruction")).unwrap();
        let report = orbiters_core::reconstruction::Report { gaussians: 812345, ..Default::default() };
        std::fs::write(ready.join("reconstruction").join("report.json"), serde_json::to_vec(&report).unwrap()).unwrap();
        std::fs::write(ready.join("reconstruction").join("preview.qoi"), b"qoif").unwrap();
        // World ids compare without their case.
        let waiting = session("waiting", &world.to_ascii_uppercase().replacen("WRLD", "wrld", 1), "spout");
        session("other", "wrld_00000000-0000-0000-0000-000000000000", "spout");
        let ctl = super::super::Controller::new(orbiters_core::settings::Settings::default());
        let mut snapshot = Snapshot::new(orbiters_core::settings::Settings::default());
        snapshot.reconstruct_queue = vec![waiting.clone()];
        let sessions = bundle::list_sessions(&root);
        let item = orbiters_core::shares::Item {
            id: 5,
            public: true,
            official: true,
            capture: Some(orbiters_core::shares::Capture { vrchat_slot: Some(17), ..Default::default() }),
            ..Default::default()
        };
        let uploads = vec![(ready.clone(), item)];
        let mut jobs = HashMap::new();
        jobs.insert("j".to_string(), Job { world: world.into(), action: "unshare", session: ready.clone(), file: root.join("j.json") });
        let s = status(&ctl, &snapshot, world, &sessions, &uploads, &jobs);
        assert_eq!(s.captures.len(), 2, "only this world's sessions");
        let r = s.captures.iter().find(|c| c.name == "ready").unwrap();
        assert_eq!((r.state, r.splats, r.busy, r.source.as_str()), ("ready", Some(812345), Some("unsharing"), "raw_sequence"));
        assert_eq!(r.preview.as_deref(), Some(ready.join("reconstruction").join("preview.qoi").as_path()));
        assert_eq!(r.shared, Some(Upload { id: 5, public: true, official: true, slot: Some(17) }));
        let w = s.captures.iter().find(|c| c.name == "waiting").unwrap();
        assert_eq!((w.state, w.shared.is_none(), w.preview.is_none(), w.busy), ("waiting", true, true, None));
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["worldId"], world);
        assert_eq!(json["captures"][0]["createdUtc"], "2026-10-29T10:00:00Z");
        assert!(json["captures"][0]["shared"].is_object() || json["captures"][0]["shared"].is_null());
        std::fs::remove_dir_all(&root).ok();
    }
}
