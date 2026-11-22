//! VRChat groups in the window (REQUEST22 1): the Groups section at the bottom of Home (the groups the signed-in account
//! owns or helps run), each group's page (Look, Members, Roles, Posts) and the banner studio.
//!
//! What it shows is asked of the service on demand and answered to this window alone (`controller::groups`); nothing
//! of it is in the snapshot or on disk, except the studio's own crops and exports. Everything is bound to the account
//! scope it was asked under: another account (or "Connecting") drops it all, confirmations included (B3). Pictures of
//! groups and members are kept in memory only, a bounded number, asked for the rows in view (M10).
pub mod art;
mod page;
mod studio;
pub(crate) use page::{Composer, RoleEditor, Tab};

use super::Cx;
use crate::backend::Ctl;
use crate::controller::groups::{Failure, Outcome, Prepared, Reply, Request};
use crate::controller::vrchat_data::Scope;
use crate::controller::vrchat_login::Presence;
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
    Open(String),
    Members { group: String, query: String, offset: usize },
    Posts { group: String, offset: usize },
    Inventory { tag: FileTag, offset: usize },
    Picture(String),
    /// A picture of the account's gallery read in full for the banner studio.
    Source(String),
    Prepare,
    Commit,
    Check(String),
    Cancel,
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
/// page closed) makes every answer still on its way obsolete.
pub(crate) struct Jobs {
    tx: mpsc::Sender<Answer>,
    rx: mpsc::Receiver<Answer>,
    generation: u64,
    running: Vec<Kind>,
}

impl Default for Jobs {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx, generation: 1, running: Vec::new() }
    }
}

