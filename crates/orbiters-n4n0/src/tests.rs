//! N4N0 in a stand-in app ([`TestHost`]: settings, VRChat as a test sets it, a real OSC service on the loopback
//! interface talking with a stand-in VRChat) on a stand-in device (its clipboard, browser, sounds, language, ears, music
//! and mouse as tests set them). The behaviour tests came from the desktop's controller; the platform tests check what
//! N4N0 asks of each trait.

use crate::media::{MediaBackend, NoMedia, Reading, Watcher};
use crate::platform::*;
use crate::runtime::{int_value, parameter, screen_replace, thumbnail_bytes, Shared};
use crate::status::*;
use crate::{Command, N4n0};
use orbiters_contracts::avatar::AvatarManifest;
use orbiters_contracts::n4n0::{self as contract, params, Page, POINTER_CLEAR, TEXT_LENGTH};
use orbiters_core::account::Api;
use orbiters_core::osc::{Latest, Network, OscEvent, OscService, OscValue, AVATAR_CHANGE, PARAMETERS_PREFIX};
use orbiters_core::settings::{Pin, Settings};
use orbiters_core::voice::{VoiceCommand, VoiceEvent};
use orbiters_core::vrchat_log::WorldState;
use orbiters_n4n0_core::menu::KeyboardLayout;
use orbiters_n4n0_core::text::{Message, TextSender};
use rosc::OscType;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

/// What the tests copied (they never touch this computer's clipboard).
static CLIPBOARD: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The stand-in app.
#[derive(Default)]
pub(crate) struct TestHost {
    settings: Mutex<Settings>,
    scene: Mutex<Scene>,
    world: Mutex<WorldState>,
    published: Mutex<N4n0Status>,
    /// How often N4N0 handed its state over, and how often it differed from the one before (what the desktop publishes).
    publishes: AtomicUsize,
    changes: AtomicUsize,
    notices: Mutex<Vec<(NoticeKind, String)>>,
    /// A stand-in Orbiters and its credential.
    server: Mutex<Option<(String, String)>>,
    actions: Actions,
    osc: Mutex<Option<OscService>>,
    latest: Mutex<Latest>,
    n4n0: Mutex<Weak<N4n0>>,
    calls: Mutex<Vec<CallRequest>>,
    ringing: Mutex<bool>,
    photos: AtomicUsize,
    /// What each 3D picture was of, as the settings said when it started.
    taken: Mutex<Vec<orbiters_core::settings::PictureMode>>,
    /// VRChat's camera open, as the stand-in VRChat says (`None`: it does not say).
    camera: Mutex<Option<bool>>,
    capturing: Mutex<bool>,
    walk: Mutex<Vec<f32>>,
    motion: Mutex<Vec<Option<(String, OscValue)>>>,
    /// The QR code scanner as the stand-in app runs it: what N4N0 hears from the last one started, how many started and
    /// were dropped; the links it opened.
    qr: Mutex<Option<QrEvents>>,
    qr_started: AtomicUsize,
    qr_dropped: Arc<AtomicUsize>,
    watched: Mutex<Vec<String>>,
    /// The 3D captures: how often they were listed, the shares asked, the friends the stand-in app reads and the pictures
    /// asked for.
    listed: AtomicUsize,
    shares: Mutex<Vec<orbiters_n4n0_core::captures::ShareAsk>>,
    friends: Mutex<Option<Result<Vec<orbiters_n4n0_core::captures::Friend>, String>>>,
    capture_pictures: Mutex<Vec<String>>,
    /// N4N0's files, gone with the test.
    _dir: TempDir,
}

/// A test's folder, gone with it.
#[derive(Default)]
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

impl TestHost {
    fn view(&self) -> N4n0Status {
        self.published.lock().unwrap().clone()
    }

    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    fn scene(&self, change: impl FnOnce(&mut Scene)) {
        change(&mut self.scene.lock().unwrap());
    }

    fn world(&self, change: impl FnOnce(&mut WorldState)) {
        change(&mut self.world.lock().unwrap());
    }

    fn sign_in(&self, base: String, token: &str) {
        *self.server.lock().unwrap() = Some((base, token.into()));
    }

    fn notices(&self) -> Vec<(NoticeKind, String)> {
        self.notices.lock().unwrap().clone()
    }

    /// OSC on the loopback interface, not advertised, to the stand-in VRChat at the settings' port: what it hears goes
    /// to N4N0 as the desktop routes it.
    fn start_osc(self: &Arc<Self>) {
        // The service before goes outside the lock: stopping it waits for its receiving thread.
        let before = self.osc.lock().unwrap().take();
        drop(before);
        let port = self.settings.lock().unwrap().osc.vrchat_port;
        let host = Arc::downgrade(self);
        let service = OscService::start_batched("Orbiters - N4N0 test", &[], Network::Loopback { vrchat_port: port }, false, move |events, at| { // i18n: not shown
            if let Some(h) = host.upgrade() {
                h.on_osc_events(events, at);
            }
        })
        .unwrap();
        *self.osc.lock().unwrap() = Some(service);
    }

    fn stop_osc(&self) {
        let osc = self.osc.lock().unwrap().take();
        drop(osc);
    }

    fn set_vrchat_query(&self, query: SocketAddr) {
        self.osc.lock().unwrap().as_ref().expect("OSC runs").set_vrchat_query(Some(query));
    }

    fn on_osc_events(&self, events: &mut Vec<OscEvent>, at: u64) {
        let n = self.n4n0.lock().unwrap().upgrade();
        let mut messages = Vec::new();
        for event in events.drain(..) {
            match event {
                OscEvent::Parameter { name, value } => {
                    if let Some(n) = &n {
                        n.on_parameter(&name, &value);
                    }
                    messages.push((parameter(&name), vec![value]));
                }
                OscEvent::AvatarChanged { id } => {
                    if let Some(n) = &n {
                        n.on_avatar(id.clone());
                    }
                    messages.push((AVATAR_CHANGE.to_string(), vec![OscValue::Text(id)]));
                }
                OscEvent::Values { address, values } => messages.push((address, values)),
            }
        }
        let mut latest = self.latest.lock().unwrap();
        for (address, values) in messages {
            latest.record(address, values, at);
        }
    }

    /// The app quits: N4N0 closes what it drives, then OSC stops.
    fn shutdown(&self, n: &Shared) {
        n.shutdown();
        self.stop_osc();
    }
}

impl Host for TestHost {
    fn with_settings(&self, read: &mut dyn FnMut(&Settings)) {
        read(&self.settings.lock().unwrap());
    }

    fn update_settings(&self, change: &mut dyn FnMut(&mut Settings)) {
        change(&mut self.settings.lock().unwrap());
    }

    fn scene(&self) -> Scene {
        *self.scene.lock().unwrap()
    }

    fn world(&self) -> WorldState {
        self.world.lock().unwrap().clone()
    }

    fn capturing(&self) -> bool {
        *self.capturing.lock().unwrap()
    }

    fn publish(&self, mut status: N4n0Status) {
        // As the desktop does: the avatar's name is VRChat's log's when N4N0 has none (VRChat's OSCQuery, its states).
        if status.states.avatar_name.is_none() {
            status.states.avatar_name = self.world.lock().unwrap().avatar_name.clone();
        }
        let mut published = self.published.lock().unwrap();
        self.publishes.fetch_add(1, Ordering::Relaxed);
        if *published != status {
            self.changes.fetch_add(1, Ordering::Relaxed);
        }
        *published = status;
    }

    fn notify(&self, kind: NoticeKind, message: String) {
        self.notices.lock().unwrap().push((kind, message));
    }

    fn account(&self) -> Option<(Api, String)> {
        self.server.lock().unwrap().clone().map(|(base, token)| (Api::at(&base), token))
    }

    fn queue(&self, action: Box<dyn FnOnce() + Send>) -> bool {
        self.actions.run(action)
    }

    fn ringing_in(&self) -> bool {
        *self.ringing.lock().unwrap()
    }

    fn call(&self, request: CallRequest) {
        self.calls.lock().unwrap().push(request);
    }

    fn photo_3d(&self) -> anyhow::Result<()> {
        anyhow::ensure!(*self.camera.lock().unwrap() != Some(false), crate::platform::CAMERA_CLOSED);
        self.taken.lock().unwrap().push(self.settings.lock().unwrap().photo_3d_mode);
        self.photos.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn camera_open(&self) -> Option<bool> {
        *self.camera.lock().unwrap()
    }

    fn walk_boost(&self, strength: f32) {
        self.walk.lock().unwrap().push(strength);
    }

    fn qr_scan(&self, events: QrEvents) -> Result<Box<dyn QrScan>, String> {
        self.qr_started.fetch_add(1, Ordering::Relaxed);
        *self.qr.lock().unwrap() = Some(events);
        Ok(Box::new(FakeScan(self.qr_dropped.clone())))
    }

    fn watch(&self, link: &str) -> Result<(), String> {
        self.watched.lock().unwrap().push(link.to_string());
        Ok(())
    }

    fn captures_list(&self) {
        self.listed.fetch_add(1, Ordering::Relaxed);
    }

    fn share_capture(&self, ask: orbiters_n4n0_core::captures::ShareAsk) -> Result<(), String> {
        self.shares.lock().unwrap().push(ask);
        Ok(())
    }

    fn friends(&self) -> Result<Vec<orbiters_n4n0_core::captures::Friend>, String> {
        self.friends.lock().unwrap().clone().unwrap_or(Ok(Vec::new()))
    }

    fn capture_picture(&self, key: &str) -> Option<orbiters_n4n0_core::picture::Rgb> {
        self.capture_pictures.lock().unwrap().push(key.to_string());
        Some(orbiters_n4n0_core::picture::Rgb { width: 16, height: 9, pixels: vec![200; 16 * 9 * 3] })
    }

    fn motion(&self, said: Option<(&str, &OscValue)>) {
        self.motion.lock().unwrap().push(said.map(|(n, v)| (n.to_string(), v.clone())));
    }
}

impl Osc for TestHost {
    fn send(&self, bundles: &[&[Message]]) -> Option<Vec<anyhow::Result<()>>> {
        let osc = self.osc.lock().unwrap();
        let o = osc.as_ref()?;
        Some(
            bundles
                .iter()
                .map(|bundle| match bundle {
                    [(address, arg)] => o.send(address, vec![arg.clone()]),
                    _ => o.send_bundle(bundle.iter().map(|(address, arg)| (address.clone(), vec![arg.clone()])).collect()),
                })
                .collect(),
        )
    }

    fn vrchat_query(&self) -> Option<Option<SocketAddr>> {
        self.osc.lock().unwrap().as_ref().map(OscService::vrchat_query)
    }

    fn latest(&self, addresses: &mut dyn Iterator<Item = String>) -> HashMap<String, OscValue> {
        let latest = self.latest.lock().unwrap();
        addresses.filter_map(|a| latest.first(&a).map(|v| (a, v))).collect()
    }

