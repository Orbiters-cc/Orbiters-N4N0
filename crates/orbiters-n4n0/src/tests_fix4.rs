//! Verify4-r21: what the assistant may do by itself, what it is shown, what a yes does, and what a handover keeps and ends;
//! driven through the real command handler and the real turn, with only the model (a stand-in Orbiters) faked.

use super::*;
use orbiters_n4n0_core::captures::{CapturePhase, CaptureRow, CapturesView};

/// A turn of `text` in which the model makes `calls`, to its end. The bodies of its requests.
fn turn(t: &TestHost, n: &Shared, text: &str, calls: Value) -> Arc<Mutex<Vec<Value>>> {
    let (base, bodies) = orbiters(vec![done(json!([{ "role": "user", "content": "x" }]), calls), done(json!([]), json!([]))]);
    t.sign_in(base, "orbit-a");
    n.asked(text.into(), "typed");
    turn_ended(n, &bodies, 2);
    bodies
}

fn off(t: &TestHost) {
    *t.app_facts.lock().unwrap() = Some(Facts { actions: false, ..full() });
}

fn instances() -> Value {
    json!([call("a", "orbiters_instances", json!({ "intent": "instances" })), call("b", "orbiters_instances", json!({ "intent": "invite_me", "index": 1 }))])
}

// --- P2: calls ---

/// Without the switch, N4N0 hangs up, declines and unmutes nothing (the Calls page opens); with it, each waits for the owner's
/// yes; muting is done at once, for everyone.
#[test]
fn the_assistant_never_ends_or_unmutes_a_call_by_itself() {
    let end = || json!([call("a", "orbiters_calls", json!({ "intent": "end_call" })), call("b", "orbiters_calls", json!({ "intent": "mute_call", "on": false }))]);
    let (t, n, _device) = ready();
    *t.in_call.lock().unwrap() = true;
    off(&t);
    turn(&t, &n, "the call with my friend, please do that thing", end());
    assert!(t.calls.lock().unwrap().is_empty() && question_shown(&t).is_none(), "{:?}", t.calls.lock().unwrap());
    turn(&t, &n, "the call with my friend, quieter", json!([call("a", "orbiters_calls", json!({ "intent": "mute_call", "on": true }))]));
    assert_eq!(*t.calls.lock().unwrap(), [CallRequest::Mute { call: 7, on: true }]);
    t.shutdown(&n);
    let (t, n, _device) = ready();
    *t.in_call.lock().unwrap() = true;
    turn(&t, &n, "the call with my friend, please do that thing", end());
    assert!(t.calls.lock().unwrap().is_empty());
    press_answer(&n, true, question_shown(&t), shown_epoch(&t));
    assert_eq!(*t.calls.lock().unwrap(), [CallRequest::End(7), CallRequest::Mute { call: 7, on: false }]);
    t.shutdown(&n);
}

/// A button's Hang up, Mute or Answer is for the call and the member it showed: naming none, or another member's, it does
/// nothing. (Another call's number is the app's to refuse: controller/calls.rs.)
#[test]
fn a_calls_button_names_its_call_and_its_member() {
    let (t, n, _device) = ready();
    *t.in_call.lock().unwrap() = true;
    // As an older window sends them.
    for m in [json!({ "kind": "end_call" }), json!({ "kind": "mute_call", "on": false }), json!({ "kind": "answer_call" })] {
        n.command(serde_json::from_value(m).unwrap());
    }
    let epoch = shown_epoch(&t);
    let buttons = |call: Option<u64>, stamp: Option<Stamp>| [Command::EndCall { call, stamp }, Command::MuteCall { on: false, call, stamp }, Command::AnswerCall { call, stamp }];
    for (call, stamp) in [(None, at(Surface::Window, None, epoch)), (Some(7), at(Surface::Window, None, epoch + 1)), (Some(7), None)] {
        for command in buttons(call, stamp) {
            n.command(command);
        }
    }
    std::thread::sleep(Duration::from_millis(50));
    assert!(t.calls.lock().unwrap().is_empty(), "{:?}", t.calls.lock().unwrap());
    // Captured under A, delivered once B is signed in.
    let captured = buttons(Some(7), at(Surface::Window, None, epoch));
    t.switch_member(&n, None);
    for command in captured {
        n.command(command);
    }
    std::thread::sleep(Duration::from_millis(50));
    assert!(t.calls.lock().unwrap().is_empty(), "{:?}", t.calls.lock().unwrap());
    let epoch = shown_epoch(&t);
    for mut command in buttons(Some(7), None) {
        *command.stamp_mut().unwrap() = at(Surface::Window, None, epoch);
        n.command(command);
    }
    until("the calls", || t.calls.lock().unwrap().len() == 3);
    t.shutdown(&n);
}

