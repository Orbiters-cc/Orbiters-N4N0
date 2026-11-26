//! The VRChat Home started on this Mac, through Obelisk's engine, for what reads VRChat's camera: which session's frames are read and by whom, the world a capture is of, and
//! where VRChat answers on this Mac. Never another's: not one the VRChat bridge started, and not one on the network (`LauncherState::desktop_game` is the one word of it).
//!
//! The world its log says is for captures alone (a capture's manifest, a new session in a new world, who else is there for a picture of the world). What the rest of the app
//! does with a world on a PC (the visited worlds, what is said to Orbiters, the notices) stays as it is on a Mac.

use orbiters_core::locks::*;
use super::Shared;
use orbiters_core::osc::OscService;
use orbiters_core::protocol::WorkerCommand;
use orbiters_core::vrchat_log::{LogEvent, LogWatcher, WorldState};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A log made up to this long before its game's process is of it (dates are coarse).
const BEFORE: Duration = Duration::from_secs(5);
/// The head of a log where VRChat says where it answers.
const HEAD: u64 = 4 << 20;

/// What the controller keeps of it for others.
#[derive(Default)]
pub(super) struct Runtime {
    /// The engine's host folder of the game that runs.
    host: Mutex<Option<PathBuf>>,
    world: Mutex<WorldState>,
}

/// The watchers' own: the log followed.
#[derive(Default)]
pub(super) struct OwnGame {
    game: Option<u32>,
    log: Option<LogWatcher>,
    /// Where its OSCQuery service is, as its log says.
    query: Option<SocketAddr>,
}

impl OwnGame {
    /// A turn of the watchers (each second): what happened in its world since the last, and whether it happened now.
    pub(super) fn turn(&mut self, ctl: &Shared, library: &Path, tick: u64) -> (Vec<LogEvent>, bool) {
        let game = ctl.own_pid();
        if game != self.game {
            self.game = game;
            self.query = None;
            self.log = game.map(|pid| follow(library, born(library, pid)));
            *ctl.mac_game.world.locked() = WorldState::default();
        }
        let Some(log) = self.log.as_mut() else { return (Vec::new(), false) };
        let events = log.poll();
        if log.caught_up() {
            *ctl.mac_game.world.locked() = log.state.clone();
        }
        if tick.is_multiple_of(2) {
            let file = log.state.log_file.clone();
            self.find_query(ctl, file.as_deref());
        }
        (events, self.log.as_ref().is_some_and(LogWatcher::live))
    }

    /// VRChat's OSCQuery service, where discovery on the network does not show it (what the engine's guest multicasts may not reach this Mac): its log names the port, and it
    /// counts once it answers there as VRChat.
    fn find_query(&mut self, ctl: &Shared, log: Option<&Path>) {
        if ctl.with_osc(|o| o.vrchat_query().is_some()).unwrap_or(true) {
            return;
        }
        if self.query.is_none() {
            let Some(log) = log else { return };
            self.query = head_of(log).and_then(|head| crate::bridge::sessions::query_port(&head)).map(|port| SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
        }
        let Some(query) = self.query else { return };
        if orbiters_core::osc::answers_as_vrchat(query) {
            ctl.with_osc(|o: &OscService| o.set_vrchat_query(Some(query)));
        }
    }
}

/// A log's first whole lines.
fn head_of(log: &Path) -> Option<String> {
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(log).ok()?.take(HEAD).read_to_end(&mut head).ok()?;
    head.truncate(head.iter().rposition(|b| *b == b'\n').map_or(0, |end| end + 1));
    Some(String::from_utf8_lossy(&head).into_owned())
}

/// When the game's process began, as its session's record says.
fn born(library: &Path, game: u32) -> Option<SystemTime> {
    let (seconds, micros) = orbiters_core::obelisk::session_games_born(library)?.into_iter().find(|(pid, _)| *pid == game)?.1?;
    Some(UNIX_EPOCH + Duration::from_secs(seconds) + Duration::from_micros(u64::from(micros)))
}

/// The newest log of VRChat's under the engine, from the game's start on.
fn follow(library: &Path, born: Option<SystemTime>) -> LogWatcher {
    let mut log = LogWatcher::new(Some(orbiters_core::obelisk::vrchat_log_dir(library)));
    log.set_skip(Arc::new(move |path: &Path| {
        let Some(born) = born else { return false };
        std::fs::metadata(path).ok().and_then(|m| m.created().ok()).is_some_and(|made| made + BEFORE < born)
    }));
    log
}

impl super::Controller {
    /// The person's own VRChat runs on this Mac, on an engine that gives its camera: with one from before nothing here is done, and everything is as it was.
    pub(crate) fn own_game(&self) -> bool {
        self.own_pid().is_some()
    }

