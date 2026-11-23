//! N4N0 as a VRChat launcher, on the desktop Home page (REQUEST10): the friends list in the right column (or a rail
//! of pictures on a narrow window, which opens it as a drawer), the two Play buttons and a received invite's card at
//! the top of Home, a person's profile in a modal, request an invite and boop on the row and in the modal.
//!
//! The page reads the contract's [`LauncherState`] (`orbiters_core::launcher`, filled by the controller) and sends its
//! [`Action`]s through [`crate::backend::Ctl::launcher`]; a press shows at once, before the service answers. Pictures
//! are textures handed in by URL ([`State::provide_picture`]); the page lists the ones it would draw
//! ([`State::wanted_pictures`]).

pub mod destinations;
mod hero;
pub mod list;
pub mod mine;
mod modal;
pub mod points;
pub mod previous;
mod row;
pub mod states;

pub use hero::hero;
pub use mine::card as mine_card;

use super::Cx;
use egui::{pos2, vec2, Align2, Color32, Id, Key, LayerId, Order, Pos2, Rect, Sense, Ui, UiBuilder};
use list::Row;
use orbiters_core::launcher::{Action, Friend, InstanceAccess, LauncherState, Presence, Rank, Status, Whereabouts};
use orbiters_i18n::{tr, tr_ctx};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::icons;
use orbiters_ui::theme::{self, Txt};
use orbiters_ui::{paint, widgets};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The right column's width, and the page width under which it becomes the rail.
const COLUMN_MIN: f32 = 300.0;
const COLUMN_MAX: f32 = 372.0;
const COLUMN_FROM: f32 = 900.0;
/// The rail's width on a narrow window.
const RAIL_W: f32 = 64.0;
/// Room between the column and the page, and around the column.
pub const GAP: f32 = 16.0;
/// The column's fill: rows fade their banners into it.
pub fn column_fill() -> Color32 {
    theme::INK_900
}

/// What the page beside Home's content shows at a width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    /// The full list, this wide.
    Column(f32),
    /// Pictures only; the list opens over the page.
    Rail,
}

/// The right side at a page `width`.
pub fn side(width: f32) -> Side {
    if width >= COLUMN_FROM {
        Side::Column((width * 0.3).clamp(COLUMN_MIN, COLUMN_MAX))
    } else {
        Side::Rail
    }
}

impl Side {
    pub fn width(self) -> f32 {
        match self {
            Side::Column(w) => w,
            Side::Rail => RAIL_W,
        }
    }
}

/// The profile shown in the modal, the row it opened from, and since when (its opening and closing move).
struct Modal {
    id: String,
    from: Rect,
    opened: f64,
    closing: Option<f64>,
}

pub struct State {
    data: Arc<LauncherState>,
    query: String,
    /// Sections closed by the person, kept between runs.
    folded: HashSet<String>,
    folded_read: bool,
    /// The rows for the data, the search and the folds they were made from.
    rows: Vec<Row>,
    keys: Vec<String>,
    tops: Vec<f32>,
    /// The data the rows were made for (held, so that its address is never handed to a newer state), the search, the folds.
    made_for: Option<(Arc<LauncherState>, String, u64, u64)>,
    folds: u64,
    /// The row at the list's top and how far into it, kept across refreshes.
    anchor: Option<(String, f32)>,
    /// What this page asked and when (its clock): shown at once.
    requested: HashMap<String, f64>,
    booped: HashMap<String, (f64, Option<String>)>,
    /// Invites, requests and boops put away here.
    dismissed: HashSet<String>,
    modal: Option<Modal>,
    /// The list over the page on a narrow window, and since when.
    drawer: Option<f64>,
    pictures: HashMap<String, egui::TextureHandle>,
    wanted: Vec<String>,
    /// A fixed clock (pictures of the page).
    now_fixed: Option<i64>,
    /// Where pictures not handed in are loaded (VRChat's pictures read with the signed-in session, through the service),
    /// and the session they are read with: set by [`State::wire`].
    loader: Option<crate::worlds::Pictures>,
    scope: Option<crate::controller::vrchat_data::Scope>,
    ctx: Option<egui::Context>,
    /// Data handed in from outside (pictures of the page): the snapshot's is then left alone.
    staged: bool,
    /// The VRChat profile the person chose for the next Play, and a new one they added here (REQUEST15 7a).
    chosen_profile: Option<u32>,
    added_profile: Option<u32>,
    /// The VRChat account the sign-in says is signed in now (`None` inside: none), once the page is wired to the service:
    /// the own-profile card shows only while it is the launcher's account too (REQUEST15 second pass, blocker 3).
    login: Option<Option<String>>,
    /// The account's own profile card (REQUEST15 7b), and the picture lists it asked for in this run.
    pub mine: mine::Mine,
    mine_listed: Vec<orbiters_core::launcher::PictureKind>,
    /// Whose picture each address is (REQUEST17: one kept per person, the one before shown until a new one arrived),
    /// for the data it was made from.
    owners: HashMap<String, String>,
    owners_for: Option<Arc<LauncherState>>,
    /// The data and session the whole list's pictures were last handed to the background for, and what was handed.
    warmed_for: Option<(Arc<LauncherState>, crate::controller::vrchat_data::Scope)>,
    warmed: u64,
    /// Where VRChat starts into, above the Play buttons (REQUEST20 2).
    pub places: destinations::Places,
}

