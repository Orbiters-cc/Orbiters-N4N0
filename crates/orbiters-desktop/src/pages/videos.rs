//! Videos page: every video the worlds' players played in VRChat (the tray service keeps them from VRChat's logs:
//! `orbiters_core::video_history`), the latest first and by day, each with its world and when, and YouTube's title,
//! channel and thumbnail; its link copied (for another world's player) or opened in the browser, and bookmarked in lists
//! of the person's own (`video-bookmarks.json`, which only this page changes), each list a press away above the videos.

use super::{Cx, Job};
use crate::ipc::N4n0Command;
use egui::{pos2, vec2, Align2, Color32, ColorImage, CursorIcon, Id, Key, Order, Rect, Sense, TextureHandle, TextureId, TextureOptions, Ui, UiBuilder};
use orbiters_i18n::{fill, mark, tr, tr_ctx, tr_n};
use orbiters_core::video_history::{self, Bookmark, Bookmarks, History, Loaded, Played};
use orbiters_ui::anim::{self, ease, Spring};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::widgets::{self, Button, IconButton, Size, Variant};
use orbiters_ui::{format, icons, logo, toast};
use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use std::time::{Duration, SystemTime};

/// A row's thumbnail (YouTube's 16:9), and the row around it.
const THUMB: egui::Vec2 = vec2(144.0, 81.0);
const ROW_H: f32 = THUMB.y + 26.0;
/// The narrowest a row gets beside another one (a list on a wide page), and the room between them.
const ROW_MIN_W: f32 = 600.0;
const ROW_GAP: f32 = 32.0;
/// The chips above the videos.
const CHIP_H: f32 = 32.0;
/// How long a row's button says its link was copied.
const COPIED_FOR: f64 = 2.0;
/// How often the page's watch looks whether the tray service wrote new videos.
const LOOK_EVERY: f64 = 5.0;
/// Thumbnails fetched at once.
const THUMBS_AT_ONCE: usize = 4;
/// A first press on Delete waits this long for the second.
const CONFIRM_FOR: f64 = 3.0;

/// What the page shows: the history, or one of the bookmark lists (by id).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Shown {
    #[default]
    History,
    List(String),
}

/// A video as a row shows it: one of the history's, or a bookmark.
#[derive(Clone, Debug, Default, PartialEq)]
struct Row {
    url: String,
    title: Option<String>,
    channel: Option<String>,
    world: Option<String>,
    /// Unix seconds: its last play, or when it was bookmarked.
    when: u64,
    /// Plays (0 for a bookmark).
    plays: u32,
    youtube: Option<String>,
}

impl Row {
    fn of_played(v: &Played) -> Self {
        Self {
            url: v.url.clone(),
            title: v.title.clone(),
            channel: v.channel.clone(),
            world: v.world.clone(),
            when: v.last,
            plays: v.plays,
            youtube: v.youtube(),
        }
    }

    /// A bookmark, with what the history learnt of it since (a title found later).
    fn of_bookmark(b: &Bookmark, history: Option<&History>) -> Self {
        let played = history.and_then(|h| h.get(&b.url));
        Self {
            url: b.url.clone(),
            title: b.title.clone().or_else(|| played.and_then(|p| p.title.clone())),
            channel: b.channel.clone().or_else(|| played.and_then(|p| p.channel.clone())),
            world: b.world.clone().or_else(|| played.and_then(|p| p.world.clone())),
            when: b.added,
            plays: 0,
            youtube: video_history::youtube_id(&b.url),
        }
    }

    fn bookmark(&self, now: u64) -> Bookmark {
        Bookmark { url: self.url.clone(), title: self.title.clone(), channel: self.channel.clone(), world: self.world.clone(), added: now }
    }

    /// Whether the search `q` (lowercase) finds it: its title, channel or world however they are written
    /// ([`orbiters_core::search`]), or its link.
    fn matches(&self, q: &str) -> bool {
        let find = orbiters_core::search::Query::new(q);
        find.matches_any([self.title.as_deref(), self.channel.as_deref(), self.world.as_deref()].into_iter().flatten())
            || (!q.trim().is_empty() && self.url.to_lowercase().contains(q.trim()))
    }
}

/// YouTube thumbnails, fetched a few at a time as rows come into view.
/// A thumbnail fetched (its video id), or none when YouTube gave none.
type Fetched = (String, Option<ColorImage>);

/// The rows shown, by day.
type Groups = Arc<Vec<(String, Vec<Row>)>>;

/// A row's thumbnail not asked for again within this long (scrolled past) is not fetched after all.
const THUMB_STALE: f64 = 2.0;

/// A thumbnail that could not be fetched is asked again when its row shows this long after: 5 s, doubling, at most
/// 5 minutes (it was never asked again).
fn thumb_retry_after(tries: u8) -> f64 {
    (5.0 * 2f64.powi(tries.saturating_sub(1).min(16) as i32)).min(300.0)
}

#[derive(Default)]
struct Thumbs {
    ready: HashMap<String, TextureHandle>,
    /// Waiting to be fetched, with when its row last asked (egui's clock): the newest asked go first, those nobody
    /// asked for since [`THUMB_STALE`] leave (a fast scroll to the end of a long history queued every row passed).
    queue: Vec<(String, f64)>,
    /// Being fetched.
    fetching: std::collections::HashSet<String>,
    /// Could not be fetched: when, and how many times.
    failed: HashMap<String, (f64, u8)>,
    channel: Option<(mpsc::Sender<Fetched>, mpsc::Receiver<Fetched>)>,
    /// Never fetched (the screenshot review's stand-ins).
    offline: bool,
}

impl Thumbs {
    fn get(&mut self, id: &str, now: f64) -> Option<TextureId> {
        if let Some(t) = self.ready.get(id) {
            return Some(t.id());
        }
        if self.offline || self.fetching.contains(id) {
            return None;
        }
        if let Some(&(since, tries)) = self.failed.get(id) {
            if now - since < thumb_retry_after(tries) {
                return None;
            }
        }
        match self.queue.iter_mut().find(|(q, _)| q == id) {
            Some(q) => q.1 = now,
            None => self.queue.push((id.to_string(), now)),
        }
        None
    }

    /// The thumbnail to fetch next: the one asked for most recently; those nobody asked for lately leave the queue.
    fn next(&mut self, now: f64) -> Option<String> {
        self.queue.retain(|(_, asked)| now - *asked <= THUMB_STALE);
        self.queue.sort_by(|a, b| a.1.total_cmp(&b.1));
        self.queue.pop().map(|(id, _)| id)
    }

    /// Thumbnails that arrived become textures; the next asked for are fetched, newest first.
    fn step(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let channel = self.channel.get_or_insert_with(mpsc::channel);
        let tx = channel.0.clone();
        let arrived: Vec<Fetched> = channel.1.try_iter().collect();
        for (id, image) in arrived {
            self.fetching.remove(&id);
            match image {
                Some(image) => {
                    self.failed.remove(&id);
                    self.ready.insert(id.clone(), ctx.load_texture(format!("video-{id}"), image, TextureOptions::LINEAR));
                }
                None => {
                    let tries = self.failed.get(&id).map_or(0, |f| f.1).saturating_add(1);
                    self.failed.insert(id, (now, tries));
                }
            }
        }
        while self.fetching.len() < THUMBS_AT_ONCE {
            let Some(id) = self.next(now) else { break };
            self.fetching.insert(id.clone());
            let (tx, ctx) = (tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let image = orbiters_n4n0_core::youtube::thumbnail(&id)
                    .ok()
                    .and_then(|jpeg| orbiters_n4n0_core::picture::rgb_jpeg(&jpeg).ok())
                    .map(|rgb| ColorImage::from_rgb([rgb.width, rgb.height], &rgb.pixels));
                let _ = tx.send((id, image));
                orbiters_ui::anim::wake(&ctx);
            });
        }
    }
}

/// The bookmark picker, under a row's bookmark button.
struct Picker {
    row: Row,
    anchor: Rect,
    serial: u64,
    name: String,
    /// Shrinking back into its button (dropped once it landed).
    closing: bool,
}