    fn own_pid(&self) -> Option<u32> {
        let snapshot = self.snapshot.locked();
        snapshot.launcher.desktop_game.filter(|_| snapshot.launcher.camera)
    }

    /// The world the game of this Mac is in.
    pub(super) fn mac_world(&self) -> WorldState {
        self.mac_game.world.locked().clone()
    }

    /// Whose frames are read from now on, here and in the capture worker: the running game's, told at each change.
    pub(super) fn follow_camera_host(self: &Shared) {
        let game = self.own_pid();
        let host = game.map(|pid| orbiters_core::obelisk::host_folder(&orbiters_core::obelisk::user_tmp(), pid)).filter(|h| h.is_dir());
        {
            let mut held = self.mac_game.host.locked();
            if *held == host {
                return;
            }
            held.clone_from(&host);
        }
        orbiters_spout::set_session(host.as_deref().map(orbiters_spout::Session::of));
        let _ = self.send_if_running(&WorkerCommand::Host { folder: host });
    }

    /// What a worker that just started is told first.
    pub(super) fn worker_host(&self) -> WorkerCommand {
        WorkerCommand::Host { folder: self.mac_game.host.locked().clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// VRChat's OSCQuery service found by its log where the network shows none, only once it answers as VRChat; then what its camera and the avatar say is read from there, and
    /// what it sends arrives on the loopback service, as beside a PC's VRChat.
    #[test]
    fn vrchat_is_found_by_its_log_and_heard_on_the_loopback() {
        use std::io::{Read, Write};
        let mut settings = orbiters_core::settings::Settings::default();
        settings.osc.enabled = true;
        // (a game's own input port: nothing is ever sent to the real one's)
        let game = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        game.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        settings.osc.vrchat_port = game.local_addr().unwrap().port();
        let ctl = crate::controller::Controller::new(settings);
        ctl.start_osc_with(false);
        let port = game.local_addr().unwrap().port();
        let http = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let query = http.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in http.incoming().take(40).flatten() {
                let mut stream = stream;
                let mut asked = [0u8; 1024];
                let n = stream.read(&mut asked).unwrap_or(0);
                let asked = String::from_utf8_lossy(&asked[..n]).into_owned();
                let body = if asked.contains("HOST_INFO") {
                    format!(r#"{{"NAME":"VRChat-Client-ABCDEF","OSC_IP":"127.0.0.1","OSC_PORT":{port},"OSC_TRANSPORT":"UDP"}}"#)
                } else {
                    r#"{"FULL_PATH":"/","CONTENTS":{"usercamera":{"FULL_PATH":"/usercamera","CONTENTS":{"Mode":{"FULL_PATH":"/usercamera/Mode","TYPE":"i","ACCESS":3,"VALUE":[2]}}}}}"#.to_string()
                };
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        let file = std::env::temp_dir().join(format!("orbiters-mac-log-{}.txt", std::process::id()));
        std::fs::write(&file, "2026.11.25 17:00:00 Debug      -  Advertising Service VRChat-Client-ABCDEF of type OSC on 9000\n").unwrap();
        let mut own = OwnGame::default();
        own.find_query(&ctl, Some(&file));
        assert_eq!(ctl.with_osc(|o| o.vrchat_query()).flatten(), None, "its log does not say yet");
        std::fs::write(&file, format!("2026.11.25 17:00:00 Debug      -  Advertising Service VRChat-Client-ABCDEF of type OSCQuery on {}\n", query.port())).unwrap();
        own.find_query(&ctl, Some(&file));
        assert_eq!(ctl.with_osc(|o| o.vrchat_query()).flatten(), Some(query));
        assert!(ctl.camera_values().iter().any(|(p, v)| p == "/usercamera/Mode" && *v == orbiters_core::osc::OscValue::Int(2)), "{:?}", ctl.camera_values());
        // The game speaks, and is spoken to.
        let to = ctl.with_osc(|o| o.local_osc_addr()).unwrap();
        let packet = rosc::encoder::encode(&rosc::OscPacket::Message(rosc::OscMessage { addr: "/usercamera/Mode".into(), args: vec![rosc::OscType::Int(2)] })).unwrap();
        game.send_to(&packet, to).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while ctl.view().camera_open_said != Some(true) && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(ctl.view().camera_open_said, Some(true));
        ctl.with_osc(|o| o.send("/usercamera/Streaming", vec![rosc::OscType::Bool(true)])).unwrap().unwrap();
        let mut heard = [0u8; 256];
        loop {
            let n = game.recv(&mut heard).unwrap();
            let Ok((_, rosc::OscPacket::Message(m))) = rosc::decoder::decode_udp(&heard[..n]) else { continue };
            if m.addr == "/usercamera/Streaming" {
                assert_eq!(m.args, [rosc::OscType::Bool(true)]);
                break;
            }
        }
        let _ = std::fs::remove_file(&file);
    }

    /// With an engine from before, nothing of this is done: the network's VRChat is the one talked with, as it was.
    #[test]
    fn an_engine_from_before_changes_nothing() {
        let ctl = crate::controller::Controller::new(orbiters_core::settings::Settings::default());
        assert!(!ctl.own_game());
        ctl.update(|s| s.launcher.desktop_game = Some(std::process::id()));
        assert!(!ctl.own_game(), "0.1.14");
        ctl.update(|s| s.launcher.camera = true);
        assert!(ctl.own_game());
        ctl.update(|s| s.launcher.desktop_game = None);
        assert!(!ctl.own_game(), "it ended");
    }

    /// The log followed is the running game's: never one left by an earlier run, whatever its date of writing.
    #[test]
    fn only_a_log_of_the_running_game_is_followed() {
        let library = std::env::temp_dir().join(format!("orbiters-mac-game-{}", std::process::id()));
        let dir = orbiters_core::obelisk::vrchat_log_dir(&library);
        std::fs::create_dir_all(&dir).unwrap();
        let line = |world: &str| format!("2026.11.25 17:00:00 Debug      -  [Behaviour] Joining wrld_{world}~private\n2026.11.25 17:00:00 Debug      -  [Behaviour] Entering Room: {world}\n2026.11.25 17:00:01 Debug      -  [Behaviour] Joining or Creating Room: {world}\n");
        std::fs::write(dir.join("output_log_2026-11-25_10-00-00.txt"), line("Old")).unwrap();
        let mut log = follow(&library, Some(SystemTime::now() + Duration::from_secs(60)));
        log.poll();
        assert_eq!(log.state.log_file, None, "made before the game began");
        let mut log = follow(&library, Some(SystemTime::now() - Duration::from_secs(60)));
        log.poll();
        assert!(log.state.log_file.is_some(), "made after it");
        assert_eq!(log.state.world_name.as_deref(), Some("Old"));
        let mut log = follow(&library, None);
        log.poll();
        assert!(log.state.log_file.is_some(), "a record that says no birth: the newest");
        let _ = std::fs::remove_dir_all(&library);
    }
}
