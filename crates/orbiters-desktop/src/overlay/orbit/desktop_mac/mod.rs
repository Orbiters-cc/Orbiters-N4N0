//! The panels around VRChat's menus on a Mac's screen (`orbiters-desktop --orbit-desktop --service <port> <token>`): the same parts, places and detectors as on Windows
//! ([`super::desktop`]), over the window of the VRChat the engine runs here. A process of its own, started by the tray service while Home's VRChat runs.
//!
//! What is this Mac's own:
//! - **The game's window** is the engine's window server's ([`game`]): borderless over the whole screen, above the menu bar's level while it is in front. Ours are ordinary
//!   panels of AppKit's just above it ([`panel`]). Nothing is injected, drawn or sent into the game, its window or its window server; no file or thread of it is touched.
//! - **A menu is known to be open** by looking at that window's own picture (ScreenCaptureKit, [`capture`]), only just after Esc or R and while a menu shows. It needs macOS's
//!   Screen Recording; without it nothing shows and Settings says so.
//! - **The keys** are read by asking for their state ([`keyboard`]), only while the game is in front: no event tap, no Accessibility.
//! - **The mouse is VRChat's for as long as it is in front, its Quick Menu included** (the engine holds the pointer of a full-screen game). Holding Left Option, or a page of
//!   ours being up, takes the keyboard for a moment without coming in front ([`pointing`], [`panel::Hold`]): the engine then lets the pointer go and releases the keys the
//!   game held, and takes it back by itself as soon as ours goes. The keyboard is then ours too: a page can be typed in, and Esc closes it.
//! - **Not here**: the 3D pictures cell and the camera's column (Spout), translation, the corner's cards and the mismatch buttons.

pub mod capture;
pub mod game;
pub mod keyboard;
pub mod lab;
pub mod panel;
pub mod pointing;
pub mod wheel;

use super::desk_shared::{self, hit, Clicks, Hints, Presence};
use super::detect::{MenuKey, Seen, Watcher};
use super::zones::{self, Around, Zone};
use super::Panels;
use crate::backend::Ctl;
use crate::ipc::{Client, Endpoint, Role};
use crate::state::{OrbitNeeds, OrbitStatus, Snapshot};
use anyhow::Context as _;
use capture::{Answer, Ask, Looker, Search, Trouble};
use egui::{pos2, vec2, Pos2, Vec2};
use game::{Finder, Game, Shot};
use keyboard::{Keys, Source};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSEventMask, NSEventModifierFlags, NSEventType, NSWorkspace};
use objc2_foundation::{NSActivityOptions, NSDate, NSDefaultRunLoopMode, NSNotification, NSProcessInfo};
use orbiters_i18n::mark;
use panel::{Arrow, Hold, Layer, PanelWindow};
use wheel::{Fit, Scores};
use pointing::{Pointing, Turn};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A turn with nothing showing: the keys are asked for, with the game in front, and nothing else happens. Also one with parts that rest.
const REST: Duration = Duration::from_millis(33);
/// With the mouse on the parts, a part moving, or a menu looked for.
const TURN: Duration = Duration::from_millis(8);
/// The game's window is asked of macOS this often at rest (a key pressed asks at once), and with parts up or a menu looked for: each time is a message to the window server.
const WINDOW_EVERY: Duration = Duration::from_secs(1);
const WINDOW_EVERY_UP: Duration = Duration::from_millis(50);
/// The app in front is asked for when macOS says another came there, and this often all the same.
const FRONT_EVERY: Duration = Duration::from_millis(500);
/// How often the windows' order is looked at while another app is in front, and where ours really are.
const ORDER_EVERY: Duration = Duration::from_millis(250);
const PLACES_EVERY: Duration = Duration::from_millis(500);
/// macOS's permission is asked for at a key that looks, and this often with the game in front.
const ALLOWED_EVERY: Duration = Duration::from_secs(10);
const ALLOWED_AT_A_KEY: Duration = Duration::from_secs(1);
/// How long the engine takes at most to let the pointer go once ours has the keyboard (3 to 7 ms were measured).
const LETS_GO: Duration = Duration::from_millis(40);
/// The keyboard macOS did not give ours this long after it was asked for went to another window.
const KEYBOARD_WITHIN: Duration = Duration::from_millis(200);
/// A mouse button still down is waited for this long at most before the game has its mouse back: it must not hear half a click.
const BUTTON_WITHIN: Duration = Duration::from_secs(2);
/// With no Screen Recording, a process this old ends at the next key that would look: macOS tells a permission given since only to a new one (the service starts it again).
const SEES_ANEW_AFTER: Duration = Duration::from_secs(30);
/// What went wrong with a picture is said for this long after the last time.
const TROUBLE_SAID: Duration = Duration::from_secs(20);