// --- P4: what the model reads ---

/// A request only to act carries no words of other people, and acts; one that asks about them carries them, and waits for a yes.
#[test]
fn others_text_goes_only_with_what_asks_for_it_and_taints_it() {
    let (t, n, _device) = ready();
    t.world(|w| {
        w.in_world = true;
        w.world_name = Some("INJECTED WORLD".into());
    });
    let walk = || json!([call("w", "orbiters_walk", json!({ "intent": "fast_walk", "on": true }))]);
    let bodies = turn(&t, &n, "make me walk faster over the hills", walk());
    assert!(!bodies.lock().unwrap()[0].to_string().contains("INJECTED"), "{}", bodies.lock().unwrap()[0]);
    assert!(t.settings().walk_boost.enabled && question_shown(&t).is_none(), "nothing of others' was read");
    t.settings.lock().unwrap().walk_boost.enabled = false;
    let bodies = turn(&t, &n, "what is this world called, and go faster", walk());
    assert!(bodies.lock().unwrap()[0].to_string().contains("INJECTED"));
    assert!(!t.settings().walk_boost.enabled && question_shown(&t).is_some(), "read: asked first");
    t.shutdown(&n);
}

/// A control the owner names by its label goes to the model as a number, with the owner's own words: done at once, and the turn acts
/// on. Named otherwise, the avatar's labels (its maker's) are read: it waits for a yes.
#[test]
fn a_control_named_by_the_owner_goes_by_its_number() {
    let (t, n, vrchat) = n4n0_with_vrchat();
    *t.app_facts.lock().unwrap() = Some(full());
    n.publish();
    let calls = |control: &str| json!([call("c", "set_avatar_control", json!({ "control": control, "on": true })), call("w", "orbiters_walk", json!({ "intent": "fast_walk", "on": true }))]);
    received(&vrchat, Duration::from_millis(300));
    let bodies = turn(&t, &n, "put my hoodie on and make me walk fast over the hills", calls("#1"));
    let sent = received(&vrchat, Duration::from_millis(500));
    assert!(sent.contains(&(parameter("Hoodie"), OscType::Bool(true))), "{sent:?}");
    assert!(t.settings().walk_boost.enabled && question_shown(&t).is_none());
    let first = bodies.lock().unwrap()[0]["context"]["summary"].as_str().unwrap().to_string();
    assert!(first.contains("#1") && first.contains("hoodie") && !first.contains("Hoodie"), "{first}");
    assert!(!bodies.lock().unwrap()[1]["tool_results"].to_string().contains("Hoodie"));
    t.settings.lock().unwrap().walk_boost.enabled = false;
    let bodies = turn(&t, &n, "my avatar, put the warm top on and walk fast over the hills", calls("Hoodie"));
    assert!(bodies.lock().unwrap()[0]["context"]["summary"].as_str().unwrap().contains("Hoodie"));
    assert!(!t.settings().walk_boost.enabled && question_shown(&t).is_some());
    t.shutdown(&n);
}

