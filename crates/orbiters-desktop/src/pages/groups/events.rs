//! The Events tab of a group's page (REQUEST28): its VRChat calendar, read only. What is coming soonest first, then what
//! is over, newest first and a few at a time; each with its picture, when in this computer's time zone, for whom, its
//! category, platforms and how many are interested. Where Orbiters has the same event, its card also says what Orbiters adds:
//! its words, the Discord server and links, sign-ups, voting, program, staff and prizes, each only where Orbiters gives it to
//! this member.
//!
//! VRChat is asked through the service like the rest of the page (`Request::Events`): the calendar whole, then this month
//! and the next, then an earlier month at a time as the person asks (VRChat lists by month). Orbiters is asked as the
//! member signed in there ([`Told`]): which of its events the calendar's entries are, by their ids, then each one's page. A
//! server from before that route is asked for its list, matched by group and time ([`group_events::candidate`]).
//!
//! Both are kept on this computer in the account's own folder (`orbiters_core::vrchat_store`, the service writes it): they
//! show at once at the next start and change in place when VRChat and Orbiters answer.
use super::page::{button_row, line, note, Act};
use super::{failure_text, Kind, State};
use crate::controller::groups::{Failure, Reply, Request};
use crate::controller::vrchat_data::Scope;
use crate::pages::{events as orbiters, AccountFollow, Cx, ForAccount, Job};
use crate::worlds::{Picture, Pictures};
use egui::{pos2, vec2, Align2, Color32, Rect, Sense, Ui};
use orbiters_core::account::{Api, Server};
use orbiters_core::community::{Card, Invites, Overview, Refused, CALENDAR_IDS};
use orbiters_core::group_events::{self, CalendarEvent, Month, EVENTS_MAX};
use orbiters_core::groups as g;
use orbiters_i18n::{tr, tr_n};
use orbiters_ui::theme::{self, weight, Txt};
use orbiters_ui::{icons, logo, paint, widgets};
use std::collections::{HashMap, HashSet, VecDeque};

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
    /// The first month of the two being looked at whole, what VRChat gave since, and whether it is done (to be kept).
    looking: Option<Month>,
    seen: HashSet<String>,
    looked: bool,
    pub told: Told,
}

impl EventList {
    /// The page was read again: so is the calendar, what is shown staying meanwhile.
    pub fn again(&mut self) {
        (self.started, self.failed, self.reading) = (false, None, None);
        self.queue.clear();
        self.told.again();
    }

    /// As last kept on this computer: shown at once, read again when the page is.
    pub fn as_kept(&mut self, items: Option<Vec<CalendarEvent>>, told: Option<group_events::Told>) {
        self.items = items.unwrap_or_default();
        self.past = PAST_PAGE;
        if let Some(told) = told {
            self.told.cards = told.cards;
            self.told.pages = told.pages.into_iter().map(|(p, i)| (p.id.clone(), (p, i))).collect();
        }
    }

    fn begin(&mut self, now: i64) {
        let month = Month::of(now);
        self.started = true;
        self.past = self.past.max(PAST_PAGE);
        self.looking = Some(month);
        self.seen.clear();
        self.queue = VecDeque::from([(None, 0), (Some(month), 0), (Some(month.after()), 0)]);
    }

