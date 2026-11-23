//! VRChat groups for the window (REQUEST22 1; DESIGN22's revision after the attack; the fix round after
//! verify-r22-groups.md): Home's Groups section, the group page's reads and its single, guarded writes.
//!
//! - Nothing of a group goes in the snapshot (M9): every reply goes to the window that asked, on demand
//!   (`ToService::Groups` → `ToClient::Groups`). Only the window may ask; the overlay, voice and the generic commands
//!   cannot (M10). The service admits a few requests at once BEFORE starting a thread for one ([`Controller::groups_admit`]).
//! - Every request is bound to its [`Scope`] (environment, Orbiters account, the signed-in VRChat account, B3) and the
//!   ids it names; a cached account in "Connecting" grants nothing. The scope is checked again under the client's lock
//!   right before each write is dispatched, and before a reply goes out.
//! - Every write is prepared ([`Request::Prepare`]: fresh reads of the exact target, the rules of
//!   `orbiters_core::groups::can`, the exact payload) and then committed once ([`Request::Commit`]) by the same window
//!   with a one-use operation id kept here. At the commit the target is read again and compared with what the person
//!   confirmed: anything material changed refuses it (Blocker 7). Its outcome is done, refused or UNKNOWN and is kept
//!   here by its op id, so that a window whose answer was lost on the link can ask what happened ([`Request::Check`],
//!   Blocker 1). After an unknown one nothing about that target is sent again until a check has read the exact resource
//!   the write touched and found it applied or not applied, or the person, told the check was inconclusive, lets it go
//!   ([`Request::Release`], Blocker 2).
//! - One write at a time per target, a few requests at a time in all; the client's lock is let go between calls (M9,
//!   M11). No bulk, no background sweeps.
use super::vrchat_data::Scope;
use super::vrchat_login::Presence;
use super::Controller;
use orbiters_core::groups::{self as g, Action, Actor, ArtSlot, Catalogue, FileTag, Group, GroupCard, InventoryFile, Member, PermissionInfo, Post, PostDraft, Role, RoleChange, RoleDraft, Verdict, Why};
use orbiters_core::vrchat_api::{Client, GroupFailure, GroupInventory, Members, Posts};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The link of the in-process window (no tray service between).
pub const LOCAL_LINK: u64 = 0;
/// A prepared write is committed within this time or prepared again.
const INTENT_FOR: Duration = Duration::from_secs(300);
/// Prepared writes kept at once, ids remembered as used, outcomes remembered, holds kept.
const INTENTS_MAX: usize = 16;
const USED_MAX: usize = 128;
const RECORDS_MAX: usize = 64;
const RECORD_FOR: Duration = Duration::from_secs(3600);
const HOLDS_MAX: usize = 64;
/// Reads at once inside the controller (each lets the client's lock go between its calls).
const RUNNING_MAX: usize = 3;
/// Threads the service starts for groups requests at once, commits and cancels included (M9: admission before a thread).
pub const ADMIT_MAX: usize = 6;
/// Groups on Home at most, and those read one by one at discovery when the account's list does not show them (the
/// rest are read as their cards come into view, [`Request::Hydrate`], Major 7).
const GROUPS_MAX: usize = 1000;
const HYDRATE_MAX: usize = 12;
/// Groups a [`Request::Hydrate`] reads at most.
pub const HYDRATE_BATCH: usize = 8;
/// Pages of posts read to find one post (an edit's or a deletion's target, a check), and of inventory to find a file.
const POST_PAGES: usize = 4;
const INVENTORY_PAGES: usize = 4;
/// A picture of the page at most (memory only).
const PICTURE_MAX: usize = 12 * 1024 * 1024;

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Home's Groups section: the groups this account owns or helps run.
    Discover,
    /// The cards of groups listed without their names yet (in view on Home), a few at a time.
    Hydrate { ids: Vec<String> },
    /// A group's page: the group with its roles and the account's membership, and (with rights over roles) the rights'
    /// catalogue.
    Open { group: String },
    /// A page of members (`query` of three characters or more: VRChat's search).
    Members { group: String, query: String, offset: usize },
    Posts { group: String, offset: usize },
    /// A page of the account's own pictures.
    Inventory { tag: FileTag, offset: usize },
    /// A picture of the page (a member's, a banner), in memory only.
    Picture { url: String },
    Prepare { write: Write },
    Commit { op: String },
    Cancel { op: String },
    /// What became of the write `op` (its answer did not arrive, or was "unknown"): the outcome kept here, or a fresh
    /// read of exactly what `write` touched.
    Check { op: String, write: Write },
    /// The person, told the check could not settle it, lets the hold on `op` go.
    Release { op: String },
}

/// A PNG the window put in the staging folder for an upload: only its name, size and SHA-256 cross the link (Major 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Staged {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// The staging folder of uploads, under the local data folder (the test folders in tests).
pub fn staging_dir() -> std::path::PathBuf {
    orbiters_core::paths::local_data_root().join("group-uploads")
}

impl Staged {
    /// The file's name is one the window makes (a UUID): nothing else is ever read.
    fn path(&self) -> Option<std::path::PathBuf> {
        let ok = self.name.len() <= 64 && self.name.ends_with(".png") && self.name[..self.name.len() - 4].bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
        ok.then(|| staging_dir().join(&self.name))
    }
    /// The bytes, when they are what was announced (size, hash, a PNG, within the limit).
    fn read(&self) -> Result<Vec<u8>, Failure> {
        use sha2::Digest;
        let path = self.path().ok_or(Failure::Api(GroupFailure::Invalid))?;
        if self.size as usize > orbiters_core::vrchat_api::GROUP_UPLOAD_MAX {
            return Err(Failure::TooLarge);
        }
        let len = std::fs::metadata(&path).map_err(|_| Failure::Expired)?.len();
        if len != self.size {
            return Err(Failure::Changed);
        }
        let bytes = std::fs::read(&path).map_err(|_| Failure::Expired)?;
        let hash: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        if hash != self.sha256 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(Failure::Changed);
        }
        Ok(bytes)
    }
}

