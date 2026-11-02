//! Background jobs for the gallery and its sharing dialogs. No disk or HTTP work in the painting code.
use orbiters_core::{
    account::{self, Api, Server},
    explicit::Content,
    maps::{Part, Uploaded},
    shares::{self, Item, Kind},
    visited,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::Instant;

/// The sharing contract keeps the member's world captures wanted while they are in VRChat. Screenshots do not
/// start the controller, so they never send presence. Failed calls back off; a world change is sent on the next tick.
pub(super) fn spawn_presence(ctl: super::Shared) {
    std::thread::spawn(move || {
        let mut last = None;
        let mut sent = std::time::Instant::now() - std::time::Duration::from_secs(300);
        let mut attempted = sent;
        let mut failed = false;
        while !ctl.quitting.load(std::sync::atomic::Ordering::Relaxed) {
            let snapshot = ctl.view();
            let world = &snapshot.vrchat.world;
            let location = world
                .world_id
                .as_ref()
                .zip(world.instance.as_ref())
                .zip(world.local_player.as_ref())
                .filter(|_| snapshot.vrchat.running && world.in_world)
                .map(|((w, i), n)| (snapshot.settings.development, w.clone(), i.clone(), n.clone(), format!("{:?}", snapshot.account.state)));
            if let Some(ref location) = location {
                if (last.as_ref() != Some(location) || sent.elapsed().as_secs() >= 300) && (!failed || attempted.elapsed().as_secs() >= 60) {
                    attempted = std::time::Instant::now();
                    failed = true;
                    if let Some((api, token)) = ctl.account_api() {
                        if api.capture_presence(&token, &location.1, &location.2, &location.3).is_ok() {
                            failed = false;
                            last = Some(location.clone());
                            sent = std::time::Instant::now();
                        }
                    }
                }
            } else {
                last = None;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Local {
    Session(PathBuf),
    Picture(PathBuf),
}
impl Local {
    pub fn path(&self) -> &Path {
        match self {
            Self::Session(p) | Self::Picture(p) => p,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub key: String,
    pub world_id: String,
    pub world_name: String,
    pub taken: u64,
    pub instance: Option<String>,
    pub source_key: Option<String>,
    pub people: Vec<String>,
    pub local: Option<Local>,
    pub remote: Option<Item>,
    pub content: Content,
    pub ready: bool,
    #[serde(skip)]
    pub picture: Option<crate::worlds::Picture>,
    /// The picture's size in pixels when the listing knows it (a screenshot's header, a capture's frame, what Orbiters
    /// says): its card has its shape before its thumbnail is decoded.
    #[serde(default)]
    pub size: Option<[u32; 2]>,
    /// A VRChat multi-layer picture's layers, back to front ([`Entry::layers`]).
    #[serde(default)]
    pub layers: Vec<Layer>,
    /// A reconstructed capture's splat edits now (orbiters_reconstruct::edit::generation), or why they cannot be read;
    /// none for a picture or a capture without its 3D view. Read with the entries ([`local_entries`]), again by its open
    /// window.
    #[serde(default)]
    pub edits: Option<Result<String, String>>,
    /// The splat edits its upload from this computer was made with (its upload record; `unedited` for an upload of
    /// before edits existed); none without a record.
    #[serde(default)]
    pub uploaded_edits: Option<String>,
}

pub use crate::worlds::Layer;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Operation {
    Share(shares::Share),
    Unshare(shares::Unshare),
    Download,
    Answer { id: u64, approve: bool, acknowledge_explicit: bool },
    /// Update the shared capture: an uploaded capture edited since gets its files on Orbiters replaced by what its 3D
    /// view shows now (contracts Docs/capture-sharing.md, Updating a capture), whoever sees it (publicly, friends, the
    /// VRChat world) sees only that from then on. [`update`].
    Update,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub server: Server,
    pub entry: Entry,
    pub extra: Vec<Entry>,
    pub operation: Operation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Outcome {
    Shared(shares::Shared),
    Unshared(Item),
    Downloaded(PathBuf),
    Answered(shares::ShareRequest),
    /// Its files on Orbiters replaced ([`Operation::Update`]): its item at the new revision and the splat edits they were
    /// made with.
    Updated { item: Item, edits: String },
    Failed { message: String, explicit: bool },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShareJob {
    pub id: String,
    pub key: String,
    /// Where it stands while it runs.
    pub stage: Stage,
    pub outcome: Option<Outcome>,
    /// It updates a shared capture's files ([`Operation::Update`]): its card says so.
    #[serde(default)]
    pub update: bool,
}
/// What a running share is doing (docs/share-speed-2026-10-29.md): what the window and the gallery's card say.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Stage {
    /// Just asked.
    #[default]
    Starting,
    /// Waiting for another change to the same item to finish.
    Waiting,
    /// Making a 3D capture's upload files, or waiting for their background preparation.
    Preparing,
    /// Sending its files: bytes sent of all.
    Sending { sent: u64, total: u64 },
    /// Its files all sent to replace those of the shared capture: Orbiters checks them and switches to them.
    Replacing,
    /// Telling Orbiters who sees it.
    Sharing,
    /// Several items (a world's pictures): how many are shared of all.
    Items { done: u32, total: u32 },
}
/// Several items shared at once at most (each its upload and its grant).
const SHARES_AT_ONCE: usize = 3;
/// Where a share reports its stage.
pub type Report<'a> = &'a (dyn Fn(Stage) + Sync);

fn run_request(request: Request, report: Report) -> anyhow::Result<Outcome> {
    match request.operation {
        Operation::Share(r) if request.extra.is_empty() => share(request.server, request.entry, r, report).map(Outcome::Shared),
        Operation::Share(r) => {
            let entries = std::iter::once(request.entry).chain(request.extra).collect();
            share_all(entries, report, |entry| share(request.server, entry, r.clone(), &|_| {})).map(Outcome::Shared)
        }
        Operation::Unshare(r) => unshare(request.server, request.entry, r).map(Outcome::Unshared),
        Operation::Answer { id, approve, acknowledge_explicit } => {
            let (api, token) = api(request.server)?;
            Ok(Outcome::Answered(api.answer_capture_request(&token, id, approve, acknowledge_explicit)?.request))
        }
        Operation::Download => download(request.server, request.entry.remote.ok_or_else(|| anyhow::anyhow!("No uploaded item"))?).map(Outcome::Downloaded),
        Operation::Update => update(request.server, request.entry, report).map(|(item, edits)| Outcome::Updated { item, edits }),
    }
}
/// A share of several items ([`SHARES_AT_ONCE`] at a time, none started after one failed): the first item's answer, or
/// the first failure in their order.
fn share_all(entries: Vec<Entry>, report: Report, one: impl Fn(Entry) -> anyhow::Result<shares::Shared> + Sync) -> anyhow::Result<shares::Shared> {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::SeqCst};
    let total = entries.len() as u32;
    report(Stage::Items { done: 0, total });
    let (next, done, failed) = (AtomicUsize::new(0), AtomicU32::new(0), AtomicBool::new(false));
    let results: Vec<Mutex<Option<anyhow::Result<shares::Shared>>>> = entries.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..SHARES_AT_ONCE.min(entries.len()) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, SeqCst);
                if i >= entries.len() || failed.load(SeqCst) {
                    break;
                }
                let result = one(entries[i].clone());
                failed.fetch_or(result.is_err(), SeqCst);
                *results[i].lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                report(Stage::Items { done: done.fetch_add(1, SeqCst) + 1, total });
            });
        }
    });
    let mut first = None;
    for result in results.into_iter().filter_map(|r| r.into_inner().unwrap_or_else(|e| e.into_inner())) {
        let shared = result?;
        first.get_or_insert(shared);
    }
    first.ok_or_else(|| anyhow::anyhow!("Nothing was shared"))
}

impl super::Controller {
    /// The tray owns each mutation until completion, even if the submitting window closes.
    pub fn capture_share(self: &super::Shared, request: Request) {
        #[cfg(test)]
        if let Some(work) = test_work(&request.entry.key) {
            return self.capture_share_with(request, move |r, report| work(r, report));
        }
        self.capture_share_with(request, run_request);
    }
    pub(crate) fn capture_share_with(
        self: &super::Shared,
        request: Request,
        work: impl FnOnce(Request, Report) -> anyhow::Result<Outcome> + Send + 'static,
    ) {
        let update = matches!(request.operation, Operation::Update);
        let accepted = self.update(|s| {
            if s.capture_shares.iter().any(|j| j.id == request.id) {
                return false;
            }
            s.capture_shares.push(ShareJob { id: request.id.clone(), key: request.entry.key.clone(), stage: Stage::Starting, outcome: None, update });
            true
        });
        if !accepted {
            return;
        }
        let ctl = self.clone();
        let id = request.id.clone();
        std::thread::spawn(move || {
            // Every change goes to every window: bytes at most every 100 ms, the other stages as they come.
            let last = Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
            let report = |stage: Stage| {
                if let Stage::Sending { sent, total } = stage {
                    let mut last = last.lock().unwrap_or_else(|e| e.into_inner());
                    if sent < total && last.elapsed().as_millis() < 100 {
                        return;
                    }
                    *last = std::time::Instant::now();
                }
                ctl.update_if(|s| match s.capture_shares.iter_mut().find(|j| j.id == id && j.stage != stage) {
                    Some(j) => {
                        j.stage = stage;
                        true
                    }
                    None => false,
                });
            };
            let outcome = work(request, &report)
                .unwrap_or_else(|e| Outcome::Failed { explicit: e.downcast_ref::<shares::ExplicitAcknowledgement>().is_some(), message: e.to_string() });
            let (kind, message) = match &outcome {
                Outcome::Failed { message, explicit: false } => (super::NoticeKind::Error, message.clone()),
                Outcome::Failed { explicit: true, .. } => {
                    (super::NoticeKind::Info, "Open Photos & captures to confirm sharing this explicit item publicly.".into())
                }
                Outcome::Shared(shares::Shared::Request { .. }) => (super::NoticeKind::Success, "Share request sent.".into()),
                Outcome::Answered(_) => (super::NoticeKind::Success, "Share request answered.".into()),
                Outcome::Downloaded(p) => (super::NoticeKind::Success, format!("Saved {}", p.display())),
                Outcome::Updated { .. } => (super::NoticeKind::Success, UPDATED.into()),
                _ => (super::NoticeKind::Success, "Sharing updated.".into()),
            };
            ctl.update(|s| {
                if let Some(j) = s.capture_shares.iter_mut().find(|j| j.id == id) {
                    j.outcome = Some(outcome);
                }
                while s.capture_shares.len() > 32 {
                    if let Some(i) = s.capture_shares.iter().position(|j| j.outcome.is_some()) {
                        s.capture_shares.remove(i);
                    } else {
                        break;
                    }
                }
            });
            ctl.notify(kind, message);
            ctl.bump_library();
            ctl.refresh_maps();
        });
    }

    /// Share to … opened on `session`: a reconstructed 3D capture this account has not uploaded gets its upload files
    /// made while friends are chosen (maps::web_preview, whose cache the share then reads at once). Nothing is sent.
    pub fn prepare_share(&self, session: PathBuf) {
        static AHEAD: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());
        let server = if self.settings().development { Server::Development } else { Server::Production };
        let uploaded = records_path(server).ok().is_some_and(|p| read_records(&p).iter().any(|r| r.path == session));
        if uploaded
            || orbiters_core::reconstruction::read_report(&session).is_none()
            || !AHEAD.lock().unwrap_or_else(|e| e.into_inner()).insert(session.clone())
        {
            return;
        }
        std::thread::spawn(move || {
            let started = Instant::now();
            let failed = super::maps::web_preview(&session).err().map(|e| format!(", failed: {e}")).unwrap_or_default();
            share_log(&format!("3D capture {} prepared ahead of its share in {:.2} s{failed}", session.display(), started.elapsed().as_secs_f64()));
            AHEAD.lock().unwrap_or_else(|e| e.into_inner()).remove(&session);
        });
    }
}

impl Entry {
    pub fn mine(&self) -> bool {
        self.remote.as_ref().is_none_or(|i| i.mine)
    }
    /// A picture (not a 3D capture or recording): a VRChat picture, a shared one, or a gallery's on Orbiters (a
    /// community's `community:…`, an asset's `asset:…`: neither on this computer nor in the sharing feed).
    pub fn is_picture(&self) -> bool {
        matches!(self.local, Some(Local::Picture(_)))
            || self.remote.as_ref().is_some_and(|i| i.kind == Kind::Picture)
            || self.local.is_none() && self.remote.is_none() && (self.key.starts_with("community:") || self.key.starts_with("asset:"))
    }

    /// A VRChat multi-layer picture's layers ([`Layer`]: each file, VRChat's name for it, its size), back to front, the
    /// order to stack them in: empty for a plain picture and for captures. The entry's own file ([`Entry::local`]) is
    /// the picture VRChat saved with them, all layers in one; when that file is gone, it is the bottom layer, then the
    /// first of these too ([`Entry::has_composite`]).
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// A capture of this computer, uploaded, whose splat edits are not those its upload was made with: the copy on
    /// Orbiters still shows what was removed since (or lacks what was brought back) until it is updated
    /// ([`Operation::Update`]; a share of it updates it first). Edits that cannot be read are not that: whatever is asked
    /// of them stops with why.
    pub fn edited_since_upload(&self) -> bool {
        let uploaded = self.uploaded_edits.is_some() || self.remote.as_ref().is_some_and(|i| i.mine && i.kind == Kind::Capture);
        let made_with = self.uploaded_edits.as_deref().unwrap_or(orbiters_reconstruct::edit::UNEDITED);
        uploaded && self.mine() && matches!(self.local, Some(Local::Session(_))) && matches!(&self.edits, Some(Ok(now)) if now != made_with)
    }

    /// Whether the entry's own file is the picture of all its layers (and not one of them).
    pub fn has_composite(&self) -> bool {
        let own = self.local.as_ref().map(Local::path);
        !self.layers.iter().any(|l| Some(l.path.as_path()) == own)
    }

    /// One of its layers as an entry of its own: the picture window shares that file when the layer is chosen (its own
    /// upload, its own grants). Its card key is its file's, as any picture's.
    pub fn layer_entry(&self, i: usize) -> Option<Entry> {
        let layer = self.layers.get(i)?;
        let own = self.local.as_ref().is_some_and(|l| l.path() == layer.path);
        Some(Entry {
            key: layer.path.to_string_lossy().into(),
            source_key: if own { self.source_key.clone() } else { None },
            local: Some(Local::Picture(layer.path.clone())),
            remote: layer.shared.clone(),
            picture: Some(layer.picture(crate::worlds::THUMB_WIDTH)),
            size: Some([layer.width, layer.height]),
            layers: Vec::new(),
            ..self.clone()
        })
    }
}

/// Server ids win; an owner's source key also identifies a local file before it has an upload record.
pub fn matches_item(entry: &Entry, item: &Item) -> bool {
    if let Some(known) = entry.remote.as_ref() {
        return known.key() == item.key();
    }
    item.mine
        && entry.world_id == item.world.id
        && entry.is_picture() == (item.kind == Kind::Picture)
        && entry.source_key.as_ref().zip(item.source_key.as_ref()).is_some_and(|(a, b)| !a.is_empty() && a == b)
}
pub fn unix(date: &str) -> u64 {
    time::OffsetDateTime::parse(date, &time::format_description::well_known::Rfc3339).ok().and_then(|t| u64::try_from(t.unix_timestamp()).ok()).unwrap_or(0)
}
pub fn iso(unix: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix as i64).ok().and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()).unwrap_or_default()
}
/// Only the last globally joined instance can own a timestamp. A later join of another world prevents misattribution.
pub fn instance_at(history: &visited::History, world: &str, taken: u64) -> Option<String> {
    history.instances.iter().filter(|i| i.at <= taken).max_by_key(|i| i.at).filter(|i| i.world_id == world).map(|i| i.instance.clone())
}
pub fn local_entries(root: &Path, server: Server) -> Vec<Entry> {
    local_entries_with_folders(root, server, &orbiters_core::settings::Settings::load().picture_folders)
}
pub fn local_entries_with_folders(root: &Path, server: Server, folders: &[PathBuf]) -> Vec<Entry> {
    let worlds = crate::worlds::load_gallery(root, folders);
    let history = visited::History::load(&visited::history_file());
    let records = records_path(server).ok().map(|p| read_records(&p)).unwrap_or_default();
    let mut entries = Vec::new();
    for w in worlds {
        for s in &w.sessions {
            let taken = unix(&s.manifest.created_utc);
            let local = Local::Session(s.dir.clone());
            let record = records.iter().find(|r| r.path == s.dir);
            let remote = record.and_then(|r| r.item.clone());
            // What its outputs leave out now, against what its upload was made with (its window offers to update it).
            let edits = s
                .reconstruction
                .as_ref()
                .map(|_| orbiters_reconstruct::edit::generation(&orbiters_core::reconstruction::result_dir(&s.dir)).map_err(|e| e.to_string()));
            let uploaded_edits = record.map(|r| r.edits.clone().unwrap_or_else(|| orbiters_reconstruct::edit::UNEDITED.into()));
            entries.push(Entry {
                key: s.dir.to_string_lossy().into(),
                world_id: w.id.clone(),
                world_name: w.name.clone(),
                taken,
                instance: s.manifest.world.as_ref().and_then(|w| w.instance.clone()).or_else(|| instance_at(&history, &w.id, taken)),
                source_key: s.dir.file_name().map(|s| s.to_string_lossy().into_owned()),
                people: Vec::new(),
                local: Some(local),
                remote,
                content: Content::read(&s.dir),
                ready: s.reconstruction.is_some(),
                picture: Some(crate::worlds::Picture::Session(s.dir.clone())),
                size: session_picture_size(s.manifest.capture.width, s.manifest.capture.height),
                layers: Vec::new(),
                edits,
                uploaded_edits,
            });
        }
        for shot in &w.shots {
            let mut content = Content::default();
            for s in &w.sessions {
                let start = unix(&s.manifest.created_utc);
                if shot.info.taken >= start && shot.info.taken <= start + s.manifest.stats.duration_s.ceil() as u64 {
                    content.merge(&Content::read(&s.dir));
                }
            }
            let remote = records.iter().find(|r| r.path == shot.path).and_then(|r| r.item.clone());
            let mut layers = w.layers.get(&shot.path).cloned().unwrap_or_default();
            for l in &mut layers {
                l.shared = records.iter().find(|r| r.path == l.path).and_then(|r| r.item.clone());
            }
            // A layer alone (its picture deleted) shows with its transparency.
            let face = layers.iter().find(|l| l.path == shot.path);
            let picture = match face {
                Some(l) if l.transparent() => l.picture(560),
                _ => crate::worlds::Picture::Shot { path: shot.path.clone(), size: shot.size, width: 560 },
            };
            entries.push(Entry {
                key: shot.path.to_string_lossy().into(),
                world_id: w.id.clone(),
                world_name: w.name.clone(),
                taken: shot.info.taken,
                instance: shot.info.instance.clone().or_else(|| instance_at(&history, &w.id, shot.info.taken)),
                source_key: shot.sha256.clone(),
                people: Vec::new(),
                local: Some(Local::Picture(shot.path.clone())),
                remote,
                content,
                ready: true,
                picture: Some(picture),
                size: (shot.info.width > 0 && shot.info.height > 0).then_some([shot.info.width, shot.info.height]),
                layers,
                edits: None,
                uploaded_edits: None,
            });
        }
    }

    entries.sort_by_key(|e| std::cmp::Reverse(e.taken));
    entries
}
pub fn remote_entry(item: Item) -> Entry {
    let picture = item.picture.as_ref().and_then(|p| {
        p.thumbnail_url.starts_with("https://").then(|| crate::worlds::Picture::Remote {
            world_id: format!("capture-{}-{}", item.kind.path(), item.id),
            url: p.thumbnail_url.clone(),
            large: false,
        })
    });
    Entry {
        key: format!("{}:{}", item.kind.path(), item.id),
        world_id: item.world.id.clone(),
        world_name: item.world.name.clone(),
        taken: unix(item.date()),
        instance: item.instance_id.clone(),
        source_key: item.source_key.clone(),
        people: Vec::new(),
        local: None,
        content: Content { explicit: item.explicit, explicit_reasons: item.explicit_reasons.clone() },
        ready: true,
        picture,
        size: item.picture.as_ref().filter(|p| p.width > 0 && p.height > 0).map(|p| [p.width, p.height]),
        layers: Vec::new(),
        edits: None,
        uploaded_edits: None,
        remote: Some(item),
    }
}

