//! Explorer (REQUEST13, `night-1119/explorer-contract.md`), in the tray service: the notice on arriving in a world that has
//! no public 3D picture on Orbiters yet, the two share buttons right after a 3D picture is done ([`Offer`]), and the
//! member's Explorer points. Every surface (the window, the SteamVR overlay's band and camera tools, the desktop's camera
//! tools) draws what [`Status`] says and sends a [`Command`] back; nothing here draws.
//!
//! The notice is welcome, not nagging: at most once per world for this computer's account (remembered on disk), never
//! within [`QUIET`] of the last one, only after the person has been in the world [`SETTLE`], never while a recording, a
//! call, VRChat's camera or N4N0's menu is in the way (the overlay holds it back while its own menus are up,
//! [`Command::NoticeHeld`]), and not at all with Settings' switch off. The world is asked about by its id alone,
//! without signing in.
//!
//! The share buttons belong to one picture, named by [`Offer::id`]: a press says which picture it was for, and a press
//! for another picture than the one offered now does nothing. A share is a Photos & captures job (the same upload
//! records, preparation and server calls, through N4N0's menu's [`super::menu_shares`], which waits for the 3D view);
//! it is said done only when Orbiters' answer shows it (`item.public`, the grants asked for), and what it earned is the
//! member's points read before and after it.

use super::Shared;
use crate::controller::shares::{Outcome, ShareJob};
use orbiters_core::account::Server;
use orbiters_core::explorer::{self as ex, Explorer, Title, WorldLookup};
use orbiters_i18n::{mark, tr};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long the person is in a world before it may be said that it has no 3D picture yet: they look around first, and
/// someone only passing through hears nothing.
pub const SETTLE: Duration = Duration::from_secs(25);
/// The least time between two notices, whatever the worlds.
pub const QUIET: Duration = Duration::from_secs(10 * 60);
/// How long a notice waits behind what is in the way before it is let go for this visit.
pub const HOLD_MAX: Duration = Duration::from_secs(3 * 60);
/// How long a notice shows.
pub const NOTICE_MS: u64 = 12_000;
/// How long the share buttons stay after a picture with nothing pressed, and after a share ended.
pub const OFFER_MS: u64 = 3 * 60_000;
pub const RESULT_MS: u64 = 30_000;
/// How long a world's answer is kept (the server caches it five minutes too).
const LOOKUP_KEPT: Duration = Duration::from_secs(5 * 60);
/// Worlds remembered as told, per account (the oldest go first).
const REMEMBERED: usize = 4000;
/// How often the member's points are read again while signed in.
const POINTS_EVERY: Duration = Duration::from_secs(10 * 60);

pub const NOTICE_TITLE: &str = mark("No 3D picture of this world yet");
pub const NOTICE_WHY: &str = mark("Take one and share it publicly: everyone on Orbiters sees it");
pub const NOTICE_EARNS: &str = mark("Take one and share it publicly: +1 Explorer point");
pub const TAKE: &str = mark("Take a 3D picture");
pub const NOT_NOW: &str = mark("Not now");

/// The time now, Unix ms.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// What every surface shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub notice: Option<WorldNotice>,
    pub offer: Option<Offer>,
    /// The signed-in member's points and title, once read.
    pub points: Option<Explorer>,
}

/// "No 3D picture of this world yet", while it is up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldNotice {
    pub id: u64,
    pub world_id: String,
    pub world_name: String,
    /// What a picture here would earn, when the server said.
    pub earns: Option<f64>,
    pub until_ms: u64,
    pub for_ms: u64,
}

impl WorldNotice {
    /// What is left of its time, 1 to 0.
    pub fn left(&self, now_ms: u64) -> f32 {
        (self.until_ms.saturating_sub(now_ms) as f32 / self.for_ms.max(1) as f32).clamp(0.0, 1.0)
    }
    pub fn up(&self, now_ms: u64) -> bool {
        now_ms < self.until_ms
    }
}

/// The two buttons after a 3D picture, and how its shares went.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    /// The picture's identity: what every press names ([`offer_id`]).
    pub id: u64,
    /// Its folder, as History names it.
    pub key: String,
    pub world_id: String,
    /// What sharing it publicly would earn, when the world's lookup said ([`ex::WorldLookup::next_earns`]).
    pub would_earn: Option<f64>,
    pub until_ms: u64,
    pub public: Step,
    pub people: Step,
}

