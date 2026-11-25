//! The videos the worlds' video players played, and the person's bookmark lists of them (the window's Videos page).
//!
//! VRChat's log names every URL a video player resolves (`[Video Playback] Attempting to resolve URL '<url>'`, on every
//! client of the instance, whoever pressed play), and VRChat deletes its old logs: the tray service keeps what it reads
//! in them ([`History`], `video-history.json` in the config folder), each video once (a YouTube video whatever its
//! link's form), with the world and the time it played last, when it played first and how many times; its title and
//! channel once YouTube's oEmbed gave them. The window reads that file and keeps the bookmark lists ([`Bookmarks`],
//! `video-bookmarks.json`), which nothing else changes. Nothing here needs a VRChat account.

use orbiters_i18n::tr;
use crate::vrchat_log::LogEvent;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::{Path, PathBuf};

/// Videos kept (those played longest ago go first).
const MAX_VIDEOS: usize = 5000;
/// Log bytes one [`History::read_logs_following`] reads at most: a first look at days of logs is spread over several
/// calls.
const READ_BUDGET: u64 = 64 << 20;
/// The same video resolved again within this many seconds of its last play is the same play (a player trying again,
/// someone joining the instance, a world syncing its player).
const SAME_PLAY: u64 = 300;
/// Longer URLs are left out (a script's data, a link signed to expire).
pub const MAX_URL: usize = 2000;
/// A bookmark list's name, in characters.
pub const LIST_NAME_MAX: usize = 40;

/// `video-history.json`, beside the visited worlds.
pub fn history_file() -> PathBuf {
    crate::paths::config_dir().join("video-history.json")
}

/// `video-bookmarks.json`.
pub fn bookmarks_file() -> PathBuf {
    crate::paths::config_dir().join("video-bookmarks.json")
}

/// A YouTube video's id, from the URL forms worlds use (`watch?v=`, `youtu.be/`, `shorts/`, `live/`, `embed/`).
pub fn youtube_id(url: &str) -> Option<String> {
    let rest = url.trim().split_once("://").map(|(_, r)| r).unwrap_or(url);
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

/// The same video whatever the URL's form: a YouTube video by its id, anything else as it is.
pub fn canonical(url: &str) -> String {
    youtube_id(url).map(|id| format!("https://www.youtube.com/watch?v={id}")).unwrap_or_else(|| url.trim().to_string())
}

/// Whether `link` opens in a browser: an `http` or `https` link with a host, on one line.
pub fn openable(link: &str) -> bool {
    let lower = link.trim().to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("https://").or_else(|| lower.strip_prefix("http://")) else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !host.starts_with('@') && !link.trim().chars().any(|c| c.is_whitespace() || c.is_control())
}

/// A video a world's player played.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Played {
    /// The link as the player was given it last.
    pub url: String,
    /// YouTube's title and channel, once its oEmbed gave them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// YouTube was asked for the title (whatever it answered): it is not asked again.
    #[serde(default)]
    pub looked_up: bool,
    /// Unix seconds of its first and last plays.
    pub first: u64,
    pub last: u64,
    #[serde(default = "one")]
    pub plays: u32,
    /// The world of its last play.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
}

fn one() -> u32 {
    1
}

impl Played {
    /// A YouTube video (its title can be asked for, its thumbnail shown).
    pub fn youtube(&self) -> Option<String> {
        youtube_id(&self.url)
    }
}

/// How far a log was read, and the world it was in there.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct LogRead {
    read: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    world_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    world: Option<String>,
    /// The name of the world being joined ("Entering Room" comes right before the join).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entering: Option<String>,
}

/// `video-history.json`: every video seen in VRChat's logs, the latest played first.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub videos: Vec<Played>,
    /// How far each log was read (file name → where its last complete line ends, and its world there).
    #[serde(default)]
    logs: BTreeMap<String, LogRead>,
    /// Each video's place in `videos`, by [`canonical`] link.
    #[serde(skip)]
    index: HashMap<String, usize>,
    #[serde(skip)]
    checkpoint: crate::Checkpoint,
}

