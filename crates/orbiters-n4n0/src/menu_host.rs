//! N4N0's menu (`orbiters_n4n0_core::menu`) run where an interface draws it: the desktop's SteamVR overlay and its window's
//! Home, a phone's screen. It keeps the menu in step with N4N0 (listening, fast walk, calls, the light, the 3D picture's
//! countdown, the avatar's saved states, whether a link can be dropped as a card), fetches YouTube's suggestions,
//! results and thumbnails itself, reads the music of its process's [`Watcher`], and asks N4N0 for the rest (calls, the
//! clipboard, a 3D picture, fast walk, the light, saving and applying states, dropping a card).
//!
//! The pictures are the interface's own ([`Pictures`]): egui textures on the desktop, a phone toolkit's images there.

use crate::media::Watcher;
use crate::platform::Locale;
use crate::screen::{keyboard, mode_setting, photo_length, photo_mode, photo_setting};
use crate::status::{N4n0Status, StateAction};
use crate::{Command, MenuHolder};
use orbiters_core::settings::Settings;
use orbiters_n4n0_core::menu::{Effect, Light, Menu, Page, Pages, StateRow, SuggestQueue, Target};
use orbiters_n4n0_core::picture::{self, Rgb};
use orbiters_n4n0_core::youtube;
use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// A light the menu set shows until N4N0 says the same, or this long.
const LIGHT_ECHO: Duration = Duration::from_secs(2);
/// A thumbnail that failed is tried again after this (times the tries), so many times.
const THUMB_RETRY: Duration = Duration::from_secs(3);
const THUMB_TRIES: u8 = 3;

/// How an interface keeps the pictures the menu shows (thumbnails, the music's art).
pub trait Pictures {
    type Picture;
    /// What the interface draws of `image`, named `name` (a texture's label).
    fn make(&self, name: &str, image: &Rgb) -> Self::Picture;
}

/// Reads a 3D capture's picture by its key (`CaptureRow::key`) where the interface runs: the computer's sessions for
/// the desktop's overlay and window. Called on a thread of its own.
pub type CapturePictures = Arc<dyn Fn(&str) -> Option<Rgb> + Send + Sync>;

/// Tells an interface that draws only when something changed that what the menu fetches on its threads arrived
/// (YouTube's suggestions and results, a thumbnail): [`MenuHost::poll`] has it. Called on those threads.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// What the menu asks of N4N0 and of the app's settings, from where it is drawn.
pub trait MenuService {
    /// Asks N4N0 for `command`, stamped with what its surface shows when it acts for the person ([`Command::stamped`]).
    fn command(&self, command: Command);
    /// Saves what changed from `before` to `after` in the settings.
    fn save_settings(&self, before: &Settings, after: &Settings);
}

enum Done {
    Suggestions { id: u64, text: String, result: anyhow::Result<Vec<String>> },
    Results { id: u64, more: bool, result: anyhow::Result<youtube::Results> },
    Thumbnail { id: String, tries: u8, image: Option<Rgb> },
}

enum Thumb<P> {
    Loading,
    Ready(P),
    /// The last try failed at `at`.
    Failed {
        at: Instant,
        tries: u8,
    },
}

/// Sets `slot` to `value`, saying whether that changed it.
/// How long a switch pressed here shows what it was pressed to while the settings or N4N0 still say the old value
/// (MENU-11: the light's [`LIGHT_ECHO`], for the other switches; the assistant and smoothing go through N4N0's process).
const SETTING_ECHO: Duration = Duration::from_secs(3);

/// A value pressed here, kept over what the host says until it says the same or [`SETTING_ECHO`] passed: a switch does
/// not jump back for the round trip, and a second press counts from what the first asked.
#[derive(Debug)]
struct Echo<T> {
    set: Option<(T, Instant)>,
}

impl<T> Default for Echo<T> {
    fn default() -> Self {
        Self { set: None }
    }
}

impl<T: PartialEq + Clone> Echo<T> {
    fn press(&mut self, value: T) {
        self.set = Some((value, Instant::now()));
    }

    /// What to show, the host saying `said`.
    fn shown(&mut self, said: T) -> T {
        match &self.set {
            Some((value, at)) if *value != said && at.elapsed() < SETTING_ECHO => value.clone(),
            _ => {
                self.set = None;
                said
            }
        }
    }

    /// The value asked and not yet said, if any (the base a second press counts from).
    fn asked(&self) -> Option<&T> {
        self.set.as_ref().filter(|(_, at)| at.elapsed() < SETTING_ECHO).map(|(v, _)| v)
    }

    fn due(&self) -> Option<Instant> {
        self.set.as_ref().map(|(_, at)| *at + SETTING_ECHO)
    }
}

/// The switches with an echo (MENU-11).
#[derive(Debug, Default)]
struct Echoes {
    fast_walk: Echo<bool>,
    mismatch: Echo<bool>,
    assistant: Echo<bool>,
    photo: Echo<(u32, orbiters_n4n0_core::menu::PhotoLength, orbiters_n4n0_core::menu::PhotoMode)>,
    tracking: Echo<(bool, [u16; 3])>,
}

fn set<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        return false;
    }
    *slot = value;
    true
}

pub struct MenuHost<P: Pictures> {
    pub menu: Menu,
    pictures: P,
    locale: Arc<dyn Locale>,
    media: Arc<Watcher>,
    thumbs: HashMap<String, Thumb<P::Picture>>,
    suggest: SuggestQueue,
    done_tx: mpsc::Sender<Done>,
    done_rx: mpsc::Receiver<Done>,
    /// The media reading the menu shows (its version: [`Watcher::reading`]).
    media_seen: u64,
    /// What play or pause was last asked to become, and when.
    play_asked: Option<(bool, Instant)>,
    /// The light as last set here, and when: what N4N0 says replaces it once it says the same (or after
    /// [`LIGHT_ECHO`]), so a switch does not jump back while N4N0 is told.
    light_set: Option<(Light, Instant)>,
    /// The other switches as last pressed here (MENU-11).
    echoes: Echoes,
    /// Something the menu shows changed since it was last drawn.
    pub dirty: bool,
    language_seen: orbiters_i18n::Language,
    /// The saved states as N4N0 last listed them (and whether the avatar is known), and the number of the last note of
    /// theirs seen.
    states_seen: Option<(Vec<StateRow>, bool)>,
    note_seen: Option<u64>,
    /// A link was dropped as a card since the host last looked (the overlay then closes: the card is in the world).
    pub dropped: bool,
    /// The number of the last suggestion result N4N0 published that this menu saw (the first status sets it).
    suggestion_seen: Option<u32>,
    /// The same for the last Invite me's answer.
    invite_seen: Option<u32>,
    /// The number of the last key of the computer's keyboard taken (the first status sets it).
    keys_seen: Option<u32>,
    /// The number of the last 3D picture note N4N0 published that this menu saw (the first status sets it).
    photo_note_seen: Option<u32>,
    /// Where the 3D captures' pictures are read (none: they show as placeholders).
    capture_pictures: Option<CapturePictures>,
    /// The worlds' feedback in the last status taken: the app's changes replace the menu's (its own presses show at once).
    feedback_seen: orbiters_n4n0_core::feedback::FeedbackView,
    /// Told when something fetched arrived (none: the interface draws all the time, as the overlay does).
    waker: Option<Waker>,
    /// Which menu this is, for the scanner and the friends it holds (MENU-13).
    holder: MenuHolder,
    /// Who looks at this computer's previous instances: another VRChat account's are not shown.
    viewer: orbiters_core::visited::Viewer,
}

