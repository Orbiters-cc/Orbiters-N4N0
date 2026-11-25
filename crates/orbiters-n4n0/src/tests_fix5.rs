//! Verify5-r21: a call is left whoever Orbiters takes the person for; a control is set when one is named exactly and the owner's words
//! leave no doubt; what opens after other people's text waits for a yes. Driven through the real command handler and the real
//! turn, with only the model (a stand-in Orbiters) faked.

use super::*;

/// The avatar of verify5-r21's cases.
fn wardrobe() -> (Arc<TestHost>, Shared, UdpSocket) {
    let (t, n, vrchat) = n4n0_with_vrchat();
    *t.app_facts.lock().unwrap() = Some(full());
    let manifest = orbiters_contracts::avatar::AvatarManifest::parse(
        r#"{ "format": "orbiters.avatar", "version": 1, "avatar_id": "avtr_x",
          "parameters": [ { "name": "HairRed", "type": "bool" }, { "name": "ShirtRed", "type": "bool" }, { "name": "Hat", "type": "bool" },
            { "name": "AllOff", "type": "bool" }, { "name": "Hood", "type": "bool" }, { "name": "Bare", "type": "bool" } ],
          "controls": [
            { "path": ["Hair", "Red"], "type": "toggle", "parameter": "HairRed", "value": 1 },
            { "path": ["Shirt", "Red"], "type": "toggle", "parameter": "ShirtRed", "value": 1 },
            { "path": ["Clothes", "Top hat"], "type": "toggle", "parameter": "Hat", "value": 1 },
            { "path": ["Clothes", "Off"], "type": "toggle", "parameter": "AllOff", "value": 1 },
            { "path": ["Clothes", "<b>Hoodie</b>"], "type": "toggle", "parameter": "Hood", "value": 1 },
            { "path": ["Clothes", "Bare"], "type": "toggle", "parameter": "Bare", "value": 1, "tags": ["nsfw"] } ] }"#,
    )
    .unwrap();
    {
        let mut rt = n.n4n0.lock().unwrap();
        rt.controls = orbiters_n4n0_core::controls::Controls::from_manifest(&manifest);
        rt.manifest = true;
    }
    n.publish();
    received(&vrchat, Duration::from_millis(300));
    (t, n, vrchat)
}

fn set(control: &str, on: bool) -> Value {
    call("c", "set_avatar_control", json!({ "control": control, "on": on }))
}

/// What VRChat was sent of the wardrobe.
fn worn(vrchat: &UdpSocket) -> Vec<String> {
    let mut out: Vec<String> = received(vrchat, Duration::from_millis(400))
        .into_iter()
        .filter_map(|(a, v)| a.strip_prefix("/avatar/parameters/").filter(|p| ["HairRed", "ShirtRed", "Hat", "AllOff", "Hood", "Bare"].contains(p)).map(|p| format!("{p}={v:?}")))
        .collect();
    out.dedup();
    out
}

fn result(bodies: &Arc<Mutex<Vec<Value>>>, n: usize) -> String {
    bodies.lock().unwrap()[1]["tool_results"][n]["content"].as_str().unwrap_or_default().to_string()
}

// --- M1 ---

/// The member unknown (their account file cannot be read): a window's Hang up and Mute still are for the call on screen, stamped
/// before or since; Answer is not.
#[test]
fn a_call_is_muted_and_left_while_the_member_is_not_known() {
    let (t, n, _device) = ready();
    let (base, _) = orbiters(vec![]);
    t.sign_in(base, "orbit-a");
    *t.in_call.lock().unwrap() = true;
    let before = at(Surface::Window, None, shown_epoch(&t));
    n.pause_effects().park_account(shown_epoch(&t), Some("production/1"));
    t.unknown.store(true, Ordering::SeqCst);
    n.publish();
    let now = || at(Surface::Window, None, shown_epoch(&t));
    n.command(Command::MuteCall { on: true, call: Some(7), stamp: before });
    n.command(Command::MuteCall { on: false, call: Some(7), stamp: now() });
    n.command(Command::AnswerCall { call: Some(7), stamp: now() });
    let end = now();
    n.command(Command::EndCall { call: Some(7), stamp: end });
    // Delivered twice, it acts once.
    n.command(Command::EndCall { call: Some(7), stamp: end });
    assert_eq!(*t.calls.lock().unwrap(), [CallRequest::Mute { call: 7, on: true }, CallRequest::Mute { call: 7, on: false }, CallRequest::End(7)]);
    t.shutdown(&n);
}

