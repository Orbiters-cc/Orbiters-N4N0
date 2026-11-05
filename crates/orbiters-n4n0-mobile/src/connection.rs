//! How the phone reaches VRChat on the Quest: which of the phone's networks the OSC services listen on, the Quest's
//! address when the person typed it, and the state of the link as the interface shows it. Pure logic, tested on any
//! computer; `app` runs it.

use std::net::Ipv4Addr;

/// How N4N0 stands with VRChat on the Quest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// The services start.
    #[default]
    Starting,
    /// The phone is on no local network (Wi-Fi off, or only mobile data).
    NoWifi,
    /// The services run and are announced; no VRChat answered yet.
    Searching,
    /// A VRChat is known (found, or its address typed) but has not answered or sent anything lately.
    Found,
    /// VRChat answers and sends what its avatar does.
    Connected,
    /// The services could not start (the error says why).
    Failed,
}

impl Phase {
    /// The short word for the status pill.
    pub fn word(self) -> &'static str {
        match self {
            Phase::Starting => "Starting",
            Phase::NoWifi => "No Wi-Fi",
            Phase::Searching => "Searching",
            Phase::Found => "Found",
            Phase::Connected => "Connected",
            Phase::Failed => "Stopped",
        }
    }

    /// The interface's tone for it: 0 neutral, 1 good, 2 waiting, 3 bad.
    pub fn tone(self) -> i32 {
        match self {
            Phase::Connected => 1,
            Phase::Searching | Phase::Found | Phase::Starting => 2,
            Phase::NoWifi | Phase::Failed => 3,
        }
    }
}

/// What the phase is made of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    /// The phone has an address on a local network.
    pub wifi: bool,
    /// The OSC services run.
    pub running: bool,
    /// They could not start.
    pub failed: bool,
    /// Where VRChat is, as discovery found it or the person typed it.
    pub target: bool,
    /// VRChat's OSCQuery service answered lately.
    pub answered: bool,
    /// VRChat sent something lately.
    pub heard: bool,
}

/// The phase those signals make.
pub fn phase(s: Signals) -> Phase {
    if !s.wifi {
        Phase::NoWifi
    } else if s.failed {
        Phase::Failed
    } else if !s.running {
        Phase::Starting
    } else if s.answered || s.heard {
        Phase::Connected
    } else if s.target {
        Phase::Found
    } else {
        Phase::Searching
    }
}

/// Interfaces of mobile data, tunnels and the like: never the local network a Quest is on, even when their address is
/// a private one (carriers hand out 10.x addresses, VPNs their own). Android's and iOS's names (`pdp_ip0` is iOS's mobile
/// data, `utun` its VPNs, `awdl` and `llw` its peer-to-peer Wi-Fi, `anpi` its link with a Mac).
const NOT_LOCAL: [&str; 13] = ["rmnet", "ccmni", "pdp", "clat", "v4-", "tun", "utun", "dummy", "ppp", "ipsec", "awdl", "llw", "anpi"];

/// How likely an interface is the Wi-Fi the Quest shares, lower first: `None` for one that is never it.
fn rank(name: &str) -> Option<u8> {
    let n = name.to_ascii_lowercase();
    if NOT_LOCAL.iter().any(|p| n.starts_with(p)) {
        return None;
    }
    Some(if n == "wlan0" || n == "en0" {
        // `en0` is an iPhone's Wi-Fi.
        0
    } else if n.starts_with("wlan") || n.starts_with("wi-fi") || n.starts_with("wifi") {
        1
    } else if n.starts_with("swlan") || n.starts_with("ap") || n.starts_with("softap") || n.starts_with("bridge") {
        // The phone's own hotspot, with the Quest on it (`bridge100` on an iPhone).
        2
    } else if n.starts_with("eth") || n.starts_with("ethernet") || n.starts_with("en") {
        3
    } else {
        4
    })
}

/// The network the OSC services listen on, among the phone's local addresses (`orbiters_core::osc::lan_addresses`):
/// the Wi-Fi's first, then the phone's hotspot, never mobile data.
pub fn choose_interface(list: &[(String, Ipv4Addr)]) -> Option<(String, Ipv4Addr)> {
    list.iter()
        .filter(|(_, ip)| !ip.is_loopback() && !ip.is_unspecified())
        .filter_map(|(name, ip)| rank(name).map(|r| (r, name, ip)))
        .min_by_key(|(r, _, _)| *r)
        .map(|(_, name, ip)| (name.clone(), *ip))
}

