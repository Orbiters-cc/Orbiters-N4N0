//! Whether this Orbiters account may use N4N0 calls: the per-account feature `n4n0-calls`
//! (`orbiters_core::features::N4N0_CALLS`, contracts `Docs/n4n0-calls.md`, "Who has calls"). [`Controller::calls_open`]
//! is the one place that decides; every way into a call asks it (calling a friend from the window, the overlay, N4N0's
//! menu or the avatar's screen, answering, being rung, the avatar's Call parameters), and without it Call shows greyed
//! out as "Coming soon" everywhere ([`CallLink::ComingSoon`](crate::state::CallLink)).
//!
//! The server says it with the session check (`GET /unity-wizard/token`, `features`), asked again here every
//! [`ASK_EVERY`] (`GET /features/mine`), and a call request it refuses with `not_enabled` closes calls at once. What it
//! last said for an account is kept on this computer ([`FILE`]) for one use only: a start without Orbiters (offline)
//! goes on what was last known until the server answers. The server stays the judge: a modified app gains nothing.

use super::Controller;
use crate::state::AccountState;
use orbiters_core::account::{Api, ApiError, Server};
use orbiters_core::features::N4N0_CALLS;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How often a signed-in account's calls are asked about again, so a switch an administrator made reaches a running app.
const ASK_EVERY: Duration = Duration::from_secs(15 * 60);
/// After a look that got no answer (offline, the server busy), or before the first answer.
const ASK_AGAIN: Duration = Duration::from_secs(30);
/// What a way into a call that is closed says (a notice, N4N0's panel).
pub(super) const CALLS_SOON_TOLD: &str = orbiters_i18n::mark("Calls are coming soon to Orbiters - N4N0. They aren't open for your account yet.");
/// What the server last said, per server and account (`{"production/42": true}`), in the config folder.
const FILE: &str = "calls-access.json";

/// What is known of the account's calls.
#[derive(Debug, Default)]
pub(super) struct CallsAccess {
    /// The server and account (its id) the rest is about.
    whose: Option<(Server, String)>,
    /// What the server said during this run.
    heard: Option<bool>,
    /// What it said last time, read from [`FILE`] when the account became this one.
    last_known: Option<bool>,
    /// When it was last asked here (the session check is not counted: it is the account's).
    asked: Option<Instant>,
}

impl CallsAccess {
    /// Follows the account now signed in (`None`: nobody): another one starts from what was last known of it.
    fn follow(&mut self, whose: Option<(Server, String)>, file: &PathBuf) {
        if self.whose == whose {
            return;
        }
        self.heard = None;
        self.asked = None;
        self.last_known = whose.as_ref().and_then(|w| read_file(file).get(&key(w)).copied());
        self.whose = whose;
    }

    /// Whether calls are open, with the account's state as the window shows it. Signed in: only what the server said
    /// in this run counts. Not answered yet or out of reach (checking, offline): what it said last time. Anything
    /// else (signed out, expired, signing in, nobody known): closed.
    fn open(&self, state: &AccountState) -> bool {
        if self.whose.is_none() {
            return false;
        }
        match state {
            AccountState::SignedIn { .. } => self.heard == Some(true),
            AccountState::Checking { .. } | AccountState::Offline { .. } => self.heard.or(self.last_known) == Some(true),
            _ => false,
        }
    }

    /// Whether to ask the server now.
    fn due(&self, now: Instant) -> bool {
        let wait = if self.heard.is_some() { ASK_EVERY } else { ASK_AGAIN };
        self.asked.is_none_or(|t| now.duration_since(t) >= wait)
    }

    /// The server said `allowed` for `whose`: kept for this run, and in the file for the next start. An answer about
    /// another account than the one followed now is dropped.
    fn heard(&mut self, whose: &(Server, String), allowed: bool, file: &PathBuf) -> bool {
        if self.whose.as_ref() != Some(whose) {
            return false;
        }
        let changed = self.heard != Some(allowed);
        self.heard = Some(allowed);
        if self.last_known != Some(allowed) {
            self.last_known = Some(allowed);
            let mut all = read_file(file);
            all.insert(key(whose), allowed);
            if let Err(e) = write_file(file, &all) {
                eprintln!("calls access: {e:#}");
            }
        }
        changed
    }
}

fn key((server, user): &(Server, String)) -> String {
    format!("{}/{user}", server.cache_name()) // i18n: not shown (a key in a file)
}

