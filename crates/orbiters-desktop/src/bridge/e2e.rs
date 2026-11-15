//! The whole bridge over its real transport, against the fake VRChat of `lab.rs`: a client here speaks HTTP to the real
//! listener on a real socket. Nothing in here sends input to, or takes the focus from, this computer's desktop: the fake game
//! records keyboard and mouse input, and its window (real, found, captured and closed through the real calls) is off every
//! screen and never activated.

use super::clients::{Change, Client as Agent, ClientCli, Entry, Registration, Registrations};
use super::input::{Hold, Input};
use super::keeper::{Flag, Keeper};
use super::lab::{Behaviour, LabFlag, LabGame, LabWindow, StandIn, AVATAR_NOT_ALLOWED, CAMERA_ICON, LAB_USER, LAB_WORLD_A, LAB_WORLD_B};
use super::mcp::{Server, LEGACY, MODERN};
use super::person::{Heard, Listener};
use super::{Bridge, Command, Status, Timing};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const TOKEN: &str = "test-token";

/// A plain HTTP exchange on a connection of its own: (status, headers, body as JSON).
fn exchange(port: u16, method: &str, path: &str, headers: &[(&str, String)], body: &str) -> std::io::Result<(u16, String, Value)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut request = format!("{method} {path} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n", body.len()); // i18n: not shown
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    let mut reply = Vec::new();
    let mut chunk = [0u8; 65536];
    let mut total = None;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => reply.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
        if total.is_none() {
            if let Some(end) = reply.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&reply[..end]).to_ascii_lowercase();
                let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length: ")).and_then(|l| l.trim().parse().ok()).unwrap_or(0);
                total = Some(end + 4 + length);
            }
        }
        if total.is_some_and(|t| reply.len() >= t) {
            break;
        }
    }
    let end = reply.windows(4).position(|w| w == b"\r\n\r\n").expect("an HTTP answer");
    let head = String::from_utf8_lossy(&reply[..end]).into_owned();
    let status = head.split(' ').nth(1).and_then(|s| s.parse().ok()).expect("a status");
    let body = serde_json::from_slice(&reply[end + 4..]).unwrap_or(Value::Null);
    Ok((status, head, body))
}

/// The usual headers of the current revision for a message, with the right token.
fn modern_headers<'a>(port: u16, method: &str, name: Option<&str>) -> Vec<(&'a str, String)> {
    let mut headers = vec![
        ("Host", format!("127.0.0.1:{port}")), // i18n: not shown
        ("Authorization", format!("Bearer {TOKEN}")), // i18n: not shown
        ("Content-Type", "application/json".to_string()),
        ("Accept", "application/json, text/event-stream".to_string()), // i18n: not shown
        ("MCP-Protocol-Version", MODERN.to_string()),
        ("Mcp-Method", method.to_string()),
    ];
    if let Some(name) = name {
        headers.push(("Mcp-Name", name.to_string()));
    }
    headers
}

fn modern_message(method: &str, params: Value) -> String {
    let mut params = params;
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": { "name": "e2e", "version": "1" },
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    json!({ "jsonrpc": "2.0", "id": 7, "method": method, "params": params }).to_string()
}

/// A client of the current revision.
struct Client {
    port: u16,
}

impl Client {
    fn rpc(&self, method: &str, params: Value) -> (u16, Value) {
        let name = params["name"].as_str().or(params["uri"].as_str()).map(str::to_owned);
        let headers = modern_headers(self.port, method, name.as_deref());
        let (status, _, body) = exchange(self.port, "POST", "/mcp", &headers, &modern_message(method, params)).expect("the listener answers");
        (status, body)
    }

    /// A tool's outcome: `Ok(structured)` or `Err(failure)`.
    fn tool(&self, name: &str, arguments: Value) -> Result<Value, Value> {
        let (status, body) = self.rpc("tools/call", json!({ "name": name, "arguments": arguments }));
        assert_eq!(status, 200, "{name}: {body}");
        assert_eq!(body["result"]["resultType"], "complete", "{name}: {body}");
        let result = &body["result"];
        if result["isError"] == true {
            Err(result["structuredContent"].clone())
        } else {
            Ok(result["structuredContent"].clone())
        }
    }

    fn state(&self) -> Value {
        self.tool("get_state", json!({})).expect("get_state always answers")
    }

    fn fails(&self, name: &str, arguments: Value, code: &str) -> Value {
        let failure = self.tool(name, arguments).expect_err(&format!("{name} should fail with {code}"));
        assert_eq!(failure["code"], code, "{name}: {failure}");
        assert!(!failure["next"].as_str().unwrap_or_default().is_empty(), "every failure says what to do next: {failure}");
        failure
    }
}

/// How long a test waits for what has to happen sooner or later: far longer than any busy machine takes (the full test run
/// keeps every core busy for ten minutes), and only ever waited in full by a test that fails. A wait that proves something
/// is quick keeps its own short limit.
const PATIENCE: Duration = Duration::from_secs(30);

/// The bridge on a free port with the lab game: no keeper, as if the flag said yes. A start may take [`PATIENCE`] to arrive:
/// the lab loads in 200 ms, which a busy machine stretched past the two seconds this used to allow.
fn lab(name: &str) -> (Arc<LabGame>, Arc<Bridge>, Server, Client) {
    lab_timed(name, PATIENCE)
}

/// [`lab`] for a test that wants a start that never arrives to be given up on: after two seconds.
fn lab_giving_up(name: &str) -> (Arc<LabGame>, Arc<Bridge>, Server, Client) {
    lab_timed(name, Duration::from_secs(2))
}

fn lab_timed(name: &str, start_limit: Duration) -> (Arc<LabGame>, Arc<Bridge>, Server, Client) {
    let dir = std::env::temp_dir().join(format!("orbiters-bridge-e2e-{name}-{}", uuid::Uuid::new_v4().simple()));
    let game = LabGame::new(&dir, None);
    let bridge = Bridge::with_timing(game.clone(), None, Timing { instance_every: Duration::from_millis(50), start_limit });
    let server = Server::start(bridge.clone(), 0, TOKEN.to_string()).expect("a free port");
    let client = Client { port: server.port() };
    (game, bridge, server, client)
}

fn until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let begin = Instant::now();
    while begin.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    done()
}

/// Goes to world A and waits for it.
fn arrive(client: &Client) -> Value {
    let arrived = client.tool("go_to_world", json!({ "world_id": LAB_WORLD_A })).expect("go_to_world");
    assert_eq!(arrived["arrived"], true, "{arrived}");
    arrived
}

// ---- The flag

struct Keeping {
    keeper: Arc<Keeper>,
    flag: Arc<LabFlag>,
    stand_in: StandIn,
    status: Arc<Mutex<Status>>,
}

fn keeping(name: &str, proof: Duration) -> Keeping {
    let dir = std::env::temp_dir().join(format!("orbiters-bridge-keeper-{name}-{}", uuid::Uuid::new_v4().simple()));
    let game = LabGame::new(&dir, None);
    let bridge = Bridge::with_timing(game, None, Timing::default());
    let stand_in = StandIn::start();
    let flag = Arc::new(LabFlag { api: orbiters_core::account::Api::at(&stand_in.address), signed_in: AtomicBool::new(true), enabled: AtomicBool::new(true) });
    let status: Arc<Mutex<Status>> = Arc::default();
    let published = status.clone();
    let keys = dir.join("keys.dat");
    // A port of its own, so that tests in parallel and an app running on this computer do not meet.
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    super::keeper::Keys { port, token: TOKEN.to_string() }.save(&keys);
    let keeper = Keeper::new(
        bridge,
        flag.clone(),
        keys,
        Duration::from_millis(100),
        proof,
        Box::new(move |s| *published.lock().unwrap() = s),
        Box::new(|_| Some(Listener::none())),
    );
    keeper.start();
    Keeping { keeper, flag, stand_in, status }
}

impl Keeping {
    fn listening(&self) -> bool {
        self.keeper.port().is_some()
    }

    fn allowed(&self) -> bool {
        self.status.lock().unwrap().allowed
    }
}

/// A client's list of servers, as a real client's command would change it.
#[derive(Default)]
struct FakeClient(Mutex<Option<Entry>>);

impl ClientCli for FakeClient {
    fn client(&self) -> Agent {
        Agent::ClaudeCode
    }
    fn installed(&self) -> bool {
        true
    }
    fn read(&self) -> Result<Option<Entry>, String> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn add(&self, address: &str, token: &str) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(Entry { address: Some(address.into()), token: Some(token.into()), ..Default::default() });
        Ok(())
    }
    fn remove(&self) -> Result<(), String> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}

#[test]
fn settings_buttons_reach_the_client_and_a_new_token_or_an_open_keeps_it_right() {
    let k = keeping("clients", Duration::from_secs(60));
    let fake = Arc::new(FakeClient::default());
    k.keeper.registrations.set(Registrations::new(vec![fake.clone()], Box::new(|_, _| {}))).ok().expect("first");
    assert!(until(PATIENCE, || k.listening()));
    let state = || k.status.lock().unwrap().clients.first().cloned();
    let registration = || state().map(|c| c.registration);

    // A look, and what Settings shows.
    k.keeper.command(Command::CheckClients);
    assert!(until(PATIENCE, || registration() == Some(Registration::NotAdded)), "{:?}", state());

    // The one click.
    k.keeper.command(Command::Client { client: Agent::ClaudeCode, change: Change::Add });
    assert!(until(PATIENCE, || registration() == Some(Registration::Added)), "{:?}", state());
    let added = fake.0.lock().unwrap().clone().unwrap();
    assert_eq!(added.token.as_deref(), Some(TOKEN));
    assert_eq!(added.address, Some(format!("http://127.0.0.1:{}/mcp", k.keeper.port().unwrap())));

    // A new token: the entry follows, and the row says what still needs doing.
    k.keeper.command(Command::NewToken);
    assert!(until(PATIENCE, || fake.0.lock().unwrap().as_ref().is_some_and(|e| e.token.as_deref() != Some(TOKEN))), "{:?}", state());
    assert!(until(PATIENCE, || registration() == Some(Registration::Added) && state().is_some_and(|c| !c.busy)), "{:?}", state());
    assert!(state().unwrap().note.unwrap().contains("session already open"));
    assert_eq!(fake.0.lock().unwrap().as_ref().unwrap().token, k.status.lock().unwrap().token, "the one the bridge now wants");

    // The address changed behind its back, as a port moved by another program does: the next opening fixes it.
    fake.0.lock().unwrap().as_mut().unwrap().address = Some("http://127.0.0.1:1/mcp".into());
    k.flag.enabled.store(false, Ordering::Relaxed);
    assert!(until(PATIENCE, || !k.listening()));
    k.flag.enabled.store(true, Ordering::Relaxed);
    assert!(until(PATIENCE, || k.listening()));
    let port = k.keeper.port().unwrap();
    assert!(
        until(PATIENCE, || fake.0.lock().unwrap().as_ref().is_some_and(|e| e.address == Some(format!("http://127.0.0.1:{port}/mcp")))),
        "{:?}",
        state()
    );

    // And the way back.
    assert!(until(PATIENCE, || state().is_some_and(|c| !c.busy)));
    k.keeper.command(Command::Client { client: Agent::ClaudeCode, change: Change::Remove });
    assert!(until(PATIENCE, || registration() == Some(Registration::NotAdded)), "{:?}", state());
    assert!(fake.0.lock().unwrap().is_none());
}

#[test]
fn only_an_account_the_server_says_has_it_with_the_setting_on_gets_a_listener() {
    let k = keeping("flag", Duration::from_secs(60));
    // Owner, setting on: it opens, and the status shows address and token.
    assert!(until(Duration::from_secs(5), || k.listening()), "opens for an owner");
    let port = k.keeper.port().unwrap();
    assert!(until(Duration::from_secs(2), || k.status.lock().unwrap().listening));
    assert_eq!(k.status.lock().unwrap().address.as_deref(), Some(format!("http://127.0.0.1:{port}/mcp").as_str()));
    assert_eq!(k.status.lock().unwrap().token.as_deref(), Some(TOKEN));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());

    // The setting off: nothing listens, though the section still shows.
    k.flag.enabled.store(false, Ordering::Relaxed);
    assert!(until(Duration::from_secs(5), || !k.listening()));
    assert!(until(Duration::from_secs(2), || TcpStream::connect(("127.0.0.1", port)).is_err()), "the port closed");
    assert!(k.allowed());
    assert_eq!(k.status.lock().unwrap().token, None, "no listener, no token shown");

    // On again; then the server stops saying owner: closed, and the section goes too.
    k.flag.enabled.store(true, Ordering::Relaxed);
    assert!(until(Duration::from_secs(5), || k.listening()));
    k.stand_in.owner.store(false, Ordering::Relaxed);
    assert!(until(Duration::from_secs(5), || !k.listening()), "closes when the server says no");
    assert!(until(Duration::from_secs(2), || !k.allowed()));
    assert!(until(Duration::from_secs(2), || TcpStream::connect(("127.0.0.1", port)).is_err()));
    assert_eq!(*k.status.lock().unwrap(), Status::default(), "nothing of it is left for Settings to show");

    // Owner again, then signed out: closed at once, without waiting for anything.
    k.stand_in.owner.store(true, Ordering::Relaxed);
    assert!(until(Duration::from_secs(5), || k.listening()));
    k.flag.signed_in.store(false, Ordering::Relaxed);
    assert!(until(Duration::from_millis(1500), || !k.listening()), "signed out closes it");
    assert!(until(Duration::from_secs(1), || !k.allowed()), "and Settings hears of it");
    k.keeper.shutdown();
}

#[test]
fn a_server_out_of_reach_ends_it_once_the_proof_is_too_old_and_starts_nothing_without_proof() {
    let k = keeping("reach", Duration::from_millis(1500));
    assert!(until(Duration::from_secs(5), || k.listening()));
    k.stand_in.reachable.store(false, Ordering::Relaxed);
    let lost = Instant::now();
    assert!(k.listening(), "a recent proof stands for a moment");
    assert!(until(Duration::from_secs(10), || !k.listening()), "fails closed when the proof is old");
    assert!(lost.elapsed() >= Duration::from_millis(500), "not before: {:?}", lost.elapsed());
    k.keeper.shutdown();

    // Never reachable: nothing ever opens, the setting on or not.
    let dir = std::env::temp_dir().join(format!("orbiters-bridge-never-{}", uuid::Uuid::new_v4().simple()));
    let flag =
        Arc::new(LabFlag { api: orbiters_core::account::Api::at("http://127.0.0.1:9"), signed_in: AtomicBool::new(true), enabled: AtomicBool::new(true) });
    let keeper = Keeper::new(
        Bridge::new(LabGame::new(&dir, None), None),
        flag,
        dir.join("keys.dat"),
        Duration::from_millis(100),
        Duration::from_secs(60),
        Box::new(|_| {}),
        Box::new(|_| Some(Listener::none())),
    );
    keeper.start();
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(keeper.port(), None);
}

#[test]
fn it_stays_closed_when_the_person_cannot_be_heard() {
    let dir = std::env::temp_dir().join(format!("orbiters-bridge-deaf-{}", uuid::Uuid::new_v4().simple()));
    let stand_in = StandIn::start();
    let flag = Arc::new(LabFlag { api: orbiters_core::account::Api::at(&stand_in.address), signed_in: AtomicBool::new(true), enabled: AtomicBool::new(true) });
    let status: Arc<Mutex<Status>> = Arc::default();
    let published = status.clone();
    let keeper = Keeper::new(
        Bridge::new(LabGame::new(&dir, None), None),
        flag,
        dir.join("keys.dat"),
        Duration::from_millis(100),
        Duration::from_secs(60),
        Box::new(move |s| *published.lock().unwrap() = s),
        Box::new(|_| None),
    );
    keeper.start();
    assert!(until(Duration::from_secs(5), || status.lock().unwrap().problem.is_some()), "it says why");
    assert_eq!(keeper.port(), None);
}

