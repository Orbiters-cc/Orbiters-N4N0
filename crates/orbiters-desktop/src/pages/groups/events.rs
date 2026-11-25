//! The Events tab of a group's page (REQUEST28): its VRChat calendar, read only. What is coming soonest first, then what
//! is over, newest first and a few at a time; each with its picture, when in this computer's time zone, for whom, its
//! category, platforms and how many are interested. Where Orbiters has the same event ([`group_events::candidate`], then
//! [`group_events::is_of`] on Orbiters' own page of it), its card also says what Orbiters adds: its words, the Discord
//! server and links, sign-ups, voting, program, staff and prizes.
//!
//! VRChat is asked through the service like the rest of the page (`Request::Events`): the calendar whole, then this month
//! and the next, then an earlier month at a time as the person asks (VRChat lists by month). Orbiters is asked as the
//! member signed in there ([`Told`]); nothing here is kept on disk.
use super::page::{button_row, line, note, Act};
use super::{failure_text, Kind, State};
use crate::controller::groups::{Failure, Reply, Request};
use crate::controller::vrchat_data::Scope;
use crate::pages::{events as orbiters, AccountFollow, Cx, ForAccount, Job};
use egui::{pos2, vec2, Align2, Color32, Rect, Sense, Ui};
use orbiters_core::account::{Api, Server};
use orbiters_core::community::{Card, Invites, Overview};
use orbiters_core::group_events::{self, CalendarEvent, Month};
use orbiters_core::groups as g;
use orbiters_i18n::{tr, tr_n};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, logo, paint, widgets};
use std::collections::{HashMap, HashSet, VecDeque};

/// Events kept loaded at most.
const EVENTS_MAX: usize = 1000;
/// Past events shown at first, and more at each press.
const PAST_PAGE: usize = 8;
/// Earlier months read for one press at most, and months with nothing in a row before there is nothing earlier.
const MONTHS_A_PRESS: u8 = 6;
const EMPTY_MONTHS: u8 = 12;
/// Pages of Orbiters' list read at most.
const LIST_PAGES: usize = 3;

/// A group's calendar as read so far.
#[derive(Default)]
pub(crate) struct EventList {
    pub items: Vec<CalendarEvent>,
    /// What is still to be read: the calendar whole, or a month of it, and from where on.
    queue: VecDeque<(Option<Month>, usize)>,
    reading: Option<(Option<Month>, usize)>,
    failed: Option<Failure>,
    started: bool,
    /// The next earlier month, once the person asked for earlier events.
    earlier: Option<Month>,
    /// Earlier months in a row with nothing in them, and how many more this press may read.
    empty: u8,
    budget: u8,
    /// Past events shown.
    past: usize,
    heights: HashMap<String, f32>,
    pub told: Told,
}

impl EventList {
    /// The page was read again: so is the calendar, what is shown staying meanwhile.
    pub fn again(&mut self) {
        (self.started, self.failed, self.reading) = (false, None, None);
        self.queue.clear();
        self.told.again();
    }

    fn begin(&mut self, now: i64) {
        let month = Month::of(now);
        self.started = true;
        self.past = self.past.max(PAST_PAGE);
        self.queue = VecDeque::from([(None, 0), (Some(month), 0), (Some(month.after()), 0)]);
    }

    /// The read to make now, if one is due and none is under way.
    fn due(&mut self) -> Option<(Option<Month>, usize)> {
        if self.reading.is_some() || self.failed.is_some() {
            return None;
        }
        self.reading = Some(self.queue.pop_front()?);
        self.reading
    }

    /// The next page or month is asked for at the next frame (`due`), from where this one ended.
    pub fn answer(&mut self, month: Option<Month>, offset: usize, reply: Reply) {
        if self.reading != Some((month, offset)) {
            return;
        }
        self.reading = None;
        let page = match reply {
            Reply::Events(page) => page,
            Reply::Failed(f) => {
                // Asked again from here when the person tries again.
                self.queue.push_front((month, offset));
                self.failed = Some(f);
                return;
            }
            _ => return,
        };
        let mut new = 0;
        for e in page.items {
            match self.items.iter().position(|x| x.id == e.id) {
                Some(i) => self.items[i] = e,
                None if self.items.len() < EVENTS_MAX => {
                    self.items.push(e);
                    new += 1;
                }
                None => {}
            }
        }
        self.items.sort_by(|a, b| (a.start().unwrap_or(i64::MAX), &a.id).cmp(&(b.start().unwrap_or(i64::MAX), &b.id)));
        if page.more && page.raw > 0 && offset + page.raw < EVENTS_MAX {
            // The rest of the same list, before anything else.
            self.queue.push_front((month, offset + page.raw));
            return;
        }
        if month.is_some() && month == self.earlier.map(Month::after) {
            self.empty = if new > 0 { 0 } else { self.empty + 1 };
            self.go_back(orbiters::now());
        }
    }

