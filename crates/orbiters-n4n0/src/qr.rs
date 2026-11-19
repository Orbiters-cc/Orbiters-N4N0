//! N4N0's QR code scanner (its menu, More; contracts `Docs/n4n0.md`, Menu): while a menu shows the scanner's page, the
//! app reads VRChat's stream camera ([`crate::Host::qr_scan`]) and N4N0 opens the web link of a QR code it sees in the
//! computer's browser. Which codes open, how often the same link does and when scanning stops are
//! [`orbiters_n4n0_core::qr`]'s; every menu shows the same scanner (the status' `qr`, the screen's menu directly).

use crate::platform::{QrScan, QrSeen};
use crate::runtime::{N4n0Runtime, Shared};
use crate::status::NoticeKind;
use orbiters_n4n0_core::qr::{self, Opening, Pace, QrPhase, QrView, Recent};
use orbiters_i18n::{mark, tr};
use orbiters_n4n0_core::text::latin;
use crate::MenuHolder;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

/// The scan running, and what the menus show of it.
#[derive(Default)]
pub(crate) struct QrRuntime {
    pub(crate) view: QrView,
    pub(crate) run: Option<Run>,
    /// The number of the last scan started: what an older one still says is not heard.
    number: u64,
    /// The menus that show the scanner's page (MENU-13): a menu's stop ends the scan only once none holds it.
    holders: HashSet<MenuHolder>,
}

pub(crate) struct Run {
    /// Dropped, it stops.
    scan: Box<dyn QrScan>,
    number: u64,
    /// It stops at this time unless a link opens before (each one gives it [`qr::SCAN_FOR`] again).
    until: Instant,
    recent: Recent,
    /// The pages it opened, and when the next may open.
    pub(crate) opening: Opening,
}

impl QrRuntime {
    pub(crate) fn next_due(&self) -> Option<Instant> {
        self.run.as_ref().map(|r| r.until)
    }

    fn stop(&mut self, phase: QrPhase, note: &str) -> Option<Box<dyn QrScan>> {
        // Stopped for every menu: the next press of a page's Scan again holds it again.
        self.holders.clear();
        self.view.phase = phase;
        self.view.note = note.to_string();
        self.run.take().map(|r| r.scan)
    }
}

/// What every menu shows of the scanner, the screen's included (its next step draws it).
fn shown(rt: &mut N4n0Runtime) {
    rt.screen.menu.qr = rt.qr.view.clone();
    rt.wake();
}

/// The N4N0 thread's step: the scan stops once its time without a link is up (returned to be dropped after the lock).
pub(crate) fn tick(rt: &mut N4n0Runtime, now: Instant) -> Option<Box<dyn QrScan>> {
    if rt.qr.run.as_ref().is_none_or(|r| now < r.until) {
        return None;
    }
    let scan = rt.qr.stop(QrPhase::Stopped, mark("Stopped after two minutes without a link: press Scan again to read the camera"));
    shown(rt);
    scan
}

impl crate::N4n0 {
    /// Starts the scanner (a menu showed its page, Scan again), or stops it (the page closed) once no other menu
    /// (`holder`: the one asking) still shows the scanner's page.
    pub(crate) fn qr_scan(self: &Shared, on: bool, holder: MenuHolder) {
        if !on {
            let scan = {
                let mut rt = self.n4n0.lock().unwrap();
                if holder == MenuHolder::Any {
                    rt.qr.holders.clear();
                } else {
                    rt.qr.holders.remove(&holder);
                }
                if !rt.qr.view.scanning() || !rt.qr.holders.is_empty() {
                    return;
                }
                let scan = rt.qr.stop(QrPhase::Off, "");
                shown(&mut rt);
                scan
            };
            drop(scan);
            self.publish();
            return;
        }
        let number = {
            let mut rt = self.n4n0.lock().unwrap();
            rt.qr.holders.insert(holder);
            if let Some(run) = rt.qr.run.as_mut() {
                // Asked again while it runs: its time starts again.
                run.until = Instant::now() + qr::SCAN_FOR;
                return;
            }
            rt.qr.number += 1;
            rt.qr.view.phase = QrPhase::Starting;
            rt.qr.view.note.clear();
            shown(&mut rt);
            rt.qr.number
        };
        self.publish();
        let weak = Arc::downgrade(self);
        let started = self.host.qr_scan(Box::new(move |seen| {
            if let Some(n) = weak.upgrade() {
                n.qr_seen(number, seen);
            }
        }));
        let late = {
            let mut rt = self.n4n0.lock().unwrap();
            match started {
                // Stopped (or started again, or it said it failed) while it started: this one goes.
                Ok(scan) if rt.qr.number != number || !rt.qr.view.scanning() => Some(scan),
                Ok(scan) => {
                    rt.qr.run = Some(Run { scan, number, until: Instant::now() + qr::SCAN_FOR, recent: Recent::default(), opening: Opening::default() });
                    None
                }
                Err(why) => {
                    if rt.qr.number == number {
                        rt.qr.stop(QrPhase::Stopped, &why);
                        shown(&mut rt);
                    }
                    None
                }
            }
        };
        drop(late);
        self.publish();
    }

