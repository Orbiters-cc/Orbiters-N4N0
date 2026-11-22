//! VRChat groups the signed-in account owns or helps run (REQUEST22 1, DESIGN22 and its revision after the attack): what
//! is read of them, who may do what (`can`, the interface's guide only: VRChat stays the authority), and the plain-text
//! rules every string from VRChat goes through before it is drawn.
//!
//! Shapes from the community OpenAPI spec (vrchatapi/specification 68091ebc: `Group`, `GroupMyMember`, `GroupRole`,
//! `GroupPermission`, `GroupMember`, `GroupPost`, `LimitedUserGroups`, `File`); the role rules as VRCX implements them
//! (e819c4e4, `GroupRolesDialog.vue`: the actor's lowest role order, strictly-below targets, owner and Everyone fixed,
//! self-assignment apart). Ids are strings everywhere, and nothing here is keyed by a number (0.3.6's overlay loop).
use serde::{Deserialize, Serialize};

/// The wildcard VRChat gives the owner's role: every right.
pub const ALL: &str = "*";
pub const ANNOUNCEMENT_MANAGE: &str = "group-announcement-manage";
pub const DATA_MANAGE: &str = "group-data-manage";
pub const DEFAULT_ROLE_MANAGE: &str = "group-default-role-manage";
pub const MEMBERS_MANAGE: &str = "group-members-manage";
pub const MEMBERS_VIEWALL: &str = "group-members-viewall";
pub const ROLES_ASSIGN: &str = "group-roles-assign";
pub const ROLES_MANAGE: &str = "group-roles-manage";

/// The rights that make someone one of a group's managers or moderators (M1: "moderating / admin rights"), whether or
/// not this version has a control for them (bans, kicks, instance moderation are on vrchat.com). Viewing every member,
/// joining, queue priority, creating instances or self-assignable roles do not. A string VRChat adds later grants
/// nothing here until it is listed.
pub const MANAGING: [&str; 15] = [
    ALL,
    ANNOUNCEMENT_MANAGE,
    "group-audit-view",
    "group-bans-manage",
    "group-calendar-manage",
    DATA_MANAGE,
    DEFAULT_ROLE_MANAGE,
    "group-galleries-manage",
    "group-instance-manage",
    "group-instance-moderate",
    "group-invites-manage",
    MEMBERS_MANAGE,
    "group-members-remove",
    ROLES_ASSIGN,
    ROLES_MANAGE,
];

/// Whether `permissions` hold `wanted` (the wildcard holds every one).
pub fn has(permissions: &[String], wanted: &str) -> bool {
    permissions.iter().any(|p| p == ALL || p == wanted)
}

/// Whether a group goes in Home's Groups section: its owner, or any managing right (M1). Viewing all members alone does
/// not list it.
pub fn listed(permissions: &[String], owner: bool) -> bool {
    owner || permissions.iter().any(|p| MANAGING.contains(&p.as_str()))
}

/// What this version lets the person do in a group, for its card and its page's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Abilities {
    pub roles: bool,
    pub assign: bool,
    pub posts: bool,
    pub art: bool,
    /// Rights this version has no control for (bans, kicks, instances, invites, audit log, calendar): the page links to
    /// vrchat.com for them.
    pub elsewhere: bool,
}

pub fn abilities(permissions: &[String]) -> Abilities {
    const HERE: [&str; 6] = [ANNOUNCEMENT_MANAGE, DATA_MANAGE, DEFAULT_ROLE_MANAGE, MEMBERS_MANAGE, ROLES_ASSIGN, ROLES_MANAGE];
    Abilities {
        roles: has(permissions, ROLES_MANAGE) || has(permissions, DEFAULT_ROLE_MANAGE),
        assign: has(permissions, ROLES_ASSIGN),
        posts: has(permissions, ANNOUNCEMENT_MANAGE),
        art: has(permissions, DATA_MANAGE),
        elsewhere: permissions.iter().any(|p| p == ALL || (MANAGING.contains(&p.as_str()) && !HERE.contains(&p.as_str()))),
    }
}

/// A group on Home: what its card shows. `id` is the GROUP's id (`grp_…`), never the membership's (`gmem_…`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupCard {
    pub id: String,
    pub name: String,
    pub short_code: String,
    pub discriminator: String,
    pub icon_url: String,
    pub banner_url: String,
    pub member_count: u32,
    pub online_count: Option<u32>,
    pub owner: bool,
    pub permissions: Vec<String>,
}

