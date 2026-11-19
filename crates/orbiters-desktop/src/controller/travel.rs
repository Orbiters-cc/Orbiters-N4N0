//! Taking the person to an instance directly (L-go-direct): its launch link handed to the person's own running VRChat
//! through VRChat's link pipe (`\\.\pipe\VRChatURLLaunchPipe`, what VRChat's `launch.exe` writes a `vrchat://` link to),
//! or VRChat started right in it when none runs. An invite to oneself is the way only where that is impossible.
//!
//! Only the first VRChat started owns the pipe (docs/vrchat-bridge.md, measured 2026-11-15): with the bridge's side
//! sessions running, a link written blindly can land in an AI's VRChat. So the pipe's server process is asked first
//! (`GetNamedPipeServerProcessId`) and the link is written only when it is the person's own VRChat: never one the bridge
//! started (its main session, a side session, or any VRChat whose `--osc` names other ports than VRChat's own 9000).

use serde::{Deserialize, Serialize};

/// The launch link of `world`'s instance `instance`, as VRChat's website and `launch.exe` write it.
pub fn link(world: &str, instance: &str) -> String {
    format!("vrchat://launch?ref=orbiters.cc&id={world}:{instance}")
}

/// How Go there took the person to the instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Went {
    /// The link went to the person's own VRChat, which takes them there.
    Direct,
    /// No VRChat ran: VRChat starts right in the instance.
    Started,
    /// An invite to oneself was sent instead, and why.
    Invited(Detour),
}

/// Why an invite went out instead of a direct trip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Detour {
    /// The person asked for the invite.
    Asked,
    /// VRChat's link pipe belongs to another VRChat of this computer (one of Orbiters' AI bridge sessions, started first).
    OtherVrchat,
    /// The person's VRChat did not take the link.
    LinkRefused,
    /// VRChat runs on another device of the network: links of this computer do not reach it.
    Remote,
}

/// What the computer is asked, real or a test's.
pub trait Travel: Send + Sync {
    /// The VRChats running on this computer, by process id.
    fn running(&self) -> Vec<u32>;
    /// Whether this VRChat's command line names OSC ports of its own (`--osc=` other than VRChat's 9000): a bridge
    /// side session, even one whose process the bridge has not recorded yet.
    fn own_osc_ports(&self, pid: u32) -> bool;
    /// Connects to VRChat's link pipe: the connection, and the process serving it when Windows says.
    fn connect(&self) -> std::io::Result<(Box<dyn LinkPipe>, Option<u32>)>;
    /// Starts VRChat (its own launcher, the default profile named so that no running VRChat is handed the link) in the
    /// instance of `link`.
    fn start(&self, link: &str) -> Result<(), String>;
}

pub trait LinkPipe {
    fn send(self: Box<Self>, link: &str) -> std::io::Result<()>;
}

/// The VRChats Orbiters' bridge runs, by process id: its main session and its side sessions.
pub struct Bridge {
    pub main: Option<u32>,
    pub sides: Vec<u32>,
}

/// What a press of Go there does now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Hand the link to the person's VRChat, if the pipe is theirs.
    Pipe,
    /// No VRChat: start it in the instance.
    Start,
    Invite(Detour),
    /// Only the bridge's own VRChat with the person's account runs: nothing of the person's to move.
    BridgeBusy,
}

pub fn route(remote: bool, running: &[u32], bridge: &Bridge, mut side_by_args: impl FnMut(u32) -> bool) -> Route {
    if remote {
        return Route::Invite(Detour::Remote);
    }
    let theirs: Vec<u32> = running.iter().copied().filter(|p| Some(*p) != bridge.main && !bridge.sides.contains(p) && !side_by_args(*p)).collect();
    if !theirs.is_empty() {
        Route::Pipe
    } else if bridge.main.is_some_and(|m| running.contains(&m)) {
        Route::BridgeBusy
    } else {
        // None, or only side sessions (their own profiles): the person's starts beside them in the default profile.
        Route::Start
    }
}