/// What Orbiters' own tool read on its side is others' text too.
#[test]
fn a_server_side_read_taints_the_turn() {
    let (t, n, _device) = ready();
    let first = format!(
        "event: tool\ndata: {}\n\n{}",
        json!({ "name": "search_orbiters", "phase": "running" }),
        done(json!([{ "role": "user", "content": "x" }]), json!([call("w", "orbiters_walk", json!({ "intent": "fast_walk", "on": true }))]))
    );
    let (base, bodies) = orbiters(vec![first, done(json!([]), json!([]))]);
    t.sign_in(base, "orbit-a");
    n.asked("look at the events of this week for me".into(), "typed");
    turn_ended(&n, &bodies, 2);
    assert!(!t.settings().walk_boost.enabled && question_shown(&t).is_some());
    t.shutdown(&n);
}

/// What was said where others' text was read is left out of a request only to act (which acts), and goes, with its mark, with one
/// that points back at it.
#[test]
fn earlier_messages_keep_their_mark() {
    let (t, n, _device) = ready();
    let read = json!([{ "role": "user", "content": "x" }, { "role": "assistant", "content": "INJECTED ANSWER" }]);
    let (base, bodies) = orbiters(vec![
        done(json!([{ "role": "user", "content": "x" }]), json!([call("w", "orbiters_worlds", json!({ "intent": "where_am_i" }))])),
        done(read.clone(), json!([])),
    ]);
    t.sign_in(base, "orbit-a");
    n.asked("tell me about the world I am in".into(), "typed");
    turn_ended(&n, &bodies, 2);
    assert!(!n.n4n0.lock().unwrap().conversation.is_empty());
    let walk = || json!([call("w", "orbiters_walk", json!({ "intent": "fast_walk", "on": true }))]);
    let bodies = turn(&t, &n, "make me walk faster over the hills", walk());
    assert!(!bodies.lock().unwrap()[0].to_string().contains("INJECTED"), "{}", bodies.lock().unwrap()[0]);
    assert!(t.settings().walk_boost.enabled && question_shown(&t).is_none());
    assert!(n.n4n0.lock().unwrap().conversation.iter().any(|m| m.to_string().contains("INJECTED")), "left out, not lost");
    t.settings.lock().unwrap().walk_boost.enabled = false;
    let bodies = turn(&t, &n, "do what you said earlier about it", walk());
    assert!(bodies.lock().unwrap()[0].to_string().contains("INJECTED"));
    assert!(!t.settings().walk_boost.enabled && question_shown(&t).is_some());
    t.shutdown(&n);
}

/// Opening the page instead is an action like another: it passes the fence, and the log has it, after others' text too.
#[test]
fn the_page_opened_instead_is_logged() {
    for text in ["could you maybe make my status the orange one", "what is this world called, and make my status the orange one"] {
        let (t, n, _device) = ready();
        off(&t);
        turn(&t, &n, text, json!([call("a", "orbiters_profile", json!({ "intent": "set_status", "status": "ask_me" }))]));
        assert!(t.app_intents.lock().unwrap().iter().all(|i| matches!(i, Intent::OpenPage { .. })), "{text}: {:?}", t.app_intents.lock().unwrap());
        assert!(t.view().assistant.log.iter().any(|l| l.intent == "open_page" && l.outcome == Outcome::Done), "{text}: {:?}", t.view().assistant.log);
        t.shutdown(&n);
    }
    let source = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/assistant.rs")).unwrap().replace("\r\n", "\n");
    assert!(body(&source, "fn open_instead(").contains("permit("), "through the fence");
}

// --- P6: a yes does what was read ---

/// What is applied is what the question showed, to its last character.
#[test]
fn what_is_applied_is_what_was_shown() {
    let (t, n, _device) = ready();
    let text = format!("Back soon {} LAST WORDS\u{202E}\u{200B}", "x".repeat(130));
    turn(&t, &n, "write something nice for while I am away", json!([call("a", "set_afk_message", json!({ "text": text, "enabled": true }))]));
    let asked = t.view().confirmations.last().unwrap().summary.clone();
    press_answer(&n, true, question_shown(&t), shown_epoch(&t));
    let applied = t.settings().n4n0.afk_message;
    assert!(!applied.is_empty() && asked.contains(&applied), "applied {applied:?}, asked {asked:?}");
    assert!(!applied.contains(['\u{202E}', '\u{200B}']), "nothing in it that does not show");
    t.shutdown(&n);
    // A bio, whole.
    let (t, n, _device) = ready();
    let bio = format!("{} THE END", "word ".repeat(60).trim());
    turn(&t, &n, "write my bio", json!([call("a", "orbiters_profile", json!({ "intent": "set_bio", "text": bio }))]));
    assert!(t.view().confirmations.last().is_some_and(|c| c.summary.contains(&bio)), "{:?}", t.view().confirmations);
    press_answer(&n, true, question_shown(&t), shown_epoch(&t));
    assert_eq!(*t.app_intents.lock().unwrap(), [Intent::SetBio { text: bio }]);
    t.shutdown(&n);
}

