//! Verify3-r21: what the gate, the fence and the marks of what was presented hold, driven through the real command handler.

use super::*;
use crate::fence::Permit;
use orbiters_n4n0_core::captures::ShareAsk;
use orbiters_n4n0_core::context::Taint;
use orbiters_n4n0_core::feedback::FeedbackAsk;
use orbiters_n4n0_core::intents::{self, Outcome, Page, Risk, Via, CATALOGUE};
use orbiters_n4n0_core::menu::InviteAnswer;
use orbiters_n4n0_core::tools::{self, Action};

fn at(surface: Surface, question: Option<u64>, epoch: u64) -> Option<Stamp> {
    Some(Stamp { epoch, question, surface, seq: crate::next_input() })
}

fn boops(t: &TestHost) -> usize {
    t.app_intents.lock().unwrap().iter().filter(|i| matches!(i, Intent::Boop { .. })).count()
}

// --- B3: a question is answered only from where it was presented ---

/// Every surface, the question presented on none of them or on each one: an answer from a surface counts exactly when that
/// surface presented it, whether pressed, typed or said.
#[test]
fn a_question_is_answered_only_where_it_was_presented() {
    use Surface::{Avatar, Overlay, Window};
    for shown in [None, Some(Window), Some(Overlay), Some(Avatar)] {
        for from in [Window, Overlay, Avatar] {
            for how in ["pressed", "typed", "said"] {
                let (t, n, _device) = ready();
                model_asks(&t, &n, "Sam", "orbit-a");
                let (question, epoch) = (question_shown(&t).unwrap(), shown_epoch(&t));
                if let Some(surface) = shown {
                    present(&n, question, surface);
                }
                match how {
                    "pressed" => n.command(Command::AskAnswer { yes: true, stamp: at(from, Some(question), epoch) }),
                    "typed" => n.command(Command::Ask { text: "yes".into(), stamp: at(from, Some(question), epoch) }),
                    // (The avatar's own Talk captures its stamp itself.)
                    _ if from == Avatar => {
                        n.talk(true, crate::runtime::Source::Menu, None, None);
                        assert!(n.n4n0.lock().unwrap().listening.is_some());
                        n.talk(false, crate::runtime::Source::Menu, None, None);
                        heard(&n, 1, "yes");
                    }
                    _ => {
                        n.app_talk(true, false, at(from, Some(question), epoch), None);
                        assert!(n.n4n0.lock().unwrap().listening.is_some(), "{shown:?} {from:?}");
                        let_go(&n);
                        heard(&n, 1, "yes");
                    }
                }
                let resolved = shown == Some(from);
                assert_eq!(boops(&t), resolved as usize, "presented on {shown:?}, {how} from {from:?}");
                assert_eq!(question_shown(&t), (!resolved).then_some(question), "presented on {shown:?}, {how} from {from:?}");
                t.shutdown(&n);
            }
        }
    }
}

/// Verify3 B3: Talk pressed on the avatar before the avatar had shown the question; its yes is for nothing, and N4N0 says what
/// it asks. Once the avatar has shown it, the same press answers it.
#[test]
fn the_avatars_talk_answers_only_what_the_avatar_has_shown() {
    let (t, n, _device) = ready();
    model_asks(&t, &n, "Sam", "orbit-a");
    let question = question_shown(&t).unwrap();
    // The window has shown it, where the person may not be.
    present(&n, question, Surface::Window);
    n.talk(true, crate::runtime::Source::Menu, None, None);
    n.talk(false, crate::runtime::Source::Menu, None, None);
    heard(&n, 1, "yes");
    assert_eq!(boops(&t), 0);
    assert_eq!(question_shown(&t), Some(question), "it still waits");
    assert!(t.view().lines.last().unwrap().text.contains("Sam"), "it is said again: {:?}", t.view().lines);
    present(&n, question, Surface::Avatar);
    n.talk(true, crate::runtime::Source::Menu, None, None);
    n.talk(false, crate::runtime::Source::Menu, None, None);
    heard(&n, 2, "yes");
    assert_eq!(boops(&t), 1);
    t.shutdown(&n);
}

