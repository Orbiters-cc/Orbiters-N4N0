//! What a Mac's window does while nobody can see it. eframe draws no window macOS called covered (another app's window
//! over it, another Space, a full-screen game), and macOS does not always say when it shows again: after a full-screen
//! VRChat over it closed or crashed, the window stayed a still picture that took no press (2026-11-22). So while eframe
//! holds it hidden, the window looks for itself, once in a while and when anything wakes it, at the windows macOS lists:
//! shown after all, it is drawn. Certainly hidden (it is not on screen, or a full-screen game is over it), nothing is drawn.
//! Where it cannot be told (macOS lists no windows, or the one over it may be clear in places: what a window holds is not
//! listed), it is drawn once in [`UNSURE`]. Whatever may have it show ([`watch`]), and any change of what it shows of the
//! service's state ([`Cover::wake`]), is drawn at once, once, however it is held.
//!
//! Until 2026-11-23 it was drawn "at least twice a second" instead, whatever it was, by asking egui for a frame of the
//! hidden window in every pass; but asking for anything of a window asks for a frame too, so that each frame asked for
//! the next one and the window drew at the display's rate for as long as it was open: 22 to 30 % of a core, behind a
//! full-screen VRChat, whose GPU shares its power and heat (docs/mac-performance.md).

use crate::bridge::mac::{Frame, MacWindow};

/// How often a window held hidden is looked for.
pub const LOOK: f64 = 1.0;
/// The same while a game of the engine is in front: it has the Mac to itself.
pub const LOOK_GAME: f64 = 3.0;
/// Woken by something else (a press, the service's news), it is looked for at most this often.
pub const WOKEN: f64 = 0.25;
/// A window that shows though eframe holds it hidden is drawn at most this often: enough to take a press and to show what
/// changed, until macOS says it shows (a press or a move of the window has it say so).
pub const DRAW: f64 = 0.25;
/// One that may show or not is drawn this often.
pub const UNSURE: f64 = 1.0;
/// How often a window that shows, behind another app's, asks whether that one is a game.
pub const LOOK_BEHIND: f64 = 2.0;

/// What a look at macOS's windows found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    /// The window shows, whole or in part; `None` when that cannot be told.
    pub shown: Option<bool>,
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
    drawn: Option<f64>,
    focused: bool,
    /// A frame is due whatever the windows say.
    owed: bool,
}

impl Cover {
    /// Something happened that may have it show, or that it is to show: looked for anew, and drawn once whatever is found.
    pub fn wake(&mut self) {
        self.owed = true;
        self.looked = None;
    }

    /// Whether the window is in the Dock, from what eframe holds (`said`) and what macOS says (`native`), and what to tell
    /// eframe where they differ: on a Mac it never asks again, so that a window put back from the Dock by macOS alone (its
    /// picture there pressed) would stay a hidden one to it.
    pub fn minimized(&mut self, said: bool, native: Option<bool>) -> (bool, Option<bool>) {
        let Some(native) = native.filter(|native| *native != said) else { return (said, None) };
        self.wake();
        (native, Some(native))
    }

