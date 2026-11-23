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

use super::destinations::{kind_access, kinds, rejoinable, Chosen};
use super::{access_icon, access_name, cover_uv, region_tag, Cx, State};
use crate::controller::vrchat_data::{Request, Response};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Key, LayerId, Order, Pos2, Rect, Sense, Ui, UiBuilder};
use orbiters_core::launcher::InstanceAccess;
use orbiters_i18n::{tr, tr_n};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, paint, popup, widgets};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

/// How long VRChat's "still open" stands before Play asks it again (verify-r21 Major 4): VRChat's own client keeps an
/// instance's answer about a minute, so asking more often learns nothing; an instance chosen longer ago than that is
/// asked again at Play, and VRChat starts only if it still says open.
pub const OPEN_FOR: Duration = Duration::from_secs(60);

/// What VRChat said of a picked instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// VRChat's session is being restored: who is signed in is not known yet; asked once it is (verify-r21 Major 1).
    Waiting,
    /// Being asked.
    Asking,
    /// Open: rejoined.
    Open,
    /// Closed (or gone).
    Closed,
    /// VRChat did not say.
    Unknown,
    /// Not asked: no VRChat sign-in for the profile chosen, or its kind needs an invite (a new instance of its world).
    NotAsked,
}

/// The row picked, and where Play goes with it.
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    pub visit: Visit,
    pub check: Check,
    /// A new instance of its world was chosen (the instance closed, not known, or not joinable by its link).
    pub new: bool,
    /// When VRChat said it is open, and for which account (an Open is good for that account only, for [`OPEN_FOR`]).
    pub checked: Option<(Instant, String)>,
    /// The friend whose instance it is, when chosen with Select (REQUEST23): their id and name.
    pub with: Option<(String, String)>,
    /// The world's picture as that friend's row shows it.
    pub picture: Option<String>,
}

impl Picked {
    pub fn new(visit: Visit, check: Check, new: bool) -> Self {
        Self { visit, check, new, checked: None, with: None, picture: None }
    }

    /// Whether Play rejoins the instance itself (else it makes a new one of its world): VRChat said it is open, for the
    /// account signed in now, and its kind lets that account in by its link.
    pub fn rejoins(&self, me: Option<&str>) -> bool {
        !self.new
            && self.check == Check::Open
            && me.is_some_and(|me| rejoinable(&self.visit.instance, me) && self.checked.as_ref().is_some_and(|(_, who)| who == me))
    }

    /// Whether VRChat's "open" is older than [`OPEN_FOR`] (Play asks again).
    pub fn stale(&self, now: Instant) -> bool {
        self.checked.as_ref().is_none_or(|(at, _)| now.saturating_duration_since(*at) > OPEN_FOR)
    }

    /// Whether it is Where's choice now (a row only asked about, or closed and not taken as a new instance, is not).
    pub fn chosen(&self) -> bool {
        self.new || self.check == Check::Open
    }
}

/// The sheet's own state, in Where's.
#[derive(Default)]
pub struct Previous {
    /// Up since (egui's clock), and closing since.
    up: Option<(f64, Option<f64>)>,
    query: String,
    /// The text searched for at the last frame ([`crate::pages::new_search`]).
    searched: Option<String>,
    /// The history's instances, read when the sheet opens (pictures of the page hand them in), and whether a read is
    /// under way: one lock, so a read finishing while the sheet is closed is never lost (verify-r21 Major 5).
    history: Arc<Mutex<HistoryRead>>,
    /// How the history is read (tests: a stand-in; the app: this computer's file).
    reader: Option<Reader>,
    /// The rows were handed in (nothing is read), and nothing is asked of VRChat or loaded either (pictures of the page).
    staged: bool,
    quiet: bool,
    pub picked: Option<Picked>,
    /// The pick being asked about: an answer for an older one is dropped before it can replace anything (Major 4).
    generation: Arc<AtomicU64>,
    /// VRChat's answer for the pick of a generation.
    answer: Arc<Mutex<Option<Answer>>>,
    /// Play pressed on a chosen instance whose "open" was too old: started once VRChat says it still is.
    play_after: Option<(orbiters_core::launcher::LaunchMode, Option<u32>)>,
    /// The worlds' pictures this sheet may show: only those already kept on this computer (Major 6), looked up once.
    kept: std::collections::HashMap<String, Option<String>>,
    /// The account Where's places are for this frame, and whether VRChat makes instances for it (Where's row says).
    pub(super) me: Option<String>,
    pub(super) can_create: bool,
    pub(super) signed_in_elsewhere: bool,
    /// VRChat's saved session is being restored: who is signed in is not known yet (Major 1).
    pub(super) connecting: bool,
}

