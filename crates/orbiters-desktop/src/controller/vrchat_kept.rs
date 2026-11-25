//! What was last read from VRChat, kept on this computer by the service (REQUEST28, `orbiters_core::vrchat_store`): the
//! friends list as it is on show, the worlds' names, the groups' pages. Written a moment after it changed and when the
//! service quits; shown at the next start before any window asks, to the Orbiters member and for the VRChat account it is
//! of, and to no other (controller/vrchat_scope.rs puts it on show).
//!
//! Only what is verified is kept: a list whose `verified_for` is a member's account, while VRChat says that account is
//! signed in. Signing out removes it all ([`Controller::kept_forget`]); a write begun before that writes nothing after.

use super::vrchat_data::Scope;
use super::vrchat_login::Bell;
use super::vrchat_scope::{By, Stamp};
use super::{groups, Controller, Shared};
use orbiters_core::group_events::{self, CalendarEvent};
use orbiters_core::groups::GroupCard;
use orbiters_core::launcher::{LauncherState, Possible, Source};
use orbiters_core::vrchat_store::{self as store, kind, Account, Change, Record, Store, World, ONLY};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Changes are gathered this long before they are written.
const GATHER: Duration = Duration::from_millis(1500);
/// When the list was last live is written again this often while nothing else is.
const SEEN_EVERY: i64 = 300;
/// A group's events kept, the latest.
const EVENTS_KEPT: usize = 300;
/// The folder VRChat's pictures were kept in before 0.3.10 (`worlds::profile_pictures` moves what is shown of it), and how
/// long what is not shown stays there.
const BEFORE: &str = "profile-pictures";
const BEFORE_FOR: Duration = Duration::from_secs(60 * 86_400);
/// A world's name and picture older than this are asked again when a place of the list is there (as each was in every run).
const WORLD_FRESH: i64 = 86_400;

pub(super) struct Keeper {
    /// Rung when the list on show changed.
    pub(super) bell: Arc<Bell>,
    rt: Mutex<Runtime>,
}

impl Default for Keeper {
    fn default() -> Self {
        // Tests keep nothing unless they give a folder of their own.
        Self { bell: Arc::default(), rt: Mutex::new(Runtime { root: (!cfg!(test)).then(store::root), open: None, forgets: 0 }) }
    }
}

struct Runtime {
    root: Option<PathBuf>,
    open: Option<Open>,
    /// Bumped at each sign-out: what was begun before it writes nothing.
    forgets: u64,
}

struct Open {
    store: Store,
    /// The list, the worlds and the decorations as they are written.
    list: Option<Arc<LauncherState>>,
    worlds: HashMap<String, World>,
    decorations: HashMap<String, String>,
    seen: i64,
}

impl Runtime {
    /// The account's store, for writing: its folder made if it has none.
    fn open(&mut self, account: &Account) -> Option<&mut Open> {
        let root = self.root.clone()?;
        if self.open.as_ref().is_none_or(|o| o.store.account() != account || !store::there(&root, account)) {
            self.open = None;
            if !store::begin(&root, account) {
                return None;
            }
            let s = Store::open(&root, account)?;
            let worlds = s.worlds().into_iter().map(|(id, r)| (id, r.value)).collect();
            let (list, decorations) = (s.launcher().map(Arc::new), s.decorations());
            self.open = Some(Open { store: s, list, worlds, decorations, seen: 0 });
        }
        self.open.as_mut()
    }
}

/// What a list starts with when its account is there again.
pub(super) struct Seed {
    pub list: Option<LauncherState>,
    /// World id → its name and picture; those to ask again.
    pub worlds: HashMap<String, (String, String)>,
    pub old_worlds: Vec<String>,
    pub decorations: HashMap<String, String>,
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// What nobody showed for [`BEFORE_FOR`] of the pictures kept before goes; the folder with the last of it.
fn leave_before(dir: &std::path::Path) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let unused = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        if unused.is_some_and(|age| age > BEFORE_FOR) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    let _ = std::fs::remove_dir(dir);
}

