//! What a Mac's window does while nobody can see it. eframe draws no window macOS called covered (another app's window
//! over it, another Space, a full-screen game), and macOS does not always say when it shows again: after a full-screen
//! VRChat over it closed or crashed, the window stayed a still picture that took no press (2026-11-22). So while eframe
//! holds it hidden, the window looks for itself, once in a while and when anything wakes it, at the windows macOS lists:
//! shown after all, it is drawn; covered, nothing is drawn at all (a change of state is drawn when it shows again).
//!
//! Until 2026-11-23 it was drawn "at least twice a second" instead, whatever it was, by asking egui for a frame of the
//! hidden window in every pass; but asking for anything of a window asks for a frame too, so that each frame asked for
//! the next one and the window drew at the display's rate for as long as it was open: 22 to 30 % of a core, behind a
//! full-screen VRChat, whose GPU shares its power and heat (docs/mac-performance.md).

use crate::bridge::mac::MacWindow;

/// How often a window held hidden is looked for.
pub const LOOK: f64 = 1.0;
/// The same while a game of the engine is in front: it has the Mac to itself.
pub const LOOK_GAME: f64 = 3.0;
/// Woken by something else (a press, the service's news), it is looked for at most this often.
pub const WOKEN: f64 = 0.25;
/// A window that shows though eframe holds it hidden is drawn at most this often: enough to take a press and to show what
/// changed, until macOS says it shows (a press or a move of the window has it say so).
pub const DRAW: f64 = 0.25;
/// How often a window that shows, behind another app's, asks whether that one is a game.
pub const LOOK_BEHIND: f64 = 2.0;

/// What a look at macOS's windows found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// The window shows, whole or in part.
    pub shown: bool,
    /// A game of the engine is in front of everything.
    pub game: bool,
}

/// What to do after a step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// The window is drawn as anything asks, as everywhere.
    AsAsked,
    /// It is held hidden and shows: this frame is drawn, and the next step is due in so many seconds.
    Draw(f64),
    /// It is held hidden and nothing of it shows: nothing is drawn, and the next step is due in so many seconds.
    Wait(f64),
}

#[derive(Default)]
pub struct Cover {
    looked: Option<(f64, Seen)>,
    drawn: f64,
}

impl Cover {
    /// One pass of the window's logic at `now` (seconds). `hidden`: eframe holds it hidden. `focused`: it has the keyboard.
    /// `look` asks macOS, only when a look is due.
    pub fn step(&mut self, now: f64, hidden: bool, focused: bool, look: impl FnOnce() -> Seen) -> Step {
        if !hidden {
            match self.looked {
                _ if focused => self.looked = None,
                Some((at, _)) if now - at < LOOK_BEHIND => {}
                _ => self.looked = Some((now, look())),
            }
            return Step::AsAsked;
        }
        let game = self.game();
        let due = if game { LOOK } else { WOKEN };
        let seen = match self.looked {
            Some((at, seen)) if now - at < due => seen,
            _ => {
                let seen = look();
                self.looked = Some((now, seen));
                seen
            }
        };
        if !seen.shown && !focused {
            return Step::Wait(if seen.game { LOOK_GAME } else { LOOK });
        }
        if now - self.drawn < DRAW {
            return Step::Wait(DRAW - (now - self.drawn));
        }
        self.drawn = now;
        Step::Draw(DRAW)
    }

    /// Whether a game of the engine was in front at the last look.
    pub fn game(&self) -> bool {
        self.looked.is_some_and(|(_, seen)| seen.game)
    }
}

/// What the windows on screen (front to back) say of the window of `own`; `game` says whether a process is a game's
/// window server.
pub fn seen(windows: &[MacWindow], own: u32, game: impl Fn(u32) -> bool) -> Seen {
    let ordinary = |w: &MacWindow| w.width > 1.0 && w.height > 1.0;
    let front = windows.iter().find(|w| ordinary(w) && (w.layer == 0 || (w.layer > 0 && w.width >= 640.0 && w.height >= 400.0 && game(w.owner))));
    let in_front = front.is_some_and(|w| game(w.owner));
    let Some(place) = windows.iter().enumerate().filter(|(_, w)| w.owner == own && w.layer == 0 && ordinary(w)).max_by(|a, b| (a.1.width * a.1.height).total_cmp(&(b.1.width * b.1.height))).map(|(i, _)| i) else {
        // Not on screen at all: another Space, hidden with its app.
        return Seen { shown: false, game: in_front };
    };
    let w = windows[place];
    let over = |o: &MacWindow| o.x <= w.x && o.y <= w.y && o.x + o.width >= w.x + w.width && o.y + o.height >= w.y + w.height;
    // Only an ordinary window, or a game's, covers: the system's own layers over the whole screen are clear.
    let covered = windows[..place].iter().any(|o| over(o) && (o.layer == 0 || game(o.owner)));
    Seen { shown: !covered, game: in_front }
}

/// [`seen`], asked of macOS now.
#[cfg(target_os = "macos")]
pub fn look() -> Seen {
    let windows = crate::bridge::mac::on_screen();
    seen(&windows, std::process::id(), is_game)
}

