//! The avatar gun (contracts `Docs/avatar-gun.md`), the service's side: whether the worn avatar has the gun or a
//! receiver (VRChat's OSCQuery tree), the event stream to Orbiters (`/n4n0/avatar-gun/events`, opened again with each
//! instance), the shooter's `AvatarGun/Loaded`, its shots and desktop mode's trigger, and the hit player's hits, what
//! the settings make of them ([`decide`]), the change itself and its undo.
//!
//! The change is asked of VRChat's API (`PUT /avatars/{id}/select`) with the person's VRChat sign-in, and held as done
//! only once VRChat's own `/avatar/change` names the avatar within [`gun::CHANGE_WAIT_S`]; without a sign-in, or when
//! the running VRChat did not follow, OSC's `/avatar/change` is sent (VRChat follows it only for the player's own
//! favourites and uploads). Each step is a line of the log (English, not shown), so that a live test tells which way
//! worked. What the person is asked and told is a card ([`Card`]): in the window, before the eyes in VR, and in VRChat's
//! window in desktop mode (`crate::avatar_gun_ui`).
//!
//! OSC: what VRChat sends is only recorded on its receiving thread; every write goes from this module's own thread, one
//! edge of a parameter per turn at most.

use orbiters_core::app_log::OrLog as _;
use orbiters_core::locks::*;
use super::{Controller, NoticeKind, Shared};
use orbiters_contracts::avatar_gun::{self as gun, params, Event, Outcome};
use orbiters_core::account::Api;
use orbiters_core::osc::{OscValue, AVATAR_CHANGE, PARAMETERS_PREFIX};
use orbiters_core::settings::AvatarGunAllow;
use orbiters_i18n::{fill, mark, tr};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// How often the thread looks; while desktop mode's trigger is read, faster.
const TURN: Duration = Duration::from_millis(100);
const TRIGGER_TURN: Duration = Duration::from_millis(33);
/// A new avatar's parameters are in VRChat's tree this long after `/avatar/change`.
const SETTLE: Duration = Duration::from_millis(1500);
/// The tree is read again this often anyway (VRChat's OSCQuery found late, an avatar reset).
const LOOK_AGAIN: Duration = Duration::from_secs(15);
/// The event stream: given up after this long without a byte (Orbiters pings every 20 s), opened again after a pause
/// that doubles while it keeps failing.
const STREAM_IDLE: Duration = Duration::from_secs(60);
const STREAM_RETRY: Duration = Duration::from_secs(2);
const STREAM_RETRY_MOST: Duration = Duration::from_secs(60);
/// The longest line of the stream (an event is a few hundred bytes).
const EVENT_MOST: usize = 16 * 1024;
/// Requests to Orbiters give up after this.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the notices stay: after a change (its Undo with it), and when a change could not be made.
const CHANGED_FOR_MS: u64 = 20_000;
const FAILED_FOR_MS: u64 = 12_000;
/// The first change's notice, which also says the setting exists: longer, and at least this long after its hint is answered.
const FIRST_CHANGED_FOR_MS: u64 = 30_000;
const ANSWERED_FOR_MS: u64 = 6_000;

/// What the menus show of the avatar gun.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// The worn avatar has the gun (`AvatarGun/Fire` in its tree), a receiver (`AvatarGun/Hit`).
    pub gun: bool,
    pub receiver: bool,
    /// `AvatarGun/Loaded` is on: shots are reported.
    pub loaded: bool,
    /// The event stream to Orbiters is open.
    pub stream: bool,
    /// Bumped each time `AvatarGun/Choose` rose: the menus open the picker.
    pub choose: u64,
    /// The card up now.
    pub card: Option<Card>,
}

/// What is asked of the hit player, or told them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub id: u64,
    pub kind: CardKind,
    /// Who fired.
    pub from: String,
    /// The avatar: its id, name and picture.
    pub avatar_id: String,
    pub avatar: String,
    pub thumbnail: String,
    pub until_ms: u64,
    pub for_ms: u64,
    /// The first change ever: under the notice, that anyone's gun can do this and how to turn it off.
    #[serde(default)]
    pub hint: Hint,
}

/// What the first change's notice says under it (Enzo, 2026-11-24: the surprise is the default, and the first time says
/// it can be turned off). Only on [`CardKind::Changed`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hint {
    /// A later change: the notice and Undo alone.
    #[default]
    None,
    /// "Anyone with an avatar gun can do this to you": Turn off, Keep it on.
    Offered,
    /// Turn off was pressed: the setting is Nobody now.
    TurnedOff,
    /// Keep it on was pressed.
    KeptOn,
}

impl Card {
    pub fn up(&self, now_ms: u64) -> bool {
        now_ms < self.until_ms
    }

    /// Whether it shows its hint under the notice (the taller card).
    pub fn hinted(&self) -> bool {
        self.kind == CardKind::Changed && self.hint != Hint::None
    }

    /// The share of its time left.
    pub fn left(&self, now_ms: u64) -> f32 {
        (self.until_ms.saturating_sub(now_ms) as f32 / self.for_ms.max(1) as f32).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CardKind {
    /// "{name} wants to change your avatar to {avatar}": Allow, Not now.
    Ask,
    /// Allowed (or allowed by the settings): VRChat is being asked.
    Changing,
    /// "{name} changed your avatar": Undo.
    Changed,
    /// Undo pressed: going back.
    Undoing,
    /// "Could not change your avatar", and why.
    Failed(Failure),
}

/// Why a change was not made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Failure {
    /// VRChat says the avatar is not public (or could not say).
    NotPublic,
    /// VRChat did not switch: signed in to VRChat in Orbiters - N4N0 or not.
    DidNotFollow { signed_in: bool },
    /// Going back did not work.
    UndoFailed,
}

impl Failure {
    pub fn says(self) -> &'static str {
        tr(match self {
            Failure::NotPublic => mark("That avatar isn't public, so it can't be worn by you."),
            Failure::DidNotFollow { signed_in: true } => mark("VRChat didn't switch to it. VRChat may only follow avatars in your own favorites."),
            Failure::DidNotFollow { signed_in: false } => mark("VRChat only follows avatars in your favorites without your VRChat sign-in. Sign in to VRChat in Settings."),
            Failure::UndoFailed => mark("VRChat didn't switch back. Pick your avatar in VRChat's menu."),
        })
    }
}

/// The card's words.
pub const ASKS: &str = mark("{name} wants to change your avatar to {avatar}");
pub const CHANGED: &str = mark("{name} changed your avatar");
pub const CHANGING: &str = mark("Changing your avatar…");
pub const UNDOING: &str = mark("Changing back…");
pub const FAILED: &str = mark("Could not change your avatar");
pub const ALLOW: &str = mark("Allow");
pub const NOT_NOW: &str = mark("Not now");
pub const UNDO: &str = mark("Undo");
/// The first change's hint, its two answers, and what each answer says after.
pub const HINT: &str = mark("Anyone with an avatar gun can do this to you. You can turn it off.");
pub const TURN_OFF: &str = mark("Turn off");
pub const KEEP_ON: &str = mark("Keep it on");
pub const TURNED_OFF: &str = mark("Turned off. You can turn it back on in Settings.");
pub const KEPT_ON: &str = mark("Kept on. You can change it anytime in Settings.");

