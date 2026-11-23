//! VRChat's group calls (REQUEST22 1), against the attack's wire contract (attack22-groups.md, "Wire contracts"): paths
//! and bodies from the community OpenAPI spec at 68091ebc (`getUserAllGroupPermissions`, `getUserGroups`, `getGroup`
//! with `includeRoles`, `getGroupPermissions`, `getGroupMembers`, `searchGroupMembers`, `getGroupMember`,
//! `addGroupMemberRole` / `removeGroupMemberRole` (no body), `createGroupRole`, `updateGroupRole`, `deleteGroupRole`,
//! `getGroupPosts`, `addGroupPost`, `deleteGroupPost`, `getFiles`, `getFile`, `uploadImage` (multipart), `updateGroup`),
//! and VRCX at e819c4e4 (`src/api/group.ts`: the post id in the post path).
//!
//! What is never sent: the legacy announcement routes (`G/announcement`, whose POST replaces every announcement: B1),
//! `updateGroupPost` (it carries the audience whole and takes no condition: verify3 Blocker 4), `nameplateId` (B4), and a
//! null. Every write goes in the [`WRITES`] family, whose lost answers are "outcome unknown" (B2), through
//! the client's pacing and shared 429 back-off. Errors are local codes ([`Failure`]): no server text reaches a notice.
use super::*;
use crate::groups::{self as g, ArtSlot, FileTag, Group, GroupCard, InventoryFile, Member, MyMember, PermissionInfo, Post, PostDraft, Role, RoleChange, RoleDraft, RoleField, Visibility};
use serde_json::{json, Map, Value};

/// The pacing family of every group write (and of a picture upload).
pub(super) const WRITES: &str = "group-writes";
const READS: &str = "groups";
/// Members and posts are asked this many at a time (VRChat takes 1 to 100).
pub const PAGE: usize = 50;
/// A picture uploaded from this computer is this large at most.
pub const UPLOAD_MAX: usize = 10 * 1024 * 1024;

/// Why a group call did not give what it asked for: local codes, worded by the interface. No server text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Failure {
    /// Not signed in to VRChat (or its session ended).
    SignedOut,
    /// The grant ended or the request expired before it was sent: nothing was sent.
    Disabled,
    /// VRChat answered 403: this account's role does not allow it (or it is not a member).
    Forbidden,
    /// VRChat answered 404, or the item is gone.
    NotFound,
    /// VRChat answered 400: what was sent does not fit its rules.
    Invalid,
    /// VRChat asks to wait this many seconds.
    RateLimited(u64),
    /// VRChat could not be reached (nothing was sent, or it was a read).
    Unavailable,
    /// A write was sent and its answer was lost: VRChat may have done it (B2).
    Unknown,
    /// VRChat's answer to a read could not be read.
    BadAnswer,
}

impl Failure {
    fn of(e: Error, write: bool) -> Self {
        match e {
            Error::OutcomeUnknown => Failure::Unknown,
            Error::Disabled | Error::DeadlineExceeded => Failure::Disabled,
            Error::Unauthorized => Failure::SignedOut,
            Error::RateLimited(d) => Failure::RateLimited(d.as_secs().max(1)),
            Error::NotFound => Failure::NotFound,
            Error::Refused(400, _) => Failure::Invalid,
            Error::Refused(401 | 403, _) => Failure::Forbidden,
            Error::Refused(404, _) => Failure::NotFound,
            // Any other refusal (409, 422…): VRChat said no; nothing was done.
            Error::Refused(400..=499, _) => Failure::Invalid,
            Error::InvalidResponse if write => Failure::Unknown,
            Error::Unavailable if write => Failure::Unknown,
            Error::Unavailable => Failure::Unavailable,
            _ => if write { Failure::Unknown } else { Failure::BadAnswer },
        }
    }
}

pub type GroupResult<T> = std::result::Result<T, Failure>;

/// A page of members: those VRChat gave (complete or not), how many it gave (offsets advance by that, M8), and the
/// search's total when it was a search.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Members {
    pub items: Vec<Member>,
    pub raw: usize,
    pub total: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posts {
    pub items: Vec<Post>,
    pub raw: usize,
    pub total: Option<u32>,
}

/// A page of the account's own pictures and how many entries VRChat gave.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    pub items: Vec<InventoryFile>,
    pub raw: usize,
    /// What tells one picture from another, for a check (never sent to the window).
    #[serde(skip)]
    pub details: Vec<FileDetail>,
}

/// A file of the account's with what VRChat says of its latest version.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDetail {
    pub file: InventoryFile,
    pub owner: String,
    pub tags: Option<Vec<String>>,
    /// Base64, as VRChat's file API gives and takes it.
    pub md5: Option<String>,
    pub size: Option<u64>,
    /// The latest version's number.
    pub version: Option<u64>,
    pub created_at: Option<String>,
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}
fn https(v: &Value, k: &str) -> String {
    Some(s(v, k)).filter(|u| u.starts_with("https://") && u.len() <= 1024).unwrap_or_default()
}
fn count(v: &Value, k: &str) -> Option<u32> {
    v.get(k).and_then(Value::as_u64).map(|n| n.min(u32::MAX as u64) as u32)
}
fn strings(v: &Value, k: &str, max: usize) -> Vec<String> {
    v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).filter(|x| x.len() <= 128).take(max).collect()).unwrap_or_default()
}
/// Every entry of an array of strings, or nothing: one dropped for its form or its length would change what the list says.
fn whole_strings(v: &Value, k: &str, max: usize) -> Option<Vec<String>> {
    let a = v.get(k)?.as_array()?;
    if a.len() > max {
        return None;
    }
    a.iter().map(|x| x.as_str().filter(|s| !s.is_empty() && s.len() <= 1024).map(str::to_string)).collect()
}
fn whole_role_ids(v: &Value, k: &str) -> Option<Vec<String>> {
    whole_strings(v, k, 64).filter(|l| l.iter().all(|r| g::is_role_id(r)))
}
/// A picture field: there (a file id, or null for none), or not known.
fn picture(v: &Value, k: &str) -> (Option<String>, bool) {
    match v.get(k) {
        Some(Value::Null) => (None, true),
        Some(Value::String(s)) if g::is_file_id(s) => (Some(s.clone()), true),
        _ => (None, false),
    }
}
fn file_id(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).filter(|id| g::is_file_id(id)).map(str::to_string)
}

pub(crate) fn parse_role(v: &Value) -> Option<Role> {
    let id = s(v, "id");
    if !g::is_role_id(&id) {
        return None;
    }
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    // What the rules decide by must all be there, the rights as an array of strings: a missing field is not "none"
    // (verify-r22 Blocker 4).
    let rights = whole_strings(v, "permissions", 128);
    let flags = |k: &[&str]| k.iter().all(|k| v.get(*k).is_some_and(Value::is_boolean));
    let known = rights.is_some() && v.get("order").and_then(Value::as_i64).is_some() && flags(&["defaultRole", "requiresPurchase"]);
    let words = ["name", "description"].iter().all(|k| v.get(*k).is_some_and(Value::is_string));
    let whole = known && words && flags(&["isManagementRole", "isSelfAssignable", "isAddedOnJoin", "requiresTwoFactor"]);
    Some(Role {
        // As VRChat has them (an edit is compared with and sent against these); shown through `groups::label`.
        name: s(v, "name").chars().take(256).collect(),
        description: s(v, "description").chars().take(2048).collect(),
        order: v.get("order").and_then(Value::as_i64),
        // Every right VRChat gives is kept as it is, those this version does not know included (never sent back unasked).
        permissions: rights.unwrap_or_default(),
        known,
        whole,
        created_at: v.get("createdAt").and_then(Value::as_str).and_then(g::time),
        default_role: flag("defaultRole"),
        // Left out, the strict side: it counts as one, so that giving it is always confirmed.
        management: flag("isManagementRole") || !flags(&["isManagementRole"]),
        self_assignable: flag("isSelfAssignable"),
        added_on_join: flag("isAddedOnJoin"),
        requires_two_factor: flag("requiresTwoFactor"),
        requires_purchase: flag("requiresPurchase"),
        updated_at: v.get("updatedAt").and_then(Value::as_str).map(str::to_string),
        id,
    })
}

