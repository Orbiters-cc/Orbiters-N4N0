//! The friends list as rows, without drawing, in VRCNext's sections (REQUEST20 3, Enzo: "the online friends (actively in
//! the game), the web active friends and the offline friends to be separated, in the same exact way it is in VRCNext"):
//! first the favourites who are in the game (one section, split by favourite group when two or more groups have someone
//! in it), then the others in the game, those active on the website (or the companion app) but not in the game, and the
//! offline ones. A favourite who is not in the game is in their presence section only, and nobody shows twice. Each
//! section is a header with its count and its people unless folded (remembered; Offline starts folded, as VRCNext's
//! does); a section with nobody says nothing. A search keeps the people it finds in the sections they are in, opens the
//! folded ones and drops the sections left empty. Only the rows in view are drawn ([`visible`]); a refresh keeps the
//! first row in view where it was ([`anchored`]). The boops and invite requests received come first, rows of the same
//! list ([`with_received`]): one list, one scroll (REQUEST18 1).

use orbiters_core::launcher::{Friend, FriendGroup, GroupKind, InviteKind, LauncherState, Presence, Whereabouts};
use std::collections::{HashMap, HashSet};
use std::ops::Range;

/// A section's header and a person's row.
pub const HEADER_H: f32 = 34.0;
pub const PERSON_H: f32 = 58.0;
/// The heading over two or more received rows (what they are, how many), and one received row (a boop, an invite request)
/// with the gap under it.
pub const GOT_HEAD_H: f32 = 28.0;
pub const GOT_H: f32 = 54.0;

/// The sections' keys (what a fold remembers): a favourite group's own is its id (`group_0`…).
pub const FAVOURITES: &str = "favourites";
pub const OTHER_FAVOURITES: &str = "favourites-other";
pub const IN_GAME: &str = "in-game";
pub const WEBSITE: &str = "website";
pub const OFFLINE: &str = "offline";

/// What a section is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// The favourites in the game, all their groups together.
    Favourites,
    /// One favourite group's people in the game, under [`Section::Favourites`] when two or more groups have someone.
    FavouriteGroup,
    /// Favourites in the game whose group the account no longer lists (VRCNext's "Other").
    OtherFavourites,
    InGame,
    Website,
    Offline,
}

impl Section {
    /// Drawn indented, inside the favourites.
    pub fn nested(self) -> bool {
        matches!(self, Section::FavouriteGroup | Section::OtherFavourites)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Header {
        /// The section's key ([`FAVOURITES`], a favourite group's id, [`IN_GAME`]…): what folds it.
        group: String,
        section: Section,
        /// The favourite group's own name (empty for the others).
        name: String,
        /// People in it (found by the search, while one is typed).
        count: usize,
        folded: bool,
    },
    Person {
        /// The key of the section the row is in.
        group: String,
        /// Index in [`LauncherState::friends`].
        friend: usize,
    },
    /// Over two or more received rows: how many, and how many of them are boops.
    GotHead { count: usize, boops: usize },
    /// A boop or an invite request received, by its notification id.
    Got { id: String },
}

impl Row {
    pub fn height(&self) -> f32 {
        match self {
            Row::Header { .. } => HEADER_H,
            Row::Person { .. } => PERSON_H,
            Row::GotHead { .. } => GOT_HEAD_H,
            Row::Got { .. } => GOT_H,
        }
    }