type Reader = Arc<dyn Fn() -> Vec<Visit> + Send + Sync>;

#[derive(Default)]
struct HistoryRead {
    rows: Option<Vec<Visit>>,
    reading: bool,
}

/// VRChat's answer about a pick: its generation, whether open, and when.
struct Answer {
    generation: u64,
    location: String,
    open: Option<bool>,
    at: Instant,
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
        self.history.lock().unwrap_or_else(|e| e.into_inner()).rows = Some(rows);
        self.staged = true;
        self.quiet = true;
        self.query = query.into();
        self.up = Some((0.0, None));
    }

    /// Tests: the history's instances handed in (nothing read); a row picked is asked of the backend as on the app.
    #[cfg(test)]
    pub fn stand_in_rows(&mut self, rows: Vec<Visit>) {
        self.history.lock().unwrap_or_else(|e| e.into_inner()).rows = Some(rows);
        self.staged = true;
    }

    /// Tests: the history read by `reader` (on its thread, as the app reads its file), nothing handed in.
    #[cfg(test)]
    pub fn stand_in_reader(&mut self, reader: impl Fn() -> Vec<Visit> + Send + Sync + 'static) {
        self.reader = Some(Arc::new(reader));
        self.staged = false;
    }

    /// Tests: VRChat's "open" for the pick made `by` ago.
    #[cfg(test)]
    pub fn age_check(&mut self, by: Duration) {
        if let Some((at, _)) = self.picked.as_mut().and_then(|p| p.checked.as_mut()) {
            *at = at.checked_sub(by).unwrap_or(*at);
        }
    }

    /// Another place was chosen in Where (a card, Home world): the pick being asked about is dropped, and its answer,
    /// when it comes, chooses nothing (verify-r21 Major 4).
    pub(super) fn cancel(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.play_after = None;
        if self.picked.as_ref().is_some_and(|p| matches!(p.check, Check::Asking | Check::Waiting)) {
            self.picked = None;
        }
    }

    /// Whether Play waits for VRChat to say again that the chosen instance is open.
    pub fn rechecking(&self) -> bool {
        self.play_after.is_some()
    }

    /// Pictures of the page: `visit` picked, VRChat having said `check`.
    pub fn stand_in_pick(&mut self, visit: Visit, check: Check, new: bool) {
        let checked = (check == Check::Open).then(|| (Instant::now(), self.me.clone().unwrap_or_default()));
        self.picked = Some(Picked { checked, ..Picked::new(visit, check, new) });
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
            let mut h = self.history.lock().unwrap_or_else(|e| e.into_inner());
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
    if widgets::activated(&b.show_at(ui, r)) {
        let clock = anim::time(ui.ctx());
        state.places.previous.open(clock);
    }
    w
}

