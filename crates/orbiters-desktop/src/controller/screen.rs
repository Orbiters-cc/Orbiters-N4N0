//! N4N0's menu in the tray service (contracts `Docs/orb.md`, Menu and Screen): on the avatar's projected screen when
//! the N4N0 prop has one, and the requests that open it in the SteamVR overlay otherwise.
//!
//! The screen's menu lives here, beside the OSC link: the orb thread writes its memory (the owner's through local
//! values, a block every step; everyone else's through the text channel, in turn with the panel), the screen's touch
//! points say where a finger is, and what the menu asks for runs on threads of its own (YouTube, calls, a picture).
//!
//! Everyone else's screen is built apart from the owner's ([`Frame::public`]): it never names the owner's friends, and
//! leaves out the owner's hover and presses, which changed with every move of the finger and held the rest back.
//!
//! A link copied from the results can be dropped in the world as a card (contracts `Docs/orb.md`, Link card): the screen
//! shows it frozen where it was for [`orb::CARD_SECONDS`] (`N4N0/Screen/Drop` for its owner, pointer 253 on the text
//! channel for everyone else), Orbiters is told so that whoever grabs it there copies its link, and a hand on a card
//! (`N4N0/Card/TouchL`, `TouchR`) copies the owner's own link, or asks Orbiters for the card dropped there.

use super::orb::{OrbRuntime, Source};
use super::{NoticeKind, Shared};
use orbiters_contracts::orb::{self, thumbnail};
use orbiters_core::account::{Api, Server};
use orbiters_core::osc::OscValue;
use orbiters_orb::menu::layout::{Content, ScreenPage};
use orbiters_orb::menu::screen::{self, Audience, Flags, Frame, Picture, LOCAL_VALUES};
use orbiters_orb::menu::touch::{self, Gesture, Pointer, Touch, LOST_GRACE};
use orbiters_orb::menu::{self, layout, Call, Card, Contact, Effect, KeyboardLayout, Menu, Page, Pages, PhotoLength, SuggestQueue};
use orbiters_orb::text::{Message, TextSender};
use orbiters_orb::youtube;
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The owner's screen takes a step at most this often, and at least this often: VRChat's animator reads one block a
/// frame, and two steps in one frame lose the first. Paced at two of VRChat's frames as the SteamVR overlay measures
/// them; without a measure, at 40 ms (two frames at 50 fps, one at 25).
const LOCAL_STEP_MIN: Duration = Duration::from_millis(28);
const LOCAL_STEP_MAX: Duration = Duration::from_millis(100);
const LOCAL_STEP_UNMEASURED: Duration = Duration::from_millis(40);
/// Once the owner's screen holds the whole page, blocks go again only this often (they repair one VRChat dropped).
const LOCAL_IDLE: Duration = Duration::from_millis(250);
/// A press shows at least this long, however short the touch.
const PRESS_SHOWN: Duration = Duration::from_millis(160);
/// The menu closes after this long without a touch, a press or a word.
const IDLE_CLOSE: Duration = Duration::from_secs(120);
/// A thumbnail that failed is tried again after this (times the tries), so many times.
const THUMB_RETRY: Duration = Duration::from_secs(3);
const THUMB_TRIES: u8 = 3;
/// VRChat sends back the `N4N0/Menu` the desktop set within this: the same value then is its echo, not the owner's.
const MENU_ECHO: Duration = Duration::from_secs(1);
/// After an avatar change, what VRChat sends of `N4N0/Menu` for this long is the loaded avatar's default.
const MENU_SETTLE: Duration = Duration::from_secs(2);
/// The overlay says its menu opened within this of being asked (else it is not there).
const OVERLAY_ANSWER: Duration = Duration::from_secs(2);
/// While looking points, the mouse is read this often (a click is down about a tenth of a second).
const GAZE_POLL: Duration = Duration::from_millis(15);
/// Clicks count only this long after the menu opened on the screen: the Action Menu's click that opened it is one, and
/// something is under the dot as it opens.
const CLICK_AFTER_OPEN: Duration = Duration::from_millis(500);
/// A click presses what the gaze rested on this long, not what it crossed mid-turn.
const GAZE_REST: Duration = Duration::from_millis(150);
/// A dropped card's freeze (pointer 253) goes again this often while it shows, for whoever loaded the avatar since.
const FREEZE_AGAIN: Duration = Duration::from_secs(4);
/// Two grabs closer than this are one (both hands, a hand brushing it twice).
const GRAB_AGAIN: Duration = Duration::from_millis(1500);

/// The mouse at a reading.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct MouseState {
    /// The left button is down.
    pub down: bool,
    /// The window in front is VRChat's.
    pub front: bool,
    /// VRChat shows the cursor: its Action Menu or Quick Menu is open, the view frozen (in mouse-look it hides it). Read
    /// only with VRChat in front.
    pub cursor: bool,
}

/// The left mouse button, the window in front and the cursor, as desktop mode's clicks read them (tests feed their own).
pub(super) trait Mouse: Send {
    /// The mouse now, VRChat being process `vrchat`.
    fn read(&mut self, vrchat: u32) -> MouseState;
}

/// This computer's mouse.
struct WindowsMouse;

impl Mouse for WindowsMouse {
    fn read(&mut self, vrchat: u32) -> MouseState {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
            use windows::Win32::UI::WindowsAndMessaging::{
                GetCursorInfo, GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, CURSORINFO, CURSOR_SHOWING, SM_SWAPBUTTON,
            };
            // SAFETY: plain reads of the input state; the process id and the cursor's state are written to locals that
            // outlive the calls.
            unsafe {
                // The left button as the owner set it up (swapped: the physical right one).
                let button = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
                let down = (GetAsyncKeyState(button.0 as i32) as u16) & 0x8000 != 0;
                let window = GetForegroundWindow();
                let mut pid = 0u32;
                let front = !window.is_invalid() && GetWindowThreadProcessId(window, Some(&mut pid as *mut u32)) != 0 && pid == vrchat;
                let cursor = front && {
                    let mut info = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
                    GetCursorInfo(&mut info).is_ok() && info.flags.0 & CURSOR_SHOWING.0 != 0
                };
                MouseState { down, front, cursor }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = vrchat;
            MouseState::default()
        }
    }
}

/// What a thread brings back.
enum Done {
    Suggestions { id: u64, text: String, result: anyhow::Result<Vec<String>> },
    Results { id: u64, more: bool, result: anyhow::Result<youtube::Results> },
    Picture { id: String, tries: u8, picture: Option<Picture> },
}

/// A result's thumbnail for the screen.
enum Thumb {
    /// On its way.
    Loading,
    Ready(Picture),
    /// The last try failed at `at`.
    Failed {
        at: Instant,
        tries: u8,
    },
}

/// What a step of the screen leaves to do once the orb's lock is let go.
#[derive(Default)]
pub(super) struct Followups {
    pub effects: Vec<Effect>,
    pub photo_due: bool,
    /// Suggestions to ask for now, and thumbnails to try again.
    suggest: Option<(u64, String)>,
    retry: Vec<String>,
}

impl Followups {
    /// Whether the menu did something others should hear of.
    pub fn changed(&self) -> bool {
        !self.effects.is_empty() || self.photo_due
    }
}

