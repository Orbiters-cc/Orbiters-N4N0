//! VRChat on the network, where it does not run on this computer ([`orbiters_core::osc::VRCHAT_RUNS_HERE`]): a thread
//! keeps the list of those found, chooses the one to talk with ([`orbiters_core::lan::choose`]), and keeps the OSC service
//! on the network that VRChat is on, on the port a PC's launch options name when it can have it.
//!
//! On a Mac, **the VRChat Home started there comes first** while it runs (`Controller::own_game`): the service is on the
//! Mac's own loopback, as beside a PC's VRChat, on the port VRChat sends to by default (9001) when it can have it, so that
//! what VRChat says arrives whether or not the two find each other by OSCQuery. An address the person typed stays theirs.

use orbiters_core::locks::*;
use super::{NoticeKind, Shared};
use crate::state::{LanLink, LanStatus};
use orbiters_core::lan::{self, Choice, LanBrowser};
use orbiters_core::osc::{Discovery, LanVrchat, Network, Ports};
use orbiters_i18n::tr;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

const EVERY: Duration = Duration::from_millis(500);
/// The same while a game runs on this very Mac.
const EVERY_IN_GAME: Duration = Duration::from_secs(2);
/// Sends nothing and answers nothing for this long: no longer connected.
const QUIET_NS: u64 = 60_000_000_000;
/// Keeps the service's own browsing from binding to one VRChat while several are there and none is chosen.
const NO_NAME: &str = "VRChat-Client-none-chosen";
/// While the system keeps the app off the network, its turns between two tries: the answer to its question comes unannounced.
const TRY_AGAIN_EVERY: u32 = 10;

/// What the controller keeps for this.
#[derive(Default)]
pub(super) struct Runtime {
    look: AtomicBool,
    settings: AtomicBool,
    alive: AtomicBool,
    turns: AtomicU32,
}

impl Runtime {
    /// Looks on the network again at its next turn (Look again).
    pub(super) fn look_again(&self) {
        self.look.store(true, Ordering::Relaxed);
    }

    /// OSC's settings changed: acted on at its next turn.
    pub(super) fn settings_changed(&self) {
        self.settings.store(true, Ordering::Relaxed);
    }

    /// A VRChat on the network answers or sends.
    pub(super) fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

/// What a running OSC service was started for. VRChat's input port is not part of it: a port learned later reaches the
/// running service, which VRChat knows, without restarting it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Running {
    here: Ipv4Addr,
    peer: Option<Ipv4Addr>,
    /// No VRChat is to be taken on its own: several are there.
    asked: bool,
    /// For the VRChat that runs on this Mac.
    own: bool,
}

impl Running {
    /// The service for the VRChat that runs on this Mac.
    fn own() -> Self {
        Self { here: Ipv4Addr::LOCALHOST, peer: Some(Ipv4Addr::LOCALHOST), asked: false, own: true }
    }

    /// Where it listens, and on what ports.
    fn network(&self, choice: &Choice, vrchat_port: u16) -> (Network, Ports) {
        if self.own {
            return (Network::Loopback { vrchat_port }, Ports { osc: lan::OSC_PORT, query: 0 });
        }
        let vrchat = LanVrchat { ip: self.peer, name: self.asked.then(|| NO_NAME.to_string()), osc_port: choice.osc_port() };
        (Network::Lan { ip: self.here, vrchat }, Ports { osc: lan::OSC_PORT, query: 0 })
    }
}

/// Whether the VRChat of this Mac is the one talked with: it runs, and the person typed no other's address.
fn talks_with_own(own_game: bool, wanted: &lan::Wanted) -> bool {
    own_game && wanted.typed.is_none()
}

/// What the link is, from what is chosen and heard, and whether the system lets the app on the network.
fn link(choice: &Choice, here: Option<Ipv4Addr>, heard: bool, refused: bool) -> LanLink {
    match (here, choice) {
        (None, _) => LanLink::NoNetwork,
        _ if refused && !heard => LanLink::NotAllowed,
        (_, Choice::Nobody) => LanLink::Looking,
        (_, Choice::Ask) => LanLink::Choose,
        (_, _) if heard => LanLink::Connected,
        (_, Choice::Found(found)) if found.osc_port.is_some() => LanLink::Connected,
        _ => LanLink::Waiting,
    }
}

/// What the link with this Mac's own VRChat is.
fn own_link(heard: bool) -> LanLink {
    if heard {
        LanLink::Connected
    } else {
        LanLink::Waiting
    }
}