/// Follows the pick: asked of VRChat once VRChat says who is signed in (a pick made while it connected), taken back
/// when that account goes, and VRChat's answer taken when it came, for the pick it was asked for only. Play waiting on
/// a fresh answer goes on, or stops with a word, here.
pub(super) fn follow(state: &mut State, cx: &Cx, clock: f64) {
    let me = state.places.previous.me.clone();
    let connecting = state.places.previous.connecting;
    // Pictures of the page stage an "open" before the account is set: it is that account's.
    if state.places.previous.quiet {
        if let Some((_, who)) = state.places.previous.picked.as_mut().and_then(|p| p.checked.as_mut()).filter(|(_, who)| who.is_empty()) {
            *who = me.clone().unwrap_or_default();
        }
    }
    // A pick made while VRChat connected: asked now that the account is known, or offered as a new instance when it
    // turned out signed out (or another account's profile): never taken as one by itself.
    if let Some(p) = state.places.previous.picked.clone().filter(|p| p.check == Check::Waiting) {
        let friends: std::collections::HashSet<String> = state.data.friends.iter().map(|f| f.id.clone()).collect();
        // A friend's instance is asked about only if it takes this account without asking (REQUEST23).
        let joinable = p.with.is_none() || me.as_deref().is_some_and(|me| super::destinations::joinable_without_asking(&p.visit.instance, me, &|id| friends.contains(id)));
        if me.is_some() && joinable {
            ask(state, cx, &p.visit, false);
        } else if !connecting {
            match &p.with {
                // A friend's instance is never taken as a new one of its world, and no sheet's row shows it: it goes, with a word.
                Some((_, name)) => {
                    state.places.previous.picked = None;
                    state.places.notice = Some((not_said(name), clock));
                }
                None => state.places.previous.picked = Some(carried(state, &p.visit, Check::NotAsked, false)),
            }
        }
    }
    // A friend's instance (REQUEST23): it goes when the friend left it or went offline; the choice goes back to Home
    // and the row says so, as when any chosen place goes.
    if let Some((p, (friend, name))) = state.places.previous.picked.clone().and_then(|p| p.with.clone().map(|w| (p, w))) {
        let there = state.data.friends.iter().find(|f| f.id == friend).is_some_and(|f| {
            f.presence != orbiters_core::launcher::Presence::Offline
                && matches!(&f.whereabouts, orbiters_core::launcher::Whereabouts::World(pl) if pl.world_id == p.visit.world_id && pl.instance_id == p.visit.instance)
        });
        if !there && !state.stale() {
            state.places.previous.generation.fetch_add(1, Ordering::SeqCst);
            state.places.previous.picked = None;
            if state.places.previous.play_after.take().is_some() {
                state.places.making = None;
            }
            if state.places.chosen.as_ref().is_some_and(|c| c.key == p.visit.key()) {
                state.places.chosen = None;
            }
            state.places.notice = Some((orbiters_i18n::tr!("{name} has left that instance: Play starts VRChat at home.", name = name), clock));
            return;
        }
    }
    let answer = state.places.previous.answer.lock().unwrap_or_else(|e| e.into_inner()).take();
    let Some(answer) = answer else { return };
    if answer.generation != state.places.previous.generation.load(Ordering::SeqCst) {
        return;
    }
    let rechecking = state.places.previous.play_after.is_some();
    let Some(p) = state.places.previous.picked.as_mut().filter(|p| p.visit.location() == answer.location && (p.check == Check::Asking || rechecking)) else { return };
    p.check = match answer.open {
        Some(true) => Check::Open,
        Some(false) => Check::Closed,
        None => Check::Unknown,
    };
    p.checked = (p.check == Check::Open).then(|| (answer.at, me.clone().unwrap_or_default()));
    let rejoins = p.rejoins(me.as_deref());
    let (key, name, with, check) = (p.visit.key(), world_name(&p.visit), p.with.clone(), p.check);
    let play_after = state.places.previous.play_after.take();
    if rejoins {
        state.places.chosen = Some(Chosen { key, name, instance: true });
        state.places.page = 0;
        state.places.notice = None;
        if let Some((mode, profile)) = play_after {
            // Asked again at Play and still open: VRChat starts there now.
            state.places.making = None;
            state.places.previous.picked.as_mut().map(|p| p.checked = Some((Instant::now(), me.clone().unwrap_or_default())));
            super::destinations::play(state, cx, clock, mode, profile);
        }
    } else {
        // A friend's instance (REQUEST23) has no row in a sheet to say it: the line under Where does, and it goes.
        if play_after.is_none() {
            if let Some((_, friend)) = &with {
                let line = match check {
                    Check::Closed => orbiters_i18n::tr!("{name}'s instance is closed now: Play starts VRChat at home.", name = friend),
                    _ => not_said(friend),
                };
                state.places.previous.picked = None;
                state.places.notice = Some((line, clock));
            }
        }
        if play_after.is_some() {
            state.places.making = None;
            if state.places.chosen.as_ref().is_some_and(|c| c.key == key) {
                state.places.chosen = None;
            }
            if with.is_some() {
                state.places.previous.picked = None;
            }
            state.places.notice = Some((orbiters_i18n::tr!("{name} has closed since you chose it: choose a new instance of its world, or another place.", name = name), clock));
        }
    }
}

