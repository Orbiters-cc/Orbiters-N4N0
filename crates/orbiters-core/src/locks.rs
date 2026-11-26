//! What a lock does after a panic, the one rule of the whole app (XCT-31, 2026-11-25).
//!
//! The service is built on "a thread may panic and the rest goes on" (a panicking thread must not take the process down).
//! A std lock held by the thread that panicked is poisoned, and `lock().unwrap()` then panics in every thread that comes
//! after it, one at a time: the app ends as a tray icon that answers nothing. So:
//!
//! - **State is safe to go on with** ([`Locked::locked`] and its siblings, what everything uses): the data is taken as it
//!   is, the poisoning is logged once with the source location of the recovering call, and the lock is a
//!   lock like any other again. What was under it may be half changed: a list with one entry more, a part not published
//!   yet. The next change or the next publish puts it right, and nothing here is worse than answering nobody.
//! - **The few whose rule cannot be trusted half changed** are [`StartsOver`], each by name where it is made, with how it
//!   starts over: the Orbiters account signed in and the VRChat sign-in (a credential must never be another's, and the two
//!   are read again from where they are kept).
//!
//! `lock().unwrap()` stays in tests alone: a test of the app reads its sources (`orbiters-desktop/src/unseen.rs`) and
//! `scripts/visible_failures.py` moves what a branch brings.

use crate::app_log::Word;
use crate::log_error;
use std::panic::Location;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError, WaitTimeoutResult};
use std::time::Duration;

/// Said once for each poisoning: `clear_poison` goes before it, so the next one to come finds an ordinary lock.
#[cold]
fn recovered(found: &'static Location<'static>) {
    log_error!("recovered a poisoned lock at {}; keeping its state", found);
}

pub trait Locked {
    type Value: ?Sized;
    /// The lock, also after a thread panicked with it.
    fn locked(&self) -> MutexGuard<'_, Self::Value>;
    /// The lock if nobody has it, also after a thread panicked with it.
    fn try_locked(&self) -> Option<MutexGuard<'_, Self::Value>>;
}

impl<T: ?Sized> Locked for Mutex<T> {
    type Value = T;

    #[track_caller]
    fn locked(&self) -> MutexGuard<'_, T> {
        let found = Location::caller();
        self.lock().unwrap_or_else(|poisoned| {
            self.clear_poison();
            recovered(found);
            poisoned.into_inner()
        })
    }

    #[track_caller]
    fn try_locked(&self) -> Option<MutexGuard<'_, T>> {
        match self.try_lock() {
            Ok(held) => Some(held),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(poisoned)) => {
                self.clear_poison();
                recovered(Location::caller());
                Some(poisoned.into_inner())
            }
        }
    }
}

/// A lock over what cannot be trusted half changed (whose credential it holds): after a panic with it, it is started over,
/// as if the app had just started, before anybody sees it. Each is named where it is made, with how it starts over.
pub struct StartsOver<T> {
    value: Mutex<T>,
    what: Word,
    over: Box<dyn Fn(&mut T) + Send + Sync>,
}

impl<T> StartsOver<T> {
    pub fn new(what: &'static str, value: T, over: impl Fn(&mut T) + Send + Sync + 'static) -> Self {
        Self { value: Mutex::new(value), what: Word(what), over: Box::new(over) }
    }

    /// The lock; what is under it was started over if a thread panicked with it.
    #[track_caller]
    pub fn locked(&self) -> MutexGuard<'_, T> {
        let found = Location::caller();
        self.value.lock().unwrap_or_else(|poisoned| {
            let mut held = poisoned.into_inner();
            (self.over)(&mut held);
            // Only now: a panic of the starting over itself leaves it to be started over by who comes next.
            self.value.clear_poison();
            log_error!("recovered {} at {}; restored its state", self.what, found);
            held
        })
    }

    /// As [`Mutex::lock`], for tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn lock(&self) -> std::sync::LockResult<MutexGuard<'_, T>> {
        self.value.lock()
    }

    /// As [`Mutex::try_lock`], for tests that check lock ownership.
    #[cfg(any(test, feature = "test-support"))]
    pub fn try_lock(&self) -> std::sync::TryLockResult<MutexGuard<'_, T>> {
        self.value.try_lock()
    }
}

pub trait RwLocked {
    type Value: ?Sized;
    fn read_locked(&self) -> RwLockReadGuard<'_, Self::Value>;
    fn write_locked(&self) -> RwLockWriteGuard<'_, Self::Value>;
}

impl<T: ?Sized> RwLocked for RwLock<T> {
    type Value = T;

    #[track_caller]
    fn read_locked(&self) -> RwLockReadGuard<'_, T> {
        match self.read() {
            Ok(held) => held,
            Err(poisoned) => {
                // Clear and report poison under an exclusive guard, even with competing readers.
                drop(poisoned.into_inner());
                RwLockWriteGuard::downgrade(self.write_locked())
            }
        }
    }

    #[track_caller]
    fn write_locked(&self) -> RwLockWriteGuard<'_, T> {
        let found = Location::caller();
        self.write().unwrap_or_else(|poisoned| {
            self.clear_poison();
            recovered(found);
            poisoned.into_inner()
        })
    }
}

