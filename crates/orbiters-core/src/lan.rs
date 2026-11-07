//! VRChat on other devices of the local network: a Quest, a Pico, an Android phone or another PC, for a computer VRChat
//! does not run on. [`LanBrowser`] lists every VRChat its mDNS announcements show (`VRChat-Client-*`, `_oscjson._tcp`),
//! asks each one who it is, and [`choose`] says which one to talk with: the address the person typed, the one they
//! chose, or the only one there is. The OSC service itself is [`crate::osc::OscService`] in [`crate::osc::Network::Lan`].
//!
//! A standalone headset or phone serves OSCQuery on its network address, so it answers. VRChat on a PC announces itself
//! too but may keep OSCQuery on its loopback interface: it is listed all the same, without an input port from it, and
//! sends here only when its launch options say so ([`launch_option`]).

use crate::osc::{http_get, on_lan, primary_ipv4};
use orbiters_i18n::tr;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SERVICE: &str = "_oscjson._tcp.local.";
/// How often each VRChat listed is asked whether it still answers, and how long it has to.
const ASK_EVERY: Duration = Duration::from_secs(5);
const ASK_WAIT: Duration = Duration::from_millis(800);
/// A VRChat that answered and stopped goes after this many questions without an answer in a row.
const MISSES: u32 = 3;

/// The port VRChat sends to by default, which the service asks for first, so that the launch option a PC needs is
/// VRChat's own default but for the address.
pub const OSC_PORT: u16 = 9001;
/// VRChat's input port by default.
pub const VRCHAT_PORT: u16 = crate::osc::VRCHAT_OSC_PORT;

/// A VRChat on the network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundVrchat {
    /// Its service's name (`VRChat-Client-ABCDEF`, new each time it starts).
    pub name: String,
    /// Its device's name on the network, when it has one (`Quest-3`, `DESKTOP-4F2K`).
    #[serde(default)]
    pub device: Option<String>,
    pub ip: Ipv4Addr,
    /// Where its OSCQuery service is, as announced.
    pub query_port: u16,
    /// Its input port, once its OSCQuery service answered over the network. `None`: it does not answer here (VRChat on a
    /// PC keeps it to itself).
    #[serde(default)]
    pub osc_port: Option<u16>,
}

impl FoundVrchat {
    /// What the interface calls it.
    pub fn label(&self) -> String {
        match &self.device {
            Some(device) => tr!("VRChat on {device}", device = device),
            None => tr!("VRChat at {address}", address = self.ip),
        }
    }
}

/// Which VRChat the person wants, as the settings keep it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Wanted {
    /// The address, when the person typed it: that one and no other, whether it was found or not.
    pub typed: Option<Ipv4Addr>,
    /// The VRChat last talked with: its service's name while it runs, its address for when it starts again under
    /// another name.
    pub name: String,
    pub ip: Option<Ipv4Addr>,
}

/// Which VRChat to talk with now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// None to choose from.
    Nobody,
    /// Several, and nothing says which: the person chooses.
    Ask,
    /// This one, listed.
    Found(FoundVrchat),
    /// The address typed, not listed (mDNS does not cross every router, and a PC's VRChat may not announce itself).
    Typed(Ipv4Addr),
    /// The one last talked with, not there now: it is waited for at its address (nothing else is heard meanwhile).
    Last(Ipv4Addr),
}

impl Choice {
    pub fn ip(&self) -> Option<Ipv4Addr> {
        match self {
            Choice::Nobody | Choice::Ask => None,
            Choice::Found(found) => Some(found.ip),
            Choice::Typed(ip) | Choice::Last(ip) => Some(*ip),
        }
    }

    /// Its input port, as far as is known.
    pub fn osc_port(&self) -> u16 {
        match self {
            Choice::Found(found) => found.osc_port.unwrap_or(VRCHAT_PORT),
            _ => VRCHAT_PORT,
        }
    }
}

