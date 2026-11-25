//! VRChat groups in the window (REQUEST22 1): the Groups section at the bottom of Home (the groups the signed-in account
//! owns or helps run), each group's page (Look, Members, Roles, Posts) and the banner studio.
//!
//! What it shows is asked of the service on demand and answered to this window alone (`controller::groups`). On disk,
//! per account: the studio's own crops and exports, and the new posts the person is writing (their own words, nothing of
//! anyone else's; a post is not edited here, its page on vrchat.com is). Everything is bound to the account scope it was asked under: another account (or "Connecting") drops it all, confirmations included (B3).
//! Pictures of groups and members, and what the pages last read (the cards, a group's page, its first members and posts),
//! are kept on this computer in the account's own folder (REQUEST28, `orbiters_core::vrchat_store`: the service writes, this
//! reads): they show at once, and change in place when VRChat answers. While the saved session is being restored
//! ([`State::held`]) they show for that very account, and nothing is asked or sent.
//!
//! The window owns no write. A press prepares one; the service seals it; the confirmation is drawn from the sealed object
//! and nothing else; Send names it and the digest of what was drawn. Leaving the view it was asked from ends it unsent.
//! Once sent it is the ledger's (`controller::groups_ledger`): what may have been done is listed here from there, with its
//! check, across tabs, pages, windows and restarts (verify2 B-A, B-E, B-F). A check says what VRChat shows now, never what
//! happened; what it cannot tell, and what VRChat does not show, are the person's to decide (verify3 Blocker 2, Major 3).
pub mod art;
mod page;
mod studio;
pub(crate) use page::{Composer, RoleEditor, Tab};

use super::Cx;
use crate::backend::Ctl;
use crate::controller::groups::{Answer as Looked, Candidate, Choice, Entry, Failure, Kind as Wrote, Likeness, Observation, Outcome, Reply, Request, Sealed, State as Stands, Unclear, Warning, Write};
use crate::controller::vrchat_data::Scope;
use crate::controller::vrchat_login::Presence;
use art::Drafts;
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui};
use orbiters_core::groups::{ArtSlot, FileTag, GroupCard, Missing, Part, Why};
use orbiters_core::vrchat_api::GroupFailure;
use orbiters_i18n::tr;
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{paint, widgets};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc;

/// Home's list is read again after this long (seconds), when Home shows.
const READ_AGAIN: f64 = 300.0;
/// The ledger is asked for again this long after it could not be read (seconds), and a card's name after it failed.
const ASK_AGAIN: f64 = 3.0;
const HYDRATE_AGAIN: f64 = 60.0;
/// Pictures kept in memory at most, and asked at once.
const PICTURES_MAX: usize = 120;
const PICTURES_AT_ONCE: usize = 2;
/// A group card on Home.
const CARD_H: f32 = 132.0;
const CARD_MIN_W: f32 = 300.0;
const GAP: f32 = 14.0;

/// What a request was for, to take its answer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Kind {
    Discover,
    Hydrate,
    Open(String),
    Members { group: String, query: String, offset: usize },
    Posts { group: String, offset: usize },
    Inventory { tag: FileTag, offset: usize },
    Picture(String),
    /// A picture of the account's gallery read in full for the banner studio.
    Source(String),
    /// A write asked to be sealed, by its number: an answer to another number is not taken.
    Prepare(u64),
    /// The sealed picture of a write, and the SHA-256 it must have.
    Art { op: String, sha256: String },
    Commit(String),
    Cancel(String),
    Ledger,
    Check(String),
    /// What the person decided of a write.
    Decide(String, Choice),
    SetAside,
    /// What moved beside a write is seen to.
    SeenTo(String),
}

pub(crate) struct Answer {
    generation: u64,
    kind: Kind,
    reply: Reply,
    /// A picture, decoded on the request's thread.
    image: Option<egui::ColorImage>,
    /// A studio's source picture, decoded whole and oriented.
    source: Option<image::RgbaImage>,
}

/// Requests on threads of their own; their answers taken at the next frame. A new generation (another account, the
/// page closed) makes every answer still on its way obsolete; what is running is counted by generation, so a late
/// answer of an old one never clears the mark of a new one (Major 9).
pub(crate) struct Jobs {
    tx: mpsc::Sender<Answer>,
    rx: mpsc::Receiver<Answer>,
    generation: u64,
    running: Vec<(u64, Kind)>,
}

impl Default for Jobs {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, generation: 1, running: Vec::new() }
    }
}

impl Jobs {
    fn running(&self, kind: &Kind) -> bool {
        self.running.iter().any(|(g, k)| *g == self.generation && k == kind)
    }
    /// Whether a read of this sort runs for the current generation (one list read at a time per list).
    pub(crate) fn busy_with(&self, matches: impl Fn(&Kind) -> bool) -> bool {
        self.running.iter().any(|(g, k)| *g == self.generation && matches(k))
    }
    fn pictures_running(&self) -> usize {
        self.running.iter().filter(|(g, k)| *g == self.generation && matches!(k, Kind::Picture(_))).count()
    }
    /// Asks `request` unless the same is on its way.
    pub(crate) fn ask(&mut self, ctx: &egui::Context, ctl: &Ctl, scope: &Scope, kind: Kind, request: Request) {
        if self.running(&kind) {
            return;
        }
        self.running.push((self.generation, kind.clone()));
        let (tx, ctl, ctx, scope, generation) = (self.tx.clone(), ctl.clone(), ctx.clone(), scope.clone(), self.generation);
        // Work of the window while it runs: it does not rest to the tray in the middle of it.
        let working = crate::pages::Working::begin();
        std::thread::spawn(move || {
            let _working = working;
            let account = scope.account();
            let reply = ctl.groups(scope, request);
            let image = match (&kind, &reply) {
                (Kind::Picture(url), Reply::Picture(bytes)) => kept_picture(account.as_ref(), url, bytes),
                // Exactly the sealed bytes, or nothing (B-E).
                (Kind::Art { sha256, .. }, Reply::Picture(bytes)) if digest(bytes) == *sha256 => decode_picture(bytes),
                _ => None,
            };
            let source = match (&kind, &reply) {
                (Kind::Source(_), Reply::Picture(bytes)) => art::decode_source(bytes).ok(),
                _ => None,
            };
            // The bytes are not kept past decoding.
            let reply = if matches!(reply, Reply::Picture(_)) { Reply::Cancelled } else { reply };
            let _ = tx.send(Answer { generation, kind, reply, image, source });
            ctx.request_repaint();
        });
    }
    fn take(&mut self) -> Vec<Answer> {
        let mut out = Vec::new();
        while let Ok(a) = self.rx.try_recv() {
            if let Some(i) = self.running.iter().position(|(g, k)| *g == a.generation && *k == a.kind) {
                self.running.remove(i);
            }
            if a.generation == self.generation {
                out.push(a);
            }
        }
        out
    }
    fn reset(&mut self) {
        self.generation += 1;
        // What ran for older generations no longer counts against anything (their answers are dropped).
        self.running.retain(|(g, _)| *g == self.generation);
    }
}

fn digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A picture as a texture's pixels, at most 640 px a side, decoded within bounds (Major 5).
fn decode_picture(bytes: &[u8]) -> Option<egui::ColorImage> {
    smaller(bytes).map(|img| egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

fn smaller(bytes: &[u8]) -> Option<image::RgbaImage> {
    let img = art::decode_bounded(bytes, art::PICTURE_PIXELS).ok()?;
    Some(if img.width() > 640 || img.height() > 640 { image::imageops::thumbnail(&img, 640.min(img.width()), ((640.0 * img.height() as f32 / img.width() as f32) as u32).clamp(1, 640)) } else { img })
}

/// A picture VRChat sent, kept for the account it was asked for (on the request's thread).
fn kept_picture(account: Option<&orbiters_core::vrchat_store::Account>, url: &str, bytes: &[u8]) -> Option<egui::ColorImage> {
    use crate::worlds::profile_pictures::{Profiles, Size};
    let img = smaller(bytes)?;
    if account.is_some() {
        Profiles::of(account).keep_sent(url, Size::Wide, &img, bytes);
    }
    Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

/// What was last read of the groups of `scope`'s account: of `kind`, under `id`.
pub(crate) fn kept<T: serde::de::DeserializeOwned>(scope: &Scope, kind: &str, id: &str) -> Option<T> {
    use orbiters_core::vrchat_store as store;
    Some(store::Store::read(&kept_root(), &scope.account()?)?.get::<T>(kind, id)?.value)
}

#[cfg(not(test))]
fn kept_root() -> std::path::PathBuf {
    orbiters_core::vrchat_store::root()
}
/// Tests: a folder of the test's own.
#[cfg(test)]
fn kept_root() -> std::path::PathBuf {
    KEPT_ROOT.with(|r| r.borrow().clone()).unwrap_or_else(|| orbiters_core::paths::cache_dir().join("no-vrchat-store"))
}
#[cfg(test)]
thread_local! {
    pub(crate) static KEPT_ROOT: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}

enum Pic {
    Waiting,
    Ready(egui::TextureHandle),
    Failed,
}

/// Group and member pictures, the newest [`PICTURES_MAX`] as textures (M10), the others read again from this computer. A
/// picture that failed is not asked again by itself.
#[derive(Default)]
pub(crate) struct Pictures {
    map: HashMap<String, Pic>,
    order: VecDeque<String>,
    /// Every picture ever asked, for the tests' count of requests.
    #[cfg(test)]
    asked: usize,
}

impl Pictures {
    /// The texture of `url` if it is here; asked for when it is not (a few at a time). Call only for what is in view.
    pub(crate) fn get(&mut self, ctx: &egui::Context, jobs: &mut Jobs, ctl: &Ctl, scope: &Scope, url: &str) -> Option<egui::TextureHandle> {
        if !url.starts_with("https://") {
            return None;
        }
        match self.map.get(url) {
            Some(Pic::Ready(t)) => return Some(t.clone()),
            Some(_) => return None,
            None => {}
        }
        // Shown before, in this run or another: in this very frame, without VRChat (REQUEST28).
        match crate::worlds::at_once::wide(ctx, scope, url) {
            crate::worlds::at_once::Wide::Ready(t) => {
                self.arrived_as(url.to_string(), Pic::Ready(t.clone()));
                return Some(t);
            }
            crate::worlds::at_once::Wide::Later => return None,
            crate::worlds::at_once::Wide::No => {}
        }
        // (Nothing is asked of an account still on its way back.)
        if !scope.signed_in() || jobs.pictures_running() >= PICTURES_AT_ONCE {
            return None;
        }
        self.map.insert(url.to_string(), Pic::Waiting);
        #[cfg(test)]
        {
            self.asked += 1;
        }
        jobs.ask(ctx, ctl, scope, Kind::Picture(url.to_string()), Request::Picture { url: url.to_string() });
        None
    }
    fn arrived(&mut self, ctx: &egui::Context, url: String, image: Option<egui::ColorImage>) {
        let pic = match image {
            Some(img) => Pic::Ready(ctx.load_texture(format!("group-picture-{}", self.order.len()), img, egui::TextureOptions::LINEAR)),
            None => Pic::Failed,
        };
        self.arrived_as(url, pic);
    }
    fn arrived_as(&mut self, url: String, pic: Pic) {
        self.map.insert(url.clone(), pic);
        self.order.retain(|u| *u != url);
        self.order.push_back(url);
        while self.order.len() > PICTURES_MAX {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }
    /// A picture given by the screenshot review (made-up art).
    pub(crate) fn stage(&mut self, ctx: &egui::Context, url: &str, img: egui::ColorImage) {
        self.arrived(ctx, url.to_string(), Some(img));
    }
    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
    /// What was on its way or failed when the page went is asked again when it is back (J4).
    fn forget_unread(&mut self) {
        self.map.retain(|_, p| matches!(p, Pic::Ready(_)));
        let map = &self.map;
        self.order.retain(|u| map.contains_key(u));
    }
}

#[derive(Default)]
pub(crate) enum Discovery {
    #[default]
    Idle,
    Loading,
    Ready(Vec<GroupCard>, f64),
    Failed(Failure, f64),
}

/// A write on its way: asked to be sealed, shown for the person's yes, sent.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Flow {
    Preparing { token: u64 },
    /// `art`: the sealed picture, decoded from the sealed bytes; `None` until it is here.
    Confirming { sealed: Sealed, serial: u64, origin: Rect, art: Option<egui::TextureHandle> },
    Sending { sealed: Sealed },
}

/// A write whose answer did not come, until the ledger can be read: what it is, for the person.
struct Lost {
    op: String,
    group: String,
    kind: Wrote,
    /// The ledger was read and does not have it: that says nothing of what was sent (verify3 Major 1).
    unfound: bool,
}

/// Writes of this window kept to be asked for again, the newest.
const SENT_MAX: usize = 16;

pub struct State {
    /// The scope everything here was asked under; `None` while VRChat is not signed in for certain (nor is on its way back as
    /// an account whose groups were kept: [`State::held`]).
    scope: Option<Scope>,
    /// The saved session is being restored: what was last read shows, nothing is asked.
    held: bool,
    pub(crate) jobs: Jobs,
    pub(crate) pictures: Pictures,
    pub(crate) discovery: Discovery,
    /// When a card that could not be read may be asked for again (J8).
    hydrate_again: HashMap<String, f64>,
    /// A group's page, open in Home's place.
    pub(crate) open: Option<page::Open>,
    /// The banner studio, over the page or straight from a card.
    pub(crate) studio: Option<studio::Studio>,
    pub(crate) flow: Option<Flow>,
    /// The last write asked to be sealed.
    token: u64,
    serial: u64,
    /// The screenshot review staged what shows: nothing is asked of the service.
    staged: bool,
    /// Where the last press that prepares a write was (the confirmation grows out of it).
    pub(crate) origin: Rect,
    /// The service's ledger, as last read; asked for when due, from `ledger_at` on.
    pub(crate) ledger: Vec<Entry>,
    ledger_due: bool,
    ledger_at: f64,
    lost: Vec<Lost>,
    /// What the last check of each operation saw, for the person (memory only).
    pub(crate) observed: HashMap<String, Observation>,
    /// What this window sent, to be asked for again as a new operation (memory only).
    sent: VecDeque<(String, Write)>,
    /// What can be put back from here of what moved beside a write (its operation), as the service offers it.
    pub(crate) back: Vec<(String, Write)>,
    /// Records of earlier writes that cannot be read; another program keeps the record.
    unreadable: usize,
    elsewhere: bool,
    /// The signed-in account's name, for what may have been done as whom.
    me_name: String,
    /// Sealed writes to tell the service are over.
    cancels: Vec<String>,
    /// The new posts being written (on disk, Major 8), and one that could not be kept.
    pub(crate) drafts: Drafts,
    /// What the last refused write said, for the page that shows it.
    pub(crate) said: Option<String>,
    /// The section comes into view at the next frame (N4N0's assistant opened "my groups").
    pub(crate) reveal: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            scope: None,
            held: false,
            jobs: Jobs::default(),
            pictures: Pictures::default(),
            discovery: Discovery::default(),
            hydrate_again: HashMap::new(),
            open: None,
            studio: None,
            flow: None,
            token: 0,
            serial: 0,
            staged: false,
            origin: Rect::NOTHING,
            ledger: Vec::new(),
            ledger_due: true,
            ledger_at: 0.0,
            lost: Vec::new(),
            observed: HashMap::new(),
            sent: VecDeque::new(),
            back: Vec::new(),
            unreadable: 0,
            elsewhere: false,
            me_name: String::new(),
            cancels: Vec::new(),
            drafts: Drafts::default(),
            said: None,
            reveal: false,
        }
    }
}

/// A picture the account uploaded for a group and has not applied there (the ledger's word).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Uploaded {
    pub slot: ArtSlot,
    pub file: String,
    /// The studio's art it was made from.
    pub art: Option<u64>,
}