impl Card {
    /// Its line: what is asked or told, with who and which avatar.
    pub fn title(&self) -> String {
        match &self.kind {
            CardKind::Ask => fill(tr(ASKS), &[("name", &self.from), ("avatar", &self.avatar)]),
            CardKind::Changing => tr(CHANGING).to_string(),
            CardKind::Changed => fill(tr(CHANGED), &[("name", &self.from)]),
            CardKind::Undoing => tr(UNDOING).to_string(),
            CardKind::Failed(_) => tr(FAILED).to_string(),
        }
    }

    /// Its second line.
    pub fn detail(&self) -> String {
        match &self.kind {
            CardKind::Failed(why) => why.says().to_string(),
            CardKind::Ask | CardKind::Changed | CardKind::Changing => self.avatar.clone(),
            CardKind::Undoing => String::new(),
        }
    }

    /// The hint's words, while it has one.
    pub fn hint_says(&self) -> Option<&'static str> {
        if !self.hinted() {
            return None;
        }
        Some(tr(match self.hint {
            Hint::TurnedOff => TURNED_OFF,
            Hint::KeptOn => KEPT_ON,
            Hint::Offered | Hint::None => HINT,
        }))
    }

    /// Its buttons: the main one and the other (none while VRChat is asked).
    pub fn buttons(&self) -> (Option<&'static str>, Option<&'static str>) {
        match self.kind {
            CardKind::Ask => (Some(tr(ALLOW)), Some(tr(NOT_NOW))),
            CardKind::Changed => (Some(tr(UNDO)), None),
            CardKind::Failed(_) => (None, None),
            CardKind::Changing | CardKind::Undoing => (None, None),
        }
    }
}

/// A press on the card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Its main button (Allow, Undo).
    Main { card: u64 },
    /// Not now, or its cross.
    Close { card: u64 },
    /// The first change's hint answered: Keep it on (`keep`), or Turn off (the setting becomes Nobody).
    Hint { card: u64, keep: bool },
}

/// What a hit asks, as the settings, the friendship, the cooldown and the instance say ([`decide`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Change,
    Ask,
    Refuse(Refusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Nobody may change it.
    Nobody,
    /// Changed less than [`gun::COOLDOWN_S`] ago.
    Cooldown,
    /// The person is no longer in the instance the hit was for.
    Elsewhere,
    /// Another hit is being asked about or changed.
    Busy,
}

/// What to do with a hit: `allow` the setting, `friend` whether the shooter is a VRChat friend of the signed-in account
/// (verified by Orbiters), `since_change` how long ago the gun last changed this avatar, `here` whether the person is
/// still in the instance the stream was opened for, `busy` whether another hit is under way.
pub fn decide(allow: AvatarGunAllow, friend: bool, since_change: Option<Duration>, here: bool, busy: bool) -> Decision {
    if allow == AvatarGunAllow::Nobody {
        return Decision::Refuse(Refusal::Nobody);
    }
    if !here {
        return Decision::Refuse(Refusal::Elsewhere);
    }
    if busy {
        return Decision::Refuse(Refusal::Busy);
    }
    if since_change.is_some_and(|d| d < Duration::from_secs(gun::COOLDOWN_S)) {
        return Decision::Refuse(Refusal::Cooldown);
    }
    match allow {
        AvatarGunAllow::Anyone => Decision::Change,
        AvatarGunAllow::Friends if friend => Decision::Change,
        _ => Decision::Ask,
    }
}

/// What `AvatarGun/Loaded` should be: an avatar chosen, the gun worn, Orbiters signed in and the stream open.
pub fn loaded(chosen: bool, gun: bool, signed_in: bool, stream: bool) -> bool {
    chosen && gun && signed_in && stream
}

/// The shooter's toast for what came back of a shot.
pub fn shooter_says(outcome: Outcome, name: &str) -> String {
    let words = match outcome {
        Outcome::Changed => mark("{name}'s avatar changed"),
        Outcome::Asked => mark("{name} is being asked"),
        Outcome::Refused => mark("{name} said no"),
        Outcome::Undone => mark("{name} changed back"),
    };
    fill(tr(words), &[("name", &name)])
}

pub const CROSSED: &str = mark("Your shot crossed another one: nobody's avatar changed.");

/// Whom a stream was opened for and a hit came to: the credential, and the epoch of the identity it was read with
/// (controller/identity.rs). What is said of a hit later is said as them, whoever is signed in by then.
#[derive(Clone)]
struct Member {
    api: Api,
    token: String,
    epoch: u64,
}

/// The hit being asked about or changed, and what it needs later.
#[derive(Clone)]
struct Hit {
    shot: String,
    avatar: gun::Avatar,
    from: String,
    card: u64,
    /// The instance it was for, and the member it came to.
    instance: String,
    to: Member,
    /// Whether the shooter was a friend when asked, and whether the person pressed Allow.
    friend: bool,
    allowed: bool,
}

/// What the module keeps.
#[derive(Default)]
pub(super) struct Runtime {
    /// The latest of the gun's parameters as VRChat sent them, and their changes not acted on yet (the receiving thread's).
    values: std::collections::HashMap<&'static str, bool>,
    edges: Vec<(&'static str, bool)>,
    /// The avatar changed then: its tree is read once it settled.
    avatar_at: Option<Instant>,
    looked: Option<Instant>,
    gun: bool,
    receiver: bool,
    /// `AvatarGun/Loaded` and `AvatarGun/Pull` as last written, and when.
    loaded_sent: Option<(bool, Instant)>,
    pull_sent: Option<(bool, Instant)>,
    /// The stream: its number, credential, instance and identity's epoch; how many were opened; when to try again; failures in a row;
    /// whether it is connected now.
    streaming: Option<(u64, String, String, u64)>,
    streams: u64,
    retry_at: Option<Instant>,
    failures: u32,
    connected: bool,
    /// Shots reported: their number and when (what comes back names them).
    shots: Vec<(String, Instant)>,
    /// The hit under way, the card up, and the next card's number.
    hit: Option<Hit>,
    cards: u64,
    /// When the gun last changed this avatar; the avatar worn before it, by hand; the avatar it put on.
    changed_at: Option<Instant>,
    before: Option<String>,
    put_on: Option<String>,
    choose: u64,
    stop: bool,
    /// What tests' hits were answered with (no server is told).
    #[cfg(test)]
    answered: Vec<(String, Outcome, String)>,
}

fn now_ms() -> u64 {
    super::unix_ms()
}

/// `POST` of `body` to `path` with the credential: Orbiters' answer, or why not.
fn post(api: &Api, token: &str, path: &str, body: &impl Serialize) -> anyhow::Result<()> {
    let response = api
        .agent()
        .post(&api.url(path))
        .config()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build()
        .header("Authorization", &format!("Bearer {token}"))
        .send_json(body)?;
    let status = response.status().as_u16();
    anyhow::ensure!((200..300).contains(&status), "Orbiters answered {status}"); // i18n: not shown (a log's line)
    Ok(())
}

impl Controller {
    /// Starts the avatar gun's thread (it does nothing while no avatar has the gun or a receiver).
    pub(super) fn start_avatar_gun(self: &Shared) {
        if !orbiters_core::osc::VRCHAT_RUNS_HERE {
            return;
        }
        let ctl = self.clone();
        let rest = self.rest("avatar-gun", |s| (s.vrchat.running, s.avatar_gun.card.is_some()));
        std::thread::Builder::new().name("avatar-gun".into()).spawn(move || loop {
            if ctl.avatar_gun.locked().stop {
                return;
            }
            let fast = ctl.avatar_gun_turn();
            rest.wait(ctl.avatar_gun_rests(fast));
        }).or_log("a thread could not be started");
    }

