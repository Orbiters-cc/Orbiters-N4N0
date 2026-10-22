//! Orb's menu in the tray service (contracts `Docs/orb.md`, Menu and Screen): on the avatar's projected screen when
//! the Orb prop has one, and the requests that open it in the SteamVR overlay otherwise.
//!
//! The screen's menu lives here, beside the OSC link: the orb thread writes its memory (the owner's through local
//! values, a block every step; everyone else's through the text channel, in turn with the panel), the screen's touch
//! points say where a finger is, and what the menu asks for runs on threads of its own (YouTube, calls, a picture).
//!
//! Everyone else's screen is built apart from the owner's ([`Frame::public`]): it never names the owner's friends, and
//! leaves out the owner's hover and presses, which changed with every move of the finger and held the rest back.

use super::orb::{OrbRuntime, Source};
use super::{NoticeKind, Shared};
use orbiters_core::osc::OscValue;
use orbiters_orb::menu::layout::{Content, ScreenPage};
use orbiters_orb::menu::screen::{self, Audience, Flags, Frame, Picture, LOCAL_VALUES};
use orbiters_orb::menu::touch::{self, Gesture, Touch, LOST_GRACE};
use orbiters_orb::menu::{self, layout, Call, Contact, Effect, KeyboardLayout, Menu, Page, PhotoLength, SuggestQueue};
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
/// VRChat sends back the `Orb/Menu` the desktop set within this: the same value then is its echo, not the owner's.
const MENU_ECHO: Duration = Duration::from_secs(1);
/// After an avatar change, what VRChat sends of `Orb/Menu` for this long is the loaded avatar's default.
const MENU_SETTLE: Duration = Duration::from_secs(2);
/// The overlay says its menu opened within this of being asked (else it is not there).
const OVERLAY_ANSWER: Duration = Duration::from_secs(2);
/// While looking points, the mouse is read this often (a click is down about a tenth of a second).
const GAZE_POLL: Duration = Duration::from_millis(15);

/// The left mouse button and whether VRChat's window is in front, as desktop mode's clicks read them (tests feed their
/// own).
pub(super) trait Mouse: Send {
    /// The button is down; the window in front is VRChat's (process `vrchat`).
    fn read(&mut self, vrchat: u32) -> (bool, bool);
}

/// This computer's mouse.
struct WindowsMouse;

impl Mouse for WindowsMouse {
    fn read(&mut self, vrchat: u32) -> (bool, bool) {
        #[cfg(windows)]
        {
            use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
            use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetSystemMetrics, GetWindowThreadProcessId, SM_SWAPBUTTON};
            // SAFETY: plain reads of the input state; the process id is written to a local that outlives the call.
            unsafe {
                // The left button as the owner set it up (swapped: the physical right one).
                let button = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
                let down = (GetAsyncKeyState(button.0 as i32) as u16) & 0x8000 != 0;
                let window = GetForegroundWindow();
                let mut pid = 0u32;
                let front = !window.is_invalid() && GetWindowThreadProcessId(window, Some(&mut pid as *mut u32)) != 0 && pid == vrchat;
                (down, front)
            }
        }
        #[cfg(not(windows))]
        {
            let _ = vrchat;
            (false, false)
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
    /// The avatar's Orb prop has the projected screen, with this many text values.
    pub available: bool,
    sync: Option<usize>,
    pub menu: Menu,
    owner: Audience,
    everyone: Option<Audience>,
    /// What `Orb/Screen/Open` was last set to.
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
    pub(super) hover: Option<usize>,
    pub(super) pressed: Option<(usize, Instant)>,
    holding: [bool; 2],
    /// The page hover and presses are for.
    page_shown: Option<ScreenPage>,
    /// Desktop mode with Orb's menu on the screen: its owner points by looking and presses with a left click.
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
    pictures: HashMap<String, Thumb>,
    suggest: SuggestQueue,
    /// What Orb says while the menu is open: on Home, a line at a time, the next once the last was read.
    answer: VecDeque<String>,
    answer_next: Option<Instant>,
    done_tx: mpsc::Sender<Done>,
    done_rx: mpsc::Receiver<Done>,
    last_activity: Instant,
    /// The overlay's menu: open, and whether a sentence Orb hears is what to search for.
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
    /// The Menu toggle of Orb's expressions menu (`Orb/Menu`) as the avatar holds it, as far as the desktop knows
    /// (`None`: not known, after an avatar change); the values the desktop set it to lately (VRChat sends them back,
    /// and an echo asks nothing); until when what VRChat sends of it is a loaded avatar's default, not the owner's.
    menu_param: Option<bool>,
    menu_sent: Vec<(bool, Instant)>,
    menu_settle_until: Option<Instant>,
}

impl Default for ScreenRuntime {
    fn default() -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        let now = Instant::now();
        Self {
            available: false,
            sync: None,
            menu: Menu::default(),
            owner: Audience::new(LOCAL_VALUES),
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
        }
    }
}

