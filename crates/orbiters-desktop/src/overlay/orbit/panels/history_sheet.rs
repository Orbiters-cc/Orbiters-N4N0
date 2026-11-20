//! History (REQUEST19 2: "the Video tab content to be a new page in the overlay accessible from the youtube page on a icon
//! button History"): every video the worlds' players played in VRChat, the latest first, as the window's Videos tab lists
//! them, from the same store (`orbiters_core::video_history`, which the tray service keeps from VRChat's logs). Reached from
//! YouTube's page by its round History button in the page's own head row, before what is searched: on its search, in the place
//! N4N0's menu keeps for its own Back (which the sheet's head has instead, so it is empty there), and on its results before the
//! search's field, the same place. Back (the head's, or this page's own "YouTube" in that same place) goes back.
//!
//! In the overlay's own design, not a smaller copy of the window's: one video a row across the whole page, as YouTube's
//! results are, its picture, its title on two lines, where and when it played, and the two buttons a result has: Copy link
//! (for the world's player; the row pressed does it too) and Drop in world (a card anyone can grab), both through N4N0's
//! menu's own effects, the YouTube page's way of playing a video. What the window also offers that does not belong in a
//! headset is left to it: opening a link in the computer's browser, and the bookmark lists (which the window's page alone
//! writes). One list that scrolls, only its rows in view laid and their pictures asked for (the menu's own cache).

use super::*;
use orbiters_core::video_history::{self, History, Played};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};

/// Where the rows start, a row's height and the room between two, a picture's width (YouTube's 16:9), and the buttons'.
pub const TOP: f32 = 60.0;
const ROW: f32 = 116.0;
const ROW_GAP: f32 = 12.0;
const PICTURE: f32 = 184.0;
const ACTION_W: f32 = 184.0;
/// The page's own way back, at its top left.
const BACK: Rect = Rect { min: Pos2 { x: 24.0, y: 8.0 }, max: Pos2 { x: 224.0, y: 52.0 } };
/// History's button on YouTube's results, before the search's field; on YouTube's search, the place N4N0's menu keeps for its
/// round Back, empty on the sheet (`Menu::back_on_the_sheet`), and the hover N4N0's page is given while it is pointed at.
pub const RESULTS_BUTTON: Rect = Rect { min: Pos2 { x: 24.0, y: 8.0 }, max: Pos2 { x: 68.0, y: 52.0 } };
pub const SEARCH_BUTTON: Rect = Rect { min: Pos2 { x: 32.0, y: 24.0 }, max: Pos2 { x: 88.0, y: 80.0 } };
pub const ON_MENU: usize = usize::MAX;
/// How often the store's file is looked at while the page shows.
const LOOK_EVERY: Duration = Duration::from_secs(5);

/// The page's words.
pub const TITLE: &str = mark("History");
pub const OPEN_WHY: &str = mark("Every video the worlds' players played, to copy or drop again.");
pub const EMPTY_WHY: &str = mark("Videos the worlds' video players play in VRChat show up here, ready to copy or drop in a world again.");

/// Where the history is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HistoryState {
    /// Being read.
    #[default]
    Reading,
    /// Read.
    Ready,
}

/// A row as the page shows it: its place in the history (the latest first), its title (or its link), where and when it
/// played, how many times, whether it is YouTube's, and its picture once there.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryRow {
    pub index: usize,
    /// Its link: what a press on the row copies or drops, as shown when it was pressed (the file may be read again meanwhile).
    pub url: String,
    pub title: String,
    /// The title is the link: no title was found for it.
    pub link_only: bool,
    pub meta: String,
    pub plays: u32,
    pub youtube: bool,
    pub picture: Option<TextureId>,
}

/// What the page draws: where the history is, how many videos it has, the rows in view, the row whose link was just
/// copied, whether a card can be dropped, VRChat's logs found on this computer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryFacts {
    pub state: HistoryState,
    pub count: usize,
    pub rows: Vec<HistoryRow>,
    pub copied: Option<usize>,
    pub log_found: bool,
}