    fn vrchat_values(&self) -> Vec<(String, OscValue)> {
        match self.vrchat_query() {
            Some(Some(addr)) => orbiters_core::osc::vrchat_values(addr, Duration::from_secs(1)).unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

/// A stand-in QR code scan: counts its drop.
struct FakeScan(Arc<AtomicUsize>);

impl QrScan for FakeScan {}

impl Drop for FakeScan {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl TestHost {
    /// The stand-in scanner sees `seen`.
    fn qr_sees(&self, seen: QrSeen) {
        let events = self.qr.lock().unwrap();
        (events.as_ref().expect("a scan started"))(seen);
    }
}

/// The stand-in device's clipboard (shared with [`CLIPBOARD`]).
struct FakeClipboard;

impl Clipboard for FakeClipboard {
    fn copy(&self, text: &str) -> Result<(), String> {
        CLIPBOARD.lock().unwrap().push(text.to_string());
        Ok(())
    }
}

/// The pages the stand-in browser opened.
#[derive(Clone, Default)]
struct FakeBrowser(Arc<Mutex<Vec<String>>>);

impl Browser for FakeBrowser {
    fn open(&self, url: &str) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(url.to_string());
        Ok(())
    }
}

/// How many press sounds played.
#[derive(Clone, Default)]
struct FakeSounds(Arc<AtomicUsize>);

impl Sounds for FakeSounds {
    fn press(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

struct English;

impl Locale for English {
    fn locale(&self) -> String {
        "en-GB".into()
    }

    fn keyboard(&self) -> KeyboardLayout {
        KeyboardLayout::Azerty
    }
}

/// The stand-in ears: none starts unless the test lets them (`workers`), and what N4N0 asked each.
#[derive(Clone, Default)]
struct FakeEars {
    /// Workers may start.
    allowed: Arc<Mutex<bool>>,
    workers: Arc<Mutex<Vec<FakeWorkerState>>>,
}

/// What a stand-in worker was told, and the calls that make it speak or end.
#[derive(Clone)]
struct FakeWorkerState {
    commands: Arc<Mutex<Vec<VoiceCommand>>>,
    events: Arc<Mutex<Option<VoiceEvents>>>,
    ended: Arc<Mutex<Option<VoiceEnded>>>,
    exit: Arc<Mutex<Option<i32>>>,
    last: Arc<Mutex<Option<String>>>,
    dropped: Arc<Mutex<bool>>,
}

impl FakeWorkerState {
    fn say(&self, event: VoiceEvent) {
        let events = self.events.lock().unwrap();
        (events.as_ref().expect("started"))(event);
    }

    /// The worker stops on its own, with `code` and `last` as its last words.
    fn crash(&self, code: i32, last: &str) {
        *self.exit.lock().unwrap() = Some(code);
        *self.last.lock().unwrap() = Some(last.to_string());
        let ended = self.ended.lock().unwrap().take().expect("running");
        ended();
    }

    fn commands(&self) -> Vec<VoiceCommand> {
        self.commands.lock().unwrap().clone()
    }
}

struct FakeWorker(FakeWorkerState);

impl VoiceWorker for FakeWorker {
    fn send(&mut self, command: &VoiceCommand) -> anyhow::Result<()> {
        self.0.commands.lock().unwrap().push(command.clone());
        Ok(())
    }

    fn exit_code(&mut self) -> Option<i32> {
        *self.0.exit.lock().unwrap()
    }

    fn last_line(&self) -> Option<String> {
        self.0.last.lock().unwrap().clone()
    }
}

impl Drop for FakeWorker {
    fn drop(&mut self) {
        *self.0.dropped.lock().unwrap() = true;
    }
}

impl Ears for FakeEars {
    fn spawn(&self, events: VoiceEvents, ended: VoiceEnded) -> anyhow::Result<Box<dyn VoiceWorker>> {
        anyhow::ensure!(*self.allowed.lock().unwrap(), "N4N0's voice worker was not found (tests)"); // i18n: not shown
        let state = FakeWorkerState {
            commands: Default::default(),
            events: Arc::new(Mutex::new(Some(events))),
            ended: Arc::new(Mutex::new(Some(ended))),
            exit: Default::default(),
            last: Default::default(),
            dropped: Default::default(),
        };
        self.workers.lock().unwrap().push(state.clone());
        Ok(Box::new(FakeWorker(state)))
    }
}

/// The stand-in device, as a test reads it back.
#[derive(Clone, Default)]
struct Device {
    browser: FakeBrowser,
    sounds: FakeSounds,
    ears: FakeEars,
}

/// N4N0 in a stand-in app with these settings, on the stand-in device (not started).
fn n4n0_on(settings: Settings, media: Arc<Watcher>) -> (Arc<TestHost>, Shared, Device) {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("orbiters-n4n0-test-{}-{}", std::process::id(), SERIAL.fetch_add(1, Ordering::Relaxed)));
    let host = Arc::new(TestHost { settings: Mutex::new(settings), _dir: TempDir(dir.clone()), ..Default::default() });
    let device = Device::default();
    let platform = Platform {
        clipboard: Box::new(FakeClipboard),
        browser: Box::new(device.browser.clone()),
        sounds: Box::new(device.sounds.clone()),
        locale: Box::new(English),
        ears: Box::new(device.ears.clone()),
        media,
        mouse: None,
    };
    let n = N4n0::new(host.clone(), host.clone(), platform, Files::in_dir(&dir));
    *host.n4n0.lock().unwrap() = Arc::downgrade(&n);
    (host, n, device)
}

fn n4n0(settings: Settings) -> (Arc<TestHost>, Shared) {
    let (host, n, _) = n4n0_on(settings, Arc::new(Watcher::new(|| Box::new(NoMedia))));
    (host, n)
}

/// A stand-in VRChat: its OSC input port, and its OSCQuery service answering `tree` to every request.
fn vrchat(tree: serde_json::Value) -> (UdpSocket, SocketAddr) {
    let input = UdpSocket::bind("127.0.0.1:0").unwrap();
    input.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let query = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = query.local_addr().unwrap();
    let body = tree.to_string();
    std::thread::spawn(move || {
        for stream in query.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()); // i18n: not shown
            let _ = stream.write_all(format!("{head}{body}").as_bytes());
        }
    });
    (input, addr)
}

/// VRChat's tree for an avatar with the N4N0 prop (8 sync values) and a Hoodie toggle.
fn avatar_tree(id: &str) -> serde_json::Value {
    let mut parameters = serde_json::Map::new();
    let mut put = |name: &str, kind: &str, value: serde_json::Value| {
        parameters.insert(name.replace('/', "_"), json!({ "FULL_PATH": format!("{PARAMETERS_PREFIX}{name}"), "TYPE": kind, "VALUE": [value] }));
    };
    for i in 0..8 {
        put(&format!("N4N0/Text/Sync{i}"), "f", json!(0.0));
    }
    put("N4N0/Text/Pointer", "i", json!(0));
    put("N4N0/Text/Visible", "T", json!(false));
    put("N4N0/State", "i", json!(0));
    put("Hoodie", "T", json!(false));
    put("VelocityX", "f", json!(0.0));
    json!({ "FULL_PATH": "/avatar", "CONTENTS": {
        "change": { "FULL_PATH": "/avatar/change", "TYPE": "s", "VALUE": [id] },
        "parameters": { "FULL_PATH": "/avatar/parameters", "CONTENTS": parameters }
    } })
}

fn received(socket: &UdpSocket, wait: Duration) -> Vec<Message> {
    // N4N0's steps go as bundles: their messages in order.
    fn unpack(packet: rosc::OscPacket, out: &mut Vec<Message>) {
        match packet {
            rosc::OscPacket::Message(m) => out.push((m.addr, m.args[0].clone())),
            rosc::OscPacket::Bundle(b) => b.content.into_iter().for_each(|p| unpack(p, out)),
        }
    }
    let mut out = Vec::new();
    let until = Instant::now() + wait;
    let mut buf = [0u8; 8192];
    while Instant::now() < until {
        if let Ok(n) = socket.recv(&mut buf) {
            if let Ok((_, packet)) = rosc::decoder::decode_udp(&buf[..n]) {
                unpack(packet, &mut out);
            }
        }
    }
    out
}

fn first_row(page: &Page) -> String {
    contract::page_text(page).lines().next().unwrap_or_default().trim_end().to_string()
}

/// N4N0 started on the stand-in avatar of `tree`, OSC with it running.
fn started_on(tree: serde_json::Value) -> (Arc<TestHost>, Shared, UdpSocket) {
    let (input, query) = vrchat(tree);
    let mut settings = Settings::default();
    settings.osc.vrchat_port = input.local_addr().unwrap().port();
    let (t, n) = n4n0(settings);
    t.start_osc();
    t.set_vrchat_query(query);
    n.start();
    (t, n, input)
}

fn n4n0_with_vrchat() -> (Arc<TestHost>, Shared, UdpSocket) {
    let (t, n, input) = started_on(avatar_tree("avtr_n4n0_test"));
    n.refresh_avatar(None, HashMap::new(), 0);
    (t, n, input)
}

#[test]
fn finds_the_prop_and_shows_text_on_its_panel() {
    let (t, n, vrchat) = n4n0_with_vrchat();
    let status = t.view();
    assert_eq!(status.avatar_id.as_deref(), Some("avtr_n4n0_test"));
    assert_eq!(status.prop_sync, Some(8));
    assert_eq!(status.controls, 1, "Hoodie: the system and N4N0 parameters are left out");
    n.command(Command::Say { text: "Bonjour ! Ça va ?".into() });
    let sent = received(&vrchat, Duration::from_millis(2700));
    let mut cells: Page = [9; TEXT_LENGTH];
    let visible = orbiters_n4n0_core::text::apply(&mut cells, 8, &sent);
    assert_eq!(visible, Some(true), "{sent:?}");
    assert_eq!(first_row(&cells), "Bonjour ! Ça va ?");
    assert!(sent.contains(&(parameter(params::STATE), OscType::Int(3))), "N4N0 talks while it shows");
    t.shutdown(&n);
}

#[test]
fn testing_holds_a_state_then_plays_them_all() {
    let (t, n, vrchat) = n4n0_with_vrchat();
    n.command(Command::TestFace { face: Some(N4n0Face::Happy) });
    let sent = received(&vrchat, Duration::from_millis(400));
    assert!(sent.contains(&(parameter(params::STATE), OscType::Int(5))), "held happy: {sent:?}");
    assert_eq!(t.view().test_face, Some(N4n0Face::Happy));
    n.command(Command::TestCycle);
    let sent = received(&vrchat, Duration::from_millis(2600));
    let states: Vec<i32> =
        sent.iter().filter(|(a, _)| *a == parameter(params::STATE)).filter_map(|(_, v)| if let OscType::Int(i) = v { Some(*i) } else { None }).collect();
    assert_eq!(states, vec![0, 1], "idle, then listening two seconds later");
    n.command(Command::TestVisible { on: false });
    let sent = received(&vrchat, Duration::from_millis(300));
    assert!(sent.contains(&(parameter(params::VISIBLE), OscType::Bool(false))));
    t.shutdown(&n);
}

/// A stand-in Orbiters server: answers each turn request with the next of `answers` (server-sent events) and keeps the
/// requests' bodies.
fn orbiters(answers: Vec<String>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = bodies.clone();
    std::thread::spawn(move || {
        let mut answers = answers.into_iter();
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut length = 0usize;
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                if line == "\r\n" {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
                line.clear();
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();
            seen.lock().unwrap().push(serde_json::from_slice::<Value>(&body).unwrap());
            let events = answers.next().unwrap_or_default();
            let mut stream = stream;
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"); // i18n: not shown
            // A NUL between events: the server takes its time there, as a model does between its words.
            for (i, part) in events.split('\0').enumerate() {
                if i > 0 {
                    std::thread::sleep(Duration::from_millis(150));
                }
                let _ = stream.write_all(part.as_bytes());
                let _ = stream.flush();
            }
        }
    });
    (base, bodies)
}

#[test]
fn a_turn_runs_the_tools_it_asks_for_and_answers_on_the_panel() {
    let (t, n, vrchat) = n4n0_with_vrchat();
    let first = format!(
        "event: done\ndata: {}\n\n",
        json!({ "messages": [{ "role": "user", "content": "mets mon sweat" }],
                "tool_calls": [{ "id": "call_1", "name": "set_avatar_control", "input": { "control": "hoodie", "on": true } }],
                "quota": { "used": 1, "limit": 80 } })
    );
    let second = format!(
        "event: text\ndata: {}\n\nevent: text\ndata: {}\n\nevent: done\ndata: {}\n\n",
        json!({ "delta": "Voilà, ton sweat. " }),
        json!({ "delta": "Va voir tes potes." }),
        json!({ "messages": [], "tool_calls": [], "quota": { "used": 2, "limit": 80 } })
    );
    let (base, bodies) = orbiters(vec![first, second]);
    t.sign_in(base, "orbit-test");
    n.ask("N4N0, mets mon sweat".into(), "typed");
    let sent = received(&vrchat, Duration::from_millis(4000));
    assert!(sent.contains(&(parameter("Hoodie"), OscType::Bool(true))), "{sent:?}");
    let mut cells: Page = [0; TEXT_LENGTH];
    orbiters_n4n0_core::text::apply(&mut cells, 8, &sent);
    assert_eq!(first_row(&cells), "Voilà, ton sweat. Va voir tes");
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[0]["input"]["text"], "N4N0, mets mon sweat");
    assert!(bodies[0]["context"]["summary"].as_str().unwrap().contains("\"Hoodie\" (parameter"), "{}", bodies[0]["context"]["summary"]);
    assert_eq!(bodies[0]["context"]["display"]["columns"], 32);
    assert_eq!(bodies[0]["context"]["locale"], "en-GB", "the device's language");
    assert!(bodies[0]["tools"].as_array().unwrap().iter().any(|t| t["name"] == "set_avatar_parameter"), "a bare avatar gets its parameters by name");
    assert_eq!(bodies[1]["tool_results"][0]["id"], "call_1");
    assert_eq!(bodies[1]["tool_results"][0]["is_error"], false);
    assert_eq!(bodies[1]["messages"][0]["content"], "mets mon sweat", "the conversation comes back as the server returned it");
    drop(bodies);
    let status = t.view();
    assert_eq!(status.quota.map(|q| q.used), Some(2));
    let whos: Vec<N4n0Who> = status.lines.iter().map(|l| l.who).collect();
    assert_eq!(whos, vec![N4n0Who::You, N4n0Who::Note, N4n0Who::N4n0]);
    assert!(!status.thinking);
    t.shutdown(&n);
}

/// Waits until `until` holds of the status, and returns it.
fn status_when(t: &TestHost, until: impl Fn(&N4n0Status) -> bool) -> N4n0Status {
    let start = Instant::now();
    loop {
        let status = t.view();
        if until(&status) {
            return status;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "never came: {:?}", status.lines);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn delta(text: &str) -> String {
    format!("event: text\ndata: {}\n\n\0", json!({ "delta": text })) // i18n: not shown
}

fn the_answers(status: &N4n0Status) -> Vec<(String, bool)> {
    status.lines.iter().filter(|l| l.who == N4n0Who::N4n0).map(|l| (l.text.clone(), l.streaming)).collect()
}

/// Enzo: "shows a stream of the text as it's being received". Each window and sheet draws `lines`: the answer is there as
/// it arrives, one line that grows, and the whole answer takes its place, never beside it.
#[test]
fn an_answer_shows_as_it_arrives_and_once_when_whole() {
    let (t, n, _vrchat) = n4n0_with_vrchat();
    let done = format!("event: done\ndata: {}\n\n", json!({ "messages": [], "tool_calls": [] }));
    let (base, _) = orbiters(vec![format!("{}{}{}{done}", delta("It is "), delta("sunny "), delta("and warm."))]);
    t.sign_in(base, "orbit-test");
    n.ask("Is it nice out?".into(), "typed");
    let first = status_when(&t, |s| !the_answers(s).is_empty());
    assert_eq!(the_answers(&first), vec![("It is".to_string(), true)], "the first words, as they came");
    assert!(first.thinking, "still answering");
    let more = status_when(&t, |s| the_answers(s).first().is_some_and(|a| a.0 == "It is sunny"));
    assert_eq!(the_answers(&more).len(), 1, "the same line, grown");
    let whole = status_when(&t, |s| !s.thinking);
    assert_eq!(the_answers(&whole), vec![("It is sunny and warm.".to_string(), false)], "once, and whole");
    let whos: Vec<N4n0Who> = whole.lines.iter().map(|l| l.who).collect();
    assert_eq!(whos, vec![N4n0Who::You, N4n0Who::N4n0]);
    t.shutdown(&n);
}

/// A round of words before a tool, then the answer: what a tool says stays above what arrives, and one answer is left.
#[test]
fn words_before_a_tool_give_way_to_the_answer() {
    let (t, n, _vrchat) = n4n0_with_vrchat();
    let tool = format!(
        "event: done\ndata: {}\n\n",
        json!({ "messages": [], "tool_calls": [{ "id": "call_1", "name": "set_avatar_control", "input": { "control": "hoodie", "on": true } }] })
    );
    let done = format!("event: done\ndata: {}\n\n", json!({ "messages": [], "tool_calls": [] }));
    let (base, _) = orbiters(vec![format!("{}{tool}", delta("One moment.")), format!("{}{}{done}", delta("Your hoodie "), delta("is on."))]);
    t.sign_in(base, "orbit-test");
    n.ask("Hoodie please".into(), "typed");
    let arriving = status_when(&t, |s| the_answers(s).first().is_some_and(|a| a.0 == "Your hoodie"));
    assert_eq!(the_answers(&arriving).len(), 1, "the next round's words replace the last round's: {:?}", arriving.lines);
    let whole = status_when(&t, |s| !s.thinking);
    assert_eq!(the_answers(&whole), vec![("Your hoodie is on.".to_string(), false)]);
    assert_eq!(whole.lines.iter().filter(|l| l.streaming).count(), 0);
    t.shutdown(&n);
}

/// The stream broken halfway: what had arrived goes, and the error says why.
#[test]
fn an_answer_that_breaks_off_leaves_no_half_line() {
    let (t, n, _vrchat) = n4n0_with_vrchat();
    let error = format!("event: error\ndata: {}\n\n", json!({ "code": "overloaded", "message": "Too many people at once." }));
    let (base, _) = orbiters(vec![format!("{}{error}", delta("Half an"))]);
    t.sign_in(base, "orbit-test");
    n.ask("Hello".into(), "typed");
    status_when(&t, |s| !the_answers(s).is_empty());
    let over = status_when(&t, |s| !s.thinking);
    assert!(the_answers(&over).is_empty(), "{:?}", over.lines);
    assert!(over.lines.iter().any(|l| l.who == N4n0Who::Error && l.text == "Too many people at once."), "{:?}", over.lines);
    t.shutdown(&n);
}

/// New conversation while an answer arrives: nothing of it comes back, not a line of what was still on its way.
#[test]
fn a_new_conversation_stops_what_is_arriving() {
    let (t, n, _vrchat) = n4n0_with_vrchat();
    let (base, _) = orbiters(vec![format!("{}{}{}", delta("One "), delta("two "), delta("three"))]);
    t.sign_in(base, "orbit-test");
    n.ask("Count".into(), "typed");
    status_when(&t, |s| !the_answers(s).is_empty());
    n.command(Command::NewConversation);
    let cleared = status_when(&t, |s| s.lines.is_empty() && !s.thinking);
    assert!(cleared.lines.is_empty());
    std::thread::sleep(Duration::from_millis(500));
    assert!(t.view().lines.is_empty(), "{:?}", t.view().lines);
    t.shutdown(&n);
}

/// What the panel holds after everything VRChat received lately.
fn panel_after(vrchat: &UdpSocket, wait: Duration, cells: &mut Page) -> usize {
    let sent = received(vrchat, wait);
    orbiters_n4n0_core::text::apply(cells, 8, &sent);
    sent.iter().filter(|(a, v)| *a == parameter(params::TEXT_POINTER) && matches!(v, OscType::Int(p) if *p > 0 && *p != POINTER_CLEAR)).count()
}

/// Enzo: the answer grows on the avatar's panel too. A slow server, one word every 450 ms: the panel holds the first words
/// long before the answer is whole, then exactly the answer, in OSC as VRChat receives it.
#[test]
fn a_slow_answer_grows_on_the_panel_over_osc() {
    let (t, n, vrchat) = n4n0_with_vrchat();
    let slowly = |text: &str| format!("{}\0\0", delta(text));
    let done = format!("event: done\ndata: {}\n\n", json!({ "messages": [], "tool_calls": [] }));
    let (base, _) = orbiters(vec![format!("{}{}{}{}{}{}{done}", slowly("Hello "), slowly("from "), slowly("the "), slowly("slow "), slowly("server "), slowly("today."))]);
    t.sign_in(base, "orbit-test");
    let mut cells: Page = [0; TEXT_LENGTH];
    n.ask("Say hello".into(), "typed");
    let asked = Instant::now();
    let mut first_words = None;
    while t.view().thinking || asked.elapsed() < Duration::from_millis(300) {
        panel_after(&vrchat, Duration::from_millis(100), &mut cells);
        if first_words.is_none() && first_row(&cells).starts_with("Hello") {
            first_words = Some(asked.elapsed());
        }
        assert!(asked.elapsed() < Duration::from_secs(10), "never ended");
    }
    let answered = asked.elapsed();
    let first = first_words.expect("the panel showed words before the answer was whole");
    println!("slow server: first words on the panel after {first:?}, the answer whole after {answered:?}");
    assert!(first + Duration::from_secs(1) < answered, "first words after {first:?}, whole after {answered:?}");
    panel_after(&vrchat, Duration::from_millis(1500), &mut cells);
    assert_eq!(first_row(&cells), "Hello from the slow server");
    assert_eq!(contract::page_text(&cells).lines().nth(1).unwrap_or_default().trim_end(), "today.");
    t.shutdown(&n);
}

/// A server far faster than OSC, a whole page in one burst, many deltas: the panel goes at its own pace, a block a step, no
/// faster, the whole answer's first page exactly, and a shorter final answer than what had arrived leaves nothing behind.
#[test]
fn a_bursty_answer_keeps_to_the_panel_s_pace_and_ends_exactly() {
    let (t, n, vrchat) = n4n0_with_vrchat();
    let words: Vec<String> = (0..40).map(|i| format!("word{i}")).collect();
    let burst: String = words.iter().map(|w| format!("event: text\ndata: {}\n\n", json!({ "delta": format!("{w} ") }))).collect();
    let done = format!("event: done\ndata: {}\n\n", json!({ "messages": [], "tool_calls": [] }));
    let (base, _) = orbiters(vec![format!("{burst}\0{done}")]);
    t.sign_in(base, "orbit-test");
    let mut cells: Page = [0; TEXT_LENGTH];
    n.ask("Count".into(), "typed");
    let start = Instant::now();
    let blocks = panel_after(&vrchat, Duration::from_secs(5), &mut cells);
    let per_second = blocks as f32 / start.elapsed().as_secs_f32();
    println!("bursty server: {blocks} blocks in {:?}: {per_second:.2} a second, at the step of 300 ms at most 3.33", start.elapsed());
    assert!(per_second <= 1000.0 / contract::DEFAULT_STEP_MS as f32, "{per_second} blocks a second");
    let answer = words.join(" ");
    let expected = contract::pages(&answer)[0];
    panel_after(&vrchat, Duration::from_secs(5), &mut cells);
    assert_eq!(cells, expected, "the first page, exactly");
    t.shutdown(&n);

    // Words arriving, then the turn's whole answer much shorter than them: every cell is the short answer's.
    let (t, n, vrchat) = n4n0_with_vrchat();
    let long = "This first round goes on for quite a while with very many words that reach far down the panel ";
    let tool = format!("event: done\ndata: {}\n\n", json!({ "messages": [], "tool_calls": [{ "id": "call_1", "name": "set_avatar_control", "input": { "control": "hoodie", "on": true } }] }));
    let (base, _) = orbiters(vec![format!("{}{tool}", delta(long)), format!("{}event: done\ndata: {}\n\n", delta("Done."), json!({ "messages": [], "tool_calls": [] }))]);
    t.sign_in(base, "orbit-test");
    let mut cells: Page = [0; TEXT_LENGTH];
    n.ask("Hoodie".into(), "typed");
    assert!(status_when(&t, |s| !s.thinking).lines.iter().any(|l| l.text == "Done."));
    let (start, wanted) = (Instant::now(), contract::pages("Done.")[0]);
    while cells != wanted {
        assert!(start.elapsed() < Duration::from_secs(10), "{:?}", contract::page_text(&cells));
        panel_after(&vrchat, Duration::from_millis(100), &mut cells);
    }
    t.shutdown(&n);
}

/// N4N0's menu open: it stands where the panel would show, and its presses and pages use the same channel. An answer arriving
/// leaves the panel out of it: the channel stays the screen's, and the answer comes whole on the menu at the end, as before.
#[test]
fn an_answer_arriving_leaves_the_open_menu_the_channel() {
    let (t, n, _vrchat) = n4n0_on_avatar(menu_tree("avtr_menu", true));
    n.on_parameter(params::MENU, &OscValue::Bool(true));
    assert!(until(|| menu_is_open(&n)));
    let done = format!("event: done\ndata: {}\n\n", json!({ "messages": [], "tool_calls": [] }));
    let (base, _) = orbiters(vec![format!("{}{}{}{done}", delta("Some "), delta("words "), delta("arriving."))]);
    t.sign_in(base, "orbit-test");
    n.ask("Hello".into(), "typed");
    let start = Instant::now();
    while t.view().thinking || start.elapsed() < Duration::from_millis(100) {
        {
            let rt = n.n4n0.lock().unwrap();
            let panel = rt.sender.as_ref().expect("the prop's panel");
            assert!(!panel.busy() && !panel.wants_step(Instant::now() + Duration::from_secs(1)), "the panel takes no turn on the channel while the menu is open");
        }
        assert!(start.elapsed() < Duration::from_secs(8), "never ended");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(until(|| n.n4n0.lock().unwrap().screen.answer_showing()), "whole, on the menu");
    t.shutdown(&n);

    // The menu opened with words already on the panel: they leave the panel for the menu at its next step, and no more follow.
    let (t, n, _vrchat) = n4n0_on_avatar(menu_tree("avtr_menu", true));
    let done = format!("event: done\ndata: {}\n\n", json!({ "messages": [], "tool_calls": [] }));
    let (base, _) = orbiters(vec![format!("{}\0\0\0{}\0\0\0{}{done}", delta("Some words "), delta("arriving "), delta("slowly."))]);
    t.sign_in(base, "orbit-test");
    n.ask("Hello".into(), "typed");
    assert!(until(|| n.n4n0.lock().unwrap().sender.as_ref().is_some_and(TextSender::streaming)));
    n.on_parameter(params::MENU, &OscValue::Bool(true));
    assert!(until(|| menu_is_open(&n)));
    assert!(until(|| !n.n4n0.lock().unwrap().sender.as_ref().is_some_and(TextSender::busy)), "off the panel");
    while t.view().thinking {
        assert!(!n.n4n0.lock().unwrap().sender.as_ref().is_some_and(TextSender::streaming), "and it stays off");
        std::thread::sleep(Duration::from_millis(10));
    }
    t.shutdown(&n);
}

#[test]
fn the_wake_word_starts_a_turn_and_other_sentences_stay_here() {
    let mut settings = Settings::default();
    settings.n4n0.wake_word = true;
    let (t, n) = n4n0(settings);
    let (base, bodies) = orbiters(vec![format!("event: done\ndata: {}\n\n", json!({ "messages": [], "tool_calls": [] }))]);
    t.sign_in(base, "orbit-test");
    // Heard while the wake word is on, but not for N4N0: nothing goes anywhere. A sentence that only starts like a
    // misheard "Nano" is ordinary speech too.
    n.heard("I saw a nano drone in that world".into(), false, false);
    n.heard("Nana, passe-moi le sel.".into(), false, false);
    // The wake word: the request carries what follows.
    n.heard("Nano, mets mon sweat à capuche.".into(), false, false);
    let started = Instant::now();
    while bodies.lock().unwrap().is_empty() && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["input"]["text"], "mets mon sweat à capuche.");
    assert_eq!(bodies[0]["input"]["via"], "voice");
    assert!(bodies[0]["context"]["display"].is_null(), "no N4N0 prop: no panel limits");
}

#[test]
fn a_held_sentence_reaches_n4n0_after_the_button_is_let_go() {
    let (t, n) = n4n0(Settings::default());
    let done = format!("event: done\ndata: {}\n\n", json!({ "messages": [], "tool_calls": [] }));
    let (base, bodies) = orbiters(vec![done]);
    t.sign_in(base, "orbit-test");
    n.n4n0.lock().unwrap().voice.installed = true;
    // Pressed, then let go: the worker transcribes only now, so its sentence arrives after the button is up.
    n.app_talk(true, false);
    assert!(n.n4n0.lock().unwrap().listening.is_some());
    n.app_talk(false, false);
    assert!(n.n4n0.lock().unwrap().listening.is_none());
    let heard = |text: &str, listen: u64| VoiceEvent::Heard { text: text.into(), for_n4n0: true, seconds: 1.2, decode_ms: 90, listen };
    n.on_voice_event(heard("mets mon sweat", 1));
    let started = Instant::now();
    while bodies.lock().unwrap().is_empty() && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(bodies.lock().unwrap().len(), 1, "the held sentence went to N4N0");
    assert_eq!(bodies.lock().unwrap()[0]["input"]["text"], "mets mon sweat");
    // The overlay goes away while its Talk is held: nothing it heard goes anywhere, even if the worker answers.
    n.app_talk(true, true);
    n.link_closed(true);
    assert!(n.n4n0.lock().unwrap().listening.is_none());
    n.on_voice_event(heard("this stays here", 2));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(bodies.lock().unwrap().len(), 1, "a cancelled listen's sentence is dropped");
    t.shutdown(&n);
}

/// A `done` event (server-sent) with this conversation and these tool calls.
fn done(messages: Value, tool_calls: Value) -> String {
    format!("event: done\ndata: {}\n\n", json!({ "messages": messages, "tool_calls": tool_calls })) // i18n: not shown
}

/// Waits until the turn under way ended and the server got `requests` of them.
fn turn_ended(n: &Shared, bodies: &Arc<Mutex<Vec<Value>>>, requests: usize) {
    let started = Instant::now();
    while (bodies.lock().unwrap().len() < requests || n.n4n0.lock().unwrap().thinking()) && started.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(bodies.lock().unwrap().len(), requests);
    assert!(!n.n4n0.lock().unwrap().thinking());
}

#[test]
fn a_failed_round_leaves_the_conversation_as_it_was() {
    let (t, n, _vrchat) = n4n0_with_vrchat();
    let before = vec![json!({ "role": "user", "content": "salut" }), json!({ "role": "assistant", "content": "yo" })];
    n.n4n0.lock().unwrap().conversation = before.clone();
    // The first request asks for a tool; the round that brings its result fails (the quota used up mid-turn).
    let call = json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "call_1", "name": "set_avatar_control", "input": {} }] });
    let first = done(
        json!([before[0], before[1], { "role": "user", "content": "mets mon sweat" }, call]),
        json!([{ "id": "call_1", "name": "set_avatar_control", "input": { "control": "hoodie", "on": true } }]),
    );
    let failed = format!("event: error\ndata: {}\n\n", json!({ "code": "quota_exhausted", "message": "Free quota used up until midnight." }));
    let (base, bodies) = orbiters(vec![first, failed, done(json!([{ "role": "user", "content": "et alors ?" }]), json!([]))]);
    t.sign_in(base, "orbit-test");
    n.ask("mets mon sweat".into(), "typed");
    turn_ended(&n, &bodies, 2);
    assert_eq!(n.n4n0.lock().unwrap().conversation, before, "no call without its result stays in the conversation");
    assert_eq!(t.view().lines.last().map(|l| (l.who, l.text.clone())), Some((N4n0Who::Error, "Free quota used up until midnight.".into())));
    // The next turn starts from the conversation as it was, and hears what the failed one did.
    n.ask("et alors ?".into(), "typed");
    turn_ended(&n, &bodies, 3);
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies[2]["messages"], json!(before));
    let summary = bodies[2]["context"]["summary"].as_str().unwrap();
    assert!(summary.contains("did not arrive") && summary.contains("turn on \"Hoodie\""), "{summary}");
    drop(bodies);
    assert!(n.n4n0.lock().unwrap().notes.is_empty(), "heard, then dropped");
    t.shutdown(&n);
}

