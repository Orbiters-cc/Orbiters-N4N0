//! A stand-in for VRChat on another device of the network, for trying the app's LAN mode without a headset:
//! `cargo run -p orbiters-core --example stand_in_vrchat [--pc]`. It announces itself over mDNS as VRChat does
//! (`VRChat-Client-*`, `_oscjson._tcp` and `_osc._udp`), answers OSCQuery on this computer's network address, prints
//! every OSC message it gets, and sends avatar parameters to the OSC services it finds, as VRChat sends to what it
//! discovers. With `--pc` its OSCQuery answers nothing over the network, as VRChat on a PC may.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    let pc = std::env::args().any(|a| a == "--pc");
    let ip = orbiters_core::lan::this_address(None).ok_or_else(|| anyhow::anyhow!("on no local network"))?;
    let input = UdpSocket::bind((ip, 0))?;
    let osc_port = input.local_addr()?.port();
    let http = TcpListener::bind((ip, 0))?;
    let query_port = http.local_addr()?.port();
    let name = format!("VRChat-Client-{:06X}", std::process::id() & 0xFF_FFFF);
    println!("{name} at {ip}: OSC input {osc_port}, OSCQuery {query_port}{}", if pc { " (not answering)" } else { "" });

    let daemon = mdns_sd::ServiceDaemon::new()?;
    let host = "stand-in-quest.local.";
    for (service, port) in [("_oscjson._tcp.local.", query_port), ("_osc._udp.local.", osc_port)] {
        daemon.register(mdns_sd::ServiceInfo::new(service, &name, host, IpAddr::V4(ip), port, None::<HashMap<String, String>>)?)?;
    }

    if !pc {
        let name = name.clone();
        std::thread::spawn(move || {
            for stream in http.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let body = if request.starts_with("GET /?HOST_INFO") {
                    format!(r#"{{"NAME":"{name}","OSC_IP":"{ip}","OSC_PORT":{osc_port},"OSC_TRANSPORT":"UDP","EXTENSIONS":{{"VALUE":true}}}}"#)
                } else {
                    r#"{"FULL_PATH":"/","ACCESS":0,"CONTENTS":{"avatar":{"FULL_PATH":"/avatar","ACCESS":0,"CONTENTS":{"change":{"FULL_PATH":"/avatar/change","ACCESS":1,"TYPE":"s","VALUE":["avtr_stand_in"]}}}}}"#.to_string()
                };
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
    }

    // What the app sends.
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 65536];
        while let Ok((n, from)) = input.recv_from(&mut buf) {
            if let Ok((_, packet)) = rosc::decoder::decode_udp(&buf[..n]) {
                println!("from {from}: {packet:?}");
            }
        }
    });

    // The services it finds, each sent avatar parameters, as VRChat sends to those its discovery shows.
    let events = daemon.browse("_osc._udp.local.")?;
    let sender = UdpSocket::bind((ip, 0))?;
    let mut targets: Vec<SocketAddr> = Vec::new();
    let mut tick = 0u32;
    loop {
        if let Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) = events.recv_timeout(Duration::from_millis(1000)) {
            if !info.get_fullname().starts_with("VRChat-Client-") {
                for address in info.get_addresses_v4() {
                    let target = SocketAddr::from((address, info.get_port()));
                    if !targets.contains(&target) {
                        println!("found {} at {target}", info.get_fullname());
                        targets.push(target);
                    }
                }
            }
        }
        tick += 1;
        for target in &targets {
            let packet = rosc::OscPacket::Message(rosc::OscMessage { addr: "/avatar/parameters/VelocityMagnitude".into(), args: vec![rosc::OscType::Float((tick % 3) as f32)] });
            let _ = sender.send_to(&rosc::encoder::encode(&packet)?, target);
            if tick == 1 {
                let packet = rosc::OscPacket::Message(rosc::OscMessage { addr: "/avatar/change".into(), args: vec![rosc::OscType::String("avtr_stand_in".into())] });
                let _ = sender.send_to(&rosc::encoder::encode(&packet)?, target);
            }
        }
    }
}
