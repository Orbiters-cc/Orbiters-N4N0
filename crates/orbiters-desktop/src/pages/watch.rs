//! Something outside the app that a page shows and that changes on its own: which projects Unity has open, a file the
//! tray service writes, a capture's edits. A [`Watch`] looks at it on a thread of its own, every so often, and asks
//! for a frame only when what it saw differs from the last look: the window draws nothing while nothing changes
//! ("a still interface costs nothing", `orbiters_ui::anim`), where a page that looked by itself had to ask for a
//! frame at every period to do so.
//!
//! Nothing tells a page it left the screen, so the watch works it out from egui's count of frames: the page keeps it
//! ([`Watch::keep`]) on every frame it is drawn, and a frame drawn without that means the page is gone (another page,
//! a closed dialog, a minimized window). No frame at all means the page is still what the window shows, at rest: the
//! looks go on. The thread ends when the page is gone and starts again, with a look at once, when it is back.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

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
}

struct Shared<T> {
    inner: Mutex<Inner<T>>,
    wake: Condvar,
}

impl<T> Shared<T> {
    /// A worker that panicked while it held the lock leaves what it had: the page goes on with the last look.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<T>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
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
}

impl<K, T> Default for Watch<K, T> {
    fn default() -> Self {
        Self { running: None, changes: 0, taken: 0 }
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
                    return inner.seen.clone();
                }
            }
        }
        // Not started, gone with the page, or another key: a new thread, whose first look counts as a change.
        self.stop();
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner { seen: None, changes: self.changes, kept: frame, now: false, stop: false, ended: false }),
            wake: Condvar::new(),
        });
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

fn run<T: PartialEq>(shared: Arc<Shared<T>>, ctx: egui::Context, period: Duration, mut look: impl FnMut() -> T) {
    loop {
        // Frames drawn since the page kept the watch: it is no longer on screen.
        let frame = ctx.cumulative_frame_nr();
        {
            let mut inner = shared.lock();
            if inner.stop || frame > inner.kept + 1 {
                inner.ended = true;
                return;
            }
        }
        let saw = look();
        let mut inner = shared.lock();
        if inner.stop {
            inner.ended = true;
            return;
        }
        if inner.seen.as_ref() != Some(&saw) {
            inner.seen = Some(saw);
            inner.changes += 1;
            orbiters_ui::anim::request_frame(&ctx);
        }
        let (mut inner, _) = shared.wake.wait_timeout_while(inner, period, |inner| !inner.stop && !inner.now).unwrap_or_else(|e| e.into_inner());
        inner.now = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
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
