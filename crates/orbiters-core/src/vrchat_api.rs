//! An independent VRChat transport. Cookies never enter the Orbiters HTTP agent or API.
//! Blocking methods belong on background threads. No credentials, codes, cookies or response bodies are logged.
//! Reads, picture updates ([`replace_image`]), deliberate instance creation and self-invites.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};

const BASE: &str = "https://api.vrchat.cloud/api/1";
const UA_CONTACT: &str = concat!(" blackorbit.eclaire@gmail.com");
fn user_agent() -> String {
    format!("Orbiters-N4N0/{}{UA_CONTACT}", crate::app_version())
}
const ENTROPY: &[u8] = b"Orbiters.N4N0 VRChat session";

/// A short-lived grant for one Orbiters identity. Revocation is permanent: a new grant needs a new client.
/// The deadline is checked even while the controller is waiting for a network response.
pub struct Gate {
    revoked: AtomicBool,
    expires: Instant,
    operations: AtomicU64,
}
impl Default for Gate {
    fn default() -> Self {
        Self::for_duration(Duration::ZERO)
    }
}
impl Gate {
    pub fn for_duration(lifetime: Duration) -> Self {
        Self { revoked: AtomicBool::new(false), expires: Instant::now() + lifetime, operations: AtomicU64::new(0) }
    }
    pub fn allowed(&self) -> bool {
        !self.revoked.load(Ordering::Acquire) && Instant::now() < self.expires
    }
    /// Cancel queued personal work without disabling the separate logout transport.
    pub fn cancel_operations(&self) {
        self.operations.fetch_add(1, Ordering::AcqRel);
    }
    pub fn operation(&self, deadline: Instant) -> Operation {
        Operation { gate: self.operations.load(Ordering::Acquire), deadline }
    }
    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }
}

/// Local dispatch authorization captured before waiting for the client lock.
#[derive(Clone, Copy)]
pub struct Operation { gate: u64, deadline: Instant }
impl Operation { pub fn with_deadline(self, deadline: Instant) -> Self { Self { deadline: self.deadline.min(deadline), ..self } } }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Error {
    Disabled,
    DeadlineExceeded,
    OutcomeUnknown,
    Unauthorized,
    WrongCode,
    RateLimited(Duration),
    Unavailable,
    InvalidResponse,
    Storage,
    NotFound,
    UnsafeUrl,
    /// The avatar or world was uploaded by another account.
    NotYours,
    /// VRChat's storage refused an upload.
    Upload,
    /// VRChat did not finish checking an upload in time.
    Processing,
    /// VRChat answered with an error status the request does not expect, with its message when it gave one.
    Refused(u16, String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeadlineExceeded => f.write_str(tr("The request expired before it could be sent. Please try again.")),
            Self::OutcomeUnknown => f.write_str(tr("VRChat may have received the request. Check VRChat before trying again.")),
            Self::Disabled => f.write_str(tr("VRChat sign-in is not available for this Orbiters account.")),
            Self::Unauthorized => f.write_str(tr("Your VRChat session expired. Sign in again.")),
            Self::WrongCode => f.write_str(tr("That code was not accepted. Check it and try again.")),
            Self::RateLimited(d) => f.write_str(&tr!("VRChat is rate limiting requests. Try again in {seconds} seconds.", seconds = d.as_secs().max(1))),
            Self::Unavailable => f.write_str(tr("VRChat could not be reached. Try again shortly.")),
            Self::InvalidResponse => f.write_str(tr("VRChat returned an unexpected answer. Try again shortly.")),
            Self::Storage => f.write_str(tr("The VRChat session could not be saved securely on this computer.")),
            Self::NotFound => f.write_str(tr("VRChat does not show this item to your account.")),
            Self::UnsafeUrl => f.write_str(tr("VRChat returned an unsupported image address.")),
            Self::NotYours => f.write_str(tr("Only the VRChat account that uploaded it can change its picture.")),
            Self::Upload => f.write_str(tr("VRChat's storage did not accept the picture. Try again shortly.")),
            Self::Processing => f.write_str(tr("VRChat is still processing the picture. Try again in a minute.")),
            Self::Refused(status, message) if message.is_empty() => f.write_str(&tr!("VRChat refused the request (HTTP {status}). Try again shortly.", status = status)),
            Self::Refused(status, message) => f.write_str(&tr!("VRChat refused the request (HTTP {status}: {message}).", status = status, message = message)),
        }
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// Password/code carried over the authenticated local IPC link. Debug output never prints its contents.
#[derive(Clone, Serialize, Deserialize)]
pub struct Secret(pub String);
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("(hidden)")
    }
}