    fn past_count(&self, now: i64) -> usize {
        self.items.iter().filter(|e| e.past(now)).count()
    }
    /// Nothing earlier was found for a year: there is nothing more to ask.
    fn exhausted(&self) -> bool {
        self.empty >= EMPTY_MONTHS
    }
    /// More past events, asked by the person: those read already, then earlier months.
    fn more_past(&mut self, now: i64) {
        self.past += PAST_PAGE;
        self.budget = MONTHS_A_PRESS;
        self.go_back(now);
    }
    /// The next earlier month, while what is wanted is not there and this press may read one more.
    fn go_back(&mut self, now: i64) {
        if self.past_count(now) >= self.past || self.budget == 0 || self.exhausted() {
            return;
        }
        let month = self.earlier.unwrap_or_else(|| self.items.iter().filter_map(CalendarEvent::start).min().map_or(Month::of(now), |s| Month::of(s.min(now))).before());
        self.budget -= 1;
        self.earlier = Some(month.before());
        self.queue.push_back((Some(month), 0));
    }
    fn busy(&self) -> bool {
        self.reading.is_some() || !self.queue.is_empty()
    }

    /// Made up for the screenshot review and tests: read, nothing under way.
    pub fn stage(&mut self, items: Vec<CalendarEvent>) {
        self.items = items;
        self.items.sort_by_key(|e| e.start().unwrap_or(i64::MAX));
        self.started = true;
        self.past = PAST_PAGE;
        self.queue.clear();
    }
    /// The same, refused.
    pub fn stage_failed(&mut self, f: Failure) {
        (self.started, self.failed) = (true, Some(f));
    }
}

/// What Orbiters says of an event.
pub(crate) enum Says {
    Is(Box<Overview>, Option<Invites>),
    /// Not for this member, gone, or not read.
    Nothing,
}

type Read = anyhow::Result<(Overview, Option<Invites>)>;

/// Orbiters' side: the events it lists and those of the galleries the member sees (which is how it gives past ones), and
/// the page of each one that may be an event of the calendar, one read at a time. All of it for the member signed in to
/// Orbiters: another one starts it over.
#[derive(Default)]
pub(crate) struct Told {
    account: AccountFollow,
    list: Job<ForAccount<anyhow::Result<Vec<Card>>>>,
    asked: bool,
    failed: bool,
    pub cards: Vec<Card>,
    pub pages: HashMap<String, Says>,
    reading: Option<(String, Job<ForAccount<Read>>)>,
}

impl Told {
    fn again(&mut self) {
        if !self.list.running() {
            (self.asked, self.failed) = (false, false);
            self.pages.retain(|_, s| matches!(s, Says::Is(..)));
        }
    }

    /// Takes what came, and asks for the list once. Never in tests, nor while a screenshot stages.
    fn follow(&mut self, ctx: &egui::Context, cx: &Cx, staged: bool) {
        if staged || cfg!(test) {
            return;
        }
        let development = cx.snap.settings.development;
        let seen = self.account.see(&cx.snap.account, development);
        if seen.changed {
            let account = std::mem::take(&mut self.account);
            *self = Self { account, ..Default::default() };
        }
        if let Some(answer) = self.list.take().and_then(|a| a.take_for(self.account.owner())) {
            match answer {
                Ok(cards) => self.cards = cards,
                Err(_) => self.failed = true,
            }
        }
        self.failed |= self.list.died();
        let mut done = None;
        if let Some((id, job)) = self.reading.as_mut() {
            if let Some(answer) = job.take() {
                done = Some((id.clone(), answer.take_for(self.account.owner())));
            } else if job.died() {
                done = Some((id.clone(), None));
            }
        }
        if let Some((id, answer)) = done {
            self.reading = None;
            let says = match answer {
                Some(Ok((page, invites))) => Says::Is(Box::new(page), invites),
                _ => Says::Nothing,
            };
            self.pages.insert(id, says);
        }
        if !self.asked && !self.list.running() {
            self.asked = true;
            let server = server(development);
            let member = crate::member::reading(server);
            self.list = crate::pages::job_for(ctx, &seen.owner, move || {
                let api = Api::new(server);
                let mut cards = Vec::new();
                let mut offset = 0;
                for _ in 0..LIST_PAGES {
                    let page = api.events(offset)?;
                    cards.extend(page.items);
                    match page.next_offset {
                        Some(next) if next > offset => offset = next,
                        _ => break,
                    }
                }
                // Events already over are not listed: those with a gallery the member sees come with it.
                if let Some(galleries) = member.and_then(|m| {
                    let (api, token) = m.api();
                    api.galleries(&token).ok()
                }) {
                    let known: HashSet<String> = cards.iter().map(|c| c.id.clone()).collect();
                    cards.extend(galleries.all().filter_map(|g| g.event.clone()).filter(|c| !known.contains(&c.id)));
                }
                Ok(cards)
            });
        }
    }

