//! Whether VRChat's picture on a Mac is there (requirement R1 of the 2026-11-23 performance review): three states, so that
//! nothing is ever offered for a healthy game. The window server presents nothing while the window is covered, on another
//! Space or behind the lock screen, and a game stalls for seconds and then draws again (6.1 s on record): neither is a
//! loss. Only the engine's graphics server dropping this very session's game (`destroying Neptune context`,
//! `VTEST_CLIENT_ERROR_CONTEXT_LOST`), not during a quit, and no frame *seen to be missing* for [`CONFIRM_AFTER`] since, is
//! a confirmed loss: the graphics of that run will not come back. A frame after it ends the loss.
//!
//! The tracker ([`Tracker`]) is pure and tested on every computer. One follower (`incident::follow`) feeds it what the
//! engine's log says in the order it says it ([`LogEvent`]) and the frames the session's record counts, each reading with
//! whether it was made at all: a reading that failed is never taken for one that shows no frame. What it finds is one
//! decision per session ([`publish`], [`of`]), which Home (the restart it offers), the VRChat bridge (`renderer_lost`) and
//! the incident capture all use.
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// No frame presented for this long after the graphics server dropped the game: the loss is confirmed.
pub(crate) const CONFIRM_AFTER: f64 = 10.0;
/// A count that grew within this long: the game presents.
pub(crate) const PRESENTING_WITHIN: f64 = 15.0;
/// A game that says its presents this many times more slowly than [`CONFIRM_AFTER`] (a line every 60 frames) is given that
/// long instead, up to this: a slow game is not a lost one.
const CONFIRM_AT_MOST: f64 = 60.0;
/// A count of shown frames read this long after the drop was seen is of after the drop (the engine counts every 2 s).
const SHOWN_LAG: f64 = 3.0;
/// What a follower said of a session this long ago says nothing now.
const STALE_AFTER: Duration = Duration::from_secs(30);

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

/// What the engine's log says of the picture, in the order it says it.
pub(crate) use orbiters_core::obelisk::GraphicsLine as LogEvent;

/// The frames a session's record counts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shown {
    pub frames: u64,
    /// When the engine counted them, in seconds on the clock of [`Look::wall`]; `None` when not known.
    pub at: Option<f64>,
}

/// One look at a session. A reading that failed is `None`: it shows neither a frame nor the absence of one.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Look {
    /// Seconds, monotonic.
    pub at: f64,
    /// Seconds on the clock the engine writes its counts with.
    pub wall: f64,
    pub running: bool,
    /// What the engine's log gained since the last look it could be read.
    pub log: Option<Vec<LogEvent>>,
    /// The frames the session's record says, while it counts them whole.
    pub shown: Option<Shown>,
    pub quitting: bool,
}

/// The loss looked at: dropped, and not yet followed by a frame.
#[derive(Clone, Copy, Debug)]
struct Loss {
    at: f64,
    wall: f64,
    /// The presents lines said after it. One may have been on its way to the log as it came.
    said: u32,
    /// The shown frames as counted after it: only more than these are frames after it.
    shown: Option<u64>,
}

/// One session's picture, look by look.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    presents: Option<u64>,
    shown: Option<u64>,
    grew_at: Option<f64>,
    /// When the presents were last said, and how far apart they were.
    said_at: Option<f64>,
    pace: f64,
    dropped: Option<Loss>,
    lost: bool,
}

