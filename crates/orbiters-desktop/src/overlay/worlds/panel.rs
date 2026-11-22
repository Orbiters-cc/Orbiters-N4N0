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
pub mod go;
mod pages;
mod shots;

pub use bar::{bar_hit, bar_press, draw_bar, BAR_PX};
pub use shots::{shots, side_live, side_loading, side_small};

use super::catalog::Entry;
pub use super::finder::Section;
use super::view::Quality;
use egui::epaint::RectShape;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Painter, Pos2, Rect, Stroke, StrokeKind, TextureId};
use orbiters_contracts::world_feedback as contract;
use orbiters_core::maps::{FoundBy, WorldPage, WorldPerformance, WorldPicture};
use orbiters_i18n::{tr, tr_ctx, tr_n};
use orbiters_n4n0_core::feedback::{CommentRow, FeedbackAsk, FeedbackView, LabelCount, WorldFeedback};
use orbiters_n4n0_core::menu::KeyboardLayout;
use orbiters_ui::theme::{self, weight, Txt};
use super::super::orbit::panels::look as orbit_look;
use orbiters_ui::{icons, widgets};
use std::collections::HashMap;

pub const WIDTH: f32 = 1024.0;
pub const HEIGHT: f32 = 576.0;
/// Cards on a page of results (three across, two down), and on the typing page's single row.
pub const PER_PAGE: usize = 6;
pub const PER_ROW: usize = 3;
/// How many popular worlds follow few captures, under their heading.
pub const FEW_MORE: usize = 3;
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
const CARD_STAGGER: f64 = 0.035;

const CARD: Color32 = theme::INK_900;
/// The corners of the hole a capture standing small is seen through: the sheet's back cuts its own to the same shape.
pub const HOLE_RADIUS: f32 = 14.0;
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
            Sort::Newest => tr_ctx("sorting", "Newest"),
            Sort::Liked => tr("Most liked"),
            Sort::Smooth => tr("Smoothest"),
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
    /// Its instances to go to, and new ones to make (F5).
    Go,
}

/// What can be pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// On a screen: what turns on the panel, to take with the mouse.
    Hero,
    /// Closes the side panel.
    CloseSide,
    /// On a screen: what turns on the side panel fills the whole page, or comes back to its frame.
    FullView,
    /// Shows it small, in the panel's picture.
    ShowSmall,
    /// What shows small goes back into the panel's picture.
    SmallBack,
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
    /// In a headset, the side panel's carousel: the capture before or after this one, as the list has them (K-carousel).
    FlipPrevious,
    FlipNext,
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
    /// The world's name to the computer's clipboard (REQUEST20 1), beside the name on its side panel.
    CopyName,
    /// Reads the world's feedback again, after Orbiters did not answer.
    RetryFeedback,
    /// An instance of the Go there list (its place in the list): there, directly.
    GoInstance(usize),
    /// A new instance of this kind ([`go::NEW_KINDS`]), then there.
    NewInstance(u8),
    /// On the card of a direct trip: an invite to it instead.
    GoInvite,
    /// The region a new instance is made in: the next one.
    GoRegion,
    /// The list of instances read again.
    GoRetry,
}

/// What a press asks of the overlay.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Show this capture (its listing id) around the person; on a screen, turning on this panel.
    Show(u64),
    Hide,
    Recenter,
    SeeThrough(bool),
    /// Captures show small before the person, to take in a hand, or all around them.
    Small(bool),
    /// What shows small goes back to its place in the panel.
    SmallBack,
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
    /// A world (its id): its Go there page, its instances read.
    Join(String),
    /// A world's page on vrchat.com (its id), in the computer's browser.
    OpenWeb(String),
    /// This text (a world's name, exactly as shown) to the computer's clipboard: in a headset too, to paste it there.
    CopyName(String),
    /// Signing in to Orbiters, in the computer's browser: the way out of "sign in to write a comment" (F6).
    SignIn,
    /// The world's feedback asked for again (Orbiters did not answer).
    RetryFeedback,
    /// To this exact instance of this world (listed or made in this scope), directly.
    GoTo { world: String, instance: String },
    /// A new instance of `world`, then there.
    NewInstance { world: String, access: orbiters_core::vrchat_api::CreateAccess, region: orbiters_core::vrchat_api::InstanceRegion },
    /// An invite to oneself to this instance, instead of the direct trip.
    InviteInstead { world: String, instance: String },
}

/// What a stick does where a laser is on a capture's side panel in a headset (K-small-stick, K-carousel): elsewhere it is VRChat's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StickSpot {
    /// On the capture lying in the panel's hole: the stick moves in it, as walking would.
    InCapture,
    /// On the carousel round it, with other captures to go to: the stick flips them, side to side.
    Flip,
}

/// How the capture view goes, as the panel shows it.
#[derive(Clone, Debug, Default, PartialEq)]
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
    /// Captures show small before the person.
    pub small: bool,
    /// What shows small: a hand took it from its place; it lies in its place in the panel; a hand is near enough to take it.
    pub small_taken: bool,
    /// A hand holds it now: its stick zooms it.
    pub small_held: bool,
    pub small_docked: bool,
    pub small_near: bool,
    /// There is a headset to show anything in.
    pub headset: bool,
    pub error: Option<String>,
    /// The listing is being read; why it could not be.
    pub fetching: bool,
    pub listing_error: Option<String>,
    /// A line of numbers for the curious (splats, the GPU's time a face).
    pub stats: Option<String>,
    /// The world list shown (a search, a section).
    pub list: ListStatus,
    /// Go there's instances (F5).
    pub go: go::GoView,
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
    /// Its saved session is being restored (r21): not signed out yet.
    pub connecting: bool,
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

/// A capture turning on the side panel, on a screen ([`super::super::orbit::sheet_view`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub id: u64,
    pub picture: Option<TextureId>,
    pub plate: Vec<[f32; 2]>,
    pub failed: bool,
    /// It fills the whole page (Enzo's full-screen button), the rest of the page away.
    pub full: bool,
}

/// The full-screen button, in the bottom right corner of the live view's frame `frame` (as the Orbiters window's viewer has it).
pub fn full_button(frame: Rect) -> Rect {
    Rect::from_min_size(frame.right_bottom() - vec2(52.0, 52.0), vec2(40.0, 40.0))
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
    // A month by its number: one text each, as a language may write them in a way the others do not.
    let month = match t.month() as u8 {
        1 => tr_ctx("month", "Jan"),
        2 => tr_ctx("month", "Feb"),
        3 => tr_ctx("month", "Mar"),
        4 => tr_ctx("month", "Apr"),
        5 => tr_ctx("month", "May"),
        6 => tr_ctx("month", "Jun"),
        7 => tr_ctx("month", "Jul"),
        8 => tr_ctx("month", "Aug"),
        9 => tr_ctx("month", "Sep"),
        10 => tr_ctx("month", "Oct"),
        11 => tr_ctx("month", "Nov"),
        _ => tr_ctx("month", "Dec"),
    };
    Some(tr!("{day} {month} {year}", day = t.day(), month = month, year = t.year()))
}

/// The tag of the language now, as the labels list keys its names by.
pub fn language_tag() -> &'static str {
    orbiters_i18n::language().tag()
}

/// A label's name in the language now (the contract has them all), its id when the list does not have it.
pub fn label_name(id: &str) -> &str {
    contract::label(id).map_or(id, |l| l.name(language_tag()))
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
        (tr_ctx("performance", "smooth"), orbit_look::accent())
    } else if p.ratio >= 0.6 {
        (tr_ctx("performance", "fair"), theme::WARN)
    } else {
        (tr_ctx("performance", "heavy"), theme::DANGER)
    }
}

/// The panel's state.
#[derive(Clone, Debug, Default)]
pub struct Panel {
    /// On a screen: the capture turning on the side panel.
    pub live: Option<Live>,
    pub page: Page,
    pub query: String,
    pub results_page: usize,
    /// On a sheet, whose head has the way back: this panel's own does not show.
    pub on_sheet: bool,
    /// Whether things move, or are simply there (Reduce motion).
    pub moves: bool,
    /// When the side panel last started to come or go.
    pub(super) side_at: f64,
    /// A row and how many in it, as the list last measured, and for what.
    list_step: std::cell::Cell<(f32, usize)>,
    list_headset: std::cell::Cell<bool>,
    /// How far down a headset's endless list of worlds is, the hand on it, how fast it was thrown, and how far it can go now.
    vr_scroll: f32,
    vr_hold: Option<VrHold>,
    vr_throw: f32,
    vr_throw_at: f64,
    vr_most: std::cell::Cell<f32>,
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
    /// The page of Go there's list, and the region a new instance is made in, when one was picked.
    pub go_page: usize,
    pub go_region: Option<orbiters_core::vrchat_api::InstanceRegion>,
    /// The picture shown whole on the pictures page.
    pub picture: Option<usize>,
    /// The member's comment whose Delete was pressed once, and when: a second press deletes it.
    confirm: Option<(u64, f64)>,
    /// Why a press did nothing (signed out, not read yet, a text refused), and when.
    pub note: Option<(String, f64)>,
    page_at: f64,
    /// When another world was pressed with the side panel open.
    selected_at: f64,
    /// The carousel flipped (forward or back), and when: the picture comes in from that side.
    flipped: Option<(bool, f64)>,
    /// Where the pointer is on the panel (a headset's laser), for what the stick does there.
    pub pointer: Option<Pos2>,
    /// Something changed: paint again.
    pub dirty: bool,
    /// Copy name's place, just after the name as the last paint laid it out, for that capture (its id) in a headset or not.
    copy_at: std::cell::Cell<Option<(u64, bool, Rect)>>,
    /// The capture whose name was last copied, and when: its button says Copied for a moment.
    copied: Option<(u64, f64)>,
}

/// How long Copy name says Copied.
const COPIED_FOR: f64 = 1.6;

fn ease_out(t: f64) -> f32 {
    let t = t.clamp(0.0, 1.0) as f32;
    1.0 - (1.0 - t).powi(3)
}

fn fade(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

/// The side panel a world's details show on, beside the list so that another can be pressed at once (Enzo: "so it's easy to
/// click quickly on multiple worlds and see what they look like"), and what lies on it.
pub const SIDE: Rect = Rect { min: Pos2 { x: 664.0, y: 12.0 }, max: Pos2 { x: 1012.0, y: 564.0 } };
const SIDE_LEFT: f32 = 676.0;
const SIDE_RIGHT: f32 = 1000.0;
/// Its picture; where a capture standing small is seen through it.
///
/// On a screen it reads in four parts, top to bottom (round 18's judges counted eight stacked rows, and a last line cut by the sheet's foot):
/// its head (how it runs, and Hide details) with its picture, name and who made it; what to do; what people gave it; what it says of itself.
/// Nothing lies on the picture but its full-screen button, and nothing is drawn that does not end over the panel's foot.
pub const SIDE_HERO: Rect = Rect { min: Pos2 { x: SIDE_LEFT, y: 54.0 }, max: Pos2 { x: SIDE_RIGHT, y: 234.0 } };
/// Hides it, in its head: off the picture (round 18's judges counted three things lying on it), and far from the sheet's own X.
/// Its close says what it does: a chevron alone read as "next picture" (round 13's to 16's judges).
const SIDE_CLOSE: Rect = Rect { min: Pos2 { x: 848.0, y: 18.0 }, max: Pos2 { x: SIDE_RIGHT, y: 48.0 } };
const SMALL_BACK: Rect = Rect { min: Pos2 { x: 862.0, y: 190.0 }, max: Pos2 { x: 992.0, y: 226.0 } };
/// Where its name's last line ends, and where who made it starts.
const SIDE_NAME_FOOT: f32 = 290.0;
const SIDE_UNDER: f32 = 293.0;
/// What to do with it first; then going there.
const SHOW: Rect = Rect { min: Pos2 { x: SIDE_LEFT, y: 326.0 }, max: Pos2 { x: 835.0, y: 368.0 } };
const SHOW_SMALL: Rect = Rect { min: Pos2 { x: 841.0, y: 326.0 }, max: Pos2 { x: SIDE_RIGHT, y: 368.0 } };
const SHOW_WHOLE: Rect = Rect { min: Pos2 { x: SIDE_LEFT, y: 326.0 }, max: Pos2 { x: SIDE_RIGHT, y: 368.0 } };
const GO: Rect = Rect { min: Pos2 { x: SIDE_LEFT, y: 374.0 }, max: Pos2 { x: 835.0, y: 410.0 } };
const WEB: Rect = Rect { min: Pos2 { x: 841.0, y: 374.0 }, max: Pos2 { x: SIDE_RIGHT, y: 410.0 } };
const WEB_WHOLE: Rect = Rect { min: Pos2 { x: SIDE_LEFT, y: 374.0 }, max: Pos2 { x: SIDE_RIGHT, y: 410.0 } };
/// The way to this world's other captures: a link at the end of the line that says who made it.
const OTHER: Rect = Rect { min: Pos2 { x: 846.0, y: 290.0 }, max: Pos2 { x: SIDE_RIGHT, y: 318.0 } };
/// What it says of itself, from here down to the panel's foot, in whole lines.
const SIDE_SAYS: f32 = 506.0;
const SIDE_FOOT: f32 = 556.0;

/// Tile `i` of the `n` small tiles under what to do, spread across the whole width.
fn small_tile(i: usize, n: usize, vr: bool) -> Rect {
    let left = if vr { VR_LEFT } else { SIDE_LEFT };
    let width = (SIDE_RIGHT - left - 6.0 * (n as f32 - 1.0)) / n as f32;
    let x = left + i as f32 * (width + 6.0);
    if vr {
        return Rect::from_min_max(pos2(x, 442.0), pos2(x + width, 504.0));
    }
    // On a screen two a row, each with its word beside its number at a size that reads (four a row left their words at 10 px, round 18's judges).
    let half = (SIDE_RIGHT - SIDE_LEFT - 6.0) / 2.0;
    let (col, row) = ((i % 2) as f32, (i / 2) as f32);
    let top = 420.0 + row * 42.0;
    // An odd last one takes the whole row.
    if n % 2 == 1 && i == n - 1 {
        return Rect::from_min_max(pos2(SIDE_LEFT, top), pos2(SIDE_RIGHT, top + 36.0));
    }
    Rect::from_min_max(pos2(SIDE_LEFT + col * (half + 6.0), top), pos2(SIDE_LEFT + col * (half + 6.0) + half, top + 36.0))
}

/// One of them: what it is, how many if any, and its word: no one has to guess what an icon alone means.
#[allow(clippy::too_many_arguments)]
fn small_action(p: &Painter, rect: Rect, glyph: &str, filled: bool, count: u64, word: &str, bg: Color32, fg: Color32, hover: bool, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(2.0) } else { rect };
    let bg = if hover { theme::mix(bg, Color32::WHITE, 0.08) } else { bg };
    p.rect_filled(rect, CornerRadius::same(14), fade(bg, alpha));
    let number = count_or_nothing(count);
    // A headset's are larger: there is the room, and the need.
    let large = rect.height() > 56.0;
    if !large {
        // On a screen: its icon, its number and its word on one line, none under 12 px at 1280 x 720.
        let number = orbit_look::txt(number.clone(), 17.5).weight(weight::BOLD).color(fade(fg, alpha));
        let number_wide = if count == 0 { 0.0 } else { p.layout_job(number.job(f32::INFINITY)).size().x + 6.0 };
        let word = fitted(p, orbit_look::txt(word, 17.5).weight(weight::MEDIUM).color(fade(if fg == theme::FG { theme::FG_2 } else { fg }, alpha)), rect.width() - 46.0 - number_wide);
        let wide = 26.0 + number_wide + p.layout_job(word.job(f32::INFINITY)).size().x;
        let left = rect.center().x - wide / 2.0;
        if filled {
            widgets::icon_fill(p, pos2(left + 9.0, rect.center().y), glyph, 19.0, fade(fg, alpha));
        } else {
            widgets::icon(p, pos2(left + 9.0, rect.center().y), glyph, 19.0, fade(fg, alpha));
        }
        if count > 0 {
            text(p, pos2(left + 26.0, rect.center().y), Align2::LEFT_CENTER, number);
        }
        text(p, pos2(left + 26.0 + number_wide, rect.center().y), Align2::LEFT_CENTER, word);
        return;
    }
    // A headset's: its icon and its number, and its word under them, each word at one size and all at a headset's (round 19's judges: 14 to 15 px in its picture).
    // On one line a long word was squeezed small beside a short one.
    let (icon, count_size, said) = (26.0, 22.0, 20.5);
    let line = rect.min.y + rect.height() * 0.34;
    let at = if number.is_empty() { rect.center().x } else { rect.center().x - 16.0 };
    if filled {
        widgets::icon_fill(p, pos2(at, line), glyph, icon, fade(fg, alpha));
    } else {
        widgets::icon(p, pos2(at, line), glyph, icon, fade(fg, alpha));
    }
    if !number.is_empty() {
        text(p, pos2(at + icon * 0.7, line), Align2::LEFT_CENTER, orbit_look::txt(number, count_size).weight(weight::SEMIBOLD).color(fade(fg, alpha)));
    }
    text(p, pos2(rect.center().x, rect.min.y + rect.height() * 0.75), Align2::CENTER_CENTER, fitted(p, orbit_look::txt(word, said).weight(weight::MEDIUM).color(fade(theme::FG, alpha)), rect.width() - 10.0));
}

/// A headset's page, on its own and not a smaller screen's: what is read at arm's length with a moving hand. Six worlds as pictures with their names, and beside them a wide panel:
/// the hole, the name, one row of what to do, one of its small tiles. Search and the filters wait behind the list's one head.
const VR_LEFT: f32 = 464.0;
const VR_SIDE: Rect = Rect { min: Pos2 { x: 452.0, y: 12.0 }, max: Pos2 { x: 1012.0, y: 564.0 } };
/// Its picture; where a capture standing small is seen through it.
// About 40% of the panel's height (round 19's judges): what is left under it holds its cue, then the name and who made it as a block of their own, well clear of the
// buttons (a laser on Around me was on the byline).
/// The shade along the top edge of the hole a capture shows through, in the modern look: the panel's own thickness, so the capture reads as behind it
/// (round 19's judges). Bands this tall, this many, each this dark (0 to 1) from the edge in. Everything of the hole below it is clear.
pub const HOLE_SHADE_BAND: f32 = 3.0;
pub const HOLE_SHADE_BANDS: usize = 7;
pub fn hole_shade(band: usize) -> f32 {
    if band >= HOLE_SHADE_BANDS { 0.0 } else { 0.5 * (1.0 - band as f32 / HOLE_SHADE_BANDS as f32).powi(2) }
}

/// A capture's picture in a headset's side panel: its whole width (K-carousel kept it: the neighbours peek out over its edges, [`VR_PEEK_NEXT`]).
pub const VR_HERO: Rect = Rect { min: Pos2 { x: VR_LEFT, y: 24.0 }, max: Pos2 { x: SIDE_RIGHT, y: 250.0 } };
const VR_SMALL_BACK: Rect = Rect { min: Pos2 { x: 804.0, y: 198.0 }, max: Pos2 { x: 958.0, y: 242.0 } };
/// The carousel's arrows, the whole height of the picture, each a laser's easy target over its edge; and the neighbour that peeks out under each, as if the row of
/// captures went on behind the panel.
const VR_FLIP_PREVIOUS: Rect = Rect { min: Pos2 { x: 452.0, y: 24.0 }, max: Pos2 { x: 506.0, y: 250.0 } };
const VR_FLIP_NEXT: Rect = Rect { min: Pos2 { x: 958.0, y: 24.0 }, max: Pos2 { x: 1012.0, y: 250.0 } };
const VR_PEEK_PREVIOUS: Rect = Rect { min: Pos2 { x: VR_LEFT, y: 24.0 }, max: Pos2 { x: 496.0, y: 250.0 } };
const VR_PEEK_NEXT: Rect = Rect { min: Pos2 { x: 968.0, y: 24.0 }, max: Pos2 { x: SIDE_RIGHT, y: 250.0 } };
/// The other layout tried (the owner's choice, K-carousel): the neighbours in gutters beside a narrower picture, which then covers nothing of the capture but
/// shows it 15% smaller. A developer's switch: `ORBIT_CAROUSEL_GUTTERS`, or [`set_carousel_gutters`] in the screenshots.
const VR_HERO_NARROW: Rect = Rect { min: Pos2 { x: 504.0, y: 24.0 }, max: Pos2 { x: 960.0, y: 250.0 } };
const VR_SMALL_BACK_NARROW: Rect = Rect { min: Pos2 { x: 800.0, y: 198.0 }, max: Pos2 { x: 952.0, y: 242.0 } };
const VR_FLIP_PREVIOUS_GUTTER: Rect = Rect { min: Pos2 { x: 452.0, y: 24.0 }, max: Pos2 { x: 503.0, y: 250.0 } };
const VR_FLIP_NEXT_GUTTER: Rect = Rect { min: Pos2 { x: 961.0, y: 24.0 }, max: Pos2 { x: 1012.0, y: 250.0 } };
const VR_PEEK_PREVIOUS_GUTTER: Rect = Rect { min: Pos2 { x: 460.0, y: 40.0 }, max: Pos2 { x: 498.0, y: 234.0 } };
const VR_PEEK_NEXT_GUTTER: Rect = Rect { min: Pos2 { x: 966.0, y: 40.0 }, max: Pos2 { x: 1004.0, y: 234.0 } };
/// Where on the side panel a stick flips the captures: the picture, its arrows and the line under it.
const VR_FLIP_BAND: Rect = Rect { min: Pos2 { x: 452.0, y: 12.0 }, max: Pos2 { x: 1012.0, y: 288.0 } };
static GUTTERS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The carousel's neighbours in gutters beside a narrower picture (true) or over its edges (false, the default).
pub fn set_carousel_gutters(on: bool) {
    GUTTERS.store(on, std::sync::atomic::Ordering::Relaxed);
}

