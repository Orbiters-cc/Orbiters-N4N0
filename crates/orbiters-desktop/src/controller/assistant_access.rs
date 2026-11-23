//! The switch of what N4N0's assistant does from an account (`orbiters_core::features::N4N0_ASSISTANT_ACTIONS`): asked of Orbiters
//! for the member signed in, kept for their identity's epoch only, and held only while the server said yes lately. Anything
//! else (nobody, not known yet, out of reach for long, a no) is off: those intents are not offered, not done, and the page
//! where the person does it opens instead.

use super::Controller;
use orbiters_core::features::{Grant, N4N0_ASSISTANT_ACTIONS};
use std::time::{Duration, Instant};

/// How often it is asked again, so that a switch an administrator made reaches a running app.
const ASK_EVERY: Duration = Duration::from_secs(15 * 60);
/// How long a yes holds without another (Orbiters out of reach).
const GOOD_FOR: Duration = orbiters_core::features::REFRESH;

pub(super) struct AssistantAccess {
    /// Whose answer it is.
    epoch: u64,
    grant: Grant,
    asking: bool,
}

impl Default for AssistantAccess {
    fn default() -> Self {
        Self { epoch: 0, grant: Grant::new(ASK_EVERY, GOOD_FOR), asking: false }
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
}

impl Controller {
    /// Whether the member signed in now has the account's actions.
    pub(super) fn assistant_actions(&self) -> bool {
        self.assistant_access.lock().unwrap().allowed(self.identity_epoch(), Instant::now())
    }

    /// Asks Orbiters when it is time to, on a thread, with the credential of the member of the epoch the answer is kept for.
    pub(super) fn assistant_access_refresh(self: &super::Shared) {
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
        });
        if started.is_err() {
            self.assistant_access.lock().unwrap().asking = false;
        }
    }

    /// Tests: what Orbiters answered for the member signed in now.
    #[cfg(test)]
    pub(crate) fn assistant_actions_for_test(&self, allowed: bool) {
        let epoch = self.identity_epoch();
        let mut access = self.assistant_access.lock().unwrap();
        access.follow(epoch);
        access.answered(epoch, &Ok(allowed), Instant::now());
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