/// The rows in view, scrolled this far: what is laid and whose pictures are asked for, and one more row each side.
pub fn shown(scroll: f32, count: usize) -> std::ops::Range<usize> {
    let first = ((scroll - TOP) / (ROW + ROW_GAP)).floor().max(0.0) as usize;
    let rows = (576.0 / (ROW + ROW_GAP)).ceil() as usize + 2;
    first.saturating_sub(1).min(count)..(first + rows).min(count)
}

/// Where row `index` lies, scrolled this far.
fn row_rect(index: usize, scroll: f32) -> Rect {
    r(24.0, TOP + 4.0 + index as f32 * (ROW + ROW_GAP) - scroll, 1024.0 - 48.0, ROW)
}

/// How far the list scrolls.
pub fn max_scroll(facts: &Facts) -> f32 {
    (TOP + 4.0 + facts.history.count as f32 * (ROW + ROW_GAP) + 8.0 - 576.0).max(0.0)
}

/// Where the list may come to rest: each row's top.
pub fn row_tops(facts: &Facts) -> impl Iterator<Item = f32> {
    (0..facts.history.count).map(|i| i as f32 * (ROW + ROW_GAP))
}

/// What can be pressed: the way back, then each row in view with its two buttons on it.
pub fn items(facts: &Facts) -> Vec<Item> {
    let mut out = Vec::new();
    for row in &facts.history.rows {
        let rect = row_rect(row.index, facts.gallery_scroll);
        if rect.bottom() < TOP || rect.top() >= 576.0 {
            continue;
        }
        out.push(Item { action: Action::HistoryCopy(row.index), rect, look: Look::HistoryRow(row.index) });
        let x = rect.right() - ACTION_W;
        let middle = rect.center().y;
        out.push(Item { action: Action::HistoryCopy(row.index), rect: r(x, middle - 43.0, ACTION_W, 40.0), look: Look::HistoryCopy(row.index) });
        out.push(Item { action: Action::HistoryDrop(row.index), rect: r(x, middle + 3.0, ACTION_W, 40.0), look: Look::HistoryDrop(row.index) });
    }
    out.push(Item { action: Action::Back, rect: BACK, look: Look::HistoryBack });
    out
}

/// The page under its items: its name and how many, or why there is nothing.
pub fn draw_page(p: &Painter, facts: &Facts) {
    let h = &facts.history;
    let x = BACK.right() + 18.0;
    text(p, pos2(x, BACK.center().y - 10.0), Align2::LEFT_CENTER, tr(TITLE), 21.0, weight::BOLD, look::LABEL);
    let sub = match h.state {
        HistoryState::Reading => tr("Reading VRChat's logs…").to_string(),
        HistoryState::Ready if h.count == 0 => tr(OPEN_WHY).to_string(),
        HistoryState::Ready => orbiters_i18n::fill(
            &orbiters_i18n::tr_n("{total} video played in worlds, from VRChat's logs", "{total} videos played in worlds, from VRChat's logs", h.count as u64),
            &[("total", &orbiters_ui::format::integer(h.count as f64))],
        ),
    };
    let font = look::font(15.0, weight::MEDIUM);
    p.text(pos2(x, BACK.center().y + 11.0), Align2::LEFT_CENTER, fit(p, &sub, &font, 1000.0 - x), font, look::MUTED);
    if h.count > 0 {
        return;
    }
    let c = pos2(512.0, 300.0);
    let (icon, first, second) = match h.state {
        HistoryState::Reading => (icons::CLOCK_COUNTER_CLOCKWISE, tr("Reading VRChat's logs…"), ""),
        HistoryState::Ready if !h.log_found => (icons::FILM_STRIP, tr("VRChat was not found"), tr("Videos show up here once VRChat ran on this computer.")),
        HistoryState::Ready => (icons::FILM_STRIP, tr("No video yet"), tr(EMPTY_WHY)),
    };
    widgets::icon(p, c - vec2(0.0, 34.0), icon, 54.0, look::MUTED);
    text(p, c + vec2(0.0, 22.0), Align2::CENTER_CENTER, first, 22.0, weight::BOLD, look::LABEL);
    if !second.is_empty() {
        let mut job = egui::text::LayoutJob::simple(second.to_string(), look::font(16.0, weight::MEDIUM), look::MUTED, 620.0);
        job.halign = egui::Align::Center;
        let galley = p.layout_job(job);
        p.galley(pos2(c.x, c.y + 44.0), galley, look::MUTED);
    }
}

