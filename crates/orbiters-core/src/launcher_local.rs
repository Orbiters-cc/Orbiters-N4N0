//! The friends list without a VRChat sign-in in N4N0: what VRCNext or VRCX already keep on this computer (read only, as
//! [`crate::vrchat_accounts`] does; nothing is written to their folders and no copy leaves this computer).
//!
//! - VRCNext (`%APPDATA%\VRCNext\Caches`): `friends_cache.json` (every friend as VRChat last described them, with
//!   `presence` game / web / offline, decorations, last seen) and `fav_friends_cache.json` (VRChat's favourite friend
//!   groups with their names, and who is in which). The richest: preferred.
//! - VRCX (`%APPDATA%\VRCX\VRCX.sqlite3`): the account's `friend_log_current` (names, trust), `notes`, the last
//!   avatar picture (`feed_avatar`), last seen (`feed_online_offline`), and VRCX's own local favourite groups
//!   (`favorite_friend`).
//!
//! Either way the status is a last-known copy: live status, invites, boops and invite requests need the sign-in
//! ([`Possible`] says so).
use crate::launcher::*;
use crate::launcher_feed;
use serde_json::Value;
use std::path::Path;

fn modified(path: &Path) -> i64 {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64)
}

/// A copy that exists but could not be read now (caught while its app rewrites it, its database busy): the list on
/// show stays, and the read is tried again soon, instead of the list emptying or switching to the other app's copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

/// The state from VRCNext's caches in `dir` (`…\VRCNext`), when there are friends in them.
pub fn vrcnext(dir: &Path) -> Option<LauncherState> {
    vrcnext_now(dir).ok().flatten()
}