const PICTURE_FAILED: &str = mark("macOS did not give a picture of VRChat's window just now. Press Esc or R again in VRChat; if nothing appears, quit and reopen Orbiters - N4N0.");

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let endpoint = Endpoint::from_args(args).context("--orbit-desktop needs --service <port> <token>")?;
    let mtm = MainThreadMarker::new().context("not on the main thread")?; // i18n: not shown (a programming error, logged)
    let client = Client::connect(&endpoint, Role::Orbit)?;
    let backend: Ctl = Arc::new(crate::backend::Remote(client.clone()));
    crate::member::through(backend.clone());
    crate::usage::forward(backend.clone());
    // No window of its own for hours: macOS would let it nap, and Esc would be seen seconds late.
    stay_awake();
    let mut desk = Desk::new(mtm, backend.clone(), Box::new(keyboard::Native::default()));
    loop {
        if client.should_close() || !super::wanted_on_desktop(&backend.view()) {
            return Ok(());
        }
        match desk.turn() {
            Next::Wait(long) => desk.wait(long),
            Next::Anew => return Ok(()),
        }
    }
}

/// Keeps macOS's App Nap off this process for as long as it runs.
fn stay_awake() {
    let reason = objc2_foundation::NSString::from_str("Watches for VRChat's menus"); // i18n: not shown
    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(NSActivityOptions::UserInitiatedAllowingIdleSystemSleep, &reason);
    // For the life of the process: ending it would let the nap back.
    std::mem::forget(activity);
}

/// The app was opened again (from the Finder, the Dock or Spotlight) and macOS told this process of it, which is one of the app's: its window is to show.
static OPENED: AtomicBool = AtomicBool::new(false);
/// Another app came in front since this was last asked.
static FRONT_CHANGED: AtomicBool = AtomicBool::new(true);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OrbitersOverlayDelegate"]
    struct Delegate;

    impl Delegate {
        #[unsafe(method(frontChanged:))]
        fn front_changed(&self, _: &NSNotification) {
            FRONT_CHANGED.store(true, Ordering::Release);
        }
    }

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _: &NSApplication, _: bool) -> bool {
            OPENED.store(true, Ordering::Release);
            false
        }
    }
);

/// What comes after a turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    Wait(Duration),
    /// The process ends, for the service to start another.
    Anew,
}

/// What our panels heard.
#[derive(Clone, Debug, PartialEq)]
enum Heard {
    Button(bool),
    /// The scroll wheel, in notches.
    Wheel(f32),
    Key { code: u16, down: bool, command: bool, control: bool, modifiers: u32 },
}

/// Ends the process if its loop hangs while it holds the keyboard: the engine takes it back the moment our windows are gone.
struct Watchdog {
    /// When the loop last turned, in milliseconds since the start, and 0 while nothing is held.
    held: Arc<AtomicU64>,
    started: Instant,
    running: bool,
}

const HANGS_AFTER: Duration = Duration::from_millis(1500);

impl Watchdog {
    fn new() -> Self {
        Self { held: Arc::default(), started: Instant::now(), running: false }
    }

    fn beat(&mut self, holding: bool) {
        let now = self.started.elapsed().as_millis() as u64 + 1;
        self.held.store(if holding { now } else { 0 }, Ordering::Release);
        if holding && !self.running {
            self.running = true;
            let (held, started) = (self.held.clone(), self.started);
            let _ = std::thread::Builder::new().name("orbit-watchdog".into()).spawn(move || loop {
                std::thread::sleep(Duration::from_millis(250));
                if hung(held.load(Ordering::Acquire), started.elapsed().as_millis() as u64 + 1) {
                    eprintln!("orbit: the loop hung while it held the keyboard: ending"); // i18n: not shown (a log's line)
                    // SAFETY: the process ends here, whatever its main thread waits for.
                    unsafe { libc::_exit(70) };
                }
            });
        }
    }
}

fn hung(held: u64, now: u64) -> bool {
    held != 0 && now.saturating_sub(held) >= HANGS_AFTER.as_millis() as u64
}

/// The windows of the parts, by zone, each kept while hidden as long as its size is the part's.
#[derive(Default)]
struct Windows {
    list: HashMap<Zone, PanelWindow>,
    scale: f64,
    /// Those showing, as they lie, bottom first.
    order: Vec<Zone>,
    layer: Option<Layer>,
    looked: Option<Instant>,
    placed: Option<Instant>,
    /// Pictures given to windows so far.
    painted: u64,
    /// A part that would have covered the game's whole window, left out.
    left_out: Option<Zone>,
    /// A part was clicked: macOS raised its window above the others of its level.
    clicked: bool,
}

impl Windows {
    fn hide_all(&mut self) {
        for w in self.list.values_mut() {
            w.hide();
        }
        self.order.clear();
        self.layer = None;
    }

    fn shown(&self) -> Vec<&PanelWindow> {
        self.order.iter().filter_map(|z| self.list.get(z)).filter(|w| w.shown()).collect()
    }

