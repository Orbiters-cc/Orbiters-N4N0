//! `orbiters-desktop --repair-capture-pictures (--development | --production) [--ids 15,16,…] [--dry-run]`: gives the
//! signed-in account's own 3D captures on Orbiters that have no picture there (`capture.pictureUrl` none) the picture
//! of their 3D view, made from what this computer still has of them, as Photos & captures' share sends it
//! (`capture_picture::jpeg`) through the owner's route for a picture afterwards (`POST /captures/capture/:id/picture`).
//! Can be run again: a capture that has its picture is left alone, so a second run sends nothing.
//!
//! Which captures: `--ids` names them, else every capture of the account (`GET /captures?filter=mine&kind=capture`).
//! Where a capture's picture comes from: its folder in this computer's upload records for that account, else the
//! session folder named as its source key in the sessions folder. One without either is reported, not touched (the
//! server keeps no picture of a capture uploaded without one, and the 3D view alone is not drawn here).
//! `--dry-run` asks only (GET) and says what it would send. No environment by default: production must be named.
//! Each picture given is also written to shares.log. The account's credential is never printed.

use orbiters_core::account::{Api, Server};
use orbiters_core::shares::{self, Item, Kind};
use std::path::{Path, PathBuf};

/// What became of one capture.
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    /// Its picture was there already.
    Has,
    /// Given now (or, dry run, would be) from this folder, this many bytes of JPEG.
    Given { from: PathBuf, bytes: usize, sent: bool },
    /// Nothing here to make it from.
    NoFolder,
    /// A folder, but no picture can leave it (not reconstructed, or edited and not drawn again).
    NoPicture(PathBuf),
    /// Not this account's, not a 3D capture, or not on Orbiters.
    NotMine,
    Failed(String),
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let server = match (args.iter().any(|a| a == "--development"), args.iter().any(|a| a == "--production")) {
        (true, false) => Server::Development,
        (false, true) => Server::Production,
        _ => anyhow::bail!("name the environment: --development or --production"), // i18n: not shown (a developer's command)
    };
    let dry = args.iter().any(|a| a == "--dry-run");
    let ids: Option<Vec<u64>> = args
        .iter()
        .position(|a| a == "--ids")
        .and_then(|i| args.get(i + 1))
        .map(|list| list.split(',').filter(|s| !s.trim().is_empty()).map(|s| s.trim().parse::<u64>()).collect::<Result<_, _>>())
        .transpose()?;
    let (api, token) = super::shares::api(server)?;
    let records: Vec<(PathBuf, Item)> = super::shares::uploads(server);
    let sessions = orbiters_core::settings::Settings::load().sessions_dir;
    let items = match &ids {
        Some(ids) => ids.iter().map(|id| api.capture_item(&token, Kind::Capture, *id).map_err(|e| (*id, e))).collect::<Vec<_>>(),
        None => mine(&api, &token)?.into_iter().map(Ok).collect(),
    };
    let env = if server == Server::Development { "development" } else { "production" };
    println!("{env}: {} capture(s){}", items.len(), if dry { ", dry run (nothing sent)" } else { "" }); // i18n: not shown
    for item in items {
        let (id, outcome) = match item {
            Ok(item) => (item.id, repair(&api, &token, &item, |item| folder(item, &records, &sessions), dry)),
            Err((id, e)) => (id, Outcome::Failed(format!("{e:#}"))),
        };
        let line = match &outcome {
            Outcome::Has => "has its picture".to_string(),
            Outcome::Given { from, bytes, sent: true } => format!("given its picture ({} KB) from {}", bytes / 1024, from.display()),
            Outcome::Given { from, bytes, sent: false } => format!("would be given its picture ({} KB) from {}", bytes / 1024, from.display()),
            Outcome::NoFolder => "no picture, and nothing on this computer to make it from".into(),
            Outcome::NoPicture(dir) => format!("no picture, and none can leave {}", dir.display()),
            Outcome::NotMine => "not this account's 3D capture: left alone".into(),
            Outcome::Failed(e) => format!("failed: {e}"),
        }; // i18n: not shown (a developer's command)
        println!("capture {id}: {line}"); // i18n: not shown
        if matches!(outcome, Outcome::Given { sent: true, .. }) {
            super::shares::note(&format!("repair: {env} capture {id} {line}")); // i18n: not shown
        }
    }
    Ok(())
}

/// Every 3D capture of the account, page after page.
fn mine(api: &Api, token: &str) -> anyhow::Result<Vec<Item>> {
    let mut query = shares::Query { filter: "mine".into(), kind: Some(Kind::Capture), limit: 50, ..Default::default() };
    let mut feed = shares::Feed::default();
    loop {
        let page = api.captures(token, &query)?;
        feed.append(page);
        match feed.next.clone() {
            Some(next) => query.cursor = Some(next),
            None => break,
        }
    }
    Ok(feed.items.into_iter().filter(|i| i.mine && i.kind == Kind::Capture).collect())
}