/// The avatar's panel presents a question once its words stand there whole, not as they begin; a newer question is presented
/// nowhere, however long the one before was.
#[test]
fn the_panel_presents_a_question_once_it_stands_whole() {
    let (t, n, _vrchat) = n4n0_with_vrchat();
    *t.app_facts.lock().unwrap() = Some(full());
    n.publish();
    model_asks(&t, &n, "Sam", "orbit-a");
    let a = question_shown(&t).unwrap();
    assert_eq!(n.own_stamp(Surface::Avatar).question, None, "its words are on their way");
    until("the panel shows it whole", || n.own_stamp(Surface::Avatar).question == Some(a));
    assert_eq!(n.own_stamp(Surface::Window).question, None, "nothing said of the window");
    press_answer(&n, false, Some(a), shown_epoch(&t));
    model_asks(&t, &n, "Mira", "orbit-a");
    let b = question_shown(&t).unwrap();
    assert_eq!(n.own_stamp(Surface::Avatar).question, None);
    // A mark that comes late, for the one before, presents nothing of this one.
    present(&n, a, Surface::Avatar);
    assert_eq!(n.own_stamp(Surface::Avatar).question, None);
    until("the panel shows the next one whole", || n.own_stamp(Surface::Avatar).question == Some(b));
    t.shutdown(&n);
}

/// A mark of another member's surface presents nothing.
#[test]
fn another_members_mark_presents_nothing() {
    let (t, n, _device) = ready();
    let before = shown_epoch(&t);
    t.switch_member(&n, None);
    model_asks(&t, &n, "Sam", "orbit-b");
    let question = question_shown(&t).unwrap();
    n.command(Command::Presented { stamp: at(Surface::Window, Some(question), before) });
    n.command(Command::Presented { stamp: at(Surface::Window, None, shown_epoch(&t)) });
    n.command(Command::Presented { stamp: None });
    assert!(n.n4n0.lock().unwrap().presented.is_empty());
    n.command(Command::Presented { stamp: at(Surface::Window, Some(question), shown_epoch(&t)) });
    assert_eq!(n.n4n0.lock().unwrap().presented, [Surface::Window]);
    t.shutdown(&n);
}

// --- B4: the one taint rule ---

/// A call of every tool that is not an intent.
fn older_tools() -> Vec<(&'static str, Value)> {
    vec![
        ("orbiters_more", json!({ "groups": ["friends"] })),
        ("set_avatar_control", json!({ "control": "Hoodie", "on": true })),
        ("find_avatar_controls", json!({ "search": "hoodie" })),
        ("hold_avatar_control", json!({ "control": "Hoodie", "on": true })),
        ("set_avatar_parameter", json!({ "name": "Hoodie", "value": 1 })),
        ("take_photo", json!({})),
        ("save_video", json!({})),
        ("list_saved_videos", json!({})),
        ("copy_to_clipboard", json!({ "text": "tainted words" })),
        ("set_afk_message", json!({ "enabled": true })),
        ("control_media", json!({ "action": "pause" })),
        ("control_media", json!({})),
        ("n4n0_light", json!({ "on": true })),
        ("n4n0_mood", json!({ "mood": "happy" })),
    ]
}

/// Every action there is (the older tools, and every intent of the catalogue): the fence gives leave on a tainted turn exactly
/// to what changes nothing, and never, by itself, to what is sent from an account.
#[test]
fn the_fence_gives_leave_by_the_one_rule_for_every_tool() {
    let (t, n, _device) = ready();
    let older = older_tools();
    for (name, _) in tools::TOOLS {
        assert!(older.iter().any(|(n, _)| n == name), "{name} has no call here");
    }
    let mut actions: Vec<Action> = older.iter().map(|(name, input)| tools::parse(name, input).unwrap()).collect();
    for spec in CATALOGUE {
        actions.push(Action::App(intents::parse(spec.group.tool(), &crate::tests::assistant::sample(spec.name)).unwrap().unwrap()));
    }
    assert_eq!(actions.len(), older.len() + CATALOGUE.len());
    let fence = n.acts();
    for action in &actions {
        let effect = action.effect();
        assert!(!effect.from_account || effect.asks, "{action:?}");
        assert_eq!(fence.permit(action, true).is_some(), !effect.changes && !effect.asks, "{action:?} after others' text");
        assert_eq!(fence.permit(action, false).is_some(), !effect.asks, "{action:?}");
        // Where only the worn avatar's labels were read, its one control named may be set besides (verify5-r21).
        let set = matches!(action, Action::SetControl { .. });
        assert_eq!(fence.permit_tool(action, Taint::Labels).is_some(), !effect.asks && (set || !effect.changes), "{action:?} after the avatar's labels");
        assert_eq!(fence.permit_tool(action, Taint::Others).is_some(), fence.permit(action, true).is_some(), "{action:?}");
        assert_eq!(fence.permit_tool(action, Taint::None).is_some(), fence.permit(action, false).is_some(), "{action:?}");
        assert_eq!(fence.permit_said(action).is_some(), !effect.asks, "{action:?} said");
        assert!(fence.permit_yes(action).is_some(), "{action:?}: the owner's yes, the switch on");
    }
    drop(fence);
    *t.app_facts.lock().unwrap() = Some(Facts { actions: false, ..full() });
    let fence = n.acts();
    for action in &actions {
        assert_eq!(fence.permit_yes(action).is_some(), !action.effect().from_account, "{action:?}: a yes, the switch off");
    }
    drop(fence);
    t.shutdown(&n);
}

