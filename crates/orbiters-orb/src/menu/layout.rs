//! Where everything sits on the menu's pages, for both displays: the SteamVR overlay draws each page from this, and the
//! avatar's screen shows the same page as a picture rendered from it (the static parts: cards, buttons, icons, labels)
//! with the changing parts drawn over it from what the desktop sends (texts, thumbnails, which button is hovered,
//! pressed, on or hidden). A page is [`WIDTH`] × [`HEIGHT`] pixels; every position is on a 4-pixel grid, so the screen's
//! layout table holds it in a byte.

use super::{Call, Keys, KeyboardLayout, Menu, Page, Target, CONTACTS_PER_PAGE, RESULTS_PER_PAGE, TIMERS};

pub const WIDTH: u16 = 1024;
pub const HEIGHT: u16 = 576;
/// The grid every position and size sits on.
pub const GRID: u16 = 4;
/// The page's corner radius.
pub const CARD_RADIUS: u16 = 28;

/// The pages as the screen numbers them (`Orb/Screen` page byte; 0 shows nothing).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ScreenPage {
    Home = 1,
    SearchQwerty = 2,
    SearchAzerty = 3,
    SearchSymbols = 4,
    Results = 5,
    CallList = 6,
    CallRinging = 7,
    CallActive = 8,
    Photo = 9,
}

impl ScreenPage {
    pub const ALL: [ScreenPage; 9] = [
        ScreenPage::Home,
        ScreenPage::SearchQwerty,
        ScreenPage::SearchAzerty,
        ScreenPage::SearchSymbols,
        ScreenPage::Results,
        ScreenPage::CallList,
        ScreenPage::CallRinging,
        ScreenPage::CallActive,
        ScreenPage::Photo,
    ];

    /// The page picture's file stem and the overlay screenshot's name.
    pub fn name(self) -> &'static str {
        match self {
            ScreenPage::Home => "home",
            ScreenPage::SearchQwerty => "search-qwerty",
            ScreenPage::SearchAzerty => "search-azerty",
            ScreenPage::SearchSymbols => "search-symbols",
            ScreenPage::Results => "results",
            ScreenPage::CallList => "call-list",
            ScreenPage::CallRinging => "call-ringing",
            ScreenPage::CallActive => "call-active",
            ScreenPage::Photo => "photo",
        }
    }

    /// The page `menu` shows.
    pub fn of(menu: &Menu) -> ScreenPage {
        match menu.page {
            Page::Home => ScreenPage::Home,
            Page::Search => match (menu.keys, menu.layout) {
                (Keys::Symbols, _) => ScreenPage::SearchSymbols,
                (Keys::Letters, KeyboardLayout::Qwerty) => ScreenPage::SearchQwerty,
                (Keys::Letters, KeyboardLayout::Azerty) => ScreenPage::SearchAzerty,
            },
            Page::Results => ScreenPage::Results,
            Page::Call => match menu.call {
                Some(Call::Ringing { .. }) => ScreenPage::CallRinging,
                Some(_) => ScreenPage::CallActive,
                None => ScreenPage::CallList,
            },
            Page::Photo => ScreenPage::Photo,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32 && x < (self.x + self.w) as f32 && y >= self.y as f32 && y < (self.y + self.h) as f32
    }

    pub fn right(&self) -> u16 {
        self.x + self.w
    }

    pub fn bottom(&self) -> u16 {
        self.y + self.h
    }
}

/// How an element looks (both displays draw these the same way).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Look {
    /// A big home tile with an icon tile, a title and a note.
    Tile = 1,
    /// A round icon button (back, close, previous, next).
    Round = 2,
    /// The main action: green.
    Accent = 3,
    /// The search field.
    Field = 4,
    /// A row of a list: shows only when hovered.
    Row = 5,
    /// A keyboard key.
    Key = 6,
    /// A keyboard key that is not a character (switch, backspace).
    KeyDark = 7,
    /// An option among others; green ring when chosen.
    Chip = 8,
    /// Hang up, decline: red.
    Danger = 9,
}

/// The icons the pages use (Phosphor icons in Orbiters desktop's font).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Close,
    Back,
    Walk,
    YouTube,
    Phone,
    PhoneOff,
    Cube,
    Search,
    Backspace,
    Copy,
    Previous,
    Next,
    User,
    /// A suggestion goes into the field.
    Fill,
    Timer,
    Microphone,
}

