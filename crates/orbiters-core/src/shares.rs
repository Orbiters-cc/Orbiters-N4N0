//! Capture sharing contract (`contracts/Docs/capture-sharing.md`). No VRChat credentials or messages.
use crate::account::{Api, ApiError};
use serde::{Deserialize, Serialize};

pub const EXPLICIT_WARNING: &str = "Any explicit content shared publicly will be reported to the VRChat staff.";

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
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: u64,
    pub name: String,
    pub avatar_url: Option<String>,
    pub vrchat_user_id: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct World {
    pub id: String,
    pub name: String,
    pub thumbnail_url: Option<String>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub id: u64,
    pub vrchat_user_id: String,
    pub display_name: Option<String>,
    pub granted_by: u64,
    pub parent_grant_id: Option<u64>,
    pub created_at: String,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capture {
    pub splat_count: u64,
    pub preview_url: String,
    pub sky_url: Option<String>,
    pub view: serde_json::Value,
    pub vrchat_slot: Option<u32>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Picture {
    pub url: String,
    pub thumbnail_url: String,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    #[serde(rename = "type")]
    pub kind: Kind,
    pub id: u64,
    pub world: World,
    pub owner: Person,
    pub from: Option<Person>,
    pub taken_at: Option<String>,
    pub created_at: String,
    pub public: bool,
    #[serde(default)]
    pub grants: Vec<Grant>,
    pub mine: bool,
    pub can_share: bool,
    pub explicit: bool,
    #[serde(default)]
    pub explicit_reasons: Vec<String>,
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
    pub items: Vec<Item>,
    pub next: Option<String>,
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
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
/// The contract does not define ShareRequest's fields. Preserve its JSON without inventing a schema.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Shared {
    Item { item: Item },
    Request { request: serde_json::Value },
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

/// Shared by uploads and sharing so a 409 keeps its machine-readable meaning.
pub(crate) fn json<T: serde::de::DeserializeOwned>(response: ureq::http::Response<ureq::Body>) -> anyhow::Result<T> {
    let status = response.status().as_u16();
    let text = response.into_body().with_config().limit(32 * 1024 * 1024).read_to_string()?;
    if !(200..300).contains(&status) {
        let value: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let message = value["message"].as_str().or_else(|| value["error"].as_str()).unwrap_or("Orbiters could not complete the request").to_string();
        if status == 409 && value["code"] == "explicit-acknowledgement" {
            return Err(ExplicitAcknowledgement(message).into());
        }
        return Err(ApiError { status, message }.into());
    }
    Ok(serde_json::from_str(&text)?)
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
    pub fn capture_item(&self, token: &str, kind: Kind, id: u64) -> anyhow::Result<Item> {
        json(self.agent().get(&self.url(&path(kind, id))).header("Authorization", &format!("Bearer {token}")).call()?)
    }
    pub fn share_capture(&self, token: &str, kind: Kind, id: u64, share: &Share) -> anyhow::Result<Shared> {
        anyhow::ensure!(share.public || !share.recipients.is_empty(), "Choose someone to share with.");
        anyhow::ensure!(
            share.recipients.len() <= 50 && share.message.chars().count() <= 280,
            "Choose at most 50 people and a message of at most 280 characters."
        );
        json(self.agent().post(&self.url(&format!("{}/share", path(kind, id)))).header("Authorization", &format!("Bearer {token}")).send_json(share)?)
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
        Ok(json::<Answer>(r)?.item)
    }
    pub fn capture_file(&self, token: &str, kind: Kind, id: u64, out: &std::path::Path) -> anyhow::Result<()> {
        let r = self.agent().get(&self.url(&format!("{}/file", path(kind, id)))).header("Authorization", &format!("Bearer {token}")).call()?;
        if !r.status().is_success() {
            json::<serde_json::Value>(r)?;
            unreachable!();
        }
        let mut file = std::fs::File::create(out)?;
        std::io::copy(&mut r.into_body().into_reader(), &mut file)?;
        Ok(())
    }
    pub fn capture_requests(&self, token: &str, incoming: bool) -> anyhow::Result<serde_json::Value> {
        json(
            self.agent()
                .get(&self.url("captures/requests"))
                .query("box", if incoming { "incoming" } else { "outgoing" })
                .header("Authorization", &format!("Bearer {token}"))
                .call()?,
        )
    }
    pub fn answer_capture_request(&self, token: &str, id: u64, approve: bool) -> anyhow::Result<serde_json::Value> {
        json(
            self.agent()
                .post(&self.url(&format!("captures/requests/{id}/{}", if approve { "approve" } else { "deny" })))
                .header("Authorization", &format!("Bearer {token}"))
                .send_empty()?,
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
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                .unwrap();
            String::from_utf8(request).unwrap()
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
        let (api, request) = server(202, r#"{"request":{"id":5,"status":"pending"}}"#.into());
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
        let metadata = serde_json::json!({"visibility":"private","explicit":true,"explicitReasons":["osc-parameter:NSFW"],"takenAt":["2026-10-28T21:00:00Z"]});
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
        for field in ["visibility", "explicit", "explicitReasons"] {
            assert!(request.contains(&format!("name=\"{field}\"")), "{field}");
        }
        assert!(request.contains("osc-parameter:NSFW"));
    }
}