    /// What the scanner `number` saw (on its thread).
    fn qr_seen(self: &Shared, number: u64, seen: QrSeen) {
        // This scan's word: the one running, or the one starting (its thread may speak before the app has its handle).
        let current = |rt: &N4n0Runtime| match rt.qr.run.as_ref() {
            Some(r) => r.number == number,
            None => rt.qr.number == number && rt.qr.view.scanning(),
        };
        match seen {
            QrSeen::Camera(true) => {
                let mut rt = self.n4n0.lock().unwrap();
                if !current(&rt) || rt.qr.view.phase == QrPhase::Looking {
                    return;
                }
                rt.qr.view.phase = QrPhase::Looking;
                rt.qr.view.note.clear();
                shown(&mut rt);
            }
            QrSeen::Camera(false) => {
                // Whether VRChat's camera is open says what to do (asked off the lock: it asks VRChat).
                let note = qr::no_camera_note(self.host.camera_open());
                let mut rt = self.n4n0.lock().unwrap();
                if !current(&rt) || (rt.qr.view.phase == QrPhase::NoCamera && rt.qr.view.note == note) {
                    return;
                }
                rt.qr.view.phase = QrPhase::NoCamera;
                rt.qr.view.note = note;
                shown(&mut rt);
            }
            QrSeen::Code(text) => {
                let link = qr::web_link(&text);
                let open = {
                    let mut rt = self.n4n0.lock().unwrap();
                    let now = Instant::now();
                    let Some(run) = rt.qr.run.as_mut().filter(|r| r.number == number) else { return };
                    match &link {
                        // Seen lately: opened already (or failing): never again meanwhile.
                        Ok(link) if run.recent.lately(link, now) => return,
                        Ok(link) => match run.opening.take(now) {
                            Pace::Open => {
                                // Noted before the browser is asked: the same code in the next look waits.
                                run.recent.opening(link, now);
                                Some(link.clone())
                            }
                            // Not yet: still in view a moment later, it opens then.
                            Pace::Later => return,
                            Pace::Enough => {
                                let scan = rt.qr.stop(QrPhase::Stopped, qr::ENOUGH_NOTE);
                                shown(&mut rt);
                                drop(rt);
                                drop(scan);
                                self.publish();
                                return;
                            }
                        },
                        Err(why) => {
                            // Said once per kind of code, never what it holds (the screen is seen by everyone).
                            let note = latin(|| tr!("{reason}: a QR code here opens only http or https pages", reason = tr(why)));
                            if rt.qr.view.note == note {
                                return;
                            }
                            rt.qr.view.note = note;
                            shown(&mut rt);
                            None
                        }
                    }
                };
                if let Some(link) = open {
                    match self.host.watch(&link) {
                        Ok(()) => {
                            // Opened: counted, and the scan's time starts again (when it is still this scan).
                            let mut rt = self.n4n0.lock().unwrap();
                            let now = Instant::now();
                            let mut ended = None;
                            if let Some(run) = rt.qr.run.as_mut().filter(|r| r.number == number) {
                                run.until = now + qr::SCAN_FOR;
                                // The sixth page opened: the scan stops at once (verify-n4n0 10).
                                let last = run.opening.opened();
                                rt.qr.view.phase = QrPhase::Looking;
                                rt.qr.view.note.clear();
                                rt.qr.view.link = Some(link.clone());
                                rt.qr.view.opened += 1;
                                // The owner's screen stays open while it finds links.
                                rt.screen.last_activity = now;
                                if last {
                                    ended = rt.qr.stop(QrPhase::Stopped, qr::ENOUGH_NOTE);
                                }
                                shown(&mut rt);
                            }
                            drop(rt);
                            drop(ended);
                            self.notify(NoticeKind::Success, tr!("Opened from a QR code: {link}", link = orbiters_n4n0_core::menu::layout::short_link(&link, 60)));
                        }
                        // Not opened: nothing counts it as opened (and the same link waits before it is tried again).
                        Err(e) => self.notify(NoticeKind::Warning, e),
                    }
                }
            }
            // A code in view not read: said in the note while it looks (nothing said of what it holds).
            QrSeen::Glimpse(glimpse) => {
                let note = match glimpse {
                    Some(crate::platform::QrGlimpse::Cut) => latin(|| tr("A QR code is cut by the camera's edge: bring all of it into the picture").to_string()),
                    Some(crate::platform::QrGlimpse::Unclear) => latin(|| tr("A QR code is in view: hold the camera still, a little closer").to_string()),
                    None => String::new(),
                };
                let mut rt = self.n4n0.lock().unwrap();
                if !current(&rt) || rt.qr.view.phase != QrPhase::Looking || rt.qr.view.note == note {
                    return;
                }
                rt.qr.view.note = note;
                shown(&mut rt);
            }
            QrSeen::Failed(why) => {
                let scan = {
                    let mut rt = self.n4n0.lock().unwrap();
                    if !current(&rt) {
                        return;
                    }
                    let scan = rt.qr.stop(QrPhase::Stopped, &why);
                    shown(&mut rt);
                    scan
                };
                drop(scan);
            }
        }
        self.publish();
    }
}
