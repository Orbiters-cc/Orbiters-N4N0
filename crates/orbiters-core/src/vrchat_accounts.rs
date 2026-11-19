//! The VRChat account signed in on this computer and its friends, as VRCX and VRCNext keep them (read only; their
//! databases are opened read-only while they run). N4N0's calls use them (contracts `Docs/n4n0-calls.md`): the account is
//! offered for the verified link, the friends are matched on Orbiters as hashes. Nothing here signs in to VRChat.

use orbiters_i18n::tr;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Where the VRChat account was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Vrcx,
    Vrcnext,
    VrchatLog,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Vrcx => "VRCX",
            Source::Vrcnext => "VRCNext",
            Source::VrchatLog => tr("VRChat's log"),
        }
    }
}

/// `usr_` followed by a lowercase UUID.
pub fn is_user_id(id: &str) -> bool {
    crate::vrchat_log::is_vrchat_id(id, "usr_")
}

/// A friend's id as Orbiters matches it: lowercase hex `SHA-256("orbiters-calls:" + id)`.
pub fn friend_hash(id: &str) -> String {
    let digest = Sha256::digest(format!("orbiters-calls:{id}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn open(path: &Path) -> Option<Connection> {
    if !path.is_file() {
        return None;
    }
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()
}

pub(crate) fn vrcx_db() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("VRCX").join("VRCX.sqlite3"))
}

pub(crate) fn vrcnext_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("VRCNext"))
}

/// The account VRCX last signed in with.
pub(crate) fn vrcx_account(db: &Connection) -> Option<String> {
    db.query_row("SELECT value FROM configs WHERE key = 'config:lastuserloggedin'", [], |r| r.get::<_, String>(0)).ok().filter(|id| is_user_id(id)) // i18n: not shown (SQL)
}

/// VRCX names its per-account tables after the account: `usr1339e525d75d…_friend_log_current`.
pub(crate) fn vrcx_table_prefix(id: &str) -> String {
    id.replace(['_', '-'], "")
}

/// How long [`vrcx_last_pictures`] keeps what it read while VRCX's database files look unchanged.
const PICTURES_KEPT: std::time::Duration = std::time::Duration::from_secs(600);

/// Each person's last avatar picture in VRCX's feed for `account` (id → address, as VRCX wrote it), from the database
/// at `path`, open as `db`. VRCX's `feed_avatar` has no index for "the last of each": the query reads the whole
/// table (hundreds of thousands of rows, a third of a second), so what it found is kept while the database and its
/// write-ahead log keep their length and time, for [`PICTURES_KEPT`] at most.
pub(crate) fn vrcx_last_pictures(path: &Path, db: &Connection, account: &str) -> std::sync::Arc<std::collections::HashMap<String, String>> {
    vrcx_last_pictures_now(path, db, account).unwrap_or_default()
}

/// [`vrcx_last_pictures`], `None` when the query fails now (SQLITE_BUSY while VRCX writes): nothing is kept then, so the
/// next round reads again instead of keeping "no pictures" for ten minutes.
pub(crate) fn vrcx_last_pictures_now(path: &Path, db: &Connection, account: &str) -> Option<std::sync::Arc<std::collections::HashMap<String, String>>> {
    let stamp = |p: &Path| std::fs::metadata(p).ok().and_then(|m| Some((m.len(), m.modified().ok()?)));
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    last_pictures_stamped(path, db, account, vec![stamp(path), stamp(Path::new(&wal))])
}

