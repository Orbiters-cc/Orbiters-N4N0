//! OSC with VRChat. Orbiters - N4N0 receives avatar parameters through OSCQuery: it advertises a small OSC
//! server over mDNS (`_osc._udp`, `_oscjson._tcp`), VRChat discovers it and sends what its tree asks for:
//! `/avatar` brings `/avatar/change` and every avatar parameter (face tracking included, whichever app writes
//! it), `/tracking/vrsystem` the headset and wrist poses once the person allows it in VRChat's Tracking & IK
//! menu. That way it coexists with VRCFT, VRCOSC and other OSC apps instead of fighting for port 9001.
//! VRChat sends values when they change; its own OSCQuery service tells the current ones ([`OscService::vrchat_values`]).
//! Messages to VRChat (camera, dolly, HUD parameters) go to its input port (9000 by default).

use rosc::{OscMessage, OscPacket, OscType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum OscValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    Text(String),
}

impl OscValue {
    pub fn truthy(&self) -> bool {
        match self {
            OscValue::Bool(b) => *b,
            OscValue::Int(i) => *i != 0,
            OscValue::Float(f) => *f > 0.5,
            OscValue::Text(t) => !t.is_empty(),
        }
    }

    fn from_osc(arg: &OscType) -> Option<Self> {
        Some(match arg {
            OscType::Bool(b) => OscValue::Bool(*b),
            OscType::Int(i) => OscValue::Int(*i),
            OscType::Float(f) => OscValue::Float(*f),
            OscType::Double(d) => OscValue::Float(*d as f32),
            // A long beyond an int keeps its size, as a float, rather than wrapping around.
            OscType::Long(l) => i32::try_from(*l).map_or(OscValue::Float(*l as f32), OscValue::Int),
            OscType::String(s) => OscValue::Text(s.clone()),
            _ => return None,
        })
    }

