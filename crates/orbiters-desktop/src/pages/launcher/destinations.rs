//! Where (REQUEST20 2): above Home's Play buttons, the places VRChat can start right into, each a card with the world's
//! picture, its name and why it is offered; the one chosen is ringed in the accent and named in the heading, and Play
//! starts VRChat (on the profile chosen under the buttons) in it.
//!
//! What is offered, strongest first (a world offered twice shows once, with the stronger reason):
//! 1. **Invites** received on the VRChat account signed in here, still in its list (not put away, not accepted) and at
//!    most [`INVITE_FOR`] old: the invite's own instance, accepted as the invite card's buttons do
//!    ([`Action::AcceptInviteOn`]); its sender's name is shown here only and goes nowhere.
//! 2. **Events** of Orbiters the member asked an invite for (the Events tab's "Invite me": a step signed up for and not
//!    withdrawn, `GET community-events/{id}/invites/me`) that are on now or start within [`EVENT_AHEAD`]: the event's
//!    live instance when Orbiters gives one; else VRChat starts at home, where the event's invite arrives.
//! 3. **The last instance** of this computer's history (`visited-worlds.json`, kept by the tray service from VRChat's
//!    logs): rejoined while VRChat says it is open and its kind lets the account in, else a new instance of its world.
//! 4. **The [`FREQUENT_MAX`] places visited most** in that history: each visit of the last [`FREQUENT_DAYS`] days counts,
//!    halved for every [`HALF_LIFE_DAYS`] days of age (a world of last week outweighs one of two months ago); a new
//!    instance of each, Friends+ unless the card's kind says otherwise.
//!
//! The **Home world** card (VRChat's own start, no destination) always comes first, and is what Play does until another
//! card is chosen: nothing is chosen for the person, an invite never. The choice lasts while the window runs; a card that
//! goes (an invite put away or accepted, an event over, an instance that closed) gives the choice back to Home and says so.
//!
//! Nothing is sent from the account by showing or choosing a card. At Play: a new instance is made by VRChat for the
//! signed-in account (`POST /instances`, the overlay's Go there's [`Request::CreateInstance`]) and VRChat started in it;
//! without a VRChat sign-in a new instance is a public one named here, as the 3D worlds' visited list does
//! ([`finder::new_instance`]). The launch is Home's own ([`Action::PlayAt`]: the link last, VRChat's ids only).
//!
//! [`Request::CreateInstance`]: crate::controller::vrchat_data::Request::CreateInstance
//! [`finder::new_instance`]: crate::overlay::worlds::finder::new_instance

use super::states::PlayState;
use super::{access_icon, access_name, cover_uv, region_tag, Cx, State};
use crate::controller::vrchat_data::{Request, Response, Scope, WorldError};
use egui::{pos2, vec2, Align2, Color32, CursorIcon, Id, Rect, Sense, Ui};
use orbiters_core::launcher::{Action, InstanceAccess, Invite, InviteKind, LaunchMode, Place, Source};
use orbiters_core::vrchat_api::{CreateAccess, InstanceRegion};
use orbiters_i18n::{tr, tr_ctx};
use orbiters_ui::anim::{self, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, paint, popup, widgets};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// An invite is offered while it is in the account's list and younger than this (an older one's instance has most
/// likely emptied; it stays in the friends list's invites as before).
pub const INVITE_FOR: i64 = 3 * 3600;
/// An event is offered from this long before its start until its end.
pub const EVENT_AHEAD: i64 = 90 * 60;
/// An event that says no end is taken to last this long.
pub const EVENT_LONG: i64 = 3 * 3600;
/// Visits older than this do not count for the places visited most.
pub const FREQUENT_DAYS: u64 = 60;
/// A visit counts half as much every this many days.
pub const HALF_LIFE_DAYS: f64 = 14.0;
/// The places visited most that are offered.
pub const FREQUENT_MAX: usize = 3;
/// Invites and events offered at most.
const INVITES_MAX: usize = 4;
const EVENTS_MAX: usize = 3;
const DAY: u64 = 86_400;
/// How often the history is read again while Home shows, the events asked again, the last instance looked at again.
const HISTORY_EVERY: Duration = Duration::from_secs(60);
const EVENTS_EVERY: Duration = Duration::from_secs(300);
const OPEN_EVERY: Duration = Duration::from_secs(120);
/// How long a quiet line (a card that went, an instance that could not be made) stays.
const NOTICE_FOR: f64 = 10.0;

/// The heading's row, the room under it, the cards' sizes and gaps.
const HEAD_H: f32 = 30.0;
const HEAD_GAP: f32 = 10.0;
const CARD_MIN_W: f32 = 176.0;
const CARD_MAX_W: f32 = 236.0;
const CARD_GAP: f32 = 12.0;
const BODY_H: f32 = 58.0;
const HINT_H: f32 = 26.0;
/// The Play heading under the row (over the Play buttons), and the room above it.
const PLAY_HEAD_H: f32 = 26.0;
const PLAY_HEAD_GAP: f32 = 22.0;
const RADIUS: f32 = 16.0;

// ---------------------------------------------------------------------------------------------------------------------
// What is offered.

/// Why a card is offered: its line under the name.
#[derive(Clone, Debug, PartialEq)]
pub enum Reason {
    Home,
    Invite { from: String, message: String },
    /// `starts_at` past: on now.
    Event { title: String, starts_at: i64 },
    /// The last instance of the history; `rejoin`: its own instance, else a new one of its world.
    Recent { rejoin: bool },
    /// Visited this many times in the last [`FREQUENT_DAYS`] days.
    Frequent { visits: u32 },
}

/// Where Play goes with a card.
#[derive(Clone, Debug, PartialEq)]
pub enum Go {
    /// VRChat's own start, at its home world.
    Home,
    /// An instance that is there now (an event's, the last one).
    Instance(Place),
    /// The invite's own instance, accepting the invite `id`.
    Invite { id: String, place: Place },
    /// A new instance of the card's world, of the kind its card says.
    New,
    /// An event with no instance yet: VRChat starts at home, and the event's invite arrives there.
    EventWaiting,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// Which card it is, whatever it goes to now: `home`, `invite:<id>`, `event:<id>`, `recent:<world>`, `often:<world>`.
    pub key: String,
    /// `wrld_…`, empty for Home (and an event that names no world).
    pub world_id: String,
    pub name: String,
    /// Its picture's address when the source gave one (an invite's world, an event's cover); else the world's own,
    /// through the Map explorer's pages.
    pub picture: Option<String>,
    pub reason: Reason,
    pub go: Go,
    /// The region of its instance, or of a new one (`us`, `use`, `eu`, `jp`).
    pub region: String,
}

impl Suggestion {
    /// Whether its place comes from this computer alone (Home, the history), not from an account's list: such a card is
    /// started whatever account is on show.
    pub fn local(&self) -> bool {
        matches!(self.reason, Reason::Home | Reason::Frequent { .. } | Reason::Recent { rejoin: false })
    }

    /// Whether Play makes a new instance for it.
    pub fn new_instance(&self) -> bool {
        self.go == Go::New
    }
}

/// An event of Orbiters the member asked an invite for, on now or soon.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EventSoon {
    pub id: String,
    pub title: String,
    pub starts_at: i64,
    pub ends_at: Option<i64>,
    pub world_id: Option<String>,
    /// Its cover's address (an event's own picture on Orbiters), or its world's picture.
    pub picture: Option<String>,
    /// `wrld_…:<instance>` of its live instance, when Orbiters gives one.
    pub location: Option<String>,
}

impl EventSoon {
    /// Whether it is offered at `now`: from [`EVENT_AHEAD`] before its start to its end.
    pub fn offered(&self, now: i64) -> bool {
        let end = self.ends_at.unwrap_or(self.starts_at + EVENT_LONG);
        now >= self.starts_at - EVENT_AHEAD && now < end
    }
}

/// A world visited, with how much it counts.
#[derive(Clone, Debug, PartialEq)]
pub struct Frequent {
    pub world_id: String,
    pub name: String,
    /// Visits in the last [`FREQUENT_DAYS`] days.
    pub visits: u32,
    pub score: f64,
    pub last: u64,
    /// A picture handed in (pictures of the page); else the world's own.
    pub picture: Option<String>,
}

/// The last instance joined.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LastVisit {
    pub world_id: String,
    pub instance: String,
    pub name: String,
    pub at: u64,
    pub picture: Option<String>,
}

/// What the history gives Where.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryView {
    pub frequent: Vec<Frequent>,
    pub last: Option<LastVisit>,
}

impl HistoryView {
    pub fn of(history: &orbiters_core::visited::History, now: u64) -> Self {
        let mut frequent = frequent(history, now);
        frequent.truncate(FREQUENT_MAX + 4);
        let last = history.instances.last().filter(|v| orbiters_core::vrchat_launch::is_destination(&v.world_id, &v.instance)).map(|v| LastVisit {
            world_id: v.world_id.clone(),
            instance: v.instance.clone(),
            name: history.worlds.get(&v.world_id).and_then(|w| w.name.clone()).unwrap_or_default(),
            at: v.at,
            picture: None,
        });
        Self { frequent, last }
    }

    /// The region new instances are made in: the last instance's (where the person plays), else VRChat's own default.
    pub fn region(&self) -> &'static str {
        crate::overlay::worlds::finder::region(self.last.as_ref().map(|l| l.instance.as_str()))
    }
}