/// The size of a capture's picture: its frames without the strip at the top and the skeleton band's rows at the bottom
/// ([`orbiters_core::bundle::capture_picture_rect`], as its thumbnail is cropped). None when the manifest has no size.
pub fn session_picture_size(width: u32, height: u32) -> Option<[u32; 2]> {
    if width == 0 || height == 0 {
        return None;
    }
    let r = orbiters_core::bundle::capture_picture_rect(width, height);
    (r.y1 > r.y0).then_some([width, r.y1 - r.y0])
}
pub fn api(server: Server) -> anyhow::Result<(Api, String)> {
    let account = account::read(server).ok_or_else(|| anyhow::anyhow!("Sign in to Orbiters in Settings first."))?;
    Ok((Api::new(server), account.token))
}
pub fn feed(server: Server, query: shares::Query) -> anyhow::Result<shares::Feed> {
    let (api, token) = api(server)?;
    api.captures(&token, &query)
}
#[derive(Serialize, Deserialize)]
struct Record {
    path: PathBuf,
    item: Option<Item>,
    uploaded: Uploaded,
    /// For a capture: the generation of its splat edits the uploaded files were made with
    /// (orbiters_reconstruct::edit::generation); none in records of uploads made before edits existed (as trained).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    edits: Option<String>,
}

/// Why a capture is not shared when its splat edits changed while it was being shared.
const EDITED_MEANWHILE: &str = "Its splats were changed in its 3D view while it was being shared, so it is not shared: share it again to send what \
     its 3D view shows now.";
