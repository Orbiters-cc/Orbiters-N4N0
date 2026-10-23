//! Orb's menu (contracts `Docs/orb.md`, Menu): what opens when the person calls Orb (a controller button, the wake word
//! on its own, touching the orb): fast walk, YouTube, calls, a 3D picture, the computer's music and the orb's light.
//! Orbiters desktop's SteamVR overlay shows it, or the avatar's projected screen when the Orb prop has one; both draw
//! the same pages from [`layout`].
//!
//! This module holds what the menu shows and what each press does, without IO: presses and typing return [`Effect`]s
//! that the host carries out (fetching suggestions and results, copying a link, starting a call), and the host hands the
//! answers back. [`screen`] turns a menu into what the avatar's screen is sent; [`touch`] finds where a finger touches it.

pub mod layout;
pub mod retro;
pub mod screen;
pub mod touch;

use crate::youtube::{Results, Video};
use std::time::{Duration, Instant};

/// Results on a page of the list.
pub const RESULTS_PER_PAGE: usize = 5;
/// Suggestions under the search field.
pub const SUGGESTIONS: usize = 5;
/// Contacts on a page of the call list.
pub const CONTACTS_PER_PAGE: usize = 5;
/// Characters the search field holds (the screen's field shows them all).
pub const QUERY_MAX: usize = 40;
/// The 3D picture's timer choices, in seconds (0: at once).
pub const TIMERS: [u32; 4] = [0, 3, 5, 10];
/// How much a volume button turns the computer's volume, in percent.
pub const VOLUME_STEP: u8 = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Page {
    #[default]
    Home,
    /// The search field, its suggestions and the keyboard.
    Search,
    Results,
    /// Friends to call; a call under way shows instead.
    Call,
    Photo,
    /// What the computer plays (Windows' media controls) and its volume.
    Music,
    /// The orb as a light: on or off, its colour and brightness.
    Light,
}

/// The letters' order on the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum KeyboardLayout {
    #[default]
    Qwerty,
    Azerty,
}

/// The keyboard's keys: letters, or digits, symbols and accented letters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Keys {
    #[default]
    Letters,
    Symbols,
}

/// A 3D picture's camera flight (Orbiters desktop's `Photo3dLength`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PhotoLength {
    /// 1 s.
    Quick,
    /// 4 s.
    #[default]
    Standard,
    /// 8 s.
    Full,
}

impl PhotoLength {
    pub const ALL: [PhotoLength; 3] = [PhotoLength::Quick, PhotoLength::Standard, PhotoLength::Full];

    pub fn seconds(self) -> u32 {
        match self {
            PhotoLength::Quick => 1,
            PhotoLength::Standard => 4,
            PhotoLength::Full => 8,
        }
    }
}

/// What the computer plays, as Windows' media controls tell it (the app playing last, or playing now).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// The app playing it ("Spotify").
    pub app: String,
    pub playing: bool,
    pub can_play_pause: bool,
    pub can_previous: bool,
    pub can_next: bool,
    /// Where it is and how long it lasts, in seconds, when the app says.
    pub position: Option<u32>,
    pub duration: Option<u32>,
    /// The art's picture key (the hosts keep the pictures by key), once there is art.
    pub art: Option<String>,
}

/// The computer's volume (its default speakers), in percent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Volume {
    pub percent: u8,
    pub muted: bool,
}

/// What the music page (or Orb's assistant) asks of the computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Play,
    Pause,
    Previous,
    Next,
    /// The volume to this percent (unmuted).
    Volume(u8),
    Mute(bool),
}

/// The orb's light as the avatar holds it (`Orb/Light`, `Orb/Light/Color`, `Orb/Light/Level`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Light {
    pub on: bool,
    /// An index into contracts `orb::LIGHT_COLORS`.
    pub color: u8,
    /// 0 to 1.
    pub level: f32,
}

impl Light {
    /// The brightness choice (contracts `orb::LIGHT_LEVELS`) nearest the level.
    pub fn level_choice(&self) -> usize {
        let levels = orbiters_contracts::orb::LIGHT_LEVELS;
        (0..levels.len()).min_by(|&a, &b| (levels[a].1 - self.level).abs().total_cmp(&(levels[b].1 - self.level).abs())).unwrap_or(0)
    }
}

/// A friend who can be called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contact {
    pub id: String,
    pub name: String,
}

/// A call as the menu shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    /// A friend calls: answer or decline.
    Ringing { name: String },
    /// Our call rings at theirs, or connects.
    Calling { name: String },
    /// Talking, since `seconds` ago.
    Connected { name: String, seconds: u64, muted: bool },
}

/// What can be pressed. The layout of each page lists where.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Close,
    Back,
    // Home's tiles.
    FastWalk,
    YouTube,
    Call,
    Photo,
    Music,
    Light,
    // Search.
    Field,
    Search,
    Suggestion(u8),
    Key(char),
    Space,
    Backspace,
    /// Switches to digits and symbols.
    Symbols,
    /// Back to letters.
    Letters,
    // Results.
    Copy(u8),
    Previous,
    Next,
    // Calls.
    Contact(u8),
    Answer,
    HangUp,
    // 3D picture.
    Timer(u8),
    Length(u8),
    Take,
    // Music.
    MediaPrevious,
    MediaPlayPause,
    MediaNext,
    VolumeDown,
    VolumeUp,
    Mute,
    // Light.
    LightSwitch,
    LightColor(u8),
    LightLevel(u8),
}