impl History {
    /// Reads the videos kept. What does not fit (an entry from a newer version, a file cut short) is left out and the
    /// rest kept ([`crate::json_lenient`]); such a file is first kept beside as `video-history.json.bad-<time>`, since
    /// the next save replaces it and VRChat has deleted the logs it came from.
    pub fn load(path: &Path) -> Self {
        let text = match crate::read_text(path) {
            Ok(Some(text)) => text,
            Ok(None) => return Self::default(),
            Err(e) => {
                eprintln!("video history: {}: {e}", path.display());
                crate::keep_aside(path);
                return Self::default();
            }
        };
        let (mut history, dropped) = crate::json_lenient::<Self>(&text);
        if !dropped.is_empty() {
            let aside = crate::keep_aside(path);
            eprintln!("video history: {} left out of {}, the file kept as {aside:?}", dropped.join(", "), path.display());
        }
        history.sort();
        history
    }

    /// Written whole ([`crate::write_atomic`]).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::write_atomic(path, &serde_json::to_vec(self)?)?;
        Ok(())
    }

    /// Whether to save now ([`crate::Checkpoint`]): `moved`, the logs were read further since the last save.
    pub fn save_due(&self, moved: bool) -> bool {
        self.checkpoint.due(moved, std::time::Instant::now())
    }

    /// [`History::save`], remembered for [`History::save_due`] (a failure is tried again later).
    pub fn checkpoint(&mut self, path: &Path) -> anyhow::Result<()> {
        let saved = self.save(path);
        self.checkpoint.saved(saved.is_ok(), std::time::Instant::now());
        saved
    }

    /// The latest played first, and each video's place known again.
    fn sort(&mut self) {
        self.videos.sort_by(|a, b| b.last.cmp(&a.last).then_with(|| a.url.cmp(&b.url)));
        self.index = self.videos.iter().enumerate().map(|(i, v)| (canonical(&v.url), i)).collect();
        if self.index.len() < self.videos.len() {
            // Two entries of one video (a hand edit): the latest stays.
            let mut seen = std::collections::HashSet::new();
            self.videos.retain(|v| seen.insert(canonical(&v.url)));
            self.index = self.videos.iter().enumerate().map(|(i, v)| (canonical(&v.url), i)).collect();
        }
    }

    /// A play of `url` at `at` (Unix seconds) in a world; returns whether anything changed. The same video again within
    /// [`SAME_PLAY`] of its last play is that play.
    pub fn record(&mut self, url: &str, at: u64, world_id: Option<&str>, world: Option<&str>) -> bool {
        let url = url.trim();
        if url.is_empty() || url.len() > MAX_URL || url.chars().any(char::is_control) {
            return false;
        }
        if self.index.len() != self.videos.len() {
            self.sort();
        }
        let key = canonical(url);
        let Some(&i) = self.index.get(&key) else {
            let new = Played {
                url: url.to_string(),
                first: at,
                last: at,
                plays: 1,
                world_id: world_id.map(str::to_string),
                world: world.map(str::to_string),
                ..Default::default()
            };
            if self.videos.len() == MAX_VIDEOS {
                // Full: the one kept least (played longest ago, then by link, as the sort orders them) goes, or the new
                // one when it is that one, in place: no sort and no new index for every video a long history adds.
                let worse = |a: &Played, b: &Played| a.last < b.last || (a.last == b.last && a.url > b.url);
                let w = (0..self.videos.len()).reduce(|m, x| if worse(&self.videos[x], &self.videos[m]) { x } else { m }).expect("full");
                if worse(&new, &self.videos[w]) {
                    return false;
                }
                let gone = std::mem::replace(&mut self.videos[w], new);
                self.index.remove(&canonical(&gone.url));
                self.index.insert(key, w);
                return true;
            }
            self.videos.push(new);
            self.index.insert(key, self.videos.len() - 1);
            if self.videos.len() > MAX_VIDEOS {
                self.sort();
                self.videos.truncate(MAX_VIDEOS);
                self.sort();
            }
            return true;
        };
        let v = &mut self.videos[i];
        if at.abs_diff(v.last) <= SAME_PLAY {
            if at <= v.last {
                return false;
            }
            v.last = at;
        } else if at > v.last {
            v.plays += 1;
            v.last = at;
            v.url = url.to_string();
            v.world_id = world_id.map(str::to_string);
            v.world = world.map(str::to_string);
        } else {
            // A play before the last one (a log read out of order).
            v.plays += 1;
            v.first = v.first.min(at);
        }
        true
    }

    /// Reads what every `output_log_*.txt` in `dir` gained since the last call; returns whether anything changed.
    pub fn read_logs(&mut self, dir: &Path) -> bool {
        self.read_logs_following(dir, None)
    }

    /// [`History::read_logs`] next to the log follower, which is at byte `at` of the log named `following.0`: that log
    /// is read up to there only, after which the follower's reading is taken instead ([`History::follow`]). At most
    /// [`READ_BUDGET`] bytes a call: the rest is read by the next ones.
    pub fn read_logs_following(&mut self, dir: &Path, following: Option<(&str, u64)>) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else { return false };
        let mut logs: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (name.starts_with("output_log_") && name.ends_with(".txt")).then(|| (name, e.path()))
            })
            .collect();
        // Their names hold the start time: oldest first.
        logs.sort();
        let mut changed = false;
        let mut budget = READ_BUDGET;
        for (name, path) in &logs {
            let Ok(file) = File::open(path) else { continue };
            let Ok(len) = file.metadata().map(|m| m.len()) else { continue };
            // A log shorter than what was read of it was replaced: read again, from no world.
            let mut log = self.logs.get(name).cloned().filter(|l| l.read <= len).unwrap_or_default();
            let end = match following {
                Some((followed, at)) if followed == name.as_str() => at.min(len),
                _ => len,
            };
            if log.read >= end || budget == 0 {
                continue;
            }
            let from = log.read;
            let mut events = Vec::new();
            // Only joins, room names and videos count here: other lines are let go after a look at their first bytes.
            let mut lines = crate::vrchat_log::Lines::at(from);
            let _ = lines.read(&file, end, budget, |line| {
                let wanted = crate::vrchat_log::message(line).is_some_and(|m| {
                    m.starts_with(b"[Video Playback] Attempting to resolve URL '")
                        || m.starts_with(b"[Behaviour] Joining ")
                        || m.starts_with(b"[Behaviour] Entering Room: ")
                });
                if let Some(e) = wanted.then(|| crate::vrchat_log::parse_bytes(line)).flatten() {
                    events.push(e);
                }
            });
            let read = lines.taken();
            budget = budget.saturating_sub(lines.position() - from);
            for e in &events {
                changed |= self.observe(&mut log, e);
            }
            if self.logs.get(name).map(|l| l.read) != Some(read) {
                changed = true;
            }
            log.read = read;
            self.logs.insert(name.clone(), log);
        }
        // Forget the logs VRChat deleted.
        let before = self.logs.len();
        self.logs.retain(|name, _| logs.iter().any(|(n, _)| n == name));
        if changed {
            self.sort();
        }
        changed | (self.logs.len() != before)
    }

    /// What the log follower read of the log named `name`, bytes `from` to `to`, with its events: taken in place of
    /// reading them again once the videos are known up to `from` (until then [`History::read_logs_following`] reads
    /// that log itself). Returns whether anything changed.
    pub fn follow(&mut self, name: &str, from: u64, to: u64, events: &[LogEvent]) -> bool {
        let known = self.logs.get(name).map_or(0, |l| l.read);
        if to <= from || known != from {
            return false;
        }
        let mut log = self.logs.remove(name).unwrap_or_default();
        let mut changed = false;
        for e in events {
            changed |= self.observe(&mut log, e);
        }
        log.read = to;
        self.logs.insert(name.to_string(), log);
        if changed {
            self.sort();
        }
        true
    }

    // A join reads "Entering Room: <name>", "Joining wrld_…:<instance>", then "Joining or Creating Room: <name>".
    fn observe(&mut self, log: &mut LogRead, event: &LogEvent) -> bool {
        match event {
            LogEvent::EnteringRoom { name } => {
                log.entering = Some(name.clone());
                false
            }
            LogEvent::Joining { world_id, .. } => {
                log.world_id = Some(world_id.clone());
                log.world = log.entering.take();
                false
            }
            LogEvent::JoinedRoom { name } => {
                log.world = Some(name.clone());
                false
            }
            LogEvent::Video { url, at: Some(at) } => {
                let changed = self.record(url, *at, log.world_id.as_deref(), log.world.as_deref());
                self.checkpoint.unsaved |= changed;
                changed
            }
            _ => false,
        }
    }

    /// Up to `n` YouTube videos whose title was not asked for yet, the latest played first.
    pub fn wanting_titles(&self, n: usize) -> Vec<String> {
        self.videos.iter().filter(|v| !v.looked_up && v.youtube().is_some()).take(n).map(|v| v.url.clone()).collect()
    }

    /// What YouTube said of `url`: its title and channel, or nothing (a private or removed video). It is not asked
    /// again either way.
    pub fn set_title(&mut self, url: &str, found: Option<(String, String)>) -> bool {
        if self.index.len() != self.videos.len() {
            self.sort();
        }
        let Some(&i) = self.index.get(&canonical(url)) else { return false };
        let v = &mut self.videos[i];
        let (title, channel) = found.map_or((None, None), |(t, c)| (Some(t), Some(c).filter(|c| !c.is_empty())));
        let changed = !v.looked_up || v.title != title || v.channel != channel;
        self.checkpoint.unsaved |= changed;
        v.looked_up = true;
        if title.is_some() {
            v.title = title;
            v.channel = channel;
        }
        changed
    }

    /// The video of `url` (in any form), when it played.
    pub fn get(&self, url: &str) -> Option<&Played> {
        let key = canonical(url);
        match self.index.get(&key) {
            Some(&i) if self.index.len() == self.videos.len() => self.videos.get(i),
            _ => self.videos.iter().find(|v| canonical(&v.url) == key),
        }
    }
}

