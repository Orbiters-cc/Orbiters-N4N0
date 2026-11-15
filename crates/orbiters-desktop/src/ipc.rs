//! The link between the tray service (controller, worker, OSC, tray) and the processes that show it: the window
//! and the VR overlay. JSON lines over a loopback TCP socket; a client proves it was started by the service with
//! the token the service passed on its command line. The service never loads a GPU driver, so its footprint while
//! it waits in the tray stays small; a window exists only while it is shown.
//!
//! The state travels as what changed. A link's first line is the whole [`Snapshot`]; from then on, for each state the
//! service publishes, it writes the parts that differ from the state it wrote last ([`write_state`]: the avatar's
//! speed is 90 bytes, not the 74 kB of everything), and the client puts them into its copy ([`Client`]), which is then
//! the state the service published, whole. An unreadable line ends the link immediately, just as service EOF does.
//! A newly made link starts with the whole state. Service and clients are one program: there is no other version to support.

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
    /// The panels around VRChat's menus in desktop mode (`overlay::orbit::desktop`).
    Orbit,
}

/// Service → client.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToClient {
    SmoothingAck { id: u64, revision: u64 },
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
    VrchatData {
        id: String,
        result: crate::controller::vrchat_data::Result,
    },
    /// Come to the front.
    Show,
    /// Come to the front on this page of the app's (`"projects"`, where My Avatar is), as the menus around VRChat ask.
    ShowPage {
        page: String,
    },
    /// Close (the app quits, or another window replaced this one).
    Close,
    /// Orbiters' answer to a [`ToService::Translate`].
    Translated {
        id: String,
        result: crate::controller::translate::Answer,
    },
    /// What VRChat said over OSC of how the player moves (`VelocityX`/`Y`/`Z`, `/avatar/eyeheight`), while live
    /// translation is on: the overlay keeps translations on the world as the person walks with the stick.
    Locomotion {
        values: Vec<(String, f32)>,
    },
}

/// A long recording can only be started by the camera overlay. Other UI surfaces remain stop-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingOrigin {
    CameraOverlay,
    OtherUi,
}