    /// How long until the next turn: at once again while the trigger is read, a turn while the avatar has the gun or a receiver
    /// or anything of a shot is under way; else the next look at the avatar's tree (a parameter or another avatar wakes it), and
    /// nothing without VRChat.
    fn avatar_gun_rests(&self, fast: bool) -> Option<Duration> {
        let view = self.view();
        let rt = self.avatar_gun.locked();
        if fast {
            return Some(TRIGGER_TURN);
        }
        if !rt.edges.is_empty() || rt.streaming.is_some() || rt.hit.is_some() || view.avatar_gun.card.is_some() || view.vrchat.running && (rt.gun || rt.receiver) {
            return Some(TURN);
        }
        view.vrchat.running.then(|| match rt.avatar_at {
            Some(at) => super::rest::until(Some(at), SETTLE),
            None => super::rest::until(rt.looked, LOOK_AGAIN),
        }.max(TURN))
    }

    pub(super) fn stop_avatar_gun(&self) {
        let mut rt = self.avatar_gun.locked();
        rt.stop = true;
        rt.streaming = None;
        self.rests.wake("avatar-gun");
    }

    /// An avatar parameter VRChat sent, on the OSC receiving thread: only kept (the gun's thread acts on it).
    pub(super) fn avatar_gun_parameter(&self, name: &str, value: &OscValue) {
        let Some(&param) = params::ALL.iter().find(|p| **p == name) else { return };
        let on = value.truthy();
        let mut rt = self.avatar_gun.locked();
        if rt.values.insert(param, on) != Some(on) && rt.edges.len() < 64 {
            rt.edges.push((param, on));
            self.rests.wake("avatar-gun");
        }
    }

    /// VRChat loaded an avatar (the OSC receiving thread): its tree is read once it settled.
    pub(super) fn avatar_gun_avatar(&self, id: &str) {
        let mut rt = self.avatar_gun.locked();
        rt.avatar_at = Some(Instant::now());
        rt.values.clear();
        rt.edges.clear();
        rt.loaded_sent = None;
        rt.pull_sent = None;
        // Worn by hand (not the gun's own change): what Undo goes back to is forgotten.
        if rt.put_on.as_deref() != Some(id) {
            rt.before = None;
            rt.put_on = None;
        }
        self.rests.wake("avatar-gun");
    }

    /// A card's button.
    pub fn avatar_gun_command(self: &Shared, command: Command) {
        let (card, main) = match command {
            Command::Main { card } => (card, true),
            Command::Close { card } => (card, false),
            Command::Hint { card, keep } => return self.avatar_gun_hint(card, keep),
        };
        let shown = self.view().avatar_gun.card.clone().filter(|c| c.id == card);
        let Some(shown) = shown else { return };
        let hit = self.avatar_gun.locked().hit.clone().filter(|h| h.card == card);
        match (&shown.kind, main) {
            (CardKind::Ask, true) => {
                if let Some(hit) = hit {
                    log_info!("avatar gun: {} allowed by the person", hit.shot); // i18n: not shown (a log's line)
                    self.avatar_gun_change(Hit { allowed: true, ..hit });
                }
            }
            (CardKind::Ask, false) => {
                if let Some(hit) = hit {
                    log_warn!("avatar gun: {} refused by the person", hit.shot); // i18n: not shown (a log's line)
                    self.avatar_gun_answer(&hit.to, &hit.shot, Outcome::Refused);
                }
                self.avatar_gun_end(card);
            }
            (CardKind::Changed, true) => {
                if let Some(hit) = hit {
                    self.avatar_gun_undo(hit);
                }
            }
            _ => self.avatar_gun_end(card),
        }
    }

    /// The first change's hint answered on card `card`: Turn off makes the setting Nobody; either way the hint says where
    /// the setting is, and the notice (its Undo with it) stays a little longer.
    fn avatar_gun_hint(self: &Shared, card: u64, keep: bool) {
        if !self.view().avatar_gun.card.as_ref().is_some_and(|c| c.id == card && c.hinted() && c.hint == Hint::Offered) {
            return;
        }
        log_info!("avatar gun: first change's hint: {}", if keep { "kept on" } else { "turned off" }); // i18n: not shown (a log's line)
        if !keep {
            self.update_settings(|s| s.avatar_gun.allow = AvatarGunAllow::Nobody);
        }
        let now = now_ms();
        self.update_if(|s| {
            let Some(c) = s.avatar_gun.card.as_mut().filter(|c| c.id == card) else { return false };
            c.hint = if keep { Hint::KeptOn } else { Hint::TurnedOff };
            // Its time from when it came stays (it does not fade in again); only its end may come later.
            let until = c.until_ms.max(now + ANSWERED_FOR_MS);
            c.for_ms += until - c.until_ms;
            c.until_ms = until;
            true
        });
    }

    /// The hit of card `card` is over: another's stays.
    fn avatar_gun_release(&self, card: u64) {
        let mut rt = self.avatar_gun.locked();
        if rt.hit.as_ref().is_some_and(|h| h.card == card) {
            rt.hit = None;
        }
    }

    /// Whether the avatar is (still) `hit`'s to change: the hit under way, in the instance and for the account it came for, and
    /// allowed by the person or by the settings as they are now. Asked as each request is about to leave.
    fn avatar_gun_permitted(&self, hit: &Hit) -> bool {
        let (here, allow) = {
            let s = self.snapshot.locked();
            (s.vrchat.world.instance_hash().as_deref() == Some(hit.instance.as_str()), s.settings.avatar_gun.allow)
        };
        here && self.identity_epoch() == hit.to.epoch
            && (hit.allowed || decide(allow, hit.friend, None, true, false) == Decision::Change)
            && self.avatar_gun.locked().hit.as_ref().is_some_and(|h| h.card == hit.card)
    }

    /// The card `card` and its hit go.
    fn avatar_gun_end(&self, card: u64) {
        self.avatar_gun_release(card);
        self.update_if(|s| {
            let gone = s.avatar_gun.card.as_ref().is_some_and(|c| c.id == card);
            if gone {
                s.avatar_gun.card = None;
            }
            gone
        });
    }