    /// New pictures, then every part where its spring has it, in their order, where `layer` says.
    fn show(&mut self, mtm: MainThreadMarker, panels: &mut Panels, snap: &Snapshot, game: &Game, layer: Layer, now: Instant) {
        if self.scale != game.scale {
            self.list.clear();
            self.order.clear();
            self.scale = game.scale;
            panels.paint_all_again();
        }
        let (list, painted) = (&mut self.list, &mut self.painted);
        panels.paint(snap, |panel, changed| {
            *painted += 1;
            if list.get(&panel.zone).is_none_or(|w| w.px() != panel.px) {
                list.insert(panel.zone, PanelWindow::new(mtm, panel.px, game.scale));
            }
            if let Some(w) = list.get_mut(&panel.zone) {
                w.picture(panel.pixels(), changed);
            }
        });
        let showing: Vec<(Zone, [i32; 2], u8)> = panels.all().filter(|p| p.painted && p.shown).map(|p| desk_shared::spot(p, pos2(0.0, 0.0))).filter(|(z, _, _)| self.list.contains_key(z)).collect();
        // Never the whole of the game's window: macOS would stop showing it.
        let rects: Vec<egui::Rect> = showing
            .iter()
            .map(|(zone, spot, _)| {
                let px = self.list[zone].px();
                egui::Rect::from_min_size(pos2(spot[0] as f32, spot[1] as f32), vec2(px[0] as f32, px[1] as f32))
            })
            .collect();
        let may = panel::uncovering(&rects, game.client());
        self.left_out = showing.iter().zip(&may).find(|(_, may)| !**may).map(|((zone, _, _), _)| *zone);
        let showing: Vec<(Zone, [i32; 2], u8)> = showing.into_iter().zip(may).filter(|(_, may)| *may).map(|(s, _)| s).collect();
        let mut came = false;
        // macOS moves windows as displays change: where each really is, is asked now and then.
        let ask = self.placed.is_none_or(|t| now.duration_since(t) >= PLACES_EVERY);
        if ask {
            self.placed = Some(now);
        }
        for (zone, spot, alpha) in &showing {
            if let Some(w) = self.list.get_mut(zone) {
                came |= w.place(game, *spot, *alpha, ask);
            }
        }
        let order: Vec<Zone> = showing.iter().map(|(z, _, _)| *z).collect();
        for (zone, w) in &mut self.list {
            if !order.contains(zone) {
                w.hide();
            }
        }
        let moved = self.order != order;
        self.order = order;
        // Behind another app's windows, another may come between the game's window and ours.
        let apart = match layer {
            Layer::With(id) if self.looked.is_none_or(|t| now.duration_since(t) >= ORDER_EVERY) => {
                self.looked = Some(now);
                let ours: Vec<u32> = self.shown().iter().filter_map(|w| u32::try_from(w.number()).ok()).collect();
                crate::bridge::mac::on_screen().is_some_and(|list| !panel::holds(&list.iter().map(|w| (w.id, w.layer)).collect::<Vec<_>>(), id, &ours))
            }
            _ => false,
        };
        if came || moved || apart || std::mem::take(&mut self.clicked) || self.layer != Some(layer) {
            self.layer = Some(layer);
            let ours: Vec<(Zone, &PanelWindow)> = self.order.iter().filter_map(|z| self.list.get(z).map(|w| (*z, w))).filter(|(_, w)| w.shown()).collect();
            panel::lay(&ours, layer);
        }
    }

    /// Only the part under the pointer takes the mouse, where it is painted; with `at` none, none does.
    fn catch_at(&mut self, at: Option<Pos2>) {
        let under = at.and_then(|at| panel::part_at(self.order.iter().rev().filter_map(|z| self.list.get(z).map(|w| (z, w))), at));
        for (zone, w) in &mut self.list {
            w.catch_mouse(under == Some(*zone));
        }
    }
}

/// A look as it goes, for the line of the log that says why it found nothing.
#[derive(Clone, Copy, Debug)]
struct LookSaw {
    /// Which of them it is, and the key that began it (none: it was the game coming in front, or a key macOS did not show).
    number: u32,
    key: Option<MenuKey>,
    began: Instant,
    /// Frames looked at, and how long the first took to come.
    frames: u32,
    first: Option<Duration>,
    /// The most any of them had.
    wheel: Scores,
    fit: Option<Fit>,
    icons: usize,
    anywhere: Option<Option<Fit>>,
}

impl LookSaw {
    fn new(number: u32, key: Option<MenuKey>, began: Instant) -> Self {
        Self { number, key, began, frames: 0, first: None, wheel: Scores::default(), fit: None, icons: 0, anywhere: None }
    }

    fn take(&mut self, answer: &Answer) {
        if answer.look != self.number {
            return;
        }
        self.frames += 1;
        self.first.get_or_insert(answer.at.saturating_duration_since(self.began));
        if answer.saw.wheel.of(&wheel::IN_PLACE) > self.wheel.of(&wheel::IN_PLACE) || self.frames == 1 {
            self.wheel = answer.saw.wheel;
        }
        if let Some(fit) = answer.saw.fit {
            if self.fit.is_none_or(|was| fit.scores.of(&wheel::FIT) > was.scores.of(&wheel::FIT)) {
                self.fit = Some(fit);
            }
        }
        self.icons = self.icons.max(answer.saw.icons);
        self.anywhere = answer.anywhere.or(self.anywhere);
    }

    /// What it found nothing in, in numbers.
    fn says(&self, shot: Shot) -> String {
        let (w, p) = (self.wheel, wheel::IN_PLACE);
        let fit = |f: &Option<Fit>| f.map_or("none".into(), |f| f.to_string()); // i18n: not shown (a log's line)
        let anywhere = self.anywhere.map_or("not looked for".into(), |f| fit(&f)); // i18n: not shown (a log's line)
        format!(
            "{} looked at ({} x {} with its title, the first after {:?} ms); the wheel in its place at best: edge {:.2}, one colour {:.2}, line {:.2} (asked {} {} {}); around it: {}; anywhere: {anywhere}; Quick Menu icons at best {} (asked {})", // i18n: not shown (a log's line)
            self.frames,
            shot.width,
            shot.height,
            self.first.map(|f| f.as_millis()),
            w.edge,
            w.uniform,
            w.line,
            p.edge,
            p.uniform,
            p.line,
            fit(&self.fit),
            self.icons,
            super::detect::ICONS_NEEDED
        ) // i18n: not shown (a log's line)
    }
}