/// The requests the records are read by.
fn source(request: &groups::Request) -> &'static str {
    match request {
        groups::Request::Discover | groups::Request::Hydrate { .. } => "my groups", // i18n: not shown (a record's words)
        groups::Request::Open { .. } => "a group",                                  // i18n: not shown (a record's words)
        groups::Request::Members { .. } => "a group's members",                     // i18n: not shown (a record's words)
        groups::Request::Events { .. } => "a group's calendar",                     // i18n: not shown (a record's words)
        groups::Request::KeepEvents { .. } => "Orbiters' events",                   // i18n: not shown (a record's words)
        _ => "a group's posts",                                                     // i18n: not shown (a record's words)
    }
}

impl Controller {
    fn kept(&self) -> std::sync::MutexGuard<'_, Runtime> {
        self.vrchat_kept.rt.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The writing, on a thread of its own.
    pub(super) fn start_vrchat_kept(self: &Shared) {
        let Some(root) = self.kept().root.clone() else { return };
        let weak = Arc::downgrade(self);
        let bell = self.vrchat_kept.bell.clone();
        let _ = std::thread::Builder::new().name("vrchat-kept".into()).spawn(move || {
            store::sweep(&root);
            leave_before(&root.with_file_name(BEFORE));
            let mut heard = 0;
            loop {
                heard = bell.wait(heard, Duration::from_secs(SEEN_EVERY as u64));
                std::thread::sleep(GATHER);
                let Some(ctl) = weak.upgrade() else { break };
                ctl.kept_save(false);
            }
        });
    }

    /// The list on show as it is now, into its account's store, where it differs from what is there; `leaving`: the service
    /// quits.
    pub(super) fn kept_save(&self, leaving: bool) {
        let forgets = self.kept().forgets;
        let view = self.view();
        let list = view.launcher.arc();
        // VRChat's own list, as VRChat says who is signed in: one shown from here, or waiting, is written already.
        if list.kept.is_some() {
            return;
        }
        let Some(account) = list.verified_for.as_ref().and_then(Account::verified) else { return };
        let signed_in = view.vrchat_login.user.as_ref().is_some_and(|u| u.id == account.user()) && view.vrchat_login.allowed;
        if !signed_in || !matches!(&list.source, Source::SignedIn { user_id, .. } if user_id == account.user()) {
            return;
        }
        let (worlds, decorations) = self.launcher_lookups();
        let mut rt = self.kept();
        if rt.forgets != forgets {
            return;
        }
        let Some(open) = rt.open(&account) else { return };
        let at = now();
        let mut changes = store::launcher_changes(open.list.as_deref(), &list, at);
        for (id, (name, picture)) in worlds {
            let world = World { name, picture };
            if open.worlds.get(&id) != Some(&world) {
                changes.extend(Change::put(kind::WORLD, &id, at, "a world", &world)); // i18n: not shown (a record's words)
                open.worlds.insert(id, world);
            }
        }
        for (id, picture) in decorations {
            if open.decorations.get(&id) != Some(&picture) {
                changes.extend(Change::put(kind::DECORATION, &id, at, "an inventory item", &picture)); // i18n: not shown (a record's words)
                open.decorations.insert(id, picture);
            }
        }
        if !changes.is_empty() || leaving || at - open.seen >= SEEN_EVERY {
            changes.extend(Change::put(kind::SEEN, ONLY, at, "friends list", &at)); // i18n: not shown (a record's words)
            open.seen = at;
        }
        if open.store.write(&changes) {
            open.list = Some(list);
        }
        if leaving {
            open.store.tidy(at);
        }
    }

    /// The account's kept state, for the session `by` says is its own.
    pub(super) fn kept_seed(&self, by: &By) -> Option<Seed> {
        let account = Account::of(&by.stamp.owner.member, &by.user)?;
        let rt = self.kept();
        let root = rt.root.as_ref()?;
        // Its folder from now on (the pages' pictures go there): only while it is signed in (a sign-out waits for `rt`).
        if self.vrchat_signed_in_now().as_deref() != Some(account.user()) || !store::begin(root, &account) {
            return None;
        }
        let s = Store::read(root, &account)?;
        let at = now();
        let worlds = s.worlds();
        Some(Seed {
            list: s.launcher(),
            old_worlds: worlds.iter().filter(|(_, r)| at - r.at > WORLD_FRESH).map(|(id, _)| id.clone()).collect(),
            worlds: worlds.into_iter().map(|(id, Record { value, .. })| (id, (value.name, value.picture))).collect(),
            decorations: s.decorations(),
        })
    }