impl Uploaded {
    /// Where VRChat serves it (an upload is its file's first version).
    pub fn url(&self) -> String {
        format!("https://api.vrchat.cloud/api/1/file/{}/1/file", self.file)
    }
}

/// Whose what is kept on this computer for `scope` is: a folder's name; none while the Orbiters member is not known for
/// certain (verify4 F8).
pub(crate) fn owner_of(scope: &Scope) -> Option<String> {
    crate::controller::groups::Key::certain(scope).map(|k| k.folder())
}

impl State {
    /// A picture of a group or a member: from memory, or asked for (never while the screenshot review stages).
    pub(crate) fn picture(&mut self, ctx: &egui::Context, ctl: &Ctl, scope: &Scope, url: &str) -> Option<egui::TextureHandle> {
        if self.staged {
            return match self.pictures.map.get(url) {
                Some(Pic::Ready(t)) => Some(t.clone()),
                _ => None,
            };
        }
        self.pictures.get(ctx, &mut self.jobs, ctl, scope, url)
    }
    /// A group's page or the studio is open in Home's place.
    pub fn page_open(&self) -> bool {
        self.open.is_some() || self.studio.is_some()
    }
    /// A confirmation, the studio, or an editor of a page owns the keyboard: the chat stands aside (J5).
    pub fn holds(&self) -> bool {
        matches!(self.flow, Some(Flow::Confirming { .. })) || self.studio.is_some() || self.open.as_ref().is_some_and(|o| o.picker.is_some() || o.composer.is_some() || o.role_editor.is_some())
    }
    /// The view a write was asked from goes (its page, its studio, Home, the account): one not sent is over and can not
    /// be taken up by another view; one sent is the ledger's (B-F).
    pub fn end_flow(&mut self) {
        match self.flow.take() {
            Some(Flow::Confirming { sealed, .. }) => self.cancels.push(sealed.op),
            // Its answer, when it comes, finds no one waiting and is told over.
            Some(Flow::Preparing { .. }) => {}
            Some(Flow::Sending { .. }) => self.ledger_due = true,
            None => {}
        }
        self.token += 1;
    }
    /// Pictures of the page: how many cards show, and the page of one opened as a press on its card opens it.
    pub fn kept_cards(&self) -> Option<usize> {
        match &self.discovery {
            Discovery::Ready(cards, _) => Some(cards.len()),
            _ => None,
        }
    }
    pub fn open_kept(&mut self, i: usize) {
        let (Discovery::Ready(cards, _), Some(scope)) = (&self.discovery, self.scope.clone()) else { return };
        let Some(card) = cards.get(i).cloned() else { return };
        let mut open = page::Open::new(card, &self.drafts);
        open.as_kept(&scope);
        self.open = Some(open);
    }
    /// Back to Home (the sidebar's Home). The new post being written is kept on disk first.
    pub fn close(&mut self) {
        self.end_flow();
        self.keep_drafts();
        self.open = None;
        self.studio = None;
        self.jobs.reset();
        self.pictures.forget_unread();
        self.ledger_due = true;
    }
    /// Out of the studio, to the page or Home under it.
    pub(crate) fn close_studio(&mut self) {
        self.end_flow();
        self.studio = None;
    }
    /// Home's Groups section, in view at the next frame: N4N0's assistant's "open my groups" (it opens, nothing more).
    pub fn reveal(&mut self) {
        self.reveal = true;
    }
    fn signed(cx: &Cx) -> Option<Scope> {
        (cx.snap.vrchat_login.presence() == Presence::SignedIn).then(|| Scope::of(cx.snap))
    }
    /// Another account, the environment, or not signed in for certain: everything goes (B3); the new account's own draft
    /// posts come back from disk, its ledger from the service.
    fn sync(&mut self, cx: &Cx) {
        if self.staged {
            return;
        }
        // Signed in; or on its way back, as the service verified it for this very member: shown as it was, nothing asked.
        let signed = Self::signed(cx);
        let held = signed.is_none() && cx.snap.vrchat_login.presence() == Presence::Connecting && cx.snap.vrchat_login.known.is_some();
        let now = signed.or_else(|| held.then(|| Scope::of(cx.snap)).filter(|s| s.account().is_some()));
        // The very account that was on its way back is there: what shows stays, and is read now.
        let back = self.held && !held && now.as_ref().zip(self.scope.as_ref()).is_some_and(|(a, b)| a.account().is_some() && a.account() == b.account());
        if back || (held && self.held && now.as_ref().zip(self.scope.as_ref()).is_some_and(|(a, b)| a.account() == b.account())) {
            if back {
                self.held = false;
                self.jobs.reset();
                self.pictures.forget_unread();
                self.ledger_due = true;
                self.ledger_at = 0.0;
                if let Some(open) = self.open.as_mut() {
                    open.reload();
                }
                if let Discovery::Ready(_, at) = &mut self.discovery {
                    *at = f64::NEG_INFINITY;
                }
            }
            self.scope = now.clone();
            if back {
                self.drafts = self.scope.as_ref().map(Drafts::load).unwrap_or_default();
            }
        }
        self.me_name = cx.snap.vrchat_login.user.as_ref().map(|u| orbiters_core::groups::label(&u.display_name, 32)).unwrap_or_default();
        if now != self.scope {
            self.end_flow();
            self.keep_drafts();
            self.scope = now;
            self.held = held && self.scope.is_some();
            self.jobs.reset();
            self.pictures.clear();
            // The cards as they were last read; read again at once (REQUEST28).
            let cards = self.scope.as_ref().and_then(|s| kept::<Vec<GroupCard>>(s, orbiters_core::vrchat_store::kind::GROUPS, orbiters_core::vrchat_store::ONLY));
            self.discovery = cards.filter(|c| !c.is_empty()).map_or(Discovery::Idle, |c| Discovery::Ready(c, f64::NEG_INFINITY));
            self.hydrate_again.clear();
            self.open = None;
            self.studio = None;
            self.ledger.clear();
            self.lost.clear();
            self.observed.clear();
            self.sent.clear();
            self.back.clear();
            (self.unreadable, self.elsewhere) = (0, false);
            self.ledger_due = true;
            self.ledger_at = 0.0;
            // Told over by its scope: the service ended them itself.
            self.cancels.clear();
            self.drafts = self.scope.as_ref().map(Drafts::load).unwrap_or_default();
        }
    }
    /// Keeps the new post being written (the person's own words, nothing of an existing post: B-C).
    pub(crate) fn keep_drafts(&mut self) {
        if self.staged {
            return;
        }
        let (Some(open), Some(scope)) = (self.open.as_mut(), self.scope.as_ref()) else { return };
        for id in std::mem::take(&mut open.drafts_over) {
            self.drafts.drop(scope, &id);
        }
        if let Some(c) = open.composer.as_mut() {
            // Another window went on from the same one: this is another new post from here on (verify4 F6).
            if let Some(id) = self.drafts.keep(scope, &open.id, &c.id, &c.draft) {
                c.id = id;
            }
        }
    }
    /// The writes of `group` that may have been done and are not settled.
    pub(crate) fn open_writes(&self, group: &str) -> Vec<Entry> {
        self.ledger.iter().filter(|e| e.group == group && e.open()).cloned().collect()
    }
    /// The pictures uploaded for `group` and not applied there, the newest of each kind.
    pub(crate) fn uploads(&self, group: &str) -> Vec<Uploaded> {
        let mut out: Vec<Uploaded> = Vec::new();
        for e in self.ledger.iter().filter(|e| e.group == group && e.stands()) {
            let slot = match e.kind {
                Wrote::UploadGallery => ArtSlot::Banner,
                Wrote::UploadIcon => ArtSlot::Icon,
                _ => continue,
            };
            let Some(file) = e.made.clone() else { continue };
            let applied = self.ledger.iter().any(|x| x.group == group && x.stands() && x.kind.slot() == Some(slot) && x.target.file.as_deref() == Some(file.as_str()) && x.ended_at >= e.ended_at);
            if !applied && !out.iter().any(|u| u.slot == slot) {
                out.push(Uploaded { slot, file, art: e.evidence.art });
            }
        }
        out
    }