/// The overlay's loop on a Mac, turn by turn ([`run`]; the lab turns it with a keyboard and a game of its own).
pub struct Desk {
    mtm: MainThreadMarker,
    /// Kept: AppKit and its notifications hold no delegate.
    _delegate: Retained<Delegate>,
    backend: Ctl,
    source: Box<dyn Source>,
    finder: Finder,
    in_front: Option<u32>,
    front_asked: Option<Instant>,
    asked_window: Option<Instant>,
    last: Option<Game>,
    windows: Windows,
    panels: Panels,
    watcher: Watcher,
    looker: Looker,
    keys: Keys,
    hints: Hints,
    pointing: Pointing,
    hold: Hold,
    /// Since when ours has the keyboard, and whether the pointer was put on the parts since.
    held_since: Option<Instant>,
    put: bool,
    /// Since when it is no longer wanted: a click under way ends first.
    leaving: Option<Instant>,
    arrow: Arrow,
    given_back: bool,
    /// An accent typed and waiting for its letter.
    dead: u32,
    clicks: Clicks,
    said: Option<OrbitStatus>,
    allowed: (bool, Option<Instant>),
    trouble: Option<Instant>,
    was_front: bool,
    was_there: bool,
    look_due: bool,
    /// macOS refused a picture.
    refused: bool,
    /// Esc went down while the keyboard was ours, with no page to type in: let go, it closes.
    esc: bool,
    /// The look under way or the last one, and what the log last said.
    saw: LookSaw,
    /// Where the wheel up was found, when beside its place.
    wheel_at: Option<Fit>,
    /// The fit of the last frame, before it opens anything.
    fit_seen: Option<Fit>,
    logged: (Option<Seen>, bool, Option<Zone>),
    started: Instant,
    last_turn: Instant,
    meter: crate::perf::TurnMeter,
    settling: crate::backend::Settling,
    watchdog: Watchdog,
}

impl Desk {
    pub fn new(mtm: MainThreadMarker, backend: Ctl, source: Box<dyn Source>) -> Self {
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let delegate = Delegate::alloc(mtm).set_ivars(());
        // SAFETY: NSObject's init, on a freshly allocated object.
        let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
        app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let centre = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: the method is this class's own and takes the notification; the delegate lives as long as the process.
        unsafe { centre.addObserver_selector_name_object(&delegate, sel!(frontChanged:), Some(objc2_app_kit::NSWorkspaceDidActivateApplicationNotification), None) };
        app.finishLaunching();
        let now = Instant::now();
        Self {
            mtm,
            _delegate: delegate,
            panels: Panels::new(backend.clone(), 1.0, true).on_a_mac(),
            backend,
            source,
            finder: Finder::default(),
            in_front: None,
            front_asked: None,
            asked_window: None,
            last: None,
            windows: Windows::default(),
            watcher: Watcher::default(),
            looker: Looker::start(),
            keys: Keys::default(),
            hints: Hints::default(),
            pointing: Pointing::default(),
            hold: Hold::new(mtm),
            held_since: None,
            put: false,
            leaving: None,
            arrow: Arrow::default(),
            given_back: false,
            dead: 0,
            clicks: Clicks::start(),
            said: None,
            allowed: (true, None),
            trouble: None,
            was_front: false,
            was_there: false,
            look_due: false,
            refused: false,
            esc: false,
            saw: LookSaw::new(0, None, now),
            wheel_at: None,
            fit_seen: None,
            logged: (None, false, None),
            started: now,
            last_turn: now,
            meter: crate::perf::TurnMeter::new("orbit-desktop", now),
            settling: Default::default(),
            watchdog: Watchdog::new(),
        }
    }

    /// Waits `long`, or until macOS has something for our windows.
    pub fn wait(&self, long: Duration) {
        autoreleasepool(|_| {
            let app = NSApplication::sharedApplication(self.mtm);
            let until = NSDate::dateWithTimeIntervalSinceNow(long.as_secs_f64());
            // SAFETY: AppKit's own run loop mode.
            let _ = app.nextEventMatchingMask_untilDate_inMode_dequeue(NSEventMask::Any, Some(&until), unsafe { NSDefaultRunLoopMode }, false);
        });
    }