impl<P: Pictures> MenuHost<P> {
    /// A menu whose pictures `pictures` keeps, in the language and keyboard `locale` says, showing the music `media`
    /// reads.
    pub fn new(pictures: P, locale: Arc<dyn Locale>, media: Arc<Watcher>) -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        Self {
            language_seen: orbiters_i18n::language(),
            menu: Menu::default(),
            pictures,
            locale,
            media,
            thumbs: HashMap::new(),
            suggest: SuggestQueue::default(),
            done_tx,
            done_rx,
            media_seen: 0,
            play_asked: None,
            light_set: None,
            echoes: Echoes::default(),
            dirty: true,
            states_seen: None,
            note_seen: None,
            dropped: false,
            suggestion_seen: None,
            invite_seen: None,
            keys_seen: None,
            photo_note_seen: None,
            capture_pictures: None,
            feedback_seen: Default::default(),
            waker: None,
            holder: MenuHolder::Any,
            viewer: Default::default(),
        }
    }

    /// Who looks now: what was listed for another goes from the list at once.
    pub fn seen_by(&mut self, viewer: orbiters_core::visited::Viewer) {
        if self.viewer != viewer {
            self.viewer = viewer;
            let rows = orbiters_n4n0_core::invites::shown(self.menu.instances.clone(), &self.viewer);
            if rows != self.menu.instances {
                let (ready, note) = (self.menu.instances_ready, self.menu.instances_note.clone());
                self.menu.set_instances(rows, ready, &note);
                self.dirty = true;
            }
        }
    }

    /// The same, as `holder` (the SteamVR menu, the panels, the window's Home): its scanner and friends are its own.
    pub fn with_holder(mut self, holder: MenuHolder) -> Self {
        self.holder = holder;
        self
    }

    /// The page stays this menu's but is no longer seen (a sheet closed, the window's Home left): the scanner and the
    /// friends it holds are let go now ([`Menu::let_go`]).
    pub fn let_go(&mut self, service: &dyn MenuService) {
        for effect in self.menu.let_go() {
            match effect {
                Effect::QrScan(on) => service.command(Command::Qr { on, holder: self.holder }),
                Effect::Friends(on) => service.command(Command::Friends { on, holder: self.holder }),
                _ => {}
            }
        }
    }

    /// The same, telling `waker` when something fetched arrived: for an interface that is still until something
    /// changes (the window), which then need not look by itself several times a second. Together with
    /// [`MenuHost::next_wake`], which says when the clock alone brings something.
    pub fn with_waker(mut self, waker: Waker) -> Self {
        self.waker = Some(waker);
        self
    }

    /// The same, reading the 3D captures' pictures with `read`.
    pub fn with_capture_pictures(mut self, read: CapturePictures) -> Self {
        self.capture_pictures = Some(read);
        self
    }

    /// Opens the menu on Home with the settings it starts from (keyboard, the 3D picture's timer and length). It has
    /// every tile: the interfaces draw every page.
    pub fn open(&mut self, settings: &Settings) {
        self.menu.layout = keyboard(settings.n4n0.menu.keyboard, &*self.locale);
        self.menu.photo_timer = settings.photo_3d_timer;
        self.menu.photo_length = photo_length(settings.photo_3d);
        self.menu.photo_mode = photo_mode(settings.photo_3d_mode);
        self.menu.pages = Pages::ALL;
        self.menu.open();
        self.dirty = true;
    }

    /// A press on `target`: what it does follows from the effects it returns.
    #[must_use = "a host runs what the menu asks"]
    pub fn press(&mut self, target: Target) -> Vec<Effect> {
        self.dirty = true;
        let effects = self.menu.press(target);
        // It switched on the press itself: what the app says a moment later, still the old state, does not switch it back.
        if matches!(target, Target::MediaPlayPause) {
            self.play_asked = self.menu.media.as_ref().map(|m| (m.playing, Instant::now()));
        }
        effects
    }

    /// Keeps the menu's picture of what N4N0 and the settings say (calls, fast walk, listening, the picture's
    /// countdown), field by field: the menu (its results and suggestions included) is not copied every frame to find a
    /// change.
    pub fn follow(&mut self, n4n0: &N4n0Status, settings: &Settings) {
        // Another language: all of it is drawn again.
        if std::mem::replace(&mut self.language_seen, orbiters_i18n::language()) != self.language_seen {
            self.dirty = true;
        }
        let m = &mut self.menu;
        let mut changed = set(&mut m.listening, n4n0.listening);
        // What N4N0 says shows on Home too.
        if let Some(panel) = &n4n0.panel {
            if m.heard_private || !m.heard.as_deref().is_some_and(|h| h.split_whitespace().eq(panel.split_whitespace())) {
                m.heard = Some(panel.split_whitespace().collect::<Vec<_>>().join(" "));
                m.heard_private = false;
                changed = true;
            }
        }
        // Each switch as pressed here until the settings say so too (MENU-11).
        let e = &mut self.echoes;
        changed |= set(&mut m.fast_walk, e.fast_walk.shown(settings.walk_boost.enabled));
        // The overlay's Suggest features has the switch of the menu mismatch buttons, the setting Settings › Beta feedback shows too.
        changed |= set(&mut m.mismatch_buttons, Some(e.mismatch.shown(settings.mismatch.buttons)));
        // Home's microphone switches N4N0's assistant: whether it is on (nothing to switch while N4N0 is off).
        changed |= set(&mut m.assistant, settings.n4n0.enabled.then(|| e.assistant.shown(settings.n4n0.assistant)));
        changed |= set(&mut m.desktop_mode, n4n0.desktop_mode);
        let said = n4n0.light.map(|l| Light { on: l.on, color: l.color, level: l.level });
        let light = match self.light_set {
            Some((set, at)) if said != Some(set) && at.elapsed() < LIGHT_ECHO => Some(set),
            _ => {
                self.light_set = None;
                said
            }
        };
        changed |= set(&mut m.light, light);
        changed |= set(&mut m.countdown, n4n0.photo_countdown);
        changed |= set(&mut m.can_drop, n4n0.can_drop);
        // The link card out in the world: Take back.
        changed |= set(&mut m.card_out, n4n0.card.is_some());
        // What stopped a 3D picture (VRChat's camera closed), on the 3D picture page.
        match (&n4n0.photo_note, self.photo_note_seen) {
            (Some((n, text)), Some(seen)) if *n != seen => {
                if m.page == Page::Photo {
                    m.notice = Some(text.clone());
                    changed = true;
                }
                self.photo_note_seen = Some(*n);
            }
            (Some((n, _)), None) => self.photo_note_seen = Some(*n),
            (None, None) => self.photo_note_seen = Some(0),
            _ => {}
        }
        // The avatar's saved states (a state saved here shows until N4N0 lists it), and what N4N0 said of the last save
        // or apply, on the States page.
        let view = &n4n0.states;
        // Compared with what was last listed where they are: this runs every turn of a menu that shows (90 a second in
        // the headset), and the rows are made, with their names copied, only when they differ.
        let as_seen = self.states_seen.as_ref().is_some_and(|(rows, known)| {
            *known == view.avatar_id.is_some()
                && rows.len() == view.states.len()
                && rows.iter().zip(&view.states).all(|(row, s)| row.id == s.id && row.name == s.name && row.default == s.default && row.pose == s.pose && row.size == s.size)
        });
        if !as_seen || m.saving() {
            let rows: Vec<StateRow> =
                view.states.iter().map(|s| StateRow { id: s.id.clone(), name: s.name.clone(), default: s.default, pose: s.pose, size: s.size }).collect();
            let listed = (rows, view.avatar_id.is_some());
            m.set_states(listed.0.clone(), listed.1, orbiters_i18n::mark("VRChat's avatar is not known yet"));
            self.states_seen = Some(listed);
            changed = true;
        }
        if let Some(note) = &view.note {
            // What went wrong replaces the line the press showed at once ("Saved as…", "Applied…").
            if self.note_seen.is_some_and(|seen| seen != note.id) && note.error && m.page == Page::States {
                m.notice = Some(note.text.clone());
                changed = true;
            }
            self.note_seen = Some(note.id);
        } else {
            self.note_seen.get_or_insert(0);
        }
        let (timer, length, mode) = self.echoes.photo.shown((settings.photo_3d_timer, photo_length(settings.photo_3d), photo_mode(settings.photo_3d_mode)));
        changed |= set(&mut m.photo_timer, timer);
        changed |= set(&mut m.photo_length, length);
        changed |= set(&mut m.photo_mode, mode);
        // Tracking smoothing, the versions, and how the last suggestion sent from here went.
        let echoes = &mut self.echoes;
        let tracking = n4n0.tracking.as_ref().map(|t| {
            let (on, ms) = echoes.tracking.shown((t.on, t.ms));
            orbiters_n4n0_core::menu::Tracking { on, ms, note: t.note.clone() }
        });
        changed |= set(&mut m.tracking, tracking);
        // Compared before they are copied.
        if m.tracking_note != n4n0.tracking_note {
            m.tracking_note = n4n0.tracking_note.clone();
            changed = true;
        }
        if m.versions != n4n0.versions {
            m.versions = n4n0.versions.clone();
            changed = true;
        }
        let video = n4n0.world_video.as_ref().map(|(url, title, at)| orbiters_n4n0_core::menu::WorldVideo { url: url.clone(), title: title.clone(), at: *at });
        changed |= set(&mut m.world_video, video);
        if !n4n0.listening && m.dictating {
            m.dictation_ended();
            changed = true;
        }
        match (&n4n0.suggestion, self.suggestion_seen) {
            (Some((n, error)), Some(seen)) if *n != seen => {
                if m.sending {
                    m.suggestion_sent(error.clone().map_or(Ok(()), Err));
                    changed = true;
                }
                self.suggestion_seen = Some(*n);
            }
            (Some((n, _)), None) => self.suggestion_seen = Some(*n),
            (None, None) => self.suggestion_seen = Some(0),
            _ => {}
        }
        changed |= set(&mut m.mirror, n4n0.mirror);
        if m.qr != n4n0.qr {
            m.qr = n4n0.qr.clone();
            changed = true;
        }
        // The 3D captures (their pictures fetched as they show) and Share to …'s friends.
        let mut pictures = Vec::new();
        if m.captures != *n4n0.captures {
            pictures = m.set_captures((*n4n0.captures).clone());
            changed = true;
        }
        if m.friends != *n4n0.friends {
            m.set_friends((*n4n0.friends).clone());
            changed = true;
        }
        // World feedback: taken when the app's changes (the menu's own presses show at once, before the app has them).
        if self.feedback_seen != n4n0.feedback {
            self.feedback_seen = n4n0.feedback.clone();
            m.set_feedback(n4n0.feedback.clone());
            changed = true;
        }
        match (&n4n0.invite, self.invite_seen) {
            (Some((n, answer)), Some(seen)) if *n != seen => {
                if m.invite_waiting {
                    m.invite_answered(answer.clone());
                    changed = true;
                }
                self.invite_seen = Some(*n);
            }
            (Some((n, _)), None) => self.invite_seen = Some(*n),
            (None, None) => self.invite_seen = Some(0),
            _ => {}
        }
        let calls = &n4n0.calls;
        // Calls not open to this account: Call greyed, "Coming soon" (before the contacts, which then mean nothing).
        if m.calls_soon != calls.coming_soon() {
            m.set_calls_soon(calls.coming_soon());
            changed = true;
        }
        let (ready, note) = calls.menu_ready();
        let same_contacts = m.contacts.len() == calls.contacts.len() && m.contacts.iter().zip(&calls.contacts).all(|(a, b)| a.id == b.id && a.name == b.name);
        if !same_contacts || m.calls_ready != ready || m.call_note != note {
            m.set_contacts(calls.menu_contacts(), ready, note);
            changed = true;
        }
        if calls.call.is_some() || m.call.is_some() {
            let call = calls.menu_call(crate::unix_ms());
            if m.call != call {
                m.set_call(call);
                changed = true;
            }
        }
        if changed {
            self.dirty = true;
        }
        for effect in pictures {
            if let Effect::Thumbnails(ids) = effect {
                self.thumbnails(ids);
            }
        }
    }

    /// The computer's keys N4N0 handed this menu since it last looked (none the first time: those were for an
    /// earlier menu); what they ask of the host follows from the effects returned.
    pub fn take_keys(&mut self, n4n0: &N4n0Status) -> Vec<Effect> {
        let newest = n4n0.keys.last().map_or(0, |(n, _)| *n);
        let Some(seen) = self.keys_seen.replace(newest) else { return Vec::new() };
        let mut effects = Vec::new();
        for (n, key) in &n4n0.keys {
            // Numbers wrap: the ones after `seen`.
            if n.wrapping_sub(seen) as i32 > 0 {
                effects.extend(self.menu.key(key.clone(), || None));
                self.dirty = true;
            }
        }
        effects
    }

    /// The music the menu shows, from this process's media reading (read only while a menu wants it), and its art as a
    /// picture.
    pub fn follow_media(&mut self) {
        self.follow_media_at(Instant::now());
    }

    /// [`MenuHost::follow_media`] at `now`.
    fn follow_media_at(&mut self, now: Instant) {
        let media = self.media.clone();
        media.want();
        let (version, reading) = media.reading();
        // What was asked of play or pause shows until the app agrees or has had its time. When that time is up the
        // app is believed again, new reading or not: an interface drawn only when what it shows changed (the window's
        // Home, which does not show where the music is) gets no other reading to do it on, and went on showing what
        // was asked of an app that never did it.
        let caught_up = self.play_asked.is_some_and(|(_, at)| now.saturating_duration_since(at) >= MEDIA_CATCHES_UP);
        if version == self.media_seen && !caught_up {
            return;
        }
        self.media_seen = version;
        let mut read = reading.media;
        if let Some(media) = read.as_mut() {
            media.playing = settled_playing(media.playing, &mut self.play_asked, now);
        }
        self.menu.media = read;
        self.menu.volume = reading.volume;
        if let Some(art) = reading.art {
            if !matches!(self.thumbs.get(&art.key), Some(Thumb::Ready(..))) {
                let ready = self.pictures.make(&art.key, &art.rgb);
                self.thumbs.insert(art.key.clone(), Thumb::Ready(ready));
            }
        }
        self.dirty = true;
    }

    /// What came back from YouTube, the suggestions once typing paused (one request at a time), and the thumbnails of
    /// the results shown that failed, once their wait is over. Returns what follows from them.
    pub fn poll(&mut self) -> Vec<Effect> {
        // What a page left by itself asked (a call that rang on the scanner's page: MENU-8).
        let mut effects = self.menu.drain();
        while let Ok(done) = self.done_rx.try_recv() {
            match done {
                Done::Suggestions { id, text, result } => {
                    self.suggest.answered(id);
                    match result {
                        Ok(list) => self.menu.suggestions_arrived(id, &text, list),
                        Err(e) => eprintln!("N4N0 menu: YouTube suggestions: {e:#}"),
                    }
                }
                Done::Results { id, more, result: Ok(results) } => effects.extend(self.menu.results_arrived(id, results, more)),
                Done::Results { id, result: Err(e), .. } => self.menu.results_failed(id, &e.to_string()),
                Done::Thumbnail { id, tries, image } => {
                    let thumb = match image {
                        // Made under the picture's own key, the one the pages ask for: under "yt-{id}" its pixels were
                        // kept where no page looked (OVERLAY-26: the first fetched thumbnail did not show on another canvas).
                        Some(image) => Thumb::Ready(self.pictures.make(&id, &image)),
                        None => Thumb::Failed { at: Instant::now(), tries: tries + 1 },
                    };
                    self.thumbs.insert(id, thumb);
                }
            }
            self.dirty = true;
        }
        if let Some((id, text)) = self.suggest.due(Instant::now()) {
            let (tx, lang, waker) = (self.done_tx.clone(), self.locale.language(), self.waker.clone());
            std::thread::spawn(move || {
                let result = youtube::suggestions(&text, &lang);
                let _ = tx.send(Done::Suggestions { id, text, result });
                wake(&waker);
            });
        }
        let now = Instant::now();
        let retry: Vec<String> = self.shown_pictures().into_iter().filter(|id| self.retry_at(id).is_some_and(|at| now >= at)).collect();
        if !retry.is_empty() {
            self.thumbnails(retry);
        }
        effects
    }

    /// The pictures of the page shown that are fetched again when they failed: the results', the 3D captures'.
    fn shown_pictures(&self) -> Vec<String> {
        match self.menu.page {
            Page::Results => self.menu.page_results().iter().map(|v| v.id.clone()).collect(),
            Page::History => self.menu.page_captures().iter().map(orbiters_n4n0_core::captures::CaptureRow::picture_key).collect(),
            _ => Vec::new(),
        }
    }

    /// When the picture `id` is tried again, if it failed and still has tries left.
    fn retry_at(&self, id: &str) -> Option<Instant> {
        match self.thumbs.get(id) {
            Some(Thumb::Failed { at, tries }) if *tries < THUMB_TRIES => Some(*at + THUMB_RETRY * *tries as u32),
            _ => None,
        }
    }

    /// How long from `now` until the clock alone gives the menu something to do, if anything waits: suggestions asked
    /// once typing paused, a shown thumbnail tried again, a light set here given up for what N4N0 says, play or pause
    /// asked here given up for what the app says. An interface that draws only when something changed asks for its
    /// next frame then ([`MenuHost::follow`], [`MenuHost::poll`] and [`MenuHost::follow_media`] do it there); what its
    /// threads fetch tells the [`Waker`] instead.
    pub fn next_wake(&self, now: Instant) -> Option<Duration> {
        let suggestions = self.suggest.next_due();
        let retry = self.thumbs.values().any(|t| matches!(t, Thumb::Failed { tries, .. } if *tries < THUMB_TRIES)).then(|| self.shown_pictures().iter().filter_map(|id| self.retry_at(id)).min()).flatten();
        let light = self.light_set.map(|(_, at)| at + LIGHT_ECHO);
        let e = &self.echoes;
        let echo = [e.fast_walk.due(), e.mismatch.due(), e.assistant.due(), e.photo.due(), e.tracking.due()].into_iter().flatten().min().filter(|due| *due > now);
        // Only while it is still to come: [`MenuHost::follow_media`] takes it once due, and a page that does not
        // follow the music would otherwise be asked for a frame at once, at every frame, for ever.
        let play = self.play_asked.map(|(_, at)| at + MEDIA_CATCHES_UP).filter(|due| *due > now);
        [suggestions, retry, light, play, echo].into_iter().flatten().min().map(|at| at.saturating_duration_since(now))
    }

    /// Fetches the thumbnails among `ids` not here yet (one that failed is tried again a little later, a few times).
    fn thumbnails(&mut self, ids: Vec<String>) {
        if self.thumbs.len() > 60 {
            self.thumbs.retain(|id, _| ids.contains(id));
        }
        let now = Instant::now();
        for id in ids {
            let tries = match self.thumbs.get(&id) {
                None => 0,
                Some(Thumb::Failed { at, tries }) if *tries < THUMB_TRIES && now >= *at + THUMB_RETRY * *tries as u32 => *tries,
                Some(_) => continue,
            };
            self.thumbs.insert(id.clone(), Thumb::Loading);
            let (tx, waker) = (self.done_tx.clone(), self.waker.clone());
            let captures = self.capture_pictures.clone();
            std::thread::spawn(move || {
                // A 3D capture's picture is read where the interface runs; the rest are YouTube's.
                let image = match id.strip_prefix(orbiters_n4n0_core::captures::PICTURE_PREFIX) {
                    Some(key) => captures.and_then(|read| read(key)),
                    None => youtube::thumbnail(&id).ok().and_then(|jpeg| picture::rgb_jpeg(&jpeg).ok()),
                };
                let _ = tx.send(Done::Thumbnail { id, tries, image });
                wake(&waker);
            });
        }
    }

    /// Does what the menu asked (`settings`: as the interface has them now); returns whether it asked to close (its
    /// close button, or idle), which the interface does.
    pub fn effects(&mut self, effects: Vec<Effect>, settings: &Settings, service: &dyn MenuService) -> bool {
        if effects.is_empty() {
            return false;
        }
        self.dirty = true;
        let mut close = false;
        for effect in effects {
            match effect {
                // Asked for once typing pauses, one request at a time ([`MenuHost::poll`] sends it).
                Effect::Suggest { id, text } => self.suggest.want(id, text, Instant::now()),
                Effect::Search { id, query } => {
                    let (tx, lang, waker) = (self.done_tx.clone(), self.locale.language(), self.waker.clone());
                    std::thread::spawn(move || {
                        let result = youtube::search(&query, &lang);
                        let _ = tx.send(Done::Results { id, more: false, result });
                        wake(&waker);
                    });
                }
                Effect::More { id, token, .. } => {
                    let (tx, lang, waker) = (self.done_tx.clone(), self.locale.language(), self.waker.clone());
                    std::thread::spawn(move || {
                        let result = youtube::more(&token, &lang);
                        let _ = tx.send(Done::Results { id, more: true, result });
                        wake(&waker);
                    });
                }
                Effect::Thumbnails(ids) => self.thumbnails(ids),
                Effect::Copy { link, .. } => service.command(Command::CopyText { text: link }),
                // No Stay here on the overlay's menu: what stays is the avatar's screen.
                Effect::StayHere(_) => {}
                // Counted from what was asked here and not yet said: a second quick press is not lost (MENU-11).
                Effect::FastWalk(on) => {
                    let base = self.asked_settings(settings);
                    let mut changed = base.clone();
                    changed.walk_boost.enabled = on;
                    self.echoes.fast_walk.press(on);
                    service.save_settings(&base, &changed);
                }
                // Through N4N0, which stops whatever the assistant was doing as it saves the setting.
                Effect::Assistant(on) => {
                    self.echoes.assistant.press(on);
                    service.command(Command::Assistant { on })
                }
                Effect::MismatchButtons(on) => {
                    let base = self.asked_settings(settings);
                    let mut changed = base.clone();
                    changed.mismatch.buttons = on;
                    self.echoes.mismatch.press(on);
                    service.save_settings(&base, &changed);
                }
                Effect::CallContact { id } => service.command(Command::Call { contact: id, stamp: None }),
                Effect::Answer => service.command(Command::AnswerCall { stamp: None }),
                Effect::HangUp => service.command(Command::EndCall),
                Effect::PhotoChoice { timer, length, mode } | Effect::Photo3d { timer, length, mode } => {
                    let take = matches!(effect, Effect::Photo3d { .. });
                    let base = self.asked_settings(settings);
                    let mut changed = base.clone();
                    changed.photo_3d_timer = timer;
                    changed.photo_3d = photo_setting(length);
                    changed.photo_3d_mode = mode_setting(mode);
                    self.echoes.photo.press((timer, length, mode));
                    service.save_settings(&base, &changed);
                    if take {
                        service.command(Command::Photo3DIn { seconds: timer });
                    }
                }
                Effect::Media(command) => self.media.command(command),
                Effect::Light(light) => {
                    self.light_set = Some((light, Instant::now()));
                    service.command(Command::Light { on: light.on, color: light.color, level: light.level });
                }
                Effect::SaveState { name, size, pose, default } => {
                    service.command(Command::States { action: StateAction::Save { name: Some(name), size, pose, default }, avatar: None })
                }
                Effect::ApplyState { id, .. } => service.command(Command::States { action: StateAction::Apply { id }, avatar: None }),
                Effect::TakeBack => service.command(Command::TakeBack),
                Effect::Drop { link, title, video } => {
                    service.command(Command::Drop { link, title, video, stamp: None });
                    self.dropped = true;
                }
                Effect::Tracking { on, ms } => {
                    self.echoes.tracking.press((on, ms));
                    service.command(Command::Tracking { on, ms })
                }
                Effect::Dictate => service.command(Command::Dictate),
                Effect::SendSuggestion { text, voice } => service.command(Command::Suggest { text, voice, stamp: None }),
                Effect::WatchVideo { link } => service.command(Command::Watch { link }),
                Effect::SaveVideo => service.command(Command::SaveVideo),
                // This computer's visits, read here (a small file): the service keeps it as VRChat's logs grow.
                Effect::Instances => self.menu.set_instances(orbiters_n4n0_core::invites::read_rows(&self.viewer), true, ""),
                Effect::InviteMe { location } => service.command(Command::InviteMe { location, stamp: None }),
                Effect::CopyText { text } => service.command(Command::CopyText { text }),
                Effect::OpenLink { link } => service.command(Command::Watch { link }),
                Effect::QrScan(on) => service.command(Command::Qr { on, holder: self.holder }),
                Effect::Captures => service.command(Command::Captures),
                Effect::Friends(on) => service.command(Command::Friends { on, holder: self.holder }),
                Effect::ShareCapture(share) => service.command(Command::ShareCapture { share, stamp: None }),
                Effect::Feedback(ask) => service.command(Command::Feedback { ask, stamp: None }),
                // The overlay never shows the camera's picture: its card is greyed out there.
                Effect::Mirror(_) => {}
                Effect::Close => close = true,
            }
        }
        close
    }

    /// `settings` with what was pressed here and not yet saved: what a change made now is counted from.
    fn asked_settings(&self, settings: &Settings) -> Settings {
        let mut base = settings.clone();
        let e = &self.echoes;
        if let Some(on) = e.fast_walk.asked() {
            base.walk_boost.enabled = *on;
        }
        if let Some(on) = e.mismatch.asked() {
            base.mismatch.buttons = *on;
        }
        if let Some((timer, length, mode)) = e.photo.asked() {
            base.photo_3d_timer = *timer;
            base.photo_3d = photo_setting(*length);
            base.photo_3d_mode = mode_setting(*mode);
        }
        base
    }

    /// The keys of every picture loaded (whatever copies other canvases keep of them).
    pub fn picture_keys(&self) -> Vec<String> {
        self.thumbs.iter().filter(|(_, t)| matches!(t, Thumb::Ready(_))).map(|(k, _)| k.clone()).collect()
    }

    /// A picture by its key, once loaded.
    pub fn picture(&self, key: &str) -> Option<&P::Picture> {
        match self.thumbs.get(key) {
            Some(Thumb::Ready(picture)) => Some(picture),
            _ => None,
        }
    }

    /// Keeps a picture made elsewhere (the screenshot review's stand-ins) under `key`, as a fetched one.
    pub fn put_picture(&mut self, key: &str, picture: P::Picture) {
        self.thumbs.insert(key.to_string(), Thumb::Ready(picture));
    }

    /// The interface's pictures.
    pub fn pictures(&self) -> &P {
        &self.pictures
    }
}