    /// What Orbiters says of `event`, asked for when it may be one of its own.
    fn of(&mut self, ctx: &egui::Context, development: bool, event: &CalendarEvent, group: &str, group_name: &str, staged: bool) -> Option<&Says> {
        let id = group_events::candidate(event, group_name, &self.cards)?.id.clone();
        if !self.pages.contains_key(&id) && self.reading.is_none() && !staged && !cfg!(test) {
            if let Some(owner) = self.account.owner().cloned() {
                let server = server(development);
                let (member, asked) = (crate::member::reading(server), id.clone());
                let job = crate::pages::job_for(ctx, &owner, move || match member.map(|m| m.api()) {
                    Some((api, token)) => Ok((api.event(Some(&token), &asked)?, api.my_invites(&token, &asked).ok())),
                    None => Ok((Api::new(server).event(None, &asked)?, None)),
                });
                self.reading = Some((id.clone(), job));
            }
        }
        self.pages.get(&id).filter(|s| matches!(s, Says::Is(page, _) if group_events::is_of(event, group, page)))
    }
}

fn server(development: bool) -> Server {
    if development { Server::Development } else { Server::Production }
}

/// An event's page on Orbiters.
fn orbiters_page(development: bool, id: &str) -> Option<String> {
    (!id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then(|| format!("{}events/{id}", server(development).website()))
}
/// A link of Discord's, and no other.
fn discord(url: Option<&str>) -> Option<String> {
    url.filter(|u| u.starts_with("https://discord.com/") && u.len() <= 300).map(str::to_string)
}

pub(super) fn show(ui: &mut Ui, cx: &mut Cx, state: &mut State, scope: &Scope, acts: &mut Vec<Act>) {
    let ctx = ui.ctx().clone();
    let now = orbiters::now();
    let staged = state.staged;
    {
        let State { open, jobs, .. } = state;
        let Some(o) = open.as_mut() else { return };
        if !o.events.started && !staged {
            o.events.begin(now);
        }
        if !staged {
            if let Some((month, offset)) = o.events.due() {
                jobs.ask(&ctx, cx.ctl, scope, Kind::Events { group: o.id.clone(), month, offset }, Request::Events { group: o.id.clone(), month, offset });
            }
        }
        o.events.told.follow(&ctx, cx, staged);
    }
    note(ui, &orbiters_i18n::tr!("Times are in your time zone ({zone}).", zone = orbiters::zone()), theme::FG_3);
    let Some(o) = state.open.as_ref() else { return };
    let (coming, mut over): (Vec<usize>, Vec<usize>) = (0..o.events.items.len()).partition(|i| !o.events.items[*i].past(now));
    over.reverse();
    let more_read = over.len() > o.events.past;
    over.truncate(o.events.past);
    let (busy, failed, exhausted, none) = (o.events.busy(), o.events.failed, o.events.exhausted(), o.events.items.is_empty());
    let orbiters_failed = o.events.told.failed;
    if let Some(f) = failed {
        note(ui, &failure_text(f), theme::WARN);
        button_row(ui, |ui| {
            if widgets::Button::new(tr("Try again")).icon(icons::ARROW_CLOCKWISE).size(widgets::Size::Sm).id_salt("group-events-retry").show(ui).clicked() {
                acts.push(Act::EventsAgain);
            }
        });
    }
    if none && busy {
        for _ in 0..2 {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 164.0), Sense::hover());
            orbiters_ui::feedback::shimmer(ui.painter(), r, theme::CARD_RADIUS);
        }
        return;
    }
    if orbiters_failed {
        note(ui, tr("Orbiters could not be reached: what it adds to these events does not show."), theme::FG_3);
    }
    heading(ui, tr("Upcoming"));
    if coming.is_empty() && failed.is_none() {
        note(ui, if busy { tr("Reading…") } else { tr("Nothing is planned in this group's VRChat calendar.") }, theme::FG_3);
    }
    for i in coming {
        event(ui, cx, state, scope, i, now, acts);
    }
    heading(ui, tr("Past events"));
    let any = !over.is_empty();
    for i in over {
        event(ui, cx, state, scope, i, now, acts);
    }
    if busy {
        note(ui, tr("Reading earlier events…"), theme::FG_3);
    } else if more_read || !exhausted {
        if failed.is_none() {
            button_row(ui, |ui| {
                if widgets::Button::new(tr("Show earlier events")).icon(icons::CLOCK_COUNTER_CLOCKWISE).id_salt("group-events-earlier").show(ui).clicked() {
                    acts.push(Act::EarlierEvents);
                }
            });
        }
    } else {
        note(ui, if any { tr("Nothing earlier was found.") } else { tr("No past events were found.") }, theme::FG_3);
    }
}