/// One of the page's items.
pub fn draw_item(p: &Painter, item: &Item, facts: &Facts, hovered: bool, pressed: bool) {
    let rect = item.rect;
    let list = p.with_clip_rect(Rect::from_min_max(pos2(0.0, TOP), pos2(1024.0, 576.0)));
    let row_of = |i: usize| facts.history.rows.iter().find(|row| row.index == i);
    match item.look {
        Look::HistoryBack => {
            // As YouTube's results have their search: a field-like pill, here the way back to YouTube.
            let radius = look::radius(rect.height() / 2.0);
            p.rect_filled(rect, radius, if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::TILE });
            p.rect_stroke(rect.shrink(0.5), radius, Stroke::new(1.0, Color32::from_white_alpha(if hovered { 110 } else { 44 })), egui::StrokeKind::Inside);
            widgets::icon(p, pos2(rect.left() + 26.0, rect.center().y), icons::ARROW_LEFT, 22.0, look::LABEL);
            widgets::icon(p, pos2(rect.left() + 58.0, rect.center().y), icons::YOUTUBE_LOGO, 22.0, tool_tint(Action::Page(Target::YouTube)));
            let font = look::font(18.0, weight::SEMIBOLD);
            p.text(pos2(rect.left() + 76.0, rect.center().y), Align2::LEFT_CENTER, fit(p, "YouTube", &font, rect.width() - 88.0), font, look::LABEL);
        }
        Look::HistoryRow(i) => {
            let Some(row) = row_of(i) else { return };
            let radii = [theme::squircle(14.0); 4];
            let picture = Rect::from_min_size(rect.min + vec2(0.0, (rect.height() - PICTURE * 9.0 / 16.0) / 2.0), vec2(PICTURE, PICTURE * 9.0 / 16.0));
            match row.picture {
                Some(id) => squircle_picture(&list, picture, radii, id, cover_uv(picture.size()), if pressed { Color32::from_gray(200) } else { Color32::WHITE }),
                None => {
                    orbiters_ui::paint::fill_convex(&list, &orbiters_ui::paint::squircle_points_corners(picture, radii), |_| theme::INK_750);
                    widgets::icon(&list, picture.center(), if row.youtube { icons::YOUTUBE_LOGO } else { icons::FILM_STRIP }, 40.0, look::MUTED);
                }
            }
            if hovered {
                list.rect_stroke(picture.shrink(1.0), look::radius(14.0), Stroke::new(2.0, look::accent()), egui::StrokeKind::Middle);
            }
            // Played more than once: how many times, where YouTube's results have their length.
            if row.plays > 1 {
                let font = look::font(14.0, weight::SEMIBOLD);
                let words = format!("\u{d7}{}", row.plays);
                let wide = list.layout_no_wrap(words.clone(), font.clone(), Color32::WHITE).size().x;
                let chip = Rect::from_min_size(picture.right_bottom() - vec2(wide + 20.0, 32.0), vec2(wide + 12.0, 24.0));
                list.rect_filled(chip, look::radius(6.0), Color32::from_black_alpha(190));
                list.text(chip.center(), Align2::CENTER_CENTER, words, font, look::LABEL);
            }
            let left = picture.right() + 16.0;
            let room = rect.right() - ACTION_W - 20.0 - left;
            let colour = if row.link_only { look::MUTED } else { look::LABEL };
            let mut job = egui::text::LayoutJob::simple(row.title.clone(), look::font(18.0, weight::BOLD), colour, room);
            job.wrap.max_rows = 2;
            job.wrap.break_anywhere = row.link_only;
            job.wrap.overflow_character = Some('\u{2026}');
            let title = list.layout_job(job);
            let high = title.size().y;
            let top = rect.center().y - (high + 26.0) / 2.0;
            list.galley(pos2(left, top), title, colour);
            let font = look::font(15.5, weight::MEDIUM);
            list.text(pos2(left, top + high + 14.0), Align2::LEFT_CENTER, fit(&list, &row.meta, &font, room), font, Color32::from_gray(200));
            list.line_segment([pos2(rect.left(), rect.bottom() + 6.0), pos2(rect.right(), rect.bottom() + 6.0)], Stroke::new(1.0, Color32::from_white_alpha(18)));
        }
        Look::HistoryCopy(i) => {
            // YouTube's results' Copy link: quiet in a list of many, solid when pointed at, and once copied it says so.
            let copied = facts.history.copied == Some(i);
            let radius = look::radius(rect.height() / 2.0);
            if copied || pressed || hovered {
                let fill = if copied { theme::mix(look::accent(), Color32::WHITE, 0.6) } else if pressed { theme::mix(look::accent(), Color32::BLACK, 0.25) } else { look::accent() };
                list.rect_filled(rect, radius, fill);
                let (words, font) = fitted(p, if copied { tr("Copied") } else { tr("Copy link") }, 16.0, weight::BOLD, rect.width() - 24.0, 13.5);
                list.text(rect.center(), Align2::CENTER_CENTER, words, font, look::on_accent());
            } else {
                list.rect_filled(rect, radius, theme::alpha(look::accent(), 0.16));
                list.rect_stroke(rect.shrink(0.75), radius, Stroke::new(1.5, look::accent()), egui::StrokeKind::Middle);
                let (words, font) = fitted(p, tr("Copy link"), 16.0, weight::BOLD, rect.width() - 24.0, 13.5);
                list.text(rect.center(), Align2::CENTER_CENTER, words, font, theme::mix(look::accent(), Color32::WHITE, 0.25));
            }
        }
        Look::HistoryDrop(_) => {
            // Greyed where the avatar cannot drop a card: pointed at, the caption says why.
            let can = facts.drop_why == DropWhy::Can;
            let radius = look::radius(rect.height() / 2.0);
            list.rect_filled(rect, radius, if !can { look::TILE } else if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::TILE });
            list.rect_stroke(rect.shrink(0.5), radius, Stroke::new(1.0, Color32::from_white_alpha(if can { 46 } else { 20 })), egui::StrokeKind::Middle);
            let (words, font) = fitted(p, tr("Drop in world"), 16.0, weight::SEMIBOLD, rect.width() - 24.0, 13.5);
            list.text(rect.center(), Align2::CENTER_CENTER, words, font, if can { look::LABEL } else { theme::alpha(look::LABEL, 0.4) });
        }
        _ => {}
    }
}

