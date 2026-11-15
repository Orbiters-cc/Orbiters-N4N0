//! What releases input when the app itself dies. Its running threads go with it, and what it held stays: an OSC
//! `/input/MoveForward` left at 1 "will continue to move you forward forever", VRChat's documentation says, and a key left
//! down is stuck for the whole computer. So while the bridge listens, a small process of its own (`--bridge-guard`) is told on
//! its standard input what is down. Its standard input ends when the app ends, however it ended, and it then brings up what
//! was still down and leaves. It has no window, opens no network port and does nothing else.

use super::game::OscArg;
use super::input::{Input, MouseButton};
use std::io::{BufRead, Write};
use std::net::SocketAddr;
use std::process::{Child, ChildStdin, Command, Stdio};

/// The running guard.
pub struct Guard {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Guard {
    pub fn spawn(exe: &std::path::Path) -> std::io::Result<Self> {
        let mut command = Command::new(exe);
        command.arg("--bridge-guard").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        Ok(Self { child, stdin })
    }

    pub fn tell(&mut self, line: &str) {
        if let Some(stdin) = self.stdin.as_mut() {
            let _ = writeln!(stdin, "{line}");
        }
    }
}

impl Drop for Guard {
    /// The ordinary end: nothing is down by then, and its standard input closing sends it home.
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.wait();
    }
}

/// What the guard has to bring up, from the lines it read: `target <address>`, `down <what>`, `up <what>`.
#[derive(Debug, Default, PartialEq)]
pub struct Reading {
    pub target: Option<SocketAddr>,
    pub down: Vec<String>,
}

impl Reading {
    pub fn line(&mut self, line: &str) {
        if let Some(target) = line.strip_prefix("target ") {
            self.target = target.trim().parse().ok().or(self.target);
        } else if let Some(what) = line.strip_prefix("down ") {
            if !self.down.iter().any(|d| d == what) {
                self.down.push(what.to_string());
            }
        } else if let Some(what) = line.strip_prefix("up ") {
            self.down.retain(|d| d != what);
        }
    }
}

/// What to send for a held thing's line.
#[derive(Debug, PartialEq)]
pub enum Release {
    /// The address, the value at rest, and where it goes when the line says (a session's own input port, `@<address>`),
    /// else the `target` line's.
    Osc(String, OscArg, Option<SocketAddr>),
    Input(Input),
}

pub fn release_of(what: &str) -> Option<Release> {
    let mut words = what.split(' ');
    let at = |word: Option<&str>| -> Option<Option<SocketAddr>> {
        match word {
            None => Some(None),
            Some(w) => w.strip_prefix('@').and_then(|a| a.parse::<SocketAddr>().ok()).filter(|a| a.ip().is_loopback()).map(Some),
        }
    };
    match (words.next()?, words.next()?, words.next()) {
        ("osc-button", address, to) if address.starts_with("/input/") && words.next().is_none() => {
            Some(Release::Osc(address.to_string(), OscArg::Int(0), at(to)?))
        }
        ("osc-axis", address, to) if address.starts_with("/input/") && words.next().is_none() => {
            Some(Release::Osc(address.to_string(), OscArg::Float(0.0), at(to)?))
        }
        ("key", scan, Some(extended)) => Some(Release::Input(Input::Key { scan: scan.parse().ok()?, extended: extended == "true", down: false })),
        ("button", "left", None) => Some(Release::Input(Input::Button { button: MouseButton::Left, down: false })),
        ("button", "right", None) => Some(Release::Input(Input::Button { button: MouseButton::Right, down: false })),
        ("button", "middle", None) => Some(Release::Input(Input::Button { button: MouseButton::Middle, down: false })),
        _ => None,
    }
}

/// Reads until the app is gone, then releases.
pub fn run() -> anyhow::Result<()> {
    let mut reading = Reading::default();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        reading.line(&line);
    }
    release_all(&reading);
    Ok(())
}

