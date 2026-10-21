//! Orb calls in the tray service (contracts `Docs/orb-calls.md`): presence and the friends this computer's VRCX and
//! VRCNext know (as hashes) go to Orbiters every minute while calls are on; the server's event stream brings incoming
//! calls and the other side's answer and candidates; the `orbiters-call` worker carries the audio peer to peer. The
//! Orb's panel lists the contacts while its Call menu is open and says who is calling.

use super::{NoticeKind, Shared};
use crate::state::{CallContact, CallLink, CallPhase, CallView};
use orbiters_core::account::Api;
use orbiters_core::vrchat_accounts::{self, LocalVrchat};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PRESENCE_EVERY: Duration = Duration::from_secs(60);
const CONTACTS_EVERY: Duration = Duration::from_secs(30);
/// VRCX and VRCNext are read again after this.
const FRIENDS_EVERY: Duration = Duration::from_secs(5 * 60);
/// Contacts shown on the panel at once.
pub const PAGE: usize = 4;

struct Worker {
    child: Child,
    stdin: ChildStdin,
}

impl Worker {
    fn spawn(ctl: Shared) -> anyhow::Result<Self> {
        let path = crate::worker::sibling_exe("orbiters-call");
        anyhow::ensure!(path.is_file(), "The call worker was not found at {}", path.display());
        let mut cmd = Command::new(&path);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        std::thread::Builder::new().name("call-stdout".into()).spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match serde_json::from_str::<Value>(&line) {
                    Ok(e) => ctl.on_call_worker(e),
                    Err(_) => eprintln!("call: unexpected output: {line}"),
                }
            }
            ctl.on_call_worker(json!({ "event": "ended", "reason": "closed" }));
        })?;
        std::thread::Builder::new().name("call-stderr".into()).spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                eprintln!("call: {line}");
            }
        })?;
        Ok(Self { child, stdin })
    }

    fn send(&mut self, command: Value) -> anyhow::Result<()> {
        writeln!(self.stdin, "{command}")?;
        self.stdin.flush()?;
        Ok(())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.send(json!({ "cmd": "quit" }));
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
    }
}

/// The call under way.
struct Call {
    /// The server's id, once it gave one (a caller's offer is sent before).
    id: Option<String>,
    contact: String,
    name: String,
    phase: CallPhase,
    incoming: bool,
    /// An incoming call's offer and relay servers, until it is answered.
    offer: Option<(String, Value)>,
    /// Candidates of this side waiting for the call's id.
    candidates: Vec<Value>,
    connected_at: Option<u64>,
    muted: bool,
    levels: (f32, f32),
}

#[derive(Default)]
pub struct CallsRuntime {
    local: LocalVrchat,
    local_read: Option<Instant>,
    /// The friend hashes Orbiters has (sent again when they change or the server lost them).
    friends_sent: Option<u64>,
    link: CallLink,
    contacts: Vec<CallContact>,
    contacts_read: Option<Instant>,
    presence_sent: Option<Instant>,
    available_sent: Option<bool>,
    call: Option<Call>,
    worker: Option<Worker>,
    /// The panel lists the contacts (the Orb's Call menu is open), and which one is chosen.
    pub(super) menu_open: bool,
    pub(super) picked: usize,
    /// The event stream is open.
    streaming: bool,
    stop: bool,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn friends_digest(hashes: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    hashes.hash(&mut h);
    h.finish()
}

/// Orbiters refused: this VRChat account is not the member's verified link (403 on calls).
fn not_linked(e: &anyhow::Error) -> bool {
    e.downcast_ref::<orbiters_core::account::ApiError>().is_some_and(|a| a.status == 403)
}

/// The panel's text for the call menu: four contacts around the chosen one, `>` before it.
pub fn menu_text(contacts: &[CallContact], picked: usize) -> String {
    if contacts.is_empty() {
        return "No friend to call right now. They need Orbiters and Orb on.".into();
    }
    let picked = picked.min(contacts.len() - 1);
    let first = (picked / PAGE) * PAGE;
    contacts[first..(first + PAGE).min(contacts.len())]
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}{}", if first + i == picked { "> " } else { "  " }, c.name))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Which contact a radial at `value` (0 to 1) chooses among `count`.
pub fn pick(value: f32, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    ((value.clamp(0.0, 1.0) * count as f32) as usize).min(count - 1)
}

impl super::Controller {
    /// Starts the presence thread (it does nothing while calls are off).
    pub(super) fn start_calls(self: &Shared) {
        let ctl = self.clone();
        let _ = std::thread::Builder::new().name("calls".into()).spawn(move || loop {
            if ctl.calls.lock().unwrap().stop {
                break;
            }
            ctl.calls_tick();
            std::thread::sleep(Duration::from_secs(2));
        });
    }