/// One write, as the person asked for it, with the ids captured at their press (M3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Write {
    MemberRole { group: String, user: String, role: String, add: bool },
    CreateRole { group: String, draft: RoleDraft },
    EditRole { group: String, baseline: Role, edit: RoleDraft },
    DeleteRole { group: String, role: String },
    CreatePost { group: String, draft: PostDraft },
    EditPost { group: String, baseline: Post, edit: PostDraft },
    /// `post` is the post as the person saw it: a different one now is not deleted (Blocker 7).
    DeletePost { group: String, post: Post },
    /// A PNG into the account's own gallery or icons, for `group`: a step of its own (M6).
    Upload { tag: FileTag, group: String, staged: Staged },
    ApplyArt { group: String, slot: ArtSlot, file: String },
}

impl Write {
    /// What one write at a time is counted on, and what an unknown outcome holds.
    pub fn target(&self) -> String {
        match self {
            Write::MemberRole { group, user, .. } => format!("member:{group}:{user}"),
            Write::CreateRole { group, .. } | Write::EditRole { group, .. } | Write::DeleteRole { group, .. } => format!("roles:{group}"),
            Write::CreatePost { group, .. } | Write::EditPost { group, .. } | Write::DeletePost { group, .. } => format!("posts:{group}"),
            Write::ApplyArt { group, .. } => format!("art:{group}"),
            Write::Upload { tag, .. } => format!("upload:{}", tag.wire()),
        }
    }
    pub fn group(&self) -> &str {
        match self {
            Write::MemberRole { group, .. }
            | Write::CreateRole { group, .. }
            | Write::EditRole { group, .. }
            | Write::DeleteRole { group, .. }
            | Write::CreatePost { group, .. }
            | Write::EditPost { group, .. }
            | Write::DeletePost { group, .. }
            | Write::Upload { group, .. }
            | Write::ApplyArt { group, .. } => group,
        }
    }
}

/// A write prepared: its one-use id, what will be sent (normalised), whether the person confirms it first, and the
/// target exactly as read just now (what the confirmation shows, and what the commit compares with).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prepared {
    pub op: String,
    pub write: Write,
    /// Public, destructive or privilege-bearing: the window shows what will be sent and waits for the person.
    pub confirm: bool,
    /// The role concerned, as read just now (a member's role change, a role's change or deletion).
    pub role: Option<Role>,
    /// The post concerned, as read just now (an edit, a deletion).
    pub post: Option<Post>,
    /// The member concerned, as read just now (ids that tell namesakes apart).
    pub member: Option<Member>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Done {
    /// The member's roles as VRChat now has them (the committed state, M3).
    MemberRoles { user: String, role_ids: Vec<String> },
    Role(Role),
    Roles(Vec<Role>),
    Post(Post),
    PostDeleted { post: String },
    Uploaded(InventoryFile),
    Group(Box<Group>),
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Done(Done),
    /// Nothing was done: why.
    Refused(Failure),
    /// It was sent and its answer was lost: it may have been done. Nothing is retried; check before sending again.
    Unknown,
}

/// What a check found of a write whose outcome was not known.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Observation {
    /// The service kept its outcome (its answer was lost on the way to the window).
    Recorded(Outcome),
    /// The commit never reached the service: nothing was sent.
    NotSent,
    /// It is being sent now.
    Running,
    /// A fresh read shows it done.
    Applied,
    /// A fresh read shows it not done: it may be done again.
    NotApplied,
    /// A fresh read cannot settle it (a post not among the newest, an upload, someone else changed it meanwhile).
    Inconclusive,
}

/// Why a request gave nothing: local codes, worded by the window. No server text, no member, role or post text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Failure {
    Api(GroupFailure),
    /// Not signed in to VRChat for certain (signed out, or still connecting).
    SignedOut,
    /// The Orbiters or VRChat account (or the environment) changed since this was asked.
    AccountChanged,
    /// Enough is being asked already: in a moment.
    Busy,
    /// This version withholds it, and why.
    Withheld(Why),
    /// The prepared write is gone (too old, cancelled, the account changed): prepare it again.
    Expired,
    /// This prepared write was already committed once.
    Duplicate,
    /// What it was made against changed since (the role, the member's roles, the post).
    Changed,
    /// The last write here may have been done: check first.
    CheckFirst,
    /// Only the window asks this.
    NotHere,
    /// Over the size this accepts.
    TooLarge,
    /// The service's answer did not arrive (the link closed or timed out): whatever was asked may have been done.
    Lost,
}

/// The group's page as read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub group: Group,
    /// The rights a role can hold: read only with rights over roles.
    pub catalogue: Option<Vec<PermissionInfo>>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    Discovered(Vec<GroupCard>),
    Hydrated(Vec<GroupCard>),
    Opened(Box<Page>),
    Members(Members),
    Posts(Posts),
    Inventory(GroupInventory),
    Picture(#[serde(with = "super::vrchat_login::bytes_b64")] Vec<u8>),
    Prepared(Box<Prepared>),
    Outcome(Outcome),
    /// A write on this target may have been done: this one (the window shows it and offers its check).
    Held { op: String, write: Write },
    Checked { op: String, observation: Observation },
    Released,
    Cancelled,
    Failed(Failure),
}

struct Intent {
    op: String,
    scope: Scope,
    link: u64,
    write: Write,
    role: Option<Role>,
    post: Option<Post>,
    at: Instant,
}

/// A write whose outcome is not known: its target is held until a check settles it.
struct Hold {
    scope: Scope,
    op: String,
    target: String,
    write: Write,
    /// A check ran and could not settle it: the person may now let it go.
    checked: bool,
}

