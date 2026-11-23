//! World feedback from N4N0's menu (contracts `Docs/n4n0.md`, Menu, from screen version 13; `Docs/world-feedback.md`): the
//! app keeps the worlds' feedback and sends what the menus ask ([`crate::Host::world_feedback`], the queue the N4N0
//! world's lines go through too), and hands N4N0 what it knows ([`crate::N4n0::set_feedback`]). Every menu shows the
//! same (the status' `feedback`, the screen's menu directly).

use crate::runtime::Shared;
use orbiters_n4n0_core::feedback::{FeedbackAsk, FeedbackView};

impl crate::N4n0 {
    /// The worlds' feedback as the app knows it now, the player's actions not sent yet included: the screen's menu shows
    /// it, the other menus through the status.
    pub fn set_feedback(self: &Shared, view: FeedbackView) {
        {
            let mut rt = self.n4n0.lock().unwrap();
            if rt.feedback == view {
                return;
            }
            rt.feedback = view.clone();
            rt.screen.menu.set_feedback(view);
            rt.wake();
        }
        self.publish();
    }

    /// What a menu asked (read a world's feedback, like, label, comment), on a thread: the app answers with
    /// [`crate::N4n0::set_feedback`].
    pub(crate) fn feedback_ask(self: &Shared, ask: FeedbackAsk) {
        let later = self.later();
        std::thread::spawn(move || {
            if let Some(fence) = later.fence() {
                fence.host.world_feedback(ask);
            }
        });
    }
}