/// Waiting on a lock: another thread may panic with it meanwhile. (Said by whoever takes it next through [`Locked`]: a
/// condition has no way to the lock itself.)
pub trait Waits {
    fn wait_locked<'a, T>(&self, held: MutexGuard<'a, T>) -> MutexGuard<'a, T>;
    fn wait_timeout_locked<'a, T>(&self, held: MutexGuard<'a, T>, most: Duration) -> (MutexGuard<'a, T>, WaitTimeoutResult);
    fn wait_while_locked<'a, T, F: FnMut(&mut T) -> bool>(&self, held: MutexGuard<'a, T>, condition: F) -> MutexGuard<'a, T>;
    fn wait_timeout_while_locked<'a, T, F: FnMut(&mut T) -> bool>(&self, held: MutexGuard<'a, T>, most: Duration, condition: F) -> (MutexGuard<'a, T>, WaitTimeoutResult);
}

impl Waits for Condvar {
    fn wait_locked<'a, T>(&self, held: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        self.wait(held).unwrap_or_else(PoisonError::into_inner)
    }

    fn wait_timeout_locked<'a, T>(&self, held: MutexGuard<'a, T>, most: Duration) -> (MutexGuard<'a, T>, WaitTimeoutResult) {
        self.wait_timeout(held, most).unwrap_or_else(PoisonError::into_inner)
    }

    fn wait_while_locked<'a, T, F: FnMut(&mut T) -> bool>(&self, held: MutexGuard<'a, T>, condition: F) -> MutexGuard<'a, T> {
        self.wait_while(held, condition).unwrap_or_else(PoisonError::into_inner)
    }

    fn wait_timeout_while_locked<'a, T, F: FnMut(&mut T) -> bool>(&self, held: MutexGuard<'a, T>, most: Duration, condition: F) -> (MutexGuard<'a, T>, WaitTimeoutResult) {
        self.wait_timeout_while(held, most, condition).unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panic_with<T: Send>(lock: &Mutex<T>) {
        std::thread::scope(|s| {
            let held = s.spawn(|| {
                let _held = lock.lock().unwrap();
                panic!("with the lock, on purpose");
            });
            assert!(held.join().is_err());
        });
        assert!(lock.is_poisoned());
    }

    #[test]
    fn a_lock_goes_on_after_a_panic_with_what_is_under_it() {
        let lock = Mutex::new(vec![1, 2]);
        panic_with(&lock);
        lock.locked().push(3);
        assert!(!lock.is_poisoned(), "said once: whoever comes next finds an ordinary lock");
        assert_eq!(*lock.lock().unwrap(), [1, 2, 3]);
        panic_with(&lock);
        assert_eq!(*lock.try_locked().expect("nobody has it"), [1, 2, 3]);
        let held = lock.locked();
        assert!(lock.try_locked().is_none(), "somebody has it");
        drop(held);

        let shared = RwLock::new(5);
        for write in [true, false] {
            std::thread::scope(|s| {
                let held = s.spawn(|| {
                    let _held = shared.write().unwrap();
                    panic!("with the lock, on purpose");
                });
                assert!(held.join().is_err());
            });
            assert!(shared.is_poisoned());
            if write {
                *shared.write_locked() += 1;
            }
            assert_eq!(*shared.read_locked(), 6);
            assert!(!shared.is_poisoned());
        }
    }

    #[test]
    fn who_waits_goes_on_when_another_panics_with_the_lock() {
        let (lock, changed) = (Mutex::new(0), Condvar::new());
        std::thread::scope(|s| {
            let (announce, announced) = std::sync::mpsc::sync_channel(1);
            let (lock, changed) = (&lock, &changed);
            let waits = s.spawn(move || {
                *changed.wait_while_locked(lock.locked(), |n| {
                    if *n == 0 {
                        let _ = announce.try_send(());
                    }
                    *n == 0
                })
            });
            // The waiter reports that it holds the lock before the panicking thread tries it.
            let entered = announced.recv_timeout(Duration::from_secs(30));
            let panics = s.spawn(move || {
                let mut held = lock.locked();
                *held = 7;
                changed.notify_all();
                panic!("with the lock, on purpose");
            });
            assert!(panics.join().is_err());
            entered.expect("the waiting thread reached the condition");
            assert_eq!(waits.join().expect("it goes on"), 7);
        });
        assert!(changed.wait_timeout_locked(lock.locked(), Duration::from_millis(1)).1.timed_out());
    }

    #[test]
    fn what_cannot_be_trusted_is_started_over() {
        let lock = StartsOver::new("a test's state", (1, 1), |v| *v = (0, 0));
        assert_eq!(*lock.locked(), (1, 1));
        for _ in 0..2 {
            std::thread::scope(|s| {
                let held = s.spawn(|| {
                    let mut held = lock.locked();
                    held.0 = 5;
                    panic!("half changed, on purpose");
                });
                assert!(held.join().is_err());
            });
            assert!(lock.lock().is_err());
            assert_eq!(*lock.locked(), (0, 0), "never half of it");
            assert!(lock.lock().is_ok());
            *lock.locked() = (2, 2);
        }
    }
}