/// A tinted icon tile's colour (home tiles).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tint {
    Green,
    Red,
    Blue,
    Violet,
}

/// Something that can be pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Element {
    pub target: Target,
    pub rect: Rect,
    pub look: Look,
    pub icon: Option<Icon>,
    pub label: Option<&'static str>,
    /// Home tiles: the icon tile's colour.
    pub tint: Option<Tint>,
}

impl Element {
    /// The corner radius both displays round it with.
    pub fn radius(&self) -> u16 {
        match self.look {
            Look::Tile => 24,
            Look::Round | Look::Field => self.rect.h / 2,
            Look::Accent | Look::Danger | Look::Chip => self.rect.h.min(56) / 2,
            Look::Row => 16,
            Look::Key | Look::KeyDark => 12,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Style {
    /// Near white.
    Primary = 0,
    /// Grey.
    Secondary = 1,
    /// Orbiters green.
    Accent = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Align {
    Left = 0,
    Center = 1,
}

/// What a text slot shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Content {
    /// Home: listening, thinking, or what to do.
    Status,
    /// Home: under a tile's title.
    TileNote(u8),
    /// Home: what Orb heard last.
    Heard,
    Query,
    Suggestion(u8),
    /// Results: the search, or the notice.
    ResultsTitle,
    /// Results and calls: "2/4".
    PageLabel,
    ResultTitle(u8),
    /// Channel and duration.
    ResultMeta(u8),
    /// Calls: who can be called, or why not.
    CallsTitle,
    ContactName(u8),
    CallName,
    CallState,
    PhotoStatus,
}

/// A line of changing text: `cols` cells of `size` pixels high and half as wide, from (`x`, `y`). On the screen, cell
/// `i` is byte `offset + i` of the screen's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Slot {
    pub x: u16,
    pub y: u16,
    pub cols: u8,
    pub size: u8,
    pub style: Style,
    pub align: Align,
    pub offset: u16,
    pub content: Content,
}

impl Slot {
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.cols as u16 * self.size as u16 / 2, self.size as u16)
    }
}

/// A result's thumbnail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Picture {
    pub rect: Rect,
    /// The result on the page (0 to 4).
    pub result: u8,
}

/// Text that never changes on a page (titles, key captions are element labels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Label {
    pub x: u16,
    pub y: u16,
    pub size: u8,
    pub text: &'static str,
    pub style: Style,
    pub bold: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub page: ScreenPage,
    pub elements: Vec<Element>,
    pub slots: Vec<Slot>,
    pub pictures: Vec<Picture>,
    pub labels: Vec<Label>,
    /// Icons drawn without being pressable (a page's illustration).
    pub icons: Vec<(Rect, Icon, Option<Tint>)>,
}

/// The most elements a page has (the screen's masks and layout table hold this many).
pub const MAX_ELEMENTS: usize = 48;
/// The most text slots a page has.
pub const MAX_SLOTS: usize = 16;
/// The thumbnails of a results page.
pub const PICTURES: usize = RESULTS_PER_PAGE;
/// The screen's text: bytes for every page's slots.
pub const TEXT_BYTES: u16 = 360;

/// Where something pressable is.
pub fn hit(layout: &Layout, x: f32, y: f32) -> Option<(usize, Target)> {
    // The last drawn wins (buttons sit on rows).
    layout.elements.iter().enumerate().rev().find(|(_, e)| e.rect.contains(x, y)).map(|(i, e)| (i, e.target))
}

fn element(target: Target, rect: Rect, look: Look) -> Element {
    Element { target, rect, look, icon: None, label: None, tint: None }
}

fn with(mut e: Element, icon: Option<Icon>, label: Option<&'static str>) -> Element {
    e.icon = icon;
    e.label = label;
    e
}

fn round(target: Target, x: u16, icon: Icon) -> Element {
    with(element(target, Rect::new(x, 24, 56, 56), Look::Round), Some(icon), None)
}

/// Builds a page's slots with consecutive offsets in the screen's text.
struct Slots {
    slots: Vec<Slot>,
    next: u16,
}

impl Slots {
    fn new() -> Self {
        Self { slots: Vec::new(), next: 0 }
    }

