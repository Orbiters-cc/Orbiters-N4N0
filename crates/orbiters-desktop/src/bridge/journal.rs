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
}
