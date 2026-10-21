//! Orb's menu in the tray service (contracts `Docs/orb.md`, Menu and Screen): on the avatar's projected screen when
//! the Orb prop has one, and the requests that open it in the SteamVR overlay otherwise.
//!
//! The screen's menu lives here, beside the OSC link: the orb thread writes its memory (the owner's through local
//! values, a block every tick; everyone else's through the text channel, in turn with the panel), the screen's touch
//! points say where a finger is, and what the menu asks for runs on threads of its own (YouTube, calls, a picture).

use super::orb::{OrbRuntime, Source};
use super::{NoticeKind, Shared};
use orbiters_core::osc::OscValue;
use orbiters_orb::menu::screen::{self, Audience, Flags, Frame, Picture, LOCAL_VALUES};
use orbiters_orb::menu::touch::{Gesture, Touch};
use orbiters_orb::menu::{layout, Call, Contact, Effect, KeyboardLayout, Menu, PhotoLength};
use orbiters_orb::text::{Message, TextSender};
use orbiters_orb::youtube;
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The owner's screen takes a block at most this often: VRChat's animator reads one block a frame, and two blocks in
/// one frame lose the first.
const LOCAL_STEP: Duration = Duration::from_millis(28);
/// The orb thread's tick while the screen is open.
pub const FAST_TICK: Duration = Duration::from_millis(14);
/// A press shows at least this long, however short the touch.
const PRESS_SHOWN: Duration = Duration::from_millis(160);
/// The menu closes after this long without a touch, a press or a word.
const IDLE_CLOSE: Duration = Duration::from_secs(120);

/// What a thread brings back.
enum Done {
    Suggestions(String, anyhow::Result<Vec<String>>),
    Results { query: String, more: bool, result: anyhow::Result<youtube::Results> },
    Picture(String, Option<Picture>),
}

pub struct ScreenRuntime {
    /// The avatar's Orb prop has the projected screen.
    pub available: bool,
    pub menu: Menu,
    owner: Audience,
    everyone: Option<Audience>,
    /// What `Orb/Screen/Open` was last set to.
    open_sent: Option<bool>,
    /// Everyone else's screen must be told it closed.
    close_everyone: bool,
    next_local: Instant,
    /// The screen's next turn on the text channel.
    next_synced: Instant,
    /// Whose turn it is when the panel and the screen both have something to send.
    screen_turn: bool,
    proximity: [[f32; 4]; 2],
    touched: [bool; 2],
    touch: [Touch; 2],
    hover: Option<usize>,
    pressed: Option<(usize, Instant)>,
    holding: [bool; 2],
    pictures: HashMap<String, Option<Picture>>,
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
}

impl Default for ScreenRuntime {
    fn default() -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        let now = Instant::now();
        Self {
            available: false,
            menu: Menu::default(),
            owner: Audience::new(LOCAL_VALUES),
            everyone: None,
            open_sent: None,
            close_everyone: false,
            next_local: now,
            next_synced: now,
            screen_turn: true,
            proximity: [[0.0; 4]; 2],
            touched: [false; 2],
            touch: [Touch::default(); 2],
            hover: None,
            pressed: None,
            holding: [false; 2],
            pictures: HashMap::new(),
            done_tx,
            done_rx,
            last_activity: now,
            overlay_open: false,
            overlay_dictation: false,
            menu_request: 0,
            menu_toggle: false,
            dictation: None,
            photo_at: None,
        }
    }
}

impl ScreenRuntime {
    /// The avatar changed: whether it has the screen, with `sync` text values (its memory is unknown again).
    pub fn avatar(&mut self, available: bool, sync: Option<usize>) {
        self.available = available && sync.is_some_and(screen::fits);
        self.everyone = sync.filter(|_| self.available).map(Audience::new);
        self.owner.forget();
        self.open_sent = None;
        if !self.available {
            self.menu.close();
        }
    }