/// A video in a bookmark list: its link and what was known of it when it was added.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// The world it played in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub added: u64,
}

impl Bookmark {
    /// A bookmark of a video from the history, added at `now`.
    pub fn of(video: &Played, now: u64) -> Self {
        Self { url: video.url.clone(), title: video.title.clone(), channel: video.channel.clone(), world: video.world.clone(), added: now }
    }
}

/// A named list of videos, the latest added first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BookmarkList {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub videos: Vec<Bookmark>,
}

/// How reading the bookmarks went.
#[derive(Clone, Debug, PartialEq)]
pub enum Loaded {
    /// All of them (none when there was no file yet).
    Whole,
    /// Some could not be read (a hand edit, a newer version's): the others were kept, and the file was copied aside
    /// first (`<name>.bad-<seconds>`), so the next save loses nothing.
    Partial(Option<PathBuf>),
    /// The file could not be read at all (another program holds it, no permission): saving over it would lose it.
    Unreadable(String),
}

/// `video-bookmarks.json`: the person's bookmark lists, in the order made.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bookmarks {
    #[serde(default)]
    pub lists: Vec<BookmarkList>,
}

/// A name as lists keep it: on one line, trimmed, cut to [`LIST_NAME_MAX`] characters.
fn list_name(name: &str) -> String {
    let one_line: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    one_line.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(LIST_NAME_MAX).collect()
}