#[test]
fn what_lasts_waits_for_the_owner_s_yes_and_n4n0_s_own_parameters_are_never_reached() {
    let (t, n, _vrchat) = n4n0_with_vrchat();
    let calls = json!([
        { "id": "c1", "name": "set_avatar_parameter", "input": { "name": "N4N0/Call/Go", "value": 1 } },
        { "id": "c2", "name": "hold_avatar_control", "input": { "control": "hoodie", "on": true } },
        { "id": "c3", "name": "set_afk_message", "input": { "text": "brb, tea", "enabled": true } }
    ]);
    let first = done(json!([{ "role": "user", "content": "q" }]), calls);
    let second = format!("event: text\ndata: {}\n\n{}", json!({ "delta": "Say yes and it's done." }), done(json!([{ "role": "user", "content": "q" }]), json!([])));
    let (base, bodies) = orbiters(vec![first, second]);
    t.sign_in(base, "orbit-test");
    n.ask("garde mon sweat et mets un message".into(), "typed");
    turn_ended(&n, &bodies, 2);
    {
        let bodies = bodies.lock().unwrap();
        let results = bodies[1]["tool_results"].as_array().unwrap();
        assert_eq!(results[0]["is_error"], true, "N4N0's own parameters are never reached: {results:?}");
        assert!(results[1]["content"].as_str().unwrap().contains("waiting_for_owner"));
        assert!(results[2]["content"].as_str().unwrap().contains("waiting_for_owner"));
    }
    let settings = t.settings().n4n0;
    assert!(settings.pins.is_empty() && settings.afk_message != "brb, tea", "nothing done before the yes");
    let status = t.view();
    assert!(status.confirmations.iter().any(|c| c.approve_url.starts_with(LOCAL_APPROVAL)));
    assert!(status.lines.iter().any(|l| l.text.starts_with("N4N0 asks to keep \"Hoodie\" on")), "{:?}", status.lines);
    assert!(status.panel.as_deref().is_some_and(|p| p.starts_with("Say yes and it's done.")), "{:?}", status.panel);
    // The owner's yes, typed: done here, without the assistant.
    n.ask("oui vas-y".into(), "typed");
    let settings = t.settings().n4n0;
    assert_eq!(settings.pins.iter().map(|p| p.parameter.as_str()).collect::<Vec<_>>(), vec!["Hoodie"]);
    assert_eq!((settings.afk_message.as_str(), settings.afk_message_enabled), ("brb, tea", true));
    assert_eq!(bodies.lock().unwrap().len(), 2, "the yes never went to the server");
    assert!(t.view().confirmations.is_empty());
    assert!(n.n4n0.lock().unwrap().notes.iter().any(|n| n.contains("said yes")), "the assistant hears of it next time");
    t.shutdown(&n);
}

/// A stand-in VRChat whose OSCQuery service lists no avatar parameters until `after` (as while it loads a heavy
/// avatar), then `tree`.
fn vrchat_late(tree: serde_json::Value, after: Duration) -> (UdpSocket, SocketAddr) {
    let input = UdpSocket::bind("127.0.0.1:0").unwrap();
    input.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let query = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = query.local_addr().unwrap();
    let ready = Instant::now() + after;
    std::thread::spawn(move || {
        for stream in query.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = if Instant::now() < ready { json!({ "FULL_PATH": "/avatar", "CONTENTS": {} }).to_string() } else { tree.to_string() };
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()); // i18n: not shown
            let _ = stream.write_all(format!("{head}{body}").as_bytes());
        }
    });
    (input, addr)
}

#[test]
fn an_unread_tree_keeps_the_panel_and_is_read_again_and_vrchat_closing_forgets_the_avatar() {
    let (input, query) = vrchat_late(avatar_tree("avtr_late"), Duration::from_millis(300));
    let mut settings = Settings::default();
    settings.osc.vrchat_port = input.local_addr().unwrap().port();
    let (t, n) = n4n0(settings);
    t.start_osc();
    t.set_vrchat_query(query);
    {
        // The last avatar had the prop.
        let mut rt = n.n4n0.lock().unwrap();
        rt.avatar_id = Some("avtr_late".into());
        rt.sender = Some(TextSender::new(8, Duration::from_millis(300)));
    }
    n.refresh_avatar(Some("avtr_late".into()), HashMap::new(), 0);
    assert_eq!(t.view().prop_sync, Some(8), "not read is not gone: the panel stays");
    assert!(n.n4n0.lock().unwrap().tree_unknown);
    let started = Instant::now();
    while n.n4n0.lock().unwrap().tree_unknown && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = t.view();
    assert_eq!((status.controls, status.prop_sync), (1, Some(8)), "read again a moment later");
    // VRChat closes: the avatar, and what VRChat said of it, go with it.
    n.start();
    n.n4n0.lock().unwrap().afk = true;
    t.scene(|s| s.vrchat = true);
    // Once the N4N0 thread saw VRChat run (a busy computer can take a while to run it).
    let started = Instant::now();
    while !n.n4n0.lock().unwrap().vrchat_running && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    t.scene(|s| s.vrchat = false);
    let started = Instant::now();
    while n.n4n0.lock().unwrap().avatar_id.is_some() && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let rt = n.n4n0.lock().unwrap();
    assert!(rt.avatar_id.is_none() && rt.sender.is_none() && !rt.afk && rt.controls.is_empty());
    drop(rt);
    t.shutdown(&n);
}