    fn add(&mut self, x: u16, y: u16, cols: u8, size: u8, style: Style, content: Content) -> &mut Self {
        self.slots.push(Slot { x, y, cols, size, style, align: Align::Left, offset: self.next, content });
        self.next += cols as u16;
        self
    }

    fn centered(&mut self) -> &mut Self {
        if let Some(last) = self.slots.last_mut() {
            last.align = Align::Center;
        }
        self
    }
}

/// The letters of the keyboard's rows, and where each row starts.
fn letter_rows(layout: KeyboardLayout) -> [(&'static str, u16); 3] {
    match layout {
        KeyboardLayout::Qwerty => [("qwertyuiop", 36), ("asdfghjkl", 84), ("'zxcvbnm", 36)],
        KeyboardLayout::Azerty => [("azertyuiop", 36), ("qsdfghjklm", 36), ("'wxcvbn-", 36)],
    }
}

const SYMBOL_ROWS: [(&str, u16); 3] = [("1234567890", 36), ("éèàçùêôïû&", 36), (".,?!:/@#", 36)];
/// The keyboard: four rows of keys 88 × 52, 8 apart.
const KEY_W: u16 = 88;
const KEY_H: u16 = 52;
const KEY_GAP: u16 = 8;
const KEY_ROWS_Y: [u16; 4] = [320, 380, 440, 500];

fn keyboard(elements: &mut Vec<Element>, page: ScreenPage, layout: KeyboardLayout) {
    let rows = if page == ScreenPage::SearchSymbols { SYMBOL_ROWS } else { letter_rows(layout) };
    for (r, (keys, x0)) in rows.iter().enumerate() {
        let y = KEY_ROWS_Y[r];
        for (i, c) in keys.chars().enumerate() {
            let x = x0 + i as u16 * (KEY_W + KEY_GAP);
            elements.push(element(Target::Key(c), Rect::new(x, y, KEY_W, KEY_H), Look::Key));
        }
        if r == 2 {
            // Backspace fills the third row to the right edge (988).
            let x = x0 + keys.chars().count() as u16 * (KEY_W + KEY_GAP);
            elements.push(with(element(Target::Backspace, Rect::new(x, y, 988 - x, KEY_H), Look::KeyDark), Some(Icon::Backspace), None));
        }
    }
    let y = KEY_ROWS_Y[3];
    let switch = if page == ScreenPage::SearchSymbols { (Target::Letters, "ABC") } else { (Target::Symbols, "?123") };
    elements.push(with(element(switch.0, Rect::new(36, y, 184, KEY_H), Look::KeyDark), None, Some(switch.1)));
    elements.push(with(element(Target::Space, Rect::new(228, y, 568, KEY_H), Look::Key), None, Some("space")));
    elements.push(with(element(Target::Search, Rect::new(804, y, 184, KEY_H), Look::Accent), Some(Icon::Search), Some("Search")));
}

/// The page's layout.
pub fn layout(page: ScreenPage) -> Layout {
    let mut elements = Vec::new();
    let mut slots = Slots::new();
    let mut pictures = Vec::new();
    let mut labels = Vec::new();
    let mut icons = Vec::new();
    match page {
        ScreenPage::Home => {
            let tiles = [
                (Target::FastWalk, Icon::Walk, "Fast walk", Tint::Green),
                (Target::YouTube, Icon::YouTube, "YouTube", Tint::Red),
                (Target::Call, Icon::Phone, "Call", Tint::Blue),
                (Target::Photo, Icon::Cube, "3D picture", Tint::Violet),
            ];
            for (i, (target, icon, title, tint)) in tiles.into_iter().enumerate() {
                let x = 32 + i as u16 * 244;
                let mut tile = with(element(target, Rect::new(x, 104, 228, 324), Look::Tile), Some(icon), Some(title));
                tile.tint = Some(tint);
                elements.push(tile);
            }
            elements.push(round(Target::Close, 936, Icon::Close));
            icons.push((Rect::new(32, 24, 56, 56), Icon::Microphone, Some(Tint::Green)));
            labels.push(Label { x: 104, y: 34, size: 32, text: "Orb", style: Style::Primary, bold: true });
            slots.add(188, 40, 48, 24, Style::Accent, Content::Status);
            for i in 0..4u8 {
                slots.add(56 + i as u16 * 244, 376, 14, 24, Style::Secondary, Content::TileNote(i));
            }
            slots.add(32, 472, 64, 28, Style::Secondary, Content::Heard);
        }
        ScreenPage::SearchQwerty | ScreenPage::SearchAzerty | ScreenPage::SearchSymbols => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(with(element(Target::Field, Rect::new(100, 24, 712, 56), Look::Field), Some(Icon::Search), None));
            elements.push(with(element(Target::Search, Rect::new(824, 24, 168, 56), Look::Accent), Some(Icon::Search), Some("Search")));
            for i in 0..super::SUGGESTIONS as u8 {
                let row = Rect::new(32, 96 + i as u16 * 44, 960, 40);
                elements.push(with(element(Target::Suggestion(i), row, Look::Row), Some(Icon::Fill), None));
            }
            let layout = if page == ScreenPage::SearchAzerty { KeyboardLayout::Azerty } else { KeyboardLayout::Qwerty };
            keyboard(&mut elements, page, layout);
            slots.add(164, 40, super::QUERY_MAX as u8, 24, Style::Primary, Content::Query);
            for i in 0..super::SUGGESTIONS as u8 {
                slots.add(60, 104 + i as u16 * 44, 64, 24, Style::Primary, Content::Suggestion(i));
            }
        }
        ScreenPage::Results => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(round(Target::Previous, 808, Icon::Previous));
            elements.push(round(Target::Next, 936, Icon::Next));
            for i in 0..RESULTS_PER_PAGE as u8 {
                elements.push(element(Target::Copy(i), Rect::new(32, 96 + i as u16 * 92, 960, 84), Look::Row));
            }
            for i in 0..RESULTS_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92 + 20;
                elements.push(with(element(Target::Copy(i), Rect::new(820, y, 156, 48), Look::Accent), Some(Icon::Copy), Some("Copy link")));
            }
            slots.add(104, 40, 36, 24, Style::Primary, Content::ResultsTitle);
            slots.add(868, 40, 5, 24, Style::Secondary, Content::PageLabel).centered();
            for i in 0..RESULTS_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92;
                pictures.push(Picture { rect: Rect::new(44, y + 4, 128, 72), result: i });
                slots.add(192, y + 12, 36, 28, Style::Primary, Content::ResultTitle(i));
                slots.add(192, y + 48, 26, 24, Style::Secondary, Content::ResultMeta(i));
            }
        }
        ScreenPage::CallList => {
            elements.push(round(Target::Back, 32, Icon::Back));
            elements.push(round(Target::Previous, 808, Icon::Previous));
            elements.push(round(Target::Next, 936, Icon::Next));
            for i in 0..CONTACTS_PER_PAGE as u8 {
                elements.push(element(Target::Contact(i), Rect::new(32, 96 + i as u16 * 92, 960, 84), Look::Row));
            }
            for i in 0..CONTACTS_PER_PAGE as u8 {
                let y = 96 + i as u16 * 92;
                elements.push(with(element(Target::Contact(i), Rect::new(840, y + 20, 136, 48), Look::Accent), Some(Icon::Phone), Some("Call")));
                icons.push((Rect::new(52, y + 20, 48, 48), Icon::User, None));
            }
            slots.add(104, 40, 52, 24, Style::Primary, Content::CallsTitle);
            slots.add(868, 40, 5, 24, Style::Secondary, Content::PageLabel).centered();
            for i in 0..CONTACTS_PER_PAGE as u8 {
                slots.add(124, 96 + i as u16 * 92 + 28, 40, 28, Style::Primary, Content::ContactName(i));
            }
        }
        ScreenPage::CallRinging | ScreenPage::CallActive => {
            elements.push(round(Target::Back, 32, Icon::Back));
            if page == ScreenPage::CallRinging {
                elements.push(with(element(Target::Answer, Rect::new(292, 400, 200, 72), Look::Accent), Some(Icon::Phone), Some("Answer")));
                elements.push(with(element(Target::HangUp, Rect::new(532, 400, 200, 72), Look::Danger), Some(Icon::PhoneOff), Some("Decline")));
            } else {
                elements.push(with(element(Target::HangUp, Rect::new(412, 400, 200, 72), Look::Danger), Some(Icon::PhoneOff), Some("Hang up")));
            }
            icons.push((Rect::new(464, 92, 96, 96), Icon::User, Some(Tint::Blue)));
            slots.add(272, 212, 20, 48, Style::Primary, Content::CallName).centered();
            slots.add(344, 288, 24, 28, Style::Secondary, Content::CallState).centered();
        }
        ScreenPage::Photo => {
            elements.push(round(Target::Back, 32, Icon::Back));
            for (i, label) in ["Off", "3 s", "5 s", "10 s"].into_iter().enumerate() {
                elements.push(with(element(Target::Timer(i as u8), Rect::new(100 + i as u16 * 152, 156, 140, 56), Look::Chip), None, Some(label)));
            }
            for (i, label) in ["1 s", "4 s", "8 s"].into_iter().enumerate() {
                elements.push(with(element(Target::Length(i as u8), Rect::new(100 + i as u16 * 152, 292, 140, 56), Look::Chip), None, Some(label)));
            }
            elements.push(with(element(Target::Take, Rect::new(100, 400, 444, 76), Look::Accent), Some(Icon::Cube), Some("Take 3D picture")));
            labels.push(Label { x: 104, y: 34, size: 32, text: "3D picture", style: Style::Primary, bold: true });
            labels.push(Label { x: 100, y: 116, size: 24, text: "Timer", style: Style::Secondary, bold: false });
            labels.push(Label { x: 100, y: 252, size: 24, text: "Camera flight", style: Style::Secondary, bold: false });
            icons.push((Rect::new(680, 140, 260, 260), Icon::Cube, Some(Tint::Violet)));
            slots.add(100, 504, 56, 28, Style::Accent, Content::PhotoStatus);
        }
    }
    Layout { page, elements, slots: slots.slots, pictures, labels, icons }
}

