//! Something outside the app that a page shows and that changes on its own: which projects Unity has open, a file the
//! tray service writes, a capture's edits. A [`Watch`] looks at it on a thread of its own, every so often, and asks
//! for a frame only when what it saw differs from the last look: the window draws nothing while nothing changes
//! ("a still interface costs nothing", `orbiters_ui::anim`), where a page that looked by itself had to ask for a
//! frame at every period to do so.
//!
//! Nothing tells a page it left the screen, so the watch works it out from egui's count of frames: the page keeps it
//! ([`Watch::keep`]) on every frame it is drawn, and a frame drawn without that means the page is gone (another page,
//! a closed dialog). No frame at all means the page is still what the window shows, at rest: the looks go on. The
//! thread ends when the page is gone and starts again, with a look at once, when it is back.
//!
//! A window that is not shown (minimized, or hidden by the system) is another matter: its integration draws no frame
//! at all while it is so (eframe runs only the app's logic then, `App::logic`), which the count of frames cannot tell
//! from a window at rest. The app's logic says it ([`follow_window`]): every watch of that window ends at once, what
//! its look held is let go (Settings' look keeps the microphones open), and each starts again, with a look, at the
//! first frame drawn once the window shows. Not the focus: a window in view behind another goes on showing changes.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

/// The watches' threads alive in this process ([`live`]).
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// How many watches look now, over every window of the process: what `--gpu-bench --watches` prints each second, to
/// see in the real window that they end when it is minimized and start again when it shows.
pub fn live() -> usize {
    LIVE.load(Ordering::Relaxed)
}

/// A window as its watches know it: whether it is shown, and the threads to wake when it no longer is.
#[derive(Default)]
struct Window {
    hidden: AtomicBool,
    watches: Mutex<Vec<Weak<dyn Sleeper>>>,
}

/// A watch's thread, asleep until its next look, that can be woken to see that it has to end.
trait Sleeper: Send + Sync {
    fn wake(&self);
}

/// The window `ctx` draws, kept with the context: a process may draw several (the idle count runs one a page).
fn window(ctx: &egui::Context) -> Arc<Window> {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<Window>>(egui::Id::unique("orbiters-page-watch-window")).clone())
}

/// Tells the watches whether the window is shown, from what the integration says of it now (minimized, hidden by the
/// system). Call at every pass of the app's logic, which runs whether a frame is drawn or not.
pub fn follow_window(ctx: &egui::Context) {
    window_shown(ctx, ctx.input(|i| i.viewport().visible()) != Some(false));
}

/// The window is shown, or no longer: then every watch's thread ends now, not at its next look.
fn window_shown(ctx: &egui::Context, shown: bool) {
    let window = window(ctx);
    if window.hidden.swap(!shown, Ordering::AcqRel) == !shown || shown {
        return;
    }
    let mut watches = window.watches.lock().unwrap_or_else(|e| e.into_inner());
    watches.retain(|watch| match watch.upgrade() {
        Some(watch) => {
            watch.wake();
            true
        }
        None => false,
    });
}

struct Inner<T> {
    /// What the last look saw, and how many times a look differed from the one before (the first counts).
    seen: Option<T>,
    changes: u64,
    /// The frame the page last kept the watch in (`egui::Context::cumulative_frame_nr`).
    kept: u64,
    /// A look asked for at once ([`Watch::look_now`]).
    now: bool,
    /// The watch was dropped, or goes on with another thread.
    stop: bool,
    /// The thread ended.
    ended: bool,
    /// It ended on a panic of its look.
    panicked: bool,
}

/// A look that panics is started again at the next frame drawn, so many times in a row; then the watch is given up
/// until the page asks for another key, with a line in the log, not started and lost again at every frame.
const PANICS_MOST: u8 = 3;

struct Shared<T> {
    inner: Mutex<Inner<T>>,
    wake: Condvar,
    /// The window the watch is drawn in.
    window: Arc<Window>,
}

impl<T> Shared<T> {
    /// A worker that panicked while it held the lock leaves what it had: the page goes on with the last look.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<T>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether the thread has to end: the watch was dropped or replaced, or its window is no longer shown.
    fn over(&self, inner: &Inner<T>) -> bool {
        inner.stop || self.window.hidden.load(Ordering::Acquire)
    }
}

impl<T: Send> Sleeper for Shared<T> {
    fn wake(&self) {
        // With the lock: the thread is either before its check of `over`, or waiting.
        let _held = self.lock();
        self.wake.notify_all();
    }
}

