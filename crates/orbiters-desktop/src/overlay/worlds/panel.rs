//! The 3D worlds panel, drawn on N4N0's menu overlay in its place (same size and look as the menu's pages): a search
//! field over VRChat's worlds in general as cards (thumbnail, world, author, likes, labels, how smoothly VRChat runs
//! there), those members captured in 3D badged: a search (every VRChat world), or what is browsed meanwhile (the 3D
//! captures, VRChat's popular and active worlds, the member's favorites, the worlds visited: [`super::finder`]),
//! ordered as listed, most liked or smoothest first and kept to the labels the person picks (and to the 3D captures),
//! the panel's own keyboard while typing, and a world's page once chosen: the card grows into it, then it says what
//! Orbiters and VRChat know of the world (its VRChat numbers, tags, frame rate, pictures, other captures) and its
//! feedback, given here as in the VRChat world and on the website (contracts `Docs/world-feedback.md`): Like, labels
//! (AI world with its reason), comments. Pressing a capture's card shows that capture around the person, and the
//! download goes on its page; a world without a capture has Invite me (or Launch VRChat) and Open on VRChat there
//! instead. The capture's controls (Hide, Recenter, See VRChat, how sharp) are not on its page: they are on a bar of
//! their own under the panel ([`bar`]), there whatever the page while a capture shows or loads.

mod bar;
mod pages;
mod shots;

pub use bar::{bar_hit, bar_press, draw_bar, BAR_PX};
pub use shots::shots;

use super::catalog::Entry;
pub use super::finder::Section;
use super::view::Quality;
use egui::epaint::RectShape;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Painter, Pos2, Rect, Stroke, StrokeKind, TextureId};
use orbiters_contracts::world_feedback as contract;
use orbiters_core::maps::{FoundBy, WorldPage, WorldPerformance, WorldPicture};
use orbiters_n4n0_core::feedback::{CommentRow, FeedbackAsk, FeedbackView, LabelCount, WorldFeedback};
use orbiters_n4n0_core::menu::KeyboardLayout;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, widgets};
use std::collections::HashMap;

pub const WIDTH: f32 = 1024.0;
pub const HEIGHT: f32 = 576.0;
/// Cards on a page of results (three across, two down), and on the typing page's single row.
pub const PER_PAGE: usize = 6;
pub const PER_ROW: usize = 3;
/// Characters the search holds.
pub const QUERY_MAX: usize = 40;
/// Comments on a page of Comments, pictures on a page of Pictures.
pub const COMMENTS_PER_PAGE: usize = 4;
pub const PICTURES_PER_PAGE: usize = 8;
/// A second press on Delete within this long deletes the comment (seconds).
const CONFIRM_FOR: f64 = 4.0;
/// A note under the page goes after this long (seconds).
const NOTE_FOR: f64 = 6.0;

const PAGE_FADE: f64 = 0.22;
const MORPH: f64 = 0.42;
const CARD_STAGGER: f64 = 0.035;

const CARD: Color32 = theme::INK_900;
const SURFACE: Color32 = theme::INK_800;
const RAISED: Color32 = theme::INK_750;
const RAISED2: Color32 = theme::INK_700;

/// How the results are ordered: as listed (newest first), by likes, or by how smoothly VRChat runs there (the
/// website's "Smoothest first": the share of the headset's refresh rate, then frames a second; unmeasured last).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Sort {
    #[default]
    Newest,
    Liked,
    Smooth,
}

impl Sort {
    pub fn label(self) -> &'static str {
        match self {
            Sort::Newest => "Newest",
            Sort::Liked => "Most liked",
            Sort::Smooth => "Smoothest",
        }
    }

    /// The next one, round (the browse page's order chip steps through them).
    pub fn next(self) -> Sort {
        match self {
            Sort::Newest => Sort::Liked,
            Sort::Liked => Sort::Smooth,
            Sort::Smooth => Sort::Newest,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Sort::Newest => icons::CLOCK,
            Sort::Liked => icons::HEART,
            Sort::Smooth => icons::GAUGE,
        }
    }
}

/// What a compose page writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Compose {
    Comment,
    /// Why the person thinks the world was made with AI (the label that needs a reason).
    Reason,
}

/// What the panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Browse,
    /// The keyboard under one row of results.
    Typing,
    /// A capture's page.
    Detail,
    /// The labels the results are kept to, and the author's own captures.
    Filter,
    /// Every label of the selected capture's world, to give or take back.
    Labels,
    /// Its world's comments.
    Comments,
    /// A comment or a reason typed on the keyboard.
    Compose(Compose),
    /// Its world's pictures on Orbiters.
    Pictures,
}

/// What can be pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Back,
    Field,
    Key(char),
    Space,
    Backspace,
    Done,
    /// A result card, by its place among the results.
    Card(usize),
    Previous,
    Next,
    Show,
    Hide,
    Recenter,
    SeeThrough,
    Quality(Quality),
    Retry,
    Sort(Sort),
    Filter,
    /// A label of the list (its place in it) on the filter page.
    FilterLabel(u8),
    FilterOfficial,
    /// Only the worlds with a 3D capture.
    Has3d,
    /// A list to browse.
    Section(Section),
    ClearFilters,
    Like,
    Labels,
    /// A label of the list (its place in it), given or taken back.
    Label(u8),
    Comments,
    WriteComment,
    /// The member's own comment (its id).
    DeleteComment(u64),
    Pictures,
    /// A picture of the page (its place among the world's pictures).
    Picture(usize),
    /// The next other capture of the same world.
    Other,
    /// The bar's capture: its page.
    Open,
    /// A world without a capture: an invite to oneself in VRChat (signed in to VRChat), else VRChat launched there.
    Join,
    /// The world's page on vrchat.com.
    OpenWeb,
}

/// What a press asks of the overlay.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Show this capture (its listing id) around the person.
    Show(u64),
    Hide,
    Recenter,
    SeeThrough(bool),
    Quality(Quality),
    /// Back to N4N0's menu.
    Leave,
    /// Read the listing again.
    Refresh,
    /// Read a world's feedback, or send the member's like, label or comment (the app's queue, as N4N0's menus do).
    Feedback(FeedbackAsk),
    /// The panel, on the capture's page.
    Open,
    /// Search what is typed now (Done), without waiting for the typing to pause.
    Search,
    /// A world (its id) in VRChat: an invite to oneself when signed in to VRChat, else VRChat's launcher.
    Join(String),
    /// A world's page on vrchat.com (its id), in the computer's browser.
    OpenWeb(String),
}

/// How the capture view goes, as the panel shows it.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// The capture shown or on its way (its listing id).
    pub current: Option<u64>,
    /// 0 to 1 while it downloads; `None` once it is past that.
    pub download: Option<f32>,
    /// What happens now ("Downloading 12 of 33 MB", "Getting your graphics card ready", "Appearing").
    pub stage: Option<String>,
    /// It shows around the person.
    pub shown: bool,
    pub quality: Quality,
    pub see_through: bool,
    pub error: Option<String>,
    /// The listing is being read; why it could not be.
    pub fetching: bool,
    pub listing_error: Option<String>,
    /// A line of numbers for the curious (splats, the GPU's time a face).
    pub stats: Option<String>,
    /// The world list shown (a search, a section).
    pub list: ListStatus,
}

/// How the world list shown goes ([`super::finder`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListStatus {
    /// A page is on its way.
    pub loading: bool,
    /// Another page follows the ones read.
    pub more: bool,
    /// Why a page could not be read.
    pub error: Option<String>,
    /// Who answered.
    pub source: Option<FoundBy>,
    /// Signed in to VRChat in the app (its own session answers; Favorites and Active need it).
    pub vrchat: bool,
    /// A search of every world applies (two characters typed and the typing paused).
    pub searching: bool,
}

/// What the panel shows of a world's page on the Map explorer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub thumbnail: Option<String>,
    /// How smoothly VRChat runs there for members who share their frame rate.
    pub performance: Option<WorldPerformance>,
    /// The tags its author gave it on VRChat.
    pub tags: Vec<String>,
    /// The first of the members' pictures of it, and how many there are.
    pub pictures: Vec<WorldPicture>,
    pub picture_count: usize,
    /// The world's author, description and numbers on VRChat (a world the list knew by its name only: a visited one).
    pub author: Option<String>,
    pub description: Option<String>,
    pub capacity: Option<u32>,
    pub visits: Option<u64>,
    pub favorites: Option<u64>,
}

/// The tags a world's author gave it, readable: VRChat's own (system_, admin_, feature_, content_…) left out.
pub fn author_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .filter_map(|t| match t.strip_prefix("author_tag_") {
            Some(t) => Some(t),
            None if ["system_", "admin_", "feature_", "content_", "debug_", "author_"].iter().any(|p| t.starts_with(p)) => None,
            None => Some(t.as_str()),
        })
        .map(|t| t.replace('_', " ").trim().to_string())
        .filter(|t| !t.is_empty())
        .take(12)
        .collect()
}

impl Facts {
    pub fn of(page: &WorldPage) -> Self {
        let w = &page.world;
        Self {
            thumbnail: w.thumbnail_url.clone().or_else(|| w.image_url.clone()),
            performance: w.performance.clone().filter(|p| p.fps > 0),
            tags: author_tags(&w.tags),
            pictures: page.pictures.clone(),
            picture_count: page.picture_count.max(page.pictures.len()),
            author: w.author_name.clone().filter(|a| !a.trim().is_empty()),
            description: w.description.clone().filter(|d| !d.trim().is_empty()),
            capacity: w.capacity.filter(|c| *c > 0),
            visits: w.visits,
            favorites: w.favorites,
        }
    }
}

/// What the panel draws from besides its own state: the listing, the capture view, the worlds' feedback and pages.
pub struct Data<'a> {
    pub entries: &'a [Entry],
    /// The world whose page shows, when the list shown no longer has it (another section, another search).
    pub picked: Option<&'a Entry>,
    pub status: &'a Status,
    /// The worlds' feedback as the app knows it (the member's own part included, the panel's presses applied).
    pub feedback: &'a FeedbackView,
    /// Worlds' pages on the Map explorer once fetched, by world id.
    pub facts: &'a HashMap<String, Facts>,
    pub thumb: &'a dyn Fn(&Entry) -> Option<TextureId>,
    /// A world picture's texture once loaded (its id; `true`: the whole picture).
    pub picture: &'a dyn Fn(u64, bool) -> Option<TextureId>,
    /// Now (Unix seconds), for the comments' ages.
    pub now: u64,
}

/// The feedback view's key of a world (the app keeps them lowercase).
pub fn world_key(world_id: &str) -> String {
    world_id.to_ascii_lowercase()
}

/// RFC 3339 in Unix seconds.
pub fn unix_seconds(at: &str) -> Option<u64> {
    time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).ok().and_then(|t| u64::try_from(t.unix_timestamp()).ok())
}

/// `12 Oct 2026`.
pub fn short_date(at: &str) -> Option<String> {
    let t = time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).ok()?;
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    Some(format!("{} {} {}", t.day(), MONTHS[t.month() as usize - 1], t.year()))
}

/// A label's English name (the overlay's menu is in English), its id when the list does not have it.
pub fn label_name(id: &str) -> &str {
    contract::label(id).map_or(id, |l| l.name("en"))
}

