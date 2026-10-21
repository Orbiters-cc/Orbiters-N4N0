//! What Orb's assistant is told about the moment (`context.summary` of a turn): where the owner is, what avatar they
//! wear and what it can do. Other people are counted, never named: their names stay on this computer.

/// The moment, as the desktop knows it.
#[derive(Clone, Debug, Default)]
pub struct Moment {
    pub vrchat_running: bool,
    pub world_name: Option<String>,
    /// People in the instance besides the owner.
    pub others: usize,
    pub avatar_name: Option<String>,
    /// The avatar carries the Orb prop (answers show on its panel).
    pub orb_prop: bool,
    pub afk: bool,
    /// The video the world played last, with its title when known.
    pub last_video: Option<(String, Option<String>)>,
    /// The avatar's controls (`Controls::summary`), or its bare parameters without a manifest.
    pub controls: String,
    pub controls_from_manifest: bool,
    pub saved_videos: usize,
}

impl Moment {
    pub fn summary(&self) -> String {
        let mut out = String::new();
        if !self.vrchat_running {
            out.push_str("VRChat is not running.\n");
        } else {
            match &self.world_name {
                Some(world) => out.push_str(&format!("In VRChat, in the world \"{world}\" with {} other people.\n", self.others)),
                None => out.push_str("VRChat is running (no world yet).\n"),
            }
            if self.afk {
                out.push_str("The owner is away (AFK).\n");
            }
        }
        if let Some(avatar) = &self.avatar_name {
            out.push_str(&format!("Avatar: \"{avatar}\"."));
            out.push_str(if self.orb_prop {
                " It carries the Orb prop: your answers show on its panel.\n"
            } else {
                " It has no Orb prop: answers show in Orbiters desktop only.\n"
            });
        }
        if let Some((url, title)) = &self.last_video {
            match title {
                Some(t) => out.push_str(&format!("Last video the world played: \"{t}\" ({url}).\n")),
                None => out.push_str(&format!("Last video the world played: {url}.\n")),
            }
        }
        if self.saved_videos > 0 {
            out.push_str(&format!("The owner has {} saved videos.\n", self.saved_videos));
        }
        if !self.controls.is_empty() {
            out.push_str(if self.controls_from_manifest {
                "Avatar controls (menu path, kind, state):\n"
            } else {
                "Avatar parameters (no menu labels known; name, kind, value):\n"
            });
            out.push_str(&self.controls);
        }
        crate::assistant::utf16_prefix(&out, crate::assistant::SUMMARY_UNITS).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_where_and_what_without_names() {
        let m = Moment {
            vrchat_running: true,
            world_name: Some("The Black Cat".into()),
            others: 11,
            avatar_name: Some("Rexouium".into()),
            orb_prop: true,
            last_video: Some(("https://youtu.be/x".into(), Some("Song".into()))),
            controls: "Clothes / Hoodie (toggle, off)\n".into(),
            controls_from_manifest: true,
            ..Default::default()
        };
        let s = m.summary();
        assert!(s.contains("\"The Black Cat\" with 11 other people"));
        assert!(s.contains("carries the Orb prop"));
        assert!(s.contains("\"Song\" (https://youtu.be/x)"));
        assert!(s.ends_with("Clothes / Hoodie (toggle, off)\n"));
        assert!(Moment::default().summary().starts_with("VRChat is not running"));
    }
}