/// The avatar's panel presents a question only when its letters write all of it.
#[test]
fn the_panel_presents_nothing_it_cannot_write() {
    let (t, n, _vrchat) = n4n0_with_vrchat();
    *t.app_facts.lock().unwrap() = Some(full());
    n.publish();
    model_asks(&t, &n, "さくら", "orbit-a");
    let question = question_shown(&t).unwrap();
    until("the panel shows what it can", || n.n4n0.lock().unwrap().sender.as_ref().is_some_and(orbiters_n4n0_core::text::TextSender::delivered));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(n.own_stamp(Surface::Avatar).question, None);
    present(&n, question, Surface::Window);
    press_answer(&n, true, Some(question), shown_epoch(&t));
    assert_eq!(boops(&t), 1, "answered where it is whole");
    t.shutdown(&n);
}

/// An answer from where no question shows (SteamVR's dashboard) resolves nothing and says where it can be answered.
#[test]
fn an_answer_from_where_no_question_shows_says_where_to() {
    let (t, n, _device) = ready();
    model_asks(&t, &n, "Sam", "orbit-a");
    let question = question_shown(&t);
    n.app_talk(true, true, at(Surface::Overlay, None, shown_epoch(&t)), None);
    let_go_overlay(&n);
    heard(&n, 1, "yes");
    assert_eq!((boops(&t), question_shown(&t)), (0, question));
    let said = t.view().lines.last().unwrap().text.clone();
    assert!(said.contains("was not shown there") && said.contains("chat") && said.contains("Sam"), "{said}");
    t.shutdown(&n);
}

fn let_go_overlay(n: &Shared) {
    let press = n.n4n0.lock().unwrap().voice.press;
    n.app_talk(false, true, None, press);
}

// --- P7: what is not undone in one step ---

/// Each waits for a yes, for everyone; the yes does it.
#[test]
fn what_is_not_undone_in_one_step_waits_for_a_yes() {
    for (tool, input) in [
        ("orbiters_pictures", json!({ "intent": "stop_recording" })),
        ("orbiters_videos", json!({ "intent": "take_back_card" })),
        ("copy_to_clipboard", json!({ "text": "replaced by N4N0" })),
    ] {
        for actions in [false, true] {
            let (t, n, _device) = ready();
            *t.app_facts.lock().unwrap() = Some(Facts { actions, ..full() });
            turn(&t, &n, "that camera and video and clipboard thing, you know what to do", json!([call("a", tool, input.clone())]));
            assert!(question_shown(&t).is_some() && t.app_intents.lock().unwrap().is_empty(), "{tool} {input}");
            t.shutdown(&n);
        }
    }
    for name in ["stop_recording", "take_back_card", "apply_state"] {
        assert_eq!(CATALOGUE.iter().find(|s| s.name == name).unwrap().risk, Risk::Ask, "{name}");
    }
}

/// A copy is done at once where what it replaces is put back by Undo, and asked about where it could not be.
#[test]
fn a_copy_is_undone_or_asked_about() {
    let (t, n, _device) = ready();
    t.app_answers.lock().unwrap().insert("world_search", json!({ "items": [{ "ref": "wrld_a", "name": "Copied by the first test" }] }));
    let copy = |t: &TestHost, n: &Shared| {
        n.acts().intent_run(&Permit::for_test(), Intent::WorldSearch { query: "x".into() }, Via::Model).unwrap();
        turn(t, n, "put the name of the first world where I can paste it", json!([call("a", "orbiters_worlds", json!({ "intent": "copy_world_name", "index": 1 }))]))
    };
    copy(&t, &n);
    assert!(CLIPBOARD.lock().unwrap().iter().any(|c| c == "Copied by the first test") && question_shown(&t).is_none());
    turn(&t, &n, "take that back", json!([call("u", "orbiters_state", json!({ "intent": "undo" }))]));
    assert!(CLIPBOARD.lock().unwrap().iter().any(|c| c == "what was there before"), "put back");
    t.shutdown(&n);
}