#[derive(Default)]
pub struct State {
    /// The text searched for at the last frame ([`super::new_search`]).
    searched: Option<String>,
    history: Option<Arc<History>>,
    loading: Job<(History, Option<SystemTime>)>,
    /// The file's time when it was read.
    read_at: Option<SystemTime>,
    /// The file's time now, looked at on a thread while the page shows (a visit is a watch of its own, so each starts
    /// with a look), and the time a look saw that no reading was started for yet.
    file: super::Watch<u64, Option<SystemTime>>,
    file_at: Option<Option<SystemTime>>,
    bookmarks: Option<Bookmarks>,
    /// The bookmarks file could not be read: nothing is saved over it.
    unreadable: Option<String>,
    shown: Shown,
    query: String,
    thumbs: Thumbs,
    copied: Option<(String, f64)>,
    picker: Option<Picker>,
    serial: u64,
    /// A new list being named (focused once).
    naming: Option<(String, bool)>,
    /// The shown list being renamed (its name typed, focused once).
    renaming: Option<(String, bool)>,
    /// When Delete was pressed once for the shown list.
    confirm: Option<f64>,
    /// The screenshot review opens the picker on the first row.
    pick_first: bool,
    /// Bumped at each change of the bookmarks.
    revision: u64,
    /// The rows shown by day, and what they were made from (the history read, the bookmarks' revision, the search, what
    /// is shown, the day): made again only when one changes, not at every frame.
    groups: Option<(GroupsKey, Groups)>,
    /// Made-up videos and lists ([`State::stand_in`]): nothing is read from this computer, and the bookmarks are saved
    /// only to `bookmarks_file` when it is given.
    fixed: bool,
    bookmarks_file: Option<std::path::PathBuf>,
    /// The time it is, in place of the computer's clock (tests of what changes by the clock alone).
    #[cfg(test)]
    clock: Option<u64>,
}

#[derive(Clone, PartialEq)]
struct GroupsKey {
    history: usize,
    revision: u64,
    query: String,
    shown: Shown,
    /// The local day it is: "Today" and "Yesterday" are other days after midnight.
    day: i64,
    /// The day names are in this language.
    language: orbiters_i18n::Language,
}

/// What a press asks, done once the page is drawn.
enum Act {
    Copy(String),
    Open(String),
    Pick(Row, Rect),
    Show(Shown),
    NewList,
    Create(String),
    /// Puts the row in list `id` or takes it out.
    Toggle(String, Row),
    /// Creates a list named so and puts the row in it.
    CreateWith(String, Row),
    Rename(String),
    Delete,
    ClosePicker,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl State {
    /// Whether the videos are still being read (screenshots wait).
    pub fn busy(&self) -> bool {
        self.loading.running()
    }

    /// Made-up videos and lists in place of this computer's (the screenshot review): `list` shows that list (by its
    /// place), `picker` opens the bookmark picker on the first row; `pictures` are thumbnails by video id.
    pub fn stand_in(&mut self, history: History, bookmarks: Bookmarks, list: Option<usize>, picker: bool, pictures: Vec<(String, TextureHandle)>) {
        self.shown = list.and_then(|i| bookmarks.lists.get(i)).map_or(Shown::History, |l| Shown::List(l.id.clone()));
        self.history = Some(Arc::new(history));
        self.bookmarks = Some(bookmarks);
        self.read_at = None;
        self.thumbs.offline = true;
        self.thumbs.ready.extend(pictures);
        self.pick_first = picker;
        self.revision += 1;
        self.fixed = true;
    }

    /// Where the bookmarks are saved (`None`: nowhere, made-up ones).
    fn bookmarks_path(&self) -> Option<std::path::PathBuf> {
        match (&self.bookmarks_file, self.fixed) {
            (Some(path), _) => Some(path.clone()),
            (None, true) => None,
            (None, false) => Some(video_history::bookmarks_file()),
        }
    }

    /// Reads the videos again when the tray service wrote them since (looked at every few seconds while the page
    /// shows, and on each visit, by a thread that asks for a frame only when the file changed), and the bookmarks once.
    fn refresh(&mut self, ctx: &egui::Context, visit: u64) {
        if self.fixed {
            return;
        }
        if let Some((history, at)) = self.loading.take() {
            self.history = Some(Arc::new(history));
            self.read_at = at;
        }
        if self.bookmarks.is_none() {
            let (bookmarks, loaded) = Bookmarks::read(&video_history::bookmarks_file());
            match loaded {
                Loaded::Whole => {}
                Loaded::Partial(aside) => eprintln!("video bookmarks: some could not be read, the file kept as {aside:?}"),
                Loaded::Unreadable(e) => self.unreadable = Some(e),
            }
            self.bookmarks = Some(bookmarks);
            self.revision += 1;
        }
        let looked = self.file.keep(ctx, &visit, Duration::from_secs_f64(LOOK_EVERY), || {
            let file = video_history::history_file();
            move || std::fs::metadata(&file).and_then(|m| m.modified()).ok()
        });
        if looked.is_some() {
            self.file_at = looked;
        }
        // One reading at a time: a change seen meanwhile is read once this one landed.
        if self.loading.running() {
            return;
        }
        if let Some(at) = self.file_at.take() {
            if self.history.is_none() || at != self.read_at {
                let file = video_history::history_file();
                self.loading = Job::spawn(ctx, move || {
                    let at = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
                    (History::load(&file), at)
                });
            }
        }
    }

    /// The bookmarks changed: saved, unless the file could not be read (it would be lost).
    fn save(&mut self, ctx: &egui::Context, cx: &mut Cx) {
        self.revision += 1;
        let Some(bookmarks) = &self.bookmarks else { return };
        if let Some(e) = &self.unreadable {
            cx.toasts.push(ctx, toast::Kind::Error, tr!("Your bookmarks could not be read ({error}): nothing is saved until they can be.", error = e));
            return;
        }
        let Some(path) = self.bookmarks_path() else { return };
        if let Err(e) = bookmarks.save(&path) {
            cx.toasts.push(ctx, toast::Kind::Error, tr!("Could not save your bookmarks: {error}", error = e));
        }
    }

    /// The rows the page shows (those the search finds), the history's by local day and the latest first, a list's in
    /// its order; made again only when what they come from changed.
    fn groups(&mut self, now: u64) -> Groups {
        let key = GroupsKey {
            history: self.history.as_ref().map_or(0, |h| Arc::as_ptr(h) as usize),
            revision: self.revision,
            query: self.query.trim().to_lowercase(),
            shown: self.shown.clone(),
            day: super::local_day(now).0,
            language: orbiters_i18n::language(),
        };
        if let Some((made, groups)) = &self.groups {
            if *made == key {
                return groups.clone();
            }
        }
        let q = key.query.as_str();
        let groups = match &self.shown {
            Shown::History => {
                let mut rows: Vec<Row> =
                    self.history.as_ref().map(|h| h.videos.iter().map(Row::of_played).filter(|r| r.matches(q)).collect()).unwrap_or_default();
                rows.sort_by_key(|r| std::cmp::Reverse(r.when));
                by_day(rows, now)
            }
            Shown::List(id) => {
                let rows: Vec<Row> = self
                    .bookmarks
                    .as_ref()
                    .and_then(|b| b.list(id))
                    .map(|l| l.videos.iter().map(|v| Row::of_bookmark(v, self.history.as_deref())).filter(|r| r.matches(q)).collect())
                    .unwrap_or_default();
                if rows.is_empty() {
                    Vec::new()
                } else {
                    vec![(String::new(), rows)]
                }
            }
        };
        let groups = Arc::new(groups);
        self.groups = Some((key, groups.clone()));
        groups
    }
}

/// The local day of `secs`, and its name: "Today", "Yesterday", a weekday within the week, else its date.
fn day(secs: u64, now: u64) -> (i64, String) {
    let offset = time::UtcOffset::current_local_offset().map_or(0, |o| o.whole_seconds() as i64);
    let day_of = |s: u64| (s as i64 + offset).div_euclid(86_400);
    let (d, today) = (day_of(secs), day_of(now));
    let date = time::OffsetDateTime::from_unix_timestamp(secs as i64 + offset).unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    let months = [
        mark("January"),
        mark("February"),
        mark("March"),
        mark("April"),
        mark("May"),
        mark("June"),
        mark("July"),
        mark("August"),
        mark("September"),
        mark("October"),
        mark("November"),
        mark("December"),
    ];
    let month = tr_ctx("whole month", months[date.month() as usize - 1]);
    let name = match today - d {
        0 => tr("Today").to_string(),
        1 => tr("Yesterday").to_string(),
        2..=6 => tr(match date.weekday() {
            time::Weekday::Monday => "Monday",
            time::Weekday::Tuesday => "Tuesday",
            time::Weekday::Wednesday => "Wednesday",
            time::Weekday::Thursday => "Thursday",
            time::Weekday::Friday => "Friday",
            time::Weekday::Saturday => "Saturday",
            time::Weekday::Sunday => "Sunday",
        })
        .to_string(),
        _ if date.year() == time::OffsetDateTime::from_unix_timestamp(now as i64 + offset).map_or(0, |n| n.year()) => {
            tr!("{day} {month}", day = date.day(), month = month)
        }
        _ => fill(tr_ctx("whole month", "{day} {month} {year}"), &[("day", &date.day()), ("month", &month), ("year", &date.year())]),
    };
    (d, name)
}

/// The rows by local day, the latest first.
fn by_day(rows: Vec<Row>, now: u64) -> Vec<(String, Vec<Row>)> {
    let mut groups: Vec<(i64, String, Vec<Row>)> = Vec::new();
    for row in rows {
        let (d, name) = day(row.when, now);
        match groups.last_mut() {
            Some((last, _, list)) if *last == d => list.push(row),
            _ => groups.push((d, name, vec![row])),
        }
    }
    groups.into_iter().map(|(_, name, rows)| (name, rows)).collect()
}

/// What the page shows on its first frame, read before it (`App::prepare`).
pub fn prepare(ui: &Ui, cx: &Cx, state: &mut State) {
    state.refresh(ui.ctx(), cx.visit);
}

pub fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State) {
    let ctx = ui.ctx().clone();
    let page = ui.max_rect();
    let t = anim::time(&ctx);
    #[cfg(not(test))]
    let now = now_secs();
    #[cfg(test)]
    let now = state.clock.unwrap_or_else(now_secs);
    state.refresh(&ctx, cx.visit);
    state.thumbs.step(&ctx);
    if state.copied.as_ref().is_some_and(|(_, at)| t - at >= COPIED_FOR) {
        state.copied = None;
    }
    if state.confirm.is_some_and(|at| t - at >= CONFIRM_FOR) {
        state.confirm = None;
    }
    // A list deleted elsewhere (the file changed): back to the history.
    if let Shown::List(id) = &state.shown {
        if state.bookmarks.as_ref().is_some_and(|b| b.list(id).is_none()) {
            state.shown = Shown::History;
        }
    }

    let head = Rect::from_min_size(pos2(page.left() + super::PAD_X, page.top() + cx.top + 8.0), vec2(page.width() - super::PAD_X * 2.0, 72.0));
    let search_r = Rect::from_min_size(pos2(head.right() - 224.0, head.bottom() - 40.0), vec2(224.0, 40.0));
    let chips = chip_layout(ui, state, head.width());
    let chips_h = chips.iter().map(|c| c.rect.bottom()).fold(CHIP_H, f32::max);
    let chips_top = head.bottom() + 18.0;
    let content_top = chips_top + chips_h + 22.0;
    let below = Rect::from_min_max(pos2(page.left(), content_top), page.max);

    let groups = state.groups(now);
    // The history is told by the day: at midnight today's videos are yesterday's. (The rows in view ask for the frame
    // their own "5 min ago" changes at.) The page looked at its file every five seconds and drew itself each time,
    // which kept both right; now that it is drawn only when the file changed, the clock has to ask.
    if state.shown == Shown::History && !groups.is_empty() {
        ctx.request_repaint_after(super::after_seconds(super::local_day(now).1));
    }
    let mut acts = Vec::new();
    let from = orbiters_ui::blur::next_shape(ui);
    let salt = match &state.shown {
        Shown::History => "history".to_string(),
        Shown::List(id) => id.clone(),
    };
    let mut scroll = egui::ScrollArea::vertical().id_salt(("videos", salt));
    if super::new_search(&mut state.searched, &state.query.to_lowercase()) {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    scroll.auto_shrink([false, false]).scroll_bar_rect(below).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 32, right: 32, top: 0, bottom: 40 }).show(ui, |ui| {
            ui.add_space(content_top - page.top());
            ui.spacing_mut().item_spacing = vec2(0.0, 20.0);
            content(ui, cx, state, &groups, now, below.height(), &mut acts);
        });
    });
    super::under_top(ui, cx, "videos", from, content_top - 10.0);

    // The heading, the search and the lists, over the videos.
    let (head_r, head_a) = super::entering(ui, cx, 0, head);
    let videos = state.history.as_ref().map_or(0, |h| h.videos.len());
    let sub = match (&state.shown, &state.history) {
        (Shown::List(id), _) => {
            let n = state.bookmarks.as_ref().and_then(|b| b.list(id)).map_or(0, |l| l.videos.len());
            tr_n("{count} video bookmarked in this list", "{count} videos bookmarked in this list", n as u64)
        }
        (Shown::History, Some(_)) if videos == 0 => tr("What the worlds' video players play shows here").to_string(),
        // The number is written with its separators, so the plural is chosen by `videos` and the number put in after.
        (Shown::History, Some(_)) => fill(
            &tr_n("{total} video played in worlds, from VRChat's logs", "{total} videos played in worlds, from VRChat's logs", videos as u64),
            &[("total", &format::integer(videos as f64))],
        ),
        (Shown::History, None) => tr("Reading VRChat's logs…").to_string(),
    };
    {
        let mut tu = ui.new_child(UiBuilder::new().max_rect(head_r).id_salt("videos-title"));
        tu.multiply_opacity(head_a);
        super::heading(&tu, head_r, "VRChat", tr("Video history"), Some(&sub));
    }
    let mut su = ui.new_child(UiBuilder::new().max_rect(search_r).id_salt("videos-search"));
    widgets::search_field(&mut su, "videos-search", &mut state.query, tr("Search videos"), 224.0);
    let chips_origin = pos2(head.left(), chips_top);
    draw_chips(ui, cx, state, &chips, chips_origin, &mut acts);

    if state.picker.is_some() {
        picker(&ctx, state, &mut acts);
    }
    for act in acts {
        apply(&ctx, cx, state, act, t, now);
    }
    if let Some((_, at)) = &state.copied {
        ctx.request_repaint_after(Duration::from_secs_f64((COPIED_FOR - (t - at)).max(0.05)));
    }
}