impl Default for State {
    fn default() -> Self {
        Self {
            data: Arc::new(LauncherState::default()),
            query: String::new(),
            folded: HashSet::new(),
            folded_read: false,
            rows: Vec::new(),
            keys: Vec::new(),
            tops: vec![0.0],
            made_for: None,
            folds: 0,
            anchor: None,
            requested: HashMap::new(),
            booped: HashMap::new(),
            dismissed: HashSet::new(),
            modal: None,
            drawer: None,
            pictures: HashMap::new(),
            wanted: Vec::new(),
            now_fixed: None,
            loader: None,
            scope: None,
            ctx: None,
            staged: false,
            chosen_profile: None,
            added_profile: None,
            login: None,
            mine: mine::Mine::default(),
            mine_listed: Vec::new(),
            owners: HashMap::new(),
            owners_for: None,
            warmed_for: None,
            warmed: 0,
            places: destinations::Places::default(),
        }
    }
}

impl State {
    /// What the controller knows now (the page keeps where the person was in the list).
    pub fn set_data(&mut self, data: Arc<LauncherState>) {
        self.data = data;
        self.staged = true;
    }

    /// Each frame, before Home draws: the controller's state (the same `Arc` while nothing changed) and the loader of
    /// the pictures it names.
    pub fn wire(&mut self, ctx: &egui::Context, cx: &Cx) {
        let data = cx.snap.launcher.arc();
        if !self.staged && !Arc::ptr_eq(&data, &self.data) {
            self.data = data;
            self.take_back_failed();
        }
        if self.loader.is_none() {
            self.loader = Some(crate::worlds::Pictures::with_backend(ctx, Some(cx.ctl.clone())));
            self.ctx = Some(ctx.clone());
        }
        self.scope = Some(crate::controller::vrchat_data::Scope::of(cx.snap));
        // The service's sign-in, against the service's list; data handed in for pictures of the page is its own.
        if !self.staged {
            // Only the account VRChat verified counts (verify-r21 Major 2): a name shown while connecting is not proof
            // of whose list this is.
            self.login = Some(cx.snap.vrchat_login.user.as_ref().map(|u| u.id.clone()));
        }
        self.mask_if_stale();
        self.warm_pictures();
    }

    /// The people's pictures of the data, each address with whose it is: the account's own, invites and boops, then
    /// the friends in the order the sectioned list shows them (favourites in the game by their groups, in the game, on
    /// the website, offline: every section open), then anyone in none, each once. The background asks them in this order,
    /// so what the list shows first comes first (the rows on screen are asked before all of them). A person's picture is the one
    /// their row shows; an invite's or a boop's other picture of someone on the list is nobody's (kept by its address:
    /// one picture a person, two would replace each other at every look).
    fn people_pictures(data: &LauncherState) -> Vec<(String, Option<String>)> {
        let icons: HashMap<&str, &str> = data.friends.iter().chain(data.me.iter()).map(|f| (f.id.as_str(), f.look.icon.as_str())).collect();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut add = |url: &str, user: &str| {
            let user = if icons.get(user).is_none_or(|icon| *icon == url) { user } else { "" };
            if url.starts_with("https://") && seen.insert((url.to_string(), user.to_string())) {
                out.push((url.to_string(), (!user.is_empty()).then(|| user.to_string())));
            }
        };
        if let Some(me) = &data.me {
            add(&me.look.icon, &me.id);
        }
        for i in &data.invites {
            add(&i.from_icon, &i.from_id);
        }
        for b in &data.boops {
            add(&b.from_icon, &b.from_id);
        }
        for row in list::rows(data, "", &HashSet::new()) {
            if let Row::Person { friend, .. } = row {
                if let Some(f) = data.friends.get(friend) {
                    add(&f.look.icon, &f.id);
                }
            }
        }
        for f in &data.friends {
            add(&f.look.icon, &f.id);
        }
        out
    }