/// A tile's note on Home.
fn tile_note(menu: &Menu, tile: u8) -> String {
    match tile {
        0 => (if menu.fast_walk { "On" } else { "Off" }).into(),
        1 => match menu.results.len() {
            0 => "Search videos".into(),
            _ => format!("\"{}\"", menu.results_for),
        },
        2 => match &menu.call {
            Some(Call::Ringing { name }) => format!("{name} calls"),
            Some(Call::Calling { .. }) => "Calling…".into(),
            Some(Call::Connected { seconds, .. }) => super::clock(*seconds),
            None if !menu.calls_ready => "Not available".into(),
            None => match menu.contacts.len() {
                0 => "No friend online".into(),
                1 => "1 friend".into(),
                n => format!("{n} friends"),
            },
        },
        _ => match menu.countdown {
            Some(n) => format!("In {n} s"),
            None if menu.photo_timer == 0 => "No timer".into(),
            None => format!("Timer {} s", menu.photo_timer),
        },
    }
}

/// What a slot shows (not yet cut to its cells).
pub fn text(menu: &Menu, content: Content) -> String {
    match content {
        Content::Status => {
            if menu.listening {
                "Listening… say what you want".into()
            } else {
                "Tap a tile, or call Orb again to talk".into()
            }
        }
        Content::TileNote(i) => tile_note(menu, i),
        Content::Heard => menu.heard.as_ref().map(|h| format!("\"{h}\"")).unwrap_or_default(),
        Content::Query => menu.query.clone(),
        Content::Suggestion(i) => menu.shown_suggestions().get(i as usize).cloned().unwrap_or_default(),
        Content::ResultsTitle => {
            if let Some(notice) = &menu.notice {
                notice.clone()
            } else if menu.loading() {
                format!("Searching \"{}\"…", menu.results_for)
            } else {
                format!("\"{}\"", menu.results_for)
            }
        }
        Content::PageLabel => match menu.page {
            Page::Results => format!("{}/{}", menu.result_page + 1, menu.result_pages()),
            Page::Call => format!("{}/{}", menu.contact_page + 1, menu.contacts.len().div_ceil(CONTACTS_PER_PAGE).max(1)),
            _ => String::new(),
        },
        Content::ResultTitle(i) => menu.page_results().get(i as usize).map(|v| v.title.clone()).unwrap_or_default(),
        Content::ResultMeta(i) => menu
            .page_results()
            .get(i as usize)
            .map(|v| {
                let length = if v.live { "LIVE".to_string() } else { v.duration.clone().unwrap_or_default() };
                if length.is_empty() {
                    v.channel.clone()
                } else {
                    // The duration stays whole; the channel gives way.
                    let room = 26usize.saturating_sub(length.chars().count() + 3);
                    format!("{} · {}", ellipsis(&v.channel, room), length)
                }
            })
            .unwrap_or_default(),
        Content::CallsTitle => {
            if !menu.calls_ready {
                menu.call_note.clone()
            } else {
                match menu.contacts.len() {
                    0 => "No friend can take a call now".into(),
                    1 => "Call a friend · 1 online".into(),
                    n => format!("Call a friend · {n} online"),
                }
            }
        }
        Content::ContactName(i) => menu.page_contacts().get(i as usize).map(|c| c.name.clone()).unwrap_or_default(),
        Content::CallName => match &menu.call {
            Some(Call::Ringing { name } | Call::Calling { name } | Call::Connected { name, .. }) => name.clone(),
            None => String::new(),
        },
        Content::CallState => match &menu.call {
            Some(Call::Ringing { .. }) => "is calling you".into(),
            Some(Call::Calling { .. }) => "Calling…".into(),
            Some(Call::Connected { seconds, muted, .. }) => {
                format!("{}{}", super::clock(*seconds), if *muted { " · muted" } else { "" })
            }
            None => String::new(),
        },
        Content::PhotoStatus => match menu.countdown {
            Some(n) => format!("Starting in {n}… hold still"),
            None => menu.notice.clone().unwrap_or_else(|| "VRChat's camera flies around what it looks at".into()),
        },
    }
}