/// A turn that reads other people's text, then calls `tool`.
fn tainted_turn(tool: &str, input: &Value) -> (Arc<TestHost>, Shared, Vec<Value>) {
    let (t, n, _device) = ready();
    let calls = json!([call("w", "orbiters_worlds", json!({ "intent": "where_am_i" })), call("x", tool, input.clone())]);
    let (base, bodies) = orbiters(vec![done(json!([{ "role": "user", "content": "x" }]), calls), done(json!([]), json!([]))]);
    t.sign_in(base, "orbit-a");
    n.asked("tell me what the world I am in is about please".into(), "typed");
    turn_ended(&n, &bodies, 2);
    let results = bodies.lock().unwrap()[1]["tool_results"].as_array().cloned().unwrap_or_default();
    (t, n, results)
}

/// Verify3 B4, whole turns: each older tool that changes something waits for the owner's yes once other people's text was read
/// (a mood is simply left), and the yes does it.
#[test]
fn an_older_tool_waits_for_a_yes_after_others_text() {
    for (tool, input) in older_tools() {
        // Those of a worn avatar have one (below); opening tools changes nothing.
        if tool.contains("avatar") || tool == "orbiters_more" {
            continue;
        }
        let changes = tools::parse(tool, &input).unwrap().effect().changes;
        let (t, n, results) = tainted_turn(tool, &input);
        assert_eq!(question_shown(&t).is_some(), changes && tool != "n4n0_mood", "{tool} {input}: {results:?}");
        assert!(!t.settings().n4n0.afk_message_enabled, "{tool}");
        assert!(!n.n4n0.lock().unwrap().outbox.iter().any(|(_, (a, _))| a == "/usercamera/Capture"), "{tool}");
        assert!(!CLIPBOARD.lock().unwrap().iter().any(|c| c == "tainted words"), "{tool}");
        if tool == "set_afk_message" {
            press_answer(&n, true, question_shown(&t), shown_epoch(&t));
            assert!(t.settings().n4n0.afk_message_enabled, "the yes does it");
        }
        t.shutdown(&n);
    }
}

/// The same for the worn avatar's controls.
#[test]
fn an_avatars_control_waits_for_a_yes_after_others_text() {
    for (tool, input) in [("set_avatar_control", json!({ "control": "Hoodie", "on": true })), ("set_avatar_parameter", json!({ "name": "Hoodie", "value": 1 }))] {
        let (t, n, vrchat) = n4n0_with_vrchat();
        *t.app_facts.lock().unwrap() = Some(full());
        n.publish();
        received(&vrchat, Duration::from_millis(300));
        let calls = json!([call("w", "orbiters_worlds", json!({ "intent": "where_am_i" })), call("x", tool, input.clone())]);
        let (base, bodies) = orbiters(vec![done(json!([{ "role": "user", "content": "x" }]), calls), done(json!([]), json!([]))]);
        t.sign_in(base, "orbit-a");
        n.asked("where am I, and put my hoodie on".into(), "typed");
        turn_ended(&n, &bodies, 2);
        let sent = received(&vrchat, Duration::from_millis(400));
        assert!(!sent.iter().any(|(a, _)| *a == parameter("Hoodie")), "{tool}: {sent:?}");
        t.shutdown(&n);
    }
}

/// An older tool that reads other people's text (a video's title, what plays) taints the rest of its turn too.
#[test]
fn what_an_older_tool_read_taints_the_rest_of_the_turn() {
    for (tool, input) in [("list_saved_videos", json!({})), ("control_media", json!({}))] {
        let (t, n, _device) = ready();
        let calls = json!([call("r", tool, input), call("w", "orbiters_walk", json!({ "intent": "fast_walk", "on": true }))]);
        let (base, bodies) = orbiters(vec![done(json!([{ "role": "user", "content": "x" }]), calls), done(json!([]), json!([]))]);
        t.sign_in(base, "orbit-a");
        n.asked("what did I save, and then walk as it says".into(), "typed");
        turn_ended(&n, &bodies, 2);
        assert!(!t.settings().walk_boost.enabled, "{tool}");
        assert!(question_shown(&t).is_some(), "{tool}: asked");
        t.shutdown(&n);
    }
}

