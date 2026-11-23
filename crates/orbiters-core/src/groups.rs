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
    const HERE: [&str; 5] = [ANNOUNCEMENT_MANAGE, DATA_MANAGE, MEMBERS_MANAGE, ROLES_ASSIGN, ROLES_MANAGE];
    Abilities {
        roles: has(permissions, ROLES_MANAGE),
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
    /// `None` when not read yet: never shown as zero (Major 7).
    pub member_count: Option<u32>,
    pub online_count: Option<u32>,
    pub owner: bool,
    pub permissions: Vec<String>,
    /// Its name and pictures were read (a hidden membership's are read when its card comes into view, Major 7).
    #[serde(default)]
    pub named: bool,
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
    /// The membership itself (`gmem_…`) and when it began: leaving and joining again is another one.
    #[serde(default)]
    pub membership_id: String,
    #[serde(default)]
    pub joined_at: Option<String>,
    /// What was in VRChat's answer, whole: the rights (none dropped for its length or form), the roles given by hand
    /// (`roleIds`), the managed ones (`mRoleIds`). Each action asks for what it needs, and no more (verify3, deviation 9).
    #[serde(default)]
    pub rights_known: bool,
    #[serde(default)]
    pub roles_known: bool,
    #[serde(default)]
    pub managed_known: bool,
    /// The group the record says it is of, when it says.
    #[serde(default)]
    pub group_id: Option<String>,
}

impl MyMember {
    /// The membership this is, when the record says which.
    pub fn membership(&self) -> Option<Membership> {
        Membership::of(&self.membership_id, &self.joined_at)
    }
}

/// One membership: a person who leaves and joins again has another (verify3 Blocker 1). What is decided for one is not
/// done to the other.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub id: String,
    pub joined_at: Option<String>,
}

impl Membership {
    fn of(id: &str, joined_at: &Option<String>) -> Option<Self> {
        (!id.is_empty() && id.len() <= 64).then(|| Self { id: id.into(), joined_at: joined_at.clone() })
    }
    /// The same membership, as one comparable value (for the record: no id kept).
    pub fn digest(&self) -> String {
        digest(&["membership", &self.id, self.joined_at.as_deref().unwrap_or("-")])
    }
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
    /// VRChat's answer said which picture is there (a file id, or null for none): an absent or unreadable field is not
    /// "another picture" (verify2 B-B).
    #[serde(default)]
    pub icon_known: bool,
    #[serde(default)]
    pub banner_known: bool,
    /// The nameplate banner's file, read only to see that an apply left it as it was: this version never sends it (B4).
    #[serde(default)]
    pub nameplate_id: Option<String>,
    #[serde(default)]
    pub nameplate_known: bool,
    /// Its name, description, rules and links were all in the answer.
    #[serde(default)]
    pub words_known: bool,
    /// `None` when VRChat did not give it: never shown as zero (N1).
    pub member_count: Option<u32>,
    pub online_count: Option<u32>,
    pub owner_id: String,
    pub privacy: String,
    pub my_member: Option<MyMember>,
    /// `None` when VRChat did not give them: then no write that depends on role order is offered.
    pub roles: Option<Vec<Role>>,
}

/// The parts of a group that moved with a picture's change, beside the slot asked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupParts {
    pub banner: bool,
    pub icon: bool,
    pub nameplate: bool,
    pub name: bool,
    pub description: bool,
    pub rules: bool,
    pub links: bool,
}

impl GroupParts {
    pub fn any(&self) -> bool {
        *self != Self::default()
    }
}

impl Group {
    /// What is not the same in `now`, the same group in the answer to a change of `slot`: every part both say.
    pub fn moved_beside(&self, slot: ArtSlot, now: &Group) -> GroupParts {
        let picture = |known: (bool, bool), a: &Option<String>, b: &Option<String>| known.0 && known.1 && a != b;
        let words = self.words_known && now.words_known;
        GroupParts {
            banner: slot != ArtSlot::Banner && picture((self.banner_known, now.banner_known), &self.banner_id, &now.banner_id),
            icon: slot != ArtSlot::Icon && picture((self.icon_known, now.icon_known), &self.icon_id, &now.icon_id),
            nameplate: picture((self.nameplate_known, now.nameplate_known), &self.nameplate_id, &now.nameplate_id),
            name: words && self.name != now.name,
            description: words && self.description != now.description,
            rules: words && self.rules != now.rules,
            links: words && self.links != now.links,
        }
    }
    /// The file in a slot: `Some(None)` for none, `None` when VRChat's answer did not say.
    pub fn art(&self, slot: ArtSlot) -> Option<Option<&str>> {
        match slot {
            ArtSlot::Banner => self.banner_known.then_some(self.banner_id.as_deref()),
            ArtSlot::Icon => self.icon_known.then_some(self.icon_id.as_deref()),
        }
    }
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
    /// What the rules decide by was in VRChat's answer: its rights, its rank, and whether it is Everyone's or a bought one.
    /// A role read without them authorises nothing and counts as privileged (verify-r22 Blocker 4). A management flag left
    /// out reads as on (it is then confirmed, always), a "members can take it" left out as off: the strict side of each.
    #[serde(default)]
    pub known: bool,
    /// Its words and every flag were there too: only such a role is compared with what was asked (an edit, a check).
    #[serde(default)]
    pub whole: bool,
    /// When it was made, only when VRChat's text is a real RFC 3339 time.
    #[serde(default)]
    pub created_at: Option<String>,
}