    /// Takes what came back.
    fn take(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        if let (Some(scope), false) = (self.scope.clone(), self.staged || self.held) {
            for op in std::mem::take(&mut self.cancels) {
                self.jobs.ask(ctx, cx.ctl, &scope, Kind::Cancel(op.clone()), Request::Cancel { op });
            }
            if self.ledger_due && anim::time(ctx) >= self.ledger_at && !self.jobs.running(&Kind::Ledger) {
                self.ledger_due = false;
                let also = self.lost.iter().map(|l| l.op.clone()).collect();
                self.jobs.ask(ctx, cx.ctl, &scope, Kind::Ledger, Request::Ledger { also });
            }
        }
        for a in self.jobs.take() {
            match (a.kind, a.reply) {
                (Kind::Discover, Reply::Discovered(mut cards)) => {
                    // A card not read yet keeps the name and pictures it shows.
                    if let Discovery::Ready(before, _) = &self.discovery {
                        for c in cards.iter_mut().filter(|c| !c.named) {
                            if let Some(b) = before.iter().find(|b| b.id == c.id && b.named) {
                                *c = GroupCard { owner: c.owner, permissions: c.permissions.clone(), ..b.clone() };
                            }
                        }
                    }
                    self.discovery = Discovery::Ready(cards, anim::time(ctx));
                }
                // What shows stays when it could not be read again: the next try as after any.
                (Kind::Discover, Reply::Failed(f)) => match &mut self.discovery {
                    Discovery::Ready(cards, at) if !cards.is_empty() => *at = anim::time(ctx),
                    _ => self.discovery = Discovery::Failed(f, anim::time(ctx)),
                },
                (Kind::Hydrate, Reply::Hydrated(named)) => {
                    let again = anim::time(ctx) + HYDRATE_AGAIN;
                    if let Discovery::Ready(cards, _) = &mut self.discovery {
                        for (id, n) in named {
                            let Ok(n) = n else {
                                self.hydrate_again.insert(id, again);
                                continue;
                            };
                            if let Some(c) = cards.iter_mut().find(|c| c.id == n.id) {
                                let (owner, permissions) = (c.owner || n.owner, if c.permissions.is_empty() { n.permissions.clone() } else { c.permissions.clone() });
                                *c = GroupCard { owner, permissions, ..n };
                            }
                        }
                    }
                }
                (Kind::Picture(url), _) => self.pictures.arrived(ctx, url, a.image),
                (Kind::Source(url), Reply::Cancelled) => {
                    if let Some(studio) = self.studio.as_mut() {
                        studio.source_arrived(&url, a.source);
                    }
                }
                (Kind::Hydrate, _) => {
                    let again = anim::time(ctx) + HYDRATE_AGAIN;
                    if let Discovery::Ready(cards, _) = &self.discovery {
                        self.hydrate_again.extend(cards.iter().filter(|c| !c.named).map(|c| (c.id.clone(), again)));
                    }
                }
                (Kind::Ledger, Reply::Ledger { entries, unreadable, back }) => {
                    (self.unreadable, self.elsewhere, self.back) = (unreadable, false, back);
                    self.read_ledger(ctx, cx, entries);
                }
                (Kind::Ledger, reply) => {
                    self.elsewhere = reply == Reply::Failed(Failure::OtherOwner);
                    self.ledger_again(ctx);
                }
                (Kind::Check(op), reply) => self.checked(ctx, cx, op, reply),
                (Kind::Decide(op, choice), reply) => self.decided(ctx, cx, op, choice, reply),
                (Kind::SetAside, reply) => {
                    if let Reply::Failed(f) = reply {
                        self.said = Some(failure_text(f));
                    }
                    self.ledger_due = true;
                }
                (Kind::SeenTo(_), reply) => {
                    if let Reply::Failed(f) = reply {
                        self.said = Some(failure_text(f));
                    }
                    self.ledger_due = true;
                }
                (Kind::Cancel(_), _) => {}
                (Kind::Prepare(token), reply) => self.prepared(ctx, cx, token, reply),
                (Kind::Art { op, .. }, _) => {
                    if let Some(Flow::Confirming { sealed, art, .. }) = self.flow.as_mut().filter(|_| a.image.is_some()) {
                        if sealed.op == op {
                            *art = a.image.map(|i| ctx.load_texture("group-sealed-picture", i, egui::TextureOptions::LINEAR));
                        }
                    }
                }
                (Kind::Commit(op), reply) => self.committed(ctx, cx, op, reply),
                (kind, reply) => {
                    if let Some(open) = self.open.as_mut() {
                        open.answer(kind.clone(), reply.clone());
                    }
                    if let Some(studio) = self.studio.as_mut() {
                        studio.answer(kind, reply);
                    }
                }
            }
        }
    }

    /// Asks the service to seal a write; its confirmation (when it needs one) grows out of `origin`.
    pub(crate) fn prepare(&mut self, ctx: &egui::Context, ctl: &Ctl, write: Write, origin: Rect) {
        let Some(scope) = self.scope.clone() else { return };
        if self.flow.is_some() {
            return;
        }
        if self.held {
            self.said = Some(failure_text(Failure::SignedOut));
            return;
        }
        self.said = None;
        self.origin = origin;
        self.token += 1;
        self.flow = Some(Flow::Preparing { token: self.token });
        self.jobs.ask(ctx, ctl, &scope, Kind::Prepare(self.token), Request::Prepare { write });
    }
    fn prepared(&mut self, ctx: &egui::Context, cx: &mut Cx, token: u64, reply: Reply) {
        let waiting = matches!(self.flow, Some(Flow::Preparing { token: t }) if t == token);
        match reply {
            // No one waits for it any more: its view went.
            Reply::Prepared(sealed) if !waiting => self.cancels.push(sealed.op),
            _ if !waiting => {}
            Reply::Prepared(sealed) if sealed.confirm => {
                if let (Some(art), Some(scope)) = (&sealed.art, self.scope.clone()) {
                    let kind = Kind::Art { op: sealed.op.clone(), sha256: art.sha256.clone() };
                    self.jobs.ask(ctx, cx.ctl, &scope, kind, Request::SealedArt { op: sealed.op.clone() });
                }
                self.serial += 1;
                self.flow = Some(Flow::Confirming { sealed: *sealed, serial: self.serial, origin: self.origin, art: None });
            }
            Reply::Prepared(sealed) => self.commit(ctx, cx.ctl, *sealed),
            Reply::MaySent { .. } => {
                self.flow = None;
                self.ledger_due = true;
                self.said = Some(failure_text(Failure::MaySent));
            }
            Reply::Failed(f) => {
                self.flow = None;
                self.ended(ctx, cx, None, Outcome::Refused(f));
            }
            _ => self.flow = None,
        }
    }
    /// Sends a sealed write, as shown: its id and the digest of what it holds now.
    fn commit(&mut self, ctx: &egui::Context, ctl: &Ctl, sealed: Sealed) {
        let Some(scope) = self.scope.clone() else { return };
        let (op, digest) = (sealed.op.clone(), sealed.digest());
        self.sent.push_back((op.clone(), sealed.write.clone()));
        while self.sent.len() > SENT_MAX {
            self.sent.pop_front();
        }
        self.flow = Some(Flow::Sending { sealed });
        self.jobs.ask(ctx, ctl, &scope, Kind::Commit(op.clone()), Request::Commit { op, digest });
    }
    fn committed(&mut self, ctx: &egui::Context, cx: &mut Cx, op: String, reply: Reply) {
        self.ledger_due = true;
        let mine = matches!(&self.flow, Some(Flow::Sending { sealed }) if sealed.op == op);
        let sealed = if mine {
            match self.flow.take() {
                Some(Flow::Sending { sealed }) => Some(sealed),
                _ => None,
            }
        } else {
            None
        };
        let outcome = match reply {
            Reply::Outcome { outcome, .. } => outcome,
            // Its answer did not come: the ledger has it. Until that can be read, what it was.
            Reply::Failed(Failure::Lost) => {
                if let Some(s) = &sealed {
                    self.lost.push(Lost { op, group: s.group.clone(), kind: s.write.kind(), unfound: false });
                }
                Outcome::Unknown
            }
            // Not asked, or refused before it was looked at.
            Reply::Failed(f) => Outcome::Refused(f),
            _ => Outcome::Unknown,
        };
        // Its view went meanwhile: what came of it is told, and no other view takes anything from it (B-F).
        if !mine {
            match outcome {
                Outcome::Done(done) => cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, done_text(&done)),
                Outcome::Refused(f) => cx.toasts.push(ctx, orbiters_ui::toast::Kind::Info, failure_text(f)),
                Outcome::Unknown => {}
            }
            return;
        }
        self.ended(ctx, cx, sealed, outcome);
    }
    fn cancel(&mut self) {
        self.end_flow();
    }
    fn ledger_again(&mut self, ctx: &egui::Context) {
        self.ledger_due = true;
        self.ledger_at = anim::time(ctx) + ASK_AGAIN;
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(ASK_AGAIN));
    }
    /// The ledger as the service has it: the state of every write, whoever asked for it.
    fn read_ledger(&mut self, ctx: &egui::Context, cx: &mut Cx, list: Vec<Entry>) {
        for mut l in std::mem::take(&mut self.lost) {
            match list.iter().find(|e| e.op == l.op).map(|e| (e.state, e.open())) {
                // Not there, or not readable: it says nothing of what was sent, so it stays said (verify3 Major 1).
                None => {
                    l.unfound = true;
                    self.lost.push(l);
                    continue;
                }
                // Not gone out yet: looked at again in a moment.
                Some((Stands::Prepared, _)) => {
                    self.lost.push(l);
                    self.ledger_again(ctx);
                    continue;
                }
                Some((Stands::Done, _)) => cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, done_words(l.kind)),
                Some((Stands::Refused, _)) => cx.toasts.push(ctx, orbiters_ui::toast::Kind::Info, tr("VRChat refused it, or it stopped before it went.")),
                Some(_) => {}
            }
            if let Some(open) = self.open.as_mut().filter(|o| o.id == l.group) {
                open.reload();
            }
        }
        // The very post being written that the ledger says is out: not offered again as unsent. Another one with the same
        // words, one changed since, or one with another notification stays (verify3 Major 6).
        if let Some(open) = self.open.as_mut() {
            let out = |c: &page::Composer| list.iter().any(|e| e.kind == Wrote::CreatePost && e.group == open.id && e.stands() && e.evidence.draft.as_ref() == Some(&c.id) && e.evidence.revision.as_deref() == Some(c.draft.revision().as_str()));
            if open.composer.as_ref().is_some_and(out) {
                open.end_composer();
            }
        }
        self.observed.retain(|op, _| list.iter().any(|e| e.op == *op && e.open()));
        self.ledger = list;
    }
    /// The person asks what VRChat shows of the write `op`.
    pub(crate) fn check(&mut self, ctx: &egui::Context, ctl: &Ctl, op: &str) {
        let Some(scope) = self.scope.clone() else { return };
        self.jobs.ask(ctx, ctl, &scope, Kind::Check(op.to_string()), Request::Check { op: op.to_string() });
    }
    /// The person decides what becomes of it.
    pub(crate) fn decide(&mut self, ctx: &egui::Context, ctl: &Ctl, op: &str, choice: Choice) {
        let Some(scope) = self.scope.clone() else { return };
        self.jobs.ask(ctx, ctl, &scope, Kind::Decide(op.to_string(), choice.clone()), Request::Decide { op: op.to_string(), choice });
    }
    fn checked(&mut self, ctx: &egui::Context, cx: &mut Cx, op: String, reply: Reply) {
        let observation = match reply {
            Reply::Checked { observation, .. } => observation,
            // Not asked of VRChat at all: the person may still look themselves, and decide.
            Reply::Failed(f) => Observation::CannotTell(Unclear::Failed(f)),
            _ => return,
        };
        self.ledger_due = true;
        match observation {
            Observation::Shows => cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, tr("VRChat now shows what you asked for.")),
            // What was said then: VRChat was not read for it now (verify4 F4).
            Observation::Answered { done: true } => cx.toasts.push(ctx, orbiters_ui::toast::Kind::Info, tr("Settled before: VRChat answered then that it was done. What it shows now was not read.")),
            Observation::Answered { done: false } => cx.toasts.push(ctx, orbiters_ui::toast::Kind::Info, tr("VRChat refused it, or it stopped before it went.")),
            Observation::Closed => cx.toasts.push(ctx, orbiters_ui::toast::Kind::Info, tr("Settled before. What VRChat shows now was not read.")),
            open => {
                self.observed.insert(op, open);
                return;
            }
        }
        self.observed.remove(&op);
        self.lost.retain(|l| l.op != op);
        if let Some(open) = self.open.as_mut() {
            open.reload();
        }
    }
    /// What the person decided is kept, or not. "Again" asks for the same as a new write, confirmed as any.
    fn decided(&mut self, ctx: &egui::Context, cx: &mut Cx, op: String, choice: Choice, reply: Reply) {
        self.ledger_due = true;
        if let Reply::Failed(f) = reply {
            self.said = Some(look_failure_text(f));
            return;
        }
        if choice == Choice::ShowsNot {
            self.observed.insert(op, Observation::ShowsNot);
            return;
        }
        self.observed.remove(&op);
        self.lost.retain(|l| l.op != op);
        let entry = self.ledger.iter().find(|e| e.op == op).cloned();
        if let Some(e) = self.ledger.iter_mut().find(|e| e.op == op) {
            e.closed = Some(match choice {
                Choice::Shows { .. } => crate::controller::groups::Closing::Shows,
                Choice::Again => crate::controller::groups::Closing::Again,
                _ => crate::controller::groups::Closing::Dropped,
            });
        }
        if let Some(open) = self.open.as_mut() {
            open.reload();
        }
        if choice == Choice::Again {
            match entry.as_ref().and_then(|e| self.again(e)).filter(|_| self.flow.is_none()) {
                Some(write) => self.prepare(ctx, cx.ctl, write, self.origin),
                None => self.said = Some(tr("Do it again where you did it: it is confirmed as a new one.").into()),
            }
        }
    }
    /// The same write as a new one: as this window sent it, or from its ids alone.
    fn again(&self, e: &Entry) -> Option<Write> {
        let sent = self.sent.iter().rev().find(|(op, _)| *op == e.op).map(|(_, w)| w.clone());
        // An upload's bytes went with it.
        if let Some(w) = sent.filter(|w| !matches!(w, Write::Upload { .. })) {
            return Some(w);
        }
        let group = e.group.clone();
        match (e.kind, &e.target) {
            // Of the very membership it was for, where the page still has it.
            (Wrote::AssignRole | Wrote::UnassignRole, t) => {
                let membership = self.open.as_ref().filter(|o| o.id == group)?.membership_of(t.user.as_ref()?).filter(|m| Some(m.digest()) == t.membership)?;
                Some(Write::MemberRole { group, user: t.user.clone()?, membership, role: t.role.clone()?, add: e.kind == Wrote::AssignRole })
            }
            (Wrote::DeleteRole, t) => Some(Write::DeleteRole { group, role: t.role.clone()? }),
            (Wrote::ApplyBanner | Wrote::ApplyIcon, t) => Some(Write::ApplyArt { group, slot: e.kind.slot()?, file: t.file.clone()?, back: None }),
            (Wrote::DeletePost, t) => {
                let post = self.open.as_ref().filter(|o| o.id == group)?.posts.items.iter().find(|p| Some(&p.id) == t.post.as_ref())?.clone();
                Some(Write::DeletePost { group, post })
            }
            _ => None,
        }
    }
    /// How a write ended, for the view it was of (a view of another group takes nothing from it). Toasts carry no names
    /// (M10).
    fn ended(&mut self, ctx: &egui::Context, cx: &mut Cx, sealed: Option<Sealed>, outcome: Outcome) {
        if let Outcome::Done(done) = &outcome {
            cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, done_text(done));
        }
        self.said = match &outcome {
            Outcome::Refused(f) => Some(failure_text_for(sealed.as_ref().map(|p| &p.write), *f)),
            Outcome::Done(_) | Outcome::Unknown => None,
        };
        // Without one: refused as it was being prepared, for the view still waiting for it.
        let group = sealed.as_ref().map(|s| s.group.clone());
        if let Some(studio) = self.studio.as_mut().filter(|s| group.as_ref() == Some(&s.card.id)) {
            studio.ended(sealed.as_ref(), &outcome);
        }
        if let Some(open) = self.open.as_mut().filter(|o| group.is_none() || group.as_ref() == Some(&o.id)) {
            open.ended(sealed.as_ref(), outcome);
        }
        self.ledger_due = true;
        self.keep_drafts();
    }
}

