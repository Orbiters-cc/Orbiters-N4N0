//! Background jobs for the gallery and its sharing dialogs. No disk or HTTP work in the painting code.
use orbiters_core::{
    account::{self, Api, Server},
    explicit::Content,
    maps::{Part, Uploaded},
    shares::{self, Item, Kind},
    visited,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The sharing contract keeps the member's world captures wanted while they are in VRChat. Screenshots do not
/// start the controller, so they never send presence. Failed calls back off; a world change is sent on the next tick.
pub(super) fn spawn_presence(ctl: super::Shared) {
    std::thread::spawn(move || {
        let mut last = None;
        let mut sent = std::time::Instant::now() - std::time::Duration::from_secs(300);
        let mut attempted = sent;
        let mut failed=false;
        while !ctl.quitting.load(std::sync::atomic::Ordering::Relaxed) {
            let snapshot = ctl.view();
            let world = &snapshot.vrchat.world;
            let location = world.world_id.as_ref().zip(world.instance.as_ref()).zip(world.local_player.as_ref())
                .filter(|_| snapshot.vrchat.running && world.in_world)
                .map(|((w,i),n)| (snapshot.settings.development,w.clone(),i.clone(),n.clone(),format!("{:?}",snapshot.account.state)));
            if let Some(ref location) = location {
                if (last.as_ref() != Some(location) || sent.elapsed().as_secs() >= 300) && (!failed || attempted.elapsed().as_secs() >= 60) {
                    attempted=std::time::Instant::now();
                    failed=true;
                    if let Some((api,token))=ctl.account_api() {
                        if api.capture_presence(&token,&location.1,&location.2,&location.3).is_ok() {failed=false;last=Some(location.clone());sent=std::time::Instant::now();}
                    }
                }
            } else {last=None;}
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}

#[derive(Clone, Debug)]
pub enum Local {
    Session(PathBuf),
    Picture(PathBuf),
}
impl Local {
    pub fn path(&self) -> &Path {
        match self {
            Self::Session(p) | Self::Picture(p) => p,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub world_id: String,
    pub world_name: String,
    pub taken: u64,
    pub instance: Option<String>,
    pub people: Vec<String>,
    pub local: Option<Local>,
    pub remote: Option<Item>,
    pub content: Content,
    pub ready: bool,
    pub picture: Option<crate::worlds::Picture>,
}
impl Entry {
    pub fn mine(&self) -> bool {
        self.remote.as_ref().is_none_or(|i| i.mine)
    }
    pub fn is_picture(&self) -> bool {
        matches!(self.local, Some(Local::Picture(_))) || self.remote.as_ref().is_some_and(|i| i.kind == Kind::Picture)
    }
}
pub fn unix(date: &str) -> u64 {
    time::OffsetDateTime::parse(date, &time::format_description::well_known::Rfc3339).ok().and_then(|t| u64::try_from(t.unix_timestamp()).ok()).unwrap_or(0)
}
pub fn iso(unix: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix as i64).ok().and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok()).unwrap_or_default()
}
/// Only the last globally joined instance can own a timestamp. A later join of another world prevents misattribution.
pub fn instance_at(history: &visited::History, world: &str, taken: u64) -> Option<String> {
    history.instances.iter().filter(|i| i.at <= taken).max_by_key(|i| i.at).filter(|i| i.world_id == world).map(|i| i.instance.clone())
}
pub fn local_entries(root: &Path, server: Server) -> Vec<Entry> {
    let worlds = crate::worlds::load(root);
    let history = visited::History::load(&visited::history_file());
    let records = records_path(server).ok().map(|p| read_records(&p)).unwrap_or_default();
    let mut entries = Vec::new();
    for w in worlds {
        for s in &w.sessions {
            let taken = unix(&s.manifest.created_utc);
            let local = Local::Session(s.dir.clone());
            let remote = records.iter().find(|r| r.path == s.dir).and_then(|r| r.item.clone());
            entries.push(Entry {
                key: s.dir.to_string_lossy().into(),
                world_id: w.id.clone(),
                world_name: w.name.clone(),
                taken,
                instance: s.manifest.world.as_ref().and_then(|w| w.instance.clone()).or_else(|| instance_at(&history, &w.id, taken)),
                people: Vec::new(),
                local: Some(local),
                remote,
                content: Content::read(&s.dir),
                ready: s.reconstruction.is_some(),
                picture: Some(crate::worlds::Picture::Session(s.dir.clone())),
            });
        }
        for shot in &w.shots {
            let mut content = Content::default();
            for s in &w.sessions {
                let start = unix(&s.manifest.created_utc);
                if shot.info.taken >= start && shot.info.taken <= start + s.manifest.stats.duration_s.ceil() as u64 {
                    content.merge(&Content::read(&s.dir));
                }
            }
            let remote = records.iter().find(|r| r.path == shot.path).and_then(|r| r.item.clone());
            entries.push(Entry {
                key: shot.path.to_string_lossy().into(),
                world_id: w.id.clone(),
                world_name: w.name.clone(),
                taken: shot.info.taken,
                instance: instance_at(&history, &w.id, shot.info.taken),
                people: Vec::new(),
                local: Some(Local::Picture(shot.path.clone())),
                remote,
                content,
                ready: true,
                picture: Some(crate::worlds::Picture::Shot { path: shot.path.clone(), size: shot.size, width: 560 }),
            });
        }
    }

    entries.sort_by_key(|e| std::cmp::Reverse(e.taken));
    entries
}
pub fn remote_entry(item: Item) -> Entry {
    let picture = item.picture.as_ref().and_then(|p| {
        p.thumbnail_url.starts_with("https://").then(|| crate::worlds::Picture::Remote {
            world_id: format!("capture-{}-{}", item.kind.path(), item.id),
            url: p.thumbnail_url.clone(),
            large: false,
        })
    });
    Entry {
        key: format!("{}:{}", item.kind.path(), item.id),
        world_id: item.world.id.clone(),
        world_name: item.world.name.clone(),
        taken: unix(item.date()),
        instance: None,
        people: Vec::new(),
        local: None,
        content: Content { explicit: item.explicit, explicit_reasons: item.explicit_reasons.clone() },
        ready: true,
        picture,
        remote: Some(item),
    }
}
pub fn api(server: Server) -> anyhow::Result<(Api, String)> {
    let account = account::read(server).ok_or_else(|| anyhow::anyhow!("Sign in to Orbiters in Settings first."))?;
    Ok((Api::new(server), account.token))
}
pub fn feed(server: Server, query: shares::Query) -> anyhow::Result<shares::Feed> {
    let (api, token) = api(server)?;
    api.captures(&token, &query)
}
#[derive(Serialize, Deserialize)]
struct Record {
    path: PathBuf,
    item: Option<Item>,
    uploaded: Uploaded,
}
fn records_path(server: Server) -> anyhow::Result<PathBuf> {
    let account = account::read(server).ok_or_else(|| anyhow::anyhow!("Sign in first"))?;
    let user = account.user.ok_or_else(|| anyhow::anyhow!("Orbiters did not identify this account"))?;
    anyhow::ensure!(user.chars().all(|c| c.is_ascii_digit()), "Invalid account id");
    Ok(orbiters_core::paths::config_dir().join(format!("capture-uploads-{}-{user}.json", if server == Server::Development { "dev" } else { "prod" })))
}
fn read_records(path: &Path) -> Vec<Record> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}
fn save_record(server: Server, entry: &Entry, item: &Item) -> anyhow::Result<()> {
    if let Some(local) = &entry.local {
        let path = records_path(server)?;
        let mut records = read_records(&path);
        records.retain(|r| r.path != local.path());
        records.push(Record {
            path: local.path().into(),
            item: Some(item.clone()),
            uploaded: Uploaded { kind: item.kind, id: item.id, visibility: if item.public { "public" } else { "private" }.into() },
        });
        orbiters_core::write_atomic(&path, &serde_json::to_vec(&records)?)?;
    }
    Ok(())
}
static MUTATION: Mutex<()> = Mutex::new(());
/// Upload identity is saved before granting: retry after a failed share never uploads a duplicate.
pub fn share(server: Server, entry: Entry, request: shares::Share) -> anyhow::Result<shares::Shared> {
    let _one = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
    let (api, token) = api(server)?;
    let remembered = entry
        .local
        .as_ref()
        .and_then(|l| records_path(server).ok().and_then(|p| read_records(&p).into_iter().find(|r| r.path == l.path())))
        .map(|r| api.capture_item(&token, r.uploaded.kind, r.uploaded.id))
        .transpose()?;
    let existing = entry.remote.clone().or(remembered);
    let item = if let Some(item) = existing {
        item
    } else {
        anyhow::ensure!(!request.public || !entry.content.explicit || request.acknowledge_explicit, "{}", shares::EXPLICIT_WARNING);
        let mut metadata = serde_json::json!({"visibility":if request.public{"public"}else{"private"},"explicit":entry.content.explicit,"explicitReasons":entry.content.explicit_reasons,"acknowledgeExplicit":request.acknowledge_explicit,"takenAt":iso(entry.taken)});
        let uploaded: Uploaded = match entry.local.as_ref().ok_or_else(|| anyhow::anyhow!("No local file to upload"))? {
            Local::Session(dir) => {
                let (files, sky, view) = super::maps::web_preview(dir)?;
                metadata.as_object_mut().unwrap().extend(view.as_object().unwrap().clone());
                api.add_preview(&token, &entry.world_id, files, sky.as_deref(), &metadata, |_, _| {})?
            }
            Local::Picture(path) => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let part = Part::file("pictures", name.as_ref(), "image/png", path)?;
                metadata["takenAt"] = serde_json::json!([iso(entry.taken)]);
                let result = api.add_shared_pictures(&token, &entry.world_id, vec![part], &metadata)?;
                result
                    .added
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("The server skipped this picture without returning its capture id. Refresh the gallery to find it."))?
            }
        };
        let file = records_path(server)?;
        let mut records = read_records(&file);
        let local = entry.local.as_ref().unwrap();
        records.retain(|r| r.path != local.path());
        records.push(Record { path: local.path().into(), item: None, uploaded: uploaded.clone() });
        orbiters_core::write_atomic(&file, &serde_json::to_vec(&records)?)?;
        let item = api.capture_item(&token, uploaded.kind, uploaded.id)?;
        save_record(server, &entry, &item)?;
        item
    };
    let result = api.share_capture(&token, item.kind, item.id, &request)?;
    if let shares::Shared::Item { item } = &result {
        save_record(server, &entry, item)?;
    }
    Ok(result)
}
pub fn unshare(server: Server, entry: Entry, request: shares::Unshare) -> anyhow::Result<Item> {
    let _one = MUTATION.lock().unwrap_or_else(|e| e.into_inner());
    let (api, token) = api(server)?;
    let item = entry.remote.as_ref().ok_or_else(|| anyhow::anyhow!("This item has not been uploaded"))?;
    let item = api.unshare_capture(&token, item.kind, item.id, &request)?;
    save_record(server, &entry, &item)?;
    Ok(item)
}
pub fn download(server: Server, item: Item) -> anyhow::Result<PathBuf> {
    let (api, token) = api(server)?;
    let dir = orbiters_core::paths::documents_root().join("Shared captures");
    std::fs::create_dir_all(&dir)?;
    let out =
        dir.join(format!("{}-{}-{}.{}", item.kind.path(), item.id, uuid::Uuid::new_v4().simple(), if item.kind == Kind::Picture { "webp" } else { "splat" }));
    let temp = out.with_extension("download");
    let result = api.capture_file(&token, item.kind, item.id, &temp);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    std::fs::rename(temp, &out)?;
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grouping_never_assigns_another_world_or_a_future_join() {
        let mut h = visited::History::default();
        h.instances = vec![
            visited::InstanceVisit { world_id: "a".into(), instance: "one".into(), at: 10 },
            visited::InstanceVisit { world_id: "b".into(), instance: "two".into(), at: 20 },
        ];
        assert_eq!(instance_at(&h, "a", 15), Some("one".into()));
        assert_eq!(instance_at(&h, "a", 21), None);
        assert_eq!(instance_at(&h, "a", 9), None);
    }
}
