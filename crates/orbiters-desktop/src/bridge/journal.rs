//! What the bridge's sessions did that outlives the service: the worlds they arrived in and the 3D pictures they took, kept in
//! a file (`bridge-journal.json` in the app's folder). A world tour labels worlds and publishes pictures later than its visit
//! (after an app update, or a long reconstruction queue): the rule stays "only worlds a bridge session really was in, and only
//! pictures it took", and this is how the bridge still knows them after a restart.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Kept at most: a tour of a day is a few hundred worlds and pictures.
const KEPT: usize = 5000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Visit {
    /// Lowercase `wrld_…`.
    pub world: String,
    #[serde(default)]
    pub name: Option<String>,
    /// VRChat's profile of the session that was there.
    #[serde(default)]
    pub profile: u32,
    /// Unix seconds of its last arrival.
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Picture {
    /// The session's folder name in the sessions folder.
    pub session: String,
    #[serde(default)]
    pub world_id: Option<String>,
    #[serde(default)]
    pub world_name: Option<String>,
    pub length_s: u32,
    #[serde(default)]
    pub profile: u32,
    pub at: u64,
    /// Its files were deleted through the bridge (`capture_files` or `everything`).
    #[serde(default)]
    pub freed: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub visits: Vec<Visit>,
    #[serde(default)]
    pub pictures: Vec<Picture>,
    /// The VRChat accounts side sessions were signed in with (`usr_…`): a session folder in a strict private instance of one
    /// of them was recorded by a bridge session ([`Journal::adopt`]).
    #[serde(default)]
    pub accounts: Vec<String>,
    /// Where it is kept; `None`: in memory only (tests that do not ask for a file).
    #[serde(skip)]
    path: Option<PathBuf>,
}

fn now() -> u64 {
    orbiters_core::visited::now()
}

impl Journal {
    /// The journal kept in `path` (empty when there is none or it cannot be read).
    pub fn at(path: &Path) -> Self {
        let mut journal: Journal = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        journal.path = Some(path.to_path_buf());
        journal
    }

    /// The service's own file.
    pub fn file() -> PathBuf {
        orbiters_core::paths::config_dir().join("bridge-journal.json")
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Ok(bytes) = serde_json::to_vec(self) {
            if let Err(e) = orbiters_core::write_atomic(path, &bytes) {
                eprintln!("bridge journal: could not be kept: {e}");
            }
        }
    }

    /// A session arrived in `world`.
    pub fn arrived(&mut self, world: &str, name: Option<&str>, profile: u32) {
        let world = world.to_ascii_lowercase();
        self.visits.retain(|v| v.world != world);
        self.visits.push(Visit { world, name: name.map(str::to_string), profile, at: now() });
        let over = self.visits.len().saturating_sub(KEPT);
        self.visits.drain(..over);
        self.save();
    }

    pub fn was_in(&self, world: &str) -> bool {
        self.visits.iter().any(|v| v.world.eq_ignore_ascii_case(world)) || self.pictures.iter().any(|p| p.world_id.as_deref().is_some_and(|w| w.eq_ignore_ascii_case(world)))
    }

    /// A session took a picture.
    pub fn pictured(&mut self, picture: Picture) {
        self.pictures.retain(|p| p.session != picture.session);
        self.pictures.push(picture);
        let over = self.pictures.len().saturating_sub(KEPT);
        self.pictures.drain(..over);
        self.save();
    }

    pub fn picture(&self, session: &str) -> Option<&Picture> {
        self.pictures.iter().find(|p| p.session == session)
    }

    /// A side session is signed in with `account`.
    pub fn account(&mut self, account: &str) {
        if account.starts_with("usr_") && !self.accounts.iter().any(|a| a == account) {
            self.accounts.push(account.to_string());
            self.save();
        }
    }

    /// Takes in the 3D pictures under `sessions_dir` that a bridge session took before this journal knew them (before the
    /// build that keeps it): a folder with a picture's settings (`picture.json`) whose manifest puts it in a strict private
    /// instance of an account a side session used, never of `persons` (the VRChat account of Orbiters - N4N0's Settings, the
    /// person's own). Nothing else is ever taken: the person's own captures stay theirs. How many were taken in.
    pub fn adopt(&mut self, sessions_dir: &Path, persons: Option<&str>) -> usize {
        let accounts: Vec<&String> = self.accounts.iter().filter(|a| Some(a.as_str()) != persons).collect();
        if accounts.is_empty() {
            return 0;
        }
        let mut found = Vec::new();
        for entry in std::fs::read_dir(sessions_dir).into_iter().flatten().flatten() {
            let dir = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if self.picture(&name).is_some() {
                continue;
            }
            let Some(picture) = orbiters_core::picture::Picture::read(&dir) else { continue };
            let Ok(manifest) = orbiters_core::bundle::read_manifest(&dir) else { continue };
            let Some(world) = manifest.world else { continue };
            let Some(instance) = world.instance.as_deref() else { continue };
            if !accounts.iter().any(|a| orbiters_core::vrchat_api::private_instance_of(instance, a)) {
                continue;
            }
            let length_s = match picture.length {
                orbiters_core::settings::Photo3dLength::Quick => 1,
                orbiters_core::settings::Photo3dLength::Standard => 4,
                orbiters_core::settings::Photo3dLength::Full => 8,
            };
            let at = entry.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            found.push((Picture { session: name, world_id: world.id.clone(), world_name: world.name.clone(), length_s, profile: 0, at, freed: None }, world.id, world.name));
        }
        found.sort_by_key(|(p, ..)| p.at);
        let taken = found.len();
        for (picture, world, name) in found {
            if let Some(world) = world.filter(|w| orbiters_core::vrchat_log::is_world_id(w)) {
                let world = world.to_ascii_lowercase();
                if !self.visits.iter().any(|v| v.world == world) {
                    self.visits.push(Visit { world, name, profile: 0, at: picture.at });
                }
            }
            self.pictures.push(picture);
        }
        if taken > 0 {
            self.pictures.sort_by_key(|p| p.at);
            self.save();
        }
        taken
    }

    pub fn freed(&mut self, session: &str, what: &str) {
        if let Some(p) = self.pictures.iter_mut().find(|p| p.session == session) {
            p.freed = Some(what.to_string());
            self.save();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visits_and_pictures_outlive_the_service() {
        let path = std::env::temp_dir().join(format!("orbiters-bridge-journal-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let mut j = Journal::at(&path);
            j.arrived("WRLD_A", Some("A"), 1);
            j.arrived("wrld_a", Some("A"), 1);
            j.pictured(Picture { session: "A 2026-11-15 10-00-00".into(), world_id: Some("wrld_b".into()), length_s: 8, profile: 1, at: 1, ..Default::default() });
        }
        let j = Journal::at(&path);
        assert_eq!(j.visits.len(), 1, "one entry a world");
        assert!(j.was_in("wrld_a") && j.was_in("WRLD_B"), "a world arrived in, and one pictured");
        assert!(!j.was_in("wrld_c"));
        assert_eq!(j.picture("A 2026-11-15 10-00-00").map(|p| p.length_s), Some(8));
        let _ = std::fs::remove_file(&path);
    }

    /// Pictures a bridge session took before the journal existed are taken in from the sessions folder by their instance's
    /// owner, a side session's account; the person's own captures (their own account's private instances, recordings without
    /// a picture's settings, other people's instances) never are.
    #[test]
    fn only_pictures_from_a_bridge_sessions_own_instances_are_taken_in() {
        let root = std::env::temp_dir().join(format!("orbiters-journal-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let side = "usr_dfc96b2c-1aec-4775-9557-eef94a7f02c3";
        let person = "usr_1339e525-0000-4000-8000-000000000000";
        let session = |name: &str, owner: &str, picture: bool| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            let mut m = orbiters_core::contracts::manifest::SessionManifest::new("id".into(), "2026-11-15T05:00:00Z".into(), "0.3.5");
            m.world = Some(orbiters_core::contracts::manifest::WorldInfo {
                id: Some("wrld_b2d24c29-1ded-4990-a90d-dd6dcc440300".into()),
                name: Some("Japan Shrine".into()),
                author_name: None,
                instance: Some(format!("54758~private({owner})~region(eu)~nonce(46fc04e3-a8f0-4b31-99f6-732418f4632c)")),
            });
            std::fs::write(dir.join(orbiters_core::contracts::manifest::MANIFEST_FILE), serde_json::to_vec(&m).unwrap()).unwrap();
            if picture {
                let p = orbiters_core::picture::Picture { length: orbiters_core::settings::Photo3dLength::Full, ..Default::default() };
                p.save(&dir).unwrap();
            }
        };
        session("Japan Shrine 2026-11-15 05-53-21", side, true);
        session("Mine 2026-11-01 10-00-00", person, true);
        session("A recording 2026-11-02 10-00-00", side, false);
        session("Someone's 2026-11-03 10-00-00", "usr_99999999-0000-4000-8000-000000000000", true);
        let mut j = Journal::default();
        assert_eq!(j.adopt(&root, Some(person)), 0, "no side session's account known yet: nothing");
        j.account(side);
        j.account(person);
        assert_eq!(j.adopt(&root, Some(person)), 1, "the person's account is never a bridge session's, even if one used it");
        assert_eq!(j.picture("Japan Shrine 2026-11-15 05-53-21").map(|p| p.length_s), Some(8));
        assert!(j.was_in("wrld_b2d24c29-1ded-4990-a90d-dd6dcc440300"));
        assert_eq!(j.adopt(&root, Some(person)), 0, "once");
        let _ = std::fs::remove_dir_all(&root);
    }
}