/// [`vrcx_last_pictures_now`] with the files' (length, time) `stamps`. What was kept is used only while the stamps AND the
/// feed's newest row are unchanged: VRCX only adds rows to its feed, so a new one shows at once even when the files'
/// length and time happen to read the same.
fn last_pictures_stamped(
    path: &Path,
    db: &Connection,
    account: &str,
    stamps: Vec<Option<(u64, std::time::SystemTime)>>,
) -> Option<std::sync::Arc<std::collections::HashMap<String, String>>> {
    type Kept = (PathBuf, String, Vec<Option<(u64, std::time::SystemTime)>>, Option<i64>, std::time::Instant, std::sync::Arc<std::collections::HashMap<String, String>>);
    // A few databases at most (one in the app; tests have their own each).
    static KEPT: std::sync::Mutex<Vec<Kept>> = std::sync::Mutex::new(Vec::new());
    let table = format!("{}_feed_avatar", vrcx_table_prefix(account));
    // The primary key's largest value: one look at the index's end, not a scan.
    let newest: Option<i64> = db.query_row(&format!("SELECT MAX(id) FROM \"{table}\""), [], |r| r.get(0)).ok()?; // i18n: not shown (SQL)
    if let Some((.., pictures)) = KEPT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(p, a, s, n, at, _)| p == path && a == account && *s == stamps && *n == newest && at.elapsed() < PICTURES_KEPT)
    {
        return Some(pictures.clone());
    }
    let mut pictures = std::collections::HashMap::new();
    let sql = format!("SELECT a.user_id, a.current_avatar_thumbnail_image_url FROM \"{table}\" a JOIN (SELECT user_id, MAX(id) AS latest FROM \"{table}\" WHERE user_id IS NOT NULL GROUP BY user_id) b ON a.id = b.latest"); // i18n: not shown (SQL)
    let mut q = db.prepare(&sql).ok()?;
    // A row whose person or picture is not text (NULL, a number: VRCX's tables allow it) is that row's loss, never the
    // whole list's: only a failing query or read (VRCX writing, a lock) fails the round.
    let rows = q
        .query_map([], |r| {
            let text = |i: usize| r.get_ref(i).ok().and_then(|v| v.as_str().ok()).map(str::to_string);
            Ok((text(0), text(1)))
        })
        .ok()?;
    for row in rows {
        let (id, image) = row.ok()?;
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            pictures.entry(id).or_insert_with(|| image.unwrap_or_default());
        }
    }
    let pictures = std::sync::Arc::new(pictures);
    let mut kept = KEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    kept.retain(|(p, a, ..)| !(p == path && a == account));
    kept.push((path.to_path_buf(), account.to_string(), stamps, newest, std::time::Instant::now(), pictures.clone()));
    if kept.len() > 4 {
        kept.remove(0);
    }
    Some(pictures)
}

fn vrcx_friends(db: &Connection, account: &str) -> BTreeSet<String> {
    let table = format!("{}_friend_log_current", vrcx_table_prefix(account));
    let Ok(mut q) = db.prepare(&format!("SELECT user_id FROM \"{table}\"")) else { return BTreeSet::new() }; // i18n: not shown (SQL)
    q.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().filter(|id| is_user_id(id)).collect()).unwrap_or_default()
}

/// VRCNext keeps one profile folder per account it signed in with.
fn vrcnext_accounts(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir.join("Caches").join("Profiles"))
        .map(|entries| entries.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| is_user_id(n)).collect())
        .unwrap_or_default()
}

fn vrcnext_friends(dir: &Path) -> BTreeSet<String> {
    let Some(db) = open(&dir.join("VRCNData.db")) else { return BTreeSet::new() };
    let Ok(mut q) = db.prepare("SELECT user_id FROM user_tracking WHERE profile_is_friend = 1") else { return BTreeSet::new() }; // i18n: not shown (SQL)
    q.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().filter(|id| is_user_id(id)).collect()).unwrap_or_default()
}

/// The VRChat account signed in on this computer and its friends.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalVrchat {
    pub account: Option<(String, Source)>,
    pub friends: BTreeSet<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Friend {
    pub id: String,
    pub name: String,
    pub image: Option<String>,
}