#[test]
fn stop_takes_control_back_at_once_and_turns_it_off() {
    let dir = std::env::temp_dir().join(format!("orbiters-bridge-stop-{}", uuid::Uuid::new_v4().simple()));
    let game = LabGame::new(&dir, None);
    let bridge = Bridge::with_timing(game.clone(), None, Timing { instance_every: Duration::from_millis(50), start_limit: Duration::from_secs(2) });
    let stand_in = StandIn::start();
    let flag = Arc::new(LabFlag { api: orbiters_core::account::Api::at(&stand_in.address), signed_in: AtomicBool::new(true), enabled: AtomicBool::new(true) });
    let keys = dir.join("keys.dat");
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    super::keeper::Keys { port, token: TOKEN.to_string() }.save(&keys);
    let keeper = Keeper::new(
        bridge.clone(),
        flag.clone(),
        keys,
        Duration::from_millis(200),
        Duration::from_secs(60),
        Box::new(|_| {}),
        Box::new(|_| Some(Listener::none())),
    );
    keeper.start();
    assert!(until(Duration::from_secs(5), || keeper.port().is_some()));
    let client = Client { port };
    arrive(&client);
    let walking = std::thread::spawn(move || client.tool("walk", json!({ "direction": "forward", "seconds": 8 })));
    assert!(until(Duration::from_secs(3), || game.last_value("/input/MoveForward") == Some(1.0)), "walking");
    let stopped = Instant::now();
    keeper.command(Command::Stop);
    assert!(until(Duration::from_secs(1), || game.last_value("/input/MoveForward") == Some(0.0)), "released");
    assert!(stopped.elapsed() < Duration::from_secs(1), "at once: {:?}", stopped.elapsed());
    assert_eq!(walking.join().unwrap().unwrap_err()["code"], "stopped", "the agent is told");
    assert!(!flag.enabled(), "the setting is off, so it stays closed");
    assert_eq!(keeper.port(), None);
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "the port closed");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(keeper.port(), None, "and it does not come back by itself");
}

// ---- The doors

#[test]
fn a_wrong_token_host_or_origin_gets_nothing() {
    let (game, _bridge, _server, client) = lab("doors");
    let port = client.port;
    let call =
        json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "go_to_world", "arguments": { "world_id": LAB_WORLD_A } } }).to_string();
    let good = |extra: &[(&'static str, String)]| {
        let mut headers =
            vec![("Host", format!("127.0.0.1:{port}")), ("Content-Type", "application/json".to_string()), ("Authorization", format!("Bearer {TOKEN}"))];
        for (name, value) in extra {
            headers.retain(|(n, _)| n != name);
            headers.push((*name, value.clone()));
        }
        headers
    };
    // No token, the wrong one.
    let without: Vec<_> = good(&[]).into_iter().filter(|(n, _)| *n != "Authorization").collect();
    let (status, head, _) = exchange(port, "POST", "/mcp", &without, &call).unwrap();
    assert_eq!(status, 401);
    assert!(head.contains("WWW-Authenticate: Bearer"));
    assert_eq!(exchange(port, "POST", "/mcp", &good(&[("Authorization", "Bearer wrong".into())]), &call).unwrap().0, 401);
    // DNS rebinding: a page under another name reaches this port with its own Host.
    assert_eq!(exchange(port, "POST", "/mcp", &good(&[("Host", "evil.example".into())]), &call).unwrap().0, 403);
    assert_eq!(exchange(port, "POST", "/mcp", &good(&[("Host", format!("evil.example:{port}"))]), &call).unwrap().0, 403);
    // A page of another site, even with the right token.
    assert_eq!(exchange(port, "POST", "/mcp", &good(&[("Origin", "https://evil.example".into())]), &call).unwrap().0, 403);
    assert_eq!(exchange(port, "POST", "/mcp", &good(&[("Origin", "null".into())]), &call).unwrap().0, 403);
    // Not JSON's content type (what a page can send without asking), other methods and paths.
    assert_eq!(exchange(port, "POST", "/mcp", &good(&[("Content-Type", "text/plain".into())]), &call).unwrap().0, 415);
    assert_eq!(exchange(port, "GET", "/mcp", &good(&[]), "").unwrap().0, 405);
    assert_eq!(exchange(port, "DELETE", "/mcp", &good(&[]), "").unwrap().0, 405);
    assert_eq!(exchange(port, "OPTIONS", "/mcp", &good(&[]), "").unwrap().0, 405);
    assert_eq!(exchange(port, "POST", "/other", &good(&[]), &call).unwrap().0, 404);
    // None of it got anywhere.
    assert!(game.instances().is_empty() && game.launches().is_empty());
    // No CORS header ever invites a page in.
    let (_, head, _) = exchange(port, "POST", "/mcp", &good(&[]), &json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string()).unwrap();
    assert!(!head.to_ascii_lowercase().contains("access-control"));
}

#[test]
fn it_speaks_the_current_revision_and_the_earlier_ones() {
    let (_game, _bridge, _server, client) = lab("protocol");
    let port = client.port;
    // The current revision: discovery, lists, results marked, headers enforced.
    let (status, body) = client.rpc("server/discover", json!({}));
    assert_eq!(status, 200);
    assert_eq!(body["result"]["supportedVersions"][0], MODERN);
    assert_eq!(body["result"]["resultType"], "complete");
    assert_eq!(body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "orbiters-n4n0-vrchat-bridge");
    let (status, body) = client.rpc("tools/list", json!({}));
    assert_eq!(status, 200);
    assert!(body["result"]["tools"].as_array().unwrap().len() >= 10);
    assert!(body["result"]["ttlMs"].is_number() && body["result"]["cacheScope"] == "private");
    let (_, body) = client.rpc("prompts/list", json!({}));
    assert_eq!(body["result"]["prompts"][0]["name"], "world_tour");
    let (_, body) = client.rpc("prompts/get", json!({ "name": "world_tour", "arguments": { "worlds": "wrld_x" } }));
    assert!(body["result"]["messages"][0]["content"]["text"].as_str().unwrap().contains("wrld_x"));
    let (_, body) = client.rpc("resources/read", json!({ "uri": "bridge://guide" }));
    assert!(body["result"]["contents"][0]["text"].as_str().unwrap().contains("When to give up"));
    let (status, body) = client.rpc("no/such/method", json!({}));
    assert_eq!((status, body["error"]["code"].as_i64()), (404, Some(-32601)));
    // A version it does not know, in the body and the header alike.
    let message = modern_message("tools/list", json!({})).replace(MODERN, "1900-01-01");
    let mut headers = modern_headers(port, "tools/list", None);
    headers.retain(|(n, _)| *n != "MCP-Protocol-Version");
    headers.push(("MCP-Protocol-Version", "1900-01-01".into()));
    let (status, _, body) = exchange(port, "POST", "/mcp", &headers, &message).unwrap();
    assert_eq!((status, body["error"]["code"].as_i64()), (400, Some(-32022)));
    assert_eq!(body["error"]["data"]["requested"], "1900-01-01");
    assert_eq!(body["error"]["data"]["supported"][0], MODERN);
    // Headers that disagree with the body.
    let headers = modern_headers(port, "tools/list", None);
    let (status, _, body) = exchange(port, "POST", "/mcp", &headers, &modern_message("prompts/list", json!({}))).unwrap();
    assert_eq!((status, body["error"]["code"].as_i64()), (400, Some(-32020)));
    let headers = modern_headers(port, "tools/call", Some("walk"));
    let (status, _, body) = exchange(port, "POST", "/mcp", &headers, &modern_message("tools/call", json!({ "name": "get_state", "arguments": {} }))).unwrap();
    assert_eq!((status, body["error"]["code"].as_i64()), (400, Some(-32020)));

    // An earlier revision: the handshake, then ordinary calls with its header.
    let plain = vec![("Host", format!("127.0.0.1:{port}")), ("Authorization", format!("Bearer {TOKEN}")), ("Content-Type", "application/json".to_string())];
    let start = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": LEGACY[0], "capabilities": {}, "clientInfo": { "name": "old-client", "version": "1" } } }).to_string();
    let (status, head, body) = exchange(port, "POST", "/mcp", &plain, &start).unwrap();
    assert_eq!(status, 200);
    assert_eq!(body["result"]["protocolVersion"], LEGACY[0]);
    assert!(!head.to_ascii_lowercase().contains("mcp-session-id"), "no session is minted");
    assert!(body["result"].get("resultType").is_none());
    let (status, _, _) = exchange(port, "POST", "/mcp", &plain, &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string()).unwrap();
    assert_eq!(status, 202);
    let mut with_version = plain.clone();
    with_version.push(("MCP-Protocol-Version", LEGACY[0].to_string()));
    let (status, _, body) = exchange(
        port,
        "POST",
        "/mcp",
        &with_version,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "get_state", "arguments": {} } }).to_string(),
    )
    .unwrap();
    assert_eq!(status, 200);
    assert_eq!(body["result"]["structuredContent"]["vrchat"]["running"], false);
    // An unknown version asked at the handshake gets the newest of that era.
    let start = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2024-01-01", "capabilities": {} } }).to_string();
    assert_eq!(exchange(port, "POST", "/mcp", &plain, &start).unwrap().2["result"]["protocolVersion"], LEGACY[0]);
}

// ---- The rules

#[test]
fn keyboard_and_mouse_go_only_into_the_foreground_game_window() {
    let (game, _bridge, _server, client) = lab("foreground");
    arrive(&client);
    // The real check, on the real window, which is off every screen and never has the foreground.
    client.fails("press", json!({ "input": "esc" }), "not_foreground");
    client.fails("mouse", json!({ "action": "click", "x": 10, "y": 10 }), "not_foreground");
    client.fails("mouse", json!({ "action": "look", "dx": 10 }), "not_foreground");
    assert!(game.injected().is_empty(), "nothing went out");
    // OSC needs no focus.
    client.tool("press", json!({ "input": "jump" })).expect("OSC goes without the focus");
    assert_eq!(game.last_value("/input/Jump"), Some(0.0));
    // With the foreground: the same calls go through, inside the window only.
    game.set_foreground(true);
    client.tool("press", json!({ "input": "esc" })).unwrap();
    assert_eq!(game.injected(), [Input::Key { scan: 0x01, extended: false, down: true }]);
    assert_eq!(game.released(), [Input::Key { scan: 0x01, extended: false, down: false }], "and it came up");
    client.fails("mouse", json!({ "action": "click", "x": 640, "y": 10 }), "outside_window");
    client.fails("mouse", json!({ "action": "move", "x": 10, "y": 360 }), "outside_window");
    client.fails("mouse", json!({ "action": "click" }), "bad_arguments");
    client.tool("mouse", json!({ "action": "click", "x": 12, "y": 34 })).unwrap();
    assert!(game.injected().contains(&Input::MoveTo { x: 12, y: 34 }));
    // Nothing that types or speaks exists, focus or none.
    for input in ["y", "enter", "t", "voice", "/chatbox/input", "/input/Voice"] {
        client.fails("press", json!({ "input": input }), "unknown_input");
    }
    // Losing the focus during a long press ends it, and brings the key up.
    let before = game.released().len();
    let port = client.port;
    let holding = std::thread::spawn(move || Client { port }.tool("press", json!({ "input": "w", "seconds": 8 })));
    assert!(until(Duration::from_secs(2), || game.injected().contains(&Input::Key { scan: 0x11, extended: false, down: true })));
    game.set_foreground(false);
    assert_eq!(holding.join().unwrap().unwrap_err()["code"], "not_foreground");
    assert_eq!(game.released()[before..], [Input::Key { scan: 0x11, extended: false, down: false }]);
}

#[test]
fn a_new_start_is_brought_forward_when_nobody_is_at_the_computer_and_says_how_that_went() {
    let (game, bridge, _server, client) = lab("forward");
    // Windows says no: told, and the keys stay refused.
    let arrived = arrive(&client);
    assert_eq!(arrived["foreground"], false);
    assert!(arrived["next"].as_str().unwrap().contains("focus_game"), "{arrived}");
    client.fails("press", json!({ "input": "esc" }), "not_foreground");
    // Windows says yes: the next start has it, as does focus_game.
    game.set_behaviour(|b| b.focus_works = true);
    assert_eq!(client.tool("focus_game", json!({})).unwrap()["foreground"], true);
    client.tool("press", json!({ "input": "esc" })).expect("the key goes through");
    std::thread::sleep(Duration::from_millis(100));
    let arrived = client.tool("go_to_world", json!({ "world_id": LAB_WORLD_B })).unwrap();
    assert_eq!(arrived["foreground"], true, "{arrived}");
    // The person at the computer: the game is not brought forward over what they do.
    game.set_foreground(false);
    bridge.heard(Heard::PersonInput);
    client.fails("focus_game", json!({}), "paused_by_person");
    let bridge_state = client.state();
    assert_eq!(bridge_state["vrchat"]["window"]["foreground"], false, "left where it was");
}