/// The service's side of groups: prepared writes, ids used, outcomes, holds, targets being written.
#[derive(Default)]
pub(crate) struct Runtime {
    intents: Vec<Intent>,
    used: VecDeque<String>,
    records: VecDeque<(Scope, String, Outcome, Instant)>,
    holds: Vec<Hold>,
    writing: HashSet<String>,
    running: usize,
    #[cfg(test)]
    pub(crate) sent_ops: u64,
    /// Requests that reached the controller (the service's admission is tested by it).
    #[cfg(test)]
    pub(crate) entered: u64,
}

impl Runtime {
    /// What is prepared under another scope goes (B3), and what is too old. Holds and outcomes stay, by scope: they are
    /// that account's, for when it is back.
    fn forget_others(&mut self, scope: &Scope) {
        self.intents.retain(|i| i.scope == *scope && i.at.elapsed() < INTENT_FOR);
        self.records.retain(|r| r.3.elapsed() < RECORD_FOR);
    }
    fn hold(&self, scope: &Scope, target: &str) -> Option<&Hold> {
        self.holds.iter().find(|h| h.scope == *scope && h.target == target)
    }
    fn release(&mut self, scope: &Scope, op: &str) {
        self.holds.retain(|h| !(h.scope == *scope && h.op == op));
    }
    fn record(&mut self, scope: &Scope, op: &str, outcome: &Outcome) {
        self.records.push_back((scope.clone(), op.to_string(), outcome.clone(), Instant::now()));
        while self.records.len() > RECORDS_MAX {
            self.records.pop_front();
        }
    }
}

/// The requests the window may make, and nobody else (M10).
pub fn allowed_from(role: crate::ipc::Role) -> bool {
    role == crate::ipc::Role::Window
}

/// A groups request the service took in: counted until its thread ends.
pub struct Admitted(Arc<AtomicUsize>);
impl Drop for Admitted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// One running request, counted while it runs.
struct Slot<'a>(&'a Controller);
impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut rt = self.0.groups.lock().unwrap();
        rt.running = rt.running.saturating_sub(1);
    }
}

fn verdict(v: Verdict) -> Result<(), Failure> {
    match v {
        Verdict::Allowed => Ok(()),
        Verdict::Withheld(why) => Err(Failure::Withheld(why)),
    }
}

/// The rules on a freshly read group.
fn check(action: Action, group: &Group, catalogue: Option<&[PermissionInfo]>) -> Result<(), Failure> {
    let me = group.my_member.as_ref().ok_or(Failure::Api(GroupFailure::Forbidden))?;
    let cat = Catalogue { roles: group.roles.as_deref(), permissions: catalogue };
    verdict(g::can(action, &Actor::of(me), &cat))
}

/// The same role for the purpose of a write made against `then`: its rights, rank, words and flags unchanged.
fn same_role(then: &Role, now: &Role) -> bool {
    let mut a = then.permissions.clone();
    let mut b = now.permissions.clone();
    a.sort();
    b.sort();
    a == b
        && then.known == now.known
        && then.order == now.order
        && then.name == now.name
        && then.description == now.description
        && then.default_role == now.default_role
        && then.management == now.management
        && then.self_assignable == now.self_assignable
        && then.added_on_join == now.added_on_join
        && then.requires_two_factor == now.requires_two_factor
        && then.requires_purchase == now.requires_purchase
}

impl Controller {
    /// Groups requests that reached the controller so far (tests of the service's admission).
    #[cfg(test)]
    pub(crate) fn groups_entered(&self) -> u64 {
        self.groups.lock().unwrap().entered
    }