/// What the host does for the menu. Requests are numbered, and their answers carry the number back: an answer to an
/// older request (the same words searched again, a slower suggestion) never replaces a newer one's.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Suggestions for this text (asked once per text; see [`SuggestQueue`] for how the hosts send them).
    Suggest {
        id: u64,
        text: String,
    },
    /// Results for `query`: search `id`, whose next pages carry the same number.
    Search {
        id: u64,
        query: String,
    },
    /// The next page of search `id`.
    More {
        id: u64,
        query: String,
        token: String,
    },
    /// Thumbnails the page shows, then the next page's (video ids); the host fetches those it does not have.
    Thumbnails(Vec<String>),
    /// Put a video's link in the clipboard.
    Copy {
        link: String,
        title: String,
    },
    FastWalk(bool),
    CallContact {
        id: String,
    },
    Answer,
    HangUp,
    /// Take a 3D picture after `timer` seconds.
    Photo3d {
        timer: u32,
        length: PhotoLength,
    },
    /// Keep the picked timer and length.
    PhotoChoice {
        timer: u32,
        length: PhotoLength,
    },
    /// Tell the computer's media controls or its volume.
    Media(MediaCommand),
    /// Set the orb's light so (its three parameters).
    Light(Light),
    /// The menu closed.
    Close,
}

/// The menu as both displays draw it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Menu {
    pub open: bool,
    pub page: Page,
    pub keys: Keys,
    pub layout: KeyboardLayout,
    /// What is in the search field.
    pub query: String,
    /// Suggestions for `suggested_for`, the best first.
    pub suggestions: Vec<String>,
    suggested_for: String,
    /// The text suggestions were last asked for (so each text is asked once), the last request's number and the
    /// number of the one shown.
    asked_for: String,
    suggest_id: u64,
    suggested_id: u64,
    /// Results of `results_for`, every page fetched so far.
    pub results: Vec<Video>,
    pub results_for: String,
    continuation: Option<String>,
    /// The search's number (its answers carry it).
    search_id: u64,
    /// The page of results shown (from 0).
    pub result_page: usize,
    pub searching: bool,
    loading_more: bool,
    /// Next was pressed on the last page fetched: the page turns once its results arrive (and stays when none do).
    turning: bool,
    /// Orb listens to the person.
    pub listening: bool,
    pub fast_walk: bool,
    /// VRChat runs in desktop mode: fast walk (a move of SteamVR's play space) is greyed out, "Only in VR".
    pub desktop_mode: bool,
    pub contacts: Vec<Contact>,
    pub contact_page: usize,
    pub call: Option<Call>,
    /// Calls are available (signed in, VRChat account linked); otherwise `call_note` says why.
    pub calls_ready: bool,
    pub call_note: String,
    pub photo_timer: u32,
    pub photo_length: PhotoLength,
    /// Seconds left before the 3D picture starts.
    pub countdown: Option<u32>,
    /// A short line for the person: what was copied, what failed. Shown until the next change of page.
    pub notice: Option<String>,
    /// What Orb heard last, or what it says (shown on Home).
    pub heard: Option<String>,
    /// `heard` holds the owner's own words: everyone else's screen does not show them (a sentence for Orb may have
    /// been kept out of the world, VRChat's microphone muted while it was said).
    pub heard_private: bool,
    /// Home has six tiles (music and light): the overlay always, the avatar's screen from version 3
    /// (`Orb/Screen/Version`); older screens have the first four.
    pub six_tiles: bool,
    /// What the computer plays (`None`: nothing, or not known yet) and its volume.
    pub media: Option<Media>,
    pub volume: Option<Volume>,
    /// The orb's light, when the avatar has it (the Light option on the orb's card).
    pub light: Option<Light>,
}

impl Menu {
    pub fn new(layout: KeyboardLayout, photo_timer: u32, photo_length: PhotoLength) -> Self {
        Self { layout, photo_timer, photo_length, ..Default::default() }
    }

    /// Opens on Home (or where it was, when it is open already).
    pub fn open(&mut self) {
        if !self.open {
            self.open = true;
            self.go(Page::Home);
        }
    }

    pub fn close(&mut self) -> Vec<Effect> {
        if !self.open {
            return Vec::new();
        }
        self.open = false;
        self.notice = None;
        vec![Effect::Close]
    }

    fn go(&mut self, page: Page) {
        self.page = page;
        self.notice = None;
        if page == Page::Search {
            self.keys = Keys::Letters;
        }
    }