/// The worlds of `history` by how much they were visited lately: every visit of the last [`FREQUENT_DAYS`] days counts
/// `0.5 ^ (age in days / HALF_LIFE_DAYS)`; the most first, then the one visited last, then by id. Worlds with no such
/// visit, and what is not a world's id, are left out.
pub fn frequent(history: &orbiters_core::visited::History, now: u64) -> Vec<Frequent> {
    let from = now.saturating_sub(FREQUENT_DAYS * DAY);
    let mut out: Vec<Frequent> = history
        .worlds
        .iter()
        .filter(|(id, _)| orbiters_core::vrchat_log::is_world_id(id))
        .filter_map(|(id, w)| {
            // A clock set back leaves visits "in the future": they count as today's.
            let counted: Vec<u64> = w.visits.iter().copied().filter(|t| *t >= from).collect();
            if counted.is_empty() {
                return None;
            }
            let score = counted.iter().map(|t| 0.5f64.powf(now.saturating_sub(*t) as f64 / (HALF_LIFE_DAYS * DAY as f64))).sum();
            Some(Frequent { world_id: id.clone(), name: w.name.clone().unwrap_or_default(), visits: counted.len() as u32, score, last: w.last().unwrap_or(0), picture: None })
        })
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then(b.last.cmp(&a.last)).then_with(|| a.world_id.cmp(&b.world_id)));
    out
}

/// Whether the account `me` can join `instance` again by its link alone: a public one (its group's too), a friends one,
/// or one of its own. An invite instance of someone else, or a group's members-only one, needs an invite.
pub fn rejoinable(instance: &str, me: &str) -> bool {
    let (access, _) = orbiters_core::launcher_feed::access(instance);
    match access {
        InstanceAccess::Public | InstanceAccess::GroupPublic | InstanceAccess::FriendsPlus | InstanceAccess::Friends => true,
        InstanceAccess::Invite | InstanceAccess::InvitePlus => !me.is_empty() && instance.contains(&format!("~private({me})")),
        InstanceAccess::GroupPlus | InstanceAccess::Group | InstanceAccess::Unknown => false,
    }
}

/// What the cards are made from.
pub struct Inputs<'a> {
    pub now: i64,
    /// The account's invites, newest first, and those put away here.
    pub invites: &'a [Invite],
    pub dismissed: &'a dyn Fn(&str) -> bool,
    pub events: &'a [EventSoon],
    pub history: Option<&'a HistoryView>,
    /// Whether VRChat says the last instance is open (`None`: not known).
    pub last_open: Option<bool>,
    /// The VRChat account signed in and on show, whose places these are for (its user id); `None`: none, or the profile
    /// chosen is another account's.
    pub me: Option<&'a str>,
}

/// The cards, in the order shown: Home, then the invites, the events, the last instance and the places visited most,
/// each world once with its strongest reason.
pub fn suggestions(i: &Inputs) -> Vec<Suggestion> {
    let mut out = vec![Suggestion { key: "home".into(), world_id: String::new(), name: tr("Home world").into(), picture: None, reason: Reason::Home, go: Go::Home, region: String::new() }];
    let region = i.history.map_or("us", HistoryView::region).to_string();
    let taken = |out: &[Suggestion], world: &str| !world.is_empty() && out.iter().any(|s| s.world_id == world && matches!(s.go, Go::Instance(_) | Go::Invite { .. } | Go::New));
    if i.me.is_some() {
        let fresh = i.invites.iter().filter(|v| v.kind == InviteKind::Invite && !(i.dismissed)(&v.id) && (v.received_at <= 0 || i.now - v.received_at <= INVITE_FOR));
        for v in fresh {
            let Some(place) = v.place.clone().filter(|p| orbiters_core::vrchat_launch::is_destination(&p.world_id, &p.instance_id)) else { continue };
            if taken(&out, &place.world_id) || out.iter().filter(|s| matches!(s.reason, Reason::Invite { .. })).count() >= INVITES_MAX {
                continue;
            }
            let name = if place.world_name.trim().is_empty() { tr("A world").to_string() } else { place.world_name.clone() };
            out.push(Suggestion {
                key: format!("invite:{}", v.id),
                world_id: place.world_id.clone(),
                name,
                picture: (!place.world_picture.is_empty()).then(|| place.world_picture.clone()),
                reason: Reason::Invite { from: v.from_name.clone(), message: v.message.clone() },
                region: place.region.clone(),
                go: Go::Invite { id: v.id.clone(), place },
            });
        }
    }
    let mut events: Vec<&EventSoon> = i.events.iter().filter(|e| e.offered(i.now)).collect();
    events.sort_by_key(|e| e.starts_at);
    for e in events.into_iter().take(EVENTS_MAX) {
        let place = e.location.as_deref().and_then(|l| orbiters_core::launcher_feed::place(l, ""));
        let world = place.as_ref().map(|p| p.world_id.clone()).or_else(|| e.world_id.clone()).unwrap_or_default();
        if place.is_some() && taken(&out, &world) {
            continue;
        }
        out.push(Suggestion {
            key: format!("event:{}", e.id),
            world_id: world,
            name: e.title.clone(),
            picture: e.picture.clone(),
            reason: Reason::Event { title: e.title.clone(), starts_at: e.starts_at },
            region: place.as_ref().map(|p| p.region.clone()).unwrap_or_else(|| region.clone()),
            go: place.map_or(Go::EventWaiting, Go::Instance),
        });
    }
    if let Some(last) = i.history.and_then(|h| h.last.as_ref()) {
        if !taken(&out, &last.world_id) {
            let rejoin = i.me.is_some_and(|me| i.last_open == Some(true) && rejoinable(&last.instance, me));
            let place = orbiters_core::launcher_feed::place(&format!("{}:{}", last.world_id, last.instance), &last.name);
            let (go, region) = match place {
                Some(p) if rejoin => {
                    let r = p.region.clone();
                    (Go::Instance(p), r)
                }
                _ => (Go::New, region.clone()),
            };
            out.push(Suggestion {
                key: format!("recent:{}", last.world_id),
                world_id: last.world_id.clone(),
                name: world_name(&last.name),
                picture: last.picture.clone(),
                reason: Reason::Recent { rejoin: matches!(go, Go::Instance(_)) },
                go,
                region,
            });
        }
    }
    let mut often = 0;
    for f in i.history.map(|h| h.frequent.as_slice()).unwrap_or_default() {
        if often == FREQUENT_MAX {
            break;
        }
        if taken(&out, &f.world_id) {
            continue;
        }
        often += 1;
        out.push(Suggestion {
            key: format!("often:{}", f.world_id),
            world_id: f.world_id.clone(),
            name: world_name(&f.name),
            picture: f.picture.clone(),
            reason: Reason::Frequent { visits: f.visits },
            go: Go::New,
            region: region.clone(),
        });
    }
    out
}

fn world_name(name: &str) -> String {
    if name.trim().is_empty() { tr("Unnamed world").into() } else { name.to_string() }
}

// ---------------------------------------------------------------------------------------------------------------------
// The choice.

/// The card chosen (Home when none).
#[derive(Clone, Debug, PartialEq)]
pub struct Chosen {
    pub key: String,
    pub name: String,
    /// It went to an instance that was there (and not a new one).
    pub instance: bool,
}

/// The chosen card against the cards of now: kept while its card is there and still goes where it went; else the
/// choice goes back to Home, and the card's name is returned for the line that says so. A card that went to a new
/// instance and now finds its old one open again keeps the choice (the person asked for that world).
pub fn settle(chosen: &mut Option<Chosen>, cards: &[Suggestion]) -> Option<String> {
    let c = chosen.as_ref()?;
    let gone = match cards.iter().find(|s| s.key == c.key) {
        None => true,
        Some(s) => c.instance && !matches!(s.go, Go::Instance(_) | Go::Invite { .. }),
    };
    if gone {
        return chosen.take().map(|c| c.name);
    }
    None
}

/// What Play does with a card.
#[derive(Clone, Debug, PartialEq)]
pub enum Launch {
    /// VRChat's own start.
    Plain,
    /// Into this instance.
    At(Place),
    /// Accepting this invite (into its own instance).
    Invite(String),
    /// A new instance made by VRChat for the signed-in account first, then into it.
    Create { world: String, name: String, access: CreateAccess, region: InstanceRegion },
}

/// What Play does with `card`: `access` the kind of a new instance, `can_create` whether VRChat can make one for the
/// account signed in (else a new instance is a public one named here from `seed`, as the 3D worlds' visited list does).
pub fn launch(card: &Suggestion, access: CreateAccess, can_create: bool, seed: u64) -> Launch {
    match &card.go {
        Go::Home | Go::EventWaiting => Launch::Plain,
        Go::Instance(p) => Launch::At(p.clone()),
        Go::Invite { id, .. } => Launch::Invite(id.clone()),
        Go::New if can_create => Launch::Create {
            world: card.world_id.clone(),
            name: card.name.clone(),
            access,
            region: crate::overlay::worlds::panel::go::region_of(&card.region).unwrap_or(InstanceRegion::Us),
        },
        Go::New => {
            let region = if crate::overlay::worlds::panel::go::region_of(&card.region).is_some() { card.region.as_str() } else { "us" };
            let instance = crate::overlay::worlds::finder::new_instance(region, seed);
            Launch::At(Place { world_id: card.world_id.clone(), world_name: card.name.clone(), world_picture: String::new(), instance_id: instance, access: InstanceAccess::Public, region: region.into() })
        }
    }
}

/// The action Play sends for `launch` in `mode` on `profile` (`None`: a Mac, one VRChat, no profile choice); none for a
/// new instance still to be made.
pub fn action(launch: &Launch, mode: LaunchMode, profile: Option<u32>) -> Option<Action> {
    Some(match (launch, profile) {
        (Launch::Plain, Some(profile)) => Action::PlayOn { mode, profile },
        (Launch::Plain, None) => Action::Play { mode, destination: None },
        (Launch::At(place), Some(profile)) => Action::PlayAt { mode, profile, destination: place.clone() },
        (Launch::At(place), None) => Action::Play { mode, destination: Some(place.clone()) },
        (Launch::Invite(id), Some(profile)) => Action::AcceptInviteOn { invite: id.clone(), mode, profile },
        (Launch::Invite(id), None) => Action::AcceptInvite { invite: id.clone(), mode },
        (Launch::Create { .. }, _) => return None,
    })
}