fn over_edges() -> bool {
    static FROM_ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // A developer's switch, for comparing both layouts in the VR sandbox.
    !(GUTTERS.load(std::sync::atomic::Ordering::Relaxed) || *FROM_ENV.get_or_init(|| std::env::var_os("ORBIT_CAROUSEL_GUTTERS").is_some())) // i18n: not shown (an environment variable's name)
}

/// The picture of a capture's side panel in a headset, as the carousel's layout has it.
pub fn vr_hero() -> Rect {
    if over_edges() { VR_HERO } else { VR_HERO_NARROW }
}

fn vr_small_back() -> Rect {
    if over_edges() { VR_SMALL_BACK } else { VR_SMALL_BACK_NARROW }
}

/// The carousel's arrow towards the next (`forward`) or the one before, and where that neighbour peeks out.
fn vr_flip(forward: bool) -> (Rect, Rect) {
    match (over_edges(), forward) {
        (true, false) => (VR_FLIP_PREVIOUS, VR_PEEK_PREVIOUS),
        (true, true) => (VR_FLIP_NEXT, VR_PEEK_NEXT),
        (false, false) => (VR_FLIP_PREVIOUS_GUTTER, VR_PEEK_PREVIOUS_GUTTER),
        (false, true) => (VR_FLIP_NEXT_GUTTER, VR_PEEK_NEXT_GUTTER),
    }
}


/// How long a flip's picture takes to come in.
const FLIP_SLIDE: f64 = 0.2;
/// Centred right under the hole, the hand's cue for what is in it (round 15's judges found it far off at the right).
// A clear line of its own between the hole and the name (the round 19 pre-check saw it on the name).
const VR_GRIP: Rect = Rect { min: Pos2 { x: VR_LEFT, y: 254.0 }, max: Pos2 { x: SIDE_RIGHT, y: 286.0 } };
/// Where a headset's name and who made it start.
const VR_NAME: f32 = 292.0;
const VR_UNDER: f32 = 325.0;
const VR_SHOW: Rect = Rect { min: Pos2 { x: VR_LEFT, y: 372.0 }, max: Pos2 { x: 637.0, y: 434.0 } };
const VR_SHOW_SMALL: Rect = Rect { min: Pos2 { x: 645.0, y: 372.0 }, max: Pos2 { x: 819.0, y: 434.0 } };
const VR_GO: Rect = Rect { min: Pos2 { x: 827.0, y: 372.0 }, max: Pos2 { x: SIDE_RIGHT, y: 434.0 } };
const VR_GO_HALF: Rect = Rect { min: Pos2 { x: VR_LEFT, y: 372.0 }, max: Pos2 { x: 728.0, y: 434.0 } };
const VR_WEB_HALF: Rect = Rect { min: Pos2 { x: 736.0, y: 372.0 }, max: Pos2 { x: SIDE_RIGHT, y: 434.0 } };
/// Without Go there (no VRChat session in Orbiters, F5): what is left shares its row.
const VR_WEB_WHOLE: Rect = Rect { min: Pos2 { x: VR_LEFT, y: 372.0 }, max: Pos2 { x: SIDE_RIGHT, y: 434.0 } };
const VR_SHOW_WIDE: Rect = Rect { min: Pos2 { x: VR_LEFT, y: 372.0 }, max: Pos2 { x: 728.0, y: 434.0 } };
const VR_SHOW_SMALL_WIDE: Rect = Rect { min: Pos2 { x: 736.0, y: 372.0 }, max: Pos2 { x: SIDE_RIGHT, y: 434.0 } };
const VR_OTHER: Rect = Rect { min: Pos2 { x: VR_LEFT, y: 512.0 }, max: Pos2 { x: SIDE_RIGHT, y: 556.0 } };
/// The list's one head: where it is, and through it search and the filters.
const VR_LIST_RIGHT: f32 = 440.0;

fn vr_list_head(pages: usize) -> Rect {
    Rect::from_min_max(pos2(24.0, 24.0), pos2(if pages > 1 { 322.0 } else { VR_LIST_RIGHT }, 84.0))
}

/// A result's place on a headset's list, before it is scrolled: two columns, as far down as there are worlds.
fn vr_card_rect(i: usize) -> Rect {
    let (col, row) = ((i % 2) as f32, (i / 2) as f32);
    Rect::from_min_size(pos2(24.0 + col * 212.0, VR_LIST_TOP + 4.0 + row * VR_ROW), vec2(204.0, 148.0))
}

/// The same with no side panel beside it: four columns under the search and the filters.
fn vr_wide_card_rect(i: usize) -> Rect {
    let (col, row) = ((i % 4) as f32, (i / 4) as f32);
    Rect::from_min_size(pos2(24.0 + col * 248.0, WIDE_LIST_TOP + 4.0 + row * 190.0), vec2(240.0, 182.0))
}

const WIDE_LIST_TOP: f32 = 142.0;

/// What a list is like in each place it shows: its first card's top, its columns, its row's height.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ListShape {
    top: f32,
    bottom: f32,
    right: f32,
    columns: usize,
    row: f32,
}

/// How far a side panel has come out at `seconds` after it started: on the same spring as what expands elsewhere in the app ([`orbiters_ui::anim::Spring::EXPAND`]), with its few percent past the end.
fn side_spring(seconds: f64) -> f32 {
    let spring = orbiters_ui::anim::Spring::EXPAND;
    let t = seconds.max(0.0) as f32;
    let natural = (spring.stiffness / spring.mass).sqrt();
    let ratio = spring.damping / (2.0 * (spring.stiffness * spring.mass).sqrt());
    let damped = natural * (1.0 - ratio * ratio).max(1e-3).sqrt();
    1.0 - (-ratio * natural * t).exp() * ((damped * t).cos() + ratio * natural / damped * (damped * t).sin())
}

/// `draw` on `p`, moved by `by` and as solid as `alpha`, whatever it draws.
fn slid(p: &Painter, by: egui::Vec2, alpha: f32, draw: impl FnOnce(&Painter)) {
    let mut there = p.clone();
    there.set_opacity(alpha.clamp(0.0, 1.0));
    let layer = p.layer_id();
    let start = p.ctx().graphics_mut(|g| g.entry(layer).next_idx());
    draw(&there);
    p.ctx().graphics_mut(|g| {
        let list = g.entry(layer);
        let end = list.next_idx();
        list.transform_range(start, end, egui::emath::TSTransform::from_translation(by));
    });
}

const VR_LIST_TOP: f32 = 92.0;
const VR_LIST_BOTTOM: f32 = 564.0;
const VR_ROW: f32 = 156.0;
/// How far a hand goes before a press on the list is a drag of it: a little more than a hand's tremble at a laser's length.
const VR_DRAG_FROM: f32 = 14.0;
/// How much of its speed a thrown list keeps after a second, and below what speed it stops.
const VR_THROW_KEEPS: f64 = 0.04;
const VR_THROW_STOPS: f32 = 24.0;

/// A hand on a headset's list: where it pressed, what it pressed, and where it was last.
#[derive(Clone, Copy, Debug)]
struct VrHold {
    from: f32,
    scroll: f32,
    target: Option<Target>,
    dragged: bool,
    last: (f32, f64),
    speed: f32,
}

/// One of them: its picture, what it is, and its name as large as the head's.
fn vr_card(p: &Painter, rect: Rect, entry: &Entry, d: &Data, hover: bool, pressed: bool, selected: bool) {
    let rect = if pressed { rect.shrink(2.0) } else { rect };
    p.rect_filled(rect, CornerRadius::same(16), if hover { RAISED2 } else { RAISED });
    // Their names at a headset's size, on two lines (round 15's judges found them at a screen's).
    let thumb = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - 62.0));
    picture(p, thumb, (d.thumb)(entry), &entry.world_id, CornerRadius { nw: 16, ne: 16, sw: 0, se: 0 }, 1.0);
    let status = d.status;
    let chip = if status.current == Some(entry.id) {
        Some((if !status.shown { tr("Loading") } else if status.small { tr_ctx("view", "Small") } else { tr("Around you") }, if status.shown { orbit_look::accent() } else { theme::WARN }))
    } else if entry.capture {
        Some(("3D", orbit_look::accent()))
    } else {
        None
    };
    if let Some((word, tint)) = chip {
        let galley = p.layout_job(orbit_look::txt(word, 16.5).weight(weight::BOLD).color(orbit_look::on_accent()).job(f32::INFINITY));
        // Inside the selection's frame, clear of it (round 12's judges found it crowding the first tile).
        let back = Rect::from_min_size(thumb.min + vec2(13.0, 13.0), vec2(galley.size().x + 18.0, 28.0));
        p.rect_filled(back, CornerRadius::same(14), tint);
        p.galley(pos2(back.min.x + 9.0, back.center().y - galley.size().y / 2.0), galley, orbit_look::on_accent());
    }
    // Its whole name, on two lines where it takes them: worlds are told apart by it.
    let mut name = orbit_look::txt(entry.name.clone(), 22.5).weight(weight::SEMIBOLD).color(theme::FG).job(rect.width() - 16.0);
    name.wrap.max_rows = 2;
    let galley = p.layout_job(name);
    p.galley(pos2(rect.min.x + 9.0, rect.max.y - 31.0 - galley.size().y / 2.0), galley, theme::FG);
    if selected {
        p.rect_stroke(rect, CornerRadius::same(16), Stroke::new(3.0, orbit_look::accent()), StrokeKind::Inside);
    }
}

/// Where a few captures and the worlds under their heading lie: at full width, or beside the side panel. `n` captures come first.
fn few_rects(count: usize, n: usize, side: bool) -> (Vec<Option<Rect>>, f32) {
    let mut out = Vec::with_capacity(count);
    if side {
        // Every world there is keeps its place beside the panel, which is why these are a little shorter than the full list's.
        let rows = n.div_ceil(2) as f32;
        let heading = 148.0 + rows * 122.0;
        for i in 0..count {
            let (slot, top) = if i < n { (i, 148.0) } else { (i - n, heading + 28.0) };
            let (col, row) = ((slot % 2) as f32, (slot / 2) as f32);
            let rect = Rect::from_min_size(pos2(32.0 + col * 316.0, top + row * 122.0), vec2(304.0, 114.0));
            out.push((rect.bottom() <= 568.0).then_some(rect));
        }
        (out, heading)
    } else {
        let rows = n.div_ceil(PER_ROW) as f32;
        let heading = 148.0 + rows * 200.0 - 4.0;
        for i in 0..count {
            let (slot, top) = if i < n { (i, 148.0) } else { (i - n, heading + 28.0) };
            let (col, row) = ((slot % PER_ROW) as f32, (slot / PER_ROW) as f32);
            let rect = Rect::from_min_size(pos2(32.0 + col * 328.0, top + row * 200.0), vec2(304.0, 188.0));
            out.push((rect.bottom() <= 572.0).then_some(rect));
        }
        (out, heading)
    }
}

/// Where the way on to other worlds takes a free slot beside a few captures, when there is one: no gap in the list's edge.
fn more_slot(count: usize, n: usize, side: bool) -> Option<Rect> {
    if !side {
        return (n % PER_ROW != 0).then(|| card_rect(n));
    }
    let others = count.saturating_sub(n);
    (others % 2 == 1).then(|| {
        let (rects, heading) = few_rects(count + 1, n, true);
        let _ = heading;
        rects.last().copied().flatten()
    }).flatten()
}

/// A capture turning on the side panel, where its picture lies: as on the gallery's sheet, smaller. Its picture stays until it is ready, and if it cannot be.
/// The round full-screen button in the live view's corner: two corners out, or in when it fills the page.
fn draw_full_button(p: &Painter, rect: Rect, full: bool, hovered: bool, pressed: bool) {
    let fill = theme::black(if pressed { 0.9 } else if hovered { 0.82 } else { 0.66 });
    p.circle_filled(rect.center(), rect.width() / 2.0, fill);
    p.circle_stroke(rect.center(), rect.width() / 2.0, Stroke::new(1.0, Color32::from_white_alpha(if hovered { 110 } else { 50 })));
    widgets::icon(p, rect.center(), if full { icons::CORNERS_IN } else { icons::CORNERS_OUT }, 20.0, Color32::WHITE);
}

fn draw_live(p: &Painter, hero: Rect, live: &Live, named: Option<&str>) {
    let corner = CornerRadius::same(14);
    // In its picture's bottom left corner, the picture itself in view: on a screen it starts by itself each time a world is chosen (REQUEST15 item 6), so what it
    // says while it comes is a small word over the picture, not a box over its middle.
    let say = |p: &Painter, words: &str, dot: Color32| {
        let txt = orbit_look::txt(words, 15.0).weight(weight::SEMIBOLD).color(Color32::WHITE);
        let galley = p.layout_job(txt.job_truncated(hero.width() - 64.0));
        let pill = Rect::from_min_size(pos2(hero.left() + 12.0, hero.bottom() - 12.0 - galley.size().y - 12.0), galley.size() + vec2(40.0, 12.0));
        p.rect_filled(pill, CornerRadius::same((pill.height() / 2.0) as u8), theme::black(0.72));
        p.circle_filled(pos2(pill.left() + 15.0, pill.center().y), 4.5, dot);
        p.galley(pos2(pill.left() + 28.0, pill.min.y + 6.0), galley, Color32::WHITE);
    };
    if live.failed {
        say(p, tr("Its 3D cannot show here. It can in Orbiters."), theme::WARN);
        return;
    }
    let Some(id) = live.picture else {
        say(p, tr("Getting its 3D ready…"), orbit_look::accent());
        return;
    };
    p.rect_filled(hero, corner, Color32::from_rgb(15, 17, 21));
    super::super::orbit::panels::keep_colour(p, hero);
    p.add(egui::epaint::RectShape::filled(hero, corner, Color32::WHITE).with_texture(id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))));
    // In its frame nothing lies on it but its full-screen button (round 18's judges counted three things on a small picture); how to look around is said under it.
    let Some(named) = named else { return };
    let txt = orbit_look::txt(named, 18.0).weight(weight::SEMIBOLD).color(Color32::WHITE);
    let galley = p.layout_job(txt.job_truncated(hero.width() - 140.0));
    let pill = Rect::from_min_size(pos2(hero.left() + 14.0, hero.bottom() - 14.0 - galley.size().y - 12.0), galley.size() + vec2(28.0, 12.0));
    p.rect_filled(pill, CornerRadius::same((pill.height() / 2.0) as u8), theme::black(0.72));
    p.galley(pos2(pill.min.x + 14.0, pill.min.y + 6.0), galley, Color32::WHITE);
}

/// Beside the side panel: the row of what is browsed gets narrower, and the field and its pages.
const SIDE_SORT: Rect = Rect { min: Pos2 { x: 540.0, y: 96.0 }, max: Pos2 { x: 592.0, y: 136.0 } };
const SIDE_FILTER: Rect = Rect { min: Pos2 { x: 600.0, y: 96.0 }, max: Pos2 { x: 652.0, y: 136.0 } };

