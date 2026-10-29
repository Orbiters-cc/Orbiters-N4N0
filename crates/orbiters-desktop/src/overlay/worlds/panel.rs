//! The 3D worlds panel, drawn on N4N0's menu overlay in its place (same size and look as the menu's pages): a search
//! field over the captures as cards (thumbnail, world, author), the panel's own keyboard while typing, and a capture's
//! page once chosen: the card grows into it, then it says how the download goes and holds the capture's controls
//! (Hide, Recenter, See VRChat, how sharp). Pressing a card shows that capture around the person.

use super::catalog::Entry;
use super::view::Quality;
use egui::epaint::RectShape;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Painter, Pos2, Rect, Stroke, StrokeKind, TextureId};
use orbiters_orb::menu::KeyboardLayout;
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, widgets};

pub const WIDTH: f32 = 1024.0;
pub const HEIGHT: f32 = 576.0;
/// Cards on a page of results (three across, two down), and on the typing page's single row.
pub const PER_PAGE: usize = 6;
pub const PER_ROW: usize = 3;
/// Characters the search holds.
pub const QUERY_MAX: usize = 40;

const PAGE_FADE: f64 = 0.22;
const MORPH: f64 = 0.42;
const CARD_STAGGER: f64 = 0.035;

const CARD: Color32 = theme::INK_900;
const SURFACE: Color32 = theme::INK_800;
const RAISED: Color32 = theme::INK_750;
const RAISED2: Color32 = theme::INK_700;

/// What the panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Browse,
    /// The keyboard under one row of results.
    Typing,
    /// A capture's page.
    Detail,
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