/// [`vrcnext`], telling "no copy" (`Ok(None)`) from "a copy, not readable now" (`Err(Busy)`).
pub fn vrcnext_now(dir: &Path) -> Result<Option<LauncherState>, Busy> {
    let caches = dir.join("Caches");
    let file = caches.join("friends_cache.json");
    let bytes = match std::fs::read(&file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Busy),
    };
    // Half written (VRCNext rewriting it): not "no friends".
    let friends: Value = serde_json::from_slice(&bytes).map_err(|_| Busy)?;
    let favourites: Value = std::fs::read(caches.join("fav_friends_cache.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let mut state = from_vrcnext(&friends, &favourites, modified(&file));
    // The friends VRCNext looked at closely keep VRChat's own addresses of their picture and banner, and the note.
    if let Some(db) = crate::vrchat_accounts::open(&dir.join("VRCNData.db")) {
        if let Ok(mut q) = db.prepare("SELECT user_id, image, profile_banner_url, profile_note FROM user_tracking WHERE profile_is_friend = 1") { // i18n: not shown (SQL)
            // A NULL column (no banner) leaves that field empty, not the whole row out.
            let text = |r: &rusqlite::Row, i: usize| r.get::<_, Option<String>>(i).map(Option::unwrap_or_default);
            let rows: Vec<[String; 4]> = q
                .query_map([], |r| Ok([text(r, 0)?, text(r, 1)?, text(r, 2)?, text(r, 3)?]))
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default();
            fill_from_tracking(&mut state, &rows);
        }
    }
    Ok((!state.friends.is_empty()).then_some(state))
}

/// VRCNext's `user_tracking` rows (id, picture, banner, note) fill what its friends cache left empty.
pub fn fill_from_tracking(state: &mut LauncherState, rows: &[[String; 4]]) {
    let at: std::collections::HashMap<String, usize> = state.friends.iter().enumerate().rev().map(|(i, f)| (f.id.clone(), i)).collect();
    for [id, picture, banner, note] in rows {
        if let Some(f) = at.get(id).map(|&i| &mut state.friends[i]) {
            if f.look.icon.is_empty() && picture.starts_with("https://") {
                f.look.icon = picture.clone();
            }
            if f.look.banner.is_empty() && banner.starts_with("https://") {
                f.look.banner = banner.clone();
            }
            if f.note.is_empty() {
                f.note = note.clone();
            }
        }
    }
}

/// [`vrcnext`] on the two files' contents.
pub fn from_vrcnext(friends: &Value, favourites: &Value, read_at: i64) -> LauncherState {
    let groups = launcher_feed::favourite_groups(favourites.get("groups").unwrap_or(&Value::Null));
    let mut members: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for f in favourites.get("friends").and_then(Value::as_array).into_iter().flatten() {
        let (user, group) = (f.get("favoriteId").and_then(Value::as_str), f.get("groupName").and_then(Value::as_str));
        if let (Some(user), Some(group)) = (user, group) {
            members.entry(user.to_string()).or_default().push(group.to_string());
        }
    }
    let mut state = LauncherState {
        source: Source::Database { app: DatabaseApp::VrcNext, read_at },
        possible: Possible { play: true, ..Possible::default() },
        ..LauncherState::default()
    };
    for v in friends.get("friends").and_then(Value::as_array).into_iter().flatten() {
        // VRCNext's own names for VRChat's fields.
        let mut user = v.clone();
        for (theirs, vrchats) in [("lastLogin", "last_login"), ("lastActivity", "last_activity"), ("dateJoined", "date_joined")] {
            if let Some(x) = v.get(theirs) {
                user[vrchats] = x.clone();
            }
        }
        if let Some(image) = v.get("image") {
            user["currentAvatarThumbnailImageUrl"] = image.clone();
        }
        user["state"] = Value::String(match v.get("presence").and_then(Value::as_str) { Some("game") => "online", Some("web") => "active", _ => "offline" }.into());
        let Some(mut friend) = launcher_feed::friend(&user, false) else { continue };
        if let Some(seen) = v.get("lastSeen").and_then(Value::as_str).and_then(launcher_feed::seconds) {
            friend.last_seen = friend.last_seen.max(Some(seen));
        }
        // Decoration pictures VRCNext stores are addresses of its own local cache (`http://localhost…`): only VRChat's
        // own `https://` ones are kept (the controller resolves the ids otherwise).
        friend.look.nameplate = Some(v.get("nameplateUrl").and_then(Value::as_str).unwrap_or_default()).filter(|u| u.starts_with("https://")).unwrap_or_default().to_string();
        friend.look.icon_frame = Some(v.get("iconFrameUrl").and_then(Value::as_str).unwrap_or_default()).filter(|u| u.starts_with("https://")).unwrap_or_default().to_string();
        friend.favourite_groups = members.remove(&friend.id).unwrap_or_default();
        state.friends.push(friend);
    }
    launcher_feed::regroup(&mut state, &groups);
    state
}

/// The state from VRCX's database at `db` for the account it last signed in with.
pub fn vrcx(db: &Path) -> Option<LauncherState> {
    vrcx_now(db).ok().flatten()
}

/// [`vrcx`], telling "no database" (`Ok(None)`) from "a database, busy or failing now" (`Err(Busy)`): a query that fails
/// (SQLITE_BUSY while VRCX writes) no longer drops the notes, pictures or last seen of that round.
pub fn vrcx_now(db: &Path) -> Result<Option<LauncherState>, Busy> {
    if !db.is_file() {
        return Ok(None);
    }
    let connection = crate::vrchat_accounts::open(db).ok_or(Busy)?;
    let _ = connection.busy_timeout(std::time::Duration::from_millis(500));
    let Some(account) = crate::vrchat_accounts::vrcx_account(&connection) else { return Ok(None) };
    let prefix = crate::vrchat_accounts::vrcx_table_prefix(&account);
    let rows = |sql: String| -> Result<Vec<Vec<String>>, Busy> {
        let mut q = connection.prepare(&sql).map_err(|_| Busy)?;
        let n = q.column_count();
        let rows = q.query_map([], |r| (0..n).map(|i| r.get::<_, Option<String>>(i).map(Option::unwrap_or_default)).collect::<Result<Vec<_>, _>>()).map_err(|_| Busy)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|_| Busy)
    };
    let friends = rows(format!("SELECT user_id, display_name, trust_level FROM \"{prefix}_friend_log_current\""))?; // i18n: not shown (SQL)
    let notes = rows(format!("SELECT user_id, note FROM \"{prefix}_notes\"")).unwrap_or_default(); // i18n: not shown (SQL)
    let pictures = crate::vrchat_accounts::vrcx_last_pictures_now(db, &connection, &account).ok_or(Busy)?;
    let seen = rows(format!("SELECT user_id, MAX(created_at) FROM \"{prefix}_feed_online_offline\" GROUP BY user_id"))?; // i18n: not shown (SQL)
    let favourites = rows("SELECT user_id, group_name FROM favorite_friend".into()).unwrap_or_default(); // i18n: not shown (SQL)
    let state = from_vrcx(&friends, &notes, &pictures, &seen, &favourites, modified(db));
    Ok((!state.friends.is_empty()).then_some(state))
}

