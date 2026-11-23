//! Whose what VRChat says is (verify3-r21 B6), and the one way it gets into the shared state.
//!
//! Everything that comes from a VRChat account (who is signed in, their name and picture, their friends, their own
//! profile, a profile read with their session, a picture being replaced) was read for an Orbiters member, of an
//! environment, at the identity's epoch then ([`Stamp`]: controller/identity.rs's one counter, so a member who left and
//! came back is another time), and for a VRChat account ([`By`]). A producer takes its stamp in the step that gives it its
//! client ([`Controller::vrchat_session`], [`Controller::vrchat_ready`]) and hands it to [`Controller::publish_login`] or
//! [`Controller::publish_launcher`]: with the state held, the value is written only while that stamp is the current one,
//! and the state says whom it was verified for. A value whose member left meanwhile is dropped, never relabelled.
//!
//! The same is checked at every publish ([`enforce`]): what is not the current member's does not go out, whatever wrote
//! it. Nothing else in the controller writes these parts (a test reads the sources for it): what this computer alone says
//! (the game, its profiles, the engine) goes through [`Controller::publish_machine`], and what only takes away through
//! [`Controller::forget_login`] and [`Controller::forget_launcher`]. Groups, and whatever else is read with a VRChat
//! session, belong here too.

use super::vrchat_login::{Grounds, Runtime, Status};
use super::Controller;
use crate::state::{AccountState, AccountStatus, Snapshot};
use orbiters_core::account::{Account, Api, Server};
use orbiters_core::launcher::{EngineSetup, EngineTrouble, Game, GameProfile, LauncherState, Notice, Source, StartTrouble, VerifiedFor};
use orbiters_core::vrchat_api::Client;
use std::sync::{Arc, Mutex};

/// Whose a VRChat account, its name and picture are to see: an Orbiters member of an environment, by the id Orbiters gives
/// them ([`Owner::of`]). Digests; nothing here is a name or a credential.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Owner {
    /// The environment and the member's id, when the account says one: what the cache on disk is kept for. Ids are
    /// Orbiters' own (digits, as text): compared exactly, with no case or Unicode folding; a name, which can be reused,
    /// changed or missing, has no part in it.
    pub(super) member: Option<String>,
    /// The environment and the credential, in memory only: the same account before its id is known (an older file).
    pub(super) credential: String,
}

impl Owner {
    pub(super) fn of(server: Server, account: &Account) -> Self {
        use sha2::{Digest, Sha256};
        let env = if server == Server::Development { "development" } else { "production" };
        let digest = |kind: &str, text: &str| -> String {
            let d = Sha256::digest(format!("{env}\n{kind}\n{text}").as_bytes());
            d.iter().take(16).map(|b| format!("{b:02x}")).collect()
        };
        let id = account.user.as_deref().map(str::trim).filter(|id| !id.is_empty());
        Self { member: id.map(|id| digest("member", id)), credential: digest("credential", &account.token) }
    }

    /// The same member: by their ids; by the credential while one of them has no id.
    pub(super) fn same(&self, other: &Owner) -> bool {
        match (&self.member, &other.member) {
            (Some(a), Some(b)) => a == b,
            _ => self.credential == other.credential,
        }
    }
}

/// The Orbiters member something was read for, and the identity's epoch then.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Stamp {
    pub(super) epoch: u64,
    pub(super) owner: Owner,
}

impl Stamp {
    pub(super) fn same(&self, other: &Stamp) -> bool {
        self.epoch == other.epoch && self.owner.same(&other.owner)
    }

    fn said(&self, user: Option<&str>) -> VerifiedFor {
        VerifiedFor { epoch: self.epoch, member: self.owner.member.clone(), user: user.map(str::to_string) }
    }
}

/// A VRChat account, as verified for an Orbiters member.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct By {
    pub(super) stamp: Stamp,
    pub(super) user: String,
}

/// A VRChat session the Orbiters member signed in now may use: its client, and whose it is, taken together.
pub(super) struct Session {
    pub(super) client: Arc<Mutex<Client>>,
    pub(super) by: By,
}

fn same(a: Option<&Stamp>, b: Option<&Stamp>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a.same(b))
}