impl<'a> Data<'a> {
    pub fn facts_of(&self, e: &Entry) -> Option<&'a Facts> {
        self.facts.get(&e.world_id)
    }

    /// The world's feedback once the app read it.
    fn read(&self, e: &Entry) -> Option<&'a WorldFeedback> {
        self.feedback.world(&world_key(&e.world_id)).filter(|w| !w.loading)
    }

    pub fn likes(&self, e: &Entry) -> u32 {
        self.read(e).map_or(e.feedback.likes, |w| w.likes)
    }

    pub fn label_count(&self, e: &Entry, id: &str) -> u32 {
        self.read(e).map_or(e.feedback.label(id), |w| w.label(id).map_or(0, |l| l.count))
    }

    /// What the panel shows of a world's feedback: the app's once read (the member's own part with it), else the
    /// listing's for everyone (`loading` then: what the member gave is not known yet).
    pub fn world_feedback(&self, e: &Entry) -> WorldFeedback {
        if let Some(w) = self.read(e) {
            return w.clone();
        }
        let agg = &e.feedback;
        WorldFeedback {
            world: world_key(&e.world_id),
            loading: true,
            likes: agg.likes,
            labels: agg.labels.iter().map(|(id, count)| LabelCount { id: id.clone(), count: *count, mine: false }).collect(),
            comments: agg
                .recent
                .iter()
                .map(|(by, text, at)| CommentRow { id: 0, by: by.clone(), text: text.clone(), at: at.as_deref().and_then(unix_seconds).unwrap_or(self.now), mine: false })
                .collect(),
            comment_count: agg.comments,
            error: self.feedback.world(&world_key(&e.world_id)).and_then(|w| w.error.clone()),
            ..Default::default()
        }
    }

    /// How smoothly VRChat runs there, when members measured it (its page's, else the list's).
    pub fn performance(&self, e: &'a Entry) -> Option<&'a WorldPerformance> {
        self.facts_of(e).and_then(|f| f.performance.as_ref()).or(e.performance.as_ref())
    }

    /// Its frame rate's (share of the refresh rate, frames a second), when members measured it.
    fn smoothness(&self, e: &'a Entry) -> Option<(f32, u32)> {
        self.performance(e).map(|p| (p.ratio, p.fps))
    }

    /// Its author: the list's, else its page's.
    pub fn author(&self, e: &'a Entry) -> Option<&'a str> {
        e.author.as_deref().or_else(|| self.facts_of(e).and_then(|f| f.author.as_deref()))
    }

    /// The other captures of `e`'s world, in the listing's order.
    pub fn others(&self, e: &Entry) -> Vec<&'a Entry> {
        if !e.capture {
            return Vec::new();
        }
        self.entries.iter().filter(|o| o.capture && o.id != e.id && o.world_id.eq_ignore_ascii_case(&e.world_id)).collect()
    }
}

/// How smoothly VRChat runs there, as the website says it (share of the headset's refresh rate).
pub fn verdict(p: &WorldPerformance) -> (&'static str, Color32) {
    if p.ratio >= 0.85 {
        ("smooth", theme::ACCENT)
    } else if p.ratio >= 0.6 {
        ("fair", theme::WARN)
    } else {
        ("heavy", theme::DANGER)
    }
}

#[derive(Clone, Copy, Debug)]
struct Morph {
    from: Rect,
    at: f64,
}

/// The panel's state.
#[derive(Clone, Debug, Default)]
pub struct Panel {
    pub page: Page,
    pub query: String,
    pub results_page: usize,
    pub hover: Option<Target>,
    pub pressed: Option<Target>,
    /// The capture whose page shows (its listing id).
    pub selected: Option<u64>,
    pub layout: KeyboardLayout,
    pub sort: Sort,
    /// What is browsed while nothing is typed.
    pub section: Section,
    /// The labels the results are kept to (all of them), the captures shared by the world's own author only, and the
    /// worlds with a 3D capture only.
    pub labels: Vec<String>,
    pub official: bool,
    pub has_3d: bool,
    /// What a compose page typed.
    pub draft: String,
    pub comments_page: usize,
    pub pictures_page: usize,
    /// The picture shown whole on the pictures page.
    pub picture: Option<usize>,
    /// The member's comment whose Delete was pressed once, and when: a second press deletes it.
    confirm: Option<(u64, f64)>,
    /// Why a press did nothing (signed out, not read yet, a text refused), and when.
    pub note: Option<(String, f64)>,
    morph: Option<Morph>,
    page_at: f64,
    /// Something changed: paint again.
    pub dirty: bool,
}

fn ease_out(t: f64) -> f32 {
    let t = t.clamp(0.0, 1.0) as f32;
    1.0 - (1.0 - t).powi(3)
}

/// Overshoots a little before settling (the app's morphs).
fn ease_back(t: f64) -> f32 {
    let t = t.clamp(0.0, 1.0) as f32;
    let c = 1.4f32;
    1.0 + (c + 1.0) * (t - 1.0).powi(3) + c * (t - 1.0).powi(2)
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::from_min_max(a.min + (b.min - a.min) * t, a.max + (b.max - a.max) * t)
}

