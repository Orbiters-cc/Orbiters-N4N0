//! VRChat groups for the window (REQUEST22 1; DESIGN22's revision after the attack): Home's Groups section, the group
//! page's reads and its single, guarded writes.
//!
//! - Nothing of a group goes in the snapshot (M9): every reply goes to the window that asked, on demand
//!   (`ToService::Groups` → `ToClient::Groups`). Only the window may ask; the overlay, voice and the generic commands
//!   cannot (M10).
//! - Every request is bound to its [`Scope`] (environment, Orbiters account, the signed-in VRChat account, B3) and the
//!   ids it names; a cached account in "Connecting" grants nothing. The scope is checked again under the client's lock
//!   right before each write is dispatched, and before a reply goes out.
//! - Every write is prepared ([`Request::Prepare`]: fresh reads, the rules of `orbiters_core::groups::can`, the exact
//!   payload) and then committed once ([`Request::Commit`]) by the same window with a one-use operation id kept here
//!   (B2, M3, M10). A second commit of the same id is dropped. Its outcome is done, refused or UNKNOWN; after an unknown
//!   one nothing about that target is sent again until the person checks ([`Request::Check`]).
//! - One write at a time per target, a few requests at a time in all; the client's lock is let go between calls (M9,
//!   M11). No bulk, no background sweeps.
use super::vrchat_data::Scope;
use super::vrchat_login::Presence;
use super::Controller;
use orbiters_core::groups::{self as g, Action, Actor, ArtSlot, Catalogue, FileTag, Group, GroupCard, InventoryFile, Member, PermissionInfo, Post, PostDraft, Role, RoleChange, RoleDraft, Verdict, Why};
use orbiters_core::vrchat_api::{Client, GroupFailure, Members, Posts};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

/// The link of the in-process window (no tray service between).
pub const LOCAL_LINK: u64 = 0;
/// A prepared write is committed within this time or prepared again.
const INTENT_FOR: Duration = Duration::from_secs(300);
/// Prepared writes kept at once, and ids remembered as used (to tell a duplicate from an expired one).
const INTENTS_MAX: usize = 16;
const USED_MAX: usize = 128;
/// Requests at once (each lets the client's lock go between its calls).
const RUNNING_MAX: usize = 3;
/// Groups on Home, and those read one by one when the account's list does not show them (hidden memberships).
const GROUPS_MAX: usize = 100;
const HYDRATE_MAX: usize = 12;
/// A picture of the page at most (memory only).
const PICTURE_MAX: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Home's Groups section: the groups this account owns or helps run.
    Discover,
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
    /// After an unknown outcome: the group read again, and what was held for it let go.
    Check { group: String },
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
    DeletePost { group: String, post: Post },
    /// A PNG into the account's own gallery or icons: a step of its own (M6).
    Upload {
        tag: FileTag,
        #[serde(with = "super::vrchat_login::bytes_b64")]
        png: Vec<u8>,
    },
    ApplyArt { group: String, slot: ArtSlot, file: String },
}

impl Write {
    pub fn group(&self) -> Option<&str> {
        match self {
            Write::MemberRole { group, .. }
            | Write::CreateRole { group, .. }
            | Write::EditRole { group, .. }
            | Write::DeleteRole { group, .. }
            | Write::CreatePost { group, .. }
            | Write::EditPost { group, .. }
            | Write::DeletePost { group, .. }
            | Write::ApplyArt { group, .. } => Some(group),
            Write::Upload { .. } => None,
        }
    }
    /// What one write at a time is counted on, and what an unknown outcome holds.
    fn target(&self) -> String {
        match self {
            Write::MemberRole { group, user, .. } => format!("member:{group}:{user}"),
            Write::CreateRole { group, .. } | Write::EditRole { group, .. } | Write::DeleteRole { group, .. } => format!("roles:{group}"),
            Write::CreatePost { group, .. } | Write::EditPost { group, .. } | Write::DeletePost { group, .. } => format!("posts:{group}"),
            Write::ApplyArt { group, .. } => format!("art:{group}"),
            Write::Upload { .. } => "upload".into(),
        }
    }
}

/// A write prepared: its one-use id, what will be sent (normalised), and whether the person confirms it first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Prepared {
    pub op: String,
    pub write: Write,
    /// Public, destructive or privilege-bearing: the window shows what will be sent and waits for the person.
    pub confirm: bool,
    /// The role concerned, as read just now (a member's role change, a role's deletion).
    pub role: Option<Role>,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Done(Done),
    /// Nothing was done: why.
    Refused(Failure),
    /// It was sent and its answer was lost: it may have been done. Nothing is retried; check before sending again.
    Unknown,
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
    /// What it was made against changed since (the role, the member's roles).
    Changed,
    /// The last write here may have been done: check first.
    CheckFirst,
    /// Only the window asks this.
    NotHere,
    /// Over the size this accepts.
    TooLarge,
}

