//! Whether this Orbiters account may use N4N0 calls: the per-account feature `n4n0-calls`
//! (`orbiters_core::features::N4N0_CALLS`, contracts `Docs/n4n0-calls.md`, "Who has calls"). [`Controller::calls_open`]
//! is the one place that decides; every way into a call asks it (calling a friend from the window, the overlay, N4N0's
//! menu or the avatar's screen, answering, being rung, the avatar's Call parameters), and without it Call shows greyed
//! out as "Coming soon" everywhere ([`CallLink::ComingSoon`](crate::state::CallLink)).
//!
//! The server says it with the session check (`GET /unity-wizard/token`, `features`), asked again here every
//! [`ASK_EVERY`] (`GET /features/mine`), with `calls_closed` on the event stream, and a call request it refuses with
//! `not_enabled` closes calls at once. Every answer is bound to the identity it was asked with ([`Whose`]: the server,
//! the account and the credential, read together with the credential sent): an answer or a refusal that comes back after
//! another account signed in never touches that account. What the server last said is kept on this computer ([`FILE`])
//! for one use only: a start without Orbiters (offline) goes on a yes of the last [`LAST_KNOWN_FOR`] until the server
//! answers. The server stays the judge: a modified app gains nothing.

use super::Controller;
use crate::state::AccountState;
use orbiters_core::account::{Api, ApiError, Server};
use orbiters_core::features::N4N0_CALLS;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often a signed-in account's calls are asked about again, so a switch an administrator made reaches a running app.
const ASK_EVERY: Duration = Duration::from_secs(15 * 60);
/// After a look that got no answer (offline, the server busy), or before the first answer.
const ASK_AGAIN: Duration = Duration::from_secs(30);
/// How old a remembered yes may be and still open calls while Orbiters cannot be reached.
const LAST_KNOWN_FOR: Duration = Duration::from_secs(24 * 60 * 60);
/// How far in the future a remembered answer's time may be (this computer's clock moving back a little).
const CLOCK_SLACK: u64 = 5 * 60;
/// What a way into a call that is closed says (a notice, N4N0's panel).
pub(super) const CALLS_SOON_TOLD: &str = orbiters_i18n::mark("Calls are coming soon to Orbiters - N4N0. They aren't open for your account yet.");
/// What the server last said, per server and account (`{"production/42": {"allowed": true, "at": 1795000000}}`), in the
/// config folder.
const FILE: &str = "calls-access.json";

/// Who calls are about: the server, the account (its id; a digest of its credential when the account file names none,
/// never a name nor nothing, so two accounts never share what is known), and a digest of the credential (another
/// sign-in starts over). Read together with the credential a request is sent with ([`Controller::calls_identity`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Whose {
    server: Server,
    account: String,
    credential: String,
}

impl Whose {
    pub(super) fn of(server: Server, user: Option<&str>, token: &str) -> Self {
        let credential = orbiters_core::vrchat_accounts::friend_hash(token);
        let account = user.map(str::trim).filter(|u| !u.is_empty()).map(str::to_string).unwrap_or_else(|| format!("credential:{credential}")); // i18n: not shown (a key)
        Self { server, account, credential }
    }

    fn key(&self) -> String {
        format!("{}/{}", self.server.cache_name(), self.account) // i18n: not shown (a key in a file)
    }
}

/// A remembered answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Known {
    allowed: bool,
    /// When the server said it (Unix seconds).
    at: u64,
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What is known of the account's calls.
#[derive(Debug, Default)]
pub(super) struct CallsAccess {
    /// The identity the rest is about.
    whose: Option<Whose>,
    /// What the server said during this run.
    heard: Option<bool>,
    /// What it said last time, read from [`FILE`] when the identity became this one.
    last_known: Option<Known>,
    /// When it was last asked here (the session check is not counted: it is the account's).
    asked: Option<Instant>,
}

impl CallsAccess {
    /// Follows the identity signed in now (`None`: nobody): another one starts from what was last known of its account.
    fn follow(&mut self, whose: Option<Whose>, file: &PathBuf) {
        if self.whose == whose {
            return;
        }
        self.heard = None;
        self.asked = None;
        self.last_known = whose.as_ref().and_then(|w| read_file(file).get(&w.key()).copied());
        self.whose = whose;
    }

