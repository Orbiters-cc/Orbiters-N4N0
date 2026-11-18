//! The friends list as rows, without drawing: the sections in the contract's order (the account's favourite groups,
//! then in game, on the website, offline), each one a header and its people unless folded; a search keeps the people
//! it finds in every section they are in, opens the folded ones and drops the sections left empty. Only the rows in
//! view are drawn ([`visible`]); a refresh keeps the first row in view where it was ([`anchored`]).

use orbiters_core::launcher::{Friend, FriendGroup, GroupKind, LauncherState, Whereabouts};
use std::collections::HashSet;
use std::ops::Range;

/// A section's header and a person's row.
pub const HEADER_H: f32 = 34.0;
pub const PERSON_H: f32 = 58.0;

#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Header {
        /// [`FriendGroup::id`].
        group: String,
        kind: GroupKind,
        /// The favourite group's own name (empty for the presence sections).
        name: String,
        /// People in it (found by the search, while one is typed).
        count: usize,
        /// Of them, in the game or on the website (a favourite group says it).
        online: usize,
        folded: bool,
    },
    Person {
        group: String,
        /// Index in [`LauncherState::friends`].
        friend: usize,
    },
}

impl Row {
    pub fn height(&self) -> f32 {
        match self {
            Row::Header { .. } => HEADER_H,
            Row::Person { .. } => PERSON_H,
        }
    }

    /// What tells a row from the others across refreshes: a person is in a favourite group and in their presence
    /// section, two rows.
    pub fn key(&self, state: &LauncherState) -> String {
        match self {
            Row::Header { group, .. } => format!("h:{group}"),
            Row::Person { group, friend } => format!("p:{group}:{}", state.friends.get(*friend).map_or("", |f| f.id.as_str())),
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

/// The rows of the list for `query` with the sections in `folded` closed.
pub fn rows(state: &LauncherState, query: &str, folded: &HashSet<String>) -> Vec<Row> {
    let query = query.trim().to_lowercase();
    let searching = !query.is_empty();
    let index: std::collections::HashMap<&str, usize> = state.friends.iter().enumerate().map(|(i, f)| (f.id.as_str(), i)).collect();
    let mut out = Vec::new();
    for group in &state.groups {
        let found: Vec<usize> =
            group.members.iter().filter_map(|id| index.get(id.as_str()).copied()).filter(|&i| matches(&state.friends[i], &query)).collect();
        // A presence section with nobody, and any section the search left empty, says nothing.
        if found.is_empty() && (searching || group.kind != GroupKind::Favourite) {
            continue;
        }
        let online = found.iter().filter(|&&i| state.friends[i].presence != orbiters_core::launcher::Presence::Offline).count();
        let is_folded = !searching && folded.contains(&group.id);
        out.push(Row::Header { group: group.id.clone(), kind: group.kind, name: group.name.clone(), count: found.len(), online, folded: is_folded });
        if !is_folded {
            out.extend(found.into_iter().map(|friend| Row::Person { group: group.id.clone(), friend }));
        }
    }
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
            })
            .collect()
    }

    #[test]
    fn favourites_come_first_then_in_game_website_offline() {
        let state = sample();
        let rows = rows(&state, "", &HashSet::new());
        let headers: Vec<_> = rows.iter().filter_map(|r| if let Row::Header { group, .. } = r { Some(group.as_str()) } else { None }).collect();
        assert_eq!(headers, ["group_0", "group_1", "in-game", "website", "offline"]);
        // A favourite is in their group and in their presence section.
        let nova = people(&rows, &state).iter().filter(|n| *n == "Nova Sample").count();
        assert_eq!(nova, 2);
        let Row::Header { online, count, .. } = &rows[0] else { panic!() };
        assert_eq!((*count, *online), (2, 2));
    }

    #[test]
    fn a_folded_section_keeps_its_header_and_hides_its_people() {
        let state = sample();
        let folded: HashSet<String> = ["in-game".to_string()].into();
        let rows = rows(&state, "", &folded);
        let at = rows.iter().position(|r| matches!(r, Row::Header { group, .. } if group == "in-game")).unwrap();
        assert!(matches!(&rows[at], Row::Header { folded: true, count: 4, .. }));
        assert!(matches!(&rows[at + 1], Row::Header { group, .. } if group == "website"));
    }

    #[test]
    fn empty_presence_sections_are_left_out_but_favourite_groups_stay() {
        let mut state = sample();
        for f in &mut state.friends {
            if f.presence == Presence::Website {
                f.presence = Presence::Offline;
            }
        }
        state.groups.iter_mut().find(|g| g.id == "website").unwrap().members.clear();
        state.groups.iter_mut().find(|g| g.id == "group_1").unwrap().members.clear();
        let rows = rows(&state, "", &HashSet::new());
        assert!(!rows.iter().any(|r| matches!(r, Row::Header { group, .. } if group == "website")));
        assert!(rows.iter().any(|r| matches!(r, Row::Header { group, count: 0, .. } if group == "group_1")));
    }

    #[test]
    fn search_finds_names_status_worlds_and_notes_and_opens_folded_sections() {
        let state = sample();
        let folded: HashSet<String> = state.groups.iter().map(|g| g.id.clone()).collect();
        assert_eq!(people(&rows(&state, "  MOCHI ", &folded), &state), ["[group_1 1]", "Mochi Byte", "[in-game 1]", "Mochi Byte"]);
        // The world someone is in.
        assert_eq!(people(&rows(&state, "café", &folded), &state), ["[group_1 1]", "Mochi Byte", "[in-game 1]", "Mochi Byte"]);
        // Their status line, the account's note.
        assert!(people(&rows(&state, "unity", &folded), &state).contains(&"Orbit Tester".to_string()));
        assert!(people(&rows(&state, "meetup", &folded), &state).contains(&"Pixel Fox".to_string()));
        assert!(rows(&state, "nobody by this name", &folded).is_empty());
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
            state.groups.iter_mut().find(|g| g.id == "offline").unwrap().members.push(f.id.clone());
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
        let offset = tops_before[7] + 10.0;
        let anchor = anchor_at(&keys, &tops_before, offset).unwrap();
        // Two more people in the first favourite group (above the view).
        let mut after_state = state.clone();
        let extra: Vec<String> = after_state.friends[3..5].iter().map(|f| f.id.clone()).collect();
        after_state.groups[0].members.extend(extra);
        let after = rows(&after_state, "", &HashSet::new());
        let after_keys: Vec<String> = after.iter().map(|r| r.key(&after_state)).collect();
        let new_offset = anchored(&after_keys, &tops(&after), &anchor).unwrap();
        assert_eq!(new_offset, offset + 2.0 * PERSON_H);
    }
}
