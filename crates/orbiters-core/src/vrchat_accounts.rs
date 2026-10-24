//! The VRChat account signed in on this computer and its friends, as VRCX and VRCNext keep them (read only; their
//! databases are opened read-only while they run). N4N0's calls use them (contracts `Docs/orb-calls.md`): the account is
//! offered for the verified link, the friends are matched on Orbiters as hashes. Nothing here signs in to VRChat.

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
            Source::VrchatLog => "VRChat's log",
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

fn open(path: &Path) -> Option<Connection> {
    if !path.is_file() {
        return None;
    }
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()
}

fn vrcx_db() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("VRCX").join("VRCX.sqlite3"))
}

fn vrcnext_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("VRCNext"))
}

/// The account VRCX last signed in with.
fn vrcx_account(db: &Connection) -> Option<String> {
    db.query_row("SELECT value FROM configs WHERE key = 'config:lastuserloggedin'", [], |r| r.get::<_, String>(0)).ok().filter(|id| is_user_id(id))
}

/// VRCX names its per-account tables after the account: `usr1339e525d75d…_friend_log_current`.
fn vrcx_table_prefix(id: &str) -> String {
    id.replace(['_', '-'], "")
}

fn vrcx_friends(db: &Connection, account: &str) -> BTreeSet<String> {
    let table = format!("{}_friend_log_current", vrcx_table_prefix(account));
    let Ok(mut q) = db.prepare(&format!("SELECT user_id FROM \"{table}\"")) else { return BTreeSet::new() };
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
    let Ok(mut q) = db.prepare("SELECT user_id FROM user_tracking WHERE profile_is_friend = 1") else { return BTreeSet::new() };
    q.query_map([], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().filter(|id| is_user_id(id)).collect()).unwrap_or_default()
}

/// The VRChat account signed in on this computer and its friends.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalVrchat {
    pub account: Option<(String, Source)>,
    pub friends: BTreeSet<String>,
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
/// when neither knows an account, or when VRCNext knows several and none is the log's.
fn choose(log_account: Option<&str>, vrcx: Option<&str>, next: &[String]) -> Option<(String, Source)> {
    let log = log_account.filter(|id| is_user_id(id));
    match (log, vrcx) {
        (Some(log), Some(vrcx)) if log == vrcx => Some((log.to_string(), Source::Vrcx)),
        (Some(log), _) if next.iter().any(|n| n == log) => Some((log.to_string(), Source::Vrcnext)),
        (_, Some(vrcx)) => Some((vrcx.to_string(), Source::Vrcx)),
        (_, None) if next.len() == 1 => Some((next[0].clone(), Source::Vrcnext)),
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

    #[test]
    #[ignore = "reads this computer's VRCX and VRCNext"]
    fn reads_this_computer() {
        let local = read(None);
        println!("account {:?}, {} friends", local.account, local.friends.len());
    }
}
