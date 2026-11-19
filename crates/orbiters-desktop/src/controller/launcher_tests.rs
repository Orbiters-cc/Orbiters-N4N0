//! The launcher's controller against recorded replies (made-up people) and a pretend computer: nothing reaches VRChat.
use super::*;
use orbiters_core::launcher::{GroupKind, Invite, InviteKind, Presence};
use orbiters_core::settings::Settings;
use serde_json::json;

const A: &str = "usr_00000000-0000-4000-8000-000000000001";
const B: &str = "usr_00000000-0000-4000-8000-000000000002";
const W: &str = "wrld_00000000-0000-4000-8000-0000000000aa";

#[derive(Default)]
struct Pretend {
    running: Mutex<Vec<(u32, bool)>>,
    installed: bool,
    started: Mutex<Vec<Vec<String>>>,
    window: Mutex<Option<SavedWindow>>,
}
impl Machine for Pretend {
    fn running(&self) -> Vec<(u32, bool)> {
        self.running.lock().unwrap().clone()
    }
    fn installed(&self) -> bool {
        self.installed
    }
    fn start(&self, args: &[String]) -> Result<(), String> {
        self.started.lock().unwrap().push(args.to_vec());
        Ok(())
    }
    fn saved_window(&self) -> Option<SavedWindow> {
        *self.window.lock().unwrap()
    }
}

fn controller(machine: Arc<Pretend>) -> Shared {
    let c = Controller::new(Settings::default());
    c.launcher_machine(machine);
    c.update(|s| s.launcher.possible.play = true);
    c
}

#[test]
fn one_read_gives_sections_groups_invites_and_boops() {
    let gate = Arc::new(orbiters_core::vrchat_api::Gate::for_duration(Duration::from_secs(60)));
    let (client, calls) = orbiters_core::vrchat_api::test_client(
        gate,
        vec![
            (200, json!([{"id": A, "displayName": "Nova", "status": "join me", "location": format!("{W}:1~region(eu)"), "platform": "standalonewindows", "tags": [], "userIcon": "https://api.vrchat.cloud/a", "nameplateEffect": "invt_1"}])),
            (200, json!([{"id": B, "displayName": "Kite", "status": "offline", "location": "offline", "platform": "", "tags": []}])),
            (200, json!({"id": "usr_00000000-0000-4000-8000-0000000000ff", "displayName": "Me", "status": "ask me", "statusDescription": "hi", "tags": []})),
            (200, json!([{"name": "group_0", "displayName": "best", "type": "friend"}])),
            (200, json!([{"type": "friend", "favoriteId": B, "tags": ["group_0"]}])),
            (200, json!([{"id": "not_1", "type": "invite", "senderUserId": A, "senderUsername": "Nova", "created_at": "2026-11-18T09:00:00Z", "details": {"worldId": format!("{W}:1~region(eu)"), "worldName": "Lounge"}}])),
            (200, json!([{"id": "not_2", "type": "boop", "senderUserId": A, "createdAt": "2026-11-18T09:01:00Z", "details": {"emojiId": "default_heart"}, "data": {"boopingUserDisplayName": "Nova"}}])),
        ],
    );
    let (state, groups, worn) = read_signed_in(&Mutex::new(client)).unwrap();
    assert_eq!(groups, [("group_0".to_string(), "best".to_string())]);
    let sections: Vec<(GroupKind, usize)> = state.groups.iter().map(|g| (g.kind, g.members.len())).collect();
    assert_eq!(sections, [(GroupKind::Favourite, 1), (GroupKind::InGame, 1), (GroupKind::Website, 0), (GroupKind::Offline, 1)]);
    assert_eq!(state.me.as_ref().unwrap().name, "Me");
    assert_eq!(state.invites[0].from_icon, "https://api.vrchat.cloud/a", "the sender's picture from the list");
    assert_eq!(state.boops[0].emoji.as_deref(), Some("default_heart"));
    assert!(state.possible.boop && state.possible.live_status);
    assert_eq!(worn[A].nameplate.as_deref(), Some("invt_1"));
    let paths: Vec<String> = calls.lock().unwrap().iter().map(|(m, u)| format!("{m} {}", u.trim_start_matches("https://api.vrchat.cloud/api/1"))).collect();
    assert_eq!(paths[0], "GET /auth/user/friends?offline=false&n=100&offset=0");
    assert_eq!(paths[1], "GET /auth/user/friends?offline=true&n=100&offset=0");
    assert_eq!(paths.len(), 7, "{paths:?}");
}