/// No Debug or public cookie access: the only serialization is inside encrypted local storage.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq)]
struct Cookies {
    auth: String,
    two_factor_auth: String,
}
impl Cookies {
    fn header(&self) -> String {
        [("auth", &self.auth), ("twoFactorAuth", &self.two_factor_auth)]
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
    fn absorb(&mut self, headers: &[String]) {
        for header in headers {
            let Some((name, value)) = header.split(';').next().unwrap_or_default().split_once('=') else { continue };
            // Cookie values are opaque RFC 6265 cookie-octets, never additional headers or cookies.
            if !value
                .bytes()
                .all(|b| b == 0x21 || (0x23..=0x2b).contains(&b) || (0x2d..=0x3a).contains(&b) || (0x3c..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b))
            {
                continue;
            }
            let expired = header.split(';').filter_map(|a| a.trim().split_once('=')).any(|(name, value)| {
                if name.eq_ignore_ascii_case("max-age") {
                    return value.trim().parse::<i64>().is_ok_and(|n| n <= 0);
                }
                name.eq_ignore_ascii_case("expires")
                    && time::OffsetDateTime::parse(value.trim(), &time::format_description::well_known::Rfc2822)
                        .is_ok_and(|date| date <= time::OffsetDateTime::now_utc())
            });
            let value = if expired { "" } else { value };
            match name.trim() {
                "auth" => self.auth = value.into(),
                "twoFactorAuth" => self.two_factor_auth = value.into(),
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TwoFactor {
    Totp,
    Email,
    Recovery,
}
impl TwoFactor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Totp => tr("Authenticator code"),
            Self::Email => tr("Email code"),
            Self::Recovery => tr("Recovery code"),
        }
    }
    fn path(self) -> &'static str {
        match self {
            Self::Totp => "totp",
            Self::Email => "emailotp",
            Self::Recovery => "otp",
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub user_icon: String,
    #[serde(default)]
    pub icon_url: String,
    #[serde(default)]
    pub current_avatar_thumbnail_image_url: String,
    #[serde(default)]
    pub current_avatar_image_url: String,
}
impl User {
    pub fn picture(&self) -> &str {
        [&self.user_icon, &self.icon_url, &self.current_avatar_thumbnail_image_url, &self.current_avatar_image_url]
            .into_iter()
            .find(|url| !url.is_empty())
            .map_or("", String::as_str)
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Avatar {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub thumbnail_image_url: String,
    #[serde(default)]
    pub image_url: String,
    /// The account that uploaded it (`usr_…`); empty when the answer did not say.
    #[serde(default)]
    pub author_id: String,
}
/// A world as VRChat describes it to the signed-in account (its own private uploads too).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct World {
    pub id: String,
    pub name: String,
    pub thumbnail_image_url: String,
    pub image_url: String,
    pub author_id: String,
    pub author_name: String,
    pub description: String,
    pub capacity: Option<u32>,
    pub recommended_capacity: Option<u32>,
    pub tags: Vec<String>,
    pub visits: Option<u64>,
    pub favorites: Option<u64>,
    /// Live enumeration uses a fresh world response, never the day-long metadata cache.
    pub instances: Vec<(String, u32, serde_json::Value)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstanceAccess { Public, FriendsPlus, Friends, InvitePlus, Invite, GroupPublic, GroupPlus, Group, Unknown }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CreateAccess { Public, FriendsPlus, Friends, InvitePlus, Invite }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceRegion { Us, Use, Eu, Jp }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldInstance {
    pub world: String,
    /// Exact opaque instanceId supplied by VRChat; never rebuild it from its number.
    pub id: String,
    pub access: InstanceAccess,
    pub region: String,
    pub occupants: u32,
    pub capacity: Option<u32>,
    pub friends: Option<u32>,
    pub full: bool,
    pub closed: bool,
    pub can_join: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldInstances {
    pub items: Vec<WorldInstance>,
    /// A lookup was hidden, missing or null; this is not evidence of closure.
    pub unavailable: usize,
    /// Exact remaining IDs for the next bounded batch. Never invite without a returned permit.
    pub remaining: Vec<String>,
}

fn instance_details(v: &serde_json::Value, world: &str, id: Option<&str>) -> Result<WorldInstance> {
    let instance = v["instanceId"].as_str().filter(|s| !s.is_empty()).ok_or(Error::InvalidResponse)?;
    if v["worldId"].as_str() != Some(world) || id.is_some_and(|id| id != instance) { return Err(Error::InvalidResponse); }
    let access = match v["accessType"].as_str() {
        Some("public") => InstanceAccess::Public, Some("friendsPlus") => InstanceAccess::FriendsPlus,
        Some("friends") => InstanceAccess::Friends, Some("invitePlus") => InstanceAccess::InvitePlus,
        Some("invite") => InstanceAccess::Invite, Some("groupPublic") => InstanceAccess::GroupPublic,
        Some("groupPlus") => InstanceAccess::GroupPlus, Some("group") => InstanceAccess::Group,
        _ => InstanceAccess::Unknown,
    };
    let occupants = v["n_users"].as_u64().and_then(|n| n.try_into().ok()).ok_or(Error::InvalidResponse)?;
    let capacity = v["capacity"].as_u64().or_else(|| v["world"]["capacity"].as_u64()).and_then(|n| u32::try_from(n).ok());
    let full = v["full"].as_bool().unwrap_or(false) || v["hasCapacityForYou"].as_bool() == Some(false)
        || capacity.is_some_and(|c| occupants >= c);
    // Closed only when VRChat says it closed (`closedAt` past, `hardClose`). `active: false` is not that: an instance
    // nobody has entered yet says it, every new one included (measured 2026-11-15: POST /instances and the GET after it
    // answer `"active": false, "closedAt": null, "hardClose": null` for a Friends+ instance of Popcorn Palace), and
    // VRChat opens it when someone arrives. Read as closed, it made every new instance unjoinable (L-invite-error).
    let closed = v["hardClose"].as_bool() == Some(true) || v["closedAt"].as_str().is_some_and(|s|
        time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
            .map_or(true, |t| t <= time::OffsetDateTime::now_utc()));
    let friends = v["users"].as_array().filter(|users| users.iter().all(|u| u["isFriend"].is_boolean()))
        .map(|users| users.iter().filter(|u| u["isFriend"].as_bool() == Some(true)).count() as u32);
    Ok(WorldInstance { world: world.into(), id: instance.into(), access, region: v["region"].as_str().unwrap_or_default().into(),
        occupants, capacity, friends, full, closed, can_join: !full && !closed && access != InstanceAccess::Unknown })
}

fn instance_body(world: &str, owner: &str, access: CreateAccess, region: InstanceRegion) -> serde_json::Value {
    let kind = match access { CreateAccess::Public => "public", CreateAccess::FriendsPlus => "hidden", CreateAccess::Friends => "friends", _ => "private" };
    let mut body = serde_json::json!({"worldId": world, "type": kind, "region": region, "canRequestInvite": access == CreateAccess::InvitePlus});
    if access != CreateAccess::Public { body["ownerId"] = owner.into(); }
    body
}
/// Whether `instance` (as VRChat's log writes it: `11027~private(usr_…)~region(eu)~nonce(…)`) is a private instance
/// `owner` owns: the strict kind only. Invite+ (`~canRequestInvite`), friends, hidden (friends+), group and public
/// instances are not, nor is one anybody else owns.
pub fn private_instance_of(instance: &str, owner: &str) -> bool {
    let mut parts = instance.split('~');
    let number = parts.next().unwrap_or_default();
    if number.is_empty() || !number.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let mut private = false;
    for part in parts {
        match part.split_once('(') {
            Some(("private", who)) => private = !owner.is_empty() && who.strip_suffix(')') == Some(owner),
            Some(("region" | "nonce", _)) => {}
            // `canRequestInvite`, `friends(…)`, `hidden(…)`, `group(…)`, and whatever VRChat adds next.
            _ => return false,
        }
    }
    private
}

/// The instance VRChat says it made for `POST /instances`: its `instanceId`, else what follows the world in its
/// `location`, when that is `world` and a private instance of `owner`.
fn created_instance(reply: &serde_json::Value, world: &str, owner: &str) -> Option<String> {
    let text = |key: &str| reply.get(key).and_then(serde_json::Value::as_str);
    let instance = text("instanceId").map(str::to_owned).or_else(|| {
        let (at, instance) = text("location")?.split_once(':')?;
        (at == world).then(|| instance.to_owned())
    })?;
    if text("worldId").is_some_and(|w| w != world) {
        return None;
    }
    private_instance_of(&instance, owner).then_some(instance)
}

/// What a picture belongs to, told by its ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Avatar,
    World,
}
impl Kind {
    pub fn of(id: &str) -> Option<Self> {
        if id.starts_with("avtr_") {
            Some(Self::Avatar)
        } else if id.starts_with("wrld_") {
            Some(Self::World)
        } else {
            None
        }
    }
    /// Its API collection, which is also the request family it is paced in.
    fn collection(self) -> &'static str {
        match self {
            Self::Avatar => "avatars",
            Self::World => "worlds",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Avatar => tr("Avatar"),
            Self::World => tr("World"),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Login {
    SignedIn(User),
    TwoFactor(Vec<TwoFactor>),
}
fn parse_login(body: &[u8]) -> Result<Login> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
    if let Some(methods) = v.get("requiresTwoFactorAuth").and_then(|v| v.as_array()) {
        let methods: Vec<_> = methods
            .iter()
            .filter_map(|m| match m.as_str()? {
                "totp" => Some(TwoFactor::Totp),
                "emailOtp" => Some(TwoFactor::Email),
                "otp" => Some(TwoFactor::Recovery),
                _ => None,
            })
            .collect();
        if methods.is_empty() {
            return Err(Error::InvalidResponse);
        }
        return Ok(Login::TwoFactor(methods));
    }
    let user: User = serde_json::from_value(v).map_err(|_| Error::InvalidResponse)?;
    if !user.id.starts_with("usr_") {
        return Err(Error::InvalidResponse);
    }
    Ok(Login::SignedIn(user))
}

/// Individually URL-encode the two fields before joining them (VRChat's Basic auth contract).
pub fn basic_header(username: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{}:{}", encode(username), encode(password)))) // i18n: not shown (an HTTP header)
}
/// A path segment as VRChat's firewall takes it: RFC 3986's path characters stay as they are (an instance's `~`, `(`,
/// `)`, `,` and `:`, which VRChat refuses percent-encoded with 400 "malformed url"), anything else is percent-encoded.
pub(crate) fn path_segment(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~!$&'()*+,;=:@".contains(&b) {
            out.push(b as char)
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// VRChat's message in an error answer (`{"error":{"message":"\"malformed url\"","status_code":400}}`), short and
/// without its quotes and the one-dot leader it ends some with.
fn error_message(body: &[u8]) -> String {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let m = v["error"]["message"].as_str().or(v["error"].as_str()).unwrap_or_default();
    let m = m.trim().trim_matches('"').trim_end_matches(['\u{2024}', '.']).trim();
    m.chars().take(120).collect()
}

/// Whether VRChat's answer for an instance says it is open: closed when it has closed (`closedAt`, `hardClose`) or
/// says so (`active`), or when it is `null` (VRChat's answer for an instance it does not know); open when it is the
/// instance (`active` is optional, true by default).
fn instance_open(v: &serde_json::Value) -> Option<bool> {
    if v.is_null() {
        return Some(false);
    }
    if !v.is_object() {
        return None;
    }
    if v["closedAt"].as_str().is_some_and(|c| !c.is_empty()) || v["hardClose"] == true {
        return Some(false);
    }
    v["active"].as_bool().or_else(|| (v["id"].is_string() || v["instanceId"].is_string()).then_some(true))
}

/// The API path of a page of a world list, one more world asked than a page holds (its being there says another page
/// follows): `GET /worlds?search=…&sort=relevance`, `/worlds?sort=popularity&order=descending`, `/worlds?sort=heat&order=descending`, `/worlds/active`,
/// `/worlds/favorites` (public worlds only but for the account's own favorites).
pub fn world_list_path(list: &crate::maps::WorldList, page: usize) -> String {
    use crate::maps::{WorldList, FOUND_PAGE, FOUND_PAGES};
    let (n, offset) = (FOUND_PAGE + 1, page.min(FOUND_PAGES) * FOUND_PAGE);
    match list {
        WorldList::Search(q) => {
            let q: String = q.trim().chars().take(crate::maps::SEARCH_MAX).collect();
            format!("/worlds?search={}&sort=relevance&releaseStatus=public&n={n}&offset={offset}", encode(&q))
        }
        WorldList::Popular => format!("/worlds?sort=popularity&order=descending&releaseStatus=public&n={n}&offset={offset}"),
        WorldList::Trending => format!("/worlds?sort=heat&order=descending&releaseStatus=public&n={n}&offset={offset}"),
        WorldList::Active => format!("/worlds/active?n={n}&offset={offset}"),
        WorldList::Favorites => format!("/worlds/favorites?n={n}&offset={offset}"),
        WorldList::Spotlight(row) => format!("/worlds?tag={}&sort=updated&order=descending&releaseStatus=public&n={n}&offset={offset}", row.tag()),
        WorldList::Row(row) => {
            let (sort, tag) = row.query();
            let tag = tag.map(|t| format!("tag={t}&")).unwrap_or_default();
            format!("/worlds?{tag}sort={sort}&order=descending&releaseStatus=public&n={n}&offset={offset}")
        }
        WorldList::FavoriteGroup(group) => format!("/worlds/favorites?tag={}&n={n}&offset={offset}", encode(group)),
    }
}

/// `GET /favorite/groups`' world groups: tag and name (the tag when it has none). A group of another type, or with a tag that
/// is not a plain word, is left out.
fn parse_favorite_groups(body: &[u8]) -> Result<Vec<(String, String)>> {
    let list: Vec<serde_json::Value> = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
    Ok(list
        .iter()
        .filter(|g| g["type"] == "world")
        .filter_map(|g| {
            let tag = g["name"].as_str().filter(|t| !t.is_empty() && t.len() <= 64 && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))?;
            let name = g["displayName"].as_str().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(tag);
            Some((tag.to_string(), name.to_string()))
        })
        .collect())
}

/// VRChat's `LimitedWorld` list as the Map explorer's worlds (pictures over https only: they are shown, fetched without
/// any credential, by the public picture loader), at most a page, and whether it held more. A world shaped otherwise is
/// left out, never the list.
fn parse_world_list(body: &[u8]) -> Result<(Vec<crate::maps::FoundWorld>, bool)> {
    let list: Vec<serde_json::Value> = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
    let more = list.len() > crate::maps::FOUND_PAGE;
    let text = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let whole = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_u64());
    let picture = |v: &serde_json::Value, k: &str| text(v, k).filter(|u| u.starts_with("https://"));
    let worlds = list
        .iter()
        .filter_map(|v| {
            let id = text(v, "id").filter(|id| crate::vrchat_log::is_world_id(id))?;
            let world = crate::maps::World {
                name: text(v, "name").unwrap_or_else(|| id.clone()),
                author_name: text(v, "authorName"),
                description: text(v, "description"),
                image_url: picture(v, "imageUrl"),
                thumbnail_url: picture(v, "thumbnailImageUrl").or_else(|| picture(v, "imageUrl")),
                capacity: whole(v, "capacity").map(|n| n.min(u32::MAX as u64) as u32),
                visits: whole(v, "visits"),
                favorites: whole(v, "favorites"),
                tags: v
                    .get("tags")
                    .and_then(|t| t.as_array())
                    .map(|t| t.iter().filter_map(|t| t.as_str().map(str::to_string)).take(64).collect())
                    .unwrap_or_default(),
                performance: None,
                world_id: id,
            };
            Some(crate::maps::FoundWorld { world, occupants: whole(v, "occupants").map(|n| n.min(u32::MAX as u64) as u32), pictures: 0, preview: false })
        })
        .take(crate::maps::FOUND_PAGE)
        .collect();
    Ok((worlds, more))
}

pub(crate) fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char)
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

struct Request {
    method: &'static str,
    url: String,
    cookie: String,
    basic: Option<String>,
    body: Option<String>,
    /// Bytes for a storage address VRChat handed out: sent alone, with their type and MD5 and no cookie.
    upload: Option<Upload>,
}
struct Upload {
    mime: &'static str,
    /// Base64, as `Content-MD5` carries it.
    md5: String,
    bytes: Vec<u8>,
}
struct Response {
    status: u16,
    cookies: Vec<String>,
    retry_after: Option<String>,
    location: Option<String>,
    body: Vec<u8>,
}
trait Transport: Send + Sync {
    fn send(&self, request: Request) -> Result<Response>;
}
struct Http(ureq::Agent);
impl Transport for Http {
    fn send(&self, r: Request) -> Result<Response> {
        // Redirects are handled explicitly below; neither Basic auth nor cookies follow an untrusted redirect.
        let response = match (r.method, r.upload) {
            ("GET", _) => {
                let mut q = self.0.get(&r.url);
                if !r.cookie.is_empty() {
                    q = q.header("Cookie", &r.cookie);
                }
                if let Some(basic) = r.basic {
                    q = q.header("Authorization", &basic);
                }
                q.call()
            }
            ("POST", _) => self.0.post(&r.url).header("Cookie", &r.cookie).header("Content-Type", "application/json").send(r.body.unwrap_or_default()),
            // A picture can take longer than an API answer on a slow connection.
            ("PUT", Some(u)) => self
                .0
                .put(&r.url)
                .config()
                .timeout_global(Some(Duration::from_secs(300)))
                .build()
                .header("Content-Type", u.mime)
                .header("Content-MD5", &u.md5)
                .send(&u.bytes[..]),
            ("PUT", None) => match r.body {
                Some(body) => self.0.put(&r.url).header("Cookie", &r.cookie).header("Content-Type", "application/json").send(body),
                None => self.0.put(&r.url).header("Cookie", &r.cookie).send_empty(),
            },
            _ => return Err(Error::InvalidResponse),
        }
        .map_err(|_| Error::Unavailable)?;
        let status = response.status().as_u16();
        let cookies = response.headers().get_all("set-cookie").iter().filter_map(|h| h.to_str().ok().map(str::to_owned)).collect();
        let retry_after = response.headers().get("retry-after").and_then(|h| h.to_str().ok()).map(str::to_owned);
        let location = response.headers().get("location").and_then(|h| h.to_str().ok()).map(str::to_owned);
        let body = response.into_body().with_config().limit(16 * 1024 * 1024).read_to_vec().map_err(|_| Error::InvalidResponse)?;
        Ok(Response { status, cookies, retry_after, location, body })
    }
}

#[derive(Default)]
struct Limiter {
    next: HashMap<&'static str, Instant>,
    blocked: Option<Instant>,
    /// Tests of long request sequences go without the one-second spacing.
    #[cfg(any(test, feature = "test-support"))]
    unpaced: bool,
}
impl Limiter {
    fn wait(&self, family: &'static str, now: Instant) -> Duration {
        self.next.get(family).copied().into_iter().chain(self.blocked).max().map_or(Duration::ZERO, |t| t.saturating_duration_since(now))
    }
    fn used(&mut self, family: &'static str, now: Instant) {
        #[cfg(any(test, feature = "test-support"))]
        if self.unpaced {
            return;
        }
        self.next.insert(family, now + Duration::from_secs(1));
    }
}
fn retry_after(value: Option<&str>) -> Duration {
    if let Some(s) = value.and_then(|s| s.trim().parse::<u64>().ok()) {
        return Duration::from_secs(s.max(1).min(86400 * 365));
    }
    if let Some(date) = value.and_then(|s| time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc2822).ok()) {
        return Duration::from_secs((date - time::OffsetDateTime::now_utc()).whole_seconds().max(1) as u64);
    }
    Duration::from_secs(60)
}

/// Reusable session, deliberately unrelated to the Orbiters client. The gate is checked on every request and cache read.
pub struct Client {
    /// Shared with [`replace_image`]'s storage uploads, which run without the client locked.
    transport: Arc<dyn Transport>,
    cookies: Cookies,
    allowed: Arc<Gate>,
    operation: Option<Operation>,
    #[cfg(test)]
    pacing_pause: Option<Arc<std::sync::Barrier>>,
    limiter: Arc<Mutex<Limiter>>,
    user: Option<User>,
    pending_methods: Vec<TwoFactor>,
    avatars: HashMap<String, (Instant, Avatar)>,
    worlds: HashMap<String, (Instant, World)>,
    friends: Option<(Instant, Vec<User>)>,
    instances: HashMap<(String, String), (Instant, Option<bool>)>,
    /// Pages of world lists (searches, popular, favorites) by their path: what they found and whether more follow.
    world_lists: HashMap<String, (Instant, Vec<crate::maps::FoundWorld>, bool)>,
    store: Option<PathBuf>,
}
impl Client {
    pub fn new(allowed: Arc<Gate>) -> Self {
        // Changing Orbiters accounts or refreshing the feature cannot reset the rate limit or a 429 backoff.
        static LIMITER: OnceLock<Arc<Mutex<Limiter>>> = OnceLock::new();
        let agent = ureq::Agent::config_builder()
            .tls_config(crate::http_tls())
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(user_agent())
            .build()
            .new_agent();
        Self {
            transport: Arc::new(Http(agent)),
            cookies: Cookies::default(),
            allowed,
            operation: None,
            #[cfg(test)]
            pacing_pause: None,
            limiter: LIMITER.get_or_init(Default::default).clone(),
            user: None,
            pending_methods: Vec::new(),
            avatars: HashMap::new(),
            worlds: HashMap::new(),
            friends: None,
            instances: HashMap::new(),
            world_lists: HashMap::new(),
            store: dirs::data_local_dir().map(|d| d.join("Orbiters/vrchat-session.bin")),
        }
    }
    pub fn operation(&self, deadline: Instant) -> Operation { self.allowed.operation(deadline) }
    pub fn with_operation<T>(&mut self, operation: Operation, work: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.operation = Some(operation);
        let result = self.permitted().and_then(|_| work(self));
        self.operation = None;
        result
    }
    fn permitted(&self) -> Result<()> {
        if self.operation.is_some_and(|o| Instant::now() >= o.deadline) { return Err(Error::DeadlineExceeded); }
        if self.allowed.allowed() && self.operation.is_none_or(|o| o.gate == self.allowed.operations.load(Ordering::Acquire)) {
            Ok(())
        } else {
            Err(Error::Disabled)
        }
    }
    pub fn signed_in(&self) -> bool {
        self.allowed.allowed() && self.user.is_some()
    }
    pub fn signed_in_user_id(&self) -> Option<&str> {
        self.signed_in().then(|| self.user.as_ref().map(|u| u.id.as_str())).flatten()
    }
    fn authenticated(&self) -> Result<()> {
        self.permitted()?;
        if self.user.is_some() {
            Ok(())
        } else {
            Err(Error::Unauthorized)
        }
    }
    /// Called only after the feature check. A disabled feature never reads or decrypts this file.
    pub fn restore(&mut self) -> Result<bool> {
        self.permitted()?;
        let path = self.store.as_ref().ok_or(Error::Storage)?;
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(Error::Storage),
        };
        if bytes.len() > 64 * 1024 {
            return Err(Error::Storage);
        }
        let plain = crate::dpapi::crypt(&bytes, ENTROPY, false).map_err(|_| Error::Storage)?;
        self.permitted()?;
        self.cookies = serde_json::from_slice(&plain).map_err(|_| Error::Storage)?;
        Ok(!self.cookies.auth.is_empty())
    }
    fn save(&self) -> Result<()> {
        self.permitted()?;
        // Unit tests have no store and never touch the person's session.
        let Some(path) = self.store.as_ref() else { return Ok(()) };
        let plain = serde_json::to_vec(&self.cookies).map_err(|_| Error::Storage)?;
        let sealed = crate::dpapi::crypt(&plain, ENTROPY, true).map_err(|_| Error::Storage)?;
        std::fs::create_dir_all(path.parent().ok_or(Error::Storage)?).map_err(|_| Error::Storage)?;
        crate::write_atomic_private(path, &sealed).map_err(|_| Error::Storage)
    }
    fn forget(&mut self) -> Result<()> {
        self.cookies = Cookies::default();
        self.user = None;
        self.pending_methods.clear();
        self.avatars.clear();
        self.worlds.clear();
        self.friends = None;
        self.instances.clear();
        self.world_lists.clear();
        if let Some(path) = &self.store {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(Error::Storage),
            }
        }
        Ok(())
    }
    fn pace(&self, family: &'static str) -> Result<()> {
        loop {
            self.permitted()?;
            let wait = {
                let mut limiter = self.limiter.lock().unwrap();
                let now = Instant::now();
                if let Some(blocked) = limiter.blocked {
                    let wait = blocked.saturating_duration_since(now);
                    if !wait.is_zero() {
                        return Err(Error::RateLimited(wait));
                    }
                }
                let wait = limiter.wait(family, now);
                if wait.is_zero() {
                    limiter.used(family, now);
                    return self.permitted();
                }
                wait
            };
            #[cfg(test)]
            if let Some(pause) = &self.pacing_pause { if self.operation.is_none_or(|o| o.gate == self.allowed.operations.load(Ordering::Acquire)) { pause.wait(); pause.wait(); } }
            std::thread::sleep(wait.min(Duration::from_millis(50)));
        }
    }
    fn back_off(&self, response: &Response) -> Error {
        let delay = retry_after(response.retry_after.as_deref());
        let until = Instant::now() + delay;
        let mut limiter = self.limiter.lock().unwrap();
        limiter.blocked = Some(limiter.blocked.map_or(until, |old| old.max(until)));
        Error::RateLimited(delay)
    }
    fn request(&mut self, family: &'static str, method: &'static str, url: String, basic: Option<String>, body: Option<String>) -> Result<Response> {
        self.request_accepting(family, method, url, basic, body, &[])
    }
    /// [`Self::request`], an answer with a status of `accept` returned as it is (a request that tells those apart).
    fn request_accepting(
        &mut self,
        family: &'static str,
        method: &'static str,
        url: String,
        basic: Option<String>,
        body: Option<String>,
        accept: &[u16],
    ) -> Result<Response> {
        self.pace(family)?;
        let verifying = family == "auth" && url.contains("/twofactorauth/") && url.ends_with("/verify");
        let session_host = url.starts_with("https://api.vrchat.cloud/") || url.starts_with("https://files.vrchat.cloud/");
        let cookie = if session_host { self.cookies.header() } else { String::new() };
        self.permitted()?;
        let response = self.transport.send(Request { method, url, cookie, basic, body, upload: None }).map_err(|e| if method == "POST" && matches!(family, "instances" | "invites") { Error::OutcomeUnknown } else { e })?;
        // Backoff survives a revoked grant. No response data or cookies do.
        if response.status == 429 {
            return Err(self.back_off(&response));
        }
        self.permitted().map_err(|e| if method == "POST" && matches!(family, "instances" | "invites") { Error::OutcomeUnknown } else { e })?;
        let before = self.cookies.clone();
        if session_host {
            self.cookies.absorb(&response.cookies);
        }
        if !before.auth.is_empty() && self.cookies.auth.is_empty() {
            self.forget()?;
            return Err(Error::Unauthorized);
        }
        if accept.contains(&response.status) {
            return Ok(response);
        }
        match response.status {
            400 if verifying => Err(Error::WrongCode),
            401 if !session_host => Err(Error::NotFound),
            401 => {
                self.forget()?;
                Err(Error::Unauthorized)
            }
            404 | 403 => Err(Error::NotFound),
            500..=599 if method == "POST" && matches!(family, "instances" | "invites") => Err(Error::OutcomeUnknown),
            500..=599 => Err(Error::Unavailable),
            200..=399 => {
                if self.cookies != before && self.user.is_some() {
                    self.save()?;
                }
                Ok(response)
            }
            status => Err(Error::Refused(status, error_message(&response.body))),
        }
    }
    fn get(&mut self, family: &'static str, path: &str) -> Result<Response> {
        self.request(family, "GET", format!("{BASE}{path}"), None, None)
    }
    fn accept_login(&mut self, body: &[u8]) -> Result<Login> {
        let login = parse_login(body)?;
        if self.cookies.auth.is_empty() {
            return Err(Error::InvalidResponse);
        }
        match &login {
            Login::SignedIn(user) => {
                self.save()?;
                self.user = Some(user.clone());
                self.pending_methods.clear();
            }
            Login::TwoFactor(methods) => {
                self.user = None;
                self.avatars.clear();
                self.worlds.clear();
                self.friends = None;
                self.instances.clear();
                self.world_lists.clear();
                self.pending_methods = methods.clone();
            }
        }
        Ok(login)
    }
    pub fn sign_in(&mut self, username: &str, password: &str) -> Result<Login> {
        self.permitted()?;
        self.forget()?;
        let response = self.request("auth", "GET", format!("{BASE}/auth/user"), Some(basic_header(username, password)), None)?;
        self.accept_login(&response.body)
    }
    pub fn verify(&mut self, method: TwoFactor, code: &str) -> Result<Login> {
        self.permitted()?;
        if !self.pending_methods.contains(&method) {
            return Err(Error::InvalidResponse);
        }
        // A successful verification followed by a failed user lookup must not consume a recovery code twice.
        if !self.cookies.two_factor_auth.is_empty() {
            return self.current_login();
        }
        let body = serde_json::json!({"code":code}).to_string();
        let response = self.request("auth", "POST", format!("{BASE}/auth/twofactorauth/{}/verify", method.path()), None, Some(body));
        let response = response?;
        let value: serde_json::Value = serde_json::from_slice(&response.body).map_err(|_| Error::InvalidResponse)?;
        if value["verified"] != true {
            return Err(Error::WrongCode);
        }
        self.current_login()
    }
    pub fn current_login(&mut self) -> Result<Login> {
        let r = self.get("auth", "/auth/user")?;
        self.accept_login(&r.body)
    }
    pub fn cancel_login(&mut self) -> Result<()> {
        self.permitted()?;
        self.forget()
    }
    /// Retry a saved or partly completed session without sending a password or repeating verification.
    pub fn resume(&mut self) -> Result<Option<Login>> {
        self.permitted()?;
        if self.cookies.auth.is_empty() && !self.restore()? {
            return Ok(None);
        }
        self.current_login().map(Some)
    }
    pub fn current_user(&mut self) -> Result<User> {
        match self.current_login()? {
            Login::SignedIn(u) => Ok(u),
            _ => Err(Error::Unauthorized),
        }
    }
    /// Forget locally even if VRChat is offline. Remote logout is best effort, with any failure reported.
    pub fn sign_out(&mut self) -> Result<()> {
        self.permitted()?;
        let cookie = self.cookies.header();
        self.forget()?;
        if cookie.is_empty() {
            return Ok(());
        }
        self.pace("auth")?;
        let r = self.transport.send(Request { method: "PUT", url: format!("{BASE}/logout"), cookie, basic: None, body: None, upload: None })?;
        if r.status == 429 {
            return Err(self.back_off(&r));
        }
        if (200..300).contains(&r.status) || r.status == 401 {
            Ok(())
        } else {
            Err(Error::Unavailable)
        }
    }
    pub fn avatar(&mut self, id: &str) -> Result<Avatar> {
        self.authenticated()?;
        if let Some((at, a)) = self.avatars.get(id).filter(|(at, _)| at.elapsed() < Duration::from_secs(86400)) {
            let _ = at;
            return Ok(a.clone());
        }
        let r = self.get("avatars", &format!("/avatars/{}", encode(id)))?;
        self.keep_avatar(id, &r.body)
    }
    /// Community specification: PUT /avatars/{avatarId}/select, authenticated session only.
    pub fn select_avatar(&mut self, id: &str) -> Result<()> {
        self.authenticated()?;
        if !id.starts_with("avtr_") || id.len() != 41 { return Err(Error::InvalidResponse); }
        self.request("avatars", "PUT", format!("{BASE}/avatars/{}/select", encode(id)), None, None)?;
        Ok(())
    }
    fn keep_avatar(&mut self, id: &str, body: &[u8]) -> Result<Avatar> {
        let a: Avatar = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
        if self.avatars.len() >= 512 {
            self.avatars.clear();
        }
        self.avatars.insert(id.into(), (Instant::now(), a.clone()));
        Ok(a)
    }
    /// A world, cached for a day like avatars.
    pub fn world(&mut self, id: &str) -> Result<World> {
        self.authenticated()?;
        if let Some((_, w)) = self.worlds.get(id).filter(|(at, _)| at.elapsed() < Duration::from_secs(86400)) {
            return Ok(w.clone());
        }
        self.fetch_world(id)
    }
    /// Always asks (a world's thumbnail changes with its uploads), and keeps the answer for [`Client::world`].
    fn fetch_world(&mut self, id: &str) -> Result<World> {
        let r = self.get("worlds", &format!("/worlds/{}", encode(id)))?;
        self.keep_world(id, &r.body)
    }
    fn keep_world(&mut self, id: &str, body: &[u8]) -> Result<World> {
        let w: World = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
        if self.worlds.len() >= 512 {
            self.worlds.clear();
        }
        self.worlds.insert(id.into(), (Instant::now(), w.clone()));
        Ok(w)
    }
    /// A world's thumbnail address (its own uploads included, public or not).
    pub fn world_thumbnail(&mut self, id: &str) -> Result<String> {
        self.authenticated()?;
        let w = self.fetch_world(id)?;
        Ok(if w.thumbnail_image_url.is_empty() { w.image_url } else { w.thumbnail_image_url })
    }
    /// Whether the signed-in account uploaded `id` (an avatar or a world), from the day-long caches. An ID VRChat does not
    /// show to the account, or of another kind, is not its own.
    pub fn owns(&mut self, id: &str) -> Result<bool> {
        self.authenticated()?;
        let me = self.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
        let author = match Kind::of(id) {
            Some(Kind::Avatar) => self.avatar(id).map(|a| a.author_id),
            Some(Kind::World) => self.world(id).map(|w| w.author_id),
            None => return Ok(false),
        };
        match author {
            Ok(author) => Ok(!author.is_empty() && author == me),
            Err(Error::NotFound) => Ok(false),
            Err(e) => Err(e),
        }
    }
    pub fn friends(&mut self) -> Result<Vec<User>> {
        self.authenticated()?;
        if let Some((_, friends)) = self.friends.as_ref().filter(|(at, _)| at.elapsed() < Duration::from_secs(300)) {
            return Ok(friends.clone());
        }
        let mut all = BTreeMap::new();
        for offline in [false, true] {
            let mut offset = 0;
            loop {
                let r = self.get("friends", &format!("/auth/user/friends?offline={offline}&n=100&offset={offset}"))?;
                let page: Vec<User> = serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)?;
                let n = page.len();
                for u in page {
                    all.insert(u.id.clone(), u);
                }
                if n < 100 {
                    break;
                }
                offset += 100;
                if offset > 10000 {
                    return Err(Error::InvalidResponse);
                }
            }
        }
        let all: Vec<_> = all.into_values().collect();
        self.friends = Some((Instant::now(), all.clone()));
        Ok(all)
    }
    pub fn user(&mut self, id: &str) -> Result<User> {
        self.authenticated()?;
        let r = self.get("users", &format!("/users/{}", encode(id)))?;
        serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)
    }
    /// Whether the instance (`25332~hidden(usr_…)~region(eu)~nonce(…)`, as VRChat's log names it) is still open:
    /// `Some(false)` when VRChat no longer knows it (404 "Instance not found", or the `null` it answers an unknown
    /// instance with) or reports it closed, `None` when access is denied: a hidden instance cannot be declared dead.
    pub fn instance_alive(&mut self, world: &str, instance: &str) -> Result<Option<bool>> {
        self.authenticated()?;
        let key = (world.to_owned(), instance.to_owned());
        if let Some((_, alive)) = self.instances.get(&key).filter(|(at, _)| at.elapsed() < Duration::from_secs(60)) {
            return Ok(*alive);
        }
        // The location goes as VRChat writes it: percent-encoded, the `( )` of its parts are refused (400).
        let url = format!("{BASE}/instances/{}:{}", path_segment(world), path_segment(instance));
        let alive = match self.request_accepting("instances", "GET", url, None, None, &[404]) {
            Ok(r) if r.status == 404 => Ok(Some(false)),
            Ok(r) if (300..400).contains(&r.status) => Err(Error::Refused(r.status, String::new())),
            Ok(r) => serde_json::from_slice(&r.body).map(|v| instance_open(&v)).map_err(|_| Error::InvalidResponse),
            Err(Error::NotFound) => Ok(None),
            Err(e) => Err(e),
        }?;
        if self.instances.len() >= 256 {
            self.instances.clear();
        }
        self.instances.insert(key, (Instant::now(), alive));
        Ok(alive)
    }
    /// A page of a world list ([`world_list_path`]) and whether another follows, read as VRChat's `LimitedWorld`s. Kept
    /// five minutes (the favorites one): typing a search again, or paging back, asks nothing.
    pub fn find_worlds(&mut self, list: &crate::maps::WorldList, page: usize) -> Result<(Vec<crate::maps::FoundWorld>, bool)> {
        self.authenticated()?;
        let path = world_list_path(list, page);
        let keep = if matches!(list, crate::maps::WorldList::Favorites | crate::maps::WorldList::FavoriteGroup(_)) { Duration::from_secs(60) } else { Duration::from_secs(300) };
        if let Some((_, worlds, more)) = self.world_lists.get(&path).filter(|(at, ..)| at.elapsed() < keep) {
            return Ok((worlds.clone(), *more));
        }
        let r = self.get("worlds", &path)?;
        let (worlds, more) = parse_world_list(&r.body)?;
        if self.world_lists.len() >= 64 {
            self.world_lists.clear();
        }
        self.world_lists.insert(path, (Instant::now(), worlds.clone(), more));
        Ok((worlds, more))
    }
    /// The account's world favourite groups, its playlists of worlds in VRChat's menu (Favorites 1 to 4, more with VRC+):
    /// each one's tag (`worlds1`…, what `/worlds/favorites?tag=` takes) and the name the account gave it, in VRChat's order.
    pub fn favorite_world_groups(&mut self) -> Result<Vec<(String, String)>> {
        self.authenticated()?;
        let r = self.get("favorites", "/favorite/groups?type=world&n=50")?;
        parse_favorite_groups(&r.body)
    }
    /// The first bounded batch of fresh visible instances; remaining holds the exact IDs for continuation.
    pub fn world_instances(&mut self, world: &str) -> Result<WorldInstances> {
        self.authenticated()?;
        let w = self.fetch_world(world)?;
        self.world_instance_batch(world, w.instances.into_iter().map(|(id, _, _)| id).collect())
    }
    pub fn world_instance_ids(&mut self, world: &str) -> Result<Vec<String>> {
        self.authenticated()?;
        Ok(self.fetch_world(world)?.instances.into_iter().map(|(id, _, _)| id).collect())
    }
    pub fn world_instance_batch(&mut self, world: &str, ids: Vec<String>) -> Result<WorldInstances> {
        let mut result = WorldInstances::default();
        result.remaining = ids.iter().skip(16).cloned().collect();
        for id in ids.into_iter().take(16) {
            match self.world_instance(world, &id) {
                Ok(Some(i)) => result.items.push(i),
                Ok(None) | Err(Error::NotFound) => result.unavailable += 1,
                Err(e) => return Err(e),
            }
        }
        Ok(result)
    }
    pub fn world_instance(&mut self, world: &str, id: &str) -> Result<Option<WorldInstance>> {
        self.authenticated()?;
        let r = self.get("instances", &format!("/instances/{}:{}", path_segment(world), path_segment(id)))?;
        let v: serde_json::Value = serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)?;
        if v.is_null() { return Ok(None); }
        instance_details(&v, world, Some(id)).map(Some)
    }
    pub fn create_instance(&mut self, world: &str, access: CreateAccess, region: InstanceRegion) -> Result<WorldInstance> {
        self.authenticated()?;
        let owner = self.user.as_ref().ok_or(Error::Unauthorized)?.id.clone();
        let body = instance_body(world, &owner, access, region);
        let r = self.request("instances", "POST", format!("{BASE}/instances"), None, Some(body.to_string()))?;
        let v: serde_json::Value = serde_json::from_slice(&r.body).map_err(|_| Error::OutcomeUnknown)?;
        let i = instance_details(&v, world, None).map_err(|_| Error::OutcomeUnknown)?;
        let expected = match access { CreateAccess::Public => InstanceAccess::Public, CreateAccess::FriendsPlus => InstanceAccess::FriendsPlus,
            CreateAccess::Friends => InstanceAccess::Friends, CreateAccess::InvitePlus => InstanceAccess::InvitePlus, CreateAccess::Invite => InstanceAccess::Invite };
        if i.access != expected || v["region"] != serde_json::to_value(region).map_err(|_| Error::OutcomeUnknown)?
            || (access != CreateAccess::Public && v["ownerId"].as_str() != Some(owner.as_str())) { return Err(Error::OutcomeUnknown); }
        Ok(i)
    }
    /// A new private instance of `world` in `region`, owned by the signed-in account (`12345~private(usr_…)~region(eu)~nonce(…)`):
    /// invite only, nobody can ask to join it. An answer that names anything else is refused here, so that what comes
    /// out of this is always such an instance ([`private_instance_of`]).
    pub fn create_private_instance(&mut self, world: &str, region: &str) -> Result<String> {
        self.authenticated()?;
        let owner = self.user.as_ref().map(|u| u.id.clone()).ok_or(Error::Unauthorized)?;
        let body = serde_json::json!({ "worldId": world, "type": "private", "ownerId": owner, "region": region, "canRequestInvite": false });
        let r = self.request("instances", "POST", format!("{BASE}/instances"), None, Some(body.to_string()))?;
        let reply: serde_json::Value = serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)?;
        created_instance(&reply, world, &owner).ok_or(Error::InvalidResponse)
    }
    /// Sends the signed-in account an invite to an exact instance. UI callers must use the service's scoped
    /// listed/created destination validation before reaching this transport operation.
    pub fn invite_self(&mut self, world: &str, instance: &str) -> Result<()> {
        self.authenticated()?;
        let url = format!("{BASE}/invite/myself/to/{}:{}", path_segment(world), path_segment(instance));
        self.request("invites", "POST", url, None, Some("{}".into())).map(drop)
    }
    /// Only VRChat-owned image hosts. Redirects to any other host are refused before credentials leave the process.
    pub fn image(&mut self, url: &str) -> Result<Vec<u8>> {
        self.authenticated()?;
        let mut url = url.to_string();
        for _ in 0..5 {
            if !image_url_allowed(&url) {
                return Err(Error::UnsafeUrl);
            }
            let r = self.request("images", "GET", url, None, None)?;
            if (200..300).contains(&r.status) {
                return Ok(r.body);
            }
            url = r.location.ok_or(Error::InvalidResponse)?;
        }
        Err(Error::InvalidResponse)
    }

    // The file API, as VRChat's SDK and VRCX use it for a picture. Each call checks the session again: a sign-out during
    // an upload stops it at its next step.
    /// The item asked again (not from the caches): who uploaded it, its name and picture.
    fn fresh(&mut self, kind: Kind, id: &str) -> Result<World> {
        self.authenticated()?;
        let r = self.get(kind.collection(), &format!("/{}/{}", kind.collection(), encode(id)))?;
        if kind == Kind::Avatar {
            self.keep_avatar(id, &r.body)?;
        }
        self.keep_world_like(kind, id, &r.body)
    }
    /// An avatar's answer read for the fields it shares with a world's; a world's kept for [`Client::world`].
    fn keep_world_like(&mut self, kind: Kind, id: &str, body: &[u8]) -> Result<World> {
        match kind {
            Kind::World => self.keep_world(id, body),
            Kind::Avatar => serde_json::from_slice(body).map_err(|_| Error::InvalidResponse),
        }
    }
    fn file(&mut self, id: &str) -> Result<RemoteFile> {
        self.authenticated()?;
        let r = self.get("files", &format!("/file/{}", encode(id)))?;
        serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)
    }
    fn create_file(&mut self, name: &str) -> Result<RemoteFile> {
        self.authenticated()?;
        let body = serde_json::json!({"name": name, "mimeType": "image/png", "extension": ".png"}).to_string();
        let r = self.request("files", "POST", format!("{BASE}/file"), None, Some(body))?;
        serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)
    }
    /// A new version of file `id` for a picture of `png` bytes with signature `signature`: its number.
    fn add_version(&mut self, id: &str, file: &Part, signature: &Part) -> Result<u64> {
        self.authenticated()?;
        let body = version_body(file, signature);
        let r = self.request("files", "POST", format!("{BASE}/file/{}", encode(id)), None, Some(body))?;
        let f: RemoteFile = serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)?;
        f.latest().filter(|v| v.version > 0).map(|v| v.version).ok_or(Error::InvalidResponse)
    }
    /// The storage address the `part` (`file` or `signature`) of a version goes to.
    fn start_part(&mut self, id: &str, version: u64, part: &str) -> Result<String> {
        self.authenticated()?;
        let r = self.request("files", "PUT", format!("{BASE}/file/{}/{version}/{part}/start", encode(id)), None, None)?;
        let v: serde_json::Value = serde_json::from_slice(&r.body).map_err(|_| Error::InvalidResponse)?;
        v["url"].as_str().filter(|u| !u.is_empty()).map(str::to_owned).ok_or(Error::InvalidResponse)
    }
    fn finish_part(&mut self, id: &str, version: u64, part: &str) -> Result<()> {
        self.authenticated()?;
        // One part, so no ETags (the signature never has any); the two counters are always zero, in strings.
        let body = serde_json::json!({"maxParts": "0", "nextPartNumber": "0"}).to_string();
        self.request("files", "PUT", format!("{BASE}/file/{}/{version}/{part}/finish", encode(id)), None, Some(body)).map(drop)
    }
    /// Points the item at its new picture; VRChat's answer must name it.
    fn set_image(&mut self, kind: Kind, id: &str, image_url: &str) -> Result<Replaced> {
        self.authenticated()?;
        let body = serde_json::json!({"id": id, "imageUrl": image_url}).to_string();
        let r = self.request(kind.collection(), "PUT", format!("{BASE}/{}/{}", kind.collection(), encode(id)), None, Some(body))?;
        if kind == Kind::Avatar {
            self.keep_avatar(id, &r.body)?;
        }
        let item = self.keep_world_like(kind, id, &r.body)?;
        if item.image_url != image_url {
            return Err(Error::InvalidResponse);
        }
        Ok(Replaced { image_url: item.image_url, thumbnail_image_url: item.thumbnail_image_url })
    }
}

