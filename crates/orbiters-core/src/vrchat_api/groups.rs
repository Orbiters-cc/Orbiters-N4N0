//! VRChat's group calls (REQUEST22 1), against the attack's wire contract (attack22-groups.md, "Wire contracts"): paths
//! and bodies from the community OpenAPI spec at 68091ebc (`getUserAllGroupPermissions`, `getUserGroups`, `getGroup`
//! with `includeRoles`, `getGroupPermissions`, `getGroupMembers`, `searchGroupMembers`, `getGroupMember`,
//! `addGroupMemberRole` / `removeGroupMemberRole` (no body), `createGroupRole`, `updateGroupRole`, `deleteGroupRole`,
//! `getGroupPosts`, `addGroupPost`, `updateGroupPost`, `deleteGroupPost`, `getFiles`, `uploadImage` (multipart),
//! `updateGroup`), and VRCX at e819c4e4 (`src/api/group.ts`: the post id in the post path, no `sendNotification` on an
//! edit).
//!
//! What is never sent: the legacy announcement routes (`G/announcement`, whose POST replaces every announcement: B1) and
//! `nameplateId` (B4). Every write goes in the [`WRITES`] family, whose lost answers are "outcome unknown" (B2), through
//! the client's pacing and shared 429 back-off. Errors are local codes ([`Failure`]): no server text reaches a notice.
use super::*;
use crate::groups::{self as g, ArtSlot, FileTag, Group, GroupCard, InventoryFile, Member, MyMember, PermissionInfo, Post, PostDraft, Role, RoleChange, RoleDraft, Visibility};
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
    let rights = v.get("permissions").and_then(Value::as_array);
    let known = rights.is_some_and(|a| a.len() <= 128 && a.iter().all(Value::is_string))
        && v.get("order").and_then(Value::as_i64).is_some()
        && ["defaultRole", "isManagementRole", "requiresPurchase"].iter().all(|k| v.get(*k).is_some_and(Value::is_boolean));
    Some(Role {
        // As VRChat has them (an edit is compared with and sent against these); shown through `groups::label`.
        name: s(v, "name").chars().take(256).collect(),
        description: s(v, "description").chars().take(2048).collect(),
        order: v.get("order").and_then(Value::as_i64),
        // Every right VRChat gives is kept as it is, those this version does not know included (never sent back unasked).
        permissions: strings(v, "permissions", 128),
        known,
        default_role: flag("defaultRole"),
        management: flag("isManagementRole"),
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
    let roles: Vec<Role> = list.iter().filter_map(parse_role).take(200).collect();
    // A role VRChat gave that cannot be read makes the list incomplete: then it is not a list to rank against.
    (roles.len() == list.len().min(200)).then_some(roles)
}

fn parse_my_member(v: &Value) -> Option<MyMember> {
    if !v.is_object() {
        return None;
    }
    let mut role_ids = strings(v, "roleIds", 64);
    for m in strings(v, "mRoleIds", 64) {
        if !role_ids.contains(&m) {
            role_ids.push(m);
        }
    }
    Some(MyMember { user_id: s(v, "userId"), role_ids, permissions: strings(v, "permissions", 128), status: s(v, "membershipStatus") })
}

pub(crate) fn parse_group(v: &Value) -> Option<Group> {
    let id = s(v, "id");
    if !g::is_group_id(&id) {
        return None;
    }
    Some(Group {
        name: g::label(&s(v, "name"), 64),
        short_code: g::label(&s(v, "shortCode"), 12),
        discriminator: g::label(&s(v, "discriminator"), 8),
        description: g::text(&s(v, "description"), 1000),
        rules: g::text(&s(v, "rules"), 4000),
        links: strings(v, "links", 3).iter().filter_map(|l| g::link(l)).collect(),
        icon_id: file_id(v, "iconId"),
        icon_url: https(v, "iconUrl"),
        banner_id: file_id(v, "bannerId"),
        banner_url: https(v, "bannerUrl"),
        member_count: count(v, "memberCount").unwrap_or(0),
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
    // Absent role arrays are unknown roles, not "no role" (Blocker 4).
    let roles_known = v.get("roleIds").and_then(Value::as_array).is_some_and(|a| a.len() <= 64 && a.iter().all(Value::is_string))
        && v.get("mRoleIds").is_none_or(|m| m.as_array().is_some_and(|a| a.len() <= 64 && a.iter().all(Value::is_string)));
    let mut role_ids = strings(v, "roleIds", 64);
    for m in strings(v, "mRoleIds", 64) {
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
    // The post exactly as VRChat has it: what an edit keeps and is compared against (Blocker 3). Shown through
    // `groups::label`/`groups::text`. Whatever this version cannot carry back whole (an unknown audience, a role id that
    // is not one, too many roles, an over-long text, an image id of another form) makes it read-only here.
    let (title, text) = (s(v, "title"), s(v, "text"));
    let visibility = match v.get("visibility").and_then(Value::as_str) {
        Some("public") => Some(Visibility::Public),
        Some("group") => Some(Visibility::Group),
        _ => None,
    };
    let roles = v.get("roleIds");
    let role_ids: Vec<String> = roles.and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    let roles_whole = match roles {
        None | Some(Value::Null) => true,
        Some(Value::Array(a)) => a.len() <= 32 && a.iter().all(|x| x.as_str().is_some_and(g::is_role_id)),
        _ => false,
    };
    let image = v.get("imageId");
    let image_whole = image.is_none_or(|i| i.is_null() || i.as_str().is_some_and(g::is_file_id));
    let complete = visibility.is_some()
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
    (g::is_file_id(&id) && url.starts_with("https://api.vrchat.cloud/api/1/file/")).then_some(InventoryFile { id, url })
}

// --- Bodies (pure: tested on their own) ---

/// `createGroupRole`: name, description, rights and the three flags. `requiresPurchase` is never sent.
pub fn role_create_body(draft: &RoleDraft) -> String {
    json!({
        "name": draft.name.trim(),
        "description": draft.description,
        "permissions": draft.permissions,
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
        m.insert("permissions".into(), json!(v));
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
    if draft.visibility == Visibility::Group && !draft.role_ids.is_empty() {
        m.insert("roleIds".into(), json!(draft.role_ids));
    }
    if let Some(image) = &draft.image_id {
        m.insert("imageId".into(), json!(image));
    }
    Value::Object(m).to_string()
}

/// `updateGroupPost`: title, text, visibility and its roles as edited (they start as the post's own, so they are kept
/// unless changed), its picture kept unless removed, and never `sendNotification`: an edit notifies nobody again (M7,
/// as VRCX does).
pub fn post_edit_body(edit: &PostDraft, baseline: &Post) -> String {
    let mut m = Map::new();
    m.insert("title".into(), json!(edit.title.trim()));
    m.insert("text".into(), json!(edit.text.trim()));
    m.insert("visibility".into(), json!(edit.visibility.wire()));
    let roles: &[String] = if edit.visibility == Visibility::Group { &edit.role_ids } else { &[] };
    m.insert("roleIds".into(), json!(roles));
    match (&edit.image_id, &baseline.image_id) {
        (Some(id), _) => {
            m.insert("imageId".into(), json!(id));
        }
        // Removed by the person.
        (None, Some(_)) => {
            m.insert("imageId".into(), Value::Null);
        }
        (None, None) => {}
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
        let family = if write { WRITES } else if path.starts_with("/files") { "files" } else { READS };
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

    /// `GET /groups/{g}/permissions`: the rights a role can hold, and which this account may give.
    pub fn group_permission_catalogue(&mut self, group: &str) -> GroupResult<Vec<PermissionInfo>> {
        let path = Self::group_path(group)?;
        let v = self.group_read(&format!("{path}/permissions"))?;
        let list = v.as_array().ok_or(Failure::BadAnswer)?;
        Ok(list
            .iter()
            .filter_map(|p| {
                let name = s(p, "name");
                (!name.is_empty() && name.len() <= 128).then(|| PermissionInfo {
                    display_name: g::label(&s(p, "displayName"), 64),
                    help: g::text(&s(p, "help"), 300),
                    allowed_to_add: p.get("allowedToAdd").and_then(Value::as_bool).unwrap_or(false),
                    depends_on: strings(p, "dependsOn", 16),
                    management: p.get("isManagementPermission").and_then(Value::as_bool).unwrap_or(false),
                    name,
                })
            })
            .take(128)
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
        Ok(ids.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
    }

    /// `POST G/roles`: the role made.
    pub fn create_role(&mut self, group: &str, draft: &RoleDraft) -> GroupResult<Role> {
        let path = Self::group_path(group)?;
        let v = self.group_write("POST", &format!("{path}/roles"), Some(role_create_body(draft)))?;
        parse_role(&v).ok_or(Failure::Unknown)
    }

    /// `PUT G/roles/{r}` with what changed: every role as VRChat now has them.
    pub fn update_role(&mut self, group: &str, role: &str, change: &RoleChange) -> GroupResult<Vec<Role>> {
        let path = Self::group_path(group)?;
        if !g::is_role_id(role) {
            return Err(Failure::NotFound);
        }
        let v = self.group_write("PUT", &format!("{path}/roles/{}", encode(role)), Some(role_update_body(change)))?;
        parse_roles(&v).ok_or(Failure::Unknown)
    }

    /// `DELETE G/roles/{r}`: the roles left.
    pub fn delete_role(&mut self, group: &str, role: &str) -> GroupResult<Vec<Role>> {
        let path = Self::group_path(group)?;
        if !g::is_role_id(role) {
            return Err(Failure::NotFound);
        }
        let v = self.group_write("DELETE", &format!("{path}/roles/{}", encode(role)), None)?;
        parse_roles(&v).ok_or(Failure::Unknown)
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
        // The post made, of this group (Major 2): another answer says nothing sure.
        let post = parse_post(&v).ok_or(Failure::Unknown)?;
        if v.get("groupId").and_then(Value::as_str).is_some_and(|g| g != group) || post.title != draft.title.trim() {
            return Err(Failure::Unknown);
        }
        Ok(post)
    }

    /// `PUT G/posts/{post}`: the post as edited.
    pub fn edit_post(&mut self, group: &str, baseline: &Post, edit: &PostDraft) -> GroupResult<Post> {
        let path = Self::group_path(group)?;
        if !g::is_post_id(&baseline.id) {
            return Err(Failure::NotFound);
        }
        let v = self.group_write("PUT", &format!("{path}/posts/{}", encode(&baseline.id)), Some(post_edit_body(edit, baseline)))?;
        let post = parse_post(&v).ok_or(Failure::Unknown)?;
        if post.id != baseline.id || post.title != edit.title.trim() || post.text != edit.text.trim() {
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
        // VRChat's success envelope (`{"success": …}`): anything else says nothing sure (Major 2).
        match body_json(&answer) {
            Some(v) if v.get("success").is_some_and(Value::is_object) => Ok(()),
            _ => Err(Failure::Unknown),
        }
    }

    /// `GET /files?tag=gallery|icon&n&offset`: a page of the account's own pictures, and how many VRChat gave (offsets
    /// advance by that, Minor 5).
    pub fn inventory(&mut self, tag: FileTag, offset: usize) -> GroupResult<Inventory> {
        let v = self.group_read(&format!("/files?tag={}&n={PAGE}&offset={offset}", tag.wire()))?;
        let list = v.as_array().ok_or(Failure::BadAnswer)?;
        Ok(Inventory { raw: list.len(), items: list.iter().take(100).filter_map(parse_file).collect() })
    }

    /// `POST /file/image` (multipart: `tag`, `file`): a PNG into the account's own gallery or icons, the file made.
    /// Its own step: nothing about a group changes here (M6).
    pub fn upload_image(&mut self, tag: FileTag, png: Vec<u8>) -> GroupResult<InventoryFile> {
        if png.len() > UPLOAD_MAX || !png.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Err(Failure::Invalid);
        }
        self.authenticated().map_err(|e| Failure::of(e, false))?;
        let form = Form { fields: vec![("tag", tag.wire().to_string())], file_name: "orbiters.png", mime: "image/png", bytes: png };
        let answer = self.group_call("POST", "/file/image", None, Some(form))?;
        body_json(&answer).as_ref().and_then(parse_file).ok_or(Failure::Unknown)
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
        let now = match slot {
            ArtSlot::Banner => found.banner_id.as_deref(),
            ArtSlot::Icon => found.icon_id.as_deref(),
        };
        if found.id != group || now != Some(file) {
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
            "roles": [{"id": "grol_owner", "name": "Owner", "order": 0, "permissions": ["*"]}, {"id": "grol_a", "name": "Artists", "order": 3, "permissions": ["group-some-future-right"]}]})
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
            raw_response(200, &json!([{"id": "gmem_1", "userId": U, "roleIds": ["grol_a"], "user": {"displayName": "Nova\u{202E}", "iconUrl": "https://api.vrchat.cloud/i"}}, {"id": "gmem_2", "userId": "usr_00000000-0000-4000-8000-0000000000cc"}, {"id": "gmem_3", "userId": "not-a-user"}]).to_string()),
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
        let post = json!({"id": "gpos_1", "title": "T", "text": "x", "visibility": "group", "roleIds": [], "imageId": "file_img"});
        let edited = json!({"id": "gpos_1", "title": "T2", "text": "x", "visibility": "group", "roleIds": [], "imageId": "file_img"});
        let (mut c, seen) = mock_signed_in(vec![raw_response(200, &post.to_string()), raw_response(200, &edited.to_string()), raw_response(200, &json!({"success": {"message": "ok"}}).to_string())]);
        let draft = PostDraft { title: "T".into(), text: "x".into(), ..Default::default() };
        let made = c.create_post(G, &draft).unwrap();
        c.edit_post(G, &made, &PostDraft { title: "T2".into(), image_id: made.image_id.clone(), ..draft.clone() }).unwrap();
        c.delete_post(G, &made.id).unwrap();
        let seen = seen.lock().unwrap();
        let calls: Vec<String> = seen.iter().map(|r| format!("{} {}", r.method, r.url.trim_start_matches(BASE))).collect();
        assert_eq!(calls, [format!("POST /groups/{G}/posts"), format!("PUT /groups/{G}/posts/gpos_1"), format!("DELETE /groups/{G}/posts/gpos_1")]);
        assert!(seen.iter().all(|r| !r.url.contains("announcement")));
        // M7: the creation asks for no notification unless turned on; the edit never carries one and keeps the picture.
        let create: Value = serde_json::from_str(seen[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(create["sendNotification"], false);
        let edit: Value = serde_json::from_str(seen[1].body.as_deref().unwrap()).unwrap();
        assert!(edit.get("sendNotification").is_none(), "an edit never notifies again: {edit}");
        assert_eq!(edit["imageId"], "file_img", "the picture is kept unless removed");
    }

    #[test]
    fn post_edits_keep_restrictions_and_only_clear_a_picture_removed() {
        let base = Post { id: "gpos_1".into(), role_ids: vec!["grol_a".into()], image_id: Some("file_x".into()), ..Default::default() };
        let keep = PostDraft { title: "a".into(), text: "b".into(), role_ids: base.role_ids.clone(), image_id: base.image_id.clone(), ..Default::default() };
        let v: Value = serde_json::from_str(&post_edit_body(&keep, &base)).unwrap();
        assert_eq!(v["roleIds"], json!(["grol_a"]));
        assert_eq!(v["imageId"], "file_x");
        let removed: Value = serde_json::from_str(&post_edit_body(&PostDraft { image_id: None, ..keep.clone() }, &base)).unwrap();
        assert!(removed["imageId"].is_null() && removed.get("imageId").is_some());
        let none: Value = serde_json::from_str(&post_edit_body(&PostDraft { image_id: None, ..keep }, &Post { image_id: None, ..base })).unwrap();
        assert!(none.get("imageId").is_none(), "no picture before, none asked: nothing sent about it");
        let public = PostDraft { visibility: Visibility::Public, role_ids: vec!["grol_a".into()], send_notification: true, ..Default::default() };
        let v: Value = serde_json::from_str(&post_create_body(&public)).unwrap();
        assert!(v.get("roleIds").is_none(), "roles restrict only a group post");
        assert_eq!(v["sendNotification"], true);
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
            post_edit_body(&PostDraft::default(), &Post::default()),
        ];
        for b in &bodies {
            assert!(!b.contains("nameplate"), "{b}");
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
        let full = parse_role(&json!({"id": "grol_a", "order": 3, "permissions": [], "defaultRole": false, "isManagementRole": false, "requiresPurchase": false})).unwrap();
        assert!(full.known && !full.privileged());
        let bare = parse_role(&json!({"id": "grol_a", "order": 3})).unwrap();
        assert!(!bare.known && bare.privileged(), "no permission list: unknown, so privileged");
        let odd = parse_role(&json!({"id": "grol_a", "order": 3, "permissions": [7], "defaultRole": false, "isManagementRole": false, "requiresPurchase": false})).unwrap();
        assert!(!odd.known);
        let m = parse_member(&json!({"id": "gmem_1", "userId": U})).unwrap();
        assert!(!m.roles_known, "absent role arrays are not an empty list");
        assert!(parse_member(&json!({"id": "gmem_1", "userId": U, "roleIds": []})).unwrap().roles_known);
    }

    /// Blocker 3: a post whose audience or content cannot be carried back whole is read-only here; its text is kept as
    /// VRChat has it (shown through the text policy).
    #[test]
    fn a_post_is_kept_exactly_or_marked_incomplete() {
        let p = parse_post(&json!({"id": "gpos_1", "title": "Hi\u{202E}", "text": "x", "visibility": "group", "roleIds": ["grol_a"], "createdAt": "not a date"})).unwrap();
        assert!(p.complete);
        assert_eq!(p.title, "Hi\u{202E}", "the exact remote title, never the display copy");
        assert_eq!(p.created_at, None, "Minor 1: a date that does not parse is not shown as it is");
        assert!(!parse_post(&json!({"id": "gpos_1", "title": "t", "text": "x", "visibility": "friends"})).unwrap().complete, "an audience this version cannot carry");
        assert!(!parse_post(&json!({"id": "gpos_1", "title": "t", "text": "x", "visibility": "group", "roleIds": ["grol_a", "weird"]})).unwrap().complete, "a role id that is not one");
        let many: Vec<String> = (0..40).map(|i| format!("grol_{i}")).collect();
        assert!(!parse_post(&json!({"id": "gpos_1", "title": "t", "text": "x", "visibility": "group", "roleIds": many})).unwrap().complete);
    }

    /// Major 2: an answer that does not match what was asked says nothing sure.
    #[test]
    fn mismatched_or_malformed_write_answers_are_unknown() {
        let other = json!({"id": "grp_00000000-0000-4000-8000-0000000000ff", "bannerId": "file_new"});
        let wrong_slot = json!({"id": G, "bannerId": "file_old", "iconId": "file_new"});
        let (mut c, _) = mock_signed_in(vec![
            raw_response(200, &other.to_string()),
            raw_response(200, &wrong_slot.to_string()),
            raw_response(200, r#"["grol_a", 5]"#),
            raw_response(200, "{}"),
        ]);
        assert_eq!(c.set_group_art(G, ArtSlot::Banner, "file_new"), Err(Failure::Unknown), "another group's answer");
        assert_eq!(c.set_group_art(G, ArtSlot::Banner, "file_new"), Err(Failure::Unknown), "the banner is not the file");
        assert_eq!(c.set_member_role(G, U, "grol_a", true), Err(Failure::Unknown), "a malformed role list");
        assert_eq!(c.delete_post(G, "gpos_1"), Err(Failure::Unknown), "no success envelope");
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
        let file = json!({"id": "file_new", "versions": [{"version": 0}, {"version": 1, "file": {"url": "https://api.vrchat.cloud/api/1/file/file_new/1/file"}}]});
        let foreign = json!({"id": "file_bad", "versions": [{"version": 1, "file": {"url": "https://elsewhere.example/x"}}]});
        let (mut c, seen) = mock_signed_in(vec![raw_response(200, &json!([file, foreign]).to_string()), raw_response(200, &file.to_string())]);
        let page = c.inventory(FileTag::Gallery, 0).unwrap();
        assert_eq!(page.raw, 2, "offsets advance by what VRChat gave, the unusable entry included (Minor 5)");
        let files = page.items;
        assert_eq!(files, [InventoryFile { id: "file_new".into(), url: "https://api.vrchat.cloud/api/1/file/file_new/1/file".into() }]);
        let png = b"\x89PNG\r\n\x1a\nrest".to_vec();
        assert_eq!(c.upload_image(FileTag::Icon, png.clone()).unwrap().id, "file_new");
        let seen = seen.lock().unwrap();
        assert!(seen[0].url.ends_with("/files?tag=gallery&n=50&offset=0"));
        let form = seen[1].form.as_ref().expect("a multipart form");
        assert_eq!(form.fields, [("tag", "icon".to_string())]);
        assert_eq!(form.bytes, png);
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
            let (mut c, rx) = serve(vec![Script::Answer(200, "[]", &[]), Script::Answer(200, r#"["grol_a"]"#, &[])]);
            assert_eq!(c.delete_role(G, "grol_a"), Ok(vec![]));
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
            let file = r#"{"id":"file_up","versions":[{"version":1,"file":{"url":"https://api.vrchat.cloud/api/1/file/file_up/1/file"}}]}"#;
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