    pub(super) fn calls_stop(self: &Shared) {
        let (api, was_on) = {
            let mut rt = self.calls.lock().unwrap();
            rt.stop = true;
            rt.worker = None;
            (self.account_api(), rt.available_sent.is_some())
        };
        if let (Some((api, token)), true) = (api, was_on) {
            let _ = api.agent().delete(&api.url("orb/calls/presence")).header("Authorization", &format!("Bearer {token}")).call();
        }
    }

    fn calls_tick(self: &Shared) {
        let settings = self.settings();
        let on = settings.orb.enabled && settings.orb.calls;
        let account = self.account_api();
        let Some((api, token)) = account.filter(|_| on) else {
            let was = self.calls.lock().unwrap().available_sent.take().is_some();
            if was {
                if let Some((api, token)) = self.account_api() {
                    let _ = api.agent().delete(&api.url("orb/calls/presence")).header("Authorization", &format!("Bearer {token}")).call();
                }
            }
            let link = if !on { CallLink::Off } else { CallLink::SignedOut };
            self.calls_publish(|rt| {
                rt.link = link.clone();
                rt.contacts.clear();
            });
            return;
        };
        // This computer's VRChat account and friends.
        let read_due = self.calls.lock().unwrap().local_read.is_none_or(|t| t.elapsed() >= FRIENDS_EVERY);
        if read_due {
            let log_account = {
                let snap = self.snapshot.lock().unwrap();
                let w = &snap.vrchat.world;
                w.local_player.as_ref().and_then(|me| w.players.iter().find(|(_, n)| *n == me).map(|(id, _)| id.clone()))
            };
            let local = vrchat_accounts::read(log_account.as_deref());
            let mut rt = self.calls.lock().unwrap();
            rt.local = local;
            rt.local_read = Some(Instant::now());
        }
        let Some((account, source)) = self.calls.lock().unwrap().local.account.clone() else {
            self.calls_publish(|rt| rt.link = CallLink::NoAccount);
            return;
        };
        let available = {
            let snap = self.snapshot.lock().unwrap();
            snap.vrchat.running && snap.orb.prop_sync.is_some()
        };
        let (due, friends) = {
            let rt = self.calls.lock().unwrap();
            let hashes: Vec<String> = rt.local.friends.iter().map(|f| vrchat_accounts::friend_hash(f)).collect();
            let changed = rt.friends_sent != Some(friends_digest(&hashes));
            let due = rt.presence_sent.is_none_or(|t| t.elapsed() >= PRESENCE_EVERY) || rt.available_sent != Some(available) || changed;
            (due, changed.then_some(hashes))
        };
        if due {
            let mut body = json!({ "available": available, "vrchat_id": account });
            if let Some(hashes) = &friends {
                body["friends"] = json!(hashes);
                body["friends_changed"] = json!(true);
            }
            let answer = api.agent().put(&api.url("orb/calls/presence")).header("Authorization", &format!("Bearer {token}")).send_json(&body);
            match answer.map_err(anyhow::Error::from).and_then(Api::json::<Value>) {
                Ok(v) => {
                    let mut rt = self.calls.lock().unwrap();
                    rt.presence_sent = Some(Instant::now());
                    rt.available_sent = Some(available);
                    if let Some(hashes) = &friends {
                        rt.friends_sent = Some(friends_digest(hashes));
                    }
                    if v["friends_known"] == json!(false) {
                        rt.friends_sent = None;
                    }
                    rt.link = CallLink::Linked { account: account.clone() };
                }
                Err(e) => {
                    let refused = not_linked(&e);
                    let mut rt = self.calls.lock().unwrap();
                    rt.presence_sent = Some(Instant::now());
                    if refused {
                        rt.link = CallLink::NotLinked { account: account.clone(), source: source.name().to_string() };
                    } else {
                        eprintln!("calls presence: {e:#}");
                    }
                }
            }
        }
        let linked = matches!(self.calls.lock().unwrap().link, CallLink::Linked { .. });
        if !linked {
            self.calls_publish(|_| {});
            return;
        }
        // The event stream, kept open on a thread of its own.
        let start_stream = !std::mem::replace(&mut self.calls.lock().unwrap().streaming, true);
        if start_stream {
            let ctl = self.clone();
            let (api, token) = (api.clone(), token.clone());
            let _ = std::thread::Builder::new().name("calls-events".into()).spawn(move || {
                if let Err(e) = ctl.calls_events(&api, &token) {
                    eprintln!("calls events: {e:#}");
                }
                ctl.calls.lock().unwrap().streaming = false;
            });
        }
        let contacts_due = self.calls.lock().unwrap().contacts_read.is_none_or(|t| t.elapsed() >= CONTACTS_EVERY);
        if contacts_due && available {
            let answer = api.agent().get(&api.url("orb/calls/contacts")).header("Authorization", &format!("Bearer {token}")).call();
            if let Ok(v) = answer.map_err(anyhow::Error::from).and_then(Api::json::<Value>) {
                let contacts: Vec<CallContact> = v["contacts"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|c| Some(CallContact { id: c["id"].as_str()?.to_string(), name: c["name"].as_str()?.to_string() })).collect())
                    .unwrap_or_default();
                let mut rt = self.calls.lock().unwrap();
                rt.contacts = contacts;
                rt.contacts_read = Some(Instant::now());
            }
        }
        self.calls_publish(|_| {});
    }