// --- B5: every command that acts for the person is stamped, and acts for that member or not at all ---

/// One of every command that acts for the person, stamped `stamp`.
fn acting(stamp: Option<Stamp>) -> Vec<Command> {
    vec![
        Command::Ask { text: "private words".into(), stamp },
        Command::AskAnswer { yes: true, stamp },
        Command::RetryTurn { id: 1, stamp },
        Command::Talk { down: true, stamp, press: None },
        Command::AskTalk { down: true, stamp, press: None },
        Command::Call { contact: "c1".into(), stamp },
        Command::AnswerCall { call: Some(7), stamp },
        Command::Suggest { text: "private idea".into(), voice: false, stamp },
        Command::Drop { link: "https://example.com/private".into(), title: "A card".into(), video: None, stamp },
        Command::InviteMe { location: "wrld_1:2".into(), stamp },
        Command::ShareCapture { share: ShareAsk { key: "c1".into(), public: true, ..Default::default() }, stamp },
        Command::Feedback { ask: FeedbackAsk::Like { world: "wrld_1".into(), on: true }, stamp },
        Command::OpenLink { url: format!("{}approve/42", orbiters_core::account::Server::current().website()), stamp },
        Command::NewConversation { conversation: Some(0), stamp },
        Command::Presented { stamp },
    ]
}

/// Verify3 B5: each of them captured while A was shown and arriving once B is signed in, or with no stamp at all, does nothing
/// as B; stamped B, each reaches what it is for.
#[test]
fn a_command_captured_under_a_does_nothing_as_b() {
    let (t, n, device) = ready();
    let (base, _) = orbiters(vec![]);
    t.sign_in(base, "orbit-a");
    let epoch_a = shown_epoch(&t);
    *t.vrchat_invite.lock().unwrap() = Some(InviteAnswer::Sent);
    *t.vrchat_account.lock().unwrap() = "usr_me".into();
    let (base_b, bodies_b) = orbiters(vec![]);
    switch_member(&t, &n, base_b, "orbit-b");
    let notices = t.notices().len();
    let done_nothing = |what: &str| {
        std::thread::sleep(Duration::from_millis(60));
        assert!(t.shares.lock().unwrap().is_empty() && t.invited_self.lock().unwrap().is_empty(), "{what}");
        assert!(t.feedback.lock().unwrap().is_empty() && t.calls.lock().unwrap().is_empty(), "{what}");
        assert!(bodies_b.lock().unwrap().is_empty() && device.browser.0.lock().unwrap().is_empty(), "{what}");
        assert!(n.n4n0.lock().unwrap().listening.is_none() && t.notices().len() == notices, "{what}: {:?}", t.notices());
        assert!(!t.view().lines.iter().any(|l| l.text.contains("private")), "{what}");
    };
    for command in acting(at(Surface::Overlay, None, epoch_a)).into_iter().chain(acting(None)) {
        let what = format!("{command:?}");
        // Every one of them is a command that carries a stamp.
        assert!(command.clone().stamp_mut().is_some(), "{what}");
        n.command(command);
        done_nothing(&what);
    }
    // A stamp a surface forgot is given by its process (and is never given over one it captured).
    let given = Command::ShareCapture { share: ShareAsk::default(), stamp: None }.stamped(at(Surface::Window, None, 7).unwrap());
    assert!(matches!(&given, Command::ShareCapture { stamp: Some(s), .. } if s.epoch == 7));
    let kept = given.stamped(at(Surface::Window, None, 8).unwrap());
    assert!(matches!(&kept, Command::ShareCapture { stamp: Some(s), .. } if s.epoch == 7));
    // As B: each does its thing.
    let b = || at(Surface::Overlay, None, shown_epoch(&t));
    for command in acting(b()) {
        if matches!(command, Command::ShareCapture { .. } | Command::InviteMe { .. } | Command::Feedback { .. } | Command::Call { .. } | Command::OpenLink { .. } | Command::Drop { .. }) {
            // (A number is one input's.)
            let mut command = command;
            *command.stamp_mut().unwrap() = b();
            n.command(command);
        }
    }
    until("B's own", || {
        t.shares.lock().unwrap().len() == 1 && t.invited_self.lock().unwrap().len() == 1 && t.feedback.lock().unwrap().len() == 1 && t.calls.lock().unwrap().len() == 1
    });
    assert_eq!(device.browser.0.lock().unwrap().len(), 1);
    assert!(t.notices().len() > notices, "the card's answer");
    t.shutdown(&n);
}