/// What a write did, by its kind.
fn done_words(kind: Wrote) -> &'static str {
    match kind {
        Wrote::AssignRole | Wrote::UnassignRole => tr("The member's roles are updated."),
        Wrote::CreateRole => tr("The role was created."),
        Wrote::EditRole | Wrote::DeleteRole => tr("The roles are updated."),
        Wrote::CreatePost => tr("The post is on VRChat."),
        Wrote::DeletePost => tr("The post was deleted."),
        Wrote::UploadGallery | Wrote::UploadIcon => tr("The picture is in your VRChat inventory."),
        Wrote::ApplyBanner | Wrote::ApplyIcon => tr("The group's picture is changed."),
    }
}

/// What a done write says in its toast: no member, role or post text.
fn done_text(done: &crate::controller::groups::Done) -> String {
    use crate::controller::groups::Done;
    match done {
        Done::MemberRoles { .. } => tr("The member's roles are updated."),
        Done::Role(_) => tr("The role was created."),
        Done::Roles(_) => tr("The roles are updated."),
        Done::Post(_) => tr("The post is on VRChat."),
        Done::PostDeleted { .. } => tr("The post was deleted."),
        Done::Uploaded(_) => tr("The picture is in your VRChat inventory."),
        Done::Group(_) => tr("The group's picture is changed."),
    }
    .to_string()
}

/// What moved beside a write, and what nothing says of, in the person's words: what it was, whose it may be, and what can not
/// be put back from here.
fn beside_text(e: &Entry, back: &[&Write]) -> String {
    let names = |parts: &[Part]| parts.iter().map(|p| part_name(*p)).collect::<Vec<_>>().join(", ");
    let mut out = vec![done_words(e.kind).to_string()];
    if !e.beside.moved.is_empty() {
        out.push(orbiters_i18n::tr!("VRChat shows more changed than was asked: {what}. It can be someone else's change meanwhile, or VRChat's.", what = names(&e.beside.moved)));
        let there: Vec<Part> = e.beside.moved.iter().copied().filter(|p| !back.iter().any(|w| w.parts().contains(p))).collect();
        if !there.is_empty() {
            out.push(orbiters_i18n::tr!("Not put back from here: {what}. Do it on vrchat.com.", what = names(&there)));
        }
    }
    if !e.beside.unread.is_empty() {
        out.push(orbiters_i18n::tr!("VRChat does not say whether this stayed as it was: {what}. Look on vrchat.com.", what = names(&e.beside.unread)));
    }
    out.join("\n")
}

fn part_name(p: Part) -> &'static str {
    match p {
        Part::Banner => tr("Group banner"),
        Part::Icon => tr("Group icon"),
        Part::Nameplate => tr("Nameplate banner"),
        Part::NameplateMode => tr("How the nameplate banner shows"),
        Part::Name => tr("Name"),
        Part::ShortCode => tr("Short code"),
        Part::Description => tr("Description"),
        Part::Rules => tr("Rules"),
        Part::Links => tr("Links"),
        Part::Languages => tr("Languages"),
        Part::JoinState => tr("Who can join"),
        Part::Privacy => tr("Who sees the group"),
        Part::JoinPrompt => tr("The prompt to join"),
        Part::Rights => tr("Its rights"),
        Part::SelfAssignable => tr("Members can take it themselves"),
        Part::AddedOnJoin => tr("Given to everyone who joins"),
        Part::TwoFactor => tr("Needs two-factor authentication"),
        Part::Standing => tr("Its place among the roles"),
    }
}

fn named<'a>(parts: &[(bool, &'a str)]) -> Vec<&'a str> {
    parts.iter().filter(|(on, _)| *on).map(|(_, name)| *name).collect()
}

