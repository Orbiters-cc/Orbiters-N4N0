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
}

pub use crate::worlds::Layer;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Operation {
    Share(shares::Share),
    Unshare(shares::Unshare),
    Download,
    Answer { id: u64, approve: bool, acknowledge_explicit: bool },
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
    Failed { message: String, explicit: bool },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShareJob {
    pub id: String,
    pub key: String,
    /// Where it stands while it runs.
    pub stage: Stage,
    pub outcome: Option<Outcome>,
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
        self.capture_share_with(request, run_request);
    }
    fn capture_share_with(self: &super::Shared, request: Request, work: impl FnOnce(Request, Report) -> anyhow::Result<Outcome> + Send + 'static) {
        let accepted = self.update(|s| {
            if s.capture_shares.iter().any(|j| j.id == request.id) {
                return false;
            }
            s.capture_shares.push(ShareJob { id: request.id.clone(), key: request.entry.key.clone(), stage: Stage::Starting, outcome: None });
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
    let worlds = crate::worlds::load_gallery(root);
    let history = visited::History::load(&visited::history_file());
    let records = records_path(server).ok().map(|p| read_records(&p)).unwrap_or_default();
    let mut entries = Vec::new();
    for w in worlds {
        for s in &w.sessions {
            let taken = unix(&s.manifest.created_utc);
            let local = Local::Session(s.dir.clone());
            let remote = records.iter().find(|r| r.path == s.dir).and_then(|r| r.item.clone());
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
    if let Some(local) = &entry.local {
        let _records = RECORDS.lock().unwrap_or_else(|e| e.into_inner());
        let path = records?;
        let mut records = read_records(&path);
        records.retain(|r| r.path != local.path());
        records.push(Record {
            path: local.path().into(),
            item: Some(item.clone()),
            uploaded: Uploaded { kind: item.kind, id: item.id, visibility: if item.public { "public" } else { "private" }.into() },
        });
        orbiters_core::write_atomic(&path, &serde_json::to_vec(&records)?)?;
    }
    Ok(())
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
/// uploaded (as the gallery or its record says) is shared with one request, never sent or fetched again. How long
/// each step took goes to `%LOCALAPPDATA%/Orbiters/logs/shares.log`.
pub fn share(server: Server, entry: Entry, request: shares::Share, report: Report) -> anyhow::Result<shares::Shared> {
    let started = Instant::now();
    let mut steps = Vec::new();
    let result = api(server).and_then(|(api, token)| share_with(&api, &token, records_path(server), &entry, &request, report, &mut steps));
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
/// [`share`] through `api` as `token`, this account's upload records in `records`; what took how long in `steps`.
fn share_with(
    api: &Api,
    token: &str,
    records: anyhow::Result<PathBuf>,
    entry: &Entry,
    request: &shares::Share,
    report: Report,
    steps: &mut Vec<String>,
) -> anyhow::Result<shares::Shared> {
    let mut at = Instant::now();
    let mut step = |what: &str, at: &mut Instant| {
        steps.push(format!("{what} {:.2} s", at.elapsed().as_secs_f64()));
        *at = Instant::now();
    };
    let _one = Changing::of(entry, report);
    if at.elapsed().as_millis() > 50 {
        step("waited for its other change", &mut at);
    }
    let records = records.as_deref().map_err(|e| anyhow::anyhow!("{e}"));
    let remembered = entry
        .local
        .as_ref()
        .and_then(|l| records.as_ref().ok().and_then(|p| read_records(p).into_iter().find(|r| r.path == l.path())))
        .map(|r| (r.uploaded.kind, r.uploaded.id));
    let existing = entry.remote.as_ref().map(|i| (i.kind, i.id)).or(remembered);
    let uploaded_now = existing.is_none();
    let (kind, id) = if let Some(known) = existing {
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
                let (files, sky, view) = super::maps::web_preview(dir)?;
                let picture = crate::capture_picture::jpeg(dir);
                step("prepared", &mut at);
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
                .inspect(|_| step(&format!("sent {:.1} MB and stored", bytes as f64 / 1e6), &mut at))?
            }
            Local::Picture(path) => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let part = Part::file("pictures", name.as_ref(), "image/png", path)?;
                metadata["takenAt"] = serde_json::json!([iso(entry.taken)]);
                report(Stage::Sending { sent: 0, total: std::fs::metadata(path).map_or(0, |m| m.len()) });
                let result = api.add_shared_pictures(token, &entry.world_id, vec![part], &metadata)?;
                step("sent and stored", &mut at);
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
            known.push(Record { path: local.path().into(), item: None, uploaded: uploaded.clone() });
            orbiters_core::write_atomic(file, &serde_json::to_vec(&known)?)?;
        }
        (uploaded.kind, uploaded.id)
    };
    report(Stage::Sharing);
    let shared = api.share_capture(token, kind, id, request);
    step("shared", &mut at);
    match shared {
        Ok(mut result) => {
            if let shares::Shared::Item { item } = &mut result {
                // A public capture uploaded without its picture gets it now: its public page's link previews show it.
                if let (true, Kind::Capture, true, Some(Local::Session(dir))) = (item.public, item.kind, item.mine, entry.local.as_ref()) {
                    if item.capture.as_ref().is_some_and(|c| c.picture_url.is_none()) {
                        match crate::capture_picture::jpeg(dir).map(|jpeg| api.set_capture_picture(token, item.id, jpeg)) {
                            Some(Ok(with_picture)) => {
                                *item = with_picture;
                                step("gave it its picture", &mut at);
                            }
                            Some(Err(e)) => step(&format!("its picture failed ({e})"), &mut at),
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
mod tests {
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

    /// An item uploaded before (its record says so) is shared with one request: nothing sent again, nothing fetched.
    #[test]
    fn an_uploaded_item_is_shared_with_one_request() {
        let dir = std::env::temp_dir().join(format!("orbiters-share-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join("Silent Dawn 2026-10-28 23-52-12");
        let records = dir.join("capture-uploads.json");
        let uploaded = Uploaded { kind: Kind::Capture, id: 3, visibility: "private".into() };
        std::fs::write(&records, serde_json::to_vec(&[Record { path: session.clone(), item: None, uploaded }]).unwrap()).unwrap();
        let (api, seen) = fake_orbiters(vec![("POST /captures/capture/3/share", shared_item(Kind::Capture, 3))]);
        let mut entry = remote_entry(Item::default());
        entry.remote = None;
        entry.local = Some(Local::Session(session.clone()));
        let stages = Mutex::new(Vec::new());
        let result = share_with(&api, "token", Ok(records.clone()), &entry, &to_a_friend(), &|s| stages.lock().unwrap().push(s), &mut Vec::new()).unwrap();
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
        share_with(&api, "token", Ok(records.clone()), &entry, &to_a_friend(), &|s| stages.lock().unwrap().push(s), &mut Vec::new()).unwrap();
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
