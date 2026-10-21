//! Saved videos: what a world's video player played (VRChat's log names every URL it resolves), kept on this computer
//! when the person asks Orb to ("save me this video").

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedVideo {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    /// The world it played in.
    #[serde(default)]
    pub world: Option<String>,
    pub saved_utc: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Videos {
    pub videos: Vec<SavedVideo>,
}

/// `<config>/orb/videos.json`.
pub fn file() -> PathBuf {
    orbiters_core::paths::config_dir().join("orb").join("videos.json")
}

impl Videos {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        orbiters_core::write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }

    /// Adds `video` first (a video saved again moves to the top, keeping a title it had); false when it was already
    /// the newest.
    pub fn add(&mut self, video: SavedVideo) -> bool {
        let key = canonical(&video.url);
        if self.videos.first().is_some_and(|v| canonical(&v.url) == key) {
            return false;
        }
        let earlier = self.videos.iter().position(|v| canonical(&v.url) == key).map(|i| self.videos.remove(i));
        let title = video.title.clone().or_else(|| earlier.and_then(|e| e.title));
        self.videos.insert(0, SavedVideo { title, ..video });
        true
    }

    pub fn remove(&mut self, url: &str) -> bool {
        let before = self.videos.len();
        self.videos.retain(|v| v.url != url);
        self.videos.len() != before
    }
}

/// A YouTube video's id, from the URL forms worlds use (`watch?v=`, `youtu.be/`, `shorts/`, `live/`, `embed/`).
pub fn youtube_id(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.trim_start_matches("www.").trim_start_matches("m.").trim_start_matches("music.");
    let id = match host {
        "youtu.be" => path.split(['?', '&', '#', '/']).next(),
        "youtube.com" => {
            if let Some(q) = path.strip_prefix("watch?").or_else(|| path.split_once("?").filter(|(p, _)| *p == "watch").map(|(_, q)| q)) {
                q.split('&').find_map(|kv| kv.strip_prefix("v="))
            } else {
                ["shorts/", "live/", "embed/"].iter().find_map(|p| path.strip_prefix(p)).and_then(|r| r.split(['?', '&', '#', '/']).next())
            }
        }
        _ => None,
    }?;
    let id: String = id.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (id.len() == 11).then_some(id)
}

/// The same video whatever the URL's form.
pub fn canonical(url: &str) -> String {
    youtube_id(url).map(|id| format!("https://www.youtube.com/watch?v={id}")).unwrap_or_else(|| url.trim().to_string())
}

/// A URL's query component, percent-encoded.
fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A YouTube video's title through YouTube's oEmbed endpoint (no key), `None` for other sites or on failure.
pub fn youtube_title(url: &str) -> Option<String> {
    let id = youtube_id(url)?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(5))).build().into();
    let link = format!("https://www.youtube.com/watch?v={id}");
    let mut response = agent.get(&format!("https://www.youtube.com/oembed?format=json&url={}", encode(&link))).call().ok()?;
    if response.status().as_u16() != 200 {
        return None;
    }
    let text = response.body_mut().with_config().limit(64 * 1024).read_to_string().ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("title")?.as_str().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_youtube_ids() {
        for url in [
            "https://www.youtube.com/watch?v=36YnV9STBqc",
            "https://youtube.com/watch?feature=share&v=36YnV9STBqc&t=12",
            "https://youtu.be/36YnV9STBqc?si=abc",
            "https://www.youtube.com/shorts/36YnV9STBqc",
            "https://m.youtube.com/live/36YnV9STBqc",
            "https://music.youtube.com/watch?v=36YnV9STBqc&list=x",
        ] {
            assert_eq!(youtube_id(url).as_deref(), Some("36YnV9STBqc"), "{url}");
        }
        assert_eq!(youtube_id("https://vimeo.com/123"), None);
        assert_eq!(youtube_id("https://www.youtube.com/watch?v=short"), None);
        assert_eq!(encode("https://a.b/?v=1&x=é"), "https%3A%2F%2Fa.b%2F%3Fv%3D1%26x%3D%C3%A9");
    }

    #[test]
    fn keeps_videos_newest_first_without_duplicates() {
        let v = |url: &str, title: Option<&str>| SavedVideo { url: url.into(), title: title.map(str::to_string), world: None, saved_utc: "t".into() };
        let mut videos = Videos::default();
        assert!(videos.add(v("https://youtu.be/36YnV9STBqc", Some("Song"))));
        assert!(videos.add(v("https://vimeo.com/1", None)));
        assert!(videos.add(v("https://www.youtube.com/watch?v=36YnV9STBqc", None)));
        assert_eq!(videos.videos.len(), 2);
        assert_eq!(videos.videos[0].title.as_deref(), Some("Song"), "the title saved before stays");
        assert!(!videos.add(v("https://youtu.be/36YnV9STBqc", None)), "already the newest");
        let path = std::env::temp_dir().join(format!("orb-videos-{}.json", std::process::id()));
        videos.save(&path).unwrap();
        assert_eq!(Videos::load(&path), videos);
        std::fs::remove_file(path).ok();
    }
}