fn role_parts(p: &orbiters_core::groups::RoleParts) -> [(bool, &'static str); 7] {
    [
        (p.name, tr("Name")),
        (p.description, tr("Description")),
        (p.permissions, tr("Its rights")),
        (p.self_assignable, tr("Members can take it themselves")),
        (p.added_on_join, tr("Given to everyone who joins")),
        (p.two_factor, tr("Needs two-factor authentication")),
        (p.standing, tr("Its place among the roles")),
    ]
}

/// A refusal in the person's words for the write it refused: a 403 to an upload is no group role's (Major 4).
pub(crate) fn failure_text_for(write: Option<&Write>, f: Failure) -> String {
    match (write, f) {
        (Some(Write::Upload { .. }), Failure::Api(GroupFailure::Forbidden)) => tr("VRChat refused the upload: VRChat+ is needed, or your inventory is full.").to_string(),
        (Some(Write::ApplyArt { .. }), Failure::Api(GroupFailure::Forbidden)) => tr("VRChat refused the picture: your role, or the picture itself.").to_string(),
        _ => failure_text(f),
    }
}

/// What someone else changed meanwhile, by the names of its parts; and what is left open, said (design 6).
fn changed_text(parts: &[(bool, &str)]) -> String {
    let what = named(parts);
    orbiters_i18n::tr!(
        "Someone changed it on VRChat since you opened it ({what}), so nothing was sent. It is read again just before each send; VRChat offers no way to close the instant between.",
        what = what.join(", ")
    )
}

/// A failure in the person's words (local codes only: nothing of VRChat's text, M5, M10).
pub(crate) fn failure_text(f: Failure) -> String {
    match f {
        Failure::Api(GroupFailure::Forbidden) => tr("Your role in this group does not allow this."),
        Failure::Api(GroupFailure::NotFound) => tr("VRChat does not show this to your account any more."),
        Failure::Api(GroupFailure::Invalid) => tr("VRChat did not take it as it is. Check what you entered."),
        Failure::Api(GroupFailure::RateLimited(s)) => return orbiters_i18n::tr!("VRChat asks to wait. Try again in {seconds} s.", seconds = s),
        Failure::Api(GroupFailure::Unavailable) => tr("VRChat could not be reached. Try again shortly."),
        Failure::Api(GroupFailure::Unknown) | Failure::Lost => tr("VRChat may have done it: its answer was lost. Check before trying again."),
        Failure::Api(GroupFailure::BadAnswer) => tr("VRChat's answer could not be read. Try again shortly."),
        Failure::Api(GroupFailure::SignedOut) | Failure::SignedOut => tr("Sign in to VRChat first."),
        Failure::Api(GroupFailure::Disabled) | Failure::AccountChanged => tr("The account changed: nothing was sent."),
        Failure::Busy => tr("Already busy with this. Try again in a moment."),
        Failure::Withheld(why) => why_text(why),
        Failure::NotRead(m) => missing_text(m),
        Failure::Expired => tr("This waited too long. Do it again to send it."),
        Failure::Duplicate => tr("This was already sent once."),
        Failure::Changed => tr("It changed on VRChat since you opened it. Look again before trying."),
        Failure::PostChanged(p) => {
            return changed_text(&[(p.title, tr("Title")), (p.text, tr("Text")), (p.visibility, tr("Who sees it")), (p.roles, tr("Restricted to")), (p.image, tr("Picture"))]);
        }
        Failure::RoleChanged(p) => {
            return changed_text(&role_parts(&p));
        }
        Failure::Rejoined => tr("This person left and rejoined: look again. Nothing was sent."),
        Failure::MaySent => tr("This may already have been sent: check first."),
        Failure::Since => tr("Another like it went out since this was shown, so this one was not sent. Look at it again to send it."),
        Failure::RecordUnreadable => tr("A record of an earlier change cannot be read, so nothing was sent: it may be this one."),
        Failure::OtherOwner => tr("Another Orbiters on this computer keeps the record of group changes: make them there."),
        Failure::NotKept => tr("Its record could not be kept on this computer, so nothing was sent. Check the disk's free space."),
        Failure::LookNotKept => tr("This could not be kept on this computer, so it stays as it was. Check the disk's free space."),
        Failure::Offline => tr("The Orbiters service could not be reached: nothing was sent."),
        Failure::NotHere => tr("Only the Orbiters window can do this."),
        Failure::TooLarge => tr("The picture is too large: 10 MB at most."),
    }
    .to_string()
}

/// The same of a check, a decision, or the list of them: they send nothing, and what they are of may have been done all the
/// same: "nothing was sent" is never said of it (verify4 F4).
fn look_failure_text(f: Failure) -> String {
    match f {
        Failure::AccountChanged | Failure::Api(GroupFailure::Disabled) => tr("The account changed meanwhile."),
        Failure::Offline => tr("The Orbiters service could not be reached."),
        Failure::Lost | Failure::Api(GroupFailure::Unknown) => tr("The Orbiters service did not answer."),
        Failure::NotKept => tr("This could not be kept on this computer, so it stays as it was. Check the disk's free space."),
        Failure::NotRead(_) => tr("VRChat left something out of its answer."),
        Failure::Expired | Failure::MaySent | Failure::RecordUnreadable => tr("Its record on this computer is gone, or cannot be read."),
        f => return failure_text(f),
    }
    .to_string()
}

pub(crate) fn why_text(why: Why) -> &'static str {
    match why {
        Why::NoPermission => tr("Your role does not include this right."),
        Why::RoleAtOrAbove => tr("This role is at or above your own highest role."),
        Why::FixedRole => tr("Everyone's role and the owner's role cannot be changed this way."),
        Why::PaidRole => tr("Roles bought in VRChat's store are not given here."),
        Why::OwnerRole => tr("Only the name and description of the owner's role change."),
        Why::NotGrantable => tr("Your role cannot give this right."),
        Why::Prerequisite => tr("A right is missing the right it depends on."),
        Why::NotSelfAssignable => tr("Members cannot take this role themselves."),
        Why::Escalation => tr("You can only give rights you hold yourself."),
        Why::NotEligible => tr("Uploading to your VRChat inventory needs VRChat+."),
        Why::NotYourPicture => tr("This picture is not one of your own VRChat pictures of that kind."),
    }
}

/// What VRChat's answer left out, in the person's words: nothing is guessed, the page can be read again.
pub(crate) fn missing_text(m: Missing) -> &'static str {
    match m {
        Missing::Membership => tr("VRChat did not give your own membership whole, so nothing is sent. Read the group again."),
        Missing::Identity => tr("What VRChat gave is not of your account in this group, so nothing is sent. Read the group again."),
        Missing::Roles => tr("VRChat did not give every role of this group, so nothing is sent. Read the group again."),
        Missing::RoleOrder => tr("VRChat did not say where this role stands among the others, so nothing is sent. Read the group again."),
        Missing::RoleDefinition => tr("VRChat did not give what this role may do, so nothing is sent. Read the group again."),
        Missing::Catalogue => tr("VRChat did not give the rights a role can have, so nothing is sent. Read the group again."),
        Missing::Member => tr("VRChat did not give this member's own roles whole, so nothing is sent. Read the group again."),
        Missing::Post => tr("VRChat did not give this post, or who sees it, whole: it is not deleted from here."),
        Missing::Picture => tr("VRChat did not say which picture is there, so nothing is sent. Read the group again."),
        Missing::Entitlement => tr("VRChat did not say whether your account may upload, so nothing is sent. Try again shortly."),
    }
}

/// Why a control is not offered.
pub(crate) fn authority_text(a: &orbiters_core::groups::Authority) -> Option<&'static str> {
    match a {
        orbiters_core::groups::Authority::Allowed(_) => None,
        orbiters_core::groups::Authority::Refused(why) => Some(why_text(*why)),
        orbiters_core::groups::Authority::Unknown(m) => Some(missing_text(*m)),
    }
}

/// What another entry of the ledger says of a write about to go.
pub(crate) fn warning_text(w: Warning) -> &'static str {
    match w {
        Warning::AnotherMayHaveGone => tr("Another change here may or may not have gone through. This one may overtake it, or it this one."),
        Warning::SameBefore => tr("The very same went out before and its answer was lost: this may make a second one."),
        Warning::AlreadyDone => tr("The very same was done less than a day ago. Sending it makes a second one."),
        Warning::RecordUnreadable => tr("A record of an earlier change could not be read: this may have been sent already."),
    }
}

/// The last six of an id, to tell one from another.
fn tail(id: &str) -> String {
    let from = id.char_indices().rev().nth(5).map_or(0, |(i, _)| i);
    format!("…{}", &id[from..])
}

/// Exactly which write it is: what it touched, by its name where it is on the page and its id, when it went, as whom (two
/// of one kind are told apart, verify3 Major 3).
fn which(state: &State, e: &Entry) -> String {
    let open = state.open.as_ref().filter(|o| o.id == e.group);
    let named = |name: Option<String>, id: &str| match name.filter(|n| !n.is_empty()) {
        Some(n) => format!("{n} ({})", tail(id)),
        None => tail(id),
    };
    let role = |id: &str| named(open.and_then(|o| o.role_name(id)), id);
    let mut parts: Vec<String> = Vec::new();
    if let Some(user) = &e.target.user {
        parts.push(named(open.and_then(|o| o.member_name(user)), user));
    }
    if let Some(id) = &e.target.role {
        parts.push(role(id));
    }
    if let Some(id) = &e.target.post {
        parts.push(named(open.and_then(|o| o.posts.items.iter().find(|p| p.id == *id)).map(|p| orbiters_core::groups::label(&p.title, 40)), id));
    }
    if let Some(id) = &e.target.file {
        parts.push(orbiters_i18n::tr!("file {id}", id = tail(id)));
    }
    // What has no id until VRChat answers: its own words where this window wrote them, and the operation's id (two new posts
    // lost in one minute are two: verify4 F10).
    if e.kind.makes() {
        let title = state.sent.iter().rev().find(|(op, _)| *op == e.op).and_then(|(_, w)| match w {
            Write::CreatePost { draft, .. } => Some(orbiters_core::groups::label(&draft.title, 40)),
            Write::CreateRole { draft, .. } => Some(orbiters_core::groups::label(&draft.name, 40)),
            _ => None,
        });
        parts.push(named(title, &e.op));
    }
    if let Some(at) = e.dispatched_at.and_then(|ms| time::OffsetDateTime::from_unix_timestamp(ms / 1000).ok()).and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()) {
        parts.push(super::local_time(&at));
    }
    if !state.me_name.is_empty() {
        parts.push(orbiters_i18n::tr!("as {name}", name = state.me_name));
    }
    parts.join(" · ")
}

/// The writes of `group` that may have been done, from the ledger: which each is, "Check VRChat", what the check saw, and,
/// once it was asked for, the person's own decision: every one has a way out. Shown on the page (every tab) and in the
/// studio.
pub(crate) fn uncertain_panel(ui: &mut Ui, ctx: &egui::Context, ctl: &Ctl, state: &mut State, group: &str) {
    let check_w = fits(ui, tr("Check VRChat"), 24.0).max(fits(ui, tr("Checking…"), 24.0)).max(136.0);
    if state.elsewhere {
        warn_card(ui, tr("Another Orbiters on this computer keeps the record of group changes: make them there."), 0.0, |_, _| {});
    }
    if state.unreadable > 0 {
        let w = fits(ui, tr("I looked: go on"), 0.0);
        let telling = state.jobs.running(&Kind::SetAside);
        let mut on = false;
        warn_card(ui, tr("Records of earlier group changes on this computer cannot be read: what they were of may have been done, or not. Look on vrchat.com first. They stay as they are if you go on."), w, |ui, rect| {
            let r = widgets::Button::new(tr("I looked: go on")).variant(widgets::Variant::Warn).size(widgets::Size::Sm).enabled(!telling).id_salt("group-set-aside").show_at(ui, rect);
            on = widgets::activated_fresh(&r);
        });
        if let Some(scope) = state.scope.clone().filter(|_| on) {
            state.jobs.ask(ctx, ctl, &scope, Kind::SetAside, Request::SetAside);
        }
    }
    let open = state.open_writes(group);
    let several = open.len() > 1;
    for e in open {
        // Which of several a press decides is in its words.
        let of = |label: &str| if several { format!("{label} ({})", tail(&e.op)) } else { label.to_string() };
        let busy = state.jobs.running(&Kind::Check(e.op.clone()));
        let telling = state.jobs.busy_with(|k| matches!(k, Kind::Decide(op, _) if *op == e.op));
        let seen = state.observed.get(&e.op).cloned();
        let text = format!("{}\n{}\n{}", uncertain_words(e.kind), which(state, &e), seen_words(&e, seen.as_ref()));
        warn_card(ui, &text, check_w, |ui, rect| {
            let label = if busy { tr("Checking…") } else { tr("Check VRChat") };
            if widgets::Button::new(label).icon(icons::ARROW_CLOCKWISE).variant(widgets::Variant::Warn).size(widgets::Size::Sm).enabled(!busy).id_salt(("group-check", e.op.as_str())).show_at(ui, rect).clicked() {
                state.check(ctx, ctl, &e.op);
            }
        });
        // The person's own word, after a check was asked for and whatever it said but "too soon" or "being sent".
        let by_ledger = e.checked.then_some(e.answer).flatten();
        let offered: Vec<(&str, Choice, &str)> = match &seen {
            Some(Observation::Running | Observation::CannotTell(Unclear::TooSoon)) => vec![],
            Some(Observation::ShowsNot) => vec![(tr("Do it again…"), Choice::Again, "again"), (tr("Drop it"), Choice::Drop, "drop")],
            None if by_ledger == Some(Looked::ShowsNot) => vec![(tr("Do it again…"), Choice::Again, "again"), (tr("Drop it"), Choice::Drop, "drop")],
            Some(Observation::Candidates(list)) if !list.is_empty() => vec![(tr("None of these"), Choice::ShowsNot, "none"), (tr("Drop it"), Choice::Drop, "drop")],
            Some(Observation::Candidates(_) | Observation::CannotTell(_)) => vec![(tr("I looked: it is there"), Choice::Shows { made: None }, "there"), (tr("I looked: it is not there"), Choice::ShowsNot, "not-there"), (tr("Drop it"), Choice::Drop, "drop")],
            _ => vec![],
        };
        let mut choice: Option<Choice> = None;
        if let Some(Observation::Candidates(list)) = &seen {
            for c in list {
                let thumb = state.scope.clone().and_then(|scope| state.picture(ctx, ctl, &scope, &c.url));
                candidate_row(ui, &e.op, &of(tr("This is it")), c, thumb.as_ref(), telling, &mut choice);
            }
        }
        if !offered.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                for (label, what, salt) in offered {
                    let label = of(label);
                    let r = widgets::Button::new(&label).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).enabled(!telling).id_salt(("group-decide", salt, e.op.as_str())).show(ui);
                    if widgets::activated_fresh(&r) {
                        state.origin = r.rect;
                        choice = Some(what);
                    }
                }
            });
        }
        if let Some(choice) = choice {
            state.decide(ctx, ctl, &e.op, choice);
        }
    }
    beside_panel(ui, ctx, ctl, state, group);
    // Sent, its answer lost, and the ledger not read yet, or read and without it.
    let waiting: Vec<(String, Wrote, bool)> = state.lost.iter().filter(|l| l.group == group && !state.ledger.iter().any(|e| e.op == l.op)).map(|l| (l.op.clone(), l.kind, l.unfound)).collect();
    for (op, kind, unfound) in waiting {
        if !unfound {
            warn_card(ui, &format!("{} {}", uncertain_words(kind), tr("Its answer was lost. Reading what is known of it…")), 0.0, |_, _| {});
            continue;
        }
        let w = fits(ui, tr("Drop it"), 0.0);
        let mut over = false;
        warn_card(ui, &format!("{} {}", uncertain_words(kind), tr("Its answer was lost, and its record here cannot be read: look on vrchat.com before doing it again.")), w, |ui, rect| {
            let r = widgets::Button::new(tr("Drop it")).variant(widgets::Variant::Warn).size(widgets::Size::Sm).id_salt(("group-lost", op.as_str())).show_at(ui, rect);
            over = widgets::activated_fresh(&r);
        });
        if over {
            state.lost.retain(|l| l.op != op);
        }
    }
    if let Some(said) = state.said.clone() {
        let w = ui.available_width();
        let p = ui.painter().clone();
        let galley = p.layout_job(Txt::new(&said, 13.0).color(theme::FG).line_height(19.0).job(w - 60.0));
        let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y.max(20.0) + 28.0), Sense::hover());
        p.rect_filled(rect, theme::radius(14.0), theme::alpha(theme::DANGER, 0.12));
        widgets::icon(&p, pos2(rect.left() + 22.0, rect.top() + 24.0), icons::WARNING_CIRCLE, 17.0, theme::DANGER);
        p.galley(pos2(rect.left() + 42.0, rect.top() + 14.0), galley, theme::FG);
    }
}

