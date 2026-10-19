//! Follows VRChat's `output_log_*.txt` (the same source VRCX uses) for the current world and instance.
//! Reading a local log needs no VRChat account access.

use serde::Serialize;
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
}

/// What the log says about where the player is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct WorldState {
    pub world_id: Option<String>,
    pub world_name: Option<String>,
    pub instance: Option<String>,
    pub in_world: bool,
    /// The log file being followed.
    pub log_file: Option<PathBuf>,
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
            }
            LogEvent::EnteringRoom { name } => {
                self.world_name = Some(name.clone());
                self.in_world = true;
            }
            LogEvent::LeftRoom => self.in_world = false,
        }
    }
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