impl Tracker {
    /// The picture at this look.
    pub(crate) fn look(&mut self, look: &Look) -> Picture {
        let at = look.at;
        let mut observed = false;
        for event in look.log.iter().flatten() {
            match *event {
                // Said again and again (a renderer that retries) it is one loss, from its first word; after frames, anew.
                LogEvent::Dropped => match &mut self.dropped {
                    Some(drop) => drop.said = 0,
                    None => self.dropped = Some(Loss { at, wall: look.wall, said: 0, shown: None }),
                },
                LogEvent::Presents(count) => {
                    let fresh = self.presents.is_some_and(|before| count < before);
                    let grew = self.presents.is_some_and(|before| count > before);
                    self.presents = Some(count);
                    if grew {
                        self.grew_at = Some(at);
                    }
                    if let Some(drop) = &mut self.dropped {
                        drop.said += 1;
                        // A line may have been on its way to the log as the drop came; a second one, a swapchain made
                        // since (it counts from its own 0), or one of a later look cannot have.
                        if drop.said >= 2 || fresh || at > drop.at {
                            self.grew_at = Some(at);
                            self.dropped = None;
                            self.lost = false;
                        }
                    }
                }
            }
        }
        if let Some(events) = &look.log {
            observed = true;
            if events.iter().any(|e| matches!(e, LogEvent::Presents(_))) {
                if let Some(before) = self.said_at {
                    let lines = events.iter().filter(|e| matches!(e, LogEvent::Presents(_))).count() as f64;
                    self.pace = ((at - before) / lines).clamp(0.0, CONFIRM_AT_MOST);
                }
                self.said_at = Some(at);
            }
        }
        if let Some(shown) = look.shown {
            let grew = self.shown.is_some_and(|before| shown.frames > before);
            self.shown = Some(shown.frames);
            observed = true;
            match &mut self.dropped {
                None if grew => self.grew_at = Some(at),
                None => {}
                // Frames counted before the drop may be told after it: only a count taken after it is compared with.
                Some(drop) => {
                    if shown.at.is_some_and(|counted| counted >= drop.wall + SHOWN_LAG) || (shown.at.is_none() && at > drop.at) {
                        match drop.shown {
                            Some(base) if shown.frames > base => {
                                self.grew_at = Some(at);
                                self.dropped = None;
                                self.lost = false;
                            }
                            _ => drop.shown = Some(shown.frames),
                        }
                    }
                }
            }
        }
        if look.quitting || !look.running {
            // a quit closes the renderer too; a game that is not running has no picture to lose
            self.dropped = None;
            self.lost = false;
            return Picture::Unobservable;
        }
        // Confirmed only by a reading that shows no frame since the drop, never by one that could not be made.
        if let Some(drop) = &self.dropped {
            if observed && at - drop.at >= CONFIRM_AFTER.max(2.0 * self.pace) {
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

/// The session the engine runs now and its picture, as the follower last saw it, and when.
static NOW: Mutex<Option<(String, Picture, Instant)>> = Mutex::new(None);

/// What the follower saw of session `session` (`None`: no session runs).
pub(crate) fn publish(seen: Option<(String, Picture)>) {
    *NOW.lock().unwrap() = seen.map(|(session, picture)| (session, picture, Instant::now()));
}

fn fresh(seen: &Option<(String, Picture, Instant)>, now: Instant) -> Option<(&str, Picture)> {
    let (session, picture, at) = seen.as_ref()?;
    (now.saturating_duration_since(*at) <= STALE_AFTER).then_some((session, *picture))
}

/// The picture of session `session`, the one decision Home, the bridge and the incident capture share: not observable
/// when it is not the session followed, or nothing was seen of it lately.
pub(crate) fn of(session: &str) -> Picture {
    fresh(&NOW.lock().unwrap(), Instant::now()).filter(|(s, _)| *s == session).map_or(Picture::Unobservable, |(_, picture)| picture)
}

/// The session whose picture is lost (confirmed), if the session running now is one.
pub(crate) fn lost() -> Option<String> {
    fresh(&NOW.lock().unwrap(), Instant::now()).filter(|(_, picture)| *picture == Picture::Lost).map(|(session, _)| session.to_owned())
}

/// Held by a test that publishes: what is published is one for the whole process.
#[cfg(test)]
pub(crate) static PUBLISHING: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    impl Tracker {
        /// The counters as they stand at a look (`None`: that one could not be read this time), and whether the log said
        /// the drop after them.
        fn at(&mut self, at: f64, running: bool, presents: Option<u64>, shown: Option<u64>, dropped: bool, quitting: bool) -> Picture {
            let log = presents.map(|count| {
                let said = (self.presents != Some(count)).then_some(LogEvent::Presents(count));
                said.into_iter().chain(dropped.then_some(LogEvent::Dropped)).collect()
            });
            let log = log.or(dropped.then(|| vec![LogEvent::Dropped]));
            self.look(&Look { at, wall: at, running, log, shown: shown.map(|frames| Shown { frames, at: Some(at) }), quitting })
        }
    }

    /// R1: a covered window or the lock screen (nothing presented, nothing dropped) is never a loss; a stall followed by
    /// frames (6.1 s on record) is none; a drop with frames after it is none.
    #[test]
    fn nothing_is_lost_without_a_drop_and_no_frame_after_it() {
        let mut t = Tracker::default();
        assert_eq!(t.at(0.0, true, Some(10), Some(0), false, false), Picture::Unobservable, "a first count proves nothing");
        assert_eq!(t.at(5.0, true, Some(70), Some(0), false, false), Picture::Presenting, "presents grow behind a lock screen");
        for at in [25.0, 60.0, 300.0] {
            assert_eq!(t.at(at, true, Some(70), Some(0), false, false), Picture::Unobservable, "stalled or covered: no loss");
        }
        assert_eq!(t.at(306.0, true, Some(130), Some(0), false, false), Picture::Presenting, "back after a stall");
        // the graphics server dropped a context, the game made another one and presents: no loss
        assert_eq!(t.at(310.0, true, Some(130), None, true, false), Picture::Presenting);
        assert_eq!(t.at(315.0, true, Some(5), None, false, false), Picture::Presenting, "a new swapchain counts from its own 0");
        assert_eq!(t.at(320.0, true, Some(65), None, false, false), Picture::Presenting);
        assert_eq!(t.at(340.0, true, Some(65), None, false, false), Picture::Unobservable);
    }

    /// R1: dropped, and no frame for 10 s since: confirmed; a frame after it ends the loss; a quit is no loss; a missing
    /// reading changes nothing.
    #[test]
    fn a_drop_without_frames_after_it_is_a_confirmed_loss() {
        let mut t = Tracker::default();
        t.at(0.0, true, Some(10), None, false, false);
        t.at(5.0, true, Some(70), None, false, false);
        assert_eq!(t.at(10.0, true, Some(70), None, true, false), Picture::Presenting, "not confirmed yet");
        assert_eq!(t.at(15.0, true, None, None, false, false), Picture::Presenting, "a reading missing");
        assert_eq!(t.at(20.0, true, Some(70), None, false, false), Picture::Lost, "10 s without a frame since the drop");
        assert_eq!(t.at(60.0, true, Some(70), None, false, false), Picture::Lost);
        assert_eq!(t.at(65.0, true, Some(71), None, false, false), Picture::Presenting, "it presents again");
        // a quit closes the renderer: never a loss
        let mut t = Tracker::default();
        t.at(0.0, true, Some(10), None, false, false);
        assert_eq!(t.at(5.0, true, Some(10), None, true, true), Picture::Unobservable);
        assert_eq!(t.at(30.0, true, Some(10), None, false, true), Picture::Unobservable);
        // a game that ended has nothing to restart
        let mut t = Tracker::default();
        t.at(0.0, true, Some(10), None, true, false);
        assert_eq!(t.at(20.0, true, Some(10), None, false, false), Picture::Lost);
        assert_eq!(t.at(25.0, false, Some(10), None, false, false), Picture::Unobservable);
    }

    #[test]
    fn only_a_confirmed_loss_names_its_session() {
        let _publishing = PUBLISHING.lock().unwrap_or_else(|e| e.into_inner());
        publish(Some(("a".into(), Picture::Unobservable)));
        assert_eq!(lost(), None);
        publish(Some(("a".into(), Picture::Lost)));
        assert_eq!(lost().as_deref(), Some("a"));
        publish(None);
        assert_eq!(lost(), None);
    }

    fn log(at: f64, events: &[LogEvent]) -> Look {
        Look { at, wall: at, running: true, log: Some(events.to_vec()), ..Default::default() }
    }
    fn unread(at: f64) -> Look {
        Look { at, wall: at, running: true, ..Default::default() }
    }
    use LogEvent::{Dropped, Presents};

    /// Fifth verification, finding 7: after a drop, 15 s of readings that all failed is no loss; the first one that
    /// succeeds and shows no frame since is.
    #[test]
    fn readings_that_failed_confirm_nothing() {
        let mut t = Tracker::default();
        t.look(&log(0.0, &[Presents(60)]));
        t.look(&log(5.0, &[Presents(120)]));
        t.look(&log(10.0, &[Dropped]));
        for at in [15.0, 20.0, 25.0, 130.0] {
            assert_ne!(t.look(&unread(at)), Picture::Lost, "nothing could be read at {at}");
        }
        // Frames in what was missed: no loss at all.
        let mut came_back = Tracker::default();
        came_back.look(&log(0.0, &[Presents(60)]));
        came_back.look(&log(5.0, &[Presents(120), Dropped]));
        came_back.look(&unread(20.0));
        assert_eq!(came_back.look(&log(25.0, &[Presents(180)])), Picture::Presenting);
        assert_ne!(came_back.look(&log(80.0, &[])), Picture::Lost);
        assert_eq!(t.look(&log(135.0, &[])), Picture::Lost, "read at last: no frame since");
    }

    /// Finding 7: growth and a drop in the same look, the growth before the drop in the log: 120 s without a frame is a
    /// loss (it was never one). The other order is none.
    #[test]
    fn the_order_of_the_log_tells_frames_before_a_drop_from_frames_after_it() {
        let mut t = Tracker::default();
        t.look(&log(0.0, &[Presents(60)]));
        assert_eq!(t.look(&log(5.0, &[Presents(120), Presents(180), Dropped])), Picture::Presenting);
        assert_eq!(t.look(&log(15.0, &[])), Picture::Lost);
        assert_eq!(t.look(&log(125.0, &[])), Picture::Lost);
        let mut t = Tracker::default();
        t.look(&log(0.0, &[Presents(60)]));
        t.look(&log(5.0, &[Dropped, Presents(120), Presents(180)]));
        for at in [15.0, 125.0] {
            assert_ne!(t.look(&log(at, &[])), Picture::Lost, "it drew after the drop");
        }
        // A new swapchain after the drop counts from its own 0: one line of it is a frame after the drop.
        let mut t = Tracker::default();
        t.look(&log(0.0, &[Presents(6000)]));
        t.look(&log(5.0, &[Dropped, Presents(60)]));
        assert_ne!(t.look(&log(20.0, &[])), Picture::Lost);
        // Dropped, drawing again, dropped again: the last one counts.
        let mut t = Tracker::default();
        t.look(&log(0.0, &[Presents(60)]));
        t.look(&log(5.0, &[Dropped, Presents(10), Presents(70), Dropped]));
        assert_eq!(t.look(&log(16.0, &[])), Picture::Lost);
        // One line of the same swapchain right after the drop may have been on its way: not proof of a frame after it.
        let mut t = Tracker::default();
        t.look(&log(0.0, &[Presents(60)]));
        t.look(&log(5.0, &[Dropped, Presents(120)]));
        assert_eq!(t.look(&log(16.0, &[])), Picture::Lost);
        assert_eq!(t.look(&log(21.0, &[Presents(180)])), Picture::Presenting, "a later one is");
    }

    /// Finding 7: the record's count lags the log: frames shown before the drop and counted after it end no loss; frames
    /// counted after it do.
    #[test]
    fn frames_counted_late_are_not_frames_after_the_drop() {
        let shown = |at: f64, frames: u64, counted: f64| Look { at, wall: at, running: true, log: Some(Vec::new()), shown: Some(Shown { frames, at: Some(counted) }), ..Default::default() };
        let mut t = Tracker::default();
        t.look(&shown(0.0, 100, 0.0));
        t.look(&shown(5.0, 160, 5.0));
        t.look(&Look { log: Some(vec![Dropped]), ..shown(10.0, 160, 6.0) });
        t.look(&shown(15.0, 190, 11.0));
        assert_eq!(t.look(&shown(20.0, 190, 19.0)), Picture::Lost);
        assert_eq!(t.look(&shown(25.0, 190, 24.0)), Picture::Lost);
        assert_eq!(t.look(&shown(30.0, 191, 29.0)), Picture::Presenting, "one more than was counted after the drop");
        // The same, its first count after the drop already the last one: no growth to wait for.
        let mut t = Tracker::default();
        t.look(&Look { log: Some(vec![Dropped]), ..shown(0.0, 10, 0.0) });
        t.look(&shown(5.0, 10, 5.0));
        assert_eq!(t.look(&shown(10.0, 10, 10.0)), Picture::Lost);
    }

    /// A renderer that retries says its loss without end: it is one loss, confirmed from its first word.
    #[test]
    fn a_loss_said_again_and_again_is_still_confirmed() {
        let mut t = Tracker::default();
        t.look(&log(0.0, &[Presents(60)]));
        for at in [5.0, 10.0] {
            assert_ne!(t.look(&log(at, &[Dropped, Dropped])), Picture::Lost);
        }
        assert_eq!(t.look(&log(15.0, &[Dropped])), Picture::Lost);
        assert_eq!(t.look(&log(20.0, &[Dropped])), Picture::Lost);
    }

    /// A slow game says its presents every 60 frames: at 5 a second, every 12 s. Drawing again that slowly after a drop, or
    /// not said yet, is no loss within twice its pace.
    #[test]
    fn a_slow_game_is_given_its_own_pace() {
        let mut t = Tracker::default();
        for k in 0..5 {
            t.look(&log(f64::from(k) * 15.0, &[Presents(u64::from(k as u32) * 60 + 60)]));
        }
        t.look(&log(65.0, &[Dropped]));
        assert_ne!(t.look(&log(76.0, &[])), Picture::Lost, "11 s: less than its own pace");
        assert_ne!(t.look(&log(90.0, &[])), Picture::Lost);
        assert_eq!(t.look(&log(96.0, &[])), Picture::Lost);
    }

    /// A quit the supervisor accepted closes the renderer: no loss, whoever asks (Home, the bridge, the incidents).
    #[test]
    fn a_quit_accepted_before_or_after_the_drop_is_no_loss() {
        let mut t = Tracker::default();
        t.look(&log(0.0, &[Presents(60)]));
        t.look(&log(5.0, &[Dropped]));
        assert_eq!(t.look(&Look { quitting: true, ..log(20.0, &[]) }), Picture::Unobservable);
        let mut t = Tracker::default();
        t.look(&log(0.0, &[Presents(60)]));
        assert_eq!(t.look(&Look { quitting: true, ..log(5.0, &[Dropped]) }), Picture::Unobservable);
        assert_eq!(t.look(&Look { quitting: true, ..log(60.0, &[]) }), Picture::Unobservable);
    }

    /// The decision is the session's: another session, or one nothing was seen of lately, is not lost.
    #[test]
    fn a_decision_is_its_session_s_alone_and_goes_stale() {
        let now = Instant::now();
        let seen = Some(("a".to_string(), Picture::Lost, now));
        assert_eq!(fresh(&seen, now), Some(("a", Picture::Lost)));
        assert_eq!(fresh(&seen, now + STALE_AFTER * 2), None, "a follower that stopped decides nothing");
        assert_eq!(of("never followed"), Picture::Unobservable);
    }
}