impl ScreenRuntime {
    /// The avatar changed: whether it has the screen, with `sync` text values (its memory is unknown again). The same
    /// avatar looked at again changes nothing.
    pub fn avatar(&mut self, available: bool, sync: Option<usize>) {
        let available = available && sync.is_some_and(screen::fits);
        if available && self.available && sync == self.sync {
            return;
        }
        self.available = available;
        self.sync = sync.filter(|_| available);
        // A loaded avatar's copies start clear; where the text channel's pointer stands is not known.
        self.everyone = self.sync.map(|n| {
            let mut everyone = Audience::new(n);
            everyone.cleared(false);
            everyone
        });
        self.owner.forget();
        self.open_sent = None;
        if !self.available {
            self.menu.close();
        }
    }

    /// Messages to VRChat were lost: neither screen's memory is known any more (each is cleared at its next step), nor
    /// whether it is open.
    pub(super) fn lost(&mut self) {
        self.owner.forget();
        if let Some(everyone) = self.everyone.as_mut() {
            everyone.forget();
        }
        self.open_sent = None;
    }

    /// Whether Orb's menu shows: on the avatar's screen when it has one, else in the overlay (asked to open or close
    /// it, it counts as open or closed until it says, or does not answer in time).
    fn menu_shown(&self, now: Instant) -> bool {
        if self.available {
            self.menu.open
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

    /// The value to set `Orb/Menu` to, when the avatar's toggle no longer says whether the menu shows (another trigger
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

    /// `Orb/Menu` as VRChat sent it: what the owner asks (`Some(open)`), or nothing: the echo of a value the desktop
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
        if self.available && self.menu.open {
            at(if self.owner_settled { self.next_local.max(self.idle_until) } else { self.next_local });
            // A press shown ends once its time is up and the finger let go (which a touch point's change says).
            if let Some((_, t)) = self.pressed.filter(|_| !self.holding.iter().any(|h| *h) && !self.clicking) {
                at(t + PRESS_SHOWN);
            }
            // Looking points: the gaze and the mouse are read every few milliseconds.
            if self.gaze {
                at(now + GAZE_POLL);
            }
            at(self.last_activity + IDLE_CLOSE);
            // What Orb said moves on only while Home shows.
            if let Some(t) = self.answer_next.filter(|_| self.menu.page == Page::Home) {
                at(t);
            }
            if let Some(t) = self.suggest.next_due() {
                at(t);
            }
            // A finger no longer heard of is let go of a moment later.
            if self.touch.iter().any(|t| t.finger.is_some()) {
                at(now + LOST_GRACE);
            }
        }
        // The screen's turn on the text channel; one already due waits for the panel's block to end, whose own steps
        // wake the orb thread.
        if self.everyone.is_some() && (self.menu.open || self.close_everyone) && self.next_synced > now {
            at(self.next_synced);
        }
        due
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

    /// Looking points (desktop mode with Orb's menu on the screen) or no longer does: what was read of it goes.
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
        self.hover = None;
    }

    /// The eyes or the gaze point near the screen (`Orb/Screen/Eye0` …, `Orb/Screen/Gaze0` …): kept only while looking
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
    /// a left click with VRChat in front presses it (nothing when nothing is looked at); letting go ends the press.
    fn look_and_click(&mut self, vrchat: Option<u32>, now: Instant, effects: &mut Vec<Effect>) {
        if !(self.gaze && self.available && self.menu.open) {
            // The mouse is not read: a click before looking began is no click.
            self.mouse_down = None;
            self.clicking = false;
            return;
        }
        let layout = Frame::layout(&self.menu);
        if std::mem::take(&mut self.gazed) {
            self.looked_at = touch::look(self.eye, self.gaze_point);
        }
        let menu = &self.menu;
        let looked = self.looked_at.and_then(|(x, y)| layout::hit(layout, x, y)).filter(|(i, _)| !layout::state(menu, &layout.elements[*i]).hidden);
        self.hover = looked.map(|(i, _)| i);
        let Some(vrchat) = vrchat else { return };
        let (down, front) = self.mouse.read(vrchat);
        if self.mouse_down == Some(false) && down && front {
            if let Some((index, target)) = looked {
                self.last_activity = now;
                self.pressed = Some((index, now));
                self.clicking = true;
                effects.extend(self.menu.press(target));
            }
        }
        if !down {
            self.clicking = false;
        }
        self.mouse_down = Some(down);
    }

    /// The owner's index finger near the screen: one of the eight touch points (`Orb/Screen/Touch/L0` …).
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
    /// Something asked for Orb's menu (the wake word alone, a hand on the orb, a controller button): it opens on the
    /// avatar's screen when there is one, else in the overlay; Orb listens either way. Opening it again closes it.
    pub(super) fn menu_open(self: &Shared, toggle: bool) {
        let settings = self.settings();
        let mut rt = self.orb.lock().unwrap();
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

    /// Closes Orb's menu: the screen's, or the overlay's (asked to when it is open, or as soon as it says it opened).
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

    /// The Menu toggle of Orb's expressions menu (`Orb/Menu`, on the OSC action thread): on opens Orb's menu as the
    /// binding does, off closes it. VRChat sending back what the desktop set asks nothing.
    pub(super) fn menu_toggled(self: &Shared, on: bool) {
        let asked = self.orb.lock().unwrap().screen.menu_toggled(on, Instant::now());
        match asked {
            Some(true) => self.menu_open(false),
            Some(false) => self.menu_close(),
            None => {}
        }
    }

    /// Orb listens to one sentence (as a hand on the orb does), unless it already listens.
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
            s.overlay_open = open;
            s.overlay_dictation = open && dictation;
            if open {
                s.overlay_asked = None;
            } else {
                s.overlay_closing = None;
            }
            if open && std::mem::take(&mut s.overlay_close_after) {
                // Closed before it opened (the Menu toggle turned off meanwhile): the overlay's menu, even when the
                // avatar's screen shows Orb's menu by now.
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

    /// A sentence Orb heard while a menu is open: the screen's search page takes it, or the overlay's (`true`: taken).
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
        // The screen stands where the panel would show: what Orb says goes on the screen's Home instead, whole, a line
        // at a time.
        if rt.screen.available && rt.screen.menu.open {
            if let Some(text) = rt.sender.as_mut().filter(|s| s.busy() && !s.sticky()).and_then(TextSender::take_text) {
                let cols = layout::cached(ScreenPage::Home).slots.iter().find(|s| s.content == Content::Heard).map_or(60, |s| s.cols as usize - 2);
                rt.screen.answer = menu::pieces(&text, cols).into();
                rt.screen.answer_next = Some(now);
            }
        } else if !rt.screen.answer.is_empty() {
            // The menu closed before Orb's answer was read: the rest goes on the panel.
            let rest = std::mem::take(&mut rt.screen.answer).into_iter().collect::<Vec<_>>().join(" ");
            rt.screen.answer_next = None;
            rt.show(&rest);
        }
        let listening = rt.listening.is_some();
        let thinking = rt.thinking();
        let s = &mut rt.screen;
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
        // What Orb said, on Home a line at a time (the next once the last was read; it waits while another page shows).
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
            crate::state::CallLink::Off => (false, "Calls are off: Orbiters desktop › Orb › Calls".to_string()),
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
        if !s.available {
            return out;
        }
        // The finger: each hand's four touch points when they changed, else how long it went unseen.
        for hand in 0..2 {
            let touched = std::mem::take(&mut s.touched[hand]);
            if !s.menu.open {
                continue;
            }
            let gesture = if touched { Some(s.touch[hand].update(s.proximity[hand], now)) } else { s.touch[hand].tick(now) };
            let layout = Frame::layout(&s.menu);
            match gesture {
                None => {}
                Some(Gesture::Press { x, y }) => {
                    s.last_activity = now;
                    s.holding[hand] = true;
                    if let Some((index, target)) = layout::hit(layout, x, y) {
                        if !layout::state(&s.menu, &layout.elements[index]).hidden {
                            s.pressed = Some((index, now));
                            followups.effects.extend(s.menu.press(target));
                        }
                    }
                }
                Some(Gesture::Hover { x, y } | Gesture::Hold { x, y }) => {
                    s.hover = layout::hit(layout, x, y).map(|(i, _)| i).filter(|&i| !layout::state(&s.menu, &layout.elements[i]).hidden);
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
        if s.menu.open && now.duration_since(s.last_activity) > IDLE_CLOSE {
            followups.effects.extend(s.menu.close());
        }
        // Another page: what was hovered or pressed was on the last one.
        let page = s.menu.open.then(|| ScreenPage::of(&s.menu));
        if s.page_shown != page {
            s.page_shown = page;
            s.hover = None;
            s.pressed = None;
        }
        // The owner's screen: open or closed, then a block per step.
        if s.open_sent != Some(s.menu.open) {
            s.open_sent = Some(s.menu.open);
            out.push(screen::local_open(s.menu.open));
            if !s.menu.open {
                s.close_everyone = true;
            }
        }
        if !s.menu.open {
            s.owner_settled = false;
            return out;
        }
        if now >= s.next_local {
            let frame = {
                let pictures = &s.pictures;
                let flags = Flags { listening, loading: s.menu.loading(), thinking };
                Frame::new(&s.menu, s.hover, s.pressed.map(|(i, _)| i), flags, &|id: &str| thumbnail(pictures, id))
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
        let screen_wants = s.close_everyone || s.menu.open;
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
        let out = if s.close_everyone {
            s.close_everyone = false;
            // Closing clears everyone else's memory, and leaves the pointer where it writes nothing.
            everyone.cleared(true);
            screen::synced_close()
        } else {
            // Everyone else's frame: no hover nor press, no friend named. A new page clears their memory first.
            let pictures = &s.pictures;
            let frame = Frame::public(&s.menu, Flags { loading: s.menu.loading(), ..Default::default() }, &|id: &str| thumbnail(pictures, id));
            let send = everyone.step(&frame.bank);
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
    /// menu is open). The other `Orb/Screen/` parameters are the app's own writes coming back.
    pub(super) fn screen_parameter(&self, name: &str, value: &OscValue) -> bool {
        if !name.starts_with("Orb/Screen/") {
            return false;
        }
        if let (true, OscValue::Float(v)) = (name.starts_with(screen::local::TOUCH_PREFIX), value) {
            let mut rt = self.orb.lock().unwrap();
            rt.screen.touch_point(name, *v);
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
        Some(Thumb::Ready(picture)) => Some(*picture),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_touch_points() {
        let mut s = ScreenRuntime::default();
        assert!(s.touch_point("Orb/Screen/Touch/L2", 0.5));
        assert!(s.touch_point("Orb/Screen/Touch/R3", 0.25));
        assert!(!s.touch_point("Orb/Text/Sync0", 0.25));
        assert_eq!(s.proximity[0][2], 0.5);
        assert_eq!(s.proximity[1][3], 0.25);
        assert_eq!(s.touched, [true, true]);
        assert!(s.touch_point("Orb/Screen/Touch/X0", 1.0), "ours, ignored");
        assert_eq!(language().len(), 2);
    }

    #[test]
    fn what_waits_never_wakes_the_orb_thread_at_once() {
        let mut s = ScreenRuntime::default();
        s.avatar(true, Some(8));
        s.menu.open();
        let now = Instant::now();
        let past = now - Duration::from_secs(1);
        // Orb's answer waits while another page shows, a press is held, the screen's turn waits for the panel's block.
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
}
