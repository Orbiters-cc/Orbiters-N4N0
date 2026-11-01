//! N4N0's menu in the app (contracts `Docs/n4n0.md`, Menu and Screen): on the avatar's projected screen when the N4N0
//! prop has one, and the requests that open it in the SteamVR overlay otherwise.
//!
//! The screen's menu lives here, beside the OSC link: the N4N0 thread writes its memory (the owner's through local
//! values, a block every step; everyone else's through the text channel, in turn with the panel), the screen's touch
//! points say where a finger is, and what the menu asks for runs on threads of its own (YouTube, calls, a picture).
//!
//! Everyone else's screen is built apart from the owner's ([`Frame::public`]): it never names the owner's friends, and
//! leaves out the owner's hover and presses, which changed with every move of the finger and held the rest back.
//!
//! A copied link can be dropped in the world as a card (contracts `Docs/n4n0.md`, Link card; screen version 7): an object
//! of the avatar's own beside the screen, which appears in front of its owner and stays where it is (anyone may move
//! it) until another replaces it, [`n4n0::card::LIFE_SECONDS`] pass, or the owner goes to another world or avatar
//! (`N4N0/Card/Out`). Its memory, the card page's words, goes to every client through the text channel (in turn with the
//! panel and the screen). Orbiters is told so that whoever grabs it there copies its link, and a hand on a card
//! (`N4N0/Card/TouchL`, `TouchR`) copies the owner's own link, or asks Orbiters for the card dropped there.

use crate::media::Watcher;
use crate::platform::{CallRequest, Locale, Mouse, NoMouse};
use crate::runtime::{N4n0, N4n0Runtime, Shared, Source};
use crate::status::{CallsStatus, CardView, NoticeKind};
use orbiters_contracts::n4n0::{self, thumbnail};
use orbiters_core::account::{Api, Server};
use orbiters_core::osc::OscValue;
use orbiters_n4n0_core::menu::layout::{Content, ScreenPage};
use orbiters_n4n0_core::menu::pictures::{self as full, Lane, Synced};
use orbiters_n4n0_core::menu::screen::{self, Audience, Flags, Frame, Picture, LOCAL_VALUES};
use orbiters_n4n0_core::menu::touch::{self, Gesture, Ray, Touch, LOST_GRACE};
use orbiters_n4n0_core::menu::{self, layout, Card, Effect, KeyboardLayout, Menu, Page, Pages, PhotoLength, SuggestQueue};
use orbiters_n4n0_core::text::{Message, TextSender};
use orbiters_n4n0_core::youtube;
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
/// VRChat sends back the `N4N0/Menu` the app set within this: the same value then is its echo, not the owner's.
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
/// Two grabs closer than this are one (both hands, a hand brushing it twice).
const GRAB_AGAIN: Duration = Duration::from_millis(1500);

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

/// The link card out in the world: what it holds, the page it is drawn from (with its picture when that fits), its
/// memory's sender, when it goes, and whether `N4N0/Card/Out` went on.
struct CardOut {
    menu: Menu,
    picture: bool,
    sender: screen::CardSender,
    until: Instant,
    out_sent: bool,
}