/// The kinds a new instance can be of (the overlay's Go there's), each as VRChat names it, and who may join.
fn kinds() -> [(CreateAccess, InstanceAccess, &'static str); 5] {
    [
        (CreateAccess::Public, InstanceAccess::Public, tr("anyone")),
        (CreateAccess::FriendsPlus, InstanceAccess::FriendsPlus, tr("your friends and theirs")),
        (CreateAccess::Friends, InstanceAccess::Friends, tr("your friends")),
        (CreateAccess::InvitePlus, InstanceAccess::InvitePlus, tr("those invited, who can ask")),
        (CreateAccess::Invite, InstanceAccess::Invite, tr("only those you invite")),
    ]
}

fn kind_access(access: CreateAccess) -> InstanceAccess {
    kinds().into_iter().find(|k| k.0 == access).map_or(InstanceAccess::Public, |k| k.1)
}

// ---------------------------------------------------------------------------------------------------------------------
// Reading what the cards are made of (on threads: Home never waits).

/// Something read on a thread, with when.
struct Read<T> {
    value: Option<T>,
    at: Option<Instant>,
    busy: bool,
}

impl<T> Default for Read<T> {
    fn default() -> Self {
        Self { value: None, at: None, busy: false }
    }
}

type Shared<T> = Arc<Mutex<Read<T>>>;

/// Starts `work` on a thread when `every` passed since the last read (and none is under way); its value lands in
/// `slot`, and the page is drawn again.
fn refresh<T: Send + 'static>(slot: &Shared<T>, every: Duration, ctx: &egui::Context, name: &str, work: impl FnOnce() -> Option<T> + Send + 'static) {
    {
        let mut s = slot.lock().unwrap_or_else(|e| e.into_inner());
        if s.busy || s.at.is_some_and(|at| at.elapsed() < every) {
            return;
        }
        s.busy = true;
    }
    let (shared, ctx) = (slot.clone(), ctx.clone());
    let started = std::thread::Builder::new().name(name.into()).spawn(move || {
        let value = work();
        let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
        if value.is_some() {
            s.value = value;
        }
        s.at = Some(Instant::now());
        s.busy = false;
        drop(s);
        ctx.request_repaint();
    });
    if started.is_err() {
        slot.lock().unwrap_or_else(|e| e.into_inner()).busy = false;
    }
}

/// The member's events on now or soon, from Orbiters (signed in there; nothing otherwise).
fn read_events(server: orbiters_core::account::Server, now: i64) -> Vec<EventSoon> {
    let Some(account) = orbiters_core::account::read(server) else { return Vec::new() };
    let api = orbiters_core::account::Api::new(server);
    let Ok(cards) = api.events(0) else { return Vec::new() };
    let mut out = Vec::new();
    // The few on now or soon: an answer each about the member's invites, one more for the live instance.
    for card in cards.items.iter().filter(|c| unix(c.starts_at.as_deref()).is_some_and(|s| EventSoon { starts_at: s, ends_at: unix(c.ends_at.as_deref()), ..Default::default() }.offered(now))).take(6) {
        let Ok(invites) = api.my_invites(&account.token, &card.id) else { continue };
        if !interested(&invites) {
            continue;
        }
        let location = api.event(Some(&account.token), &card.id).ok().and_then(|o| event_location(&o));
        let world = card.world.as_ref();
        out.push(EventSoon {
            id: card.id.clone(),
            title: card.title.clone(),
            starts_at: unix(card.starts_at.as_deref()).unwrap_or(now),
            ends_at: unix(card.ends_at.as_deref()),
            world_id: world.and_then(|w| w.id.clone()).filter(|w| orbiters_core::vrchat_log::is_world_id(w)),
            picture: world.and_then(|w| w.image_url.clone()).filter(|u| u.starts_with("https://")),
            location,
        });
    }
    out
}

fn unix(iso: Option<&str>) -> Option<i64> {
    time::OffsetDateTime::parse(iso?, &time::format_description::well_known::Rfc3339).ok().map(|t| t.unix_timestamp())
}

/// Whether the member asked an invite for one of an event's steps (signed up, and not withdrawn since).
pub fn interested(invites: &orbiters_core::community::Invites) -> bool {
    invites.items.values().any(|status| status != "withdrawn")
}

/// An event's instance to go to: its live step's, else the event's own; only one of VRChat's own locations.
pub fn event_location(o: &orbiters_core::community::Overview) -> Option<String> {
    let live = o.steps.iter().find(|s| s.live).and_then(|s| s.instance_url.as_deref());
    let location = live.or(o.instance_url.as_deref()).and_then(orbiters_core::community::launch_location)?;
    let (world, instance) = location.split_once(':')?;
    orbiters_core::vrchat_launch::is_destination(world, instance).then_some(location)
}

// ---------------------------------------------------------------------------------------------------------------------
// The page's state.

/// Where's own state on Home's launcher.
#[derive(Default)]
pub struct Places {
    history: Shared<HistoryView>,
    events: Shared<Vec<EventSoon>>,
    /// Whether the last instance is open, for its location.
    open: Shared<(String, Option<bool>)>,
    chosen: Option<Chosen>,
    /// The kind of a new instance chosen on a card, by world (Friends+ otherwise).
    kinds: HashMap<String, CreateAccess>,
    /// The page of cards shown when they do not all fit.
    page: usize,
    /// A quiet line and when it was said (egui's clock).
    notice: Option<(String, f64)>,
    /// A new instance being made for Play, the mode pressed (its button turns), and its answer.
    making: Option<LaunchMode>,
    made: Arc<Mutex<Option<Result<(), String>>>>,
    /// The cards drawn last (the Play press uses the very card shown).
    cards: Vec<Suggestion>,
    /// Whether the row showed in the last frame (the invite card under Play then stands aside).
    pub(super) showing: bool,
    /// What the Map explorer says of the worlds (their pictures), once asked.
    infos: Option<crate::worlds::Infos>,
    /// Pictures of the page: what is handed in instead of being read.
    staged: Option<Staged>,
}

struct Staged {
    signed_in: bool,
    loading: bool,
}

impl Places {
    /// Pictures of the page: the history, the events and the last instance's state handed in; `signed_in` whether
    /// VRChat could make instances for the account. Nothing is read.
    pub fn stand_in(&mut self, history: Option<HistoryView>, events: Vec<EventSoon>, last_open: Option<bool>, signed_in: bool) {
        let loading = history.is_none();
        let at = Some(Instant::now());
        *self.history.lock().unwrap() = Read { value: history, at, busy: false };
        *self.events.lock().unwrap() = Read { value: Some(events), at, busy: false };
        let location = self.history.lock().unwrap().value.as_ref().and_then(|h| h.last.as_ref()).map(|l| format!("{}:{}", l.world_id, l.instance)).unwrap_or_default();
        *self.open.lock().unwrap() = Read { value: Some((location, last_open)), at, busy: false };
        self.staged = Some(Staged { signed_in, loading });
    }

    /// Pictures of the page: the card `key` chosen.
    pub fn stand_in_choose(&mut self, key: &str, name: &str, instance: bool) {
        self.chosen = Some(Chosen { key: key.into(), name: name.into(), instance });
    }

    /// Pictures of the page: a new instance's kind chosen for `world`.
    pub fn stand_in_kind(&mut self, world: &str, access: CreateAccess) {
        self.kinds.insert(world.into(), access);
    }

    /// Pictures of the page: the line under the cards.
    pub fn stand_in_notice(&mut self, text: &str, clock: f64) {
        self.notice = Some((text.into(), clock));
    }

    /// Pictures of the page: the list of kinds open on the card `key`.
    pub fn stand_in_kind_list(&mut self, ctx: &egui::Context, key: &str) {
        popup::open(ctx, Id::unique(("where-kind", key)).with("list"));
    }

    /// Pictures of the page: a new instance being made for Play in `mode`.
    pub fn stand_in_making(&mut self, mode: LaunchMode) {
        self.making = Some(mode);
    }

    /// Pictures of the page: the page of cards shown.
    pub fn stand_in_page(&mut self, page: usize) {
        self.page = page;
    }

    /// The chosen card's key, and the quiet line under the cards (tests).
    #[cfg(test)]
    pub fn chosen_key(&self) -> Option<&str> {
        self.chosen.as_ref().map(|c| c.key.as_str())
    }

    #[cfg(test)]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_ref().map(|n| n.0.as_str())
    }

    /// A new instance is being made for Play: the Play button of `mode` turns.
    pub fn making(&self) -> Option<LaunchMode> {
        self.making
    }
}

/// The account whose places are offered: the one signed in and on show, when the profile chosen is its own (or says
/// no account); and whether VRChat can make instances for it.
fn account(state: &State, cx: &Cx, profile_account: Option<&str>) -> (Option<String>, bool) {
    let staged = state.places.staged.as_ref();
    let me = match (&state.data.source, state.stale()) {
        (Source::SignedIn { user_id, name }, false) if !user_id.is_empty() => {
            let own = profile_account.is_none_or(|a| a.is_empty() || name.is_empty() || a == name);
            own.then(|| user_id.clone())
        }
        _ => None,
    };
    let can_create = match staged {
        Some(s) => s.signed_in && me.is_some(),
        None => me.as_deref().is_some_and(|me| cx.snap.vrchat_login.allowed && cx.snap.vrchat_login.user.as_ref().is_some_and(|u| u.id == me)),
    };
    (me, can_create)
}