    /// The whole list's pictures to the background (REQUEST17), when the list or the session changed: kept ones are
    /// shown at once from then on, missing ones come from VRCX's or VRCNext's copies or, slowly, from VRChat.
    fn warm_pictures(&mut self) {
        let (Some(_), Some(scope)) = (self.loader.as_ref(), self.scope.clone()) else { return };
        if self.stale() || self.warmed_for.as_ref().is_some_and(|(d, s)| Arc::ptr_eq(d, &self.data) && *s == scope) {
            return;
        }
        self.warmed_for = Some((self.data.clone(), scope.clone()));
        let items = Self::people_pictures(&self.data);
        let hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (&scope, &items).hash(&mut h);
            h.finish()
        };
        if hash != self.warmed && !items.is_empty() {
            self.warmed = hash;
            if let Some(loader) = self.loader.as_mut() {
                loader.warm_profiles(&scope, items);
            }
            // Who has no picture address at all (their row shows the letter whatever the cache does), for the log.
            let without: Vec<String> = self.data.friends.iter().filter(|f| f.look.icon.is_empty()).map(|f| crate::worlds::profile_pictures::who(Some(&f.id))).collect();
            if !without.is_empty() {
                let shown: Vec<&str> = without.iter().take(40).map(String::as_str).collect();
                crate::worlds::profile_pictures::note(&format!("{} of {} friends have no picture address: {}", without.len(), self.data.friends.len(), shown.join(" "))); // i18n: not shown (a log line)
            }
        }
    }

    /// Whose picture `url` is, in the data on show.
    fn owner_of(&mut self, url: &str) -> Option<String> {
        if !self.owners_for.as_ref().is_some_and(|d| Arc::ptr_eq(d, &self.data)) {
            self.owners = Self::people_pictures(&self.data).into_iter().rev().filter_map(|(u, user)| Some((u, user?))).collect();
            self.owners_for = Some(self.data.clone());
        }
        self.owners.get(url).cloned()
    }

    /// Whether the list on show is a VRChat account's that is no longer the one signed in (signed out, or another account
    /// signed in), before the service's next look replaces it (REQUEST15 third pass, blocker 2): nothing of it is shown,
    /// pressed, opened or loaded meanwhile.
    pub fn stale(&self) -> bool {
        match (&self.data.source, &self.login) {
            (orbiters_core::launcher::Source::SignedIn { user_id, .. }, Some(login)) => login.as_deref() != Some(user_id.as_str()),
            _ => false,
        }
    }

    /// The same frame the list goes stale: its profile window and its drawer close, what was asked for it is dropped.
    fn mask_if_stale(&mut self) {
        if self.stale() {
            self.modal = None;
            self.drawer = None;
            self.wanted.clear();
        }
    }

    /// Pictures of the page: the VRChat account signed in now (`None`: signed out), against the list handed in.
    pub fn stand_in_login(&mut self, account: Option<String>) {
        self.login = Some(account);
        self.mask_if_stale();
    }

    /// A picture by its URL, once loaded.
    pub fn provide_picture(&mut self, url: &str, texture: egui::TextureHandle) {
        self.pictures.insert(url.to_string(), texture);
    }

    /// The pictures the last frame would have drawn and did not have (the controller loads them).
    #[allow(dead_code)]
    pub fn wanted_pictures(&self) -> &[String] {
        &self.wanted
    }

    /// Pictures of the page: whether pictures asked of the loader are still being read.
    pub fn stand_in_pictures_loading(&self) -> bool {
        self.loader.as_ref().is_some_and(|l| l.loading())
    }

    /// Pictures of the page: a fixed clock, the search typed.
    pub fn stand_in(&mut self, now: i64, query: &str) {
        self.now_fixed = Some(now);
        self.query = query.to_string();
        self.folded_read = true;
    }

    /// Pictures of the page: a boop sent at `clock` (its animation), a request, something put away.
    pub fn stand_in_boop(&mut self, to: &str, clock: f64, emoji: Option<String>) {
        self.note(clock, &Action::Boop { to: to.into(), emoji });
    }

    pub fn stand_in_request(&mut self, to: &str, clock: f64) {
        self.note(clock, &Action::RequestInvite { to: to.into() });
    }

    pub fn stand_in_dismiss(&mut self, id: &str) {
        self.dismissed.insert(id.to_string());
    }

    /// Pictures of the page: these sections folded (never saved).
    pub fn stand_in_folded(&mut self, sections: &[&str]) {
        self.folded = sections.iter().map(|s| s.to_string()).collect();
        self.folds += 1;
    }

    /// Pictures of the page: the VRChat profile chosen for the next Play.
    pub fn stand_in_profile(&mut self, profile: u32) {
        self.chosen_profile = Some(profile);
    }

    /// The list's data (tests).
    #[cfg(test)]
    pub fn data_for_tests(&self) -> Arc<LauncherState> {
        self.data.clone()
    }

    /// The profile window open, whose (tests).
    #[cfg(test)]
    pub fn modal_open(&self) -> Option<&str> {
        self.modal.as_ref().filter(|m| m.closing.is_none()).map(|m| m.id.as_str())
    }

    /// Opens a person's profile as if their row was pressed at `from`.
    pub fn open_profile(&mut self, ctx: &egui::Context, id: &str, from: Rect) {
        self.modal = Some(Modal { id: id.to_string(), from, opened: anim::time(ctx), closing: None });
    }

    /// Opens the list over the page (the rail's button).
    pub fn open_drawer(&mut self, ctx: &egui::Context) {
        self.drawer = Some(anim::time(ctx));
    }

    fn now(&self) -> i64 {
        self.now_fixed.unwrap_or_else(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
        })
    }

    /// Whether there is anything to show: what the controller read, or VRChat to start. Until then (a Mac, the
    /// service not yet started) Home is as it was.
    pub fn shown(&self) -> bool {
        let d = &self.data;
        !matches!(d.source, orbiters_core::launcher::Source::Nothing) || d.possible.play || d.game != orbiters_core::launcher::Game::NotRunning
    }

    /// A friend's profile (or another of its windows) is open, or the account's own is being changed
    /// (`crate::window_rest`).
    pub fn holds(&self) -> bool {
        self.modal.is_some() || self.mine.editing.is_some() || self.places.previous.is_up()
    }

    /// A boop or request the service could not send: its "Sent" (shown at the press) is taken back; the toast says why.
    fn take_back_failed(&mut self) {
        if let Some(orbiters_core::launcher::Notice::Failed { to, action, .. }) = &self.data.notice {
            match action {
                orbiters_core::launcher::SentKind::Boop => {
                    self.booped.remove(to);
                }
                orbiters_core::launcher::SentKind::RequestInvite => {
                    self.requested.remove(to);
                }
                _ => {}
            }
        }
    }

    /// Sends `action` and shows it at once.
    fn act(&mut self, cx: &Cx, clock: f64, action: Action) {
        self.press(clock, action, |a| cx.ctl.launcher(a));
    }

    /// [`Self::act`] with the sending given: shows the press, sends it bound to the account on show, and takes the
    /// press back when it never reached the service.
    fn press(&mut self, clock: f64, action: Action, send: impl FnOnce(Action) -> bool) {
        // A list whose account is gone takes no press of its own; the Play buttons and the profile chips, which are
        // this computer's and not the list's, still start VRChat (sent bound to no account: nothing of the old list goes).
        if self.stale() {
            if local_play(&action) {
                send(action);
            }
            return;
        }
        self.note(clock, &action);
        if !send(self.as_shown(action.clone())) {
            // Never reached the service: nothing went, so nothing says it did.
            self.unnote(&action);
        }
    }

    /// The action bound to the VRChat account whose list is on show: the service sends nothing with another one.
    fn as_shown(&self, action: Action) -> Action {
        match &self.data.source {
            orbiters_core::launcher::Source::SignedIn { user_id, .. } if !user_id.is_empty() => Action::As { account: user_id.clone(), action: Box::new(action) },
            _ => action,
        }
    }

    /// Takes back what [`Self::note`] showed for `action`.
    fn unnote(&mut self, action: &Action) {
        match action {
            Action::RequestInvite { to } => {
                self.requested.remove(to);
            }
            Action::Boop { to, .. } => {
                self.booped.remove(to);
            }
            Action::Dismiss { id } => {
                self.dismissed.remove(id);
            }
            _ => {}
        }
    }

    /// What a press shows before the service answers.
    fn note(&mut self, clock: f64, action: &Action) {
        match action {
            Action::RequestInvite { to } => {
                self.requested.insert(to.clone(), clock);
            }
            Action::Boop { to, emoji } => {
                self.booped.insert(to.clone(), (clock, emoji.clone()));
            }
            Action::Dismiss { id } => {
                self.dismissed.insert(id.clone());
            }
            _ => {}
        }
    }

    fn picture(&mut self, url: &str) -> Option<egui::TextureHandle> {
        // Nothing of a list whose account is gone is asked for (a picture would be read with the session of now).
        if url.is_empty() || self.stale() {
            return None;
        }
        if let Some(found) = self.pictures.get(url) {
            return Some(found.clone());
        }
        // Loaded on its own threads (three, newest first, kept small on this computer): never waits here.
        let user = if url.starts_with("https://") { self.owner_of(url) } else { None };
        if let (Some(loader), Some(scope), Some(ctx)) = (self.loader.as_mut(), self.scope.as_ref(), self.ctx.as_ref()) {
            if url.starts_with("https://") {
                if let Some(t) = loader.get(ctx, &crate::worlds::Picture::Profile { scope: scope.clone(), url: url.to_string(), user }) {
                    return Some(t);
                }
            }
        }
        if !self.wanted.iter().any(|w| w == url) {
            self.wanted.push(url.to_string());
        }
        None
    }

    fn fold(&mut self, group: &str) {
        if !self.folded.remove(group) {
            self.folded.insert(group.to_string());
        }
        self.folds += 1;
        save_folded(&self.folded);
    }

    /// The rows again when the data, the search or a fold changed; the row at the top stays where it was.
    fn refresh_rows(&mut self) -> Option<f32> {
        if !self.folded_read {
            self.folded_read = true;
            self.folded = load_folded();
        }
        // What was put away (its received rows go), whatever the order the set gives it in.
        let put_away = self.dismissed.iter().fold(self.dismissed.len() as u64, |sum, id| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            id.hash(&mut h);
            sum.wrapping_add(h.finish())
        });
        if self.made_for.as_ref().is_some_and(|(data, query, folds, away)| Arc::ptr_eq(data, &self.data) && *query == self.query && *folds == self.folds && *away == put_away) {
            return None;
        }
        let same_search = self.made_for.as_ref().is_some_and(|m| m.1 == self.query);
        self.made_for = Some((self.data.clone(), self.query.clone(), self.folds, put_away));
        self.rows = list::with_received(&self.data, &self.query, &self.folded, &self.dismissed);
        self.keys = self.rows.iter().map(|r| r.key(&self.data)).collect();
        self.tops = list::tops(&self.rows);
        // A new search starts at the top; new data keeps the row in view.
        if !same_search {
            return Some(0.0);
        }
        self.anchor.as_ref().and_then(|a| list::anchored(&self.keys, &self.tops, a))
    }
}

