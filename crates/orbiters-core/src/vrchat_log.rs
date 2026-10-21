//! Follows VRChat's `output_log_*.txt` (the same source VRCX uses) for the current world and instance, who is in it,
//! the avatar the local player wears and the videos the world plays. Reading a local log needs no VRChat account
//! access.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogEvent {
    /// `[Behaviour] Joining wrld_…:instance`
    Joining { world_id: String, instance: String },
    /// `[Behaviour] Entering Room: <name>` (also "Joining or Creating Room")
    EnteringRoom { name: String },
    /// `[Behaviour] OnLeftRoom`
    LeftRoom,
    /// `[Behaviour] Initialized PlayerAPI "<name>" is local`
    LocalPlayer { name: String },
    /// `[Behaviour] OnPlayerJoined <name> (usr_…)` (the local player too)
    PlayerJoined { name: String, user_id: String },
    /// `[Behaviour] OnPlayerLeft <name> (usr_…)`
    PlayerLeft { name: String, user_id: String },
    /// `[Behaviour] Switching <name> to avatar <avatar>` (every player's avatar changes)
    AvatarSwitched { player: String, avatar: String },
    /// `[Video Playback] Attempting to resolve URL '<url>'`, with the line's time (Unix seconds)
    Video { url: String, at: Option<u64> },
}

/// A video a video player of the world played.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoSeen {
    pub url: String,
    /// When it started (Unix seconds, from the log line).
    #[serde(default)]
    pub at: Option<u64>,
}

/// What the log says about where the player is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldState {
    pub world_id: Option<String>,
    pub world_name: Option<String>,
    pub instance: Option<String>,
    pub in_world: bool,
    /// The log file being followed.
    pub log_file: Option<PathBuf>,
    /// The local player's display name.
    #[serde(default)]
    pub local_player: Option<String>,
    /// Who is in the instance (the local player too): display names by user id (by `name:<display name>` when the
    /// log gives no id).
    #[serde(default)]
    pub players: BTreeMap<String, String>,
    /// The name of the avatar the local player wears, as the log last named it.
    #[serde(default)]
    pub avatar_name: Option<String>,
    /// The avatar each person in the instance wears, as the log last named it, by display name (the local player too).
    #[serde(default)]
    pub avatars: BTreeMap<String, String>,
    /// The last video a video player of the world played in this instance.
    #[serde(default)]
    pub last_video: Option<VideoSeen>,
}

impl WorldState {
    pub fn apply(&mut self, e: &LogEvent) {
        match e {
            LogEvent::Joining { world_id, instance } => {
                if self.world_id.as_deref() != Some(world_id.as_str()) {
                    self.world_name = None;
                }
                self.world_id = Some(world_id.clone());
                self.instance = Some(instance.clone());
                self.in_world = true;
                // Everyone in the new instance, the local player included, joins again.
                self.players.clear();
                self.avatars.clear();
                self.last_video = None;
            }
            LogEvent::EnteringRoom { name } => {
                self.world_name = Some(name.clone());
                self.in_world = true;
            }
            LogEvent::LeftRoom => {
                self.in_world = false;
                self.players.clear();
                self.avatars.clear();
            }
            LogEvent::LocalPlayer { name } => self.local_player = Some(name.clone()),
            LogEvent::PlayerJoined { name, user_id } => {
                self.players.insert(player_key(name, user_id), name.clone());
            }
            LogEvent::PlayerLeft { name, user_id } => {
                self.players.remove(&player_key(name, user_id));
                self.avatars.remove(name);
            }
            LogEvent::AvatarSwitched { player, avatar } => {
                if self.local_player.as_deref() == Some(player.as_str()) {
                    self.avatar_name = Some(avatar.clone());
                }
                self.avatars.insert(player.clone(), avatar.clone());
            }
            LogEvent::Video { url, at } => self.last_video = Some(VideoSeen { url: url.clone(), at: *at }),
        }
    }

    /// The avatars the other people wear, by avatar name, with how many wear each: who wears them stays out.
    pub fn other_avatars(&self) -> BTreeMap<String, usize> {
        let local = self.local_player.as_deref();
        let mut out = BTreeMap::new();
        for (player, avatar) in &self.avatars {
            if Some(player.as_str()) != local {
                *out.entry(avatar.clone()).or_insert(0) += 1;
            }
        }
        out
    }

    /// People in the instance other than the local player.
    pub fn others(&self) -> usize {
        let local = self.local_player.as_deref();
        self.players.values().filter(|n| Some(n.as_str()) != local).count()
    }
}

fn player_key(name: &str, user_id: &str) -> String {
    if user_id.is_empty() {
        format!("name:{name}")
    } else {
        user_id.to_string()
    }
}

/// `<name> (usr_…)` → name and id; a name alone has an empty id.
fn name_and_id(rest: &str) -> (String, String) {
    let rest = rest.trim();
    if let Some(i) = rest.rfind(" (usr_") {
        if let Some(id) = rest[i + 2..].strip_suffix(')') {
            return (rest[..i].to_string(), id.to_string());
        }
    }
    (rest.to_string(), String::new())
}