/// Its folder on this computer: by the upload records, else the sessions folder's folder named as its source key.
fn folder(item: &Item, records: &[(PathBuf, Item)], sessions: &Path) -> Option<PathBuf> {
    let recorded = records.iter().find(|(_, r)| r.kind == Kind::Capture && r.id == item.id).map(|(p, _)| p.clone()).filter(|p| p.is_dir());
    recorded.or_else(|| {
        let key = item.source_key.as_deref().filter(|k| !k.is_empty() && !k.contains(['/', '\\']) && *k != "." && *k != "..")?;
        Some(sessions.join(key)).filter(|p| p.join("manifest.json").is_file())
    })
}

/// One capture: its picture given from `folder_of` when it has none.
pub(crate) fn repair(api: &Api, token: &str, item: &Item, folder_of: impl Fn(&Item) -> Option<PathBuf>, dry: bool) -> Outcome {
    if !item.mine || item.kind != Kind::Capture || item.capture.is_none() {
        return Outcome::NotMine;
    }
    if item.capture.as_ref().is_some_and(|c| c.picture_url.is_some()) {
        return Outcome::Has;
    }
    let Some(dir) = folder_of(item) else { return Outcome::NoFolder };
    let Some(jpeg) = crate::capture_picture::jpeg(&dir) else { return Outcome::NoPicture(dir) };
    let bytes = jpeg.len();
    if dry {
        return Outcome::Given { from: dir, bytes, sent: false };
    }
    match api.set_capture_picture(token, item.id, jpeg) {
        Ok(after) if after.capture.as_ref().is_some_and(|c| c.picture_url.is_some()) => Outcome::Given { from: dir, bytes, sent: true },
        Ok(_) => Outcome::Failed("Orbiters answered without a picture".into()), // i18n: not shown
        Err(e) => Outcome::Failed(format!("{e:#}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::shares::tests::{capture_item, requests, stand_in};

    /// The repair gives a capture of the account without its picture the one its folder here makes, says what it would
    /// do on a dry run without sending, and leaves alone what has its picture, what is not the account's, and what
    /// nothing here can make a picture of: run again, it sends nothing.
    #[test]
    fn the_repair_gives_only_missing_pictures_and_can_run_again() {
        let dir = std::env::temp_dir().join(format!("orbiters-repair-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pixels: Vec<u8> = (0..32 * 18).flat_map(|i| [(i % 255) as u8, 80, 160, 255]).collect();
        std::fs::write(dir.join(orbiters_core::bundle::PICTURE_THUMBNAIL_FILE), orbiters_contracts::qoi::encode(&pixels, 32, 18, 4)).unwrap();
        let here = |_: &Item| Some(dir.clone());
        let with = serde_json::json!({ "item": capture_item(3, false, true) }).to_string();
        let (api, seen) = stand_in(vec![("POST /captures/capture/3/picture", 200, with)]);
        let missing = capture_item(3, false, false);
        assert!(matches!(repair(&api, "token", &missing, here, true), Outcome::Given { sent: false, .. }));
        assert!(requests(&seen).is_empty(), "a dry run sends nothing");
        assert!(matches!(repair(&api, "token", &missing, here, false), Outcome::Given { sent: true, bytes, .. } if bytes > 0));
        assert_eq!(requests(&seen), ["POST /captures/capture/3/picture"]);
        // (A copy: a reference would hold the stand-in's lock to the end of the test.)
        let sent = seen.lock().unwrap()[0].1.clone();
        assert!(sent.windows(4).any(|w| w == [0xFF, 0xD8, 0xFF, 0xE0]) || sent.windows(3).any(|w| w == [0xFF, 0xD8, 0xFF]), "a JPEG");
        // Run again (Orbiters now lists its picture): nothing sent.
        assert_eq!(repair(&api, "token", &capture_item(3, false, true), here, false), Outcome::Has);
        let mut other = capture_item(4, true, false);
        other.mine = false;
        assert_eq!(repair(&api, "token", &other, here, false), Outcome::NotMine);
        assert_eq!(repair(&api, "token", &capture_item(5, false, false), |_| None, false), Outcome::NoFolder);
        std::fs::remove_file(dir.join(orbiters_core::bundle::PICTURE_THUMBNAIL_FILE)).unwrap();
        assert_eq!(repair(&api, "token", &capture_item(6, false, false), here, false), Outcome::NoPicture(dir.clone()));
        assert_eq!(requests(&seen).len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }
}
