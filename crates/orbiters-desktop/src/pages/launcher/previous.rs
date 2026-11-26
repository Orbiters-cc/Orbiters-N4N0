//! Previous instances in Where (REQUEST21 7: "In the Where area, add a button to see all the previous instances so the
//! user can launch VRChat on an instance he was previously in."). Where's heading has a Previous instances button; it
//! opens a sheet over Home with every instance of this computer's history (`visited-worlds.json`, the tray service's
//! record of VRChat's logs: the store N4N0's Previous instances and Where's last instance read), newest first, each as
//! a row of Where's own look (the world's picture, its name, the instance's kind and region, when), searched by the
//! world's name, read when the sheet opens and laid out only for the rows in view (their pictures asked for then).
//!
//! Choosing a row makes it Where's destination, with the existing rule: VRChat is asked whether the instance is still
//! open (the overlay's own read, [`Request::Instance`], nothing sent from the account), and only an open one whose kind
//! lets the account in by its link is rejoined; the row says what VRChat said. Closed (or not known, or an invite
//! instance of someone else's), the row offers a new instance of its world, its kind chosen there as on Where's cards.
//! Nothing starts VRChat here: the choice is Where's selected card ("Previous instance · 2 d ago"), Play starts it.
//! Signed out of VRChat (or on another account's profile), the list still shows, and a row is a new public instance
//! of its world, as Where says. Escape, the close button or a press beside the sheet put it away; the choice stays.
//!
//! [`Request::Instance`]: crate::controller::vrchat_data::Request::Instance

use orbiters_core::app_log::OrLog as _;
use orbiters_core::locks::*;
use super::destinations::{joinable_without_asking, kind_access, kinds, rejoinable, Places, Tone};
use super::selection::{Asker, Destination, Offer, Origin, Outcome, Verified};
use super::{access_icon, access_name, cover_uv, region_tag, Cx, State};
use crate::controller::vrchat_data::{Request, Response};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Key, LayerId, Order, Pos2, Rect, Sense, Ui, UiBuilder};
use orbiters_core::launcher::{InstanceAccess, LaunchMode};
use orbiters_i18n::{tr, tr_n};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, paint, popup, widgets};
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

/// The sheet's size at most, a row's height, its picture.
const W: f32 = 660.0;
const H: f32 = 680.0;
const ROW_H: f32 = 72.0;
const PIC_W: f32 = 96.0;
const RADIUS: f32 = 12.0;

/// An instance of the history.
#[derive(Clone, Debug, PartialEq)]
pub struct Visit {
    pub world_id: String,
    /// What follows the world's id (`12345~hidden(usr_…)~region(eu)`).
    pub instance: String,
    /// The world's name, when its join said it.
    pub name: String,
    /// When it was joined last (Unix seconds).
    pub at: u64,
}

impl Visit {
    /// Its key as Where's card: `previous:<world>:<instance>`.
    pub fn key(&self) -> String {
        format!("previous:{}:{}", self.world_id, self.instance)
    }

    pub fn location(&self) -> String {
        format!("{}:{}", self.world_id, self.instance)
    }

    /// Its kind, as VRChat's location says.
    pub fn access(&self) -> InstanceAccess {
        orbiters_core::launcher_feed::access(&self.instance).0
    }

    /// Its region tag (`eu`, `us`…), when the location says.
    pub fn region(&self) -> String {
        orbiters_core::launcher_feed::access(&self.instance).1
    }
}

/// The history's instances, newest first: those VRChat's own ids name.
pub fn visits(history: &orbiters_core::visited::History) -> Vec<Visit> {
    history
        .recent_instances()
        .into_iter()
        .filter(|(v, _)| orbiters_core::vrchat_launch::is_destination(&v.world_id, &v.instance))
        .map(|(v, name)| Visit { world_id: v.world_id, instance: v.instance, name: name.unwrap_or_default(), at: v.at })
        .collect()
}

pub use super::selection::OPEN_FOR;

/// The sheet's own state, in Where's.
#[derive(Default)]
pub struct Previous {
    /// Up since (egui's clock), and closing since.
    up: Option<(f64, Option<f64>)>,
    query: String,
    /// The text searched for at the last frame ([`crate::pages::new_search`]).
    searched: Option<String>,
    /// Which of the rows the search finds: the rows and the text they were looked for in.
    found: Option<(Arc<[Visit]>, String, Arc<[usize]>)>,
    /// The history's instances, read when the sheet opens (pictures of the page hand them in), and whether a read is
    /// under way: one lock, so a read finishing while the sheet is closed is never lost (verify-r21 Major 5).
    history: Arc<Mutex<HistoryRead>>,
    /// How the history is read (tests: a stand-in; the app: this computer's file).
    reader: Option<Reader>,
    /// The rows were handed in (nothing is read), and nothing is asked of VRChat or loaded either (pictures of the page).
    staged: bool,
    quiet: bool,
    /// The worlds' pictures this sheet may show: only those already kept on this computer (Major 6), looked up once.
    kept: std::collections::HashMap<String, Option<String>>,
    /// Whether VRChat makes instances for the account Where's places are for, this frame (Where's row says; who it is, is
    /// the selection's own asker).
    pub(super) can_create: bool,
    pub(super) signed_in_elsewhere: bool,
    /// VRChat's log cannot be read here: nothing private goes to a VRChat.
    pub(super) log: Option<orbiters_core::launcher::LogTrouble>,
    /// VRChat's saved session is being restored: who is signed in is not known yet (Major 1).
    pub(super) connecting: bool,
    /// Because the saved Orbiters account cannot be read now.
    pub(super) unreadable: bool,
}