    /// Whether calls are open, with the account's state as the window shows it, at Unix second `now`. Signed in: only
    /// what the server said in this run counts. Not answered yet or out of reach (checking, offline): a yes it said less
    /// than [`LAST_KNOWN_FOR`] ago. Anything else (signed out, expired, signing in, nobody known): closed.
    fn open(&self, state: &AccountState, now: u64) -> bool {
        if self.whose.is_none() {
            return false;
        }
        match state {
            AccountState::SignedIn { .. } => self.heard == Some(true),
            // Every yes counts offline only while it is younger than LAST_KNOWN_FOR, one heard in this run too (it is the
            // last known one: `heard` keeps both), and never one dated in the future.
            AccountState::Checking { .. } | AccountState::Offline { .. } => {
                self.last_known.is_some_and(|k| k.allowed && k.at <= now.saturating_add(CLOCK_SLACK) && now.saturating_sub(k.at) < LAST_KNOWN_FOR.as_secs())
            }
            _ => false,
        }
    }

    /// Whether to ask the server now.
    fn due(&self, now: Instant) -> bool {
        self.asked.is_none_or(|t| now.duration_since(t) >= self.every())
    }

    fn every(&self) -> Duration {
        if self.heard.is_some() { ASK_EVERY } else { ASK_AGAIN }
    }

    /// How long until then.
    fn wait(&self) -> Duration {
        super::rest::until(self.asked, self.every())
    }

    /// The server said `allowed` to `whose`: kept for this run, and in the file for the next start. An answer for another
    /// identity than the one followed now is dropped. True when it was taken.
    fn heard(&mut self, whose: &Whose, allowed: bool, file: &PathBuf) -> bool {
        if self.whose.as_ref() != Some(whose) {
            return false;
        }
        self.heard = Some(allowed);
        let known = Known { allowed, at: unix_now() };
        self.last_known = Some(known);
        let mut all = read_file(file);
        all.insert(whose.key(), known);
        if let Err(e) = write_file(file, &all) {
            eprintln!("calls access: {e:#}");
        }
        true
    }
}

/// The remembered answers; one from before they had a time (a plain `true`) counts as nothing.
fn read_file(path: &PathBuf) -> BTreeMap<String, Known> {
    let Some(all) = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<BTreeMap<String, serde_json::Value>>(&b).ok()) else { return BTreeMap::new() };
    all.into_iter().filter_map(|(k, v)| serde_json::from_value(v).ok().map(|known| (k, known))).collect()
}