impl GroupCard {
    pub fn abilities(&self) -> Abilities {
        abilities(&self.permissions)
    }
}

/// The signed-in account's own membership (`myMember`). Manager notes are never read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MyMember {
    pub user_id: String,
    pub role_ids: Vec<String>,
    pub permissions: Vec<String>,
    /// `member`, `requested`, `invited`, `banned`…; only `member` may assign itself a role.
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub short_code: String,
    pub discriminator: String,
    pub description: String,
    pub rules: String,
    pub links: Vec<String>,
    pub icon_id: Option<String>,
    pub icon_url: String,
    pub banner_id: Option<String>,
    pub banner_url: String,
    pub member_count: u32,
    pub online_count: Option<u32>,
    pub owner_id: String,
    pub privacy: String,
    pub my_member: Option<MyMember>,
    /// `None` when VRChat did not give them: then no write that depends on role order is offered.
    pub roles: Option<Vec<Role>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Role {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Smaller is higher. `None` when missing: then nothing compares against it.
    pub order: Option<i64>,
    pub permissions: Vec<String>,
    pub default_role: bool,
    pub management: bool,
    pub self_assignable: bool,
    pub added_on_join: bool,
    pub requires_two_factor: bool,
    pub requires_purchase: bool,
    pub updated_at: Option<String>,
}

impl Role {
    /// The owner's role: it holds the wildcard.
    pub fn is_owner_role(&self) -> bool {
        self.permissions.iter().any(|p| p == ALL)
    }
    /// A grant or removal of this role changes what someone may do (M3): it is confirmed.
    pub fn privileged(&self) -> bool {
        self.management || !self.permissions.is_empty()
    }
}

/// One right a role can hold, as `G/permissions` describes it for this account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionInfo {
    pub name: String,
    pub display_name: String,
    pub help: String,
    pub allowed_to_add: bool,
    pub depends_on: Vec<String>,
    pub management: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub user_id: String,
    /// The membership's own id (`gmem_…`): never used as a user's.
    pub membership_id: String,
    /// Missing when VRChat left the nested user out (a single read): such a record authorises nothing (M8).
    pub display_name: Option<String>,
    pub icon_url: String,
    pub role_ids: Vec<String>,
    pub joined_at: Option<String>,
}

impl Member {
    /// Whole enough to act on: a user id, its name, its roles.
    pub fn complete(&self) -> bool {
        crate::vrchat_accounts::is_user_id(&self.user_id) && self.display_name.is_some()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    #[default]
    Group,
    Public,
}

impl Visibility {
    pub fn wire(self) -> &'static str {
        match self {
            Visibility::Group => "group", // i18n: not shown (VRChat's API value)
            Visibility::Public => "public", // i18n: not shown (VRChat's API value)
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Post {
    pub id: String,
    pub title: String,
    pub text: String,
    pub visibility: Visibility,
    pub role_ids: Vec<String>,
    pub image_id: Option<String>,
    pub image_url: String,
    pub author_id: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A picture of the account's own VRChat inventory (`/files?tag=gallery|icon`): its file id (what a group stores) and
/// its latest version's address (what is drawn).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InventoryFile {
    pub id: String,
    pub url: String,
}

/// Which of the account's inventories: banners come from the gallery, icons from the icons (VRCX's picker).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileTag {
    Gallery,
    Icon,
}

impl FileTag {
    pub fn wire(self) -> &'static str {
        match self {
            FileTag::Gallery => "gallery", // i18n: not shown (VRChat's API value)
            FileTag::Icon => "icon", // i18n: not shown (VRChat's API value)
        }
    }
}

/// Which picture of a group a file goes to (`bannerId`, `iconId`). The nameplate slot is not one: v1 never sends
/// `nameplateId` (B4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ArtSlot {
    Banner,
    Icon,
}

impl ArtSlot {
    pub fn tag(self) -> FileTag {
        match self {
            ArtSlot::Banner => FileTag::Gallery,
            ArtSlot::Icon => FileTag::Icon,
        }
    }
}

/// A new role as the person filled it in. `requiresPurchase` is never offered (paid roles are VRChat's store's).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleDraft {
    pub name: String,
    pub description: String,
    pub permissions: Vec<String>,
    pub self_assignable: bool,
    pub added_on_join: bool,
    pub requires_two_factor: bool,
}