    /// [`OscValue::from_osc`] taking the argument: a text is moved rather than copied.
    fn from_osc_owned(arg: OscType) -> Option<Self> {
        match arg {
            OscType::String(s) => Some(OscValue::Text(s)),
            other => Self::from_osc(&other),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OscEvent {
    /// `/avatar/parameters/<name>`.
    Parameter {
        name: String,
        value: OscValue,
    },
    AvatarChanged {
        id: String,
    },
    /// Any other message (`/avatar/eyeheight`, `/tracking/vrsystem/head/pose`, ...), all its arguments.
    Values {
        address: String,
        values: Vec<OscValue>,
    },
}

pub const PARAMETERS_PREFIX: &str = "/avatar/parameters/";
pub const AVATAR_CHANGE: &str = "/avatar/change";
/// The capture kit's depth-frames toggle (Mixer's `CaptureKit.DepthParameter`); its record toggle is the record
/// parameter of the settings.
pub const DEPTH_FRAMES_PARAMETER: &str = "OrbitersDepthFrames";

/// What VRChat sends that the app reads: everything under these (the avatar, its changes and parameters; the headset
/// and wrist poses; the camera). Anything else that reaches the port is dropped where it arrives.
const ACCEPTED: [&str; 3] = ["/avatar/", "/tracking/vrsystem/", "/usercamera/"];
/// The longest address, and the most arguments, of a message passed on: VRChat's addresses are a few dozen bytes and
/// a pose has six floats, while one datagram could hold thousands of arguments, each a sample to record.
const MAX_ADDRESS: usize = 512;
const MAX_ARGUMENTS: usize = 16;

/// Turns one OSC packet (bundles included) into events, in the packet's order. Only what the app reads passes
/// ([`ACCEPTED`] addresses of at most [`MAX_ADDRESS`] bytes, with at most [`MAX_ARGUMENTS`] arguments).
pub fn events_from_packet(packet: &OscPacket, out: &mut Vec<OscEvent>) {
    take_events(packet.clone(), out);
}

/// [`events_from_packet`] taking the packet: addresses and texts are moved into the events, not copied.
fn take_events(packet: OscPacket, out: &mut Vec<OscEvent>) {
    // A list instead of recursion: how deep bundles nest is up to the sender.
    let mut todo = vec![packet];
    while let Some(packet) = todo.pop() {
        match packet {
            OscPacket::Message(m) => out.extend(event_of(m.addr, m.args)),
            OscPacket::Bundle(b) => todo.extend(b.content.into_iter().rev()),
        }
    }
}

/// The event of one message; `None` for one the app does not read.
fn event_of(mut address: String, args: Vec<OscType>) -> Option<OscEvent> {
    if address.len() > MAX_ADDRESS || args.len() > MAX_ARGUMENTS || !ACCEPTED.iter().any(|prefix| address.starts_with(prefix)) {
        return None;
    }
    if address.starts_with(PARAMETERS_PREFIX) {
        let value = args.into_iter().next().and_then(OscValue::from_osc_owned)?;
        address.drain(..PARAMETERS_PREFIX.len());
        return (!address.is_empty()).then_some(OscEvent::Parameter { name: address, value });
    }
    if address == AVATAR_CHANGE {
        return match args.into_iter().next() {
            Some(OscType::String(id)) if is_avatar_id(&id) => Some(OscEvent::AvatarChanged { id }),
            _ => None,
        };
    }
    let values: Vec<OscValue> = args.into_iter().filter_map(OscValue::from_osc_owned).collect();
    (!values.is_empty()).then_some(OscEvent::Values { address, values })
}

/// An avatar id as VRChat sends it (`avtr_` and a UUID, or a local test build's): it names files (the avatar's
/// manifest), so nothing that could lead out of a folder.
fn is_avatar_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-:".contains(&c))
}

/// Deepest nesting of bundles, and of arrays in a message's arguments, that the receiver decodes. rosc decodes
/// bundles recursively and nested arrays are dropped recursively: one datagram nested a few thousand deep would
/// overflow the receiving thread's stack. VRChat sends plain messages.
const MAX_NESTING: usize = 16;

/// Whether the bundles and arrays of a datagram nest at most [`MAX_NESTING`] deep, found without recursing.
/// Malformed sizes end the scan there: rosc refuses the packet before it reaches anything deeper.
fn nesting_within_limit(data: &[u8]) -> bool {
    // Elements still to look at: their bytes and how many bundles hold them.
    let mut todo = vec![(data, 0usize)];
    while let Some((element, depth)) = todo.pop() {
        if element.starts_with(b"#bundle\0") {
            if depth >= MAX_NESTING {
                return false;
            }
            // After the tag and the time tag: elements, each a big-endian size, then that many bytes.
            let mut rest = element.get(16..).unwrap_or(&[]);
            while let Some((size, after)) = rest.split_first_chunk::<4>() {
                let Some(inner) = after.get(..u32::from_be_bytes(*size) as usize) else { break };
                todo.push((inner, depth + 1));
                rest = &after[inner.len()..];
            }
        } else if array_depth(element) > MAX_NESTING {
            return false;
        }
    }
    true
}

/// How deep a message's arguments nest arrays (`[` in its type tags).
fn array_depth(message: &[u8]) -> usize {
    // The address, null-terminated and padded to four bytes, then the type tags (",f[ii]").
    let Some(end) = message.iter().position(|&b| b == 0) else { return 0 };
    let tags = message.get((end + 4) & !3..).unwrap_or(&[]);
    let (mut depth, mut deepest) = (0usize, 0usize);
    for &tag in tags.iter().take_while(|&&t| t != 0) {
        match tag {
            b'[' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

/// OSCQuery node tree: all of `/avatar` (avatar changes and every parameter), the headset and wrist poses, and
/// the parameters named here (the Action Menu toggle) listed explicitly.
pub fn query_tree(parameters: &[String]) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    for p in parameters {
        params.insert(
            p.clone(),
            serde_json::json!({ "FULL_PATH": format!("{PARAMETERS_PREFIX}{p}"), "TYPE": "T", "ACCESS": 2, "DESCRIPTION": "Orbiters - N4N0" }),
        );
    }
    let pose = |part: &str| {
        serde_json::json!({ "FULL_PATH": format!("/tracking/vrsystem/{part}"), "ACCESS": 0, "CONTENTS": {
            "pose": { "FULL_PATH": format!("/tracking/vrsystem/{part}/pose"), "TYPE": "ffffff", "ACCESS": 2 }
        } })
    };
    serde_json::json!({
        "DESCRIPTION": "root node", "FULL_PATH": "/", "ACCESS": 0,
        "CONTENTS": {
            "avatar": { "FULL_PATH": "/avatar", "ACCESS": 0, "CONTENTS": {
                "change": { "FULL_PATH": AVATAR_CHANGE, "TYPE": "s", "ACCESS": 2 },
                "parameters": { "FULL_PATH": "/avatar/parameters", "ACCESS": 0, "CONTENTS": params }
            } },
            "tracking": { "FULL_PATH": "/tracking", "ACCESS": 0, "CONTENTS": {
                "vrsystem": { "FULL_PATH": "/tracking/vrsystem", "ACCESS": 0, "CONTENTS": {
                    "head": pose("head"), "leftwrist": pose("leftwrist"), "rightwrist": pose("rightwrist")
                } }
            } }
        }
    })
}

pub fn host_info(name: &str, osc_port: u16) -> serde_json::Value {
    serde_json::json!({
        "NAME": name, "OSC_IP": "127.0.0.1", "OSC_PORT": osc_port, "OSC_TRANSPORT": "UDP",
        "EXTENSIONS": { "ACCESS": true, "CLIPMODE": false, "RANGE": true, "TYPE": true, "VALUE": true }
    })
}

/// OSCQuery requests answered at once at most: each has a thread for up to a second and a half.
const MAX_ANSWERING: usize = 8;

/// Answers one OSCQuery request: HOST_INFO, the tree, or 404. The request may come in pieces, but whole within half a
/// second; the answer is given up on after a second (a client that stops reading).
fn serve_http(mut stream: TcpStream, tree: &str, host: &str) {
    let deadline = Instant::now() + Duration::from_millis(500);
    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
    let mut buf = [0u8; 2048];
    let mut n = 0;
    while n < buf.len() && !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            break;
        }
        match stream.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(read) => n += read,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let request = String::from_utf8_lossy(&buf[..n]);
    // No whole request line: no answer (half a line would read as the root).
    let Some((line, _)) = request.split_once("\r\n") else { return };
    let Some(path) = line.split_whitespace().nth(1) else { return };
    let (status, body) = if path.contains("HOST_INFO") {
        ("200 OK", host)
    } else if path == "/" || path.starts_with("/?") || path.starts_with("/avatar") || path.starts_with("/tracking") {
        ("200 OK", tree)
    } else {
        ("404 Not Found", "{}")
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = stream.write_all(response.as_bytes());
}

/// Largest answer [`http_get`] reads: VRChat's `/avatar` tree with every parameter of a big avatar is a few
/// hundred kilobytes.
const MAX_ANSWER: usize = 8 << 20;

/// `GET path` over HTTP/1.1 (identity or chunked body), for OSCQuery trees. `timeout` bounds the whole request,
/// answer included, which may be at most [`MAX_ANSWER`] bytes.
pub fn http_get(addr: SocketAddr, path: &str, timeout: Duration) -> anyhow::Result<String> {
    let (status, body) = http_request(addr, path, timeout)?;
    anyhow::ensure!(status == 200, "HTTP status {status}");
    Ok(String::from_utf8(body)?)
}

/// `path` with the bytes a request line cannot hold written `%XX` (a parameter's name may hold a space).
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~?=&!$'()*+,;:@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// [`http_get`] with the answer's status: (status, body).
fn http_request(addr: SocketAddr, path: &str, timeout: Duration) -> anyhow::Result<(u16, Vec<u8>)> {
    let deadline = Instant::now() + timeout;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_write_timeout(Some(timeout))?;
    let path = encode_path(path);
    // One write: `write!` would send every piece in its own segment.
    stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\nConnection: close\r\n\r\n").as_bytes())?;
    let mut raw = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        anyhow::ensure!(!left.is_zero(), "no complete answer from {addr} within {} ms", timeout.as_millis());
        stream.set_read_timeout(Some(left))?;
        let n = match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        anyhow::ensure!(raw.len() + n <= MAX_ANSWER, "the answer from {addr} is over {} MB", MAX_ANSWER >> 20);
        raw.extend_from_slice(&chunk[..n]);
    }
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| anyhow::anyhow!("no HTTP header"))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let status = head.strip_prefix("http/1.").and_then(|h| h.split_whitespace().nth(1)).and_then(|s| s.parse::<u16>().ok());
    let status = status.ok_or_else(|| anyhow::anyhow!("not an HTTP answer: {}", head.lines().next().unwrap_or("")))?;
    let body = &raw[split + 4..];
    let body = if head.contains("transfer-encoding: chunked") { dechunk(body)? } else { body.to_vec() };
    Ok((status, body))
}

fn dechunk(mut data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let line_end = data.windows(2).position(|w| w == b"\r\n").ok_or_else(|| anyhow::anyhow!("bad chunk"))?;
        let size_text = String::from_utf8_lossy(&data[..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        anyhow::ensure!(data.len() >= size, "truncated chunk");
        out.extend_from_slice(&data[..size]);
        data = data.get(size + 2..).unwrap_or(&[]);
    }
}

/// Current values VRChat reports through its OSCQuery tree at `addr` ([`OscService::vrchat_query`]): the avatar
/// (`/avatar/change`) and every avatar parameter, by address. `timeout` bounds the whole request.
pub fn vrchat_values(addr: SocketAddr, timeout: Duration) -> anyhow::Result<Vec<(String, OscValue)>> {
    let tree: serde_json::Value = serde_json::from_str(&http_get(addr, "/avatar", timeout)?)?;
    let mut out = Vec::new();
    tree_values(&tree, &mut out);
    Ok(out)
}

/// VRChat's camera as its OSCQuery tree at `addr` has it (`/usercamera/Mode`, `/usercamera/Streaming`…): empty when
/// the tree has no such node (VRChat before 2025.3.3) or VRChat did not answer within `timeout`.
pub fn vrchat_camera_values(addr: SocketAddr, timeout: Duration) -> Vec<(String, OscValue)> {
    let mut out = Vec::new();
    if let Some(tree) = http_get(addr, "/usercamera", timeout).ok().and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok()) {
        tree_values(&tree, &mut out);
    }
    out
}

/// The current value of one node of VRChat's OSCQuery tree at `addr` (`/avatar/parameters/OrbitersRecord`): `None`
/// when VRChat has no such node (the avatar worn has no such parameter); an error when VRChat could not tell (not
/// reached, no answer within `timeout`, a node without a value).
pub fn vrchat_value(addr: SocketAddr, path: &str, timeout: Duration) -> anyhow::Result<Option<OscValue>> {
    let (status, body) = http_request(addr, path, timeout)?;
    if status == 404 {
        return Ok(None);
    }
    anyhow::ensure!(status == 200, "HTTP status {status}");
    let node: serde_json::Value = serde_json::from_slice(&body)?;
    let mut values = Vec::new();
    tree_values(&node, &mut values);
    let value = values.into_iter().find(|(p, _)| p == path).map(|(_, v)| v);
    value.map(Some).ok_or_else(|| anyhow::anyhow!("VRChat's {path} has no value"))
}

/// Current values of an OSCQuery tree: every node with a `VALUE`, by full path (first value of each node).
pub fn tree_values(tree: &serde_json::Value, out: &mut Vec<(String, OscValue)>) {
    if let (Some(path), Some(kind), Some(value)) = (tree["FULL_PATH"].as_str(), tree["TYPE"].as_str(), tree["VALUE"].get(0)) {
        let v = match kind.chars().next() {
            Some('f') | Some('d') => value.as_f64().map(|f| OscValue::Float(f as f32)),
            Some('i') | Some('h') => value.as_i64().map(|i| i32::try_from(i).map_or(OscValue::Float(i as f32), OscValue::Int)),
            Some('T') | Some('F') => value.as_bool().map(OscValue::Bool),
            Some('s') => value.as_str().map(|s| OscValue::Text(s.to_string())),
            _ => None,
        };
        if let Some(v) = v {
            out.push((path.to_string(), v));
        }
    }
    if let Some(children) = tree["CONTENTS"].as_object() {
        for child in children.values() {
            tree_values(child, out);
        }
    }
}

/// Running OSC + OSCQuery service. Dropping it stops everything.
pub struct OscService {
    sender: UdpSocket,
    pub osc_port: u16,
    pub query_port: u16,
    vrchat: SocketAddr,
    stop: Arc<AtomicBool>,
    mdns: Option<mdns_sd::ServiceDaemon>,
    /// The address the services are announced with (see [`OscService::advertise`]).
    /// What VRChat is told about these services ([`loopback_answer`]), while advertised.
    announcement: Option<Announcement>,
    /// VRChat's own OSCQuery service, found over mDNS.
    vrchat_query: Arc<Mutex<Option<SocketAddr>>>,
    /// When the last packet came in ([`crate::clock::now_ns`]; 0: none yet).
    last_received: Arc<AtomicU64>,
    threads: Vec<JoinHandle<()>>,
}

/// The OSC socket, with a receive buffer of a few megabytes: face tracking sends thousands of values a second, which
/// Windows' default buffer holds for only a moment of a busy receiving thread.
fn udp_socket(bind: SocketAddr) -> std::io::Result<UdpSocket> {
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    if let Err(e) = socket.set_recv_buffer_size(4 << 20) {
        eprintln!("OSC receive buffer: {e}");
    }
    socket.bind(&bind.into())?;
    Ok(socket.into())
}

impl OscService {
    /// Starts listening. With `advertise`, registers the OSCQuery services over mDNS so VRChat finds it, and
    /// looks for VRChat's own. `on_event` gets every event with its arrival time ([`crate::clock::now_ns`]).
    pub fn start(
        name: &str,
        parameters: &[String],
        vrchat_port: u16,
        advertise: bool,
        on_event: impl Fn(OscEvent, u64) + Send + Sync + 'static,
    ) -> anyhow::Result<Self> {
        Self::start_batched(name, parameters, vrchat_port, advertise, move |events: &mut Vec<OscEvent>, at| {
            for e in events.drain(..) {
                on_event(e, at);
            }
        })
    }

    /// [`OscService::start`] handing over a datagram's events at once (a bundle holds several), so that what they
    /// change is changed in one go: `on_events` gets the list and takes them out of it.
    pub fn start_batched(
        name: &str,
        parameters: &[String],
        vrchat_port: u16,
        advertise: bool,
        mut on_events: impl FnMut(&mut Vec<OscEvent>, u64) + Send + 'static,
    ) -> anyhow::Result<Self> {
        // Both listen on 127.0.0.1 alone: VRChat sends OSC and asks OSCQuery at the address the services are
        // announced with, not at HOST_INFO's OSC_IP (checked with VRChat running, tests/vrchat_live.rs), and Orbiters
        // announces 127.0.0.1 itself ([`loopback_answer`]). A datagram spoofed from the LAN with this machine's address
        // as its source never reaches a loopback socket (review DSK-13).
        let udp = udp_socket(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
        // Woken at once when the service stops (see `Drop`); the timeout only in case that wake-up is lost.
        udp.set_read_timeout(Some(Duration::from_secs(1)))?;
        let osc_port = udp.local_addr()?.port();
        let http = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
        let query_port = http.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let vrchat_query = Arc::new(Mutex::new(None));
        let last_received = Arc::new(AtomicU64::new(0));
        let mut threads = Vec::new();

        {
            let stop = stop.clone();
            let udp = udp.try_clone()?;
            let last_received = last_received.clone();
            threads.push(std::thread::Builder::new().name("osc-receive".into()).spawn(move || {
                let mut buf = vec![0u8; 65536];
                let mut events = Vec::new();
                let mut local = LocalAddrs::default();
                while !stop.load(Ordering::Relaxed) {
                    let Ok((n, from)) = udp.recv_from(&mut buf) else { continue };
                    if n == 0 || !local.contains(from.ip()) || !nesting_within_limit(&buf[..n]) {
                        continue;
                    }
                    let at = crate::clock::now_ns();
                    last_received.store(at.max(1), Ordering::Relaxed);
                    if let Ok((_, packet)) = rosc::decoder::decode_udp(&buf[..n]) {
                        take_events(packet, &mut events);
                        if !events.is_empty() {
                            on_events(&mut events, at);
                            events.clear();
                        }
                    }
                }
            })?);
        }
        {
            let stop = stop.clone();
            let tree: Arc<str> = query_tree(parameters).to_string().into();
            let host: Arc<str> = host_info(name, osc_port).to_string().into();
            threads.push(std::thread::Builder::new().name("oscquery-http".into()).spawn(move || {
                let mut local = LocalAddrs::default();
                let answering = Arc::new(AtomicUsize::new(0));
                // Waits for connections without waking up in between (the service's end connects to wake it).
                for stream in http.incoming() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(stream) = stream else {
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    };
                    if !stream.peer_addr().is_ok_and(|from| local.contains(from.ip())) {
                        continue;
                    }
                    // Each answer on a thread of its own, a few at once: a client that sends its request slowly holds
                    // up no other.
                    if answering.fetch_add(1, Ordering::SeqCst) >= MAX_ANSWERING {
                        answering.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                    let (tree, host, done) = (tree.clone(), host.clone(), answering.clone());
                    let spawned = std::thread::Builder::new().name("oscquery-answer".into()).spawn(move || {
                        serve_http(stream, &tree, &host);
                        done.fetch_sub(1, Ordering::SeqCst);
                    });
                    if spawned.is_err() {
                        answering.fetch_sub(1, Ordering::SeqCst);
                    }
                }
            })?);
        }

        let (mdns, announcement) = if advertise {
            let announcement = Announcement { primary: primary_ipv4(), instance: format!("{}-{}", name.replace(' ', "-"), osc_port), osc_port, query_port };
            // A VRChat already running finds the services from these answers at once (and again, in case one is lost).
            announce_later(announcement.clone(), &[300, 3500, 3500]);
            // mdns-sd only browses: for VRChat's own service, and to answer for these again whenever a VRChat starts.
            match mdns_sd::ServiceDaemon::new() {
                Ok(d) => {
                    match d.browse("_oscjson._tcp.local.") {
                        Ok(events) => {
                            let (stop, found, a) = (stop.clone(), vrchat_query.clone(), announcement.clone());
                            threads.push(std::thread::Builder::new().name("oscquery-browse".into()).spawn(move || watch_vrchat(events, &stop, &found, &a))?);
                        }
                        Err(e) => eprintln!("OSCQuery browse failed: {e}"),
                    }
                    (Some(d), Some(announcement))
                }
                Err(e) => {
                    eprintln!("OSCQuery browsing failed: {e}");
                    (None, Some(announcement))
                }
            }
        } else {
            (None, None)
        };

        Ok(Self {
            sender: udp,
            osc_port,
            query_port,
            vrchat: SocketAddr::from(([127, 0, 0, 1], vrchat_port)),
            stop,
            mdns,
            announcement,
            vrchat_query,
            last_received,
            threads,
        })
    }

    /// Where the OSC socket listens (127.0.0.1 and [`OscService::osc_port`]).
    pub fn local_osc_addr(&self) -> SocketAddr {
        self.sender.local_addr().unwrap_or_else(|_| SocketAddr::from((Ipv4Addr::LOCALHOST, self.osc_port)))
    }

    pub fn advertising(&self) -> bool {
        self.announcement.is_some()
    }

    /// Makes a running VRChat find the services again (VRChat sends avatar parameters only to the services it found,
    /// and looks for them only when it starts): what [`OscService::start`] does once, and the browsing thread does
    /// whenever a VRChat starts, done now. A VRChat that already knows them ignores the answer.
    pub fn rediscover(&self) {
        if let Some(a) = &self.announcement {
            announce_later(a.clone(), &[300, 3500]);
        }
    }

    /// When VRChat (or anything else on this machine) last sent a packet ([`crate::clock::now_ns`]).
    pub fn last_received_ns(&self) -> Option<u64> {
        Some(self.last_received.load(Ordering::Relaxed)).filter(|&t| t != 0)
    }

    /// VRChat's OSCQuery service when it was found.
    pub fn vrchat_query(&self) -> Option<SocketAddr> {
        *self.vrchat_query.lock().unwrap()
    }

    /// Uses this OSCQuery address for VRChat (tests, or a known port).
    pub fn set_vrchat_query(&self, addr: Option<SocketAddr>) {
        *self.vrchat_query.lock().unwrap() = addr;
    }

    /// Sends one message to VRChat's input port.
    pub fn send(&self, address: &str, args: Vec<OscType>) -> anyhow::Result<()> {
        let packet = OscPacket::Message(OscMessage { addr: address.to_string(), args });
        let bytes = rosc::encoder::encode(&packet)?;
        self.sender.send_to(&bytes, self.vrchat)?;
        Ok(())
    }

    /// Sends messages to VRChat's input port as one bundle, one datagram, to be applied at once (VRChat takes bundles:
    /// tests/vrchat_live.rs). Order within it is kept.
    pub fn send_bundle(&self, messages: Vec<(String, Vec<OscType>)>) -> anyhow::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let content = messages.into_iter().map(|(addr, args)| OscPacket::Message(OscMessage { addr, args })).collect();
        // Time tag 1: "immediately" (OSC 1.0).
        let packet = OscPacket::Bundle(rosc::OscBundle { timetag: (0, 1).into(), content });
        let bytes = rosc::encoder::encode(&packet)?;
        self.sender.send_to(&bytes, self.vrchat)?;
        Ok(())
    }
}

/// The address this machine reaches the network from (no packet is sent: connecting a UDP socket only picks the
/// route), or `None` offline.
fn primary_ipv4() -> Option<std::net::Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() => Some(ip),
        _ => None,
    }
}