/// A picture's new addresses on VRChat, once [`replace_image`] pointed its item at them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Replaced {
    pub image_url: String,
    pub thumbnail_image_url: String,
}
/// Where [`replace_image`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Step {
    /// Making sure the signed-in account uploaded the item.
    Checking,
    /// Adding a version to the item's picture file.
    Preparing,
    Uploading,
    /// Uploading the picture's librsync signature, which VRChat's files keep beside each version.
    Signing,
    /// Waiting for VRChat to call the version complete.
    Processing,
    /// Pointing the item at its new picture.
    Saving,
}
impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Self::Checking => tr("Checking it is yours…"),
            Self::Preparing => tr("Preparing the upload…"),
            Self::Uploading => tr("Uploading the picture…"),
            Self::Signing => tr("Uploading its signature…"),
            Self::Processing => tr("VRChat is checking it…"),
            Self::Saving => tr("Saving…"),
        }
    }
    /// How far the replacement is (0 to 1) once this step starts.
    pub fn progress(self) -> f32 {
        match self {
            Self::Checking => 0.04,
            Self::Preparing => 0.14,
            Self::Uploading => 0.24,
            Self::Signing => 0.66,
            Self::Processing => 0.76,
            Self::Saving => 0.9,
        }
    }
}

/// A part of a file version: its bytes, type and base64 MD5 (the form both VRChat's file API and `Content-MD5` take).
struct Part<'a> {
    bytes: &'a [u8],
    mime: &'static str,
    md5: String,
}
impl<'a> Part<'a> {
    fn new(bytes: &'a [u8], mime: &'static str) -> Self {
        Self { bytes, mime, md5: md5_base64(bytes) }
    }
}
fn version_body(file: &Part, signature: &Part) -> String {
    serde_json::json!({
        "signatureMd5": signature.md5,
        "signatureSizeInBytes": signature.bytes.len(),
        "fileMd5": file.md5,
        "fileSizeInBytes": file.bytes.len(),
    })
    .to_string()
}
fn md5_base64(bytes: &[u8]) -> String {
    STANDARD.encode(md5(bytes))
}
/// MD5 (RFC 1321), which VRChat's file API checks uploads with.
fn md5(data: &[u8]) -> [u8; 16] {
    const SHIFT: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
        0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
        0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
        0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
        0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1, 0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
        0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
    ];
    let mut state: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_le_bytes());
    for chunk in padded.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (w, c) in m.iter_mut().zip(chunk.chunks_exact(4)) {
            *w = u32::from_le_bytes(c.try_into().unwrap());
        }
        let [mut a, mut b, mut c, mut d] = state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(SHIFT[(i / 16) * 4 + i % 4]));
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
    let mut out = [0u8; 16];
    for (o, s) in out.chunks_exact_mut(4).zip(state) {
        o.copy_from_slice(&s.to_le_bytes());
    }
    out
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteFile {
    id: String,
    #[serde(default)]
    owner_id: String,
    #[serde(default)]
    mime_type: String,
    #[serde(default)]
    versions: Vec<RemoteVersion>,
}
#[derive(Deserialize)]
struct RemoteVersion {
    version: u64,
    #[serde(default)]
    status: String,
}
impl RemoteFile {
    fn latest(&self) -> Option<&RemoteVersion> {
        self.versions.iter().max_by_key(|v| v.version)
    }
    /// A PNG file of `me` whose last version is complete takes the next version, as the SDK's uploads do. One with an
    /// upload left unfinished (here or by the SDK) is left alone: a new file is made instead, nothing deleted.
    fn takes_a_version(&self, me: &str) -> bool {
        self.owner_id == me && self.mime_type == "image/png" && self.latest().is_some_and(|v| v.status == "complete")
    }
}
/// The file a VRChat picture address names (`https://api.vrchat.cloud/api/1/file/file_<uuid>/<version>/file`).
fn file_id(url: &str) -> Option<&str> {
    url.split(['/', '?', '#']).find(|s| s.len() > 5 && s.starts_with("file_") && s[5..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// Replaces the picture of `id`, an avatar or a world the signed-in account uploaded, with `png` (a PNG), the way
/// VRChat's SDK and VRCX do: a new version of the item's own picture file (or of a new file when it has none that
/// can take one) gets the picture and its librsync signature, each put to the storage address VRChat hands out for
/// it, and once VRChat calls the version complete the item is pointed at
/// `https://api.vrchat.cloud/api/1/file/<file>/<version>/file`. `progress` hears each step as it starts.
///
/// The owner is asked again first: anything another account uploaded is refused before a byte is sent. The client
/// is locked for each VRChat request only, so the uploads and the waits leave it to other requests. Nothing is
/// retried: a failure leaves at most an unfinished version of the account's own file, which the next attempt does
/// not reuse.
pub fn replace_image(client: &Mutex<Client>, id: &str, png: &[u8], mut progress: impl FnMut(Step)) -> Result<Replaced> {
    let kind = Kind::of(id).ok_or(Error::NotFound)?;
    progress(Step::Checking);
    let (me, item) = {
        let mut c = client.lock().unwrap();
        c.authenticated()?;
        let me = c.user.as_ref().map(|u| u.id.clone()).unwrap_or_default();
        (me, c.fresh(kind, id)?)
    };
    if item.author_id.is_empty() || item.author_id != me {
        return Err(Error::NotYours);
    }
    progress(Step::Preparing);
    let signature = signature(png);
    let parts = [("file", Part::new(png, "image/png")), ("signature", Part::new(&signature, "application/x-rsync-signature"))];
    let (file, version) = {
        let mut c = client.lock().unwrap();
        let existing = match file_id(&item.image_url) {
            Some(f) => match c.file(f) {
                Ok(file) if file.takes_a_version(&me) => Some(file.id),
                Ok(_) | Err(Error::NotFound) => None,
                Err(e) => return Err(e),
            },
            None => None,
        };
        let file = match existing {
            Some(f) => f,
            None => c.create_file(&format!("{} - {} - Image", kind.label(), item.name))?.id,
        };
        let version = c.add_version(&file, &parts[0].1, &parts[1].1)?;
        (file, version)
    };
    for ((name, part), step) in parts.iter().zip([Step::Uploading, Step::Signing]) {
        progress(step);
        let url = client.lock().unwrap().start_part(&file, version, name)?;
        put_part(client, &url, part)?;
        client.lock().unwrap().finish_part(&file, version, name)?;
    }
    progress(Step::Processing);
    let mut attempts = 0;
    loop {
        let status = {
            let mut c = client.lock().unwrap();
            let f = c.file(&file)?;
            f.versions.into_iter().find(|v| v.version == version).map(|v| v.status).unwrap_or_default()
        };
        match status.as_str() {
            "complete" => break,
            "waiting" | "queued" if attempts < 30 => {}
            "waiting" | "queued" => return Err(Error::Processing),
            _ => return Err(Error::InvalidResponse),
        }
        attempts += 1;
        // The client stays free meanwhile; a revoked grant ends the wait.
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(500));
            client.lock().unwrap().permitted()?;
        }
    }
    progress(Step::Saving);
    client.lock().unwrap().set_image(kind, id, &format!("{BASE}/file/{file}/{version}/file"))
}
/// Puts a part to the storage address VRChat just gave for it, without the client locked: no session cookie or
/// credential goes with it and none is taken from the answer, and VRChat's back-off is not touched by the storage's.
fn put_part(client: &Mutex<Client>, url: &str, part: &Part) -> Result<()> {
    if !upload_url_allowed(url) {
        return Err(Error::UnsafeUrl);
    }
    let (transport, gate) = {
        let c = client.lock().unwrap();
        c.authenticated()?;
        c.pace("uploads")?;
        (c.transport.clone(), c.allowed.clone())
    };
    let upload = Upload { mime: part.mime, md5: part.md5.clone(), bytes: part.bytes.to_vec() };
    let r = transport.send(Request { method: "PUT", url: url.into(), cookie: String::new(), basic: None, body: None, upload: Some(upload) })?;
    if !gate.allowed() {
        return Err(Error::Disabled);
    }
    match r.status {
        200..=299 => Ok(()),
        500..=599 => Err(Error::Unavailable),
        _ => Err(Error::Upload),
    }
}
/// VRChat hands out Amazon S3 addresses for uploads (presigned for one object, valid minutes): HTTPS on the default
/// port, without credentials in the address. Anything else is refused before the picture leaves.
fn upload_url_allowed(url: &str) -> bool {
    let Ok(uri) = url.parse::<ureq::http::Uri>() else { return false };
    let host = uri.host().unwrap_or_default().to_ascii_lowercase();
    uri.scheme_str() == Some("https")
        && (host.ends_with(".amazonaws.com") || host == "files.vrchat.cloud")
        && uri.port_u16().is_none()
        && !uri.authority().is_some_and(|a| a.as_str().contains('@'))
}

/// The librsync signature VRChat's file versions keep beside each upload, as its SDK and VRCX make it (librsync.net's
/// defaults): a header of the BLAKE2 signature magic (`rs\x017`), the block length (2048) and the strong-sum length
/// (32), then for each 2 KiB block its rolling checksum and its BLAKE2b-256, all big-endian.
pub fn signature(data: &[u8]) -> Vec<u8> {
    const MAGIC: u32 = 0x7273_0137;
    const BLOCK: usize = 2048;
    const STRONG: usize = 32;
    let mut out = Vec::with_capacity(12 + data.len().div_ceil(BLOCK) * (4 + STRONG));
    for word in [MAGIC, BLOCK as u32, STRONG as u32] {
        out.extend_from_slice(&word.to_be_bytes());
    }
    for block in data.chunks(BLOCK) {
        out.extend_from_slice(&rollsum(block).to_be_bytes());
        out.extend_from_slice(&blake2b_256(block)[..STRONG]);
    }
    out
}
/// librsync's weak checksum (`rollsum.c`, with its character offset of 31): the low halves of its two sums.
fn rollsum(block: &[u8]) -> u32 {
    const OFFSET: u64 = 31;
    let (mut s1, mut s2) = (0u64, 0u64);
    for &b in block {
        s1 = s1.wrapping_add(b as u64 + OFFSET);
        s2 = s2.wrapping_add(s1);
    }
    ((s2 << 16) | (s1 & 0xffff)) as u32
}
/// BLAKE2b (RFC 7693) with a 32-byte digest and no key: librsync's strong sum.
fn blake2b_256(data: &[u8]) -> [u8; 32] {
    const IV: [u64; 8] = [
        0x6a09e667f3bcc908,
        0xbb67ae8584caa73b,
        0x3c6ef372fe94f82b,
        0xa54ff53a5f1d36f1,
        0x510e527fade682d1,
        0x9b05688c2b3e6c1f,
        0x1f83d9abfb41bd6b,
        0x5be0cd19137e2179,
    ];
    const SIGMA: [[usize; 16]; 10] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
        [14, 10, 4, 8, 9, 15, 13, 6, 1, 12, 0, 2, 11, 7, 5, 3],
        [11, 8, 12, 0, 5, 2, 15, 13, 10, 14, 3, 6, 7, 1, 9, 4],
        [7, 9, 3, 1, 13, 12, 11, 14, 2, 6, 5, 10, 4, 0, 15, 8],
        [9, 0, 5, 7, 2, 4, 10, 15, 14, 1, 11, 12, 6, 8, 3, 13],
        [2, 12, 6, 10, 0, 11, 8, 3, 4, 13, 7, 5, 15, 14, 1, 9],
        [12, 5, 1, 15, 14, 13, 4, 10, 0, 7, 6, 3, 9, 2, 8, 11],
        [13, 11, 7, 14, 12, 1, 3, 9, 5, 0, 15, 4, 8, 6, 2, 10],
        [6, 15, 14, 9, 11, 3, 0, 8, 12, 2, 13, 7, 1, 4, 10, 5],
        [10, 2, 8, 4, 7, 6, 1, 5, 15, 11, 9, 14, 3, 12, 13, 0],
    ];
    fn compress(h: &mut [u64; 8], block: &[u8; 128], bytes: u128, last: bool) {
        let mut m = [0u64; 16];
        for (w, c) in m.iter_mut().zip(block.chunks_exact(8)) {
            *w = u64::from_le_bytes(c.try_into().unwrap());
        }
        let mut v = [0u64; 16];
        v[..8].copy_from_slice(h);
        v[8..].copy_from_slice(&IV);
        v[12] ^= bytes as u64;
        v[13] ^= (bytes >> 64) as u64;
        if last {
            v[14] = !v[14];
        }
        let mut g = |a: usize, b: usize, c: usize, d: usize, x: u64, y: u64| {
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(x);
            v[d] = (v[d] ^ v[a]).rotate_right(32);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(24);
            v[a] = v[a].wrapping_add(v[b]).wrapping_add(y);
            v[d] = (v[d] ^ v[a]).rotate_right(16);
            v[c] = v[c].wrapping_add(v[d]);
            v[b] = (v[b] ^ v[c]).rotate_right(63);
        };
        for round in 0..12 {
            let s = &SIGMA[round % 10];
            g(0, 4, 8, 12, m[s[0]], m[s[1]]);
            g(1, 5, 9, 13, m[s[2]], m[s[3]]);
            g(2, 6, 10, 14, m[s[4]], m[s[5]]);
            g(3, 7, 11, 15, m[s[6]], m[s[7]]);
            g(0, 5, 10, 15, m[s[8]], m[s[9]]);
            g(1, 6, 11, 12, m[s[10]], m[s[11]]);
            g(2, 7, 8, 13, m[s[12]], m[s[13]]);
            g(3, 4, 9, 14, m[s[14]], m[s[15]]);
        }
        for i in 0..8 {
            h[i] ^= v[i] ^ v[i + 8];
        }
    }
    let mut h = IV;
    // The parameter block: a 32-byte digest, no key, fanout and depth 1.
    h[0] ^= 0x0101_0000 ^ 32;
    let mut done = 0u128;
    let mut rest = data;
    // Every block but the last is compressed as it comes; the last (an empty one for empty data) is padded and final.
    while rest.len() > 128 {
        done += 128;
        compress(&mut h, rest[..128].try_into().unwrap(), done, false);
        rest = &rest[128..];
    }
    let mut last = [0u8; 128];
    last[..rest.len()].copy_from_slice(rest);
    compress(&mut h, &last, done + rest.len() as u128, true);
    let mut out = [0u8; 32];
    for (c, w) in out.chunks_exact_mut(8).zip(h) {
        c.copy_from_slice(&w.to_le_bytes());
    }
    out
}
fn image_url_allowed(url: &str) -> bool {
    let Ok(uri) = url.parse::<ureq::http::Uri>() else { return false };
    uri.scheme_str() == Some("https")
        && matches!(uri.host(), Some("api.vrchat.cloud" | "files.vrchat.cloud" | "assets.vrchat.com"))
        && uri.port_u16().is_none()
        && !uri.authority().is_some_and(|a| a.as_str().contains('@'))
}