/// The name a new list is made with: the one typed, or, when none was typed and there is no list yet, "Favourites" in the language now
/// (the field says so). Later blank names are left to the bookmarks, which number them.
fn first_list_name(bookmarks: &Bookmarks, typed: &str) -> String {
    if typed.trim().is_empty() && bookmarks.lists.is_empty() {
        tr("Favourites").to_string()
    } else {
        typed.to_string()
    }
}

/// Does what was pressed.
fn apply(ctx: &egui::Context, cx: &mut Cx, state: &mut State, act: Act, t: f64, now: u64) {
    match act {
        Act::Copy(url) => {
            cx.ctl.n4n0(N4n0Command::CopyText { text: url.clone() });
            state.copied = Some((url, t));
        }
        Act::Open(url) => {
            let r = crate::controller::open_url(&url);
            cx.run(ctx, r);
        }
        Act::Pick(row, anchor) => {
            state.serial += 1;
            state.picker = Some(Picker { row, anchor, serial: state.serial, name: String::new(), closing: false });
        }
        Act::ClosePicker => {
            if let Some(p) = state.picker.as_mut() {
                p.closing = true;
            }
        }
        Act::Show(shown) => {
            if state.shown != shown {
                state.shown = shown;
                state.renaming = None;
                state.confirm = None;
            }
        }
        Act::NewList => state.naming = Some((String::new(), true)),
        Act::Create(name) => {
            state.naming = None;
            if let Some(b) = state.bookmarks.as_mut() {
                let id = b.create(&first_list_name(b, &name));
                state.shown = Shown::List(id);
                state.save(ctx, cx);
            }
        }
        Act::Toggle(id, row) => {
            let Some(b) = state.bookmarks.as_mut() else { return };
            let name = b.list(&id).map(|l| l.name.clone()).unwrap_or_default();
            let note = if b.holds(&id, &row.url) {
                b.remove(&id, &row.url);
                tr!("Taken out of {name}", name = name)
            } else {
                b.add(&id, row.bookmark(now));
                tr!("Saved in {name}", name = name)
            };
            state.save(ctx, cx);
            cx.toasts.push(ctx, toast::Kind::Success, note);
        }
        Act::CreateWith(name, row) => {
            let Some(b) = state.bookmarks.as_mut() else { return };
            let id = b.create(&first_list_name(b, &name));
            b.add(&id, row.bookmark(now));
            let name = b.list(&id).map(|l| l.name.clone()).unwrap_or_default();
            state.save(ctx, cx);
            if let Some(p) = state.picker.as_mut() {
                p.name.clear();
            }
            cx.toasts.push(ctx, toast::Kind::Success, tr!("Saved in {name}", name = name));
        }
        Act::Rename(name) => {
            state.renaming = None;
            let Shown::List(id) = state.shown.clone() else { return };
            if state.bookmarks.as_mut().is_some_and(|b| b.rename(&id, &name)) {
                state.save(ctx, cx);
            }
        }
        Act::Delete => {
            if state.confirm.is_none() {
                state.confirm = Some(t);
                ctx.request_repaint_after(Duration::from_secs_f64(CONFIRM_FOR));
                return;
            }
            state.confirm = None;
            let Shown::List(id) = state.shown.clone() else { return };
            let name = state.bookmarks.as_ref().and_then(|b| b.list(&id)).map(|l| l.name.clone()).unwrap_or_default();
            if state.bookmarks.as_mut().is_some_and(|b| b.delete(&id)) {
                state.shown = Shown::History;
                state.save(ctx, cx);
                cx.toasts.push(ctx, toast::Kind::Info, tr!("Deleted the list {name}", name = name));
            }
        }
    }
}

