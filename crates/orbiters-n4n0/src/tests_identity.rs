//! Verify2-r21: inputs are bound at capture to the Orbiters identity and to the question they were shown; a turn's side
//! effects are fenced against identity turnover. Driven through the real command handler, with only the model (a
//! stand-in Orbiters) faked.

use super::*;
use orbiters_n4n0_core::intents::{Facts, Intent};

fn full() -> Facts {
    Facts { vrchat: true, signed_in: true, vrchat_account: true, calls: true, n4n0_prop: true, can_drop: true, computer: true, clipboard: true, actions: true }
}

fn ready() -> (Arc<TestHost>, Shared, Device) {
    let (t, n, device) = n4n0_on(Settings::default(), Arc::new(Watcher::new(|| Box::new(NoMedia))));
    *t.app_facts.lock().unwrap() = Some(full());
    t.scene(|s| s.vrchat = true);
    *device.ears.allowed.lock().unwrap() = true;
    n.n4n0.lock().unwrap().voice.installed = true;
    // As a start does: the windows are shown the identity's epoch.
    n.publish();
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
            // Orbiters takes the request at once; what it answers waits.
            let mut stream = stream;
            if json {
                let head = format!("HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: {}
Connection: close

", events.len()); // i18n: not shown
                let _ = stream.write_all(head.as_bytes());
            } else {
                let _ = stream.write_all(b"HTTP/1.1 200 OK
Content-Type: text/event-stream
Connection: close

"); // i18n: not shown
            }
            let _ = stream.flush();
            let start = Instant::now();
            while !go.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(5));
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
    n.asked(format!("boop {friend}"), "typed");
    turn_ended(n, &bodies, 2);
}

fn stamp(question: Option<u64>, epoch: u64) -> Option<Stamp> {
    Some(Stamp { epoch, question, surface: Surface::Window, seq: crate::next_input() })
}

/// The window shows `question` (its card came into view).
fn showing(n: &Shared, question: Option<u64>, epoch: u64) {
    if question.is_some() {
        n.command(Command::Presented { stamp: stamp(question, epoch) });
    }
}

/// A surface's Yes or No pressed while it showed `question`, for the account epoch it showed.
fn press_answer(n: &Shared, yes: bool, question: Option<u64>, epoch: u64) {
    showing(n, question, epoch);
    n.command(Command::AskAnswer { yes, stamp: stamp(question, epoch) });
}

/// A sentence typed and sent while the surface showed `question`, for the account epoch it showed.
fn typed(n: &Shared, text: &str, question: Option<u64>, epoch: u64) {
    showing(n, question, epoch);
    n.command(Command::Ask { text: text.into(), stamp: stamp(question, epoch) });
}

/// The window's Talk pressed while it showed `question`.
fn talk_pressed(n: &Shared, question: Option<u64>, epoch: u64) {
    showing(n, question, epoch);
    n.app_talk(true, false, stamp(question, epoch), None);
}

/// The account epoch the windows are shown now.
fn shown_epoch(t: &TestHost) -> u64 {
    t.view().account_epoch
}

/// Another Orbiters member is signed in (as the controller's identity gate does it).
fn switch_member(t: &TestHost, n: &Shared, base: String, token: &str) {
    t.switch_member(n, Some((base, token)));
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
    let_go(&n);
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
    let_go(&n);
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

// --- What a message without its stamp is, on the wire's own bytes ---

/// Verify2-r21 design 2: an older process' messages, fields that default, and numbers that are nobody's: none of them
/// answers, is sent or starts listening, and what is said of it repeats nothing of it.
#[test]
fn unstamped_and_default_valued_messages_do_nothing() {
    let (t, n, device) = ready();
    model_asks(&t, &n, "Sam", "orbit-a");
    let question = question_shown(&t).unwrap();
    let epoch = shown_epoch(&t);
    let (base, bodies) = orbiters(vec![]);
    t.sign_in(base, "orbit-a");
    let zero = json!({ "epoch": 0, "question": question, "surface": "window", "seq": 1 });
    let messages = [
        // As 0.3.9-dev.26 wrote them.
        json!({ "kind": "ask", "text": "SECRET words", "question": question }),
        json!({ "kind": "ask", "text": "yes", "question": question }),
        json!({ "kind": "ask_answer", "yes": true, "question": question }),
        json!({ "kind": "ask_answer", "yes": true }),
        json!({ "kind": "talk", "down": true }),
        json!({ "kind": "ask_talk", "down": true }),
        json!({ "kind": "retry_turn", "id": 1 }),
        // A stamp that is nobody's.
        json!({ "kind": "ask", "text": "SECRET words", "stamp": null }),
        json!({ "kind": "ask", "text": "yes", "stamp": zero }),
        json!({ "kind": "ask_answer", "yes": true, "stamp": zero }),
        json!({ "kind": "talk", "down": true, "stamp": zero }),
        // The right member, with no question, or with none that is real.
        json!({ "kind": "ask_answer", "yes": true, "stamp": { "epoch": epoch, "surface": "overlay", "seq": 2 } }),
        json!({ "kind": "ask_answer", "yes": true, "stamp": { "epoch": epoch, "question": 0, "surface": "overlay", "seq": 3 } }),
        json!({ "kind": "ask", "text": "yes", "stamp": { "epoch": epoch, "surface": "window", "seq": 4 } }),
    ];
    for message in messages {
        let bytes = serde_json::to_vec(&message).unwrap();
        let command: Command = serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{message}: {e}"));
        n.command(command);
        assert!(t.app_intents.lock().unwrap().is_empty(), "{message}");
        assert_eq!(question_shown(&t), Some(question), "{message}: the question still waits");
        assert!(n.n4n0.lock().unwrap().listening.is_none() && device.ears.workers.lock().unwrap().is_empty(), "{message}");
        assert!(bodies.lock().unwrap().is_empty(), "{message}: nothing goes out");
        assert!(!t.view().lines.iter().any(|l| l.text.contains("SECRET")), "{message}: {:?}", t.view().lines);
    }
    // A stamp whose epoch is missing altogether is no stamp: the message does not parse as one.
    assert!(serde_json::from_value::<Command>(json!({ "kind": "ask", "text": "x", "stamp": { "question": question, "surface": "window", "seq": 5 } })).is_err());
    // What the windows are shown before anything was published is 0: refused as above.
    assert_eq!(N4n0Status::default().account_epoch, 0);
    assert_ne!(crate::FIRST_EPOCH, 0);
    // The bytes a surface writes carry the stamp whole.
    let written = serde_json::to_value(Command::AskAnswer { yes: true, stamp: stamp(Some(question), epoch) }).unwrap();
    assert_eq!((written["stamp"]["epoch"].as_u64(), written["stamp"]["question"].as_u64()), (Some(epoch), Some(question)));
    t.shutdown(&n);
}

/// A press delivered twice (the same number) acts once.
#[test]
fn an_input_delivered_twice_acts_once() {
    let (t, n, _device) = ready();
    n.asked("turn on fast walk".into(), "typed");
    let logged = t.view().assistant.log.len();
    let again = Command::Ask { text: "turn off fast walk".into(), stamp: stamp(None, shown_epoch(&t)) };
    n.command(again.clone());
    n.command(again);
    assert_eq!(t.view().assistant.log.len(), logged + 1);
    t.shutdown(&n);
}

/// A yes said aloud: for the question that was up when Talk was pressed, and none that came since.
#[test]
fn a_spoken_yes_is_for_the_question_up_at_the_press() {
    let (t, n, _device) = ready();
    let epoch = shown_epoch(&t);
    model_asks(&t, &n, "Sam", "orbit-a");
    let a = question_shown(&t).unwrap();
    // Pressed on A; A is answered elsewhere and B asked before the recognition ends.
    talk_pressed(&n, Some(a), epoch);
    assert!(n.n4n0.lock().unwrap().listening.is_some(), "a question may be answered aloud");
    let_go(&n);
    press_answer(&n, false, Some(a), epoch);
    model_asks(&t, &n, "Mira", "orbit-a");
    let b = question_shown(&t).unwrap();
    heard(&n, 1, "yes");
    assert!(t.app_intents.lock().unwrap().is_empty(), "{:?}", t.app_intents.lock().unwrap());
    assert_eq!(question_shown(&t), Some(b));
    let said = t.view().lines.last().unwrap().text.clone();
    assert!(said.contains("asked something else") && said.contains("Mira"), "the current question is said: {said}");
    // Pressed on B, said yes: B.
    talk_pressed(&n, Some(b), epoch);
    let_go(&n);
    heard(&n, 2, "yes");
    assert_eq!(*t.app_intents.lock().unwrap(), vec![Intent::Boop { friend: Some("Mira".into()) }]);
    // Its question gone, a late yes is for nothing, and not a message either.
    let (base, bodies) = orbiters(vec![]);
    t.sign_in(base, "orbit-a");
    typed(&n, "yes", Some(b), epoch);
    std::thread::sleep(Duration::from_millis(100));
    assert!(bodies.lock().unwrap().is_empty());
    t.shutdown(&n);
}

// --- Everything else a turn or an input starts, after the member changed ---

/// Streamed words, a tool's note, an approval, a quota and a tool call arriving as they come (server-sent events) after B
/// took over.
#[test]
fn a_late_stream_for_a_shows_nothing_after_b_took_over() {
    let (t, n, _device) = ready();
    let go = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let events = format!(
        "event: text\ndata: {}\n\nevent: tool\ndata: {}\n\nevent: confirmation\ndata: {}\n\n{}",
        json!({ "text": "A's private answer" }),
        json!({ "name": "create_event_draft", "phase": "running" }),
        json!({ "id": "c1", "title": "A's movie night", "summary": "", "approve_url": "https://orbiters.cc/a" }),
        done(json!([{ "role": "user", "content": "x" }]), json!([call("w1", "orbiters_walk", json!({ "intent": "fast_walk", "on": true }))]))
    );
    let (base, bodies) = held_orbiters(vec![events], go.clone(), false);
    t.sign_in(base, "orbit-a");
    typed(&n, "plan my movie night", None, shown_epoch(&t));
    until("A's request", || !bodies.lock().unwrap().is_empty());
    let (base_b, _) = orbiters(vec![]);
    switch_member(&t, &n, base_b, "orbit-b");
    let notices = t.notices().len();
    go.store(true, Ordering::SeqCst);
    std::thread::sleep(Duration::from_millis(600));
    let view = t.view();
    assert!(view.lines.is_empty() && view.confirmations.is_empty() && !view.thinking, "{:?} {:?}", view.lines, view.confirmations);
    assert!(!t.settings().walk_boost.enabled);
    assert_eq!(t.notices().len(), notices, "no toast");
    assert!(n.n4n0.lock().unwrap().conversation.is_empty());
    t.shutdown(&n);
}

/// The handover waits for a side effect under way, and lets none in: a tool running when B signs in ends for A, and the
/// next one of the same turn never starts.
#[test]
fn the_handover_waits_for_what_acts_and_nothing_acts_after_it() {
    let (t, n, _device) = ready();
    let (base, _) = orbiters(vec![]);
    t.sign_in(base, "orbit-a");
    let acting = n.let_in(stamp(None, shown_epoch(&t))).unwrap().0;
    let later = acting.later();
    let (t2, n2) = (t.clone(), n.clone());
    let switching = std::thread::spawn(move || t2.switch_member(&n2, None));
    std::thread::sleep(Duration::from_millis(150));
    assert!(!switching.is_finished(), "it waits for what acts");
    // What acts now may start more of its own (a tool's second step), even with the handover waiting.
    assert!(later.fence().is_some());
    drop(acting);
    switching.join().unwrap();
    assert!(later.fence().is_none(), "nothing for the member before");
    assert!(n.let_in(stamp(None, shown_epoch(&t))).is_some(), "and the next one's inputs are let in");
    t.shutdown(&n);
}

/// Deferred work of A's, each started under A and due after B took over: the share of the next capture, a message to be
/// sent again by itself, a retry pressed on A's failure, a 3D picture's timer, a question waiting, the sentence heard.
#[test]
fn what_a_left_waiting_does_nothing_once_b_took_over() {
    let (t, n, _device) = ready();
    let epoch_a = shown_epoch(&t);
    // A message that failed and waits to be sent again by itself.
    let busy = "HTTP 429";
    let (base, bodies_a) = orbiters(vec![]);
    let _ = busy;
    t.sign_in(base, "orbit-a");
    {
        let mut rt = n.n4n0.lock().unwrap();
        rt.assistant.retry = Some(crate::assistant::Retrying { id: 5, text: "A's private retry".into(), via: "typed", not_before: Instant::now(), auto_used: false });
        rt.assistant.view.failure = Some(crate::TurnFailure { id: 5, retry: true, ..Default::default() });
        rt.screen.photo_at = Some(Instant::now() + Duration::from_millis(150));
        rt.send_at(Instant::now() + Duration::from_secs(20), [("/usercamera/Capture".to_string(), OscType::Bool(true))]);
    }
    n.acts().intent_run(&crate::fence::Permit::for_test(), Intent::ShareCapture { which: orbiters_n4n0_core::intents::Which::Next, public: true, friends: vec![] }, orbiters_n4n0_core::intents::Via::Model).unwrap();
    let off = n.acts().later();
    let (base_b, bodies_b) = orbiters(vec![]);
    switch_member(&t, &n, base_b, "orbit-b");
    // The share's capture is ready; the retries come due; the timer runs out; a thread A's tool started comes to act.
    n.share_next_check();
    n.command(Command::RetryTurn { id: 5, stamp: stamp(None, epoch_a) });
    n.command(Command::RetryTurn { id: 5, stamp: stamp(None, shown_epoch(&t)) });
    assert!(off.fence().is_none(), "AssistantOff's thread");
    std::thread::sleep(Duration::from_millis(400));
    assert!(t.shares.lock().unwrap().is_empty(), "nothing of A's is shared by B");
    assert!(bodies_a.lock().unwrap().is_empty() && bodies_b.lock().unwrap().is_empty(), "nothing is sent again");
    assert_eq!(t.photos.load(Ordering::SeqCst), 0, "no picture of A's timer");
    assert!(!n.n4n0.lock().unwrap().outbox.iter().any(|(_, (a, _))| a == "/usercamera/Capture"), "nor VRChat's picture after its delay");
    assert!(t.view().assistant.failure.is_none());
    t.shutdown(&n);
}

/// The same member's credential refreshed (no handover): everything goes on.
#[test]
fn a_refreshed_credential_keeps_what_is_under_way() {
    let (t, n, _device) = ready();
    model_asks(&t, &n, "Sam", "orbit-a");
    let (question, epoch) = (question_shown(&t).unwrap(), shown_epoch(&t));
    let (base, _) = orbiters(vec![]);
    t.sign_in(base, "orbit-a-refreshed");
    press_answer(&n, true, Some(question), epoch);
    assert_eq!(*t.app_intents.lock().unwrap(), vec![Intent::Boop { friend: Some("Sam".into()) }]);
    t.shutdown(&n);
}

// --- The guard is complete: what the types cannot hold, read in the source ---

/// The source of the function (or block) that starts at `head`, to its closing brace at the same indentation.
fn body<'a>(source: &'a str, head: &str) -> &'a str {
    let at = source.find(head).unwrap_or_else(|| panic!("{head}"));
    let indent = source[..at].rfind('\n').map_or(0, |n| at - n - 1);
    let close = format!("\n{}}}\n", " ".repeat(indent));
    let end = source[at..].find(&close).map_or(source.len(), |e| at + e + close.len());
    &source[at..end]
}