/// Client → service.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToService {
    PatchSmoothing { id: u64, patch: serde_json::Value },
    Hello {
        token: String,
        role: Role,
        pid: u32,
    },
    StartCapture { origin: RecordingOrigin },
    CameraQr { active: bool },
    SwitchDemoAvatar,
    StopCapture,
    /// The Orbiters window to the front.
    ShowWindow,
    /// The Orbiters window to the front, on one of its pages ([`ToClient::ShowPage`]).
    ShowPage {
        page: String,
    },
    /// What the panels around VRChat's menus last said of themselves, for Settings.
    Orbit {
        status: crate::state::OrbitStatus,
    },
    /// The beta's usage statistics this client noted since it last sent them (`usage`).
    Usage {
        steps: Vec<crate::usage::Step>,
    },
    /// What live translation in VR is doing (the overlay says).
    TranslateStatus {
        status: crate::state::TranslateStatus,
    },
    /// Lines for Orbiters' translation service, which the service reaches with the account's credential; answered with
    /// [`ToClient::Translated`].
    Translate {
        id: String,
        batch: orbiters_translate::provider::Batch,
    },
    Pause {
        paused: bool,
    },
    /// The VRChat pictures are taken in, among several on this computer (`None`: the person's own).
    ChooseVrchat {
        pid: Option<u32>,
    },
    TestPattern,
    /// VRChat's camera flies a scan around where it stands (controller/scan.rs).
    Scan,
    SaveSettings {
        settings: Box<Settings>,
    },
    /// Changes only these settings: a JSON object of the changed fields, a section as an object of its own changed
    /// fields (`{"n4n0": {"wake_word": true}}`), merged into the service's settings as they are ([`Settings::patched`]).
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
    VrchatLogin {
        command: crate::controller::vrchat_login::Command,
    },
    VrchatData {
        id: String,
        scope: crate::controller::vrchat_data::Scope,
        request: crate::controller::vrchat_data::Request,
    },
    /// A new picture on VRChat for an avatar or world of the signed-in VRChat account, from a picture file
    /// (`Controller::vrchat_picture`; its progress in the snapshot).
    VrchatPicture {
        scope: crate::controller::vrchat_data::Scope,
        id: String,
        path: PathBuf,
    },
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
    /// Share to … opened on a 3D capture not uploaded yet: its upload files are made while friends are chosen.
    PrepareShare {
        session: PathBuf,
    },
    /// A T-pose calibration taken in the overlay (`time_ns` is a shared-clock reading).
    Calibrated {
        calibration: Box<Calibration>,
    },
    /// N4N0: a question, the Talk button, the panel, saved videos.
    N4n0 {
        command: N4n0Command,
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
    /// How Fast walk carries the person now.
    WalkPath {
        path: crate::state::WalkPath,
    },
    /// The overlay changed the walk boost's strength (also sent to the avatar's `N4N0/WalkBoost`).
    WalkBoost {
        strength: f32,
    },
    /// What the overlay concluded of the frames (`crate::booster`).
    BoosterReport {
        report: Box<crate::booster::OverlayReport>,
    },
    /// What the window or the overlay asks of the FPS booster.
    Booster {
        command: crate::booster::Command,
    },
    /// The VRChat bridge: Stop, Pause, Resume, a new token.
    Bridge {
        command: crate::bridge::Command,
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
pub use orbiters_n4n0::Command as N4n0Command;

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

/// How a line of changed parts starts; `{…}}` follows, the parts by name ([`orbiters_core::slice::changes_between`]).
/// Not a [`ToClient`]: the client reads the parts straight into its snapshot, not into a message first.
const CHANGES: &str = r#"{"type":"changes","parts":"#;

/// The state `snapshot` written to a link that last wrote `sent`: the parts that changed since, or the whole
/// [`ToClient::Snapshot`] to a link that wrote none yet. A line is written for every state, changed or not: the client
/// hears of every publish, as it did when each one was the whole state.
///
/// Made into JSON once however many links write it: the window and the overlay are handed the same published
/// snapshot, and while both keep up they last wrote the same one.
pub fn write_state(w: &mut impl Write, sent: Option<&Arc<Snapshot>>, snapshot: &Arc<Snapshot>) -> std::io::Result<()> {
    #[derive(Serialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum Out<'a> {
        Snapshot { snapshot: &'a Snapshot },
    }
    /// The last line made: from which state (none: the whole one) to which. While a snapshot is alive its address
    /// names it: one published later is another allocation.
    struct Written {
        since: Option<std::sync::Weak<Snapshot>>,
        now: std::sync::Weak<Snapshot>,
        line: Arc<[u8]>,
    }
    fn is(weak: &std::sync::Weak<Snapshot>, snapshot: &Arc<Snapshot>) -> bool {
        weak.strong_count() > 0 && std::sync::Weak::as_ptr(weak) == Arc::as_ptr(snapshot)
    }
    static LAST: Mutex<Option<Written>> = Mutex::new(None);
    let line = {
        let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let made = last.as_ref().filter(|made| {
            is(&made.now, snapshot)
                && match (&made.since, sent) {
                    (None, None) => true,
                    (Some(since), Some(sent)) => is(since, sent),
                    _ => false,
                }
        });
        match made {
            Some(made) => made.line.clone(),
            None => {
                let mut line = match sent {
                    None => serde_json::to_vec(&Out::Snapshot { snapshot }).map_err(std::io::Error::other)?,
                    Some(sent) => {
                        let (parts, _) = orbiters_core::slice::changes_between(&**snapshot, &**sent).map_err(std::io::Error::other)?;
                        let mut line = Vec::with_capacity(CHANGES.len() + parts.len() + 2);
                        line.extend_from_slice(CHANGES.as_bytes());
                        line.extend_from_slice(&parts);
                        line.push(b'}');
                        line
                    }
                };
                line.push(b'\n');
                let line: Arc<[u8]> = line.into();
                *last = Some(Written { since: sent.map(Arc::downgrade), now: Arc::downgrade(snapshot), line: line.clone() });
                line
            }
        }
    };
    w.write_all(&line)?;
    w.flush()
}

/// The changed parts of a line [`write_state`] wrote as such (a JSON object), or `None` for any other line.
pub(crate) fn changed_parts(line: &str) -> Option<&str> {
    line.strip_prefix(CHANGES)?.strip_suffix('}')
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
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "line too long")); // i18n: not shown (the service drops a connection that sends it, nobody is told)
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

/// `--say-state` (a switch of `--link-smoke`): a client prints a digest of the state its link holds whenever it takes
/// one, and says when it could not read a line and when its link ended.
pub static SAY_STATE: AtomicBool = AtomicBool::new(false);

/// A short name for a state, the same for equal states in any process of this program: what `--link-smoke` compares
/// between the service and the processes it started. Its JSON with every object's keys in order, hashed.
pub fn digest(snapshot: &Snapshot) -> String {
    fn ordered(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::Object(fields) => {
                let mut fields: Vec<_> = fields.iter().collect();
                fields.sort_by(|a, b| a.0.cmp(b.0));
                out.push('{');
                for (name, value) in fields {
                    out.push_str(name);
                    out.push(':');
                    ordered(value, out);
                    out.push(',');
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for item in items {
                    ordered(item, out);
                    out.push(',');
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut text = String::new();
    ordered(&serde_json::to_value(snapshot).unwrap_or_default(), &mut text);
    // FNV-1a, 64 bits.
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3));
    format!("{hash:016x}")
}

/// The state as a client holds it.
struct Held {
    smoothing: Vec<(u64, serde_json::Value, Option<u64>)>,
    next_smoothing: u64,
    /// What the service published, as its link last said: the snapshot it sent first, with every change since.
    service: Arc<Snapshot>,
    /// What the interface reads: `service`, or that with a change shown before the service confirms it
    /// ([`Client::update_local`]), until the service's next line.
    shown: Arc<Snapshot>,
    /// Terminal for this connection; protected by the same lock as both state replacement paths.
    closed: bool,
}

impl Held {
    fn follow_smoothing(&mut self) {
        let revision = self.service.smoothing_revision;
        self.smoothing.retain(|p| p.2.is_none_or(|ack| revision < ack));
        let mut shown = (*self.service).clone();
        for (_, patch, _) in &self.smoothing { shown.settings = shown.settings.patched(patch).0.into(); }
        self.shown = Arc::new(shown);
    }
}

#[cfg(test)]
mod smoothing_ack_tests {
    use super::*;
    #[test]
    fn two_device_sliders_and_both_menu_sliders_survive_a_stale_snapshot() {
        let first = Arc::new(Snapshot::new(orbiters_core::settings::Settings::default()));
        let mut h = Held { service: first.clone(), shown: first, closed: false, next_smoothing: 0, smoothing: Vec::new() };
        for patch in [
            serde_json::json!({"tracking":{"ms":[90,0,0]}}),
            serde_json::json!({"tracking":{"ms":[90,70,0]}}),
            serde_json::json!({"orbit":{"vr_smoothing_quick_menu_ms":60}}),
            serde_json::json!({"orbit":{"vr_smoothing_wheel_ms":75}}),
        ] {
            h.next_smoothing += 1;
            h.smoothing.push((h.next_smoothing, patch, None));
            h.follow_smoothing();
        }
        for _ in 0..90 { h.follow_smoothing(); }
        assert_eq!(h.shown.settings.tracking.ms, [90,70,0]);
        assert_eq!(h.shown.settings.orbit.vr_smoothing_quick_menu_ms, 60.0);
        assert_eq!(h.shown.settings.orbit.vr_smoothing_wheel_ms, 75.0);
    }

    #[test]
    fn releases_survive_stale_snapshots_and_settle_on_clamp_or_refusal() {
        let first = Arc::new(Snapshot::new(orbiters_core::settings::Settings::default()));
        let mut h = Held { service: first.clone(), shown: first.clone(), closed: false, next_smoothing: 2, smoothing: vec![
            (1, serde_json::json!({"tracking":{"ms":[90,0,0]}}), None),
            (2, serde_json::json!({"orbit":{"vr_smoothing_wheel_ms":75}}), None),
        ] };
        h.follow_smoothing();
        assert_eq!(h.shown.settings.tracking.ms[0], 90);
        h.service = first.clone(); // unrelated stale state after release
        h.follow_smoothing();
        assert_eq!(h.shown.settings.orbit.vr_smoothing_wheel_ms, 75.0);
        h.smoothing[0].2 = Some(1);
        h.follow_smoothing(); // acknowledgement ahead of its snapshot
        assert_eq!(h.shown.settings.tracking.ms[0], 90);
        let mut accepted = (*first).clone();
        accepted.smoothing_revision = 1;
        accepted.settings.tracking.ms[0] = 80; // clamped
        h.service = Arc::new(accepted);
        h.follow_smoothing();
        assert_eq!(h.shown.settings.tracking.ms[0], 80);
        assert_eq!(h.shown.settings.orbit.vr_smoothing_wheel_ms, 75.0);
        h.smoothing[0].2 = Some(2);
        let mut refused = (*h.service).clone();
        refused.smoothing_revision = 2;
        h.service = Arc::new(refused);
        h.follow_smoothing();
        assert!(h.smoothing.is_empty());
        assert_eq!(h.shown.settings.orbit.vr_smoothing_wheel_ms, first.settings.orbit.vr_smoothing_wheel_ms);
    }
}

/// What a client keeps of the service: the latest snapshot, queued notices and requests for the window.
pub struct Client {
    writer: Mutex<TcpStream>,
    state: Mutex<Held>,
    /// Shutdown must never wait behind a blocked command writer.
    shutdown: TcpStream,
    notices: Mutex<Vec<Notice>>,
    library: AtomicU64,
    worker_log: Mutex<Option<Vec<String>>>,
    maps: Mutex<Option<MapsMine>>,
    vrchat_data: Mutex<std::collections::HashMap<String, std::sync::mpsc::Sender<crate::controller::vrchat_data::Result>>>,
    translated: Mutex<std::collections::HashMap<String, std::sync::mpsc::Sender<crate::controller::translate::Answer>>>,
    /// [`ToClient::Locomotion`]'s values, with when they came, until the overlay takes them.
    locomotion: Mutex<Vec<(Instant, String, f32)>>,
    show: AtomicBool,
    /// A page asked for with the last Show.
    show_page: Mutex<Option<String>>,
    close: AtomicBool,
    /// Why the link ended, first reason only (`vr-test.log` says it when the overlay ends with it: K-qr-page).
    ended: Mutex<Option<String>>,
    /// The window is behind VRChat ([`Client::set_background`]).
    background: AtomicBool,
    repaint: Mutex<Option<egui::Context>>,
}

/// While the window is behind VRChat it draws the service's news at most five times a second (the service sends up to
/// 30 a second, while recording or with N4N0's menu open): docs/igpu-experiment.md.
pub const BACKGROUND_PACE: Duration = Duration::from_millis(200);

/// The window closes on its next frame, but the Mac draws no frame for a window on a sleeping display or behind others,
/// and such a window would outlive the app: past a moment for that frame, the process just ends.
fn leave_soon() {
    // Tests make links that close: their process stays.
    #[cfg(all(target_os = "macos", not(test)))]
    let _ = std::thread::Builder::new().name("leaving".into()).spawn(|| {
        std::thread::sleep(Duration::from_secs(3));
        std::process::exit(0);
    });
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
            other => anyhow::bail!("{}", orbiters_i18n::tr!("unexpected first message from the service: {message}", message = format!("{other:?}"))),
        };
        let client = Arc::new(Self {
            writer: Mutex::new(writer),
            state: Mutex::new(Held { service: first.clone(), shown: first, closed: false, smoothing: Vec::new(), next_smoothing: 0 }),
            shutdown: reader.get_ref().try_clone()?,
            notices: Mutex::new(Vec::new()),
            library: AtomicU64::new(0),
            worker_log: Mutex::new(None),
            maps: Mutex::new(None),
            vrchat_data: Mutex::default(),
            translated: Mutex::default(),
            locomotion: Mutex::default(),
            show: AtomicBool::new(false),
            show_page: Mutex::new(None),
            close: AtomicBool::new(false),
            ended: Mutex::new(None),
            background: AtomicBool::new(false),
            repaint: Mutex::new(None),
        });
        client.say_state();
        let c = client.clone();
        std::thread::Builder::new().name("service-link".into()).spawn(move || {
            for line in reader.lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(e) => {
                        c.because(format!("the link was lost: {e}")); // i18n: not shown (a log's line)
                        break;
                    }
                };
                if let Some(parts) = changed_parts(&line) {
                    // The state changed: its parts, and a frame for them as for a whole snapshot.
                    if c.take_changes(parts) {
                        c.repaint(true);
                    } else {
                        break;
                    }
                    continue;
                }
                let msg = match serde_json::from_str::<ToClient>(&line) {
                    Ok(msg) => msg,
                    Err(e) => {
                        // It may have been the state: what this client holds can no longer be trusted to be it.
                        c.because(format!("a message from the service could not be read: {e} ({}...)", line.chars().take(48).collect::<String>())); // i18n: not shown (a log's line)
                        c.disconnect();
                        break;
                    }
                };
                let news = matches!(msg, ToClient::Snapshot { .. });
                match msg {
                    ToClient::SmoothingAck { id, revision } => {
                        let mut held = c.state.lock().unwrap();
                        if let Some(p) = held.smoothing.iter_mut().find(|p| p.0 == id) { p.2 = Some(revision); }
                        held.follow_smoothing();
                    }
                    ToClient::Snapshot { snapshot } => c.take_snapshot(Arc::new(*snapshot)),
                    ToClient::Notice { notice } => c.notices.lock().unwrap().push(notice),
                    ToClient::Library { version } => c.library.store(version, Ordering::Relaxed),
                    ToClient::WorkerLog { lines } => *c.worker_log.lock().unwrap() = Some(lines),
                    ToClient::Maps { mine } => *c.maps.lock().unwrap() = Some(mine),
                    ToClient::VrchatData { id, result } => {
                        if let Some(tx) = c.vrchat_data.lock().unwrap().remove(&id) {
                            let _ = tx.send(result);
                        }
                    }
                    ToClient::Translated { id, result } => {
                        if let Some(tx) = c.translated.lock().unwrap().remove(&id) {
                            let _ = tx.send(result);
                        }
                    }
                    ToClient::Locomotion { values } => {
                        let now = Instant::now();
                        let mut held = c.locomotion.lock().unwrap();
                        held.extend(values.into_iter().map(|(name, value)| (now, name, value)));
                        // Nobody took them (translation went off in between): the latest are enough.
                        let excess = held.len().saturating_sub(256);
                        held.drain(..excess);
                        // Nothing to draw for it.
                        continue;
                    }
                    ToClient::Show => c.show.store(true, Ordering::Relaxed),
                    ToClient::ShowPage { page } => {
                        *c.show_page.lock().unwrap() = Some(page);
                        c.show.store(true, Ordering::Relaxed);
                    }
                    ToClient::Close => {
                        c.because("the service asked it to close".into()); // i18n: not shown (a log's line)
                        c.close.store(true, Ordering::Relaxed);
                        leave_soon();
                    }
                }
                c.repaint(news);
            }
            c.because("the service ended the link".into()); // i18n: not shown (a log's line)
            c.disconnect();
        })?;
        Ok(client)
    }

    /// With `--say-state`: the state the link holds, by its digest.
    fn say_state(&self) {
        if SAY_STATE.load(Ordering::Relaxed) {
            let held = self.state.lock().unwrap().service.clone();
            println!("state {}", digest(&held));
        }
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
        let mut writer = self.writer.lock().unwrap();
        if !self.state.lock().unwrap().closed {
            let _ = write_line(&mut *writer, msg);
        }
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.state.lock().unwrap().shown.clone()
    }

    /// Keep released slider values until both their acknowledgement and its authoritative state arrive.
    pub fn patch_smoothing(&self, patch: serde_json::Value) {
        let mut held = self.state.lock().unwrap();
        held.next_smoothing += 1;
        let id = held.next_smoothing;
        held.smoothing.push((id, patch.clone(), None));
        held.follow_smoothing();
        drop(held);
        self.send(&ToService::PatchSmoothing { id, patch });
        self.say_state();
    }

    pub fn update_local(&self, f: impl FnOnce(&mut Snapshot)) {
        let mut held = self.state.lock().unwrap();
        if held.closed { return; }
        let mut s = (*held.shown).clone();
        f(&mut s);
        held.shown = Arc::new(s);
    }

    /// Whole state replacement and closure are serialized. A terminated connection can never be revived by a
    /// late readable line; a new connection gets its own full baseline in connect.
    fn take_snapshot(&self, snapshot: Arc<Snapshot>) {
        let mut held = self.state.lock().unwrap();
        if held.closed { return; }
        held.service = snapshot.clone();
        held.shown = snapshot;
        held.follow_smoothing();
        drop(held);
        self.say_state();
    }

    /// Apply atomically, or terminate this connection under the very same lock. There is no retry or recovery worker.
    fn take_changes(&self, parts: &str) -> bool {
        let mut held = self.state.lock().unwrap();
        if held.closed { return false; }
        let mut next = (*held.service).clone();
        if let Err(e) = orbiters_core::slice::apply_changes(&mut next, parts) {
            self.because(format!("a change of state could not be applied: {e:?}")); // i18n: not shown (a log's line)
            self.close_locked(&mut held);
            drop(held);
            self.finish_close();
            return false;
        }
        let next = Arc::new(next);
        held.service = next.clone();
        held.shown = next;
        held.follow_smoothing();
        drop(held);
        self.say_state();
        true
    }

    /// Record the terminal decision before waking socket users. This handle is independent of the writer mutex.
    fn close_locked(&self, held: &mut Held) {
        held.closed = true;
        self.close.store(true, Ordering::Relaxed);
        let _ = self.shutdown.shutdown(std::net::Shutdown::Both);
    }

    /// EOF, unreadable input and I/O errors all use exactly the same disconnect path.
    fn disconnect(&self) {
        let mut held = self.state.lock().unwrap();
        if held.closed { return; }
        self.close_locked(&mut held);
        drop(held);
        self.finish_close();
    }

    fn finish_close(&self) {
        // Cancels waiting receivers, not work the service has already accepted. Nothing is replayed.
        self.vrchat_data.lock().unwrap().clear();
        if SAY_STATE.load(Ordering::Relaxed) { println!("link ended"); }
        self.repaint(false);
        leave_soon();
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

    pub fn vrchat_data(
        &self,
        scope: crate::controller::vrchat_data::Scope,
        request: crate::controller::vrchat_data::Request,
    ) -> crate::controller::vrchat_data::Result {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        self.vrchat_data.lock().unwrap().insert(id.clone(), tx);
        self.send(&ToService::VrchatData { id: id.clone(), scope, request });
        let result = rx.recv_timeout(Duration::from_secs(120)).unwrap_or_else(|_| Err(orbiters_i18n::tr("VRChat data request timed out. Please try again.").into()));
        self.vrchat_data.lock().unwrap().remove(&id);
        result
    }

    /// Lines translated by Orbiters' service, through the desktop service (which holds the account's credential).
    pub fn translate(&self, batch: orbiters_translate::provider::Batch) -> crate::controller::translate::Answer {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        self.translated.lock().unwrap().insert(id.clone(), tx);
        self.send(&ToService::Translate { id: id.clone(), batch });
        let result = rx.recv_timeout(Duration::from_secs(30)).unwrap_or(Err(crate::controller::translate::Refused::Unreachable("no answer".into()))); // i18n: not shown (log detail)
        self.translated.lock().unwrap().remove(&id);
        result
    }

    /// VRChat's locomotion values since the last call ([`ToClient::Locomotion`]).
    pub fn take_locomotion(&self) -> Vec<(Instant, String, f32)> {
        std::mem::take(&mut *self.locomotion.lock().unwrap())
    }

    pub fn take_show(&self) -> bool {
        self.show.swap(false, Ordering::Relaxed)
    }

    pub fn take_show_page(&self) -> Option<String> {
        self.show_page.lock().unwrap().take()
    }

    pub fn should_close(&self) -> bool {
        self.close.load(Ordering::Relaxed)
    }

    /// Why the link ended, once it did.
    pub fn ended_because(&self) -> Option<String> {
        self.ended.lock().unwrap().clone()
    }

    fn because(&self, why: String) {
        self.ended.lock().unwrap().get_or_insert(why);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::controller::NoticeKind;
    use std::net::TcpListener;

    #[test]
    fn vrchat_data_replies_are_correlated_and_pending_requests_are_removed() {
        use crate::controller::vrchat_data::{Request, Response, Scope};
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = Endpoint { port: listener.local_addr().unwrap().port(), token: "data-test".into() };
        let snapshot = Snapshot::new(Settings::default());
        let scope = Scope::of(&snapshot);
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            write_line(&mut writer, &ToClient::Snapshot { snapshot: Box::new(snapshot) }).unwrap();
            let mut replies = Vec::new();
            for _ in 0..2 {
                line.clear();
                reader.read_line(&mut line).unwrap();
                match serde_json::from_str::<ToService>(&line).unwrap() {
                    ToService::VrchatData { id, request, .. } => {
                        let result = match request {
                            Request::Image { .. } => Ok(Response::Image(vec![1, 2, 3])),
                            _ => Err("rate limited".into()),
                        };
                        replies.push(ToClient::VrchatData { id, result });
                    }
                    other => panic!("{other:?}"),
                }
            }
            for reply in replies.into_iter().rev() {
                write_line(&mut writer, &reply).unwrap();
            }
        });
        let client = Client::connect(&endpoint, Role::Window).unwrap();
        let c = client.clone();
        let s = scope.clone();
        let image = std::thread::spawn(move || c.vrchat_data(s, Request::Image { url: "https://assets.vrchat.com/image".into() }));
        assert!(matches!(client.vrchat_data(scope, Request::Friends), Err(e) if e == "rate limited"));
        assert!(matches!(image.join().unwrap(), Ok(Response::Image(bytes)) if bytes == [1, 2, 3]));
        server.join().unwrap();
        assert!(client.vrchat_data.lock().unwrap().is_empty());
    }

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
        write_state(&mut a, None, &first).unwrap();
        write_state(&mut b, None, &first).unwrap();
        assert_eq!(a, b);
        assert!(matches!(serde_json::from_slice::<ToClient>(&a).unwrap(), ToClient::Snapshot { .. }));
        // A later snapshot is written as it is, not as the one before.
        let mut changed = Snapshot::new(Settings::default());
        changed.capture.trail = vec![[1.0, 2.0, 3.0]];
        let later = Arc::new(changed);
        let mut c = Vec::new();
        write_state(&mut c, None, &later).unwrap();
        match serde_json::from_slice::<ToClient>(&c).unwrap() {
            ToClient::Snapshot { snapshot } => assert_eq!(snapshot.capture.trail, vec![[1.0, 2.0, 3.0]]),
            other => panic!("{other:?}"),
        }
        // To a link that wrote the first: only what differs, the same line for every such link, and another line for
        // a link that last wrote something else.
        let (mut d, mut e, mut f) = (Vec::new(), Vec::new(), Vec::new());
        write_state(&mut d, Some(&first), &later).unwrap();
        write_state(&mut e, Some(&first), &later).unwrap();
        assert_eq!(d, e);
        let text = String::from_utf8(d).unwrap();
        assert!(text.starts_with(r#"{"type":"changes","parts":{"capture":{"#) && text.ends_with("}}\n"), "{text}");
        assert!(!text.contains("settings"), "what did not change does not travel: {text}");
        write_state(&mut f, Some(&later), &later).unwrap();
        assert_eq!(String::from_utf8(f).unwrap(), "{\"type\":\"changes\",\"parts\":{}}\n", "a publish that changed nothing is still told");
    }

    /// A stand-in service for one client: says hello back with `first`, then hands the test the stream to write on.
    fn linked(first: &Arc<Snapshot>) -> (Arc<Client>, TcpStream, BufReader<TcpStream>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = Endpoint { port: listener.local_addr().unwrap().port(), token: "state".into() };
        let first = first.clone();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            write_state(&mut writer, None, &first).unwrap();
            (writer, reader)
        });
        let client = Client::connect(&endpoint, Role::Overlay).unwrap();
        let (writer, reader) = server.join().unwrap();
        (client, writer, reader)
    }

    /// Waits up to two seconds for `done`.
    fn until(done: impl Fn() -> bool) -> bool {
        let started = Instant::now();
        while !done() && started.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(2));
        }
        done()
    }

    /// A snapshot as a value that compares whole, whatever order a map of its settings is walked in.
    fn json(snapshot: &Snapshot) -> serde_json::Value {
        serde_json::to_value(snapshot).unwrap()
    }

    /// A state with something in every kind of part, changed a little more at each `round`: what the service's own
    /// changes look like (a plain value, a slice, a part inside a part, a list that cannot be compared).
    fn changed(snapshot: &Snapshot, round: u32) -> Snapshot {
        let mut s = snapshot.clone();
        match round % 9 {
            0 => s.vrchat.motion = Some(crate::state::AvatarMotion { in_station: false, seated: false, speed: round as f32 * 0.25, speed_at_ms: None }),
            1 => s.n4n0.panel = Some(format!("N4N0's panel, round {round}")),
            2 => s.n4n0.lines.push(crate::state::N4n0Line { who: crate::state::N4n0Who::N4n0, text: format!("line {round}"), at_ms: round as u64, streaming: false }),
            3 => {
                s.vrchat.world.players.insert(format!("usr_{round}"), format!("Someone {round}"));
            }
            4 => s.settings.walk_boost.strength = (round % 10) as f32 / 10.0,
            5 => s.projects_changed += 1,
            6 => s.capture.trail.push([round as f32, 1.0, 2.0]),
            7 => s.capture_shares.push(crate::controller::shares::ShareJob { id: format!("job-{round}"), key: "capture".into(), stage: Default::default(), outcome: None, update: false }),
            _ => {
                // Two parts that go together: the account and what it may do.
                s.account.state = crate::state::AccountState::SignedIn { username: format!("member-{round}") };
                s.bridge.allowed = round.is_multiple_of(2);
                s.n4n0.calls.picked = round as usize;
            }
        }
        s
    }

    /// Every state the service publishes reaches a client as that state, whole, however many of them the link skipped
    /// in between, and the parts that did not change are the client's own parts still (nothing was read for them).
    #[test]
    fn a_client_holds_every_state_whole_from_the_changes_alone() {
        let mut published = Arc::new(Snapshot::new(Settings::default()));
        let (client, mut writer, _reader) = linked(&published);
        assert_eq!(json(&client.snapshot()), json(&published));
        let mut sent = published.clone();
        for round in 0..400u32 {
            published = Arc::new(changed(&published, round));
            // A slow link: it writes one state in three (the mailbox keeps only the newest), from the one it last wrote.
            if round % 3 != 0 && round % 7 != 0 {
                continue;
            }
            let before = client.snapshot();
            write_state(&mut writer, Some(&sent), &published).unwrap();
            sent = published.clone();
            let wanted = json(&published);
            assert!(until(|| json(&client.snapshot()) == wanted), "round {round}: the client holds\n{}\nnot\n{wanted}", json(&client.snapshot()));
            let after = client.snapshot();
            assert!(!Arc::ptr_eq(&before, &after), "a new snapshot for every state told");
            // What these rounds never touch was sent once, with the first snapshot.
            assert!(orbiters_core::slice::Slice::same(&before.storage, &after.storage) && orbiters_core::slice::Slice::same(&before.n4n0.videos, &after.n4n0.videos));
        }
        // A state told again with nothing changed is a new snapshot all the same (a frame is drawn for it), sharing
        // every part.
        let before = client.snapshot();
        write_state(&mut writer, Some(&sent), &published).unwrap();
        assert!(until(|| !Arc::ptr_eq(&before, &client.snapshot())));
        let after = client.snapshot();
        assert!(orbiters_core::slice::Slice::same(&before.settings, &after.settings) && orbiters_core::slice::Slice::same(&before.vrchat.world, &after.vrchat.world));
        assert_eq!(json(&after), json(&published));
    }

    /// A change shown before the service confirms it lasts until the service's next state, whatever that state
    /// changed: a refusal that changes nothing of that part still puts back what the service holds.
    #[test]
    fn what_a_window_shows_ahead_goes_with_the_service_s_next_state() {
        let first = Arc::new(Snapshot::new(Settings::default()));
        let (client, mut writer, _reader) = linked(&first);
        client.update_local(|s| s.vrchat_login.busy = true);
        client.update_local(|s| s.account.state = crate::state::AccountState::SigningIn);
        assert!(client.snapshot().vrchat_login.busy && client.snapshot().account.state == crate::state::AccountState::SigningIn);
        // The service publishes something else entirely.
        let mut next = (*first).clone();
        next.n4n0.afk = true;
        let next = Arc::new(next);
        write_state(&mut writer, Some(&first), &next).unwrap();
        assert!(until(|| client.snapshot().n4n0.afk));
        let shown = client.snapshot();
        assert!(!shown.vrchat_login.busy, "the service never said busy");
        assert_eq!(shown.account.state, crate::state::AccountState::Unknown);
        // Even a publish that changed nothing does.
        client.update_local(|s| s.vrchat_login.busy = true);
        write_state(&mut writer, Some(&next), &next).unwrap();
        assert!(until(|| !client.snapshot().vrchat_login.busy));
    }

    /// A state in which every part differs from a new snapshot's: the busy evening of `--publish-cost`, and the rest.
    pub(crate) fn every_part_set() -> Snapshot {
        use crate::state::*;
        let plain = Snapshot::new(Settings::default());
        let mut s = crate::controller::publish_cost::busy_evening();
        s.smoothing_revision = 3;
        s.version = "another version".into();
        s.vrchat.log_found = !plain.vrchat.log_found;
        s.vrchat.remote = true;
        s.lan.link = LanLink::Looking;
        s.capture.state = CaptureState::Recording;
        s.capture.trail = vec![[1.0, 2.0, 3.0]];
        s.settings.walk_boost.strength += 0.25;
        s.reconstruction = Some(ReconstructionJob::new(PathBuf::from("a session"), true));
        s.vrchat_login.revision = 3;
        s.capture_shares.push(crate::controller::shares::ShareJob { id: "job".into(), key: "capture".into(), stage: Default::default(), outcome: None, update: true });
        s.update.channel = "another channel".into();
        s.cleanup = Some(CleanupJob::default());
        s.vr_audio.last = Some(crate::controller::vr_audio::Event { utc_ms: 1, text: "Set to the headset's".into(), error: false });
        s.bridge.allowed = true;
        s.orbit.note = Some("in the way".into());
        s.worn_kit = Some(true);
        s.camera_open_said = Some(false);
        s.vrchat_wrists = [Some([0.1, 1.2, -0.3, 10.0, 20.0, 30.0]), None];
        s.vrchat_wrists_ns = [7, 0];
        s.camera_tools.qr = crate::controller::camera_tools::Qr::Found { url: "https://orbiters.cc".into() };
        s.camera_tools.demo = crate::controller::camera_tools::Demo::Busy;
        s.camera_tools.qr_generation = 3;
        s.projects_changed = 5;
        s.translate.set(crate::state::TranslateStatus { lines: 3, ..Default::default() });
        let o = &mut s.n4n0;
        o.face = N4n0Face::Happy;
        o.listening = true;
        o.thinking = true;
        o.afk = true;
        o.confirmations = vec![N4n0Confirmation { id: "c1".into(), title: "Publish".into(), summary: "Saturday".into(), approve_url: "https://orbiters.cc/confirm/c1".into() }];
        o.quota = Some(orbiters_n4n0_core::assistant::Quota { used: 13, limit: 80, resets_at: None, own_key: false });
        o.test_face = Some(N4n0Face::Annoyed);
        o.test_cycle = true;
        o.test_afk = true;
        o.test_hidden = true;
        o.test_pictures = true;
        o.desktop_mode = true;
        o.menu_request = 4;
        o.menu_toggle = true;
        o.dictation = Some((2, "play some music".into()));
        o.photo_countdown = Some(3);
        o.photo_wait = Some(orbiters_n4n0::status::PhotoWait::Camera);
        o.light = Some(N4n0Light { on: true, color: 2, level: 0.5 });
        o.card = Some(orbiters_n4n0::status::CardView { title: "A link".into(), link: "https://orbiters.cc".into(), left: 40 });
        o.tracking = Some(orbiters_n4n0::status::TrackingView { on: true, ms: [4, 8, 16], note: "smoothing".into() });
        o.suggestion = Some((1, None));
        o.keys = vec![(7, orbiters_n4n0_core::menu::KeyInput::Enter)];
        o.world_video = Some(("https://youtu.be/36YnV9STBqc".into(), Some("A song".into()), Some(1_762_800_000)));
        o.photo_note = Some((2, "Open the camera before".into()));
        o.invite = Some((1, orbiters_n4n0_core::menu::InviteAnswer::Sent));
        o.qr.note = "Turn the stream camera on".into();
        o.mirror.zoom = 3;
        o.calls.picked = 2;
        s
    }

    /// Review S2: every part the wire declares, changed alone to something it is not by default, is the one part a link
    /// writes and is what a client then holds. A part left off the wire, or travelling under a name the snapshot's own
    /// JSON does not have, cannot pass; nor can a part added later that this fixture leaves as it is by default.
    #[test]
    fn every_part_of_the_state_travels_alone() {
        use orbiters_core::slice::{apply_changes, Parts};
        use serde_json::Value;
        let plain = Arc::new(Snapshot::new(Settings::default()));
        let (plain_json, full_json) = (json(&plain), json(&every_part_set()));
        let mut names = Vec::new();
        plain.part_names("", &mut names);
        // The names are the snapshot's own JSON: each field is a part, or a state whose fields are parts.
        let fields = |at: &Value, prefix: &str| at.as_object().unwrap().keys().map(|key| format!("{prefix}{key}")).collect::<Vec<_>>();
        let mut expected = Vec::new();
        for field in fields(&plain_json, "") {
            if field == "vrchat" || field == "n4n0" {
                expected.extend(fields(&plain_json[field.as_str()], &format!("{field}.")));
            } else {
                expected.push(field);
            }
        }
        let (mut declared, mut wanted) = (names.clone(), expected);
        declared.sort();
        wanted.sort();
        assert_eq!(declared, wanted, "the parts declared are the snapshot's fields");
        let at = |json: &Value, name: &str| name.split('.').fold(json, |at, key| &at[key]).clone();
        let mut as_by_default = Vec::new();
        for name in &names {
            let value = at(&full_json, name);
            if value == at(&plain_json, name) {
                as_by_default.push(name.clone());
                continue;
            }
            // The plain state with this one part changed…
            let mut one = (*plain).clone();
            apply_changes(&mut one, &serde_json::json!({ name.as_str(): value }).to_string()).unwrap();
            let one = Arc::new(one);
            assert_eq!(at(&json(&one), name), value, "{name}: read into the state");
            // …written by a link that last wrote the plain one: that part, and no other…
            let mut line = Vec::new();
            write_state(&mut line, Some(&plain), &one).unwrap();
            let line = String::from_utf8(line).unwrap();
            let parts = changed_parts(line.trim_end()).unwrap_or_else(|| panic!("{name}: {line}"));
            let written: Value = serde_json::from_str(parts).unwrap();
            assert_eq!(written.as_object().unwrap().keys().collect::<Vec<_>>(), [name], "{name}: the one part written");
            // …and read by a client that holds the plain one, which then holds the same state, whole.
            let mut held = (*plain).clone();
            apply_changes(&mut held, parts).unwrap();
            assert_eq!(json(&held), json(&one), "{name}: the client holds it");
        }
        assert!(as_by_default.is_empty(), "the fixture leaves these parts as a new snapshot has them: {as_by_default:?}");
        assert!(names.len() > 70, "{} parts", names.len());
    }

    /// Neither a held writer mutex nor late valid snapshots can prevent or undo terminal closure.
    #[test]
    pub(crate) fn unreadable_changes_close_without_the_writer_and_never_revive() {
        let first = Arc::new(Snapshot::new(Settings::default()));
        let (client, _writer, mut reader) = linked(&first);
        reader.get_mut().set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let writer_held = client.writer.lock().unwrap();
        let c = client.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let task = std::thread::spawn(move || {
            assert!(!c.take_changes(r#"{"n4n0.afk":true,"osc":false}"#));
            tx.send(()).unwrap();
        });
        rx.recv_timeout(Duration::from_secs(10)).expect("closure must not need the held writer");
        assert!(client.should_close());
        assert_eq!(json(&client.snapshot()), json(&first), "no partially applied state");
        let mut later = (*first).clone(); later.n4n0.afk = true;
        client.take_snapshot(Arc::new(later));
        assert!(!client.take_changes(r#"{"n4n0.afk":true}"#));
        assert_eq!(json(&client.snapshot()), json(&first), "late lines cannot revive a closed link");
        let mut line = String::new();
        assert_eq!(reader.read_line(&mut line).unwrap(), 0);
        drop(writer_held);
        task.join().unwrap();
    }

    #[test]
    pub(crate) fn unreadable_wire_lines_end_the_link() {
        for line in [b"not json\n".as_slice(), b"{\"type\":\"changes\",\"parts\":{\"osc\":7}}\n", b"{\"type\":\"snapshot\",\"snapshot\":{} }\n"] {
            let first = Arc::new(Snapshot::new(Settings::default()));
            let (client, mut writer, mut reader) = linked(&first);
            reader.get_mut().set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            writer.write_all(line).unwrap();
            let mut answer = String::new();
            assert_eq!(reader.read_line(&mut answer).unwrap(), 0, "no resync command is sent");
            assert!(client.should_close());
            assert_eq!(json(&client.snapshot()), json(&first));
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