/// Parses one log line.
pub fn parse_line(line: &str) -> Option<LogEvent> {
    if let Some(i) = line.find("[Behaviour] Joining wrld_") {
        let rest = &line[i + "[Behaviour] Joining ".len()..];
        let token = rest.split_whitespace().next()?;
        let (world, instance) = token.split_once(':').unwrap_or((token, ""));
        return Some(LogEvent::Joining { world_id: world.to_string(), instance: instance.to_string() });
    }
    for marker in ["[Behaviour] Entering Room: ", "[Behaviour] Joining or Creating Room: "] {
        if let Some(i) = line.find(marker) {
            let name = line[i + marker.len()..].trim();
            if !name.is_empty() {
                return Some(LogEvent::EnteringRoom { name: name.to_string() });
            }
        }
    }
    if line.contains("[Behaviour] OnLeftRoom") {
        return Some(LogEvent::LeftRoom);
    }
    if let Some(i) = line.find("[Behaviour] OnPlayerJoined ") {
        let (name, user_id) = name_and_id(&line[i + "[Behaviour] OnPlayerJoined ".len()..]);
        return (!name.is_empty()).then_some(LogEvent::PlayerJoined { name, user_id });
    }
    if let Some(i) = line.find("[Behaviour] OnPlayerLeft ") {
        let (name, user_id) = name_and_id(&line[i + "[Behaviour] OnPlayerLeft ".len()..]);
        return (!name.is_empty()).then_some(LogEvent::PlayerLeft { name, user_id });
    }
    if let Some(i) = line.find("[Behaviour] Initialized PlayerAPI \"") {
        let rest = &line[i + "[Behaviour] Initialized PlayerAPI \"".len()..];
        if let Some(name) = rest.trim_end().strip_suffix("\" is local") {
            return Some(LogEvent::LocalPlayer { name: name.to_string() });
        }
        return None;
    }
    if let Some(i) = line.find("[Behaviour] Switching ") {
        let rest = &line[i + "[Behaviour] Switching ".len()..];
        let (player, avatar) = rest.split_once(" to avatar ")?;
        return Some(LogEvent::AvatarSwitched { player: player.to_string(), avatar: avatar.trim_end().to_string() });
    }
    if let Some(i) = line.find("[Video Playback] Attempting to resolve URL '") {
        let rest = &line[i + "[Video Playback] Attempting to resolve URL '".len()..];
        let url = rest.trim_end().strip_suffix('\'')?;
        return (!url.is_empty()).then(|| LogEvent::Video { url: url.to_string(), at: crate::visited::log_time(line) });
    }
    None
}