impl Role {
    /// The owner's role: it holds the wildcard.
    pub fn is_owner_role(&self) -> bool {
        self.permissions.iter().any(|p| p == ALL)
    }
    /// A grant or removal of this role changes what someone may do (M3): it is confirmed.
    pub fn privileged(&self) -> bool {
        !self.known || self.management || !self.permissions.is_empty()
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
    /// VRChat's answer held the member's role arrays: only then are their roles known (absent ≠ empty, Blocker 4).
    #[serde(default)]
    pub roles_known: bool,
    /// The group the record says it is of, when it says.
    #[serde(default)]
    pub group_id: Option<String>,
}

impl Member {
    /// Whole enough to act on: a user id, its name, its roles.
    pub fn complete(&self) -> bool {
        crate::vrchat_accounts::is_user_id(&self.user_id) && self.display_name.is_some()
    }
    /// The record of exactly this person in exactly this group, with its role arrays whole: what a role write is decided
    /// and checked by. Anything else says nothing about them (verify2 B-B, B-D).
    pub fn is_of(&self, group: &str, user: &str) -> bool {
        self.roles_known && self.user_id == user && self.group_id.as_deref() == Some(group) && self.membership().is_some()
    }
    /// The membership this is, when the record says which.
    pub fn membership(&self) -> Option<Membership> {
        Membership::of(&self.membership_id, &self.joined_at)
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
    /// When it was made, only when VRChat's text is a real RFC 3339 time (Minor 1).
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// Its words were there (as words) and its audience was read whole (every role id kept, none over the limit): only
    /// then may it be deleted from here (Blocker 3, verify3 Major 5).
    #[serde(default)]
    pub complete: bool,
}

/// A time from VRChat, kept only when it parses as RFC 3339.
pub fn time(s: &str) -> Option<String> {
    let s = s.trim();
    (s.len() <= 40 && time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).is_ok()).then(|| s.to_string())
}

/// The post's audience and content as one comparable value: what a deletion was made against.
pub fn same_post(a: &Post, b: &Post) -> bool {
    let mut ra = a.role_ids.clone();
    let mut rb = b.role_ids.clone();
    ra.sort();
    rb.sort();
    a.id == b.id && a.title == b.title && a.text == b.text && a.visibility == b.visibility && ra == rb && a.image_id == b.image_id
}

/// Length-prefixed parts, so that no two different lists give the same bytes; SHA-256, in hexadecimal.
pub fn digest(parts: &[&str]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A set, in one order and once each: what goes over the wire, what is sealed and what is compared are the same bytes.
pub fn in_order(list: &[String]) -> Vec<String> {
    let mut l = list.to_vec();
    l.sort();
    l.dedup();
    l
}

fn joined(list: &[String]) -> String {
    in_order(list).join("\u{1f}")
}

/// Words as a person sees them, for telling "the same again" from something else: one Unicode form (NFKC), nothing
/// invisible, every run of whitespace one space. Never sent or shown.
pub fn canonical(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let invisible = |c: char| bidi(c) || matches!(c, '\u{00AD}' | '\u{034F}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{2064}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}') || (c.is_control() && !c.is_whitespace());
    let plain: String = text.nfkc().filter(|c| !invisible(*c)).collect();
    plain.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The parts of a post someone else changed, to say them (never what they hold).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostParts {
    pub title: bool,
    pub text: bool,
    pub visibility: bool,
    pub roles: bool,
    pub image: bool,
}

/// The parts of a role someone else changed, to say them. `standing` is its place among the roles and its fixed flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleParts {
    pub name: bool,
    pub description: bool,
    pub permissions: bool,
    pub self_assignable: bool,
    pub added_on_join: bool,
    pub two_factor: bool,
    pub standing: bool,
}

impl Post {
    fn parts(&self) -> [String; 5] {
        [self.title.trim().to_string(), self.text.trim().to_string(), self.visibility.wire().to_string(), joined(&self.role_ids), self.image_id.clone().unwrap_or_default()]
    }
    /// Its words, audience and picture as one comparable value (no text kept).
    pub fn content(&self) -> String {
        let p = self.parts();
        digest(&p.each_ref().map(String::as_str))
    }
    /// What is not the same in `now`, the same post read again.
    pub fn differs(&self, now: &Post) -> PostParts {
        let (a, b) = (self.parts(), now.parts());
        PostParts { title: a[0] != b[0], text: a[1] != b[1], visibility: a[2] != b[2], roles: a[3] != b[3], image: a[4] != b[4] }
    }
}

/// A part of a role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoleField {
    Name,
    Description,
    Permissions,
    SelfAssignable,
    AddedOnJoin,
    TwoFactor,
}

impl RoleField {
    pub const ALL: [RoleField; 6] = [RoleField::Name, RoleField::Description, RoleField::Permissions, RoleField::SelfAssignable, RoleField::AddedOnJoin, RoleField::TwoFactor];
}

impl Role {
    fn part(&self, f: RoleField) -> String {
        match f {
            RoleField::Name => self.name.trim().to_string(),
            RoleField::Description => self.description.trim().to_string(),
            RoleField::Permissions => joined(&self.permissions),
            RoleField::SelfAssignable => self.self_assignable.to_string(),
            RoleField::AddedOnJoin => self.added_on_join.to_string(),
            RoleField::TwoFactor => self.requires_two_factor.to_string(),
        }
    }
    /// What `fields` of this role hold, as one comparable value (no text kept).
    pub fn digest_of(&self, fields: &[RoleField]) -> String {
        let parts: Vec<String> = RoleField::ALL.iter().filter(|f| fields.contains(f)).flat_map(|f| [format!("{f:?}"), self.part(*f)]).collect();
        digest(&parts.iter().map(String::as_str).collect::<Vec<_>>())
    }
    /// The role as it would be with `change` made on it.
    pub fn with(&self, change: &RoleChange) -> Role {
        let mut r = self.clone();
        if let Some(v) = &change.name {
            r.name = v.clone();
        }
        if let Some(v) = &change.description {
            r.description = v.clone();
        }
        r.self_assignable = change.self_assignable.unwrap_or(r.self_assignable);
        r.added_on_join = change.added_on_join.unwrap_or(r.added_on_join);
        r.requires_two_factor = change.requires_two_factor.unwrap_or(r.requires_two_factor);
        r
    }
}

impl Role {
    /// What is not the same in `now`, the same role read again.
    pub fn differs(&self, now: &Role) -> RoleParts {
        RoleParts {
            name: self.name != now.name,
            description: self.description != now.description,
            permissions: joined(&self.permissions) != joined(&now.permissions) || self.known != now.known,
            self_assignable: self.self_assignable != now.self_assignable,
            added_on_join: self.added_on_join != now.added_on_join,
            two_factor: self.requires_two_factor != now.requires_two_factor,
            standing: self.whole != now.whole || self.order != now.order || self.default_role != now.default_role || self.management != now.management || self.requires_purchase != now.requires_purchase,
        }
    }
}

impl RoleParts {
    pub fn any(&self) -> bool {
        *self != Self::default()
    }
}

impl RoleDraft {
    /// What makes two new roles the same one again, whatever the spaces and the order of the rights.
    pub fn canonical(&self) -> String {
        let flag = |b: bool| if b { "1" } else { "0" }; // i18n: not shown (a record's words)
        digest(&[&canonical(&self.name), &canonical(&self.description), &joined(&self.permissions), flag(self.self_assignable), flag(self.added_on_join), flag(self.requires_two_factor)])
    }
    /// The new role's words, rights and flags as a role, to compare one VRChat made with it.
    pub fn as_role(&self) -> Role {
        Role {
            name: self.name.trim().into(),
            description: self.description.trim().into(),
            permissions: self.permissions.clone(),
            self_assignable: self.self_assignable,
            added_on_join: self.added_on_join,
            requires_two_factor: self.requires_two_factor,
            ..Default::default()
        }
    }
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

/// How the nameplate banner is applied: the seam DESIGN22's revision asks for (B4). This version has only
/// [`NameplateApply::Unavailable`]: it exports both modes and links to the group's page on vrchat.com. Nothing builds a
/// `nameplateId`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NameplateApply {
    #[default]
    Unavailable,
}

impl NameplateApply {
    /// What this version can do with a nameplate banner.
    pub const fn current() -> Self {
        NameplateApply::Unavailable
    }
    pub const fn can_apply(self) -> bool {
        match self {
            NameplateApply::Unavailable => false,
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

/// A role's words and flags as the person edits them. Not its rights: VRChat takes the whole list of them and no condition,
/// so a right another manager removed meanwhile would be put back. They are chosen when a role is created, and changed on
/// vrchat.com.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleEdit {
    pub name: String,
    pub description: String,
    pub self_assignable: bool,
    pub added_on_join: bool,
    pub requires_two_factor: bool,
}

impl RoleEdit {
    pub fn of(role: &Role) -> Self {
        Self { name: role.name.clone(), description: role.description.clone(), self_assignable: role.self_assignable, added_on_join: role.added_on_join, requires_two_factor: role.requires_two_factor }
    }
}

/// The fields of a role edit that differ from the baseline it was made against: only those are sent (M2, M4), each a value of
/// its own that the person chose; nothing that was read is sent back.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleChange {
    pub name: Option<String>,
    pub description: Option<String>,
    pub self_assignable: Option<bool>,
    pub added_on_join: Option<bool>,
    pub requires_two_factor: Option<bool>,
}

impl RoleChange {
    pub fn between(baseline: &Role, edited: &RoleEdit) -> Self {
        let changed = |a: bool, b: bool| (a != b).then_some(b);
        Self {
            // Untouched, whatever its spaces, it is no change: what was read is not sent back tidied.
            name: (edited.name != baseline.name && edited.name.trim() != baseline.name).then(|| edited.name.trim().to_string()),
            description: (edited.description != baseline.description).then(|| edited.description.clone()),
            self_assignable: changed(baseline.self_assignable, edited.self_assignable),
            added_on_join: changed(baseline.added_on_join, edited.added_on_join),
            requires_two_factor: changed(baseline.requires_two_factor, edited.requires_two_factor),
        }
    }
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
    /// What makes two changes the same one again.
    pub fn canonical(&self) -> String {
        let flag = |b: Option<bool>| match b {
            None => "-", // i18n: not shown (a record's words)
            Some(true) => "1", // i18n: not shown (a record's words)
            Some(false) => "0", // i18n: not shown (a record's words)
        };
        let words = |t: &Option<String>| t.as_deref().map_or_else(|| "-".to_string(), |t| format!("+{}", canonical(t))); // i18n: not shown (a record's words)
        digest(&[&words(&self.name), &words(&self.description), flag(self.self_assignable), flag(self.added_on_join), flag(self.requires_two_factor)])
    }
    /// The parts it changes.
    pub fn fields(&self) -> Vec<RoleField> {
        let parts = [RoleField::Name, RoleField::Description, RoleField::SelfAssignable, RoleField::AddedOnJoin, RoleField::TwoFactor];
        let on = [self.name.is_some(), self.description.is_some(), self.self_assignable.is_some(), self.added_on_join.is_some(), self.requires_two_factor.is_some()];
        parts.into_iter().zip(on).filter(|(_, o)| *o).map(|(f, _)| f).collect()
    }
    /// Nothing but its words: all the owner's role lets change (J6). The interface offers exactly what this allows.
    pub fn only_words(&self) -> bool {
        self.self_assignable.is_none() && self.added_on_join.is_none() && self.requires_two_factor.is_none()
    }
    /// It changes what members may do, now or when they join (M2): confirmed before it is sent.
    pub fn consequential(&self) -> bool {
        !self.only_words()
    }
}

/// A new post as the person wrote it. The notification is off unless they turn it on (M7). A post is created and deleted
/// here, never edited: VRChat's edit carries the audience whole and takes no condition, so it can always undo another
/// moderator's restriction (verify3 Blocker 4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostDraft {
    pub title: String,
    pub text: String,
    pub visibility: Visibility,
    pub role_ids: Vec<String>,
    pub send_notification: bool,
}

impl PostDraft {
    /// The roles a post is restricted to: none for a public one, whatever was chosen before (N3).
    pub fn audience(&self) -> Vec<String> {
        if self.visibility == Visibility::Group { in_order(&self.role_ids) } else { Vec::new() }
    }
    /// A new post's words and audience as one comparable value, exactly as they go (no text kept).
    pub fn content(&self) -> String {
        Post { title: self.title.trim().into(), text: self.text.trim().into(), visibility: self.visibility, role_ids: self.audience(), ..Default::default() }.content()
    }
    /// What makes two new posts the same one again: their words as a person sees them and who they are for, whatever the
    /// spaces, the Unicode form, the order of the roles, or the notification (the same words, notified or not, are the
    /// same post a second time).
    pub fn canonical(&self) -> String {
        digest(&[&canonical(&self.title), &canonical(&self.text), self.visibility.wire(), &joined(&self.audience())])
    }
}

pub const POST_TITLE_MAX: usize = 64;
pub const POST_TEXT_MAX: usize = 2000;
pub const ROLE_NAME_MAX: usize = 64;
pub const ROLE_DESCRIPTION_MAX: usize = 512;

/// Why an action is refused by the rules. VRChat may still refuse what is offered.
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
    /// The owner's own role: only its words change.
    OwnerRole,
    /// A right this account may not give (or the wildcard).
    NotGrantable,
    /// A right without the one it depends on.
    Prerequisite,
    /// The role is not one members may take themselves.
    NotSelfAssignable,
    /// A right (or a flag on a role holding rights) the account does not hold itself: no one gives more than they have
    /// (Major 11).
    Escalation,
    /// The account cannot upload to its VRChat inventory (VRChat+ is needed for gallery and icon uploads, Major 4).
    NotEligible,
    /// The picture is not one of the account's own inventory of that kind (Major 4).
    NotYourPicture,
}