#[test]
fn an_avatar_change_keeps_what_is_not_the_avatar_s_and_pins_are_not_put_off() {
    let mut settings = Settings::default();
    settings.n4n0.pins.push(Pin { parameter: "Hoodie".into(), value: OscValue::Bool(true) });
    let (_t, n) = n4n0(settings);
    let later = Instant::now() + Duration::from_secs(5);
    n.n4n0.lock().unwrap().send_at(
        later,
        [("/input/Voice".to_string(), OscType::Int(0)), (parameter("Hoodie"), OscType::Bool(true)), ("/usercamera/Mode".to_string(), OscType::Int(0))],
    );
    n.on_avatar("avtr_next".into());
    let left: Vec<String> = n.n4n0.lock().unwrap().outbox.iter().map(|(_, (a, _))| a.clone()).collect();
    assert_eq!(left, vec!["/input/Voice", "/usercamera/Mode"], "the mute press's end and the camera put back still go");
    // A held parameter that keeps moving is put back all the same: its deadline is not pushed back by every change.
    n.on_parameter("Hoodie", &OscValue::Bool(false));
    let first = n.n4n0.lock().unwrap().pins_due;
    assert!(first.is_some());
    std::thread::sleep(Duration::from_millis(30));
    n.on_parameter("Hoodie", &OscValue::Bool(false));
    assert_eq!(n.n4n0.lock().unwrap().pins_due, first);
    // N4N0's own writes coming back are let go at once (the panel's and the screen's, hundreds a second).
    assert!(N4n0::echo("N4N0/Text/Sync3") && N4n0::echo("N4N0/Screen/Value7") && N4n0::echo("N4N0/State"));
    assert!(!N4n0::echo("N4N0/Screen/Touch/L0") && !N4n0::echo("N4N0/Talk"));
    // VRChat's contacts are VRChat's, not echoes; the drop flag and the size are the app's own coming back.
    assert!(!N4n0::echo("N4N0/Screen/Aim/L0") && !N4n0::echo("N4N0/Screen/Others3"));
    assert!(N4n0::echo("N4N0/Screen/Size"));
    assert!(!N4n0::echo("N4N0/Card/TouchL") && N4n0::echo("N4N0/Screen/Drop"));
}

#[test]
fn a_lost_send_has_the_text_sent_whole_again() {
    let (t, n, vrchat) = n4n0_with_vrchat();
    n.command(Command::Say { text: "Hi, I'm N4N0. Yes, the one in the menu.".into() });
    let sent = received(&vrchat, Duration::from_millis(3200));
    let mut panel: Page = [0; TEXT_LENGTH];
    orbiters_n4n0_core::text::apply(&mut panel, 8, &sent);
    assert_eq!(first_row(&panel), "Hi, I'm N4N0. Yes, the one in");
    // OSC stops (its settings changed): what is sent meanwhile is lost, and the sender is told.
    t.stop_osc();
    let text = "Hi, I'm N4N0. Yes, the one in the menu! And no, I am not your friend, but I will help you all the same today.";
    n.command(Command::Say { text: text.into() });
    std::thread::sleep(Duration::from_millis(700));
    // Back, while VRChat reloaded the avatar without a word: its panel holds nothing. It is cleared, then sent whole.
    t.start_osc();
    let sent = received(&vrchat, Duration::from_millis(3500));
    assert!(sent.contains(&(parameter(params::TEXT_POINTER), OscType::Int(POINTER_CLEAR))), "cleared first: {sent:?}");
    let mut wiped: Page = [0; TEXT_LENGTH];
    orbiters_n4n0_core::text::apply(&mut wiped, 8, &sent);
    assert_eq!(first_row(&wiped), "Hi, I'm N4N0. Yes, the one in");
    t.shutdown(&n);
}

/// The stand-in avatar's tree with the Menu toggle, and the projected screen (`screen`) or not.
fn menu_tree(id: &str, screen: bool) -> serde_json::Value {
    let mut tree = avatar_tree(id);
    let parameters = &mut tree["CONTENTS"]["parameters"]["CONTENTS"];
    let mut put = |name: &str, kind: &str, value: serde_json::Value| {
        parameters[name.replace('/', "_")] = json!({ "FULL_PATH": format!("{PARAMETERS_PREFIX}{name}"), "TYPE": kind, "VALUE": [value] });
    };
    put(params::MENU, "T", json!(false));
    if screen {
        put(params::SCREEN_OPEN, "T", json!(false));
        put(params::SCREEN_POINTER, "i", json!(0));
    }
    tree
}

/// N4N0 on the stand-in avatar of `tree`, its startup messages to VRChat drained. Its ears are taken as not installed
/// (whatever this computer has): opening the menu starts no listen, which would keep the menu from closing idle.
fn n4n0_on_avatar(tree: serde_json::Value) -> (Arc<TestHost>, Shared, UdpSocket) {
    let (t, n, input, _) = n4n0_started(tree);
    (t, n, input)
}

/// [`n4n0_on`], with the startup messages it sent VRChat.
fn n4n0_started(tree: serde_json::Value) -> (Arc<TestHost>, Shared, UdpSocket, Vec<Message>) {
    let (t, n, input) = started_on(tree);
    n.n4n0.lock().unwrap().voice.installed = false;
    n.refresh_avatar(None, HashMap::new(), 0);
    let sent = received(&input, Duration::from_millis(400));
    (t, n, input, sent)
}

/// Waits up to two seconds for `done`.
fn until(done: impl Fn() -> bool) -> bool {
    let started = Instant::now();
    while !done() && started.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    done()
}

/// What `sent` set `N4N0/Menu` to, in order.
fn menu_values(sent: &[Message]) -> Vec<bool> {
    sent.iter().filter(|(a, _)| *a == parameter(params::MENU)).filter_map(|(_, v)| if let OscType::Bool(b) = v { Some(*b) } else { None }).collect()
}

fn menu_is_open(n: &Shared) -> bool {
    n.n4n0.lock().unwrap().screen.menu.open
}

/// A measure, not a check (architecture review 2026-11-10, DRT-1): how often N4N0 hands the app a state that differs
/// from the last, which the desktop publishes to its window and overlay each time. Printed (`--nocapture`); the only
/// thing held is that a screen nobody touches does not publish without end.
#[test]
fn how_often_n4n0_publishes_its_state() {
    let (t, n, _vrchat) = n4n0_on_avatar(menu_tree("avtr_publish_rate", true));
    let measure = |label: &str, seconds: f32, each: &dyn Fn(f32)| {
        let (calls, changes) = (t.publishes.load(Ordering::Relaxed), t.changes.load(Ordering::Relaxed));
        let started = Instant::now();
        while started.elapsed().as_secs_f32() < seconds {
            each(started.elapsed().as_secs_f32());
            std::thread::sleep(Duration::from_millis(11));
        }
        let took = started.elapsed().as_secs_f32();
        let (calls, changes) = (t.publishes.load(Ordering::Relaxed) - calls, t.changes.load(Ordering::Relaxed) - changes);
        println!("publish rate, {label}: {:.1} states handed over a second, {:.1} of them changed", calls as f32 / took, changes as f32 / took);
        changes as f32 / took
    };
    measure("screen closed, nothing happening", 3.0, &|_| {});
    n.menu_open(true);
    assert!(until(|| menu_is_open(&n)));
    std::thread::sleep(Duration::from_millis(500));
    let idle = measure("screen open, nobody touching it", 3.0, &|_| {});
    assert!(idle < 5.0, "an open screen nobody touches publishes {idle} times a second");
    // The owner's right index finger moving over the screen, as VRChat sends its four contacts at 90 Hz.
    measure("screen open, a finger moving over it", 3.0, &|at| {
        for i in 0..4 {
            let value = 0.5 + 0.4 * (at * 3.0 + i as f32).sin();
            n.on_parameter(&format!("{}R{i}", params::SCREEN_TOUCH_PREFIX), &OscValue::Float(value));
        }
    });
    // N4N0 answering on its panel while the screen is open: the text goes up a step at a time.
    n.command(Command::Say { text: "Hi, I'm N4N0. Yes, the one in the menu! And no, I am not your friend, but I will help you all the same today. Ask me anything you like, and I will do my best.".into() });
    measure("screen open, N4N0 writing on its panel", 3.0, &|_| {});
    // The avatar walking: VRChat's own VelocityMagnitude, as the host hears it (the desktop keeps it to 0.25 m/s).
    let said = t.motion.lock().unwrap().len();
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        let at = started.elapsed().as_secs_f32();
        n.on_parameter("VelocityMagnitude", &OscValue::Float(1.5 + 1.5 * (at * 2.0).sin()));
        std::thread::sleep(Duration::from_millis(11));
    }
    println!("publish rate, walking: VelocityMagnitude reached the host {:.1} times a second", (t.motion.lock().unwrap().len() - said) as f32 / 2.0);
    t.shutdown(&n);
}

#[test]
fn the_menu_toggle_opens_n4n0_s_menu_and_is_kept_in_step() {
    let (t, n, vrchat) = n4n0_on_avatar(menu_tree("avtr_menu", true));
    assert!(t.view().screen, "the prop has the projected screen");
    // The owner turns the Menu toggle on: the menu opens on the screen, as the binding opens it.
    n.on_parameter(params::MENU, &OscValue::Bool(true));
    assert!(until(|| menu_is_open(&n)));
    let sent = received(&vrchat, Duration::from_millis(400));
    assert!(sent.contains(&(parameter(params::SCREEN_OPEN), OscType::Bool(true))));
    assert_eq!(menu_values(&sent), Vec::<bool>::new(), "the toggle says so already");
    // Its echo, and the toggle on again, change nothing.
    n.on_parameter(params::MENU, &OscValue::Bool(true));
    std::thread::sleep(Duration::from_millis(100));
    assert!(menu_is_open(&n));
    // Idle for too long, the menu closes on its own: the toggle is turned off.
    n.n4n0.lock().unwrap().screen.idle_long_ago();
    n.wake();
    assert!(until(|| !menu_is_open(&n)));
    let sent = received(&vrchat, Duration::from_millis(400));
    assert_eq!(menu_values(&sent), vec![false]);
    assert!(sent.contains(&(parameter(params::SCREEN_OPEN), OscType::Bool(false))));
    // VRChat sends our false back: it closes nothing more, and opens nothing.
    n.on_parameter(params::MENU, &OscValue::Bool(false));
    std::thread::sleep(Duration::from_millis(150));
    assert!(!menu_is_open(&n));
    // On, then off at once in VRChat (well within a second of our own off): the menu opens, then closes.
    n.on_parameter(params::MENU, &OscValue::Bool(true));
    assert!(until(|| menu_is_open(&n)));
    n.on_parameter(params::MENU, &OscValue::Bool(false));
    assert!(until(|| !menu_is_open(&n)));
    assert_eq!(menu_values(&received(&vrchat, Duration::from_millis(300))), Vec::<bool>::new(), "the toggle said so itself");
    t.shutdown(&n);
}

#[test]
fn quitting_closes_n4n0_s_screen_and_its_menu_toggle() {
    let (t, n, vrchat) = n4n0_on_avatar(menu_tree("avtr_quit", true));
    n.menu_open(true);
    assert!(until(|| menu_is_open(&n)));
    let opened = received(&vrchat, Duration::from_millis(300));
    assert!(opened.contains(&(parameter(params::SCREEN_OPEN), OscType::Bool(true))), "{opened:?}");
    // The app quits with the menu on the avatar's screen: both go off, so nothing stays open without the app.
    t.shutdown(&n);
    let sent = received(&vrchat, Duration::from_millis(300));
    assert!(sent.contains(&(parameter(params::SCREEN_OPEN), OscType::Bool(false))), "{sent:?}");
    // The menu's own toggle may say so too as it closes: every word is off.
    let menu = menu_values(&sent);
    assert!(!menu.is_empty() && menu.iter().all(|on| !on), "{sent:?}");
    let last_open = sent.iter().rposition(|m| m.0 == parameter(params::SCREEN_OPEN)).unwrap();
    assert_eq!(sent[last_open].1, OscType::Bool(false), "nothing opens it again after: {sent:?}");
}

#[test]
fn a_screen_left_open_is_closed_at_start() {
    // The app ended without closing it (it stopped answering, or was killed): VRChat still holds the screen open.
    let mut tree = menu_tree("avtr_stale_screen", true);
    tree["CONTENTS"]["parameters"]["CONTENTS"][params::SCREEN_OPEN.replace('/', "_")]["VALUE"] = json!([true]);
    let (t, n, _vrchat, sent) = n4n0_started(tree);
    assert!(sent.contains(&(parameter(params::SCREEN_OPEN), OscType::Bool(false))), "the first state closes it: {sent:?}");
    t.shutdown(&n);
}

#[test]
fn our_own_menu_echoes_never_reopen_it_nor_loop() {
    let (t, n, vrchat) = n4n0_on_avatar(menu_tree("avtr_menu_echo", true));
    // Opened by another trigger (a hand on N4N0, the binding): the toggle is turned on.
    n.menu_open(true);
    assert!(menu_is_open(&n));
    let sent = received(&vrchat, Duration::from_millis(300));
    assert_eq!(menu_values(&sent), vec![true]);
    // Closed again at once (touched again), before VRChat sent our on back: the toggle is turned off.
    n.menu_open(true);
    assert!(!menu_is_open(&n));
    let sent = received(&vrchat, Duration::from_millis(300));
    assert_eq!(menu_values(&sent), vec![false]);
    // Now the late echo of our on, then of our off: the menu stays closed.
    n.on_parameter(params::MENU, &OscValue::Bool(true));
    std::thread::sleep(Duration::from_millis(150));
    assert!(!menu_is_open(&n), "an echo of ours asks nothing");
    n.on_parameter(params::MENU, &OscValue::Bool(false));
    // Nothing sent back for them: no loop.
    assert_eq!(menu_values(&received(&vrchat, Duration::from_millis(600))), Vec::<bool>::new());
    assert!(!menu_is_open(&n));
    // A third, unexpected off (VRChat saying it again) asks nothing either: the menu is closed already.
    n.on_parameter(params::MENU, &OscValue::Bool(false));
    assert_eq!(menu_values(&received(&vrchat, Duration::from_millis(300))), Vec::<bool>::new());
    assert!(!menu_is_open(&n));
    t.shutdown(&n);
}

#[test]
fn without_the_screen_the_toggle_asks_the_overlay_and_a_close_waits_for_its_answer() {
    let (t, n, _vrchat) = n4n0_on_avatar(menu_tree("avtr_menu_overlay", false));
    assert!(!t.view().screen);
    let request = |t: &TestHost| (t.view().menu_request, t.view().menu_toggle);
    let (before, _) = request(&t);
    n.on_parameter(params::MENU, &OscValue::Bool(true));
    assert!(until(|| request(&t).0 == before.wrapping_add(1)), "the overlay is asked to open its menu");
    assert!(!request(&t).1, "an open, not a toggle");
    // Off before the overlay said it opened: no request yet (it would read as another open)...
    n.on_parameter(params::MENU, &OscValue::Bool(false));
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(request(&t).0, before.wrapping_add(1));
    // ...the close goes once it says it did.
    n.command(Command::MenuState { open: true, dictation: false, keyboard: false });
    assert_eq!(request(&t), (before.wrapping_add(2), true));
    t.shutdown(&n);
}

/// The stand-in avatar's tree with the projected screen, `N4N0/Screen/Replace`, and VRChat's mode (`VRMode`, 1 in VR;
/// `None`: not listed).
fn mode_tree(id: &str, vr_mode: Option<i32>, replace: bool) -> serde_json::Value {
    let mut tree = menu_tree(id, true);
    let parameters = &mut tree["CONTENTS"]["parameters"]["CONTENTS"];
    if let Some(vr_mode) = vr_mode {
        parameters["VRMode"] = json!({ "FULL_PATH": parameter(params::VR_MODE), "TYPE": "i", "VALUE": [vr_mode] });
    }
    parameters["N4N0_Screen_Replace"] = json!({ "FULL_PATH": parameter(params::SCREEN_REPLACE), "TYPE": "T", "VALUE": [replace] });
    tree
}

/// Whether `sent` writes the avatar's screen (whether it shows, a block).
fn writes_the_screen(sent: &[Message]) -> bool {
    sent.iter().any(|(a, _)| a.starts_with(&parameter("N4N0/Screen/")))
}

/// The Menu toggle turned on: N4N0's menu opens on the avatar's screen, which gets its blocks; the overlay is not asked.
fn opens_on_the_screen(t: &TestHost, n: &Shared, vrchat: &UdpSocket) {
    let asked = t.view().menu_request;
    n.on_parameter(params::MENU, &OscValue::Bool(true));
    assert!(until(|| menu_is_open(n)));
    let sent = received(vrchat, Duration::from_millis(600));
    assert!(sent.contains(&(parameter(params::SCREEN_OPEN), OscType::Bool(true))), "{sent:?}");
    assert!(sent.iter().any(|(a, _)| *a == parameter(params::SCREEN_POINTER)), "the owner's screen gets its blocks");
    assert_eq!(t.view().menu_request, asked, "the overlay is not asked");
}