// --- M2 ---

/// Two controls of one label: told with their menus, and set when the owner's words say which; else the owner is asked, whatever the
/// model picked.
#[test]
fn two_controls_of_one_label_are_told_apart_or_asked_about() {
    for (text, control, expected) in [
        ("make my hair red", "Hair / Red", vec!["HairRed=Bool(true)"]),
        ("make my hair red", "Shirt / Red", vec![]),
        ("red on my avatar please", "Hair / Red", vec![]),
        ("my avatar in red please", "Red", vec![]),
    ] {
        let (t, n, vrchat) = wardrobe();
        let bodies = turn(&t, &n, text, json!([set(control, true)]));
        let told = bodies.lock().unwrap()[0]["context"]["summary"].as_str().unwrap().to_string();
        assert!(told.contains("\"Hair / Red\" (toggle") && told.contains("\"Shirt / Red\" (toggle"), "{told}");
        assert_eq!(worn(&vrchat), expected, "{text:?}, {control}");
        assert!(question_shown(&t).is_none(), "{text:?}: the owner says which, not yes or no");
        if expected.is_empty() {
            let said = result(&bodies, 0);
            assert!(said.contains("Hair / Red") && said.contains("Shirt / Red") && said.contains("which one"), "{said}");
        }
        t.shutdown(&n);
    }
}

/// "Take my hat off" offers the hat, not only what is called Off; a label with formatting is found by its words; what names no
/// control exactly sets nothing.
#[test]
fn a_control_is_set_by_its_exact_name_only() {
    for (text, control, on, expected) in [
        ("take my hat off", "Clothes / Top hat", false, vec!["Hat=Bool(false)"]),
        ("put my hoodie on", "Clothes / Hoodie", true, vec!["Hood=Bool(true)"]),
        ("mets mon sweat", "hoodie", true, vec!["Hood=Bool(true)"]),
        ("take my hat off", "hat", false, vec![]),
        ("my avatar, the jet pack", "jetpack", true, vec![]),
    ] {
        let (t, n, vrchat) = wardrobe();
        let bodies = turn(&t, &n, text, json!([set(control, on)]));
        assert!(bodies.lock().unwrap()[0]["context"]["summary"].as_str().unwrap().contains("\"Clothes / Top hat\""), "{text}");
        assert_eq!(worn(&vrchat), expected, "{text:?}, {control}");
        assert!(question_shown(&t).is_none(), "{text:?}");
        assert_eq!(result(&bodies, 0).contains("done"), !expected.is_empty(), "{}", result(&bodies, 0));
        t.shutdown(&n);
    }
}

/// The owner's decision of 2026-11-24: where the worn avatar's labels are all that was read, the control is set at once, and
/// anything else that changes something waits for a yes; with others' text read too, the control waits as well; an nsfw one
/// always does.
#[test]
fn a_control_alone_is_set_after_the_avatars_labels() {
    let walk = || call("w", "orbiters_walk", json!({ "intent": "fast_walk", "on": true }));
    let (t, n, vrchat) = wardrobe();
    turn(&t, &n, "put my hoodie on and make me walk fast", json!([set("Clothes / Hoodie", true), walk()]));
    assert_eq!(worn(&vrchat), ["Hood=Bool(true)"]);
    assert!(!t.settings().walk_boost.enabled && question_shown(&t).is_some(), "the walk waits");
    press_answer(&n, true, question_shown(&t), shown_epoch(&t));
    assert!(t.settings().walk_boost.enabled);
    // A follow-up, with what was said and done with it: still the avatar's labels alone.
    turn(&t, &n, "take it off", json!([set("Clothes / Hoodie", false)]));
    assert_eq!(worn(&vrchat), ["Hood=Bool(false)"]);
    t.shutdown(&n);
    // Other people's text beside.
    let (t, n, vrchat) = wardrobe();
    t.world(|w| {
        w.in_world = true;
        w.world_name = Some("INJECTED WORLD".into());
    });
    turn(&t, &n, "what is this world called, and put my hoodie on", json!([set("Clothes / Hoodie", true)]));
    assert!(worn(&vrchat).is_empty() && question_shown(&t).is_some());
    t.shutdown(&n);
    let (t, n, vrchat) = wardrobe();
    turn(&t, &n, "my avatar, bare please", json!([set("Clothes / Bare", true)]));
    assert!(worn(&vrchat).is_empty() && question_shown(&t).is_some());
    t.shutdown(&n);
}