fn parse_roles(v: &Value) -> Option<Vec<Role>> {
    let list = v.as_array()?;
    // A role VRChat gave that cannot be read, or more than this reads, makes the list incomplete: then it is not a list
    // to rank against, nor one a role's absence means anything in.
    if list.len() > 200 {
        return None;
    }
    list.iter().map(parse_role).collect()
}

fn parse_my_member(v: &Value) -> Option<MyMember> {
    if !v.is_object() {
        return None;
    }
    let (ids, managed, rights) = (whole_role_ids(v, "roleIds"), whole_role_ids(v, "mRoleIds"), whole_strings(v, "permissions", 128));
    let (rights_known, roles_known, managed_known) = (rights.is_some(), ids.is_some(), managed.is_some());
    let mut role_ids = ids.unwrap_or_default();
    for m in managed.unwrap_or_default() {
        if !role_ids.contains(&m) {
            role_ids.push(m);
        }
    }
    Some(MyMember {
        user_id: s(v, "userId"),
        membership_id: s(v, "id"),
        joined_at: v.get("joinedAt").and_then(Value::as_str).and_then(g::time),
        role_ids,
        permissions: rights.unwrap_or_default(),
        status: s(v, "membershipStatus"),
        rights_known,
        roles_known,
        managed_known,
        group_id: v.get("groupId").and_then(Value::as_str).map(str::to_string),
    })
}

pub(crate) fn parse_group(v: &Value) -> Option<Group> {
    let id = s(v, "id");
    if !g::is_group_id(&id) {
        return None;
    }
    let ((icon_id, icon_known), (banner_id, banner_known)) = (picture(v, "iconId"), picture(v, "bannerId"));
    Some(Group {
        icon_id,
        icon_known,
        banner_id,
        banner_known,
        name: g::label(&s(v, "name"), 64),
        short_code: g::label(&s(v, "shortCode"), 12),
        discriminator: g::label(&s(v, "discriminator"), 8),
        description: g::text(&s(v, "description"), 1000),
        rules: g::text(&s(v, "rules"), 4000),
        links: strings(v, "links", 3).iter().filter_map(|l| g::link(l)).collect(),
        icon_url: https(v, "iconUrl"),
        banner_url: https(v, "bannerUrl"),
        member_count: count(v, "memberCount"),
        online_count: count(v, "onlineMemberCount"),
        owner_id: s(v, "ownerId"),
        privacy: g::label(&s(v, "privacy"), 16),
        my_member: v.get("myMember").and_then(parse_my_member),
        roles: v.get("roles").and_then(parse_roles),
        id,
    })
}

/// A membership of the account's list (`LimitedUserGroups`): keyed by its GROUP id (`groupId`), never by `id`, which is
/// the membership's (M1).
pub(crate) fn parse_membership(v: &Value) -> Option<(GroupCard, String)> {
    let id = s(v, "groupId");
    if !g::is_group_id(&id) {
        return None;
    }
    let card = GroupCard {
        name: g::label(&s(v, "name"), 64),
        short_code: g::label(&s(v, "shortCode"), 12),
        discriminator: g::label(&s(v, "discriminator"), 8),
        icon_url: https(v, "iconUrl"),
        banner_url: https(v, "bannerUrl"),
        member_count: count(v, "memberCount"),
        named: true,
        id,
        ..Default::default()
    };
    Some((card, s(v, "ownerId")))
}

pub(crate) fn parse_member(v: &Value) -> Option<Member> {
    let user_id = s(v, "userId");
    if !crate::vrchat_accounts::is_user_id(&user_id) {
        return None;
    }
    let user = v.get("user").filter(|u| u.is_object());
    // Both role arrays (the schema requires them): an absent one is unknown roles, not "no role" (Blocker 4, B-D).
    let (ids, managed) = (whole_role_ids(v, "roleIds"), whole_role_ids(v, "mRoleIds"));
    let roles_known = ids.is_some() && managed.is_some();
    let mut role_ids = ids.unwrap_or_default();
    for m in managed.unwrap_or_default() {
        if !role_ids.contains(&m) {
            role_ids.push(m);
        }
    }
    Some(Member {
        membership_id: s(v, "id"),
        display_name: user.map(|u| g::label(&s(u, "displayName"), 32)).filter(|n| !n.is_empty()),
        icon_url: user.map(|u| https(u, "iconUrl")).unwrap_or_default(),
        role_ids,
        joined_at: v.get("joinedAt").and_then(Value::as_str).and_then(g::time),
        roles_known,
        group_id: v.get("groupId").and_then(Value::as_str).map(str::to_string),
        user_id,
    })
}

pub(crate) fn parse_post(v: &Value) -> Option<Post> {
    let id = s(v, "id");
    if !g::is_post_id(&id) {
        return None;
    }
    // The post exactly as VRChat has it: what a deletion was made against (Blocker 3). Shown through `groups::label` and
    // `groups::text`. Whatever is not whole (words that are not there or not words, an unknown audience, a role id that
    // is not one, too many roles, an over-long text, an image id of another form) makes it read-only here.
    let (title, text) = (s(v, "title"), s(v, "text"));
    let words = ["title", "text"].iter().all(|k| v.get(*k).is_some_and(Value::is_string));
    let visibility = match v.get("visibility").and_then(Value::as_str) {
        Some("public") => Some(Visibility::Public),
        Some("group") => Some(Visibility::Group),
        _ => None,
    };
    let roles = v.get("roleIds");
    let role_ids: Vec<String> = roles.and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    // A group post's restriction must be there to be known: absent is not "every member" (verify2 B7). A public one has none.
    let roles_whole = match roles {
        Some(Value::Array(a)) => a.len() <= 32 && a.iter().all(|x| x.as_str().is_some_and(g::is_role_id)),
        None | Some(Value::Null) => visibility == Some(Visibility::Public),
        _ => false,
    };
    let image_whole = picture(v, "imageId").1;
    let complete = words
        && visibility.is_some()
        && roles_whole
        && image_whole
        && title.chars().count() <= g::POST_TITLE_MAX
        && text.chars().count() <= g::POST_TEXT_MAX;
    Some(Post {
        visibility: visibility.unwrap_or(Visibility::Group),
        role_ids,
        image_id: file_id(v, "imageId"),
        image_url: https(v, "imageUrl"),
        author_id: s(v, "authorId"),
        created_at: g::time(&s(v, "createdAt")),
        updated_at: g::time(&s(v, "updatedAt")),
        title: title.chars().take(g::POST_TITLE_MAX * 4).collect(),
        text: text.chars().take(g::POST_TEXT_MAX * 4).collect(),
        complete,
        id,
    })
}

/// A file of the account's (`File`): its id and its latest version's address on VRChat's file API.
pub(crate) fn parse_file(v: &Value) -> Option<InventoryFile> {
    let id = s(v, "id");
    let url = v.get("versions")?.as_array()?.iter().rev().find_map(|version| version.get("file").and_then(|f| f.get("url")).and_then(Value::as_str).map(str::to_string))?;
    // Its address is that file's own: what is shown for a file id is that file (verify2 B-E).
    (g::is_file_id(&id) && url.starts_with(&format!("https://api.vrchat.cloud/api/1/file/{id}/"))).then_some(InventoryFile { id, url })
}

pub(crate) fn parse_file_detail(v: &Value) -> Option<FileDetail> {
    let file = parse_file(v)?;
    let latest = v.get("versions")?.as_array()?.iter().rev().find(|x| x.get("file").is_some_and(|f| f.get("url").is_some()))?;
    let data = latest.get("file");
    Some(FileDetail {
        file,
        owner: s(v, "ownerId"),
        tags: whole_strings(v, "tags", 64),
        md5: data.and_then(|f| f.get("md5")).and_then(Value::as_str).map(str::to_string),
        size: data.and_then(|f| f.get("sizeInBytes")).and_then(Value::as_u64),
        version: latest.get("version").and_then(Value::as_u64),
        created_at: latest.get("created_at").and_then(Value::as_str).and_then(g::time),
    })
}

/// The MD5 of `bytes` as VRChat's file API writes it.
pub fn file_md5(bytes: &[u8]) -> String {
    md5_base64(bytes)
}

// --- Bodies (pure: tested on their own) ---

/// `createGroupRole`: name, description, rights and the three flags. `requiresPurchase` is never sent.
pub fn role_create_body(draft: &RoleDraft) -> String {
    json!({
        "name": draft.name.trim(),
        "description": draft.description,
        "permissions": g::in_order(&draft.permissions),
        "isSelfAssignable": draft.self_assignable,
        "isAddedOnJoin": draft.added_on_join,
        "requiresTwoFactor": draft.requires_two_factor,
    })
    .to_string()
}