pub struct ScreenRuntime {
    /// N4N0's menu shows on the avatar's projected screen (the prop has it and it replaces the overlay, or VRChat runs
    /// in desktop mode), with this many text values and thumbnails of this many bytes (`N4N0/Screen/ThumbnailBytes`).
    pub available: bool,
    /// The prop has the projected screen, written through the text channel, wherever the menu shows: a dropped link
    /// card shows on it even while the menu is in the overlay.
    pub(super) present: bool,
    sync: Option<usize>,
    pub(super) thumbnail: usize,
    pub menu: Menu,
    owner: Audience,
    everyone: Option<Audience>,
    /// What `N4N0/Screen/Open` was last set to.
    open_sent: Option<bool>,
    /// Everyone else's screen must be told it closed.
    close_everyone: bool,
    next_local: Instant,
    /// The owner's screen holds the whole page: the next block goes again at `idle_until`.
    owner_settled: bool,
    idle_until: Instant,
    /// VRChat's frame time as the overlay measured it (milliseconds, the slowest tenth).
    frame_ms: Option<f32>,
    /// The screen's next turn on the text channel.
    next_synced: Instant,
    /// Whose turn it is when the panel and the screen both have something to send.
    screen_turn: bool,
    proximity: [[f32; 4]; 2],
    touched: [bool; 2],
    touch: [Touch; 2],
    /// The screen points with rays (version 4): each hand's ray (hit, ratio along it) and pointer, instead of fingers.
    pub(super) pointing: bool,
    ray: [(bool, f32); 2],
    pointers: [Pointer; 2],
    pub(super) hover: Option<usize>,
    pub(super) pressed: Option<(usize, Instant)>,
    holding: [bool; 2],
    /// The page hover and presses are for.
    page_shown: Option<ScreenPage>,
    /// Desktop mode with N4N0's menu on the screen: its owner points by looking and presses with a left click.
    gaze: bool,
    /// How near the eyes and the gaze point are to the touch points, new since the last step; where they look.
    eye: [f32; 4],
    gaze_point: [f32; 4],
    gazed: bool,
    looked_at: Option<(f32, f32)>,
    /// The left mouse button and the window in front, read only while looking points.
    pub(super) mouse: Box<dyn Mouse>,
    /// The button at the last reading (`None`: not read since looking began), and a click holds its press.
    mouse_down: Option<bool>,
    clicking: bool,
    /// Since when looking points with the menu open (clicks count [`CLICK_AFTER_OPEN`] later), and the element looked
    /// at, on its page, since when.
    look_began: Option<Instant>,
    rested: Option<(ScreenPage, usize, Instant)>,
    pictures: HashMap<String, Thumb>,
    suggest: SuggestQueue,
    /// What N4N0 says while the menu is open: on Home, a line at a time, the next once the last was read.
    answer: VecDeque<String>,
    answer_next: Option<Instant>,
    done_tx: mpsc::Sender<Done>,
    done_rx: mpsc::Receiver<Done>,
    last_activity: Instant,
    /// The overlay's menu: open, and whether a sentence N4N0 hears is what to search for.
    pub overlay_open: bool,
    pub overlay_dictation: bool,
    /// Bumped to ask the overlay to open its menu.
    pub menu_request: u32,
    /// The last request may close the overlay's menu when it is open.
    pub menu_toggle: bool,
    /// The last sentence for the overlay's search field, numbered.
    pub dictation: Option<(u32, String)>,
    /// When the 3D picture starts (a menu's timer).
    pub photo_at: Option<Instant>,
    /// When the overlay was last asked to open its menu, until it says it did (it counts as open meanwhile), and
    /// whether it is to close it once it did (asked to close before it answered).
    overlay_asked: Option<Instant>,
    overlay_close_after: bool,
    /// When the overlay was last asked to close its menu, until it says it did (it counts as closed meanwhile: the Menu
    /// toggle the owner just turned off is not turned on again).
    overlay_closing: Option<Instant>,
    /// The Menu toggle of N4N0's expressions menu (`N4N0/Menu`) as the avatar holds it, as far as the desktop knows
    /// (`None`: not known, after an avatar change); the values the desktop set it to lately (VRChat sends them back,
    /// and an echo asks nothing); until when what VRChat sends of it is a loaded avatar's default, not the owner's.
    menu_param: Option<bool>,
    menu_sent: Vec<(bool, Instant)>,
    menu_settle_until: Option<Instant>,
    /// The media reading the menu shows (its version: [`crate::media::Watcher::reading`]).
    media_seen: u64,
    /// The saved states the menu lists (the states' version, the avatar they are for).
    states_seen: Option<(u64, Option<String>)>,
    /// When the dropped card goes; what `N4N0/Screen/Drop` was last set to.
    card_until: Option<Instant>,
    drop_sent: Option<bool>,
    /// Everyone else's copy of the card was told to freeze (pointer 253) since the screen last cleared; it is told
    /// again ([`FREEZE_AGAIN`]) when due.
    frozen: bool,
    freeze_due: bool,
    freeze_next: Option<Instant>,
    /// The hands on a card (`N4N0/Card/TouchL`, `TouchR`), and when a grab was last taken.
    grab: [bool; 2],
    grab_at: Option<Instant>,
}

impl Default for ScreenRuntime {
    fn default() -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        let now = Instant::now();
        Self {
            available: false,
            sync: None,
            menu: Menu::default(),
            owner: Audience::new(LOCAL_VALUES, thumbnail::SIZES[0]),
            thumbnail: thumbnail::SIZES[0],
            everyone: None,
            open_sent: None,
            close_everyone: false,
            next_local: now,
            owner_settled: false,
            idle_until: now,
            frame_ms: None,
            next_synced: now,
            screen_turn: true,
            proximity: [[0.0; 4]; 2],
            touched: [false; 2],
            touch: [Touch::default(); 2],
            pointing: false,
            ray: [(false, 1.0); 2],
            pointers: [Pointer::default(); 2],
            hover: None,
            pressed: None,
            holding: [false; 2],
            page_shown: None,
            gaze: false,
            eye: [0.0; 4],
            gaze_point: [0.0; 4],
            gazed: false,
            looked_at: None,
            mouse: Box::new(WindowsMouse),
            mouse_down: None,
            clicking: false,
            look_began: None,
            rested: None,
            pictures: HashMap::new(),
            suggest: SuggestQueue::default(),
            answer: VecDeque::new(),
            answer_next: None,
            done_tx,
            done_rx,
            last_activity: now,
            overlay_open: false,
            overlay_dictation: false,
            menu_request: 0,
            menu_toggle: false,
            dictation: None,
            photo_at: None,
            overlay_asked: None,
            overlay_close_after: false,
            overlay_closing: None,
            menu_param: None,
            menu_sent: Vec::new(),
            menu_settle_until: None,
            media_seen: 0,
            states_seen: None,
            card_until: None,
            drop_sent: None,
            frozen: false,
            freeze_due: false,
            freeze_next: None,
            grab: [false; 2],
            grab_at: None,
            present: false,
        }
    }
}

impl ScreenRuntime {
    /// The avatar changed, or where the menu shows: whether the menu shows on its screen (`available`), whether it has
    /// one at all (`present`: a dropped card shows there), with `sync` text values and thumbnails of `thumbnail` bytes
    /// (its memory is unknown again). The same avatar looked at again changes nothing.
    pub fn avatar(&mut self, available: bool, present: bool, sync: Option<usize>, thumbnail: usize) {
        let present = present && sync.is_some_and(|n| screen::fits(n, thumbnail));
        let available = available && present;
        if available == self.available && present == self.present && (!present || (sync == self.sync && thumbnail == self.thumbnail)) {
            return;
        }
        self.available = available;
        self.present = present;
        self.sync = sync.filter(|_| present);
        self.thumbnail = thumbnail;
        // A loaded avatar's copies start clear; where the text channel's pointer stands is not known.
        self.everyone = self.sync.map(|n| {
            let mut everyone = Audience::new(n, thumbnail);
            everyone.cleared(false);
            everyone
        });
        self.owner = Audience::new(LOCAL_VALUES, thumbnail);
        self.owner.forget();
        self.open_sent = None;
        self.frozen = false;
        // A card stays while the screen does (the menu moved to the overlay); the menu goes where it shows now.
        if !self.present {
            self.end_card();
            self.drop_sent = None;
        }
        if !self.available && !self.menu.card_shown() {
            self.menu.close();
        }
    }

    /// A copied link can be dropped as a card here: the screen is of version 5 or later.
    pub(super) fn card_ready(&self) -> bool {
        self.present && self.menu.pages.states_cards()
    }

    /// The card the screen shows, for the window.
    pub(super) fn card_view(&self) -> Option<crate::state::CardView> {
        let card = self.menu.card.as_ref().filter(|_| self.menu.card_shown())?;
        Some(crate::state::CardView { title: card.title.clone(), link: card.link.clone(), left: card.left })
    }

    /// The dropped card goes (its time is up, the menu opened again, the screen went): what it showed goes with it.
    fn end_card(&mut self) {
        if self.menu.end_card().is_some() {
            self.hover = None;
            self.pressed = None;
        }
        self.card_until = None;
        self.freeze_due = false;
        self.freeze_next = None;
    }

    /// A hand on a dropped card, the owner's own or someone else's (`right`: `N4N0/Card/TouchR`), now `on`: whether it
    /// is a grab (either hand coming on, not twice in [`GRAB_AGAIN`]).
    pub(super) fn card_touch(&mut self, right: bool, on: bool, now: Instant) -> bool {
        let before = self.grab.iter().any(|h| *h);
        self.grab[right as usize] = on;
        let grabbed = !before && on && self.grab_at.is_none_or(|t| now.duration_since(t) >= GRAB_AGAIN);
        if grabbed {
            self.grab_at = Some(now);
        }
        grabbed
    }

    /// The app quits: the menu closes, and the owner's screen counts as told so (the quit's last message tells it), so
    /// a step still under way sends nothing more.
    pub(super) fn quit(&mut self) {
        let _ = self.menu.close();
        self.open_sent = Some(false);
    }

    /// Messages to VRChat were lost: neither screen's memory is known any more (each is cleared at its next step), nor
    /// whether it is open.
    pub(super) fn lost(&mut self) {
        self.owner.forget();
        if let Some(everyone) = self.everyone.as_mut() {
            everyone.forget();
        }
        self.open_sent = None;
        // Said again (the opposite of what it now is was the last word VRChat may have).
        if self.drop_sent.is_some() {
            self.drop_sent = Some(!self.menu.card_shown());
        }
    }

    /// Whether N4N0's menu shows: on the avatar's screen when it has one, else in the overlay (asked to open or close
    /// it, it counts as open or closed until it says, or does not answer in time).
    fn menu_shown(&self, now: Instant) -> bool {
        if self.available {
            // A dropped card is not the menu: the Menu toggle goes off while it shows.
            self.menu.open && !self.menu.card_shown()
        } else {
            let closing = self.overlay_closing.is_some_and(|at| now.duration_since(at) < OVERLAY_ANSWER);
            (self.overlay_open && !closing) || self.overlay_asked.is_some_and(|at| now.duration_since(at) < OVERLAY_ANSWER)
        }
    }