/// The cards now, after the reads have been started where due.
fn cards(ui: &Ui, cx: &Cx, state: &mut State, me: Option<&str>) -> (Vec<Suggestion>, bool) {
    let ctx = ui.ctx().clone();
    let now = state.now();
    // Tests read nothing of this computer's or the network's (pictures of the page hand everything in).
    let reads = state.places.staged.is_none() && !cfg!(test);
    if reads {
        refresh(&state.places.history, HISTORY_EVERY, &ctx, "where-history", || {
            let history = orbiters_core::visited::History::load(&orbiters_core::visited::history_file());
            Some(HistoryView::of(&history, orbiters_core::visited::now()))
        });
        let server = orbiters_core::account::Server::current();
        refresh(&state.places.events, EVENTS_EVERY, &ctx, "where-events", move || Some(read_events(server, events_now())));
    }
    let history = state.places.history.lock().unwrap_or_else(|e| e.into_inner()).value.clone();
    let loading = match &state.places.staged {
        Some(s) => s.loading,
        None => history.is_none(),
    };
    let events = state.places.events.lock().unwrap_or_else(|e| e.into_inner()).value.clone().unwrap_or_default();
    // Whether the last instance is open: asked of VRChat (the overlay's own read) while a rejoin could be offered.
    let last = history.as_ref().and_then(|h| h.last.clone());
    let location = last.as_ref().map(|l| format!("{}:{}", l.world_id, l.instance)).unwrap_or_default();
    let mut last_open = None;
    if let (Some(last), Some(me)) = (&last, me) {
        if rejoinable(&last.instance, me) {
            let known = state.places.open.lock().unwrap_or_else(|e| e.into_inner()).value.clone();
            if reads {
                if known.as_ref().is_some_and(|k| k.0 != location) {
                    // Another instance since: asked about now.
                    let mut s = state.places.open.lock().unwrap_or_else(|e| e.into_inner());
                    s.at = None;
                    s.value = None;
                }
                if let Some(scope) = state.scope.clone() {
                    let (ctl, world, instance, key) = (cx.ctl.clone(), last.world_id.clone(), last.instance.clone(), location.clone());
                    refresh(&state.places.open, OPEN_EVERY, &ctx, "where-open", move || match ctl.vrchat_data(scope, Request::Instance { world, instance }) {
                        Ok(Response::Instance(open)) => Some((key, open)),
                        _ => Some((key, None)),
                    });
                }
            }
            last_open = state.places.open.lock().unwrap_or_else(|e| e.into_inner()).value.clone().filter(|k| k.0 == location).and_then(|k| k.1);
        }
    }
    let dismissed = |id: &str| state.dismissed.contains(id);
    let inputs = Inputs { now, invites: &state.data.invites, dismissed: &dismissed, events: &events, history: history.as_ref(), last_open, me };
    (suggestions(&inputs), loading)
}

/// The clock events are measured with (pictures of the page fix the launcher's).
fn events_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Whether the Where row shows now: VRChat can be started on the profile chosen (it does not run there).
fn showing(play: PlayState) -> bool {
    matches!(play, PlayState::Ready | PlayState::Starting(_) | PlayState::OtherStarting)
}

// ---------------------------------------------------------------------------------------------------------------------
// Drawing.

/// Where, as block `index` of Home, above the Play buttons; then the Play heading. Nothing while VRChat runs on the
/// profile chosen (the Play part says so, and an invite's card offers Join there), nor while nothing can be started.
pub fn row(ui: &mut Ui, cx: &mut Cx, state: &mut State, index: usize) {
    state.places.showing = false;
    if !state.shown() {
        return;
    }
    let (play, profile_account) = if orbiters_core::vrchat_launch::VR_HERE {
        let offered = super::states::profiles(&state.data, state.added_profile);
        let next = super::states::next_profile(&offered, state.chosen_profile);
        let account = offered.iter().find(|p| p.profile == next).and_then(|p| p.account.clone());
        (super::states::play_on(&state.data, &offered, next), account)
    } else {
        (super::states::play(&state.data), None)
    };
    if !showing(play) {
        return;
    }
    state.places.showing = true;
    let ctx = ui.ctx().clone();
    let clock = anim::time(&ctx);
    let (me, can_create) = account(state, cx, profile_account.as_deref());
    let (cards, loading) = cards(ui, cx, state, me.as_deref());
    if let Some(name) = settle(&mut state.places.chosen, &cards) {
        state.places.notice = Some((orbiters_i18n::tr!("{name} is no longer available: Play starts VRChat at home.", name = name), clock));
    }
    // The answer of a new instance being made.
    if let Some(result) = state.places.made.lock().unwrap_or_else(|e| e.into_inner()).take() {
        state.places.making = None;
        if let Err(e) = result {
            state.places.notice = Some((e, clock));
        }
    }
    state.places.cards = cards.clone();

    // The cards' room: as many as fit side by side, the rest on further pages (arrows), never a scroll of its own.
    let width = ui.available_width();
    let fit = (((width + CARD_GAP) / (CARD_MIN_W + CARD_GAP)).floor() as usize).max(1);
    let card_w = ((width - CARD_GAP * (fit as f32 - 1.0)) / fit as f32).min(CARD_MAX_W);
    let pic_h = (card_w * 9.0 / 16.0).round();
    let card_h = pic_h + BODY_H;
    let shown_cards: Vec<Suggestion> = if loading { cards.iter().take(1).cloned().collect() } else { cards.clone() };
    // Loading: Home and stand-ins; nothing visited yet: Home and a card that says what comes there.
    let extra = if loading { 2.min(fit.saturating_sub(1)) } else { usize::from(cards.len() == 1 && fit > 1) };
    let total = shown_cards.len() + extra;
    let pages = total.div_ceil(fit).max(1);
    state.places.page = state.places.page.min(pages - 1);
    let hint = hint(state, &cards, me.is_some(), can_create, clock, profile_account.as_deref());
    // Signed out of VRChat: the way to sign in, at the line's end.
    let offer_sign_in = !matches!(state.data.source, Source::SignedIn { .. }) && state.places.notice.is_none() && state.places.making.is_none();
    let mut sign_in = false;
    let height = HEAD_H + HEAD_GAP + card_h + if hint.is_some() { HINT_H + 4.0 } else { 0.0 } + PLAY_HEAD_GAP + PLAY_HEAD_H;
    super::super::block(ui, cx, index, height, |ui, rect| {
        // The heading: Where, and what is chosen.
        let p = ui.painter().clone();
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), HEAD_H));
        let title = widgets::text(&p, pos2(head.left(), head.center().y), Align2::LEFT_CENTER, &Txt::new(tr_ctx("launch", "Where"), 19.0).weight(weight::SEMIBOLD).tracking(-0.01));
        // A new instance says its kind with the name (the card's line no longer has room for it).
        let chosen_card = state.places.chosen.as_ref().and_then(|c| shown_cards.iter().find(|s| s.key == c.key));
        let chosen_name = match chosen_card {
            Some(card) if card.new_instance() => orbiters_i18n::tr!("{name} · new {kind} instance", name = card.name, kind = access_name(kind_access(kind_of(state, card, can_create)))),
            Some(card) => card.name.clone(),
            None => tr("Home world").to_string(),
        };
        let arrows_w = if pages > 1 { 2.0 * 30.0 + 52.0 } else { 0.0 };
        let selected = orbiters_i18n::tr!("Selected: {name}", name = chosen_name);
        widgets::text_truncated(&p, pos2(title.right() + 14.0, head.center().y + 1.0), Align2::LEFT_CENTER, &Txt::new(selected, 13.5).weight(weight::MEDIUM).color(theme::ACCENT), head.right() - title.right() - 26.0 - arrows_w);
        if pages > 1 {
            let next = Rect::from_center_size(pos2(head.right() - 15.0, head.center().y), vec2(30.0, 30.0));
            let prev = next.translate(vec2(-34.0, 0.0));
            let at = state.places.page;
            widgets::text(&p, pos2(prev.left() - 8.0, head.center().y), Align2::RIGHT_CENTER, &Txt::new(format!("{} / {pages}", at + 1), 12.5).color(theme::FG_3));
            // Both arrows always, the one that leads nowhere faded and inert (the row does not shift between pages).
            for (r, glyph, label, back) in [(prev, icons::CARET_LEFT, orbiters_i18n::mark("Previous places"), true), (next, icons::CARET_RIGHT, orbiters_i18n::mark("More places"), false)] {
                let can = if back { at > 0 } else { at + 1 < pages };
                if can {
                    if widgets::activated(&widgets::IconButton::new(glyph, label).size(30.0, 14.0).id_salt(("where-page", back)).show_at(ui, r)) {
                        state.places.page = if back { at - 1 } else { at + 1 };
                    }
                } else {
                    widgets::icon(&p, r.center(), glyph, 14.0, theme::FG_4);
                }
            }
        }
        // The cards of this page.
        let top = head.bottom() + HEAD_GAP;
        let first = state.places.page * fit;
        for slot in 0..fit {
            let i = first + slot;
            if i >= total {
                break;
            }
            let r = Rect::from_min_size(pos2(rect.left() + slot as f32 * (card_w + CARD_GAP), top), vec2(card_w, card_h));
            match shown_cards.get(i) {
                Some(card) => card_ui(ui, cx, state, r, pic_h, card, can_create, clock),
                None if loading => stand_in_card(ui, r, pic_h),
                None => empty_card(ui, r),
            }
        }
        if let Some((text, tone)) = &hint {
            let y = top + card_h + 4.0 + HINT_H * 0.5;
            let glyph = match tone {
                Tone::Quiet => icons::INFO,
                Tone::Busy => icons::HOURGLASS_MEDIUM,
                Tone::Warn => icons::WARNING_CIRCLE,
            };
            let color = match tone {
                Tone::Warn => theme::WARN,
                _ => theme::FG_3,
            };
            widgets::icon(&p, pos2(rect.left() + 8.0, y), glyph, 14.0, color);
            let mut room = rect.width() - 26.0;
            if offer_sign_in {
                let b = widgets::Button::new(tr("Sign in to VRChat")).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt("where-sign-in");
                let bw = b.desired_width(ui);
                if b.show_at(ui, Rect::from_min_size(pos2(rect.right() - bw, y - 14.0), vec2(bw, 28.0))).clicked() {
                    sign_in = true;
                }
                room -= bw + 8.0;
            }
            widgets::text_truncated(&p, pos2(rect.left() + 22.0, y), Align2::LEFT_CENTER, &Txt::new(text.as_str(), 12.5).color(theme::FG_2), room);
        }
        // The Play heading, under the row, over the buttons.
        let py = rect.bottom() - PLAY_HEAD_H * 0.5 - 2.0;
        widgets::text(&p, pos2(rect.left(), py), Align2::LEFT_CENTER, &Txt::new(tr_ctx("launch", "Play"), 19.0).weight(weight::SEMIBOLD).tracking(-0.01));
    });
    if sign_in {
        cx.go = Some(super::super::Go::VrchatAccount);
    }
    if state.places.notice.as_ref().is_some_and(|(_, at)| clock - at < NOTICE_FOR) {
        super::super::frame_in(&ctx, NOTICE_FOR - (clock - state.places.notice.as_ref().map_or(clock, |n| n.1)));
    }
}