impl RoleDraft {
    pub fn of(role: &Role) -> Self {
        Self {
            name: role.name.clone(),
            description: role.description.clone(),
            permissions: role.permissions.clone(),
            self_assignable: role.self_assignable,
            added_on_join: role.added_on_join,
            requires_two_factor: role.requires_two_factor,
        }
    }
}

/// The fields of a role edit that differ from the baseline it was made against: only those are sent (M2, M4), so a right
/// this version does not know is kept by being left out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleChange {
    pub name: Option<String>,
    pub description: Option<String>,
    pub permissions: Option<Vec<String>>,
    pub self_assignable: Option<bool>,
    pub added_on_join: Option<bool>,
    pub requires_two_factor: Option<bool>,
}

fn same_set(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x)) && b.iter().all(|x| a.contains(x))
}

impl RoleChange {
    pub fn between(baseline: &Role, edited: &RoleDraft) -> Self {
        let changed = |a: bool, b: bool| (a != b).then_some(b);
        Self {
            name: (edited.name.trim() != baseline.name).then(|| edited.name.trim().to_string()),
            description: (edited.description != baseline.description).then(|| edited.description.clone()),
            permissions: (!same_set(&baseline.permissions, &edited.permissions)).then(|| edited.permissions.clone()),
            self_assignable: changed(baseline.self_assignable, edited.self_assignable),
            added_on_join: changed(baseline.added_on_join, edited.added_on_join),
            requires_two_factor: changed(baseline.requires_two_factor, edited.requires_two_factor),
        }
    }
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
    /// It changes what members may do, now or when they join (M2): confirmed before it is sent.
    pub fn consequential(&self) -> bool {
        self.permissions.is_some() || self.self_assignable.is_some() || self.added_on_join.is_some() || self.requires_two_factor.is_some()
    }
    fn touches_details(&self) -> bool {
        self.name.is_some() || self.description.is_some() || self.requires_two_factor.is_some()
    }
    fn touches_flags(&self) -> bool {
        self.self_assignable.is_some() || self.added_on_join.is_some()
    }
}

/// A post as the person wrote it. The notification is off unless they turn it on (M7), and only creation can send one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostDraft {
    pub title: String,
    pub text: String,
    pub visibility: Visibility,
    pub role_ids: Vec<String>,
    pub image_id: Option<String>,
    pub send_notification: bool,
}

pub const POST_TITLE_MAX: usize = 64;
pub const POST_TEXT_MAX: usize = 2000;
pub const ROLE_NAME_MAX: usize = 64;
pub const ROLE_DESCRIPTION_MAX: usize = 512;

/// Why the interface withholds an action. VRChat may still refuse what is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Why {
    /// The account's rights in this group do not include it.
    NoPermission,
    /// The role is at or above the account's own highest role.
    RoleAtOrAbove,
    /// Everyone's or the owner's role: not deleted, not assigned, its details fixed.
    FixedRole,
    /// A role bought in VRChat's store.
    PaidRole,
    /// The owner's own role: its rights cannot be changed.
    OwnerRole,
    /// A right this account may not give (or the wildcard).
    NotGrantable,
    /// A right without the one it depends on.
    Prerequisite,
    /// Roles, their order or the rights' list are missing or incomplete: nothing is guessed.
    MissingData,
    /// The role is not one members may take themselves.
    NotSelfAssignable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    Withheld(Why),
}

impl Verdict {
    pub fn allowed(self) -> bool {
        self == Verdict::Allowed
    }
}

/// The signed-in account in the group, freshly read.
#[derive(Clone, Copy, Debug)]
pub struct Actor<'a> {
    pub user_id: &'a str,
    pub permissions: &'a [String],
    pub role_ids: &'a [String],
    pub status: &'a str,
}

impl<'a> Actor<'a> {
    pub fn of(me: &'a MyMember) -> Self {
        Self { user_id: &me.user_id, permissions: &me.permissions, role_ids: &me.role_ids, status: &me.status }
    }
}

/// The group's roles and the rights' catalogue, as read.
#[derive(Clone, Copy, Debug, Default)]
pub struct Catalogue<'a> {
    pub roles: Option<&'a [Role]>,
    pub permissions: Option<&'a [PermissionInfo]>,
}