/// [`vrcx`] on the rows read (`pictures`: each person's last avatar picture).
pub fn from_vrcx(
    friends: &[Vec<String>],
    notes: &[Vec<String>],
    pictures: &std::collections::HashMap<String, String>,
    seen: &[Vec<String>],
    favourites: &[Vec<String>],
    read_at: i64,
) -> LauncherState {
    // Each table looked up by id once (a person's first row, as a search from the top finds it), not searched for each friend.
    fn by_id(rows: &[Vec<String>]) -> std::collections::HashMap<&str, &str> {
        let mut map = std::collections::HashMap::new();
        for r in rows {
            if let Some(id) = r.first() {
                map.entry(id.as_str()).or_insert_with(|| r.get(1).map_or("", String::as_str));
            }
        }
        map
    }
    let (notes, seen) = (by_id(notes), by_id(seen));
    let mut groups_of: std::collections::HashMap<&str, Vec<String>> = std::collections::HashMap::new();
    for r in favourites {
        if let (Some(id), Some(group)) = (r.first(), r.get(1)) {
            groups_of.entry(id.as_str()).or_default().push(group.clone());
        }
    }
    let find = |map: &std::collections::HashMap<&str, &str>, id: &str| map.get(id).map(|s| s.to_string()).unwrap_or_default();
    let mut group_names: Vec<String> = favourites.iter().filter_map(|r| r.get(1)).filter(|g| !g.is_empty()).cloned().collect();
    group_names.sort();
    group_names.dedup();
    let groups: Vec<(String, String)> = group_names.into_iter().map(|g| (g.clone(), g)).collect();
    let mut state = LauncherState {
        source: Source::Database { app: DatabaseApp::Vrcx, read_at },
        possible: Possible { play: true, ..Possible::default() },
        ..LauncherState::default()
    };
    for row in friends {
        let (Some(id), Some(name)) = (row.first(), row.get(1)) else { continue };
        if !crate::vrchat_accounts::is_user_id(id) {
            continue;
        }
        let trust = row.get(2).map(String::as_str).unwrap_or_default();
        let rank = match trust {
            "Trusted User" | "Veteran User" => Rank::TrustedUser,
            "Known User" => Rank::KnownUser,
            "User" => Rank::User,
            "New User" => Rank::NewUser,
            _ => Rank::Visitor,
        };
        let picture = pictures.get(id).cloned().unwrap_or_default();
        state.friends.push(Friend {
            id: id.clone(),
            name: if name.is_empty() { id.clone() } else { name.clone() },
            look: Look { icon: if picture.starts_with("https://") { picture } else { String::new() }, rank, ..Look::default() },
            whereabouts: Whereabouts::Unknown,
            last_seen: launcher_feed::seconds(&find(&seen, id)),
            note: find(&notes, id),
            favourite_groups: groups_of.get(id.as_str()).cloned().unwrap_or_default(),
            ..Friend::default()
        });
    }
    launcher_feed::regroup(&mut state, &groups);
    state
}

/// What there is without a sign-in: VRCNext's copy when it has friends, else VRCX's, else nothing.
pub fn read() -> LauncherState {
    read_now().unwrap_or_else(|Busy| nothing())
}

fn nothing() -> LauncherState {
    LauncherState { possible: Possible { play: true, ..Possible::default() }, ..LauncherState::default() }
}

/// [`read`], or `Err(Busy)` when the copy to show could not be read now: VRCNext's copy that is there but unreadable
/// does not fall to VRCX's (another list, other groups).
pub fn read_now() -> Result<LauncherState, Busy> {
    choose(crate::vrchat_accounts::vrcnext_dir().map_or(Ok(None), |d| vrcnext_now(&d)), || crate::vrchat_accounts::vrcx_db().map_or(Ok(None), |d| vrcx_now(&d)))
}