enum Tone {
    Quiet,
    Busy,
    Warn,
}

/// The line under the cards, when there is something to say: an instance being made, a card that went or an instance
/// that could not be made (for a while), an event that waits for its invite, why invites and Friends+ are missing.
fn hint(state: &mut State, cards: &[Suggestion], signed_in: bool, can_create: bool, clock: f64, profile_account: Option<&str>) -> Option<(String, Tone)> {
    if let Some(mode) = state.places.making {
        let _ = mode;
        let kind = state.places.chosen.as_ref().and_then(|c| cards.iter().find(|s| s.key == c.key)).map(|s| kind_of(state, s, can_create)).unwrap_or(CreateAccess::FriendsPlus);
        return Some((orbiters_i18n::tr!("Opening a new {kind} instance…", kind = access_name(kind_access(kind))), Tone::Busy));
    }
    if let Some((text, at)) = &state.places.notice {
        if clock - at < NOTICE_FOR {
            return Some((text.clone(), Tone::Warn));
        }
        state.places.notice = None;
    }
    let chosen = state.places.chosen.as_ref().and_then(|c| cards.iter().find(|s| s.key == c.key));
    if chosen.is_some_and(|s| s.go == Go::EventWaiting) {
        return Some((tr("VRChat starts at home: the event's invite arrives there.").into(), Tone::Quiet));
    }
    if state.stale() {
        return None;
    }
    match (&state.data.source, signed_in) {
        (Source::SignedIn { name, .. }, false) if profile_account.is_some() => {
            Some((orbiters_i18n::tr!("Invites and Friends+ instances are for {name}: choose their profile to use them.", name = name), Tone::Quiet))
        }
        (_, false) => Some((tr("Sign in to VRChat to see invites here and open Friends+ instances.").into(), Tone::Quiet)),
        _ => None,
    }
}

/// The kind of a new instance for `card`: the one chosen on it, else Friends+; public only where VRChat cannot make one.
fn kind_of(state: &State, card: &Suggestion, can_create: bool) -> CreateAccess {
    if !can_create {
        return CreateAccess::Public;
    }
    state.places.kinds.get(&card.world_id).copied().unwrap_or(CreateAccess::FriendsPlus)
}

/// A card's line under its name (why it is offered), and the colour of its mark. What it goes to is on its picture: a
/// new instance's kind ("New · Friends+", to change), or the kind and region of the one that is there.
fn reason_line(card: &Suggestion, now: i64) -> (String, &'static str, Color32) {
    match &card.reason {
        Reason::Home => (tr("VRChat's usual start").into(), icons::HOUSE, theme::FG_3),
        Reason::Invite { from, .. } => (orbiters_i18n::tr!("Invite from {name}", name = from), icons::ENVELOPE_SIMPLE, theme::ACCENT),
        Reason::Event { starts_at, .. } if *starts_at <= now => (tr("Event · on now").into(), icons::CALENDAR_DOTS, theme::WARN),
        Reason::Event { starts_at, .. } => {
            let minutes = ((starts_at - now + 59) / 60).max(1);
            (orbiters_i18n::tr!("Event · starts in {minutes} min", minutes = minutes), icons::CALENDAR_DOTS, theme::WARN)
        }
        Reason::Recent { .. } => (tr("Last visited").into(), icons::CLOCK_COUNTER_CLOCKWISE, theme::FG_3),
        Reason::Frequent { .. } => (tr("Often visited").into(), icons::STAR, theme::FG_3),
    }
}

/// A card: its picture (the kind of instance on it), its name and its reason; ringed when chosen. A press chooses it.
#[allow(clippy::too_many_arguments)]
fn card_ui(ui: &mut Ui, cx: &Cx, state: &mut State, rect: Rect, pic_h: f32, card: &Suggestion, can_create: bool, clock: f64) {
    let ctx = ui.ctx().clone();
    let chosen = state.places.chosen.as_ref().map_or(card.go == Go::Home, |c| c.key == card.key);
    let id = Id::unique(("where-card", &card.key));
    let resp = ui.interact(rect, id, Sense::click());
    widgets::note_control(&ctx, &card.name, rect, true);
    let hover = anim::toggle(&ctx, id.with("hover"), resp.hovered(), Spring::SNAPPY);
    let ring = anim::toggle(&ctx, id.with("chosen"), chosen, Spring::SNAPPY);
    let p = ui.painter().clone();
    // The card, lifted a little on hover; the accent ring when chosen.
    let fill = theme::mix(theme::surface_soft(), theme::surface_strong(), 0.35 + 0.4 * hover);
    widgets::frame(&p, rect, RADIUS, fill);
    let pic = Rect::from_min_size(rect.min, vec2(rect.width(), pic_h));
    let radii = [RADIUS, RADIUS, 0.0, 0.0];
    match picture(state, card) {
        Some(t) => paint::squircle_texture(&p, pic, radii, t.id(), cover_uv(t.size_vec2(), pic.size()), Color32::WHITE),
        None => {
            // No picture (yet, or none): the name's colour as a soft wash, and what the card is.
            let c = if card.go == Go::Home { theme::ACCENT } else { widgets::name_color(&card.name) };
            let points = paint::squircle_points(pic, theme::squircle(RADIUS));
            paint::fill_convex(&p, &points, |at| {
                let t = ((at.x - pic.left()) / pic.width() + (at.y - pic.top()) / pic.height()) * 0.5;
                theme::mix(theme::alpha(c, 0.32), theme::INK_800, t)
            });
            p.rect_filled(Rect::from_min_max(pos2(pic.left(), pic.bottom() - RADIUS), pic.max), 0.0, theme::mix(theme::alpha(c, 0.32), theme::INK_800, 0.9));
            let glyph = match card.go {
                Go::Home => icons::HOUSE,
                _ => icons::GLOBE_HEMISPHERE_WEST,
            };
            widgets::icon(&p, pic.center(), glyph, 30.0, theme::alpha(theme::FG, 0.55));
        }
    }
    // The instance on the picture: the kind of a new one, to change; the kind and region of one that is there.
    let now = state.now();
    let kind = kind_of(state, card, can_create);
    match &card.go {
        Go::New => kind_pill(ui, state, pic, card, kind, can_create),
        Go::Instance(place) | Go::Invite { place, .. } => {
            let mut chips = access_name(place.access).to_string();
            if !place.region.is_empty() {
                chips = format!("{chips} · {}", region_tag(&place.region));
            }
            let txt = Txt::new(chips, 11.5).weight(weight::SEMIBOLD).color(theme::FG);
            let w = widgets::text_size(ui, &txt).x + 30.0;
            let chip = Rect::from_min_size(pic.left_bottom() + vec2(8.0, -30.0), vec2(w.min(pic.width() - 16.0), 22.0));
            p.rect_filled(chip, theme::radius(11.0), theme::black(0.6));
            widgets::icon(&p, pos2(chip.left() + 12.0, chip.center().y), access_icon(place.access), 12.0, theme::FG);
            widgets::text_truncated(&p, pos2(chip.left() + 22.0, chip.center().y), Align2::LEFT_CENTER, &txt, chip.width() - 28.0);
        }
        Go::Home | Go::EventWaiting => {}
    }
    // The chosen card's tick, top right.
    if ring > 0.01 {
        let c = pos2(pic.right() - 18.0, pic.top() + 18.0);
        p.circle_filled(c, 11.0 * ring, theme::ACCENT);
        widgets::icon(&p, c, icons::CHECK, 13.0 * ring, theme::ON_ACCENT);
    }
    // An invite can be put away here, as on its card.
    if let Go::Invite { id: invite, .. } = &card.go {
        if hover > 0.01 && !chosen {
            let close = Rect::from_center_size(pos2(pic.right() - 18.0, pic.top() + 18.0), vec2(26.0, 26.0));
            if widgets::activated(&widgets::IconButton::new(icons::X, orbiters_i18n::mark("Dismiss")).size(26.0, 12.0).backdrop(theme::black(0.55)).id_salt(("where-dismiss", invite)).show_at(ui, close)) {
                state.act(cx, clock, Action::Dismiss { id: invite.clone() });
            }
        }
    }
    // The name, and why it is offered.
    let x = rect.left() + 14.0;
    let room = rect.width() - 28.0;
    widgets::text_truncated(&p, pos2(x, pic.bottom() + 19.0), Align2::LEFT_CENTER, &Txt::new(card.name.as_str(), 14.5).weight(weight::SEMIBOLD).tracking(-0.01), room);
    let (line, glyph, tone) = reason_line(card, now);
    let y = pic.bottom() + 41.0;
    if matches!(card.reason, Reason::Invite { .. }) {
        p.circle_filled(pos2(x + 4.0, y), 4.0, theme::ACCENT);
    } else {
        widgets::icon(&p, pos2(x + 5.0, y), glyph, 12.5, tone);
    }
    let line_color = if matches!(card.reason, Reason::Invite { .. }) { theme::ACCENT } else { theme::FG_3 };
    widgets::text_truncated(&p, pos2(x + 16.0, y), Align2::LEFT_CENTER, &Txt::new(line, 12.5).weight(weight::MEDIUM).color(line_color), room - 16.0);
    if ring > 0.01 {
        paint::stroke_squircle(&p, rect.expand(3.0), theme::squircle(RADIUS + 3.0), egui::Stroke::new(2.0, theme::alpha(theme::ACCENT, ring)));
    } else if hover > 0.01 {
        paint::stroke_squircle(&p, rect, theme::squircle(RADIUS), egui::Stroke::new(1.0, theme::white(0.08 * hover)));
    }
    if widgets::activated(&resp) && state.places.making.is_none() {
        state.places.chosen = (card.go != Go::Home).then(|| Chosen { key: card.key.clone(), name: card.name.clone(), instance: matches!(card.go, Go::Instance(_) | Go::Invite { .. }) });
        state.places.notice = None;
    }
    let tip = match &card.reason {
        Reason::Invite { message, from } if !message.trim().is_empty() => format!("{from}: “{}”", message.trim()),
        Reason::Frequent { visits } => orbiters_i18n::tr_n("Visited once in the last 60 days", "Visited {count} times in the last 60 days", *visits as u64),
        Reason::Event { title, .. } => title.clone(),
        Reason::Home => tr("VRChat starts as it always does, in your home world.").into(),
        _ => card.name.clone(),
    };
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip);
}

