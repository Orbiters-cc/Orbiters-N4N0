//! Who an input belongs to, and what may still act for it (verify2-r21): one gate, one fence.
//!
//! The Orbiters **identity epoch** ([`Host::identity_epoch`](crate::Host::identity_epoch)) moves exactly when the member
//! signed in changes (the environment and the member, never the credential). Every input of a person is stamped by the
//! surface that captured it ([`Stamp`]: the epoch it saw, the question it was shown, its sequence number) and the stamp
//! is carried unchanged to the service. [`N4n0::let_in`] is the one place an input is let in: for the epoch signed in
//! now only; and [`Fence::answer_with`] the one place a yes or no is taken as an answer: to the question it names only.
//!
//! What acts for an input (a turn of the assistant, an answer, a quick command, what they started and that ends later)
//! acts through a [`Fence`]. A fence exists only while its [`Ticket`] is valid, and holds the effects gate shared; an
//! identity handover ([`N4n0::pause_effects`]) holds it alone. So a side effect either ended before the handover, or
//! starts after it, finds its ticket stale and does nothing.
//!
//! It is a requirement of the types: the functions that act for the assistant are the fence's own (`impl Fence`), and a
//! turn's thread is given an [`Actor`], which holds N4N0 privately and hands out fences only. Code that has no fence has
//! nothing to act with. `tests_identity` reads the source to hold what the types cannot.
//!
//! Locks, outermost first: `voice_lifecycle`, effects, then N4N0's, the voice's and the app's account. A fence is never
//! taken with one of the last three held.

use crate::command::{Stamp, Surface};
use crate::runtime::N4n0;
use crate::status::{N4n0Who, SignedAs};
use orbiters_i18n::tr;
use orbiters_n4n0_core::context::Taint;
use orbiters_n4n0_core::tools::Action;
use parking_lot::{RwLockReadGuard, RwLockWriteGuard};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Presses and sentences remembered, so that one delivered twice acts once.
const INPUTS_KEPT: usize = 64;

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

/// N4N0 for the work of one ticket: nothing of it can be reached but through a [`Fence`].
#[derive(Clone)]
pub(crate) struct Actor {
    shared: Arc<N4n0>,
    ticket: Ticket,
}

impl Actor {
    /// The fence, while the ticket's work may act.
    pub(crate) fn fence(&self) -> Option<Fence<'_>> {
        self.shared.fence(&self.ticket)
    }
}

/// Tests: what is done as a turn's request is about to go.
#[cfg(test)]
impl Actor {
    pub(crate) fn before_send(&self) -> Option<Box<dyn FnOnce() + Send>> {
        self.shared.before_send.lock().unwrap().take()
    }
}

/// Proof that `ticket`'s work may act, for as long as it lives: an identity handover waits until it is dropped.
pub(crate) struct Fence<'a> {
    shared: &'a Arc<N4n0>,
    ticket: Ticket,
    _effects: RwLockReadGuard<'a, ()>,
}

impl Fence<'_> {
    pub(crate) fn epoch(&self) -> u64 {
        self.ticket.epoch
    }

    /// The account this work was accepted for.
    pub(crate) fn account(&self) -> Option<(orbiters_core::account::Api, String)> {
        self.host.account_for(self.ticket.epoch)
    }

    /// N4N0 for the turn `id` of the conversation's `generation`.
    pub(crate) fn turn_actor(&self, id: u64, generation: u64) -> Actor {
        Actor { shared: self.shared.clone(), ticket: Ticket { epoch: self.ticket.epoch, turn: Some((id, generation)) } }
    }
}

/// Leave for one action of the assistant's, given by the gate ([`Fence::permit`]), by the owner's yes or by the person's own
/// sentence: what acts ([`Fence::action`], [`Fence::intent_run`]) takes one, so no tool, old or new, acts without it.
pub(crate) struct Permit(());

impl Fence<'_> {
    /// The one taint rule, for every tool (verify3-r21 B4): `action` may be done at once, on a turn that read other people's
    /// text (`tainted`) only when it changes nothing; what is sent from an account, or asked about every time, never is.
    pub(crate) fn permit(&self, action: &Action, tainted: bool) -> Option<Permit> {
        let effect = action.effect();
        (!(tainted && effect.changes) && !effect.from_account && !effect.asks).then_some(Permit(()))
    }

    /// [`Fence::permit`] for a tool call of a turn that read `taint`. The owner's decision of 2026-11-24: where the worn avatar's
    /// labels are all it read of others, the one control it names may be set (the caller saw that it names exactly one).
    pub(crate) fn permit_tool(&self, action: &Action, taint: Taint) -> Option<Permit> {
        let labels_only = taint == Taint::Labels && matches!(action, Action::SetControl { .. });
        self.permit(action, taint.any() && !labels_only)
    }

    /// The owner said yes to `action`: the account's actions still need their switch now.
    pub(crate) fn permit_yes(&self, action: &Action) -> Option<Permit> {
        self.switched_on(action).then_some(Permit(()))
    }

    /// The person's own sentence asked for `action` (the offline path): what is sent from an account is asked about first.
    pub(crate) fn permit_said(&self, action: &Action) -> Option<Permit> {
        let effect = action.effect();
        (!effect.from_account && !effect.asks).then_some(Permit(()))
    }

    /// Whether the account's actions are on for this member, when `action` is one.
    pub(crate) fn switched_on(&self, action: &Action) -> bool {
        !action.effect().from_account || self.assistant_facts().actions
    }
}