#[test]
fn in_vr_without_replace_the_overlay_shows_the_menu_and_the_screen_is_never_written() {
    let (t, n, vrchat, startup) = n4n0_started(mode_tree("avtr_vr", Some(1), false));
    assert!(!t.view().screen, "in VR, a screen that does not replace the overlay is not used");
    assert_eq!(t.view().prop_sync, Some(8), "the panel is used all the same");
    // The Menu toggle on: the overlay is asked to open N4N0's menu, and says it did; off: to close it.
    let asked = t.view().menu_request;
    n.on_parameter(params::MENU, &OscValue::Bool(true));
    assert!(until(|| t.view().menu_request == asked.wrapping_add(1)), "the overlay is asked to open its menu");
    n.command(Command::MenuState { open: true, dictation: false, keyboard: false });
    n.on_parameter(params::MENU, &OscValue::Bool(false));
    assert!(until(|| t.view().menu_request == asked.wrapping_add(2)));
    assert!(t.view().menu_toggle, "a close");
    // Until the overlay says it closed, the toggle the owner turned off is left so (not turned on, then off).
    let mut sent = received(&vrchat, Duration::from_millis(200));
    n.command(Command::MenuState { open: false, dictation: false, keyboard: false });
    sent.extend(received(&vrchat, Duration::from_millis(300)));
    assert_eq!(menu_values(&sent), Vec::<bool>::new());
    assert!(!writes_the_screen(&startup) && !writes_the_screen(&sent), "{startup:?} {sent:?}");
    assert!(!menu_is_open(&n));
    // N4N0/Screen/Replace on, as VRChat says: the screen shows the menu from now on.
    n.on_parameter(params::SCREEN_REPLACE, &OscValue::Bool(true));
    assert!(until(|| t.view().screen));
    opens_on_the_screen(&t, &n, &vrchat);
    t.shutdown(&n);
}

#[test]
fn with_replace_in_vr_or_in_desktop_mode_the_screen_shows_the_menu() {
    for (id, vr_mode, replace) in [("avtr_replace", 1, true), ("avtr_desktop", 0, false)] {
        let (t, n, vrchat) = n4n0_on_avatar(mode_tree(id, Some(vr_mode), replace));
        assert!(t.view().screen, "{id}");
        opens_on_the_screen(&t, &n, &vrchat);
        t.shutdown(&n);
    }
}

#[test]
fn the_screen_keeps_the_thumbnail_size_vrchat_says() {
    // 128 bytes as the avatar's tree says: the owner's blocks reach past the 744 bytes of 72-byte thumbnails.
    let mut tree = mode_tree("avtr_thumbnails_128", Some(0), false);
    tree["CONTENTS"]["parameters"]["CONTENTS"]["N4N0_Screen_ThumbnailBytes"] =
        json!({ "FULL_PATH": parameter(params::SCREEN_THUMBNAIL_BYTES), "TYPE": "i", "VALUE": [128] });
    let (t, n, vrchat) = n4n0_on_avatar(tree);
    assert!(until(|| n.n4n0.lock().unwrap().screen.thumbnail == 128));
    opens_on_the_screen(&t, &n, &vrchat);
    // A size no screen keeps: 72, the default.
    n.on_parameter(params::SCREEN_THUMBNAIL_BYTES, &OscValue::Int(100));
    assert!(until(|| n.n4n0.lock().unwrap().screen.thumbnail == 72));
    t.shutdown(&n);
    // Neither VRChat nor a manifest says: 72.
    let (t, n, _vrchat) = n4n0_on_avatar(mode_tree("avtr_thumbnails_default", Some(0), false));
    assert!(until(|| t.view().screen));
    assert_eq!(n.n4n0.lock().unwrap().screen.thumbnail, 72);
    t.shutdown(&n);
    assert_eq!(thumbnail_bytes(Some(128), None), Some(128));
    assert_eq!(thumbnail_bytes(Some(4), None), None);
    assert_eq!(int_value(&OscValue::Float(72.0)), Some(72));
}

#[test]
fn vrchat_in_vr_without_replace_takes_the_menu_off_the_screen_and_the_toggle_follows() {
    let (t, n, vrchat) = n4n0_on_avatar(mode_tree("avtr_desktop_then_vr", Some(0), false));
    opens_on_the_screen(&t, &n, &vrchat);
    // VRChat says it runs in VR: the screen's menu closes (the animator hides the screen), the Menu toggle is turned
    // off, and nothing more is written to the screen.
    n.on_parameter(params::VR_MODE, &OscValue::Int(1));
    assert!(until(|| !t.view().screen));
    assert!(!menu_is_open(&n));
    let mut sent = received(&vrchat, Duration::from_millis(50));
    let later = received(&vrchat, Duration::from_millis(400));
    assert!(!writes_the_screen(&later), "{later:?}");
    sent.extend(later);
    assert_eq!(menu_values(&sent), vec![false]);
    // The toggle on again: the overlay is asked.
    let asked = t.view().menu_request;
    n.on_parameter(params::MENU, &OscValue::Bool(true));
    assert!(until(|| t.view().menu_request == asked.wrapping_add(1)));
    let sent = received(&vrchat, Duration::from_millis(300));
    assert!(!writes_the_screen(&sent), "{sent:?}");
    t.shutdown(&n);
}

#[test]
fn until_vrchat_says_its_mode_vr_is_taken_while_steamvr_runs() {
    let (t, n, _vrchat) = n4n0_on_avatar(mode_tree("avtr_mode_unknown", None, false));
    assert!(t.view().screen, "no SteamVR: desktop mode, the screen");
    t.scene(|s| s.steamvr = true);
    n.wake();
    assert!(until(|| !t.view().screen), "SteamVR runs: VR, the overlay");
    // VRChat says it runs in desktop mode, SteamVR open beside it: the screen.
    n.on_parameter(params::VR_MODE, &OscValue::Int(0));
    assert!(until(|| t.view().screen));
    t.shutdown(&n);
}

#[test]
fn the_menu_shows_on_the_screen_in_desktop_mode_or_with_replace() {
    let (_t, n) = n4n0(Settings::default());
    let mut rt = n.n4n0.lock().unwrap();
    rt.screen_present = true;
    rt.screen_sync = Some(8);
    // VRChat's mode (VR), N4N0/Screen/Replace, SteamVR runs: the screen.
    for (vr, replace, steamvr, screen) in [
        (Some(false), Some(false), true, true),
        (Some(true), Some(false), false, false),
        (Some(true), Some(true), true, true),
        (Some(true), None, true, true),
        (None, Some(false), true, false),
        (None, Some(false), false, true),
    ] {
        rt.vr_mode = vr;
        rt.screen_replace = replace;
        rt.screen_place(steamvr);
        assert_eq!(rt.screen.available, screen, "VR {vr:?}, Replace {replace:?}, SteamVR {steamvr}");
    }
    // A prop without the screen (or text values it cannot use) never has it.
    rt.vr_mode = Some(false);
    rt.screen_sync = None;
    rt.screen_place(false);
    assert!(!rt.screen.available);
    rt.screen_present = false;
    rt.screen_sync = Some(8);
    rt.screen_place(false);
    assert!(!rt.screen.available);
    drop(rt);
    // Replace as VRChat holds it, else the manifest's default; neither: a prop from before it.
    let manifest = |default: f32| {
        AvatarManifest::parse(&format!(
            r#"{{ "format": "orbiters.avatar", "version": 1, "avatar_id": "avtr_x",
                 "parameters": [ {{ "name": "{}", "type": "bool", "synced": false, "default": {default} }} ] }}"#,
            params::SCREEN_REPLACE
        ))
        .unwrap()
    };
    assert_eq!(screen_replace(Some(false), Some(&manifest(1.0))), Some(false));
    assert_eq!(screen_replace(None, Some(&manifest(1.0))), Some(true));
    assert_eq!(screen_replace(None, Some(&manifest(0.0))), Some(false));
    let old = AvatarManifest::parse(r#"{ "format": "orbiters.avatar", "version": 1, "avatar_id": "avtr_x" }"#).unwrap();
    assert_eq!(screen_replace(None, Some(&old)), None);
    assert_eq!(screen_replace(None, None), None);
}

/// The stand-in mouse: the left button and VRChat in front as a test sets them, and how often it was read.
#[derive(Clone, Default)]
struct FakeMouse(Arc<Mutex<(bool, bool, usize)>>);

impl Mouse for FakeMouse {
    fn read(&mut self, vrchat: u32) -> MouseState {
        assert_eq!(vrchat, VRCHAT_PID);
        let mut m = self.0.lock().unwrap();
        m.2 += 1;
        // VRChat in mouse-look: its cursor hidden.
        MouseState { down: m.0, front: m.1, cursor: false }
    }
}

impl FakeMouse {
    fn set(&self, down: bool, front: bool) {
        let mut m = self.0.lock().unwrap();
        (m.0, m.1) = (down, front);
    }

    fn reads(&self) -> usize {
        self.0.lock().unwrap().2
    }
}

const VRCHAT_PID: u32 = 4242;

/// [`n4n0_on`] with the stand-in mouse, VRChat's process known.
fn n4n0_with_mouse(tree: serde_json::Value) -> (Arc<TestHost>, Shared, UdpSocket, FakeMouse) {
    let (t, n, vrchat) = n4n0_on_avatar(tree);
    let mouse = FakeMouse::default();
    n.n4n0.lock().unwrap().screen.mouse = Box::new(mouse.clone());
    t.scene(|s| s.vrchat_pid = Some(VRCHAT_PID));
    (t, n, vrchat, mouse)
}

/// What VRChat sends of the eyes (at rest) and the gaze point when the owner looks at pixel (`x`, `y`) of the page, each
/// contact 1 cm as the prop's.
fn look_at(n: &Shared, x: f32, y: f32) {
    use orbiters_n4n0_core::menu::{layout, touch};
    let eyes = params::GAZE_EYES;
    let on = [x as f64 / layout::WIDTH as f64 * touch::WIDTH - touch::WIDTH / 2.0, touch::HEIGHT / 2.0 - y as f64 / layout::HEIGHT as f64 * touch::HEIGHT, 0.0];
    let ahead = [on[0] - eyes[0], on[1] - eyes[1], on[2] - eyes[2]];
    let length = (ahead[0].powi(2) + ahead[1].powi(2) + ahead[2].powi(2)).sqrt();
    let gaze = [eyes[0] + 0.38 * ahead[0] / length, eyes[1] + 0.38 * ahead[1] / length, eyes[2] + 0.38 * ahead[2] / length];
    for (prefix, at) in [(params::SCREEN_EYE_PREFIX, eyes), (params::SCREEN_GAZE_PREFIX, gaze)] {
        for (i, receiver) in touch::RECEIVERS.iter().enumerate() {
            let d = ((at[0] - receiver[0]).powi(2) + (at[1] - receiver[1]).powi(2) + (at[2] - receiver[2]).powi(2)).sqrt() - 0.01;
            n.on_parameter(&format!("{prefix}{i}"), &OscValue::Float((1.0 - d / touch::RADIUS).clamp(0.0, 1.0) as f32));
        }
    }
}

/// Home's YouTube tile: its index in the page's layout, and its middle on the page.
fn youtube_tile() -> (usize, f32, f32) {
    use orbiters_n4n0_core::menu::layout::{self, ScreenPage};
    let home = layout::cached(ScreenPage::Home);
    let (i, e) = home.elements.iter().enumerate().find(|(_, e)| e.target == orbiters_n4n0_core::menu::Target::YouTube).unwrap();
    (i, e.rect.x as f32 + e.rect.w as f32 / 2.0, e.rect.y as f32 + e.rect.h as f32 / 2.0)
}

/// A left click: down, then up a tenth of a second later.
fn click(mouse: &FakeMouse, front: bool) {
    mouse.set(true, front);
    std::thread::sleep(Duration::from_millis(100));
    mouse.set(false, front);
    std::thread::sleep(Duration::from_millis(100));
}

#[test]
fn in_desktop_mode_looking_hovers_and_a_left_click_presses() {
    use orbiters_n4n0_core::menu::Page as MenuPage;
    let (t, n, vrchat, mouse) = n4n0_with_mouse(mode_tree("avtr_look", Some(0), false));
    let screen = |n: &Shared| {
        let rt = n.n4n0.lock().unwrap();
        (rt.screen.hover, rt.screen.menu.page)
    };
    // The menu closed: the mouse is never read.
    click(&mouse, true);
    assert_eq!(mouse.reads(), 0);
    opens_on_the_screen(&t, &n, &vrchat);
    // The hovered element follows the gaze.
    let (tile, x, y) = youtube_tile();
    look_at(&n, x, y);
    assert!(until(|| screen(&n).0 == Some(tile)), "the tile looked at is hovered");
    look_at(&n, x, -100.0);
    assert!(until(|| screen(&n).0.is_none()), "above the screen: nothing");
    // A click with nothing looked at presses nothing.
    click(&mouse, true);
    assert_eq!(screen(&n), (None, MenuPage::Home));
    // Back on the tile, VRChat behind another window: its click is not for VRChat.
    look_at(&n, x, y);
    assert!(until(|| screen(&n).0 == Some(tile)));
    click(&mouse, false);
    assert_eq!(screen(&n).1, MenuPage::Home);
    assert!(n.n4n0.lock().unwrap().screen.pressed.is_none());
    // VRChat in front: the click presses the tile (YouTube's search opens); the press ends once let go.
    mouse.set(true, true);
    assert!(until(|| screen(&n).1 == MenuPage::Search));
    mouse.set(false, true);
    assert!(until(|| n.n4n0.lock().unwrap().screen.pressed.is_none()));
    assert!(mouse.reads() > 0);
    t.shutdown(&n);
}

#[test]
fn in_vr_the_gaze_and_the_mouse_are_left_alone() {
    let (t, n, vrchat, mouse) = n4n0_with_mouse(mode_tree("avtr_look_vr", Some(1), true));
    opens_on_the_screen(&t, &n, &vrchat);
    let (_, x, y) = youtube_tile();
    look_at(&n, x, y);
    click(&mouse, true);
    let rt = n.n4n0.lock().unwrap();
    assert_eq!(rt.screen.hover, None, "the fingers point in VR");
    assert_eq!(rt.screen.menu.page, orbiters_n4n0_core::menu::Page::Home);
    drop(rt);
    assert_eq!(mouse.reads(), 0, "the mouse is not read in VR");
    t.shutdown(&n);
}

/// A stand-in VRChat whose OSCQuery tree the test changes as it goes.
fn vrchat_live(tree: Arc<Mutex<serde_json::Value>>) -> (UdpSocket, SocketAddr) {
    let input = UdpSocket::bind("127.0.0.1:0").unwrap();
    input.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let query = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = query.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in query.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = tree.lock().unwrap().to_string();
            let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()); // i18n: not shown
            let _ = stream.write_all(format!("{head}{body}").as_bytes());
        }
    });
    (input, addr)
}

/// Sets a node of the stand-in avatar's tree (`access` 3: OSC may write it, 1: read only).
fn put_node(tree: &mut serde_json::Value, path: &str, kind: &str, value: serde_json::Value, access: u8) {
    let node = json!({ "FULL_PATH": path, "TYPE": kind, "ACCESS": access, "VALUE": [value] });
    match path.strip_prefix(PARAMETERS_PREFIX) {
        Some(name) => tree["CONTENTS"]["parameters"]["CONTENTS"][name.replace('/', "_")] = node,
        None => tree["CONTENTS"][path.replace('/', "_")] = node,
    }
}

/// The stand-in avatar for saved states: toggles, a slider, an outfit, GoGoLoco, VRChat's own (read only), N4N0's, the
/// capture kit's toggle, and its size.
fn states_tree(id: &str) -> serde_json::Value {
    let mut tree = avatar_tree(id);
    for (name, kind, value, access) in [
        ("Hoodie", "T", json!(true), 3), // i18n: not shown
        ("Hue", "f", json!(0.25), 3), // i18n: not shown
        ("Outfit", "i", json!(2), 3), // i18n: not shown
        ("Go/Stationary", "T", json!(true), 3),
        ("VRCEmote", "i", json!(5), 3),
        ("VRMode", "i", json!(1), 1),
        ("VelocityX", "f", json!(0.0), 1),
        ("N4N0/Visible", "T", json!(true), 3),
        ("N4N0/Capture/Record", "T", json!(false), 3),
    ] {
        put_node(&mut tree, &format!("{PARAMETERS_PREFIX}{name}"), kind, value, access);
    }
    put_node(&mut tree, orbiters_n4n0_core::states::EYE_HEIGHT, "f", json!(1.62), 3);
    put_node(&mut tree, orbiters_n4n0_core::states::EYE_HEIGHT_SCALING_ALLOWED, "T", json!(true), 1);
    tree
}