    /// Closes the overlay's menu: asks it to when it is open, or as soon as it says it opened (a request the overlay
    /// has not read would otherwise be taken as an open).
    fn overlay_close(&mut self) {
        if self.overlay_open {
            self.menu_request = self.menu_request.wrapping_add(1);
            self.menu_toggle = true;
            self.overlay_closing = Some(Instant::now());
        } else if self.overlay_asked.is_some() {
            self.overlay_close_after = true;
            self.overlay_asked = None;
        }
    }

    /// An avatar changed or went: its Menu toggle is not known, and what VRChat sends of it a moment is the loaded
    /// avatar's default.
    pub(super) fn menu_param_reset(&mut self, now: Instant) {
        self.menu_param = None;
        self.menu_settle_until = Some(now + MENU_SETTLE);
    }

    /// The value to set `N4N0/Menu` to, when the avatar's toggle no longer says whether the menu shows (another trigger
    /// opened it, it closed on its own), taken as sent.
    pub(super) fn menu_sync(&mut self, now: Instant) -> Option<bool> {
        if self.menu_settle_until.is_some_and(|t| now < t) {
            return None;
        }
        let shown = self.menu_shown(now);
        if self.menu_param == Some(shown) {
            return None;
        }
        // VRChat sends a value back only when it changes the avatar's: one set while that was not known may never come.
        let known = self.menu_param.is_some();
        self.menu_param = Some(shown);
        self.menu_sent.retain(|(_, at)| now.duration_since(*at) < MENU_ECHO);
        if known {
            self.menu_sent.push((shown, now));
        }
        Some(shown)
    }

    /// `N4N0/Menu` as VRChat sent it: what the owner asks (`Some(open)`), or nothing: the echo of a value the desktop
    /// set, a loaded avatar's default, the menu already so.
    pub(super) fn menu_toggled(&mut self, on: bool, now: Instant) -> Option<bool> {
        self.menu_sent.retain(|(_, at)| now.duration_since(*at) < MENU_ECHO);
        // VRChat sends back what the desktop set, in order: this echo is done, and so are the sends before it.
        if let Some(i) = self.menu_sent.iter().position(|&(sent, _)| sent == on) {
            self.menu_sent.drain(..=i);
            return None;
        }
        // The owner's own (or the avatar's default): whatever the desktop set before was sent back already.
        self.menu_sent.clear();
        self.menu_param = Some(on);
        if self.menu_settle_until.is_some_and(|t| now < t) {
            return None;
        }
        (on != self.menu_shown(now)).then_some(on)
    }

    /// The owner's step: two of VRChat's frames, as measured.
    fn local_step(&self) -> Duration {
        self.frame_ms.map_or(LOCAL_STEP_UNMEASURED, |ms| Duration::from_secs_f32(2.0 * ms / 1000.0)).clamp(LOCAL_STEP_MIN, LOCAL_STEP_MAX)
    }

    /// When the screen next has something to do (for the orb thread's sleep).
    pub(super) fn next_due(&self, now: Instant) -> Option<Instant> {
        let mut due: Option<Instant> = None;
        let mut at = |t: Instant| due = Some(due.map_or(t, |d: Instant| d.min(t)));
        if let Some(t) = self.photo_at {
            at(t);
        }
        // The Menu toggle is put in step once a loaded avatar settled, or once the overlay did not answer.
        if let Some(t) = self.menu_settle_until.filter(|t| *t > now) {
            at(t);
        }
        for asked in [self.overlay_asked, self.overlay_closing].into_iter().flatten() {
            if asked + OVERLAY_ANSWER > now {
                at(asked + OVERLAY_ANSWER);
            }
        }
        // A dropped card's countdown moves each second, and it goes at its end.
        if let Some(until) = self.card_until.filter(|_| self.menu.card_shown()) {
            let left = until.saturating_duration_since(now);
            let to_next = if left.subsec_nanos() == 0 { Duration::from_secs(1) } else { Duration::from_nanos(left.subsec_nanos() as u64) };
            at((now + to_next).min(until));
        }
        let card = self.menu.card_shown();
        if (self.available || card) && self.menu.open {
            at(if self.owner_settled { self.next_local.max(self.idle_until) } else { self.next_local });
            // A press shown ends once its time is up and the finger let go (which a touch point's change says).
            if let Some((_, t)) = self.pressed.filter(|_| !self.holding.iter().any(|h| *h) && !self.clicking) {
                at(t + PRESS_SHOWN);
            }
            // Looking points: the gaze and the mouse are read every few milliseconds.
            if self.gaze {
                at(now + GAZE_POLL);
            }
            if !card {
                at(self.last_activity + IDLE_CLOSE);
            }
            // What N4N0 said moves on only while Home shows.
            if let Some(t) = self.answer_next.filter(|_| self.menu.page == Page::Home) {
                at(t);
            }
            if let Some(t) = self.suggest.next_due() {
                at(t);
            }
            // A finger or a dot no longer heard of is let go of a moment later.
            if self.touch.iter().any(|t| t.finger.is_some()) || self.pointers.iter().any(|p| p.at.is_some()) {
                at(now + LOST_GRACE);
            }
        }
        // The screen's turn on the text channel; one already due waits for the panel's block to end, whose own steps
        // wake the orb thread.
        if self.everyone.is_some() && ((self.menu.open && (self.available || card)) || self.close_everyone) && self.next_synced > now {
            at(self.next_synced);
        }
        due
    }

    /// Testing: the dropped card's time is up now.
    #[cfg(test)]
    pub(super) fn card_ends_now(&mut self) {
        if self.card_until.is_some() {
            self.card_until = Some(Instant::now());
        }
    }

    /// Testing: nothing happened on the menu for longer than it stays open idle.
    #[cfg(test)]
    pub(super) fn idle_long_ago(&mut self) {
        self.last_activity = Instant::now().checked_sub(IDLE_CLOSE + Duration::from_secs(1)).unwrap_or(self.last_activity);
    }

    /// Seconds before the 3D picture starts.
    pub fn countdown(&self, now: Instant) -> Option<u32> {
        self.photo_at.map(|at| at.saturating_duration_since(now).as_secs_f32().ceil() as u32)
    }

    /// Looking points (desktop mode with N4N0's menu on the screen) or no longer does: what was read of it goes.
    pub(super) fn set_gaze(&mut self, on: bool) {
        if self.gaze == on {
            return;
        }
        self.gaze = on;
        self.eye = [0.0; 4];
        self.gaze_point = [0.0; 4];
        self.gazed = false;
        self.looked_at = None;
        self.mouse_down = None;
        self.clicking = false;
        self.look_began = None;
        self.rested = None;
        self.hover = None;
    }

    /// The eyes or the gaze point near the screen (`N4N0/Screen/Eye0` …, `N4N0/Screen/Gaze0` …): kept only while looking
    /// points (in VR the fingers do). Whether `name` is one.
    pub fn gaze_value(&mut self, name: &str, value: f32) -> bool {
        let (values, rest) = if let Some(rest) = name.strip_prefix(screen::local::EYE_PREFIX) {
            (&mut self.eye, rest)
        } else if let Some(rest) = name.strip_prefix(screen::local::GAZE_PREFIX) {
            (&mut self.gaze_point, rest)
        } else {
            return false;
        };
        if let Some(i) = rest.parse::<usize>().ok().filter(|i| *i < 4 && self.gaze) {
            values[i] = value;
            self.gazed = true;
        }
        true
    }

    /// Desktop mode's looking and clicking at a step (`vrchat`: VRChat's process): where the owner looks is hovered, and
    /// a left click presses it; letting go ends the press. A click counts with VRChat in front and its cursor hidden
    /// (mouse-look), [`CLICK_AFTER_OPEN`] after the menu opened, on what the gaze rested on for [`GAZE_REST`]; else it
    /// presses nothing, as with nothing looked at.
    fn look_and_click(&mut self, vrchat: Option<u32>, now: Instant, effects: &mut Vec<Effect>) {
        if !(self.gaze && self.available && self.menu.open && !self.menu.card_shown()) {
            // The mouse is not read: a click before looking began is no click.
            self.mouse_down = None;
            self.clicking = false;
            self.look_began = None;
            self.rested = None;
            return;
        }
        let began = *self.look_began.get_or_insert(now);
        let layout = Frame::layout(&self.menu);
        if std::mem::take(&mut self.gazed) {
            self.looked_at = touch::look(self.eye, self.gaze_point);
        }
        let menu = &self.menu;
        let looked = self.looked_at.and_then(|(x, y)| layout::hit_pointable(layout, menu, x, y));
        self.hover = looked.map(|(i, _)| i);
        // How long the gaze rests on what it looks at.
        let page = ScreenPage::of(&self.menu);
        self.rested = match (looked, self.rested) {
            (Some((i, _)), Some((p, j, since))) if p == page && j == i => Some((p, j, since)),
            (Some((i, _)), _) => Some((page, i, now)),
            (None, _) => None,
        };
        let Some(vrchat) = vrchat else { return };
        let mouse = self.mouse.read(vrchat);
        let click = self.mouse_down == Some(false) && mouse.down && mouse.front && !mouse.cursor && now.duration_since(began) >= CLICK_AFTER_OPEN;
        let rested = self.rested.is_some_and(|(_, _, since)| now.duration_since(since) >= GAZE_REST);
        if let Some((index, target)) = looked.filter(|_| click && rested) {
            self.last_activity = now;
            self.pressed = Some((index, now));
            self.clicking = true;
            effects.extend(self.menu.press(target));
        }
        if !mouse.down {
            self.clicking = false;
        }
        self.mouse_down = Some(mouse.down);
    }