impl Previous {
    /// What is said while it connects.
    pub(super) fn connecting_text(&self) -> &'static str {
        crate::state::AccountStatus { unreadable: self.unreadable, ..Default::default() }.connecting()
    }
}

type Reader = Arc<dyn Fn() -> Vec<Visit> + Send + Sync>;

#[derive(Default)]
struct HistoryRead {
    rows: Option<Arc<[Visit]>>,
    reading: bool,
}

impl Previous {
    /// Whether nothing is asked or loaded (pictures of the page).
    pub fn is_staged(&self) -> bool {
        self.quiet
    }

    /// Whether the sheet is up.
    pub fn is_up(&self) -> bool {
        self.up.is_some_and(|(_, closing)| closing.is_none())
    }

    /// Pictures of the page and tests: the history's instances handed in (nothing read), the sheet up with `query`.
    pub fn stand_in(&mut self, rows: Vec<Visit>, query: &str) {
        self.history.locked().rows = Some(rows.into());
        self.staged = true;
        self.quiet = true;
        self.query = query.into();
        self.up = Some((0.0, None));
    }

    /// Tests: the history's instances handed in (nothing read); a row picked is asked of the backend as on the app.
    #[cfg(test)]
    pub fn stand_in_rows(&mut self, rows: Vec<Visit>) {
        self.history.lock().unwrap_or_else(|e| e.into_inner()).rows = Some(rows.into());
        self.staged = true;
    }

    /// Tests: the history read by `reader` (on its thread, as the app reads its file), nothing handed in.
    #[cfg(test)]
    pub fn stand_in_reader(&mut self, reader: impl Fn() -> Vec<Visit> + Send + Sync + 'static) {
        self.reader = Some(Arc::new(reader));
        self.staged = false;
    }

    /// Pictures of the page: the sheet put away (the choice kept).
    pub fn stand_in_close(&mut self) {
        self.up = None;
    }

    fn open(&mut self, clock: f64) {
        self.up = Some((clock, None));
        self.query.clear();
        if !self.staged {
            // Read again (it changes as VRChat runs), unless a read is under way: its rows come.
            let mut h = self.history.locked();
            if !h.reading {
                h.rows = None;
            }
        }
    }

    fn close(&mut self, clock: f64) {
        if let Some((_, closing)) = self.up.as_mut() {
            closing.get_or_insert(clock);
        }
    }
}

/// Where's heading button: opens the sheet.
pub(super) fn button(ui: &mut Ui, state: &mut State, right_center: Pos2) -> f32 {
    let b = widgets::Button::new(tr("Previous instances"))
        .icon(icons::CLOCK_COUNTER_CLOCKWISE)
        .variant(widgets::Variant::Ghost)
        .size(widgets::Size::Sm)
        .id_salt("where-previous")
        .hint(tr("Every instance you were in: choose one to start VRChat there"));
    let w = b.desired_width(ui);
    let r = Rect::from_min_size(pos2(right_center.x - w, right_center.y - 16.0), vec2(w, 32.0));
    if widgets::pressed_fresh(&b.show_at(ui, r)) {
        let clock = anim::time(ui.ctx());
        state.places.previous.open(clock);
    }
    w
}

/// What the selection waits for: the answers that came (Play's own among them), a friend who left the instance chosen,
/// and an instance chosen before who is signed in was known (or chosen for another account or profile than now's): asked
/// about as soon as there is someone to ask for, or given up when it takes this account no more.
pub(super) fn follow(state: &mut State, cx: &Cx, clock: f64) {
    // Who left comes first: an answer that came with the news of it starts nothing there (verify3-r21 M3).
    friend_left(state, clock);
    for outcome in state.places.desk.take(Instant::now()) {
        match outcome {
            Outcome::Launch { visit, mode, profile, account } => super::destinations::enter(state, cx, clock, &visit, mode, profile, account),
            Outcome::MakeFailed(e) => state.places.notice = Some((e, clock)),
        }
    }
    let desk = &state.places.desk;
    let selection = desk.selection();
    if desk.is_quiet() || *selection.verified() != Verified::Unverified {
        return;
    }
    let (Some(visit), Some(me)) = (selection.instance().cloned(), desk.asker().account.clone()) else { return };
    if joins(state, &visit, selection.origin(), &me) {
        check(state, cx, None);
    } else {
        // Another account's now, which its link does not let in.
        state.places.desk.go_home();
        state.places.notice = Some((orbiters_i18n::tr!("{name} is no longer available: Play starts VRChat at home.", name = world_name(&visit)), clock));
    }
}

/// Whether `me` goes into it by its link alone: one of the history (its owner was a friend then), a friend's by the
/// friends of now.
fn joins(state: &State, visit: &Visit, origin: &Origin, me: &str) -> bool {
    match origin {
        Origin::Friend { .. } => joinable_without_asking(&visit.instance, me, &|id| state.has_friend(id)),
        _ => rejoinable(&visit.instance, me),
    }
}