/// VRCNext first; VRCX only when VRCNext has no copy (not when it was caught busy).
pub fn choose(vrcnext: Result<Option<LauncherState>, Busy>, vrcx: impl FnOnce() -> Result<Option<LauncherState>, Busy>) -> Result<LauncherState, Busy> {
    match vrcnext? {
        Some(state) => Ok(state),
        None => Ok(vrcx()?.unwrap_or_else(nothing)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const A: &str = "usr_00000000-0000-4000-8000-000000000001";
    const B: &str = "usr_00000000-0000-4000-8000-000000000002";

    #[test]
    fn vrcnexts_caches_give_the_sections_and_the_named_groups() {
        let friends = json!({"friends": [
            {"id": A, "displayName": "Nova", "image": "https://api.vrchat.cloud/a", "status": "join me", "statusDescription": "hi", "location": "wrld_00000000-0000-4000-8000-0000000000aa:1~region(eu)",
             "platform": "standalonewindows", "presence": "game", "tags": ["system_trust_veteran"], "lastLogin": "2026-11-18T08:00:00Z", "lastActivity": "", "nameplateUrl": "http://localhost:1/img", "nameplateEffect": "invt_1"},
            {"id": B, "displayName": "Kite", "image": "", "status": "offline", "location": "offline", "platform": "", "presence": "offline", "tags": [], "lastSeen": "2026-11-10T08:00:00Z"}
        ], "counts": {"game": 1, "web": 0, "offline": 1}});
        let favourites = json!({"friends": [{"favoriteId": A, "fvrtId": "fvrt_1", "groupName": "group_0"}],
            "groups": [{"name": "group_0", "displayName": "best", "type": "friend", "visibility": "private", "capacity": 150, "local": false}]});
        let s = from_vrcnext(&friends, &favourites, 5);
        assert_eq!(s.source, Source::Database { app: DatabaseApp::VrcNext, read_at: 5 });
        assert!(!s.possible.live_status && !s.possible.boop && !s.possible.invites && s.possible.play);
        let ids: Vec<(&str, &str, usize)> = s.groups.iter().map(|g| (g.id.as_str(), g.name.as_str(), g.members.len())).collect();
        assert_eq!(ids, [("group_0", "best", 1), ("in-game", "", 1), ("website", "", 0), ("offline", "", 1)]);
        let a = s.friend(A).unwrap();
        assert_eq!((a.status, a.look.rank, a.look.icon.as_str(), a.look.nameplate.as_str()), (Status::JoinMe, Rank::TrustedUser, "https://api.vrchat.cloud/a", ""));
        assert_eq!(s.friend(B).unwrap().last_seen, launcher_feed::seconds("2026-11-10T08:00:00Z"));
        let mut s = s;
        fill_from_tracking(&mut s, &[[B.into(), "https://api.vrchat.cloud/b".into(), "http://localhost/x".into(), "note".into()]]);
        let b = s.friend(B).unwrap();
        assert_eq!((b.look.icon.as_str(), b.look.banner.as_str(), b.note.as_str()), ("https://api.vrchat.cloud/b", "", "note"));
    }

    #[test]
    fn vrcxs_rows_give_names_notes_and_its_own_groups() {
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let s = from_vrcx(
            &[r(&[A, "Nova", "Trusted User"]), r(&[B, "Kite", "User"]), r(&["bad", "x", ""])],
            &[r(&[A, "met at a meetup"])],
            &[(A.to_string(), "https://api.vrchat.cloud/a".to_string())].into_iter().collect(),
            &[r(&[B, "2026-11-10T08:00:00.000Z"])],
            &[r(&[A, "Close"])],
            9,
        );
        assert_eq!(s.friends.len(), 2);
        let a = s.friend(A).unwrap();
        assert_eq!((a.note.as_str(), a.look.rank, a.whereabouts.clone(), a.favourite_groups.clone()), ("met at a meetup", Rank::TrustedUser, Whereabouts::Unknown, vec!["Close".to_string()]));
        assert_eq!(s.groups[0].name, "Close");
        assert_eq!(s.friend(B).unwrap().last_seen, launcher_feed::seconds("2026-11-10T08:00:00Z"));
    }

    #[test]
    fn a_copy_caught_while_it_is_written_keeps_the_list_and_never_switches_app() {
        let dir = std::env::temp_dir().join(format!("n4n0-local-{}", std::process::id()));
        let caches = dir.join("Caches");
        std::fs::create_dir_all(&caches).unwrap();
        assert_eq!(vrcnext_now(&dir), Ok(None), "no copy: none");
        std::fs::write(caches.join("friends_cache.json"), b"{\"friends\": [{\"id\": \"usr_").unwrap();
        assert_eq!(vrcnext_now(&dir), Err(Busy), "half written: busy, not empty");
        let other = || Ok(Some(LauncherState { source: Source::Database { app: DatabaseApp::Vrcx, read_at: 1 }, ..LauncherState::default() }));
        assert_eq!(choose(Err(Busy), other), Err(Busy), "VRCNext busy: not VRCX's list instead");
        assert!(matches!(choose(Ok(None), other).unwrap().source, Source::Database { app: DatabaseApp::Vrcx, .. }));
        assert_eq!(choose(Ok(None), || Err(Busy)), Err(Busy));
        assert_eq!(vrcx_now(&dir.join("none.sqlite3")), Ok(None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
