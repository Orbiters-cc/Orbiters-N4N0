//! The link between the tray service (controller, worker, OSC, tray) and the processes that show it: the window
//! and the VR overlay. JSON lines over a loopback TCP socket; a client proves it was started by the service with
//! the token the service passed on its command line. The service never loads a GPU driver, so its footprint while
//! it waits in the tray stays small; a window exists only while it is shown.

use crate::controller::Notice;
use crate::state::Snapshot;
use orbiters_contracts::performance::calibration::Calibration;
use orbiters_core::settings::Settings;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Which kind of process is talking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Window,
    Overlay,
}

/// Service → client.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToClient {
    Snapshot {
        snapshot: Box<Snapshot>,
    },
    Notice {
        notice: Notice,
    },
    /// The session library changed (reload lists).
    Library {
        version: u64,
    },
    WorkerLog {
        lines: Vec<String>,
    },
    /// Come to the front.
    Show,
    /// Close (the app quits, or another window replaced this one).
    Close,
}

/// Client → service.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToService {
    Hello {
        token: String,
        role: Role,
        pid: u32,
    },
    StartCapture,
    StopCapture,
    Pause {
        paused: bool,
    },
    TestPattern,
    SaveSettings {
        settings: Box<Settings>,
    },
    ImportRaw {
        dir: PathBuf,
    },
    Trash {
        dir: PathBuf,
    },
    Reconstruct {
        dir: PathBuf,
    },
    StopReconstruction,
    SendToMixer {
        dir: PathBuf,
        title: String,
    },
    WorkerLog,
    /// A T-pose calibration taken in the overlay (`time_ns` is a shared-clock reading).
    Calibrated {
        calibration: Box<Calibration>,
    },
    Quit,
}

pub fn write_line<T: Serialize>(w: &mut impl Write, msg: &T) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(msg).map_err(std::io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// Where a client finds the service, as passed on its command line (`--service <port> <token>`).
#[derive(Clone, Debug)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
}

impl Endpoint {
    pub fn args(&self) -> [String; 3] {
        ["--service".into(), self.port.to_string(), self.token.clone()]
    }

    pub fn from_args(args: &[String]) -> Option<Self> {
        let i = args.iter().position(|a| a == "--service")?;
        Some(Self { port: args.get(i + 1)?.parse().ok()?, token: args.get(i + 2)?.clone() })
    }
}

/// What a client keeps of the service: the latest snapshot, queued notices and requests for the window.
pub struct Client {
    writer: Mutex<TcpStream>,
    snapshot: Mutex<Arc<Snapshot>>,
    notices: Mutex<Vec<Notice>>,
    library: AtomicU64,
    worker_log: Mutex<Option<Vec<String>>>,
    show: AtomicBool,
    close: AtomicBool,
    connected: AtomicBool,
    repaint: Mutex<Option<egui::Context>>,
}

