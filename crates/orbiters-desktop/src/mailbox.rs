//! What the service tells a window, the overlay or the tray: the newest snapshot (each one replaces the one not read
//! yet: only the newest matters) and, in order, the other messages (notices, the library's version, Close), a few
//! hundred at most. A reader that stops reading (hung, suspended in a debugger) leaves one snapshot waiting rather
//! than a queue of full copies growing in the service.

use crate::ipc::ToClient;
use crate::state::Snapshot;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

/// Messages kept for a reader that does not read: past this, the oldest go.
const MAX_MESSAGES: usize = 256;

#[derive(Default)]
struct State {
    snapshot: Option<Arc<Snapshot>>,
    messages: VecDeque<ToClient>,
    closed: bool,
    /// For a reader that watches one thing ([`Mailbox::watching`]): whether a state differs in it from the last state it
    /// was handed. It keeps that one value between states, never a state.
    watch: Option<Box<dyn FnMut(&Snapshot) -> bool + Send>>,
}

pub struct Mailbox {
    state: Mutex<State>,
    ready: Condvar,
    /// The reader shows notices (a window, the overlay); the tray does not, so a notice it gets still waits for one.
    notices: bool,
}

impl Mailbox {
    pub fn new(shows_notices: bool) -> Arc<Self> {
        Arc::new(Self { state: Mutex::new(State::default()), ready: Condvar::new(), notices: shows_notices })
    }

    /// A mailbox for a reader inside the service that follows one thing of the state (the tray's menu: whether a capture
    /// runs; tracking smoothing: its settings), which `watched` reads from a state: it is handed the first state, then
    /// only those where that thing differs from the last state it was handed. It sleeps through every other publish,
    /// where it used to be woken for each to compare and go back to sleep. It shows no notices.
    ///
    /// Between two states the mailbox keeps the watched value alone. A whole state kept for comparison would stay
    /// alive, with a conversation or a picture long gone from the service, until the watched thing changed: hours.
    pub fn watching<K: PartialEq + Send + 'static>(watched: fn(&Snapshot) -> K) -> Arc<Self> {
        let mut last: Option<K> = None;
        let watch = move |snapshot: &Snapshot| {
            let now = watched(snapshot);
            let changed = last.as_ref() != Some(&now);
            if changed {
                last = Some(now);
            }
            changed
        };
        Arc::new(Self { state: Mutex::new(State { watch: Some(Box::new(watch)), ..Default::default() }), ready: Condvar::new(), notices: false })
    }

    pub fn shows_notices(&self) -> bool {
        self.notices
    }

    /// The newest state, replacing one not read yet. False once the reader is gone.
    pub fn post_snapshot(&self, snapshot: Arc<Snapshot>) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return false;
        }
        if state.watch.as_mut().is_some_and(|changed| !changed(&snapshot)) {
            return true;
        }
        state.snapshot = Some(snapshot);
        self.ready.notify_one();
        true
    }

    /// Another message, after the ones waiting. False once the reader is gone.
    pub fn post(&self, message: ToClient) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return false;
        }
        if let ToClient::Snapshot { snapshot } = message {
            drop(state);
            return self.post_snapshot(Arc::new(*snapshot));
        }
        state.messages.push_back(message);
        while state.messages.len() > MAX_MESSAGES {
            state.messages.pop_front();
        }
        self.ready.notify_one();
        true
    }

    /// Waits for mail: the messages waiting, then the newest snapshot. `None` once closed and empty.
    pub fn take(&self) -> Option<(Vec<ToClient>, Option<Arc<Snapshot>>)> {
        let mut state = self.state.lock().unwrap();
        while state.snapshot.is_none() && state.messages.is_empty() && !state.closed {
            state = self.ready.wait(state).unwrap();
        }
        if state.closed && state.snapshot.is_none() && state.messages.is_empty() {
            return None;
        }
        Some((state.messages.drain(..).collect(), state.snapshot.take()))
    }

    /// The reader is gone: what waits is dropped and nothing more is kept.
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.snapshot = None;
        state.messages.clear();
        self.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::settings::Settings;

    #[test]
    fn keeps_the_newest_snapshot_and_a_bounded_queue() {
        let mailbox = Mailbox::new(true);
        for _ in 0..10 {
            assert!(mailbox.post_snapshot(Arc::new(Snapshot::new(Settings::default()))));
        }
        for version in 0..(MAX_MESSAGES as u64 + 50) {
            assert!(mailbox.post(ToClient::Library { version }));
        }
        let (messages, snapshot) = mailbox.take().unwrap();
        assert!(snapshot.is_some(), "one snapshot, the newest");
        assert_eq!(messages.len(), MAX_MESSAGES, "the oldest messages went");
        assert!(matches!(messages[0], ToClient::Library { version: 50 }));
        mailbox.close();
        assert!(!mailbox.post(ToClient::Show), "a closed mailbox takes nothing");
        assert!(mailbox.take().is_none());
    }

    #[test]
    fn a_reader_that_watches_one_thing_is_handed_only_its_changes() {
        let mailbox = Mailbox::watching(|s| s.settings.tracking.clone());
        let first = Arc::new(Snapshot::new(Settings::default()));
        // The first state, whatever it holds.
        assert!(mailbox.post_snapshot(first.clone()));
        let (_, handed) = mailbox.take().unwrap();
        assert!(handed.is_some_and(|s| Arc::ptr_eq(&s, &first)));
        // Once the reader has it and lets it go, the mailbox holds no state: only the value it watches (review N3: it
        // kept the whole first state, with whatever it held, until that value changed).
        let gone = Arc::downgrade(&first);
        let copy = (*first).clone();
        drop(first);
        assert!(gone.upgrade().is_none(), "the state handed is kept by nobody once its reader let it go");
        let first = copy;
        // States that differ elsewhere: nothing waits, and the reader would sleep on.
        let mut elsewhere = first.clone();
        elsewhere.n4n0.afk = true;
        elsewhere.settings.walk_boost.strength += 0.25;
        let elsewhere = Arc::new(elsewhere);
        assert!(mailbox.post_snapshot(elsewhere.clone()), "still subscribed");
        assert!(mailbox.state.lock().unwrap().snapshot.is_none());
        assert_eq!(Arc::strong_count(&elsewhere), 1, "a state it was not handed is not kept either");
        // What it watches changed since the state it was handed, two publishes later: handed that newest state.
        let mut changed = (*elsewhere).clone();
        changed.settings.tracking.enabled = !changed.settings.tracking.enabled;
        let changed = Arc::new(changed);
        assert!(mailbox.post_snapshot(changed.clone()));
        let mut later = (*changed).clone();
        later.n4n0.afk = false;
        assert!(mailbox.post_snapshot(Arc::new(later)));
        let (_, handed) = mailbox.take().unwrap();
        assert!(handed.is_some_and(|s| Arc::ptr_eq(&s, &changed)), "the state that changed it, not a later one that did not");
        // Back to what it was: a change again.
        assert!(mailbox.post_snapshot(elsewhere.clone()));
        assert!(mailbox.take().unwrap().1.is_some_and(|s| Arc::ptr_eq(&s, &elsewhere)));
        // Messages reach it as any other mailbox.
        assert!(mailbox.post(ToClient::Library { version: 2 }));
        let (messages, handed) = mailbox.take().unwrap();
        assert!(messages.len() == 1 && handed.is_none());
        assert!(!mailbox.shows_notices());
    }
}