/// A chip above the videos: the history, a list, or New list (a field while it is being named).
struct Chip {
    shown: Option<Shown>,
    label: String,
    /// `label` is one of the page's own words (an English key to translate), not a list's name.
    fixed: bool,
    count: Option<usize>,
    glyph: &'static str,
    /// Relative to the chips' top left.
    rect: Rect,
}

impl Chip {
    /// What the chip says. Its ids stay on `label`, in English, so that a press in flight survives a change of language.
    fn text(&self) -> &str {
        if self.fixed {
            tr(&self.label)
        } else {
            &self.label
        }
    }
}

/// The chips in rows across `width`.
fn chip_layout(ui: &Ui, state: &State, width: f32) -> Vec<Chip> {
    let (history, new_list) = (mark("History"), mark("New list"));
    let mut chips = vec![Chip {
        shown: Some(Shown::History),
        label: history.into(),
        fixed: true,
        count: state.history.as_ref().map(|h| h.videos.len()),
        glyph: icons::CLOCK,
        rect: Rect::NOTHING,
    }];
    if let Some(b) = &state.bookmarks {
        for l in &b.lists {
            chips.push(Chip {
                shown: Some(Shown::List(l.id.clone())),
                label: l.name.clone(),
                fixed: false,
                count: Some(l.videos.len()),
                glyph: icons::BOOKMARK_SIMPLE,
                rect: Rect::NOTHING,
            });
        }
    }
    chips.push(Chip { shown: None, label: new_list.into(), fixed: true, count: None, glyph: icons::PLUS, rect: Rect::NOTHING });
    let (mut x, mut y) = (0.0, 0.0);
    for chip in &mut chips {
        let w = if chip.shown.is_none() && state.naming.is_some() {
            320.0
        } else {
            let label = widgets::text_size(ui, &Txt::new(chip.text(), 13.0).weight(weight::MEDIUM)).x;
            let count = chip.count.map_or(0.0, |n| widgets::text_size(ui, &Txt::new(n.to_string(), 12.0)).x + 8.0);
            14.0 + 16.0 + 8.0 + label + count + 16.0
        };
        if x > 0.0 && x + w > width {
            x = 0.0;
            y += CHIP_H + 8.0;
        }
        chip.rect = Rect::from_min_size(pos2(x, y), vec2(w, CHIP_H));
        x += w + 8.0;
    }
    chips
}