/// Tells the interface, when it asked to be told, that something fetched is there for [`MenuHost::poll`].
fn wake(waker: &Option<Waker>) {
    if let Some(waker) = waker {
        waker();
    }
}

/// How long an app gets to do what it was asked before what it says is believed over what was asked.
const MEDIA_CATCHES_UP: Duration = Duration::from_millis(2500);

/// Whether something plays, from what the app says now and what was last asked: what was asked, until the app agrees or has had its time.
fn settled_playing(read: bool, asked: &mut Option<(bool, Instant)>, now: Instant) -> bool {
    let Some((playing, at)) = *asked else { return read };
    if read == playing || now.duration_since(at) >= MEDIA_CATCHES_UP {
        *asked = None;
        return read;
    }
    playing
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Enzo: the state must switch on the press itself. A read of the old state does not flip it back; the app is believed once it agrees, or has had its time.
    #[test]
    fn play_and_pause_show_what_was_asked_until_the_app_has_caught_up() {
        let t = Instant::now();
        let mut asked = Some((false, t));
        assert!(!settled_playing(true, &mut asked, t + Duration::from_millis(900)));
        assert!(asked.is_some());
        assert!(!settled_playing(false, &mut asked, t + Duration::from_millis(1000)), "it agrees");
        assert!(asked.is_none());
        assert!(settled_playing(true, &mut asked, t + Duration::from_millis(1100)), "and from then on it is believed");
        let mut asked = Some((false, t));
        assert!(settled_playing(true, &mut asked, t + MEDIA_CATCHES_UP), "it had its time");
    }
    use crate::status::{CallContact, CallLink, CallPhase, CallView, CallsStatus, N4n0Light, StateView, StatesNote, StatesView};
    use std::sync::Mutex;

    /// Pictures as their size and name.
    struct Sizes;

    impl Pictures for Sizes {
        type Picture = (String, usize, usize);
        fn make(&self, name: &str, image: &Rgb) -> Self::Picture {
            (name.to_string(), image.width, image.height)
        }
    }

    struct English;

    impl Locale for English {
        fn locale(&self) -> String {
            "en-GB".into()
        }
    }

    /// What the menu asked of N4N0 and the settings.
    #[derive(Default)]
    struct Recorder {
        commands: Mutex<Vec<Command>>,
        saved: Mutex<Vec<Settings>>,
        /// What each save counted from.
        before: Mutex<Vec<Settings>>,
    }

    impl MenuService for Recorder {
        fn command(&self, command: Command) {
            self.commands.lock().unwrap().push(command);
        }

        fn save_settings(&self, before: &Settings, after: &Settings) {
            self.before.lock().unwrap().push(before.clone());
            self.saved.lock().unwrap().push(after.clone());
        }
    }

    fn host() -> MenuHost<Sizes> {
        MenuHost::new(Sizes, Arc::new(English), Arc::new(Watcher::new(|| Box::new(crate::media::NoMedia))))
    }

    /// verify2 (OVERLAY-16): the loaded pictures are listed from the host itself, so a page is given every drawable one.
    #[test]
    fn every_loaded_picture_is_listed() {
        let mut host = host();
        for n in 0..70 {
            host.done_tx.send(Done::Thumbnail { id: format!("v{n}"), tries: 0, image: Some(Rgb { width: 1, height: 1, pixels: vec![0; 3] }) }).unwrap();
        }
        host.poll();
        let keys = host.picture_keys();
        assert_eq!(keys.len(), 70);
        assert!(keys.contains(&"v0".to_string()), "the first, whatever copies were let go");
    }

    /// OVERLAY-26: a fetched thumbnail is made, and found, under the key the pages ask for.
    #[test]
    fn a_fetched_thumbnail_is_kept_under_the_key_it_is_asked_by() {
        let mut host = host();
        let image = Rgb { width: 2, height: 1, pixels: vec![0; 6] };
        host.done_tx.send(Done::Thumbnail { id: "dQw4w9WgXcQ".into(), tries: 0, image: Some(image) }).unwrap();
        host.poll();
        assert_eq!(host.picture("dQw4w9WgXcQ"), Some(&("dQw4w9WgXcQ".to_string(), 2, 1)), "made under the same key it is found by");
    }

    /// The window's Home drew itself two to ten times a second to look whether a thread had fetched something. A host
    /// with a waker tells the interface instead, and says when the clock alone brings it something to do.
    #[test]
    fn what_is_fetched_wakes_the_interface_and_the_clock_says_when_it_is_next_needed() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let woken = Arc::new(AtomicU32::new(0));
        let told = woken.clone();
        let mut h = host()
            .with_capture_pictures(Arc::new(|_key| Some(Rgb { width: 2, height: 1, pixels: vec![0; 6] })))
            .with_waker(Arc::new(move || {
                told.fetch_add(1, Ordering::SeqCst);
            }));
        let (settings, service) = (Settings::default(), Recorder::default());
        h.open(&settings);
        assert_eq!(h.next_wake(Instant::now()), None, "nothing waits");
        // A 3D capture's picture, read on a thread: the interface is told once it is there for `poll`.
        let key = format!("{}abc", orbiters_n4n0_core::captures::PICTURE_PREFIX);
        h.effects(vec![Effect::Thumbnails(vec![key.clone()])], &settings, &service);
        let until = Instant::now() + Duration::from_secs(10);
        while woken.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < until, "the interface was not told");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(h.picture(&key).is_none(), "not before the interface looks");
        h.poll();
        assert!(h.picture(&key).is_some());
        assert_eq!(h.next_wake(Instant::now()), None);
        // The light set here shows until N4N0 says the same, or its time is up: a frame is due then.
        h.effects(vec![Effect::Light(Light { on: true, color: 1, level: 0.5 })], &settings, &service);
        let wait = h.next_wake(Instant::now()).expect("the light's echo ends by the clock");
        assert!(wait <= LIGHT_ECHO && wait > LIGHT_ECHO / 2, "{wait:?}");
        // Typing: the suggestions are asked for once it paused, which only the clock tells (and comes first).
        h.effects(vec![Effect::Suggest { id: 1, text: "lofi".into() }], &settings, &service);
        let wait = h.next_wake(Instant::now()).expect("the suggestions wait for the pause");
        assert!(wait <= orbiters_n4n0_core::menu::SUGGEST_PAUSE, "{wait:?}");
    }

    /// Pause asked of an app that goes on playing: the menu shows the pause until the app has had its time, then
    /// what the app says. The window's Home is drawn only when what it shows of the music changed, which the song
    /// going on is not: without the clock's frame, and a reading taken again on it, Home went on showing the pause.
    #[test]
    fn play_or_pause_asked_is_given_up_by_the_clock_alone_when_the_app_never_did_it() {
        use orbiters_n4n0_core::menu::Media;
        let playing = |position| crate::media::Reading {
            media: Some(Media { title: "Song".into(), playing: true, can_play_pause: true, position: Some(position), duration: Some(300), ..Default::default() }),
            ..Default::default()
        };
        // Made-up readings: the device's music is never read.
        let watcher = Arc::new(Watcher::new(|| Box::new(crate::media::NoMedia)));
        watcher.stand_in(playing(10));
        let mut h = MenuHost::new(Sizes, Arc::new(English), watcher.clone());
        let (settings, service) = (Settings::default(), Recorder::default());
        h.open(&settings);
        h.follow_media();
        assert!(h.menu.media.as_ref().unwrap().playing);
        assert_eq!(h.next_wake(Instant::now()), None);
        // Pause, shown at once; the app goes on playing, and says so.
        let effects = h.press(Target::MediaPlayPause);
        h.effects(effects, &settings, &service);
        let asked = h.play_asked.expect("pause was asked").1;
        watcher.stand_in(playing(11));
        h.follow_media_at(asked + Duration::from_millis(100));
        assert!(!h.menu.media.as_ref().unwrap().playing, "what was asked shows while the app has its time");
        // The clock's frame is due when that time is up.
        let wait = h.next_wake(asked + Duration::from_millis(100)).expect("a frame when the app has had its time");
        assert_eq!(wait, MEDIA_CATCHES_UP - Duration::from_millis(100));
        // On it, with no reading newer than the one taken: the app is believed.
        h.dirty = false;
        let seen = h.media_seen;
        h.follow_media_at(asked + MEDIA_CATCHES_UP);
        assert_eq!(h.media_seen, seen, "no new reading");
        assert!(h.menu.media.as_ref().unwrap().playing && h.dirty, "the app went on playing: shown, and to be drawn");
        assert!(h.play_asked.is_none());
        assert_eq!(h.next_wake(asked + MEDIA_CATCHES_UP), None);
        // Asked again, then a page that does not follow the music: once the time is up, no frame is asked for it (it
        // would be asked at once, for ever); the music is put right when a page follows it again.
        let effects = h.press(Target::MediaPlayPause);
        h.effects(effects, &settings, &service);
        let asked = h.play_asked.expect("pause was asked").1;
        assert!(h.next_wake(asked).is_some());
        assert_eq!(h.next_wake(asked + MEDIA_CATCHES_UP), None);
        assert_eq!(h.next_wake(asked + MEDIA_CATCHES_UP * 4), None);
        watcher.stand_in(playing(12));
        h.follow_media_at(asked + MEDIA_CATCHES_UP * 4);
        assert!(h.menu.media.as_ref().unwrap().playing && h.play_asked.is_none());
    }

    /// Request15 10: until the service says this account has N4N0 calls (what it starts with), Call is "Coming soon".
    #[test]
    fn call_is_coming_soon_until_the_service_says_otherwise() {
        let mut h = host();
        let settings = Settings::default();
        h.open(&settings);
        let mut n4n0 = N4n0Status::default();
        h.follow(&n4n0, &settings);
        assert!(h.menu.calls_soon && orbiters_n4n0_core::menu::layout::target_state(&h.menu, Target::Call).soon);
        n4n0.calls = CallsStatus { link: CallLink::Linked { account: "usr_1".into() }, ..Default::default() }.into();
        h.follow(&n4n0, &settings);
        assert!(!h.menu.calls_soon && !orbiters_n4n0_core::menu::layout::target_state(&h.menu, Target::Call).unavailable);
    }

    #[test]
    fn the_menu_follows_n4n0_and_the_settings() {
        let mut h = host();
        let mut settings = Settings::default();
        h.open(&settings);
        assert!(h.menu.open && h.menu.pages == Pages::ALL);
        h.dirty = false;
        let mut n4n0 = N4n0Status { listening: true, panel: Some("Hello   there".into()), light: Some(N4n0Light { on: true, color: 2, level: 0.5 }), ..Default::default() };
        n4n0.calls = CallsStatus {
            link: CallLink::Linked { account: "usr_1".into() },
            contacts: vec![CallContact { id: "c1".into(), name: "Ada".into() }],
            call: Some(CallView { name: "Ada".into(), phase: CallPhase::Ringing, incoming: true, connected_at_ms: None, muted: false, mic_level: 0.0, remote_level: 0.0 }),
            ..Default::default()
        }
        .into();
        n4n0.states = StatesView {
            avatar_id: Some("avtr_a".into()),
            states: vec![StateView { id: "s1".into(), name: "Cosy".into(), default: true, ..Default::default() }],
            ..Default::default()
        };
        settings.walk_boost.enabled = true;
        h.follow(&n4n0, &settings);
        assert!(h.dirty);
        let m = &h.menu;
        assert!(m.listening && m.fast_walk && m.calls_ready);
        assert_eq!(m.heard.as_deref(), Some("Hello there"));
        assert_eq!(m.light, Some(Light { on: true, color: 2, level: 0.5 }));
        assert_eq!(m.contacts.len(), 1);
        assert_eq!(m.call, Some(orbiters_n4n0_core::menu::Call::Ringing { name: "Ada".into() }));
        // Nothing new: nothing to draw again.
        h.dirty = false;
        h.follow(&n4n0, &settings);
        assert!(!h.dirty);
        // An error about the states, on their page, replaces the press's own line.
        h.menu.page = Page::States;
        n4n0.states.note = Some(StatesNote { id: 7, text: "VRChat said no".into(), error: true });
        h.follow(&n4n0, &settings);
        h.follow(&n4n0, &settings);
        n4n0.states.note = Some(StatesNote { id: 8, text: "VRChat said no again".into(), error: true });
        h.follow(&n4n0, &settings);
        assert_eq!(h.menu.notice.as_deref(), Some("VRChat said no again"));
    }

    /// The overlay's Suggest features has the switch of the menu mismatch buttons: it shows the setting Settings › Beta feedback shows
    /// (changed there, it follows), and a press saves the other value in the same setting.
    #[test]
    fn suggest_s_switch_is_the_beta_feedback_setting() {
        use orbiters_n4n0_core::menu::layout::{self, ScreenPage};
        let mut h = host();
        let mut settings = Settings::default();
        h.open(&settings);
        let n4n0 = N4n0Status::default();
        h.follow(&n4n0, &settings);
        h.menu.page = Page::Suggest;
        assert_eq!(h.menu.mismatch_buttons, Some(false));
        let page = layout::cached(ScreenPage::Suggest);
        let switch = page.elements.iter().find(|e| e.target == Target::MismatchButtons).expect("on the page");
        assert!(layout::state(&h.menu, switch).pointable() && !layout::state(&h.menu, switch).on);
        // Turned on in Settings: on here.
        settings.mismatch.buttons = true;
        h.dirty = false;
        h.follow(&n4n0, &settings);
        assert!(h.dirty && layout::state(&h.menu, switch).on);
        // Pressed here: off at once, and saved off.
        let service = Recorder::default();
        let effects = h.press(Target::MismatchButtons);
        assert_eq!(effects, vec![Effect::MismatchButtons(false)]);
        assert!(!layout::state(&h.menu, switch).on);
        h.effects(effects, &settings, &service);
        let saved = service.saved.lock().unwrap();
        assert_eq!(saved.len(), 1);
        assert!(!saved[0].mismatch.buttons && saved[0].mismatch.picture == settings.mismatch.picture);
        // A menu that never had the setting (the avatar's screen): no switch, a press does nothing.
        let mut screen = orbiters_n4n0_core::menu::Menu::default();
        screen.page = Page::Suggest;
        assert!(layout::state(&screen, switch).hidden);
        assert!(screen.press(Target::MismatchButtons).is_empty());
    }

    /// Home's microphone switches N4N0's assistant (Enzo, 2026-11-16): it shows the setting Settings › N4N0 shows, and a press goes
    /// through N4N0, which stops what the assistant was doing as it saves it.
    #[test]
    fn home_s_microphone_is_the_assistant_setting() {
        let mut h = host();
        let mut settings = Settings::default();
        h.open(&settings);
        let n4n0 = N4n0Status::default();
        h.follow(&n4n0, &settings);
        assert_eq!(h.menu.assistant, Some(true));
        settings.n4n0.assistant = false;
        h.dirty = false;
        h.follow(&n4n0, &settings);
        assert!(h.dirty && h.menu.assistant == Some(false));
        let service = Recorder::default();
        let effects = h.press(Target::Assistant);
        assert_eq!(effects, vec![Effect::Assistant(true)]);
        h.effects(effects, &settings, &service);
        assert_eq!(*service.commands.lock().unwrap(), vec![Command::Assistant { on: true }]);
        assert!(service.saved.lock().unwrap().is_empty(), "N4N0 saves it");
        // N4N0 off: nothing to switch.
        settings.n4n0.enabled = false;
        h.follow(&n4n0, &settings);
        assert_eq!(h.menu.assistant, None);
    }

    #[test]
    fn the_card_out_and_a_stopped_picture_show_in_the_menu() {
        let mut h = host();
        let settings = Settings::default();
        h.open(&settings);
        let service = Recorder::default();
        // A note from before this menu was opened is not its own.
        let mut n4n0 = N4n0Status { photo_note: Some((3, "Open the camera before".into())), ..Default::default() };
        h.follow(&n4n0, &settings);
        let _ = h.press(Target::Photo);
        h.follow(&n4n0, &settings);
        assert_eq!(h.menu.notice, None);
        n4n0.photo_note = Some((4, "Open the camera before".into()));
        h.follow(&n4n0, &settings);
        assert_eq!(h.menu.notice.as_deref(), Some("Open the camera before"));
        // A card out: Take back asks N4N0 to take it back.
        n4n0.card = Some(crate::status::CardView { title: "t".into(), link: "https://x.y/".into(), left: 500 });
        h.follow(&n4n0, &settings);
        assert!(h.menu.card_out);
        let _ = h.press(Target::Back);
        let _ = h.press(Target::Music);
        let effects = h.press(Target::TakeBack);
        assert!(!h.effects(effects, &settings, &service));
        assert!(matches!(service.commands.lock().unwrap().as_slice(), [Command::TakeBack]));
    }

    #[test]
    fn a_light_set_here_stays_until_n4n0_says_it() {
        let mut h = host();
        let settings = Settings::default();
        h.open(&settings);
        let service = Recorder::default();
        let set = Light { on: true, color: 3, level: 1.0 };
        assert!(!h.effects(vec![Effect::Light(set)], &settings, &service));
        assert!(matches!(service.commands.lock().unwrap().as_slice(), [Command::Light { on: true, color: 3, .. }]));
        // N4N0 still says the light was off: the switch does not jump back.
        let n4n0 = N4n0Status { light: Some(N4n0Light { on: false, color: 0, level: 0.6 }), ..Default::default() };
        h.follow(&n4n0, &settings);
        assert_eq!(h.menu.light, Some(set));
        // Now it says the same: its word again.
        let n4n0 = N4n0Status { light: Some(N4n0Light { on: true, color: 3, level: 1.0 }), ..Default::default() };
        h.follow(&n4n0, &settings);
        assert!(h.light_set.is_none());
    }

    /// MENU-11: a switch pressed here stays as pressed while the settings still say the old value, and a second quick
    /// press counts from what the first asked (fast walk on, then off, both saved).
    #[test]
    fn a_switch_pressed_here_does_not_jump_back_and_a_second_press_is_not_lost() {
        let mut h = host();
        let settings = Settings::default();
        assert!(!settings.walk_boost.enabled);
        h.open(&settings);
        let n4n0 = N4n0Status::default();
        h.follow(&n4n0, &settings);
        let service = Recorder::default();
        let effects = h.press(Target::FastWalk);
        h.effects(effects, &settings, &service);
        h.follow(&n4n0, &settings);
        assert!(h.menu.fast_walk, "still on while the settings come back");
        // Pressed again before the settings said on: off is saved, counted from on.
        let effects = h.press(Target::FastWalk);
        assert_eq!(effects, vec![Effect::FastWalk(false)]);
        h.effects(effects, &settings, &service);
        let (before, saved) = (service.before.lock().unwrap().clone(), service.saved.lock().unwrap().clone());
        assert_eq!(saved.iter().map(|s| s.walk_boost.enabled).collect::<Vec<_>>(), [true, false]);
        assert_eq!(before.iter().map(|s| s.walk_boost.enabled).collect::<Vec<_>>(), [false, true], "the second change is not empty");
        h.follow(&n4n0, &settings);
        assert!(!h.menu.fast_walk);
        // The 3D picture's timer and the assistant the same.
        let mut on = settings.clone();
        on.n4n0.enabled = true;
        h.follow(&n4n0, &on);
        let assistant = h.menu.assistant;
        let effects = h.press(Target::Assistant);
        h.effects(effects, &on, &service);
        h.follow(&n4n0, &on);
        assert_eq!(h.menu.assistant, assistant.map(|a| !a));
        let effects = h.press(Target::Timer(2));
        h.effects(effects, &on, &service);
        h.follow(&n4n0, &on);
        assert_eq!(h.menu.photo_timer, orbiters_n4n0_core::menu::TIMERS[2]);
        // Once the echo is over, what the settings say again (the service refused it).
        h.echoes.fast_walk.press(true);
        h.echoes.fast_walk.set.as_mut().unwrap().1 -= SETTING_ECHO;
        h.follow(&n4n0, &settings);
        assert!(!h.menu.fast_walk);
    }

    #[test]
    fn effects_go_to_n4n0_and_the_settings() {
        let mut h = host();
        let settings = Settings::default();
        h.open(&settings);
        let service = Recorder::default();
        let close = h.effects(
            vec![
                Effect::Copy { link: "https://x.y/".into(), title: "t".into() },
                Effect::FastWalk(true),
                Effect::Photo3d { timer: 5, length: orbiters_n4n0_core::menu::PhotoLength::Full, mode: orbiters_n4n0_core::menu::PhotoMode::People },
                Effect::SaveState { name: "State 2".into(), size: true, pose: false, default: false },
                Effect::Drop { link: "https://x.y/".into(), title: "t".into(), video: None },
                Effect::Close,
            ],
            &settings,
            &service,
        );
        assert!(close && h.dropped);
        let commands = service.commands.lock().unwrap();
        assert!(matches!(&commands[..], [
            Command::CopyText { .. },
            Command::Photo3DIn { seconds: 5 },
            Command::States { action: StateAction::Save { name: Some(n), .. }, avatar: None },
            Command::Drop { .. },
        ] if n == "State 2"), "{commands:?}");
        let saved = service.saved.lock().unwrap();
        assert!(saved[0].walk_boost.enabled && saved[1].photo_3d_timer == 5);
        // A picture made elsewhere is kept under its key.
        h.put_picture("art:1", ("art".into(), 4, 2));
        assert_eq!(h.picture("art:1"), Some(&("art".to_string(), 4, 2)));
        assert!(h.picture("nope").is_none());
    }
}
