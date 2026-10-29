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
    /// VRChat's camera flies a scan around where it stands (controller/scan.rs).
    Scan,
    SaveSettings {
        settings: Box<Settings>,
    },
    /// Changes only these settings: a JSON object of the changed fields, a section as an object of its own changed
    /// fields (`{"orb": {"wake_word": true}}`), merged into the service's settings as they are ([`Settings::patched`]).
    /// A window's copy of the settings may lag the service's (the avatar's radial moved meanwhile): saved whole, it
    /// would put back what changed in between.
    PatchSettings {
        patch: serde_json::Value,
    },
    ImportRaw {
        dir: PathBuf,
    },
    Trash {
        dir: PathBuf,
    },
    /// Frees space for good: the capture files of `trim` (their 3D views stay), and the whole of `delete`.
    CleanUp {
        trim: Vec<PathBuf>,
        delete: Vec<PathBuf>,
    },
    Reconstruct {
        dir: PathBuf,
    },
    StopReconstruction,
    /// Takes a session off the reconstruction waiting list.
    Unqueue {
        dir: PathBuf,
    },
    SendToMixer {
        dir: PathBuf,
        title: String,
    },
    WorkerLog,
    /// Signs in to Orbiters through the browser (Connect on the page it opens).
    SignIn,
    ReopenSignIn,
    CancelSignIn,
    SignOut,
    VrchatLogin { command: crate::controller::vrchat_login::Command },
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
    CaptureShare {
        request: Box<crate::controller::shares::Request>,
    },
    /// A T-pose calibration taken in the overlay (`time_ns` is a shared-clock reading).
    Calibrated {
        calibration: Box<Calibration>,
    },
    /// N4N0: a question, the Talk button, the panel, saved videos.
    Orb {
        command: OrbCommand,
    },
    /// Make VRChat find the OSC service again (it sends avatar parameters only to services it found).
    OscReconnect,
    /// VRChat's frames the overlay saw, for world performance statistics.
    Frames {
        chunk: crate::state::FrameChunk,
    },
    /// Someone put the headset on or took it off (SteamVR's activity level, read by the overlay).
    Headset {
        in_use: bool,
    },
    /// What each of SteamVR's device indices is (read by the overlay), for tracking smoothing's strengths.
    SteamVrDevices {
        kinds: Vec<crate::tracking::DeviceKind>,
    },
    /// The overlay changed the walk boost's strength (also sent to the avatar's `N4N0/WalkBoost`).
    WalkBoost {
        strength: f32,
    },
    /// Updates of an installed app (controller/update.rs).
    Updates {
        action: UpdateAction,
    },
    Quit,
}

/// What the window asks of updates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpdateAction {
    /// Look for a newer release now.
    Check,
    /// Restart into the downloaded update: once the running work ends, or `now`.
    Restart { now: bool },
    /// No restart once the running work ends after all.
    CancelRestart,
}

/// What the window and the overlay ask of N4N0 (`orbiters_n4n0::Command`, the phone app's too).
pub use orbiters_n4n0::Command as OrbCommand;

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

/// Whether a client's token is the service's, in a time that does not depend on where they differ.
pub fn same_token(given: &str, expected: &str) -> bool {
    let (a, b) = (given.as_bytes(), expected.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn write_line<T: Serialize>(w: &mut impl Write, msg: &T) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(msg).map_err(std::io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// [`ToClient::Snapshot`] written from a shared snapshot, without copying it, and made into JSON once however many
/// links write it: the window and the overlay are handed the same published snapshot.
pub fn write_snapshot(w: &mut impl Write, snapshot: &Arc<Snapshot>) -> std::io::Result<()> {
    #[derive(Serialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum Out<'a> {
        Snapshot { snapshot: &'a Snapshot },
    }
    // The last snapshot written and its line. While it is alive the address names it: a snapshot published later is
    // another allocation.
    type Written = Option<(std::sync::Weak<Snapshot>, Arc<[u8]>)>;
    static LAST: Mutex<Written> = Mutex::new(None);
    let line = {
        let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match last.as_ref().filter(|(written, _)| written.strong_count() > 0 && std::sync::Weak::as_ptr(written) == Arc::as_ptr(snapshot)) {
            Some((_, line)) => line.clone(),
            None => {
                let mut line = serde_json::to_vec(&Out::Snapshot { snapshot }).map_err(std::io::Error::other)?;
                line.push(b'\n');
                let line: Arc<[u8]> = line.into();
                *last = Some((Arc::downgrade(snapshot), line.clone()));
                line
            }
        }
    };
    w.write_all(&line)?;
    w.flush()
}

/// Reads one line of at most `max` bytes (without its end). `Ok(None)` at the end of the stream; a longer line is an
/// error, so that a peer can never make this side keep an endless line in memory.
pub fn read_line_capped(reader: &mut impl BufRead, max: usize) -> std::io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok((!line.is_empty()).then(|| String::from_utf8_lossy(&line).into_owned()));
        }
        let (taken, done) = match available.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        line.extend_from_slice(&available[..taken]);
        reader.consume(taken);
        if line.len() > max + 1 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "line too long"));
        }
        if done {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
        }
    }
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
    /// The window is behind VRChat ([`Client::set_background`]).
    background: AtomicBool,
    repaint: Mutex<Option<egui::Context>>,
}