impl Client {
    /// Connects, says hello, and waits for the first snapshot (so the first frame has real data).
    pub fn connect(endpoint: &Endpoint, role: Role) -> anyhow::Result<Arc<Self>> {
        let stream = TcpStream::connect(("127.0.0.1", endpoint.port))?;
        stream.set_nodelay(true)?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut writer = stream;
        write_line(&mut writer, &ToService::Hello { token: endpoint.token.clone(), role, pid: std::process::id() })?;
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let first = match serde_json::from_str::<ToClient>(&line)? {
            ToClient::Snapshot { snapshot } => Arc::new(*snapshot),
            other => anyhow::bail!("unexpected first message from the service: {other:?}"),
        };
        let client = Arc::new(Self {
            writer: Mutex::new(writer),
            snapshot: Mutex::new(first),
            notices: Mutex::new(Vec::new()),
            library: AtomicU64::new(0),
            worker_log: Mutex::new(None),
            show: AtomicBool::new(false),
            close: AtomicBool::new(false),
            connected: AtomicBool::new(true),
            repaint: Mutex::new(None),
        });
        let c = client.clone();
        std::thread::Builder::new().name("service-link".into()).spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Ok(msg) = serde_json::from_str::<ToClient>(&line) else { continue };
                match msg {
                    ToClient::Snapshot { snapshot } => *c.snapshot.lock().unwrap() = Arc::new(*snapshot),
                    ToClient::Notice { notice } => c.notices.lock().unwrap().push(notice),
                    ToClient::Library { version } => c.library.store(version, Ordering::Relaxed),
                    ToClient::WorkerLog { lines } => *c.worker_log.lock().unwrap() = Some(lines),
                    ToClient::Show => c.show.store(true, Ordering::Relaxed),
                    ToClient::Close => c.close.store(true, Ordering::Relaxed),
                }
                c.repaint();
            }
            // The service is gone: close too.
            c.connected.store(false, Ordering::Relaxed);
            c.close.store(true, Ordering::Relaxed);
            c.repaint();
        })?;
        Ok(client)
    }

    fn repaint(&self) {
        if let Some(ctx) = self.repaint.lock().unwrap().as_ref() {
            ctx.request_repaint();
        }
    }

    pub fn attach(&self, ctx: &egui::Context) {
        *self.repaint.lock().unwrap() = Some(ctx.clone());
    }

    pub fn send(&self, msg: &ToService) {
        if self.connected.load(Ordering::Relaxed) {
            let _ = write_line(&mut *self.writer.lock().unwrap(), msg);
        }
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.lock().unwrap().clone()
    }

    /// Shows a change before the service confirms it (the service's next snapshot replaces it).
    pub fn update_local(&self, f: impl FnOnce(&mut Snapshot)) {
        let mut guard = self.snapshot.lock().unwrap();
        let mut s = (**guard).clone();
        f(&mut s);
        *guard = Arc::new(s);
    }

    pub fn take_notices(&self) -> Vec<Notice> {
        std::mem::take(&mut *self.notices.lock().unwrap())
    }

    pub fn library_version(&self) -> u64 {
        self.library.load(Ordering::Relaxed)
    }

    pub fn worker_log(&self) -> Option<Vec<String>> {
        self.worker_log.lock().unwrap().clone()
    }

    pub fn take_show(&self) -> bool {
        self.show.swap(false, Ordering::Relaxed)
    }

    pub fn should_close(&self) -> bool {
        self.close.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::NoticeKind;
    use std::net::TcpListener;

    #[test]
    fn link_carries_snapshots_notices_and_commands() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = Endpoint { port: listener.local_addr().unwrap().port(), token: "t0ken".into() };
        let mut snapshot = Snapshot::new(Settings::default());
        snapshot.capture.trail = vec![[1.0, 2.0, -0.0]; 3];
        let sent = snapshot.clone();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let hello: ToService = serde_json::from_str(&line).unwrap();
            assert!(matches!(hello, ToService::Hello { ref token, role: Role::Window, .. } if token == "t0ken"));
            write_line(&mut writer, &ToClient::Snapshot { snapshot: Box::new(sent) }).unwrap();
            write_line(&mut writer, &ToClient::Notice { notice: Notice::Toast { kind: NoticeKind::Warning, message: "careful".into() } }).unwrap();
            write_line(&mut writer, &ToClient::Library { version: 7 }).unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            serde_json::from_str::<ToService>(&line).unwrap()
        });
        let client = Client::connect(&endpoint, Role::Window).unwrap();
        assert_eq!(client.snapshot().capture.trail, snapshot.capture.trail);
        client.send(&ToService::Pause { paused: true });
        assert!(matches!(server.join().unwrap(), ToService::Pause { paused: true }));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while client.library_version() != 7 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(client.library_version(), 7);
        assert!(matches!(client.take_notices().as_slice(), [Notice::Toast { kind: NoticeKind::Warning, .. }]));
    }
}
