//! Evidence saved when a capture starts. Parameter names are signals, never inferred from avatar names.
use serde::{Deserialize, Serialize};
use std::path::Path;
pub const FILE: &str = "capture-content.json";
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    pub explicit: bool,
    pub explicit_reasons: Vec<String>,
}
pub fn matches_parameter(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["nsfw", "nude", "naked", "sps", "dps", "tps", "penetrat", "orifice", "genital", "lewd"].iter().any(|s| lower.contains(s))
}

/// Toolkit's final built parameter names and explicit menu tags for the avatar currently worn.
pub fn manifest_content(avatar_id: Option<&str>) -> Content {
    let Some(id) = avatar_id else { return Content::default() };
    let file = crate::paths::avatars_dir().join(crate::contracts::avatar::file_name(id));
    let Some(manifest) =
        std::fs::read_to_string(file).ok().and_then(|s| crate::contracts::avatar::AvatarManifest::parse(&s).ok()).filter(|m| m.avatar_id == id)
    else {
        return Content::default();
    };
    from_manifest(&manifest)
}
fn from_manifest(manifest: &crate::contracts::avatar::AvatarManifest) -> Content {
    let mut content = Content::parameters(manifest.parameters.iter().map(|p| p.name.as_str()));
    for control in &manifest.controls {
        if control.has(crate::contracts::avatar::Tag::Nsfw) {
            content.explicit_reasons.push(format!("manifest-tag:nsfw:{}", control.path.join("/")));
        }
    }
    content.explicit_reasons.sort();
    content.explicit_reasons.dedup();
    content.explicit = !content.explicit_reasons.is_empty();
    content
}
impl Content {
    pub fn parameters<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut reasons: Vec<String> = names
            .into_iter()
            .filter_map(|n| {
                let n = n.strip_prefix("/avatar/parameters/").unwrap_or(n);
                matches_parameter(n).then(|| format!("osc-parameter:{n}"))
            })
            .collect();
        reasons.sort();
        reasons.dedup();
        Self { explicit: !reasons.is_empty(), explicit_reasons: reasons }
    }
    pub fn merge(&mut self, other: &Self) {
        self.explicit_reasons.extend(other.explicit_reasons.iter().cloned());
        self.explicit_reasons.sort();
        self.explicit_reasons.dedup();
        self.explicit |= other.explicit || !self.explicit_reasons.is_empty();
    }
    pub fn read(session: &Path) -> Self {
        std::fs::read(session.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    pub fn save(&self, session: &Path) -> anyhow::Result<()> {
        crate::write_atomic(&session.join(FILE), &serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}
/// VRCNext's real cached avatar tags, joined by avatar id and exact current instance. VRCX's cache_avatar has no
/// content labels in the inspected schema; its user-authored avatar_tags are not VRChat content labels.
pub fn local_avatar_labels(world: &crate::vrchat_log::WorldState) -> Content {
    let Some(location) = world.world_id.as_ref().zip(world.instance.as_ref()).filter(|_| world.in_world).map(|(w, i)| format!("{w}:{i}")) else {
        return Content::default();
    };
    let Some(path) = dirs::config_dir().map(|d| d.join("VRCNext/VRCNData.db")) else { return Content::default() };
    let Ok(db) = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX) else {
        return Content::default();
    };
    labels_in(&db, &location, &world.players.keys().cloned().collect::<Vec<_>>())
}
fn labels_in(db: &rusqlite::Connection, location: &str, players: &[String]) -> Content {
    let Ok(mut q) = db
        .prepare("SELECT u.user_id,a.tags FROM user_tracking u JOIN avatar_tracking a ON a.avatar_id=u.profile_current_avatar_id WHERE u.profile_location=?1")
    else {
        return Content::default();
    };
    let mut content = Content::default();
    if let Ok(rows) = q.query_map([location], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
        for (user, tags) in rows.flatten() {
            if !players.contains(&user) {
                continue;
            }
            if let Ok(tags) = serde_json::from_str::<Vec<String>>(&tags) {
                for tag in tags {
                    if matches!(tag.as_str(), "content_sex" | "sexually_explicit") {
                        content.explicit_reasons.push(format!("avatar-label:{}", tag));
                    }
                }
            }
        }
    }
    content.explicit_reasons.sort();
    content.explicit_reasons.dedup();
    content.explicit = !content.explicit_reasons.is_empty();
    content
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parameter_parts_case_and_reasons() {
        for n in ["NSFW/Toggle", "Clothes/NuDe", "Naked", "SPS/Enabled", "DpsOn", "TPS", "Penetration", "Orifice/1", "Genitals", "lewdness"] {
            assert!(matches_parameter(n), "{n}");
        }
        for n in ["VelocityX", "Clothes/Dress", "N4N0/Record", ""] {
            assert!(!matches_parameter(n), "{n}");
        }
        assert_eq!(Content::parameters(["/avatar/parameters/NSFW/Toggle"]).explicit_reasons, ["osc-parameter:NSFW/Toggle"]);
    }
    #[test]
    fn only_current_players_and_real_content_tags() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE user_tracking(user_id TEXT,profile_location TEXT,profile_current_avatar_id TEXT); CREATE TABLE avatar_tracking(avatar_id TEXT,tags TEXT); INSERT INTO user_tracking VALUES('one','here','a'),('two','elsewhere','b'); INSERT INTO avatar_tracking VALUES('a','[\"content_sex\",\"author_lewd_name\"]'),('b','[\"content_sexually_explicit\"]');").unwrap();
        assert_eq!(labels_in(&db, "here", &["one".into()]).explicit_reasons, ["avatar-label:content_sex"]);
        assert!(!labels_in(&db, "here", &[]).explicit);
    }
    #[test]
    fn manifest_tags_flag_controls_with_ordinary_parameter_names() {
        let m=crate::contracts::avatar::AvatarManifest::parse(r#"{"format":"orbiters.avatar","version":1,"avatar_id":"avtr_fixture","parameters":[{"name":"NSFW/Toggle","type":"bool","default":0}],"controls":[{"path":["Body","Hidden toggle"],"type":"toggle","parameter":"VF12/Option","tags":["nsfw"]},{"path":["Clothes","Shirt"],"type":"toggle","parameter":"Shirt","tags":["clothing"]}]}"#).unwrap();
        let content = from_manifest(&m);
        assert_eq!(content.explicit_reasons, ["manifest-tag:nsfw:Body/Hidden toggle", "osc-parameter:NSFW/Toggle"]);
        assert!(content.explicit);
    }
}
