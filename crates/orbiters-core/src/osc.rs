//! OSC with VRChat. Orbiters - N4N0 receives avatar parameters through OSCQuery: it advertises a small OSC
//! server over mDNS (`_osc._udp`, `_oscjson._tcp`), VRChat discovers it and sends what its tree asks for:
//! `/avatar` brings `/avatar/change` and every avatar parameter (face tracking included, whichever app writes
//! it), `/tracking/vrsystem` the headset and wrist poses once the person allows it in VRChat's Tracking & IK
//! menu. That way it coexists with VRCFT, VRCOSC and other OSC apps instead of fighting for port 9001.
//! VRChat sends values when they change; its own OSCQuery service tells the current ones ([`OscService::vrchat_values`]).
//! Messages to VRChat (camera, dolly, HUD parameters) go to its input port (9000 by default).
//!
//! The desktop talks with the VRChat on its own machine, over the loopback interface ([`Network::Loopback`]). A phone
//! talks with VRChat on a Quest on the same Wi-Fi ([`Network::Lan`]): the services listen on the phone's address there
//! and are announced with it, messages go to the Quest's input port, and only that Quest is heard. The Quest takes no
//! launch options, so it finds the phone only through these announcements: the app holds Android's multicast lock
//! while the service runs, and starts it again when the Wi-Fi changes (another address).
//!
//! A phone whose system keeps mDNS to itself (iOS: multicast takes an entitlement Apple grants, Bonjour does not) uses
//! [`Discovery::Platform`]: the system's Bonjour registers the services and browses, the app hands what it sees to
//! [`OscService::saw`], and the answers VRChat takes also go to the chosen VRChat directly ([`announce`]).

use rosc::{OscMessage, OscPacket, OscType};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
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
/// The avatar's name as set in its SDK (newer VRChat versions): in VRChat's OSCQuery tree, which carries it whole in
/// UTF-8 (its UDP messages may not yet), read once the avatar changed.
pub const AVATAR_NAME: &str = "/avatar/name";
/// N4N0's capture kit (contracts' `n4n0::params::CAPTURE_*`, `Docs/rig-frames.md`): its record toggle (N4N0 › Record
/// in the Action Menu, or the app while it records), whose changes start and stop recording, and its depth-frames
/// toggle. [`CAPTURE_PARAMETERS`] are all of N4N0's capture parameters, the camera rig's included: the app's own.
pub const RECORD_PARAMETER: &str = crate::contracts::n4n0::params::CAPTURE_RECORD;
pub const DEPTH_FRAMES_PARAMETER: &str = crate::contracts::n4n0::params::CAPTURE_DEPTH;
pub const CAPTURE_PARAMETERS: [&str; 9] = crate::contracts::n4n0::params::CAPTURE;
/// Flipped while recording without the wearer: an avatar uploaded before it could be built has no such parameter.
pub const HIDE_SELF_PARAMETER: &str = crate::contracts::n4n0::params::CAPTURE_HIDE_SELF;

/// What VRChat sent last for each address (all of a message's arguments, and when, on the shared clock): a session's
/// initial values, N4N0's carried parameters, the camera's mode. An avatar change forgets the last avatar's parameters
/// (a session started then lists the avatar worn, not every avatar worn before); past [`Latest::MOST`] addresses, a new
/// one is not kept. It reads as the map it keeps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Latest {
    values: HashMap<String, (Vec<OscValue>, u64)>,
}

impl Latest {
    /// Addresses kept at most: an avatar's parameters, the poses and the camera are a few hundred.
    pub const MOST: usize = 4096;

    /// What VRChat just sent to `address`, at `at_ns`.
    pub fn record(&mut self, address: String, values: Vec<OscValue>, at_ns: u64) {
        if address == AVATAR_CHANGE {
            self.values.retain(|a, _| !a.starts_with(PARAMETERS_PREFIX));
        }
        // Changed in place: most messages are an address already known.
        if let Some(latest) = self.values.get_mut(&address) {
            *latest = (values, at_ns);
        } else if self.values.len() < Self::MOST {
            self.values.insert(address, (values, at_ns));
        }
    }

    /// The first argument VRChat last sent to `address`.
    pub fn first(&self, address: &str) -> Option<OscValue> {
        self.values.get(address).and_then(|(v, _)| v.first().cloned())
    }
}

impl std::ops::Deref for Latest {
    type Target = HashMap<String, (Vec<OscValue>, u64)>;

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

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

/// The OSCQuery service's HOST_INFO: where its OSC server listens (`osc_ip`: 127.0.0.1 for the desktop, the phone's
/// Wi-Fi address on the network).
pub fn host_info(name: &str, osc_ip: Ipv4Addr, osc_port: u16) -> serde_json::Value {
    serde_json::json!({
        "NAME": name, "OSC_IP": osc_ip.to_string(), "OSC_PORT": osc_port, "OSC_TRANSPORT": "UDP",
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
    walk_tree(tree, &mut |node| out.push((node.path, node.value)));
}

/// A node of an OSCQuery tree that has a value: its full path, its first value, and whether OSC may write it (its
/// `ACCESS` is 2 or 3; VRChat's own avatar parameters, `VRMode`, `GestureLeft` or `Viseme`, are 1, read only). A node that
/// says nothing of its access counts as writable.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeNode {
    pub path: String,
    pub value: OscValue,
    pub writable: bool,
}

/// Every node of an OSCQuery tree that has a value ([`TreeNode`]).
pub fn tree_nodes(tree: &serde_json::Value, out: &mut Vec<TreeNode>) {
    walk_tree(tree, &mut |node| out.push(node));
}

/// The avatar's nodes in VRChat's OSCQuery tree at `addr` (`/avatar`: its change, its parameters, its eye height),
/// with whether OSC may write each. `timeout` bounds the whole request.
pub fn vrchat_nodes(addr: SocketAddr, timeout: Duration) -> anyhow::Result<Vec<TreeNode>> {
    let tree: serde_json::Value = serde_json::from_str(&http_get(addr, "/avatar", timeout)?)?;
    let mut out = Vec::new();
    tree_nodes(&tree, &mut out);
    Ok(out)
}

fn walk_tree(tree: &serde_json::Value, f: &mut dyn FnMut(TreeNode)) {
    if let (Some(path), Some(kind), Some(value)) = (tree["FULL_PATH"].as_str(), tree["TYPE"].as_str(), tree["VALUE"].get(0)) {
        let v = match kind.chars().next() {
            Some('f') | Some('d') => value.as_f64().map(|f| OscValue::Float(f as f32)),
            Some('i') | Some('h') => value.as_i64().map(|i| i32::try_from(i).map_or(OscValue::Float(i as f32), OscValue::Int)),
            Some('T') | Some('F') => value.as_bool().map(OscValue::Bool),
            Some('s') => value.as_str().map(|s| OscValue::Text(s.to_string())),
            _ => None,
        };
        if let Some(value) = v {
            let writable = tree["ACCESS"].as_u64().is_none_or(|access| access & 2 != 0);
            f(TreeNode { path: path.to_string(), value, writable });
        }
    }
    if let Some(children) = tree["CONTENTS"].as_object() {
        for child in children.values() {
            walk_tree(child, f);
        }
    }
}

/// Where VRChat runs, as the service reaches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Network {
    /// VRChat on this machine (the desktop): the services listen on the loopback interface alone and are announced at
    /// 127.0.0.1, and messages go to VRChat's input port there (9000 unless its launch options say otherwise).
    Loopback { vrchat_port: u16 },
    /// VRChat on another device of the local network (a Quest, from a phone on the same Wi-Fi): the services listen on
    /// this device's address there (`ip`) and are announced with it, and only the VRChat chosen is talked with.
    Lan { ip: Ipv4Addr, vrchat: LanVrchat },
}

/// The VRChat a [`Network::Lan`] service talks with: the one at `ip` when the person entered its address, else the one
/// discovery finds (the one whose OSCQuery service is named `name` when several answer).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LanVrchat {
    pub ip: Option<Ipv4Addr>,
    /// Its OSCQuery service's name (`VRChat-Client-ABCDEF`).
    pub name: Option<String>,
    /// Its input port until its OSCQuery service says (VRChat's own default, 9000).
    pub osc_port: u16,
}