/// The kind of a new instance on its card's picture: a pill whose list chooses among VRChat's kinds. Where VRChat cannot
/// make one for the account (no sign-in), only a public one can be: the pill says so and opens nothing.
fn kind_pill(ui: &mut Ui, state: &mut State, pic: Rect, card: &Suggestion, kind: CreateAccess, can_create: bool) {
    let ctx = ui.ctx().clone();
    let access = kind_access(kind);
    let label = access_name(access);
    let txt = Txt::new(orbiters_i18n::tr!("New · {kind}", kind = label), 11.5).weight(weight::SEMIBOLD).color(theme::FG);
    let w = widgets::text_size(ui, &txt).x + if can_create { 44.0 } else { 30.0 };
    let r = Rect::from_min_size(pic.left_bottom() + vec2(8.0, -30.0), vec2(w.min(pic.width() - 16.0), 22.0));
    let id = Id::unique(("where-kind", &card.key));
    let resp = ui.interact(r, id, if can_create { Sense::click() } else { Sense::hover() });
    let list = id.with("list");
    let hover = anim::toggle(&ctx, id.with("hover"), can_create && (resp.hovered() || popup::is_open(&ctx, list)), Spring::SNAPPY);
    let p = ui.painter();
    p.rect_filled(r, theme::radius(11.0), theme::mix(theme::black(0.6), theme::black(0.8), hover));
    if can_create {
        p.rect_stroke(r, theme::radius(11.0), egui::Stroke::new(1.0, theme::white(0.14 + 0.1 * hover)), egui::StrokeKind::Inside);
    }
    widgets::icon(p, pos2(r.left() + 12.0, r.center().y), access_icon(access), 12.0, theme::FG);
    widgets::text_truncated(p, pos2(r.left() + 22.0, r.center().y), Align2::LEFT_CENTER, &txt, r.width() - if can_create { 40.0 } else { 28.0 });
    if !can_create {
        resp.on_hover_text(tr("Sign in to VRChat in Settings to open Friends+, Friends or Invite instances: without it a new instance is public."));
        return;
    }
    widgets::icon(p, pos2(r.right() - 11.0, r.center().y), icons::CARET_DOWN, 10.0, theme::FG_2);
    widgets::note_control(&ctx, label, r, true);
    let items: Vec<popup::Item> = kinds().iter().map(|(_, a, says)| popup::Item::new(access_name(*a)).detail(says).icon(access_icon(*a))).collect();
    let selected = kinds().iter().position(|k| k.0 == kind);
    let pressed = widgets::activated(&resp);
    if let Some(i) = popup::list_from(&ctx, list, r, pressed, &items, selected, popup::list_width(ui, &items).max(220.0)) {
        state.places.kinds.insert(card.world_id.clone(), kinds()[i].0);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Who can join the new instance"));
}

/// A card's picture: the one its source gave (an invite's world through the launcher's pictures), else its world's on
/// the Map explorer (Orbiters' public pages, kept on this computer).
fn picture(state: &mut State, card: &Suggestion) -> Option<egui::TextureHandle> {
    if let Some(url) = &card.picture {
        if let Some(t) = state.pictures.get(url) {
            return Some(t.clone());
        }
        if url.starts_with("https://") && matches!(card.reason, Reason::Invite { .. }) {
            return state.picture(url);
        }
    }
    if state.places.staged.is_some() || card.world_id.is_empty() || cfg!(test) {
        return None;
    }
    let ctx = state.ctx.clone()?;
    let url = match &card.picture {
        Some(url) if url.starts_with("https://") => url.clone(),
        _ => {
            let infos = state.places.infos.get_or_insert_with(|| crate::worlds::Infos::new(&ctx));
            match infos.get(&card.world_id)? {
                crate::worlds::Info::Listed(page) => page.world.thumbnail_url.clone().or(page.world.image_url.clone()).filter(|u| u.starts_with("https://"))?,
                _ => return None,
            }
        }
    };
    let picture = crate::worlds::Picture::Remote { world_id: card.world_id.clone(), url, large: false };
    state.loader.as_mut()?.get(&ctx, &picture)
}

/// A card while the history is read.
fn stand_in_card(ui: &Ui, rect: Rect, pic_h: f32) {
    let p = ui.painter();
    let t = anim::clock(ui.ctx(), 30.0) as f32;
    let shimmer = 0.5 + 0.5 * (t * 2.4).sin();
    widgets::frame(p, rect, RADIUS, theme::mix(theme::surface_soft(), theme::surface_strong(), 0.2 + 0.15 * shimmer));
    let pic = Rect::from_min_size(rect.min, vec2(rect.width(), pic_h));
    paint::fill_convex(p, &paint::squircle_points(pic, theme::squircle(RADIUS)), |_| theme::white(0.03 + 0.02 * shimmer));
    p.rect_filled(Rect::from_min_size(pos2(rect.left() + 14.0, pic.bottom() + 13.0), vec2(rect.width() * 0.55, 12.0)), theme::radius(6.0), theme::white(0.06));
    p.rect_filled(Rect::from_min_size(pos2(rect.left() + 14.0, pic.bottom() + 35.0), vec2(rect.width() * 0.75, 10.0)), theme::radius(5.0), theme::white(0.04));
    ui.ctx().request_repaint_after(Duration::from_millis(33));
}

/// The place of the visited places while there are none yet: what comes there.
fn empty_card(ui: &Ui, rect: Rect) {
    let p = ui.painter();
    paint::stroke_squircle(p, rect.shrink(0.5), theme::squircle(RADIUS), egui::Stroke::new(1.0, theme::white(0.1)));
    widgets::icon(p, pos2(rect.center().x, rect.center().y - 22.0), icons::STAR, 22.0, theme::FG_4);
    let txt = Txt::new(tr("The places you visit most will show up here."), 12.5).color(theme::FG_3);
    let mut job = egui::text::LayoutJob::single_section(txt.text.clone(), txt.format());
    job.wrap.max_width = rect.width() - 36.0;
    job.halign = egui::Align::Center;
    let galley = p.layout_job(job);
    p.galley(pos2(rect.center().x, rect.center().y + 2.0), galley, txt.color);
}

// ---------------------------------------------------------------------------------------------------------------------
// Play.

/// Play pressed in `mode`, on `profile` (`None`: a Mac): VRChat starts in the chosen card's place, or as it always does.
/// A new instance is first made by VRChat for the account (on a thread: the button turns meanwhile).
pub fn play(state: &mut State, cx: &Cx, clock: f64, mode: LaunchMode, profile: Option<u32>) {
    if state.places.making.is_some() {
        return;
    }
    let card = state.places.chosen.as_ref().filter(|_| state.places.showing).and_then(|c| state.places.cards.iter().find(|s| s.key == c.key)).cloned();
    let Some(card) = card else {
        let action = action(&Launch::Plain, mode, profile).expect("a plain start");
        return state.act(cx, clock, action);
    };
    let profile_account = profile.and_then(|n| state.data.profiles.iter().find(|p| p.profile == n)).and_then(|p| p.account.clone());
    let (me, can_create) = account(state, cx, profile_account.as_deref());
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let plan = launch(&card, kind_of(state, &card, can_create), can_create, seed);
    if let Some(action) = action(&plan, mode, profile) {
        return state.act(cx, clock, action);
    }
    let (Launch::Create { world, name, access, region }, Some(scope), Some(me)) = (plan, state.scope.clone(), me) else { return };
    state.places.making = Some(mode);
    let (ctl, made, ctx) = (cx.ctl.clone(), state.places.made.clone(), state.ctx.clone());
    let started = std::thread::Builder::new().name("where-new-instance".into()).spawn(move || {
        let result = make_and_go(ctl.as_ref(), scope, &me, &world, &name, access, region, mode, profile);
        *made.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    });
    if started.is_err() {
        state.places.making = None;
    }
}

/// Has VRChat make the new instance for the account `me` (the overlay's Go there's request), then starts VRChat in it:
/// the launch bound to that account, which the service checks again before anything goes.
#[allow(clippy::too_many_arguments)]
fn make_and_go(ctl: &dyn crate::backend::Backend, scope: Scope, me: &str, world: &str, name: &str, access: CreateAccess, region: InstanceRegion, mode: LaunchMode, profile: Option<u32>) -> Result<(), String> {
    let made = match ctl.vrchat_data(scope, Request::CreateInstance { world: world.into(), access, region }) {
        Ok(Response::CreatedInstance(made)) => made,
        Ok(Response::WorldError(e)) => return Err(orbiters_i18n::tr!("The instance could not be made: {e}", e = world_error(&e))),
        Ok(_) => return Err(tr("VRChat answered something unexpected.").into()),
        Err(e) => return Err(orbiters_i18n::tr!("The instance could not be made: {e}", e = e)),
    };
    let place = Place { world_id: made.world.clone(), world_name: name.into(), world_picture: String::new(), instance_id: made.id.clone(), access: made.access, region: made.region.clone() };
    let action = action(&Launch::At(place), mode, profile).expect("a start into the instance");
    if ctl.launcher(Action::As { account: me.into(), action: Box::new(action) }) { Ok(()) } else { Err(tr("Orbiters - N4N0's service did not answer: try again.").into()) }
}

fn world_error(e: &WorldError) -> String {
    match e {
        WorldError::AccountChanged => tr("Your VRChat account changed: try again.").into(),
        WorldError::SignInRequired => tr("Sign in to VRChat in Orbiters to go there.").into(),
        WorldError::InvalidWorld => tr("VRChat does not know this world.").into(),
        WorldError::Api(e) => e.to_string(),
        _ => tr("VRChat answered something unexpected.").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::visited::{History, InstanceVisit, WorldVisits};

    const NOW: i64 = 1_795_000_000;
    const ME: &str = "usr_00000000-0000-4000-8000-0000000000ff";

    fn world(n: u32) -> String {
        format!("wrld_00000000-0000-4000-8000-{n:012}")
    }

    fn invite(id: &str, n: u32, received_at: i64) -> Invite {
        Invite {
            id: id.into(),
            kind: InviteKind::Invite,
            from_id: "usr_00000000-0000-4000-8000-000000000001".into(),
            from_name: "Sample Friend".into(),
            from_icon: String::new(),
            place: orbiters_core::launcher_feed::place(&format!("{}:777~friends(usr_00000000-0000-4000-8000-000000000001)~region(jp)", world(n)), "Chill House"),
            message: String::new(),
            received_at,
        }
    }

    fn often(n: u32, visits: u32) -> Frequent {
        Frequent { world_id: world(n), name: format!("World {n}"), visits, score: visits as f64, last: 0, picture: None }
    }

    fn view(last: Option<(u32, &str)>, frequent: &[u32]) -> HistoryView {
        HistoryView {
            frequent: frequent.iter().map(|n| often(*n, 10 - *n)).collect(),
            last: last.map(|(n, instance)| LastVisit { world_id: world(n), instance: instance.into(), name: format!("World {n}"), at: 0, picture: None }),
        }
    }

    fn keys(cards: &[Suggestion]) -> Vec<String> {
        cards.iter().map(|c| c.key.clone()).collect()
    }

    fn inputs<'a>(invites: &'a [Invite], events: &'a [EventSoon], history: Option<&'a HistoryView>, last_open: Option<bool>, me: Option<&'a str>) -> Inputs<'a> {
        Inputs { now: NOW, invites, dismissed: &|_| false, events, history, last_open, me }
    }

    #[test]
    fn home_first_then_invites_events_the_last_instance_and_the_places_visited_most() {
        let invites = [invite("not_1", 1, NOW - 60)];
        let events = [EventSoon { id: "ev1".into(), title: "Sample Meetup".into(), starts_at: NOW + 600, location: Some(format!("{}:5~region(eu)", world(2))), ..Default::default() }];
        let history = view(Some((3, "9~hidden(usr_x)~region(eu)")), &[4, 5, 6, 7]);
        let cards = suggestions(&inputs(&invites, &events, Some(&history), None, Some(ME)));
        assert_eq!(keys(&cards), ["home", "invite:not_1", "event:ev1", &format!("recent:{}", world(3)), &format!("often:{}", world(4)), &format!("often:{}", world(5)), &format!("often:{}", world(6))]);
        assert_eq!(cards[0].go, Go::Home);
        assert!(matches!(&cards[1].go, Go::Invite { id, place } if id == "not_1" && place.instance_id.starts_with("777~friends")));
        assert!(matches!(&cards[2].go, Go::Instance(p) if p.world_id == world(2)));
        assert_eq!(cards[3].go, Go::New, "the last instance's state is not known yet: a new instance of its world");
        assert!(cards[4..].iter().all(|c| c.go == Go::New && c.region == "eu"), "new ones in the region played last");
        assert_eq!(cards.iter().filter(|c| matches!(c.reason, Reason::Frequent { .. })).count(), FREQUENT_MAX);
    }

    #[test]
    fn a_world_offered_twice_shows_once_with_the_stronger_reason() {
        // The invite's world is also the last one and the most visited; the event's world is the second most visited.
        let invites = [invite("not_1", 4, NOW - 60)];
        let events = [EventSoon { id: "ev1".into(), title: "Sample Meetup".into(), starts_at: NOW - 60, location: Some(format!("{}:5~region(eu)", world(5))), ..Default::default() }];
        let history = view(Some((4, "9~region(eu)")), &[4, 5, 6, 7, 8]);
        let cards = suggestions(&inputs(&invites, &events, Some(&history), Some(true), Some(ME)));
        assert_eq!(keys(&cards), ["home", "invite:not_1", "event:ev1", &format!("often:{}", world(6)), &format!("often:{}", world(7)), &format!("often:{}", world(8))]);
        for w in [4, 5] {
            assert_eq!(cards.iter().filter(|c| c.world_id == world(w)).count(), 1, "world {w} once");
        }
        // Two invites to one world: the newest (first) only.
        let invites = [invite("not_new", 1, NOW - 10), invite("not_old", 1, NOW - 900)];
        let cards = suggestions(&inputs(&invites, &[], None, None, Some(ME)));
        assert_eq!(keys(&cards), ["home", "invite:not_new"]);
    }

    #[test]
    fn invites_need_the_account_and_are_offered_only_while_fresh() {
        let invites = [invite("not_1", 1, NOW - 60), invite("not_old", 2, NOW - INVITE_FOR - 1)];
        assert_eq!(keys(&suggestions(&inputs(&invites, &[], None, None, None))), ["home"], "signed out, or another account's profile: none");
        assert_eq!(keys(&suggestions(&inputs(&invites, &[], None, None, Some(ME)))), ["home", "invite:not_1"]);
        let dismissed = |id: &str| id == "not_1";
        let i = Inputs { dismissed: &dismissed, ..inputs(&invites, &[], None, None, Some(ME)) };
        assert_eq!(keys(&suggestions(&i)), ["home"], "put away here: gone");
        // A request to invite is not an invite; an invite without a place of VRChat's grammar is not offered.
        let mut request = invite("not_r", 3, NOW);
        request.kind = InviteKind::Request;
        let mut odd = invite("not_odd", 4, NOW);
        odd.place.as_mut().unwrap().instance_id = "1 --evil".into();
        assert_eq!(keys(&suggestions(&inputs(&[request, odd], &[], None, None, Some(ME)))), ["home"]);
    }

    #[test]
    fn events_are_offered_from_ninety_minutes_before_to_their_end() {
        let at = |starts: i64, ends: Option<i64>| EventSoon { id: "e".into(), title: "t".into(), starts_at: starts, ends_at: ends, ..Default::default() };
        assert!(at(NOW + EVENT_AHEAD, None).offered(NOW));
        assert!(!at(NOW + EVENT_AHEAD + 1, None).offered(NOW));
        assert!(at(NOW - 3600, Some(NOW + 1)).offered(NOW));
        assert!(!at(NOW - 3600, Some(NOW)).offered(NOW), "over");
        assert!(!at(NOW - EVENT_LONG, None).offered(NOW), "no end said: over after three hours");
        // Without an instance yet: VRChat starts at home (the invite comes there).
        let cards = suggestions(&inputs(&[], &[at(NOW + 600, None)], None, None, None));
        assert_eq!(cards[1].go, Go::EventWaiting);
        assert_eq!(launch(&cards[1], CreateAccess::FriendsPlus, true, 1), Launch::Plain);
        // The member's interest: a step signed up for, not withdrawn.
        let mut invites = orbiters_core::community::Invites::default();
        assert!(!interested(&invites));
        invites.items.insert("step1".into(), "withdrawn".into());
        assert!(!interested(&invites));
        invites.items.insert("step2".into(), "pending".into());
        assert!(interested(&invites));
    }

    #[test]
    fn an_events_location_is_its_live_steps_and_only_vrchats_own() {
        let mut o = orbiters_core::community::Overview::default();
        assert_eq!(event_location(&o), None);
        let w = world(9);
        o.instance_url = Some(format!("https://vrchat.com/home/launch?worldId={w}&instanceId=1~region(eu)"));
        assert_eq!(event_location(&o).as_deref(), Some(format!("{w}:1~region(eu)").as_str()));
        o.steps.push(orbiters_core::community::Step { live: true, instance_url: Some(format!("https://vrchat.com/home/launch?worldId={w}&instanceId=2~group(grp_00000000-0000-4000-8000-000000000001)~groupAccessType(public)~region(eu)")), ..Default::default() });
        assert!(event_location(&o).is_some_and(|l| l.ends_with(":2~group(grp_00000000-0000-4000-8000-000000000001)~groupAccessType(public)~region(eu)")));
        o.steps[0].instance_url = Some(format!("https://vrchat.com/home/launch?worldId={w}&instanceId=1%20--evil"));
        assert_eq!(event_location(&o), None, "not VRChat's grammar: nothing");
    }

    #[test]
    fn the_places_visited_most_count_recent_visits_more() {
        let now = NOW as u64;
        let mut h = History::default();
        let days = |d: &[u64]| d.iter().map(|d| now - d * DAY).collect::<Vec<_>>();
        // A: 6 visits two months ago and older; B: 3 visits this week; C: 5 visits a month ago; D: one yesterday.
        h.worlds.insert(world(1), WorldVisits { name: Some("A".into()), visits: days(&[90, 80, 70, 65, 62, 59]) });
        h.worlds.insert(world(2), WorldVisits { name: Some("B".into()), visits: days(&[6, 3, 1]) });
        h.worlds.insert(world(3), WorldVisits { name: Some("C".into()), visits: days(&[33, 32, 31, 30, 29]) });
        h.worlds.insert(world(4), WorldVisits { name: Some("D".into()), visits: days(&[1]) });
        h.worlds.insert("not a world".into(), WorldVisits { name: Some("x".into()), visits: days(&[1, 1, 1, 1]) });
        let f = frequent(&h, now);
        assert_eq!(f.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["B", "C", "D", "A"]);
        assert_eq!(f.iter().map(|f| f.visits).collect::<Vec<_>>(), [3, 5, 1, 1], "only the last 60 days count");
        // B: 0.5^(6/14) + 0.5^(3/14) + 0.5^(1/14)
        let b = [6.0f64, 3.0, 1.0].iter().map(|d| 0.5f64.powf(d / 14.0)).sum::<f64>();
        assert!((f[0].score - b).abs() < 1e-9);
        // Nothing in the window: none; a tie goes to the one visited last.
        let mut h = History::default();
        h.worlds.insert(world(1), WorldVisits { name: None, visits: vec![now - 3 * DAY] });
        h.worlds.insert(world(2), WorldVisits { name: None, visits: vec![now - 3 * DAY] });
        h.worlds.insert(world(3), WorldVisits { name: None, visits: vec![now - 61 * DAY] });
        let f = frequent(&h, now);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].world_id, world(1), "same score and time: by id");
        // The view: the last instance joined, with its world's name.
        h.record_instance(&world(2), "5~hidden(usr_x)~region(jp)", now - 100);
        h.instances.push(InstanceVisit { world_id: "wrld_bad".into(), instance: "1".into(), at: 0 });
        let v = HistoryView::of(&h, now);
        assert_eq!(v.last, None, "the last one is not VRChat's own: none");
        h.instances.pop();
        let v = HistoryView::of(&h, now);
        assert_eq!(v.last.as_ref().map(|l| l.instance.as_str()), Some("5~hidden(usr_x)~region(jp)"));
        assert_eq!(v.region(), "jp");
        assert_eq!(HistoryView::default().region(), "us");
    }

    #[test]
    fn the_last_instance_is_rejoined_only_while_open_and_joinable() {
        let public = view(Some((3, "9~region(eu)")), &[]);
        let card = |history: &HistoryView, open: Option<bool>, me: Option<&str>| suggestions(&inputs(&[], &[], Some(history), open, me))[1].clone();
        assert!(matches!(card(&public, Some(true), Some(ME)).go, Go::Instance(_)));
        assert_eq!(card(&public, Some(true), Some(ME)).reason, Reason::Recent { rejoin: true });
        assert_eq!(card(&public, Some(false), Some(ME)).go, Go::New, "closed: a new one");
        assert_eq!(card(&public, None, Some(ME)).go, Go::New, "not known: a new one");
        assert_eq!(card(&public, Some(true), None).go, Go::New, "signed out: a new one");
        let others = view(Some((3, "9~private(usr_someone)~region(eu)")), &[]);
        assert_eq!(card(&others, Some(true), Some(ME)).go, Go::New, "someone else's invite instance needs an invite");
        let mine = view(Some((3, &format!("9~private({ME})~canRequestInvite~region(eu)"))), &[]);
        assert!(matches!(card(&mine, Some(true), Some(ME)).go, Go::Instance(_)), "one's own");
        assert!(rejoinable("1~hidden(usr_x)~region(eu)", ME) && rejoinable("1~group(grp_x)~groupAccessType(public)", ME));
        assert!(!rejoinable("1~group(grp_x)~groupAccessType(members)", ME));
    }

    #[test]
    fn a_card_that_goes_gives_the_choice_back_to_home_and_says_so() {
        let invites = [invite("not_1", 1, NOW - 60)];
        let history = view(Some((3, "9~region(eu)")), &[4]);
        let cards = suggestions(&inputs(&invites, &[], Some(&history), Some(true), Some(ME)));
        let mut chosen = Some(Chosen { key: "invite:not_1".into(), name: "Chill House".into(), instance: true });
        assert_eq!(settle(&mut chosen, &cards), None, "still there");
        // The invite put away (or accepted elsewhere): back to Home, named.
        let cards = suggestions(&inputs(&[], &[], Some(&history), Some(true), Some(ME)));
        assert_eq!(settle(&mut chosen, &cards).as_deref(), Some("Chill House"));
        assert_eq!(chosen, None);
        // The last instance closed: its card stays (a new one of its world) but the choice was for that instance.
        let mut chosen = Some(Chosen { key: format!("recent:{}", world(3)), name: "World 3".into(), instance: true });
        let closed = suggestions(&inputs(&[], &[], Some(&history), Some(false), Some(ME)));
        assert_eq!(settle(&mut chosen, &closed).as_deref(), Some("World 3"));
        // A card for a new instance stays chosen whatever the old instance does.
        let mut chosen = Some(Chosen { key: format!("recent:{}", world(3)), name: "World 3".into(), instance: false });
        assert_eq!(settle(&mut chosen, &closed), None);
        let open = suggestions(&inputs(&[], &[], Some(&history), Some(true), Some(ME)));
        assert_eq!(settle(&mut chosen, &open), None);
        // An event over.
        let events = [EventSoon { id: "ev".into(), title: "Meetup".into(), starts_at: NOW - 3600, ends_at: Some(NOW + 60), ..Default::default() }];
        let mut chosen = Some(Chosen { key: "event:ev".into(), name: "Meetup".into(), instance: false });
        assert_eq!(settle(&mut chosen, &suggestions(&inputs(&[], &events, None, None, None))), None);
        let later = Inputs { now: NOW + 61, ..inputs(&[], &events, None, None, None) };
        assert_eq!(settle(&mut chosen, &suggestions(&later)).as_deref(), Some("Meetup"));
        // Nothing chosen: nothing to say.
        let mut none = None;
        assert_eq!(settle(&mut none, &[]), None);
    }

    #[test]
    fn each_kind_of_card_starts_vrchat_its_own_way() {
        let invites = [invite("not_1", 1, NOW - 60)];
        let events = [EventSoon { id: "ev1".into(), title: "Sample Meetup".into(), starts_at: NOW, location: Some(format!("{}:5~region(eu)", world(2))), ..Default::default() }];
        let history = view(Some((3, "9~region(use)")), &[4]);
        let cards = suggestions(&inputs(&invites, &events, Some(&history), Some(true), Some(ME)));
        let by = |key: &str| cards.iter().find(|c| c.key.starts_with(key)).unwrap();
        let mode = LaunchMode::Desktop;
        // Home: VRChat's own start, on the profile chosen.
        assert_eq!(action(&launch(by("home"), CreateAccess::FriendsPlus, true, 0), mode, Some(2)), Some(Action::PlayOn { mode, profile: 2 }));
        assert_eq!(action(&launch(by("home"), CreateAccess::FriendsPlus, true, 0), mode, None), Some(Action::Play { mode, destination: None }), "a Mac");
        // An invite: accepted, into its own instance.
        assert_eq!(action(&launch(by("invite"), CreateAccess::FriendsPlus, true, 0), mode, Some(1)), Some(Action::AcceptInviteOn { invite: "not_1".into(), mode, profile: 1 }));
        assert_eq!(action(&launch(by("invite"), CreateAccess::FriendsPlus, true, 0), mode, None), Some(Action::AcceptInvite { invite: "not_1".into(), mode }));
        // An event's live instance, the last instance still open: into them.
        let Some(Action::PlayAt { destination, profile: 0, .. }) = action(&launch(by("event"), CreateAccess::FriendsPlus, true, 0), mode, Some(0)) else { panic!() };
        assert_eq!((destination.world_id.as_str(), destination.instance_id.as_str()), (world(2).as_str(), "5~region(eu)"));
        let Some(Action::PlayAt { destination, .. }) = action(&launch(by("recent"), CreateAccess::FriendsPlus, true, 0), mode, Some(0)) else { panic!() };
        assert_eq!(destination.instance_id, "9~region(use)");
        // A place visited often, signed in: VRChat makes a new Friends+ instance first (in the region played last).
        let plan = launch(by("often"), CreateAccess::FriendsPlus, true, 0);
        assert_eq!(plan, Launch::Create { world: world(4), name: "World 4".into(), access: CreateAccess::FriendsPlus, region: InstanceRegion::Use });
        assert_eq!(action(&plan, mode, Some(0)), None, "made first, then started");
        assert!(matches!(launch(by("often"), CreateAccess::Invite, true, 0), Launch::Create { access: CreateAccess::Invite, .. }), "the kind chosen on the card");
        // Signed out: a new public instance named here, VRChat's own grammar, in that region.
        let Launch::At(place) = launch(by("often"), CreateAccess::FriendsPlus, false, 123_456_789) else { panic!() };
        assert_eq!(place.access, InstanceAccess::Public);
        assert!(crate::controller::vrchat_data::public_instance(&place.instance_id), "{}", place.instance_id);
        assert!(place.instance_id.ends_with("~region(use)"));
        assert!(orbiters_core::vrchat_launch::is_destination(&place.world_id, &place.instance_id));
        let Some(Action::PlayAt { destination, profile: 3, .. }) = action(&Launch::At(place.clone()), mode, Some(3)) else { panic!() };
        assert_eq!(destination, place);
        // What Steam is given for it: the profile, then the link last.
        let args = orbiters_core::vrchat_launch::arguments_on(mode, &orbiters_core::vrchat_launch::LaunchSettings { performance: false, ..Default::default() }, Vec::new(), 3, Some((&place.world_id, &place.instance_id)));
        assert_eq!(args, ["--no-vr".to_string(), "--profile=3".into(), format!("vrchat://launch?ref=orbiters.cc&id={}:{}", place.world_id, place.instance_id)]);
    }

    #[test]
    fn nothing_visited_leaves_home_alone() {
        let empty = HistoryView::default();
        let cards = suggestions(&inputs(&[], &[], Some(&empty), None, None));
        assert_eq!(keys(&cards), ["home"]);
        assert!(cards[0].local() && !cards[0].new_instance());
        assert_eq!(keys(&suggestions(&inputs(&[], &[], None, None, Some(ME)))), ["home"], "still reading: Home");
    }
}
