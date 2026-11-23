//! Whether VRChat's picture on a Mac is there (requirement R1 of the 2026-11-23 performance review): three states, so that
//! nothing is ever offered for a healthy game. The window server presents nothing while the window is covered, on another
//! Space or behind the lock screen, and a game stalls for seconds and then draws again (6.1 s on record): neither is a
//! loss. Only the engine's graphics server dropping this very session's game (`destroying Neptune context`,
//! `VTEST_CLIENT_ERROR_CONTEXT_LOST`), not during a quit, and no frame presented for [`CONFIRM_AFTER`] since, is a confirmed
//! loss: the graphics of that run will not come back. A present after it ends the loss.
//!
//! The tracker ([`Tracker`]) is pure and tested on every computer; the incident follower feeds it on a Mac, and Home asks
//! [`lost`] whether its own session lost its picture (it then offers to start VRChat again, checked again on the click).
use std::sync::Mutex;

/// No frame presented for this long after the graphics server dropped the game: the loss is confirmed.
pub(crate) const CONFIRM_AFTER: f64 = 10.0;
/// A count that grew within this long: the game presents.
pub(crate) const PRESENTING_WITHIN: f64 = 15.0;

/// What the picture of one session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Picture {
    /// Frames are being presented (the guest's presents or the shown frames grew lately).
    Presenting,
    /// Nothing seen lately (a covered window, the lock screen, a stall, loading, a count not read yet): nothing is said
    /// or offered.
    Unobservable,
    /// The graphics went for this run of the game: confirmed.
    Lost,
}

/// One session's picture, look by look.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    presents: Option<u64>,
    shown: Option<u64>,
    grew_at: Option<f64>,
    lost_at: Option<f64>,
    lost: bool,
}

/// A counter grew from its last valid reading (the reading kept): a missing one changes nothing; a smaller one is a
/// counter that began again (each swapchain counts its own presents): its new base, not a frame.
fn advanced(last: &mut Option<u64>, now: Option<u64>) -> bool {
    let Some(now) = now else { return false };
    let grew = last.is_some_and(|before| now > before);
    *last = Some(now);
    grew
}

impl Tracker {
    /// The picture at `at` (seconds, monotonic): the session still runs (`running`), its guest's presents and its shown
    /// frames as read now (`None`: not read this time), whether the engine's log said since the last look that the
    /// graphics server dropped it, and whether the game is quitting.
    pub(crate) fn look(&mut self, at: f64, running: bool, presents: Option<u64>, shown: Option<u64>, dropped: bool, quitting: bool) -> Picture {
        let a = advanced(&mut self.presents, presents);
        let b = advanced(&mut self.shown, shown);
        if a || b {
            self.grew_at = Some(at);
            // drawing again: whatever was dropped came back
            self.lost_at = None;
            self.lost = false;
        }
        if quitting || !running {
            // a quit closes the renderer too; a game that is not running has no picture to lose
            self.lost_at = None;
            self.lost = false;
            return Picture::Unobservable;
        }
        if dropped && self.lost_at.is_none() && !self.lost {
            self.lost_at = Some(at);
        }
        if let Some(since) = self.lost_at {
            if at - since >= CONFIRM_AFTER && self.grew_at.is_none_or(|grew| grew < since) {
                self.lost = true;
            }
        }
        if self.lost {
            Picture::Lost
        } else if self.grew_at.is_some_and(|grew| at - grew <= PRESENTING_WITHIN) {
            Picture::Presenting
        } else {
            Picture::Unobservable
        }
    }
}

/// The picture of the session the engine runs now, as last seen: its Game session UUID and state.
static NOW: Mutex<Option<(String, Picture)>> = Mutex::new(None);

/// What the follower saw of session `session` (`None`: no session runs).
pub(crate) fn publish(seen: Option<(String, Picture)>) {
    *NOW.lock().unwrap() = seen;
}

/// The session whose picture is lost (confirmed), if the session running now is one.
pub(crate) fn lost() -> Option<String> {
    NOW.lock().unwrap().as_ref().filter(|(_, picture)| *picture == Picture::Lost).map(|(session, _)| session.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R1: a covered window or the lock screen (nothing presented, nothing dropped) is never a loss; a stall followed by
    /// frames (6.1 s on record) is none; a drop with frames after it is none.
    #[test]
    fn nothing_is_lost_without_a_drop_and_no_frame_after_it() {
        let mut t = Tracker::default();
        assert_eq!(t.look(0.0, true, Some(10), Some(0), false, false), Picture::Unobservable, "a first count proves nothing");
        assert_eq!(t.look(5.0, true, Some(70), Some(0), false, false), Picture::Presenting, "presents grow behind a lock screen");
        for at in [25.0, 60.0, 300.0] {
            assert_eq!(t.look(at, true, Some(70), Some(0), false, false), Picture::Unobservable, "stalled or covered: no loss");
        }
        assert_eq!(t.look(306.0, true, Some(130), Some(0), false, false), Picture::Presenting, "back after a stall");
        // the graphics server dropped a context, the game made another one and presents: no loss
        assert_eq!(t.look(310.0, true, Some(130), None, true, false), Picture::Presenting);
        assert_eq!(t.look(315.0, true, Some(5), None, false, false), Picture::Presenting, "a new swapchain counts from its own 0");
        assert_eq!(t.look(320.0, true, Some(65), None, false, false), Picture::Presenting);
        assert_eq!(t.look(340.0, true, Some(65), None, false, false), Picture::Unobservable);
    }

    /// R1: dropped, and no frame for 10 s since: confirmed; a frame after it ends the loss; a quit is no loss; a missing
    /// reading changes nothing.
    #[test]
    fn a_drop_without_frames_after_it_is_a_confirmed_loss() {
        let mut t = Tracker::default();
        t.look(0.0, true, Some(10), None, false, false);
        t.look(5.0, true, Some(70), None, false, false);
        assert_eq!(t.look(10.0, true, Some(70), None, true, false), Picture::Presenting, "not confirmed yet");
        assert_eq!(t.look(15.0, true, None, None, false, false), Picture::Presenting, "a reading missing");
        assert_eq!(t.look(20.0, true, Some(70), None, false, false), Picture::Lost, "10 s without a frame since the drop");
        assert_eq!(t.look(60.0, true, Some(70), None, false, false), Picture::Lost);
        assert_eq!(t.look(65.0, true, Some(71), None, false, false), Picture::Presenting, "it presents again");
        // a quit closes the renderer: never a loss
        let mut t = Tracker::default();
        t.look(0.0, true, Some(10), None, false, false);
        assert_eq!(t.look(5.0, true, Some(10), None, true, true), Picture::Unobservable);
        assert_eq!(t.look(30.0, true, Some(10), None, false, true), Picture::Unobservable);
        // a game that ended has nothing to restart
        let mut t = Tracker::default();
        t.look(0.0, true, Some(10), None, true, false);
        assert_eq!(t.look(20.0, true, Some(10), None, false, false), Picture::Lost);
        assert_eq!(t.look(25.0, false, Some(10), None, false, false), Picture::Unobservable);
    }

    #[test]
    fn only_a_confirmed_loss_names_its_session() {
        publish(Some(("a".into(), Picture::Unobservable)));
        assert_eq!(lost(), None);
        publish(Some(("a".into(), Picture::Lost)));
        assert_eq!(lost().as_deref(), Some("a"));
        publish(None);
        assert_eq!(lost(), None);
    }
}