/// What the tray says once an update is done.
pub(crate) const UPDATED: &str = "Updated: everyone who sees this 3D capture now sees it as its 3D view shows it.";
/// How a failed update ends: Orbiters switches to new files whole or not at all.
pub(crate) const KEPT: &str = "Everyone still sees the version shared before.";

/// The generation of the splat edits of a capture's session now (none for a picture); edits that cannot be read stop
/// whatever asked.
fn edits_now(entry: &Entry) -> anyhow::Result<Option<String>> {
    match entry.local.as_ref() {
        Some(Local::Session(dir)) => Ok(Some(orbiters_reconstruct::edit::generation(&orbiters_core::reconstruction::result_dir(dir))?)),
        _ => Ok(None),
    }
}
fn records_path(server: Server) -> anyhow::Result<PathBuf> {
    let account = account::read(server).ok_or_else(|| anyhow::anyhow!("Sign in first"))?;
    let user = account.user.ok_or_else(|| anyhow::anyhow!("Orbiters did not identify this account"))?;
    anyhow::ensure!(user.chars().all(|c| c.is_ascii_digit()), "Invalid account id");
    Ok(orbiters_core::paths::config_dir().join(format!("capture-uploads-{}-{user}.json", if server == Server::Development { "dev" } else { "prod" })))
}
/// What this account uploaded from this computer: each session's or picture's upload as the server last answered it.
pub fn uploads(server: Server) -> Vec<(PathBuf, Item)> {
    records_path(server).map(|p| read_records(&p)).unwrap_or_default().into_iter().filter_map(|r| r.item.map(|item| (r.path, item))).collect()
}
fn read_records(path: &Path) -> Vec<Record> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}
fn save_record(records: anyhow::Result<&Path>, entry: &Entry, item: &Item) -> anyhow::Result<()> {
    save_record_edits(records, entry, item, None)
}
/// [`save_record`] with the splat edits its files on Orbiters were made with (`None`: those its record had).
fn save_record_edits(records: anyhow::Result<&Path>, entry: &Entry, item: &Item, edits: Option<String>) -> anyhow::Result<()> {
    if let Some(local) = &entry.local {
        let _records = RECORDS.lock().unwrap_or_else(|e| e.into_inner());
        let path = records?;
        let mut records = read_records(&path);
        // The splat edits it was uploaded with stay known (the same item: the same files) until it is updated.
        let edits = edits.or_else(|| {
            records.iter().find(|r| r.path == local.path() && r.uploaded.id == item.id && r.uploaded.kind == item.kind).and_then(|r| r.edits.clone())
        });
        records.retain(|r| r.path != local.path());
        records.push(Record {
            path: local.path().into(),
            item: Some(item.clone()),
            uploaded: Uploaded { kind: item.kind, id: item.id, visibility: if item.public { "public" } else { "private" }.into() },
            edits,
        });
        orbiters_core::write_atomic(&path, &serde_json::to_vec(&records)?)?;
    }
    Ok(())
}
/// Its upload is not on Orbiters any more (removed there): its record goes, so that a share uploads it again.
fn forget_record(path: &Path, dir: &Path, id: u64) {
    let _records = RECORDS.lock().unwrap_or_else(|e| e.into_inner());
    let mut records = read_records(path);
    let before = records.len();
    records.retain(|r| !(r.path == dir && r.uploaded.kind == Kind::Capture && r.uploaded.id == id));
    if records.len() != before {
        if let Ok(bytes) = serde_json::to_vec(&records) {
            let _ = orbiters_core::write_atomic(path, &bytes);
        }
    }
}
/// The items being changed (their local file, else their server key): one change at a time each, different items at
/// once. A share uploading a 3D capture no longer holds up another item's share.
static CHANGING: (Mutex<BTreeSet<String>>, Condvar) = (Mutex::new(BTreeSet::new()), Condvar::new());
/// The upload records are read, changed and written whole: one writer at a time.
static RECORDS: Mutex<()> = Mutex::new(());
struct Changing(String);
impl Changing {
    fn of(entry: &Entry, report: Report) -> Self {
        let key = entry.local.as_ref().map_or_else(|| entry.key.clone(), |l| l.path().to_string_lossy().into_owned());
        let (set, wake) = &CHANGING;
        let mut set = set.lock().unwrap_or_else(|e| e.into_inner());
        if set.contains(&key) {
            report(Stage::Waiting);
        }
        while set.contains(&key) {
            set = wake.wait(set).unwrap_or_else(|e| e.into_inner());
        }
        set.insert(key.clone());
        Self(key)
    }
}
impl Drop for Changing {
    fn drop(&mut self) {
        let (set, wake) = &CHANGING;
        set.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0);
        wake.notify_all();
    }
}
/// Upload identity is saved before granting: retry after a failed share never uploads a duplicate. An item already
/// uploaded (as the gallery or its record says) is shared with one request, never sent or fetched again, unless it is a
/// capture whose splats were edited since: its files on Orbiters are replaced first ([`update_with`]), so what is shared
/// is what its 3D view shows. How long each step took goes to `%LOCALAPPDATA%/Orbiters/logs/shares.log`.
pub fn share(server: Server, entry: Entry, request: shares::Share, report: Report) -> anyhow::Result<shares::Shared> {
    let started = Instant::now();
    let mut steps = Vec::new();
    let result =
        api(server).and_then(|(api, token)| share_with(&api, &token, records_path(server), &entry, &request, report, &mut steps, &super::maps::web_preview));
    let whom = match (request.public, request.recipients.len()) {
        (true, 0) => "publicly".to_string(),
        (public, n) => format!("{}with {n} friend{}", if public { "publicly and " } else { "" }, if n == 1 { "" } else { "s" }),
    };
    let what = if entry.is_picture() { "picture" } else { "3D capture" };
    let end = match &result {
        Ok(_) => String::new(),
        Err(e) => format!(", failed: {e}"),
    };
    share_log(&format!("{what} {} shared {whom}: {}; {:.2} s in all{end}", entry.key, steps.join(", "), started.elapsed().as_secs_f64()));
    result
}
/// What makes a capture's upload files: [`super::maps::web_preview`] (a stand-in in tests).
pub(crate) type Prepare<'a> = &'a (dyn Fn(&Path) -> anyhow::Result<super::maps::Prepared> + Sync);
/// Notes how long a step took in `steps` (shares.log).
fn step(steps: &mut Vec<String>, what: &str, at: &mut Instant) {
    steps.push(format!("{what} {:.2} s", at.elapsed().as_secs_f64()));
    *at = Instant::now();
}
/// [`share`] through `api` as `token`, this account's upload records in `records`, a capture's files made by `prepare`;
/// what took how long in `steps`.
#[allow(clippy::too_many_arguments)]
fn share_with(
    api: &Api,
    token: &str,
    records: anyhow::Result<PathBuf>,
    entry: &Entry,
    request: &shares::Share,
    report: Report,
    steps: &mut Vec<String>,
    prepare: Prepare,
) -> anyhow::Result<shares::Shared> {
    let mut at = Instant::now();
    let _one = Changing::of(entry, report);
    if at.elapsed().as_millis() > 50 {
        step(steps, "waited for its other change", &mut at);
    }
    let records = records.as_deref().map_err(|e| anyhow::anyhow!("{e}"));
    let record = entry.local.as_ref().and_then(|l| records.as_ref().ok().and_then(|p| read_records(p).into_iter().find(|r| r.path == l.path())));
    let remembered = record.as_ref().map(|r| (r.uploaded.kind, r.uploaded.id));
    let existing = entry.remote.as_ref().map(|i| (i.kind, i.id)).or(remembered);
    let uploaded_now = existing.is_none();
    // A capture's files on the server are of the splat edits it was uploaded with (an upload this computer has no record
    // of was made without any).
    let mut uploaded_edits = record.as_ref().and_then(|r| r.edits.clone()).unwrap_or_else(|| orbiters_reconstruct::edit::UNEDITED.to_string());
    let mut sent_edits = None;
    let (kind, id) = if let Some(known) = existing {
        // Edited since it was uploaded: its files on Orbiters are replaced first (Update the shared capture).
        if let Some(now) = edits_now(entry)? {
            if now != uploaded_edits && known.0 == Kind::Capture {
                let records = records.as_ref().map(|p| *p).map_err(|e| anyhow::anyhow!("{e}"));
                let (_, made_with) = update_with(api, token, records, entry, report, steps, prepare).map_err(|e| anyhow::anyhow!("{}", not_updated(&e)))?;
                at = Instant::now();
                uploaded_edits = made_with;
            }
        }
        known
    } else {
        anyhow::ensure!(!request.public || !entry.content.explicit || request.acknowledge_explicit, "{}", shares::EXPLICIT_WARNING);
        let source_key = match entry.local.as_ref().ok_or_else(|| anyhow::anyhow!("No local file to upload"))? {
            Local::Session(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| anyhow::anyhow!("No capture folder name"))?,
            Local::Picture(p) => entry
                .source_key
                .clone()
                .or_else(|| crate::worlds::hashes(vec![p.clone()]).remove(p))
                .ok_or_else(|| anyhow::anyhow!("Could not fingerprint this picture"))?,
        };
        anyhow::ensure!(source_key.chars().count() <= 128, "The capture folder name is longer than the server's 128-character source key limit.");
        let mut metadata = serde_json::json!({"visibility":if request.public{"public"}else{"private"},"explicit":entry.content.explicit,"explicitReasons":entry.content.explicit_reasons,"acknowledgeExplicit":request.acknowledge_explicit,"takenAt":iso(entry.taken)});
        metadata["sourceKey"] = serde_json::json!(source_key);
        metadata["instanceId"] = serde_json::json!(entry.instance);

        let uploaded: Uploaded = match entry.local.as_ref().ok_or_else(|| anyhow::anyhow!("No local file to upload"))? {
            Local::Session(dir) => {
                report(Stage::Preparing);
                let super::maps::Prepared { files, sky, metadata: view, generation } = prepare(dir)?;
                // The picture only when drawn for these edits (capture_picture), and the edits still those prepared.
                let picture = crate::capture_picture::jpeg(dir);
                anyhow::ensure!(edits_now(entry)?.as_deref() == Some(generation.as_str()), "Its splat edits changed while it was being prepared: share it again.");
                sent_edits = Some(generation);
                step(steps, "prepared", &mut at);
                let bytes = files.len() + sky.as_deref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
                metadata.as_object_mut().unwrap().extend(view.as_object().unwrap().clone());
                // The upload's progress callback must be 'static: it reports over a channel read here.
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::scope(|scope| {
                    let sending = scope.spawn(|| {
                        api.add_preview(token, &entry.world_id, files, sky.as_deref(), picture, &metadata, move |sent, total| {
                            let _ = tx.send((sent, total));
                        })
                    });
                    for (sent, total) in rx {
                        report(Stage::Sending { sent, total });
                    }
                    sending.join().unwrap_or_else(|_| Err(anyhow::anyhow!("The upload stopped unexpectedly")))
                })
                .inspect(|_| step(steps, &format!("sent {:.1} MB and stored", bytes as f64 / 1e6), &mut at))?
            }
            Local::Picture(path) => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let part = Part::file("pictures", name.as_ref(), "image/png", path)?;
                metadata["takenAt"] = serde_json::json!([iso(entry.taken)]);
                report(Stage::Sending { sent: 0, total: std::fs::metadata(path).map_or(0, |m| m.len()) });
                let result = api.add_shared_pictures(token, &entry.world_id, vec![part], &metadata)?;
                step(steps, "sent and stored", &mut at);
                match result.added.into_iter().next() {
                    Some(uploaded) => uploaded,
                    None => {
                        let item = api.capture_by_source_key(token, &entry.world_id, Kind::Picture, &source_key)?.ok_or_else(|| {
                            anyhow::anyhow!("The server skipped this picture, but its source identity is not available yet. Refresh the gallery and retry.")
                        })?;
                        Uploaded { kind: item.kind, id: item.id, visibility: if item.public { "public" } else { "private" }.into() }
                    }
                }
            }
        };
        {
            let _records = RECORDS.lock().unwrap_or_else(|e| e.into_inner());
            let file = records.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut known = read_records(file);
            let local = entry.local.as_ref().unwrap();
            known.retain(|r| r.path != local.path());
            known.push(Record { path: local.path().into(), item: None, uploaded: uploaded.clone(), edits: sent_edits.clone() });
            orbiters_core::write_atomic(file, &serde_json::to_vec(&known)?)?;
        }
        (uploaded.kind, uploaded.id)
    };
    // Edited while it was uploading: not shared, and an upload that went public at once taken back from the public.
    if let (Some(sent), Ok(now)) = (&sent_edits, edits_now(entry)) {
        if now.as_deref() != Some(sent.as_str()) {
            if request.public {
                let withdraw = shares::Unshare { public: true, recipients: Vec::new(), all: false };
                if let Err(e) = api.unshare_capture(token, kind, id, &withdraw) {
                    step(steps, &format!("could not take it back from the public ({e})"), &mut at);
                }
            }
            anyhow::bail!("{EDITED_MEANWHILE}");
        }
    }
    // Edits that cannot be read now stop the share too; edited while it was updated, it is not shared either.
    if let Some(now) = edits_now(entry)? {
        let made_with = sent_edits.as_deref().unwrap_or(&uploaded_edits);
        anyhow::ensure!(now == made_with, "{EDITED_MEANWHILE}");
    }
    report(Stage::Sharing);
    let shared = api.share_capture(token, kind, id, request);
    step(steps, "shared", &mut at);
    match shared {
        Ok(mut result) => {
            if let shares::Shared::Item { item } = &mut result {
                // A public capture uploaded without its picture gets it now: its public page's link previews show it.
                if let (true, Kind::Capture, true, Some(Local::Session(dir))) = (item.public, item.kind, item.mine, entry.local.as_ref()) {
                    if item.capture.as_ref().is_some_and(|c| c.picture_url.is_none()) {
                        match crate::capture_picture::jpeg(dir).map(|jpeg| api.set_capture_picture(token, item.id, jpeg)) {
                            Some(Ok(with_picture)) => {
                                *item = with_picture;
                                step(steps, "gave it its picture", &mut at);
                            }
                            Some(Err(e)) => step(steps, &format!("its picture failed ({e})"), &mut at),
                            None => {}
                        }
                    }
                }
                save_record(records, entry, item)?;
            }
            Ok(result)
        }
        Err(e) => {
            // Uploaded but not shared: its item is remembered whole all the same, so the gallery shows it uploaded.
            if uploaded_now {
                if let Ok(item) = api.capture_item(token, kind, id) {
                    let _ = save_record(records, entry, &item);
                }
            }
            Err(e)
        }
    }
}