    /// Shows `kind` on the card of `hit` (a new time for it).
    fn avatar_gun_card(&self, hit: &Hit, kind: CardKind, for_ms: u64) {
        self.avatar_gun_card_hinted(hit, kind, for_ms, Hint::None);
    }

    /// [`Self::avatar_gun_card`] with a hint under it.
    fn avatar_gun_card_hinted(&self, hit: &Hit, kind: CardKind, for_ms: u64, hint: Hint) {
        let card = Card {
            id: hit.card,
            kind,
            from: hit.from.clone(),
            avatar_id: hit.avatar.id.clone(),
            avatar: hit.avatar.name.clone(),
            thumbnail: hit.avatar.thumbnail.clone().unwrap_or_default(),
            until_ms: now_ms() + for_ms,
            for_ms,
            hint,
        };
        self.update(|s| s.avatar_gun.card = Some(card));
    }

    /// One turn of the gun's thread. True while desktop mode's trigger is read (the next turn comes sooner).
    fn avatar_gun_turn(self: &Shared) -> bool {
        let snap = self.view();
        let settings = &snap.settings.avatar_gun;
        let now = Instant::now();
        // The tree, once the avatar settled, and again now and then.
        let look = {
            let rt = self.avatar_gun.locked();
            match rt.avatar_at {
                Some(at) => now.duration_since(at) >= SETTLE,
                None => rt.looked.is_none_or(|t| now.duration_since(t) >= LOOK_AGAIN),
            }
        };
        if look {
            self.avatar_gun_look();
        }
        // The account and its identity, read as one: installed between the two, another is looked at the next turn.
        let epoch = self.identity_epoch();
        let account = self.account_api().filter(|_| self.identity_epoch() == epoch).map(|(api, token)| Member { api, token, epoch });
        let instance = snap.vrchat.world.instance_hash().filter(|_| snap.vrchat.running);
        let (gun_worn, receiver) = {
            let rt = self.avatar_gun.locked();
            (rt.gun, rt.receiver)
        };
        // The stream: while a gun is worn, or a receiver with hits allowed; opened again in another instance.
        let want = account.is_some() && instance.is_some() && (gun_worn || (receiver && settings.allow != AvatarGunAllow::Nobody));
        if let (true, Some(member), Some(instance)) = (want, account.as_ref(), instance.as_ref()) {
            self.avatar_gun_stream(member, instance);
        } else {
            let mut rt = self.avatar_gun.locked();
            if rt.streaming.take().is_some() {
                log_info!("avatar gun: event stream closed (not needed)"); // i18n: not shown (a log's line)
            }
            rt.connected = false;
        }
        // What VRChat said since the last turn.
        let edges = std::mem::take(&mut self.avatar_gun.locked().edges);
        let connected = self.avatar_gun.locked().connected;
        let ready = loaded(settings.loaded.is_some(), gun_worn, account.is_some(), connected);
        for (param, on) in edges {
            match (param, on) {
                (params::FIRE, true) if gun_worn => match (ready, account.as_ref(), instance.as_ref(), settings.loaded.as_ref()) {
                    (true, Some(member), Some(instance), Some(loaded)) => self.avatar_gun_shoot(&member.api, &member.token, instance, loaded),
                    _ => log_info!("avatar gun: Fire rose, not loaded: no shot reported"), // i18n: not shown (a log's line)
                },
                (params::CHOOSE, true) if gun_worn => {
                    let choose = {
                        let mut rt = self.avatar_gun.locked();
                        rt.choose += 1;
                        rt.choose
                    };
                    log_info!("avatar gun: Choose pressed: the picker opens"); // i18n: not shown (a log's line)
                    self.update(|s| s.avatar_gun.choose = choose);
                }
                (params::HIT, true) if receiver && settings.allow != AvatarGunAllow::Nobody => match (connected, account.as_ref(), instance.as_ref()) {
                    (true, Some(member), Some(instance)) => self.avatar_gun_report_hit(&member.api, &member.token, instance),
                    _ => log_info!("avatar gun: Hit rose, the stream is not open: not reported"), // i18n: not shown (a log's line)
                },
                _ => {}
            }
        }
        // AvatarGun/Loaded, again every LOADED_REFRESH_MS while the gun is worn.
        if gun_worn {
            let due = {
                let rt = self.avatar_gun.locked();
                rt.loaded_sent.is_none_or(|(was, at)| was != ready || now.duration_since(at) >= Duration::from_millis(gun::LOADED_REFRESH_MS))
            };
            if due && self.avatar_gun_send(params::LOADED, ready) {
                let mut rt = self.avatar_gun.locked();
                if rt.loaded_sent.is_none_or(|(was, _)| was != ready) {
                    log_info!("avatar gun: Loaded {ready}", ready = ready); // i18n: not shown (a log's line)
                }
                rt.loaded_sent = Some((ready, now));
            }
        }
        // Desktop mode's trigger: the left button while VRChat has the mouse (mouse-look) and the gun is out.
        let out = self.avatar_gun.locked().values.get(params::OUT).copied().unwrap_or(false);
        let reading = gun_worn && out && snap.n4n0.desktop_mode && snap.vrchat.pid.is_some();
        let pulled = reading && snap.vrchat.pid.is_some_and(|pid| {
            use orbiters_n4n0::Mouse;
            let m = crate::device::WindowsMouse.read(pid);
            m.down && m.front && !m.cursor
        });
        let pull_due = {
            let rt = self.avatar_gun.locked();
            // One edge a turn, never two in one frame: a change waits for the turn after the last one.
            match rt.pull_sent {
                Some((was, at)) => was != pulled && now.duration_since(at) >= TRIGGER_TURN,
                None => pulled,
            }
        };
        if pull_due && self.avatar_gun_send(params::PULL, pulled) {
            self.avatar_gun.locked().pull_sent = Some((pulled, now));
        }
        // Shots that paired with nothing: forgotten.
        {
            let mut rt = self.avatar_gun.locked();
            rt.shots.retain(|(_, at)| now.duration_since(*at) < Duration::from_secs(60));
        }
        // The card's time.
        let ms = now_ms();
        if let Some(card) = snap.avatar_gun.card.clone().filter(|c| !c.up(ms)) {
            if card.kind == CardKind::Ask {
                let hit = self.avatar_gun.locked().hit.clone().filter(|h| h.card == card.id);
                if let Some(hit) = hit {
                    log_warn!("avatar gun: {} not answered in time: refused", hit.shot); // i18n: not shown (a log's line)
                    self.avatar_gun_answer(&hit.to, &hit.shot, Outcome::Refused);
                }
            }
            if !matches!(card.kind, CardKind::Changing | CardKind::Undoing) {
                self.avatar_gun_end(card.id);
            }
        }
        // What the menus show.
        let shown = (gun_worn, receiver, ready && gun_worn, connected);
        self.update_if(|s| {
            let a = &mut s.avatar_gun;
            let changed = (a.gun, a.receiver, a.loaded, a.stream) != shown;
            (a.gun, a.receiver, a.loaded, a.stream) = shown;
            changed
        });
        reading
    }