/// The services as Orbiters announces them: instance name, ports, and the interface of the default route that the
/// answers go out on (the machine's own multicast reaches its VRChat).
#[derive(Clone, Debug)]
struct Announcement {
    primary: Option<Ipv4Addr>,
    instance: String,
    osc_port: u16,
    query_port: u16,
}

/// The host name the answers give the services.
const HOST: &str = "orbiters-desktop.local";
/// How long VRChat may keep the answers' records, seconds.
const ANSWER_TTL: u32 = 120;

/// An mDNS answer for one service (`service` "_oscjson._tcp.local" or "_osc._udp.local", on `port`) at 127.0.0.1, in
/// the only shape VRChat takes one (vrc-oscquery-lib's MeaModDiscovery): the PTR record as the answer, the SRV, TXT
/// and A records as additional records, the way answers to queries come; it ignores announcements (every record an
/// answer, RFC 6762) and queries only when it starts, but takes such an answer whenever it hears one. mdns-sd
/// announces only its interfaces' addresses, never 127.0.0.1 (Windows lists no IPv4 loopback interface), and VRChat
/// sends OSC and asks OSCQuery at the announced address: so Orbiters makes these answers itself. `ttl` 0 says goodbye.
fn loopback_answer(service: &str, instance: &str, port: u16, ttl: u32) -> Vec<u8> {
    fn name(out: &mut Vec<u8>, name: &str) {
        for label in name.split('.').filter(|l| !l.is_empty()) {
            out.push(label.len().min(63) as u8);
            out.extend_from_slice(&label.as_bytes()[..label.len().min(63)]);
        }
        out.push(0);
    }
    fn record(out: &mut Vec<u8>, owner: &str, kind: u16, class: u16, ttl: u32, rdata: &[u8]) {
        name(out, owner);
        out.extend_from_slice(&kind.to_be_bytes());
        out.extend_from_slice(&class.to_be_bytes());
        out.extend_from_slice(&ttl.to_be_bytes());
        out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        out.extend_from_slice(rdata);
    }
    let full = format!("{instance}.{service}");
    // ID 0, a response with authority, no questions, one answer, three additional records.
    let mut out = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 3];
    let mut ptr = Vec::new();
    name(&mut ptr, &full);
    record(&mut out, service, 12, 1, ttl, &ptr);
    // The others are this host's alone: class IN with the cache-flush bit.
    let mut srv = vec![0, 0, 0, 0];
    srv.extend_from_slice(&port.to_be_bytes());
    name(&mut srv, HOST);
    record(&mut out, &full, 33, 0x8001, ttl, &srv);
    let txt = b"txtvers=1";
    let mut text = vec![txt.len() as u8];
    text.extend_from_slice(txt);
    record(&mut out, &full, 16, 0x8001, ttl, &text);
    record(&mut out, HOST, 1, 0x8001, ttl, &[127, 0, 0, 1]);
    out
}

