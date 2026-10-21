//! The link between the tray service (controller, worker, OSC, tray) and the processes that show it: the window
//! and the VR overlay. JSON lines over a loopback TCP socket; a client proves it was started by the service with
//! the token the service passed on its command line. The service never loads a GPU driver, so its footprint while
//! it waits in the tray stays small; a window exists only while it is shown.

use crate::controller::Notice;
use crate::state::{MapsMine, Snapshot};
use orbiters_contracts::performance::calibration::Calibration;
use orbiters_core::settings::Settings;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// What this member added to the Map explorer.
    Maps {
        mine: MapsMine,
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
    /// Signs in to Orbiters through the browser with `provider` (`discord` or `telegram`).
    SignIn {
        provider: String,
    },
    ReopenSignIn,
    CancelSignIn,
    SignOut,
    /// Shares pictures of a world on the Map explorer.
    UploadPictures {
        world_id: String,
        world_name: String,
        paths: Vec<PathBuf>,
    },
    /// Shares a session's reconstruction as a world's 3D preview.
    UploadPreview {
        world_id: String,
        world_name: String,
        session: PathBuf,
    },
    /// Asks what this member added to the Map explorer (answered with `ToClient::Maps`).
    Maps,
    /// A T-pose calibration taken in the overlay (`time_ns` is a shared-clock reading).
    Calibrated {
        calibration: Box<Calibration>,
    },
    /// Orb: a question, the Talk button, the panel, saved videos.
    Orb {
        command: OrbCommand,
    },
    Quit,
}

/// What the window and the overlay ask of Orb.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OrbCommand {
    /// A question typed in the window or the overlay.
    Ask {
        text: String,
    },
    /// The window's or the overlay's Talk button, pressed or let go.
    Talk {
        down: bool,
    },
    /// Shows a text on the panel as it is (tries the panel).
    Say {
        text: String,
    },
    HidePanel,
    /// Starts a new conversation (Orb forgets the current one).
    NewConversation,
    SaveVideo,
    RemoveVideo {
        url: String,
    },
    /// A picture with VRChat's camera after `delay_seconds`.
    Photo {
        delay_seconds: u32,
    },
    CopyText {
        text: String,
    },
    /// Opens a page of the Orbiters website (an approval).
    OpenLink {
        url: String,
    },
    /// Downloads the speech models.
    InstallVoice,
}

/// Longest Hello a client may send, and how long it has to send it. Any local process can connect: before it proves
/// it was started by the service, it must not make the service read without end.
const HELLO_LIMIT: usize = 4096;
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Reads a client's Hello line (token, role, pid): at most [`HELLO_LIMIT`] bytes, all within `timeout`. Byte by
/// byte, so nothing after the line is taken from the stream.
pub fn read_hello(stream: &TcpStream, timeout: Duration) -> Option<(String, Role, u32)> {
    let deadline = Instant::now() + timeout;
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    let mut reader = stream;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || line.len() >= HELLO_LIMIT {
            return None;
        }
        stream.set_read_timeout(Some(left)).ok()?;
        match reader.read(&mut byte) {
            Ok(0) => return None,
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => line.push(byte[0]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    stream.set_read_timeout(None).ok()?;
    match serde_json::from_slice(&line) {
        Ok(ToService::Hello { token, role, pid }) => Some((token, role, pid)),
        _ => None,
    }
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
    maps: Mutex<Option<MapsMine>>,
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
            maps: Mutex::new(None),
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
                    ToClient::Maps { mine } => *c.maps.lock().unwrap() = Some(mine),
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

    pub fn maps(&self) -> Option<MapsMine> {
        self.maps.lock().unwrap().clone()
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

    #[test]
    fn hello_is_short_and_quick_or_refused() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        // Reads the Hello of a client that does `client`; how long that took.
        let hello = |client: fn(TcpStream), timeout: Duration| {
            let peer = std::thread::spawn(move || client(TcpStream::connect(("127.0.0.1", port)).unwrap()));
            let (stream, _) = listener.accept().unwrap();
            let started = Instant::now();
            let got = read_hello(&stream, timeout);
            let took = started.elapsed();
            drop(stream);
            let _ = peer.join();
            (got, took)
        };
        // A line that never ends: refused after a few kilobytes, long before the timeout.
        let (got, took) = hello(
            |mut s| {
                let _ = s.write_all(&vec![b'a'; 1 << 20]);
            },
            Duration::from_secs(5),
        );
        assert!(got.is_none() && took < Duration::from_secs(2), "{took:?}");
        // Nothing at all, or a byte now and then: refused when the time is up, whatever arrives meanwhile.
        let (got, took) = hello(|_s| std::thread::sleep(Duration::from_secs(1)), Duration::from_millis(200));
        assert!(got.is_none() && took < Duration::from_millis(800), "{took:?}");
        let (got, took) = hello(
            |mut s| {
                for _ in 0..40 {
                    if s.write_all(b"{").is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            },
            Duration::from_millis(300),
        );
        assert!(got.is_none() && took < Duration::from_millis(900), "{took:?}");

        // A real Hello, and the line after it is still there to read.
        let peer = std::thread::spawn(move || {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write_line(&mut s, &ToService::Hello { token: "t0ken".into(), role: Role::Overlay, pid: 7 }).unwrap();
            write_line(&mut s, &ToService::StopCapture).unwrap();
            s
        });
        let (stream, _) = listener.accept().unwrap();
        assert_eq!(read_hello(&stream, Duration::from_secs(2)), Some(("t0ken".into(), Role::Overlay, 7)));
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        assert!(matches!(serde_json::from_str(&line).unwrap(), ToService::StopCapture));
        drop(peer.join());
    }
}
