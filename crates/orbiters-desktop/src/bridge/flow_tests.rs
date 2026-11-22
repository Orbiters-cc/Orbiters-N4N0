//! The bridge's flows on every computer, a Mac included (the end-to-end module `e2e.rs` drives a real window and runs on
//! Windows only): the lab's stand-in game (`lab.rs`, whose window off Windows is no system window at all) behind the real
//! MCP listener, with a Mac's capabilities or a PC's. What a Mac's bridge offers, what it refuses before any side effect,
//! the observe-and-travel loop, a crashed start, a game whose graphics went, and the person's own VRChat.

use super::caps::Capabilities;
use super::lab::{LabGame, LAB_WORLD_A, LAB_WORLD_B};
use super::mcp::{Server, MODERN};
use super::{Bridge, Timing};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

const TOKEN: &str = "flow-token";

/// One JSON-RPC message over the real listener, as an MCP client of the current revision sends it.
fn rpc(port: u16, method: &str, params: Value) -> Value {
    let name = params["name"].as_str().or(params["uri"].as_str()).map(str::to_owned);
    let mut params = params;
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": { "name": "flow", "version": "1" },
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    let mut head = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {MODERN}\r\nMcp-Method: {method}\r\nConnection: close\r\nContent-Length: {}\r\n", // i18n: not shown
        body.len()
    );
    if let Some(name) = name {
        head.push_str(&format!("Mcp-Name: {name}\r\n"));
    }
    head.push_str("\r\n");
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the listener");
    stream.set_read_timeout(Some(Duration::from_secs(90))).unwrap();
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body.as_bytes()).unwrap();
    // Read up to the answer's length: the listener may keep the connection open after it.
    let mut reply = Vec::new();
    let mut chunk = [0u8; 65536];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => reply.extend_from_slice(&chunk[..n]),
        }
        if let Some(end) = reply.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&reply[..end]).to_ascii_lowercase();
            let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length: ")).and_then(|l| l.trim().parse().ok()).unwrap_or(0);
            if reply.len() >= end + 4 + length {
                break;
            }
        }
    }
    let end = reply.windows(4).position(|w| w == b"\r\n\r\n").expect("an HTTP answer");
    serde_json::from_slice(&reply[end + 4..]).unwrap_or(Value::Null)
}

struct Flow {
    game: Arc<LabGame>,
    bridge: Arc<Bridge>,
    server: Server,
}

impl Flow {
    fn new(name: &str, caps: Capabilities) -> Self {
        let dir = std::env::temp_dir().join(format!("orbiters-bridge-flow-{name}-{}", uuid::Uuid::new_v4().simple()));
        let game = LabGame::new(&dir, None);
        let bridge = Bridge::with_timing(game.clone(), None, Timing { instance_every: Duration::from_millis(50), start_limit: Duration::from_secs(30), close_limit: Duration::from_secs(30) });
        bridge.set_capabilities(caps);
        let server = Server::start(bridge.clone(), 0, TOKEN.to_string()).expect("a free port");
        Self { game, bridge, server }
    }

    fn tool(&self, name: &str, arguments: Value) -> Result<Value, Value> {
        let answer = rpc(self.server.port(), "tools/call", json!({ "name": name, "arguments": arguments }));
        let result = &answer["result"];
        if result["isError"] == true {
            Err(result["structuredContent"].clone())
        } else {
            Ok(result["structuredContent"].clone())
        }
    }

    fn fails(&self, name: &str, arguments: Value, code: &str) -> Value {
        let failure = self.tool(name, arguments).expect_err(name);
        assert_eq!(failure["code"], code, "{name}: {failure}");
        assert!(!failure["next"].as_str().unwrap_or_default().is_empty(), "{failure}");
        failure
    }