#[cfg(test)]
impl Permit {
    /// Tests: as the gate would give it.
    pub(crate) fn for_test() -> Self {
        Permit(())
    }
}

/// The member of `epoch` is signed in, for as long as this lives: what the app does for them ([`N4n0::acting`]) is theirs
/// or is not done.
pub struct Acting<'a>(Fence<'a>);

impl Acting<'_> {
    pub fn epoch(&self) -> u64 {
        self.0.epoch()
    }

    /// Their account.
    pub fn account(&self) -> Option<(orbiters_core::account::Api, String)> {
        self.0.account()
    }
}

/// Who holds a fence may do what N4N0 does.
impl std::ops::Deref for Fence<'_> {
    type Target = Arc<N4n0>;
    fn deref(&self) -> &Arc<N4n0> {
        self.shared
    }
}

/// Why an input was not let in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// It carries no stamp (an older process, a message whose fields were lost).
    Unstamped,
    /// It was captured for another Orbiters identity than the one signed in now.
    OtherIdentity,
    /// Who is signed in is not known yet: nothing is anyone's.
    Unidentified,
    /// The same press or sentence, delivered again.
    Again,
}

/// The inputs let in lately: `(surface, number)`.
#[derive(Default)]
pub(crate) struct Inputs(VecDeque<(Surface, u64)>);

impl Inputs {
    /// Whether `stamp` is new (and is remembered).
    fn take(&mut self, stamp: &Stamp) -> bool {
        let key = (stamp.surface, stamp.seq);
        if self.0.contains(&key) {
            return false;
        }
        self.0.push_back(key);
        while self.0.len() > INPUTS_KEPT {
            self.0.pop_front();
        }
        true
    }
}

