//! N4N0's menu (`orbiters_orb::menu`) run where an interface draws it: the desktop's SteamVR overlay and its window's
//! Home, a phone's screen. It keeps the menu in step with N4N0 (listening, fast walk, calls, the light, the 3D picture's
//! countdown, the avatar's saved states, whether a link can be dropped as a card), fetches YouTube's suggestions,
//! results and thumbnails itself, reads the music of its process's [`Watcher`], and asks N4N0 for the rest (calls, the
//! clipboard, a 3D picture, fast walk, the light, saving and applying states, dropping a card).
//!
//! The pictures are the interface's own ([`Pictures`]): egui textures on the desktop, a phone toolkit's images there.

use crate::media::Watcher;
use crate::platform::Locale;
use crate::screen::{keyboard, photo_length, photo_setting};
use crate::status::{OrbStatus, StateAction};
use crate::Command;
use orbiters_core::settings::Settings;
use orbiters_orb::menu::{Effect, Light, Menu, Page, Pages, StateRow, SuggestQueue, Target};
use orbiters_orb::picture::{self, Rgb};
use orbiters_orb::youtube;
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

/// What the menu asks of N4N0 and of the app's settings, from where it is drawn.
pub trait MenuService {
    /// Asks N4N0 for `command`.
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
    /// The light as last set here, and when: what N4N0 says replaces it once it says the same (or after
    /// [`LIGHT_ECHO`]), so a switch does not jump back while N4N0 is told.
    light_set: Option<(Light, Instant)>,
    /// Something the menu shows changed since it was last drawn.
    pub dirty: bool,
    /// The saved states as N4N0 last listed them (and whether the avatar is known), and the number of the last note of
    /// theirs seen.
    states_seen: Option<(Vec<StateRow>, bool)>,
    note_seen: Option<u64>,
    /// A link was dropped as a card since the host last looked (the overlay then closes: the card is in the world).
    pub dropped: bool,
    /// The number of the last suggestion result N4N0 published that this menu saw (the first status sets it).
    suggestion_seen: Option<u32>,
    /// The number of the last key of the computer's keyboard taken (the first status sets it).
    keys_seen: Option<u32>,
}

impl<P: Pictures> MenuHost<P> {
    /// A menu whose pictures `pictures` keeps, in the language and keyboard `locale` says, showing the music `media`
    /// reads.
    pub fn new(pictures: P, locale: Arc<dyn Locale>, media: Arc<Watcher>) -> Self {
        let (done_tx, done_rx) = mpsc::channel();
        Self {
            menu: Menu::default(),
            pictures,
            locale,
            media,
            thumbs: HashMap::new(),
            suggest: SuggestQueue::default(),
            done_tx,
            done_rx,
            media_seen: 0,
            light_set: None,
            dirty: true,
            states_seen: None,
            note_seen: None,
            dropped: false,
            suggestion_seen: None,
            keys_seen: None,
        }
    }

    /// Opens the menu on Home with the settings it starts from (keyboard, the 3D picture's timer and length). It has
    /// every tile: the interfaces draw every page.
    pub fn open(&mut self, settings: &Settings) {
        self.menu.layout = keyboard(settings.orb.menu.keyboard, &*self.locale);
        self.menu.photo_timer = settings.photo_3d_timer;
        self.menu.photo_length = photo_length(settings.photo_3d);
        self.menu.pages = Pages::ALL;
        self.menu.open();
        self.dirty = true;
    }

    /// A press on `target`: what it does follows from the effects it returns.
    pub fn press(&mut self, target: Target) -> Vec<Effect> {
        self.dirty = true;
        self.menu.press(target)
    }

