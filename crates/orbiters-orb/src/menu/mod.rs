//! Orb's menu (contracts `Docs/orb.md`, Menu): what opens when the person calls Orb (a controller button, the wake word
//! on its own, touching the orb): fast walk, YouTube, calls and a 3D picture. Orbiters desktop's SteamVR overlay shows
//! it, or the avatar's projected screen when the Orb prop has one; both draw the same pages from [`layout`].
//!
//! This module holds what the menu shows and what each press does, without IO: presses and typing return [`Effect`]s
//! that the host carries out (fetching suggestions and results, copying a link, starting a call), and the host hands the
//! answers back. [`screen`] turns a menu into what the avatar's screen is sent; [`touch`] finds where a finger touches it.

pub mod layout;
pub mod screen;
pub mod touch;

use crate::youtube::{Results, Video};

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
}

/// What the host does for the menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Suggestions for this text (sent once per text; the host may drop answers to older texts).
    Suggest(String),
    Search(String),
    /// The next page of results for `query`.
    More { query: String, token: String },
    /// Thumbnails the page shows (video ids); the host fetches those it does not have.
    Thumbnails(Vec<String>),
    /// Put a video's link in the clipboard.
    Copy { link: String, title: String },
    FastWalk(bool),
    CallContact { id: String },
    Answer,
    HangUp,
    /// Take a 3D picture after `timer` seconds.
    Photo3d { timer: u32, length: PhotoLength },
    /// Keep the picked timer and length.
    PhotoChoice { timer: u32, length: PhotoLength },
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
    /// The text suggestions were last asked for (so each text is asked once).
    asked_for: String,
    /// Results of `results_for`, every page fetched so far.
    pub results: Vec<Video>,
    pub results_for: String,
    continuation: Option<String>,
    /// The page of results shown (from 0).
    pub result_page: usize,
    pub searching: bool,
    loading_more: bool,
    /// Orb listens to the person.
    pub listening: bool,
    pub fast_walk: bool,
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
    /// What Orb heard last (shown on Home).
    pub heard: Option<String>,
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
            Target::Field => Vec::new(),
            Target::Search => self.search(),
            Target::Suggestion(i) => match self.shown_suggestions().get(i as usize).cloned() {
                Some(text) => {
                    self.query = cut(&text, QUERY_MAX);
                    self.search()
                }
                None => Vec::new(),
            },
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
        vec![Effect::Suggest(text)]
    }

    /// The suggestions to show: those for what is typed, or for an earlier text while the new ones are on their way
    /// (only while they still start like it, so they never contradict the field).
    pub fn shown_suggestions(&self) -> Vec<String> {
        let typed = self.query.trim().to_lowercase();
        if typed.is_empty() {
            return Vec::new();
        }
        let fresh = self.suggested_for.trim().eq_ignore_ascii_case(self.query.trim());
        self.suggestions
            .iter()
            .filter(|s| fresh || s.to_lowercase().starts_with(&typed))
            .filter(|s| !s.trim().eq_ignore_ascii_case(self.query.trim()) || !fresh)
            .take(SUGGESTIONS)
            .cloned()
            .collect()
    }

    /// Suggestions arrived for `text`.
    pub fn suggestions_arrived(&mut self, text: &str, suggestions: Vec<String>) {
        // An answer to a text the field no longer starts with is stale.
        if !self.query.trim_start().to_lowercase().starts_with(&text.trim().to_lowercase()) {
            return;
        }
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
        self.results.clear();
        self.continuation = None;
        self.results_for = query.clone();
        self.result_page = 0;
        self.go(Page::Results);
        vec![Effect::Search(query)]
    }

    /// Search for `query` straight away (said aloud, or asked of Orb).
    pub fn search_for(&mut self, query: &str) -> Vec<Effect> {
        self.open = true;
        self.query = cut(query.trim(), QUERY_MAX);
        self.search()
    }

    /// Results arrived for `query`: the first page, or the next one (`more`).
    pub fn results_arrived(&mut self, query: &str, results: Results, more: bool) -> Vec<Effect> {
        if query != self.results_for {
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
        if self.results.is_empty() {
            self.notice = Some(format!("Nothing found for \"{}\"", self.results_for));
        }
        self.thumbnails()
    }

    /// The search or the next page failed.
    pub fn results_failed(&mut self, query: &str, error: &str) {
        if query != self.results_for {
            return;
        }
        self.searching = false;
        self.loading_more = false;
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
                    let more = self.fetch_more();
                    if !more.is_empty() {
                        // The page turns when its results arrive.
                        self.result_page += 1;
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
                vec![Effect::More { query: self.results_for.clone(), token }]
            }
            None => Vec::new(),
        }
    }

    /// Thumbnails of the results page shown.
    pub fn thumbnails(&self) -> Vec<Effect> {
        let ids: Vec<String> = self.page_results().iter().map(|v| v.id.clone()).collect();
        if ids.is_empty() {
            Vec::new()
        } else {
            vec![Effect::Thumbnails(ids)]
        }
    }

    /// Whether the next page of results is being fetched (shown as loading).
    pub fn loading(&self) -> bool {
        self.searching || (self.loading_more && self.page_results().is_empty())
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
        None
    }
}