/// A friend's instance (REQUEST23): it goes when the friend left it or went offline; the choice goes back to Home and the
/// line says so, as when any chosen place goes. A list that is not the account's own says nothing of who left.
fn friend_left(state: &mut State, clock: f64) {
    let Some(Offer { visit, origin: Origin::Friend { id, name, .. } }) = state.places.desk.offer().cloned() else { return };
    let found = state.data.friends.iter().find(|f| f.id == id);
    let there = found.is_some_and(|f| {
        f.presence != orbiters_core::launcher::Presence::Offline
            && matches!(&f.whereabouts, orbiters_core::launcher::Whereabouts::World(pl) if pl.world_id == visit.world_id && pl.instance_id == visit.instance)
    });
    let listed = found.is_some() || matches!(state.data.source, orbiters_core::launcher::Source::SignedIn { .. });
    if there || !listed || state.stale() || state.places.desk.is_quiet() {
        return;
    }
    state.places.desk.withdraw_offer();
    if state.places.desk.selection().instance() == Some(&visit) {
        state.places.desk.go_home();
        state.places.notice = Some((orbiters_i18n::tr!("{name} has left that instance: Play starts VRChat at home.", name = name), clock));
    }
}

/// What the line under Where says of a previous instance or a friend's that is chosen and not said open: being asked
/// about, closed, not known, nobody to ask for. (What can be had instead is the button at its end, [`alternatives`].)
pub(super) fn line(places: &Places) -> Option<(String, Tone)> {
    let selection = places.desk.selection();
    let visit = selection.instance()?;
    let friend = match selection.origin() {
        Origin::Friend { name, .. } => Some(name.as_str()),
        _ => None,
    };
    let nobody = places.desk.asker().account.is_none();
    let world = world_name(visit);
    Some(match (selection.verified(), friend) {
        (Verified::Open { .. }, _) => return None,
        (Verified::Closed, Some(name)) => (orbiters_i18n::tr!("{name}'s instance is closed now: choose another place.", name = name), Tone::Warn),
        (Verified::Closed, None) => (orbiters_i18n::tr!("{name} is closed now: choose another place.", name = world), Tone::Warn),
        (Verified::Unverified, Some(name)) if nobody && places.previous.connecting => (orbiters_i18n::tr!("Connecting to VRChat: {name}'s instance is checked next…", name = name), Tone::Busy),
        (Verified::Unverified, None) if nobody && places.previous.connecting => (places.previous.connecting_text().into(), Tone::Busy),
        (Verified::Unverified | Verified::Unknown, _) if nobody => (tr("VRChat is not signed in for this profile: choose another place.").into(), Tone::Warn),
        (Verified::Unknown, Some(name)) => (orbiters_i18n::tr!("VRChat did not say whether {name}'s instance is open: Play asks again.", name = name), Tone::Warn),
        (Verified::Unknown, None) => (orbiters_i18n::tr!("VRChat did not say whether {name} is open: Play asks again.", name = world), Tone::Warn),
        (Verified::Unverified | Verified::Checking { .. }, Some(name)) => (orbiters_i18n::tr!("Asking VRChat whether {name}'s instance is open…", name = name), Tone::Busy),
        (Verified::Unverified | Verified::Checking { .. }, None) => (tr("Asking VRChat…").into(), Tone::Busy),
    })
}

/// Whether the chosen instance cannot be gone to now and a new one of its world is offered instead.
pub(super) fn alternatives(places: &Places) -> bool {
    let selection = places.desk.selection();
    let nobody = places.desk.asker().account.is_none() && !places.previous.connecting;
    selection.instance().is_some()
        && match selection.verified() {
            Verified::Closed | Verified::Unknown => true,
            Verified::Unverified => nobody,
            _ => false,
        }
}

/// The chosen instance's world, a new instance of it.
pub(super) fn take_new_of_chosen(state: &mut State) {
    let selection = state.places.desk.selection();
    let Some(visit) = selection.instance().cloned() else { return };
    // Of the last instance's card too: from here on it is a choice of its own, its card right after Home.
    let origin = match selection.origin().clone() {
        Origin::Recent | Origin::Home | Origin::Card => Origin::Previous,
        origin => origin,
    };
    take_new(state, &visit, origin);
}

/// A row pressed (or the card of one chosen before): kept until VRChat says who is signed in (while it connects), asked
/// of VRChat where it can be rejoined, else taken at once as a new instance of its world. It is what is chosen from this
/// press on: Play goes nowhere else (verify2-r21 M3).
fn pick(state: &mut State, cx: &Cx, visit: &Visit, origin: Origin) {
    let Asker { account, .. } = state.places.desk.asker().clone();
    let joinable = match &account {
        Some(me) => joins(state, visit, &origin, me),
        None => state.places.previous.connecting,
    };
    if !joinable {
        // Signed out, another account's profile, or an instance only an invite lets in: a new one of its world.
        return take_new(state, visit, origin);
    }
    // The same one again, proven or being asked: it is so already.
    let same = state.places.desk.selection().instance() == Some(visit);
    if same && matches!(state.places.desk.selection().verified(), Verified::Open { .. }) && state.places.desk.busy().is_none() {
        return;
    }
    state.places.desk.choose(Destination::Instance { key: visit.key(), visit: visit.clone() }, origin);
    state.places.page = 0;
    state.places.notice = None;
    if account.is_some() {
        check(state, cx, None);
    }
}

/// The card of a previous instance or a friend's pressed.
pub(super) fn choose_offered(state: &mut State, cx: &Cx, new: bool) {
    let Some(Offer { visit, origin }) = state.places.desk.offer().cloned() else { return };
    let chosen = state.places.desk.selection().destination();
    if new {
        if !matches!(chosen, Destination::NewOf { visit: v, .. } if *v == visit) {
            take_new(state, &visit, origin);
        }
    } else {
        pick(state, cx, &visit, origin);
    }
}

