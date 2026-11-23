use crate::avatars::Avatar;
use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use orbiters_i18n::tr;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub kind: String,
    pub asset_id: u64,
    pub version: Option<String>,
    pub name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub schema: u32,
    pub id: String,
    pub created_utc: String,
    pub from: String,
    /// The Orbiters API the items' asset ids belong to (`Server::api`); Unity refuses a request for another server.
    pub server: String,
    pub avatar: Avatar,
    pub items: Vec<Item>,
    pub then: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Done,
    Declined,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultFile {
    pub schema: u32,
    pub id: String,
    pub status: Status,
    pub message: String,
    pub finished_utc: String,
    #[serde(default)]
    pub installed: Vec<Installed>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub asset_id: u64,
    pub version: String,
}

pub fn request(avatar: Avatar, asset_id: u64, server: &str, app_version: &str) -> anyhow::Result<Request> {
    uuid::Uuid::parse_str(avatar.id.strip_prefix("avtr_").with_context(|| tr("Only uploaded avatars can receive N4N0"))?)?;
    if asset_id == 0 {
        bail!("{}", tr("N4N0 is not published in this environment yet."));
    }
    Ok(Request {
        schema: 1,
        id: uuid::Uuid::new_v4().to_string(),
        created_utc: time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?,
        from: format!("Orbiters - N4N0 {app_version}"),
        server: server.to_string(),
        avatar,
        items: vec![Item { kind: "gallery".into(), asset_id, version: None, name: "N4N0".into() }],
        then: vec!["save-scene".into(), "open-build-panel".into()],
    })
}
pub fn write(project: &Path, request: &Request, handoff: &crate::unity6::authority::Handoff) -> anyhow::Result<()> {
    // It ends in the SDK's build panel, ready to upload: leave for that is asked of the one place that knows.
    // Asked again now: package work, another program or a swapped folder may have changed what it was given for.
    handoff.still(project).map_err(|why| anyhow::anyhow!("{why}"))?;
    if request.schema != 1 || uuid::Uuid::parse_str(&request.id)?.to_string() != request.id {
        bail!("{}", tr("Invalid setup request id or schema"));
    }
    crate::write_json(&project.join("UserSettings/Orbiters/SetupRequests").join(format!("{}.json", request.id)), request)
}
/// Consume only a valid result for this request. A partially published or wrong result stays untouched.
pub fn read_result(project: &Path, id: &str) -> anyhow::Result<Option<ResultFile>> {
    if uuid::Uuid::parse_str(id)?.to_string() != id {
        bail!("{}", tr("Invalid setup request id"));
    }
    let p = project.join("UserSettings/Orbiters/SetupRequests").join(format!("{id}.result.json"));
    let bytes = match std::fs::read(&p) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let result: ResultFile = serde_json::from_slice(&bytes)?;
    if result.schema != 1 || result.id != id {
        bail!("{}", tr("Toolkit returned a result for a different request or schema"));
    }
    Ok(Some(result))
}

pub fn take_result(project: &Path, id: &str) -> anyhow::Result<Option<ResultFile>> {
    let result = read_result(project, id)?;
    if result.is_some() {
        std::fs::remove_file(project.join("UserSettings/Orbiters/SetupRequests").join(format!("{id}.result.json")))?;
    }
    Ok(result)
}