fn folded_file() -> Option<std::path::PathBuf> {
    orbiters_core::paths::roots().map(|r| r.config.join("launcher-folded.json"))
}

fn load_folded() -> HashSet<String> {
    folded_from(folded_file().and_then(|f| std::fs::read(f).ok()))
}

/// The sections folded, from what was saved: never saved, Offline starts folded as VRCNext's does (REQUEST20 3), so the
/// list opens on who is there; once the person folded or opened anything, what they chose.
fn folded_from(saved: Option<Vec<u8>>) -> HashSet<String> {
    match saved {
        Some(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        None => [list::OFFLINE.to_string()].into(),
    }
}

fn save_folded(folded: &HashSet<String>) {
    if let (Some(file), Ok(json)) = (folded_file(), serde_json::to_vec(folded)) {
        let _ = std::fs::write(file, json);
    }
}

// --- What every part draws the same way ---

/// A status's colour (VRChat's meaning, Orbiters' shades).
pub fn status_color(status: Status) -> Color32 {
    match status {
        Status::JoinMe => theme::BLUE,
        Status::Active => theme::ACCENT,
        Status::AskMe => theme::WARN,
        Status::Busy => theme::DANGER,
        Status::Offline => theme::FG_4,
    }
}

pub fn status_name(status: Status) -> &'static str {
    match status {
        Status::JoinMe => tr_ctx("vrchat status", "Join me"),
        Status::Active => tr_ctx("vrchat status", "Online"),
        Status::AskMe => tr_ctx("vrchat status", "Ask me"),
        Status::Busy => tr_ctx("vrchat status", "Busy"),
        Status::Offline => tr_ctx("vrchat status", "Offline"),
    }
}

