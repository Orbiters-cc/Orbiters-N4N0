//! Listens like Orbiters (a second, advertised service) for 30 s and prints what a running VRChat sends it: run by
//! hand while in VRChat, moving and opening the Action Menu:
//! cargo test -p orbiters-core --test vrchat_live -- --ignored --nocapture

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orbiters_core::osc::{vrchat_value, Network, OscEvent, OscService, OscValue};
use rosc::OscType;

#[test]
#[ignore]
fn vrchat_sends_parameters_and_poses() {
    let seen: Arc<Mutex<BTreeMap<String, (usize, String)>>> = Arc::default();
    let s = seen.clone();
    let service = OscService::start("Orbiters probe", &[], Network::Loopback { vrchat_port: 9000 }, true, move |event, _| {
        let (address, value) = match event {
            OscEvent::Parameter { name, value } => (format!("/avatar/parameters/{name}"), format!("{value:?}")),
            OscEvent::AvatarChanged { id } => ("/avatar/change".into(), id),
            OscEvent::Values { address, values } => (address, format!("{values:?}")),
        };
        let mut s = s.lock().unwrap();
        let e = s.entry(address).or_default();
        e.0 += 1;
        e.1 = value;
    })
    .expect("service");
    eprintln!("listening on OSC {} / OSCQuery {}", service.osc_port, service.query_port);
    let end = Instant::now() + Duration::from_secs(30);
    while Instant::now() < end {
        std::thread::sleep(Duration::from_secs(5));
        eprintln!("{} addresses so far", seen.lock().unwrap().len());
    }
    drop(service);
    for (address, (n, last)) in seen.lock().unwrap().iter() {
        eprintln!("{n:6} {address} {last}");
    }
}

/// Run by hand with VRChat open, wearing an avatar with the capture kit (its `OrbitersDepthFrames` does nothing while
/// `OrbitersRecord` is off) and N4N0's screen (`N4N0/Screen/Value15`):
/// cargo test -p orbiters-core --test vrchat_live vrchat_sends_to_loopback_and_takes_bundles -- --ignored --nocapture
/// The service's OSC socket listens on 127.0.0.1 alone, so VRChat's echo of a change reaching it shows that VRChat
/// sends to HOST_INFO's 127.0.0.1 (review DSK-13); changes sent as one bundle, read back from VRChat's OSCQuery, show
/// that VRChat takes bundles and applies every message in one.
#[test]
#[ignore]
fn vrchat_sends_to_loopback_and_takes_bundles() {
    let echoes: Arc<Mutex<Vec<(String, OscValue)>>> = Arc::default();
    let e = echoes.clone();
    let service = OscService::start("Orbiters probe", &[], Network::Loopback { vrchat_port: 9000 }, true, move |event, _| {
        if let OscEvent::Parameter { name, value } = event {
            e.lock().unwrap().push((name, value));
        }
    })
    .expect("service");
    assert!(service.local_osc_addr().ip().is_loopback(), "the OSC socket listens on 127.0.0.1 alone");
    // VRChat's own OSCQuery service, found by browsing; VRChat finds ours from the answers asked for after 3.5 and 7 s.
    let start = Instant::now();
    let query = loop {
        if let Some(q) = service.vrchat_query() {
            break q;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "VRChat's OSCQuery service was not found: is VRChat open, with OSC on?");
        std::thread::sleep(Duration::from_millis(200));
    };
    std::thread::sleep(Duration::from_secs(9).saturating_sub(start.elapsed()));
    let depth = "/avatar/parameters/OrbitersDepthFrames";
    let value15 = "/avatar/parameters/N4N0/Screen/Value15";
    let read = |path: &str| vrchat_value(query, path, Duration::from_secs(2)).expect("VRChat's OSCQuery").unwrap_or_else(|| panic!("the avatar has no {path}"));
    let echoed = |name: &str, want: &OscValue| {
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            if echoes.lock().unwrap().iter().any(|(n, v)| n == name && v == want) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    };
    let (depth_before, value15_before) = (read(depth), read(value15));
    let as_osc = |v: &OscValue| match v {
        OscValue::Bool(b) => OscType::Bool(*b),
        OscValue::Int(i) => OscType::Int(*i),
        OscValue::Float(f) => OscType::Float(*f),
        OscValue::Text(t) => OscType::String(t.clone()),
    };
    // 1. One message: applied, and echoed to the loopback socket.
    service.send(depth, vec![OscType::Bool(true)]).expect("send");
    let single_echo = echoed("OrbitersDepthFrames", &OscValue::Bool(true));
    let single_value = read(depth);
    // 2. Two messages in one bundle: both applied, both echoed.
    service.send_bundle(vec![(depth.into(), vec![OscType::Bool(false)]), (value15.into(), vec![OscType::Float(0.25)])]).expect("bundle");
    let bundle_echo = echoed("OrbitersDepthFrames", &OscValue::Bool(false)) && echoed("N4N0/Screen/Value15", &OscValue::Float(0.25));
    let bundle_values = (read(depth), read(value15));
    // Back as they were.
    service.send_bundle(vec![(depth.into(), vec![as_osc(&depth_before)]), (value15.into(), vec![as_osc(&value15_before)])]).expect("restore");
    std::thread::sleep(Duration::from_millis(300));
    eprintln!("before: {depth_before:?}, {value15_before:?}; one message: echoed {single_echo}, now {single_value:?}");
    eprintln!("bundle: echoed {bundle_echo}, now {bundle_values:?}; restored: {:?}, {:?}", read(depth), read(value15));
    assert!(single_echo, "VRChat's echo never reached the loopback OSC socket: it sends elsewhere than HOST_INFO's 127.0.0.1");
    assert_eq!(single_value, OscValue::Bool(true));
    assert!(bundle_echo, "no echo of the bundle's changes");
    assert_eq!(bundle_values, (OscValue::Bool(false), OscValue::Float(0.25)), "VRChat did not apply both messages of the bundle");
}
