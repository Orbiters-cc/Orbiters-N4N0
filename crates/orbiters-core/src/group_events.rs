//! A group's events (REQUEST28): its VRChat calendar as `GET /calendar/{group}` gives it, read only, and which of Orbiters'
//! events each one is.
//!
//! Orbiters keeps the calendar entry it made for an event (`cal_…`) and says which of its events each entry is
//! ([`crate::account::Api::events_of_calendar`]). Against a server from before that, the two are told to be the same by the
//! group and the start: the same start, with the same title or the group's own name as the host ([`candidate`]), then, once
//! Orbiters' page of it is read, the very group ([`is_of`]). Never by the start alone.

use crate::community::{Card, Invites, Overview};
use crate::groups as g;
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// One event of a group's VRChat calendar.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: String,
    pub title: String,
    pub description: String,
    /// RFC 3339 as VRChat gave them; none when they are not.
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    /// `group` (its members) or `public`.
    pub access: String,
    /// VRChat's category, as it names it (`film_media`, …).
    pub category: String,
    /// `standalonewindows`, `android`, `ios`.
    pub platforms: Vec<String>,
    pub languages: Vec<String>,
    pub tags: Vec<String>,
    pub image_url: String,
    /// How many said they are interested, when VRChat says.
    pub interested: Option<u32>,
    /// The signed-in account is one of them, when VRChat says.
    pub following: Option<bool>,
    pub featured: bool,
    pub draft: bool,
    /// The group's roles it is for, when it is for some only.
    pub role_ids: Vec<String>,
    /// How often it repeats (`daily`, `weekly`, `monthly`, `yearly`), when it does.
    pub repeats: Option<String>,
}

fn unix(iso: Option<&str>) -> Option<i64> {
    OffsetDateTime::parse(iso?, &Rfc3339).ok().map(OffsetDateTime::unix_timestamp)
}

impl CalendarEvent {
    pub fn start(&self) -> Option<i64> {
        unix(self.starts_at.as_deref())
    }
    pub fn end(&self) -> Option<i64> {
        unix(self.ends_at.as_deref())
    }
    /// Over at `now` (unix seconds): past its end, or, with no end, half a day past its start.
    pub fn past(&self, now: i64) -> bool {
        self.end().or(self.start().map(|s| s + 12 * 3600)).is_some_and(|e| e <= now)
    }
}

/// A page of a group's calendar: what could be read of it, how many VRChat gave, and whether there is more after it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Events {
    pub items: Vec<CalendarEvent>,
    pub raw: usize,
    pub more: bool,
}

/// The months of a calendar, as VRChat is asked for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Month {
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
}

impl Month {
    pub fn of(unix: i64) -> Self {
        let t = OffsetDateTime::from_unix_timestamp(unix).unwrap_or(OffsetDateTime::UNIX_EPOCH);
        Self { year: t.year(), month: t.month() as u8 }
    }
    pub fn before(self) -> Self {
        if self.month <= 1 { Self { year: self.year - 1, month: 12 } } else { Self { month: self.month - 1, ..self } }
    }
    pub fn after(self) -> Self {
        if self.month >= 12 { Self { year: self.year + 1, month: 1 } } else { Self { month: self.month + 1, ..self } }
    }
    /// When it starts (Unix seconds, UTC).
    fn start(self) -> i64 {
        let month = time::Month::try_from(self.month.clamp(1, 12)).unwrap_or(time::Month::January);
        time::Date::from_calendar_date(self.year, month, 1).map_or(0, |d| d.midnight().assume_utc().unix_timestamp())
    }
    /// The date VRChat is asked the month of: its middle, the same month in every time zone.
    pub fn date(self) -> String {
        format!("{:04}-{:02}-15T12:00:00.000Z", self.year.clamp(2000, 9999), self.month.clamp(1, 12))
    }
}

/// Events of a calendar held at most, the latest.
pub const EVENTS_MAX: usize = 1000;

/// Events kept for the next start, the latest.
pub const EVENTS_KEPT: usize = 300;

/// Takes a page of a calendar into `list`, soonest first: each event once, as VRChat last gave it. How many are new.
pub fn merge(list: &mut Vec<CalendarEvent>, page: &Events) -> usize {
    let mut new = 0;
    for e in &page.items {
        match list.iter().position(|x| x.id == e.id) {
            Some(i) => list[i] = e.clone(),
            None => {
                list.push(e.clone());
                new += 1;
            }
        }
    }
    list.sort_by(|a, b| (a.start().unwrap_or(i64::MAX), &a.id).cmp(&(b.start().unwrap_or(i64::MAX), &b.id)));
    let over = list.len().saturating_sub(EVENTS_MAX);
    list.drain(..over);
    new
}

/// Takes out of `list` what `seen` does not have of `month` and the one after, once all that VRChat gives of them was read:
/// deleted or moved away. Their first and last day are left alone (VRChat's month may be another time zone's), and every other
/// month is. Whether anything went.
pub fn prune(list: &mut Vec<CalendarEvent>, month: Month, seen: &std::collections::HashSet<String>) -> bool {
    let (from, to, before) = (month.start() + 86_400, month.after().after().start() - 86_400, list.len());
    list.retain(|e| seen.contains(&e.id) || !e.start().is_some_and(|s| s >= from && s < to));
    list.len() != before
}