/// The group's page as read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub group: Group,
    /// The rights a role can hold: read only with rights over roles.
    pub catalogue: Option<Vec<PermissionInfo>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    Discovered(Vec<GroupCard>),
    Opened(Box<Page>),
    Members(Members),
    Posts(Posts),
    Inventory(Vec<InventoryFile>),
    Picture(#[serde(with = "super::vrchat_login::bytes_b64")] Vec<u8>),
    Prepared(Box<Prepared>),
    Outcome(Outcome),
    Cancelled,
    Failed(Failure),
}

struct Intent {
    op: String,
    scope: Scope,
    link: u64,
    write: Write,
    role: Option<Role>,
    at: Instant,
}

/// The service's side of groups: prepared writes, ids used, what an unknown outcome holds, targets being written.
#[derive(Default)]
pub(crate) struct Runtime {
    intents: Vec<Intent>,
    used: VecDeque<String>,
    held: Vec<(Scope, String)>,
    writing: HashSet<String>,
    running: usize,
    #[cfg(test)]
    pub(crate) sent_ops: u64,
}

impl Runtime {
    /// Everything prepared under another scope goes (B3), and what is too old.
    fn forget_others(&mut self, scope: &Scope) {
        self.intents.retain(|i| i.scope == *scope && i.at.elapsed() < INTENT_FOR);
        self.held.retain(|(s, _)| s == scope);
    }
}