    /// Reads the worn avatar's tree: whether it has the gun, a receiver. A tree with none of its parameters was not read
    /// (VRChat's OSCQuery not found yet): what was known stays, and it is read again soon.
    fn avatar_gun_look(&self) {
        let values = self.with_osc(|o| o.vrchat_query()).flatten().and_then(|addr| orbiters_core::osc::vrchat_values(addr, Duration::from_millis(800)).ok());
        let mut rt = self.avatar_gun.locked();
        rt.looked = Some(Instant::now());
        let Some(values) = values.filter(|v| v.iter().any(|(a, _)| a.starts_with(PARAMETERS_PREFIX))) else {
            // Asked again in a moment rather than at the next long look.
            if rt.avatar_at.is_some() {
                rt.avatar_at = Some(Instant::now() - SETTLE + Duration::from_secs(2));
            }
            return;
        };
        rt.avatar_at = None;
        let has = |name: &str| values.iter().any(|(a, _)| a.strip_prefix(PARAMETERS_PREFIX) == Some(name));
        let (gun_worn, receiver) = (has(params::FIRE), has(params::HIT));
        if (gun_worn, receiver) != (rt.gun, rt.receiver) {
            log_info!("avatar gun: the avatar has the gun {gun_worn}, a receiver {receiver}", gun_worn = gun_worn, receiver = receiver); // i18n: not shown (a log's line)
        }
        (rt.gun, rt.receiver) = (gun_worn, receiver);
        for (address, value) in &values {
            if let Some(&p) = address.strip_prefix(PARAMETERS_PREFIX).and_then(|n| params::ALL.iter().find(|p| **p == n)) {
                rt.values.insert(p, value.truthy());
            }
        }
    }

    /// Writes an avatar parameter of the gun: whether it went.
    fn avatar_gun_send(&self, param: &str, on: bool) -> bool {
        matches!(self.with_osc(|o| o.send(&format!("{PARAMETERS_PREFIX}{param}"), vec![rosc::OscType::Bool(on)])), Some(Ok(())))
    }

    /// Keeps the stream of `member` open for `instance`: opened when it is not, opened anew in another instance.
    fn avatar_gun_stream(self: &Shared, member: &Member, instance: &str) {
        let number = {
            let mut rt = self.avatar_gun.locked();
            if rt.streaming.as_ref().is_some_and(|(_, t, i, e)| *t != member.token || i != instance || *e != member.epoch) {
                log_info!("avatar gun: another instance or account: the event stream is opened again"); // i18n: not shown (a log's line)
                rt.streaming = None;
                rt.connected = false;
                rt.retry_at = None;
                rt.failures = 0;
            }
            if rt.streaming.is_some() || rt.retry_at.is_some_and(|t| Instant::now() < t) {
                return;
            }
            rt.streams += 1;
            let number = rt.streams;
            rt.streaming = Some((number, member.token.clone(), instance.to_string(), member.epoch));
            number
        };
        let ctl = self.clone();
        let (member, instance) = (member.clone(), instance.to_string());
        std::thread::Builder::new().name("avatar-gun-events".into()).spawn(move || {
            let url = member.api.url(&format!("{}?instance={instance}", gun::routes::EVENTS));
            log_info!("avatar gun: event stream opening"); // i18n: not shown (a log's line)
            let still = || {
                let mut rt = ctl.avatar_gun.locked();
                let current = !rt.stop && rt.streaming.as_ref().is_some_and(|(n, ..)| *n == number);
                if current {
                    rt.connected = true;
                }
                current
            };
            let result = super::event_stream::follow(&url, &member.token, STREAM_IDLE, EVENT_MOST, still, |event, data| {
                match Event::parse(event, data) {
                    Some(e) => ctl.avatar_gun_event(e, &member, &instance),
                    None => log_warn!("avatar gun: event {event} not read", event = event), // i18n: not shown (a log's line)
                }
                true
            });
            if let Err(e) = &result {
                log_warn!("avatar gun events: {e:#}", e = e); // i18n: not shown (a log's line)
            }
            ctl.avatar_gun_stream_ended(number, matches!(result, Ok(true)));
        }).or_log("a thread could not be started");
    }

    /// The stream `number` ended, `well` or not: when the next one is tried, unless another took its place meanwhile (its
    /// waits are its own).
    fn avatar_gun_stream_ended(&self, number: u64, well: bool) {
        let mut rt = self.avatar_gun.locked();
        if !rt.streaming.as_ref().is_some_and(|(n, ..)| *n == number) {
            return;
        }
        rt.streaming = None;
        rt.connected = false;
        rt.failures = if well { 0 } else { rt.failures + 1 };
        let pause = if well { STREAM_RETRY } else { STREAM_RETRY.saturating_mul(1 << (rt.failures - 1).min(5)).min(STREAM_RETRY_MOST) };
        rt.retry_at = Some(Instant::now() + pause);
    }

    /// `AvatarGun/Fire` rose with the gun loaded: the shot goes to Orbiters.
    fn avatar_gun_shoot(&self, api: &Api, token: &str, instance: &str, loaded: &orbiters_core::settings::LoadedAvatar) {
        let Some(avatar) = gun::Avatar::for_shot(&loaded.id, &loaded.name, Some(&loaded.thumbnail)) else {
            log_info!("avatar gun: the loaded avatar is not an avatar's id: no shot"); // i18n: not shown (a log's line)
            return;
        };
        let shot = uuid::Uuid::new_v4().to_string();
        self.avatar_gun.locked().shots.push((shot.clone(), Instant::now()));
        let report = gun::ShotReport { instance: instance.to_string(), shot: shot.clone(), avatar };
        let (api, token) = (api.clone(), token.to_string());
        std::thread::spawn(move || match post(&api, &token, gun::routes::SHOTS, &report) {
            Ok(()) => log_info!("avatar gun: shot {shot} reported", shot = shot), // i18n: not shown (a log's line)
            Err(e) => log_warn!("avatar gun: shot {shot} not reported: {e:#}", shot = shot, e = e), // i18n: not shown (a log's line)
        });
    }

    /// `AvatarGun/Hit` rose on the receiver: the hit goes to Orbiters.
    fn avatar_gun_report_hit(&self, api: &Api, token: &str, instance: &str) {
        let report = gun::HitReport { instance: instance.to_string() };
        let (api, token) = (api.clone(), token.to_string());
        std::thread::spawn(move || match post(&api, &token, gun::routes::HITS, &report) {
            Ok(()) => log_info!("avatar gun: hit reported"), // i18n: not shown (a log's line)
            Err(e) => log_warn!("avatar gun: hit not reported: {e:#}", e = e), // i18n: not shown (a log's line)
        });
    }