    /// Seconds before the 3D picture starts.
    pub fn countdown(&self, now: Instant) -> Option<u32> {
        self.photo_at.map(|at| at.saturating_duration_since(now).as_secs_f32().ceil() as u32)
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
        } else {
            // The overlay opens its menu (or closes it, when it is open and this was a toggle).
            rt.screen.menu_request = rt.screen.menu_request.wrapping_add(1);
            rt.screen.menu_toggle = toggle;
        }
        drop(rt);
        self.menu_listen();
        self.orb_publish();
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
        let opened = {
            let mut rt = self.orb.lock().unwrap();
            let opened = open && !rt.screen.overlay_open;
            rt.screen.overlay_open = open;
            rt.screen.overlay_dictation = open && dictation;
            opened
        };
        if opened {
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

    /// Carries out what the screen's menu asked for.
    pub(super) fn screen_effects(self: &Shared, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Suggest(text) => {
                    let tx = self.orb.lock().unwrap().screen.done_tx.clone();
                    std::thread::spawn(move || {
                        let result = youtube::suggestions(&text, &language());
                        let _ = tx.send(Done::Suggestions(text, result));
                    });
                }
                Effect::Search(query) => {
                    let tx = self.orb.lock().unwrap().screen.done_tx.clone();
                    std::thread::spawn(move || {
                        let result = youtube::search(&query, &language());
                        let _ = tx.send(Done::Results { query, more: false, result });
                    });
                }
                Effect::More { query, token } => {
                    let tx = self.orb.lock().unwrap().screen.done_tx.clone();
                    std::thread::spawn(move || {
                        let result = youtube::more(&token, &language());
                        let _ = tx.send(Done::Results { query, more: true, result });
                    });
                }
                Effect::Thumbnails(ids) => {
                    let (tx, wanted) = {
                        let mut rt = self.orb.lock().unwrap();
                        let s = &mut rt.screen;
                        // Keep the pictures of the last few pages only.
                        if s.pictures.len() > 60 {
                            s.pictures.retain(|id, _| ids.contains(id));
                        }
                        let wanted: Vec<String> = ids.into_iter().filter(|id| !s.pictures.contains_key(id)).collect();
                        for id in &wanted {
                            s.pictures.insert(id.clone(), None);
                        }
                        (s.done_tx.clone(), wanted)
                    };
                    for id in wanted {
                        let tx = tx.clone();
                        std::thread::spawn(move || {
                            let picture = youtube::thumbnail(&id).ok().and_then(|jpeg| screen::picture_of(&jpeg).ok());
                            let _ = tx.send(Done::Picture(id, picture));
                        });
                    }
                }
                Effect::Copy { link, .. } => {
                    if let Err(e) = super::orb::copy_to_clipboard(&link) {
                        self.notify(NoticeKind::Error, e);
                    }
                }
                Effect::FastWalk(on) => {
                    let mut settings = self.settings();
                    settings.walk_boost.enabled = on;
                    self.save_settings(settings);
                }
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
                    let mut settings = self.settings();
                    settings.photo_3d_timer = timer;
                    settings.photo_3d = photo_setting(length);
                    self.save_settings(settings);
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
    /// the steps to the screen. Returns the messages for VRChat.
    pub(super) fn screen_tick(
        rt: &mut OrbRuntime,
        now: Instant,
        snapshot_calls: &crate::state::CallsStatus,
        settings_walk: bool,
        effects: &mut Vec<Effect>,
        photo_due: &mut bool,
    ) -> Vec<Message> {
        let mut out = Vec::new();
        // The screen stands where the panel would show: what Orb says goes on the screen's Home instead.
        if rt.screen.available && rt.screen.menu.open {
            if let Some(sender) = rt.sender.as_mut().filter(|s| s.busy() && !s.sticky()) {
                if let Some(text) = sender.current_text() {
                    rt.screen.menu.heard = Some(text.split_whitespace().collect::<Vec<_>>().join(" "));
                }
                sender.hide();
            }
        }
        let listening = rt.listening.is_some();
        let thinking = rt.thinking;
        let s = &mut rt.screen;
        if s.photo_at.is_some_and(|at| now >= at) {
            s.photo_at = None;
            *photo_due = true;
        }
        while let Ok(done) = s.done_rx.try_recv() {
            match done {
                Done::Suggestions(text, Ok(list)) => s.menu.suggestions_arrived(&text, list),
                Done::Suggestions(_, Err(_)) => {}
                Done::Results { query, more, result: Ok(results) } => effects.extend(s.menu.results_arrived(&query, results, more)),
                Done::Results { query, result: Err(e), .. } => s.menu.results_failed(&query, &e.to_string()),
                Done::Picture(id, picture) => {
                    s.pictures.insert(id, picture);
                }
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
        // The finger: each hand's four touch points, wherever they changed.
        let layout = Frame::layout(&s.menu);
        for hand in 0..2 {
            if !std::mem::take(&mut s.touched[hand]) || !s.menu.open {
                continue;
            }
            match s.touch[hand].update(s.proximity[hand]) {
                Gesture::Press { x, y } => {
                    s.last_activity = now;
                    s.holding[hand] = true;
                    if let Some((index, target)) = layout::hit(&layout, x, y) {
                        if !layout::state(&s.menu, &layout.elements[index]).hidden {
                            s.pressed = Some((index, now));
                            effects.extend(s.menu.press(target));
                        }
                    }
                }
                Gesture::Hover { x, y } | Gesture::Hold { x, y } => {
                    s.hover = layout::hit(&layout, x, y).map(|(i, _)| i).filter(|&i| !layout::state(&s.menu, &layout.elements[i]).hidden);
                }
                Gesture::Away => {
                    s.holding[hand] = false;
                    if !s.holding.iter().any(|h| *h) {
                        s.hover = None;
                    }
                }
            }
        }
        if s.pressed.is_some_and(|(_, at)| now.duration_since(at) > PRESS_SHOWN && !s.holding.iter().any(|h| *h)) {
            s.pressed = None;
        }
        if s.menu.open && listening {
            s.last_activity = now;
        }
        if s.menu.open && now.duration_since(s.last_activity) > IDLE_CLOSE {
            effects.extend(s.menu.close());
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
            return out;
        }
        let frame = {
            let pictures = &s.pictures;
            let flags = Flags { listening, loading: s.menu.loading(), thinking };
            Frame::new(&s.menu, s.hover, s.pressed.map(|(i, _)| i), flags, &|id: &str| pictures.get(id).copied().flatten())
        };
        if now >= s.next_local {
            s.next_local = now + LOCAL_STEP;
            if s.owner.take_clear() {
                out.extend(screen::local_clear());
            } else {
                // A changed block, else one in turn (a block VRChat dropped comes back within a round), in three steps.
                out.extend(screen::local_messages(&s.owner.step(&frame.bank)));
            }
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
            everyone.forget();
            screen::synced_close()
        } else {
            let frame =
                Frame::new(&s.menu, s.hover, s.pressed.map(|(i, _)| i), Flags { listening: false, loading: s.menu.loading(), thinking: false }, &|id: &str| {
                    s.pictures.get(id).copied().flatten()
                });
            // Everyone else's memory starts clear (closing cleared it): the screen opens with its first block.
            everyone.take_clear();
            let send = everyone.step(&frame.bank);
            if !send.moves_to_block() {
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

    /// What the screen's touch points and buttons send (on the OSC receiving thread: record only).
    pub(super) fn screen_parameter(&self, name: &str, value: &OscValue) -> bool {
        if !name.starts_with("Orb/Screen/") {
            return false;
        }
        if let OscValue::Float(v) = value {
            self.orb.lock().unwrap().screen.touch_point(name, *v);
        }
        true
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
}