/// What was accepted for A and is not sent yet as B signs in is not sent: the menu's share, its invite, its feedback and its
/// suggestion each start a thread, whose fence is the one it was accepted with.
#[test]
fn what_was_accepted_for_a_waits_for_no_b() {
    let (t, n, _device) = ready();
    let (base, _) = orbiters(vec![]);
    t.sign_in(base, "orbit-a");
    *t.vrchat_invite.lock().unwrap() = Some(InviteAnswer::Sent);
    let (base_b, bodies_b) = orbiters(vec![]);
    for _ in 0..20 {
        let epoch = shown_epoch(&t);
        for command in acting(at(Surface::Overlay, None, epoch)).into_iter().filter(|c| matches!(c, Command::ShareCapture { .. } | Command::InviteMe { .. } | Command::Feedback { .. } | Command::Suggest { .. })) {
            let mut command = command;
            *command.stamp_mut().unwrap() = at(Surface::Overlay, None, epoch);
            n.command(command);
        }
        t.shares.lock().unwrap().clear();
        // Each sends as A (before the handover), or not at all: never as B, whose server hears nothing.
        switch_member(&t, &n, base_b.clone(), "orbit-b");
        let after = (t.shares.lock().unwrap().len(), t.invited_self.lock().unwrap().len(), t.feedback.lock().unwrap().len());
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!((t.shares.lock().unwrap().len(), t.invited_self.lock().unwrap().len(), t.feedback.lock().unwrap().len()), after, "nothing once B is signed in");
        let (base_a, _) = orbiters(vec![]);
        switch_member(&t, &n, base_a, "orbit-a");
    }
    assert!(bodies_b.lock().unwrap().is_empty());
    t.shutdown(&n);
}