    /// What the hit player's app did with `shot`, to Orbiters as the member it came to (the shooter is told).
    fn avatar_gun_answer(&self, to: &Member, shot: &str, result: Outcome) {
        #[cfg(test)]
        self.avatar_gun.lock().unwrap().answered.push((shot.to_string(), result, to.token.clone()));
        if cfg!(test) {
            return;
        }
        let (to, report) = (to.clone(), gun::AnswerReport { shot: shot.to_string(), result });
        std::thread::spawn(move || {
            if let Err(e) = post(&to.api, &to.token, gun::routes::ANSWERS, &report) {
                log_warn!("avatar gun: answer {:?} for {} not sent: {e:#}", report.result, report.shot, e = e); // i18n: not shown (a log's line)
            }
        });
    }

    /// One event of the stream opened for `instance`.
    fn avatar_gun_event(self: &Shared, event: Event, to: &Member, instance: &str) {
        match event {
            Event::Hit(hit) => self.avatar_gun_hit(hit, to, instance),
            Event::Landed(landed) => {
                // What the hit player then does comes as an answer: nothing to say yet.
                log_info!("avatar gun: shot {} landed on {}", landed.shot, landed.name); // i18n: not shown (a log's line)
            }
            Event::Answer(answer) => {
                log_info!("avatar gun: shot {} answered {:?}", answer.shot, answer.result); // i18n: not shown (a log's line)
                if self.avatar_gun.locked().shots.iter().any(|(s, _)| *s == answer.shot) {
                    let kind = if answer.result == Outcome::Changed { NoticeKind::Success } else { NoticeKind::Info };
                    self.notify(kind, shooter_says(answer.result, &answer.name));
                }
            }
            Event::Crossed(crossed) => {
                log_info!("avatar gun: shot {} crossed another", crossed.shot); // i18n: not shown (a log's line)
                if self.avatar_gun.locked().shots.iter().any(|(s, _)| *s == crossed.shot) {
                    self.notify(NoticeKind::Info, tr(CROSSED));
                }
            }
        }
    }

    /// A hit of another player's shot: what the settings say of it ([`decide`]).
    fn avatar_gun_hit(self: &Shared, hit: gun::Hit, to: &Member, instance: &str) {
        if !gun::is_avatar_id(&hit.avatar.id) {
            log_warn!("avatar gun: hit {} names no avatar: refused", hit.shot); // i18n: not shown (a log's line)
            self.avatar_gun_answer(to, &hit.shot, Outcome::Refused);
            return;
        }
        let ctl = self.clone();
        let (to, instance) = (to.clone(), instance.to_string());
        // A friend's (a verified VRChat account) is looked up off the stream's thread.
        std::thread::spawn(move || {
            let friend = match (&hit.from.vrchat_id, ctl.view().settings.avatar_gun.allow) {
                (Some(id), AvatarGunAllow::Friends) => ctl.vrchat_source().friends().map(|f| f.iter().any(|u| u.id == *id)).unwrap_or(false),
                _ => false,
            };
            // As it is once the shooter was looked up: the room left, or the setting tightened, meanwhile counts.
            let snap = ctl.view();
            let allow = snap.settings.avatar_gun.allow;
            let here = snap.vrchat.world.instance_hash().as_deref() == Some(instance.as_str()) && ctl.identity_epoch() == to.epoch;
            let name = hit.from.name.chars().take(64).collect::<String>();
            let avatar = gun::Avatar { name: hit.avatar.name.chars().take(gun::NAME_MOST).collect(), ..hit.avatar.clone() };
            // Decided and taken in one step: of two hits at once, one is.
            let (decision, pending) = {
                let mut rt = ctl.avatar_gun.locked();
                let decision = decide(allow, friend, rt.changed_at.map(|t| t.elapsed()), here, rt.hit.is_some());
                rt.cards += 1;
                let pending = Hit { shot: hit.shot.clone(), avatar, from: name, card: rt.cards, instance: instance.clone(), to: to.clone(), friend, allowed: false };
                if !matches!(decision, Decision::Refuse(_)) {
                    rt.hit = Some(pending.clone());
                }
                (decision, pending)
            };
            log_info!("avatar gun: hit {} by {} ({}), {:?} in {instance}: {decision:?}", hit.shot, hit.from.name, if friend { "a friend" } else { "not a friend" }, allow, instance = instance, decision = decision); // i18n: not shown (a log's line)
            match decision {
                Decision::Refuse(_) => ctl.avatar_gun_answer(&to, &hit.shot, Outcome::Refused),
                Decision::Ask => {
                    ctl.avatar_gun_card(&pending, CardKind::Ask, gun::ASK_S * 1000);
                    ctl.avatar_gun_answer(&to, &hit.shot, Outcome::Asked);
                }
                Decision::Change => ctl.avatar_gun_change(pending),
            }
        });
    }

    /// Changes the avatar to the hit's (allowed by the person or the settings), on a thread of its own: VRChat says it is
    /// public, the instance is still the one, then VRChat is asked; the card says how it went, and Orbiters is told.
    fn avatar_gun_change(self: &Shared, hit: Hit) {
        self.avatar_gun_card(&hit, CardKind::Changing, gun::CHANGE_WAIT_S * 2000 + 4000);
        let ctl = self.clone();
        std::thread::spawn(move || {
            let source = ctl.vrchat_source();
            let signed_in = source.direct_available();
            // Public, as VRChat says it (with the sign-in); without it, Orbiters' fallback must know it.
            let public = match source.avatar(&hit.avatar.id) {
                Ok(Some(a)) if signed_in => a.public(),
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(e) => {
                    log_warn!("avatar gun: {} could not be read: {e:#}", hit.avatar.id, e = e); // i18n: not shown (a log's line)
                    false
                }
            };
            log_info!("avatar gun: {} is public: {public} (signed in to VRChat: {signed_in})", hit.avatar.id, public = public, signed_in = signed_in); // i18n: not shown (a log's line)
            if !public {
                ctl.avatar_gun_card(&hit, CardKind::Failed(Failure::NotPublic), FAILED_FOR_MS);
                ctl.avatar_gun_answer(&hit.to, &hit.shot, Outcome::Refused);
                ctl.avatar_gun_release(hit.card);
                return;
            }
            let worn = ctl.avatar_gun_worn();
            if ctl.avatar_gun_wear(&hit.avatar.id, || ctl.avatar_gun_permitted(&hit)) {
                {
                    let mut rt = ctl.avatar_gun.locked();
                    // The avatar worn before, by hand: kept until the next change by hand, through several shots.
                    if rt.before.is_none() || rt.put_on.as_deref() != worn.as_deref() {
                        rt.before = worn;
                    }
                    rt.put_on = Some(hit.avatar.id.clone());
                    rt.changed_at = Some(Instant::now());
                }
                // The first change ever also says that anyone's gun can do this, and how to turn it off; Settings shows
                // the setting from now on.
                let first = !ctl.view().settings.avatar_gun.changed_once;
                if first {
                    ctl.update_settings(|s| s.avatar_gun.changed_once = true);
                    ctl.avatar_gun_card_hinted(&hit, CardKind::Changed, FIRST_CHANGED_FOR_MS, Hint::Offered);
                } else {
                    ctl.avatar_gun_card(&hit, CardKind::Changed, CHANGED_FOR_MS);
                }
                ctl.avatar_gun_answer(&hit.to, &hit.shot, Outcome::Changed);
            } else {
                if ctl.avatar_gun_permitted(&hit) {
                    ctl.avatar_gun_card(&hit, CardKind::Failed(Failure::DidNotFollow { signed_in }), FAILED_FOR_MS);
                    ctl.avatar_gun_release(hit.card);
                } else {
                    log_info!("avatar gun: {} is not allowed any more (the instance left, the setting changed): stopped", hit.shot); // i18n: not shown (a log's line)
                    ctl.avatar_gun_end(hit.card);
                }
                ctl.avatar_gun_answer(&hit.to, &hit.shot, Outcome::Refused);
            }
        });
    }

