//! Saved videos: what a world's video player played (VRChat's log names every URL it resolves), kept on this computer
//! when the person asks N4N0 to ("save me this video").

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

/// `<config>/n4n0/videos.json` (N4N0's earlier folder moved there first, [`migrate`]).
pub fn file() -> PathBuf {
    let config = orbiters_core::paths::config_dir();
    migrate(&config);
    config.join(DIR).join("videos.json")
}

/// N4N0's folder in the config folder.
const DIR: &str = "n4n0";

/// The folder N4N0 kept its saved videos in before the companion took its name (0.3.4-dev.8 and earlier).
const LEGACY_DIR: &str = "orb";

/// Moves N4N0's earlier folder in `config` to today's, all of it (the list and the copies kept aside), once: when
/// today's is there already, only an earlier list it lacks moves, and the earlier folder goes once empty. What cannot
/// be moved stays where it was, and is said.
pub fn migrate(config: &Path) {
    let (legacy, dir) = (config.join(LEGACY_DIR), config.join(DIR));
    if !legacy.is_dir() {
        return;
    }
    if !dir.exists() {
        if let Err(e) = std::fs::rename(&legacy, &dir) {
            eprintln!("videos: {} could not move to {}: {e}", legacy.display(), dir.display());
        }
        return;
    }
    let (from, to) = (legacy.join("videos.json"), dir.join("videos.json"));
    if from.is_file() && !to.exists() {
        if let Err(e) = std::fs::rename(&from, &to) {
            eprintln!("videos: {} could not move to {}: {e}", from.display(), to.display());
        }
    }
    let _ = std::fs::remove_dir(&legacy);
}

/// How reading the saved videos went.
#[derive(Clone, Debug, PartialEq)]
pub enum Loaded {
    /// The whole list (an empty one when there was no file yet).
    Whole,
    /// Some entries could not be read (a hand edit, a newer version's): the others were kept, and the file was copied
    /// aside first (`<name>.bad-<seconds>`), so the next save loses nothing.
    Partial(Option<PathBuf>),
    /// The file could not be read at all (another program holds it, no permission): saving over it would lose it.
    Unreadable(String),
}

