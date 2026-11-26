//! Background jobs for the gallery and its sharing dialogs. No disk or HTTP work in the painting code.
use orbiters_core::locks::*;
use orbiters_core::{
    account::{self, Api, Server},
    explicit::Content,
    maps::{Part, Uploaded},
    shares::{self, Item, Kind},
    visited,
};
use serde::{Deserialize, Serialize};
use orbiters_i18n::{mark, tr};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::Instant;

/// The sharing contract keeps the member's world captures wanted while they are in VRChat. Screenshots do not
/// start the controller, so they never send presence. Failed calls back off; a world change is sent on the next tick.
pub(super) fn spawn_presence(ctl: super::Shared) {
    let presence = std::thread::Builder::new().name("presence".into()).spawn(move || {
        let mut last = None;
        // "Five minutes ago", or the clock's start when the computer started less than five minutes ago: an `Instant`
        // before the boot cannot be made, and `now - 300 s` killed this thread after a cold boot (REL-5).
        let mut sent = ago(std::time::Duration::from_secs(300));
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
    if let Err(e) = presence {
        eprintln!("capture presence: {e}");
    }
}

/// `since` before now, or the clock's start when the computer started less recently than that (`Instant - Duration`
/// panics before the boot, which is the clock's zero on Windows).
pub(crate) fn ago(since: std::time::Duration) -> Instant {
    let now = Instant::now();
    now.checked_sub(since).unwrap_or(now)
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
    /// A recording whose frames never had the capture strip: no keyframes, nothing to make a 3D view from.
    #[serde(default)]
    pub no_keyframes: bool,
    /// What it is a 3D picture of (`picture.json`): `None` for a recording, or what is not a session here.
    #[serde(default)]
    pub picture_mode: Option<orbiters_core::settings::PictureMode>,
    /// A session recorded without depth frames: it cannot be reconstructed.
    #[serde(default)]
    pub no_depth: bool,
    /// Its capture files were deleted ([`orbiters_core::cleanup::trimmed`]): with a 3D view, that view is all it has left.
    #[serde(default)]
    pub trimmed: bool,
    /// When its reconstruction was made, none without one: another value is a new result.
    #[serde(default)]
    pub result: Option<String>,
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
    /// Why Orbiters refused it, when it is one the interfaces say in their own way ([`refusal`]).
    #[serde(default)]
    pub refused: Option<Refusal>,
}

/// Refusals said in their own way (contract `night-1119/explorer-contract.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Refusal {
    /// 30 first public shares in 24 hours: nothing changed, it can be shared publicly tomorrow.
    DailyLimit,
    /// 503: the server is not ready (its start-up ranking): try again in a moment.
    NotReady,
}

/// Which [`Refusal`] an error is, if any.
pub fn refusal(e: &anyhow::Error) -> Option<Refusal> {
    if e.downcast_ref::<shares::ExplorerDailyLimit>().is_some() {
        return Some(Refusal::DailyLimit);
    }
    e.downcast_ref::<orbiters_core::account::ApiError>().filter(|a| a.status == 503).map(|_| Refusal::NotReady)
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

/// Whose a share is: the account as it was when it was accepted (its credential, its server and its records), whoever is
/// signed in by the time it runs.
#[derive(Clone)]
pub struct Whose {
    server: Server,
    api: Api,
    token: String,
    user: Option<String>,
}

impl Whose {
    /// The member of `epoch`, while they are the one signed in on `server`.
    fn of(ctl: &super::Controller, epoch: u64, server: Server) -> anyhow::Result<Self> {
        let (on, account) = ctl.member_of(epoch).ok_or_else(signed_out)?;
        anyhow::ensure!(on == server, "{}", tr("Sign in to Orbiters in Settings first."));
        Ok(Self::with(server, account))
    }

    /// The account saved for `server`, read once (what has no window behind it: the bridge, a developer's command).
    pub fn saved(server: Server) -> anyhow::Result<Self> {
        Ok(Self::with(server, account::read(server).ok_or_else(signed_out)?))
    }

    fn with(server: Server, account: account::Account) -> Self {
        Self { server, api: Api::new(server), token: account.token, user: account.user }
    }

    /// Their API and credential.
    pub fn api(&self) -> (Api, String) {
        (self.api.clone(), self.token.clone())
    }

    /// What they uploaded from this computer ([`uploads`]).
    pub fn uploads(&self) -> Vec<(PathBuf, Item)> {
        uploads_in(self.records())
    }

    /// The file of what they uploaded from this computer.
    fn records(&self) -> anyhow::Result<PathBuf> {
        records_of(self.server, self.user.as_deref())
    }
}

pub(super) fn signed_out() -> anyhow::Error {
    anyhow::anyhow!("{}", tr("Sign in to Orbiters in Settings first."))
}

fn run_request(request: Request, whose: &Whose, report: Report) -> anyhow::Result<Outcome> {
    match request.operation {
        Operation::Share(r) if request.extra.is_empty() => share_as(whose, request.entry, r, report).map(Outcome::Shared),
        Operation::Share(r) => {
            let entries = std::iter::once(request.entry).chain(request.extra).collect();
            share_all(entries, report, |entry| share_as(whose, entry, r.clone(), &|_| {})).map(Outcome::Shared)
        }
        Operation::Unshare(r) => unshare(whose, request.entry, r).map(Outcome::Unshared),
        Operation::Answer { id, approve, acknowledge_explicit } => {
            Ok(Outcome::Answered(whose.api.answer_capture_request(&whose.token, id, approve, acknowledge_explicit)?.request))
        }
        Operation::Download => download(whose, request.entry.remote.ok_or_else(|| anyhow::anyhow!("{}", tr("No uploaded item")))?).map(Outcome::Downloaded),
        Operation::Update => update(whose, request.entry, report).map(|(item, edits)| Outcome::Updated { item, edits }),
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
                *results[i].locked() = Some(result);
                report(Stage::Items { done: done.fetch_add(1, SeqCst) + 1, total });
            });
        }
    });
    let mut first = None;
    for result in results.into_iter().filter_map(|r| r.into_inner().unwrap_or_else(|e| e.into_inner())) {
        let shared = result?;
        first.get_or_insert(shared);
    }
    first.ok_or_else(|| anyhow::anyhow!("{}", tr("Nothing was shared")))
}

