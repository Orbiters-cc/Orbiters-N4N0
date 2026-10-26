//! N4N0 in the tray service: `orbiters_n4n0` runs it (the companion, its menu, states, link cards, its ears, its
//! assistant's turns); this is the service's side of it. The controller is N4N0's host (its settings, what the watchers
//! know of VRChat, the snapshot N4N0's state goes into, the account, the OSC action thread, calls, 3D pictures and the
//! walk boost) and its OSC (the service's own, on this computer's loopback, and the values VRChat sent), through
//! [`Link`], which holds the controller weakly: N4N0 lives inside it.

use super::{Controller, NoticeKind, Shared};
use crate::state::{CallsStatus, OrbStatus, ProcessUsage};
use orbiters_core::account::Api;
use orbiters_core::osc::OscValue;
use orbiters_core::settings::Settings;
use orbiters_core::vrchat_log::WorldState;
use orbiters_n4n0::{CallRequest, Files, Host, Osc, Scene};
use orbiters_orb::text::Message;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Weak;
use std::time::Duration;

/// Where N4N0 keeps its files: the app's (its videos and the avatars' states beside the settings). Tests never touch the
/// person's: theirs go to a folder of their own.
pub(super) fn files() -> Files {
    if cfg!(test) {
        Files::in_dir(&std::env::temp_dir().join(format!("orbiters-desktop-n4n0-test-{}", std::process::id())))
    } else {
        Files::app()
    }
}

/// The controller as N4N0's host and OSC.
pub(super) struct Link(pub(super) Weak<Controller>);

impl Link {
    /// Runs `f` with the controller while it lives (it always does while N4N0 runs, which it holds).
    fn with<R>(&self, f: impl FnOnce(&Shared) -> R) -> Option<R> {
        self.0.upgrade().map(|c| f(&c))
    }
}

impl Host for Link {
    fn with_settings(&self, read: &mut dyn FnMut(&Settings)) {
        if let Some(c) = self.0.upgrade() {
            c.with_settings(|s| read(s));
        } else {
            read(&Settings::default());
        }
    }

    fn update_settings(&self, change: &mut dyn FnMut(&mut Settings)) {
        self.with(|c| c.update_settings(|s| change(s)));
    }

    fn scene(&self) -> Scene {
        self.with(|c| {
            let s = c.snapshot.lock().unwrap();
            Scene { vrchat: s.vrchat.running, vrchat_pid: s.vrchat.pid, steamvr: s.steamvr.running }
        })
        .unwrap_or_default()
    }

    fn world(&self) -> WorldState {
        self.with(|c| c.snapshot.lock().unwrap().vrchat.world.clone()).unwrap_or_default()
    }

    fn capturing(&self) -> bool {
        self.with(|c| c.is_capturing()).unwrap_or(false)
    }

    /// Into the snapshot, with the calls' part (published by the calls themselves) and the avatar's name (VRChat's
    /// log's, when N4N0 has none: VRChat's OSCQuery, its states), and only when it changed.
    fn publish(&self, mut status: OrbStatus) {
        self.with(|c| {
            c.update_if(|s| {
                status.calls = s.orb.calls.clone();
                if status.states.avatar_name.is_none() {
                    status.states.avatar_name = s.vrchat.world.avatar_name.clone();
                }
                let changed = s.orb != status;
                s.orb = status;
                changed
            })
        });
    }

    fn notify(&self, kind: NoticeKind, message: String) {
        self.with(|c| c.notify(kind, message));
    }

    fn account(&self) -> Option<(Api, String)> {
        self.with(|c| c.account_api()).flatten()
    }

    /// On the OSC action thread, in order with the record toggle.
    fn queue(&self, action: Box<dyn FnOnce() + Send>) -> bool {
        self.with(|c| c.osc_action(move |_| action())).unwrap_or(false)
    }

    fn calls(&self) -> CallsStatus {
        self.with(|c| c.snapshot.lock().unwrap().orb.calls.clone()).unwrap_or_default()
    }

    fn ringing_in(&self) -> bool {
        self.with(|c| c.call_ringing_in()).unwrap_or(false)
    }

    fn call(&self, request: CallRequest) {
        self.with(|c| match request {
            CallRequest::Contact(id) => c.call_contact(id),
            CallRequest::Answer => c.call_answer(),
            CallRequest::End => c.call_end(),
            CallRequest::Mute(on) => c.call_mute(on),
            CallRequest::Menu => c.calls_menu(),
            CallRequest::Go => {
                c.calls_go();
            }
            CallRequest::Pick(value) => c.calls_pick(value),
        });
    }

