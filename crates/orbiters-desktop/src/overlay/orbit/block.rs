//! Taking a hand's trigger from VRChat while it points at one of the panels ("so we don't have interaction conflicts between the
//! two menus"), in VR. On the desktop nothing like it is needed: the panel's own window takes the click (measured with the real game: a
//! click on a topmost no-activate window over a Quick Menu button did not reach the button, and VRChat stayed in front).
//!
//! **SteamVR's means, as openvr.h has them:** an overlay application makes an action set active at a priority between
//! `k_nActionSetOverlayGlobalPriorityMin` and `Max`, "to cause any inputs bound to a source used by that action set to be disabled in
//! scene applications". So a set that binds each hand's trigger, made active for that hand (`ulRestrictedToDevice`) only while its
//! laser is on a panel: VRChat's trigger actions on that source read nothing, its own laser hovers on but cannot press, and the other hand
//! keeps its trigger. It is "experimental" in the header and needs SteamVR's developer setting "Experimental overlay input overrides"
//! (`steamvr.globalActionSetPriority`); it is on on Enzo's computer, the overlay turns it on where it is not and Settings says so. Reading the
//! trigger while blocked goes through that same set's own actions, since the block takes it from this application's lower sets too.
//!
//! **The rules** ([`Block::step`]):
//! - Blocked on hover, before any press: a press never reaches VRChat and us both.
//! - A trigger already held when the laser arrives stays VRChat's until let go: its press must not be cut in the middle.
//! - A press begun on a panel stays blocked until let go, wherever the hand has gone: VRChat must not see half of it.
//! - Off the panel with the trigger up, free at once. The panels gone: free at once, whatever is held.
//! - **Nothing can stay blocked.** What is active is said anew every frame, so a closing overlay frees it by not saying it, and SteamVR forgets
//!   it with the process. The one case left is this process hanging with a block on: [`Stuck`] ends it after [`STUCK_AFTER`].

use orbiters_core::app_log::OrLog as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One hand this turn: its laser on a panel, its trigger held.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HandNow {
    pub on_panel: bool,
    pub trigger: bool,
}

/// Which hands' triggers are blocked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Block {
    blocked: [bool; 2],
}

impl Block {
    pub fn blocked(&self) -> [bool; 2] {
        self.blocked
    }

    /// A turn: `shown` says the panels are up. Returns which hands' triggers VRChat must not get from now on.
    pub fn step(&mut self, hands: [HandNow; 2], shown: bool) -> [bool; 2] {
        for (blocked, hand) in self.blocked.iter_mut().zip(hands) {
            *blocked = shown && if *blocked { hand.on_panel || hand.trigger } else { hand.on_panel && !hand.trigger };
        }
        self.blocked
    }

    /// Everything free now (the panels closed, the overlay going).
    pub fn release(&mut self) {
        self.blocked = [false; 2];
    }
}

/// How long the overlay's loop may be silent with a block on before the process ends itself. Its turns take 11 ms; a turn loading a page has
/// taken 200 ms at worst.
pub const STUCK_AFTER: Duration = Duration::from_millis(1500);

/// Whether a block has been left with nothing to end it: on, and the loop silent for too long.
pub fn stuck(blocked: bool, silent: Duration) -> bool {
    blocked && silent >= STUCK_AFTER
}

/// Watches the overlay's loop while a block is on: should it hang, the process ends, and SteamVR frees the trigger with it (the service starts a
/// new overlay). The rest of the time it costs a sleeping thread.
#[derive(Clone)]
pub struct Stuck {
    blocked: Arc<AtomicBool>,
    beat_ms: Arc<AtomicU64>,
    started: Instant,
}

impl Stuck {
    pub fn start() -> Self {
        let this = Self { blocked: Arc::new(AtomicBool::new(false)), beat_ms: Arc::new(AtomicU64::new(0)), started: Instant::now() };
        let watch = this.clone();
        std::thread::Builder::new().name("orbit-block-watch".into()).spawn(move || loop {
            std::thread::sleep(Duration::from_millis(250));
            let silent = Duration::from_millis((watch.started.elapsed().as_millis() as u64).saturating_sub(watch.beat_ms.load(Ordering::Relaxed)));
            if stuck(watch.blocked.load(Ordering::Relaxed), silent) {
                log_info!("the overlay's loop stopped with the trigger blocked: ending it frees the trigger");
                std::process::exit(1);
            }
        }).or_log("a thread could not be started");
        this
    }

    /// A turn of the loop, and whether a block is on now.
    pub fn beat(&self, blocked: bool) {
        self.beat_ms.store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
        self.blocked.store(blocked, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AWAY: HandNow = HandNow { on_panel: false, trigger: false };
    const ON: HandNow = HandNow { on_panel: true, trigger: false };
    const ON_PRESSED: HandNow = HandNow { on_panel: true, trigger: true };
    const AWAY_PRESSED: HandNow = HandNow { on_panel: false, trigger: true };

    #[test]
    fn it_blocks_on_hover_and_frees_on_leaving() {
        let mut b = Block::default();
        assert_eq!(b.step([AWAY, AWAY], true), [false, false]);
        assert_eq!(b.step([AWAY, ON], true), [false, true], "only the hand pointing");
        assert_eq!(b.step([ON, ON], true), [true, true]);
        assert_eq!(b.step([AWAY, ON], true), [false, true]);
        assert_eq!(b.step([AWAY, AWAY], true), [false, false], "free at once");
    }

    #[test]
    fn a_press_begun_on_a_panel_stays_ours_until_let_go() {
        let mut b = Block::default();
        b.step([ON, AWAY], true);
        assert_eq!(b.step([ON_PRESSED, AWAY], true), [true, false]);
        assert_eq!(b.step([AWAY_PRESSED, AWAY], true), [true, false], "dragged off, still held: VRChat must not see half a press");
        assert_eq!(b.step([AWAY, AWAY], true), [false, false]);
    }

    #[test]
    fn a_press_begun_in_the_game_stays_the_game_s() {
        let mut b = Block::default();
        assert_eq!(b.step([AWAY_PRESSED, AWAY], true), [false, false]);
        assert_eq!(b.step([ON_PRESSED, AWAY], true), [false, false], "the laser arrives with the trigger held");
        assert_eq!(b.step([ON_PRESSED, AWAY], true), [false, false]);
        assert_eq!(b.step([ON, AWAY], true), [true, false], "let go on the panel: from now on ours");
    }

    #[test]
    fn panels_closing_or_the_overlay_going_free_it_whatever_is_held() {
        let mut b = Block::default();
        b.step([ON, ON], true);
        b.step([ON_PRESSED, ON], true);
        assert_eq!(b.step([ON_PRESSED, ON], false), [false, false], "the panels closed");
        assert_eq!(b.step([ON_PRESSED, ON], false), [false, false], "and they stay free while closed");
        b.step([ON, ON], true);
        assert_eq!(b.blocked(), [true, true]);
        b.release();
        assert_eq!(b.blocked(), [false, false], "the overlay going");
    }

    #[test]
    fn a_hung_loop_with_a_block_ends_the_process_and_only_then() {
        assert!(!stuck(false, Duration::from_secs(600)), "no block: nothing to free, however long");
        assert!(!stuck(true, Duration::from_millis(200)), "a slow turn");
        assert!(stuck(true, STUCK_AFTER));
    }
}