/// A trust rank's colour on the name, softened to sit with the rest.
pub fn rank_color(rank: Rank) -> Color32 {
    let c = match rank {
        Rank::Visitor => theme::FG_2,
        Rank::NewUser => Color32::from_rgb(0x5a, 0x9b, 0xff),
        Rank::User => Color32::from_rgb(0x4c, 0xd9, 0x7b),
        Rank::KnownUser => Color32::from_rgb(0xff, 0x96, 0x5c),
        Rank::TrustedUser => Color32::from_rgb(0xb3, 0x86, 0xff),
    };
    theme::mix(theme::FG, c, 0.8)
}

pub fn rank_name(rank: Rank) -> &'static str {
    match rank {
        Rank::Visitor => tr("Visitor"),
        Rank::NewUser => tr("New user"),
        Rank::User => tr("User"),
        Rank::KnownUser => tr("Known user"),
        Rank::TrustedUser => tr("Trusted user"),
    }
}

/// An instance's kind, as VRChat names them.
pub fn access_name(access: InstanceAccess) -> &'static str {
    match access {
        InstanceAccess::Public => tr_ctx("instance", "Public"),
        InstanceAccess::FriendsPlus => tr_ctx("instance", "Friends+"),
        InstanceAccess::Friends => tr_ctx("instance", "Friends"),
        InstanceAccess::InvitePlus => tr_ctx("instance", "Invite+"),
        InstanceAccess::Invite => tr_ctx("instance", "Invite"),
        InstanceAccess::GroupPublic => tr_ctx("instance", "Group public"),
        InstanceAccess::GroupPlus => tr_ctx("instance", "Group+"),
        InstanceAccess::Group => tr_ctx("instance", "Group"),
        InstanceAccess::Unknown => "",
    }
}

pub fn access_icon(access: InstanceAccess) -> &'static str {
    match access {
        InstanceAccess::Public | InstanceAccess::GroupPublic => icons::GLOBE,
        InstanceAccess::FriendsPlus | InstanceAccess::Friends => icons::USERS,
        InstanceAccess::GroupPlus | InstanceAccess::Group => icons::USERS_THREE,
        InstanceAccess::InvitePlus | InstanceAccess::Invite | InstanceAccess::Unknown => icons::LOCK_SIMPLE,
    }
}

/// `us`, `use`, `eu`, `jp` as a short tag.
pub fn region_tag(region: &str) -> String {
    match region {
        "us" => "US W".into(),
        "use" => "US E".into(),
        other => other.to_uppercase(),
    }
}

/// Where someone is, in a line: the world and its instance, or what is said instead.
pub fn where_line(friend: &Friend) -> Option<(&'static str, String)> {
    match &friend.whereabouts {
        Whereabouts::World(place) => {
            let world = if place.world_name.is_empty() { tr("A world").to_string() } else { place.world_name.clone() };
            Some((access_icon(place.access), world))
        }
        Whereabouts::Private => Some((icons::LOCK_SIMPLE, tr("In a private instance").into())),
        Whereabouts::Traveling => Some((icons::PAPER_PLANE_TILT, tr("Traveling…").into())),
        Whereabouts::Website => Some((icons::GLOBE_HEMISPHERE_WEST, tr("On the website").into())),
        Whereabouts::Offline | Whereabouts::Unknown => None,
    }
}

/// The part of a texture of `size` that covers a rect of `target` (centred, cropped).
pub fn cover_uv(size: egui::Vec2, target: egui::Vec2) -> Rect {
    let (ia, ta) = (size.x / size.y.max(1.0), target.x / target.y.max(1.0));
    let (w, h) = if ta > ia { (1.0, ia / ta) } else { (ta / ia, 1.0) };
    Rect::from_center_size(pos2(0.5, 0.5), vec2(w, h))
}