/// The input port VRChat listens on unless told otherwise.
pub const VRCHAT_OSC_PORT: u16 = 9000;

/// How a service and VRChat find each other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Discovery {
    /// They do not: the address is known another way (tests, a known port).
    #[default]
    Off,
    /// This process does it all over mDNS: it browses (mdns-sd) and multicasts the answers VRChat takes.
    Mdns,
    /// The system's Bonjour registers the services and browses, for an app that cannot use multicast freely: the app
    /// hands what it sees to [`OscService::saw`]. The answers VRChat takes are still made here: they go to the chosen
    /// VRChat directly, and over multicast where the system lets them.
    Platform,
}

/// What the system's Bonjour saw of a VRChat OSCQuery service ([`Discovery::Platform`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sighting {
    /// `name` (`VRChat-Client-ABCDEF`) is announced at `addresses`, on `port`.
    Found { name: String, addresses: Vec<Ipv4Addr>, port: u16 },
    /// `name` is said to be gone.
    Gone { name: String },
}

/// The ports a service would like (0: any the system gives). A service asked for ports taken gets others: VRChat only
/// sends to the ports it found, so an app that must start its services again keeps them when it can.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ports {
    pub osc: u16,
    pub query: u16,
}

/// Where messages to VRChat go, and whose datagrams the services take: VRChat's input port on this machine (loopback),
/// or the VRChat chosen on the network (LAN), once known.
#[derive(Debug)]
struct Route {
    lan: bool,
    /// Where messages to VRChat go (`None`: no VRChat chosen on the network yet).
    target: Mutex<Option<SocketAddr>>,
    /// The VRChat chosen on the network, the only one heard (LAN).
    peer: Mutex<Option<Ipv4Addr>>,
}

impl Route {
    fn of(network: &Network) -> Self {
        match network {
            Network::Loopback { vrchat_port } => {
                Self { lan: false, target: Mutex::new(Some(SocketAddr::from((Ipv4Addr::LOCALHOST, *vrchat_port)))), peer: Mutex::new(None) }
            }
            Network::Lan { vrchat, .. } => Self {
                lan: true,
                target: Mutex::new(vrchat.ip.map(|ip| SocketAddr::from((ip, vrchat.osc_port)))),
                peer: Mutex::new(vrchat.ip),
            },
        }
    }

    fn target(&self) -> Option<SocketAddr> {
        *self.target.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn peer(&self) -> Option<Ipv4Addr> {
        *self.peer.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The VRChat at `ip`, listening on `osc_port`, is the one talked with now (LAN).
    fn choose(&self, ip: Ipv4Addr, osc_port: u16) {
        *self.peer.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ip);
        *self.target.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(SocketAddr::from((ip, osc_port)));
    }
}

/// Whose datagrams and requests the services take: this machine's own (loopback), or the chosen VRChat's (LAN). Before
/// a VRChat is chosen on the network, OSCQuery requests from the local network are answered (they read the tree, which
/// holds nothing of the person's), and no datagram is taken.
enum Heard {
    Local(LocalAddrs),
    Peer(Arc<Route>),
}

impl Heard {
    fn of(route: &Arc<Route>) -> Self {
        if route.lan {
            Heard::Peer(route.clone())
        } else {
            Heard::Local(LocalAddrs::default())
        }
    }

    /// Whether a datagram from `ip` is taken.
    fn datagram(&mut self, ip: std::net::IpAddr) -> bool {
        match self {
            Heard::Local(local) => local.contains(ip),
            Heard::Peer(route) => route.peer().is_some_and(|peer| ip == std::net::IpAddr::V4(peer)),
        }
    }

