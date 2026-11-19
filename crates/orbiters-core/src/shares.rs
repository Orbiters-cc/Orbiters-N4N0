//! Capture sharing contract (`contracts/Docs/capture-sharing.md`). No VRChat credentials or messages.
use crate::account::{Api, ApiError};
use orbiters_i18n::{mark, tr};
use serde::{Deserialize, Serialize};

/// Shown as it is where a window shows it; `tr(EXPLICIT_WARNING)` gives it in the language now.
pub const EXPLICIT_WARNING: &str = mark("Any explicit content shared publicly will be reported to the VRChat staff.");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Capture,
    Picture,
}
impl Kind {
    pub fn path(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Picture => "picture",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Person {
    pub id: u64,
    pub name: String,
    pub avatar_url: Option<String>,
    pub vrchat_user_id: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct World {
    pub id: String,
    pub name: String,
    pub thumbnail_url: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Grant {
    pub id: u64,
    pub vrchat_user_id: String,
    pub display_name: Option<String>,
    pub granted_by: u64,
    pub parent_grant_id: Option<u64>,
    pub created_at: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Capture {
    pub splat_count: u64,
    pub preview_url: String,
    pub sky_url: Option<String>,
    /// Its picture (its 3D view) when the server has one (contracts `Docs/capture-sharing.md`, Capture picture).
    #[serde(default)]
    pub picture_url: Option<String>,
    pub view: serde_json::Value,
    pub vrchat_slot: Option<u32>,
    /// Its files' revision: 1 as uploaded, one more each time its owner replaced them after editing its splats
    /// (contracts `Docs/capture-sharing.md`, Updating a capture); 0 from a server that does not say ([`Capture::revision`]).
    #[serde(default)]
    pub revision: u64,
    /// When its files were last replaced (none until then).
    #[serde(default)]
    pub revised_at: Option<String>,
}
impl Capture {
    /// Its files' revision (a server listing none has only the files as uploaded: 1).
    pub fn revision(&self) -> u64 {
        self.revision.max(1)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Picture {
    pub url: String,
    pub thumbnail_url: String,
    pub width: u32,
    pub height: u32,
}
/// Read leniently (a field a server leaves out gets its default); in a list, an item of a kind this build does not
/// know, or without its id, is left out alone ([`Feed`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Item {
    #[serde(rename = "type")]
    pub kind: Kind,
    pub id: u64,
    pub world: World,
    pub owner: Person,
    pub from: Option<Person>,
    pub taken_at: Option<String>,
    pub instance_id: Option<String>,
    pub source_key: Option<String>,
    pub created_at: String,
    pub public: bool,
    #[serde(default)]
    pub grants: Vec<Grant>,
    pub mine: bool,
    pub can_share: bool,
    pub explicit: bool,
    #[serde(default)]
    pub explicit_reasons: Vec<String>,
    /// An official preview: a capture shared by the world's own VRChat author (contracts' Docs/capture-sharing.md).
    #[serde(default)]
    pub official: bool,
    pub hidden: Option<serde_json::Value>,
    pub capture: Option<Capture>,
    pub picture: Option<Picture>,
}
impl Item {
    pub fn key(&self) -> (Kind, u64) {
        (self.kind, self.id)
    }
    pub fn date(&self) -> &str {
        self.taken_at.as_deref().unwrap_or(&self.created_at)
    }
    /// Immediate local revoke. Keep a saved copy until the server reconciles the operation.
    pub fn revoke(&mut self, request: &Unshare) {
        if request.all || request.public {
            self.public = false;
        }
        let mut removed: Vec<u64> = self.grants.iter().filter(|g| request.all || request.recipients.contains(&g.vrchat_user_id)).map(|g| g.id).collect();
        loop {
            let before = removed.len();
            for g in &self.grants {
                if g.parent_grant_id.is_some_and(|p| removed.contains(&p)) && !removed.contains(&g.id) {
                    removed.push(g.id);
                }
            }
            if before == removed.len() {
                break;
            }
        }
        self.grants.retain(|g| !removed.contains(&g.id));
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Feed {
    #[serde(default, deserialize_with = "items_that_read")]
    pub items: Vec<Item>,
    #[serde(default)]
    pub next: Option<String>,
}

/// The elements of a list that read as `T` and pass `known` (a kind this build knows, an id): one the server added
/// later, or sent without a field this build needs, costs itself, not the page.
fn list_that_reads<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned>(d: D, known: fn(&serde_json::Value) -> bool) -> Result<Vec<T>, D::Error> {
    let values = Option::<Vec<serde_json::Value>>::deserialize(d)?.unwrap_or_default();
    let total = values.len();
    let out: Vec<T> = values.into_iter().filter(known).filter_map(|v| serde_json::from_value(v).ok()).collect();
    if out.len() < total {
        eprintln!("captures: {} of {total} left out (not read by this version)", total - out.len()); // i18n: not shown (a log's line)
    }
    Ok(out)
}

fn known_item(v: &serde_json::Value) -> bool {
    matches!(v.get("type").and_then(serde_json::Value::as_str), Some("capture" | "picture")) && v.get("id").is_some_and(serde_json::Value::is_u64)
}

fn known_request(v: &serde_json::Value) -> bool {
    matches!(v.get("type").and_then(serde_json::Value::as_str), Some("capture" | "picture"))
        && matches!(v.get("status").and_then(serde_json::Value::as_str), Some("pending" | "approved" | "denied" | "cancelled"))
        && v.get("id").is_some_and(serde_json::Value::is_u64)
}

fn items_that_read<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Item>, D::Error> {
    list_that_reads(d, known_item)
}

fn requests_that_read<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<ShareRequest>, D::Error> {
    list_that_reads(d, known_request)
}
impl Feed {
    /// Cursor is opaque. Deduplicate overlapping pages, and stop a repeated cursor rather than looping.
    pub fn append(&mut self, page: Feed) {
        for item in page.items {
            if let Some(old) = self.items.iter_mut().find(|i| i.key() == item.key()) {
                *old = item;
            } else {
                self.items.push(item);
            }
        }
        self.items.sort_by(|a, b| b.date().cmp(a.date()).then_with(|| b.created_at.cmp(&a.created_at)));
        self.next = page.next.filter(|n| Some(n) != self.next.as_ref());
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recipient {
    pub vrchat_user_id: String,
    pub display_name: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub public: bool,
    #[serde(default)]
    pub recipients: Vec<Recipient>,
    pub acknowledge_explicit: bool,
    pub message: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Unshare {
    pub public: bool,
    pub recipients: Vec<String>,
    pub all: bool,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestStatus {
    #[default]
    Pending,
    Approved,
    Denied,
    Cancelled,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Member {
    pub id: u64,
    pub name: String,
    pub avatar_url: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShareRequest {
    pub id: u64,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub item_id: u64,
    pub status: RequestStatus,
    pub public: bool,
    pub recipients: Vec<Recipient>,
    pub message: String,
    pub requester: Member,
    pub approver: Member,
    pub item: Option<Item>,
    pub created_at: String,
    pub answered_at: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Requests {
    #[serde(default, deserialize_with = "requests_that_read")]
    pub requests: Vec<ShareRequest>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestAnswer {
    pub request: ShareRequest,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemAnswer {
    pub item: Item,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Shared {
    Item { item: Item },
    Request { request: ShareRequest },
}
#[derive(Clone, Debug, Default)]
pub struct Query {
    pub filter: String,
    pub world_id: Option<String>,
    pub q: String,
    pub kind: Option<Kind>,
    pub cursor: Option<String>,
    pub limit: u8,
}

#[derive(Debug)]
pub struct ExplicitAcknowledgement(pub String);
impl std::fmt::Display for ExplicitAcknowledgement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ExplicitAcknowledgement {}

/// A replacement of a capture's files refused because they are not at the revision it named any more (contracts
/// `Docs/capture-sharing.md`, Updating a capture: 409 `revision-conflict`): replaced meanwhile, from elsewhere.
#[derive(Debug)]
pub struct RevisionConflict {
    /// The revision its files are at now.
    pub revision: u64,
    pub message: String,
}
impl std::fmt::Display for RevisionConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RevisionConflict {}

/// Orbiters refused a request for coming too often (429, `backend/src/middleware/requestLimit.js`: fixed windows per scope,
/// `Retry-After` set): nothing was done, and the same request works again once `retry_after_s` have passed. Its window
/// counts refused requests too, though they never lengthen it.
#[derive(Debug)]
pub struct RateLimited {
    pub retry_after_s: u64,
    pub message: String,
}
impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RateLimited {}

/// A `Retry-After` header's seconds (a date is not what Orbiters sends: a minute then).
pub fn retry_after_seconds(value: Option<&str>) -> u64 {
    value.and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(60).max(1)
}

/// Shared by uploads and sharing so a 409 keeps its machine-readable meaning, and a 429 its wait.
pub(crate) fn json<T: serde::de::DeserializeOwned>(response: ureq::http::Response<ureq::Body>) -> anyhow::Result<T> {
    let status = response.status().as_u16();
    let retry_after = response.headers().get("retry-after").and_then(|h| h.to_str().ok()).map(str::to_owned);
    let text = response.into_body().with_config().limit(32 * 1024 * 1024).read_to_string()?;
    if !(200..300).contains(&status) {
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let message = value["message"].as_str().or_else(|| value["error"].as_str()).unwrap_or(tr("Orbiters could not complete the request")).to_string();
        if status == 409 && value["code"] == "explicit-acknowledgement" {
            return Err(ExplicitAcknowledgement(message).into());
        }
        if status == 409 && value["code"] == "revision-conflict" {
            return Err(RevisionConflict { revision: value["revision"].as_u64().unwrap_or(0), message }.into());
        }
        if status == 429 {
            return Err(RateLimited { retry_after_s: retry_after_seconds(retry_after.as_deref()), message }.into());
        }
        return Err(ApiError { status, message, code: value["code"].as_str().map(str::to_string) }.into());
    }
    Ok(serde_json::from_str(&text)?)
}

/// [`json`] for a request that changes something: Orbiters said it was done (2xx) but its answer does not read (a
/// newer shape): done all the same, and what it is now is asked again with `again`, rather than reporting a failure
/// that would be tried again.
///
/// `as_asked` says whether an answer shows what was asked (the item or request asked about, in the state asked for).
/// The answer is taken only when it does; when it does not read, or names something else (an empty `{"item":{}}`
/// reads as a default item), what is there now is read again with `again`, and taken only when it shows what was
/// asked. Otherwise the outcome is not known: an error, and nothing is sent again by this call.
fn json_done<T: serde::de::DeserializeOwned>(
    response: ureq::http::Response<ureq::Body>,
    as_asked: impl Fn(&T) -> bool,
    again: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let done = response.status().is_success();
    match json::<T>(response) {
        Ok(answer) if as_asked(&answer) => Ok(answer),
        Err(e) if !done || e.downcast_ref::<serde_json::Error>().is_none() => Err(e),
        Ok(_) if !done => Err(anyhow::anyhow!("{}", tr("Orbiters could not complete the request"))),
        _ => {
            eprintln!("captures: done, its answer not what was asked or not read; reading it again"); // i18n: not shown (a log's line)
            let now = again()?;
            if as_asked(&now) {
                Ok(now)
            } else {
                eprintln!("captures: what was asked is not there (yet): outcome unknown, not sent again"); // i18n: not shown (a log's line)
                Err(anyhow::anyhow!("{}", tr("Orbiters could not complete the request")))
            }
        }
    }
}

/// Whether `item` is `kind` `id` as `share` asked: public when asked, every recipient asked among its grants. A share
/// that became a request to the owner (another member's item) is a `Shared::Request` for that item.
fn shared_as_asked(shared: &Shared, kind: Kind, id: u64, share: &Share) -> bool {
    match shared {
        Shared::Item { item } => {
            item.kind == kind
                && item.id == id
                && (!share.public || item.public)
                && share.recipients.iter().all(|r| item.grants.iter().any(|g| g.vrchat_user_id == r.vrchat_user_id))
        }
        Shared::Request { request } => request.id != 0 && request.kind == kind && request.item_id == id,
    }
}

/// Whether `item` is `kind` `id` with what `unshare` withdrew gone.
fn unshared_as_asked(item: &Item, kind: Kind, id: u64, unshare: &Unshare) -> bool {
    item.kind == kind
        && item.id == id
        && (!(unshare.all || unshare.public) || !item.public)
        && (if unshare.all { item.grants.is_empty() } else { unshare.recipients.iter().all(|r| item.grants.iter().all(|g| &g.vrchat_user_id != r)) })
}
/// Whether `address` is a 3D capture's picture on the server asked ([`Api::capture_picture`]): a path of its own API
/// (`/captures/capture/<id>/file?part=picture`, with its revision), never another host's address.
pub fn capture_picture_address(address: &str) -> bool {
    let Some(rest) = address.strip_prefix("/captures/capture/") else { return false };
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut parts = path.split('/');
    let id_ok = parts.next().is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
    id_ok && parts.next() == Some("file") && parts.next().is_none() && query.split('&').any(|p| p == "part=picture")
        && address.bytes().all(|b| b.is_ascii_alphanumeric() || b"/?&=_-.".contains(&b))
}
fn path(kind: Kind, id: u64) -> String {
    format!("captures/{}/{id}", kind.path())
}
impl Api {
    pub fn captures(&self, token: &str, query: &Query) -> anyhow::Result<Feed> {
        let mut r = self
            .agent()
            .get(&self.url("captures"))
            .header("Authorization", &format!("Bearer {token}"))
            .query("filter", &query.filter)
            .query("q", &query.q)
            .query("limit", query.limit.clamp(1, 50).to_string());
        if let Some(w) = &query.world_id {
            r = r.query("worldId", w);
        }
        if let Some(k) = query.kind {
            r = r.query("kind", k.path());
        }
        if let Some(c) = &query.cursor {
            r = r.query("cursor", c);
        }
        json(r.call()?)
    }
    /// A 3D capture's picture (its item's `capture.pictureUrl`), its 640-pixel thumbnail when `thumbnail`: what a card
    /// shows of a capture that is on Orbiters only (contracts `Docs/capture-sharing.md`, Capture picture). Asked with the
    /// account when signed in (a private capture, one shared with it), and only at an address of this server's captures:
    /// the credential never goes elsewhere.
    pub fn capture_picture(&self, token: Option<&str>, address: &str, thumbnail: bool) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(capture_picture_address(address), "not a capture picture's address: {address}"); // i18n: not shown (a server answering otherwise; only logged)
        let mut address = address.to_string();
        if thumbnail {
            address.push(if address.contains('?') { '&' } else { '?' });
            address.push_str("thumbnail=true");
        }
        // The picture itself can be large: a download's time; its thumbnail an API call's.
        let mut request = if thumbnail { self.agent().get(&self.url(&address)) } else { self.get_transfer(&self.url(&address)) };
        if let Some(token) = token {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let response = request.call()?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(ApiError { status, message: String::new(), code: None }.into());
        }
        Ok(response.into_body().with_config().limit(16 << 20).read_to_vec()?)
    }
    pub fn capture_item(&self, token: &str, kind: Kind, id: u64) -> anyhow::Result<Item> {
        Ok(json::<ItemAnswer>(self.agent().get(&self.url(&path(kind, id))).header("Authorization", &format!("Bearer {token}")).call()?)?.item)
    }
    /// A duplicate picture upload may be skipped without returning its id. Resolve its real identity, including
    /// older pages, instead of uploading again or matching dates/thumbnails.
    pub fn capture_by_source_key(&self, token: &str, world_id: &str, kind: Kind, source_key: &str) -> anyhow::Result<Option<Item>> {
        let mut query = Query { filter: "mine".into(), world_id: Some(world_id.into()), kind: Some(kind), limit: 50, ..Default::default() };
        let mut seen = std::collections::HashSet::new();
        loop {
            let page = self.captures(token, &query)?;
            if let Some(item) =
                page.items.into_iter().find(|i| i.mine && i.world.id == world_id && i.kind == kind && i.source_key.as_deref() == Some(source_key))
            {
                return Ok(Some(item));
            }
            match page.next {
                Some(next) if seen.insert(next.clone()) => query.cursor = Some(next),
                _ => return Ok(None),
            }
        }
    }
    pub fn share_capture(&self, token: &str, kind: Kind, id: u64, share: &Share) -> anyhow::Result<Shared> {
        anyhow::ensure!(share.public || !share.recipients.is_empty(), "{}", tr("Choose someone to share with."));
        anyhow::ensure!(
            share.recipients.len() <= 50 && share.message.chars().count() <= 280,
            "{}",
            tr("Choose at most 50 people and a message of at most 280 characters.")
        );
        let r = self.agent().post(&self.url(&format!("{}/share", path(kind, id)))).header("Authorization", &format!("Bearer {token}")).send_json(share)?;
        json_done(r, |s| shared_as_asked(s, kind, id, share), || Ok(Shared::Item { item: self.capture_item(token, kind, id)? }))
    }
    pub fn unshare_capture(&self, token: &str, kind: Kind, id: u64, unshare: &Unshare) -> anyhow::Result<Item> {
        #[derive(Deserialize)]
        struct Answer {
            item: Item,
        }
        let r = self
            .agent()
            .delete(&self.url(&format!("{}/share", path(kind, id))))
            .header("Authorization", &format!("Bearer {token}"))
            .force_send_body()
            .send_json(unshare)?;
        Ok(json_done(r, |a: &Answer| unshared_as_asked(&a.item, kind, id, unshare), || Ok(Answer { item: self.capture_item(token, kind, id)? }))?.item)
    }
    pub fn capture_file(&self, token: &str, kind: Kind, id: u64, out: &std::path::Path) -> anyhow::Result<()> {
        let r = self.get_transfer(&self.url(&format!("{}/file", path(kind, id)))).header("Authorization", &format!("Bearer {token}")).call()?;
        if !r.status().is_success() {
            json::<serde_json::Value>(r)?;
            unreachable!();
        }
        let mut file = std::fs::File::create(out)?;
        std::io::copy(&mut r.into_body().into_reader(), &mut file)?;
        Ok(())
    }
    pub fn capture_requests(&self, token: &str, incoming: bool) -> anyhow::Result<Requests> {
        json(
            self.agent()
                .get(&self.url("captures/requests"))
                .query("box", if incoming { "incoming" } else { "outgoing" })
                .header("Authorization", &format!("Bearer {token}"))
                .call()?,
        )
    }
    pub fn answer_capture_request(&self, token: &str, id: u64, approve: bool, acknowledge_explicit: bool) -> anyhow::Result<RequestAnswer> {
        let request = self
            .agent()
            .post(&self.url(&format!("captures/requests/{id}/{}", if approve { "approve" } else { "deny" })))
            .header("Authorization", &format!("Bearer {token}"));
        let r = if approve { request.send_json(serde_json::json!({"acknowledgeExplicit":acknowledge_explicit}))? } else { request.send_empty()? };
        // Answered as asked: approved, or denied; a request still pending is not.
        let wanted = if approve { RequestStatus::Approved } else { RequestStatus::Denied };
        json_done(
            r,
            |a: &RequestAnswer| a.request.id == id && a.request.status == wanted,
            || {
                let request = self.capture_requests(token, true)?.requests.into_iter().find(|r| r.id == id);
                request.map(|request| RequestAnswer { request }).ok_or_else(|| anyhow::anyhow!("{}", tr("Orbiters could not complete the request")))
            },
        )
    }
    pub fn moderate_capture(&self, token: &str, kind: Kind, id: u64, reason: Option<&str>) -> anyhow::Result<serde_json::Value> {
        json(
            self.agent()
                .post(&self.url(&format!("{}/{}", path(kind, id), if reason.is_some() { "hide" } else { "unhide" })))
                .header("Authorization", &format!("Bearer {token}"))
                .send_json(serde_json::json!({"reason":reason}))?,
        )
    }
    pub fn capture_presence(&self, token: &str, world_id: &str, instance_id: &str, display_name: &str) -> anyhow::Result<serde_json::Value> {
        json(
            self.agent()
                .post(&self.url("captures/vrchat/presence"))
                .header("Authorization", &format!("Bearer {token}"))
                .send_json(serde_json::json!({"worldId":world_id,"instanceId":instance_id,"displayName":display_name}))?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in Orbiters answering each request in turn with (status, body).
    fn answering(answers: Vec<(u16, String)>) -> Api {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for (status, body) in answers {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                while let Ok(n) = stream.read(&mut buffer) {
                    request.extend_from_slice(&buffer[..n]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                let _ = write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        Api::at(&base)
    }

    /// Verifier (CORE2-2): a share, unshare or answer whose answer does not show what was asked is taken only when what is
    /// there now shows it; an empty `{"item":{}}`, an unchanged item or a request still pending is not a success.
    #[test]
    fn a_change_is_reported_done_only_when_it_shows() {
        let grant = |user: &str| Grant { id: 1, vrchat_user_id: user.into(), ..Default::default() };
        let item = |grants: Vec<Grant>| serde_json::json!({"item": Item { kind: Kind::Capture, id: 3, mine: true, grants, ..Default::default() }}).to_string();
        let share = Share { recipients: vec![Recipient { vrchat_user_id: "usr_friend".into(), display_name: "Friend".into() }], ..Default::default() };
        // An empty item, then the item unchanged: not done as far as anyone can tell.
        let api = answering(vec![(200, r#"{"item":{}}"#.into()), (200, item(vec![]))]);
        assert!(api.share_capture("t", Kind::Capture, 3, &share).is_err());
        // Its answer does not read, but the item shows the grant: done.
        let api = answering(vec![(200, "{}".into()), (200, item(vec![grant("usr_friend")]))]);
        assert!(matches!(api.share_capture("t", Kind::Capture, 3, &share), Ok(Shared::Item { item }) if item.id == 3));
        // The answer itself shows it: no second request (the stand-in has none).
        let api = answering(vec![(200, item(vec![grant("usr_friend")]))]);
        assert!(api.share_capture("t", Kind::Capture, 3, &share).is_ok());
        // Withdrawn: the grant must be gone.
        let unshare = Unshare { recipients: vec!["usr_friend".into()], ..Default::default() };
        let api = answering(vec![(200, "{}".into()), (200, item(vec![grant("usr_friend")]))]);
        assert!(api.unshare_capture("t", Kind::Capture, 3, &unshare).is_err(), "still shared");
        let api = answering(vec![(200, "{}".into()), (200, item(vec![]))]);
        assert!(api.unshare_capture("t", Kind::Capture, 3, &unshare).is_ok());
        // A request answered: approved, not still pending.
        let request = |status: &str| serde_json::json!({"requests": [{"id": 5, "type": "capture", "itemId": 3, "status": status}]}).to_string();
        let api = answering(vec![(200, "{}".into()), (200, request("pending"))]);
        assert!(api.answer_capture_request("t", 5, true, false).is_err());
        let api = answering(vec![(200, "{}".into()), (200, request("approved"))]);
        assert!(api.answer_capture_request("t", 5, true, false).is_ok());
        // A refusal is a refusal, never read again.
        let api = answering(vec![(409, r#"{"code":"explicit-acknowledgement","message":"explicit"}"#.into())]);
        assert!(api.share_capture("t", Kind::Capture, 3, &share).unwrap_err().downcast_ref::<ExplicitAcknowledgement>().is_some());
    }
    /// The account's credential goes only with a capture picture's address on the server it was given for.
    #[test]
    fn only_a_capture_pictures_own_address_is_asked_with_the_account() {
        for ok in ["/captures/capture/61/file?part=picture", "/captures/capture/7/file?part=picture&v=2"] {
            assert!(capture_picture_address(ok), "{ok}");
        }
        for bad in [
            "https://elsewhere.example/captures/capture/61/file?part=picture",
            "//elsewhere.example/captures/capture/61/file?part=picture",
            "/captures/capture/61/file",
            "/captures/capture/61/file?part=sky",
            "/captures/capture/x/file?part=picture",
            "/captures/capture/61/../../users/me?part=picture",
            "/captures/picture/61/file?part=picture",
        ] {
            assert!(!capture_picture_address(bad), "{bad}");
        }
    }
    #[test]
    fn contract_json() {
        let feed: Feed = serde_json::from_str(include_str!("../tests/fixtures/captures.json")).unwrap();
        let item = &feed.items[0];
        assert_eq!(item.kind, Kind::Capture);
        assert_eq!(item.grants[0].parent_grant_id, None);
        assert_eq!(serde_json::from_value::<Item>(serde_json::to_value(item).unwrap()).unwrap(), *item);
        let share: Share = serde_json::from_str(
            r#"{"public":true,"recipients":[{"vrchatUserId":"usr_…","displayName":"Name"}],"acknowledgeExplicit":false,"message":"optional, 280 characters"}"#,
        )
        .unwrap();
        assert!(share.public);
        assert_eq!(serde_json::to_value(share).unwrap()["acknowledgeExplicit"], false);
    }
    /// CORE2-2: what a later server adds costs only itself: an item of a kind this build does not know, or a request of a
    /// status it does not know, is left out; an item without a field the contract made optional since still reads.
    #[test]
    fn a_page_reads_whatever_one_item_holds() {
        let mut page: serde_json::Value = serde_json::from_str(include_str!("../tests/fixtures/captures.json")).unwrap();
        let known = page["items"][0].clone();
        let mut newer = known.clone();
        newer["type"] = "recording".into();
        newer["id"] = 999.into();
        let mut sparse = known.clone();
        sparse["id"] = 1000.into();
        for field in ["canShare", "explicit", "mine", "public", "createdAt", "world", "owner"] {
            sparse.as_object_mut().unwrap().remove(field);
        }
        page["items"] = serde_json::json!([known, newer, sparse, {"type": "capture"}]);
        let feed: Feed = serde_json::from_value(page).unwrap();
        assert_eq!(feed.items.iter().map(|i| i.id).collect::<Vec<_>>(), [feed.items[0].id, 1000], "the unknown kind and the item without id left out");
        assert!(!feed.items[1].can_share);
        let requests: Requests = serde_json::from_value(serde_json::json!({"requests": [
            {"id": 1, "type": "capture", "itemId": 3, "status": "pending", "public": true, "recipients": [], "message": "", "requester": {"id": 1, "name": "A"}, "approver": {"id": 2, "name": "B"}, "createdAt": "2026-11-19T00:00:00Z"},
            {"id": 2, "type": "capture", "itemId": 3, "status": "expired"},
            {"id": 3, "type": "picture", "itemId": 4, "status": "denied"}
        ]}))
        .unwrap();
        assert_eq!(requests.requests.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 3]);
    }

    #[test]
    fn paging_is_opaque_deduplicated_and_stops_repeated_cursor() {
        let mut feed: Feed = serde_json::from_str(include_str!("../tests/fixtures/captures.json")).unwrap();
        feed.next = Some("a+/=?".into());
        let page = feed.clone();
        let n = feed.items.len();
        feed.append(page);
        assert_eq!(feed.items.len(), n);
        assert!(feed.next.is_none());
    }
    #[test]
    fn revoke_descendants_and_public_independently() {
        let mut i = Item { public: true, ..Default::default() };
        i.grants = vec![Grant { id: 1, vrchat_user_id: "one".into(), ..Default::default() }, Grant { id: 2, parent_grant_id: Some(1), ..Default::default() }];
        i.revoke(&Unshare { recipients: vec!["one".into()], ..Default::default() });
        assert!(i.grants.is_empty());
        assert!(i.public);
        i.revoke(&Unshare { all: true, ..Default::default() });
        assert!(!i.public);
    }
    fn server(status: u16, body: String) -> (Api, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&request[..end]);
                    let len =
                        head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").and_then(|s| s.trim().parse::<usize>().ok())).unwrap_or(0);
                    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
                        let tail = &request[end + 4..];
                        if tail == b"0\r\n\r\n" || tail.ends_with(b"\r\n0\r\n\r\n") {
                            break;
                        }
                    } else if request.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                .unwrap();
            let raw = String::from_utf8(request).unwrap();
            if let Some((head, body)) = raw.split_once("\r\n\r\n") {
                if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
                    let mut rest = body;
                    let mut decoded = String::new();
                    while let Some((size, chunk)) = rest.split_once("\r\n") {
                        let n = usize::from_str_radix(size.split(';').next().unwrap(), 16).unwrap();
                        if n == 0 {
                            break;
                        }
                        decoded.push_str(&chunk[..n]);
                        rest = &chunk[n + 2..];
                    }
                    return format!("{head}\r\n\r\n{decoded}");
                }
            }
            raw
        });
        (Api::at(&url), thread)
    }
    #[test]
    fn explicit_conflict_keeps_code_and_server_message() {
        let (api, request) = server(409, serde_json::json!({"code":"explicit-acknowledgement","message":EXPLICIT_WARNING}).to_string());
        let err = api.share_capture("fixture", Kind::Picture, 12, &Share { public: true, ..Default::default() }).unwrap_err();
        assert_eq!(err.downcast_ref::<ExplicitAcknowledgement>().unwrap().0, EXPLICIT_WARNING);
        let request = request.join().unwrap();
        assert!(request.starts_with("POST /captures/picture/12/share "));
        assert!(request.contains("acknowledgeExplicit"));
    }
    #[test]
    fn a_grantee_gets_a_request_and_never_an_owned_item() {
        let requests: Requests = serde_json::from_str(include_str!("../tests/fixtures/capture-requests.json")).unwrap();
        let (api, request) = server(202, serde_json::json!({"request":requests.requests[0]}).to_string());
        let result = api.share_capture("fixture", Kind::Capture, 123, &Share { public: true, ..Default::default() }).unwrap();
        assert!(matches!(result, Shared::Request { .. }));
        request.join().unwrap();
    }
    #[test]
    fn query_cursor_and_world_search_are_encoded_as_values() {
        let (api, request) = server(200, r#"{"items":[],"next":"opaque+/=?"}"#.into());
        let feed = api
            .captures("fixture", &Query { filter: "shared".into(), q: "a & b".into(), cursor: Some("opaque+/=?".into()), limit: 24, ..Default::default() })
            .unwrap();
        assert_eq!(feed.next.as_deref(), Some("opaque+/=?"));
        let request = request.join().unwrap();
        let line = request.lines().next().unwrap();
        assert!(line.contains("filter=shared"));
        assert!(!line.contains("a & b"));
        assert!(line.contains("cursor=opaque%2B%2F%3D%3F"));
    }

    #[test]
    fn private_upload_sends_contract_fields_and_returns_identity() {
        let (api, request) = server(200, r#"{"added":[{"type":"picture","id":42,"visibility":"private"}],"skipped":0}"#.into());
        let metadata = serde_json::json!({"visibility":"private","explicit":true,"explicitReasons":["osc-parameter:NSFW"],"takenAt":["2026-10-28T21:00:00Z"],"instanceId":"12345~friends(usr_fixture)","sourceKey":"0123456789abcdef"});
        let result = api
            .add_shared_pictures(
                "fixture",
                "wrld_11111111-1111-1111-1111-111111111111",
                vec![crate::maps::Part::bytes("pictures", "fixture.png", "image/png", vec![1, 2, 3])],
                &metadata,
            )
            .unwrap();
        assert_eq!(result.added[0].id, 42);
        let request = request.join().unwrap();
        for field in ["visibility", "explicit", "explicitReasons", "instanceId", "sourceKey"] {
            assert!(request.contains(&format!("name=\"{field}\"")), "{field}");
        }
        assert!(request.contains("osc-parameter:NSFW"));
    }
    #[test]
    fn single_item_is_wrapped_and_nullable_attribution_is_allowed() {
        let feed: Feed = serde_json::from_str(include_str!("../tests/fixtures/captures.json")).unwrap();
        let (api, request) = server(200, serde_json::json!({"item":feed.items[0]}).to_string());
        let item = api.capture_item("fixture", Kind::Capture, 123).unwrap();
        assert!(item.instance_id.is_some());
        assert_eq!(item.source_key.as_deref(), Some("2026-10-28_21-00-00"));
        request.join().unwrap();
        let mut value = serde_json::to_value(&item).unwrap();
        value.as_object_mut().unwrap().remove("instanceId");
        value["sourceKey"] = serde_json::Value::Null;
        let item: Item = serde_json::from_value(value).unwrap();
        assert!(item.instance_id.is_none() && item.source_key.is_none());
    }
    #[test]
    fn typed_incoming_requests_and_explicit_approval() {
        let fixture = include_str!("../tests/fixtures/capture-requests.json");
        let (api, request) = server(200, fixture.into());
        let mut requests = api.capture_requests("fixture", true).unwrap();
        assert_eq!(requests.requests[0].status, RequestStatus::Pending);
        assert_eq!(requests.requests[0].recipients[0].display_name, "Robin");
        assert!(request.join().unwrap().contains("box=incoming"));
        let mut r = requests.requests.remove(0);
        r.status = RequestStatus::Approved;
        r.answered_at = Some("2026-10-29T01:00:00Z".into());
        let (api, request) = server(200, serde_json::json!({"request":r}).to_string());
        let answer = api.answer_capture_request("fixture", 12, true, true).unwrap();
        assert_eq!(answer.request.status, RequestStatus::Approved);
        let body = request.join().unwrap();
        assert!(body.starts_with("POST /captures/requests/12/approve "));
        let json: serde_json::Value = serde_json::from_str(body.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(json["acknowledgeExplicit"], true);
        r.status = RequestStatus::Denied;
        let (api, request) = server(200, serde_json::json!({"request":r}).to_string());
        assert_eq!(api.answer_capture_request("fixture", 12, false, false).unwrap().request.status, RequestStatus::Denied);
        let body = request.join().unwrap();
        assert!(body.starts_with("POST /captures/requests/12/deny "));
        assert!(!body.contains("acknowledgeExplicit"));
        let (api, request) = server(409, serde_json::json!({"code":"explicit-acknowledgement","message":EXPLICIT_WARNING}).to_string());
        let error = api.answer_capture_request("fixture", 12, true, false).unwrap_err();
        assert!(error.downcast_ref::<ExplicitAcknowledgement>().is_some());
        request.join().unwrap();
        for status in [RequestStatus::Pending, RequestStatus::Approved, RequestStatus::Denied, RequestStatus::Cancelled] {
            r.status = status;
            let encoded = serde_json::to_value(&r).unwrap();
            assert_eq!(serde_json::from_value::<ShareRequest>(encoded).unwrap().status, status);
        }
    }
    #[test]
    fn skipped_upload_identity_is_resolved_from_owned_source_keys() {
        let fixture = include_str!("../tests/fixtures/captures.json");
        let (api, request) = server(200, fixture.into());
        let found = api.capture_by_source_key("fixture", "wrld_11111111-1111-1111-1111-111111111111", Kind::Capture, "2026-10-28_21-00-00").unwrap().unwrap();
        assert_eq!(found.id, 123);
        let request = request.join().unwrap();
        assert!(request.contains("filter=mine") && request.contains("worldId=") && request.contains("kind=capture"));
    }
}