#[test]
fn a_client_going_away_releases_what_it_held() {
    let (game, bridge, _server, client) = lab("disconnect");
    arrive(&client);
    let body = modern_message("tools/call", json!({ "name": "walk", "arguments": { "direction": "forward", "seconds": 9 } }));
    let headers = modern_headers(client.port, "tools/call", Some("walk"));
    let mut request = format!("POST /mcp HTTP/1.1\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in &headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&body);
    let mut stream = TcpStream::connect(("127.0.0.1", client.port)).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    assert!(until(Duration::from_secs(3), || game.last_value("/input/MoveForward") == Some(1.0)), "walking");
    assert!(!bridge.main.held.is_empty());
    let gone = Instant::now();
    drop(stream);
    assert!(until(Duration::from_secs(2), || game.last_value("/input/MoveForward") == Some(0.0)), "released when the client went away");
    assert!(gone.elapsed() < Duration::from_secs(2), "not at the end of the 9 s: {:?}", gone.elapsed());
    assert!(until(Duration::from_secs(1), || bridge.main.held.is_empty()));
    client.fails("walk", json!({ "direction": "forward", "seconds": 11 }), "bad_duration");
}

#[test]
fn the_sweep_brings_up_what_stayed_down_past_its_time() {
    let (game, bridge, _server, _client) = lab("sweep");
    bridge.main.held.press(Hold::OscButton("/input/MoveForward"), Instant::now() - Duration::from_secs(5));
    assert!(until(Duration::from_secs(2), || bridge.main.held.is_empty()), "the listener's tick took care of it");
    assert!(until(Duration::from_secs(1), || game.last_value("/input/MoveForward") == Some(0.0)));
}

#[test]
fn the_person_always_wins() {
    let (game, bridge, _server, client) = lab("person");
    arrive(&client);
    game.set_foreground(true);
    // Their input during a walk: released at once, the agent told, and nothing more goes through.
    let port = client.port;
    let walking = std::thread::spawn(move || Client { port }.tool("walk", json!({ "direction": "forward", "seconds": 8 })));
    assert!(until(Duration::from_secs(2), || game.last_value("/input/MoveForward") == Some(1.0)));
    bridge.heard(Heard::PersonInput);
    assert!(until(Duration::from_millis(500), || game.last_value("/input/MoveForward") == Some(0.0)), "released at once");
    let failure = walking.join().unwrap().unwrap_err();
    assert_eq!(failure["code"], "paused_by_person");
    assert!(failure["retry_after_s"].as_u64().unwrap() > 0);
    for (tool, args) in [
        ("walk", json!({ "direction": "forward", "seconds": 1 })),
        ("turn", json!({ "degrees": 10 })),
        ("press", json!({ "input": "esc" })),
        ("mouse", json!({ "action": "move", "x": 1, "y": 1 })),
        ("screenshot", json!({})),
        ("take_3d_picture", json!({})),
        ("go_to_world", json!({ "world_id": LAB_WORLD_B })),
        ("close_vrchat", json!({})),
        ("focus_game", json!({})),
    ] {
        client.fails(tool, args, "paused_by_person");
    }
    assert_eq!(game.launches().len(), 1, "no new start");
    assert_eq!(client.state()["bridge"]["paused"]["by"], "person");
    assert!(client.state()["next"].as_str().unwrap().contains("not_paused"));
    // Resuming from Settings forgives the input that came with it.
    bridge.hold(false);
    client.tool("turn", json!({ "degrees": 5 })).expect("goes on");
    // The hotkey: until pressed again, whatever time passes.
    bridge.heard(Heard::Hotkey);
    let failure = client.fails("walk", json!({ "direction": "forward", "seconds": 1 }), "paused_held");
    assert!(failure["message"].as_str().unwrap().contains("Ctrl+Alt+F12"));
    let done = client.tool("wait_for", json!({ "until": "not_paused", "timeout_s": 1 })).unwrap();
    assert_eq!(done["done"], false);
    bridge.heard(Heard::Hotkey);
    assert_eq!(client.tool("wait_for", json!({ "until": "not_paused", "timeout_s": 1 })).unwrap()["done"], true);
}

#[test]
fn only_private_instances_of_ones_own_are_acted_in() {
    for (instance, code) in [
        ("12345~region(eu)", "public"),
        ("12345~hidden(usr_11111111-2222-4333-8444-555555555555)~region(eu)", "friends+"),
        ("12345~private(usr_11111111-2222-4333-8444-555555555555)~canRequestInvite~region(eu)", "invite+"),
        ("12345~private(usr_someone-else)~region(eu)", "someone else's"),
        ("12345~group(grp_1)~groupAccessType(public)~region(eu)", "a group's"),
    ] {
        // A long limit: the leaving has to come from seeing where it is, not from giving up on the start (Wizard Showdown's
        // start_timeout on 2026-11-15 came after four minutes, and said nothing of whether anyone was there).
        let (game, _bridge, _server, client) = lab("private");
        game.set_behaviour(|b| b.land_in = Some(instance.to_string()));
        let begin = Instant::now();
        // VRChat ended up elsewhere than asked: never counted as arrived, and left at once, saying where it was.
        let failure = client.fails("go_to_world", json!({ "world_id": LAB_WORLD_A }), "landed_elsewhere");
        assert!(begin.elapsed() < Duration::from_secs(10), "{instance}: left as soon as it was seen, not at the time limit");
        let message = failure["message"].as_str().unwrap();
        assert!(message.contains(code) && message.contains("12345") && message.contains("closed it"), "{instance}: {failure}");
        assert!(failure["next"].as_str().unwrap().contains("go_to_world"), "{failure}");
        assert!(game.closes() >= 1, "{instance}: closed");
        assert!(until(PATIENCE, || client.state()["vrchat"]["running"] == false), "{instance}: gone");
        let state = client.state();
        assert!(state["bridge"]["last_event"].as_str().unwrap().contains(code), "{state}");
        assert!(!state["bridge"]["last_event"].as_str().unwrap().contains("crashed"), "its going is the bridge's doing: {state}");
        for (tool, args) in [
            ("walk", json!({ "direction": "forward", "seconds": 1 })),
            ("press", json!({ "input": "jump" })),
            ("screenshot", json!({})),
            ("take_3d_picture", json!({})),
        ] {
            client.fails(tool, args, "vrchat_not_running");
        }
        assert_eq!(game.heard().iter().filter(|(h, _)| h.address.starts_with("/input/")).count(), 0, "not one input went out");
        assert!(game.instances().iter().all(|i| i.contains("~private(") && !i.contains("canRequestInvite")), "only private instances were ever made");
        assert!(!game.reconstructions_held());
    }
    let tools = super::tools::list();
    let go = tools.as_array().unwrap().iter().find(|t| t["name"] == "go_to_world").unwrap();
    assert_eq!(go["inputSchema"]["additionalProperties"], false, "no way to name an instance");
}

#[test]
fn a_vrchat_that_moves_from_its_private_instance_to_a_public_one_is_left_at_once() {
    let (game, _bridge, _server, client) = lab("moved");
    arrive(&client);
    // Somewhere else of its own, strictly private: it stays, and that is where it is now.
    game.moves_to(LAB_WORLD_B, &format!("20001~private({LAB_USER})~region(eu)"));
    assert!(until(PATIENCE, || client.state()["place"]["world_id"] == LAB_WORLD_B), "{}", client.state());
    assert_eq!(client.state()["vrchat"]["running"], true);
    assert_eq!(game.closes(), 0);
    // A public one (a portal, a home): closed by the bridge itself, and said.
    game.moves_to(LAB_WORLD_B, "30001~region(eu)");
    assert!(until(PATIENCE, || client.state()["vrchat"]["running"] == false), "{}", client.state());
    assert_eq!(game.closes(), 1);
    let event = client.state()["bridge"]["last_event"].as_str().unwrap().to_string();
    assert!(event.contains("public") && event.contains("closed it"), "{event}");
    assert_eq!(game.heard().iter().filter(|(h, _)| h.address.starts_with("/input/")).count(), 0);
}

#[test]
fn a_restart_never_takes_what_its_last_vrchat_showed_for_its_new_place() {
    // The app follows the newest log, which until the new VRChat writes its own is the last one: its world and instance
    // are the past (the first world tour's side session followed the last world for each whole start). Neither an arrival
    // nor a reason to leave.
    let (game, _bridge, _server, client) = lab("restart");
    arrive(&client);
    std::thread::sleep(Duration::from_millis(60));
    game.set_behaviour(|b| b.load = Duration::from_millis(1500));
    let begin = Instant::now();
    let arrived = client.tool("go_to_world", json!({ "world_id": LAB_WORLD_A })).expect("arrives");
    assert_eq!(arrived["arrived"], true, "{arrived}");
    assert!(begin.elapsed() >= Duration::from_millis(1400), "not on the old log's word");
    assert_eq!(game.closes(), 1, "only the close of the last world's VRChat");
    assert_eq!(arrived["instance"].as_str().unwrap().split('~').next(), game.instances().last().unwrap().split(':').nth(1).unwrap().split('~').next());
}

#[test]
fn a_window_someone_resized_is_put_back_before_anything_is_clicked() {
    let (game, bridge, _server, client) = lab("resized");
    arrive(&client);
    game.set_foreground(true);
    // The owner made profile 1's window 1777 by 1117 on 2026-11-15: the guide's (640, 620) was not the camera icon there.
    game.resize_by_hand(900, 500);
    assert_eq!(client.state()["vrchat"]["window"]["width"], 900);
    // A click on a page the agent has not seen at that size: put back, and nothing clicked.
    let failure = client.fails("mouse", json!({ "action": "click", "x": CAMERA_ICON.0, "y": CAMERA_ICON.1 }), "window_resized");
    assert!(failure["next"].as_str().unwrap().contains("screenshot"), "{failure}");
    assert!(game.injected().is_empty(), "nothing clicked");
    assert_eq!(client.state()["vrchat"]["window"]["width"], 640);
    assert_eq!(client.state()["vrchat"]["window"]["height"], 360);
    // A screenshot puts it back too, and says so; then the click lands where the guide says, at the size it was made for.
    game.resize_by_hand(900, 500);
    let (_, body) = client.rpc("tools/call", json!({ "name": "screenshot", "arguments": {} }));
    let info = &body["result"]["structuredContent"];
    assert_eq!(info["window_restored"], true, "{info}");
    assert_eq!((info["window_width"].clone(), info["window_height"].clone()), (json!(640), json!(360)));
    client.tool("press", json!({ "input": "esc" })).unwrap();
    client.tool("mouse", json!({ "action": "click", "x": CAMERA_ICON.0, "y": CAMERA_ICON.1 })).unwrap();
    assert_eq!(game.clicked_at_size(), vec![(640, 360)]);
    assert_eq!(client.state()["camera"]["open"], true);
    // Never by taking the foreground: the bridge asked Windows for nothing but the size.
    assert!(game.resizes() >= 2);
    // With the person at the computer, a screenshot leaves the size they gave it.
    game.resize_by_hand(900, 500);
    bridge.heard(super::person::Heard::PersonInput);
    let failure = client.fails("screenshot", json!({}), "paused_by_person");
    assert!(failure["retry_after_s"].as_u64().is_some());
    assert_eq!(client.state()["vrchat"]["window"]["width"], 900);
}

#[test]
fn a_vrchat_the_person_started_is_left_alone() {
    let (game, _bridge, _server, client) = lab("foreign");
    let mine = format!("11027~private({LAB_USER})~region(eu)");
    game.set_foreign_game(Some(&mine));
    assert!(until(Duration::from_secs(1), || client.state()["vrchat"]["running"] == true));
    let state = client.state();
    assert_eq!(state["vrchat"]["started_by_bridge"], false);
    assert!(state["next"].as_str().unwrap().contains("person's"));
    assert!(state["vrchat"]["window"].is_null(), "not even its window is looked at");
    for (tool, args) in [
        ("walk", json!({ "direction": "forward", "seconds": 1 })),
        ("press", json!({ "input": "jump" })),
        ("screenshot", json!({})),
        ("take_3d_picture", json!({})),
        ("close_vrchat", json!({})),
        ("go_to_world", json!({ "world_id": LAB_WORLD_A })),
    ] {
        client.fails(tool, args, "not_started_by_bridge");
    }
    assert!(game.instances().is_empty() && game.launches().is_empty() && game.heard().is_empty());
    assert_eq!(game.closes(), 0);
}

#[test]
fn failures_say_what_happened_and_what_to_do() {
    let (game, _bridge, _server, client) = lab_giving_up("failures");
    client.fails("walk", json!({ "direction": "forward", "seconds": 1 }), "vrchat_not_running");
    client.fails("screenshot", json!({}), "vrchat_not_running");
    client.fails("close_vrchat", json!({}), "vrchat_not_running");
    client.fails("go_to_world", json!({ "world_id": "not-a-world" }), "bad_world_id");
    client.fails("go_to_world", json!({ "world_id": "wrld_../../etc" }), "bad_world_id");
    client.fails("go_to_world", json!({ "world_id": LAB_WORLD_A, "region": "mars" }), "bad_arguments");
    client.fails("find_worlds", json!({ "source": "search" }), "bad_arguments");
    client.fails("no_such_tool", json!({}), "unknown_tool");
    // A start that fails hands reconstructions back.
    game.set_behaviour(|b| b.launch_fails = true);
    client.fails("go_to_world", json!({ "world_id": LAB_WORLD_A }), "start_failed");
    assert!(!game.reconstructions_held());
    game.set_behaviour(|b| b.launch_fails = false);
    std::thread::sleep(Duration::from_millis(100));
    // A start that never arrives ends with the reason.
    game.set_behaviour(|b| b.never_arrives = true);
    let failure = client.tool("go_to_world", json!({ "world_id": LAB_WORLD_A })).unwrap_err();
    assert_eq!(failure["code"], "start_timeout");
    assert!(failure["message"].as_str().unwrap().contains("did not load"), "{failure}");
    assert!(!game.reconstructions_held());
    // The game ending by itself is told, and input is not sent to a game that is gone.
    client.tool("close_vrchat", json!({})).unwrap();
    assert_eq!(client.state()["vrchat"]["started_by_bridge"], false);
}

#[test]
fn worlds_are_changed_at_a_sensible_pace() {
    let dir = std::env::temp_dir().join(format!("orbiters-bridge-pace-{}", uuid::Uuid::new_v4().simple()));
    let game = LabGame::new(&dir, None);
    let bridge = Bridge::with_timing(game.clone(), None, Timing { instance_every: Duration::from_secs(30), start_limit: Duration::from_secs(2) });
    let server = Server::start(bridge, 0, TOKEN.to_string()).unwrap();
    let client = Client { port: server.port() };
    arrive(&client);
    let failure = client.fails("go_to_world", json!({ "world_id": LAB_WORLD_B }), "too_soon");
    assert!((1..=30).contains(&failure["retry_after_s"].as_u64().unwrap()), "{failure}");
    assert_eq!(game.instances().len(), 1, "no instance was made");
    assert_eq!(game.closes(), 0, "the game was left alone");
}

/// Both ways of capture on a normal window, the kind a game has: Windows Graphics Capture takes neither tool windows nor
/// windows off every screen, so this one is on the screen for a moment, as good as invisible, its clicks going through it,
/// and in the task bar: not in the everyday run, so as to leave the desktop of the person at this computer alone.
/// `-- --ignored --nocapture`.
#[cfg(windows)]
#[test]
#[ignore = "shows a nearly invisible window for a moment"]
fn both_ways_of_capture_give_the_windows_client_area() {
    let title = format!("{} faint {}", LabWindow::TITLE, uuid::Uuid::new_v4().simple());
    let window = LabWindow::open(&title).expect("the fake window");
    let found = super::win::window_of(std::process::id(), Some(&title)).expect("found");
    assert!(!super::win::is_foreground(found.handle), "it never took the focus");
    std::thread::sleep(Duration::from_millis(300));
    let colour = |f: &super::capture::Bgra, y: u32| {
        let i = (y as usize * f.width as usize + f.width as usize / 2) * 4;
        [f.pixels[i], f.pixels[i + 1], f.pixels[i + 2]]
    };
    for (how, frame) in
        [("graphics capture", super::capture::imp::graphics_capture(found.handle)), ("print window", super::capture::imp::print_window(found.handle))]
    {
        let frame = frame.unwrap_or_else(|e| panic!("{how}: {e}"));
        println!("{how}: {}x{}: {:?} / {:?}", frame.width, frame.height, colour(&frame, 20), colour(&frame, frame.height - 20));
        assert_eq!((frame.width, frame.height), (found.width, found.height), "{how}: the client area, without the frame");
        assert_eq!(colour(&frame, 20), [0, 200, 0], "{how}");
        assert_eq!(colour(&frame, frame.height - 20), [200, 0, 200], "{how}");
    }
    super::win::close_window(found.handle).unwrap();
    assert!(until(Duration::from_secs(2), || window.closed.load(Ordering::Relaxed)));
}

#[test]
fn a_screenshot_is_of_the_window_with_its_size() {
    let (game, _bridge, _server, client) = lab("screenshot");
    arrive(&client);
    let (status, body) = client.rpc("tools/call", json!({ "name": "screenshot", "arguments": {} }));
    assert_eq!(status, 200);
    let content = body["result"]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["mimeType"], "image/jpeg");
    use base64::Engine as _;
    let jpeg = base64::engine::general_purpose::STANDARD.decode(content[0]["data"].as_str().unwrap()).unwrap();
    let picture = image::load_from_memory(&jpeg).unwrap().to_rgb8();
    let info = &body["result"]["structuredContent"];
    assert_eq!(info["window_width"], 640);
    assert_eq!(info["window_height"], 360);
    assert_eq!(info["image_width"], picture.width());
    assert_eq!(info["image_height"], picture.height());
    assert_eq!(info["scale"], 1.0, "at the default width, image pixels are window pixels");
    assert!(info["timestamp"].as_u64().unwrap() > 1_700_000_000);
    assert_eq!(info["all_black"], false);
    // It is that window: green above, magenta below, the right way up, with nothing of the desktop around it.
    let (above, below) = (picture.get_pixel(320, 20), picture.get_pixel(320, 340));
    assert!(above[1] > 170 && above[0] < 60 && above[2] < 60, "{above:?}");
    assert!(below[0] > 170 && below[2] > 170 && below[1] < 60, "{below:?}");
    // Smaller when asked, with the scale to map back.
    let (_, body) = client.rpc("tools/call", json!({ "name": "screenshot", "arguments": { "max_width": 320 } }));
    assert_eq!(body["result"]["structuredContent"]["image_width"], 320);
    assert_eq!(body["result"]["structuredContent"]["scale"], 0.5);
    drop(game);
}