fn draw_chips(ui: &mut Ui, cx: &Cx, state: &mut State, chips: &[Chip], origin: egui::Pos2, acts: &mut Vec<Act>) {
    let ctx = ui.ctx().clone();
    for (i, chip) in chips.iter().enumerate() {
        let (r, alpha) = super::entering(ui, cx, 1, chip.rect.translate(origin.to_vec2()));
        let Some(shown) = &chip.shown else {
            if let Some((name, focus)) = state.naming.as_mut() {
                // Naming the new list: a field, Enter makes it, Escape lets it be.
                let field = Rect::from_min_size(r.min, vec2(r.width() - 84.0, CHIP_H)).shrink2(vec2(0.0, -2.0));
                let mut fu = ui.new_child(UiBuilder::new().max_rect(field).id_salt("videos-new-list"));
                let resp = widgets::text_field(&mut fu, "videos-new-list-name", name, tr("Name the list"), field.width(), false);
                if std::mem::take(focus) {
                    resp.request_focus();
                }
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let make = Button::new(tr("Make")).variant(Variant::Primary).size(Size::Sm).width(76.0).id_salt("videos-new-list-make");
                if make.show_at(ui, Rect::from_min_size(pos2(field.right() + 8.0, r.top()), vec2(76.0, CHIP_H))).clicked() || enter {
                    acts.push(Act::Create(name.clone()));
                } else if ui.input(|i| i.key_pressed(Key::Escape)) || (resp.lost_focus() && name.trim().is_empty()) {
                    state.naming = None;
                }
                continue;
            }
            let resp = ui.interact(r, Id::unique(("videos-chip-new", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            let hover = anim::toggle(&ctx, resp.id.with("hover"), resp.hovered(), Spring::SNAPPY);
            let mut p = ui.painter().clone();
            p.multiply_opacity(alpha);
            p.rect_filled(r, theme::radius(CHIP_H / 2.0), theme::white(0.06 * hover));
            let color = theme::mix(theme::fg_3(), theme::fg(), hover);
            widgets::icon(&p, pos2(r.left() + 14.0 + 8.0, r.center().y), chip.glyph, 14.0, color);
            widgets::text(
                &p,
                pos2(r.left() + 14.0 + 16.0 + 8.0, r.center().y),
                Align2::LEFT_CENTER,
                &Txt::new(chip.text(), 13.0).weight(weight::MEDIUM).color(color),
            );
            if widgets::pressed(&resp) {
                acts.push(Act::NewList);
            }
            continue;
        };
        let active = *shown == state.shown;
        let resp = ui.interact(r, Id::unique(("videos-chip", i, &chip.label)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
        if widgets::pressed(&resp) {
            acts.push(Act::Show(shown.clone()));
        }
        let hover = anim::toggle(&ctx, resp.id.with("hover"), resp.hovered(), Spring::SNAPPY);
        let on = anim::toggle(&ctx, resp.id.with("on"), active, Spring::SNAPPY);
        let mut p = ui.painter().clone();
        p.multiply_opacity(alpha);
        // As the segmented control's: the chosen one lighter, never green.
        p.rect_filled(r, theme::radius(CHIP_H / 2.0), theme::mix(theme::mix(theme::surface(), theme::ink_750(), hover), theme::ink_600(), on));
        let fg = theme::mix(theme::mix(theme::fg_3(), theme::fg_2(), hover), Color32::WHITE, on);
        let icon_c = theme::mix(theme::mix(theme::fg_4(), theme::fg_3(), hover), theme::accent(), on);
        widgets::icon(&p, pos2(r.left() + 14.0 + 8.0, r.center().y), chip.glyph, 14.0, icon_c);
        let label = widgets::text(
            &p,
            pos2(r.left() + 14.0 + 16.0 + 8.0, r.center().y),
            Align2::LEFT_CENTER,
            &Txt::new(chip.text(), 13.0).weight(weight::MEDIUM).color(fg),
        );
        if let Some(n) = chip.count {
            widgets::text(
                &p,
                pos2(label.right() + 8.0, r.center().y),
                Align2::LEFT_CENTER,
                &Txt::new(n.to_string(), 12.0).color(theme::mix(theme::fg_4(), theme::fg_3(), on)),
            );
        }
    }
}

/// The videos by day, the shown list, or what to do when there are none.
fn content(ui: &mut Ui, cx: &Cx, state: &mut State, groups: &[(String, Vec<Row>)], now: u64, height: f32, acts: &mut Vec<Act>) {
    if let Shown::List(id) = state.shown.clone() {
        let rows = groups.first().map_or(&[][..], |g| g.1.as_slice());
        return list_view(ui, cx, state, &id, rows, now, acts);
    }
    if state.history.is_none() {
        for _ in 0..3 {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 120.0), Sense::hover());
            if ui.is_rect_visible(r) {
                super::sessions::shimmer_card(ui, r);
            }
        }
        return;
    }
    if groups.is_empty() {
        let (title, text) = if !state.query.trim().is_empty() {
            (tr("No video matches"), tr("Try a title, a channel, a world or a part of the link."))
        } else if !cx.snap.vrchat.log_found {
            (tr("VRChat was not found"), tr("Videos show up here once VRChat ran on this computer."))
        } else {
            (tr("No video yet"), tr("Videos the worlds' video players play in VRChat show up here, with the world and when, ready to copy or bookmark."))
        };
        return empty_state(ui, cx, title, text, height * 0.6);
    }
    let pick_first = std::mem::take(&mut state.pick_first);
    // The days side by side on wide pages.
    let mut m = super::Masonry::new(ui, "videos-history");
    for (g, (name, rows)) in groups.iter().enumerate() {
        let sub = tr_n("{count} video", "{count} videos", rows.len() as u64);
        m.add(g, |ui| {
            super::section(ui, cx, 1 + g, icons::CLOCK, name, Some(&sub), |ui| {
                ui.add_space(6.0);
                video_rows(ui, state, rows, now, pick_first && g == 0, acts);
            })
        });
    }
    m.finish(ui);
}

/// A list: its name with Rename and Delete (twice), its videos, the latest saved first.
fn list_view(ui: &mut Ui, cx: &Cx, state: &mut State, id: &str, rows: &[Row], now: u64, acts: &mut Vec<Act>) {
    let Some(list) = state.bookmarks.as_ref().and_then(|b| b.list(id)).cloned() else { return };
    let pick_first = std::mem::take(&mut state.pick_first);
    let sub = if list.videos.is_empty() { tr("Empty for now") } else { tr("The latest saved first") };
    // As wide as the page: its videos go side by side on wide ones.
    super::section_up_to(ui, cx, f32::INFINITY, 1, icons::BOOKMARK_SIMPLE, &list.name, Some(sub), |ui| {
        // Rename and Delete in the card's heading, at its right.
        let header = Rect::from_min_size(ui.min_rect().min, vec2(ui.available_width(), 40.0));
        let confirming = state.confirm.is_some();
        let delete = Button::new(if confirming { tr("Delete it?") } else { tr("Delete") }).icon(icons::TRASH).variant(Variant::Danger).size(Size::Sm);
        let delete = delete.id_salt(("videos-list-delete", id));
        let dw = delete.desired_width(ui);
        if delete.confirm_at(ui, Rect::from_min_size(pos2(header.right() - dw, header.center().y - 16.0), vec2(dw, 32.0))) {
            acts.push(Act::Delete);
        }
        let rename = Button::new(tr("Rename")).icon(icons::PENCIL_SIMPLE).variant(Variant::Ghost).size(Size::Sm).id_salt(("videos-list-rename", id));
        let rw = rename.desired_width(ui);
        if rename.show_at(ui, Rect::from_min_size(pos2(header.right() - dw - 8.0 - rw, header.center().y - 16.0), vec2(rw, 32.0))).clicked() {
            state.renaming = Some((list.name.clone(), true));
        }
        if state.renaming.is_some() {
            ui.add_space(12.0);
            rename_row(ui, state, &list.name, acts);
        }
        ui.add_space(6.0);
        if rows.is_empty() {
            let text = if state.query.trim().is_empty() {
                tr("Nothing here yet: press a video's bookmark in the history to keep it in this list.")
            } else {
                tr("No video of this list matches.")
            };
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
            widgets::text(ui.painter(), pos2(r.left() + 48.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(text, 12.5).color(theme::fg_3()));
            return;
        }
        video_rows(ui, state, rows, now, pick_first, acts);
    });
}

/// The list's name being typed: Save and Cancel (Enter saves, Escape cancels).
fn rename_row(ui: &mut Ui, state: &mut State, old: &str, acts: &mut Vec<Act>) {
    let Some((name, focus)) = state.renaming.as_mut() else { return };
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let field = Rect::from_min_size(row.min + vec2(0.0, 2.0), vec2((row.width() - 216.0).min(360.0), 36.0));
    let mut fu = ui.new_child(UiBuilder::new().max_rect(field).id_salt("videos-rename"));
    let resp = widgets::text_field(&mut fu, "videos-rename-name", name, old, field.width(), false);
    if std::mem::take(focus) {
        resp.request_focus();
    }
    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let x = field.right() + 12.0;
    let save = Button::new(tr("Save")).variant(Variant::Primary).width(96.0).id_salt("videos-rename-save");
    let saved = save.show_at(ui, Rect::from_min_size(pos2(x, row.top()), vec2(96.0, 40.0))).clicked();
    let cancel = Button::new(tr("Cancel")).variant(Variant::Ghost).width(96.0).id_salt("videos-rename-cancel");
    let cancelled = cancel.show_at(ui, Rect::from_min_size(pos2(x + 104.0, row.top()), vec2(96.0, 40.0))).clicked();
    if saved || enter {
        acts.push(Act::Rename(name.clone()));
    } else if cancelled || ui.input(|i| i.key_pressed(Key::Escape)) {
        state.renaming = None;
    }
}

/// The rows of a card, only those in view drawn (a history holds thousands); `pick_first` opens the bookmark picker on
/// the first (the screenshot review).
fn video_rows(ui: &mut Ui, state: &mut State, rows: &[Row], now: u64, pick_first: bool, acts: &mut Vec<Act>) {
    let copied = state.copied.as_ref().map(|(u, _)| u.clone());
    // Side by side, newest first along each line, when the card is wide enough for rows of ROW_MIN_W.
    let (cols, w) = orbiters_ui::layout::columns(ui.available_width(), ROW_MIN_W, ROW_GAP);
    let cols = cols.min(rows.len().max(1));
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), rows.len().div_ceil(cols) as f32 * ROW_H), Sense::hover());
    for (i, row) in rows.iter().enumerate() {
        let r = Rect::from_min_size(pos2(area.left() + (i % cols) as f32 * (w + ROW_GAP), area.top() + (i / cols) as f32 * ROW_H), vec2(w, ROW_H));
        if !ui.is_rect_visible(r) {
            continue;
        }
        if i >= cols {
            ui.painter().hline(r.x_range(), r.top(), egui::Stroke::new(1.0, theme::white(0.05)));
        }
        let texture = row.youtube.as_deref().and_then(|id| state.thumbs.get(id, ui.input(|i| i.time)));
        let was_copied = copied.as_deref() == Some(row.url.as_str());
        let held = state.bookmarks.as_ref().is_some_and(|b| b.any_holds(&row.url));
        if let Some(act) = video_row(ui, r, row, texture, held, was_copied, now) {
            acts.push(act);
        }
        if pick_first && i == 0 {
            acts.push(Act::Pick(row.clone(), bookmark_button(r)));
        }
    }
}

/// Where a row's bookmark button sits.
fn bookmark_button(row: Rect) -> Rect {
    let x = row.right() - 124.0 - 8.0 - 36.0 - 4.0 - 36.0;
    Rect::from_min_size(pos2(x, row.center().y - 18.0), vec2(36.0, 36.0))
}

/// One video: its thumbnail, its title (or link), where and when it played; Bookmark, Open and Copy link.
fn video_row(ui: &mut Ui, r: Rect, row: &Row, texture: Option<TextureId>, held: bool, copied: bool, now: u64) -> Option<Act> {
    let thumb = Rect::from_min_size(pos2(r.left(), r.center().y - THUMB.y / 2.0), THUMB);
    let buttons_w = 124.0 + 8.0 + 36.0 + 4.0 + 36.0;
    let x = thumb.right() + 16.0;
    let text_w = (r.right() - buttons_w - 20.0 - x).max(80.0);
    {
        let p = ui.painter();
        match texture {
            Some(id) => {
                p.add(
                    egui::epaint::RectShape::filled(thumb, theme::radius(10.0), Color32::WHITE)
                        .with_texture(id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
                );
            }
            None => {
                p.rect_filled(thumb, theme::radius(10.0), theme::surface_soft());
                let glyph = if row.youtube.is_some() { icons::YOUTUBE_LOGO } else { icons::FILM_STRIP };
                widgets::icon(p, thumb.center(), glyph, 22.0, theme::fg_4());
            }
        }
        if row.plays > 1 {
            super::sessions::badge(p, ui, pos2(thumb.right() - 6.0, thumb.bottom() - 28.0), &format!("×{}", row.plays), theme::black(0.7), theme::fg(), true);
        }
        let (title, title_c) = match &row.title {
            Some(t) => (t.clone(), theme::fg()),
            None => (short_link(&row.url), theme::fg_2()),
        };
        let mut job = Txt::new(title, 14.0).weight(weight::MEDIUM).line_height(19.0).color(title_c).job(text_w);
        job.wrap.max_rows = 2;
        let galley = p.layout_job(job);
        let title_h = galley.size().y;
        let top = r.center().y - (title_h + 22.0) / 2.0;
        p.galley(pos2(x, top), galley, title_c);
        let when = if row.plays > 0 { crate::pages::ago(ui.ctx(), row.when, now) } else { tr!("saved {when}", when = crate::pages::ago(ui.ctx(), row.when, now)) };
        let meta: Vec<&str> = [row.channel.as_deref(), row.world.as_deref(), Some(when.as_str())].into_iter().flatten().filter(|s| !s.is_empty()).collect();
        widgets::text_truncated(p, pos2(x, top + title_h + 14.0), Align2::LEFT_CENTER, &Txt::new(meta.join(" · "), 12.5).color(theme::fg_3()), text_w);
    }
    let mut act = None;
    // At the right: the bookmark, Open, Copy link.
    let bookmark_r = bookmark_button(r);
    let label = if held { tr("In your lists: change them") } else { tr("Bookmark in a list") };
    let resp = IconButton::new(icons::BOOKMARK_SIMPLE, label).id_salt(("videos-bookmark", &row.url)).show_at(ui, bookmark_r);
    if held {
        // Filled, in the accent, over the outline the button drew.
        let p = ui.painter();
        p.circle_filled(bookmark_r.center(), 12.0, theme::surface());
        widgets::icon_fill(p, bookmark_r.center(), icons::BOOKMARK_SIMPLE, 17.0, theme::accent());
    }
    if resp.clicked() {
        act = Some(Act::Pick(row.clone(), bookmark_r));
    }
    let open_r = Rect::from_min_size(pos2(bookmark_r.right() + 4.0, bookmark_r.top()), vec2(36.0, 36.0));
    if video_history::openable(&row.url) {
        if IconButton::new(icons::ARROW_SQUARE_OUT, mark("Open in the browser")).id_salt(("videos-open", &row.url)).show_at(ui, open_r).clicked() {
            act = Some(Act::Open(row.url.clone()));
        }
    } else {
        widgets::icon(ui.painter(), open_r.center(), icons::ARROW_SQUARE_OUT, 17.0, theme::alpha(theme::fg_4(), 0.5));
    }
    let (label, glyph) = if copied { (tr("Copied"), icons::CHECK) } else { (tr("Copy link"), icons::COPY) };
    let copy = Button::new(label).icon(glyph).size(Size::Sm).width(124.0).id_salt(("videos-copy", &row.url)).hint(tr("Paste it in a world's video player"));
    if copy.show_at(ui, Rect::from_min_size(pos2(r.right() - 124.0, r.center().y - 16.0), vec2(124.0, 32.0))).clicked() {
        act = Some(Act::Copy(row.url.clone()));
    }
    act
}

/// A link without its scheme and `www.`, as a title.
fn short_link(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.strip_prefix("www.").unwrap_or(rest).to_string()
}

fn empty_state(ui: &mut Ui, cx: &Cx, title: &str, text: &str, height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height.max(240.0)), Sense::hover());
    let (rect, alpha) = super::entering(ui, cx, 1, rect);
    let mut p = ui.painter().clone();
    p.multiply_opacity(alpha);
    let t = anim::clock(ui.ctx(), 30.0);
    let bob = -8.0 * ease::in_out(ease::mirror(t, 2.0));
    let lr = Rect::from_center_size(rect.center() + vec2(0.0, -44.0 + bob), logo::size_for_width(70.0));
    logo::paint(&p, lr, logo::Look::default());
    widgets::text(&p, rect.center() + vec2(0.0, 18.0), Align2::CENTER_CENTER, &Txt::new(title, 17.0).weight(weight::SEMIBOLD));
    let g = p.layout_job({
        let mut job = Txt::new(text, 13.0).color(theme::fg_3()).line_height(20.0).job(380.0);
        job.halign = egui::Align::Center;
        job
    });
    p.galley(pos2(rect.center().x, rect.center().y + 34.0), g, theme::fg_3());
}