/// [`Operation::Update`]: the capture's files on Orbiters replaced by what its 3D view shows now, one change of the item
/// at a time ([`Changing`]); its item at the new revision and the splat edits they were made with. How long each step
/// took goes to shares.log. A failure says that the version shared before stays: Orbiters switches to new files whole or
/// not at all (contracts Docs/capture-sharing.md, Updating a capture).
pub fn update(server: Server, entry: Entry, report: Report) -> anyhow::Result<(Item, String)> {
    let started = Instant::now();
    let mut steps = Vec::new();
    let result = api(server).and_then(|(api, token)| {
        let _one = Changing::of(&entry, report);
        let records = records_path(server);
        update_with(&api, &token, records.as_deref().map_err(|e| anyhow::anyhow!("{e}")), &entry, report, &mut steps, &super::maps::web_preview)
    });
    let end = match &result {
        Ok((item, _)) => format!(", revision {}", item.capture.as_ref().map_or(1, shares::Capture::revision)),
        Err(e) => format!(", failed: {e}"),
    };
    share_log(&format!("3D capture {} updated: {}; {:.2} s in all{end}", entry.key, steps.join(", "), started.elapsed().as_secs_f64()));
    result.map_err(|e| anyhow::anyhow!("{}", not_updated(&e)))
}

/// Why an update did not happen, and that the version shared before stays.
fn not_updated(e: &anyhow::Error) -> String {
    let why = e.to_string();
    let end = if why.ends_with(['.', '!', '?']) { "" } else { "." };
    format!("Not updated: {why}{end} {KEPT}")
}