/// Whose the account installed is, of the environment chosen; none while who it is is unknown (its file could not be read,
/// the environment is being switched: controller/identity.rs).
fn installed(rt: &super::account::AccountRuntime) -> Option<(Server, Owner)> {
    let (server, _) = rt.read.filter(|(server, _)| !rt.unknown && *server == Server::current())?;
    rt.account.as_ref().map(|a| (server, Owner::of(server, a)))
}

/// What this computer alone says on Home's launcher: the game, its profiles and what runs it. Nobody's account.
pub(super) struct MachineParts {
    pub(super) game: Game,
    pub(super) play: bool,
    pub(super) profiles: Vec<GameProfile>,
    pub(super) unresolved: u32,
    pub(super) start_trouble: Option<StartTrouble>,
    pub(super) picture_lost: Option<String>,
    pub(super) engine_trouble: Option<EngineTrouble>,
    pub(super) engine_setup: Option<EngineSetup>,
}

impl MachineParts {
    fn of(l: &LauncherState) -> Self {
        Self {
            game: l.game.clone(),
            play: l.possible.play,
            profiles: l.profiles.clone(),
            unresolved: l.unresolved,
            start_trouble: l.start_trouble.clone(),
            picture_lost: l.picture_lost.clone(),
            engine_trouble: l.engine_trouble.clone(),
            engine_setup: l.engine_setup.clone(),
        }
    }

    fn into(self, l: &mut LauncherState) {
        l.game = self.game;
        l.possible.play = self.play;
        l.profiles = self.profiles;
        l.unresolved = self.unresolved;
        l.start_trouble = self.start_trouble;
        l.picture_lost = self.picture_lost;
        l.engine_trouble = self.engine_trouble;
        l.engine_setup = self.engine_setup;
    }
}

/// A launcher with nobody's list: what this computer says, and nothing of an account.
fn blank(l: &LauncherState) -> LauncherState {
    let mut out = LauncherState { loading: matches!(l.source, Source::SignedIn { .. }), ..LauncherState::default() };
    MachineParts::of(l).into(&mut out);
    out
}

/// The sign-in's state as the Orbiters account goes: nobody signed in to Orbiters (or its credential refused) and nothing of
/// VRChat shows as usable; the environment switching (its account not read yet), and the session shows as kept and
/// waiting, not signed out (r20).
fn follow_orbiters(st: &mut Status, grounds: Grounds) {
    match grounds {
        Grounds::Gone if st.allowed || st.user.is_some() || st.waiting || st.busy => {
            let known = st.user.take().or(st.known.take());
            *st = Status { revision: st.revision, known, upload: st.upload.take(), ..Status::default() };
        }
        Grounds::Switching if st.allowed || st.user.is_some() => {
            let known = st.user.take().or(st.known.take());
            *st = Status { revision: st.revision, waiting: true, restoring: true, known, picture: st.picture.take(), upload: st.upload.take(), ..Status::default() };
        }
        _ => {}
    }
}

/// Takes out of the status what is not `now`'s: the account, its name and picture, what was being done with it.
fn drop_foreign(st: &mut Status, rt: &mut Runtime, now: Option<&Stamp>) {
    // Tests put a status in place by hand now and then (nothing ever said whose it is).
    if rt.status_for.is_none() && cfg!(test) {
        return;
    }
    // What could not be removed of a session from this computer is said to whoever is at it.
    let local = st.recovery == super::vrchat_login::Recovery::RemoveSavedSession;
    let theirs = st.user.is_some() || st.known.is_some() || st.picture.is_some() || st.upload.is_some() || !st.methods.is_empty() || (st.error.is_some() && !local);
    if same(rt.status_for.as_ref(), now) || !(theirs || st.allowed) {
        return;
    }
    // A session there was goes on being connected, plainly (its file and its memory are kept for a next grant).
    let had = st.allowed || st.user.is_some() || st.waiting || st.restoring;
    if st.allowed || st.user.is_some() {
        rt.suspend();
    }
    let (error, recovery) = if local { (st.error.take(), st.recovery.clone()) } else { Default::default() };
    *st = Status { revision: st.revision, waiting: had, restoring: had, error, recovery, ..Status::default() };
}