/// History's button: round as the menu's own Back it stands in for, the clock turning back, an outline that says it is pressable.
pub fn draw_round_button(p: &Painter, rect: Rect, hovered: bool, pressed: bool) {
    let fill = if pressed { look::TILE_DOWN } else if hovered { look::TILE_HOVER } else { look::TILE };
    p.circle_filled(rect.center(), rect.width() / 2.0, fill);
    p.circle_stroke(rect.center(), rect.width() / 2.0 - 0.75, Stroke::new(1.0, Color32::from_white_alpha(if hovered { 110 } else { 44 })));
    widgets::icon(p, rect.center(), icons::CLOCK_COUNTER_CLOCKWISE, rect.width() * 0.52, look::LABEL);
}

/// N4N0's menu on the sheet shows YouTube's search, where History has the place of the menu's own Back.
pub fn on_search(menu: &orbiters_n4n0_core::menu::Menu) -> bool {
    menu.page == orbiters_n4n0_core::menu::Page::Search && menu.back_on_the_sheet
}

/// What is at `p` on N4N0's menu's page on the sheet, for History: [`ON_MENU`] on its button, none elsewhere (the menu's own).
pub fn menu_hit(menu: &orbiters_n4n0_core::menu::Menu, p: Pos2) -> Option<usize> {
    (on_search(menu) && SEARCH_BUTTON.contains(p)).then_some(ON_MENU)
}

/// History's button over N4N0's menu's YouTube search, as the sheet draws the page.
pub fn draw_on_menu(p: &Painter, menu: &orbiters_n4n0_core::menu::Menu, hover: Option<usize>, pressed: Option<usize>) {
    if on_search(menu) {
        draw_round_button(p, SEARCH_BUTTON, hover == Some(ON_MENU), pressed == Some(ON_MENU));
    }
}

