//! Follows VRChat's `output_log_*.txt` (the same source VRCX uses) for the current world and instance, who is in it,
//! the avatar the local player wears and the videos the world plays. Reading a local log needs no VRChat account
//! access.
//!
//! The log also holds text others write: what world scripts log (`Debug.Log`, over several lines too), and display
//! and avatar names inside VRChat's own lines. A line counts only in VRChat's own shape (`2026.10.18 20:23:20 Debug
//! -  ` then the message), with its marker at the very start of the message, and world and user ids that are ids.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogEvent {
    /// `[Behaviour] Joining wrld_…:instance`, with the line's time (Unix seconds)
    Joining { world_id: String, instance: String, at: Option<u64> },
    /// `[Behaviour] Entering Room: <name>`, which comes right before the join
    EnteringRoom { name: String },
    /// `[Behaviour] Joining or Creating Room: <name>`, which follows the join and completes it
    JoinedRoom { name: String },
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
    /// A line the N4N0 world wrote for Orbiters - N4N0 (`[N4N0-Feedback] {…}`, contracts `Docs/world-feedback.md`), with
    /// the line's time (Unix seconds): `message` is the whole message, marker included. Any world can write one; what is
    /// done with it depends on the world the log joined.
    WorldLine { message: String, at: Option<u64> },
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
            LogEvent::Joining { world_id, instance, .. } => {
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
            LogEvent::EnteringRoom { name } | LogEvent::JoinedRoom { name } => {
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
                let (player, avatar) = self.who_switched(player, avatar);
                if self.local_player.as_deref() == Some(player.as_str()) {
                    self.avatar_name = Some(avatar.clone());
                }
                self.avatars.insert(player, avatar);
            }
            LogEvent::Video { url, at } => self.last_video = Some(VideoSeen { url: url.clone(), at: *at }),
            LogEvent::WorldLine { .. } => {}
        }
    }

    /// Who switched to which avatar in `Switching <player> to avatar <avatar>`, which the parse cut at the first
    /// " to avatar ": a name can hold those words too ("Me to avatar X", next to the local "Me"). The longest name of
    /// someone here that the line starts with is who it is; with none, the parse's cut stands.
    fn who_switched(&self, player: &str, avatar: &str) -> (String, String) {
        const TO: &str = " to avatar ";
        let line = format!("{player}{TO}{avatar}");
        let known = self.players.values().chain(self.local_player.iter());
        let named = known.filter(|name| line.len() > name.len() + TO.len() && line.starts_with(name.as_str()) && line[name.len()..].starts_with(TO));
        match named.max_by_key(|name| name.len()) {
            Some(name) => (name.clone(), line[name.len() + TO.len()..].to_string()),
            None => (player.to_string(), avatar.to_string()),
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

    /// The instance as Orbiters knows it for N4N0's link cards (contracts `Docs/n4n0.md`, Link card): the lowercase hex
    /// SHA-256 of `<world id>:<instance>`, as VRChat's log writes them on joining. `None` outside a world: everyone in the
    /// same instance computes the same, and Orbiters never learns the world.
    pub fn instance_hash(&self) -> Option<String> {
        use sha2::{Digest, Sha256};
        let world = self.world_id.as_deref().filter(|_| self.in_world)?;
        let instance = self.instance.as_deref().filter(|i| !i.is_empty())?;
        let digest = Sha256::digest(format!("{world}:{instance}").as_bytes());
        Some(digest.iter().map(|b| format!("{b:02x}")).collect())
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

/// `prefix` and a lowercase UUID, as VRChat writes its ids (`wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b`).
pub fn is_vrchat_id(id: &str, prefix: &str) -> bool {
    let Some(uuid) = id.strip_prefix(prefix) else { return false };
    let b = uuid.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_digit() || (b'a'..=b'f').contains(c) })
}

/// A VRChat world id (`wrld_` and a UUID): what may go into a path of Orbiters' API or a session's manifest.
pub fn is_world_id(id: &str) -> bool {
    is_vrchat_id(id, "wrld_")
}

/// An instance as VRChat names it after the world (`77505~group(grp_…)~groupAccessType(public)~region(eu)`).
fn is_instance(instance: &str) -> bool {
    instance.len() <= 512 && instance.bytes().all(|c| c.is_ascii_alphanumeric() || b"~()_-.,:".contains(&c))
}

/// `<name> (usr_…)` → name and id; a name alone (or with something else in brackets) has an empty id.
fn name_and_id(rest: &str) -> (String, String) {
    let rest = rest.trim();
    if let Some(i) = rest.rfind(" (usr_") {
        if let Some(id) = rest[i + 2..].strip_suffix(')').filter(|id| is_vrchat_id(id, "usr_")) {
            return (rest[..i].to_string(), id.to_string());
        }
    }
    (rest.to_string(), String::new())
}

/// Where the message starts in a line of VRChat's own shape, `2026.10.18 20:23:20 Debug      -  <message>` (the level
/// padded with spaces); `None` for anything else, such as the further lines of a script's message.
fn message_start(line: &[u8]) -> Option<usize> {
    const DIGITS: [usize; 14] = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    let shaped = line.len() > 24
        && line[4] == b'.'
        && line[7] == b'.'
        && line[10] == b' '
        && line[13] == b':'
        && line[16] == b':'
        && line[19] == b' '
        && DIGITS.iter().all(|&i| line[i].is_ascii_digit());
    if !shaped {
        return None;
    }
    let level = 20 + line[20..].iter().take_while(|c| c.is_ascii_alphabetic()).count();
    if level == 20 {
        return None;
    }
    let dash = level + line[level..].iter().take_while(|&&c| c == b' ').count();
    (line.get(dash..dash + 3) == Some(b"-  ")).then_some(dash + 3)
}

const BEHAVIOUR: &[u8] = b"[Behaviour] ";
const VIDEO: &[u8] = b"[Video Playback] Attempting to resolve URL '";
const WORLD_LINE: &[u8] = orbiters_contracts::world_feedback::MARKER.as_bytes();

/// The message of a line in VRChat's own shape ([`message_start`]).
pub(crate) fn message(line: &[u8]) -> Option<&[u8]> {
    message_start(line).map(|start| &line[start..])
}

/// Parses one log line.
pub fn parse_line(line: &str) -> Option<LogEvent> {
    parse_bytes(line.as_bytes())
}

/// Parses one log line read as bytes. Most lines are none of VRChat's markers: they are let go after a look at their
/// first bytes, before any text is made of them.
pub(crate) fn parse_bytes(line: &[u8]) -> Option<LogEvent> {
    let start = message_start(line)?;
    let message = &line[start..];
    if !message.starts_with(BEHAVIOUR) && !message.starts_with(VIDEO) && !message.starts_with(WORLD_LINE) {
        return None;
    }
    // The part before the message is ASCII: the offsets stay where they were.
    let line = String::from_utf8_lossy(line);
    parse_message(line[start..].trim_end(), &line[..19])
}

/// A message's event; `stamp` is its line's time (`2026.10.18 20:23:20`).
fn parse_message(message: &str, stamp: &str) -> Option<LogEvent> {
    if message.starts_with(orbiters_contracts::world_feedback::MARKER) {
        let fits = message.len() <= orbiters_contracts::world_feedback::MAX_LINE;
        return fits.then(|| LogEvent::WorldLine { message: message.to_string(), at: crate::visited::log_time(stamp) });
    }
    if let Some(url) = message.strip_prefix("[Video Playback] Attempting to resolve URL '") {
        let url = url.strip_suffix('\'')?;
        return (!url.is_empty()).then(|| LogEvent::Video { url: url.to_string(), at: crate::visited::log_time(stamp) });
    }
    let rest = message.strip_prefix("[Behaviour] ")?;
    if let Some(name) = rest.strip_prefix("Joining or Creating Room: ") {
        let name = name.trim();
        return (!name.is_empty()).then(|| LogEvent::JoinedRoom { name: name.to_string() });
    }
    if let Some(target) = rest.strip_prefix("Joining ") {
        let token = target.split_whitespace().next()?;
        let (world, instance) = token.split_once(':').unwrap_or((token, ""));
        return (is_world_id(world) && is_instance(instance)).then(|| LogEvent::Joining {
            world_id: world.to_string(),
            instance: instance.to_string(),
            at: crate::visited::log_time(stamp),
        });
    }
    if let Some(name) = rest.strip_prefix("Entering Room: ") {
        let name = name.trim();
        return (!name.is_empty()).then(|| LogEvent::EnteringRoom { name: name.to_string() });
    }
    if rest == "OnLeftRoom" {
        return Some(LogEvent::LeftRoom);
    }
    if let Some(who) = rest.strip_prefix("OnPlayerJoined ") {
        let (name, user_id) = name_and_id(who);
        return (!name.is_empty()).then_some(LogEvent::PlayerJoined { name, user_id });
    }
    if let Some(who) = rest.strip_prefix("OnPlayerLeft ") {
        let (name, user_id) = name_and_id(who);
        return (!name.is_empty()).then_some(LogEvent::PlayerLeft { name, user_id });
    }
    if let Some(who) = rest.strip_prefix("Initialized PlayerAPI \"") {
        return who.strip_suffix("\" is local").map(|name| LogEvent::LocalPlayer { name: name.to_string() });
    }
    if let Some(switch) = rest.strip_prefix("Switching ") {
        let (player, avatar) = switch.split_once(" to avatar ")?;
        return Some(LogEvent::AvatarSwitched { player: player.to_string(), avatar: avatar.to_string() });
    }
    None
}

/// Logs are read in blocks of this size, each let go once its lines are read.
const BLOCK: usize = 1 << 20;
/// A line longer than this (a script dumping data) is passed over rather than kept whole.
const MAX_LINE: usize = 1 << 20;

/// Reads the complete lines of `file` from byte `from` up to byte `end`, about `budget` bytes of them at most, a block
/// at a time: `f` gets each line without its end. Returns where the last complete line read ends (a line still being
/// written is read again next time) and whether the reading got to `end` (or the end of the file).
pub(crate) fn read_lines(file: &mut File, from: u64, end: u64, budget: u64, mut f: impl FnMut(&[u8])) -> (u64, bool) {
    if from >= end || file.seek(SeekFrom::Start(from)).is_err() {
        return (from, from >= end);
    }
    let mut block = vec![0u8; BLOCK.min((end - from) as usize)];
    // The line begun in an earlier block, and whether it is one too long, passed over to its end.
    let mut pending: Vec<u8> = Vec::new();
    let mut skipping = false;
    let (mut read, mut done) = (from, from);
    while read < end && done - from < budget {
        let want = ((end - read) as usize).min(block.len());
        let n = match file.read(&mut block[..want]) {
            Ok(0) => return (done, true),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return (done, false),
        };
        let data = &block[..n];
        let mut start = 0;
        while let Some(i) = memchr::memchr(b'\n', &data[start..]) {
            let line_end = start + i;
            if skipping {
                skipping = false;
            } else if pending.is_empty() {
                f(trim_cr(&data[start..line_end]));
            } else {
                pending.extend_from_slice(&data[start..line_end]);
                f(trim_cr(&pending));
                pending.clear();
            }
            start = line_end + 1;
            done = read + start as u64;
        }
        if !skipping {
            pending.extend_from_slice(&data[start..]);
            if pending.len() > MAX_LINE {
                pending = Vec::new();
                skipping = true;
            }
        }
        read += n as u64;
    }
    (done, read >= end)
}

fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// Where the last line of `file` (`len` bytes) that joins a world starts, looked for from the end a block at a time:
/// what the log says about the world being played follows it. `None` when no line does.
fn last_join(file: &mut File, len: u64) -> Option<u64> {
    const MARKER: &[u8] = b"[Behaviour] Joining wrld_";
    // Each block reads on into the next by this much: a line cut between two is whole in one of them.
    const OVERLAP: u64 = 4096;
    let finder = memchr::memmem::FinderRev::new(MARKER);
    let mut buf = vec![0u8; (BLOCK as u64 + OVERLAP).min(len) as usize];
    let mut end = len;
    while end > 0 {
        let start = end.saturating_sub(BLOCK as u64);
        let stop = (end + OVERLAP).min(len);
        let n = (stop - start) as usize;
        file.seek(SeekFrom::Start(start)).ok()?;
        file.read_exact(&mut buf[..n]).ok()?;
        let block = &buf[..n];
        for at in finder.rfind_iter(block) {
            // Its line must begin in this block (or the file); one that begins before is in the block before.
            let line_start = match memchr::memrchr(b'\n', &block[..at]) {
                Some(i) => i + 1,
                None if start == 0 => 0,
                None => continue,
            };
            let line_end = memchr::memchr(b'\n', &block[at..]).map_or(n, |i| at + i);
            if matches!(parse_bytes(trim_cr(&block[line_start..line_end])), Some(LogEvent::Joining { .. })) {
                return Some(start + line_start as u64);
            }
        }
        end = start;
    }
    None
}

/// The log of the VRChat run that began last: `output_log_<date>_<time>.txt` names sort by when the run began. Not the
/// log written to last: a second VRChat, or an old log something touched, would be followed then.
pub fn newest_log(dir: &Path) -> Option<PathBuf> {
    let dated = |name: &str| {
        let b = name.as_bytes();
        name.len() == "output_log_2026-10-20_00-00-00.txt".len()
            && b[11..30].iter().enumerate().all(|(i, c)| if matches!(i, 4 | 7 | 10 | 13 | 16) { !c.is_ascii_digit() } else { c.is_ascii_digit() })
    };
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            (name.starts_with("output_log_") && name.ends_with(".txt")).then(|| ((dated(&name), name), e.path()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, path)| path)
}

/// What one poll reads at most: the rest waits for the next (the watcher thread has other things to do).
const PER_POLL: u64 = 32 << 20;

/// Incremental follower: call [`LogWatcher::poll`] periodically.
pub struct LogWatcher {
    dir: Option<PathBuf>,
    file: Option<PathBuf>,
    /// Where the next line to read starts.
    position: u64,
    /// Everything in the log was read: the state is VRChat's as it is now.
    caught_up: bool,
    /// The last poll's events happened while it followed (not read back from before it began on this log).
    live: bool,
    /// What the last poll read: the log's file name and the bytes from, to.
    last_read: Option<(String, u64, u64)>,
    pub state: WorldState,
}

impl LogWatcher {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self { dir, file: None, position: 0, caught_up: false, live: false, last_read: None, state: WorldState::default() }
    }

    /// Whether everything the log holds was read: until then the state may be an hour old (a world left long ago).
    pub fn caught_up(&self) -> bool {
        self.caught_up
    }

    /// Whether the last poll's events happen now: a join read back from the time before following began (when Orbiters
    /// started while VRChat ran) is not one.
    pub fn live(&self) -> bool {
        self.live
    }

    /// The log the last poll read (its file name) and the bytes it read, from and to.
    pub fn last_read(&self) -> Option<(&str, u64, u64)> {
        self.last_read.as_ref().map(|(name, from, to)| (name.as_str(), *from, *to))
    }

    /// Reads what was appended since the last call; returns the new events (state is updated too). A log first seen
    /// is read from its last join on: what came before is a world left already.
    pub fn poll(&mut self) -> Vec<LogEvent> {
        let mut events = Vec::new();
        let was_caught_up = self.caught_up;
        self.last_read = None;
        self.live = false;
        let Some(dir) = self.dir.clone().or_else(crate::paths::vrchat_log_dir) else { return events };
        let Some(newest) = newest_log(&dir) else { return events };
        let Ok(mut f) = File::open(&newest) else { return events };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        let mut live = was_caught_up;
        if self.file.as_ref() != Some(&newest) {
            // A new VRChat run, the previous world forgotten. One that began while another log was followed happens
            // now, read whole; one already under way when Orbiters started is read from its last join.
            live = self.file.is_some();
            self.position = if live { 0 } else { last_join(&mut f, len).unwrap_or(0) };
            self.file = Some(newest.clone());
            self.state = WorldState { log_file: Some(newest.clone()), ..Default::default() };
            self.caught_up = false;
        } else if len < self.position {
            // Truncated: read again from its start.
            self.position = 0;
            self.state = WorldState { log_file: Some(newest.clone()), ..Default::default() };
        }
        let from = self.position;
        let state = &mut self.state;
        let (to, all) = read_lines(&mut f, from, len, PER_POLL, |line| {
            if let Some(e) = parse_bytes(line) {
                state.apply(&e);
                events.push(e);
            }
        });
        self.position = to;
        self.caught_up = all;
        self.live = live;
        let name = newest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.last_read = Some((name, from, to));
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORLD_A: &str = "wrld_99123966-6b37-4b5b-9f3c-0a1b2c3d4e5f";
    const WORLD_B: &str = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";
    const ME: &str = "usr_8784f94a-27ed-4a3b-9c1d-112233445566";
    const FRIEND: &str = "usr_1339e525-d75d-4832-9104-98dd89287681";

    /// A line as VRChat writes it, at 20:23:20.
    fn line(message: &str) -> String {
        format!("2026.10.18 20:23:20 Debug      -  {message}")
    }

    fn event(message: &str) -> LogEvent {
        parse_line(&line(message)).unwrap_or_else(|| panic!("no event in {message}"))
    }

    /// The N4N0 world's lines (contracts `Docs/world-feedback.md`, Log lines): at the start of a message of VRChat's own
    /// shape only, whole, with their time.
    #[test]
    fn world_lines_are_read_at_the_start_of_a_message_only() {
        let message = r#"[N4N0-Feedback] {"v":1,"t":"hello","session":"5f3a9c01","seq":1}"#;
        match event(message) {
            LogEvent::WorldLine { message: read, at } => {
                assert_eq!(read, message);
                assert!(at.is_some());
            }
            other => panic!("{other:?}"),
        }
        assert!(!matches!(parse_line(&line(&format!("[Behaviour] OnPlayerJoined {message}"))), Some(LogEvent::WorldLine { .. })), "a display name");
        assert_eq!(parse_line(&format!("   {message}")), None, "a further line of a script's message");
        let long = format!("[N4N0-Feedback] {}", "x".repeat(orbiters_contracts::world_feedback::MAX_LINE));
        assert_eq!(parse_line(&line(&long)), None);
    }

    #[test]
    fn the_instance_is_known_by_its_hash_only_while_in_it() {
        let mut world = WorldState::default();
        assert_eq!(world.instance_hash(), None);
        let line = "2026.10.25 10:00:00 Debug      -  [Behaviour] Joining wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b:77505~group(grp_1d2e3f40-1a2b-4c3d-9e8f-0a1b2c3d4e5f)~groupAccessType(public)~region(eu)";
        world.apply(&parse_line(line).expect("a join"));
        assert_eq!(world.instance_hash().as_deref(), Some("952c84c07b16eedff77c95ffef47dd8a24a378c20469b78964b4bad467770fbf"));
        world.apply(&LogEvent::LeftRoom);
        assert_eq!(world.instance_hash(), None, "left: no instance");
    }

    #[test]
    fn parses_lines() {
        let joining = format!("2026.10.18 20:23:20 Debug      -  [Behaviour] Joining {WORLD_A}:77505~group(grp_9)~groupAccessType(public)~region(eu)");
        match parse_line(&joining) {
            Some(LogEvent::Joining { world_id, instance, at }) => {
                assert_eq!((world_id.as_str(), instance.as_str()), (WORLD_A, "77505~group(grp_9)~groupAccessType(public)~region(eu)"));
                assert!(at.is_some());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse_line("2026.10.18 20:23:20 Debug      -  [Behaviour] Entering Room: VRC Volleyball"),
            Some(LogEvent::EnteringRoom { name: "VRC Volleyball".into() })
        );
        assert_eq!(event("[Behaviour] Joining or Creating Room: VRC Volleyball"), LogEvent::JoinedRoom { name: "VRC Volleyball".into() });
        assert_eq!(parse_line("2026.10.18 20:59:59 Log        -  [Behaviour] OnLeftRoom\r"), Some(LogEvent::LeftRoom));
        assert_eq!(parse_line("2026.10.18 20:23:18 Warning    -  Path already exists, skipping: /usercamera/Pose"), None);
    }

    #[test]
    fn only_vrchats_own_lines_count() {
        let join = format!("[Behaviour] Joining {WORLD_B}:1");
        // A world script's message holding the marker, a further line of a message, no time, a name holding it.
        assert_eq!(parse_line(&line(&format!("[MyWorldScript] {join}"))), None);
        assert_eq!(parse_line(&join), None);
        assert_eq!(parse_line(&format!("  {join}")), None);
        assert_eq!(parse_line(&format!("2026.10.18 20:23:20 -  {join}")), None, "no level");
        assert!(matches!(parse_line(&line(&format!("[Behaviour] OnPlayerJoined x {join} (usr_x)"))), Some(LogEvent::PlayerJoined { .. })));
        // Ids that are not VRChat's.
        assert_eq!(parse_line(&line("[Behaviour] Joining wrld_99123966-6b37:1")), None);
        assert_eq!(parse_line(&line("[Behaviour] Joining wrld_../../x:1")), None);
        assert_eq!(parse_line(&line(&format!("[Behaviour] Joining {}:1", WORLD_A.to_uppercase().replace("WRLD", "wrld")))), None);
        assert_eq!(parse_line(&line(&format!("[Behaviour] Joining {WORLD_A}:1\"/../x"))), None, "an instance with a quote");
        assert_eq!(
            event("[Behaviour] OnPlayerJoined Some One (usr_8784f94a-27ed)"),
            LogEvent::PlayerJoined { name: "Some One (usr_8784f94a-27ed)".into(), user_id: String::new() },
            "a short id is part of the name"
        );
    }

    #[test]
    fn parses_people_avatars_and_videos() {
        assert_eq!(
            parse_line(&format!("2026.10.18 20:23:34 Debug      -  [Behaviour] OnPlayerJoined Some One (2) ({ME})")),
            Some(LogEvent::PlayerJoined { name: "Some One (2)".into(), user_id: ME.into() })
        );
        assert_eq!(
            parse_line("2026.10.18 20:34:02 Debug      -  [Behaviour] OnPlayerLeft Old Name"),
            Some(LogEvent::PlayerLeft { name: "Old Name".into(), user_id: String::new() })
        );
        assert_eq!(parse_line(&line("[Behaviour] OnPlayerLeftRoom")), None);
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
        for message in [
            format!("[Behaviour] Joining {WORLD_A}:1"),
            "[Behaviour] Initialized PlayerAPI \"Me\" is local".into(),
            format!("[Behaviour] OnPlayerJoined Me ({ME})"),
            format!("[Behaviour] OnPlayerJoined Friend ({FRIEND})"),
            "[Behaviour] Switching Friend to avatar Fox".into(),
            "[Behaviour] Switching Me to avatar Orbit".into(),
            "[Video Playback] Attempting to resolve URL 'https://youtu.be/x'".into(),
        ] {
            s.apply(&event(&message));
        }
        assert_eq!(s.players.len(), 2);
        assert_eq!(s.others(), 1);
        assert_eq!(s.avatar_name.as_deref(), Some("Orbit"));
        assert_eq!(s.last_video.as_ref().map(|v| v.url.as_str()), Some("https://youtu.be/x"));
        assert_eq!(s.other_avatars(), [("Fox".to_string(), 1)].into(), "names of avatars only, mine left out");
        s.apply(&event(&format!("[Behaviour] OnPlayerLeft Friend ({FRIEND})")));
        assert_eq!(s.others(), 0);
        assert!(s.other_avatars().is_empty());
        s.apply(&event(&format!("[Behaviour] Joining {WORLD_B}:2")));
        assert!(s.players.is_empty() && s.last_video.is_none(), "a new instance starts empty");
        assert_eq!(s.avatar_name.as_deref(), Some("Orbit"), "the avatar stays");
    }

    #[test]
    fn a_name_that_holds_to_avatar_is_not_mine() {
        let mut s = WorldState::default();
        s.apply(&event("[Behaviour] Initialized PlayerAPI \"Me\" is local"));
        s.apply(&event(&format!("[Behaviour] OnPlayerJoined Me ({ME})")));
        s.apply(&event(&format!("[Behaviour] OnPlayerJoined Me to avatar X ({FRIEND})")));
        s.apply(&event("[Behaviour] Switching Me to avatar Orbit"));
        // The other person, whose name starts like the line about me would.
        s.apply(&event("[Behaviour] Switching Me to avatar X to avatar Fox"));
        assert_eq!(s.avatar_name.as_deref(), Some("Orbit"));
        assert_eq!(s.avatars.get("Me to avatar X").map(String::as_str), Some("Fox"));
        // An avatar whose name holds the words, worn by me.
        s.apply(&event("[Behaviour] Switching Me to avatar Fly to avatar heaven"));
        assert_eq!(s.avatar_name.as_deref(), Some("Fly to avatar heaven"));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn follows_a_growing_log() {
        let dir = temp_dir("growing");
        let path = dir.join("output_log_2026-10-20_00-00-00.txt");
        let joining = line(&format!("[Behaviour] Joining {WORLD_A}:1"));
        std::fs::write(&path, format!("x\n{joining}\n\n{}\r\npartial", line("[Behaviour] Joining or Creating Room: A"))).unwrap();
        let mut w = LogWatcher::new(Some(dir.clone()));
        assert_eq!(w.poll().len(), 2);
        assert_eq!(w.state.world_name.as_deref(), Some("A"));
        assert!(w.caught_up() && !w.live(), "read back from before following began");
        assert_eq!(w.last_read().map(|(name, from, _)| (name.to_string(), from)), Some(("output_log_2026-10-20_00-00-00.txt".into(), 2)));
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, " line\n{}", line("[Behaviour] OnLeftRoom")).unwrap();
        drop(f);
        assert_eq!(w.poll(), vec![LogEvent::LeftRoom]);
        assert!(!w.state.in_world && w.live());
        // A VRChat started while this one was followed: its log is read whole, as it happens.
        std::fs::write(dir.join("output_log_2026-10-20_09-00-00.txt"), format!("{}\n", line(&format!("[Behaviour] Joining {WORLD_B}:7")))).unwrap();
        let events = w.poll();
        assert!(matches!(events.as_slice(), [LogEvent::Joining { world_id, .. }] if world_id == WORLD_B), "{events:?}");
        assert!(w.live() && w.state.world_id.as_deref() == Some(WORLD_B));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_long_log_is_read_from_its_last_join() {
        let dir = temp_dir("long");
        let mut text = String::new();
        text.push_str(&format!("{}\n{}\n", line(&format!("[Behaviour] Joining {WORLD_A}:1")), line("[Behaviour] Joining or Creating Room: Old")));
        // Megabytes of a world's chatter, a fake join among it, then the last real join and what followed it.
        let chatter = format!("{}\n", line(&format!("[Script] tick [Behaviour] Joining {WORLD_A}:9")));
        while text.len() < 3 * BLOCK {
            text.push_str(&chatter);
        }
        let last = text.len() as u64;
        text.push_str(&format!("{}\n", line(&format!("[Behaviour] Joining {WORLD_B}:2"))));
        text.push_str(&format!("{}\n", line("[Behaviour] Joining or Creating Room: New")));
        text.push_str(&format!("{}\n", line(&format!("[Behaviour] OnPlayerJoined Friend ({FRIEND})"))));
        let path = dir.join("output_log_2026-10-20_00-00-00.txt");
        std::fs::write(&path, &text).unwrap();
        assert_eq!(last_join(&mut File::open(&path).unwrap(), text.len() as u64), Some(last));
        let mut w = LogWatcher::new(Some(dir.clone()));
        let events = w.poll();
        assert_eq!(events.len(), 3, "only what follows the last join: {events:?}");
        assert_eq!((w.state.world_id.as_deref(), w.state.world_name.as_deref(), w.state.players.len()), (Some(WORLD_B), Some("New"), 1));
        assert!(w.caught_up());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn lines_are_read_in_blocks_and_long_ones_passed_over() {
        let dir = temp_dir("blocks");
        let path = dir.join("log.txt");
        let left = line("[Behaviour] OnLeftRoom");
        // A line longer than any kept, holding what would be a line if it were cut at a block's end.
        let huge = format!("{}{}", "z".repeat(MAX_LINE + BLOCK - 3), line("[Behaviour] OnLeftRoom"));
        std::fs::write(&path, format!("{left}\r\n{huge}\n{left}\npartial")).unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        let mut lines = Vec::new();
        let (to, all) = read_lines(&mut File::open(&path).unwrap(), 0, len, u64::MAX, |l| lines.push(parse_bytes(l)));
        assert_eq!(lines.iter().filter(|e| **e == Some(LogEvent::LeftRoom)).count(), 2, "the first and the last, not the long one");
        assert_eq!((to, all), (len - "partial".len() as u64, true));
        // A budget stops at a line's end, and the next read goes on from there.
        let (first, all) = read_lines(&mut File::open(&path).unwrap(), 0, len, 1, |_| {});
        assert_eq!((first, all), (left.len() as u64 + 2, false));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_run_that_began_last_is_followed() {
        let dir = temp_dir("newest");
        std::fs::write(dir.join("output_log_2026-10-20_10-00-00.txt"), "").unwrap();
        std::fs::write(dir.join("output_log_2026-10-19_23-00-00.txt"), "").unwrap();
        // Written to last, but older.
        std::fs::write(dir.join("output_log_2026-10-19_23-00-00.txt"), "touched").unwrap();
        std::fs::write(dir.join("output_log_notes.txt"), "").unwrap();
        assert_eq!(newest_log(&dir).unwrap().file_name().unwrap(), "output_log_2026-10-20_10-00-00.txt");
        std::fs::remove_dir_all(dir).ok();
    }
}