/// What `sent` set the stand-in avatar's own parameters and its size to (N4N0's own and the menu's left out).
fn avatar_writes(sent: &[Message]) -> Vec<(String, OscType)> {
    sent.iter()
        .filter(|(a, _)| (a.starts_with(PARAMETERS_PREFIX) && !a.contains("N4N0/")) || a == orbiters_n4n0_core::states::EYE_HEIGHT)
        .map(|(a, v)| (a.trim_start_matches(PARAMETERS_PREFIX).to_string(), v.clone()))
        .collect()
}

#[test]
fn a_state_keeps_what_osc_may_write_and_puts_back_only_what_it_should() {
    let tree = Arc::new(Mutex::new(states_tree("avtr_states")));
    let (input, query) = vrchat_live(tree.clone());
    let mut settings = Settings::default();
    settings.osc.vrchat_port = input.local_addr().unwrap().port();
    let (t, n) = n4n0(settings);
    t.start_osc();
    t.set_vrchat_query(query);
    n.start();
    n.n4n0.lock().unwrap().voice.installed = false;
    n.refresh_avatar(None, HashMap::new(), 0);
    received(&input, Duration::from_millis(300));
    // Saved: what OSC may write (GoGoLoco's too), never VRChat's own, N4N0's nor the capture kit's toggle.
    n.command(Command::States { action: StateAction::Save { name: Some("Cosy".into()), size: false, pose: false, default: false }, avatar: None });
    assert!(until(|| t.view().states.states.len() == 1 && !t.view().states.saving));
    let state = t.view().states.states[0].clone();
    assert_eq!((state.name.as_str(), state.parameters, state.pose_parameters), ("Cosy", 5, 2), "{state:?}");
    assert_eq!(state.eye_height, Some(1.62));
    assert!(!state.pose && !state.size && !state.default, "the boxes are off, no default");
    // The avatar changes: applying puts back what differs, the pose and the size only with their boxes.
    {
        let mut t = tree.lock().unwrap();
        for (name, kind, value) in [("Hoodie", "T", json!(false)), ("Outfit", "i", json!(0)), ("Go/Stationary", "T", json!(false)), ("VRCEmote", "i", json!(0))] {
            put_node(&mut t, &format!("{PARAMETERS_PREFIX}{name}"), kind, value, 3);
        }
        put_node(&mut t, orbiters_n4n0_core::states::EYE_HEIGHT, "f", json!(1.2), 3);
    }
    n.command(Command::States { action: StateAction::Apply { id: state.id.clone() }, avatar: None });
    let sent = avatar_writes(&received(&input, Duration::from_millis(800)));
    let expected = vec![("Hoodie".into(), OscType::Bool(true)), ("Hue".into(), OscType::Float(0.25)), ("Outfit".into(), OscType::Int(2))];
    assert_eq!(sent, expected, "every parameter it saved, forced: neither the pose nor the size");
    n.command(Command::States { action: StateAction::Options { id: state.id.clone(), pose: true, size: true }, avatar: None });
    assert!(until(|| t.view().states.states[0].pose));
    n.command(Command::States { action: StateAction::Apply { id: state.id.clone() }, avatar: None });
    let sent = avatar_writes(&received(&input, Duration::from_millis(800)));
    let names: Vec<&str> = sent.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["Go/Stationary", "Hoodie", "Hue", "Outfit", "VRCEmote", orbiters_n4n0_core::states::EYE_HEIGHT]);
    assert_eq!(sent.last().unwrap().1, OscType::Float(1.62));
    // The default: put back once VRChat loaded the avatar again (a world joined, or a switch back to it).
    n.command(Command::States { action: StateAction::Default { id: Some(state.id.clone()) }, avatar: None });
    assert!(until(|| t.view().states.states[0].default));
    n.on_avatar("avtr_states".into());
    let early = avatar_writes(&received(&input, Duration::from_millis(1000)));
    assert!(early.iter().all(|(n, _)| n != "Go/Stationary"), "VRChat applies its own first: {early:?}");
    let later = avatar_writes(&received(&input, Duration::from_millis(2600)));
    assert!(
        later.contains(&("Go/Stationary".into(), OscType::Bool(true))) && later.contains(&(orbiters_n4n0_core::states::EYE_HEIGHT.into(), OscType::Float(1.62))),
        "{later:?}"
    );
    // Renamed, deleted: the list follows, the default with it.
    n.command(Command::States { action: StateAction::Rename { id: state.id.clone(), name: "Evening".into() }, avatar: None });
    assert!(until(|| t.view().states.states[0].name == "Evening"));
    n.command(Command::States { action: StateAction::Delete { id: state.id }, avatar: None });
    assert!(until(|| t.view().states.states.is_empty()));
    // They were kept in N4N0's own file.
    assert!(n.files.states.is_file());
    t.shutdown(&n);
}

#[test]
fn each_avatar_s_states_carry_its_name_from_vrchat_s_oscquery() {
    let mut tree = states_tree("avtr_named");
    // Its name as set in its SDK, whole in UTF-8 over OSCQuery.
    put_node(&mut tree, orbiters_core::osc::AVATAR_NAME, "s", json!("Ultirex ✨ Noir"), 1);
    let tree = Arc::new(Mutex::new(tree));
    let (input, query) = vrchat_live(tree.clone());
    let mut settings = Settings::default();
    settings.osc.vrchat_port = input.local_addr().unwrap().port();
    let (t, n) = n4n0(settings);
    t.start_osc();
    t.set_vrchat_query(query);
    t.world(|w| w.avatar_name = Some("Ultirex Noir (log)".into()));
    n.start();
    n.n4n0.lock().unwrap().voice.installed = false;
    // Another avatar's states, saved before.
    n.n4n0.lock().unwrap().states.store().add("avtr_other", Some("Rexouium"), Some("Party"), Default::default(), None, "2026-10-20T10:00:00Z".into()).unwrap();
    n.refresh_avatar(None, HashMap::new(), 0);
    assert!(until(|| t.view().states.avatar_name.as_deref() == Some("Ultirex ✨ Noir")), "{:?}", t.view().states.avatar_name);
    // A card for the avatar worn (nothing saved yet), then the other.
    let cards = |t: &TestHost| t.view().states.avatars.iter().map(|a| (a.id.clone(), a.name.clone(), a.worn, a.states.len())).collect::<Vec<_>>();
    assert_eq!(cards(&t), vec![
        ("avtr_named".into(), Some("Ultirex ✨ Noir".into()), true, 0),
        ("avtr_other".into(), Some("Rexouium".into()), false, 1)
    ]);
    // Saved: the state keeps the SDK's name with it.
    n.command(Command::States { action: StateAction::Save { name: Some("Cosy".into()), size: false, pose: false, default: false }, avatar: None });
    assert!(until(|| t.view().states.states.len() == 1 && !t.view().states.saving));
    assert_eq!(n.n4n0.lock().unwrap().states.store().avatar("avtr_named").and_then(|a| a.name.clone()).as_deref(), Some("Ultirex ✨ Noir"));
    // The other avatar's states change from the window; applying them waits for that avatar to be worn.
    let other = n.n4n0.lock().unwrap().states.store().avatar("avtr_other").unwrap().states[0].id.clone();
    n.command(Command::States { action: StateAction::Rename { id: other.clone(), name: "Party night".into() }, avatar: Some("avtr_other".into()) });
    assert!(until(|| t.view().states.avatars.get(1).is_some_and(|a| a.states[0].name == "Party night")));
    assert_eq!(t.view().states.states[0].name, "Cosy", "the worn avatar's own are untouched");
    n.command(Command::States { action: StateAction::Apply { id: other }, avatar: Some("avtr_other".into()) });
    assert!(until(|| t.view().states.note.as_ref().is_some_and(|note| note.error && note.text.contains("Wear this avatar"))));
    // Renamed in its SDK and uploaded again: its states follow, and so does the file.
    put_node(&mut tree.lock().unwrap(), orbiters_core::osc::AVATAR_NAME, "s", json!("Ultirex Blanc"), 1);
    n.refresh_avatar(None, HashMap::new(), 0);
    assert!(until(|| t.view().states.avatars[0].name.as_deref() == Some("Ultirex Blanc")));
    let file = std::fs::read_to_string(&n.files.states).unwrap();
    assert!(file.contains("Ultirex Blanc") && file.contains("Party night"), "{file}");
    t.shutdown(&n);
}

#[test]
fn a_3d_picture_with_vrchat_s_camera_closed_says_open_the_camera_before() {
    let (t, n) = n4n0(Settings::default());
    *t.camera.lock().unwrap() = Some(false);
    // With a timer: said at once, no countdown.
    n.command(Command::Photo3DIn { seconds: 5 });
    assert!(until(|| t.view().photo_note.as_ref().is_some_and(|(_, text)| text == crate::platform::CAMERA_CLOSED)));
    assert!(t.view().photo_countdown.is_none());
    assert!(t.notices().iter().any(|(kind, text)| *kind == NoticeKind::Warning && text == crate::platform::CAMERA_CLOSED));
    // At once: the same, numbered anew (each menu shows it again).
    let first = t.view().photo_note.unwrap().0;
    n.command(Command::Photo3DIn { seconds: 0 });
    assert!(until(|| t.view().photo_note.as_ref().is_some_and(|(k, _)| *k != first)));
    assert_eq!(t.photos.load(Ordering::Relaxed), 0);
    // Opened: the countdown starts.
    *t.camera.lock().unwrap() = Some(true);
    n.command(Command::Photo3DIn { seconds: 5 });
    assert!(until(|| t.view().photo_countdown.is_some()));
}

/// Around VRChat's menus on the desktop: opening a menu closes the camera, and nothing opens it but those menus. So the picture waits for it.
#[test]
fn a_3d_picture_can_wait_for_vrchat_s_camera_and_be_called_off() {
    use crate::status::PhotoWait;
    let (t, n) = n4n0(Settings::default());
    *t.camera.lock().unwrap() = Some(false);
    n.command(Command::Photo3DWhenCamera { seconds: 5 });
    assert!(until(|| t.view().photo_wait == Some(PhotoWait::Camera)));
    assert!(t.view().photo_note.is_none(), "waiting is nothing wrong");
    assert!(t.view().photo_countdown.is_none());

    // Pressed again: called off, and the camera opening later starts nothing.
    n.command(Command::Photo3DWhenCamera { seconds: 5 });
    assert!(until(|| t.view().photo_wait.is_none()));
    *t.camera.lock().unwrap() = Some(true);
    std::thread::sleep(std::time::Duration::from_millis(400));
    assert!(t.view().photo_countdown.is_none());

    // Asked again: the camera opens, the countdown starts by itself.
    *t.camera.lock().unwrap() = Some(false);
    n.command(Command::Photo3DWhenCamera { seconds: 5 });
    assert!(until(|| t.view().photo_wait == Some(PhotoWait::Camera)));
    *t.camera.lock().unwrap() = Some(true);
    assert!(until(|| t.view().photo_countdown.is_some() && t.view().photo_wait.is_none()));
    // During a countdown a press changes nothing.
    n.command(Command::Photo3DWhenCamera { seconds: 5 });
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(t.view().photo_wait.is_none() && t.view().photo_countdown.is_some());
}

/// The Action Menu's two pictures (`N4N0/Photo3DPeople`, `N4N0/Photo3DWorld`): a press takes that picture, whatever was last
/// chosen, at the length chosen, and letting go takes none. The one button of an avatar uploaded before them takes the
/// last one chosen and changes nothing.
#[test]
fn the_action_menu_s_two_buttons_take_their_own_picture_and_the_old_one_the_last_chosen() {
    use orbiters_core::settings::{Photo3dLength, PictureMode};
    let settings = Settings { photo_3d: Photo3dLength::Full, photo_3d_mode: PictureMode::People, ..Default::default() };
    let (t, n) = n4n0(settings);
    n.n4n0.lock().unwrap().params = [params::PHOTO_3D, params::PHOTO_3D_PEOPLE, params::PHOTO_3D_WORLD].into_iter().map(String::from).collect();
    let press = |name: &str| {
        let before = t.photos.load(Ordering::Relaxed);
        n.on_parameter(name, &OscValue::Bool(true));
        assert!(until(|| t.photos.load(Ordering::Relaxed) == before + 1), "{name} takes a picture");
        n.on_parameter(name, &OscValue::Bool(false));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(t.photos.load(Ordering::Relaxed), before + 1, "{name}: letting go takes none");
        *t.taken.lock().unwrap().last().unwrap()
    };
    assert_eq!(press(params::PHOTO_3D_WORLD), PictureMode::World);
    assert_eq!(t.settings.lock().unwrap().photo_3d_mode, PictureMode::World, "the last chosen");
    assert_eq!(press(params::PHOTO_3D), PictureMode::World, "the old button: the last chosen");
    assert_eq!(press(params::PHOTO_3D_PEOPLE), PictureMode::People);
    assert_eq!(press(params::PHOTO_3D), PictureMode::People);
    assert_eq!(t.settings.lock().unwrap().photo_3d, Photo3dLength::Full, "the length stays as chosen");
}

/// Another OSC app cannot press a picture on an avatar without the buttons, once the avatar's parameters are known: an
/// avatar with only the old button, or none.
#[test]
fn the_two_picture_buttons_count_only_on_an_avatar_that_has_them() {
    let (t, n) = n4n0(Settings::default());
    n.n4n0.lock().unwrap().params = [params::PHOTO_3D].into_iter().map(String::from).collect();
    n.on_parameter(params::PHOTO_3D_PEOPLE, &OscValue::Bool(true));
    n.on_parameter(params::PHOTO_3D_WORLD, &OscValue::Bool(true));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(t.photos.load(Ordering::Relaxed), 0);
    assert_eq!(t.settings.lock().unwrap().photo_3d_mode, Default::default(), "and chooses nothing");
}

/// VRChat's camera closed: a press on either button says so where the other 3D picture presses do, and takes nothing.
#[test]
fn the_two_picture_buttons_say_when_vrchat_s_camera_is_closed() {
    let (t, n) = n4n0(Settings::default());
    *t.camera.lock().unwrap() = Some(false);
    n.on_parameter(params::PHOTO_3D_WORLD, &OscValue::Bool(true));
    assert!(until(|| t.view().photo_note.as_ref().is_some_and(|(_, text)| text == crate::platform::CAMERA_CLOSED)));
    assert_eq!(t.photos.load(Ordering::Relaxed), 0);
}

/// The avatar's own screen, by its version: from 15 a press where a picture's button is drawn takes that picture; a screen
/// of 14 keeps its one Take and the line that changes it, whose press takes nothing.
#[test]
fn a_screen_takes_what_its_own_page_shows() {
    use orbiters_core::settings::PictureMode;
    use orbiters_n4n0_core::menu::layout::{self, ScreenPage};
    use orbiters_n4n0_core::menu::Target;
    // A press at (x, y) of the page the screen shows, as a finger's goes (menu::touch), and what comes of it.
    let press = |n: &Shared, x: f32, y: f32| {
        let effects = {
            let mut rt = n.n4n0.lock().unwrap();
            let menu = &mut rt.screen.menu;
            let (_, target) = layout::hit_pointable(layout::cached(ScreenPage::of(menu)), menu, x, y).expect("something to press there");
            menu.press(target)
        };
        n.screen_effects(effects);
    };
    let on_screen = |version: i32, id: &str| {
        let mut tree = card_tree(id);
        put_node(&mut tree, &parameter(params::SCREEN_VERSION), "i", json!(version), 3);
        let (t, n, _vrchat) = n4n0_on_avatar(tree);
        n.menu_open(false);
        assert!(until(|| n.n4n0.lock().unwrap().screen.menu.open));
        let mut rt = n.n4n0.lock().unwrap();
        rt.screen.menu.press(Target::Photo);
        assert_eq!(rt.screen.menu.page, orbiters_n4n0_core::menu::Page::Photo);
        drop(rt);
        (t, n)
    };

    let (t, n) = on_screen(contract::SCREEN_TWO_PICTURES, "avtr_two_pictures");
    assert_eq!(ScreenPage::of(&n.n4n0.lock().unwrap().screen.menu), ScreenPage::PhotoTwo);
    assert!(t.taken.lock().unwrap().is_empty());
    press(&n, 300.0, 280.0);
    assert!(until(|| t.taken.lock().unwrap().last() == Some(&PictureMode::World)), "the world's button");
    press(&n, 300.0, 150.0);
    assert!(until(|| t.taken.lock().unwrap().last() == Some(&PictureMode::People)), "the people's button");
    assert_eq!(t.taken.lock().unwrap().len(), 2);
    n.shutdown();

    let (t, n) = on_screen(contract::SCREEN_MIRROR, "avtr_one_take");
    assert_eq!(ScreenPage::of(&n.n4n0.lock().unwrap().screen.menu), ScreenPage::PhotoShare);
    press(&n, 200.0, 512.0);
    assert!(until(|| t.settings.lock().unwrap().photo_3d_mode == PictureMode::World), "the line changes the picture");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(t.photos.load(Ordering::Relaxed), 0, "and takes none");
    press(&n, 300.0, 430.0);
    assert!(until(|| t.taken.lock().unwrap().last() == Some(&PictureMode::World)), "Take takes the one the line says");
    n.shutdown();
}