/// What VRChat's answer left out, so that nothing can be decided: never guessed, never sent (verify2 B-D).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Missing {
    /// The account's own membership, or a part of it.
    Membership,
    /// The membership VRChat gave is not the signed-in account's, or not this group's.
    Identity,
    /// The group's roles.
    Roles,
    /// Where a role stands among the others.
    RoleOrder,
    /// A role's rights or flags.
    RoleDefinition,
    /// The rights a role can be given.
    Catalogue,
    /// The other member's own record, whole.
    Member,
    /// The post, or who it is for.
    Post,
    /// Which picture the group has, or whether the picture is one of the account's.
    Picture,
    /// Whether the account may upload (VRChat+).
    Entitlement,
}

/// Whether the signed-in account may do something, decided on what VRChat's answers held. `Unknown` cannot become a
/// dispatch: the dispatch asks for the [`Permit`], which only this module makes, and only for `Allowed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Authority {
    Allowed(Permit),
    Refused(Why),
    Unknown(Missing),
}

/// The proof that the rules allowed one thing, to one account, in one group. Its fields are private and it is neither
/// made from outside nor read from a file or a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permit {
    needs_confirmation: bool,
    actor: String,
    group: String,
    subject: String,
}

impl Permit {
    /// From the role's own rights and flags, all of them known; never "no confirmation" for want of them.
    pub fn needs_confirmation(&self) -> bool {
        self.needs_confirmation
    }
    /// Whether it is the permit of exactly this, asked by exactly this account in exactly this group.
    pub fn covers(&self, actor: &str, group: &str, subject: &str) -> bool {
        self.actor == actor && self.group == group && self.subject == subject
    }
}