/// A turn is given an actor and no N4N0: nothing in it names the runtime but through a fence; and every function that acts
/// for the assistant is the fence's own.
#[test]
fn every_side_effect_of_the_assistant_is_behind_the_fence() {
    let read = |name: &str| std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name)).unwrap().replace("\r\n", "\n");
    let (runtime, assistant, fence) = (read("runtime.rs"), read("assistant.rs"), read("fence.rs"));
    // The turn's thread: its only way to N4N0 is `actor.fence()`.
    let turn = body(&runtime, "fn turn(actor: Actor,");
    for banned in ["self.", "Shared", "N4n0>", "Arc::clone", ".shared"] {
        assert!(!turn.contains(banned), "the turn reaches N4N0 with {banned}");
    }
    assert!(turn.matches("actor.fence()").count() >= 6, "a fence per step");
    // The actor hands out fences only.
    let actor = body(&fence, "impl Actor {");
    assert_eq!(actor.matches("fn ").count(), 1, "{actor}");
    assert!(fence.contains("pub(crate) struct Actor {\n    shared: Arc<N4n0>,"), "its N4N0 is private to fence.rs");
    // What acts for the assistant is `impl Fence` and is not on N4N0, where anything could call it.
    let on_fence = |source: &str| {
        let mut out = String::new();
        let mut rest = source;
        while let Some(at) = rest.find("impl Fence<'_> {") {
            let block = body(&rest[at..], "impl Fence<'_> {");
            out.push_str(block);
            rest = &rest[at + block.len()..];
        }
        out
    };
    let (acts_runtime, acts_assistant) = (on_fence(&runtime), on_fence(&assistant));
    let acting = ["ask", "start_turn", "retry_turn", "ask_owner", "stream_answer", "website_approval", "answer_with", "confirmed", "run_tool", "run_intent", "run_action_for_assistant", "action"];
    for name in acting {
        assert!(acts_runtime.contains(&format!("fn {name}(&self")), "{name} is the fence's");
        if name != "ask" {
            assert_eq!(runtime.matches(&format!("fn {name}(")).count(), 1, "{name} is nowhere else");
        }
    }
    for name in ["intent_log", "log_line", "intent_doing", "intent_run", "keep_list", "intent_do", "open_page", "open_instead", "quick"] {
        assert!(acts_assistant.contains(&format!("fn {name}(&self")), "{name} is the fence's");
        assert_eq!(assistant.matches(&format!("fn {name}(")).count(), 1, "{name} is nowhere else");
    }
    // What they start on another thread goes through `later()`: never a plain copy of N4N0.
    for (name, source) in [("runtime.rs", &acts_runtime), ("assistant.rs", &acts_assistant)] {
        for at in source.match_indices("spawn(").map(|(i, _)| i) {
            let around = &source[at.saturating_sub(400)..(at + 60).min(source.len())];
            assert!(around.contains("later()") || around.contains("turn_actor("), "{name}: a thread without a fence of its own: {around}");
        }
        assert!(!source.contains("self.clone()") && !source.contains("Arc::clone"), "{name}");
    }
    // The state a turn writes back is written there only.
    for (written, times) in [("rt.conversation = ", 1), (".quota = done.quota", 1), ("rt.pending = ", 2)] {
        let within = turn.matches(written).count() + acts_runtime.matches(written).count();
        assert_eq!((runtime.matches(written).count(), within), (times, times), "{written}");
    }
    // Inputs are admitted at one gate, and an answer is compared with its question at one place.
    assert_eq!(fence.matches("fn admitted(").count(), 1);
    assert_eq!(runtime.matches("fn answer_with(").count(), 1);
    // Work a menu or a tool starts as the member asks for the account of its own epoch, never for whoever is signed in.
    for name in ["runtime.rs", "assistant.rs", "screen.rs", "captures.rs", "feedback.rs", "conversation.rs", "voice.rs"] {
        let source = read(name);
        for (n, line) in source.lines().enumerate().filter(|(_, l)| l.contains("host.account()")) {
            assert!(line.contains("Server::current()"), "{name}:{}: {line}", n + 1);
        }
    }
}

#[path = "tests_fix3.rs"]
mod fix3;

#[path = "tests_1124.rs"]
mod review_1124;