/// Takes the person to `world`'s `instance`: the way taken, or why there is none (words for the page).
pub fn go(travel: &dyn Travel, remote: bool, bridge: &Bridge, world: &str, instance: &str) -> Result<Went, GoFailure> {
    // Ids come from the network: only VRChat's own grammar reaches the pipe, the link or `launch.exe`.
    if !orbiters_core::vrchat_launch::is_destination(world, instance) {
        return Err(GoFailure::BadDestination);
    }
    let running = travel.running();
    let link = link(world, instance);
    match route(remote, &running, bridge, |p| travel.own_osc_ports(p)) {
        Route::Invite(why) => Err(GoFailure::Invite(why)),
        Route::BridgeBusy => Err(GoFailure::BridgeBusy),
        Route::Start => travel.start(&link).map(|()| Went::Started).map_err(GoFailure::Start),
        Route::Pipe => {
            let Ok((pipe, server)) = travel.connect() else { return Err(GoFailure::Invite(Detour::LinkRefused)) };
            // The person's: running, and none of the bridge's. Unknown or another: nothing is written.
            let theirs = server.is_some_and(|s| running.contains(&s) && Some(s) != bridge.main && !bridge.sides.contains(&s) && !travel.own_osc_ports(s));
            if !theirs {
                return Err(GoFailure::Invite(Detour::OtherVrchat));
            }
            pipe.send(&link).map(|()| Went::Direct).map_err(|_| GoFailure::Invite(Detour::LinkRefused))
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum GoFailure {
    /// No direct way: an invite is the way, for this reason.
    Invite(Detour),
    BridgeBusy,
    /// VRChat could not be started, in the launcher's words.
    Start(String),
    /// The world or instance id is not one of VRChat's: nothing was done.
    BadDestination,
}

/// This computer.
pub struct Real;

const PIPE: &str = r"\\.\pipe\VRChatURLLaunchPipe";

impl Travel for Real {
    fn running(&self) -> Vec<u32> {
        // The system's own list: no process is opened (sysinfo opened each with memory-read rights).
        super::bridge_side::vrchat_pids()
    }

    fn own_osc_ports(&self, pid: u32) -> bool {
        // Without opening the game (verify-n4n0 5): its command line as its own log lists it, the log made in the minute
        // after the process started. Anything uncertain counts as maybe the bridge's: the person is then offered an invite,
        // and nothing is written into a VRChat that may be an agent's.
        let crate::bridge::game::Identity::Running(start) = super::bridge_side::vrchat_identity(pid) else { return true };
        if start == 0 {
            return true;
        }
        let Some(dir) = orbiters_core::paths::vrchat_log_dir() else { return true };
        let logs: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with("output_log_"))
                    .filter(|e| {
                        e.metadata()
                            .ok()
                            .and_then(|m| m.created().ok())
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .is_some_and(|t| t.as_secs() + 5 >= start && t.as_secs() <= start + 60)
                    })
                    .map(|e| e.path())
                    .collect()
            })
            .unwrap_or_default();
        let [log] = &logs[..] else { return true };
        match crate::bridge::sessions::complete_args(&crate::bridge::sessions::read_head(log, 64 * 1024)) {
            Some(args) => args.iter().any(|a| side_osc(a)),
            None => true,
        }
    }

    fn connect(&self) -> std::io::Result<(Box<dyn LinkPipe>, Option<u32>)> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(PIPE)?;
        let server = pipe_server(&file);
        Ok((Box::new(Pipe(file)), server))
    }

    fn start(&self, link: &str) -> Result<(), String> {
        crate::bridge::win::start_vrchat_beside(&[link.to_string(), "--profile=0".to_string()]).map_err(|f| f.message)
    }
}

/// A `--osc=<in>:<ip>:<out>` argument whose port in is not VRChat's own 9000: the bridge's side sessions have these.
fn side_osc(arg: &str) -> bool {
    arg.strip_prefix("--osc=").and_then(|rest| rest.split(':').next()).is_some_and(|port| port != "9000")
}