#[test]
fn boops_and_requests_hold_a_cool_down_per_person() {
    let c = controller(Arc::new(Pretend::default()));
    let (_, calls) = c.test_vrchat(vec![(200, json!({"success": {"message": "ok"}})), (400, json!({"error": {"message": "You are not friends", "status_code": 400}})), (200, json!({}))]);
    c.update(|s| {
        s.launcher.friends = vec![orbiters_core::launcher::Friend { id: A.into(), ..Default::default() }];
        s.launcher.source = Source::SignedIn { user_id: "usr_test".into(), name: String::new() };
    });
    assert_eq!(c.launcher_do(Action::Boop { to: A.into(), emoji: Some("default_heart".into()) }), Some(Notice::Sent { to: A.into(), action: SentKind::Boop }));
    assert!(matches!(c.launcher_do(Action::Boop { to: A.into(), emoji: None }), Some(Notice::CoolingDown { .. })), "a second boop waits");
    assert!(c.view().launcher.friend(A).unwrap().boop_ready_at.is_some(), "the row knows when");
    assert!(matches!(c.launcher_do(Action::RequestInvite { to: B.into() }), Some(Notice::Error(_))));
    assert_eq!(c.launcher_do(Action::RequestInvite { to: B.into() }), Some(Notice::Sent { to: B.into(), action: SentKind::RequestInvite }), "a refused one can be tried again");
    let calls: Vec<String> = calls.lock().unwrap().iter().map(|(m, u)| format!("{m} {}", u.trim_start_matches("https://api.vrchat.cloud/api/1"))).collect();
    assert_eq!(calls, [format!("POST /users/{A}/boop"), format!("POST /requestInvite/{B}"), format!("POST /requestInvite/{B}")]);
}

#[test]
fn without_a_sign_in_nothing_is_sent() {
    let c = controller(Arc::new(Pretend::default()));
    assert!(matches!(c.launcher_do(Action::Boop { to: A.into(), emoji: None }), Some(Notice::Error(_))));
}

#[test]
fn play_starts_one_vrchat_with_the_options_and_never_a_second() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    assert_eq!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch }));
    assert_eq!(machine.started.lock().unwrap()[0][0], "--no-vr");
    assert!(!machine.started.lock().unwrap()[0].contains(&"-screen-width".to_string()), "the desktop window stays normal");
    assert_eq!(c.view().launcher.game, Game::Starting(LaunchMode::Desktop));
    assert_eq!(c.view().settings.launch.last_mode, LaunchMode::Desktop);
    machine.running.lock().unwrap().push((42, false));
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Vr, destination: None }), Some(Notice::Error(_))));
    assert_eq!(machine.started.lock().unwrap().len(), 1, "no second VRChat");
}

#[test]
fn accepting_an_invite_starts_in_its_instance() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    let place = launcher_feed::place(&format!("{W}:777~friends({A})~region(jp)"), "Café").unwrap();
    c.update(|s| {
        s.launcher.invites = vec![Invite { id: "not_1".into(), kind: InviteKind::Invite, from_id: A.into(), from_name: "Nova".into(), from_icon: String::new(), place: Some(place), message: String::new(), received_at: 0 }];
    });
    c.launcher_do(Action::AcceptInvite { invite: "not_1".into(), mode: LaunchMode::Vr });
    let args = machine.started.lock().unwrap()[0].clone();
    assert_eq!(args.last().unwrap(), &format!("vrchat://launch?ref=orbiters.cc&id={W}:777~friends({A})~region(jp)"));
    assert!(args.contains(&"-screen-width".to_string()), "the small window in VR");
    assert!(c.view().launcher.invites.is_empty(), "the card goes");
    assert!(matches!(c.launcher_do(Action::AcceptInvite { invite: "not_1".into(), mode: LaunchMode::Vr }), Some(Notice::Error(_))));
}

#[test]
fn not_installed_says_so() {
    let c = controller(Arc::new(Pretend::default()));
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Vr, destination: None }), Some(Notice::Error(_))));
}