    /// Reads the server's event stream until it ends.
    fn calls_events(self: &Shared, api: &Api, token: &str) -> anyhow::Result<()> {
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(Duration::from_secs(15))).build().into();
        let response =
            agent.get(&api.url("orb/calls/events")).header("Authorization", &format!("Bearer {token}")).header("Accept", "text/event-stream").call()?;
        let reader = BufReader::new(response.into_body().into_reader());
        let (mut event, mut data) = (String::new(), String::new());
        for line in reader.lines() {
            if self.calls.lock().unwrap().stop {
                break;
            }
            let line = line?;
            if line.is_empty() {
                if !event.is_empty() {
                    let value: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                    self.on_call_event(&event, value);
                }
                event.clear();
                data.clear();
            } else if let Some(v) = line.strip_prefix("event:") {
                event = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(v.strip_prefix(' ').unwrap_or(v));
            }
        }
        Ok(())
    }

    /// One event of the server's stream.
    fn on_call_event(self: &Shared, event: &str, v: Value) {
        let call_id = v["call"].as_str().unwrap_or_default().to_string();
        match event {
            "incoming" => {
                let name = v["from"]["name"].as_str().unwrap_or("A friend").to_string();
                let busy = self.calls.lock().unwrap().call.is_some();
                if busy {
                    let ctl = self.clone();
                    std::thread::spawn(move || ctl.calls_post(&format!("orb/calls/{call_id}/decline"), json!({})));
                    return;
                }
                let offer = v["offer"].as_str().unwrap_or_default().to_string();
                {
                    let mut rt = self.calls.lock().unwrap();
                    rt.call = Some(Call {
                        id: Some(call_id),
                        contact: v["from"]["id"].as_str().unwrap_or_default().to_string(),
                        name: name.clone(),
                        phase: CallPhase::Ringing,
                        incoming: true,
                        offer: Some((offer, v["ice_servers"].clone())),
                        candidates: Vec::new(),
                        connected_at: None,
                        muted: false,
                        levels: (0.0, 0.0),
                    });
                }
                self.orb_say(&format!("{name} is calling. Touch me or press Call to answer."));
                self.notify(NoticeKind::Info, format!("{name} is calling: answer on the Orb page, in the SteamVR tab or by touching the orb."));
            }
            "answered" => {
                let answer = v["answer"].as_str().unwrap_or_default().to_string();
                self.calls_worker_send(json!({ "cmd": "remote_answer", "sdp": answer }));
                self.calls_publish(|rt| {
                    if let Some(c) = rt.call.as_mut() {
                        c.phase = CallPhase::Connecting;
                    }
                });
            }
            "candidates" => {
                if let Some(list) = v["candidates"].as_array() {
                    let list: Vec<Value> = list.iter().map(|c| if c.is_string() { json!({ "candidate": c }) } else { c.clone() }).collect();
                    self.calls_worker_send(json!({ "cmd": "candidates", "candidates": list }));
                }
            }
            "declined" => self.calls_finish("declined"),
            "ended" => self.calls_finish(if v["reason"] == "timeout" { "unanswered" } else { "ended" }),
            _ => {}
        }
    }

    fn calls_post(self: &Shared, path: &str, body: Value) -> Option<Value> {
        let (api, token) = self.account_api()?;
        match api
            .agent()
            .post(&api.url(path))
            .header("Authorization", &format!("Bearer {token}"))
            .send_json(&body)
            .map_err(anyhow::Error::from)
            .and_then(Api::json::<Value>)
        {
            Ok(v) => Some(v),
            Err(e) => {
                eprintln!("calls {path}: {e:#}");
                self.notify(NoticeKind::Warning, format!("Call: {e}"));
                None
            }
        }
    }

    fn calls_worker_send(self: &Shared, command: Value) {
        let mut rt = self.calls.lock().unwrap();
        if rt.worker.is_none() {
            match Worker::spawn(self.clone()) {
                Ok(w) => rt.worker = Some(w),
                Err(e) => {
                    drop(rt);
                    self.notify(NoticeKind::Error, e.to_string());
                    return;
                }
            }
        }
        if let Some(w) = rt.worker.as_mut() {
            if let Err(e) = w.send(command) {
                eprintln!("call worker: {e:#}");
                rt.worker = None;
            }
        }
    }

    /// Calls a contact (the window, the overlay or the Orb's menu).
    pub fn call_contact(self: &Shared, contact: String) {
        let name = {
            let rt = self.calls.lock().unwrap();
            if rt.call.is_some() {
                return;
            }
            rt.contacts.iter().find(|c| c.id == contact).map(|c| c.name.clone())
        };
        let Some(name) = name else {
            self.notify(NoticeKind::Warning, "This friend cannot be called right now.");
            return;
        };
        self.calls_publish(|rt| {
            rt.menu_open = false;
            rt.call = Some(Call {
                id: None,
                contact: contact.clone(),
                name: name.clone(),
                phase: CallPhase::Calling,
                incoming: false,
                offer: None,
                candidates: Vec::new(),
                connected_at: None,
                muted: false,
                levels: (0.0, 0.0),
            });
        });
        self.orb_say(&format!("Calling {name}…"));
        let ctl = self.clone();
        std::thread::spawn(move || {
            let Some((api, token)) = ctl.account_api() else { return };
            let ice = api.agent().get(&api.url("orb/calls/ice-servers")).header("Authorization", &format!("Bearer {token}")).call();
            let ice = ice.map_err(anyhow::Error::from).and_then(Api::json::<Value>).map(|v| v["ice_servers"].clone()).unwrap_or(json!([]));
            let mic = ctl.settings().orb.mic_device;
            ctl.calls_worker_send(json!({ "cmd": "offer", "ice_servers": ice, "mic": (!mic.is_empty()).then_some(mic) }));
        });
    }

    /// Answers the ringing call.
    pub fn call_answer(self: &Shared) {
        let offer = {
            let mut rt = self.calls.lock().unwrap();
            match rt.call.as_mut() {
                Some(c) if c.incoming && c.phase == CallPhase::Ringing => {
                    c.phase = CallPhase::Connecting;
                    c.offer.take()
                }
                _ => None,
            }
        };
        let Some((offer, ice)) = offer else { return };
        let mic = self.settings().orb.mic_device;
        self.calls_worker_send(json!({ "cmd": "answer", "offer": offer, "ice_servers": ice, "mic": (!mic.is_empty()).then_some(mic) }));
        self.calls_publish(|_| {});
    }

    /// Declines a ringing call or hangs up.
    pub fn call_end(self: &Shared) {
        let (id, ringing_in) = {
            let rt = self.calls.lock().unwrap();
            match &rt.call {
                Some(c) => (c.id.clone(), c.incoming && c.phase == CallPhase::Ringing),
                None => return,
            }
        };
        if let Some(id) = id {
            let ctl = self.clone();
            std::thread::spawn(move || {
                ctl.calls_post(&format!("orb/calls/{id}/{}", if ringing_in { "decline" } else { "end" }), json!({}));
            });
        }
        self.calls_finish(if ringing_in { "declined" } else { "ended" });
    }

    pub fn call_mute(self: &Shared, on: bool) {
        self.calls_worker_send(json!({ "cmd": "mute", "on": on }));
        self.calls_publish(|rt| {
            if let Some(c) = rt.call.as_mut() {
                c.muted = on;
            }
        });
    }

    /// The call is over (either side, or it failed): the worker hangs up and the panel says so.
    fn calls_finish(self: &Shared, how: &str) {
        let name = {
            let mut rt = self.calls.lock().unwrap();
            let call = rt.call.take();
            if let Some(w) = rt.worker.as_mut() {
                let _ = w.send(json!({ "cmd": "hangup" }));
            }
            call.map(|c| c.name)
        };
        if let Some(name) = name {
            let text = match how {
                "declined" => format!("{name} can't talk right now."),
                "unanswered" => format!("{name} didn't answer."),
                "failed" => format!("The call with {name} dropped."),
                _ => format!("Call with {name} ended."),
            };
            self.orb_say(&text);
        }
        self.calls_publish(|_| {});
    }

    /// One event of the call worker.
    fn on_call_worker(self: &Shared, e: Value) {
        match e["event"].as_str().unwrap_or_default() {
            "offer" => {
                let sdp = e["sdp"].as_str().unwrap_or_default().to_string();
                let contact = self.calls.lock().unwrap().call.as_ref().map(|c| c.contact.clone());
                let Some(contact) = contact else { return };
                let ctl = self.clone();
                std::thread::spawn(move || match ctl.calls_post("orb/calls", json!({ "contact": contact, "offer": sdp })) {
                    Some(v) => {
                        let id = v["call"].as_str().map(str::to_string);
                        let waiting = {
                            let mut rt = ctl.calls.lock().unwrap();
                            match rt.call.as_mut() {
                                Some(c) => {
                                    c.id = id.clone();
                                    std::mem::take(&mut c.candidates)
                                }
                                None => Vec::new(),
                            }
                        };
                        if let (Some(id), false) = (id, waiting.is_empty()) {
                            ctl.calls_post(&format!("orb/calls/{id}/candidates"), json!({ "candidates": waiting }));
                        }
                    }
                    None => ctl.calls_finish("failed"),
                });
            }
            "answer" => {
                let sdp = e["sdp"].as_str().unwrap_or_default().to_string();
                let id = self.calls.lock().unwrap().call.as_ref().and_then(|c| c.id.clone());
                if let Some(id) = id {
                    let ctl = self.clone();
                    std::thread::spawn(move || {
                        if ctl.calls_post(&format!("orb/calls/{id}/answer"), json!({ "answer": sdp })).is_none() {
                            ctl.calls_finish("failed");
                        }
                    });
                }
            }
            "candidate" => {
                let candidate = e["candidate"].clone();
                let id = {
                    let mut rt = self.calls.lock().unwrap();
                    match rt.call.as_mut() {
                        Some(c) if c.id.is_none() => {
                            c.candidates.push(candidate.clone());
                            None
                        }
                        Some(c) => c.id.clone(),
                        None => None,
                    }
                };
                if let Some(id) = id {
                    let ctl = self.clone();
                    std::thread::spawn(move || {
                        ctl.calls_post(&format!("orb/calls/{id}/candidates"), json!({ "candidates": [candidate] }));
                    });
                }
            }
            "connected" => {
                let name = self.calls.lock().unwrap().call.as_ref().map(|c| c.name.clone());
                self.calls_publish(|rt| {
                    if let Some(c) = rt.call.as_mut() {
                        c.phase = CallPhase::Connected;
                        c.connected_at = Some(now_ms());
                    }
                });
                if let Some(name) = name {
                    self.orb_say(&format!("On a call with {name}."));
                }
            }
            "levels" => {
                let levels = (e["mic"].as_f64().unwrap_or(0.0) as f32, e["remote"].as_f64().unwrap_or(0.0) as f32);
                self.calls_publish(|rt| {
                    if let Some(c) = rt.call.as_mut() {
                        c.levels = levels;
                    }
                });
            }
            "ended" => {
                let active = self.calls.lock().unwrap().call.is_some();
                if active {
                    let reason = e["reason"].as_str().unwrap_or("ended");
                    if reason == "failed" {
                        let id = self.calls.lock().unwrap().call.as_ref().and_then(|c| c.id.clone());
                        if let Some(id) = id {
                            let ctl = self.clone();
                            std::thread::spawn(move || {
                                ctl.calls_post(&format!("orb/calls/{id}/end"), json!({}));
                            });
                        }
                        self.calls_finish("failed");
                    }
                }
            }
            "error" => {
                if let Some(m) = e["message"].as_str() {
                    self.notify(NoticeKind::Warning, format!("Call: {m}"));
                }
            }
            _ => {}
        }
    }

    // ---- The Orb's Call menu (avatar parameters, contracts Orb/Call/*) ----

    /// `Orb/Call/Menu` pressed: the contacts on the panel, or off it.
    pub(super) fn calls_menu(self: &Shared) {
        let (open, text) = {
            let mut rt = self.calls.lock().unwrap();
            rt.menu_open = !rt.menu_open;
            (rt.menu_open, menu_text(&rt.contacts, rt.picked))
        };
        if open {
            self.orb_say(&text);
        } else {
            self.orb_command(crate::ipc::OrbCommand::HidePanel);
        }
        self.calls_publish(|_| {});
    }

    /// `Orb/Call/Pick` moved.
    pub(super) fn calls_pick(self: &Shared, value: f32) {
        let text = {
            let mut rt = self.calls.lock().unwrap();
            let picked = pick(value, rt.contacts.len());
            if picked == rt.picked || !rt.menu_open {
                rt.picked = picked;
                None
            } else {
                rt.picked = picked;
                Some(menu_text(&rt.contacts, picked))
            }
        };
        if let Some(text) = text {
            self.orb_say(&text);
        }
        self.calls_publish(|_| {});
    }

    /// `Orb/Call/Go` (or a touch while it rings): answers a ringing call, or calls the chosen contact.
    pub(super) fn calls_go(self: &Shared) -> bool {
        let (ringing, chosen) = {
            let rt = self.calls.lock().unwrap();
            let ringing = rt.call.as_ref().is_some_and(|c| c.incoming && c.phase == CallPhase::Ringing);
            let chosen = (rt.menu_open && rt.call.is_none()).then(|| rt.contacts.get(rt.picked).map(|c| c.id.clone())).flatten();
            (ringing, chosen)
        };
        if ringing {
            self.call_answer();
            return true;
        }
        if let Some(id) = chosen {
            self.call_contact(id);
            return true;
        }
        false
    }

    /// Whether a call rings for this member (touching the orb answers it).
    pub(super) fn call_ringing_in(&self) -> bool {
        self.calls.lock().unwrap().call.as_ref().is_some_and(|c| c.incoming && c.phase == CallPhase::Ringing)
    }

    fn calls_publish(self: &Shared, change: impl FnOnce(&mut CallsRuntime)) {
        let status = {
            let mut rt = self.calls.lock().unwrap();
            change(&mut rt);
            crate::state::CallsStatus {
                link: rt.link.clone(),
                contacts: rt.contacts.clone(),
                call: rt.call.as_ref().map(|c| CallView {
                    name: c.name.clone(),
                    phase: c.phase,
                    incoming: c.incoming,
                    connected_at_ms: c.connected_at,
                    muted: c.muted,
                    mic_level: c.levels.0,
                    remote_level: c.levels.1,
                }),
                menu_open: rt.menu_open,
                picked: rt.picked,
            }
        };
        self.update_if(|s| {
            let changed = s.orb.calls != status;
            s.orb.calls = status;
            changed
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contacts(n: usize) -> Vec<CallContact> {
        (0..n).map(|i| CallContact { id: format!("c_{i}"), name: format!("Friend {i}") }).collect()
    }

    #[test]
    fn the_menu_shows_four_contacts_around_the_chosen_one() {
        assert!(menu_text(&[], 0).starts_with("No friend"));
        let list = contacts(6);
        assert_eq!(menu_text(&list, 1), "  Friend 0\n> Friend 1\n  Friend 2\n  Friend 3");
        assert_eq!(menu_text(&list, 5), "  Friend 4\n> Friend 5");
        assert_eq!(menu_text(&list, 99), "  Friend 4\n> Friend 5", "past the end: the last");
    }

    #[test]
    fn the_radial_spreads_over_the_list() {
        assert_eq!(pick(0.0, 6), 0);
        assert_eq!(pick(0.5, 6), 3);
        assert_eq!(pick(1.0, 6), 5);
        assert_eq!(pick(0.7, 0), 0);
    }
}
