//! Where's destination: one value ([`Selection`]) with one owner ([`Desk`]) (verify2-r21 M3 and M4: it was several fields
//! patched by late answers). What Play goes to, what is known of it, which card, sheet or friend it came from, and the
//! generation of that choice are replaced together, by [`Desk::replace`] alone; nothing outside this file writes any part
//! of it.
//!
//! Whatever is asked elsewhere (whether an instance is still open, VRChat making a new one) carries the generation it was
//! asked under and the account and profile it was asked for ([`Ticket`], [`MakeTicket`]), comes back through one channel
//! that keeps every answer ([`Desk::take`]), and counts only while all three are still the current ones. A worker that
//! ends without an answer answers "not known": nothing waits for it for ever.
//!
//! Play is a function of the selection ([`Desk::decide`]): VRChat starts in a previous or a friend's instance only from an
//! [`Verified::Open`] of the last minute, for the account and the profile chosen now. "Busy" is not kept anywhere: it is
//! a check (or a new instance being made) of the current generation that Play waits for ([`Desk::busy`]).

use super::previous::Visit;
use orbiters_core::launcher::LaunchMode;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long VRChat's "still open" stands before Play asks it again (verify-r21 Major 4): VRChat's own client keeps an
/// instance's answer about a minute, so asking more often learns nothing; an instance chosen longer ago than that is
/// asked again at Play, and VRChat starts only if it still says open.
pub const OPEN_FOR: Duration = Duration::from_secs(60);
/// A check with no answer after this is "not known" (the service answers or fails well before).
pub const CHECK_FOR: Duration = Duration::from_secs(30);
/// A press of Play older than this starts nothing by itself when the answer comes.
pub const PLAY_WAIT: Duration = Duration::from_secs(30);
/// Two presses of Play this close (seconds of the window's clock) are one.
const ONE_PRESS: f64 = 0.4;

/// Who a thing is asked for: the VRChat account verified as signed in (none: nobody, or another account's profile is
/// chosen) and the profile Play would start (none on a Mac).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asker {
    pub account: Option<String>,
    pub profile: Option<u32>,
}

/// What Play goes to.
#[derive(Clone, Debug, PartialEq)]
pub enum Destination {
    /// VRChat's own start.
    Home,
    /// One of Where's cards that needs no check of its own (an invite, an event, a place visited often): where it goes
    /// is the card's, as it is now; `instance` when it went to an instance that was there.
    Card { key: String, name: String, instance: bool },
    /// This very instance (a previous one, the last one, a friend's): only once VRChat says it is open.
    Instance { key: String, visit: Visit },
    /// A new instance of that one's world.
    NewOf { key: String, visit: Visit },
}

impl Destination {
    /// The card it is, none for Home.
    pub fn key(&self) -> Option<&str> {
        match self {
            Self::Home => None,
            Self::Card { key, .. } | Self::Instance { key, .. } | Self::NewOf { key, .. } => Some(key),
        }
    }

    pub fn visit(&self) -> Option<&Visit> {
        match self {
            Self::Instance { visit, .. } | Self::NewOf { visit, .. } => Some(visit),
            _ => None,
        }
    }
}

/// Where a selection came from.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    Home,
    Card,
    /// The previous instances' sheet (or the card it left).
    Previous,
    /// The last instance's card.
    Recent,
    /// Select on a friend (REQUEST23): who, and the world's picture as their row shows it.
    Friend { id: String, name: String, picture: Option<String> },
}

/// What is known of a [`Destination::Instance`] (of nothing else: the rest needs no check).
#[derive(Clone, Debug, PartialEq)]
pub enum Verified {
    /// Not asked: who is signed in is not known yet, or nobody can be asked for (signed out, another account's profile).
    Unverified,
    /// Being asked, under this generation.
    Checking { generation: u64, since: Instant },
    /// VRChat said it is open, then, for that account and profile.
    Open { at: Instant, by: Asker },
    /// Closed (or gone).
    Closed,
    /// VRChat did not say.
    Unknown,
}

/// The one value. Made and replaced in this file only.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    destination: Destination,
    origin: Origin,
    verified: Verified,
    generation: u64,
}

