//! N4N0's state behind its lock, which remembers that something wrote to it since it was last handed to the interfaces: what nobody
//! published by itself is, at the N4N0 thread's next step, and nothing looks for it meanwhile (N4N-3).

use orbiters_core::locks::*;
use crate::runtime::N4n0Runtime;
use std::ops::{Deref, DerefMut};
use std::sync::{Condvar, Mutex, MutexGuard};
#[cfg(test)]
use std::sync::{LockResult, PoisonError};
use std::time::Duration;

pub(crate) struct Held(Mutex<Inner>);

struct Inner {
    rt: N4n0Runtime,
    changed: bool,
    /// The N4N0 thread sleeps until something happens: the next write wakes it, to hand it over.
    resting: bool,
}

impl Held {
    pub(crate) fn new(rt: N4n0Runtime) -> Self {
        Held(Mutex::new(Inner { rt, changed: false, resting: false }))
    }

    /// As [`Locked::locked`]: also after a thread panicked with it.
    #[track_caller]
    pub(crate) fn locked(&self) -> Guard<'_> {
        Guard(self.0.locked())
    }

    /// As [`Mutex::lock`], for the tests.
    #[cfg(test)]
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
        if std::mem::take(&mut self.0.resting) {
            self.0.rt.wake();
        }
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

    /// Unlocked until `wake` is notified, `most` at most; with `resting`, until anything is written too.
    pub(crate) fn wait(mut self, wake: &Condvar, most: Duration, resting: bool) -> Self {
        self.0.resting = resting;
        let mut held = wake.wait_timeout_locked(self.0, most).0;
        held.resting = false;
        Guard(held)
    }
}