    /// Undo: back to the avatar worn before, by the same means; Orbiters tells the shooter.
    fn avatar_gun_undo(self: &Shared, hit: Hit) {
        let Some(before) = self.avatar_gun.locked().before.clone() else {
            log_info!("avatar gun: Undo, but the avatar worn before is not known"); // i18n: not shown (a log's line)
            self.avatar_gun_card(&hit, CardKind::Failed(Failure::UndoFailed), FAILED_FOR_MS);
            return;
        };
        self.avatar_gun_card(&hit, CardKind::Undoing, gun::CHANGE_WAIT_S * 2000 + 4000);
        let ctl = self.clone();
        std::thread::spawn(move || {
            log_info!("avatar gun: Undo of {}: back to {before}", hit.shot, before = before); // i18n: not shown (a log's line)
            // The person's own, whatever the settings say.
            if ctl.avatar_gun_wear(&before, || true) {
                {
                    let mut rt = ctl.avatar_gun.locked();
                    (rt.before, rt.put_on) = (None, None);
                }
                ctl.avatar_gun_end(hit.card);
                if let Err(e) = post(&hit.to.api, &hit.to.token, gun::routes::UNDONE, &gun::UndoneReport { shot: hit.shot.clone() }) {
                    log_warn!("avatar gun: undone of {} not sent: {e:#}", hit.shot, e = e); // i18n: not shown (a log's line)
                }
            } else {
                ctl.avatar_gun_card(&hit, CardKind::Failed(Failure::UndoFailed), FAILED_FOR_MS);
                ctl.avatar_gun_release(hit.card);
            }
        });
    }

    /// The avatar VRChat says is worn.
    fn avatar_gun_worn(&self) -> Option<String> {
        match self.osc_values.locked().latest.get(AVATAR_CHANGE).and_then(|(v, _)| v.first().cloned()) {
            Some(OscValue::Text(id)) if !id.is_empty() => Some(id),
            _ => self.view().n4n0.avatar_id.clone(),
        }
    }

    /// Whether VRChat said `/avatar/change` with `id` since `since_ns`.
    fn avatar_gun_echo(&self, id: &str, since_ns: u64) -> bool {
        self.osc_values.locked().latest.get(AVATAR_CHANGE).is_some_and(|(v, at)| *at >= since_ns && matches!(v.first(), Some(OscValue::Text(a)) if a == id))
    }