/// Multicasts the answers for both services ([`loopback_answer`]) from the mDNS port, on the default route's interface.
fn announce(a: &Announcement, ttl: u32) -> std::io::Result<()> {
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddr::from(([0, 0, 0, 0], 5353)).into())?;
    if let Some(ip) = a.primary {
        socket.set_multicast_if_v4(&ip)?;
    }
    socket.set_multicast_loop_v4(true)?;
    let group: socket2::SockAddr = SocketAddr::from(([224, 0, 0, 251], 5353)).into();
    socket.send_to(&loopback_answer("_oscjson._tcp.local", &a.instance, a.query_port, ttl), &group)?;
    socket.send_to(&loopback_answer("_osc._udp.local", &a.instance, a.osc_port, ttl), &group)?;
    Ok(())
}

/// [`announce`] after each pause of `waits` (milliseconds, in turn), on a thread of its own.
fn announce_later(a: Announcement, waits: &'static [u64]) {
    let _ = std::thread::Builder::new().name("oscquery-announce".into()).spawn(move || {
        for &wait in waits {
            std::thread::sleep(Duration::from_millis(wait));
            if let Err(e) = announce(&a, ANSWER_TTL) {
                eprintln!("OSCQuery announcement failed: {e}");
            }
        }
    });
}

