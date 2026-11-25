//! How the service's threads wait (review 1124, XCT-23, DCT-14): a loop that has nothing to do sleeps until the published
//! state changes in what it looks at, until something of its own wakes it, or until the one time it knows something is due.
//! It polls at a pace only while what it follows has no such signal (VRChat's log growing, the headset's frames).
//!
//! A loop asks for its [`Rest`] once, naming what it watches, and waits on it with the longest time it may sleep. The state
//! hands it nothing: it is woken, and looks at the state itself ([`Controller::view`]). Everything wakes when the account
//! installed changes, a window or an overlay links (what nobody looked at is looked at again), or the app quits.
//!
//! Each return from a wait is counted under the loop's name, for `--idle-service` and the test that holds an idle minute to
//! its budget (`controller::rest::tests`).

use super::Controller;
use crate::mailbox::Mailbox;
use crate::state::Snapshot;
use orbiters_core::slice::Slice;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// A part of the state as something to watch: changed exactly when the part did, known without looking at what it holds. It
/// keeps that one part, never a state.
#[derive(Clone)]
pub(crate) struct Part<T>(Slice<T>);

impl<T> Part<T> {
    pub(crate) fn of(slice: &Slice<T>) -> Self {
        Self(slice.clone())
    }
}

impl<T> PartialEq for Part<T> {
    fn eq(&self, other: &Self) -> bool {
        Slice::same(&self.0, &other.0)
    }
}

/// One loop's wait.
pub(crate) struct Rest {
    mailbox: Arc<Mailbox>,
    turns: Arc<AtomicU64>,
}

impl Rest {
    /// Sleeps until what the loop watches changed, something woke it, or `most` passed (never, without one). What came while
    /// it worked is not missed: the wait is over at once.
    pub(crate) fn wait(&self, most: Option<Duration>) {
        self.mailbox.wait(most);
        self.turns.fetch_add(1, Ordering::Relaxed);
    }

    /// Ends the wait under way, or the next one.
    pub(crate) fn wake(&self) {
        self.mailbox.nudge();
    }
}

impl Drop for Rest {
    fn drop(&mut self) {
        self.mailbox.close();
    }
}

/// Every loop's.
#[derive(Default)]
pub(super) struct Rests {
    loops: Mutex<Vec<(&'static str, Weak<Rest>)>>,
    turns: Mutex<BTreeMap<&'static str, Arc<AtomicU64>>>,
}

impl Rests {
    fn loops(&self) -> std::sync::MutexGuard<'_, Vec<(&'static str, Weak<Rest>)>> {
        self.loops.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Wakes every loop.
    pub(super) fn wake_all(&self) {
        let mut loops = self.loops();
        loops.retain(|(_, rest)| rest.upgrade().inspect(|rest| rest.wake()).is_some());
    }

    /// Wakes the loop `name`, when there is one.
    pub(super) fn wake(&self, name: &str) {
        for rest in self.loops().iter().filter(|(n, _)| *n == name).filter_map(|(_, rest)| rest.upgrade()) {
            rest.wake();
        }
    }
}

impl Controller {
    /// The wait of the loop `name`, woken when `watched` changes in the published state.
    pub(crate) fn rest<K: PartialEq + Send + 'static>(&self, name: &'static str, watched: fn(&Snapshot) -> K) -> Arc<Rest> {
        let mailbox = Mailbox::watching(watched);
        // The state now is what the loop begins with: only a change of it wakes it.
        self.watch_from_now(&mailbox);
        let turns = self.rests.turns.lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(name).or_default().clone();
        let rest = Arc::new(Rest { mailbox, turns });
        self.rests.loops().push((name, Arc::downgrade(&rest)));
        rest
    }

    /// How often each loop woke so far.
    pub(crate) fn wake_ups(&self) -> BTreeMap<&'static str, u64> {
        let turns = self.rests.turns.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut all: BTreeMap<&'static str, u64> = turns.iter().map(|(name, n)| (*name, n.load(Ordering::Relaxed))).collect();
        all.insert("n4n0", self.n4n0.steps());
        all
    }

    /// Whether an interface shows the state: a window, the overlay or the desktop panels linked, or one in this process. What
    /// only they show is not kept up for nobody.
    pub(crate) fn shown(&self) -> bool {
        self.repaint.lock().unwrap().is_some() || self.subscribers.lock().unwrap().iter().any(|m| m.shows_notices() && !m.closed())
    }
}

/// How long until `every` passed since `last`: no time, when it never was.
pub(super) fn until(last: Option<Instant>, every: Duration) -> Duration {
    last.map_or(Duration::ZERO, |last| every.saturating_sub(last.elapsed()))
}