    /// Waits up to [`gun::CHANGE_WAIT_S`] for VRChat to say it wears `id`.
    fn avatar_gun_wait(&self, id: &str, since_ns: u64) -> bool {
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(gun::CHANGE_WAIT_S) {
            if self.avatar_gun_echo(id, since_ns) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    /// Puts `id` on: VRChat's API with the sign-in, held as done once VRChat names it; else, or when VRChat did not follow,
    /// OSC's `/avatar/change`, waited for the same way; each only if `permitted` says so as it is about to leave. Each step a line
    /// of the log.
    fn avatar_gun_wear(&self, id: &str, permitted: impl Fn() -> bool) -> bool {
        let source = self.vrchat_source();
        if source.direct_available() {
            let asked = orbiters_core::clock::now_ns();
            match source.select_avatar_while(id, &permitted) {
                Ok(()) => {
                    log_info!("avatar gun: {id} selected through VRChat's API; waiting for VRChat", id = id); // i18n: not shown (a log's line)
                    if self.avatar_gun_wait(id, asked) {
                        log_info!("avatar gun: {id} worn: VRChat followed its API", id = id); // i18n: not shown (a log's line)
                        return true;
                    }
                    log_warn!("avatar gun: VRChat did not follow its API within {} s", gun::CHANGE_WAIT_S); // i18n: not shown (a log's line)
                }
                Err(e) => log_warn!("avatar gun: VRChat's API refused the selection: {e:#}", e = e), // i18n: not shown (a log's line)
            }
        } else {
            log_info!("avatar gun: not signed in to VRChat here: OSC only"); // i18n: not shown (a log's line)
        }
        if !permitted() {
            return false;
        }
        let asked = orbiters_core::clock::now_ns();
        match self.with_osc(|o| o.send(AVATAR_CHANGE, vec![rosc::OscType::String(id.to_string())])) {
            Some(Ok(())) => log_info!("avatar gun: /avatar/change {id} sent over OSC; waiting for VRChat", id = id), // i18n: not shown (a log's line)
            Some(Err(e)) => {
                log_warn!("avatar gun: OSC /avatar/change failed: {e:#}", e = e); // i18n: not shown (a log's line)
                return false;
            }
            None => {
                log_info!("avatar gun: OSC is off: nothing sent"); // i18n: not shown (a log's line)
                return false;
            }
        }
        let worn = self.avatar_gun_wait(id, asked);
        log_warn!("avatar gun: {id} {}", if worn { "worn: VRChat followed OSC" } else { "not worn: VRChat did not follow OSC" }, id = id); // i18n: not shown (a log's line)
        worn
    }
}

#[cfg(test)]
#[path = "avatar_gun_tests.rs"]
mod permits;

#[cfg(test)]
mod tests {
    use super::*;
    use AvatarGunAllow::*;

    const LONG_AGO: Option<Duration> = Some(Duration::from_secs(gun::COOLDOWN_S + 1));

    /// The contract's setting, case by case: Nobody refuses all; Ask first asks everyone; Friends changes at once for a
    /// friend and asks anyone else; Anyone changes at once.
    #[test]
    fn the_setting_decides_what_a_hit_does() {
        for friend in [false, true] {
            assert_eq!(decide(Nobody, friend, None, true, false), Decision::Refuse(Refusal::Nobody));
            assert_eq!(decide(AskFirst, friend, None, true, false), Decision::Ask);
            assert_eq!(decide(Anyone, friend, LONG_AGO, true, false), Decision::Change);
        }
        assert_eq!(decide(Friends, true, None, true, false), Decision::Change);
        assert_eq!(decide(Friends, false, None, true, false), Decision::Ask);
    }

    /// At most one change every 30 s, only in the instance the hit was for, one hit at a time, whatever the setting.
    #[test]
    fn cooldown_instance_and_a_hit_under_way_refuse() {
        for allow in [AskFirst, Friends, Anyone] {
            assert_eq!(decide(allow, true, Some(Duration::from_secs(5)), true, false), Decision::Refuse(Refusal::Cooldown), "{allow:?}");
            assert_eq!(decide(allow, true, Some(Duration::from_secs(29)), true, false), Decision::Refuse(Refusal::Cooldown), "{allow:?}");
            assert_ne!(decide(allow, true, LONG_AGO, true, false), Decision::Refuse(Refusal::Cooldown), "{allow:?}");
            assert_eq!(decide(allow, true, None, false, false), Decision::Refuse(Refusal::Elsewhere), "{allow:?}");
            assert_eq!(decide(allow, true, None, true, true), Decision::Refuse(Refusal::Busy), "{allow:?}");
        }
        assert_eq!(decide(Nobody, true, Some(Duration::ZERO), false, true), Decision::Refuse(Refusal::Nobody), "Nobody says no first");
    }

    #[test]
    fn loaded_needs_an_avatar_the_gun_an_account_and_the_stream() {
        assert!(loaded(true, true, true, true));
        for i in 0..4 {
            let mut v = [true; 4];
            v[i] = false;
            assert!(!loaded(v[0], v[1], v[2], v[3]), "{v:?}");
        }
    }

    #[test]
    fn its_words_are_in_every_language() {
        let all = [ASKS, CHANGED, CHANGING, UNDOING, FAILED, ALLOW, NOT_NOW, UNDO, CROSSED, HINT, TURN_OFF, KEEP_ON, TURNED_OFF, KEPT_ON];
        for language in orbiters_i18n::Language::TRANSLATED {
            for words in all {
                let said = orbiters_i18n::in_language(language, || tr(words).to_string());
                assert!(said != words, "{language:?}: {words}");
            }
            orbiters_i18n::in_language(language, || {
                for outcome in [Outcome::Changed, Outcome::Asked, Outcome::Refused, Outcome::Undone] {
                    assert!(shooter_says(outcome, "Sam").contains("Sam"), "{language:?}: {outcome:?}");
                }
                for f in [Failure::NotPublic, Failure::DidNotFollow { signed_in: true }, Failure::DidNotFollow { signed_in: false }, Failure::UndoFailed] {
                    assert!(!f.says().is_empty());
                }
            });
        }
    }

    /// The reports go as the contract writes them (against a stand-in of Orbiters on this computer): the route, the
    /// credential, the JSON; 202 is taken, a refusal is an error.
    #[test]
    fn reports_reach_the_route_with_the_credential() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = Api::at(&format!("http://{}", listener.local_addr().unwrap()));
        let server = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for status in ["202 Accepted", "429 Too Many Requests"] {
                let (mut s, _) = listener.accept().unwrap();
                let mut data = vec![0; 8192];
                let mut n = 0;
                // Headers, then as much body as they say.
                let whole = |d: &[u8]| -> bool {
                    let text = String::from_utf8_lossy(d).to_string();
                    let Some(h) = text.find("\r\n\r\n") else { return false };
                    let length = text[..h].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                    d.len() >= h + 4 + length
                };
                while n < data.len() && !whole(&data[..n]) {
                    let got = s.read(&mut data[n..]).unwrap();
                    if got == 0 {
                        break;
                    }
                    n += got;
                }
                seen.push(String::from_utf8_lossy(&data[..n]).to_string());
                write!(s, "HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
            }
            seen
        });
        let avatar = gun::Avatar::for_shot("avtr_c38a1615-5bf5-42b4-84eb-a8b6c37cbd11", "Robot", None).unwrap();
        let shot = gun::ShotReport { instance: "ab12".into(), shot: "s-1".into(), avatar };
        assert!(post(&api, "orbit-test", gun::routes::SHOTS, &shot).is_ok());
        assert!(post(&api, "orbit-test", gun::routes::HITS, &gun::HitReport { instance: "ab12".into() }).is_err(), "a 429 is not taken");
        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("POST /n4n0/avatar-gun/shots "), "{}", seen[0]);
        assert!(seen[0].to_ascii_lowercase().contains("authorization: bearer orbit-test"));
        let body: serde_json::Value = serde_json::from_str(seen[0].split_once("\r\n\r\n").map_or("", |(_, b)| b)).unwrap();
        assert_eq!(body, serde_json::json!({"instance": "ab12", "shot": "s-1", "avatar": {"id": "avtr_c38a1615-5bf5-42b4-84eb-a8b6c37cbd11", "name": "Robot"}}));
        assert!(seen[1].starts_with("POST /n4n0/avatar-gun/hits "), "{}", seen[1]);
    }

    /// The card says who and what: the question with both names, the notice with the shooter's.
    #[test]
    fn the_card_says_who_and_what() {
        let card = Card { id: 1, kind: CardKind::Ask, from: "Kite".into(), avatar_id: "avtr_x".into(), avatar: "Robot".into(), thumbnail: String::new(), until_ms: 20_000, for_ms: 20_000, hint: Hint::None };
        orbiters_i18n::in_language(orbiters_i18n::Language::En, || {
            assert_eq!(card.title(), "Kite wants to change your avatar to Robot");
            assert_eq!(card.buttons(), (Some("Allow"), Some("Not now")));
            let changed = Card { kind: CardKind::Changed, ..card.clone() };
            assert_eq!(changed.title(), "Kite changed your avatar");
            assert_eq!(changed.buttons(), (Some("Undo"), None));
            assert_eq!(Card { kind: CardKind::Changing, ..card.clone() }.buttons(), (None, None));
        });
        assert!((card.left(10_000) - 0.5).abs() < 1e-6 && card.up(19_999) && !card.up(20_000));
    }

    /// The first change's hint: only on the notice of a change, its words following its answer; a later change has none.
    #[test]
    fn the_first_change_says_it_can_be_turned_off() {
        let changed = Card { id: 1, kind: CardKind::Changed, from: "Kite".into(), avatar_id: "avtr_x".into(), avatar: "Robot".into(), thumbnail: String::new(), until_ms: 20_000, for_ms: 20_000, hint: Hint::None };
        orbiters_i18n::in_language(orbiters_i18n::Language::En, || {
            assert!(!changed.hinted() && changed.hint_says().is_none(), "a later change: the notice alone");
            let first = Card { hint: Hint::Offered, ..changed.clone() };
            assert!(first.hinted());
            assert_eq!(first.hint_says(), Some("Anyone with an avatar gun can do this to you. You can turn it off."));
            assert_eq!(first.buttons(), (Some("Undo"), None), "Undo stays");
            assert_eq!(Card { hint: Hint::TurnedOff, ..changed.clone() }.hint_says(), Some(TURNED_OFF));
            assert_eq!(Card { hint: Hint::KeptOn, ..changed.clone() }.hint_says(), Some(KEPT_ON));
            assert!(!Card { kind: CardKind::Undoing, hint: Hint::Offered, ..changed.clone() }.hinted(), "going back shows no hint");
        });
    }
}