/// `text` cut to `cols` characters, ending in "…" when cut.
pub fn ellipsis(text: &str, cols: usize) -> String {
    let count = text.chars().count();
    if count <= cols {
        return text.to_string();
    }
    if cols == 0 {
        return String::new();
    }
    let mut cut: String = text.chars().take(cols - 1).collect();
    cut = cut.trim_end().to_string();
    cut.push('…');
    cut
}

/// What a slot shows, cut to its cells (the screen shows exactly this; the overlay too).
pub fn slot_text(menu: &Menu, slot: &Slot) -> String {
    let text = text(menu, slot.content);
    match slot.content {
        // The field keeps the end in view, with the caret.
        Content::Query => {
            let shown: Vec<char> = text.chars().collect();
            let room = slot.cols as usize - 1;
            let start = shown.len().saturating_sub(room);
            shown[start..].iter().collect::<String>() + "|"
        }
        _ => ellipsis(&text, slot.cols as usize),
    }
}

/// How an element shows now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Chosen or switched on: a green ring.
    pub on: bool,
    /// Not there now (no such result, no page before): not drawn.
    pub hidden: bool,
}

pub fn state(menu: &Menu, element: &Element) -> State {
    let on = match element.target {
        Target::FastWalk => menu.fast_walk,
        Target::Timer(i) => TIMERS.get(i as usize) == Some(&menu.photo_timer),
        Target::Length(i) => super::PhotoLength::ALL.get(i as usize) == Some(&menu.photo_length),
        _ => false,
    };
    let hidden = match element.target {
        Target::Suggestion(i) => i as usize >= menu.shown_suggestions().len(),
        Target::Copy(i) => i as usize >= menu.page_results().len(),
        Target::Contact(i) => i as usize >= menu.page_contacts().len() || menu.call.is_some(),
        Target::Previous => !menu.has_previous(),
        Target::Next => !menu.has_next(),
        Target::Take => menu.countdown.is_some(),
        _ => false,
    };
    State { on, hidden }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_fits_the_screen_and_its_tables() {
        for page in ScreenPage::ALL {
            let l = layout(page);
            assert!(l.elements.len() <= MAX_ELEMENTS, "{page:?}: {} elements", l.elements.len());
            assert!(l.slots.len() <= MAX_SLOTS, "{page:?}: {} slots", l.slots.len());
            assert!(l.pictures.len() <= PICTURES);
            let rects = l.elements.iter().map(|e| e.rect).chain(l.pictures.iter().map(|p| p.rect)).chain(l.icons.iter().map(|i| i.0));
            for r in rects {
                assert!(r.right() <= WIDTH && r.bottom() <= HEIGHT, "{page:?}: {r:?} leaves the page");
                for v in [r.x, r.y, r.w, r.h] {
                    assert_eq!(v % GRID, 0, "{page:?}: {r:?} is off the grid");
                }
            }
            for s in &l.slots {
                assert!(s.rect().right() <= WIDTH && s.rect().bottom() <= HEIGHT, "{page:?}: {s:?} leaves the page");
                assert_eq!((s.x % GRID, s.y % GRID), (0, 0), "{page:?}: {s:?} is off the grid");
                assert_eq!(s.size % 2, 0, "cells are half as wide as high, in whole pixels");
            }
            let used: u16 = l.slots.iter().map(|s| s.cols as u16).sum();
            assert!(used <= TEXT_BYTES, "{page:?}: {used} text bytes");
        }
    }

    #[test]
    fn keyboards_have_every_letter_once() {
        for page in [ScreenPage::SearchQwerty, ScreenPage::SearchAzerty] {
            let l = layout(page);
            let mut letters: Vec<char> = l.elements.iter().filter_map(|e| if let Target::Key(c) = e.target { Some(c) } else { None }).filter(|c| c.is_ascii_lowercase()).collect();
            letters.sort();
            letters.dedup();
            assert_eq!(letters.len(), 26, "{page:?}");
            // Keys do not overlap.
            let keys: Vec<Rect> = l.elements.iter().filter(|e| matches!(e.look, Look::Key | Look::KeyDark) || e.target == Target::Search).map(|e| e.rect).collect();
            for (i, a) in keys.iter().enumerate() {
                for b in &keys[i + 1..] {
                    let apart = a.right() <= b.x || b.right() <= a.x || a.bottom() <= b.y || b.bottom() <= a.y;
                    assert!(apart, "{page:?}: {a:?} and {b:?} overlap");
                }
            }
        }
        let symbols = layout(ScreenPage::SearchSymbols);
        assert!(symbols.elements.iter().any(|e| e.target == Target::Key('é')));
        assert!(symbols.elements.iter().any(|e| e.target == Target::Letters));
    }

    #[test]
    fn hits_buttons_over_rows() {
        let l = layout(ScreenPage::Results);
        assert_eq!(hit(&l, 900.0, 140.0).map(|h| h.1), Some(Target::Copy(0)), "the copy button");
        assert_eq!(hit(&l, 400.0, 140.0).map(|h| h.1), Some(Target::Copy(0)), "the row copies too");
        assert_eq!(hit(&l, 60.0, 50.0).map(|h| h.1), Some(Target::Back));
        assert_eq!(hit(&l, 512.0, 570.0), None);
    }

    #[test]
    fn texts_fit_their_slots() {
        let mut menu = Menu::default();
        menu.open();
        menu.press(Target::YouTube);
        menu.type_text("a really long query that will never fit in the field at all");
        let l = layout(ScreenPage::of(&menu));
        let query = l.slots.iter().find(|s| s.content == Content::Query).unwrap();
        let shown = slot_text(&menu, query);
        assert_eq!(menu.query.chars().count(), crate::menu::QUERY_MAX, "the field holds 40 characters");
        assert_eq!(shown.chars().count(), query.cols as usize);
        assert!(shown.ends_with("fit |") && !shown.starts_with("a "), "the field shows the end being typed: {shown}");
        assert_eq!(ellipsis("Lofi Girl and friends", 8), "Lofi Gi…");
        assert_eq!(ellipsis("Lofi", 8), "Lofi");
        menu.search_for("cats");
        menu.results_arrived(
            "cats",
            crate::youtube::Results {
                videos: vec![crate::youtube::Video {
                    id: "n61ULEU7CO0".into(),
                    title: "x".into(),
                    channel: "A channel with a very long name indeed".into(),
                    duration: Some("6:10:58".into()),
                    live: false,
                }],
                continuation: None,
            },
            false,
        );
        let meta = text(&menu, Content::ResultMeta(0));
        assert!(meta.ends_with(" · 6:10:58") && meta.chars().count() <= 26, "{meta}");
        let l = layout(ScreenPage::Results);
        let copy = l.elements.iter().rev().find(|e| e.target == Target::Copy(1)).unwrap();
        assert!(state(&menu, copy).hidden, "one result: the second row is hidden");
        assert!(state(&menu, &l.elements[2]).hidden, "no next page");
    }
}