    /// What tells a row from the others across refreshes.
    pub fn key(&self, state: &LauncherState) -> String {
        match self {
            Row::Header { group, .. } => format!("h:{group}"),
            Row::Person { group, friend } => format!("p:{group}:{}", state.friends.get(*friend).map_or("", |f| f.id.as_str())),
            Row::GotHead { .. } => "gh".into(),
            Row::Got { id } => format!("g:{id}"),
        }
    }
}

/// Whether `friend` is what `query` (already lower case, trimmed) looks for: their name, status line, the world
/// they are in, or the account's note about them.
pub fn matches(friend: &Friend, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let has = |s: &str| s.to_lowercase().contains(query);
    has(&friend.name)
        || has(&friend.status_text)
        || has(&friend.note)
        || matches!(&friend.whereabouts, Whereabouts::World(place) if has(&place.world_name))
}

/// Whether the list knows who is in the game: a copy without presence (VRCX's database) does not, and then the
/// favourites hold every favourite (else they would all be "offline" and the favourites empty).
fn knows_presence(state: &LauncherState) -> bool {
    !state.friends.iter().all(|f| f.whereabouts == Whereabouts::Unknown)
}

/// The rows of the list for `query` with the sections in `folded` closed.
pub fn rows(state: &LauncherState, query: &str, folded: &HashSet<String>) -> Vec<Row> {
    let query = query.trim().to_lowercase();
    let searching = !query.is_empty();
    let is_folded = |key: &str| !searching && folded.contains(key);
    let index: HashMap<&str, usize> = state.friends.iter().enumerate().map(|(i, f)| (f.id.as_str(), i)).collect();
    let presence_known = knows_presence(state);
    let favourite_groups: Vec<&FriendGroup> = state.groups.iter().filter(|g| g.kind == GroupKind::Favourite).collect();
    // A favourite shows among the favourites while in the game (VRCNext), under the first of their groups the account lists.
    let in_favourites = |f: &Friend| !f.favourite_groups.is_empty() && (f.presence == Presence::InGame || !presence_known);
    let home = |f: &Friend| f.favourite_groups.iter().find(|g| favourite_groups.iter().any(|fg| fg.id == **g)).cloned();
    let mut seen: HashSet<usize> = HashSet::new();
    let mut subs: Vec<(&FriendGroup, Vec<usize>)> = Vec::new();
    for group in &favourite_groups {
        let people: Vec<usize> = group
            .members
            .iter()
            .filter_map(|id| index.get(id.as_str()).copied())
            .filter(|&i| {
                let f = &state.friends[i];
                in_favourites(f) && home(f).as_deref() == Some(group.id.as_str()) && matches(f, &query)
            })
            .filter(|&i| seen.insert(i))
            .collect();
        if !people.is_empty() {
            subs.push((group, people));
        }
    }
    // Favourites of a group the account does not list (any more), in the presence sections' order.
    let other: Vec<usize> = state
        .groups
        .iter()
        .filter(|g| g.kind != GroupKind::Favourite)
        .flat_map(|g| g.members.iter())
        .filter_map(|id| index.get(id.as_str()).copied())
        .filter(|&i| in_favourites(&state.friends[i]) && home(&state.friends[i]).is_none() && matches(&state.friends[i], &query))
        .filter(|&i| seen.insert(i))
        .collect();
    let mut out = Vec::new();
    let header = |group: &str, section, name: &str, count| Row::Header { group: group.into(), section, name: name.into(), count, folded: is_folded(group) };
    let total = subs.iter().map(|s| s.1.len()).sum::<usize>() + other.len();
    if total > 0 {
        out.push(header(FAVOURITES, Section::Favourites, "", total));
        if !is_folded(FAVOURITES) {
            let person = |group: &str, friend| Row::Person { group: group.into(), friend };
            if subs.len() > 1 {
                for (group, people) in &subs {
                    out.push(header(&group.id, Section::FavouriteGroup, &group.name, people.len()));
                    if !is_folded(&group.id) {
                        out.extend(people.iter().map(|&i| person(&group.id, i)));
                    }
                }
                if !other.is_empty() {
                    out.push(header(OTHER_FAVOURITES, Section::OtherFavourites, "", other.len()));
                    if !is_folded(OTHER_FAVOURITES) {
                        out.extend(other.iter().map(|&i| person(OTHER_FAVOURITES, i)));
                    }
                }
            } else {
                out.extend(subs.iter().flat_map(|s| s.1.iter()).chain(&other).map(|&i| person(FAVOURITES, i)));
            }
        }
    }
    for (key, kind, section) in [(IN_GAME, GroupKind::InGame, Section::InGame), (WEBSITE, GroupKind::Website, Section::Website), (OFFLINE, GroupKind::Offline, Section::Offline)] {
        let found: Vec<usize> = state
            .groups
            .iter()
            .filter(|g| g.kind == kind)
            .flat_map(|g| g.members.iter())
            .filter_map(|id| index.get(id.as_str()).copied())
            .filter(|&i| !seen.contains(&i) && matches(&state.friends[i], &query))
            .collect();
        if found.is_empty() {
            continue;
        }
        out.push(header(key, section, "", found.len()));
        if !is_folded(key) {
            out.extend(found.into_iter().map(|friend| Row::Person { group: key.into(), friend }));
        }
    }
    out
}

/// The boops and invite requests received and not put away, newest first, by id, with whether each is a boop: every one,
/// never a count of the rest. While a search is typed, those from someone it finds.
pub fn received(state: &LauncherState, dismissed: &HashSet<String>, query: &str) -> Vec<(String, bool)> {
    let query = query.trim().to_lowercase();
    let found = |name: &str| query.is_empty() || name.to_lowercase().contains(&query);
    let mut got: Vec<(i64, String, bool)> = Vec::new();
    for b in state.boops.iter().filter(|b| !dismissed.contains(&b.id) && found(&b.from_name)) {
        got.push((b.received_at, b.id.clone(), true));
    }
    for i in state.invites.iter().filter(|i| i.kind == InviteKind::Request && !dismissed.contains(&i.id) && found(&i.from_name)) {
        got.push((i.received_at, i.id.clone(), false));
    }
    got.sort_by_key(|g| std::cmp::Reverse(g.0));
    got.into_iter().map(|(_, id, boop)| (id, boop)).collect()
}

/// The whole list (REQUEST18 1, Enzo: "the whole thing is scrollable", never a scrolling frame inside it): what was
/// received first, every one a row (under a heading when there are two or more), then the sections and their people.
pub fn with_received(state: &LauncherState, query: &str, folded: &HashSet<String>, dismissed: &HashSet<String>) -> Vec<Row> {
    let got = received(state, dismissed, query);
    let mut out = Vec::with_capacity(got.len() + 1);
    if got.len() > 1 {
        out.push(Row::GotHead { count: got.len(), boops: got.iter().filter(|g| g.1).count() });
    }
    out.extend(got.into_iter().map(|(id, _)| Row::Got { id }));
    out.extend(rows(state, query, folded));
    out
}

/// The tops of the rows (one more: the list's height).
pub fn tops(rows: &[Row]) -> Vec<f32> {
    let mut y = 0.0;
    let mut out = Vec::with_capacity(rows.len() + 1);
    for row in rows {
        out.push(y);
        y += row.height();
    }
    out.push(y);
    out
}

/// The rows that meet the band `from..to` of the list (its scroll offset and the view's height): the only ones drawn.
pub fn visible(tops: &[f32], from: f32, to: f32) -> Range<usize> {
    let count = tops.len().saturating_sub(1);
    // The first row whose bottom is below `from`, then up to the first whose top is at or past `to`.
    let start = tops[1..].partition_point(|&bottom| bottom <= from).min(count);
    let end = (start + tops[start..count].partition_point(|&top| top < to)).min(count);
    start..end
}

/// The scroll offset that keeps `anchor` (a row's key and how far the view's top is into it) where it was after the
/// rows changed: people arriving or leaving above the view do not move what the person looks at. `None`: the row is
/// gone, the offset stays.
pub fn anchored(new_keys: &[String], new_tops: &[f32], anchor: &(String, f32)) -> Option<f32> {
    let i = new_keys.iter().position(|k| *k == anchor.0)?;
    Some(new_tops[i] + anchor.1)
}

/// The row at the view's top and how far into it the view starts.
pub fn anchor_at(keys: &[String], tops: &[f32], offset: f32) -> Option<(String, f32)> {
    let i = visible(tops, offset, offset + 1.0).start;
    keys.get(i).map(|k| (k.clone(), offset - tops[i]))
}

/// The counts the header gives: in the game, on the website, everyone.
pub fn counts(state: &LauncherState) -> (usize, usize, usize) {
    use orbiters_core::launcher::Presence;
    let n = |p: Presence| state.friends.iter().filter(|f| f.presence == p).count();
    (n(Presence::InGame), n(Presence::Website), state.friends.len())
}

/// The favourite groups only (what a person's modal says they are in).
pub fn favourite_names<'a>(state: &'a LauncherState, friend: &Friend) -> Vec<&'a str> {
    state
        .groups
        .iter()
        .filter(|g: &&FriendGroup| g.kind == GroupKind::Favourite && friend.favourite_groups.contains(&g.id))
        .map(|g| g.name.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::launcher::{sample, Presence};

    fn people(rows: &[Row], state: &LauncherState) -> Vec<String> {
        rows.iter()
            .map(|r| match r {
                Row::Header { group, count, .. } => format!("[{group} {count}]"),
                Row::Person { friend, .. } => state.friends[*friend].name.clone(),
                Row::GotHead { count, .. } => format!("<{count} received>"),
                Row::Got { id } => format!("<{id}>"),
            })
            .collect()
    }

    fn headers(rows: &[Row]) -> Vec<(String, Section, usize)> {
        rows.iter().filter_map(|r| if let Row::Header { group, section, count, .. } = r { Some((group.clone(), *section, *count)) } else { None }).collect()
    }

    /// A friend of the sample, changed.
    fn one(state: &mut LauncherState, n: usize, presence: Presence, whereabouts: Whereabouts, groups: &[&str]) {
        let f = &mut state.friends[n];
        f.presence = presence;
        f.whereabouts = whereabouts;
        f.favourite_groups = groups.iter().map(|g| g.to_string()).collect();
        let id = f.id.clone();
        for g in &mut state.groups {
            g.members.retain(|m| *m != id);
            let here = match g.kind {
                GroupKind::Favourite => groups.contains(&g.id.as_str()),
                GroupKind::InGame => presence == Presence::InGame,
                GroupKind::Website => presence == Presence::Website,
                GroupKind::Offline => presence == Presence::Offline,
            };
            if here {
                g.members.push(id.clone());
            }
        }
    }

    /// REQUEST20 3, as VRCNext's friends list: the favourites in the game (by group when two or more groups have someone),
    /// then the others in the game, on the website, offline; a favourite not in the game only in their presence section.
    #[test]
    fn sections_are_vrcnext_s_favourites_in_game_then_in_game_website_offline() {
        let state = sample();
        let rows = rows(&state, "", &HashSet::new());
        assert_eq!(
            people(&rows, &state),
            [
                "[favourites 3]", "[group_0 2]", "Nova Sample", "Pixel Fox", "[group_1 1]", "Mochi Byte",
                "[in-game 1]", "Orbit Tester", "[website 1]", "Lumen", "[offline 2]", "Quiet Kite", "Sample Seven"
            ]
        );
        let sections: Vec<Section> = headers(&rows).into_iter().map(|h| h.1).collect();
        assert_eq!(sections, [Section::Favourites, Section::FavouriteGroup, Section::FavouriteGroup, Section::InGame, Section::Website, Section::Offline]);
        // Pixel Fox is in both groups: under the first the account lists, once. Quiet Kite (a favourite, offline) is in Offline only.
        let shown: Vec<usize> = rows.iter().filter_map(|r| if let Row::Person { friend, .. } = r { Some(*friend) } else { None }).collect();
        assert_eq!(shown.len(), state.friends.len(), "everyone once");
        assert_eq!(shown.iter().collect::<HashSet<_>>().len(), state.friends.len(), "nobody twice");
        // The names a favourite group header shows.
        assert!(matches!(&rows[1], Row::Header { name, .. } if name == "best people"));
    }

    /// One favourite group with someone in the game: one Favourites section, its people right under it (VRCNext).
    #[test]
    fn one_group_in_the_game_is_one_favourites_section() {
        let mut state = sample();
        let there = state.friends[2].whereabouts.clone();
        one(&mut state, 2, Presence::InGame, there, &["group_0"]);
        let shown = rows(&state, "", &HashSet::new());
        assert_eq!(people(&shown, &state)[..4], ["[favourites 3]", "Nova Sample", "Pixel Fox", "Mochi Byte"]);
        assert!(!shown.iter().any(|r| matches!(r, Row::Header { section: Section::FavouriteGroup, .. })));
        // Nobody of them in the game: no Favourites at all, they are in their sections.
        let mut state = sample();
        for n in [0, 1, 2] {
            one(&mut state, n, Presence::Website, Whereabouts::Website, &["group_0"]);
        }
        let shown = rows(&state, "", &HashSet::new());
        assert_eq!(headers(&shown).iter().map(|h| h.0.as_str()).collect::<Vec<_>>(), ["in-game", "website", "offline"]);
        assert_eq!(headers(&shown)[1].2, 4);
    }

    /// Every presence a friend can have puts them in one section, by these rules.
    #[test]
    fn every_friend_state_has_its_section() {
        let place = match &sample().friends[0].whereabouts {
            Whereabouts::World(p) => p.clone(),
            _ => unreachable!(),
        };
        let cases: [(Presence, Whereabouts, &[&str], &str); 9] = [
            (Presence::InGame, Whereabouts::World(place.clone()), &[], IN_GAME),
            (Presence::InGame, Whereabouts::Private, &[], IN_GAME),
            (Presence::InGame, Whereabouts::Traveling, &[], IN_GAME),
            (Presence::Website, Whereabouts::Website, &[], WEBSITE),
            (Presence::Offline, Whereabouts::Offline, &[], OFFLINE),
            (Presence::InGame, Whereabouts::Private, &["group_0"], FAVOURITES),
            (Presence::Website, Whereabouts::Website, &["group_0"], WEBSITE),
            (Presence::Offline, Whereabouts::Offline, &["group_0"], OFFLINE),
            // A favourite group the account no longer lists.
            (Presence::InGame, Whereabouts::World(place), &["group_9"], FAVOURITES),
        ];
        for (presence, whereabouts, groups, expected) in cases {
            let mut state = sample();
            one(&mut state, 6, presence, whereabouts.clone(), groups);
            let rows = rows(&state, "", &HashSet::new());
            let at: Vec<&str> = rows.iter().filter_map(|r| match r {
                Row::Person { group, friend: 6 } => Some(group.as_str()),
                _ => None,
            }).collect();
            // Under the favourites, split by group here (two groups have someone): its group's key, or the others'.
            let expected = match (expected, groups.first()) {
                (FAVOURITES, Some(&"group_9")) => OTHER_FAVOURITES,
                (FAVOURITES, Some(g)) => *g,
                (e, _) => e,
            };
            assert_eq!(at, [expected], "{presence:?} {whereabouts:?} {groups:?}");
        }
        // A copy that knows nobody's presence (VRCX's database): the favourites hold every favourite.
        let mut state = sample();
        for n in 0..state.friends.len() {
            let groups: Vec<String> = state.friends[n].favourite_groups.clone();
            let groups: Vec<&str> = groups.iter().map(String::as_str).collect();
            one(&mut state, n, Presence::Offline, Whereabouts::Unknown, &groups);
        }
        let rows = rows(&state, "", &HashSet::new());
        assert_eq!(headers(&rows).iter().map(|h| (h.0.as_str(), h.2)).collect::<Vec<_>>(), [("favourites", 4), ("group_0", 2), ("group_1", 2), ("offline", 3)]);
    }

    /// The header's line ("9 in game · 24 on the website · 224 friends") says what the sections hold.
    #[test]
    fn the_counts_line_agrees_with_the_sections() {
        let mut state = sample();
        one(&mut state, 5, Presence::InGame, Whereabouts::Private, &["group_1"]);
        let rows = rows(&state, "", &HashSet::new());
        let count = |s: Section| headers(&rows).iter().filter(|h| h.1 == s).map(|h| h.2).sum::<usize>();
        let (in_game, web, all) = counts(&state);
        assert_eq!(count(Section::Favourites) + count(Section::InGame), in_game);
        assert_eq!(count(Section::Website), web);
        assert_eq!(count(Section::Favourites) + count(Section::InGame) + count(Section::Website) + count(Section::Offline), all);
        // The groups inside the favourites add up to it.
        assert_eq!(count(Section::FavouriteGroup) + count(Section::OtherFavourites), count(Section::Favourites));
    }

    #[test]
    fn a_folded_section_keeps_its_header_and_count_and_hides_its_people() {
        let state = sample();
        let folded: HashSet<String> = [OFFLINE.to_string(), "group_0".to_string()].into();
        assert_eq!(
            people(&rows(&state, "", &folded), &state),
            ["[favourites 3]", "[group_0 2]", "[group_1 1]", "Mochi Byte", "[in-game 1]", "Orbit Tester", "[website 1]", "Lumen", "[offline 2]"]
        );
        let all = rows(&state, "", &folded);
        assert!(matches!(all.last(), Some(Row::Header { folded: true, count: 2, section: Section::Offline, .. })));
        // The favourites folded: their groups go with them.
        let folded: HashSet<String> = [FAVOURITES.to_string()].into();
        assert_eq!(people(&rows(&state, "", &folded), &state)[..3], ["[favourites 3]", "[in-game 1]", "Orbit Tester"]);
    }

    #[test]
    fn empty_sections_say_nothing() {
        let mut state = sample();
        one(&mut state, 4, Presence::Offline, Whereabouts::Offline, &[]);
        let rows = rows(&state, "", &HashSet::new());
        assert!(!rows.iter().any(|r| matches!(r, Row::Header { section: Section::Website, .. })));
        assert!(!rows.iter().any(|r| matches!(r, Row::Header { count: 0, .. })));
    }

    #[test]
    fn search_finds_names_status_worlds_and_notes_across_sections_and_opens_folded_ones() {
        let state = sample();
        let mut folded: HashSet<String> = state.groups.iter().map(|g| g.id.clone()).collect();
        folded.insert(FAVOURITES.into());
        assert_eq!(people(&rows(&state, "  MOCHI ", &folded), &state), ["[favourites 1]", "Mochi Byte"]);
        // The world someone is in.
        assert_eq!(people(&rows(&state, "café", &folded), &state), ["[favourites 1]", "Mochi Byte"]);
        // Their status line, the account's note; one in every section.
        assert!(people(&rows(&state, "unity", &folded), &state).contains(&"Orbit Tester".to_string()));
        assert!(people(&rows(&state, "meetup", &folded), &state).contains(&"Pixel Fox".to_string()));
        assert_eq!(people(&rows(&state, "e", &folded), &state).iter().filter(|r| r.starts_with('[')).count(), 6, "{:?}", people(&rows(&state, "e", &folded), &state));
        assert!(rows(&state, "nobody by this name", &folded).is_empty());
    }

    /// Exactly one scrolling region in the panel (Enzo 2026-11-20, never a scroll area inside another): the list's own,
    /// whose rows are the received ones, the section headers and the people.
    #[test]
    fn the_friends_panel_has_one_scroll_region() {
        let scrolls = |source: &str| source.matches("ScrollArea::").count();
        assert_eq!(scrolls(include_str!("row.rs")), 1, "the list");
        assert_eq!(scrolls(include_str!("list.rs")), 1, "rows only (the one here is this test's own)");
        // The column draws the head, then that list: mod.rs has none (the profile window and the account's own are over the page).
        assert_eq!(scrolls(include_str!("mod.rs")), 0);
    }

    /// REQUEST18 1: every boop and invite request received is a row of the one list, newest first, before the sections;
    /// what was put away is not; a search keeps those from someone it finds.
    #[test]
    fn what_was_received_is_the_first_rows_of_the_one_list() {
        let mut state = sample();
        let from = state.friends[2].clone();
        // Newer than the sample's invite request, which comes after them.
        let newest = state.invites.iter().map(|i| i.received_at).max().unwrap_or(0) + 1_000;
        state.boops = (0..30)
            .map(|i| orbiters_core::launcher::Boop { id: format!("not_boop_{i:02}"), from_id: from.id.clone(), from_name: from.name.clone(), from_icon: String::new(), emoji: None, received_at: newest - i })
            .collect();
        let requests = state.invites.iter().filter(|i| i.kind == InviteKind::Request).count();
        assert!(requests > 0, "the sample has an invite request");
        let dismissed: HashSet<String> = ["not_boop_05".to_string()].into();
        let all = with_received(&state, "", &HashSet::new(), &dismissed);
        let got = 29 + requests;
        assert_eq!(all[0], Row::GotHead { count: got, boops: 29 });
        assert!(all[1..=got].iter().all(|r| matches!(r, Row::Got { .. })), "every one a row, none folded into a count");
        assert!(!all.contains(&Row::Got { id: "not_boop_05".into() }), "put away");
        assert_eq!(all[1], Row::Got { id: "not_boop_00".into() }, "the newest first");
        // Then the sections, as they are without anything received: one list.
        assert_eq!(all[got + 1..], rows(&state, "", &HashSet::new())[..]);
        let t = tops(&all);
        assert_eq!(t[got + 1], GOT_HEAD_H + got as f32 * GOT_H, "the friends start right under the last received row");
        // One received: its row alone, no heading. A search: those from whom it finds.
        state.boops.truncate(1);
        state.invites.retain(|i| i.kind != InviteKind::Request);
        assert_eq!(with_received(&state, "", &HashSet::new(), &HashSet::new())[0], Row::Got { id: "not_boop_00".into() });
        assert!(matches!(with_received(&state, &from.name.to_uppercase(), &HashSet::new(), &HashSet::new())[0], Row::Got { .. }));
        assert!(!with_received(&state, "nobody by this name", &HashSet::new(), &HashSet::new()).iter().any(|r| matches!(r, Row::Got { .. })));
    }

    /// 400 friends: only the rows in the view are drawn, whatever the scroll.
    #[test]
    fn only_the_rows_in_view_are_drawn() {
        let mut state = sample();
        let template = state.friends[6].clone();
        for i in 0..400 {
            let mut f = template.clone();
            f.id = format!("usr_many_{i}");
            f.name = format!("Many {i}");
            state.groups.iter_mut().find(|g| g.id == OFFLINE).unwrap().members.push(f.id.clone());
            state.friends.push(f);
        }
        let rows = rows(&state, "", &HashSet::new());
        assert!(rows.len() > 400);
        let tops = tops(&rows);
        let view = 700.0;
        for offset in [0.0, 1234.5, tops[tops.len() - 1] - view] {
            let range = visible(&tops, offset, offset + view);
            assert!(range.len() <= (view / HEADER_H) as usize + 2, "{} rows drawn at {offset}", range.len());
            assert!(tops[range.start] <= offset && tops[range.end] >= offset + view - 0.01, "the view is covered at {offset}");
        }
        assert_eq!(visible(&tops, 0.0, 0.0), 0..0);
    }

    /// People coming online above the view do not move the row the person looks at.
    #[test]
    fn a_refresh_keeps_the_row_in_view_where_it_was() {
        let state = sample();
        let before = rows(&state, "", &HashSet::new());
        let keys: Vec<String> = before.iter().map(|r| r.key(&state)).collect();
        let tops_before = tops(&before);
        // Orbit Tester, in the game.
        let offset = tops_before[7] + 10.0;
        let anchor = anchor_at(&keys, &tops_before, offset).unwrap();
        // Two more favourites come into the game in the first group (above the view).
        let mut after_state = state.clone();
        for n in [4, 6] {
            one(&mut after_state, n, Presence::InGame, Whereabouts::Private, &["group_0"]);
        }
        let after = rows(&after_state, "", &HashSet::new());
        let after_keys: Vec<String> = after.iter().map(|r| r.key(&after_state)).collect();
        let new_offset = anchored(&after_keys, &tops(&after), &anchor).unwrap();
        // Two more rows above it, and the Website section (Lumen, its only one) gone below it.
        assert_eq!(new_offset, offset + 2.0 * PERSON_H);
    }
}