#[cfg(test)]
pub(crate) mod tests {
    #[test]
    fn vrchats_other_rows_and_the_accounts_playlists_are_asked_as_its_client_asks_them() {
        use crate::maps::{Row, WorldList};
        assert_eq!(world_list_path(&WorldList::Row(Row::Games), 0), "/worlds?tag=author_tag_game&sort=heat&order=descending&releaseStatus=public&n=25&offset=0");
        assert_eq!(world_list_path(&WorldList::Row(Row::CommunityLabs), 2), "/worlds?sort=labs&order=descending&releaseStatus=public&n=25&offset=48");
        assert_eq!(world_list_path(&WorldList::Row(Row::New), 0), "/worlds?sort=publication&order=descending&releaseStatus=public&n=25&offset=0");
        assert_eq!(world_list_path(&WorldList::FavoriteGroup("worlds2".into()), 1), "/worlds/favorites?tag=worlds2&n=25&offset=24");
        let body = br#"[{"id":"fvgrp_1","name":"worlds1","displayName":"Chill spots","type":"world"},{"name":"avatars1","displayName":"Mine","type":"avatar"},{"name":"worlds2","displayName":"","type":"world"},{"name":"../x","type":"world"}]"#;
        assert_eq!(parse_favorite_groups(body).unwrap(), [("worlds1".to_string(), "Chill spots".to_string()), ("worlds2".to_string(), "worlds2".to_string())]);
    }