    /// Whether an OSCQuery request from `ip` is answered.
    fn request(&mut self, ip: std::net::IpAddr) -> bool {
        match self {
            Heard::Local(local) => local.contains(ip),
            Heard::Peer(route) => match (route.peer(), ip) {
                (Some(peer), ip) => ip == std::net::IpAddr::V4(peer),
                (None, std::net::IpAddr::V4(ip)) => on_lan(ip),
                (None, _) => false,
            },
        }
    }
}

/// An address of the local network (private or link-local), where a Quest on the same Wi-Fi is.
fn on_lan(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}

/// This device's addresses on the local network, by interface (`wlan0`, `Wi-Fi`): where a [`Network::Lan`] service may
/// listen (on a phone, the Wi-Fi's; never the cellular network's, which is no local one).
pub fn lan_addresses() -> Vec<(String, Ipv4Addr)> {
    if_addrs::get_if_addrs()
        .map(|list| {
            list.into_iter()
                .filter_map(|i| match i.ip() {
                    std::net::IpAddr::V4(ip) if on_lan(ip) && !ip.is_loopback() => Some((i.name, ip)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Running OSC + OSCQuery service. Dropping it stops everything.
pub struct OscService {
    sender: UdpSocket,
    pub osc_port: u16,
    pub query_port: u16,
    /// The address the services listen on (127.0.0.1, or this device's on the network).
    bound: Ipv4Addr,
    route: Arc<Route>,
    stop: Arc<AtomicBool>,
    mdns: Option<mdns_sd::ServiceDaemon>,
    /// The address the services are announced with (see [`OscService::advertise`]).
    /// What VRChat is told about these services ([`loopback_answer`]), while advertised.
    announcement: Option<Announcement>,
    /// Where what the system's Bonjour sees goes ([`Discovery::Platform`]).
    sightings: Option<mpsc::Sender<Sighting>>,
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
    /// Starts listening, for the VRChat `network` says. With `advertise`, registers the OSCQuery services over mDNS so
    /// VRChat finds it, and looks for VRChat's own. `on_event` gets every event with its arrival time
    /// ([`crate::clock::now_ns`]).
    pub fn start(
        name: &str,
        parameters: &[String],
        network: Network,
        advertise: bool,
        on_event: impl Fn(OscEvent, u64) + Send + Sync + 'static,
    ) -> anyhow::Result<Self> {
        Self::start_batched(name, parameters, network, advertise, move |events: &mut Vec<OscEvent>, at| {
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
        network: Network,
        advertise: bool,
        on_events: impl FnMut(&mut Vec<OscEvent>, u64) + Send + 'static,
    ) -> anyhow::Result<Self> {
        Self::start_with(name, parameters, network, if advertise { Discovery::Mdns } else { Discovery::Off }, Ports::default(), on_events)
    }

    /// [`OscService::start_batched`], found the way `discovery` says, on the `ports` it would like.
    pub fn start_with(
        name: &str,
        parameters: &[String],
        network: Network,
        discovery: Discovery,
        ports: Ports,
        mut on_events: impl FnMut(&mut Vec<OscEvent>, u64) + Send + 'static,
    ) -> anyhow::Result<Self> {
        // On this machine both listen on 127.0.0.1 alone: VRChat sends OSC and asks OSCQuery at the address the
        // services are announced with, not at HOST_INFO's OSC_IP (checked with VRChat running, tests/vrchat_live.rs),
        // and Orbiters announces 127.0.0.1 itself ([`loopback_answer`]). A datagram spoofed from the LAN with this
        // machine's address as its source never reaches a loopback socket (review DSK-13). On the network, they listen
        // on this device's address there, which the answers give, and take only the chosen VRChat's datagrams.
        let bound = match &network {
            Network::Loopback { .. } => Ipv4Addr::LOCALHOST,
            Network::Lan { ip, .. } => *ip,
        };
        let route = Arc::new(Route::of(&network));
        let udp = udp_socket(SocketAddr::from((bound, ports.osc))).or_else(|e| {
            if ports.osc == 0 {
                return Err(e);
            }
            eprintln!("OSC port {} is taken ({e}): another one", ports.osc);
            udp_socket(SocketAddr::from((bound, 0)))
        })?;
        // Woken at once when the service stops (see `Drop`); the timeout only in case that wake-up is lost.
        udp.set_read_timeout(Some(Duration::from_secs(1)))?;
        let osc_port = udp.local_addr()?.port();
        let http = TcpListener::bind(SocketAddr::from((bound, ports.query))).or_else(|e| {
            if ports.query == 0 {
                return Err(e);
            }
            eprintln!("OSCQuery port {} is taken ({e}): another one", ports.query);
            TcpListener::bind(SocketAddr::from((bound, 0)))
        })?;
        let query_port = http.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let vrchat_query = Arc::new(Mutex::new(None));
        let last_received = Arc::new(AtomicU64::new(0));
        let mut threads = Vec::new();

        {
            let stop = stop.clone();
            let udp = udp.try_clone()?;
            let last_received = last_received.clone();
            let mut heard = Heard::of(&route);
            threads.push(std::thread::Builder::new().name("osc-receive".into()).spawn(move || {
                let mut buf = vec![0u8; 65536];
                let mut events = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    let (n, from) = match udp.recv_from(&mut buf) {
                        Ok(got) => got,
                        Err(e) => {
                            // Not the read timeout: a socket the system took back (a phone's app that was suspended)
                            // fails at once, every time, until the service is started again.
                            if !matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
                                std::thread::sleep(Duration::from_millis(100));
                            }
                            continue;
                        }
                    };
                    if n == 0 || !heard.datagram(from.ip()) || !nesting_within_limit(&buf[..n]) {
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
            let host: Arc<str> = host_info(name, bound, osc_port).to_string().into();
            let mut heard = Heard::of(&route);
            threads.push(std::thread::Builder::new().name("oscquery-http".into()).spawn(move || {
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
                    if !stream.peer_addr().is_ok_and(|from| heard.request(from.ip())) {
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

        let (mdns, announcement, sightings) = if discovery != Discovery::Off {
            let instance = format!("{}-{}", name.replace(' ', "-"), osc_port);
            // Directly to the chosen VRChat too, where the system's Bonjour stands in for multicast.
            let direct = (discovery == Discovery::Platform).then(|| route.clone());
            let announcement = match &network {
                Network::Loopback { .. } => {
                    Announcement { interface: primary_ipv4(), host: HOST.to_string(), address: Ipv4Addr::LOCALHOST, instance, osc_port, query_port, direct }
                }
                // Its own host name: another app's answers on the network (the desktop's, at 127.0.0.1) never stand for
                // this device's.
                Network::Lan { ip, .. } => Announcement {
                    interface: Some(*ip),
                    host: format!("{}.local", instance.to_ascii_lowercase()),
                    address: *ip,
                    instance,
                    osc_port,
                    query_port,
                    direct,
                },
            };
            // A VRChat already running finds the services from these answers at once (and again, in case one is lost).
            announce_later(announcement.clone(), &[300, 3500, 3500]);
            let wanted = match &network {
                Network::Loopback { .. } => None,
                Network::Lan { vrchat, .. } => Some(vrchat.clone()),
            };
            if discovery == Discovery::Platform {
                // The system browses, and what it saw comes in through `saw`.
                let (tx, rx) = mpsc::channel::<Sighting>();
                let (stop, found, a, route) = (stop.clone(), vrchat_query.clone(), announcement.clone(), route.clone());
                let next = move || match rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(sighting) => Some(Some(sighting)),
                    Err(mpsc::RecvTimeoutError::Timeout) => Some(None),
                    Err(mpsc::RecvTimeoutError::Disconnected) => None,
                };
                threads.push(std::thread::Builder::new().name("oscquery-browse".into()).spawn(move || watch_vrchat(next, &stop, &found, &a, wanted.as_ref(), &route))?);
                (None, Some(announcement), Some(tx))
            } else {
                // mdns-sd only browses: for VRChat's own service, and to answer for these again whenever a VRChat starts.
                match mdns_sd::ServiceDaemon::new() {
                    Ok(d) => {
                        match d.browse("_oscjson._tcp.local.") {
                            Ok(events) => {
                                let (stop, found, a, route) = (stop.clone(), vrchat_query.clone(), announcement.clone(), route.clone());
                                let next = move || match events.recv_timeout(Duration::from_millis(250)) {
                                    Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => Some(Some(Sighting::Found {
                                        name: info.get_fullname().to_string(),
                                        addresses: info.get_addresses_v4().into_iter().collect(),
                                        port: info.get_port(),
                                    })),
                                    Ok(mdns_sd::ServiceEvent::ServiceRemoved(_, name)) => Some(Some(Sighting::Gone { name })),
                                    Ok(_) | Err(mdns_sd::RecvTimeoutError::Timeout) => Some(None),
                                    Err(_) => None,
                                };
                                threads.push(
                                    std::thread::Builder::new()
                                        .name("oscquery-browse".into())
                                        .spawn(move || watch_vrchat(next, &stop, &found, &a, wanted.as_ref(), &route))?,
                                );
                            }
                            Err(e) => eprintln!("OSCQuery browse failed: {e}"),
                        }
                        (Some(d), Some(announcement), None)
                    }
                    Err(e) => {
                        eprintln!("OSCQuery browsing failed: {e}");
                        (None, Some(announcement), None)
                    }
                }
            }
        } else {
            (None, None, None)
        };

        Ok(Self { sender: udp, osc_port, query_port, bound, route, stop, mdns, announcement, sightings, vrchat_query, last_received, threads })
    }

    /// Where the OSC socket listens (127.0.0.1, or this device's address on the network, and [`OscService::osc_port`]).
    pub fn local_osc_addr(&self) -> SocketAddr {
        self.sender.local_addr().unwrap_or_else(|_| SocketAddr::from((self.bound, self.osc_port)))
    }

    /// Where messages to VRChat go: its input port on this machine, or the chosen VRChat's on the network (`None`
    /// until one is: entered, or found by discovery).
    pub fn vrchat_target(&self) -> Option<SocketAddr> {
        self.route.target()
    }

    /// The VRChat to talk with on the network from now on (its address and input port: the person entered them, or
    /// chose it among several), whatever discovery found before. Nothing on this machine's own (loopback).
    pub fn choose_vrchat(&self, ip: Ipv4Addr, osc_port: u16) {
        if self.route.lan {
            self.route.choose(ip, osc_port);
        }
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

    /// What the system's Bonjour saw ([`Discovery::Platform`]; nothing otherwise).
    pub fn saw(&self, sighting: Sighting) {
        if let Some(tx) = &self.sightings {
            let _ = tx.send(sighting);
        }
    }

    /// Stops without telling VRChat the services are gone, for an app about to start them again on the same ports.
    pub fn stop_quietly(mut self) {
        self.announcement = None;
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

    /// Where messages go now, or why none can.
    fn to_vrchat(&self) -> anyhow::Result<SocketAddr> {
        self.route.target().ok_or_else(|| anyhow::anyhow!("VRChat was not found on the network yet"))
    }

    /// Sends one message to VRChat's input port.
    pub fn send(&self, address: &str, args: Vec<OscType>) -> anyhow::Result<()> {
        let to = self.to_vrchat()?;
        let packet = OscPacket::Message(OscMessage { addr: address.to_string(), args });
        let bytes = rosc::encoder::encode(&packet)?;
        self.sender.send_to(&bytes, to)?;
        Ok(())
    }

    /// Sends messages to VRChat's input port as one bundle, one datagram, to be applied at once (VRChat takes bundles:
    /// tests/vrchat_live.rs). Order within it is kept.
    pub fn send_bundle(&self, messages: Vec<(String, Vec<OscType>)>) -> anyhow::Result<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let to = self.to_vrchat()?;
        let content = messages.into_iter().map(|(addr, args)| OscPacket::Message(OscMessage { addr, args })).collect();
        // Time tag 1: "immediately" (OSC 1.0).
        let packet = OscPacket::Bundle(rosc::OscBundle { timetag: (0, 1).into(), content });
        let bytes = rosc::encoder::encode(&packet)?;
        self.sender.send_to(&bytes, to)?;
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

/// The services as Orbiters announces them: instance name, ports, the host name and address the answers give them,
/// and the interface the answers go out on (the default route's on the desktop: the machine's own multicast reaches its
/// VRChat; on a phone, the Wi-Fi's, never the cellular network's).
#[derive(Clone, Debug)]
struct Announcement {
    interface: Option<Ipv4Addr>,
    instance: String,
    host: String,
    address: Ipv4Addr,
    osc_port: u16,
    query_port: u16,
    /// [`Discovery::Platform`]: the answers go to the chosen VRChat directly too, and multicast is only tried.
    direct: Option<Arc<Route>>,
}

/// The host name the desktop's answers give the services.
const HOST: &str = "orbiters-desktop.local";
/// How long VRChat may keep the answers' records, seconds.
const ANSWER_TTL: u32 = 120;

/// An mDNS answer for one service (`service` "_oscjson._tcp.local" or "_osc._udp.local", on `port`) of `host` at
/// `address` (the desktop's at 127.0.0.1, a phone's at its Wi-Fi address), in the only shape VRChat takes one
/// (vrc-oscquery-lib's MeaModDiscovery): the PTR record as the answer, the SRV, TXT and A records as additional records,
/// the way answers to queries come; it ignores announcements (every record an answer, RFC 6762) and queries only when it
/// starts, but takes such an answer whenever it hears one. mdns-sd announces only its interfaces' addresses, never
/// 127.0.0.1 (Windows lists no IPv4 loopback interface), and VRChat sends OSC and asks OSCQuery at the announced
/// address: so Orbiters makes these answers itself. `ttl` 0 says goodbye.
fn answer(service: &str, instance: &str, host: &str, address: Ipv4Addr, port: u16, ttl: u32) -> Vec<u8> {
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
    name(&mut srv, host);
    record(&mut out, &full, 33, 0x8001, ttl, &srv);
    let txt = b"txtvers=1";
    let mut text = vec![txt.len() as u8];
    text.extend_from_slice(txt);
    record(&mut out, &full, 16, 0x8001, ttl, &text);
    record(&mut out, host, 1, 0x8001, ttl, &address.octets());
    out
}

/// A socket on the mDNS port, for the answers to go out from: VRChat's discovery may well hold to mDNS's rule that an
/// answer comes from that port.
fn mdns_socket(interface: Option<Ipv4Addr>) -> std::io::Result<socket2::Socket> {
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    // Apple's mDNSResponder holds the port with SO_REUSEPORT, which only others asking for it can share.
    #[cfg(target_vendor = "apple")]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddr::from(([0, 0, 0, 0], 5353)).into())?;
    if let Some(ip) = interface {
        socket.set_multicast_if_v4(&ip)?;
    }
    socket.set_multicast_loop_v4(true)?;
    Ok(socket)
}

/// The answers for both services ([`answer`]), for one way out.
fn answers(a: &Announcement, ttl: u32) -> [Vec<u8>; 2] {
    [
        answer("_oscjson._tcp.local", &a.instance, &a.host, a.address, a.query_port, ttl),
        answer("_osc._udp.local", &a.instance, &a.host, a.address, a.osc_port, ttl),
    ]
}

/// Multicasts the answers for both services ([`answer`]) from the mDNS port, on the announcement's interface.
///
/// [`Discovery::Platform`] differs: multicast may be refused (iOS asks for an entitlement), so it is only tried, and
/// the answers go straight to the chosen VRChat's mDNS port. The system's Bonjour answers VRChat's own query when VRChat
/// starts; these are for a VRChat that started first, and for one on a network that hides mDNS.
fn announce(a: &Announcement, ttl: u32) -> std::io::Result<()> {
    let group: socket2::SockAddr = SocketAddr::from(([224, 0, 0, 251], 5353)).into();
    let Some(route) = &a.direct else {
        let socket = mdns_socket(a.interface)?;
        for answer in answers(a, ttl) {
            socket.send_to(&answer, &group)?;
        }
        return Ok(());
    };
    let socket = match mdns_socket(a.interface) {
        Ok(socket) => socket,
        // The mDNS port is the system's: from another one, which VRChat may or may not take.
        Err(_) => {
            let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
            if let Some(ip) = a.interface {
                let _ = socket.set_multicast_if_v4(&ip);
            }
            socket
        }
    };
    let peer = route.peer();
    for answer in answers(a, ttl) {
        let _ = socket.send_to(&answer, &group);
        if let Some(peer) = peer {
            socket.send_to(&answer, &SocketAddr::from((peer, 5353)).into())?;
        }
    }
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

/// Where to reach a `VRChat-Client-*` OSCQuery service announced with `addresses` on the local network: at the address
/// the person entered (`wanted`), else at the first address of the local network it announces. A VRChat elsewhere (an
/// address the person did not enter, one beyond the local network) gives `None`.
fn lan_vrchat_address(addresses: &[Ipv4Addr], port: u16, wanted: Option<Ipv4Addr>) -> Option<SocketAddr> {
    let ip = match wanted {
        Some(ip) => addresses.contains(&ip).then_some(ip)?,
        None => *addresses.iter().find(|a| on_lan(**a) && !a.is_loopback())?,
    };
    Some(SocketAddr::from((ip, port)))
}

/// `addresses` with those sharing the most leading bits with `own` first: a host announced with several (a link-local one,
/// another interface's) is reached on the network this device is on. The order is kept otherwise.
fn nearest_first(mut addresses: Vec<Ipv4Addr>, own: Ipv4Addr) -> Vec<Ipv4Addr> {
    addresses.sort_by_key(|a| (u32::from(*a) ^ u32::from(own)).leading_zeros());
    addresses.reverse();
    addresses
}

/// Whether VRChat's OSCQuery service answers at `addr`: its HOST_INFO, within a second, has an OSC port and VRChat's
/// name (`VRChat-Client-…`, as it announces itself). An announcement pointing at another program's service on this
/// machine (an OSC app's), or back at this one, is not VRChat.
fn is_vrchat_client(addr: SocketAddr) -> bool {
    vrchat_client_port(addr).is_some()
}

/// The input port VRChat's OSCQuery service at `addr` says (its HOST_INFO's `OSC_PORT`), when it answers as VRChat.
fn vrchat_client_port(addr: SocketAddr) -> Option<u16> {
    let info = serde_json::from_str::<serde_json::Value>(&http_get(addr, "/?HOST_INFO", Duration::from_secs(1)).ok()?).ok()?;
    let port = info["OSC_PORT"].as_u64().and_then(|p| u16::try_from(p).ok()).filter(|p| *p != 0)?;
    info["NAME"].as_str().is_some_and(|n| n.starts_with("VRChat")).then_some(port)
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

/// Whether a `VRChat-Client-*` service named `full` is the one the person chose by name (any, without one): its
/// instance name, with or without the service's domain.
fn named(full: &str, name: Option<&str>) -> bool {
    let short = |s: &str| {
        let s = s.trim_end_matches('.');
        s.strip_suffix("._oscjson._tcp.local").unwrap_or(s).to_ascii_lowercase()
    };
    name.is_none_or(|name| short(full) == short(name))
}

/// Keeps `found` pointing at this machine's VRChat OSCQuery service (`VRChat-Client-*`) while it runs
/// ([`VrchatServices`]), or, on the network (`lan`: the VRChat wanted there), at the chosen VRChat's, whose address and
/// input port the `route` takes. A VRChat that starts while the services run looks for them only as it starts, before
/// it could hear them: when its own service shows up, they are announced again ([`announce`]) so that it finds them.
fn watch_vrchat(
    mut next: impl FnMut() -> Option<Option<Sighting>>,
    stop: &AtomicBool,
    found: &Mutex<Option<SocketAddr>>,
    announcement: &Announcement,
    lan: Option<&LanVrchat>,
    route: &Route,
) {
    let mut services = VrchatServices::default();
    let mut local = LocalAddrs::default();
    let mut checked = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let before = services.current();
        match next() {
            // A short name from the system's Bonjour, a full one from mdns-sd.
            Some(Some(Sighting::Found { name, addresses, port })) if name.starts_with("VRChat-Client-") => {
                let addr = match lan {
                    None => vrchat_address(&addresses, port, &mut local),
                    Some(wanted) if named(&name, wanted.name.as_deref()) => {
                        lan_vrchat_address(&nearest_first(addresses, announcement.address), port, wanted.ip)
                    }
                    Some(_) => None,
                };
                let Some(addr) = addr else { continue };
                if services.announced(&name, addr, is_vrchat_client) {
                    checked = Instant::now();
                    // After VRChat's discovery is up (it announced itself), twice, and once more later.
                    announce_later(announcement.clone(), &[2000, 3500, 10000]);
                }
            }
            Some(Some(Sighting::Gone { name })) => services.removed(&name, is_vrchat_client),
            Some(_) => {}
            None => return,
        }
        if checked.elapsed() >= RECHECK {
            checked = Instant::now();
            services.recheck(is_vrchat_client);
        }
        if services.current() != before {
            let current = services.current();
            *found.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = current;
            // On the network, the VRChat found is the one talked with now, at the input port it says. One that went
            // stays chosen: the Quest may come back, and nothing else is heard meanwhile.
            if let (Some(wanted), Some(addr)) = (lan, current) {
                if let std::net::SocketAddr::V4(v4) = addr {
                    route.choose(*v4.ip(), vrchat_client_port(addr).unwrap_or(wanted.osc_port));
                }
            }
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
        let _ = self.sender.send_to(&[], (self.bound, self.osc_port));
        let woken = TcpStream::connect_timeout(&SocketAddr::from((self.bound, self.query_port)), Duration::from_millis(500)).is_ok();
        for t in self.threads.drain(..) {
            if woken || t.thread().name() != Some("oscquery-http") {
                let _ = t.join();
            }
        }
    }
}

#[cfg(test)]
mod platform_tests {
    use super::*;

    /// A stand-in for a VRChat's OSCQuery service: answers HOST_INFO as VRChat, with `osc_port`.
    fn vrchat(osc_port: u16) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = serde_json::json!({ "NAME": "VRChat-Client-TEST", "OSC_PORT": osc_port }).to_string();
                let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        addr
    }

    fn lan_service(ports: Ports) -> OscService {
        let network = Network::Lan { ip: Ipv4Addr::LOCALHOST, vrchat: LanVrchat { osc_port: VRCHAT_OSC_PORT, ..Default::default() } };
        OscService::start_with("Test", &[], network, Discovery::Platform, ports, |_, _| {}).unwrap()
    }

    /// `next` as the mdns-sd and system-Bonjour paths both drive it. (127.0.0.1 is no address of the local network, so
    /// the name's check is what is exercised here; the choice itself is `lan_vrchat_address`'s tests.)
    #[test]
    fn what_the_system_sees_reaches_the_watcher_and_others_are_ignored() {
        let service = lan_service(Ports::default());
        let addr = vrchat(9123);
        service.saw(Sighting::Found { name: "Some-Other-Service".into(), addresses: vec![Ipv4Addr::new(192, 168, 1, 9)], port: addr.port() });
        service.saw(Sighting::Gone { name: "VRChat-Client-NOT-KNOWN".into() });
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(service.vrchat_query(), None, "neither is a VRChat that answered");
    }

    #[test]
    fn a_host_with_several_addresses_is_reached_on_this_device_s_network() {
        let ip = |s: &str| s.parse::<Ipv4Addr>().unwrap();
        let addresses = vec![ip("127.0.0.1"), ip("169.254.150.121"), ip("10.0.0.9"), ip("192.168.1.134")];
        let sorted = nearest_first(addresses, ip("192.168.1.20"));
        assert_eq!(sorted[0], ip("192.168.1.134"));
        // And it is what gets chosen, loopback never.
        assert_eq!(lan_vrchat_address(&sorted, 5000, None), Some(SocketAddr::from((ip("192.168.1.134"), 5000))));
        assert_eq!(
            lan_vrchat_address(&nearest_first(vec![ip("127.0.0.1"), ip("169.254.1.1")], ip("192.168.1.20")), 5000, None),
            Some(SocketAddr::from((ip("169.254.1.1"), 5000)))
        );
    }

    #[test]
    fn ports_asked_for_are_kept_and_taken_ones_replaced() {
        let first = lan_service(Ports::default());
        let wanted = Ports { osc: first.osc_port, query: first.query_port };
        // Taken: others, and no failure.
        let second = lan_service(wanted);
        assert_ne!((second.osc_port, second.query_port), (wanted.osc, wanted.query));
        drop(second);
        // Free again, on purpose without a goodbye: the same ones.
        first.stop_quietly();
        let again = lan_service(wanted);
        assert_eq!((again.osc_port, again.query_port), (wanted.osc, wanted.query));
    }

    /// The answers reach the chosen VRChat directly, in the shape VRChat takes, with this service's ports.
    #[test]
    fn answers_go_directly_to_the_chosen_vrchat() {
        let route = Arc::new(Route { lan: true, target: Mutex::new(None), peer: Mutex::new(None) });
        let a = Announcement {
            interface: None,
            instance: "Test-4242".into(),
            host: "test-4242.local".into(),
            address: Ipv4Addr::new(192, 168, 1, 20),
            osc_port: 4242,
            query_port: 4243,
            direct: Some(route),
        };
        let [query, osc] = answers(&a, ANSWER_TTL);
        let text = |b: &[u8]| String::from_utf8_lossy(b).to_string();
        assert!(text(&query).contains("_oscjson"), "{query:?}");
        assert!(text(&osc).contains("_osc"), "{osc:?}");
        assert!(query.windows(2).any(|w| w == 4243u16.to_be_bytes()), "the query port is in the SRV record");
        assert!(osc.windows(2).any(|w| w == 4242u16.to_be_bytes()), "the OSC port is in the SRV record");
        assert!(query.windows(4).any(|w| w == [192, 168, 1, 20]), "the phone's address is in the A record");
        // No chosen VRChat and a socket that may not multicast: nothing fails.
        announce(&a, ANSWER_TTL).unwrap();
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
        let svc = OscService::start("Orbiters - N4N0 test", &["OrbitersRecord".into()], Network::Loopback { vrchat_port: 9 }, false, move |e, at| sink.lock().unwrap().push((e, at))).unwrap();
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
        let tree_copy = tree.clone();
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
        // With what OSC may write: VRChat's own are read only.
        let mut nodes = Vec::new();
        tree_nodes(&serde_json::from_str(&tree_copy).unwrap(), &mut nodes);
        nodes.sort_by(|a, b| a.path.cmp(&b.path));
        let writable: Vec<(&str, bool)> = nodes.iter().map(|n| (n.path.as_str(), n.writable)).collect();
        assert_eq!(
            writable,
            vec![
                ("/avatar/change", true),
                ("/avatar/parameters/MuteSelf", false),
                ("/avatar/parameters/Viseme", false),
                ("/avatar/parameters/v2/JawOpen", true)
            ]
        );
        let mut silent = Vec::new();
        tree_nodes(&serde_json::json!({ "FULL_PATH": "/avatar/parameters/Old", "TYPE": "T", "VALUE": [true] }), &mut silent);
        assert!(silent[0].writable, "a tree that says nothing of access: writable");
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
        let svc = OscService::start("Orbiters - N4N0 test", &[], Network::Loopback { vrchat_port: 9 }, false, move |e, at| sink.lock().unwrap().push((e, at))).unwrap();
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

    /// The desktop's answer: its services at 127.0.0.1, under its own name.
    fn loopback_answer(service: &str, instance: &str, port: u16, ttl: u32) -> Vec<u8> {
        answer(service, instance, HOST, Ipv4Addr::LOCALHOST, port, ttl)
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
        let svc = OscService::start("Orbiters - N4N0 test", &[], Network::Loopback { vrchat_port: 9 }, false, move |e, at| sink.lock().unwrap().push((e, at))).unwrap();
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
        let svc = OscService::start("Orbiters - N4N0 test", &[], Network::Loopback { vrchat_port: 9 }, false, |_, _| {}).unwrap();
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

    /// A stand-in Quest on the loopback range: its own address (127.0.0.2 and so on stand for the devices of a Wi-Fi).
    fn device(last: u8) -> UdpSocket {
        let socket = UdpSocket::bind((Ipv4Addr::new(127, 0, 0, last), 0)).unwrap();
        socket.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        socket
    }

    /// An HTTP request to the service's OSCQuery from `from`; its answer (empty when it was refused).
    fn ask_from(from: Ipv4Addr, to: SocketAddr, path: &str) -> String {
        let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, Some(socket2::Protocol::TCP)).unwrap();
        socket.bind(&SocketAddr::from((from, 0)).into()).unwrap();
        socket.connect_timeout(&to.into(), Duration::from_secs(1)).unwrap();
        let mut stream: TcpStream = socket.into();
        stream.set_read_timeout(Some(Duration::from_millis(800))).unwrap();
        stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).unwrap();
        let mut answer = String::new();
        let _ = stream.read_to_string(&mut answer);
        answer
    }

    #[test]
    fn on_the_network_only_the_chosen_vrchat_is_heard_and_messages_go_to_it() {
        // The phone at 127.0.0.1, its Quest at 127.0.0.2, another device at 127.0.0.3.
        let quest = device(2);
        let other = device(3);
        let quest_port = quest.local_addr().unwrap().port();
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let network = Network::Lan { ip: Ipv4Addr::LOCALHOST, vrchat: LanVrchat { ip: Some(Ipv4Addr::new(127, 0, 0, 2)), name: None, osc_port: quest_port } };
        let svc = OscService::start("Orbiters - N4N0 phone", &[], network, false, move |e, at| sink.lock().unwrap().push((e, at))).unwrap();
        assert_eq!(svc.local_osc_addr().ip(), std::net::IpAddr::V4(Ipv4Addr::LOCALHOST));
        let message = |addr: &str| {
            rosc::encoder::encode(&OscPacket::Message(OscMessage { addr: addr.into(), args: vec![OscType::Bool(true)] })).unwrap()
        };
        let to = ("127.0.0.1", svc.osc_port);
        other.send_to(&message("/avatar/parameters/FromElsewhere"), to).unwrap();
        quest.send_to(&message("/avatar/parameters/FromTheQuest"), to).unwrap();
        wait_for(&got, 1);
        std::thread::sleep(Duration::from_millis(100));
        let names: Vec<String> = got
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(e, _)| match e {
                OscEvent::Parameter { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["FromTheQuest"], "another device's datagram is dropped");
        // Messages go to the Quest's input port.
        svc.send("/avatar/parameters/Hoodie", vec![OscType::Bool(true)]).unwrap();
        let mut buf = [0u8; 512];
        let (n, from) = quest.recv_from(&mut buf).unwrap();
        assert_eq!(from.port(), svc.osc_port, "from the service's own socket");
        let (_, packet) = rosc::decoder::decode_udp(&buf[..n]).unwrap();
        assert!(matches!(packet, OscPacket::Message(m) if m.addr == "/avatar/parameters/Hoodie"));
        assert_eq!(svc.vrchat_target(), Some(SocketAddr::from(([127, 0, 0, 2], quest_port))));
        // Its OSCQuery answers the Quest, with the phone's address as where OSC goes, and nobody else.
        let query = SocketAddr::from((Ipv4Addr::LOCALHOST, svc.query_port));
        let answer = ask_from(Ipv4Addr::new(127, 0, 0, 2), query, "/?HOST_INFO");
        assert!(answer.contains("\"OSC_IP\":\"127.0.0.1\"") && answer.contains(&svc.osc_port.to_string()), "{answer}");
        assert!(ask_from(Ipv4Addr::new(127, 0, 0, 3), query, "/?HOST_INFO").is_empty(), "another device is not answered");
        // Stopping wakes the threads bound on the network address: it ends at once.
        let started = Instant::now();
        drop(svc);
        assert!(started.elapsed() < Duration::from_millis(900), "{:?}", started.elapsed());
    }

    #[test]
    fn on_the_network_nothing_is_heard_nor_sent_until_a_vrchat_is_chosen() {
        let quest = device(4);
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let network = Network::Lan { ip: Ipv4Addr::LOCALHOST, vrchat: LanVrchat { osc_port: VRCHAT_OSC_PORT, ..Default::default() } };
        let svc = OscService::start("Orbiters - N4N0 phone", &[], network, false, move |e, at| sink.lock().unwrap().push((e, at))).unwrap();
        assert_eq!(svc.vrchat_target(), None);
        let err = svc.send("/avatar/parameters/Hoodie", vec![OscType::Bool(true)]).unwrap_err();
        assert!(err.to_string().contains("not found on the network"), "{err}");
        let bytes = rosc::encoder::encode(&OscPacket::Message(OscMessage { addr: "/avatar/parameters/Early".into(), args: vec![OscType::Int(1)] })).unwrap();
        quest.send_to(&bytes, ("127.0.0.1", svc.osc_port)).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(got.lock().unwrap().is_empty(), "no VRChat chosen: nothing is heard");
        // Chosen (entered, or among several): heard, and sent to.
        let quest_port = quest.local_addr().unwrap().port();
        svc.choose_vrchat(Ipv4Addr::new(127, 0, 0, 4), quest_port);
        quest.send_to(&bytes, ("127.0.0.1", svc.osc_port)).unwrap();
        wait_for(&got, 1);
        assert_eq!(got.lock().unwrap().len(), 1);
        svc.send_bundle(vec![("/avatar/parameters/A".into(), vec![OscType::Int(1)]), ("/avatar/parameters/B".into(), vec![OscType::Int(2)])]).unwrap();
        let mut buf = [0u8; 512];
        let (n, _) = quest.recv_from(&mut buf).unwrap();
        assert!(matches!(rosc::decoder::decode_udp(&buf[..n]).unwrap().1, OscPacket::Bundle(b) if b.content.len() == 2));
        // On this machine (the desktop's), there is nothing to choose.
        let local = OscService::start("Orbiters - N4N0 test", &[], Network::Loopback { vrchat_port: 9 }, false, |_, _| {}).unwrap();
        local.choose_vrchat(Ipv4Addr::new(127, 0, 0, 4), 1);
        assert_eq!(local.vrchat_target(), Some(SocketAddr::from(([127, 0, 0, 1], 9))));
    }

    #[test]
    fn a_phone_s_answers_give_its_own_name_and_wifi_address() {
        let host = "orbiters---n4n0-50123.local";
        let a = answer("_osc._udp.local", "Orbiters---N4N0-50123", host, Ipv4Addr::new(192, 168, 1, 20), 50123, 120);
        assert_eq!(&a[..12], &[0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 3]);
        // The A record last: the host's name, then its address.
        let mut tail = Vec::new();
        for label in host.split('.') {
            tail.push(label.len() as u8);
            tail.extend_from_slice(label.as_bytes());
        }
        tail.push(0);
        tail.extend_from_slice(&[0, 1, 0x80, 0x01, 0, 0, 0, 120, 0, 4, 192, 168, 1, 20]);
        assert!(a.ends_with(&tail));
        // The desktop's are as they were: at 127.0.0.1, under its own name.
        assert_eq!(loopback_answer("_osc._udp.local", "x", 1, 0), answer("_osc._udp.local", "x", HOST, Ipv4Addr::LOCALHOST, 1, 0));
        assert_eq!(host_info("Orbiters - N4N0", Ipv4Addr::new(192, 168, 1, 20), 50123)["OSC_IP"], "192.168.1.20");
    }

    #[test]
    fn a_vrchat_on_the_network_is_taken_by_its_address_or_its_name() {
        let (quest, phone_hotspot) = (Ipv4Addr::new(192, 168, 1, 30), Ipv4Addr::new(10, 0, 0, 5));
        // Discovery takes the first address of the local network it announces; an entered address only that one.
        assert_eq!(lan_vrchat_address(&[Ipv4Addr::new(8, 8, 8, 8), quest], 41000, None), Some(SocketAddr::from((quest, 41000))));
        assert_eq!(lan_vrchat_address(&[quest], 41000, Some(phone_hotspot)), None);
        assert_eq!(lan_vrchat_address(&[phone_hotspot, quest], 41000, Some(quest)), Some(SocketAddr::from((quest, 41000))));
        assert_eq!(lan_vrchat_address(&[Ipv4Addr::new(8, 8, 8, 8)], 41000, None), None, "beyond the local network");
        assert_eq!(lan_vrchat_address(&[Ipv4Addr::LOCALHOST], 41000, None), None, "the phone's own loopback is no Quest");
        // By name, with or without the service's domain.
        let full = "VRChat-Client-A1B2C3._oscjson._tcp.local.";
        assert!(named(full, None) && named(full, Some("VRChat-Client-A1B2C3")) && named(full, Some("vrchat-client-a1b2c3._oscjson._tcp.local.")));
        assert!(!named(full, Some("VRChat-Client-FFFFFF")));
        for (_, ip) in lan_addresses() {
            assert!(on_lan(ip) && !ip.is_loopback(), "{ip}");
        }
    }

    #[test]
    fn the_latest_values_forget_an_avatar_s_parameters_with_it_and_are_bounded() {
        let mut latest = Latest::default();
        latest.record("/avatar/parameters/Old".into(), vec![OscValue::Bool(true)], 5);
        latest.record("/usercamera/Mode".into(), vec![OscValue::Int(1)], 5);
        latest.record(AVATAR_CHANGE.into(), vec![OscValue::Text("avtr_x".into())], 6);
        assert!(!latest.contains_key("/avatar/parameters/Old"));
        assert_eq!(latest.first("/usercamera/Mode"), Some(OscValue::Int(1)));
        latest.record("/usercamera/Mode".into(), vec![OscValue::Int(2)], 7);
        assert_eq!(latest["/usercamera/Mode"], (vec![OscValue::Int(2)], 7));
        for i in 0..Latest::MOST + 5 {
            latest.record(format!("/avatar/parameters/P{i}"), vec![OscValue::Int(1)], 8);
        }
        assert_eq!(latest.len(), Latest::MOST);
    }
}