    /// Keeps the menu's picture of what N4N0 and the settings say (calls, fast walk, listening, the picture's
    /// countdown), field by field: the menu (its results and suggestions included) is not copied every frame to find a
    /// change.
    pub fn follow(&mut self, orb: &OrbStatus, settings: &Settings) {
        let m = &mut self.menu;
        let mut changed = set(&mut m.listening, orb.listening);
        // What N4N0 says shows on Home too.
        if let Some(panel) = &orb.panel {
            if m.heard_private || !m.heard.as_deref().is_some_and(|h| h.split_whitespace().eq(panel.split_whitespace())) {
                m.heard = Some(panel.split_whitespace().collect::<Vec<_>>().join(" "));
                m.heard_private = false;
                changed = true;
            }
        }
        changed |= set(&mut m.fast_walk, settings.walk_boost.enabled);
        changed |= set(&mut m.desktop_mode, orb.desktop_mode);
        let said = orb.light.map(|l| Light { on: l.on, color: l.color, level: l.level });
        let light = match self.light_set {
            Some((set, at)) if said != Some(set) && at.elapsed() < LIGHT_ECHO => Some(set),
            _ => {
                self.light_set = None;
                said
            }
        };
        changed |= set(&mut m.light, light);
        changed |= set(&mut m.countdown, orb.photo_countdown);
        changed |= set(&mut m.can_drop, orb.can_drop);
        // The avatar's saved states (a state saved here shows until N4N0 lists it), and what N4N0 said of the last save
        // or apply, on the States page.
        let view = &orb.states;
        let rows: Vec<StateRow> =
            view.states.iter().map(|s| StateRow { id: s.id.clone(), name: s.name.clone(), default: s.default, pose: s.pose, size: s.size }).collect();
        let listed = (rows, view.avatar_id.is_some());
        if self.states_seen.as_ref() != Some(&listed) || m.saving() {
            m.set_states(listed.0.clone(), listed.1, "VRChat's avatar is not known yet");
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
        changed |= set(&mut m.photo_timer, settings.photo_3d_timer);
        changed |= set(&mut m.photo_length, photo_length(settings.photo_3d));
        // Tracking smoothing, the versions, and how the last suggestion sent from here went.
        let tracking = orb.tracking.as_ref().map(|t| orbiters_orb::menu::Tracking { on: t.on, levels: t.levels, note: t.note.clone() });
        changed |= set(&mut m.tracking, tracking);
        changed |= set(&mut m.tracking_note, orb.tracking_note.clone());
        changed |= set(&mut m.versions, orb.versions.clone());
        let video = orb.world_video.as_ref().map(|(url, title, at)| orbiters_orb::menu::WorldVideo { url: url.clone(), title: title.clone(), at: *at });
        changed |= set(&mut m.world_video, video);
        if !orb.listening && m.dictating {
            m.dictation_ended();
            changed = true;
        }
        match (&orb.suggestion, self.suggestion_seen) {
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
        let calls = &orb.calls;
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
    }

    /// The computer's keys N4N0 handed this menu since it last looked (none the first time: those were for an
    /// earlier menu); what they ask of the host follows from the effects returned.
    pub fn take_keys(&mut self, orb: &OrbStatus) -> Vec<Effect> {
        let newest = orb.keys.last().map_or(0, |(n, _)| *n);
        let Some(seen) = self.keys_seen.replace(newest) else { return Vec::new() };
        let mut effects = Vec::new();
        for (n, key) in &orb.keys {
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
        let media = self.media.clone();
        media.want();
        let (version, reading) = media.reading();
        if version == self.media_seen {
            return;
        }
        self.media_seen = version;
        self.menu.media = reading.media;
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
        let mut effects = Vec::new();
        while let Ok(done) = self.done_rx.try_recv() {
            match done {
                Done::Suggestions { id, text, result } => {
                    self.suggest.answered(id);
                    match result {
                        Ok(list) => self.menu.suggestions_arrived(id, &text, list),
                        Err(e) => eprintln!("orb menu: YouTube suggestions: {e:#}"),
                    }
                }
                Done::Results { id, more, result: Ok(results) } => effects.extend(self.menu.results_arrived(id, results, more)),
                Done::Results { id, result: Err(e), .. } => self.menu.results_failed(id, &e.to_string()),
                Done::Thumbnail { id, tries, image } => {
                    let thumb = match image {
                        Some(image) => Thumb::Ready(self.pictures.make(&format!("yt-{id}"), &image)),
                        None => Thumb::Failed { at: Instant::now(), tries: tries + 1 },
                    };
                    self.thumbs.insert(id, thumb);
                }
            }
            self.dirty = true;
        }
        if let Some((id, text)) = self.suggest.due(Instant::now()) {
            let (tx, lang) = (self.done_tx.clone(), self.locale.language());
            std::thread::spawn(move || {
                let result = youtube::suggestions(&text, &lang);
                let _ = tx.send(Done::Suggestions { id, text, result });
            });
        }
        if self.menu.page == Page::Results {
            let now = Instant::now();
            let retry: Vec<String> = self
                .menu
                .page_results()
                .iter()
                .filter(
                    |v| matches!(self.thumbs.get(&v.id), Some(Thumb::Failed { at, tries }) if *tries < THUMB_TRIES && now >= *at + THUMB_RETRY * *tries as u32),
                )
                .map(|v| v.id.clone())
                .collect();
            if !retry.is_empty() {
                self.thumbnails(retry);
            }
        }
        effects
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
            let tx = self.done_tx.clone();
            std::thread::spawn(move || {
                let image = youtube::thumbnail(&id).ok().and_then(|jpeg| picture::rgb_jpeg(&jpeg).ok());
                let _ = tx.send(Done::Thumbnail { id, tries, image });
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
                    let (tx, lang) = (self.done_tx.clone(), self.locale.language());
                    std::thread::spawn(move || {
                        let result = youtube::search(&query, &lang);
                        let _ = tx.send(Done::Results { id, more: false, result });
                    });
                }
                Effect::More { id, token, .. } => {
                    let (tx, lang) = (self.done_tx.clone(), self.locale.language());
                    std::thread::spawn(move || {
                        let result = youtube::more(&token, &lang);
                        let _ = tx.send(Done::Results { id, more: true, result });
                    });
                }
                Effect::Thumbnails(ids) => self.thumbnails(ids),
                Effect::Copy { link, .. } => service.command(Command::CopyText { text: link }),
                Effect::FastWalk(on) => {
                    let mut changed = settings.clone();
                    changed.walk_boost.enabled = on;
                    service.save_settings(settings, &changed);
                }
                Effect::CallContact { id } => service.command(Command::Call { contact: id }),
                Effect::Answer => service.command(Command::AnswerCall),
                Effect::HangUp => service.command(Command::EndCall),
                Effect::PhotoChoice { timer, length } | Effect::Photo3d { timer, length } => {
                    let take = matches!(effect, Effect::Photo3d { .. });
                    let mut changed = settings.clone();
                    changed.photo_3d_timer = timer;
                    changed.photo_3d = photo_setting(length);
                    service.save_settings(settings, &changed);
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
                    service.command(Command::States { action: StateAction::Save { name: Some(name), size, pose, default } })
                }
                Effect::ApplyState { id, .. } => service.command(Command::States { action: StateAction::Apply { id } }),
                Effect::Drop { link, title, video } => {
                    service.command(Command::Drop { link, title, video });
                    self.dropped = true;
                }
                Effect::Tracking { on, levels } => service.command(Command::Tracking { on, levels }),
                Effect::Dictate => service.command(Command::Dictate),
                Effect::SendSuggestion { text, voice } => service.command(Command::Suggest { text, voice }),
                Effect::WatchVideo { link } => service.command(Command::Watch { link }),
                Effect::SaveVideo => service.command(Command::SaveVideo),
                Effect::Close => close = true,
            }
        }
        close
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::{CallContact, CallLink, CallPhase, CallView, CallsStatus, OrbLight, StateView, StatesNote, StatesView};
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
    }

    impl MenuService for Recorder {
        fn command(&self, command: Command) {
            self.commands.lock().unwrap().push(command);
        }

        fn save_settings(&self, _before: &Settings, after: &Settings) {
            self.saved.lock().unwrap().push(after.clone());
        }
    }

    fn host() -> MenuHost<Sizes> {
        MenuHost::new(Sizes, Arc::new(English), Arc::new(Watcher::new(|| Box::new(crate::media::NoMedia))))
    }

    #[test]
    fn the_menu_follows_n4n0_and_the_settings() {
        let mut h = host();
        let mut settings = Settings::default();
        h.open(&settings);
        assert!(h.menu.open && h.menu.pages == Pages::ALL);
        h.dirty = false;
        let mut orb = OrbStatus { listening: true, panel: Some("Hello   there".into()), light: Some(OrbLight { on: true, color: 2, level: 0.5 }), ..Default::default() };
        orb.calls = CallsStatus {
            link: CallLink::Linked { account: "usr_1".into() },
            contacts: vec![CallContact { id: "c1".into(), name: "Ada".into() }],
            call: Some(CallView { name: "Ada".into(), phase: CallPhase::Ringing, incoming: true, connected_at_ms: None, muted: false, mic_level: 0.0, remote_level: 0.0 }),
            ..Default::default()
        };
        orb.states = StatesView {
            avatar_id: Some("avtr_a".into()),
            states: vec![StateView { id: "s1".into(), name: "Cosy".into(), default: true, ..Default::default() }],
            ..Default::default()
        };
        settings.walk_boost.enabled = true;
        h.follow(&orb, &settings);
        assert!(h.dirty);
        let m = &h.menu;
        assert!(m.listening && m.fast_walk && m.calls_ready);
        assert_eq!(m.heard.as_deref(), Some("Hello there"));
        assert_eq!(m.light, Some(Light { on: true, color: 2, level: 0.5 }));
        assert_eq!(m.contacts.len(), 1);
        assert_eq!(m.call, Some(orbiters_orb::menu::Call::Ringing { name: "Ada".into() }));
        // Nothing new: nothing to draw again.
        h.dirty = false;
        h.follow(&orb, &settings);
        assert!(!h.dirty);
        // An error about the states, on their page, replaces the press's own line.
        h.menu.page = Page::States;
        orb.states.note = Some(StatesNote { id: 7, text: "VRChat said no".into(), error: true });
        h.follow(&orb, &settings);
        h.follow(&orb, &settings);
        orb.states.note = Some(StatesNote { id: 8, text: "VRChat said no again".into(), error: true });
        h.follow(&orb, &settings);
        assert_eq!(h.menu.notice.as_deref(), Some("VRChat said no again"));
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
        let orb = OrbStatus { light: Some(OrbLight { on: false, color: 0, level: 0.6 }), ..Default::default() };
        h.follow(&orb, &settings);
        assert_eq!(h.menu.light, Some(set));
        // Now it says the same: its word again.
        let orb = OrbStatus { light: Some(OrbLight { on: true, color: 3, level: 1.0 }), ..Default::default() };
        h.follow(&orb, &settings);
        assert!(h.light_set.is_none());
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
                Effect::Photo3d { timer: 5, length: orbiters_orb::menu::PhotoLength::Full },
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
            Command::States { action: StateAction::Save { name: Some(n), .. } },
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