    #[test]
    fn only_the_strict_private_instance_of_its_owner_counts() {
        let me = "usr_1339e525-d75d-4832-9104-98dd89287681";
        assert!(private_instance_of(&format!("11027~private({me})~region(eu)"), me));
        assert!(private_instance_of(&format!("11027~private({me})~region(eu)~nonce(ab-cd)"), me));
        for other in [
            "12345".to_string(),
            "12345~region(eu)".to_string(),
            format!("11027~private({me})~canRequestInvite~region(eu)"),
            format!("11027~friends({me})~region(eu)"),
            format!("11027~hidden({me})~region(eu)"),
            "11027~group(grp_1)~groupAccessType(public)~region(eu)".to_string(),
            "11027~private(usr_someone-else)~region(eu)".to_string(),
            format!("~private({me})"),
            format!("11027~private({me}"),
            format!("11027~private({me})~whatever(next)"),
            String::new(),
        ] {
            assert!(!private_instance_of(&other, me), "{other}");
        }
        assert!(!private_instance_of("11027~private()~region(eu)", ""), "no account, no owner");
    }
    #[test]
    fn a_made_instance_is_read_only_when_it_is_the_private_one_asked_for() {
        let (world, me) = ("wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b", "usr_1");
        let instance = "98765~private(usr_1)~region(eu)~nonce(x)";
        let read = |v: serde_json::Value| created_instance(&v, world, me);
        assert_eq!(read(serde_json::json!({ "instanceId": instance, "worldId": world })).as_deref(), Some(instance));
        assert_eq!(read(serde_json::json!({ "location": format!("{world}:{instance}") })).as_deref(), Some(instance));
        assert_eq!(read(serde_json::json!({ "location": format!("wrld_other:{instance}") })), None);
        assert_eq!(read(serde_json::json!({ "instanceId": instance, "worldId": "wrld_other" })), None);
        assert_eq!(read(serde_json::json!({ "instanceId": "98765~region(eu)" })), None, "VRChat made a public one");
        assert_eq!(read(serde_json::json!({})), None);
    }
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    struct Mock {
        answers: Mutex<VecDeque<Response>>,
        seen: Arc<Mutex<Vec<Request>>>,
    }
    impl Transport for Mock {
        fn send(&self, r: Request) -> Result<Response> {
            self.seen.lock().unwrap().push(r);
            Ok(self.answers.lock().unwrap().pop_front().expect("unexpected HTTP request"))
        }
    }
    fn response(status: u16, body: &str, cookies: &[&str]) -> Response {
        Response { status, body: body.as_bytes().to_vec(), cookies: cookies.iter().map(|s| s.to_string()).collect(), retry_after: None, location: None }
    }
    fn mock(answers: Vec<Response>) -> (Client, Arc<Mutex<Vec<Request>>>) {
        let seen = Arc::new(Mutex::new(vec![]));
        let mut c = Client::new(Arc::new(Gate::for_duration(Duration::from_secs(3600))));
        c.transport = Arc::new(Mock { answers: Mutex::new(answers.into()), seen: seen.clone() });
        c.limiter = Default::default();
        c.store = None;
        c.user = Some(User { id: "usr_test".into(), display_name: "Tester".into(), ..Default::default() });
        c.cookies.auth = "mock-session".into();
        (c, seen)
    }
    pub(crate) fn expired_session() -> Client {
        mock(vec![response(401, "", &[])]).0
    }
    fn chooser_instance(id: &str, access: &str) -> serde_json::Value {
        serde_json::json!({"worldId":"wrld_test", "instanceId":id, "accessType":access, "region":"eu", "n_users":3,
            "capacity":20,"full":false,"active":true,"ownerId":if access == "public" { serde_json::Value::Null } else if access.starts_with("group") { serde_json::json!("grp_test") } else { serde_json::json!("usr_test") }, "users":[{"isFriend":true},{"isFriend":false}]})
    }
    #[test]
    fn chooser_lists_exact_ids_kinds_and_world_details_without_a_capture() {
        let ids = ["123~region(eu)", "456~hidden(usr_test)~region(eu)~nonce(a)", "789~private(usr_test)~canRequestInvite~region(eu)~nonce(b)"];
        let body = serde_json::json!({"id":"wrld_test","name":"World","authorName":"Creator","description":"A place", "capacity":20,
            "tags":["author_tag_chill"],"visits":12,"favorites":7,"instances":ids.map(|id| serde_json::json!([id,3,{"en":1.0}]))});
        let mut answers = vec![json(body)];
        answers.extend(ids.iter().zip(["public","friendsPlus","invitePlus"]).map(|(id,a)| json(chooser_instance(id,a))));
        let (mut c, seen) = mock(answers);
        c.limiter.lock().unwrap().unpaced = true;
        let list = c.world_instances("wrld_test").unwrap();
        assert_eq!(list.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ids);
        assert_eq!(list.items[1].access, InstanceAccess::FriendsPlus);
        assert_eq!(list.items[2].access, InstanceAccess::InvitePlus);
        assert_eq!(list.items[0].friends, Some(1));
        assert!(list.items.iter().all(|i| i.can_join));
        let w = c.world("wrld_test").unwrap();
        assert_eq!((w.author_name.as_str(),w.description.as_str(),w.capacity,w.visits,w.favorites), ("Creator","A place",Some(20),Some(12),Some(7)));
        assert_eq!(seen.lock().unwrap().len(),4);
    }
    #[test]
    fn chooser_empty_hidden_expired_errors_and_no_session() {
        let (mut c, _) = mock(vec![json(serde_json::json!({"instances":[]}))]);
        assert!(c.world_instances("wrld_test").unwrap().items.is_empty());
        for status in [401,500,429] {
            let (mut c, _) = mock(vec![response(status,"",&[])]);
            let error = c.world_instances("wrld_test").unwrap_err();
            assert!(match status {401=>error==Error::Unauthorized,500=>error==Error::Unavailable,_=>matches!(error,Error::RateLimited(_))});
        }
        let (mut c, seen) = mock(vec![]);
        c.user = None;
        assert_eq!(c.world_instances("wrld_test"),Err(Error::Unauthorized));
        assert!(seen.lock().unwrap().is_empty());
        let (mut c, _) = mock(vec![json(serde_json::json!({"instances":[["1",0,{}],["2",0,{}]]})),response(403,"",&[]),json(serde_json::Value::Null)]);
        c.limiter.lock().unwrap().unpaced=true;
        let list=c.world_instances("wrld_test").unwrap();
        assert_eq!(list.unavailable,2);
        assert!(list.items.is_empty());
    }
    #[test]
    fn chooser_creation_bodies_and_exact_selected_invite() {
        for (access,kind,label,requestable) in [(CreateAccess::Public,"public","public",false),
            (CreateAccess::FriendsPlus,"hidden","friendsPlus",false),(CreateAccess::Friends,"friends","friends",false),
            (CreateAccess::InvitePlus,"private","invitePlus",true),(CreateAccess::Invite,"private","invite",false)] {
            let id = if access == CreateAccess::Public { "123~region(eu)".to_string() } else { format!("123~{kind}(usr_test){}~region(eu)~nonce(opaque)", if requestable { "~canRequestInvite" } else { "" }) };
            let (mut c,seen)=mock(vec![json(chooser_instance(&id,label)),response(200,"{}",&[])]);
            let created=c.create_instance("wrld_test",access,InstanceRegion::Eu).unwrap();
            c.invite_self("wrld_test",&created.id).unwrap();
            let calls=seen.lock().unwrap();
            let body:serde_json::Value=serde_json::from_str(calls[0].body.as_ref().unwrap()).unwrap();
            let mut expected=serde_json::json!({"worldId":"wrld_test","type":kind,"region":"eu","canRequestInvite":requestable});
            if access!=CreateAccess::Public {expected["ownerId"]="usr_test".into();}
            assert_eq!(body,expected);
            assert!(calls[1].url.ends_with(&format!("/invite/myself/to/wrld_test:{}",path_segment(&id))));
        }
    }
    /// VRChat's real answer to POST /instances (a Friends+ instance of Popcorn Palace in Europe, 2026-11-15, the fields
    /// the client reads): a new instance says `active: false` and carries no `closedAt`. It must come out joinable, or the
    /// invite or trip to it is refused as "no longer on the list" (Enzo's L-invite-error).
    #[test]
    fn a_new_instance_as_vrchat_answers_its_making_is_joinable() {
        let world = "wrld_266523e8-9161-40da-acd0-6bd82e075833";
        let id = "46657~hidden(usr_test)~region(eu)";
        let answer = serde_json::json!({"accessType":"friendsPlus","active":false,"canRequestInvite":false,"capacity":80,
            "full":false,"hidden":"usr_test","instanceId":id,"location":format!("{world}:{id}"),"n_users":0,"name":"46657",
            "nonce":"d5ec8417-3b1d-4af7-a840-c59518f97e6c","ownerId":"usr_test","permanent":true,"queueEnabled":false,
            "recommendedCapacity":40,"region":"eu","type":"hidden","userCount":0,"users":[],"worldId":world});
        let (mut c, _) = mock(vec![json(answer)]);
        let made = c.create_instance(world, CreateAccess::FriendsPlus, InstanceRegion::Eu).unwrap();
        assert_eq!(made.id, id);
        assert!(made.can_join && !made.closed && !made.full, "{made:?}");
    }
    #[test]
    fn chooser_full_closed_and_unknown_are_not_joinable() {
        for (field,value) in [("full",serde_json::json!(true)),("hardClose",serde_json::json!(true)),("closedAt",serde_json::json!("2026-01-01T00:00:00Z")),("hasCapacityForYou",serde_json::json!(false)),("accessType",serde_json::json!("new-kind"))] {
            let mut v=chooser_instance("1","friendsPlus"); v[field]=value;
            assert!(!instance_details(&v,"wrld_test",Some("1")).unwrap().can_join, "{field}");
        }
        // Nobody in it yet: VRChat says `active: false` and opens it when someone arrives.
        let mut v=chooser_instance("1","friendsPlus"); v["active"]=false.into(); v["n_users"]=0.into();
        let i=instance_details(&v,"wrld_test",Some("1")).unwrap();
        assert!(i.can_join && !i.closed, "an instance nobody entered yet is not closed");
        assert!(instance_details(&chooser_instance("2","public"),"wrld_test",Some("1")).is_err());
        assert!(instance_details(&chooser_instance("1","public"),"wrld_other",Some("1")).is_err());
    }
    #[test]
    fn chooser_scope_change_during_mocked_http_discards_the_answer() {
        struct ChangeScope { current: Arc<AtomicBool>, calls: Arc<std::sync::atomic::AtomicUsize> }
        impl Transport for ChangeScope {
            fn send(&self, _: Request) -> Result<Response> {
                self.calls.fetch_add(1, Ordering::Relaxed);
                self.current.store(false, Ordering::Release);
                Ok(json(serde_json::json!({"instances":[]})))
            }
        }
        let (mut c,_) = mock(vec![]);
        let current=Arc::new(AtomicBool::new(true));
        let calls=Arc::new(std::sync::atomic::AtomicUsize::new(0));
        c.transport=Arc::new(ChangeScope {current:current.clone(),calls:calls.clone()});
        let source=crate::vrchat_source::Source::new(Some(Arc::new(Mutex::new(c))),None,None);
        assert_eq!(source.personal("usr_test",|| current.load(Ordering::Acquire),|c| c.world_instances("wrld_test")),Err(Error::Disabled));
        assert_eq!(calls.load(Ordering::Relaxed),1);
        assert_eq!(source.personal("usr_test",|| false,|c| c.world_instances("wrld_test")),Err(Error::Disabled));
        assert_eq!(calls.load(Ordering::Relaxed),1,"stale work never reaches HTTP");
        assert_eq!(source.personal("usr_other",|| true,|c| c.world_instances("wrld_test")),Err(Error::Unauthorized));
    }