    /// A hand's pointing ray (`N4N0/Screen/Ray/L_Hit`, `…_Ratio`, the right hand's): whether it meets the screen, where
    /// along it.
    pub fn ray_value(&mut self, name: &str, value: &OscValue) -> bool {
        let Some(rest) = name.strip_prefix(screen::local::RAY_PREFIX) else { return false };
        let hand = match rest.chars().next() {
            Some('L') => 0,
            Some('R') => 1,
            _ => return true,
        };
        match (&rest[1..], value) {
            ("_Hit", v) => self.ray[hand].0 = v.truthy(),
            ("_Ratio", OscValue::Float(v)) => self.ray[hand].1 = *v,
            _ => return true,
        }
        self.touched[hand] = true;
        true
    }

    /// The owner's index finger near the screen: one of the eight touch points (`N4N0/Screen/Touch/L0` …).
    pub fn touch_point(&mut self, name: &str, value: f32) -> bool {
        let Some(rest) = name.strip_prefix(screen::local::TOUCH_PREFIX) else { return false };
        let mut chars = rest.chars();
        let hand = match chars.next() {
            Some('L') => 0,
            Some('R') => 1,
            _ => return true,
        };
        if let Some(i) = chars.as_str().parse::<usize>().ok().filter(|i| *i < 4) {
            self.proximity[hand][i] = value;
            self.touched[hand] = true;
        }
        true
    }
}

impl ScreenRuntime {
    /// The owner's screen dropped or not, and shown or not, as the card and the menu say now: dropped first, so that the
    /// screen stays shown and frozen whatever the Menu toggle says next. A screen never dropped is never told it is not.
    fn sync_open_and_drop(&mut self, out: &mut Vec<Message>) {
        let dropped = self.menu.card_shown();
        if self.drop_sent != Some(dropped) && (dropped || self.drop_sent.is_some()) {
            self.drop_sent = Some(dropped);
            out.push(screen::local_drop(dropped));
        }
        if self.open_sent != Some(self.menu.open) {
            self.open_sent = Some(self.menu.open);
            out.push(screen::local_open(self.menu.open));
            if !self.menu.open {
                self.close_everyone = true;
            }
        }
    }
}

/// What N4N0 says, cut as the Home of a display with `pages` shows it, a piece at a time: a line on the four- and
/// six-tile Home, what its place holds on the seven-tile one.
fn answer_pieces(text: &str, pages: Pages) -> Vec<String> {
    if pages.states_cards() {
        return layout::heard_pages(text);
    }
    let home = layout::cached(ScreenPage::home(pages));
    let cols = home.slots.iter().find(|s| s.content == Content::Heard).map_or(60, |s| s.cols as usize - 2);
    menu::pieces(text, cols)
}

/// The press sound, as a copy's feedback in VR (the overlay's own, computed the first time).
fn copy_sound() {
    if cfg!(test) {
        return;
    }
    static SOUND: std::sync::OnceLock<(orbiters_sound::Output, std::sync::Arc<orbiters_sound::Sound>)> = std::sync::OnceLock::new();
    let _ = std::thread::Builder::new().name("copy-sound".into()).spawn(|| {
        let (output, sound) = SOUND.get_or_init(|| {
            use orbiters_sound::{render, sounds, Output, RATE};
            (Output::new(), std::sync::Arc::new(render(&sounds::ROCKER, RATE)))
        });
        output.play(sound);
    });
}

/// The ISO 639-1 code of this computer's language, for YouTube.
fn language() -> String {
    let locale = super::orb::locale();
    let code = locale.split(['-', '_']).next().unwrap_or("").to_lowercase();
    if code.len() == 2 {
        code
    } else {
        "en".into()
    }
}

/// The keyboard's letters for the setting (Auto: this computer's keyboard).
pub fn keyboard(setting: orbiters_core::settings::KeyboardLayout) -> KeyboardLayout {
    use orbiters_core::settings::KeyboardLayout as Setting;
    match setting {
        Setting::Qwerty => KeyboardLayout::Qwerty,
        Setting::Azerty => KeyboardLayout::Azerty,
        Setting::Auto => {
            #[cfg(windows)]
            {
                // SAFETY: no arguments that could be invalid; 0 is the calling thread.
                let layout = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout(0) };
                let language = (layout.0 as usize & 0xffff) as u16;
                // French (0x0c), and Belgian French; Swiss and Canadian French keep QWERTZ/QWERTY.
                if language & 0x3ff == 0x0c && matches!(language >> 10, 0x01 | 0x02) {
                    return KeyboardLayout::Azerty;
                }
            }
            KeyboardLayout::Qwerty
        }
    }
}

pub fn photo_length(length: orbiters_core::settings::Photo3dLength) -> PhotoLength {
    use orbiters_core::settings::Photo3dLength as L;
    match length {
        L::Quick => PhotoLength::Quick,
        L::Standard => PhotoLength::Standard,
        L::Full => PhotoLength::Full,
    }
}

pub fn photo_setting(length: PhotoLength) -> orbiters_core::settings::Photo3dLength {
    use orbiters_core::settings::Photo3dLength as L;
    match length {
        PhotoLength::Quick => L::Quick,
        PhotoLength::Standard => L::Standard,
        PhotoLength::Full => L::Full,
    }
}

impl super::Controller {
    /// Something asked for N4N0's menu (the wake word alone, a hand on N4N0, a controller button): it opens on the
    /// avatar's screen when there is one, else in the overlay; N4N0 listens either way. Opening it again closes it.
    pub(super) fn menu_open(self: &Shared, toggle: bool) {
        let settings = self.settings();
        let mut rt = self.orb.lock().unwrap();
        // Opening the menu again takes a dropped card back (on the screen, the menu replaces it).
        if rt.screen.menu.card_shown() {
            rt.screen.end_card();
        }
        if rt.screen.available {
            if rt.screen.menu.open {
                if toggle {
                    let effects = rt.screen.menu.close();
                    drop(rt);
                    self.screen_effects(effects);
                }
                return;
            }
            let keyboard = keyboard(settings.orb.menu.keyboard);
            let s = &mut rt.screen;
            s.menu.layout = keyboard;
            s.menu.photo_timer = settings.photo_3d_timer;
            s.menu.photo_length = photo_length(settings.photo_3d);
            s.menu.open();
            s.owner.forget();
            s.last_activity = Instant::now();
            s.hover = None;
            s.pressed = None;
            s.page_shown = None;
        } else {
            // The overlay opens its menu (or closes it, when it is open and this was a toggle).
            let s = &mut rt.screen;
            s.menu_request = s.menu_request.wrapping_add(1);
            s.menu_toggle = toggle;
            if toggle && s.overlay_open {
                // A close, which starts no listen (as on the screen).
                s.overlay_closing = Some(Instant::now());
                drop(rt);
                self.orb_publish();
                return;
            }
            s.overlay_asked = Some(Instant::now());
            s.overlay_close_after = false;
            s.overlay_closing = None;
        }
        drop(rt);
        self.menu_listen();
        self.orb_publish();
    }

    /// Closes N4N0's menu: the screen's, or the overlay's (asked to when it is open, or as soon as it says it opened).
    pub(super) fn menu_close(self: &Shared) {
        let effects = {
            let mut rt = self.orb.lock().unwrap();
            let s = &mut rt.screen;
            if s.available {
                s.menu.close()
            } else {
                s.overlay_close();
                Vec::new()
            }
        };
        self.screen_effects(effects);
        self.orb_publish();
    }

    /// The Menu toggle of N4N0's expressions menu (`N4N0/Menu`, on the OSC action thread): on opens N4N0's menu as the
    /// binding does, off closes it. VRChat sending back what the desktop set asks nothing.
    pub(super) fn menu_toggled(self: &Shared, on: bool) {
        let asked = self.orb.lock().unwrap().screen.menu_toggled(on, Instant::now());
        match asked {
            Some(true) => self.menu_open(false),
            Some(false) => self.menu_close(),
            None => {}
        }
    }

    /// N4N0 listens to one sentence (as a hand on N4N0 does), unless it already listens.
    fn menu_listen(self: &Shared) {
        let settings = self.settings().orb;
        if !settings.enabled || !settings.assistant {
            return;
        }
        {
            let rt = self.orb.lock().unwrap();
            if rt.listening.is_some() || !rt.voice.installed {
                return;
            }
        }
        self.orb.lock().unwrap().listening = Some((Source::Touch, Instant::now()));
        self.voice_listen(Source::Touch);
    }