impl State {
    pub(super) fn events_again(&mut self) {
        if let Some(o) = self.open.as_mut() {
            o.events.failed = None;
        }
    }
    pub(super) fn earlier_events(&mut self) {
        if let Some(o) = self.open.as_mut() {
            o.events.more_past(orbiters::now());
        }
    }
}

fn heading(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    widgets::text(ui.painter(), r.left_center(), Align2::LEFT_CENTER, &Txt::new(text, 15.5).weight(weight::SEMIBOLD));
}

fn access_text(e: &CalendarEvent, roles: &HashMap<String, String>) -> Option<String> {
    let named: Vec<String> = e.role_ids.iter().filter_map(|r| roles.get(r).cloned()).collect();
    if !named.is_empty() {
        return Some(named.join(", "));
    }
    match e.access.as_str() {
        "public" => Some(tr("Public").to_string()),
        "group" => Some(tr("Members").to_string()),
        _ => None,
    }
}

/// What an event is, in a few words each: for whom, its category, its platforms, who is interested.
fn tags(e: &CalendarEvent, roles: &HashMap<String, String>) -> Vec<(String, Color32)> {
    let mut out: Vec<(String, Color32)> = Vec::new();
    if e.draft {
        out.push((tr("Draft").into(), theme::WARN));
    }
    if e.featured {
        out.push((tr("Featured").into(), theme::ACCENT));
    }
    out.extend(access_text(e, roles).map(|t| (t, theme::FG_2)));
    if !e.category.is_empty() {
        out.push((orbiters::category_label(&e.category), theme::FG_2));
    }
    let platforms: Vec<&str> = [("standalonewindows", "PC"), ("android", "Android"), ("ios", "iOS")].into_iter().filter(|(k, _)| e.platforms.iter().any(|p| p == k)).map(|(_, n)| n).collect(); // i18n: not shown (platforms' own names)
    if !platforms.is_empty() {
        out.push((platforms.join(" · "), theme::FG_2));
    }
    match e.repeats.as_deref() {
        Some("daily") => out.push((tr("Every day").into(), theme::FG_2)),
        Some("weekly") => out.push((tr("Every week").into(), theme::FG_2)),
        Some("monthly") => out.push((tr("Every month").into(), theme::FG_2)),
        Some("yearly") => out.push((tr("Every year").into(), theme::FG_2)),
        _ => {}
    }
    if let Some(n) = e.interested {
        out.push((tr_n("{count} person interested", "{count} people interested", n as u64), theme::FG_2));
    }
    if e.following == Some(true) {
        out.push((tr("You follow it").into(), theme::ACCENT));
    }
    out
}

