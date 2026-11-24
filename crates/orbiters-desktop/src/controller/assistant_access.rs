//! The switch of what N4N0's assistant does from an account (`orbiters_core::features::N4N0_ASSISTANT_ACTIONS`): asked of Orbiters
//! for the member signed in, kept for their identity's epoch only, and held only while the server said yes lately. Anything
//! else (nobody, not known yet, out of reach for a quarter of an hour, a no) is off: those intents are not offered, not done,
//! and the page where the person does it opens instead; what waits to be done (a question, a share for the next capture) is
//! dropped as it goes off.

use super::Controller;
use orbiters_core::features::{Grant, N4N0_ASSISTANT_ACTIONS};
use std::time::{Duration, Instant};

/// How often it is asked again, so that a switch an administrator made reaches a running app.
const ASK_EVERY: Duration = Duration::from_secs(5 * 60);
/// How long a yes holds without another (Orbiters out of reach; verify4-r21 P11).
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
        self.grant.answered(answer, now);
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
        self.assistant_access.lock().unwrap().allowed(self.identity_epoch(), Instant::now())
    }

    /// Asks Orbiters when it is time to, on a thread, with the credential of the member of the epoch the answer is kept for.
    pub(super) fn assistant_access_refresh(self: &super::Shared) {
        self.assistant_access_changed();
        let epoch = self.identity_epoch();
        let Some((api, token)) = self.account_api_of(epoch) else { return };
        {
            let mut access = self.assistant_access.lock().unwrap();
            access.follow(epoch);
            if access.asking || !access.grant.due(Instant::now()) {
                return;
            }
            access.asking = true;
        }
        let ctl = self.clone();
        let started = std::thread::Builder::new().name("assistant-access".into()).spawn(move || {
            let answer = api.member_feature(&token, N4N0_ASSISTANT_ACTIONS);
            ctl.assistant_access.lock().unwrap().answered(epoch, &answer, Instant::now());
            ctl.assistant_access_changed();
        });
        if started.is_err() {
            self.assistant_access.lock().unwrap().asking = false;
        }
    }

    /// It went off: what waits to be done with it is dropped at once. (Outside its lock: N4N0 asks whether it is on.)
    fn assistant_access_changed(&self) {
        let off = self.assistant_access.lock().unwrap().went_off(Instant::now());
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
        a.answered(3, &Err(anyhow::anyhow!("offline")), now);
        assert!(a.allowed(3, now) && a.allowed(3, now + GOOD_FOR - Duration::from_secs(1)));
        assert!(!a.allowed(3, now + GOOD_FOR), "an old yes ends");
        assert!(a.answered(3, &Ok(false), now) && !a.allowed(3, now));
        a.answered(3, &Ok(true), now);
        a.follow(4);
        assert!(!a.allowed(4, now) && !a.allowed(3, now));
        assert!(!a.answered(3, &Ok(true), now) && !a.allowed(4, now), "the last member's answer, late");
    }

    /// Verify4-r21 P11: a failed refresh keeps a yes fifteen minutes, not three hours; a no ends it at once; and what went off is said
    /// once (N4N0 drops what waits).
    #[test]
    fn an_old_yes_ends_after_a_quarter_of_an_hour() {
        let now = Instant::now();
        let minutes = |m: u64| now + Duration::from_secs(m * 60);
        let mut a = AssistantAccess::default();
        a.follow(3);
        a.answered(3, &Ok(true), now);
        assert!(!a.went_off(now));
        assert!(a.grant.due(minutes(5)), "asked again");
        a.answered(3, &Err(anyhow::anyhow!("offline")), minutes(5));
        assert!(a.allowed(3, minutes(14)) && !a.went_off(minutes(14)));
        assert!(!a.allowed(3, minutes(16)), "held on an old yes");
        assert!(a.went_off(minutes(16)) && !a.went_off(minutes(17)), "said once");
        a.answered(3, &Ok(true), minutes(20));
        assert!(!a.went_off(minutes(20)));
        a.answered(3, &Ok(false), minutes(21));
        assert!(!a.allowed(3, minutes(21)) && a.went_off(minutes(21)), "a no, at once");
    }

    /// P11 through the controller: what N4N0 asked of the account goes as the server says no, and as an old yes ends.
    #[test]
    fn what_waits_goes_with_the_yes() {
        for old in [false, true] {
            let ctl = Controller::new(orbiters_core::settings::Settings::default());
            ctl.sign_in_for_test("1");
            ctl.update(|s| {
                s.launcher.source = orbiters_core::launcher::Source::SignedIn { user_id: "usr_a".into(), name: String::new() };
            });
            let then = Instant::now().checked_sub(GOOD_FOR * 2).unwrap_or_else(Instant::now);
            ctl.assistant_actions_for_test(true);
            let stamp = orbiters_n4n0::Stamp { epoch: ctl.identity_epoch(), question: None, surface: orbiters_n4n0::Surface::Window, seq: orbiters_n4n0::next_input() };
            ctl.n4n0_command(crate::ipc::N4n0Command::Ask { text: "put me in orange".into(), stamp: Some(stamp) });
            let asked = || ctl.view().n4n0.confirmations.iter().any(|c| c.is_local());
            assert!(asked(), "{:?}", ctl.view().n4n0.lines);
            if old {
                // (As if the yes had been given long ago and nothing answered since.)
                ctl.assistant_access.lock().unwrap().grant = {
                    let mut g = Grant::new(ASK_EVERY, GOOD_FOR);
                    g.answered(&Ok(true), then);
                    g
                };
                ctl.assistant_access_changed();
            } else {
                ctl.assistant_actions_for_test(false);
            }
            if !old || then + GOOD_FOR <= Instant::now() {
                assert!(!asked(), "old {old}");
            }
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