    /// One pass of the window's logic at `now` (seconds). `hidden`: eframe holds it hidden. `focused`: it has the keyboard.
    /// `look` asks macOS, only when a look is due.
    pub fn step(&mut self, now: f64, hidden: bool, focused: bool, look: impl FnOnce() -> Seen) -> Step {
        if focused && !std::mem::replace(&mut self.focused, focused) {
            self.wake();
        }
        self.focused = focused;
        if !hidden {
            self.owed = false;
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
        let every = match seen.shown {
            _ if focused || self.owed => DRAW,
            Some(true) => DRAW,
            None => UNSURE,
            Some(false) => return Step::Wait(if seen.game { LOOK_GAME } else { LOOK }),
        };
        if let Some(left) = self.drawn.map(|drawn| every - (now - drawn)).filter(|left| *left > 0.0) {
            return Step::Wait(left);
        }
        self.drawn = Some(now);
        self.owed = false;
        Step::Draw(every)
    }

    /// Whether a game of the engine was in front at the last look.
    pub fn game(&self) -> bool {
        self.looked.is_some_and(|(_, seen)| seen.game)
    }
}

/// What the windows on screen (front to back; `None` when they could not all be listed) say of the window `id` of `own`
/// (its largest when it is not known which); `display` is the frame of the display a window is on, and `game` says whether
/// a process is a game's window server.
pub fn seen(windows: Option<&[MacWindow]>, own: u32, id: Option<u32>, display: impl Fn(&MacWindow) -> Option<Frame>, game: impl Fn(u32) -> bool) -> Seen {
    // (the menu bar is a window: no window at all is no list)
    let Some(windows) = windows.filter(|windows| !windows.is_empty()) else { return Seen::default() };
    let ordinary = |w: &MacWindow| w.width > 1.0 && w.height > 1.0;
    let front = windows.iter().find(|w| ordinary(w) && (w.layer == 0 || (w.layer > 0 && w.width >= 640.0 && w.height >= 400.0 && game(w.owner))));
    let in_front = front.is_some_and(|w| game(w.owner));
    let place = match id {
        Some(id) => windows.iter().position(|w| w.owner == own && w.id == id),
        None => windows.iter().enumerate().filter(|(_, w)| w.owner == own && w.layer == 0 && ordinary(w)).max_by(|a, b| (a.1.width * a.1.height).total_cmp(&(b.1.width * b.1.height))).map(|(i, _)| i),
    };
    let Some(place) = place else {
        // Not on screen at all: another Space, hidden with its app.
        return Seen { shown: id.is_some().then_some(false), game: in_front };
    };
    let w = windows[place];
    let over = |o: &MacWindow| o.x <= w.x && o.y <= w.y && o.x + o.width >= w.x + w.width && o.y + o.height >= w.y + w.height;
    // Only an ordinary window, or a game's, covers: the system's own layers over the whole screen are clear. One known to be
    // clear covers nothing; a full-screen game's is opaque; any other may be clear in places, as far as macOS lists it.
    let over_it = || windows[..place].iter().filter(|o| over(o) && (o.layer == 0 || game(o.owner)) && o.alpha.is_none_or(|alpha| alpha >= 1.0));
    let full_screen = |o: &MacWindow| game(o.owner) && o.alpha.is_some() && crate::bridge::mac::whole_display(o, display(&w));
    let shown = if over_it().any(full_screen) { Some(false) } else { over_it().next().is_none().then_some(true) };
    Seen { shown, game: in_front }
}

/// [`seen`] of the window `id`, asked of macOS now.
#[cfg(target_os = "macos")]
pub fn look(id: Option<u32>) -> Seen {
    use crate::bridge::mac;
    seen(mac::on_screen().as_deref(), std::process::id(), id, mac::display_frame, is_game)
}

/// What macOS says of the window itself.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Default)]
pub struct Native {
    /// Its number among macOS's windows.
    pub id: Option<u32>,
    pub minimized: Option<bool>,
}

/// [`Native`] of the window of `frame`, on its thread.
#[cfg(target_os = "macos")]
pub fn native(frame: &eframe::Frame) -> Native {
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    let Ok(RawWindowHandle::AppKit(handle)) = frame.window_handle().map(|handle| handle.as_raw()) else { return Native::default() };
    // SAFETY: the window's own view, which lives for as long as the frame's handle; asked on the window's thread, where eframe
    // runs its logic.
    let view: &objc2_app_kit::NSView = unsafe { handle.ns_view.cast().as_ref() };
    let Some(window) = view.window() else { return Native::default() };
    Native { id: u32::try_from(window.windowNumber()).ok().filter(|id| *id > 0), minimized: Some(window.isMiniaturized()) }
}

#[cfg(target_os = "macos")]
pub use watch::{watch, woken};

/// What macOS says that may have the window show: it became the one in front, came back from the Dock or with its app, the
/// Space changed, the displays woke or changed.
#[cfg(target_os = "macos")]
mod watch {
    use objc2::rc::Retained;
    use objc2::runtime::NSObject;
    use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass};
    use objc2_app_kit as app;
    use objc2_foundation::{NSNotification, NSNotificationCenter, NSNotificationName};
    use std::sync::atomic::{AtomicBool, Ordering};

