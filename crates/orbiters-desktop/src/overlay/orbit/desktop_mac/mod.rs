//! The panels around VRChat's menus on a Mac's screen (`orbiters-desktop --orbit-desktop --service <port> <token>`): the same parts, places and detectors as on Windows
//! ([`super::desktop`]), over the window of the VRChat the engine runs here. A process of its own, started by the tray service while Home's VRChat runs.
//!
//! What is this Mac's own:
//! - **The game's window** is the engine's window server's ([`game`]): borderless over the whole screen, above the menu bar's level while it is in front. Ours are ordinary
//!   panels of AppKit's just above it ([`panel`]). Nothing is injected, drawn or sent into the game, its window or its window server; no file or thread of it is touched.
//! - **A menu is known to be open** by looking at that window's own picture (ScreenCaptureKit, [`capture`]), only just after Esc or R and while a menu shows. It needs macOS's
//!   Screen Recording; without it nothing shows and Settings says so.
//! - **The keys** are read by asking for their state ([`keyboard`]), only while the game is in front: no event tap, no Accessibility.
//! - **The mouse is VRChat's while it holds it, and the person's in its Quick Menu**, as on Windows: the engine (obelisk-cli 0.1.15) lets it go where the game does and says so
//!   ([`channel`]). The wing and the pages there are clicked as any window is, and the game keeps its keyboard. VRChat's wheel holds the mouse: the ring round it is picked by
//!   pushing past the wheel's rim ([`gesture`]), with no key and nothing taken from the game.
//! - **The keyboard is ours only where that is needed** ([`pointing`], [`panel::Hold`]): while something of ours is typed in, and for a page opened from the wheel. It is taken
//!   without coming in front: the engine then lets the pointer go and releases the keys the game held, and takes it back by itself as soon as ours goes.
//! - **With an engine from before**, which holds the mouse for as long as its game is in front, Left Option does what it did: it takes the keyboard while held.
//! - **VRChat's camera** (the 3D pictures cell, its second ring, the column beside the camera's panel, the pill of a picture under way) is there where the engine gives its
//!   stream (`LauncherState::camera`): the panel is found in the same picture of the window, only while VRChat says over OSC that its camera is open.
//! - **Not here**: translation, the corner's cards and the mismatch buttons.

pub mod capture;
pub mod channel;
pub mod game;
pub mod gesture;
pub mod keyboard;
pub mod lab;
pub mod panel;
pub mod pointing;
pub mod wheel;