/// The same account/friend selection as calls, with the display metadata already cached by the local apps.
pub fn friends(log_account: Option<&str>) -> Vec<Friend> {
    let local = read(log_account);
    let mut out: std::collections::BTreeMap<String, Friend> =
        local.friends.iter().map(|id| (id.clone(), Friend { id: id.clone(), name: id.clone(), image: None })).collect();
    if let Some((account, _)) = &local.account {
        if let Some((path, db)) = vrcx_db().and_then(|p| open(&p).map(|db| (p, db))) {
            if vrcx_account(&db).as_ref() == Some(account) {
                let table = format!("{}_friend_log_current", vrcx_table_prefix(account));
                if let Ok(mut q) = db.prepare(&format!("SELECT user_id,display_name FROM \"{table}\"")) { // i18n: not shown (SQL)
                    if let Ok(rows) = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
                        for (id, name) in rows.flatten() {
                            if let Some(f) = out.get_mut(&id) {
                                f.name = name;
                            }
                        }
                    }
                }
                let pictures = vrcx_last_pictures(&path, &db, account);
                for f in out.values_mut() {
                    if let Some(image) = pictures.get(&f.id) {
                        f.image = Some(image.clone()).filter(|s| s.starts_with("https://"));
                    }
                }
            }
        }
        if let Some(db) = vrcnext_dir().and_then(|d| open(&d.join("VRCNData.db"))) {
            if let Ok(mut q) = db.prepare("SELECT user_id,display_name,profile_avatar_img FROM user_tracking WHERE profile_is_friend=1") {
                if let Ok(rows) = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))) {
                    for (id, name, image) in rows.flatten() {
                        if let Some(f) = out.get_mut(&id) {
                            if !name.is_empty() {
                                f.name = name;
                            }
                            if image.starts_with("https://") {
                                f.image = Some(image);
                            }
                        }
                    }
                }
            }
        }
    }
    let mut out: Vec<_> = out.into_values().collect();
    out.sort_by_key(|f| f.name.to_lowercase());
    out
}

/// Reconstruct the roster at the picture's timestamp from VRCX's recorded joins/leaves, when retained locally.
pub fn instance_people(location: &str, taken: u64) -> Vec<String> {
    let Some(db) = vrcx_db().and_then(|p| open(&p)) else { return Vec::new() };
    instance_people_in(&db, location, taken)
}

/// [`instance_people`] in VRCX's database `db`: each person's last join or leave there before `taken` (however many
/// events the instance had before it), those whose last one is a join.
fn instance_people_in(db: &Connection, location: &str, taken: u64) -> Vec<String> {
    let Ok(mut q) = db.prepare(
        "SELECT type,display_name,user_id FROM (SELECT type,display_name,user_id,ROW_NUMBER() OVER (PARTITION BY COALESCE(user_id,display_name) ORDER BY created_at DESC,id DESC) AS latest FROM gamelog_join_leave WHERE location=?1 AND type IN ('OnPlayerJoined','OnPlayerLeft') AND CAST(strftime('%s',created_at) AS INTEGER)<=?2) WHERE latest=1 LIMIT 20000", // i18n: not shown (SQL)
    ) else {
        return Vec::new();
    };
    let mut roster = std::collections::BTreeMap::new();
    if let Ok(rows) =
        q.query_map(rusqlite::params![location, taken as i64], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))
    {
        for (kind, name, id) in rows.flatten() {
            let id = id.unwrap_or_else(|| name.clone());
            if kind == "OnPlayerJoined" {
                roster.insert(id, name);
            } else if kind == "OnPlayerLeft" {
                roster.remove(&id);
            }
        }
    }
    roster.into_values().collect()
}

/// Reads VRCX and VRCNext. `log_account`: the local player's id from VRChat's log, used when neither knows it and to
/// pick between them. Friends come from the apps that know the chosen account.
pub fn read(log_account: Option<&str>) -> LocalVrchat {
    let vrcx = vrcx_db().and_then(|p| open(&p));
    let vrcx_account = vrcx.as_ref().and_then(vrcx_account);
    let next_dir = vrcnext_dir();
    let next_accounts = next_dir.as_deref().map(vrcnext_accounts).unwrap_or_default();
    let account = choose(log_account, vrcx_account.as_deref(), &next_accounts);
    let mut friends = BTreeSet::new();
    if let Some((id, _)) = &account {
        if let (Some(db), Some(vrcx_id)) = (&vrcx, &vrcx_account) {
            if vrcx_id == id {
                friends.extend(vrcx_friends(db, id));
            }
        }
        if let Some(dir) = &next_dir {
            if next_accounts.contains(id) {
                friends.extend(vrcnext_friends(dir));
            }
        }
        friends.remove(id);
    }
    LocalVrchat { account, friends }
}