fn write_file(path: &PathBuf, all: &BTreeMap<String, Known>) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(all)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// What a look at `/features/mine` decides: yes, no (the credential refused too), or nothing (`None`: out of reach, or
/// the server could not read its rules, 503): calls then keep what they had.
fn refresh_outcome(answer: anyhow::Result<bool>) -> Option<bool> {
    match answer {
        Ok(allowed) => Some(allowed),
        Err(e) if e.downcast_ref::<ApiError>().is_some_and(|a| matches!(a.status, 401 | 403 | 404 | 425)) => Some(false),
        Err(e) => {
            eprintln!("calls access: {e:#}");
            None
        }
    }
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

    /// The identity calls are about now, with the server's API and the credential, read together: the server the account
    /// was read for (the chosen one before it was), the account and its credential.
    pub(super) fn calls_identity(&self) -> Option<(Whose, Api, String)> {
        let rt = self.account_runtime.lock().unwrap();
        let account = rt.account.as_ref()?;
        let server = rt.read.map_or_else(Server::current, |(s, _)| s);
        Some((Whose::of(server, account.user.as_deref(), &account.token), Api::new(server), account.token.clone()))
    }

    /// May this account use calls now? The one question every way into a call asks.
    pub(crate) fn calls_open(&self) -> bool {
        let whose = self.calls_identity().map(|(w, ..)| w);
        let state = self.snapshot.lock().unwrap().account.state.clone();
        let file = self.calls_access_file();
        let mut rt = self.calls.lock().unwrap();
        rt.access.follow(whose, &file);
        rt.access.open(&state, unix_now())
    }

    /// What the session check of `token` (on `server`, for account `user`) said of its features (`None`: nothing, an
    /// older server). Taken only while that is still the identity signed in.
    pub(super) fn calls_access_checked(&self, server: Server, user: Option<&str>, token: &str, features: Option<&BTreeMap<String, bool>>) {
        // Not said (an older server, or the rules could not be read and the feature was left out): nothing changes.
        let Some(allowed) = features.and_then(|f| f.get(N4N0_CALLS).copied()) else { return };
        let whose = Whose::of(server, user, token);
        if self.calls_identity().map(|(w, ..)| w).as_ref() != Some(&whose) {
            return;
        }
        let file = self.calls_access_file();
        let mut rt = self.calls.lock().unwrap();
        rt.access.follow(Some(whose.clone()), &file);
        rt.access.heard(&whose, allowed, &file);
    }

    /// Orbiters closed calls for the credential `token` (a request refused with `not_enabled`, or `calls_closed` on its
    /// event stream). Nothing happens unless that is the credential signed in now: then calls close at once, whatever of
    /// them was shown goes (friends to call, N4N0's call menu) and a call under way ends. True when it was taken.
    pub(super) fn calls_access_denied(self: &super::Shared, token: &str) -> bool {
        let Some((whose, ..)) = self.calls_identity() else { return false };
        if whose.credential != orbiters_core::vrchat_accounts::friend_hash(token) {
            return false;
        }
        let file = self.calls_access_file();
        {
            let mut rt = self.calls.lock().unwrap();
            rt.access.follow(Some(whose.clone()), &file);
            rt.access.heard(&whose, false, &file);
        }
        self.calls_closed_here(token);
        true
    }

    /// Asks the server about the calls of the identity signed in when it is time to, with that identity's own credential;
    /// the answer is kept for that identity only. On the calls thread: the answer can take a moment.
    pub(super) fn calls_access_refresh(&self) {
        // Only an account read for a server is asked about (before that, its server is not settled).
        if self.account_runtime.lock().unwrap().read.is_none() {
            return;
        }
        let Some((whose, api, token)) = self.calls_identity() else { return };
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
        let Some(answer) = refresh_outcome(api.member_feature(&token, N4N0_CALLS)) else { return };
        self.calls.lock().unwrap().access.heard(&whose, answer, &file);
    }

    /// How long until [`Self::calls_access_refresh`] asks again (never, with nobody signed in).
    pub(super) fn calls_access_wait(&self) -> Option<Duration> {
        self.calls_identity()?;
        Some(self.calls.lock().unwrap().access.wait())
    }
}
#[cfg(test)]
pub(super) mod tests {
    use super::*;

    fn whose(user: &str) -> Whose {
        Whose::of(Server::Production, Some(user), &format!("orbit-{user}"))
    }
    fn signed_in() -> AccountState {
        AccountState::SignedIn { username: "Nova".into() }
    }
    fn offline() -> AccountState {
        AccountState::Offline { username: None }
    }