impl Offer {
    /// Something runs: its buttons wait.
    pub fn busy(&self) -> bool {
        matches!(self.public, Step::Busy { .. }) || matches!(self.people, Step::Busy { .. })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Step {
    #[default]
    Idle,
    /// On its way: its 3D view still being made, or how far its files went (0 to 1, none before they go).
    Busy { making: bool, sent: Option<f32> },
    Done(Done),
    /// It is explicit: confirming that is done in Photos & captures.
    Explicit,
    Failed(String),
}

/// A share Orbiters confirmed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Done {
    /// Friends it was shared with (a "Share to …").
    pub people: u32,
    /// What it earned (a public share): none when the points could not be read before or after.
    pub earned: Option<f64>,
    /// Why it earned nothing, when it did not.
    pub nothing: Option<Nothing>,
    /// The member's points after it.
    pub after: Option<Explorer>,
    /// A title reached by it.
    pub reached: Option<Title>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Nothing {
    /// The world already has its three ranked pictures.
    WorldHasThree,
    /// This picture was shared publicly before: it earned then.
    SharedBefore,
    /// Orbiters did not say why.
    Unknown,
}

/// Why nothing was earned, from what the world's lookup said before the share and whether the picture was public before.
pub fn why_nothing(would_earn: Option<f64>, public_before: bool) -> Nothing {
    if public_before {
        Nothing::SharedBefore
    } else if would_earn == Some(0.0) {
        Nothing::WorldHasThree
    } else {
        Nothing::Unknown
    }
}

/// What a surface asks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// "Take a 3D picture" on notice `id`.
    NoticeTake { id: u64 },
    /// Its close.
    NoticeDismiss { id: u64 },
    /// It could not show: a menu is up. It waits (a little).
    NoticeHeld { id: u64 },
    /// "Share publicly" on the picture `offer`; `confirmed` the first time's question was answered yes (the settings
    /// remember it).
    SharePublic { offer: u64, confirmed: bool },
    /// "Share to …" sent: the friends chosen (VRChat user id, name).
    ShareTo { offer: u64, recipients: Vec<(String, String)> },
    /// The buttons put away.
    OfferDismiss { offer: u64 },
}

/// A picture's identity from its folder's name (whichever way its path is written).
pub fn offer_id(key: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let name = Path::new(key).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    h.finish() | 1
}

/// What stands in the notice's way now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InTheWay {
    pub recording: bool,
    pub call: bool,
    pub camera_open: bool,
    pub menu_open: bool,
}
impl InTheWay {
    pub fn any(self) -> bool {
        self.recording || self.call || self.camera_open || self.menu_open
    }
}

/// What to do with a world that has no public 3D picture, now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Show,
    /// Not yet: in the world too short a time, something in the way, too soon after the last.
    Wait,
    /// Not on this visit.
    Drop,
}

/// The notice's rules (tested alone): `since` in the world this long, `told` already about this world, the last notice
/// `last` ago, `held` the notice waited this long behind something.
pub fn decide(on: bool, told: bool, since: Duration, last: Option<Duration>, way: InTheWay, waited: Duration) -> Decision {
    if !on || told {
        return Decision::Drop;
    }
    if since < SETTLE {
        return Decision::Wait;
    }
    if waited > HOLD_MAX {
        return Decision::Drop;
    }
    if way.any() || last.is_some_and(|l| l < QUIET) {
        return Decision::Wait;
    }
    Decision::Show
}

/// Worlds told about, per account, on disk.
#[derive(Default, Serialize, Deserialize)]
struct Told {
    accounts: BTreeMap<String, Vec<String>>,
}

fn told_path() -> PathBuf {
    orbiters_core::paths::local_data_root().join("explorer-worlds.json")
}