/// At each publish, with the state held, before it goes out: what is not the current member's of VRChat does not
/// (`now`: the member this very state is for; none, when nobody's account is known).
pub(super) fn enforce(s: &mut Snapshot, rt: &mut Runtime, now: Option<&Stamp>) {
    let grounds = rt.follow_account(&s.account);
    follow_orbiters(&mut s.vrchat_login, grounds);
    drop_foreign(&mut s.vrchat_login, rt, now);
    if let Source::SignedIn { user_id, .. } = &s.launcher.source {
        let known = rt.list_for.is_some() || !cfg!(test);
        let login = &s.vrchat_login;
        // Signed in as that account, or on the way back to it (the session waits for Orbiters).
        let account = login.user.as_ref().map_or(login.waiting || login.restoring, |u| u.id == *user_id);
        if known && !(same(rt.list_for.as_ref(), now) && account) {
            let blank = blank(&s.launcher);
            *s.launcher = blank;
            rt.list_for = None;
        }
    }
}

impl Controller {
    /// Whom what is read from VRChat now is for: the Orbiters account installed (at launch, the one its file names), while
    /// the one on show is of the environment chosen; none while no account is known (being read, signed out, refused).
    pub(super) fn vrchat_now(&self) -> Option<Stamp> {
        self.vrchat_stamp_of(&self.view().account)
    }

    /// [`Self::vrchat_now`] for the account state `shown` (about to be published: the snapshot may be held, nothing else of
    /// the account's or of VRChat's).
    pub(super) fn vrchat_stamp_of(&self, shown: &AccountStatus) -> Option<Stamp> {
        if matches!(shown.state, AccountState::Unknown | AccountState::SignedOut | AccountState::Expired { .. }) {
            return None;
        }
        if shown.development != (Server::current() == Server::Development) {
            return None;
        }
        let (read, owner, epoch) = {
            let rt = self.account_runtime.lock().unwrap();
            // The epoch moves under this lock: it is the account's beside it.
            (rt.read.is_some() || rt.unknown, installed(&rt).map(|(_, owner)| owner), self.identity_epoch())
        };
        let mut login = self.vrchat_login.lock().unwrap();
        if read {
            // The account thread read it (or could not): from here on only what is installed counts.
            login.at_launch = None;
        }
        owner.or_else(|| login.at_launch.clone()).map(|owner| Stamp { epoch, owner })
    }

    /// The Orbiters account VRChat may be asked for now: its API and credential, and whom that is for, taken together.
    pub(super) fn vrchat_ready(&self) -> Option<(Api, String, Stamp)> {
        let view = self.view();
        if view.account.development != (Server::current() == Server::Development) {
            return None;
        }
        if matches!(view.account.state, AccountState::Unknown | AccountState::SignedOut | AccountState::Expired { .. }) {
            return None;
        }
        let rt = self.account_runtime.lock().unwrap();
        let (server, owner) = installed(&rt)?;
        let token = rt.account.as_ref()?.token.clone();
        Some((Api::new(server), token, Stamp { epoch: self.identity_epoch(), owner }))
    }

    /// The VRChat session signed in and allowed for the Orbiters member now, and whose it is.
    pub(super) fn vrchat_session(&self) -> Option<Session> {
        let (client, stamp) = self.vrchat_granted()?;
        let user = client.lock().unwrap().signed_in_user_id().map(str::to_owned)?;
        Some(Session { client, by: By { stamp, user } })
    }

    /// The sign-in's state, changed by what was read for `stamp`, only while that is whom it is for now. Whether it was
    /// written.
    pub(super) fn publish_login(&self, stamp: &Stamp, f: impl FnOnce(&mut Status, &mut Runtime)) -> bool {
        let mut written = false;
        self.update_if(|s| {
            let now = self.vrchat_stamp_of(&s.account);
            if !same(now.as_ref(), Some(stamp)) {
                return false;
            }
            let mut rt = self.vrchat_login.lock().unwrap();
            let before = s.vrchat_login.clone();
            drop_foreign(&mut s.vrchat_login, &mut rt, now.as_ref());
            f(&mut s.vrchat_login, &mut rt);
            let st = &mut s.vrchat_login;
            rt.status_for = Some(stamp.clone());
            let user = st.user.as_ref().or(st.known.as_ref()).map(|u| u.id.clone());
            st.verified_for = user.is_some().then(|| stamp.said(user.as_deref()));
            written = true;
            *st != before
        });
        written
    }