/// The VRChat to talk with among those `found`: an address typed comes first; then the one last talked with (by its
/// name, or at its address: it has another name each time it starts); else the only one listed, whichever it is.
pub fn choose(wanted: &Wanted, found: &[FoundVrchat]) -> Choice {
    if let Some(ip) = wanted.typed {
        return found.iter().find(|f| f.ip == ip).cloned().map_or(Choice::Typed(ip), Choice::Found);
    }
    let last = found.iter().find(|f| !wanted.name.is_empty() && f.name == wanted.name).or_else(|| found.iter().find(|f| Some(f.ip) == wanted.ip));
    match (last, found) {
        (Some(found), _) => Choice::Found(found.clone()),
        (None, [only]) => Choice::Found(only.clone()),
        (None, []) => wanted.ip.map_or(Choice::Nobody, Choice::Last),
        (None, _) => Choice::Ask,
    }
}

/// An interface of this computer on the local network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub ip: Ipv4Addr,
    pub netmask: Ipv4Addr,
}

impl Interface {
    fn reaches(&self, ip: Ipv4Addr) -> bool {
        let mask = u32::from(self.netmask);
        u32::from(self.ip) & mask == u32::from(ip) & mask
    }
}

/// This computer's interfaces a VRChat could be on: of a private address, without what is no local network (macOS's
/// peer-to-peer links, its internet sharing and virtual machines' bridges, VPN tunnels).
pub fn interfaces() -> Vec<Interface> {
    const NOT_LOCAL: [&str; 9] = ["utun", "awdl", "llw", "bridge", "vmnet", "vnic", "docker", "veth", "tun"];
    if_addrs::get_if_addrs()
        .map(|list| {
            list.into_iter()
                .filter(|i| !NOT_LOCAL.iter().any(|skip| i.name.starts_with(skip)))
                .filter_map(|i| match i.addr {
                    if_addrs::IfAddr::V4(v4) if on_lan(v4.ip) && !v4.ip.is_loopback() => Some(Interface { name: i.name, ip: v4.ip, netmask: v4.netmask }),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The address the services listen on and announce: the interface on the same network as the VRChat, else the one of
/// the default route, else the first there is. `None`: this computer is on no local network.
pub fn local_address(peer: Option<Ipv4Addr>, interfaces: &[Interface], primary: Option<Ipv4Addr>) -> Option<Ipv4Addr> {
    let near = peer.and_then(|peer| interfaces.iter().find(|i| i.reaches(peer)));
    let main = primary.and_then(|ip| interfaces.iter().find(|i| i.ip == ip));
    near.or(main).or(interfaces.first()).map(|i| i.ip)
}

/// [`local_address`] on this computer as it is now.
pub fn this_address(peer: Option<Ipv4Addr>) -> Option<Ipv4Addr> {
    local_address(peer, &interfaces(), primary_ipv4())
}

/// The launch option that has VRChat on a PC send here (Steam: VRChat's properties, Launch options).
pub fn launch_option(ip: Ipv4Addr, osc_port: u16) -> String {
    format!("--osc={VRCHAT_PORT}:{ip}:{osc_port}")
}

/// The VRChat that answers at `addr`: its input port. An announcement of something that answers as another program is
/// not VRChat (`Some(None)` is a service that does not answer at all, which may still be VRChat on a PC).
fn ask(addr: SocketAddr) -> Asked {
    let Ok(text) = http_get(addr, "/?HOST_INFO", ASK_WAIT) else { return Asked::Silent };
    let Ok(info) = serde_json::from_str::<serde_json::Value>(&text) else { return Asked::NotVrchat };
    if !info["NAME"].as_str().is_some_and(|n| n.starts_with("VRChat")) {
        return Asked::NotVrchat;
    }
    match info["OSC_PORT"].as_u64().and_then(|p| u16::try_from(p).ok()).filter(|p| *p != 0) {
        Some(port) => Asked::Vrchat(port),
        None => Asked::NotVrchat,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    Vrchat(u16),
    Silent,
    NotVrchat,
}

/// The first private address among those announced: where a VRChat on the local network is.
fn lan_address(addresses: &[Ipv4Addr]) -> Option<Ipv4Addr> {
    addresses.iter().copied().find(|a| on_lan(*a) && !a.is_loopback())
}

/// The short name of a service's full name (`VRChat-Client-ABCDEF._oscjson._tcp.local.`).
fn short_name(full: &str) -> &str {
    full.split_once("._").map_or(full, |(name, _)| name)
}

/// A device's name from what it answers to, when it says more than VRChat's own service name.
fn device_name(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.');
    let host = host.strip_suffix(".local").unwrap_or(host);
    let useful = !host.is_empty() && !host.to_ascii_lowercase().starts_with("vrchat") && host.parse::<Ipv4Addr>().is_err();
    useful.then(|| host.to_string())
}

/// The list, as the browsing thread keeps it.
#[derive(Default)]
struct List {
    found: Vec<FoundVrchat>,
    misses: HashMap<String, u32>,
    /// Names of what answered as another program: left out.
    refused: HashMap<String, Instant>,
}

impl List {
    /// `name` is announced at `ip`, `query_port`, by a host called `device`.
    fn announced(&mut self, name: &str, ip: Ipv4Addr, query_port: u16, device: Option<String>, ask: &impl Fn(SocketAddr) -> Asked) {
        if self.refused.get(name).is_some_and(|at| at.elapsed() < Duration::from_secs(30)) {
            return;
        }
        let osc_port = match ask(SocketAddr::from((ip, query_port))) {
            Asked::Vrchat(port) => Some(port),
            Asked::Silent => None,
            Asked::NotVrchat => {
                self.refused.insert(name.to_string(), Instant::now());
                self.found.retain(|f| f.name != name);
                return;
            }
        };
        self.misses.remove(name);
        let known = self.found.iter_mut().find(|f| f.name == name);
        match known {
            Some(f) => {
                f.ip = ip;
                f.query_port = query_port;
                f.osc_port = osc_port.or(f.osc_port);
                f.device = device.or(f.device.take());
            }
            None => {
                // Another service at that address is that VRChat started again.
                self.found.retain(|f| f.ip != ip);
                self.found.push(FoundVrchat { name: name.to_string(), device, ip, query_port, osc_port });
            }
        }
    }

    /// The network says `name` is gone.
    fn gone(&mut self, name: &str, ask: &impl Fn(SocketAddr) -> Asked) {
        let answers = self.found.iter().find(|f| f.name == name).is_some_and(|f| matches!(ask(SocketAddr::from((f.ip, f.query_port))), Asked::Vrchat(_)));
        if !answers {
            self.found.retain(|f| f.name != name);
            self.misses.remove(name);
        }
    }

    /// Those that answered once are asked again; what stopped answering goes. What never answered (VRChat on a PC) stays
    /// until the network says it is gone.
    fn ask_again(&mut self, ask: &impl Fn(SocketAddr) -> Asked) {
        let mut going = Vec::new();
        for f in &mut self.found {
            match ask(SocketAddr::from((f.ip, f.query_port))) {
                Asked::Vrchat(port) => {
                    f.osc_port = Some(port);
                    self.misses.remove(&f.name);
                }
                _ if f.osc_port.is_some() => {
                    let misses = self.misses.entry(f.name.clone()).or_default();
                    *misses += 1;
                    if *misses >= MISSES {
                        going.push(f.name.clone());
                    }
                }
                _ => {}
            }
        }
        self.found.retain(|f| !going.contains(&f.name));
        for name in going {
            self.misses.remove(&name);
        }
    }
}

/// Lists every VRChat on the local network while it lives.
pub struct LanBrowser {
    found: Arc<Mutex<Vec<FoundVrchat>>>,
    stop: Arc<AtomicBool>,
    again: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LanBrowser {
    /// Starts looking.
    pub fn start() -> anyhow::Result<Self> {
        anyhow::ensure!(!crate::osc::no_network_listening(), "nothing listens on the network in this test run"); // i18n: not shown (only when ORBITERS_NO_NETWORK_LISTENING is set, which the agents' test runs do)
        let daemon = mdns_sd::ServiceDaemon::new()?;
        let events = daemon.browse(SERVICE)?;
        let found = Arc::<Mutex<Vec<FoundVrchat>>>::default();
        let stop = Arc::new(AtomicBool::new(false));
        let again = Arc::new(AtomicBool::new(false));
        let (shared, stopping, asked) = (found.clone(), stop.clone(), again.clone());
        let thread = std::thread::Builder::new().name("vrchat-lan".into()).spawn(move || browse(daemon, events, &shared, &stopping, &asked))?;
        Ok(Self { found, stop, again, thread: Some(thread) })
    }

    /// The VRChats on the network now.
    pub fn found(&self) -> Vec<FoundVrchat> {
        self.found.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Asks the network again at once (the person pressed Look again, or the network changed).
    pub fn look_again(&self) {
        self.again.store(true, Ordering::Relaxed);
    }
}

impl Drop for LanBrowser {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn browse(daemon: mdns_sd::ServiceDaemon, mut events: mdns_sd::Receiver<mdns_sd::ServiceEvent>, shared: &Mutex<Vec<FoundVrchat>>, stop: &AtomicBool, again: &AtomicBool) {
    let mut list = List::default();
    let mut asked = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if again.swap(false, Ordering::Relaxed) {
            let _ = daemon.stop_browse(SERVICE);
            if let Ok(fresh) = daemon.browse(SERVICE) {
                events = fresh;
            }
        }
        match events.recv_timeout(Duration::from_millis(250)) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) if info.get_fullname().starts_with("VRChat-Client-") => {
                let addresses: Vec<Ipv4Addr> = info.get_addresses_v4().into_iter().collect();
                if let Some(ip) = lan_address(&addresses) {
                    list.announced(short_name(info.get_fullname()), ip, info.get_port(), device_name(info.get_hostname()), &ask);
                }
            }
            Ok(mdns_sd::ServiceEvent::ServiceRemoved(_, name)) => list.gone(short_name(&name), &ask),
            Ok(_) | Err(mdns_sd::RecvTimeoutError::Timeout) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
        if asked.elapsed() >= ASK_EVERY {
            asked = Instant::now();
            list.ask_again(&ask);
        }
        let mut shared = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if *shared != list.found {
            shared.clone_from(&list.found);
        }
    }
    let _ = daemon.shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vrchat(name: &str, last: u8) -> FoundVrchat {
        FoundVrchat { name: name.into(), device: None, ip: Ipv4Addr::new(192, 168, 1, last), query_port: 4000, osc_port: Some(9000) }
    }

    #[test]
    fn the_only_vrchat_is_the_one_without_asking() {
        let wanted = Wanted::default();
        assert_eq!(choose(&wanted, &[]), Choice::Nobody);
        assert_eq!(choose(&wanted, &[vrchat("VRChat-Client-A", 5)]), Choice::Found(vrchat("VRChat-Client-A", 5)));
    }

    #[test]
    fn several_ask_unless_one_was_chosen_before() {
        let found = [vrchat("VRChat-Client-A", 5), vrchat("VRChat-Client-B", 6)];
        assert_eq!(choose(&Wanted::default(), &found), Choice::Ask);
        let by_name = Wanted { name: "VRChat-Client-B".into(), ..Default::default() };
        assert_eq!(choose(&by_name, &found), Choice::Found(found[1].clone()));
        // It started again: another name, the same device.
        let by_address = Wanted { name: "VRChat-Client-OLD".into(), ip: Some(found[0].ip), ..Default::default() };
        assert_eq!(choose(&by_address, &found), Choice::Found(found[0].clone()));
    }

    #[test]
    fn the_one_last_talked_with_is_waited_for_until_another_is_alone_on_the_network() {
        let wanted = Wanted { name: "VRChat-Client-OLD".into(), ip: Some(Ipv4Addr::new(192, 168, 1, 9)), ..Default::default() };
        assert_eq!(choose(&wanted, &[]), Choice::Last(Ipv4Addr::new(192, 168, 1, 9)));
        // The headset got another address.
        let moved = vrchat("VRChat-Client-NEW", 7);
        assert_eq!(choose(&wanted, std::slice::from_ref(&moved)), Choice::Found(moved));
    }

    #[test]
    fn an_address_typed_is_that_one_and_no_other() {
        let typed = Ipv4Addr::new(10, 0, 0, 2);
        let wanted = Wanted { typed: Some(typed), name: "VRChat-Client-A".into(), ip: Some(Ipv4Addr::new(192, 168, 1, 5)) };
        assert_eq!(choose(&wanted, &[vrchat("VRChat-Client-A", 5)]), Choice::Typed(typed));
        let there = FoundVrchat { ip: typed, ..vrchat("VRChat-Client-T", 1) };
        assert_eq!(choose(&wanted, std::slice::from_ref(&there)), Choice::Found(there));
        assert_eq!(Choice::Typed(typed).osc_port(), 9000);
    }

    #[test]
    fn the_input_port_it_says_is_the_one_used() {
        let mut quest = vrchat("VRChat-Client-A", 5);
        quest.osc_port = Some(9123);
        assert_eq!(Choice::Found(quest.clone()).osc_port(), 9123);
        quest.osc_port = None;
        assert_eq!(Choice::Found(quest).osc_port(), 9000);
    }

    #[test]
    fn the_address_listened_on_is_on_the_vrchat_s_network() {
        let wifi = Interface { name: "en0".into(), ip: Ipv4Addr::new(192, 168, 1, 20), netmask: Ipv4Addr::new(255, 255, 255, 0) };
        let wired = Interface { name: "en5".into(), ip: Ipv4Addr::new(10, 0, 0, 20), netmask: Ipv4Addr::new(255, 255, 0, 0) };
        let both = [wired.clone(), wifi.clone()];
        assert_eq!(local_address(Some(Ipv4Addr::new(192, 168, 1, 5)), &both, Some(wired.ip)), Some(wifi.ip));
        assert_eq!(local_address(Some(Ipv4Addr::new(10, 0, 7, 5)), &both, Some(wifi.ip)), Some(wired.ip));
        assert_eq!(local_address(Some(Ipv4Addr::new(172, 16, 0, 1)), &both, Some(wifi.ip)), Some(wifi.ip), "elsewhere: the default route's");
        assert_eq!(local_address(None, &both, Some(wifi.ip)), Some(wifi.ip));
        assert_eq!(local_address(None, &both, None), Some(wired.ip));
        assert_eq!(local_address(Some(wifi.ip), &[], None), None, "no local network");
    }

    #[test]
    fn a_service_that_answers_as_another_program_is_left_out() {
        let mut list = List::default();
        let ip = Ipv4Addr::new(192, 168, 1, 5);
        list.announced("VRChat-Client-FAKE", ip, 4000, None, &|_| Asked::NotVrchat);
        assert!(list.found.is_empty());
        list.announced("VRChat-Client-FAKE", ip, 4000, None, &|_| Asked::Vrchat(9000));
        assert!(list.found.is_empty(), "not asked again for a while");
    }

    #[test]
    fn a_pc_that_keeps_oscquery_to_itself_is_listed_until_the_network_says_it_is_gone() {
        let mut list = List::default();
        let silent = |_| Asked::Silent;
        list.announced("VRChat-Client-PC", Ipv4Addr::new(192, 168, 1, 8), 4000, Some("DESKTOP-1".into()), &silent);
        assert_eq!(list.found.len(), 1);
        assert_eq!(list.found[0].osc_port, None);
        for _ in 0..10 {
            list.ask_again(&silent);
        }
        assert_eq!(list.found.len(), 1, "it never answered: silence says nothing");
        list.gone("VRChat-Client-PC", &silent);
        assert!(list.found.is_empty());
    }

    #[test]
    fn a_headset_that_stops_answering_goes_after_a_few_questions() {
        let mut list = List::default();
        list.announced("VRChat-Client-Q", Ipv4Addr::new(192, 168, 1, 8), 4000, None, &|_| Asked::Vrchat(9000));
        for _ in 0..MISSES - 1 {
            list.ask_again(&|_| Asked::Silent);
            assert_eq!(list.found.len(), 1);
        }
        list.ask_again(&|_| Asked::Vrchat(9000));
        for _ in 0..MISSES - 1 {
            list.ask_again(&|_| Asked::Silent);
        }
        assert_eq!(list.found.len(), 1, "an answer in between starts the count again");
        list.ask_again(&|_| Asked::Silent);
        assert!(list.found.is_empty());
    }

    #[test]
    fn a_vrchat_started_again_replaces_its_old_name_at_that_address() {
        let mut list = List::default();
        let ip = Ipv4Addr::new(192, 168, 1, 8);
        list.announced("VRChat-Client-OLD", ip, 4000, None, &|_| Asked::Vrchat(9000));
        list.announced("VRChat-Client-NEW", ip, 4100, None, &|_| Asked::Vrchat(9000));
        assert_eq!(list.found.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["VRChat-Client-NEW"]);
    }

    #[test]
    fn names_and_addresses_are_read_from_what_is_announced() {
        assert_eq!(short_name("VRChat-Client-ABC._oscjson._tcp.local."), "VRChat-Client-ABC");
        assert_eq!(short_name("VRChat-Client-ABC"), "VRChat-Client-ABC");
        assert_eq!(device_name("Quest-3.local."), Some("Quest-3".into()));
        assert_eq!(device_name("VRChat-Client-ABC.local."), None, "says nothing of the device");
        assert_eq!(device_name("192.168.1.3.local."), None);
        assert_eq!(lan_address(&[Ipv4Addr::LOCALHOST, Ipv4Addr::new(8, 8, 8, 8), Ipv4Addr::new(192, 168, 1, 3)]), Some(Ipv4Addr::new(192, 168, 1, 3)));
        assert_eq!(lan_address(&[Ipv4Addr::LOCALHOST]), None);
        assert_eq!(launch_option(Ipv4Addr::new(192, 168, 1, 20), 9001), "--osc=9000:192.168.1.20:9001");
    }

    /// A stand-in VRChat answering on this computer's address is found, whichever process announces it.
    #[test]
    fn a_vrchat_announced_on_the_network_is_listed_with_its_input_port() {
        if crate::osc::no_network_listening() {
            return;
        }
        let Some(ip) = this_address(None) else { return };
        let listener = std::net::TcpListener::bind((ip, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                use std::io::{Read, Write};
                let mut stream = stream;
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = r#"{"NAME":"VRChat-Client-TEST01","OSC_IP":"0.0.0.0","OSC_PORT":9123}"#;
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        let daemon = mdns_sd::ServiceDaemon::new().unwrap();
        let info = mdns_sd::ServiceInfo::new(SERVICE, "VRChat-Client-TEST01", "stand-in-quest.local.", std::net::IpAddr::V4(ip), port, None::<HashMap<String, String>>).unwrap();
        if daemon.register(info).is_err() {
            return;
        }
        let browser = LanBrowser::start().unwrap();
        let started = Instant::now();
        let mut found = Vec::new();
        while started.elapsed() < Duration::from_secs(10) {
            found = browser.found();
            if !found.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = daemon.shutdown();
        // A network that keeps multicast from this process (macOS asks the person first) finds nothing: not this test's to judge.
        if found.is_empty() {
            eprintln!("nothing heard over mDNS here");
            return;
        }
        assert_eq!(found[0].name, "VRChat-Client-TEST01");
        assert_eq!(found[0].ip, ip);
        assert_eq!(found[0].osc_port, Some(9123));
        assert_eq!(found[0].device.as_deref(), Some("stand-in-quest"));
        assert_eq!(found[0].label(), "VRChat on stand-in-quest");
    }
}