    /// The entries that show: what is coming, and the past ones shown.
    fn shown(&self, now: i64) -> Vec<String> {
        let (over, coming): (Vec<&CalendarEvent>, Vec<&CalendarEvent>) = self.items.iter().partition(|e| e.past(now));
        coming.into_iter().chain(over.into_iter().rev().take(self.past)).map(|e| e.id.clone()).collect()
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
        self.seen.extend(page.items.iter().map(|e| e.id.clone()));
        let new = group_events::merge(&mut self.items, &page);
        if page.more && page.raw > 0 && offset + page.raw < EVENTS_MAX {
            // The rest of the same list, before anything else.
            self.queue.push_front((month, offset + page.raw));
            return;
        }
        // All of this month and the next was read: what none of it gave is not on the calendar any more.
        if self.looking.is_some_and(|first| month == Some(first.after())) {
            if let Some(first) = self.looking.take() {
                group_events::prune(&mut self.items, first, &self.seen);
                self.looked = true;
            }
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

/// An event's page on Orbiters, and the member's invites.
type Page = (Overview, Option<Invites>);

/// What was asked of Orbiters' list.
enum Listed {
    /// Which of its events these calendar entries are.
    Entries(Vec<String>, anyhow::Result<Vec<Card>>),
    /// Its list, from a server that does not know the entries.
    All(anyhow::Result<Vec<Card>>),
}

/// Orbiters' side: which of its events the calendar's entries are, and the page of each that shows, one read at a time. All
/// of it for the member signed in to Orbiters: another one starts it over.
#[derive(Default)]
pub(crate) struct Told {
    account: AccountFollow,
    list: Job<ForAccount<Listed>>,
    /// The entries asked for since the page was last read.
    asked: HashSet<String>,
    /// The server does not know calendar entries: its list, matched by group and time, asked for once.
    by_time: bool,
    listed: bool,
    failed: bool,
    pub cards: Vec<Card>,
    pub pages: HashMap<String, Page>,
    /// The pages read since the group's page was (the others show as they were kept).
    fresh: HashSet<String>,
    reading: Option<(String, Job<ForAccount<anyhow::Result<Page>>>)>,
    /// Something changed since it was last kept.
    changed: bool,
    icons: Option<Pictures>,
}

impl Told {
    fn again(&mut self) {
        self.asked.clear();
        self.fresh.clear();
        (self.listed, self.failed, self.by_time) = (false, false, false);
    }

    /// Takes what came, and asks which of Orbiters' events the entries that show are. Never in tests, nor while a screenshot
    /// stages.
    fn follow(&mut self, ctx: &egui::Context, cx: &Cx, entries: Vec<String>, staged: bool) {
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
            self.take_list(answer);
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
            self.take_page(id, answer);
        }
        if self.list.running() || self.failed {
            return;
        }
        let server = server(development);
        let member = crate::member::reading(server);
        if self.by_time {
            if !std::mem::replace(&mut self.listed, true) {
                self.list = crate::pages::job_for(ctx, &seen.owner, move || Listed::All(list(server, member)));
            }
            return;
        }
        let ids: Vec<String> = entries.into_iter().filter(|id| group_events::is_event_id(id) && !self.asked.contains(id)).take(CALENDAR_IDS).collect();
        if !ids.is_empty() {
            self.asked.extend(ids.iter().cloned());
            self.list = crate::pages::job_for(ctx, &seen.owner, move || {
                let found = match member.map(|m| m.api()) {
                    Some((api, token)) => api.events_of_calendar(Some(&token), &ids),
                    None => Api::new(server).events_of_calendar(None, &ids),
                };
                Listed::Entries(ids, found)
            });
        }
    }

    fn take_list(&mut self, answer: Listed) {
        match answer {
            Listed::Entries(ids, Ok(cards)) => {
                // What these entries are, and no more: one that is nothing of Orbiters' (any more) is gone.
                let of = |c: &Card| c.calendar_id.as_ref().is_some_and(|id| ids.contains(id));
                let gone: HashSet<String> = self.cards.iter().filter(|c| of(c) && !cards.iter().any(|n| n.id == c.id)).map(|c| c.id.clone()).collect();
                self.pages.retain(|id, _| !gone.contains(id));
                let before = self.cards.clone();
                // Cards of the list matched by time belong to a server from before; this one has said better.
                self.cards.retain(|c| c.calendar_id.is_some() && !of(c));
                self.cards.extend(cards.into_iter().filter(of));
                self.changed |= self.cards != before;
            }
            Listed::Entries(_, Err(e)) if Refused::of(&e).is_some_and(Refused::no_calendar_route) => self.by_time = true,
            Listed::All(Ok(cards)) => {
                self.changed |= self.cards != cards;
                self.cards = cards;
            }
            // What shows stays.
            Listed::Entries(_, Err(_)) | Listed::All(Err(_)) => self.failed = true,
        }
    }

    fn take_page(&mut self, id: String, answer: Option<anyhow::Result<Page>>) {
        match answer {
            Some(Ok(page)) => {
                self.changed |= self.pages.get(&id) != Some(&page);
                self.pages.insert(id.clone(), page);
            }
            // Not for this member (any more), or gone.
            Some(Err(e)) if Refused::of(&e).is_some_and(|r| matches!(r.status, 401 | 403 | 404)) => self.changed |= self.pages.remove(&id).is_some(),
            // Not read: what shows stays.
            _ => {}
        }
        self.fresh.insert(id);
    }

    /// What Orbiters says of `event`: its page when it is here and is this event's, else its card when it was made as this
    /// very entry. Its page is asked for when it was not yet (`ask`).
    fn of(&mut self, ctx: &egui::Context, development: bool, event: &CalendarEvent, group: &str, group_name: &str, ask: bool) -> Option<Page> {
        let card = group_events::candidate(event, group_name, &self.cards)?.clone();
        if ask && !self.fresh.contains(&card.id) && self.reading.is_none() && !cfg!(test) {
            if let Some(owner) = self.account.owner().cloned() {
                let server = server(development);
                let (member, asked) = (crate::member::reading(server), card.id.clone());
                let job = crate::pages::job_for(ctx, &owner, move || match member.map(|m| m.api()) {
                    Some((api, token)) => {
                        let page = api.event(Some(&token), &asked)?;
                        // A server from before gives the member's invites apart.
                        let invites = if page.invites.is_none() { api.my_invites(&token, &asked).ok() } else { None };
                        Ok((page, invites))
                    }
                    None => Ok((Api::new(server).event(None, &asked)?, None)),
                });
                self.reading = Some((card.id.clone(), job));
            }
        }
        match self.pages.get(&card.id) {
            Some((page, invites)) if group_events::is_of(event, group, page) => Some((page.clone(), invites.clone())),
            _ => (card.calendar_id.as_deref() == Some(event.id.as_str())).then(|| (card.overview(), None)),
        }
    }

    /// What is kept for the next start: the cards and pages of this group's own events.
    fn to_keep(&self, events: &[CalendarEvent], group: &str, group_name: &str) -> group_events::Told {
        let mut out = group_events::Told::default();
        for e in events.iter().rev() {
            let Some(card) = group_events::candidate(e, group_name, &self.cards) else { continue };
            if out.cards.len() >= group_events::Told::MAX || out.cards.iter().any(|c| c.id == card.id) {
                continue;
            }
            let page = self.pages.get(&card.id).filter(|(p, _)| group_events::is_of(e, group, p));
            if page.is_none() && card.calendar_id.as_deref() != Some(e.id.as_str()) {
                continue;
            }
            out.cards.push(card.clone());
            out.pages.extend(page.cloned());
        }
        out
    }

    /// The picture of a Discord server.
    fn icon(&mut self, ctx: &egui::Context, url: Option<&str>, staged: bool) -> Option<egui::TextureHandle> {
        let url = url.filter(|u| u.starts_with("https://cdn.discordapp.com/") && u.len() <= 300)?;
        if staged || cfg!(test) {
            return None;
        }
        self.icons.get_or_insert_with(|| Pictures::new(ctx)).get(ctx, &Picture::Gallery { development: false, asset: 0, id: 0, url: url.to_string(), large: false })
    }
}

/// Orbiters' list, from a server that does not know calendar entries: what it lists, and (events already over are not listed)
/// the events of the galleries the member sees.
fn list(server: Server, member: Option<crate::member::Member>) -> anyhow::Result<Vec<Card>> {
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
    if let Some(galleries) = member.and_then(|m| {
        let (api, token) = m.api();
        api.galleries(&token).ok()
    }) {
        let known: HashSet<String> = cards.iter().map(|c| c.id.clone()).collect();
        cards.extend(galleries.all().filter_map(|g| g.event.clone()).filter(|c| !known.contains(&c.id)));
    }
    Ok(cards)
}

fn server(development: bool) -> Server {
    if development { Server::Development } else { Server::Production }
}

/// An event's page on Orbiters: where the server says, on the website itself, else by its id.
fn orbiters_page(development: bool, page: &Overview) -> Option<String> {
    let website = server(development).website();
    let given = page.url.as_deref().filter(|u| u.starts_with(website) && u.len() <= 300 && !u.contains(|c: char| c.is_whitespace() || c.is_control()));
    given.map(str::to_string).or_else(|| by_id(website, &page.id))
}
fn by_id(website: &str, id: &str) -> Option<String> {
    (!id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')).then(|| format!("{website}events/{id}"))
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
        // Nothing is asked for an account on its way back, nor before the group's own page is read again.
        let held = state.held;
        let State { open, jobs, .. } = state;
        let Some(o) = open.as_mut() else { return };
        if !staged && !held && !o.kept() {
            if !o.events.started {
                o.events.begin(now);
            }
            if let Some((month, offset)) = o.events.due() {
                jobs.ask(&ctx, cx.ctl, scope, Kind::Events { group: o.id.clone(), month, offset }, Request::Events { group: o.id.clone(), month, offset });
            }
            let entries = o.events.shown(now);
            o.events.told.follow(&ctx, cx, entries, staged);
            // Kept for the next start, once what is on its way came.
            let quiet = o.events.told.reading.is_none() && !o.events.told.list.running() && !o.events.busy();
            if (o.events.told.changed || o.events.looked) && quiet {
                (o.events.told.changed, o.events.looked) = (false, false);
                let name = o.group_name();
                let told = o.events.told.to_keep(&o.events.items, &o.id, &name);
                // The calendar as it is once looked at whole: what is gone from it is not shown at the next start.
                let items = &o.events.items;
                let events = items[items.len().saturating_sub(group_events::EVENTS_KEPT)..].to_vec();
                jobs.ask(&ctx, cx.ctl, scope, Kind::KeepEvents(o.id.clone()), Request::KeepEvents { group: o.id.clone(), events, told: Box::new(told) });
            }
        }
    }
    note(ui, &orbiters_i18n::tr!("Times are in your time zone ({zone}).", zone = orbiters::zone()), theme::fg_3());
    let Some(o) = state.open.as_ref() else { return };
    let (coming, mut over): (Vec<usize>, Vec<usize>) = (0..o.events.items.len()).partition(|i| !o.events.items[*i].past(now));
    over.reverse();
    let more_read = over.len() > o.events.past;
    over.truncate(o.events.past);
    let (busy, failed, exhausted, none) = (o.events.busy(), o.events.failed, o.events.exhausted(), o.events.items.is_empty());
    let (earlier, waiting) = (o.events.earlier.is_some(), !o.events.started && !staged);
    let orbiters_failed = o.events.told.failed;
    if let Some(f) = failed {
        note(ui, &failure_text(f), theme::WARN);
        button_row(ui, |ui| {
            if widgets::Button::new(tr("Try again")).icon(icons::ARROW_CLOCKWISE).size(widgets::Size::Sm).id_salt("group-events-retry").show(ui).clicked() {
                acts.push(Act::EventsAgain);
            }
        });
    }
    if none && failed.is_some() {
        return;
    }
    if none && (busy || waiting) {
        for _ in 0..2 {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 164.0), Sense::hover());
            orbiters_ui::feedback::shimmer(ui.painter(), r, theme::CARD_RADIUS);
        }
        return;
    }
    if orbiters_failed {
        note(ui, tr("Orbiters could not be reached: what it adds to these events does not show."), theme::fg_3());
    }
    heading(ui, tr("Upcoming"));
    if coming.is_empty() && failed.is_none() {
        note(ui, if busy { tr("Reading…") } else { tr("Nothing is planned in this group's VRChat calendar.") }, theme::fg_3());
    }
    for i in coming {
        event(ui, cx, state, scope, i, now, acts);
    }
    heading(ui, tr("Past events"));
    let any = !over.is_empty();
    for i in over {
        event(ui, cx, state, scope, i, now, acts);
    }
    if busy || waiting {
        // Read again behind what shows: only what the person asked for is said.
        if earlier {
            note(ui, tr("Reading earlier events…"), theme::fg_3());
        }
    } else if more_read || !exhausted {
        if failed.is_none() {
            button_row(ui, |ui| {
                if widgets::Button::new(tr("Show earlier events")).icon(icons::CLOCK_COUNTER_CLOCKWISE).id_salt("group-events-earlier").show(ui).clicked() {
                    acts.push(Act::EarlierEvents);
                }
            });
        }
    } else {
        note(ui, if any { tr("Nothing earlier was found.") } else { tr("No past events were found.") }, theme::fg_3());
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
        out.push((tr("Featured").into(), theme::accent()));
    }
    out.extend(access_text(e, roles).map(|t| (t, theme::fg_2())));
    if !e.category.is_empty() {
        out.push((orbiters::category_label(&e.category), theme::fg_2()));
    }
    let platforms: Vec<&str> = [("standalonewindows", "PC"), ("android", "Android"), ("ios", "iOS")].into_iter().filter(|(k, _)| e.platforms.iter().any(|p| p == k)).map(|(_, n)| n).collect(); // i18n: not shown (platforms' own names)
    if !platforms.is_empty() {
        out.push((platforms.join(" · "), theme::fg_2()));
    }
    match e.repeats.as_deref() {
        Some("daily") => out.push((tr("Every day").into(), theme::fg_2())),
        Some("weekly") => out.push((tr("Every week").into(), theme::fg_2())),
        Some("monthly") => out.push((tr("Every month").into(), theme::fg_2())),
        Some("yearly") => out.push((tr("Every year").into(), theme::fg_2())),
        _ => {}
    }
    if let Some(n) = e.interested {
        out.push((tr_n("{count} person interested", "{count} people interested", n as u64), theme::fg_2()));
    }
    if e.following == Some(true) {
        out.push((tr("You follow it").into(), theme::accent()));
    }
    out
}