    static WOKEN: AtomicBool = AtomicBool::new(false);

    struct Ivars {
        ctx: egui::Context,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "OrbitersWindowWatch"]
        #[ivars = Ivars]
        struct Watch;

        impl Watch {
            #[unsafe(method(woken:))]
            fn woken(&self, _: &NSNotification) {
                WOKEN.store(true, Ordering::Relaxed);
                self.ivars().ctx.request_repaint();
            }
        }
    );

    /// Whether any of it was said since the last time this was asked.
    pub fn woken() -> bool {
        WOKEN.swap(false, Ordering::Relaxed)
    }

    /// Where each is said, and by which name.
    pub(super) fn said() -> Vec<(Retained<NSNotificationCenter>, &'static NSNotificationName)> {
        let centre = NSNotificationCenter::defaultCenter;
        let workspace = || app::NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: AppKit's own constants.
        unsafe {
            vec![
                (centre(), app::NSWindowDidBecomeKeyNotification),
                (centre(), app::NSWindowDidDeminiaturizeNotification),
                (centre(), app::NSWindowDidChangeScreenNotification),
                (centre(), app::NSApplicationDidUnhideNotification),
                (centre(), app::NSApplicationDidChangeScreenParametersNotification),
                (workspace(), app::NSWorkspaceActiveSpaceDidChangeNotification),
                (workspace(), app::NSWorkspaceScreensDidWakeNotification),
            ]
        }
    }

    /// Wakes the window of `ctx` at each, for as long as the process runs.
    pub fn watch(ctx: &egui::Context) {
        let this = Watch::alloc().set_ivars(Ivars { ctx: ctx.clone() });
        // SAFETY: NSObject's init, on a freshly allocated object.
        let this: Retained<Watch> = unsafe { msg_send![super(this), init] };
        for (centre, name) in said() {
            // SAFETY: the method is this class's own and takes the notification.
            unsafe { centre.addObserver_selector_name_object(&this, sel!(woken:), Some(name), None) };
        }
        // Notification centres keep no observer alive.
        std::mem::forget(this);
    }
}

/// Whether a process is a game's window server: the engine runs it from an application bundle of its own.
#[cfg(target_os = "macos")]
fn is_game(pid: u32) -> bool {
    let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is ours and its size is said.
    let n = unsafe { libc::proc_pidpath(pid as i32, path.as_mut_ptr().cast(), path.len() as u32) };
    n > 0 && path[..n as usize].windows(b"/obelisk-apps/".len()).any(|w| w == b"/obelisk-apps/")
}