    /// The service takes a groups request in only while fewer than [`ADMIT_MAX`] run: checked before its thread starts.
    pub fn groups_admit(&self) -> Option<Admitted> {
        let n = self.groups_admitted.clone();
        let mut now = n.load(Ordering::Acquire);
        loop {
            if now >= ADMIT_MAX {
                return None;
            }
            match n.compare_exchange(now, now + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Admitted(n)),
                Err(seen) => now = seen,
            }
        }
    }

    /// A request of the window's (link `link`) under `scope`. Blocking: call on a thread of its own.
    pub fn groups(&self, link: u64, scope: Scope, request: Request) -> Reply {
        #[cfg(test)]
        {
            self.groups.lock().unwrap().entered += 1;
        }
        if scope != Scope::of(&self.view()) {
            return Reply::Failed(Failure::AccountChanged);
        }
        if self.view().vrchat_login.presence() != Presence::SignedIn {
            return Reply::Failed(Failure::SignedOut);
        }
        {
            let mut rt = self.groups.lock().unwrap();
            rt.forget_others(&scope);
            // Committing, cancelling, checking and releasing never wait behind reads.
            let control = matches!(request, Request::Cancel { .. } | Request::Commit { .. } | Request::Check { .. } | Request::Release { .. });
            if rt.running >= RUNNING_MAX && !control {
                return Reply::Failed(Failure::Busy);
            }
            rt.running += 1;
        }
        let _slot = Slot(self);
        let reply = match request {
            Request::Discover => self.groups_discover(&scope).map(Reply::Discovered),
            Request::Hydrate { ids } => self.groups_hydrate(&scope, ids).map(Reply::Hydrated),
            Request::Open { group } => self.groups_open(&scope, &group).map(|p| Reply::Opened(Box::new(p))),
            Request::Check { op, write } => Ok(self.groups_check(link, &scope, &op, write)),
            Request::Release { op } => {
                let mut rt = self.groups.lock().unwrap();
                match rt.holds.iter().find(|h| h.scope == scope && h.op == op) {
                    // Only after a check that could not settle it: the person decided.
                    Some(h) if !h.checked => Err(Failure::CheckFirst),
                    _ => {
                        rt.release(&scope, &op);
                        Ok(Reply::Released)
                    }
                }
            }
            Request::Members { group, query, offset } => {
                let query = query.trim().to_string();
                self.groups_read(&scope, |c| if query.chars().count() >= 3 { c.search_group_members(&group, &query, offset) } else { c.group_members(&group, offset) }).map(Reply::Members)
            }
            Request::Posts { group, offset } => self.groups_read(&scope, |c| c.group_posts(&group, offset)).map(Reply::Posts),
            Request::Inventory { tag, offset } => self.groups_read(&scope, |c| c.inventory(tag, offset)).map(Reply::Inventory),
            Request::Picture { url } => self
                .groups_read(&scope, |c| c.image(&url).map_err(|e| if e == orbiters_core::vrchat_api::Error::UnsafeUrl { GroupFailure::NotFound } else { GroupFailure::Unavailable }))
                .and_then(|bytes| if bytes.len() > PICTURE_MAX { Err(Failure::TooLarge) } else { Ok(Reply::Picture(bytes)) }),
            Request::Prepare { write } => self.groups_prepare(link, &scope, write),
            Request::Commit { op } => Ok(Reply::Outcome(self.groups_commit(link, &scope, &op))),
            Request::Cancel { op } => {
                self.groups.lock().unwrap().intents.retain(|i| !(i.op == op && i.link == link));
                Ok(Reply::Cancelled)
            }
        };
        // Nothing read for one account is handed to another (B3).
        if scope != Scope::of(&self.view()) {
            return match reply {
                Ok(Reply::Outcome(Outcome::Done(_) | Outcome::Unknown)) => Reply::Outcome(Outcome::Unknown),
                _ => Reply::Failed(Failure::AccountChanged),
            };
        }
        reply.unwrap_or_else(Reply::Failed)
    }

    /// A read of the signed-in account, the scope checked under the client's lock before and after.
    fn groups_read<T>(&self, scope: &Scope, f: impl FnOnce(&mut Client) -> Result<T, GroupFailure>) -> Result<T, Failure> {
        let user = scope_user(scope)?;
        let source = self.vrchat_source();
        let deadline = Instant::now() + Duration::from_secs(60);
        match source.personal_until(&user, || *scope == Scope::of(&self.view()), deadline, |c| Ok(f(c))) {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(Failure::Api(e)),
            Err(orbiters_core::vrchat_api::Error::Disabled) => Err(Failure::AccountChanged),
            Err(_) => Err(Failure::SignedOut),
        }
    }

    fn groups_discover(&self, scope: &Scope) -> Result<Vec<GroupCard>, Failure> {
        let me = scope_user(scope)?;
        let rights = self.groups_read(scope, |c| c.my_group_permissions())?;
        let listed = self.groups_read(scope, |c| c.my_groups())?;
        let mut cards: Vec<GroupCard> = Vec::new();
        for (id, permissions) in &rights {
            let shown = listed.iter().find(|(card, _)| card.id == *id);
            let owner = shown.is_some_and(|(_, owner)| *owner == me) || permissions.iter().any(|p| p == g::ALL);
            if !g::listed(permissions, owner) {
                continue;
            }
            let mut card = shown.map(|(card, _)| card.clone()).unwrap_or_else(|| GroupCard { id: id.clone(), ..Default::default() });
            card.owner = owner;
            card.permissions = permissions.clone();
            cards.push(card);
        }
        // Owned groups the rights' map left out still show.
        for (card, owner) in &listed {
            if *owner == me && !cards.iter().any(|c| c.id == card.id) {
                cards.push(GroupCard { owner: true, permissions: vec![g::ALL.into()], ..card.clone() });
            }
        }
        // Memberships the account's list does not show (hidden ones): a few read now, the rest as their cards come into
        // view; every one is listed (Major 7).
        for card in cards.iter_mut().filter(|c| !c.named).take(HYDRATE_MAX) {
            if let Ok(group) = self.groups_read(scope, |c| c.group(&card.id)) {
                fill_card(card, &group, &me);
            }
        }
        cards.sort_by(|a, b| b.owner.cmp(&a.owner).then_with(|| b.named.cmp(&a.named)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        cards.truncate(GROUPS_MAX);
        Ok(cards)
    }

    /// The cards of groups listed without a name yet, read one by one (a few at a time, as they come into view).
    fn groups_hydrate(&self, scope: &Scope, ids: Vec<String>) -> Result<Vec<GroupCard>, Failure> {
        let me = scope_user(scope)?;
        if ids.len() > HYDRATE_BATCH || ids.iter().any(|id| !g::is_group_id(id)) {
            return Err(Failure::Api(GroupFailure::Invalid));
        }
        let mut out = Vec::new();
        for id in ids {
            let group = self.groups_read(scope, |c| c.group(&id))?;
            let mut card = GroupCard { id: id.clone(), ..Default::default() };
            let rights = group.my_member.as_ref().map(|m| m.permissions.clone()).unwrap_or_default();
            card.permissions = rights;
            fill_card(&mut card, &group, &me);
            out.push(card);
        }
        Ok(out)
    }

    fn groups_open(&self, scope: &Scope, group: &str) -> Result<Page, Failure> {
        let found = self.groups_read(scope, |c| c.group(group))?;
        let me = found.my_member.as_ref().ok_or(Failure::Api(GroupFailure::Forbidden))?;
        let catalogue = if g::abilities(&me.permissions).roles {
            Some(self.groups_read(scope, |c| c.group_permission_catalogue(group))?)
        } else {
            None
        };
        Ok(Page { group: found, catalogue })
    }

    /// The post `id` as VRChat has it now, from the newest pages; and whether every post was seen.
    fn groups_find_post(&self, scope: &Scope, group: &str, id: &str) -> Result<(Option<Post>, bool), Failure> {
        let mut offset = 0;
        for _ in 0..POST_PAGES {
            let page = self.groups_read(scope, |c| c.group_posts(group, offset))?;
            if let Some(p) = page.items.iter().find(|p| p.id == id) {
                return Ok((Some(p.clone()), true));
            }
            offset += page.raw;
            if page.raw < orbiters_core::vrchat_api::GROUP_PAGE || page.total.is_some_and(|t| offset as u32 >= t) {
                return Ok((None, true));
            }
        }
        Ok((None, false))
    }

    /// Whether `file` is one of the account's own pictures of `tag` (the newest pages).
    fn groups_own_file(&self, scope: &Scope, tag: FileTag, file: &str) -> Result<bool, Failure> {
        let mut offset = 0;
        for _ in 0..INVENTORY_PAGES {
            let page = self.groups_read(scope, |c| c.inventory(tag, offset))?;
            if page.items.iter().any(|f| f.id == file) {
                return Ok(true);
            }
            if page.raw < orbiters_core::vrchat_api::GROUP_PAGE {
                return Ok(false);
            }
            offset += page.raw;
        }
        Ok(false)
    }

    /// Fresh reads and the rules, then the write kept here under a one-use id.
    fn groups_prepare(&self, link: u64, scope: &Scope, write: Write) -> Result<Reply, Failure> {
        let target = write.target();
        if let Some(h) = self.groups.lock().unwrap().hold(scope, &target) {
            // The window is shown which write is uncertain, to check it (Blocker 2).
            return Ok(Reply::Held { op: h.op.clone(), write: h.write.clone() });
        }
        let ready = self.groups_validate(scope, write)?;
        let op = uuid::Uuid::new_v4().to_string();
        let mut rt = self.groups.lock().unwrap();
        rt.intents.retain(|i| i.link != link || i.write.target() != target);
        if rt.intents.len() >= INTENTS_MAX {
            rt.intents.remove(0);
        }
        rt.intents.push(Intent { op: op.clone(), scope: scope.clone(), link, write: ready.write.clone(), role: ready.role.clone(), post: ready.post.clone(), at: Instant::now() });
        Ok(Reply::Prepared(Box::new(Prepared { op, write: ready.write, confirm: ready.confirm, role: ready.role, post: ready.post, member: ready.member })))
    }

    /// The write checked against what VRChat says now: normalised, whether it is confirmed, and its target as read.
    fn groups_validate(&self, scope: &Scope, write: Write) -> Result<Ready, Failure> {
        let ready = |write: Write, confirm: bool| Ready { write, confirm, role: None, post: None, member: None };
        match write {
            Write::MemberRole { group, user, role, add } => {
                let page = self.groups_read(scope, |c| c.group(&group))?;
                let me = scope_user(scope)?;
                // The member as VRChat has them now, whole: their id, this group, and their role arrays (Blocker 4). A
                // lookup of oneself gives no name: the account's own membership counts.
                let (current, member) = if user == me {
                    let mine = page.my_member.as_ref().ok_or(Failure::Api(GroupFailure::Forbidden))?;
                    (mine.role_ids.clone(), None)
                } else {
                    let member: Member = self.groups_read(scope, |c| c.group_member(&group, &user))?.ok_or(Failure::Api(GroupFailure::NotFound))?;
                    if member.user_id != user || member.group_id.as_deref().is_some_and(|g| g != group) {
                        return Err(Failure::Changed);
                    }
                    if !member.roles_known {
                        return Err(Failure::Withheld(Why::MissingData));
                    }
                    (member.role_ids.clone(), Some(member))
                };
                if current.contains(&role) == add {
                    return Err(Failure::Changed);
                }
                check(Action::AssignRole { role: &role, user: &user, add }, &page, None)?;
                let found = page.roles.as_ref().and_then(|r| r.iter().find(|x| x.id == role)).cloned().ok_or(Failure::Withheld(Why::MissingData))?;
                // Unknown fields count as privileged: confirmed (Blocker 4).
                let confirm = found.privileged();
                Ok(Ready { write: Write::MemberRole { group, user, role, add }, confirm, role: Some(found), post: None, member })
            }
            Write::CreateRole { group, draft } => {
                let draft = RoleDraft { name: g::label(&draft.name, g::ROLE_NAME_MAX), description: g::text(&draft.description, g::ROLE_DESCRIPTION_MAX), ..draft };
                if draft.name.chars().count() < 1 {
                    return Err(Failure::Api(GroupFailure::Invalid));
                }
                let page = self.groups_open(scope, &group)?;
                check(Action::CreateRole { draft: &draft }, &page.group, page.catalogue.as_deref())?;
                let confirm = !draft.permissions.is_empty() || draft.self_assignable || draft.added_on_join || draft.requires_two_factor;
                Ok(ready(Write::CreateRole { group, draft }, confirm))
            }
            Write::EditRole { group, baseline, edit } => {
                let edit = RoleDraft { name: g::label(&edit.name, g::ROLE_NAME_MAX), description: g::text(&edit.description, g::ROLE_DESCRIPTION_MAX), ..edit };
                let page = self.groups_open(scope, &group)?;
                let now = page.group.roles.as_ref().and_then(|r| r.iter().find(|x| x.id == baseline.id)).cloned().ok_or(Failure::Api(GroupFailure::NotFound))?;
                // Made against what is no longer so (another manager changed it): nothing is sent (M4).
                if !same_role(&baseline, &now) {
                    return Err(Failure::Changed);
                }
                let change = RoleChange::between(&now, &edit);
                if change.is_empty() {
                    return Err(Failure::Api(GroupFailure::Invalid));
                }
                check(Action::EditRole { role: &now.id, change: &change }, &page.group, page.catalogue.as_deref())?;
                let confirm = change.consequential() || now.privileged();
                Ok(Ready { write: Write::EditRole { group, baseline: now.clone(), edit }, confirm, role: Some(now), post: None, member: None })
            }
            Write::DeleteRole { group, role } => {
                let page = self.groups_read(scope, |c| c.group(&group))?;
                check(Action::DeleteRole { role: &role }, &page, None)?;
                let found = page.roles.as_ref().and_then(|r| r.iter().find(|x| x.id == role)).cloned().ok_or(Failure::Api(GroupFailure::NotFound))?;
                Ok(Ready { write: Write::DeleteRole { group, role }, confirm: true, role: Some(found), post: None, member: None })
            }
            Write::CreatePost { group, draft } => {
                let draft = clean_post(draft)?;
                let page = self.groups_read(scope, |c| c.group(&group))?;
                check(Action::Post, &page, None)?;
                Ok(ready(Write::CreatePost { group, draft }, true))
            }
            Write::EditPost { group, baseline, edit } => {
                let edit = PostDraft { send_notification: false, ..clean_post(edit)? };
                if !g::is_post_id(&baseline.id) || !baseline.complete {
                    return Err(Failure::Withheld(Why::MissingData));
                }
                let page = self.groups_read(scope, |c| c.group(&group))?;
                check(Action::Post, &page, None)?;
                // The post as VRChat has it now must be the one the edit was made on: its audience, picture and words
                // (another moderator may have changed them, Blocker 3).
                let (now, _) = self.groups_find_post(scope, &group, &baseline.id)?;
                let now = now.ok_or(Failure::Withheld(Why::MissingData))?;
                if !now.complete || !g::same_post(&now, &baseline) {
                    return Err(Failure::Changed);
                }
                Ok(Ready { write: Write::EditPost { group, baseline: now.clone(), edit }, confirm: true, role: None, post: Some(now), member: None })
            }
            Write::DeletePost { group, post } => {
                if !g::is_post_id(&post.id) {
                    return Err(Failure::Api(GroupFailure::NotFound));
                }
                let page = self.groups_read(scope, |c| c.group(&group))?;
                check(Action::Post, &page, None)?;
                // Deleted only when the post found now is the one shown (Blocker 7); one not found is not deleted blind.
                let (now, _) = self.groups_find_post(scope, &group, &post.id)?;
                let now = now.ok_or(Failure::Withheld(Why::MissingData))?;
                if !g::same_post(&now, &post) {
                    return Err(Failure::Changed);
                }
                Ok(Ready { write: Write::DeletePost { group, post: now.clone() }, confirm: true, role: None, post: Some(now), member: None })
            }
            Write::Upload { tag, group, staged } => {
                // The file as announced (Major 1), then the account's right to upload, apart from the group's (Major 4).
                staged.read()?;
                if !g::is_group_id(&group) {
                    return Err(Failure::Api(GroupFailure::NotFound));
                }
                let me = self.groups_read(scope, |c| c.current_user_value().map_err(|_| GroupFailure::Unavailable))?;
                if !vrchat_plus(&me) {
                    return Err(Failure::Withheld(Why::NotEligible));
                }
                Ok(ready(Write::Upload { tag, group, staged }, true))
            }
            Write::ApplyArt { group, slot, file } => {
                if !g::is_file_id(&file) {
                    return Err(Failure::Api(GroupFailure::NotFound));
                }
                let page = self.groups_read(scope, |c| c.group(&group))?;
                check(Action::ApplyArt, &page, None)?;
                // Only a picture of the account's own inventory of that kind (Major 4).
                if !self.groups_own_file(scope, slot.tag(), &file)? {
                    return Err(Failure::Withheld(Why::NotYourPicture));
                }
                Ok(ready(Write::ApplyArt { group, slot, file }, true))
            }
        }
    }

    /// The prepared write `op`, once: re-validated, compared with what was confirmed, the scope checked under the
    /// client's lock, sent, and how it ended (kept here by its op).
    fn groups_commit(&self, link: u64, scope: &Scope, op: &str) -> Outcome {
        let intent = {
            let mut rt = self.groups.lock().unwrap();
            let Some(at) = rt.intents.iter().position(|i| i.op == op) else {
                return Outcome::Refused(if rt.used.iter().any(|u| u == op) { Failure::Duplicate } else { Failure::Expired });
            };
            // Another window's (or the overlay's) prepared write is not this one's to send.
            if rt.intents[at].link != link || rt.intents[at].scope != *scope {
                return Outcome::Refused(Failure::Expired);
            }
            let target = rt.intents[at].write.target();
            if rt.writing.contains(&target) {
                return Outcome::Refused(Failure::Busy);
            }
            if rt.hold(scope, &target).is_some() {
                rt.intents.remove(at);
                return Outcome::Refused(Failure::CheckFirst);
            }
            let intent = rt.intents.remove(at);
            rt.used.push_back(intent.op.clone());
            while rt.used.len() > USED_MAX {
                rt.used.pop_front();
            }
            rt.writing.insert(target);
            intent
        };
        let target = intent.write.target();
        let outcome = self.groups_send(scope, &intent);
        let mut rt = self.groups.lock().unwrap();
        rt.writing.remove(&target);
        rt.record(scope, op, &outcome);
        if outcome == Outcome::Unknown {
            rt.holds.push(Hold { scope: scope.clone(), op: op.to_string(), target, write: intent.write.clone(), checked: false });
            while rt.holds.len() > HOLDS_MAX {
                rt.holds.remove(0);
            }
        }
        if let Write::Upload { staged, .. } = &intent.write {
            if let Some(p) = staged.path() {
                let _ = std::fs::remove_file(p);
            }
        }
        outcome
    }

    fn groups_send(&self, scope: &Scope, intent: &Intent) -> Outcome {
        // What it was prepared against is read again and must be what the person confirmed (M3, Blocker 7).
        let fresh = match self.groups_validate(scope, intent.write.clone()) {
            Ok(r) => r,
            Err(f) => return Outcome::Refused(f),
        };
        let unchanged = match &intent.write {
            Write::MemberRole { .. } | Write::DeleteRole { .. } => fresh.role.as_ref().zip(intent.role.as_ref()).is_some_and(|(n, t)| same_role(t, n)),
            Write::EditPost { .. } | Write::DeletePost { .. } => fresh.post.as_ref().zip(intent.post.as_ref()).is_some_and(|(n, t)| g::same_post(t, n)),
            _ => true,
        };
        if !unchanged || fresh.write != intent.write {
            return Outcome::Refused(Failure::Changed);
        }
        match fresh.write {
            Write::MemberRole { group, user, role, add } => {
                self.groups_dispatch(scope, move |c| c.set_member_role(&group, &user, &role, add).map(|role_ids| Done::MemberRoles { user: user.clone(), role_ids }))
            }
            Write::CreateRole { group, draft } => self.groups_dispatch(scope, move |c| c.create_role(&group, &draft).map(Done::Role)),
            Write::EditRole { group, baseline, edit } => {
                let change = RoleChange::between(&baseline, &edit);
                self.groups_dispatch(scope, move |c| c.update_role(&group, &baseline.id, &change).map(Done::Roles))
            }
            Write::DeleteRole { group, role } => self.groups_dispatch(scope, move |c| c.delete_role(&group, &role).map(Done::Roles)),
            Write::CreatePost { group, draft } => self.groups_dispatch(scope, move |c| c.create_post(&group, &draft).map(Done::Post)),
            Write::EditPost { group, baseline, edit } => self.groups_dispatch(scope, move |c| c.edit_post(&group, &baseline, &edit).map(Done::Post)),
            Write::DeletePost { group, post } => self.groups_dispatch(scope, move |c| c.delete_post(&group, &post.id).map(|_| Done::PostDeleted { post: post.id.clone() })),
            Write::ApplyArt { group, slot, file } => self.groups_dispatch(scope, move |c| c.set_group_art(&group, slot, &file).map(|g| Done::Group(Box::new(g)))),
            Write::Upload { tag, staged, .. } => match staged.read() {
                Ok(png) => self.groups_dispatch(scope, move |c| c.upload_image(tag, png).map(Done::Uploaded)),
                Err(f) => Outcome::Refused(f),
            },
        }
    }

    /// One write: the scope checked again under the client's lock, immediately before it goes (B3).
    fn groups_dispatch(&self, scope: &Scope, f: impl FnOnce(&mut Client) -> Result<Done, GroupFailure>) -> Outcome {
        let Ok(user) = scope_user(scope) else { return Outcome::Refused(Failure::SignedOut) };
        let source = self.vrchat_source();
        let deadline = Instant::now() + Duration::from_secs(60);
        let sent = source.personal_until(&user, || true, deadline, |c| {
            if *scope != Scope::of(&self.view()) || self.view().vrchat_login.presence() != Presence::SignedIn {
                return Ok(Err(GroupFailure::Disabled));
            }
            #[cfg(test)]
            {
                self.groups.lock().unwrap().sent_ops += 1;
            }
            Ok(f(c))
        });
        match sent {
            Ok(Ok(done)) => Outcome::Done(done),
            Ok(Err(GroupFailure::Unknown)) => Outcome::Unknown,
            Ok(Err(GroupFailure::Disabled)) => Outcome::Refused(Failure::AccountChanged),
            Ok(Err(e)) => Outcome::Refused(Failure::Api(e)),
            // Refused before anything went: the session or the grant.
            Err(orbiters_core::vrchat_api::Error::Disabled) => Outcome::Refused(Failure::AccountChanged),
            Err(_) => Outcome::Refused(Failure::SignedOut),
        }
    }

    /// What became of `op` (Blockers 1 and 2): the service's own record first, then a fresh read of exactly what
    /// `write` touched. The hold on its target goes only when that read settles it.
    fn groups_check(&self, link: u64, scope: &Scope, op: &str, write: Write) -> Reply {
        let checked = |observation: Observation| Reply::Checked { op: op.to_string(), observation };
        {
            let mut rt = self.groups.lock().unwrap();
            // Prepared and never committed: nothing went.
            if let Some(at) = rt.intents.iter().position(|i| i.op == op && i.link == link && i.scope == *scope) {
                rt.intents.remove(at);
                return checked(Observation::NotSent);
            }
            if let Some((_, _, outcome, _)) = rt.records.iter().rev().find(|r| r.0 == *scope && r.1 == op) {
                if *outcome != Outcome::Unknown {
                    let outcome = outcome.clone();
                    rt.release(scope, op);
                    return checked(Observation::Recorded(outcome));
                }
            } else if rt.used.iter().any(|u| u == op) {
                return checked(Observation::Running);
            }
        }
        // The write the service itself held, when it holds one for this op; else the window's own copy.
        let write = self.groups.lock().unwrap().holds.iter().find(|h| h.scope == *scope && h.op == op).map(|h| h.write.clone()).unwrap_or(write);
        let observation = match self.groups_reconcile(scope, &write) {
            Ok(o) => o,
            Err(f) => return Reply::Failed(f),
        };
        let mut rt = self.groups.lock().unwrap();
        match observation {
            Observation::Applied | Observation::NotApplied => rt.release(scope, op),
            _ => {
                if let Some(h) = rt.holds.iter_mut().find(|h| h.scope == *scope && h.op == op) {
                    h.checked = true;
                } else {
                    // Held by the window alone (the service restarted): the person may let it go after this check.
                    rt.holds.push(Hold { scope: scope.clone(), op: op.to_string(), target: write.target(), write: write.clone(), checked: true });
                }
            }
        }
        checked(observation)
    }

    /// A fresh read of the exact resource a write touched: done, not done, or no way to tell.
    fn groups_reconcile(&self, scope: &Scope, write: &Write) -> Result<Observation, Failure> {
        use Observation::{Applied, Inconclusive, NotApplied};
        let me = scope_user(scope)?;
        Ok(match write {
            Write::MemberRole { group, user, role, add } => {
                let roles = if *user == me {
                    self.groups_read(scope, |c| c.group(group))?.my_member.map(|m| m.role_ids)
                } else {
                    self.groups_read(scope, |c| c.group_member(group, user))?.filter(|m| m.roles_known).map(|m| m.role_ids)
                };
                match roles {
                    Some(r) if r.contains(role) == *add => Applied,
                    Some(_) => NotApplied,
                    None => Inconclusive,
                }
            }
            Write::CreateRole { group, draft } => {
                let roles = self.groups_read(scope, |c| c.group(group))?.roles;
                match roles {
                    Some(r) if r.iter().any(|x| x.name.trim() == draft.name.trim()) => Applied,
                    Some(_) => NotApplied,
                    None => Inconclusive,
                }
            }
            Write::EditRole { group, baseline, edit } => {
                let now = self.groups_read(scope, |c| c.group(group))?.roles.and_then(|r| r.into_iter().find(|x| x.id == baseline.id));
                match now {
                    Some(n) if RoleChange::between(&n, edit).is_empty() => Applied,
                    Some(n) if same_role(baseline, &n) => NotApplied,
                    _ => Inconclusive,
                }
            }
            Write::DeleteRole { group, role } => match self.groups_read(scope, |c| c.group(group))?.roles {
                Some(r) if r.iter().any(|x| x.id == *role) => NotApplied,
                Some(_) => Applied,
                None => Inconclusive,
            },
            Write::CreatePost { group, draft } => {
                // Looked for among the newest posts by this account with these words; not seen there proves nothing
                // unless every post was seen.
                let mut offset = 0;
                let mut seen_all = false;
                let mut found = false;
                for _ in 0..POST_PAGES {
                    let page = self.groups_read(scope, |c| c.group_posts(group, offset))?;
                    found |= page.items.iter().any(|p| p.author_id == me && p.title == draft.title.trim() && p.text == draft.text.trim());
                    offset += page.raw;
                    if found || page.raw < orbiters_core::vrchat_api::GROUP_PAGE || page.total.is_some_and(|t| offset as u32 >= t) {
                        seen_all = !found;
                        break;
                    }
                }
                if found { Applied } else if seen_all { NotApplied } else { Inconclusive }
            }
            Write::EditPost { group, baseline, edit } => match self.groups_find_post(scope, group, &baseline.id)?.0 {
                Some(p) if p.title == edit.title.trim() && p.text == edit.text.trim() && p.visibility == edit.visibility => Applied,
                Some(p) if g::same_post(&p, baseline) => NotApplied,
                _ => Inconclusive,
            },
            Write::DeletePost { group, post } => match self.groups_find_post(scope, group, &post.id)? {
                (Some(_), _) => NotApplied,
                (None, true) => Applied,
                (None, false) => Inconclusive,
            },
            // An upload cannot be told apart from the account's other pictures: the person looks at the gallery.
            Write::Upload { .. } => Inconclusive,
            Write::ApplyArt { group, slot, file } => {
                let now = self.groups_read(scope, |c| c.group(group))?;
                let id = match slot {
                    ArtSlot::Banner => now.banner_id,
                    ArtSlot::Icon => now.icon_id,
                };
                if id.as_deref() == Some(file.as_str()) { Applied } else { NotApplied }
            }
        })
    }
}

/// What a write needs once checked.
struct Ready {
    write: Write,
    confirm: bool,
    role: Option<Role>,
    post: Option<Post>,
    member: Option<Member>,
}

/// The account has VRChat+ (`system_supporter`), which gallery and icon uploads need.
fn vrchat_plus(me: &serde_json::Value) -> bool {
    me.get("tags").and_then(|t| t.as_array()).is_some_and(|t| t.iter().any(|x| x.as_str() == Some("system_supporter")))
}

fn fill_card(card: &mut GroupCard, group: &Group, me: &str) {
    card.name = group.name.clone();
    card.short_code = group.short_code.clone();
    card.discriminator = group.discriminator.clone();
    card.icon_url = group.icon_url.clone();
    card.banner_url = group.banner_url.clone();
    card.member_count = Some(group.member_count);
    card.online_count = group.online_count;
    card.owner = card.owner || group.owner_id == me;
    card.named = true;
}

fn scope_user(scope: &Scope) -> Result<String, Failure> {
    scope.vrchat_user().map(str::to_string).ok_or(Failure::SignedOut)
}

/// A post as it goes out: an audience this version cannot carry whole is refused, never narrowed or widened quietly
/// (Blocker 3).
fn clean_post(draft: PostDraft) -> Result<PostDraft, Failure> {
    let title = draft.title.trim().to_string();
    let text = draft.text.trim().to_string();
    if title.is_empty() || text.is_empty() || title.chars().count() > g::POST_TITLE_MAX || text.chars().count() > g::POST_TEXT_MAX {
        return Err(Failure::Api(GroupFailure::Invalid));
    }
    if draft.role_ids.len() > 32 || draft.role_ids.iter().any(|r| !g::is_role_id(r)) || draft.image_id.as_deref().is_some_and(|i| !g::is_file_id(i)) {
        return Err(Failure::Api(GroupFailure::Invalid));
    }
    if draft.visibility == g::Visibility::Public && !draft.role_ids.is_empty() {
        return Err(Failure::Api(GroupFailure::Invalid));
    }
    Ok(PostDraft { title, text, ..draft })
}

#[cfg(test)]
#[path = "groups_tests.rs"]
mod tests;