// --- P8: the same member's history ---

/// Not known for a while is not a sign-out: what was said waits for the same member, and for nobody else.
#[test]
fn what_was_said_comes_back_to_its_member_only() {
    let said = || {
        let (t, n, _device) = ready();
        let (base, bodies) = orbiters(vec![done(json!([{ "role": "user", "content": "hi" }, { "role": "assistant", "content": "Hello there." }]), boop("Sam")), done(json!([{ "role": "user", "content": "hi" }, { "role": "assistant", "content": "Hello there." }]), json!([]))]);
        t.sign_in(base, "orbit-a");
        n.asked("say something about the sea".into(), "typed");
        turn_ended(&n, &bodies, 2);
        assert!(question_shown(&t).is_some());
        let epoch = shown_epoch(&t);
        n.pause_effects().park_account(epoch + 1, Some("production/1"));
        {
            let rt = n.n4n0.lock().unwrap();
            assert!(rt.conversation.is_empty() && rt.question().is_none() && !rt.lines.iter().any(|l| l.text.contains("the sea")), "nobody's meanwhile");
        }
        (t, n, epoch + 1)
    };
    let spoken = |n: &Shared| {
        let rt = n.n4n0.lock().unwrap();
        !rt.conversation.is_empty() && rt.lines.iter().any(|l| l.text.contains("the sea"))
    };
    // The same member, after one or two unknown credentials in between.
    let (t, n, epoch) = said();
    n.pause_effects().park_account(epoch + 1, None);
    n.pause_effects().establish_account(epoch + 2, "production/1");
    assert!(spoken(&n) && n.n4n0.lock().unwrap().question().is_none(), "what it asked is not brought back");
    assert!(n.n4n0.lock().unwrap().account_epoch == epoch + 2);
    t.shutdown(&n);
    // Another member, another environment, a sign-out: gone, also for the first one after.
    for other in [Some("production/2"), Some("development/1"), None] {
        let (t, n, epoch) = said();
        match other {
            Some(other) => n.pause_effects().establish_account(epoch + 1, other),
            None => n.pause_effects().forget_account(epoch + 1),
        }
        assert!(!spoken(&n), "{other:?}");
        n.pause_effects().establish_account(epoch + 2, "production/1");
        assert!(!spoken(&n), "{other:?}, then the first one again");
        t.shutdown(&n);
    }
}

// --- P9: a release ends its own press ---

/// A release that names no press, or another one, ends nothing: not the page's own listening, nor another press'.
#[test]
fn a_release_ends_only_its_own_press() {
    let (t, n, _device) = ready();
    let (base, _) = orbiters(vec![]);
    t.sign_in(base, "orbit-a");
    n.ask_page(true);
    for press in [None, Some(1)] {
        n.command(Command::AskTalk { down: false, stamp: None, press });
        let rt = n.n4n0.lock().unwrap();
        assert!(rt.voice.automatic && rt.listening.is_some(), "a stale release ended the page's listening");
    }
    let stamp = at(Surface::Overlay, None, shown_epoch(&t));
    n.command(Command::AskTalk { down: true, stamp, press: None });
    assert_eq!(t.view().voice.press, stamp.map(|s| s.seq));
    for press in [None, stamp.map(|s| s.seq + 1)] {
        n.command(Command::AskTalk { down: false, stamp: None, press });
        n.command(Command::Talk { down: false, stamp: None, press: stamp.map(|s| s.seq) });
        assert!(n.n4n0.lock().unwrap().listening.is_some(), "{press:?}, or another button's release");
    }
    n.command(Command::AskTalk { down: false, stamp: None, press: stamp.map(|s| s.seq) });
    assert!(n.n4n0.lock().unwrap().listening.is_none(), "its own");
    t.shutdown(&n);
}