fn fade(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

const HERO: Rect = Rect { min: Pos2 { x: 32.0, y: 100.0 }, max: Pos2 { x: 576.0, y: 406.0 } };

/// The detail page's places.
const SHOW: Rect = Rect { min: Pos2 { x: 332.0, y: 350.0 }, max: Pos2 { x: 560.0, y: 398.0 } };
const LIKE: Rect = Rect { min: Pos2 { x: 608.0, y: 310.0 }, max: Pos2 { x: 792.0, y: 366.0 } };
const COMMENTS: Rect = Rect { min: Pos2 { x: 800.0, y: 310.0 }, max: Pos2 { x: 992.0, y: 366.0 } };
const PICTURES: Rect = Rect { min: Pos2 { x: 608.0, y: 460.0 }, max: Pos2 { x: 792.0, y: 516.0 } };
const OTHER: Rect = Rect { min: Pos2 { x: 800.0, y: 460.0 }, max: Pos2 { x: 992.0, y: 516.0 } };
const TEASER: Rect = Rect { min: Pos2 { x: 32.0, y: 460.0 }, max: Pos2 { x: 576.0, y: 548.0 } };
/// Labels on a capture's page: the most given (five), then Labels.
const TOP_LABELS: usize = 5;

fn top_label_rect(k: usize) -> Rect {
    Rect::from_min_size(pos2(608.0 + (k % 3) as f32 * 131.0, 378.0 + (k / 3) as f32 * 38.0), vec2(122.0, 30.0))
}

/// The browse page's second row: what is browsed (searching: 3D only), the order, the labels the results are kept to.
const SECTION_W: [f32; 5] = [118.0, 90.0, 106.0, 98.0, 86.0];
fn section_rect(i: usize) -> Rect {
    let x = 32.0 + SECTION_W[..i].iter().sum::<f32>();
    Rect::from_min_size(pos2(x, 96.0), vec2(SECTION_W[i], 40.0))
}
const ONLY_3D: Rect = Rect { min: Pos2 { x: 32.0, y: 96.0 }, max: Pos2 { x: 164.0, y: 136.0 } };
const SORT: Rect = Rect { min: Pos2 { x: 542.0, y: 96.0 }, max: Pos2 { x: 692.0, y: 136.0 } };
const FILTER: Rect = Rect { min: Pos2 { x: 700.0, y: 96.0 }, max: Pos2 { x: 944.0, y: 136.0 } };
const CLEAR: Rect = Rect { min: Pos2 { x: 952.0, y: 96.0 }, max: Pos2 { x: 992.0, y: 136.0 } };

impl Panel {
    /// The results for the search typed so far, kept to the labels picked, in the order chosen.
    pub fn results<'a>(&self, d: &Data<'a>) -> Vec<&'a Entry> {
        let mut out: Vec<&'a Entry> = d
            .entries
            .iter()
            .filter(|e| {
                e.matches(&self.query) && (!self.official || e.official) && (!self.has_3d || e.capture) && self.labels.iter().all(|l| d.label_count(e, l) > 0)
            })
            .collect();
        match self.sort {
            Sort::Newest => {}
            Sort::Liked => out.sort_by_key(|e| std::cmp::Reverse(d.likes(e))),
            Sort::Smooth => out.sort_by(|a, b| match (d.smoothness(a), d.smoothness(b)) {
                (Some(x), Some(y)) => y.0.total_cmp(&x.0).then(y.1.cmp(&x.1)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }),
        }
        out
    }

    fn filtered(&self) -> bool {
        self.official || self.has_3d || !self.labels.is_empty()
    }

    /// Pages of `n` results: one more while the list has more to read (its next page is read once there).
    fn pages(&self, d: &Data, n: usize) -> usize {
        n.div_ceil(PER_PAGE).max(1) + usize::from(d.status.list.more && n > 0)
    }

    /// A search of every world shows (else what is browsed).
    fn searching(&self, d: &Data) -> bool {
        d.status.list.searching && !self.query.trim().is_empty()
    }

    /// The results are on their way.
    fn loading(&self, d: &Data) -> bool {
        if self.searching(d) || self.section != Section::Captures {
            d.status.list.loading
        } else {
            d.entries.is_empty() && d.status.listing_error.is_none()
        }
    }

    /// Why the results could not be read: a title and a line.
    fn failure<'d>(&self, d: &'d Data) -> Option<(&'static str, &'d str)> {
        if self.searching(d) || self.section != Section::Captures {
            let title = if d.status.list.source == Some(FoundBy::Vrchat) { "VRChat could not be asked" } else { "The worlds could not be read" };
            d.status.list.error.as_deref().map(|e| (title, e))
        } else {
            d.status.listing_error.as_deref().filter(|_| d.entries.is_empty()).map(|e| ("Orbiters could not be reached", e))
        }
    }

    /// The order's name where it applies: as listed is a search's best match, VRChat's lists' top.
    fn sort_label(&self, d: &Data) -> &'static str {
        match self.sort {
            Sort::Newest if self.searching(d) => "Best match",
            Sort::Newest => match self.section {
                Section::Captures => "Newest",
                Section::Visited => "Latest",
                Section::Popular | Section::Active | Section::Favorites => "Top",
            },
            s => s.label(),
        }
    }

    fn go(&mut self, page: Page, time: f64) {
        if page != self.page {
            self.page = page;
            self.page_at = time;
        }
        self.dirty = true;
    }

    /// The world whose page shows.
    pub fn entry<'a>(&self, d: &Data<'a>) -> Option<&'a Entry> {
        let id = self.selected?;
        d.entries.iter().find(|e| e.id == id).or(d.picked.filter(|e| e.id == id))
    }

    pub fn say(&mut self, note: impl Into<String>, time: f64) {
        self.note = Some((note.into(), time));
    }

    /// The note to show now.
    fn note(&self, time: f64) -> Option<&str> {
        self.note.as_ref().filter(|(_, at)| time - at < NOTE_FOR).map(|(n, _)| n.as_str())
    }

    /// Opens on the results (the capture's page when one shows).
    pub fn open(&mut self, status: &Status, time: f64) {
        self.hover = None;
        self.pressed = None;
        self.morph = None;
        self.page_at = time;
        self.note = None;
        self.confirm = None;
        match status.current {
            Some(id) => {
                self.selected = Some(id);
                self.page = Page::Detail;
            }
            None => self.page = Page::Browse,
        }
        self.dirty = true;
    }

    /// Whether something moves (a page fading in, a card growing, a download, a cursor, a Delete waiting).
    pub fn animating(&self, status: &Status, time: f64) -> bool {
        let since = time - self.page_at;
        since < PAGE_FADE + CARD_STAGGER * PER_PAGE as f64 + 0.1
            || self.morph.is_some_and(|m| time - m.at < MORPH + 0.3)
            || status.stage.is_some()
            || status.fetching
            || status.list.loading
            || matches!(self.page, Page::Typing | Page::Compose(_))
            || self.confirm.is_some_and(|(_, at)| time - at < CONFIRM_FOR + 0.1)
            || self.note.as_ref().is_some_and(|(_, at)| time - at < NOTE_FOR + 0.1)
    }

    /// Where each pressable thing is now.
    pub fn targets(&self, d: &Data) -> Vec<(Target, Rect)> {
        let mut out = vec![(Target::Back, Rect::from_center_size(pos2(60.0, 52.0), vec2(56.0, 56.0)))];
        match self.page {
            Page::Browse => {
                let results = self.results(d);
                out.push((Target::Field, field_rect()));
                let pages = self.pages(d, results.len());
                if pages > 1 {
                    out.push((Target::Previous, Rect::from_center_size(pos2(900.0, 52.0), vec2(56.0, 56.0))));
                    out.push((Target::Next, Rect::from_center_size(pos2(964.0, 52.0), vec2(56.0, 56.0))));
                }
                if self.searching(d) {
                    out.push((Target::Has3d, ONLY_3D));
                } else {
                    for (i, s) in Section::ALL.into_iter().enumerate() {
                        out.push((Target::Section(s), section_rect(i)));
                    }
                }
                out.push((Target::Sort(self.sort.next()), SORT));
                out.push((Target::Filter, FILTER));
                if self.filtered() {
                    out.push((Target::ClearFilters, CLEAR));
                }
                let first = self.results_page.min(pages - 1) * PER_PAGE;
                for (i, _) in results.iter().enumerate().skip(first).take(PER_PAGE) {
                    out.push((Target::Card(i), card_rect(i - first)));
                }
                if results.is_empty() && self.failure(d).is_some() {
                    out.push((Target::Retry, Rect::from_center_size(pos2(512.0, 420.0), vec2(200.0, 56.0))));
                }
            }
            Page::Typing => {
                out.push((Target::Field, field_rect()));
                out.push((Target::Done, Rect::from_min_size(pos2(824.0, 24.0), vec2(168.0, 56.0))));
                for (i, _) in self.results(d).iter().enumerate().take(PER_ROW) {
                    out.push((Target::Card(i), compact_rect(i)));
                }
                out.extend(keys(self.layout, false));
            }
            Page::Detail => {
                let Some(e) = self.entry(d) else { return out };
                let current = d.status.current == Some(e.id) && (d.status.shown || d.status.stage.is_some());
                if !e.capture {
                    out.push((Target::Join, SHOW));
                } else if !current {
                    out.push((Target::Show, SHOW));
                }
                out.push((Target::Like, LIKE));
                out.push((Target::Comments, COMMENTS));
                out.push((Target::Comments, TEASER));
                let wf = d.world_feedback(e);
                let top = wf.top();
                for (k, l) in top.iter().take(TOP_LABELS).enumerate() {
                    if let Some(i) = contract::labels().iter().position(|x| x.id == l.id) {
                        out.push((Target::Label(i as u8), top_label_rect(k)));
                    }
                }
                out.push((Target::Labels, top_label_rect(top.len().min(TOP_LABELS))));
                if d.facts_of(e).is_some_and(|f| f.picture_count > 0) || e.pictures > 0 {
                    out.push((Target::Pictures, PICTURES));
                }
                if !d.others(e).is_empty() {
                    out.push((Target::Other, OTHER));
                } else if super::finder::web_link(&e.world_id).is_some() {
                    out.push((Target::OpenWeb, OTHER));
                }
            }
            Page::Filter | Page::Labels | Page::Comments | Page::Compose(_) | Page::Pictures => self.page_targets(d, &mut out),
        }
        out
    }

    pub fn hit(&self, d: &Data, x: f32, y: f32) -> Option<Target> {
        self.targets(d).into_iter().rev().find(|(_, r)| r.expand(4.0).contains(pos2(x, y))).map(|(t, _)| t)
    }

    /// Whether the member can give feedback on `e`'s world now: signed in, and its feedback read (what they gave is
    /// known). Says why not otherwise.
    fn can_give(&mut self, d: &Data, e: &Entry, time: f64) -> Option<WorldFeedback> {
        if !d.feedback.signed_in {
            self.say(contract::Refusal::SignIn.text(), time);
            return None;
        }
        let wf = d.world_feedback(e);
        if wf.loading {
            self.say("Reading this world's feedback from Orbiters…", time);
            return None;
        }
        Some(wf)
    }

    /// The person pressed `target`.
    pub fn press(&mut self, target: Target, d: &Data, time: f64) -> Vec<Action> {
        self.dirty = true;
        let status = d.status;
        match target {
            Target::Back => match self.page {
                Page::Detail => {
                    self.go(if self.query.is_empty() { Page::Browse } else { Page::Typing }, time);
                    self.morph = None;
                    Vec::new()
                }
                Page::Typing | Page::Filter => {
                    self.go(Page::Browse, time);
                    Vec::new()
                }
                Page::Pictures if self.picture.is_some() => {
                    self.picture = None;
                    Vec::new()
                }
                Page::Labels | Page::Comments | Page::Pictures => {
                    self.morph = None;
                    self.go(Page::Detail, time);
                    Vec::new()
                }
                Page::Compose(Compose::Comment) => {
                    self.go(Page::Comments, time);
                    Vec::new()
                }
                Page::Compose(Compose::Reason) => {
                    self.go(Page::Labels, time);
                    Vec::new()
                }
                Page::Browse => vec![Action::Leave],
            },
            Target::Field => {
                if self.page != Page::Compose(Compose::Comment) && self.page != Page::Compose(Compose::Reason) {
                    self.go(Page::Typing, time);
                }
                Vec::new()
            }
            Target::Done => match self.page {
                Page::Compose(c) => self.send(c, d, time),
                Page::Filter => {
                    self.results_page = 0;
                    self.go(Page::Browse, time);
                    Vec::new()
                }
                _ => {
                    self.results_page = 0;
                    self.go(Page::Browse, time);
                    vec![Action::Search]
                }
            },
            Target::Key(c) => {
                self.type_text(&c.to_string());
                Vec::new()
            }
            Target::Space => {
                self.type_text(" ");
                Vec::new()
            }
            Target::Backspace => {
                self.erase();
                Vec::new()
            }
            Target::Previous | Target::Next => {
                let forward = target == Target::Next;
                let step = |at: usize, pages: usize| if forward { (at + 1) % pages } else { (at + pages - 1) % pages };
                match self.page {
                    Page::Comments => {
                        let n = self.entry(d).map_or(0, |e| d.world_feedback(e).comments.len());
                        self.comments_page = step(self.comments_page, n.div_ceil(COMMENTS_PER_PAGE).max(1));
                    }
                    Page::Pictures => {
                        let n = self.entry(d).and_then(|e| d.facts_of(e)).map_or(0, |f| f.pictures.len());
                        self.pictures_page = step(self.pictures_page, n.div_ceil(PICTURES_PER_PAGE).max(1));
                    }
                    _ => self.results_page = step(self.results_page, self.pages(d, self.results(d).len())),
                }
                self.page_at = time;
                Vec::new()
            }
            Target::Card(i) => {
                let results = self.results(d);
                let Some(entry) = results.get(i) else { return Vec::new() };
                let from = match self.page {
                    Page::Typing => compact_thumb(compact_rect(i)),
                    _ => card_thumb(card_rect(i % PER_PAGE)),
                };
                self.morph = Some(Morph { from, at: time });
                self.select(entry.id);
                self.go(Page::Detail, time);
                // A world without a capture: its page only.
                if !entry.capture || status.current == Some(entry.id) && (status.shown || status.stage.is_some()) {
                    Vec::new()
                } else {
                    vec![Action::Show(entry.id)]
                }
            }
            Target::Show => self.entry(d).filter(|e| e.capture).map(|e| Action::Show(e.id)).into_iter().collect(),
            Target::Join => self.entry(d).map(|e| Action::Join(e.world_id.clone())).into_iter().collect(),
            Target::OpenWeb => self.entry(d).map(|e| Action::OpenWeb(e.world_id.clone())).into_iter().collect(),
            Target::Has3d => {
                self.has_3d = !self.has_3d;
                self.results_page = 0;
                self.page_at = time;
                Vec::new()
            }
            Target::Section(s) => {
                if self.section != s {
                    self.section = s;
                    self.results_page = 0;
                    self.page_at = time;
                }
                Vec::new()
            }
            Target::Hide => vec![Action::Hide],
            Target::Recenter => vec![Action::Recenter],
            Target::SeeThrough => vec![Action::SeeThrough(!status.see_through)],
            Target::Quality(q) => vec![Action::Quality(q)],
            Target::Retry => vec![Action::Refresh],
            Target::Sort(s) => {
                self.sort = s;
                self.results_page = 0;
                self.page_at = time;
                Vec::new()
            }
            Target::Filter => {
                self.go(Page::Filter, time);
                Vec::new()
            }
            Target::FilterLabel(i) => {
                if let Some(l) = contract::labels().get(i as usize) {
                    match self.labels.iter().position(|x| *x == l.id) {
                        Some(at) => {
                            self.labels.remove(at);
                        }
                        None => self.labels.push(l.id.clone()),
                    }
                    self.results_page = 0;
                }
                Vec::new()
            }
            Target::FilterOfficial => {
                self.official = !self.official;
                self.results_page = 0;
                Vec::new()
            }
            Target::ClearFilters => {
                self.labels.clear();
                self.official = false;
                self.has_3d = false;
                self.results_page = 0;
                self.page_at = time;
                Vec::new()
            }
            Target::Like => {
                let Some(e) = self.entry(d) else { return Vec::new() };
                let Some(wf) = self.can_give(d, e, time) else { return Vec::new() };
                vec![Action::Feedback(FeedbackAsk::Like { world: wf.world, on: !wf.liked })]
            }
            Target::Labels => {
                self.go(Page::Labels, time);
                Vec::new()
            }
            Target::Label(i) => {
                let Some(e) = self.entry(d) else { return Vec::new() };
                let Some(label) = contract::labels().get(i as usize) else { return Vec::new() };
                let Some(wf) = self.can_give(d, e, time) else { return Vec::new() };
                let on = !wf.has(&label.id);
                if on && label.justify.is_some() {
                    // Its reason first (the one given before, to change or send again).
                    self.draft = wf.justification.clone().unwrap_or_default();
                    self.go(Page::Compose(Compose::Reason), time);
                    return Vec::new();
                }
                vec![Action::Feedback(FeedbackAsk::Label { world: wf.world, label: label.id.clone(), on, text: None })]
            }
            Target::Comments => {
                self.comments_page = 0;
                self.confirm = None;
                self.go(Page::Comments, time);
                Vec::new()
            }
            Target::WriteComment => {
                let Some(e) = self.entry(d) else { return Vec::new() };
                if self.can_give(d, e, time).is_some() {
                    self.draft.clear();
                    self.go(Page::Compose(Compose::Comment), time);
                }
                Vec::new()
            }
            Target::DeleteComment(id) => {
                let Some(e) = self.entry(d) else { return Vec::new() };
                if self.confirm.is_some_and(|(c, at)| c == id && time - at < CONFIRM_FOR) {
                    self.confirm = None;
                    vec![Action::Feedback(FeedbackAsk::Uncomment { world: world_key(&e.world_id), comment: id })]
                } else {
                    self.confirm = Some((id, time));
                    Vec::new()
                }
            }
            Target::Pictures => {
                self.pictures_page = 0;
                self.picture = None;
                self.go(Page::Pictures, time);
                Vec::new()
            }
            Target::Picture(i) => {
                self.picture = if self.picture == Some(i) { None } else { Some(i) };
                Vec::new()
            }
            Target::Other => {
                let Some(e) = self.entry(d) else { return Vec::new() };
                let others = d.others(e);
                // The next one after this capture in the listing's order, round.
                let after = d.entries.iter().position(|x| x.id == e.id).unwrap_or(0);
                let next = others.iter().find(|o| d.entries.iter().position(|x| x.id == o.id).unwrap_or(0) > after).or(others.first());
                let Some(next) = next else { return Vec::new() };
                self.select(next.id);
                self.morph = None;
                self.page_at = time;
                if status.current == Some(next.id) && (status.shown || status.stage.is_some()) {
                    Vec::new()
                } else {
                    vec![Action::Show(next.id)]
                }
            }
            Target::Open => vec![Action::Open],
        }
    }

    fn select(&mut self, id: u64) {
        if self.selected != Some(id) {
            self.comments_page = 0;
            self.pictures_page = 0;
            self.picture = None;
            self.confirm = None;
            self.note = None;
        }
        self.selected = Some(id);
    }

    /// Sends what a compose page typed, as the contract wants it; says why not otherwise.
    fn send(&mut self, compose: Compose, d: &Data, time: f64) -> Vec<Action> {
        let Some(e) = self.entry(d) else { return Vec::new() };
        let Some(wf) = self.can_give(d, e, time) else { return Vec::new() };
        let ask = match compose {
            Compose::Comment => contract::comment(&self.draft).map(|text| FeedbackAsk::Comment { world: wf.world.clone(), text }),
            Compose::Reason => {
                let label = contract::labels().iter().find(|l| l.justify.is_some());
                match label {
                    Some(label) => contract::justification(label, &self.draft)
                        .map(|text| FeedbackAsk::Label { world: wf.world.clone(), label: label.id.clone(), on: true, text: Some(text) }),
                    None => return Vec::new(),
                }
            }
        };
        match ask {
            Ok(ask) => {
                self.draft.clear();
                self.note = None;
                self.comments_page = 0;
                self.go(if compose == Compose::Comment { Page::Comments } else { Page::Labels }, time);
                vec![Action::Feedback(ask)]
            }
            Err(refusal) => {
                self.say(refusal.text(), time);
                Vec::new()
            }
        }
    }

    /// The most characters what is typed on this page holds.
    fn room(&self) -> usize {
        match self.page {
            Page::Compose(Compose::Comment) => contract::max_comment(),
            Page::Compose(Compose::Reason) => contract::max_justification(),
            _ => QUERY_MAX,
        }
    }

    fn typed(&mut self) -> &mut String {
        if matches!(self.page, Page::Compose(_)) {
            &mut self.draft
        } else {
            &mut self.query
        }
    }

    fn type_text(&mut self, text: &str) {
        let room = self.room();
        let typed = self.typed();
        for c in text.chars().filter(|c| !c.is_control()) {
            if typed.chars().count() >= room {
                break;
            }
            typed.push(c);
        }
        self.results_page = 0;
        self.dirty = true;
    }

    fn erase(&mut self) {
        self.typed().pop();
        self.results_page = 0;
        self.dirty = true;
    }

    /// A key of the computer's keyboard while the panel types (the service takes them from VRChat).
    pub fn key(&mut self, key: &orbiters_n4n0_core::menu::KeyInput, d: &Data, time: f64) -> Vec<Action> {
        use orbiters_n4n0_core::menu::KeyInput;
        if !self.typing() {
            return Vec::new();
        }
        match key {
            KeyInput::Text(t) => self.type_text(t),
            KeyInput::Backspace => self.erase(),
            KeyInput::Enter => return self.press(Target::Done, d, time),
            KeyInput::Escape => return self.press(Target::Back, d, time),
            KeyInput::Paste => {}
        }
        Vec::new()
    }

    /// Whether the computer's keyboard should type here.
    pub fn typing(&self) -> bool {
        matches!(self.page, Page::Typing | Page::Compose(_))
    }

    /// Draws the panel.
    pub fn draw(&self, p: &Painter, d: &Data, time: f64) {
        let page = Rect::from_min_size(Pos2::ZERO, vec2(WIDTH, HEIGHT));
        p.rect_filled(page, CornerRadius::same(28), CARD);
        let shown = ease_out((time - self.page_at) / PAGE_FADE);
        let targets = self.targets(d);
        let state = |t: Target| (self.hover == Some(t), self.pressed == Some(t));
        // The back button, on every page.
        let (hover, pressed) = state(Target::Back);
        round_button(p, pos2(60.0, 52.0), icons::ARROW_LEFT, hover, pressed, 1.0);
        match self.page {
            Page::Browse => self.browse(p, d, &targets, time, shown),
            Page::Typing => {
                let results = self.results(d);
                field(p, &self.query, "Search VRChat worlds", true, state(Target::Field), time, 1.0);
                let (h, pr) = state(Target::Done);
                pill_button(p, Rect::from_min_size(pos2(824.0, 24.0), vec2(168.0, 56.0)), Some(icons::MAGNIFYING_GLASS), "Done", theme::ACCENT, theme::ON_ACCENT, h, pr, 1.0);
                for (t, rect) in &targets {
                    if let Target::Card(i) = *t {
                        let (h, pr) = state(*t);
                        compact_card(p, *rect, results[i], d, h, pr, shown);
                    }
                }
                if results.is_empty() {
                    let msg = if self.loading(d) {
                        "Searching…"
                    } else if let Some((title, _)) = self.failure(d) {
                        title
                    } else if self.query.trim().chars().count() < 2 {
                        "Type a world's name"
                    } else {
                        "No world matches"
                    };
                    text(p, pos2(512.0, 162.0), Align2::CENTER_CENTER, Txt::new(msg, 19.0).color(fade(theme::FG_3, shown)));
                }
                self.keyboard(p, &targets, shown);
            }
            Page::Detail => self.detail(p, d, time, &targets),
            Page::Filter | Page::Labels | Page::Comments | Page::Compose(_) | Page::Pictures => self.draw_page(p, d, time, &targets, shown),
        }
    }

    fn keyboard(&self, p: &Painter, targets: &[(Target, Rect)], shown: f32) {
        let state = |t: Target| (self.hover == Some(t), self.pressed == Some(t));
        for (t, rect) in targets {
            let (h, pr) = state(*t);
            match *t {
                Target::Key(c) => key_cap(p, *rect, &c.to_string(), None, RAISED, theme::FG, h, pr, shown),
                Target::Space => key_cap(p, *rect, "space", None, RAISED, theme::FG, h, pr, shown),
                Target::Backspace => key_cap(p, *rect, "", Some(icons::BACKSPACE), RAISED2, theme::FG_2, h, pr, shown),
                _ => {}
            }
        }
    }

    fn browse(&self, p: &Painter, d: &Data, targets: &[(Target, Rect)], time: f64, shown: f32) {
        let state = |t: Target| (self.hover == Some(t), self.pressed == Some(t));
        let results = self.results(d);
        field(p, &self.query, "Search VRChat worlds", false, state(Target::Field), time, 1.0);
        // How many, at the field's end (while what is typed leaves room).
        if self.query.chars().count() <= 28 && !results.is_empty() {
            let more = if d.status.list.more { "+" } else { "" };
            let count = if results.len() == 1 { "1 world".to_string() } else { format!("{}{more} worlds", results.len()) };
            text(p, pos2(field_rect().max.x - 28.0, field_rect().center().y), Align2::RIGHT_CENTER, Txt::new(count, 16.0).color(theme::FG_4));
        }
        let pages = self.pages(d, results.len());
        let current = self.results_page.min(pages - 1);
        if pages > 1 {
            let (h, pr) = state(Target::Previous);
            round_button(p, pos2(900.0, 52.0), icons::CARET_LEFT, h, pr, 1.0);
            let (h, pr) = state(Target::Next);
            round_button(p, pos2(964.0, 52.0), icons::CARET_RIGHT, h, pr, 1.0);
            let of = if d.status.list.more { format!("{}/{}+", current + 1, pages - 1) } else { format!("{}/{}", current + 1, pages) };
            text(p, pos2(860.0, 52.0), Align2::RIGHT_CENTER, Txt::new(of, 17.0).color(theme::FG_3));
        }
        // What is browsed (searching: every world, 3D only), the order, the labels kept to.
        if self.searching(d) {
            let (h, pr) = state(Target::Has3d);
            let (bg, fg) = if self.has_3d { (theme::alpha(theme::ACCENT, 0.18), theme::ACCENT) } else { (if h { RAISED } else { SURFACE }, theme::FG_2) };
            chip_button(p, ONLY_3D, icons::CUBE, "3D only", bg, fg, pr, 1.0);
            let from = match d.status.list.source {
                Some(FoundBy::Vrchat) => "Every VRChat world, from your VRChat account",
                Some(FoundBy::Orbiters) => "Every public VRChat world, from Orbiters",
                None => "Every VRChat world",
            };
            text_cut(p, pos2(ONLY_3D.max.x + 14.0, 106.0), SORT.min.x - ONLY_3D.max.x - 24.0, Txt::new(from, 15.0).color(theme::FG_3));
        } else {
            let track = Rect::from_min_max(section_rect(0).min, section_rect(Section::ALL.len() - 1).max);
            p.rect_filled(track, CornerRadius::same(20), SURFACE);
            for (i, s) in Section::ALL.into_iter().enumerate() {
                let (h, pr) = state(Target::Section(s));
                segment(p, section_rect(i), s.label(d.status.list.vrchat), self.section == s, h, pr, 1.0);
            }
        }
        let (h, pr) = state(Target::Sort(self.sort.next()));
        let (bg, fg) = if self.sort != Sort::Newest { (theme::alpha(theme::ACCENT, 0.18), theme::ACCENT) } else { (if h { RAISED } else { SURFACE }, theme::FG_2) };
        chip_button(p, SORT, self.sort.icon(), self.sort_label(d), bg, fg, pr, 1.0);
        let (h, pr) = state(Target::Filter);
        let filter_label = if self.filtered() {
            let mut names: Vec<&str> = self.labels.iter().map(|l| label_name(l)).collect();
            if self.official {
                names.insert(0, "By its author");
            }
            if self.has_3d && !self.searching(d) {
                names.insert(0, "3D only");
            }
            if names.is_empty() {
                "Filter by label".to_string()
            } else {
                names.join(" · ")
            }
        } else {
            "Filter by label".to_string()
        };
        let lit = !self.labels.is_empty() || self.official || self.has_3d && !self.searching(d);
        let (bg, fg) = if lit { (theme::alpha(theme::ACCENT, 0.18), theme::ACCENT) } else { (if h { RAISED } else { SURFACE }, theme::FG_2) };
        chip_button(p, FILTER, icons::TAG, &filter_label, bg, fg, pr, 1.0);
        if self.filtered() {
            let (h, pr) = state(Target::ClearFilters);
            p.circle_filled(CLEAR.center(), if pr { 18.0 } else { 20.0 }, if h { RAISED } else { SURFACE });
            widgets::icon(p, CLEAR.center(), icons::X, 18.0, theme::FG_2);
        }
        if !results.is_empty() && current * PER_PAGE >= results.len() {
            text(p, pos2(512.0, 330.0), Align2::CENTER_CENTER, Txt::new("Finding more worlds…", 19.0).color(fade(theme::FG_3, shown)));
        }
        for (t, rect) in targets {
            if let Target::Card(i) = *t {
                let slot = i - current * PER_PAGE;
                let appear = ease_out((time - self.page_at - slot as f64 * CARD_STAGGER) / PAGE_FADE);
                let lift = (1.0 - appear) * 18.0;
                let (h, pr) = state(*t);
                card(p, rect.translate(vec2(0.0, lift)), results[i], d, h, pr, appear);
            }
        }
        if results.is_empty() {
            self.empty(p, d, shown, state(Target::Retry));
        }
    }

    fn empty(&self, p: &Painter, d: &Data, shown: f32, retry: (bool, bool)) {
        let searching = self.searching(d);
        let (icon, title, line) = if let Some((title, line)) = self.failure(d) {
            (icons::WARNING, title, line)
        } else if !searching && self.section.needs_vrchat() && !d.status.list.vrchat {
            (icons::SIGN_IN, "Sign in to VRChat to see these", "On the computer: Orbiters - N4N0, Settings, VRChat account")
        } else if self.loading(d) {
            let line = match (searching, self.section) {
                (true, _) => "Every VRChat world whose name matches",
                (false, Section::Captures) => "The worlds members captured and shared",
                (false, Section::Visited) => "The worlds you visited in VRChat",
                (false, _) => "From VRChat",
            };
            (icons::GLOBE, "Finding worlds…", line)
        } else if self.has_3d && self.labels.is_empty() && !self.official {
            (icons::CUBE, "No 3D capture among these worlds", "Turn 3D only off to see them all")
        } else if self.filtered() {
            (icons::TAG, "No world has these labels", "Clear the filter, or pick other labels")
        } else if !searching && self.section == Section::Visited {
            (icons::CLOCK, "No world visited yet", "The worlds you visit in VRChat show here")
        } else if !searching && self.section == Section::Favorites {
            (icons::STAR, "No favorite world yet", "Favorite worlds in VRChat to find them here")
        } else if self.query.trim().chars().count() < 2 && !self.query.trim().is_empty() {
            (icons::MAGNIFYING_GLASS, "Type a little more", "Two letters search every VRChat world")
        } else {
            (icons::MAGNIFYING_GLASS, "No world matches", "Try another name, or a 3D capture's author or sharer")
        };
        widgets::icon(p, pos2(512.0, 270.0), icon, 44.0, fade(theme::FG_4, shown));
        text(p, pos2(512.0, 326.0), Align2::CENTER_CENTER, Txt::new(title, 24.0).weight(weight::SEMIBOLD).color(fade(theme::FG, shown)));
        let line: String = line.chars().take(80).collect();
        text(p, pos2(512.0, 360.0), Align2::CENTER_CENTER, Txt::new(line, 17.0).color(fade(theme::FG_3, shown)));
        if self.failure(d).is_some() {
            pill_button(p, Rect::from_center_size(pos2(512.0, 420.0), vec2(200.0, 56.0)), Some(icons::ARROW_CLOCKWISE), "Try again", RAISED, theme::FG, retry.0, retry.1, shown);
        }
    }

    fn detail(&self, p: &Painter, d: &Data, time: f64, targets: &[(Target, Rect)]) {
        let Some(entry) = self.entry(d) else {
            text(p, pos2(512.0, 288.0), Align2::CENTER_CENTER, Txt::new("This world is no longer listed", 22.0).color(theme::FG_3));
            return;
        };
        let status = d.status;
        let state = |t: Target| (self.hover == Some(t), self.pressed == Some(t));
        // The card grows into the page's picture; the rest comes in once it is most of the way.
        let (hero, rest) = match self.morph {
            Some(m) => {
                let t = (time - m.at) / MORPH;
                (lerp_rect(m.from, HERO, ease_back(t)), ease_out((t - 0.45) / 0.55))
            }
            None => (HERO, ease_out((time - self.page_at) / PAGE_FADE)),
        };
        let slide = (1.0 - rest) * 16.0;
        let radius = 14.0 + 8.0 * rest;
        picture(p, hero, (d.thumb)(entry), &entry.world_id, CornerRadius::same(radius as u8), 1.0);
        // The picture's foot: what happens to the capture, or showing it.
        let current = status.current == Some(entry.id);
        let foot = Rect::from_min_max(pos2(hero.min.x, hero.max.y - 64.0), hero.max);
        p.rect_filled(foot, CornerRadius { nw: 0, ne: 0, sw: radius as u8, se: radius as u8 }, fade(theme::black(0.62), rest));
        if current {
            let (dot, label) = if let Some(e) = &status.error {
                (theme::DANGER, e.clone())
            } else if status.shown && status.stage.is_none() {
                (theme::ACCENT, "Around you now".to_string())
            } else {
                (theme::WARN, status.stage.clone().unwrap_or_else(|| "Getting ready".into()))
            };
            let pulse = if status.shown && status.stage.is_none() { 1.0 } else { 0.55 + 0.45 * ((time * 4.0).sin() as f32 * 0.5 + 0.5) };
            p.circle_filled(pos2(hero.min.x + 30.0, foot.center().y), 6.0, fade(dot, rest * pulse));
            let label: String = label.chars().take(60).collect();
            text(p, pos2(hero.min.x + 48.0, foot.center().y), Align2::LEFT_CENTER, Txt::new(label, 18.0).weight(weight::MEDIUM).color(fade(theme::FG, rest)));
            if let Some(f) = status.download {
                let track = Rect::from_min_size(pos2(hero.min.x, hero.max.y - 5.0), vec2(hero.width(), 5.0));
                p.rect_filled(track, CornerRadius::same(2), fade(theme::white(0.12), rest));
                p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * f, 5.0)), CornerRadius::same(2), fade(theme::ACCENT, rest));
            }
        } else if entry.capture {
            text(p, pos2(hero.min.x + 24.0, foot.center().y), Align2::LEFT_CENTER, Txt::new("Its start view where you stand", 15.0).color(fade(theme::FG_2, rest)));
        } else {
            let line = match (entry.occupants.filter(|n| *n > 0), entry.preview) {
                (Some(1), _) => "1 person in it now".to_string(),
                (Some(n), _) => format!("{} people in it now", short_count(n as u64)),
                (None, true) => "Its 3D preview is on orbiters.cc".to_string(),
                (None, false) => "No 3D capture of it yet".to_string(),
            };
            text_cut(p, pos2(hero.min.x + 24.0, foot.center().y - 10.0), SHOW.min.x - hero.min.x - 36.0, Txt::new(line, 15.0).color(fade(theme::FG_2, rest)));
        }
        // The world's name on the header row, beside Back; the rest in the column beside the picture.
        let title = Txt::new(entry.name.clone(), 28.0).weight(weight::SEMIBOLD).color(fade(theme::FG, rest));
        let galley = p.layout_job(title.job_truncated(860.0));
        p.galley(pos2(108.0, 52.0 - galley.size().y / 2.0 - slide / 2.0), galley, theme::FG);
        let facts = d.facts_of(entry);
        let x = 608.0;
        let mut y = 100.0 + slide;
        if let Some(author) = d.author(entry) {
            text_cut(p, pos2(x, y), 384.0, Txt::new(format!("by {author}"), 19.0).weight(weight::MEDIUM).color(fade(theme::FG_2, rest)));
            y += 30.0;
        }
        let mut meta = Vec::new();
        if entry.capture {
            meta.push(format!("{} splats", short_count(entry.splats as u64)));
            if entry.sky {
                meta.push("sky".into());
            }
            if !entry.official {
                if let Some(u) = &entry.uploader {
                    meta.push(format!("3D by {u}"));
                }
            }
            if let Some(date) = entry.captured_at.as_deref().and_then(short_date) {
                meta.push(date);
            }
        } else if let Some((last, visits)) = entry.visited {
            meta.push(if visits == 1 { "Visited once".to_string() } else { format!("Visited {visits} times") });
            if last > 0 {
                meta.push(format!("last {}", orbiters_n4n0_core::feedback::ago(d.now.saturating_sub(last))));
            }
        }
        if !meta.is_empty() {
            text_cut(p, pos2(x, y), 384.0, Txt::new(meta.join(" · "), 16.0).color(fade(theme::FG_3, rest)));
            y += 26.0;
        }
        let mut numbers = Vec::new();
        if let Some(c) = entry.capacity.or(facts.and_then(|f| f.capacity)).filter(|c| *c > 0) {
            numbers.push(format!("{c} people"));
        }
        if let Some(v) = entry.visits.or(facts.and_then(|f| f.visits)) {
            numbers.push(format!("{} visits", short_count(v)));
        }
        if let Some(f) = entry.favorites.or(facts.and_then(|f| f.favorites)) {
            numbers.push(format!("{} favorites", short_count(f)));
        }
        if !numbers.is_empty() {
            text_cut(p, pos2(x, y), 384.0, Txt::new(numbers.join(" · "), 16.0).color(fade(theme::FG_3, rest)));
            y += 30.0;
        }
        // Shared by its author; how smoothly VRChat runs there.
        let mut cx = x;
        if entry.official {
            cx += chip(p, pos2(cx, y), icons::SEAL_CHECK, "By its author", theme::ACCENT, rest) + 8.0;
        }
        if let Some(perf) = d.performance(entry) {
            let (word, tint) = verdict(perf);
            cx += chip(p, pos2(cx, y), icons::GAUGE, &format!("{} fps · {word}", perf.fps), tint, rest) + 10.0;
            let detail = format!("slowest {} · {:.0}% of refresh", perf.fps_low, perf.ratio * 100.0);
            text_cut(p, pos2(cx, y + 6.0), (992.0 - cx).max(0.0), Txt::new(detail, 14.0).color(fade(theme::FG_4, rest)));
        }
        if entry.official || d.performance(entry).is_some() {
            y += 40.0;
        }
        // As much of the world's description as fits above the feedback.
        let room = 300.0 + slide - y;
        let description = if entry.description.trim().is_empty() { facts.and_then(|f| f.description.as_deref()).unwrap_or("") } else { entry.description.as_str() };
        if !description.trim().is_empty() && room > 22.0 {
            let lines = (room / 22.0).floor().max(1.0) as usize;
            let mut job = Txt::new(description.split_whitespace().collect::<Vec<_>>().join(" "), 15.0).color(fade(theme::FG_3, rest)).line_height(22.0).job(384.0);
            job.wrap.max_rows = lines;
            p.galley(pos2(x, y), p.layout_job(job), theme::FG_3);
        }
        // Its feedback: Like, the comments, the labels given most.
        let wf = d.world_feedback(entry);
        let on = |r: Rect| r.translate(vec2(0.0, slide));
        let (h, pr) = state(Target::Like);
        let (bg, fg) = if wf.liked { (theme::alpha(theme::REC, 0.22), theme::REC) } else { (RAISED, theme::FG) };
        let likes = if wf.likes == 1 { "1 like".to_string() } else { format!("{} likes", short_count(wf.likes as u64)) };
        icon_button(p, on(LIKE), icons::HEART, wf.liked, &likes, bg, fg, h, pr, rest * if wf.loading { 0.75 } else { 1.0 });
        let (h, pr) = state(Target::Comments);
        let comments = match wf.comment_count {
            0 => "Comment".to_string(),
            1 => "1 comment".to_string(),
            n => format!("{n} comments"),
        };
        icon_button(p, on(COMMENTS), icons::CHAT_CIRCLE_TEXT, false, &comments, RAISED, theme::FG, h, pr, rest);
        for (t, rect) in targets {
            let (h, pr) = state(*t);
            match *t {
                Target::Label(i) => {
                    let Some(l) = contract::labels().get(i as usize) else { continue };
                    let count = wf.label(&l.id).map_or(0, |c| c.count);
                    label_chip(p, on(*rect), l.name("en"), Some(count), wf.has(&l.id), h, pr, rest);
                }
                Target::Labels => {
                    let r = on(*rect);
                    let r = if pr { r.shrink(1.5) } else { r };
                    p.rect_filled(r, CornerRadius::same(15), fade(if h { RAISED2 } else { RAISED }, rest));
                    widgets::icon(p, pos2(r.min.x + 18.0, r.center().y), icons::PLUS, 16.0, fade(theme::FG_2, rest));
                    text(p, pos2(r.min.x + 32.0, r.center().y), Align2::LEFT_CENTER, Txt::new("Labels", 15.0).weight(weight::MEDIUM).color(fade(theme::FG_2, rest)));
                }
                Target::Show => pill_button(p, on(*rect), Some(icons::CUBE), "Show around me", theme::ACCENT, theme::ON_ACCENT, h, pr, rest),
                Target::Join => {
                    let (glyph, label) = if status.list.vrchat { (icons::ENVELOPE_SIMPLE, "Invite me") } else { (icons::PLAY, "Launch VRChat") };
                    pill_button(p, on(*rect), Some(glyph), label, theme::ACCENT, theme::ON_ACCENT, h, pr, rest);
                }
                Target::OpenWeb => icon_button(p, on(*rect), icons::ARROW_SQUARE_OUT, false, "Open on VRChat", RAISED, theme::FG, h, pr, rest),
                Target::Pictures => {
                    let n = facts.map_or(entry.pictures as usize, |f| f.picture_count.max(entry.pictures as usize));
                    icon_button(p, on(*rect), icons::IMAGES, false, &format!("Pictures · {n}"), RAISED, theme::FG, h, pr, rest);
                }
                Target::Other => {
                    let n = d.others(entry).len();
                    let label = if n == 1 { "1 more capture".to_string() } else { format!("{n} more captures") };
                    icon_button(p, on(*rect), icons::STACK, false, &label, RAISED, theme::FG, h, pr, rest);
                }
                _ => {}
            }
        }
        // Under the picture: the author's tags, the newest comment.
        let tags = match facts.filter(|f| !f.tags.is_empty()) {
            Some(f) => f.tags.clone(),
            None => author_tags(&entry.tags),
        };
        {
            let mut tx = 32.0;
            for tag in &tags {
                let txt = Txt::new(tag.clone(), 14.0).color(fade(theme::FG_3, rest));
                let galley = p.layout_job(txt.job(f32::INFINITY));
                let w = galley.size().x + 24.0;
                if tx + w > 576.0 {
                    break;
                }
                let r = Rect::from_min_size(pos2(tx, 418.0 + slide), vec2(w, 28.0));
                p.rect_filled(r, CornerRadius::same(14), fade(SURFACE, rest));
                p.galley(pos2(r.min.x + 12.0, r.center().y - galley.size().y / 2.0), galley, theme::FG_3);
                tx += w + 6.0;
            }
        }
        let (h, _) = state(Target::Comments);
        let teaser = on(TEASER);
        p.rect_filled(teaser, CornerRadius::same(18), fade(if h { RAISED } else { SURFACE }, rest));
        widgets::icon(p, pos2(teaser.min.x + 30.0, teaser.min.y + 28.0), icons::QUOTES, 22.0, fade(theme::FG_4, rest));
        match wf.comments.first() {
            Some(c) => {
                let by = if c.by.is_empty() { "Someone".to_string() } else { c.by.clone() };
                let head = format!("{by} · {}", orbiters_n4n0_core::feedback::ago(d.now.saturating_sub(c.at)));
                text_cut(p, pos2(teaser.min.x + 56.0, teaser.min.y + 16.0), teaser.width() - 72.0, Txt::new(head, 14.0).weight(weight::MEDIUM).color(fade(theme::FG_3, rest)));
                let mut job = Txt::new(c.text.split_whitespace().collect::<Vec<_>>().join(" "), 16.0).color(fade(theme::FG_2, rest)).line_height(21.0).job(teaser.width() - 72.0);
                job.wrap.max_rows = 2;
                p.galley(pos2(teaser.min.x + 56.0, teaser.min.y + 40.0), p.layout_job(job), theme::FG_2);
            }
            None => {
                let line = if wf.loading && wf.comment_count > 0 { "Reading the comments…" } else { "No comment yet: be the first to write one" };
                text(p, pos2(teaser.min.x + 56.0, teaser.center().y), Align2::LEFT_CENTER, Txt::new(line, 16.0).color(fade(theme::FG_3, rest)));
            }
        }
        // Why a press did nothing, what Orbiters refused, or what went wrong with the capture.
        let note = self.note(time).map(str::to_string).or_else(|| wf.error.clone()).or_else(|| status.error.clone().filter(|_| !current));
        if let Some(note) = note {
            text_cut(p, pos2(608.0, 526.0 + slide), 384.0, Txt::new(note, 15.0).color(fade(theme::WARN, rest)));
        } else if let Some(stats) = status.stats.as_ref().filter(|_| current && status.shown) {
            text(p, pos2(992.0, 540.0), Align2::RIGHT_CENTER, Txt::new(stats.clone(), 14.0).color(fade(theme::FG_4, rest)));
        }
    }
}

