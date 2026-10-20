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
use std::time::Duration;

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

/// Turns one OSC packet (bundles included) into events.
pub fn events_from_packet(packet: &OscPacket, out: &mut Vec<OscEvent>) {
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
        OscPacket::Bundle(b) => {
            for p in &b.content {
                events_from_packet(p, out);
            }
        }
    }
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

/// `GET path` over HTTP/1.1 (identity or chunked body), for OSCQuery trees.
pub fn http_get(addr: SocketAddr, path: &str, timeout: Duration) -> anyhow::Result<String> {
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    // One write: `write!` would send every piece in its own segment.
    stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nAccept: application/json\r\nConnection: close\r\n\r\n").as_bytes())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
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
        let udp = UdpSocket::bind("127.0.0.1:0")?;
        udp.set_read_timeout(Some(Duration::from_millis(200)))?;
        let osc_port = udp.local_addr()?.port();
        let http = TcpListener::bind("127.0.0.1:0")?;
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
                    let Ok((n, _)) = udp.recv_from(&mut buf) else { continue };
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
                while !stop.load(Ordering::Relaxed) {
                    match http.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            serve_http(stream, &tree, &host);
                        }
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

    fn advertise(name: &str, osc_port: u16, query_port: u16) -> anyhow::Result<mdns_sd::ServiceDaemon> {
        let daemon = mdns_sd::ServiceDaemon::new()?;
        let instance = format!("{}-{}", name.replace(' ', "-"), osc_port);
        let host = "orbiters-desktop.local.";
        let props: &[(&str, &str)] = &[("txtvers", "1")];
        daemon.register(mdns_sd::ServiceInfo::new("_oscjson._tcp.local.", &instance, host, "127.0.0.1", query_port, props)?)?;
        daemon.register(mdns_sd::ServiceInfo::new("_osc._udp.local.", &instance, host, "127.0.0.1", osc_port, props)?)?;
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

    /// Current values VRChat reports through its OSCQuery tree: the avatar (`/avatar/change`) and every avatar
    /// parameter, by address.
    pub fn vrchat_values(&self, timeout: Duration) -> anyhow::Result<Vec<(String, OscValue)>> {
        let addr = self.vrchat_query().ok_or_else(|| anyhow::anyhow!("VRChat's OSCQuery service was not found"))?;
        let tree: serde_json::Value = serde_json::from_str(&http_get(addr, "/avatar", timeout)?)?;
        let mut out = Vec::new();
        tree_values(&tree, &mut out);
        Ok(out)
    }

    /// Sends one message to VRChat's input port.
    pub fn send(&self, address: &str, args: Vec<OscType>) -> anyhow::Result<()> {
        let packet = OscPacket::Message(OscMessage { addr: address.to_string(), args });
        let bytes = rosc::encoder::encode(&packet)?;
        self.sender.send_to(&bytes, self.vrchat)?;
        Ok(())
    }
}

/// Keeps `found` pointing at VRChat's OSCQuery service (`VRChat-Client-*`) while mDNS announces it.
fn watch_vrchat(events: mdns_sd::Receiver<mdns_sd::ServiceEvent>, stop: &AtomicBool, found: &Mutex<Option<SocketAddr>>) {
    let mut current: Option<String> = None;
    while !stop.load(Ordering::Relaxed) {
        match events.recv_timeout(Duration::from_millis(250)) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) if info.get_fullname().starts_with("VRChat-Client-") => {
                // VRChat serves OSCQuery on the loopback interface; prefer it over the announced LAN address.
                let ip = if info.get_addresses_v4().iter().any(|a| a.is_loopback()) || info.get_addresses_v4().is_empty() {
                    Ipv4Addr::LOCALHOST
                } else {
                    *info.get_addresses_v4().iter().next().expect("one address")
                };
                *found.lock().unwrap() = Some(SocketAddr::from((ip, info.get_port())));
                current = Some(info.get_fullname().to_string());
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
        let svc = OscService::start("Orbiters desktop test", &[], 9, false, |_, _| {}).unwrap();
        assert!(svc.vrchat_values(Duration::from_secs(1)).is_err());
        svc.set_vrchat_query(Some(addr));
        let mut values = svc.vrchat_values(Duration::from_secs(2)).unwrap();
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
}