impl Selection {
    pub fn destination(&self) -> &Destination {
        &self.destination
    }

    pub fn origin(&self) -> &Origin {
        &self.origin
    }

    pub fn verified(&self) -> &Verified {
        &self.verified
    }

    /// The instance it is, when that is what it goes to.
    pub fn instance(&self) -> Option<&Visit> {
        match &self.destination {
            Destination::Instance { visit, .. } => Some(visit),
            _ => None,
        }
    }

    /// Whether Play goes to it without a check that is still to come or went against it: no check needed, or said open
    /// for `asker`.
    pub fn proven(&self, asker: &Asker) -> bool {
        self.instance().is_none() || matches!(&self.verified, Verified::Open { by, .. } if by == asker)
    }
}

/// A previous instance or a friend's that was chosen: its card stays in Where when another is chosen (choosing it again
/// asks again: nothing of what was known is kept with it).
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub visit: Visit,
    pub origin: Origin,
}

/// Play pressed while an answer is to come.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingPlay {
    generation: u64,
    mode: LaunchMode,
    at: Instant,
}

/// A new instance VRChat is making for Play.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Creating {
    generation: u64,
    mode: LaunchMode,
}

/// What Play does, decided from the selection alone.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// VRChat's own start.
    Plain,
    /// That card's own (an invite, an event, a new instance of a place).
    Card(String),
    /// Into this instance, proven.
    Enter(Visit),
    /// A new instance of this one's world.
    New(Visit),
    /// VRChat is to be asked first ([`Desk::check`] with the press).
    Check,
    /// An answer is on its way: this press waits for it too ([`Desk::wait`]).
    Wait,
    /// Nothing: a press already waits, or was this very moment's.
    Busy,
    /// Nothing starts: who is signed in is not known, nobody is, or the instance is closed. The line under Where says it.
    Held,
}

/// What [`Desk::take`] leaves to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Play was waiting for this: VRChat starts there now.
    Launch { visit: Visit, mode: LaunchMode, profile: Option<u32> },
    /// The new instance could not be made, or gone to.
    MakeFailed(String),
}

enum Reply {
    Open { generation: u64, asker: Asker, open: Option<bool>, at: Instant },
    Made { generation: u64, asker: Asker, result: Result<(), String> },
}

/// A check under way: what it was asked under and for. Answered once; dropped without an answer, "not known".
pub struct Ticket {
    generation: u64,
    asker: Asker,
    pub visit: Visit,
    tx: Option<mpsc::Sender<Reply>>,
}

impl Ticket {
    pub fn answer(mut self, open: Option<bool>, at: Instant) {
        self.send(open, at);
    }

    fn send(&mut self, open: Option<bool>, at: Instant) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Reply::Open { generation: self.generation, asker: self.asker.clone(), open, at });
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.send(None, Instant::now());
    }
}

/// A new instance being made for Play.
pub struct MakeTicket {
    generation: u64,
    asker: Asker,
    fence: Arc<Mutex<u64>>,
    tx: Option<mpsc::Sender<Reply>>,
}

impl MakeTicket {
    /// The instance made (or not): `start` sends VRChat there only if the selection is still the one Play was pressed on,
    /// which cannot change while it runs (another page may be shown by now: the press still counts).
    pub fn finish<T>(mut self, made: Result<T, String>, start: impl FnOnce(T) -> Result<(), String>) {
        let result = {
            let current = self.fence.lock().unwrap_or_else(|e| e.into_inner());
            match made {
                Err(e) => Err(e),
                Ok(t) if *current == self.generation => start(t),
                Ok(_) => Ok(()),
            }
        };
        self.send(result);
    }

    fn send(&mut self, result: Result<(), String>) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Reply::Made { generation: self.generation, asker: self.asker.clone(), result });
        }
    }
}

impl Drop for MakeTicket {
    fn drop(&mut self) {
        self.send(Err(orbiters_i18n::tr("VRChat answered something unexpected.").into()));
    }
}