/// One event's card. Laid out only in view: elsewhere its last measured height holds its place.
fn event(ui: &mut Ui, cx: &mut Cx, state: &mut State, scope: &Scope, index: usize, now: i64, acts: &mut Vec<Act>) {
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let top = ui.cursor().min;
    let (staged, held) = (state.staged, state.held);
    let Some(o) = state.open.as_mut() else { return };
    let Some(e) = o.events.items.get(index).cloned() else { return };
    let guess = o.events.heights.get(&e.id).copied().unwrap_or(164.0);
    if !ui.is_rect_visible(Rect::from_min_size(top, vec2(w, guess)).expand(300.0)) {
        ui.allocate_exact_size(vec2(w, guess), Sense::hover());
        return;
    }
    let group = o.id.clone();
    let name = o.group_name();
    let ask = !staged && !held && !o.kept();
    let roles: HashMap<String, String> = o.page.as_ref().and_then(|p| p.group.roles.as_deref()).unwrap_or(&[]).iter().map(|r| (r.id.clone(), g::label(&r.name, g::ROLE_NAME_MAX))).collect();
    let development = cx.snap.settings.development;
    let said = o.events.told.of(&ctx, development, &e, &group, &name, ask);
    let icon = said.as_ref().and_then(|(p, _)| p.discord.as_ref()).and_then(|d| d.icon_url.clone());
    let icon = o.events.told.icon(&ctx, icon.as_deref(), staged);
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
    paragraph(&mut side, &e.title, 16.0, weight::SEMIBOLD, theme::fg(), 22.0, 2);
    paragraph(&mut side, &orbiters::when(e.starts_at.as_deref(), e.ends_at.as_deref()), 13.0, weight::MEDIUM, theme::fg_2(), 18.0, 2);
    chips(&mut side, &tags(&e, &roles));
    if !e.description.trim().is_empty() {
        paragraph(&mut side, &e.description, 13.0, weight::REGULAR, theme::fg_2(), 19.0, 4);
    }
    if let Some(url) = group_events::web_page(&group, &e.id) {
        if widgets::Button::new(tr("Open on vrchat.com")).icon(icons::ARROW_SQUARE_OUT).variant(widgets::Variant::Ghost).size(widgets::Size::Sm).id_salt(("event-web", e.id.as_str())).show(&mut side).clicked() {
            acts.push(Act::Url(url));
        }
    }
    let used = side.min_rect().height().max(art.y);
    child.allocate_exact_size(vec2(inner.width(), used), Sense::hover());
    if let Some((page, invites)) = &said {
        added(&mut child, &e, page, invites.as_ref(), icon.as_ref(), development, acts);
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
        p.rect_filled(r, theme::radius(11.0), theme::alpha(if *color == theme::fg_2() { theme::fg_3() } else { *color }, 0.14));
        widgets::text_truncated(p, pos2(r.left() + 9.0, r.center().y), Align2::LEFT_CENTER, &Txt::new(text, 11.5).weight(weight::MEDIUM).color(*color), cw - 14.0);
    }
}