/// [`update`] through `api` as `token` (the caller holds the item's [`Changing`]), this account's upload records in
/// `records`: what Orbiters has (the revision to replace; still there, still this member's), the capture's files made
/// for its edits now by `prepare`, sent to replace those of that revision; the record then keeps the edits they were made
/// with.
fn update_with(
    api: &Api,
    token: &str,
    records: anyhow::Result<&Path>,
    entry: &Entry,
    report: Report,
    steps: &mut Vec<String>,
    prepare: Prepare,
) -> anyhow::Result<(Item, String)> {
    let mut at = Instant::now();
    let Some(Local::Session(dir)) = entry.local.as_ref() else { anyhow::bail!("Only a 3D capture of this computer can be updated.") };
    let record = records.as_ref().ok().and_then(|p| read_records(p).into_iter().find(|r| r.path == *dir));
    let id = entry
        .remote
        .as_ref()
        .filter(|i| i.kind == Kind::Capture)
        .map(|i| i.id)
        .or_else(|| record.as_ref().filter(|r| r.uploaded.kind == Kind::Capture).map(|r| r.uploaded.id))
        .ok_or_else(|| anyhow::anyhow!("It is not uploaded yet: share it to upload it."))?;
    // Damaged edits stop it before anything is asked of Orbiters.
    edits_now(entry)?;
    let current = match api.capture_item(token, Kind::Capture, id) {
        Ok(item) => item,
        Err(e) if e.downcast_ref::<orbiters_core::account::ApiError>().is_some_and(|a| a.status == 404) => {
            if let Ok(path) = records.as_ref() {
                forget_record(path, dir, id);
            }
            anyhow::bail!("It is not on Orbiters any more: share it again to upload it as its 3D view shows it now.");
        }
        Err(e) => return Err(e),
    };
    anyhow::ensure!(current.mine, "Only the member who uploaded it can update it.");
    let revision = current.capture.as_ref().map_or(1, shares::Capture::revision);
    step(steps, "asked Orbiters", &mut at);
    report(Stage::Preparing);
    let super::maps::Prepared { files, sky, metadata, generation } = prepare(dir)?;
    // The picture only when drawn for these edits (capture_picture), and the edits still those prepared.
    let picture = crate::capture_picture::jpeg(dir);
    anyhow::ensure!(
        edits_now(entry)?.as_deref() == Some(generation.as_str()),
        "Its splat edits changed while it was being prepared: update it again once they are done."
    );
    step(steps, "prepared", &mut at);
    let bytes = files.len() + sky.as_deref().and_then(|p| std::fs::metadata(p).ok()).map_or(0, |m| m.len());
    let (tx, rx) = std::sync::mpsc::channel();
    let sent = std::thread::scope(|scope| {
        let sending = scope.spawn(|| {
            api.replace_capture_files(token, id, revision, files, sky.as_deref(), picture, &metadata, move |sent, total| {
                let _ = tx.send((sent, total));
            })
        });
        // Every byte sent: Orbiters checks the files and switches to them.
        for (sent, total) in rx {
            report(if total > 0 && sent >= total { Stage::Replacing } else { Stage::Sending { sent, total } });
        }
        sending.join().unwrap_or_else(|_| Err(anyhow::anyhow!("The update stopped unexpectedly")))
    });
    let item = match sent {
        Ok(item) => item,
        Err(e) => {
            if let Some(conflict) = e.downcast_ref::<shares::RevisionConflict>() {
                anyhow::bail!(
                    "it was updated from somewhere else meanwhile (it is at revision {}; this computer replaced revision {revision}). Look at it, then \
                     update it again to send what this computer's 3D view shows.",
                    conflict.revision
                );
            }
            // No answer after the files went (the connection dropped): Orbiters switched to them, or not, as its revision
            // says (another computer's update of the same capture in that very moment would read the same).
            let switched = match e.downcast_ref::<orbiters_core::account::ApiError>() {
                Some(_) => None,
                None => api.capture_item(token, Kind::Capture, id).ok().filter(|now| now.capture.as_ref().map_or(1, shares::Capture::revision) == revision + 1),
            };
            match switched {
                Some(now) => {
                    step(steps, &format!("lost the answer ({e}), found it switched"), &mut at);
                    now
                }
                None => return Err(e),
            }
        }
    };
    step(steps, &format!("sent {:.1} MB and switched", bytes as f64 / 1e6), &mut at);
    save_record_edits(records, entry, &item, Some(generation.clone()))?;
    Ok((item, generation))
}
/// Tests: what a share of the entry with this key does instead of the real job (against a stand-in Orbiters), so that
/// a window's own buttons run the tray's job machinery without an account or the network.
#[cfg(test)]
pub(crate) type TestWork = std::sync::Arc<dyn Fn(Request, Report) -> anyhow::Result<Outcome> + Send + Sync>;
#[cfg(test)]
pub(crate) static TEST_WORK: Mutex<Vec<(String, TestWork)>> = Mutex::new(Vec::new());
#[cfg(test)]
fn test_work(key: &str) -> Option<TestWork> {
    TEST_WORK.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| k == key).map(|(_, w)| w.clone())
}
/// Tests: [`Operation::Update`] through `api` (a stand-in), this account's records in `records`, files by `prepare`.
#[cfg(test)]
pub(crate) fn update_against(api: Api, records: PathBuf, prepare: std::sync::Arc<dyn Fn(&Path) -> anyhow::Result<super::maps::Prepared> + Send + Sync>) -> TestWork {
    std::sync::Arc::new(move |request: Request, report: Report| {
        let _one = Changing::of(&request.entry, report);
        update_with(&api, "token", Ok(records.as_path()), &request.entry, report, &mut Vec::new(), &*prepare)
            .map(|(item, edits)| Outcome::Updated { item, edits })
            .map_err(|e| anyhow::anyhow!("{}", not_updated(&e)))
    })
}
/// A line in `%LOCALAPPDATA%/Orbiters/logs/shares.log`, kept under 1 MB (the last one renamed .old). Tests write none.
fn share_log(line: &str) {
    #[cfg(not(test))]
    {
        use std::io::Write;
        let dir = orbiters_core::paths::logs_dir();
        let path = dir.join("shares.log");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
            let _ = std::fs::rename(&path, dir.join("shares.old.log"));
        }
        if let Ok(mut f) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::OpenOptions::new().create(true).append(true).open(&path)) {
            let _ = writeln!(f, "{} {line}", orbiters_core::now_utc_iso());
        }
    }
    #[cfg(test)]
    let _ = line;
}
pub fn unshare(server: Server, entry: Entry, request: shares::Unshare) -> anyhow::Result<Item> {
    let _one = Changing::of(&entry, &|_| {});
    let (api, token) = api(server)?;
    let item = entry.remote.as_ref().ok_or_else(|| anyhow::anyhow!("This item has not been uploaded"))?;
    let item = api.unshare_capture(&token, item.kind, item.id, &request)?;
    save_record(records_path(server).as_deref().map_err(|e| anyhow::anyhow!("{e}")), &entry, &item)?;
    Ok(item)
}
pub fn download(server: Server, item: Item) -> anyhow::Result<PathBuf> {
    let (api, token) = api(server)?;
    let dir = orbiters_core::paths::documents_root().join("Shared captures");
    std::fs::create_dir_all(&dir)?;
    let out =
        dir.join(format!("{}-{}-{}.{}", item.kind.path(), item.id, uuid::Uuid::new_v4().simple(), if item.kind == Kind::Picture { "webp" } else { "splat" }));
    let temp = out.with_extension("download");
    let result = api.capture_file(&token, item.kind, item.id, &temp);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    std::fs::rename(temp, &out)?;
    Ok(out)
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn grouping_never_assigns_another_world_or_a_future_join() {
        let mut h = visited::History::default();
        h.instances = vec![
            visited::InstanceVisit { world_id: "a".into(), instance: "one".into(), at: 10 },
            visited::InstanceVisit { world_id: "b".into(), instance: "two".into(), at: 20 },
        ];
        assert_eq!(instance_at(&h, "a", 15), Some("one".into()));
        assert_eq!(instance_at(&h, "a", 21), None);
        assert_eq!(instance_at(&h, "a", 9), None);
    }
    #[test]
    fn tray_job_survives_a_dropped_window_snapshot_and_round_trips_ipc() {
        let ctl = super::super::Controller::new(orbiters_core::settings::Settings::default());
        let request = Request {
            id: "fixture-job".into(),
            server: Server::Development,
            entry: remote_entry(Item::default()),
            extra: Vec::new(),
            operation: Operation::Share(shares::Share { public: true, ..Default::default() }),
        };
        let message = crate::ipc::ToService::CaptureShare { request: Box::new(request.clone()) };
        let json = serde_json::to_string(&message).unwrap();
        let parsed: crate::ipc::ToService = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, crate::ipc::ToService::CaptureShare { .. }));
        let (tx, rx) = std::sync::mpsc::channel();
        ctl.capture_share_with(request, move |_, _| {
            rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
            Ok(Outcome::Shared(shares::Shared::Request { request: shares::ShareRequest { id: 5, ..Default::default() } }))
        });
        let window = ctl.view();
        assert_eq!(window.busy_with(), Some("the upload"));
        drop(window);
        tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while ctl.view().capture_shares[0].outcome.is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let snapshot = ctl.view();
        assert!(matches!(snapshot.capture_shares[0].outcome, Some(Outcome::Shared(shares::Shared::Request { .. }))));
        assert!(snapshot.busy_with().is_none());
        let encoded = serde_json::to_string(snapshot.as_ref()).unwrap();
        let decoded: crate::state::Snapshot = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.capture_shares[0].id, "fixture-job");
    }
    /// A stand-in for Orbiters on a loopback port, answering by the start of the request line; the requests it got, in
    /// order ("POST /captures/capture/3/share").
    fn fake_orbiters(answers: Vec<(&'static str, String)>) -> (Api, std::sync::Arc<Mutex<Vec<String>>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = Api::at(&format!("http://{}/", listener.local_addr().unwrap()));
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let (mut line, mut length) = (String::new(), 0);
                reader.read_line(&mut line).unwrap();
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                reader.read_exact(&mut vec![0; length]).unwrap();
                let request = line.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
                let answer = answers.iter().find(|(start, _)| request.starts_with(start)).map_or("{}".to_string(), |(_, a)| a.clone());
                log.lock().unwrap().push(request);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len())
                    .unwrap();
            }
        });
        (api, seen)
    }

    fn shared_item(kind: Kind, id: u64) -> String {
        let grants = vec![shares::Grant { id: 1, vrchat_user_id: "usr_friend".into(), display_name: Some("Friend".into()), ..Default::default() }];
        serde_json::to_string(&shares::Shared::Item { item: Item { kind, id, mine: true, can_share: true, grants, ..Default::default() } }).unwrap()
    }

    fn to_a_friend() -> shares::Share {
        shares::Share { recipients: vec![shares::Recipient { vrchat_user_id: "usr_friend".into(), display_name: "Friend".into() }], ..Default::default() }
    }

    /// A stand-in for Orbiters on a loopback port that answers each request with the first answer whose start matches its
    /// request line ("PUT /captures/capture/3/files"): an answer is used once while another one matches after it, the
    /// last one that matches stays; status 0 reads the request and closes the connection without an answer. The
    /// requests it got with their bodies, in order.
    pub(crate) fn stand_in(answers: Vec<(&'static str, u16, String)>) -> (Api, std::sync::Arc<Mutex<Vec<(String, Vec<u8>)>>>) {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = Api::at(&format!("http://{}/", listener.local_addr().unwrap()));
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            let mut answers = answers;
            for mut stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let (mut line, mut length) = (String::new(), 0);
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let request = line.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
                log.lock().unwrap().push((request.clone(), body));
                let matching: Vec<usize> = answers.iter().enumerate().filter(|(_, (start, ..))| request.starts_with(start)).map(|(i, _)| i).collect();
                let (status, answer) = match matching.as_slice() {
                    [] => (200, "{}".to_string()),
                    [only] => (answers[*only].1, answers[*only].2.clone()),
                    [first, ..] => {
                        let (_, status, answer) = answers.remove(*first);
                        (status, answer)
                    }
                };
                if status == 0 {
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    continue;
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                    answer.len()
                );
            }
        });
        (api, seen)
    }

    /// An item answer of the capture `id` at `revision`, its owner's.
    pub(crate) fn capture_at(id: u64, revision: u64) -> String {
        let capture = shares::Capture { splat_count: 2000, revision, preview_url: format!("/captures/capture/{id}/file?v={revision}"), ..Default::default() };
        serde_json::json!({ "item": Item { kind: Kind::Capture, id, mine: true, can_share: true, public: true, capture: Some(capture), ..Default::default() } })
            .to_string()
    }

    /// A reconstructed session in a temporary folder, uploaded as capture 3 (its record made with `edits`), and its entry
    /// as Photos & captures has it (no item fetched yet).
    pub(crate) fn uploaded_session(name: &str, edits: Option<String>) -> (PathBuf, PathBuf, PathBuf, Entry) {
        let dir = std::env::temp_dir().join(format!("orbiters-update-{name}-{}", uuid::Uuid::new_v4()));
        let session = dir.join("Home 2026-11-02 10-00-00");
        std::fs::create_dir_all(orbiters_core::reconstruction::dir(&session)).unwrap();
        let records = dir.join("capture-uploads.json");
        let uploaded = Uploaded { kind: Kind::Capture, id: 3, visibility: "public".into() };
        std::fs::write(&records, serde_json::to_vec(&[Record { path: session.clone(), item: None, uploaded, edits: edits.clone() }]).unwrap()).unwrap();
        let mut entry = remote_entry(Item::default());
        entry.key = session.to_string_lossy().into();
        entry.remote = None;
        entry.local = Some(Local::Session(session.clone()));
        entry.uploaded_edits = Some(edits.unwrap_or_else(|| orbiters_reconstruct::edit::UNEDITED.into()));
        (dir, session, records, entry)
    }

    /// Splat `n` of the session deleted in its 3D view (on top of what the edit file holds); its generation.
    pub(crate) fn delete_splat(session: &Path, n: usize) -> String {
        use orbiters_reconstruct::edit::{Edit, Identity, DELETED};
        let result = orbiters_core::reconstruction::result_dir(session);
        let file = result.join(orbiters_core::reconstruction::EDIT_FILE);
        let mut edit =
            std::fs::read(&file).ok().and_then(|b| Edit::decode(&b).ok()).unwrap_or_else(|| Edit::new(10, Identity { bytes: 1 << 20, hash: 1 }));
        edit.marks[n] = DELETED;
        edit.save(&result).unwrap();
        edit.generation()
    }

    /// The upload files made for the session's edits now (a stand-in for the GPU-tuned trees: bytes the stand-in server
    /// takes as they are), counted.
    pub(crate) fn prepared(session: &Path) -> anyhow::Result<super::super::maps::Prepared> {
        let generation = orbiters_reconstruct::edit::generation(&orbiters_core::reconstruction::result_dir(session))?;
        let tree = |tag: u8| [orbiters_reconstruct::tree::MAGIC.as_slice(), &[tag; 56]].concat();
        Ok(super::super::maps::Prepared {
            files: orbiters_core::maps::PreviewFiles { vrchat_pc: tree(1), vrchat_quest: tree(2), vrchat_coarse: tree(3) },
            sky: None,
            metadata: serde_json::json!({ "view": { "position": [0.0, 1.6, 0.0], "target": [0.0, 1.6, -4.0] }, "quality": 31.5 }),
            generation,
        })
    }

    pub(crate) fn requests(seen: &Mutex<Vec<(String, Vec<u8>)>>) -> Vec<String> {
        seen.lock().unwrap().iter().map(|(r, _)| r.clone()).collect()
    }

    fn record_of(records: &Path) -> Option<Record> {
        read_records(records).into_iter().next()
    }

    /// Update the shared capture: what Orbiters has asked first (its revision), the files made for the edits now sent to
    /// replace that revision's (each tree, the start view, the revision named), its stages said; the record then keeps
    /// the edits sent and the item at the next revision, and the entry no longer asks for an update.
    #[test]
    fn an_edited_upload_is_updated_in_one_step_and_its_record_keeps_the_edits_sent() {
        let (dir, session, records, mut entry) = uploaded_session("one-step", None);
        let generation = delete_splat(&session, 4);
        entry.edits = Some(Ok(generation.clone()));
        assert!(entry.edited_since_upload());
        let (api, seen) = stand_in(vec![("GET /captures/capture/3", 200, capture_at(3, 1)), ("PUT /captures/capture/3/files", 200, capture_at(3, 2))]);
        let stages = Mutex::new(Vec::new());
        let (item, edits) =
            update_with(&api, "token", Ok(records.as_path()), &entry, &|s| stages.lock().unwrap().push(s), &mut Vec::new(), &prepared).unwrap();
        assert_eq!((item.capture.as_ref().map(|c| c.revision), edits.as_str()), (Some(2), generation.as_str()));
        assert_eq!(requests(&seen), ["GET /captures/capture/3", "PUT /captures/capture/3/files"]);
        let body = String::from_utf8_lossy(&seen.lock().unwrap()[1].1).into_owned();
        for part in ["name=\"vrchatPc\"", "name=\"vrchatQuest\"", "name=\"vrchatCoarse\"", "\"revision\":1", "\"quality\":31.5", "\"position\":[0.0,1.6,0.0]"] {
            assert!(body.contains(part), "the replacement sends {part}");
        }
        assert!(!body.contains("name=\"sky\"") && !body.contains("name=\"picture\""), "no sky, no picture drawn: none sent (the old ones go)");
        let said = stages.into_inner().unwrap();
        assert_eq!(said.first(), Some(&Stage::Preparing));
        assert_eq!(said.last(), Some(&Stage::Replacing), "every byte sent: Orbiters switches");
        assert!(said.iter().any(|s| matches!(s, Stage::Sending { .. })));
        let record = record_of(&records).unwrap();
        assert_eq!(record.edits.as_deref(), Some(generation.as_str()));
        assert_eq!(record.item.and_then(|i| i.capture).map(|c| c.revision), Some(2));
        entry.uploaded_edits = Some(generation);
        assert!(!entry.edited_since_upload(), "updated: nothing to update any more");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Sharing a capture edited since it was uploaded updates it first (its files replaced, then the share asked), never
    /// shares the copy with what was removed; edited back to what was uploaded later, a share updates it again. Damaged
    /// edits refuse before anything is asked of Orbiters.
    #[test]
    fn a_share_of_an_edited_upload_updates_it_first() {
        let (dir, session, records, entry) = uploaded_session("share", None);
        let generation = delete_splat(&session, 4);
        let answers = || {
            vec![
                ("GET /captures/capture/3", 200, capture_at(3, 1)),
                ("PUT /captures/capture/3/files", 200, capture_at(3, 2)),
                ("POST /captures/capture/3/share", 200, shared_item(Kind::Capture, 3)),
            ]
        };
        let (api, seen) = stand_in(answers());
        share_with(&api, "token", Ok(records.clone()), &entry, &to_a_friend(), &|_| {}, &mut Vec::new(), &prepared).unwrap();
        assert_eq!(requests(&seen), ["GET /captures/capture/3", "PUT /captures/capture/3/files", "POST /captures/capture/3/share"]);
        assert_eq!(record_of(&records).unwrap().edits.as_deref(), Some(generation.as_str()), "kept across the share's answer");
        // Shared again with the same edits: one request.
        let (api, seen) = stand_in(answers());
        share_with(&api, "token", Ok(records.clone()), &entry, &to_a_friend(), &|_| {}, &mut Vec::new(), &prepared).unwrap();
        assert_eq!(requests(&seen), ["POST /captures/capture/3/share"]);
        // Damaged edits: refused, nothing asked.
        let edit_file = orbiters_core::reconstruction::result_dir(&session).join(orbiters_core::reconstruction::EDIT_FILE);
        let kept = std::fs::read(&edit_file).unwrap();
        std::fs::write(&edit_file, b"damaged").unwrap();
        let (api, seen) = stand_in(answers());
        let refused = share_with(&api, "token", Ok(records.clone()), &entry, &to_a_friend(), &|_| {}, &mut Vec::new(), &prepared).unwrap_err();
        assert!(refused.to_string().contains("could not be read"), "{refused}");
        assert!(requests(&seen).is_empty());
        // Every splat brought back (the file gone: as trained): the share updates it again, to the capture as trained.
        std::fs::write(&edit_file, kept).unwrap();
        std::fs::remove_file(&edit_file).unwrap();
        let (api, seen) = stand_in(answers());
        share_with(&api, "token", Ok(records.clone()), &entry, &to_a_friend(), &|_| {}, &mut Vec::new(), &prepared).unwrap();
        assert_eq!(requests(&seen).len(), 3);
        assert_eq!(record_of(&records).unwrap().edits.as_deref(), Some(orbiters_reconstruct::edit::UNEDITED));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An update Orbiters refuses changes nothing here either (the record keeps the edits of the files there) and says
    /// that everyone still sees the version shared before; tried again, it goes through.
    #[test]
    fn a_failed_update_keeps_the_previous_version_and_says_so() {
        let (dir, session, records, mut entry) = uploaded_session("failed", None);
        entry.edits = Some(Ok(delete_splat(&session, 2)));
        let (api, seen) = stand_in(vec![
            ("GET /captures/capture/3", 200, capture_at(3, 1)),
            ("PUT /captures/capture/3/files", 502, r#"{"error":"Cloud storage upload failed for file 81: fixture."}"#.into()),
            ("PUT /captures/capture/3/files", 200, capture_at(3, 2)),
        ]);
        let failed = update_with(&api, "token", Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &prepared).unwrap_err();
        assert_eq!(
            not_updated(&failed),
            "Not updated: Cloud storage upload failed for file 81: fixture. Everyone still sees the version shared before."
        );
        assert_eq!(record_of(&records).unwrap().edits, None, "the record still says the files there are as trained");
        update_with(&api, "token", Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &prepared).unwrap();
        assert_eq!(requests(&seen), ["GET /captures/capture/3", "PUT /captures/capture/3/files", "GET /captures/capture/3", "PUT /captures/capture/3/files"]);
        assert!(record_of(&records).unwrap().edits.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Replaced from somewhere else meanwhile (409 revision-conflict): said with the revision it is at, nothing changed
    /// here. Removed from Orbiters: said, and its record forgotten so that a share uploads it again.
    #[test]
    fn a_conflict_or_a_removed_upload_is_said_and_changes_nothing_else() {
        let (dir, session, records, mut entry) = uploaded_session("conflict", None);
        entry.edits = Some(Ok(delete_splat(&session, 2)));
        let conflict = r#"{"error":"This capture was updated meanwhile: load it again, then update it.","code":"revision-conflict","revision":5}"#;
        let (api, _) = stand_in(vec![("GET /captures/capture/3", 200, capture_at(3, 4)), ("PUT /captures/capture/3/files", 409, conflict.into())]);
        let refused = update_with(&api, "token", Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &prepared).unwrap_err().to_string();
        assert!(refused.contains("updated from somewhere else") && refused.contains("revision 5") && refused.contains("replaced revision 4"), "{refused}");
        assert_eq!(record_of(&records).unwrap().edits, None);
        let gone = r#"{"error":"This capture or picture is not available."}"#;
        let (api, seen) = stand_in(vec![("GET /captures/capture/3", 404, gone.into())]);
        let refused = update_with(&api, "token", Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &prepared).unwrap_err().to_string();
        assert!(refused.contains("not on Orbiters any more"), "{refused}");
        assert_eq!(requests(&seen), ["GET /captures/capture/3"], "nothing prepared or sent");
        assert!(record_of(&records).is_none(), "its record forgotten: a share uploads it again");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The connection lost after the files went: the capture's revision says whether Orbiters switched to them (one more
    /// than the one replaced: it did, and the update counts) or not (the failure stands).
    #[test]
    fn a_lost_answer_is_read_from_the_revision_the_capture_is_at() {
        let (dir, session, records, mut entry) = uploaded_session("lost", None);
        entry.edits = Some(Ok(delete_splat(&session, 2)));
        let (api, _) = stand_in(vec![
            ("GET /captures/capture/3", 200, capture_at(3, 1)),
            ("PUT /captures/capture/3/files", 0, String::new()),
            ("GET /captures/capture/3", 200, capture_at(3, 2)),
        ]);
        let (item, _) = update_with(&api, "token", Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &prepared).unwrap();
        assert_eq!(item.capture.map(|c| c.revision), Some(2));
        assert!(record_of(&records).unwrap().edits.is_some());
        let (dir2, session2, records2, mut entry2) = uploaded_session("lost-not-switched", None);
        entry2.edits = Some(Ok(delete_splat(&session2, 2)));
        let (api, _) = stand_in(vec![("GET /captures/capture/3", 200, capture_at(3, 1)), ("PUT /captures/capture/3/files", 0, String::new())]);
        assert!(update_with(&api, "token", Ok(records2.as_path()), &entry2, &|_| {}, &mut Vec::new(), &prepared).is_err());
        assert_eq!(record_of(&records2).unwrap().edits, None);
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(dir2).unwrap();
    }

    /// Edited again while its files were made: not sent (they would hold neither the edits before nor those after).
    #[test]
    fn edits_changed_while_prepared_are_not_sent() {
        let (dir, session, records, mut entry) = uploaded_session("moving", None);
        entry.edits = Some(Ok(delete_splat(&session, 2)));
        let (api, seen) = stand_in(vec![("GET /captures/capture/3", 200, capture_at(3, 1))]);
        let moving = |s: &Path| {
            let made = prepared(s);
            delete_splat(s, 6);
            made
        };
        let refused = update_with(&api, "token", Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &moving).unwrap_err().to_string();
        assert!(refused.contains("changed while it was being prepared"), "{refused}");
        assert_eq!(requests(&seen), ["GET /captures/capture/3"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Which captures ask to be updated: uploaded from this computer (or known to Orbiters as this member's) with other
    /// edits than those it was uploaded with; never an unreadable edit, a picture, someone else's or one never uploaded.
    #[test]
    fn only_an_upload_edited_since_asks_to_be_updated() {
        let unedited = orbiters_reconstruct::edit::UNEDITED.to_string();
        let mut e = remote_entry(Item::default());
        e.remote = None;
        e.local = Some(Local::Session(PathBuf::from("C:/Sessions/Home")));
        e.edits = Some(Ok("aa-bb".into()));
        assert!(!e.edited_since_upload(), "never uploaded");
        e.uploaded_edits = Some(unedited.clone());
        assert!(e.edited_since_upload());
        e.uploaded_edits = Some("aa-bb".into());
        assert!(!e.edited_since_upload(), "uploaded with these edits");
        e.uploaded_edits = None;
        e.remote = Some(Item { kind: Kind::Capture, id: 3, mine: true, ..Default::default() });
        assert!(e.edited_since_upload(), "known to Orbiters only: uploaded as trained");
        e.edits = Some(Ok(unedited));
        assert!(!e.edited_since_upload());
        e.edits = Some(Err("damaged".into()));
        assert!(!e.edited_since_upload(), "unreadable edits stop whatever is asked, with why");
        e.edits = Some(Ok("aa-bb".into()));
        e.remote.as_mut().unwrap().mine = false;
        assert!(!e.edited_since_upload(), "someone else's");
        e.remote = None;
        e.uploaded_edits = Some("cc-dd".into());
        e.local = Some(Local::Picture(PathBuf::from("C:/Pictures/a.png")));
        assert!(!e.edited_since_upload(), "a picture");
    }

    /// The tray's job of an update says it is one (its card says Updating…), and its outcome carries the edits sent.
    #[test]
    fn an_update_job_is_marked_and_answers_its_item_and_edits() {
        let ctl = super::super::Controller::new(orbiters_core::settings::Settings::default());
        let request = Request { id: "update-job".into(), server: Server::Development, entry: remote_entry(Item::default()), extra: Vec::new(), operation: Operation::Update };
        let json = serde_json::to_string(&crate::ipc::ToService::CaptureShare { request: Box::new(request.clone()) }).unwrap();
        assert!(matches!(serde_json::from_str::<crate::ipc::ToService>(&json).unwrap(), crate::ipc::ToService::CaptureShare { .. }));
        ctl.capture_share_with(request, |_, report| {
            report(Stage::Replacing);
            Ok(Outcome::Updated { item: Item { id: 3, ..Default::default() }, edits: "aa-bb".into() })
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while ctl.view().capture_shares[0].outcome.is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let job = ctl.view().capture_shares[0].clone();
        assert!(job.update);
        assert!(matches!(job.outcome, Some(Outcome::Updated { ref edits, .. }) if edits == "aa-bb"));
    }

    /// The removed splats' positions as the files write them (right-handed: Z negated, exact), bit for bit.
    fn positions_of(g: &orbiters_reconstruct::gaussians::Gaussians, ids: impl Iterator<Item = usize>) -> std::collections::HashSet<[u32; 3]> {
        ids.map(|i| [g.means[i][0].to_bits(), g.means[i][1].to_bits(), (-g.means[i][2]).to_bits()]).collect()
    }

    /// How many 32-byte splat records of `bytes` (from byte `from`) sit at one of `removed`, of how many.
    fn removed_in(bytes: &[u8], from: usize, removed: &std::collections::HashSet<[u32; 3]>) -> (usize, usize) {
        let records = bytes[from..].chunks_exact(32);
        let total = records.len();
        let found = records.filter(|r| removed.contains(&[0, 4, 8].map(|o| u32::from_le_bytes(r[o..o + 4].try_into().unwrap())))).count();
        (found, total)
    }

    /// Where a splat tree's splats start (after its header and boxes).
    fn tree_splats(bytes: &[u8]) -> usize {
        (1 + u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize) * 32
    }

    /// Update the shared capture from end to end on a disposable Orbiters (its backend on a disposable database, its
    /// storage a folder: reports/reshare.md says how to start one), with the app's own upload preparation (VRChat's trees
    /// tuned on the integrated GPU, as the app's background preparation does) on a disposable copy of a reconstructed
    /// session:
    /// `ORBITERS_TEST_API=http://127.0.0.1:4243 ORBITERS_TEST_TOKEN=… ORBITERS_TEST_WORLD=wrld_… ORBITERS_TEST_SESSION=<copy>
    /// ORBITERS_TEST_STORAGE=<the server's storage folder> cargo test -p orbiters-desktop an_uploaded_capture_is_updated_end_to_end
    /// -- --ignored --nocapture --test-threads=1`.
    /// Shared publicly as trained; a fifth of its splats deleted; updated: the item at revision 2 without the old picture,
    /// the website's `.splat`, the VRChat slot's three trees and the listing hold none of the deleted splats, the old
    /// files are gone from storage and their cached addresses purged. Then a slot refusing the new trees: not updated,
    /// everything as at revision 2. A replacement of a stale revision: refused as a conflict. Updated again: revision 3.
    #[test]
    #[ignore]
    fn an_uploaded_capture_is_updated_end_to_end() {
        use orbiters_reconstruct::edit::{Edit, Identity, DELETED};
        let env = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k}"));
        let api = Api::at(&env("ORBITERS_TEST_API"));
        let token = env("ORBITERS_TEST_TOKEN");
        let session = PathBuf::from(env("ORBITERS_TEST_SESSION"));
        let storage = PathBuf::from(env("ORBITERS_TEST_STORAGE"));
        let result = orbiters_core::reconstruction::result_dir(&session);
        let ply = result.join(orbiters_core::reconstruction::PLY_FILE);
        let _ = std::fs::remove_file(result.join(orbiters_core::reconstruction::EDIT_FILE));
        let records = std::env::temp_dir().join(format!("orbiters-e2e-records-{}.json", uuid::Uuid::new_v4()));
        let mut entry = remote_entry(Item::default());
        entry.remote = None;
        entry.world_id = env("ORBITERS_TEST_WORLD");
        entry.world_name = "Exoplanet Journey".into();
        entry.local = Some(Local::Session(session.clone()));
        entry.source_key = session.file_name().map(|n| n.to_string_lossy().into_owned());
        let stored = || -> std::collections::BTreeSet<String> {
            let mut out = std::collections::BTreeSet::new();
            let mut stack = vec![storage.clone()];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    if e.path().is_dir() {
                        stack.push(e.path());
                    } else if e.path().to_string_lossy().contains("files") {
                        out.insert(e.path().strip_prefix(&storage).unwrap().to_string_lossy().replace('\\', "/"));
                    }
                }
            }
            out
        };
        // The stand-in storage's objects: one file each, the key's '/' written '~'.
        let slot_file = |slot: u32, name: &str| std::fs::read(storage.join("public-bucket").join(format!("public~vrchat~slots~{slot}~{name}"))).ok();
        let listed = |id: u64| -> serde_json::Value {
            let r = api.agent().get(&api.url("maps/vrchat/previews")).call().unwrap();
            let v: serde_json::Value = serde_json::from_str(&r.into_body().read_to_string().unwrap()).unwrap();
            v["previews"].as_array().unwrap().iter().find(|p| p["id"].as_u64() == Some(id)).cloned().expect("listed for worlds")
        };
        // The app's preparation of a capture's upload files on the integrated GPU (as its background preparation runs
        // it: DeviceOptions::low_power), which leaves the lab's graphics card to its queue.
        let igpu = |s: &Path| super::super::maps::prepare(s, orbiters_reconstruct::gpu::DeviceOptions { low_power: true, ..Default::default() });
        let t = Instant::now();

        // What earlier runs left in the stand-in storage (this run looks only at what it makes).
        let existing = stored();

        // 1. Shared publicly, as trained (the app's share: prepared, uploaded with its picture, shared).
        let shared = share_with(&api, &token, Ok(records.clone()), &entry, &shares::Share { public: true, ..Default::default() }, &|_| {}, &mut Vec::new(), &igpu)
            .unwrap();
        let shares::Shared::Item { item } = shared else { panic!("shared as its owner") };
        let id = item.id;
        let slot = item.capture.as_ref().and_then(|c| c.vrchat_slot).expect("a public capture holds a VRChat slot");
        assert!(item.capture.as_ref().is_some_and(|c| c.picture_url.is_some() && c.revision == 1));
        let first_files: std::collections::BTreeSet<String> = stored().difference(&existing).cloned().collect();
        println!("1. shared publicly in {:.1} s: capture {id}, slot {slot}, {} stored objects", t.elapsed().as_secs_f32(), first_files.len());

        // 2. A fifth of its splats deleted in its 3D view (those farthest along X).
        let g = orbiters_reconstruct::ply::read(&ply).unwrap();
        let n = g.means.len();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|a, b| g.means[*b][0].total_cmp(&g.means[*a][0]));
        let gone: Vec<usize> = order[..n / 5].to_vec();
        let mut edit = Edit::new(n, Identity::read(&ply).unwrap());
        for &i in &gone {
            edit.marks[i] = DELETED;
        }
        // As the 3D view saves a change of what leaves the capture: its picture not drawn for it yet.
        edit.picture = false;
        edit.save(&result).unwrap();
        let removed = positions_of(&g, gone.iter().copied());
        entry.edits = Some(Ok(edit.generation()));
        entry.uploaded_edits = record_of(&records).map(|r| r.edits.unwrap_or_else(|| orbiters_reconstruct::edit::UNEDITED.into()));
        assert!(entry.edited_since_upload(), "its window offers Update the shared capture");
        let pc_before = slot_file(slot, "pc").unwrap();
        assert!(removed_in(&pc_before, tree_splats(&pc_before), &removed).0 > 0, "the shared copy holds the splats deleted");

        // 3. Update the shared capture.
        let t = Instant::now();
        let stages = Mutex::new(Vec::new());
        let (item, made_with) =
            update_with(&api, &token, Ok(records.as_path()), &entry, &|s| stages.lock().unwrap().push(s), &mut Vec::new(), &igpu)
                .unwrap();
        let capture = item.capture.clone().unwrap();
        assert_eq!((capture.revision, made_with.as_str()), (2, edit.generation().as_str()));
        assert!(capture.preview_url.ends_with("?v=2"));
        assert!(capture.picture_url.is_none(), "its picture, drawn before the edit, is not shown any more");
        println!("3. updated in {:.1} s; stages {:?}", t.elapsed().as_secs_f32(), stages.lock().unwrap().iter().filter(|s| !matches!(s, Stage::Sending { .. })).collect::<Vec<_>>());
        // What the website loads: none of the deleted splats.
        let web = std::env::temp_dir().join(format!("orbiters-e2e-{id}.splat"));
        api.capture_file(&token, Kind::Capture, id, &web).unwrap();
        let web = std::fs::read(web).unwrap();
        let (found, total) = removed_in(&web, 0, &removed);
        assert_eq!(found, 0, "the website's .splat holds none of the {} deleted splats ({total} splats)", removed.len());
        assert!(total as u64 <= (n - gone.len()) as u64 && total > 0);
        // What VRChat worlds load: the slot's three trees under the new key, none of the deleted splats; the listing.
        let key = |b: &[u8]| u32::from_le_bytes(b[20..24].try_into().unwrap());
        let pc = slot_file(slot, "pc").unwrap();
        for name in ["pc", "quest", "coarse"] {
            let tree = slot_file(slot, name).unwrap();
            assert_eq!(key(&tree), key(&pc));
            assert_eq!(removed_in(&tree, tree_splats(&tree), &removed).0, 0, "slot {name} holds none of the deleted splats");
        }
        assert_ne!(key(&pc), key(&pc_before));
        let entry_listed = listed(id);
        assert_eq!((entry_listed["key"].as_u64(), entry_listed["revision"].as_u64(), entry_listed["slot"].as_u64()), (Some(key(&pc) as u64), Some(2), Some(slot as u64)));
        // The old files are gone from storage; their cached addresses (and the slot's) purged.
        let second_files = stored();
        let left: Vec<_> = first_files.intersection(&second_files).filter(|k| !k.contains("vrchat/slots")).collect();
        assert!(left.is_empty(), "old objects left: {left:?}");
        let purges = std::fs::read_to_string(storage.join("purges.jsonl")).unwrap_or_default();
        assert!(purges.contains(&format!("public/vrchat/slots/{slot}/pc")), "the slot's cached files purged");
        assert_eq!(record_of(&records).unwrap().edits.as_deref(), Some(edit.generation().as_str()));
        entry.uploaded_edits = Some(made_with);
        assert!(!entry.edited_since_upload());

        // 4. More deleted, and the slot refusing the new trees: not updated, everything as at revision 2.
        for &i in &order[n / 5..n / 4] {
            edit.marks[i] = DELETED;
        }
        edit.picture = false;
        edit.save(&result).unwrap();
        entry.edits = Some(Ok(edit.generation()));
        std::fs::write(storage.join("fail-slot-quest"), b"").unwrap();
        let failed = update_with(&api, &token, Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &igpu).unwrap_err();
        std::fs::remove_file(storage.join("fail-slot-quest")).unwrap();
        println!("4. refused as expected: {}", not_updated(&failed));
        let still = api.capture_item(&token, Kind::Capture, id).unwrap();
        assert_eq!(still.capture.as_ref().map(|c| c.revision), Some(2));
        assert_eq!(stored().into_iter().filter(|k| !k.contains("vrchat/slots")).collect::<Vec<_>>(), second_files.iter().filter(|k| !k.contains("vrchat/slots")).cloned().collect::<Vec<_>>(), "revision 2's files, and only them");
        let listed_now = listed(id);
        let pc_now = slot_file(slot, "pc");
        match listed_now["slot"].as_u64() {
            Some(s) => assert_eq!((s, pc_now.as_deref().map(key)), (slot as u64, Some(key(&pc))), "its slot serves revision 2's trees again"),
            None => assert!(pc_now.is_none(), "or it gave its slot up, emptied"),
        }
        assert_eq!(record_of(&records).unwrap().edits.as_deref(), entry.uploaded_edits.as_deref(), "the record still says revision 2's edits");

        // 5. Another computer's replacement of revision 1 (stale): a conflict naming revision 2.
        let stale = igpu(&session).unwrap();
        let conflict = api.replace_capture_files(&token, id, 1, stale.files, stale.sky.as_deref(), None, &stale.metadata, |_, _| {}).unwrap_err();
        assert_eq!(conflict.downcast_ref::<shares::RevisionConflict>().map(|c| c.revision), Some(2));

        // 6. Updated again: revision 3.
        let (item, _) = update_with(&api, &token, Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &igpu).unwrap();
        assert_eq!(item.capture.map(|c| c.revision), Some(3));
        println!("6. updated again: revision 3; {} splats deleted in all", n / 4);
        let _ = std::fs::remove_file(records);
    }

    /// An item uploaded before (its record says so) is shared with one request: nothing sent again, nothing fetched.
    #[test]
    fn an_uploaded_item_is_shared_with_one_request() {
        let dir = std::env::temp_dir().join(format!("orbiters-share-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join("Silent Dawn 2026-10-28 23-52-12");
        let records = dir.join("capture-uploads.json");
        let uploaded = Uploaded { kind: Kind::Capture, id: 3, visibility: "private".into() };
        std::fs::write(&records, serde_json::to_vec(&[Record { path: session.clone(), item: None, uploaded, edits: None }]).unwrap()).unwrap();
        let (api, seen) = fake_orbiters(vec![("POST /captures/capture/3/share", shared_item(Kind::Capture, 3))]);
        let mut entry = remote_entry(Item::default());
        entry.remote = None;
        entry.local = Some(Local::Session(session.clone()));
        let stages = Mutex::new(Vec::new());
        let result = share_with(&api, "token", Ok(records.clone()), &entry, &to_a_friend(), &|s| stages.lock().unwrap().push(s), &mut Vec::new(), &prepared).unwrap();
        assert!(matches!(result, shares::Shared::Item { ref item } if item.grants.len() == 1));
        assert_eq!(*seen.lock().unwrap(), ["POST /captures/capture/3/share"]);
        assert_eq!(*stages.lock().unwrap(), [Stage::Sharing]);
        let saved = read_records(&records);
        assert_eq!(saved[0].item.as_ref().map(|i| i.id), Some(3), "the item the share answered is remembered");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A picture not uploaded yet: sent once, shared with the identity the upload answered (no fetch in between), and
    /// that identity remembered before the share is asked.
    #[test]
    fn a_new_picture_is_sent_then_shared_without_fetching_it() {
        let dir = std::env::temp_dir().join(format!("orbiters-share-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let picture = dir.join("VRChat_2026-10-29_19-00-00.000_1920x1080.png");
        std::fs::write(&picture, [7u8; 4096]).unwrap();
        let records = dir.join("capture-uploads.json");
        let world = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";
        let added = r#"{"added":[{"type":"picture","id":9,"visibility":"private"}],"skipped":0}"#.to_string();
        let (api, seen) = fake_orbiters(vec![(r"POST /maps/worlds/", added), ("POST /captures/picture/9/share", shared_item(Kind::Picture, 9))]);
        let mut entry = remote_entry(Item::default());
        entry.remote = None;
        entry.world_id = world.into();
        entry.source_key = Some("a".repeat(64));
        entry.local = Some(Local::Picture(picture.clone()));
        let stages = Mutex::new(Vec::new());
        share_with(&api, "token", Ok(records.clone()), &entry, &to_a_friend(), &|s| stages.lock().unwrap().push(s), &mut Vec::new(), &prepared).unwrap();
        assert_eq!(*seen.lock().unwrap(), [format!("POST /maps/worlds/{world}/pictures"), "POST /captures/picture/9/share".to_string()]);
        assert_eq!(*stages.lock().unwrap(), [Stage::Sending { sent: 0, total: 4096 }, Stage::Sharing]);
        let saved = read_records(&records);
        assert_eq!((saved[0].uploaded.id, saved[0].item.as_ref().map(|i| i.id)), (9, Some(9)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A world's pictures go three at a time, their count shared said as it grows; after a failure none is started, and
    /// the failure is the answer.
    #[test]
    fn several_items_are_shared_three_at_a_time() {
        let entries: Vec<Entry> = (0..7).map(|i| remote_entry(Item { kind: Kind::Picture, id: i, ..Default::default() })).collect();
        let flight = Mutex::new((0, 0));
        let stages = Mutex::new(Vec::new());
        let one = |e: Entry| {
            {
                let mut f = flight.lock().unwrap();
                f.0 += 1;
                f.1 = f.1.max(f.0);
            }
            std::thread::sleep(std::time::Duration::from_millis(60));
            flight.lock().unwrap().0 -= 1;
            Ok(shares::Shared::Item { item: e.remote.unwrap() })
        };
        let first = share_all(entries.clone(), &|s| stages.lock().unwrap().push(s), one).unwrap();
        assert!(matches!(first, shares::Shared::Item { ref item } if item.id == 0), "the first item's answer");
        assert_eq!(flight.lock().unwrap().1, SHARES_AT_ONCE);
        let said = stages.into_inner().unwrap();
        assert_eq!((said.first(), said.last(), said.len()), (Some(&Stage::Items { done: 0, total: 7 }), Some(&Stage::Items { done: 7, total: 7 }), 8));
        let started = Mutex::new(0);
        let failing = |e: Entry| {
            *started.lock().unwrap() += 1;
            std::thread::sleep(std::time::Duration::from_millis(30));
            if e.remote.as_ref().unwrap().id == 1 {
                anyhow::bail!("Orbiters could not be reached")
            }
            Ok(shares::Shared::Item { item: e.remote.unwrap() })
        };
        let error = share_all(entries, &|_| {}, failing).unwrap_err();
        assert_eq!(error.to_string(), "Orbiters could not be reached");
        assert!(*started.lock().unwrap() < 7, "none started after the failure");
    }

    /// Two changes of one item wait for each other (the second says so); another item's goes at once.
    #[test]
    fn changes_wait_only_for_the_same_item() {
        let item = |name: &str| {
            let mut e = remote_entry(Item::default());
            e.local = Some(Local::Session(PathBuf::from(format!("C:/Sessions/{name}-{}", uuid::Uuid::new_v4()))));
            e
        };
        let (a, b) = (item("a"), item("b"));
        let held = Changing::of(&a, &|_| {});
        let other = std::time::Instant::now();
        drop(Changing::of(&b, &|_| panic!("another item never waits")));
        assert!(other.elapsed().as_millis() < 500);
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let said = Mutex::new(Vec::new());
            let _second = Changing::of(&a, &|s| said.lock().unwrap().push(s));
            tx.send(said.into_inner().unwrap()).unwrap();
        });
        assert!(rx.recv_timeout(std::time::Duration::from_millis(300)).is_err(), "the same item waits");
        drop(held);
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap(), [Stage::Waiting]);
        waiter.join().unwrap();
    }

    #[test]
    fn recording_start_saves_the_actual_osc_names_even_when_false() {
        let dir = std::env::temp_dir().join(format!("orbiters-content-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let ctl = super::super::Controller::new(orbiters_core::settings::Settings::default());
        ctl.osc_values.lock().unwrap().latest.record("/avatar/parameters/NSFW/Toggle".into(), vec![orbiters_core::osc::OscValue::Bool(false)], 0);
        ctl.on_worker_event(orbiters_core::protocol::WorkerEvent::Started { session_dir: dir.clone(), source: "fixture".into() });
        let content = Content::read(&dir);
        assert!(content.explicit);
        assert_eq!(content.explicit_reasons, ["osc-parameter:NSFW/Toggle"]);
        assert!(dir.join(orbiters_core::explicit::FILE).is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }
    /// A multi-layer picture's layer, chosen in its window, is shared as its own file: its path, its own record's item,
    /// no layers; the picture keeps its own.
    #[test]
    fn a_chosen_layer_is_an_entry_of_its_own() {
        let layer = |name: &str, shared: Option<Item>| Layer {
            path: PathBuf::from(format!("C:/P/VRChat_2026-10-29_21-04-11.532_1920x1080_{name}.png")),
            name: name.into(),
            width: 1920,
            height: 1080,
            shared,
        };
        let mut e = remote_entry(Item::default());
        e.remote = None;
        e.key = "C:/P/VRChat_2026-10-29_21-04-11.532_1920x1080.png".into();
        e.local = Some(Local::Picture(PathBuf::from(&e.key)));
        e.source_key = Some("composite-hash".into());
        e.layers = vec![layer("Environment", None), layer("Player", Some(Item { kind: Kind::Picture, id: 7, mine: true, ..Default::default() }))];
        assert!(e.has_composite());
        let player = e.layer_entry(1).unwrap();
        assert_eq!(player.key, e.layers[1].path.to_string_lossy());
        assert!(matches!(&player.local, Some(Local::Picture(p)) if *p == e.layers[1].path));
        assert_eq!(player.remote.as_ref().map(|i| i.id), Some(7));
        assert!(player.layers().is_empty() && player.source_key.is_none());
        assert_eq!((player.world_id.as_str(), player.size), (e.world_id.as_str(), Some([1920, 1080])));
        // The picture deleted: the entry's own file is its bottom layer, whose fingerprint it keeps.
        e.local = Some(Local::Picture(e.layers[0].path.clone()));
        assert!(!e.has_composite());
        assert_eq!(e.layer_entry(0).unwrap().source_key.as_deref(), Some("composite-hash"));
        assert!(e.layer_entry(2).is_none());
    }

    #[test]
    fn source_keys_match_only_owned_items_of_the_same_kind_and_world() {
        let item = Item {
            kind: Kind::Capture,
            mine: true,
            source_key: Some("recording-folder".into()),
            world: shares::World { id: "world".into(), ..Default::default() },
            ..Default::default()
        };
        let mut entry = remote_entry(item.clone());
        entry.remote = None;
        assert!(matches_item(&entry, &item));
        let mut different = item.clone();
        different.mine = false;
        assert!(!matches_item(&entry, &different));
        different = item.clone();
        different.kind = Kind::Picture;
        assert!(!matches_item(&entry, &different));
        different = item.clone();
        different.world.id = "other".into();
        assert!(!matches_item(&entry, &different));
        different = item;
        different.source_key = None;
        assert!(!matches_item(&entry, &different));
    }
}