#[test]
fn a_card_is_taken_back_from_n4n0_s_menu_the_overlay_or_the_window() {
    use orbiters_contracts::n4n0::card;
    let mut tree = card_tree("avtr_take_back");
    put_node(&mut tree, &parameter(params::SCREEN_VERSION), "i", json!(contract::SCREEN_TIMELINE), 3);
    let (t, n, vrchat) = n4n0_on_avatar(tree);
    assert!(until(|| t.view().can_drop));
    let drop = || Command::Drop { link: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(), title: "Never Gonna Give You Up".into(), video: None };
    n.command(drop());
    assert!(until(|| t.view().card.is_some()));
    received(&vrchat, Duration::from_millis(600));
    // The screen's menu offers Take back while it is out (screen version 9).
    n.menu_open(false);
    assert!(until(|| {
        let rt = n.n4n0.lock().unwrap();
        rt.screen.menu.card_out && rt.screen.menu.pages.timeline()
    }));
    // Taken back from the window or the overlay: out no more, as when its time is up.
    n.command(Command::TakeBack);
    let sent = received(&vrchat, Duration::from_millis(900));
    assert!(sent.contains(&(parameter(card::OUT), OscType::Bool(false))), "{sent:?}");
    assert!(t.view().card.is_none() && !n.n4n0.lock().unwrap().screen.menu.card_out);
    // Dropped again, then taken back from the screen's own menu.
    n.command(drop());
    assert!(until(|| t.view().card.is_some()));
    n.screen_effects(vec![orbiters_n4n0_core::menu::Effect::TakeBack]);
    assert!(until(|| t.view().card.is_none()));
    let sent = received(&vrchat, Duration::from_millis(900));
    assert!(sent.contains(&(parameter(card::OUT), OscType::Bool(false))), "{sent:?}");
    t.shutdown(&n);
}

/// A stand-in Orbiters for link cards: each request answered with the next of `answers` (status, body); what it got.
#[allow(clippy::type_complexity)]
fn drops_server(answers: Vec<(u16, &'static str)>) -> (String, Arc<Mutex<Vec<(String, Option<String>, String)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = seen.clone();
    std::thread::spawn(move || {
        let mut answers = answers.into_iter();
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let (mut first, mut auth, mut length) = (String::new(), None, 0usize);
            reader.read_line(&mut first).unwrap();
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
                if lower.starts_with("authorization:") {
                    auth = Some(line["authorization:".len()..].trim().to_string());
                }
                line.clear();
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();
            kept.lock().unwrap().push((first.trim().to_string(), auth, String::from_utf8_lossy(&body).into_owned()));
            let (status, text) = answers.next().unwrap_or((500, "{}"));
            let mut stream = stream;
            let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
        }
    });
    (base, seen)
}

/// The stand-in avatar's tree with a screen of version 7 (its card of its own), in VRChat's desktop mode.
fn card_tree(id: &str) -> serde_json::Value {
    let mut tree = mode_tree(id, Some(0), false);
    put_node(&mut tree, &parameter(params::SCREEN_VERSION), "i", json!(7), 3);
    for name in [orbiters_contracts::n4n0::card::OUT, params::CARD_TOUCH_L, params::CARD_TOUCH_R] {
        put_node(&mut tree, &parameter(name), "T", json!(false), 3);
    }
    tree
}

/// The values `sent` set `N4N0/Text/Pointer` to, in order.
fn text_pointers(sent: &[Message]) -> Vec<i32> {
    sent.iter().filter(|(a, _)| *a == parameter(params::TEXT_POINTER)).filter_map(|(_, v)| if let OscType::Int(i) = v { Some(*i) } else { None }).collect()
}

fn copied(text: &str) -> bool {
    CLIPBOARD.lock().unwrap().iter().any(|t| t == text)
}

#[test]
fn a_dropped_card_is_out_for_everyone_beside_the_menu_until_its_time_is_up() {
    use orbiters_contracts::n4n0::card;
    let (t, n, vrchat) = n4n0_on_avatar(card_tree("avtr_card"));
    assert!(until(|| t.view().can_drop), "a screen of version 7 drops cards");
    let link = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
    n.command(Command::Drop { link: link.into(), title: "Never Gonna Give You Up".into(), video: None });
    let sent = received(&vrchat, Duration::from_millis(2500));
    assert!(sent.contains(&(parameter(card::OUT), OscType::Bool(true))), "the card is out: {sent:?}");
    assert_ne!(n.n4n0.lock().unwrap().screen.menu.page, orbiters_n4n0_core::menu::Page::Card, "the menu is not the card");
    // Everyone's card memory: cleared (252), then the card's blocks past the screen's.
    let pointers = text_pointers(&sent);
    let first = card::first_pointer(8, 72) as i32;
    let clear = pointers.iter().position(|&p| p == card::POINTER_CLEAR).expect("cleared first");
    let block = pointers.iter().position(|&p| p >= first && p < card::POINTER_CLEAR).expect("a card block");
    assert!(clear < block, "{pointers:?}");
    assert!(!pointers.contains(&253), "no screen freeze any more");
    let view = t.view().card.clone().expect("the card is out");
    assert_eq!(view.link, link);
    assert!(view.left > card::LIFE_SECONDS - 10, "{view:?}");
    // A hand on it: its link is copied, here.
    n.on_parameter(params::CARD_TOUCH_R, &OscValue::Bool(true));
    assert!(until(|| copied(link)));
    n.on_parameter(params::CARD_TOUCH_R, &OscValue::Bool(false));
    // The menu opens and closes beside it: the card stays out.
    n.menu_open(false);
    received(&vrchat, Duration::from_millis(500));
    assert!(menu_is_open(&n) && t.view().card.is_some(), "the menu beside the card");
    n.menu_open(true);
    let sent = received(&vrchat, Duration::from_millis(500));
    assert!(!sent.contains(&(parameter(card::OUT), OscType::Bool(false))), "{sent:?}");
    assert!(t.view().card.is_some());
    // Its time is up: no longer out.
    n.n4n0.lock().unwrap().screen.card_ends_now();
    n.wake();
    let sent = received(&vrchat, Duration::from_millis(900));
    assert!(sent.contains(&(parameter(card::OUT), OscType::Bool(false))), "{sent:?}");
    assert!(t.view().card.is_none());
    t.shutdown(&n);
}

#[test]
fn a_screen_of_version_8_sends_the_card_s_full_picture_to_its_owner_then_to_everyone() {
    use orbiters_contracts::n4n0::picture::{chunk, SLOT_CARD};
    let mut tree = card_tree("avtr_pictures");
    put_node(&mut tree, &parameter(params::SCREEN_VERSION), "i", json!(contract::SCREEN_FULL_PICTURES), 3);
    for i in 0..chunk::LOCAL_VALUES {
        put_node(&mut tree, &parameter(&params::picture_value(i)), "f", json!(0.0), 3);
    }
    put_node(&mut tree, &parameter(params::PICTURE_SEND), "T", json!(false), 3);
    let (t, n, vrchat) = n4n0_on_avatar(tree);
    assert!(until(|| t.view().can_drop && n.n4n0.lock().unwrap().screen.full_pictures), "a screen of version 8 shows full pictures");
    let id = "dQw4w9WgXcQ";
    let rgb = orbiters_n4n0_core::picture::Rgb { width: 320, height: 180, pixels: (0..320 * 180 * 3).map(|i| (i * 7 % 251) as u8).collect() };
    let picture = orbiters_n4n0_core::menu::screen::picture_from_rgb(&rgb);
    let stream = picture.full_stream(SLOT_CARD, 72).expect("a full picture");
    n.n4n0.lock().unwrap().screen.put_picture(id, picture);
    n.command(Command::Drop { link: format!("https://www.youtube.com/watch?v={id}"), title: "Never Gonna Give You Up".into(), video: Some(id.into()) });
    // The owner: the bridge on, then the stream's chunks through the local values (each one checked, rebuilding it).
    let sent = received(&vrchat, Duration::from_millis(1500));
    assert!(sent.contains(&(parameter(params::PICTURE_SEND), OscType::Bool(true))), "the bridge on");
    let value = |sent: &[Message], i: usize| sent.iter().filter(|(a, _)| *a == parameter(&params::picture_value(i))).count();
    assert!(value(&sent, 0) >= stream.len().div_ceil(chunk::LOCAL_PAYLOAD), "every local chunk went");
    // Everyone: once the card's own blocks went, picture chunks on pointers 188 to 251 (slot 6's: 236 to 243).
    let sent = received(&vrchat, Duration::from_millis(9000));
    let pointers: Vec<i32> = text_pointers(&sent).into_iter().filter(|p| (chunk::POINTER_FIRST..chunk::POINTER_FIRST + 64).contains(p)).collect();
    assert!(!pointers.is_empty(), "picture chunks for everyone");
    assert!(pointers.iter().all(|p| (p - chunk::POINTER_FIRST) / 8 == SLOT_CARD as i32), "{pointers:?}");
    // Its two rounds went: the owner's bridge off.
    assert!(sent.contains(&(parameter(params::PICTURE_SEND), OscType::Bool(false))), "the bridge off once its rounds went");
    t.shutdown(&n);
}

#[test]
fn a_card_is_told_to_orbiters_and_someone_else_s_is_fetched_when_grabbed() {
    let (t, n, _vrchat) = n4n0_on_avatar(card_tree("avtr_card_server"));
    let (base, seen) = drops_server(vec![
        (201, r#"{ "expiresAt": "2026-10-25T10:00:20.000Z" }"#),
        (200, r#"{ "link": "https://www.youtube.com/watch?v=jNQXAC9IVRw", "title": "Me at the zoo", "droppedAt": "2026-10-25T10:00:30.000Z" }"#),
        (404, r#"{ "error": "No card was dropped here in the last twenty seconds." }"#),
    ]);
    t.sign_in(base, "orbit-test");
    let world = "wrld_4cf554b4-430c-4f8f-b53e-1f294eed230b";
    t.world(|w| {
        w.world_id = Some(world.into());
        w.instance = Some("12345~private(usr_1339e525-d75d-4832-9104-98dd89287681)".into());
        w.in_world = true;
    });
    let hash = t.world.lock().unwrap().instance_hash().unwrap();
    assert!(until(|| t.view().can_drop));
    let link = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
    n.command(Command::Drop { link: link.into(), title: "Never Gonna".into(), video: None });
    assert!(until(|| !seen.lock().unwrap().is_empty()));
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].0, "POST /n4n0/drops HTTP/1.1");
        assert_eq!(seen[0].1.as_deref(), Some("Bearer orbit-test"));
        let body: serde_json::Value = serde_json::from_str(&seen[0].2).unwrap();
        assert_eq!(body, json!({ "instance": hash, "link": link, "title": "Never Gonna" }));
    }
    // The owner's card gone, a hand on someone else's: Orbiters gives the newest dropped here.
    n.n4n0.lock().unwrap().screen.card_ends_now();
    n.wake();
    assert!(until(|| t.view().card.is_none()));
    n.on_parameter(params::CARD_TOUCH_L, &OscValue::Bool(true));
    assert!(until(|| copied("https://www.youtube.com/watch?v=jNQXAC9IVRw")));
    assert_eq!(seen.lock().unwrap()[1].0, format!("GET /n4n0/drops/latest?instance={hash} HTTP/1.1"));
    // Both hands at once, or one brushing it again at once: one grab.
    n.on_parameter(params::CARD_TOUCH_R, &OscValue::Bool(true));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(seen.lock().unwrap().len(), 2, "no second request");
    t.shutdown(&n);
}

#[test]
fn in_vr_without_replace_a_card_drops_while_the_menu_is_the_overlay_s() {
    use orbiters_contracts::n4n0::card;
    let mut tree = card_tree("avtr_card_vr");
    tree["CONTENTS"]["parameters"]["CONTENTS"]["VRMode"] = json!({ "FULL_PATH": parameter(params::VR_MODE), "TYPE": "i", "VALUE": [1] });
    let (t, n, vrchat, startup) = n4n0_started(tree);
    assert!(!t.view().screen, "the menu is the overlay's");
    assert!(t.view().can_drop, "the avatar's card is there");
    assert!(!writes_the_screen(&startup), "{startup:?}");
    // Dropped from the overlay's menu: the card is out and its blocks go; the screen stays closed.
    let link = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
    n.command(Command::Drop { link: link.into(), title: "Never Gonna".into(), video: None });
    let sent = received(&vrchat, Duration::from_millis(1500));
    assert!(sent.contains(&(parameter(card::OUT), OscType::Bool(true))), "{sent:?}");
    assert!(!sent.contains(&(parameter(params::SCREEN_OPEN), OscType::Bool(true))), "the screen stays closed");
    assert!(text_pointers(&sent).iter().any(|&p| p >= card::first_pointer(8, 72) as i32 && p < card::POINTER_CLEAR));
    // The overlay's menu opening changes nothing for the card.
    n.command(Command::MenuState { open: true, dictation: false, keyboard: false });
    received(&vrchat, Duration::from_millis(400));
    assert!(t.view().card.is_some());
    t.shutdown(&n);
}

// What N4N0 asks of each platform trait, with the stand-ins.

/// Waits up to two seconds for `done`, then says whether it came.
fn soon(done: impl Fn() -> bool) -> bool {
    until(done)
}

#[test]
fn the_ears_start_with_vrchat_listen_and_stop_and_a_crash_is_told() {
    let media = Arc::new(Watcher::new(|| Box::new(NoMedia)));
    let (t, n, device) = n4n0_on(Settings::default(), media);
    *device.ears.allowed.lock().unwrap() = true;
    n.n4n0.lock().unwrap().voice.installed = true;
    // VRChat closed: no worker; open: one, started with the models' folder.
    n.voice_follow();
    assert!(device.ears.workers.lock().unwrap().is_empty());
    t.scene(|s| s.vrchat = true);
    n.voice_follow();
    let worker = device.ears.workers.lock().unwrap()[0].clone();
    assert!(matches!(&worker.commands()[..], [VoiceCommand::Start { models, wake_word: false, .. }] if *models == n.files.models));
    worker.say(VoiceEvent::Ready { engine: "test".into(), load_ms: 1 });
    assert!(t.view().voice.running);
    // A wake word turned on reaches the running worker, without starting another.
    t.settings.lock().unwrap().n4n0.wake_word = true;
    n.voice_follow();
    assert!(matches!(worker.commands().last(), Some(VoiceCommand::WakeWord { on: true, .. })));
    assert_eq!(device.ears.workers.lock().unwrap().len(), 1);
    // Talk: a held listen, then its end.
    n.app_talk(true, false);
    assert!(matches!(worker.commands().last(), Some(VoiceCommand::Listen { hold: true, id: 1 })));
    n.app_talk(false, false);
    assert!(matches!(worker.commands().last(), Some(VoiceCommand::Finish)));
    // It stops on its own: N4N0 says so and the next start waits.
    worker.crash(3, "something went wrong");
    assert!(soon(|| t.view().voice.error.as_deref() == Some("N4N0's ears stopped (exit code 0x3): something went wrong.")));
    assert!(*worker.dropped.lock().unwrap());
    n.voice_follow();
    assert_eq!(device.ears.workers.lock().unwrap().len(), 1, "not started again at once");
    // VRChat closes: no worker at all (none runs now; the lease of the last Talk keeps none either once it ended).
    t.scene(|s| s.vrchat = false);
    n.voice_stop();
    // Installing runs a worker of its own.
    n.command(Command::InstallVoice);
    assert!(soon(|| t.view().voice.downloading == Some(0.0)));
    let installer = device.ears.workers.lock().unwrap().last().unwrap().clone();
    assert!(matches!(&installer.commands()[..], [VoiceCommand::Install { repair: false, .. }]));
    installer.say(VoiceEvent::Installing { fraction: 0.5 });
    assert_eq!(t.view().voice.downloading, Some(0.5));
    installer.say(VoiceEvent::Installed);
    let view = t.view();
    assert!(view.voice.installed && view.voice.downloading.is_none());
    assert!(t.notices().iter().any(|(k, m)| *k == NoticeKind::Success && m == "N4N0 can hear now."));
    n.shutdown();
}