    /// The person pressed `target`.
    pub fn press(&mut self, target: Target) -> Vec<Effect> {
        if !self.open {
            return Vec::new();
        }
        match target {
            Target::Close => self.close(),
            Target::Back => {
                let to = match self.page {
                    Page::Results => Page::Search,
                    _ => Page::Home,
                };
                self.go(to);
                Vec::new()
            }
            Target::FastWalk if self.desktop_mode => Vec::new(),
            Target::FastWalk => {
                self.fast_walk = !self.fast_walk;
                vec![Effect::FastWalk(self.fast_walk)]
            }
            Target::YouTube => {
                self.go(Page::Search);
                Vec::new()
            }
            Target::Call => {
                self.go(Page::Call);
                Vec::new()
            }
            Target::Photo => {
                self.go(Page::Photo);
                Vec::new()
            }
            Target::Music => {
                self.go(Page::Music);
                Vec::new()
            }
            Target::Light => {
                self.go(Page::Light);
                Vec::new()
            }
            Target::Field => Vec::new(),
            Target::Search => self.search(),
            Target::Suggestion(i) => {
                let picked = self.shown_suggestions().nth(i as usize).map(str::to_string);
                match picked {
                    Some(text) => {
                        self.query = cut(&text, QUERY_MAX);
                        self.search()
                    }
                    None => Vec::new(),
                }
            }
            Target::Key(c) => self.type_text(&c.to_string()),
            Target::Space => self.type_text(" "),
            Target::Backspace => self.backspace(),
            Target::Symbols => {
                self.keys = Keys::Symbols;
                Vec::new()
            }
            Target::Letters => {
                self.keys = Keys::Letters;
                Vec::new()
            }
            Target::Copy(i) => match self.page_results().get(i as usize) {
                Some(video) => {
                    let effect = Effect::Copy { link: video.link(), title: video.title.clone() };
                    self.notice = Some("Link copied: paste it in the video player".into());
                    vec![effect]
                }
                None => Vec::new(),
            },
            Target::Previous => self.turn(-1),
            Target::Next => self.turn(1),
            Target::Contact(i) => match self.page_contacts().get(i as usize) {
                Some(contact) if self.call.is_none() => vec![Effect::CallContact { id: contact.id.clone() }],
                _ => Vec::new(),
            },
            Target::Answer => match self.call {
                Some(Call::Ringing { .. }) => vec![Effect::Answer],
                _ => Vec::new(),
            },
            Target::HangUp => match self.call {
                Some(_) => vec![Effect::HangUp],
                None => Vec::new(),
            },
            Target::Timer(i) => match TIMERS.get(i as usize) {
                Some(&timer) => {
                    self.photo_timer = timer;
                    vec![Effect::PhotoChoice { timer, length: self.photo_length }]
                }
                None => Vec::new(),
            },
            Target::Length(i) => match PhotoLength::ALL.get(i as usize) {
                Some(&length) => {
                    self.photo_length = length;
                    vec![Effect::PhotoChoice { timer: self.photo_timer, length }]
                }
                None => Vec::new(),
            },
            Target::Take => {
                if self.countdown.is_some() {
                    return Vec::new();
                }
                vec![Effect::Photo3d { timer: self.photo_timer, length: self.photo_length }]
            }
            // What the music page shows changes at once; the computer follows.
            Target::MediaPlayPause => match self.media.as_mut() {
                Some(media) if media.can_play_pause => {
                    media.playing = !media.playing;
                    vec![Effect::Media(MediaCommand::PlayPause)]
                }
                _ => Vec::new(),
            },
            Target::MediaPrevious => match &self.media {
                Some(media) if media.can_previous => vec![Effect::Media(MediaCommand::Previous)],
                _ => Vec::new(),
            },
            Target::MediaNext => match &self.media {
                Some(media) if media.can_next => vec![Effect::Media(MediaCommand::Next)],
                _ => Vec::new(),
            },
            Target::VolumeDown | Target::VolumeUp => match self.volume.as_mut() {
                Some(volume) => {
                    let percent = if target == Target::VolumeUp {
                        volume.percent.saturating_add(VOLUME_STEP).min(100)
                    } else {
                        volume.percent.saturating_sub(VOLUME_STEP)
                    };
                    *volume = Volume { percent, muted: false };
                    vec![Effect::Media(MediaCommand::Volume(percent))]
                }
                None => Vec::new(),
            },
            Target::Mute => match self.volume.as_mut() {
                Some(volume) => {
                    volume.muted = !volume.muted;
                    vec![Effect::Media(MediaCommand::Mute(volume.muted))]
                }
                None => Vec::new(),
            },
            // Choosing a colour or a brightness turns the light on.
            Target::LightSwitch | Target::LightColor(_) | Target::LightLevel(_) => match self.light.as_mut() {
                Some(light) => {
                    match target {
                        Target::LightColor(i) if (i as usize) < orbiters_contracts::orb::LIGHT_COLORS.len() => {
                            light.color = i;
                            light.on = true;
                        }
                        Target::LightLevel(i) => match orbiters_contracts::orb::LIGHT_LEVELS.get(i as usize) {
                            Some(&(_, level)) => {
                                light.level = level;
                                light.on = true;
                            }
                            None => return Vec::new(),
                        },
                        Target::LightSwitch => light.on = !light.on,
                        _ => return Vec::new(),
                    }
                    vec![Effect::Light(*light)]
                }
                None => Vec::new(),
            },
        }
    }