    fn names(&self) -> Vec<String> {
        rpc(self.server.port(), "tools/list", json!({}))["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect()
    }
}

#[test]
fn a_mac_offers_observe_and_travel_and_refuses_the_rest_before_anything_happens() {
    let flow = Flow::new("mac-offers", Capabilities::MAC);
    let names = flow.names();
    for offered in ["get_state", "find_worlds", "go_to_world", "close_vrchat", "wait_for", "screenshot", "world_labels", "pictures", "unity_list_projects"] {
        assert!(names.iter().any(|n| n == offered), "{offered} in {names:?}");
    }
    for missing in ["walk", "turn", "press", "mouse", "focus_game", "wear_avatar", "aim_camera", "take_3d_picture", "open_camera", "open_session", "close_session", "vr_sandbox"] {
        assert!(!names.iter().any(|n| n == missing), "{missing} is not offered on a Mac");
    }
    let list = rpc(flow.server.port(), "tools/list", json!({}));
    let go = list["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "go_to_world").unwrap().clone();
    assert!(go["inputSchema"]["properties"].get("relaunch").is_none() && go["inputSchema"]["properties"].get("session").is_none());
    let guide = rpc(flow.server.port(), "resources/read", json!({ "uri": "bridge://guide" }));
    let text = guide["result"]["contents"][0]["text"].as_str().unwrap_or_default();
    assert!(text.contains("on a Mac") && !text.contains("take_3d_picture` with"), "the Mac's guide");
    let init = rpc(flow.server.port(), "server/discover", json!({}));
    assert!(init.to_string().contains("on this Mac") || init["result"].is_null(), "{init}");

    // Called anyway: refused by name, and nothing done (no instance made, no start, no close).
    for (name, args) in [
        ("walk", json!({ "direction": "forward", "seconds": 1 })),
        ("press", json!({ "input": "jump" })),
        ("mouse", json!({ "action": "click", "x": 1, "y": 1 })),
        ("take_3d_picture", json!({})),
        ("open_session", json!({ "profile": 1 })),
        ("go_to_world", json!({ "world_id": LAB_WORLD_A, "profile": 2 })),
    ] {
        flow.fails(name, args, "not_on_this_computer");
    }
    assert!(flow.game.instances().is_empty() && flow.game.launches().is_empty() && flow.game.closes() == 0);
    let state = flow.tool("get_state", json!({})).unwrap();
    assert_eq!(state["this_computer"]["platform"], "macos");
    assert_eq!(state["this_computer"]["not_available"].as_array().map(Vec::len), Some(5));
}

#[test]
fn a_pc_offers_every_tool_as_before() {
    let flow = Flow::new("pc-offers", Capabilities::WINDOWS);
    let all: Vec<String> = super::tools::list().as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(flow.names(), all);
    assert_eq!(super::tools::list_for(&Capabilities::WINDOWS), super::tools::list());
    assert_eq!(super::tools::guide_for(&Capabilities::WINDOWS), super::tools::GUIDE);
    assert!(flow.tool("get_state", json!({})).unwrap().get("this_computer").is_none(), "a PC's state is as it was");
}

#[test]
fn a_mac_goes_from_world_to_world_looks_and_closes() {
    let flow = Flow::new("mac-travel", Capabilities::MAC);
    let arrived = flow.tool("go_to_world", json!({ "world_id": LAB_WORLD_A })).expect("go_to_world");
    let arrived = if arrived["arrived"] == true { arrived } else { flow.tool("wait_for", json!({ "until": "in_world" })).expect("arrives") };
    assert_eq!(arrived["arrived"], true, "{arrived}");
    assert!(arrived["next"].as_str().unwrap().contains("screenshot"), "{arrived}");
    let answer = rpc(flow.server.port(), "tools/call", json!({ "name": "screenshot", "arguments": {} }));
    let info = &answer["result"]["structuredContent"];
    assert_eq!((info["window_width"].as_u64(), info["window_height"].as_u64()), (Some(1280), Some(720)), "{answer}");
    assert_eq!(info["window_restored"], false, "a Mac never resizes the window");
    assert_eq!(flow.game.resizes(), 0);
    assert!(answer["result"]["content"].as_array().unwrap().iter().any(|c| c["type"] == "image"));
    let state = flow.tool("get_state", json!({})).unwrap();
    assert_eq!(state["vrchat"]["started_by_bridge"], true);
    assert_eq!(state["vrchat"]["renderer_lost"], false);
    assert!(state["next"].as_str().unwrap().contains("go_to_world"), "{}", state["next"]);

    // The next world is a new start: the first game asked to close, the second started in a new private instance.
    std::thread::sleep(Duration::from_millis(60));
    let second = flow.tool("go_to_world", json!({ "world_id": LAB_WORLD_B })).expect("the next world");
    let second = if second["arrived"] == true { second } else { flow.tool("wait_for", json!({ "until": "in_world" })).unwrap() };
    assert_eq!(second["world_id"], LAB_WORLD_B);
    assert_eq!((flow.game.closes(), flow.game.launches().len(), flow.game.instances().len()), (1, 2, 2));
    flow.tool("close_vrchat", json!({})).expect("closes");
    assert_eq!(flow.game.closes(), 2);
    assert_eq!(flow.tool("get_state", json!({})).unwrap()["vrchat"]["running"], false);
}

#[test]
fn a_start_that_crashes_is_said_at_once_and_a_lost_renderer_stops_the_looking() {
    let flow = Flow::new("mac-crash", Capabilities::MAC);
    flow.game.set_behaviour(|b| b.start_fails = Some("VRChat closed shortly after it started.".into()));
    let begin = std::time::Instant::now();
    let failure = flow.tool("go_to_world", json!({ "world_id": LAB_WORLD_A })).and_then(|a| if a["arrived"] == true { Ok(a) } else { flow.tool("wait_for", json!({ "until": "in_world" })) });
    let failure = failure.expect_err("a crashed start fails");
    assert_eq!(failure["code"], "start_crashed", "{failure}");
    assert!(begin.elapsed() < Duration::from_secs(20), "not after the start's whole time");

    // A game that arrived, then lost its graphics: nothing is looked at, and the agent is told to close it.
    let flow = Flow::new("mac-renderer", Capabilities::MAC);
    let arrived = flow.tool("go_to_world", json!({ "world_id": LAB_WORLD_A })).unwrap();
    if arrived["arrived"] != true {
        flow.tool("wait_for", json!({ "until": "in_world" })).unwrap();
    }
    flow.game.set_behaviour(|b| b.renderer_lost = true);
    flow.fails("screenshot", json!({}), "renderer_lost");
    let state = flow.tool("get_state", json!({})).unwrap();
    assert_eq!(state["vrchat"]["renderer_lost"], true);
    assert!(state["next"].as_str().unwrap().contains("close_vrchat"));
    flow.tool("close_vrchat", json!({})).expect("still closes");
}

#[test]
fn the_person_s_vrchat_on_a_mac_is_never_touched() {
    let flow = Flow::new("mac-person", Capabilities::MAC);
    flow.game.set_foreign_game(Some("1~private(usr_someone)~region(eu)"));
    flow.fails("go_to_world", json!({ "world_id": LAB_WORLD_A }), "not_started_by_bridge");
    flow.fails("close_vrchat", json!({}), "not_started_by_bridge");
    flow.fails("screenshot", json!({}), "not_started_by_bridge");
    assert_eq!((flow.game.closes(), flow.game.launches().len()), (0, 0));
    assert!(flow.game.instances().is_empty(), "no instance is made for a start that cannot happen");
    drop(flow.bridge);
}