fn release_all(reading: &Reading) {
    let socket = std::net::UdpSocket::bind(("127.0.0.1", 0)).ok();
    for release in reading.down.iter().filter_map(|what| release_of(what)) {
        match release {
            Release::Osc(address, value, to) => {
                let args = vec![match value {
                    OscArg::Int(i) => rosc::OscType::Int(i),
                    OscArg::Float(f) => rosc::OscType::Float(f),
                }];
                let packet = rosc::OscPacket::Message(rosc::OscMessage { addr: address, args });
                if let (Some(socket), Some(target), Ok(bytes)) = (&socket, to.or(reading.target), rosc::encoder::encode(&packet)) {
                    let _ = socket.send_to(&bytes, target);
                }
            }
            Release::Input(input) => super::win::release(&[input]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_knows_what_is_still_down_when_its_input_ends() {
        let mut reading = Reading::default();
        for line in [
            "target 127.0.0.1:9000",
            "down osc-button /input/MoveForward",
            "down key 17 false",
            "down key 17 false",
            "up key 17 false",
            "down osc-axis /input/Vertical",
            "up osc-axis /input/Vertical",
            "nonsense",
        ] {
            reading.line(line);
        }
        assert_eq!(reading, Reading { target: Some("127.0.0.1:9000".parse().unwrap()), down: vec!["osc-button /input/MoveForward".to_string()] });
    }

    #[test]
    fn every_kind_of_hold_has_its_release_and_nothing_else_does() {
        use crate::bridge::input::{Hold, MouseButton};
        for hold in [
            Hold::OscButton("/input/MoveForward"),
            Hold::OscAxis("/input/Vertical"),
            Hold::Key { scan: 0x48, extended: true },
            Hold::Button(MouseButton::Right),
        ] {
            let release = release_of(&hold.guard_line()).unwrap();
            match (&hold, release) {
                (Hold::OscButton(a), Release::Osc(address, value, None)) => assert_eq!((*a, value), (address.as_str(), OscArg::Int(0))),
                (Hold::OscAxis(a), Release::Osc(address, value, None)) => assert_eq!((*a, value), (address.as_str(), OscArg::Float(0.0))),
                (Hold::Key { scan, extended }, Release::Input(input)) => assert_eq!(input, Input::Key { scan: *scan, extended: *extended, down: false }),
                (Hold::Button(button), Release::Input(input)) => assert_eq!(input, Input::Button { button: *button, down: false }),
                other => panic!("{other:?}"),
            }
        }
        // A side session's hold names its own input port; only one on this computer.
        assert_eq!(
            release_of("osc-button /input/MoveForward @127.0.0.1:9010"),
            Some(Release::Osc("/input/MoveForward".into(), OscArg::Int(0), Some("127.0.0.1:9010".parse().unwrap())))
        );
        for line in ["osc-button /input/MoveForward @192.168.1.2:9010", "osc-button /input/MoveForward @x", "osc-button /input/MoveForward @127.0.0.1:9010 more", "osc-button /chatbox/input", "osc-axis /avatar/parameters/x", "key abc false", "button side", "nonsense", ""] {
            assert_eq!(release_of(line), None, "{line}");
        }
    }

    #[test]
    fn what_is_down_over_osc_is_released_on_the_wire() {
        let vrchat = std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap();
        vrchat.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut reading = Reading::default();
        reading.line(&format!("target {}", vrchat.local_addr().unwrap()));
        reading.line("down osc-button /input/MoveForward");
        release_all(&reading);
        let mut buffer = [0; 256];
        let n = vrchat.recv(&mut buffer).unwrap();
        let (_, packet) = rosc::decoder::decode_udp(&buffer[..n]).unwrap();
        let rosc::OscPacket::Message(message) = packet else { panic!() };
        assert_eq!(message.addr, "/input/MoveForward");
        assert_eq!(message.args, [rosc::OscType::Int(0)]);
    }

    #[test]
    fn two_sessions_walking_are_each_released_at_their_own_port() {
        let (main, side) = (std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap(), std::net::UdpSocket::bind(("127.0.0.1", 0)).unwrap());
        for s in [&main, &side] {
            s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        }
        let mut reading = Reading::default();
        reading.line(&format!("target {}", main.local_addr().unwrap()));
        reading.line("down osc-button /input/MoveForward");
        reading.line(&format!("down osc-button /input/MoveForward @{}", side.local_addr().unwrap()));
        assert_eq!(reading.down.len(), 2, "the same input in two sessions is two holds");
        release_all(&reading);
        for socket in [&main, &side] {
            let mut buffer = [0; 256];
            let n = socket.recv(&mut buffer).unwrap();
            let (_, rosc::OscPacket::Message(message)) = rosc::decoder::decode_udp(&buffer[..n]).unwrap() else { panic!() };
            assert_eq!((message.addr.as_str(), message.args.as_slice()), ("/input/MoveForward", &[rosc::OscType::Int(0)][..]));
        }
    }
}