#[test]
fn print_window_answers_for_the_same_window_when_it_is_needed() {
    #[cfg(windows)]
    {
        let title = format!("{} {}", LabWindow::TITLE, uuid::Uuid::new_v4().simple());
        let window = LabWindow::open(&title).expect("the fake window");
        let found = super::win::window_of(std::process::id(), Some(&title)).expect("found by its process and title");
        assert_eq!((found.width, found.height), (640, 360));
        assert!(!super::win::is_foreground(found.handle), "the test never took the focus");
        let frame = super::capture::grab(found.handle).expect("a picture");
        assert_eq!((frame.width, frame.height), (640, 360));
        super::win::close_window(found.handle).unwrap();
        assert!(until(Duration::from_secs(2), || window.closed.load(Ordering::Relaxed)), "closed by its close message");
    }
}

/// A look at what the real servers say for the account signed in on this computer, the app's own read-only question:
/// `cargo test … real_servers -- --ignored --nocapture`.
#[test]
#[ignore = "asks the real servers, with this computer's signed-in account"]
fn real_servers_say_what_they_say_for_the_account_here() {
    use orbiters_core::account::{read, Api, Server};
    for server in [Server::Production, Server::Development] {
        let Some(account) = read(server) else {
            println!("{server:?}: nobody is signed in");
            continue;
        };
        let answer = Api::new(server).member_feature(&account.token, orbiters_core::features::VRCHAT_BRIDGE);
        println!("{server:?}, {:?}: {answer:?}", account.username);
        // A feature that nobody defined is the point: an owner has it, and the same question about a made-up name shows
        // what `features/mine` gives for that (owners pass everything).
        let made_up = Api::new(server).member_feature(&account.token, "n4n0-no-such-feature");
        println!("  a made-up feature: {made_up:?}");
    }
}

// ---- The whole loop

/// What an agent does, unattended, from nothing to the reconstructions done: as the guide says it.
fn tour(client: &Client, game: &LabGame) {
    // Where are we?
    let state = client.state();
    assert_eq!(state["vrchat"]["running"], false);
    assert!(state["next"].as_str().unwrap().contains("go_to_world"), "{state}");
    // A world to go to.
    let worlds = client.tool("find_worlds", json!({ "source": "search", "query": "lab" })).unwrap();
    let first = worlds["worlds"][0]["id"].as_str().unwrap().to_string();
    let second = worlds["worlds"][1]["id"].as_str().unwrap().to_string();
    for (number, world) in [first, second].iter().enumerate() {
        // Started, or restarted, into a new private instance.
        let arrived = client.tool("go_to_world", json!({ "world_id": world, "region": "eu" })).unwrap();
        assert_eq!(arrived["arrived"], true, "{arrived}");
        assert_eq!(arrived["private"], true);
        assert!(arrived["instance"].as_str().unwrap().contains(&format!("private({LAB_USER})")));
        assert_eq!(game.launches().len(), number + 1);
        assert_eq!(game.launches()[number].args()[0], "--no-vr");
        assert!(!game.reconstructions_held(), "handed back on arrival");
        // Sees.
        let (_, picture) = client.rpc("tools/call", json!({ "name": "screenshot", "arguments": {} }));
        assert_eq!(picture["result"]["content"][0]["type"], "image");
        let state = client.state();
        assert_eq!(state["vrchat"]["started_by_bridge"], true);
        assert_eq!(state["place"]["private"], true);
        assert_eq!(state["camera"]["open"], false);
        assert!(state["next"].as_str().unwrap().contains("Open VRChat's camera"), "{state}");
        // A picture before the camera is open is refused, saying how.
        assert!(client.fails("take_3d_picture", json!({}), "camera_closed")["next"].as_str().unwrap().contains("esc"));
        // Opens the camera through the menu, seeing as it goes (the fake's icon is known here: a model reads it off the picture).
        game.set_foreground(true);
        client.tool("press", json!({ "input": "esc" })).unwrap();
        client.tool("mouse", json!({ "action": "click", "x": CAMERA_ICON.0, "y": CAMERA_ICON.1 })).unwrap();
        client.tool("press", json!({ "input": "esc" })).unwrap();
        let state = client.state();
        assert_eq!(state["camera"]["open"], true);
        assert_eq!(state["camera"]["pose_known"], false);
        client.fails("take_3d_picture", json!({}), "camera_pose_unknown");
        // Moves.
        let turned = client.tool("turn", json!({ "degrees": 45 })).unwrap();
        assert!(turned["note"].as_str().unwrap().contains("Approximate"));
        assert_eq!(game.last_value("/input/LookRight"), Some(0.0));
        assert_eq!(client.state()["camera"]["pose_known"], true);
        let walked = client.tool("walk", json!({ "direction": "forward", "seconds": 0.4, "run": true })).unwrap();
        assert_eq!(walked["speed_seen"], 1.5);
        assert_eq!(game.last_value("/input/MoveForward"), Some(0.0));
        assert_eq!(game.last_value("/input/Run"), Some(0.0));
        client.tool("press", json!({ "input": "move_horizontal", "value": -0.5, "seconds": 0.1 })).unwrap();
        assert_eq!(game.last_value("/input/Horizontal"), Some(0.0));
        // Takes pictures, each told through to its session.
        for length in [1, 4, 8] {
            let picture = client.tool("take_3d_picture", json!({ "length_s": length })).unwrap();
            assert_eq!(picture["saved"], true, "{picture}");
            assert_eq!(picture["keyframes"], 40);
            assert_eq!(picture["reconstruction"]["state"], "waiting", "{picture}");
            assert!(picture["reconstruction"]["starts"].as_str().unwrap().contains("VRChat closes"));
        }
        assert_eq!(client.state()["reconstructions"]["running"], Value::Null, "reconstructions wait for VRChat");
        assert_eq!(client.state()["pictures"].as_array().unwrap().len(), (number + 1) * 3);
    }
    // Closes the game, and the reconstructions run.
    let closed = client.tool("close_vrchat", json!({})).unwrap();
    assert_eq!(closed["closed"], true);
    assert_eq!(client.state()["vrchat"]["running"], false);
    let mut done = false;
    for _ in 0..4 {
        let answer = client.tool("wait_for", json!({ "until": "reconstructions_done", "timeout_s": 5 })).unwrap();
        if answer["done"] == true {
            done = true;
            break;
        }
    }
    assert!(done, "the queue ran");
    let state = client.state();
    assert!(state["pictures"].as_array().unwrap().iter().all(|p| p["reconstruction"]["state"] == "done"), "{state}");
    assert_eq!(state["pictures"][5]["world_name"], "Lab World B");
    assert_eq!(game.closes(), 2, "once going to the next world, once at the end");
    assert!(state["bridge"]["last_event"].is_null(), "closing it was the agent's own doing, no news: {state}");
}