/// What Orbiters says of a group's events, as kept to show at the next start: the cards of those that are its own, and the
/// pages read of them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Told {
    pub cards: Vec<Card>,
    pub pages: Vec<(Overview, Option<Invites>)>,
}

impl Told {
    /// How many of each are kept, and how large it is at most (bytes).
    pub const MAX: usize = 200;
    const BYTES: usize = 2 << 20;
    pub fn fits(&self) -> bool {
        self.cards.len() <= Self::MAX && self.pages.len() <= Self::MAX && serde_json::to_vec(self).is_ok_and(|v| v.len() <= Self::BYTES)
    }
}

/// A calendar event's page on vrchat.com.
pub fn web_page(group: &str, event: &str) -> Option<String> {
    (g::is_group_id(group) && is_event_id(event)).then(|| format!("https://vrchat.com/home/group/{group}/calendar/{event}"))
}

pub fn is_event_id(s: &str) -> bool {
    s.len() <= 64 && s.strip_prefix("cal_").is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Which of Orbiters' listed events this one may be: the one made as this very calendar entry, else the only one that
/// starts with it and has its title, else the only one that starts with it and is hosted by a community of the group's
/// name. Nothing is certain before its page says whose it is ([`is_of`]).
pub fn candidate<'a>(event: &CalendarEvent, group_name: &str, cards: &'a [Card]) -> Option<&'a Card> {
    if let Some(c) = cards.iter().find(|c| c.calendar_id.as_deref() == Some(event.id.as_str())) {
        return Some(c);
    }
    let start = event.start()?;
    let same: Vec<&Card> = cards.iter().filter(|c| c.calendar_id.is_none() && unix(c.starts_at.as_deref()).is_some_and(|s| (s - start).abs() < 60)).collect();
    let only = |found: Vec<&'a Card>| if found.len() == 1 { found.first().copied() } else { None };
    let titled: Vec<&Card> = same.iter().copied().filter(|c| words(&c.title) == words(&event.title)).collect();
    if !titled.is_empty() {
        // The same title twice at once: hosted by this group, if that tells them apart.
        let here: Vec<&Card> = titled.iter().copied().filter(|c| hosted(c, group_name)).collect();
        return only(titled).or_else(|| only(here));
    }
    only(same.into_iter().filter(|c| hosted(c, group_name)).collect())
}

fn hosted(c: &Card, group_name: &str) -> bool {
    !group_name.trim().is_empty() && c.community.as_ref().is_some_and(|h| words(&h.name) == words(group_name))
}