/// Tests: the worker asking about this instance stands still, its answer in hand, until told (verify2-r21 M4).
#[cfg(test)]
pub(in crate::pages) static PAUSE: Mutex<Option<(String, std::sync::mpsc::Receiver<()>)>> = Mutex::new(None);

#[cfg(test)]
fn pause_before_publishing(instance: &str) {
    let held = {
        let mut p = PAUSE.lock().unwrap();
        match p.as_ref() {
            Some((i, _)) if i == instance => p.take(),
            _ => None,
        }
    };
    if let Some((_, held)) = held {
        let _ = held.recv_timeout(Duration::from_secs(3));
    }
}

/// Asks VRChat whether the chosen instance is open ([`super::selection::Desk::check`]), for the press `play` when Play is
/// what asks. The answer, or its absence, comes back with what it was asked under.
pub(super) fn check(state: &mut State, cx: &Cx, play: Option<LaunchMode>) {
    if state.places.desk.is_quiet() {
        return;
    }
    let Some(ticket) = state.places.desk.check(play, Instant::now()) else { return };
    let ctx = state.ctx.clone();
    let Some(scope) = state.scope.clone() else {
        // Not asked: "not known" (the ticket says so as it goes), read on the next frame.
        drop(ticket);
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
        return;
    };
    let ctl = cx.ctl.clone();
    // Play goes by what VRChat says now; a choice, by an answer good for a while yet.
    let max_age_ms = if play.is_some() { 2_000 } else { 30_000 };
    // A read into memory (whether VRChat says the instance is open): nothing of the window's work, unless Play waits for
    // it: the window does not rest to the tray in the middle of that.
    let working = play.map(|_| crate::pages::Working::begin());
    std::thread::Builder::new().name("where-previous-open".into()).spawn(move || {
        let _working = working;
        let (world, instance) = (ticket.visit.world_id.clone(), ticket.visit.instance.clone());
        #[cfg(test)]
        let paused = instance.clone();
        let asked = Instant::now();
        let (open, at) = said(asked, ctl.vrchat_data(scope, Request::InstanceSeen { world, instance, max_age_ms }));
        #[cfg(test)]
        pause_before_publishing(&paused);
        ticket.answer(open, at);
        if let Some(ctx) = ctx {
            anim::wake(&ctx);
        }
    }).or_log("a thread could not be started");
}

/// What VRChat said of an instance asked about at `asked`, and when it said so: the service may hand on an answer it kept,
/// as old as it says (verify3-r21 M2); never counted younger than the asking.
pub(super) fn said(asked: Instant, answer: crate::controller::vrchat_data::Result) -> (Option<bool>, Instant) {
    match answer {
        Ok(Response::InstanceSeen { open, age_ms }) => (open, asked.checked_sub(std::time::Duration::from_millis(age_ms)).unwrap_or(asked)),
        _ => (None, asked),
    }
}

/// Its world taken as a new instance.
fn take_new(state: &mut State, visit: &Visit, origin: Origin) {
    state.places.desk.choose(Destination::NewOf { key: visit.key(), visit: visit.clone() }, origin);
    state.places.page = 0;
    state.places.notice = None;
}

/// Whether a friend's row or profile offers Select instead of Request (REQUEST23): Where shows (VRChat does not run on
/// the profile chosen), the friend is in an instance named by VRChat's own ids, and it takes the account without asking
/// ([`super::destinations::joinable_without_asking`], with the account's friends for a friends-only one; while VRChat
/// connects, the account is not known: its own invite instances are not offered). Their location as the list gives it.
pub(super) fn friend_selectable(state: &State, friend: &orbiters_core::launcher::Friend) -> Option<orbiters_core::launcher::Place> {
    use orbiters_core::launcher::{Presence, Whereabouts};
    if !state.places.showing || friend.presence == Presence::Offline {
        return None;
    }
    let Whereabouts::World(place) = &friend.whereabouts else { return None };
    if !orbiters_core::vrchat_launch::is_destination(&place.world_id, &place.instance_id) {
        return None;
    }
    // Signed out, or a profile of another account: nothing can be asked of VRChat for it; Request as before.
    let me = match state.places.desk.asker().account.as_deref() {
        Some(me) => me,
        None if state.places.previous.connecting => "",
        None => return None,
    };
    joinable_without_asking(&place.instance_id, me, &|id| state.has_friend(id)).then(|| place.clone())
}

/// Select on a friend's row or profile (REQUEST23): their instance becomes Where's destination, nothing sent to VRChat.
/// The same path as a previous instance: VRChat is asked whether it is still open (held while VRChat connects), it is
/// chosen once it is, Play starts VRChat there (asked again if that answer is over a minute old). `place` is the friend's
/// location as the friends list gives it; anything that is not VRChat's own ids is refused.
pub(super) fn select_friend(state: &mut State, cx: &Cx, friend_id: &str, friend_name: &str, place: &orbiters_core::launcher::Place) {
    if !orbiters_core::vrchat_launch::is_destination(&place.world_id, &place.instance_id) {
        return;
    }
    let visit = Visit { world_id: place.world_id.clone(), instance: place.instance_id.clone(), name: place.world_name.clone(), at: state.now().max(0) as u64 };
    let picture = (!place.world_picture.is_empty()).then(|| place.world_picture.clone());
    let origin = Origin::Friend { id: friend_id.to_string(), name: friend_name.to_string(), picture };
    // Not joinable any more (the list changed under the press): nothing is chosen.
    if state.places.desk.asker().account.clone().is_some_and(|me| !joins(state, &visit, &origin, &me)) {
        return;
    }
    pick(state, cx, &visit, origin);
    state.places.reveal = true;
}