/// What VRChat shows otherwise than it was beside a write of `group`, until the person has seen to it: what it is, and each
/// thing that can be put back from here as a new write of its own, confirmed as any (the sparse updates rely on VRChat
/// leaving alone what is not sent: verify4).
fn beside_panel(ui: &mut Ui, ctx: &egui::Context, ctl: &Ctl, state: &mut State, group: &str) {
    let moved: Vec<Entry> = state.ledger.iter().filter(|e| e.group == group && !e.open() && e.beside.any()).cloned().collect();
    for e in moved {
        let back: Vec<Write> = state.back.iter().filter(|(op, _)| *op == e.op).map(|(_, w)| w.clone()).collect();
        warn_card(ui, &beside_text(&e, &back.iter().collect::<Vec<_>>()), 0.0, |_, _| {});
        let (free, telling) = (state.flow.is_none(), state.jobs.running(&Kind::SeenTo(e.op.clone())));
        let (mut put, mut over) = (None, false);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for w in &back {
                let label = match w {
                    Write::ApplyArt { slot: ArtSlot::Banner, .. } => tr("Put the banner back…"),
                    Write::ApplyArt { slot: ArtSlot::Icon, .. } => tr("Put the icon back…"),
                    _ => tr("Put the role back…"),
                };
                let r = widgets::Button::new(label).icon(icons::ARROW_COUNTER_CLOCKWISE).variant(widgets::Variant::Warn).size(widgets::Size::Sm).enabled(free).id_salt(("group-put-back", e.op.as_str(), label)).show(ui);
                if widgets::activated_fresh(&r) {
                    put = Some((w.clone(), r.rect));
                }
            }
            if widgets::Button::new(tr("Open on vrchat.com")).icon(icons::ARROW_SQUARE_OUT).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt(("group-beside-web", e.op.as_str())).show(ui).clicked() {
                if let Some(url) = orbiters_core::groups::web_page(group) {
                    let _ = crate::controller::open_url(&url);
                }
            }
            let r = widgets::Button::new(tr("I have seen to it")).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).enabled(!telling).id_salt(("group-seen-to", e.op.as_str())).show(ui);
            over = widgets::activated_fresh(&r);
        });
        if let Some((write, origin)) = put {
            state.prepare(ctx, ctl, write, origin);
        } else if let Some(scope) = state.scope.clone().filter(|_| over) {
            state.jobs.ask(ctx, ctl, &scope, Kind::SeenTo(e.op.clone()), Request::SeenTo { op: e.op.clone() });
        }
    }
}

fn fits(ui: &Ui, label: &str, glyph: f32) -> f32 {
    widgets::text_size(ui, &Txt::new(label, 14.0).weight(weight::MEDIUM)).x + 36.0 + glyph
}

fn warn_card(ui: &mut Ui, text: &str, button_w: f32, button: impl FnOnce(&mut Ui, Rect)) {
    let w = ui.available_width();
    let p = ui.painter().clone();
    let galley = p.layout_job(Txt::new(text, 13.0).color(theme::FG).line_height(19.0).job((w - 42.0 - button_w - 34.0).max(120.0)));
    let h = galley.size().y.max(20.0) + 28.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    p.rect_filled(rect, theme::radius(14.0), theme::alpha(theme::WARN, 0.12));
    p.rect_stroke(rect, theme::radius(14.0), egui::Stroke::new(1.0, theme::alpha(theme::WARN, 0.45)), egui::StrokeKind::Inside);
    widgets::icon(&p, pos2(rect.left() + 22.0, rect.top() + 24.0), icons::WARNING, 17.0, theme::WARN);
    p.galley(pos2(rect.left() + 42.0, rect.top() + 14.0), galley, theme::FG);
    if button_w > 0.0 {
        button(ui, Rect::from_min_size(pos2(rect.right() - 14.0 - button_w, (rect.center().y - 17.0).min(rect.top() + 10.0)), vec2(button_w, 34.0)));
    }
}

/// One thing VRChat shows that the write may have made, its picture when it is one, and "This is it".
fn candidate_row(ui: &mut Ui, op: &str, label: &str, c: &Candidate, thumb: Option<&egui::TextureHandle>, telling: bool, choice: &mut Option<Choice>) {
    let w = ui.available_width();
    let bw = fits(ui, label, 0.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, 46.0), Sense::hover());
    let p = ui.painter().clone();
    widgets::frame(&p, rect, 12.0, theme::surface());
    let mut x = rect.left() + 16.0;
    if !c.url.is_empty() {
        let r = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top() + 6.0), vec2(60.0, 34.0));
        p.rect_filled(r, theme::radius(8.0), theme::INK_750);
        if let Some(t) = thumb {
            paint::squircle_texture(&p, r, [theme::squircle(8.0); 4], t.id(), super::launcher::cover_uv(t.size_vec2(), r.size()), Color32::WHITE);
        }
        x = r.right() + 12.0;
    }
    let name = if c.label.is_empty() { tr("A picture").to_string() } else { c.label.clone() };
    let when = c.at.as_deref().map(super::local_time).unwrap_or_else(|| "—".to_string()); // i18n: not shown
    let same = match c.like {
        Likeness::Exactly => tr("exactly what you asked for"),
        Likeness::NotExactly => tr("not exactly what you asked for"),
        Likeness::Since => tr("added since you asked"),
        Likeness::Uncertain => tr("it may have been there before"),
    };
    widgets::text_truncated(&p, pos2(x, rect.center().y), Align2::LEFT_CENTER, &Txt::new(format!("{name} · {} · {when} · {same}", tail(&c.id)), 13.0).color(theme::FG_2), rect.right() - bw - 28.0 - x);
    let b = Rect::from_min_size(pos2(rect.right() - 10.0 - bw, rect.center().y - 16.0), vec2(bw, 32.0));
    let r = widgets::Button::new(label).size(widgets::Size::Sm).enabled(!telling).id_salt(("group-candidate", op, c.id.as_str())).show_at(ui, b);
    if widgets::activated_fresh(&r) {
        *choice = Some(Choice::Shows { made: Some(c.id.clone()) });
    }
}

/// What a write that may have been done was.
fn uncertain_words(kind: Wrote) -> &'static str {
    match kind {
        Wrote::AssignRole => tr("A role given to a member may or may not have been given."),
        Wrote::UnassignRole => tr("A role taken back from a member may or may not have been taken back."),
        Wrote::CreateRole => tr("A new role may or may not have been created."),
        Wrote::EditRole => tr("A role change may or may not have been saved."),
        Wrote::DeleteRole => tr("A role may or may not have been deleted."),
        Wrote::CreatePost => tr("A post may or may not have been published."),
        Wrote::DeletePost => tr("A post may or may not have been deleted."),
        Wrote::UploadGallery | Wrote::UploadIcon => tr("A picture may or may not have been uploaded to your inventory."),
        Wrote::ApplyBanner | Wrote::ApplyIcon => tr("The group's picture may or may not have been changed."),
    }
}

/// What is known of it: what VRChat shows now, and never what happened.
fn seen_words(e: &Entry, seen: Option<&Observation>) -> String {
    let text = match seen {
        _ if e.state == Stands::Dispatched => tr("It is still being sent: check again in a moment."),
        Some(Observation::Running) => tr("It is still being sent: check again in a moment."),
        None if e.answer == Some(Looked::ShowsNot) => tr("VRChat does not show it now: it may never have applied, or someone changed it since."),
        None => tr("Its answer was lost. The same is not sent again until you check."),
        Some(Observation::ShowsNot) => tr("VRChat does not show it now: it may never have applied, or someone changed it since."),
        Some(Observation::CannotTell(Unclear::TooSoon)) => tr("Cannot tell yet: VRChat does not show it, and it was asked for moments ago. Check again in a minute."),
        Some(Observation::CannotTell(Unclear::NotRead)) => tr("Cannot tell: VRChat's answer does not say. Look on vrchat.com."),
        Some(Observation::CannotTell(Unclear::NotSeen)) => tr("Cannot tell: it is not in what VRChat shows your account, which proves nothing. Look on vrchat.com."),
        Some(Observation::CannotTell(Unclear::Rejoined)) => tr("Cannot tell: this person left the group and joined again since. Look on vrchat.com."),
        Some(Observation::CannotTell(Unclear::NoRecord)) => tr("Cannot tell: its record here cannot be read. Look on vrchat.com."),
        Some(Observation::CannotTell(Unclear::Failed(f))) => return format!("{} {}", tr("Cannot tell: VRChat could not be read. Look on vrchat.com, or check again."), look_failure_text(*f)),
        Some(Observation::Candidates(list)) if list.is_empty() => tr("Cannot tell: nothing new is in what VRChat shows your account, which proves nothing. Look on vrchat.com."),
        Some(Observation::Candidates(_)) => tr("Cannot tell whether one of these is yours: only you can."),
        Some(Observation::Shows | Observation::Closed | Observation::Answered { .. }) => "",
    };
    text.to_string()
}