fn field_rect() -> Rect {
    Rect::from_min_max(pos2(100.0, 24.0), pos2(808.0, 80.0))
}

fn card_rect(slot: usize) -> Rect {
    let (col, row) = ((slot % PER_ROW) as f32, (slot / PER_ROW) as f32);
    Rect::from_min_size(pos2(32.0 + col * 328.0, 148.0 + row * 200.0), vec2(304.0, 188.0))
}

const CARD_THUMB: f32 = 124.0;

fn card_thumb(card: Rect) -> Rect {
    Rect::from_min_size(card.min, vec2(card.width(), CARD_THUMB))
}

fn compact_rect(i: usize) -> Rect {
    Rect::from_min_size(pos2(32.0 + i as f32 * 328.0, 100.0), vec2(304.0, 120.0))
}

fn compact_thumb(card: Rect) -> Rect {
    Rect::from_min_size(card.min + vec2(10.0, 10.0), vec2(112.0, 100.0))
}

/// The keyboard's keys: digits, three rows of letters (Backspace at the end of the last), then the space bar; with
/// `punctuation` (writing a comment or a reason) four marks beside it.
fn keys(layout: KeyboardLayout, punctuation: bool) -> Vec<(Target, Rect)> {
    let rows: [&str; 4] = match layout {
        KeyboardLayout::Qwerty => ["1234567890", "qwertyuiop", "asdfghjkl'", "zxcvbnm-"],
        KeyboardLayout::Azerty => ["1234567890", "azertyuiop", "qsdfghjklm", "wxcvbn'-"],
    };
    let (w, gap, h) = (88.0f32, 8.0f32, 50.0f32);
    let top = 240.0;
    let mut out = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let y = top + r as f32 * (h + gap);
        let n = row.chars().count() as f32;
        let mut x = if r == 3 { 36.0 } else { 36.0 + (10.0 - n) * (w + gap) / 2.0 };
        for c in row.chars() {
            out.push((Target::Key(c), Rect::from_min_size(pos2(x, y), vec2(w, h))));
            x += w + gap;
        }
        if r == 3 {
            out.push((Target::Backspace, Rect::from_min_max(pos2(x, y), pos2(988.0, y + h))));
        }
    }
    let y = top + 4.0 * (h + gap);
    out.push((Target::Space, Rect::from_min_max(pos2(228.0, y), pos2(796.0, y + h))));
    if punctuation {
        for (i, c) in [',', '.', '?', '!'].into_iter().enumerate() {
            let x = if i < 2 { 36.0 + i as f32 * (w + gap) } else { 804.0 + (i - 2) as f32 * (w + gap) };
            out.push((Target::Key(c), Rect::from_min_size(pos2(x, y), vec2(w, h))));
        }
    }
    out
}

