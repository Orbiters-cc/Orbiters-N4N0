//! OSC with VRChat. Orbiters desktop receives avatar parameters through OSCQuery: it advertises a small OSC
//! server over mDNS (`_osc._udp`, `_oscjson._tcp`), VRChat discovers it and sends the parameters it asks for.
//! That way it coexists with VRCFT, VRCOSC and other OSC apps instead of fighting for port 9001.
//! Messages to VRChat (camera, dolly, HUD parameters) go to its input port (9000 by default).

use rosc::{OscMessage, OscPacket, OscType};
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Serialize)]
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
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OscEvent {
    Parameter { name: String, value: OscValue },
    AvatarChanged { id: String },
}

fn value_of(args: &[OscType]) -> Option<OscValue> {
    Some(match args.first()? {
        OscType::Bool(b) => OscValue::Bool(*b),
        OscType::Int(i) => OscValue::Int(*i),
        OscType::Float(f) => OscValue::Float(*f),
        OscType::String(s) => OscValue::Text(s.clone()),
        _ => return None,
    })
}

/// Turns one OSC packet (bundles included) into events.
pub fn events_from_packet(packet: &OscPacket, out: &mut Vec<OscEvent>) {
    match packet {
        OscPacket::Message(m) => {
            if let Some(name) = m.addr.strip_prefix("/avatar/parameters/") {
                if let Some(value) = value_of(&m.args) {
                    out.push(OscEvent::Parameter { name: name.to_string(), value });
                }
            } else if m.addr == "/avatar/change" {
                if let Some(OscValue::Text(id)) = value_of(&m.args) {
                    out.push(OscEvent::AvatarChanged { id });
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

/// OSCQuery node tree for the parameters this app listens to.
pub fn query_tree(parameters: &[String]) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    for p in parameters {
        params.insert(
            p.clone(),
            serde_json::json!({ "FULL_PATH": format!("/avatar/parameters/{p}"), "TYPE": "T", "ACCESS": 2, "DESCRIPTION": "Orbiters desktop" }),
        );
    }
    serde_json::json!({
        "DESCRIPTION": "root node", "FULL_PATH": "/", "ACCESS": 0,
        "CONTENTS": { "avatar": { "FULL_PATH": "/avatar", "ACCESS": 0, "CONTENTS": {
            "change": { "FULL_PATH": "/avatar/change", "TYPE": "s", "ACCESS": 2 },
            "parameters": { "FULL_PATH": "/avatar/parameters", "ACCESS": 0, "CONTENTS": params }
        } } }
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
    } else if path == "/" || path.starts_with("/?") || path.starts_with("/avatar") {
        ("200 OK", tree)
    } else {
        ("404 Not Found", "{}")
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = stream.write_all(response.as_bytes());
}

/// Running OSC + OSCQuery service. Dropping it stops everything.
pub struct OscService {
    sender: UdpSocket,
    pub osc_port: u16,
    pub query_port: u16,
    vrchat: SocketAddr,
    stop: Arc<AtomicBool>,
    mdns: Option<mdns_sd::ServiceDaemon>,
    threads: Vec<JoinHandle<()>>,
}

impl OscService {
    /// Starts listening. With `advertise`, registers the OSCQuery services over mDNS so VRChat finds it.
    pub fn start(
        name: &str,
        parameters: &[String],
        vrchat_port: u16,
        advertise: bool,
        on_event: impl Fn(OscEvent) + Send + Sync + 'static,
    ) -> anyhow::Result<Self> {
        let udp = UdpSocket::bind("127.0.0.1:0")?;
        udp.set_read_timeout(Some(Duration::from_millis(200)))?;
        let osc_port = udp.local_addr()?.port();
        let http = TcpListener::bind("127.0.0.1:0")?;
        http.set_nonblocking(true)?;
        let query_port = http.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let on_event = Arc::new(on_event);
        let mut threads = Vec::new();

        {
            let stop = stop.clone();
            let on_event = on_event.clone();
            let udp = udp.try_clone()?;
            threads.push(std::thread::Builder::new().name("osc-receive".into()).spawn(move || {
                let mut buf = vec![0u8; 65536];
                while !stop.load(Ordering::Relaxed) {
                    let Ok((n, _)) = udp.recv_from(&mut buf) else { continue };
                    if let Ok((_, packet)) = rosc::decoder::decode_udp(&buf[..n]) {
                        let mut events = Vec::new();
                        events_from_packet(&packet, &mut events);
                        for e in events {
                            on_event(e);
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
                Ok(d) => Some(d),
                Err(e) => {
                    eprintln!("OSCQuery advertisement failed: {e}");
                    None
                }
            }
        } else {
            None
        };

        Ok(Self { sender: udp, osc_port, query_port, vrchat: SocketAddr::from(([127, 0, 0, 1], vrchat_port)), stop, mdns, threads })
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

    /// Sends one message to VRChat's input port.
    pub fn send(&self, address: &str, args: Vec<OscType>) -> anyhow::Result<()> {
        let packet = OscPacket::Message(OscMessage { addr: address.to_string(), args });
        let bytes = rosc::encoder::encode(&packet)?;
        self.sender.send_to(&bytes, self.vrchat)?;
        Ok(())
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
    use std::sync::Mutex;

    #[test]
    fn receives_parameters_and_serves_tree() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        let svc = OscService::start("Orbiters desktop test", &["OrbitersRecord".into()], 9, false, move |e| sink.lock().unwrap().push(e)).unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let msg = OscPacket::Message(OscMessage { addr: "/avatar/parameters/OrbitersRecord".into(), args: vec![OscType::Bool(true)] });
        client.send_to(&rosc::encoder::encode(&msg).unwrap(), ("127.0.0.1", svc.osc_port)).unwrap();
        for _ in 0..50 {
            if !got.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(got.lock().unwrap()[0], OscEvent::Parameter { name: "OrbitersRecord".into(), value: OscValue::Bool(true) });

        let mut s = TcpStream::connect(("127.0.0.1", svc.query_port)).unwrap();
        s.write_all(b"GET /?HOST_INFO HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("\"OSC_PORT\"") && resp.contains(&svc.osc_port.to_string()));
        let tree = query_tree(&["OrbitersRecord".into()]);
        assert_eq!(tree["CONTENTS"]["avatar"]["CONTENTS"]["parameters"]["CONTENTS"]["OrbitersRecord"]["ACCESS"], 2);
    }
}