/// The selection's owner.
pub struct Desk {
    selection: Selection,
    offer: Option<Offer>,
    asker: Asker,
    pending: Option<PendingPlay>,
    creating: Option<Creating>,
    /// The current generation, as the workers that start VRChat see it.
    fence: Arc<Mutex<u64>>,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    /// When Play last did something, and under which generation.
    pressed: Option<(f64, u64)>,
    /// The page's pass Where was last drawn in.
    drawn: u64,
    /// Pictures of the page: nothing is asked, and what was handed in stays.
    quiet: bool,
}

impl Default for Desk {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            selection: Selection { destination: Destination::Home, origin: Origin::Home, verified: Verified::Unverified, generation: 0 },
            offer: None,
            asker: Asker::default(),
            pending: None,
            creating: None,
            fence: Arc::default(),
            tx,
            rx,
            pressed: None,
            drawn: 0,
            quiet: false,
        }
    }
}

impl Desk {
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    pub fn asker(&self) -> &Asker {
        &self.asker
    }

    pub fn offer(&self) -> Option<&Offer> {
        self.offer.as_ref()
    }

    /// The one way the selection changes: the whole value at once, under a new generation. Whatever waited for the one
    /// before (a press of Play, a check, an instance being made) counts for nothing from here on.
    fn replace(&mut self, destination: Destination, origin: Origin, verified: Verified) {
        let generation = self.selection.generation + 1;
        *self.fence.lock().unwrap_or_else(|e| e.into_inner()) = generation;
        self.pending = None;
        self.creating = None;
        if let (Some(visit), Origin::Previous | Origin::Friend { .. }) = (destination.visit(), &origin) {
            self.offer = Some(Offer { visit: visit.clone(), origin: origin.clone() });
        }
        self.selection = Selection { destination, origin, verified, generation };
    }

    /// Home world, a card, an instance (not verified: [`Desk::check`] asks), or a new one of its world.
    pub fn choose(&mut self, destination: Destination, origin: Origin) {
        self.replace(destination, origin, Verified::Unverified);
    }

    /// An instance VRChat said was open at `at` for `by` (the last instance's card, whose state is read ahead).
    pub fn choose_open(&mut self, key: String, visit: Visit, origin: Origin, at: Instant, by: Asker) {
        let verified = if by == self.asker { Verified::Open { at, by } } else { Verified::Unverified };
        self.replace(Destination::Instance { key, visit }, origin, verified);
    }

    pub fn go_home(&mut self) {
        self.replace(Destination::Home, Origin::Home, Verified::Unverified);
    }

    /// The offered card goes (its friend left the instance).
    pub fn withdraw_offer(&mut self) {
        self.offer = None;
    }

    /// The account or the profile chosen now. Another one than before: the destination stays, nothing known of it does,
    /// and what waited is off. Whether it changed.
    pub fn set_asker(&mut self, now: Asker) -> bool {
        if now == self.asker {
            return false;
        }
        self.asker = now;
        if self.quiet {
            if let Verified::Open { by, .. } = &mut self.selection.verified {
                *by = self.asker.clone();
            }
            return false;
        }
        self.again(Verified::Unverified);
        true
    }

    /// The same destination under a new generation.
    fn again(&mut self, verified: Verified) {
        let (destination, origin) = (self.selection.destination.clone(), self.selection.origin.clone());
        self.replace(destination, origin, verified);
    }

    /// Where is drawn in `pass`. Not drawn in the one before (another page, All tools): a press made then is off, so an
    /// answer never starts VRChat for someone who came back to Home long after.
    pub fn drawn(&mut self, pass: u64) {
        if pass > self.drawn + 1 {
            self.pending = None;
        }
        self.drawn = pass;
    }

    /// Where is hidden (VRChat runs on the profile chosen, or cannot be started): what waited is off.
    pub fn hidden(&mut self) {
        if self.pending.is_some() || self.creating.is_some() {
            // A check under way belonged to the generation before: asked again when Where is back.
            let verified = match self.selection.verified.clone() {
                Verified::Checking { .. } => Verified::Unverified,
                v => v,
            };
            self.again(verified);
        }
    }