/// An invite is for the VRChat account connected as it was accepted: another one by the time it goes sends nothing, and
/// Orbiters' account is not asked in its place.
#[test]
fn an_invite_is_for_the_vrchat_account_it_was_accepted_with() {
    let (base, asked) = orbiters_invites(201, r#"{"sent":true}"#);
    let host = TestHost::default();
    *host.server.lock().unwrap() = Some((base, "orbit-test".into()));
    *host.vrchat_invite.lock().unwrap() = Some(InviteAnswer::Sent);
    *host.vrchat_account.lock().unwrap() = "usr_a".into();
    let account = host.invite_account();
    assert_eq!(account.as_deref(), Some("usr_a"));
    *host.vrchat_account.lock().unwrap() = "usr_b".into();
    let InviteAnswer::Failed { message } = crate::screen::invite_route(&host, crate::FIRST_EPOCH, account.as_deref(), "wrld_1:2") else { panic!("nothing sent") };
    assert!(message.contains("changed"), "{message}");
    assert!(host.invited_self.lock().unwrap().is_empty() && asked.load(Ordering::SeqCst) == 0);
    // Orbiters' own account is asked only where none was connected as it was accepted, whatever came since.
    let host = TestHost::default();
    *host.vrchat_invite.lock().unwrap() = Some(InviteAnswer::Sent);
    let _ = crate::screen::invite_route(&host, crate::FIRST_EPOCH, None, "wrld_1:2");
    assert!(host.invited_self.lock().unwrap().is_empty());
}

// --- A: nobody's while who it is is not known ---

#[test]
fn nothing_is_let_in_while_who_is_signed_in_is_not_known() {
    let (t, n, _device) = ready();
    let (base, bodies) = orbiters(vec![]);
    t.sign_in(base, "orbit-a");
    t.unknown.store(true, Ordering::SeqCst);
    n.publish();
    assert_eq!(t.view().signed_as, SignedAs::Unknown);
    assert_eq!(t.view().voice.readiness, VoiceReadiness::SigningIn);
    for command in acting(at(Surface::Window, None, shown_epoch(&t))) {
        let mut command = command;
        *command.stamp_mut().unwrap() = at(Surface::Window, None, shown_epoch(&t));
        n.command(command);
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(bodies.lock().unwrap().is_empty() && t.shares.lock().unwrap().is_empty() && n.n4n0.lock().unwrap().listening.is_none());
    assert!(t.view().lines.iter().any(|l| l.text.contains("still signing you in")), "{:?}", t.view().lines);
    assert!(!t.view().lines.iter().any(|l| l.text.contains("private")));
    t.unknown.store(false, Ordering::SeqCst);
    n.publish();
    assert_eq!((t.view().signed_as, t.view().voice.readiness), (SignedAs::Member, VoiceReadiness::Ready));
    t.shutdown(&n);
}

// --- G: the switch of the account's actions ---

/// Without it: not offered, refused when called anyway, and the page opens instead; the same for what is said offline, and for
/// a yes given after it was switched off. What is not sent from an account goes on.
#[test]
fn without_the_switch_the_page_opens_instead() {
    let (t, n, _device) = ready();
    *t.app_facts.lock().unwrap() = Some(Facts { actions: false, ..full() });
    let calls = json!([call("b1", "orbiters_friends", json!({ "intent": "boop", "friend": "Sam" })), call("w1", "orbiters_walk", json!({ "intent": "fast_walk", "on": true }))]);
    let (base, bodies) = orbiters(vec![done(json!([{ "role": "user", "content": "x" }]), calls), done(json!([]), json!([]))]);
    t.sign_in(base, "orbit-a");
    n.asked("boop Sam and turn the fast walking on please".into(), "typed");
    turn_ended(&n, &bodies, 2);
    let first = bodies.lock().unwrap()[0]["tools"].to_string();
    assert!(!first.contains("\"boop\""), "not offered");
    let result = bodies.lock().unwrap()[1]["tool_results"][0].clone();
    assert!(result["is_error"] == true && result.to_string().contains("page"), "{result}");
    assert_eq!((boops(&t), question_shown(&t)), (0, None), "neither done nor asked");
    assert_eq!(t.view().assistant.open.as_ref().map(|o| o.page), Some(Page::AppHome), "its page opens");
    assert!(t.settings().walk_boost.enabled, "what is not sent from an account goes on");
    assert!(t.view().assistant.log.iter().any(|e| e.outcome == Outcome::Refused));
    // Said offline.
    n.asked("set my status to busy".into(), "typed");
    assert!(!t.app_intents.lock().unwrap().iter().any(|i| matches!(i, Intent::SetStatus { .. })));
    assert!(question_shown(&t).is_none());
    // Asked while it was on, answered once it is off.
    *t.app_facts.lock().unwrap() = Some(full());
    model_asks(&t, &n, "Sam", "orbit-a");
    let question = question_shown(&t);
    assert!(question.is_some());
    *t.app_facts.lock().unwrap() = Some(Facts { actions: false, ..full() });
    press_answer(&n, true, question, shown_epoch(&t));
    assert_eq!(boops(&t), 0);
    assert!(t.view().lines.iter().any(|l| l.text.contains("not from your account yet")), "{:?}", t.view().lines);
    t.shutdown(&n);
}

/// Every intent behind the switch, through the fence itself.
#[test]
fn every_account_intent_is_refused_by_the_fence_without_the_switch() {
    let (t, n, _device) = ready();
    *t.app_facts.lock().unwrap() = Some(Facts { actions: false, ..full() });
    for spec in CATALOGUE.iter().filter(|s| s.risk == Risk::Confirm) {
        let intent = intents::parse(spec.group.tool(), &crate::tests::assistant::sample(spec.name)).unwrap().unwrap();
        let action = Action::App(intent);
        let fence = n.acts();
        assert!(fence.permit(&action, false).is_none() && fence.permit_yes(&action).is_none() && fence.permit_said(&action).is_none(), "{}", spec.name);
    }
    let _ = Via::Model;
    t.shutdown(&n);
}

// --- The Majors ---

/// M5: a member's conversation outlives a restart of the app, sealed, under their epoch; another member's never comes back, nor
/// one New conversation ended, nor what N4N0 asked.
#[test]
fn a_conversation_outlives_a_restart_for_its_member_only() {
    let (t, n, _device) = ready();
    let (base, bodies) = orbiters(vec![
        done(json!([{ "role": "user", "content": "hi" }, { "role": "assistant", "content": "Hello there." }]), boop("Sam")),
        done(json!([{ "role": "user", "content": "hi" }, { "role": "assistant", "content": "Hello there." }]), json!([])),
    ]);
    t.sign_in(base, "orbit-a");
    n.asked("say something about the sea".into(), "typed");
    turn_ended(&n, &bodies, 2);
    let (conversation, epoch) = (n.n4n0.lock().unwrap().conversation.clone(), shown_epoch(&t));
    assert!(!conversation.is_empty() && question_shown(&t).is_some());
    until("the conversation is kept", || n.files.conversation.exists());
    let kept = std::fs::read(&n.files.conversation).unwrap();
    assert!(!String::from_utf8_lossy(&kept).contains("Hello there"), "sealed");
    let restart = || {
        let platform = Platform {
            clipboard: Box::new(FakeClipboard),
            browser: Box::new(Device::default().browser.clone()),
            sounds: Box::new(Device::default().sounds.clone()),
            locale: Box::new(English),
            ears: Box::new(Device::default().ears.clone()),
            media: Arc::new(Watcher::new(|| Box::new(NoMedia))),
            mouse: None,
        };
        N4n0::new(t.clone(), t.clone(), platform, n.files.clone())
    };
    // Not before who it is is known.
    let again = restart();
    assert!(again.n4n0.lock().unwrap().conversation.is_empty());
    again.pause_effects().resume_account(epoch);
    assert_eq!(again.n4n0.lock().unwrap().conversation, conversation);
    assert!(again.n4n0.lock().unwrap().lines.iter().any(|l| l.text.contains("about the sea")));
    assert!(again.n4n0.lock().unwrap().question().is_none(), "what it asked is not brought back");
    // Another member: it goes for good.
    let other = restart();
    other.pause_effects().resume_account(epoch + 1);
    assert!(other.n4n0.lock().unwrap().conversation.is_empty() && !n.files.conversation.exists());
    // A handover, and New conversation, drop it too.
    for drop_it in [true, false] {
        n.acts().keep_conversation();
        assert!(n.files.conversation.exists());
        if drop_it {
            n.command(Command::NewConversation { conversation: Some(t.view().conversation), stamp: n.stamped() });
        } else {
            t.switch_member(&n, None);
        }
        assert!(!n.files.conversation.exists());
    }
    t.shutdown(&n);
}

/// M6: a press that is not let in changes nothing of what the Ask page listens to.
#[test]
fn a_refused_ask_talk_leaves_the_listening_as_it_was() {
    let (t, n, _device) = ready();
    let (base, _) = orbiters(vec![]);
    t.sign_in(base, "orbit-a");
    n.ask_page(true);
    for stamp in [None, at(Surface::Overlay, None, shown_epoch(&t) + 1), at(Surface::Overlay, None, 0)] {
        n.command(Command::AskTalk { down: true, stamp, press: None });
        let rt = n.n4n0.lock().unwrap();
        assert!(rt.voice.automatic && rt.listening.is_some(), "{stamp:?} ended the page's listening");
    }
    n.command(Command::AskTalk { down: true, stamp: at(Surface::Overlay, None, shown_epoch(&t)), press: None });
    assert!(!n.n4n0.lock().unwrap().voice.automatic, "the person's own takes over");
    t.shutdown(&n);
}

/// M6: between a press let in and its listen installed there is no handover: its fence is held.
#[test]
fn a_talk_let_in_is_installed_before_a_handover() {
    let (t, n, _device) = ready();
    let (base, _) = orbiters(vec![]);
    t.sign_in(base, "orbit-a");
    let admitted = n.let_in(at(Surface::Window, None, shown_epoch(&t)));
    let (t2, n2) = (t.clone(), n.clone());
    let switching = std::thread::spawn(move || t2.switch_member(&n2, None));
    std::thread::sleep(Duration::from_millis(100));
    assert!(!switching.is_finished());
    n.talk_admitted(true, crate::runtime::Source::Window, admitted);
    switching.join().unwrap();
    assert!(n.n4n0.lock().unwrap().listening.is_none(), "it went with its member");
    t.shutdown(&n);
}

/// M7: a turn whose thread cannot be started ends, says so, and its message can be sent again; nothing promises to send it by
/// itself.
#[test]
fn a_turn_that_could_not_start_ends_and_can_be_retried() {
    let (t, n, _device) = ready();
    let (base, bodies) = orbiters(vec![format!("{}{}", delta("The sea."), done(json!([{ "role": "user", "content": "x" }, { "role": "assistant", "content": "The sea." }]), json!([])))]);
    t.sign_in(base, "orbit-a");
    n.no_threads.store(true, Ordering::SeqCst);
    n.asked("say something about the sea".into(), "typed");
    let view = t.view();
    assert!(!view.thinking && !n.n4n0.lock().unwrap().thinking(), "it does not think for ever");
    let failure = view.assistant.failure.clone().expect("said");
    assert!(failure.retry && !failure.automatic);
    n.no_threads.store(false, Ordering::SeqCst);
    n.command(Command::RetryTurn { id: failure.id, stamp: at(Surface::Window, None, shown_epoch(&t)) });
    turn_ended(&n, &bodies, 1);
    assert_eq!(bodies.lock().unwrap().len(), 1);
    assert!(t.view().lines.iter().any(|l| l.text.contains("The sea")));
    t.shutdown(&n);
}

/// M8: New conversation clears the conversation its surface showed, and no other.
#[test]
fn a_new_conversation_clears_only_the_one_shown() {
    let (t, n, _device) = ready();
    n.asked("turn on fast walk".into(), "typed");
    let first = t.view().conversation;
    let epoch = shown_epoch(&t);
    for (conversation, stamp) in [
        (Some(first), None),
        (None, at(Surface::Window, None, epoch)),
        (Some(first + 1), at(Surface::Window, None, epoch)),
        (Some(first), at(Surface::Window, None, epoch + 1)),
    ] {
        n.command(Command::NewConversation { conversation, stamp });
        assert!(t.view().lines.iter().any(|l| l.text.contains("fast walk")), "{conversation:?} {stamp:?}");
    }
    n.command(Command::NewConversation { conversation: Some(first), stamp: at(Surface::Window, None, epoch) });
    assert!(t.view().lines.is_empty() && t.view().conversation != first);
    // The same press, late, over the next one.
    n.asked("turn off fast walk".into(), "typed");
    n.command(Command::NewConversation { conversation: Some(first), stamp: at(Surface::Overlay, None, epoch) });
    assert!(!t.view().lines.is_empty());
    t.shutdown(&n);
}

// --- What the types cannot hold, read in the source ---

/// Every command that carries a stamp passes the gate where it is handled; what resolves an account or the VRChat account does
/// so in the fence's own functions; and the account's side effects are the fence's, which N4N0 alone has not.
#[test]
fn every_acting_command_passes_the_gate_and_nothing_resolves_whoever_is_signed_in() {
    let read = |name: &str| std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(name)).unwrap().replace("\r\n", "\n");
    let runtime = read("runtime.rs");
    let handler = body(&runtime, "pub fn command(self");
    let arms: Vec<&str> = handler.split("\n            Command::").skip(1).collect();
    for mut command in acting(None) {
        let name = format!("{command:?}").split([' ', '{']).next().unwrap().to_string();
        assert!(command.stamp_mut().is_some(), "{name}");
        let arm = arms.iter().find(|a| a.starts_with(&format!("{name} "))).unwrap_or_else(|| panic!("{name} has no arm"));
        let passes = ["let_in(", "self.talk(", "fence(&crate::fence::Ticket::input(epoch))", "app_talk(", "ask_talk(", "retry_turn(", ".ask(", "answer(", "ask_answer("].iter().any(|g| arm.contains(g));
        assert!(passes, "{name} is handled without the gate: {arm}");
    }
    // (Talk and Ask pass it where they begin.)
    for begins in ["pub(crate) fn talk(self", "pub(crate) fn ask(self"] {
        assert!(runtime.contains(begins) && (body(&runtime, begins).contains("let_in(") || body(&runtime, begins).contains("still(")), "{begins}");
    }
    // Those that act from an account exist on the fence only.
    let all: Vec<(&str, String)> = ["runtime.rs", "assistant.rs", "screen.rs", "captures.rs", "feedback.rs", "voice.rs", "conversation.rs"].into_iter().map(|f| (f, read(f))).collect();
    for name in ["suggestion_send", "invite_send", "card_drop", "share_capture", "feedback_ask", "keep_conversation", "open_instead"] {
        let defined: Vec<&str> = all.iter().filter(|(_, s)| s.contains(&format!("pub(crate) fn {name}(&self"))).map(|(f, _)| *f).collect();
        assert_eq!(defined.len(), 1, "{name}: {defined:?}");
        let source = &all.iter().find(|(f, _)| *f == defined[0]).unwrap().1;
        let before = &source[..source.find(&format!("pub(crate) fn {name}(&self")).unwrap()];
        let block = before.rmatch_indices("\nimpl ").next().map(|(i, _)| &before[i + 1..]).unwrap();
        assert!(block.starts_with("impl Fence<'_> {"), "{name} is not the fence's");
    }
    for (file, source) in &all {
        let calls = |what: &str| source.matches(what).count();
        let may = match *file {
            "screen.rs" => 1,
            // As the person is asked, and the same one as they say yes.
            "assistant.rs" => 2,
            _ => 0,
        };
        assert_eq!(calls(".invite_account("), may, "{file}: the VRChat account is resolved as an invite is accepted");
        assert_eq!(calls(".invite_self("), (*file == "screen.rs") as usize, "{file}: and nowhere but in invite_route is it used");
    }
    let _ = Permit::for_test;
}

#[path = "tests_fix4.rs"]
mod fix4;