#[derive(Clone, Copy, Debug)]
pub enum Action<'a> {
    /// Give (`add`) or take back a role from the member `user`.
    AssignRole { role: &'a str, user: &'a str, add: bool },
    CreateRole { draft: &'a RoleDraft },
    EditRole { role: &'a str, change: &'a RoleChange },
    DeleteRole { role: &'a str },
    /// Write, edit or delete a post.
    Post,
    /// Set the group's banner or icon to a file.
    ApplyArt,
}

/// The actor's highest role's order (the smallest number): `None` when a role of theirs or its order is not known.
fn top_order(actor: &Actor, roles: &[Role]) -> Option<i64> {
    if actor.role_ids.is_empty() {
        return None;
    }
    let mut top = i64::MAX;
    for id in actor.role_ids {
        top = top.min(roles.iter().find(|r| r.id == *id)?.order?);
    }
    Some(top)
}

/// Whether the actor may change a role at all, by rank: strictly below their own highest role.
fn below(actor: &Actor, roles: &[Role], role: &Role) -> Verdict {
    match (top_order(actor, roles), role.order) {
        (Some(top), Some(order)) if order > top => Verdict::Allowed,
        (Some(_), Some(_)) => Verdict::Withheld(Why::RoleAtOrAbove),
        _ => Verdict::Withheld(Why::MissingData),
    }
}

/// Every right added or removed between `from` and `to` may be given by this account, and every right of `to` has what
/// it depends on.
fn grantable(from: &[String], to: &[String], catalogue: Option<&[PermissionInfo]>) -> Verdict {
    if to.iter().any(|p| p == ALL) {
        return Verdict::Withheld(Why::NotGrantable);
    }
    let changed: Vec<&String> = to.iter().filter(|p| !from.contains(p)).chain(from.iter().filter(|p| !to.contains(p))).collect();
    if changed.is_empty() {
        return Verdict::Allowed;
    }
    let Some(catalogue) = catalogue else { return Verdict::Withheld(Why::MissingData) };
    for p in &changed {
        match catalogue.iter().find(|c| c.name == **p) {
            Some(c) if c.allowed_to_add => {}
            _ => return Verdict::Withheld(Why::NotGrantable),
        }
    }
    for p in to {
        if let Some(c) = catalogue.iter().find(|c| c.name == *p) {
            if c.depends_on.iter().any(|d| !to.contains(d)) {
                return Verdict::Withheld(Why::Prerequisite);
            }
        }
    }
    Verdict::Allowed
}

/// What the interface offers (M2): from the actor's fresh state, the target, the proposed change and the catalogue.
/// Missing data withholds. This mirrors VRCX's rules; it is no proof of what VRChat's server accepts.
pub fn can(action: Action, actor: &Actor, catalogue: &Catalogue) -> Verdict {
    use Verdict::{Allowed, Withheld};
    let rights = actor.permissions;
    let role_of = |id: &str| catalogue.roles.and_then(|roles| roles.iter().find(|r| r.id == id));
    match action {
        Action::Post => if has(rights, ANNOUNCEMENT_MANAGE) { Allowed } else { Withheld(Why::NoPermission) },
        Action::ApplyArt => if has(rights, DATA_MANAGE) { Allowed } else { Withheld(Why::NoPermission) },
        Action::AssignRole { role, user, .. } => {
            let (Some(roles), Some(role)) = (catalogue.roles, role_of(role)) else { return Withheld(Why::MissingData) };
            if role.default_role {
                return Withheld(Why::FixedRole);
            }
            if role.is_owner_role() {
                return Withheld(Why::OwnerRole);
            }
            if role.requires_purchase {
                return Withheld(Why::PaidRole);
            }
            if user == actor.user_id {
                // Taking or leaving a role oneself: one members may take, or one below one's own with the right to assign.
                if actor.status != "member" {
                    return Withheld(Why::NoPermission);
                }
                if role.self_assignable {
                    return Allowed;
                }
                if !has(rights, ROLES_ASSIGN) {
                    return Withheld(Why::NotSelfAssignable);
                }
                return below(actor, roles, role);
            }
            if !(has(rights, ROLES_ASSIGN) && has(rights, MEMBERS_MANAGE)) {
                return Withheld(Why::NoPermission);
            }
            below(actor, roles, role)
        }
        Action::CreateRole { draft } => {
            if !has(rights, ROLES_MANAGE) {
                return Withheld(Why::NoPermission);
            }
            if catalogue.roles.is_none() {
                return Withheld(Why::MissingData);
            }
            grantable(&[], &draft.permissions, catalogue.permissions)
        }
        Action::EditRole { role, change } => {
            let (Some(roles), Some(role)) = (catalogue.roles, role_of(role)) else { return Withheld(Why::MissingData) };
            if role.default_role {
                // Everyone's: only its rights change, with both rights (VRCX, and the wiki's Manage Default Role).
                if change.touches_details() || change.touches_flags() {
                    return Withheld(Why::FixedRole);
                }
                if !(has(rights, DEFAULT_ROLE_MANAGE) && has(rights, ROLES_MANAGE)) {
                    return Withheld(Why::NoPermission);
                }
                return match &change.permissions {
                    Some(to) => grantable(&role.permissions, to, catalogue.permissions),
                    None => Allowed,
                };
            }
            if role.is_owner_role() {
                // Only its holder edits its words; its rights and flags are fixed.
                if !actor.role_ids.contains(&role.id) {
                    return Withheld(Why::RoleAtOrAbove);
                }
                if change.permissions.is_some() || change.touches_flags() {
                    return Withheld(Why::OwnerRole);
                }
                return Allowed;
            }
            if !has(rights, ROLES_MANAGE) {
                return Withheld(Why::NoPermission);
            }
            match below(actor, roles, role) {
                Allowed => {}
                no => return no,
            }
            match &change.permissions {
                Some(to) => grantable(&role.permissions, to, catalogue.permissions),
                None => Allowed,
            }
        }
        Action::DeleteRole { role } => {
            let (Some(roles), Some(role)) = (catalogue.roles, role_of(role)) else { return Withheld(Why::MissingData) };
            if role.default_role || role.is_owner_role() {
                return Withheld(Why::FixedRole);
            }
            if !has(rights, ROLES_MANAGE) {
                return Withheld(Why::NoPermission);
            }
            below(actor, roles, role)
        }
    }
}

/// The rights a role holds once `name` is turned on (what it depends on, too) or off (what depends on it, too).
pub fn toggled(permissions: &[String], name: &str, on: bool, catalogue: &[PermissionInfo]) -> Vec<String> {
    let mut out: Vec<String> = permissions.to_vec();
    if on {
        let mut stack = vec![name.to_string()];
        while let Some(p) = stack.pop() {
            if out.contains(&p) {
                continue;
            }
            if let Some(c) = catalogue.iter().find(|c| c.name == p) {
                stack.extend(c.depends_on.iter().cloned());
            }
            out.push(p);
        }
    } else {
        let mut gone = vec![name.to_string()];
        while let Some(p) = gone.pop() {
            out.retain(|x| *x != p);
            for c in catalogue {
                if c.depends_on.contains(&p) && out.contains(&c.name) {
                    gone.push(c.name.clone());
                }
            }
        }
    }
    out
}

// --- Text from VRChat ---

/// Bidirectional controls: removed from names so that a name cannot turn the text around it (M10).
fn bidi(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A one-line label from VRChat: no control or bidirectional characters, at most `max` characters.
pub fn label(s: &str, max: usize) -> String {
    let s: String = s.chars().map(|c| if c == '\n' || c == '\t' { ' ' } else { c }).filter(|c| !c.is_control() && !bidi(*c)).take(max).collect();
    s.trim().to_string()
}

/// Text of several lines from VRChat (a description, a post): lines kept, controls and bidirectional characters gone.
pub fn text(s: &str, max: usize) -> String {
    let s: String = s.chars().filter(|c| (*c == '\n' || !c.is_control()) && !bidi(*c)).take(max).collect();
    s.trim().to_string()
}

/// A link from VRChat, only when it is http or https and plain.
pub fn link(s: &str) -> Option<String> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    let ok = (lower.starts_with("https://") || lower.starts_with("http://"))
        && s.len() <= 300
        && !s.chars().any(|c| c.is_whitespace() || c.is_control() || bidi(c) || c == '"' || c == '<' || c == '>');
    ok.then(|| s.to_string())
}

fn id_with(s: &str, prefixes: &[&str]) -> bool {
    s.len() <= 64 && prefixes.iter().any(|p| s.strip_prefix(p).is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')))
}
pub fn is_group_id(s: &str) -> bool {
    id_with(s, &["grp_"])
}
pub fn is_role_id(s: &str) -> bool {
    id_with(s, &["grol_"])
}
/// A post's id: the spec calls it a notification id; VRChat's posts carry their own prefix.
pub fn is_post_id(s: &str) -> bool {
    id_with(s, &["gpos_", "not_"])
}
pub fn is_file_id(s: &str) -> bool {
    id_with(s, &["file_"])
}

/// The group's page on vrchat.com: where what this version does not do is done.
pub fn web_page(id: &str) -> Option<String> {
    is_group_id(id).then(|| format!("https://vrchat.com/home/group/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }
    fn role(id: &str, order: i64, perms: &[&str]) -> Role {
        Role { id: id.into(), name: id.into(), order: Some(order), permissions: s(perms), ..Default::default() }
    }
    fn roles() -> Vec<Role> {
        vec![
            role("grol_owner", 0, &["*"]),
            role("grol_admin", 1, &[ROLES_MANAGE, ROLES_ASSIGN, MEMBERS_MANAGE]),
            role("grol_mod", 2, &["group-instance-moderate"]),
            role("grol_artist", 3, &[]),
            Role { self_assignable: true, ..role("grol_pronouns", 4, &[]) },
            Role { requires_purchase: true, ..role("grol_supporter", 5, &[]) },
            Role { default_role: true, ..role("grol_everyone", 6, &["group-instance-join"]) },
        ]
    }
    fn catalogue_perms() -> Vec<PermissionInfo> {
        let p = |name: &str, add: bool, deps: &[&str]| PermissionInfo { name: name.into(), allowed_to_add: add, depends_on: s(deps), ..Default::default() };
        vec![
            p(ROLES_ASSIGN, true, &[MEMBERS_MANAGE]),
            p(MEMBERS_MANAGE, true, &[]),
            p(ROLES_MANAGE, false, &[]),
            p("group-instance-join", true, &[]),
            p("group-instance-moderate", true, &[]),
        ]
    }

    /// M2: the authority table — every row is (who, what, on which role) → verdict.
    #[test]
    fn the_authority_table() {
        let roles = roles();
        let perms = catalogue_perms();
        let cat = Catalogue { roles: Some(&roles), permissions: Some(&perms) };
        let owner_perms = s(&["*"]);
        let admin_perms = s(&[ROLES_MANAGE, ROLES_ASSIGN, MEMBERS_MANAGE]);
        let assign_only = s(&[ROLES_ASSIGN]);
        let mod_perms = s(&["group-instance-moderate"]);
        let (owner_roles, admin_roles, mod_roles) = (s(&["grol_owner"]), s(&["grol_admin"]), s(&["grol_mod"]));
        let owner = Actor { user_id: "usr_owner", permissions: &owner_perms, role_ids: &owner_roles, status: "member" };
        let admin = Actor { user_id: "usr_admin", permissions: &admin_perms, role_ids: &admin_roles, status: "member" };
        let assigner = Actor { user_id: "usr_admin", permissions: &assign_only, role_ids: &admin_roles, status: "member" };
        let moderator = Actor { user_id: "usr_mod", permissions: &mod_perms, role_ids: &mod_roles, status: "member" };
        let other = "usr_00000000-0000-4000-8000-000000000001";
        let assign = |role| Action::AssignRole { role, user: other, add: true };
        let rename = RoleChange { name: Some("New".into()), ..Default::default() };
        let rights = RoleChange { permissions: Some(s(&["group-instance-join", "group-instance-moderate"])), ..Default::default() };
        let wildcard = RoleChange { permissions: Some(s(&["*"])), ..Default::default() };
        let manage = RoleChange { permissions: Some(s(&[ROLES_MANAGE])), ..Default::default() };
        let lonely = RoleChange { permissions: Some(s(&[ROLES_ASSIGN])), ..Default::default() };
        use Verdict::*;
        let table: Vec<(&str, Verdict, Verdict)> = vec![
            ("owner assigns admin", can(assign("grol_admin"), &owner, &cat), Allowed),
            ("owner assigns own role", can(assign("grol_owner"), &owner, &cat), Withheld(Why::OwnerRole)),
            ("nobody assigns Everyone", can(assign("grol_everyone"), &owner, &cat), Withheld(Why::FixedRole)),
            ("nobody assigns a paid role", can(assign("grol_supporter"), &owner, &cat), Withheld(Why::PaidRole)),
            ("admin assigns below", can(assign("grol_mod"), &admin, &cat), Allowed),
            ("admin cannot assign own rank", can(assign("grol_admin"), &admin, &cat), Withheld(Why::RoleAtOrAbove)),
            ("assign without member data", can(assign("grol_mod"), &assigner, &cat), Withheld(Why::NoPermission)),
            ("moderator cannot assign", can(assign("grol_artist"), &moderator, &cat), Withheld(Why::NoPermission)),
            ("moderator takes a self role", can(Action::AssignRole { role: "grol_pronouns", user: "usr_mod", add: true }, &moderator, &cat), Allowed),
            ("moderator cannot self-take another", can(Action::AssignRole { role: "grol_artist", user: "usr_mod", add: true }, &moderator, &cat), Withheld(Why::NotSelfAssignable)),
            ("unknown role", can(assign("grol_gone"), &owner, &cat), Withheld(Why::MissingData)),
            ("admin renames below", can(Action::EditRole { role: "grol_mod", change: &rename }, &admin, &cat), Allowed),
            ("admin cannot rename own", can(Action::EditRole { role: "grol_admin", change: &rename }, &admin, &cat), Withheld(Why::RoleAtOrAbove)),
            ("admin grants allowed rights", can(Action::EditRole { role: "grol_artist", change: &rights }, &admin, &cat), Allowed),
            ("never the wildcard", can(Action::EditRole { role: "grol_artist", change: &wildcard }, &owner, &cat), Withheld(Why::NotGrantable)),
            ("not a right one may not add", can(Action::EditRole { role: "grol_artist", change: &manage }, &admin, &cat), Withheld(Why::NotGrantable)),
            ("a right without its prerequisite", can(Action::EditRole { role: "grol_artist", change: &lonely }, &admin, &cat), Withheld(Why::Prerequisite)),
            ("owner renames own role", can(Action::EditRole { role: "grol_owner", change: &rename }, &owner, &cat), Allowed),
            ("owner role rights fixed", can(Action::EditRole { role: "grol_owner", change: &rights }, &owner, &cat), Withheld(Why::OwnerRole)),
            ("admin cannot touch owner role", can(Action::EditRole { role: "grol_owner", change: &rename }, &admin, &cat), Withheld(Why::RoleAtOrAbove)),
            ("Everyone's name fixed", can(Action::EditRole { role: "grol_everyone", change: &rename }, &owner, &cat), Withheld(Why::FixedRole)),
            ("Everyone's rights need default-role", can(Action::EditRole { role: "grol_everyone", change: &rights }, &admin, &cat), Withheld(Why::NoPermission)),
            ("owner edits Everyone's rights", can(Action::EditRole { role: "grol_everyone", change: &rights }, &owner, &cat), Allowed),
            ("delete below", can(Action::DeleteRole { role: "grol_artist" }, &admin, &cat), Allowed),
            ("never delete Everyone", can(Action::DeleteRole { role: "grol_everyone" }, &owner, &cat), Withheld(Why::FixedRole)),
            ("never delete owner role", can(Action::DeleteRole { role: "grol_owner" }, &owner, &cat), Withheld(Why::FixedRole)),
            ("moderator cannot delete", can(Action::DeleteRole { role: "grol_artist" }, &moderator, &cat), Withheld(Why::NoPermission)),
            ("create with grantable rights", can(Action::CreateRole { draft: &RoleDraft { permissions: s(&["group-instance-join"]), ..Default::default() } }, &admin, &cat), Allowed),
            ("posts need announcements", can(Action::Post, &admin, &cat), Withheld(Why::NoPermission)),
            ("owner posts", can(Action::Post, &owner, &cat), Allowed),
            ("art needs group data", can(Action::ApplyArt, &moderator, &cat), Withheld(Why::NoPermission)),
        ];
        for (row, got, want) in table {
            assert_eq!(got, want, "{row}");
        }
    }

    /// Missing order or role data never turns into a right to edit everything.
    #[test]
    fn missing_role_data_withholds() {
        let admin_perms = s(&[ROLES_MANAGE, ROLES_ASSIGN, MEMBERS_MANAGE]);
        let admin_roles = s(&["grol_admin"]);
        let admin = Actor { user_id: "usr_admin", permissions: &admin_perms, role_ids: &admin_roles, status: "member" };
        let mut roles = roles();
        let rename = RoleChange { name: Some("x".into()), ..Default::default() };
        let none = Catalogue { roles: None, permissions: None };
        assert_eq!(can(Action::EditRole { role: "grol_mod", change: &rename }, &admin, &none), Verdict::Withheld(Why::MissingData));
        assert_eq!(can(Action::CreateRole { draft: &RoleDraft::default() }, &admin, &none), Verdict::Withheld(Why::MissingData));
        roles[1].order = None; // the actor's own order unknown
        let cat = Catalogue { roles: Some(&roles), permissions: None };
        assert_eq!(can(Action::EditRole { role: "grol_mod", change: &rename }, &admin, &cat), Verdict::Withheld(Why::MissingData));
        let roles = self::roles();
        let cat = Catalogue { roles: Some(&roles), permissions: None };
        let rights = RoleChange { permissions: Some(s(&["group-instance-join"])), ..Default::default() };
        assert_eq!(can(Action::EditRole { role: "grol_mod", change: &rights }, &admin, &cat), Verdict::Withheld(Why::MissingData), "no catalogue, no right changes");
        let nobody: Vec<String> = vec![];
        let roleless = Actor { role_ids: &nobody, ..admin };
        assert_eq!(can(Action::DeleteRole { role: "grol_artist" }, &roleless, &cat), Verdict::Withheld(Why::MissingData));
    }

    /// M1: listed are owners and managers, moderators with instance or ban rights included; viewers and plain members not.
    #[test]
    fn who_is_listed() {
        assert!(listed(&[], true));
        assert!(listed(&s(&["group-instance-moderate"]), false), "instance moderators are moderating rights");
        assert!(listed(&s(&["group-bans-manage"]), false));
        assert!(listed(&s(&["group-members-remove"]), false), "kick-only roles count");
        assert!(!listed(&s(&[MEMBERS_VIEWALL]), false), "view-all alone is a read grant");
        assert!(!listed(&s(&["group-instance-join", "group-instance-queue-priority", "group-instance-public-create"]), false));
        assert!(!listed(&s(&["group-some-future-right"]), false), "unknown strings grant nothing");
        let a = abilities(&s(&["group-instance-moderate"]));
        assert!(a.elsewhere && !a.roles && !a.posts && !a.art && !a.assign);
        let o = abilities(&s(&["*"]));
        assert!(o.roles && o.assign && o.posts && o.art && o.elsewhere);
    }

    #[test]
    fn edits_send_only_what_changed_and_consequential_ones_are_confirmed() {
        let base = Role { permissions: s(&["b", "a"]), name: "Artists".into(), ..role("grol_a", 3, &[]) };
        let base = Role { permissions: s(&["b", "a"]), ..base };
        let mut draft = RoleDraft::of(&base);
        draft.permissions = s(&["a", "b"]);
        assert!(RoleChange::between(&base, &draft).is_empty(), "the same rights in another order are no change");
        draft.name = " Painters ".into();
        let c = RoleChange::between(&base, &draft);
        assert_eq!(c.name.as_deref(), Some("Painters"));
        assert!(c.permissions.is_none() && !c.consequential());
        draft.requires_two_factor = true;
        assert!(RoleChange::between(&base, &draft).consequential());
    }

    #[test]
    fn toggling_a_right_brings_its_prerequisite_and_drops_its_dependents() {
        let cat = catalogue_perms();
        let on = toggled(&[], ROLES_ASSIGN, true, &cat);
        assert!(on.contains(&ROLES_ASSIGN.to_string()) && on.contains(&MEMBERS_MANAGE.to_string()));
        let off = toggled(&on, MEMBERS_MANAGE, false, &cat);
        assert!(off.is_empty(), "{off:?}");
    }

    #[test]
    fn text_from_vrchat_is_plain_and_bounded() {
        assert_eq!(label("A\u{202E}dlrow\u{2066}\n\u{7}x", 40), "Adlrow x");
        assert_eq!(label(&"é".repeat(100), 10).chars().count(), 10);
        assert_eq!(text("line one\nline\u{200F} two\u{0}", 100), "line one\nline two");
        assert_eq!(link("https://example.org/a"), Some("https://example.org/a".into()));
        assert_eq!(link("javascript:alert(1)"), None);
        assert_eq!(link("file:///C:/x"), None);
        assert_eq!(link("https://a b"), None);
        assert!(is_group_id("grp_0a1b2c3d-0000-4000-8000-000000000001"));
        assert!(!is_group_id("gmem_0a1b"), "a membership is not a group");
        assert!(!is_group_id("grp_../x"));
        assert!(is_post_id("gpos_1") && is_post_id("not_1") && !is_post_id("grp_1"));
        assert_eq!(web_page("grp_1").as_deref(), Some("https://vrchat.com/home/group/grp_1"));
        assert_eq!(web_page("grp_1/../x"), None);
    }
}