impl Panel {
    /// The results for the search typed so far.
    pub fn results<'a>(&self, entries: &'a [Entry]) -> Vec<&'a Entry> {
        entries.iter().filter(|e| e.matches(&self.query)).collect()
    }

    fn pages(&self, n: usize) -> usize {
        n.div_ceil(PER_PAGE).max(1)
    }

    fn go(&mut self, page: Page, time: f64) {
        if page != self.page {
            self.page = page;
            self.page_at = time;
        }
        self.dirty = true;
    }

    /// Opens on the results (the capture's page when one shows).
    pub fn open(&mut self, status: &Status, time: f64) {
        self.hover = None;
        self.pressed = None;
        self.morph = None;
        self.page_at = time;
        match status.current {
            Some(id) => {
                self.selected = Some(id);
                self.page = Page::Detail;
            }
            None => self.page = Page::Browse,
        }
        self.dirty = true;
    }

    /// Whether something moves (a page fading in, a card growing, a download).
    pub fn animating(&self, status: &Status, time: f64) -> bool {
        let since = time - self.page_at;
        since < PAGE_FADE + CARD_STAGGER * PER_PAGE as f64 + 0.1
            || self.morph.is_some_and(|m| time - m.at < MORPH + 0.3)
            || status.stage.is_some()
            || status.fetching
            || self.page == Page::Typing
    }

    /// Where each pressable thing is now.
    pub fn targets(&self, entries: &[Entry], status: &Status) -> Vec<(Target, Rect)> {
        let mut out = vec![(Target::Back, Rect::from_center_size(pos2(60.0, 52.0), vec2(56.0, 56.0)))];
        let results = self.results(entries);
        match self.page {
            Page::Browse => {
                out.push((Target::Field, field_rect()));
                let pages = self.pages(results.len());
                if pages > 1 {
                    out.push((Target::Previous, Rect::from_center_size(pos2(900.0, 52.0), vec2(56.0, 56.0))));
                    out.push((Target::Next, Rect::from_center_size(pos2(964.0, 52.0), vec2(56.0, 56.0))));
                }
                let first = self.results_page.min(pages - 1) * PER_PAGE;
                for (i, _) in results.iter().enumerate().skip(first).take(PER_PAGE) {
                    out.push((Target::Card(i), card_rect(i - first)));
                }
                if results.is_empty() && status.listing_error.is_some() {
                    out.push((Target::Retry, Rect::from_center_size(pos2(512.0, 380.0), vec2(200.0, 56.0))));
                }
            }
            Page::Typing => {
                out.push((Target::Field, field_rect()));
                out.push((Target::Done, Rect::from_min_size(pos2(824.0, 24.0), vec2(168.0, 56.0))));
                for (i, _) in results.iter().enumerate().take(PER_ROW) {
                    out.push((Target::Card(i), compact_rect(i)));
                }
                out.extend(keys(self.layout));
            }
            Page::Detail => {
                let current = self.selected.is_some() && status.current == self.selected;
                if current && (status.shown || status.stage.is_some()) {
                    out.push((Target::Hide, Rect::from_min_size(pos2(608.0, 316.0), vec2(384.0, 56.0))));
                    if status.shown {
                        out.push((Target::Recenter, Rect::from_min_size(pos2(608.0, 384.0), vec2(188.0, 56.0))));
                        out.push((Target::SeeThrough, Rect::from_min_size(pos2(804.0, 384.0), vec2(188.0, 56.0))));
                        for (i, q) in Quality::ALL.into_iter().enumerate() {
                            out.push((Target::Quality(q), Rect::from_min_size(pos2(612.0 + i as f32 * 126.0, 456.0), vec2(124.0, 48.0))));
                        }
                    }
                } else {
                    out.push((Target::Show, Rect::from_min_size(pos2(608.0, 316.0), vec2(384.0, 64.0))));
                }
            }
        }
        out
    }

    pub fn hit(&self, entries: &[Entry], status: &Status, x: f32, y: f32) -> Option<Target> {
        self.targets(entries, status).into_iter().rev().find(|(_, r)| r.expand(4.0).contains(pos2(x, y))).map(|(t, _)| t)
    }

    /// The person pressed `target`.
    pub fn press(&mut self, target: Target, entries: &[Entry], status: &Status, time: f64) -> Vec<Action> {
        self.dirty = true;
        let results = self.results(entries);
        match target {
            Target::Back => match self.page {
                Page::Detail => {
                    self.go(if self.query.is_empty() { Page::Browse } else { Page::Typing }, time);
                    self.morph = None;
                    Vec::new()
                }
                Page::Typing => {
                    self.go(Page::Browse, time);
                    Vec::new()
                }
                Page::Browse => vec![Action::Leave],
            },
            Target::Field => {
                self.go(Page::Typing, time);
                Vec::new()
            }
            Target::Done => {
                self.results_page = 0;
                self.go(Page::Browse, time);
                Vec::new()
            }
            Target::Key(c) => {
                self.type_text(&c.to_string());
                Vec::new()
            }
            Target::Space => {
                self.type_text(" ");
                Vec::new()
            }
            Target::Backspace => {
                self.query.pop();
                self.results_page = 0;
                Vec::new()
            }
            Target::Previous => {
                let pages = self.pages(results.len());
                self.results_page = (self.results_page + pages - 1) % pages;
                self.page_at = time;
                Vec::new()
            }
            Target::Next => {
                self.results_page = (self.results_page + 1) % self.pages(results.len());
                self.page_at = time;
                Vec::new()
            }
            Target::Card(i) => {
                let Some(entry) = results.get(i) else { return Vec::new() };
                let from = match self.page {
                    Page::Typing => compact_thumb(compact_rect(i)),
                    _ => card_thumb(card_rect(i % PER_PAGE)),
                };
                self.morph = Some(Morph { from, at: time });
                self.selected = Some(entry.id);
                self.go(Page::Detail, time);
                if status.current == Some(entry.id) && (status.shown || status.stage.is_some()) {
                    Vec::new()
                } else {
                    vec![Action::Show(entry.id)]
                }
            }
            Target::Show => self.selected.map(Action::Show).into_iter().collect(),
            Target::Hide => vec![Action::Hide],
            Target::Recenter => vec![Action::Recenter],
            Target::SeeThrough => vec![Action::SeeThrough(!status.see_through)],
            Target::Quality(q) => vec![Action::Quality(q)],
            Target::Retry => vec![Action::Refresh],
        }
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars().filter(|c| !c.is_control()) {
            if self.query.chars().count() >= QUERY_MAX {
                break;
            }
            self.query.push(c);
        }
        self.results_page = 0;
        self.dirty = true;
    }

    /// A key of the computer's keyboard while the panel types (the service takes them from VRChat).
    pub fn key(&mut self, key: &orbiters_orb::menu::KeyInput, time: f64) -> Vec<Action> {
        use orbiters_orb::menu::KeyInput;
        if self.page != Page::Typing {
            return Vec::new();
        }
        match key {
            KeyInput::Text(t) => self.type_text(t),
            KeyInput::Backspace => {
                self.query.pop();
                self.dirty = true;
            }
            KeyInput::Enter => {
                self.results_page = 0;
                self.go(Page::Browse, time);
            }
            KeyInput::Escape => self.go(Page::Browse, time),
            KeyInput::Paste => {}
        }
        Vec::new()
    }

    /// Whether the computer's keyboard should type here.
    pub fn typing(&self) -> bool {
        self.page == Page::Typing
    }

    /// Draws the panel. `thumb` gives a capture's thumbnail once loaded.
    pub fn draw(&self, p: &Painter, entries: &[Entry], status: &Status, thumb: &dyn Fn(&Entry) -> Option<TextureId>, time: f64) {
        let page = Rect::from_min_size(Pos2::ZERO, vec2(WIDTH, HEIGHT));
        p.rect_filled(page, CornerRadius::same(28), CARD);
        let shown = ease_out((time - self.page_at) / PAGE_FADE);
        let targets = self.targets(entries, status);
        let state = |t: Target| (self.hover == Some(t), self.pressed == Some(t));
        // The back button, on every page.
        let (hover, pressed) = state(Target::Back);
        round_button(p, pos2(60.0, 52.0), icons::ARROW_LEFT, hover, pressed, 1.0);
        let results = self.results(entries);
        match self.page {
            Page::Browse => {
                field(p, &self.query, false, state(Target::Field), time, 1.0);
                let pages = self.pages(results.len());
                let current = self.results_page.min(pages - 1);
                if pages > 1 {
                    let (h, pr) = state(Target::Previous);
                    round_button(p, pos2(900.0, 52.0), icons::CARET_LEFT, h, pr, 1.0);
                    let (h, pr) = state(Target::Next);
                    round_button(p, pos2(964.0, 52.0), icons::CARET_RIGHT, h, pr, 1.0);
                    text(p, pos2(860.0, 52.0), Align2::RIGHT_CENTER, Txt::new(format!("{}/{}", current + 1, pages), 17.0).color(theme::FG_3));
                } else {
                    let label = match results.len() {
                        1 => "1 world".to_string(),
                        n => format!("{n} worlds"),
                    };
                    text(p, pos2(984.0, 52.0), Align2::RIGHT_CENTER, Txt::new(label, 17.0).color(theme::FG_3));
                }
                for (t, rect) in &targets {
                    if let Target::Card(i) = *t {
                        let slot = i - current * PER_PAGE;
                        let appear = ease_out((time - self.page_at - slot as f64 * CARD_STAGGER) / PAGE_FADE);
                        let lift = (1.0 - appear) * 18.0;
                        let (h, pr) = state(*t);
                        card(p, rect.translate(vec2(0.0, lift)), results[i], status, thumb(results[i]), h, pr, appear);
                    }
                }
                if results.is_empty() {
                    self.empty(p, entries, status, shown, state(Target::Retry));
                }
            }
            Page::Typing => {
                field(p, &self.query, true, state(Target::Field), time, 1.0);
                let (h, pr) = state(Target::Done);
                pill_button(p, Rect::from_min_size(pos2(824.0, 24.0), vec2(168.0, 56.0)), Some(icons::MAGNIFYING_GLASS), "Done", theme::ACCENT, theme::ON_ACCENT, h, pr, 1.0);
                for (t, rect) in &targets {
                    if let Target::Card(i) = *t {
                        let (h, pr) = state(*t);
                        compact_card(p, *rect, results[i], status, thumb(results[i]), h, pr, shown);
                    }
                }
                if results.is_empty() {
                    let msg = if entries.is_empty() { "Loading the 3D worlds…" } else { "No 3D world matches" };
                    text(p, pos2(512.0, 162.0), Align2::CENTER_CENTER, Txt::new(msg, 19.0).color(fade(theme::FG_3, shown)));
                }
                for (t, rect) in &targets {
                    let (h, pr) = state(*t);
                    match *t {
                        Target::Key(c) => key_cap(p, *rect, &c.to_string(), None, RAISED, theme::FG, h, pr, shown),
                        Target::Space => key_cap(p, *rect, "space", None, RAISED, theme::FG, h, pr, shown),
                        Target::Backspace => key_cap(p, *rect, "", Some(icons::BACKSPACE), RAISED2, theme::FG_2, h, pr, shown),
                        _ => {}
                    }
                }
            }
            Page::Detail => self.detail(p, entries, status, thumb, time, &targets),
        }
    }

    fn empty(&self, p: &Painter, entries: &[Entry], status: &Status, shown: f32, retry: (bool, bool)) {
        let (icon, title, line) = if let Some(e) = status.listing_error.as_ref().filter(|_| entries.is_empty()) {
            (icons::WARNING, "Orbiters could not be reached", e.as_str())
        } else if entries.is_empty() {
            (icons::GLOBE, "Finding 3D worlds…", "The worlds members captured and shared")
        } else {
            (icons::MAGNIFYING_GLASS, "No 3D world matches", "Try a world's name, its author or who shared it")
        };
        widgets::icon(p, pos2(512.0, 230.0), icon, 44.0, fade(theme::FG_4, shown));
        text(p, pos2(512.0, 286.0), Align2::CENTER_CENTER, Txt::new(title, 24.0).weight(weight::SEMIBOLD).color(fade(theme::FG, shown)));
        let line: String = line.chars().take(80).collect();
        text(p, pos2(512.0, 320.0), Align2::CENTER_CENTER, Txt::new(line, 17.0).color(fade(theme::FG_3, shown)));
        if status.listing_error.is_some() && entries.is_empty() {
            pill_button(p, Rect::from_center_size(pos2(512.0, 380.0), vec2(200.0, 56.0)), Some(icons::ARROW_CLOCKWISE), "Try again", RAISED, theme::FG, retry.0, retry.1, shown);
        }
    }

    fn detail(&self, p: &Painter, entries: &[Entry], status: &Status, thumb: &dyn Fn(&Entry) -> Option<TextureId>, time: f64, targets: &[(Target, Rect)]) {
        let Some(entry) = self.selected.and_then(|id| entries.iter().find(|e| e.id == id)) else {
            text(p, pos2(512.0, 288.0), Align2::CENTER_CENTER, Txt::new("This 3D world is no longer listed", 22.0).color(theme::FG_3));
            return;
        };
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
        picture(p, hero, thumb(entry), &entry.world_id, CornerRadius::same(radius as u8), 1.0);
        // The picture's foot: what happens to the capture.
        let current = status.current == Some(entry.id);
        if current {
            let foot = Rect::from_min_max(pos2(hero.min.x, hero.max.y - 64.0), hero.max);
            p.rect_filled(foot, CornerRadius { nw: 0, ne: 0, sw: radius as u8, se: radius as u8 }, fade(theme::black(0.62), rest));
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
        }
        // The world's name on the header row, beside Back; the rest in the column beside the picture.
        let title = Txt::new(entry.name.clone(), 28.0).weight(weight::SEMIBOLD).color(fade(theme::FG, rest));
        let galley = p.layout_job(title.job_truncated(860.0));
        p.galley(pos2(108.0, 52.0 - galley.size().y / 2.0 - slide / 2.0), galley, theme::FG);
        let x = 608.0;
        let mut y = 100.0 + slide;
        if let Some(author) = &entry.author {
            text_cut(p, pos2(x, y), 384.0, Txt::new(format!("by {author}"), 19.0).weight(weight::MEDIUM).color(fade(theme::FG_2, rest)));
            y += 32.0;
        }
        let mut meta = vec![format!("{} splats", short_count(entry.splats as u64))];
        if entry.sky {
            meta.push("sky".into());
        }
        if let Some(u) = &entry.uploader {
            meta.push(format!("shared by {u}"));
        }
        text_cut(p, pos2(x, y), 384.0, Txt::new(meta.join(" · "), 16.0).color(fade(theme::FG_3, rest)));
        y += 34.0;
        if entry.official {
            chip(p, pos2(x, y), icons::SEAL_CHECK, "Shared by its author", theme::ACCENT, rest);
            y += 42.0;
        }
        // As much of the world's description as fits above the buttons.
        let room = 300.0 + slide - y;
        if !entry.description.trim().is_empty() && room > 24.0 {
            let lines = (room / 23.0).floor().max(1.0) as usize;
            let mut job = Txt::new(entry.description.split_whitespace().collect::<Vec<_>>().join(" "), 16.0).color(fade(theme::FG_3, rest)).line_height(23.0).job(384.0);
            job.wrap.max_rows = lines;
            p.galley(pos2(x, y), p.layout_job(job), theme::FG_3);
        }
        for (t, rect) in targets {
            let (h, pr) = state(*t);
            let rect = rect.translate(vec2(0.0, slide));
            match *t {
                Target::Show => pill_button(p, rect, Some(icons::CUBE), "Show around me", theme::ACCENT, theme::ON_ACCENT, h, pr, rest),
                Target::Hide => {
                    let label = if status.shown { "Hide" } else { "Cancel" };
                    pill_button(p, rect, Some(icons::EYE_SLASH), label, theme::mix(RAISED, theme::DANGER, 0.22), theme::FG, h, pr, rest)
                }
                Target::Recenter => pill_button(p, rect, Some(icons::CROSSHAIR), "Recenter", RAISED, theme::FG, h, pr, rest),
                Target::SeeThrough => {
                    let (bg, fg) = if status.see_through { (theme::ACCENT, theme::ON_ACCENT) } else { (RAISED, theme::FG) };
                    pill_button(p, rect, Some(icons::VIRTUAL_REALITY), "See VRChat", bg, fg, h, pr, rest)
                }
                Target::Quality(q) => {
                    if q == Quality::Performance {
                        let track = Rect::from_min_size(pos2(608.0, 452.0 + slide), vec2(384.0, 56.0));
                        p.rect_filled(track, CornerRadius::same(28), fade(SURFACE, rest));
                    }
                    let on = status.quality == q;
                    let bg = if on { theme::FG } else if h { RAISED } else { SURFACE };
                    let fg = if on { CARD } else { theme::FG_2 };
                    let r = if pr { rect.shrink(2.0) } else { rect };
                    p.rect_filled(r, CornerRadius::same(24), fade(bg, rest));
                    text(p, r.center(), Align2::CENTER_CENTER, Txt::new(q.label(), 17.0).weight(weight::MEDIUM).color(fade(fg, rest)));
                }
                _ => {}
            }
        }
        if let Some(stats) = &status.stats {
            if current && status.shown {
                text(p, pos2(992.0, 540.0), Align2::RIGHT_CENTER, Txt::new(stats.clone(), 14.0).color(fade(theme::FG_4, rest)));
            }
        }
        if !current {
            text(p, pos2(x + 192.0, 404.0 + slide), Align2::CENTER_TOP, Txt::new("Its start view where you stand", 15.0).color(fade(theme::FG_4, rest)));
        }
    }
}