/// A look repeated on a thread while the page that keeps it is on screen. `K` says what is looked at (a project's
/// folder, the projects listed): another key is another watch.
pub struct Watch<K, T> {
    running: Option<(K, Arc<Shared<T>>)>,
    /// Changes counted by the threads before this one, so that the count never goes back.
    changes: u64,
    /// The change the page took last ([`Watch::keep`]).
    taken: u64,
    /// Threads in a row that ended on a panic of their look, with no look given to the page since.
    panics: u8,
}

impl<K, T> Default for Watch<K, T> {
    fn default() -> Self {
        Self { running: None, changes: 0, taken: 0, panics: 0 }
    }
}

impl<K, T> Drop for Watch<K, T> {
    fn drop(&mut self) {
        self.stop();
    }
}

impl<K, T> Watch<K, T> {
    /// Ends the looks (the next [`Watch::keep`] starts them again, and gives the page its first look as a new one).
    pub fn stop(&mut self) {
        if let Some((_, shared)) = self.running.take() {
            let mut inner = shared.lock();
            self.changes = self.changes.max(inner.changes);
            inner.stop = true;
            shared.wake.notify_all();
        }
    }

    /// Looks again now instead of at the next period (something the page did changes what a look sees).
    pub fn look_now(&self) {
        if let Some((_, shared)) = &self.running {
            shared.lock().now = true;
            shared.wake.notify_all();
        }
    }
}

impl<K: PartialEq + Clone, T: Clone + PartialEq + Send + 'static> Watch<K, T> {
    /// Call on every frame the page shows what is watched: `make` gives the look (run on the watch's thread, every
    /// `period`) when the watch starts, for `key`. Returns what the last look saw when the page has not had it yet (a
    /// look that differed from the one before, or the first of a watch just started), once.
    pub fn keep<L: FnMut() -> T + Send + 'static>(&mut self, ctx: &egui::Context, key: &K, period: Duration, make: impl FnOnce() -> L) -> Option<T> {
        let frame = ctx.cumulative_frame_nr();
        if let Some((running, shared)) = &self.running {
            if running == key {
                let mut inner = shared.lock();
                if !inner.ended {
                    inner.kept = frame;
                    if inner.changes == self.taken {
                        return None;
                    }
                    self.taken = inner.changes;
                    self.panics = 0;
                    return inner.seen.clone();
                }
                if std::mem::take(&mut inner.panicked) {
                    self.panics += 1;
                    if self.panics == PANICS_MOST {
                        eprintln!("a page's watch is given up: its look panicked {PANICS_MOST} times in a row");
                    }
                }
                if self.panics >= PANICS_MOST {
                    return None;
                }
            } else {
                self.panics = 0;
            }
        }
        // Not started, gone with the page or the window, or another key: a new thread, whose first look counts as a
        // change.
        self.stop();
        let window = window(ctx);
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner { seen: None, changes: self.changes, kept: frame, now: false, stop: false, ended: false, panicked: false }),
            wake: Condvar::new(),
            window: window.clone(),
        });
        {
            let mut watches = window.watches.lock().unwrap_or_else(|e| e.into_inner());
            watches.retain(|watch| watch.strong_count() > 0);
            let sleeper: Arc<dyn Sleeper> = shared.clone();
            watches.push(Arc::downgrade(&sleeper));
        }
        let (theirs, ctx, look) = (shared.clone(), ctx.clone(), make());
        if std::thread::Builder::new().name("page-watch".into()).spawn(move || run(theirs, ctx, period, look)).is_err() {
            // No thread to be had: tried again at the next frame drawn.
            shared.lock().ended = true;
        }
        self.running = Some((key.clone(), shared));
        None
    }

    /// What the last look saw, taken already or not (`None` until the first look of a watch just started).
    pub fn seen(&self) -> Option<T> {
        self.running.as_ref().and_then(|(_, shared)| shared.lock().seen.clone())
    }
}

/// Says the thread ended, however it ends: a look that panicked left the watch looking alive (`ended` was set only
/// where the thread returned), and no frame ever started it again, on this visit of the page or a later one.
struct Ended<'a, T>(&'a Shared<T>);

impl<T> Drop for Ended<'_, T> {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::Relaxed);
        let mut inner = self.0.lock();
        inner.ended = true;
        inner.panicked = std::thread::panicking();
    }
}