struct Pipe(std::fs::File);

impl LinkPipe for Pipe {
    fn send(self: Box<Self>, link: &str) -> std::io::Result<()> {
        use std::io::{Read, Write};
        let mut file = self.0;
        file.write_all(link.as_bytes())?;
        file.flush()?;
        // As launch.exe: VRChat may answer; the connection stays open a moment for it to read, then goes.
        if let Ok(mut reader) = file.try_clone() {
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = std::thread::Builder::new().name("vrchat-link-pipe".into()).spawn(move || {
                let mut answer = [0u8; 64];
                let _ = reader.read(&mut answer);
                let _ = tx.send(());
            });
            let _ = rx.recv_timeout(std::time::Duration::from_secs(3));
        }
        Ok(())
    }
}

#[cfg(windows)]
fn pipe_server(file: &std::fs::File) -> Option<u32> {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetNamedPipeServerProcessId(pipe: *mut core::ffi::c_void, pid: *mut u32) -> i32;
    }
    let mut pid = 0u32;
    // SAFETY: the handle is the open file's for the call; Windows writes one u32.
    let ok = unsafe { GetNamedPipeServerProcessId(file.as_raw_handle(), &mut pid) };
    (ok != 0 && pid != 0).then_some(pid)
}

#[cfg(not(windows))]
fn pipe_server(_: &std::fs::File) -> Option<u32> {
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A computer with these VRChats, a pipe served by `server`, and what was written or started.
    #[derive(Default)]
    pub(crate) struct Fake {
        pub running: Vec<u32>,
        pub side_args: Vec<u32>,
        pub server: Option<u32>,
        pub pipe_missing: bool,
        pub pipe_breaks: bool,
        pub start_fails: bool,
        pub written: Arc<Mutex<Vec<String>>>,
        pub started: Mutex<Vec<String>>,
        pub connects: Mutex<u32>,
    }
    struct FakePipe(Arc<Mutex<Vec<String>>>, bool);
    impl LinkPipe for FakePipe {
        fn send(self: Box<Self>, link: &str) -> std::io::Result<()> {
            if self.1 {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.0.lock().unwrap().push(link.into());
            Ok(())
        }
    }
    impl Travel for Fake {
        fn running(&self) -> Vec<u32> {
            self.running.clone()
        }
        fn own_osc_ports(&self, pid: u32) -> bool {
            self.side_args.contains(&pid)
        }
        fn connect(&self) -> std::io::Result<(Box<dyn LinkPipe>, Option<u32>)> {
            *self.connects.lock().unwrap() += 1;
            if self.pipe_missing {
                return Err(std::io::ErrorKind::NotFound.into());
            }
            Ok((Box::new(FakePipe(self.written.clone(), self.pipe_breaks)), self.server))
        }
        fn start(&self, link: &str) -> Result<(), String> {
            if self.start_fails {
                return Err("VRChat's launcher was not found on this computer.".into()); // i18n: not shown (a test double's words)
            }
            self.started.lock().unwrap().push(link.into());
            Ok(())
        }
    }

    const W: &str = "wrld_266523e8-9161-40da-acd0-6bd82e075833";
    const I: &str = "46657~hidden(usr_1)~region(eu)";
    fn none() -> Bridge {
        Bridge { main: None, sides: vec![] }
    }

    #[test]
    fn the_persons_own_vrchat_gets_the_link_and_nothing_else_happens() {
        let pc = Fake { running: vec![10], server: Some(10), ..Default::default() };
        assert_eq!(go(&pc, false, &none(), W, I), Ok(Went::Direct));
        assert_eq!(*pc.written.lock().unwrap(), [format!("vrchat://launch?ref=orbiters.cc&id={W}:{I}")]);
        assert!(pc.started.lock().unwrap().is_empty());
    }

    #[test]
    fn a_bridge_session_owning_the_pipe_never_gets_the_link() {
        // Its side session started first (2026-11-15): the pipe is its, the person's VRChat is 10.
        for (bridge, side_args) in [(Bridge { main: None, sides: vec![20] }, vec![]), (none(), vec![20])] {
            let pc = Fake { running: vec![20, 10], server: Some(20), side_args, ..Default::default() };
            assert_eq!(go(&pc, false, &bridge, W, I), Err(GoFailure::Invite(Detour::OtherVrchat)));
            assert!(pc.written.lock().unwrap().is_empty(), "nothing written to an AI's VRChat");
        }
        // A server Windows does not name is not trusted either.
        let pc = Fake { running: vec![10], server: None, ..Default::default() };
        assert_eq!(go(&pc, false, &none(), W, I), Err(GoFailure::Invite(Detour::OtherVrchat)));
        assert!(pc.written.lock().unwrap().is_empty());
    }

    #[test]
    fn no_vrchat_starts_one_in_the_instance_and_only_the_bridges_main_refuses() {
        let pc = Fake::default();
        assert_eq!(go(&pc, false, &none(), W, I), Ok(Went::Started));
        assert_eq!(pc.started.lock().unwrap().len(), 1);
        assert_eq!(*pc.connects.lock().unwrap(), 0, "no pipe touched");
        // Only side sessions run: the person's starts beside them (default profile named).
        let pc = Fake { running: vec![20], server: Some(20), ..Default::default() };
        assert_eq!(go(&pc, false, &Bridge { main: None, sides: vec![20] }, W, I), Ok(Went::Started));
        assert!(pc.written.lock().unwrap().is_empty());
        let pc = Fake { running: vec![30], server: Some(30), ..Default::default() };
        assert_eq!(go(&pc, false, &Bridge { main: Some(30), sides: vec![] }, W, I), Err(GoFailure::BridgeBusy));
        assert!(pc.written.lock().unwrap().is_empty() && pc.started.lock().unwrap().is_empty());
        let pc = Fake { start_fails: true, ..Default::default() };
        assert!(matches!(go(&pc, false, &none(), W, I), Err(GoFailure::Start(_))));
    }

    #[test]
    fn remote_vrchat_and_a_refused_link_fall_back_to_the_invite() {
        let pc = Fake { running: vec![10], server: Some(10), ..Default::default() };
        assert_eq!(go(&pc, true, &none(), W, I), Err(GoFailure::Invite(Detour::Remote)));
        assert!(pc.written.lock().unwrap().is_empty());
        let pc = Fake { running: vec![10], pipe_missing: true, ..Default::default() };
        assert_eq!(go(&pc, false, &none(), W, I), Err(GoFailure::Invite(Detour::LinkRefused)));
        let pc = Fake { running: vec![10], server: Some(10), pipe_breaks: true, ..Default::default() };
        assert_eq!(go(&pc, false, &none(), W, I), Err(GoFailure::Invite(Detour::LinkRefused)));
    }

    #[test]
    fn an_id_outside_vrchats_grammar_never_reaches_the_pipe_or_launch_exe() {
        for (world, instance) in [(W, "1&shortName=x"), (W, "1 --profile=3"), (W, "1~region(\"eu\")"), ("wrld_x", "1")] {
            let pc = Fake { running: vec![10], server: Some(10), ..Default::default() };
            assert_eq!(go(&pc, false, &none(), world, instance), Err(GoFailure::BadDestination));
            assert!(pc.written.lock().unwrap().is_empty());
            let pc = Fake::default();
            assert_eq!(go(&pc, false, &none(), world, instance), Err(GoFailure::BadDestination), "nothing started");
            assert!(pc.started.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn side_session_ports_are_told_from_vrchats_own() {
        assert!(side_osc("--osc=9010:127.0.0.1:9011"));
        assert!(!side_osc("--osc=9000:127.0.0.1:9001"));
        assert!(!side_osc("--profile=1"));
    }
}