    /// Text typed into the search field (a key, or several characters at once).
    pub fn type_text(&mut self, text: &str) -> Vec<Effect> {
        if self.page != Page::Search {
            return Vec::new();
        }
        for c in text.chars() {
            if self.query.chars().count() >= QUERY_MAX {
                break;
            }
            // No leading space and no double space: the field shows what will be searched.
            if c == ' ' && (self.query.is_empty() || self.query.ends_with(' ')) {
                continue;
            }
            if !c.is_control() {
                self.query.push(c);
            }
        }
        self.suggest()
    }

    pub fn backspace(&mut self) -> Vec<Effect> {
        if self.page != Page::Search {
            return Vec::new();
        }
        self.query.pop();
        self.suggest()
    }

    fn suggest(&mut self) -> Vec<Effect> {
        let text = self.query.trim_start().to_string();
        if text.trim().is_empty() {
            self.suggestions.clear();
            self.suggested_for.clear();
            self.asked_for.clear();
            return Vec::new();
        }
        if text == self.asked_for {
            return Vec::new();
        }
        self.asked_for = text.clone();
        self.suggest_id += 1;
        vec![Effect::Suggest { id: self.suggest_id, text }]
    }

    /// The suggestions to show: those for what is typed, or for an earlier text while the new ones are on their way
    /// (only while they still start like it, so they never contradict the field). Read in place: the screen's steps ask
    /// for them many times a second.
    pub fn shown_suggestions(&self) -> impl Iterator<Item = &str> + '_ {
        let typed = self.query.trim();
        let fresh = same_ignoring_case(self.suggested_for.trim(), typed);
        self.suggestions
            .iter()
            .map(String::as_str)
            .filter(move |s| !typed.is_empty() && (fresh || starts_ignoring_case(s, typed)))
            .filter(move |s| !fresh || !same_ignoring_case(s.trim(), typed))
            .take(SUGGESTIONS)
    }

    /// Suggestions arrived for request `id`, asked for `text`.
    pub fn suggestions_arrived(&mut self, id: u64, text: &str, suggestions: Vec<String>) {
        // An older request's answer never replaces a newer one's, and an answer to a text the field no longer starts
        // with is stale.
        if id <= self.suggested_id || !starts_ignoring_case(self.query.trim_start(), text.trim()) {
            return;
        }
        self.suggested_id = id;
        self.suggested_for = text.to_string();
        self.suggestions = suggestions;
    }

    fn search(&mut self) -> Vec<Effect> {
        let query = self.query.trim().to_string();
        if query.is_empty() {
            return Vec::new();
        }
        self.searching = true;
        self.loading_more = false;
        self.turning = false;
        self.results.clear();
        self.continuation = None;
        self.results_for = query.clone();
        self.result_page = 0;
        self.search_id += 1;
        self.go(Page::Results);
        vec![Effect::Search { id: self.search_id, query }]
    }

    /// Search for `query` straight away (said aloud, or asked of Orb).
    pub fn search_for(&mut self, query: &str) -> Vec<Effect> {
        self.open = true;
        self.query = cut(query.trim(), QUERY_MAX);
        self.search()
    }

    /// Results arrived for search `id`: its first page, or the next one (`more`).
    pub fn results_arrived(&mut self, id: u64, results: Results, more: bool) -> Vec<Effect> {
        // Another search's (an older one for the same words included) is dropped.
        if id != self.search_id {
            return Vec::new();
        }
        self.searching = false;
        self.loading_more = false;
        if !more {
            self.results.clear();
            self.result_page = 0;
        }
        for video in results.videos {
            if !self.results.iter().any(|v| v.id == video.id) {
                self.results.push(video);
            }
        }
        self.continuation = results.continuation;
        // The page asked for turns now that it has results; when none came, it stays where it was.
        if std::mem::take(&mut self.turning) && (self.result_page + 1) * RESULTS_PER_PAGE < self.results.len() {
            self.result_page += 1;
        }
        if self.results.is_empty() {
            self.notice = Some(format!("Nothing found for \"{}\"", self.results_for));
        }
        self.thumbnails()
    }

    /// Search `id` or its next page failed: the page stays where it was (Next tries again).
    pub fn results_failed(&mut self, id: u64, error: &str) {
        if id != self.search_id {
            return;
        }
        self.searching = false;
        self.loading_more = false;
        self.turning = false;
        self.notice = Some(format!("YouTube did not answer: {}", cut(error, 60)));
    }

    /// The results of the page shown.
    pub fn page_results(&self) -> &[Video] {
        let start = (self.result_page * RESULTS_PER_PAGE).min(self.results.len());
        &self.results[start..(start + RESULTS_PER_PAGE).min(self.results.len())]
    }

    /// Pages of results, counting one more while YouTube has more.
    pub fn result_pages(&self) -> usize {
        self.results.len().div_ceil(RESULTS_PER_PAGE).max(1)
    }

    /// YouTube has results past those fetched.
    pub fn more_available(&self) -> bool {
        self.continuation.is_some()
    }

    pub fn has_previous(&self) -> bool {
        match self.page {
            Page::Results => self.result_page > 0,
            Page::Call => self.contact_page > 0,
            _ => false,
        }
    }

    pub fn has_next(&self) -> bool {
        match self.page {
            Page::Results => (self.result_page + 1) * RESULTS_PER_PAGE < self.results.len() || self.continuation.is_some(),
            Page::Call => (self.contact_page + 1) * CONTACTS_PER_PAGE < self.contacts.len(),
            _ => false,
        }
    }

    fn turn(&mut self, by: i32) -> Vec<Effect> {
        match self.page {
            Page::Results => {
                if by < 0 && self.result_page > 0 {
                    self.result_page -= 1;
                    self.notice = None;
                    return self.thumbnails();
                }
                if by > 0 && self.has_next() {
                    self.notice = None;
                    if (self.result_page + 1) * RESULTS_PER_PAGE < self.results.len() {
                        self.result_page += 1;
                        let mut effects = self.thumbnails();
                        // Keep a page ahead: fetch more when the next page would not be full.
                        if (self.result_page + 2) * RESULTS_PER_PAGE > self.results.len() {
                            effects.extend(self.fetch_more());
                        }
                        return effects;
                    }
                    // The next page is not fetched yet. A full page turns once its results arrive; one still filling
                    // up waits for them, so no result is skipped.
                    let more = self.fetch_more();
                    if self.loading_more && self.page_results().len() == RESULTS_PER_PAGE {
                        self.turning = true;
                    }
                    return more;
                }
                Vec::new()
            }
            Page::Call => {
                let pages = self.contacts.len().div_ceil(CONTACTS_PER_PAGE).max(1);
                let next = (self.contact_page as i32 + by).clamp(0, pages as i32 - 1) as usize;
                self.contact_page = next;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn fetch_more(&mut self) -> Vec<Effect> {
        if self.loading_more {
            return Vec::new();
        }
        match self.continuation.clone() {
            Some(token) => {
                self.loading_more = true;
                vec![Effect::More { id: self.search_id, query: self.results_for.clone(), token }]
            }
            None => Vec::new(),
        }
    }

    /// Thumbnails of the results page shown, then of the next page (fetched ahead, so they show as it turns).
    pub fn thumbnails(&self) -> Vec<Effect> {
        let start = (self.result_page * RESULTS_PER_PAGE).min(self.results.len());
        let ids: Vec<String> = self.results[start..].iter().take(2 * RESULTS_PER_PAGE).map(|v| v.id.clone()).collect();
        if ids.is_empty() {
            Vec::new()
        } else {
            vec![Effect::Thumbnails(ids)]
        }
    }

    /// Whether a search, or the page asked for, is being fetched (shown as loading).
    pub fn loading(&self) -> bool {
        self.searching || self.turning || (self.loading_more && self.page_results().is_empty())
    }

    pub fn page_contacts(&self) -> &[Contact] {
        let start = (self.contact_page * CONTACTS_PER_PAGE).min(self.contacts.len());
        &self.contacts[start..(start + CONTACTS_PER_PAGE).min(self.contacts.len())]
    }

    /// The friends who can be called changed.
    pub fn set_contacts(&mut self, contacts: Vec<Contact>, ready: bool, note: &str) {
        self.contacts = contacts;
        self.calls_ready = ready;
        self.call_note = note.to_string();
        let pages = self.contacts.len().div_ceil(CONTACTS_PER_PAGE).max(1);
        self.contact_page = self.contact_page.min(pages - 1);
    }

    /// A call started, changed or ended. A ringing call opens the call page.
    pub fn set_call(&mut self, call: Option<Call>) {
        if matches!(call, Some(Call::Ringing { .. })) && self.open && !matches!(self.call, Some(Call::Ringing { .. })) {
            self.go(Page::Call);
        }
        self.call = call;
    }

    /// The sentence Orb heard while the menu is open: on the search page it is what to search for, and the menu takes
    /// it (`Some`); elsewhere it is for Orb's assistant (`None`).
    pub fn heard(&mut self, text: &str) -> Option<Vec<Effect>> {
        let text = text.trim().trim_end_matches(['.', '!', '?']).trim();
        if !self.open || text.is_empty() {
            return None;
        }
        if self.page == Page::Search {
            self.query = cut(text, QUERY_MAX);
            return Some(self.search());
        }
        self.heard = Some(cut(text, 60));
        self.heard_private = true;
        None
    }
}

/// `text`'s first `max` characters.
pub fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn lower(text: &str) -> impl Iterator<Item = char> + '_ {
    text.chars().flat_map(char::to_lowercase)
}

/// `text` starts with `prefix`, whatever their case (no copy made).
fn starts_ignoring_case(text: &str, prefix: &str) -> bool {
    let mut text = lower(text);
    lower(prefix).all(|c| text.next() == Some(c))
}

fn same_ignoring_case(a: &str, b: &str) -> bool {
    lower(a).eq(lower(b))
}

/// `text` in lines of at most `cols` characters, cut between words (a longer word is cut): what Orb says, a line at a
/// time on the screen's Home.
pub fn pieces(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let word = cut(word, cols);
        let width = line.chars().count();
        if width > 0 && width + 1 + word.chars().count() > cols {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// How long a line of `text` stays up: time to read its words.
pub fn reading_time(text: &str) -> Duration {
    Duration::from_millis((800 + 300 * text.split_whitespace().count() as u64).max(2500))
}

/// The pause after a keystroke before suggestions are asked for: typing a word asks once, not once a letter.
pub const SUGGEST_PAUSE: Duration = Duration::from_millis(130);
/// A request that has not answered by then is taken as lost (its own time limit is shorter).
const SUGGEST_LOST: Duration = Duration::from_secs(5);

/// How the hosts send the menu's [`Effect::Suggest`]s: the latest text only, once typing pauses for [`SUGGEST_PAUSE`],
/// one request at a time (the next waits for the answer, then asks for what the field holds by then).
#[derive(Debug, Default)]
pub struct SuggestQueue {
    wanted: Option<(u64, String, Instant)>,
    in_flight: Option<(u64, Instant)>,
}

impl SuggestQueue {
    /// The menu asked for suggestions for `text` (request `id`) at `now`: it replaces any not sent yet.
    pub fn want(&mut self, id: u64, text: String, now: Instant) {
        self.wanted = Some((id, text, now));
    }

    /// The request to send now, if one is due.
    pub fn due(&mut self, now: Instant) -> Option<(u64, String)> {
        if self.in_flight.is_some_and(|(_, since)| now.duration_since(since) < SUGGEST_LOST) {
            return None;
        }
        if self.wanted.as_ref().is_none_or(|(_, _, at)| now.duration_since(*at) < SUGGEST_PAUSE) {
            return None;
        }
        let (id, text, _) = self.wanted.take()?;
        self.in_flight = Some((id, now));
        Some((id, text))
    }

    /// Request `id` answered (or failed).
    pub fn answered(&mut self, id: u64) {
        if self.in_flight.is_some_and(|(i, _)| i == id) {
            self.in_flight = None;
        }
    }

    /// When a request may next be due (for a host that sleeps until then).
    pub fn next_due(&self) -> Option<Instant> {
        let (_, _, at) = self.wanted.as_ref()?;
        let after_pause = *at + SUGGEST_PAUSE;
        Some(match self.in_flight {
            Some((_, since)) => after_pause.max(since + SUGGEST_LOST),
            None => after_pause,
        })
    }
}

/// A duration as a call shows it: `0:42`, `12:05`, `1:02:03`.
pub fn clock(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn music_and_light_presses_show_at_once_and_ask_the_hosts() {
        let mut menu = Menu { six_tiles: true, ..Default::default() };
        menu.open();
        menu.press(Target::Music);
        assert!(menu.press(Target::MediaPlayPause).is_empty(), "nothing to play or pause");
        menu.media = Some(Media { title: "A song".into(), playing: true, can_play_pause: true, ..Default::default() });
        menu.volume = Some(Volume { percent: 98, muted: true });
        assert_eq!(menu.press(Target::MediaPlayPause), vec![Effect::Media(MediaCommand::PlayPause)]);
        assert!(!menu.media.as_ref().unwrap().playing, "paused at once");
        assert!(menu.press(Target::MediaNext).is_empty(), "the app can't go to the next one");
        assert_eq!(menu.press(Target::VolumeUp), vec![Effect::Media(MediaCommand::Volume(100))]);
        assert_eq!(menu.volume, Some(Volume { percent: 100, muted: false }), "turning it up unmutes");
        assert_eq!(menu.press(Target::Mute), vec![Effect::Media(MediaCommand::Mute(true))]);
        menu.press(Target::Back);
        assert_eq!(menu.page, Page::Home);

        menu.press(Target::Light);
        assert!(menu.press(Target::LightSwitch).is_empty(), "no light on this avatar");
        menu.light = Some(Light { on: false, color: 0, level: 0.6 });
        let effects = menu.press(Target::LightColor(3));
        assert_eq!(effects, vec![Effect::Light(Light { on: true, color: 3, level: 0.6 })], "a colour turns it on");
        assert_eq!(menu.press(Target::LightLevel(2)), vec![Effect::Light(Light { on: true, color: 3, level: 1.0 })]);
        assert_eq!(menu.press(Target::LightSwitch), vec![Effect::Light(Light { on: false, color: 3, level: 1.0 })]);
        assert!(menu.press(Target::LightColor(8)).is_empty(), "no ninth colour");
    }

    fn video(id: &str) -> Video {
        Video { id: format!("{id:0>11}"), title: format!("Title {id}"), channel: "Channel".into(), duration: Some("3:21".into()), live: false }
    }

    fn results(ids: std::ops::Range<usize>, more: bool) -> Results {
        Results { videos: ids.map(|i| video(&i.to_string())).collect(), continuation: more.then(|| "token".to_string()) }
    }

    fn suggest(id: u64, text: &str) -> Effect {
        Effect::Suggest { id, text: text.into() }
    }

    #[test]
    fn types_suggests_and_searches() {
        let mut menu = Menu::default();
        assert!(menu.press(Target::YouTube).is_empty(), "closed: presses do nothing");
        menu.open();
        menu.press(Target::YouTube);
        assert_eq!(menu.page, Page::Search);
        assert!(menu.type_text(" ").is_empty(), "no leading space");
        assert_eq!(menu.press(Target::Key('l')), vec![suggest(1, "l")]);
        assert_eq!(menu.type_text("ofi"), vec![suggest(2, "lofi")]);
        assert_eq!(menu.type_text(" "), vec![suggest(3, "lofi ")]);
        assert!(menu.type_text(" ").is_empty(), "no double space");
        menu.suggestions_arrived(2, "lofi", vec!["lofi hip hop".into(), "lofi girl".into(), "LOFI".into()]);
        assert_eq!(
            menu.shown_suggestions().collect::<Vec<_>>(),
            vec!["lofi hip hop", "lofi girl"],
            "suggestions start like the field; the field itself is not repeated"
        );
        menu.suggestions_arrived(1, "l", vec!["lol".into()]);
        assert_eq!(menu.suggestions.len(), 3, "an older request's answer never replaces a newer one's");
        menu.suggestions_arrived(4, "lol", vec!["lol".into()]);
        assert_eq!(menu.suggestions.len(), 3, "an answer for another text is dropped");
        menu.backspace();
        assert_eq!(menu.query, "lofi");
        let effects = menu.press(Target::Suggestion(1));
        assert_eq!(effects, vec![Effect::Search { id: 1, query: "lofi girl".into() }]);
        assert_eq!((menu.page, menu.searching, menu.query.as_str()), (Page::Results, true, "lofi girl"));
        for _ in 0..60 {
            menu.type_text("x");
        }
        assert_eq!(menu.query, "lofi girl", "typing goes nowhere outside the search page");
    }

    #[test]
    fn suggestions_wait_for_a_pause_one_request_at_a_time() {
        let mut queue = SuggestQueue::default();
        let t0 = Instant::now();
        assert_eq!(queue.due(t0), None);
        queue.want(1, "l".into(), t0);
        queue.want(2, "lo".into(), t0 + Duration::from_millis(60));
        assert_eq!(queue.due(t0 + Duration::from_millis(100)), None, "still typing");
        assert_eq!(queue.next_due(), Some(t0 + Duration::from_millis(60) + SUGGEST_PAUSE));
        let sent = t0 + Duration::from_millis(200);
        assert_eq!(queue.due(sent), Some((2, "lo".into())), "the latest text only");
        queue.want(3, "lof".into(), sent);
        assert_eq!(queue.due(sent + Duration::from_millis(300)), None, "one request at a time");
        queue.answered(2);
        assert_eq!(queue.due(sent + Duration::from_millis(300)), Some((3, "lof".into())));
        assert_eq!(queue.due(sent + Duration::from_secs(1)), None, "nothing more wanted");
    }

    #[test]
    fn pages_through_results_and_fetches_ahead() {
        let mut menu = Menu::default();
        menu.open();
        let effects = menu.search_for("cats");
        assert_eq!(effects, vec![Effect::Search { id: 1, query: "cats".into() }]);
        assert!(menu.results_arrived(2, results(0..20, true), false).is_empty(), "results of another search are dropped");
        let effects = menu.results_arrived(1, results(0..12, true), false);
        assert_eq!(menu.page_results().len(), 5);
        assert!(matches!(&effects[..], [Effect::Thumbnails(ids)] if ids.len() == 10), "this page's and the next's: {effects:?}");
        assert!(!menu.has_previous() && menu.has_next());
        menu.press(Target::Next);
        assert_eq!(menu.result_page, 1);
        menu.press(Target::Next);
        // Page 3 has two results; there is no page after them yet, so more are fetched.
        assert_eq!(menu.result_page, 2);
        assert_eq!(menu.page_results().len(), 2);
        let effects = menu.press(Target::Next);
        assert!(effects.is_empty(), "already fetching");
        assert_eq!(menu.result_page, 2, "a page still filling up waits for its results");
        let effects = menu.results_arrived(1, results(12..20, false), true);
        assert!(!effects.is_empty());
        assert_eq!(menu.page_results().len(), 5);
        assert_eq!(menu.result_pages(), 4);
        let copy = menu.press(Target::Copy(0));
        assert_eq!(copy, vec![Effect::Copy { link: "https://www.youtube.com/watch?v=00000000010".into(), title: "Title 10".into() }]);
        assert!(menu.notice.is_some());
        menu.press(Target::Back);
        assert_eq!(menu.page, Page::Search, "back from results is the search");
        assert_eq!(menu.press(Target::Close), vec![Effect::Close]);
        assert!(!menu.open);
    }

    #[test]
    fn a_failed_or_empty_next_page_leaves_the_page_where_it_was() {
        let mut menu = Menu::default();
        menu.open();
        menu.search_for("cats");
        menu.results_arrived(1, results(0..10, true), false);
        menu.press(Target::Next);
        assert_eq!(menu.result_page, 1);
        // The last full page: Next fetches, the page turns once results come.
        let effects = menu.press(Target::Next);
        assert!(effects.is_empty() || matches!(&effects[..], [Effect::More { id: 1, .. }]), "{effects:?}");
        assert!(menu.loading());
        menu.results_failed(1, "timed out");
        assert_eq!((menu.result_page, menu.loading()), (1, false), "no page past the end");
        assert_eq!(layout::text(&menu, layout::Content::PageLabel), "2/2+");
        // Next again: this time results come, and the next page shows them from its first.
        assert!(matches!(&menu.press(Target::Next)[..], [Effect::More { id: 1, .. }]));
        menu.results_arrived(1, results(10..20, true), true);
        assert_eq!(menu.result_page, 2);
        assert_eq!(menu.page_results()[0].id, video("10").id, "nothing skipped");
        // An empty next page: the page stays, and there is no "+" any more.
        menu.press(Target::Next);
        menu.press(Target::Next);
        menu.results_arrived(1, Results::default(), true);
        assert_eq!(menu.result_page, 3);
        assert_eq!(layout::text(&menu, layout::Content::PageLabel), "4/4");
        // An older search's next page, for the same words, lands nowhere.
        menu.search_for("cats");
        assert!(menu.results_arrived(1, results(40..45, true), true).is_empty());
        assert!(menu.results.is_empty() && menu.searching);
    }

    #[test]
    fn cuts_what_orb_says_into_lines() {
        assert_eq!(pieces("Voilà ton sweat, va voir tes potes maintenant", 16), vec!["Voilà ton sweat,", "va voir tes", "potes maintenant"]);
        assert_eq!(pieces("abcdefghijklmnopqrstuvwxyz", 10), vec!["abcdefghij"]);
        assert!(pieces("  ", 10).is_empty());
        assert!(reading_time("one two") >= Duration::from_millis(2500));
    }

    #[test]
    fn walks_calls_and_takes_pictures() {
        let mut menu = Menu::new(KeyboardLayout::Azerty, 3, PhotoLength::Quick);
        menu.open();
        assert_eq!(menu.press(Target::FastWalk), vec![Effect::FastWalk(true)]);
        assert_eq!(menu.press(Target::FastWalk), vec![Effect::FastWalk(false)]);
        menu.desktop_mode = true;
        assert!(menu.press(Target::FastWalk).is_empty(), "no fast walk in desktop mode");
        menu.desktop_mode = false;
        menu.set_contacts((0..7).map(|i| Contact { id: format!("u{i}"), name: format!("Friend {i}") }).collect(), true, "");
        menu.press(Target::Call);
        menu.press(Target::Next);
        assert_eq!(menu.page_contacts().len(), 2);
        assert_eq!(menu.press(Target::Contact(1)), vec![Effect::CallContact { id: "u6".into() }]);
        menu.set_call(Some(Call::Calling { name: "Friend 6".into() }));
        assert!(menu.press(Target::Contact(0)).is_empty(), "one call at a time");
        assert_eq!(menu.press(Target::HangUp), vec![Effect::HangUp]);
        menu.set_call(None);
        menu.press(Target::Back);
        menu.set_call(Some(Call::Ringing { name: "Ana".into() }));
        assert_eq!(menu.page, Page::Call, "a ringing call opens the call page");
        assert_eq!(menu.press(Target::Answer), vec![Effect::Answer]);
        menu.press(Target::Back);
        menu.press(Target::Photo);
        assert_eq!(menu.press(Target::Timer(3)), vec![Effect::PhotoChoice { timer: 10, length: PhotoLength::Quick }]);
        assert_eq!(menu.press(Target::Length(2)), vec![Effect::PhotoChoice { timer: 10, length: PhotoLength::Full }]);
        assert_eq!(menu.press(Target::Take), vec![Effect::Photo3d { timer: 10, length: PhotoLength::Full }]);
        menu.countdown = Some(8);
        assert!(menu.press(Target::Take).is_empty(), "one picture at a time");
    }

    #[test]
    fn hears_a_search_on_the_search_page_only() {
        let mut menu = Menu::default();
        assert_eq!(menu.heard("cats"), None, "closed");
        menu.open();
        assert_eq!(menu.heard("what time is it?"), None, "on Home it is for Orb");
        assert_eq!(menu.heard.as_deref(), Some("what time is it"));
        assert!(menu.heard_private, "the owner's own words");
        menu.press(Target::YouTube);
        assert_eq!(menu.heard("Lofi hip hop."), Some(vec![Effect::Search { id: 1, query: "Lofi hip hop".into() }]));
        assert_eq!(clock(42), "0:42");
        assert_eq!(clock(3723), "1:02:03");
    }
}