fn field_rect() -> Rect {
    Rect::from_min_max(pos2(100.0, 24.0), pos2(808.0, 80.0))
}

fn card_rect(slot: usize) -> Rect {
    let (col, row) = ((slot % PER_ROW) as f32, (slot / PER_ROW) as f32);
    Rect::from_min_size(pos2(32.0 + col * 328.0, 100.0 + row * 228.0), vec2(304.0, 212.0))
}

fn card_thumb(card: Rect) -> Rect {
    Rect::from_min_size(card.min, vec2(card.width(), 150.0))
}

fn compact_rect(i: usize) -> Rect {
    Rect::from_min_size(pos2(32.0 + i as f32 * 328.0, 100.0), vec2(304.0, 120.0))
}

fn compact_thumb(card: Rect) -> Rect {
    Rect::from_min_size(card.min + vec2(10.0, 10.0), vec2(112.0, 100.0))
}

/// The keyboard's keys: digits, three rows of letters (Backspace at the end of the last), then the space bar.
fn keys(layout: KeyboardLayout) -> Vec<(Target, Rect)> {
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
    let galley = p.layout_job(label_txt.job(f32::INFINITY));
    let icon_w = if glyph.is_some() { 30.0 } else { 0.0 };
    let total = icon_w + galley.size().x;
    let left = rect.center().x - total / 2.0;
    if let Some(g) = glyph {
        widgets::icon(p, pos2(left + 11.0, rect.center().y), g, 22.0, fade(fg, alpha));
    }
    p.galley(pos2(left + icon_w, rect.center().y - galley.size().y / 2.0), galley, fade(fg, alpha));
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

fn field(p: &Painter, query: &str, active: bool, (hover, _): (bool, bool), time: f64, alpha: f32) {
    let rect = field_rect();
    p.rect_filled(rect, CornerRadius::same(28), fade(if hover && !active { RAISED } else { SURFACE }, alpha));
    if active {
        p.rect_stroke(rect, CornerRadius::same(28), Stroke::new(2.0, fade(theme::ACCENT, alpha)), StrokeKind::Inside);
    }
    widgets::icon(p, pos2(rect.min.x + 34.0, rect.center().y), icons::MAGNIFYING_GLASS, 22.0, fade(theme::FG_3, alpha));
    let at = pos2(rect.min.x + 62.0, rect.center().y);
    let shown = if query.is_empty() && !active { Txt::new("Search 3D worlds", 20.0).color(fade(theme::FG_4, alpha)) } else { Txt::new(query, 20.0).color(fade(theme::FG, alpha)) };
    let galley = p.layout_job(shown.job(f32::INFINITY));
    let width = galley.size().x;
    let height = galley.size().y;
    p.galley(pos2(at.x, at.y - height / 2.0), galley, theme::FG);
    if active && (time * 2.0).fract() < 0.6 {
        let x = at.x + if query.is_empty() { 0.0 } else { width + 2.0 };
        p.line_segment([pos2(x, rect.center().y - 13.0), pos2(x, rect.center().y + 13.0)], Stroke::new(2.0, fade(theme::ACCENT, alpha)));
    }
}

/// A capture's picture: its world's thumbnail, or a placeholder in the world's own colour.
fn picture(p: &Painter, rect: Rect, texture: Option<TextureId>, world_id: &str, corner: CornerRadius, alpha: f32) {
    match texture {
        Some(id) => {
            // Cover the rect with the 16:9 thumbnail (cropped, never stretched).
            let aspect = rect.width() / rect.height().max(1.0);
            let uv = if aspect > 16.0 / 9.0 {
                let v = (16.0 / 9.0) / aspect;
                Rect::from_min_max(pos2(0.0, 0.5 - v / 2.0), pos2(1.0, 0.5 + v / 2.0))
            } else {
                let u = aspect / (16.0 / 9.0);
                Rect::from_min_max(pos2(0.5 - u / 2.0, 0.0), pos2(0.5 + u / 2.0, 1.0))
            };
            p.add(RectShape::filled(rect, corner, fade(Color32::WHITE, alpha)).with_texture(id, uv));
        }
        None => {
            let hue = world_id.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360;
            let base = hsv(hue as f32 / 360.0, 0.45, 0.42);
            p.rect_filled(rect, corner, fade(base, alpha));
            widgets::icon(p, rect.center(), icons::CUBE, (rect.height() * 0.3).clamp(18.0, 64.0), fade(theme::white(0.55), alpha));
        }
    }
}

fn hsv(h: f32, s: f32, v: f32) -> Color32 {
    let rgb = egui::ecolor::Hsva::new(h, s, v, 1.0).to_srgb();
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

fn chip(p: &Painter, at: Pos2, glyph: &str, label: &str, tint: Color32, alpha: f32) {
    let txt = Txt::new(label, 15.0).weight(weight::MEDIUM).color(fade(tint, alpha));
    let galley = p.layout_job(txt.job(f32::INFINITY));
    let rect = Rect::from_min_size(at, vec2(galley.size().x + 42.0, 30.0));
    p.rect_filled(rect, CornerRadius::same(15), fade(theme::alpha(tint, 0.14), alpha));
    widgets::icon(p, pos2(rect.min.x + 17.0, rect.center().y), glyph, 17.0, fade(tint, alpha));
    p.galley(pos2(rect.min.x + 31.0, rect.center().y - galley.size().y / 2.0), galley, tint);
}

/// 786 432 → "786k", 1 026 271 → "1.03M".
pub fn short_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{}k", n / 1000),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

#[allow(clippy::too_many_arguments)]
fn card(p: &Painter, rect: Rect, entry: &Entry, status: &Status, texture: Option<TextureId>, hover: bool, pressed: bool, alpha: f32) {
    let rect = if pressed {
        rect.shrink(3.0)
    } else if hover {
        rect.expand(2.0)
    } else {
        rect
    };
    p.rect_filled(rect, CornerRadius::same(20), fade(if hover { RAISED } else { SURFACE }, alpha));
    let thumb = Rect::from_min_size(rect.min, vec2(rect.width(), 150.0));
    let top_only = CornerRadius { nw: 20, ne: 20, sw: 0, se: 0 };
    picture(p, thumb, texture, &entry.world_id, top_only, alpha);
    if status.current == Some(entry.id) {
        let label = if status.shown { "Around you" } else { "Loading" };
        let tint = if status.shown { theme::ACCENT } else { theme::WARN };
        let txt = Txt::new(label, 14.0).weight(weight::SEMIBOLD).color(fade(theme::ON_ACCENT, alpha));
        let galley = p.layout_job(txt.job(f32::INFINITY));
        let chip = Rect::from_min_size(thumb.min + vec2(12.0, 12.0), vec2(galley.size().x + 22.0, 26.0));
        p.rect_filled(chip, CornerRadius::same(13), fade(tint, alpha));
        p.galley(pos2(chip.min.x + 11.0, chip.center().y - galley.size().y / 2.0), galley, theme::ON_ACCENT);
    }
    if entry.official {
        widgets::icon(p, pos2(thumb.max.x - 22.0, thumb.min.y + 24.0), icons::SEAL_CHECK, 22.0, fade(theme::ACCENT, alpha));
    }
    text_cut(p, pos2(rect.min.x + 16.0, rect.min.y + 160.0), rect.width() - 32.0, Txt::new(entry.name.clone(), 19.0).weight(weight::SEMIBOLD).color(fade(theme::FG, alpha)));
    let meta = match &entry.author {
        Some(a) => format!("{a} · {} splats", short_count(entry.splats as u64)),
        None => format!("{} splats", short_count(entry.splats as u64)),
    };
    text_cut(p, pos2(rect.min.x + 16.0, rect.min.y + 186.0), rect.width() - 32.0, Txt::new(meta, 15.0).color(fade(theme::FG_3, alpha)));
}

#[allow(clippy::too_many_arguments)]
fn compact_card(p: &Painter, rect: Rect, entry: &Entry, status: &Status, texture: Option<TextureId>, hover: bool, pressed: bool, alpha: f32) {
    let rect = if pressed { rect.shrink(3.0) } else { rect };
    p.rect_filled(rect, CornerRadius::same(18), fade(if hover { RAISED } else { SURFACE }, alpha));
    let thumb = compact_thumb(rect);
    picture(p, thumb, texture, &entry.world_id, CornerRadius::same(10), alpha);
    let x = thumb.max.x + 14.0;
    let width = rect.max.x - x - 12.0;
    let mut name = Txt::new(entry.name.clone(), 17.0).weight(weight::SEMIBOLD).color(fade(theme::FG, alpha)).job(width);
    name.wrap.max_rows = 2;
    p.galley(pos2(x, rect.min.y + 14.0), p.layout_job(name), theme::FG);
    if let Some(a) = &entry.author {
        text_cut(p, pos2(x, rect.min.y + 62.0), width, Txt::new(a.clone(), 15.0).color(fade(theme::FG_3, alpha)));
    }
    if status.current == Some(entry.id) {
        p.circle_filled(pos2(x + 6.0, rect.min.y + 88.0), 5.0, fade(if status.shown { theme::ACCENT } else { theme::WARN }, alpha));
        text(p, pos2(x + 18.0, rect.min.y + 88.0), Align2::LEFT_CENTER, Txt::new(if status.shown { "Around you" } else { "Loading" }, 14.0).color(fade(theme::FG_2, alpha)));
    }
}

/// The small pill under N4N0's menu that opens the panel (or, while a capture shows, says which), `size` pixels.
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
        // A turning ring while it loads.
        let a = (time * 5.0) as f32;
        let c = pos2(icon_x, rect.center().y);
        let points: Vec<Pos2> = (0..=16).map(|i| a + i as f32 * 0.28).map(|t| c + vec2(t.cos(), t.sin()) * 15.0).collect();
        p.add(egui::Shape::line(points, Stroke::new(3.0, theme::WARN)));
    } else {
        widgets::icon(p, pos2(icon_x, rect.center().y), glyph, 26.0, tint);
    }
    text_cut(p, pos2(rect.min.x + 64.0, rect.center().y - 13.0), rect.width() - 100.0, Txt::new(label, 21.0).weight(weight::SEMIBOLD).color(theme::FG));
    widgets::icon(p, pos2(rect.max.x - 30.0, rect.center().y), icons::CARET_RIGHT, 20.0, theme::FG_3);
}

