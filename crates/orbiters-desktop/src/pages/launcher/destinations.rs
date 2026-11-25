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

use super::selection::{Asker, Desk, Destination, Offer, Origin, Selection, Verified};
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
    /// Chosen among the previous instances (REQUEST21 7), joined last at `at` (Unix seconds).
    Previous { at: u64 },
    /// A friend's instance chosen with Select (REQUEST23): who is there.
    Friend { name: String },
}

/// Where Play goes with a card.
#[derive(Clone, Debug, PartialEq)]
pub enum Go {
    /// VRChat's own start, at its home world.
    Home,
    /// An instance that is there now (an event's, the last one, a chosen one VRChat said is open).
    Instance(Place),
    /// A previous instance or a friend's that VRChat has not said is open (being asked, closed, not known): nothing goes
    /// there from its card.
    Held(Place),
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
        matches!(self.reason, Reason::Home | Reason::Frequent { .. } | Reason::Recent { rejoin: false }) || matches!(self.reason, Reason::Previous { .. } | Reason::Friend { .. }) && self.go == Go::New
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
    // An instance of the history was joined: its friends-only owner was this account or a friend then.
    joinable_without_asking(instance, me, &|_| true)
}

/// Whether the account `me` can go into `instance` by its link alone, without asking anyone (REQUEST23: Select instead
/// of Request; Where's rejoin; a friend's profile's Join): public and a group's public ones; friends+ (anyone's friend
/// in it, so the friend there); friends-only when its owner is `me` or one of `friend_of`; an invite instance only when
/// `me` made it. A group's members-only or plus instance (membership is not known here), an unknown kind, never. Whether
/// it is full, age-gated or closed is not known from a location: VRChat is asked whether it is open before Play.
pub fn joinable_without_asking(instance: &str, me: &str, friend_of: &dyn Fn(&str) -> bool) -> bool {
    let (access, _) = orbiters_core::launcher_feed::access(instance);
    let tag = |name: &str| instance.split('~').find_map(|t| t.strip_prefix(name).and_then(|t| t.strip_prefix('(')).and_then(|t| t.strip_suffix(')')));
    match access {
        InstanceAccess::Public | InstanceAccess::GroupPublic | InstanceAccess::FriendsPlus => true,
        InstanceAccess::Friends => tag("friends").is_some_and(|owner| (!me.is_empty() && owner == me) || friend_of(owner)),
        InstanceAccess::Invite | InstanceAccess::InvitePlus => !me.is_empty() && tag("private") == Some(me),
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
    /// What is chosen, and the account and profile chosen: a previous instance (REQUEST21 7) or a friend's (REQUEST23)
    /// is right after Home.
    pub selection: Option<(&'a Selection, &'a Asker)>,
    /// The one chosen before, still offered there while another card is chosen.
    pub offer: Option<&'a Offer>,
    /// This computer uses several VRChat profiles: a private place goes to a VRChat for an account, or not at all.
    pub several: bool,
}

/// The cards, in the order shown: Home, then the invites, the events, the last instance and the places visited most,
/// each world once with its strongest reason.
pub fn suggestions(i: &Inputs) -> Vec<Suggestion> {
    let mut out = vec![Suggestion { key: "home".into(), world_id: String::new(), name: tr("Home world").into(), picture: None, reason: Reason::Home, go: Go::Home, region: String::new() }];
    let region = i.history.map_or("us", HistoryView::region).to_string();
    let taken = |out: &[Suggestion], world: &str| !world.is_empty() && out.iter().any(|s| s.world_id == world && matches!(s.go, Go::Instance(_) | Go::Held(_) | Go::Invite { .. } | Go::New));
    // The instance chosen among the previous ones, or a friend's: gone to only while VRChat says it is open for this
    // account; being asked, closed or not known, its card says where it is and Play holds.
    let chosen = i.selection.filter(|(s, _)| matches!(s.origin(), Origin::Previous | Origin::Friend { .. }));
    let shown = match chosen {
        Some((s, asker)) => s.destination().visit().map(|v| (v, s.origin(), s.instance().is_some().then(|| s.proven(asker)))),
        // Chosen before: chosen again, it is asked about again; signed out, a new one of its world.
        None => i.offer.map(|o| (&o.visit, &o.origin, i.me.is_some_and(|me| rejoinable(&o.visit.instance, me)).then_some(false))),
    };
    if let Some((v, origin, proven)) = shown {
        let place = orbiters_core::launcher_feed::place(&v.location(), &v.name);
        let own = v.region();
        let (go, region) = match (proven, place) {
            (Some(proven), Some(pl)) => {
                let r = pl.region.clone();
                (if proven { Go::Instance(pl) } else { Go::Held(pl) }, r)
            }
            _ => (Go::New, if own.is_empty() { region.clone() } else { own }),
        };
        let (reason, picture) = match origin {
            Origin::Friend { name, picture, .. } => (Reason::Friend { name: name.clone() }, picture.clone()),
            _ => (Reason::Previous { at: v.at }, None),
        };
        out.push(Suggestion { key: v.key(), world_id: v.world_id.clone(), name: world_name(&v.name), picture, reason, go, region });
    }
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
        // Its private instance, with nobody to go there as, where there are several profiles: VRChat starts at home, where the
        // event's invite arrives.
        let place = place.filter(|p| !i.several || i.me.is_some() || orbiters_core::vrchat_launch::is_public_instance(&p.instance_id));
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

/// The choice against the cards of now: kept while its card is there and still goes where it went; else the name of the
/// card that went (the choice goes back to Home, and the line says so). A card that went to a new instance and now finds
/// its old one open again keeps the choice (the person asked for that world). A previous instance or a friend's is not a
/// card's to take away: what VRChat says of it is said of the choice itself.
pub fn settle(selection: &Selection, cards: &[Suggestion]) -> Option<String> {
    let (key, name, instance) = match selection.destination() {
        Destination::Card { key, name, instance } => (key, name.clone(), *instance),
        Destination::Instance { key, visit } if *selection.origin() == Origin::Recent => (key, world_name(&visit.name), true),
        _ => return None,
    };
    let gone = match cards.iter().find(|s| s.key == *key) {
        None => true,
        Some(s) => instance && !matches!(s.go, Go::Instance(_) | Go::Invite { .. }),
    };
    gone.then_some(name)
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
    /// Nothing: the instance was not said open.
    Held,
    /// A new instance made by VRChat for the signed-in account first, then into it.
    Create { world: String, name: String, access: CreateAccess, region: InstanceRegion },
}

/// What Play does with `card`: `access` the kind of a new instance, `can_create` whether VRChat can make one for the
/// account signed in (else a new instance is a public one named here from `seed`, as the 3D worlds' visited list does).
pub fn launch(card: &Suggestion, access: CreateAccess, can_create: bool, seed: u64) -> Launch {
    match &card.go {
        Go::Home | Go::EventWaiting => Launch::Plain,
        Go::Instance(p) => Launch::At(p.clone()),
        Go::Held(_) => Launch::Held,
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

/// The action Play sends for `launch` in `mode` on `profile` (`None`: no profile choice, VRChat's default); none for a
/// new instance still to be made.
pub fn action(launch: &Launch, mode: LaunchMode, profile: Option<u32>) -> Option<Action> {
    Some(match (launch, profile) {
        (Launch::Plain, Some(profile)) => Action::PlayOn { mode, profile },
        (Launch::Plain, None) => Action::Play { mode, destination: None },
        (Launch::At(place), Some(profile)) => Action::PlayAt { mode, profile, destination: place.clone() },
        (Launch::At(place), None) => Action::Play { mode, destination: Some(place.clone()) },
        (Launch::Invite(id), Some(profile)) => Action::AcceptInviteOn { invite: id.clone(), mode, profile },
        (Launch::Invite(id), None) => Action::AcceptInvite { invite: id.clone(), mode },
        (Launch::Create { .. } | Launch::Held, _) => return None,
    })
}

/// The kinds a new instance can be of (the overlay's Go there's), each as VRChat names it, and who may join.
pub(super) fn kinds() -> [(CreateAccess, InstanceAccess, &'static str); 5] {
    [
        (CreateAccess::Public, InstanceAccess::Public, tr("anyone")),
        (CreateAccess::FriendsPlus, InstanceAccess::FriendsPlus, tr("your friends and theirs")),
        (CreateAccess::Friends, InstanceAccess::Friends, tr("your friends")),
        (CreateAccess::InvitePlus, InstanceAccess::InvitePlus, tr("those invited, who can ask")),
        (CreateAccess::Invite, InstanceAccess::Invite, tr("only those you invite")),
    ]
}

pub(super) fn kind_access(access: CreateAccess) -> InstanceAccess {
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
    // Reads only, into memory (a file of this computer, Orbiters' events, whether VRChat says an instance is open):
    // nothing of the window's work to finish before it may rest (window_rest's classified threads).
    {
        let mut s = slot.lock().unwrap_or_else(|e| e.into_inner());
        if s.busy || s.at.is_some_and(|at| at.elapsed() < every) {
            return;
        }
        s.busy = true;
    }
    let (shared, ctx) = (slot.clone(), ctx.clone());
    let started = std::thread::Builder::new().name(format!("where-{name}")).spawn(move || {
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

/// What was read for `member`: nothing that was for another (verify5-r21 m4), which is read again at once.
fn theirs<T: Clone>(slot: &Shared<(String, T)>, member: &str) -> Option<T> {
    let mut s = slot.lock().unwrap_or_else(|e| e.into_inner());
    if s.value.as_ref().is_some_and(|(whose, _)| whose != member) {
        s.value = None;
        s.at = None;
    }
    s.value.as_ref().map(|(_, value)| value.clone())
}

/// The member's events on now or soon, from Orbiters.
fn read_events(member: crate::member::Member, now: i64) -> Vec<EventSoon> {
    let (api, token) = member.api();
    let Ok(cards) = api.events(0) else { return Vec::new() };
    let mut out = Vec::new();
    // The few on now or soon: an answer each about the member's invites, one more for the live instance.
    for card in cards.items.iter().filter(|c| unix(c.starts_at.as_deref()).is_some_and(|s| EventSoon { starts_at: s, ends_at: unix(c.ends_at.as_deref()), ..Default::default() }.offered(now))).take(6) {
        let Ok(invites) = api.my_invites(&token, &card.id) else { continue };
        if !interested(&invites) {
            continue;
        }
        let location = api.event(Some(&token), &card.id).ok().and_then(|o| event_location(&o));
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
    /// The member's events on now or soon, and whose they are ([`crate::state::AccountStatus::member`]).
    events: Shared<(String, Vec<EventSoon>)>,
    /// Whether the last instance is open: its location, whom it was asked for, what VRChat said and when.
    open: Shared<(String, Asker, Option<bool>, Instant)>,
    /// Where's destination and everything known of it: one value, its one owner ([`super::selection`]).
    pub desk: Desk,
    /// The kind of a new instance chosen on a card, by world (Friends+ otherwise).
    pub(super) kinds: HashMap<String, CreateAccess>,
    /// The page of cards shown when they do not all fit.
    pub(super) page: usize,
    /// A quiet line and when it was said (egui's clock).
    pub(super) notice: Option<(String, f64)>,
    /// The cards drawn last (the Play press uses the very card shown).
    cards: Vec<Suggestion>,
    /// Whether the row showed in the last frame (the invite card under Play then stands aside).
    pub(super) showing: bool,
    /// A place was just chosen from outside the row (a friend's Select, REQUEST23): Home shows the row (its top).
    pub(super) reveal: bool,
    /// What the Map explorer says of the worlds (their pictures), once asked.
    infos: Option<crate::worlds::Infos>,
    /// Pictures of the page: what is handed in instead of being read.
    staged: Option<Staged>,
    /// Pictures of the page: the picture of the place chosen is still being read.
    stand_in_loading: bool,
    /// How each card's picture was last drawn (tests).
    #[cfg(test)]
    shown: HashMap<String, &'static str>,
    /// Every previous instance, in a sheet over Home (REQUEST21 7), and the one chosen there.
    pub previous: super::previous::Previous,
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
        *self.events.lock().unwrap() = Read { value: Some((String::new(), events)), at, busy: false };
        let location = self.history.lock().unwrap().value.as_ref().and_then(|h| h.last.as_ref()).map(|l| format!("{}:{}", l.world_id, l.instance)).unwrap_or_default();
        *self.open.lock().unwrap() = Read { value: Some((location, Asker::default(), last_open, Instant::now())), at, busy: false };
        self.staged = Some(Staged { signed_in, loading });
    }

    /// Pictures of the page: the picture of the place chosen is still being read.
    pub fn stand_in_picture_loading(&mut self) {
        self.stand_in_loading = true;
    }

    /// How the card `key`'s picture was last drawn: "ready", "loading" or "none" (tests).
    #[cfg(test)]
    pub fn picture_of(&self, key: &str) -> Option<&'static str> {
        self.shown.get(key).copied()
    }

    /// Pictures of the page: the card `key` chosen.
    pub fn stand_in_choose(&mut self, key: &str, name: &str, instance: bool) {
        self.desk.stand_in(Destination::Card { key: key.into(), name: name.into(), instance }, Origin::Card, Verified::Unverified);
    }

    /// Pictures of the page: `visit` chosen (or a new instance of its world), with what is known of it.
    pub fn stand_in_instance(&mut self, visit: super::previous::Visit, origin: Origin, verified: Verified, new: bool) {
        let key = visit.key();
        self.desk.stand_in(if new { Destination::NewOf { key, visit } } else { Destination::Instance { key, visit } }, origin, verified);
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
        self.desk.stand_in_making(mode);
    }

    /// Pictures of the page: the page of cards shown.
    pub fn stand_in_page(&mut self, page: usize) {
        self.page = page;
    }

    /// The chosen card's key, when Play goes there without asking (tests).
    #[cfg(test)]
    pub fn chosen_key(&self) -> Option<&str> {
        let s = self.desk.selection();
        s.destination().key().filter(|_| s.proven(self.desk.asker()))
    }

    /// What is chosen, whatever is known of it (tests).
    #[cfg(test)]
    pub fn selected_key(&self) -> Option<&str> {
        self.desk.selection().destination().key()
    }

    /// The line while a friend's chosen instance is being checked (tests).
    #[cfg(test)]
    pub fn checking_line(&self) -> Option<String> {
        super::previous::line(self).filter(|(_, tone)| *tone == Tone::Busy).map(|l| l.0)
    }

    /// What the line under Where warns of: a notice, or what is known of the choice (tests).
    #[cfg(test)]
    pub fn notice(&self) -> Option<String> {
        self.notice.as_ref().map(|n| n.0.clone()).or_else(|| super::previous::line(self).filter(|(_, tone)| *tone == Tone::Warn).map(|l| l.0))
    }

    /// Whether Home should bring Where into view (a place was just chosen from the friends list): once.
    pub fn take_reveal(&mut self) -> bool {
        std::mem::take(&mut self.reveal)
    }

    /// Play waits (a new instance being made, an instance being asked about): the Play button of that mode turns.
    pub fn making(&self) -> Option<LaunchMode> {
        self.desk.busy()
    }

    /// Why Play goes nowhere now: the instance chosen is closed, or nobody can be asked about it.
    pub fn held(&self) -> Option<&'static str> {
        let s = self.desk.selection();
        s.instance().filter(|_| self.showing)?;
        match s.verified() {
            Verified::Unverified | Verified::Unknown if self.previous.log.is_some() => Some(tr("VRChat's log cannot be read")),
            Verified::Closed => Some(tr("Closed now")),
            Verified::Unverified if self.previous.connecting => Some(self.previous.connecting_text()),
            Verified::Unverified | Verified::Unknown if self.desk.asker().account.is_none() => Some(tr("Not signed in")),
            _ => None,
        }
    }

    fn previous_staged(&self) -> bool {
        self.previous.is_staged()
    }
}

/// Why private places are unavailable while VRChat's log cannot be read, and what to do.
pub(crate) fn log_words(trouble: &orbiters_core::launcher::LogTrouble) -> String {
    use orbiters_core::launcher::LogTrouble;
    match trouble {
        LogTrouble::NoFolder => tr("Private places need VRChat's log, not found: turn on Logging in VRChat's settings.").into(),
        LogTrouble::Denied(path) => orbiters_i18n::tr!("Private places need VRChat's log: allow Orbiters - N4N0 to read {path}", path = path),
        LogTrouble::NotWritten => tr("Private places need VRChat's log, which is off: turn on Logging in VRChat's settings.").into(),
    }
}

/// The account whose places are offered: the one signed in and on show, when the profile chosen is its own (or says
/// no account); and whether VRChat can make instances for it.
fn account(state: &State, cx: &Cx, profile_account: Option<&str>) -> (Option<String>, bool) {
    let who = who(state, cx, profile_account);
    (who.me, who.can_create)
}

/// Who Where's places are for (verify-r21 Major 1).
pub(super) struct Who {
    /// The VRChat account VRChat verified as signed in here (the sign-in's own `user`, not the friends list, which comes
    /// seconds later), when the profile chosen under Play is its own (or says no account).
    pub me: Option<String>,
    /// Whether VRChat makes instances for it.
    pub can_create: bool,
    /// A saved session is being restored: who is signed in is not known yet (no sign-in offered, nothing decided).
    pub connecting: bool,
    /// Signed in, but the profile chosen is another account's.
    pub elsewhere: bool,
    /// Signed in, on a computer with several VRChat profiles, and VRChat's log cannot be read here: nothing private goes to a
    /// VRChat.
    pub log: Option<orbiters_core::launcher::LogTrouble>,
    /// The friends list is this account's own, read with its session (its invites can be offered).
    pub feed: bool,
}

pub(super) fn who(state: &State, cx: &Cx, profile_account: Option<&str>) -> Who {
    use crate::controller::vrchat_login::Presence;
    let login = &cx.snap.vrchat_login;
    let own = |name: &str| profile_account.is_none_or(|a| a.is_empty() || name.is_empty() || a == name);
    let presence = login.presence();
    let verified = login.user.as_ref().filter(|_| login.allowed);
    let feed_of = |id: &str| matches!(&state.data.source, Source::SignedIn { user_id, .. } if user_id == id) && !state.stale();
    let staged = state.places.staged.as_ref();
    let (me, feed) = match (verified, presence, staged) {
        (Some(u), _, _) if own(&u.display_name) => (Some(u.id.clone()), feed_of(&u.id)),
        (Some(_), _, _) => (None, false),
        // Pictures of the page and tests that hand the friends list in alone (no sign-in in their snapshot): its account.
        (None, Presence::SignedOut, Some(_)) => match (&state.data.source, state.stale()) {
            (Source::SignedIn { user_id, name }, false) if !user_id.is_empty() && own(name) => (Some(user_id.clone()), true),
            _ => (None, false),
        },
        _ => (None, false),
    };
    let elsewhere = me.is_none() && (verified.is_some() || matches!(state.data.source, Source::SignedIn { .. }) && staged.is_some()) && profile_account.is_some();
    // (With the default profile alone to choose, whose its VRChat is may still be told by the account's own invite.)
    let log = state.data.log_trouble.clone().filter(|_| state.data.several_profiles && state.data.profiles.len() > 1 && me.is_some());
    let (me, feed) = if log.is_some() { (None, false) } else { (me, feed) };
    let can_create = match staged {
        Some(s) => s.signed_in && me.is_some(),
        None => me.is_some() && login.allowed,
    };
    Who { me, can_create, connecting: presence == Presence::Connecting && verified.is_none(), elsewhere, log, feed }
}

/// The cards now, after the reads have been started where due.
fn cards(ui: &Ui, cx: &Cx, state: &mut State, me: Option<&str>, feed: bool) -> (Vec<Suggestion>, bool) {
    let ctx = ui.ctx().clone();
    let now = state.now();
    // Tests read nothing of this computer's or the network's (pictures of the page hand everything in).
    let reads = state.places.staged.is_none() && !cfg!(test);
    if reads {
        refresh(&state.places.history, HISTORY_EVERY, &ctx, "history", || {
            let history = orbiters_core::visited::History::load(&orbiters_core::visited::history_file());
            Some(HistoryView::of(&history, orbiters_core::visited::now()))
        });
        // Only once who it is is known: an empty list read before would stand for theirs.
        let server = orbiters_core::account::Server::current();
        if let Some(member) = crate::member::reading(server).filter(|m| cx.snap.account.member.as_deref() == Some(m.key().as_str())) {
            let _ = theirs(&state.places.events, &member.key());
            refresh(&state.places.events, EVENTS_EVERY, &ctx, "events", move || Some((member.key(), read_events(member, events_now()))));
        }
    }
    let history = state.places.history.lock().unwrap_or_else(|e| e.into_inner()).value.clone();
    let loading = match &state.places.staged {
        Some(s) => s.loading,
        None => history.is_none(),
    };
    // (Pictures of the page: nobody's in particular.)
    let whose = if state.places.staged.is_some() { "" } else { cx.snap.account.member.as_deref().unwrap_or_default() };
    let events = theirs(&state.places.events, whose).unwrap_or_default();
    // Whether the last instance is open: asked of VRChat (the overlay's own read) while a rejoin could be offered.
    let last = history.as_ref().and_then(|h| h.last.clone());
    let location = last.as_ref().map(|l| format!("{}:{}", l.world_id, l.instance)).unwrap_or_default();
    let mut last_open = None;
    if let (Some(last), Some(me)) = (&last, me) {
        if rejoinable(&last.instance, me) {
            let known = state.places.open.lock().unwrap_or_else(|e| e.into_inner()).value.clone();
            if reads {
                if known.as_ref().is_some_and(|k| k.0 != location || k.1 != *state.places.desk.asker()) {
                    // Another instance, account or profile since: asked about now.
                    let mut s = state.places.open.lock().unwrap_or_else(|e| e.into_inner());
                    s.at = None;
                    s.value = None;
                }
                if let Some(scope) = state.scope.clone() {
                    let (ctl, world, instance, key, by) = (cx.ctl.clone(), last.world_id.clone(), last.instance.clone(), location.clone(), state.places.desk.asker().clone());
                    refresh(&state.places.open, OPEN_EVERY, &ctx, "open", move || {
                        let asked = Instant::now();
                        let (open, at) = super::previous::said(asked, ctl.vrchat_data(scope, Request::InstanceSeen { world, instance, max_age_ms: 30_000 }));
                        Some((key, by, open, at))
                    });
                }
            }
            // What was said to this account (handed in, for pictures of the page: to nobody in particular). On which profile,
            // and how long ago, counts when it is chosen.
            last_open = state.places.open.lock().unwrap_or_else(|e| e.into_inner()).value.clone().filter(|k| k.0 == location && (k.1.account.as_deref() == Some(me) || !reads)).and_then(|k| k.2);
        }
    }
    let dismissed = |id: &str| state.dismissed.contains(id);
    // Invites are the friends list's: offered only when it is this account's own, read with its session.
    let invites: &[Invite] = if feed { &state.data.invites } else { &[] };
    let desk = &state.places.desk;
    let inputs = Inputs { now, invites, dismissed: &dismissed, events: &events, history: history.as_ref(), last_open, me, selection: Some((desk.selection(), desk.asker())), offer: desk.offer(), several: state.data.several_profiles };
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
        state.places.desk.hidden();
        state.places.previous.stand_in_close();
        return;
    }
    let offered = super::states::profiles(&state.data, state.added_profile);
    let next = super::states::next_here(&offered, state.chosen_profile);
    let profile_account = offered.iter().find(|p| p.profile == next).and_then(|p| p.account.clone());
    let (play, profile) = (super::states::play_here(&state.data, &offered, next), Some(next));
    if !showing(play) {
        // VRChat runs on the profile chosen: the previous instances' sheet goes with Where, and so does a press of Play
        // that waits for an answer.
        state.places.desk.hidden();
        state.places.previous.stand_in_close();
        return;
    }
    state.places.showing = true;
    let ctx = ui.ctx().clone();
    let clock = anim::time(&ctx);
    let who = who(state, cx, profile_account.as_deref());
    let (me, can_create) = (who.me.clone(), who.can_create);
    // What the previous instances' sheet allows (it is drawn over the page, after this): the account VRChat verified,
    // or that it is still connecting (verify-r21 Major 1).
    state.places.previous.can_create = can_create;
    state.places.previous.signed_in_elsewhere = who.elsewhere;
    state.places.previous.log = who.log.clone();
    state.places.previous.connecting = who.connecting;
    state.places.previous.unreadable = cx.snap.account.unreadable;
    // The account and the profile chosen now first: an answer for another counts for nothing.
    state.places.desk.set_asker(Asker { account: me.clone(), profile });
    state.places.desk.drawn(ctx.cumulative_pass_nr());
    super::previous::follow(state, cx, clock);
    let (cards, loading) = cards(ui, cx, state, me.as_deref(), who.feed);
    if let Some(name) = settle(state.places.desk.selection(), &cards) {
        state.places.desk.go_home();
        state.places.notice = Some((orbiters_i18n::tr!("{name} is no longer available: Play starts VRChat at home.", name = name), clock));
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
    // r21: a saved session being restored says so; only a certain "signed out" offers the sign-in.
    let signed_out = cx.snap.vrchat_login.presence() == crate::controller::vrchat_login::Presence::SignedOut;
    let hint = hint(state, &cards, me.is_some(), can_create, clock, profile_account.as_deref(), !signed_out, who.log.as_ref());
    // Signed out of VRChat: the way to sign in, at the line's end.
    let offer_sign_in = signed_out && !matches!(state.data.source, Source::SignedIn { .. }) && state.places.notice.is_none() && state.places.making().is_none() && state.places.held().is_none();
    let mut sign_in = false;
    // The instance chosen is closed, not known or cannot be asked about: what can be had instead, at the line's end.
    let offer_new = state.places.notice.is_none() && super::previous::alternatives(&state.places);
    let mut take_new = false;
    let height = HEAD_H + HEAD_GAP + card_h + if hint.is_some() { HINT_H + 4.0 } else { 0.0 } + PLAY_HEAD_GAP + PLAY_HEAD_H;
    super::super::block(ui, cx, index, height, |ui, rect| {
        // The heading: Where, and what is chosen.
        let p = ui.painter().clone();
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), HEAD_H));
        let title = widgets::text(&p, pos2(head.left(), head.center().y), Align2::LEFT_CENTER, &Txt::new(tr_ctx("launch", "Where"), 19.0).weight(weight::SEMIBOLD).tracking(-0.01));
        // A new instance says its kind with the name (the card's line no longer has room for it).
        let chosen_card = state.places.desk.selection().destination().key().and_then(|k| shown_cards.iter().find(|s| s.key == k));
        let chosen_name = match chosen_card {
            Some(card) if card.new_instance() => orbiters_i18n::tr!("{name} · new {kind} instance", name = card.name, kind = access_name(kind_access(kind_of(state, card, can_create)))),
            Some(card) => card.name.clone(),
            None => tr("Home world").to_string(),
        };
        let arrows_w = if pages > 1 { 2.0 * 30.0 + 52.0 } else { 0.0 };
        // Every instance he was in, to start VRChat in one of them (REQUEST21 7).
        let previous_w = super::previous::button(ui, state, pos2(head.right() - arrows_w - if pages > 1 { 10.0 } else { 0.0 }, head.center().y));
        let selected = orbiters_i18n::tr!("Selected: {name}", name = chosen_name);
        widgets::text_truncated(&p, pos2(title.right() + 14.0, head.center().y + 1.0), Align2::LEFT_CENTER, &Txt::new(selected, 13.5).weight(weight::MEDIUM).color(theme::accent()), head.right() - title.right() - 38.0 - arrows_w - previous_w);
        if pages > 1 {
            let next = Rect::from_center_size(pos2(head.right() - 15.0, head.center().y), vec2(30.0, 30.0));
            let prev = next.translate(vec2(-34.0, 0.0));
            let at = state.places.page;
            widgets::text(&p, pos2(prev.left() - 8.0, head.center().y), Align2::RIGHT_CENTER, &Txt::new(format!("{} / {pages}", at + 1), 12.5).color(theme::fg_3()));
            // Both arrows always, the one that leads nowhere faded and inert (the row does not shift between pages).
            for (r, glyph, label, back) in [(prev, icons::CARET_LEFT, orbiters_i18n::mark("Previous places"), true), (next, icons::CARET_RIGHT, orbiters_i18n::mark("More places"), false)] {
                let can = if back { at > 0 } else { at + 1 < pages };
                if can {
                    if widgets::activated(&widgets::IconButton::new(glyph, label).size(30.0, 14.0).id_salt(("where-page", back)).show_at(ui, r)) {
                        state.places.page = if back { at - 1 } else { at + 1 };
                    }
                } else {
                    widgets::icon(&p, r.center(), glyph, 14.0, theme::fg_4());
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
                _ => theme::fg_3(),
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
            if offer_new {
                let b = widgets::Button::new(tr("New instance of this world")).icon(icons::PLUS).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt("where-new-instead");
                let bw = b.desired_width(ui);
                if widgets::pressed_fresh(&b.show_at(ui, Rect::from_min_size(pos2(rect.right() - bw, y - 14.0), vec2(bw, 28.0)))) {
                    take_new = true;
                }
                room -= bw + 8.0;
            }
            widgets::text_truncated(&p, pos2(rect.left() + 22.0, y), Align2::LEFT_CENTER, &Txt::new(text.as_str(), 12.5).color(theme::fg_2()), room);
            if *tone == Tone::Warn {
                // The whole of it on hover (a path may be long).
                ui.interact(Rect::from_min_size(pos2(rect.left() + 22.0, y - HINT_H * 0.5), vec2(room, HINT_H)), Id::unique("where-hint"), Sense::hover()).on_hover_text(text.as_str());
            }
        }
        // The Play heading, under the row, over the buttons.
        let py = rect.bottom() - PLAY_HEAD_H * 0.5 - 2.0;
        widgets::text(&p, pos2(rect.left(), py), Align2::LEFT_CENTER, &Txt::new(tr_ctx("launch", "Play"), 19.0).weight(weight::SEMIBOLD).tracking(-0.01));
    });
    if sign_in {
        cx.go = Some(super::super::Go::VrchatAccount);
    }
    if take_new {
        super::previous::take_new_of_chosen(state);
    }
    if state.places.notice.as_ref().is_some_and(|(_, at)| clock - at < NOTICE_FOR) {
        super::super::frame_in(&ctx, NOTICE_FOR - (clock - state.places.notice.as_ref().map_or(clock, |n| n.1)));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Quiet,
    Busy,
    Warn,
}

/// The line under the cards, when there is something to say: an instance being made, a card that went or an instance
/// that could not be made (for a while), an event that waits for its invite, why invites and Friends+ are missing.
fn hint(state: &mut State, cards: &[Suggestion], signed_in: bool, can_create: bool, clock: f64, profile_account: Option<&str>, connecting: bool, log: Option<&orbiters_core::launcher::LogTrouble>) -> Option<(String, Tone)> {
    // First of all: why private places are unavailable.
    if let Some(trouble) = log.filter(|_| !state.stale()) {
        return Some((log_words(trouble), Tone::Warn));
    }
    if let Some((text, at)) = &state.places.notice {
        if clock - at < NOTICE_FOR {
            return Some((text.clone(), Tone::Warn));
        }
        state.places.notice = None;
    }
    // A previous instance or a friend's: being asked about (Play pressed on an older "open" too), closed, not known.
    if let Some(line) = super::previous::line(&state.places) {
        return Some(line);
    }
    if state.places.making().is_some() {
        let chosen = state.places.desk.selection().destination().key().and_then(|k| cards.iter().find(|s| s.key == k));
        let kind = chosen.map(|s| kind_of(state, s, can_create)).unwrap_or(CreateAccess::FriendsPlus);
        return Some((orbiters_i18n::tr!("Opening a new {kind} instance…", kind = access_name(kind_access(kind))), Tone::Busy));
    }
    let chosen = state.places.desk.selection().destination().key().and_then(|k| cards.iter().find(|s| s.key == k));
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
        // r21: a saved session being restored (or its list on the way) is not "signed out".
        (_, false) if connecting => Some((state.places.previous.connecting_text().into(), Tone::Busy)),
        (_, false) => Some((tr("Sign in to VRChat to see invites here and open Friends+ instances.").into(), Tone::Quiet)),
        _ => None,
    }
}

/// The kind of a new instance for `card`: the one chosen on it, else Friends+; public only where VRChat cannot make one.
fn kind_of(state: &State, card: &Suggestion, can_create: bool) -> CreateAccess {
    kind_for(state, &card.world_id, can_create)
}

/// [`kind_of`] for a world (the previous instances' rows choose it too, for the same card).
pub(super) fn kind_for(state: &State, world: &str, can_create: bool) -> CreateAccess {
    if !can_create {
        return CreateAccess::Public;
    }
    state.places.kinds.get(world).copied().unwrap_or(CreateAccess::FriendsPlus)
}

/// A card's line under its name (why it is offered), and the colour of its mark. What it goes to is on its picture: a
/// new instance's kind ("New · Friends+", to change), or the kind and region of the one that is there.
fn reason_line(card: &Suggestion, now: i64) -> (String, &'static str, Color32) {
    match &card.reason {
        Reason::Home => (tr("VRChat's usual start").into(), icons::HOUSE, theme::fg_3()),
        Reason::Invite { from, .. } => (orbiters_i18n::tr!("Invite from {name}", name = from), icons::ENVELOPE_SIMPLE, theme::accent()),
        Reason::Event { starts_at, .. } if *starts_at <= now => (tr("Event · on now").into(), icons::CALENDAR_DOTS, theme::WARN),
        Reason::Event { starts_at, .. } => {
            let minutes = ((starts_at - now + 59) / 60).max(1);
            (orbiters_i18n::tr!("Event · starts in {minutes} min", minutes = minutes), icons::CALENDAR_DOTS, theme::WARN)
        }
        Reason::Recent { .. } => (tr("Last visited").into(), icons::CLOCK_COUNTER_CLOCKWISE, theme::fg_3()),
        Reason::Frequent { .. } => (tr("Often visited").into(), icons::STAR, theme::fg_3()),
        Reason::Friend { name } => (orbiters_i18n::tr!("With {name}", name = name), icons::USERS, theme::accent()),
        Reason::Previous { at } => (
            orbiters_i18n::tr!("Previous instance · {when}", when = orbiters_ui::format::relative_time(*at, now.max(0) as u64)),
            icons::CLOCK_COUNTER_CLOCKWISE,
            theme::fg_3(),
        ),
    }
}

/// A card: its picture (the kind of instance on it), its name and its reason; ringed when chosen. A press chooses it.
#[allow(clippy::too_many_arguments)]
fn card_ui(ui: &mut Ui, cx: &Cx, state: &mut State, rect: Rect, pic_h: f32, card: &Suggestion, can_create: bool, clock: f64) {
    let ctx = ui.ctx().clone();
    let chosen = state.places.desk.selection().destination().key().map_or(card.go == Go::Home, |k| k == card.key);
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
    let shown = picture(state, card);
    #[cfg(test)]
    state.places.shown.insert(card.key.clone(), match &shown {
        Shown::Ready(_) => "ready",
        Shown::Loading => "loading",
        Shown::None => "none",
    });
    match shown {
        Shown::Ready(t) => paint::squircle_texture(&p, pic, radii, t.id(), cover_uv(t.size_vec2(), pic.size()), Color32::WHITE),
        Shown::Loading => {
            // Square below, as a picture is: the placeholder's lower corners are past the edge.
            let clip = p.clip_rect().intersect(pic);
            orbiters_ui::feedback::shimmer(&p.with_clip_rect(clip), Rect::from_min_max(pic.min, pic.max + vec2(0.0, RADIUS * 2.0)), RADIUS);
            // The sweep is faint on a card: the spinner says it is on its way.
            orbiters_ui::feedback::spinner(&p, pic.center(), 11.0, 2.2, theme::fg_3());
        }
        Shown::None => {
            // No picture: the name's colour as a soft wash, and what the card is.
            let c = if card.go == Go::Home { theme::accent() } else { widgets::name_color(&card.name) };
            let points = paint::squircle_points(pic, theme::squircle(RADIUS));
            paint::fill_convex(&p, &points, |at| {
                let t = ((at.x - pic.left()) / pic.width() + (at.y - pic.top()) / pic.height()) * 0.5;
                theme::mix(theme::alpha(c, 0.32), theme::ink_800(), t)
            });
            p.rect_filled(Rect::from_min_max(pos2(pic.left(), pic.bottom() - RADIUS), pic.max), 0.0, theme::mix(theme::alpha(c, 0.32), theme::ink_800(), 0.9));
            let glyph = match card.go {
                Go::Home => icons::HOUSE,
                _ => icons::GLOBE_HEMISPHERE_WEST,
            };
            widgets::icon(&p, pic.center(), glyph, 30.0, theme::alpha(theme::fg(), 0.55));
        }
    }
    // The instance on the picture: the kind of a new one, to change; the kind and region of one that is there.
    let now = state.now();
    let kind = kind_of(state, card, can_create);
    match &card.go {
        Go::New => kind_pill(ui, state, pic, card, kind, can_create),
        Go::Instance(place) | Go::Held(place) | Go::Invite { place, .. } => {
            let mut chips = access_name(place.access).to_string();
            if !place.region.is_empty() {
                chips = format!("{chips} · {}", region_tag(&place.region));
            }
            let txt = Txt::new(chips, 11.5).weight(weight::SEMIBOLD).color(theme::fg());
            let w = widgets::text_size(ui, &txt).x + 30.0;
            let chip = Rect::from_min_size(pic.left_bottom() + vec2(8.0, -30.0), vec2(w.min(pic.width() - 16.0), 22.0));
            p.rect_filled(chip, theme::radius(11.0), theme::black(0.6));
            widgets::icon(&p, pos2(chip.left() + 12.0, chip.center().y), access_icon(place.access), 12.0, theme::fg());
            widgets::text_truncated(&p, pos2(chip.left() + 22.0, chip.center().y), Align2::LEFT_CENTER, &txt, chip.width() - 28.0);
        }
        Go::Home | Go::EventWaiting => {}
    }
    // The chosen card's tick, top right.
    if ring > 0.01 {
        let c = pos2(pic.right() - 18.0, pic.top() + 18.0);
        p.circle_filled(c, 11.0 * ring, theme::accent());
        widgets::icon(&p, c, icons::CHECK, 13.0 * ring, theme::on_accent());
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
        p.circle_filled(pos2(x + 4.0, y), 4.0, theme::accent());
    } else {
        widgets::icon(&p, pos2(x + 5.0, y), glyph, 12.5, tone);
    }
    let line_color = if matches!(card.reason, Reason::Invite { .. }) { theme::accent() } else { theme::fg_3() };
    widgets::text_truncated(&p, pos2(x + 16.0, y), Align2::LEFT_CENTER, &Txt::new(line, 12.5).weight(weight::MEDIUM).color(line_color), room - 16.0);
    if ring > 0.01 {
        paint::stroke_squircle(&p, rect.expand(3.0), theme::squircle(RADIUS + 3.0), egui::Stroke::new(2.0, theme::alpha(theme::accent(), ring)));
    } else if hover > 0.01 {
        paint::stroke_squircle(&p, rect, theme::squircle(RADIUS), egui::Stroke::new(1.0, theme::white(0.08 * hover)));
    }
    if widgets::pressed_fresh(&resp) {
        choose(state, cx, card);
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
    let txt = Txt::new(orbiters_i18n::tr!("New · {kind}", kind = label), 11.5).weight(weight::SEMIBOLD).color(theme::fg());
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
    widgets::icon(p, pos2(r.left() + 12.0, r.center().y), access_icon(access), 12.0, theme::fg());
    widgets::text_truncated(p, pos2(r.left() + 22.0, r.center().y), Align2::LEFT_CENTER, &txt, r.width() - if can_create { 40.0 } else { 28.0 });
    if !can_create {
        resp.on_hover_text(tr("Sign in to VRChat in Settings to open Friends+, Friends or Invite instances: without it a new instance is public."));
        return;
    }
    widgets::icon(p, pos2(r.right() - 11.0, r.center().y), icons::CARET_DOWN, 10.0, theme::fg_2());
    widgets::note_control(&ctx, label, r, true);
    let items: Vec<popup::Item> = kinds().iter().map(|(_, a, says)| popup::Item::new(access_name(*a)).detail(says).icon(access_icon(*a))).collect();
    let selected = kinds().iter().position(|k| k.0 == kind);
    let pressed = widgets::activated(&resp);
    if let Some(i) = popup::list_from(&ctx, list, r, pressed, &items, selected, popup::list_width(ui, &items).max(220.0)) {
        state.places.kinds.insert(card.world_id.clone(), kinds()[i].0);
    }
    resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tr("Who can join the new instance"));
}

/// A card's picture.
enum Shown {
    Ready(egui::TextureHandle),
    /// Being read: the app's loading placeholder.
    Loading,
    /// None (it could not be had): the name's colour and what the card is.
    None,
}

/// A card's picture. The place chosen has its world's, whatever card it is of: what is on this computer, else asked for
/// once ([`chosen_picture`]). A card that is not chosen shows what its source always did: an invite's (the launcher's own
/// pictures), an event's cover, its world's on the Map explorer (Orbiters' public pages, kept on this computer); a
/// previous instance's or a friend's, chosen before, only what is here already.
fn picture(state: &mut State, card: &Suggestion) -> Shown {
    if let Some(t) = card.picture.as_ref().and_then(|url| state.pictures.get(url)) {
        return Shown::Ready(t.clone());
    }
    if card.go != Go::Home && state.places.desk.is_chosen(&card.key) {
        return chosen_picture(state, card);
    }
    here(state, card).or_else(|| sourced(state, card)).map_or(Shown::None, Shown::Ready)
}

/// What a card's own source gives, asked as it always was; nothing for a previous instance or a friend's.
fn sourced(state: &mut State, card: &Suggestion) -> Option<egui::TextureHandle> {
    match (&card.reason, &card.picture) {
        (Reason::Home | Reason::Previous { .. } | Reason::Friend { .. }, _) => None,
        (Reason::Invite { .. }, Some(url)) if url.starts_with("https://") => {
            let url = url.clone();
            state.picture(&url)
        }
        (_, Some(url)) if url.starts_with("https://") => remote_picture(state, &card.world_id, url.clone()),
        _ => world_picture(state, &card.world_id),
    }
}

/// The address of the world's picture the app holds already: the card's own (a friend's place, an invite's), else one a
/// friend or an invite in that world brought.
fn held_url(state: &State, card: &Suggestion) -> Option<String> {
    use orbiters_core::launcher::Whereabouts;
    if card.world_id.is_empty() || matches!(card.reason, Reason::Event { .. }) {
        return None;
    }
    let places = state
        .data
        .friends
        .iter()
        .filter_map(|f| match &f.whereabouts {
            Whereabouts::World(p) => Some(p),
            _ => None,
        })
        .chain(state.data.invites.iter().filter_map(|i| i.place.as_ref()));
    let given = card.picture.clone().filter(|u| u.starts_with("https://"));
    // A list that is no longer the account's says nothing.
    given.or_else(|| (!state.stale()).then(|| places.filter(|p| p.world_id == card.world_id).map(|p| p.world_picture.clone()).find(|u| u.starts_with("https://"))).flatten())
}

/// The card's picture when it is here already: read before, or kept on this computer. Nothing is asked.
fn here(state: &mut State, card: &Suggestion) -> Option<egui::TextureHandle> {
    use crate::worlds::{Look, Picture};
    if !matches!(card.reason, Reason::Previous { .. } | Reason::Friend { .. }) {
        return None;
    }
    if let (Some(url), Some(scope), false) = (held_url(state, card), state.scope.clone(), state.places.desk.is_quiet()) {
        let ctx = state.ctx.clone()?;
        let loader = state.loader.as_mut()?;
        if let Look::Ready(t) = loader.look(&Picture::World { scope, world_id: card.world_id.clone(), url: url.clone() }) {
            return Some(t);
        }
        if let Some(t) = loader.get(&ctx, &Picture::RemoteKept { world_id: card.world_id.clone(), url }) {
            return Some(t);
        }
    }
    // By its page on the Map explorer: read in this run, or kept on this computer.
    let page = state.places.infos.as_ref().and_then(|i| i.known(&card.world_id)).and_then(|info| page_url(&info));
    if let (Some(url), Some(loader)) = (page, state.loader.as_ref()) {
        if let Look::Ready(t) = loader.look(&Picture::Remote { world_id: card.world_id.clone(), url, large: false }) {
            return Some(t);
        }
    }
    super::previous::kept_world_picture(state, &card.world_id)
}

/// The picture a world's page gives.
fn page_url(info: &crate::worlds::Info) -> Option<String> {
    match info {
        crate::worlds::Info::Listed(page) => page.world.thumbnail_url.clone().or(page.world.image_url.clone()).filter(|u| u.starts_with("https://")),
        _ => None,
    }
}

/// The picture of the place chosen: the one read already, else asked for once under this choice: at the address the app
/// holds ([`held_url`]), and only without one through the Map explorer's page of that world. What is here meanwhile shows
/// (a friend's profile's small one), else the loading placeholder; what could not be had is the usual card.
fn chosen_picture(state: &mut State, card: &Suggestion) -> Shown {
    use crate::worlds::{Look, Picture};
    if state.places.stand_in_loading {
        return Shown::Loading;
    }
    if state.places.desk.is_quiet() {
        return here(state, card).or_else(|| sourced(state, card)).map_or(Shown::None, Shown::Ready);
    }
    // What a card that asks by itself has got already stays as it is.
    let had = if matches!(card.reason, Reason::Invite { .. }) { None } else { sourced(state, card) };
    if let Some(t) = had {
        return Shown::Ready(t);
    }
    let (Some(ctx), Some(scope)) = (state.ctx.clone(), state.scope.clone()) else { return Shown::None };
    // A place of this computer's history, or an event, has its own on its way: not asked for twice.
    if !matches!(card.reason, Reason::Previous { .. } | Reason::Friend { .. } | Reason::Invite { .. }) && page_loading(state, card) {
        return Shown::Loading;
    }
    let Some(url) = held_url(state, card) else {
        // No address: the world's page on the Map explorer, then its picture (a previous instance's; the others asked).
        if matches!(card.reason, Reason::Previous { .. } | Reason::Friend { .. }) {
            if let Some(t) = here(state, card).or_else(|| world_picture(state, &card.world_id)) {
                return Shown::Ready(t);
            }
        }
        return if page_loading(state, card) { Shown::Loading } else { Shown::None };
    };
    // The same picture at the small size the launcher keeps, when its profile or its invite showed it.
    let small = {
        let user = state.owner_of(&url);
        state.loader.as_ref().and_then(|l| match l.look(&Picture::Profile { scope: scope.clone(), url: url.clone(), user }) {
            Look::Ready(t) => Some(t),
            _ => None,
        })
    };
    let small = small.or_else(|| here(state, card));
    let wanted = Picture::World { scope, world_id: card.world_id.clone(), url: url.clone() };
    let first = state.places.desk.may_ask_picture(&card.key, &url);
    let Some(loader) = state.loader.as_mut() else { return Shown::None };
    let look = loader.look(&wanted);
    let instead = |state| match small {
        Some(t) => Shown::Ready(t),
        None => state,
    };
    if matches!(look, Look::Missing) {
        if !first {
            return instead(Shown::None);
        }
        loader.again(&wanted);
    }
    match loader.get(&ctx, &wanted) {
        Some(t) => Shown::Ready(t),
        None => instead(Shown::Loading),
    }
}

/// Whether the Map explorer's page of the world, or the picture it gives, is being read.
fn page_loading(state: &mut State, card: &Suggestion) -> bool {
    use crate::worlds::{Look, Picture};
    if state.places.staged.is_some() || cfg!(test) || card.world_id.is_empty() {
        return false;
    }
    let own = card.picture.clone().filter(|u| u.starts_with("https://") && matches!(card.reason, Reason::Event { .. }));
    let url = match own {
        Some(url) => Some(url),
        None => match state.places.infos.as_mut().map(|i| i.get(&card.world_id)) {
            None => return false,
            Some(None) => return true,
            Some(Some(info)) => page_url(&info),
        },
    };
    let picture = url.map(|url| Picture::Remote { world_id: card.world_id.clone(), url, large: false });
    picture.is_some_and(|p| state.loader.as_ref().is_some_and(|l| matches!(l.look(&p), Look::Loading)))
}

/// A world's own picture on the Map explorer (Orbiters' public pages, kept on this computer): Where's cards' and the
/// previous instances' rows'. None in pictures of the page and tests (nothing read).
pub(super) fn world_picture(state: &mut State, world_id: &str) -> Option<egui::TextureHandle> {
    if state.places.staged.is_some() || state.places.previous_staged() || world_id.is_empty() || cfg!(test) {
        return None;
    }
    let ctx = state.ctx.clone()?;
    let infos = state.places.infos.get_or_insert_with(|| crate::worlds::Infos::new(&ctx));
    let url = page_url(&infos.get(world_id)?)?;
    remote_picture(state, world_id, url)
}

fn remote_picture(state: &mut State, world_id: &str, url: String) -> Option<egui::TextureHandle> {
    if state.places.staged.is_some() || world_id.is_empty() || cfg!(test) {
        return None;
    }
    let ctx = state.ctx.clone()?;
    let picture = crate::worlds::Picture::Remote { world_id: world_id.to_string(), url, large: false };
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
    widgets::icon(p, pos2(rect.center().x, rect.center().y - 22.0), icons::STAR, 22.0, theme::fg_4());
    let txt = Txt::new(tr("The places you visit most will show up here."), 12.5).color(theme::fg_3());
    let mut job = egui::text::LayoutJob::single_section(txt.text.clone(), txt.format());
    job.wrap.max_width = rect.width() - 36.0;
    job.halign = egui::Align::Center;
    let galley = p.layout_job(job);
    p.galley(pos2(rect.center().x, rect.center().y + 2.0), galley, txt.color);
}

// ---------------------------------------------------------------------------------------------------------------------
// Play.

/// A card pressed: it is where Play goes. An instance that has to be open is asked about; nothing is started.
fn choose(state: &mut State, cx: &Cx, card: &Suggestion) {
    state.places.notice = None;
    match (&card.go, &card.reason) {
        (Go::Home, _) => state.places.desk.go_home(),
        // Signed out, or an instance only an invite lets in: a new one of its world, as from its sheet.
        (_, Reason::Previous { .. } | Reason::Friend { .. }) => super::previous::choose_offered(state, cx, card.go == Go::New),
        (Go::Instance(place), Reason::Recent { .. }) => choose_last(state, card, place),
        _ => state.places.desk.choose(Destination::Card { key: card.key.clone(), name: card.name.clone(), instance: matches!(card.go, Go::Instance(_) | Go::Invite { .. }) }, Origin::Card),
    }
}

/// The last instance's card: what VRChat said of it lately, when that was for this account, else it is asked about.
fn choose_last(state: &mut State, card: &Suggestion, place: &Place) {
    let visit = super::previous::Visit { world_id: place.world_id.clone(), instance: place.instance_id.clone(), name: card.name.clone(), at: state.now().max(0) as u64 };
    let asker = state.places.desk.asker().clone();
    let staged = state.places.staged.is_some();
    let said = {
        let read = state.places.open.lock().unwrap_or_else(|e| e.into_inner());
        // For the very account and profile chosen now, from when VRChat said it.
        read.value.as_ref().filter(|k| k.0 == visit.location() && k.2 == Some(true) && (staged || k.1 == asker)).map(|k| k.3)
    };
    match said {
        Some(at) => state.places.desk.choose_open(card.key.clone(), visit, Origin::Recent, at, asker),
        None => state.places.desk.choose(Destination::Instance { key: card.key.clone(), visit }, Origin::Recent),
    }
}

/// Play pressed in `mode`, on `profile` (`None`: no profile choice): what the selection says ([`Desk::decide`]). VRChat starts in the
/// chosen place, or as it always does; an instance not said open lately is asked about first; a new instance is first
/// made by VRChat for the account (on a thread: the button turns meanwhile).
pub fn play(state: &mut State, cx: &Cx, clock: f64, mode: LaunchMode, profile: Option<u32>) {
    use super::selection::Decision;
    let plain = |state: &mut State| {
        let action = action(&Launch::Plain, mode, profile).expect("a plain start");
        state.act(cx, clock, action);
    };
    // Where is not there (VRChat's own start is all Play does then).
    if !state.places.showing {
        return plain(state);
    }
    // The profile under the buttons is the one Where is for: a chip pressed in this very frame comes first.
    let asker = super::selection::Asker { account: state.places.desk.asker().account.clone(), profile };
    if state.places.desk.set_asker(asker) {
        return;
    }
    // The last instance's card, chosen for a new one of its world, finds it open again: asked for as that instance.
    if let (Destination::Card { key, .. }, None) = (state.places.desk.selection().destination().clone(), state.places.desk.busy()) {
        if let Some((card, place)) = state.places.cards.iter().find(|s| s.key == key).cloned().and_then(|c| match (&c.go, &c.reason) {
            (Go::Instance(p), Reason::Recent { .. }) => Some((c.clone(), p.clone())),
            _ => None,
        }) {
            choose_last(state, &card, &place);
        }
    }
    let card = match state.places.desk.decide(Instant::now(), clock) {
        Decision::Busy | Decision::Held => return,
        Decision::Plain => return plain(state),
        Decision::Check => return super::previous::check(state, cx, Some(mode)),
        Decision::Wait => return state.places.desk.wait(mode, Instant::now()),
        Decision::Enter(visit) => {
            let account = state.places.desk.asker().account.clone();
            return enter(state, cx, clock, &visit, mode, profile, account);
        }
        Decision::Card(key) => state.places.cards.iter().find(|s| s.key == key).cloned(),
        Decision::New(visit) => state.places.cards.iter().find(|s| s.key == visit.key()).cloned().filter(|c| c.go == Go::New),
    };
    // Its card is not there: nothing starts (the next frame says the choice went).
    let Some(card) = card else { return };
    let profile_account = profile.and_then(|n| state.data.profiles.iter().find(|p| p.profile == n)).and_then(|p| p.account.clone());
    let (me, can_create) = account(state, cx, profile_account.as_deref());
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let plan = launch(&card, kind_of(state, &card, can_create), can_create, seed);
    if let Some(action) = action(&plan, mode, profile) {
        return state.act(cx, clock, action);
    }
    let (Launch::Create { world, name, access, region }, Some(scope), Some(me)) = (plan, state.scope.clone(), me) else { return };
    let ticket = state.places.desk.create(mode);
    let (ctl, ctx) = (cx.ctl.clone(), state.ctx.clone());
    // Work of the window until VRChat was asked to start: it does not rest to the tray in the middle of it.
    let working = super::super::Working::begin();
    let _ = std::thread::Builder::new().name("where-new-instance".into()).spawn(move || {
        let _working = working;
        let made = make(ctl.as_ref(), scope, &world, &name, access, region);
        ticket.finish(made, |place| {
            let action = action(&Launch::At(place), mode, profile).expect("a start into the instance");
            if ctl.launcher(Action::As { account: me, action: Box::new(action) }) { Ok(()) } else { Err(tr("Orbiters - N4N0's service did not answer: try again.").into()) }
        });
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    });
}

/// VRChat starts in this instance: the selection said it is proven, for `account`. The start goes bound to that account,
/// whether its friends list is read yet or not: the service starts it for no other (verify3-r21 M1).
pub(super) fn enter(state: &mut State, cx: &Cx, clock: f64, visit: &super::previous::Visit, mode: LaunchMode, profile: Option<u32>, account: Option<String>) {
    let Some(place) = orbiters_core::launcher_feed::place(&visit.location(), &visit.name) else { return };
    // Nothing is proven for nobody.
    let (Some(action), Some(account)) = (action(&Launch::At(place), mode, profile), account) else { return };
    state.act_for(cx, clock, account, action);
}

/// Has VRChat make the new instance for the account signed in (the overlay's Go there's request). Going there is
/// [`super::selection::MakeTicket::finish`]'s, bound to that account, which the service checks again.
fn make(ctl: &dyn crate::backend::Backend, scope: Scope, world: &str, name: &str, access: CreateAccess, region: InstanceRegion) -> Result<Place, String> {
    let made = match ctl.vrchat_data(scope, Request::CreateInstance { world: world.into(), access, region }) {
        Ok(Response::CreatedInstance(made)) => made,
        Ok(Response::WorldError(e)) => return Err(orbiters_i18n::tr!("The instance could not be made: {e}", e = world_error(&e))),
        Ok(_) => return Err(tr("VRChat answered something unexpected.").into()),
        Err(e) => return Err(orbiters_i18n::tr!("The instance could not be made: {e}", e = e)),
    };
    Ok(Place { world_id: made.world.clone(), world_name: name.into(), world_picture: String::new(), instance_id: made.id.clone(), access: made.access, region: made.region.clone() })
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

    /// Verify5-r21 m4: the last member's events are nobody else's, and are read again at once for the next.
    #[test]
    fn events_are_the_members_they_were_read_for() {
        let at = Some(Instant::now());
        let slot: Shared<(String, Vec<EventSoon>)> = Arc::new(Mutex::new(Read { value: Some(("production/1".into(), vec![EventSoon::default()])), at, busy: false }));
        assert_eq!(theirs(&slot, "production/1").map(|e| e.len()), Some(1));
        assert!(theirs(&slot, "production/2").is_none());
        assert!(slot.lock().unwrap().at.is_none(), "due now");
        assert!(theirs(&slot, "production/1").is_none(), "gone");
    }

    fn inputs<'a>(invites: &'a [Invite], events: &'a [EventSoon], history: Option<&'a HistoryView>, last_open: Option<bool>, me: Option<&'a str>) -> Inputs<'a> {
        Inputs { now: NOW, invites, dismissed: &|_| false, events, history, last_open, me, selection: None, offer: None, several: false }
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
        let card = |key: &str, name: &str, instance: bool| {
            let mut d = Desk::default();
            d.choose(Destination::Card { key: key.into(), name: name.into(), instance }, Origin::Card);
            d
        };
        let settled = |d: &Desk, cards: &[Suggestion]| settle(d.selection(), cards);
        let invited = card("invite:not_1", "Chill House", true);
        assert_eq!(settled(&invited, &cards), None, "still there");
        // The invite put away (or accepted elsewhere): back to Home, named.
        let cards = suggestions(&inputs(&[], &[], Some(&history), Some(true), Some(ME)));
        assert_eq!(settled(&invited, &cards).as_deref(), Some("Chill House"));
        // The last instance closed: its card stays (a new one of its world) but the choice was for that instance.
        let mut last = Desk::default();
        last.choose(
            Destination::Instance { key: format!("recent:{}", world(3)), visit: super::super::previous::Visit { world_id: world(3), instance: "9~region(eu)".into(), name: "World 3".into(), at: 0 } },
            Origin::Recent,
        );
        assert_eq!(settled(&last, &cards), None);
        let closed = suggestions(&inputs(&[], &[], Some(&history), Some(false), Some(ME)));
        assert_eq!(settled(&last, &closed).as_deref(), Some("World 3"));
        // A card for a new instance stays chosen whatever the old instance does.
        let new = card(&format!("recent:{}", world(3)), "World 3", false);
        assert_eq!(settled(&new, &closed), None);
        let open = suggestions(&inputs(&[], &[], Some(&history), Some(true), Some(ME)));
        assert_eq!(settled(&new, &open), None);
        // An event over.
        let events = [EventSoon { id: "ev".into(), title: "Meetup".into(), starts_at: NOW - 3600, ends_at: Some(NOW + 60), ..Default::default() }];
        let event = card("event:ev", "Meetup", false);
        assert_eq!(settled(&event, &suggestions(&inputs(&[], &events, None, None, None))), None);
        let later = Inputs { now: NOW + 61, ..inputs(&[], &events, None, None, None) };
        assert_eq!(settled(&event, &suggestions(&later)).as_deref(), Some("Meetup"));
        // Home chosen: nothing to say.
        assert_eq!(settled(&Desk::default(), &[]), None);
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

    /// REQUEST21 7: a previous instance chosen is Where's card right after Home ("Previous instance · …"): open (VRChat
    /// said so), Play starts VRChat in that very instance; being asked about, closed or not known, its card is there and
    /// nothing goes from it; taken as a new one, VRChat makes a new instance of its world (the kind chosen), or without a
    /// VRChat sign-in a public one is named here. It is the strongest: its world shows once. Another card chosen, it is
    /// still offered, and starts nothing without being asked about again.
    #[test]
    fn a_previous_instance_chosen_starts_vrchat_in_it_while_open_else_in_a_new_one() {
        use super::super::previous::Visit;
        let instance = "48213~hidden(usr_00000000-0000-4000-8000-000000000004)~region(eu)~nonce(0a1b2c3d-0000-4000-8000-000000000000)";
        let v = Visit { world_id: world(3), instance: instance.into(), name: "World 3".into(), at: NOW as u64 - 86_400 };
        let history = view(Some((3, "9~region(use)")), &[3, 4]);
        let mode = LaunchMode::Vr;
        let chosen = |v: &Visit, verified: Verified, new: bool| {
            let mut d = Desk::default();
            d.set_asker(Asker { account: Some(ME.into()), profile: Some(0) });
            let key = v.key();
            d.stand_in(if new { Destination::NewOf { key, visit: v.clone() } } else { Destination::Instance { key, visit: v.clone() } }, Origin::Previous, verified);
            d
        };
        let cards_of = |d: &Desk, me: Option<&'static str>| suggestions(&Inputs { selection: Some((d.selection(), d.asker())), offer: d.offer(), ..inputs(&[], &[], Some(&history), Some(true), me) });
        let open = || Verified::Open { at: Instant::now(), by: Asker::default() };
        // Open: that instance, on the profile chosen.
        let mut d = chosen(&v, open(), false);
        let cards = cards_of(&d, Some(ME));
        assert_eq!(keys(&cards)[..2], ["home".to_string(), v.key()]);
        assert!(!cards.iter().any(|c| c.key.starts_with("recent:") || c.key == format!("often:{}", world(3))), "its world once: {:?}", keys(&cards));
        assert_eq!(cards[1].reason, Reason::Previous { at: v.at });
        assert!(reason_line(&cards[1], NOW).0.contains("1 d ago"), "{}", reason_line(&cards[1], NOW).0);
        let Some(Action::PlayAt { destination, profile: 2, mode: m }) = action(&launch(&cards[1], CreateAccess::FriendsPlus, true, 0), mode, Some(2)) else { panic!() };
        assert_eq!((m, destination.world_id.as_str(), destination.instance_id.as_str()), (mode, world(3).as_str(), instance));
        let args = orbiters_core::vrchat_launch::arguments_on(mode, &orbiters_core::vrchat_launch::LaunchSettings { performance: false, ..Default::default() }, Vec::new(), 2, Some((&destination.world_id, &destination.instance_id)));
        assert_eq!(args.last(), Some(&format!("vrchat://launch?ref=orbiters.cc&id={}:{instance}", world(3))), "the link last: {args:?}");
        assert!(args.contains(&"--profile=2".to_string()));
        // Another card chosen: still offered, and nothing goes from it as it is.
        d.go_home();
        let cards = cards_of(&d, Some(ME));
        assert_eq!(keys(&cards)[..2], ["home".to_string(), v.key()]);
        assert!(matches!(&cards[1].go, Go::Held(p) if p.instance_id == instance));
        assert_eq!(action(&launch(&cards[1], CreateAccess::FriendsPlus, true, 0), mode, Some(0)), None);
        // Someone else's invite instance: not offered by its link (a new instance of its world).
        let theirs = Visit { instance: "77~private(usr_00000000-0000-4000-8000-000000000009)~region(eu)".into(), ..v.clone() };
        let mut d = chosen(&theirs, open(), false);
        d.go_home();
        assert_eq!(cards_of(&d, Some(ME))[1].go, Go::New);
        // Being asked about, not asked, closed, not known: its card says where it is, and starts nothing.
        for verified in [Verified::Unverified, Verified::Checking { generation: 0, since: Instant::now() }, Verified::Closed, Verified::Unknown] {
            let cards = cards_of(&chosen(&v, verified.clone(), false), Some(ME));
            assert!(matches!(&cards[1].go, Go::Held(p) if p.instance_id == instance), "{verified:?}: {:?}", cards[1].go);
            assert_eq!(action(&launch(&cards[1], CreateAccess::FriendsPlus, true, 0), mode, Some(0)), None, "{verified:?}");
        }
        // A new instance of its world taken: VRChat makes it (the kind chosen), in the instance's own region.
        let cards = cards_of(&chosen(&v, Verified::Unverified, true), Some(ME));
        assert_eq!(cards[1].go, Go::New);
        assert!(cards[1].local(), "a new instance of a world of this computer's history");
        assert_eq!(launch(&cards[1], CreateAccess::Friends, true, 0), Launch::Create { world: world(3), name: "World 3".into(), access: CreateAccess::Friends, region: InstanceRegion::Eu });
        // Signed out of VRChat: a public one named here, VRChat's own grammar.
        let cards = cards_of(&chosen(&v, Verified::Unverified, true), None);
        let Launch::At(place) = launch(&cards[1], CreateAccess::FriendsPlus, false, 42) else { panic!() };
        assert!(crate::controller::vrchat_data::public_instance(&place.instance_id) && place.instance_id.ends_with("~region(eu)"), "{}", place.instance_id);
        assert_eq!(action(&Launch::At(place.clone()), mode, Some(0)), Some(Action::PlayAt { mode, profile: 0, destination: place }));
    }

    /// REQUEST23: which instances take the account without asking (Select) and which need an invite (Request), and that
    /// Where's rejoin of its own history and the friends' Select are one rule.
    #[test]
    fn joinable_without_asking_decides_select_or_request() {
        let friend = "usr_00000000-0000-4000-8000-0000000000f1";
        let stranger = "usr_00000000-0000-4000-8000-0000000000f2";
        let is_friend = |id: &str| id == friend;
        for (instance, joinable) in [
            ("1~region(eu)".to_string(), true),
            (format!("2~hidden({stranger})~region(eu)"), true),
            (format!("3~friends({friend})~region(eu)"), true),
            (format!("4~friends({ME})~region(eu)"), true),
            (format!("5~friends({stranger})~region(eu)"), false),
            (format!("6~private({stranger})~region(eu)"), false),
            (format!("7~private({stranger})~canRequestInvite~region(eu)"), false),
            (format!("8~private({ME})~region(eu)"), true),
            ("9~group(grp_00000000-0000-4000-8000-000000000001)~groupAccessType(public)~region(eu)".to_string(), true),
            ("10~group(grp_00000000-0000-4000-8000-000000000001)~groupAccessType(plus)~region(eu)".to_string(), false),
            ("11~group(grp_00000000-0000-4000-8000-000000000001)~groupAccessType(members)~region(eu)".to_string(), false),
        ] {
            assert_eq!(joinable_without_asking(&instance, ME, &is_friend), joinable, "{instance}");
        }
        // Signed out (no account): nothing of its own; the rest as above.
        assert!(!joinable_without_asking(&format!("8~private({ME})"), "", &is_friend));
        // The history: a friends-only instance joined then is taken as its owner's friend's.
        assert!(rejoinable(&format!("5~friends({stranger})~region(eu)"), ME));
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