/// What the caption says of the page's parts.
pub fn caption(action: Action, facts: &Facts) -> Option<(String, String)> {
    let row = |i: usize| facts.history.rows.iter().find(|row| row.index == i);
    Some(match action {
        Action::VideoHistory if facts.sheet == Some(Sheet::VideoHistory) => (tr(TITLE).into(), tr("Back to YouTube.").into()),
        Action::VideoHistory => (tr(TITLE).into(), tr(OPEN_WHY).into()),
        Action::HistoryCopy(i) => (row(i).map_or(String::new(), |r| r.title.clone()), tr("Press to copy its link, for the world's video player.").into()),
        Action::HistoryDrop(_) if facts.drop_why != DropWhy::Can => (tr("Drop in world").into(), facts.drop_why.says().into()),
        Action::HistoryDrop(_) => (tr("Drop in world").into(), tr("Puts a card with the link in front of you, for anyone to grab.").into()),
        _ => return None,
    })
}

/// The history as the window's Videos tab reads it, from its file, on a thread: read when the page opens and again when the
/// tray service wrote it since (looked at every few seconds while the page shows). The latest first, as the tab lists them.
#[derive(Default)]
pub struct Store {
    /// The file; none (tests): nothing is read, only what is handed in.
    file: Option<std::path::PathBuf>,
    videos: Option<Arc<Vec<Played>>>,
    read_at: Option<SystemTime>,
    looked: Option<Instant>,
    reading: Option<mpsc::Receiver<Option<(Option<SystemTime>, History)>>>,
}

impl Store {
    /// The store reading `file` (`video_history::history_file()` in the app).
    pub fn at(file: Option<std::path::PathBuf>) -> Self {
        Self { file, ..Self::default() }
    }

    /// The app's own history file.
    pub fn app() -> Self {
        Self::at(Some(video_history::history_file()))
    }

    /// Made-up videos in place of the file's (tests, the screenshot review): nothing is read.
    pub fn stand_in(&mut self, history: History) {
        self.file = None;
        self.reading = None;
        self.videos = Some(Arc::new(latest_first(history)));
    }

    /// The videos, the latest first, once read.
    pub fn videos(&self) -> Option<&Arc<Vec<Played>>> {
        self.videos.as_ref()
    }

    /// Looks again now when the page opens.
    pub fn look_now(&mut self) {
        self.looked = None;
    }