    /// The overlay's menu opened, closed, or moved to or from its search page.
    pub(super) fn menu_overlay_state(self: &Shared, open: bool, dictation: bool) {
        let listen = {
            let mut rt = self.orb.lock().unwrap();
            let s = &mut rt.screen;
            let opened = open && !s.overlay_open;
            if opened && s.menu.card_shown() {
                s.end_card();
            }
            s.overlay_open = open;
            s.overlay_dictation = open && dictation;
            if open {
                s.overlay_asked = None;
            } else {
                s.overlay_closing = None;
            }
            if open && std::mem::take(&mut s.overlay_close_after) {
                // Closed before it opened (the Menu toggle turned off meanwhile): the overlay's menu, even when the
                // avatar's screen shows N4N0's menu by now.
                s.overlay_close();
                false
            } else {
                opened
            }
        };
        if listen {
            self.menu_listen();
        }
        self.orb_publish();
    }

    /// A sentence N4N0 heard while a menu is open: the screen's search page takes it, or the overlay's (`true`: taken).
    pub(super) fn menu_heard(self: &Shared, text: &str) -> bool {
        let mut rt = self.orb.lock().unwrap();
        if rt.screen.menu.open {
            rt.screen.last_activity = Instant::now();
            if let Some(effects) = rt.screen.menu.heard(text) {
                drop(rt);
                self.screen_effects(effects);
                return true;
            }
            return false;
        }
        if rt.screen.overlay_dictation {
            let n = rt.screen.dictation.as_ref().map_or(1, |(n, _)| n.wrapping_add(1));
            rt.screen.dictation = Some((n, text.trim().to_string()));
            drop(rt);
            self.orb_publish();
            return true;
        }
        false
    }

    /// A 3D picture in `seconds` (now when 0); the menus count down.
    pub(super) fn photo_3d_in(self: &Shared, seconds: u32) {
        if seconds == 0 {
            let ctl = self.clone();
            std::thread::spawn(move || {
                if let Err(e) = ctl.take_3d_picture() {
                    ctl.notify(NoticeKind::Warning, e.to_string());
                }
            });
            return;
        }
        self.orb.lock().unwrap().screen.photo_at = Some(Instant::now() + Duration::from_secs(seconds as u64));
        self.orb_publish();
    }

    /// Drops a link card in the world (contracts `Docs/orb.md`, Link card): the avatar's screen shows it, frozen where it
    /// is, for [`orb::CARD_SECONDS`]; Orbiters is told, so that whoever grabs it in this instance copies its link too.
    /// From N4N0's menu on the screen, the overlay's or the window's Home.
    pub(super) fn card_drop(self: &Shared, link: String, title: String, video: Option<String>) {
        if !orbiters_orb::drops::copyable(&link) {
            self.notify(NoticeKind::Warning, "Only web links can be dropped as cards.");
            return;
        }
        let fetch = {
            let mut rt = self.orb.lock().unwrap();
            if !rt.screen.card_ready() {
                drop(rt);
                self.notify(NoticeKind::Info, "This avatar's N4N0 cannot drop cards: update N4N0 in My Avatar, then upload the avatar again.");
                return;
            }
            let now = Instant::now();
            let s = &mut rt.screen;
            s.end_card();
            s.menu.show_card(Card { link: link.clone(), title: title.clone(), video: video.clone(), left: orb::CARD_SECONDS });
            s.card_until = Some(now + Duration::from_secs(orb::CARD_SECONDS as u64));
            s.last_activity = now;
            let fetch = video.filter(|id| !matches!(s.pictures.get(id), Some(Thumb::Ready(_))));
            rt.wake();
            fetch
        };
        if let Some(id) = fetch {
            self.screen_thumbnails(vec![id]);
        }
        // Orbiters keeps it for whoever grabs it in this instance (a member's card only).
        let instance = self.snapshot.lock().unwrap().vrchat.world.instance_hash();
        match (self.orb_account(), instance) {
            (Some((api, token)), Some(instance)) => {
                let ctl = self.clone();
                std::thread::spawn(move || {
                    if let Err(e) = orbiters_orb::drops::post(&api, &token, &instance, &link, &title) {
                        ctl.notify(NoticeKind::Warning, format!("People around cannot copy your card's link: {e:#}"));
                    }
                });
            }
            (None, Some(_)) => self.notify(NoticeKind::Info, "Sign in to Orbiters so that people around can copy your card's link too."),
            _ => {}
        }
        self.orb_publish();
    }

    /// A hand on a dropped card (on the OSC action thread): the owner's own card's link is copied; with none out, someone
    /// else's card was grabbed, and Orbiters gives the newest dropped in this instance.
    pub(super) fn card_grab(self: &Shared) {
        let own = self.orb.lock().unwrap().screen.card_view().map(|c| (c.link, c.title));
        if let Some((link, title)) = own {
            self.card_copied(&link, &title);
            return;
        }
        let Some(instance) = self.snapshot.lock().unwrap().vrchat.world.instance_hash() else { return };
        let api = self.orb_account().map(|(api, _)| api).unwrap_or_else(|| Api::new(Server::current()));
        let ctl = self.clone();
        std::thread::spawn(move || match orbiters_orb::drops::latest(&api, &instance) {
            Ok(Some(card)) => ctl.card_copied(&card.link, &card.title),
            Ok(None) => ctl.notify(NoticeKind::Info, "That card is gone: its link can no longer be copied."),
            Err(e) => ctl.notify(NoticeKind::Warning, format!("Could not copy the card's link: {e:#}")),
        });
    }

    /// A card's link in the clipboard, with the press sound and a toast, as Copy does.
    fn card_copied(&self, link: &str, title: &str) {
        match super::orb::copy_to_clipboard(link) {
            Ok(()) => {
                copy_sound();
                let what = match title.trim() {
                    "" => "the card's link".to_string(),
                    title => format!("\"{}\"", layout::ellipsis(title, 60)),
                };
                self.notify(NoticeKind::Success, format!("Copied {what}: paste it in VRChat."));
            }
            Err(e) => self.notify(NoticeKind::Error, e),
        }
    }