#[test]
fn dismissing_an_invite_hides_it_at_vrchat_and_a_boop_only_here() {
    let c = controller(Arc::new(Pretend::default()));
    let (_, calls) = c.test_vrchat(vec![(200, json!({}))]);
    c.update(|s| {
        s.launcher.source = Source::SignedIn { user_id: "usr_test".into(), name: String::new() };
        s.launcher.invites = vec![Invite { id: "not_1".into(), kind: InviteKind::Request, from_id: A.into(), from_name: String::new(), from_icon: String::new(), place: None, message: String::new(), received_at: 0 }];
        s.launcher.boops = vec![orbiters_core::launcher::Boop { id: "not_2".into(), from_id: A.into(), from_name: String::new(), from_icon: String::new(), emoji: None, received_at: 0 }];
    });
    c.launcher_do(Action::Dismiss { id: "not_1".into() });
    c.launcher_do(Action::Dismiss { id: "not_2".into() });
    assert!(c.view().launcher.invites.is_empty() && c.view().launcher.boops.is_empty());
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert!(calls.lock().unwrap()[0].1.ends_with("/auth/user/notifications/not_1/hide"));
}

#[test]
fn a_pipeline_message_moves_a_friend() {
    let c = controller(Arc::new(Pretend::default()));
    c.update(|s| {
        s.launcher.friends = vec![launcher_feed::friend(&json!({"id": A, "displayName": "Nova", "location": "offline"}), false).unwrap()];
    });
    let message = json!({"type": "friend-online", "content": json!({"userId": A, "location": "private", "platform": "android", "canRequestInvite": true, "user": {"id": A, "displayName": "Nova", "status": "join me", "nameplateEffect": "invt_9"}}).to_string()}).to_string();
    c.launcher_message(&message);
    let view = c.view();
    assert_eq!(view.launcher.friend(A).unwrap().presence, Presence::InGame);
    assert_eq!(view.launcher.groups.iter().find(|g| g.kind == GroupKind::InGame).unwrap().members, [A]);
    assert_eq!(c.launcher.lock().unwrap().worn[A].nameplate.as_deref(), Some("invt_9"), "its decoration is to be resolved");
}

#[test]
fn the_profile_opens_at_once_then_fills_in() {
    let c = controller(Arc::new(Pretend::default()));
    let (_, _) = c.test_vrchat(vec![(200, json!({"bio": "hello", "pronouns": "they/them"})), (200, json!({"note": "n", "date_joined": "2021-04-01"})), (200, json!({"friends": 2, "groups": 1}))]);
    c.update(|s| s.launcher.friends = vec![orbiters_core::launcher::Friend { id: A.into(), name: "Nova".into(), ..Default::default() }]);
    c.launcher_do(Action::OpenProfile { id: A.into() });
    let p = c.view().launcher.profile.clone().unwrap();
    assert_eq!((p.bio.as_str(), p.friend.note.as_str(), p.mutual_friends, p.loading), ("hello", "n", Some(2), false));
    c.launcher_do(Action::CloseProfile);
    assert!(c.view().launcher.profile.is_none());
}

#[test]
fn worlds_get_their_names_once_known() {
    let mut state = LauncherState::default();
    state.friends = vec![launcher_feed::friend(&json!({"id": A, "displayName": "Nova", "location": format!("{W}:1~region(eu)"), "state": "online"}), true).unwrap()];
    launcher_feed::regroup(&mut state, &[]);
    assert_eq!(unnamed_worlds(&state), [W]);
    let worlds: HashMap<String, Option<(String, String)>> = [(W.to_string(), Some(("Lounge".to_string(), "https://api.vrchat.cloud/w".to_string())))].into();
    fill_worlds(&mut state, &worlds);
    let orbiters_core::launcher::Whereabouts::World(p) = &state.friend(A).unwrap().whereabouts else { panic!() };
    assert_eq!((p.world_name.as_str(), p.world_picture.as_str()), ("Lounge", "https://api.vrchat.cloud/w"));
    assert!(unnamed_worlds(&state).is_empty());
}