    /// Starts a check of the selected instance, for the press `play` when there is one: the wait is in place before the
    /// ticket exists, so no answer can come before it. `None` when there is nothing to check, or nobody to ask for.
    pub fn check(&mut self, play: Option<LaunchMode>, now: Instant) -> Option<Ticket> {
        let visit = self.selection.instance().cloned()?;
        self.asker.account.as_ref()?;
        let generation = self.selection.generation + 1;
        self.again(Verified::Checking { generation, since: now });
        self.pending = play.map(|mode| PendingPlay { generation, mode, at: now });
        Some(Ticket { generation, asker: self.asker.clone(), visit, tx: Some(self.tx.clone()) })
    }

    /// The press waits for the check under way.
    pub fn wait(&mut self, mode: LaunchMode, now: Instant) {
        if matches!(self.selection.verified, Verified::Checking { .. }) {
            self.pending = Some(PendingPlay { generation: self.selection.generation, mode, at: now });
        }
    }

    /// VRChat is to make a new instance for Play.
    pub fn create(&mut self, mode: LaunchMode) -> MakeTicket {
        let generation = self.selection.generation;
        self.creating = Some(Creating { generation, mode });
        MakeTicket { generation, asker: self.asker.clone(), fence: self.fence.clone(), tx: Some(self.tx.clone()) }
    }

    /// Every answer that came, each applied only if it was asked under the current generation, for the account and the
    /// profile chosen now. A check too old is "not known".
    pub fn take(&mut self, now: Instant) -> Vec<Outcome> {
        let mut out = Vec::new();
        while let Ok(reply) = self.rx.try_recv() {
            out.extend(self.apply(reply, now));
        }
        if let Verified::Checking { since, .. } = self.selection.verified {
            if !self.quiet && now.saturating_duration_since(since) > CHECK_FOR {
                self.again(Verified::Unknown);
            }
        }
        out
    }

    /// The one way an answer counts.
    fn apply(&mut self, reply: Reply, now: Instant) -> Option<Outcome> {
        let (generation, asker) = match &reply {
            Reply::Open { generation, asker, .. } | Reply::Made { generation, asker, .. } => (*generation, asker),
        };
        if generation != self.selection.generation || *asker != self.asker {
            return None;
        }
        match reply {
            Reply::Made { result, .. } => {
                self.creating = None;
                result.err().map(Outcome::MakeFailed)
            }
            Reply::Open { asker, open, at, .. } => {
                if !matches!(self.selection.verified, Verified::Checking { generation: g, .. } if g == generation) {
                    return None;
                }
                self.selection.verified = match open {
                    Some(true) => Verified::Open { at, by: asker },
                    Some(false) => Verified::Closed,
                    None => Verified::Unknown,
                };
                let play = self.pending.take().filter(|p| p.generation == generation && now.saturating_duration_since(p.at) <= PLAY_WAIT)?;
                // Only from an "open" good now, however long the answer waited to be read.
                let visit = self.enterable(now)?;
                Some(Outcome::Launch { visit, mode: play.mode, profile: self.asker.profile })
            }
        }
    }

    /// The selected instance, when VRChat said it is open within the minute for the account and profile chosen now.
    fn enterable(&self, now: Instant) -> Option<Visit> {
        match (&self.selection.destination, &self.selection.verified) {
            (Destination::Instance { visit, .. }, Verified::Open { at, by }) if *by == self.asker && now.saturating_duration_since(*at) <= OPEN_FOR => Some(visit.clone()),
            _ => None,
        }
    }

    /// What Play does now. `clock`: the window's, for two presses that are one.
    pub fn decide(&mut self, now: Instant, clock: f64) -> Decision {
        let generation = self.selection.generation;
        if self.busy().is_some() || self.pressed.is_some_and(|(at, g)| g == generation && clock - at < ONE_PRESS) {
            return Decision::Busy;
        }
        let decision = match &self.selection.destination {
            Destination::Home => Decision::Plain,
            Destination::Card { key, .. } => Decision::Card(key.clone()),
            Destination::NewOf { visit, .. } => Decision::New(visit.clone()),
            Destination::Instance { .. } => match (self.enterable(now), &self.selection.verified) {
                (Some(visit), _) => Decision::Enter(visit),
                (None, Verified::Checking { .. }) => Decision::Wait,
                (None, Verified::Closed) => Decision::Held,
                (None, _) if self.asker.account.is_none() => Decision::Held,
                (None, _) => Decision::Check,
            },
        };
        if decision != Decision::Held {
            self.pressed = Some((clock, generation));
        }
        decision
    }