/// One event's card. Laid out only in view: elsewhere its last measured height holds its place.
fn event(ui: &mut Ui, cx: &mut Cx, state: &mut State, scope: &Scope, index: usize, now: i64, acts: &mut Vec<Act>) {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let top = ui.cursor().min;
    let staged = state.staged;
    let Some(o) = state.open.as_mut() else { return };
    let Some(e) = o.events.items.get(index).cloned() else { return };
    let guess = o.events.heights.get(&e.id).copied().unwrap_or(164.0);
    if !ui.is_rect_visible(Rect::from_min_size(top, vec2(w, guess)).expand(300.0)) {
        ui.allocate_exact_size(vec2(w, guess), Sense::hover());
        return;
    }
    let group = o.id.clone();
    let name = o.page.as_ref().map(|p| p.group.name.clone()).unwrap_or_else(|| o.card.name.clone());
    let roles: HashMap<String, String> = o.page.as_ref().and_then(|p| p.group.roles.as_deref()).unwrap_or(&[]).iter().map(|r| (r.id.clone(), g::label(&r.name, g::ROLE_NAME_MAX))).collect();
    let development = cx.snap.settings.development;
    let said = match o.events.told.of(&ctx, development, &e, &group, &name, staged) {
        Some(Says::Is(page, invites)) => Some((page.clone(), invites.clone())),
        _ => None,
    };
    let picture = state.picture(&ctx, cx.ctl, scope, &e.image_url);
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = Rect::from_min_size(top + vec2(16.0, 16.0), vec2(w - 32.0, 100_000.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(("group-event", e.id.as_str())));
    child.spacing_mut().item_spacing = vec2(0.0, 6.0);
    // The picture on the left, its words beside it.
    let art = if w >= 760.0 { vec2(232.0, 130.5) } else { vec2(184.0, 103.5) };
    let shot = Rect::from_min_size(inner.min, art);
    let p = child.painter().clone();
    match &picture {
        Some(t) => paint::squircle_texture(&p, shot, [theme::squircle(14.0); 4], t.id(), super::super::launcher::cover_uv(t.size_vec2(), shot.size()), Color32::WHITE),
        None => orbiters::paint_cover(&p, shot, [14.0; 4], None, &e.id, 1.0, 1.0),
    }
    let phase = orbiters::phase(said.as_ref().map(|(p, _)| p.status.as_str()), e.starts_at.as_deref(), e.ends_at.as_deref(), said.as_ref().is_some_and(|(p, _)| p.steps.iter().any(|s| s.live)), now);
    orbiters::status_pill(&p, &child, shot.min + vec2(8.0, 8.0), phase, true);
    let left = shot.right() + 18.0;
    let mut side = child.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(left, inner.top()), inner.max)).layout(egui::Layout::top_down(egui::Align::Min)).id_salt("words"));
    side.spacing_mut().item_spacing = vec2(0.0, 7.0);
    paragraph(&mut side, &e.title, 16.0, weight::SEMIBOLD, theme::FG, 22.0, 2);
    paragraph(&mut side, &orbiters::when(e.starts_at.as_deref(), e.ends_at.as_deref()), 13.0, weight::MEDIUM, theme::FG_2, 18.0, 2);
    chips(&mut side, &tags(&e, &roles));
    if !e.description.trim().is_empty() {
        paragraph(&mut side, &e.description, 13.0, weight::REGULAR, theme::FG_2, 19.0, 4);
    }
    if let Some(url) = group_events::web_page(&group, &e.id) {
        if widgets::Button::new(tr("Open on vrchat.com")).icon(icons::ARROW_SQUARE_OUT).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt(("event-web", e.id.as_str())).show(&mut side).clicked() {
            acts.push(Act::Url(url));
        }
    }
    let used = side.min_rect().height().max(art.y);
    child.allocate_exact_size(vec2(inner.width(), used), Sense::hover());
    if let Some((page, invites)) = &said {
        added(&mut child, &e, page, invites.as_ref(), development, acts);
    }
    let h = child.min_rect().height() + 32.0;
    let rect = Rect::from_min_size(top, vec2(w, h));
    ui.painter().set(bg, egui::Shape::mesh(paint::squircle_mesh(rect, theme::squircle(theme::CARD_RADIUS), theme::surface(), 1.0 / ctx.pixels_per_point())));
    ui.allocate_exact_size(vec2(w, h), Sense::hover());
    if let Some(o) = state.open.as_mut() {
        if (o.events.heights.insert(e.id.clone(), h).unwrap_or(0.0) - h).abs() > 0.5 {
            ctx.request_repaint();
        }
    }
}

fn paragraph(ui: &mut Ui, text: &str, size: f32, heavy: f32, color: Color32, line_height: f32, rows: usize) {
    let w = ui.available_width();
    let mut job = Txt::new(text, size).weight(heavy).color(color).line_height(line_height).job(w);
    job.wrap.max_rows = rows;
    let galley = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y), Sense::hover());
    ui.painter().galley(r.min, galley, color);
}