// --- P10: a handover and a request about to go ---

/// A handover waits for a request already admitted to be with Orbiters, not for its answer; none is sent after one.
#[test]
fn a_request_is_sent_before_a_handover_or_not_at_all() {
    let (t, n, _device) = ready();
    let go = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (base, bodies) = held_orbiters(vec![done(json!([]), json!([]))], go.clone(), false);
    t.sign_in(base, "orbit-a");
    // The handover asked for as the request is about to go.
    let (t2, n2) = (t.clone(), n.clone());
    let sent_then = Arc::new(Mutex::new(None));
    let (seen, at_handover) = (bodies.clone(), sent_then.clone());
    let switching = Arc::new(Mutex::new(None));
    let handle = switching.clone();
    *n.before_send.lock().unwrap() = Some(Box::new(move || {
        *handle.lock().unwrap() = Some(std::thread::spawn(move || {
            t2.switch_member(&n2, None);
            *at_handover.lock().unwrap() = Some(seen.lock().unwrap().len());
        }));
        std::thread::sleep(Duration::from_millis(150));
    }));
    n.asked("say something about the sea".into(), "typed");
    until("the handover", || sent_then.lock().unwrap().is_some());
    assert!(!go.load(Ordering::SeqCst), "not once the answer was read");
    assert_eq!(*sent_then.lock().unwrap(), Some(1), "with Orbiters before the handover");
    go.store(true, Ordering::SeqCst);
    switching.lock().unwrap().take().unwrap().join().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(bodies.lock().unwrap().len(), 1);
    assert!(!t.view().lines.iter().any(|l| l.text.contains("the sea")));
    t.shutdown(&n);
}

// --- P11: the switch off ---

/// A share armed for the next capture, and a question of the account's, go as the switch does; a later capture shares nothing.
#[test]
fn what_waited_goes_with_the_switch() {
    let (t, n, _device) = ready();
    n.acts().intent_run(&Permit::for_test(), Intent::ShareCapture { which: intents::Which::Next, public: true, friends: vec![] }, Via::Model).unwrap();
    model_asks(&t, &n, "Sam", "orbit-a");
    assert!(question_shown(&t).is_some());
    n.account_actions_changed();
    assert!(question_shown(&t).is_some(), "still on: nothing goes");
    off(&t);
    n.account_actions_changed();
    assert!(question_shown(&t).is_none());
    assert!(t.view().lines.iter().any(|l| l.text.contains("not shared after all")), "{:?}", t.view().lines);
    // On again: what was dropped is not armed again.
    *t.app_facts.lock().unwrap() = Some(full());
    n.set_captures(CapturesView { latest: Some(CaptureRow { key: "new".into(), phase: CapturePhase::Ready, ..Default::default() }), ..Default::default() });
    std::thread::sleep(Duration::from_millis(200));
    n.share_next_check();
    std::thread::sleep(Duration::from_millis(100));
    assert!(t.shares.lock().unwrap().is_empty());
    t.shutdown(&n);
}

/// The same when nobody said so yet, at the moment it would go; and with the assistant switched off.
#[test]
fn an_armed_share_does_not_go_after_a_denial() {
    for assistant_off in [false, true] {
        let (t, n, _device) = ready();
        n.acts().intent_run(&Permit::for_test(), Intent::ShareCapture { which: intents::Which::Next, public: true, friends: vec![] }, Via::Model).unwrap();
        if assistant_off {
            t.settings.lock().unwrap().n4n0.assistant = false;
        } else {
            off(&t);
        }
        n.set_captures(CapturesView { latest: Some(CaptureRow { key: "new".into(), phase: CapturePhase::Ready, ..Default::default() }), ..Default::default() });
        std::thread::sleep(Duration::from_millis(200));
        n.share_next_check();
        std::thread::sleep(Duration::from_millis(200));
        assert!(t.shares.lock().unwrap().is_empty());
        t.shutdown(&n);
    }
}

