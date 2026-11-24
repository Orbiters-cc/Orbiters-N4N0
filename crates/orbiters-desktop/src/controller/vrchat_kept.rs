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

/// The requests the records are read by.
fn source(request: &groups::Request) -> &'static str {
    match request {
        groups::Request::Discover | groups::Request::Hydrate { .. } => "my groups", // i18n: not shown (a record's words)
        groups::Request::Open { .. } => "a group",                                  // i18n: not shown (a record's words)
        groups::Request::Members { .. } => "a group's members",                     // i18n: not shown (a record's words)
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
        store::forget(&root, user)
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
                for kind in [kind::GROUP, kind::MEMBERS, kind::POSTS, kind::EVENTS] {
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
            (Request::Posts { group, offset: 0 }, Reply::Posts(p)) => changes.extend(Change::put(kind::POSTS, group, at, by, p)),
            _ => {}
        }
        open.store.write(&changes);
    }

    #[cfg(test)]
    pub(super) fn kept_at_for_tests(&self, root: PathBuf) {
        self.kept().root = Some(root);
    }
}