/// While the window is behind VRChat it draws the service's news at most five times a second (the service sends up to
/// 30 a second, while recording or with N4N0's menu open): docs/igpu-experiment.md.
pub const BACKGROUND_PACE: Duration = Duration::from_millis(200);

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
            background: AtomicBool::new(false),
            repaint: Mutex::new(None),
        });
        let c = client.clone();
        std::thread::Builder::new().name("service-link".into()).spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Ok(msg) = serde_json::from_str::<ToClient>(&line) else { continue };
                let news = matches!(msg, ToClient::Snapshot { .. });
                match msg {
                    ToClient::Snapshot { snapshot } => *c.snapshot.lock().unwrap() = Arc::new(*snapshot),
                    ToClient::Notice { notice } => c.notices.lock().unwrap().push(notice),
                    ToClient::Library { version } => c.library.store(version, Ordering::Relaxed),
                    ToClient::WorkerLog { lines } => *c.worker_log.lock().unwrap() = Some(lines),
                    ToClient::Maps { mine } => *c.maps.lock().unwrap() = Some(mine),
                    ToClient::Show => c.show.store(true, Ordering::Relaxed),
                    ToClient::Close => c.close.store(true, Ordering::Relaxed),
                }
                c.repaint(news);
            }
            // The service is gone: close too.
            c.connected.store(false, Ordering::Relaxed);
            c.close.store(true, Ordering::Relaxed);
            c.repaint(false);
        })?;
        Ok(client)
    }

    /// A frame for what came; for the service's news, while the window is behind VRChat, at most [`BACKGROUND_PACE`]
    /// after the last.
    fn repaint(&self, news: bool) {
        if let Some(ctx) = self.repaint.lock().unwrap().as_ref() {
            if news && self.background.load(Ordering::Relaxed) {
                ctx.request_repaint_after(BACKGROUND_PACE);
            } else {
                orbiters_ui::anim::request_frame(ctx);
            }
        }
    }

    /// Whether the window is behind VRChat (VRChat runs and the window does not have the focus): each frame it draws
    /// is GPU time VRChat could use, on the graphics card or, drawn on the integrated GPU, as the copy that shows it.
    pub fn set_background(&self, background: bool) {
        self.background.store(background, Ordering::Relaxed);
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
    fn a_snapshot_written_to_several_links_is_the_same_line() {
        let first = Arc::new(Snapshot::new(Settings::default()));
        let (mut a, mut b) = (Vec::new(), Vec::new());
        write_snapshot(&mut a, &first).unwrap();
        write_snapshot(&mut b, &first).unwrap();
        assert_eq!(a, b);
        assert!(matches!(serde_json::from_slice::<ToClient>(&a).unwrap(), ToClient::Snapshot { .. }));
        // A later snapshot is written as it is, not as the one before.
        let mut changed = Snapshot::new(Settings::default());
        changed.capture.trail = vec![[1.0, 2.0, 3.0]];
        let later = Arc::new(changed);
        let mut c = Vec::new();
        write_snapshot(&mut c, &later).unwrap();
        match serde_json::from_slice::<ToClient>(&c).unwrap() {
            ToClient::Snapshot { snapshot } => assert_eq!(snapshot.capture.trail, vec![[1.0, 2.0, 3.0]]),
            other => panic!("{other:?}"),
        }
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