fn world_name(v: &Visit) -> String {
    if v.name.trim().is_empty() { tr("Unnamed world").into() } else { v.name.clone() }
}

/// The sheet over Home (`page`), while it is up or closing.
pub(super) fn sheet(ui: &mut Ui, cx: &mut Cx, state: &mut State, page: Rect) {
    let Some((opened, closing)) = state.places.previous.up else { return };
    let ctx = ui.ctx().clone();
    let clock = anim::time(&ctx);
    let key = Id::unique(("where-previous-t", opened.to_bits()));
    let t = anim::spring_from(&ctx, key, [0.0], [if closing.is_some() { 0.0 } else { 1.0 }], Spring::SOFT)[0];
    if closing.is_some() && t < 0.03 {
        state.places.previous.up = None;
        return;
    }
    // The history, read once the sheet opens (tests hand a reader in, or the rows; pictures of the page the rows), on a
    // thread that leaves its rows and the end of its read together.
    let reader: Option<Reader> = match &state.places.previous.reader {
        Some(r) => Some(r.clone()),
        None if cfg!(test) => None,
        None => Some(Arc::new(|| visits(&orbiters_core::visited::History::load(&orbiters_core::visited::history_file())))),
    };
    if let (false, Some(reader)) = (state.places.previous.staged, reader) {
        let start = {
            let mut h = state.places.previous.history.locked();
            let start = h.rows.is_none() && !h.reading;
            h.reading |= start;
            start
        };
        if start {
            let (history, ctx2) = (state.places.previous.history.clone(), ctx.clone());
            let started = std::thread::Builder::new().name("where-previous-history".into()).spawn(move || {
                let rows = reader();
                let mut h = history.locked();
                h.rows = Some(rows.into());
                h.reading = false;
                drop(h);
                anim::wake(&ctx2);
            });
            if started.is_err() {
                let mut h = state.places.previous.history.locked();
                h.rows = Some(Vec::new().into());
                h.reading = false;
            }
        }
    }
    let layer = LayerId::new(Order::Foreground, Id::unique("where-previous-layer"));
    orbiters_ui::front::layer(&ctx, layer, closing.is_none());
    let mut over = ui.new_child(UiBuilder::new().max_rect(page).layer_id(layer));
    let scrim = over.interact(page, Id::unique("where-previous-scrim"), Sense::click());
    over.painter().rect_filled(page, 0.0, theme::black(0.5 * t.clamp(0.0, 1.0)));
    let area = Rect::from_min_max(pos2(page.left(), page.top() + cx.top), page.max);
    let w = W.min(area.width() - 48.0);
    let h = H.min(area.height() - 40.0);
    let rest = Rect::from_center_size(area.center(), vec2(w, h));
    let rect = rest.translate(vec2(0.0, 24.0 * (1.0 - t)));
    let mut panel = over.new_child(UiBuilder::new().max_rect(rect).id_salt("where-previous-panel"));
    panel.multiply_opacity(t.clamp(0.0, 1.0));
    panel.set_clip_rect(rect.intersect(ui.clip_rect()));
    paint::shadow(panel.painter(), rect, 22.0, vec2(0.0, 18.0), 44.0, -6.0, theme::black(0.55));
    // Opaque under its surface: nothing of Home shows through the list.
    widgets::frame(panel.painter(), rect, 22.0, theme::ink_900());
    widgets::frame(panel.painter(), rect, 22.0, theme::surface());
    paint::stroke_squircle(panel.painter(), rect, theme::squircle(22.0), theme::hairline());
    let inner = rect.shrink2(vec2(22.0, 20.0));

    // The head: its name, what it is, and close.
    let p = panel.painter().clone();
    widgets::text(&p, pos2(inner.left(), inner.top() + 14.0), Align2::LEFT_CENTER, &Txt::new(tr("Previous instances"), 20.0).weight(weight::SEMIBOLD).tracking(-0.01));
    let rows = state.places.previous.history.locked().rows.clone();
    let sub = match &rows {
        Some(rows) if !rows.is_empty() => tr_n("{count} instance you were in, newest first", "{count} instances you were in, newest first", rows.len() as u64),
        _ => tr("Instances you were in").into(),
    };
    widgets::text_truncated(&p, pos2(inner.left(), inner.top() + 40.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.5).color(theme::fg_3()), inner.width() - 60.0);
    let close_r = Rect::from_min_size(pos2(inner.right() - 32.0, inner.top() - 2.0), vec2(32.0, 32.0));
    let close = widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Close")).size(32.0, 14.0).id_salt("where-previous-close").show_at(&mut panel, close_r));

    // The search, and what the sign-in allows.
    let search_r = Rect::from_min_size(pos2(inner.left(), inner.top() + 62.0), vec2(inner.width(), 40.0));
    {
        let mut su = panel.new_child(UiBuilder::new().max_rect(search_r).id_salt("where-previous-search"));
        widgets::search_field(&mut su, "where-previous-search", &mut state.places.previous.query, tr("Search worlds"), inner.width());
    }
    let mut y = search_r.bottom() + 12.0;
    let me = state.places.desk.asker().account.clone();
    let note = if me.is_none() && state.places.previous.connecting {
        // A saved session being restored: no sign-in offered; what is picked is asked once it is known (Major 1).
        Some(state.places.previous.connecting_text())
    } else if me.is_none() {
        Some(if state.places.previous.signed_in_elsewhere || state.places.previous.log.is_some() {
            tr("These are this computer's: on this profile, Play opens a new public instance of the world chosen.")
        } else {
            tr("Sign in to VRChat to rejoin an instance: without it, Play opens a new public instance of its world.")
        })
    } else {
        None
    };
    if let Some(note) = note {
        widgets::icon(&p, pos2(inner.left() + 8.0, y + 10.0), icons::INFO, 14.0, theme::fg_3());
        widgets::text_truncated(&p, pos2(inner.left() + 24.0, y + 10.0), Align2::LEFT_CENTER, &Txt::new(note, 12.5).color(theme::fg_2()), inner.width() - 24.0);
        y += 30.0;
    }

    // The rows: one list that scrolls, only those in view laid out.
    let list = Rect::from_min_max(pos2(inner.left() - 8.0, y), pos2(inner.right() + 8.0, inner.bottom()));
    let previous = &mut state.places.previous;
    let query = previous.query.trim();
    // Which of them the search finds, looked for again when it or they changed.
    if !previous.found.as_ref().is_some_and(|(of, asked, _)| asked == query && rows.as_ref().is_some_and(|r| Arc::ptr_eq(of, r))) {
        let find = orbiters_core::search::Query::new(query);
        previous.found = rows.as_ref().map(|r| (r.clone(), query.to_owned(), r.iter().enumerate().filter(|(_, v)| find.matches(&v.name)).map(|(i, _)| i).collect()));
    }
    let shown = previous.found.as_ref().map(|(.., found)| found.clone()).unwrap_or_default();
    let mut pressed: Option<(Visit, bool)> = None;
    let mut lu = panel.new_child(UiBuilder::new().max_rect(list).id_salt("where-previous-list"));
    lu.set_clip_rect(list.intersect(panel.clip_rect()));
    match &rows {
        None => {
            for i in 0..4 {
                stand_in_row(&lu, Rect::from_min_size(pos2(list.left() + 8.0, list.top() + i as f32 * (ROW_H + 6.0)), vec2(list.width() - 16.0, ROW_H)));
            }
        }
        Some(_) if shown.is_empty() => {
            let (title, text) = if state.places.previous.query.trim().is_empty() {
                (tr("No instance yet"), tr("Worlds you join in VRChat show here, newest first."))
            } else {
                (tr("No world of that name"), tr("Search another name, or clear the search to see every instance."))
            };
            let c = pos2(list.center().x, list.top() + 80.0);
            widgets::icon(&p, c, icons::GLOBE, 26.0, theme::fg_4());
            widgets::text(&p, c + vec2(0.0, 34.0), Align2::CENTER_CENTER, &Txt::new(title, 15.0).weight(weight::SEMIBOLD));
            widgets::text(&p, c + vec2(0.0, 56.0), Align2::CENTER_CENTER, &Txt::new(text, 12.5).color(theme::fg_3()));
        }
        Some(rows) => {
            let mut scroll = egui::ScrollArea::vertical().id_salt("where-previous-rows").auto_shrink([false, false]);
            let searched = &mut state.places.previous.searched;
            if crate::pages::new_search(searched, state.places.previous.query.as_str()) {
                scroll = scroll.vertical_scroll_offset(0.0);
            }
            let now = state.now().max(0) as u64;
            scroll.show_rows(&mut lu, ROW_H, shown.len(), |ui, range| {
                ui.spacing_mut().item_spacing = vec2(0.0, 6.0);
                for i in range {
                    let v = &rows[shown[i]];
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                    let r = r.shrink2(vec2(8.0, 0.0));
                    if let Some(p) = row(ui, state, r, v, now) {
                        pressed = Some((v.clone(), p));
                    }
                }
            });
        }
    }
    if let Some((visit, new)) = pressed {
        if new {
            take_new(state, &visit, Origin::Previous);
        } else {
            pick(state, cx, &visit, Origin::Previous);
        }
    }
    let outside = scrim.clicked() && !rect.contains(ctx.pointer_interact_pos().unwrap_or(Pos2::ZERO)) && !popup::any_open(&ctx);
    let escape = ctx.input(|i| i.key_pressed(Key::Escape)) && !popup::any_open(&ctx);
    if close || outside || escape {
        state.places.previous.close(clock);
    }
    if t < 0.999 || closing.is_some() {
        ctx.request_repaint();
    }
}

