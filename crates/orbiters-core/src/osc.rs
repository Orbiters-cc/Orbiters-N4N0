//! OSC with VRChat. Orbiters desktop receives avatar parameters through OSCQuery: it advertises a small OSC
//! server over mDNS (`_osc._udp`, `_oscjson._tcp`), VRChat discovers it and sends what its tree asks for:
//! `/avatar` brings `/avatar/change` and every avatar parameter (face tracking included, whichever app writes
//! it), `/tracking/vrsystem` the headset and wrist poses once the person allows it in VRChat's Tracking & IK
//! menu. That way it coexists with VRCFT, VRCOSC and other OSC apps instead of fighting for port 9001.
//! VRChat sends values when they change; its own OSCQuery service tells the current ones ([`OscService::vrchat_values`]).
//! Messages to VRChat (camera, dolly, HUD parameters) go to its input port (9000 by default).

use rosc::{OscMessage, OscPacket, OscType};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
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
            OscType::Long(l) => OscValue::Int(*l as i32),
            OscType::String(s) => OscValue::Text(s.clone()),
            _ => return None,
        })
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

/// Turns one OSC packet (bundles included) into events, in the packet's order.
pub fn events_from_packet(packet: &OscPacket, out: &mut Vec<OscEvent>) {
    // A list instead of recursion: how deep bundles nest is up to the sender.
    let mut todo = vec![packet];
    while let Some(packet) = todo.pop() {
        match packet {
            OscPacket::Message(m) => {
                if let Some(name) = m.addr.strip_prefix(PARAMETERS_PREFIX) {
                    if let Some(value) = m.args.first().and_then(OscValue::from_osc) {
                        out.push(OscEvent::Parameter { name: name.to_string(), value });
                    }
                } else if m.addr == AVATAR_CHANGE {
                    if let Some(OscValue::Text(id)) = m.args.first().and_then(OscValue::from_osc) {
                        out.push(OscEvent::AvatarChanged { id });
                    }
                } else {
                    let values: Vec<OscValue> = m.args.iter().filter_map(OscValue::from_osc).collect();
                    if !values.is_empty() {
                        out.push(OscEvent::Values { address: m.addr.clone(), values });
                    }
                }
            }
            OscPacket::Bundle(b) => todo.extend(b.content.iter().rev()),
        }
    }
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
            serde_json::json!({ "FULL_PATH": format!("{PARAMETERS_PREFIX}{p}"), "TYPE": "T", "ACCESS": 2, "DESCRIPTION": "Orbiters desktop" }),
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

fn serve_http(mut stream: TcpStream, tree: &str, host: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut buf = [0u8; 2048];
    let n = stream.read(&mut buf).unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    let path = request.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/");
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
    let deadline = Instant::now() + timeout;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_write_timeout(Some(timeout))?;
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
    anyhow::ensure!(head.starts_with("http/1.") && head.split_whitespace().nth(1) == Some("200"), "HTTP status {}", head.lines().next().unwrap_or(""));
    let body = &raw[split + 4..];
    let body = if head.contains("transfer-encoding: chunked") { dechunk(body)? } else { body.to_vec() };
    Ok(String::from_utf8(body)?)
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

/// Current values of an OSCQuery tree: every node with a `VALUE`, by full path (first value of each node).
pub fn tree_values(tree: &serde_json::Value, out: &mut Vec<(String, OscValue)>) {
    if let (Some(path), Some(kind), Some(value)) = (tree["FULL_PATH"].as_str(), tree["TYPE"].as_str(), tree["VALUE"].get(0)) {
        let v = match kind.chars().next() {
            Some('f') | Some('d') => value.as_f64().map(|f| OscValue::Float(f as f32)),
            Some('i') | Some('h') => value.as_i64().map(|i| OscValue::Int(i as i32)),
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
    /// VRChat's own OSCQuery service, found over mDNS.
    vrchat_query: Arc<Mutex<Option<SocketAddr>>>,
    threads: Vec<JoinHandle<()>>,
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
        // VRChat finds the OSCQuery service at an address mDNS announces (this machine's own, see `advertise`):
        // advertised, it listens on every interface but takes connections from this machine only. VRChat sends OSC
        // where HOST_INFO says, 127.0.0.1, and UDP has no handshake to tell a spoofed source: OSC listens on the
        // loopback interface only.
        let udp = UdpSocket::bind("127.0.0.1:0")?;
        udp.set_read_timeout(Some(Duration::from_millis(200)))?;
        let osc_port = udp.local_addr()?.port();
        let http = TcpListener::bind(if advertise { "0.0.0.0:0" } else { "127.0.0.1:0" })?;
        http.set_nonblocking(true)?;
        let query_port = http.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let on_event = Arc::new(on_event);
        let vrchat_query = Arc::new(Mutex::new(None));
        let mut threads = Vec::new();

        {
            let stop = stop.clone();
            let on_event = on_event.clone();
            let udp = udp.try_clone()?;
            threads.push(std::thread::Builder::new().name("osc-receive".into()).spawn(move || {
                let mut buf = vec![0u8; 65536];
                let mut events = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    let Ok((n, from)) = udp.recv_from(&mut buf) else { continue };
                    if !from.ip().is_loopback() || !nesting_within_limit(&buf[..n]) {
                        continue;
                    }
                    let at = crate::clock::now_ns();
                    if let Ok((_, packet)) = rosc::decoder::decode_udp(&buf[..n]) {
                        events_from_packet(&packet, &mut events);
                        for e in events.drain(..) {
                            on_event(e, at);
                        }
                    }
                }
            })?);
        }
        {
            let stop = stop.clone();
            let tree = query_tree(parameters).to_string();
            let host = host_info(name, osc_port).to_string();
            threads.push(std::thread::Builder::new().name("oscquery-http".into()).spawn(move || {
                let mut local = LocalAddrs::default();
                while !stop.load(Ordering::Relaxed) {
                    match http.accept() {
                        Ok((stream, from)) if local.contains(from.ip()) => {
                            let _ = stream.set_nonblocking(false);
                            serve_http(stream, &tree, &host);
                        }
                        Ok(_) => {}
                        Err(_) => std::thread::sleep(Duration::from_millis(50)),
                    }
                }
            })?);
        }

        let mdns = if advertise {
            match Self::advertise(name, osc_port, query_port) {
                Ok(d) => {
                    match d.browse("_oscjson._tcp.local.") {
                        Ok(events) => {
                            let (stop, found) = (stop.clone(), vrchat_query.clone());
                            threads.push(std::thread::Builder::new().name("oscquery-browse".into()).spawn(move || watch_vrchat(events, &stop, &found))?);
                        }
                        Err(e) => eprintln!("OSCQuery browse failed: {e}"),
                    }
                    Some(d)
                }
                Err(e) => {
                    eprintln!("OSCQuery advertisement failed: {e}");
                    None
                }
            }
        } else {
            None
        };

        Ok(Self { sender: udp, osc_port, query_port, vrchat: SocketAddr::from(([127, 0, 0, 1], vrchat_port)), stop, mdns, vrchat_query, threads })
    }

    /// Announces the services with one of this machine's own addresses, the one of its default route (all of them
    /// when it has none). mdns-sd announces an address only on the interfaces of its subnet: 127.0.0.1 alone was
    /// never announced (Windows lists no IPv4 loopback interface). VRChat makes one client per address it finds, and
    /// every client gets every message: one address keeps them single. Then asks for the services, so that a
    /// VRChat already running finds them too (see [`ask_for_services`]).
    fn advertise(name: &str, osc_port: u16, query_port: u16) -> anyhow::Result<mdns_sd::ServiceDaemon> {
        let daemon = mdns_sd::ServiceDaemon::new()?;
        let instance = format!("{}-{}", name.replace(' ', "-"), osc_port);
        let host = "orbiters-desktop.local.";
        let props: &[(&str, &str)] = &[("txtvers", "1")];
        let primary = primary_ipv4();
        let info = |ty: &str, port: u16| -> anyhow::Result<mdns_sd::ServiceInfo> {
            Ok(match primary {
                Some(ip) => mdns_sd::ServiceInfo::new(ty, &instance, host, std::net::IpAddr::V4(ip), port, props)?,
                None => mdns_sd::ServiceInfo::new(ty, &instance, host, "", port, props)?.enable_addr_auto(),
            })
        };
        daemon.register(info("_oscjson._tcp.local.", query_port)?)?;
        daemon.register(info("_osc._udp.local.", osc_port)?)?;
        if let Some(ip) = primary {
            std::thread::Builder::new().name("oscquery-ask".into()).spawn(move || {
                // After mdns-sd's probes and its two announcements (about two seconds; see `ask_for_services`
                // on why more than a second after the last), and once more in case.
                for wait in [3500, 3500] {
                    std::thread::sleep(Duration::from_millis(wait));
                    if let Err(e) = ask_for_services(ip) {
                        eprintln!("OSCQuery services query failed: {e}");
                    }
                }
            })?;
        }
        Ok(daemon)
    }

    pub fn advertising(&self) -> bool {
        self.mdns.is_some()
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

/// VRChat takes a service only from an mDNS response with the PTR record as answer and the SRV and A records as
/// additional records (vrc-oscquery-lib's MeaModDiscovery), the way answers to queries come; it ignores
/// announcements (every record an answer, RFC 6762) and queries only when it starts. A query for the OSCQuery
/// service type from the mDNS port, on `ip`'s interface, makes every responder there (mdns-sd among them) multicast
/// such an answer, which a running VRChat hears. Only that type, and over a second after mdns-sd last sent the
/// address: it sends a record at most once a second, so an answer right after another (or after its announcements)
/// comes without the A record, and VRChat drops it ("no SRV Records found"). mdns-sd's own browsing would not do
/// either: it lists its own records as known answers, which suppresses them.
fn ask_for_services(ip: std::net::Ipv4Addr) -> std::io::Result<()> {
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddr::from(([0, 0, 0, 0], 5353)).into())?;
    socket.set_multicast_if_v4(&ip)?;
    // ID 0, flags 0, one question, no records.
    let mut query = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in ["_oscjson", "_tcp", "local"] {
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    // End of the name, type PTR, class IN (multicast answer).
    query.extend_from_slice(&[0, 0, 12, 0, 1]);
    socket.send_to(&query, &SocketAddr::from(([224, 0, 0, 251], 5353)).into())?;
    Ok(())
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
    let ip = if addresses.iter().any(|a| a.is_loopback()) {
        Ipv4Addr::LOCALHOST
    } else {
        *addresses.iter().find(|a| local.contains(std::net::IpAddr::V4(**a)))?
    };
    Some(SocketAddr::from((ip, port)))
}

/// Keeps `found` pointing at this machine's VRChat OSCQuery service (`VRChat-Client-*`) while mDNS announces it.
fn watch_vrchat(events: mdns_sd::Receiver<mdns_sd::ServiceEvent>, stop: &AtomicBool, found: &Mutex<Option<SocketAddr>>) {
    let mut current: Option<String> = None;
    let mut local = LocalAddrs::default();
    while !stop.load(Ordering::Relaxed) {
        match events.recv_timeout(Duration::from_millis(250)) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) if info.get_fullname().starts_with("VRChat-Client-") => {
                let addresses: Vec<Ipv4Addr> = info.get_addresses_v4().into_iter().collect();
                if let Some(addr) = vrchat_address(&addresses, info.get_port(), &mut local) {
                    *found.lock().unwrap() = Some(addr);
                    current = Some(info.get_fullname().to_string());
                }
            }
            Ok(mdns_sd::ServiceEvent::ServiceRemoved(_, name)) if current.as_deref() == Some(name.as_str()) => {
                *found.lock().unwrap() = None;
                current = None;
            }
            Ok(_) | Err(mdns_sd::RecvTimeoutError::Timeout) => {}
            Err(_) => return,
        }
    }
}

impl Drop for OscService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(d) = self.mdns.take() {
            let _ = d.shutdown();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
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
        let svc = OscService::start("Orbiters desktop test", &["OrbitersRecord".into()], 9, false, move |e, at| sink.lock().unwrap().push((e, at))).unwrap();
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
        let svc = OscService::start("Orbiters desktop test", &[], 9, false, move |e, at| sink.lock().unwrap().push((e, at))).unwrap();
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

    #[test]
    fn osc_listens_on_the_loopback_interface_only() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let svc = OscService::start("Orbiters desktop test", &[], 9, false, move |e, at| sink.lock().unwrap().push((e, at))).unwrap();
        assert!(svc.sender.local_addr().unwrap().ip().is_loopback());
        let Some(ip) = primary_ipv4() else { return };
        // Sent to this machine's LAN address: nothing listens there.
        let client = UdpSocket::bind((ip, 0)).unwrap();
        let msg = rosc::encoder::encode(&OscPacket::Message(OscMessage { addr: "/avatar/parameters/OrbitersRecord".into(), args: vec![OscType::Bool(true)] })).unwrap();
        let _ = client.send_to(&msg, (ip, svc.osc_port));
        std::thread::sleep(Duration::from_millis(300));
        assert!(got.lock().unwrap().is_empty());
    }
}
