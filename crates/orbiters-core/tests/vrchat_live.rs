//! Listens like Orbiters (a second, advertised service) for 30 s and prints what a running VRChat sends it: run by
//! hand while in VRChat, moving and opening the Action Menu:
//! cargo test -p orbiters-core --test vrchat_live -- --ignored --nocapture

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use orbiters_core::osc::{OscEvent, OscService};

#[test]
#[ignore]
fn vrchat_sends_parameters_and_poses() {
    let seen: Arc<Mutex<BTreeMap<String, (usize, String)>>> = Arc::default();
    let s = seen.clone();
    let service = OscService::start("Orbiters probe", &[], 9000, true, move |event, _| {
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