/// This machine's addresses: loopback and every interface's, read again (at most once a second) when an unknown
/// address knocks, since interfaces come and go.
#[derive(Default)]
struct LocalAddrs {
    addrs: Vec<std::net::IpAddr>,
    read_at: Option<std::time::Instant>,
}

impl LocalAddrs {
    fn contains(&mut self, ip: std::net::IpAddr) -> bool {
        if ip.is_loopback() || self.addrs.contains(&ip) {
            return true;
        }
        if self.read_at.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) {
            self.read_at = Some(std::time::Instant::now());
            self.addrs = if_addrs::get_if_addrs().map(|l| l.into_iter().map(|i| i.ip()).collect()).unwrap_or_default();
        }
        self.addrs.contains(&ip)
    }
}

/// Where to reach a `VRChat-Client-*` OSCQuery service announced with `addresses`: VRChat serves it on the loopback
/// interface, preferred over the LAN address it announces too. Only this machine's VRChat counts: an announcement
/// whose addresses are all another machine's (another PC running VRChat, or a host posing as one) gives `None`.
fn vrchat_address(addresses: &[Ipv4Addr], port: u16, local: &mut LocalAddrs) -> Option<SocketAddr> {
    let ip =
        if addresses.iter().any(|a| a.is_loopback()) { Ipv4Addr::LOCALHOST } else { *addresses.iter().find(|a| local.contains(std::net::IpAddr::V4(**a)))? };
    Some(SocketAddr::from((ip, port)))
}

/// Whether VRChat's OSCQuery service answers at `addr`: its HOST_INFO, within a second, has an OSC port and VRChat's
/// name (`VRChat-Client-…`, as it announces itself). An announcement pointing at another program's service on this
/// machine (an OSC app's), or back at this one, is not VRChat.
fn is_vrchat_client(addr: SocketAddr) -> bool {
    http_get(addr, "/?HOST_INFO", Duration::from_secs(1)).ok().and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok()).is_some_and(|info| {
        info["OSC_PORT"].as_u64().is_some_and(|p| (1..=65535).contains(&p)) && info["NAME"].as_str().is_some_and(|n| n.starts_with("VRChat"))
    })
}

/// How often the VRChat service in use is asked whether it still answers, and how many times in a row it may not
/// before it counts as gone (VRChat quit or crashed without saying so).
const RECHECK: Duration = Duration::from_secs(15);
const MISSES: u32 = 2;
/// An announcement that did not answer as VRChat is not tried again for this long (a host repeating it).
const REFUSED_FOR: Duration = Duration::from_secs(30);

/// The VRChat OSCQuery services announced from this machine that answered as VRChat, the newest last: the one in use.
/// A service comes in only by answering, and goes only once it stops answering, whatever the network says: an
/// announcement or a goodbye from another host changes nothing.
#[derive(Default)]
struct VrchatServices {
    verified: Vec<(String, SocketAddr)>,
    refused: HashMap<String, Instant>,
    misses: u32,
}

impl VrchatServices {
    fn current(&self) -> Option<SocketAddr> {
        self.verified.last().map(|(_, addr)| *addr)
    }

    /// `name` announced at `addr`: kept, as the one in use, when it answers as VRChat. Whether it was.
    fn announced(&mut self, name: &str, addr: SocketAddr, answers: impl Fn(SocketAddr) -> bool) -> bool {
        if self.verified.iter().any(|(n, a)| n == name && *a == addr) || self.refused.get(name).is_some_and(|t| t.elapsed() < REFUSED_FOR) {
            return false;
        }
        if !answers(addr) {
            if self.refused.len() >= 64 {
                self.refused.clear();
            }
            self.refused.insert(name.to_string(), Instant::now());
            return false;
        }
        self.refused.remove(name);
        self.verified.retain(|(n, _)| n != name);
        self.verified.push((name.to_string(), addr));
        self.misses = 0;
        true
    }

    /// The network says `name` is gone: it goes when it no longer answers.
    fn removed(&mut self, name: &str, answers: impl Fn(SocketAddr) -> bool) {
        let Some(i) = self.verified.iter().position(|(n, _)| n == name) else { return };
        if answers(self.verified[i].1) {
            return;
        }
        self.verified.remove(i);
        self.drop_silent(&answers);
    }

    /// The service in use, asked again: gone after [`MISSES`] times in a row without an answer.
    fn recheck(&mut self, answers: impl Fn(SocketAddr) -> bool) {
        let Some(addr) = self.current() else { return };
        if answers(addr) {
            self.misses = 0;
            return;
        }
        self.misses += 1;
        if self.misses >= MISSES {
            self.verified.pop();
            self.misses = 0;
            self.drop_silent(&answers);
        }
    }

    /// Those that no longer answer go, newest first, so that the one in use answers.
    fn drop_silent(&mut self, answers: &impl Fn(SocketAddr) -> bool) {
        while self.current().is_some_and(|addr| !answers(addr)) {
            self.verified.pop();
        }
    }
}