/// How an invite of the member's own stands, over all of the event's steps: the furthest one.
fn invite_text(page: &Overview, invites: Option<&Invites>) -> Option<&'static str> {
    let all: Vec<&str> = page.invites.as_ref().or(invites).into_iter().flat_map(|i| i.items.values()).map(String::as_str).collect();
    if all.contains(&"sent") {
        Some(tr("Your invite was sent in VRChat."))
    } else if all.iter().any(|s| matches!(*s, "pending" | "sending" | "uncertain")) || (all.is_empty() && page.signed_up == Some(true)) {
        Some(tr("You're on the invite list."))
    } else if page.steps.iter().any(|s| s.invite_open) {
        Some(tr("Open: ask for your invite on Orbiters or in Events."))
    } else {
        None
    }
}

/// What Orbiters adds to an event, under its VRChat half.
fn added(ui: &mut Ui, e: &CalendarEvent, page: &Overview, invites: Option<&Invites>, icon: Option<&egui::TextureHandle>, development: bool, acts: &mut Vec<Act>) {
    ui.add_space(6.0);
    let w = ui.available_width();
    let (head, _) = ui.allocate_exact_size(vec2(w, 44.0), Sense::hover());
    let p = ui.painter().clone();
    p.hline(head.x_range(), head.top(), theme::hairline());
    let y = head.bottom() - 18.0;
    logo::paint(&p, Rect::from_center_size(pos2(head.left() + 10.0, y), logo::size_for_width(20.0)), logo::Look::default());
    widgets::text(&p, pos2(head.left() + 28.0, y), Align2::LEFT_CENTER, &Txt::new(tr("On Orbiters"), 13.5).weight(weight::SEMIBOLD));
    if let Some(url) = orbiters_page(development, page) {
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
        // The server's name beside its picture, where the other lines have theirs.
        let name = g::label(&d.name, 100);
        let (r, _) = ui.allocate_exact_size(vec2(w, 32.0), Sense::hover());
        let p = ui.painter().clone();
        widgets::text(&p, pos2(r.left(), r.center().y - 2.0), Align2::LEFT_CENTER, &Txt::new("Discord", 12.5).color(theme::fg_3())); // i18n: not shown (Discord's own name)
        let face = Rect::from_center_size(pos2(r.left() + 160.0, r.center().y - 2.0), vec2(20.0, 20.0));
        widgets::avatar(&p, face, icon, &name);
        widgets::text_truncated(&p, pos2(face.right() + 8.0, r.center().y - 2.0), Align2::LEFT_CENTER, &Txt::new(&name, 13.5), w - 190.0);
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
        assert!(!l.looked);
        l.answer(Some(month.after()), 0, page(vec![], false));
        assert!(!l.busy() && l.looked);
        assert_eq!(l.items.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["a", "Renamed", "c"], "what any of the reads gave stays");
        assert_eq!(l.past_count(now), 1);
        // Read again: one that none of the reads gives any more goes, once they are all read; a failed look takes nothing away.
        // (The middle of next month: well inside the two that are looked at whole, whatever day it is.)
        let mid = OffsetDateTime::from_unix_timestamp(now).unwrap().date();
        let mid = time::Date::from_calendar_date(Month::of(now).after().year, time::Month::try_from(Month::of(now).after().month).unwrap(), 15).unwrap_or(mid).midnight().assume_utc().unix_timestamp();
        group_events::merge(&mut l.items, &Events { raw: 1, items: vec![at("gone", mid)], more: false });
        let reads = [(None, vec![at("a", now - DAY), at("b", now + DAY), at("c", now + 2 * DAY)]), (Some(month), vec![]), (Some(month.after()), vec![])];
        for failing in [true, false] {
            l.again();
            l.begin(now);
            for (which, items) in reads.clone() {
                assert_eq!(l.due(), Some((which, 0)));
                if failing && which == Some(month) {
                    l.answer(which, 0, Reply::Failed(Failure::Offline));
                    break;
                }
                l.answer(which, 0, page(items, false));
            }
            assert_eq!(l.items.iter().any(|e| e.title == "gone"), failing);
            l.failed = None;
        }
        assert_eq!(l.items.len(), 3);
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
        let of_event = |told: &mut Told, e: &CalendarEvent| told.of(&ctx, false, e, group, "Makers", false);
        assert!(of_event(&mut told, &e).is_none(), "its page is not read yet");
        told.pages.insert(card.id.clone(), (of("grp_other"), None));
        assert!(of_event(&mut told, &e).is_none(), "another group's event of the same name and time");
        told.pages.insert(card.id.clone(), (of(group), None));
        assert!(of_event(&mut told, &e).is_some());
        assert!(of_event(&mut told, &at("Showcase", start + 3600)).is_none());
        assert_eq!(told.to_keep(std::slice::from_ref(&e), group, "Makers").pages.len(), 1);
        told.pages.clear();
        assert!(of_event(&mut told, &e).is_none());
        assert_eq!(told.to_keep(std::slice::from_ref(&e), group, "Makers"), Default::default(), "what is not certainly this group's is not kept");
    }

    fn entry(id: &str, entry: &str) -> Card {
        Card { id: id.into(), title: "Showcase".into(), calendar_id: Some(entry.into()), signed_up: Some(true), ..Default::default() }
    }

    /// By the calendar's own ids: the card says it at once, its page when it is read, and an entry Orbiters does not name any
    /// more takes both away; the other entries are left alone.
    #[test]
    fn the_calendars_entries_say_which_event_they_are() {
        let (group, start) = ("grp_00000000-0000-4000-8000-00000000a001", 1_795_286_400);
        let (a, b) = (at("a", start), at("b", start + 60));
        let ctx = egui::Context::default();
        let ids = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut told = Told::default();
        told.take_list(Listed::Entries(ids(&["cal_a", "cal_b"]), Ok(vec![entry("1", "cal_a"), entry("2", "cal_elsewhere")])));
        assert_eq!(told.cards.len(), 1, "only what was asked");
        let (page, _) = told.of(&ctx, false, &a, group, "Makers", false).expect("its card");
        assert_eq!((page.title.as_str(), page.signed_up), ("Showcase", Some(true)));
        assert!(told.of(&ctx, false, &b, group, "Makers", false).is_none());
        assert!(std::mem::take(&mut told.changed));
        let read = Overview { id: "1".into(), calendar_id: Some("cal_a".into()), signups: Some(4), ..Default::default() };
        told.take_page("1".into(), Some(Ok((read.clone(), None))));
        assert_eq!(told.of(&ctx, false, &a, group, "Makers", false).unwrap().0, read);
        let kept = told.to_keep(&[a.clone(), b.clone()], group, "Makers");
        assert_eq!((kept.cards.len(), kept.pages.len()), (1, 1));
        // A read that failed leaves what shows; one refused leaves the card.
        told.take_page("1".into(), Some(Err(anyhow::anyhow!("no network"))));
        told.take_list(Listed::Entries(ids(&["cal_a"]), Err(anyhow::anyhow!("no network"))));
        assert!(told.failed && told.of(&ctx, false, &a, group, "Makers", false).unwrap().0.signups == Some(4));
        told.take_page("1".into(), Some(Err(Refused { status: 403, code: None, message: String::new() }.into())));
        assert_eq!(told.of(&ctx, false, &a, group, "Makers", false).unwrap().0.signups, None);
        // Asked again: what another entry is stays, this one is none of Orbiters' any more.
        told.take_list(Listed::Entries(ids(&["cal_b"]), Ok(vec![entry("3", "cal_b")])));
        told.take_list(Listed::Entries(ids(&["cal_a"]), Ok(vec![])));
        assert!(told.of(&ctx, false, &a, group, "Makers", false).is_none() && told.of(&ctx, false, &b, group, "Makers", false).is_some());
        // A server from before: its list, matched by group and time.
        told.take_list(Listed::Entries(ids(&["cal_a"]), Err(Refused { status: 404, code: None, message: String::new() }.into())));
        assert!(told.by_time);
        told.again();
        assert!(!told.by_time && !told.failed, "asked again as if it knew, the next time");
    }

    /// What was kept shows before anything is asked, and the same is given to be kept again.
    #[test]
    fn kept_events_show_at_once() {
        let (group, now) = ("grp_00000000-0000-4000-8000-00000000a001", orbiters::now());
        let items = vec![at("old", now - 5 * DAY), at("soon", now + DAY)];
        let page = Overview { id: "1".into(), calendar_id: Some("cal_soon".into()), signups: Some(4), ..Default::default() };
        let told = group_events::Told { cards: vec![entry("1", "cal_soon")], pages: vec![(page, None)] };
        let mut l = EventList::default();
        l.as_kept(Some(items.clone()), Some(told.clone()));
        assert!(!l.started && l.reading.is_none());
        assert_eq!(l.shown(now), ["cal_soon", "cal_old"]);
        assert_eq!(l.told.to_keep(&l.items, group, "Makers"), told);
        assert!(l.told.of(&egui::Context::default(), false, &items[1], group, "Makers", false).is_some());
        l.again();
        assert_eq!(l.items, items, "what shows stays while it is read again");
        l.begin(now);
        assert_eq!(l.due(), Some((None, 0)));
        let mut nothing = EventList::default();
        nothing.as_kept(None, None);
        assert!(nothing.items.is_empty() && nothing.shown(now).is_empty());
    }

    #[test]
    fn only_orbiters_and_discord_links_open() {
        let page = |id: &str, url: Option<&str>| Overview { id: id.into(), url: url.map(str::to_string), ..Default::default() };
        assert_eq!(orbiters_page(false, &page("11111111-1111-4111-8111-111111111111", None)).unwrap(), "https://orbiters.cc/events/11111111-1111-4111-8111-111111111111");
        assert_eq!(orbiters_page(false, &page("1", Some("https://orbiters.cc/events/other"))).unwrap(), "https://orbiters.cc/events/other");
        assert_eq!(orbiters_page(false, &page("1", Some("https://orbiters.cc.example/events/1"))).unwrap(), "https://orbiters.cc/events/1", "the website's own pages only");
        assert!(orbiters_page(false, &page("../x", None)).is_none() && orbiters_page(false, &page("", Some("file:///x"))).is_none());
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
        // As the page itself says it, which needs no second question.
        let said = Overview { invites: Some(with("sent")), signed_up: Some(true), ..page.clone() };
        assert_eq!(invite_text(&said, None), Some("Your invite was sent in VRChat."));
        assert_eq!(invite_text(&Overview { signed_up: Some(true), ..page.clone() }, None), Some("You're on the invite list."));
        assert_eq!(invite_text(&Overview { signed_up: Some(false), ..Overview::default() }, None), None);
    }
}