impl Authority {
    pub fn allowed(&self) -> bool {
        matches!(self, Authority::Allowed(_))
    }
    pub fn refused(&self) -> Option<Why> {
        match self {
            Authority::Refused(w) => Some(*w),
            _ => None,
        }
    }
    pub fn missing(&self) -> Option<Missing> {
        match self {
            Authority::Unknown(m) => Some(*m),
            _ => None,
        }
    }
}

/// The signed-in account in the group, freshly read and whole.
#[derive(Clone, Copy, Debug)]
pub struct Actor<'a> {
    pub group: &'a str,
    pub user_id: &'a str,
    pub permissions: &'a [String],
    /// The roles their rank is read from, when VRChat gave those given by hand. Managed ones left out of the answer can
    /// only make the rank read here lower than it is: what that allows is a part of what the account may do, never more.
    pub role_ids: Option<&'a [String]>,
    /// Only for taking a role oneself.
    pub status: Option<&'a str>,
}

impl<'a> Actor<'a> {
    /// The account's membership in `group`, only with its rights whole, of this group, and the signed-in account's own.
    /// What one action alone needs (its roles, its status) is asked where it is used.
    pub fn of(group: &'a Group, signed_in: &str) -> Result<Self, Missing> {
        let me = group.my_member.as_ref().ok_or(Missing::Membership)?;
        if !me.rights_known {
            return Err(Missing::Membership);
        }
        if me.user_id != signed_in || me.group_id.as_deref().is_some_and(|g| g != group.id) {
            return Err(Missing::Identity);
        }
        Ok(Self { group: &group.id, user_id: &me.user_id, permissions: &me.permissions, role_ids: me.roles_known.then_some(me.role_ids.as_slice()), status: Some(me.status.as_str()).filter(|s| !s.is_empty()) })
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
    /// Write or delete a post.
    Post,
    /// Set the group's banner or icon to a file.
    ApplyArt,
}

impl Action<'_> {
    /// What a permit for it is for, whole: another role, person, direction or payload is another subject.
    pub fn subject(&self) -> String {
        match self {
            Action::AssignRole { role, user, add } => digest(&["assign", role, user, if *add { "add" } else { "remove" }]),
            Action::CreateRole { draft } => digest(&["create-role", &draft.as_role().digest_of(&RoleField::ALL)]),
            Action::EditRole { role, change } => digest(&["edit-role", role, &format!("{:?}", change.fields()), &Role::default().with(change).digest_of(&change.fields())]),
            Action::DeleteRole { role } => digest(&["delete-role", role]),
            Action::Post => "post".into(),
            Action::ApplyArt => "art".into(),
        }
    }
}

/// Not allowed, and which way.
enum No {
    Refused(Why),
    Unknown(Missing),
}
impl From<Why> for No {
    fn from(w: Why) -> Self {
        No::Refused(w)
    }
}
impl From<Missing> for No {
    fn from(m: Missing) -> Self {
        No::Unknown(m)
    }
}

/// The actor's highest role's order (the smallest number): not known when a role of theirs or its order is not.
fn top_order(actor: &Actor, roles: &[Role]) -> Result<i64, Missing> {
    let mine = actor.role_ids.filter(|r| !r.is_empty()).ok_or(Missing::Membership)?;
    let mut top = i64::MAX;
    for id in mine {
        top = top.min(roles.iter().find(|r| r.id == *id).ok_or(Missing::Roles)?.order.ok_or(Missing::RoleOrder)?);
    }
    Ok(top)
}

/// Whether the actor may change a role at all, by rank: strictly below their own highest role.
fn below(actor: &Actor, roles: &[Role], role: &Role) -> Result<(), No> {
    let (top, order) = (top_order(actor, roles)?, role.order.ok_or(Missing::RoleOrder)?);
    if order > top { Ok(()) } else { Err(Why::RoleAtOrAbove.into()) }
}

/// Every right a new role is given may be given by this account (VRChat's `allowedToAdd`), is one the account holds itself
/// (no one gives more than they have, Major 11), and has what it depends on.
fn grantable(actor: &[String], rights: &[String], catalogue: Option<&[PermissionInfo]>) -> Result<(), No> {
    if rights.iter().any(|p| p == ALL) {
        return Err(Why::NotGrantable.into());
    }
    if rights.iter().any(|p| !has(actor, p)) {
        return Err(Why::Escalation.into());
    }
    if rights.is_empty() {
        return Ok(());
    }
    let catalogue = catalogue.ok_or(Missing::Catalogue)?;
    for p in rights {
        match catalogue.iter().find(|c| c.name == *p) {
            Some(c) if c.allowed_to_add => {
                if c.depends_on.iter().any(|d| !rights.contains(d)) {
                    return Err(Why::Prerequisite.into());
                }
            }
            _ => return Err(Why::NotGrantable.into()),
        }
    }
    Ok(())
}

/// What the interface offers and what a dispatch needs (M2): from the actor's fresh state, the target, the proposed
/// change and the catalogue. What is missing is `Unknown`, never a refusal dressed as an answer, never allowed. This
/// mirrors VRCX's rules; it is no proof of what VRChat's server accepts.
pub fn can(action: Action, actor: Result<Actor, Missing>, catalogue: &Catalogue) -> Authority {
    let actor = match actor {
        Ok(a) => a,
        Err(m) => return Authority::Unknown(m),
    };
    match decide(&action, &actor, catalogue) {
        Ok(needs_confirmation) => Authority::Allowed(Permit { needs_confirmation, actor: actor.user_id.into(), group: actor.group.into(), subject: action.subject() }),
        Err(No::Refused(w)) => Authority::Refused(w),
        Err(No::Unknown(m)) => Authority::Unknown(m),
    }
}

/// Whether the signed-in account may upload to its own inventory: by its own tags, all of them read.
pub fn can_upload(signed_in: &str, group: &str, tags: Option<&[String]>) -> Authority {
    match tags {
        None => Authority::Unknown(Missing::Entitlement),
        Some(t) if t.iter().any(|x| x == "system_supporter") => Authority::Allowed(Permit { needs_confirmation: true, actor: signed_in.into(), group: group.into(), subject: UPLOAD.into() }),
        Some(_) => Authority::Refused(Why::NotEligible),
    }
}