fn text(p: &Painter, at: Pos2, align: Align2, txt: Txt) {
    let galley = p.layout_job(txt.job(f32::INFINITY));
    let rect = align.anchor_size(at, galley.size());
    p.galley(rect.min, galley, txt.color);
}

fn text_cut(p: &Painter, at: Pos2, width: f32, txt: Txt) {
    let galley = p.layout_job(txt.job_truncated(width));
    p.galley(at, galley, txt.color);
}

fn round_button(p: &Painter, center: Pos2, glyph: &str, hover: bool, pressed: bool, alpha: f32) {
    let r = if pressed { 26.0 } else { 28.0 };
    p.circle_filled(center, r, fade(if hover { RAISED2 } else { RAISED }, alpha));
    widgets::icon(p, center, glyph, 26.0, fade(theme::FG, alpha));
}

#[allow(clippy::too_many_arguments)]
fn pill_button(p: &Painter, rect: Rect, glyph: Option<&str>, label: &str, bg: Color32, fg: Color32, hover: bool, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(2.0) } else { rect };
    let bg = if hover { theme::mix(bg, Color32::WHITE, 0.08) } else { bg };
    p.rect_filled(rect, CornerRadius::same((rect.height() / 2.0) as u8), fade(bg, alpha));
    let label_txt = Txt::new(label, 20.0).weight(weight::SEMIBOLD).color(fade(fg, alpha));
    let galley = p.layout_job(label_txt.job_truncated((rect.width() - 40.0).max(10.0)));
    let icon_w = if glyph.is_some() { 30.0 } else { 0.0 };
    let total = icon_w + galley.size().x;
    let left = rect.center().x - total / 2.0;
    if let Some(g) = glyph {
        widgets::icon(p, pos2(left + 11.0, rect.center().y), g, 22.0, fade(fg, alpha));
    }
    p.galley(pos2(left + icon_w, rect.center().y - galley.size().y / 2.0), galley, fade(fg, alpha));
}