/// Which account is this computer's. VRCX and VRCNext signed in to VRChat with theirs, so an account one of them
/// knows wins; the log's id (the local player's, as VRChat's log names it) picks between them, and stands alone only
/// when neither knows an account, or when VRCNext knows several and none is the log's. A folder in VRCNext's
/// `Caches\Profiles` is not proof of a sign-in (VRCNext makes one for a person whose favourites were only looked at):
/// it never stands against the log's own id.
fn choose(log_account: Option<&str>, vrcx: Option<&str>, next: &[String]) -> Option<(String, Source)> {
    let log = log_account.filter(|id| is_user_id(id));
    match (log, vrcx) {
        (Some(log), Some(vrcx)) if log == vrcx => Some((log.to_string(), Source::Vrcx)),
        (Some(log), _) if next.iter().any(|n| n == log) => Some((log.to_string(), Source::Vrcnext)),
        (_, Some(vrcx)) => Some((vrcx.to_string(), Source::Vrcx)),
        (None, None) if next.len() == 1 => Some((next[0].clone(), Source::Vrcnext)),
        (Some(log), None) => Some((log.to_string(), Source::VrchatLog)),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_the_apps_know_wins() {
        let (me, other, third) =
            ("usr_11111111-1111-1111-1111-111111111111", "usr_22222222-2222-2222-2222-222222222222", "usr_33333333-3333-3333-3333-333333333333");
        let next = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(choose(Some(me), Some(me), &[]), Some((me.into(), Source::Vrcx)));
        // The log says another account (a name posing in it, or an old line): VRCX's stands.
        assert_eq!(choose(Some(other), Some(me), &[]), Some((me.into(), Source::Vrcx)));
        // The log picks between VRCX and VRCNext.
        assert_eq!(choose(Some(other), Some(me), &next(&[other])), Some((other.into(), Source::Vrcnext)));
        assert_eq!(choose(None, None, &next(&[other])), Some((other.into(), Source::Vrcnext)));
        assert_eq!(choose(Some(third), None, &next(&[me, other])), Some((third.into(), Source::VrchatLog)), "VRCNext cannot tell which");
        assert_eq!(choose(Some(me), None, &[]), Some((me.into(), Source::VrchatLog)));
        // One profile VRCNext keeps is someone else's (favourites looked at): the log's own id stands.
        assert_eq!(choose(Some(me), None, &next(&[other])), Some((me.into(), Source::VrchatLog)));
        assert_eq!(choose(Some("usr_bad"), None, &[]), None);
    }

    #[test]
    fn ids_and_hashes() {
        assert!(is_user_id("usr_1339e525-d75d-4832-9104-98dd89287681"));
        assert!(!is_user_id("usr_1339E525-D75D-4832-9104-98DD89287681"), "VRChat writes them lowercase");
        assert!(!is_user_id("usr_1339e525"));
        assert!(!is_user_id("grp_1339e525-d75d-4832-9104-98dd89287681"));
        assert_eq!(vrcx_table_prefix("usr_1339e525-d75d-4832-9104-98dd89287681"), "usr1339e525d75d4832910498dd89287681");
        let h = friend_hash("usr_1339e525-d75d-4832-9104-98dd89287681");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn reads_vrcx_like_tables() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE configs (key TEXT, value TEXT);
             INSERT INTO configs VALUES ('config:lastuserloggedin', 'usr_11111111-1111-1111-1111-111111111111');
             CREATE TABLE usr11111111111111111111111111111111_friend_log_current (user_id TEXT, display_name TEXT);
             INSERT INTO usr11111111111111111111111111111111_friend_log_current VALUES ('usr_22222222-2222-2222-2222-222222222222', 'Friend'), ('bad', 'x');",
        )
        .unwrap();
        let me = vrcx_account(&db).unwrap();
        assert_eq!(me, "usr_11111111-1111-1111-1111-111111111111");
        assert_eq!(vrcx_friends(&db, &me), ["usr_22222222-2222-2222-2222-222222222222".to_string()].into());
    }

    /// CORE-10: VRCX's avatar feed is scanned once while its database is unchanged, again when it changed.
    #[test]
    fn last_pictures_are_read_again_only_when_vrcx_wrote() {
        let dir = std::env::temp_dir().join(format!("orbiters-vrcx-pictures-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("VRCX.sqlite3");
        let me = "usr_11111111-1111-1111-1111-111111111111";
        let table = format!("{}_feed_avatar", vrcx_table_prefix(me));
        let db = Connection::open(&path).unwrap();
        db.execute_batch(&format!(
            "CREATE TABLE \"{table}\" (id INTEGER PRIMARY KEY, user_id TEXT, current_avatar_thumbnail_image_url TEXT);
             INSERT INTO \"{table}\" (user_id, current_avatar_thumbnail_image_url) VALUES ('usr_a', 'https://old'), ('usr_a', 'https://new'), ('usr_b', NULL);"
        ))
        .unwrap();
        let first = vrcx_last_pictures(&path, &db, me);
        assert_eq!((first["usr_a"].as_str(), first["usr_b"].as_str()), ("https://new", ""), "each person's last");
        assert!(std::sync::Arc::ptr_eq(&first, &vrcx_last_pictures(&path, &db, me)), "unchanged: not read again");
        let rows: String = (0..200).map(|i| format!("('usr_c', 'https://{i}{}'),", "x".repeat(200))).collect();
        db.execute_batch(&format!("INSERT INTO \"{table}\" (user_id, current_avatar_thumbnail_image_url) VALUES {} ('usr_a', 'https://newest');", rows)).unwrap();
        let again = vrcx_last_pictures(&path, &db, me);
        assert_eq!(again["usr_a"], "https://newest", "VRCX wrote: read again");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Verifier (integration): one feed row without a person (NULL, as VRCX's schema allows) or with a number in place of
    /// text costs that row, not the whole friends' pictures (which made the launcher retry for ever, "busy").
    #[test]
    fn a_feed_row_without_a_person_costs_only_itself() {
        let dir = std::env::temp_dir().join(format!("orbiters-vrcx-null-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("VRCX.sqlite3");
        let me = "usr_11111111-1111-1111-1111-111111111111";
        let p = vrcx_table_prefix(me);
        let db = Connection::open(&path).unwrap();
        db.execute_batch(&format!(
            "CREATE TABLE configs (key TEXT, value TEXT);
             INSERT INTO configs VALUES ('config:lastuserloggedin', '{me}');
             CREATE TABLE \"{p}_friend_log_current\" (user_id TEXT, display_name TEXT, trust_level TEXT);
             INSERT INTO \"{p}_friend_log_current\" VALUES ('usr_22222222-2222-2222-2222-222222222222', 'Kite', 'User');
             CREATE TABLE \"{p}_feed_online_offline\" (id INTEGER PRIMARY KEY, user_id TEXT, created_at TEXT);
             CREATE TABLE \"{p}_feed_avatar\" (id INTEGER PRIMARY KEY, user_id TEXT, current_avatar_thumbnail_image_url TEXT);
             INSERT INTO \"{p}_feed_avatar\" (user_id, current_avatar_thumbnail_image_url) VALUES
               (NULL, 'https://nobody'), ('usr_22222222-2222-2222-2222-222222222222', 'https://api.vrchat.cloud/k'), (X'07', 'https://number'), ('usr_33333333-3333-3333-3333-333333333333', X'05');"
        ))
        .unwrap();
        let pictures = vrcx_last_pictures_now(&path, &db, me).expect("read, not busy");
        assert_eq!(pictures.get("usr_22222222-2222-2222-2222-222222222222").map(String::as_str), Some("https://api.vrchat.cloud/k"));
        assert_eq!(pictures.get("usr_33333333-3333-3333-3333-333333333333").map(String::as_str), Some(""), "a picture that is not text: none");
        drop(db);
        let state = crate::launcher_local::vrcx_now(&path).expect("the launcher's read is not busy").expect("friends");
        assert_eq!(state.friends[0].look.icon, "https://api.vrchat.cloud/k");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Verifier (CORE-10): a new feed row is seen at once even when the files' length and time read the same.
    #[test]
    fn a_new_feed_row_is_seen_whatever_the_files_say() {
        let dir = std::env::temp_dir().join(format!("orbiters-vrcx-newest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("VRCX.sqlite3");
        let me = "usr_11111111-1111-1111-1111-111111111111";
        let table = format!("{}_feed_avatar", vrcx_table_prefix(me));
        let db = Connection::open(&path).unwrap();
        db.execute_batch(&format!("CREATE TABLE \"{table}\" (id INTEGER PRIMARY KEY, user_id TEXT, current_avatar_thumbnail_image_url TEXT); INSERT INTO \"{table}\" (user_id, current_avatar_thumbnail_image_url) VALUES ('usr_a', 'https://old');")).unwrap();
        let same = vec![Some((1, std::time::UNIX_EPOCH)), None];
        assert_eq!(last_pictures_stamped(&path, &db, me, same.clone()).unwrap()["usr_a"], "https://old");
        db.execute_batch(&format!("INSERT INTO \"{table}\" (user_id, current_avatar_thumbnail_image_url) VALUES ('usr_a', 'https://new');")).unwrap();
        assert_eq!(last_pictures_stamped(&path, &db, me, same).unwrap()["usr_a"], "https://new");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_roster_counts_each_persons_last_event_however_busy_the_instance() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE gamelog_join_leave (id INTEGER PRIMARY KEY, created_at TEXT, type TEXT, display_name TEXT, location TEXT, user_id TEXT, time INTEGER);",
        )
        .unwrap();
        let here = "wrld_00000000-0000-4000-8000-000000000001:1~region(eu)";
        let event = |db: &Connection, at: u64, kind: &str, name: &str, id: Option<&str>, location: &str| {
            let at = time::OffsetDateTime::from_unix_timestamp(at as i64).unwrap().format(&time::format_description::well_known::Rfc3339).unwrap();
            db.execute("INSERT INTO gamelog_join_leave (created_at,type,display_name,location,user_id,time) VALUES (?1,?2,?3,?4,?5,0)", rusqlite::params![at, kind, name, location, id]).unwrap();
        };
        let base = 1_790_000_000;
        // Kite joins first and leaves just before the picture, after 20 000 others came and went.
        event(&db, base, "OnPlayerJoined", "Kite", Some("usr_k"), here);
        let tx = db.unchecked_transaction().unwrap();
        for i in 0..10_000u64 {
            event(&tx, base + 1 + i, "OnPlayerJoined", "Crowd", Some(&format!("usr_c{i}")), here);
            event(&tx, base + 1 + i, "OnPlayerLeft", "Crowd", Some(&format!("usr_c{i}")), here);
        }
        tx.commit().unwrap();
        let late = base + 20_000;
        event(&db, late, "OnPlayerLeft", "Kite", Some("usr_k"), here);
        // Nova joins late and stays; a name without an id joins; Lark joins after the picture; Moss joins elsewhere.
        event(&db, late, "OnPlayerJoined", "Nova", Some("usr_n"), here);
        event(&db, late, "OnPlayerJoined", "Wren", None, here);
        event(&db, late + 100, "OnPlayerJoined", "Lark", Some("usr_l"), here);
        event(&db, late, "OnPlayerJoined", "Moss", Some("usr_m"), "wrld_other:2");
        let mut people = instance_people_in(&db, here, late + 10);
        people.sort();
        assert_eq!(people, ["Nova", "Wren"]);
        // Before Kite left, Kite was there.
        assert!(instance_people_in(&db, here, late - 1).contains(&"Kite".to_string()));
    }

    #[test]
    #[ignore = "reads this computer's VRCX and VRCNext"]
    fn reads_this_computer() {
        let local = read(None);
        println!("account {:?}, {} friends", local.account, local.friends.len());
    }
}