#[test]
fn only_an_ended_session_stops_the_pipeline() {
    use orbiters_core::pipeline::NoToken;
    assert_eq!(token_outcome(Ok("t".into())), Ok("t".into()));
    assert_eq!(token_outcome(Err(Error::Unauthorized)), Err(NoToken::Refused));
    assert_eq!(token_outcome(Err(Error::Disabled)), Err(NoToken::Refused));
    for later in [Error::Unavailable, Error::RateLimited(Duration::from_secs(9)), Error::InvalidResponse, Error::Refused(502, String::new()), Error::DeadlineExceeded] {
        assert_eq!(token_outcome(Err(later)), Err(NoToken::Later), "a network blip does not end the live list");
    }
}

#[test]
fn a_first_read_that_fails_is_not_asked_again_at_every_look() {
    let c = controller(Arc::new(Pretend::default()));
    // Both friend lists answer, then the account itself fails: one round of requests, then a wait.
    let (_, calls) = c.test_vrchat(vec![(200, json!([])), (200, json!([])), (503, json!({}))]);
    for _ in 0..4 {
        c.launcher_tick();
    }
    assert_eq!(calls.lock().unwrap().len(), 3, "one round, not one every two seconds: {:?}", calls.lock().unwrap());
    let wait = c.launcher.lock().unwrap().retry_at.unwrap().saturating_duration_since(Instant::now());
    assert!(wait > Duration::from_secs(20) && wait <= Duration::from_secs(38), "{wait:?}");
    assert!(matches!(c.view().launcher.notice, Some(Notice::Error(_))));
}

#[test]
fn failed_reads_wait_longer_and_longer_and_respect_vrchats_own_wait() {
    let e = Error::Unavailable;
    let secs: Vec<u64> = (1..=7).map(|n| retry_wait(n, &e, 32768).as_secs()).collect();
    assert_eq!(secs, [30, 60, 120, 240, 480, 600, 600], "neither every tick nor half an hour");
    assert!(retry_wait(1, &e, 0) >= Duration::from_millis(22_500) && retry_wait(1, &e, u16::MAX) < Duration::from_millis(37_500));
    assert_eq!(retry_wait(1, &Error::RateLimited(Duration::from_secs(90)), 32768), Duration::from_secs(90));
}

#[test]
fn a_second_play_while_vrchat_starts_starts_nothing() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    assert_eq!(c.launcher_do(Action::Play { mode: LaunchMode::Vr, destination: None }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch }));
    // Steam and Easy Anti-Cheat take their time: no VRChat process yet.
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Vr, destination: None }), Some(Notice::Error(_))));
    let place = launcher_feed::place(&format!("{W}:1~region(eu)"), "").unwrap();
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place) }), Some(Notice::Error(_))));
    assert_eq!(machine.started.lock().unwrap().len(), 1, "one start");
    // Two at the same moment, on their own threads: still one.
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    let threads: Vec<_> = (0..4).map(|_| { let c = c.clone(); std::thread::spawn(move || c.launcher_do(Action::Play { mode: LaunchMode::Vr, destination: None })) }).collect();
    threads.into_iter().for_each(|t| { t.join().unwrap(); });
    assert_eq!(machine.started.lock().unwrap().len(), 1);
}

#[test]
fn a_destination_outside_vrchats_grammar_starts_nothing() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    for instance in ["1&shortName=x", "1 --profile=3", "1~region(\"eu\")"] {
        let place = Place { world_id: W.into(), world_name: String::new(), world_picture: String::new(), instance_id: instance.into(), access: orbiters_core::launcher::InstanceAccess::Public, region: "eu".into() };
        assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Vr, destination: Some(place) }), Some(Notice::Error(_))), "{instance}");
    }
    assert!(machine.started.lock().unwrap().is_empty());
}