/// A pill with an icon (filled when `filled`) and a label, the label smaller than [`pill_button`]'s.
#[allow(clippy::too_many_arguments)]
fn icon_button(p: &Painter, rect: Rect, glyph: &str, filled: bool, label: &str, bg: Color32, fg: Color32, hover: bool, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(2.0) } else { rect };
    let bg = if hover { theme::mix(bg, Color32::WHITE, 0.08) } else { bg };
    p.rect_filled(rect, CornerRadius::same((rect.height() / 2.0) as u8), fade(bg, alpha));
    let txt = Txt::new(label, 18.0).weight(weight::SEMIBOLD).color(fade(fg, alpha));
    let galley = p.layout_job(txt.job_truncated((rect.width() - 52.0).max(10.0)));
    let total = 30.0 + galley.size().x;
    let left = rect.center().x - total / 2.0;
    let at = pos2(left + 11.0, rect.center().y);
    if filled {
        widgets::icon_fill(p, at, glyph, 22.0, fade(fg, alpha));
    } else {
        widgets::icon(p, at, glyph, 22.0, fade(fg, alpha));
    }
    p.galley(pos2(left + 30.0, rect.center().y - galley.size().y / 2.0), galley, fade(fg, alpha));
}

/// A small rounded chip with an icon and a label (a filter, Clear).
#[allow(clippy::too_many_arguments)]
fn chip_button(p: &Painter, rect: Rect, glyph: &str, label: &str, bg: Color32, fg: Color32, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(1.5) } else { rect };
    p.rect_filled(rect, CornerRadius::same((rect.height() / 2.0) as u8), fade(bg, alpha));
    widgets::icon(p, pos2(rect.min.x + 22.0, rect.center().y), glyph, 18.0, fade(fg, alpha));
    text_cut(p, pos2(rect.min.x + 40.0, rect.center().y - 10.0), rect.width() - 54.0, Txt::new(label, 16.0).weight(weight::MEDIUM).color(fade(fg, alpha)));
}

/// One choice of a segmented control (the order, how sharp).
fn segment(p: &Painter, rect: Rect, label: &str, on: bool, hover: bool, pressed: bool, alpha: f32) {
    let bg = if on { theme::FG } else if hover { RAISED } else { SURFACE };
    let fg = if on { CARD } else { theme::FG_2 };
    let r = if pressed { rect.shrink(2.0) } else { rect.shrink(1.0) };
    p.rect_filled(r, CornerRadius::same((r.height() / 2.0) as u8), fade(bg, alpha));
    text(p, r.center(), Align2::CENTER_CENTER, Txt::new(label, 16.0).weight(weight::MEDIUM).color(fade(fg, alpha)));
}

/// A label as a chip: its name and how many gave it; lit when the member gave it.
#[allow(clippy::too_many_arguments)]
fn label_chip(p: &Painter, rect: Rect, name: &str, count: Option<u32>, mine: bool, hover: bool, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(1.5) } else { rect };
    let (bg, fg, sub) = if mine {
        (theme::ACCENT, theme::ON_ACCENT, theme::alpha(theme::ON_ACCENT, 0.7))
    } else {
        (if hover { RAISED2 } else { RAISED }, theme::FG, theme::FG_3)
    };
    let size = if rect.height() > 40.0 { 18.0 } else { 15.0 };
    p.rect_filled(rect, CornerRadius::same((rect.height() / 2.0).min(26.0) as u8), fade(bg, alpha));
    let pad = rect.height() / 2.0;
    let mut right = rect.max.x - pad + 4.0;
    if let Some(n) = count.filter(|n| *n > 0) {
        let galley = p.layout_job(Txt::new(short_count(n as u64), size - 1.0).color(fade(sub, alpha)).job(f32::INFINITY));
        right -= galley.size().x;
        p.galley(pos2(right, rect.center().y - galley.size().y / 2.0), galley, sub);
        right -= 8.0;
    }
    let mut left = rect.min.x + pad - 2.0;
    if mine {
        widgets::icon(p, pos2(left + 7.0, rect.center().y), icons::CHECK, size, fade(fg, alpha));
        left += 20.0;
    }
    text_cut(p, pos2(left, rect.center().y - size * 0.62), (right - left).max(10.0), Txt::new(name, size).weight(weight::MEDIUM).color(fade(fg, alpha)));
}

#[allow(clippy::too_many_arguments)]
fn key_cap(p: &Painter, rect: Rect, label: &str, glyph: Option<&str>, bg: Color32, fg: Color32, hover: bool, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(2.0) } else { rect };
    let bg = if pressed { theme::mix(bg, Color32::WHITE, 0.16) } else if hover { theme::mix(bg, Color32::WHITE, 0.07) } else { bg };
    p.rect_filled(rect, CornerRadius::same(10), fade(bg, alpha));
    match glyph {
        Some(g) => widgets::icon(p, rect.center(), g, 24.0, fade(fg, alpha)),
        None => text(p, rect.center(), Align2::CENTER_CENTER, Txt::new(label, 22.0).color(fade(fg, alpha))),
    }
}

/// The search field, or a compose page's: what is typed (its end when longer than the field), else `hint`.
#[allow(clippy::too_many_arguments)]
fn field(p: &Painter, typed: &str, hint: &str, active: bool, (hover, _): (bool, bool), time: f64, alpha: f32) {
    let rect = field_rect();
    p.rect_filled(rect, CornerRadius::same(28), fade(if hover && !active { RAISED } else { SURFACE }, alpha));
    if active {
        p.rect_stroke(rect, CornerRadius::same(28), Stroke::new(2.0, fade(theme::ACCENT, alpha)), StrokeKind::Inside);
    }
    let glyph = if hint.starts_with("Search") { icons::MAGNIFYING_GLASS } else { icons::PENCIL_SIMPLE };
    widgets::icon(p, pos2(rect.min.x + 34.0, rect.center().y), glyph, 22.0, fade(theme::FG_3, alpha));
    let at = pos2(rect.min.x + 62.0, rect.center().y);
    let room = rect.max.x - at.x - 28.0;
    let shown = if typed.is_empty() && !active {
        Txt::new(hint, 20.0).color(fade(theme::FG_4, alpha))
    } else if typed.is_empty() {
        Txt::new("", 20.0).color(fade(theme::FG, alpha))
    } else {
        // The end of what is typed, where the cursor is.
        let chars: Vec<char> = typed.chars().collect();
        let mut from = chars.len().saturating_sub(64);
        loop {
            let tail: String = chars[from..].iter().collect();
            let galley = p.layout_job(Txt::new(tail.clone(), 20.0).job(f32::INFINITY));
            if galley.size().x <= room || from + 1 >= chars.len() {
                break Txt::new(tail, 20.0).color(fade(theme::FG, alpha));
            }
            from += 2;
        }
    };
    let galley = p.layout_job(shown.job(f32::INFINITY));
    let width = galley.size().x;
    let height = galley.size().y.max(24.0);
    p.galley(pos2(at.x, at.y - galley.size().y / 2.0), galley, theme::FG);
    if active && (time * 2.0).fract() < 0.6 {
        let x = at.x + if typed.is_empty() { 0.0 } else { width + 2.0 };
        p.line_segment([pos2(x, rect.center().y - height / 2.0), pos2(x, rect.center().y + height / 2.0)], Stroke::new(2.0, fade(theme::ACCENT, alpha)));
    }
}

/// A capture's picture: its world's thumbnail, or a placeholder in the world's own colour.
fn picture(p: &Painter, rect: Rect, texture: Option<TextureId>, world_id: &str, corner: CornerRadius, alpha: f32) {
    match texture {
        Some(id) => cover(p, rect, id, 16.0 / 9.0, corner, alpha),
        None => {
            let hue = world_id.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360;
            let base = hsv(hue as f32 / 360.0, 0.45, 0.42);
            p.rect_filled(rect, corner, fade(base, alpha));
            widgets::icon(p, rect.center(), icons::CUBE, (rect.height() * 0.3).clamp(18.0, 64.0), fade(theme::white(0.55), alpha));
        }
    }
}