/// One instance: its world's picture, name, kind, region and when; what VRChat said of it once picked, and the way to a
/// new instance of its world where it cannot be rejoined. `Some(false)` pressed to pick it, `Some(true)` its new
/// instance taken.
fn row(ui: &mut Ui, state: &mut State, r: Rect, v: &Visit, now: u64) -> Option<bool> {
    let ctx = ui.ctx().clone();
    let id = Id::unique(("where-previous-row", v.location()));
    let resp = ui.interact(r, id, Sense::click());
    let name = world_name(v);
    widgets::note_control(&ctx, &name, r, true);
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let selection = state.places.desk.selection();
    let picked = (selection.destination().visit() == Some(v)).then(|| (selection.verified().clone(), selection.instance().is_none()));
    let chosen = picked.is_some() && selection.proven(state.places.desk.asker());
    let p = ui.painter().clone();
    let fill = if chosen { theme::alpha(theme::accent(), 0.10) } else { theme::white(0.03 + 0.04 * hover) };
    p.rect_filled(r, theme::radius(RADIUS + 2.0), fill);
    if chosen {
        p.rect_stroke(r, theme::radius(RADIUS + 2.0), egui::Stroke::new(1.5, theme::alpha(theme::accent(), 0.8)), egui::StrokeKind::Inside);
    }
    // The picture, the world's (Where's cards' own), else the name's colour with a globe.
    let pic = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 27.0), vec2(PIC_W, 54.0));
    match kept_picture(state, &v.world_id) {
        Some(tex) => paint::squircle_texture(&p, pic, [RADIUS; 4], tex.id(), cover_uv(tex.size_vec2(), pic.size()), Color32::WHITE),
        None => {
            let c = widgets::name_color(&name);
            paint::fill_convex(&p, &paint::squircle_points(pic, theme::squircle(RADIUS)), |at| {
                let t = ((at.x - pic.left()) / pic.width() + (at.y - pic.top()) / pic.height()) * 0.5;
                theme::mix(theme::alpha(c, 0.32), theme::ink_800(), t)
            });
            widgets::icon(&p, pic.center(), icons::GLOBE_HEMISPHERE_WEST, 20.0, theme::alpha(theme::fg(), 0.55));
        }
    }
    // The right: what it is now.
    let mut right = r.right() - 12.0;
    let mut took_new = false;
    let nobody = state.places.desk.asker().account.is_none();
    let connecting = state.places.previous.connecting;
    let new = matches!(picked, Some((_, true)));
    let status: Option<(String, Color32, &str)> = match &picked {
        None => None,
        Some((_, true)) => {
            let kind = super::destinations::kind_for(state, &v.world_id, state.places.previous.can_create);
            Some((orbiters_i18n::tr!("New {kind} instance · chosen", kind = access_name(kind_access(kind))), theme::accent(), icons::CHECK))
        }
        Some((Verified::Open { .. }, _)) => Some((tr("Open · chosen").into(), theme::accent(), icons::CHECK)),
        Some((Verified::Closed, _)) => Some((tr("Closed now").into(), theme::WARN, icons::WARNING_CIRCLE)),
        Some((Verified::Unverified, _)) if nobody && connecting => Some((state.places.previous.connecting_text().into(), theme::fg_3(), icons::HOURGLASS_MEDIUM)),
        Some((Verified::Unverified | Verified::Unknown, _)) if nobody => Some((tr("Not signed in").into(), theme::WARN, icons::WARNING_CIRCLE)),
        Some((Verified::Unknown, _)) => Some((tr("VRChat did not say").into(), theme::WARN, icons::WARNING_CIRCLE)),
        Some((Verified::Unverified | Verified::Checking { .. }, _)) => Some((tr("Asking VRChat…").into(), theme::fg_3(), icons::HOURGLASS_MEDIUM)),
    };
    let offer_new = picked.is_some() && alternatives(&state.places);
    if offer_new {
        let b = widgets::Button::new(tr("New instance of this world")).icon(icons::PLUS).variant(widgets::Variant::Secondary).size(widgets::Size::Sm).id_salt(("where-previous-new", v.location()));
        let bw = b.desired_width(ui);
        let br = Rect::from_min_size(pos2(right - bw, r.center().y + 2.0), vec2(bw, 28.0));
        if widgets::pressed_fresh(&b.show_at(ui, br)) {
            took_new = true;
        }
        kind_choice(ui, state, pos2(br.left() - 8.0, br.center().y), v);
    } else if new && state.places.previous.can_create {
        kind_choice(ui, state, pos2(right, r.center().y + 16.0), v);
    }
    if let Some((text, color, glyph)) = &status {
        let y = if offer_new { r.center().y - 14.0 } else if new && state.places.previous.can_create { r.center().y - 12.0 } else { r.center().y };
        let txt = Txt::new(text.as_str(), 12.5).weight(weight::MEDIUM).color(*color);
        let tw = widgets::text_size(ui, &txt).x;
        widgets::text(&p, pos2(right, y), Align2::RIGHT_CENTER, &txt);
        if *glyph == icons::HOURGLASS_MEDIUM {
            orbiters_ui::feedback::spinner(&p, pos2(right - tw - 12.0, y), 6.0, 1.6, *color);
        } else {
            widgets::icon(&p, pos2(right - tw - 12.0, y), glyph, 13.0, *color);
        }
        right -= tw + 26.0;
    } else if hover > 0.01 && picked.is_none() {
        let txt = Txt::new(tr("Choose"), 12.5).weight(weight::MEDIUM).color(theme::alpha(theme::fg_2(), hover));
        let tw = widgets::text_size(ui, &txt).x;
        widgets::text(&p, pos2(right, r.center().y), Align2::RIGHT_CENTER, &txt);
        right -= tw + 16.0;
    }
    if offer_new {
        right = right.min(r.right() - 12.0 - 300.0);
    }
    // The name, and the instance's kind, region and when.
    let x = pic.right() + 14.0;
    let room = (right - x - 12.0).max(60.0);
    widgets::text_truncated(&p, pos2(x, r.center().y - 10.0), Align2::LEFT_CENTER, &Txt::new(name.as_str(), 14.5).weight(weight::SEMIBOLD).tracking(-0.01), room);
    let access = v.access();
    let mut meta = access_name(access).to_string();
    let region = v.region();
    if !region.is_empty() {
        meta = format!("{meta} · {}", region_tag(&region));
    }
    meta = format!("{meta} · {}", crate::pages::ago(&ctx, v.at, now));
    widgets::icon(&p, pos2(x + 6.0, r.center().y + 12.0), access_icon(access), 12.0, theme::fg_3());
    widgets::text_truncated(&p, pos2(x + 18.0, r.center().y + 12.0), Align2::LEFT_CENTER, &Txt::new(meta, 12.0).color(theme::fg_3()), room - 18.0);
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    if took_new {
        return Some(true);
    }
    // A press on the row picks it (again: asked again).
    (widgets::pressed_fresh(&resp) && !popup::any_open(&ctx)).then_some(false)
}