    /// Runs `fetch` on a thread of its own; what it brings back goes to the screen's next step, which it wakes.
    fn screen_fetch(self: &Shared, fetch: impl FnOnce() -> Done + Send + 'static) {
        let ctl = self.clone();
        let tx = self.orb.lock().unwrap().screen.done_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(fetch());
            ctl.orb_wake();
        });
    }

    /// Fetches the thumbnails among `ids` the screen has not got (nor is getting), the page shown first; one that
    /// failed is tried again a little later, a few times.
    fn screen_thumbnails(self: &Shared, ids: Vec<String>) {
        let wanted: Vec<(String, u8)> = {
            let mut rt = self.orb.lock().unwrap();
            let s = &mut rt.screen;
            // Keep the pictures of the last few pages only.
            if s.pictures.len() > 60 {
                s.pictures.retain(|id, _| ids.contains(id));
            }
            let now = Instant::now();
            ids.into_iter()
                .filter_map(|id| {
                    let tries = match s.pictures.get(&id) {
                        None => 0,
                        Some(Thumb::Failed { at, tries }) if *tries < THUMB_TRIES && now >= *at + THUMB_RETRY * *tries as u32 => *tries,
                        Some(_) => return None,
                    };
                    s.pictures.insert(id.clone(), Thumb::Loading);
                    Some((id, tries))
                })
                .collect()
        };
        for (id, tries) in wanted {
            self.screen_fetch(move || {
                let picture = youtube::thumbnail(&id).ok().and_then(|jpeg| screen::picture_of(&jpeg).ok());
                Done::Picture { id, tries, picture }
            });
        }
    }

    /// What a step of the screen left to do: the menu's effects, a picture's timer, the suggestions and thumbnails due.
    pub(super) fn screen_followups(self: &Shared, followups: Followups) {
        self.screen_effects(followups.effects);
        if followups.photo_due {
            self.photo_3d_in(0);
        }
        if let Some((id, text)) = followups.suggest {
            self.screen_fetch(move || {
                let result = youtube::suggestions(&text, &language());
                Done::Suggestions { id, text, result }
            });
        }
        if !followups.retry.is_empty() {
            self.screen_thumbnails(followups.retry);
        }
    }

    /// Carries out what the screen's menu asked for.
    pub(super) fn screen_effects(self: &Shared, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Suggest { id, text } => {
                    // Asked for once typing pauses, one request at a time: the screen's step sends it.
                    let mut rt = self.orb.lock().unwrap();
                    rt.screen.suggest.want(id, text, Instant::now());
                    rt.wake();
                }
                Effect::Search { id, query } => self.screen_fetch(move || Done::Results { id, more: false, result: youtube::search(&query, &language()) }),
                Effect::More { id, token, .. } => self.screen_fetch(move || Done::Results { id, more: true, result: youtube::more(&token, &language()) }),
                Effect::Thumbnails(ids) => self.screen_thumbnails(ids),
                Effect::Copy { link, .. } => {
                    if let Err(e) = super::orb::copy_to_clipboard(&link) {
                        self.notify(NoticeKind::Error, e);
                    }
                }
                Effect::FastWalk(on) => self.update_settings(|s| s.walk_boost.enabled = on),
                Effect::CallContact { id } => {
                    let ctl = self.clone();
                    std::thread::spawn(move || ctl.call_contact(id));
                }
                Effect::Answer => {
                    let ctl = self.clone();
                    std::thread::spawn(move || ctl.call_answer());
                }
                Effect::HangUp => {
                    let ctl = self.clone();
                    std::thread::spawn(move || ctl.call_end());
                }
                Effect::PhotoChoice { timer, length } | Effect::Photo3d { timer, length } => {
                    let take = matches!(effect, Effect::Photo3d { .. });
                    self.update_settings(|s| {
                        s.photo_3d_timer = timer;
                        s.photo_3d = photo_setting(length);
                    });
                    if take {
                        self.photo_3d_in(timer);
                    }
                }
                Effect::Media(command) => crate::media::watcher().command(command),
                Effect::Light(light) => {
                    if let Err(e) = self.orb_set_light(light) {
                        self.notify(NoticeKind::Warning, e);
                    }
                }
                Effect::SaveState { name } => self.states_save(Some(name)),
                Effect::ApplyState { id, .. } => self.states_apply(id, false),
                Effect::Drop { link, title, video } => self.card_drop(link, title, video),
                Effect::Close => {
                    let mut rt = self.orb.lock().unwrap();
                    rt.screen.hover = None;
                    rt.screen.pressed = None;
                }
            }
        }
    }

    /// The screen's part of the orb thread's tick: what came back from threads, the finger, the picture timer, then
    /// the steps to the screen. Returns the messages for VRChat; what is left to do goes in `followups`.
    pub(super) fn screen_tick(
        rt: &mut OrbRuntime,
        now: Instant,
        snapshot_calls: &crate::state::CallsStatus,
        settings_walk: bool,
        vrchat: Option<u32>,
        followups: &mut Followups,
    ) -> Vec<Message> {
        let mut out = Vec::new();
        // The screen stands where the panel would show: what N4N0 says goes on the screen's Home instead, whole, a line
        // at a time.
        if rt.screen.available && rt.screen.menu.open && !rt.screen.menu.card_shown() {
            if let Some(text) = rt.sender.as_mut().filter(|s| s.busy() && !s.sticky()).and_then(TextSender::take_text) {
                rt.screen.answer = answer_pieces(&text, rt.screen.menu.pages).into();
                rt.screen.answer_next = Some(now);
            }
        } else if !rt.screen.answer.is_empty() {
            // The menu closed before N4N0's answer was read: the rest goes on the panel.
            let rest = std::mem::take(&mut rt.screen.answer).into_iter().collect::<Vec<_>>().join(" ");
            rt.screen.answer_next = None;
            rt.show(&rest);
        }
        let listening = rt.listening.is_some();
        let thinking = rt.thinking();
        let light = rt.light;
        // The avatar's saved states, as the States page lists them (again while one saved there waits for the list).
        let states_key = (rt.states.version, rt.avatar_id.clone());
        let rows = (rt.screen.states_seen.as_ref() != Some(&states_key) || rt.screen.menu.saving()).then(|| rt.states.rows(rt.avatar_id.as_deref()));
        let s = &mut rt.screen;
        if let Some(rows) = rows {
            s.menu.set_states(rows, states_key.1.is_some(), "VRChat's avatar is not known yet");
            s.states_seen = Some(states_key);
        }
        // The light as the avatar holds it; whether a copied link drops as a card here; the music while the menu shows
        // it (read on the media thread).
        s.menu.light = light;
        s.menu.can_drop = s.card_ready();
        if s.menu.open && s.menu.pages.music_light() {
            let media = crate::media::watcher();
            media.want();
            let (version, reading) = media.reading();
            if version != s.media_seen {
                s.media_seen = version;
                s.menu.media = reading.media;
                s.menu.volume = reading.volume;
                if let Some(art) = reading.art {
                    if !matches!(s.pictures.get(&art.key), Some(Thumb::Ready(_))) {
                        s.pictures.insert(art.key.clone(), Thumb::Ready(screen::picture_from_rgb(&art.rgb)));
                    }
                }
            }
        }
        if s.photo_at.is_some_and(|at| now >= at) {
            s.photo_at = None;
            followups.photo_due = true;
        }
        while let Ok(done) = s.done_rx.try_recv() {
            match done {
                Done::Suggestions { id, text, result } => {
                    s.suggest.answered(id);
                    match result {
                        Ok(list) => s.menu.suggestions_arrived(id, &text, list),
                        Err(e) => eprintln!("orb menu: YouTube suggestions: {e:#}"),
                    }
                }
                Done::Results { id, more, result: Ok(results) } => followups.effects.extend(s.menu.results_arrived(id, results, more)),
                Done::Results { id, result: Err(e), .. } => s.menu.results_failed(id, &e.to_string()),
                Done::Picture { id, tries, picture } => {
                    let thumb = match picture {
                        Some(picture) => Thumb::Ready(picture),
                        None => Thumb::Failed { at: now, tries: tries + 1 },
                    };
                    s.pictures.insert(id, thumb);
                }
            }
        }
        // Suggestions once typing paused, one request at a time.
        followups.suggest = s.suggest.due(now);
        // Thumbnails of the results shown that failed: tried again once their wait is over.
        if s.menu.open && s.menu.page == Page::Results {
            for video in s.menu.page_results() {
                if let Some(Thumb::Failed { at, tries }) = s.pictures.get(&video.id) {
                    if *tries < THUMB_TRIES && now >= *at + THUMB_RETRY * *tries as u32 {
                        followups.retry.push(video.id.clone());
                    }
                }
            }
        }
        // What N4N0 said, on Home a line at a time (the next once the last was read; it waits while another page shows).
        if s.menu.open && s.menu.page == Page::Home && s.answer_next.is_some_and(|t| now >= t) {
            match s.answer.pop_front() {
                Some(line) => {
                    s.answer_next = Some(now + menu::reading_time(&line));
                    s.menu.heard = Some(line);
                    s.menu.heard_private = false;
                }
                None => s.answer_next = None,
            }
        }
        // What the menu shows from elsewhere.
        s.menu.listening = listening;
        s.menu.fast_walk = settings_walk;
        s.menu.countdown = s.photo_at.map(|at| at.saturating_duration_since(now).as_secs_f32().ceil() as u32);
        let contacts: Vec<Contact> = snapshot_calls.contacts.iter().map(|c| Contact { id: c.id.clone(), name: c.name.clone() }).collect();
        let (ready, note) = match &snapshot_calls.link {
            crate::state::CallLink::Linked { .. } => (true, String::new()),
            crate::state::CallLink::Off => (false, "Calls are off: Orbiters - N4N0 › N4N0 › Calls".to_string()),
            crate::state::CallLink::SignedOut => (false, "Sign in to Orbiters to call friends".to_string()),
            crate::state::CallLink::NoAccount => (false, "VRChat's account here is unknown: open VRChat".to_string()),
            crate::state::CallLink::NotLinked { .. } => (false, "Link this VRChat account on Orbiters to call".to_string()),
        };
        if s.menu.contacts != contacts || s.menu.calls_ready != ready || s.menu.call_note != note {
            s.menu.set_contacts(contacts, ready, &note);
        }
        let call = snapshot_calls.call.as_ref().map(|c| match c.phase {
            crate::state::CallPhase::Ringing => Call::Ringing { name: c.name.clone() },
            crate::state::CallPhase::Calling | crate::state::CallPhase::Connecting => Call::Calling { name: c.name.clone() },
            crate::state::CallPhase::Connected => {
                Call::Connected { name: c.name.clone(), seconds: c.connected_at_ms.map_or(0, |at| super::unix_ms().saturating_sub(at) / 1000), muted: c.muted }
            }
        });
        if s.menu.call != call {
            s.menu.set_call(call);
        }
        // A dropped card counts its seconds down, and goes at the end.
        if let Some(until) = s.card_until {
            if !s.menu.card_shown() {
                s.card_until = None;
            } else if now >= until {
                s.end_card();
            } else if let Some(card) = s.menu.card.as_mut() {
                card.left = until.saturating_duration_since(now).as_secs_f32().ceil() as u32;
            }
        }
        let card = s.menu.card_shown();
        if !s.available && !card {
            // The menu is not on this screen: only what a card that just went leaves to say (closed, not dropped).
            if s.present && (s.open_sent == Some(true) || s.drop_sent == Some(true)) {
                s.sync_open_and_drop(&mut out);
            }
            return out;
        }
        // The finger: each hand's four touch points when they changed, else how long it went unseen.
        for hand in 0..2 {
            let touched = std::mem::take(&mut s.touched[hand]);
            // A dropped card is not the menu: nothing on it is pointed at.
            if !s.menu.open || card {
                continue;
            }
            let gesture = match (s.pointing, touched) {
                (true, true) => {
                    let [a, b, c, _] = s.proximity[hand];
                    Some(s.pointers[hand].update(s.ray[hand].0, s.ray[hand].1, [a, b, c], now))
                }
                (true, false) => s.pointers[hand].tick(now),
                (false, true) => Some(s.touch[hand].update(s.proximity[hand], now)),
                (false, false) => s.touch[hand].tick(now),
            };
            let layout = Frame::layout(&s.menu);
            match gesture {
                None => {}
                Some(Gesture::Press { x, y }) => {
                    s.last_activity = now;
                    s.holding[hand] = true;
                    if let Some((index, target)) = layout::hit_pointable(layout, &s.menu, x, y) {
                        s.pressed = Some((index, now));
                        followups.effects.extend(s.menu.press(target));
                    }
                }
                Some(Gesture::Hover { x, y } | Gesture::Hold { x, y }) => {
                    s.hover = layout::hit_pointable(layout, &s.menu, x, y).map(|(i, _)| i);
                }
                Some(Gesture::Away) => {
                    s.holding[hand] = false;
                    if !s.holding.iter().any(|h| *h) {
                        s.hover = None;
                    }
                }
            }
        }
        // Desktop mode: where the owner looks, and a left click.
        s.look_and_click(vrchat, now, &mut followups.effects);
        if s.pressed.is_some_and(|(_, at)| now.duration_since(at) > PRESS_SHOWN && !s.holding.iter().any(|h| *h) && !s.clicking) {
            s.pressed = None;
        }
        if s.menu.open && listening {
            s.last_activity = now;
        }
        if s.menu.open && !card && now.duration_since(s.last_activity) > IDLE_CLOSE {
            followups.effects.extend(s.menu.close());
        }
        // Another page: what was hovered or pressed was on the last one.
        let page = s.menu.open.then(|| ScreenPage::of(&s.menu));
        if s.page_shown != page {
            s.page_shown = page;
            s.hover = None;
            s.pressed = None;
        }
        // The owner's screen: dropped or not, open or closed, then a block per step.
        s.sync_open_and_drop(&mut out);
        if !s.menu.open {
            s.owner_settled = false;
            return out;
        }
        if now >= s.next_local {
            let frame = {
                let pictures = &s.pictures;
                let flags = Flags { listening, loading: s.menu.loading(), thinking };
                Frame::new(&s.menu, s.hover, s.pressed.map(|(i, _)| i), flags, s.thumbnail, &|id: &str| thumbnail(pictures, id))
            };
            // Once it holds the whole page, blocks go again only now and then (they repair a block VRChat dropped).
            let settled = s.owner.settled(&frame.bank);
            if !settled || now >= s.idle_until {
                // A new page's clear, a changed block, else one in turn, in three steps (VRChat's frames apart).
                let send = s.owner.step(&frame.bank);
                out.extend(screen::local_messages(&send));
                s.next_local = now + s.local_step();
                if send.moves_to_block() && s.owner.resending() {
                    s.idle_until = now + LOCAL_IDLE;
                }
            }
            s.owner_settled = settled;
        }
        out
    }

    /// The screen's turn on the text channel, when it is due: everyone else's screen, a block a step (its three steps
    /// a phase apart, then the pointer stays on it a whole step).
    pub(super) fn screen_synced(rt: &mut OrbRuntime, now: Instant) -> Vec<Message> {
        let panel_wants = rt.sender.as_ref().is_some_and(|p| p.wants_step(now));
        // A panel block between its steps finishes first.
        let panel_mid_block = rt.sender.as_ref().is_some_and(TextSender::mid_block);
        let step = rt.sender.as_ref().map_or(Duration::from_millis(300), TextSender::step);
        let s = &mut rt.screen;
        let Some(everyone) = s.everyone.as_mut() else { return Vec::new() };
        // The screen's menu, or a dropped card (the only thing the screen shows while the menu is the overlay's).
        let screen_wants = s.close_everyone || (s.menu.open && (s.available || s.menu.card_shown()));
        if !screen_wants || now < s.next_synced || panel_mid_block {
            return Vec::new();
        }
        // Both want the channel: they take turns, a whole block each.
        if panel_wants && !s.screen_turn && !everyone.mid_block() {
            s.screen_turn = true;
            s.next_synced = now + step;
            return Vec::new();
        }
        s.screen_turn = false;
        let mut wait = step;
        let card = s.menu.card_shown();
        if card && s.frozen && s.freeze_next.is_some_and(|t| now >= t) {
            s.freeze_due = true;
        }
        let out = if s.close_everyone {
            s.close_everyone = false;
            s.frozen = false;
            // Closing clears everyone else's memory, and leaves the pointer where it writes nothing.
            everyone.cleared(true);
            screen::synced_close()
        } else if card && s.freeze_due && !everyone.mid_block() {
            // A dropped card: once its first block is on its way, everyone else's copy freezes where it is (and again
            // now and then, for whoever loaded the avatar since). 253 writes nothing: the next block's values may go.
            s.freeze_due = false;
            s.frozen = true;
            s.freeze_next = Some(now + FREEZE_AGAIN);
            everyone.park();
            screen::synced_drop()
        } else {
            // Everyone else's frame: no hover nor press, no friend named. A new page clears their memory first.
            let pictures = &s.pictures;
            let flags = Flags { loading: s.menu.loading(), ..Default::default() };
            let frame = Frame::public(&s.menu, flags, s.thumbnail, &|id: &str| thumbnail(pictures, id));
            let send = everyone.step(&frame.bank);
            if matches!(send, screen::Send::Clear) {
                // 254 closed everyone else's copy: a card's freeze is undone with it.
                s.frozen = false;
            }
            if card && !s.frozen && send.moves_to_block() {
                s.freeze_due = true;
            }
            if !send.holds() {
                wait = orbiters_orb::text::PHASE_GAP;
            }
            screen::synced_messages(rt.sender.as_ref().map_or(8, TextSender::sync), &send)
        };
        s.next_synced = now + wait;
        if let Some(sender) = rt.sender.as_mut() {
            sender.hold_until(now + wait);
        }
        out
    }

    /// What the screen's touch points send (on the OSC receiving thread: record only, and wake the orb thread while the
    /// menu is open). The other `N4N0/Screen/` parameters are the app's own writes coming back.
    pub(super) fn screen_parameter(&self, name: &str, value: &OscValue) -> bool {
        if !name.starts_with("N4N0/Screen/") {
            return false;
        }
        if let (true, OscValue::Float(v)) = (name.starts_with(screen::local::TOUCH_PREFIX), value) {
            let mut rt = self.orb.lock().unwrap();
            rt.screen.touch_point(name, *v);
            if rt.screen.menu.open {
                rt.wake();
            }
        } else if name.starts_with(screen::local::RAY_PREFIX) {
            let mut rt = self.orb.lock().unwrap();
            rt.screen.ray_value(name, value);
            if rt.screen.menu.open {
                rt.wake();
            }
        } else if let (true, OscValue::Float(v)) = (name.starts_with(screen::local::EYE_PREFIX) || name.starts_with(screen::local::GAZE_PREFIX), value) {
            // Read at the orb thread's next step, a few milliseconds away while looking points: no wake for each.
            self.orb.lock().unwrap().screen.gaze_value(name, *v);
        }
        true
    }

    /// VRChat's frame times as the SteamVR overlay measured them (milliseconds between frames, what
    /// [`super::Controller::frames`] gets): the owner's screen paces its steps by the slowest tenth of them, two frames
    /// a step. Until a measure comes, it steps at [`LOCAL_STEP_UNMEASURED`].
    pub(super) fn screen_frames(&self, intervals_ms: &[f32]) {
        let mut sorted: Vec<f32> = intervals_ms.iter().copied().filter(|v| v.is_finite() && *v > 0.0).collect();
        if sorted.len() < 10 {
            return;
        }
        sorted.sort_by(f32::total_cmp);
        let slow = sorted[sorted.len() * 9 / 10];
        self.orb.lock().unwrap().screen.frame_ms = Some(slow.min(200.0));
    }
}