/// The panel's pages with stand-in captures, for the offline review (`--world-view`).
pub fn shots(out: &std::path::Path) -> anyhow::Result<()> {
    use super::super::Canvas;
    let names = [
        ("VRChat Home", "vrchat", 1_026_271u32, false),
        ("VRC NYE 2026 - Normal Capacity", "VRChat", 1_034_068, true),
        ("Autumn fragrance / 秋の香り", "_Harim", 722_750, false),
        ("Find the Needle", "Roofy.", 487_568, false),
        ("Silent Dawn", "Mei", 471_419, true),
        ("The Black Cat", "spookyghostboo", 1_520_000, false),
        ("Japan Shrine", "Kuroneko", 990_000, false),
    ];
    let json = serde_json::json!({ "previews": names.iter().enumerate().map(|(i, (name, author, splats, sky))| serde_json::json!({
        "slot": i, "key": 100 + i, "id": 10 + i, "worldId": format!("wrld_stand_in_{i}"), "name": name, "author": author, "uploader": "blackorbit",
        "description": if i == 0 { "Your home in VRChat: a quiet place to meet friends, try avatars and find worlds." } else { "" }, "splats": { "pc": splats }, "view": { "position": [0.0, 1.6, 0.0], "target": [0.0, 1.6, -1.0], "floor": null, "eyeHeight": null },
        "sky": sky, "official": i == 1 })).collect::<Vec<_>>() });
    let entries = super::catalog::parse(&json.to_string())?;
    let mut canvas = Canvas::new(WIDTH as u32, HEIGHT as u32);
    // Stand-in thumbnails: soft gradients like the menu's history stand-ins.
    let thumbs: Vec<egui::TextureHandle> = (0..entries.len())
        .map(|i| {
            let hue = (i as f32 * 0.137 + 0.55).fract();
            let (a, b) = (hsv(hue, 0.55, 0.62), hsv((hue + 0.08).fract(), 0.6, 0.22));
            let (w, h) = (320usize, 180usize);
            let mut px = Vec::with_capacity(w * h);
            for y in 0..h {
                for x in 0..w {
                    let t = ((x as f32 / w as f32) * 0.6 + (y as f32 / h as f32) * 0.4).clamp(0.0, 1.0);
                    let horizon = (y as f32 / h as f32 - 0.62).abs() < 0.004;
                    px.push(if horizon { theme::white(0.5) } else { theme::mix(a, b, t) });
                }
            }
            canvas.ctx().load_texture(format!("stand-in-{i}"), egui::ColorImage { size: [w, h], source_size: vec2(w as f32, h as f32), pixels: px }, egui::TextureOptions::LINEAR)
        })
        .collect();
    let thumb = |e: &Entry| -> Option<TextureId> { entries.iter().position(|x| x.id == e.id).filter(|i| *i != 3).map(|i| thumbs[i].id()) };
    let mut shot = |name: &str, panel: &Panel, status: &Status, time: f64| -> anyhow::Result<()> {
        // Twice: textures load on the first frame.
        for _ in 0..2 {
            canvas.paint(time, |p| panel.draw(p, &entries, status, &thumb, time));
        }
        let rgba = straight(canvas.pixels());
        super::shots::write_png(&out.join(format!("panel-{name}.png")), WIDTH as u32, HEIGHT as u32, &rgba)
    };
    let idle = Status::default();
    let mut panel = Panel { dirty: true, ..Default::default() };
    shot("browse", &panel, &idle, 5.0)?;
    panel.hover = Some(Target::Card(1));
    shot("browse-hover", &panel, &idle, 5.0)?;
    panel.hover = None;
    shot("browse-appearing", &panel, &idle, 0.12)?;
    let mut typing = Panel { page: Page::Typing, query: "ho".into(), ..Default::default() };
    typing.pressed = Some(Target::Key('m'));
    shot("typing", &typing, &idle, 5.2)?;
    let none = Panel { page: Page::Typing, query: "zzz".into(), ..Default::default() };
    shot("typing-none", &none, &idle, 5.2)?;
    let mut detail = Panel { page: Page::Detail, selected: Some(10), ..Default::default() };
    let downloading = Status { current: Some(10), download: Some(0.42), stage: Some("Downloading 13.8 of 32.9 MB".into()), ..Default::default() };
    shot("loading", &detail, &downloading, 5.0)?;
    let shown = Status { current: Some(10), shown: true, stats: Some("786k splats · 6.4 ms a batch · 1024 px".into()), ..Default::default() };
    shot("shown", &detail, &shown, 5.0)?;
    detail.hover = Some(Target::Quality(Quality::High));
    let see = Status { see_through: true, quality: Quality::High, ..shown.clone() };
    shot("shown-see-through", &detail, &see, 5.0)?;
    // The card growing into the page, a third of the way and nearly there.
    let mut morph = Panel { page: Page::Browse, ..Default::default() };
    morph.press(Target::Card(1), &entries, &idle, 10.0);
    shot("morph-1", &morph, &downloading, 10.0 + MORPH * 0.33)?;
    shot("morph-2", &morph, &Status { current: Some(11), download: Some(0.1), stage: Some("Downloading 3.3 of 33.1 MB".into()), ..Default::default() }, 10.0 + MORPH * 0.8)?;
    let failed = Status { listing_error: Some("Orbiters could not be reached (timed out)".into()), ..Default::default() };
    shot("offline", &Panel::default(), &failed, 5.0)?;
    let not_current = Panel { page: Page::Detail, selected: Some(12), ..Default::default() };
    shot("detail", &not_current, &shown, 5.0)?;
    // The chip under N4N0's menu.
    let mut chip_canvas = Canvas::new(CHIP_PX[0], CHIP_PX[1]);
    for (name, status, hover) in [("chip", Status::default(), false), ("chip-hover", Status::default(), true), ("chip-shown", shown.clone(), false), ("chip-loading", downloading.clone(), false)] {
        chip_canvas.paint(1.0, |p| draw_chip(p, &status, Some("VRChat Home"), hover, false, 1.0));
        super::shots::write_png(&out.join(format!("panel-{name}.png")), CHIP_PX[0], CHIP_PX[1], &straight(chip_canvas.pixels()))?;
    }
    Ok(())
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