/// Covers `rect` with a picture of `aspect` (cropped, never stretched).
fn cover(p: &Painter, rect: Rect, id: TextureId, aspect: f32, corner: CornerRadius, alpha: f32) {
    let ratio = rect.width() / rect.height().max(1.0);
    let uv = if ratio > aspect {
        let v = aspect / ratio;
        Rect::from_min_max(pos2(0.0, 0.5 - v / 2.0), pos2(1.0, 0.5 + v / 2.0))
    } else {
        let u = ratio / aspect;
        Rect::from_min_max(pos2(0.5 - u / 2.0, 0.0), pos2(0.5 + u / 2.0, 1.0))
    };
    p.add(RectShape::filled(rect, corner, fade(Color32::WHITE, alpha)).with_texture(id, uv));
}

fn hsv(h: f32, s: f32, v: f32) -> Color32 {
    let rgb = egui::ecolor::Hsva::new(h, s, v, 1.0).to_srgb();
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// A tinted chip at `at`; returns its width.
fn chip(p: &Painter, at: Pos2, glyph: &str, label: &str, tint: Color32, alpha: f32) -> f32 {
    let txt = Txt::new(label, 15.0).weight(weight::MEDIUM).color(fade(tint, alpha));
    let galley = p.layout_job(txt.job(f32::INFINITY));
    let rect = Rect::from_min_size(at, vec2(galley.size().x + 42.0, 30.0));
    p.rect_filled(rect, CornerRadius::same(15), fade(theme::alpha(tint, 0.14), alpha));
    widgets::icon(p, pos2(rect.min.x + 17.0, rect.center().y), glyph, 17.0, fade(tint, alpha));
    p.galley(pos2(rect.min.x + 31.0, rect.center().y - galley.size().y / 2.0), galley, tint);
    rect.width()
}

/// 786 432 → "786k", 1 026 271 → "1.03M".
pub fn short_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{}k", n / 1000),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

/// A badge on a picture's corner: an icon and a number or a word.
fn badge(p: &Painter, at: Pos2, right: bool, glyph: &str, filled: bool, label: &str, tint: Color32, alpha: f32) {
    let galley = p.layout_job(Txt::new(label, 14.0).weight(weight::SEMIBOLD).color(fade(theme::FG, alpha)).job(f32::INFINITY));
    let w = galley.size().x + 36.0;
    let rect = if right { Rect::from_min_size(pos2(at.x - w, at.y), vec2(w, 26.0)) } else { Rect::from_min_size(at, vec2(w, 26.0)) };
    p.rect_filled(rect, CornerRadius::same(13), fade(theme::black(0.6), alpha));
    let c = pos2(rect.min.x + 15.0, rect.center().y);
    if filled {
        widgets::icon_fill(p, c, glyph, 15.0, fade(tint, alpha));
    } else {
        widgets::icon(p, c, glyph, 15.0, fade(tint, alpha));
    }
    p.galley(pos2(rect.min.x + 28.0, rect.center().y - galley.size().y / 2.0), galley, theme::FG);
}

#[allow(clippy::too_many_arguments)]
fn card(p: &Painter, rect: Rect, entry: &Entry, d: &Data, hover: bool, pressed: bool, alpha: f32) {
    let status = d.status;
    let rect = if pressed {
        rect.shrink(3.0)
    } else if hover {
        rect.expand(2.0)
    } else {
        rect
    };
    p.rect_filled(rect, CornerRadius::same(20), fade(if hover { RAISED } else { SURFACE }, alpha));
    let thumb = Rect::from_min_size(rect.min, vec2(rect.width(), CARD_THUMB));
    let top_only = CornerRadius { nw: 20, ne: 20, sw: 0, se: 0 };
    picture(p, thumb, (d.thumb)(entry), &entry.world_id, top_only, alpha);
    if status.current == Some(entry.id) {
        let label = if status.shown { "Around you" } else { "Loading" };
        let tint = if status.shown { theme::ACCENT } else { theme::WARN };
        let txt = Txt::new(label, 14.0).weight(weight::SEMIBOLD).color(fade(theme::ON_ACCENT, alpha));
        let galley = p.layout_job(txt.job(f32::INFINITY));
        let chip = Rect::from_min_size(thumb.min + vec2(12.0, 12.0), vec2(galley.size().x + 22.0, 26.0));
        p.rect_filled(chip, CornerRadius::same(13), fade(tint, alpha));
        p.galley(pos2(chip.min.x + 11.0, chip.center().y - galley.size().y / 2.0), galley, theme::ON_ACCENT);
    } else if entry.capture {
        // A 3D capture: shown around the person from here.
        let txt = Txt::new("3D", 14.0).weight(weight::SEMIBOLD).color(fade(theme::ON_ACCENT, alpha));
        let galley = p.layout_job(txt.job(f32::INFINITY));
        let chip = Rect::from_min_size(thumb.min + vec2(12.0, 12.0), vec2(galley.size().x + 44.0, 26.0));
        p.rect_filled(chip, CornerRadius::same(13), fade(theme::ACCENT, alpha));
        widgets::icon(p, pos2(chip.min.x + 16.0, chip.center().y), icons::CUBE, 15.0, fade(theme::ON_ACCENT, alpha));
        p.galley(pos2(chip.min.x + 30.0, chip.center().y - galley.size().y / 2.0), galley, theme::ON_ACCENT);
    }
    if entry.official {
        widgets::icon(p, pos2(thumb.max.x - 22.0, thumb.min.y + 24.0), icons::SEAL_CHECK, 22.0, fade(theme::ACCENT, alpha));
    }
    // Its likes and how smoothly it runs, on the picture's foot.
    let likes = d.likes(entry);
    if likes > 0 {
        let liked = d.feedback.world(&world_key(&entry.world_id)).is_some_and(|w| w.liked);
        badge(p, pos2(thumb.min.x + 10.0, thumb.max.y - 34.0), false, icons::HEART, true, &short_count(likes as u64), if liked { theme::REC } else { theme::FG_2 }, alpha);
    }
    if let Some(perf) = d.performance(entry) {
        let (_, tint) = verdict(perf);
        badge(p, pos2(thumb.max.x - 10.0, thumb.max.y - 34.0), true, icons::GAUGE, false, &format!("{} fps", perf.fps), tint, alpha);
    } else if let Some(n) = entry.occupants.filter(|n| *n > 0) {
        badge(p, pos2(thumb.max.x - 10.0, thumb.max.y - 34.0), true, icons::USERS, false, &short_count(n as u64), theme::FG_2, alpha);
    }
    text_cut(p, pos2(rect.min.x + 16.0, rect.min.y + CARD_THUMB + 9.0), rect.width() - 32.0, Txt::new(entry.name.clone(), 19.0).weight(weight::SEMIBOLD).color(fade(theme::FG, alpha)));
    // Its author, and the labels people gave its world most.
    let mut meta: Vec<String> = d.author(entry).map(str::to_string).into_iter().collect();
    let wf = d.world_feedback(entry);
    meta.extend(wf.top().iter().take(2).map(|l| label_name(&l.id).to_string()));
    if meta.is_empty() {
        meta.push(match entry.visited {
            _ if entry.capture => format!("{} splats", short_count(entry.splats as u64)),
            Some((_, 1)) => "Visited once".to_string(),
            Some((_, n)) => format!("Visited {n} times"),
            None => "No 3D capture yet".to_string(),
        });
    }
    text_cut(p, pos2(rect.min.x + 16.0, rect.min.y + CARD_THUMB + 36.0), rect.width() - 32.0, Txt::new(meta.join(" · "), 15.0).color(fade(theme::FG_3, alpha)));
}

#[allow(clippy::too_many_arguments)]
fn compact_card(p: &Painter, rect: Rect, entry: &Entry, d: &Data, hover: bool, pressed: bool, alpha: f32) {
    let status = d.status;
    let rect = if pressed { rect.shrink(3.0) } else { rect };
    p.rect_filled(rect, CornerRadius::same(18), fade(if hover { RAISED } else { SURFACE }, alpha));
    let thumb = compact_thumb(rect);
    picture(p, thumb, (d.thumb)(entry), &entry.world_id, CornerRadius::same(10), alpha);
    let x = thumb.max.x + 14.0;
    let width = rect.max.x - x - 12.0;
    let mut name = Txt::new(entry.name.clone(), 17.0).weight(weight::SEMIBOLD).color(fade(theme::FG, alpha)).job(width);
    name.wrap.max_rows = 2;
    p.galley(pos2(x, rect.min.y + 14.0), p.layout_job(name), theme::FG);
    if let Some(a) = d.author(entry) {
        text_cut(p, pos2(x, rect.min.y + 62.0), width, Txt::new(a.to_string(), 15.0).color(fade(theme::FG_3, alpha)));
    }
    if status.current == Some(entry.id) {
        p.circle_filled(pos2(x + 6.0, rect.min.y + 88.0), 5.0, fade(if status.shown { theme::ACCENT } else { theme::WARN }, alpha));
        text(p, pos2(x + 18.0, rect.min.y + 88.0), Align2::LEFT_CENTER, Txt::new(if status.shown { "Around you" } else { "Loading" }, 14.0).color(fade(theme::FG_2, alpha)));
    } else {
        let mut at = x;
        if entry.capture {
            widgets::icon(p, pos2(at + 7.0, rect.min.y + 88.0), icons::CUBE, 15.0, fade(theme::ACCENT, alpha));
            text(p, pos2(at + 20.0, rect.min.y + 88.0), Align2::LEFT_CENTER, Txt::new("3D", 14.0).weight(weight::SEMIBOLD).color(fade(theme::ACCENT, alpha)));
            at += 50.0;
        }
        if d.likes(entry) > 0 {
            widgets::icon_fill(p, pos2(at + 7.0, rect.min.y + 88.0), icons::HEART, 14.0, fade(theme::FG_3, alpha));
            text(p, pos2(at + 20.0, rect.min.y + 88.0), Align2::LEFT_CENTER, Txt::new(short_count(d.likes(entry) as u64), 14.0).color(fade(theme::FG_3, alpha)));
        }
    }
}

/// The small pill under N4N0's menu that opens the panel, `size` pixels (the bar takes its place while a capture
/// shows or loads).
pub const CHIP_PX: [u32; 2] = [448, 72];

pub fn draw_chip(p: &Painter, status: &Status, name: Option<&str>, hover: bool, pressed: bool, time: f64) {
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(CHIP_PX[0] as f32, CHIP_PX[1] as f32)).shrink(4.0);
    let rect = if pressed { rect.shrink(2.0) } else { rect };
    p.rect_filled(rect, CornerRadius::same((rect.height() / 2.0) as u8), if hover { RAISED } else { theme::INK_850 });
    let showing = status.current.is_some();
    let (glyph, label, tint) = match name {
        Some(n) if showing => {
            let n: String = n.chars().take(24).collect();
            (icons::CUBE_FOCUS, if status.shown { n } else { format!("Loading {n}") }, theme::ACCENT)
        }
        _ => (icons::CUBE, "3D worlds".to_string(), theme::FG),
    };
    let icon_x = rect.min.x + 38.0;
    if showing && !status.shown {
        loading_ring(p, pos2(icon_x, rect.center().y), time);
    } else {
        widgets::icon(p, pos2(icon_x, rect.center().y), glyph, 26.0, tint);
    }
    text_cut(p, pos2(rect.min.x + 64.0, rect.center().y - 13.0), rect.width() - 100.0, Txt::new(label, 21.0).weight(weight::SEMIBOLD).color(theme::FG));
    widgets::icon(p, pos2(rect.max.x - 30.0, rect.center().y), icons::CARET_RIGHT, 20.0, theme::FG_3);
}

/// A turning ring while a capture loads.
fn loading_ring(p: &Painter, c: Pos2, time: f64) {
    let a = (time * 5.0) as f32;
    let points: Vec<Pos2> = (0..=16).map(|i| a + i as f32 * 0.28).map(|t| c + vec2(t.cos(), t.sin()) * 15.0).collect();
    p.add(egui::Shape::line(points, Stroke::new(3.0, theme::WARN)));
}

