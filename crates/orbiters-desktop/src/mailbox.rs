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

    pub fn shows_notices(&self) -> bool {
        self.notices
    }

    /// The newest state, replacing one not read yet. False once the reader is gone.
    pub fn post_snapshot(&self, snapshot: Arc<Snapshot>) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return false;
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
}
