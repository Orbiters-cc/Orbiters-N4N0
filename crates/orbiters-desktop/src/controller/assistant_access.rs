//! The switch of what N4N0's assistant does from an account (`orbiters_core::features::N4N0_ASSISTANT_ACTIONS`): asked of Orbiters
//! for the member signed in, kept for their identity's epoch only, and held only while the server said yes lately. Anything
//! else (nobody, not known yet, a refresh that failed, a no) is off: those intents are not offered, not done,
//! and the page where the person does it opens instead; what waits to be done (a question, a share for the next capture) is
//! dropped as it goes off.

use orbiters_core::locks::*;
use super::Controller;
use orbiters_core::features::{Grant, N4N0_ASSISTANT_ACTIONS};
use std::time::{Duration, Instant};

/// How often it is asked again, so that a switch an administrator made reaches a running app.
const ASK_EVERY: Duration = Duration::from_secs(5 * 60);
/// How long a yes holds without another, whatever keeps the next answer (a refresh that fails ends it at once: verify5-r21 m1).
const GOOD_FOR: Duration = Duration::from_secs(15 * 60);

pub(super) struct AssistantAccess {
    /// Whose answer it is.
    epoch: u64,
    grant: Grant,
    asking: bool,
    /// N4N0 was told it is on.
    told: bool,
}

impl Default for AssistantAccess {
    fn default() -> Self {
        Self { epoch: 0, grant: Grant::new(ASK_EVERY, GOOD_FOR), asking: false, told: false }
    }
}

impl AssistantAccess {
    fn follow(&mut self, epoch: u64) {
        if self.epoch != epoch {
            *self = Self { epoch, ..Self::default() };
        }
    }

    fn allowed(&self, epoch: u64, now: Instant) -> bool {
        self.epoch == epoch && self.grant.allowed(now)
    }

    /// What the server answered for `epoch`: dropped when that is not whose it is now. Whether what it holds changed.
    fn answered(&mut self, epoch: u64, answer: &anyhow::Result<bool>, now: Instant) -> bool {
        if self.epoch != epoch {
            return false;
        }
        let before = self.grant.allowed(now);
        self.asking = false;
        // No answer is no yes, for this one: what it lets be done goes out as the person.
        self.grant.answered(&Ok(matches!(answer, Ok(true))), now);
        before != self.grant.allowed(now)
    }

    /// Whether it went off (a no, or a yes too old) since this was asked last.
    fn went_off(&mut self, now: Instant) -> bool {
        let on = self.grant.allowed(now);
        std::mem::replace(&mut self.told, on) && !on
    }
}

impl Controller {
    /// Whether the member signed in now has the account's actions.
    pub(super) fn assistant_actions(&self) -> bool {
        self.assistant_access.locked().allowed(self.identity_epoch(), Instant::now())
    }

    /// Asks Orbiters when it is time to, on a thread, with the credential of the member of the epoch the answer is kept for.
    pub(super) fn assistant_access_refresh(self: &super::Shared) {
        self.assistant_access_changed();
        let epoch = self.identity_epoch();
        let Some((api, token)) = self.account_api_of(epoch) else { return };
        {
            let mut access = self.assistant_access.locked();
            access.follow(epoch);
            if access.asking || !access.grant.due(Instant::now()) {
                return;
            }
            access.asking = true;
        }
        let ctl = self.clone();
        let started = std::thread::Builder::new().name("assistant-access".into()).spawn(move || {
            let answer = api.member_feature(&token, N4N0_ASSISTANT_ACTIONS);
            ctl.assistant_access.locked().answered(epoch, &answer, Instant::now());
            ctl.assistant_access_changed();
        });
        if started.is_err() {
            self.assistant_access.locked().asking = false;
        }
    }

    /// How long until it is asked again, or a yes is too old to hold (never, with no member it could be asked for).
    pub(super) fn assistant_access_wait(&self) -> Option<Duration> {
        self.account_api_of(self.identity_epoch())?;
        let access = self.assistant_access.locked();
        let now = Instant::now();
        let asked = if access.asking { Duration::from_secs(1) } else { access.grant.wait(now) };
        Some(if access.grant.allowed(now) { asked.min(access.grant.remaining(now)) } else { asked })
    }

    /// It went off: what waits to be done with it is dropped at once. (Outside its lock: N4N0 asks whether it is on.)
    fn assistant_access_changed(&self) {
        let off = self.assistant_access.locked().went_off(Instant::now());
        if off {
            self.n4n0.account_actions_changed();
        }
    }

    /// Tests: what Orbiters answered for the member signed in now.
    #[cfg(test)]
    pub(crate) fn assistant_actions_for_test(&self, allowed: bool) {
        self.assistant_answer_for_test(&Ok(allowed), Instant::now());
    }