    fn photo_3d(&self) -> anyhow::Result<()> {
        self.with(|c| c.take_3d_picture()).unwrap_or(Ok(()))
    }

    fn camera_open(&self) -> Option<bool> {
        self.with(|c| c.camera_open()).flatten()
    }

    /// A video's link opened in the default browser (http or https only).
    fn watch(&self, link: &str) -> Result<(), String> {
        super::open_url(link).map_err(|e| e.to_string())
    }

    /// N4N0's menu shows its keyboard: the computer's keyboard types there while VRChat is in front.
    fn keyboard(&self, on: bool, vrchat: Option<u32>) {
        crate::keys::capture(on, vrchat);
    }

    /// Tracking smoothing: what the driver does now (Windows only).
    fn smoothing(&self) -> Option<String> {
        if !cfg!(windows) {
            return None;
        }
        self.0.upgrade().map(|c| c.tracking_note())
    }

    /// The avatar's Walk boost radial: kept, and not sent back to the radial.
    fn walk_boost(&self, strength: f32) {
        self.with(|c| c.set_walk_boost(strength, true));
    }

    /// VRChat's `InStation`, `Seated` or `VelocityMagnitude`: the SteamVR tab's walk boost carries only while VRChat
    /// walks the avatar. Published only when what they say changes; a new avatar says it again.
    fn motion(&self, said: Option<(&str, &OscValue)>) {
        self.with(|c| match said {
            None => {
                c.update_if(|s| s.vrchat.motion.take().is_some());
            }
            Some((name, value)) => {
                c.update_if(|s| {
                    let mut motion = s.vrchat.motion.unwrap_or_default();
                    match (name, value) {
                        ("InStation", v) => motion.in_station = v.truthy(),
                        ("Seated", v) => motion.seated = v.truthy(),
                        ("VelocityMagnitude", OscValue::Float(v)) if v.is_finite() => motion.speed = (v.abs() * 4.0).round() / 4.0,
                        _ => return false,
                    }
                    let changed = s.vrchat.motion != Some(motion);
                    s.vrchat.motion = Some(motion);
                    changed
                });
            }
        });
    }
}

impl Osc for Link {
    /// Under the OSC lock, every bundle to the same service.
    fn send(&self, bundles: &[&[Message]]) -> Option<Vec<anyhow::Result<()>>> {
        self.with(|c| {
            c.with_osc(|o| {
                bundles
                    .iter()
                    .map(|bundle| match bundle {
                        [(address, arg)] => o.send(address, vec![arg.clone()]),
                        _ => o.send_bundle(bundle.iter().map(|(address, arg)| (address.clone(), vec![arg.clone()])).collect()),
                    })
                    .collect()
            })
        })
        .flatten()
    }

    fn vrchat_query(&self) -> Option<Option<SocketAddr>> {
        self.with(|c| c.with_osc(|o| o.vrchat_query())).flatten()
    }

    fn latest(&self, addresses: &mut dyn Iterator<Item = String>) -> HashMap<String, OscValue> {
        self.with(|c| {
            let osc = c.osc_values.lock().unwrap();
            addresses.filter_map(|a| osc.latest.first(&a).map(|v| (a, v))).collect()
        })
        .unwrap_or_default()
    }

    fn vrchat_values(&self) -> Vec<(String, OscValue)> {
        self.with(|c| c.vrchat_values()).unwrap_or_default()
    }
}