#[test]
fn desktop_after_vr_gets_the_window_vrchat_had_before() {
    let normal = SavedWindow { width: 1920, height: 1080, mode: 1 };
    let machine = Arc::new(Pretend { installed: true, window: Mutex::new(Some(normal)), ..Default::default() });
    let c = controller(machine.clone());
    c.launcher_do(Action::Play { mode: LaunchMode::Vr, destination: None });
    assert_eq!(c.view().settings.launch.desktop_window, Some(normal), "kept before the tiny window replaces it");
    // VRChat quit; its saved window is now the tiny one.
    *c.launcher_started.lock().unwrap() = None;
    *machine.window.lock().unwrap() = Some(SavedWindow { width: 100, height: 100, mode: 3 });
    c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None });
    let args = machine.started.lock().unwrap()[1].clone();
    let at = args.iter().position(|a| a == "-screen-width").expect("a size is asked for");
    assert_eq!(args[at..at + 6], ["-screen-width", "1920", "-screen-height", "1080", "-screen-fullscreen", "1"]);
    // A normal saved window: Desktop passes nothing.
    *c.launcher_started.lock().unwrap() = None;
    *machine.window.lock().unwrap() = Some(normal);
    c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None });
    assert!(!machine.started.lock().unwrap()[2].contains(&"-screen-width".to_string()));
}

#[test]
fn play_goes_through_steam_only_when_steam_has_vrchat() {
    assert_eq!(route(true, true), Route::Steam);
    assert_eq!(route(true, false), Route::Steam);
    assert_eq!(route(false, true), Route::Launcher, "the Meta store's VRChat beside a Steam without it");
    assert_eq!(route(false, false), Route::NotInstalled);
}

#[test]
fn an_accepted_invite_is_hidden_at_vrchat_only_once_the_account_got_there() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    let (client, calls) = c.test_vrchat(vec![(200, json!({}))]);
    c.update(|s| s.launcher.source = Source::SignedIn { user_id: "usr_test".into(), name: String::new() });
    let place = launcher_feed::place(&format!("{W}:777~friends({A})~region(jp)"), "Café").unwrap();
    let invite = |id: &str| Invite { id: id.into(), kind: InviteKind::Invite, from_id: A.into(), from_name: "Nova".into(), from_icon: String::new(), place: Some(place.clone()), message: String::new(), received_at: 0 };
    c.update(|s| s.launcher.invites = vec![invite("not_1")]);
    c.launcher_do(Action::AcceptInvite { invite: "not_1".into(), mode: LaunchMode::Vr });
    assert!(c.view().launcher.invites.is_empty(), "the card goes at once");
    c.launcher_settle_accepted(Some(&client));
    assert!(calls.lock().unwrap().is_empty(), "nothing hidden at VRChat while VRChat is only starting");
    // The account's own location arrives (the pipeline's user-location): now VRChat hides it.
    c.update(|s| s.launcher.me = Some(orbiters_core::launcher::Friend { id: "usr_test".into(), whereabouts: Whereabouts::World(place.clone()), ..Default::default() }));
    c.launcher_settle_accepted(Some(&client));
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert!(calls.lock().unwrap()[0].1.ends_with("/auth/user/notifications/not_1/hide"));
    // One that never arrives comes back as a card, and VRChat was told nothing.
    c.update(|s| s.launcher.me = None);
    c.launcher.lock().unwrap().accepted.push((invite("not_2"), Instant::now().checked_sub(ARRIVE_WITHIN + Duration::from_secs(1)).unwrap()));
    c.launcher_settle_accepted(Some(&client));
    assert_eq!(c.view().launcher.invites.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["not_2"]);
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn a_press_on_another_accounts_list_sends_nothing() {
    let c = controller(Arc::new(Pretend::default()));
    let (_, calls) = c.test_vrchat(vec![]);
    // The list on show was read for another account (the sign-in changed; its own list is not read yet).
    c.update(|s| {
        s.launcher.source = Source::SignedIn { user_id: B.into(), name: String::new() };
        s.launcher.friends = vec![orbiters_core::launcher::Friend { id: A.into(), ..Default::default() }];
        s.launcher.invites = vec![Invite { id: "not_1".into(), kind: InviteKind::Request, from_id: A.into(), from_name: String::new(), from_icon: String::new(), place: None, message: String::new(), received_at: 0 }];
    });
    assert!(matches!(c.launcher_do(Action::Boop { to: A.into(), emoji: None }), Some(Notice::Error(_))));
    assert!(matches!(c.launcher_do(Action::RequestInvite { to: A.into() }), Some(Notice::Error(_))));
    c.launcher_do(Action::Dismiss { id: "not_1".into() });
    assert!(calls.lock().unwrap().is_empty(), "nothing sent from the account that is not the list's");
}