    /// At launch, the session saved here being restored: the list last on show for the member the state is for and the
    /// account it is restored as. Nothing reaches a person from it until VRChat answers.
    pub(super) fn kept_at_launch(&self, stamp: &Stamp, user: &str) {
        let list = Account::of(&stamp.owner.member, user).and_then(|account| {
            let rt = self.kept();
            Store::read(rt.root.as_ref()?, &account)?.launcher()
        });
        if let Some(mut list) = list {
            list.possible = Possible { live_status: true, ..Possible::default() };
            self.publish_kept_list(stamp, user, list);
        }
    }

    /// Signed out of the VRChat account `user` (of an account not known: of every one): nothing read with it stays on this
    /// computer. Whether it is all gone.
    pub(super) fn kept_forget(&self, user: Option<&str>) -> bool {
        let mut rt = self.kept();
        rt.forgets += 1;
        rt.open = None;
        let Some(root) = rt.root.clone() else { return true };
        // Pictures kept before 0.3.10, not moved yet: whose they were was not written (VRCX's copies fill them again).
        let before = root.with_file_name(BEFORE);
        let gone = std::fs::remove_dir_all(&before).is_ok() || !before.exists();
        store::forget(&root, user) && gone
    }

    /// When a read of the groups begins: what it gives is kept only if nobody signed out meanwhile.
    pub(super) fn kept_turn(&self) -> u64 {
        self.kept().forgets
    }

    /// A groups' answer, for the account of `scope` (still the one signed in: the caller checked).
    pub(super) fn kept_groups(&self, turn: u64, scope: &Scope, request: &groups::Request, reply: &groups::Reply) {
        use groups::{Reply, Request};
        let at = now();
        let by = source(request);
        let Some(account) = scope.account() else { return };
        let mut rt = self.kept();
        if rt.forgets != turn {
            return;
        }
        let Some(open) = rt.open(&account) else { return };
        let mut changes = Vec::new();
        match (request, reply) {
            (Request::Discover, Reply::Discovered(cards)) => {
                // A card not read yet keeps the name and pictures it had.
                let before = open.store.get::<Vec<GroupCard>>(kind::GROUPS, ONLY).map(|r| r.value).unwrap_or_default();
                let cards: Vec<GroupCard> = cards.iter().map(|c| before.iter().find(|b| b.id == c.id && b.named && !c.named).map_or_else(|| c.clone(), |b| GroupCard { owner: c.owner, permissions: c.permissions.clone(), ..b.clone() })).collect();
                changes.extend(Change::put(kind::GROUPS, ONLY, at, by, &cards));
                let ids: Vec<String> = cards.into_iter().map(|c| c.id).collect();
                for kind in [kind::GROUP, kind::MEMBERS, kind::POSTS, kind::EVENTS, kind::TOLD] {
                    changes.push(Change::Only { kind, ids: ids.clone() });
                }
            }
            (Request::Hydrate { .. }, Reply::Hydrated(named)) => {
                let Some(Record { value: mut cards, .. }) = open.store.get::<Vec<GroupCard>>(kind::GROUPS, ONLY) else { return };
                for n in named.iter().filter_map(|(_, n)| n.as_ref().ok()) {
                    if let Some(c) = cards.iter_mut().find(|c| c.id == n.id) {
                        let (owner, permissions) = (c.owner || n.owner, if c.permissions.is_empty() { n.permissions.clone() } else { c.permissions.clone() });
                        *c = GroupCard { owner, permissions, ..n.clone() };
                    }
                }
                changes.extend(Change::put(kind::GROUPS, ONLY, at, by, &cards));
            }
            (Request::Open { group }, Reply::Opened(page)) => changes.extend(Change::put(kind::GROUP, group, at, by, page)),
            (Request::Members { group, query, offset: 0 }, Reply::Members(m)) if query.trim().is_empty() => changes.extend(Change::put(kind::MEMBERS, group, at, by, m)),
            (Request::Events { group, month, offset }, Reply::Events(page)) => {
                let mut list = open.store.get::<Vec<CalendarEvent>>(kind::EVENTS, group).map(|r| r.value).unwrap_or_default();
                group_events::merge(&mut list, *month, *offset, page);
                list.drain(..list.len().saturating_sub(EVENTS_KEPT));
                changes.extend(Change::put(kind::EVENTS, group, at, by, &list));
            }
            (Request::KeepEvents { group, told }, Reply::Cancelled) => changes.extend(Change::put(kind::TOLD, group, at, by, told)),
            (Request::Posts { group, offset: 0 }, Reply::Posts(p)) => changes.extend(Change::put(kind::POSTS, group, at, by, p)),
            _ => {}
        }
        open.store.write(&changes);
    }