/// Premultiplied RGBA to straight alpha.
fn straight(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .flat_map(|p| {
            let a = p[3] as f32 / 255.0;
            if a <= 0.0 {
                [0, 0, 0, 0]
            } else {
                [(p[0] as f32 / a).min(255.0) as u8, (p[1] as f32 / a).min(255.0) as u8, (p[2] as f32 / a).min(255.0) as u8, p[3]]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"{"previews":[
        {"slot":0,"key":1,"id":1,"worldId":"wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd","name":"Home","author":"vrchat","uploader":"a","official":true,"view":{"position":[0,1,0],"target":[0,1,1]}},
        {"slot":1,"key":2,"id":2,"worldId":"wrld_00000000-0000-4000-8000-000000000002","name":"Dawn","author":"Mei","uploader":"b","view":{"position":[0,1,0],"target":[0,1,1]}},
        {"slot":2,"key":3,"id":3,"worldId":"wrld_00000000-0000-4000-8000-000000000003","name":"Shrine","author":"Kuro","uploader":"c","view":{"position":[0,1,0],"target":[0,1,1]}},
        {"slot":3,"key":4,"id":4,"worldId":"wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd","name":"Home","author":"vrchat","uploader":"d","view":{"position":[0,1,0],"target":[0,1,1]}}
    ],"feedback":{
        "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd":{"likes":5,"labels":{"mirror":2}},
        "wrld_00000000-0000-4000-8000-000000000002":{"likes":9,"labels":{"nature":3,"mirror":1}}
    }}"#;
    const HOME: &str = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd";

    fn data<'a>(entries: &'a [Entry], status: &'a Status, feedback: &'a FeedbackView, facts: &'a HashMap<String, Facts>) -> Data<'a> {
        Data { entries, picked: None, status, feedback, facts, thumb: &|_| None, picture: &|_, _| None, now: 0 }
    }

    fn read_home(liked: bool, mine: &[&str]) -> FeedbackView {
        FeedbackView {
            signed_in: true,
            subjects: Vec::new(),
            worlds: vec![WorldFeedback {
                world: HOME.into(),
                likes: 6,
                liked,
                labels: vec![LabelCount { id: "mirror".into(), count: 3, mine: mine.contains(&"mirror") }],
                comments: vec![CommentRow { id: 12, by: "You".into(), text: "mine".into(), at: 0, mine: true }],
                comment_count: 1,
                ..Default::default()
            }],
        }
    }

    #[test]
    fn results_are_ordered_and_kept_to_the_labels_picked() {
        let entries = super::super::catalog::parse(LISTING).unwrap();
        let (status, none, mut facts) = (Status::default(), FeedbackView::default(), HashMap::new());
        let ids = |panel: &Panel, facts: &HashMap<String, Facts>, feedback: &FeedbackView| {
            panel.results(&data(&entries, &status, feedback, facts)).iter().map(|e| e.id).collect::<Vec<_>>()
        };
        let mut panel = Panel::default();
        assert_eq!(ids(&panel, &facts, &none), [1, 2, 3, 4], "newest first, as listed");
        panel.sort = Sort::Liked;
        assert_eq!(ids(&panel, &facts, &none), [2, 1, 4, 3]);
        // The member's own like counts once the app read the world.
        let liked = FeedbackView { worlds: vec![WorldFeedback { world: HOME.into(), likes: 12, ..Default::default() }], ..Default::default() };
        assert_eq!(ids(&panel, &facts, &liked), [1, 4, 2, 3]);
        panel.labels = vec!["mirror".into()];
        assert_eq!(ids(&panel, &facts, &none), [2, 1, 4]);
        panel.labels.push("nature".into());
        assert_eq!(ids(&panel, &facts, &none), [2], "every label picked");
        panel.labels.clear();
        panel.official = true;
        assert_eq!(ids(&panel, &facts, &none), [1]);
        panel.official = false;
        // Smoothest: by the share of the refresh rate, then frames a second; the unmeasured last, as listed.
        let perf = |fps, ratio| Facts { performance: Some(WorldPerformance { fps, ratio, ..Default::default() }), ..Default::default() };
        facts.insert("wrld_00000000-0000-4000-8000-000000000003".into(), perf(70, 0.9));
        facts.insert(HOME.into(), perf(80, 0.9));
        panel.sort = Sort::Smooth;
        assert_eq!(ids(&panel, &facts, &none), [1, 4, 3, 2]);
        panel.query = "shr".into();
        assert_eq!(ids(&panel, &facts, &none), [3]);
    }

    #[test]
    fn feedback_is_given_once_read_and_signed_in() {
        let entries = super::super::catalog::parse(LISTING).unwrap();
        let (status, facts) = (Status::default(), HashMap::new());
        let mut panel = Panel { page: Page::Detail, selected: Some(1), ..Default::default() };
        // Signed out: nothing sent, the reason said.
        let out = FeedbackView::default();
        assert!(panel.press(Target::Like, &data(&entries, &status, &out, &facts), 1.0).is_empty());
        assert_eq!(panel.note(1.0), Some(contract::Refusal::SignIn.text()));
        // Not read yet: what the member gave is unknown, so nothing is sent either.
        let unread = FeedbackView { signed_in: true, ..Default::default() };
        let d = data(&entries, &status, &unread, &facts);
        assert_eq!(d.world_feedback(&entries[0]).likes, 5, "the listing's count meanwhile");
        assert!(panel.press(Target::Like, &d, 2.0).is_empty());
        // Read: Like sets the opposite of what the member has, a label too.
        let read = read_home(false, &["mirror"]);
        let d = data(&entries, &status, &read, &facts);
        assert_eq!(panel.press(Target::Like, &d, 3.0), [Action::Feedback(FeedbackAsk::Like { world: HOME.into(), on: true })]);
        let mirror = contract::labels().iter().position(|l| l.id == "mirror").unwrap() as u8;
        assert_eq!(
            panel.press(Target::Label(mirror), &d, 3.0),
            [Action::Feedback(FeedbackAsk::Label { world: HOME.into(), label: "mirror".into(), on: false, text: None })]
        );
        // The label that needs a reason asks for it first, then sends it with the label.
        let ai = contract::labels().iter().position(|l| l.justify.is_some()).unwrap() as u8;
        assert!(panel.press(Target::Label(ai), &d, 4.0).is_empty());
        assert_eq!(panel.page, Page::Compose(Compose::Reason));
        panel.draft = "too short".into();
        assert!(panel.press(Target::Done, &d, 5.0).is_empty());
        assert_eq!(panel.note(5.0), Some(contract::Refusal::JustificationTooShort.text()));
        panel.draft = "The thumbnail and every texture look generated, the signs are unreadable".into();
        let sent = panel.press(Target::Done, &d, 6.0);
        assert!(matches!(&sent[..], [Action::Feedback(FeedbackAsk::Label { label, on: true, text: Some(_), .. })] if label == "ai-world"), "{sent:?}");
        assert_eq!(panel.page, Page::Labels);
        // A comment: written, sent as the contract cleans it.
        panel.page = Page::Comments;
        assert!(panel.press(Target::WriteComment, &d, 7.0).is_empty());
        assert_eq!(panel.page, Page::Compose(Compose::Comment));
        panel.type_text("  Lovely   at sunset ");
        let sent = panel.press(Target::Done, &d, 8.0);
        assert!(matches!(&sent[..], [Action::Feedback(FeedbackAsk::Comment { world, text })] if world == HOME && text.trim() == "Lovely at sunset"), "{sent:?}");
        // Delete asks for a second press within a few seconds.
        assert_eq!(panel.page, Page::Comments);
        assert!(panel.targets(&d).iter().any(|(t, _)| *t == Target::DeleteComment(12)));
        assert!(panel.press(Target::DeleteComment(12), &d, 9.0).is_empty());
        assert!(panel.press(Target::DeleteComment(12), &d, 9.0 + CONFIRM_FOR + 1.0).is_empty(), "too late: asks again");
        assert_eq!(
            panel.press(Target::DeleteComment(12), &d, 9.0 + CONFIRM_FOR + 2.0),
            [Action::Feedback(FeedbackAsk::Uncomment { world: HOME.into(), comment: 12 })]
        );
    }

    #[test]
    fn a_world_without_a_capture_opens_its_page_to_join_it() {
        let captures = super::super::catalog::parse(LISTING).unwrap();
        let cat = "wrld_00000000-0000-4000-8000-00000000ca70";
        let found = [orbiters_core::maps::FoundWorld {
            world: orbiters_core::maps::World { world_id: cat.into(), name: "Black Cat Bar".into(), ..Default::default() },
            occupants: Some(12),
            ..Default::default()
        }];
        let first: Vec<&Entry> = captures.iter().filter(|e| e.matches("ho")).collect();
        let entries = super::super::finder::merge(&captures, &first, &found, Some("ho"), &|_| Default::default());
        assert_eq!(entries.iter().map(|e| e.id).collect::<Vec<_>>()[..2], [1, 4], "the search's captures first, both of Home");
        let world = entries.last().unwrap().clone();
        let (feedback, facts) = (FeedbackView::default(), HashMap::new());
        let searched = Status { list: ListStatus { searching: true, more: true, ..Default::default() }, ..Default::default() };
        let d = data(&entries, &searched, &feedback, &facts);
        let mut panel = Panel { query: "ho".into(), ..Default::default() };
        // Searching: 3D only instead of the sections, and a page more while the list has more.
        let targets: Vec<Target> = panel.targets(&d).into_iter().map(|(t, _)| t).collect();
        assert!(targets.contains(&Target::Has3d) && !targets.contains(&Target::Section(Section::Popular)));
        assert!(targets.contains(&Target::Next), "3 results and more to read: two pages");
        assert_eq!(panel.results(&d).len(), 3, "found by the search, whatever its name");
        panel.press(Target::Has3d, &d, 1.0);
        assert!(panel.results(&d).iter().all(|e| e.capture));
        panel.press(Target::Has3d, &d, 1.0);
        // Its card opens its page, nothing to show around the person.
        let i = panel.results(&d).iter().position(|e| e.id == world.id).unwrap();
        assert!(panel.press(Target::Card(i), &d, 2.0).is_empty());
        assert_eq!((panel.page, panel.selected), (Page::Detail, Some(world.id)));
        let targets: Vec<Target> = panel.targets(&d).into_iter().map(|(t, _)| t).collect();
        assert!(targets.contains(&Target::Join) && targets.contains(&Target::OpenWeb) && !targets.contains(&Target::Show), "{targets:?}");
        assert!(targets.contains(&Target::Like), "its feedback is given as a capture's");
        assert_eq!(panel.press(Target::Join, &d, 3.0), [Action::Join(cat.into())]);
        assert_eq!(panel.press(Target::OpenWeb, &d, 3.0), [Action::OpenWeb(cat.into())]);
        assert!(panel.press(Target::Show, &d, 3.0).is_empty());
        // Another list shows: its page stays, kept by the host.
        let elsewhere = data(&captures, &searched, &feedback, &facts);
        assert!(panel.entry(&elsewhere).is_none());
        let kept = Data { picked: Some(&world), ..data(&captures, &searched, &feedback, &facts) };
        assert_eq!(panel.entry(&kept).map(|e| e.id), Some(world.id));
        // Not searching: the sections; Done searches at once.
        let browsing = Status::default();
        let mut panel = Panel { page: Page::Typing, ..Default::default() };
        let d = data(&captures, &browsing, &feedback, &facts);
        assert_eq!(panel.press(Target::Done, &d, 4.0), [Action::Search]);
        let targets: Vec<Target> = panel.targets(&d).into_iter().map(|(t, _)| t).collect();
        assert!(Section::ALL.iter().all(|s| targets.contains(&Target::Section(*s))) && !targets.contains(&Target::Has3d));
        panel.results_page = 1;
        panel.press(Target::Section(Section::Visited), &d, 5.0);
        assert_eq!((panel.section, panel.results_page), (Section::Visited, 0));
        assert!(targets.contains(&Target::Sort(Sort::Liked)), "the order chip steps to the next order");
    }

    #[test]
    fn a_capture_page_has_no_capture_controls_and_steps_through_its_world_captures() {
        let entries = super::super::catalog::parse(LISTING).unwrap();
        let (feedback, facts) = (read_home(true, &[]), HashMap::new());
        let shown = Status { current: Some(1), shown: true, ..Default::default() };
        let mut panel = Panel { page: Page::Detail, selected: Some(1), ..Default::default() };
        let d = data(&entries, &shown, &feedback, &facts);
        let targets: Vec<Target> = panel.targets(&d).into_iter().map(|(t, _)| t).collect();
        for t in [Target::Hide, Target::Recenter, Target::SeeThrough, Target::Quality(Quality::High), Target::Show] {
            assert!(!targets.contains(&t), "{t:?} is on the bar, not the page");
        }
        assert!(targets.contains(&Target::Like) && targets.contains(&Target::Labels) && targets.contains(&Target::Other));
        // Another capture of the same world: its page, shown.
        assert_eq!(panel.press(Target::Other, &d, 1.0), [Action::Show(4)]);
        assert_eq!(panel.selected, Some(4));
        let targets: Vec<Target> = panel.targets(&d).into_iter().map(|(t, _)| t).collect();
        assert!(targets.contains(&Target::Show), "not the one around you: Show around me");
        assert_eq!(panel.press(Target::Other, &d, 2.0), Vec::<Action>::new(), "back to the one shown: nothing to download");
        assert_eq!(panel.selected, Some(1));
    }
}
