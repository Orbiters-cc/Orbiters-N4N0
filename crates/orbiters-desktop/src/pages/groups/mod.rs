//! VRChat groups in the window (REQUEST22 1): the Groups section at the bottom of Home (the groups the signed-in account
//! owns or helps run), each group's page (Look, Members, Roles, Posts) and the banner studio.
//!
//! What it shows is asked of the service on demand and answered to this window alone (`controller::groups`); nothing
//! of others' data is kept on disk. On disk, per account: the studio's own crops and exports, and the operator's own
//! drafts, uploaded-not-applied pictures and writes whose outcome is not known yet (verify-r22 Major 8). Everything is
//! bound to the account scope it was asked under: another account (or "Connecting") drops it all, confirmations
//! included (B3). Pictures of groups and members are kept in memory only, a bounded number, asked for what is in view.
//!
//! A write whose answer did not come back is UNKNOWN here, whatever the cause (the service's reply lost on the link, a
//! timeout, VRChat's answer lost): it stays listed with "Check VRChat" until a check settles it, across tabs, pages and
//! restarts, and nothing about its target is prepared again meanwhile (Blockers 1 and 2).
pub mod art;
mod page;
mod studio;
pub(crate) use page::{Composer, RoleEditor, Tab};

use super::Cx;
use crate::backend::Ctl;
use crate::controller::groups::{Failure, Observation, Outcome, Prepared, Reply, Request, Write};
use crate::controller::vrchat_data::Scope;
use crate::controller::vrchat_login::Presence;
use art::{Drafts, Uncertain};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui};
use orbiters_core::groups::{FileTag, GroupCard, Why};
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
    Prepare,
    Commit,
    Cancel,
    Check(String),
    Release(String),
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
        if self.running(&kind) && !matches!(kind, Kind::Prepare | Kind::Commit | Kind::Cancel) {
            return;
        }
        self.running.push((self.generation, kind.clone()));
        let (tx, ctl, ctx, scope, generation) = (self.tx.clone(), ctl.clone(), ctx.clone(), scope.clone(), self.generation);
        // Work of the window while it runs: it does not rest to the tray in the middle of it.
        let working = crate::pages::Working::begin();
        std::thread::spawn(move || {
            let _working = working;
            let reply = ctl.groups(scope, request);
            let image = match (&kind, &reply) {
                (Kind::Picture(_), Reply::Picture(bytes)) => decode_picture(bytes),
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

/// A picture as a texture's pixels, at most 640 px a side, decoded within bounds (Major 5).
fn decode_picture(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = art::decode_bounded(bytes, art::PICTURE_PIXELS).ok()?;
    let img = if img.width() > 640 || img.height() > 640 { image::imageops::thumbnail(&img, 640.min(img.width()), ((640.0 * img.height() as f32 / img.width() as f32) as u32).clamp(1, 640)) } else { img };
    Some(egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw()))
}

enum Pic {
    Waiting,
    Ready(egui::TextureHandle),
    Failed,
}

/// Group and member pictures, in memory only, the newest [`PICTURES_MAX`] (M10). A picture that failed is not asked
/// again by itself.
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
        if jobs.pictures_running() >= PICTURES_AT_ONCE {
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
}

#[derive(Default)]
pub(crate) enum Discovery {
    #[default]
    Idle,
    Loading,
    Ready(Vec<GroupCard>, f64),
    Failed(Failure, f64),
}

/// A write on its way: prepared, waiting for the person's yes, sent.
pub(crate) enum Flow {
    Preparing,
    Confirming { prepared: Prepared, serial: u64, origin: Rect },
    Sending { prepared: Prepared },
}

pub struct State {
    /// The scope everything here was asked under; `None` while VRChat is not signed in for certain.
    scope: Option<Scope>,
    pub(crate) jobs: Jobs,
    pub(crate) pictures: Pictures,
    pub(crate) discovery: Discovery,
    /// A group's page, open in Home's place.
    pub(crate) open: Option<page::Open>,
    /// The banner studio, over the page or straight from a card.
    pub(crate) studio: Option<studio::Studio>,
    pub(crate) flow: Option<Flow>,
    serial: u64,
    /// The screenshot review staged what shows: nothing is asked of the service.
    staged: bool,
    /// Where the last press that prepares a write was (the confirmation grows out of it).
    pub(crate) origin: Rect,
    /// The account's own drafts, uploads not applied and writes whose outcome is not known (on disk, Major 8).
    pub(crate) drafts: Drafts,
    /// What was last written to disk, to write only on a change.
    drafts_written: String,
    /// The art a picture write puts on VRChat, shown in its confirmation (Blocker 5).
    pub(crate) confirm_art: Option<egui::TextureHandle>,
    /// What the last refused write said, for the page that shows it.
    pub(crate) said: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            scope: None,
            jobs: Jobs::default(),
            pictures: Pictures::default(),
            discovery: Discovery::default(),
            open: None,
            studio: None,
            flow: None,
            serial: 0,
            staged: false,
            origin: Rect::NOTHING,
            drafts: Drafts::default(),
            drafts_written: String::new(),
            confirm_art: None,
            said: None,
        }
    }
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
    /// Back to Home (the sidebar's Home). What the operator was writing is kept on disk first.
    pub fn close(&mut self) {
        self.keep_drafts();
        self.open = None;
        self.studio = None;
        self.flow = None;
        self.jobs.reset();
    }
    pub(crate) fn account(&self) -> Option<&str> {
        self.scope.as_ref().and_then(|s| s.vrchat_user())
    }
    fn signed(cx: &Cx) -> Option<Scope> {
        (cx.snap.vrchat_login.presence() == Presence::SignedIn).then(|| Scope::of(cx.snap))
    }
    /// Another account, the environment, or not signed in for certain: everything goes (B3); the new account's own
    /// drafts and pending checks come back from disk.
    fn sync(&mut self, cx: &Cx) {
        if self.staged {
            return;
        }
        let now = Self::signed(cx);
        if now != self.scope {
            self.keep_drafts();
            self.scope = now;
            self.jobs.reset();
            self.pictures.clear();
            self.discovery = Discovery::Idle;
            self.open = None;
            self.studio = None;
            self.flow = None;
            self.confirm_art = None;
            self.drafts = self.account().map(art::load_drafts).unwrap_or_default();
            self.drafts_written = serde_json::to_string(&self.drafts).unwrap_or_default();
        }
    }
    /// Writes the drafts when they changed (the composer's text included).
    pub(crate) fn keep_drafts(&mut self) {
        if self.staged {
            return;
        }
        if let Some(open) = self.open.as_ref() {
            self.drafts.set_composer(&open.id, open.composer.as_ref().map(|c| art::ComposerDraft { baseline: c.baseline.clone(), draft: c.draft.clone() }));
        }
        let text = serde_json::to_string(&self.drafts).unwrap_or_default();
        if text != self.drafts_written {
            if let Some(account) = self.account().map(str::to_string) {
                art::store_drafts(&account, &self.drafts);
                self.drafts_written = text;
            }
        }
    }
    /// The writes of `group` whose outcome is not known yet.
    pub(crate) fn uncertain_of(&self, group: &str) -> Vec<Uncertain> {
        self.drafts.uncertain.iter().filter(|u| u.write.group() == group).cloned().collect()
    }
    fn add_uncertain(&mut self, op: String, write: Write) {
        if !self.drafts.uncertain.iter().any(|u| u.op == op) {
            self.drafts.uncertain.push(Uncertain { op, write, observation: None });
        }
        self.keep_drafts();
    }
    fn settle(&mut self, op: &str) {
        self.drafts.uncertain.retain(|u| u.op != op);
        self.keep_drafts();
    }

    /// Takes what came back.
    fn take(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        for a in self.jobs.take() {
            match (a.kind, a.reply) {
                (Kind::Discover, Reply::Discovered(cards)) => self.discovery = Discovery::Ready(cards, anim::time(ctx)),
                (Kind::Discover, Reply::Failed(f)) => self.discovery = Discovery::Failed(f, anim::time(ctx)),
                (Kind::Hydrate, Reply::Hydrated(named)) => {
                    if let Discovery::Ready(cards, _) = &mut self.discovery {
                        for n in named {
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
                (Kind::Check(op), reply) => self.checked(ctx, cx, op, reply),
                (Kind::Release(op), Reply::Released) => self.settle(&op),
                (Kind::Release(_), Reply::Failed(f)) => self.said = Some(failure_text(f)),
                (kind @ (Kind::Prepare | Kind::Commit | Kind::Cancel), reply) => self.flow_answer(ctx, cx, kind, reply),
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

    /// Prepares a write; its confirmation (when it needs one) grows out of `origin`. A target with a write whose outcome
    /// is not known yet is not prepared again: its check comes first (Blocker 2).
    pub(crate) fn prepare(&mut self, ctx: &egui::Context, ctl: &Ctl, write: Write, origin: Rect) {
        let Some(scope) = self.scope.clone() else { return };
        if self.flow.is_some() {
            return;
        }
        let target = write.target();
        if self.drafts.uncertain.iter().any(|u| u.write.target() == target) {
            self.said = Some(failure_text(Failure::CheckFirst));
            return;
        }
        self.said = None;
        self.origin = origin;
        self.flow = Some(Flow::Preparing);
        self.jobs.ask(ctx, ctl, &scope, Kind::Prepare, Request::Prepare { write });
    }
    fn commit(&mut self, ctx: &egui::Context, ctl: &Ctl, prepared: Prepared) {
        let Some(scope) = self.scope.clone() else { return };
        let op = prepared.op.clone();
        self.flow = Some(Flow::Sending { prepared });
        self.jobs.ask(ctx, ctl, &scope, Kind::Commit, Request::Commit { op });
    }
    fn cancel(&mut self, ctx: &egui::Context, ctl: &Ctl, op: String) {
        if let Some(scope) = self.scope.clone() {
            self.jobs.ask(ctx, ctl, &scope, Kind::Cancel, Request::Cancel { op });
        }
        self.flow = None;
        self.confirm_art = None;
    }
    /// The person asks what became of the uncertain write `op`.
    pub(crate) fn check(&mut self, ctx: &egui::Context, ctl: &Ctl, op: &str) {
        let (Some(scope), Some(u)) = (self.scope.clone(), self.drafts.uncertain.iter().find(|u| u.op == op).cloned()) else { return };
        self.jobs.ask(ctx, ctl, &scope, Kind::Check(op.to_string()), Request::Check { op: op.to_string(), write: u.write });
    }
    /// The person, told a check could not settle it, lets it go.
    pub(crate) fn release(&mut self, ctx: &egui::Context, ctl: &Ctl, op: &str) {
        let Some(scope) = self.scope.clone() else { return };
        self.jobs.ask(ctx, ctl, &scope, Kind::Release(op.to_string()), Request::Release { op: op.to_string() });
    }
    fn checked(&mut self, ctx: &egui::Context, cx: &mut Cx, op: String, reply: Reply) {
        let Reply::Checked { observation, .. } = reply else {
            if let Reply::Failed(f) = reply {
                self.said = Some(failure_text(f));
            }
            return;
        };
        let write = self.drafts.uncertain.iter().find(|u| u.op == op).map(|u| u.write.clone());
        match observation.clone() {
            Observation::Recorded(Outcome::Done(done)) => {
                self.settle(&op);
                let prepared = write.map(|w| Prepared { op: op.clone(), write: w, confirm: false, role: None, post: None, member: None });
                self.ended(ctx, cx, prepared, Outcome::Done(done));
            }
            Observation::Applied => {
                self.settle(&op);
                cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, tr("VRChat shows it done."));
                if let Some(open) = self.open.as_mut() {
                    open.reload();
                }
            }
            Observation::Recorded(_) | Observation::NotSent | Observation::NotApplied => {
                self.settle(&op);
                cx.toasts.push(ctx, orbiters_ui::toast::Kind::Info, tr("It was not done: you can do it again."));
                if let Some(open) = self.open.as_mut() {
                    open.reload();
                }
            }
            Observation::Running | Observation::Inconclusive => {
                if let Some(u) = self.drafts.uncertain.iter_mut().find(|u| u.op == op) {
                    u.observation = Some(observation);
                }
                self.keep_drafts();
            }
        }
    }
    fn flow_answer(&mut self, ctx: &egui::Context, cx: &mut Cx, kind: Kind, reply: Reply) {
        match (kind, reply) {
            (Kind::Prepare, Reply::Prepared(p)) => {
                if p.confirm {
                    self.serial += 1;
                    self.flow = Some(Flow::Confirming { prepared: *p, serial: self.serial, origin: self.origin });
                } else {
                    self.commit(ctx, cx.ctl, *p);
                }
            }
            // Another write on this target is still uncertain: it is listed, with its check (Blocker 2).
            (Kind::Prepare, Reply::Held { op, write }) => {
                self.flow = None;
                self.add_uncertain(op, write);
                self.said = Some(failure_text(Failure::CheckFirst));
            }
            (Kind::Prepare, Reply::Failed(f)) => {
                self.flow = None;
                self.ended(ctx, cx, None, Outcome::Refused(f));
            }
            (Kind::Commit, Reply::Outcome(outcome)) => {
                let prepared = match self.flow.take() {
                    Some(Flow::Sending { prepared }) => Some(prepared),
                    _ => None,
                };
                if outcome == Outcome::Unknown {
                    if let Some(p) = &prepared {
                        self.add_uncertain(p.op.clone(), p.write.clone());
                    }
                }
                self.ended(ctx, cx, prepared, outcome);
            }
            // The service's answer to a commit did not arrive: the write may have been done. Unknown, never "refused"
            // (Blocker 1).
            (Kind::Commit, Reply::Failed(Failure::Lost)) => {
                let prepared = match self.flow.take() {
                    Some(Flow::Sending { prepared }) => Some(prepared),
                    _ => None,
                };
                if let Some(p) = &prepared {
                    self.add_uncertain(p.op.clone(), p.write.clone());
                }
                self.ended(ctx, cx, prepared, Outcome::Unknown);
            }
            (Kind::Commit, Reply::Failed(f)) => {
                // Refused before it was looked at (the account changed, the service busy): nothing was sent.
                self.flow = None;
                self.ended(ctx, cx, None, Outcome::Refused(f));
            }
            _ => {}
        }
    }
    /// How a write ended: the page takes VRChat's answer as the state; refusals and unknowns stay on the page. Toasts
    /// carry no names (M10).
    fn ended(&mut self, ctx: &egui::Context, cx: &mut Cx, prepared: Option<Prepared>, outcome: Outcome) {
        self.confirm_art = None;
        if let Outcome::Done(done) = &outcome {
            cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, done_text(done));
        }
        match &outcome {
            Outcome::Refused(f) => self.said = Some(failure_text_for(prepared.as_ref().map(|p| &p.write), *f)),
            Outcome::Unknown => self.said = None,
            Outcome::Done(_) => self.said = None,
        }
        if let Some(studio) = self.studio.as_mut() {
            studio.ended(prepared.as_ref(), &outcome);
        }
        if let Some(open) = self.open.as_mut() {
            open.ended(prepared.as_ref(), outcome.clone());
        }
        // An upload done and not applied yet is remembered for its group (Major 8).
        if let (Some(p), Outcome::Done(crate::controller::groups::Done::Uploaded(file))) = (&prepared, &outcome) {
            if let Write::Upload { tag, group, .. } = &p.write {
                let slot = if *tag == FileTag::Icon { orbiters_core::groups::ArtSlot::Icon } else { orbiters_core::groups::ArtSlot::Banner };
                let revision = self.studio.as_ref().filter(|s| s.card.id == *group && slot == orbiters_core::groups::ArtSlot::Banner).map(|s| s.uploaded_revision());
                self.drafts.set_uploaded(art::UploadedRef { group: group.clone(), slot, file: file.clone(), revision: revision.flatten() });
            }
        }
        if let (Some(p), Outcome::Done(crate::controller::groups::Done::Group(_))) = (&prepared, &outcome) {
            if let Write::ApplyArt { group, slot, .. } = &p.write {
                self.drafts.clear_uploaded(group, *slot);
            }
        }
        if matches!(outcome, Outcome::Done(crate::controller::groups::Done::Post(_))) {
            if let Some(open) = self.open.as_ref() {
                let id = open.id.clone();
                self.drafts.set_composer(&id, None);
            }
        }
        self.keep_drafts();
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

/// A refusal in the person's words for the write it refused: a 403 to an upload is no group role's (Major 4).
pub(crate) fn failure_text_for(write: Option<&Write>, f: Failure) -> String {
    match (write, f) {
        (Some(Write::Upload { .. }), Failure::Api(GroupFailure::Forbidden)) => tr("VRChat refused the upload: VRChat+ is needed, or your inventory is full.").to_string(),
        (Some(Write::ApplyArt { .. }), Failure::Api(GroupFailure::Forbidden)) => tr("VRChat refused the picture: your role, or the picture itself.").to_string(),
        _ => failure_text(f),
    }
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
        Failure::Expired => tr("This waited too long. Do it again to send it."),
        Failure::Duplicate => tr("This was already sent once."),
        Failure::Changed => tr("It changed on VRChat since you opened it. Look again before trying."),
        Failure::CheckFirst => tr("The last change here may have gone through. Check first."),
        Failure::NotHere => tr("Only the Orbiters window can do this."),
        Failure::TooLarge => tr("The picture is too large: 10 MB at most."),
    }
    .to_string()
}

pub(crate) fn why_text(why: Why) -> &'static str {
    match why {
        Why::NoPermission => tr("Your role does not include this right."),
        Why::RoleAtOrAbove => tr("This role is at or above your own highest role."),
        Why::FixedRole => tr("Everyone's role and the owner's role cannot be changed this way."),
        Why::PaidRole => tr("Roles bought in VRChat's store are not given here."),
        Why::OwnerRole => tr("The owner's role keeps every right."),
        Why::NotGrantable => tr("Your role cannot give this right."),
        Why::Prerequisite => tr("A right is missing the right it depends on."),
        Why::MissingData => tr("VRChat did not give enough about the roles to do this safely."),
        Why::NotSelfAssignable => tr("Members cannot take this role themselves."),
        Why::Escalation => tr("You can only give rights you hold yourself."),
        Why::NotEligible => tr("Uploading to your VRChat inventory needs VRChat+."),
        Why::NotYourPicture => tr("This picture is not one of your own VRChat pictures of that kind."),
    }
}

/// The writes of `group` whose outcome is not known, each with "Check VRChat", and, once a check could not settle it,
/// what it saw and "Let it go". Shown on the page (every tab) and in the studio.
pub(crate) fn uncertain_panel(ui: &mut Ui, ctx: &egui::Context, ctl: &Ctl, state: &mut State, group: &str) {
    let list = state.uncertain_of(group);
    let checking: Vec<String> = list.iter().filter(|u| state.jobs.running(&Kind::Check(u.op.clone()))).map(|u| u.op.clone()).collect();
    for u in list {
        let w = ui.available_width();
        let (what, more) = uncertain_words(&u);
        let text = format!("{what} {more}");
        let p = ui.painter().clone();
        let galley = p.layout_job(Txt::new(&text, 13.0).color(theme::FG).line_height(19.0).job(w - 300.0));
        let h = galley.size().y.max(20.0) + 28.0;
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        p.rect_filled(rect, theme::radius(14.0), theme::alpha(theme::WARN, 0.12));
        p.rect_stroke(rect, theme::radius(14.0), egui::Stroke::new(1.0, theme::alpha(theme::WARN, 0.45)), egui::StrokeKind::Inside);
        widgets::icon(&p, pos2(rect.left() + 22.0, rect.top() + 24.0), icons::WARNING, 17.0, theme::WARN);
        p.galley(pos2(rect.left() + 42.0, rect.top() + 14.0), galley, theme::FG);
        let check = Rect::from_min_size(pos2(rect.right() - 150.0, rect.top() + 10.0), vec2(136.0, 34.0));
        let busy = checking.contains(&u.op);
        let label = if busy { tr("Checking…") } else { tr("Check VRChat") };
        if widgets::Button::new(label).icon(icons::ARROW_CLOCKWISE).variant(widgets::Variant::Warn).size(widgets::Size::Sm).enabled(!busy).id_salt(("group-check", u.op.as_str())).show_at(ui, check).clicked() {
            state.check(ctx, ctl, &u.op);
        }
        if u.observation == Some(Observation::Inconclusive) {
            let release = Rect::from_min_size(pos2(rect.right() - 296.0, rect.top() + 10.0), vec2(138.0, 34.0));
            let r = widgets::Button::new(tr("Let it go")).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt(("group-release", u.op.as_str())).show_at(ui, release);
            if widgets::deliberate(&r) {
                state.release(ctx, ctl, &u.op);
            }
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

/// What an uncertain write was, and what is known of it.
fn uncertain_words(u: &Uncertain) -> (String, String) {
    let what = match &u.write {
        Write::MemberRole { add: true, .. } => tr("A role given to a member may or may not have been given."),
        Write::MemberRole { add: false, .. } => tr("A role taken back from a member may or may not have been taken back."),
        Write::CreateRole { .. } => tr("A new role may or may not have been created."),
        Write::EditRole { .. } => tr("A role change may or may not have been saved."),
        Write::DeleteRole { .. } => tr("A role may or may not have been deleted."),
        Write::CreatePost { .. } => tr("A post may or may not have been published."),
        Write::EditPost { .. } => tr("A post change may or may not have been saved."),
        Write::DeletePost { .. } => tr("A post may or may not have been deleted."),
        Write::Upload { .. } => tr("A picture may or may not have been uploaded to your inventory."),
        Write::ApplyArt { .. } => tr("The group's picture may or may not have been changed."),
    };
    let more = match &u.observation {
        None => tr("Its answer was lost. Nothing is sent here again until you check."),
        Some(Observation::Running) => tr("It is still being sent: check again in a moment."),
        Some(Observation::Inconclusive) => tr("The check could not tell: look on vrchat.com. If it is not there, let it go to do it again."),
        Some(_) => "",
    };
    (what.to_string(), more.to_string())
}

/// Home's Groups section, at its bottom: absent while VRChat is not signed in for certain, while it is read, and when
/// the account runs no group; "could not load" when reading failed (never "no groups", M1).
pub fn home_section(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    state.sync(cx);
    state.take(&ctx, cx);
    let Some(scope) = state.scope.clone() else { return };
    let now = anim::time(&ctx);
    let due = match &state.discovery {
        Discovery::Idle => true,
        Discovery::Ready(_, at) | Discovery::Failed(_, at) => now - at > READ_AGAIN,
        Discovery::Loading => false,
    };
    if due && !state.staged {
        if !matches!(state.discovery, Discovery::Ready(..)) {
            state.discovery = Discovery::Loading;
        }
        state.jobs.ask(&ctx, cx.ctl, &scope, Kind::Discover, Request::Discover);
    }
    let w = ui.available_width();
    match &state.discovery {
        Discovery::Ready(cards, _) if !cards.is_empty() => {
            let cards = cards.clone();
            caption(ui, w, &orbiters_i18n::tr!("Your groups · {n}", n = cards.len()));
            ui.add_space(-8.0);
            let cols = (((w + GAP) / (CARD_MIN_W + GAP)).floor() as usize).clamp(1, 3);
            let card_w = (w - GAP * (cols - 1) as f32) / cols as f32;
            let rows = cards.len().div_ceil(cols);
            let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (CARD_H + GAP) - GAP), Sense::hover());
            // Cards in view without their names yet are read, a few at a time (Major 7).
            let mut unnamed: Vec<String> = Vec::new();
            for (i, card) in cards.iter().enumerate() {
                let at = area.min + vec2((i % cols) as f32 * (card_w + GAP), (i / cols) as f32 * (CARD_H + GAP));
                let rect = Rect::from_min_size(at, vec2(card_w, CARD_H));
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                if !card.named && unnamed.len() < crate::controller::groups::HYDRATE_BATCH {
                    unnamed.push(card.id.clone());
                }
                match group_card(ui, cx, state, &scope, card, rect) {
                    Some(CardPress::Open) => state.open = Some(page::Open::new(card.clone(), &state.drafts)),
                    Some(CardPress::Studio) => state.studio = Some(studio::Studio::new(card.clone(), scope.vrchat_user().unwrap_or_default(), None, &state.drafts)),
                    None => {}
                }
            }
            if !unnamed.is_empty() && !state.staged && !state.jobs.busy_with(|k| *k == Kind::Hydrate) {
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
            widgets::text_truncated(&p, pos2(rect.left() + 46.0, rect.center().y), Align2::LEFT_CENTER, &Txt::new(line, 13.0).color(theme::FG_2), rect.width() - 190.0);
            let b = Rect::from_min_size(pos2(rect.right() - 132.0, rect.center().y - 17.0), vec2(116.0, 34.0));
            if widgets::Button::new(tr("Try again")).icon(icons::ARROW_CLOCKWISE).size(widgets::Size::Sm).id_salt("groups-retry").show_at(ui, b).clicked() {
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
    if state.studio.is_some() {
        studio::show(ui, cx, state);
    } else if state.open.is_some() {
        page::show(ui, cx, state);
    }
    confirmation(ui, cx, state);
    state.keep_drafts();
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) && state.flow.is_none() {
        if state.studio.is_some() {
            state.studio = None;
        } else {
            state.close();
        }
    }
}

/// The confirmation of a prepared write: exactly what goes, and who it reaches, in words (M7); Send commits that one
/// prepared write once, on a deliberate press (a held key's repeats never count, Blocker 6).
fn confirmation(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let Some(Flow::Confirming { prepared, serial, origin }) = &state.flow else { return };
    let (prepared, serial, origin) = (prepared.clone(), *serial, *origin);
    let id = Id::unique(("group-confirm", serial));
    let (title, action, danger) = confirm_words(&prepared);
    let mut send = false;
    let mut cancel = false;
    let names = match (state.open.as_ref(), state.studio.as_ref()) {
        (Some(o), _) => page::Names::of(o),
        (None, Some(s)) => page::Names::of_card(&s.card),
        _ => page::Names::default(),
    };
    let art = state.confirm_art.clone();
    let response = orbiters_ui::dialog::Dialog::new(id, origin, &title).width(520.0).glyph(if danger { icons::WARNING } else { icons::CHECK_CIRCLE }).footer(true).show(&ctx, |ui, part| match part {
        orbiters_ui::dialog::Part::Body => page::confirm_body(ui, &prepared, &names, art.as_ref()),
        orbiters_ui::dialog::Part::Footer => {
            let variant = if danger { widgets::Variant::Danger } else { widgets::Variant::Primary };
            if widgets::deliberate(&widgets::Button::new(&action).variant(variant).id_salt("group-confirm-send").show(ui)) {
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
        state.commit(&ctx, cx.ctl, prepared);
    } else if cancel || (response.closing && matches!(state.flow, Some(Flow::Confirming { .. }))) {
        orbiters_ui::dialog::close(&ctx, id);
        state.cancel(&ctx, cx.ctl, prepared.op);
    }
}

fn confirm_words(p: &Prepared) -> (String, String, bool) {
    let s = |a: &str, b: &str, d: bool| (a.to_string(), b.to_string(), d);
    match &p.write {
        Write::MemberRole { add: true, .. } => s(tr("Give this role?"), tr("Give the role"), false),
        Write::MemberRole { add: false, .. } => s(tr("Take this role back?"), tr("Take it back"), true),
        Write::CreateRole { .. } => s(tr("Create this role?"), tr("Create the role"), false),
        Write::EditRole { .. } => s(tr("Change this role?"), tr("Save the role"), false),
        Write::DeleteRole { .. } => s(tr("Delete this role?"), tr("Delete the role"), true),
        Write::CreatePost { .. } => s(tr("Post this?"), tr("Post"), false),
        Write::EditPost { .. } => s(tr("Save this post?"), tr("Save the post"), false),
        Write::DeletePost { .. } => s(tr("Delete this post?"), tr("Delete the post"), true),
        Write::Upload { tag: FileTag::Gallery, .. } => s(tr("Upload to your VRChat gallery?"), tr("Upload"), false),
        Write::Upload { tag: FileTag::Icon, .. } => s(tr("Upload to your VRChat icons?"), tr("Upload"), false),
        Write::ApplyArt { slot: orbiters_core::groups::ArtSlot::Banner, .. } => s(tr("Change the group's banner?"), tr("Change the banner"), false),
        Write::ApplyArt { slot: orbiters_core::groups::ArtSlot::Icon, .. } => s(tr("Change the group's icon?"), tr("Change the icon"), false),
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
    pub fn studio<'a>(state: &'a mut State, card: GroupCard, account: &str, source: image::RgbaImage) -> &'a mut studio::Studio {
        state.staged = true;
        state.studio = Some(studio::Studio::new(card, account, Some(source), &Drafts::default()));
        state.studio.as_mut().unwrap()
    }
    pub fn studio_empty(state: &mut State, card: GroupCard) {
        state.staged = true;
        state.studio = Some(studio::Studio::staged_empty(card));
    }
    pub fn confirming(state: &mut State, prepared: Prepared, origin: Rect) {
        state.serial += 1;
        state.flow = Some(Flow::Confirming { prepared, serial: state.serial, origin });
    }
    pub fn uncertain(state: &mut State, op: &str, write: Write, observation: Option<Observation>) {
        state.drafts.uncertain.push(Uncertain { op: op.into(), write, observation });
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
        for f in [Failure::Api(GroupFailure::Forbidden), Failure::Changed, Failure::Withheld(Why::RoleAtOrAbove), Failure::Lost] {
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
}