impl super::Controller {
    /// Starts the thread, on a computer VRChat does not run on.
    pub(super) fn start_lan(self: &Shared) {
        if orbiters_core::osc::VRCHAT_RUNS_HERE {
            return;
        }
        let ctl = std::sync::Arc::downgrade(self);
        let _ = std::thread::Builder::new().name("vrchat-network".into()).spawn(move || {
            let mut browser: Option<LanBrowser> = None;
            let mut running: Option<Running> = None;
            loop {
                let wait = {
                    let Some(ctl) = ctl.upgrade() else { return };
                    if ctl.quitting.load(Ordering::Relaxed) {
                        return;
                    }
                    ctl.keep_lan(&mut browser, &mut running);
                    if ctl.game_on_this_mac() { EVERY_IN_GAME } else { EVERY }
                };
                std::thread::sleep(wait);
            }
        });
    }

    /// One turn: the list, the choice, the service, what the interface shows.
    fn keep_lan(self: &Shared, browser: &mut Option<LanBrowser>, running: &mut Option<Running>) {
        let (enabled, wanted) = self.with_settings(|s| (s.osc.enabled, s.osc.remote.clone()));
        let settings_changed = self.lan.settings.swap(false, Ordering::Relaxed);
        if !enabled {
            *browser = None;
            *running = None;
            self.lan.alive.store(false, Ordering::Relaxed);
            let service = self.osc.locked().take();
            drop(service);
            self.camera_route_changed();
            self.update_if(|s| {
                let changed = s.lan != LanStatus::default();
                *s.lan = LanStatus::default();
                changed
            });
            return;
        }
        if browser.is_none() {
            match LanBrowser::start() {
                Ok(started) => *browser = Some(started),
                Err(e) => eprintln!("looking for VRChat on the network: {e:#}"),
            }
        }
        if self.lan.look.swap(false, Ordering::Relaxed) {
            if let Some(browser) = browser.as_ref() {
                browser.look_again();
            }
        }
        let found = browser.as_ref().map(LanBrowser::found).unwrap_or_default();
        let own = talks_with_own(self.own_game(), &wanted);
        let choice = lan::choose(&wanted, &found);
        let peer = if own { Some(Ipv4Addr::LOCALHOST) } else { choice.ip() };
        let here = if own { Some(Ipv4Addr::LOCALHOST) } else { lan::this_address(peer) };

        // Remembered, so that it is the one next time: under another name, or among others.
        if let Choice::Found(vrchat) = &choice {
            // (this Mac's own may be listed at the Mac's address: not one to remember)
            if !own && wanted.typed.is_none() && (wanted.name != vrchat.name || wanted.ip != Some(vrchat.ip)) {
                let first = wanted.ip != Some(vrchat.ip);
                let (name, ip, label) = (vrchat.name.clone(), vrchat.ip, vrchat.label());
                self.update_settings(|s| {
                    s.osc.remote.name = name;
                    s.osc.remote.ip = Some(ip);
                });
                if first {
                    self.notify(NoticeKind::Success, tr!("Connected to {label}.", label = label));
                }
            }
        }

        let wanted_service = if own { Some(Running::own()) } else { here.map(|here| Running { here, peer, asked: choice == Choice::Ask, own: false }) };
        if wanted_service.as_ref() != running.as_ref() || (settings_changed && wanted_service.is_some()) {
            // The service before goes first: the new one wants its port (and stopping waits for its receiving thread,
            // outside the lock).
            let before = self.osc.locked().take();
            drop(before);
            self.camera_route_changed();
            match &wanted_service {
                Some(next) => {
                    let (network, ports) = next.network(&choice, self.with_settings(|s| s.osc.vrchat_port));
                    self.start_osc_on(network, Discovery::Mdns, ports);
                }
                None => self.update(|s| *s.osc = crate::state::OscStatus { enabled: true, ..Default::default() }),
            }
            *running = wanted_service;
        } else if let Some(peer) = peer.filter(|_| !own) {
            self.with_osc(|o| {
                let target = std::net::SocketAddr::from((peer, choice.osc_port()));
                if o.vrchat_target() != Some(target) {
                    o.choose_vrchat(peer, choice.osc_port());
                }
            });
        }

        let heard = {
            let osc = self.osc.locked();
            let received = osc.as_ref().and_then(|o| o.last_received_ns());
            let answers = osc.as_ref().is_some_and(|o| o.vrchat_query().is_some());
            answers || received.is_some_and(|t| orbiters_core::clock::now_ns().saturating_sub(t) < QUIET_NS)
        };
        // (the loopback is no network macOS keeps an app off)
        let refused = !own && orbiters_core::osc::local_network_refused();
        if refused && self.lan.turns.fetch_add(1, Ordering::Relaxed) % TRY_AGAIN_EVERY == 0 {
            self.with_osc(|o| o.rediscover());
        }
        let link = if own { own_link(heard) } else { link(&choice, here, heard, refused) };
        if self.lan.alive.swap(link == LanLink::Connected, Ordering::Relaxed) != (link == LanLink::Connected) {
            eprintln!("VRChat on the network: {link:?} {}", peer.map(|p| p.to_string()).unwrap_or_default());
        }
        let label = match &choice {
            _ if own => Some(tr("VRChat on this Mac").to_string()),
            Choice::Found(vrchat) => Some(vrchat.label()),
            Choice::Typed(ip) | Choice::Last(ip) => Some(tr!("VRChat at {ip}", ip = ip)),
            Choice::Nobody | Choice::Ask => None,
        };
        let status = LanStatus { link, found, peer, peer_label: label, this_computer: here.filter(|_| !own) };
        self.update_if(|s| s.lan.set(status));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::lan::FoundVrchat;

    fn found(osc_port: Option<u16>) -> Choice {
        Choice::Found(FoundVrchat { name: "VRChat-Client-A".into(), device: None, ip: Ipv4Addr::new(192, 168, 1, 5), query_port: 4000, osc_port })
    }

    /// The VRChat Home started on this Mac is talked with on its loopback, on the port VRChat sends to by default; an address the person typed stays theirs.
    #[test]
    fn this_mac_s_own_vrchat_comes_first() {
        let typed = lan::Wanted { typed: Some(Ipv4Addr::new(192, 168, 1, 5)), ..lan::Wanted::default() };
        let last = lan::Wanted { name: "VRChat-Client-A".into(), ip: Some(Ipv4Addr::new(192, 168, 1, 5)), ..lan::Wanted::default() };
        assert!(talks_with_own(true, &lan::Wanted::default()) && talks_with_own(true, &last));
        assert!(!talks_with_own(true, &typed));
        assert!(!talks_with_own(false, &last), "it does not run");
        let (network, ports) = Running::own().network(&found(Some(9000)), 9000);
        assert_eq!(network, Network::Loopback { vrchat_port: 9000 });
        assert_eq!((ports.osc, ports.query), (9001, 0));
        let other = Running { here: Ipv4Addr::new(192, 168, 1, 20), peer: found(None).ip(), asked: false, own: false };
        assert!(matches!(other.network(&found(Some(9010)), 9000).0, Network::Lan { vrchat, .. } if vrchat.osc_port == 9010));
        assert_eq!((own_link(true), own_link(false)), (LanLink::Connected, LanLink::Waiting));
    }

    #[test]
    fn the_link_says_what_is_going_on() {
        let here = Some(Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(link(&Choice::Nobody, None, false, false), LanLink::NoNetwork);
        assert_eq!(link(&found(Some(9000)), None, true, false), LanLink::NoNetwork, "without a network, nothing else counts");
        assert_eq!(link(&Choice::Nobody, here, false, false), LanLink::Looking);
        assert_eq!(link(&Choice::Ask, here, false, false), LanLink::Choose);
        // A headset answers over the network; a PC only by what it sends.
        assert_eq!(link(&found(Some(9000)), here, false, false), LanLink::Connected);
        assert_eq!(link(&found(None), here, false, false), LanLink::Waiting);
        assert_eq!(link(&found(None), here, true, false), LanLink::Connected);
        assert_eq!(link(&Choice::Typed(Ipv4Addr::new(10, 0, 0, 2)), here, false, false), LanLink::Waiting);
        assert_eq!(link(&Choice::Last(Ipv4Addr::new(10, 0, 0, 2)), here, true, false), LanLink::Connected);
        // The system says no: that first, unless something was heard all the same.
        assert_eq!(link(&Choice::Nobody, here, false, true), LanLink::NotAllowed);
        assert_eq!(link(&found(Some(9000)), here, false, true), LanLink::NotAllowed);
        assert_eq!(link(&found(Some(9000)), here, true, true), LanLink::Connected);
    }
}