// --- A yes is for what was asked about ---

/// Another list since, and the yes is still for the one that was shown.
#[test]
fn a_yes_is_for_the_item_it_was_shown() {
    let (t, n, _device) = ready();
    *t.vrchat_invite.lock().unwrap() = Some(InviteAnswer::Sent);
    t.app_answers.lock().unwrap().insert("instances", json!({ "items": [{ "ref": "wrld_a:1", "world": "First place" }] }));
    turn(&t, &n, "my previous instances, invite me to the first", instances());
    let question = question_shown(&t);
    assert!(t.view().confirmations.last().is_some_and(|c| c.summary.contains("First place")), "{:?}", t.view().confirmations);
    n.n4n0.lock().unwrap().assistant.lists.insert("instances", vec![json!({ "ref": "wrld_b:2" })]);
    press_answer(&n, true, question, shown_epoch(&t));
    until("the invite", || !t.invited_self.lock().unwrap().is_empty());
    assert_eq!(*t.invited_self.lock().unwrap(), ["wrld_a:1"]);
    t.shutdown(&n);
}

/// Another VRChat account connected since, and the invite is not sent from it.
#[test]
fn a_yes_is_for_the_vrchat_account_it_was_asked_with() {
    let (t, n, _device) = ready();
    *t.vrchat_invite.lock().unwrap() = Some(InviteAnswer::Sent);
    *t.vrchat_account.lock().unwrap() = "usr_a".into();
    t.app_answers.lock().unwrap().insert("instances", json!({ "items": [{ "ref": "wrld_a:1", "world": "First place" }] }));
    turn(&t, &n, "my previous instances, invite me to the first", instances());
    let question = question_shown(&t);
    assert!(question.is_some());
    *t.vrchat_account.lock().unwrap() = "usr_b".into();
    press_answer(&n, true, question, shown_epoch(&t));
    std::thread::sleep(Duration::from_millis(300));
    assert!(t.invited_self.lock().unwrap().is_empty());
    assert!(t.view().lines.iter().any(|l| l.text.contains("VRChat account changed")), "{:?}", t.view().lines);
    t.shutdown(&n);
}

/// A newer capture since, and the yes is still for the one that was the latest; the app's own target goes with its action.
#[test]
fn a_yes_is_for_the_capture_and_the_person_it_was_shown() {
    let (t, n, _device) = ready();
    let row = |key: &str| CaptureRow { key: key.into(), world: format!("World of {key}"), taken: 1, phase: CapturePhase::Ready, ..Default::default() };
    n.set_captures(CapturesView { latest: Some(row("old")), history: vec![row("old")], ..Default::default() });
    turn(&t, &n, "share my latest capture with everyone", json!([call("a", "orbiters_pictures", json!({ "intent": "share_capture", "which": "latest", "public": true }))]));
    let question = question_shown(&t);
    assert!(t.view().confirmations.last().is_some_and(|c| c.summary.contains("World of old")), "{:?}", t.view().confirmations);
    n.set_captures(CapturesView { latest: Some(row("new")), history: vec![row("new"), row("old")], ..Default::default() });
    press_answer(&n, true, question, shown_epoch(&t));
    until("the share", || !t.shares.lock().unwrap().is_empty());
    assert_eq!(t.shares.lock().unwrap()[0].key, "old");
    *t.app_target.lock().unwrap() = json!({ "vrchat": "usr_me", "friend": "usr_sam", "says": "Samuel" });
    model_asks(&t, &n, "Sam", "orbit-a");
    assert!(t.view().confirmations.last().is_some_and(|c| c.summary.contains("Samuel")), "who the name means: {:?}", t.view().confirmations);
    *t.app_target.lock().unwrap() = json!({ "vrchat": "usr_other", "friend": "usr_other_sam" });
    press_answer(&n, true, question_shown(&t), shown_epoch(&t));
    assert_eq!(t.app_done.lock().unwrap().last().map(|(_, target)| target["friend"].clone()), Some(json!("usr_sam")), "as it was asked");
    t.shutdown(&n);
}