/// The Quest's address as the person typed it (`192.168.1.42`, spaces around it are fine): an address of the local
/// network, or why not.
pub fn parse_quest_address(text: &str) -> Result<Ipv4Addr, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Type the Quest's address, like 192.168.1.42.".into());
    }
    let ip: Ipv4Addr = text.parse().map_err(|_| format!("“{text}” is not an address like 192.168.1.42."))?;
    if ip.is_loopback() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() {
        return Err(format!("{ip} cannot be the Quest's address."));
    }
    if !(ip.is_private() || ip.is_link_local()) {
        return Err(format!("{ip} is not on a local network: the Quest's address starts with 192.168., 10. or 172.16-31."));
    }
    Ok(ip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[test]
    fn the_wifi_wins_and_mobile_data_never_counts() {
        let list = vec![("rmnet_data0".to_string(), ip("10.12.0.4")), ("wlan0".to_string(), ip("192.168.1.20"))];
        assert_eq!(choose_interface(&list), Some(("wlan0".into(), ip("192.168.1.20"))));
        // Mobile data alone: no local network.
        assert_eq!(choose_interface(&list[..1]), None);
        // The phone's hotspot when there is no Wi-Fi.
        let hotspot = vec![("ccmni1".to_string(), ip("10.1.1.1")), ("swlan0".to_string(), ip("192.168.43.1"))];
        assert_eq!(choose_interface(&hotspot), Some(("swlan0".into(), ip("192.168.43.1"))));
        // A Windows computer running the tests.
        let windows = vec![("Ethernet".to_string(), ip("192.168.0.9")), ("Wi-Fi".to_string(), ip("192.168.1.7"))];
        assert_eq!(choose_interface(&windows).unwrap().0, "Wi-Fi");
        assert_eq!(choose_interface(&[]), None);
    }

    #[test]
    fn an_iphone_s_interfaces() {
        let list = vec![
            ("pdp_ip0".to_string(), ip("10.40.2.1")),
            ("utun4".to_string(), ip("10.8.0.2")),
            ("llw0".to_string(), ip("169.254.1.1")),
            ("en0".to_string(), ip("192.168.1.20")),
        ];
        assert_eq!(choose_interface(&list), Some(("en0".into(), ip("192.168.1.20"))));
        // A VPN and mobile data, no Wi-Fi: no local network, whatever their addresses look like.
        assert_eq!(choose_interface(&list[..3]), None);
        // The hotspot, with the Quest on it.
        let hotspot = vec![("pdp_ip0".to_string(), ip("10.40.2.1")), ("bridge100".to_string(), ip("172.20.10.1"))];
        assert_eq!(choose_interface(&hotspot), Some(("bridge100".into(), ip("172.20.10.1"))));
    }

    #[test]
    fn typed_addresses() {
        assert_eq!(parse_quest_address(" 192.168.1.42 "), Ok(ip("192.168.1.42")));
        assert_eq!(parse_quest_address("10.0.0.5"), Ok(ip("10.0.0.5")));
        assert_eq!(parse_quest_address("169.254.3.4"), Ok(ip("169.254.3.4")));
        assert!(parse_quest_address("").is_err());
        assert!(parse_quest_address("quest.local").is_err());
        assert!(parse_quest_address("192.168.1.300").is_err());
        assert!(parse_quest_address("127.0.0.1").unwrap_err().contains("cannot"));
        assert!(parse_quest_address("8.8.8.8").unwrap_err().contains("not on a local network"));
    }

    #[test]
    fn phases() {
        let base = Signals { wifi: true, running: true, ..Default::default() };
        assert_eq!(phase(Signals::default()), Phase::NoWifi);
        assert_eq!(phase(Signals { wifi: true, ..Default::default() }), Phase::Starting);
        assert_eq!(phase(Signals { failed: true, ..base }), Phase::Failed);
        assert_eq!(phase(base), Phase::Searching);
        assert_eq!(phase(Signals { target: true, ..base }), Phase::Found);
        assert_eq!(phase(Signals { target: true, answered: true, ..base }), Phase::Connected);
        // VRChat sending is enough (a typed address: no OSCQuery service found).
        assert_eq!(phase(Signals { heard: true, ..base }), Phase::Connected);
        // No Wi-Fi beats everything else.
        assert_eq!(phase(Signals { wifi: false, ..Signals { target: true, answered: true, ..base } }), Phase::NoWifi);
        assert_eq!(Phase::Connected.tone(), 1);
        assert_eq!(Phase::NoWifi.word(), "No Wi-Fi");
    }
}