/// `text`'s first `max` characters.
pub fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
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

    fn video(id: &str) -> Video {
        Video { id: format!("{id:0>11}"), title: format!("Title {id}"), channel: "Channel".into(), duration: Some("3:21".into()), live: false }
    }

    fn results(ids: std::ops::Range<usize>, more: bool) -> Results {
        Results { videos: ids.map(|i| video(&i.to_string())).collect(), continuation: more.then(|| "token".to_string()) }
    }

    #[test]
    fn types_suggests_and_searches() {
        let mut menu = Menu::default();
        assert!(menu.press(Target::YouTube).is_empty(), "closed: presses do nothing");
        menu.open();
        menu.press(Target::YouTube);
        assert_eq!(menu.page, Page::Search);
        assert!(menu.type_text(" ").is_empty(), "no leading space");
        assert_eq!(menu.press(Target::Key('l')), vec![Effect::Suggest("l".into())]);
        assert_eq!(menu.type_text("ofi"), vec![Effect::Suggest("lofi".into())]);
        assert_eq!(menu.type_text(" "), vec![Effect::Suggest("lofi ".into())]);
        assert!(menu.type_text(" ").is_empty(), "no double space");
        menu.suggestions_arrived("lof", vec!["loft".into()]);
        menu.suggestions_arrived("lofi", vec!["lofi hip hop".into(), "lofi girl".into(), "LOFI".into()]);
        assert_eq!(menu.shown_suggestions(), vec!["lofi hip hop", "lofi girl"], "suggestions start like the field; the field itself is not repeated");
        menu.suggestions_arrived("lol", vec!["lol".into()]);
        assert_eq!(menu.suggestions.len(), 3, "an answer for another text is dropped");
        menu.backspace();
        assert_eq!(menu.query, "lofi");
        let effects = menu.press(Target::Suggestion(1));
        assert_eq!(effects, vec![Effect::Search("lofi girl".into())]);
        assert_eq!((menu.page, menu.searching, menu.query.as_str()), (Page::Results, true, "lofi girl"));
        for _ in 0..60 {
            menu.type_text("x");
        }
        assert_eq!(menu.query, "lofi girl", "typing goes nowhere outside the search page");
    }

    #[test]
    fn pages_through_results_and_fetches_ahead() {
        let mut menu = Menu::default();
        menu.open();
        let effects = menu.search_for("cats");
        assert_eq!(effects, vec![Effect::Search("cats".into())]);
        assert!(menu.results_arrived("dogs", results(0..20, true), false).is_empty(), "results of another search are dropped");
        let effects = menu.results_arrived("cats", results(0..12, true), false);
        assert_eq!(menu.page_results().len(), 5);
        assert!(matches!(&effects[..], [Effect::Thumbnails(ids)] if ids.len() == 5));
        assert!(!menu.has_previous() && menu.has_next());
        menu.press(Target::Next);
        assert_eq!(menu.result_page, 1);
        menu.press(Target::Next);
        // Page 3 has two results; there is no page after them yet, so more are fetched.
        assert_eq!(menu.result_page, 2);
        assert_eq!(menu.page_results().len(), 2);
        let effects = menu.press(Target::Next);
        assert!(effects.is_empty(), "already fetching");
        let effects = menu.results_arrived("cats", results(12..20, false), true);
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
    fn walks_calls_and_takes_pictures() {
        let mut menu = Menu::new(KeyboardLayout::Azerty, 3, PhotoLength::Quick);
        menu.open();
        assert_eq!(menu.press(Target::FastWalk), vec![Effect::FastWalk(true)]);
        assert_eq!(menu.press(Target::FastWalk), vec![Effect::FastWalk(false)]);
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
        menu.press(Target::YouTube);
        assert_eq!(menu.heard("Lofi hip hop."), Some(vec![Effect::Search("Lofi hip hop".into())]));
        assert_eq!(clock(42), "0:42");
        assert_eq!(clock(3723), "1:02:03");
    }
}