#[test]
fn the_whole_loop_runs_unattended() {
    let (game, bridge, _server, client) = lab("tour");
    game.set_behaviour(|b| {
        *b = Behaviour { reconstruct: Duration::from_millis(100), ..Behaviour::default() };
    });
    tour(&client, &game);
    assert!(bridge.main.held.is_empty(), "nothing left down at the end");
    let mut status = Status::default();
    bridge.status_into(&mut status);
    assert!(status.log.iter().any(|l| l.text.contains("3D picture") && l.ok), "the log tells what it did");
    let state = client.state();
    let last = state["pictures"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(state["recording"]["frames"], last["frames"], "the saved recording's own numbers: {state}");
    assert_eq!(state["recording"]["keyframes"], last["keyframes"], "{state}");
    assert_eq!(status.client.as_ref().map(|c| c.name.as_str()), Some("e2e"));
    assert_eq!(status.client.map(|c| c.connected), Some(true));
}

#[test]
fn an_avatar_without_the_capture_kit_is_said_on_arrival_and_nothing_records() {
    let (game, _bridge, _server, client) = lab("no-kit-known");
    game.set_behaviour(|b| b.kit = false);
    let arrived = arrive(&client);
    assert_eq!(arrived["avatar"]["capture_kit"], false, "{arrived}");
    assert!(arrived["next"].as_str().unwrap().contains("capture kit"), "{arrived}");
    let state = client.state();
    assert_eq!(state["avatar"]["capture_kit"], false);
    assert!(state["next"].as_str().unwrap().contains("capture kit"), "{state}");
    game.set_foreground(true);
    client.tool("press", json!({ "input": "esc" })).unwrap();
    client.tool("mouse", json!({ "action": "click", "x": CAMERA_ICON.0, "y": CAMERA_ICON.1 })).unwrap();
    client.tool("turn", json!({ "degrees": 5 })).unwrap();
    let failure = client.fails("take_3d_picture", json!({ "length_s": 8 }), "no_capture_kit");
    assert!(failure["next"].as_str().unwrap().contains("Do not change the avatar"), "{failure}");
    assert_eq!(client.state()["recording"]["state"], "idle", "nothing recorded");
    assert_eq!(client.state()["pictures"].as_array().unwrap().len(), 0);
}

#[test]
fn an_avatar_is_worn_when_asked_and_says_so_when_vrchat_does_not_take_it() {
    const WITH_KIT: &str = "avtr_4f3b5bc4-87f0-4872-bc7e-22c5c071fdcf";
    let (game, bridge, _server, client) = lab("wear");
    game.set_behaviour(|b| b.kit = false);
    arrive(&client);

    // What is not an id goes nowhere, whatever it is.
    client.fails("wear_avatar", json!({ "avatar_id": "/avatar/parameters/x" }), "not_an_avatar");

    // VRChat stays silent on one this account may not wear: the answer says it did not change, in the time asked.
    let refused = client.tool("wear_avatar", json!({ "avatar_id": AVATAR_NOT_ALLOWED, "timeout_s": 1 })).unwrap();
    assert_eq!(refused["changed"], false, "{refused}");
    assert_eq!(refused["avatar"]["capture_kit"], false);
    assert!(refused["next"].as_str().unwrap().contains("tell the person"), "{refused}");

    // One it may: the kit follows, and with it a 3D picture is allowed again.
    let worn = client.tool("wear_avatar", json!({ "avatar_id": WITH_KIT })).unwrap();
    assert_eq!(worn["changed"], true, "{worn}");
    assert_eq!(worn["avatar"]["capture_kit"], true);
    assert!(worn["avatar"]["name"].as_str().unwrap().contains(WITH_KIT));
    assert_eq!(client.state()["avatar"]["capture_kit"], true);

    // The person at the computer wins here as everywhere.
    bridge.hold(true);
    client.fails("wear_avatar", json!({ "avatar_id": WITH_KIT }), "paused_held");
    bridge.hold(false);
}

#[test]
fn a_public_instance_changes_no_avatar() {
    let (game, _bridge, _server, client) = lab_giving_up("wear-public");
    game.set_behaviour(|b| b.land_in = Some("12345~region(eu)".to_string()));
    // The bridge leaves a public instance at once; whatever comes before or after that, no avatar changes there.
    client.fails("go_to_world", json!({ "world_id": LAB_WORLD_A }), "landed_elsewhere");
    let answer = client.tool("wear_avatar", json!({ "avatar_id": "avtr_4f3b5bc4-87f0-4872-bc7e-22c5c071fdcf" })).unwrap_err();
    assert!(answer["code"] == "not_private_instance" || answer["code"] == "vrchat_not_running", "{answer}");
    assert!(client.state()["avatar"]["name"] != "Lab Avatar avtr_4f3b5bc4-87f0-4872-bc7e-22c5c071fdcf", "unchanged");
}

#[test]
fn a_reconstruction_that_fails_says_so_to_the_agent() {
    let (game, _bridge, _server, client) = lab("recon-fails");
    game.set_behaviour(|b| {
        b.reconstruct = Duration::from_millis(50);
        b.reconstruct_fails = Some("no scene pixel to score".into());
    });
    arrive(&client);
    game.set_foreground(true);
    client.tool("press", json!({ "input": "esc" })).unwrap();
    client.tool("mouse", json!({ "action": "click", "x": CAMERA_ICON.0, "y": CAMERA_ICON.1 })).unwrap();
    client.tool("turn", json!({ "degrees": 5 })).unwrap();
    client.tool("take_3d_picture", json!({ "length_s": 1, "wait_s": 20 })).unwrap();
    client.tool("close_vrchat", json!({})).unwrap();
    let done = until(Duration::from_secs(10), || client.state()["pictures"][0]["reconstruction"]["state"] == "failed");
    let state = client.state();
    assert!(done, "{state}");
    assert_eq!(state["pictures"][0]["reconstruction"]["message"], "no scene pixel to score");
}

#[test]
fn the_bridges_own_input_is_never_taken_for_the_person() {
    let (game, bridge, _server, client) = lab("own-input");
    arrive(&client);
    game.set_foreground(true);
    // The thread that hears the person wakes the moment `SendInput` runs: by then it must know the input is ours.
    bridge.own_input.sent(0);
    let told = bridge.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let saw = seen.clone();
    *game.sending.lock().unwrap() = Some(Box::new(move || saw.lock().unwrap().push(told.own_input.last())));
    client.tool("press", json!({ "input": "esc" })).unwrap();
    client.tool("mouse", json!({ "action": "move", "x": 10, "y": 10 })).unwrap();
    client.tool("mouse", json!({ "action": "click", "x": 10, "y": 10 })).unwrap();
    client.tool("mouse", json!({ "action": "look", "dx": 1, "dy": 0 })).unwrap();
    let seen = seen.lock().unwrap();
    assert!(seen.len() >= 5, "{seen:?}");
    assert!(seen.iter().all(|at| *at != 0), "said before each: {seen:?}");
}

#[test]
fn a_click_rests_on_its_place_before_it_presses() {
    let (game, _bridge, _server, client) = lab("click-order");
    arrive(&client);
    game.set_foreground(true);
    client.tool("mouse", json!({ "action": "click", "x": 20, "y": 30 })).unwrap();
    let sent = game.injected();
    assert!(matches!(sent[0], Input::MoveTo { x: 20, y: 30 }), "{sent:?}");
    assert!(matches!(sent[1], Input::Button { down: true, .. }), "{sent:?}");
}

#[test]
fn arriving_waits_for_the_world_to_have_loaded() {
    let (game, _bridge, _server, client) = lab("loading");
    // The game says it is joining, and the player does not exist yet.
    game.set_behaviour(|b| b.player_after = Duration::from_millis(1500));
    let begin = Instant::now();
    let arrived = arrive(&client);
    assert_eq!(arrived["arrived"], true, "{arrived}");
    assert!(begin.elapsed() >= Duration::from_millis(1400), "too early: {:?}", begin.elapsed());
}

#[test]
fn a_picture_without_the_capture_kit_says_so() {
    let (game, _bridge, _server, client) = lab("no-kit");
    // VRChat's tree does not say, so it only shows in what was recorded.
    game.set_behaviour(|b| {
        b.kit = false;
        b.tree_says_kit = false;
    });
    arrive(&client);
    game.set_foreground(true);
    client.tool("press", json!({ "input": "esc" })).unwrap();
    client.tool("mouse", json!({ "action": "click", "x": CAMERA_ICON.0, "y": CAMERA_ICON.1 })).unwrap();
    client.tool("turn", json!({ "degrees": 5 })).unwrap();
    let failure = client.fails("take_3d_picture", json!({ "length_s": 1 }), "no_keyframes");
    assert!(failure["next"].as_str().unwrap().contains("capture kit"), "{failure}");
    assert_eq!(client.state()["reconstructions"]["waiting"].as_array().unwrap().len(), 0);
}

#[test]
fn an_answer_does_not_wait_for_the_look_at_its_socket() {
    // The watcher's look lasts up to the socket's read timeout (200 ms in the app): an answer must not wait for it. A minute
    // here, so that one that did would take a minute, where ten answers take milliseconds on an idle machine and under two
    // seconds on one running the whole test suite (once 1.78 s, which the 1.5 s this test allowed with the app's 200 ms
    // could not tell from waiting).
    let look = Duration::from_secs(60);
    let dir = std::env::temp_dir().join(format!("orbiters-bridge-e2e-latency-{}", uuid::Uuid::new_v4().simple()));
    let bridge = Bridge::with_timing(LabGame::new(&dir, None), None, Timing { instance_every: Duration::from_millis(50), start_limit: PATIENCE });
    let server = Server::start_looking_every(bridge, 0, TOKEN.to_string(), look).expect("a free port");
    let client = Client { port: server.port() };
    let begin = Instant::now();
    for _ in 0..10 {
        client.state();
    }
    assert!(begin.elapsed() < look, "an answer waited for the look at its socket: {:?}", begin.elapsed());
}

#[test]
fn one_action_at_a_time() {
    let (game, _bridge, _server, client) = lab("one");
    arrive(&client);
    let port = client.port;
    let walking = std::thread::spawn(move || Client { port }.tool("walk", json!({ "direction": "forward", "seconds": 1 })));
    assert!(until(Duration::from_secs(2), || game.last_value("/input/MoveForward") == Some(1.0)));
    client.fails("turn", json!({ "degrees": 10 }), "busy");
    assert!(client.tool("get_state", json!({})).is_ok(), "reading is always fine");
    walking.join().unwrap().unwrap();
}

#[test]
fn the_guards_lines_follow_what_is_held() {
    // What a press tells the guard is what the guard's reading makes of it.
    let mut reading = super::guard::Reading::default();
    reading.line("target 127.0.0.1:9000");
    let hold = Hold::OscButton("/input/MoveForward");
    reading.line(&format!("down {}", hold.guard_line()));
    assert_eq!(reading.down.len(), 1);
    reading.line(&format!("up {}", hold.guard_line()));
    assert!(reading.down.is_empty());
}

// ---- The Unity projects tools

/// Over the wire as any client calls them: listed after the VRChat tools, their guide a resource, the server's
/// instructions naming them, a refusal saying what to do next, the person's pause respected, and a token needed.
#[test]
fn the_unity_projects_tools_are_reached_like_the_others() {
    let (_game, bridge, _server, client) = lab("unity-projects");
    let (_, body) = client.rpc("tools/list", json!({}));
    let names: Vec<String> = body["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    let unity: Vec<&str> = names.iter().map(String::as_str).filter(|n| n.starts_with("unity_")).collect();
    assert_eq!(unity, super::projects::NAMES);
    assert_eq!(names.first().map(String::as_str), Some("get_state"), "the VRChat tools first");
    let (_, body) = client.rpc("resources/list", json!({}));
    let uris: Vec<&str> = body["result"]["resources"].as_array().unwrap().iter().map(|r| r["uri"].as_str().unwrap()).collect();
    assert_eq!(uris, ["bridge://guide", "bridge://unity-projects"]);
    let (_, body) = client.rpc("resources/read", json!({ "uri": "bridge://unity-projects" }));
    assert!(body["result"]["contents"][0]["text"].as_str().unwrap().contains("unity_change_packages"));
    let (_, body) = client.rpc("server/discover", json!({}));
    assert!(body["result"]["instructions"].as_str().unwrap().contains("bridge://unity-projects"));

    // Jobs are listed (none yet); a made-up project is refused with what to do.
    assert_eq!(client.tool("unity_get_job", json!({})).unwrap()["jobs"], json!([]));
    client.fails("unity_get_project", json!({ "path": "Z:/no/such/project" }), "not_a_project");
    client.fails("unity_change_packages", json!({ "path": "relative" , "action": "restore" }), "bad_arguments");
    client.fails("unity_cancel_job", json!({ "job": 7 }), "no_such_job");
    // Nothing is taken out of the list without the exact path of a listed project.
    let dir = std::env::temp_dir().join(format!("orbiters-bridge-e2e-unity-{}", uuid::Uuid::new_v4().simple()));
    client.fails("unity_manage_project", json!({ "path": dir.join("Not listed"), "action": "remove_from_list", "confirm": true }), "not_listed");

    // The person's pause: changes wait for them, reading does not.
    bridge.hold(true);
    client.fails("unity_change_packages", json!({ "path": "Z:/no/such/project", "action": "restore" }), "paused_held");
    assert!(client.tool("unity_get_job", json!({})).is_ok());
    bridge.hold(false);
    // What was asked shows in Settings' log, as the VRChat tools' actions do.
    assert!(bridge.log.lock().unwrap().iter().any(|l| !l.ok && l.text.starts_with("manage project remove_from_list")), "{:?}", bridge.log.lock().unwrap());

    // Without the token, nothing.
    let (status, _, _) = exchange(client.port, "POST", "/mcp", &[("Host", format!("127.0.0.1:{}", client.port)), ("Content-Type", "application/json".into())], &modern_message("tools/list", json!({}))).unwrap();
    assert_eq!(status, 401);
}

// ---- Sessions: several VRChats at once

use super::lab::LabOpener;

/// The bridge with the lab's main game and side sessions, and a client.
fn lab_with_sessions(name: &str) -> (Arc<LabGame>, Arc<LabOpener>, Arc<Bridge>, Server, Client) {
    let (game, bridge, server, client) = lab(name);
    let opener = LabOpener::new(&std::env::temp_dir().join(format!("orbiters-bridge-sessions-{name}-{}", uuid::Uuid::new_v4().simple())));
    bridge.set_opener(opener.clone());
    (game, opener, bridge, server, client)
}

/// Opens a session of `profile` and takes it to world A: its id.
fn open_and_arrive(client: &Client, profile: u32, world: &str) -> String {
    let opened = client.tool("open_session", json!({ "profile": profile })).expect("open_session");
    let id = opened["session"].as_str().unwrap().to_string();
    assert!(id.starts_with(&format!("p{profile}-")), "{opened}");
    assert_eq!(opened["needs_sign_in"], false, "{opened}");
    assert_eq!(opened["signed_in_as"], format!("Lab Profile {profile}"));
    let arrived = client.tool("go_to_world", json!({ "session": id, "world_id": world })).expect("go_to_world");
    assert_eq!(arrived["arrived"], true, "{arrived}");
    id
}

#[test]
fn two_sessions_are_driven_at_once_without_cross_talk() {
    let (main, opener, _bridge, _server, client) = lab_with_sessions("two");
    let one = open_and_arrive(&client, 1, LAB_WORLD_A);
    let two = open_and_arrive(&client, 2, LAB_WORLD_B);
    let (g1, g2) = (opener.game_of(1).unwrap(), opener.game_of(2).unwrap());
    // Each started with its own profile and OSC ports, the main session's game never.
    assert!(main.launches().is_empty(), "the main session was not touched");
    let args1 = g1.launches().last().unwrap().args();
    let args2 = g2.launches().last().unwrap().args();
    assert!(args1.contains(&"--profile=1".into()) && args1.contains(&"--osc=9010:127.0.0.1:9011".into()), "{args1:?}");
    assert!(args2.contains(&"--profile=2".into()) && args2.contains(&"--osc=9020:127.0.0.1:9021".into()), "{args2:?}");
    // Both walk at the same time, each in its own VRChat only.
    let port = client.port;
    let (a, b) = (one.clone(), two.clone());
    let walking1 = std::thread::spawn(move || Client { port }.tool("walk", json!({ "session": a, "direction": "forward", "seconds": 0.6 })));
    let walking2 = std::thread::spawn(move || Client { port }.tool("walk", json!({ "session": b, "direction": "left", "seconds": 0.6 })));
    walking1.join().unwrap().expect("session 1 walks");
    walking2.join().unwrap().expect("session 2 walks beside it");
    assert_eq!(g1.last_value("/input/MoveForward"), Some(0.0));
    assert_eq!(g1.last_value("/input/MoveLeft"), None, "nothing of session 2's reached session 1");
    assert_eq!(g2.last_value("/input/MoveLeft"), Some(0.0));
    assert_eq!(g2.last_value("/input/MoveForward"), None, "nothing of session 1's reached session 2");
    assert!(main.heard().is_empty(), "nor the main session's");
    // Each looks at its own window.
    for id in [&one, &two] {
        let (_, picture) = client.rpc("tools/call", json!({ "name": "screenshot", "arguments": { "session": id } }));
        assert_eq!(picture["result"]["content"][0]["type"], "image", "{picture}");
    }
    // Each state is its own: its world, its account, its pictures.
    let (s1, s2) = (client.tool("get_state", json!({ "session": one })).unwrap(), client.tool("get_state", json!({ "session": two })).unwrap());
    assert_eq!((s1["place"]["world_id"].as_str(), s2["place"]["world_id"].as_str()), (Some(LAB_WORLD_A), Some(LAB_WORLD_B)));
    assert_eq!(s1["session"]["account"]["name"], "Lab Profile 1");
    // Each in a strict private instance of its own account, not of the app's (VRChat refuses another account there).
    assert!(s1["place"]["instance"].as_str().unwrap().contains("~private(usr_00000001-"), "{s1}");
    assert!(s2["place"]["instance"].as_str().unwrap().contains("~private(usr_00000002-"), "{s2}");
    assert_eq!((s1["place"]["private"].as_bool(), s2["place"]["private"].as_bool()), (Some(true), Some(true)));
    assert!(main.instances().is_empty() && opener.game_of(1).unwrap().instances().is_empty(), "no API instance for a side session");
    assert_eq!(s2["session"]["profile"], 2);
    assert_eq!(client.state()["vrchat"]["running"], false, "the main session still has no VRChat");
    // A picture in each, the camera opened through each session's own window (one keyboard turn after the other).
    for (id, game) in [(&one, &g1), (&two, &g2)] {
        game.set_foreground(true);
        let mut opened = false;
        for _ in 0..20 {
            match client.tool("press", json!({ "session": id, "input": "esc" })) {
                Ok(_) => {
                    opened = true;
                    break;
                }
                Err(f) if f["code"] == "focus_busy" => std::thread::sleep(Duration::from_millis(500)),
                Err(f) => panic!("{f}"),
            }
        }
        assert!(opened, "the keyboard turn came");
        client.tool("mouse", json!({ "session": id, "action": "click", "x": CAMERA_ICON.0, "y": CAMERA_ICON.1 })).unwrap();
        client.tool("press", json!({ "session": id, "input": "esc" })).unwrap();
        client.tool("turn", json!({ "session": id, "degrees": 20 })).unwrap();
        let picture = client.tool("take_3d_picture", json!({ "session": id, "length_s": 1 })).unwrap();
        assert_eq!(picture["saved"], true, "{picture}");
        game.set_foreground(false);
        // Another session's turn comes once this one leaves the keyboard alone.
        std::thread::sleep(super::FOCUS_HOLD);
    }
    let (s1, s2) = (client.tool("get_state", json!({ "session": one })).unwrap(), client.tool("get_state", json!({ "session": two })).unwrap());
    let (p1, p2) = (s1["pictures"].as_array().unwrap(), s2["pictures"].as_array().unwrap());
    assert_eq!((p1.len(), p2.len()), (1, 1), "each picture in its own session");
    assert_eq!((p1[0]["world_id"].as_str(), p2[0]["world_id"].as_str()), (Some(LAB_WORLD_A), Some(LAB_WORLD_B)));
    assert!(g1.injected().iter().any(|i| matches!(i, Input::Key { scan: 0x01, .. })) && g2.injected().iter().any(|i| matches!(i, Input::Key { scan: 0x01, .. })));
    // Listed, ids not shown.
    let list = client.tool("list_sessions", json!({})).unwrap();
    let listed = list["sessions"].as_array().unwrap();
    assert_eq!(listed.len(), 3, "{list}");
    assert!(listed.iter().all(|s| !s["session"].as_str().unwrap().contains(&one[3..])), "{list}");
    // Closed, and gone.
    client.tool("close_session", json!({ "session": one })).unwrap();
    client.fails("walk", json!({ "session": one, "direction": "forward", "seconds": 0.1 }), "unknown_session");
    assert!(opener.forgotten.lock().unwrap().contains(&one));
    assert_eq!(g1.closes(), 2, "once to go to the world from the account's home, once at the end");
    assert_eq!(client.tool("get_state", json!({ "session": two })).unwrap()["vrchat"]["running"], true, "the other session goes on");
}

#[test]
fn a_session_is_driven_with_its_id_only_and_taken_over_once_left_alone() {
    let (_main, _opener, bridge, _server, client) = lab_with_sessions("lease");
    let id = open_and_arrive(&client, 1, LAB_WORLD_A);
    client.fails("get_state", json!({ "session": "p1-000000000000" }), "session_taken_over");
    client.fails("get_state", json!({ "session": "p5-000000000000" }), "unknown_session");
    // The profile has its session: another agent is told, and may not take it while it is in use.
    let busy = client.fails("open_session", json!({ "profile": 1, "take_over": true }), "session_in_use");
    assert!(busy["message"].as_str().unwrap().contains("e2e"), "who drives it: {busy}");
    // Left alone for longer than the lease: taken over, the same VRChat, a new id; the old one is refused.
    let side = bridge.sides.lock().unwrap()[0].clone();
    side.lease.lock().unwrap().as_mut().unwrap().last_call = Instant::now() - super::sessions::LEASE_STALE - Duration::from_secs(1);
    client.fails("open_session", json!({ "profile": 1 }), "session_in_use");
    let taken = client.tool("open_session", json!({ "profile": 1, "take_over": true })).unwrap();
    assert_eq!(taken["taken_over"], true);
    let new = taken["session"].as_str().unwrap();
    assert_ne!(new, id);
    client.fails("walk", json!({ "session": id, "direction": "forward", "seconds": 0.1 }), "session_taken_over");
    assert_eq!(client.tool("get_state", json!({ "session": new })).unwrap()["vrchat"]["started_by_bridge"], true, "the VRChat came with it");
}

#[test]
fn a_profile_without_an_account_says_it_needs_the_persons_sign_in() {
    let (_main, opener, _bridge, _server, client) = lab_with_sessions("sign-in");
    opener.signed_out.lock().unwrap().push(3);
    let opened = client.tool("open_session", json!({ "profile": 3 })).unwrap();
    assert_eq!(opened["needs_sign_in"], true, "{opened}");
    assert!(opened["next"].as_str().unwrap().contains("sign in"), "{opened}");
    let id = opened["session"].as_str().unwrap();
    let state = client.tool("get_state", json!({ "session": id })).unwrap();
    assert_eq!(state["session"]["needs_sign_in"], true);
    assert!(state["next"].as_str().unwrap().contains("never types"), "{state}");
    client.fails("go_to_world", json!({ "session": id, "world_id": LAB_WORLD_A }), "needs_sign_in");
    assert_eq!(opener.game_of(3).unwrap().launches().len(), 1, "nothing restarted for it");
    let waited = client.tool("wait_for", json!({ "session": id, "until": "signed_in", "timeout_s": 1 })).unwrap();
    assert_eq!(waited["done"], false);
    opener.game_of(3).unwrap().set_behaviour(|b| {
        b.needs_sign_in = false;
        b.account = Some(("usr_x".into(), "Signed In".into()));
    });
    assert_eq!(client.tool("wait_for", json!({ "session": id, "until": "signed_in", "timeout_s": 2 })).unwrap()["done"], true);
}

#[test]
fn a_session_beyond_what_is_allowed_is_refused_with_its_reason() {
    let (_main, opener, _bridge, _server, client) = lab_with_sessions("limits");
    client.fails("open_session", json!({ "profile": 0 }), "bad_profile");
    opener.others.lock().unwrap().push(4);
    client.fails("open_session", json!({ "profile": 4 }), "profile_in_use");
    *opener.memory.lock().unwrap() = Some(1 << 30);
    client.fails("open_session", json!({ "profile": 1 }), "low_memory");
    *opener.memory.lock().unwrap() = Some(16 << 30);
    // Easy Anti-Cheat of a VRChat that just closed: no launch until it unloaded.
    *opener.eac.lock().unwrap() = super::sessions::Eac { service: "STOPPED".into(), driver: "RUNNING".into(), vrchat_runs: false, launcher_runs: false, delete_pending: false };
    let busy = client.fails("open_session", json!({ "profile": 1 }), "eac_busy");
    assert!(busy["retry_after_s"].as_u64().is_some());
    assert!(opener.games.lock().unwrap().iter().all(|g| g.2.launches().is_empty()), "nothing was launched");
    *opener.eac.lock().unwrap() = super::sessions::Eac { service: "ABSENT".into(), driver: "ABSENT".into(), vrchat_runs: false, launcher_runs: false, delete_pending: false };
    *opener.limit.lock().unwrap() = 1;
    open_and_arrive(&client, 1, LAB_WORLD_A);
    let full = client.fails("open_session", json!({ "profile": 2 }), "too_many_sessions");
    assert!(full["message"].as_str().unwrap().contains("graphics memory"), "{full}");
}

#[test]
fn the_persons_input_in_a_sessions_window_pauses_that_session_only() {
    let (_main, opener, bridge, _server, client) = lab_with_sessions("person");
    let (one, two) = (open_and_arrive(&client, 1, LAB_WORLD_A), open_and_arrive(&client, 2, LAB_WORLD_B));
    let (g1, g2) = (opener.game_of(1).unwrap(), opener.game_of(2).unwrap());
    g1.set_foreground(true);
    bridge.heard(Heard::PersonInput);
    client.fails("walk", json!({ "session": one, "direction": "forward", "seconds": 0.2 }), "paused_by_person");
    client.tool("walk", json!({ "session": two, "direction": "forward", "seconds": 0.2 })).expect("the other session walks on");
    client.tool("turn", json!({ "session": two, "degrees": 10 })).expect("and turns");
    // Keys need the person away from the whole computer.
    g2.set_foreground(true);
    g1.set_foreground(false);
    client.fails("press", json!({ "session": two, "input": "esc" }), "paused_by_person");
    assert!(g2.injected().is_empty(), "no key reached the window while the person is at the computer");
}

#[test]
fn the_persons_own_vrchat_never_loses_the_foreground_to_a_session() {
    let (_main, opener, _bridge, _server, client) = lab_with_sessions("person-focus");
    let one = open_and_arrive(&client, 1, LAB_WORLD_A);
    // The person's VRChat runs and has the foreground.
    opener.persons.lock().unwrap().push(4242);
    *opener.foreground.lock().unwrap() = Some(4242);
    let game = opener.game_of(1).unwrap();
    game.set_behaviour(|b| b.focus_works = true);
    client.fails("press", json!({ "session": one, "input": "esc" }), "person_vrchat_has_focus");
    client.fails("focus_game", json!({ "session": one }), "person_vrchat_has_focus");
    assert!(game.injected().is_empty());
    client.tool("walk", json!({ "session": one, "direction": "forward", "seconds": 0.2 })).expect("OSC needs no focus");
    // Another window of theirs (not a VRChat) has it: the session's window is brought forward on its turn.
    *opener.foreground.lock().unwrap() = Some(1);
    client.tool("press", json!({ "session": one, "input": "esc" })).expect("brought forward and pressed");
    let list = client.tool("list_sessions", json!({})).unwrap();
    assert_eq!(list["persons_vrchats_running"], 1, "{list}");
}

#[test]
fn keys_and_clicks_take_turns_between_sessions() {
    let mut turn = super::FocusTurn::default();
    let t0 = Instant::now();
    assert!(turn.ask("a", t0).is_ok());
    assert!(turn.ask("a", t0 + Duration::from_secs(5)).is_ok(), "kept while used");
    let wait = turn.ask("b", t0 + Duration::from_secs(6)).unwrap_err();
    assert!((1..=8).contains(&wait), "{wait}");
    let _ = turn.ask("c", t0 + Duration::from_secs(7));
    // a stops using it: b asked first, b is next, c after.
    let later = t0 + Duration::from_secs(5) + super::FOCUS_HOLD + Duration::from_millis(1);
    assert!(turn.ask("c", later).is_err(), "c asked after b");
    assert!(turn.ask("b", later).is_ok());
    assert_eq!(turn.holder(), Some("b"));
    turn.leave("b");
    assert!(turn.ask("c", later).is_ok(), "b closed: c's turn at once");
}

#[test]
fn the_main_session_waits_while_a_side_sessions_picture_borrows_the_app() {
    let (main, _opener, _bridge, _server, client) = lab_with_sessions("lent");
    arrive(&client);
    main.set_behaviour(|b| b.lent_away = true);
    let failure = client.fails("walk", json!({ "direction": "forward", "seconds": 0.2 }), "app_lent");
    assert!(failure["retry_after_s"].as_u64().is_some());
    assert!(main.last_value("/input/MoveForward").is_none(), "nothing of main's went out while lent");
    assert_eq!(client.state()["app_lent_to_another_session"], true);
    main.set_behaviour(|b| b.lent_away = false);
    assert_eq!(client.state()["vrchat"]["started_by_bridge"], true, "still the bridge's after the lend: not taken for gone");
    client.tool("walk", json!({ "direction": "forward", "seconds": 0.2 })).unwrap();
}

// ---- World labels (VR1115 labels): N4N0's world label feedback, from the bridge

/// The label tools over the real transport: only N4N0's own labels, only for a world this session saw, the reason ai-world
/// needs, under the signed-in Orbiters account, refused when there is none, and what Orbiters answered.
#[test]
fn labels_are_given_from_the_offered_list_for_a_world_seen_here() {
    let (_game, bridge, _server, client) = lab("labels");
    let feedback = super::lab::LabFeedback::signed_in();
    bridge.set_feedback(feedback.clone());
    let (_, listed) = client.rpc("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    let ids: Vec<&str> = orbiters_contracts::world_feedback::labels().iter().map(|l| l.id.as_str()).collect();
    for name in ["world_labels", "label_world"] {
        let tool = tools.iter().find(|t| t["name"] == name).expect(name);
        let description = tool["description"].as_str().unwrap();
        assert!(description.contains("published") && description.contains("actually saw"), "{name} says what labels are: {description}");
        assert!(tool["inputSchema"]["properties"]["session"].is_object(), "{name} takes the session");
    }
    let label = tools.iter().find(|t| t["name"] == "label_world").unwrap();
    assert_eq!(label["inputSchema"]["properties"]["add"]["items"]["enum"], json!(ids), "the vocabulary, and nothing else");

    client.fails("label_world", json!({ "add": ["mirror"] }), "not_in_world");
    client.fails("label_world", json!({ "world_id": LAB_WORLD_B, "add": ["mirror"] }), "not_seen_here");
    arrive(&client);
    let read = client.tool("world_labels", json!({})).unwrap();
    assert_eq!(read["world_id"], LAB_WORLD_A);
    assert_eq!(read["offered"].as_array().unwrap().len(), ids.len());
    let ai = read["offered"].as_array().unwrap().iter().find(|l| l["id"] == "ai-world").unwrap();
    assert_eq!(ai["needs_reason"]["min_words"], 10);
    assert_eq!((read["account"]["environment"].as_str(), read["account"]["name"].as_str()), (Some("development"), Some("Lab member")));
    assert_eq!(read["account"]["server"], "http://127.0.0.1:4100/");
    assert_eq!(read["world"]["read"], true);
    assert_eq!(feedback.state.lock().unwrap().shown, [LAB_WORLD_A], "read as opening it on the worlds page does");

    // Nothing outside the list, nothing half sent.
    let unknown = client.fails("label_world", json!({ "add": ["mirror", "beautiful"] }), "unknown_label");
    assert!(unknown["message"].as_str().unwrap().contains("video-player"), "the list is in the refusal: {unknown}");
    client.fails("label_world", json!({ "add": ["ai-world"] }), "reason_needed");
    client.fails("label_world", json!({ "add": ["ai-world"], "reason": "looks generated" }), "reason_needed"); // i18n: not shown (a test)
    client.fails("label_world", json!({ "add": ["mirror"], "reason": "a mirror in the hall" }), "bad_arguments"); // i18n: not shown (a test)
    client.fails("label_world", json!({ "add": ["mirror"], "remove": ["mirror"] }), "bad_arguments");
    client.fails("label_world", json!({}), "bad_arguments");
    assert!(feedback.state.lock().unwrap().asks.is_empty(), "a refused call sends nothing");

    let given = client.tool("label_world", json!({ "add": ["mirror", "pens"] })).unwrap();
    assert_eq!((given["sent"].as_bool(), given["not_as_asked"].as_array().map(Vec::len)), (Some(true), Some(0)), "{given}");
    let mine: Vec<&str> = given["world"]["labels"].as_array().unwrap().iter().filter(|l| l["mine"] == true).map(|l| l["id"].as_str().unwrap()).collect();
    assert_eq!(mine, ["mirror", "pens"]);
    let reason = "The trees, the sky and the water all look like one generated painting."; // i18n: not shown (a test)
    let changed = client.tool("label_world", json!({ "add": ["ai-world"], "remove": ["pens"], "reason": reason })).unwrap();
    assert_eq!(changed["sent"], true, "{changed}");
    {
        let st = feedback.state.lock().unwrap();
        assert_eq!(
            st.asks[2..],
            [
                orbiters_n4n0_core::feedback::FeedbackAsk::Label { world: LAB_WORLD_A.into(), label: "ai-world".into(), on: true, text: Some(reason.into()) },
                orbiters_n4n0_core::feedback::FeedbackAsk::Label { world: LAB_WORLD_A.into(), label: "pens".into(), on: false, text: None },
            ],
            "the asks of presses on the worlds page"
        );
    }

    // A world arrived in earlier in this run may still be labelled from the next one.
    let moved = client.tool("go_to_world", json!({ "world_id": LAB_WORLD_B })).unwrap();
    assert_eq!(moved["arrived"], true, "{moved}");
    assert_eq!(client.state()["place"]["world_id"], LAB_WORLD_B);
    client.tool("label_world", json!({ "world_id": LAB_WORLD_A, "add": ["dark"] })).unwrap();
    client.fails("label_world", json!({ "world_id": "wrld_x", "add": ["dark"] }), "bad_world_id");

    // Refused by Orbiters, queued while it cannot be reached, nothing without an account.
    feedback.state.lock().unwrap().refuses = Some("Slow down a little and try again.".into()); // i18n: not shown (a test)
    let refused = client.fails("label_world", json!({ "world_id": LAB_WORLD_A, "add": ["chill"] }), "labels_not_taken");
    assert!(refused["message"].as_str().unwrap().contains("Slow down"), "{refused}");
    {
        let mut st = feedback.state.lock().unwrap();
        st.refuses = None;
        st.offline = true;
    }
    let queued = client.tool("label_world", json!({ "world_id": LAB_WORLD_A, "add": ["chill"], "wait_s": 0.3 })).unwrap();
    assert_eq!((queued["sent"].as_bool(), queued["world"]["pending"].as_u64()), (Some(false), Some(1)), "{queued}");
    feedback.state.lock().unwrap().account = None;
    let asks = feedback.state.lock().unwrap().asks.len();
    client.fails("label_world", json!({ "add": ["city"] }), "orbiters_not_signed_in");
    assert_eq!(feedback.state.lock().unwrap().asks.len(), asks, "nothing sent without an account");
    assert_eq!(client.tool("world_labels", json!({})).unwrap()["account"]["signed_in"], false);
}

/// `find_worlds` reaches what VRChat highlights (its two Spotlight rows) and pages every list to its end, saying when it is.
#[test]
fn find_worlds_pages_every_list_and_reaches_vrchats_spotlight() {
    let (_game, _bridge, _server, client) = lab("find-worlds");
    for source in ["spotlight_pc", "spotlight_cross_platform"] {
        let first = client.tool("find_worlds", json!({ "source": source })).unwrap();
        assert_eq!((first["worlds"][0]["id"].as_str(), first["more"].as_bool(), first["next_page"].as_u64()), (Some(LAB_WORLD_A), Some(true), Some(1)), "{first}");
        let second = client.tool("find_worlds", json!({ "source": source, "page": 1 })).unwrap();
        assert_eq!((second["worlds"][0]["id"].as_str(), second["more"].as_bool()), (Some(LAB_WORLD_B), Some(false)), "{second}");
        assert!(second["next_page"].is_null());
    }
    // VRChat's lists are read 21 pages deep: the last says it is, and past it is refused rather than the last again.
    assert_eq!(client.tool("find_worlds", json!({ "source": "popular", "page": 19 })).unwrap()["more"], true);
    let last = client.tool("find_worlds", json!({ "source": "popular", "page": 20 })).unwrap();
    assert_eq!((last["more"].as_bool(), last["next_page"].is_null()), (Some(false), true), "{last}");
    client.fails("find_worlds", json!({ "source": "popular", "page": 21 }), "bad_arguments");
    client.tool("find_worlds", json!({ "source": "visited", "page": 40 })).expect("the visited worlds are this computer's own, as deep as they go");
    client.fails("find_worlds", json!({ "source": "featured" }), "bad_arguments");
}

// ---- The world tour's kit (VR1115 L-tour): the environment, publishing, the pictures' files, labels after a restart

/// Opens the lab's camera and makes its pose known, as an agent does through the menu.
fn open_camera(game: &LabGame, client: &Client) {
    game.set_foreground(true);
    client.tool("press", json!({ "input": "esc" })).unwrap();
    client.tool("mouse", json!({ "action": "click", "x": CAMERA_ICON.0, "y": CAMERA_ICON.1 })).unwrap();
    client.tool("press", json!({ "input": "esc" })).unwrap();
    client.tool("turn", json!({ "degrees": 10 })).unwrap();
}

/// A tour switches the app to the environment it was told (never silently: said, logged, refused while labels wait), takes
/// three 8 s pictures in one world without opening the camera again, publishes them there (two private, the best public, with
/// the item's id and link), frees their files once that is safe, and after a restart still knows the worlds it was in and the
/// pictures it took.
#[test]
fn a_tour_publishes_its_pictures_where_it_was_told_and_frees_them_once_safe() {
    let (game, bridge, _server, client) = lab("tourkit");
    let publish = super::lab::LabPublish::new(game.dir());
    bridge.set_publish(publish.clone());
    let journal = game.dir().join("journal.json");
    bridge.keep_journal(&journal);
    bridge.set_feedback(super::lab::LabFeedback::signed_in());

    // Where things go is always said.
    let state = client.state();
    assert_eq!((state["orbiters"]["environment"].as_str(), state["orbiters"]["signed_in"].as_bool()), (Some("development"), Some(true)), "{state}");
    assert_eq!(state["disks"][0]["free_gb"], 20.0);
    let read = client.tool("orbiters_environment", json!({ "action": "read" })).unwrap();
    assert!(read["switching_changes"].as_str().unwrap().contains("update feed"), "{read}");
    // Not while labels wait: they would go to the other one.
    publish.state.lock().unwrap().labels_pending = 2;
    client.fails("orbiters_environment", json!({ "action": "switch", "to": "production" }), "labels_waiting");
    assert!(publish.state.lock().unwrap().switches.is_empty());
    publish.state.lock().unwrap().labels_pending = 0;
    let switched = client.tool("orbiters_environment", json!({ "action": "switch", "to": "production" })).unwrap();
    assert_eq!((switched["switched"].as_bool(), switched["before"]["environment"].as_str(), switched["orbiters"]["environment"].as_str()), (Some(true), Some("development"), Some("production")), "{switched}");
    assert!(switched["what_changed"].as_str().unwrap().contains("account"));
    assert_eq!(publish.state.lock().unwrap().switches, [false]);
    let again = client.tool("orbiters_environment", json!({ "action": "switch", "to": "production" })).unwrap();
    assert_eq!(again["switched"], false, "{again}");
    let mut logged = crate::bridge::Status::default();
    bridge.status_into(&mut logged);
    assert!(logged.log.iter().any(|l| l.text.contains("switched to production")), "the switch is in the bridge's log");

    // Three 8 s pictures in a row, the camera opened once.
    arrive(&client);
    open_camera(&game, &client);
    let mut names = Vec::new();
    for _ in 0..3 {
        let picture = client.tool("take_3d_picture", json!({ "length_s": 8 })).unwrap();
        assert_eq!(picture["saved"], true, "{picture}");
        names.push(picture["session"].as_str().unwrap().to_string());
    }
    assert_eq!(client.state()["camera"]["open"], true, "still open for the next one");
    client.fails("publish_3d_picture", json!({ "picture": names[0] }), "not_reconstructed");
    client.fails("publish_3d_picture", json!({ "picture": "..\\elsewhere" }), "unknown_picture");
    client.fails("free_picture_files", json!({ "picture": names[0], "what": "capture_files" }), "reconstruction_waiting");
    client.tool("close_vrchat", json!({})).unwrap();
    assert!(until(Duration::from_secs(10), || client.tool("wait_for", json!({ "until": "reconstructions_done", "timeout_s": 2 })).unwrap()["done"] == true));

    // Not deleted whole before it is safe on Orbiters; its recording may go once its 3D view is made.
    client.fails("free_picture_files", json!({ "picture": names[0], "what": "everything" }), "not_published");
    let freed = client.tool("free_picture_files", json!({ "picture": names[0], "what": "capture_files" })).unwrap();
    assert!(freed["freed_mb"].as_f64().unwrap() > 0.2, "{freed}");
    assert!(!game.dir().join(&names[0]).join("keyframes").exists() && game.dir().join(&names[0]).exists(), "its 3D view stays");

    // Orbiters asks to wait (429, 2026-11-15: twelve uploads an hour): the answer says so with the seconds, the picture stays
    // queued, asking again sends nothing more, and its files stay until it is published.
    publish.state.lock().unwrap().rate_limited = Some(1200);
    let limited = client.tool("publish_3d_picture", json!({ "picture": names[0], "visibility": "private" })).unwrap();
    assert_eq!((limited["code"].as_str(), limited["retry_after_s"].as_u64(), limited["publish"]["state"].as_str()), (Some("rate_limited"), Some(1200), Some("rate_limited")), "{limited}");
    assert!(limited["next"].as_str().unwrap().contains("queued"), "{limited}");
    client.tool("publish_3d_picture", json!({ "picture": names[0], "visibility": "private" })).unwrap();
    assert_eq!(publish.state.lock().unwrap().asked.len(), 1, "queued once, not asked again");
    client.fails("free_picture_files", json!({ "picture": names[0], "what": "everything" }), "publish_under_way");
    client.fails("orbiters_environment", json!({ "action": "switch", "to": "development" }), "publish_under_way");
    publish.let_through();

    // Published to production: two private, the best public last.
    for (i, name) in names.iter().enumerate() {
        let visibility = if i == 2 { "public" } else { "private" };
        let published = client.tool("publish_3d_picture", json!({ "picture": name, "visibility": visibility })).unwrap();
        assert_eq!((published["publish"]["state"].as_str(), published["orbiters"]["environment"].as_str()), (Some("done"), Some("production")), "{published}");
        assert_eq!(published["publish"]["public"], i == 2);
        assert_eq!(published["world_id"], LAB_WORLD_A);
    }
    let last = client.tool("pictures", json!({ "picture": names[2] })).unwrap();
    assert!(last["publish"]["link"].as_str().unwrap().starts_with(&format!("https://orbiters.cc/maps/{LAB_WORLD_A}/captures/")), "{last}");
    assert_eq!(publish.state.lock().unwrap().asked.len(), 3, "each once");
    // Published again: nothing sent twice.
    client.tool("publish_3d_picture", json!({ "picture": names[2] })).unwrap();
    assert_eq!(publish.state.lock().unwrap().asked.len(), 3);
    let listed = client.tool("pictures", json!({})).unwrap();
    assert_eq!(listed["pictures"].as_array().unwrap().len(), 3);
    assert_eq!(client.tool("pictures", json!({ "unpublished": true })).unwrap()["pictures"].as_array().unwrap().len(), 0);
    assert_eq!(client.state()["pictures"][0]["publish"]["state"], "done");
    let gone = client.tool("free_picture_files", json!({ "picture": names[1], "what": "everything" })).unwrap();
    assert!(gone["freed_mb"].as_f64().unwrap() > 0.2 && !game.dir().join(&names[1]).exists(), "{gone}");
    assert_eq!(client.tool("pictures", json!({ "picture": names[1] })).unwrap()["reconstruction"]["state"], "deleted");

    // Not signed in there: nothing sent. Low on disk: get_state says so first.
    publish.state.lock().unwrap().signed_in.0 = false;
    client.fails("publish_3d_picture", json!({ "picture": names[0] }), "orbiters_not_signed_in");
    publish.state.lock().unwrap().free_gb = 3.0;
    let low = client.state();
    assert!(low["next"].as_str().unwrap().contains("free_picture_files"), "{low}");

    // A restart: the journal still knows the world it was in and the pictures it took.
    let (_game2, bridge2, _server2, client2) = lab("tourkit-restart");
    bridge2.keep_journal(&journal);
    bridge2.set_publish(publish.clone());
    bridge2.set_feedback(super::lab::LabFeedback::signed_in());
    client2.tool("label_world", json!({ "world_id": LAB_WORLD_A, "add": ["bright"] })).expect("a world arrived in before the restart");
    client2.fails("label_world", json!({ "world_id": LAB_WORLD_B, "add": ["bright"] }), "not_seen_here");
    assert_eq!(client2.tool("pictures", json!({})).unwrap()["taken_in_all"], 3);
}

/// Vantage without walking: the open camera is put higher and further, from where it is and the way it looks, and a picture
/// starts there; VRChat's other rows and the person's playlists are sources.
#[test]
fn the_camera_is_placed_for_a_view_and_more_rows_are_sources() {
    let (game, _bridge, _server, client) = lab("aim");
    arrive(&client);
    client.fails("aim_camera", json!({ "up_m": 3 }), "camera_closed");
    open_camera(&game, &client);
    // The lab's camera opens at (1, 1.5, 2) looking along +X (yaw 90).
    let aimed = client.tool("aim_camera", json!({ "forward_m": 4, "right_m": 1, "up_m": 3, "turn_deg": -30, "pitch_deg": 20 })).unwrap();
    let to: Vec<f64> = aimed["to"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    assert_eq!(to, [5.0, 4.5, 1.0, 20.0, 60.0, 0.0], "{aimed}");
    assert_eq!(aimed["moved_m"], 5.1);
    client.fails("aim_camera", json!({ "up_m": 40 }), "bad_arguments");
    let picture = client.tool("take_3d_picture", json!({ "length_s": 8 })).unwrap();
    assert_eq!(picture["saved"], true);

    let rows = client.tool("find_worlds", json!({ "source": "games" })).unwrap();
    assert!(!rows["worlds"].as_array().unwrap().is_empty(), "{rows}");
    let playlists = client.tool("find_worlds", json!({ "source": "playlists" })).unwrap();
    assert_eq!(playlists["playlists"][0]["group"], "worlds1", "{playlists}");
    client.tool("find_worlds", json!({ "source": "playlists", "group": "worlds1" })).unwrap();
    client.fails("find_worlds", json!({ "source": "playlists", "group": "worlds9" }), "unknown_playlist");
}

#[test]
fn a_tour_travels_in_its_running_vrchat_and_starts_it_once() {
    // 2026-11-15: every go_to_world started the side session's VRChat again (~40 Easy Anti-Cheat driver installs in two
    // hours), and the run ended with that driver service stuck until Windows restarts. Now the running game travels.
    let (_main, opener, _bridge, _server, client) = lab_with_sessions("travel");
    *opener.links_open_page.lock().unwrap() = true;
    let opened = client.tool("open_session", json!({ "profile": 1 })).unwrap();
    let id = opened["session"].as_str().unwrap().to_string();
    let game = opener.game_of(1).unwrap();
    game.set_behaviour(|b| b.focus_works = true);
    *opener.foreground.lock().unwrap() = Some(1);
    // From the account's home (where open_session starts it) nothing may be looked at or clicked: the first world is a start.
    let first = client.tool("go_to_world", json!({ "session": id, "world_id": LAB_WORLD_A })).unwrap();
    assert_eq!((first["arrived"].as_bool(), first["travelled"].as_bool()), (Some(true), None), "{first}");
    assert_eq!((game.launches().len(), game.links().len()), (2, 0));
    for (round, world) in [LAB_WORLD_B, LAB_WORLD_A, LAB_WORLD_B].into_iter().enumerate() {
        // VRChat shows the world's page for the link: the answer says to click Join, and nothing was started or closed.
        let went = client.tool("go_to_world", json!({ "session": id, "world_id": world })).unwrap();
        assert_eq!((went["arrived"].as_bool(), went["travelled"].as_bool()), (Some(false), Some(true)), "{went}");
        assert!(went["next"].as_str().unwrap().contains("Join"), "{went}");
        assert!(game.world_page_up());
        assert_eq!(game.links().len(), round + 1);
        assert_eq!(game.links()[round].world_id, world);
        client.tool("screenshot", json!({ "session": id })).unwrap();
        client.tool("mouse", json!({ "session": id, "action": "click", "x": 320, "y": 180 })).unwrap();
        let there = client.tool("wait_for", json!({ "session": id, "until": "in_world", "timeout_s": 5 })).unwrap();
        assert_eq!(there["arrived"], true, "{there}");
        let state = client.tool("get_state", json!({ "session": id })).unwrap();
        assert_eq!(state["place"]["world_id"], world, "{state}");
        assert!(state["place"]["instance"].as_str().unwrap().contains("~private(usr_00000001"), "its own private instance: {state}");
    }
    assert_eq!((game.launches().len(), game.closes()), (2, 1), "four worlds, one start after the home one");
    // A VRChat that travels at once answers arrived.
    game.set_behaviour(|b| b.link_opens_page = false);
    let went = client.tool("go_to_world", json!({ "session": id, "world_id": LAB_WORLD_A })).unwrap();
    assert_eq!((went["arrived"].as_bool(), went["travelled"].as_bool()), (Some(true), Some(true)), "{went}");
    // The link pipe is another VRChat's (the person's, started first): never written to; started again, and said why.
    game.set_behaviour(|b| b.link_refused = true);
    let went = client.tool("go_to_world", json!({ "session": id, "world_id": LAB_WORLD_B })).unwrap();
    assert_eq!(went["arrived"], true, "{went}");
    assert!(went["started_again_because"].as_str().unwrap().contains("another VRChat"), "{went}");
    assert_eq!((game.launches().len(), game.closes(), game.links().len()), (3, 2, 4));
    // Asked for, a start again.
    game.set_behaviour(|b| b.link_refused = false);
    let went = client.tool("go_to_world", json!({ "session": id, "world_id": LAB_WORLD_A, "relaunch": true })).unwrap();
    assert_eq!(went["arrived"], true, "{went}");
    assert_eq!((game.launches().len(), game.links().len()), (4, 4));
}

#[test]
fn a_start_that_cannot_come_up_releases_everything_and_says_why() {
    let (_main, opener, bridge, _server, client) = lab_with_sessions("start-failed");
    // Easy Anti-Cheat's driver service marked for deletion (2026-11-15 07:17): nothing is tried.
    *opener.eac.lock().unwrap() = super::sessions::Eac { service: "STOPPED".into(), driver: "STOPPED".into(), vrchat_runs: false, launcher_runs: false, delete_pending: true };
    let stuck = client.fails("open_session", json!({ "profile": 1 }), "eac_stuck");
    assert!(stuck["message"].as_str().unwrap().contains("until Windows restarts"), "{stuck}");
    assert!(opener.games.lock().unwrap().iter().all(|g| g.2.launches().is_empty()), "nothing was launched");
    assert!(bridge.sides.lock().unwrap().is_empty());
    let first = opener.games.lock().unwrap()[0].0.clone();
    assert!(opener.forgotten.lock().unwrap().contains(&first), "what the refused start took was given back");
    *opener.eac.lock().unwrap() = super::sessions::Eac { service: "ABSENT".into(), driver: "ABSENT".into(), vrchat_runs: false, launcher_runs: false, delete_pending: false };
    // The launcher hangs on its error window (07:04: "Launch Error (30005)"), then says so in its log: ended, and plain words.
    *opener.launches_hang.lock().unwrap() = true;
    // Its outcome line as Easy Anti-Cheat wrote it on 2026-11-13 15:16 (anticheatlauncher-backup-2026.11.13-15.16.54.log).
    *opener.launcher_said.lock().unwrap() = super::sessions::launcher_outcome("[15:16:39:736] [Windows] [EAC Bootstrapper] [Info] Launcher finished with: 206, 'EAC unload timeout'.\n");
    let begin = Instant::now();
    let failed = client.fails("open_session", json!({ "profile": 1 }), "start_failed");
    assert!(begin.elapsed() < Duration::from_secs(10), "known from the launcher's log, not after 30 s");
    assert_eq!(failed["retry_after_s"], 90, "{failed}");
    let message = failed["message"].as_str().unwrap();
    assert!(message.contains("206: EAC unload timeout") && message.contains("looked again"), "{failed}");
    assert_eq!(opener.launchers_ended.lock().unwrap().as_slice(), [(9010, 9011)], "its own launcher ended");
    let second = opener.games.lock().unwrap()[1].0.clone();
    assert!(opener.forgotten.lock().unwrap().contains(&second), "its OSC listener and mark released");
    assert!(bridge.sides.lock().unwrap().is_empty(), "no session left behind");
    // The launcher said 1072 (marked for deletion) after a start: eac_stuck too.
    *opener.launcher_said.lock().unwrap() = super::sessions::launcher_outcome("[06:04:55:187] [Windows] [EAC Bootstrapper] [Info] Launcher finished with: 206, 'CreateService failed with 1072.'.\n");
    client.fails("open_session", json!({ "profile": 1 }), "eac_stuck");
    // Once it can, the same profile opens at once: nothing of the failed starts is in the way.
    *opener.launches_hang.lock().unwrap() = false;
    *opener.launcher_said.lock().unwrap() = None;
    open_and_arrive(&client, 1, LAB_WORLD_A);
}

/// Easy Anti-Cheat's success line, verbatim from the start of 2026-11-15 12:25 (`anticheatlauncher.log`, times UTC).
const LOADED_301: &str = "[11:25:13:653] [Windows] [EAC Bootstrapper] [Info] Launcher finished with: 301, 'Easy Anti-Cheat successfully loaded in-game'.
[11:25:13:653] [Windows] [EAC Bootstrapper] [Info] Successfully initialized the Easy Anti-Cheat module, waiting for game window to become visible.
[11:25:13:653] [Windows] [EAC Bootstrapper] [Info] Unlocking the launch lock, allowing next game to launch to take place.
";

/// [`lab_with_sessions`] whose starts are given up on sooner: each wait for a VRChat to come up lasts `start_limit`.
fn lab_with_sessions_timed(name: &str, start_limit: Duration) -> (Arc<LabGame>, Arc<LabOpener>, Arc<Bridge>, Server, Client) {
    let (game, bridge, server, client) = lab_timed(name, start_limit);
    let opener = LabOpener::new(&std::env::temp_dir().join(format!("orbiters-bridge-sessions-{name}-{}", uuid::Uuid::new_v4().simple())));
    bridge.set_opener(opener.clone());
    (game, opener, bridge, server, client)
}

#[test]
fn a_start_whose_anti_cheat_says_loaded_is_its_sessions_vrchat() {
    // 2026-11-15 12:25: the launcher's 301 came before VRChat.exe, and go_to_world answered start_failed for a VRChat that
    // came up and arrived; the bridge then called it the person's.
    let (_main, opener, _bridge, _server, client) = lab_with_sessions("loaded-301");
    *opener.launcher_said.lock().unwrap() = super::sessions::launcher_outcome(LOADED_301);
    assert_eq!(opener.launcher_said.lock().unwrap().as_ref().map(|s| s.0), Some(301));
    *opener.launches_come_up_after.lock().unwrap() = Some(Duration::from_millis(1500));
    let opened = client.tool("open_session", json!({ "profile": 1 })).expect("a 301 start is a start");
    assert_eq!(opened["adopted"], false, "{opened}");
    let id = opened["session"].as_str().unwrap().to_string();
    let arrived = client.tool("go_to_world", json!({ "session": id, "world_id": LAB_WORLD_A })).expect("the first world from home starts it again: 301 again");
    assert_eq!(arrived["arrived"], true, "{arrived}");
    let state = client.tool("get_state", json!({ "session": id })).unwrap();
    assert_eq!(state["vrchat"]["started_by_bridge"], true, "{state}");
    assert!(opener.launchers_ended.lock().unwrap().is_empty(), "a start that works has nothing ended");
    assert_eq!(client.tool("list_sessions", json!({})).unwrap()["persons_vrchats_running"], 0);
    assert_eq!(opener.game_of(1).unwrap().launches().len(), 2);
}

#[test]
fn a_start_given_up_on_whose_vrchat_comes_up_later_is_its_session_never_the_persons() {
    let limit = Duration::from_secs(5);
    let (_main, opener, _bridge, _server, client) = lab_with_sessions_timed("given-up", limit);
    let id = client.tool("open_session", json!({ "profile": 1 })).unwrap()["session"].as_str().unwrap().to_string();
    let game = opener.game_of(1).unwrap();
    // The next start: the anti-cheat says it loaded, and VRChat takes longer than both waits.
    *opener.launcher_said.lock().unwrap() = super::sessions::launcher_outcome(LOADED_301);
    game.set_behaviour(|b| b.comes_up_after = Some(limit * 2 + Duration::from_secs(4)));
    let launched = Instant::now();
    let failed = client.fails("go_to_world", json!({ "session": id, "world_id": LAB_WORLD_A }), "start_failed");
    let message = failed["message"].as_str().unwrap();
    assert!(message.contains("said 301") && message.contains("looked again") && !message.contains("nothing of this start is left running"), "{failed}");
    assert!(failed["next"].as_str().unwrap().contains("get_state"), "{failed}");
    assert_eq!(opener.launchers_ended.lock().unwrap().as_slice(), [(9010, 9011)]);
    // It comes up: the session's own, where its start was going, never the person's.
    *opener.persons.lock().unwrap() = vec![4243];
    let deadline = launched + limit * 2 + Duration::from_secs(4) + PATIENCE;
    let state = loop {
        let state = client.tool("get_state", json!({ "session": id })).unwrap();
        if state["vrchat"]["started_by_bridge"] == true && state["place"]["in_world"] == true || Instant::now() > deadline {
            break state;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    assert_eq!(state["vrchat"]["started_by_bridge"], true, "{state}");
    assert_eq!(state["place"]["world_id"], LAB_WORLD_A, "{state}");
    assert_eq!(client.tool("list_sessions", json!({})).unwrap()["persons_vrchats_running"], 1, "only the person's own VRChat");
    assert_eq!(game.launches().len(), 2, "taken, not launched again");
    // And it is driven as the bridge's: a screenshot is not refused as not_started_by_bridge.
    if let Err(refused) = client.tool("screenshot", json!({ "session": id })) {
        assert_ne!(refused["code"], "not_started_by_bridge", "{refused}");
    }
}

#[test]
fn open_session_takes_the_vrchat_a_failed_start_of_its_profile_left_running() {
    let limit = Duration::from_secs(5);
    let (_main, opener, bridge, _server, client) = lab_with_sessions_timed("left-running", limit);
    *opener.launcher_said.lock().unwrap() = super::sessions::launcher_outcome(LOADED_301);
    *opener.launches_come_up_after.lock().unwrap() = Some(limit * 2 + Duration::from_secs(3));
    let launched = Instant::now();
    client.fails("open_session", json!({ "profile": 1 }), "start_failed");
    assert!(bridge.sides.lock().unwrap().is_empty());
    let first = opener.games.lock().unwrap()[0].0.clone();
    assert!(opener.forgotten.lock().unwrap().contains(&first));
    // Its VRChat comes up after all (profile 1, the session's ports): the next open_session takes it, launching nothing.
    std::thread::sleep((launched + limit * 2 + Duration::from_secs(4)).saturating_duration_since(Instant::now()));
    *opener.launches_come_up_after.lock().unwrap() = None;
    let opened = client.tool("open_session", json!({ "profile": 1 })).expect("adopted");
    assert_eq!(opened["adopted"], true, "{opened}");
    assert!(opened["next"].as_str().unwrap().contains("Nothing was launched"), "{opened}");
    let id = opened["session"].as_str().unwrap().to_string();
    let game = opener.game_of(1).unwrap();
    assert_eq!(game.launches().len(), 1, "no second VRChat");
    assert_eq!(client.tool("get_state", json!({ "session": id })).unwrap()["vrchat"]["started_by_bridge"], true);
    assert_eq!(client.tool("list_sessions", json!({})).unwrap()["persons_vrchats_running"], 0);
    // A VRChat the bridge did not start in another profile (no session's marks) is never taken.
    *opener.others.lock().unwrap() = vec![2];
    client.fails("open_session", json!({ "profile": 2 }), "profile_in_use");
    let refused = opener.games.lock().unwrap().iter().find(|g| g.1 == 2).map(|g| g.0.clone()).unwrap();
    assert!(opener.forgotten.lock().unwrap().contains(&refused), "what the refusal made is given back");
    assert_eq!(opener.game_of(2).unwrap().launches().len(), 0);
}

/// The map (L-tour-map): no plan before a finished picture; then the world's plan with its picture, where the earlier picture
/// stood, the avatar from the recording, suggestions apart from it with aim_camera calls and a walk through the door; a camera
/// sent into a wall is warned of (not refused), a walk towards one too; and the plan outlives the picture's files once it is
/// published and freed, across a restart.
#[test]
fn a_worlds_pictures_plan_the_next_one_and_warn_before_moves() {
    let (game, bridge, _server, client) = lab("map");
    let publish = super::lab::LabPublish::new(game.dir());
    bridge.set_publish(publish.clone());
    let journal = game.dir().join("journal.json");
    bridge.keep_journal(&journal);
    let maps = game.dir().join("maps");
    bridge.keep_maps(maps.clone());
    arrive(&client);
    client.fails("map_plan", json!({}), "no_map");
    open_camera(&game, &client);
    let picture = client.tool("take_3d_picture", json!({ "length_s": 8 })).unwrap();
    let name = picture["session"].as_str().unwrap().to_string();
    // Two rooms joined by a door; the picture was taken in the first, the avatar standing at (3, 5) facing +x.
    super::map::tests::write_session(&game.dir().join(&name), &super::map::tests::two_rooms(), ([0.0, 0.0], [24.0, 10.0]), [4.0, 5.0], [3.0, 1.02, 5.0], 90.0);
    let waiting = client.fails("map_plan", json!({}), "no_map");
    assert!(waiting["message"].as_str().unwrap().contains("finished"), "not before its reconstruction is done: {waiting}");
    client.tool("close_vrchat", json!({})).unwrap();
    assert!(until(PATIENCE, || client.tool("wait_for", json!({ "until": "reconstructions_done", "timeout_s": 2 })).unwrap()["done"] == true));
    arrive(&client);
    open_camera(&game, &client);

    let (status, body) = client.rpc("tools/call", json!({ "name": "map_plan", "arguments": {} }));
    assert_eq!(status, 200);
    let result = &body["result"];
    assert_eq!(result["isError"], false, "{body}");
    assert_eq!(result["content"][0]["mimeType"], "image/png", "the plan's picture");
    let plan = &result["structuredContent"];
    assert_eq!(plan["pictures_used"], 1, "{plan}");
    assert_eq!(plan["verdict"]["pictures"], 3, "two rooms: more than one picture: {plan}");
    assert_eq!(plan["centres"][0]["at"], json!([4.0, 1.6, 5.0]));
    assert_eq!(plan["avatar"]["as_of_picture"], name.as_str(), "{plan}");
    assert_ne!(plan["camera"]["at"], "solid", "{plan}");
    let first = &plan["suggestions"][0];
    assert!(first["why"].as_str().unwrap().contains("another area"), "{plan}");
    assert!(first["from_centres_m"].as_f64().unwrap() >= 10.0, "{first}");
    assert!(first["aim_camera"].as_array().is_some_and(|a| !a.is_empty()), "{first}");
    assert!(first["walk"].as_array().is_some_and(|a| !a.is_empty()), "a walk through the door: {first}");
    assert!(plan.to_string().len() < 4000, "short: {} bytes", plan.to_string().len());

    // The camera sent into the wall between the rooms: it goes (the map has holes), and the answer warns.
    let aimed = client.tool("aim_camera", json!({ "forward_m": 11 })).unwrap();
    assert_eq!(aimed["to"][0], 12.0, "{aimed}");
    assert_eq!(aimed["map"]["at"], "solid", "{aimed}");
    assert!(aimed["map"]["warning"].is_string(), "{aimed}");
    let checked = client.tool("map_check", json!({ "point": [6.0, 1.6, 5.0], "walk_to": [18.0, 5.0] })).unwrap();
    assert_eq!(checked["point"]["at"], "free", "{checked}");
    assert!(checked["walk_to"]["steps"].as_array().is_some_and(|s| !s.is_empty()), "{checked}");
    client.fails("map_check", json!({ "point": [1.0, 2.0] }), "bad_arguments");
    // Backwards from (3, 5) facing +x is west: the outer wall 3 m away.
    let walked = client.tool("walk", json!({ "direction": "backward", "seconds": 2 })).unwrap();
    assert!(walked["map"]["then"].as_str().unwrap().contains("wall"), "{walked}");
    assert!(walked["map"]["clear_m"].as_f64().unwrap() < 4.0, "{walked}");

    // Published and freed whole, the picture still maps its world, after a restart too.
    client.tool("close_vrchat", json!({})).unwrap();
    let published = client.tool("publish_3d_picture", json!({ "picture": name, "visibility": "private" })).unwrap();
    assert_eq!(published["publish"]["state"], "done", "{published}");
    client.tool("free_picture_files", json!({ "picture": name, "what": "everything" })).unwrap();
    assert!(!game.dir().join(&name).exists());
    let (_game2, bridge2, _server2, client2) = lab("map-restart");
    bridge2.keep_journal(&journal);
    bridge2.keep_maps(maps);
    bridge2.set_publish(publish.clone());
    arrive(&client2);
    let again = client2.tool("map_plan", json!({ "image": false })).unwrap();
    assert_eq!(again["pictures_used"], 1, "{again}");
    assert!(again["left_out"].is_null(), "{again}");
}

#[test]
fn a_vr_session_tours_while_the_person_uses_the_computer() {
    // nofocus, 2026-11-15: with the person at the computer every key and click of a desktop session answers paused_by_person, and
    // its camera opens only through the menu: the tour stood still. A VR session goes on: no key, no click, no focus of theirs.
    let (_main, opener, bridge, _server, client) = lab_with_sessions("vr");
    *opener.links_open_page.lock().unwrap() = true;
    // Where nobody set the sandbox up: said, nothing started.
    let none = client.fails("open_session", json!({ "profile": 1, "vr": true }), "vr_unavailable");
    assert!(none["message"].as_str().unwrap().contains("vrchat_bridge_vr_sandbox"), "{none}");
    assert!(opener.games.lock().unwrap().is_empty(), "nothing was made");
    // The person's VR in use: refused with its reason, nothing launched, the turn not kept.
    *opener.vr.lock().unwrap() = Some(Some("the owner's headset connected through Virtual Desktop".into()));
    let busy = client.fails("open_session", json!({ "profile": 1, "vr": true }), "vr_busy");
    assert!(busy["message"].as_str().unwrap().contains("Virtual Desktop"), "{busy}");
    assert!(opener.games.lock().unwrap().iter().all(|g| g.2.launches().is_empty()));
    // Free: the sandbox's turn first, then VRChat in VR mode with the profile's own ports.
    *opener.vr.lock().unwrap() = Some(None);
    let opened = client.tool("open_session", json!({ "profile": 1, "vr": true })).unwrap();
    let id = opened["session"].as_str().unwrap().to_string();
    let game = opener.game_of(1).unwrap();
    let rig = opener.rigs.lock().unwrap().last().unwrap().clone();
    assert_eq!(rig.calls.lock().unwrap().first().map(String::as_str), Some("take"), "SteamVR before VRChat");
    let args = game.launches()[0].args();
    assert!(!args.contains(&"--no-vr".to_string()), "VR mode: {args:?}");
    assert!(args.contains(&"--profile=1".to_string()) && args.contains(&"--osc=9010:127.0.0.1:9011".to_string()), "{args:?}");
    // The person works from now on: their input anywhere, another program in front.
    *opener.foreground.lock().unwrap() = Some(4242);
    game.set_foreground(false);
    bridge.heard(Heard::PersonInput);
    let went = client.tool("go_to_world", json!({ "session": id, "world_id": LAB_WORLD_A })).unwrap();
    assert_eq!(went["arrived"], true, "{went}");
    assert!(game.launches().iter().all(|p| !p.args().contains(&"--no-vr".to_string())), "a start again stays in VR");
    bridge.heard(Heard::PersonInput);
    let went = client.tool("go_to_world", json!({ "session": id, "world_id": LAB_WORLD_B })).unwrap();
    assert_eq!(went["travelled"], true, "{went}");
    assert!(game.world_page_up(), "the world's page waits for Join");
    // Its screenshot is the headset's eye; a click there is the laser and the trigger, not the person's mouse.
    let shot = client.tool("screenshot", json!({ "session": id, "max_width": 810 })).unwrap();
    assert_eq!((shot["vr"].as_bool(), shot["image_width"].as_u64(), shot["image_height"].as_u64()), (Some(true), Some(810), Some(900)), "{shot}");
    bridge.heard(Heard::PersonInput);
    let clicked = client.tool("mouse", json!({ "session": id, "action": "click", "x": 405, "y": 520 })).unwrap();
    assert_eq!(clicked["over"], "vr_controller", "{clicked}");
    assert!(rig.calls.lock().unwrap().contains(&"point 405 520 810 900 true".to_string()), "{:?}", rig.calls.lock().unwrap());
    assert!(game.injected().is_empty(), "no key or click of this computer's");
    let there = client.tool("wait_for", json!({ "session": id, "until": "in_world", "timeout_s": 5 })).unwrap();
    assert_eq!(there["arrived"], true, "{there}");
    // Keys and the focus do not exist here: said, with what to use.
    let key = client.fails("press", json!({ "session": id, "input": "esc" }), "vr_session");
    assert!(key["next"].as_str().unwrap().contains("open_camera"), "{key}");
    client.fails("focus_game", json!({ "session": id }), "vr_session");
    // The camera: the controllers' route, then Stream over OSC; then the picture as always, the person still working.
    bridge.heard(Heard::PersonInput);
    let camera = client.tool("open_camera", json!({ "session": id })).unwrap();
    assert_eq!(camera["camera"], "open", "{camera}");
    assert!(rig.calls.lock().unwrap().contains(&"camera".to_string()));
    assert!(game.heard().iter().any(|(h, _)| h.address == "/usercamera/Mode"), "Stream asked over OSC");
    client.tool("walk", json!({ "session": id, "direction": "forward", "seconds": 0.2 })).unwrap();
    bridge.heard(Heard::PersonInput);
    let picture = client.tool("take_3d_picture", json!({ "session": id, "length_s": 4 })).unwrap();
    assert!(picture["session"].is_string() || picture["saved"].is_boolean() || picture.is_object(), "{picture}");
    assert!(game.injected().is_empty(), "still nothing of the person's keyboard or mouse");
    // The hotkey still holds it.
    bridge.heard(Heard::Hotkey);
    client.fails("open_camera", json!({ "session": id }), "paused_held");
    bridge.heard(Heard::Hotkey);
    // A desktop session's camera: the menu's clicks, said.
    let desktop = client.tool("open_session", json!({ "profile": 2 })).unwrap();
    let d = desktop["session"].as_str().unwrap().to_string();
    client.fails("open_camera", json!({ "session": d }), "desktop_session");
    // Closed: the sandbox turn is given back.
    client.tool("wait_for", json!({ "session": id, "until": "picture_saved", "timeout_s": 20 })).unwrap();
    client.tool("close_session", json!({ "session": id })).unwrap();
    assert_eq!(rig.calls.lock().unwrap().last().map(String::as_str), Some("release"), "{:?}", rig.calls.lock().unwrap());
}