use orbiters_core::app_log::OrLog as _;
use super::desk_shared::{self, hit, Clicks, Hints, Presence};
use super::detect::{MenuKey, Seen, Watcher};
use super::zones::{self, Around, Zone};
use super::Panels;
use crate::backend::Ctl;
use crate::ipc::{Client, Endpoint, Role};
use crate::state::{OrbitNeeds, OrbitPointing, OrbitStatus, Snapshot};
use anyhow::Context as _;
use capture::{Answer, Ask, CameraAsk, Looker, Measured, Search, Trouble};
use channel::{Channel, Pointer};
use egui::{pos2, vec2, Pos2, Vec2};
use game::{Finder, Game, Shot};
use gesture::{Gesture, Moved};
use keyboard::{Keys, Source};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSEventMask, NSEventModifierFlags, NSEventType, NSWorkspace};
use objc2_foundation::{NSActivityOptions, NSDate, NSDefaultRunLoopMode, NSNotification, NSProcessInfo};
use orbiters_i18n::mark;
use panel::{Arrow, Hold, Layer, PanelWindow, Pill, RestRing};
use wheel::{Fit, Scores};
use pointing::{Pointing, Turn, Who};
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
/// With the mouse the person's, its movement keeps the loop at [`TURN`] for this long.
const MOVED_FOR: Duration = Duration::from_millis(300);
/// How long macOS's pointer is waited for over a part, with the mouse the person's, before ours is drawn there (the engine looks 30 times a second where it is).
const SHOWS_WITHIN: Duration = Duration::from_millis(250);
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
            std::thread::Builder::new().name("orbit-watchdog".into()).spawn(move || loop {
                std::thread::sleep(Duration::from_millis(250));
                if hung(held.load(Ordering::Acquire), started.elapsed().as_millis() as u64 + 1) {
                    log_info!("orbit: the loop hung while it held the keyboard: ending"); // i18n: not shown (a log's line)
                    // SAFETY: the process ends here, whatever its main thread waits for.
                    unsafe { libc::_exit(70) };
                }
            }).or_log("a thread could not be started");
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

    fn take(&mut self, answer: &Answer, saw: &Measured) {
        if answer.look != self.number {
            return;
        }
        self.frames += 1;
        self.first.get_or_insert(answer.at.saturating_duration_since(self.began));
        if saw.wheel.of(&wheel::IN_PLACE) > self.wheel.of(&wheel::IN_PLACE) || self.frames == 1 {
            self.wheel = saw.wheel;
        }
        if let Some(fit) = saw.fit {
            if self.fit.is_none_or(|was| fit.scores.of(&wheel::FIT) > was.scores.of(&wheel::FIT)) {
                self.fit = Some(fit);
            }
        }
        self.icons = self.icons.max(saw.icons);
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
    /// VRChat's camera panel: where it shows, and when it was last seen there, as said to the service (part of what proves a live camera before a recording).
    camera: super::detect::CameraWatch,
    camera_seen_ns: Option<u64>,
    was_menu: bool,
    camera_logged: Option<bool>,
    pill: Pill,
    keys: Keys,
    hints: Hints,
    pointing: Pointing,
    hold: Hold,
    /// The engine's word of the mouse, and the folder it is in when that is not the game's own (the lab's).
    channel: Channel,
    host: Option<std::path::PathBuf>,
    was_pointer: Option<Pointer>,
    /// The mouse is the person's: the parts are clicked, the keyboard the game's.
    mouse: bool,
    /// Where it was last turn, when it last moved, and since when macOS hides its pointer over a part.
    was_at: Option<Pos2>,
    moved: Option<Instant>,
    hidden_since: Option<Instant>,
    /// The ring round VRChat's wheel, picked by the mouse's movement: since when it is, whether the key that opens the wheel began it, and the scroll wheel's count.
    gesture: Gesture,
    begun: bool,
    scrolls: Option<u32>,
    rest_ring: RestRing,
    was_lit: bool,
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
    logged: (Option<Seen>, bool, Option<Zone>, Option<Pointer>),
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
            camera: Default::default(),
            camera_seen_ns: None,
            was_menu: false,
            camera_logged: None,
            pill: Pill::default(),
            keys: Keys::default(),
            hints: Hints::default(),
            pointing: Pointing::default(),
            hold: Hold::new(mtm),
            channel: Channel::default(),
            host: None,
            was_pointer: None,
            mouse: false,
            was_at: None,
            moved: None,
            hidden_since: None,
            gesture: Gesture::default(),
            begun: false,
            scrolls: None,
            rest_ring: RestRing::default(),
            was_lit: false,
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
            logged: (None, false, None, None),
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

    /// The game has its keyboard again, at once (and its mouse, where it holds it).
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
        let pointing = self.last.map(|_| if self.channel.state().is_some() { OrbitPointing::Free } else { OrbitPointing::LeftOption });
        let status = OrbitStatus { note: note.map(str::to_string), needs, pointing, camera_panel_seen_ns: self.camera_seen_ns, ..OrbitStatus::default() };
        if self.said.as_ref() != Some(&status) {
            self.backend.orbit_status(status.clone());
            self.said = Some(status);
        }
    }

    /// Where the pointer starts on the parts of the menu `open`: the ring's first cell, the Quick Menu's first tile, the page's middle.
    fn first_point(&self, open: Option<Around>, size: Vec2) -> Option<Pos2> {
        let zones: &[Zone] = match open {
            _ if self.panels.sheet_open() => &[Zone::Takeover],
            // Its first tile.
            Some(Around::Camera) => &[Zone::CameraPeople],
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
    fn watched(&mut self, answer: &Answer, saw: &Measured) {
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
        let resting = !self.watcher.wants_frames() && self.panels.is_empty() && self.camera.shown().is_none();
        let due = self.asked_window.is_none_or(|t| now.duration_since(t) >= if resting { WINDOW_EVERY } else { WINDOW_EVERY_UP });
        if due || back || keys.menu.is_some() || keys.unknown {
            self.asked_window = Some(now);
            self.last = self.finder.find(snap.launcher.desktop_game, self.channel.state().map(|s| (s.window, s.frame)));
        }
        let found = self.last;
        // The engine's word of the mouse, of this very game's.
        let path = snap.launcher.desktop_game.map(|game| {
            let host = self.host.clone().unwrap_or_else(|| orbiters_core::obelisk::host_folder(&orbiters_core::obelisk::user_tmp(), game));
            orbiters_core::obelisk::pointer_channel(&host)
        });
        let moves = self.channel.turn(path.as_deref(), self.finder.server(), now);
        let pointer = self.channel.state().map(|s| s.pointer);
        let freed = self.was_pointer == Some(Pointer::Held) && pointer == Some(Pointer::Free);
        self.was_pointer = pointer;
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
            self.camera.reset();
            self.camera_seen_ns = None;
            self.was_menu = false;
            self.pill.hide();
            if self.watcher.wants_frames() || !self.panels.is_empty() {
                self.watcher.reset();
                self.panels.set_open(None, Vec::new(), &snap);
                self.windows.hide_all();
            }
            if holding {
                self.let_go();
            }
            self.mouse = false;
            self.gesture_over();
            self.panels.serve_menu(&snap);
            return self.end(now, REST);
        };
        // VRChat's camera panel: its window of another size, its panel goes with it now, not two looks later.
        let size = game.client();
        let camera_wanted = snap.launcher.camera && snap.settings.orbit.camera_tiles && self.allowed.0;
        if !camera_wanted {
            self.camera.reset();
        }
        self.camera.follow_size(size);
        // VRChat's keys: the game's alone, so not those pressed while the keyboard is ours.
        if !holding {
            if let Some(key) = keys.menu {
                self.new_look(Some(key), now);
                self.watcher.pressed(key, now);
                if key == MenuKey::Esc {
                    self.camera.esc(now);
                }
            } else if (keys.unknown || self.look_due || freed && front) && self.watcher.seen() == Seen::Resting {
                // (the mouse let go: a menu of VRChat's opened, whatever opened it)
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
            if let Some(saw) = answer.saw {
                self.saw.take(&answer, &saw);
                self.watched(&answer, &saw);
            }
            if let Some(look) = answer.camera.filter(|_| camera_wanted) {
                self.camera.look(look, answer.at);
            }
        }
        let looking = self.watcher.wants_frames();
        let camera_frames = camera_wanted && self.camera.wants_frames(snap.camera_open_said, now);
        let search = match (self.wheel_at, self.watcher.seen()) {
            (Some(fit), Seen::Open(Around::Wheel)) => Search::At(fit),
            // Esc opens none.
            (None, Seen::Looking | Seen::Open(Around::Wheel)) | (_, Seen::Looking) if self.saw.key != Some(MenuKey::Esc) => Search::Around,
            _ => Search::InPlace,
        };
        self.looker.ask((looking || camera_frames).then(|| Ask {
            window: game.window.id,
            owner: game.window.owner,
            shot: Shot::of(&game),
            menus: looking,
            camera: camera_frames.then(|| CameraAsk { client: size, shown: self.camera.shown(), open: snap.camera_open_said == Some(true) }),
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
        // A menu of VRChat's that went may have opened its camera (with OSC off, nothing else says so).
        let menu_up = matches!(seen, Seen::Open(_));
        if std::mem::replace(&mut self.was_menu, menu_up) && !menu_up {
            self.camera.menu_went(now);
        }
        let camera_panel = if camera_wanted { self.camera.tick(snap.camera_open_said, now) } else { None };
        self.camera_seen_ns = camera_panel.map(|_| {
            let at = orbiters_core::clock::now_ns();
            self.camera_seen_ns.filter(|seen| at.saturating_sub(*seen) < 500_000_000).unwrap_or(at)
        });
        self.camera_log(camera_panel, &snap, size);
        let open = match seen {
            Seen::Open(Around::Wheel) if snap.settings.orbit.around_wheel() => Some(Around::Wheel),
            Seen::Open(Around::QuickMenu) if snap.settings.orbit.around_quick_menu() => Some(Around::QuickMenu),
            _ => None,
        };
        // VRChat's menus first: they close its camera on the desktop.
        let open = open.or(camera_panel.filter(|_| !menu_up).map(|_| Around::Camera));
        let places = open.map_or_else(Vec::new, |menu| {
            let mut places = match (menu, camera_panel) {
                (Around::Camera, Some(panel)) => zones::camera_on_screen(panel, size),
                _ => zones::layout_as(menu, size, snap.settings.orbit.layout == orbiters_core::settings::OrbitLayout::Strip),
            };
            places.retain(|p| snap.launcher.camera || !p.zone.needs_the_camera_s_frames());
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
            self.mouse = false;
            self.follow_wheel(&game, seen, &keys, front, pointer, &moves, None, now);
            self.windows.hide_all();
            // What a 3D picture is doing, with no menu up to say it. (Its camera records its own stream, not this window.)
            let words = if front && snap.launcher.camera { self.panels.away_words(&snap) } else { None };
            self.pill.show(self.mtm, words, &game, panel::level(&game, true));
            return self.end(now, if looking { TURN } else { REST });
        }
        self.pill.hide();
        self.panels.follow(&snap, self.backend.library_version());
        self.panels.menu_turn(&snap);
        self.panels.step_springs(dt);
        let at = game.pixel(panel::pointer_at());
        if self.was_at.replace(at) != Some(at) {
            self.moved = Some(now);
        }
        let pressed = heard.iter().any(|h| matches!(h, Heard::Button(true)));
        self.point(&game, open, &keys, front, pointer, pressed, at, now);
        // Around the wheel, what the mouse's own movement points at; everywhere else, where the pointer is.
        let pushed = self.follow_wheel(&game, seen, &keys, front, pointer, &moves, open, now);
        let active = self.put || self.mouse;
        let at = pushed.unwrap_or(at);
        self.panels.set_pointing((self.put || pushed.is_some()) && !self.panels.sheet_open(), !snap.settings.reduce_motion);
        self.panels.set_key_caps(front && keys.option);
        let on = (active || pushed.is_some()).then(|| hit(&self.panels, at)).flatten();
        self.panels.hover(on);
        let opens = self.rest_on(&game, &snap, pushed, on.is_some_and(|(i, _)| self.panels.all().nth(i).is_some_and(|p| !p.zone.passes_clicks())), now);
        self.hints.follow(open, self.put || pushed.is_some());
        let keyless = pointer.is_some().then_some(snap.settings.orbit.wheel_pick);
        self.panels.set_hint(self.hints.now_on_a_mac(open, self.put || pushed.is_some(), keyless));
        if opens {
            self.press(true, &snap);
            self.press(false, &snap);
        }
        for event in heard {
            match event {
                // (on a pixel of a part, whose window took it)
                Heard::Button(down) => {
                    self.windows.clicked = true;
                    self.press(down, &snap);
                }
                Heard::Wheel(notches) if active && pushed.is_none() => self.panels.wheel(notches),
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
            self.windows.catch_at((active && pushed.is_none()).then_some(at));
        }
        let moving = self.mouse && self.moved.is_some_and(|t| now.duration_since(t) < MOVED_FOR);
        let quick = self.hold.held() || looking && self.watcher.look_every(now).is_zero() || self.panels.busy() || moving || pushed.is_some() || !moves.is_empty() && open == Some(Around::Wheel);
        self.end(now, if quick { TURN } else { REST })
    }

    /// Takes the keyboard or gives it back as [`Pointing`] says, and puts the pointer on the parts once the engine let it go.
    #[allow(clippy::too_many_arguments)]
    fn point(&mut self, game: &Game, open: Option<Around>, keys: &keyboard::Said, front: bool, pointer: Option<Pointer>, pressed: bool, at: Pos2, now: Instant) {
        let sheet = self.panels.sheet_open() && !self.panels.peek;
        let lost = self.held_since.is_some_and(|t| now.duration_since(t) >= KEYBOARD_WITHIN) && !self.hold.has_keyboard();
        let given_back = std::mem::take(&mut self.given_back);
        let typing = self.panels.typing();
        let who = self.pointing.turn(Turn { front, open, sheet, typing, option: keys.option, pointer, pressed, lost, given_back });
        self.mouse = who == Who::Mouse;
        let level = panel::level(game, true);
        if who != Who::Parts {
            if self.hold.held() {
                // Not in the middle of a click, whose end the game would hear alone.
                let since = *self.leaving.get_or_insert(now);
                let mid_click = front && open.is_some() && !lost && self.put && self.source.button() && now.duration_since(since) < BUTTON_WITHIN;
                if !mid_click {
                    self.let_go();
                }
            }
            // The mouse the person's, macOS shows its pointer over ours by itself, the engine permitting.
            let over = self.mouse && hit(&self.panels, at).is_some() && !panel::pointer_shows();
            let since = if over { Some(*self.hidden_since.get_or_insert(now)) } else { None };
            self.hidden_since = since;
            if since.is_some_and(|t| now.duration_since(t) >= SHOWS_WITHIN) {
                self.arrow.show(self.mtm, game, at, level);
            } else {
                self.arrow.hide();
            }
            return;
        }
        self.hidden_since = None;
        self.leaving = None;
        if !self.hold.held() {
            self.hold.take(game, level);
            self.held_since = Some(now);
            // The mouse was the person's already: where it is, is where they have it.
            self.put = matches!(pointer, Some(Pointer::Free | Pointer::Plain));
            return;
        }
        let held_for = self.held_since.map_or(Duration::ZERO, |t| now.duration_since(t));
        let gone = match pointer {
            // (its own word that it let go, and not waited for beyond any time it may take)
            Some(Pointer::Held) => held_for >= LETS_GO * 5,
            Some(_) => true,
            None => held_for >= LETS_GO,
        };
        if !self.put && self.hold.has_keyboard() && gone {
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

    /// The mouse's movement, where VRChat's wheel holds it: where it points on the ring, in pixels of the game's picture, once it was pushed past the wheel's rim.
    #[allow(clippy::too_many_arguments)]
    fn follow_wheel(&mut self, game: &Game, seen: Seen, keys: &keyboard::Said, front: bool, pointer: Option<Pointer>, moves: &[channel::Move], open: Option<Around>, now: Instant) -> Option<Pos2> {
        let clock = now.duration_since(self.started).as_secs_f64();
        if keys.menu == Some(MenuKey::R) {
            // VRChat's pointer is in its wheel's middle as it opens, some time before it is seen here.
            self.gesture.begin(clock);
            self.begun = true;
        }
        let up = seen == Seen::Open(Around::Wheel);
        let coming = seen == Seen::Looking && self.begun;
        let ours = front && pointer == Some(Pointer::Held) && !self.hold.held() && !(self.panels.sheet_open() && !self.panels.peek);
        if !(up || coming) || !ours {
            self.gesture_over();
            return None;
        }
        if !self.begun {
            self.gesture.begin(clock);
            self.begun = true;
        }
        let (center, radius) = self.wheel_at.map_or_else(|| zones::wheel(game.client()), |fit| fit.circle(game.client()));
        let moved: Vec<Moved> = moves.iter().map(|(x, y)| Moved { by: vec2(*x, *y), at: clock }).collect();
        self.gesture.moves(&moved, radius / game.scale as f32, open == Some(Around::Wheel), self.panels.second_ring());
        self.gesture.at().map(|at| center + gesture::shown_at(at, radius))
    }

    fn gesture_over(&mut self) {
        self.gesture.end();
        self.begun = false;
        self.scrolls = None;
        self.was_lit = false;
        self.rest_ring.hide();
    }

    /// What is pointed at by pushing past the rim: its arrow, the rest on it as it passes, and whether it opens now.
    fn rest_on(&mut self, game: &Game, snap: &Snapshot, pushed: Option<Pos2>, pressable: bool, now: Instant) -> bool {
        if !self.begun {
            return false;
        }
        let scrolls = self.source.scrolls();
        let scrolled = self.scrolls.replace(scrolls).is_some_and(|was| was != scrolls);
        let orbit = &snap.settings.orbit;
        let told = gesture::Now { at: now.duration_since(self.started).as_secs_f64(), scrolled, button: self.source.any_button(), pressable: pressable && pushed.is_some() };
        let (opens, passed) = self.gesture.turn(&told, orbit.wheel_pick, orbit.wheel_rest());
        let lit = self.gesture.at().is_some();
        if opens || lit != self.was_lit {
            // What the thresholds of the gesture are judged by after a real session.
            super::super::vr_log::line(&format!("desk: the ring {} at {:?} radii (wheel's radius {:.0} points), {} moves so far, opens {opens} (scroll wheel {scrolled})", if lit { "lit" } else { "out" }, self.gesture.at().map(|a| (a.x, a.y)), zones::wheel(game.client()).1 / game.scale as f32, self.gesture.counted())); // i18n: not shown (a log's line)
        }
        self.was_lit = lit;
        let level = panel::level(game, true);
        match pushed.filter(|_| self.gesture.at().is_some()) {
            Some(at) => {
                self.rest_ring.show(self.mtm, game, at, passed, level);
                self.arrow.show(self.mtm, game, at, level);
            }
            None => {
                self.rest_ring.hide();
                if pushed.is_some() {
                    self.arrow.hide();
                }
            }
        }
        opens
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
        let is = (Some(seen), self.hold.held(), self.windows.left_out, self.was_pointer);
        if is != self.logged {
            let after = now.saturating_duration_since(self.saw.began).as_millis();
            let (game, frames) = (self.last.map(|g| (g.window.id, g.window.layer, g.client(), g.scale)), self.looker.running());
            let key = self.saw.key;
            super::super::vr_log::line(&format!("desk: {seen:?}, {after} ms after the look {} began (key {key:?}); in front {}; mouse {:?}; keyboard held {}; window {game:?}; picture (runs, frames) {frames:?}; left out {:?}", self.saw.number, self.was_front, is.3, is.1, is.2)); // i18n: not shown (a log's line)
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

    /// The column beside VRChat's camera came or went, with why, in `vr-test.log`.
    fn camera_log(&mut self, panel: Option<egui::Rect>, snap: &Snapshot, size: Vec2) {
        let shows = panel.is_some();
        if self.camera_logged.replace(shows).unwrap_or(false) != shows {
            let (went, said) = (self.camera.take_went(), snap.camera_open_said);
            super::super::vr_log::line(&format!("desk camera: column {} at {panel:?} in {} x {} (osc {said:?})", if shows { "shown".into() } else { format!("gone ({went:?})") }, size.x, size.y)); // i18n: not shown (a log's line)
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
            mouse: self.mouse,
            pointer: self.was_pointer,
            lit: self.gesture.at().is_some().then(|| self.panels.all().find(|p| p.hover.is_some()).map(|p| p.zone)).flatten(),
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
    /// The mouse is the person's, the keyboard the game's.
    pub mouse: bool,
    pub pointer: Option<Pointer>,
    /// The part lit by pushing past VRChat's wheel's rim.
    pub lit: Option<Zone>,
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