/// The sooner of two waits, each `None` for "nothing is due".
pub(super) fn sooner(a: Option<Duration>, b: Option<Duration>) -> Option<Duration> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::settings::Settings;

    #[test]
    fn a_rest_ends_for_what_it_watches_and_nothing_else() {
        let ctl = Controller::new(Settings::default());
        let rest = ctl.rest("test", |s| (s.vrchat.running, Part::of(&s.vrchat.world)));
        let waited = |most: u64| {
            let began = Instant::now();
            rest.wait(Some(Duration::from_millis(most)));
            began.elapsed()
        };
        assert!(waited(30) >= Duration::from_millis(30), "nothing happened: its time");
        // Something else changed, then what it watches.
        ctl.update(|s| s.n4n0.afk = true);
        assert!(waited(30) >= Duration::from_millis(30));
        ctl.update(|s| s.vrchat.running = true);
        assert!(waited(5000) < Duration::from_millis(2500));
        // A part written to with what it held is the same part.
        ctl.update(|s| s.vrchat.world.players.clear());
        assert!(waited(30) >= Duration::from_millis(30));
        ctl.update(|s| s.vrchat.world.in_world = true);
        assert!(waited(5000) < Duration::from_millis(2500));
        // Woken while it worked: the next wait is over at once, and only that one.
        rest.wake();
        assert!(waited(5000) < Duration::from_millis(2500));
        assert!(waited(30) >= Duration::from_millis(30));
        ctl.rests.wake_all();
        assert!(waited(5000) < Duration::from_millis(2500));
        ctl.rests.wake("another");
        assert!(waited(30) >= Duration::from_millis(30));
        ctl.rests.wake("test");
        assert!(waited(5000) < Duration::from_millis(2500));
        assert_eq!(ctl.wake_ups()["test"], 10);
        // Gone with its loop.
        drop(rest);
        ctl.update(|s| s.vrchat.running = false);
        assert!(ctl.subscribers.lock().unwrap().is_empty());
    }

    /// XCT-23: a minute in which nothing happens (no VRChat, no SteamVR, nobody signed in, no window) wakes the service less than
    /// once a second in all, and publishes nothing. Most of what is left is the one look at the processes every three seconds: no
    /// event says that a program starts. Beyond the loops started here: the OSC socket (four a minute), the bridge's keeper (none
    /// while nobody is signed in: `bridge::keeper`), the statistics' upload (one); `--idle-service` counts every thread.
    #[test]
    fn an_idle_minute_wakes_the_service_less_than_once_a_second() {
        const SECONDS: u64 = 20;
        let mut settings = Settings::default();
        settings.osc.enabled = false;
        let ctl = Controller::new(settings);
        crate::controller::launcher::pretend_this_computer(&ctl);
        // The folders followed are there: what other tests make beside them is nothing to this service.
        let account = orbiters_core::account::account_path(orbiters_core::account::Server::Production).unwrap();
        for folder in [account.parent().unwrap().to_path_buf(), crate::controller::world_captures::root().join("requests")] {
            std::fs::create_dir_all(folder).unwrap();
        }
        ctl.start_loops();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = crate::ipc::Endpoint { port: listener.local_addr().unwrap().port(), token: "idle-test".into() };
        let service = crate::service::Service::around(ctl.clone(), endpoint, Default::default());
        service.accept(listener).unwrap();
        service.keep().unwrap();
        std::thread::sleep(Duration::from_secs(5));
        let (before, published) = (ctl.wake_ups(), ctl.publishes());
        std::thread::sleep(Duration::from_secs(SECONDS));
        let (after, published) = (ctl.wake_ups(), ctl.publishes() - published);
        ctl.quitting.store(true, Ordering::Relaxed);
        ctl.rests.wake_all();
        ctl.n4n0.shutdown();
        let woke: BTreeMap<&str, u64> = after.iter().map(|(name, n)| (*name, n - before.get(name).copied().unwrap_or(0))).filter(|(_, n)| *n > 0).collect();
        let a_minute = woke.values().sum::<u64>() * 60 / SECONDS;
        println!("{} loops; awake in {SECONDS} s: {woke:?}; {a_minute} a minute", after.len());
        assert!(after.len() >= 20, "every loop is counted: {after:?}");
        assert!(a_minute < 60, "{a_minute} a minute: {woke:?}");
        assert_eq!(published, 0, "nothing happened");
        assert!(woke.keys().all(|name| ["watchers", "n4n0", "perf", "account", "overlay-keeper", "launcher"].contains(name)), "{woke:?}");
    }

    #[test]
    fn the_state_is_shown_while_an_interface_is_linked() {
        let ctl = Controller::new(Settings::default());
        let rest = ctl.rest("test", |_| ());
        assert!(!ctl.shown(), "a loop's wait is no interface");
        let window = Mailbox::new(true);
        ctl.subscribe(window.clone());
        assert!(ctl.shown());
        let began = Instant::now();
        rest.wait(Some(Duration::from_secs(5)));
        assert!(began.elapsed() < Duration::from_millis(2500), "a window that links wakes every loop");
        window.close();
        assert!(!ctl.shown());
    }
}