/// Held by a test that has the window woken, or asks whether it was: that is one for the whole process.
#[cfg(all(test, target_os = "macos"))]
pub(crate) static WATCHING: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const OWN: u32 = 10;
    const GAME: u32 = 20;

    fn window(owner: u32, layer: i64, x: f64, y: f64, width: f64, height: f64) -> MacWindow {
        MacWindow { id: owner, owner, x, y, width, height, layer, on_screen: true, alpha: Some(1.0) }
    }

    fn of(windows: &[MacWindow]) -> Seen {
        seen(Some(windows), OWN, Some(OWN), |_| Some((0.0, 0.0, 1470.0, 956.0)), |pid| pid == GAME)
    }

    #[test]
    fn a_window_is_covered_only_by_a_window_over_all_of_it() {
        let own = window(OWN, 0, 100.0, 100.0, 1280.0, 820.0);
        let game = window(GAME, 0, 0.0, 0.0, 1470.0, 956.0);
        assert_eq!(of(&[own]).shown, Some(true));
        assert_eq!(of(&[own, game]).shown, Some(true), "a window behind covers nothing");
        assert_eq!(of(&[window(30, 0, 100.0, 100.0, 640.0, 820.0), own]).shown, Some(true), "half of it shows");
        assert_eq!(of(&[window(30, 25, 0.0, 0.0, 1470.0, 956.0), own]).shown, Some(true), "the system's clear layers cover nothing");
        assert_eq!(of(&[game, own]), Seen { shown: Some(false), game: true });
        assert_eq!(of(&[window(GAME, 101, 0.0, 0.0, 1470.0, 956.0), own]), Seen { shown: Some(false), game: true }, "a full-screen game, whatever its layer");
        assert_eq!(of(&[game]), Seen { shown: Some(false), game: true }, "on another Space it is not listed");
        assert_eq!(of(&[window(GAME, 0, 0.0, 0.0, 640.0, 400.0), own]), Seen { shown: Some(true), game: true }, "a game in a window beside it");
    }

    /// Ninth verification: what cannot be told is not hidden: no list, an ordinary window over it (what it holds may be clear), a
    /// game's in a window of its own, one whose opacity is not said. One known to be clear covers nothing, a game's either; a
    /// full-screen game under any of them still covers.
    #[test]
    fn what_cannot_be_told_is_not_hidden() {
        let own = window(OWN, 0, 100.0, 100.0, 1280.0, 820.0);
        let (other, game) = (window(30, 0, 0.0, 0.0, 1470.0, 956.0), window(GAME, 0, 0.0, 0.0, 1470.0, 956.0));
        let clear = |w: MacWindow, alpha: Option<f64>| MacWindow { alpha, ..w };
        let display = |_: &MacWindow| Some((0.0, 0.0, 1470.0, 956.0));
        assert_eq!(seen(None, OWN, Some(OWN), display, |pid| pid == GAME), Seen { shown: None, game: false }, "the windows could not be listed");
        assert_eq!(of(&[]), Seen { shown: None, game: false }, "nor is an empty list one");
        assert_eq!(of(&[other, own]).shown, None);
        assert_eq!(of(&[clear(other, None), own]).shown, None);
        assert_eq!(of(&[clear(game, None), own]).shown, None);
        assert_eq!(of(&[window(GAME, 0, 90.0, 60.0, 1300.0, 880.0), own]).shown, None, "a game's window has a frame");
        for alpha in [0.0, 0.5, 0.99] {
            assert_eq!(of(&[clear(other, Some(alpha)), own]).shown, Some(true), "{alpha}");
            assert_eq!(of(&[clear(game, Some(alpha)), own]).shown, Some(true), "{alpha}");
        }
        assert_eq!(of(&[clear(other, Some(0.5)), other, own]).shown, None);
        assert_eq!(of(&[clear(other, Some(0.5)), other, game, own]).shown, Some(false));
        assert_eq!(seen(Some(&[game, own]), OWN, Some(OWN), |_| None, |pid| pid == GAME).shown, None, "its display is not known");
        // Its own window by its number, not its largest one: the other may be anywhere.
        let larger = MacWindow { id: 99, ..window(OWN, 0, 0.0, 0.0, 1470.0, 900.0) };
        assert_eq!(of(&[larger, game, own]).shown, Some(false));
        assert_eq!(of(&[larger, game]).shown, Some(false));
        assert_eq!(seen(Some(&[game]), OWN, None, display, |pid| pid == GAME).shown, None, "not known which, none is known to be hidden");
        assert_eq!(seen(Some(&[own]), OWN, None, display, |pid| pid == GAME).shown, Some(true));
    }

    /// Ninth verification: a window that may show is drawn once a second whatever else wakes it, and never left undrawn; one
    /// certainly hidden is drawn once, at once, for whatever may have it show or is to be shown, and no more.
    #[test]
    fn a_window_that_may_show_is_drawn_now_and_then_and_what_wakes_it_at_once() {
        let frames = |cover: &mut Cover, from: f64, seen: Seen, focused: bool| (0..=1000).filter(|i| matches!(cover.step(from + f64::from(*i) * 0.01, true, focused, || seen), Step::Draw(_))).count();
        let mut cover = Cover::default();
        assert!((10..=11).contains(&frames(&mut cover, 10.0, Seen::default(), false)), "once a second");
        // Left alone, its own timer alone does it.
        let (mut now, mut drawn) = (30.0, 0);
        while now < 40.0 {
            match cover.step(now, true, false, Seen::default) {
                Step::Draw(again) => {
                    drawn += 1;
                    now += again;
                }
                Step::Wait(again) => now += again,
                Step::AsAsked => unreachable!(),
            }
        }
        assert!((9..=11).contains(&drawn), "{drawn} frames in ten seconds");
        for (from, game) in [(50.0, false), (150.0, true)] {
            let hidden = Seen { shown: Some(false), game };
            assert_eq!(frames(&mut cover, from, hidden, false), 0);
            for again in 1..=3 {
                cover.wake();
                assert_eq!(frames(&mut cover, from + 20.0 * f64::from(again), hidden, false), 1);
            }
        }
        // Woken without end (the state changing at every pass), at most four times a second.
        let mut drawn = 0;
        for i in 0..1000 {
            cover.wake();
            drawn += i32::from(matches!(cover.step(300.0 + f64::from(i) * 0.01, true, false, || Seen { shown: Some(false), game: true }), Step::Draw(_)));
        }
        assert!((38..=41).contains(&drawn), "{drawn}");
        // It takes the keyboard: at once, whatever was seen a moment ago.
        let mut cover = Cover::default();
        assert_eq!(cover.step(10.0, true, false, || Seen { shown: Some(false), game: false }), Step::Wait(LOOK));
        assert!(matches!(cover.step(10.01, true, true, || Seen { shown: Some(false), game: false }), Step::Draw(_)));
    }

    /// Ninth verification: put back from the Dock by macOS alone, eframe is told and the window drawn at once, however it was
    /// held; put there by macOS alone, alike.
    #[test]
    fn a_window_put_back_from_the_dock_is_known_to_be() {
        let hidden = || Seen { shown: Some(false), game: false };
        let mut cover = Cover::default();
        assert_eq!(cover.minimized(true, Some(true)), (true, None));
        assert_eq!(cover.step(10.0, true, false, hidden), Step::Wait(LOOK));
        assert_eq!(cover.minimized(true, Some(false)), (false, Some(false)));
        assert!(matches!(cover.step(10.01, true, false, hidden), Step::Draw(_)));
        assert_eq!(cover.minimized(false, Some(false)), (false, None));
        assert_eq!(cover.step(10.5, true, false, hidden), Step::Wait(LOOK));
        assert_eq!(cover.minimized(false, Some(true)), (true, Some(true)));
        assert_eq!(cover.minimized(true, None), (true, None), "macOS does not say: as eframe has it");
    }

    /// Each thing macOS says that may have it show wakes it.
    #[test]
    #[cfg(target_os = "macos")]
    fn what_macos_says_wakes_the_window() {
        let _watching = WATCHING.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        watch(&ctx);
        for (centre, name) in super::watch::said() {
            let _ = woken();
            // SAFETY: a notification of this process's own, to its own observer.
            unsafe { centre.postNotificationName_object(name, None) };
            assert!(woken(), "{name}");
            assert!(!woken(), "told once");
        }
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
                Seen { shown: Some(false), game }
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
        let look = || Seen { shown: Some(shown.get()), game: false };
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
            Seen { shown: Some(true), game: true }
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
