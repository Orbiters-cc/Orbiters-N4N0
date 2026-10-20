//! Prints the OSCQuery services this machine announces, as mdns-sd resolves them (run by hand):
//! cargo test -p orbiters-core --test mdns_browse -- --ignored --nocapture

#[test]
#[ignore]
fn browse_oscquery_services() {
    let daemon = mdns_sd::ServiceDaemon::new().expect("daemon");
    for ty in ["_oscjson._tcp.local.", "_osc._udp.local."] {
        let rx = daemon.browse(ty).expect("browse");
        let end = std::time::Instant::now() + std::time::Duration::from_secs(4);
        while std::time::Instant::now() < end {
            if let Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
                eprintln!("{ty} {} port {} addresses {:?}", info.get_fullname(), info.get_port(), info.get_addresses());
            }
        }
    }
    for i in if_addrs::get_if_addrs().unwrap() {
        eprintln!("interface {} {:?} loopback {}", i.name, i.ip(), i.is_loopback());
    }
}