    /// Tests: what Orbiters answered (or that it did not), at `now`.
    #[cfg(test)]
    pub(crate) fn assistant_answer_for_test(&self, answer: &anyhow::Result<bool>, now: Instant) {
        let epoch = self.identity_epoch();
        {
            let mut access = self.assistant_access.lock().unwrap();
            access.follow(epoch);
            access.answered(epoch, answer, now);
        }
        self.assistant_access_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Off until Orbiters says yes for this member; off again at a no, for another member, and once the yes is old; an
    /// answer for a member who left changes nothing.
    #[test]
    fn it_is_held_only_on_the_servers_word_for_this_member() {
        let now = Instant::now();
        let mut a = AssistantAccess::default();
        a.follow(3);
        assert!(!a.allowed(3, now), "not asked yet");
        a.answered(3, &Err(anyhow::anyhow!("offline")), now);
        assert!(!a.allowed(3, now), "no answer is not a yes");
        assert!(a.answered(3, &Ok(true), now));
        assert!(a.allowed(3, now) && !a.allowed(4, now), "another member's epoch");
        assert!(a.allowed(3, now + GOOD_FOR - Duration::from_secs(1)) && !a.allowed(3, now + GOOD_FOR), "an old yes ends");
        assert!(a.answered(3, &Err(anyhow::anyhow!("offline")), now) && !a.allowed(3, now), "no answer is not a yes, held or not");
        assert!(a.grant.due(now + Duration::from_secs(31)), "asked again soon");
        a.answered(3, &Ok(true), now);
        assert!(a.answered(3, &Ok(false), now) && !a.allowed(3, now));
        a.answered(3, &Ok(true), now);
        a.follow(4);
        assert!(!a.allowed(4, now) && !a.allowed(3, now));
        assert!(!a.answered(3, &Ok(true), now) && !a.allowed(4, now), "the last member's answer, late");
    }

    /// Verify4-r21 P11, verify5-r21 m1: a failed refresh, like a no, ends a yes at once; one nothing was said about since ends by
    /// itself; and what went off is said once (N4N0 drops what waits).
    #[test]
    fn a_yes_ends_with_a_failed_refresh() {
        let now = Instant::now();
        let minutes = |m: u64| now + Duration::from_secs(m * 60);
        let mut a = AssistantAccess::default();
        a.follow(3);
        a.answered(3, &Ok(true), now);
        assert!(!a.went_off(now));
        assert!(a.grant.due(minutes(5)), "asked again");
        a.answered(3, &Err(anyhow::anyhow!("offline")), minutes(5));
        assert!(!a.allowed(3, minutes(5)), "held on an old yes");
        assert!(a.went_off(minutes(5)) && !a.went_off(minutes(6)), "said once");
        a.answered(3, &Ok(true), minutes(6));
        assert!(!a.went_off(minutes(6)) && a.allowed(3, minutes(20)) && !a.allowed(3, minutes(22)));
        assert!(a.went_off(minutes(22)));
        a.answered(3, &Ok(true), minutes(23));
        assert!(!a.went_off(minutes(23)));
        a.answered(3, &Ok(false), minutes(24));
        assert!(!a.allowed(3, minutes(24)) && a.went_off(minutes(24)), "a no, at once");
    }

    /// P11 through the controller: what N4N0 asked of the account goes as the server says no, and as it says nothing.
    #[test]
    fn what_waits_goes_with_the_yes() {
        for answer in [Ok(false), Err(anyhow::anyhow!("offline"))] {
            let ctl = Controller::new(orbiters_core::settings::Settings::default());
            ctl.sign_in_for_test("1");
            ctl.update(|s| {
                s.launcher.source = orbiters_core::launcher::Source::SignedIn { user_id: "usr_a".into(), name: String::new() };
            });
            ctl.assistant_actions_for_test(true);
            let stamp = orbiters_n4n0::Stamp { epoch: ctl.identity_epoch(), question: None, surface: orbiters_n4n0::Surface::Window, seq: orbiters_n4n0::next_input() };
            ctl.n4n0_command(crate::ipc::N4n0Command::Ask { text: "put me in orange".into(), stamp: Some(stamp) });
            let asked = || ctl.view().n4n0.confirmations.iter().any(|c| c.is_local());
            assert!(asked(), "{:?}", ctl.view().n4n0.lines);
            ctl.assistant_answer_for_test(&answer, Instant::now());
            assert!(!asked(), "{answer:?}");
        }
    }

    /// The controller: off by default, and it goes with the member.
    #[test]
    fn the_switch_goes_with_the_member() {
        let ctl = Controller::new(orbiters_core::settings::Settings::default());
        assert!(!ctl.assistant_facts().actions, "nobody");
        ctl.sign_in_for_test("1");
        assert!(!ctl.assistant_facts().actions, "not held by default");
        ctl.assistant_actions_for_test(true);
        assert!(ctl.assistant_facts().actions);
        ctl.sign_in_for_test("2");
        assert!(!ctl.assistant_facts().actions, "another member");
    }
}