    /// The mode Play waits in: a check, or a new instance being made, of the current generation.
    pub fn busy(&self) -> Option<LaunchMode> {
        let current = self.selection.generation;
        let waits = self.pending.filter(|p| p.generation == current && matches!(self.selection.verified, Verified::Checking { generation, .. } if generation == current));
        waits.map(|p| p.mode).or(self.creating.filter(|c| c.generation == current).map(|c| c.mode))
    }

    /// Whether it is a check that Play waits for (and not a new instance).
    pub fn rechecking(&self) -> bool {
        self.busy().is_some() && self.creating.is_none()
    }

    /// Pictures of the page: `destination` chosen with what is known of it; nothing is asked.
    pub fn stand_in(&mut self, destination: Destination, origin: Origin, verified: Verified) {
        self.quiet = true;
        let generation = self.selection.generation + 1;
        let verified = match verified {
            Verified::Checking { since, .. } => Verified::Checking { generation, since },
            Verified::Open { at, .. } => Verified::Open { at, by: self.asker.clone() },
            v => v,
        };
        self.replace(destination, origin, verified);
    }

    /// Pictures of the page: a new instance being made for Play in `mode`.
    pub fn stand_in_making(&mut self, mode: LaunchMode) {
        self.creating = Some(Creating { generation: self.selection.generation, mode });
    }

    /// Pictures of the page: nothing is asked.
    pub fn is_quiet(&self) -> bool {
        self.quiet
    }