impl Jobs {
    fn running(&self, kind: &Kind) -> bool {
        self.running.contains(kind)
    }
    fn pictures_running(&self) -> usize {
        self.running.iter().filter(|k| matches!(k, Kind::Picture(_))).count()
    }
    /// Asks `request` unless the same is on its way.
    pub(crate) fn ask(&mut self, ctx: &egui::Context, ctl: &Ctl, scope: &Scope, kind: Kind, request: Request) {
        if self.running(&kind) && !matches!(kind, Kind::Prepare | Kind::Commit | Kind::Cancel) {
            return;
        }
        self.running.push(kind.clone());
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
            if let Some(i) = self.running.iter().position(|k| *k == a.kind) {
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
        self.running.clear();
    }
}

/// A picture as a texture's pixels, at most 640 px a side (cards and rows are smaller).
fn decode_picture(bytes: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width() > 640 || img.height() > 640 { img.thumbnail(640, 640) } else { img };
    let rgba = img.to_rgba8();
    Some(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

enum Pic {
    Waiting,
    Ready(egui::TextureHandle),
    Failed,
}

/// Group and member pictures, in memory only, the newest [`PICTURES_MAX`] (M10).
#[derive(Default)]
pub(crate) struct Pictures {
    map: HashMap<String, Pic>,
    order: VecDeque<String>,
}

impl Pictures {
    /// The texture of `url` if it is here; asked for when it is not (a few at a time).
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

/// A write on its way: prepared, waiting for the person's yes, sent, or ended.
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
    /// Back to Home (the sidebar's Home).
    pub fn close(&mut self) {
        self.open = None;
        self.studio = None;
        self.flow = None;
        self.jobs.reset();
    }
    fn signed(cx: &Cx) -> Option<Scope> {
        (cx.snap.vrchat_login.presence() == Presence::SignedIn).then(|| Scope::of(cx.snap))
    }
    /// Another account, the environment, or not signed in for certain: everything goes (B3).
    fn sync(&mut self, cx: &Cx) {
        if self.staged {
            return;
        }
        let now = Self::signed(cx);
        if now != self.scope {
            self.scope = now;
            self.jobs.reset();
            self.pictures.clear();
            self.discovery = Discovery::Idle;
            self.open = None;
            self.studio = None;
            self.flow = None;
        }
    }
    /// Takes what came back.
    fn take(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        for a in self.jobs.take() {
            match (a.kind, a.reply) {
                (Kind::Discover, Reply::Discovered(cards)) => self.discovery = Discovery::Ready(cards, anim::time(ctx)),
                (Kind::Discover, Reply::Failed(f)) => self.discovery = Discovery::Failed(f, anim::time(ctx)),
                (Kind::Picture(url), _) => self.pictures.arrived(ctx, url, a.image),
                (Kind::Source(_), Reply::Cancelled) => {
                    if let Some(studio) = self.studio.as_mut() {
                        studio.source_arrived(a.source);
                    }
                }
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

    /// Prepares a write; its confirmation (when it needs one) grows out of `origin`.
    pub(crate) fn prepare(&mut self, ctx: &egui::Context, ctl: &Ctl, write: crate::controller::groups::Write, origin: Rect) {
        let Some(scope) = self.scope.clone() else { return };
        if self.flow.is_some() {
            return;
        }
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
            (Kind::Prepare, Reply::Failed(f)) => {
                self.flow = None;
                self.ended(ctx, cx, None, Outcome::Refused(f));
            }
            (Kind::Commit, Reply::Outcome(outcome)) => {
                let prepared = match self.flow.take() {
                    Some(Flow::Sending { prepared }) => Some(prepared),
                    _ => None,
                };
                self.ended(ctx, cx, prepared, outcome);
            }
            (Kind::Commit, Reply::Failed(f)) => {
                // Refused before it was looked at (the account changed): nothing was sent.
                self.flow = None;
                self.ended(ctx, cx, None, Outcome::Refused(f));
            }
            _ => {}
        }
    }
    /// How a write ended: the page takes VRChat's answer as the state; refusals and unknowns stay on the page. Toasts
    /// carry no names (M10).
    fn ended(&mut self, ctx: &egui::Context, cx: &mut Cx, prepared: Option<Prepared>, outcome: Outcome) {
        if let Outcome::Done(done) = &outcome {
            cx.toasts.push(ctx, orbiters_ui::toast::Kind::Success, done_text(done));
        }
        if let Some(studio) = self.studio.as_mut() {
            if studio.ended(prepared.as_ref(), &outcome) {
                return;
            }
        }
        if let Some(open) = self.open.as_mut() {
            open.ended(prepared.as_ref(), outcome);
        }
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

/// A failure in the person's words (local codes only: nothing of VRChat's text, M5, M10).
pub(crate) fn failure_text(f: Failure) -> String {
    match f {
        Failure::Api(GroupFailure::Forbidden) => tr("Your role in this group does not allow this."),
        Failure::Api(GroupFailure::NotFound) => tr("VRChat does not show this to your account any more."),
        Failure::Api(GroupFailure::Invalid) => tr("VRChat did not take it as it is. Check what you entered."),
        Failure::Api(GroupFailure::RateLimited(s)) => return orbiters_i18n::tr!("VRChat asks to wait. Try again in {seconds} s.", seconds = s),
        Failure::Api(GroupFailure::Unavailable) => tr("VRChat could not be reached. Try again shortly."),
        Failure::Api(GroupFailure::Unknown) => tr("VRChat may have done it: its answer was lost. Check before trying again."),
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
    }
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
            for (i, card) in cards.iter().enumerate() {
                let at = area.min + vec2((i % cols) as f32 * (card_w + GAP), (i / cols) as f32 * (CARD_H + GAP));
                let rect = Rect::from_min_size(at, vec2(card_w, CARD_H));
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                match group_card(ui, cx, state, &scope, card, rect) {
                    Some(CardPress::Open) => state.open = Some(page::Open::new(card.clone())),
                    Some(CardPress::Studio) => state.studio = Some(studio::Studio::new(card.clone(), scope.vrchat_user().unwrap_or_default(), None)),
                    None => {}
                }
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
    if let Some(t) = &banner {
        paint::squircle_texture(&p, r, radii, t.id(), super::launcher::cover_uv(t.size_vec2(), r.size()), Color32::WHITE);
    } else {
        let tint = widgets::name_color(&card.name);
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
        None => widgets::avatar(&p, icon_rect, None, &card.name),
    }
    let tx = icon_rect.right() + 14.0;
    let room = r.right() - 56.0 - tx;
    let name = if card.name.is_empty() { tr("A group you help run") } else { card.name.as_str() };
    widgets::text_truncated(&p, pos2(tx, icon_rect.top() + 14.0), Align2::LEFT_CENTER, &Txt::new(name, 17.0).weight(weight::SEMIBOLD), room);
    let code = short_code(&card.short_code, &card.discriminator);
    let members = orbiters_i18n::tr!("{n} members", n = card.member_count);
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
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) && state.flow.is_none() {
        if state.studio.is_some() {
            state.studio = None;
        } else {
            state.close();
        }
    }
}

/// The confirmation of a prepared write: exactly what goes, and who it reaches, in words (M7); Send commits that one
/// prepared write once.
fn confirmation(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let Some(Flow::Confirming { prepared, serial, origin }) = &state.flow else { return };
    let (prepared, serial, origin) = (prepared.clone(), *serial, *origin);
    let id = Id::unique(("group-confirm", serial));
    let (title, action, danger) = confirm_words(&prepared);
    let mut send = false;
    let mut cancel = false;
    let names = state.open.as_ref().map(page::Names::of).unwrap_or_default();
    let response = orbiters_ui::dialog::Dialog::new(id, origin, &title).width(520.0).glyph(if danger { icons::WARNING } else { icons::CHECK_CIRCLE }).footer(true).show(&ctx, |ui, part| match part {
        orbiters_ui::dialog::Part::Body => page::confirm_body(ui, &prepared, &names),
        orbiters_ui::dialog::Part::Footer => {
            let variant = if danger { widgets::Variant::Danger } else { widgets::Variant::Primary };
            if widgets::Button::new(&action).variant(variant).id_salt("group-confirm-send").show(ui).clicked() {
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
    use crate::controller::groups::Write;
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
        let mut open = page::Open::new(card);
        open.answer(Kind::Open(open.id.clone()), Reply::Opened(Box::new(page)));
        state.open = Some(open);
        state.open.as_mut().unwrap()
    }
    pub fn studio<'a>(state: &'a mut State, card: GroupCard, account: &str, source: image::RgbaImage) -> &'a mut studio::Studio {
        state.staged = true;
        state.studio = Some(studio::Studio::new(card, account, Some(source)));
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M9: an answer asked under an earlier generation (another account, a page closed) is dropped, never shown.
    #[test]
    fn obsolete_answers_are_dropped() {
        let mut jobs = Jobs::default();
        jobs.running.push(Kind::Discover);
        let old = jobs.generation;
        jobs.reset();
        jobs.tx.send(Answer { generation: old, kind: Kind::Discover, reply: Reply::Discovered(vec![GroupCard::default()]), image: None, source: None }).unwrap();
        assert!(jobs.take().is_empty(), "a reply to the old account is not taken");
        jobs.tx.send(Answer { generation: jobs.generation, kind: Kind::Discover, reply: Reply::Discovered(vec![]), image: None, source: None }).unwrap();
        assert_eq!(jobs.take().len(), 1);
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
        for f in [Failure::Api(GroupFailure::Forbidden), Failure::Changed, Failure::Withheld(Why::RoleAtOrAbove)] {
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