/// What a step of the screen leaves to do once the N4N0's lock is let go.
#[derive(Default)]
pub(crate) struct Followups {
    pub effects: Vec<Effect>,
    pub photo_due: bool,
    /// Something on the screen was pressed (a finger, a trigger, a click): the overlay's press sound plays.
    pub pressed: bool,
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

pub(crate) struct ScreenRuntime {
    /// N4N0's menu shows on the avatar's projected screen (the prop has it and it replaces the overlay, or VRChat runs
    /// in desktop mode), with this many text values and thumbnails of this many bytes (`N4N0/Screen/ThumbnailBytes`).
    pub available: bool,
    /// The prop has the projected screen, written through the text channel, wherever the menu shows: a dropped link
    /// card shows on it even while the menu is in the overlay.
    pub(crate) present: bool,
    sync: Option<usize>,
    pub(crate) thumbnail: usize,
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
    /// Each hand's fingertip near the touch points (its four receivers), new since the last step, and its touch.
    proximity: [[f32; 4]; 2],
    touched: [bool; 2],
    touch: [Touch; 2],
    /// The screen points along the fingers (version 7): each hand's aim near the touch points, its trigger (and whether
    /// it changed since the last step), its ray.
    pub(crate) pointing: bool,
    aim: [[f32; 4]; 2],
    trigger: [bool; 2],
    pulled: [bool; 2],
    rays: [Ray; 2],
    /// Everyone else's fingertips near the touch points (version 7), new since the last step, and their touch.
    others: [f32; 4],
    others_touched: bool,
    others_touch: Touch,
    pub(crate) hover: Option<usize>,
    pub(crate) pressed: Option<(usize, Instant)>,
    /// Presses so far (a drag along the timeline is none), for the press sound.
    presses: u32,
    /// The hands and everyone else's fingers holding a press.
    holding: [bool; 3],
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
    pub(crate) mouse: Box<dyn Mouse>,
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
    pub(crate) last_activity: Instant,
    /// The overlay's menu: open, and whether a sentence N4N0 hears is what to search for.
    pub overlay_open: bool,
    pub overlay_dictation: bool,
    /// The overlay's menu shows its keyboard.
    pub overlay_keyboard: bool,
    /// Bumped to ask the overlay to open its menu.
    pub menu_request: u32,
    /// The last request may close the overlay's menu when it is open.
    pub menu_toggle: bool,
    /// The last sentence for the overlay's search field, numbered.
    pub dictation: Option<(u32, String)>,
    /// When the 3D picture starts (a menu's timer).
    pub photo_at: Option<Instant>,
    /// What stopped the last 3D picture ("Open the camera before"), numbered: every menu shows it on its 3D picture page.
    pub(crate) photo_note: Option<(u32, String)>,
    /// A press held on the music's timeline, by each hand (the left, the right, everyone else's) and by looking and
    /// clicking: the segment it is on, so that moving along the timeline moves the music.
    dragging: [Option<u8>; 4],
    /// When the overlay was last asked to open its menu, until it says it did (it counts as open meanwhile), and
    /// whether it is to close it once it did (asked to close before it answered).
    overlay_asked: Option<Instant>,
    overlay_close_after: bool,
    /// When the overlay was last asked to close its menu, until it says it did (it counts as closed meanwhile: the Menu
    /// toggle the owner just turned off is not turned on again).
    overlay_closing: Option<Instant>,
    /// The Menu toggle of N4N0's expressions menu (`N4N0/Menu`) as the avatar holds it, as far as the app knows
    /// (`None`: not known, after an avatar change); the values the app set it to lately (VRChat sends them back,
    /// and an echo asks nothing); until when what VRChat sends of it is a loaded avatar's default, not the owner's.
    menu_param: Option<bool>,
    menu_sent: Vec<(bool, Instant)>,
    menu_settle_until: Option<Instant>,
    /// The media reading the menu shows (its version: [`Watcher::reading`]).
    media_seen: u64,
    /// The saved states the menu lists (the states' version, the avatar they are for).
    states_seen: Option<(u64, Option<String>)>,
    /// The avatar's N4N0 has the link card of its own (screen version 7, `N4N0/Card/Out`).
    pub(crate) card_capable: bool,
    /// The link card out in the world; `N4N0/Card/Out` is to go off; the card's turn on the text channel is next.
    card: Option<CardOut>,
    card_off: bool,
    card_turn: bool,
    /// again ([`FREEZE_AGAIN`]) when due.
    /// The hands on a card (`N4N0/Card/TouchL`, `TouchR`), and when a grab was last taken.
    grab: [bool; 2],
    grab_at: Option<Instant>,
    /// Full pictures (screen version 8, contracts `Docs/n4n0-picture.md`): the avatar's screen has them; its owner's lane
    /// (the local values, a chunk a local step) and everyone's (the text channel, with 8 or 16 values); how many turns
    /// everyone's took since the screen's last; `N4N0/Picture/Send` as last sent; when the owner's next chunk may go and
    /// when its lane goes round again.
    pub(crate) full_pictures: bool,
    picture_owner: Lane,
    picture_everyone: Option<Synced>,
    picture_turns: u8,
    picture_send: Option<bool>,
    picture_next: Instant,
    picture_repair: Option<Instant>,
    /// Testing: the screen shows the full pictures' diagnostics (its flags' bit 3), here and to everyone.
    pub(crate) pictures_diagnostics: bool,
    /// What the pictures log last said of the slots wanted, and of the chunks sent since.
    picture_logged: Vec<(usize, usize)>,
    picture_state_logged: Option<PictureState>,
    picture_log_at: Option<Instant>,
    picture_sent: [u32; 2],
}

/// What the pictures log last said of the full pictures: on, the screen's version, the menu on the screen, the screen on
/// the avatar, its text values, its thumbnails' bytes.
type PictureState = (bool, Option<i32>, bool, bool, Option<usize>, usize);

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
            aim: [[0.0; 4]; 2],
            trigger: [false; 2],
            pulled: [false; 2],
            rays: [Ray::default(); 2],
            others: [0.0; 4],
            others_touched: false,
            others_touch: Touch::default(),
            hover: None,
            pressed: None,
            presses: 0,
            holding: [false; 3],
            page_shown: None,
            gaze: false,
            eye: [0.0; 4],
            gaze_point: [0.0; 4],
            gazed: false,
            looked_at: None,
            mouse: Box::new(NoMouse),
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
            overlay_keyboard: false,
            menu_request: 0,
            menu_toggle: false,
            dictation: None,
            photo_at: None,
            photo_note: None,
            dragging: [None; 4],
            overlay_asked: None,
            overlay_close_after: false,
            overlay_closing: None,
            menu_param: None,
            menu_sent: Vec::new(),
            menu_settle_until: None,
            media_seen: 0,
            states_seen: None,
            card_capable: false,
            card: None,
            card_off: false,
            card_turn: false,
            grab: [false; 2],
            grab_at: None,
            present: false,
            full_pictures: false,
            picture_owner: Lane::local(),
            picture_everyone: None,
            picture_turns: 0,
            picture_send: None,
            picture_next: now,
            picture_repair: None,
            pictures_diagnostics: false,
            picture_logged: Vec::new(),
            picture_state_logged: None,
            picture_log_at: None,
            picture_sent: [0; 2],
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
        self.reset_pictures();
        // A loaded avatar starts without its card out: the card is said again, whole.
        if let Some(card) = self.card.as_mut() {
            card.sender.forget();
            card.out_sent = false;
        }
        if !self.available {
            self.menu.close();
        }
    }

    /// The avatar's N4N0 has the link card of its own, or not (another avatar): a card out on the last one is gone.
    pub(crate) fn set_card_capable(&mut self, capable: bool) {
        if !capable {
            self.card = None;
            self.card_off = false;
        }
        self.card_capable = capable;
    }

    /// A copied link can be dropped as a card here: the avatar's N4N0 has the card of its own and its text channel
    /// reaches the card's blocks.
    pub(crate) fn card_ready(&self) -> bool {
        self.card_capable && self.sync.is_some_and(|n| n4n0::card::fits(n, self.thumbnail))
    }

    /// The card out in the world, for the window.
    pub(crate) fn card_view(&self) -> Option<CardView> {
        let card = self.card.as_ref()?.menu.card.as_ref()?;
        let left = self.card.as_ref().map_or(0, |c| c.until.saturating_duration_since(Instant::now()).as_secs() as u32);
        Some(CardView { title: card.title.clone(), link: card.link.clone(), left })
    }

    /// The card goes (its time is up, the owner went to another world, another replaces it): `N4N0/Card/Out` goes off.
    pub(crate) fn end_card(&mut self) {
        if self.card.take().is_some() {
            self.card_off = true;
        }
    }

    /// A new card out with `card` on it: drawn from the card page as everyone sees it (its picture when the prop's text
    /// channel reaches it), its memory cleared first and written whole.
    fn put_card(&mut self, card: Card, now: Instant) {
        let Some(sync) = self.sync else { return };
        let picture = n4n0::card::picture_fits(sync, self.thumbnail);
        let mut menu = Menu::default();
        menu.pages = self.menu.pages;
        menu.show_card(card);
        let bytes = self.card_bytes(&menu, picture);
        self.card_off = false;
        self.card = Some(CardOut { menu, picture, sender: screen::CardSender::new(sync, bytes), until: now + Duration::from_secs(n4n0::card::LIFE_SECONDS as u64), out_sent: false });
    }

    /// The card's memory for `menu` (its card page) as everyone sees it.
    fn card_bytes(&self, menu: &Menu, picture: bool) -> Vec<u8> {
        let pictures = &self.pictures;
        let frame = Frame::public(menu, Flags::default(), self.thumbnail, &|id: &str| thumbnail(pictures, id));
        screen::card_bytes(&frame.bank, self.thumbnail, picture)
    }

    /// The card's step at `now`: it goes at its end; its picture arrived since (its memory then changes); `N4N0/Card/Out`
    /// on once, or off once.
    fn card_step(&mut self, now: Instant, out: &mut Vec<Message>) {
        if self.card.as_ref().is_some_and(|c| now >= c.until) {
            self.end_card();
        }
        if std::mem::take(&mut self.card_off) {
            out.push((crate::runtime::parameter(n4n0::card::OUT), rosc::OscType::Bool(false)));
        }
        let Some(card) = self.card.as_ref() else { return };
        let bytes = self.card_bytes(&card.menu, card.picture);
        let card = self.card.as_mut().unwrap();
        card.sender.update(bytes);
        if !card.out_sent {
            card.out_sent = true;
            out.push((crate::runtime::parameter(n4n0::card::OUT), rosc::OscType::Bool(true)));
        }
    }

    /// A hand on a dropped card, the owner's own or someone else's (`right`: `N4N0/Card/TouchR`), now `on`: whether it
    /// is a grab (either hand coming on, not twice in [`GRAB_AGAIN`]).
    pub(crate) fn card_touch(&mut self, right: bool, on: bool, now: Instant) -> bool {
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
    pub(crate) fn quit(&mut self) {
        let _ = self.menu.close();
        self.open_sent = Some(false);
    }

    /// Messages to VRChat were lost: neither screen's memory is known any more (each is cleared at its next step), nor
    /// whether it is open.
    pub(crate) fn lost(&mut self) {
        self.picture_owner.forget();
        if let Some(everyone) = self.picture_everyone.as_mut() {
            everyone.forget();
        }
        self.picture_send = None;
        self.owner.forget();
        if let Some(everyone) = self.everyone.as_mut() {
            everyone.forget();
        }
        self.open_sent = None;
        // The card is said again, whole.
        if let Some(card) = self.card.as_mut() {
            card.sender.forget();
            card.out_sent = false;
        }
    }

    /// Whether N4N0's menu shows: on the avatar's screen when it has one, else in the overlay (asked to open or close
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
    pub(crate) fn menu_param_reset(&mut self, now: Instant) {
        self.menu_param = None;
        self.menu_settle_until = Some(now + MENU_SETTLE);
    }

    /// The value to set `N4N0/Menu` to, when the avatar's toggle no longer says whether the menu shows (another trigger
    /// opened it, it closed on its own), taken as sent.
    pub(crate) fn menu_sync(&mut self, now: Instant) -> Option<bool> {
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

    /// `N4N0/Menu` as VRChat sent it: what the owner asks (`Some(open)`), or nothing: the echo of a value the app set, a
    /// loaded avatar's default, the menu already so.
    pub(crate) fn menu_toggled(&mut self, on: bool, now: Instant) -> Option<bool> {
        self.menu_sent.retain(|(_, at)| now.duration_since(*at) < MENU_ECHO);
        // VRChat sends back what the app set, in order: this echo is done, and so are the sends before it.
        if let Some(i) = self.menu_sent.iter().position(|&(sent, _)| sent == on) {
            self.menu_sent.drain(..=i);
            return None;
        }
        // The owner's own (or the avatar's default): whatever the app set before was sent back already.
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

    /// When the screen next has something to do (for the N4N0 thread's sleep).
    pub(crate) fn next_due(&self, now: Instant) -> Option<Instant> {
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
        // The owner's picture chunks, a local step apart; the bridge off after the last; a round again later.
        if self.picture_owner.wants() || self.picture_send == Some(true) {
            at(self.picture_next.max(now));
        }
        if let Some(t) = self.picture_repair {
            at(t);
        }
        // The card goes at its end, and its blocks go again now and then.
        if let Some(card) = self.card.as_ref() {
            at(card.until);
            if let Some(t) = card.sender.resend_due() {
                at(t);
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
            // What N4N0 said moves on only while Home shows.
            if let Some(t) = self.answer_next.filter(|_| self.menu.page == Page::Home) {
                at(t);
            }
            if let Some(t) = self.suggest.next_due() {
                at(t);
            }
            // A finger or a ray no longer heard of is let go of a moment later.
            if self.touch.iter().chain([&self.others_touch]).any(|t| t.finger.is_some()) || self.rays.iter().any(|r| r.at.is_some()) {
                at(now + LOST_GRACE);
            }
        }
        // The screen's turn on the text channel; one already due waits for the panel's block to end, whose own steps
        // wake the N4N0 thread.
        let card_wants = self.card.as_ref().is_some_and(|c| c.sender.wants(now));
        let pictures_want = self.picture_everyone.as_ref().is_some_and(Synced::wants);
        if self.everyone.is_some() && ((self.menu.open && self.available) || self.close_everyone || card_wants || pictures_want) && self.next_synced > now {
            at(self.next_synced);
        }
        due
    }

    /// Testing: thumbnail `id` arrived.
    #[cfg(test)]
    pub(crate) fn put_picture(&mut self, id: &str, picture: Picture) {
        self.pictures.insert(id.to_string(), Thumb::Ready(picture));
    }

    /// Testing: the card's time is up now.
    #[cfg(test)]
    pub(crate) fn card_ends_now(&mut self) {
        if let Some(card) = self.card.as_mut() {
            card.until = Instant::now();
        }
    }

    /// Testing: nothing happened on the menu for longer than it stays open idle.
    #[cfg(test)]
    pub(crate) fn idle_long_ago(&mut self) {
        self.last_activity = Instant::now().checked_sub(IDLE_CLOSE + Duration::from_secs(1)).unwrap_or(self.last_activity);
    }

    /// Seconds before the 3D picture starts.
    pub fn countdown(&self, now: Instant) -> Option<u32> {
        self.photo_at.map(|at| at.saturating_duration_since(now).as_secs_f32().ceil() as u32)
    }

    /// Looking points (desktop mode with N4N0's menu on the screen) or no longer does: what was read of it goes.
    pub(crate) fn set_gaze(&mut self, on: bool) {
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

    /// The eyes or the gaze point near the screen (`N4N0/Screen/Eye0` â€¦, `N4N0/Screen/Gaze0` â€¦): kept only while looking
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
        if !(self.gaze && self.available && self.menu.open) {
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
            self.presses = self.presses.wrapping_add(1);
            self.clicking = true;
            self.dragging[3] = menu::timeline_segment(target);
            effects.extend(self.menu.press(target));
        } else if let (Some(from), Some((index, target)), true) = (self.dragging[3], looked, mouse.down && self.clicking) {
            // The click held and the gaze moved along the music's timeline.
            let (on, more) = self.menu.drag(from, target);
            if !more.is_empty() {
                self.last_activity = now;
                self.pressed = Some((index, now));
            }
            self.dragging[3] = Some(on);
            effects.extend(more);
        }
        if !mouse.down {
            self.clicking = false;
            self.dragging[3] = None;
        }
        self.mouse_down = Some(mouse.down);
    }

    /// The fingers at a step: each hand's receivers (and its trigger) when they changed, else how long they went unseen,
    /// then everyone else's fingertips (version 7). Hovered where they point, pressed by the trigger or a touch.
    fn point_and_touch(&mut self, now: Instant, effects: &mut Vec<Effect>) {
        if !self.menu.open {
            self.touch = [Touch::default(); 2];
            self.rays = [Ray::default(); 2];
            self.others_touch = Touch::default();
            self.holding = [false; 3];
            self.dragging[..3].fill(None);
            self.touched = [false; 2];
            self.pulled = [false; 2];
            self.others_touched = false;
            return;
        }
        for hand in 0..3 {
            if !self.menu.open { break; }
            let touched = if hand < 2 {
                std::mem::take(&mut self.touched[hand]) | std::mem::take(&mut self.pulled[hand])
            } else {
                std::mem::take(&mut self.others_touched)
            };
            let gesture = match (hand, self.pointing, touched) {
                (2, false, _) => None,
                (2, true, true) => Some(self.others_touch.update(self.others, touch::RADIUS, now)),
                (2, true, false) => self.others_touch.tick(now),
                // The fingertip touching, and the ray it points along with its trigger.
                (_, true, true) => {
                    let touch = self.touch[hand].update(self.proximity[hand], touch::REACH, now);
                    let ray = self.rays[hand].update(touch::aimed(self.proximity[hand], self.aim[hand]), self.trigger[hand], now);
                    touch::either(Some(touch), Some(ray))
                }
                (_, true, false) => touch::either(self.touch[hand].tick(now), self.rays[hand].tick(now)),
                (_, false, true) => Some(self.touch[hand].update(self.proximity[hand], touch::RADIUS, now)),
                (_, false, false) => self.touch[hand].tick(now),
            };
            let layout = Frame::layout(&self.menu);
            match gesture {
                None => {}
                Some(Gesture::Press { x, y }) => {
                    self.last_activity = now;
                    self.holding[hand] = true;
                    self.dragging[hand] = None;
                    if let Some((index, target)) = layout::hit_pointable(layout, &self.menu, x, y) {
                        self.pressed = Some((index, now));
                        self.presses = self.presses.wrapping_add(1);
                        self.dragging[hand] = menu::timeline_segment(target);
                        effects.extend(self.menu.press(target));
                    }
                }
                Some(Gesture::Hold { x, y }) => {
                    let hit = layout::hit_pointable(layout, &self.menu, x, y);
                    self.hover = hit.map(|(i, _)| i);
                    // Held and moved along the music's timeline: the music follows, a segment at a time.
                    if let (Some(from), Some((index, target))) = (self.dragging[hand], hit) {
                        let (on, more) = self.menu.drag(from, target);
                        if !more.is_empty() {
                            self.last_activity = now;
                            self.pressed = Some((index, now));
                        }
                        self.dragging[hand] = Some(on);
                        effects.extend(more);
                    }
                }
                Some(Gesture::Hover { x, y }) => {
                    self.holding[hand] = false;
                    self.dragging[hand] = None;
                    self.hover = layout::hit_pointable(layout, &self.menu, x, y).map(|(i, _)| i);
                }
                Some(Gesture::Away) => {
                    self.dragging[hand] = None;
                    self.holding[hand] = false;
                    if !self.holding.iter().any(|h| *h) {
                        self.hover = None;
                    }
                }
            }
        }
    }

    /// A hand's trigger, pulled or let go (screen version 7: it presses where the finger points).
    pub fn trigger(&mut self, right: bool, down: bool) {
        let hand = right as usize;
        if self.trigger[hand] != down {
            self.trigger[hand] = down;
            self.pulled[hand] = true;
        }
    }

    /// The point along an index finger near the screen: one of its four receivers (`N4N0/Screen/Aim/L0` â€¦).
    pub fn aim_point(&mut self, name: &str, value: f32) -> bool {
        let Some(rest) = name.strip_prefix(screen::local::AIM_PREFIX) else { return false };
        let mut chars = rest.chars();
        let hand = match chars.next() {
            Some('L') => 0,
            Some('R') => 1,
            _ => return true,
        };
        if let Some(i) = chars.as_str().parse::<usize>().ok().filter(|i| *i < 4) {
            self.aim[hand][i] = value;
            self.touched[hand] = true;
        }
        true
    }

    /// Someone else's index fingertip near the screen: one of its four receivers (`N4N0/Screen/Others0` â€¦).
    pub fn others_point(&mut self, name: &str, value: f32) -> bool {
        let Some(rest) = name.strip_prefix(screen::local::OTHERS_PREFIX) else { return false };
        if let Some(i) = rest.parse::<usize>().ok().filter(|i| *i < 4) {
            self.others[i] = value;
            self.others_touched = true;
        }
        true
    }

    /// The owner's index finger near the screen: one of the eight touch points (`N4N0/Screen/Touch/L0` â€¦).
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

    /// The owner's screen shown or not, as the menu says now.
    /// Whether the avatar's screen shows full pictures (its version, `N4N0/Screen/Version` 8 or later).
    pub(crate) fn set_full_pictures(&mut self, on: bool, version: Option<i32>) {
        let state = (on, version, self.available, self.present, self.sync, self.thumbnail);
        if self.picture_state_logged != Some(state) {
            self.picture_state_logged = Some(state);
            picture_log(&format!(
                "full pictures {} (screen version {version:?}, the menu on it {}, on the avatar {}, {:?} text values, {} B thumbnails)",
                if on { "on" } else { "off" },
                self.available,
                self.present,
                self.sync,
                self.thumbnail
            ));
        }
        if on != self.full_pictures {
            self.full_pictures = on;
            self.reset_pictures();
        }
    }

    /// Both picture lanes start over (another avatar, another screen): everyone's when its text channel reaches the
    /// picture pointers.
    fn reset_pictures(&mut self) {
        self.picture_owner = Lane::local();
        self.picture_everyone = self.sync.filter(|&n| self.full_pictures && orbiters_contracts::n4n0::picture::chunk::reaches_everyone(n, self.thumbnail)).map(Synced::new);
        self.picture_send = None;
        self.picture_repair = None;
    }

    /// What the picture slots should hold now (contracts `Docs/n4n0-picture.md`, Slots): the results' thumbnails (slots
    /// 0 to 4) or the music's art (slot 5) while the menu shows on the screen, and the card's picture (slot 6) while it
    /// is out with its picture; each under its thumbnail as the prop keeps it.
    fn wanted_pictures(&self) -> Vec<(usize, std::sync::Arc<Vec<u8>>)> {
        use orbiters_contracts::n4n0::picture::{SLOT_CARD, SLOT_MUSIC};
        let mut out = Vec::new();
        if !self.full_pictures {
            return out;
        }
        if self.available && self.menu.open {
            let page = ScreenPage::of(&self.menu);
            for p in &Frame::layout(&self.menu).pictures {
                let slot = match page {
                    // The captures' pictures take the results' slots (screens from version 12).
                    ScreenPage::Results | ScreenPage::History | ScreenPage::PhotoShare | ScreenPage::Explicit => p.result as usize,
                    ScreenPage::Music => SLOT_MUSIC,
                    _ => continue,
                };
                if let Some(stream) = layout::picture_key_for(&self.menu, p, true)
                    .and_then(|key| thumbnail(&self.pictures, &key))
                    .and_then(|t| t.full_stream(slot, self.thumbnail))
                {
                    out.push((slot, stream));
                }
            }
        }
        if let Some(card) = self.card.as_ref().filter(|c| c.picture) {
            if let Some(stream) = card.menu.card.as_ref().and_then(|c| c.video.as_deref()).and_then(|id| thumbnail(&self.pictures, id)).and_then(|t| t.full_stream(SLOT_CARD, self.thumbnail)) {
                out.push((SLOT_CARD, stream));
            }
        }
        out
    }

    /// The owner's picture lane at `now`: what the slots should hold, then a chunk a local step while one is due, the bridge
    /// on meanwhile (`N4N0/Picture/Send`), off once every chunk went its rounds; a round again every so often.
    fn picture_owner_step(&mut self, now: Instant, out: &mut Vec<Message>) {
        if !self.full_pictures {
            return;
        }
        let wanted = self.wanted_pictures();
        let summary: Vec<(usize, usize)> = wanted.iter().map(|(slot, stream)| (*slot, stream.len())).collect();
        if summary != self.picture_logged {
            picture_log(&format!(
                "slots wanted (slot, stream bytes): {summary:?}; everyone's lane {}",
                if self.picture_everyone.is_some() { "on" } else { "off (the text channel cannot reach the picture pointers)" }
            ));
            self.picture_logged = summary;
        }
        if self.picture_log_at.is_none_or(|t| now >= t) {
            if self.picture_sent != [0; 2] {
                picture_log(&format!("chunks sent in the last 10 s: {} to the owner, {} to everyone", self.picture_sent[0], self.picture_sent[1]));
                self.picture_sent = [0; 2];
            }
            self.picture_log_at = Some(now + std::time::Duration::from_secs(10));
        }
        self.picture_owner.want(&wanted);
        if let Some(everyone) = self.picture_everyone.as_mut() {
            everyone.lane.want(&wanted);
        }
        if wanted.is_empty() {
            self.picture_repair = None;
        } else if self.picture_repair.is_some_and(|t| now >= t) {
            self.picture_owner.again();
            self.picture_repair = None;
        }
        let wants = self.picture_owner.wants();
        if wants && now >= self.picture_next {
            if self.picture_send != Some(true) {
                self.picture_send = Some(true);
                picture_log("N4N0/Picture/Send on");
                out.push(full::local_send(true));
            }
            if let Some(chunk) = self.picture_owner.next_chunk() {
                self.picture_sent[0] += 1;
                out.extend(full::local_messages(&chunk));
            }
            self.picture_next = now + self.local_step();
        } else if !wants && now >= self.picture_next {
            if self.picture_send != Some(false) {
                self.picture_send = Some(false);
                picture_log("N4N0/Picture/Send off (every chunk went its rounds)");
                out.push(full::local_send(false));
            }
            if !wanted.is_empty() && self.picture_repair.is_none() {
                self.picture_repair = Some(now + full::LOCAL_REPAIR);
            }
        }
    }

    fn sync_open(&mut self, out: &mut Vec<Message>) {
        if self.open_sent != Some(self.menu.open) {
            self.open_sent = Some(self.menu.open);
            out.push(screen::local_open(self.menu.open));
            if !self.menu.open {
                self.close_everyone = true;
            }
        }
    }
}

/// A line in `%LOCALAPPDATA%/Orbiters/logs/n4n0-pictures.log` (what the full pictures' lanes do: whether the avatar's
/// screen has them, the slots wanted, N4N0/Picture/Send, the chunks sent), kept under 1 MB (the last one renamed .old).
/// Tests write none.
fn picture_log(line: &str) {
    #[cfg(not(test))]
    {
        use std::io::Write;
        let dir = orbiters_core::paths::logs_dir();
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join("n4n0-pictures.log");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
            let _ = std::fs::rename(&path, dir.join("n4n0-pictures.old.log"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(f, "{} {line}", orbiters_core::now_utc_iso());
        }
    }
    #[cfg(test)]
    let _ = line;
}

/// Closes N4N0's menu under N4N0's lock: the screen's (what it leaves to do), or the overlay's.
fn close_menu(rt: &mut N4n0Runtime) -> Vec<Effect> {
    let s = &mut rt.screen;
    if s.available {
        s.menu.close()
    } else {
        s.overlay_close();
        Vec::new()
    }
}

/// What N4N0 says, cut as the Home of a display with `pages` shows it, a piece at a time: a line on the four- and
/// six-tile Home, what its place holds on the seven-tile one.
fn answer_pieces(text: &str, pages: Pages) -> Vec<String> {
    // The simple Home's one line under its status.
    if pages.simple() {
        return menu::pieces(text, layout::SIMPLE_HEARD_COLUMNS - 2);
    }
    if pages.ten() {
        return layout::wide_heard_pages(text);
    }
    if pages.states_cards() {
        return layout::heard_pages(text);
    }
    let home = layout::cached(ScreenPage::home(pages));
    let cols = home.slots.iter().find(|s| s.content == Content::Heard).map_or(60, |s| s.cols as usize - 2);
    menu::pieces(text, cols)
}

/// The keyboard's letters for the setting (Auto: the device's keyboard, as `locale` says).
pub fn keyboard(setting: orbiters_core::settings::KeyboardLayout, locale: &dyn Locale) -> KeyboardLayout {
    use orbiters_core::settings::KeyboardLayout as Setting;
    match setting {
        Setting::Qwerty => KeyboardLayout::Qwerty,
        Setting::Azerty => KeyboardLayout::Azerty,
        Setting::Auto => locale.keyboard(),
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

/// What opening N4N0's menu leaves to do once N4N0's lock is let go.
enum Opened {
    /// It was open, and this closed it: the menu's effects.
    Closed(Vec<Effect>),
    /// It was open already.
    Nothing,
    /// The overlay is asked to close its menu.
    Told,
    /// It opens: N4N0 listens.
    Listen,
}

impl N4n0 {
    /// Something asked for N4N0's menu (the wake word alone, a hand on N4N0, a controller button): it opens on the
    /// avatar's screen when there is one, else in the overlay; N4N0 listens either way. Opening it again closes it.
    pub(crate) fn menu_open(self: &Shared, toggle: bool) {
        let settings = self.settings();
        let opened = self.open_menu(&mut self.n4n0.lock().unwrap(), toggle, &settings);
        self.menu_opened(opened);
    }

    /// [`N4n0::menu_open`] under N4N0's lock.
    fn open_menu(&self, rt: &mut N4n0Runtime, toggle: bool, settings: &orbiters_core::settings::Settings) -> Opened {
        if rt.screen.available {
            if rt.screen.menu.open {
                return if toggle { Opened::Closed(rt.screen.menu.close()) } else { Opened::Nothing };
            }
            let keyboard = keyboard(settings.n4n0.menu.keyboard, &*self.device.locale);
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
                return Opened::Told;
            }
            s.overlay_asked = Some(Instant::now());
            s.overlay_close_after = false;
            s.overlay_closing = None;
        }
        Opened::Listen
    }

    /// What opening the menu left to do, N4N0's lock let go.
    fn menu_opened(self: &Shared, opened: Opened) {
        match opened {
            Opened::Closed(effects) => self.screen_effects(effects),
            Opened::Nothing => {}
            Opened::Told => self.publish(),
            Opened::Listen => {
                self.menu_listen();
                self.publish();
            }
        }
    }

    /// The Menu toggle of N4N0's expressions menu (`N4N0/Menu`, on the OSC action queue): on opens N4N0's menu as the
    /// binding does, off closes it. VRChat sending back what the app set asks nothing. The toggle is read and the menu
    /// opened or closed in one turn of N4N0's lock: a step of the N4N0 thread in between would find the toggle and the
    /// menu apart, and send the toggle back as it was.
    pub(crate) fn menu_toggled(self: &Shared, on: bool) {
        let settings = self.settings();
        let mut rt = self.n4n0.lock().unwrap();
        match rt.screen.menu_toggled(on, Instant::now()) {
            Some(true) => {
                let opened = self.open_menu(&mut rt, false, &settings);
                drop(rt);
                self.menu_opened(opened);
            }
            Some(false) => {
                let effects = close_menu(&mut rt);
                drop(rt);
                self.screen_effects(effects);
                self.publish();
            }
            None => {}
        }
    }

    /// A key of the computer's keyboard while N4N0's menu shows its keyboard: typed there on the avatar's screen, or handed
    /// to the overlay's menu (Ctrl+V's clipboard read here).
    pub fn key(self: &Shared, input: orbiters_n4n0_core::menu::KeyInput) {
        use orbiters_n4n0_core::menu::KeyInput;
        let input = match input {
            KeyInput::Paste => match self.device.clipboard.paste() {
                Some(text) => KeyInput::Text(text.replace(['\r', '\n'], " ")),
                None => return,
            },
            other => other,
        };
        let effects = {
            let mut rt = self.n4n0.lock().unwrap();
            if rt.screen.available && rt.screen.menu.shows_keyboard() {
                rt.screen.last_activity = Instant::now();
                let effects = rt.screen.menu.key(input, || None);
                rt.wake();
                effects
            } else if rt.screen.overlay_open && rt.screen.overlay_keyboard {
                let n = rt.keys.back().map_or(1, |(k, _)| k.wrapping_add(1));
                rt.keys.push_back((n, input));
                while rt.keys.len() > 32 {
                    rt.keys.pop_front();
                }
                Vec::new()
            } else {
                return;
            }
        };
        self.screen_effects(effects);
        self.publish();
    }

    /// N4N0 listens to one sentence (as a hand on N4N0 does), unless it already listens.
    pub(crate) fn menu_listen(self: &Shared) {
        let settings = self.with_settings(|s| s.n4n0.clone());
        if !settings.enabled || !settings.assistant {
            return;
        }
        {
            let rt = self.n4n0.lock().unwrap();
            if rt.listening.is_some() || !rt.voice.installed {
                return;
            }
        }
        self.n4n0.lock().unwrap().listening = Some((Source::Touch, Instant::now()));
        self.voice_listen(Source::Touch);
    }

    /// The overlay's menu opened, closed, or moved to or from its search page.
    pub(crate) fn menu_overlay_state(self: &Shared, open: bool, dictation: bool) {
        let listen = {
            let mut rt = self.n4n0.lock().unwrap();
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
        self.publish();
    }

    /// A sentence N4N0 heard while a menu is open: the screen's search page takes it, or the overlay's (`true`: taken).
    pub(crate) fn menu_heard(self: &Shared, text: &str) -> bool {
        let mut rt = self.n4n0.lock().unwrap();
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
            self.publish();
            return true;
        }
        false
    }

    /// Sends a feature suggestion to Orbiters as the signed-in account, on a thread of its own: from the avatar's
    /// screen (`screen`: its menu hears how it went), or from the overlay's or the window's (numbered in the status).
    pub(crate) fn suggestion_send(self: &Shared, text: String, voice: bool, screen: bool) {
        let n = self.clone();
        let _ = std::thread::Builder::new().name("n4n0-suggestion".into()).spawn(move || {
            let about = orbiters_n4n0_core::suggestions::About {
                source: "menu",
                app_version: n.host.app_version(),
                n4n0_version: n.n4n0.lock().unwrap().n4n0_version.clone(),
                voice,
            };
            let result = match n.host.account() {
                Some((api, token)) => orbiters_n4n0_core::suggestions::send(&api, &token, &text, &about).map_err(|e| format!("{e:#}")),
                None => Err("Sign in to Orbiters in Orbiters - N4N0 first (Settings)".to_string()),
            };
            {
                let mut rt = n.n4n0.lock().unwrap();
                if screen {
                    rt.screen.menu.suggestion_sent(result.clone());
                    rt.screen.last_activity = Instant::now();
                } else {
                    let number = rt.suggestion.as_ref().map_or(1, |(k, _)| k.wrapping_add(1));
                    rt.suggestion = Some((number, result.clone().err()));
                }
            }
            match result {
                Ok(()) => n.notify(NoticeKind::Info, "Your suggestion reached Orbiters: thank you!".to_string()),
                Err(e) => n.notify(NoticeKind::Warning, format!("The suggestion was not sent: {e}")),
            }
            n.publish();
        });
    }

    /// Orbiters' VRChat account invites the player to `location` (Previous instances' Invite me), from the screen's menu
    /// (`screen`) or another's (the answer then goes in the status' `invite`); a toast says how it went.
    pub(crate) fn invite_send(self: &Shared, location: String, screen: bool) {
        use orbiters_n4n0_core::menu::InviteAnswer;
        let n = self.clone();
        let _ = std::thread::Builder::new().name("n4n0-invite".into()).spawn(move || {
            let answer = match n.host.account() {
                Some((api, token)) => orbiters_n4n0_core::invites::send(&api, &token, &location),
                None => InviteAnswer::Failed { message: "Sign in to Orbiters in Orbiters - N4N0 first (Settings)".into() },
            };
            {
                let mut rt = n.n4n0.lock().unwrap();
                if screen {
                    rt.screen.menu.invite_answered(answer.clone());
                    rt.screen.last_activity = Instant::now();
                } else {
                    let number = rt.invite.as_ref().map_or(1, |(k, _)| k.wrapping_add(1));
                    rt.invite = Some((number, answer.clone()));
                }
            }
            match &answer {
                InviteAnswer::Sent => n.notify(NoticeKind::Success, "Orbiters invited you: open the invite in VRChat's notifications.".to_string()),
                InviteAnswer::NeedsFriend { account, .. } => {
                    n.notify(NoticeKind::Info, format!("Add {} as a friend on VRChat, then try again.", account.name))
                }
                InviteAnswer::Failed { message } => n.notify(NoticeKind::Warning, format!("No invite: {message}")),
            }
            n.publish();
        });
    }

    /// A 3D picture now. What stops it (VRChat's camera closed: [`crate::platform::CAMERA_CLOSED`]) is told, and shown on
    /// every menu's 3D picture page.
    pub(crate) fn photo_3d_now(self: &Shared) {
        if let Err(e) = self.host.photo_3d() {
            self.photo_stopped(e.to_string());
        }
    }

    /// What stopped a 3D picture: a toast, and the 3D picture page's line on the screen's menu and, through the status
    /// (numbered), on the overlay's and the window's.
    fn photo_stopped(self: &Shared, text: String) {
        {
            let mut rt = self.n4n0.lock().unwrap();
            if rt.screen.menu.open && rt.screen.menu.page == orbiters_n4n0_core::menu::Page::Photo {
                rt.screen.menu.notice = Some(text.clone());
            }
            let number = rt.screen.photo_note.as_ref().map_or(1, |(n, _)| n.wrapping_add(1));
            rt.screen.photo_note = Some((number, text.clone()));
        }
        self.notify(NoticeKind::Warning, text);
        self.publish();
    }

    /// A 3D picture in `seconds` (now when 0); the menus count down. VRChat's camera closed (OSC cannot open it) is said
    /// at once, not once the timer ran out.
    pub(crate) fn photo_3d_in(self: &Shared, seconds: u32) {
        let n = self.clone();
        std::thread::spawn(move || {
            if seconds == 0 {
                return n.photo_3d_now();
            }
            if n.host.camera_open() == Some(false) {
                return n.photo_stopped(crate::platform::CAMERA_CLOSED.to_string());
            }
            n.n4n0.lock().unwrap().screen.photo_at = Some(Instant::now() + Duration::from_secs(seconds as u64));
            n.publish();
        });
    }

    /// Takes the link card out in the world back (from N4N0's menu, the overlay or the window), as when its time is up:
    /// `N4N0/Card/Out` goes off at the next step.
    pub(crate) fn card_take_back(self: &Shared) {
        let taken = {
            let mut rt = self.n4n0.lock().unwrap();
            let taken = rt.screen.card.is_some();
            rt.screen.end_card();
            rt.screen.menu.card_out = false;
            rt.wake();
            taken
        };
        if taken {
            self.device.sounds.press();
            self.notify(NoticeKind::Info, "Card taken back: it is gone from the world.".to_string());
        }
        self.publish();
    }

    /// Drops a link card in the world (contracts `Docs/n4n0.md`, Link card): the avatar's card of its own shows it where it
    /// appears and stays (contracts `n4n0::card`); Orbiters is told, so that whoever grabs it in this instance copies its link.
    /// From N4N0's menu on the screen, the overlay's or the window's Home.
    pub(crate) fn card_drop(self: &Shared, link: String, title: String, video: Option<String>) {
        if !orbiters_n4n0_core::drops::copyable(&link) {
            self.notify(NoticeKind::Warning, "Only web links can be dropped as cards.");
            return;
        }
        let fetch = {
            let mut rt = self.n4n0.lock().unwrap();
            if !rt.screen.card_ready() {
                drop(rt);
                self.notify(NoticeKind::Info, "This avatar's N4N0 cannot drop cards: update N4N0 in My Avatar, then upload the avatar again.");
                return;
            }
            let now = Instant::now();
            let s = &mut rt.screen;
            s.put_card(Card { link: link.clone(), title: title.clone(), video: video.clone(), left: n4n0::card::LIFE_SECONDS }, now);
            s.last_activity = now;
            let fetch = video.filter(|id| !matches!(s.pictures.get(id), Some(Thumb::Ready(_))));
            rt.wake();
            fetch
        };
        if let Some(id) = fetch {
            self.screen_thumbnails(vec![id]);
        }
        // Orbiters keeps it for whoever grabs it in this instance (a member's card only).
        let instance = self.host.world().instance_hash();
        match (self.host.account(), instance) {
            (Some((api, token)), Some(instance)) => {
                let n = self.clone();
                std::thread::spawn(move || {
                    if let Err(e) = orbiters_n4n0_core::drops::post(&api, &token, &instance, &link, &title) {
                        n.notify(NoticeKind::Warning, format!("People around cannot copy your card's link: {e:#}"));
                    }
                });
            }
            (None, Some(_)) => self.notify(NoticeKind::Info, "Sign in to Orbiters so that people around can copy your card's link too."),
            _ => {}
        }
        self.publish();
    }

    /// A hand on a dropped card (on the OSC action queue): the owner's own card's link is copied; with none out, someone
    /// else's card was grabbed, and Orbiters gives the newest dropped in this instance.
    pub(crate) fn card_grab(self: &Shared) {
        let own = self.n4n0.lock().unwrap().screen.card_view().map(|c| (c.link, c.title));
        if let Some((link, title)) = own {
            self.card_copied(&link, &title);
            return;
        }
        let Some(instance) = self.host.world().instance_hash() else { return };
        let api = self.host.account().map(|(api, _)| api).unwrap_or_else(|| Api::new(Server::current()));
        let n = self.clone();
        std::thread::spawn(move || match orbiters_n4n0_core::drops::latest(&api, &instance) {
            Ok(Some(card)) => n.card_copied(&card.link, &card.title),
            Ok(None) => n.notify(NoticeKind::Info, "That card is gone: its link can no longer be copied."),
            Err(e) => n.notify(NoticeKind::Warning, format!("Could not copy the card's link: {e:#}")),
        });
    }

    /// A card's link in the clipboard, with the press sound and a toast, as Copy does.
    fn card_copied(&self, link: &str, title: &str) {
        match self.device.clipboard.copy(link) {
            Ok(()) => {
                self.device.sounds.press();
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
        let n = self.clone();
        let tx = self.n4n0.lock().unwrap().screen.done_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(fetch());
            n.wake();
        });
    }

    /// Fetches the thumbnails among `ids` the screen has not got (nor is getting), the page shown first; one that
    /// failed is tried again a little later, a few times.
    fn screen_thumbnails(self: &Shared, ids: Vec<String>) {
        let wanted: Vec<(String, u8)> = {
            let mut rt = self.n4n0.lock().unwrap();
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
            let n = self.clone();
            self.screen_fetch(move || {
                // A 3D capture's picture is the app's (its key after the prefix); the rest are YouTube's.
                let picture = match id.strip_prefix(orbiters_n4n0_core::captures::PICTURE_PREFIX) {
                    Some(key) => n.host.capture_picture(key).map(|rgb| screen::picture_from_rgb(&rgb)),
                    None => youtube::thumbnail(&id).ok().and_then(|jpeg| screen::picture_of(&jpeg).ok()),
                };
                Done::Picture { id, tries, picture }
            });
        }
    }

    /// What a step of the screen left to do: the menu's effects, a picture's timer, the suggestions and thumbnails due.
    pub(crate) fn screen_followups(self: &Shared, followups: Followups) {
        if followups.pressed {
            self.device.sounds.press();
        }
        self.screen_effects(followups.effects);
        if followups.photo_due {
            self.photo_3d_in(0);
        }
        if let Some((id, text)) = followups.suggest {
            let language = self.device.locale.language();
            self.screen_fetch(move || {
                let result = youtube::suggestions(&text, &language);
                Done::Suggestions { id, text, result }
            });
        }
        if !followups.retry.is_empty() {
            self.screen_thumbnails(followups.retry);
        }
    }

    /// Carries out what the screen's menu asked for.
    pub(crate) fn screen_effects(self: &Shared, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Suggest { id, text } => {
                    // Asked for once typing pauses, one request at a time: the screen's step sends it.
                    let mut rt = self.n4n0.lock().unwrap();
                    rt.screen.suggest.want(id, text, Instant::now());
                    rt.wake();
                }
                Effect::Search { id, query } => {
                    let language = self.device.locale.language();
                    self.screen_fetch(move || Done::Results { id, more: false, result: youtube::search(&query, &language) })
                }
                Effect::More { id, token, .. } => {
                    let language = self.device.locale.language();
                    self.screen_fetch(move || Done::Results { id, more: true, result: youtube::more(&token, &language) })
                }
                Effect::Thumbnails(ids) => self.screen_thumbnails(ids),
                Effect::Copy { link, .. } => {
                    if let Err(e) = self.device.clipboard.copy(&link) {
                        self.notify(NoticeKind::Error, e);
                    }
                }
                Effect::FastWalk(on) => self.update_settings(|s| s.walk_boost.enabled = on),
                Effect::CallContact { id } => {
                    let n = self.clone();
                    std::thread::spawn(move || n.host.call(CallRequest::Contact(id)));
                }
                Effect::Answer => {
                    let n = self.clone();
                    std::thread::spawn(move || n.host.call(CallRequest::Answer));
                }
                Effect::HangUp => {
                    let n = self.clone();
                    std::thread::spawn(move || n.host.call(CallRequest::End));
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
                Effect::Media(command) => self.device.media.command(command),
                Effect::Light(light) => {
                    if let Err(e) = self.set_light(light) {
                        self.notify(NoticeKind::Warning, e);
                    }
                }
                Effect::SaveState { name, size, pose, default } => self.states_save(Some(name), crate::states::SaveOptions { size, pose, default }),
                Effect::ApplyState { id, .. } => self.states_apply(id, false),
                Effect::Drop { link, title, video } => self.card_drop(link, title, video),
                Effect::TakeBack => self.card_take_back(),
                Effect::Tracking { on, levels } => self.update_settings(|s| {
                    s.tracking.enabled = on;
                    s.tracking.levels = levels;
                }),
                Effect::Dictate => self.menu_listen(),
                Effect::SendSuggestion { text, voice } => self.suggestion_send(text, voice, true),
                Effect::WatchVideo { link } => {
                    if let Err(e) = self.host.watch(&link) {
                        self.notify(NoticeKind::Warning, e);
                    }
                }
                Effect::SaveVideo => self.command(crate::Command::SaveVideo),
                Effect::Instances => {
                    let n = self.clone();
                    std::thread::spawn(move || {
                        let rows = orbiters_n4n0_core::invites::read_rows();
                        n.n4n0.lock().unwrap().screen.menu.set_instances(rows, true, "");
                        n.publish();
                    });
                }
                Effect::InviteMe { location } => self.invite_send(location, true),
                Effect::QrScan(on) => {
                    let n = self.clone();
                    std::thread::spawn(move || n.qr_scan(on));
                }
                Effect::Captures => self.captures_list(),
                Effect::Friends(on) => self.friends(on),
                Effect::ShareCapture(ask) => self.share_capture(ask),
                Effect::Feedback(ask) => self.feedback_ask(ask),
                Effect::Mirror(ask) => {
                    let mut rt = self.n4n0.lock().unwrap();
                    rt.mirror.ask(ask, Instant::now());
                    rt.wake();
                }
                Effect::CopyText { text } => {
                    if let Err(e) = self.device.clipboard.copy(&text) {
                        self.notify(NoticeKind::Error, e);
                    }
                }
                Effect::OpenLink { link } => {
                    if let Err(e) = self.host.watch(&link) {
                        self.notify(NoticeKind::Warning, e);
                    }
                }
                Effect::Close => {
                    let mut rt = self.n4n0.lock().unwrap();
                    rt.screen.hover = None;
                    rt.screen.pressed = None;
                }
            }
        }
    }

    /// What the screen's touch points send (on the OSC receiving thread: record only, and wake the N4N0 thread while the
    /// menu is open). The other `N4N0/Screen/` parameters are the app's own writes coming back.
    pub(crate) fn screen_parameter(&self, name: &str, value: &OscValue) -> bool {
        if !name.starts_with("N4N0/Screen/") {
            return false;
        }
        let finger = [screen::local::TOUCH_PREFIX, screen::local::AIM_PREFIX, screen::local::OTHERS_PREFIX].iter().any(|p| name.starts_with(p));
        if let (true, OscValue::Float(v)) = (finger, value) {
            let mut rt = self.n4n0.lock().unwrap();
            let s = &mut rt.screen;
            let _ = s.touch_point(name, *v) || s.aim_point(name, *v) || s.others_point(name, *v);
            if rt.screen.menu.open {
                rt.wake();
            }
        } else if let (true, OscValue::Float(v)) = (name.starts_with(screen::local::EYE_PREFIX) || name.starts_with(screen::local::GAZE_PREFIX), value) {
            // Read at the N4N0 thread's next step, a few milliseconds away while looking points: no wake for each.
            self.n4n0.lock().unwrap().screen.gaze_value(name, *v);
        }
        true
    }

    /// VRChat's frame times as the SteamVR overlay measured them (milliseconds between frames): the owner's screen paces
    /// its steps by the slowest tenth of them, two frames a step. Until a measure comes, it steps at
    /// [`LOCAL_STEP_UNMEASURED`].
    pub fn frames(&self, intervals_ms: &[f32]) {
        let mut sorted: Vec<f32> = intervals_ms.iter().copied().filter(|v| v.is_finite() && *v > 0.0).collect();
        if sorted.len() < 10 {
            return;
        }
        sorted.sort_by(f32::total_cmp);
        let slow = sorted[sorted.len() * 9 / 10];
        self.n4n0.lock().unwrap().screen.frame_ms = Some(slow.min(200.0));
    }
}

/// The screen's part of the N4N0 thread's tick: what came back from threads, the finger, the picture timer, then the
/// steps to the screen (`calls`: the app's, for the screen's menu; `walk`: fast walk is on; `vrchat`: VRChat's
/// process, for desktop mode's clicks; `media`: the music the menu shows). Returns the messages for VRChat; what is
/// left to do goes in `followups`.
pub(crate) fn tick(
    rt: &mut N4n0Runtime,
    now: Instant,
    calls: &CallsStatus,
    walk: bool,
    vrchat: Option<u32>,
    media: &Watcher,
    followups: &mut Followups,
) -> Vec<Message> {
    let mut out = Vec::new();
    // The screen stands where the panel would show: what N4N0 says goes on the screen's Home instead, whole, a line at
    // a time.
    if rt.screen.available && rt.screen.menu.open {
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
    let tracking = rt.tracking.as_ref().map(|t| orbiters_n4n0_core::menu::Tracking { on: t.on, levels: t.levels, note: t.note.clone() });
    let (tracking_note, versions) = (rt.tracking_note.clone(), rt.versions.clone());
    // The avatar's saved states, as the States page lists them (again while one saved there waits for the list).
    let states_key = (rt.states.version, rt.avatar_id.clone());
    let rows = (rt.screen.states_seen.as_ref() != Some(&states_key) || rt.screen.menu.saving()).then(|| rt.states.rows(rt.avatar_id.as_deref()));
    let s = &mut rt.screen;
    if let Some(rows) = rows {
        s.menu.set_states(rows, states_key.1.is_some(), "VRChat's avatar is not known yet");
        s.states_seen = Some(states_key);
    }
    // The light as the avatar holds it; whether a copied link drops as a card here; the music while the menu shows it
    // (read on the media thread).
    s.menu.light = light;
    s.menu.can_drop = s.card_ready();
    s.menu.card_out = s.card.is_some();
    s.menu.tracking = tracking;
    s.menu.tracking_note = tracking_note;
    s.menu.versions = versions;
    if s.menu.open && s.menu.pages.music_light() {
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
                    Err(e) => eprintln!("N4N0 menu: YouTube suggestions: {e:#}"),
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
    // Thumbnails of the results (or the 3D captures) shown that failed: tried again once their wait is over.
    if s.menu.open && matches!(s.menu.page, Page::Results | Page::History) {
        let ids: Vec<String> = match s.menu.page {
            Page::Results => s.menu.page_results().iter().map(|v| v.id.clone()).collect(),
            _ => s.menu.page_captures().iter().map(orbiters_n4n0_core::captures::CaptureRow::picture_key).collect(),
        };
        for id in ids {
            if let Some(Thumb::Failed { at, tries }) = s.pictures.get(&id) {
                if *tries < THUMB_TRIES && now >= *at + THUMB_RETRY * *tries as u32 {
                    followups.retry.push(id);
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
    if !listening {
        s.menu.dictation_ended();
    }
    s.menu.fast_walk = walk;
    s.menu.countdown = s.photo_at.map(|at| at.saturating_duration_since(now).as_secs_f32().ceil() as u32);
    let contacts = calls.menu_contacts();
    let (ready, note) = calls.menu_ready();
    if s.menu.contacts != contacts || s.menu.calls_ready != ready || s.menu.call_note != note {
        s.menu.set_contacts(contacts, ready, note);
    }
    let call = calls.menu_call(crate::unix_ms());
    if s.menu.call != call {
        s.menu.set_call(call);
    }
    // The card, wherever the menu shows; then the full pictures (the card's too).
    s.card_step(now, &mut out);
    s.picture_owner_step(now, &mut out);
    if !s.available {
        // The menu is not on this screen: only a close it still has to say.
        if s.present && s.open_sent == Some(true) {
            s.sync_open(&mut out);
        }
        return out;
    }
    // The fingers, then desktop mode: where the owner looks, and a left click. A press plays the overlay's sound.
    let presses = s.presses;
    s.point_and_touch(now, &mut followups.effects);
    s.look_and_click(vrchat, now, &mut followups.effects);
    followups.pressed |= s.presses != presses;
    if s.pressed.is_some_and(|(_, at)| now.duration_since(at) > PRESS_SHOWN && !s.holding.iter().any(|h| *h) && !s.clicking) {
        s.pressed = None;
    }
    // Listening, or watching the screen mirror's page (a video, nothing to press): the menu stays open.
    if s.menu.open && (listening || s.menu.page == Page::Mirror) {
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
    // The owner's screen: dropped or not, open or closed, then a block per step.
    s.sync_open(&mut out);
    if !s.menu.open {
        s.owner_settled = false;
        return out;
    }
    if now >= s.next_local {
        let frame = {
            let pictures = &s.pictures;
            let flags = Flags { listening, loading: s.menu.loading(), thinking, pictures: s.pictures_diagnostics };
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

/// The screen's turn on the text channel, when it is due: everyone else's screen, a block a step (its three steps a
/// The screen's and the card's turns on the text channel, when due: everyone else's screen a block a step (its three
/// steps a phase apart, then the pointer stays on it a whole step), and every client's card the same way. The panel,
/// the screen and the card take turns, a whole block each; whoever takes the channel after another moved the pointer
/// onto a block of its own rests the pointer first, so its values never write that block.
pub(crate) fn synced(rt: &mut N4n0Runtime, now: Instant) -> Vec<Message> {
    let panel_wants = rt.sender.as_ref().is_some_and(|p| p.wants_step(now));
    // A panel block between its steps finishes first.
    let panel_mid_block = rt.sender.as_ref().is_some_and(TextSender::mid_block);
    let step = rt.sender.as_ref().map_or(Duration::from_millis(300), TextSender::step);
    let sync = rt.sender.as_ref().map_or(8, TextSender::sync);
    let s = &mut rt.screen;
    if s.everyone.is_none() {
        return Vec::new();
    }
    let screen_wants = s.close_everyone || (s.menu.open && s.available);
    let card_wants = s.card.as_ref().is_some_and(|c| c.sender.wants(now));
    let pictures_want = s.picture_everyone.as_ref().is_some_and(Synced::wants);
    if !(screen_wants || card_wants || pictures_want) || now < s.next_synced || panel_mid_block {
        return Vec::new();
    }
    let screen_mid = s.everyone.as_ref().is_some_and(Audience::mid_block);
    let card_mid = s.card.as_ref().is_some_and(|c| c.sender.mid_block());
    let picture_mid = s.picture_everyone.as_ref().is_some_and(Synced::mid_block);
    // The panel wants the channel too: they take turns, a whole block each.
    if panel_wants && !s.screen_turn && !screen_mid && !card_mid && !picture_mid {
        s.screen_turn = true;
        s.next_synced = now + step;
        // The panel moves the pointer onto its own blocks.
        s.everyone.as_mut().unwrap().moved_elsewhere();
        if let Some(card) = s.card.as_mut() {
            card.sender.moved_elsewhere();
        }
        if let Some(pictures) = s.picture_everyone.as_mut() {
            pictures.moved_elsewhere();
        }
        return Vec::new();
    }
    // Full pictures (contracts Docs/n4n0-picture.md): a chunk between its steps finishes first; else, once the screen's page
    // went whole (its blocks only go again) or the screen wants nothing, three turns of every four, when the card waits for
    // nothing.
    let screen_settled = !screen_wants
        || (!s.close_everyone && !screen_mid && {
            let pictures = &s.pictures;
            let frame = Frame::public(&s.menu, Flags { loading: s.menu.loading(), pictures: s.pictures_diagnostics, ..Default::default() }, s.thumbnail, &|id: &str| thumbnail(pictures, id));
            s.everyone.as_ref().unwrap().settled(&frame.bank)
        });
    let picture_goes = pictures_want && (picture_mid || (!screen_mid && !card_mid && !card_wants && screen_settled && (!screen_wants || s.picture_turns % 4 != 3)));
    if picture_goes {
        s.screen_turn = false;
        let step_now = s.picture_everyone.as_mut().unwrap().step();
        let mut wait = step;
        let out = match step_now {
            Some(p) => {
                if p.moves_to_block() {
                    // The pointer on a picture chunk: the screen's and the card's next blocks rest first.
                    s.everyone.as_mut().unwrap().moved_elsewhere();
                    if let Some(card) = s.card.as_mut() {
                        card.sender.moved_elsewhere();
                    }
                    s.picture_turns = s.picture_turns.wrapping_add(1);
                    s.picture_sent[1] += 1;
                } else {
                    wait = orbiters_n4n0_core::text::PHASE_GAP;
                }
                p.messages()
            }
            None => Vec::new(),
        };
        s.next_synced = now + wait;
        if let Some(sender) = rt.sender.as_mut() {
            sender.hold_until(now + wait);
        }
        return out;
    }
    if screen_wants && !s.close_everyone {
        // The screen's own block: picture turns count again from it.
        if s.picture_turns % 4 == 3 {
            s.picture_turns = s.picture_turns.wrapping_add(1);
        }
    }
    s.screen_turn = false;
    let mut wait = step;
    // The card's turn: its block between its steps, else when the screen wants nothing or after the screen's block.
    let card_goes = card_wants && (card_mid || (!screen_mid && (!screen_wants || s.card_turn)));
    let out = if card_goes {
        let card = s.card.as_mut().unwrap();
        let send = card.sender.step(now);
        if send.moves_to_block() {
            // The pointer on the card's block: the screen's next block rests first; the screen goes next.
            s.everyone.as_mut().unwrap().moved_elsewhere();
            s.card_turn = false;
            if let Some(pictures) = s.picture_everyone.as_mut() {
                pictures.moved_elsewhere();
            }
        }
        if !send.holds() {
            wait = orbiters_n4n0_core::text::PHASE_GAP;
        }
        screen::card_messages(sync, s.thumbnail, &send)
    } else if s.close_everyone {
        s.close_everyone = false;
        // Closing clears everyone else's memory, and leaves the pointer where it writes nothing.
        s.everyone.as_mut().unwrap().cleared(true);
        s.card_turn = true;
        screen::synced_close()
    } else {
        // Everyone else's frame: no hover nor press, no friend named. A new page clears their memory first.
        let pictures = &s.pictures;
        let flags = Flags { loading: s.menu.loading(), pictures: s.pictures_diagnostics, ..Default::default() };
        let frame = Frame::public(&s.menu, flags, s.thumbnail, &|id: &str| thumbnail(pictures, id));
        let send = s.everyone.as_mut().unwrap().step(&frame.bank);
        if send.moves_to_block() {
            // The pointer on the screen's block: the card's next block rests first; the card goes next.
            if let Some(card) = s.card.as_mut() {
                card.sender.moved_elsewhere();
            }
            s.card_turn = true;
            if let Some(pictures) = s.picture_everyone.as_mut() {
                pictures.moved_elsewhere();
            }
        }
        if !send.holds() {
            wait = orbiters_n4n0_core::text::PHASE_GAP;
        }
        screen::synced_messages(sync, &send)
    };
    s.next_synced = now + wait;
    if let Some(sender) = rt.sender.as_mut() {
        sender.hold_until(now + wait);
    }
    out
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
    use crate::platform::MouseState;

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
    }

    #[test]
    fn reads_the_aims_others_fingertips_and_triggers() {
        let mut s = ScreenRuntime::default();
        assert!(s.aim_point("N4N0/Screen/Aim/R1", 0.4));
        assert!(!s.aim_point("N4N0/Screen/Touch/R0", 0.3));
        assert_eq!(s.aim[1][1], 0.4);
        assert_eq!(s.touched, [false, true]);
        assert!(s.others_point("N4N0/Screen/Others3", 0.7));
        assert!(!s.others_point("N4N0/Screen/Aim/L0", 0.7));
        assert!(s.others_touched && s.others[3] == 0.7);
        s.trigger(false, true);
        s.trigger(false, true);
        assert_eq!((s.trigger, s.pulled), ([true, false], [true, false]), "a change, once");
    }

    /// What receivers of `radius` at the touch points read for a 2 mm contact at `at` (the screen's space).
    fn reading(at: [f64; 3], radius: f64) -> [f32; 4] {
        touch::RECEIVERS.map(|c| {
            let d = ((at[0] - c[0]).powi(2) + (at[1] - c[1]).powi(2) + (at[2] - c[2]).powi(2)).sqrt() - 0.002;
            (1.0 - d / radius).clamp(0.0, 1.0) as f32
        })
    }

    /// The page pixel (`x`, `y`) in the screen's space.
    fn on_screen(x: f32, y: f32) -> [f64; 3] {
        [x as f64 / layout::WIDTH as f64 * touch::WIDTH - touch::WIDTH / 2.0, touch::HEIGHT / 2.0 - y as f64 / layout::HEIGHT as f64 * touch::HEIGHT, 0.0]
    }

    /// What VRChat sends of a hand's fingertip at `tip` and its aim when the finger points at `at` (both the screen's space).
    fn point(s: &mut ScreenRuntime, right: bool, tip: [f64; 3], at: [f64; 3]) {
        let along = [at[0] - tip[0], at[1] - tip[1], at[2] - tip[2]];
        let length = (along[0] * along[0] + along[1] * along[1] + along[2] * along[2]).sqrt();
        let aim = [0, 1, 2].map(|i| tip[i] + along[i] / length * orbiters_contracts::n4n0::AIM_DISTANCE);
        let hand = if right { "R" } else { "L" };
        for (i, v) in reading(tip, touch::REACH).into_iter().enumerate() {
            assert!(s.touch_point(&format!("{}{hand}{i}", screen::local::TOUCH_PREFIX), v));
        }
        for (i, v) in reading(aim, touch::REACH).into_iter().enumerate() {
            assert!(s.aim_point(&format!("{}{hand}{i}", screen::local::AIM_PREFIX), v));
        }
    }

    /// Home on a screen of version 7, in VR.
    fn pointing() -> ScreenRuntime {
        let mut s = ScreenRuntime::default();
        s.avatar(true, true, Some(8), 72);
        s.pointing = true;
        s.menu.open();
        s
    }

    #[test]
    fn a_finger_hovers_where_it_points_and_its_trigger_presses_there() {
        let mut s = pointing();
        let (x, y) = youtube_tile();
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        // From 45 cm away, beside the screen, pointing at the tile.
        point(&mut s, true, [0.2, -0.15, -0.45], on_screen(x, y));
        s.point_and_touch(ms(0), &mut Vec::new());
        let tile = s.hover.expect("the tile pointed at is hovered");
        assert!(s.pressed.is_none(), "pointing alone presses nothing");
        s.trigger(true, true);
        s.point_and_touch(ms(20), &mut Vec::new());
        assert_eq!(s.pressed.map(|(i, _)| i), Some(tile), "the trigger presses what the finger points at");
        assert_eq!(s.menu.page, Page::Search);
        // The other hand's trigger, pointing nowhere, presses nothing.
        s.trigger(false, true);
        s.point_and_touch(ms(40), &mut Vec::new());
        assert_eq!(s.menu.page, Page::Search);
        s.trigger(true, false);
        s.point_and_touch(ms(60), &mut Vec::new());
        assert!(!s.holding[1], "released while still pointing at the submenu: no held press remains");
        let back = Frame::layout(&s.menu).elements.iter().find(|e| e.target == menu::Target::Back).unwrap().rect;
        point(&mut s, true, [0.2, -0.15, -0.45], on_screen(back.x as f32 + back.w as f32 / 2.0, back.y as f32 + back.h as f32 / 2.0));
        s.point_and_touch(ms(80), &mut Vec::new());
        s.trigger(true, true);
        s.point_and_touch(ms(100), &mut Vec::new());
        assert_eq!(s.menu.page, Page::Home, "the next pull works in the submenu");
        assert_eq!(s.presses, 2);
        s.menu.close();
        s.point_and_touch(ms(120), &mut Vec::new());
        assert_eq!(s.holding, [false; 3], "closing during a press releases the input session");
        s.menu.open();
        point(&mut s, true, [0.2, -0.15, -0.45], on_screen(x, y));
        s.point_and_touch(ms(140), &mut Vec::new());
        assert_eq!(s.presses, 2, "a trigger still held when reopening does not click through");
        s.trigger(true, false);
        s.point_and_touch(ms(160), &mut Vec::new());
        s.trigger(true, true);
        s.point_and_touch(ms(180), &mut Vec::new());
        assert_eq!(s.menu.page, Page::Search);
        assert_eq!(s.presses, 3);
    }

    #[test]
    fn a_held_trigger_drags_the_music_along_its_timeline() {
        let mut s = pointing();
        s.menu.pages = Pages::Timeline;
        s.menu.media = Some(menu::Media { title: "Song".into(), can_seek: true, position: Some(0), duration: Some(320), ..Default::default() });
        s.menu.press(menu::Target::Music);
        let place = |i: u8| {
            let r = layout::cached(ScreenPage::Music).elements.iter().find(|e| e.target == menu::Target::Seek(i)).unwrap().rect;
            on_screen(r.x as f32 + r.w as f32 / 2.0, r.y as f32 + r.h as f32 / 2.0)
        };
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let seeks = |effects: &[Effect]| {
            effects.iter().filter_map(|e| if let Effect::Media(menu::MediaCommand::Seek(at)) = e { Some(*at) } else { None }).collect::<Vec<_>>()
        };
        // The trigger on segment 2 (of 16, 20 s each): the music to 40 s.
        point(&mut s, true, [0.0, -0.2, -0.45], place(2));
        s.point_and_touch(ms(0), &mut Vec::new());
        s.trigger(true, true);
        let mut effects = Vec::new();
        s.point_and_touch(ms(20), &mut effects);
        assert_eq!(seeks(&effects), vec![40]);
        // Held and moved along: each new segment moves it once; the same segment again does nothing.
        let mut effects = Vec::new();
        for (n, i) in [(40, 2), (60, 5), (80, 5), (100, 9)] {
            point(&mut s, true, [0.0, -0.2, -0.45], place(i));
            s.point_and_touch(ms(n), &mut effects);
        }
        assert_eq!(seeks(&effects), vec![100, 180]);
        assert_eq!(s.presses, 1, "a drag is no new press: one click");
        // Let go, then pointed elsewhere on the timeline: nothing more until pressed.
        s.trigger(true, false);
        s.point_and_touch(ms(120), &mut Vec::new());
        let mut effects = Vec::new();
        point(&mut s, true, [0.0, -0.2, -0.45], place(12));
        s.point_and_touch(ms(140), &mut effects);
        assert!(seeks(&effects).is_empty(), "{effects:?}");
    }

    #[test]
    fn a_fingertip_touching_the_screen_presses_and_so_does_someone_else_s() {
        let mut s = pointing();
        let (x, y) = youtube_tile();
        let target = on_screen(x, y);
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        // The left fingertip comes from in front onto the tile, no trigger.
        for (n, z) in [(0, -0.06), (20, -0.03), (40, -0.005)] {
            point(&mut s, false, [target[0], target[1], z], [target[0], target[1], 0.1]);
            s.point_and_touch(ms(n), &mut Vec::new());
        }
        assert_eq!(s.menu.page, Page::Search, "the touch pressed the tile");
        assert_eq!(s.presses, 1, "one press, one click of the overlay's sound");
        // Someone else's fingertip on Search's back button presses it too.
        let back = layout::cached(ScreenPage::of(&s.menu)).elements.iter().find(|e| e.target == menu::Target::Back).map(|e| e.rect).expect("a back button");
        let at = on_screen(back.x as f32 + back.w as f32 / 2.0, back.y as f32 + back.h as f32 / 2.0);
        for (n, z) in [(100, -0.06), (120, -0.03), (140, -0.005)] {
            for (i, v) in touch::RECEIVERS.iter().map(|c| {
                let d = ((at[0] - c[0]).powi(2) + (at[1] - c[1]).powi(2) + (z - c[2]).powi(2)).sqrt() - 0.008;
                (1.0 - d / touch::RADIUS).clamp(0.0, 1.0) as f32
            }).enumerate() {
                assert!(s.others_point(&format!("{}{i}", screen::local::OTHERS_PREFIX), v));
            }
            s.point_and_touch(ms(n), &mut Vec::new());
        }
        assert_eq!(s.menu.page, Page::Home, "someone else's touch went back");
        assert_eq!(s.presses, 2);
    }

    #[test]
    fn what_waits_never_wakes_the_n4n0_thread_at_once() {
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
        s.holding = [true, false, false];
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
        let eyes = orbiters_contracts::n4n0::params::GAZE_EYES;
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
    fn the_screen_s_own_menu_offers_drop_with_the_avatar_s_card_only() {
        use orbiters_n4n0_core::menu::Target;
        let mut rt = N4n0Runtime::default();
        rt.screen.avatar(true, true, Some(8), 72);
        let media = Watcher::new(|| Box::new(crate::media::NoMedia));
        let tick = |rt: &mut N4n0Runtime| {
            super::tick(rt, Instant::now(), &CallsStatus::default(), false, None, &media, &mut Followups::default());
        };
        rt.screen.menu.pages = Pages::Ten;
        tick(&mut rt);
        assert!(!rt.screen.menu.can_drop && !rt.screen.card_ready(), "an N4N0 without its card of its own (before screen version 7)");
        rt.screen.set_card_capable(true);
        tick(&mut rt);
        assert!(rt.screen.menu.can_drop && rt.screen.card_ready());
        // A copied result's Drop drops it (the card then goes out: its host's part).
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

    #[test]
    fn a_results_page_on_a_version_8_screen_sends_its_full_pictures_through_the_owner_s_values() {
        use orbiters_contracts::n4n0::params;
        let mut rt = N4n0Runtime::default();
        rt.screen.avatar(true, true, Some(8), 128);
        rt.screen.menu.pages = Pages::of_screen(Some(8));
        rt.screen.set_full_pictures(true, Some(8));
        let media = Watcher::new(|| Box::new(crate::media::NoMedia));
        let menu = &mut rt.screen.menu;
        menu.open();
        menu.search_for("cats");
        let videos: Vec<youtube::Video> = (0..5)
            .map(|i| youtube::Video { id: format!("video{i:06}"), title: format!("Cats {i}"), channel: "C".into(), duration: None, live: false })
            .collect();
        menu.results_arrived(1, youtube::Results { videos: videos.clone(), continuation: None }, false);
        assert_eq!(menu.page, Page::Results);
        for (i, v) in videos.iter().enumerate() {
            let rgb = orbiters_n4n0_core::picture::Rgb { width: 320, height: 180, pixels: (0..320 * 180 * 3).map(|p| ((p * (7 + i)) % 251) as u8).collect() };
            rt.screen.put_picture(&v.id, orbiters_n4n0_core::menu::screen::picture_from_rgb(&rgb));
        }
        let mut sent = Vec::new();
        let start = Instant::now();
        for step in 0..400u64 {
            sent.extend(super::tick(&mut rt, start + Duration::from_millis(step * 25), &CallsStatus::default(), false, None, &media, &mut Followups::default()));
        }
        assert!(sent.contains(&full::local_send(true)), "the bridge on");
        // Each chunk's first value: slot + 8 Ã— salt (contracts Docs/n4n0-picture.md, local chunks).
        let first = crate::runtime::parameter(&params::picture_value(0));
        let firsts: Vec<f32> = sent.iter().filter(|(a, _)| *a == first).filter_map(|(_, v)| if let rosc::OscType::Float(f) = v { Some(*f) } else { None }).collect();
        let slots: std::collections::BTreeSet<i32> = firsts.iter().map(|f| ((f * 127.0).round() as i32).rem_euclid(256) & 7).collect();
        assert_eq!(slots.into_iter().collect::<Vec<_>>(), vec![0, 1, 2, 3, 4], "the five results' pictures, {} chunks", firsts.len());
    }

    #[test]
    fn the_keyboard_follows_its_setting_or_the_device() {
        use orbiters_core::settings::KeyboardLayout as Setting;
        struct French;
        impl Locale for French {
            fn locale(&self) -> String {
                "fr-FR".into()
            }
            fn keyboard(&self) -> KeyboardLayout {
                KeyboardLayout::Azerty
            }
        }
        assert_eq!(keyboard(Setting::Auto, &French), KeyboardLayout::Azerty);
        assert_eq!(keyboard(Setting::Qwerty, &French), KeyboardLayout::Qwerty);
        assert_eq!(French.language(), "fr");
        assert_eq!(photo_setting(photo_length(orbiters_core::settings::Photo3dLength::Full)), orbiters_core::settings::Photo3dLength::Full);
    }
}