    /// Tests: VRChat's "open" made `by` ago.
    #[cfg(test)]
    pub fn age_check(&mut self, by: Duration) {
        if let Verified::Open { at, .. } = &mut self.selection.verified {
            *at = at.checked_sub(by).unwrap_or(*at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visit(n: u32) -> Visit {
        Visit { world_id: format!("wrld_00000000-0000-4000-8000-{n:012}"), instance: format!("{n}~hidden(usr_00000000-0000-4000-8000-000000000004)~region(eu)"), name: format!("World {n}"), at: 0 }
    }

    fn instance(n: u32) -> Destination {
        Destination::Instance { key: visit(n).key(), visit: visit(n) }
    }

    fn new_of(n: u32) -> Destination {
        Destination::NewOf { key: visit(n).key(), visit: visit(n) }
    }

    fn asker(account: Option<&str>, profile: u32) -> Asker {
        Asker { account: account.map(str::to_string), profile: Some(profile) }
    }

    fn desk() -> Desk {
        let mut d = Desk::default();
        d.set_asker(asker(Some("usr_a"), 0));
        d
    }

    /// verify2-r21 M4: answers come through a channel that keeps each: a stale one, published after the current one and
    /// before anybody read, neither replaces it nor counts.
    #[test]
    fn a_stale_answer_published_last_counts_for_nothing() {
        let now = Instant::now();
        let mut d = desk();
        d.choose(instance(1), Origin::Previous);
        let a = d.check(None, now).unwrap();
        d.choose(instance(2), Origin::Previous);
        let b = d.check(None, now).unwrap();
        b.answer(Some(true), now);
        a.answer(Some(false), now);
        assert!(d.take(now).is_empty());
        assert!(matches!(d.selection().verified(), Verified::Open { .. }));
        assert_eq!(d.selection().instance(), Some(&visit(2)));
    }

    /// verify2-r21 M4: a worker that could not start, or ended without an answer, leaves "not known" and Play free.
    #[test]
    fn a_worker_that_never_answers_leaves_play_free() {
        let now = Instant::now();
        let mut d = desk();
        d.choose(instance(1), Origin::Previous);
        drop(d.check(Some(LaunchMode::Vr), now));
        assert!(d.busy().is_some(), "until its end is read");
        assert!(d.take(now).is_empty());
        assert_eq!((d.busy(), d.selection().verified()), (None, &Verified::Unknown));
        // One that neither answers nor ends: not known after a while.
        let held = d.check(Some(LaunchMode::Vr), now);
        assert!(d.busy().is_some());
        d.take(now + CHECK_FOR + Duration::from_secs(1));
        assert_eq!((d.busy(), d.selection().verified()), (None, &Verified::Unknown));
        drop(held);
        // The same for a new instance being made.
        d.choose(new_of(1), Origin::Previous);
        drop(d.create(LaunchMode::Vr));
        assert!(matches!(d.take(now).as_slice(), [Outcome::MakeFailed(_)]));
        assert_eq!(d.busy(), None);
    }

    /// verify2-r21 M3: a new instance made for a place left meanwhile is not gone to; one made for the place still
    /// chosen is, and only once.
    #[test]
    fn a_new_instance_made_for_a_place_left_is_not_gone_to() {
        let mut d = desk();
        d.choose(new_of(1), Origin::Previous);
        let left = d.create(LaunchMode::Vr);
        d.go_home();
        assert_eq!(d.busy(), None, "Play is free as soon as another place is chosen");
        let mut started = 0;
        left.finish(Ok(()), |()| {
            started += 1;
            Ok(())
        });
        assert_eq!(started, 0);
        d.choose(new_of(1), Origin::Previous);
        d.create(LaunchMode::Vr).finish(Ok(()), |()| {
            started += 1;
            Ok(())
        });
        assert_eq!(started, 1);
        assert!(d.take(Instant::now()).is_empty());
        assert_eq!(d.busy(), None);
    }

    /// verify2-r21 M3: an "open" is what VRChat said at that time, for that account and profile: it is not made fresh by
    /// being read late, and a press too old starts nothing.
    #[test]
    fn an_open_keeps_its_own_time_and_an_old_press_starts_nothing() {
        let now = Instant::now();
        let mut d = desk();
        d.choose(instance(1), Origin::Previous);
        d.check(Some(LaunchMode::Vr), now).unwrap().answer(Some(true), now);
        // Read an hour later (the window was away).
        let late = now + Duration::from_secs(3600);
        assert!(d.take(late).is_empty(), "an old press, an old answer");
        assert_eq!(d.decide(late, 10.0), Decision::Check, "asked again");
        // Back to Home after another page: the answer still proves the instance, the press is off.
        let mut d = desk();
        d.drawn(1);
        d.choose(instance(1), Origin::Previous);
        let t = d.check(Some(LaunchMode::Vr), now).unwrap();
        t.answer(Some(true), now);
        d.drawn(40);
        assert!(d.take(now).is_empty());
        assert_eq!(d.decide(now, 10.0), Decision::Enter(visit(1)));
    }

    /// The random walk's numbers (a xorshift: the same each run).
    struct Dice(u64);
    impl Dice {
        fn roll(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) % n
        }
    }

    /// verify2-r21 M3 and M4: whatever is chosen, pressed, changed and answered in whatever order: Play never starts
    /// another destination than the current selection, never a previous or friend's instance that VRChat did not say
    /// open within the minute for the account and profile chosen now; and nothing stays busy once its answers came.
    #[test]
    fn whatever_the_order_play_starts_only_what_is_chosen_and_proven_and_busy_ends() {
        let t0 = Instant::now();
        let (mut launched, mut entered_by_answer, mut busy_seen) = (0u32, 0u32, 0u32);
        for seed in 1..=400u64 {
            let mut dice = Dice(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut d = desk();
            let mut now = t0;
            let (mut clock, mut pass) = (0.0f64, 1u64);
            let mut tickets: Vec<Ticket> = Vec::new();
            let mut makes: Vec<(MakeTicket, Destination)> = Vec::new();
            // What VRChat truly said: of which instance, for whom, when: what counts as proof.
            let mut said: Vec<(Visit, Asker, Instant)> = Vec::new();
            let allowed = |d: &Desk, v: &Visit, said: &[(Visit, Asker, Instant)], now: Instant| {
                d.selection().instance() == Some(v) && said.iter().any(|(s, by, at)| s == v && by == d.asker() && now.saturating_duration_since(*at) <= OPEN_FOR)
            };
            for _ in 0..120 {
                clock += 0.05 + dice.roll(10) as f64 * 0.1;
                // Now and then the person is away: an "open" grows old with nothing else changing.
                now += Duration::from_millis(if dice.roll(6) == 0 { 50_000 + dice.roll(40_000) } else { dice.roll(6000) });
                match dice.roll(16) {
                    0 => d.go_home(),
                    1 => d.choose(Destination::Card { key: "often:w".into(), name: "W".into(), instance: false }, Origin::Card),
                    2 | 3 => d.choose(instance(dice.roll(3) as u32), Origin::Previous),
                    4 => d.choose(instance(dice.roll(3) as u32), Origin::Friend { id: "usr_f".into(), name: "F".into(), picture: None }),
                    5 => d.choose(new_of(dice.roll(3) as u32), Origin::Previous),
                    6 => {
                        d.set_asker(match dice.roll(5) {
                            0 => asker(None, 0),
                            1 => asker(Some("usr_b"), 0),
                            2 => asker(Some("usr_a"), 1),
                            _ => asker(Some("usr_a"), 0),
                        });
                    }
                    7 => d.hidden(),
                    8 => {
                        // Home left and come back to.
                        pass += 30;
                        d.drawn(pass);
                    }
                    9 => tickets.extend(d.check(None, now)),
                    10..=12 => {
                        let mode = if dice.roll(2) == 0 { LaunchMode::Vr } else { LaunchMode::Desktop };
                        match d.decide(now, clock) {
                            Decision::Enter(v) => {
                                launched += 1;
                                assert!(allowed(&d, &v, &said, now), "seed {seed}: {v:?} of {:?}", d.selection());
                            }
                            Decision::Check => tickets.extend(d.check(Some(mode), now)),
                            Decision::Wait => d.wait(mode, now),
                            Decision::New(v) => {
                                assert_eq!(d.selection().destination().visit(), Some(&v), "seed {seed}");
                                let at = d.selection().destination().clone();
                                makes.push((d.create(mode), at));
                            }
                            Decision::Card(key) => assert_eq!(d.selection().destination().key(), Some(key.as_str()), "seed {seed}"),
                            Decision::Plain => assert_eq!(d.selection().destination(), &Destination::Home, "seed {seed}"),
                            Decision::Busy | Decision::Held => {}
                        }
                    }
                    13 | 14 if !tickets.is_empty() => {
                        let t = tickets.remove(dice.roll(tickets.len() as u64) as usize);
                        match dice.roll(5) {
                            0 => drop(t),
                            1 => t.answer(Some(false), now),
                            2 => t.answer(None, now),
                            _ => {
                                said.push((t.visit.clone(), t.asker.clone(), now));
                                t.answer(Some(true), now);
                            }
                        }
                    }
                    15 if !makes.is_empty() => {
                        let (m, at) = makes.remove(dice.roll(makes.len() as u64) as usize);
                        let current = d.selection().destination().clone();
                        m.finish(if dice.roll(4) == 0 { Err("no".into()) } else { Ok(()) }, |()| {
                            launched += 1;
                            assert_eq!(at, current, "seed {seed}: a new instance for another selection");
                            Ok(())
                        });
                    }
                    _ => {}
                }
                busy_seen += u32::from(d.busy().is_some());
                // Sometimes the window does not look for a while.
                if dice.roll(3) != 0 {
                    pass += 1;
                    d.drawn(pass);
                    for o in d.take(now) {
                        if let Outcome::Launch { visit: v, profile, .. } = o {
                            launched += 1;
                            entered_by_answer += 1;
                            assert!(allowed(&d, &v, &said, now), "seed {seed}: {v:?} of {:?}", d.selection());
                            assert_eq!(profile, d.asker().profile, "seed {seed}");
                        }
                    }
                }
            }
            // Every answer comes (or its worker ends): nothing is busy.
            tickets.clear();
            for (m, _) in makes {
                m.finish(Err::<(), _>("ended".into()), |()| Ok(()));
            }
            d.take(now);
            assert_eq!(d.busy(), None, "seed {seed}: {:?}", d.selection());
        }
        assert!(launched > 200 && entered_by_answer > 20 && busy_seen > 200, "the walk went through it: {launched} {entered_by_answer} {busy_seen}");
    }
}