/// A question about something named tells no more than that: "can you put my hoodie on?" sets it.
#[test]
fn asking_for_a_control_is_not_asking_about_the_world() {
    let (t, n, vrchat) = wardrobe();
    t.world(|w| {
        w.in_world = true;
        w.world_name = Some("INJECTED WORLD".into());
    });
    let bodies = turn(&t, &n, "can you put my hoodie on?", json!([set("Clothes / Hoodie", true)]));
    assert!(!bodies.lock().unwrap()[0].to_string().contains("INJECTED"));
    assert_eq!(worn(&vrchat), ["Hood=Bool(true)"]);
    t.shutdown(&n);
}

// --- m2: pages ---

/// After other people's text, the page opened instead and the Ask page (which listens as it opens) wait for a yes; before, they
/// open.
#[test]
fn pages_that_listen_or_open_instead_wait_after_others_text() {
    let ask = || json!([call("b", "orbiters_pages", json!({ "intent": "open_page", "page": "ask" }))]);
    let status = || json!([call("a", "orbiters_profile", json!({ "intent": "set_status", "status": "ask_me" }))]);
    for (calls, opens) in [(ask(), "Ask"), (status(), "Home")] {
        let (t, n, _device) = ready();
        off(&t);
        t.world(|w| {
            w.in_world = true;
            w.world_name = Some("INJECTED WORLD".into());
        });
        turn(&t, &n, "what is this world called", calls.clone());
        assert!(t.view().assistant.open.is_none() && t.app_intents.lock().unwrap().is_empty(), "{opens}: {:?}", t.view().assistant.open);
        let question = question_shown(&t);
        assert!(question.is_some(), "{opens}: asked");
        press_answer(&n, true, question, shown_epoch(&t));
        assert!(t.view().assistant.open.is_some(), "{opens}: the yes opens it");
        t.shutdown(&n);
        let (t, n, _device) = ready();
        off(&t);
        turn(&t, &n, "could you maybe make my status the orange one", calls);
        assert!(t.view().assistant.open.is_some() && question_shown(&t).is_none(), "{opens}: nothing of others' was read");
        t.shutdown(&n);
    }
}

// --- m10 ---

/// N4N0's own notes are told past one that waits for a turn that may hear it, each once.
#[test]
fn own_notes_are_told_past_one_that_waits() {
    let (t, n, _device) = ready();
    {
        let mut rt = n.n4n0.lock().unwrap();
        for (text, taint) in [("FIRST", Taint::None), ("QUOTES OTHERS", Taint::Others), ("LAST", Taint::None)] {
            rt.notes.push(orbiters_n4n0_core::context::Note { text: text.into(), taint });
        }
    }
    let summary = |bodies: Arc<Mutex<Vec<Value>>>| bodies.lock().unwrap()[0]["context"]["summary"].as_str().unwrap().to_string();
    let walk = || json!([call("w", "orbiters_walk", json!({ "intent": "fast_walk", "on": true }))]);
    let first = summary(turn(&t, &n, "make me walk faster over the hills", walk()));
    assert!(first.contains("FIRST") && first.contains("LAST") && !first.contains("QUOTES OTHERS"), "{first}");
    assert!(t.settings().walk_boost.enabled);
    let then = summary(turn(&t, &n, "do that again, what you said earlier", walk()));
    assert!(then.contains("QUOTES OTHERS") && !then.contains("FIRST"), "{then}");
    assert!(n.n4n0.lock().unwrap().notes.iter().all(|note| !note.text.contains("QUOTES")), "told once");
    t.shutdown(&n);
}