/// The kind of a new instance of `v`'s world, as Where's card pill chooses it: a pill whose list has VRChat's kinds,
/// its right end at `right_center`. Public only (no list) where VRChat cannot make one for the account.
fn kind_choice(ui: &mut Ui, state: &mut State, right_center: Pos2, v: &Visit) -> Rect {
    let ctx = ui.ctx().clone();
    let can = state.places.previous.can_create;
    let kind = super::destinations::kind_for(state, &v.world_id, can);
    let access = kind_access(kind);
    let txt = Txt::new(access_name(access), 12.0).weight(weight::SEMIBOLD).color(theme::fg());
    let w = widgets::text_size(ui, &txt).x + if can { 46.0 } else { 32.0 };
    let r = Rect::from_min_size(pos2(right_center.x - w, right_center.y - 14.0), vec2(w, 28.0));
    let id = Id::unique(("where-previous-kind", v.location()));
    let resp = ui.interact(r, id, if can { Sense::click() } else { Sense::hover() });
    let list = id.with("list");
    let hover = anim::toggle(&ctx, id.with("hover"), can && (resp.hovered() || popup::is_open(&ctx, list)), Spring::SNAPPY);
    let p = ui.painter();
    p.rect_filled(r, theme::radius(14.0), theme::mix(theme::white(0.05), theme::white(0.1), hover));
    widgets::icon(p, pos2(r.left() + 13.0, r.center().y), access_icon(access), 12.0, theme::fg_2());
    widgets::text(p, pos2(r.left() + 24.0, r.center().y), Align2::LEFT_CENTER, &txt);
    if !can {
        resp.on_hover_text(tr("Sign in to VRChat in Settings to open Friends+, Friends or Invite instances: without it a new instance is public."));
        return r;
    }
    widgets::icon(p, pos2(r.right() - 13.0, r.center().y), icons::CARET_DOWN, 10.0, theme::fg_2());
    widgets::note_control(&ctx, tr("Who can join the new instance"), r, true);
    let items: Vec<popup::Item> = kinds().iter().map(|(_, a, says)| popup::Item::new(access_name(*a)).detail(says).icon(access_icon(*a))).collect();
    let selected = kinds().iter().position(|k| k.0 == kind);
    let pressed = widgets::activated(&resp);
    if let Some(i) = popup::list_from(&ctx, list, r, pressed, &items, selected, popup::list_width(ui, &items).max(220.0)) {
        state.places.kinds.insert(v.world_id.clone(), kinds()[i].0);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Who can join the new instance"));
    r
}

/// A row while the history is read.
fn stand_in_row(ui: &Ui, r: Rect) {
    let p = ui.painter();
    let t = anim::working(ui.ctx()) as f32;
    let shimmer = 0.5 + 0.5 * (t * 2.4).sin();
    p.rect_filled(r, theme::radius(RADIUS + 2.0), theme::white(0.02 + 0.02 * shimmer));
    p.rect_filled(Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 27.0), vec2(PIC_W, 54.0)), theme::radius(RADIUS), theme::white(0.04));
    p.rect_filled(Rect::from_min_size(pos2(r.left() + PIC_W + 22.0, r.center().y - 14.0), vec2(r.width() * 0.35, 11.0)), theme::radius(5.0), theme::white(0.06));
    p.rect_filled(Rect::from_min_size(pos2(r.left() + PIC_W + 22.0, r.center().y + 6.0), vec2(r.width() * 0.22, 9.0)), theme::radius(4.5), theme::white(0.04));
}

/// A world's picture for this sheet: only one already kept on this computer, never fetched (verify-r21 Major 6: opening
/// the list sends nothing, world ids included, before the person chooses anything). Its address is read once from the
/// Map explorer's page kept on disk; the picture from the file kept beside it; else the name's colour and a globe.
pub(super) fn kept_world_picture(state: &mut State, world_id: &str) -> Option<egui::TextureHandle> {
    kept_picture(state, world_id)
}

fn kept_picture(state: &mut State, world_id: &str) -> Option<egui::TextureHandle> {
    if state.places.previous.quiet || world_id.is_empty() {
        return None;
    }
    let url = state.places.previous.kept.entry(world_id.to_string()).or_insert_with(|| crate::worlds::kept_thumbnail(world_id)).clone()?;
    let ctx = state.ctx.clone()?;
    state.loader.as_mut()?.get(&ctx, &crate::worlds::Picture::RemoteKept { world_id: world_id.to_string(), url })
}

