//! Verify2-r21: inputs are bound at capture to the Orbiters identity and to the question they were shown; a turn's side
//! effects are fenced against identity turnover. Driven through the real command handler, with only the model (a
//! stand-in Orbiters) faked.

use super::*;
use orbiters_n4n0_core::intents::{Facts, Intent};

fn full() -> Facts {
    Facts { vrchat: true, signed_in: true, vrchat_account: true, calls: true, n4n0_prop: true, can_drop: true, computer: true }
}

fn ready() -> (Arc<TestHost>, Shared, Device) {
    let (t, n, device) = n4n0_on(Settings::default(), Arc::new(Watcher::new(|| Box::new(NoMedia))));
    *t.app_facts.lock().unwrap() = Some(full());
    t.scene(|s| s.vrchat = true);
    *device.ears.allowed.lock().unwrap() = true;
    n.n4n0.lock().unwrap().voice.installed = true;
    (t, n, device)
}

fn call(id: &str, tool: &str, input: Value) -> Value {
    json!({ "id": id, "name": tool, "input": input })
}

fn boop(friend: &str) -> Value {
    json!([call("b1", "orbiters_friends", json!({ "intent": "boop", "friend": friend }))])
}

/// A stand-in Orbiters that holds every answer until `go` is set (a slow model), then sends it: server-sent events, or
/// (`json`) one JSON answer.
fn held_orbiters(answers: Vec<String>, go: Arc<std::sync::atomic::AtomicBool>, json: bool) -> (String, Arc<Mutex<Vec<Value>>>) {
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
            let start = Instant::now();
            while !go.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(5));
            }
            let mut stream = stream;
            if json {
                let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", events.len()); // i18n: not shown
                let _ = stream.write_all(head.as_bytes());
            } else {
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"); // i18n: not shown
            }
            let _ = stream.write_all(events.as_bytes());
            let _ = stream.flush();
        }
    });
    (base, bodies)
}

fn until(what: &str, f: impl Fn() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(8), "never: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn question_shown(t: &TestHost) -> Option<u64> {
    t.view().confirmations.iter().rev().find_map(|c| c.local_question())
}

/// The model asks to boop `friend` (it waits for a yes), and the turn ends.
fn model_asks(t: &TestHost, n: &Shared, friend: &str, token: &str) {
    let (base, bodies) = orbiters(vec![done(json!([{ "role": "user", "content": "x" }]), boop(friend)), done(json!([]), json!([]))]);
    t.sign_in(base, token);
    n.ask(format!("boop {friend}"), "typed");
    turn_ended(n, &bodies, 2);
}

// --- What changes between the code before and after the fix: how a surface sends, and how the account changes. ---

/// A surface's Yes or No pressed while it showed `question`, for the account epoch it showed.
fn press_answer(n: &Shared, yes: bool, question: Option<u64>, epoch: u64) {
    let _ = (question, epoch); // BEFORE: no stamp in this API
    n.command(Command::AskAnswer { yes, question: question.unwrap_or(0) });
}

/// A sentence typed and sent while the surface showed `question`, for the account epoch it showed.
fn typed(n: &Shared, text: &str, question: Option<u64>, epoch: u64) {
    let _ = epoch; // BEFORE
    n.command(Command::Ask { text: text.into(), question });
}

/// The window's Talk pressed while it showed `question`.
fn talk_pressed(n: &Shared, question: Option<u64>, epoch: u64) {
    let _ = (question, epoch); // BEFORE
    n.app_talk(true, false);
}

/// The account epoch the windows are shown now.
fn shown_epoch(t: &TestHost) -> u64 {
    let _ = t;
    0 // BEFORE: no epoch
}

/// Another Orbiters member is signed in (as the controller's identity gate does it).
fn switch_member(t: &TestHost, n: &Shared, base: String, token: &str) {
    n.command(Command::ForgetAccount); // BEFORE
    t.sign_in(base, token);
}

fn heard(n: &Shared, listen: u64, text: &str) {
    n.on_voice_event(VoiceEvent::Heard { text: text.into(), for_n4n0: true, seconds: 1.0, decode_ms: 50, listen });
}

// --- B1: an answer authorises only the question it was shown ---

/// Verify2 B1 (speech): Talk pressed while nothing was asked; another input then makes N4N0 ask to boop Sam; the
/// recognition of "yes" ends after that. The sentence was not an answer to a question the person never heard.
#[test]
fn a_late_spoken_yes_never_answers_a_question_asked_after_talk_was_pressed() {
    let (t, n, _device) = ready();
    let (base, _) = orbiters(vec![]);
    t.sign_in(base, "orbit-a");
    let epoch = shown_epoch(&t);
    talk_pressed(&n, None, epoch);
    assert!(n.n4n0.lock().unwrap().listening.is_some(), "Talk is pressed");
    n.app_talk(false, false);
    model_asks(&t, &n, "Sam", "orbit-a");
    assert!(question_shown(&t).is_some());
    heard(&n, 1, "yes");
    std::thread::sleep(Duration::from_millis(200));
    assert!(t.app_intents.lock().unwrap().is_empty(), "the late yes booped Sam: {:?}", t.app_intents.lock().unwrap());
    assert!(question_shown(&t).is_some(), "the question still waits");
    t.shutdown(&n);
}