    #[cfg(test)]
    pub(crate) fn kept_at_for_tests(&self, root: PathBuf) {
        self.kept().root = Some(root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::vrchat_login::{Command, Presence};
    use crate::state::AccountState;
    use orbiters_core::account::Server;
    use orbiters_core::launcher::{Presence as Where, Whereabouts};
    use orbiters_core::vrchat_api::Gate;
    use serde_json::{json, Value};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    const ME: &str = "usr_00000000-0000-4000-8000-0000000000aa";
    const OTHER: &str = "usr_00000000-0000-4000-8000-0000000000bb";

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let dir = orbiters_core::paths::cache_dir().join(format!("vrchat-kept-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir.join("vrchat"))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    fn person(n: u32, name: &str, status: &str) -> Value {
        json!({"id": format!("usr_00000000-0000-4000-8000-{n:012}"), "displayName": name, "status": status, "statusDescription": format!("says {name}"), "location": "private", "state": "online", "platform": "standalonewindows",
            "userIcon": format!("https://api.vrchat.cloud/api/1/file/file_00000000-0000-4000-8000-{n:012}/1/file")})
    }

    /// VRChat as `me` is signed in, its friends as `friends` gives them now; nothing answers while `down`.
    fn vrchat(me: &'static str, friends: fn() -> Vec<Value>, down: Arc<AtomicBool>) -> impl Fn(&str, &str) -> (u16, Value) + Send + Sync + 'static {
        move |_: &str, url: &str| {
            if down.load(Ordering::SeqCst) {
                return (503, json!({}));
            }
            match url.split("/api/1").nth(1).unwrap_or_default() {
                u if u.starts_with("/auth/user/friends") => (200, if u.contains("offline=false") { Value::Array(friends()) } else { json!([]) }),
                u if u.starts_with("/auth/user/notifications") => (200, json!([])),
                u if u.starts_with("/auth/user") => (200, json!({"id": me, "displayName": "Tester", "status": "active", "statusDescription": "hello"})),
                u if u.starts_with("/favorite") => (200, json!([])),
                u if u.starts_with("/notifications") => (200, json!([])),
                _ => (404, json!({})),
            }
        }
    }

    /// A start of the service with a session saved here, its folder `root`, the Orbiters member `id`: to its first state, as
    /// a window that connects then has it.
    fn start(root: &std::path::Path, id: &str, server: impl Fn(&str, &str) -> (u16, Value) + Send + Sync + 'static) -> Shared {
        let c = Controller::new(orbiters_core::settings::Settings::default());
        c.kept_at_for_tests(root.to_path_buf());
        let (client, _) = orbiters_core::vrchat_api::test_server(Arc::new(Gate::default()), Duration::ZERO, server);
        c.test_saved_vrchat(client, |_, _| Ok(true));
        c.account_saved_at_launch(Server::current(), Some(account(id)));
        let cached = serde_json::from_value(json!({"id": ME, "displayName": "Tester"})).unwrap();
        let owner = c.vrchat_now().map(|s| s.owner.member);
        c.vrchat_at_launch(Some((cached, None, owner)));
        c
    }
    fn account(id: &str) -> orbiters_core::account::Account {
        orbiters_core::account::Account { token: "fake".into(), user: Some(id.into()), username: Some("tester".into()), avatar_url: None, features: None }
    }
    /// The account thread read the file; then the looks, until VRChat said who is signed in.
    fn signs_in(c: &Shared, id: &str) {
        {
            let mut rt = c.account_runtime.lock().unwrap();
            rt.read = Some((Server::current(), None));
            rt.account = Some(account(id));
        }
        c.update(|s| s.account.state = AccountState::Checking { username: Some("tester".into()) });
        for _ in 0..4 {
            c.vrchat_feature_tick();
        }
    }
    fn two() -> Vec<Value> {
        vec![person(1, "Ada", "join me"), person(2, "Bea", "busy")]
    }
    fn changed() -> Vec<Value> {
        vec![person(1, "Ada", "ask me"), person(3, "Cleo", "active")]
    }
    fn up() -> Arc<AtomicBool> {
        Arc::default()
    }
    fn names(l: &LauncherState) -> Vec<&str> {
        l.friends.iter().map(|f| f.name.as_str()).collect()
    }
    /// A run that read the list and quit.
    fn a_run(root: &std::path::Path) -> LauncherState {
        let c = start(root, "1", vrchat(ME, two, up()));
        assert_eq!(c.view().launcher.kept, None, "nothing kept yet");
        signs_in(&c, "1");
        c.launcher_tick();
        c.kept_save(true);
        let list = (*c.view().launcher).clone();
        assert_eq!((names(&list), list.kept), (vec!["Ada", "Bea"], None));
        list
    }

    #[test]
    fn the_list_is_as_it_was_at_the_first_state_of_the_next_start_and_changes_in_place() {
        let t = Temp::new();
        let last = a_run(&t.0);
        let down = Arc::new(AtomicBool::new(true));
        let c = start(&t.0, "1", vrchat(ME, changed, down.clone()));
        let first = (*c.view().launcher).clone();
        assert_eq!((&first.friends, &first.groups, &first.me, &first.source, &first.invites, &first.boops), (&last.friends, &last.groups, &last.me, &last.source, &last.invites, &last.boops), "before any window asks");
        assert!(first.kept.is_some() && !first.loading && !first.possible.boop && !first.possible.request_invite, "nothing reaches a person from it");
        assert_eq!(c.view().vrchat_login.presence(), Presence::Connecting);
        // VRChat out of reach: it stays, whatever the looks do, and no local copy takes its place.
        signs_in(&c, "1");
        for _ in 0..3 {
            c.launcher_tick();
        }
        assert_eq!(c.view().launcher.friends, last.friends);
        assert!(c.view().launcher.kept.is_some() && c.launcher_sender(Some(ME)).is_err());
        // A save of what came from here writes nothing.
        c.kept_save(true);
        // VRChat answers: signed in, the same list until it is read; then its own.
        down.store(false, Ordering::SeqCst);
        c.vrchat_login.lock().unwrap().test_restore_now();
        signs_in(&c, "1");
        assert_eq!(c.view().vrchat_login.presence(), Presence::SignedIn);
        assert_eq!(c.view().launcher.friends, last.friends, "still there while its own is read");
        c.launcher_tick();
        let now = c.view();
        assert_eq!((names(&now.launcher), now.launcher.kept), (vec!["Ada", "Cleo"], None));
        assert!(now.launcher.possible.boop);
        c.kept_save(false);
        let again = start(&t.0, "1", vrchat(ME, changed, Arc::new(AtomicBool::new(true))));
        assert_eq!(again.view().launcher.friends, now.launcher.friends);
    }

    #[test]
    fn another_member_or_another_vrchat_account_sees_nothing_of_it() {
        let t = Temp::new();
        a_run(&t.0);
        // Another Orbiters member on this computer, with the same saved session.
        let c = start(&t.0, "2", vrchat(ME, two, Arc::new(AtomicBool::new(true))));
        signs_in(&c, "2");
        c.launcher_tick();
        assert!(names(&c.view().launcher).is_empty() && c.view().launcher.kept.is_none(), "{:?}", c.view().launcher.source);
        // The session turns out to be another VRChat account's: its own list or none, never the other's.
        let c = start(&t.0, "1", vrchat(OTHER, changed, up()));
        assert_eq!(names(&c.view().launcher), ["Ada", "Bea"], "the account being restored, as far as is known");
        signs_in(&c, "1");
        assert_eq!(c.view().vrchat_login.user.as_ref().map(|u| u.id.as_str()), Some(OTHER));
        assert!(names(&c.view().launcher).is_empty(), "gone with the first word of who it is");
        c.launcher_tick();
        assert_eq!(names(&c.view().launcher), ["Ada", "Cleo"], "its own");
        c.kept_save(false);
        // Each has its own from then on, switching back and forth.
        let c = start(&t.0, "1", vrchat(ME, two, Arc::new(AtomicBool::new(true))));
        assert_eq!(names(&c.view().launcher), ["Ada", "Bea"]);
        // Under another time of the member's, or no member, nothing is put on show.
        let stamp = c.vrchat_now().unwrap();
        let list = (*c.view().launcher).clone();
        c.blank_launcher(|_| true);
        let late = Stamp { epoch: stamp.epoch + 1, ..stamp.clone() };
        assert!(!c.publish_kept_list(&late, ME, list.clone()) && !c.publish_kept_list(&stamp, OTHER, list.clone()));
        assert!(!c.publish_kept_list(&stamp, ME, LauncherState { kept: None, ..list.clone() }));
        assert!(c.publish_kept_list(&stamp, ME, list.clone()));
        assert!(!c.publish_kept_list(&stamp, ME, list), "never onto a list that is there");
    }

    #[test]
    fn signing_out_takes_everything_of_the_account_off_this_computer() {
        let t = Temp::new();
        a_run(&t.0);
        let c = start(&t.0, "1", vrchat(ME, two, up()));
        signs_in(&c, "1");
        c.launcher_tick();
        let account = c.view().launcher.verified_for.as_ref().and_then(Account::verified).unwrap();
        assert!(store::there(&t.0, &account));
        let turn = c.kept_turn();
        let scope = Scope::of(&c.view());
        let before = t.0.with_file_name(BEFORE);
        std::fs::create_dir_all(&before).unwrap();
        std::fs::write(before.join("u-x.pic"), b"a picture kept before").unwrap();
        c.vrchat_command(Command::SignOut);
        let until = Instant::now() + Duration::from_secs(20);
        while (store::there(&t.0, &account) || c.view().vrchat_login.busy) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!account.dir(&t.0).exists() && !before.exists());
        // What was on its way then writes nothing.
        c.kept_groups(turn, &scope, &groups::Request::Discover, &groups::Reply::Discovered(vec![GroupCard::default()]));
        c.kept_save(true);
        assert!(!account.dir(&t.0).exists());
        assert!(names(&c.view().launcher).is_empty());
        assert!(names(&start(&t.0, "1", vrchat(ME, two, up())).view().launcher).is_empty());
    }

    #[test]
    fn a_store_that_cannot_be_read_is_no_list_and_is_written_again() {
        let t = Temp::new();
        let last = a_run(&t.0);
        let account = last.verified_for.as_ref().and_then(Account::verified).unwrap();
        for end in ["-wal", "-shm"] {
            let _ = std::fs::remove_file(account.dir(&t.0).join(format!("state.sqlite3{end}")));
        }
        std::fs::write(account.dir(&t.0).join("state.sqlite3"), [0x5Au8; 9000]).unwrap();
        let c = start(&t.0, "1", vrchat(ME, two, up()));
        assert!(names(&c.view().launcher).is_empty());
        signs_in(&c, "1");
        c.launcher_tick();
        c.kept_save(true);
        let again = start(&t.0, "1", vrchat(ME, two, Arc::new(AtomicBool::new(true))));
        assert_eq!(again.view().launcher.friends, last.friends);
    }

    /// A friend who moves while the app runs is where they were then, at the next start.
    #[test]
    fn what_the_pipeline_changed_is_kept_too() {
        let t = Temp::new();
        let c = start(&t.0, "1", vrchat(ME, two, up()));
        signs_in(&c, "1");
        c.launcher_tick();
        c.kept_save(false);
        c.launcher_message(&json!({"type": "friend-offline", "content": json!({"userId": "usr_00000000-0000-4000-8000-000000000002"}).to_string()}).to_string());
        c.kept_save(false);
        let next = start(&t.0, "1", vrchat(ME, two, Arc::new(AtomicBool::new(true))));
        let view = next.view();
        let bea = view.launcher.friends.iter().find(|f| f.name == "Bea").unwrap();
        assert_eq!((bea.presence, &bea.whereabouts), (Where::Offline, &Whereabouts::Offline));
        assert_eq!(view.launcher.groups, c.view().launcher.groups);
    }

    #[test]
    fn a_groups_answer_is_kept_for_the_pages() {
        let t = Temp::new();
        let c = start(&t.0, "1", vrchat(ME, two, up()));
        signs_in(&c, "1");
        let (turn, scope) = (c.kept_turn(), Scope::of(&c.view()));
        let account = scope.account().unwrap();
        let card = |n: u32, name: &str| GroupCard { id: format!("grp_00000000-0000-4000-8000-{n:012}"), name: name.into(), named: !name.is_empty(), icon_url: format!("https://api.vrchat.cloud/{name}"), ..Default::default() };
        let keep = |request: groups::Request, reply: groups::Reply| c.kept_groups(turn, &scope, &request, &reply);
        keep(groups::Request::Discover, groups::Reply::Discovered(vec![card(1, "One"), card(2, "Two")]));
        let page = groups::Page { group: orbiters_core::groups::Group { id: card(1, "").id, name: "One".into(), ..Default::default() }, catalogue: None };
        keep(groups::Request::Open { group: card(1, "").id }, groups::Reply::Opened(Box::new(page.clone())));
        keep(groups::Request::Members { group: card(1, "").id, query: String::new(), offset: 0 }, groups::Reply::Members(orbiters_core::vrchat_api::Members { raw: 7, ..Default::default() }));
        // A search and a later page are not the first one.
        keep(groups::Request::Members { group: card(1, "").id, query: "ada".into(), offset: 0 }, groups::Reply::Members(orbiters_core::vrchat_api::Members { raw: 1, ..Default::default() }));
        keep(groups::Request::Members { group: card(1, "").id, query: String::new(), offset: 50 }, groups::Reply::Members(orbiters_core::vrchat_api::Members { raw: 2, ..Default::default() }));
        // The calendar: every page of it, merged; what Orbiters says of it.
        let one = card(1, "").id;
        let event = |id: &str, at: &str| CalendarEvent { id: id.into(), title: id.into(), starts_at: Some(at.into()), ..Default::default() };
        let events = |items: Vec<CalendarEvent>, more: bool| groups::Reply::Events(group_events::Events { raw: items.len(), items, more });
        let month = group_events::Month { year: 2026, month: 11 };
        keep(groups::Request::Events { group: one.clone(), month: None, offset: 0 }, events(vec![event("cal_a", "2026-11-10T20:00:00Z"), event("cal_b", "2026-12-10T20:00:00Z")], true));
        keep(groups::Request::Events { group: one.clone(), month: Some(month), offset: 0 }, events(vec![event("cal_c", "2026-11-12T20:00:00Z")], false));
        let told = group_events::Told { cards: vec![orbiters_core::community::Card { id: "e1".into(), calendar_id: Some("cal_c".into()), ..Default::default() }], pages: vec![] };
        keep(groups::Request::KeepEvents { group: one.clone(), told: Box::new(told.clone()) }, groups::Reply::Cancelled);
        let read = || Store::read(&t.0, &account).unwrap();
        let kept = read().get::<Vec<CalendarEvent>>(kind::EVENTS, &one).unwrap().value;
        assert_eq!(kept.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["cal_c", "cal_b"]);
        assert_eq!(read().get::<group_events::Told>(kind::TOLD, &one).unwrap().value, told);
        assert_eq!(read().get::<groups::Page>(kind::GROUP, &card(1, "").id).unwrap().value, page);
        assert_eq!(read().get::<orbiters_core::vrchat_api::Members>(kind::MEMBERS, &card(1, "").id).unwrap().value.raw, 7);
        // Read again: a card not named this time keeps its name; a group left goes, its page with it.
        keep(groups::Request::Discover, groups::Reply::Discovered(vec![GroupCard { icon_url: String::new(), ..card(2, "") }]));
        let cards = read().get::<Vec<GroupCard>>(kind::GROUPS, ONLY).unwrap().value;
        assert_eq!(cards, [card(2, "Two")]);
        assert!(read().get::<groups::Page>(kind::GROUP, &card(1, "").id).is_none());
        assert!(read().get::<Vec<CalendarEvent>>(kind::EVENTS, &one).is_none() && read().get::<group_events::Told>(kind::TOLD, &one).is_none());
    }
}