/// Home's Groups section, at its bottom: absent while VRChat is not signed in for certain, while it is read, and when
/// the account runs no group; "could not load" when reading failed (never "no groups", M1).
pub fn home_section(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    if std::mem::take(&mut state.reveal) {
        ui.scroll_to_cursor(Some(egui::Align::TOP));
    }
    state.sync(cx);
    state.take(&ctx, cx);
    let Some(scope) = state.scope.clone() else { return };
    let now = anim::time(&ctx);
    let due = match &state.discovery {
        Discovery::Idle => true,
        Discovery::Ready(_, at) | Discovery::Failed(_, at) => now - at > READ_AGAIN,
        Discovery::Loading => false,
    };
    if due && !state.staged && !state.held {
        if !matches!(state.discovery, Discovery::Ready(..)) {
            state.discovery = Discovery::Loading;
        }
        state.jobs.ask(&ctx, cx.ctl, &scope, Kind::Discover, Request::Discover);
    }
    let w = ui.available_width();
    match &state.discovery {
        Discovery::Ready(cards, _) if !cards.is_empty() => {
            let count = cards.len();
            caption(ui, w, &orbiters_i18n::tr!("Your groups · {n}", n = count));
            ui.add_space(-8.0);
            let cols = (((w + GAP) / (CARD_MIN_W + GAP)).floor() as usize).clamp(1, 3);
            let card_w = (w - GAP * (cols - 1) as f32) / cols as f32;
            let rows = count.div_ceil(cols);
            let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (CARD_H + GAP) - GAP), Sense::hover());
            // Cards in view without their names yet are read, a few at a time (Major 7).
            let mut unnamed: Vec<String> = Vec::new();
            for i in 0..count {
                let at = area.min + vec2((i % cols) as f32 * (card_w + GAP), (i / cols) as f32 * (CARD_H + GAP));
                let rect = Rect::from_min_size(at, vec2(card_w, CARD_H));
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let Discovery::Ready(cards, _) = &state.discovery else { break };
                let card = cards[i].clone();
                if !card.named && state.hydrate_again.get(&card.id).is_none_or(|at| now >= *at) && unnamed.len() < crate::controller::groups::HYDRATE_BATCH {
                    unnamed.push(card.id.clone());
                }
                match group_card(ui, cx, state, &scope, &card, rect) {
                    Some(CardPress::Open) => {
                        state.end_flow();
                        state.open = Some(page::Open::new(card, &state.drafts));
                        if let (Some(open), false) = (state.open.as_mut(), state.staged) {
                            open.as_kept(&scope);
                        }
                        state.ledger_due = true;
                    }
                    Some(CardPress::Studio) => {
                        state.end_flow();
                        state.studio = Some(studio::Studio::new(card, owner_of(&scope), None));
                        state.ledger_due = true;
                    }
                    None => {}
                }
            }
            if !unnamed.is_empty() && !state.staged && !state.held && !state.jobs.busy_with(|k| *k == Kind::Hydrate) {
                state.jobs.ask(&ctx, cx.ctl, &scope, Kind::Hydrate, Request::Hydrate { ids: unnamed });
            }
        }
        Discovery::Failed(f, _) => {
            let f = *f;
            caption(ui, w, tr("Your groups"));
            ui.add_space(-8.0);
            let (rect, _) = ui.allocate_exact_size(vec2(w, 64.0), Sense::hover());
            let p = ui.painter().clone();
            widgets::frame(&p, rect, theme::CARD_RADIUS, theme::surface());
            widgets::icon(&p, pos2(rect.left() + 26.0, rect.center().y), icons::WARNING_CIRCLE, 18.0, theme::WARN);
            let line = format!("{} {}", tr("Your groups could not be loaded."), failure_text(f));
            let bw = fits(ui, tr("Read again"), 24.0);
            widgets::text_truncated(&p, pos2(rect.left() + 46.0, rect.center().y), Align2::LEFT_CENTER, &Txt::new(line, 13.0).color(theme::FG_2), rect.width() - 74.0 - bw);
            let b = Rect::from_min_size(pos2(rect.right() - 16.0 - bw, rect.center().y - 17.0), vec2(bw, 34.0));
            if widgets::Button::new(tr("Read again")).icon(icons::ARROW_CLOCKWISE).size(widgets::Size::Sm).id_salt("groups-retry").show_at(ui, b).clicked() {
                state.discovery = Discovery::Loading;
                state.jobs.ask(&ctx, cx.ctl, &scope, Kind::Discover, Request::Discover);
            }
        }
        _ => {}
    }
}

fn caption(ui: &mut Ui, w: f32, text: &str) {
    let p = ui.painter().clone();
    let (cap, _) = ui.allocate_exact_size(vec2(w, 14.0), Sense::hover());
    widgets::text(&p, pos2(cap.left() + 2.0, cap.center().y), Align2::LEFT_CENTER, &Txt::caps(text));
}

enum CardPress {
    Open,
    Studio,
}

/// What the person may do in a group, in a few words for its card.
pub(crate) fn abilities_line(card: &GroupCard) -> String {
    let a = card.abilities();
    let mut parts: Vec<&str> = Vec::new();
    if card.owner {
        parts.push(tr("Owner"));
    }
    if a.roles || a.assign {
        parts.push(tr("Roles"));
    }
    if a.posts {
        parts.push(tr("Posts"));
    }
    if a.art {
        parts.push(tr("Banner"));
    }
    if parts.is_empty() || (!card.owner && a.elsewhere && !(a.roles || a.assign || a.posts || a.art)) {
        parts.push(tr("Moderation on vrchat.com"));
    }
    parts.join(" · ")
}

pub(crate) fn short_code(code: &str, discriminator: &str) -> String {
    match (code.is_empty(), discriminator.is_empty()) {
        (true, _) => String::new(),
        (false, true) => code.to_string(),
        (false, false) => format!("{code}.{discriminator}"),
    }
}

/// A card: the group's banner behind, its icon, name, code, members and what the person can do; pressed, its page; its
/// corner button, the banner studio.
fn group_card(ui: &mut Ui, cx: &Cx, state: &mut State, scope: &Scope, card: &GroupCard, rect: Rect) -> Option<CardPress> {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("group-card", card.id.as_str()));
    let resp = ui.interact(rect, id, Sense::click());
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let r = rect.translate(vec2(0.0, -2.0 * hover));
    let p = ui.painter().clone();
    paint::shadow(&p, r, theme::CARD_RADIUS, vec2(0.0, 8.0), 22.0, -8.0, theme::black(0.22 + 0.12 * hover));
    widgets::frame(&p, r, theme::CARD_RADIUS, theme::surface());
    let banner = state.picture(&ctx, cx.ctl, scope, &card.banner_url);
    let radii = [theme::squircle(theme::CARD_RADIUS); 4];
    let shown_name = orbiters_core::groups::label(&card.name, 64);
    if let Some(t) = &banner {
        paint::squircle_texture(&p, r, radii, t.id(), super::launcher::cover_uv(t.size_vec2(), r.size()), Color32::WHITE);
    } else {
        let tint = widgets::name_color(&shown_name);
        paint::fill_convex(&p, &paint::squircle_points(r, theme::squircle(theme::CARD_RADIUS)), |at| {
            let t = ((at.x - r.left()) / r.width()).clamp(0.0, 1.0);
            theme::mix(theme::mix(tint, theme::INK_850, 0.55), theme::INK_850, t)
        });
    }
    // A veil from the left, so the words read over any banner.
    paint::fill_convex(&p, &paint::squircle_points(r, theme::squircle(theme::CARD_RADIUS)), |at| {
        let t = ((at.x - r.left()) / r.width()).clamp(0.0, 1.0);
        theme::black(0.78 - 0.5 * t + 0.08 * hover)
    });
    paint::stroke_squircle(&p, r, theme::squircle(theme::CARD_RADIUS), theme::hairline());
    let icon_rect = Rect::from_min_size(pos2(r.left() + 18.0, r.top() + 18.0), vec2(52.0, 52.0));
    let icon = state.picture(&ctx, cx.ctl, scope, &card.icon_url);
    p.circle_filled(icon_rect.center(), 28.0, theme::black(0.35));
    match &icon {
        Some(t) => paint::circle_texture(&p, icon_rect, t.id(), super::launcher::cover_uv(t.size_vec2(), icon_rect.size()), Color32::WHITE),
        None => widgets::avatar(&p, icon_rect, None, &shown_name),
    }
    let tx = icon_rect.right() + 14.0;
    let room = r.right() - 56.0 - tx;
    let name = if shown_name.is_empty() { tr("A group you help run") } else { shown_name.as_str() };
    widgets::text_truncated(&p, pos2(tx, icon_rect.top() + 14.0), Align2::LEFT_CENTER, &Txt::new(name, 17.0).weight(weight::SEMIBOLD), room);
    let code = short_code(&card.short_code, &card.discriminator);
    // A count not read yet is never shown as zero (Major 7).
    let members = match card.member_count {
        Some(n) => orbiters_i18n::tr!("{n} members", n = n),
        None => tr("Reading…").to_string(),
    };
    let line = if code.is_empty() { members } else { format!("{code} · {members}") };
    let line = match card.online_count {
        Some(n) => format!("{line} · {}", orbiters_i18n::tr!("{n} online", n = n)),
        None => line,
    };
    // Under the corner button: the line takes the card's width.
    widgets::text_truncated(&p, pos2(tx, icon_rect.top() + 38.0), Align2::LEFT_CENTER, &Txt::new(line, 12.5).color(theme::FG_2), r.right() - 18.0 - tx);
    // What the person can do here.
    let what = abilities_line(card);
    let pill = Txt::new(&what, 12.0).weight(weight::MEDIUM).color(theme::FG);
    let pw = (widgets::text_size(ui, &pill).x + 34.0).min(r.width() - 36.0);
    let pr = Rect::from_min_size(pos2(r.left() + 18.0, r.bottom() - 18.0 - 28.0), vec2(pw, 28.0));
    p.rect_filled(pr, theme::radius(14.0), theme::black(0.42));
    widgets::icon(&p, pos2(pr.left() + 15.0, pr.center().y), if card.owner { icons::SHIELD_CHECK } else { icons::USERS_THREE }, 13.0, theme::ACCENT);
    widgets::text_truncated(&p, pos2(pr.left() + 27.0, pr.center().y), Align2::LEFT_CENTER, &pill, pr.width() - 34.0);
    // The banner studio, in the corner.
    let sb = Rect::from_min_size(pos2(r.right() - 48.0, r.top() + 14.0), vec2(36.0, 36.0));
    let studio = widgets::IconButton::new(icons::IMAGE, tr("Banner studio")).id_salt(("group-card-studio", card.id.as_str())).backdrop(theme::black(0.45)).show_at(ui, sb);
    if studio.clicked() {
        return Some(CardPress::Studio);
    }
    let open = widgets::activated(&resp) && !studio.hovered();
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Open the group's page"));
    open.then_some(CardPress::Open)
}

/// A group's page or the studio, in Home's place.
pub fn show_page(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    state.sync(cx);
    state.take(&ctx, cx);
    if state.scope.is_none() && !state.staged {
        state.close();
        return;
    }
    // What is in front of the page says so, for what stands aside under it (the confirmation says it as a dialog).
    if state.holds() {
        orbiters_ui::front::hold(&ctx, Id::unique("groups-sheet"));
    }
    if state.studio.is_some() {
        studio::show(ui, cx, state);
    } else if state.open.is_some() {
        page::show(ui, cx, state);
    }
    confirmation(ui, cx, state);
    state.keep_drafts();
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !matches!(state.flow, Some(Flow::Confirming { .. })) {
        if state.studio.is_some() {
            state.close_studio();
        } else {
            state.close();
        }
    }
}