impl Bookmarks {
    /// The lists in `path`, and how reading them went.
    pub fn read(path: &Path) -> (Self, Loaded) {
        let text = match crate::read_text(path) {
            Ok(None) => return (Self::default(), Loaded::Whole),
            Ok(Some(text)) => text,
            Err(e) => return (Self::default(), Loaded::Unreadable(e.to_string())),
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        if let Ok(bookmarks) = serde_json::from_str::<Bookmarks>(text) {
            return (bookmarks, Loaded::Whole);
        }
        // List by list, video by video: what can be read stays.
        let lists = serde_json::from_str::<serde_json::Value>(text).ok().and_then(|v| v.get("lists").and_then(|l| l.as_array()).cloned()).unwrap_or_default();
        let lists = lists
            .into_iter()
            .filter_map(|list| {
                let id = list.get("id")?.as_str()?.to_string();
                let name = list.get("name")?.as_str()?.to_string();
                let videos = list.get("videos").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                Some(BookmarkList { id, name, videos: videos.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect() })
            })
            .collect();
        (Self { lists }, Loaded::Partial(crate::keep_aside(path)))
    }

    /// Written whole ([`crate::write_atomic`]).
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::write_atomic(path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }

    /// A new empty list named `name` (or "Favourites", then "List 2", "List 3"… when it has none); returns its id.
    pub fn create(&mut self, name: &str) -> String {
        let mut name = list_name(name);
        if name.is_empty() {
            // Named in the language now and saved so: a name stays what it was when it was made, as one typed does. So the first list made in another language counts too.
            let first = tr("Favourites");
            name = if self.lists.iter().any(|l| l.name == first || l.name == "Favourites") { // i18n: not shown (a list made in English)
                (2..).map(|n| tr!("List {number}", number = n)).find(|n| !self.lists.iter().any(|l| l.name == *n)).unwrap_or_default()
            } else {
                first.into()
            };
        }
        let id = loop {
            let id = format!("list-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
            if !self.lists.iter().any(|l| l.id == id) {
                break id;
            }
        };
        self.lists.push(BookmarkList { id: id.clone(), name, videos: Vec::new() });
        id
    }

    /// Renames list `id` (a name left empty keeps the old one).
    pub fn rename(&mut self, id: &str, name: &str) -> bool {
        let name = list_name(name);
        match self.lists.iter_mut().find(|l| l.id == id) {
            Some(list) if !name.is_empty() && list.name != name => {
                list.name = name;
                true
            }
            _ => false,
        }
    }

    pub fn delete(&mut self, id: &str) -> bool {
        let before = self.lists.len();
        self.lists.retain(|l| l.id != id);
        self.lists.len() != before
    }

    pub fn list(&self, id: &str) -> Option<&BookmarkList> {
        self.lists.iter().find(|l| l.id == id)
    }

    /// Whether list `id` holds the video of `url` (in any form).
    pub fn holds(&self, id: &str, url: &str) -> bool {
        let key = canonical(url);
        self.list(id).is_some_and(|l| l.videos.iter().any(|v| canonical(&v.url) == key))
    }

    /// Whether any list holds the video of `url`.
    pub fn any_holds(&self, url: &str) -> bool {
        let key = canonical(url);
        self.lists.iter().any(|l| l.videos.iter().any(|v| canonical(&v.url) == key))
    }

    /// Adds `video` at the top of list `id`; false when it holds it already (or there is no such list).
    pub fn add(&mut self, id: &str, video: Bookmark) -> bool {
        let key = canonical(&video.url);
        match self.lists.iter_mut().find(|l| l.id == id) {
            Some(list) if !list.videos.iter().any(|v| canonical(&v.url) == key) => {
                list.videos.insert(0, video);
                true
            }
            _ => false,
        }
    }

    /// Takes the video of `url` (in any form) out of list `id`.
    pub fn remove(&mut self, id: &str, url: &str) -> bool {
        let key = canonical(url);
        let Some(list) = self.lists.iter_mut().find(|l| l.id == id) else { return false };
        let before = list.videos.len();
        list.videos.retain(|v| canonical(&v.url) != key);
        list.videos.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AS-30: a full history keeps the same videos as a sort and a cut would, without sorting for each new one.
    #[test]
    fn a_full_history_lets_the_oldest_go_as_a_sort_would() {
        let mut h = History::default();
        let url = |i: u64| format!("https://example.com/v{i}");
        for i in 0..MAX_VIDEOS as u64 {
            assert!(h.record(&url(i), 10_000 + i * 1000, None, None));
        }
        h.sort();
        // Older than every kept one: not kept, nothing changed.
        assert!(!h.record(&url(9_000_000), 5, None, None));
        assert_eq!(h.videos.len(), MAX_VIDEOS);
        assert!(h.get(&url(9_000_000)).is_none());
        // Newer: the oldest goes. Same last play as the next oldest: the link orders them, as the sort does.
        assert!(h.record(&url(9_000_001), 99_999_999, None, None));
        assert!(h.get(&url(0)).is_none() && h.get(&url(9_000_001)).is_some());
        let tie = 10_000 + 1000;
        assert!(h.record("https://example.com/a-first", tie, None, None), "sorts before v1 at the same time: v1 goes");
        assert!(h.get(&url(1)).is_none() && h.get("https://example.com/a-first").is_some());
        assert_eq!(h.videos.len(), MAX_VIDEOS);
        // Each one still found where it is.
        for v in &h.videos {
            assert_eq!(h.get(&v.url).map(|p| &p.url), Some(&v.url));
        }
    }

    const A: &str = "wrld_99123966-6b37-4b5b-9f3c-0a1b2c3d4e5f";
    const B: &str = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";

    fn line(time: &str, message: &str) -> String {
        format!("2026.10.18 {time} Debug      -  {message}\n")
    }

    fn video(time: &str, url: &str) -> String {
        line(time, &format!("[Video Playback] Attempting to resolve URL '{url}'"))
    }

    fn join(time: &str, world: &str, name: &str) -> String {
        format!(
            "{}{}{}",
            line(time, &format!("[Behaviour] Entering Room: {name}")),
            line(time, &format!("[Behaviour] Joining {world}:1~region(eu)")),
            line(time, &format!("[Behaviour] Joining or Creating Room: {name}"))
        )
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-videos-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_youtube_ids_and_links() {
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
        assert_eq!(canonical(" https://vimeo.com/123 "), "https://vimeo.com/123");
        assert!(openable("https://stream.vrcdn.live/live/abc.live.ts"));
        assert!(!openable("rtspt://stream.vrcdn.live/live/abc") && !openable("https://") && !openable("https://a b"));
    }

    #[test]
    fn keeps_each_video_once_with_its_world_and_plays() {
        let dir = temp("history");
        let log = dir.join("output_log_2026-10-18_20-17-45.txt");
        let text = [
            join("20:23:20", A, "Movie Night"),
            video("20:30:00", "https://youtu.be/36YnV9STBqc?si=x"),
            // The player trying again, and someone joining: the same play.
            video("20:30:02", "https://www.youtube.com/watch?v=36YnV9STBqc"),
            video("20:31:00", "https://www.youtube.com/watch?v=36YnV9STBqc"),
            line("20:40:00", "[Script] [Video Playback] Attempting to resolve URL 'https://evil.example/not-a-player'"),
            video("20:45:00", "https://stream.vrcdn.live/live/club.live.ts"),
            line("20:59:59", "[Behaviour] OnLeftRoom"),
            join("21:00:00", B, "The Club"),
            video("21:10:00", "https://youtube.com/watch?v=36YnV9STBqc&t=30"),
        ]
        .concat();
        std::fs::write(&log, &text[..text.len() - 20]).unwrap();
        let mut h = History::default();
        assert!(h.read_logs(&dir));
        assert_eq!(h.videos.len(), 2, "{:#?}", h.videos);
        let first = h.get("https://www.youtube.com/watch?v=36YnV9STBqc").unwrap().clone();
        assert_eq!((first.plays, first.world.as_deref(), first.world_id.as_deref()), (1, Some("Movie Night"), Some(A)));
        assert_eq!(first.last - first.first, 60, "the same play, its last resolve kept");
        assert_eq!(h.videos[0].url, "https://stream.vrcdn.live/live/club.live.ts", "the latest played first");
        // The rest of the log, its last line now whole: played again in another world.
        std::fs::write(&log, &text).unwrap();
        assert!(h.read_logs(&dir));
        let again = h.get("https://youtu.be/36YnV9STBqc").unwrap();
        assert_eq!((again.plays, again.world.as_deref()), (2, Some("The Club")));
        assert_eq!(again.url, "https://youtube.com/watch?v=36YnV9STBqc&t=30", "as played last");
        assert_eq!(h.videos[0].url, again.url);
        assert!(!h.read_logs(&dir), "nothing new");
        // Saved and read back, the reading's place and world with it.
        let path = dir.join("video-history.json");
        h.save(&path).unwrap();
        let mut back = History::load(&path);
        assert_eq!(back.videos, h.videos);
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
        f.write_all(video("22:00:00", "https://vimeo.com/76979871").as_bytes()).unwrap();
        drop(f);
        assert!(back.read_logs(&dir));
        assert_eq!(back.get("https://vimeo.com/76979871").and_then(|v| v.world.as_deref()), Some("The Club"), "the world kept with the log");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_newest_log_is_read_once_beside_the_follower() {
        let dir = temp("follow");
        let name = "output_log_2026-10-18_20-17-45.txt";
        let first = format!("{}{}", join("20:23:20", A, "Movie Night"), video("20:30:00", "https://youtu.be/36YnV9STBqc"));
        std::fs::write(dir.join(name), &first).unwrap();
        let mut h = History::default();
        assert!(h.read_logs_following(&dir, Some((name, first.len() as u64))));
        assert_eq!(h.videos.len(), 1);
        let more = video("21:00:00", "https://vimeo.com/1");
        std::fs::write(dir.join(name), format!("{first}{more}")).unwrap();
        let to = (first.len() + more.len()) as u64;
        let events = vec![crate::vrchat_log::parse_line(more.trim_end()).unwrap()];
        assert!(!h.follow(name, 3, to, &events), "not where the history is");
        assert!(h.follow(name, first.len() as u64, to, &events));
        assert_eq!(h.get("https://vimeo.com/1").and_then(|v| v.world.as_deref()), Some("Movie Night"));
        assert!(!h.read_logs_following(&dir, Some((name, to))), "nothing left to read");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn titles_are_asked_once_for_youtube_videos() {
        let mut h = History::default();
        h.record("https://vimeo.com/1", 10, None, None);
        h.record("https://youtu.be/36YnV9STBqc", 20, None, None);
        h.record("https://www.youtube.com/watch?v=dQw4w9WgXcQ", 30, None, None);
        h.sort();
        assert_eq!(h.wanting_titles(5), vec!["https://www.youtube.com/watch?v=dQw4w9WgXcQ", "https://youtu.be/36YnV9STBqc"], "the latest first");
        assert!(h.set_title("https://youtube.com/watch?v=dQw4w9WgXcQ", Some(("Never Gonna Give You Up".into(), "Rick Astley".into()))));
        assert!(h.set_title("https://youtu.be/36YnV9STBqc", None), "a private video: asked, no title");
        assert!(h.wanting_titles(5).is_empty());
        assert_eq!(h.get("https://youtu.be/dQw4w9WgXcQ").and_then(|v| v.channel.as_deref()), Some("Rick Astley"));
        assert!(!h.set_title("https://youtu.be/36YnV9STBqc", None), "nothing new");
        assert!(!h.set_title("https://example.com/never-played", None));
        assert!(!h.record("", 1, None, None) && !h.record(&format!("https://a.b/{}", "x".repeat(MAX_URL)), 1, None, None));
    }

    #[test]
    fn a_history_that_does_not_fit_keeps_what_does_and_the_file() {
        let dir = temp("bad");
        let path = dir.join("video-history.json");
        std::fs::write(&path, "\u{feff}{\"videos\":[{\"url\":\"https://vimeo.com/1\",\"first\":1,\"last\":2}],\"logs\":7}").unwrap();
        let h = History::load(&path);
        assert_eq!(h.videos.len(), 1);
        assert_eq!(h.videos[0].plays, 1);
        let kept = std::fs::read_dir(&dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("video-history.json.bad-"));
        assert!(kept);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn bookmark_lists_are_made_filled_renamed_and_deleted() {
        let mut b = Bookmarks::default();
        let fav = b.create("  ");
        assert_eq!(b.list(&fav).unwrap().name, "Favourites", "the first list without a name");
        let movies = b.create("  Movie\nnight  ");
        assert_eq!(b.list(&movies).unwrap().name, "Movie night");
        let third = b.create("");
        assert_eq!(b.list(&third).unwrap().name, "List 2");
        let mark = |url: &str| Bookmark { url: url.into(), added: 5, ..Default::default() };
        assert!(b.add(&fav, mark("https://youtu.be/36YnV9STBqc")));
        assert!(!b.add(&fav, mark("https://www.youtube.com/watch?v=36YnV9STBqc")), "already there, whatever the link's form");
        assert!(b.add(&fav, mark("https://vimeo.com/1")));
        assert_eq!(b.list(&fav).unwrap().videos[0].url, "https://vimeo.com/1", "the latest added first");
        assert!(b.holds(&fav, "https://youtube.com/watch?v=36YnV9STBqc") && !b.holds(&movies, "https://vimeo.com/1"));
        assert!(b.any_holds("https://vimeo.com/1") && !b.any_holds("https://vimeo.com/2"));
        assert!(!b.add("no-such-list", mark("https://vimeo.com/2")));
        assert!(b.rename(&movies, "Cinema") && !b.rename(&movies, " ") && !b.rename(&movies, "Cinema"));
        assert!(b.remove(&fav, "https://www.youtube.com/watch?v=36YnV9STBqc") && !b.remove(&fav, "https://youtu.be/36YnV9STBqc"));
        assert!(b.delete(&movies) && !b.delete(&movies));
        assert_eq!(b.lists.len(), 2);
        assert_eq!(list_name(&"x".repeat(80)).chars().count(), LIST_NAME_MAX);

        let dir = temp("bookmarks");
        let path = dir.join("video-bookmarks.json");
        assert_eq!(Bookmarks::read(&path), (Bookmarks::default(), Loaded::Whole), "no file yet");
        b.save(&path).unwrap();
        assert_eq!(Bookmarks::read(&path), (b.clone(), Loaded::Whole));
        // A video from a newer version (no url) and a list without a name: the rest is kept, the file set aside.
        let text = format!(
            "{{\"lists\":[{{\"id\":\"{fav}\",\"name\":\"Favourites\",\"videos\":[{{\"title\":\"x\"}},{{\"url\":\"https://vimeo.com/1\",\"added\":3}}]}},{{\"id\":\"l\"}}]}}"
        );
        std::fs::write(&path, &text).unwrap();
        let (read, loaded) = Bookmarks::read(&path);
        assert_eq!(read.lists.len(), 1);
        assert_eq!(read.lists[0].videos.len(), 1);
        let Loaded::Partial(Some(aside)) = loaded else { panic!("{loaded:?}") };
        assert_eq!(std::fs::read_to_string(aside).unwrap(), text);
        std::fs::remove_dir_all(dir).ok();
    }

    /// `cargo test -p orbiters-core reads_this_computers_videos -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn reads_this_computers_videos() {
        let mut h = History::default();
        let t = std::time::Instant::now();
        while h.read_logs(&crate::paths::vrchat_log_dir().expect("VRChat's log folder")) {}
        println!("{} videos from the logs in {:?}", h.videos.len(), t.elapsed());
        for v in h.videos.iter().take(40) {
            println!("  {} ×{} {:?} {}", v.last, v.plays, v.world, v.url);
        }
    }
}