/// The next number of an input's stamp: grows in a process, and starts from the time, so that two processes' (or two
/// runs') numbers do not meet.
pub fn next_input() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let start = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_micros() as u64).max(1);
    let _ = NEXT.compare_exchange(0, start(), Ordering::Relaxed, Ordering::Relaxed);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl N4n0 {
    /// The gate every input of a person goes through as it arrives: let in (its fence), or said calmly that nothing was
    /// done (nothing of it is repeated). An input already let in (its Talk press), acted on later, passes [`N4n0::still`].
    pub(crate) fn let_in(self: &Arc<Self>, stamp: Option<Stamp>) -> Option<(Fence<'_>, Stamp)> {
        match self.admitted(stamp, true) {
            Ok(admitted) => Some(admitted),
            Err(why) => {
                self.refused(why);
                None
            }
        }
    }

    /// [`N4n0::let_in`] for an input let in before, whose words came since (what was said between a press and its
    /// recognition).
    pub(crate) fn still(self: &Arc<Self>, stamp: Stamp) -> Option<(Fence<'_>, Stamp)> {
        match self.admitted(Some(stamp), false) {
            Ok(admitted) => Some(admitted),
            Err(why) => {
                self.refused(why);
                None
            }
        }
    }

    fn admitted(self: &Arc<Self>, stamp: Option<Stamp>, fresh: bool) -> Result<(Fence<'_>, Stamp), Refusal> {
        let Some(stamp) = stamp else { return Err(Refusal::Unstamped) };
        let fence = self.fence(&Ticket::input(stamp.epoch)).ok_or(Refusal::OtherIdentity)?;
        if self.host.signed_as() == SignedAs::Unknown {
            return Err(Refusal::Unidentified);
        }
        let mut stamp = stamp;
        if fresh {
            let mut rt = fence.n4n0.lock().unwrap();
            if !rt.inputs.take(&stamp) {
                return Err(Refusal::Again);
            }
            // The question waiting is named only by a surface it was presented on (verify3-r21 B3): named otherwise, the input
            // names none, and answers nothing.
            if stamp.question.is_some() && stamp.question == rt.question() && !rt.presented.contains(&stamp.surface) {
                stamp.question = None;
            }
        }
        Ok((fence, stamp))
    }

    /// A press on a surface's Hang up or Mute: whoever is signed in by then, or while that is not known, it is the person's
    /// own call it ends or mutes (verify5-r21 M1). Which call it names says which: a member leaving ends theirs, and no
    /// other call has its number.
    pub(crate) fn let_in_call(self: &Arc<Self>, stamp: Option<Stamp>) -> Option<Fence<'_>> {
        let Some(stamp) = stamp else {
            self.refused(Refusal::Unstamped);
            return None;
        };
        let fence = self.fence(&Ticket::input(self.host.identity_epoch()))?;
        let taken = fence.n4n0.lock().unwrap().inputs.take(&stamp);
        taken.then_some(fence)
    }

    fn refused(&self, why: Refusal) {
        let text = match why {
            Refusal::Unstamped => tr("That came from an older part of Orbiters - N4N0: nothing was done. Send it again."),
            Refusal::OtherIdentity => tr("That was for the Orbiters account signed in before: nothing was done."),
            Refusal::Unidentified => tr("Orbiters is still signing you in: nothing was done. Try again in a moment."),
            Refusal::Again => return,
        };
        self.n4n0.lock().unwrap().line(N4n0Who::Note, text.to_string());
        self.publish();
    }

    /// The stamp of an input N4N0 itself captures now (the avatar's menu, hands-free, the wake word): the identity signed
    /// in, and the question that waits if it was presented on `surface`, where the person is.
    pub(crate) fn own_stamp(&self, surface: Surface) -> Stamp {
        let rt = self.n4n0.lock().unwrap();
        let question = rt.question().filter(|_| rt.presented.contains(&surface));
        Stamp { epoch: rt.account_epoch, question, surface, seq: next_input() }
    }

    /// Acts for `surface` as it stands now, through the gate (what the avatar's own menu asks).
    pub(crate) fn as_now(self: &Arc<Self>, surface: Surface, act: impl FnOnce(&Fence<'_>)) {
        let stamp = self.own_stamp(surface);
        if let Some((fence, _)) = self.let_in(Some(stamp)) {
            act(&fence);
        }
    }

    /// The member of `epoch`, while they are the one signed in: the app's own way to act for them (what it resolves under
    /// this is theirs, and a handover waits for what it does).
    pub fn acting(self: &Arc<Self>, epoch: u64) -> Option<Acting<'_>> {
        self.fence(&Ticket::input(epoch)).map(Acting)
    }

    /// What a surface asked was for another identity (or had no stamp): said calmly.
    pub fn refuse_other_identity(&self) {
        self.refused(Refusal::OtherIdentity);
    }

    /// The fence of `ticket`'s work, while it may act; `None` once it may not (another identity is signed in, or its
    /// turn is over).
    pub(crate) fn fence(self: &Arc<Self>, ticket: &Ticket) -> Option<Fence<'_>> {
        // Recursive: a fenced tool may start work that fences itself, and a handover waiting must not lock it out.
        let effects = self.effects.read_recursive();
        if ticket.epoch == 0 || ticket.epoch != self.host.identity_epoch() {
            return None;
        }
        if let Some((id, generation)) = ticket.turn {
            if !self.n4n0.lock().unwrap().turn_is(id, generation) {
                return None;
            }
        }
        Some(Fence { shared: self, ticket: *ticket, _effects: effects })
    }

    /// N4N0 for what is started now and ends on another thread, or later (a menu's, or a tool's: then it is the tool's
    /// identity, whose fence is held): it acts through a fence of its own, never for another identity.
    pub(crate) fn later(self: &Arc<Self>) -> Actor {
        Actor { shared: self.clone(), ticket: Ticket::input(self.host.identity_epoch()) }
    }

    /// Waits until every side effect under way has ended, and keeps new ones out while the returned guard lives: the
    /// app installs the next Orbiters account between the two (see [`Paused::forget_account`]). Taken before any lock of
    /// the account's.
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
    /// Who is signed in is not known any more (its epoch is now `epoch`): as [`Paused::forget_account`], and what the member
    /// before (`member`, when one was known) had said is put aside, out of everyone's reach, for them alone
    /// ([`Paused::establish_account`]).
    pub fn park_account(&self, epoch: u64, member: Option<&str>) {
        self.n4n0.park_account(epoch, member);
    }

    /// `member` is signed in (its epoch is `epoch`): as [`Paused::forget_account`]; what was put aside goes on when it was
    /// theirs, and is gone when it was not.
    pub fn establish_account(&self, epoch: u64, member: &str) {
        self.n4n0.establish_account(epoch, member);
    }

    /// The member signed in changed (its epoch is now `epoch`): nothing of N4N0's conversation, its lines, what it
    /// asked, its action log, the turn under way or the sentence being listened to belongs to the next one.
    pub fn forget_account(&self, epoch: u64) {
        self.n4n0.drop_parked();
        self.n4n0.forget_account(epoch);
    }

    /// The member of `epoch` is known to be signed in again after a start: their conversation goes on (verify3-r21 M5).
    pub fn resume_account(&self, epoch: u64) {
        self.n4n0.resume_account(epoch);
    }
}