/// VRChat could not say whether a friend's instance is open.
fn not_said(friend: &str) -> String {
    orbiters_i18n::tr!("VRChat did not say whether {name}'s instance is open: Play starts VRChat at home.", name = friend)
}

/// What the line under Where says while a friend's instance, chosen with Select, is being checked (REQUEST23).
pub(super) fn checking(previous: &Previous) -> Option<String> {
    let p = previous.picked.as_ref()?;
    let (_, name) = p.with.as_ref()?;
    match p.check {
        Check::Waiting => Some(orbiters_i18n::tr!("Connecting to VRChat: {name}'s instance is checked next…", name = name)),
        Check::Asking => Some(orbiters_i18n::tr!("Asking VRChat whether {name}'s instance is open…", name = name)),
        _ => None,
    }
}

/// A row pressed: kept until VRChat says who is signed in (while it connects), asked of VRChat where it can be
/// rejoined, else taken at once as a new instance of its world.
fn pick(state: &mut State, cx: &Cx, visit: &Visit) {
    let me = state.places.previous.me.clone();
    // A pick replaces the one before: its answer, still to come, chooses nothing.
    state.places.previous.generation.fetch_add(1, Ordering::SeqCst);
    state.places.previous.play_after = None;
    if state.places.chosen.as_ref().is_some_and(|c| c.key.starts_with("previous:")) {
        // Not chosen until VRChat said it is open (a closed one is not taken silently as a new one).
        state.places.chosen = None;
    }
    if me.is_none() && state.places.previous.connecting {
        // VRChat's session is being restored (verify-r21 Major 1): the instance is kept, asked once it is known.
        state.places.previous.picked = Some(Picked::new(visit.clone(), Check::Waiting, false));
        return;
    }
    if me.as_deref().is_some_and(|me| rejoinable(&visit.instance, me)) {
        ask(state, cx, visit, false);
    } else {
        // Signed out, another account's profile, or an instance only an invite lets in: a new one of its world.
        take_new(state, visit, Check::NotAsked);
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

/// Asks VRChat whether `visit` is still open, for the current pick's generation (a later pick, or another place
/// chosen, drops this answer before it can replace anything).
/// `again`: the choice stands while it is asked again (Play on an old "open"), else the pick shows as being asked.
fn ask(state: &mut State, cx: &Cx, visit: &Visit, again: bool) {
    let generation = state.places.previous.generation.load(Ordering::SeqCst);
    if !again {
        state.places.previous.picked = Some(carried(state, visit, Check::Asking, false));
    }
    if state.places.previous.quiet {
        return;
    }
    let Some(scope) = state.scope.clone() else {
        // Not asked: said as an answer that knows nothing (`follow` takes it on the next frame).
        unanswered(state, visit, generation);
        return;
    };
    let (ctl, answer, current, ctx) = (cx.ctl.clone(), state.places.previous.answer.clone(), state.places.previous.generation.clone(), state.ctx.clone());
    let (world, instance, location) = (visit.world_id.clone(), visit.instance.clone(), visit.location());
    #[cfg(test)]
    let paused = instance.clone();
    // A read into memory (whether VRChat says the instance is open): nothing of the window's work.
    let started = std::thread::Builder::new().name("where-previous-open".into()).spawn(move || {
        let open = match ctl.vrchat_data(scope, Request::Instance { world, instance }) {
            Ok(Response::Instance(open)) => open,
            _ => None,
        };
        // Only the current pick's answer is kept: an older one never overwrites it.
        if current.load(Ordering::SeqCst) == generation {
            #[cfg(test)]
            pause_before_publishing(&paused);
            *answer.lock().unwrap_or_else(|e| e.into_inner()) = Some(Answer { generation, location, open, at: Instant::now() });
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        }
    });
    if started.is_err() {
        unanswered(state, visit, generation);
    }
}

/// An answer for `visit` that does not know whether it is open (it could not be asked).
fn unanswered(state: &mut State, visit: &Visit, generation: u64) {
    *state.places.previous.answer.lock().unwrap_or_else(|e| e.into_inner()) = Some(Answer { generation, location: visit.location(), open: None, at: Instant::now() });
    if let Some(ctx) = &state.ctx {
        ctx.request_repaint();
    }
}

/// Play on a previous instance VRChat said was open longer ago than [`OPEN_FOR`]: asked again first; VRChat starts
/// there only if it still is (`follow`). Whether Play waits for that.
pub(super) fn recheck_before_play(state: &mut State, cx: &Cx, key: &str, mode: orbiters_core::launcher::LaunchMode, profile: Option<u32>) -> bool {
    let Some(p) = state.places.previous.picked.clone().filter(|p| p.visit.key() == key && p.check == Check::Open) else { return false };
    if !p.stale(Instant::now()) {
        return false;
    }
    state.places.previous.generation.fetch_add(1, Ordering::SeqCst);
    // The choice stays shown while it is asked again.
    ask(state, cx, &p.visit, true);
    state.places.previous.play_after = Some((mode, profile));
    state.places.making = Some(mode);
    true
}

/// The row's world taken as a new instance: Where's choice now.
fn take_new(state: &mut State, visit: &Visit, check: Check) {
    state.places.previous.generation.fetch_add(1, Ordering::SeqCst);
    state.places.previous.picked = Some(carried(state, visit, check, true));
    state.places.chosen = Some(Chosen { key: visit.key(), name: world_name(visit), instance: false });
    state.places.page = 0;
    state.places.notice = None;
}

/// A pick of `visit` as `check`, keeping whose instance it is (a friend's Select) when it is the same instance.
fn carried(state: &State, visit: &Visit, check: Check, new: bool) -> Picked {
    let before = state.places.previous.picked.as_ref().filter(|p| p.visit.world_id == visit.world_id && p.visit.instance == visit.instance);
    Picked { with: before.and_then(|p| p.with.clone()), picture: before.and_then(|p| p.picture.clone()), ..Picked::new(visit.clone(), check, new) }
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
    let me = match &state.places.previous.me {
        Some(me) => me.clone(),
        None if state.places.previous.connecting => String::new(),
        None => return None,
    };
    let friends: std::collections::HashSet<&str> = state.data.friends.iter().map(|f| f.id.as_str()).collect();
    super::destinations::joinable_without_asking(&place.instance_id, &me, &|id| friends.contains(id)).then(|| place.clone())
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
    let with = Some((friend_id.to_string(), friend_name.to_string()));
    let picture = (!place.world_picture.is_empty()).then(|| place.world_picture.clone());
    let me = state.places.previous.me.clone();
    let friends: std::collections::HashSet<String> = state.data.friends.iter().map(|f| f.id.clone()).collect();
    let check = if me.is_none() && state.places.previous.connecting {
        Check::Waiting
    } else if me.as_deref().is_some_and(|me| super::destinations::joinable_without_asking(&visit.instance, me, &|id| friends.contains(id))) {
        Check::Asking
    } else {
        // Not joinable any more (the list changed under the press): nothing is chosen.
        return;
    };
    state.places.previous.generation.fetch_add(1, Ordering::SeqCst);
    state.places.previous.play_after = None;
    if state.places.chosen.as_ref().is_some_and(|c| c.key.starts_with("previous:")) {
        state.places.chosen = None;
    }
    state.places.previous.picked = Some(Picked { with, picture, ..Picked::new(visit.clone(), check, false) });
    if check == Check::Asking {
        ask(state, cx, &visit, false);
    }
    state.places.reveal = true;
    state.places.page = 0;
    state.places.notice = None;
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
            let mut h = state.places.previous.history.lock().unwrap_or_else(|e| e.into_inner());
            let start = h.rows.is_none() && !h.reading;
            h.reading |= start;
            start
        };
        if start {
            let (history, ctx2) = (state.places.previous.history.clone(), ctx.clone());
            let started = std::thread::Builder::new().name("where-previous-history".into()).spawn(move || {
                let rows = reader();
                let mut h = history.lock().unwrap_or_else(|e| e.into_inner());
                h.rows = Some(rows);
                h.reading = false;
                drop(h);
                ctx2.request_repaint();
            });
            if started.is_err() {
                let mut h = state.places.previous.history.lock().unwrap_or_else(|e| e.into_inner());
                h.rows = Some(Vec::new());
                h.reading = false;
            }
        }
    }
    follow(state, cx, clock);

    let layer = LayerId::new(Order::Foreground, Id::unique("where-previous-layer"));
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
    widgets::frame(panel.painter(), rect, 22.0, theme::INK_900);
    widgets::frame(panel.painter(), rect, 22.0, theme::surface());
    paint::stroke_squircle(panel.painter(), rect, theme::squircle(22.0), theme::hairline());
    let inner = rect.shrink2(vec2(22.0, 20.0));

    // The head: its name, what it is, and close.
    let p = panel.painter().clone();
    widgets::text(&p, pos2(inner.left(), inner.top() + 14.0), Align2::LEFT_CENTER, &Txt::new(tr("Previous instances"), 20.0).weight(weight::SEMIBOLD).tracking(-0.01));
    let rows = state.places.previous.history.lock().unwrap_or_else(|e| e.into_inner()).rows.clone();
    let sub = match &rows {
        Some(rows) if !rows.is_empty() => tr_n("{count} instance you were in, newest first", "{count} instances you were in, newest first", rows.len() as u64),
        _ => tr("Instances you were in").into(),
    };
    widgets::text_truncated(&p, pos2(inner.left(), inner.top() + 40.0), Align2::LEFT_CENTER, &Txt::new(sub, 12.5).color(theme::FG_3), inner.width() - 60.0);
    let close_r = Rect::from_min_size(pos2(inner.right() - 32.0, inner.top() - 2.0), vec2(32.0, 32.0));
    let close = widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Close")).size(32.0, 14.0).id_salt("where-previous-close").show_at(&mut panel, close_r));

    // The search, and what the sign-in allows.
    let search_r = Rect::from_min_size(pos2(inner.left(), inner.top() + 62.0), vec2(inner.width(), 40.0));
    {
        let mut su = panel.new_child(UiBuilder::new().max_rect(search_r).id_salt("where-previous-search"));
        widgets::search_field(&mut su, "where-previous-search", &mut state.places.previous.query, tr("Search worlds"), inner.width());
    }
    let mut y = search_r.bottom() + 12.0;
    let me = state.places.previous.me.clone();
    let note = if me.is_none() && state.places.previous.connecting {
        // A saved session being restored: no sign-in offered; what is picked is asked once it is known (Major 1).
        Some(tr("Connecting to VRChat…"))
    } else if me.is_none() {
        Some(if state.places.previous.signed_in_elsewhere {
            tr("These are this computer's: on this profile, Play opens a new public instance of the world chosen.")
        } else {
            tr("Sign in to VRChat to rejoin an instance: without it, Play opens a new public instance of its world.")
        })
    } else {
        None
    };
    if let Some(note) = note {
        widgets::icon(&p, pos2(inner.left() + 8.0, y + 10.0), icons::INFO, 14.0, theme::FG_3);
        widgets::text_truncated(&p, pos2(inner.left() + 24.0, y + 10.0), Align2::LEFT_CENTER, &Txt::new(note, 12.5).color(theme::FG_2), inner.width() - 24.0);
        y += 30.0;
    }

    // The rows: one list that scrolls, only those in view laid out.
    let list = Rect::from_min_max(pos2(inner.left() - 8.0, y), pos2(inner.right() + 8.0, inner.bottom()));
    let find = orbiters_core::search::Query::new(state.places.previous.query.trim());
    let shown: Vec<Visit> = rows.as_deref().unwrap_or_default().iter().filter(|v| find.matches(&v.name)).cloned().collect();
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
            widgets::icon(&p, c, icons::GLOBE, 26.0, theme::FG_4);
            widgets::text(&p, c + vec2(0.0, 34.0), Align2::CENTER_CENTER, &Txt::new(title, 15.0).weight(weight::SEMIBOLD));
            widgets::text(&p, c + vec2(0.0, 56.0), Align2::CENTER_CENTER, &Txt::new(text, 12.5).color(theme::FG_3));
        }
        Some(_) => {
            let mut scroll = egui::ScrollArea::vertical().id_salt("where-previous-rows").auto_shrink([false, false]);
            let searched = &mut state.places.previous.searched;
            if crate::pages::new_search(searched, state.places.previous.query.as_str()) {
                scroll = scroll.vertical_scroll_offset(0.0);
            }
            let now = state.now().max(0) as u64;
            scroll.show_rows(&mut lu, ROW_H, shown.len(), |ui, range| {
                ui.spacing_mut().item_spacing = vec2(0.0, 6.0);
                for i in range {
                    let v = &shown[i];
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
            take_new(state, &visit, state.places.previous.picked.as_ref().map_or(Check::Unknown, |p| p.check));
        } else {
            pick(state, cx, &visit);
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
    let picked = state.places.previous.picked.as_ref().filter(|p| p.visit == *v).cloned();
    let chosen = picked.as_ref().is_some_and(|p| p.chosen()) && state.places.chosen.as_ref().is_some_and(|c| c.key == v.key());
    let p = ui.painter().clone();
    let fill = if chosen { theme::alpha(theme::ACCENT, 0.10) } else { theme::white(0.03 + 0.04 * hover) };
    p.rect_filled(r, theme::radius(RADIUS + 2.0), fill);
    if chosen {
        p.rect_stroke(r, theme::radius(RADIUS + 2.0), egui::Stroke::new(1.5, theme::alpha(theme::ACCENT, 0.8)), egui::StrokeKind::Inside);
    }
    // The picture, the world's (Where's cards' own), else the name's colour with a globe.
    let pic = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 27.0), vec2(PIC_W, 54.0));
    match kept_picture(state, &v.world_id) {
        Some(tex) => paint::squircle_texture(&p, pic, [RADIUS; 4], tex.id(), cover_uv(tex.size_vec2(), pic.size()), Color32::WHITE),
        None => {
            let c = widgets::name_color(&name);
            paint::fill_convex(&p, &paint::squircle_points(pic, theme::squircle(RADIUS)), |at| {
                let t = ((at.x - pic.left()) / pic.width() + (at.y - pic.top()) / pic.height()) * 0.5;
                theme::mix(theme::alpha(c, 0.32), theme::INK_800, t)
            });
            widgets::icon(&p, pic.center(), icons::GLOBE_HEMISPHERE_WEST, 20.0, theme::alpha(theme::FG, 0.55));
        }
    }
    // The right: what it is now.
    let mut right = r.right() - 12.0;
    let mut took_new = false;
    let check = picked.as_ref().map(|p| (p.check, p.new));
    let status: Option<(String, Color32, &str)> = match check {
        None => None,
        Some((Check::Asking, _)) => Some((tr("Asking VRChat…").into(), theme::FG_3, icons::HOURGLASS_MEDIUM)),
        Some((Check::Waiting, _)) => Some((tr("Connecting to VRChat…").into(), theme::FG_3, icons::HOURGLASS_MEDIUM)),
        Some((Check::NotAsked, false)) => Some((tr("Not signed in").into(), theme::WARN, icons::WARNING_CIRCLE)),
        Some((_, true)) => {
            let kind = super::destinations::kind_for(state, &v.world_id, state.places.previous.can_create);
            Some((orbiters_i18n::tr!("New {kind} instance · chosen", kind = access_name(kind_access(kind))), theme::ACCENT, icons::CHECK))
        }
        Some((Check::Open, false)) => Some((tr("Open · chosen").into(), theme::ACCENT, icons::CHECK)),
        Some((Check::Closed, false)) => Some((tr("Closed now").into(), theme::WARN, icons::WARNING_CIRCLE)),
        Some((Check::Unknown, false)) => Some((tr("VRChat did not say").into(), theme::WARN, icons::WARNING_CIRCLE)),
    };
    let offer_new = matches!(check, Some((Check::Closed | Check::Unknown | Check::NotAsked, false)));
    if offer_new {
        let b = widgets::Button::new(tr("New instance of this world")).icon(icons::PLUS).variant(widgets::Variant::Secondary).size(widgets::Size::Sm).id_salt(("where-previous-new", v.location()));
        let bw = b.desired_width(ui);
        let br = Rect::from_min_size(pos2(right - bw, r.center().y + 2.0), vec2(bw, 28.0));
        if widgets::activated(&b.show_at(ui, br)) {
            took_new = true;
        }
        kind_choice(ui, state, pos2(br.left() - 8.0, br.center().y), v);
    } else if matches!(check, Some((_, true))) && state.places.previous.can_create {
        kind_choice(ui, state, pos2(right, r.center().y + 16.0), v);
    }
    if let Some((text, color, glyph)) = &status {
        let y = if offer_new { r.center().y - 14.0 } else if matches!(check, Some((_, true))) && state.places.previous.can_create { r.center().y - 12.0 } else { r.center().y };
        let txt = Txt::new(text.as_str(), 12.5).weight(weight::MEDIUM).color(*color);
        let tw = widgets::text_size(ui, &txt).x;
        widgets::text(&p, pos2(right, y), Align2::RIGHT_CENTER, &txt);
        if *glyph == icons::HOURGLASS_MEDIUM {
            orbiters_ui::feedback::spinner(&p, pos2(right - tw - 12.0, y), 6.0, 1.6, *color);
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
        } else {
            widgets::icon(&p, pos2(right - tw - 12.0, y), glyph, 13.0, *color);
        }
        right -= tw + 26.0;
    } else if hover > 0.01 && check.is_none() {
        let txt = Txt::new(tr("Choose"), 12.5).weight(weight::MEDIUM).color(theme::alpha(theme::FG_2, hover));
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
    widgets::icon(&p, pos2(x + 6.0, r.center().y + 12.0), access_icon(access), 12.0, theme::FG_3);
    widgets::text_truncated(&p, pos2(x + 18.0, r.center().y + 12.0), Align2::LEFT_CENTER, &Txt::new(meta, 12.0).color(theme::FG_3), room - 18.0);
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    if took_new {
        return Some(true);
    }
    // A press on the row picks it (again: asked again).
    (widgets::activated(&resp) && !popup::any_open(&ctx)).then_some(false)
}

/// The kind of a new instance of `v`'s world, as Where's card pill chooses it: a pill whose list has VRChat's kinds,
/// its right end at `right_center`. Public only (no list) where VRChat cannot make one for the account.
fn kind_choice(ui: &mut Ui, state: &mut State, right_center: Pos2, v: &Visit) -> Rect {
    let ctx = ui.ctx().clone();
    let can = state.places.previous.can_create;
    let kind = super::destinations::kind_for(state, &v.world_id, can);
    let access = kind_access(kind);
    let txt = Txt::new(access_name(access), 12.0).weight(weight::SEMIBOLD).color(theme::FG);
    let w = widgets::text_size(ui, &txt).x + if can { 46.0 } else { 32.0 };
    let r = Rect::from_min_size(pos2(right_center.x - w, right_center.y - 14.0), vec2(w, 28.0));
    let id = Id::unique(("where-previous-kind", v.location()));
    let resp = ui.interact(r, id, if can { Sense::click() } else { Sense::hover() });
    let list = id.with("list");
    let hover = anim::toggle(&ctx, id.with("hover"), can && (resp.hovered() || popup::is_open(&ctx, list)), Spring::SNAPPY);
    let p = ui.painter();
    p.rect_filled(r, theme::radius(14.0), theme::mix(theme::white(0.05), theme::white(0.1), hover));
    widgets::icon(p, pos2(r.left() + 13.0, r.center().y), access_icon(access), 12.0, theme::FG_2);
    widgets::text(p, pos2(r.left() + 24.0, r.center().y), Align2::LEFT_CENTER, &txt);
    if !can {
        resp.on_hover_text(tr("Sign in to VRChat in Settings to open Friends+, Friends or Invite instances: without it a new instance is public."));
        return r;
    }
    widgets::icon(p, pos2(r.right() - 13.0, r.center().y), icons::CARET_DOWN, 10.0, theme::FG_2);
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
    let t = anim::clock(ui.ctx(), 30.0) as f32;
    let shimmer = 0.5 + 0.5 * (t * 2.4).sin();
    p.rect_filled(r, theme::radius(RADIUS + 2.0), theme::white(0.02 + 0.02 * shimmer));
    p.rect_filled(Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 27.0), vec2(PIC_W, 54.0)), theme::radius(RADIUS), theme::white(0.04));
    p.rect_filled(Rect::from_min_size(pos2(r.left() + PIC_W + 22.0, r.center().y - 14.0), vec2(r.width() * 0.35, 11.0)), theme::radius(5.0), theme::white(0.06));
    p.rect_filled(Rect::from_min_size(pos2(r.left() + PIC_W + 22.0, r.center().y + 6.0), vec2(r.width() * 0.22, 9.0)), theme::radius(4.5), theme::white(0.04));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
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