/// `updateGroupRole`: only the fields that changed (M2, M4). `order` is never sent (reorder is not in v1).
pub fn role_update_body(change: &RoleChange) -> String {
    let mut m = Map::new();
    if let Some(v) = &change.name {
        m.insert("name".into(), json!(v));
    }
    if let Some(v) = &change.description {
        m.insert("description".into(), json!(v));
    }
    if let Some(v) = &change.permissions {
        m.insert("permissions".into(), json!(g::in_order(v)));
    }
    if let Some(v) = change.self_assignable {
        m.insert("isSelfAssignable".into(), json!(v));
    }
    if let Some(v) = change.added_on_join {
        m.insert("isAddedOnJoin".into(), json!(v));
    }
    if let Some(v) = change.requires_two_factor {
        m.insert("requiresTwoFactor".into(), json!(v));
    }
    Value::Object(m).to_string()
}

/// `addGroupPost`: the notification only when the person turned it on (off by default, M7); roles only for a group post.
pub fn post_create_body(draft: &PostDraft) -> String {
    let mut m = Map::new();
    m.insert("title".into(), json!(draft.title.trim()));
    m.insert("text".into(), json!(draft.text.trim()));
    m.insert("visibility".into(), json!(draft.visibility.wire()));
    m.insert("sendNotification".into(), json!(draft.send_notification));
    let roles = draft.audience();
    if !roles.is_empty() {
        m.insert("roleIds".into(), json!(roles));
    }
    Value::Object(m).to_string()
}

/// `updateGroup` for one picture: `{"bannerId": …}` or `{"iconId": …}` and nothing else (no name, no `nameplateId`).
pub fn art_body(slot: ArtSlot, file: &str) -> String {
    let key = match slot {
        ArtSlot::Banner => "bannerId",
        ArtSlot::Icon => "iconId",
    };
    let mut m = Map::new();
    m.insert(key.into(), json!(file));
    Value::Object(m).to_string()
}

fn body_json(body: &[u8]) -> Option<Value> {
    serde_json::from_slice(body).ok()
}

impl Client {
    /// One call of this module: its status told apart (403 from 404, 400 apart), its body when it worked.
    fn group_call(&mut self, method: &'static str, path: &str, body: Option<String>, form: Option<Form>) -> GroupResult<Vec<u8>> {
        let write = method != "GET";
        self.authenticated().map_err(|e| Failure::of(e, false))?;
        let family = if write { WRITES } else if path.starts_with("/file") { "files" } else { READS };
        let r = self.request_with(family, method, format!("{BASE}{path}"), None, body, form, &[400, 403, 404]).map_err(|e| Failure::of(e, write))?;
        match r.status {
            400 => Err(Failure::Invalid),
            403 => Err(Failure::Forbidden),
            404 => Err(Failure::NotFound),
            _ => Ok(r.body),
        }
    }
    fn group_read(&mut self, path: &str) -> GroupResult<Value> {
        let body = self.group_call("GET", path, None, None)?;
        body_json(&body).ok_or(Failure::BadAnswer)
    }
    /// A write's answer: one that cannot be read means it may have been done (B2).
    fn group_write(&mut self, method: &'static str, path: &str, body: Option<String>) -> GroupResult<Value> {
        let answer = self.group_call(method, path, body, None)?;
        body_json(&answer).ok_or(Failure::Unknown)
    }
    /// Waits until a group write may go (its family's pacing), so that a read made next is followed by the write without
    /// waiting on anything else (verify2 B-G). Under the client's lock nothing else writes to groups meanwhile.
    pub fn ready_to_write(&mut self) -> GroupResult<()> {
        self.authenticated().map_err(|e| Failure::of(e, false))?;
        loop {
            self.permitted().map_err(|e| Failure::of(e, false))?;
            let wait = {
                let limiter = self.limiter.lock().unwrap();
                let now = Instant::now();
                if let Some(wait) = limiter.blocked.map(|b| b.saturating_duration_since(now)).filter(|w| !w.is_zero()) {
                    return Err(Failure::RateLimited(wait.as_secs().max(1)));
                }
                limiter.wait(WRITES, now)
            };
            if wait.is_zero() {
                return Ok(());
            }
            std::thread::sleep(wait.min(Duration::from_millis(50)));
        }
    }
    fn me(&self) -> GroupResult<String> {
        self.own_id().map_err(|_| Failure::SignedOut)
    }
    fn group_path(group: &str) -> GroupResult<String> {
        if g::is_group_id(group) { Ok(format!("/groups/{}", encode(group))) } else { Err(Failure::NotFound) }
    }