impl Controller {
    /// N4N0 starts, with the service's own watch beside it: the footprint of Orbiters' processes and VRChat's (the N4N0
    /// page's), and the world performance summaries still waiting (VRChat closed before they were sent).
    pub(super) fn start_orb(self: &Shared) {
        self.n4n0.start();
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("orb-resources".into()).spawn(move || {
            let mut system = sysinfo::System::new();
            let mut beat = 0u64;
            while !ctl.n4n0.stopped() {
                if beat.is_multiple_of(3) {
                    ctl.orb_resources(&mut system);
                }
                if beat.is_multiple_of(60) && ctl.settings().world_performance {
                    ctl.performance_send_if_due(false);
                }
                beat += 1;
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    /// What the window and the overlay ask of N4N0.
    pub fn orb_command(self: &Shared, command: crate::ipc::OrbCommand) {
        self.n4n0.command(command);
    }

    /// The Talk button of the window, or of the overlay (`overlay`), from the service's link with it.
    pub fn orb_app_talk(self: &Shared, down: bool, overlay: bool) {
        self.n4n0.app_talk(down, overlay);
    }

    /// The window or the overlay went away: a Talk button it held is let go without its sentence, and the overlay's menu
    /// is closed.
    pub fn orb_link_closed(self: &Shared, overlay: bool) {
        self.n4n0.link_closed(overlay);
    }

    /// Shows `text` on N4N0's panel (nothing without the prop).
    pub(super) fn orb_say(&self, text: &str) {
        self.n4n0.say(text);
    }

    /// CPU and memory of Orbiters' processes and VRChat's (the N4N0 page's footprint).
    fn orb_resources(&self, system: &mut sysinfo::System) {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory());
        let groups: [(&str, &[&str]); 5] = [
            ("Orbiters - N4N0", &["orbiters-desktop.exe", "orbiters-desktop"]),
            ("Capture worker", &["orbiters-capture.exe", "orbiters-capture"]),
            ("N4N0's ears", &["orbiters-voice.exe", "orbiters-voice"]),
            ("Reconstruction", &["orbiters-reconstruct.exe", "orbiters-reconstruct"]),
            ("VRChat", &["vrchat.exe", "vrchat"]),
        ];
        let mut usage = Vec::new();
        for (label, names) in groups {
            let (mut cpu, mut memory, mut any) = (0.0f32, 0u64, false);
            for p in system.processes().values() {
                let n = p.name().to_string_lossy().to_lowercase();
                if names.contains(&n.as_str()) {
                    any = true;
                    cpu += p.cpu_usage();
                    memory += p.memory();
                }
            }
            if any {
                usage.push(ProcessUsage { name: label.to_string(), cpu_percent: cpu, memory_mb: memory as f32 / (1024.0 * 1024.0) });
            }
        }
        self.n4n0.set_resources(usage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::osc::{OscEvent, PARAMETERS_PREFIX};
    use std::time::Instant;

    /// N4N0 lives in the controller: what VRChat sends reaches it, its state lands in the snapshot (the calls' part
    /// kept), and quitting closes N4N0's screen while OSC still runs.
    #[test]
    fn n4n0_runs_inside_the_controller() {
        let stand_in = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        stand_in.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        let mut settings = Settings::default();
        settings.osc.vrchat_port = stand_in.local_addr().unwrap().port();
        let ctl = Controller::new(settings);
        ctl.start_osc_with(false);
        // The calls' part of the snapshot stays the calls' own.
        ctl.update(|s| s.orb.calls.menu_open = true);
        ctl.n4n0.say("nothing to show: no prop");
        ctl.orb_command(crate::ipc::OrbCommand::TestFace { face: Some(crate::state::OrbFace::Happy) });
        let orb = ctl.view().orb.clone();
        assert_eq!(orb.test_face, Some(crate::state::OrbFace::Happy), "published into the snapshot");
        assert!(orb.calls.menu_open, "the calls' part kept");
        // An avatar parameter and an avatar change go to N4N0, and the values VRChat sent are kept for it.
        let mut events = vec![
            OscEvent::Parameter { name: "AFK".into(), value: OscValue::Bool(true) },
            OscEvent::Parameter { name: "Seated".into(), value: OscValue::Bool(true) },
        ];
        ctl.on_osc_events(&mut events, 5, "OrbitersRecord");
        assert_eq!(ctl.view().vrchat.motion.map(|m| m.seated), Some(true), "the avatar's motion, the walk boost's");
        assert_eq!(Link(std::sync::Arc::downgrade(&ctl)).latest(&mut std::iter::once(format!("{PARAMETERS_PREFIX}AFK"))).len(), 1);
        let mut change = vec![OscEvent::AvatarChanged { id: "avtr_ctl".into() }];
        ctl.on_osc_events(&mut change, 6, "OrbitersRecord");
        assert!(ctl.view().vrchat.motion.is_none(), "a new avatar says how it moves again");
        // Quitting sends N4N0's screen off before OSC stops.
        let started = Instant::now();
        ctl.shutdown();
        let mut buf = [0u8; 1024];
        let mut offs = Vec::new();
        while started.elapsed() < Duration::from_secs(1) {
            if let Ok(n) = stand_in.recv(&mut buf) {
                if let Ok((_, rosc::OscPacket::Bundle(b))) = rosc::decoder::decode_udp(&buf[..n]) {
                    offs.extend(b.content.into_iter().filter_map(|p| match p {
                        rosc::OscPacket::Message(m) => Some(m.addr),
                        _ => None,
                    }));
                }
            }
        }
        assert!(offs.contains(&format!("{PARAMETERS_PREFIX}{}", orbiters_contracts::orb::params::SCREEN_OPEN)), "{offs:?}");
    }
}