    /// While the page shows: takes what a reading brought, and starts the next look when it is due. Whether the videos changed.
    pub fn follow(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.reading {
            match rx.try_recv() {
                Ok(read) => {
                    self.reading = None;
                    if let Some((at, history)) = read {
                        self.read_at = at;
                        self.videos = Some(Arc::new(latest_first(history)));
                        changed = true;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => self.reading = None,
            }
        }
        let Some(file) = self.file.clone() else { return changed };
        if self.looked.is_some_and(|at| now.duration_since(at) < LOOK_EVERY) {
            return changed;
        }
        self.looked = Some(now);
        let (tx, rx) = mpsc::channel();
        let (known, had) = (self.read_at, self.videos.is_some());
        std::thread::spawn(move || {
            let at = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
            // Unchanged since it was read: nothing to read again.
            let unchanged = had && at.is_some() && at == known;
            let _ = tx.send((!unchanged).then(|| (at, History::load(&file))));
        });
        self.reading = Some(rx);
        changed
    }

    /// What the page shows of it, scrolled this far: the rows in view only (there may be thousands), `picture` giving each
    /// YouTube video's picture once fetched, `now` the time (Unix seconds) "5 min ago" is told from.
    pub fn facts(&self, scroll: f32, copied: Option<usize>, log_found: bool, now: u64, picture: impl Fn(&str) -> Option<TextureId>) -> HistoryFacts {
        let Some(videos) = self.videos.as_ref() else { return HistoryFacts { log_found, ..HistoryFacts::default() } };
        let rows = shown(scroll, videos.len())
            .map(|index| {
                let v = &videos[index];
                let youtube = v.youtube();
                let when = orbiters_ui::format::relative_time(v.last, now);
                let meta = [v.channel.as_deref(), v.world.as_deref(), Some(when.as_str())].into_iter().flatten().filter(|s| !s.trim().is_empty()).collect::<Vec<_>>().join(" \u{b7} ");
                let title = v.title.clone().filter(|t| !t.trim().is_empty());
                HistoryRow {
                    index,
                    url: v.url.clone(),
                    link_only: title.is_none(),
                    title: title.unwrap_or_else(|| short_link(&v.url)),
                    meta,
                    plays: v.plays,
                    picture: youtube.as_deref().and_then(&picture),
                    youtube: youtube.is_some(),
                }
            })
            .collect();
        HistoryFacts { state: HistoryState::Ready, count: videos.len(), rows, copied, log_found }
    }

    /// The YouTube videos of the rows in view, whose pictures to fetch.
    pub fn pictures_wanted(&self, scroll: f32) -> Vec<String> {
        self.videos.as_ref().map_or_else(Vec::new, |videos| shown(scroll, videos.len()).filter_map(|i| videos[i].youtube()).collect())
    }

    /// The video at `index`: its link, its title (or its link), YouTube's id. `shown`: the link that row showed when it was
    /// pressed; the history read again since may have another video there, and then that one is looked for by its link.
    pub fn video(&self, index: usize, shown: Option<&str>) -> Option<(String, String, Option<String>)> {
        let videos = self.videos.as_ref()?;
        let v = match (videos.get(index), shown) {
            (Some(v), Some(url)) if v.url != url => videos.iter().find(|v| v.url == url)?,
            (Some(v), _) => v,
            (None, Some(url)) => videos.iter().find(|v| v.url == url)?,
            (None, None) => return None,
        };
        Some((v.url.clone(), v.title.clone().unwrap_or_else(|| short_link(&v.url)), v.youtube()))
    }
}

/// Made-up videos for tests and the screenshot review: `n` of them a minute apart, the latest "Sample video {n-1}", some
/// played more than once, every fifth with no title (a stream's link), every seventh in a long title.
pub fn sample(n: usize, latest: u64) -> History {
    let mut h = History::default();
    h.videos = (0..n)
        .map(|i| Played {
            url: if i % 5 == 4 { format!("https://stream.example.invalid/live/sample-{i}.m3u8") } else { format!("https://youtu.be/sample{i:05}") },
            title: match i {
                _ if i % 5 == 4 => None,
                _ if i % 7 == 3 => Some(format!("Sample video {i}: a very long title that a world's player shows whole, with a subtitle and the names of everyone in it")), // i18n: not shown (made-up videos)
                _ => Some(format!("Sample video {i}")), // i18n: not shown (made-up videos)
            },
            channel: (i % 5 != 4).then(|| ["Quiet Hours", "Night Radio", "Spatial Notes"][i % 3].to_string()), // i18n: not shown (made-up videos)
            world: Some(["Sample Lounge", "Midnight Rooftop", "Quiet Café"][i % 3].to_string()), // i18n: not shown (made-up videos)
            first: latest - 60 * (n - 1 - i) as u64,
            last: latest - 60 * (n - 1 - i) as u64,
            plays: 1 + (i % 4 == 0) as u32 * 2,
            ..Played::default()
        })
        .collect();
    h
}

/// The videos, the latest played first (the window's tab's order).
fn latest_first(history: History) -> Vec<Played> {
    let mut videos = history.videos;
    videos.sort_by_key(|v| std::cmp::Reverse(v.last));
    videos
}

/// A link without its scheme and `www.`, as a title (the window's tab says it so).
fn short_link(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.strip_prefix("www.").unwrap_or(rest).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn played(url: &str, title: Option<&str>, last: u64, plays: u32) -> Played {
        serde_json::from_value(serde_json::json!({ "url": url, "title": title, "channel": "Sample Channel", "world": "Sample World", "first": last, "last": last, "plays": plays })).unwrap()
    }

    pub(crate) fn history(n: usize) -> History {
        let mut h = History::default();
        h.videos = (0..n).map(|i| played(&format!("https://youtu.be/sample{i:05}"), Some(&format!("Sample video {i}")), 1_700_000_000 + i as u64 * 60, 1 + (i % 3) as u32)).collect();
        h
    }

    /// REQUEST19 2: the page lists the store's videos the latest first, lays only the rows in view (of thousands), says a
    /// link without a title by its link, and scrolls to its last row.
    #[test]
    fn the_history_page_lists_the_latest_first_and_lays_only_what_shows() {
        let mut store = Store::default();
        let mut h = history(2000);
        h.videos.push(played("https://stream.example.invalid/live/sample.ts", None, 1_800_000_000, 1));
        store.stand_in(h);
        let facts = Facts { sheet: Some(Sheet::VideoHistory), history: store.facts(0.0, None, true, 1_800_000_100, |_| None), ..Facts::default() };
        assert_eq!(facts.history.count, 2001);
        assert!(facts.history.rows.len() < 10, "only the rows in view: {}", facts.history.rows.len());
        let first = &facts.history.rows[0];
        assert_eq!((first.index, first.title.as_str(), first.link_only, first.youtube), (0, "stream.example.invalid/live/sample.ts", true, false), "the latest first, its link for a title");
        assert_eq!(facts.history.rows[1].title, "Sample video 1999");
        let laid = items(&facts);
        assert!(laid.iter().any(|i| i.action == Action::Back), "the page's own way back");
        assert!(laid.iter().filter(|i| matches!(i.look, Look::HistoryRow(_))).all(|i| i.rect.top() < 576.0 && i.rect.bottom() > TOP), "rows in view only");
        let end = max_scroll(&facts);
        let at_end = Facts { gallery_scroll: end, history: store.facts(end, None, true, 1_800_000_100, |_| None), ..facts.clone() };
        let last = items(&at_end).into_iter().filter(|i| matches!(i.look, Look::HistoryRow(_))).map(|i| (i.look, i.rect)).last().unwrap();
        assert_eq!(last.0, Look::HistoryRow(2000), "scrolled to its end, the oldest is the last row");
        assert!(last.1.bottom() <= 576.0, "and wholly on the page: {:?}", last.1);
        assert_eq!(store.video(1, None), Some(("https://youtu.be/sample01999".into(), "Sample video 1999".into(), Some("sample01999".into()))));
        // Pressed on a row that showed another video than the one now there (the file read again in between): that video.
        assert_eq!(store.video(1, Some("https://youtu.be/sample00007")).map(|v| v.0), Some("https://youtu.be/sample00007".into()));
        assert_eq!(store.video(1, Some("https://gone.example.invalid/")), None, "gone since: nothing");
    }

    /// Empty, not read yet, or never read from a file: what the page says.
    #[test]
    fn an_empty_history_says_why() {
        let store = Store::default();
        assert_eq!(store.facts(0.0, None, true, 0, |_| None).state, HistoryState::Reading, "nothing read yet");
        let mut store = Store::default();
        store.stand_in(History::default());
        let facts = store.facts(0.0, None, false, 0, |_| None);
        assert_eq!((facts.state, facts.count, facts.log_found), (HistoryState::Ready, 0, false));
        let facts = Facts { sheet: Some(Sheet::VideoHistory), history: facts, ..Facts::default() };
        assert_eq!(max_scroll(&facts), 0.0);
        assert_eq!(items(&facts).len(), 1, "only the way back");
    }

    /// A store with no file reads nothing (tests never touch the person's folders); one with a file reads it on a thread.
    #[test]
    fn the_store_reads_its_file_and_again_once_it_changed() {
        let dir = std::env::temp_dir().join(format!("orbiters-r19-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("video-history.json");
        std::fs::write(&file, serde_json::to_string(&history(3)).unwrap()).unwrap();
        let mut store = Store::at(Some(file.clone()));
        let start = Instant::now();
        let mut now = start;
        while store.videos().is_none() && now.duration_since(start) < Duration::from_secs(10) {
            store.follow(now);
            std::thread::sleep(Duration::from_millis(10));
            now = Instant::now();
        }
        assert_eq!(store.videos().map(|v| v.len()), Some(3));
        assert_eq!(store.videos().unwrap()[0].title.as_deref(), Some("Sample video 2"), "the latest first");
        let mut none = Store::default();
        assert!(!none.follow(Instant::now()) && none.videos().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