/// Thumbnail `id`, when the screen has it.
fn thumbnail(pictures: &HashMap<String, Thumb>, id: &str) -> Option<Picture> {
    match pictures.get(id) {
        Some(Thumb::Ready(picture)) => Some(picture.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_touch_points() {
        let mut s = ScreenRuntime::default();
        assert!(s.touch_point("N4N0/Screen/Touch/L2", 0.5));
        assert!(s.touch_point("N4N0/Screen/Touch/R3", 0.25));
        assert!(!s.touch_point("N4N0/Text/Sync0", 0.25));
        assert_eq!(s.proximity[0][2], 0.5);
        assert_eq!(s.proximity[1][3], 0.25);
        assert_eq!(s.touched, [true, true]);
        assert!(s.touch_point("N4N0/Screen/Touch/X0", 1.0), "ours, ignored");
        assert_eq!(language().len(), 2);
    }

    #[test]
    fn reads_pointing_rays() {
        let mut s = ScreenRuntime::default();
        assert!(s.ray_value("N4N0/Screen/Ray/R_Hit", &OscValue::Bool(true)));
        assert!(s.ray_value("N4N0/Screen/Ray/R_Ratio", &OscValue::Float(0.2)));
        assert!(s.ray_value("N4N0/Screen/Ray/L_Distance", &OscValue::Float(0.3)), "ours, not read");
        assert!(!s.ray_value("N4N0/Screen/Touch/R0", &OscValue::Float(0.3)));
        assert_eq!(s.ray, [(false, 1.0), (true, 0.2)]);
        assert_eq!(s.touched, [false, true]);
    }

    #[test]
    fn what_waits_never_wakes_the_orb_thread_at_once() {
        let mut s = ScreenRuntime::default();
        s.avatar(true, true, Some(8), 72);
        s.menu.open();
        let now = Instant::now();
        let past = now - Duration::from_secs(1);
        // N4N0's answer waits while another page shows, a press is held, the screen's turn waits for the panel's block.
        s.menu.page = Page::Search;
        s.answer.push_back("later".into());
        s.answer_next = Some(past);
        s.pressed = Some((3, past));
        s.holding = [true, false];
        s.next_synced = past;
        (s.owner_settled, s.next_local, s.idle_until) = (true, now + Duration::from_millis(10), now + LOCAL_IDLE);
        assert_eq!(s.next_due(now), Some(now + LOCAL_IDLE), "the owner's next resend, nothing sooner");
        s.menu.page = Page::Home;
        assert_eq!(s.next_due(now), Some(past), "on Home the next line is due");
    }

    /// The stand-in mouse: as a test sets it.
    #[derive(Clone, Default)]
    struct FakeMouse(std::sync::Arc<std::sync::Mutex<MouseState>>);

    impl Mouse for FakeMouse {
        fn read(&mut self, _vrchat: u32) -> MouseState {
            *self.0.lock().unwrap()
        }
    }

    /// Home on the screen in desktop mode, the stand-in mouse up, VRChat in front in mouse-look.
    fn looking(mouse: &FakeMouse) -> ScreenRuntime {
        let mut s = ScreenRuntime::default();
        s.avatar(true, true, Some(8), 72);
        s.set_gaze(true);
        s.menu.open();
        s.mouse = Box::new(mouse.clone());
        *mouse.0.lock().unwrap() = MouseState { down: false, front: true, cursor: false };
        s
    }

    /// What VRChat sends of the eyes (at rest) and the gaze point when the owner looks at pixel (`x`, `y`) of the page.
    fn look_at(s: &mut ScreenRuntime, x: f32, y: f32) {
        let eyes = orbiters_contracts::orb::params::GAZE_EYES;
        let on =
            [x as f64 / layout::WIDTH as f64 * touch::WIDTH - touch::WIDTH / 2.0, touch::HEIGHT / 2.0 - y as f64 / layout::HEIGHT as f64 * touch::HEIGHT, 0.0];
        let ahead = [on[0] - eyes[0], on[1] - eyes[1], on[2] - eyes[2]];
        let length = (ahead[0].powi(2) + ahead[1].powi(2) + ahead[2].powi(2)).sqrt();
        let gaze = [eyes[0] + 0.38 * ahead[0] / length, eyes[1] + 0.38 * ahead[1] / length, eyes[2] + 0.38 * ahead[2] / length];
        for (prefix, at) in [(screen::local::EYE_PREFIX, eyes), (screen::local::GAZE_PREFIX, gaze)] {
            for (i, receiver) in touch::RECEIVERS.iter().enumerate() {
                let d = ((at[0] - receiver[0]).powi(2) + (at[1] - receiver[1]).powi(2) + (at[2] - receiver[2]).powi(2)).sqrt() - 0.01;
                assert!(s.gaze_value(&format!("{prefix}{i}"), (1.0 - d / touch::RADIUS).clamp(0.0, 1.0) as f32));
            }
        }
    }

    /// Home's YouTube tile, its middle on the page.
    fn youtube_tile() -> (f32, f32) {
        let tile = layout::cached(ScreenPage::Home).elements.iter().find(|e| e.target == menu::Target::YouTube).unwrap();
        (tile.rect.x as f32 + tile.rect.w as f32 / 2.0, tile.rect.y as f32 + tile.rect.h as f32 / 2.0)
    }

    /// A step `ms` after `start` with the mouse so; whether it pressed the tile (and the search page opened).
    fn step(s: &mut ScreenRuntime, mouse: &FakeMouse, start: Instant, ms: u64, state: MouseState) -> bool {
        *mouse.0.lock().unwrap() = state;
        let now = start + Duration::from_millis(ms);
        s.look_and_click(Some(1), now, &mut Vec::new());
        let pressed = s.pressed.is_some_and(|(_, at)| at == now);
        assert!(!pressed || s.menu.page == Page::Search, "the tile's press opens the search page");
        pressed
    }

    const UP: MouseState = MouseState { down: false, front: true, cursor: false };
    const DOWN: MouseState = MouseState { down: true, front: true, cursor: false };

    #[test]
    fn a_click_just_after_the_menu_opened_presses_nothing() {
        let mouse = FakeMouse::default();
        let mut s = looking(&mouse);
        let (x, y) = youtube_tile();
        look_at(&mut s, x, y);
        let start = Instant::now();
        assert!(!step(&mut s, &mouse, start, 0, UP));
        assert!(s.hover.is_some(), "the tile looked at is hovered");
        // The Action Menu's click, or one at once: within half a second of opening, nothing.
        assert!(!step(&mut s, &mouse, start, 300, DOWN));
        assert!(!step(&mut s, &mouse, start, 400, UP));
        assert!(s.pressed.is_none());
        // Later, the same click presses the tile.
        assert!(step(&mut s, &mouse, start, 700, DOWN));
        // The menu opened again: half a second again.
        s.menu.close();
        assert!(!step(&mut s, &mouse, start, 800, UP));
        s.menu.open();
        assert!(!step(&mut s, &mouse, start, 900, UP));
        assert!(!step(&mut s, &mouse, start, 1000, DOWN), "home again, too soon");
    }

    #[test]
    fn a_click_presses_only_what_the_gaze_rested_on() {
        let mouse = FakeMouse::default();
        let mut s = looking(&mouse);
        let (x, y) = youtube_tile();
        let start = Instant::now();
        // Above the screen first, then onto the tile mid-turn: a click at once presses nothing.
        look_at(&mut s, x, -100.0);
        assert!(!step(&mut s, &mouse, start, 0, UP));
        assert!(!step(&mut s, &mouse, start, 600, UP));
        look_at(&mut s, x, y);
        assert!(!step(&mut s, &mouse, start, 610, UP));
        assert!(!step(&mut s, &mouse, start, 660, DOWN), "50 ms on the tile");
        assert!(!step(&mut s, &mouse, start, 700, UP));
        // Resting on it: the click presses it.
        assert!(step(&mut s, &mouse, start, 800, DOWN), "190 ms on the tile");
    }

    #[test]
    fn no_click_counts_while_vrchat_shows_its_cursor() {
        let mouse = FakeMouse::default();
        let mut s = looking(&mouse);
        let (x, y) = youtube_tile();
        look_at(&mut s, x, y);
        let start = Instant::now();
        assert!(!step(&mut s, &mouse, start, 0, UP));
        // VRChat's Quick Menu open: its cursor shows, the click is VRChat's.
        let shown = MouseState { cursor: true, ..DOWN };
        assert!(!step(&mut s, &mouse, start, 600, shown));
        assert!(!step(&mut s, &mouse, start, 700, MouseState { cursor: true, ..UP }));
        // Back in mouse-look.
        assert!(step(&mut s, &mouse, start, 800, DOWN));
    }

    #[test]
    fn the_screen_s_own_menu_offers_drop_from_version_5_only() {
        use orbiters_orb::menu::Target;
        let mut rt = OrbRuntime::default();
        rt.screen.avatar(true, true, Some(8), 72);
        let tick = |rt: &mut OrbRuntime| {
            super::super::Controller::screen_tick(rt, Instant::now(), &crate::state::CallsStatus::default(), false, None, &mut Followups::default());
        };
        rt.screen.menu.pages = Pages::Six;
        tick(&mut rt);
        assert!(!rt.screen.menu.can_drop && !rt.screen.card_ready(), "a screen of version 4 has no card page");
        rt.screen.menu.pages = Pages::Seven;
        tick(&mut rt);
        assert!(rt.screen.menu.can_drop && rt.screen.card_ready());
        // A copied result's Drop drops it (the screen then shows the card: its host's part).
        let menu = &mut rt.screen.menu;
        menu.open();
        menu.search_for("cats");
        let video = youtube::Video { id: "dQw4w9WgXcQ".into(), title: "Never".into(), channel: "R".into(), duration: None, live: false };
        menu.results_arrived(1, youtube::Results { videos: vec![video], continuation: None }, false);
        menu.press(Target::Copy(0));
        assert!(matches!(&menu.press(Target::Drop(0))[..], [Effect::Drop { video: Some(v), .. }] if v == "dQw4w9WgXcQ"));
        // Without the screen (none, or one the text channel cannot reach), no card.
        rt.screen.avatar(false, false, None, 72);
        assert!(!rt.screen.card_ready());
    }
}