/// Whether a process is a game's window server: the engine runs it from an application bundle of its own.
#[cfg(target_os = "macos")]
fn is_game(pid: u32) -> bool {
    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is ours and its size is said.
    let n = unsafe { libc::proc_pidpath(pid as i32, path.as_mut_ptr().cast(), path.len() as u32) };
    n > 0 && path[..n as usize].windows(b"/obelisk-apps/".len()).any(|w| w == b"/obelisk-apps/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const OWN: u32 = 10;
    const GAME: u32 = 20;

    fn window(owner: u32, layer: i64, x: f64, y: f64, width: f64, height: f64) -> MacWindow {
        MacWindow { id: 1, owner, x, y, width, height, layer, on_screen: true }
    }

    fn of(windows: &[MacWindow]) -> Seen {
        seen(windows, OWN, |pid| pid == GAME)
    }

    #[test]
    fn a_window_is_covered_only_by_a_window_over_all_of_it() {
        let own = window(OWN, 0, 100.0, 100.0, 1280.0, 820.0);
        assert!(of(&[own]).shown);
        assert!(of(&[own, window(GAME, 0, 0.0, 0.0, 1470.0, 956.0)]).shown, "a window behind covers nothing");
        assert!(of(&[window(30, 0, 100.0, 100.0, 640.0, 820.0), own]).shown, "half of it shows");
        assert_eq!(of(&[window(30, 0, 0.0, 0.0, 1470.0, 956.0), own]), Seen { shown: false, game: false });
        assert!(of(&[window(30, 25, 0.0, 0.0, 1470.0, 956.0), own]).shown, "the system's clear layers cover nothing");
        assert_eq!(of(&[window(GAME, 0, 0.0, 0.0, 1470.0, 956.0), own]), Seen { shown: false, game: true });
        assert_eq!(of(&[window(GAME, 101, 0.0, 0.0, 1470.0, 956.0), own]), Seen { shown: false, game: true }, "a full-screen game, whatever its layer");
        assert_eq!(of(&[window(GAME, 0, 0.0, 0.0, 1470.0, 956.0)]), Seen { shown: false, game: true }, "on another Space it is not listed");
        assert_eq!(of(&[]), Seen::default());
        assert_eq!(of(&[window(GAME, 0, 0.0, 0.0, 640.0, 400.0), own]), Seen { shown: true, game: true }, "a game in a window beside it");
    }

    /// The defect of 2026-11-22 to 23: whatever wakes it, a window nobody can see is never drawn, and looks no more often
    /// than it says.
    #[test]
    fn a_covered_window_is_never_drawn_whatever_wakes_it() {
        for (game, most) in [(false, 60.0 / LOOK), (true, 60.0 / LOOK_GAME)] {
            let looks = Cell::new(0);
            let mut cover = Cover::default();
            let look = || {
                looks.set(looks.get() + 1);
                Seen { shown: false, game }
            };
            // Only its own timer: a look each time.
            let mut now = 0.0;
            while now < 60.0 {
                let Step::Wait(again) = cover.step(now, true, false, look) else { panic!("drawn") };
                now += again;
            }
            assert!(f64::from(looks.get()) <= most + 1.0, "{} looks", looks.get());
            // Woken two hundred times a second: never drawn, and looked for at most as often as that allows.
            looks.set(0);
            for i in 0..2000 {
                assert!(matches!(cover.step(60.0 + f64::from(i) * 0.005, true, false, look), Step::Wait(_)), "drawn");
            }
            assert!(looks.get() <= if game { 11 } else { 41 }, "{} looks", looks.get());
        }
    }

    /// And what it was there for: a window that shows though macOS never said so is drawn, soon and at a low rate, and takes
    /// a press at once.
    #[test]
    fn a_window_that_shows_again_unsaid_is_drawn() {
        let shown = Cell::new(false);
        let mut cover = Cover::default();
        let look = || Seen { shown: shown.get(), game: false };
        assert_eq!(cover.step(10.0, true, false, look), Step::Wait(LOOK));
        shown.set(true);
        // The game closed: drawn within the second it looks again.
        assert_eq!(cover.step(10.0 + LOOK, true, false, look), Step::Draw(DRAW));
        let mut drawn = 0;
        for i in 1..=1000 {
            drawn += i32::from(matches!(cover.step(11.0 + f64::from(i) * 0.01, true, false, look), Step::Draw(_)));
        }
        assert!((38..=41).contains(&drawn), "{drawn} frames in ten seconds");
        // macOS says it shows: as anywhere.
        assert_eq!(cover.step(30.0, false, false, look), Step::AsAsked);
        // Focused, it is drawn whatever the windows say.
        let mut cover = Cover::default();
        assert_eq!(cover.step(40.0, true, true, || Seen::default()), Step::Draw(DRAW));
    }

    #[test]
    fn a_window_that_shows_asks_for_the_game_now_and_then_and_never_while_it_is_in_front() {
        let looks = Cell::new(0);
        let mut cover = Cover::default();
        let look = || {
            looks.set(looks.get() + 1);
            Seen { shown: true, game: true }
        };
        for i in 0..1000 {
            assert_eq!(cover.step(f64::from(i) * 0.01, false, false, look), Step::AsAsked);
        }
        assert_eq!(looks.get(), 5);
        assert!(cover.game());
        looks.set(0);
        assert_eq!(cover.step(11.0, false, true, look), Step::AsAsked);
        assert!(!cover.game() && looks.get() == 0, "in front itself");
    }
}