/// Whether an event's page on Orbiters is this calendar event's: made as this very entry, or hosted by this very group with
/// the same start (its title may have been changed on either side).
pub fn is_of(event: &CalendarEvent, group: &str, page: &Overview) -> bool {
    if let Some(id) = page.calendar_id.as_deref() {
        return id == event.id;
    }
    let same_start = match (event.start(), unix(page.starts_at.as_deref())) {
        (Some(a), Some(b)) => (a - b).abs() < 60,
        _ => false,
    };
    same_start && page.community.as_ref().and_then(|c| c.id.as_deref()) == Some(group)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::community::Host;

    const G: &str = "grp_00000000-0000-4000-8000-00000000a001";

    fn event() -> CalendarEvent {
        CalendarEvent { id: "cal_1".into(), title: "Saturday  Showcase".into(), starts_at: Some("2026-11-28T20:00:00.000Z".into()), ends_at: Some("2026-11-28T22:00:00.000Z".into()), ..Default::default() }
    }
    fn card(id: &str, title: &str, starts: &str, host: &str) -> Card {
        Card { id: id.into(), title: title.into(), starts_at: Some(starts.into()), community: (!host.is_empty()).then(|| Host { name: host.into(), ..Default::default() }), ..Default::default() }
    }

    #[test]
    fn an_orbiters_event_is_found_by_its_entry_else_by_start_and_title_or_host() {
        let e = event();
        let same = "2026-11-28T21:00:00+01:00";
        let found = |cards: &[Card]| candidate(&e, "Made-up Makers", cards).map(|c| c.id.clone());
        assert_eq!(found(&[card("a", "saturday showcase", same, "")]).as_deref(), Some("a"), "the same instant and words");
        assert_eq!(found(&[card("a", "Another night", same, "made-up makers")]).as_deref(), Some("a"), "renamed on one side, the group's own");
        assert_eq!(found(&[card("a", "Another night", same, "Elsewhere")]), None, "a start alone is nobody's");
        assert_eq!(found(&[card("a", "Saturday Showcase", "2026-11-28T20:30:00Z", "Made-up Makers")]), None, "another start");
        assert_eq!(found(&[card("a", "Saturday Showcase", same, ""), card("b", "Saturday Showcase", same, "Made-up Makers")]).as_deref(), Some("b"));
        assert_eq!(found(&[card("a", "Saturday Showcase", same, ""), card("b", "Saturday Showcase", same, "")]), None, "two of them: neither");
        let linked = Card { calendar_id: Some("cal_1".into()), ..card("c", "Renamed", "2026-12-01T20:00:00Z", "") };
        assert_eq!(found(&[card("a", "Saturday Showcase", same, "Made-up Makers"), linked]).as_deref(), Some("c"), "the entry itself says it");
        let other = Card { calendar_id: Some("cal_2".into()), ..card("d", "Saturday Showcase", same, "Made-up Makers") };
        assert_eq!(found(&[other]), None, "another entry is not this one");
    }

    #[test]
    fn its_page_says_whose_it_is() {
        let e = event();
        let page = |group: Option<&str>, starts: &str| Overview { starts_at: Some(starts.into()), community: Some(Host { id: group.map(str::to_string), ..Default::default() }), ..Default::default() };
        assert!(is_of(&e, G, &page(Some(G), "2026-11-28T20:00:00Z")));
        assert!(!is_of(&e, G, &page(Some("grp_other"), "2026-11-28T20:00:00Z")) && !is_of(&e, G, &page(None, "2026-11-28T20:00:00Z")));
        assert!(!is_of(&e, G, &page(Some(G), "2026-11-28T20:05:00Z")));
        assert!(is_of(&e, G, &Overview { calendar_id: Some("cal_1".into()), ..Default::default() }));
        assert!(!is_of(&e, G, &Overview { calendar_id: Some("cal_2".into()), ..page(Some(G), "2026-11-28T20:00:00Z") }));
    }

    #[test]
    fn months_go_back_and_forth_over_years() {
        let m = Month::of(1_795_286_400);
        assert_eq!((m.year, m.month), (2026, 11));
        let january = Month { year: 2027, month: 1 };
        assert_eq!((january.before(), Month { year: 2026, month: 12 }.after()), (Month { year: 2026, month: 12 }, january));
        assert_eq!(january.date(), "2027-01-15T12:00:00.000Z");
    }

    /// Each event once, as last given, whatever page it came in; what a whole look at two months did not give goes from those
    /// two alone.
    #[test]
    fn pages_are_taken_into_the_list_and_what_is_gone_goes() {
        let at = |id: &str, start: &str| CalendarEvent { id: id.into(), title: id.into(), starts_at: Some(start.into()), ..Default::default() };
        let page = |items: Vec<CalendarEvent>| Events { raw: items.len(), items, more: false };
        let ids = |l: &[CalendarEvent]| l.iter().map(|e| e.id.clone()).collect::<Vec<_>>().join(" ");
        let mut l = Vec::new();
        let all = vec![at("d", "2026-12-04T20:00:00Z"), at("b", "2026-11-10T20:00:00Z"), at("a", "2026-10-10T20:00:00Z"), at("c", "2026-11-20T20:00:00Z"), at("e", "2026-12-31T23:30:00Z"), at("f", "2027-01-05T20:00:00Z")];
        assert_eq!(merge(&mut l, &page(all)), 6);
        assert_eq!(ids(&l), "a b c d e f", "soonest first");
        assert_eq!(merge(&mut l, &page(vec![at("b", "2026-11-11T20:00:00Z"), at("g", "2026-11-12T20:00:00Z")])), 1);
        assert_eq!(merge(&mut l, &page(vec![])), 0);
        assert_eq!(ids(&l), "a b g c d e f", "nothing goes by a page alone");
        assert_eq!(l[1].starts_at.as_deref(), Some("2026-11-11T20:00:00Z"), "moved, not twice");
        let november = Month { year: 2026, month: 11 };
        let seen = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<std::collections::HashSet<_>>();
        assert!(prune(&mut l, november, &seen(&["b", "g"])));
        assert_eq!(ids(&l), "a b g e f", "the last day and the other months left alone");
        assert!(!prune(&mut l, november, &seen(&["b", "g"])));
        let many = (0..EVENTS_MAX + 5).map(|i| at(&format!("m{i:05}"), &format!("2027-02-01T{:02}:{:02}:00Z", i / 60 % 24, i % 60))).collect();
        merge(&mut l, &page(many));
        assert_eq!(l.len(), EVENTS_MAX, "the latest");
        assert!(!l.iter().any(|e| e.id == "a"));
        assert!(Told::default().fits() && !Told { cards: vec![Card::default(); Told::MAX + 1], ..Default::default() }.fits());
    }

    #[test]
    fn past_is_past_the_end() {
        let e = event();
        let end = e.end().unwrap();
        assert!(!e.past(end - 1) && e.past(end));
        let open = CalendarEvent { ends_at: None, ..event() };
        assert!(!open.past(end) && open.past(open.start().unwrap() + 12 * 3600));
        assert!(!CalendarEvent::default().past(end), "no date known");
        assert_eq!(web_page(G, "cal_1").unwrap(), format!("https://vrchat.com/home/group/{G}/calendar/cal_1"));
        assert!(web_page(G, "cal_../x").is_none() && web_page("grp_/", "cal_1").is_none());
    }
}