/// Newest `output_log_*.txt` in a folder.
pub fn newest_log(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with("output_log_") && n.ends_with(".txt")
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

/// Incremental follower: call [`LogWatcher::poll`] periodically.
pub struct LogWatcher {
    dir: Option<PathBuf>,
    file: Option<PathBuf>,
    position: u64,
    partial: Vec<u8>,
    pub state: WorldState,
}

impl LogWatcher {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir, file: None, position: 0, partial: Vec::new(), state: WorldState::default() }
    }

    /// Reads what was appended since the last call; returns the new events (state is updated too).
    pub fn poll(&mut self) -> Vec<LogEvent> {
        let mut events = Vec::new();
        let Some(dir) = self.dir.clone().or_else(crate::paths::vrchat_log_dir) else { return events };
        let Some(newest) = newest_log(&dir) else { return events };
        if self.file.as_ref() != Some(&newest) {
            // A new VRChat run: start from the beginning of its log, forget the previous world.
            self.file = Some(newest.clone());
            self.position = 0;
            self.partial.clear();
            self.state = WorldState { log_file: Some(newest.clone()), ..Default::default() };
        }
        let Ok(mut f) = File::open(&newest) else { return events };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.position {
            self.position = 0; // truncated
            self.partial.clear();
        }
        if len == self.position || f.seek(SeekFrom::Start(self.position)).is_err() {
            return events;
        }
        let mut buf = Vec::with_capacity((len - self.position).min(64 << 20) as usize);
        if f.take(64 << 20).read_to_end(&mut buf).is_err() {
            return events;
        }
        self.position += buf.len() as u64;
        self.partial.extend_from_slice(&buf);
        let mut start = 0;
        for i in 0..self.partial.len() {
            if self.partial[i] == b'\n' {
                let line = String::from_utf8_lossy(&self.partial[start..i]);
                if let Some(e) = parse_line(&line) {
                    self.state.apply(&e);
                    events.push(e);
                }
                start = i + 1;
            }
        }
        self.partial.drain(..start);
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lines() {
        assert_eq!(
            parse_line("2026.10.18 20:23:20 Debug      -  [Behaviour] Joining wrld_99123966-6b37:77505~group(grp_9)~groupAccessType(public)~region(eu)"),
            Some(LogEvent::Joining { world_id: "wrld_99123966-6b37".into(), instance: "77505~group(grp_9)~groupAccessType(public)~region(eu)".into() })
        );
        assert_eq!(
            parse_line("2026.10.18 20:23:20 Debug      -  [Behaviour] Entering Room: VRC Volleyball"),
            Some(LogEvent::EnteringRoom { name: "VRC Volleyball".into() })
        );
        assert_eq!(parse_line("[Behaviour] OnLeftRoom"), Some(LogEvent::LeftRoom));
        assert_eq!(parse_line("2026.10.18 20:23:18 Warning    -  Path already exists, skipping: /usercamera/Pose"), None);
    }

    #[test]
    fn parses_people_avatars_and_videos() {
        assert_eq!(
            parse_line("2026.10.18 20:23:34 Debug      -  [Behaviour] OnPlayerJoined Some One (2) (usr_8784f94a-27ed)"),
            Some(LogEvent::PlayerJoined { name: "Some One (2)".into(), user_id: "usr_8784f94a-27ed".into() })
        );
        assert_eq!(
            parse_line("2026.10.18 20:34:02 Debug      -  [Behaviour] OnPlayerLeft Old Name"),
            Some(LogEvent::PlayerLeft { name: "Old Name".into(), user_id: String::new() })
        );
        assert_eq!(parse_line("[Behaviour] OnPlayerLeftRoom"), None);
        assert_eq!(
            parse_line("2026.10.18 20:23:34 Debug      -  [Behaviour] Initialized PlayerAPI \"Me Myself\" is local"),
            Some(LogEvent::LocalPlayer { name: "Me Myself".into() })
        );
        assert_eq!(parse_line("2026.10.18 20:23:34 Debug      -  [Behaviour] Initialized PlayerAPI \"You\" is remote"), None);
        assert_eq!(
            parse_line("2026.10.18 20:23:33 Debug      -  [Behaviour] Switching Me Myself to avatar Orbit v4"),
            Some(LogEvent::AvatarSwitched { player: "Me Myself".into(), avatar: "Orbit v4".into() })
        );
        let video = parse_line("2026.10.18 21:33:13 Debug      -  [Video Playback] Attempting to resolve URL 'https://www.youtube.com/watch?v=36YnV9STBqc'");
        match video {
            Some(LogEvent::Video { url, at }) => {
                assert_eq!(url, "https://www.youtube.com/watch?v=36YnV9STBqc");
                assert!(at.is_some());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn knows_who_is_there_and_what_i_wear() {
        let mut s = WorldState::default();
        for line in [
            "[Behaviour] Joining wrld_a:1",
            "[Behaviour] Initialized PlayerAPI \"Me\" is local",
            "[Behaviour] OnPlayerJoined Me (usr_me)",
            "[Behaviour] OnPlayerJoined Friend (usr_f)",
            "[Behaviour] Switching Friend to avatar Fox",
            "[Behaviour] Switching Me to avatar Orbit",
            "[Video Playback] Attempting to resolve URL 'https://youtu.be/x'",
        ] {
            s.apply(&parse_line(line).unwrap());
        }
        assert_eq!(s.players.len(), 2);
        assert_eq!(s.others(), 1);
        assert_eq!(s.avatar_name.as_deref(), Some("Orbit"));
        assert_eq!(s.last_video.as_ref().map(|v| v.url.as_str()), Some("https://youtu.be/x"));
        assert_eq!(s.other_avatars(), [("Fox".to_string(), 1)].into(), "names of avatars only, mine left out");
        s.apply(&parse_line("[Behaviour] OnPlayerLeft Friend (usr_f)").unwrap());
        assert_eq!(s.others(), 0);
        assert!(s.other_avatars().is_empty());
        s.apply(&parse_line("[Behaviour] Joining wrld_b:2").unwrap());
        assert!(s.players.is_empty() && s.last_video.is_none(), "a new instance starts empty");
        assert_eq!(s.avatar_name.as_deref(), Some("Orbit"), "the avatar stays");
    }

    #[test]
    fn follows_a_growing_log() {
        let dir = std::env::temp_dir().join(format!("orbiters-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("output_log_2026-10-20_00-00-00.txt");
        std::fs::write(&path, "x\n[Behaviour] Joining wrld_a:1\n[Behaviour] Entering Room: A\npartial").unwrap();
        let mut w = LogWatcher::new(Some(dir.clone()));
        assert_eq!(w.poll().len(), 2);
        assert_eq!(w.state.world_name.as_deref(), Some("A"));
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, " line\n[Behaviour] OnLeftRoom").unwrap();
        drop(f);
        assert_eq!(w.poll(), vec![LogEvent::LeftRoom]);
        assert!(!w.state.in_world);
        std::fs::remove_dir_all(dir).ok();
    }
}