    /// What our windows heard since the last turn.
    fn heard(&self) -> Vec<Heard> {
        let app = NSApplication::sharedApplication(self.mtm);
        let past = NSDate::distantPast();
        let mut out = Vec::new();
        // (bounded: a flood of events never keeps the loop from its turn)
        for _ in 0..256 {
            // SAFETY: AppKit's own run loop mode.
            let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(NSEventMask::Any, Some(&past), unsafe { NSDefaultRunLoopMode }, true) else { break };
            match event.r#type() {
                NSEventType::LeftMouseDown => out.push(Heard::Button(true)),
                NSEventType::LeftMouseUp => out.push(Heard::Button(false)),
                NSEventType::ScrollWheel => {
                    let dy = event.scrollingDeltaY();
                    // A trackpad says points, a wheel its lines.
                    out.push(Heard::Wheel((if event.hasPreciseScrollingDeltas() { dy / 40.0 } else { dy }) as f32));
                }
                kind @ (NSEventType::KeyDown | NSEventType::KeyUp) => {
                    let flags = event.modifierFlags();
                    let has = |f| flags.contains(f);
                    let modifiers = u32::from(has(NSEventModifierFlags::Shift)) << 1 | u32::from(has(NSEventModifierFlags::CapsLock)) << 2 | u32::from(has(NSEventModifierFlags::Option)) << 3;
                    out.push(Heard::Key { code: event.keyCode(), down: kind == NSEventType::KeyDown, command: has(NSEventModifierFlags::Command), control: has(NSEventModifierFlags::Control), modifiers });
                }
                // (the other buttons do nothing here, and nothing else is told of them)
                NSEventType::LeftMouseDragged | NSEventType::RightMouseDown | NSEventType::RightMouseUp | NSEventType::OtherMouseDown | NSEventType::OtherMouseUp => {}
                _ => app.sendEvent(&event),
            }
        }
        out
    }

    /// The app in front. (Whose process it is, macOS is asked over its launch services: not at each turn.)
    fn front(&mut self, now: Instant) -> Option<u32> {
        let due = self.front_asked.is_none_or(|t| now.duration_since(t) >= FRONT_EVERY);
        if FRONT_CHANGED.swap(false, Ordering::AcqRel) || due {
            self.front_asked = Some(now);
            self.in_front = NSWorkspace::sharedWorkspace().frontmostApplication().and_then(|a| u32::try_from(a.processIdentifier()).ok());
        }
        self.in_front
    }

    fn press(&mut self, down: bool, snap: &Snapshot) {
        if !down {
            self.panels.release();
            return;
        }
        let pressed = self.panels.press(snap);
        if pressed.clicked {
            self.clicks.play();
        }
        if pressed.hide {
            self.panels.set_open(self.panels.open_menu(), Vec::new(), snap);
            self.windows.hide_all();
        }
    }

    /// The game has its mouse and keyboard again, at once.
    fn let_go(&mut self) {
        self.hold.give_back();
        self.held_since = None;
        self.leaving = None;
        self.put = false;
        self.dead = 0;
        self.esc = false;
        self.arrow.hide();
        self.windows.catch_at(None);
        self.watchdog.beat(false);
    }

    /// What Settings says of it, when it changed.
    fn say(&mut self, now: Instant) {
        let needs = if !self.allowed.0 {
            Some(OrbitNeeds::ScreenRecording)
        } else if self.keys.hidden(&*self.source) {
            Some(OrbitNeeds::InputMonitoring)
        } else {
            None
        };
        let failed = self.trouble.is_some_and(|t| now.duration_since(t) < TROUBLE_SAID);
        let note = needs.map(OrbitNeeds::words).or(failed.then_some(PICTURE_FAILED));
        let status = OrbitStatus { note: note.map(str::to_string), needs, ..OrbitStatus::default() };
        if self.said.as_ref() != Some(&status) {
            self.backend.orbit_status(status.clone());
            self.said = Some(status);
        }
    }

    /// Where the pointer starts on the parts of the menu `open`: the ring's first cell, the Quick Menu's first tile, the page's middle.
    fn first_point(&self, open: Option<Around>, size: Vec2) -> Option<Pos2> {
        let zones: &[Zone] = match open {
            _ if self.panels.sheet_open() => &[Zone::Takeover],
            Some(Around::Wheel) => {
                let (center, radius) = zones::wheel(size);
                let center = center + self.wheel_at.map_or(Vec2::ZERO, |fit| fit.moved(size));
                // The middle of the ring's first cell (Windows starts there too).
                return Some(center + vec2(0.0, -1.4 * radius));
            }
            _ => &[Zone::WingWorlds, Zone::QmStrip],
        };
        zones.iter().find_map(|z| self.panels.all().find(|p| p.zone == *z && p.shown).map(|p| p.placed.rect.center()))
    }

    /// A key that may have opened a menu: a look of its own in the log.
    fn new_look(&mut self, key: Option<MenuKey>, now: Instant) {
        self.saw = LookSaw::new(self.saw.number.wrapping_add(1), key, now);
        self.fit_seen = None;
    }

    /// A look's answer to what watches. A wheel beside its place opens on two frames that have it at the same one, and stays where the first had it: as it is looked at each
    /// time anew, the ring would shake by a step of the search.
    fn watched(&mut self, answer: &Answer) {
        let saw = answer.saw;
        let was = self.watcher.seen();
        let beside = saw.fit.filter(|_| saw.menu == Some(Around::Wheel));
        let again = beside.is_some_and(|fit| self.fit_seen.is_some_and(|before| before.same(&fit)));
        if let (Some(_), false, Seen::Looking) = (beside, again, was) {
            // The first frame of it there: one, whatever the frame before had elsewhere.
            self.watcher.frame_coming(None, false, answer.at);
        }
        if !again {
            self.fit_seen = beside;
        }
        // (the rim alone, in place, starts it as on Windows)
        let seen = self.watcher.frame_coming(saw.menu, saw.coming, answer.at);
        if seen == Seen::Open(Around::Wheel) {
            if was != seen {
                self.wheel_at = self.fit_seen;
            } else if again && self.wheel_at.is_none() {
                // Opened on its rim, and it is beside its place.
                self.wheel_at = self.fit_seen;
            }
        }
    }

    /// One turn of the loop. (What AppKit makes meanwhile goes with the turn.)
    pub fn turn(&mut self) -> Next {
        autoreleasepool(|_| self.once())
    }

    fn once(&mut self) -> Next {
        if OPENED.swap(false, Ordering::AcqRel) {
            self.backend.show_window();
        }
        let now = Instant::now();
        let dt = now.duration_since(self.last_turn).as_secs_f32();
        self.last_turn = now;
        let heard = self.heard();
        let snap = self.backend.view();
        // What is pressed this turn was pressed on pictures of the member shown before it.
        self.backend.shown(self.settling.turn(snap.n4n0.account_epoch, now), orbiters_n4n0::Surface::Overlay);
        let in_front = self.front(now);
        let front = self.finder.server().is_some_and(|s| in_front == Some(s));
        let holding = self.hold.held();
        let keys = if front {
            self.keys.turn(&*self.source, now)
        } else {
            self.keys.away(&*self.source);
            keyboard::Said::default()
        };
        let back = front && !self.was_front;
        self.was_front = front;
        let resting = !self.watcher.wants_frames() && self.panels.is_empty();
        let due = self.asked_window.is_none_or(|t| now.duration_since(t) >= if resting { WINDOW_EVERY } else { WINDOW_EVERY_UP });
        if due || back || keys.menu.is_some() || keys.unknown {
            self.asked_window = Some(now);
            self.last = self.finder.find(snap.launcher.desktop_game);
        }
        let found = self.last;
        // Back in front, or its window back on the screen (or this just started): a menu may be open that nobody pressed a key for since.
        let there = found.is_some();
        self.look_due = front && (self.look_due || back || there && !self.was_there);
        self.was_there = there;
        let allowed_every = if keys.menu.is_some() { ALLOWED_AT_A_KEY } else { ALLOWED_EVERY };
        if front && self.allowed.1.is_none_or(|t| now.duration_since(t) >= allowed_every) {
            let was = self.allowed.0;
            self.allowed = (!self.refused && crate::bridge::mac::screen_recording() != Some(false), Some(now));
            if was != self.allowed.0 {
                super::super::vr_log::line(&format!("desk: Screen Recording allowed {}", self.allowed.0)); // i18n: not shown (a log's line)
            }
        }
        if let Some(why) = self.looker.trouble() {
            super::super::vr_log::line(&format!("desk: no picture of the game's window: {why:?}")); // i18n: not shown (a log's line)
            match why {
                // (macOS keeps telling a process what it was told at its start: refused since, it stays so until a new one asks)
                Trouble::NotAllowed => {
                    self.refused = true;
                    self.allowed = (false, Some(now));
                }
                Trouble::Stopped => {}
                Trouble::Failed(_) => self.trouble = Some(now),
            }
        }
        self.say(now);
        let presence = Presence::of(found.is_some(), found.is_some(), false, front);
        let Some(game) = found.filter(|_| presence.show) else {
            // The parts of menus that closed are kept for a while; nothing else comes by to let them go.
            self.panels.let_go(now);
            self.looker.ask(None);
            let _ = self.looker.answers();
            if self.watcher.wants_frames() || !self.panels.is_empty() {
                self.watcher.reset();
                self.panels.set_open(None, Vec::new(), &snap);
                self.windows.hide_all();
            }
            if holding {
                self.let_go();
            }
            self.panels.serve_menu(&snap);
            return self.end(now, REST);
        };
        // VRChat's keys: the game's alone, so not those pressed while the keyboard is ours.
        if !holding {
            if let Some(key) = keys.menu {
                self.new_look(Some(key), now);
                self.watcher.pressed(key, now);
            } else if (keys.unknown || self.look_due) && self.watcher.seen() == Seen::Resting {
                self.new_look(None, now);
                self.watcher.pressed(MenuKey::Esc, now);
            }
            self.look_due = false;
        }
        if self.watcher.wants_frames() && !self.allowed.0 {
            self.watcher.reset();
            if keys.menu.is_some() && self.started.elapsed() >= SEES_ANEW_AFTER {
                return Next::Anew;
            }
        }
        for answer in self.looker.answers() {
            self.trouble = None;
            self.saw.take(&answer);
            self.watched(&answer);
        }
        let looking = self.watcher.wants_frames();
        let search = match (self.wheel_at, self.watcher.seen()) {
            (Some(fit), Seen::Open(Around::Wheel)) => Search::At(fit),
            // Esc opens none.
            (None, Seen::Looking | Seen::Open(Around::Wheel)) | (_, Seen::Looking) if self.saw.key != Some(MenuKey::Esc) => Search::Around,
            _ => Search::InPlace,
        };
        self.looker.ask(looking.then(|| Ask {
            window: game.window.id,
            owner: game.window.owner,
            shot: Shot::of(&game),
            every: self.watcher.look_every(now),
            search,
            look: self.saw.number,
            began: self.saw.began,
            keep: snap.settings.orbit.keep_looks,
        }));
        let seen = self.watcher.tick(now);
        if seen != Seen::Open(Around::Wheel) && seen != Seen::Looking {
            self.wheel_at = None;
        }
        let open = match seen {
            Seen::Open(Around::Wheel) if snap.settings.orbit.around_wheel() => Some(Around::Wheel),
            Seen::Open(Around::QuickMenu) if snap.settings.orbit.around_quick_menu() => Some(Around::QuickMenu),
            _ => None,
        };
        let size = game.client();
        let places = open.map_or_else(Vec::new, |menu| {
            let mut places = zones::layout_as(menu, size, snap.settings.orbit.layout == orbiters_core::settings::OrbitLayout::Strip);
            places.retain(|p| !p.zone.needs_the_camera_s_frames());
            // Around the wheel where it really is.
            if let Some(fit) = self.wheel_at.filter(|_| menu == Around::Wheel) {
                let moved = fit.moved(size);
                for p in &mut places {
                    (p.rect, p.center) = (p.rect.translate(moved), p.center + moved);
                }
            }
            places
        });
        self.panels.takeover_place = zones::place(Zone::Takeover, size);
        self.panels.back_place = zones::place(Zone::SheetBack, size);
        self.panels.hint_place = zones::place(Zone::SheetHint, size);
        self.panels.set_open(open, places, &snap);
        self.panels.serve_menu(&snap);
        self.panels.serve_assistant(&snap);
        self.panels.follow_note(&snap);
        if self.panels.is_empty() {
            self.pointing.turn(Turn::default());
            if holding {
                self.let_go();
            }
            self.windows.hide_all();
            return self.end(now, if looking { TURN } else { REST });
        }
        self.panels.follow(&snap, self.backend.library_version());
        self.panels.menu_turn(&snap);
        self.panels.step_springs(dt);
        let at = game.pixel(panel::pointer_at());
        self.point(&game, open, &keys, front, at, now);
        let active = self.put;
        self.panels.set_pointing(active && !self.panels.sheet_open(), !snap.settings.reduce_motion);
        self.panels.set_key_caps(front && keys.option);
        let on = active.then(|| hit(&self.panels, at)).flatten();
        self.panels.hover(on);
        self.hints.follow(open, active);
        self.panels.set_hint(self.hints.now_on_a_mac(open, active));
        for event in heard {
            match event {
                // (on a pixel of a part, whose window took it)
                Heard::Button(down) => {
                    self.windows.clicked = true;
                    self.press(down, &snap);
                }
                Heard::Wheel(notches) if active => self.panels.wheel(notches),
                Heard::Wheel(_) => {}
                Heard::Key { code, down, command, control, modifiers } => self.key(code, down, command, control, modifiers),
            }
        }
        if let Some(cell) = self.keys.digit(&*self.source, front && open == Some(Around::Wheel) && !self.panels.sheet_open()) {
            if self.panels.press_cell(cell, &snap).clicked {
                self.clicks.play();
            }
        }
        // A button let go off a part never reaches its window: what is held ends with it.
        if !self.source.button() && self.panels.all().any(|p| p.pressed.is_some()) {
            self.panels.release();
        }
        let layer = if front { Layer::Above(panel::level(&game, true)) } else { Layer::With(game.window.id) };
        self.windows.show(self.mtm, &mut self.panels, &snap, &game, layer, now);
        if !self.source.button() {
            self.windows.catch_at(active.then_some(at));
        }
        let quick = self.hold.held() || looking && self.watcher.look_every(now).is_zero() || self.panels.busy();
        self.end(now, if quick { TURN } else { REST })
    }

    /// Takes the keyboard or gives it back as [`Pointing`] says, and puts the pointer on the parts once the engine let it go.
    fn point(&mut self, game: &Game, open: Option<Around>, keys: &keyboard::Said, front: bool, at: Pos2, now: Instant) {
        let sheet = self.panels.sheet_open() && !self.panels.peek;
        let lost = self.held_since.is_some_and(|t| now.duration_since(t) >= KEYBOARD_WITHIN) && !self.hold.has_keyboard();
        let given_back = std::mem::take(&mut self.given_back);
        let wanted = self.pointing.turn(Turn { front, open, sheet, option: keys.option, lost, given_back });
        let level = panel::level(game, true);
        if !wanted {
            if self.hold.held() {
                // Not in the middle of a click, whose end the game would hear alone.
                let since = *self.leaving.get_or_insert(now);
                let mid_click = front && open.is_some() && !lost && self.put && self.source.button() && now.duration_since(since) < BUTTON_WITHIN;
                if !mid_click {
                    self.let_go();
                }
            }
            return;
        }
        self.leaving = None;
        if !self.hold.held() {
            self.hold.take(game, level);
            self.held_since = Some(now);
            self.put = false;
            return;
        }
        if !self.put && self.hold.has_keyboard() && self.held_since.is_some_and(|t| now.duration_since(t) >= LETS_GO) {
            // Where the game left it, if that is on a part already; else on the first of them.
            let on_a_part = hit(&self.panels, at).is_some();
            if let Some(first) = self.first_point(open, game.client()).filter(|_| !on_a_part) {
                panel::put_pointer(&self.hold, game.point(first));
            }
            self.put = true;
        }
        if self.put && !panel::pointer_shows() {
            self.arrow.show(self.mtm, game, game.pixel(panel::pointer_at()), level);
        } else {
            self.arrow.hide();
        }
    }

    /// A key pressed while the keyboard is ours: typed in a page that takes it; Esc, let go, closes the page or gives the game its keyboard back. (As it is let go: held, it
    /// would go on repeating into the game.)
    fn key(&mut self, code: u16, down: bool, command: bool, control: bool, modifiers: u32) {
        if code == keyboard::ESC && down {
            self.esc = !self.panels.typing();
        }
        // (its down was the page's: so is its up, though the page took no more typing by then)
        let closes = code == keyboard::ESC && !down && std::mem::take(&mut self.esc);
        if self.panels.typing() {
            if down {
                let dead = &mut self.dead;
                let key = keyboard::typed(code, command, control, || keyboard::Layout::now().map(|l| l.writes(code, modifiers, dead)).unwrap_or_default());
                if let Some(key) = key {
                    self.backend.typed(key);
                }
            }
            return;
        }
        if closes {
            if self.panels.sheet_open() {
                self.panels.close_sheet();
            } else {
                self.given_back = true;
            }
        }
    }

    /// What it does, in `vr-test.log`, as it changes: what a report of something wrong with it starts from. Numbers alone: no picture, no name, no key but Esc and R.
    fn log(&mut self, now: Instant) {
        let seen = self.watcher.seen();
        let is = (Some(seen), self.hold.held(), self.windows.left_out);
        if is != self.logged {
            let after = now.saturating_duration_since(self.saw.began).as_millis();
            let (game, frames) = (self.last.map(|g| (g.window.id, g.window.layer, g.client(), g.scale)), self.looker.running());
            let key = self.saw.key;
            super::super::vr_log::line(&format!("desk: {seen:?}, {after} ms after the look {} began (key {key:?}); in front {}; keyboard held {}; window {game:?}; picture (runs, frames) {frames:?}; left out {:?}", self.saw.number, self.was_front, is.1, is.2)); // i18n: not shown (a log's line)
            match (self.logged.0, seen) {
                (Some(Seen::Looking), Seen::Resting) => {
                    if let Some(game) = self.last {
                        super::super::vr_log::line(&format!("desk: the look {} after {key:?} found nothing: {}", self.saw.number, self.saw.says(Shot::of(&game)))); // i18n: not shown (a log's line)
                    }
                }
                (was, Seen::Open(menu)) if was != Some(seen) => {
                    let parts: Vec<String> = self.windows.order.iter().filter_map(|z| self.windows.list.get(z).map(|w| format!("{z:?} {:?} at {:?}", w.px(), w.spot()))).collect(); // i18n: not shown (a log's line)
                    let place = self.wheel_at.filter(|_| menu == Around::Wheel).map_or_else(|| "in its place".to_string(), |f| f.to_string()); // i18n: not shown (a log's line)
                    super::super::vr_log::line(&format!("desk: {menu:?} {place}; parts {}", parts.join(", "))); // i18n: not shown (a log's line)
                }
                _ => {}
            }
            self.logged = is;
        }
    }

    fn end(&mut self, began: Instant, wait: Duration) -> Next {
        self.log(began);
        self.watchdog.beat(self.hold.held());
        self.meter.turn(began.elapsed());
        if let Some(turns) = self.meter.take(Instant::now()) {
            self.backend.perf_turns(turns);
        }
        Next::Wait(wait.saturating_sub(began.elapsed()))
    }

    /// What a part or the keyboard is doing, for the lab.
    pub fn doing(&self) -> Doing {
        Doing {
            seen: self.watcher.seen(),
            open: self.panels.open_menu(),
            shown: self.windows.shown().iter().filter_map(|w| u32::try_from(w.number()).ok()).collect(),
            catching: self.windows.list.values().filter(|w| w.catches()).count(),
            held: self.hold.held(),
            keyboard: self.hold.has_keyboard(),
            pointing: self.put,
            sheet: self.panels.sheet_open(),
            looking: self.looker.running(),
            painted: self.windows.painted,
            left_out: self.windows.left_out,
        }
    }

}

/// What the loop is doing ([`Desk::doing`]).
#[derive(Clone, Debug)]
pub struct Doing {
    pub seen: Seen,
    pub open: Option<Around>,
    /// The parts' windows showing (their numbers), bottom first.
    pub shown: Vec<u32>,
    pub catching: usize,
    pub held: bool,
    pub keyboard: bool,
    pub pointing: bool,
    pub sheet: bool,
    pub looking: (bool, u64),
    pub painted: u64,
    pub left_out: Option<Zone>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loop_that_hangs_while_it_holds_the_keyboard_is_ended() {
        assert!(!hung(0, 1_000_000), "nothing held");
        assert!(!hung(5_000, 6_400));
        assert!(hung(5_000, 6_500));
        assert!(!hung(6_000, 5_000), "a beat after the clock was read");
    }
}