    /// The sign-in's state with something taken away, or said of nobody's account (busy, connecting): whatever it still
    /// shows of one is what it showed.
    pub(super) fn forget_login(&self, f: impl FnOnce(&mut Status)) {
        self.update_if(|s| {
            let before = s.vrchat_login.clone();
            let st = &mut s.vrchat_login;
            f(st);
            if st.user != before.user {
                st.user = None;
            }
            if st.known != before.known && st.known != before.user {
                st.known = None;
            }
            if st.picture != before.picture {
                st.picture = None;
            }
            if st.user.is_none() && st.known.is_none() {
                st.verified_for = None;
            }
            *st != before
        });
    }

    /// Home's list, changed by what `by`'s session gave, only while that is the session of the member signed in now (and
    /// the list on show, if any, is that account's). Whether it was written. What this computer says stays as it is.
    pub(super) fn publish_launcher(&self, by: &By, f: impl FnOnce(&mut LauncherState)) -> bool {
        let mut written = false;
        self.update_if(|s| {
            let now = self.vrchat_stamp_of(&s.account);
            let login = &s.vrchat_login;
            let account = match &login.user {
                Some(u) => u.id == by.user,
                // Connecting again: only onto the list that account already has on show.
                None => (login.waiting || login.restoring) && matches!(&s.launcher.source, Source::SignedIn { user_id, .. } if *user_id == by.user),
            };
            if !(same(now.as_ref(), Some(&by.stamp)) && account) {
                return false;
            }
            let mut rt = self.vrchat_login.lock().unwrap();
            let mut state = (*s.launcher).clone();
            f(&mut state);
            MachineParts::of(&s.launcher).into(&mut state);
            if let Source::SignedIn { user_id, .. } = &state.source {
                if *user_id != by.user {
                    return false;
                }
                rt.list_for = Some(by.stamp.clone());
                state.verified_for = Some(by.stamp.said(Some(&by.user)));
            }
            written = true;
            let changed = state != *s.launcher;
            if changed {
                *s.launcher = state;
            }
            changed
        });
        written
    }

    /// Home's list as VRCX's or VRCNext's copy on this computer gives it, changed: only while no VRChat account is signed in
    /// here (a list read with a session is never turned into it, nor it into one).
    pub(super) fn publish_local_list(&self, f: impl FnOnce(&mut LauncherState)) -> bool {
        self.update_if(|s| {
            if s.vrchat_login.user.is_some() {
                return false;
            }
            let mut state = (*s.launcher).clone();
            f(&mut state);
            MachineParts::of(&s.launcher).into(&mut state);
            if matches!(state.source, Source::SignedIn { .. }) && state.source != s.launcher.source {
                return false;
            }
            if !matches!(state.source, Source::SignedIn { .. }) {
                state.verified_for = None;
                self.vrchat_login.lock().unwrap().list_for = None;
            }
            let changed = state != *s.launcher;
            if changed {
                *s.launcher = state;
            }
            changed
        })
    }

    /// Home's list with something taken away (a profile closed, an invite put away), or whether it is being read.
    pub(super) fn forget_launcher(&self, f: impl FnOnce(&mut LauncherState)) {
        self.update_if(|s| {
            let mut state = (*s.launcher).clone();
            f(&mut state);
            let before = &*s.launcher;
            // Taken away only: nothing of an account comes in this way.
            debug_assert!(state.friends.len() <= before.friends.len() && state.invites.len() <= before.invites.len() && state.boops.len() <= before.boops.len());
            debug_assert!(state.source == before.source && (state.profile.is_none() || state.profile == before.profile) && state.me == before.me && state.mine == before.mine);
            let changed = state != *s.launcher;
            if changed {
                *s.launcher = state;
            }
            changed
        });
    }