/// Keeps `found` pointing at this machine's VRChat OSCQuery service (`VRChat-Client-*`) while it runs
/// ([`VrchatServices`]). A VRChat that starts while the services run looks for them only as it starts, before it could
/// hear them: when its own service shows up, they are announced again ([`announce`]) so that it finds them.
fn watch_vrchat(events: mdns_sd::Receiver<mdns_sd::ServiceEvent>, stop: &AtomicBool, found: &Mutex<Option<SocketAddr>>, announcement: &Announcement) {
    let mut services = VrchatServices::default();
    let mut local = LocalAddrs::default();
    let mut checked = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let before = services.current();
        match events.recv_timeout(Duration::from_millis(250)) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) if info.get_fullname().starts_with("VRChat-Client-") => {
                let addresses: Vec<Ipv4Addr> = info.get_addresses_v4().into_iter().collect();
                let Some(addr) = vrchat_address(&addresses, info.get_port(), &mut local) else { continue };
                if services.announced(info.get_fullname(), addr, is_vrchat_client) {
                    checked = Instant::now();
                    // After VRChat's discovery is up (it announced itself), twice, and once more later.
                    announce_later(announcement.clone(), &[2000, 3500, 10000]);
                }
            }
            Ok(mdns_sd::ServiceEvent::ServiceRemoved(_, name)) => services.removed(&name, is_vrchat_client),
            Ok(_) | Err(mdns_sd::RecvTimeoutError::Timeout) => {}
            Err(_) => return,
        }
        if checked.elapsed() >= RECHECK {
            checked = Instant::now();
            services.recheck(is_vrchat_client);
        }
        if services.current() != before {
            *found.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = services.current();
        }
    }
}