impl Told {
    fn read(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    fn has(&self, account: &str, world: &str) -> bool {
        self.accounts.get(account).is_some_and(|w| w.iter().any(|x| x == world))
    }
    fn add(&mut self, account: &str, world: &str) {
        let worlds = self.accounts.entry(account.to_string()).or_default();
        if !worlds.iter().any(|x| x == world) {
            worlds.push(world.to_string());
        }
        let over = worlds.len().saturating_sub(REMEMBERED);
        worlds.drain(..over);
    }
    fn write(&self, path: &Path) {
        if let Ok(bytes) = serde_json::to_vec(self) {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = orbiters_core::write_atomic(path, &bytes);
        }
    }
}

/// A share asked from the buttons.
struct Asked {
    public: bool,
    people: Vec<String>,
    /// Points before it (a public share), read when it was asked.
    before: Option<Explorer>,
    /// The picture was public before (it earns nothing again).
    public_before: bool,
    since: Instant,
    /// The result is being read (the points after it).
    reading: bool,
}

/// What the explorer keeps between ticks.
#[derive(Default)]
pub(super) struct ExplorerState {
    /// The world the person is in, since when, and whether it was asked about on this visit.
    here: Option<(String, Instant, bool)>,
    /// Worlds' answers, when.
    lookups: HashMap<String, (Option<WorldLookup>, Instant)>,
    /// A world answered as having none, waiting to be told (since when it waits).
    candidate: Option<(String, Instant)>,
    last_notice: Option<Instant>,
    next_id: u64,
    told: Option<Told>,
    /// The share asked per offer side.
    public: Option<Asked>,
    people: Option<Asked>,
    points_at: Option<(Instant, String)>,
    /// The overlay held the notice back at this time last.
    held: Option<Instant>,
}

/// The explorer's own thread: a look once a second while the service runs.
pub(super) fn spawn(ctl: Shared) {
    let started = std::thread::Builder::new().name("explorer".into()).spawn(move || {
        while !ctl.quitting.load(std::sync::atomic::Ordering::Relaxed) {
            ctl.explorer_tick();
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    if let Err(e) = started {
        eprintln!("explorer: {e}"); // i18n: not shown (a log's line)
    }
}

impl super::Controller {
    /// Once a second, on its own thread: arrival in a world, the offer's shares and timeouts, the points.
    pub(super) fn explorer_tick(self: &Shared) {
        let snap = self.view();
        let server = if snap.settings.development { Server::Development } else { Server::Production };
        let now = Instant::now();
        let mut s = self.explorer.lock().unwrap();
        // Arrival.
        let world = &snap.vrchat.world;
        let here = world.world_id.clone().filter(|_| snap.vrchat.running && world.in_world).filter(|w| ex::is_world_id(w));
        if here.as_deref() != s.here.as_ref().map(|h| h.0.as_str()) {
            s.here = here.map(|w| (w, now, false));
            s.candidate = None;
        }
        let account = account_key(&snap);
        let on = snap.settings.explorer.world_notices;
        if let Some((world_id, since, asked)) = s.here.clone() {
            let told = s.told.get_or_insert_with(|| Told::read(&told_path())).has(&account, &world_id);
            if on && !told && !asked && now.duration_since(since) >= SETTLE {
                s.here.as_mut().unwrap().2 = true;
                let fresh = s.lookups.get(&world_id).filter(|(_, at)| at.elapsed() < LOOKUP_KEPT).map(|(l, _)| *l);
                match fresh {
                    Some(lookup) => {
                        if lookup.is_some_and(|l| !l.has_public) {
                            s.candidate = Some((world_id.clone(), now));
                        }
                    }
                    None => self.explorer_look_up(server, world_id.clone(), true),
                }
            }
            if let Some((cand, waiting)) = s.candidate.clone().filter(|(c, _)| *c == world_id) {
                let way = InTheWay {
                    recording: snap.is_capturing() || snap.capture.state == crate::state::CaptureState::Stopping,
                    call: snap.n4n0.calls.call.is_some(),
                    camera_open: snap.camera_open_said == Some(true),
                    menu_open: snap.n4n0.screen_open || snap.n4n0.calls.menu_open,
                };
                let waited = now.duration_since(waiting);
                match decide(on, told, now.duration_since(since), s.last_notice.map(|l| now.duration_since(l)), way, waited) {
                    Decision::Show if self.explorer_status().notice.is_none() => {
                        s.next_id += 1;
                        let earns = s.lookups.get(&cand).and_then(|(l, _)| l.and_then(|l| l.next_earns));
                        let notice = WorldNotice {
                            id: s.next_id,
                            world_id: cand.clone(),
                            world_name: world.world_name.clone().unwrap_or_default(),
                            earns,
                            until_ms: now_ms() + NOTICE_MS,
                            for_ms: NOTICE_MS,
                        };
                        s.candidate = None;
                        s.held = None;
                        s.last_notice = Some(now);
                        let told = s.told.get_or_insert_with(Told::default);
                        told.add(&account, &cand);
                        told.write(&told_path());
                        self.update(|st| st.explorer.notice = Some(notice.clone()));
                    }
                    Decision::Drop => s.candidate = None,
                    _ => {}
                }
            }
        }
        // A notice whose time is over goes; one for another world than this one too.
        let status = self.explorer_status();
        if let Some(n) = status.notice.as_ref() {
            if !n.up(now_ms()) || s.here.as_ref().is_none_or(|h| h.0 != n.world_id) {
                self.update(|st| st.explorer.notice = None);
            }
        }
        // The offer: its shares, then its time.
        self.explorer_follow_shares(&mut s, &snap, server);
        let status = self.explorer_status();
        if let Some(offer) = status.offer.as_ref() {
            let recording = snap.is_capturing();
            if (now_ms() >= offer.until_ms && !offer.busy()) || recording {
                self.update(|st| st.explorer.offer = None);
                s.public = None;
                s.people = None;
            }
        }
        // The points: while signed in, now and then; gone at sign-out.
        if account.is_empty() {
            if status.points.is_some() {
                self.update(|st| st.explorer.points = None);
            }
            s.points_at = None;
        } else if s.points_at.as_ref().is_none_or(|(at, who)| *who != account || at.elapsed() > POINTS_EVERY) {
            s.points_at = Some((now, account.clone()));
            self.explorer_read_points(server, None);
        }
    }

    fn explorer_status(&self) -> Status {
        self.view().explorer.clone()
    }

    /// Asks whether `world_id` has a public 3D picture (by its id alone), on a thread; `notice`: for the arrival.
    fn explorer_look_up(self: &Shared, server: Server, world_id: String, notice: bool) {
        let ctl = self.clone();
        std::thread::spawn(move || {
            let lookup = orbiters_core::account::Api::new(server).world_public_capture(&world_id).ok().flatten();
            let mut s = ctl.explorer.lock().unwrap();
            s.lookups.insert(world_id.clone(), (lookup, Instant::now()));
            if notice && lookup.is_some_and(|l| !l.has_public) && s.here.as_ref().is_some_and(|h| h.0 == world_id) {
                s.candidate = Some((world_id.clone(), Instant::now()));
            }
            drop(s);
            // The offer of a picture of this world says what it would earn.
            if let Some(earns) = lookup.and_then(|l| l.next_earns) {
                ctl.update_if(|st| match st.explorer.offer.as_mut().filter(|o| o.world_id == world_id && o.would_earn != Some(earns)) {
                    Some(o) => {
                        o.would_earn = Some(earns);
                        true
                    }
                    None => false,
                });
            }
        });
    }

    /// Reads the member's points on a thread; with `then`, a public share's result for offer `then` is made from them.
    fn explorer_read_points(self: &Shared, server: Server, then: Option<u64>) {
        let ctl = self.clone();
        std::thread::spawn(move || {
            let points = ctl.account_api().and_then(|(api, token)| api.explorer(&token).ok());
            if let Some(p) = points.clone() {
                ctl.update(|st| st.explorer.points = Some(p.clone()));
            }
            let Some(offer) = then else { return };
            let mut s = ctl.explorer.lock().unwrap();
            let Some(asked) = s.public.take() else { return };
            let mut done = Done { people: 0, earned: None, nothing: None, after: points.clone(), reached: None };
            if let Some(after) = points.as_ref() {
                done.earned = ex::earned(asked.before.as_ref(), after);
                done.reached = ex::title_reached(asked.before.as_ref(), after);
            }
            let would = ctl.view().explorer.offer.as_ref().and_then(|o| o.would_earn);
            if done.earned == Some(0.0) || (done.earned.is_none() && asked.public_before) {
                done.nothing = Some(why_nothing(would, asked.public_before));
            }
            drop(s);
            let words = result_words(&done);
            ctl.update(|st| {
                if let Some(o) = st.explorer.offer.as_mut().filter(|o| o.id == offer) {
                    o.public = Step::Done(done.clone());
                    o.until_ms = now_ms() + RESULT_MS;
                }
            });
            ctl.notify(super::NoticeKind::Success, words);
            let _ = server;
        });
    }

    /// A 3D picture was saved (`dir`): its two buttons are offered, in place of the last picture's.
    pub(super) fn explorer_offer(self: &Shared, dir: &Path) {
        let snap = self.view();
        let key = snap.settings.sessions_dir.join(dir.file_name().unwrap_or_default()).to_string_lossy().into_owned();
        let world_id = snap.vrchat.world.world_id.clone().unwrap_or_default();
        let server = if snap.settings.development { Server::Development } else { Server::Production };
        let would_earn = {
            let mut s = self.explorer.lock().unwrap();
            s.public = None;
            s.people = None;
            s.lookups.get(&world_id).filter(|(_, at)| at.elapsed() < LOOKUP_KEPT).and_then(|(l, _)| l.and_then(|l| l.next_earns))
        };
        let offer = Offer { id: offer_id(&key), key, world_id: world_id.clone(), would_earn, until_ms: now_ms() + OFFER_MS, public: Step::Idle, people: Step::Idle };
        self.update(|st| st.explorer.offer = Some(offer.clone()));
        if would_earn.is_none() && ex::is_world_id(&world_id) {
            self.explorer_look_up(server, world_id, false);
        }
    }

    /// What a surface asked.
    pub fn explorer_command(self: &Shared, command: Command) {
        let status = self.explorer_status();
        match command {
            Command::NoticeTake { id } => {
                if status.notice.as_ref().is_some_and(|n| n.id == id) {
                    self.update(|st| st.explorer.notice = None);
                    // The existing flow: a 3D picture once VRChat's camera is open (it waits for it, and says so).
                    self.n4n0_command(crate::ipc::N4n0Command::Photo3DWhenCamera { seconds: 3 });
                }
            }
            Command::NoticeDismiss { id } => {
                if status.notice.as_ref().is_some_and(|n| n.id == id) {
                    self.update(|st| st.explorer.notice = None);
                }
            }
            Command::NoticeHeld { id } => {
                // Held behind a menu: its time starts again, up to HOLD_MAX after it came.
                let mut s = self.explorer.lock().unwrap();
                let first = *s.held.get_or_insert_with(Instant::now);
                drop(s);
                if first.elapsed() < HOLD_MAX {
                    self.update_if(|st| match st.explorer.notice.as_mut().filter(|n| n.id == id) {
                        Some(n) => {
                            n.until_ms = now_ms() + n.for_ms;
                            true
                        }
                        None => false,
                    });
                }
            }
            Command::OfferDismiss { offer } => {
                if status.offer.as_ref().is_some_and(|o| o.id == offer && !o.busy()) {
                    self.update(|st| st.explorer.offer = None);
                }
            }
            Command::SharePublic { offer, confirmed } => self.explorer_share(offer, true, Vec::new(), confirmed),
            Command::ShareTo { offer, recipients } => self.explorer_share(offer, false, recipients, false),
        }
    }

    fn explorer_share(self: &Shared, offer: u64, public: bool, recipients: Vec<(String, String)>, confirmed: bool) {
        let Some(o) = self.explorer_status().offer.filter(|o| o.id == offer) else { return };
        let step = if public { &o.public } else { &o.people };
        if matches!(step, Step::Busy { .. }) || (public && matches!(step, Step::Done(_))) {
            return;
        }
        // Public and irreversible in practice: asked once, the first time.
        let settings = self.settings();
        if public && !settings.explorer.public_confirmed {
            if !confirmed {
                return;
            }
            let mut changed = settings.clone();
            changed.explorer.public_confirmed = true;
            self.save_settings(changed);
        }
        if !public && recipients.is_empty() {
            return;
        }
        let set = move |st: &mut crate::state::Snapshot, step: Step| {
            if let Some(o) = st.explorer.offer.as_mut().filter(|o| o.id == offer) {
                if public {
                    o.public = step;
                } else {
                    o.people = step;
                }
                o.until_ms = o.until_ms.max(now_ms() + OFFER_MS);
            }
        };
        self.update(|st| set(st, Step::Busy { making: false, sent: None }));
        let ctl = self.clone();
        let key = o.key.clone();
        std::thread::spawn(move || {
            // The points before (a public share): what it earned is the difference.
            let before = if public { ctl.account_api().and_then(|(api, token)| api.explorer(&token).ok()) } else { None };
            let ask = orbiters_n4n0_core::captures::ShareAsk {
                key: key.clone(),
                public,
                recipients: recipients.iter().map(|(id, name)| orbiters_n4n0_core::captures::Friend { id: id.clone(), name: name.clone() }).collect(),
                acknowledge_explicit: false,
                ..Default::default()
            };
            let public_before = false;
            match ctl.menu_capture_share(ask) {
                Ok(()) => {
                    let asked = Asked { public, people: recipients.iter().map(|r| r.0.clone()).collect(), before, public_before, since: Instant::now(), reading: false };
                    let mut s = ctl.explorer.lock().unwrap();
                    if public {
                        s.public = Some(asked);
                    } else {
                        s.people = Some(asked);
                    }
                }
                Err(message) => ctl.update(|st| set(st, Step::Failed(message.clone()))),
            }
        });
    }

    /// Where the offer's shares stand, from N4N0's menu's asks and Photos & captures' jobs; a share confirmed by Orbiters'
    /// answer only.
    fn explorer_follow_shares(self: &Shared, s: &mut ExplorerState, snap: &crate::state::Snapshot, server: Server) {
        let Some(offer) = snap.explorer.offer.as_ref() else { return };
        for public in [true, false] {
            let Some(asked) = (if public { s.public.as_mut() } else { s.people.as_mut() }) else { continue };
            if asked.reading {
                continue;
            }
            let state = self.menu_share_state(&offer.key);
            let step = match state {
                Some(super::menu_shares::AskState::Waiting) => Step::Busy { making: true, sent: None },
                Some(super::menu_shares::AskState::Failed(message)) => Step::Failed(message),
                Some(super::menu_shares::AskState::Job(id)) => match snap.capture_shares.iter().find(|j| j.id == id) {
                    None | Some(ShareJob { outcome: None, .. }) => {
                        let sent = snap.capture_shares.iter().find(|j| j.id == id).and_then(|j| match j.stage {
                            crate::controller::shares::Stage::Sending { sent, total } if total > 0 => Some(sent as f32 / total as f32),
                            _ => None,
                        });
                        Step::Busy { making: false, sent }
                    }
                    Some(ShareJob { outcome: Some(outcome), .. }) => verified(outcome, public, &asked.people),
                },
                None if asked.since.elapsed() < Duration::from_secs(10) => Step::Busy { making: false, sent: None },
                None => Step::Failed(tr("The share did not start: try again from Photos & captures").into()),
            };
            match step {
                // A public share Orbiters confirmed: the points after it, then it is done.
                Step::Done(_) if public => {
                    asked.reading = true;
                    self.explorer_read_points(server, Some(offer.id));
                }
                step => {
                    let finished = matches!(step, Step::Done(_) | Step::Failed(_) | Step::Explicit);
                    let id = offer.id;
                    self.update_if(|st| match st.explorer.offer.as_mut().filter(|o| o.id == id) {
                        Some(o) => {
                            let slot = if public { &mut o.public } else { &mut o.people };
                            if *slot == step {
                                return false;
                            }
                            *slot = step.clone();
                            if finished {
                                o.until_ms = now_ms() + RESULT_MS;
                            }
                            true
                        }
                        None => false,
                    });
                    if finished {
                        if public {
                            s.public = None;
                        } else {
                            s.people = None;
                        }
                    }
                }
            }
        }
    }
}

/// What a share job's outcome is, believed only as far as Orbiters' answer shows it: public, its id there, the friends
/// asked among its grants (verify-core 4: a 2xx without them is no success).
pub fn verified(outcome: &Outcome, public: bool, people: &[String]) -> Step {
    use orbiters_core::shares::Shared;
    match outcome {
        Outcome::Shared(Shared::Item { item }) if item.id != 0 && (!public || item.public) => {
            let missing = people.iter().filter(|p| !item.grants.iter().any(|g| &g.vrchat_user_id == *p)).count();
            if public || missing == 0 {
                Step::Done(Done { people: people.len() as u32, earned: None, nothing: None, after: None, reached: None })
            } else {
                Step::Failed(tr("Orbiters did not confirm who sees it: look in Photos & captures").into())
            }
        }
        Outcome::Shared(Shared::Item { .. }) => Step::Failed(tr("Orbiters did not confirm the share: look in Photos & captures").into()),
        Outcome::Shared(Shared::Request { .. }) => Step::Failed(tr("It went as a request: look in Photos & captures").into()),
        Outcome::Failed { explicit: true, .. } => Step::Explicit,
        Outcome::Failed { message, .. } => Step::Failed(message.clone()),
        _ => Step::Failed(tr("Orbiters did not confirm the share: look in Photos & captures").into()),
    }
}

/// Who is signed in, as the told worlds are kept by: empty when nobody.
fn account_key(snap: &crate::state::Snapshot) -> String {
    match &snap.account.state {
        crate::state::AccountState::SignedIn { username } => username.clone(),
        crate::state::AccountState::Offline { username: Some(u) } | crate::state::AccountState::Checking { username: Some(u) } => u.clone(),
        _ => String::new(),
    }
}

pub const SHARED_PUBLICLY: &str = mark("Shared publicly");
pub const EARNED_ONE: &str = mark("+1 Explorer point");
pub const EARNED_SOME: &str = mark("+{points} Explorer points");
pub const NONE_WORLD_FULL: &str = mark("No points: this world already has three public 3D pictures");
pub const NONE_SHARED_BEFORE: &str = mark("No points: this picture was shared publicly before");
pub const NONE_UNKNOWN: &str = mark("No new points this time");
pub const ONE_POINT: &str = mark("1 point");
pub const POINTS: &str = mark("{points} points");
pub const TO_NEXT: &str = mark("{remaining} to {title}");
pub const NEW_TITLE: &str = mark("New title: {title}");

/// What one share earned, in words: "+1 Explorer point", "+0.5 Explorer points", or why nothing.
pub fn earned_words(done: &Done) -> Option<String> {
    match (done.earned, done.nothing) {
        (Some(e), _) if e == 1.0 => Some(tr(EARNED_ONE).into()),
        (Some(e), _) if e > 0.0 => Some(tr!("+{points} Explorer points", points = orbiters_ui::format::points(e))),
        (_, Some(Nothing::WorldHasThree)) => Some(tr(NONE_WORLD_FULL).into()),
        (_, Some(Nothing::SharedBefore)) => Some(tr(NONE_SHARED_BEFORE).into()),
        (_, Some(Nothing::Unknown)) => Some(tr(NONE_UNKNOWN).into()),
        _ => None,
    }
}

/// A total of points in words: "1 point", "12.5 points", "-1 points".
pub fn points_words(points: f64) -> String {
    if (points - 1.0).abs() < 1e-9 {
        tr(ONE_POINT).into()
    } else {
        tr!("{points} points", points = orbiters_ui::format::points(points))
    }
}

/// A title's name in the language now, by its stable key (the server's name when the key is not one of these).
pub fn title_name(title: &Title) -> String {
    let english = match title.key.as_str() {
        "explorer" => mark("Explorer"),
        "navigator" => mark("Navigator"),
        "discoverer" => mark("Discoverer"),
        "marcoPolo" => mark("Marco polo"),
        "multiverseTraveler" => mark("Multiverse traveler"),
        _ => return title.name.clone(),
    };
    tr(english).into()
}

/// Where the member stands, in words: "12.5 points · 37.5 to Navigator", or the title at the top.
pub fn standing_words(e: &Explorer) -> String {
    let total = points_words(e.points);
    match (e.next_title(), e.title.as_ref()) {
        (Some(next), _) => {
            let title = Title { key: next.key.clone(), points: next.points, name: next.name.clone() };
            format!("{total} · {}", tr!("{remaining} to {title}", remaining = orbiters_ui::format::points(next.remaining), title = title_name(&title)))
        }
        (None, Some(t)) => format!("{total} · {}", title_name(t)),
        (None, None) => total,
    }
}

/// The toast after a public share (the buttons may be gone by then).
pub fn result_words(done: &Done) -> String {
    let mut words = vec![tr(SHARED_PUBLICLY).to_string()];
    words.extend(earned_words(done));
    if let Some(t) = done.reached.as_ref() {
        words.push(tr!("New title: {title}", title = title_name(t)));
    } else if let Some(after) = done.after.as_ref() {
        words.push(standing_words(after));
    }
    words.join(" · ")
}