fn read_file(path: &PathBuf) -> BTreeMap<String, bool> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn write_file(path: &PathBuf, all: &BTreeMap<String, bool>) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(all)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Orbiters refused a call request because the account has not N4N0 calls (403 `{"code": "not_enabled"}`; only with
/// that code: another 403 is not about the feature).
pub(super) fn not_enabled(e: &anyhow::Error) -> bool {
    e.downcast_ref::<ApiError>().is_some_and(|a| a.status == 403 && a.code.as_deref() == Some("not_enabled"))
}

impl Controller {
    fn calls_access_file(&self) -> PathBuf {
        orbiters_core::paths::config_dir().join(FILE)
    }

    /// The account calls are about now: the server it was read for (the chosen one before it was) and its id.
    fn calls_whose(&self) -> Option<(Server, String)> {
        let rt = self.account_runtime.lock().unwrap();
        let account = rt.account.as_ref()?;
        let server = rt.read.map_or_else(Server::current, |(s, _)| s);
        Some((server, account.user.clone().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| account.username.clone().unwrap_or_default())))
    }

    /// May this account use calls now? The one question every way into a call asks.
    pub(crate) fn calls_open(&self) -> bool {
        let whose = self.calls_whose();
        let state = self.snapshot.lock().unwrap().account.state.clone();
        let file = self.calls_access_file();
        let mut rt = self.calls.lock().unwrap();
        rt.access.follow(whose, &file);
        rt.access.open(&state)
    }

    /// What the session check said of the account's features (`None`: nothing, an older server): calls follow it.
    pub(super) fn calls_access_checked(&self, server: Server, user: Option<String>, username: Option<String>, features: Option<&BTreeMap<String, bool>>) {
        let Some(allowed) = features.map(|f| f.get(N4N0_CALLS).copied().unwrap_or(false)) else { return };
        let whose = (server, user.filter(|u| !u.trim().is_empty()).unwrap_or_else(|| username.unwrap_or_default()));
        let file = self.calls_access_file();
        let mut rt = self.calls.lock().unwrap();
        rt.access.follow(Some(whose.clone()), &file);
        rt.access.heard(&whose, allowed, &file);
    }

    /// A call request was refused with `not_enabled`: calls close here at once (the next look publishes it).
    pub(super) fn calls_access_refused(&self) {
        let Some(whose) = self.calls_whose() else { return };
        let file = self.calls_access_file();
        let mut rt = self.calls.lock().unwrap();
        rt.access.follow(Some(whose.clone()), &file);
        rt.access.heard(&whose, false, &file);
    }

    /// Asks the server about the account's calls when it is time to (`api` and `token` are the account's). On the calls
    /// thread: the answer can take a moment.
    pub(super) fn calls_access_refresh(&self, api: &Api, token: &str) {
        let Some(whose) = self.calls_whose() else { return };
        let file = self.calls_access_file();
        let now = Instant::now();
        {
            let mut rt = self.calls.lock().unwrap();
            rt.access.follow(Some(whose.clone()), &file);
            if !rt.access.due(now) {
                return;
            }
            rt.access.asked = Some(now);
        }
        match api.member_feature(token, N4N0_CALLS) {
            Ok(allowed) => {
                self.calls.lock().unwrap().access.heard(&whose, allowed, &file);
            }
            // The credential refused: no calls with it.
            Err(e) if e.downcast_ref::<ApiError>().is_some_and(|a| matches!(a.status, 401 | 403 | 404 | 425)) => {
                self.calls.lock().unwrap().access.heard(&whose, false, &file);
            }
            Err(e) => eprintln!("calls access: {e:#}"),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    fn whose(user: &str) -> (Server, String) {
        (Server::Production, user.to_string())
    }
    fn signed_in() -> AccountState {
        AccountState::SignedIn { username: "Nova".into() }
    }

    fn file(name: &str) -> PathBuf {
        let dir = orbiters_core::paths::test_roots().config.join(format!("calls-access-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(FILE)
    }

    /// Off unless the server said so for this account: signed out, not known yet, refused, another account, or
    /// offline with nothing known; offline with a yes last time goes on it until the server answers.
    #[test]
    fn calls_are_open_only_on_the_server_s_word_for_this_account() {
        let path = file("word");
        let mut access = CallsAccess::default();
        assert!(!access.open(&AccountState::SignedOut), "nobody");
        access.follow(Some(whose("42")), &path);
        for state in [AccountState::Unknown, AccountState::SignedOut, AccountState::SigningIn, AccountState::Expired { username: None }, signed_in()] {
            assert!(!access.open(&state), "not known yet: {state:?}");
        }
        assert!(!access.open(&AccountState::Offline { username: None }), "offline with nothing known");
        assert!(access.heard(&whose("42"), true, &path));
        assert!(access.open(&signed_in()));
        assert!(!access.open(&AccountState::Expired { username: None }), "an expired sign-in has no calls");
        // Another account (or another server) starts from what was known of it: nothing.
        access.follow(Some(whose("7")), &path);
        assert!(!access.open(&signed_in()));
        assert!(!access.heard(&whose("42"), true, &path), "an answer about the account before is dropped");
        // Back to the first, at the next start: only "last known", used while Orbiters does not answer.
        let mut next = CallsAccess::default();
        next.follow(Some(whose("42")), &path);
        assert!(next.open(&AccountState::Offline { username: None }) && next.open(&AccountState::Checking { username: None }));
        assert!(!next.open(&signed_in()), "signed in, the server's word of this run is needed");
        // Switched off: closed at once, and remembered so.
        next.heard(&whose("42"), false, &path);
        assert!(!next.open(&signed_in()));
        let mut after = CallsAccess::default();
        after.follow(Some(whose("42")), &path);
        assert!(!after.open(&AccountState::Offline { username: None }));
        assert_eq!(read_file(&path).get("production/42"), Some(&false));
    }

    #[test]
    fn it_is_asked_again_soon_until_answered_then_every_quarter_hour() {
        let path = file("due");
        let now = Instant::now();
        let mut access = CallsAccess::default();
        access.follow(Some(whose("42")), &path);
        assert!(access.due(now));
        access.asked = Some(now);
        assert!(!access.due(now + Duration::from_secs(10)) && access.due(now + ASK_AGAIN));
        access.heard(&whose("42"), true, &path);
        assert!(!access.due(now + ASK_AGAIN) && access.due(now + ASK_EVERY));
    }

    #[test]
    fn only_the_feature_s_own_refusal_closes_calls() {
        let e = |status: u16, code: Option<&str>| anyhow::Error::from(ApiError { status, message: String::new(), code: code.map(str::to_string) });
        assert!(not_enabled(&e(403, Some("not_enabled"))));
        assert!(!not_enabled(&e(403, None)) && !not_enabled(&e(403, Some("not_linked"))) && !not_enabled(&anyhow::anyhow!("offline")));
    }
}