impl Videos {
    /// The list in `path`, and how reading it went.
    pub fn read(path: &Path) -> (Self, Loaded) {
        let text = match orbiters_core::read_text(path) {
            Ok(None) => return (Self::default(), Loaded::Whole),
            Ok(Some(text)) => text,
            Err(e) => return (Self::default(), Loaded::Unreadable(e.to_string())),
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        if let Ok(videos) = serde_json::from_str::<Videos>(text) {
            return (videos, Loaded::Whole);
        }
        // Entry by entry: what can be read stays.
        let entries =
            serde_json::from_str::<serde_json::Value>(text).ok().and_then(|v| v.get("videos").and_then(|l| l.as_array()).cloned()).unwrap_or_default();
        let videos = entries.into_iter().filter_map(|e| serde_json::from_value(e).ok()).collect();
        (Self { videos }, Loaded::Partial(orbiters_core::keep_aside(path)))
    }

    /// The list in `path`, whatever could be read of it.
    pub fn load(path: &Path) -> Self {
        Self::read(path).0
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

/// A YouTube video's id, from the URL forms worlds use (`watch?v=`, `youtu.be/`, `shorts/`, `live/`, `embed/`): the
/// video history's ([`orbiters_core::video_history::youtube_id`]).
pub fn youtube_id(url: &str) -> Option<String> {
    orbiters_core::video_history::youtube_id(url)
}

/// The same video whatever the URL's form.
pub fn canonical(url: &str) -> String {
    orbiters_core::video_history::canonical(url)
}

/// A URL's query component, percent-encoded.
pub(crate) fn encode(value: &str) -> String {
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
    youtube_oembed(url).ok().flatten().map(|(title, _)| title)
}

/// A YouTube video's title and channel through YouTube's oEmbed endpoint (no key): `Ok(None)` when YouTube has no such
/// public video to show (private, removed, not embeddable) or the link is not YouTube's, an error when YouTube could
/// not be asked (no network). Blocking, 5 s at most.
pub fn youtube_oembed(url: &str) -> anyhow::Result<Option<(String, String)>> {
    let Some(id) = youtube_id(url) else { return Ok(None) };
    let agent = orbiters_core::http_agent(None, Some(std::time::Duration::from_secs(5)));
    let link = format!("https://www.youtube.com/watch?v={id}");
    let request = agent.get(&format!("https://www.youtube.com/oembed?format=json&url={}", encode(&link)));
    let mut response = request.config().http_status_as_error(false).build().call()?;
    match response.status().as_u16() {
        200 => {}
        400..=499 => return Ok(None),
        status => anyhow::bail!("YouTube answered {status}"),
    }
    let text = response.body_mut().with_config().limit(64 * 1024).read_to_string()?;
    Ok(parse_oembed(&text))
}

/// The title and channel (`author_name`) of an oEmbed answer.
fn parse_oembed(text: &str) -> Option<(String, String)> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let title = value.get("title")?.as_str().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())?;
    let channel = value.get("author_name").and_then(|a| a.as_str()).map(|a| a.trim().to_string()).unwrap_or_default();
    Some((title, channel))
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
    fn reads_oembed_answers() {
        let text = r#"{"title":" Never Gonna Give You Up ","author_name":"Rick Astley","type":"video"}"#;
        assert_eq!(parse_oembed(text), Some(("Never Gonna Give You Up".into(), "Rick Astley".into())));
        assert_eq!(parse_oembed(r#"{"title":"Only a title"}"#), Some(("Only a title".into(), String::new())));
        assert_eq!(parse_oembed(r#"{"title":""}"#), None);
        assert_eq!(parse_oembed("Unauthorized"), None);
        assert!(matches!(youtube_oembed("https://vimeo.com/1"), Ok(None)), "not YouTube's: nothing asked");
    }

    /// Against YouTube itself: `cargo test -p orbiters-n4n0-core oembed_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn oembed_live() {
        let found = youtube_oembed("https://youtu.be/dQw4w9WgXcQ").unwrap();
        println!("{found:?}");
        assert_eq!(found.map(|(_, channel)| channel).as_deref(), Some("Rick Astley"));
        assert_eq!(youtube_oembed("https://www.youtube.com/watch?v=aaaaaaaaaaa").unwrap(), None, "no such video");
    }

    #[test]
    fn n4n0_s_earlier_folder_moves_once() {
        let config = std::env::temp_dir().join(format!("n4n0-videos-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&config);
        let (legacy, dir) = (config.join(LEGACY_DIR), config.join(DIR));
        std::fs::create_dir_all(&legacy).unwrap();
        let list = Videos { videos: vec![SavedVideo { url: "https://youtu.be/36YnV9STBqc".into(), title: None, world: None, saved_utc: "t".into() }] };
        list.save(&legacy.join("videos.json")).unwrap();
        std::fs::write(legacy.join("videos.json.bad-1"), "{").unwrap();
        migrate(&config);
        assert_eq!(Videos::read(&dir.join("videos.json")), (list.clone(), Loaded::Whole), "the list moved");
        assert!(dir.join("videos.json.bad-1").is_file(), "with what was kept aside");
        assert!(!legacy.exists(), "the earlier folder is gone");
        migrate(&config);
        assert_eq!(Videos::load(&dir.join("videos.json")), list, "a second time changes nothing");
        // Today's folder there already (another N4N0 file in it): an earlier list it lacks still moves.
        std::fs::remove_file(dir.join("videos.json")).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        list.save(&legacy.join("videos.json")).unwrap();
        migrate(&config);
        assert_eq!(Videos::load(&dir.join("videos.json")), list);
        assert!(!legacy.exists());
        // Both lists: today's is kept, the earlier one stays where it was.
        std::fs::create_dir_all(&legacy).unwrap();
        Videos::default().save(&legacy.join("videos.json")).unwrap();
        migrate(&config);
        assert_eq!(Videos::load(&dir.join("videos.json")), list);
        assert!(legacy.join("videos.json").is_file());
        std::fs::remove_dir_all(&config).ok();
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
        let path = std::env::temp_dir().join(format!("n4n0-videos-{}.json", std::process::id()));
        videos.save(&path).unwrap();
        assert_eq!(Videos::load(&path), videos);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn a_list_read_in_part_is_kept_aside_before_it_is_replaced() {
        let dir = std::env::temp_dir().join(format!("n4n0-videos-partial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("videos.json");
        assert_eq!(Videos::read(&path), (Videos::default(), Loaded::Whole), "no file yet");
        // One entry from a newer version (no saved_utc), one as this version writes them.
        let text =
            "\u{feff}{ \"videos\": [ { \"url\": \"https://vimeo.com/2\", \"rating\": 5 }, { \"url\": \"https://vimeo.com/1\", \"saved_utc\": \"t\" } ] }";
        std::fs::write(&path, text).unwrap();
        let (videos, loaded) = Videos::read(&path);
        assert_eq!(videos.videos.len(), 1);
        assert_eq!(videos.videos[0].url, "https://vimeo.com/1");
        let Loaded::Partial(Some(aside)) = loaded else { panic!("{loaded:?}") };
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), text, "the whole file, as it was");
        std::fs::remove_dir_all(dir).ok();
    }
}