    /// Nobody's list: what this computer says stays. `only`: while the list on show is as it says.
    pub(super) fn blank_launcher(&self, only: impl FnOnce(&LauncherState) -> bool) -> bool {
        self.update_if(|s| {
            if !only(&s.launcher) {
                return false;
            }
            let blank = blank(&s.launcher);
            *s.launcher = blank;
            true
        })
    }

    /// What this computer says on Home's launcher; whether it changed.
    pub(super) fn publish_machine(&self, f: impl FnOnce(&mut MachineParts)) -> bool {
        self.update_if(|s| {
            let mut parts = MachineParts::of(&s.launcher);
            f(&mut parts);
            let mut state = (*s.launcher).clone();
            parts.into(&mut state);
            let changed = state != *s.launcher;
            if changed {
                *s.launcher = state;
            }
            changed
        })
    }

    /// An action's outcome, shown to the member who asked for it (`asked`: who it was then), or taken back.
    pub(super) fn publish_notice(&self, asked: Option<&Stamp>, f: impl FnOnce(Option<&Notice>) -> Option<Option<Notice>>) {
        self.update_if(|s| {
            let now = self.vrchat_stamp_of(&s.account);
            let still = match (asked, &now) {
                (None, None) => true,
                (a, b) => same(a, b.as_ref()),
            };
            match f(s.launcher.notice.as_ref()).filter(|_| still) {
                Some(next) if next != s.launcher.notice => {
                    s.launcher.notice = next;
                    true
                }
                _ => false,
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(token: &str, id: Option<&str>) -> Account {
        Account { token: token.into(), user: id.map(str::to_string), username: Some("tester".into()), avatar_url: None, features: None }
    }

    /// verify2-r21 M2: who it belongs to: the member's id in its environment (exactly, spaces around it aside); the
    /// credential only while one of the two has no id.
    #[test]
    fn verify2_the_owner_is_the_member_id_in_its_environment() {
        let of = |server, token: &str, id: Option<&str>| Owner::of(server, &account(token, id));
        let (prod, dev) = (Server::Production, Server::Development);
        assert!(of(prod, "a", Some("1")).same(&of(prod, "b", Some(" 1 "))), "the same member, another credential");
        assert!(!of(prod, "a", Some("1")).same(&of(prod, "a", Some("2"))), "another member, whatever the credential");
        assert!(!of(prod, "a", Some("1")).same(&of(dev, "a", Some("1"))), "another environment");
        assert!(of(prod, "a", None).same(&of(prod, "a", Some("1"))), "its id learned later");
        assert!(!of(prod, "a", None).same(&of(prod, "b", Some("1"))) && !of(prod, "a", None).same(&of(prod, "b", None)));
        assert_eq!(of(prod, "a", Some("  ")).member, None, "no id is no member: nothing is kept on disk for it");
        // The same member at another time of the identity: what was read before they left is not theirs now.
        let at = |epoch| Stamp { epoch, owner: of(prod, "a", Some("1")) };
        assert!(at(3).same(&at(3)) && !at(3).same(&at(5)));
    }

    /// Where a line writes VRChat's parts of the shared state.
    fn writes(line: &str) -> bool {
        let code = line.split("//").next().unwrap_or_default();
        ["launcher", "vrchat_login"].iter().any(|part| {
            let mut rest = code;
            while let Some(i) = rest.find(&format!(".{part}")) {
                let before = &rest[..i];
                let after = &rest[i + part.len() + 1..];
                rest = after;
                // Another name that only starts so, or the runtimes' own locks.
                if after.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '(') || after.starts_with(".lock()") || ["self", "launch"].iter().any(|o| before.trim_end().ends_with(o)) {
                    continue;
                }
                // The path that follows: `.friends`, `.user`, `[0]`…
                let path: String = after.chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '.')).collect();
                let tail = after[path.len()..].trim_start();
                let assigned = tail.starts_with('=') && !tail.starts_with("==") && !tail.starts_with("=>");
                let compound = ["+=", "-=", "|=", "&="].iter().any(|op| tail.starts_with(op));
                let borrowed = {
                    let name_start = before.rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
                    let lead = before[..name_start].trim_end();
                    let place = lead.trim_end_matches('*');
                    place.trim_end().ends_with("&mut") || place != lead && assigned
                };
                let changing = ["push", "retain", "take", "insert", "clear", "get_or_insert_with", "get_or_insert_default", "as_mut", "iter_mut", "sort_by_key", "sort", "replace", "extend", "truncate", "remove", "drain", "pop", "dedup"]
                    .iter()
                    .any(|m| path.ends_with(&format!(".{m}")) && tail.starts_with('('));
                if assigned || compound || borrowed || changing {
                    return true;
                }
            }
            false
        })
    }

    /// Nothing but this file writes what VRChat says into the shared state (verify3-r21 B6): the controller's sources,
    /// read for a writer elsewhere.
    #[test]
    fn nothing_else_writes_vrchats_parts_of_the_state() {
        // The reading of the lines itself.
        for wrong in [
            "s.launcher.friends = vec![];",
            "*s.launcher = state;",
            "s.vrchat_login.user = Some(u);",
            "let st = &mut s.vrchat_login;",
            "s.launcher.invites.retain(|i| i.id != id);",
            "s.launcher.profile.take().is_some()",
            "cooldowns.mark(&mut s.launcher.friends, now());",
            "let mine = s.launcher.mine.get_or_insert_with(MyProfile::default);",
            "if let Some(me) = s.launcher.me.as_mut() {",
            "s.vrchat_login.revision += 1;",
            "std::mem::replace(&mut s.launcher.loading, true)",
            "let list = &mut *s.launcher;",
            "Arc::make_mut(&mut view.vrchat_login).user = None;",
            "s.launcher.friends.iter_mut().for_each(|f| f.name.clear());",
            "*st = Status::default(); s.vrchat_login = st;",
        ] {
            assert!(writes(wrong), "a writer not seen: {wrong}");
        }
        for fine in [
            "let found = self.view().launcher.invites.iter().find(|i| i.id == invite).cloned();",
            "if s.launcher.source == state.source {",
            "match &self.view().launcher.source {",
            "let mut rt = self.launcher.lock().unwrap();",
            "self.vrchat_login.lock().unwrap().at_launch = None;",
            "user: s.vrchat_login.user.as_ref().map(|u| u.id.clone()),",
            "let same = s.launcher.notice.as_ref() == Some(notice);",
            "ctl.launcher_notice(Notice::Error(why))",
            "Source::SignedIn { .. } => s.launcher.friends.len() >= 2,",
            "// s.launcher.friends = vec![]; (said in a comment)",
            "launch.launcher = pid;",
        ] {
            assert!(!writes(fine), "not a writer: {fine}");
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![root.join("controller.rs"), root.join("service.rs")];
        let mut dirs = vec![root.join("controller")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    files.push(path);
                }
            }
        }
        assert!(files.len() > 40, "the controller's sources: {}", files.len());
        let mut found = Vec::new();
        for file in files.iter().filter(|f| f.is_file()) {
            let name = file.file_name().unwrap().to_string_lossy().to_string();
            if name == "vrchat_scope.rs" || name.ends_with("_tests.rs") || name == "tests.rs" {
                continue;
            }
            let text = std::fs::read_to_string(file).unwrap();
            let lines: Vec<&str> = text.lines().collect();
            // What tests of the file do by hand is theirs: they come last, after `#[cfg(test)]` on a module or an impl.
            for (i, line) in lines.iter().enumerate() {
                let next = lines.get(i + 1).copied().unwrap_or_default();
                if line.trim() == "#[cfg(test)]" && (next.starts_with("mod ") || next.starts_with("pub mod ") || next.starts_with("impl ")) {
                    break;
                }
                // A chain over several lines is read as one.
                let chain: String = std::iter::once(*line).chain(lines[i + 1..].iter().map(|l| l.trim()).take_while(|l| l.starts_with('.'))).collect();
                if writes(&chain) {
                    found.push(format!("{name}:{}: {}", i + 1, line.trim()));
                }
            }
        }
        assert!(found.is_empty(), "written outside controller/vrchat_scope.rs (publish_login, publish_launcher…):\n{}", found.join("\n"));
    }
}