#[test]
fn copying_goes_to_the_device_s_clipboard_with_its_press_sound() {
    let media = Arc::new(Watcher::new(|| Box::new(NoMedia)));
    let (t, n, device) = n4n0_on(Settings::default(), media);
    n.command(Command::CopyText { text: "https://example.com/copied-by-a-test".into() });
    assert!(copied("https://example.com/copied-by-a-test"));
    assert!(t.notices().contains(&(NoticeKind::Success, "Copied: paste it in VRChat.".into())));
    // A dropped card's link copied when a hand grabs it: the press sound too (the person may be in VR).
    {
        let mut rt = n.n4n0.lock().unwrap();
        rt.screen.avatar(true, true, Some(8), 72);
        rt.screen.menu.pages = orbiters_n4n0_core::menu::Pages::Ten;
        rt.screen.set_card_capable(true);
    }
    n.command(Command::Drop { link: "https://example.com/card".into(), title: "A card".into(), video: None });
    n.card_grab();
    assert!(copied("https://example.com/card"));
    assert_eq!(device.sounds.0.load(Ordering::Relaxed), 1);
    assert!(t.notices().iter().any(|(_, m)| m == "Copied \"A card\": paste it in VRChat."), "{:?}", t.notices());
    n.shutdown();
}

#[test]
fn only_orbiters_pages_reach_the_device_s_browser() {
    let media = Arc::new(Watcher::new(|| Box::new(NoMedia)));
    let (_t, n, device) = n4n0_on(Settings::default(), media);
    n.command(Command::OpenLink { url: "https://evil.example/phish".into() });
    let approve = format!("{}approve/42", orbiters_core::account::Server::current().website());
    n.command(Command::OpenLink { url: approve.clone() });
    assert_eq!(device.browser.0.lock().unwrap().as_slice(), &[approve]);
    n.shutdown();
}

#[test]
fn the_avatar_s_triggers_go_to_the_app_s_own_features_in_order() {
    let (t, n) = n4n0(Settings::default());
    // An avatar whose parameters are known, with N4N0's call and 3D picture triggers and the walk boost.
    n.n4n0.lock().unwrap().params = [params::CALL_MENU, params::CALL_GO, params::CALL_PICK, params::PHOTO_3D, params::WALK_BOOST]
        .into_iter()
        .map(String::from)
        .collect();
    n.on_parameter(params::CALL_MENU, &OscValue::Bool(true));
    n.on_parameter(params::CALL_PICK, &OscValue::Float(0.4));
    n.on_parameter(params::CALL_GO, &OscValue::Bool(true));
    n.on_parameter(params::PHOTO_3D, &OscValue::Bool(true));
    n.on_parameter(params::WALK_BOOST, &OscValue::Float(0.75));
    n.on_parameter("Seated", &OscValue::Bool(true));
    assert!(soon(|| t.photos.load(Ordering::Relaxed) == 1 && t.walk.lock().unwrap().len() == 1));
    assert_eq!(t.calls.lock().unwrap().as_slice(), &[CallRequest::Menu, CallRequest::Pick(0.4), CallRequest::Go]);
    assert_eq!(t.walk.lock().unwrap().as_slice(), &[0.75]);
    assert_eq!(t.motion.lock().unwrap().as_slice(), &[Some(("Seated".to_string(), OscValue::Bool(true)))]);
    // A trigger the avatar lacks does nothing (another OSC app cannot press it).
    n.on_parameter(params::CALL_END, &OscValue::Bool(true));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(t.calls.lock().unwrap().len(), 3);
    // A new avatar: it has not said how it moves yet.
    n.on_avatar("avtr_other".into());
    assert_eq!(t.motion.lock().unwrap().last(), Some(&None));
    // The interfaces' call buttons, and a touch while a call rings: answered.
    n.command(Command::Call { contact: "c1".into() });
    n.command(Command::MuteCall { on: true });
    *t.ringing.lock().unwrap() = true;
    n.n4n0.lock().unwrap().params.insert(params::TOUCH.to_string());
    n.on_parameter(params::TOUCH, &OscValue::Bool(true));
    assert!(soon(|| t.calls.lock().unwrap().last() == Some(&CallRequest::Answer)));
    assert_eq!(&t.calls.lock().unwrap()[3..5], &[CallRequest::Contact("c1".into()), CallRequest::Mute(true)]);
    // A picture with VRChat's camera waits for no recording.
    *t.capturing.lock().unwrap() = true;
    n.command(Command::Photo { delay_seconds: 0 });
    assert!(t.notices().iter().any(|(k, m)| *k == NoticeKind::Warning && m.contains("recording")), "{:?}", t.notices());
    n.shutdown();
}

#[test]
fn the_menu_opens_with_the_device_s_keyboard_and_drives_its_music() {
    /// What the stand-in music backend was asked.
    struct Asked(Arc<Mutex<Vec<orbiters_n4n0_core::menu::MediaCommand>>>);
    impl MediaBackend for Asked {
        fn run(&mut self, command: orbiters_n4n0_core::menu::MediaCommand) -> Result<(), String> {
            self.0.lock().unwrap().push(command);
            Ok(())
        }
        fn read(&mut self, _previous: &Reading) -> Reading {
            Reading::default()
        }
    }
    let asked = Arc::new(Mutex::new(Vec::new()));
    let kept = asked.clone();
    let media = Arc::new(Watcher::new(move || Box::new(Asked(kept))));
    let (_t, n, _) = n4n0_on(Settings::default(), media);
    n.n4n0.lock().unwrap().screen.avatar(true, true, Some(8), 72);
    n.menu_open(false);
    assert_eq!(n.n4n0.lock().unwrap().screen.menu.layout, KeyboardLayout::Azerty, "Auto: the device's keyboard");
    n.screen_effects(vec![orbiters_n4n0_core::menu::Effect::Media(orbiters_n4n0_core::menu::MediaCommand::Next)]);
    assert!(soon(|| asked.lock().unwrap().as_slice() == [orbiters_n4n0_core::menu::MediaCommand::Next]));
    n.shutdown();
}

#[test]
fn n4n0_keeps_its_files_where_the_app_says() {
    let (t, n) = n4n0(Settings::default());
    std::fs::create_dir_all(&n.files.avatars).unwrap();
    // A manifest Orbiters Toolkit wrote for the avatar: N4N0 reads it there.
    let manifest = r#"{ "format": "orbiters.avatar", "version": 1, "avatar_id": "avtr_files", "n4n0": { "text_sync": 8, "screen": 0 } }"#;
    std::fs::write(n.files.avatars.join(orbiters_contracts::avatar::file_name("avtr_files")), manifest).unwrap();
    // (OSC off: VRChat's tree says nothing, and it is not asked again.)
    n.refresh_avatar(Some("avtr_files".into()), HashMap::new(), crate::runtime::TREE_RETRIES);
    assert!(n.n4n0.lock().unwrap().manifest, "read from the app's avatars folder");
    assert_eq!(t.view().prop_sync, Some(8), "the manifest's panel");
    // A state saved goes to the app's states file.
    n.n4n0.lock().unwrap().states.store().add("avtr_files", None, Some("Cosy"), Default::default(), None, "t".into()).unwrap();
    n.command(Command::States { action: StateAction::Default { id: Some("s1".into()) }, avatar: None });
    assert!(n.files.states.is_file(), "{}", n.files.states.display());
    assert!(std::fs::read_to_string(&n.files.states).unwrap().contains("Cosy"));
    n.shutdown();
}

#[test]
fn n4n0_hands_its_menus_the_3d_captures_shares_them_and_reads_friends_while_share_to_shows() {
    use orbiters_n4n0_core::captures::{CapturePhase, CaptureRow, CapturesView, Friend, Friends, ShareAsk};
    use orbiters_n4n0_core::menu::{Effect, Page, Pages};
    let (host, n) = n4n0(Settings::default());
    // The app hands its captures over: the status has them for the overlay, the screen's menu directly.
    let row = CaptureRow { key: "C:/s/a".into(), world: "Rooftop".into(), phase: CapturePhase::Ready, ..Default::default() };
    let view = CapturesView { latest: Some(row.clone()), history: vec![row.clone()], listed: true, note: String::new() };
    n.set_captures(view.clone());
    assert_eq!(host.view().captures, view);
    assert_eq!(n.n4n0.lock().unwrap().screen.menu.captures, view);
    // History opened in the overlay lists them again; a share goes to the app as asked.
    n.command(Command::Captures);
    assert!(until(|| host.listed.load(Ordering::Relaxed) == 1));
    let ask = ShareAsk { key: "C:/s/a".into(), public: true, recipients: Vec::new(), acknowledge_explicit: true };
    n.command(Command::ShareCapture { share: ask.clone() });
    assert!(until(|| host.shares.lock().unwrap().as_slice() == std::slice::from_ref(&ask)));
    // Friends: read while Share to … shows (by name), forgotten when it closes; why they could not be read.
    let f = |id: &str, name: &str| Friend { id: id.into(), name: name.into() };
    *host.friends.lock().unwrap() = Some(Ok(vec![f("usr_b", "bob"), f("usr_a", "Alice")]));
    n.command(Command::Friends { on: true });
    assert!(until(|| matches!(host.view().friends, Friends::Ready { .. })));
    assert_eq!(host.view().friends.list(), [f("usr_a", "Alice"), f("usr_b", "bob")]);
    assert_eq!(n.n4n0.lock().unwrap().screen.menu.friends, host.view().friends);
    n.command(Command::Friends { on: false });
    assert_eq!(host.view().friends, Friends::None);
    *host.friends.lock().unwrap() = Some(Err("VRCX and VRCNext were not found".into()));
    n.command(Command::Friends { on: true });
    assert!(until(|| host.view().friends == Friends::Failed { message: "VRCX and VRCNext were not found".into() }));
    // The screen's menu fetches a capture's picture from the app, by its key.
    {
        let mut rt = n.n4n0.lock().unwrap();
        rt.screen.menu.pages = Pages::Share;
        rt.screen.menu.open();
        rt.screen.menu.page = Page::Photo;
    }
    n.screen_effects(vec![Effect::Thumbnails(vec![row.picture_key()])]);
    assert!(until(|| host.capture_pictures.lock().unwrap().as_slice() == ["C:/s/a"]));
    // The same captures again change nothing.
    let published = host.view();
    n.set_captures(view);
    assert_eq!(host.view(), published);
    n.shutdown();
}

#[test]
fn the_qr_code_scanner_opens_web_links_once_and_stops_with_its_page() {
    use orbiters_n4n0_core::qr::QrPhase;
    let (host, n) = n4n0(Settings::default());
    let phase = |host: &TestHost| host.view().qr.phase;
    n.command(Command::Qr { on: true });
    assert_eq!(host.qr_started.load(Ordering::Relaxed), 1);
    assert_eq!(phase(&host), QrPhase::Starting);
    n.command(Command::Qr { on: true });
    assert_eq!(host.qr_started.load(Ordering::Relaxed), 1, "asked again while it runs: the same scan");
    // No picture from VRChat's camera: what to do, by whether the camera is open.
    *host.camera.lock().unwrap() = Some(false);
    host.qr_sees(QrSeen::Camera(false));
    let view = host.view().qr;
    assert_eq!(view.phase, QrPhase::NoCamera);
    assert!(view.note.starts_with("Open VRChat's camera"), "{}", view.note);
    *host.camera.lock().unwrap() = Some(true);
    host.qr_sees(QrSeen::Camera(false));
    assert!(host.view().qr.note.contains("choose Stream"), "the camera open: Stream mode and Spout Stream");
    host.qr_sees(QrSeen::Camera(true));
    assert_eq!((phase(&host), host.view().qr.note.as_str()), (QrPhase::Looking, ""));
    // A web link opens at once, once; anything else opens nothing and only says so.
    host.qr_sees(QrSeen::Code("https://orbiters.cc/maps".into()));
    host.qr_sees(QrSeen::Code("https://orbiters.cc/maps".into()));
    host.qr_sees(QrSeen::Code("javascript:alert(1)".into()));
    host.qr_sees(QrSeen::Code("https://bank.example@evil.example/".into()));
    host.qr_sees(QrSeen::Code("HTTPS://example.com/a".into()));
    assert_eq!(*host.watched.lock().unwrap(), ["https://orbiters.cc/maps", "https://example.com/a"]);
    let view = host.view().qr;
    assert_eq!((view.link.as_deref(), view.opened), (Some("https://example.com/a"), 2));
    assert!(host.notices().iter().any(|(k, m)| *k == NoticeKind::Success && m == "Opened from a QR code: orbiters.cc/maps"));
    // The screen's menu shows the same scanner.
    assert_eq!(n.n4n0.lock().unwrap().screen.menu.qr, view);
    // Its page closed: it stops, and what that scan still says is not heard.
    n.command(Command::Qr { on: false });
    assert_eq!(host.qr_dropped.load(Ordering::Relaxed), 1);
    assert_eq!(phase(&host), QrPhase::Off);
    host.qr_sees(QrSeen::Code("https://late.example/".into()));
    assert_eq!(host.watched.lock().unwrap().len(), 2, "a stopped scan opens nothing");
    // Started again, it stops on its own after its time without a link.
    n.command(Command::Qr { on: true });
    assert_eq!(host.qr_started.load(Ordering::Relaxed), 2);
    {
        let mut rt = n.n4n0.lock().unwrap();
        let ended = crate::qr::tick(&mut rt, Instant::now() + orbiters_n4n0_core::qr::SCAN_FOR + Duration::from_secs(1));
        assert!(ended.is_some());
        assert_eq!(rt.qr.view.phase, QrPhase::Stopped);
        assert!(rt.qr.next_due().is_none());
    }
    assert_eq!(host.qr_dropped.load(Ordering::Relaxed), 2);
    // A scanner that cannot run says why.
    n.command(Command::Qr { on: true });
    host.qr_sees(QrSeen::Failed("The stream camera cannot be read".into()));
    assert_eq!((phase(&host), host.view().qr.note), (QrPhase::Stopped, "The stream camera cannot be read".to_string()));
    n.shutdown();
}

/// VRChat's tree for the N4N0 avatar with some of N4N0's controller input parameters (contracts `Docs/n4n0-input.md`).
fn input_tree(id: &str, inputs: &[&str]) -> serde_json::Value {
    let mut tree = avatar_tree(id);
    let parameters = tree["CONTENTS"]["parameters"]["CONTENTS"].as_object_mut().unwrap();
    for name in inputs {
        let kind = if contract::input::is_bool(name) { "F" } else { "f" };
        let value = if kind == "F" { json!(false) } else { json!(0.0) };
        parameters.insert(name.replace('/', "_"), json!({ "FULL_PATH": format!("{PARAMETERS_PREFIX}{name}"), "TYPE": kind, "VALUE": [value] }));
    }
    tree
}

#[test]
fn the_controllers_touchpads_reach_only_the_input_parameters_the_avatar_has() {
    use crate::input::{Pad, Pads};
    let right = contract::input::TOUCHPAD[1];
    let (t, n, vrchat) = started_on(input_tree("avtr_input", &[right[0], right[2], contract::input::STICK[0][3]]));
    n.refresh_avatar(None, HashMap::new(), 0);
    assert_eq!(t.view().input_params, 3);
    assert_eq!(t.view().controls, 1, "the input parameters are never the assistant's");
    received(&vrchat, Duration::from_millis(400));
    let mut pads = Pads::default();
    pads.touchpads[1] = Pad { x: 0.25, y: -0.5, touch: true, click: false, force: 0.0 };
    pads.sticks[0].click = true;
    n.command(Command::Input { pads });
    let sent: Vec<Message> = received(&vrchat, Duration::from_millis(300)).into_iter().filter(|(a, _)| a.contains("/Touchpad/") || a.contains("/Stick/")).collect();
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert!(sent.contains(&(parameter(right[0]), OscType::Float(0.25))));
    assert!(sent.contains(&(parameter(right[2]), OscType::Bool(true))));
    assert!(sent.contains(&(parameter(contract::input::STICK[0][3]), OscType::Bool(true))));
    // Turned off: back to rest, once.
    t.settings.lock().unwrap().n4n0.controller_input = false;
    n.wake();
    let rest: Vec<Message> = received(&vrchat, Duration::from_millis(400)).into_iter().filter(|(a, _)| a.contains("/Touchpad/") || a.contains("/Stick/")).collect();
    assert_eq!(rest.len(), 3, "{rest:?}");
    assert!(rest.contains(&(parameter(right[0]), OscType::Float(0.0))));
    assert!(rest.contains(&(parameter(right[2]), OscType::Bool(false))));
    t.shutdown(&n);
}