    fn file(name: &str) -> PathBuf {
        let dir = orbiters_core::paths::test_roots().config.join(format!("calls-access-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(FILE)
    }

    /// Off unless the server said so for this identity: signed out, not known yet, refused, another account, or
    /// offline with nothing known; offline with a recent yes goes on it until the server answers.
    #[test]
    fn calls_are_open_only_on_the_server_s_word_for_this_account() {
        let path = file("word");
        let now = unix_now();
        let mut access = CallsAccess::default();
        assert!(!access.open(&AccountState::SignedOut, now), "nobody");
        access.follow(Some(whose("42")), &path);
        for state in [AccountState::Unknown, AccountState::SignedOut, AccountState::SigningIn, AccountState::Expired { username: None }, signed_in()] {
            assert!(!access.open(&state, now), "not known yet: {state:?}");
        }
        assert!(!access.open(&offline(), now), "offline with nothing known");
        assert!(access.heard(&whose("42"), true, &path));
        assert!(access.open(&signed_in(), now));
        assert!(!access.open(&AccountState::Expired { username: None }, now), "an expired sign-in has no calls");
        // Another account (or another server) starts from what was known of it: nothing.
        access.follow(Some(whose("7")), &path);
        assert!(!access.open(&signed_in(), now));
        assert!(!access.heard(&whose("42"), true, &path), "an answer about the account before is dropped");
        // Back to the first, at the next start: only "last known", used while Orbiters does not answer.
        let mut next = CallsAccess::default();
        next.follow(Some(whose("42")), &path);
        assert!(next.open(&offline(), now) && next.open(&AccountState::Checking { username: None }, now));
        assert!(!next.open(&signed_in(), now), "signed in, the server's word of this run is needed");
        // Switched off: closed at once, and remembered so.
        next.heard(&whose("42"), false, &path);
        assert!(!next.open(&signed_in(), now));
        let mut after = CallsAccess::default();
        after.follow(Some(whose("42")), &path);
        assert!(!after.open(&offline(), now));
        assert_eq!(read_file(&path).get("production/42").map(|k| k.allowed), Some(false));
    }

    /// Verify A2: what is known belongs to one identity. The same account with another credential starts over; an account
    /// file without an id is told apart by its credential, never by a name or nothing; a yes remembered for offline starts
    /// lasts a day; a file from before the times counts as nothing.
    #[test]
    fn what_is_known_belongs_to_one_identity_and_a_remembered_yes_expires() {
        let path = file("identity");
        let a = Whose::of(Server::Production, None, "orbit-a");
        let b = Whose::of(Server::Production, Some(" "), "orbit-b");
        assert_ne!(a, b);
        assert_ne!(a.key(), b.key());
        assert!(a.key().starts_with("production/credential:") && !a.key().ends_with('/'));
        assert_ne!(Whose::of(Server::Production, Some("42"), "orbit-1"), Whose::of(Server::Production, Some("42"), "orbit-2"), "another sign-in");
        let mut access = CallsAccess::default();
        access.follow(Some(a.clone()), &path);
        access.heard(&a, true, &path);
        let mut other = CallsAccess::default();
        other.follow(Some(b.clone()), &path);
        assert!(!other.open(&offline(), unix_now()), "B never inherits A's yes");
        // A day later, offline: no more.
        let mut later = CallsAccess::default();
        later.follow(Some(a.clone()), &path);
        let at = later.last_known.unwrap().at;
        assert!(later.open(&offline(), at + 3600));
        assert!(!later.open(&offline(), at + LAST_KNOWN_FOR.as_secs()));
        std::fs::write(&path, br#"{"production/42": true}"#).unwrap();
        let mut old = CallsAccess::default();
        old.follow(Some(whose("42")), &path);
        assert!(!old.open(&offline(), unix_now()), "an old file's yes has no time: nothing");
    }

    /// Verify2 S7: offline, every yes is bounded: one heard in this run expires like a remembered one, and a time in the
    /// future counts as nothing.
    #[test]
    fn every_offline_yes_expires_and_none_comes_from_the_future() {
        let path = file("bounded");
        let mut access = CallsAccess::default();
        access.follow(Some(whose("42")), &path);
        access.heard(&whose("42"), true, &path);
        let at = access.last_known.unwrap().at;
        assert!(access.open(&offline(), at + 3600));
        assert!(!access.open(&offline(), at + LAST_KNOWN_FOR.as_secs()), "heard in this run, a day ago: no more");
        assert!(access.open(&signed_in(), at + LAST_KNOWN_FOR.as_secs()), "signed in, this run's word stands");
        access.last_known = Some(Known { allowed: true, at: at + 2 * 24 * 3600 });
        assert!(!access.open(&offline(), at), "dated in the future");
    }

    /// Verify3 (fix2-3): a server that cannot read its rules says 503, and a session check leaves the feature out: neither is
    /// a no, so a healthy call is never hung up for it.
    #[test]
    fn an_answer_that_is_not_known_keeps_what_calls_had() {
        let api_error = |status: u16| anyhow::Error::from(ApiError { status, message: String::new(), code: None });
        assert_eq!(refresh_outcome(Ok(true)), Some(true));
        assert_eq!(refresh_outcome(Err(api_error(503))), None, "not known");
        assert_eq!(refresh_outcome(Err(anyhow::anyhow!("offline"))), None);
        assert_eq!(refresh_outcome(Err(api_error(401))), Some(false), "the credential refused");
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