/// What an upload's permit is for.
pub const UPLOAD: &str = "upload";

/// Allowed (and whether the person confirms it), or not.
fn decide(action: &Action, actor: &Actor, catalogue: &Catalogue) -> Result<bool, No> {
    let rights = actor.permissions;
    let role_of = |id: &str| -> Result<(&[Role], &Role), No> {
        let roles = catalogue.roles.ok_or(Missing::Roles)?;
        let role = roles.iter().find(|r| r.id == id).ok_or(Missing::RoleDefinition)?;
        // A role read without its rights, rank or flags: nothing says it is harmless (Blocker 4).
        if !role.known {
            return Err(Missing::RoleDefinition.into());
        }
        Ok((roles, role))
    };
    let right = |r: &str| if has(rights, r) { Ok(()) } else { Err(No::Refused(Why::NoPermission)) };
    match action {
        Action::Post => right(ANNOUNCEMENT_MANAGE).map(|_| true),
        Action::ApplyArt => right(DATA_MANAGE).map(|_| true),
        Action::AssignRole { role, user, .. } => {
            let (roles, role) = role_of(role)?;
            if role.default_role {
                return Err(Why::FixedRole.into());
            }
            if role.is_owner_role() {
                return Err(Why::OwnerRole.into());
            }
            if role.requires_purchase {
                return Err(Why::PaidRole.into());
            }
            if *user == actor.user_id {
                // Taking or leaving a role oneself: one members may take, or one below one's own with the right to assign.
                if actor.status.ok_or(Missing::Membership)? != "member" {
                    return Err(Why::NoPermission.into());
                }
                if !role.self_assignable {
                    if !has(rights, ROLES_ASSIGN) {
                        return Err(Why::NotSelfAssignable.into());
                    }
                    below(actor, roles, role)?;
                }
                return Ok(role.privileged());
            }
            right(ROLES_ASSIGN)?;
            right(MEMBERS_MANAGE)?;
            below(actor, roles, role)?;
            Ok(role.privileged())
        }
        Action::CreateRole { draft } => {
            right(ROLES_MANAGE)?;
            // The account's own rank must be known: a new role lands below it.
            top_order(actor, catalogue.roles.ok_or(Missing::Roles)?)?;
            grantable(rights, &draft.permissions, catalogue.permissions)?;
            Ok(!draft.permissions.is_empty() || draft.self_assignable || draft.added_on_join || draft.requires_two_factor)
        }
        Action::EditRole { role, change } => {
            let (roles, role) = role_of(role)?;
            let confirm = change.consequential() || role.privileged();
            if role.default_role {
                // Everyone's: only its rights change, and those on vrchat.com.
                return Err(Why::FixedRole.into());
            }
            if role.is_owner_role() {
                // Only its holder, and only its words (J6).
                if !actor.role_ids.ok_or(Missing::Membership)?.contains(&role.id) {
                    return Err(Why::RoleAtOrAbove.into());
                }
                if !change.only_words() {
                    return Err(Why::OwnerRole.into());
                }
                return Ok(confirm);
            }
            right(ROLES_MANAGE)?;
            below(actor, roles, role)?;
            // Opening a role that holds rights to more people (members take it, everyone joining gets it) confers those
            // rights: only with every one of them held by the account itself (Major 11).
            let widening = change.self_assignable == Some(true) || change.added_on_join == Some(true);
            if widening && role.permissions.iter().any(|p| !has(rights, p)) {
                return Err(Why::Escalation.into());
            }
            Ok(confirm)
        }
        Action::DeleteRole { role } => {
            let (roles, role) = role_of(role)?;
            if role.default_role || role.is_owner_role() {
                return Err(Why::FixedRole.into());
            }
            right(ROLES_MANAGE)?;
            below(actor, roles, role)?;
            Ok(true)
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
        Role { id: id.into(), name: id.into(), order: Some(order), permissions: s(perms), known: true, ..Default::default() }
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

    /// An authority without its permit, to compare.
    #[derive(Debug, PartialEq)]
    enum Got {
        Allowed,
        Withheld(Why),
        Unknown(Missing),
    }
    fn can(action: Action, actor: &Actor, catalogue: &Catalogue) -> Got {
        match super::can(action, Ok(*actor), catalogue) {
            Authority::Allowed(_) => Got::Allowed,
            Authority::Refused(w) => Got::Withheld(w),
            Authority::Unknown(m) => Got::Unknown(m),
        }
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
        let owner = Actor { group: "grp_1", user_id: "usr_owner", permissions: &owner_perms, role_ids: Some(&owner_roles), status: Some("member") };
        let admin = Actor { group: "grp_1", user_id: "usr_admin", permissions: &admin_perms, role_ids: Some(&admin_roles), status: Some("member") };
        let assigner = Actor { group: "grp_1", user_id: "usr_admin", permissions: &assign_only, role_ids: Some(&admin_roles), status: Some("member") };
        let moderator = Actor { group: "grp_1", user_id: "usr_mod", permissions: &mod_perms, role_ids: Some(&mod_roles), status: Some("member") };
        let rich_perms = s(&[ROLES_MANAGE, ROLES_ASSIGN, MEMBERS_MANAGE, "group-instance-join", "group-instance-moderate"]);
        let rich_admin = Actor { permissions: &rich_perms, ..admin };
        let open_up = RoleChange { self_assignable: Some(true), ..Default::default() };
        let other = "usr_00000000-0000-4000-8000-000000000001";
        let assign = |role| Action::AssignRole { role, user: other, add: true };
        let rename = RoleChange { name: Some("New".into()), ..Default::default() };
        let made = |rights: &[&str]| RoleDraft { permissions: s(rights), ..Default::default() };
        use Got::*;
        let table: Vec<(&str, Got, Got)> = vec![
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
            ("unknown role", can(assign("grol_gone"), &owner, &cat), Unknown(Missing::RoleDefinition)),
            ("admin renames below", can(Action::EditRole { role: "grol_mod", change: &rename }, &admin, &cat), Allowed),
            ("admin cannot rename own", can(Action::EditRole { role: "grol_admin", change: &rename }, &admin, &cat), Withheld(Why::RoleAtOrAbove)),
            ("no widening a privileged role without its rights", can(Action::EditRole { role: "grol_mod", change: &open_up }, &admin, &cat), Withheld(Why::Escalation)),
            ("the owner widens it", can(Action::EditRole { role: "grol_mod", change: &open_up }, &owner, &cat), Allowed),
            ("owner renames own role", can(Action::EditRole { role: "grol_owner", change: &rename }, &owner, &cat), Allowed),
            ("owner role two-factor fixed (J6)", can(Action::EditRole { role: "grol_owner", change: &RoleChange { requires_two_factor: Some(true), ..Default::default() } }, &owner, &cat), Withheld(Why::OwnerRole)),
            ("admin cannot touch owner role", can(Action::EditRole { role: "grol_owner", change: &rename }, &admin, &cat), Withheld(Why::RoleAtOrAbove)),
            ("Everyone's flags fixed", can(Action::EditRole { role: "grol_everyone", change: &open_up }, &owner, &cat), Withheld(Why::FixedRole)),
            ("Everyone's name fixed", can(Action::EditRole { role: "grol_everyone", change: &rename }, &owner, &cat), Withheld(Why::FixedRole)),
            ("delete below", can(Action::DeleteRole { role: "grol_artist" }, &admin, &cat), Allowed),
            ("never delete Everyone", can(Action::DeleteRole { role: "grol_everyone" }, &owner, &cat), Withheld(Why::FixedRole)),
            ("never delete owner role", can(Action::DeleteRole { role: "grol_owner" }, &owner, &cat), Withheld(Why::FixedRole)),
            ("moderator cannot delete", can(Action::DeleteRole { role: "grol_artist" }, &moderator, &cat), Withheld(Why::NoPermission)),
            ("create with rights it lacks", can(Action::CreateRole { draft: &RoleDraft { permissions: s(&["group-instance-join"]), ..Default::default() } }, &admin, &cat), Withheld(Why::Escalation)),
            ("create with rights it holds", can(Action::CreateRole { draft: &RoleDraft { permissions: s(&["group-instance-join"]), ..Default::default() } }, &rich_admin, &cat), Allowed),
            ("never created with the wildcard", can(Action::CreateRole { draft: &made(&["*"]) }, &owner, &cat), Withheld(Why::NotGrantable)),
            ("not created with a right one may not add", can(Action::CreateRole { draft: &made(&[ROLES_MANAGE]) }, &admin, &cat), Withheld(Why::NotGrantable)),
            ("not created with a right without its prerequisite", can(Action::CreateRole { draft: &made(&[ROLES_ASSIGN]) }, &admin, &cat), Withheld(Why::Prerequisite)),
            ("posts need announcements", can(Action::Post, &admin, &cat), Withheld(Why::NoPermission)),
            ("owner posts", can(Action::Post, &owner, &cat), Allowed),
            ("art needs group data", can(Action::ApplyArt, &moderator, &cat), Withheld(Why::NoPermission)),
        ];
        for (row, got, want) in table {
            assert_eq!(got, want, "{row}");
        }
    }

    /// Blocker 4: a role read without its rights or flags is no cosmetic role: it is never given from here, and counts
    /// as privileged (so anything about it is confirmed).
    #[test]
    fn a_role_read_without_its_rights_authorises_nothing() {
        let owner_perms = s(&["*"]);
        let owner_roles = s(&["grol_owner"]);
        let owner = Actor { group: "grp_1", user_id: "usr_owner", permissions: &owner_perms, role_ids: Some(&owner_roles), status: Some("member") };
        let mut roles = roles();
        roles[3].known = false; // "grol_artist" arrived without its permission list
        let cat = Catalogue { roles: Some(&roles), permissions: None };
        assert!(roles[3].privileged(), "unknown means privileged");
        assert_eq!(can(Action::AssignRole { role: "grol_artist", user: "usr_x", add: true }, &owner, &cat), Got::Unknown(Missing::RoleDefinition));
        assert_eq!(can(Action::DeleteRole { role: "grol_artist" }, &owner, &cat), Got::Unknown(Missing::RoleDefinition));
    }

    /// verify2 B-D, verify3 deviation 9: the actor is the signed-in account's own membership of this group with its rights
    /// whole, or nothing at all; what else is missing withholds what needs it, and only that.
    #[test]
    fn an_actor_is_the_signed_in_accounts_own_with_its_rights_whole() {
        let me = MyMember { user_id: "usr_me".into(), role_ids: s(&["grol_admin"]), permissions: s(&["*"]), status: "member".into(), rights_known: true, roles_known: true, managed_known: true, group_id: Some("grp_1".into()), ..Default::default() };
        let group = |m: Option<MyMember>| Group { id: "grp_1".into(), my_member: m, ..Default::default() };
        let of = |m: Option<MyMember>, who: &str| Actor::of(&group(m), who).map(|a| a.user_id.to_string());
        assert_eq!(of(Some(me.clone()), "usr_me"), Ok("usr_me".to_string()));
        assert_eq!(of(None, "usr_me"), Err(Missing::Membership));
        assert_eq!(of(Some(MyMember { rights_known: false, ..me.clone() }), "usr_me"), Err(Missing::Membership));
        assert_eq!(of(Some(me.clone()), "usr_other"), Err(Missing::Identity));
        assert_eq!(of(Some(MyMember { group_id: Some("grp_2".into()), ..me.clone() }), "usr_me"), Err(Missing::Identity));
        let g = group(Some(MyMember { rights_known: false, ..me.clone() }));
        assert_eq!(super::can(Action::Post, Actor::of(&g, "usr_me"), &Catalogue::default()), Authority::Unknown(Missing::Membership), "unknown, not refused and not allowed");
        // A delegated manager whose managed roles or status VRChat leaves out still writes posts and changes pictures; with
        // no role at all read, what needs a rank is withheld; the rank read from a part is never above the real one.
        let roles = roles();
        let cat = Catalogue { roles: Some(&roles), permissions: None };
        let part = group(Some(MyMember { managed_known: false, status: String::new(), ..me.clone() }));
        let none = group(Some(MyMember { roles_known: false, role_ids: vec![], ..me.clone() }));
        for g in [&part, &none] {
            assert!(super::can(Action::Post, Actor::of(g, "usr_me"), &cat).allowed() && super::can(Action::ApplyArt, Actor::of(g, "usr_me"), &cat).allowed());
        }
        let delete = Action::DeleteRole { role: "grol_mod" };
        assert!(super::can(delete, Actor::of(&part, "usr_me"), &cat).allowed(), "below the role that was read");
        assert_eq!(super::can(delete, Actor::of(&none, "usr_me"), &cat), Authority::Unknown(Missing::Membership));
        let lower = group(Some(MyMember { role_ids: s(&["grol_artist"]), managed_known: false, ..me }));
        assert_eq!(super::can(delete, Actor::of(&lower, "usr_me"), &cat), Authority::Refused(Why::RoleAtOrAbove), "a rank read low refuses more, never less");
        let mine = Action::AssignRole { role: "grol_pronouns", user: "usr_me", add: true };
        assert_eq!(super::can(mine, Actor::of(&part, "usr_me"), &cat), Authority::Unknown(Missing::Membership), "taking a role oneself needs the status");
    }

    /// verify3 Blocker 1: two memberships of one person are two.
    #[test]
    fn leaving_and_joining_again_is_another_membership() {
        let m = |id: &str, at: Option<&str>| Member { user_id: "usr_a".into(), membership_id: id.into(), joined_at: at.map(str::to_string), roles_known: true, group_id: Some("grp_1".into()), ..Default::default() };
        let one = m("gmem_1", Some("2026-01-01T00:00:00Z"));
        assert!(one.is_of("grp_1", "usr_a") && !m("", None).is_of("grp_1", "usr_a"), "a record that does not say which membership it is says nothing");
        let digest = |m: &Member| m.membership().unwrap().digest();
        assert_eq!(digest(&one), digest(&one.clone()));
        assert_ne!(digest(&one), digest(&m("gmem_2", Some("2026-01-01T00:00:00Z"))));
        assert_ne!(digest(&one), digest(&m("gmem_1", Some("2026-02-01T00:00:00Z"))), "the same id, joined at another time");
        assert_ne!(digest(&one), digest(&m("gmem_1", None)));
    }

    /// A permit is for one action, one account, one group; confirmation follows the role's own rights.
    #[test]
    fn a_permit_covers_only_what_was_decided() {
        let (rights, ids, roles) = (s(&["*"]), s(&["grol_owner"]), roles());
        let actor = Actor { group: "grp_1", user_id: "usr_owner", permissions: &rights, role_ids: Some(&ids), status: Some("member") };
        let cat = Catalogue { roles: Some(&roles), permissions: None };
        let give = |role: &'static str| Action::AssignRole { role, user: "usr_x", add: true };
        let Authority::Allowed(p) = super::can(give("grol_artist"), Ok(actor), &cat) else { panic!() };
        assert!(!p.needs_confirmation(), "a role with no rights");
        assert!(p.covers("usr_owner", "grp_1", &give("grol_artist").subject()));
        for (who, group, subject) in [
            ("usr_other", "grp_1", give("grol_artist").subject()),
            ("usr_owner", "grp_2", give("grol_artist").subject()),
            ("usr_owner", "grp_1", give("grol_mod").subject()),
            ("usr_owner", "grp_1", Action::AssignRole { role: "grol_artist", user: "usr_y", add: true }.subject()),
            ("usr_owner", "grp_1", Action::AssignRole { role: "grol_artist", user: "usr_x", add: false }.subject()),
            ("usr_owner", "grp_1", Action::Post.subject()),
        ] {
            assert!(!p.covers(who, group, &subject));
        }
        let Authority::Allowed(p) = super::can(give("grol_mod"), Ok(actor), &cat) else { panic!() };
        assert!(p.needs_confirmation());
        assert_eq!(super::can_upload("usr_me", "grp_1", None), Authority::Unknown(Missing::Entitlement));
        assert_eq!(super::can_upload("usr_me", "grp_1", Some(&[])), Authority::Refused(Why::NotEligible));
        assert!(super::can_upload("usr_me", "grp_1", Some(&s(&["system_supporter"]))).allowed());
    }

    /// One value per content: no two different ones look alike, whatever the order they were chosen in.
    #[test]
    fn digests_tell_contents_apart() {
        let now = Post { title: "a".into(), text: "b".into(), role_ids: s(&["grol_a", "grol_b"]), ..Default::default() };
        let same = PostDraft { title: " a ".into(), text: "b".into(), role_ids: s(&["grol_b", "grol_a", "grol_b"]), send_notification: true, ..Default::default() };
        assert_eq!(now.content(), same.content());
        assert_ne!(Post { image_id: Some("file_x".into()), ..now.clone() }.content(), same.content());
        assert_ne!(now.content(), PostDraft { visibility: Visibility::Public, ..same.clone() }.content(), "a public post has no restriction");
        assert_ne!(Post { title: "ab".into(), ..Default::default() }.content(), Post { title: "a".into(), text: "b".into(), ..Default::default() }.content());
        let parts = now.differs(&Post { text: "c".into(), role_ids: s(&["grol_b", "grol_a"]), ..now.clone() });
        assert_eq!(parts, PostParts { text: true, ..Default::default() });
    }

    /// verify3 Minor 2, the orchestrator's point 4: the same post in other bytes is the same post; another is not.
    #[test]
    fn the_same_again_is_seen_through_spaces_unicode_and_order() {
        let post = |title: &str, text: &str, roles: &[&str]| PostDraft { title: title.into(), text: text.into(), role_ids: s(roles), ..Default::default() };
        let one = post("Caf\u{e9} tonight", "Bring your art\nWe start at 20:00", &["grol_a", "grol_b"]);
        for (what, other) in [
            ("another order of the roles", post("Caf\u{e9} tonight", "Bring your art\nWe start at 20:00", &["grol_b", "grol_a"])),
            ("a role twice", post("Caf\u{e9} tonight", "Bring your art\nWe start at 20:00", &["grol_a", "grol_b", "grol_a"])),
            ("e and an accent apart", post("Cafe\u{301} tonight", "Bring your art\nWe start at 20:00", &["grol_a", "grol_b"])),
            ("more spaces and lines", post(" Caf\u{e9}  tonight ", "Bring your art \n\n We start  at 20:00\n", &["grol_a", "grol_b"])),
            ("a no-break space", post("Caf\u{e9}\u{a0}tonight", "Bring your art\r\nWe start at 20:00", &["grol_a", "grol_b"])),
            ("something invisible", post("Caf\u{e9}\u{200b} tonight", "\u{feff}Bring your art\nWe start at 20:00\u{200d}", &["grol_a", "grol_b"])),
            ("wide figures", post("Caf\u{e9} tonight", "Bring your art\nWe start at \u{ff12}\u{ff10}:00", &["grol_a", "grol_b"])),
            ("notified", PostDraft { send_notification: true, ..one.clone() }),
        ] {
            assert_eq!(one.canonical(), other.canonical(), "{what}");
        }
        for (what, other) in [
            ("other words", post("Caf\u{e9} tonight", "Bring your music\nWe start at 20:00", &["grol_a", "grol_b"])),
            ("another case", post("CAF\u{c9} tonight", "Bring your art\nWe start at 20:00", &["grol_a", "grol_b"])),
            ("words moved to the title", post("Caf\u{e9} tonight Bring", "your art\nWe start at 20:00", &["grol_a", "grol_b"])),
            ("another audience", post("Caf\u{e9} tonight", "Bring your art\nWe start at 20:00", &["grol_a"])),
            ("everyone", PostDraft { visibility: Visibility::Public, ..one.clone() }),
        ] {
            assert_ne!(one.canonical(), other.canonical(), "{what}");
        }
        let role = RoleDraft { name: "Greeters".into(), permissions: s(&["a", "b"]), ..Default::default() };
        assert_eq!(role.canonical(), RoleDraft { name: "Greeters ".into(), permissions: s(&["b", "a"]), ..role.clone() }.canonical());
        assert_ne!(role.canonical(), RoleDraft { self_assignable: true, ..role.clone() }.canonical());
        let change = RoleChange { name: Some("Caf\u{e9}".into()), ..Default::default() };
        assert_eq!(change.canonical(), RoleChange { name: Some(" Cafe\u{301}".into()), ..Default::default() }.canonical());
        assert_ne!(change.canonical(), RoleChange { self_assignable: Some(true), ..change.clone() }.canonical());
        assert_ne!(RoleChange { name: Some("-".into()), ..Default::default() }.canonical(), RoleChange::default().canonical(), "a name of that word is not no name");
    }

    /// Minor 2: the nameplate's apply is a seam with only "unavailable" in this version.
    #[test]
    fn the_nameplate_apply_is_unavailable_in_this_version() {
        assert_eq!(NameplateApply::current(), NameplateApply::Unavailable);
        assert!(!NameplateApply::current().can_apply());
    }

    /// Missing order or role data never turns into a right to edit everything.
    #[test]
    fn missing_role_data_withholds() {
        let admin_perms = s(&[ROLES_MANAGE, ROLES_ASSIGN, MEMBERS_MANAGE]);
        let admin_roles = s(&["grol_admin"]);
        let admin = Actor { group: "grp_1", user_id: "usr_admin", permissions: &admin_perms, role_ids: Some(&admin_roles), status: Some("member") };
        let mut roles = roles();
        let rename = RoleChange { name: Some("x".into()), ..Default::default() };
        let none = Catalogue { roles: None, permissions: None };
        assert_eq!(can(Action::EditRole { role: "grol_mod", change: &rename }, &admin, &none), Got::Unknown(Missing::Roles));
        assert_eq!(can(Action::CreateRole { draft: &RoleDraft::default() }, &admin, &none), Got::Unknown(Missing::Roles));
        roles[1].order = None; // the actor's own order unknown
        let cat = Catalogue { roles: Some(&roles), permissions: None };
        assert_eq!(can(Action::EditRole { role: "grol_mod", change: &rename }, &admin, &cat), Got::Unknown(Missing::RoleOrder));
        let roles = self::roles();
        let cat = Catalogue { roles: Some(&roles), permissions: None };
        // A right the admin holds: only the missing catalogue stands in the way.
        let made = |right: &str| RoleDraft { permissions: s(&[right]), ..Default::default() };
        assert_eq!(can(Action::CreateRole { draft: &made(MEMBERS_MANAGE) }, &admin, &cat), Got::Unknown(Missing::Catalogue), "no catalogue, no rights given");
        // A right the admin lacks is refused for that, whatever the catalogue says.
        assert_eq!(can(Action::CreateRole { draft: &made("group-instance-join") }, &admin, &cat), Got::Withheld(Why::Escalation));
        // Words and flags need no catalogue.
        assert_eq!(can(Action::EditRole { role: "grol_mod", change: &rename }, &admin, &cat), Got::Allowed);
        let nobody: Vec<String> = vec![];
        let roleless = Actor { role_ids: Some(&nobody), ..admin };
        assert_eq!(can(Action::DeleteRole { role: "grol_artist" }, &roleless, &cat), Got::Unknown(Missing::Membership));
        let unread = Actor { role_ids: None, ..admin };
        assert_eq!(can(Action::DeleteRole { role: "grol_artist" }, &unread, &cat), Got::Unknown(Missing::Membership));
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
        let d = abilities(&s(&[DEFAULT_ROLE_MANAGE]));
        assert!(d.elsewhere && !d.roles, "Everyone's rights are changed on vrchat.com");
        let o = abilities(&s(&["*"]));
        assert!(o.roles && o.assign && o.posts && o.art && o.elsewhere);
    }

    #[test]
    fn edits_send_only_what_changed_and_consequential_ones_are_confirmed() {
        let base = Role { permissions: s(&["b", "a"]), name: "Artists".into(), ..role("grol_a", 3, &[]) };
        let mut edit = RoleEdit::of(&base);
        assert!(RoleChange::between(&base, &edit).is_empty());
        let spaced = Role { name: " Artists ".into(), ..base.clone() };
        assert!(RoleChange::between(&spaced, &RoleEdit::of(&spaced)).is_empty(), "what was not touched is not sent back tidied");
        edit.name = " Painters ".into();
        let c = RoleChange::between(&base, &edit);
        assert_eq!(c.name.as_deref(), Some("Painters"));
        assert_eq!((c.fields(), c.consequential()), (vec![RoleField::Name], false));
        edit.requires_two_factor = true;
        let c = RoleChange::between(&base, &edit);
        assert_eq!((c.fields(), c.consequential()), (vec![RoleField::Name, RoleField::TwoFactor], true));
        assert_eq!(base.with(&c).permissions, base.permissions, "its rights are no part of a change");
    }

    /// What moved beside what was asked: a role's other parts, a group's other pictures and words; what one of the two
    /// answers leaves out says nothing.
    #[test]
    fn what_moved_beside_is_seen() {
        let a = Role { permissions: s(&["a", "b"]), ..role("grol_a", 3, &[]) };
        assert!(!a.differs(&Role { permissions: s(&["b", "a"]), ..a.clone() }).any(), "an order is no change");
        assert_eq!(a.differs(&Role { permissions: s(&["a"]), ..a.clone() }), RoleParts { permissions: true, ..Default::default() });
        assert_eq!(a.differs(&Role { order: Some(9), ..a.clone() }), RoleParts { standing: true, ..Default::default() });
        let g = Group { banner_id: Some("file_b".into()), icon_id: Some("file_i".into()), nameplate_id: Some("file_n".into()), banner_known: true, icon_known: true, nameplate_known: true, words_known: true, description: "d".into(), ..Default::default() };
        let after = Group { banner_id: Some("file_new".into()), ..g.clone() };
        assert!(!g.moved_beside(ArtSlot::Banner, &after).any(), "the slot asked");
        assert_eq!(g.moved_beside(ArtSlot::Icon, &after), GroupParts { banner: true, ..Default::default() });
        assert_eq!(g.moved_beside(ArtSlot::Banner, &Group { icon_id: None, nameplate_id: None, ..after.clone() }), GroupParts { icon: true, nameplate: true, ..Default::default() });
        assert!(!g.moved_beside(ArtSlot::Banner, &Group { icon_id: None, icon_known: false, ..after.clone() }).any(), "not said is not gone");
        assert!(!g.moved_beside(ArtSlot::Banner, &Group { words_known: false, description: String::new(), ..after.clone() }).any());
        assert!(g.moved_beside(ArtSlot::Banner, &Group { description: String::new(), ..after }).description);
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
