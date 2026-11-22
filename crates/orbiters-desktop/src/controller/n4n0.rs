//! N4N0 in the tray service: `orbiters_n4n0` runs it (the companion, its menu, states, link cards, its ears, its
//! assistant's turns); this is the service's side of it. The controller is N4N0's host (its settings, what the watchers
//! know of VRChat, the snapshot N4N0's state goes into, the account, the OSC action thread, calls, 3D pictures and the
//! walk boost) and its OSC (the service's own, on this computer's loopback, and the values VRChat sent), through
//! [`Link`], which holds the controller weakly: N4N0 lives inside it.

use super::{Controller, NoticeKind, Shared};
use crate::state::{CallsStatus, N4n0Status, ProcessUsage};
use orbiters_core::account::Api;
use orbiters_core::osc::OscValue;
use orbiters_core::settings::Settings;
use orbiters_core::vrchat_log::WorldState;
use orbiters_i18n::{mark, tr};
use orbiters_n4n0::{CallRequest, Files, Host, Osc, Scene};
use orbiters_n4n0_core::text::Message;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Weak;
use std::time::{Duration, Instant};

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
        self.with(|c| (*c.snapshot.lock().unwrap().vrchat.world).clone()).unwrap_or_default()
    }

    fn capturing(&self) -> bool {
        self.with(|c| c.is_capturing()).unwrap_or(false)
    }

    /// Into the snapshot, with the calls' part (published by the calls themselves) and the avatar's name (VRChat's
    /// log's, when N4N0 has none: VRChat's OSCQuery, its states), and only when it changed. N4N0's lists come as the
    /// slices it keeps: one it did not change is the snapshot's own part still, and is neither compared nor replaced.
    fn publish(&self, mut status: N4n0Status) {
        self.with(|c| {
            c.update_if(|s| {
                status.calls = s.n4n0.calls.clone();
                if status.states.avatar_name.is_none() {
                    status.states.avatar_name = s.vrchat.world.avatar_name.clone();
                }
                let changed = s.n4n0 != status;
                if changed {
                    s.n4n0 = status;
                }
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

    /// With the VRChat account connected in Settings, when there is one (REQUEST20 4).
    fn invite_self(&self, location: &str) -> Option<orbiters_n4n0_core::menu::InviteAnswer> {
        self.with(|c| c.previous_instance_invite(location)).flatten()
    }

    /// On the OSC action thread, in order with the record toggle.
    fn queue(&self, action: Box<dyn FnOnce() + Send>) -> bool {
        self.with(|c| c.osc_action(move |_| action())).unwrap_or(false)
    }

    fn calls(&self) -> CallsStatus {
        self.with(|c| (*c.snapshot.lock().unwrap().n4n0.calls).clone()).unwrap_or_default()
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

    fn camera_pose_known(&self) -> bool {
        self.with(|c| c.camera_pose_known()).unwrap_or(true)
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

    /// N4N0's QR code scanner: VRChat's stream camera (or Settings' Spout sender) read on a thread of its own.
    fn qr_scan(&self, events: orbiters_n4n0::QrEvents) -> Result<Box<dyn orbiters_n4n0::QrScan>, String> {
        let wanted = self.with(|c| c.with_settings(|s| s.spout_sender.clone())).unwrap_or_default();
        crate::qr::start(wanted, events)
    }

    /// N4N0's menu opened History: the captures listed again (controller/menu_shares.rs).
    fn captures_list(&self) {
        self.with(|c| c.menu_captures_list());
    }

    /// A 3D capture shared from N4N0's menu, as Photos & captures shares it.
    fn share_capture(&self, ask: orbiters_n4n0_core::captures::ShareAsk) -> Result<(), String> {
        self.with(|c| c.menu_capture_share(ask)).unwrap_or(Ok(()))
    }

    /// The VRChat friends for N4N0's Share to … (the member's VRChat session, else VRCX's or VRCNext's).
    fn friends(&self) -> Result<Vec<orbiters_n4n0_core::captures::Friend>, String> {
        self.with(|c| c.menu_friends()).unwrap_or_else(|| Ok(Vec::new()))
    }

    /// World feedback N4N0's menu asked: read a world's, or send the player's like, label or comment (controller/
    /// world_feedback.rs).
    fn world_feedback(&self, ask: orbiters_n4n0_core::feedback::FeedbackAsk) {
        self.with(|c| c.world_feedback_ask(ask));
    }

    /// A capture's picture: its session's thumbnail.
    fn capture_picture(&self, key: &str) -> Option<orbiters_n4n0_core::picture::Rgb> {
        super::menu_shares::capture_picture(key)
    }

    /// The avatar's Walk boost radial: kept, and not sent back to the radial.
    fn walk_boost(&self, strength: f32) {
        self.with(|c| c.set_walk_boost(strength, true));
    }

    /// VRChat's `InStation`, `Seated` or `VelocityMagnitude`: the SteamVR tab's walk boost carries only while VRChat
    /// walks the avatar. Published only when what they say changes ([`motion_news`]); a new avatar says it again.
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
                        ("VelocityMagnitude", OscValue::Float(v)) if v.is_finite() => {
                            motion.speed = v.abs();
                            motion.speed_at_ms = Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64);
                        },
                        _ => return false,
                    }
                    let news = motion_news(s.vrchat.motion, motion);
                    s.vrchat.motion = Some(motion);
                    news
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

/// The N4N0 page's footprint rows from `(name, cpu percent of a core, memory MB)` of every process.
fn footprint<'a>(processes: impl Iterator<Item = (&'a str, f32, f32)>) -> Vec<ProcessUsage> {
    let groups: [(&str, &[&str]); 5] = [
        ("Orbiters - N4N0", &["orbiters-desktop.exe", "orbiters-desktop"]),
        (mark("Capture worker"), &["orbiters-capture.exe", "orbiters-capture"]),
        (mark("N4N0's ears"), &["orbiters-voice.exe", "orbiters-voice"]),
        (mark("Reconstruction"), &["orbiters-reconstruct.exe", "orbiters-reconstruct"]),
        ("VRChat", &["vrchat.exe", "vrchat"]),
    ];
    let processes: Vec<(String, f32, f32)> = processes.map(|(n, c, m)| (n.to_lowercase(), c, m)).collect();
    let mut usage = Vec::new();
    for (label, names) in groups {
        let mut found = processes.iter().filter(|(n, _, _)| names.contains(&n.as_str())).peekable();
        if found.peek().is_none() {
            continue;
        }
        let (cpu, memory) = found.fold((0.0f32, 0.0f32), |(c, m), p| (c + p.1, m + p.2));
        usage.push(ProcessUsage { name: tr(label).to_string(), cpu_percent: cpu, memory_mb: memory });
    }
    usage
}

/// When a motion of VRChat's was last handed out only for its speed (milliseconds since 1970), see [`motion_news`].
static SPEED_HANDED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Whether a motion VRChat said is worth handing the state out for (it is kept either way): a station or a seat, or the
/// speed crossing what Fast walk tells apart (still under 0.15 m/s, walking from 0.4), at once; another speed at most four
/// times a second. VRChat says `VelocityMagnitude` at its frame rate while the avatar moves, and each one, its time
/// changing, published the whole state (N-friend-lag).
fn motion_news(before: Option<crate::state::AvatarMotion>, now: crate::state::AvatarMotion) -> bool {
    use std::sync::atomic::Ordering;
    let Some(before) = before else { return true };
    if crate::perf::old_publishing() {
        return before != now;
    }
    let class = |speed: f32| if !speed.is_finite() { 3 } else if speed < 0.15 { 0 } else if speed < 0.4 { 1 } else { 2 };
    if before.in_station != now.in_station || before.seated != now.seated || class(before.speed) != class(now.speed) || before.speed_at_ms.is_some() != now.speed_at_ms.is_some() {
        if let Some(at) = now.speed_at_ms {
            SPEED_HANDED_MS.store(at, Ordering::Relaxed);
        }
        return true;
    }
    if before.speed == now.speed {
        return false;
    }
    let at = now.speed_at_ms.unwrap_or(0);
    if at.saturating_sub(SPEED_HANDED_MS.load(Ordering::Relaxed)) < 250 {
        return false;
    }
    SPEED_HANDED_MS.store(at, Ordering::Relaxed);
    true
}

impl Controller {
    /// N4N0 starts, with the service's own watch beside it: the footprint of Orbiters' processes and VRChat's (the N4N0
    /// page's), the world performance summaries still waiting (VRChat closed before they were sent), and the 3D captures
    /// its menu shows.
    pub(super) fn start_n4n0(self: &Shared) {
        self.n4n0.start();
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("n4n0-resources".into()).spawn(move || {
            let mut system = sysinfo::System::new();
            let mut before: Option<(Instant, Vec<crate::perf::Raw>)> = None;
            let mut beat = 0u64;
            while !ctl.n4n0.stopped() {
                if beat.is_multiple_of(3) {
                    if cfg!(windows) {
                        ctl.n4n0_resources_scanned(&mut before);
                    } else {
                        ctl.n4n0_resources(&mut system);
                    }
                }
                // Where the 3D captures N4N0's menu shows stand (their recording, 3D view and shares).
                ctl.menu_captures_tick();
                if beat.is_multiple_of(60) && ctl.settings().world_performance {
                    ctl.performance_send_if_due(false);
                }
                beat += 1;
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    /// What the window and the overlay ask of N4N0.
    pub fn n4n0_command(self: &Shared, command: crate::ipc::N4n0Command) {
        self.n4n0.command(command);
    }

    /// The Talk button of the window, or of the overlay (`overlay`), from the service's link with it.
    pub fn n4n0_app_talk(self: &Shared, down: bool, overlay: bool) {
        self.n4n0.app_talk(down, overlay);
    }

    /// The window or the overlay went away: a Talk button it held is let go without its sentence, and the overlay's menu
    /// is closed.
    pub fn n4n0_link_closed(self: &Shared, overlay: bool) {
        self.n4n0.link_closed(overlay);
    }

    /// Shows `text` on N4N0's panel (nothing without the prop).
    pub(super) fn n4n0_say(&self, text: &str) {
        self.n4n0.say(text);
    }

    /// The panel shows `text` at once for the owner ([`orbiters_n4n0::runtime::N4n0::say_quick`]).
    pub(super) fn n4n0_say_quick(&self, text: &str) {
        self.n4n0.say_quick(text);
    }

    /// [`Controller::n4n0_resources`] from one `NtQuerySystemInformation` call (`perf::scan`) instead of sysinfo's refresh,
    /// which opened every process: 8 ms a look instead of 61 ms with 494 processes on Enzo's PC, every 3 s whether the
    /// page shows or not (N-friend-lag, `perf::tests::a_scan_costs_less_than_sysinfo_s_refresh`).
    fn n4n0_resources_scanned(&self, before: &mut Option<(Instant, Vec<crate::perf::Raw>)>) {
        let now = Instant::now();
        let scan = crate::perf::scan();
        let Some((then, old)) = before.replace((now, scan.clone())) else { return };
        let used = crate::perf::uses(&old, &scan, now.duration_since(then).as_secs_f32());
        self.n4n0.set_resources(footprint(used.iter().map(|p| (p.name.as_str(), p.cpu, p.working_mb))));
    }

    /// CPU and memory of Orbiters' processes and VRChat's (the N4N0 page's footprint).
    fn n4n0_resources(&self, system: &mut sysinfo::System) {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory());
        let processes: Vec<(String, f32, f32)> = system.processes().values().map(|p| (p.name().to_string_lossy().into_owned(), p.cpu_usage(), p.memory() as f32 / (1024.0 * 1024.0))).collect();
        self.n4n0.set_resources(footprint(processes.iter().map(|(n, c, m)| (n.as_str(), *c, *m))));
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
        ctl.update(|s| s.n4n0.calls.menu_open = true);
        ctl.n4n0.say("nothing to show: no prop");
        ctl.n4n0_command(crate::ipc::N4n0Command::TestFace { face: Some(crate::state::N4n0Face::Happy) });
        let n4n0 = ctl.view().n4n0.clone();
        assert_eq!(n4n0.test_face, Some(crate::state::N4n0Face::Happy), "published into the snapshot");
        assert!(n4n0.calls.menu_open, "the calls' part kept");
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
        assert!(offs.contains(&format!("{PARAMETERS_PREFIX}{}", orbiters_contracts::n4n0::params::SCREEN_OPEN)), "{offs:?}");
    }
}

#[cfg(test)]
mod motion_news_tests {
    use super::motion_news;
    use crate::state::AvatarMotion;

    /// N-friend-lag: VRChat says `VelocityMagnitude` every frame while walking; the state goes out four times a second
    /// for it, and at once when Fast walk's reading of it changes (still, between, walking) or a station or seat does.
    #[test]
    fn walking_hands_its_speed_out_four_times_a_second_not_at_vrchats_frame_rate() {
        let m = |speed: f32, at: u64| AvatarMotion { speed, speed_at_ms: Some(at), ..Default::default() };
        let base = 40_000_000_000u64;
        let mut before = Some(m(1.0, base));
        let mut handed = 0;
        for f in 1..=90u64 {
            let now = m(1.0 + f as f32 * 0.001, base + f * 11);
            handed += motion_news(before, now) as u32;
            before = Some(now);
        }
        assert!((3..=5).contains(&handed), "{handed} in a second at 90 Hz");
        let last = before.unwrap();
        assert!(motion_news(before, m(0.05, last.speed_at_ms.unwrap() + 1)), "stopping is said at once");
        assert!(motion_news(Some(m(0.05, 1)), AvatarMotion { in_station: true, ..m(0.05, 2) }), "a station at once");
        assert!(!motion_news(Some(m(0.05, 1)), m(0.05, 2)), "the same speed again is nothing new");
        assert!(motion_news(None, m(0.0, 1)), "the first is news");
    }
}

#[cfg(test)]
mod footprint_tests {
    /// The N4N0 page's footprint groups processes by program, whatever the case of their names, and leaves out absent ones.
    #[test]
    fn the_footprint_groups_processes_by_program() {
        let rows = super::footprint([("orbiters-desktop.exe", 1.5, 100.0), ("Orbiters-Desktop.exe", 0.5, 50.0), ("VRChat.exe", 60.0, 7000.0), ("other.exe", 99.0, 1.0)].into_iter());
        let got: Vec<(String, f32, f32)> = rows.iter().map(|r| (r.name.clone(), r.cpu_percent, r.memory_mb)).collect();
        assert_eq!(got, vec![("Orbiters - N4N0".to_string(), 2.0, 150.0), ("VRChat".to_string(), 60.0, 7000.0)]);
    }
}