/// The requests the window may make, and nobody else (M10).
pub fn allowed_from(role: crate::ipc::Role) -> bool {
    role == crate::ipc::Role::Window
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

/// The same role for the purpose of a write made against `then`: its rights, rank and flags unchanged.
fn same_role(then: &Role, now: &Role) -> bool {
    let mut a = then.permissions.clone();
    let mut b = now.permissions.clone();
    a.sort();
    b.sort();
    a == b
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
    /// A request of the window's (link `link`) under `scope`. Blocking: call on a thread of its own.
    pub fn groups(&self, link: u64, scope: Scope, request: Request) -> Reply {
        if scope != Scope::of(&self.view()) {
            return Reply::Failed(Failure::AccountChanged);
        }
        if self.view().vrchat_login.presence() != Presence::SignedIn {
            return Reply::Failed(Failure::SignedOut);
        }
        {
            let mut rt = self.groups.lock().unwrap();
            rt.forget_others(&scope);
            // Cancelling and committing never wait behind reads.
            if rt.running >= RUNNING_MAX && !matches!(request, Request::Cancel { .. } | Request::Commit { .. }) {
                return Reply::Failed(Failure::Busy);
            }
            rt.running += 1;
        }
        let _slot = Slot(self);
        let reply = match request {
            Request::Discover => self.groups_discover(&scope).map(Reply::Discovered),
            Request::Open { group } => self.groups_open(&scope, &group).map(|p| Reply::Opened(Box::new(p))),
            Request::Check { group } => {
                let page = self.groups_open(&scope, &group);
                if page.is_ok() {
                    self.groups.lock().unwrap().held.retain(|(s, key)| !(*s == scope && (key.contains(&group) || key == "upload")));
                }
                page.map(|p| Reply::Opened(Box::new(p)))
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
            Request::Prepare { write } => self.groups_prepare(link, &scope, write).map(|p| Reply::Prepared(Box::new(p))),
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
        // Memberships the account's list does not show (hidden ones): read one by one, a few at most.
        let mut read = 0;
        for card in cards.iter_mut().filter(|c| c.name.is_empty()) {
            if read == HYDRATE_MAX {
                break;
            }
            read += 1;
            if let Ok(group) = self.groups_read(scope, |c| c.group(&card.id)) {
                card.name = group.name;
                card.short_code = group.short_code;
                card.discriminator = group.discriminator;
                card.icon_url = group.icon_url;
                card.banner_url = group.banner_url;
                card.member_count = group.member_count;
                card.online_count = group.online_count;
                card.owner = card.owner || group.owner_id == me;
            }
        }
        cards.sort_by(|a, b| b.owner.cmp(&a.owner).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        cards.truncate(GROUPS_MAX);
        Ok(cards)
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

    /// Fresh reads and the rules, then the write kept here under a one-use id.
    fn groups_prepare(&self, link: u64, scope: &Scope, write: Write) -> Result<Prepared, Failure> {
        let target = write.target();
        if self.groups.lock().unwrap().held.iter().any(|(s, key)| s == scope && *key == target) {
            return Err(Failure::CheckFirst);
        }
        let (write, confirm, role) = self.groups_validate(scope, write)?;
        let op = uuid::Uuid::new_v4().to_string();
        let mut rt = self.groups.lock().unwrap();
        rt.intents.retain(|i| i.link != link || i.write.target() != target);
        if rt.intents.len() >= INTENTS_MAX {
            rt.intents.remove(0);
        }
        rt.intents.push(Intent { op: op.clone(), scope: scope.clone(), link, write: write.clone(), role: role.clone(), at: Instant::now() });
        Ok(Prepared { op, write, confirm, role })
    }

    /// The write checked against what VRChat says now: normalised, whether it is confirmed, the role it concerns.
    fn groups_validate(&self, scope: &Scope, write: Write) -> Result<(Write, bool, Option<Role>), Failure> {
        match write {
            Write::MemberRole { group, user, role, add } => {
                let page = self.groups_read(scope, |c| c.group(&group))?;
                let me = scope_user(scope)?;
                // The member as VRChat has them now (a lookup of oneself gives no name: the account's own membership counts).
                let current: Vec<String> = if user == me {
                    page.my_member.as_ref().map(|m| m.role_ids.clone()).ok_or(Failure::Api(GroupFailure::Forbidden))?
                } else {
                    let member: Member = self.groups_read(scope, |c| c.group_member(&group, &user))?.ok_or(Failure::Api(GroupFailure::NotFound))?;
                    if member.user_id != user {
                        return Err(Failure::Changed);
                    }
                    member.role_ids
                };
                if current.contains(&role) == add {
                    return Err(Failure::Changed);
                }
                check(Action::AssignRole { role: &role, user: &user, add }, &page, None)?;
                let found = page.roles.as_ref().and_then(|r| r.iter().find(|x| x.id == role)).cloned().ok_or(Failure::Withheld(Why::MissingData))?;
                let confirm = found.privileged();
                Ok((Write::MemberRole { group, user, role, add }, confirm, Some(found)))
            }
            Write::CreateRole { group, draft } => {
                let draft = RoleDraft { name: g::label(&draft.name, g::ROLE_NAME_MAX), description: g::text(&draft.description, g::ROLE_DESCRIPTION_MAX), ..draft };
                if draft.name.chars().count() < 1 {
                    return Err(Failure::Api(GroupFailure::Invalid));
                }
                let page = self.groups_open(scope, &group)?;
                check(Action::CreateRole { draft: &draft }, &page.group, page.catalogue.as_deref())?;
                let confirm = !draft.permissions.is_empty() || draft.self_assignable || draft.added_on_join || draft.requires_two_factor;
                Ok((Write::CreateRole { group, draft }, confirm, None))
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
                let confirm = change.consequential();
                Ok((Write::EditRole { group, baseline: now.clone(), edit }, confirm, Some(now)))
            }
            Write::DeleteRole { group, role } => {
                let page = self.groups_read(scope, |c| c.group(&group))?;
                check(Action::DeleteRole { role: &role }, &page, None)?;
                let found = page.roles.as_ref().and_then(|r| r.iter().find(|x| x.id == role)).cloned();
                Ok((Write::DeleteRole { group, role }, true, found))
            }
            Write::CreatePost { group, draft } => {
                let draft = clean_post(draft)?;
                let page = self.groups_read(scope, |c| c.group(&group))?;
                check(Action::Post, &page, None)?;
                Ok((Write::CreatePost { group, draft }, true, None))
            }
            Write::EditPost { group, baseline, edit } => {
                let edit = PostDraft { send_notification: false, ..clean_post(edit)? };
                if !g::is_post_id(&baseline.id) {
                    return Err(Failure::Api(GroupFailure::NotFound));
                }
                let page = self.groups_read(scope, |c| c.group(&group))?;
                check(Action::Post, &page, None)?;
                Ok((Write::EditPost { group, baseline, edit }, true, None))
            }
            Write::DeletePost { group, post } => {
                if !g::is_post_id(&post.id) {
                    return Err(Failure::Api(GroupFailure::NotFound));
                }
                let page = self.groups_read(scope, |c| c.group(&group))?;
                check(Action::Post, &page, None)?;
                Ok((Write::DeletePost { group, post }, true, None))
            }
            Write::Upload { tag, png } => {
                if png.len() > orbiters_core::vrchat_api::GROUP_UPLOAD_MAX {
                    return Err(Failure::TooLarge);
                }
                if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
                    return Err(Failure::Api(GroupFailure::Invalid));
                }
                // Into the person's own inventory: public there, so confirmed.
                Ok((Write::Upload { tag, png }, true, None))
            }
            Write::ApplyArt { group, slot, file } => {
                if !g::is_file_id(&file) {
                    return Err(Failure::Api(GroupFailure::NotFound));
                }
                let page = self.groups_read(scope, |c| c.group(&group))?;
                check(Action::ApplyArt, &page, None)?;
                Ok((Write::ApplyArt { group, slot, file }, true, None))
            }
        }
    }

    /// The prepared write `op`, once: re-validated, the scope checked under the client's lock, sent, and how it ended.
    fn groups_commit(&self, link: u64, scope: &Scope, op: &str) -> Outcome {
        let intent = {
            let mut rt = self.groups.lock().unwrap();
            let Some(at) = rt.intents.iter().position(|i| i.op == op) else {
                return Outcome::Refused(if rt.used.iter().any(|u| u == op) { Failure::Duplicate } else { Failure::Expired });
            };
            // Another window's (or the overlay's) prepared write is not this one's to send.
            if rt.intents[at].link != link || rt.intents[at].scope != *scope {
                rt.intents.remove(at);
                return Outcome::Refused(Failure::Expired);
            }
            let target = rt.intents[at].write.target();
            if rt.writing.contains(&target) {
                return Outcome::Refused(Failure::Busy);
            }
            if rt.held.iter().any(|(s, key)| s == scope && *key == target) {
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
        if outcome == Outcome::Unknown {
            rt.held.push((scope.clone(), target));
        }
        outcome
    }

    fn groups_send(&self, scope: &Scope, intent: &Intent) -> Outcome {
        // What it was prepared against is read again: a member's roles and the role's definition (M3).
        match &intent.write {
            Write::MemberRole { group, user, role, add } => {
                let then = intent.role.clone();
                match self.groups_validate(scope, intent.write.clone()) {
                    Ok((_, _, now)) if now.as_ref().zip(then.as_ref()).is_some_and(|(n, t)| same_role(t, n)) => {}
                    Ok(_) => return Outcome::Refused(Failure::Changed),
                    Err(f) => return Outcome::Refused(f),
                }
                let (group, user, role, add) = (group.clone(), user.clone(), role.clone(), *add);
                self.groups_dispatch(scope, move |c| c.set_member_role(&group, &user, &role, add).map(|role_ids| Done::MemberRoles { user: user.clone(), role_ids }))
            }
            Write::EditRole { .. } | Write::DeleteRole { .. } | Write::ApplyArt { .. } | Write::CreateRole { .. } | Write::CreatePost { .. } | Write::EditPost { .. } | Write::DeletePost { .. } => {
                // The rules once more on what VRChat says now (a right lost since the press).
                let fresh = match self.groups_validate(scope, intent.write.clone()) {
                    Ok((w, _, _)) => w,
                    Err(f) => return Outcome::Refused(f),
                };
                if fresh != intent.write {
                    return Outcome::Refused(Failure::Changed);
                }
                match fresh {
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
                    Write::MemberRole { .. } | Write::Upload { .. } => unreachable!(),
                }
            }
            Write::Upload { tag, png } => {
                let (tag, png) = (*tag, png.clone());
                self.groups_dispatch(scope, move |c| c.upload_image(tag, png).map(Done::Uploaded))
            }
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
}

fn scope_user(scope: &Scope) -> Result<String, Failure> {
    scope.vrchat_user().map(str::to_string).ok_or(Failure::SignedOut)
}

fn clean_post(draft: PostDraft) -> Result<PostDraft, Failure> {
    let title = g::label(&draft.title, g::POST_TITLE_MAX);
    let text = g::text(&draft.text, g::POST_TEXT_MAX);
    if title.is_empty() || text.is_empty() {
        return Err(Failure::Api(GroupFailure::Invalid));
    }
    let role_ids = if draft.visibility == g::Visibility::Group { draft.role_ids.into_iter().filter(|r| g::is_role_id(r)).take(32).collect() } else { Vec::new() };
    let image_id = draft.image_id.filter(|i| g::is_file_id(i));
    Ok(PostDraft { title, text, role_ids, image_id, ..draft })
}

#[cfg(test)]
#[path = "groups_tests.rs"]
mod tests;