    /// `GET /users/{me}/groups/permissions`: every joined group's id and the account's effective rights in it, at once.
    pub fn my_group_permissions(&mut self) -> GroupResult<Vec<(String, Vec<String>)>> {
        let me = self.me()?;
        let v = self.group_read(&format!("/users/{}/groups/permissions", encode(&me)))?;
        let map = v.as_object().ok_or(Failure::BadAnswer)?;
        Ok(map
            .iter()
            .filter(|(id, _)| g::is_group_id(id))
            .take(1000)
            .map(|(id, rights)| (id.clone(), rights.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).take(128).collect()).unwrap_or_default()))
            .collect())
    }

    /// `GET /users/{me}/groups`: the account's memberships, as cards and their owner's id.
    pub fn my_groups(&mut self) -> GroupResult<Vec<(GroupCard, String)>> {
        let me = self.me()?;
        let v = self.group_read(&format!("/users/{}/groups", encode(&me)))?;
        let list = v.as_array().ok_or(Failure::BadAnswer)?;
        Ok(list.iter().filter_map(parse_membership).take(1000).collect())
    }

    /// `GET /groups/{g}?includeRoles=true`: the group, the account's membership and the roles.
    pub fn group(&mut self, group: &str) -> GroupResult<Group> {
        let path = Self::group_path(group)?;
        let v = self.group_read(&format!("{path}?includeRoles=true"))?;
        let found = parse_group(&v).ok_or(Failure::BadAnswer)?;
        if found.id != group {
            return Err(Failure::BadAnswer);
        }
        Ok(found)
    }

    /// `GET /groups/{g}/roles`.
    pub fn group_roles(&mut self, group: &str) -> GroupResult<Vec<Role>> {
        let path = Self::group_path(group)?;
        let v = self.group_read(&format!("{path}/roles"))?;
        parse_roles(&v).ok_or(Failure::BadAnswer)
    }

    /// `GET /groups/{g}/permissions`: the rights a role can hold, and which this account may give: every entry whole, or
    /// nothing (a right missing from a list read in part is not "not grantable").
    pub fn group_permission_catalogue(&mut self, group: &str) -> GroupResult<Option<Vec<PermissionInfo>>> {
        let path = Self::group_path(group)?;
        let v = self.group_read(&format!("{path}/permissions"))?;
        let list = v.as_array().ok_or(Failure::BadAnswer)?;
        if list.len() > 256 {
            return Ok(None);
        }
        Ok(list
            .iter()
            .map(|p| {
                let name = s(p, "name");
                let depends_on = match p.get("dependsOn") {
                    None => Some(Vec::new()),
                    Some(_) => whole_strings(p, "dependsOn", 64),
                };
                (!name.is_empty() && name.len() <= 1024).then_some(())?;
                Some(PermissionInfo {
                    display_name: g::label(&s(p, "displayName"), 64),
                    help: g::text(&s(p, "help"), 300),
                    allowed_to_add: p.get("allowedToAdd").and_then(Value::as_bool)?,
                    depends_on: depends_on?,
                    management: p.get("isManagementPermission").and_then(Value::as_bool).unwrap_or(false),
                    name,
                })
            })
            .collect())
    }

    /// `GET /groups/{g}/members?n&offset&sort=joinedAt:desc`: an array of the OTHER members (never the account itself).
    pub fn group_members(&mut self, group: &str, offset: usize) -> GroupResult<Members> {
        let path = Self::group_path(group)?;
        let v = self.group_read(&format!("{path}/members?n={PAGE}&offset={offset}&sort=joinedAt:desc"))?;
        let list = v.as_array().ok_or(Failure::BadAnswer)?;
        Ok(Members { raw: list.len(), items: list.iter().take(100).filter_map(parse_member).collect(), total: None })
    }

    /// `GET /groups/{g}/members/search?query&n&offset`: `{results,total}`, three characters at least.
    pub fn search_group_members(&mut self, group: &str, query: &str, offset: usize) -> GroupResult<Members> {
        let path = Self::group_path(group)?;
        let query: String = query.trim().chars().take(32).collect();
        if query.chars().count() < 3 {
            return Err(Failure::Invalid);
        }
        let v = self.group_read(&format!("{path}/members/search?query={}&n={PAGE}&offset={offset}", encode(&query)))?;
        let list = v.get("results").and_then(Value::as_array).ok_or(Failure::BadAnswer)?;
        Ok(Members { raw: list.len(), items: list.iter().take(100).filter_map(parse_member).collect(), total: count(&v, "total") })
    }

    /// `GET /groups/{g}/members/{u}`: the membership, or `None` (VRChat answers `null` for someone not in it).
    pub fn group_member(&mut self, group: &str, user: &str) -> GroupResult<Option<Member>> {
        let path = Self::group_path(group)?;
        if !crate::vrchat_accounts::is_user_id(user) {
            return Err(Failure::NotFound);
        }
        let v = self.group_read(&format!("{path}/members/{}", encode(user)))?;
        if v.is_null() {
            return Ok(None);
        }
        parse_member(&v).map(Some).ok_or(Failure::BadAnswer)
    }

    /// `PUT` (add) or `DELETE` (remove) `G/members/{u}/roles/{r}`, no body: the member's roles as VRChat now has them.
    pub fn set_member_role(&mut self, group: &str, user: &str, role: &str, add: bool) -> GroupResult<Vec<String>> {
        let path = Self::group_path(group)?;
        if !crate::vrchat_accounts::is_user_id(user) || !g::is_role_id(role) {
            return Err(Failure::NotFound);
        }
        let v = self.group_write(if add { "PUT" } else { "DELETE" }, &format!("{path}/members/{}/roles/{}", encode(user), encode(role)), None)?;
        // The member's roles now: an answer that is not a plain list of role ids says nothing sure (Major 2).
        let ids = v.as_array().filter(|a| a.len() <= 64 && a.iter().all(|x| x.as_str().is_some_and(g::is_role_id))).ok_or(Failure::Unknown)?;
        let ids: Vec<String> = ids.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
        // With the change asked: an answer without it contradicts itself, and settles nothing (J2).
        if ids.iter().any(|r| r == role) != add {
            return Err(Failure::Unknown);
        }
        Ok(ids)
    }

    /// `POST G/roles`: the role made.
    pub fn create_role(&mut self, group: &str, draft: &RoleDraft) -> GroupResult<Role> {
        let path = Self::group_path(group)?;
        let v = self.group_write("POST", &format!("{path}/roles"), Some(role_create_body(draft)))?;
        // The role asked, in this group (J2).
        let made = parse_role(&v).ok_or(Failure::Unknown)?;
        let same = made.digest_of(&RoleField::ALL) == draft.as_role().digest_of(&RoleField::ALL);
        if !made.whole || !same || v.get("groupId").and_then(Value::as_str).is_some_and(|g| g != group) {
            return Err(Failure::Unknown);
        }
        Ok(made)
    }

    /// `PUT G/roles/{r}` with what changed: every role as VRChat now has them.
    pub fn update_role(&mut self, group: &str, role: &str, change: &RoleChange) -> GroupResult<Vec<Role>> {
        let path = Self::group_path(group)?;
        if !g::is_role_id(role) {
            return Err(Failure::NotFound);
        }
        let v = self.group_write("PUT", &format!("{path}/roles/{}", encode(role)), Some(role_update_body(change)))?;
        let roles = parse_roles(&v).ok_or(Failure::Unknown)?;
        // The role asked, with what was asked of it (J2).
        let fields = change.fields();
        match roles.iter().find(|r| r.id == role) {
            Some(r) if r.whole && r.digest_of(&fields) == r.with(change).digest_of(&fields) => Ok(roles),
            _ => Err(Failure::Unknown),
        }
    }

    /// `DELETE G/roles/{r}`: the roles left.
    pub fn delete_role(&mut self, group: &str, role: &str) -> GroupResult<Vec<Role>> {
        let path = Self::group_path(group)?;
        if !g::is_role_id(role) {
            return Err(Failure::NotFound);
        }
        let v = self.group_write("DELETE", &format!("{path}/roles/{}", encode(role)), None)?;
        // The roles left: without it, and with those no group is without (its owner's, Everyone's). A list that holds
        // neither is not the group's roles (verify3 Major 5).
        let roles = parse_roles(&v).ok_or(Failure::Unknown)?;
        if roles.iter().any(|r| r.id == role) || !roles.iter().any(|r| r.known && (r.default_role || r.is_owner_role())) {
            return Err(Failure::Unknown);
        }
        Ok(roles)
    }

    /// `GET G/posts?n&offset`: `{posts,total}`.
    pub fn group_posts(&mut self, group: &str, offset: usize) -> GroupResult<Posts> {
        let path = Self::group_path(group)?;
        let v = self.group_read(&format!("{path}/posts?n={PAGE}&offset={offset}"))?;
        let list = v.get("posts").and_then(Value::as_array).ok_or(Failure::BadAnswer)?;
        Ok(Posts { raw: list.len(), items: list.iter().take(100).filter_map(parse_post).collect(), total: count(&v, "total") })
    }

    /// `POST G/posts`: the post made (VRChat gives no count of who was notified).
    pub fn create_post(&mut self, group: &str, draft: &PostDraft) -> GroupResult<Post> {
        let path = Self::group_path(group)?;
        let v = self.group_write("POST", &format!("{path}/posts"), Some(post_create_body(draft)))?;
        // The post asked, whole, by this account, of this group (Major 2, J2): another answer says nothing sure.
        let post = parse_post(&v).ok_or(Failure::Unknown)?;
        let mine = self.me().ok();
        if !post.complete
            || post.content() != draft.content()
            || v.get("groupId").and_then(Value::as_str).is_some_and(|g| g != group)
            || (!post.author_id.is_empty() && mine.as_deref() != Some(post.author_id.as_str()))
        {
            return Err(Failure::Unknown);
        }
        Ok(post)
    }

    /// `DELETE G/posts/{post}`.
    pub fn delete_post(&mut self, group: &str, post: &str) -> GroupResult<()> {
        let path = Self::group_path(group)?;
        if !g::is_post_id(post) {
            return Err(Failure::NotFound);
        }
        let answer = self.group_call("DELETE", &format!("{path}/posts/{}", encode(post)), None, None)?;
        // VRChat's success envelope (`{"success":{…,"status_code":200}}`): anything else says nothing sure (Major 2, J2).
        match body_json(&answer) {
            Some(v) if v.get("success").and_then(|s| s.get("status_code")).and_then(Value::as_u64) == Some(200) => Ok(()),
            _ => Err(Failure::Unknown),
        }
    }

    /// `GET /files?tag=gallery|icon&n&offset`: a page of the account's own pictures, and how many VRChat gave (offsets
    /// advance by that, Minor 5).
    pub fn inventory(&mut self, tag: FileTag, offset: usize) -> GroupResult<Inventory> {
        let v = self.group_read(&format!("/files?tag={}&n={PAGE}&offset={offset}", tag.wire()))?;
        let list = v.as_array().ok_or(Failure::BadAnswer)?;
        let details: Vec<FileDetail> = list.iter().take(100).filter_map(parse_file_detail).collect();
        Ok(Inventory { raw: list.len(), items: details.iter().map(|d| d.file.clone()).collect(), details })
    }

    /// `GET /file/{id}`: a file with its owner, its tags and its latest version.
    pub fn picture_file(&mut self, id: &str) -> GroupResult<FileDetail> {
        if !g::is_file_id(id) {
            return Err(Failure::NotFound);
        }
        let v = self.group_read(&format!("/file/{}", encode(id)))?;
        parse_file_detail(&v).filter(|d| d.file.id == id).ok_or(Failure::BadAnswer)
    }

    /// `POST /file/image` (multipart: `tag`, `file`): a PNG into the account's own gallery or icons, the file made.
    /// Its own step: nothing about a group changes here (M6).
    pub fn upload_image(&mut self, tag: FileTag, png: Vec<u8>) -> GroupResult<InventoryFile> {
        if png.len() > UPLOAD_MAX || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(Failure::Invalid);
        }
        self.authenticated().map_err(|e| Failure::of(e, false))?;
        let (md5, size) = (md5_base64(&png), png.len() as u64);
        let form = Form { fields: vec![("tag", tag.wire().to_string())], file_name: "orbiters.png", mime: "image/png", bytes: png };
        let answer = self.group_call("POST", "/file/image", None, Some(form))?;
        // One of this account's, of the kind asked (the schema requires both): another file's answer settles nothing (J2).
        let made = body_json(&answer).as_ref().and_then(parse_file_detail).ok_or(Failure::Unknown)?;
        if self.me().ok().as_deref() != Some(made.owner.as_str()) || !made.tags.as_ref().is_some_and(|t| t.iter().any(|t| t == tag.wire())) {
            return Err(Failure::Unknown);
        }
        // Of these bytes, when VRChat says of which (verify3 Major 4).
        if made.size.is_some_and(|n| n != size) || made.md5.as_ref().is_some_and(|m| *m != md5) {
            return Err(Failure::Unknown);
        }
        Ok(made.file)
    }

    /// `PUT /groups/{g}` with one picture's file id: the group as VRChat now has it.
    pub fn set_group_art(&mut self, group: &str, slot: ArtSlot, file: &str) -> GroupResult<Group> {
        let path = Self::group_path(group)?;
        if !g::is_file_id(file) {
            return Err(Failure::NotFound);
        }
        let v = self.group_write("PUT", &path, Some(art_body(slot, file)))?;
        // The group asked, with the picture asked in the slot asked (Major 2).
        let found = parse_group(&v).ok_or(Failure::Unknown)?;
        if found.id != group || found.art(slot) != Some(Some(file)) {
            return Err(Failure::Unknown);
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{mock_signed_in, raw_response};
    use super::*;
    const G: &str = "grp_00000000-0000-4000-8000-0000000000aa";
    const U: &str = "usr_00000000-0000-4000-8000-0000000000bb";

    fn group_json() -> Value {
        json!({"id": G, "name": "Made-up Makers", "shortCode": "MAKE", "discriminator": "0001", "ownerId": "usr_test", "memberCount": 120,
            "onlineMemberCount": 9, "iconId": "file_icon", "bannerId": "file_banner", "iconUrl": "https://api.vrchat.cloud/api/1/file/file_icon/1/file",
            "nameplateId": null, "links": ["https://example.org", "javascript:alert(1)"],
            "myMember": {"id": "gmem_me", "userId": "usr_test", "roleIds": ["grol_a"], "mRoleIds": ["grol_owner"], "permissions": ["*"], "membershipStatus": "member", "managerNotes": "private"},
            "roles": [role_json("grol_owner", "Owner", 0, &["*"]), role_json("grol_a", "Artists", 3, &["group-some-future-right"])]})
    }
    fn role_json(id: &str, name: &str, order: i64, rights: &[&str]) -> Value {
        json!({"id": id, "groupId": G, "name": name, "description": "", "order": order, "permissions": rights, "defaultRole": false, "isManagementRole": false,
            "requiresPurchase": false, "isSelfAssignable": false, "isAddedOnJoin": false, "requiresTwoFactor": false})
    }

    #[test]
    fn a_group_reads_its_membership_roles_and_plain_links() {
        let g = parse_group(&group_json()).unwrap();
        assert_eq!(g.links, ["https://example.org"], "only http(s) links");
        assert_eq!(g.my_member.as_ref().unwrap().role_ids, ["grol_a", "grol_owner"]);
        assert_eq!(g.roles.as_ref().unwrap()[1].permissions, ["group-some-future-right"], "unknown rights are kept as read");
        assert_eq!(g.online_count, Some(9));
        let text = serde_json::to_string(&g).unwrap();
        assert!(!text.contains("private"), "manager notes are never read");
    }

    /// M1: the membership list's `id` is the membership; the group is `groupId`.
    #[test]
    fn memberships_are_keyed_by_group_id() {
        let (card, owner) = parse_membership(&json!({"id": "gmem_1", "groupId": G, "name": "N", "ownerId": "usr_x"})).unwrap();
        assert_eq!(card.id, G);
        assert_eq!(owner, "usr_x");
        assert!(parse_membership(&json!({"id": G, "name": "no group id"})).is_none());
    }

    /// M8: the three shapes — an array page, a `{results,total}` search, a lookup that is `null` — and partial records.
    #[test]
    fn members_come_in_three_shapes() {
        let (mut c, seen) = mock_signed_in(vec![
            raw_response(200, &json!([{"id": "gmem_1", "userId": U, "roleIds": ["grol_a"], "mRoleIds": [], "user": {"displayName": "Nova\u{202E}", "iconUrl": "https://api.vrchat.cloud/i"}}, {"id": "gmem_2", "userId": "usr_00000000-0000-4000-8000-0000000000cc"}, {"id": "gmem_3", "userId": "not-a-user"}]).to_string()),
            raw_response(200, &json!({"results": [{"id": "gmem_1", "userId": U, "user": {"displayName": "Nova"}}], "total": 1}).to_string()),
            raw_response(200, "null"),
        ]);
        let page = c.group_members(G, 0).unwrap();
        assert_eq!(page.raw, 3, "offsets advance by what VRChat gave");
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].display_name.as_deref(), Some("Nova"));
        assert!(page.items[0].complete());
        assert!(!page.items[1].complete(), "no nested user: authorises nothing");
        let found = c.search_group_members(G, "Nov", 0).unwrap();
        assert_eq!((found.items.len(), found.total), (1, Some(1)));
        assert_eq!(c.group_member(G, U).unwrap(), None);
        assert_eq!(c.search_group_members(G, "No", 0), Err(Failure::Invalid), "three characters at least, nothing sent");
        let calls: Vec<String> = seen.lock().unwrap().iter().map(|r| format!("{} {}", r.method, r.url.trim_start_matches(BASE))).collect();
        assert_eq!(calls, [format!("GET /groups/{G}/members?n=50&offset=0&sort=joinedAt:desc"), format!("GET /groups/{G}/members/search?query=Nov&n=50&offset=0"), format!("GET /groups/{G}/members/{U}")]);
    }

    /// B1: every post write goes to the posts route; the legacy announcement routes are never asked.
    #[test]
    fn post_writes_use_only_the_posts_route() {
        let post = json!({"id": "gpos_1", "title": "T", "text": "x", "visibility": "group", "roleIds": [], "imageId": null});
        let (mut c, seen) = mock_signed_in(vec![raw_response(200, &post.to_string()), raw_response(200, &json!({"success": {"message": "ok", "status_code": 200}}).to_string())]);
        let draft = PostDraft { title: "T".into(), text: "x".into(), ..Default::default() };
        let made = c.create_post(G, &draft).unwrap();
        c.delete_post(G, &made.id).unwrap();
        let seen = seen.lock().unwrap();
        let calls: Vec<String> = seen.iter().map(|r| format!("{} {}", r.method, r.url.trim_start_matches(BASE))).collect();
        assert_eq!(calls, [format!("POST /groups/{G}/posts"), format!("DELETE /groups/{G}/posts/gpos_1")]);
        assert!(seen.iter().all(|r| !r.url.contains("announcement")));
        // M7: the creation asks for no notification unless turned on.
        let create: Value = serde_json::from_str(seen[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(create, json!({"title": "T", "text": "x", "visibility": "group", "sendNotification": false}));
        let body = |d: &PostDraft| -> Value { serde_json::from_str(&post_create_body(d)).unwrap() };
        let public = PostDraft { visibility: Visibility::Public, role_ids: vec!["grol_a".into()], send_notification: true, ..Default::default() };
        assert!(body(&public).get("roleIds").is_none(), "roles restrict only a group post");
        assert_eq!(body(&public)["sendNotification"], true);
        // verify3 Minor 2: sets go in one order, whatever the order they were chosen in.
        let chosen = PostDraft { role_ids: vec!["grol_b".into(), "grol_a".into(), "grol_b".into()], ..Default::default() };
        assert_eq!(body(&chosen)["roleIds"], json!(["grol_a", "grol_b"]));
        assert_eq!(role_update_body(&RoleChange { permissions: Some(vec!["b".into(), "a".into()]), ..Default::default() }), r#"{"permissions":["a","b"]}"#);
        assert!(role_create_body(&RoleDraft { permissions: vec!["b".into(), "a".into()], ..Default::default() }).contains(r#"["a","b"]"#));
    }

    /// B4: no body this module builds carries `nameplateId`; the group update carries one picture's id only.
    #[test]
    fn no_nameplate_id_is_ever_serialised() {
        let bodies = [
            art_body(ArtSlot::Banner, "file_1"),
            art_body(ArtSlot::Icon, "file_1"),
            role_create_body(&RoleDraft::default()),
            role_update_body(&RoleChange { name: Some("x".into()), ..Default::default() }),
            post_create_body(&PostDraft::default()),
        ];
        for b in &bodies {
            assert!(!b.contains("nameplate") && !b.contains("null"), "{b}");
        }
        assert_eq!(art_body(ArtSlot::Banner, "file_1"), r#"{"bannerId":"file_1"}"#);
        assert_eq!(art_body(ArtSlot::Icon, "file_2"), r#"{"iconId":"file_2"}"#);
        assert!(!role_create_body(&RoleDraft::default()).contains("requiresPurchase"));
        assert_eq!(role_update_body(&RoleChange { permissions: Some(vec![]), ..Default::default() }), r#"{"permissions":[]}"#);
        assert!(!role_update_body(&RoleChange { name: Some("x".into()), ..Default::default() }).contains("order"));
    }

    /// M5: 403 and 404 are told apart, and no server text is kept.
    #[test]
    fn forbidden_and_not_found_are_told_apart() {
        let (mut c, _) = mock_signed_in(vec![
            raw_response(403, r#"{"error":{"message":"You do not have permission: secret detail","status_code":403}}"#),
            raw_response(404, "{}"),
            raw_response(400, "{}"),
        ]);
        assert_eq!(c.group_roles(G), Err(Failure::Forbidden));
        assert_eq!(c.group_roles(G), Err(Failure::NotFound));
        assert_eq!(c.create_role(G, &RoleDraft::default()), Err(Failure::Invalid));
    }

    /// B2: a write's 5xx, unreadable answer or lost connection is "unknown", a read's is not.
    #[test]
    fn a_lost_write_answer_is_unknown() {
        let (mut c, _) = mock_signed_in(vec![raw_response(502, ""), raw_response(200, "not json"), raw_response(502, "")]);
        assert_eq!(c.create_post(G, &PostDraft::default()), Err(Failure::Unknown));
        assert_eq!(c.create_role(G, &RoleDraft::default()), Err(Failure::Unknown));
        assert_eq!(c.group_roles(G), Err(Failure::Unavailable));
    }

    /// Blocker 4: a role or member read without what the rules decide by is not taken for a harmless one.
    #[test]
    fn missing_fields_are_unknown_not_empty() {
        let full = parse_role(&role_json("grol_a", "Artists", 3, &[])).unwrap();
        assert!(full.known && full.whole && !full.privileged());
        // verify2 B-D: a right too long to carry is a right all the same: the role is not a harmless one.
        let long = parse_role(&role_json("grol_a", "Artists", 3, &[&"r".repeat(2000)])).unwrap();
        assert!(!long.known && long.privileged());
        let later = parse_role(&role_json("grol_a", "Artists", 3, &[&"r".repeat(200)])).unwrap();
        assert!(later.known && later.privileged(), "kept whole");
        let bare = parse_role(&json!({"id": "grol_a", "order": 3})).unwrap();
        assert!(!bare.known && bare.privileged(), "no permission list: unknown, so privileged");
        let odd = parse_role(&json!({"id": "grol_a", "order": 3, "permissions": [7], "defaultRole": false, "isManagementRole": false, "requiresPurchase": false})).unwrap();
        assert!(!odd.known);
        let m = parse_member(&json!({"id": "gmem_1", "userId": U})).unwrap();
        assert!(!m.roles_known, "absent role arrays are not an empty list");
        assert!(!parse_member(&json!({"id": "gmem_1", "userId": U, "roleIds": []})).unwrap().roles_known, "the schema requires both");
        let m = parse_member(&json!({"id": "gmem_1", "groupId": G, "userId": U, "roleIds": [], "mRoleIds": []})).unwrap();
        assert!(m.is_of(G, U) && !m.is_of(G, "usr_00000000-0000-4000-8000-0000000000cc") && !m.is_of("grp_other", U));
        // verify3 Blocker 1: a record that does not say which membership it is says nothing of one.
        assert!(!parse_member(&json!({"groupId": G, "userId": U, "roleIds": [], "mRoleIds": []})).unwrap().is_of(G, U));
        let m = parse_member(&json!({"id": "gmem_1", "groupId": G, "userId": U, "roleIds": [], "mRoleIds": [], "joinedAt": "2026-01-01T00:00:00Z"})).unwrap();
        assert_eq!(m.membership().unwrap().joined_at.as_deref(), Some("2026-01-01T00:00:00Z"));
        let me = json!({"id": "gmem_me", "userId": "usr_test", "roleIds": [], "mRoleIds": [], "permissions": [], "membershipStatus": "member"});
        let known = |v: &Value| parse_my_member(v).map(|m| (m.rights_known, m.roles_known, m.managed_known, m.membership().is_some())).unwrap();
        assert_eq!(known(&me), (true, true, true, true));
        for (gone, want) in [("permissions", (false, true, true, true)), ("roleIds", (true, false, true, true)), ("mRoleIds", (true, true, false, true)), ("id", (true, true, true, false))] {
            let mut v = me.clone();
            v.as_object_mut().unwrap().remove(gone);
            assert_eq!(known(&v), want, "without {gone}");
        }
        // The strict side of a flag left out: a management role, not one to take oneself; never one to edit against.
        let mut v = role_json("grol_a", "Artists", 3, &[]);
        v.as_object_mut().unwrap().remove("isManagementRole");
        v.as_object_mut().unwrap().remove("isSelfAssignable");
        let r = parse_role(&v).unwrap();
        assert!(r.known && !r.whole && r.management && r.privileged() && !r.self_assignable);
        for needed in ["permissions", "order", "defaultRole", "requiresPurchase"] {
            let mut v = role_json("grol_a", "Artists", 3, &[]);
            v.as_object_mut().unwrap().remove(needed);
            assert!(!parse_role(&v).unwrap().known, "without {needed}");
        }
        let g = parse_group(&json!({"id": G, "bannerId": null, "iconId": "https://elsewhere"})).unwrap();
        assert!(g.banner_known && g.banner_id.is_none() && !g.icon_known, "null is none; another form is not known");
        assert!(g.member_count.is_none(), "N1: not read is not zero");
    }

    /// Blocker 3: a post whose audience or content cannot be carried back whole is read-only here; its text is kept as
    /// VRChat has it (shown through the text policy).
    #[test]
    fn a_post_is_kept_exactly_or_marked_incomplete() {
        let p = parse_post(&json!({"id": "gpos_1", "title": "Hi\u{202E}", "text": "x", "visibility": "group", "roleIds": ["grol_a"], "imageId": null, "createdAt": "not a date"})).unwrap();
        for (gone, why) in [("roleIds", "no restriction given is not every member"), ("imageId", "no picture given is not no picture"), ("title", "no title given is not an empty one"), ("text", "no text given is not an empty one")] {
            let mut v = json!({"id": "gpos_1", "title": "t", "text": "x", "visibility": "group", "roleIds": [], "imageId": null});
            v.as_object_mut().unwrap().remove(gone);
            assert!(!parse_post(&v).unwrap().complete, "{why}");
        }
        assert!(p.complete);
        assert_eq!(p.title, "Hi\u{202E}", "the exact remote title, never the display copy");
        assert_eq!(p.created_at, None, "Minor 1: a date that does not parse is not shown as it is");
        assert!(!parse_post(&json!({"id": "gpos_1", "title": "t", "text": "x", "visibility": "friends"})).unwrap().complete, "an audience this version cannot carry");
        assert!(!parse_post(&json!({"id": "gpos_1", "title": "t", "text": "x", "visibility": "group", "roleIds": ["grol_a", "weird"]})).unwrap().complete, "a role id that is not one");
        let many: Vec<String> = (0..40).map(|i| format!("grol_{i}")).collect();
        assert!(!parse_post(&json!({"id": "gpos_1", "title": "t", "text": "x", "visibility": "group", "roleIds": many})).unwrap().complete);
    }

    /// Major 2, J2: an answer that does not match what was asked says nothing sure, for every endpoint.
    #[test]
    fn mismatched_or_malformed_write_answers_are_unknown() {
        let png = b"\x89PNG\r\n\x1a\nx".to_vec();
        let file = |id: &str, owner: &str, tag: &str| json!({"id": id, "ownerId": owner, "tags": [tag], "versions": [{"version": 1, "file": {"url": format!("https://api.vrchat.cloud/api/1/file/{id}/1/file"), "md5": file_md5(&png), "sizeInBytes": png.len()}}]});
        let with = |key: &str, v: Value| {
            let mut f = file("file_up", "usr_test", "gallery");
            f["versions"][0]["file"][key] = v;
            f
        };
        let post = |id: &str, title: &str, text: &str, roles: Value, image: Value| json!({"id": id, "groupId": G, "authorId": "usr_test", "title": title, "text": text, "visibility": "group", "roleIds": roles, "imageId": image});
        let draft = RoleDraft { name: "New".into(), ..Default::default() };
        let rename = RoleChange { name: Some("New".into()), ..Default::default() };
        let new = PostDraft { title: "t".into(), text: "x".into(), ..Default::default() };
        type Call<'a> = Box<dyn Fn(&mut Client) -> GroupResult<()> + 'a>;
        let art: Call = Box::new(|c| c.set_group_art(G, ArtSlot::Banner, "file_new").map(drop));
        let give: Call = Box::new(|c| c.set_member_role(G, U, "grol_a", true).map(drop));
        let take: Call = Box::new(|c| c.set_member_role(G, U, "grol_a", false).map(drop));
        let make: Call = Box::new(|c| c.create_role(G, &draft).map(drop));
        let update: Call = Box::new(|c| c.update_role(G, "grol_a", &rename).map(drop));
        let drop_role: Call = Box::new(|c| c.delete_role(G, "grol_a").map(drop));
        let publish: Call = Box::new(|c| c.create_post(G, &new).map(drop));
        let remove: Call = Box::new(|c| c.delete_post(G, "gpos_1"));
        let upload: Call = Box::new(|c| c.upload_image(FileTag::Gallery, png.clone()).map(drop));
        let new_role = || role_json("grol_n", "New", 4, &[]);
        let owner = || role_json("grol_owner", "Owner", 0, &["*"]);
        let mut other_group = new_role();
        other_group["groupId"] = json!("grp_other");
        // (what, the call, an answer that matches, answers that do not)
        let table: Vec<(&str, &Call, Value, Vec<Value>)> = vec![
            ("apply", &art, json!({"id": G, "bannerId": "file_new"}), vec![json!({"id": "grp_00000000-0000-4000-8000-0000000000ff", "bannerId": "file_new"}), json!({"id": G, "bannerId": "file_old", "iconId": "file_new"}), json!({"id": G})]),
            ("give", &give, json!(["grol_a"]), vec![json!(["grol_b"]), json!([]), json!(["grol_a", 5]), json!({})]),
            ("take back", &take, json!(["grol_b"]), vec![json!(["grol_a", "grol_b"]), json!(null)]),
            ("new role", &make, new_role(), vec![role_json("grol_n", "Another", 4, &[]), role_json("grol_n", "New", 4, &["group-data-manage"]), other_group, json!({"id": "grol_n", "name": "New"})]),
            ("edit role", &update, json!([role_json("grol_a", "New", 3, &[])]), vec![json!([role_json("grol_a", "Old", 3, &[])]), json!([role_json("grol_b", "New", 3, &[])]), json!([]), json!([{"id": "grol_a", "name": "New"}])]),
            ("delete role", &drop_role, json!([owner(), role_json("grol_b", "B", 3, &[])]), vec![json!([owner(), role_json("grol_a", "A", 3, &[])]), json!({}), json!([]), json!([role_json("grol_b", "B", 3, &[])])]),
            ("new post", &publish, post("gpos_1", "t", "x", json!([]), json!(null)), vec![post("gpos_1", "t", "another", json!([]), json!(null)), post("gpos_1", "t", "x", json!(["grol_a"]), json!(null)), json!({"id": "gpos_1", "authorId": "usr_test", "title": "t", "text": "x"})]),
            ("delete post", &remove, json!({"success": {"message": "Group Post was deleted!", "status_code": 200}}), vec![json!({}), json!({"success": {}}), json!({"success": {"status_code": 500}})]),
            ("upload", &upload, file("file_up", "usr_test", "gallery"), vec![file("file_up", "usr_other", "gallery"), file("file_up", "usr_test", "icon"), json!({"id": "file_up"}), with("sizeInBytes", json!(7)), with("md5", json!("AAAAAAAAAAAAAAAAAAAAAA=="))]),
        ];
        for (what, call, right, wrong) in table {
            let (mut c, _) = mock_signed_in(vec![raw_response(200, &right.to_string())]);
            assert_eq!(call(&mut c), Ok(()), "{what}");
            for w in wrong {
                let (mut c, _) = mock_signed_in(vec![raw_response(200, &w.to_string())]);
                assert_eq!(call(&mut c), Err(Failure::Unknown), "{what}: {w}");
            }
        }
    }

    /// Major 2: VRChat deleting the session cookie in its answer to a write that got a 5xx: unknown, not "signed out".
    #[test]
    fn a_write_answered_by_a_dropped_session_is_unknown() {
        let (mut c, _) = mock_signed_in(vec![super::super::tests::raw_response_with_cookie(502, "", "auth=; Max-Age=0")]);
        assert_eq!(c.create_post(G, &PostDraft { title: "t".into(), text: "x".into(), ..Default::default() }), Err(Failure::Unknown));
    }

    #[test]
    fn ids_are_checked_before_any_path_is_made() {
        let (mut c, seen) = mock_signed_in(vec![]);
        assert_eq!(c.group("grp_../users"), Err(Failure::NotFound));
        assert_eq!(c.set_member_role(G, "usr_x/../y", "grol_a", true), Err(Failure::NotFound));
        assert_eq!(c.set_group_art(G, ArtSlot::Banner, "https://elsewhere"), Err(Failure::NotFound));
        assert_eq!(c.upload_image(FileTag::Gallery, b"GIF89a".to_vec()), Err(Failure::Invalid));
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn discovery_reads_permissions_and_memberships() {
        let (mut c, seen) = mock_signed_in(vec![
            raw_response(200, &json!({G: ["group-instance-moderate"], "grp_bad/x": ["*"], "grp_00000000-0000-4000-8000-0000000000ab": []}).to_string()),
            raw_response(200, &json!([{"id": "gmem_1", "groupId": G, "name": "A", "ownerId": "usr_o"}]).to_string()),
        ]);
        let perms = c.my_group_permissions().unwrap();
        assert_eq!(perms.len(), 2, "ids that are not group ids are dropped");
        assert_eq!(c.my_groups().unwrap()[0].0.id, G);
        let calls: Vec<String> = seen.lock().unwrap().iter().map(|r| format!("{} {}", r.method, r.url.trim_start_matches(BASE))).collect();
        assert_eq!(calls, ["GET /users/usr_test/groups/permissions", "GET /users/usr_test/groups"]);
    }

    #[test]
    fn member_roles_are_set_without_a_body_and_the_answer_is_the_committed_state() {
        let (mut c, seen) = mock_signed_in(vec![raw_response(200, r#"["grol_a","grol_b"]"#), raw_response(200, r#"["grol_a"]"#)]);
        assert_eq!(c.set_member_role(G, U, "grol_b", true).unwrap(), ["grol_a", "grol_b"]);
        assert_eq!(c.set_member_role(G, U, "grol_b", false).unwrap(), ["grol_a"]);
        let seen = seen.lock().unwrap();
        assert_eq!((seen[0].method, seen[0].body.is_none()), ("PUT", true));
        assert_eq!((seen[1].method, seen[1].body.is_none()), ("DELETE", true));
        assert!(seen[0].url.ends_with(&format!("/groups/{G}/members/{U}/roles/grol_b")));
    }

    #[test]
    fn inventory_and_upload_read_file_ids() {
        let file = json!({"id": "file_new", "ownerId": "usr_test", "tags": ["icon", "gallery"], "versions": [{"version": 0}, {"version": 1, "file": {"url": "https://api.vrchat.cloud/api/1/file/file_new/1/file"}}]});
        let foreign = json!({"id": "file_bad", "versions": [{"version": 1, "file": {"url": "https://elsewhere.example/x"}}]});
        let (mut c, seen) = mock_signed_in(vec![raw_response(200, &json!([file, foreign]).to_string()), raw_response(200, &file.to_string()), raw_response(200, &file.to_string()), raw_response(200, &foreign.to_string())]);
        let page = c.inventory(FileTag::Gallery, 0).unwrap();
        assert_eq!(page.raw, 2, "offsets advance by what VRChat gave, the unusable entry included (Minor 5)");
        let files = page.items;
        assert_eq!(files, [InventoryFile { id: "file_new".into(), url: "https://api.vrchat.cloud/api/1/file/file_new/1/file".into() }]);
        let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
        assert_eq!(c.upload_image(FileTag::Icon, png.clone()).unwrap().id, "file_new");
        let one = c.picture_file("file_new").unwrap();
        assert_eq!((one.owner.as_str(), one.version, one.tags.map(|t| t.len())), ("usr_test", Some(1), Some(2)));
        assert_eq!(c.picture_file("file_new"), Err(Failure::BadAnswer), "another file's answer");
        let seen = seen.lock().unwrap();
        assert!(seen[0].url.ends_with("/files?tag=gallery&n=50&offset=0"));
        let form = seen[1].form.as_ref().expect("a multipart form");
        assert_eq!(form.fields, [("tag", "icon".to_string())]);
        assert_eq!(form.bytes, png);
        assert!(seen[2].method == "GET" && seen[2].url.ends_with("/file/file_new"));
    }

    /// M5: the real HTTP adapter against a loopback server — DELETE, multipart, a PUT without a body, 403 apart from 404,
    /// 429 with Retry-After, a connection dropped after the request (commit-then-drop) and a timeout.
    mod loopback {
        use super::*;
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;

        pub(super) enum Script {
            Answer(u16, &'static str, &'static [(&'static str, &'static str)]),
            /// Read the request whole, then close without a word.
            Drop,
            /// Wait this long before answering 200 (past the client's timeout).
            Stall(u64),
        }
        #[derive(Debug, Clone)]
        pub(super) struct Seen {
            pub head: String,
            pub body: Vec<u8>,
        }
        /// Rewrites VRChat's origin to the loopback server, then sends through the real adapter ([`Http`]).
        struct Loopback {
            http: Http,
            origin: String,
        }
        impl Transport for Loopback {
            fn send(&self, mut r: Request) -> Result<Response> {
                r.url = r.url.replacen("https://api.vrchat.cloud", &self.origin, 1);
                self.http.send(r)
            }
        }
        pub(super) fn serve(script: Vec<Script>) -> (Client, std::sync::mpsc::Receiver<Seen>) {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for step in script {
                    let Ok((stream, _)) = listener.accept() else { return };
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        head.push_str(&line);
                    }
                    let length = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let _ = tx.send(Seen { head, body });
                    let mut stream = stream;
                    match step {
                        Script::Answer(status, text, headers) => {
                            let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
                            let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n{text}", text.len());
                        }
                        Script::Drop => drop(stream),
                        Script::Stall(ms) => {
                            std::thread::sleep(Duration::from_millis(ms));
                            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]");
                        }
                    }
                }
            });
            let agent = ureq::Agent::config_builder().http_status_as_error(false).max_redirects(0).timeout_global(Some(Duration::from_millis(700))).build().new_agent();
            let (mut c, _) = mock_signed_in(vec![]);
            c.transport = Arc::new(Loopback { http: Http(agent), origin: format!("http://127.0.0.1:{port}") });
            c.cookies.auth = "loop-session".into();
            (c, rx)
        }
        fn next(rx: &std::sync::mpsc::Receiver<Seen>) -> Seen {
            rx.recv_timeout(Duration::from_secs(5)).expect("the server saw the request")
        }

        #[test]
        fn delete_and_a_bodyless_put_go_with_the_session_cookie() {
            let (mut c, rx) = serve(vec![Script::Answer(200, r#"[{"id":"grol_o","order":0,"permissions":["*"],"defaultRole":false,"requiresPurchase":false}]"#, &[]), Script::Answer(200, r#"["grol_a"]"#, &[])]);
            assert_eq!(c.delete_role(G, "grol_a").map(|r| r.len()), Ok(1));
            let seen = next(&rx);
            assert!(seen.head.starts_with(&format!("DELETE /api/1/groups/{G}/roles/grol_a HTTP/1.1")), "{}", seen.head);
            assert!(seen.head.to_ascii_lowercase().contains("cookie: auth=loop-session"));
            assert_eq!(c.set_member_role(G, U, "grol_a", true), Ok(vec!["grol_a".to_string()]));
            let seen = next(&rx);
            assert!(seen.head.starts_with(&format!("PUT /api/1/groups/{G}/members/{U}/roles/grol_a ")), "{}", seen.head);
            assert!(seen.body.is_empty(), "no body");
            assert!(!seen.head.to_ascii_lowercase().contains("content-type: application/json"));
        }

        #[test]
        fn a_picture_goes_as_a_multipart_form_with_the_cookie() {
            let file = r#"{"id":"file_up","ownerId":"usr_test","tags":["gallery"],"versions":[{"version":1,"file":{"url":"https://api.vrchat.cloud/api/1/file/file_up/1/file"}}]}"#;
            let (mut c, rx) = serve(vec![Script::Answer(200, file, &[])]);
            let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
            png.extend((0..=255u8).cycle().take(4000));
            assert_eq!(c.upload_image(FileTag::Gallery, png.clone()).map(|f| f.id), Ok("file_up".into()));
            let seen = next(&rx);
            assert!(seen.head.starts_with("POST /api/1/file/image "), "{}", seen.head);
            let lower = seen.head.to_ascii_lowercase();
            assert!(lower.contains("cookie: auth=loop-session"));
            let boundary = seen.head.lines().find_map(|l| l.split_once("boundary=").map(|(_, b)| b.trim().to_string())).expect("a multipart boundary");
            let body = seen.body;
            let text = String::from_utf8_lossy(&body);
            assert!(text.contains(&format!("--{boundary}\r\nContent-Disposition: form-data; name=\"tag\"\r\n\r\ngallery\r\n")));
            assert!(text.contains("name=\"file\"; filename=\"orbiters.png\"\r\nContent-Type: image/png"));
            assert!(body.windows(png.len()).any(|w| w == png.as_slice()), "the PNG's bytes whole");
            assert!(text.ends_with(&format!("--{boundary}--\r\n")));
        }

        #[test]
        fn forbidden_not_found_and_rate_limits_through_the_adapter() {
            let (mut c, rx) = serve(vec![
                Script::Answer(403, r#"{"error":{"message":"secret","status_code":403}}"#, &[]),
                Script::Answer(404, "{}", &[]),
                Script::Answer(429, "{}", &[("Retry-After", "7")]),
            ]);
            assert_eq!(c.group_roles(G), Err(Failure::Forbidden));
            assert_eq!(c.group_roles(G), Err(Failure::NotFound));
            // Major 2: a post that went out and came back 429 is unknown (the server may have taken it); the back-off holds.
            assert_eq!(c.create_post(G, &PostDraft::default()), Err(Failure::Unknown), "a returned 429 to a sent write proves nothing");
            for _ in 0..3 {
                next(&rx);
            }
            // The shared back-off holds: the next request is not sent at all, and that one is a plain refusal.
            assert!(matches!(c.group_roles(G), Err(Failure::RateLimited(s)) if s <= 7));
            assert!(matches!(c.create_post(G, &PostDraft::default()), Err(Failure::RateLimited(_))), "refused before it was sent");
            assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "nothing sent during the back-off");
        }

        /// B2: VRChat took the request and the connection died before its answer: "unknown", never "not sent".
        #[test]
        fn commit_then_drop_and_a_timeout_are_unknown() {
            let (mut c, rx) = serve(vec![Script::Drop, Script::Stall(2000), Script::Drop]);
            assert_eq!(c.create_post(G, &PostDraft { title: "t".into(), text: "x".into(), ..Default::default() }), Err(Failure::Unknown));
            assert!(next(&rx).head.starts_with(&format!("POST /api/1/groups/{G}/posts")));
            assert_eq!(c.set_member_role(G, U, "grol_a", false), Err(Failure::Unknown), "a timeout after sending");
            next(&rx);
            assert_eq!(c.group_roles(G), Err(Failure::Unavailable), "a read that drops is just unavailable");
        }
    }
}