/// Verify2 B1 (typed, overlay): answers sent for question A that arrive after B was asked never resolve B.
#[test]
fn answers_shown_question_a_never_resolve_b() {
    let (t, n, _device) = ready();
    let epoch = shown_epoch(&t);
    model_asks(&t, &n, "Sam", "orbit-a");
    let a = question_shown(&t).unwrap();
    press_answer(&n, false, Some(a), epoch);
    model_asks(&t, &n, "Mira", "orbit-a");
    let b = question_shown(&t).unwrap();
    // The overlay's late press on A (its painter still showed A), a typed yes deferred from A's moment.
    press_answer(&n, true, Some(a), epoch);
    typed(&n, "yes", Some(a), epoch);
    assert!(t.app_intents.lock().unwrap().is_empty(), "{:?}", t.app_intents.lock().unwrap());
    assert_eq!(question_shown(&t), Some(b), "B still waits");
    // A typed yes sent with no question shown: no answer either.
    typed(&n, "yes", None, epoch);
    assert!(t.app_intents.lock().unwrap().is_empty());
    assert_eq!(question_shown(&t), Some(b));
    press_answer(&n, true, Some(b), epoch);
    assert_eq!(*t.app_intents.lock().unwrap(), vec![Intent::Boop { friend: Some("Mira".into()) }]);
    t.shutdown(&n);
}

// --- B2: a turn of A's acts and shows nothing once B took over ---

/// Verify2 B2: A's turn is under way when B is signed in; A's late answer then arrives with a tool call to run at once
/// (fast walk), one that waits for a yes (boop), a website approval, streamed words and a quota. Nothing of it runs or
/// shows for B.
#[test]
fn a_late_answer_for_a_runs_and_shows_nothing_after_b_took_over() {
    let (t, n, _device) = ready();
    let go = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The whole answer as one JSON body (read to the end before anything of it is looked at).
    let late = json!({
        "text": "A's private answer",
        "messages": [{ "role": "user", "content": "x" }],
        "tool_calls": [call("w1", "orbiters_walk", json!({ "intent": "fast_walk", "on": true })), call("b1", "orbiters_friends", json!({ "intent": "boop", "friend": "Sam" }))],
        "quota": { "used": 7, "limit": 80 }
    })
    .to_string();
    let (base, bodies) = held_orbiters(vec![late], go.clone(), true);
    t.sign_in(base, "orbit-a");
    typed(&n, "turn on fast walk and boop Sam", None, shown_epoch(&t));
    until("A's request", || !bodies.lock().unwrap().is_empty());
    let (base_b, _) = orbiters(vec![]);
    switch_member(&t, &n, base_b, "orbit-b");
    go.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(600));
    assert!(!t.settings().walk_boost.enabled, "A's tool ran after B took over");
    assert!(t.app_intents.lock().unwrap().is_empty());
    let view = t.view();
    assert!(view.confirmations.is_empty(), "A's approvals shown to B: {:?}", view.confirmations);
    assert!(view.lines.is_empty(), "A's lines shown to B: {:?}", view.lines);
    assert!(view.quota.is_none(), "A's quota shown to B");
    assert!(view.assistant.log.is_empty(), "A's log shown to B");
    t.shutdown(&n);
}

// --- B3: input of A's never goes out with B's credential ---

/// Verify2 B3: a sentence typed for A (sent from a window still showing A) and a sentence A said (Talk pressed under A,
/// recognised after B signed in) reach the service after B took over: neither goes out with B's credential.
#[test]
fn input_of_a_never_goes_out_as_b() {
    let (t, n, _device) = ready();
    let (base_a, _) = orbiters(vec![]);
    t.sign_in(base_a, "orbit-a");
    let epoch_a = shown_epoch(&t);
    talk_pressed(&n, None, epoch_a);
    assert!(n.n4n0.lock().unwrap().listening.is_some(), "Talk is pressed");
    n.app_talk(false, false);
    let (base_b, bodies_b) = orbiters(vec![done(json!([]), json!([])), done(json!([]), json!([]))]);
    switch_member(&t, &n, base_b, "orbit-b");
    typed(&n, "A's private draft", None, epoch_a);
    heard(&n, 1, "A's private words");
    std::thread::sleep(Duration::from_millis(400));
    let bodies = bodies_b.lock().unwrap().iter().map(Value::to_string).collect::<Vec<_>>();
    assert!(bodies.is_empty(), "A's input went out as B: {bodies:?}");
    assert!(!t.view().lines.iter().any(|l| l.text.contains("A's private")), "A's words shown to B: {:?}", t.view().lines);
    t.shutdown(&n);
}