fn run<T: PartialEq>(shared: Arc<Shared<T>>, ctx: egui::Context, period: Duration, mut look: impl FnMut() -> T) {
    LIVE.fetch_add(1, Ordering::Relaxed);
    let _ended = Ended(&shared);
    loop {
        // Frames drawn since the page kept the watch: it is no longer on screen. Or the window is not.
        let frame = ctx.cumulative_frame_nr();
        {
            let inner = shared.lock();
            if shared.over(&inner) || frame > inner.kept + 1 {
                return;
            }
        }
        let saw = look();
        let mut inner = shared.lock();
        if shared.over(&inner) {
            return;
        }
        if inner.seen.as_ref() != Some(&saw) {
            inner.seen = Some(saw);
            inner.changes += 1;
            orbiters_ui::anim::request_frame(&ctx);
        }
        let (mut inner, _) = shared.wake.wait_timeout_while(inner, period, |inner| !shared.over(inner) && !inner.now).unwrap_or_else(|e| e.into_inner());
        inner.now = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::time::Instant;

    /// One frame of a page that keeps `watch` (or not), at `t`.
    fn frame(ctx: &egui::Context, t: f64, watch: Option<(&mut Watch<u8, u32>, &Arc<AtomicU32>, &Arc<AtomicU32>)>) -> Option<u32> {
        let mut seen = None;
        let mut watch = watch;
        let mut out = ctx.run_ui(egui::RawInput { time: Some(t), ..Default::default() }, |ui| {
            if let Some((watch, value, looks)) = watch.as_mut() {
                seen = watch.keep(ui.ctx(), &0, Duration::from_millis(5), || {
                    let (value, looks) = (value.clone(), looks.clone());
                    move || {
                        looks.fetch_add(1, Ordering::SeqCst);
                        value.load(Ordering::SeqCst)
                    }
                });
            }
        });
        out.textures_delta.clear();
        seen
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < until, "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn it_asks_for_a_frame_only_when_the_look_changed() {
        let ctx = egui::Context::default();
        let (value, looks) = (Arc::new(AtomicU32::new(7)), Arc::new(AtomicU32::new(0)));
        let mut watch = Watch::default();
        // The first look is given to the page once, on the frame it asked for.
        let mut t = 0.0;
        let mut first = None;
        wait_for("the first look", || {
            t += 0.01;
            first = frame(&ctx, t, Some((&mut watch, &value, &looks)));
            first.is_some()
        });
        assert_eq!(first, Some(7));
        // egui's own first frames ask for more: let them pass, keeping the watch.
        for _ in 0..5 {
            t += 0.01;
            assert_eq!(frame(&ctx, t, Some((&mut watch, &value, &looks))), None, "taken once");
        }
        assert_eq!(watch.seen(), Some(7), "and still there to read");
        // Many looks that see the same: no frame asked for, though the page is still the one on screen.
        let before = looks.load(Ordering::SeqCst);
        wait_for("more looks", || looks.load(Ordering::SeqCst) >= before + 5);
        assert!(!ctx.has_requested_repaint(), "the same look asks for nothing");
        // It changes: one frame, and the page is given the new look.
        value.store(9, Ordering::SeqCst);
        wait_for("a frame for the change", || ctx.has_requested_repaint());
        t += 0.01;
        assert_eq!(frame(&ctx, t, Some((&mut watch, &value, &looks))), Some(9));
    }

    #[test]
    fn it_stops_looking_when_the_page_is_gone_and_looks_at_once_when_it_is_back() {
        let ctx = egui::Context::default();
        let (value, looks) = (Arc::new(AtomicU32::new(1)), Arc::new(AtomicU32::new(0)));
        let mut watch = Watch::default();
        frame(&ctx, 0.01, Some((&mut watch, &value, &looks)));
        wait_for("the first look", || looks.load(Ordering::SeqCst) > 0);
        assert_eq!(frame(&ctx, 0.02, Some((&mut watch, &value, &looks))), Some(1));
        // Frames without the page: its thread ends, and looks no more.
        frame(&ctx, 0.03, None);
        frame(&ctx, 0.04, None);
        let shared = watch.running.as_ref().unwrap().1.clone();
        wait_for("the thread to end", || shared.lock().ended);
        let ended_at = looks.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(looks.load(Ordering::SeqCst), ended_at, "no look while the page is not shown");
        // Back: nothing to show for a moment, then a look of its own, counted as a change so the page takes it in.
        value.store(2, Ordering::SeqCst);
        assert_eq!(frame(&ctx, 0.05, Some((&mut watch, &value, &looks))), None);
        wait_for("a look on return", || looks.load(Ordering::SeqCst) > ended_at);
        wait_for("its frame", || ctx.has_requested_repaint());
        assert_eq!(frame(&ctx, 0.06, Some((&mut watch, &value, &looks))), Some(2));
        // Dropped: the thread ends.
        let shared = watch.running.as_ref().unwrap().1.clone();
        drop(watch);
        wait_for("the thread to end with its watch", || shared.lock().ended);
    }

    /// What a look holds while it runs (Settings' holds the microphones open): counted when it is let go.
    struct Lease(Arc<AtomicU32>);

    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A pass of the app's logic alone, as eframe runs it for a window that is not shown (`minimized`) and before
    /// every frame of one that is: no frame is drawn, and egui's count of frames does not move.
    fn logic_pass(ctx: &egui::Context, t: f64, minimized: bool) {
        let mut input = egui::RawInput { time: Some(t), ..Default::default() };
        input.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { minimized: Some(minimized), occluded: Some(false), ..Default::default() });
        let _ = ctx.run_logic(&input, follow_window);
    }

    /// Minimizing the window ended no watch: eframe draws no frame for a window that is not shown, so no frame was
    /// ever drawn "without the page", and Settings' watch kept the microphones open for as long as the window stayed
    /// minimized. The app's logic, which eframe still runs then, tells the watches.
    #[test]
    fn a_window_no_longer_shown_ends_its_watches_and_they_look_again_once_it_shows() {
        let ctx = egui::Context::default();
        let (looks, released) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let (others, others_released) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        // Two watches of one page: the second looks rarely, and is asleep when the window goes. Each look holds a
        // lease made when its thread starts.
        let mut watch: Watch<(), u32> = Watch::default();
        let mut other: Watch<(), u32> = Watch::default();
        let mut t = 0.0;
        let mut frame = |watch: &mut Watch<(), u32>, other: &mut Watch<(), u32>| -> (Option<u32>, Option<u32>) {
            t += 0.01;
            logic_pass(&ctx, t, false);
            let mut seen = (None, None);
            let mut out = ctx.run_ui(egui::RawInput { time: Some(t), ..Default::default() }, |ui| {
                seen.0 = watch.keep(ui.ctx(), &(), Duration::from_millis(5), || {
                    let (looks, lease) = (looks.clone(), Lease(released.clone()));
                    move || {
                        let _held = &lease;
                        looks.fetch_add(1, Ordering::SeqCst)
                    }
                });
                seen.1 = other.keep(ui.ctx(), &(), Duration::from_secs(3600), || {
                    let (looks, lease) = (others.clone(), Lease(others_released.clone()));
                    move || {
                        let _held = &lease;
                        looks.fetch_add(1, Ordering::SeqCst)
                    }
                });
            });
            out.textures_delta.clear();
            seen
        };
        let let_go = || (released.load(Ordering::SeqCst), others_released.load(Ordering::SeqCst));
        // Shown: both look, the page at rest, the leases held.
        frame(&mut watch, &mut other);
        wait_for("both to look", || looks.load(Ordering::SeqCst) > 0 && others.load(Ordering::SeqCst) == 1);
        frame(&mut watch, &mut other);
        assert_eq!(let_go(), (0, 0), "the looks hold their leases");
        // Minimized: a pass of the logic and no frame. The looks stop and both let go of what they held, the one
        // asleep for an hour too.
        logic_pass(&ctx, 1.0, true);
        wait_for("both leases to be let go", || let_go() == (1, 1));
        let stopped_at = looks.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(looks.load(Ordering::SeqCst), stopped_at, "no look while the window is not shown");
        assert_eq!(others.load(Ordering::SeqCst), 1);
        // More passes of the logic while minimized change nothing.
        logic_pass(&ctx, 1.1, true);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!((looks.load(Ordering::SeqCst), let_go()), (stopped_at, (1, 1)));
        // Shown again: the first frame starts both anew, and each gives the page a new look.
        assert_eq!(frame(&mut watch, &mut other), (None, None), "starting again");
        wait_for("both to look again", || looks.load(Ordering::SeqCst) > stopped_at && others.load(Ordering::SeqCst) == 2);
        wait_for("a frame for the new looks", || ctx.has_requested_repaint());
        let seen = frame(&mut watch, &mut other);
        assert!(seen.0.is_some_and(|n| n >= stopped_at) && seen.1 == Some(1), "{seen:?}");
        assert_eq!(let_go(), (1, 1), "and hold new leases");
    }

    /// A window in view behind another (not focused) is shown: its watches go on.
    #[test]
    fn a_window_without_the_focus_keeps_its_watches() {
        let ctx = egui::Context::default();
        let looks = Arc::new(AtomicU32::new(0));
        let mut watch: Watch<(), u32> = Watch::default();
        let mut input = egui::RawInput { time: Some(0.01), focused: false, ..Default::default() };
        input.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { minimized: Some(false), occluded: Some(false), focused: Some(false), ..Default::default() });
        let _ = ctx.run_logic(&input, follow_window);
        let mut out = ctx.run_ui(input.clone(), |ui| {
            let looks = looks.clone();
            watch.keep(ui.ctx(), &(), Duration::from_millis(5), move || move || looks.fetch_add(1, Ordering::SeqCst));
        });
        out.textures_delta.clear();
        let _ = ctx.run_logic(&input, follow_window);
        wait_for("looks without the focus", || looks.load(Ordering::SeqCst) >= 5);
    }

    /// A look that panicked left its watch stranded: the thread was gone but never said so, and no frame started
    /// another, on this visit or the next.
    #[test]
    fn a_look_that_panics_is_started_again_and_given_up_when_it_keeps_panicking() {
        let ctx = egui::Context::default();
        let looks = Arc::new(AtomicU32::new(0));
        // How many of the first looks panic.
        let failing = Arc::new(AtomicU32::new(1));
        let mut watch: Watch<(), u32> = Watch::default();
        let mut t = 0.0;
        let mut frame = |watch: &mut Watch<(), u32>| -> Option<u32> {
            t += 0.01;
            let mut seen = None;
            let mut out = ctx.run_ui(egui::RawInput { time: Some(t), ..Default::default() }, |ui| {
                seen = watch.keep(ui.ctx(), &(), Duration::from_millis(5), || {
                    let (looks, failing) = (looks.clone(), failing.clone());
                    move || {
                        let n = looks.fetch_add(1, Ordering::SeqCst);
                        assert!(n >= failing.load(Ordering::SeqCst), "a look that fails (expected in this test)");
                        n
                    }
                });
            });
            out.textures_delta.clear();
            seen
        };
        let ended = |watch: &Watch<(), u32>| watch.running.as_ref().is_some_and(|(_, shared)| shared.lock().ended);
        // The first look panics: its thread says it ended, and the next frame starts another, whose look is given.
        frame(&mut watch);
        wait_for("the panicked thread to say it ended", || ended(&watch));
        assert_eq!(frame(&mut watch), None, "started again");
        let mut seen = None;
        wait_for("a look after the panic", || {
            seen = frame(&mut watch);
            seen.is_some()
        });
        assert!(seen.is_some_and(|n| n >= 1));
        // A look that always panics is started so many times, then left alone: no thread a frame for ever.
        let mut always: Watch<(), u32> = Watch::default();
        failing.store(u32::MAX, Ordering::SeqCst);
        let before = looks.load(Ordering::SeqCst);
        for _ in 0..PANICS_MOST {
            frame(&mut always);
            wait_for("the panicked thread to say it ended", || ended(&always));
        }
        for _ in 0..5 {
            assert_eq!(frame(&mut always), None);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(looks.load(Ordering::SeqCst) - before, u32::from(PANICS_MOST), "given up after {PANICS_MOST} panics in a row");
    }

    #[test]
    fn a_look_asked_for_does_not_wait_for_the_period() {
        let ctx = egui::Context::default();
        let looks = Arc::new(AtomicU32::new(0));
        let mut watch: Watch<(), u32> = Watch::default();
        let keep = |watch: &mut Watch<(), u32>, t: f64| {
            let mut out = ctx.run_ui(egui::RawInput { time: Some(t), ..Default::default() }, |ui| {
                let looks = looks.clone();
                watch.keep(ui.ctx(), &(), Duration::from_secs(3600), move || move || looks.fetch_add(1, Ordering::SeqCst));
            });
            out.textures_delta.clear();
        };
        keep(&mut watch, 0.01);
        wait_for("the first look", || looks.load(Ordering::SeqCst) == 1);
        keep(&mut watch, 0.02);
        watch.look_now();
        wait_for("the look asked for", || looks.load(Ordering::SeqCst) == 2);
    }
}