    #[test]
    fn review_all_access_kinds_preserve_coherent_opaque_ids() {
        for (fragment,label) in [("","public"),("~hidden(usr_test)","friendsPlus"),("~friends(usr_test)","friends"),("~private(usr_test)~canRequestInvite","invitePlus"),("~private(usr_test)","invite"),("~group(grp_test)~groupAccessType(public)","groupPublic"),("~group(grp_test)~groupAccessType(plus)","groupPlus"),("~group(grp_test)~groupAccessType(members)","group")] {
            let id=format!("123{fragment}~region(eu){}",if fragment.is_empty(){""}else{"~nonce(opaque)"});
            let (mut c,calls)=mock(vec![json(chooser_instance(&id,label))]);
            let row=c.world_instance("wrld_test",&id).unwrap().unwrap();
            assert_eq!(row.id,id);assert!(row.can_join);assert_ne!(row.access,InstanceAccess::Unknown);
            assert!(calls.lock().unwrap()[0].url.ends_with(&path_segment(&id)));
        }
    }
    #[test]
    fn review_large_listing_is_bounded_and_preserves_remaining_ids() {
        let ids:Vec<_>=(0..122).map(|n|format!("{n}~region(eu)")).collect();
        let mut answers=vec![json(serde_json::json!({"instances":ids.iter().map(|id|serde_json::json!([id,1,{}])).collect::<Vec<_>>()}))];
        answers.extend(ids.iter().take(16).map(|id|json(chooser_instance(id,"public"))));
        let (mut c,calls)=mock(answers);c.limiter.lock().unwrap().unpaced=true;
        let batch=c.world_instances("wrld_test").unwrap();
        assert_eq!(batch.items.len(),16);assert_eq!(batch.remaining,ids[16..]);assert_eq!(calls.lock().unwrap().len(),17);
    }
    #[test]
    fn review_future_closure_embedded_capacity_and_unknown_region() {
        let mut v=chooser_instance("1~region(moon)","public");
        v["closedAt"]="2099-01-01T00:00:00Z".into();v["region"]="moon".into();v.as_object_mut().unwrap().remove("capacity");v["world"]=serde_json::json!({"capacity":25});
        let i=instance_details(&v,"wrld_test",None).unwrap();assert!(!i.closed);assert_eq!(i.capacity,Some(25));assert_eq!(i.region,"moon");
    }
    #[test]
    fn review_revocation_during_pacing_prevents_create_and_invite_dispatch() {
        for invite in [false,true] {
            let (mut c,calls)=mock(vec![response(200,"{}",&[])]);
            let gate=c.allowed.clone();
            let operation=c.operation(Instant::now()+Duration::from_secs(60));
            c.limiter.lock().unwrap().next.insert(if invite {"invites"} else {"instances"},Instant::now()+Duration::from_millis(200));
            let pause = Arc::new(std::sync::Barrier::new(2));
            c.pacing_pause = Some(pause.clone());
            let worker=std::thread::spawn(move || c.with_operation(operation,|c| if invite {c.invite_self("wrld_test","123~region(eu)")} else {c.create_instance("wrld_test",CreateAccess::Public,InstanceRegion::Eu).map(drop)}));
            pause.wait();
            gate.cancel_operations();
            pause.wait();
            assert_eq!(worker.join().unwrap(),Err(Error::Disabled));
            assert!(calls.lock().unwrap().is_empty());
        }
    }
    #[test]
    fn world_lists_are_asked_a_page_at_a_time_and_kept() {
        use crate::maps::WorldList;
        assert_eq!(
            world_list_path(&WorldList::Search("  black cat & co ".into()), 1),
            "/worlds?search=black%20cat%20%26%20co&sort=relevance&releaseStatus=public&n=25&offset=24"
        );
        assert_eq!(world_list_path(&WorldList::Popular, 0), "/worlds?sort=popularity&order=descending&releaseStatus=public&n=25&offset=0");
        assert_eq!(world_list_path(&WorldList::Active, 2), "/worlds/active?n=25&offset=48");
        assert_eq!(world_list_path(&WorldList::Trending, 0), "/worlds?sort=heat&order=descending&releaseStatus=public&n=25&offset=0");
        assert_eq!(
            world_list_path(&WorldList::Spotlight(crate::maps::Spotlight::CrossPlatform), 1),
            "/worlds?tag=admin_spotlight_xplat&sort=updated&order=descending&releaseStatus=public&n=25&offset=24"
        );
        assert_eq!(world_list_path(&WorldList::Favorites, 999), "/worlds/favorites?n=25&offset=480", "paging stops at 20 pages");
        let long = "x".repeat(300);
        assert!(world_list_path(&WorldList::Search(long), 0).contains(&format!("search={}&", "x".repeat(100))));
        let id = |i: usize| format!("wrld_{:08x}-0000-4000-8000-000000000000", i);
        let page: Vec<serde_json::Value> = (0..25)
            .map(|i| {
                serde_json::json!({"id": id(i), "name": format!("World {i}"), "authorName": "Mei", "capacity": 32, "occupants": 4,
                "favorites": 120, "visits": 9988, "thumbnailImageUrl": "https://api.vrchat.cloud/api/1/image/file_a/1/256",
                "imageUrl": "http://insecure/x", "tags": ["author_tag_chill", "system_approved"]})
            })
            .chain([serde_json::json!({"id": "not a world", "name": "skip"})])
            .collect();
        let (mut c, seen) = mock(vec![response(200, &serde_json::to_string(&page).unwrap(), &[]), response(200, "[]", &[])]);
        let (worlds, more) = c.find_worlds(&WorldList::Search("cat".into()), 0).unwrap();
        assert!(more, "25 asked, 25 worlds: another page follows");
        assert_eq!(worlds.len(), 24);
        let w = &worlds[0];
        assert_eq!((w.world.world_id.as_str(), w.world.name.as_str(), w.world.author_name.as_deref()), (id(0).as_str(), "World 0", Some("Mei")));
        assert_eq!((w.world.capacity, w.occupants, w.world.favorites, w.world.visits), (Some(32), Some(4), Some(120), Some(9988)));
        assert_eq!(w.world.image_url, None, "plain http is not shown");
        assert_eq!(w.world.tags, ["author_tag_chill", "system_approved"]);
        // Asked again: kept, nothing sent; another page is another request.
        assert_eq!(c.find_worlds(&WorldList::Search("cat".into()), 0).unwrap().0.len(), 24);
        assert_eq!(c.find_worlds(&WorldList::Search("cat".into()), 1).unwrap(), (vec![], false));
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|r| r.method == "GET" && r.cookie.contains("mock-session")));
        assert!(calls[1].url.ends_with("offset=24"));
        drop(calls);
        // Signing out forgets them.
        c.forget().unwrap();
        assert_eq!(c.find_worlds(&WorldList::Popular, 0), Err(Error::Unauthorized));
    }
    #[test]
    fn an_invite_to_oneself_names_the_instance_as_vrchat_writes_it() {
        let (mut c, seen) = mock(vec![response(200, r#"{"id":"not_a"}"#, &[])]);
        c.invite_self("wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd", "48213~region(eu)").unwrap();
        let calls = seen.lock().unwrap();
        assert_eq!(calls[0].method, "POST");
        assert_eq!(calls[0].url, format!("{BASE}/invite/myself/to/wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd:48213~region(eu)"));
    }
    #[test]
    fn selecting_avatar_uses_authenticated_put() {
        let (mut c, seen) = mock(vec![response(200, "{}", &[])]);
        c.select_avatar("avtr_4432ea9b-729c-46e3-8eaf-846aa0a37fdd").unwrap();
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "PUT");
        assert!(calls[0].url.ends_with("/avatars/avtr_4432ea9b-729c-46e3-8eaf-846aa0a37fdd/select"));
        assert!(calls[0].cookie.contains("mock-session"));
    }
    #[test]
    fn selecting_avatar_refuses_missing_or_mismatched_playing_account() {
        for playing in [None, Some("usr_other")] {
            let (c, seen) = mock(vec![response(200, r#"{"id":"usr_test","displayName":"Tester"}"#, &[])]);
            let source = crate::vrchat_source::Source::new(Some(Arc::new(Mutex::new(c))),None,playing.map(str::to_owned));
            assert!(source.select_avatar("avtr_4432ea9b-729c-46e3-8eaf-846aa0a37fdd").is_err());
            assert!(seen.lock().unwrap().iter().all(|r| r.method == "GET" && r.url.ends_with("/auth/user")), "identity lookup is allowed; selection is not");
        }
    }
    #[test]
    fn source_routes_avatar_image_and_instance_through_the_personal_session() {
        let (c, seen) = mock(vec![
            response(200, r#"{"id":"avtr_a","name":"Avatar","thumbnailImageUrl":"https://api.vrchat.cloud/image"}"#, &[]),
            response(200, "pixels", &[]),
            response(200, r#"{"active":true}"#, &[]),
        ]);
        let source = crate::vrchat_source::Source::new(Some(Arc::new(Mutex::new(c))), None, None);
        assert!(source.direct_available());
        let avatar = source.avatar("avtr_a").unwrap().unwrap();
        assert_eq!(source.image(&avatar.thumbnail_image_url).unwrap(), b"pixels");
        assert_eq!(source.instance_alive("wrld_a", "123").unwrap(), Some(true));
        assert_eq!(source.instance_alive("wrld_a", "123").unwrap(), Some(true));
        let calls = seen.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|r| r.cookie.contains("mock-session") && r.basic.is_none()));
    }
    #[test]
    fn basic_encodes_special_characters_before_base64() {
        assert_eq!(
            String::from_utf8(STANDARD.decode(basic_header("a:b +é", "p:@/%").strip_prefix("Basic ").unwrap()).unwrap()).unwrap(),
            "a%3Ab%20%2B%C3%A9:p%3A%40%2F%25"
        );
    }
    #[test]
    fn parses_each_two_factor_method() {
        for (wire, method) in [("totp", TwoFactor::Totp), ("otp", TwoFactor::Recovery), ("emailOtp", TwoFactor::Email)] {
            assert_eq!(parse_login(format!(r#"{{"requiresTwoFactorAuth":["{wire}"]}}"#).as_bytes()).unwrap(), Login::TwoFactor(vec![method]));
        }
        assert!(parse_login(br#"{"requiresTwoFactorAuth":["unknown"]}"#).is_err());
    }
    #[test]
    fn extracts_only_session_cookies_and_serializes() {
        let mut c = Cookies::default();
        c.absorb(&["auth=a=b; Path=/; HttpOnly".into(), "twoFactorAuth=second; Secure".into(), "other=ignored".into(), "auth=bad\r\nCookie: stolen".into()]);
        let c: Cookies = serde_json::from_slice(&serde_json::to_vec(&c).unwrap()).unwrap();
        assert_eq!(c.header(), "auth=a=b; twoFactorAuth=second");
    }
    #[test]
    fn limiter_separates_families_and_honors_backoff() {
        let now = Instant::now();
        let mut l = Limiter::default();
        l.used("auth", now);
        assert_eq!(l.wait("auth", now), Duration::from_secs(1));
        assert_eq!(l.wait("avatars", now), Duration::ZERO);
        l.blocked = Some(now + Duration::from_secs(120));
        assert_eq!(l.wait("avatars", now), Duration::from_secs(120));
        assert_eq!(retry_after(Some("73")), Duration::from_secs(73));
        assert_eq!(retry_after(Some("Wed, 21 Oct 2015 07:28:00 GMT")), Duration::from_secs(1));
    }
    #[test]
    fn gate_prevents_network_and_cached_data() {
        let (mut c, seen) = mock(vec![]);
        c.avatars.insert("avtr_a".into(), (Instant::now(), Avatar::default()));
        c.friends = Some((Instant::now(), vec![User::default()]));
        c.allowed.revoke();
        assert_eq!(c.sign_in("u", "p"), Err(Error::Disabled));
        assert_eq!(c.avatar("avtr_a"), Err(Error::Disabled));
        assert_eq!(c.friends(), Err(Error::Disabled));
        assert!(c.restore() == Err(Error::Disabled));
        assert!(seen.lock().unwrap().is_empty());
    }
    #[test]
    fn full_two_factor_flow_reuses_cookies_without_repeating_password() {
        for method in [TwoFactor::Totp, TwoFactor::Email, TwoFactor::Recovery] {
            let (mut c, seen) = mock(vec![
                response(
                    200,
                    &format!(
                        r#"{{"requiresTwoFactorAuth":["{}"]}}"#,
                        match method {
                            TwoFactor::Totp => "totp",
                            TwoFactor::Email => "emailOtp",
                            TwoFactor::Recovery => "otp",
                        }
                    ),
                    &["auth=session; HttpOnly"],
                ),
                response(200, r#"{"verified":true}"#, &["twoFactorAuth=factor; HttpOnly"]),
                response(200, r#"{"id":"usr_test","displayName":"Tester"}"#, &[]),
            ]);
            assert!(matches!(c.sign_in("user", "pass"), Ok(Login::TwoFactor(_))));
            assert!(matches!(c.verify(method, "123456"), Ok(Login::SignedIn(_))));
            let seen = seen.lock().unwrap();
            assert!(seen[0].basic.is_some());
            assert!(seen[1].basic.is_none() && seen[2].basic.is_none());
            assert_eq!(seen[1].cookie, "auth=session");
            assert_eq!(seen[2].cookie, "auth=session; twoFactorAuth=factor");
            assert!(seen[1].url.ends_with(&format!("/{}/verify", method.path())));
        }
    }
    #[test]
    fn expired_session_and_rate_limit_are_actionable() {
        let (mut c, _) = mock(vec![response(401, "", &[])]);
        assert_eq!(c.current_user(), Err(Error::Unauthorized));
        let mut r = response(429, "", &[]);
        r.retry_after = Some("20".into());
        let (mut c, seen) = mock(vec![r]);
        assert!(matches!(c.current_user(), Err(Error::RateLimited(_))));
        assert!(matches!(c.current_user(), Err(Error::RateLimited(_))));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
    #[test]
    fn avatars_are_cached_and_untrusted_images_never_get_cookies() {
        let (mut c, seen) = mock(vec![response(200, r#"{"id":"avtr_a","name":"A"}"#, &[])]);
        c.avatar("avtr_a").unwrap();
        c.avatar("avtr_a").unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1);
        for u in ["https://api.orbiters.cc/image", "https://api.vrchat.cloud.evil/a", "https://evil@api.vrchat.cloud/a", "http://api.vrchat.cloud/a"] {
            assert_eq!(c.image(u), Err(Error::UnsafeUrl));
        }
    }
    #[test]
    fn transport_has_no_orbiters_api_or_credential_path() {
        let source = include_str!("vrchat_api.rs").split("#[cfg(test)]").next().unwrap();
        assert!(!source.contains("account::") && !source.contains("crate::account") && !source.contains("Bearer "));
    }
    #[test]
    fn expired_grants_and_revoked_in_flight_requests_stay_disabled() {
        let (mut c, seen) = mock(vec![]);
        c.allowed = Arc::new(Gate::for_duration(Duration::ZERO));
        assert_eq!(c.current_user(), Err(Error::Disabled));
        assert!(seen.lock().unwrap().is_empty());
        struct Revoke(Arc<Gate>);
        impl Transport for Revoke {
            fn send(&self, _: Request) -> Result<Response> {
                self.0.revoke();
                Ok(response(200, r#"{"id":"usr_test","displayName":"Tester"}"#, &["auth=must-not-be-kept"]))
            }
        }
        c.allowed = Arc::new(Gate::for_duration(Duration::from_secs(3600)));
        c.transport = Arc::new(Revoke(c.allowed.clone()));
        assert_eq!(c.sign_in("user", "password"), Err(Error::Disabled));
        assert!(c.cookies.auth.is_empty());
        assert!(!c.signed_in());
    }
    #[test]
    fn wrong_code_keeps_the_challenge_and_unrequested_methods_send_nothing() {
        let (mut c, seen) = mock(vec![
            response(200, r#"{"requiresTwoFactorAuth":["emailOtp"]}"#, &["auth=session"]),
            response(400, "", &[]),
            response(200, r#"{"verified":false}"#, &[]),
        ]);
        c.sign_in("user", "password").unwrap();
        assert_eq!(c.verify(TwoFactor::Totp, "123456"), Err(Error::InvalidResponse));
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(c.verify(TwoFactor::Email, "bad"), Err(Error::WrongCode));
        assert_eq!(c.verify(TwoFactor::Email, "bad"), Err(Error::WrongCode));
        assert_eq!(c.cookies.auth, "session");
        assert_eq!(c.pending_methods, [TwoFactor::Email]);
    }
    #[test]
    fn an_expired_second_factor_session_returns_to_signed_out() {
        let (mut c, _) = mock(vec![response(200, r#"{"requiresTwoFactorAuth":["totp"]}"#, &["auth=session"]), response(401, "", &[])]);
        c.sign_in("test", "test").unwrap();
        assert_eq!(c.verify(TwoFactor::Totp, "123456"), Err(Error::Unauthorized));
        assert!(c.cookies.auth.is_empty() && c.pending_methods.is_empty() && !c.signed_in());
    }
    #[test]
    fn a_deleted_auth_cookie_invalidates_the_session_and_cached_data() {
        let (mut c, _) = mock(vec![response(200, r#"{"id":"avtr_a","name":"A"}"#, &["auth=deleted; Max-Age=-1"])]);
        assert_eq!(c.avatar("avtr_a"), Err(Error::Unauthorized));
        assert!(c.cookies.auth.is_empty() && c.avatars.is_empty() && !c.signed_in());
    }
    #[test]
    fn verification_is_not_repeated_after_a_failed_user_lookup() {
        let (mut c, seen) = mock(vec![
            response(200, r#"{"requiresTwoFactorAuth":["otp"]}"#, &["auth=session"]),
            response(200, r#"{"verified":true}"#, &["twoFactorAuth=verified"]),
            response(503, "", &[]),
            response(200, r#"{"id":"usr_test","displayName":"Tester"}"#, &[]),
        ]);
        c.sign_in("user", "password").unwrap();
        assert_eq!(c.verify(TwoFactor::Recovery, "one-use-code"), Err(Error::Unavailable));
        assert!(matches!(c.resume(), Ok(Some(Login::SignedIn(_)))));
        assert_eq!(seen.lock().unwrap().iter().filter(|r| r.method == "POST").count(), 1);
    }
    #[test]
    fn friends_page_both_states_deduplicate_and_cache_for_five_minutes() {
        let first: Vec<_> = (0..100).map(|i| serde_json::json!({"id":format!("usr_{i}"),"displayName":format!("Friend {i}")})).collect();
        let (mut c, seen) = mock(vec![
            response(200, &serde_json::to_string(&first).unwrap(), &[]),
            response(200, "[]", &[]),
            response(200, r#"[{"id":"usr_0","displayName":"Friend 0"},{"id":"usr_offline","displayName":"Offline"}]"#, &[]),
            response(200, "[]", &[]),
            response(200, "[]", &[]),
        ]);
        assert_eq!(c.friends().unwrap().len(), 101);
        assert_eq!(c.friends().unwrap().len(), 101);
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 3);
            assert!(seen[0].url.ends_with("offline=false&n=100&offset=0"));
            assert!(seen[1].url.ends_with("offline=false&n=100&offset=100"));
            assert!(seen[2].url.ends_with("offline=true&n=100&offset=0"));
        }
        c.friends.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(301);
        assert!(c.friends().unwrap().is_empty());
    }
    #[test]
    fn avatar_cache_expires_after_twenty_four_hours() {
        let (mut c, seen) = mock(vec![response(200, r#"{"id":"avtr_a","name":"Before"}"#, &[]), response(200, r#"{"id":"avtr_a","name":"After"}"#, &[])]);
        assert_eq!(c.avatar("avtr_a").unwrap().name, "Before");
        c.avatars.get_mut("avtr_a").unwrap().0 = Instant::now() - Duration::from_secs(86401);
        assert_eq!(c.avatar("avtr_a").unwrap().name, "After");
        assert_eq!(seen.lock().unwrap().len(), 2);
    }
    #[test]
    fn redirects_never_send_cookies_to_another_service_or_absorb_public_cdn_cookies() {
        let mut redirect = response(302, "", &[]);
        redirect.location = Some("https://api.orbiters.cc/image".into());
        let (mut c, seen) = mock(vec![redirect]);
        c.cookies.auth = "private-cookie".into();
        assert_eq!(c.image("https://api.vrchat.cloud/api/1/file/file_a/1/file"), Err(Error::UnsafeUrl));
        assert_eq!(seen.lock().unwrap().len(), 1);
        let mut redirect = response(302, "", &[]);
        redirect.location = Some("https://assets.vrchat.com/image.png".into());
        let (mut c, seen) = mock(vec![redirect, response(200, "image", &["auth=untrusted"])]);
        c.cookies.auth = "private-cookie".into();
        assert_eq!(c.image("https://api.vrchat.cloud/api/1/image/file_a/1/256").unwrap(), b"image");
        assert!(seen.lock().unwrap()[1].cookie.is_empty());
        assert_eq!(c.cookies.auth, "private-cookie");
    }
    #[test]
    fn instances_do_not_confuse_hidden_or_unknown_with_closed() {
        let (mut c, seen) =
            mock(vec![response(200, r#"{"active":true}"#, &[]), response(200, r#"{"active":false}"#, &[]), response(403, "", &[]), response(200, "null", &[])]);
        assert_eq!(c.instance_alive("wrld_a", "123"), Ok(Some(true)));
        assert_eq!(c.instance_alive("wrld_a", "123"), Ok(Some(true)));
        assert_eq!(seen.lock().unwrap().len(), 1);
        c.instances.get_mut(&("wrld_a".into(), "123".into())).unwrap().0 -= Duration::from_secs(61);
        assert_eq!(c.instance_alive("wrld_a", "123"), Ok(Some(false)));
        assert_eq!(c.instance_alive("wrld_a", "hidden"), Ok(None));
        // VRChat answers `null` for an instance it does not know: not open any more.
        assert_eq!(c.instance_alive("wrld_a", "unknown"), Ok(Some(false)));
        c.allowed.revoke();
        assert_eq!(c.instance_alive("wrld_a", "123"), Err(Error::Disabled));
    }
    #[test]
    fn instances_are_asked_for_as_vrchat_writes_them() {
        let instance = "25332~hidden(usr_c1644b5b-3ca4-45b4-97c6-a2a0de70d469)~region(eu)~nonce(9f0e1d2c-3b4a-5968-7766-554433221100)";
        let (mut c, seen) = mock(vec![response(404, r#"{"error":{"message":"Instance not found․","status_code":404}}"#, &[])]);
        // A closed instance VRChat no longer knows is not open any more, rather than an error.
        assert_eq!(c.instance_alive("wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b", instance), Ok(Some(false)));
        assert_eq!(seen.lock().unwrap()[0].url, format!("{BASE}/instances/wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b:{instance}"));
        assert_eq!(path_segment("a b/c?d#e%f"), "a%20b%2Fc%3Fd%23e%25f");
    }
    #[test]
    fn instance_answers_open_closed_or_say_why_not() {
        let (mut c, _) = mock(vec![
            response(200, r#"{"id":"wrld_a:1","instanceId":"1","n_users":3}"#, &[]),
            response(200, r#"{"id":"wrld_a:2","active":true,"closedAt":"2026-10-30T10:00:00.000Z"}"#, &[]),
            response(200, r#"{"id":"wrld_a:3","hardClose":true}"#, &[]),
            response(400, r#"{"error":{"message":"\"malformed url\"","status_code":400,"waf_code":26497}}"#, &[]),
            response(200, "<html>", &[]),
            response(302, "", &[]),
        ]);
        assert_eq!(c.instance_alive("wrld_a", "1"), Ok(Some(true)), "`active` is optional and true by default");
        assert_eq!(c.instance_alive("wrld_a", "2"), Ok(Some(false)));
        assert_eq!(c.instance_alive("wrld_a", "3"), Ok(Some(false))); // i18n: not shown (inside a test the scan cannot skip)
        let refused = c.instance_alive("wrld_a", "4").unwrap_err();
        assert_eq!(refused, Error::Refused(400, "malformed url".into()));
        assert_eq!(refused.to_string(), "VRChat refused the request (HTTP 400: malformed url).");
        assert_eq!(c.instance_alive("wrld_a", "5"), Err(Error::InvalidResponse));
        assert_eq!(c.instance_alive("wrld_a", "6"), Err(Error::Refused(302, String::new())));
        assert!(!c.instances.contains_key(&("wrld_a".into(), "4".into())), "errors are not cached"); // i18n: not shown (inside a test the scan cannot skip)
    }
    #[cfg(windows)]
    #[test]
    fn encrypted_store_rotates_restores_and_is_deleted_even_when_logout_fails() {
        let path = std::env::temp_dir().join(format!("orbiters-vrchat-test-{}.bin", uuid::Uuid::new_v4()));
        let (mut c, _) = mock(vec![
            response(200, r#"{"id":"usr_test","displayName":"Tester"}"#, &["auth=initial"]), // i18n: not shown (inside a test the scan cannot skip)
            response(200, r#"{"id":"avtr_a","name":"A"}"#, &["auth=rotated"]),
            response(503, "", &[]),
        ]);
        c.store = Some(path.clone());
        c.sign_in("test-user", "never-persist-this-password").unwrap(); // i18n: not shown (inside a test the scan cannot skip)
        c.avatar("avtr_a").unwrap();
        let encrypted = std::fs::read(&path).unwrap();
        let plain = crate::dpapi::crypt(&encrypted, ENTROPY, false).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(v.as_object().unwrap().len(), 2);
        assert_eq!(v["auth"], "rotated");
        assert!(!String::from_utf8_lossy(&plain).contains("never-persist")); // i18n: not shown (inside a test the scan cannot skip)
        let (mut restored, seen) = mock(vec![response(200, r#"{"id":"usr_test","displayName":"Tester"}"#, &[])]); // i18n: not shown (inside a test the scan cannot skip)
        restored.cookies = Cookies::default();
        restored.user = None;
        restored.store = Some(path.clone());
        restored.resume().unwrap();
        assert_eq!(seen.lock().unwrap()[0].cookie, "auth=rotated");
        assert!(seen.lock().unwrap()[0].basic.is_none());
        assert_eq!(c.sign_out(), Err(Error::Unavailable));
        assert!(!path.exists());
        assert!(!c.signed_in() && c.cookies.header().is_empty());
    }
    #[cfg(windows)]
    #[test]
    fn dpapi_round_trip_and_wrong_entropy() {
        let plain = br#"{"auth":"test-cookie","two_factor_auth":"test-factor"}"#;
        let sealed = crate::dpapi::crypt(plain, ENTROPY, true).unwrap();
        assert_ne!(sealed, plain);
        assert_eq!(crate::dpapi::crypt(&sealed, ENTROPY, false).unwrap(), plain);
        assert!(crate::dpapi::crypt(&sealed, b"other", false).is_err());
    }

    fn unpaced(answers: Vec<Response>) -> (Client, Arc<Mutex<Vec<Request>>>) {
        let (c, seen) = mock(answers);
        c.limiter.lock().unwrap().unpaced = true;
        (c, seen)
    }
    fn json(value: serde_json::Value) -> Response {
        response(200, &value.to_string(), &[])
    }
    const FILE: &str = "https://api.vrchat.cloud/api/1/file/file_0a1b-2c3d";
    const STORAGE: &str = "https://s3.us-east-1.amazonaws.com/files.vrchat.cloud/file_0a1b-2c3d.4.png?X-Amz-Signature=abc";
    const STORAGE_SIGNATURE: &str = "https://s3.us-east-1.amazonaws.com/files.vrchat.cloud/file_0a1b-2c3d.4.sig?X-Amz-Signature=def";
    /// What VRChat answers to replacing avatar `avtr_a`'s picture (version 3 of its file) with a fourth version.
    fn avatar_upload_answers() -> Vec<Response> {
        vec![
            json(serde_json::json!({"id":"avtr_a","name":"Mine","authorId":"usr_test","imageUrl":format!("{FILE}/3/file")})),
            json(
                serde_json::json!({"id":"file_0a1b-2c3d","ownerId":"usr_test","mimeType":"image/png","versions":[{"version":0,"status":"complete"},{"version":3,"status":"complete"}]}),
            ),
            json(
                serde_json::json!({"id":"file_0a1b-2c3d","ownerId":"usr_test","mimeType":"image/png","versions":[{"version":3,"status":"complete"},{"version":4,"status":"waiting"}]}),
            ),
            json(serde_json::json!({ "url": STORAGE })),
            // The storage's own cookie is never taken.
            response(200, "", &["auth=from-storage"]),
            response(200, "{}", &[]),
            json(serde_json::json!({ "url": STORAGE_SIGNATURE })),
            response(200, "", &[]),
            response(200, "{}", &[]),
            json(serde_json::json!({"id":"file_0a1b-2c3d","versions":[{"version":3,"status":"complete"},{"version":4,"status":"complete"}]})),
            json(
                serde_json::json!({"id":"avtr_a","name":"Mine","authorId":"usr_test","imageUrl":format!("{FILE}/4/file"),"thumbnailImageUrl":"https://api.vrchat.cloud/api/1/image/file_0a1b-2c3d/4/256"}),
            ),
        ]
    }
    #[test]
    fn replacing_a_picture_adds_a_version_uploads_both_parts_and_points_the_avatar_at_it() {
        let png = b"not a real PNG, any bytes do: ".repeat(150);
        let (c, seen) = unpaced(avatar_upload_answers());
        let client = Mutex::new(c);
        let mut steps = vec![];
        let replaced = replace_image(&client, "avtr_a", &png, |s| steps.push(s)).unwrap();
        assert_eq!(replaced.image_url, format!("{FILE}/4/file"));
        assert_eq!(replaced.thumbnail_image_url, "https://api.vrchat.cloud/api/1/image/file_0a1b-2c3d/4/256");
        assert_eq!(steps, [Step::Checking, Step::Preparing, Step::Uploading, Step::Signing, Step::Processing, Step::Saving]);
        let seen = seen.lock().unwrap();
        let calls: Vec<String> = seen.iter().map(|r| format!("{} {}", r.method, r.url)).collect();
        assert_eq!(
            calls,
            [
                format!("GET {BASE}/avatars/avtr_a"),
                format!("GET {FILE}"),
                format!("POST {FILE}"),
                format!("PUT {FILE}/4/file/start"),
                format!("PUT {STORAGE}"),
                format!("PUT {FILE}/4/file/finish"),
                format!("PUT {FILE}/4/signature/start"),
                format!("PUT {STORAGE_SIGNATURE}"),
                format!("PUT {FILE}/4/signature/finish"),
                format!("GET {FILE}"),
                format!("PUT {BASE}/avatars/avtr_a"),
            ]
        );
        let body = |i: usize| serde_json::from_str::<serde_json::Value>(seen[i].body.as_deref().unwrap()).unwrap();
        let sig = signature(&png);
        assert_eq!(
            body(2),
            serde_json::json!({"fileMd5": md5_base64(&png), "fileSizeInBytes": png.len(), "signatureMd5": md5_base64(&sig), "signatureSizeInBytes": sig.len()})
        );
        assert_eq!(body(5), serde_json::json!({"maxParts": "0", "nextPartNumber": "0"}));
        assert_eq!(body(8), body(5));
        assert_eq!(body(10), serde_json::json!({"id": "avtr_a", "imageUrl": format!("{FILE}/4/file")}));
        // The storage gets the bytes alone, typed and with their MD5; never a cookie or a credential.
        for (r, bytes, mime) in [(&seen[4], &png[..], "image/png"), (&seen[7], &sig[..], "application/x-rsync-signature")] {
            let u = r.upload.as_ref().unwrap();
            assert!(r.cookie.is_empty() && r.basic.is_none() && r.body.is_none());
            assert_eq!((u.mime, u.md5.as_str(), &u.bytes[..]), (mime, md5_base64(bytes).as_str(), bytes));
        }
        assert!(seen.iter().filter(|r| r.upload.is_none()).all(|r| r.cookie == "auth=mock-session" && r.basic.is_none()));
        let c = client.lock().unwrap();
        assert_eq!(c.cookies.auth, "mock-session");
        assert_eq!(c.avatars["avtr_a"].1.image_url, format!("{FILE}/4/file"));
    }
    #[test]
    fn someone_elses_avatar_is_refused_before_anything_is_sent() {
        let (c, seen) = unpaced(vec![json(serde_json::json!({"id":"avtr_a","name":"Theirs","authorId":"usr_other","imageUrl":format!("{FILE}/1/file")}))]);
        let client = Mutex::new(c);
        assert_eq!(replace_image(&client, "avtr_a", b"picture", |_| {}), Err(Error::NotYours));
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(replace_image(&client, "usr_test", b"picture", |_| {}), Err(Error::NotFound));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
    #[test]
    fn a_file_left_unfinished_gets_a_new_file_and_a_foreign_storage_address_gets_nothing() {
        let (c, seen) = unpaced(vec![
            json(serde_json::json!({"id":"wrld_a","name":"Moon","authorId":"usr_test","imageUrl":format!("{FILE}/2/file")})),
            json(serde_json::json!({"id":"file_0a1b-2c3d","ownerId":"usr_test","mimeType":"image/png","versions":[{"version":2,"status":"waiting"}]})),
            json(serde_json::json!({"id":"file_new","ownerId":"usr_test","mimeType":"image/png","versions":[{"version":0,"status":"complete"}]})),
            json(
                serde_json::json!({"id":"file_new","ownerId":"usr_test","mimeType":"image/png","versions":[{"version":0,"status":"complete"},{"version":1,"status":"waiting"}]}),
            ),
            json(serde_json::json!({"url": "https://uploads.example.com/file_new.png"})),
        ]);
        let client = Mutex::new(c);
        assert_eq!(replace_image(&client, "wrld_a", b"picture", |_| {}), Err(Error::UnsafeUrl));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 5);
        assert_eq!(seen[0].url, format!("{BASE}/worlds/wrld_a"));
        assert_eq!((seen[2].method, seen[2].url.as_str()), ("POST", format!("{BASE}/file").as_str()));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(seen[2].body.as_deref().unwrap()).unwrap(),
            serde_json::json!({"name": "World - Moon - Image", "mimeType": "image/png", "extension": ".png"})
        );
        assert_eq!(seen[3].url, format!("{BASE}/file/file_new"));
        assert_eq!(seen[4].url, format!("{BASE}/file/file_new/1/file/start"));
    }
    #[test]
    fn a_revoked_grant_stops_an_upload_at_its_next_step() {
        let (c, seen) = unpaced(avatar_upload_answers());
        let gate = c.allowed.clone();
        let client = Mutex::new(c);
        let result = replace_image(&client, "avtr_a", b"picture", |s| {
            if s == Step::Signing {
                gate.revoke();
            }
        });
        assert_eq!(result, Err(Error::Disabled));
        // The avatar, its file, the version, then the picture's start, upload and finish; the signature never starts.
        assert_eq!(seen.lock().unwrap().len(), 6);
    }
    #[test]
    fn ownership_compares_the_author_with_the_session_and_is_cached() {
        let (mut c, seen) = unpaced(vec![
            json(serde_json::json!({"id":"avtr_a","name":"A","authorId":"usr_test"})),
            json(serde_json::json!({"id":"wrld_b","name":"B","authorId":"usr_other"})),
            response(404, "", &[]),
            json(serde_json::json!({"id":"avtr_c","name":"C"})),
        ]);
        assert_eq!(c.owns("avtr_a"), Ok(true));
        assert_eq!(c.owns("avtr_a"), Ok(true));
        assert_eq!(c.owns("wrld_b"), Ok(false));
        assert_eq!(c.owns("wrld_gone"), Ok(false));
        assert_eq!(c.owns("avtr_c"), Ok(false));
        assert_eq!(c.owns("usr_test"), Ok(false));
        assert_eq!(seen.lock().unwrap().len(), 4);
        c.allowed.revoke();
        assert_eq!(c.owns("avtr_a"), Err(Error::Disabled));
    }
    #[test]
    fn storage_addresses_and_file_ids_are_read_strictly() {
        for ok in [STORAGE, "https://files.vrchat.cloud.s3.amazonaws.com/a.png?sig=1", "https://files.vrchat.cloud/upload/a"] {
            assert!(upload_url_allowed(ok), "{ok}");
        }
        for bad in [
            "http://s3.amazonaws.com/a",
            "https://amazonaws.com.evil.com/a",
            "https://s3.amazonaws.com:8443/a",
            "https://user@s3.amazonaws.com/a",
            "https://api.orbiters.cc/a",
            "not a url",
        ] {
            assert!(!upload_url_allowed(bad), "{bad}");
        }
        assert_eq!(file_id(&format!("{FILE}/3/file")), Some("file_0a1b-2c3d"));
        assert_eq!(file_id("https://api.vrchat.cloud/api/1/image/file_ab-12/1/256?x=1"), Some("file_ab-12"));
        assert_eq!(file_id("https://files.vrchat.cloud/thumbnails/x.png"), None);
        assert_eq!(file_id("https://api.vrchat.cloud/api/1/file/file_../1/file"), None);
    }
    /// A deterministic input, also given to the Python reference that produced the expected values below
    /// (`hashlib.blake2b(digest_size=32)`, `hashlib.md5`, and librsync.net's `Rollsum.Update` and signature writer
    /// transcribed).
    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 + i / 13) as u8).collect()
    }
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
    #[test]
    fn blake2b_256_and_md5_match_the_reference() {
        assert_eq!(hex(&blake2b_256(b"")), "0e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a8");
        assert_eq!(hex(&blake2b_256(b"abc")), "bddd813c634239723171ef3fee98579b94964e3bb1cb3e427262c8c068d52319");
        assert_eq!(hex(&blake2b_256(&pattern(128))), "e72160789c9e7f92097993129753cf19d63dd5e7e7a752a5900a45d7077a25b9");
        assert_eq!(hex(&blake2b_256(&pattern(129))), "170c01ceb5d6879b5f1ac36b5639dc80b276b79ba74b1db650c3c2b380163eba");
        assert_eq!(hex(&blake2b_256(&pattern(1000))), "a2475af68dc9c424af64e8703b88e465ead7f959ecd11fff50871d8a41f2a981");
        assert_eq!(md5_base64(b""), "1B2M2Y8AsgTpgAmY7PhCfg==");
        assert_eq!(md5_base64(b"abc"), "kAFQmDzST7DWlj99KOF/cg==");
        assert_eq!(md5_base64(b"The quick brown fox jumps over the lazy dog"), "nhB9nTcrtoJr2B01QqQZ1g==");
        // Around the padding's boundaries.
        for (n, md5) in
            [(55, "7jz+ubOTNhC0cycSxUFNyw=="), (56, "29Hc00Nb4aGlImKkspIqPw=="), (64, "rwPnqjfSUQYYOq1oMXGTvw=="), (119, "q1UULBvJk301vBXwUsy6fQ==")]
        {
            assert_eq!(md5_base64(&pattern(n)), md5, "{n} bytes");
        }
        assert_eq!(md5_base64(&pattern(5000)), "ECX7OHPPxsGEXJLtiZPZtQ==");
    }
    #[test]
    fn signatures_have_librsync_nets_layout_and_sums() {
        assert_eq!(rollsum(&pattern(5)), 0x025d00e1);
        assert_eq!(rollsum(&pattern(2048)), 0x6fc6ef29);
        assert_eq!(hex(&signature(&pattern(5))), "727301370000080000000020025d00e19cb9287b8139d67f72ec9d9486d820743163e5f4128f394b8368666aee839b9f");
        // Length and MD5 of whole signatures, for inputs around the block boundaries.
        for (n, len, md5) in [
            (0, 12, "wFTc7oMFDcoZnqdLNiwHBw=="),
            (1, 48, "C/T4GyLdmTdktnl/gji4tQ=="),
            (3, 48, "URpkolLJ8oy/1hyLsJ7OnA=="),
            (4, 48, "O4EYS7ZbPQRXwTPsCL8YTA=="),
            (2048, 48, "kG3LqRw5Sb9Ne853/PwN/w=="),
            (2049, 84, "TtBd/yvRGSZVXHrJ1JCirw=="),
            (5000, 120, "WT6uuGuLWfJdEjj8NSTgHA=="),
        ] {
            let s = signature(&pattern(n));
            assert_eq!((s.len(), md5_base64(&s).as_str()), (len, md5), "{n} bytes");
        }
    }
}

/// In-memory HTTP seam for cross-crate regression tests. No cookies or session files leave this fake.
#[cfg(feature = "test-support")]
pub fn test_client(gate: Arc<Gate>, replies: Vec<(u16, serde_json::Value)>) -> (Client, Arc<Mutex<Vec<(String, String)>>>) {
    struct Fake { replies: Mutex<std::collections::VecDeque<(u16, serde_json::Value)>>, calls: Arc<Mutex<Vec<(String, String)>>> }
    impl Transport for Fake {
        fn send(&self, r: Request) -> Result<Response> {
            self.calls.lock().unwrap().push((r.method.into(), r.url));
            let (status, body) = self.replies.lock().unwrap().pop_front().expect("unexpected HTTP");
            Ok(Response { status, body: body.to_string().into_bytes(), cookies: vec![], retry_after: None, location: None })
        }
    }
    let calls = Arc::new(Mutex::new(vec![]));
    let mut c = Client::new(gate);
    c.store = None;
    c.limiter = Arc::new(Mutex::new(Limiter { unpaced: true, ..Default::default() }));
    c.cookies.auth = "fake".into();
    c.user = Some(User { id: "usr_test".into(), ..Default::default() });
    c.transport = Arc::new(Fake { replies: Mutex::new(replies.into()), calls: calls.clone() });
    (c, calls)
}