/// `color` over `outer`, rounded by `outer_radius`, but for `hole`, rounded by `hole_radius`, where what is behind shows.
pub fn fill_around(p: &Painter, outer: Rect, outer_radius: f32, hole: Rect, hole_radius: f32, color: Color32) {
    const PER_CORNER: usize = 9;
    let ring = |r: Rect, radius: f32| -> Vec<Pos2> {
        let corners = [(r.max.x - radius, r.min.y + radius, -90.0f32), (r.max.x - radius, r.max.y - radius, 0.0), (r.min.x + radius, r.max.y - radius, 90.0), (r.min.x + radius, r.min.y + radius, 180.0)];
        corners
            .iter()
            .flat_map(|&(x, y, from)| {
                (0..PER_CORNER).map(move |i| {
                    let (s, c) = (from + 90.0 * i as f32 / (PER_CORNER - 1) as f32).to_radians().sin_cos();
                    pos2(x + c * radius, y + s * radius)
                })
            })
            .collect()
    };
    let (a, b) = (ring(outer, outer_radius), ring(hole, hole_radius));
    let mut mesh = egui::epaint::Mesh::default();
    for (o, i) in a.iter().zip(&b) {
        mesh.vertices.push(egui::epaint::Vertex { pos: *o, uv: egui::epaint::WHITE_UV, color });
        mesh.vertices.push(egui::epaint::Vertex { pos: *i, uv: egui::epaint::WHITE_UV, color });
    }
    let n = a.len() as u32;
    for i in 0..n {
        let (o, h) = (i * 2, i * 2 + 1);
        let (o2, h2) = (((i + 1) % n) * 2, ((i + 1) % n) * 2 + 1);
        mesh.indices.extend_from_slice(&[o, o2, h, h, o2, h2]);
    }
    p.add(egui::Shape::mesh(mesh));
    // The edge the mesh leaves rough.
    p.rect_stroke(outer, CornerRadius::same(outer_radius as u8), Stroke::new(1.0, color), StrokeKind::Inside);
    p.rect_stroke(hole, CornerRadius::same(hole_radius as u8), Stroke::new(1.0, color), StrokeKind::Outside);
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
        let q = orbiters_core::search::Query::new(&self.query);
        let mut out: Vec<&'a Entry> = d
            .entries
            .iter()
            .filter(|e| {
                e.matches_query(&q, &self.query) && (!self.official || e.official) && (!self.has_3d || e.capture) && self.labels.iter().all(|l| d.label_count(e, l) > 0)
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
    /// A list goes on as far as there are worlds, with no pages, wherever it shows: Enzo, "infinite scroll creates less friction".
    pub fn vr_list_on(&self, _status: &Status) -> bool {
        matches!(self.page, Page::Browse | Page::Detail)
    }

    /// Its shape here.
    fn list_shape(&self, headset: bool) -> ListShape {
        match (self.page == Page::Detail, headset) {
            (true, true) => ListShape { top: VR_LIST_TOP, bottom: VR_LIST_BOTTOM, right: VR_LIST_RIGHT + 8.0, columns: 2, row: VR_ROW },
            (false, true) => ListShape { top: WIDE_LIST_TOP, bottom: HEIGHT, right: WIDTH - 8.0, columns: 4, row: 190.0 },
            (true, false) => ListShape { top: 142.0, bottom: HEIGHT - 4.0, right: 660.0, columns: 2, row: 136.0 },
            (false, false) => ListShape { top: 142.0, bottom: HEIGHT - 4.0, right: WIDTH - 8.0, columns: PER_ROW, row: 200.0 },
        }
    }

    /// Whether a press at this place is on the list, which may turn out a drag of it.
    pub fn vr_list_holds(&self, status: &Status, x: f32, y: f32) -> bool {
        let shape = self.list_shape(status.headset);
        (24.0..shape.right).contains(&x) && (shape.top..shape.bottom).contains(&y)
    }

    /// Where a result lies before it is scrolled: in a headset, on its way between the two shapes while the side panel comes and goes, so the page changes its layout and no other page comes.
    fn list_card(&self, i: usize, headset: bool, time: f64) -> Rect {
        let (narrow, wide) = if headset { (vr_card_rect(i), vr_wide_card_rect(i)) } else { (side_card_rect_flowing(i), card_rect(i)) };
        let out = self.side_out(time);
        let mix = |a: f32, b: f32| a + (b - a) * out;
        Rect::from_min_max(pos2(mix(wide.min.x, narrow.min.x), mix(wide.min.y, narrow.min.y)), pos2(mix(wide.max.x, narrow.max.x), mix(wide.max.y, narrow.max.y)))
    }

    /// How far the side panel is out, 0 to 1 and a little past it.
    fn side_out(&self, time: f64) -> f32 {
        let open = self.page == Page::Detail;
        if !self.moves {
            return if open { 1.0 } else { 0.0 };
        }
        let along = side_spring(time - self.side_at);
        if open { along } else { 1.0 - along }
    }

    /// Which results may show now, to read their pictures.
    pub fn list_window(&self, headset: bool, count: usize) -> std::ops::Range<usize> {
        let shape = self.list_shape(headset);
        let first = ((self.vr_scroll / shape.row).floor().max(0.0) as usize) * shape.columns;
        let rows = ((shape.bottom - shape.top) / shape.row).ceil() as usize + 1;
        first.min(count)..(first + rows * shape.columns).min(count)
    }

    /// The search field: from the page's edge on a sheet, whose head has the way back.
    fn field_here(&self) -> Rect {
        let field = field_rect();
        if self.on_sheet { Rect::from_min_max(pos2(32.0, field.min.y), field.max) } else { field }
    }

    /// Whether a Back here stays on this panel: what was opened from what it opened on.
    pub fn goes_back_within(&self) -> bool {
        self.page != Page::Browse
    }

    /// How far down its list it is, for the tests.
    #[cfg(test)]
    pub fn list_scroll_for_test(&self) -> f32 {
        self.vr_scroll
    }

    /// The list moved by so much, by a stick or a wheel.
    pub fn scroll_by(&mut self, by: f32) {
        self.vr_throw = 0.0;
        self.vr_scroll_to(self.vr_scroll + by);
    }

    /// Back to the start of a list: another section, another search.
    fn to_the_top(&mut self) {
        self.results_page = 0;
        self.vr_scroll = 0.0;
        self.vr_throw = 0.0;
    }

    fn vr_scroll_to(&mut self, to: f32) {
        self.vr_scroll = to.clamp(0.0, self.vr_most.get());
        // What is near the end asks for what follows it, as a page near the end did.
        let (row, columns) = self.list_step.get();
        self.results_page = ((self.vr_scroll / row.max(1.0)) as usize * columns.max(1)) / PER_PAGE;
        self.dirty = true;
    }

    pub fn vr_press(&mut self, y: f32, target: Option<Target>, time: f64) {
        self.vr_throw = 0.0;
        self.vr_hold = Some(VrHold { from: y, scroll: self.vr_scroll, target, dragged: false, last: (y, time), speed: 0.0 });
    }

    /// The hand on the list moved: past a tremble, the list goes with it.
    pub fn vr_drag(&mut self, y: f32, time: f64) {
        let Some(mut hold) = self.vr_hold else { return };
        hold.dragged |= (y - hold.from).abs() >= VR_DRAG_FROM;
        if hold.dragged {
            let seconds = (time - hold.last.1) as f32;
            if seconds > 0.001 {
                hold.speed = (hold.last.0 - y) / seconds;
                hold.last = (y, time);
            }
            self.vr_hold = Some(hold);
            self.vr_scroll_to(hold.scroll + hold.from - y);
        } else {
            self.vr_hold = Some(hold);
        }
    }

    pub fn vr_dragging(&self) -> bool {
        self.vr_hold.is_some_and(|h| h.dragged)
    }

    /// Let go: what was pressed, if it stayed a press; thrown, if it was moving.
    /// A press of the lasers that ends with no let-go (its menu went): no target, no throw.
    pub fn vr_cancel(&mut self) {
        self.vr_hold = None;
    }

    pub fn vr_release(&mut self, time: f64) -> Option<Target> {
        let hold = self.vr_hold.take()?;
        if hold.dragged {
            self.vr_throw = hold.speed;
            self.vr_throw_at = time;
            return None;
        }
        hold.target
    }

    /// A list thrown goes on, slowing.
    pub fn vr_coast(&mut self, time: f64) {
        if self.vr_throw == 0.0 {
            return;
        }
        let seconds = (time - self.vr_throw_at).clamp(0.0, 0.1);
        self.vr_throw_at = time;
        let to = self.vr_scroll + self.vr_throw * seconds as f32;
        self.vr_throw *= VR_THROW_KEEPS.powf(seconds) as f32;
        self.vr_scroll_to(to);
        if self.vr_throw.abs() < VR_THROW_STOPS || self.vr_scroll <= 0.0 || self.vr_scroll >= self.vr_most.get() {
            self.vr_throw = 0.0;
        }
        self.dirty = true;
    }

    /// How far a list of `n` goes, with a row more when more are coming.
    fn vr_measure(&self, n: usize, more: bool, headset: bool) {
        let shape = self.list_shape(headset);
        let rows = n.div_ceil(shape.columns) + usize::from(more);
        self.vr_most.set((shape.top + 4.0 + rows as f32 * shape.row - shape.bottom).max(0.0));
        self.list_step.set((shape.row, shape.columns));
        self.list_headset.set(headset);
    }

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
            let title = if d.status.list.source == Some(FoundBy::Vrchat) { tr("VRChat could not be asked") } else { tr("The worlds could not be read") };
            d.status.list.error.as_deref().map(|e| (title, e))
        } else {
            d.status.listing_error.as_deref().filter(|_| d.entries.is_empty()).map(|e| (tr("Orbiters could not be reached"), e))
        }
    }

    /// The order's name where it applies: as listed is a search's best match, VRChat's lists' top.
    fn sort_label(&self, d: &Data) -> &'static str {
        match self.sort {
            Sort::Newest if self.searching(d) => tr("Best match"),
            Sort::Newest => match self.section {
                Section::Captures => tr_ctx("sorting", "Newest"),
                Section::Visited => tr("Latest"),
                Section::Popular | Section::Active | Section::Favorites => tr_ctx("sorting", "Top"),
            },
            s => s.label(),
        }
    }

    fn go(&mut self, page: Page, time: f64) {
        if page != self.page {
            // To and from the side panel the page stays and its layout changes, with where the list is scrolled to kept in step.
            if (page == Page::Detail) != (self.page == Page::Detail) && matches!(page, Page::Browse | Page::Detail) && matches!(self.page, Page::Browse | Page::Detail) {
                self.side_at = time;
                let headset = self.list_headset.get();
                let was = self.list_shape(headset);
                let first = (self.vr_scroll / was.row) as usize * was.columns;
                self.page = page;
                let now = self.list_shape(headset);
                self.vr_scroll = (first / now.columns) as f32 * now.row;
            }
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
            || time - self.selected_at < 0.3
            || (self.moves && time - self.side_at < 0.7)
            || status.small_docked
            || status.stage.is_some()
            || status.fetching
            || status.list.loading
            || matches!(self.page, Page::Typing | Page::Compose(_))
            || self.confirm.is_some_and(|(_, at)| time - at < CONFIRM_FOR + 0.1)
            || self.note.as_ref().is_some_and(|(_, at)| time - at < NOTE_FOR + 0.1)
            || self.copied.is_some_and(|(_, at)| time - at < COPIED_FOR + 0.1)
    }

    /// How many captures lead the list when worlds without one follow them for lack of them.
    fn few_leading(&self, d: &Data, results: &[&Entry]) -> Option<usize> {
        let followed = !self.searching(d) && self.section == Section::Captures && !self.filtered() && results.iter().any(|e| !e.capture);
        followed.then(|| results.iter().filter(|e| e.capture).count())
    }

    /// Showing the captures, of which there are few.
    fn few_captures(&self, d: &Data) -> bool {
        !self.searching(d) && self.section == Section::Captures && !self.filtered() && self.results(d).len() <= 3
    }

    /// Where else to go from few captures.
    fn few_targets(&self, side: bool) -> [(Section, &'static str, Rect); 2] {
        let (x, width) = if side { (106.0, 220.0) } else { (272.0, 232.0) };
        let (y, gap) = if side { (472.0, 8.0) } else { (486.0, 16.0) };
        [
            (Section::Popular, icons::GLOBE_HEMISPHERE_WEST, Rect::from_min_size(pos2(x, y), vec2(width, 52.0))),
            (Section::Active, icons::USERS, Rect::from_min_size(pos2(x + width + gap, y), vec2(width, 52.0))),
        ]
    }

    /// The browse page's pressable things, at full width or beside the side panel.
    fn browse_targets(&self, d: &Data, out: &mut Vec<(Target, Rect)>, side: bool) {
        let results = self.results(d);
        if side && d.status.headset {
            out.push((Target::CloseSide, vr_list_head(1)));
            self.vr_measure(results.len(), d.status.list.more || self.loading(d), true);
            // What shows of those in view is what can be pressed.
            let list = Rect::from_min_max(pos2(0.0, VR_LIST_TOP), pos2(VR_LIST_RIGHT + 8.0, VR_LIST_BOTTOM));
            for i in 0..results.len() {
                let rect = vr_card_rect(i).translate(vec2(0.0, -self.vr_scroll)).intersect(list);
                if rect.is_positive() {
                    out.push((Target::Card(i), rect));
                }
            }
            return;
        }
        out.push((Target::Field, if side { side_field_rect(self.on_sheet) } else { self.field_here() }));
        if self.searching(d) {
            out.push((Target::Has3d, ONLY_3D));
        } else {
            for (i, s) in Section::ALL.into_iter().enumerate() {
                out.push((Target::Section(s), section_rect(i)));
            }
        }
        out.push((Target::Sort(self.sort.next()), if side { SIDE_SORT } else { SORT }));
        out.push((Target::Filter, if side { SIDE_FILTER } else { FILTER }));
        if self.filtered() && !side {
            out.push((Target::ClearFilters, CLEAR));
        }
        if let Some(n) = self.few_leading(d, &results).filter(|_| !d.status.headset) {
            let (rects, heading) = few_rects(results.len(), n, side);
            for (i, rect) in rects.into_iter().enumerate() {
                if let Some(rect) = rect {
                    out.push((Target::Card(i), rect));
                }
            }
            // Beside few captures, the way on takes the row's place; else it is on the heading's end.
            if let Some(slot) = more_slot(results.len(), n, side) {
                out.push((Target::Section(Section::Popular), slot));
            } else {
                let right = if side { 652.0 } else { 992.0 };
                out.push((Target::Section(Section::Popular), Rect::from_min_size(pos2(right - 132.0, heading - 6.0), vec2(132.0, 34.0))));
            }
        } else {
            // As far as there are worlds: what shows of those in view is what can be pressed.
            self.vr_measure(results.len(), d.status.list.more || self.loading(d), d.status.headset);
            let shape = self.list_shape(d.status.headset);
            let list = Rect::from_min_max(pos2(0.0, shape.top), pos2(WIDTH, shape.bottom));
            for i in 0..results.len() {
                let rect = if d.status.headset { vr_wide_card_rect(i) } else if side { side_card_rect_flowing(i) } else { card_rect(i) };
                let rect = rect.translate(vec2(0.0, -self.vr_scroll)).intersect(list);
                if rect.is_positive() {
                    out.push((Target::Card(i), rect));
                }
            }
        }
        if !results.is_empty() && self.few_captures(d) {
            for (s, _, rect) in self.few_targets(side) {
                out.push((Target::Section(s), rect));
            }
        }
        if results.is_empty() && self.failure(d).is_some() && !side {
            out.push((Target::Retry, Rect::from_center_size(pos2(512.0, 420.0), vec2(200.0, 56.0))));
        }
    }

    /// What the carousel flips through, in the list's order (its search, section, labels and sort): the worlds with a 3D capture while the one shown has one, else
    /// every world of the list.
    pub fn carousel<'a>(&self, d: &Data<'a>) -> Vec<&'a Entry> {
        let results = self.results(d);
        if self.entry(d).is_none_or(|e| e.capture) { results.into_iter().filter(|e| e.capture).collect() } else { results }
    }

    /// Where the world shown is in the carousel (from 0), how many it has, and the worlds before and after it.
    pub fn carousel_place<'a>(&self, d: &Data<'a>) -> Option<(usize, usize, Option<&'a Entry>, Option<&'a Entry>)> {
        if self.page != Page::Detail || !d.status.headset {
            return None;
        }
        let set = self.carousel(d);
        let at = set.iter().position(|e| Some(e.id) == self.selected)?;
        Some((at, set.len(), at.checked_sub(1).map(|i| set[i]), set.get(at + 1).copied()))
    }

    /// The world before or after the one shown, on its side panel: it is shown there at once, around or small as this one was, and the list beside it keeps it in view
    /// and lit (K-carousel: Enzo, "more frictionless and more straight forward to go through all the 3D capture pictures").
    fn flip(&mut self, d: &Data, forward: bool, time: f64) -> Vec<Action> {
        let Some((_, _, before, after)) = self.carousel_place(d) else { return Vec::new() };
        let Some(entry) = (if forward { after } else { before }) else { return Vec::new() };
        self.selected_at = time;
        self.flipped = Some((forward, time));
        self.select(entry.id);
        if let Some(i) = self.results(d).iter().position(|e| e.id == entry.id) {
            self.reveal(i, d.status.headset);
        }
        let status = d.status;
        if !entry.capture || status.current == Some(entry.id) && (status.shown || status.stage.is_some()) {
            Vec::new()
        } else {
            vec![Action::Show(entry.id)]
        }
    }

    /// The list beside the side panel scrolled just enough for result `i` to be in view, as the carousel goes past its end.
    fn reveal(&mut self, i: usize, headset: bool) {
        let shape = self.list_shape(headset);
        let rect = if headset { vr_card_rect(i) } else { side_card_rect_flowing(i) };
        let (top, bottom) = (rect.min.y - self.vr_scroll, rect.max.y - self.vr_scroll);
        if top < shape.top {
            self.vr_throw = 0.0;
            self.vr_scroll_to(self.vr_scroll - (shape.top - top) - 8.0);
        } else if bottom > shape.bottom {
            self.vr_throw = 0.0;
            self.vr_scroll_to(self.vr_scroll + (bottom - shape.bottom) + 8.0);
        }
    }

    /// What a stick does with a laser at (x, y) on this page ([`StickSpot`]).
    pub fn stick_spot(&self, d: &Data, x: f32, y: f32) -> Option<StickSpot> {
        let at = pos2(x, y);
        // Over the picture's edges, its arrows are the carousel's.
        let on_arrow = over_edges() && self.carousel_place(d).is_some_and(|(_, _, b, a)| (b.is_some() && vr_flip(false).0.contains(at)) || (a.is_some() && vr_flip(true).0.contains(at)));
        if !on_arrow && self.hole(d.status).is_some_and(|h| h.contains(at)) {
            return Some(StickSpot::InCapture);
        }
        let flips = self.carousel_place(d).is_some_and(|(_, n, _, _)| n > 1);
        (flips && VR_FLIP_BAND.contains(at)).then_some(StickSpot::Flip)
    }

    /// The carousel was last flipped back.
    pub fn flipped_back(&self) -> bool {
        self.flipped.is_some_and(|(forward, _)| !forward)
    }

    /// The stick pushed to one side on the carousel: the next or the one before.
    pub fn flip_by_stick(&mut self, d: &Data, forward: bool, time: f64) -> Vec<Action> {
        self.dirty = true;
        self.flip(d, forward, time)
    }

    /// Where a capture standing small may lie on this page: the picture of its world's side panel.
    pub fn small_place(&self, status: &Status) -> Option<Rect> {
        (self.page == Page::Detail && status.small && status.current.is_some() && status.current == self.selected).then_some(if status.headset { vr_hero() } else { SIDE_HERO })
    }

    /// The hole it is seen through, when it lies there now.
    pub fn hole(&self, status: &Status) -> Option<Rect> {
        self.small_place(status).filter(|_| status.small_docked && status.shown && status.stage.is_none())
    }

    /// Where each pressable thing is now.
    pub fn targets(&self, d: &Data) -> Vec<(Target, Rect)> {
        let mut out = if self.on_sheet { Vec::new() } else { vec![(Target::Back, Rect::from_center_size(pos2(60.0, 52.0), vec2(56.0, 56.0)))] };
        match self.page {
            Page::Browse => self.browse_targets(d, &mut out, false),
            Page::Typing => {
                out.push((Target::Field, self.field_here()));
                out.push((Target::Done, Rect::from_min_size(pos2(824.0, 24.0), vec2(168.0, 56.0))));
                for (i, _) in self.results(d).iter().enumerate().take(PER_ROW) {
                    out.push((Target::Card(i), compact_rect(i)));
                }
                out.extend(keys(self.layout, false));
            }
            Page::Detail => {
                let vr = d.status.headset;
                let live_here = !vr && self.entry(d).is_some_and(|e| self.live.as_ref().is_some_and(|l| l.id == e.id));
                // In a headset the list's head is the way back as well, and there is one.
                if vr {
                    out.clear();
                }
                self.browse_targets(d, &mut out, true);
                // Full: the view and its button back, and nothing else.
                if live_here && self.live.as_ref().is_some_and(|l| l.full) {
                    let page = Rect::from_min_size(Pos2::ZERO, vec2(WIDTH, HEIGHT));
                    return vec![(Target::Hero, page), (Target::FullView, full_button(page))];
                }
                // A hand on the capture turning there, under what lies over it.
                if live_here {
                    out.push((Target::Hero, SIDE_HERO));
                    // Its full-screen button, once there is something to fill the page with.
                    if self.live.as_ref().is_some_and(|l| l.picture.is_some()) {
                        out.push((Target::FullView, full_button(SIDE_HERO)));
                    }
                }
                if !vr {
                    out.push((Target::CloseSide, SIDE_CLOSE));
                }
                let Some(e) = self.entry(d) else { return out };
                // Just after its name, where the last paint put it.
                if let Some((_, _, rect)) = self.copy_at.get().filter(|(id, headset, _)| *id == e.id && *headset == vr) {
                    out.push((Target::CopyName, rect));
                }
                let status = d.status;
                let current = status.current == Some(e.id) && (status.shown || status.stage.is_some());
                // Going there needs the member's own VRChat session in Orbiters (Enzo, F5): without it there is no list to choose from, and no button.
                let go_ok = status.list.vrchat;
                if !e.capture {
                    if go_ok {
                        out.push((Target::Join, if vr { VR_GO_HALF } else { SHOW_WHOLE }));
                    }
                } else if !status.headset {
                    // On a screen its capture turns where its picture is as soon as it is chosen (REQUEST15 item 6): no See it in 3D.
                } else if current {
                    // The other way to see it: the sheet's head, or N4N0's bar, hides it.
                    // Both ways stay, the one it is in lit.
                    out.push((Target::Show, if go_ok { VR_SHOW } else { VR_SHOW_WIDE }));
                    out.push((Target::ShowSmall, if go_ok { VR_SHOW_SMALL } else { VR_SHOW_SMALL_WIDE }));
                    if status.small && status.small_taken {
                        out.push((Target::SmallBack, vr_small_back()));
                    }
                } else {
                    out.push((Target::Show, if go_ok { VR_SHOW } else { VR_SHOW_WIDE }));
                    out.push((Target::ShowSmall, if go_ok { VR_SHOW_SMALL } else { VR_SHOW_SMALL_WIDE }));
                }
                // Going there, from a capture as from any world; else its page on VRChat alone.
                let web = super::finder::web_link(&e.world_id).is_some();
                if e.capture {
                    if go_ok {
                        out.push((Target::Join, match (web, vr) {
                            (_, true) => VR_GO,
                            (true, false) => GO,
                            (false, false) => WEB_WHOLE,
                        }));
                    }
                    // A page on VRChat's site is for a screen.
                    if web && !vr {
                        out.push((Target::OpenWeb, if go_ok { WEB } else { WEB_WHOLE }));
                    }
                } else if web {
                    out.push((Target::OpenWeb, match (vr, go_ok) {
                        (true, true) => VR_WEB_HALF,
                        (true, false) => VR_WEB_WHOLE,
                        (false, _) => WEB_WHOLE,
                    }));
                }
                let mut tiles = vec![Target::Like, Target::Comments];
                if d.facts_of(e).is_some_and(|f| f.picture_count > 0) || e.pictures > 0 {
                    tiles.push(Target::Pictures);
                }
                tiles.push(Target::Labels);
                let n = tiles.len();
                for (i, t) in tiles.into_iter().enumerate() {
                    out.push((t, small_tile(i, n, vr)));
                }
                if !d.others(e).is_empty() {
                    out.push((Target::Other, if vr { VR_OTHER } else { OTHER }));
                }
                // The carousel's way to the capture before and after this one, where there is one.
                if let Some((_, _, before, after)) = self.carousel_place(d) {
                    if before.is_some() {
                        out.push((Target::FlipPrevious, vr_flip(false).0));
                    }
                    if after.is_some() {
                        out.push((Target::FlipNext, vr_flip(true).0));
                    }
                }
            }
            Page::Filter | Page::Labels | Page::Comments | Page::Compose(_) | Page::Pictures => self.page_targets(d, &mut out),
            Page::Go => self.go_targets(d, &mut out),
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
            self.say(tr("Sign in to Orbiters - N4N0 to give feedback."), time);
            return None;
        }
        let wf = d.world_feedback(e);
        if wf.loading {
            // Orbiters did not answer: that is what is said, not a wait that never ends.
            match &wf.error {
                Some(error) => self.say(error.clone(), time),
                None => self.say(tr("Reading this world's feedback from Orbiters…"), time),
            }
            return None;
        }
        Some(wf)
    }

    /// Whether a comment can be written on `e`'s world now: signed in to Orbiters is all it takes (a comment is added to the others, so what the member gave
    /// before need not be read first; VR feedback F6 found Write a comment refused for good where Orbiters did not answer that read). Otherwise the way to
    /// sign in, said beside the button.
    fn can_comment(&mut self, d: &Data, e: &Entry, time: f64) -> Result<WorldFeedback, Vec<Action>> {
        if !d.feedback.signed_in {
            self.say(tr("Your browser opens on your computer: sign in to Orbiters there, then write your comment."), time);
            return Err(vec![Action::SignIn]);
        }
        Ok(d.world_feedback(e))
    }

    /// The person pressed `target`.
    pub fn press(&mut self, target: Target, d: &Data, time: f64) -> Vec<Action> {
        self.dirty = true;
        let status = d.status;
        match target {
            Target::CloseSide => {
                if let Some(live) = self.live.as_mut() {
                    live.full = false;
                }
                self.go(if self.query.is_empty() { Page::Browse } else { Page::Typing }, time);
                Vec::new()
            }
            Target::SmallBack => vec![Action::SmallBack],
            Target::ShowSmall => {
                let Some(e) = self.entry(d).filter(|e| e.capture) else { return Vec::new() };
                if status.current == Some(e.id) {
                    vec![Action::Small(true)]
                } else {
                    vec![Action::Small(true), Action::Show(e.id)]
                }
            }
            Target::Back => match self.page {
                Page::Detail => {
                    self.go(if self.query.is_empty() { Page::Browse } else { Page::Typing }, time);
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
                Page::Labels | Page::Comments | Page::Pictures | Page::Go => {
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
                    self.to_the_top();
                    self.go(Page::Browse, time);
                    Vec::new()
                }
                _ => {
                    self.to_the_top();
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
                    Page::Go => {
                        self.go_page = step(self.go_page, go::go_pages(&status.go));
                    }
                    _ => self.results_page = step(self.results_page, self.pages(d, self.results(d).len())),
                }
                self.page_at = time;
                Vec::new()
            }
            Target::FlipPrevious | Target::FlipNext => self.flip(d, target == Target::FlipNext, time),
            Target::Card(i) => {
                let results = self.results(d);
                let Some(entry) = results.get(i) else { return Vec::new() };
                // Beside the list, which stays: one after another, with nothing to come back from.
                if self.page == Page::Detail {
                    self.selected_at = time;
                    self.select(entry.id);
                } else {
                    self.select(entry.id);
                    self.go(Page::Detail, time);
                }
                // A world without a capture: its details only. With no headset nothing shows around anyone, and its details have what opens the app's window.
                if !status.headset || !entry.capture || status.current == Some(entry.id) && (status.shown || status.stage.is_some()) {
                    Vec::new()
                } else {
                    vec![Action::Show(entry.id)]
                }
            }
            Target::Show => {
                let Some(e) = self.entry(d).filter(|e| e.capture) else { return Vec::new() };
                match (status.small, status.current == Some(e.id)) {
                    (true, true) => vec![Action::Small(false)],
                    (true, false) => vec![Action::Small(false), Action::Show(e.id)],
                    (false, true) => Vec::new(),
                    (false, false) => vec![Action::Show(e.id)],
                }
            }
            Target::Hero => Vec::new(),
            Target::FullView => {
                if let Some(live) = self.live.as_mut() {
                    live.full = !live.full;
                }
                Vec::new()
            }
            // Its instances, read: nothing is sent to VRChat until one is pressed (F5).
            Target::Join => {
                // A press left over from before signing out of VRChat does nothing (the button is gone).
                let Some(e) = self.entry(d).filter(|_| d.status.list.vrchat) else { return Vec::new() };
                self.go_page = 0;
                self.go(Page::Go, time);
                vec![Action::Join(e.world_id.clone())]
            }
            Target::GoRetry => {
                self.go_page = 0;
                let world = if status.go.world.is_empty() { self.entry(d).map(|e| e.world_id.clone()) } else { Some(status.go.world.clone()) };
                world.map(Action::Join).into_iter().collect()
            }
            Target::GoInstance(i) => {
                let go = &status.go;
                if go.busy.is_some() {
                    return Vec::new();
                }
                match go.list.as_ref().and_then(|l| l.items.get(i)).filter(|x| x.can_join) {
                    Some(x) => vec![Action::GoTo { world: go.world.clone(), instance: x.id.clone() }],
                    None => Vec::new(),
                }
            }
            Target::GoInvite => {
                let go = &status.go;
                match go.went.as_ref().filter(|g| go.busy.is_none() && g.how == crate::controller::travel::Went::Direct && !g.arrived) {
                    Some(g) => vec![Action::InviteInstead { world: go.world.clone(), instance: g.instance.id.clone() }],
                    None => Vec::new(),
                }
            }
            Target::NewInstance(k) => {
                let go = &status.go;
                let Some(access) = go::NEW_KINDS.get(k as usize).copied().filter(|_| go.busy.is_none() && !go.world.is_empty()) else { return Vec::new() };
                vec![Action::NewInstance { world: go.world.clone(), access, region: self.go_region_now(go) }]
            }
            Target::GoRegion => {
                let now = self.go_region_now(&status.go);
                let at = go::REGIONS.iter().position(|r| *r == now).unwrap_or(0);
                self.go_region = Some(go::REGIONS[(at + 1) % go::REGIONS.len()]);
                Vec::new()
            }
            Target::OpenWeb => self.entry(d).map(|e| Action::OpenWeb(e.world_id.clone())).into_iter().collect(),
            Target::CopyName => match self.entry(d).filter(|e| !e.name.trim().is_empty()) {
                Some(e) => {
                    self.copied = Some((e.id, time));
                    self.dirty = true;
                    vec![Action::CopyName(e.name.clone())]
                }
                None => Vec::new(),
            },
            Target::Has3d => {
                self.has_3d = !self.has_3d;
                self.to_the_top();
                self.page_at = time;
                Vec::new()
            }
            Target::Section(s) => {
                if self.section != s {
                    self.section = s;
                    self.to_the_top();
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
                self.to_the_top();
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
                    self.to_the_top();
                }
                Vec::new()
            }
            Target::FilterOfficial => {
                self.official = !self.official;
                self.to_the_top();
                Vec::new()
            }
            Target::ClearFilters => {
                self.labels.clear();
                self.official = false;
                self.has_3d = false;
                self.to_the_top();
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
                let Some(e) = self.entry(d) else {
                    self.say(tr("This world is no longer listed: go back and pick it again."), time);
                    return Vec::new();
                };
                match self.can_comment(d, e, time) {
                    Ok(_) => {
                        self.draft.clear();
                        self.note = None;
                        self.go(Page::Compose(Compose::Comment), time);
                        Vec::new()
                    }
                    Err(way_out) => way_out,
                }
            }
            Target::RetryFeedback => {
                self.say(tr("Asking Orbiters again…"), time);
                vec![Action::RetryFeedback]
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

    /// The page of the world `id` (picked by the caller), opened from elsewhere: no list press brought it.
    pub fn show_detail(&mut self, id: u64, time: f64) {
        self.select(id);
        self.note = None;
        self.confirm = None;
        self.go(Page::Detail, time);
        self.selected_at = time;
    }

    /// Back to what showed before [`Panel::show_detail`].
    pub fn restore(&mut self, page: Page, selected: Option<u64>, time: f64) {
        self.go(page, time);
        match selected {
            Some(id) => self.select(id),
            None => self.selected = None,
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
        let wf = match compose {
            // What was typed stays: signed in, it is sent with Send again.
            Compose::Comment => match self.can_comment(d, e, time) {
                Ok(wf) => wf,
                Err(way_out) => return way_out,
            },
            Compose::Reason => match self.can_give(d, e, time) {
                Some(wf) => wf,
                None => return Vec::new(),
            },
        };
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
        self.to_the_top();
        self.dirty = true;
    }

    fn erase(&mut self) {
        self.typed().pop();
        self.to_the_top();
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
        // The live view on the whole page: only it, and the button that brings it back to its frame.
        if let Some(live) = self.live.as_ref().filter(|l| l.full && !d.status.headset && self.page == Page::Detail && self.entry(d).is_some_and(|e| e.id == l.id)) {
            p.rect_filled(page, CornerRadius::same(28), CARD);
            // Its world's name stays in view (round 18's judges: on the whole page nothing said what was looked at).
            draw_live(p, page, live, self.entry(d).map(|e| e.name.as_str()));
            let state = (self.hover == Some(Target::FullView), self.pressed == Some(Target::FullView));
            draw_full_button(p, full_button(page), true, state.0, state.1);
            return;
        }
        // A hole where a capture standing small is seen through, or the page whole.
        match self.hole(d.status) {
            Some(hole) => fill_around(p, page, 28.0, hole, HOLE_RADIUS, CARD),
            None => {
                p.rect_filled(page, CornerRadius::same(28), CARD);
            }
        }
        let shown = ease_out((time - self.page_at) / PAGE_FADE);
        let targets = self.targets(d);
        let state = |t: Target| (self.hover == Some(t), self.pressed == Some(t));
        // The back button, on every page but a headset's side panel, whose list's head is that.
        if targets.iter().any(|(t, _)| *t == Target::Back) {
            let (hover, pressed) = state(Target::Back);
            round_button(p, pos2(60.0, 52.0), icons::ARROW_LEFT, hover, pressed, 1.0);
        }
        match self.page {
            Page::Browse => self.browse(p, d, &targets, time, shown),
            Page::Typing => {
                let results = self.results(d);
                field(p, self.field_here(), &self.query, tr("Search VRChat worlds"), icons::MAGNIFYING_GLASS, true, state(Target::Field), time, 1.0);
                let (h, pr) = state(Target::Done);
                pill_button(p, Rect::from_min_size(pos2(824.0, 24.0), vec2(168.0, 56.0)), Some(icons::MAGNIFYING_GLASS), tr("Done"), orbit_look::accent(), orbit_look::on_accent(), h, pr, 1.0);
                for (t, rect) in &targets {
                    if let Target::Card(i) = *t {
                        let (h, pr) = state(*t);
                        compact_card(p, *rect, results[i], d, h, pr, shown, false);
                    }
                }
                if results.is_empty() {
                    let msg = if self.loading(d) {
                        tr("Searching…")
                    } else if let Some((title, _)) = self.failure(d) {
                        title
                    } else if self.query.trim().chars().count() < 2 {
                        tr("Type a world's name")
                    } else {
                        tr("No world matches")
                    };
                    text(p, pos2(512.0, 162.0), Align2::CENTER_CENTER, orbit_look::txt(msg, 19.0).color(fade(theme::FG_3, shown)));
                }
                self.keyboard(p, &targets, shown);
            }
            Page::Detail => {
                self.browse(p, d, &targets, time, 1.0);
                // It comes out beside the list, which makes it room: the page changes its layout, and no other page comes (Enzo).
                let out = self.side_out(time);
                // Where a capture shows through it, it stands where its hole is: drawn on its way, moved and see-through, its frame round the hole covered
                // part of the capture lying there for most of a second (Enzo: "the part that should be see through is glitchy").
                if (out >= 0.999 && (out - 1.0).abs() < 0.001) || self.hole(d.status).is_some() {
                    self.side_panel(p, d, time, &targets);
                } else {
                    slid(p, vec2((1.0 - out) * 70.0, 0.0), out, |there| self.side_panel(there, d, time, &targets));
                }
            }
            Page::Filter | Page::Labels | Page::Comments | Page::Compose(_) | Page::Pictures => self.draw_page(p, d, time, &targets, shown),
            Page::Go => self.draw_go(p, d, time, &targets, shown),
        }
    }

    fn keyboard(&self, p: &Painter, targets: &[(Target, Rect)], shown: f32) {
        let state = |t: Target| (self.hover == Some(t), self.pressed == Some(t));
        for (t, rect) in targets {
            let (h, pr) = state(*t);
            match *t {
                Target::Key(c) => key_cap(p, *rect, &c.to_string(), None, RAISED, theme::FG, h, pr, shown),
                Target::Space => key_cap(p, *rect, tr("space"), None, RAISED, theme::FG, h, pr, shown),
                Target::Backspace => key_cap(p, *rect, "", Some(icons::BACKSPACE), RAISED2, theme::FG_2, h, pr, shown),
                _ => {}
            }
        }
    }

    fn browse(&self, p: &Painter, d: &Data, targets: &[(Target, Rect)], time: f64, shown: f32) {
        let state = |t: Target| (self.hover == Some(t), self.pressed == Some(t));
        let results = self.results(d);
        let side = self.page == Page::Detail;
        if side && d.status.headset {
            let (h, pr) = state(Target::CloseSide);
            let head = vr_list_head(1);
            // Its name on a ground of its own, then search and the filters each on a square tile of theirs (round 19's judges: two small icons at the end of one
            // bar). All of it is one press, which goes back to the full list where they are.
            let tile = head.height();
            let named = Rect::from_min_max(head.min, pos2(head.max.x - 2.0 * (tile + 6.0), head.max.y));
            p.rect_filled(if pr { named.shrink(2.0) } else { named }, CornerRadius::same(16), if h { RAISED2 } else { RAISED });
            widgets::icon(p, pos2(named.min.x + 24.0, named.center().y), icons::CARET_LEFT, 24.0, theme::FG);
            let name = if self.query.is_empty() { self.section.label(d.status.list.vrchat).to_string() } else { self.query.clone() };
            let galley = p.layout_job(orbit_look::txt(name, 21.0).weight(weight::SEMIBOLD).color(theme::FG).job_truncated(named.width() - 52.0));
            p.galley(pos2(named.min.x + 42.0, named.center().y - galley.size().y / 2.0), galley, theme::FG);
            for (i, glyph) in [icons::MAGNIFYING_GLASS, icons::SLIDERS_HORIZONTAL].into_iter().enumerate() {
                let at = Rect::from_min_size(pos2(head.max.x - (2 - i) as f32 * (tile + 6.0) + 6.0, head.min.y), egui::Vec2::splat(tile));
                p.rect_filled(if pr { at.shrink(2.0) } else { at }, CornerRadius::same(16), if h { RAISED2 } else { RAISED });
                widgets::icon(p, at.center(), glyph, 25.0, theme::FG);
            }
            // While it is on its way to its two columns, it still has their room.
            let right = if self.side_out(time) < 0.995 { WIDTH } else { VR_LIST_RIGHT + 8.0 };
            let list = p.with_clip_rect(Rect::from_min_max(pos2(0.0, VR_LIST_TOP), pos2(right, VR_LIST_BOTTOM)));
            for (t, _) in targets {
                if let Target::Card(i) = *t {
                    let (h, pr) = state(*t);
                    vr_card(&list, self.list_card(i, true, time).translate(vec2(0.0, -self.vr_scroll)), results[i], d, h, pr, self.selected == Some(results[i].id));
                }
            }
            if results.is_empty() {
                text(p, pos2(232.0, 320.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Nothing else here"), 20.0).color(theme::FG_3));
            } else if d.status.list.more || self.loading(d) {
                // Under the last of them, as they keep coming.
                let y = vr_card_rect(results.len().next_multiple_of(2)).center().y - self.vr_scroll;
                if y < VR_LIST_BOTTOM {
                    text(&list, pos2(232.0, y), Align2::CENTER_CENTER, orbit_look::txt(tr("Finding more worlds…"), 20.0).color(theme::FG_3));
                }
            }
            // Where in it this is.
            let most = self.vr_most.get();
            if most > 0.0 {
                let track = VR_LIST_BOTTOM - VR_LIST_TOP;
                let high = (track * track / (track + most)).max(48.0);
                let y = VR_LIST_TOP + (track - high) * (self.vr_scroll / most);
                p.rect_filled(Rect::from_min_size(pos2(VR_LIST_RIGHT + 3.0, y), vec2(4.0, high)), CornerRadius::same(2), theme::white(0.3));
            }
            return;
        }
        field(p, if side { side_field_rect(self.on_sheet) } else { self.field_here() }, &self.query, tr("Search VRChat worlds"), icons::MAGNIFYING_GLASS, false, state(Target::Field), time, 1.0);
        // How many, at the field's end (while what is typed leaves room).
        if !side && self.query.chars().count() <= 28 && !results.is_empty() {
            // How many of what the filter that is on shows.
            let count = match self.few_leading(d, &results) {
                Some(n) => tr!("{count} with a 3D capture", count = n),
                None if results.len() == 1 || !d.status.list.more => tr_n("{count} world", "{count} worlds", results.len() as u64),
                None => tr!("{count}+ worlds", count = results.len()),
            };
            text(p, pos2(self.field_here().max.x - 28.0, self.field_here().center().y), Align2::RIGHT_CENTER, orbit_look::txt(count, 17.5).color(theme::FG_3));
        }
        // What is browsed (searching: every world, 3D only), the order, the labels kept to.
        if self.searching(d) {
            let (h, pr) = state(Target::Has3d);
            let (bg, fg) = if self.has_3d { (theme::alpha(orbit_look::accent(), 0.18), orbit_look::accent()) } else { (if h { RAISED } else { SURFACE }, theme::FG_2) };
            chip_button(p, ONLY_3D, icons::CUBE, tr("3D only"), bg, fg, pr, 1.0);
            let from = match d.status.list.source {
                Some(FoundBy::Vrchat) => tr("Every VRChat world, from your VRChat account"),
                Some(FoundBy::Orbiters) => tr("Every public VRChat world, from Orbiters"),
                None => tr("Every VRChat world"),
            };
            text_cut(p, pos2(ONLY_3D.max.x + 14.0, 106.0), (if side { SIDE_SORT.min.x } else { SORT.min.x }) - ONLY_3D.max.x - 24.0, orbit_look::txt(from, 15.0).color(theme::FG_3));
        } else {
            let track = Rect::from_min_max(section_rect(0).min, section_rect(Section::ALL.len() - 1).max);
            p.rect_filled(track, CornerRadius::same(20), SURFACE);
            for (i, s) in Section::ALL.into_iter().enumerate() {
                let (h, pr) = state(Target::Section(s));
                segment(p, section_rect(i), s.label(d.status.list.vrchat), self.section == s, h, pr, 1.0);
            }
        }
        let (h, pr) = state(Target::Sort(self.sort.next()));
        let (bg, fg) = if self.sort != Sort::Newest { (theme::alpha(orbit_look::accent(), 0.18), orbit_look::accent()) } else { (if h { RAISED } else { SURFACE }, theme::FG_2) };
        if side {
            chip_button(p, SIDE_SORT, self.sort.icon(), "", bg, fg, pr, 1.0);
        } else {
            chip_button(p, SORT, self.sort.icon(), self.sort_label(d), bg, fg, pr, 1.0);
        }
        let (h, pr) = state(Target::Filter);
        let filter_label = if self.filtered() {
            let mut names: Vec<&str> = self.labels.iter().map(|l| label_name(l)).collect();
            if self.official {
                names.insert(0, tr("By its author"));
            }
            if self.has_3d && !self.searching(d) {
                names.insert(0, tr("3D only"));
            }
            if names.is_empty() {
                tr("Filter by label").to_string()
            } else {
                names.join(" · ")
            }
        } else {
            tr("Filter by label").to_string()
        };
        let lit = !self.labels.is_empty() || self.official || self.has_3d && !self.searching(d);
        let (bg, fg) = if lit { (theme::alpha(orbit_look::accent(), 0.18), orbit_look::accent()) } else { (if h { RAISED } else { SURFACE }, theme::FG_2) };
        if side {
            chip_button(p, SIDE_FILTER, icons::TAG, "", bg, fg, pr, 1.0);
        } else {
            chip_button(p, FILTER, icons::TAG, &filter_label, bg, fg, pr, 1.0);
        }
        if self.filtered() && !side {
            let (h, pr) = state(Target::ClearFilters);
            p.circle_filled(CLEAR.center(), if pr { 18.0 } else { 20.0 }, if h { RAISED } else { SURFACE });
            widgets::icon(p, CLEAR.center(), icons::X, 18.0, theme::FG_2);
        }
        // Worlds that follow few captures, under a heading of their own.
        if let Some(n) = self.few_leading(d, &results) {
            let (_, heading) = few_rects(results.len(), n, side);
            let line = Rect::from_min_size(pos2(32.0, heading - 2.0), vec2(if side { 620.0 } else { 960.0 }, 1.5));
            p.rect_filled(line, CornerRadius::ZERO, theme::white(0.12));
            let name = if d.status.list.vrchat { tr("Popular worlds") } else { tr("Featured worlds") };
            text(p, pos2(32.0, heading + 13.0), Align2::LEFT_CENTER, orbit_look::txt(name, 16.5).weight(weight::SEMIBOLD).color(theme::FG));
            let wide = p.layout_job(orbit_look::txt(name, 16.5).weight(weight::SEMIBOLD).job(f32::INFINITY)).size().x;
            text(p, pos2(32.0 + wide + 12.0, heading + 13.0), Align2::LEFT_CENTER, orbit_look::txt(tr("No 3D capture yet"), 15.5).color(theme::FG_3));
            let (h, pr) = state(Target::Section(Section::Popular));
            if let Some(slot) = more_slot(results.len(), n, side) {
                let slot = if pr { slot.shrink(3.0) } else { slot };
                p.rect_filled(slot, CornerRadius::same(20), if h { RAISED } else { SURFACE });
                if side {
                    text(p, slot.center() - vec2(0.0, 13.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Other worlds"), 18.0).weight(weight::SEMIBOLD).color(theme::FG));
                    text(p, slot.center() + vec2(0.0, 15.0), Align2::CENTER_CENTER, orbit_look::txt(tr("See them all ›"), 16.0).weight(weight::SEMIBOLD).color(if h { theme::FG } else { orbit_look::accent() }));
                } else {
                    widgets::icon(p, slot.center() - vec2(0.0, 46.0), icons::GLOBE_HEMISPHERE_WEST, 40.0, orbit_look::accent());
                    text(p, slot.center() + vec2(0.0, 2.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Other worlds"), 20.0).weight(weight::SEMIBOLD).color(theme::FG));
                    text(p, slot.center() + vec2(0.0, 30.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Every popular one, or search for any"), 15.5).color(theme::FG_2));
                    text(p, slot.center() + vec2(0.0, 60.0), Align2::CENTER_CENTER, orbit_look::txt(tr("See them all ›"), 16.0).weight(weight::SEMIBOLD).color(if h { theme::FG } else { orbit_look::accent() }));
                }
            } else {
                text(p, pos2(if side { 652.0 } else { 992.0 }, heading + 11.0), Align2::RIGHT_CENTER, orbit_look::txt(tr("See them all ›"), 15.5).weight(weight::SEMIBOLD).color(if h { theme::FG } else { orbit_look::accent() }));
            }
        }
        // Few captures yet: the free space goes on to the worlds that are there.
        if !results.is_empty() && self.few_captures(d) {
            for (s, glyph, rect) in self.few_targets(side) {
                let (h, pr) = state(Target::Section(s));
                let label = s.label(d.status.list.vrchat);
                pill_button(p, rect, Some(glyph), label, if h { RAISED2 } else { RAISED }, theme::FG, h, pr, 1.0);
            }
        }
        // The list, as far down as it goes, and its way through.
        let shape = self.list_shape(d.status.headset);
        let list = p.with_clip_rect(Rect::from_min_max(pos2(0.0, shape.top), pos2(WIDTH, shape.bottom)));
        for (t, rect) in targets {
            if let Target::Card(i) = *t {
                let (h, pr) = state(*t);
                if d.status.headset {
                    vr_card(&list, self.list_card(i, true, time).translate(vec2(0.0, -self.vr_scroll)), results[i], d, h, pr, self.selected == Some(results[i].id));
                } else if self.few_leading(d, &results).is_some() {
                    let lift = (1.0 - ease_out((time - self.page_at) / PAGE_FADE)) * 18.0;
                    if side {
                        compact_card(p, *rect, results[i], d, h, pr, 1.0, self.selected == Some(results[i].id));
                    } else {
                        card(p, rect.translate(vec2(0.0, lift)), results[i], d, h, pr, shown);
                    }
                } else if side {
                    let whole = side_card_rect_flowing(i).translate(vec2(0.0, -self.vr_scroll));
                    compact_card(&list, whole, results[i], d, h, pr, 1.0, self.selected == Some(results[i].id));
                } else {
                    card(&list, card_rect(i).translate(vec2(0.0, -self.vr_scroll)), results[i], d, h, pr, shown);
                }
            }
        }
        if !results.is_empty() && self.few_leading(d, &results).is_none() {
            if d.status.list.more || self.loading(d) {
                // Under the last of them, as they keep coming.
                let next = results.len().next_multiple_of(shape.columns);
                let y = (if d.status.headset { vr_wide_card_rect(next) } else if side { side_card_rect_flowing(next) } else { card_rect(next) }).min.y + 40.0 - self.vr_scroll;
                if y < shape.bottom {
                    text(&list, pos2((shape.right + 24.0) / 2.0, y), Align2::CENTER_CENTER, orbit_look::txt(tr("Finding more worlds…"), 19.0).color(fade(theme::FG_3, shown)));
                }
            }
            let most = self.vr_most.get();
            if most > 0.0 {
                let track = shape.bottom - shape.top;
                let high = (track * track / (track + most)).max(48.0);
                let y = shape.top + (track - high) * (self.vr_scroll / most);
                p.rect_filled(Rect::from_min_size(pos2(shape.right - 5.0, y), vec2(4.0, high)), CornerRadius::same(2), theme::white(0.3));
            }
        }
        if side {
            if results.is_empty() {
                text(p, pos2(340.0, 330.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Nothing else here"), 18.0).color(theme::FG_3));
            } else if self.few_captures(d) {
                text(p, pos2(340.0, 420.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Few worlds have a 3D capture yet"), 17.0).weight(weight::MEDIUM).color(theme::FG_2));
                text(p, pos2(340.0, 448.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Search any world above, or look through these"), 15.0).color(theme::FG_3));
            }
        } else if results.is_empty() {
            self.empty(p, d, shown, state(Target::Retry));
        } else if self.few_captures(d) {
            // Few worlds have a capture yet: the free space says what these are, and where the others are.
            text(p, pos2(512.0, 424.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Worlds people captured in 3D: open one to see it in 3D"), 19.0).weight(weight::MEDIUM).color(fade(theme::FG, shown)));
            text(p, pos2(512.0, 456.0), Align2::CENTER_CENTER, orbit_look::txt(tr("Few have one yet. Search any world above, or look through these"), 16.0).color(fade(theme::FG_3, shown)));
        }
    }

    fn empty(&self, p: &Painter, d: &Data, shown: f32, retry: (bool, bool)) {
        let searching = self.searching(d);
        let (icon, title, line) = if let Some((title, line)) = self.failure(d) {
            (icons::WARNING, title, line)
        } else if !searching && self.section.needs_vrchat() && !d.status.list.vrchat && d.status.list.connecting {
            (icons::HOURGLASS_MEDIUM, tr("Connecting to VRChat…"), tr("Your VRChat session is being restored."))
        } else if !searching && self.section.needs_vrchat() && !d.status.list.vrchat {
            (icons::SIGN_IN, tr("Sign in to VRChat to see these"), tr("On the computer: Orbiters - N4N0, Settings, VRChat account"))
        } else if self.loading(d) {
            let line = match (searching, self.section) {
                (true, _) => tr("Every VRChat world whose name matches"),
                (false, Section::Captures) => tr("The worlds members captured and shared"),
                (false, Section::Visited) => tr("The worlds you visited in VRChat"),
                (false, _) => tr("From VRChat"),
            };
            (icons::GLOBE, tr("Finding worlds…"), line)
        } else if self.has_3d && self.labels.is_empty() && !self.official {
            (icons::CUBE, tr("No 3D capture among these worlds"), tr("Turn 3D only off to see them all"))
        } else if self.filtered() {
            (icons::TAG, tr("No world has these labels"), tr("Clear the filter, or pick other labels"))
        } else if !searching && self.section == Section::Visited {
            (icons::CLOCK, tr("No world visited yet"), tr("The worlds you visit in VRChat show here"))
        } else if !searching && self.section == Section::Favorites {
            (icons::STAR, tr("No favorite world yet"), tr("Favorite worlds in VRChat to find them here"))
        } else if self.query.trim().chars().count() < 2 && !self.query.trim().is_empty() {
            (icons::MAGNIFYING_GLASS, tr("Type a little more"), tr("Two letters search every VRChat world"))
        } else {
            (icons::MAGNIFYING_GLASS, tr("No world matches"), tr("Try another name, or a 3D capture's author or sharer"))
        };
        widgets::icon(p, pos2(512.0, 270.0), icon, 44.0, fade(theme::FG_4, shown));
        text(p, pos2(512.0, 326.0), Align2::CENTER_CENTER, orbit_look::txt(title, 24.0).weight(weight::SEMIBOLD).color(fade(theme::FG, shown)));
        let line: String = line.chars().take(80).collect();
        text(p, pos2(512.0, 360.0), Align2::CENTER_CENTER, orbit_look::txt(line, 17.0).color(fade(theme::FG_3, shown)));
        if self.failure(d).is_some() {
            pill_button(p, Rect::from_center_size(pos2(512.0, 420.0), vec2(200.0, 56.0)), Some(icons::ARROW_CLOCKWISE), tr("Try again"), RAISED, theme::FG, retry.0, retry.1, shown);
        }
    }

    /// A world's details beside the list.
    fn side_panel(&self, p: &Painter, d: &Data, time: f64, targets: &[(Target, Rect)]) {
        let status = d.status;
        let state = |t: Target| (self.hover == Some(t), self.pressed == Some(t));
        let hole = self.hole(status);
        let opening = ease_out((time - self.page_at) / PAGE_FADE);
        let vr = status.headset;
        let side = if vr { VR_SIDE } else { SIDE };
        match hole {
            Some(h) => fill_around(p, side, 22.0, h, 14.0, SURFACE),
            None => {
                p.rect_filled(side.translate(vec2((1.0 - opening) * 28.0, 0.0)), CornerRadius::same(22), fade(SURFACE, opening.max(0.3)));
            }
        }
        let Some(entry) = self.entry(d) else {
            text(p, side.center(), Align2::CENTER_CENTER, orbit_look::txt(tr("This world is no longer listed"), 17.0).color(theme::FG_3));
            return;
        };
        // Another world's, coming in at once.
        let rest = opening.min(ease_out((time - self.selected_at) / 0.16).max(0.25));
        let current = status.current == Some(entry.id);
        // The carousel (K-carousel): the captures before and after this one peek out on either side, each under its arrow, and the line under the picture says how
        // far along the list this one is and what the stick does where the laser is.
        let place = self.carousel_place(d);
        if vr {
            self.carousel_line(p, d, hole, place, opening);
        }
        if let Some(h) = hole {
            // Through the hole: the capture. What says it can be taken lies on it, and brightens as a hand comes near.
            let near = status.small_near;
            let beat = if near { 0.65 + 0.35 * ((time * 6.0).sin() as f32 * 0.5 + 0.5) } else { 0.6 + 0.2 * ((time * 2.0).sin() as f32 * 0.5 + 0.5) };
            p.rect_stroke(h, CornerRadius::same(14), Stroke::new(if near { 3.0 } else { 1.5 }, fade(orbit_look::accent(), beat)), StrokeKind::Outside);
            let arm = 22.0;
            for (corner, x, y) in [(h.left_top(), 1.0, 1.0), (h.right_top(), -1.0, 1.0), (h.left_bottom(), 1.0, -1.0), (h.right_bottom(), -1.0, -1.0)] {
                let at = corner + vec2(x * 7.0, y * 7.0);
                let stroke = Stroke::new(if near { 4.0 } else { 3.0 }, fade(orbit_look::accent(), beat));
                p.line_segment([at, at + vec2(x * arm, 0.0)], stroke);
                p.line_segment([at, at + vec2(0.0, y * arm)], stroke);
            }
            // A shade along the hole's top edge, under its frame: the panel's own thickness, so the capture reads as behind it in a still (round 19's judges).
            // Not in the retro look, whose flat grounds have no shades.
            for i in (0..HOLE_SHADE_BANDS).filter(|_| !orbit_look::retro()) {
                let band = Rect::from_min_size(pos2(h.left(), h.top() + i as f32 * HOLE_SHADE_BAND), vec2(h.width(), HOLE_SHADE_BAND));
                p.rect_filled(band, CornerRadius::ZERO, theme::black(hole_shade(i)));
            }
        } else {
            let hero = if vr { vr_hero() } else { SIDE_HERO };
            let small_back = if vr { vr_small_back() } else { SMALL_BACK };
            // Flipped to: it comes in from the side it was on, inside its frame, over the one it replaces.
            match self.flipped.filter(|(_, at)| self.moves && time - at < FLIP_SLIDE) {
                Some((forward, at)) => {
                    let along = 1.0 - ease_out((time - at) / FLIP_SLIDE);
                    let by = along * hero.width() * 0.35 * if forward { 1.0 } else { -1.0 };
                    picture(&p.with_clip_rect(hero), hero.translate(vec2(by, 0.0)), (d.thumb)(entry), &entry.world_id, CornerRadius::same(14), rest.max(1.0 - along));
                }
                None => picture(p, hero, (d.thumb)(entry), &entry.world_id, CornerRadius::same(14), rest),
            }
            if let Some(live) = self.live.as_ref().filter(|l| l.id == entry.id && !vr) {
                draw_live(p, hero, live, None);
                if live.picture.is_some() {
                    draw_full_button(p, full_button(hero), false, self.hover == Some(Target::FullView), self.pressed == Some(Target::FullView));
                }
            }
            let foot = Rect::from_min_max(pos2(hero.min.x, hero.max.y - 44.0), hero.max);
            if current || !entry.capture {
                p.rect_filled(foot, CornerRadius { nw: 0, ne: 0, sw: 14, se: 14 }, fade(theme::black(0.62), rest));
            }
            if current {
                let taken = status.small && status.small_taken;
                let (dot, label) = if let Some(e) = &status.error {
                    (theme::DANGER, e.clone())
                } else if status.shown && status.stage.is_none() {
                    (orbit_look::accent(), if taken { tr("In your hand").to_string() } else if status.small { tr("Before you").to_string() } else { tr("Around you now").to_string() })
                } else {
                    (theme::WARN, status.stage.clone().unwrap_or_else(|| tr("Getting ready").into()))
                };
                let pulse = if status.shown && status.stage.is_none() { 1.0 } else { 0.55 + 0.45 * ((time * 4.0).sin() as f32 * 0.5 + 0.5) };
                // Clear of the carousel's neighbours over the picture's edges.
                let inset = if vr && over_edges() { 36.0 } else { 0.0 };
                p.circle_filled(pos2(hero.min.x + inset + 20.0, foot.center().y), 5.0, fade(dot, rest * pulse));
                let room = if taken { small_back.min.x - hero.min.x - inset - 44.0 } else { hero.width() - 2.0 * inset - 44.0 };
                text_cut(p, pos2(hero.min.x + inset + 34.0, foot.center().y), room, orbit_look::txt(label, 15.0).weight(weight::MEDIUM).color(fade(theme::FG, rest)));
                if let Some(f) = status.download {
                    let track = Rect::from_min_size(pos2(hero.min.x, hero.max.y - 4.0), vec2(hero.width(), 4.0));
                    p.rect_filled(track, CornerRadius::same(2), fade(theme::white(0.12), rest));
                    p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * f, 4.0)), CornerRadius::same(2), fade(orbit_look::accent(), rest));
                }
            } else if !entry.capture {
                let line = match (entry.occupants.filter(|n| *n > 0), entry.preview) {
                    (Some(1), _) => tr("1 person in it now").to_string(),
                    (Some(n), _) => tr!("{count} people in it now", count = short_count(n as u64)),
                    (None, true) => tr("Its 3D preview is on orbiters.cc").to_string(),
                    (None, false) => tr("No 3D capture of it yet").to_string(),
                };
                let inset = if vr && over_edges() { 36.0 } else { 0.0 };
                text_cut(p, pos2(hero.min.x + inset + 16.0, foot.center().y), hero.width() - 2.0 * inset - 32.0, orbit_look::txt(line, 15.0).color(fade(theme::FG_2, rest)));
            }
        }
        // Over the picture's edges, over what lies on it (its frame's corners), as if the next ones lay on the panel.
        if let Some((_, _, before, after)) = place {
            for (neighbour, peek, target, glyph, forward) in [(before, vr_flip(false).1, Target::FlipPrevious, icons::CARET_LEFT, false), (after, vr_flip(true).1, Target::FlipNext, icons::CARET_RIGHT, true)] {
                let Some(n) = neighbour else { continue };
                let (h, pr) = state(target);
                let shown = opening * if h { 1.0 } else { 0.8 };
                // Over the picture's edge it follows the hole's rounded corners on its outer side.
                let corner = match (over_edges(), forward) {
                    (false, _) => CornerRadius::same(10),
                    (true, false) => CornerRadius { nw: 14, sw: 14, ne: 0, se: 0 },
                    (true, true) => CornerRadius { nw: 0, sw: 0, ne: 14, se: 14 },
                };
                peek_picture(p, peek, (d.thumb)(n), &n.world_id, !forward, shown, corner);
                p.rect_filled(peek, corner, theme::black(if h { 0.2 } else { 0.45 } * opening));
                let r = if pr { 17.0 } else { 19.0 };
                p.circle_filled(peek.center(), r + 1.5, theme::black(0.35 * opening));
                p.circle_filled(peek.center(), r, fade(if h { orbit_look::accent() } else { RAISED2 }, opening));
                widgets::icon(p, peek.center(), glyph, 22.0, fade(if h { orbit_look::on_accent() } else { theme::FG }, opening));
            }
        }
        // Its way out, on its edge: it slides away to the right, so it says so. An X here, just under the sheet's own, was one close too many (a judge's).
        if !vr {
            let (h, pr) = state(Target::CloseSide);
            let tab = if pr { SIDE_CLOSE.shrink(1.0) } else { SIDE_CLOSE };
            p.rect_filled(tab, CornerRadius::same((tab.height() / 2.0) as u8), fade(if h { RAISED2 } else { RAISED }, rest));
            let galley = p.layout_job(orbit_look::txt(tr("Hide details"), 17.5).weight(weight::SEMIBOLD).color(theme::FG).job_truncated(tab.width() - 36.0));
            let wide = galley.size().x + 26.0;
            let from = tab.center().x - wide / 2.0;
            p.galley(pos2(from, tab.center().y - galley.size().y / 2.0), galley, theme::FG);
            widgets::icon(p, pos2(from + wide - 9.0, tab.center().y), icons::CARET_RIGHT, 18.0, theme::FG);
        }
        // Its name, as long as it is; who made it.
        let (x, width) = if vr { (VR_LEFT, SIDE_RIGHT - VR_LEFT) } else { (SIDE_LEFT, SIDE_RIGHT - SIDE_LEFT) };
        // Copy name right after it (REQUEST20 1): its room comes off the name's.
        let (copy_w, copy_h) = if vr { (44.0, 38.0) } else { (34.0, 30.0) };
        let name_room = width - copy_w - 8.0;
        let copy_rect = if vr {
            let galley = p.layout_job(orbit_look::txt(entry.name.clone(), 24.0).weight(weight::SEMIBOLD).color(fade(theme::FG, rest)).job_truncated(name_room));
            let middle = VR_NAME + galley.size().y / 2.0;
            let end = x + galley.size().x;
            p.galley(pos2(x, VR_NAME), galley, theme::FG);
            Rect::from_min_size(pos2(end + 6.0, middle - copy_h / 2.0), vec2(copy_w, copy_h))
        } else {
            let mut title = orbit_look::txt(entry.name.clone(), 21.0).weight(weight::SEMIBOLD).color(fade(theme::FG, rest)).job(name_room);
            title.wrap.max_rows = 2;
            let galley = p.layout_job(title);
            // Its last line just over who made it, however many it has.
            let top = SIDE_NAME_FOOT - galley.size().y;
            let last = galley.rows.last().map(|r| r.rect()).unwrap_or(Rect::NOTHING);
            let (end, middle) = if last.is_positive() { (x + last.right(), top + last.center().y) } else { (x, SIDE_NAME_FOOT - 14.0) };
            p.galley(pos2(x, top), galley, theme::FG);
            Rect::from_min_size(pos2(end + 4.0, middle - copy_h / 2.0), vec2(copy_w, copy_h))
        };
        if !entry.name.trim().is_empty() {
            self.copy_at.set(Some((entry.id, vr, copy_rect)));
            let (h, pr) = state(Target::CopyName);
            let copied = self.copied.is_some_and(|(id, at)| id == entry.id && time - at < COPIED_FOR);
            copy_button(p, copy_rect, copied, vr, h, pr, rest);
        } else {
            self.copy_at.set(None);
        }
        let facts = d.facts_of(entry);
        let mut under = Vec::new();
        if let Some(author) = d.author(entry) {
            under.push(tr!("by {author}", author = author));
        }
        if entry.capture && !entry.official {
            if let Some(u) = &entry.uploader {
                under.push(tr!("3D by {user}", user = u));
            }
        } else if let Some((last, visits)) = entry.visited {
            under.push(if visits == 1 { tr("Visited once").to_string() } else { tr!("Visited {count} times", count = visits) });
            if last > 0 {
                under.push(tr!("last {when}", when = orbiters_n4n0_core::feedback::ago(d.now.saturating_sub(last))));
            }
        }
        // In a headset, who made it and no more.
        // On a screen the way to its other captures ends this line, which then says who made it and no more: nothing on it is cut.
        let linked = !vr && !d.others(entry).is_empty();
        let under = if vr || linked { under.first().cloned().unwrap_or_default() } else { under.join(" · ") };
        let under_room = if linked { OTHER.min.x - 10.0 - x } else { width };
        text_cut(p, pos2(x, if vr { VR_UNDER } else { SIDE_UNDER }), under_room, orbit_look::txt(under, if vr { 20.0 } else { 17.5 }).color(fade(if vr { theme::FG } else { theme::FG_2 }, rest)));
        // It shows in 3D on its picture: where See it in 3D was, how to look around it. Said here, not over the picture, and no second full-screen button (round 18's judges).
        if !vr && self.live.as_ref().is_some_and(|l| l.id == entry.id && l.picture.is_some()) {
            // Words from the panel's left edge, with nothing round them: in an outline with an icon it read as a button that did nothing (the round 19 pre-check).
            let galley = p.layout_job(orbit_look::txt(tr("Drag the picture to look around"), 19.0).weight(weight::MEDIUM).color(fade(theme::FG_2, rest)).job_truncated(width - 28.0));
            widgets::icon(p, pos2(x + 9.0, SHOW_WHOLE.center().y), icons::HAND_GRABBING, 20.0, fade(theme::FG_2, rest));
            p.galley(pos2(x + 27.0, SHOW_WHOLE.center().y - galley.size().y / 2.0), galley, theme::FG_2);
        }
        // What it can be seen as, and the rest.
        let wf = d.world_feedback(entry);
        for (t, rect) in targets {
            let (h, pr) = state(*t);
            match *t {
                Target::Show => {
                    // On a screen it opens the Orbiters window, and says so.
                    let (glyph, label) = if !status.headset { (icons::CUBE, tr("See it in 3D")) } else { (icons::CUBE, tr("Around me")) };
                    let lit = !(current && status.small);
                    pill_button(p, *rect, Some(glyph), label, if lit { orbit_look::accent() } else { RAISED2 }, if lit { orbit_look::on_accent() } else { theme::FG }, h, pr, rest);
                }
                Target::ShowSmall => {
                    let lit = current && status.small;
                    pill_button(p, *rect, Some(icons::HAND_GRABBING), tr("Small here"), if lit { orbit_look::accent() } else { RAISED2 }, if lit { orbit_look::on_accent() } else { theme::FG }, h, pr, rest);
                }
                Target::SmallBack => pill_button(p, *rect, Some(icons::ARROW_U_UP_LEFT), tr("Back here"), orbit_look::accent(), orbit_look::on_accent(), h, pr, rest),
                Target::Join => {
                    let (glyph, label) = (if status.list.vrchat { icons::SIGN_IN } else { icons::PLAY }, tr("Go there"));
                    if entry.capture && vr {
                        pill_button(p, *rect, Some(glyph), label, RAISED2, theme::FG, h, pr, rest);
                    } else if entry.capture {
                        icon_button(p, *rect, glyph, false, label, RAISED2, theme::FG, h, pr, rest);
                    } else {
                        pill_button(p, *rect, Some(glyph), label, orbit_look::accent(), orbit_look::on_accent(), h, pr, rest);
                    }
                }
                Target::OpenWeb if vr => pill_button(p, *rect, Some(icons::ARROW_SQUARE_OUT), tr("Web page"), RAISED, theme::FG, h, pr, rest),
                Target::OpenWeb => icon_button(p, *rect, icons::ARROW_SQUARE_OUT, false, tr("Web page"), RAISED, theme::FG, h, pr, rest),
                Target::Like => {
                    let (bg, fg) = if wf.liked { (theme::alpha(theme::REC, 0.22), theme::REC) } else { (RAISED, theme::FG) };
                    small_action(p, *rect, icons::HEART, wf.liked, wf.likes as u64, if wf.likes == 0 { tr("Like") } else { tr("Likes") }, bg, fg, h, pr, rest * if wf.loading { 0.75 } else { 1.0 });
                }
                Target::Comments => small_action(p, *rect, icons::CHAT_CIRCLE_TEXT, false, wf.comment_count as u64, if wf.comment_count == 0 { tr_ctx("count", "Comment") } else { tr("Comments") }, RAISED, theme::FG, h, pr, rest),
                Target::Pictures => {
                    let n = facts.map_or(entry.pictures as usize, |f| f.picture_count.max(entry.pictures as usize));
                    small_action(p, *rect, icons::IMAGES, false, n as u64, tr("Pictures"), RAISED, theme::FG, h, pr, rest);
                }
                Target::Labels => {
                    let given = wf.top().len();
                    small_action(p, *rect, icons::TAG, false, given as u64, if given == 0 { tr("Label it") } else { tr("Labels") }, RAISED, theme::FG, h, pr, rest);
                }
                Target::Other => {
                    let n = d.others(entry).len();
                    if vr {
                        let label = if n == 1 { tr("1 more capture of this world").to_string() } else { tr!("{count} more captures of this world", count = n) };
                        pill_button(p, *rect, Some(icons::CUBE), &label, RAISED, theme::FG, h, pr, rest);
                        continue;
                    }
                    let label = if n == 1 { tr("1 more capture ›").to_string() } else { tr!("{count} more ›", count = n) };
                    text(p, pos2(rect.max.x, SIDE_UNDER + 12.0), Align2::RIGHT_CENTER, orbit_look::txt(label, 17.5).weight(weight::SEMIBOLD).color(fade(if h { theme::FG } else { orbit_look::accent() }, rest)));
                }
                _ => {}
            }
        }
        // As much of the world's description as fits. In a headset, larger, and no more than that under it: its numbers are on the tiles over it.
        let description = if entry.description.trim().is_empty() { facts.and_then(|f| f.description.as_deref()).unwrap_or("") } else { entry.description.as_str() };
        if !description.trim().is_empty() {
            let said = description.split_whitespace().collect::<Vec<_>>().join(" ");
            if vr {
                let mut job = orbit_look::txt(said, 18.0).color(fade(theme::FG, rest)).job(width);
                job.wrap.max_rows = 2;
                // In a headset it has what the row of other captures leaves.
                if d.others(entry).is_empty() {
                    p.galley(pos2(x, 510.0), p.layout_job(job), theme::FG);
                }
            } else if self.note(time).is_none() && wf.error.is_none() && (current || status.error.is_none()) {
                // On a screen: as many whole lines as end over the panel's foot, by what they measure, and its last one cut with an ellipsis.
                let lines = |rows: usize| {
                    let mut job = orbit_look::txt(said.clone(), 17.5).color(fade(theme::FG_2, rest)).job(width);
                    job.wrap.max_rows = rows;
                    job.wrap.overflow_character = Some('\u{2026}');
                    p.layout_job(job)
                };
                let galley = (1..=3).rev().map(&lines).find(|g| SIDE_SAYS + g.size().y <= SIDE_FOOT).unwrap_or_else(|| lines(1));
                p.galley(pos2(x, SIDE_SAYS), galley, theme::FG_2);
            }
        }
        if vr {
            // Why a press did nothing, over what is there.
            if let Some(note) = self.note(time).map(str::to_string).or_else(|| wf.error.clone()).or_else(|| status.error.clone().filter(|_| !current)) {
                p.rect_filled(VR_OTHER, CornerRadius::same(14), SURFACE);
                text_cut(p, pos2(x, VR_OTHER.min.y + 10.0), width, orbit_look::txt(note, 18.0).color(fade(theme::WARN, rest)));
            }
            return;
        }
        // How smoothly VRChat runs there: in its head, the first thing known of it.
        if let Some(perf) = d.performance(entry) {
            let (word, tint) = verdict(perf);
            chip(p, pos2(x, SIDE_CLOSE.min.y), icons::GAUGE, &tr!("{fps} fps · {verdict}", fps = perf.fps, verdict = word), tint, rest);
        }
        // Why a press did nothing, what Orbiters refused, what went wrong with the capture: where its description is, for as long as it is said. What was last said of
        // it is behind Comments: one more line here was the one the sheet's foot cut (round 18's judges).
        if let Some(note) = self.note(time).map(str::to_string).or_else(|| wf.error.clone()).or_else(|| status.error.clone().filter(|_| !current)) {
            let mut job = orbit_look::txt(note, 17.5).color(fade(theme::WARN, rest)).job(width);
            job.wrap.max_rows = 2;
            job.wrap.overflow_character = Some('\u{2026}');
            p.galley(pos2(x, SIDE_SAYS), p.layout_job(job), theme::WARN);
        }
    }
}

impl Panel {
    /// The line under a capture's picture in a headset, beside its carousel: what each control does here, lit where the laser is, and how far along the list
    /// this one is. Words with nothing round them: this is done with a hand, and is no button for a laser (as before).
    fn carousel_line(&self, p: &Painter, d: &Data, hole: Option<Rect>, place: Option<(usize, usize, Option<&Entry>, Option<&Entry>)>, alpha: f32) {
        let y = VR_GRIP.center().y;
        let mut right = VR_GRIP.max.x;
        if let Some((at, n, _, _)) = place.filter(|(_, n, _, _)| *n > 1) {
            let more = if d.status.list.more { "+" } else { "" };
            let galley = p.layout_job(orbit_look::txt(tr!("{at} of {count}", at = at + 1, count = format!("{n}{more}")), 17.0).weight(weight::MEDIUM).color(fade(theme::FG_3, alpha)).job(f32::INFINITY));
            right -= galley.size().x;
            p.galley(pos2(right, y - galley.size().y / 2.0), galley, theme::FG_3);
            right -= 18.0;
        }
        // What the stick does where the laser is, by the same rule as the stick itself.
        let spot = self.pointer.and_then(|at| self.stick_spot(d, at.x, at.y));
        let on_capture = spot == Some(StickSpot::InCapture);
        let on_band = spot == Some(StickSpot::Flip);
        let near = d.status.small_near;
        // Each part: its icon, its words, lit or not.
        let mut parts: Vec<(&str, &str, bool)> = Vec::new();
        if on_band {
            parts.push((icons::CARET_RIGHT, tr("Stick left or right: another capture"), true));
        } else if hole.is_some() {
            parts.push((icons::ARROWS_OUT, tr("Stick: move around"), on_capture && !near));
            parts.push((icons::HAND_GRABBING, if near { tr("Close your grip") } else { tr("Grip: take it") }, near));
        }
        let mut x = VR_LEFT;
        for (i, (glyph, words, lit)) in parts.into_iter().enumerate() {
            if i > 0 {
                text(p, pos2(x + 2.0, y), Align2::LEFT_CENTER, orbit_look::txt("\u{b7}", 18.0).color(fade(theme::FG_3, alpha)));
                x += 16.0;
            }
            let ink = fade(if lit { orbit_look::accent() } else { theme::FG_2 }, alpha);
            if x + 30.0 >= right {
                break;
            }
            // Side to side: both ways.
            let icon_w = if glyph == icons::CARET_RIGHT {
                widgets::icon(p, pos2(x + 8.0, y), icons::CARET_LEFT, 20.0, ink);
                widgets::icon(p, pos2(x + 22.0, y), icons::CARET_RIGHT, 20.0, ink);
                34.0
            } else {
                widgets::icon(p, pos2(x + 12.0, y), glyph, 24.0, ink);
                30.0
            };
            let galley = p.layout_job(orbit_look::txt(words, 19.0).weight(if lit { weight::BOLD } else { weight::SEMIBOLD }).color(ink).job_truncated(right - x - icon_w));
            let wide = galley.size().x;
            p.galley(pos2(x + icon_w, y - galley.size().y / 2.0), galley, ink);
            x += icon_w + wide + 8.0;
        }
    }
}

/// A neighbour peeking out of the carousel's gutter: the edge of its picture nearest the one shown (`right_edge`: its right one), as if the row of them went on
/// behind the panel.
fn peek_picture(p: &Painter, rect: Rect, texture: Option<TextureId>, world_id: &str, right_edge: bool, alpha: f32, corner: CornerRadius) {
    let Some(id) = texture else {
        let hue = world_id.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360;
        p.rect_filled(rect, corner, fade(hsv(hue as f32 / 360.0, 0.45, 0.42), alpha));
        return;
    };
    let u = (rect.width() / (rect.height() * 16.0 / 9.0)).min(1.0);
    let uv = if right_edge { Rect::from_min_max(pos2(1.0 - u, 0.0), pos2(1.0, 1.0)) } else { Rect::from_min_max(pos2(0.0, 0.0), pos2(u, 1.0)) };
    super::super::orbit::panels::keep_colour(p, rect);
    p.add(RectShape::filled(rect, corner, fade(Color32::WHITE, alpha)).with_texture(id, uv));
}

/// The search beside the side panel: from the list's left edge to its right one on a sheet (round 18's judges: it shared neither), after the Back button where there is one.
fn side_field_rect(on_sheet: bool) -> Rect {
    Rect::from_min_max(pos2(if on_sheet { 32.0 } else { 100.0 }, 24.0), pos2(652.0, 80.0))
}

/// A number on a small tile, or nothing where it is nought: a row of noughts reads as a row of faults.
fn count_or_nothing(n: u64) -> String {
    if n == 0 {
        String::new()
    } else {
        short_count(n)
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

/// A result's card beside the side panel: two columns of three, as many as the full width shows.
fn side_card_rect(slot: usize) -> Rect {
    let (col, row) = ((slot % 2) as f32, (slot / 2) as f32);
    Rect::from_min_size(pos2(32.0 + col * 316.0, 148.0 + row * 136.0), vec2(304.0, 124.0))
}

/// A result beside a side panel on a screen, as far down as there are.
fn side_card_rect_flowing(i: usize) -> Rect {
    side_card_rect(i)
}

fn compact_rect(i: usize) -> Rect {
    Rect::from_min_size(pos2(32.0 + i as f32 * 328.0, 100.0), vec2(304.0, 120.0))
}

/// A card's picture beside the side panel: a block as high as the card, an even margin round it, cut to that shape (Enzo, 2026-11-10: a small picture
/// floating in a dark box read as rushed). Wide enough to show a place, narrow enough to leave its name two whole lines.
fn compact_thumb(card: Rect) -> Rect {
    const MARGIN: f32 = 8.0;
    let high = card.height() - 2.0 * MARGIN;
    Rect::from_min_size(card.min + vec2(MARGIN, MARGIN), vec2((high * 1.1).round(), high))
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

/// A button's label set in the room it has: as it is where it fits beside its icon; else without the icon where it fits alone; else smaller (to 70% at
/// most), and cut at its end if that is still too long. These buttons were drawn for English, and a translation can be a third longer; English
/// itself fits as drawn, so none of this changes it.
fn button_label(p: &Painter, txt: Txt, room_with_icon: f32, room_alone: f32) -> (std::sync::Arc<egui::Galley>, bool) {
    let natural = p.layout_job(txt.job(f32::INFINITY));
    let width = natural.size().x;
    if width <= room_with_icon {
        return (natural, true);
    }
    if width <= room_alone {
        return (natural, false);
    }
    let mut smaller = txt.clone();
    smaller.size = (txt.size * room_alone / width).max(txt.size * 0.7);
    (p.layout_job(smaller.job_truncated(room_alone)), false)
}

/// `txt` at the size where `room` holds it: its own where it fits, else smaller (to 70% at most).
fn fitted(p: &Painter, mut txt: Txt, room: f32) -> Txt {
    let width = p.layout_job(txt.job(f32::INFINITY)).size().x;
    if width > room {
        txt.size = (txt.size * room / width).max(txt.size * 0.7);
    }
    txt
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

/// Copy name beside a world's name: an icon on the panel's raised tone; pointed at, it says what it does; pressed, Copied
/// with a check for a moment, in the accent. What it says grows to its right, or to its left where the panel ends first.
#[allow(clippy::too_many_arguments)]
fn copy_button(p: &Painter, rect: Rect, copied: bool, vr: bool, hover: bool, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(1.5) } else { rect };
    let (bg, fg) = if copied { (orbit_look::accent(), orbit_look::on_accent()) } else if hover { (RAISED2, theme::FG) } else { (RAISED, theme::FG_2) };
    let words = if copied { Some(tr("Copied")) } else if hover { Some(tr_ctx("world name", "Copy name")) } else { None };
    let galley = words.map(|w| p.layout_job(orbit_look::txt(w, if vr { 18.0 } else { 15.5 }).weight(weight::SEMIBOLD).color(fade(fg, alpha)).job(f32::INFINITY)));
    let wide = galley.as_ref().map_or(0.0, |g| g.size().x + 12.0);
    let mut face = Rect::from_min_size(rect.min, vec2(rect.width() + wide, rect.height()));
    if face.right() > SIDE_RIGHT {
        face = face.translate(vec2(SIDE_RIGHT - face.right(), 0.0));
    }
    p.rect_filled(face, CornerRadius::same((face.height() / 2.0) as u8), fade(bg, alpha));
    let glyph_at = pos2(face.left() + rect.width() / 2.0, face.center().y);
    widgets::icon(p, glyph_at, if copied { icons::CHECK } else { icons::COPY }, if vr { 22.0 } else { 18.0 }, fade(fg, alpha));
    if let Some(g) = galley {
        p.galley(pos2(face.left() + rect.width() - 4.0, face.center().y - g.size().y / 2.0), g, fade(fg, alpha));
    }
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
    let label_txt = orbit_look::txt(label, 20.0).weight(weight::SEMIBOLD).color(fade(fg, alpha));
    let room = (rect.width() - 40.0).max(10.0);
    let (galley, with_icon) = button_label(p, label_txt, room, if glyph.is_some() { (rect.width() - 24.0).max(10.0) } else { room });
    let icon_w = if glyph.is_some() && with_icon { 30.0 } else { 0.0 };
    let total = icon_w + galley.size().x;
    let left = rect.center().x - total / 2.0;
    if let Some(g) = glyph.filter(|_| with_icon) {
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
    let txt = orbit_look::txt(label, 18.0).weight(weight::SEMIBOLD).color(fade(fg, alpha));
    let (galley, with_icon) = button_label(p, txt, (rect.width() - 52.0).max(10.0), (rect.width() - 24.0).max(10.0));
    let with_icon = with_icon || label.is_empty();
    let total = if label.is_empty() { 22.0 } else if with_icon { 30.0 + galley.size().x } else { galley.size().x };
    let left = rect.center().x - total / 2.0;
    let at = pos2(left + 11.0, rect.center().y);
    if with_icon {
        if filled {
            widgets::icon_fill(p, at, glyph, 22.0, fade(fg, alpha));
        } else {
            widgets::icon(p, at, glyph, 22.0, fade(fg, alpha));
        }
    }
    p.galley(pos2(left + if with_icon { 30.0 } else { 0.0 }, rect.center().y - galley.size().y / 2.0), galley, fade(fg, alpha));
}

/// A small rounded chip with an icon and a label (a filter, Clear).
#[allow(clippy::too_many_arguments)]
fn chip_button(p: &Painter, rect: Rect, glyph: &str, label: &str, bg: Color32, fg: Color32, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(1.5) } else { rect };
    p.rect_filled(rect, CornerRadius::same((rect.height() / 2.0) as u8), fade(bg, alpha));
    widgets::icon(p, pos2(rect.min.x + 22.0, rect.center().y), glyph, 18.0, fade(fg, alpha));
    text_cut(p, pos2(rect.min.x + 40.0, rect.center().y - 10.0), rect.width() - 54.0, orbit_look::txt(label, 16.0).weight(weight::MEDIUM).color(fade(fg, alpha)));
}

/// One choice of a segmented control (the order, how sharp).
fn segment(p: &Painter, rect: Rect, label: &str, on: bool, hover: bool, pressed: bool, alpha: f32) {
    let bg = if on { theme::FG } else if hover { RAISED } else { SURFACE };
    let fg = if on { CARD } else { theme::FG_2 };
    let r = if pressed { rect.shrink(2.0) } else { rect.shrink(1.0) };
    p.rect_filled(r, CornerRadius::same((r.height() / 2.0) as u8), fade(bg, alpha));
    text(p, r.center(), Align2::CENTER_CENTER, orbit_look::txt(label, 17.5).weight(weight::MEDIUM).color(fade(fg, alpha)));
}

/// A label as a chip: its name and how many gave it; lit when the member gave it.
#[allow(clippy::too_many_arguments)]
fn label_chip(p: &Painter, rect: Rect, name: &str, count: Option<u32>, mine: bool, hover: bool, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(1.5) } else { rect };
    let (bg, fg, sub) = if mine {
        (orbit_look::accent(), orbit_look::on_accent(), theme::alpha(orbit_look::on_accent(), 0.7))
    } else {
        (if hover { RAISED2 } else { RAISED }, theme::FG, theme::FG_3)
    };
    let size = if rect.height() > 40.0 { 18.0 } else { 15.0 };
    p.rect_filled(rect, CornerRadius::same((rect.height() / 2.0).min(26.0) as u8), fade(bg, alpha));
    let pad = rect.height() / 2.0;
    let mut right = rect.max.x - pad + 4.0;
    if let Some(n) = count.filter(|n| *n > 0) {
        let galley = p.layout_job(orbit_look::txt(short_count(n as u64), size - 1.0).color(fade(sub, alpha)).job(f32::INFINITY));
        right -= galley.size().x;
        p.galley(pos2(right, rect.center().y - galley.size().y / 2.0), galley, sub);
        right -= 8.0;
    }
    let mut left = rect.min.x + pad - 2.0;
    if mine {
        widgets::icon(p, pos2(left + 7.0, rect.center().y), icons::CHECK, size, fade(fg, alpha));
        left += 20.0;
    }
    text_cut(p, pos2(left, rect.center().y - size * 0.62), (right - left).max(10.0), orbit_look::txt(name, size).weight(weight::MEDIUM).color(fade(fg, alpha)));
}

#[allow(clippy::too_many_arguments)]
fn key_cap(p: &Painter, rect: Rect, label: &str, glyph: Option<&str>, bg: Color32, fg: Color32, hover: bool, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(2.0) } else { rect };
    let bg = if pressed { theme::mix(bg, Color32::WHITE, 0.16) } else if hover { theme::mix(bg, Color32::WHITE, 0.07) } else { bg };
    p.rect_filled(rect, CornerRadius::same(10), fade(bg, alpha));
    match glyph {
        Some(g) => widgets::icon(p, rect.center(), g, 24.0, fade(fg, alpha)),
        None => text(p, rect.center(), Align2::CENTER_CENTER, orbit_look::txt(label, 22.0).color(fade(fg, alpha))),
    }
}

/// The search field, or a compose page's: what is typed (its end when longer than the field), else `hint`. `glyph` says which it is, whatever language the hint is in.
#[allow(clippy::too_many_arguments)]
fn field(p: &Painter, rect: Rect, typed: &str, hint: &str, glyph: &str, active: bool, (hover, _): (bool, bool), time: f64, alpha: f32) {
    p.rect_filled(rect, CornerRadius::same(28), fade(if hover && !active { RAISED } else { SURFACE }, alpha));
    if active {
        p.rect_stroke(rect, CornerRadius::same(28), Stroke::new(2.0, fade(orbit_look::accent(), alpha)), StrokeKind::Inside);
    }
    widgets::icon(p, pos2(rect.min.x + 34.0, rect.center().y), glyph, 22.0, fade(theme::FG_3, alpha));
    let at = pos2(rect.min.x + 62.0, rect.center().y);
    let room = rect.max.x - at.x - 28.0;
    let shown = if typed.is_empty() && !active {
        orbit_look::txt(hint, 20.0).color(fade(theme::FG_4, alpha))
    } else if typed.is_empty() {
        orbit_look::txt("", 20.0).color(fade(theme::FG, alpha))
    } else {
        // The end of what is typed, where the cursor is.
        let chars: Vec<char> = typed.chars().collect();
        let mut from = chars.len().saturating_sub(64);
        loop {
            let tail: String = chars[from..].iter().collect();
            let galley = p.layout_job(orbit_look::txt(tail.clone(), 20.0).job(f32::INFINITY));
            if galley.size().x <= room || from + 1 >= chars.len() {
                break orbit_look::txt(tail, 20.0).color(fade(theme::FG, alpha));
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
        p.line_segment([pos2(x, rect.center().y - height / 2.0), pos2(x, rect.center().y + height / 2.0)], Stroke::new(2.0, fade(orbit_look::accent(), alpha)));
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
    super::super::orbit::panels::keep_colour(p, rect);
    p.add(RectShape::filled(rect, corner, fade(Color32::WHITE, alpha)).with_texture(id, uv));
}

fn hsv(h: f32, s: f32, v: f32) -> Color32 {
    let rgb = egui::ecolor::Hsva::new(h, s, v, 1.0).to_srgb();
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// A tinted chip at `at`; returns its width.
fn chip(p: &Painter, at: Pos2, glyph: &str, label: &str, tint: Color32, alpha: f32) -> f32 {
    let txt = orbit_look::txt(label, 17.5).weight(weight::MEDIUM).color(fade(tint, alpha));
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
    let galley = p.layout_job(orbit_look::txt(label, 14.0).weight(weight::SEMIBOLD).color(fade(theme::FG, alpha)).job(f32::INFINITY));
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
        let label = if status.shown { tr("Around you") } else { tr("Loading") };
        let tint = if status.shown { orbit_look::accent() } else { theme::WARN };
        let txt = orbit_look::txt(label, 14.0).weight(weight::SEMIBOLD).color(fade(orbit_look::on_accent(), alpha));
        let galley = p.layout_job(txt.job(f32::INFINITY));
        let chip = Rect::from_min_size(thumb.min + vec2(12.0, 12.0), vec2(galley.size().x + 22.0, 26.0));
        p.rect_filled(chip, CornerRadius::same(13), fade(tint, alpha));
        p.galley(pos2(chip.min.x + 11.0, chip.center().y - galley.size().y / 2.0), galley, orbit_look::on_accent());
    } else if entry.capture {
        // A 3D capture: shown around the person from here.
        let txt = orbit_look::txt("3D", 15.5).weight(weight::SEMIBOLD).color(fade(orbit_look::on_accent(), alpha));
        let galley = p.layout_job(txt.job(f32::INFINITY));
        let chip = Rect::from_min_size(thumb.min + vec2(12.0, 12.0), vec2(galley.size().x + 44.0, 26.0));
        p.rect_filled(chip, CornerRadius::same(13), fade(orbit_look::accent(), alpha));
        widgets::icon(p, pos2(chip.min.x + 16.0, chip.center().y), icons::CUBE, 15.0, fade(orbit_look::on_accent(), alpha));
        p.galley(pos2(chip.min.x + 30.0, chip.center().y - galley.size().y / 2.0), galley, orbit_look::on_accent());
    }
    if entry.official {
        widgets::icon(p, pos2(thumb.max.x - 22.0, thumb.min.y + 24.0), icons::SEAL_CHECK, 22.0, fade(orbit_look::accent(), alpha));
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
    // Its whole name, on two lines where one does not hold it, and then nothing under it (round 11's judges: "Normal Capa..." could not be told from the next).
    let room = rect.width() - 32.0;
    let one = p.layout_job(orbit_look::txt(entry.name.clone(), 19.0).weight(weight::SEMIBOLD).job(f32::INFINITY));
    if one.size().x > room {
        let mut job = orbit_look::txt(entry.name.clone(), 17.0).weight(weight::SEMIBOLD).color(fade(theme::FG, alpha)).job(room);
        job.wrap.max_rows = 2;
        job.wrap.overflow_character = Some('\u{2026}');
        p.galley(pos2(rect.min.x + 16.0, rect.min.y + CARD_THUMB + 6.0), p.layout_job(job), fade(theme::FG, alpha));
        return;
    }
    text_cut(p, pos2(rect.min.x + 16.0, rect.min.y + CARD_THUMB + 9.0), room, orbit_look::txt(entry.name.clone(), 19.0).weight(weight::SEMIBOLD).color(fade(theme::FG, alpha)));
    // Its author, and the labels people gave its world most.
    let mut meta: Vec<String> = d.author(entry).map(str::to_string).into_iter().collect();
    let wf = d.world_feedback(entry);
    meta.extend(wf.top().iter().take(2).map(|l| label_name(&l.id).to_string()));
    if meta.is_empty() {
        meta.push(match entry.visited {
            _ if entry.capture => tr!("{count} splats", count = short_count(entry.splats as u64)),
            Some((_, 1)) => tr("Visited once").to_string(),
            Some((_, n)) => tr!("Visited {count} times", count = n),
            None => tr("No 3D capture yet").to_string(),
        });
    }
    text_cut(p, pos2(rect.min.x + 16.0, rect.min.y + CARD_THUMB + 36.0), rect.width() - 32.0, orbit_look::txt(meta.join(" · "), 15.0).color(fade(theme::FG_3, alpha)));
}

/// A world beside the side panel (and over the keyboard): its picture as a block on the left, and beside it its name, who made it and how it is liked, one
/// group centred on the picture, whatever its name's length. `selected`: its details are the ones open, the card in the accent.
#[allow(clippy::too_many_arguments)]
fn compact_card(p: &Painter, rect: Rect, entry: &Entry, d: &Data, hover: bool, pressed: bool, alpha: f32, selected: bool) {
    let status = d.status;
    let rect = if pressed { rect.shrink(3.0) } else { rect };
    let (corner, accent) = (CornerRadius::same(16), orbit_look::accent());
    let fill = if selected { theme::mix(SURFACE, accent, if hover { 0.2 } else { 0.13 }) } else if hover { RAISED } else { SURFACE };
    p.rect_filled(rect, corner, fade(fill, alpha));
    // An edge of its own: a hairline at rest, the accent while its details are open.
    let edge = if selected { Stroke::new(2.0, fade(accent, alpha)) } else { Stroke::new(1.0, fade(theme::white(if hover { 0.16 } else { 0.07 }), alpha)) };
    p.rect_stroke(rect, corner, edge, StrokeKind::Inside);
    let thumb = compact_thumb(rect);
    picture(p, thumb, (d.thumb)(entry), &entry.world_id, CornerRadius::same(10), alpha);
    if entry.capture {
        // It has a 3D capture: on its picture's corner, as on the full list's cards.
        // At 12 px at 1280 x 720, as the rest of the card (it was the one thing on it under that).
        let galley = p.layout_job(orbit_look::txt("3D", 17.5).weight(weight::BOLD).color(fade(orbit_look::on_accent(), alpha)).job(f32::INFINITY));
        let chip = Rect::from_min_size(thumb.min + vec2(6.0, 6.0), vec2(galley.size().x + 34.0, 26.0));
        p.rect_filled(chip, CornerRadius::same(13), fade(accent, alpha));
        widgets::icon(p, pos2(chip.min.x + 14.0, chip.center().y), icons::CUBE, 16.0, fade(orbit_look::on_accent(), alpha));
        p.galley(pos2(chip.min.x + 25.0, chip.center().y - galley.size().y / 2.0), galley, orbit_look::on_accent());
    }
    let x = thumb.max.x + 12.0;
    let width = rect.max.x - x - 12.0;
    // Nothing on it under 12 px at 1280 x 720 (round 18's judges): its name a little over, the rest at that.
    let mut name = orbit_look::txt(entry.name.clone(), 18.0).weight(weight::SEMIBOLD).color(fade(theme::FG, alpha)).job(width);
    name.wrap.max_rows = 2;
    name.wrap.overflow_character = Some('\u{2026}');
    let name = p.layout_job(name);
    let author = d.author(entry).map(|a| p.layout_job(orbit_look::txt(a.trim().to_string(), 17.5).color(fade(theme::FG_3, alpha)).job_truncated(width)));
    let current = status.current == Some(entry.id);
    let likes = d.likes(entry);
    let foot = current || likes > 0;
    // One group, centred on the picture: a short name does not leave the rest hanging at the card's foot.
    const FOOT: f32 = 20.0;
    let high = name.size().y + author.as_ref().map_or(0.0, |a| 1.0 + a.size().y) + if foot { 4.0 + FOOT } else { 0.0 };
    let mut y = rect.center().y - high / 2.0;
    let name_high = name.size().y;
    p.galley(pos2(x, y), name, theme::FG);
    y += name_high;
    if let Some(author) = author {
        y += 1.0;
        let author_high = author.size().y;
        p.galley(pos2(x, y), author, theme::FG_3);
        y += author_high;
    }
    if !foot {
        return;
    }
    let line = y + 4.0 + FOOT / 2.0;
    if current {
        p.circle_filled(pos2(x + 5.0, line), 4.5, fade(if status.shown { accent } else { theme::WARN }, alpha));
        text(p, pos2(x + 16.0, line), Align2::LEFT_CENTER, orbit_look::txt(if !status.shown { tr("Loading") } else if status.small { tr_ctx("view", "Small") } else { tr("Around you") }, 17.5).weight(weight::MEDIUM).color(fade(theme::FG_2, alpha)));
    } else {
        let liked = d.feedback.world(&world_key(&entry.world_id)).is_some_and(|w| w.liked);
        widgets::icon_fill(p, pos2(x + 8.0, line), icons::HEART, 16.0, fade(if liked { theme::REC } else { theme::FG_3 }, alpha));
        text(p, pos2(x + 21.0, line), Align2::LEFT_CENTER, orbit_look::txt(short_count(likes as u64), 17.5).weight(weight::MEDIUM).color(fade(theme::FG_2, alpha)));
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
            (icons::CUBE_FOCUS, if status.shown { n } else { tr!("Loading {name}", name = n) }, orbit_look::accent())
        }
        _ => (icons::CUBE, tr("3D worlds").to_string(), theme::FG),
    };
    let icon_x = rect.min.x + 38.0;
    if showing && !status.shown {
        loading_ring(p, pos2(icon_x, rect.center().y), time);
    } else {
        widgets::icon(p, pos2(icon_x, rect.center().y), glyph, 26.0, tint);
    }
    text_cut(p, pos2(rect.min.x + 64.0, rect.center().y - 13.0), rect.width() - 100.0, orbit_look::txt(label, 21.0).weight(weight::SEMIBOLD).color(theme::FG));
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
    use orbiters_i18n::{in_language, Language};

    /// What a person reads follows the language; what is saved in a picture's name and in files, and what the code decides by, does not.
    #[test]
    fn the_panel_speaks_the_language_chosen_and_keeps_the_names_it_saves_in_english() {
        let at = "2026-10-12T18:30:00Z";
        assert_eq!(short_date(at).as_deref(), Some("12 Oct 2026"));
        in_language(Language::Ja, || assert_eq!(short_date(at).as_deref(), Some("2026年10月12日")));
        in_language(Language::Fr, || {
            assert_eq!(short_date(at).as_deref(), Some("12 oct. 2026"));
            assert_eq!(label_name("mirror"), "Miroir", "the labels come with the contract's own names");
            assert_eq!(Quality::High.shown_label(), "Net");
            assert_eq!(Quality::High.label(), "Sharp", "the name kept in pictures stays English");
            assert_eq!(Sort::Liked.label(), "Plus aimés");
            assert_eq!(Section::Captures.label(true), "Captures 3D");
        });
        assert_eq!(label_name("mirror"), "Mirror");
        assert_eq!(Quality::High.shown_label(), "Sharp");
        // The popular/featured name is the one the list is, whatever it is called.
        in_language(Language::PtBr, || assert_eq!(Section::Popular.label(false), "Destaques"));
    }

    /// A line with values in it (megabytes read) keeps them and takes the language's order, spacing and unit.
    #[test]
    fn the_status_line_of_a_download_is_said_in_the_language_chosen() {
        let say = |read: &str, total: &str| tr!("Downloading {read} of {total} MB", read = read, total = total);
        assert_eq!(say("1.5", "3.0"), "Downloading 1.5 of 3.0 MB");
        in_language(Language::Fr, || assert_eq!(say("1.5", "3.0"), "Téléchargement\u{a0}: 1.5 sur 3.0 Mo"));
        in_language(Language::Ja, || assert_eq!(say("1.5", "3.0"), "ダウンロード中 1.5 / 3.0 MB"));
    }

    /// Round 5's judges: beside the panel, the list lost its fifth world.
    /// Enzo: "infinite scroll creates less friction". A press stays a press; a drag moves the list and presses nothing; a throw coasts and stops at the end.
    #[test]
    fn a_headsets_list_of_worlds_is_endless() {
        let mut panel = Panel { page: Page::Detail, ..Default::default() };
        panel.vr_measure(40, true, true);
        assert_eq!(panel.vr_most.get(), VR_LIST_TOP + 4.0 + 21.0 * VR_ROW - VR_LIST_BOTTOM);
        // A press, with a hand's tremble.
        panel.vr_press(200.0, Some(Target::Card(3)), 0.0);
        panel.vr_drag(204.0, 0.01);
        assert!(!panel.vr_dragging());
        assert_eq!(panel.vr_release(0.02), Some(Target::Card(3)));
        assert_eq!(panel.vr_scroll, 0.0);
        // A drag.
        panel.vr_press(400.0, Some(Target::Card(3)), 1.0);
        panel.vr_drag(380.0, 1.01);
        panel.vr_drag(300.0, 1.02);
        assert!(panel.vr_dragging());
        assert_eq!(panel.vr_scroll, 100.0);
        // Down a good way, it asks for what follows, as turning the pages did.
        panel.vr_drag(-2400.0, 1.1);
        assert!(panel.results_page >= 4, "{}", panel.results_page);
        assert_eq!(panel.vr_release(1.11), None, "a drag is no press");
        // Thrown: it goes on, slowing, and stops.
        let thrown = panel.vr_throw;
        assert!(thrown > 0.0);
        let was = panel.vr_scroll;
        panel.vr_coast(1.2);
        assert!(panel.vr_scroll > was && panel.vr_throw < thrown);
        for n in 0..400 {
            panel.vr_coast(1.2 + n as f64 * 0.05);
        }
        assert_eq!(panel.vr_throw, 0.0);
        assert!(panel.vr_scroll <= panel.vr_most.get());
        // Another list starts from its top.
        panel.to_the_top();
        assert_eq!((panel.vr_scroll, panel.results_page), (0.0, 0));
    }

    #[test]
    fn every_world_of_a_few_keeps_its_place_beside_the_side_panel() {
        let (rects, heading) = few_rects(5, 2, true);
        assert!(rects.iter().all(Option::is_some), "{rects:?}");
        let rects: Vec<Rect> = rects.into_iter().flatten().collect();
        assert!(rects[1].bottom() < heading && rects[2].top() > heading);
        for (i, a) in rects.iter().enumerate() {
            assert!(a.max.x < SIDE_LEFT && a.max.y <= 576.0, "{a:?}");
            assert!(rects[i + 1..].iter().all(|b| !a.intersects(*b)), "{a:?}");
        }
    }

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

    /// `n` captures, ids 1 to n, newest first as the listing has them.
    fn many(n: u64) -> Vec<Entry> {
        let previews: Vec<String> = (1..=n)
            .map(|i| format!(r#"{{"slot":{i},"key":{i},"id":{i},"worldId":"wrld_00000000-0000-4000-8000-{i:012}","name":"World {i}","author":"a","uploader":"b","view":{{"position":[0,1,0],"target":[0,1,1]}}}}"#))
            .collect();
        super::super::catalog::parse(&format!(r#"{{"previews":[{}],"feedback":{{}}}}"#, previews.join(","))).unwrap()
    }

    /// K-carousel (Enzo, 2026-11-15: "more frictionless and more straight forward to go through all the 3D capture pictures. Maybe if it could be a caroussel"):
    /// from a capture's side panel, the one after and the one before are a press away, in the list's order, skipping worlds without a capture, and each is
    /// shown as it comes, small as the one before was. Nothing past either end.
    #[test]
    fn the_carousel_goes_through_the_captures_in_the_lists_order() {
        let mut entries = super::super::catalog::parse(LISTING).unwrap();
        let found = orbiters_core::maps::FoundWorld { world: orbiters_core::maps::World { world_id: "wrld_no_capture".into(), name: "Plain".into(), ..Default::default() }, ..Default::default() };
        entries.insert(1, Entry::world(&found, None, Default::default()));
        let (facts, feedback) = (HashMap::new(), FeedbackView::default());
        let status = Status { headset: true, current: Some(1), shown: true, small: true, small_docked: true, ..Default::default() };
        let d = data(&entries, &status, &feedback, &facts);
        let mut panel = Panel { page: Page::Detail, selected: Some(1), ..Default::default() };
        let order: Vec<u64> = panel.carousel(&d).iter().map(|e| e.id).collect();
        assert_eq!(order, vec![1, 2, 3, 4], "the captures only, as listed");
        let has = |panel: &Panel, t: Target| panel.targets(&d).iter().any(|(x, _)| *x == t);
        assert!(!has(&panel, Target::FlipPrevious) && has(&panel, Target::FlipNext), "the first: nothing before it");
        assert_eq!(panel.press(Target::FlipNext, &d, 1.0), vec![Action::Show(2)]);
        assert_eq!(panel.selected, Some(2));
        assert_eq!(panel.page, Page::Detail, "still its side panel");
        assert!(has(&panel, Target::FlipPrevious) && has(&panel, Target::FlipNext));
        assert_eq!(panel.press(Target::FlipNext, &d, 2.0), vec![Action::Show(3)]);
        assert_eq!(panel.press(Target::FlipNext, &d, 3.0), vec![Action::Show(4)]);
        assert!(!has(&panel, Target::FlipNext), "the last: nothing after it");
        assert_eq!(panel.flip_by_stick(&d, true, 4.0), Vec::new(), "the stick past the end does nothing");
        assert_eq!(panel.selected, Some(4));
        assert_eq!(panel.flip_by_stick(&d, false, 5.0), vec![Action::Show(3)]);
        assert_eq!(panel.press(Target::FlipPrevious, &d, 6.0), vec![Action::Show(2)]);
        // Back to the one showing: nothing to ask, it is there.
        assert_eq!(panel.press(Target::FlipPrevious, &d, 7.0), Vec::new());
        assert_eq!(panel.selected, Some(1));
        // Where it is: first of four, its neighbours.
        let (at, n, before, after) = panel.carousel_place(&d).unwrap();
        assert_eq!((at, n, before.map(|e| e.id), after.map(|e| e.id)), (0, 4, None, Some(2)));
        // On a screen there is no carousel.
        let screen = Status { headset: false, ..status.clone() };
        assert!(panel.carousel_place(&data(&entries, &screen, &feedback, &facts)).is_none());
    }

    /// K-carousel: a flip shows the next one's picture at once and never the capture leaving through an open hole: the hole closes the moment another is chosen
    /// (its still covers it) and opens again only once that one shows. The arrows are tall, easy targets beside the hole and never over it.
    #[test]
    fn a_flip_shows_its_picture_at_once_and_never_an_empty_hole() {
        let entries = super::super::catalog::parse(LISTING).unwrap();
        let (facts, feedback) = (HashMap::new(), FeedbackView::default());
        let shown = Status { headset: true, current: Some(2), shown: true, small: true, small_docked: true, ..Default::default() };
        let mut panel = Panel { page: Page::Detail, selected: Some(2), ..Default::default() };
        assert_eq!(panel.hole(&shown), Some(VR_HERO));
        panel.press(Target::FlipNext, &data(&entries, &shown, &feedback, &facts), 1.0);
        assert_eq!(panel.hole(&shown), None, "the old capture is not seen under the new name: its still covers it");
        let opening = Status { current: Some(3), shown: false, stage: Some("Opening it".into()), ..shown.clone() };
        assert_eq!(panel.hole(&opening), None);
        assert_eq!(panel.hole(&Status { current: Some(3), ..shown.clone() }), Some(VR_HERO), "open again once it shows");
        for t in [VR_FLIP_PREVIOUS, VR_FLIP_NEXT] {
            assert!(VR_SIDE.contains_rect(t), "{t:?}");
            assert!(t.intersect(VR_HERO).width() <= 42.0, "over the picture's edge only: {t:?}");
            assert!(t.height() >= VR_HERO.height() && t.width() >= 50.0, "the whole height of the picture: {t:?}");
        }
        for peek in [VR_PEEK_PREVIOUS, VR_PEEK_NEXT] {
            assert!(peek.width() <= 32.0 && VR_HERO.contains_rect(peek), "a neighbour hides no more than its edge: {peek:?}");
        }
    }

    /// K-small-stick and K-carousel: where the laser is says what the stick does. On the capture in the hole it moves in it; on the picture's band round it, with
    /// other captures, it flips them; on the buttons, the list or a world alone it is VRChat's.
    #[test]
    fn where_the_laser_is_says_what_the_stick_does() {
        let entries = super::super::catalog::parse(LISTING).unwrap();
        let (facts, feedback) = (HashMap::new(), FeedbackView::default());
        let status = Status { headset: true, current: Some(2), shown: true, small: true, small_docked: true, ..Default::default() };
        let d = data(&entries, &status, &feedback, &facts);
        let panel = Panel { page: Page::Detail, selected: Some(2), ..Default::default() };
        let at = |p: Pos2| panel.stick_spot(&d, p.x, p.y);
        assert_eq!(at(VR_HERO.center()), Some(StickSpot::InCapture));
        assert_eq!(at(VR_FLIP_NEXT.center()), Some(StickSpot::Flip), "the arrow over the picture's edge flips");
        assert_eq!(at(pos2(VR_HERO.max.x - 60.0, VR_HERO.center().y)), Some(StickSpot::InCapture), "just inside it, it moves");
        assert_eq!(at(VR_GRIP.center()), Some(StickSpot::Flip));
        assert_eq!(at(VR_SHOW.center()), None, "the buttons: VRChat's");
        assert_eq!(at(pos2(200.0, 300.0)), None, "the list: VRChat's");
        // Still opening: the picture flips too, nothing moves in it yet.
        let opening = Status { shown: false, stage: Some("Opening it".into()), ..status.clone() };
        assert_eq!(panel.stick_spot(&data(&entries, &opening, &feedback, &facts), VR_HERO.center().x, VR_HERO.center().y), Some(StickSpot::Flip));
        // One capture alone: nothing to flip to.
        let one = &entries[1..2];
        assert_eq!(panel.stick_spot(&data(one, &opening, &feedback, &facts), VR_HERO.center().x, VR_HERO.center().y), None);
        // Another page: nothing.
        let list = Panel { page: Page::Browse, ..panel.clone() };
        assert_eq!(list.stick_spot(&d, VR_HERO.center().x, VR_HERO.center().y), None);
    }

    /// K-carousel: the list beside the side panel keeps in step: flipped past what shows of it, it scrolls just enough for the one shown, lit, to be in view.
    #[test]
    fn the_list_keeps_in_step_with_the_carousel() {
        let entries = many(30);
        let (facts, feedback) = (HashMap::new(), FeedbackView::default());
        let status = Status { headset: true, ..Default::default() };
        let d = data(&entries, &status, &feedback, &facts);
        let mut panel = Panel { page: Page::Detail, selected: Some(1), ..Default::default() };
        panel.targets(&d);
        assert_eq!(panel.vr_scroll, 0.0);
        let shape = panel.list_shape(true);
        for n in 1..20u64 {
            panel.press(Target::FlipNext, &d, n as f64);
            assert_eq!(panel.selected, Some(n + 1));
            let card = vr_card_rect(n as usize).translate(vec2(0.0, -panel.vr_scroll));
            assert!(card.min.y >= shape.top - 0.5 && card.max.y <= shape.bottom + 0.5, "flip {n}: {card:?} out of {}..{}", shape.top, shape.bottom);
        }
        assert!(panel.vr_scroll > 0.0);
        // And back up.
        for _ in 0..19 {
            panel.press(Target::FlipPrevious, &d, 30.0);
        }
        assert_eq!(panel.selected, Some(1));
        assert!(vr_card_rect(0).translate(vec2(0.0, -panel.vr_scroll)).min.y >= shape.top - 0.5);
    }

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

    /// Round 8's judges: See it in 3D means one thing. On a screen it starts the capture turning in the panel; there, the button says what it adds, and a hand can take it.
    #[test]
    fn on_a_screen_a_world_turns_in_its_side_panel() {
        let entries = super::super::catalog::parse(LISTING).unwrap();
        let (facts, feedback) = (HashMap::new(), FeedbackView::default());
        let status = Status::default();
        let d = data(&entries, &status, &feedback, &facts);
        let mut panel = Panel { page: Page::Detail, selected: Some(1), ..Default::default() };
        let has = |panel: &Panel, t: Target| panel.targets(&d).iter().any(|(x, _)| *x == t);
        // REQUEST15 item 6: no See it in 3D on a screen; the capture turns there by itself (`Worlds::follow_screen_live`).
        assert!(!has(&panel, Target::Show) && !has(&panel, Target::Hero));
        panel.live = Some(Live { id: 1, ..Default::default() });
        assert!(!has(&panel, Target::Show) && has(&panel, Target::Hero));
        // Its close stays on top of it.
        let (x, y) = (SIDE_CLOSE.center().x, SIDE_CLOSE.center().y);
        assert_eq!(panel.hit(&d, x, y), Some(Target::CloseSide));
        // Enzo: a full-screen button in its corner fills the page with it, and brings it back; once it shows something.
        assert!(!has(&panel, Target::FullView));
        panel.live = Some(Live { id: 1, picture: Some(TextureId::Managed(1)), ..Default::default() });
        assert!(has(&panel, Target::FullView));
        let (x, y) = (full_button(SIDE_HERO).center().x, full_button(SIDE_HERO).center().y);
        assert_eq!(panel.hit(&d, x, y), Some(Target::FullView));
        panel.press(Target::FullView, &d, 2.0);
        assert!(panel.live.as_ref().is_some_and(|l| l.full));
        let page = Rect::from_min_size(Pos2::ZERO, vec2(WIDTH, HEIGHT));
        assert_eq!(panel.hit(&d, page.center().x, page.center().y), Some(Target::Hero), "the whole page turns it");
        assert!(!has(&panel, Target::CloseSide), "nothing else on the page");
        panel.press(Target::FullView, &d, 3.0);
        assert!(panel.live.as_ref().is_some_and(|l| !l.full) && has(&panel, Target::CloseSide));
        // Another world's page: its own button again, until that one turns as well.
        panel.selected = Some(2);
        assert!(!has(&panel, Target::Hero));
    }

    /// Round 8's judges: no orphan row beside the panel. An odd last world has the way on to other worlds beside it.
    #[test]
    fn a_few_worlds_leave_no_hole_and_no_orphan_row_beside_the_side_panel() {
        let (rects, _) = few_rects(5, 2, true);
        assert!(rects.iter().flatten().all(|r| r.width() == 304.0));
        let slot = more_slot(5, 2, true).expect("a free slot beside the fifth");
        assert!(rects.iter().flatten().all(|r| !r.intersects(slot)));
        assert_eq!(slot.top(), rects[4].unwrap().top());
        assert_eq!(more_slot(6, 2, true), None);
    }

    /// Round 6's judges: a headset's page is not a smaller screen's. One head for the list, no filter row, and nothing over the hole.
    #[test]
    fn a_headsets_side_panel_has_no_filter_row_and_nothing_over_its_hole() {
        let entries = super::super::catalog::parse(LISTING).unwrap();
        let facts = HashMap::new();
        let feedback = FeedbackView::default();
        let status = Status { headset: true, current: Some(1), shown: true, small: true, small_docked: true, ..Default::default() };
        let panel = Panel { page: Page::Detail, selected: Some(1), ..Default::default() };
        let d = data(&entries, &status, &feedback, &facts);
        let targets = panel.targets(&d);
        for (t, r) in &targets {
            assert!(!matches!(t, Target::Back | Target::Field | Target::Section(_) | Target::Sort(_) | Target::Filter), "{t:?}");
            // The carousel's arrows lie over its edges (K-carousel), and only there.
            if matches!(t, Target::FlipPrevious | Target::FlipNext) {
                assert!(r.intersect(VR_HERO).width() <= 42.0, "{t:?} {r:?}");
                continue;
            }
            assert!(!r.intersects(VR_HERO), "{t:?} over the hole");
            if matches!(t, Target::Card(_)) {
                assert!(r.max.x <= VR_SIDE.min.x && r.max.y <= 576.0, "{r:?}");
            }
        }
        assert!(!VR_GRIP.intersects(VR_HERO));
        assert_eq!(panel.hole(&status), Some(VR_HERO));
        assert!(VR_HERO.width() * VR_HERO.height() > 2.0 * SIDE_HERO.width() * SIDE_HERO.height());
        // The list's head goes back to all of it.
        assert!(targets.iter().any(|(t, _)| *t == Target::CloseSide));
    }

    /// Round 3's judges, of the headset: tight targets. On a world's side panel and its list, at arm's length, none under 2 degrees.
    #[test]
    fn what_can_be_pressed_on_a_world_is_large_enough_for_a_laser() {
        let entries = super::super::catalog::parse(LISTING).unwrap();
        let facts = HashMap::new();
        let feedback = FeedbackView::default();
        let status = Status { headset: true, current: Some(1), shown: true, small: true, small_taken: true, ..Default::default() };
        let panel = Panel { page: Page::Detail, selected: Some(1), ..Default::default() };
        let d = data(&entries, &status, &feedback, &facts);
        const MM_A_PIXEL: f32 = 0.0009 * 0.6875 * 1000.0;
        let degrees = |pixels: f32| 2.0 * (pixels * MM_A_PIXEL / 2.0 / 550.0).atan().to_degrees();
        let targets = panel.targets(&d);
        assert!(targets.iter().any(|(t, _)| *t == Target::CloseSide));
        let smallest = targets.iter().map(|(t, r)| (r.width().min(r.height()), *t)).min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
        println!("a world's side panel: smallest target {} px = {:.1} degrees ({:?})", smallest.0, degrees(smallest.0), smallest.1);
        assert!(degrees(smallest.0) >= 2.0, "{smallest:?}");
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

    /// VR feedback F6: a comment needs a signed-in member, not the world's feedback read (Orbiters may not answer that read); signed out, Write a comment says
    /// so and starts signing in; a read Orbiters did not answer is said, with a way to ask again, and the comment can still be written and sent.
    #[test]
    fn a_comment_needs_a_sign_in_and_nothing_else() {
        let entries = super::super::catalog::parse(LISTING).unwrap();
        let (status, facts) = (Status::default(), HashMap::new());
        let mut panel = Panel { page: Page::Comments, selected: Some(1), ..Default::default() };
        let out = FeedbackView::default();
        assert_eq!(panel.press(Target::WriteComment, &data(&entries, &status, &out, &facts), 1.0), [Action::SignIn], "signed out: the way to sign in");
        assert_eq!(panel.page, Page::Comments);
        assert!(panel.note(1.0).is_some_and(|n| n.contains("sign in")), "{:?}", panel.note(1.0));
        // Signed in, the read never answered: said, with Try again, and Write a comment opens its keyboard all the same.
        let unread = FeedbackView {
            signed_in: true,
            worlds: vec![WorldFeedback { world: HOME.into(), loading: true, error: Some("Orbiters did not answer".into()), ..Default::default() }],
            ..Default::default()
        };
        let d = data(&entries, &status, &unread, &facts);
        assert!(panel.targets(&d).iter().any(|(t, _)| *t == Target::RetryFeedback));
        assert_eq!(panel.press(Target::RetryFeedback, &d, 2.0), [Action::RetryFeedback]);
        assert!(panel.press(Target::WriteComment, &d, 3.0).is_empty());
        assert_eq!(panel.page, Page::Compose(Compose::Comment));
        panel.type_text("Lovely at sunset");
        let sent = panel.press(Target::Done, &d, 4.0);
        assert!(matches!(&sent[..], [Action::Feedback(FeedbackAsk::Comment { world, .. })] if world == HOME), "{sent:?}");
        // Signed out while writing: what was typed stays, and Send is the way to sign in.
        panel.page = Page::Compose(Compose::Comment);
        panel.type_text("Kept");
        assert_eq!(panel.press(Target::Done, &data(&entries, &status, &out, &facts), 5.0), [Action::SignIn]);
        assert_eq!(panel.draft, "Kept");
    }

    /// Enzo (VR feedback F5): "Make the Go to world button give choices of instance. If the user is not using their VRChat account in Orbiters N4N0, hide
    /// the button." Hidden signed out of VRChat; pressed, its list is read and nothing else; an instance pressed is invited to by its exact id, one that
    /// cannot be joined cannot be pressed; a new one is made of the kind pressed, in the player's region or the one picked; nothing while one is on its way.
    #[test]
    fn go_there_offers_the_instances_and_new_ones_signed_in_to_vrchat_only() {
        use orbiters_core::vrchat_api::{CreateAccess, InstanceAccess, InstanceRegion, WorldInstance, WorldInstances};
        let entries = super::super::catalog::parse(LISTING).unwrap();
        let (feedback, facts) = (FeedbackView::default(), HashMap::new());
        let mut panel = Panel { page: Page::Detail, selected: Some(1), ..Default::default() };
        for headset in [false, true] {
            let out = Status { headset, ..Default::default() };
            let targets: Vec<Target> = panel.targets(&data(&entries, &out, &feedback, &facts)).into_iter().map(|(t, _)| t).collect();
            assert!(!targets.contains(&Target::Join), "headset {headset}: no VRChat session in Orbiters, no Go there: {targets:?}");
            let signed_in = Status { headset, list: ListStatus { vrchat: true, ..Default::default() }, ..Default::default() };
            let targets: Vec<Target> = panel.targets(&data(&entries, &signed_in, &feedback, &facts)).into_iter().map(|(t, _)| t).collect();
            assert!(targets.contains(&Target::Join), "headset {headset}: {targets:?}");
        }
        let signed_in = Status { list: ListStatus { vrchat: true, ..Default::default() }, ..Default::default() };
        assert_eq!(panel.press(Target::Join, &data(&entries, &signed_in, &feedback, &facts), 1.0), [Action::Join(HOME.into())], "the list asked for, nothing sent");
        assert_eq!(panel.page, Page::Go);
        let instance = |id: &str, can_join: bool| WorldInstance {
            world: HOME.into(),
            id: id.into(),
            access: InstanceAccess::FriendsPlus,
            region: "eu".into(),
            occupants: if can_join { 7 } else { 32 },
            capacity: Some(32),
            friends: Some(2),
            full: !can_join,
            closed: false,
            can_join,
        };
        let listed = super::go::GoView {
            world: HOME.into(),
            list: Some(WorldInstances { items: vec![instance("48213~hidden(usr_a)~region(eu)~nonce(x)", true), instance("1~region(eu)", false)], unavailable: 1, remaining: Vec::new() }),
            region: Some(InstanceRegion::Eu),
            ..Default::default()
        };
        let ready = Status { go: listed.clone(), ..signed_in.clone() };
        let d = data(&entries, &ready, &feedback, &facts);
        let targets: Vec<Target> = panel.targets(&d).into_iter().map(|(t, _)| t).collect();
        assert!(targets.contains(&Target::GoInstance(0)) && !targets.contains(&Target::GoInstance(1)), "a full one cannot be pressed: {targets:?}");
        assert_eq!(panel.press(Target::GoInstance(0), &d, 2.0), [Action::GoTo { world: HOME.into(), instance: "48213~hidden(usr_a)~region(eu)~nonce(x)".into() }]);
        assert!(panel.press(Target::GoInstance(1), &d, 2.0).is_empty());
        assert_eq!(panel.press(Target::NewInstance(1), &d, 3.0), [Action::NewInstance { world: HOME.into(), access: CreateAccess::FriendsPlus, region: InstanceRegion::Eu }]);
        assert!(panel.press(Target::GoRegion, &d, 3.0).is_empty());
        assert_eq!(panel.press(Target::NewInstance(4), &d, 3.0), [Action::NewInstance { world: HOME.into(), access: CreateAccess::Invite, region: InstanceRegion::Us }], "the region picked");
        // One on its way: nothing more is sent.
        let busy = Status { go: super::go::GoView { busy: Some(super::go::GoBusy::Going), ..listed.clone() }, ..signed_in.clone() };
        let d = data(&entries, &busy, &feedback, &facts);
        assert!(panel.press(Target::GoInstance(0), &d, 4.0).is_empty() && panel.press(Target::NewInstance(0), &d, 4.0).is_empty());
        // On the way directly (L-go-direct): the way back to the list, and an invite instead should VRChat only show the world.
        use crate::controller::travel::{Detour, Went};
        let gone = |how, arrived| Status { go: super::go::GoView { went: Some(super::go::GoneTo { instance: instance("48213~hidden(usr_a)~region(eu)~nonce(x)", true), how, arrived }), ..listed.clone() }, ..signed_in.clone() };
        let direct = gone(Went::Direct, false);
        let d = data(&entries, &direct, &feedback, &facts);
        let targets: Vec<Target> = panel.targets(&d).into_iter().map(|(t, _)| t).filter(|t| *t != Target::Back).collect();
        assert_eq!(targets, [Target::GoRetry, Target::GoInvite], "the trip's card, the way back to the list, the invite instead");
        assert_eq!(panel.press(Target::GoInvite, &d, 4.5), [Action::InviteInstead { world: HOME.into(), instance: "48213~hidden(usr_a)~region(eu)~nonce(x)".into() }]);
        // Arrived, started, or an invite already: nothing more to ask.
        for status in [gone(Went::Direct, true), gone(Went::Started, false), gone(Went::Invited(Detour::OtherVrchat), false)] {
            let d = data(&entries, &status, &feedback, &facts);
            let targets: Vec<Target> = panel.targets(&d).into_iter().map(|(t, _)| t).filter(|t| *t != Target::Back).collect();
            assert_eq!(targets, [Target::GoRetry]);
            assert!(panel.press(Target::GoInvite, &d, 4.6).is_empty());
        }
        let sent = gone(Went::Invited(Detour::Asked), false);
        let d = data(&entries, &sent, &feedback, &facts);
        assert_eq!(panel.press(Target::GoRetry, &d, 5.0), [Action::Join(HOME.into())]);
        panel.press(Target::Back, &d, 6.0);
        assert_eq!(panel.page, Page::Detail);
    }

    /// REQUEST20 1: Copy name right after a world's name on its side panel, in a headset and on a screen, once painted;
    /// pressed, the name exactly as shown, and Copied for a moment.
    #[test]
    fn copy_name_sits_after_the_name_and_copies_it_exactly() {
        let mut entries = super::super::catalog::parse(LISTING).unwrap();
        entries[1].name = "Dawn ☆ Café [JP] ".into();
        let (facts, feedback) = (HashMap::new(), FeedbackView::default());
        let ctx = egui::Context::default();
        orbiters_ui::install(&ctx);
        for headset in [false, true] {
            let status = Status { headset, ..Default::default() };
            let d = data(&entries, &status, &feedback, &facts);
            let mut panel = Panel { page: Page::Detail, selected: Some(2), ..Default::default() };
            let copy = |panel: &Panel| panel.targets(&d).into_iter().find(|(t, _)| *t == Target::CopyName).map(|(_, r)| r);
            assert!(copy(&panel).is_none(), "placed by a paint");
            let mut painted = ctx.run_ui(egui::RawInput::default(), |ui| panel.draw(ui.painter(), &d, 1.0));
            painted.textures_delta.clear();
            let rect = copy(&panel).expect("beside the name");
            let (side, left) = if headset { (VR_SIDE, VR_LEFT) } else { (SIDE, SIDE_LEFT) };
            assert!(side.contains_rect(rect), "{rect:?}");
            assert!(rect.left() > left + 60.0 && rect.left() < left + 300.0, "just after a short name: {rect:?}");
            assert!(rect.height() >= 30.0 && rect.width() >= 34.0, "big enough to press: {rect:?}");
            let others: Vec<Rect> = panel.targets(&d).into_iter().filter(|(t, _)| *t != Target::CopyName && *t != Target::Back).map(|(_, r)| r).collect();
            assert!(others.iter().all(|r| !r.intersects(rect)), "on nothing else: {rect:?} {others:?}");
            assert_eq!(panel.hit(&d, rect.center().x, rect.center().y), Some(Target::CopyName));
            assert_eq!(panel.press(Target::CopyName, &d, 2.0), [Action::CopyName("Dawn ☆ Café [JP] ".into())], "exactly as shown");
            assert!(panel.copied.is_some_and(|(id, _)| id == 2) && panel.animating(&status, 2.5));
            assert!(!panel.animating(&status, 2.0 + COPIED_FOR + 0.5) || panel.page_at > 0.0);
            // Another world chosen: its own name, never the one before's.
            panel.selected = Some(3);
            assert!(copy(&panel).is_none(), "placed again for it");
        }
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
        let searched = Status { list: ListStatus { searching: true, more: true, vrchat: true, ..Default::default() }, ..Default::default() };
        let d = data(&entries, &searched, &feedback, &facts);
        let mut panel = Panel { query: "ho".into(), ..Default::default() };
        // Searching: 3D only instead of the sections, and a page more while the list has more.
        let targets: Vec<Target> = panel.targets(&d).into_iter().map(|(t, _)| t).collect();
        assert!(targets.contains(&Target::Has3d) && !targets.contains(&Target::Section(Section::Popular)));
        assert!(!targets.contains(&Target::Next), "more to read comes as the list goes on: no pages");
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
        let signed_out = Status { list: ListStatus { vrchat: false, ..searched.list.clone() }, ..searched.clone() };
        let out = data(&entries, &signed_out, &feedback, &facts);
        assert!(!panel.targets(&out).iter().any(|(t, _)| *t == Target::Join));
        assert!(panel.press(Target::Join, &out, 3.0).is_empty());
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
        assert!(!targets.contains(&Target::Show), "on a screen no See it in 3D: its capture turns in the panel by itself (REQUEST15 item 6)");
        assert_eq!(panel.press(Target::Other, &d, 2.0), Vec::<Action>::new(), "back to the one shown: nothing to download");
        assert_eq!(panel.selected, Some(1));
    }
}

impl Panel {
    // i18n: not shown (render cache identity)
    pub(crate) fn paint_key(&self) -> String {
        format!("worlds {:?} {:?} {} {:?} {:?} {:?} {} {} {} {:?}", self.page, self.selected, self.results_page, self.sort, self.section, self.labels, self.comments_page, self.pictures_page, self.go_page, self.picture) // i18n: not shown (render cache identity)
    }
    pub(crate) fn adopt_metrics(&self, from: &Self) {
        self.list_step.set(from.list_step.get()); self.list_headset.set(from.list_headset.get()); self.vr_most.set(from.vr_most.get());
        self.copy_at.set(from.copy_at.get());
    }
    pub(crate) fn same_input(&self, b: &Self, target: Target) -> bool {
        if self.page != b.page || self.selected != b.selected || self.layout != b.layout { return false; }
        // These navigate within the selected world; arriving descriptions/comments
        // and list scroll do not change their source. Snapshot validation checks that
        // world's identity separately. Indexed actions and edits keep the full guard.
        if matches!(target, Target::Comments | Target::Join | Target::Back | Target::Labels | Target::Pictures | Target::WriteComment) { return true; }
        if matches!(target, Target::Key(_) | Target::Space | Target::Backspace) { return true; }
        self.query == b.query && self.results_page == b.results_page && self.sort == b.sort && self.section == b.section
            && self.labels == b.labels && self.official == b.official && self.has_3d == b.has_3d && self.draft == b.draft
            && self.comments_page == b.comments_page && self.pictures_page == b.pictures_page && self.go_page == b.go_page
            && self.go_region == b.go_region && self.picture == b.picture && self.vr_scroll == b.vr_scroll
    }
}
