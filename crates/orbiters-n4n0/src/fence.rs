//! Who an input belongs to, and what may still act for it (verify2-r21): one gate, one fence.
//!
//! The Orbiters **identity epoch** ([`Host::identity_epoch`](crate::Host::identity_epoch)) moves exactly when the member
//! signed in changes (the environment and the member, never the credential). Every input of a person is stamped by the
//! surface that captured it ([`Stamp`]: the epoch it saw, the question it was shown, its sequence number) and the stamp is
//! carried unchanged to the service. [`N4n0::admit`] is the one place an input is let in: for the epoch signed in now only;
//! and the one place ([`N4n0::answer_with`](crate::N4n0)) a yes or no is taken as an answer: to the question it names only.
//!
//! A **turn** of the assistant (and any other work done for an input) carries a [`Ticket`] of its epoch. What it does to
//! anything is done through a [`Fence`], which exists only while the ticket is still valid: it holds the effects gate shared,
//! and an identity handover ([`N4n0::pause_effects`]) holds it alone. So a side effect either ended before the handover or
//! starts after it, finds its ticket stale, and does nothing. The functions that act for a turn (`run_tool`, …) take a
//! `&Fence`: there is no other way to call them, and in the turn's own code the runtime is reached through the fence only
//! (`tests_identity` reads the source to hold that).

use crate::command::Stamp;
use crate::runtime::{N4n0, N4n0Runtime, Seen};
use crate::status::{N4n0Who, NoticeKind};
use orbiters_i18n::tr;
use parking_lot::{RwLockReadGuard, RwLockWriteGuard};
use std::sync::{Arc, MutexGuard};

/// What a piece of work was accepted for: an identity epoch, and (a turn of the assistant) which turn of which
/// conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ticket {
    pub(crate) epoch: u64,
    /// The turn's number and the conversation's generation it began on.
    pub(crate) turn: Option<(u64, u64)>,
}

impl Ticket {
    /// For work outside a turn (an answer, a quick command, a deferred action).
    pub(crate) fn input(epoch: u64) -> Self {
        Self { epoch, turn: None }
    }
}

/// Proof that `ticket`'s work may act, for as long as it lives: an identity handover waits until it is dropped.
pub(crate) struct Fence<'a> {
    n4n0: &'a Arc<N4n0>,
    _effects: RwLockReadGuard<'a, ()>,
}

impl<'a> Fence<'a> {
    /// The runtime's state (a turn's changes to it are made through here).
    pub(crate) fn rt(&self) -> MutexGuard<'a, N4n0Runtime> {
        self.n4n0.n4n0.lock().unwrap()
    }

    pub(crate) fn line(&self, who: N4n0Who, text: String) {
        self.n4n0.add_line(who, text);
    }

    pub(crate) fn notify(&self, kind: NoticeKind, text: String) {
        self.n4n0.notify(kind, text);
    }

    pub(crate) fn publish(&self) {
        self.n4n0.publish();
    }
}

/// Why an input was not let in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// It carries no stamp (an older process, a message whose fields were lost).
    Unstamped,
    /// It was captured for another Orbiters identity than the one signed in now.
    OtherIdentity,
}

impl N4n0 {
    /// The gate every input of a person goes through: its stamp says which identity it was captured for; only the one
    /// signed in now is let in.
    pub(crate) fn admit(&self, stamp: Option<Stamp>) -> Result<Stamp, Refusal> {
        let Some(stamp) = stamp else { return Err(Refusal::Unstamped) };
        if stamp.epoch == 0 || stamp.epoch != self.host.identity_epoch() {
            return Err(Refusal::OtherIdentity);
        }
        Ok(stamp)
    }

    /// Says calmly that an input was not used (nothing of it is repeated).
    pub(crate) fn refused(&self, why: Refusal) {
        let text = match why {
            Refusal::Unstamped => tr("That came from an older part of Orbiters - N4N0: nothing was done. Send it again."),
            Refusal::OtherIdentity => tr("That was for the Orbiters account signed in before: nothing was done."),
        };
        self.n4n0.lock().unwrap().line(N4n0Who::Note, text.to_string());
        self.publish();
    }

    /// The fence of `ticket`'s work, while it may act; `None` once it may not (another identity is signed in, or its
    /// turn is over).
    pub(crate) fn fence<'a>(self: &'a Arc<Self>, ticket: &Ticket) -> Option<Fence<'a>> {
        // Recursive: a fenced tool may start work that fences itself, and a handover waiting must not lock it out.
        let effects = self.effects.read_recursive();
        if ticket.epoch != self.host.identity_epoch() {
            return None;
        }
        if let Some((id, generation)) = ticket.turn {
            let rt = self.n4n0.lock().unwrap();
            if !rt.turn_is(id, generation) {
                return None;
            }
        }
        Some(Fence { n4n0: self, _effects: effects })
    }

    /// Waits until every side effect under way has ended, and keeps new ones out while the returned guard lives: the
    /// controller installs the next Orbiters account between the two (see [`Paused::forget_account`]). Take it before
    /// any lock of the account's.
    pub fn pause_effects(&self) -> Paused<'_> {
        Paused { n4n0: self, _effects: self.effects.write() }
    }
}

/// No side effect of N4N0's runs while this lives (see [`N4n0::pause_effects`]).
pub struct Paused<'a> {
    n4n0: &'a N4n0,
    _effects: RwLockWriteGuard<'a, ()>,
}

impl Paused<'_> {
    /// The member signed in changed (its epoch is now `epoch`): nothing of N4N0's conversation, its lines, what it asked,
    /// its action log, the turn under way or the sentence being listened to belongs to the next one.
    pub fn forget_account(&self, epoch: u64) {
        self.n4n0.forget_account(epoch);
    }
}

/// The question a press or sentence answers, as [`Seen`] (the runtime compares it under the lock it takes it under).
pub(crate) fn answered_question(stamp: &Stamp) -> Seen {
    match stamp.question {
        Some(n) => Seen::Question(n),
        None => Seen::Question(0),
    }
}