impl Drop for OscService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(d) = self.mdns.take() {
            let _ = d.shutdown();
        }
        // VRChat forgets the services.
        if let Some(a) = &self.announcement {
            let _ = announce(a, 0);
        }
        // The receiving thread and the OSCQuery server wait for what comes next: an empty datagram and a connection
        // wake them, to see they are done. One that cannot be woken ends on its own later rather than holding up the
        // app's end.
        let _ = self.sender.send_to(&[], (Ipv4Addr::LOCALHOST, self.osc_port));
        let woken = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, self.query_port)), Duration::from_millis(500)).is_ok();
        for t in self.threads.drain(..) {
            if woken || t.thread().name() != Some("oscquery-http") {
                let _ = t.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(got: &Mutex<Vec<(OscEvent, u64)>>, n: usize) {
        for _ in 0..100 {
            if got.lock().unwrap().len() >= n {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn receives_parameters_and_serves_tree() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let svc = OscService::start("Orbiters - N4N0 test", &["OrbitersRecord".into()], 9, false, move |e, at| sink.lock().unwrap().push((e, at))).unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let send = |addr: &str, args: Vec<OscType>| {
            let msg = OscPacket::Message(OscMessage { addr: addr.into(), args });
            client.send_to(&rosc::encoder::encode(&msg).unwrap(), ("127.0.0.1", svc.osc_port)).unwrap();
        };
        let before = crate::clock::now_ns();
        send("/avatar/parameters/OrbitersRecord", vec![OscType::Bool(true)]);
        send(
            "/tracking/vrsystem/head/pose",
            vec![OscType::Float(0.1), OscType::Float(1.6), OscType::Float(0.0), OscType::Float(0.0), OscType::Float(90.0), OscType::Float(0.0)],
        );
        send("/avatar/change", vec![OscType::String("avtr_x".into())]);
        wait_for(&got, 3);
        let got = got.lock().unwrap();
        assert_eq!(got[0].0, OscEvent::Parameter { name: "OrbitersRecord".into(), value: OscValue::Bool(true) });
        assert!(got[0].1 >= before && got[0].1 <= crate::clock::now_ns());
        match &got[1].0 {
            OscEvent::Values { address, values } => {
                assert_eq!((address.as_str(), values.len(), &values[4]), ("/tracking/vrsystem/head/pose", 6, &OscValue::Float(90.0)))
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(got[2].0, OscEvent::AvatarChanged { id: "avtr_x".into() });

        let mut s = TcpStream::connect(("127.0.0.1", svc.query_port)).unwrap();
        s.write_all(b"GET /?HOST_INFO HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("\"OSC_PORT\"") && resp.contains(&svc.osc_port.to_string()));
        let tree = query_tree(&["OrbitersRecord".into()]);
        assert_eq!(tree["CONTENTS"]["avatar"]["CONTENTS"]["parameters"]["CONTENTS"]["OrbitersRecord"]["ACCESS"], 2);
        assert_eq!(tree["CONTENTS"]["tracking"]["CONTENTS"]["vrsystem"]["CONTENTS"]["leftwrist"]["CONTENTS"]["pose"]["TYPE"], "ffffff");
    }

    #[test]
    fn reads_current_values_from_vrchat_tree() {
        // A stand-in for VRChat's OSCQuery service, answering in chunks like .NET's HttpListener can.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let tree = serde_json::json!({ "FULL_PATH": "/avatar", "ACCESS": 0, "CONTENTS": {
            "change": { "FULL_PATH": "/avatar/change", "TYPE": "s", "ACCESS": 3, "VALUE": ["avtr_1234"] },
            "parameters": { "FULL_PATH": "/avatar/parameters", "ACCESS": 0, "CONTENTS": {
                "v2": { "FULL_PATH": "/avatar/parameters/v2", "ACCESS": 0, "CONTENTS": {
                    "JawOpen": { "FULL_PATH": "/avatar/parameters/v2/JawOpen", "TYPE": "f", "ACCESS": 3, "VALUE": [0.25] }
                } },
                "Viseme": { "FULL_PATH": "/avatar/parameters/Viseme", "TYPE": "i", "ACCESS": 1, "VALUE": [7] },
                "MuteSelf": { "FULL_PATH": "/avatar/parameters/MuteSelf", "TYPE": "T", "ACCESS": 1, "VALUE": [false] }
            } }
        } })
        .to_string();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let n = s.read(&mut buf).unwrap();
            assert!(String::from_utf8_lossy(&buf[..n]).starts_with("GET /avatar HTTP/1.1"));
            let (a, b) = tree.split_at(tree.len() / 2);
            let body = format!("{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n", a.len(), b.len());
            s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{body}").as_bytes()).unwrap();
        });
        let mut values = vrchat_values(addr, Duration::from_secs(2)).unwrap();
        server.join().unwrap();
        values.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            values,
            vec![
                ("/avatar/change".into(), OscValue::Text("avtr_1234".into())),
                ("/avatar/parameters/MuteSelf".into(), OscValue::Bool(false)),
                ("/avatar/parameters/Viseme".into(), OscValue::Int(7)),
                ("/avatar/parameters/v2/JawOpen".into(), OscValue::Float(0.25)),
            ]
        );
    }

    #[test]
    fn answers_that_never_end_or_grow_too_big_are_refused() {
        // One byte every 100 ms, forever: every read is quick, the whole answer never comes.
        let slow = TcpListener::bind("127.0.0.1:0").unwrap();
        let slow_addr = slow.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = slow.accept().unwrap();
            let _ = s.read(&mut [0u8; 1024]);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n");
            while s.write_all(b" ").is_ok() {
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        let started = Instant::now();
        assert!(http_get(slow_addr, "/avatar", Duration::from_millis(500)).is_err());
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());

        // A body larger than any OSCQuery tree.
        let big = TcpListener::bind("127.0.0.1:0").unwrap();
        let big_addr = big.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = big.accept().unwrap();
            let _ = s.read(&mut [0u8; 1024]);
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\n\r\n");
            let block = vec![b' '; 1 << 20];
            for _ in 0..(MAX_ANSWER >> 20) + 2 {
                if s.write_all(&block).is_err() {
                    break;
                }
            }
            // Closed only once the client gave up.
            let _ = s.read(&mut [0u8; 16]);
        });
        let err = http_get(big_addr, "/avatar", Duration::from_secs(10)).unwrap_err();
        assert!(err.to_string().contains("MB"), "{err}");
    }

    #[test]
    fn only_this_machines_vrchat_counts() {
        let mut local = LocalAddrs::default();
        // VRChat announces the loopback address with its LAN one: loopback wins.
        let addr = vrchat_address(&[Ipv4Addr::new(192, 0, 2, 7), Ipv4Addr::LOCALHOST], 50000, &mut local);
        assert_eq!(addr, Some(SocketAddr::from(([127, 0, 0, 1], 50000))));
        // Another machine's VRChat (TEST-NET-1, never this machine's), or an announcement without an address.
        assert_eq!(vrchat_address(&[Ipv4Addr::new(192, 0, 2, 7)], 50000, &mut local), None);
        assert_eq!(vrchat_address(&[], 50000, &mut local), None);
        // One of this machine's own addresses.
        if let Some(ip) = primary_ipv4() {
            assert_eq!(vrchat_address(&[Ipv4Addr::new(192, 0, 2, 7), ip], 50000, &mut local), Some(SocketAddr::from((ip, 50000))));
        }
    }

    /// `depth` bundles, one inside the other, around a message.
    fn nested_bundles(depth: usize, message: &OscMessage) -> Vec<u8> {
        let mut packet = rosc::encoder::encode(&OscPacket::Message(message.clone())).unwrap();
        for _ in 0..depth {
            let mut outer = b"#bundle\0".to_vec();
            outer.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
            outer.extend_from_slice(&(packet.len() as u32).to_be_bytes());
            outer.extend_from_slice(&packet);
            packet = outer;
        }
        packet
    }

    #[test]
    fn deep_nesting_is_dropped_without_recursing() {
        let message = OscMessage { addr: "/avatar/parameters/Viseme".into(), args: vec![OscType::Int(3)] };
        let shallow = nested_bundles(3, &message);
        assert!(nesting_within_limit(&shallow));
        assert!(nesting_within_limit(&nested_bundles(MAX_NESTING, &message)));
        assert!(!nesting_within_limit(&nested_bundles(MAX_NESTING + 1, &message)));
        // Arrays nested in a message's arguments.
        let arrays = |depth: usize| {
            let mut content = vec![OscType::Int(1)];
            for _ in 0..depth {
                content = vec![OscType::Array(rosc::OscArray { content })];
            }
            rosc::encoder::encode(&OscPacket::Message(OscMessage { addr: "/x".into(), args: content })).unwrap()
        };
        assert!(nesting_within_limit(&arrays(2)) && !nesting_within_limit(&arrays(MAX_NESTING + 1)));

        // Bundles are walked in order, at any depth they are allowed.
        let (_, packet) = rosc::decoder::decode_udp(&shallow).unwrap();
        let second = OscPacket::Message(OscMessage { addr: "/avatar/eyeheight".into(), args: vec![OscType::Float(1.5)] });
        let both = OscPacket::Bundle(rosc::OscBundle { timetag: (0, 1).into(), content: vec![packet, second] });
        let mut events = Vec::new();
        events_from_packet(&both, &mut events);
        assert_eq!(
            events,
            vec![
                OscEvent::Parameter { name: "Viseme".into(), value: OscValue::Int(3) },
                OscEvent::Values { address: "/avatar/eyeheight".into(), values: vec![OscValue::Float(1.5)] },
            ]
        );

        // A datagram nested 3,000 deep (rosc alone would overflow the receiving thread's stack) is dropped, and the
        // service keeps receiving.
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let svc = OscService::start("Orbiters - N4N0 test", &[], 9, false, move |e, at| sink.lock().unwrap().push((e, at))).unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let deep = nested_bundles(3000, &message);
        assert!(deep.len() < 65_000);
        client.send_to(&deep, ("127.0.0.1", svc.osc_port)).unwrap();
        client.send_to(&shallow, ("127.0.0.1", svc.osc_port)).unwrap();
        wait_for(&got, 1);
        std::thread::sleep(Duration::from_millis(100));
        let got = got.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, OscEvent::Parameter { name: "Viseme".into(), value: OscValue::Int(3) });
    }

    /// The answer VRChat reads: a response with one answer and three additional records, the PTR to the instance,
    /// the SRV with the port and host, the TXT, and the host's A record at 127.0.0.1; every TTL 0 for a goodbye.
    #[test]
    fn loopback_answers_have_the_shape_vrchat_reads() {
        fn read_name(buf: &[u8], at: &mut usize) -> String {
            let mut labels = Vec::new();
            loop {
                let n = buf[*at] as usize;
                *at += 1;
                if n == 0 {
                    return labels.join(".");
                }
                labels.push(String::from_utf8(buf[*at..*at + n].to_vec()).unwrap());
                *at += n;
            }
        }
        for ttl in [120, 0] {
            let a = loopback_answer("_oscjson._tcp.local", "Orbiters-desktop-50123", 50124, ttl);
            assert_eq!(&a[..12], &[0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 3]);
            let mut at = 12;
            let mut records = Vec::new();
            for _ in 0..4 {
                let owner = read_name(&a, &mut at);
                let kind = u16::from_be_bytes([a[at], a[at + 1]]);
                let class = u16::from_be_bytes([a[at + 2], a[at + 3]]);
                let record_ttl = u32::from_be_bytes(a[at + 4..at + 8].try_into().unwrap());
                let len = u16::from_be_bytes([a[at + 8], a[at + 9]]) as usize;
                at += 10;
                records.push((owner, kind, class, record_ttl, a[at..at + len].to_vec()));
                at += len;
            }
            assert_eq!(at, a.len(), "nothing after the records");
            let full = "Orbiters-desktop-50123._oscjson._tcp.local";
            let (owner, kind, class, record_ttl, data) = &records[0];
            assert_eq!((owner.as_str(), *kind, *class, *record_ttl), ("_oscjson._tcp.local", 12, 1, ttl));
            assert_eq!(read_name(data, &mut 0), full);
            let (owner, kind, class, _, data) = &records[1];
            assert_eq!((owner.as_str(), *kind, *class), (full, 33, 0x8001));
            assert_eq!(u16::from_be_bytes([data[4], data[5]]), 50124);
            assert_eq!(read_name(data, &mut 6), HOST);
            assert_eq!((records[2].1, &records[2].4[..]), (16, &b"\x09txtvers=1"[..]));
            assert_eq!((records[3].0.as_str(), records[3].1, &records[3].4[..]), (HOST, 1, &[127u8, 0, 0, 1][..]));
            assert!(records.iter().all(|r| r.3 == ttl));
        }
    }

    #[test]
    fn a_service_not_advertised_listens_on_the_loopback_interface_only() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let svc = OscService::start("Orbiters - N4N0 test", &[], 9, false, move |e, at| sink.lock().unwrap().push((e, at))).unwrap();
        assert!(svc.sender.local_addr().unwrap().ip().is_loopback());
        let Some(ip) = primary_ipv4() else { return };
        // Sent to this machine's LAN address: nothing listens there.
        let client = UdpSocket::bind((ip, 0)).unwrap();
        let msg = rosc::encoder::encode(&OscPacket::Message(OscMessage { addr: "/avatar/parameters/OrbitersRecord".into(), args: vec![OscType::Bool(true)] }))
            .unwrap();
        let _ = client.send_to(&msg, (ip, svc.osc_port));
        std::thread::sleep(Duration::from_millis(300));
        assert!(got.lock().unwrap().is_empty());
    }

    #[test]
    fn only_what_the_app_reads_passes() {
        let message = |addr: &str, args: Vec<OscType>| OscPacket::Message(OscMessage { addr: addr.into(), args });
        let mut events = Vec::new();
        for packet in [
            message("/chatbox/input", vec![OscType::String("hi".into()), OscType::Bool(true)]),
            message("/input/Jump", vec![OscType::Int(1)]),
            message(&format!("/avatar/parameters/{}", "x".repeat(600)), vec![OscType::Bool(true)]),
            message("/tracking/vrsystem/head/pose", vec![OscType::Float(0.0); MAX_ARGUMENTS + 1]),
            message("/avatar/change", vec![OscType::String(r"..\..\settings".into())]),
            message("/avatar/parameters/", vec![OscType::Bool(true)]),
        ] {
            events_from_packet(&packet, &mut events);
        }
        assert!(events.is_empty(), "{events:?}");
        events_from_packet(&message("/usercamera/Mode", vec![OscType::Int(2)]), &mut events);
        events_from_packet(&message("/avatar/parameters/Big", vec![OscType::Long(1 << 40)]), &mut events);
        events_from_packet(&message("/avatar/change", vec![OscType::String("avtr_4cf554b4-430c-4f8f-b53e-1f294eed230b".into())]), &mut events);
        assert_eq!(
            events,
            vec![
                OscEvent::Values { address: "/usercamera/Mode".into(), values: vec![OscValue::Int(2)] },
                OscEvent::Parameter { name: "Big".into(), value: OscValue::Float((1u64 << 40) as f32) },
                OscEvent::AvatarChanged { id: "avtr_4cf554b4-430c-4f8f-b53e-1f294eed230b".into() },
            ]
        );
    }

    #[test]
    fn a_request_in_pieces_gets_its_answer_and_half_a_request_none() {
        let svc = OscService::start("Orbiters - N4N0 test", &[], 9, false, |_, _| {}).unwrap();
        let mut s = TcpStream::connect(("127.0.0.1", svc.query_port)).unwrap();
        s.set_nodelay(true).unwrap();
        s.write_all(b"GET /?HOST").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        s.write_all(b"_INFO HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).unwrap();
        assert!(answer.contains("\"OSC_PORT\""), "{answer}");
        // A request line never finished: no tree comes back.
        let mut half = TcpStream::connect(("127.0.0.1", svc.query_port)).unwrap();
        half.write_all(b"GET /av").unwrap();
        let mut answer = String::new();
        let _ = half.read_to_string(&mut answer);
        assert!(answer.is_empty(), "{answer}");
        // Stopping wakes the server: it ends at once.
        let started = Instant::now();
        drop(svc);
        assert!(started.elapsed() < Duration::from_millis(900), "{:?}", started.elapsed());
    }

    #[test]
    fn reads_one_value_of_vrchats_tree() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).unwrap();
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let answer = if request.starts_with("GET /avatar/parameters/Orbiters%20Record HTTP/1.1") {
                    let body = r#"{"FULL_PATH":"/avatar/parameters/Orbiters Record","TYPE":"T","ACCESS":3,"VALUE":[true]}"#;
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len())
                } else {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_string()
                };
                s.write_all(answer.as_bytes()).unwrap();
            }
        });
        assert_eq!(vrchat_value(addr, "/avatar/parameters/Orbiters Record", Duration::from_secs(2)).unwrap(), Some(OscValue::Bool(true)));
        assert_eq!(vrchat_value(addr, "/avatar/parameters/Nope", Duration::from_secs(2)).unwrap(), None, "the avatar has no such parameter");
        server.join().unwrap();
        // Nothing listens: VRChat cannot tell, which is not a no.
        let gone = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        assert!(vrchat_value(gone, "/avatar/parameters/Nope", Duration::from_millis(500)).is_err());
    }

    #[test]
    fn only_vrchat_answers_as_vrchat() {
        let host = |info: &'static str| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            std::thread::spawn(move || {
                if let Ok((mut s, _)) = listener.accept() {
                    let _ = s.read(&mut [0u8; 1024]);
                    let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{info}", info.len()).as_bytes());
                }
            });
            addr
        };
        assert!(is_vrchat_client(host(r#"{"NAME":"VRChat-Client-A1B2C3","OSC_IP":"127.0.0.1","OSC_PORT":9000}"#)));
        assert!(!is_vrchat_client(host(r#"{"NAME":"VRCFT","OSC_IP":"127.0.0.1","OSC_PORT":9010}"#)), "another OSC app");
        assert!(!is_vrchat_client(host(r#"{"NAME":"Orbiters desktop","OSC_PORT":50000}"#)), "this one");
        assert!(!is_vrchat_client(host(r#"{"NAME":"VRChat-Client-A1B2C3"}"#)), "no OSC port");
    }

    #[test]
    fn vrchat_is_kept_until_it_stops_answering() {
        let (real, other, fake) =
            (SocketAddr::from(([127, 0, 0, 1], 50001)), SocketAddr::from(([127, 0, 0, 1], 50002)), SocketAddr::from(([127, 0, 0, 1], 50003)));
        let answering = std::cell::RefCell::new(vec![real, other]);
        let answers = |addr: SocketAddr| answering.borrow().contains(&addr);
        let (a, b) = ("VRChat-Client-AAAAAA._oscjson._tcp.local.", "VRChat-Client-BBBBBB._oscjson._tcp.local.");
        let mut s = VrchatServices::default();
        assert!(s.announced(a, real, answers));
        // An announcement that does not answer, then a goodbye for the real one from someone else: nothing changes.
        assert!(!s.announced("VRChat-Client-EVIL._oscjson._tcp.local.", fake, answers));
        s.removed(a, answers);
        assert_eq!(s.current(), Some(real));
        // A second VRChat; once it stops answering, the first is used again.
        assert!(s.announced(b, other, answers));
        assert_eq!(s.current(), Some(other));
        answering.borrow_mut().retain(|addr| *addr != other);
        s.recheck(answers);
        assert_eq!(s.current(), Some(other), "one miss is not enough");
        s.recheck(answers);
        assert_eq!(s.current(), Some(real));
        // VRChat quits: its goodbye is believed once it no longer answers.
        answering.borrow_mut().clear();
        s.removed(a, answers);
        assert_eq!(s.current(), None);
    }
}