/// The confirmation of a sealed write, drawn from it and nothing else: exactly what goes, and who it reaches (M7, B-E,
/// B-F). Send commits that one sealed write once, on a deliberate press (a held key's repeats never count, Blocker 6).
fn confirmation(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let Some(Flow::Confirming { sealed, serial, origin, art }) = &state.flow else { return };
    let (sealed, serial, origin, art) = (sealed.clone(), *serial, *origin, art.clone());
    let id = Id::unique(("group-confirm", serial));
    let (title, action, danger) = confirm_words(&sealed);
    let mut send = false;
    let mut cancel = false;
    // A picture goes only once it shows here, from the sealed bytes: nobody applies one they cannot see (Blocker 5).
    let shown = sealed.art.is_none() || art.is_some();
    let response = orbiters_ui::dialog::Dialog::new(id, origin, &title).width(520.0).glyph(if danger { icons::WARNING } else { icons::CHECK_CIRCLE }).footer(true).show(&ctx, |ui, part| match part {
        orbiters_ui::dialog::Part::Body => page::confirm_body(ui, &sealed, art.as_ref()),
        orbiters_ui::dialog::Part::Footer => {
            let variant = if danger { widgets::Variant::Danger } else { widgets::Variant::Primary };
            let button = widgets::Button::new(&action).variant(variant).enabled(shown).id_salt("group-confirm-send").show(ui);
            if widgets::activated_fresh(&button) {
                send = true;
            }
            ui.add_space(8.0);
            if widgets::Button::new(tr("Cancel")).variant(widgets::Variant::Ghost).id_salt("group-confirm-cancel").show(ui).clicked() {
                cancel = true;
            }
        }
    });
    if send {
        orbiters_ui::dialog::close(&ctx, id);
        state.commit(&ctx, cx.ctl, sealed);
    } else if cancel || (response.closing && matches!(state.flow, Some(Flow::Confirming { .. }))) {
        orbiters_ui::dialog::close(&ctx, id);
        state.cancel();
    }
}

fn confirm_words(p: &Sealed) -> (String, String, bool) {
    let s = |a: &str, b: &str, d: bool| (a.to_string(), b.to_string(), d);
    match &p.write {
        Write::MemberRole { add: true, .. } => s(tr("Give this role?"), tr("Give the role"), false),
        Write::MemberRole { add: false, .. } => s(tr("Take this role back?"), tr("Take it back"), true),
        Write::CreateRole { .. } => s(tr("Create this role?"), tr("Create the role"), false),
        Write::EditRole { back: Some(_), .. } => s(tr("Put this role back?"), tr("Put it back"), false),
        Write::ApplyArt { slot: ArtSlot::Banner, back: Some(_), .. } => s(tr("Put the group's banner back?"), tr("Put it back"), false),
        Write::ApplyArt { slot: ArtSlot::Icon, back: Some(_), .. } => s(tr("Put the group's icon back?"), tr("Put it back"), false),
        Write::EditRole { .. } => s(tr("Change this role?"), tr("Save the role"), false),
        Write::DeleteRole { .. } => s(tr("Delete this role?"), tr("Delete the role"), true),
        Write::CreatePost { .. } => s(tr("Post this?"), tr("Post"), false),
        Write::DeletePost { .. } => s(tr("Delete this post?"), tr("Delete the post"), true),
        Write::Upload { tag: FileTag::Gallery, .. } => s(tr("Upload to your VRChat gallery?"), tr("Upload"), false),
        Write::Upload { tag: FileTag::Icon, .. } => s(tr("Upload to your VRChat icons?"), tr("Upload"), false),
        Write::ApplyArt { slot: ArtSlot::Banner, .. } => s(tr("Change the group's banner?"), tr("Change the banner"), false),
        Write::ApplyArt { slot: ArtSlot::Icon, .. } => s(tr("Change the group's icon?"), tr("Change the icon"), false),
    }
}

/// A PNG put in the service's staging folder for an upload (Major 1): only its name, size and hash cross the link.
pub(crate) fn stage_upload(png: &[u8]) -> std::io::Result<crate::controller::groups::Staged> {
    use sha2::Digest;
    use std::io::Write as _;
    let dir = crate::controller::groups::staging_dir();
    std::fs::create_dir_all(&dir)?;
    let name = format!("{}.png", uuid::Uuid::new_v4());
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join(&name))?;
    f.write_all(png)?;
    f.sync_all()?;
    let sha256 = sha2::Sha256::digest(png).iter().map(|b| format!("{b:02x}")).collect();
    Ok(crate::controller::groups::Staged { name, size: png.len() as u64, sha256 })
}

/// The screenshot review: made-up groups, members and art only, nothing asked of the service.
pub mod stage {
    use super::*;
    pub fn groups(state: &mut State, cards: Vec<GroupCard>) {
        state.staged = true;
        state.discovery = Discovery::Ready(cards, 1.0e9);
    }
    pub fn failed(state: &mut State) {
        state.staged = true;
        state.discovery = Discovery::Failed(Failure::Api(GroupFailure::Unavailable), 1.0e9);
    }
    pub fn scope(state: &mut State, scope: Scope) {
        state.staged = true;
        state.scope = Some(scope);
    }
    pub fn picture(ctx: &egui::Context, state: &mut State, url: &str, img: egui::ColorImage) {
        state.pictures.stage(ctx, url, img);
    }
    pub fn open(state: &mut State, card: GroupCard, page: crate::controller::groups::Page) -> &mut page::Open {
        state.staged = true;
        let mut open = page::Open::new(card, &Drafts::default());
        open.answer(Kind::Open(open.id.clone()), Reply::Opened(Box::new(page)));
        state.open = Some(open);
        state.open.as_mut().unwrap()
    }
    pub fn studio<'a>(state: &'a mut State, card: GroupCard, source: image::RgbaImage) -> &'a mut studio::Studio {
        state.staged = true;
        state.studio = Some(studio::Studio::new(card, None, Some(source)));
        state.studio.as_mut().unwrap()
    }
    pub fn studio_empty(state: &mut State, card: GroupCard) {
        state.staged = true;
        state.studio = Some(studio::Studio::staged_empty(card));
    }
    /// A sealed write shown, with a staged picture (by its address) for its art.
    pub fn confirming(state: &mut State, sealed: Sealed, origin: Rect, art: Option<&str>) {
        let art = art.and_then(|url| match state.pictures.map.get(url) {
            Some(Pic::Ready(t)) => Some(t.clone()),
            _ => None,
        });
        state.serial += 1;
        state.flow = Some(Flow::Confirming { sealed, serial: state.serial, origin, art });
    }
    pub fn named(state: &mut State, me: &str) {
        state.me_name = me.into();
    }
    pub fn unreadable(state: &mut State, n: usize) {
        state.unreadable = n;
    }
    pub fn entry(state: &mut State, entry: Entry, observed: Option<Observation>) {
        if let Some(o) = observed {
            state.observed.insert(entry.op.clone(), o);
        }
        state.ledger.push(entry);
    }
    pub fn back(state: &mut State, op: &str, write: Write) {
        state.back.push((op.into(), write));
    }
}

#[cfg(test)]
#[path = "window_tests.rs"]
mod window_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// M9: an answer asked under an earlier generation (another account, a page closed) is dropped, never shown; and a
    /// late answer of an old request never clears the mark of a new one with the same key (Major 9).
    #[test]
    fn obsolete_answers_are_dropped() {
        let mut jobs = Jobs::default();
        jobs.running.push((jobs.generation, Kind::Discover));
        let old = jobs.generation;
        jobs.reset();
        jobs.running.push((jobs.generation, Kind::Discover));
        jobs.tx.send(Answer { generation: old, kind: Kind::Discover, reply: Reply::Discovered(vec![GroupCard::default()]), image: None, source: None }).unwrap();
        assert!(jobs.take().is_empty(), "a reply to the old account is not taken");
        assert!(jobs.running(&Kind::Discover), "the new request's mark stays");
        jobs.tx.send(Answer { generation: jobs.generation, kind: Kind::Discover, reply: Reply::Discovered(vec![]), image: None, source: None }).unwrap();
        assert_eq!(jobs.take().len(), 1);
        assert!(!jobs.running(&Kind::Discover));
    }

    /// M10: what a done write says names nobody and no group, role or post.
    #[test]
    fn toasts_carry_no_names() {
        use crate::controller::groups::Done;
        let role = orbiters_core::groups::Role { name: "Secret Role Name".into(), ..Default::default() };
        let post = orbiters_core::groups::Post { title: "Secret Post Title".into(), ..Default::default() };
        for done in [Done::Role(role.clone()), Done::Roles(vec![role]), Done::Post(post), Done::MemberRoles { user: "usr_x".into(), role_ids: vec![] }] {
            let text = done_text(&done);
            assert!(!text.contains("Secret") && !text.contains("usr_"), "{text}");
        }
        for f in [Failure::AccountChanged, Failure::Offline, Failure::Lost, Failure::NotKept, Failure::LookNotKept, Failure::NotRead(Missing::Member), Failure::Expired, Failure::MaySent, Failure::RecordUnreadable, Failure::OtherOwner, Failure::Busy, Failure::SignedOut, Failure::Api(GroupFailure::Unavailable), Failure::Api(GroupFailure::Disabled), Failure::Api(GroupFailure::Unknown)] {
            assert!(!look_failure_text(f).contains("sent"), "{f:?}: a check says nothing of what was sent: {}", look_failure_text(f));
        }
        for f in [Failure::Api(GroupFailure::Forbidden), Failure::Changed, Failure::PostChanged(Default::default()), Failure::RoleChanged(Default::default()), Failure::Rejoined, Failure::Since, Failure::RecordUnreadable, Failure::OtherOwner, Failure::Withheld(Why::RoleAtOrAbove), Failure::NotRead(Missing::RoleOrder), Failure::MaySent, Failure::NotKept, Failure::LookNotKept, Failure::Offline, Failure::Lost] {
            assert!(!failure_text(f).is_empty());
        }
    }

    /// Pictures are kept in memory, a bounded number: the oldest go first (M10).
    #[test]
    fn pictures_are_bounded() {
        let ctx = egui::Context::default();
        let mut p = Pictures::default();
        for i in 0..PICTURES_MAX + 5 {
            p.stage(&ctx, &format!("https://api.vrchat.cloud/api/1/file/file_{i}/1/file"), egui::ColorImage::new([1, 1], vec![Color32::WHITE]));
        }
        assert_eq!(p.map.len(), PICTURES_MAX);
        assert!(!p.map.contains_key("https://api.vrchat.cloud/api/1/file/file_0/1/file"));
    }

    /// Major 6: a picture that failed or is on its way is not asked again by itself.
    #[test]
    fn a_failed_picture_is_not_asked_again() {
        let ctx = egui::Context::default();
        let ctl: Ctl = std::sync::Arc::new(crate::backend::Local::new(crate::controller::Controller::new(orbiters_core::settings::Settings::default())));
        let scope = Scope::of(&ctl.view());
        let mut jobs = Jobs::default();
        let mut p = Pictures::default();
        let url = "https://api.vrchat.cloud/api/1/file/file_1/1/file";
        p.arrived(&ctx, url.to_string(), None);
        for _ in 0..5 {
            assert!(p.get(&ctx, &mut jobs, &ctl, &scope, url).is_none());
        }
        assert_eq!(p.asked, 0, "never asked again");
    }
}