impl super::Controller {
    /// The tray owns each mutation until completion, even if the submitting window closes. As the member of `epoch`, whom
    /// the surface showed: their account is taken while they are known to be signed in (a handover waits), and the whole job
    /// is theirs; anyone else signed in since, nothing is done and it says so.
    pub fn capture_share(self: &super::Shared, request: Request, epoch: u64) {
        let acting = self.n4n0.acting(epoch);
        let whose = match &acting {
            Some(_) => Whose::of(self, epoch, request.server),
            None => Err(anyhow::anyhow!("{}", tr("That was for the Orbiters account signed in before: nothing was done."))),
        };
        #[cfg(test)]
        if let Some(work) = test_work(&request.entry.key) {
            return self.capture_share_with(request, move |r, report| work(r, report));
        }
        self.capture_share_with(request, move |request, report| run_request(request, &whose?, report));
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
            s.capture_shares.push(ShareJob { id: request.id.clone(), key: request.entry.key.clone(), stage: Stage::Starting, outcome: None, update, refused: None });
            true
        });
        if !accepted {
            return;
        }
        let ctl = self.clone();
        let id = request.id.clone();
        std::thread::spawn(move || {
            // Every change goes to every window: bytes at most every 100 ms, the other stages as they come.
            let last = Mutex::new(ago(std::time::Duration::from_secs(1)));
            let report = |stage: Stage| {
                if let Stage::Sending { sent, total } = stage {
                    let mut last = last.locked();
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
            let result = work(request, &report);
            let refused = result.as_ref().err().and_then(refusal);
            let outcome = result.unwrap_or_else(|e| Outcome::Failed { explicit: e.downcast_ref::<shares::ExplicitAcknowledgement>().is_some(), message: e.to_string() });
            let (kind, message) = match &outcome {
                Outcome::Failed { message, explicit: false } => (super::NoticeKind::Error, message.clone()),
                Outcome::Failed { explicit: true, .. } => {
                    (super::NoticeKind::Info, tr("Open Photos & captures to confirm sharing this explicit item publicly.").into())
                }
                Outcome::Shared(shares::Shared::Request { .. }) => (super::NoticeKind::Success, tr("Share request sent.").into()),
                Outcome::Answered(_) => (super::NoticeKind::Success, tr("Share request answered.").into()),
                Outcome::Downloaded(p) => (super::NoticeKind::Success, tr!("Saved {path}", path = p.display())),
                Outcome::Updated { .. } => (super::NoticeKind::Success, tr(UPDATED).into()),
                _ => (super::NoticeKind::Success, tr("Sharing updated.").into()),
            };
            ctl.update(|s| {
                if let Some(j) = s.capture_shares.iter_mut().find(|j| j.id == id) {
                    j.outcome = Some(outcome);
                    j.refused = refused;
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
            || !AHEAD.locked().insert(session.clone())
        {
            return;
        }
        std::thread::spawn(move || {
            let started = Instant::now();
            let failed = super::maps::web_preview(&session).err().map(|e| format!(", failed: {e}")).unwrap_or_default(); // i18n: not shown
            share_log(&format!("3D capture {} prepared ahead of its share in {:.2} s{failed}", session.display(), started.elapsed().as_secs_f64())); // i18n: not shown
            AHEAD.locked().remove(&session);
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

    /// [`Entry::edited_since_upload`] because it was reconstructed again since ([`reconstructed`]): what it says differs.
    pub fn reconstructed_since_upload(&self) -> bool {
        self.edited_since_upload() && self.uploaded_edits.as_deref() == Some(RECONSTRUCTED)
    }

    /// What its session says now ([`session_state`]), a capture's window following a reconstruction that finished.
    pub fn set_session(&mut self, s: SessionState) {
        self.ready = s.ready;
        self.no_keyframes = s.no_keyframes;
        self.picture_mode = s.picture_mode;
        self.no_depth = s.no_depth;
        self.trimmed = s.trimmed;
        self.result = s.result;
        self.edits = s.edits;
        self.uploaded_edits = s.uploaded_edits;
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
/// A number that changes when what [`local_entries_with_folders`] reads beyond the pictures' index does: the capture
/// folders (one added, removed, or written in), the visit history (as [`visited::History::revision`]: not how far the
/// logs were read), and this account's upload records. Cheap enough to ask every few seconds on a thread: it lists the
/// captures' folder and asks seven dates a capture (no file of a capture is opened), and reads the history only when
/// its file changed. Measured on the library of the computer it was written on (`orbiters-desktop --idle-frames
/// --local-revision`): see the stream's report. The app's own changes say so themselves (the library's version);
/// this is for what changes them from outside, another program deleting a capture or the service writing the history.
///
/// A file of a capture that the list does not read (its frames, its video, its splats) changes nothing here, and a
/// file written again with the same date and the same length is not seen.
#[derive(Default)]
pub struct LocalRevision {
    /// The history file as it was when last read (its date and length), and its visits' revision then.
    history: Option<((Option<std::time::SystemTime>, u64), u64)>,
}

impl LocalRevision {
    /// The revision now, for the captures under `root` and the account of `server`.
    pub fn look(&mut self, root: &Path, server: Server) -> u64 {
        self.look_at(root, &visited::history_file(), records_path(server).ok().as_deref())
    }

    /// How many captures a look counts under `root` (the folders with a manifest).
    pub fn captures(root: &Path) -> usize {
        std::fs::read_dir(root).into_iter().flatten().flatten().filter(|e| e.path().join(orbiters_core::contracts::manifest::MANIFEST_FILE).is_file()).count()
    }

    /// [`LocalRevision::look`] with the history and the upload records at these files.
    pub fn look_at(&mut self, root: &Path, history: &Path, records: Option<&Path>) -> u64 {
        use std::hash::{Hash, Hasher};
        let stamp = |path: &Path| std::fs::metadata(path).ok().map(|m| (m.modified().ok(), m.len()));
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // The captures: which folders there are, and for each what a change in it moves. Its own date and its
        // reconstruction's folder's: a file added, removed or written through a rename, as the app's files are
        // (its thumbnail, its keyframes trimmed, a result). And the date and length of each file the list reads of
        // it, should one be written in place: its manifest, what it shows, how it was taken, the reconstruction's
        // report and the splats' edits.
        let mut captures: Vec<_> = std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter_map(|dir| {
                let manifest = stamp(&dir.join(orbiters_core::contracts::manifest::MANIFEST_FILE))?;
                let result = orbiters_core::reconstruction::result_dir(&dir);
                let read = [
                    dir.join(orbiters_core::explicit::FILE),
                    dir.join(orbiters_core::picture::FILE),
                    result.join(orbiters_core::reconstruction::REPORT_FILE),
                    result.join(orbiters_core::reconstruction::EDIT_FILE),
                ]
                .map(|file| stamp(&file));
                Some((dir, stamp(&result).map(|s| s.0), manifest, read))
            })
            .map(|(dir, result, manifest, read)| (stamp(&dir).map(|s| s.0), manifest, result, read, dir))
            .collect();
        captures.sort_by(|a, b| a.4.cmp(&b.4));
        captures.hash(&mut hasher);
        // The history's file is written every few seconds while VRChat runs: read only when it was, and counted by
        // what it says of the visits.
        let written = stamp(history).unwrap_or_default();
        let visits = match self.history {
            Some((at, visits)) if at == written => visits,
            _ => {
                let visits = visited::History::load(history).revision();
                self.history = Some((written, visits));
                visits
            }
        };
        visits.hash(&mut hasher);
        records.and_then(stamp).hash(&mut hasher);
        hasher.finish()
    }
}

pub fn local_entries(root: &Path, server: Server) -> Vec<Entry> {
    local_entries_with_folders(root, server, &orbiters_core::settings::Settings::load().picture_folders)
}
pub fn local_entries_with_folders(root: &Path, server: Server, folders: &[PathBuf]) -> Vec<Entry> {
    let worlds = crate::worlds::load_gallery(root, folders);
    let history = visited::History::load(&visited::history_file());
    let records = records_path(server).ok().map(|p| read_records(&p)).unwrap_or_default();
    entries_of(worlds, &history, &records)
}

/// [`local_entries_with_folders`] from what is handed over, not read from where the app keeps it: the captures under
/// `root`, these pictures, the visit history in the file `history` and the upload records in the file `records`. What
/// the page's tests make their lists from, so that none reads the member's own.
pub fn local_entries_at(root: &Path, index: visited::ShotIndex, history: &Path, records: Option<&Path>) -> Vec<Entry> {
    let history = visited::History::load(history);
    let worlds = crate::worlds::gallery_from(root, index, &history);
    entries_of(worlds, &history, &records.map(read_records).unwrap_or_default())
}

/// The list's entries: each world's captures and pictures, the instance a picture was taken in from the visit history
/// where its own record does not say, and what was uploaded of each from the upload records.
fn entries_of(worlds: Vec<crate::worlds::VisitedWorld>, history: &visited::History, records: &[Record]) -> Vec<Entry> {
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
                instance: s.manifest.world.as_ref().and_then(|w| w.instance.clone()).or_else(|| instance_at(history, &w.id, taken)),
                source_key: s.dir.file_name().map(|s| s.to_string_lossy().into_owned()),
                people: Vec::new(),
                local: Some(local),
                remote,
                content: Content::read(&s.dir),
                ready: s.reconstruction.is_some(),
                no_keyframes: s.has_no_keyframes(),
                picture_mode: s.picture.map(|p| p.mode),
                no_depth: s.manifest.stats.depth_frames == 0,
                trimmed: s.trimmed,
                result: s.reconstruction.as_ref().map(|r| r.created_utc.clone()),
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
                instance: shot.info.instance.clone().or_else(|| instance_at(history, &w.id, shot.info.taken)),
                source_key: shot.sha256.clone(),
                people: Vec::new(),
                local: Some(Local::Picture(shot.path.clone())),
                remote,
                content,
                ready: true,
                no_keyframes: false,
                picture_mode: None,
                no_depth: false,
                trimmed: false,
                result: None,
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
/// The entry of one session folder of this computer, as Photos & captures lists it ([`entries_of`]), for the bridge's
/// publishing of the 3D pictures it took (controller/bridge_publish.rs); `None` for a folder without a readable manifest.
pub fn session_entry(dir: &Path, server: Server) -> Option<Entry> {
    let manifest = orbiters_core::bundle::read_manifest(dir).ok()?;
    let world = manifest.world.clone().unwrap_or_default();
    let records = records_path(server).ok().map(|p| read_records(&p)).unwrap_or_default();
    let record = records.iter().find(|r| r.path == dir);
    let reconstructed = orbiters_core::reconstruction::read_report(dir).is_some();
    let taken = unix(&manifest.created_utc);
    Some(Entry {
        key: dir.to_string_lossy().into(),
        world_id: world.id.clone().unwrap_or_default(),
        world_name: world.name.clone().unwrap_or_default(),
        taken,
        instance: world.instance.clone(),
        source_key: dir.file_name().map(|s| s.to_string_lossy().into_owned()),
        people: Vec::new(),
        local: Some(Local::Session(dir.to_path_buf())),
        remote: record.and_then(|r| r.item.clone()),
        content: Content::read(dir),
        ready: reconstructed,
        no_keyframes: manifest.stats.frames > 0 && manifest.stats.keyframes == 0,
        picture_mode: orbiters_core::picture::Picture::read(dir).map(|p| p.mode),
        no_depth: manifest.stats.depth_frames == 0,
        trimmed: orbiters_core::cleanup::trimmed(dir),
        result: None,
        picture: Some(crate::worlds::Picture::Session(dir.to_path_buf())),
        size: session_picture_size(manifest.capture.width, manifest.capture.height),
        layers: Vec::new(),
        edits: reconstructed.then(|| orbiters_reconstruct::edit::generation(&orbiters_core::reconstruction::result_dir(dir)).map_err(|e| e.to_string())),
        uploaded_edits: record.map(|r| r.edits.clone().unwrap_or_else(|| orbiters_reconstruct::edit::UNEDITED.into())),
    })
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
        no_keyframes: false,
        picture_mode: None,
        no_depth: false,
        trimmed: false,
        result: None,
        picture,
        size: item.picture.as_ref().filter(|p| p.width > 0 && p.height > 0).map(|p| [p.width, p.height]),
        layers: Vec::new(),
        edits: None,
        uploaded_edits: None,
        remote: Some(item),
    }
}

/// What the card of an item on Orbiters shows when nothing on this computer does (its capture's folder deleted, or
/// never here), in the environment `development` names: a picture's own picture, a 3D capture's picture (its 3D view as
/// uploaded with it, `capture.pictureUrl`); `large` for a window. None when Orbiters has none.
pub fn remote_picture(item: &Item, development: bool, large: bool) -> Option<crate::worlds::Picture> {
    match item.kind {
        Kind::Picture => item.picture.is_some().then_some(crate::worlds::Picture::Capture { development, id: item.id, large }),
        Kind::Capture => {
            let url = item.capture.as_ref()?.picture_url.as_ref().filter(|u| shares::capture_picture_address(u))?;
            Some(crate::worlds::Picture::CaptureView { development, id: item.id, url: url.clone(), large })
        }
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
    let whose = Whose::saved(server)?;
    Ok((whose.api, whose.token))
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

/// What a capture's upload record says it was made with once a new reconstruction replaced the result it was uploaded
/// from. Never an edits' generation (`unedited`, or two hashes), so that [`Entry::edited_since_upload`] says the copy on
/// Orbiters is not what its 3D view shows, and Update the shared capture sends the new result; without this a capture
/// uploaded unedited and reconstructed unedited looks up to date for good, the link showing the old result.
pub const RECONSTRUCTED: &str = "reconstructed";

/// A reconstruction of `dir` finished: its uploads are of the result it replaced. In every account's records, signed in
/// now or not ([`records_path`]: one file for each account on each server): one signed out while it trained comes back
/// to a capture that says so.
pub fn reconstructed(dir: &Path) {
    let Ok(files) = std::fs::read_dir(orbiters_core::paths::config_dir()) else { return };
    for file in files.flatten() {
        let name = file.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("capture-uploads-") && name.ends_with(".json") {
            mark_reconstructed(&file.path(), dir);
        }
    }
}

fn mark_reconstructed(path: &Path, dir: &Path) {
    let _records = RECORDS.locked();
    let mut records = read_records(path);
    let mut changed = false;
    for r in records.iter_mut().filter(|r| r.path == dir && r.uploaded.kind == Kind::Capture) {
        r.edits = Some(RECONSTRUCTED.into());
        changed = true;
    }
    if changed {
        let written = serde_json::to_vec(&records).map_err(anyhow::Error::from).and_then(|b| orbiters_core::write_atomic(path, &b).map_err(anyhow::Error::from));
        if let Err(e) = written {
            share_log(&format!("3D capture {} reconstructed, its upload record not updated: {e:#}", dir.display())); // i18n: not shown
        }
    }
}

/// What a finished reconstruction of the session `dir` takes from what it has now, as sentences for the person (none:
/// nothing they could miss, it may start without asking). `remote`: what the page knows of its share, else its record.
pub fn reconstruct_replaces(dir: &Path, server: Server, remote: Option<&Item>) -> Vec<String> {
    let records = records_path(server).ok();
    replaces(&orbiters_core::reconstruction::result_dir(dir), records.as_deref(), dir, remote)
}

fn replaces(result: &Path, records: Option<&Path>, dir: &Path, remote: Option<&Item>) -> Vec<String> {
    let mut lines = Vec::new();
    // The worker deletes the edits with the result they name splats of.
    if let Ok(bytes) = std::fs::read(result.join(orbiters_core::reconstruction::EDIT_FILE)) {
        match orbiters_reconstruct::edit::Edit::decode(&bytes) {
            Ok(e) => lines.extend(edits_line(e.hidden() + e.deleted())),
            Err(_) => lines.push(tr("Its splat edits, which could not be read, go with the current one.").to_string()),
        }
    }
    let record = records.and_then(|p| read_records(p).into_iter().find(|r| r.path == dir));
    let item = remote.filter(|i| i.mine).or(record.as_ref().and_then(|r| r.item.as_ref()));
    let public = item.map_or(record.as_ref().is_some_and(|r| r.uploaded.visibility == "public"), |i| i.public);
    let friends = item.map_or(0, |i| i.grants.len());
    lines.extend(shared_line(public, friends));
    lines
}

/// What a new result takes of `removed` splats hidden or deleted.
pub(crate) fn edits_line(removed: usize) -> Option<String> {
    match removed {
        0 => None,
        1 => Some(tr("The splat hidden or deleted in its 3D view comes back: you can select it again in the new one.").to_string()),
        n => Some(tr!("The {count} splats hidden or deleted in its 3D view come back: you can select them again in the new one.", count = orbiters_ui::format::integer(n as f64))),
    }
}

/// What a new result does to a capture shared publicly and with `friends`: nothing at once, and that is what to know.
pub(crate) fn shared_line(public: bool, friends: usize) -> Option<String> {
    // Whole sentences, each with its ending: the order of the parts is each language's own.
    match (public, friends) {
        (false, 0) => None,
        (true, 0) => Some(tr("Its public link stays, and goes on showing the current version until you update the shared capture, which its window offers once the new one is ready.").to_string()),
        (false, 1) => Some(tr("The friend you shared it with keeps seeing the current version until you update the shared capture, which its window offers once the new one is ready.").to_string()),
        (false, n) => Some(tr!("The {friends} friends you shared it with keep seeing the current version until you update the shared capture, which its window offers once the new one is ready.", friends = n)),
        (true, 1) => Some(tr("Its public link and the friend you shared it with keep the current version until you update the shared capture, which its window offers once the new one is ready.").to_string()),
        (true, n) => Some(tr!("Its public link and the {friends} friends you shared it with keep the current version until you update the shared capture, which its window offers once the new one is ready.", friends = n)),
    }
}

/// What a capture's window follows of its session.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionState {
    pub ready: bool,
    pub no_keyframes: bool,
    pub picture_mode: Option<orbiters_core::settings::PictureMode>,
    pub no_depth: bool,
    pub trimmed: bool,
    pub result: Option<String>,
    pub edits: Option<Result<String, String>>,
    pub uploaded_edits: Option<String>,
}

/// What the session `dir` says now, read from its own files alone (a capture's window asks when the library changed,
/// without listing every picture again); none when it is gone.
pub fn session_state(dir: &Path, server: Server) -> Option<SessionState> {
    let manifest = orbiters_core::bundle::read_manifest(dir).ok()?;
    let report = orbiters_core::reconstruction::read_report(dir);
    let records = records_path(server).ok().map(|p| read_records(&p)).unwrap_or_default();
    let record = records.iter().find(|r| r.path == dir);
    Some(SessionState {
        ready: report.is_some(),
        no_keyframes: manifest.stats.frames > 0 && manifest.stats.keyframes == 0,
        picture_mode: orbiters_core::picture::Picture::read(dir).map(|p| p.mode),
        no_depth: manifest.stats.depth_frames == 0,
        trimmed: orbiters_core::cleanup::trimmed(dir),
        edits: report.as_ref().map(|_| orbiters_reconstruct::edit::generation(&orbiters_core::reconstruction::result_dir(dir)).map_err(|e| e.to_string())),
        result: report.map(|r| r.created_utc),
        uploaded_edits: record.map(|r| r.edits.clone().unwrap_or_else(|| orbiters_reconstruct::edit::UNEDITED.into())),
    })
}

/// Why a capture is not shared when its splat edits changed while it was being shared.
const EDITED_MEANWHILE: &str = mark("Its splats were changed in its 3D view while it was being shared, so it is not shared: share it again to send what its 3D view shows now.");
/// What the tray says once an update is done.
pub(crate) const UPDATED: &str = mark("Updated: everyone who sees this 3D capture now sees it as its 3D view shows it.");
/// How a failed update ends: Orbiters switches to new files whole or not at all.
pub(crate) const KEPT: &str = mark("Everyone still sees the version shared before.");

/// The generation of the splat edits of a capture's session now (none for a picture); edits that cannot be read stop
/// whatever asked.
fn edits_now(entry: &Entry) -> anyhow::Result<Option<String>> {
    match entry.local.as_ref() {
        Some(Local::Session(dir)) => Ok(Some(orbiters_reconstruct::edit::generation(&orbiters_core::reconstruction::result_dir(dir))?)),
        _ => Ok(None),
    }
}
fn records_path(server: Server) -> anyhow::Result<PathBuf> {
    // A window lists what the member it shows uploaded; the service, the account saved.
    if crate::member::linked() {
        return records_of(server, crate::member::reading(server).map(|m| m.user).as_deref());
    }
    let account = account::read(server).ok_or_else(|| anyhow::anyhow!("{}", tr("Sign in first")))?;
    records_of(server, account.user.as_deref())
}
fn records_of(server: Server, user: Option<&str>) -> anyhow::Result<PathBuf> {
    let user = user.ok_or_else(|| anyhow::anyhow!("{}", tr("Orbiters did not identify this account")))?;
    anyhow::ensure!(user.chars().all(|c| c.is_ascii_digit()), "{}", tr("Invalid account id"));
    Ok(orbiters_core::paths::config_dir().join(format!("capture-uploads-{}-{user}.json", if server == Server::Development { "dev" } else { "prod" })))
}
/// What this account uploaded from this computer: each session's or picture's upload as the server last answered it.
pub fn uploads(server: Server) -> Vec<(PathBuf, Item)> {
    uploads_in(records_path(server))
}
fn uploads_in(records: anyhow::Result<PathBuf>) -> Vec<(PathBuf, Item)> {
    records.map(|p| read_records(&p)).unwrap_or_default().into_iter().filter_map(|r| r.item.map(|item| (r.path, item))).collect()
}
/// Writes, as the upload records of an account, that the capture or picture at `path` was uploaded as `item` (tests
/// of what is made from those records).
#[cfg(test)]
pub(crate) fn write_record_for_test(file: &Path, path: &Path, item: Item) {
    let uploaded = Uploaded { kind: item.kind, id: item.id, visibility: "private".into(), explorer: None };
    let records = vec![Record { path: path.to_path_buf(), item: Some(item), uploaded, edits: None }];
    std::fs::write(file, serde_json::to_vec(&records).unwrap()).unwrap();
}

fn read_records(path: &Path) -> Vec<Record> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The library moved (another sessions folder, another drive): a capture's upload record names the folder where it was.
/// A record whose folder is gone is pointed at the folder of the same name in `sessions_dir`, when there is one, so that
/// every path reading the records (N4N0's menu share, My World, the bridge's publish) still knows it uploaded and does
/// not send it again (CTL2-11 k). Pictures, and folders of that name that are not there, are left as they are.
/// How many records moved.
pub(crate) fn follow_moved_sessions(records_file: &Path, sessions_dir: &Path) -> usize {
    let _records = RECORDS.locked();
    let mut records = read_records(records_file);
    let mut moved = 0;
    for record in records.iter_mut().filter(|r| r.uploaded.kind == Kind::Capture) {
        if record.path.starts_with(sessions_dir) || record.path.is_dir() {
            continue;
        }
        let Some(name) = record.path.file_name() else { continue };
        let here = sessions_dir.join(name);
        if here.join("manifest.json").is_file() {
            record.path = here;
            moved += 1;
        }
    }
    if moved > 0 {
        if let Ok(bytes) = serde_json::to_vec(&records) {
            if let Err(e) = orbiters_core::write_atomic(records_file, &bytes) {
                eprintln!("upload records: the moved library could not be noted: {e}");
                return 0;
            }
        }
    }
    moved
}

/// [`follow_moved_sessions`] for the account signed in now, on `server`.
pub(crate) fn follow_moved_library(server: Server, sessions_dir: &Path) {
    if let Ok(file) = records_path(server) {
        let moved = follow_moved_sessions(&file, sessions_dir);
        if moved > 0 {
            share_log(&format!("{moved} upload records follow the library to {}", sessions_dir.display())); // i18n: not shown
        }
    }
}
fn save_record(records: anyhow::Result<&Path>, entry: &Entry, item: &Item) -> anyhow::Result<()> {
    save_record_edits(records, entry, item, None)
}
/// [`save_record`] with the splat edits its files on Orbiters were made with (`None`: those its record had).
fn save_record_edits(records: anyhow::Result<&Path>, entry: &Entry, item: &Item, edits: Option<String>) -> anyhow::Result<()> {
    if let Some(local) = &entry.local {
        let _records = RECORDS.locked();
        let path = records?;
        let mut records = read_records(path);
        // The splat edits it was uploaded with stay known (the same item: the same files) until it is updated.
        let edits = edits.or_else(|| {
            records.iter().find(|r| r.path == local.path() && r.uploaded.id == item.id && r.uploaded.kind == item.kind).and_then(|r| r.edits.clone())
        });
        records.retain(|r| r.path != local.path());
        records.push(Record {
            path: local.path().into(),
            item: Some(item.clone()),
            uploaded: Uploaded { kind: item.kind, id: item.id, visibility: if item.public { "public" } else { "private" }.into(), explorer: None },
            edits,
        });
        orbiters_core::write_atomic(path, &serde_json::to_vec(&records)?)?;
    }
    Ok(())
}
/// Before a capture's folder is deleted (the library's Delete, the bridge's `free_picture_files everything`): its uploads
/// on Orbiters (this computer's records, each environment signed in) have their picture there, given now from the folder
/// when one had none, so that its card in every gallery still shows it once the folder is gone. What could not be done,
/// in words for a log; nothing asked of Orbiters when the records say the picture is there.
pub fn keep_pictures_on_orbiters(dir: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    for server in [Server::Production, Server::Development] {
        // The account and its records from one read of who is signed in there.
        let Ok(whose) = Whose::saved(server) else { continue };
        let Ok(file) = whose.records() else { continue };
        let result = keep_picture_with(&whose.api, &whose.token, &file, dir);
        match result {
            Ok(Some(id)) => share_log(&format!("3D capture {} gave capture {id} its picture before its folder was deleted", dir.display())), // i18n: not shown
            Ok(None) => {}
            Err(e) => {
                let problem = format!("{}: {e:#}", if server == Server::Development { "development" } else { "production" }); // i18n: not shown (a log line)
                share_log(&format!("3D capture {} has no picture on Orbiters, none given before its folder was deleted ({problem})", dir.display())); // i18n: not shown
                problems.push(problem);
            }
        }
    }
    problems
}
/// [`keep_pictures_on_orbiters`] for one account's `records`: the capture's id when its picture was given now.
pub(crate) fn keep_picture_with(api: &Api, token: &str, records: &Path, dir: &Path) -> anyhow::Result<Option<u64>> {
    let Some(record) = read_records(records).into_iter().find(|r| r.path == dir && r.uploaded.kind == Kind::Capture) else { return Ok(None) };
    if record.item.as_ref().and_then(|i| i.capture.as_ref()).is_some_and(|c| c.picture_url.is_some()) {
        return Ok(None);
    }
    let mut item = api.capture_item(token, Kind::Capture, record.uploaded.id)?;
    let given = item.capture.as_ref().is_some_and(|c| c.picture_url.is_none());
    if given {
        let jpeg = crate::capture_picture::jpeg(dir).ok_or_else(|| anyhow::anyhow!("no picture can be made from its folder"))?; // i18n: not shown (a log line)
        item = api.set_capture_picture(token, item.id, jpeg)?;
    }
    let mut entry = remote_entry(item.clone());
    entry.local = Some(Local::Session(dir.to_path_buf()));
    save_record_edits(Ok(records), &entry, &item, None)?;
    Ok(given.then_some(item.id))
}
/// Its upload is not on Orbiters any more (removed there): its record goes, so that a share uploads it again.
fn forget_record(path: &Path, dir: &Path, id: u64) {
    forget_record_of(path, dir, Kind::Capture, id);
}

/// [`forget_record`] for an upload of either kind (a capture or a picture).
fn forget_record_of(path: &Path, dir: &Path, kind: Kind, id: u64) {
    let _records = RECORDS.locked();
    let mut records = read_records(path);
    let before = records.len();
    records.retain(|r| !(r.path == dir && r.uploaded.kind == kind && r.uploaded.id == id));
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
        let mut set = set.locked();
        if set.contains(&key) {
            report(Stage::Waiting);
        }
        while set.contains(&key) {
            set = wake.wait_locked(set);
        }
        set.insert(key.clone());
        Self(key)
    }
}
impl Drop for Changing {
    fn drop(&mut self) {
        let (set, wake) = &CHANGING;
        set.locked().remove(&self.0);
        wake.notify_all();
    }
}
/// Upload identity is saved before granting: retry after a failed share never uploads a duplicate. An item already
/// uploaded (as the gallery or its record says) is shared with one request, never sent or fetched again, unless it is a
/// capture whose splats were edited since: its files on Orbiters are replaced first ([`update_with`]), so what is shared
/// is what its 3D view shows. How long each step took goes to `%LOCALAPPDATA%/Orbiters/logs/shares.log`.
pub(super) fn share_as(whose: &Whose, entry: Entry, request: shares::Share, report: Report) -> anyhow::Result<shares::Shared> {
    let started = Instant::now();
    let mut steps = Vec::new();
    let result = share_with(&whose.api, &whose.token, whose.records(), &entry, &request, report, &mut steps, &super::maps::web_preview);
    let whom = match (request.public, request.recipients.len()) {
        (true, 0) => "publicly".to_string(),
        (public, n) => format!("{}with {n} friend{}", if public { "publicly and " } else { "" }, if n == 1 { "" } else { "s" }), // i18n: not shown
    };
    let what = if entry.is_picture() { "picture" } else { "3D capture" }; // i18n: not shown
    let end = match &result {
        Ok(_) => String::new(),
        Err(e) => format!(", failed: {e}"), // i18n: not shown
    };
    share_log(&format!("{what} {} shared {whom}: {}; {:.2} s in all{end}", entry.key, steps.join(", "), started.elapsed().as_secs_f64())); // i18n: not shown
    result
}
/// What makes a capture's upload files: [`super::maps::web_preview`] (a stand-in in tests).
pub(crate) type Prepare<'a> = &'a (dyn Fn(&Path) -> anyhow::Result<super::maps::Prepared> + Sync);
/// Notes how long a step took in `steps` (shares.log).
fn step(steps: &mut Vec<String>, what: &str, at: &mut Instant) {
    steps.push(format!("{what} {:.2} s", at.elapsed().as_secs_f64()));
    *at = Instant::now();
}
/// Orbiters' own "not found" for an item (its JSON error, not a proxy's page): the item was removed there. The captures
/// API gives an answer without a JSON error its general sentence (orbiters_core::shares `json`): that is no proof.
fn removed_on_orbiters(e: &anyhow::Error) -> bool {
    let general = tr("Orbiters could not complete the request");
    e.downcast_ref::<orbiters_core::account::ApiError>().is_some_and(|a| a.status == 404 && !a.message.is_empty() && a.message != general)
}

/// [`share_as`] through `api` as `token`, this account's upload records in `records`, a capture's files made by `prepare`;
/// what took how long in `steps`. One try ([`share_with_once`]); an item this computer's record says it uploaded, and that Orbiters answers it no longer has
/// (removed on the website, another computer, moderation), loses that record and is uploaded again, once, as the person
/// asked to share it. It stayed unshareable for good before: every share asked the dead id again (CTL2-4).
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
    let records = records.map_err(|e| e.to_string());
    let again = || records.clone().map_err(anyhow::Error::msg);
    let recorded = entry.local.as_ref().and_then(|l| records.as_ref().ok().and_then(|p| read_records(p).into_iter().find(|r| r.path == l.path())));
    let result = share_with_once(api, token, again(), entry, request, report, steps, prepare);
    let (Err(e), Some(record), Ok(file)) = (&result, recorded, records.as_ref()) else { return result };
    // Only the id this computer's record named, and only when the entry had no other (a listed item is Orbiters' word).
    let named = entry.remote.as_ref().is_none_or(|i| (i.kind, i.id) == (record.uploaded.kind, record.uploaded.id));
    if !removed_on_orbiters(e) || !named {
        return result;
    }
    let mut at = Instant::now();
    step(steps, &format!("{:?} {} is not on Orbiters any more: its record goes and it is uploaded again", record.uploaded.kind, record.uploaded.id), &mut at); // i18n: not shown
    forget_record_of(file, &record.path, record.uploaded.kind, record.uploaded.id);
    let mut fresh = entry.clone();
    fresh.remote = None;
    share_with_once(api, token, again(), &fresh, request, report, steps, prepare)
}

/// One try of [`share_with`].
#[allow(clippy::too_many_arguments)]
fn share_with_once(
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
        step(steps, "waited for its other change", &mut at); // i18n: not shown
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
    // A public upload's points (the share after it earns nothing more for it): what this share earned is both answers'.
    let mut upload_explorer: Option<orbiters_core::explorer::Outcome> = None;
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
        // The record of the upload must be writable before anything is sent: an upload whose record cannot be kept is
        // sent again by every retry (CTL2-11 f).
        records.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
        anyhow::ensure!(!request.public || !entry.content.explicit || request.acknowledge_explicit, "{}", tr(shares::EXPLICIT_WARNING));
        let source_key = match entry.local.as_ref().ok_or_else(|| anyhow::anyhow!("{}", tr("No local file to upload")))? {
            Local::Session(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| anyhow::anyhow!("{}", tr("No capture folder name")))?,
            Local::Picture(p) => entry
                .source_key
                .clone()
                .or_else(|| crate::worlds::hashes(vec![p.clone()]).remove(p))
                .ok_or_else(|| anyhow::anyhow!("{}", tr("Could not fingerprint this picture")))?,
        };
        anyhow::ensure!(source_key.chars().count() <= 128, "{}", tr("The capture folder name is longer than the server's 128-character source key limit."));
        let mut metadata = serde_json::json!({"visibility":if request.public{"public"}else{"private"},"explicit":entry.content.explicit,"explicitReasons":entry.content.explicit_reasons,"acknowledgeExplicit":request.acknowledge_explicit,"takenAt":iso(entry.taken)});
        metadata["sourceKey"] = serde_json::json!(source_key);
        metadata["instanceId"] = serde_json::json!(entry.instance);

        let uploaded: Uploaded = match entry.local.as_ref().ok_or_else(|| anyhow::anyhow!("{}", tr("No local file to upload")))? {
            Local::Session(dir) => {
                report(Stage::Preparing);
                let prepared = prepare(dir)?;
                // The picture only when drawn for these edits (capture_picture), and the edits still those prepared.
                let picture = crate::capture_picture::jpeg(dir);
                anyhow::ensure!(edits_now(entry)?.as_deref() == Some(prepared.generation.as_str()), "{}", tr("Its splat edits changed while it was being prepared: share it again."));
                // Every part of the one result they were made from (CHALLENGER-C1).
                prepared.ensure_current(dir)?;
                let super::maps::Prepared { files, sky, metadata: view, generation, .. } = prepared;
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
                    sending.join().unwrap_or_else(|_| Err(anyhow::anyhow!("{}", tr("The upload stopped unexpectedly"))))
                })
                .inspect(|_| step(steps, &format!("sent {:.1} MB and stored", bytes as f64 / 1e6), &mut at))? // i18n: not shown
            }
            Local::Picture(path) => {
                // PRIV-8: what goes is a copy without the picture's metadata (who took it, who else was there, the
                // instance), the pixels as they are; the file here keeps it all, and the original's hash stays its
                // identity (`sourceKey`), so a picture shared again is still recognised.
                let clean = orbiters_core::png_meta::CleanCopy::of(path)?;
                let length = clean.len;
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let part = Part::file("pictures", name.as_ref(), "image/png", &clean.path)?;
                metadata["takenAt"] = serde_json::json!([iso(entry.taken)]);
                report(Stage::Sending { sent: 0, total: length });
                let result = api.add_shared_pictures(token, &entry.world_id, vec![part], &metadata)?;
                step(steps, "sent and stored", &mut at); // i18n: not shown
                match result.added.into_iter().next() {
                    Some(uploaded) => uploaded,
                    None => {
                        let item = api.capture_by_source_key(token, &entry.world_id, Kind::Picture, &source_key)?.ok_or_else(|| {
                            anyhow::anyhow!("{}", tr("The server skipped this picture, but its source identity is not available yet. Refresh the gallery and retry."))
                        })?;
                        Uploaded { kind: item.kind, id: item.id, visibility: if item.public { "public" } else { "private" }.into(), explorer: None }
                    }
                }
            }
        };
        {
            let _records = RECORDS.locked();
            let file = records.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut known = read_records(file);
            let local = entry.local.as_ref().unwrap();
            known.retain(|r| r.path != local.path());
            known.push(Record { path: local.path().into(), item: None, uploaded: uploaded.clone(), edits: sent_edits.clone() });
            orbiters_core::write_atomic(file, &serde_json::to_vec(&known)?)?;
        }
        upload_explorer = uploaded.explorer.clone();
        (uploaded.kind, uploaded.id)
    };
    // Edited while it was uploading: not shared, and an upload that went public at once taken back from the public.
    if let (Some(sent), Ok(now)) = (&sent_edits, edits_now(entry)) {
        if now.as_deref() != Some(sent.as_str()) {
            if request.public {
                let withdraw = shares::Unshare { public: true, recipients: Vec::new(), all: false };
                if let Err(e) = api.unshare_capture(token, kind, id, &withdraw) {
                    step(steps, &format!("could not take it back from the public ({e})"), &mut at); // i18n: not shown
                }
            }
            anyhow::bail!("{}", tr(EDITED_MEANWHILE));
        }
    }
    // Edits that cannot be read now stop the share too; edited while it was updated, it is not shared either.
    if let Some(now) = edits_now(entry)? {
        let made_with = sent_edits.as_deref().unwrap_or(&uploaded_edits);
        anyhow::ensure!(now == made_with, "{}", tr(EDITED_MEANWHILE));
    }
    // Kept on the account and listed nowhere: the upload already is that (sent with visibility private). Asking Orbiters to
    // share it with nobody is refused ("Choose someone to share with.", the world tour's first publish, 2026-11-15), so a
    // private share with no recipient asks nothing more than the item, to remember it.
    if !request.public && request.recipients.is_empty() {
        let mut item = api.capture_item(token, kind, id)?;
        step(steps, "kept private", &mut at); // i18n: not shown
        give_picture(api, token, entry, &mut item, steps, &mut at);
        save_record(records, entry, &item)?;
        return Ok(shares::Shared::Item { item, explorer: None });
    }
    report(Stage::Sharing);
    let shared = api.share_capture(token, kind, id, request);
    step(steps, "shared", &mut at);
    match shared {
        Ok(mut result) => {
            if let shares::Shared::Item { item, explorer } = &mut result {
                give_picture(api, token, entry, item, steps, &mut at);
                save_record(records, entry, item)?;
                *explorer = match (upload_explorer, explorer.take()) {
                    (Some(up), Some(shared)) => Some(up.then(shared)),
                    (up, shared) => shared.or(up),
                };
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

/// A capture of this account uploaded without its picture (one uploaded before captures had them, or while its picture
/// could not leave the app) gets it now, public or private alike: every gallery's card of it shows that picture once its
/// folder here is gone, and its public page's link previews show it. A failure is noted and changes nothing else.
fn give_picture(api: &Api, token: &str, entry: &Entry, item: &mut Item, steps: &mut Vec<String>, at: &mut Instant) {
    let Some(Local::Session(dir)) = entry.local.as_ref() else { return };
    if item.kind != Kind::Capture || !item.mine || item.capture.as_ref().is_none_or(|c| c.picture_url.is_some()) {
        return;
    }
    match crate::capture_picture::jpeg(dir).map(|jpeg| api.set_capture_picture(token, item.id, jpeg)) {
        Some(Ok(with_picture)) => {
            *item = with_picture;
            step(steps, "gave it its picture", at); // i18n: not shown
        }
        Some(Err(e)) => step(steps, &format!("its picture failed ({e})"), at), // i18n: not shown
        None => {}
    }
}

/// [`Operation::Update`]: the capture's files on Orbiters replaced by what its 3D view shows now, one change of the item
/// at a time ([`Changing`]); its item at the new revision and the splat edits they were made with. How long each step
/// took goes to shares.log. A failure says that the version shared before stays: Orbiters switches to new files whole or
/// not at all (contracts Docs/capture-sharing.md, Updating a capture).
fn update(whose: &Whose, entry: Entry, report: Report) -> anyhow::Result<(Item, String)> {
    let started = Instant::now();
    let mut steps = Vec::new();
    let result = {
        let _one = Changing::of(&entry, report);
        let records = whose.records();
        update_with(&whose.api, &whose.token, records.as_deref().map_err(|e| anyhow::anyhow!("{e}")), &entry, report, &mut steps, &super::maps::web_preview)
    };
    let end = match &result {
        Ok((item, _)) => format!(", revision {}", item.capture.as_ref().map_or(1, shares::Capture::revision)), // i18n: not shown
        Err(e) => format!(", failed: {e}"), // i18n: not shown
    };
    share_log(&format!("3D capture {} updated: {}; {:.2} s in all{end}", entry.key, steps.join(", "), started.elapsed().as_secs_f64())); // i18n: not shown
    result.map_err(|e| anyhow::anyhow!("{}", not_updated(&e)))
}

/// Why an update did not happen, and that the version shared before stays.
fn not_updated(e: &anyhow::Error) -> String {
    let why = e.to_string();
    // Where a sentence ends in the language it is in: the full-width marks are Japanese's.
    let end = if why.ends_with(['.', '!', '?', '。', '！', '？']) { "" } else { "." };
    format!("{} {}", tr!("Not updated: {reason}", reason = format!("{why}{end}")), tr(KEPT))
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
    let Some(Local::Session(dir)) = entry.local.as_ref() else { anyhow::bail!("{}", tr("Only a 3D capture of this computer can be updated.")) };
    let record = records.as_ref().ok().and_then(|p| read_records(p).into_iter().find(|r| r.path == *dir));
    let id = entry
        .remote
        .as_ref()
        .filter(|i| i.kind == Kind::Capture)
        .map(|i| i.id)
        .or_else(|| record.as_ref().filter(|r| r.uploaded.kind == Kind::Capture).map(|r| r.uploaded.id))
        .ok_or_else(|| anyhow::anyhow!("{}", tr("It is not uploaded yet: share it to upload it.")))?;
    // Damaged edits stop it before anything is asked of Orbiters.
    edits_now(entry)?;
    let current = match api.capture_item(token, Kind::Capture, id) {
        Ok(item) => item,
        Err(e) if e.downcast_ref::<orbiters_core::account::ApiError>().is_some_and(|a| a.status == 404) => {
            if let Ok(path) = records.as_ref() {
                forget_record(path, dir, id);
            }
            anyhow::bail!("{}", tr("It is not on Orbiters any more: share it again to upload it as its 3D view shows it now."));
        }
        Err(e) => return Err(e),
    };
    anyhow::ensure!(current.mine, "{}", tr("Only the member who uploaded it can update it."));
    let revision = current.capture.as_ref().map_or(1, shares::Capture::revision);
    step(steps, "asked Orbiters", &mut at); // i18n: not shown
    report(Stage::Preparing);
    let prepared = prepare(dir)?;
    // The picture only when drawn for these edits (capture_picture), and the edits still those prepared.
    let picture = crate::capture_picture::jpeg(dir);
    anyhow::ensure!(
        edits_now(entry)?.as_deref() == Some(prepared.generation.as_str()),
        "{}",
        tr("Its splat edits changed while it was being prepared: update it again once they are done.")
    );
    // Every part of the one result they were made from (CHALLENGER-C1).
    prepared.ensure_current(dir)?;
    let super::maps::Prepared { files, sky, metadata, generation, .. } = prepared;
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
        sending.join().unwrap_or_else(|_| Err(anyhow::anyhow!("{}", tr("The update stopped unexpectedly"))))
    });
    let item = match sent {
        Ok(item) => item,
        Err(e) => {
            if let Some(conflict) = e.downcast_ref::<shares::RevisionConflict>() {
                anyhow::bail!(
                    "{}",
                    tr!(
                        "it was updated from somewhere else meanwhile (it is at revision {current}; this computer replaced revision {revision}). Look at it, then update it again to send what this computer's 3D view shows.",
                        current = conflict.revision,
                        revision = revision
                    )
                );
            }
            // No answer after the files went (the connection dropped), or a server error, which may come after the switch
            // (Cloudflare's 524 when Orbiters took over 100 s, its 502 when the backend restarted, a 500 once committed):
            // Orbiters switched to them, or not, as its revision says (another computer's update of the same capture in
            // that very moment would read the same). Only a refusal (4xx) is final without asking.
            let refused = e.downcast_ref::<orbiters_core::account::ApiError>().is_some_and(|a| a.status < 500);
            let switched = if refused {
                None
            } else {
                api.capture_item(token, Kind::Capture, id).ok().filter(|now| now.capture.as_ref().map_or(1, shares::Capture::revision) == revision + 1)
            };
            match switched {
                Some(now) => {
                    step(steps, &format!("lost the answer ({e}), found it switched"), &mut at); // i18n: not shown
                    now
                }
                None => return Err(e),
            }
        }
    };
    step(steps, &format!("sent {:.1} MB and switched", bytes as f64 / 1e6), &mut at); // i18n: not shown
    // Switched: everyone sees the update whatever happens here. A record that cannot be saved is only this computer's
    // note of it (the capture then asks to be updated again), never "Not updated".
    if let Err(e) = save_record_edits(records, entry, &item, Some(generation.clone())) {
        step(steps, &format!("its upload record could not be saved ({e})"), &mut at); // i18n: not shown
    }
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
/// Tests: what makes a capture's upload files, shared with a job's thread.
#[cfg(test)]
pub(crate) type SharedPrepare = std::sync::Arc<dyn Fn(&Path) -> anyhow::Result<super::maps::Prepared> + Send + Sync>;
/// Tests: [`Operation::Update`] through `api` (a stand-in), this account's records in `records`, files by `prepare`.
#[cfg(test)]
pub(crate) fn update_against(api: Api, records: PathBuf, prepare: SharedPrepare) -> TestWork {
    std::sync::Arc::new(move |request: Request, report: Report| {
        let _one = Changing::of(&request.entry, report);
        update_with(&api, "token", Ok(records.as_path()), &request.entry, report, &mut Vec::new(), &*prepare)
            .map(|(item, edits)| Outcome::Updated { item, edits })
            .map_err(|e| anyhow::anyhow!("{}", not_updated(&e)))
    })
}
/// A line in `%LOCALAPPDATA%/Orbiters/logs/shares.log`, kept under 1 MB (the last one renamed .old). Tests write none.
/// A line in shares.log (what the repair of captures' pictures sent: controller/picture_repair.rs).
pub(crate) fn note(line: &str) {
    share_log(line);
}
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
fn unshare(whose: &Whose, entry: Entry, request: shares::Unshare) -> anyhow::Result<Item> {
    let _one = Changing::of(&entry, &|_| {});
    let item = entry.remote.as_ref().ok_or_else(|| anyhow::anyhow!("{}", tr("This item has not been uploaded")))?;
    let item = whose.api.unshare_capture(&whose.token, item.kind, item.id, &request)?;
    save_record(whose.records().as_deref().map_err(|e| anyhow::anyhow!("{e}")), &entry, &item)?;
    Ok(item)
}
fn download(whose: &Whose, item: Item) -> anyhow::Result<PathBuf> {
    let dir = orbiters_core::paths::documents_root().join("Shared captures");
    std::fs::create_dir_all(&dir)?;
    let out =
        dir.join(format!("{}-{}-{}.{}", item.kind.path(), item.id, uuid::Uuid::new_v4().simple(), if item.kind == Kind::Picture { "webp" } else { "splat" }));
    let temp = out.with_extension("download");
    let result = whose.api.capture_file(&whose.token, item.kind, item.id, &temp);
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
        let message = crate::ipc::ToService::CaptureShare { request: Box::new(request.clone()), epoch: 1 };
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
        serde_json::to_string(&shares::Shared::Item { explorer: None, item: Item { kind, id, mine: true, can_share: true, grants, ..Default::default() } }).unwrap()
    }

    fn to_a_friend() -> shares::Share {
        shares::Share { recipients: vec![shares::Recipient { vrchat_user_id: "usr_friend".into(), display_name: "Friend".into() }], ..Default::default() }
    }

    /// The requests a stand-in got, each with its body.
    pub(crate) type Seen = std::sync::Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    /// A stand-in for Orbiters on a loopback port that answers each request with the first answer whose start matches its
    /// request line ("PUT /captures/capture/3/files"): an answer is used once while another one matches after it, the
    /// last one that matches stays; status 0 reads the request and closes the connection without an answer. The
    /// requests it got with their bodies, in order.
    pub(crate) fn stand_in(answers: Vec<(&'static str, u16, String)>) -> (Api, Seen) {
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
                // As Orbiters' limiter: a refusal for coming too often says when to come back.
                let wait = if status == 429 { "Retry-After: 1200\r\n" } else { "" };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\n{wait}Content-Length: {}\r\nConnection: close\r\n\r\n{answer}",
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
        let uploaded = Uploaded { kind: Kind::Capture, id: 3, visibility: "public".into(), explorer: None };
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
            identity: crate::upload_prep::ResultIdentity::read(session),
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
        // A reason that ends with Japanese's full-width stop gets no second one.
        let said = orbiters_i18n::in_language(orbiters_i18n::Language::Ja, || not_updated(&anyhow::anyhow!("サーバーが応答しませんでした。")));
        assert!(said.starts_with("更新されませんでした：サーバーが応答しませんでした。") && !said.contains("。."), "{said}");
        update_with(&api, "token", Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &prepared).unwrap();
        assert_eq!(
            requests(&seen),
            [
                "GET /captures/capture/3",
                "PUT /captures/capture/3/files",
                // A server error may follow the switch: its revision (still 1) says it did not.
                "GET /captures/capture/3",
                "GET /captures/capture/3",
                "PUT /captures/capture/3/files"
            ]
        );
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
        // Cloudflare giving up on a slow answer (524) after Orbiters switched: the update counts, it is not "Not updated".
        let (api, _) = stand_in(vec![
            ("GET /captures/capture/3", 200, capture_at(3, 1)),
            ("PUT /captures/capture/3/files", 524, r#"{"error":"A timeout occurred"}"#.into()),
            ("GET /captures/capture/3", 200, capture_at(3, 2)),
        ]);
        let (item, _) = update_with(&api, "token", Ok(records2.as_path()), &entry2, &|_| {}, &mut Vec::new(), &prepared).unwrap();
        assert_eq!(item.capture.map(|c| c.revision), Some(2));
        assert!(record_of(&records2).unwrap().edits.is_some());
        // A refusal (4xx) is final: nothing asked after it.
        let (api, seen) = stand_in(vec![
            ("GET /captures/capture/3", 200, capture_at(3, 2)),
            ("PUT /captures/capture/3/files", 400, r#"{"error":"The VRChat pc file of this 3D preview is not a valid splat tree."}"#.into()),
        ]);
        entry2.edits = Some(Ok(delete_splat(&session2, 3)));
        assert!(update_with(&api, "token", Ok(records2.as_path()), &entry2, &|_| {}, &mut Vec::new(), &prepared).is_err());
        assert_eq!(requests(&seen), ["GET /captures/capture/3", "PUT /captures/capture/3/files"]);
        std::fs::remove_dir_all(dir).unwrap();
        std::fs::remove_dir_all(dir2).unwrap();
    }

    /// Switched, but this computer's record of it cannot be written: the update is still done (everyone sees it), and the
    /// capture asks to be updated again rather than say that nothing changed.
    #[test]
    fn an_update_whose_record_cannot_be_saved_is_still_done() {
        let (dir, session, _, mut entry) = uploaded_session("record", None);
        entry.edits = Some(Ok(delete_splat(&session, 2)));
        // Known from the gallery (its item), since the record cannot be read either.
        entry.remote = Some(Item { kind: Kind::Capture, id: 3, mine: true, ..Default::default() });
        let (api, _) = stand_in(vec![("GET /captures/capture/3", 200, capture_at(3, 1)), ("PUT /captures/capture/3/files", 200, capture_at(3, 2))]);
        let records = Err(anyhow::anyhow!("the records folder is not writable (fixture)"));
        let mut steps = Vec::new();
        let (item, _) = update_with(&api, "token", records, &entry, &|_| {}, &mut steps, &prepared).unwrap();
        assert_eq!(item.capture.map(|c| c.revision), Some(2));
        assert!(steps.iter().any(|s| s.contains("record could not be saved")), "{steps:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// CHALLENGER-C1: reconstructed again (unedited before and after) while its files were made: not sent, since the
    /// trees would be the old result's and the picture the new one's.
    #[test]
    fn a_result_replaced_while_prepared_is_not_sent() {
        let (dir, _session, records, mut entry) = uploaded_session("replaced", None);
        entry.edits = Some(Ok(orbiters_reconstruct::edit::UNEDITED.into()));
        let (api, seen) = stand_in(vec![("GET /captures/capture/3", 200, capture_at(3, 1))]);
        let replaced = |s: &Path| {
            let made = prepared(s);
            let result = orbiters_core::reconstruction::result_dir(s);
            std::fs::write(result.join(orbiters_core::reconstruction::REPORT_FILE), br#"{"created_utc":"a newer run"}"#).unwrap();
            made
        };
        let refused = update_with(&api, "token", Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &replaced).unwrap_err().to_string();
        assert!(refused.contains("reconstructed again while it was being prepared"), "{refused}");
        assert_eq!(requests(&seen), ["GET /captures/capture/3"], "nothing sent");
        // The same result throughout: sent as before.
        let (api, _) = stand_in(vec![("GET /captures/capture/3", 200, capture_at(3, 1)), ("PUT /captures/capture/3/files", 200, capture_at(3, 2))]);
        assert!(update_with(&api, "token", Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &prepared).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// night2 (the verifier): a result left half replaced (new splats beside the previous preview) passes every identity
    /// check, since nothing changes any more: it is refused, nothing sent.
    #[test]
    fn a_half_replaced_result_is_not_sent() {
        let (dir, session, records, mut entry) = uploaded_session("half", None);
        entry.edits = Some(Ok(orbiters_reconstruct::edit::UNEDITED.into()));
        let result = orbiters_core::reconstruction::result_dir(&session);
        std::fs::write(result.join(orbiters_core::reconstruction::REPLACING_FILE), b"").unwrap();
        let (api, seen) = stand_in(vec![("GET /captures/capture/3", 200, capture_at(3, 1))]);
        let refused = update_with(&api, "token", Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &prepared).unwrap_err().to_string();
        assert!(refused.contains("only partly saved"), "{refused}");
        assert_eq!(requests(&seen), ["GET /captures/capture/3"], "nothing sent");
        std::fs::remove_dir_all(dir).unwrap();
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

    /// Reconstructed again, an upload made without edits has nothing in its edits to say it is out of date: its record
    /// says so, whatever the new result's edits are, the window offers the update, and updating sends the new result and
    /// settles it. A capture never uploaded has no record to touch.
    #[test]
    fn a_capture_reconstructed_again_asks_to_update_what_is_shared() {
        let (dir, session, records, mut entry) = uploaded_session("reconstructed", None);
        entry.edits = Some(Ok(orbiters_reconstruct::edit::UNEDITED.into()));
        assert!(!entry.edited_since_upload(), "uploaded unedited, unedited now");
        mark_reconstructed(&records, &session);
        let record = record_of(&records).unwrap();
        assert_eq!(record.edits.as_deref(), Some(RECONSTRUCTED));
        entry.uploaded_edits = record.edits.clone();
        assert!(entry.edited_since_upload() && entry.reconstructed_since_upload());
        let (api, _seen) = stand_in(vec![("GET /captures/capture/3", 200, capture_at(3, 1)), ("PUT /captures/capture/3/files", 200, capture_at(3, 2))]);
        let (_, edits) = update_with(&api, "token", Ok(records.as_path()), &entry, &|_| {}, &mut Vec::new(), &prepared).unwrap();
        assert_eq!(edits, orbiters_reconstruct::edit::UNEDITED);
        assert_eq!(record_of(&records).unwrap().edits.as_deref(), Some(orbiters_reconstruct::edit::UNEDITED), "updated: the record has what was sent");
        entry.uploaded_edits = Some(edits);
        assert!(!entry.edited_since_upload(), "updated: nothing to update any more");
        mark_reconstructed(&records, &std::env::temp_dir().join("another session"));
        assert_eq!(record_of(&records).unwrap().edits.as_deref(), Some(orbiters_reconstruct::edit::UNEDITED), "another session's result is not this one's");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// What reconstructing again takes is said before it runs: splats hidden or deleted (and only those: a selection is
    /// not kept), edits that cannot be read, a public link, friends. A capture with none of these starts without asking.
    #[test]
    fn what_a_new_reconstruction_replaces_is_said_before_it_runs() {
        let (dir, session, records, _) = uploaded_session("replaces", None);
        let result = orbiters_core::reconstruction::result_dir(&session);
        let asked = |remote: Option<&Item>| replaces(&result, Some(&records), &session, remote);
        // Uploaded and public in its record, no edits.
        assert_eq!(asked(None).len(), 1, "{:?}", asked(None));
        assert!(asked(None)[0].starts_with("Its public link stays"));
        let private = Item { kind: Kind::Capture, id: 3, mine: true, public: false, ..Default::default() };
        assert!(asked(Some(&private)).is_empty(), "uploaded, private, shared with nobody: nothing anybody sees changes");
        // One then two splats deleted.
        delete_splat(&session, 1);
        assert!(asked(Some(&private))[0].starts_with("The splat hidden or deleted"), "{:?}", asked(Some(&private)));
        delete_splat(&session, 2);
        assert!(asked(Some(&private))[0].starts_with("The 2 splats hidden or deleted"), "{:?}", asked(Some(&private)));
        let friend = |id: u64| shares::Grant { id, vrchat_user_id: format!("usr_{id}"), ..Default::default() };
        let shared = Item { grants: vec![friend(1), friend(2)], ..private.clone() };
        let lines = asked(Some(&shared));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[1].starts_with("The 2 friends you shared it with keep seeing"), "{lines:?}");
        let both = Item { public: true, ..shared.clone() };
        assert!(asked(Some(&both))[1].starts_with("Its public link and the 2 friends"), "{:?}", asked(Some(&both)));
        std::fs::write(result.join(orbiters_core::reconstruction::EDIT_FILE), b"damaged").unwrap();
        assert!(asked(Some(&private))[0].starts_with("Its splat edits, which could not be read"), "{:?}", asked(Some(&private)));
        std::fs::remove_file(result.join(orbiters_core::reconstruction::EDIT_FILE)).unwrap();
        std::fs::remove_file(&records).unwrap();
        assert!(asked(None).is_empty(), "never uploaded, no edits: it starts without asking");
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
        let json = serde_json::to_string(&crate::ipc::ToService::CaptureShare { request: Box::new(request.clone()), epoch: 1 }).unwrap();
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

    /// What every file keeps exactly of a splat (VRChat's trees of version 2 and the website's `.splat` decoded from
    /// them): its log scales as half floats, its colour and opacity bytes (orbiters_reconstruct::tree_file::
    /// round_attributes makes the same values of either); a tree stores its position in steps of its box, within a
    /// fraction of a millimetre near the capture.
    type Fingerprint = [u32; 7];

    fn fingerprints(g: &orbiters_reconstruct::gaussians::Gaussians) -> Vec<Fingerprint> {
        let mut rounded = g.clone();
        orbiters_reconstruct::tree_file::round_attributes(&mut rounded);
        (0..rounded.len())
            .map(|i| {
                let (s, c) = (rounded.log_scales[i], rounded.colors[i]);
                [s[0], s[1], s[2], c[0], c[1], c[2], rounded.opacity_logits[i]].map(f32::to_bits)
            })
            .collect()
    }

    /// The removed splats of `g` (`ids`): each one's fingerprint and where it was (Unity's world).
    fn removed_of(g: &orbiters_reconstruct::gaussians::Gaussians, ids: impl Iterator<Item = usize>) -> std::collections::HashMap<Fingerprint, Vec<[f32; 3]>> {
        let mut picked = orbiters_reconstruct::gaussians::Gaussians::default();
        for i in ids {
            picked.push(g.means[i], g.log_scales[i], g.rotations[i], g.opacity_logits[i], g.colors[i]);
        }
        let mut removed: std::collections::HashMap<Fingerprint, Vec<[f32; 3]>> = std::collections::HashMap::new();
        for (i, f) in fingerprints(&picked).into_iter().enumerate() {
            removed.entry(f).or_default().push(picked.means[i]);
        }
        removed
    }

    /// How many splats of `g` (a file's, decoded) are one of `removed` (the same fingerprint within 1 cm), of how many.
    fn removed_in(g: &orbiters_reconstruct::gaussians::Gaussians, removed: &std::collections::HashMap<Fingerprint, Vec<[f32; 3]>>) -> (usize, usize) {
        let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() <= 0.01);
        let found = fingerprints(g).iter().enumerate().filter(|(i, f)| removed.get(*f).is_some_and(|at| at.iter().any(|p| near(*p, g.means[*i])))).count();
        (found, g.len())
    }

    /// A splat tree's splats, every box's, decoded.
    fn tree_splats(bytes: &[u8]) -> orbiters_reconstruct::gaussians::Gaussians {
        orbiters_reconstruct::tree::Tree::parse(bytes).unwrap().splats
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
        let shares::Shared::Item { item, .. } = shared else { panic!("shared as its owner") };
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
        let removed = removed_of(&g, gone.iter().copied());
        entry.edits = Some(Ok(edit.generation()));
        entry.uploaded_edits = record_of(&records).map(|r| r.edits.unwrap_or_else(|| orbiters_reconstruct::edit::UNEDITED.into()));
        assert!(entry.edited_since_upload(), "its window offers Update the shared capture");
        let pc_before = slot_file(slot, "pc").unwrap();
        assert!(removed_in(&tree_splats(&pc_before), &removed).0 > 0, "the shared copy holds the splats deleted");

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
        let (found, total) = removed_in(&orbiters_reconstruct::web::read_splat(&web).unwrap(), &removed);
        assert_eq!(found, 0, "the website's .splat holds none of the {} deleted splats ({total} splats)", removed.len());
        assert!(total as u64 <= (n - gone.len()) as u64 && total > 0);
        // What VRChat worlds load: the slot's three trees under the new key, none of the deleted splats; the listing.
        let key = |b: &[u8]| u32::from_le_bytes(b[20..24].try_into().unwrap());
        let pc = slot_file(slot, "pc").unwrap();
        for name in ["pc", "quest", "coarse"] {
            let tree = slot_file(slot, name).unwrap();
            assert_eq!(key(&tree), key(&pc));
            assert_eq!(removed_in(&tree_splats(&tree), &removed).0, 0, "slot {name} holds none of the deleted splats");
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

    /// Private is an upload kept on the account and listed nowhere: no share with nobody is asked (Orbiters refuses it with
    /// "Choose someone to share with.", the tour's first publish on 2026-11-15). A 429 is said with its Retry-After, and a
    /// publish stopped after its upload was accepted goes on with the share alone: nothing is sent twice.
    #[test]
    fn a_private_publish_is_kept_on_the_account_and_a_wait_is_said_and_resumed_without_sending_again() {
        let private = shares::Share { public: false, recipients: Vec::new(), ..Default::default() };
        let public = shares::Share { public: true, recipients: Vec::new(), ..Default::default() };
        let kept = serde_json::json!({ "item": Item { kind: Kind::Capture, id: 5, mine: true, can_share: true, public: false, ..Default::default() } }).to_string();
        let added = |id: u64| format!(r#"{{"type":"capture","id":{id},"visibility":"private"}}"#);
        let too_many = r#"{"error":"Too many attempts. Please wait before trying again."}"#.to_string();

        let (dir, _session, records, mut entry) = uploaded_session("private", None);
        std::fs::remove_file(&records).unwrap();
        entry.world_id = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd".into();
        let (api, seen) = stand_in(vec![("POST /maps/worlds/", 200, added(5)), ("GET /captures/capture/5", 200, kept.clone())]);
        let result = share_with(&api, "token", Ok(records.clone()), &entry, &private, &|_| {}, &mut Vec::new(), &prepared).unwrap();
        assert!(matches!(result, shares::Shared::Item { ref item, .. } if item.id == 5 && !item.public));
        let asked = requests(&seen);
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert!(asked[0].starts_with("POST /maps/worlds/") && asked[0].ends_with("/previews") && asked[1] == "GET /captures/capture/5", "{asked:?}");
        let uploaded = std::str::from_utf8(&seen.lock().unwrap()[0].1).unwrap().contains(r#""visibility":"private""#);
        assert!(uploaded, "sent as private");
        assert_eq!(record_of(&records).unwrap().item.map(|i| i.id), Some(5), "remembered: published, private");
        std::fs::remove_dir_all(dir).ok();

        // Orbiters' limiter refuses the upload: said with its wait, nothing remembered as sent.
        let (dir, _session, records, mut entry) = uploaded_session("limited", None);
        std::fs::remove_file(&records).unwrap();
        entry.world_id = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd".into();
        let (api, _seen) = stand_in(vec![("POST /maps/worlds/", 429, too_many.clone())]);
        let refused = share_with(&api, "token", Ok(records.clone()), &entry, &public, &|_| {}, &mut Vec::new(), &prepared).unwrap_err();
        let limited = refused.downcast_ref::<shares::RateLimited>().expect("a wait, not a failure");
        assert_eq!(limited.retry_after_s, 1200);
        assert!(record_of(&records).is_none());
        // Uploaded, then the share refused: the next try only shares.
        let (api, _seen) = stand_in(vec![("POST /maps/worlds/", 200, added(6)), ("POST /captures/capture/6/share", 429, too_many), ("GET /captures/capture/6", 200, capture_at(6, 1))]);
        let refused = share_with(&api, "token", Ok(records.clone()), &entry, &public, &|_| {}, &mut Vec::new(), &prepared).unwrap_err();
        assert!(refused.downcast_ref::<shares::RateLimited>().is_some(), "{refused}");
        let (api, seen) = stand_in(vec![("POST /captures/capture/6/share", 200, capture_at(6, 1))]);
        share_with(&api, "token", Ok(records.clone()), &entry, &public, &|_| {}, &mut Vec::new(), &prepared).unwrap();
        assert_eq!(requests(&seen), ["POST /captures/capture/6/share"], "its upload is not sent again");
        std::fs::remove_dir_all(dir).ok();
    }

    /// CTL2-4: an item this computer uploaded and that was removed on Orbiters (its share answers Orbiters' own 404)
    /// loses its record and is uploaded again, once; the record then names the new item. A 404 that is not Orbiters'
    /// (no JSON error) changes nothing.
    #[test]
    fn a_share_of_an_upload_removed_on_orbiters_uploads_it_again_once() {
        let public = shares::Share { public: true, recipients: Vec::new(), ..Default::default() };
        let gone = r#"{"error":"Capture not found"}"#.to_string();
        let added = r#"{"type":"capture","id":9,"visibility":"public"}"#.to_string();
        let (dir, _session, records, mut entry) = uploaded_session("removed", None);
        entry.world_id = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd".into();
        let (api, seen) = stand_in(vec![
            ("POST /captures/capture/3/share", 404, gone),
            ("POST /maps/worlds/", 200, added),
            // The share's answer shows what was asked (public), or it is read again (CORE2-2).
            ("POST /captures/capture/9/share", 200, serde_json::to_string(&shares::Shared::Item { explorer: None, item: Item { kind: Kind::Capture, id: 9, mine: true, can_share: true, public: true, ..Default::default() } }).unwrap()),
        ]);
        share_with(&api, "token", Ok(records.clone()), &entry, &public, &|_| {}, &mut Vec::new(), &prepared).unwrap();
        let asked = requests(&seen);
        assert_eq!(asked.len(), 3, "{asked:?}");
        assert_eq!(asked[0], "POST /captures/capture/3/share");
        assert!(asked[1].starts_with("POST /maps/worlds/"), "uploaded again: {asked:?}");
        assert_eq!(asked[2], "POST /captures/capture/9/share");
        assert_eq!(record_of(&records).map(|r| r.uploaded.id), Some(9), "the record names the new upload");
        std::fs::remove_dir_all(dir).ok();

        // A 404 from something in front of Orbiters (no JSON error): the record stays, nothing is uploaded.
        let (dir, _session, records, mut entry) = uploaded_session("proxy", None);
        entry.world_id = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd".into();
        let (api, seen) = stand_in(vec![("POST /captures/capture/3/share", 404, "<html>Not Found</html>".into())]);
        assert!(share_with(&api, "token", Ok(records.clone()), &entry, &public, &|_| {}, &mut Vec::new(), &prepared).is_err());
        assert_eq!(requests(&seen), ["POST /captures/capture/3/share"]);
        assert_eq!(record_of(&records).map(|r| r.uploaded.id), Some(3));
        std::fs::remove_dir_all(dir).ok();
    }

    /// CTL2-11 f: with no upload record possible (the account has no id), nothing is sent, so a retry sends nothing twice.
    #[test]
    fn no_upload_is_sent_when_its_record_cannot_be_kept() {
        let public = shares::Share { public: true, recipients: Vec::new(), ..Default::default() };
        let (dir, _session, records, mut entry) = uploaded_session("no-record", None);
        std::fs::remove_file(&records).unwrap();
        entry.world_id = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd".into();
        let (api, seen) = stand_in(vec![]);
        let refused = share_with(&api, "token", Err(anyhow::anyhow!("Orbiters did not identify this account")), &entry, &public, &|_| {}, &mut Vec::new(), &prepared).unwrap_err();
        assert!(refused.to_string().contains("did not identify"), "{refused}");
        assert!(requests(&seen).is_empty(), "nothing sent");
        std::fs::remove_dir_all(dir).ok();
    }

    /// CTL2-11 k: the library moved to another folder: a capture's record follows it to the folder of the same name, and
    /// the rest stays as it was.
    #[test]
    fn upload_records_follow_a_moved_library() {
        let root = std::env::temp_dir().join(format!("orbiters-moved-{}", uuid::Uuid::new_v4()));
        let (old, new) = (root.join("old"), root.join("new"));
        for name in ["Home 1", "Here still"] {
            std::fs::create_dir_all(new.join(name)).unwrap();
            std::fs::write(new.join(name).join("manifest.json"), b"{}").unwrap();
        }
        std::fs::create_dir_all(old.join("Here still")).unwrap();
        let record = |path: PathBuf, kind: Kind, id: u64| Record { path, item: None, uploaded: Uploaded { kind, id, visibility: "private".into(), explorer: None }, edits: None };
        let file = root.join("capture-uploads.json");
        let records = vec![
            record(old.join("Home 1"), Kind::Capture, 1),
            record(old.join("Here still"), Kind::Capture, 2),
            record(old.join("Nowhere"), Kind::Capture, 3),
            record(old.join("Home 1"), Kind::Picture, 4),
        ];
        std::fs::write(&file, serde_json::to_vec(&records).unwrap()).unwrap();
        assert_eq!(follow_moved_sessions(&file, &new), 1);
        let now: Vec<PathBuf> = read_records(&file).into_iter().map(|r| r.path).collect();
        assert_eq!(now, [new.join("Home 1"), old.join("Here still"), old.join("Nowhere"), old.join("Home 1")]);
        assert_eq!(follow_moved_sessions(&file, &new), 0, "once");
        std::fs::remove_dir_all(root).ok();
    }

    /// An item uploaded before (its record says so) is shared with one request: nothing sent again, nothing fetched.
    #[test]
    fn an_uploaded_item_is_shared_with_one_request() {
        let dir = std::env::temp_dir().join(format!("orbiters-share-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = dir.join("Silent Dawn 2026-10-28 23-52-12");
        let records = dir.join("capture-uploads.json");
        let uploaded = Uploaded { kind: Kind::Capture, id: 3, visibility: "private".into(), explorer: None };
        std::fs::write(&records, serde_json::to_vec(&[Record { path: session.clone(), item: None, uploaded, edits: None }]).unwrap()).unwrap();
        let (api, seen) = fake_orbiters(vec![("POST /captures/capture/3/share", shared_item(Kind::Capture, 3))]);
        let mut entry = remote_entry(Item::default());
        entry.remote = None;
        entry.local = Some(Local::Session(session.clone()));
        let stages = Mutex::new(Vec::new());
        let result = share_with(&api, "token", Ok(records.clone()), &entry, &to_a_friend(), &|s| stages.lock().unwrap().push(s), &mut Vec::new(), &prepared).unwrap();
        assert!(matches!(result, shares::Shared::Item { ref item, .. } if item.grants.len() == 1));
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
        // A real PNG, with what VRChat writes of its taker: the copy sent leaves it out (PRIV-8).
        let png = {
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(&mut bytes, 4, 4);
            encoder.set_color(png::ColorType::Rgba);
            encoder.add_itxt_chunk("XML:com.adobe.xmp".into(), "vrc:AuthorID=usr_00000000-0000-4000-8000-000000000001".into()).unwrap();
            encoder.write_header().unwrap().write_image_data(&[7u8; 64]).unwrap();
            bytes
        };
        std::fs::write(&picture, &png).unwrap();
        let mut clean = Vec::new();
        let sent = orbiters_core::png_meta::copy_without_metadata(&mut png.as_slice(), &mut clean).unwrap();
        assert!(sent < png.len() as u64 && !String::from_utf8_lossy(&clean).contains("usr_"));
        let records = dir.join("capture-uploads.json");
        let world = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";
        let added = r#"{"added":[{"type":"picture","id":9,"visibility":"private"}],"skipped":0}"#.to_string();
        let (api, bodies) = stand_in(vec![(r"POST /maps/worlds/", 200, added), ("POST /captures/picture/9/share", 200, shared_item(Kind::Picture, 9))]);
        let mut entry = remote_entry(Item::default());
        entry.remote = None;
        entry.world_id = world.into();
        entry.source_key = Some("a".repeat(64));
        entry.local = Some(Local::Picture(picture.clone()));
        let stages = Mutex::new(Vec::new());
        share_with(&api, "token", Ok(records.clone()), &entry, &to_a_friend(), &|s| stages.lock().unwrap().push(s), &mut Vec::new(), &prepared).unwrap();
        let bodies = bodies.lock().unwrap().clone();
        assert_eq!(bodies.iter().map(|(line, _)| line.clone()).collect::<Vec<_>>(), [format!("POST /maps/worlds/{world}/pictures"), "POST /captures/picture/9/share".to_string()]);
        // What actually went (verifier): the picture part of the request holds the clean copy, never the taker.
        let request_text = String::from_utf8_lossy(&bodies[0].1);
        assert!(!request_text.contains("usr_00000000") && !request_text.contains("XML:com.adobe.xmp"), "the metadata never leaves");
        assert!(bodies[0].1.windows(clean.len()).any(|w| w == clean.as_slice()), "the clean copy is what was sent");
        assert_eq!(*stages.lock().unwrap(), [Stage::Sending { sent: 0, total: sent }, Stage::Sharing]);
        let saved = read_records(&records);
        assert_eq!((saved[0].uploaded.id, saved[0].item.as_ref().map(|i| i.id)), (9, Some(9)));
        assert_eq!(std::fs::read(&picture).unwrap(), png, "the picture here is untouched");
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
            Ok(shares::Shared::Item { explorer: None, item: e.remote.unwrap() })
        };
        let first = share_all(entries.clone(), &|s| stages.lock().unwrap().push(s), one).unwrap();
        assert!(matches!(first, shares::Shared::Item { ref item, .. } if item.id == 0), "the first item's answer");
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
            Ok(shares::Shared::Item { explorer: None, item: e.remote.unwrap() })
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
        drop(Changing::of(&b, &|_| panic!("another item never waits")));
        let (waiting, announced) = std::sync::mpsc::channel();
        let (entered, complete) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _second = Changing::of(&a, &|stage| waiting.send(stage).unwrap());
            entered.send(()).unwrap();
        });
        let guard = std::time::Duration::from_secs(30);
        assert_eq!(announced.recv_timeout(guard).unwrap(), Stage::Waiting);
        assert_eq!(complete.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty), "the same item waits");
        drop(held);
        complete.recv_timeout(guard).unwrap();
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


    /// A small picture as PNG bytes (what a stand-in Orbiters serves as a capture's picture).
    pub(crate) fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        use image::ImageEncoder;
        let rgb: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, 90, 200]).collect();
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).unwrap();
        out
    }

    /// A stand-in for Orbiters that serves `bytes` as a picture to every request: each request line and whether it came
    /// with the account's credential.
    /// Each request line a picture stand-in got, and whether it came with the account's credential.
    pub(crate) type Asked = std::sync::Arc<Mutex<Vec<(String, bool)>>>;

    pub(crate) fn picture_server(bytes: Vec<u8>) -> (Api, Asked) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = Api::at(&format!("http://{}/", listener.local_addr().unwrap()));
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut signed = false;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    signed |= header.to_ascii_lowercase().starts_with("authorization: bearer token");
                }
                log.lock().unwrap().push((line.split_whitespace().take(2).collect::<Vec<_>>().join(" "), signed));
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len());
                let _ = stream.write_all(&bytes);
            }
        });
        (api, seen)
    }

    /// A 3D capture of this account on Orbiters (`id`), with its picture there or not.
    pub(crate) fn capture_item(id: u64, public: bool, pictured: bool) -> Item {
        let capture = shares::Capture {
            splat_count: 2000,
            preview_url: format!("/captures/capture/{id}/file"),
            picture_url: pictured.then(|| format!("/captures/capture/{id}/file?part=picture")),
            revision: 1,
            ..Default::default()
        };
        Item { kind: Kind::Capture, id, mine: true, can_share: true, public, capture: Some(capture), ..Default::default() }
    }

    /// L-no-thumbnails (2026-11-15): the world tour's captures, published and then deleted from this computer, showed
    /// grey cards in Photos & captures. Orbiters had every one's picture (`capture.pictureUrl`, private and public alike);
    /// the card of a capture on Orbiters only was given none (only a picture's own picture was asked for). Now it is
    /// that picture, read with the account and kept on this computer: shown again with Orbiters out of reach.
    #[test]
    fn a_published_capture_whose_folder_is_gone_still_shows_its_picture() {
        let (api, seen) = picture_server(png_bytes(64, 36));
        let cache = std::env::temp_dir().join(format!("orbiters-capture-view-{}", uuid::Uuid::new_v4()));
        for (id, public) in [(65, false), (66, true)] {
            let item = capture_item(id, public, true);
            // As the gallery lists it with no folder here.
            let entry = remote_entry(item.clone());
            assert!(entry.local.is_none());
            let picture = remote_picture(&item, false, false).expect("a card picture for a capture on Orbiters only");
            let crate::worlds::Picture::CaptureView { development: false, id: got, url, large: false } = &picture else { panic!("{picture:?}") };
            assert_eq!((*got, url.as_str()), (id, format!("/captures/capture/{id}/file?part=picture").as_str()));
            let file = cache.join(format!("{id}.img"));
            let image = crate::worlds::capture_view(&api, Some("token"), url, false, Some(&file)).expect("its picture");
            assert_eq!((image.width(), image.height()), (64, 36));
            assert!(file.is_file(), "kept on this computer");
            // Orbiters out of reach: the kept one.
            let gone = Api::at("http://127.0.0.1:9/");
            assert!(crate::worlds::capture_view(&gone, Some("token"), url, false, Some(&file)).is_some());
        }
        assert_eq!(
            *seen.lock().unwrap(),
            [("GET /captures/capture/65/file?part=picture&thumbnail=true".to_string(), true), ("GET /captures/capture/66/file?part=picture&thumbnail=true".to_string(), true)],
            "the thumbnail, asked as the account, once each"
        );
        // The window's large picture is the whole one; no picture on Orbiters, no card picture; a picture's own as before.
        let item = capture_item(66, true, true);
        assert!(matches!(remote_picture(&item, true, true), Some(crate::worlds::Picture::CaptureView { development: true, large: true, .. })));
        assert!(remote_picture(&capture_item(67, false, false), false, false).is_none());
        let mut elsewhere = capture_item(68, true, true);
        elsewhere.capture.as_mut().unwrap().picture_url = Some("https://elsewhere.example/captures/capture/68/file?part=picture".into());
        assert!(remote_picture(&elsewhere, false, false).is_none(), "the credential never goes to another host");
        let picture = Item { kind: Kind::Picture, id: 9, picture: Some(shares::Picture::default()), ..Default::default() };
        assert_eq!(remote_picture(&picture, false, false), Some(crate::worlds::Picture::Capture { development: false, id: 9, large: false }));
        std::fs::remove_dir_all(cache).ok();
    }

    /// A reconstructed session in its own sessions folder, with the picture a share sends (its thumbnail).
    fn pictured_session(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("orbiters-parity-{name}-{}", uuid::Uuid::new_v4()));
        let session = root.join("Freddy's Custom Pizzeria 2.0 2026-11-15 13-24-21");
        std::fs::create_dir_all(orbiters_core::reconstruction::dir(&session)).unwrap();
        let mut m = orbiters_contracts::manifest::SessionManifest::new("Freddy".into(), "2026-11-15T12:24:21Z".into(), "test");
        m.world = Some(orbiters_contracts::manifest::WorldInfo { id: Some("wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd".into()), name: Some("Freddy's Custom Pizzeria 2.0".into()), ..Default::default() });
        std::fs::write(session.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
        let pixels: Vec<u8> = (0..32 * 18).flat_map(|i| [(i % 255) as u8, 80, 160, 255]).collect();
        std::fs::write(session.join(orbiters_core::bundle::PICTURE_THUMBNAIL_FILE), orbiters_contracts::qoi::encode(&pixels, 32, 18, 4)).unwrap();
        (root, session)
    }

    /// The bytes of a multipart body's part `name`.
    fn part(body: &[u8], name: &str) -> Option<Vec<u8>> {
        let marker = format!("name=\"{name}\"");
        let at = body.windows(marker.len()).position(|w| w == marker.as_bytes())?;
        let start = at + body[at..].windows(4).position(|w| w == b"\r\n\r\n")? + 4;
        let end = start + body[start..].windows(4).position(|w| w == b"\r\n--")?;
        Some(body[start..end].to_vec())
    }

    /// The bridge's `publish_3d_picture` (its entry: `session_entry`) uploads exactly the picture Photos & captures' own
    /// share (its entry: the library's) uploads with a capture, private and public alike.
    #[test]
    fn the_bridges_publish_sends_the_picture_a_persons_share_sends() {
        let (root, session) = pictured_session("bridge");
        let bridge = session_entry(&session, Server::Development).unwrap();
        let person = local_entries_at(&root, Default::default(), &root.join("no-history.json"), None)
            .into_iter()
            .find(|e| e.local.as_ref().is_some_and(|l| l.path() == session))
            .expect("listed by the library");
        let expected = crate::capture_picture::jpeg(&session).unwrap();
        let added = r#"{"type":"capture","id":5,"visibility":"private"}"#.to_string();
        let item = |public: bool| serde_json::json!({ "item": capture_item(5, public, true) }).to_string();
        let shared = |public: bool| serde_json::to_string(&shares::Shared::Item { explorer: None, item: capture_item(5, public, true) }).unwrap();
        for public in [false, true] {
            let request = shares::Share { public, ..Default::default() };
            let mut pictures = Vec::new();
            for (who, entry) in [("bridge", &bridge), ("person", &person)] {
                let records = root.join(format!("records-{who}-{public}.json"));
                let (api, seen) = stand_in(vec![("POST /maps/worlds/", 200, added.clone()), ("GET /captures/capture/5", 200, item(public)), ("POST /captures/capture/5/share", 200, shared(public))]);
                share_with(&api, "token", Ok(records), entry, &request, &|_| {}, &mut Vec::new(), &prepared).unwrap();
                let seen = seen.lock().unwrap();
                let (_, body) = seen.iter().find(|(r, _)| r.ends_with("/previews")).expect("uploaded");
                pictures.push(part(body, "picture").unwrap_or_else(|| panic!("{who} sent no picture (public {public})")));
            }
            assert_eq!(pictures[0], expected, "the bridge's picture (public {public})");
            assert_eq!(pictures[0], pictures[1], "the same as a person's share (public {public})");
        }
        std::fs::remove_dir_all(root).ok();
    }

    /// Uploaded without its picture (before captures had them, or while it could not leave the app): a private publish
    /// gives it now as a public share did, and freeing its folder gives it first, so its card keeps a picture; with the
    /// picture there, nothing more is asked.
    #[test]
    fn a_capture_without_its_picture_gets_it_private_or_before_its_folder_goes() {
        let (root, session) = pictured_session("give");
        let mut entry = session_entry(&session, Server::Development).unwrap();
        entry.world_id = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd".into();
        let private = shares::Share { public: false, ..Default::default() };
        let without = serde_json::json!({ "item": capture_item(5, false, false) }).to_string();
        let with = serde_json::json!({ "item": capture_item(5, false, true) }).to_string();
        let records = root.join("records.json");
        let (api, seen) = stand_in(vec![
            ("POST /maps/worlds/", 200, r#"{"type":"capture","id":5,"visibility":"private"}"#.to_string()),
            ("GET /captures/capture/5", 200, without.clone()),
            ("POST /captures/capture/5/picture", 200, with.clone()),
        ]);
        share_with(&api, "token", Ok(records.clone()), &entry, &private, &|_| {}, &mut Vec::new(), &prepared).unwrap();
        let asked = requests(&seen);
        assert_eq!(&asked[1..], ["GET /captures/capture/5", "POST /captures/capture/5/picture"], "{asked:?}");
        assert_eq!(part(&seen.lock().unwrap()[2].1, "picture"), crate::capture_picture::jpeg(&session));
        assert!(record_of(&records).unwrap().item.unwrap().capture.unwrap().picture_url.is_some(), "remembered with its picture");

        // Before its folder is deleted: the record says it has its picture, nothing asked.
        let (api, seen) = stand_in(vec![]);
        assert_eq!(keep_picture_with(&api, "token", &records, &session).unwrap(), None);
        assert!(requests(&seen).is_empty());
        // A record without it: asked, given, remembered.
        let mut kept = read_records(&records);
        kept[0].item = Some(capture_item(5, false, false));
        std::fs::write(&records, serde_json::to_vec(&kept).unwrap()).unwrap();
        let (api, seen) = stand_in(vec![("GET /captures/capture/5", 200, without), ("POST /captures/capture/5/picture", 200, with)]);
        assert_eq!(keep_picture_with(&api, "token", &records, &session).unwrap(), Some(5));
        assert_eq!(requests(&seen), ["GET /captures/capture/5", "POST /captures/capture/5/picture"]);
        assert!(record_of(&records).unwrap().item.unwrap().capture.unwrap().picture_url.is_some());
        // Nothing to make it from: said, so the bridge keeps the folder.
        std::fs::remove_file(session.join(orbiters_core::bundle::PICTURE_THUMBNAIL_FILE)).unwrap();
        let mut kept = read_records(&records);
        kept[0].item = Some(capture_item(5, false, false));
        std::fs::write(&records, serde_json::to_vec(&kept).unwrap()).unwrap();
        let (api, _) = stand_in(vec![("GET /captures/capture/5", 200, serde_json::json!({ "item": capture_item(5, false, false) }).to_string())]);
        assert!(keep_picture_with(&api, "token", &records, &session).is_err());
        std::fs::remove_dir_all(root).ok();
    }
}

/// REL-5: "five minutes ago" right after a boot (or any span longer than the uptime) is the clock's start, never a
/// panic that kills the presence thread.
#[test]
fn a_time_before_the_boot_is_the_clocks_start() {
    let long_ago = ago(std::time::Duration::from_secs(u64::MAX / 4));
    assert!(long_ago.elapsed() < std::time::Duration::from_secs(5));
    assert!(ago(std::time::Duration::from_millis(1)).elapsed() >= std::time::Duration::from_millis(1));
}
