//! N4N0's state behind its lock, which remembers that something wrote to it since it was last handed to the interfaces: what nobody
//! published by itself is, at the N4N0 thread's next step, and nothing looks for it meanwhile (N4N-3).

use crate::runtime::N4n0Runtime;
use std::ops::{Deref, DerefMut};
use std::sync::{Condvar, LockResult, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

pub(crate) struct Held(Mutex<Inner>);

struct Inner {
    rt: N4n0Runtime,
    changed: bool,
}

impl Held {
    pub(crate) fn new(rt: N4n0Runtime) -> Self {
        Held(Mutex::new(Inner { rt, changed: false }))
    }

    /// As [`Mutex::lock`].
    pub(crate) fn lock(&self) -> LockResult<Guard<'_>> {
        self.0.lock().map(Guard).map_err(|e| PoisonError::new(Guard(e.into_inner())))
    }
}

/// N4N0's state, for as long as it is locked: writing to it is a change.
pub(crate) struct Guard<'a>(MutexGuard<'a, Inner>);

impl Deref for Guard<'_> {
    type Target = N4n0Runtime;
    fn deref(&self) -> &N4n0Runtime {
        &self.0.rt
    }
}

impl DerefMut for Guard<'_> {
    fn deref_mut(&mut self) -> &mut N4n0Runtime {
        self.0.changed = true;
        &mut self.0.rt
    }
}

impl Guard<'_> {
    /// Written to since [`Guard::set_changed`] said otherwise.
    pub(crate) fn changed(&self) -> bool {
        self.0.changed
    }

    pub(crate) fn set_changed(&mut self, changed: bool) {
        self.0.changed = changed;
    }

    /// For what no interface shows (the N4N0 thread's own wake-ups).
    pub(crate) fn quietly(&mut self) -> &mut N4n0Runtime {
        &mut self.0.rt
    }

    /// Unlocked until `wake` is notified, `most` at most.
    pub(crate) fn wait(self, wake: &Condvar, most: Duration) -> Self {
        Guard(wake.wait_timeout(self.0, most).unwrap().0)
    }
}