fn chips(ui: &mut Ui, tags: &[(String, Color32)]) {
    if tags.is_empty() {
        return;
    }
    let w = ui.available_width();
    let widths: Vec<f32> = tags.iter().map(|(t, _)| (widgets::text_size(ui, &Txt::new(t, 11.5).weight(weight::MEDIUM)).x + 18.0).min(w)).collect();
    let (spots, h) = widgets::flow(&widths, w, 22.0, 6.0);
    let (area, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter();
    for (((text, color), cw), at) in tags.iter().zip(&widths).zip(spots) {
        let r = Rect::from_min_size(area.min + at, vec2(*cw, 22.0));
        p.rect_filled(r, theme::radius(11.0), theme::alpha(if *color == theme::FG_2 { theme::FG_3 } else { *color }, 0.14));
        widgets::text_truncated(p, pos2(r.left() + 9.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(text, 11.5).weight(weight::MEDIUM).color(*color), cw - 14.0);
    }
}

/// How an invite of the member's own stands, over all of the event's steps: the furthest one.
fn invite_text(page: &Overview, invites: Option<&Invites>) -> Option<&'static str> {
    let all: Vec<&str> = invites.into_iter().flat_map(|i| i.items.values()).map(String::as_str).collect();
    if all.contains(&"sent") {
        Some(tr("Your invite was sent in VRChat."))
    } else if all.iter().any(|s| matches!(*s, "pending" | "sending" | "uncertain")) {
        Some(tr("You're on the invite list."))
    } else if page.steps.iter().any(|s| s.invite_open) {
        Some(tr("Open: ask for your invite on Orbiters or in Events."))
    } else {
        None
    }
}

/// What Orbiters adds to an event, under its VRChat half.
fn added(ui: &mut Ui, e: &CalendarEvent, page: &Overview, invites: Option<&Invites>, development: bool, acts: &mut Vec<Act>) {
    ui.add_space(6.0);
    let w = ui.available_width();
    let (head, _) = ui.allocate_exact_size(vec2(w, 44.0), Sense::hover());
    let p = ui.painter().clone();
    p.hline(head.x_range(), head.top(), theme::hairline());
    let y = head.bottom() - 18.0;
    logo::paint(&p, Rect::from_center_size(pos2(head.left() + 10.0, y), logo::size_for_width(20.0)), logo::Look::default());
    widgets::text(&p, pos2(head.left() + 28.0, y), Align2::LEFT_CENTER, &Txt::new(tr("On Orbiters"), 13.5).weight(weight::SEMIBOLD));
    if let Some(url) = orbiters_page(development, &page.id) {
        let b = widgets::Button::new(tr("Open on Orbiters")).icon(icons::ARROW_SQUARE_OUT).size(widgets::Size::Sm).id_salt(("event-orbiters", e.id.as_str()));
        let bw = b.desired_width(ui);
        if b.show_at(ui, Rect::from_min_size(pos2(head.right() - bw, y - 16.0), vec2(bw, 32.0))).clicked() {
            acts.push(Act::Url(url));
        }
    }
    let plain = |s: &str| g::text(&orbiters::plain(s), 1200);
    if !page.title.trim().is_empty() && page.title.trim() != e.title.trim() {
        line(ui, tr("Title"), &g::label(&page.title, 128));
    }
    // Orbiters' own words, where they say more than VRChat's.
    let long = plain(&page.long_description);
    let words = if long.is_empty() { plain(&page.description) } else { long };
    if !words.is_empty() && words.trim() != e.description.trim() {
        line(ui, tr("Description"), &words);
    }
    if let Some(d) = page.discord.as_ref().filter(|d| !d.name.is_empty()) {
        line(ui, "Discord", &g::label(&d.name, 100)); // i18n: not shown (Discord's own name)
    }
    let who = match page.access.as_deref() {
        Some("public") => Some(tr("Everyone").to_string()),
        Some("members") => Some(tr("Members of the group").to_string()),
        Some("plus") => Some(tr("Members of the group and their friends").to_string()),
        _ => None,
    };
    if let Some(who) = who {
        line(ui, tr("Who can come"), &if page.adult_only { format!("{who} · 18+") } else { who });
    }
    if page.planning || page.status == "planning" {
        line(ui, tr("Voting"), tr("Open: vote for the time and the worlds on Orbiters."));
    }
    let signed = invite_text(page, invites);
    let count = page.signups.map(|n| tr_n("{count} sign-up", "{count} sign-ups", n as u64));
    if signed.is_some() || count.is_some() {
        let both: Vec<&str> = count.as_deref().into_iter().chain(signed).collect();
        line(ui, tr("Sign-ups"), &both.join(" · "));
    }
    let steps: Vec<String> = page
        .steps
        .iter()
        .filter(|s| page.steps.len() > 1 || s.world.as_ref().is_some_and(|w| w.name.is_some()) || s.movie.is_some())
        .map(|s| {
            let at = if s.organizer_starts { Some(tr("Later").to_string()) } else { orbiters::clock_of(s.starts_at.as_deref()) };
            let world = s.world.as_ref().and_then(|w| w.name.as_deref()).map(|n| g::label(n, 80));
            let movie = s.movie.as_ref().map(|m| g::label(&m.title, 80));
            let title = (page.steps.len() > 1 && !s.title.is_empty()).then(|| g::label(&s.title, 64));
            [at, title, world, movie].into_iter().flatten().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" · ")
        })
        .filter(|t| !t.is_empty())
        .collect();
    if !steps.is_empty() {
        line(ui, tr("Program"), &steps.join("\n"));
    }
    let staff: Vec<String> = page.details.staff.iter().filter(|s| !s.people.is_empty()).map(|s| format!("{}: {}", g::label(&s.name, 64), s.people.iter().map(|p| g::label(&p.username, 32)).collect::<Vec<_>>().join(", "))).collect();
    if !staff.is_empty() {
        line(ui, tr("Event staff"), &staff.join("\n"));
    }
    let prizes: Vec<String> = page.details.prizes.iter().map(|p| [g::label(&p.place(), 32), g::label(&p.title, 100)].into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" · ")).filter(|t| !t.is_empty()).collect();
    if !prizes.is_empty() {
        line(ui, tr("Prizes"), &prizes.join("\n"));
    }
    let links = [(tr("Open the Discord event"), discord(page.discord_event_url.as_deref())), (tr("Open the Discord announcement"), discord(page.discord_announcement_url.as_deref()))];
    if links.iter().any(|(_, u)| u.is_some()) {
        button_row(ui, |ui| {
            for (label, url) in links {
                let Some(url) = url else { continue };
                if widgets::Button::new(label).icon(icons::DISCORD_LOGO).size(widgets::Size::Sm).id_salt(("event-discord", e.id.as_str(), label)).show(ui).clicked() {
                    acts.push(Act::Url(url));
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::group_events::Events;
    use time::format_description::well_known::Rfc3339;
    use time::OffsetDateTime;

    const DAY: i64 = 86400;

    fn iso(t: i64) -> String {
        OffsetDateTime::from_unix_timestamp(t).unwrap().format(&Rfc3339).unwrap()
    }
    fn at(id: &str, start: i64) -> CalendarEvent {
        CalendarEvent { id: format!("cal_{id}"), title: id.into(), starts_at: Some(iso(start)), ends_at: Some(iso(start + 7200)), ..Default::default() }
    }
    fn page(items: Vec<CalendarEvent>, more: bool) -> Reply {
        Reply::Events(Events { raw: items.len(), items, more })
    }

    /// The calendar whole (page by page), then this month and the next; each event once, as VRChat last gave it.
    #[test]
    fn the_calendar_is_read_once_each() {
        let now = orbiters::now();
        let mut l = EventList::default();
        l.begin(now);
        assert_eq!(l.due(), Some((None, 0)));
        assert_eq!(l.due(), None, "one read at a time");
        l.answer(None, 5, page(vec![at("x", now)], false));
        assert!(l.items.is_empty(), "an answer to another read is not taken");
        l.answer(None, 0, page(vec![at("b", now + DAY), at("a", now - DAY)], true));
        assert_eq!(l.due(), Some((None, 2)), "the rest of it first");
        l.answer(None, 2, page(vec![at("c", now + 2 * DAY)], false));
        let month = Month::of(now);
        assert_eq!(l.due(), Some((Some(month), 0)));
        l.answer(Some(month), 0, page(vec![CalendarEvent { title: "Renamed".into(), ..at("b", now + DAY) }], false));
        assert_eq!(l.due(), Some((Some(month.after()), 0)));
        l.answer(Some(month.after()), 0, page(vec![], false));
        assert!(!l.busy());
        assert_eq!(l.items.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["a", "Renamed", "c"]);
        assert_eq!(l.past_count(now), 1);
    }

    /// A read that failed waits for the person, then goes on from where it was.
    #[test]
    fn a_failed_read_goes_on_from_there() {
        let now = orbiters::now();
        let mut l = EventList::default();
        l.begin(now);
        l.due();
        l.answer(None, 0, page(vec![at("a", now)], true));
        l.due();
        l.answer(None, 1, Reply::Failed(Failure::Offline));
        assert_eq!(l.due(), None);
        l.failed = None;
        assert_eq!(l.due(), Some((None, 1)));
    }

    /// Earlier events: those read already, then earlier months, a few a press, until a year of nothing.
    #[test]
    fn earlier_events_are_read_a_few_months_at_a_time() {
        let now = orbiters::now();
        let mut l = EventList::default();
        l.stage((0..12).map(|i| at(&i.to_string(), now - (i + 1) * DAY)).collect());
        assert_eq!((l.past, l.past_count(now)), (PAST_PAGE, 12));
        l.more_past(now);
        // 16 are wanted and 12 are read: earlier months, one after another.
        let oldest = Month::of(now - 12 * DAY);
        let Some((Some(first), 0)) = l.due() else { panic!() };
        assert_eq!(first, oldest.before());
        l.answer(Some(first), 0, page((0..5).map(|i| at(&format!("old{i}"), now - 70 * DAY - i)).collect(), false));
        assert_eq!(l.due(), None, "enough for this press");
        assert_eq!(l.past_count(now), 17);
        l.more_past(now);
        let mut read = 0;
        while let Some((month, offset)) = l.due() {
            read += 1;
            l.answer(month, offset, page(vec![], false));
        }
        assert_eq!(read, MONTHS_A_PRESS, "no more than that for one press");
        assert!(!l.exhausted());
        for _ in 0..3 {
            l.more_past(now);
            while let Some((month, offset)) = l.due() {
                read += 1;
                l.answer(month, offset, page(vec![], false));
            }
        }
        assert!(l.exhausted() && read == EMPTY_MONTHS, "{read}");
    }

    /// What Orbiters says shows on the event it is of, and on no other: its page must be this group's.
    #[test]
    fn what_orbiters_says_is_on_its_own_event() {
        let (group, start) = ("grp_00000000-0000-4000-8000-00000000a001", 1_795_286_400);
        let e = at("Showcase", start);
        let card = Card { id: "11111111-1111-4111-8111-111111111111".into(), title: "showcase".into(), starts_at: Some(iso(start)), ..Default::default() };
        let of = |of: &str| Overview { id: card.id.clone(), starts_at: Some(iso(start)), community: Some(orbiters_core::community::Host { id: Some(of.into()), ..Default::default() }), ..Default::default() };
        let ctx = egui::Context::default();
        let mut told = Told { cards: vec![card.clone()], ..Default::default() };
        assert!(told.of(&ctx, false, &e, group, "Makers", true).is_none(), "its page is not read yet");
        told.pages.insert(card.id.clone(), Says::Is(Box::new(of("grp_other")), None));
        assert!(told.of(&ctx, false, &e, group, "Makers", true).is_none(), "another group's event of the same name and time");
        told.pages.insert(card.id.clone(), Says::Is(Box::new(of(group)), None));
        assert!(told.of(&ctx, false, &e, group, "Makers", true).is_some());
        assert!(told.of(&ctx, false, &at("Showcase", start + 3600), group, "Makers", true).is_none());
        told.pages.insert(card.id.clone(), Says::Nothing);
        assert!(told.of(&ctx, false, &e, group, "Makers", true).is_none());
    }

    #[test]
    fn only_orbiters_and_discord_links_open() {
        assert_eq!(orbiters_page(false, "11111111-1111-4111-8111-111111111111").unwrap(), "https://orbiters.cc/events/11111111-1111-4111-8111-111111111111");
        assert!(orbiters_page(false, "../x").is_none() && orbiters_page(false, "").is_none());
        assert!(discord(Some("https://discord.com/events/1/2")).is_some());
        assert!(discord(Some("https://discord.com.example/x")).is_none() && discord(Some("http://discord.com/x")).is_none() && discord(None).is_none());
    }

    #[test]
    fn an_events_tags_say_who_what_and_how_many() {
        let roles = HashMap::from([("grol_a".to_string(), "Artists".to_string())]);
        let text = |e: &CalendarEvent| tags(e, &roles).into_iter().map(|t| t.0).collect::<Vec<_>>();
        let e = CalendarEvent { access: "group".into(), category: "arts".into(), platforms: vec!["android".into(), "standalonewindows".into()], interested: Some(12), repeats: Some("weekly".into()), following: Some(true), draft: true, ..Default::default() };
        assert_eq!(text(&e), ["Draft", "Members", "Arts", "PC · Android", "Every week", "12 people interested", "You follow it"]);
        let e = CalendarEvent { access: "group".into(), role_ids: vec!["grol_a".into(), "grol_gone".into()], ..Default::default() };
        assert_eq!(text(&e), ["Artists"], "what VRChat leaves out is not said");
        assert!(text(&CalendarEvent::default()).is_empty());
    }

    #[test]
    fn the_members_own_invite_is_said() {
        let page = Overview { steps: vec![orbiters_core::community::Step { invite_open: true, ..Default::default() }], ..Default::default() };
        let with = |s: &str| Invites { items: [("main".to_string(), s.to_string())].into(), ..Default::default() };
        assert_eq!(invite_text(&page, Some(&with("sent"))), Some("Your invite was sent in VRChat."));
        assert_eq!(invite_text(&page, Some(&with("pending"))), Some("You're on the invite list."));
        assert_eq!(invite_text(&page, Some(&with("withdrawn"))), Some("Open: ask for your invite on Orbiters or in Events."));
        assert_eq!(invite_text(&Overview::default(), None), None);
    }
}