/// The bookmark picker: the lists (pressed to put the video in or take it out) and a new list.
fn picker(ctx: &egui::Context, state: &mut State, acts: &mut Vec<Act>) {
    let Some(p) = state.picker.as_mut() else { return };
    let lists: Vec<(String, String, usize, bool)> = state
        .bookmarks
        .as_ref()
        .map(|b| b.lists.iter().map(|l| (l.id.clone(), l.name.clone(), l.videos.len(), b.holds(&l.id, &p.row.url))).collect())
        .unwrap_or_default();
    let id = Id::unique(("videos-picker", p.serial));
    let width = 300.0;
    let list_h = (lists.len() as f32 * 40.0).min(240.0);
    let h = 64.0 + list_h + if lists.is_empty() { 0.0 } else { 9.0 } + 52.0;
    let window = ctx.content_rect();
    let mut base = Rect::from_min_size(pos2(p.anchor.right() - width, p.anchor.bottom() + 6.0), vec2(width, h));
    if base.bottom() > window.bottom() - 12.0 {
        base = base.translate(vec2(0.0, -(h + p.anchor.height() + 12.0)));
    }
    // Grows out of its button's corner, and back into it.
    let closing = p.closing;
    let pop = orbiters_ui::popup::popover(ctx, id, p.anchor, base, !closing);
    if pop.gone {
        state.picker = None;
        return;
    }
    let rect = pop.rect;
    let area = orbiters_ui::front::area(ctx, id, Order::Foreground, !closing).fixed_pos(rect.min).interactable(!closing).show(ctx, |ui| {
        ui.set_min_size(rect.size());
        ui.multiply_opacity(pop.opacity);
        if closing {
            ui.disable();
        }
        let painter = ui.painter().clone();
        widgets::panel(&painter, rect, 16.0);
        widgets::text(&painter, pos2(rect.left() + 18.0, rect.top() + 24.0), Align2::LEFT_CENTER, &Txt::new(tr("Save in a list"), 13.5).weight(weight::SEMIBOLD));
        let title = p.row.title.clone().unwrap_or_else(|| short_link(&p.row.url));
        widgets::text_truncated(
            &painter,
            pos2(rect.left() + 18.0, rect.top() + 44.0),
            Align2::LEFT_CENTER,
            &Txt::new(title, 12.0).color(theme::fg_3()),
            width - 36.0,
        );
        let list = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top() + 62.0), vec2(width - 16.0, list_h));
        if !lists.is_empty() {
            let mut lu = ui.new_child(UiBuilder::new().max_rect(list).id_salt("videos-picker-lists"));
            egui::ScrollArea::vertical().max_height(list_h).show(&mut lu, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                for (lid, name, count, holds) in &lists {
                    let (row, resp) = ui.allocate_exact_size(vec2(list.width(), 40.0), Sense::click());
                    let hover = anim::toggle(ctx, Id::unique(("videos-picker-row", lid)), resp.hovered(), Spring::SNAPPY);
                    let rp = ui.painter();
                    rp.rect_filled(row, theme::radius(10.0), theme::white(0.06 * hover));
                    let mark = pos2(row.left() + 20.0, row.center().y);
                    if *holds {
                        widgets::icon_fill(rp, mark, icons::CHECK_CIRCLE, 18.0, theme::accent());
                    } else {
                        widgets::icon(rp, mark, icons::CIRCLE, 18.0, theme::fg_4());
                    }
                    widgets::text_truncated(
                        rp,
                        pos2(row.left() + 40.0, row.center().y),
                        Align2::LEFT_CENTER,
                        &Txt::new(name, 13.0).weight(weight::MEDIUM),
                        row.width() - 90.0,
                    );
                    widgets::text(rp, pos2(row.right() - 12.0, row.center().y), Align2::RIGHT_CENTER, &Txt::new(count.to_string(), 12.0).color(theme::fg_4()));
                    if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        acts.push(Act::Toggle(lid.clone(), p.row.clone()));
                    }
                }
            });
            painter.hline((rect.left() + 16.0)..=(rect.right() - 16.0), list.bottom() + 4.0, egui::Stroke::new(1.0, theme::white(0.06)));
        }
        // A new list, the video in it.
        let y = rect.bottom() - 46.0;
        let field = Rect::from_min_size(pos2(rect.left() + 16.0, y), vec2(width - 32.0 - 72.0 - 8.0, 36.0));
        let mut fu = ui.new_child(UiBuilder::new().max_rect(field).id_salt("videos-picker-new"));
        let hint = if lists.is_empty() { tr("Favourites") } else { tr("New list") };
        let resp = widgets::text_field(&mut fu, "videos-picker-name", &mut p.name, hint, field.width(), false);
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let add = Button::new(tr("Add")).icon(icons::PLUS).variant(Variant::Primary).size(Size::Sm).width(72.0).id_salt("videos-picker-add");
        if add.show_at(ui, Rect::from_min_size(pos2(field.right() + 8.0, y + 2.0), vec2(72.0, 32.0))).clicked() || enter {
            acts.push(Act::CreateWith(p.name.clone(), p.row.clone()));
        }
    });
    let anchor = p.anchor;
    let outside = area.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|q| anchor.contains(q)));
    if outside || ctx.input(|i| i.key_pressed(Key::Escape)) {
        acts.push(Act::ClosePicker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Night PAGES-UI-7: the rows on screen are fetched first, rows scrolled past leave the queue, and a thumbnail
    /// that could not be fetched is asked again when its row shows after a doubling wait (it never was).
    #[test]
    fn video_thumbnails_fetch_what_shows_first_and_retry_what_failed() {
        let mut t = Thumbs::default();
        for (id, at) in [("passed", 0.0), ("passed-too", 0.5), ("on-screen", 1.0)] {
            assert!(t.get(id, at).is_none());
        }
        t.get("on-screen", 2.6);
        assert_eq!(t.next(2.6).as_deref(), Some("on-screen"), "what shows now, first");
        assert_eq!(t.next(2.6), None, "rows scrolled past are not fetched");
        t.failed.insert("broken".into(), (3.0, 1));
        t.get("broken", 5.0);
        assert!(t.queue.is_empty(), "not before its wait");
        t.get("broken", 8.5);
        assert_eq!(t.next(8.5).as_deref(), Some("broken"), "asked again after it");
        t.failed.insert("broken".into(), (9.0, 2));
        t.get("broken", 15.0);
        assert!(t.queue.is_empty(), "the wait doubled");
    }

    fn played(url: &str, last: u64, title: Option<&str>) -> Played {
        Played { url: url.into(), title: title.map(str::to_string), first: last, last, plays: 1, world: Some("Movie Night".into()), ..Default::default() }
    }

    #[test]
    fn rows_are_found_by_title_channel_world_or_link_and_grouped_by_day() {
        let row = Row::of_played(&played("https://youtu.be/36YnV9STBqc", 100, Some("Lo-fi beats")));
        assert_eq!(row.youtube.as_deref(), Some("36YnV9STBqc"));
        assert!(row.matches("") && row.matches("lo-fi") && row.matches("movie") && row.matches("36ynv9"));
        assert!(!row.matches("jazz"));
        let now = 1_792_900_000;
        let rows = vec![
            Row { when: now - 60, ..Default::default() },
            Row { when: now - 120, ..Default::default() },
            Row { when: now - 3 * 86_400, ..Default::default() },
            Row { when: now - 400 * 86_400, ..Default::default() },
        ];
        let groups = by_day(rows, now);
        assert_eq!(groups.len(), 3, "{:?}", groups.iter().map(|g| &g.0).collect::<Vec<_>>());
        assert_eq!((groups[0].0.as_str(), groups[0].1.len()), ("Today", 2));
        assert!(groups[2].0.ends_with("2025"), "a year ago: its year");
        assert_eq!(short_link("https://www.youtube.com/watch?v=x"), "youtube.com/watch?v=x");
    }

    #[test]
    fn a_bookmark_takes_the_title_the_history_found_since() {
        let mut history = History::default();
        history.record("https://www.youtube.com/watch?v=36YnV9STBqc", 100, None, Some("Movie Night"));
        history.set_title("https://youtu.be/36YnV9STBqc", Some(("Lo-fi beats".into(), "Lofi Girl".into())));
        let bookmark = Bookmark { url: "https://youtu.be/36YnV9STBqc".into(), added: 5, ..Default::default() };
        let row = Row::of_bookmark(&bookmark, Some(&history));
        assert_eq!((row.title.as_deref(), row.channel.as_deref(), row.world.as_deref()), (Some("Lo-fi beats"), Some("Lofi Girl"), Some("Movie Night")));
        assert_eq!((row.when, row.plays), (5, 0));
        assert_eq!(row.bookmark(9).added, 9);
    }

    use crate::backend::{Backend, Ctl};
    use crate::state::Snapshot;
    use egui::{Event, Modifiers, PointerButton, RawInput};
    use orbiters_core::settings::Settings;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// What the page asked of N4N0 (copies).
    #[derive(Default)]
    struct Recorder {
        n4n0: Mutex<Vec<N4n0Command>>,
    }

    impl Backend for Recorder {
        fn n4n0(&self, command: N4n0Command) {
            self.n4n0.lock().unwrap().push(command);
        }
        fn patch_settings(&self, _patch: serde_json::Value) {}
        fn view(&self) -> Arc<Snapshot> {
            Arc::new(Snapshot::new(Settings::default()))
        }
        fn take_notices(&self) -> Vec<crate::controller::Notice> {
            Vec::new()
        }
        fn library_version(&self) -> u64 {
            0
        }
        fn attach(&self, _ctx: &egui::Context) {}
        fn start_capture(&self) {}
        fn stop_capture(&self) {}
        fn pause_capture(&self, _paused: bool) {}
        fn start_test_capture(&self) {}
        fn import_raw_capture(&self, _dir: PathBuf) {}
        fn trash_session(&self, _dir: PathBuf) {}
        fn clean_up(&self, _trim: Vec<PathBuf>, _delete: Vec<PathBuf>) {}
        fn reconstruct(&self, _dir: PathBuf) {}
        fn stop_reconstruction(&self) {}
        fn unqueue_reconstruction(&self, _dir: PathBuf) {}
        fn send_to_mixer(&self, _dir: PathBuf, _title: String) {}
        fn sign_in(&self) {}
        fn reopen_sign_in(&self) {}
        fn cancel_sign_in(&self) {}
        fn sign_out(&self) {}
        fn request_worker_log(&self) {}
        fn worker_log(&self) -> Option<Vec<String>> {
            None
        }
        fn upload_pictures(&self, _world_id: String, _world_name: String, _paths: Vec<PathBuf>) {}
        fn upload_preview(&self, _world_id: String, _world_name: String, _session: PathBuf) {}
        fn request_maps(&self) {}
        fn maps(&self) -> Option<std::sync::Arc<crate::state::MapsMine>> {
            None
        }
        fn calibrated(&self, _calibration: orbiters_contracts::performance::calibration::Calibration) {}
        fn frames(&self, _chunk: crate::state::FrameChunk) {}
        fn headset(&self, _in_use: bool) {}
        fn walk_boost(&self, _strength: f32) {}
        fn osc_reconnect(&self) {}
        fn updates(&self, _action: crate::ipc::UpdateAction) {}
        fn quit(&self) {}
    }

    /// The page in a 1048 × 780 window, frame by frame, over made-up videos (its bookmarks saved in a file of the test's).
    struct Harness {
        ctx: egui::Context,
        recorder: Arc<Recorder>,
        ctl: Ctl,
        snap: Snapshot,
        toasts: toast::Toasts,
        state: State,
        time: f64,
    }

    impl Harness {
        fn new(history: History, file: PathBuf) -> Self {
            let ctx = egui::Context::default();
            orbiters_ui::install(&ctx);
            widgets::watch_controls();
            let recorder = Arc::new(Recorder::default());
            let ctl: Ctl = recorder.clone();
            let mut state = State::default();
            state.stand_in(history, Bookmarks::default(), None, false, Vec::new());
            state.bookmarks_file = Some(file);
            Self { ctx, recorder, ctl, snap: Snapshot::new(Settings::default()), toasts: toast::Toasts::default(), state, time: 0.0 }
        }

        fn frame(&mut self, events: Vec<Event>) {
            self.frame_said(events);
        }

        /// A frame with nothing done; how soon it asked for the next, and the texts it painted, in small letters.
        fn frame_lower(&mut self) -> (std::time::Duration, String) {
            let (asked, said) = self.frame_said(Vec::new());
            (asked, said.to_lowercase())
        }

        /// A frame; how soon it asked for the next, and the texts it painted.
        fn frame_said(&mut self, events: Vec<Event>) -> (std::time::Duration, String) {
            self.time += 1.0 / 60.0;
            let input =
                RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1048.0, 780.0))), time: Some(self.time), events, ..Default::default() };
            let (ctl, snap, toasts, state) = (&self.ctl, &self.snap, &mut self.toasts, &mut self.state);
            let mut output = self.ctx.run_ui(input, |ui| {
                let mut cx = Cx { ctl, snap, toasts: &mut *toasts, entered: 0.0, visit: 1, windowed: true, gpu: None, top: 40.0, go: None };
                show(ui, &mut cx, state);
            });
            output.textures_delta.clear();
            (crate::pages::rest::asked(&output), crate::pages::rest::texts(&output))
        }

        fn settle(&mut self) {
            for _ in 0..90 {
                self.frame(Vec::new());
            }
        }

        fn click(&mut self, at: egui::Pos2) {
            let button = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            self.frame(vec![Event::PointerMoved(at), button(true)]);
            self.frame(vec![button(false)]);
        }

        fn control(&self, label: &str) -> Rect {
            widgets::control(&self.ctx, label).unwrap_or_else(|| panic!("{label} is not drawn")).0
        }
    }

    #[test]
    fn a_video_is_bookmarked_in_a_new_list_copied_and_taken_out() {
        let dir = std::env::temp_dir().join(format!("orbiters-videos-page-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("video-bookmarks.json");
        let _ = std::fs::remove_file(&file);
        let mut history = History::default();
        history.record("https://www.youtube.com/watch?v=36YnV9STBqc", now_secs() - 60, None, Some("Movie Night"));
        let mut h = Harness::new(history, file.clone());
        h.settle();
        // Copy link: N4N0 copies it.
        h.click(h.control("Copy link").center());
        assert_eq!(h.recorder.n4n0.lock().unwrap().as_slice(), &[N4n0Command::CopyText { text: "https://www.youtube.com/watch?v=36YnV9STBqc".into() }]);
        // The bookmark opens the picker; Add with no name makes Favourites, the video in it.
        h.click(h.control("Bookmark in a list").center());
        h.settle();
        h.click(h.control("Add").center());
        let saved = Bookmarks::read(&file).0;
        assert_eq!(saved.lists.iter().map(|l| (l.name.as_str(), l.videos.len())).collect::<Vec<_>>(), vec![("Favourites", 1)]);
        h.settle();
        assert!(widgets::control(&h.ctx, "In your lists: change them").is_some(), "the row's bookmark says so");
        // The picker lists it, ticked: pressing it takes the video out.
        let add = h.control("Add");
        let (right, bottom) = (add.right() + 16.0, add.top() + 44.0);
        let top = bottom - (64.0 + 40.0 + 9.0 + 52.0);
        h.click(egui::pos2(right - 150.0, top + 82.0));
        assert!(Bookmarks::read(&file).0.lists[0].videos.is_empty(), "taken out");
        // Escape closes the picker: it shrinks back into its button, then is gone.
        h.frame(vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        h.frame(Vec::new());
        assert!(h.state.picker.as_ref().is_some_and(|p| p.closing));
        h.settle();
        assert!(h.state.picker.is_none());
        std::fs::remove_dir_all(dir).ok();
    }

    /// The page drew itself every five seconds to look at its file, which also kept "5 min ago" and "Today" right.
    /// Drawn only when the file changed, an untouched page went on saying "just now" of a video played an hour ago,
    /// and kept yesterday's under Today. The clock asks for its frames now: here the file never changes, nobody
    /// touches the page, and only the time moves.
    #[test]
    fn what_the_clock_alone_changes_asks_for_its_frame() {
        let dir = std::env::temp_dir().join(format!("orbiters-videos-clock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Half a minute before the local midnight, a video played fifty seconds ago.
        let (_, to_midnight) = crate::pages::local_day(now_secs());
        let now = now_secs() + to_midnight - 30;
        let mut history = History::default();
        history.record("https://www.youtube.com/watch?v=36YnV9STBqc", now - 50, None, Some("Movie Night"));
        let mut h = Harness::new(history, dir.join("video-bookmarks.json"));
        h.state.clock = Some(now);
        h.settle();
        let (asked, said) = h.frame_lower();
        assert!(said.contains("just now") && said.contains("today"), "{said}");
        // In ten seconds it is a minute old.
        assert!(asked > Duration::from_secs(9) && asked < Duration::from_secs(11), "the next frame is the text's: {asked:?}");
        h.state.clock = Some(now + 10);
        let (asked, said) = h.frame_lower();
        assert!(said.contains("1 min ago") && !said.contains("just now") && said.contains("today"), "{said}");
        // Twenty seconds later it is another day.
        h.settle();
        let (asked_settled, _) = h.frame_lower();
        assert!(asked_settled > Duration::from_secs(19) && asked_settled < Duration::from_secs(21), "the next frame is midnight's: {asked:?} then {asked_settled:?}");
        h.state.clock = Some(now + 30);
        let (_, said) = h.frame_lower();
        assert!(said.contains("yesterday") && !said.contains("today"), "{said}");
        h.settle();
        // Then the minutes, one frame each.
        let (asked, said) = h.frame_lower();
        assert!(said.contains("1 min ago"), "{said}");
        assert!(asked > Duration::from_secs(39) && asked < Duration::from_secs(41), "{asked:?}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn day_names_and_the_new_list_name_follow_the_language_and_so_do_the_rows_made_before() {
        use orbiters_i18n::{in_language, Language};
        let now = now_secs();
        // The names of the days are made once and kept: a change of language makes them again.
        let mut history = History::default();
        history.record("https://www.youtube.com/watch?v=36YnV9STBqc", now - 60, None, Some("Movie Night"));
        let mut state = State::default();
        state.stand_in(history, Bookmarks::default(), None, false, Vec::new());
        assert_eq!(state.groups(now)[0].0, "Today");
        assert_eq!(in_language(Language::Fr, || state.groups(now)[0].0.clone()), "Aujourd’hui");
        assert_eq!(in_language(Language::Ja, || state.groups(now)[0].0.clone()), "今日");
        assert_eq!(state.groups(now)[0].0, "Today", "and back");
        // A date far back is a whole phrase of the language, not English words in its order.
        let (_, long_ago) = in_language(Language::Es, || day(now - 400 * 86_400, now));
        assert!(long_ago.contains(" de "), "{long_ago}");
        let (_, long_ago) = in_language(Language::Ja, || day(now - 400 * 86_400, now));
        assert!(long_ago.contains('年') && long_ago.ends_with('日'), "{long_ago}");
        // The chips' ids stay on the English words, whatever they say.
        let chip = Chip { shown: Some(Shown::History), label: mark("History").into(), fixed: true, count: None, glyph: icons::CLOCK, rect: Rect::NOTHING };
        assert_eq!(in_language(Language::Fr, || chip.text().to_string()), "Historique");
        assert_eq!(chip.label, "History");
        let own = Chip { label: "History".into(), fixed: false, ..chip };
        assert_eq!(in_language(Language::Fr, || own.text().to_string()), "History", "a list called History is the person's word");
        // Making the first list with no name: Favourites, in the language the field said it in.
        let mut bookmarks = Bookmarks::default();
        assert_eq!(first_list_name(&bookmarks, "  "), "Favourites");
        assert_eq!(in_language(Language::Fr, || first_list_name(&bookmarks, "")), "Favoris");
        assert_eq!(in_language(Language::Fr, || first_list_name(&bookmarks, "Musique")), "Musique");
        bookmarks.create("Musique");
        assert_eq!(in_language(Language::Fr, || first_list_name(&bookmarks, "")), "", "later blank names are numbered by the bookmarks");
    }
}