/// A person's round picture with their status dot at its foot (hollow while they are on the website), ringed by
/// `ring` so it reads over anything.
pub fn person_picture(p: &egui::Painter, rect: Rect, picture: Option<&egui::TextureHandle>, frame: Option<&egui::TextureHandle>, friend: &Friend, ring: Color32) {
    widgets::avatar(p, rect, picture, &friend.name);
    // Their icon frame (a VRChat decoration) around it.
    if let Some(frame) = frame {
        let around = rect.expand(rect.width() * 0.16);
        p.image(frame.id(), around, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    let r = (rect.width() * 0.15).clamp(4.0, 9.0);
    let c = rect.right_bottom() - vec2(r * 0.95, r * 0.95);
    p.circle_filled(c, r + 2.5, ring);
    let color = status_color(friend.status);
    match friend.presence {
        Presence::Website => {
            p.circle_stroke(c, r - 1.0, egui::Stroke::new(2.2, color));
        }
        Presence::Offline => {
            p.circle_stroke(c, r - 1.0, egui::Stroke::new(2.0, theme::FG_4));
        }
        Presence::InGame => {
            p.circle_filled(c, r, color);
        }
    }
}

/// A banner behind `rect`, fading into `fill` towards its left (`from`: where it starts to show, 0..1).
pub fn banner(p: &egui::Painter, rect: Rect, radius: f32, texture: &egui::TextureHandle, fill: Color32, from: f32, strength: f32) {
    let uv = cover_uv(texture.size_vec2(), rect.size());
    paint::squircle_texture(p, rect, [radius; 4], texture.id(), uv, theme::white(strength));
    let points = paint::squircle_points(rect, radius);
    paint::fill_convex(p, &points, |at| {
        let t = ((at.x - rect.left()) / rect.width().max(1.0) - from) / (1.0 - from).max(0.01);
        let t = t.clamp(0.0, 1.0);
        // Opaque at the left, a veil at the right so the text stays readable.
        theme::alpha(fill, 1.0 - 0.62 * anim::ease::in_out(t))
    });
}

/// What is behind a person's row or head: their nameplate, else their banner, else their banner's colour as a soft
/// wash, fading into `fill` towards the left from `from`.
#[allow(clippy::too_many_arguments)]
pub(super) fn backdrop(p: &egui::Painter, rect: Rect, radius: f32, state: &mut State, look: &orbiters_core::launcher::Look, fill: Color32, from: f32, strength: f32) {
    let picture = state.picture(&look.nameplate).or_else(|| state.picture(&look.banner));
    if let Some(texture) = picture {
        banner(p, rect, radius, &texture, fill, from, strength);
    } else if let Some([r, g, b]) = look.banner_colour {
        let c = Color32::from_rgb(r, g, b);
        paint::fill_convex(p, &paint::squircle_points(rect, radius), |at| {
            let t = (((at.x - rect.left()) / rect.width().max(1.0) - from) / (1.0 - from).max(0.01)).clamp(0.0, 1.0);
            theme::alpha(c, 0.42 * strength * anim::ease::in_out(t))
        });
    }
}

/// A press that needs nothing of the account's list: Play with no destination, Play on a profile, a Mac's Obelisk, and
/// Where's places of this computer's own history (a list's invite or room is never one: [`destinations::Suggestion::local`]).
fn local_play(action: &Action) -> bool {
    matches!(action, Action::Play { destination: None, .. } | Action::PlayOn { .. } | Action::PlayAt { .. } | Action::OpenEngine | Action::InstallEngine)
}

/// The column (or the rail) beside Home's content, at `rect`.
pub fn side_panel(ui: &mut Ui, cx: &mut Cx, state: &mut State, rect: Rect, side: Side) {
    state.wanted.clear();
    if state.stale() {
        return row::put_away(ui, state, rect, side);
    }
    match side {
        Side::Column(_) => column(ui, cx, state, rect, false),
        Side::Rail => row::rail(ui, cx, state, rect),
    }
}

/// What floats over the page: the drawer of a narrow window, then the profile modal. Call after everything else.
pub fn over_page(ui: &mut Ui, cx: &mut Cx, state: &mut State, page: Rect) {
    if state.stale() {
        return;
    }
    let ctx = ui.ctx().clone();
    let clock = anim::time(&ctx);
    if let Some(since) = state.drawer {
        let open = anim::spring_from(&ctx, Id::unique(("launcher-drawer", since.to_bits())), [0.0], [1.0], Spring::EXPAND)[0];
        let layer = LayerId::new(Order::Middle, Id::unique("launcher-drawer-layer"));
        let mut over = ui.new_child(UiBuilder::new().max_rect(page).layer_id(layer));
        let scrim = over.interact(page, Id::unique("launcher-drawer-scrim"), Sense::click());
        over.painter().rect_filled(page, 0.0, theme::black(0.45 * open.clamp(0.0, 1.0)));
        let w = COLUMN_MAX.min(page.width() - 48.0);
        let at = Rect::from_min_size(pos2(page.right() - GAP - w + (1.0 - open) * (w + GAP), page.top() + cx.top + 8.0), vec2(w, page.height() - cx.top - 8.0 - GAP));
        let mut panel = over.new_child(UiBuilder::new().max_rect(at));
        paint::shadow(panel.painter(), at, 18.0, vec2(0.0, 16.0), 40.0, -6.0, theme::black(0.5));
        column(&mut panel, cx, state, at, true);
        let escape = state.modal.is_none() && ctx.input(|i| i.key_pressed(Key::Escape));
        if (scrim.clicked() && !at.contains(ctx.pointer_interact_pos().unwrap_or(Pos2::ZERO))) || escape {
            state.drawer = None;
        }
    }
    modal::show(ui, cx, state, page, clock);
    // Where's previous instances (REQUEST21 7).
    previous::sheet(ui, cx, state, page);
}

/// The friends list: the account, search, counts, what needs the sign-in, received boops and requests, then the
/// sections. `drawer`: over the page (a close button in its head).
fn column(ui: &mut Ui, cx: &mut Cx, state: &mut State, rect: Rect, drawer: bool) {
    let ctx = ui.ctx().clone();
    let clock = anim::time(&ctx);
    let (alpha, dy) = widgets::entrance(ui, Id::unique(("launcher-column", cx.visit)), cx.entered, 1);
    let rect = rect.translate(vec2(0.0, dy * 0.5));
    let mut ui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("launcher-column"));
    ui.multiply_opacity(alpha);
    ui.set_clip_rect(rect.expand(2.0).intersect(ui.clip_rect()));
    widgets::frame(ui.painter(), rect, theme::CARD_RADIUS, column_fill());
    paint::stroke_squircle(ui.painter(), rect, theme::squircle(theme::CARD_RADIUS), theme::hairline());

    let inner = rect.shrink2(vec2(10.0, 10.0));
    let mut y = inner.top();
    // The account itself.
    let me_h = 60.0;
    row::me(&mut ui, cx, state, Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), me_h)), drawer, clock);
    y += me_h + 8.0;
    // Search.
    {
        let field = Rect::from_min_size(pos2(inner.left() + 2.0, y), vec2(inner.width() - 4.0, 40.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(field));
        let hint = tr("Search friends, worlds, notes");
        widgets::search_field_clearable(&mut child, "launcher-search", &mut state.query, hint, field.width());
        y += 40.0 + 10.0;
    }
    // Counts.
    {
        let (in_game, web, all) = list::counts(&state.data);
        let p = ui.painter();
        let mut x = inner.left() + 8.0;
        let cy = y + 8.0;
        let mut part = |p: &egui::Painter, dot: Option<(Color32, bool)>, text: String| {
            if let Some((c, filled)) = dot {
                if filled {
                    p.circle_filled(pos2(x + 4.0, cy), 4.0, c);
                } else {
                    p.circle_stroke(pos2(x + 4.0, cy), 3.3, egui::Stroke::new(1.6, c));
                }
                x += 13.0;
            }
            let r = widgets::text(p, pos2(x, cy), Align2::LEFT_CENTER, &Txt::new(text, 12.0).color(theme::FG_2));
            x = r.right() + 14.0;
        };
        // A copy knows nobody's presence now: only how many.
        if states::live(&state.data) == states::Live::SignedIn {
            part(p, Some((theme::ACCENT, true)), orbiters_i18n::tr!("{n} in game", n = in_game));
            part(p, Some((theme::ACCENT, false)), orbiters_i18n::tr!("{n} on the website", n = web));
        }
        let total = orbiters_i18n::tr!("{n} friends", n = all);
        widgets::text(p, pos2(inner.right() - 8.0, cy), Align2::RIGHT_CENTER, &Txt::new(total, 12.0).color(theme::FG_4));
        y += 16.0 + 10.0;
    }
    // The one line about what needs the sign-in.
    y = row::sign_in_note(&mut ui, cx, state, inner, y);
    // Boops and requests received are the list's first rows: one list, one scroll (REQUEST18 1).

    let list_rect = Rect::from_min_max(pos2(rect.left() + 4.0, y), pos2(rect.right() - 4.0, rect.bottom() - 6.0));
    if list_rect.height() > 20.0 {
        // A thin line where the list scrolls under the head.
        ui.painter().hline(inner.x_range(), y - 1.0, egui::Stroke::new(1.0, theme::white(0.05)));
        row::list(&mut ui, cx, state, list_rect, clock);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_narrow_page_gets_the_rail_and_a_wide_one_the_column() {
        assert_eq!(side(668.0), Side::Rail);
        assert_eq!(side(1048.0), Side::Column(COLUMN_MIN.max(1048.0 * 0.3)));
        assert_eq!(side(2400.0), Side::Column(COLUMN_MAX));
        // The default window (1280 points, the sidebar's 232 off) has the column and three feature tiles' room (Home's
        // margins 32 and 20, tiles of 210 at least, 16 apart).
        let Side::Column(w) = side(1048.0) else { panic!() };
        assert!(1048.0 - GAP - w - 52.0 >= 3.0 * 210.0 + 2.0 * 16.0);
    }

    #[test]
    fn a_press_shows_at_once() {
        let mut state = State { data: Arc::new(orbiters_core::launcher::sample()), ..Default::default() };
        let to = state.data.friends[0].id.clone();
        state.note(3.0, &Action::RequestInvite { to: to.clone() });
        state.note(3.5, &Action::Boop { to: to.clone(), emoji: Some("sample_1".into()) });
        state.note(3.6, &Action::Dismiss { id: "not_1".into() });
        let friend = state.data.friends[0].clone();
        assert_eq!(states::request(&state.data, &friend, state.requested.get(&to).copied(), 4.0, 0), states::Reach::Sent);
        assert_eq!(states::boop(&state.data, &friend, state.booped.get(&to).map(|b| b.0), 4.0, 0), states::Reach::Sent);
        assert!(state.dismissed.contains("not_1"));
    }

    #[test]
    fn a_press_that_never_reached_the_service_does_not_say_sent_and_one_that_did_names_its_account() {
        let mut data = orbiters_core::launcher::sample();
        data.source = orbiters_core::launcher::Source::SignedIn { user_id: "usr_a".into(), name: String::new() };
        let mut state = State { data: Arc::new(data), ..Default::default() };
        let to = state.data.friends[0].id.clone();
        state.press(3.0, Action::Boop { to: to.clone(), emoji: None }, |_| false);
        assert!(!state.booped.contains_key(&to), "the link was closed: no Sent");
        let mut sent = None;
        state.press(3.0, Action::RequestInvite { to: to.clone() }, |a| {
            sent = Some(a);
            true
        });
        assert!(state.requested.contains_key(&to));
        assert_eq!(sent, Some(Action::As { account: "usr_a".into(), action: Box::new(Action::RequestInvite { to }) }), "bound to the list's account");
    }

    #[test]
    fn a_list_whose_account_signed_out_is_put_away_on_the_same_frame() {
        let mut data = orbiters_core::launcher::sample();
        data.source = orbiters_core::launcher::Source::SignedIn { user_id: "usr_a".into(), name: String::new() };
        let friend = data.friends[0].clone();
        let mut state = State { data: Arc::new(data), login: Some(Some("usr_a".into())), ..Default::default() };
        assert!(!state.stale());
        state.modal = Some(Modal { id: friend.id.clone(), from: Rect::NOTHING, opened: 0.0, closing: None });
        state.drawer = Some(0.0);
        // Signed out: the service has not looked again yet, A's list is still the data.
        state.login = Some(None);
        assert!(state.stale());
        state.mask_if_stale();
        assert!(state.modal.is_none() && state.drawer.is_none(), "A's profile window (and its note) closes at once");
        let mut sent = Vec::new();
        state.press(1.0, Action::Dismiss { id: "not_boop_a".into() }, |a| {
            sent.push(a);
            true
        });
        state.press(1.0, Action::OpenProfile { id: friend.id.clone() }, |a| {
            sent.push(a);
            true
        });
        assert!(sent.is_empty() && state.dismissed.is_empty(), "no press of the old list goes");
        // The Play buttons and the profile chips are this computer's: they still start VRChat, bound to no account.
        let place = orbiters_core::launcher::Place {
            world_id: "wrld_00000000-0000-4000-8000-000000000001".into(),
            world_name: String::new(),
            world_picture: String::new(),
            instance_id: "1".into(),
            access: orbiters_core::launcher::InstanceAccess::Public,
            region: String::new(),
        };
        for action in [
            Action::Play { mode: orbiters_core::launcher::LaunchMode::Vr, destination: None },
            Action::PlayOn { mode: orbiters_core::launcher::LaunchMode::Desktop, profile: 1 },
            Action::Play { mode: orbiters_core::launcher::LaunchMode::Vr, destination: Some(place) },
        ] {
            state.press(1.0, action, |a| {
                sent.push(a);
                true
            });
        }
        assert_eq!(
            sent,
            [Action::Play { mode: orbiters_core::launcher::LaunchMode::Vr, destination: None }, Action::PlayOn { mode: orbiters_core::launcher::LaunchMode::Desktop, profile: 1 }],
            "a destination of the old list (a friend's room) does not go"
        );
        assert!(state.picture("https://api.vrchat.cloud/api/1/file/file_x/1/file").is_none() && state.wanted.is_empty(), "no picture of it is asked for");
        // Another account signed in, the old list still there: the same.
        state.login = Some(Some("usr_b".into()));
        assert!(state.stale());
        // Unwired (pictures of the page, tests) or a local copy: never stale.
        state.login = None;
        assert!(!state.stale());
    }

    #[test]
    fn offline_starts_folded_and_then_what_the_person_chose_is_kept() {
        assert_eq!(folded_from(None), [list::OFFLINE.to_string()].into());
        assert!(folded_from(Some(b"[]".to_vec())).is_empty(), "opened once: stays open");
        assert_eq!(folded_from(Some(br#"["group_0","website"]"#.to_vec())), ["group_0".to_string(), "website".to_string()].into());
    }

    #[test]
    fn home_stays_as_it_was_until_there_is_something_to_show() {
        let mut state = State::default();
        assert!(!state.shown());
        let mut data = LauncherState::default();
        data.possible.play = true;
        state.set_data(Arc::new(data));
        assert!(state.shown(), "VRChat to start, nobody signed in: the list says how to sign in");
    }
}
