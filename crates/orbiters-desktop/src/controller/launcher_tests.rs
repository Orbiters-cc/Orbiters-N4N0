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

/// A press on the list of the test client's account (`usr_test`).
fn mine(action: Action) -> Action {
    Action::As { account: "usr_test".into(), action: Box::new(action) }
}

fn signed_in_list(c: &Shared) {
    c.update(|s| s.launcher.source = Source::SignedIn { user_id: "usr_test".into(), name: String::new() });
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
    assert_eq!(c.launcher_do(mine(Action::Boop { to: A.into(), emoji: Some("default_heart".into()) })), Some(Notice::Sent { to: A.into(), action: SentKind::Boop }));
    assert!(matches!(c.launcher_do(mine(Action::Boop { to: A.into(), emoji: None })), Some(Notice::CoolingDown { .. })), "a second boop waits");
    assert!(c.view().launcher.friend(A).unwrap().boop_ready_at.is_some(), "the row knows when");
    assert!(matches!(c.launcher_do(mine(Action::RequestInvite { to: B.into() })), Some(Notice::Failed { action: SentKind::RequestInvite, .. })), "the page takes its Sent back");
    assert_eq!(c.launcher_do(mine(Action::RequestInvite { to: B.into() })), Some(Notice::Sent { to: B.into(), action: SentKind::RequestInvite }), "a refused one can be tried again");
    let calls: Vec<String> = calls.lock().unwrap().iter().map(|(m, u)| format!("{m} {}", u.trim_start_matches("https://api.vrchat.cloud/api/1"))).collect();
    assert_eq!(calls, [format!("POST /users/{A}/boop"), format!("POST /requestInvite/{B}"), format!("POST /requestInvite/{B}")]);
}

#[test]
fn without_a_sign_in_nothing_is_sent() {
    let c = controller(Arc::new(Pretend::default()));
    assert!(matches!(c.launcher_do(Action::Boop { to: A.into(), emoji: None }), Some(Notice::Failed { .. })));
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
    let (_, _) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    let place = launcher_feed::place(&format!("{W}:777~friends({A})~region(jp)"), "Café").unwrap();
    c.update(|s| {
        s.launcher.invites = vec![Invite { id: "not_1".into(), kind: InviteKind::Invite, from_id: A.into(), from_name: "Nova".into(), from_icon: String::new(), place: Some(place), message: String::new(), received_at: 0 }];
    });
    c.launcher_do(mine(Action::AcceptInvite { invite: "not_1".into(), mode: LaunchMode::Vr }));
    let args = machine.started.lock().unwrap()[0].clone();
    assert_eq!(args.last().unwrap(), &format!("vrchat://launch?ref=orbiters.cc&id={W}:777~friends({A})~region(jp)"));
    assert!(args.contains(&"-screen-width".to_string()), "the small window in VR");
    assert!(c.view().launcher.invites.is_empty(), "the card goes");
    assert!(matches!(c.launcher_do(mine(Action::AcceptInvite { invite: "not_1".into(), mode: LaunchMode::Vr })), Some(Notice::Error(_))));
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
    c.launcher_do(mine(Action::Dismiss { id: "not_1".into() }));
    c.launcher_do(mine(Action::Dismiss { id: "not_2".into() }));
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
    signed_in_list(&c);
    c.launcher_do(mine(Action::OpenProfile { id: A.into() }));
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
    c.launcher_do(mine(Action::AcceptInvite { invite: "not_1".into(), mode: LaunchMode::Vr }));
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
    c.launcher.lock().unwrap().accepted.push(accepted(invite("not_2"), "usr_test", ARRIVE_WITHIN + Duration::from_secs(1)));
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
    assert!(matches!(c.launcher_do(Action::Boop { to: A.into(), emoji: None }), Some(Notice::Failed { .. })));
    assert!(matches!(c.launcher_do(Action::RequestInvite { to: A.into() }), Some(Notice::Failed { .. })));
    c.launcher_do(Action::Dismiss { id: "not_1".into() });
    assert!(calls.lock().unwrap().is_empty(), "nothing sent from the account that is not the list's");
}

/// A signed-in launcher with its list read, a friend in a world already named, the game looked at once.
fn loaded_and_idle() -> (Shared, Arc<Mutex<Vec<(String, String)>>>) {
    let c = controller(Arc::new(Pretend { installed: true, ..Default::default() }));
    let (client, calls) = c.test_vrchat(vec![]);
    let mut state = LauncherState { source: Source::SignedIn { user_id: "usr_test".into(), name: String::new() }, ..Default::default() };
    state.friends = vec![launcher_feed::friend(&json!({"id": A, "displayName": "Nova", "location": format!("{W}:1~region(eu)"), "state": "online"}), true).unwrap()];
    launcher_feed::regroup(&mut state, &[]);
    c.update(|s| *s.launcher = state);
    {
        let mut rt = c.launcher.lock().unwrap();
        rt.loaded_for = Some(Some("usr_test".into()));
        rt.loaded_at = Some(Instant::now());
        rt.worlds.insert(W.into(), Some(("Lounge".into(), String::new())));
        rt.pipeline = Some(Arc::new(AtomicBool::new(false)));
        rt.pipeline_client = Some(client);
    }
    c.launcher_tick();
    (c, calls)
}

#[test]
fn an_idle_launcher_publishes_nothing() {
    let (c, calls) = loaded_and_idle();
    let before = c.publishes.load(Ordering::Relaxed);
    for _ in 0..10 {
        c.launcher.lock().unwrap().game_checked = None;
        c.launcher_tick();
    }
    assert_eq!(c.publishes.load(Ordering::Relaxed), before, "nothing changed: nothing published");
    assert!(calls.lock().unwrap().is_empty());
    // A pipeline message that changes nothing publishes nothing either.
    let same = json!({"type": "friend-location", "content": json!({"userId": "usr_nobody", "location": "private"}).to_string()}).to_string();
    c.launcher_message(&same);
    assert_eq!(c.publishes.load(Ordering::Relaxed), before);
}

#[test]
fn a_replaced_pipeline_writes_nothing_into_the_list() {
    let (c, _) = loaded_and_idle();
    let old = c.launcher.lock().unwrap().pipeline.clone().unwrap();
    // The same account signed in again: a new client, so the old connection is stopped and its messages dropped.
    c.test_vrchat(vec![]);
    c.launcher_tick();
    assert!(old.load(Ordering::Acquire), "the old connection is told to stop");
    let new = c.launcher.lock().unwrap().pipeline.clone().unwrap();
    assert!(!Arc::ptr_eq(&old, &new), "a new one for the new sign-in");
    let offline = json!({"type": "friend-offline", "content": json!({"userId": A, "platform": ""}).to_string()}).to_string();
    c.launcher_message_of(&old, &offline);
    assert_eq!(c.view().launcher.friend(A).unwrap().presence, orbiters_core::launcher::Presence::InGame, "the old one's message is dropped");
    c.launcher_message_of(&new, &offline);
    assert_ne!(c.view().launcher.friend(A).unwrap().presence, orbiters_core::launcher::Presence::InGame);
}

#[test]
fn a_failed_action_is_said_in_the_toast_and_every_outcome_goes_after_a_while() {
    let c = controller(Arc::new(Pretend::default()));
    let before = c.notices.lock().unwrap().len();
    let failed = Notice::Failed { to: A.into(), action: SentKind::Boop, text: "VRChat refused the request (HTTP 400).".into() };
    c.launcher_notice(failed.clone());
    assert_eq!(c.notices.lock().unwrap().len(), before + 1, "a toast");
    assert_eq!(c.view().launcher.notice, Some(failed.clone()));
    c.launcher_notice_over(&failed);
    assert_eq!(c.view().launcher.notice, None, "not kept for ever (the Request pill is pressable again)");
    let sent = Notice::Sent { to: A.into(), action: SentKind::RequestInvite };
    c.launcher_notice(sent.clone());
    assert_eq!(c.notices.lock().unwrap().len(), before + 1, "a success is no toast");
    let newer = Notice::Sent { to: B.into(), action: SentKind::Boop };
    c.launcher_notice(newer.clone());
    c.launcher_notice_over(&sent);
    assert_eq!(c.view().launcher.notice, Some(newer), "a newer outcome stays its own time");
}

#[test]
fn a_world_that_failed_for_a_passing_reason_is_asked_again_later_not_never() {
    let (c, _) = loaded_and_idle();
    let (client, calls) = c.test_vrchat(vec![(503, json!({}))]);
    c.update(|s| {
        if let Whereabouts::World(p) = &mut s.launcher.friends[0].whereabouts {
            p.world_id = "wrld_00000000-0000-4000-8000-0000000000bb".into();
            p.world_name.clear();
        }
        launcher_feed::regroup(&mut s.launcher, &[]);
    });
    c.launcher_resolve_worlds(&client);
    c.launcher_resolve_worlds(&client);
    assert_eq!(calls.lock().unwrap().len(), 1, "not every tick");
    let rt = c.launcher.lock().unwrap();
    assert!(!rt.worlds.contains_key("wrld_00000000-0000-4000-8000-0000000000bb"), "not remembered as 'not given'");
    assert!(rt.lookup_failed.contains_key("wrld_00000000-0000-4000-8000-0000000000bb"));
}

#[test]
fn a_profile_opened_again_is_not_read_again_and_one_closed_is_not_read_on() {
    let c = controller(Arc::new(Pretend::default()));
    let (_, calls) = c.test_vrchat(vec![(200, json!({"bio": "hello"})), (200, json!({"note": "n"})), (200, json!({"friends": 2, "groups": 1}))]);
    c.update(|s| s.launcher.friends = vec![orbiters_core::launcher::Friend { id: A.into(), name: "Nova".into(), ..Default::default() }]);
    signed_in_list(&c);
    c.launcher_do(mine(Action::OpenProfile { id: A.into() }));
    c.launcher_do(Action::CloseProfile);
    c.launcher_do(mine(Action::OpenProfile { id: A.into() }));
    assert_eq!(calls.lock().unwrap().len(), 3, "kept five minutes");
    assert_eq!(c.view().launcher.profile.as_ref().unwrap().bio, "hello");
}

#[test]
fn a_boop_put_away_does_not_come_back_with_the_next_message() {
    let c = controller(Arc::new(Pretend::default()));
    c.update(|s| s.launcher.boops = vec![orbiters_core::launcher::Boop { id: "not_2".into(), from_id: A.into(), from_name: String::new(), from_icon: String::new(), emoji: None, received_at: 0 }]);
    c.launcher_do(Action::Dismiss { id: "not_2".into() });
    let again = json!({"type": "notification-v2", "content": json!({"id": "not_2", "type": "boop", "senderUserId": A, "createdAt": "2026-11-18T09:01:00Z", "data": {}}).to_string()}).to_string();
    c.launcher_message(&again);
    assert!(c.view().launcher.boops.is_empty());
}

/// A Mac: Obelisk runs VRChat and the person presses Play in it.
#[derive(Default)]
struct PretendMac {
    running: Mutex<Vec<(u32, bool)>>,
    obelisk: bool,
    vrchat: bool,
    opened: Mutex<u32>,
    cannot_open: bool,
}
impl Machine for PretendMac {
    fn running(&self) -> Vec<(u32, bool)> {
        self.running.lock().unwrap().clone()
    }
    fn installed(&self) -> bool {
        self.obelisk && self.vrchat
    }
    fn engine_missing(&self) -> bool {
        !self.obelisk
    }
    fn by_hand(&self) -> bool {
        true
    }
    fn start(&self, args: &[String]) -> Result<(), String> {
        assert!(args.is_empty(), "nothing can be given to a game the person starts in Obelisk");
        self.open_engine()
    }
    fn open_engine(&self) -> Result<(), String> {
        if self.cannot_open {
            return Err("no".into());
        }
        *self.opened.lock().unwrap() += 1;
        Ok(())
    }
}

fn mac(machine: Arc<PretendMac>) -> Shared {
    let c = Controller::new(Settings::default());
    c.launcher_machine(machine);
    c
}

fn place() -> Place {
    Place { world_id: W.into(), world_name: String::new(), world_picture: String::new(), instance_id: "7~friends(usr_x)".into(), access: orbiters_core::launcher::InstanceAccess::Friends, region: String::new() }
}

#[test]
fn a_mac_says_what_is_missing_then_offers_play() {
    let machine = Arc::new(PretendMac::default());
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::NoEngine);
    assert!(c.view().launcher.possible.play, "Home shows the line that says to get Obelisk");
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None }), Some(Notice::Error(_))));
    assert_eq!(*machine.opened.lock().unwrap(), 1, "where Obelisk is got was opened");
    assert_eq!(c.view().launcher.game, Game::NoEngine, "nothing is said to be starting");

    let machine = Arc::new(PretendMac { obelisk: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::NotInstalled, "Obelisk without VRChat");
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None }), Some(Notice::Error(_))));
    assert_eq!(c.launcher_do(Action::OpenEngine), None);
    assert_eq!(*machine.opened.lock().unwrap(), 2);

    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::NotRunning);
    assert!(c.view().launcher.possible.play);
}

#[test]
fn a_mac_that_follows_a_vrchat_elsewhere_is_not_told_to_get_obelisk() {
    let machine = Arc::new(PretendMac::default());
    let c = mac(machine.clone());
    c.update(|s| s.vrchat.remote = true);
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::NotRunning);
    assert!(!c.view().launcher.possible.play);
    // With Obelisk and VRChat here, this Mac can still start its own.
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(machine.clone());
    c.update(|s| s.vrchat.remote = true);
    c.launcher_game(machine.as_ref());
    assert!(c.view().launcher.possible.play);
}

#[test]
fn play_on_a_mac_opens_obelisk_and_the_game_is_seen_once_it_runs() {
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    // The VR button does not exist on a Mac; whatever mode is asked, it is the desktop game.
    assert_eq!(c.launcher_do(Action::Play { mode: LaunchMode::Vr, destination: None }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch }));
    assert_eq!(*machine.opened.lock().unwrap(), 1);
    assert_eq!(c.view().launcher.game, Game::Starting(LaunchMode::Desktop));
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::Starting(LaunchMode::Desktop), "while the person presses Play in Obelisk");
    machine.running.lock().unwrap().push((42, true));
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::Running(LaunchMode::Desktop));
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None }), Some(Notice::Error(_))));
    assert_eq!(*machine.opened.lock().unwrap(), 1, "never a second VRChat");
    machine.running.lock().unwrap().clear();
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::NotRunning, "closed: Play again");
}

#[test]
fn obelisk_that_does_not_open_is_said_and_nothing_starts() {
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, cannot_open: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place()) }), Some(Notice::Error(_))));
    assert_eq!(c.view().launcher.game, Game::NotRunning);
    assert!(c.launcher.lock().unwrap().pending.is_none(), "no destination waits for a game that was not started");
    assert!(matches!(c.launcher_do(Action::OpenEngine), Some(Notice::Error(_))));
}

#[test]
fn a_destination_on_a_mac_is_an_invite_to_oneself_now_or_once_the_game_is_up() {
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(machine.clone());
    let (_, calls) = c.test_vrchat(vec![(200, json!({})), (200, json!({}))]);
    // The list on show is the signed-in account's (an invite to oneself is sent only for it).
    c.update(|s| s.launcher.source = Source::SignedIn { user_id: "usr_test".into(), name: String::new() });
    c.launcher_game(machine.as_ref());
    // Not running: Obelisk opens, the destination waits, nothing is sent yet.
    assert_eq!(c.launcher_do(mine(Action::Play { mode: LaunchMode::Desktop, destination: Some(place()) })), Some(Notice::Sent { to: W.into(), action: SentKind::Launch }));
    assert!(calls.lock().unwrap().is_empty());
    machine.running.lock().unwrap().push((42, true));
    c.launcher_game(machine.as_ref());
    let sent = |n: usize| {
        for _ in 0..200 {
            if calls.lock().unwrap().len() >= n {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        calls.lock().unwrap().iter().map(|(m, u)| format!("{m} {}", u.trim_start_matches("https://api.vrchat.cloud/api/1"))).collect::<Vec<_>>()
    };
    assert_eq!(sent(1), [format!("POST /invite/myself/to/{W}:7~friends(usr_x)")], "sent when the game came up");
    c.launcher_game(machine.as_ref());
    assert_eq!(sent(1).len(), 1, "once");
    // Running: at once, never through Windows' link pipe.
    assert_eq!(c.launcher_do(mine(Action::Play { mode: LaunchMode::Desktop, destination: Some(place()) })), Some(Notice::Sent { to: W.into(), action: SentKind::Join }));
    assert_eq!(sent(2).len(), 2);
    assert_eq!(*machine.opened.lock().unwrap(), 1);
}

#[test]
fn a_destination_that_waited_too_long_is_forgotten() {
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(machine.clone());
    let (_, calls) = c.test_vrchat(vec![(200, json!({}))]);
    let long_ago = Instant::now().checked_sub(PENDING_FOR + Duration::from_secs(1)).expect("a clock that started more than ten minutes ago");
    c.launcher.lock().unwrap().pending = Some((place(), long_ago, Some("usr_test".into())));
    machine.running.lock().unwrap().push((42, true));
    c.launcher_game(machine.as_ref());
    std::thread::sleep(Duration::from_millis(150));
    assert!(calls.lock().unwrap().is_empty(), "the game of another evening does not get yesterday's invite");
    assert!(c.launcher.lock().unwrap().pending.is_none());
}

fn accepted(invite: Invite, owner: &str, ago: Duration) -> Accepted {
    Accepted { invite, at: Instant::now().checked_sub(ago).unwrap(), owner: owner.into(), from: None, hide_failures: 0, hide_again: None, moved: false }
}

#[test]
fn a_press_made_on_account_as_list_never_goes_out_with_bs_session() {
    // B (the test client, usr_test) is signed in and B's list is on show; the press was made on A's list before.
    let c = controller(Arc::new(Pretend::default()));
    let (_, calls) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    c.update(|s| s.launcher.friends = vec![orbiters_core::launcher::Friend { id: A.into(), ..Default::default() }]);
    let from_a = |action: Action| Action::As { account: "usr_aaaa".into(), action: Box::new(action) };
    assert!(matches!(c.launcher_do(from_a(Action::Boop { to: A.into(), emoji: None })), Some(Notice::Failed { action: SentKind::Boop, .. })), "refused, and the page takes its Sent back");
    assert!(matches!(c.launcher_do(from_a(Action::RequestInvite { to: A.into() })), Some(Notice::Failed { .. })));
    // A press that names no account sends nothing either.
    assert!(matches!(c.launcher_do(Action::Boop { to: A.into(), emoji: None }), Some(Notice::Failed { .. })));
    assert!(calls.lock().unwrap().is_empty(), "nothing went out with B's session: {:?}", calls.lock().unwrap());
    assert!(c.view().launcher.friend(A).unwrap().boop_ready_at.is_none(), "no cool-down taken for a send that never went");
}

#[test]
fn the_send_itself_checks_whose_session_it_is() {
    let (client, calls) = orbiters_core::vrchat_api::test_client(Arc::new(orbiters_core::vrchat_api::Gate::for_duration(Duration::from_secs(60))), vec![]);
    let client = Mutex::new(client);
    assert!(send_as(&client, "usr_aaaa", |c| c.boop(A, None)).is_err(), "the session became another account's");
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn a_private_profile_is_closed_when_the_account_changes() {
    let c = controller(Arc::new(Pretend::default()));
    let (_, _) = c.test_vrchat(vec![(503, json!({}))]);
    c.update(|s| {
        s.launcher.source = Source::SignedIn { user_id: "usr_aaaa".into(), name: String::new() };
        s.launcher.profile = Some(orbiters_core::launcher::Profile { friend: orbiters_core::launcher::Friend { id: A.into(), note: "A's private note".into(), ..Default::default() }, ..Default::default() });
    });
    c.launcher.lock().unwrap().loaded_for = Some(Some("usr_aaaa".into()));
    c.launcher_tick();
    assert!(c.view().launcher.profile.is_none(), "B never sees A's note");
}

#[test]
fn an_accepted_invite_of_another_account_is_neither_hidden_nor_shown_and_a_failed_hide_is_tried_again() {
    let c = controller(Arc::new(Pretend::default()));
    let (client, calls) = c.test_vrchat(vec![(503, json!({})), (200, json!({}))]);
    signed_in_list(&c);
    let place = launcher_feed::place(&format!("{W}:777~friends({A})~region(jp)"), "Café").unwrap();
    let invite = |id: &str| Invite { id: id.into(), kind: InviteKind::Invite, from_id: A.into(), from_name: "Nova".into(), from_icon: String::new(), place: Some(place.clone()), message: String::new(), received_at: 0 };
    c.update(|s| s.launcher.me = Some(orbiters_core::launcher::Friend { id: "usr_test".into(), whereabouts: Whereabouts::World(place.clone()), ..Default::default() }));
    // A's invite, ten minutes on, while B's list is on show: B's list does not get A's card; B's session hides nothing.
    c.launcher.lock().unwrap().accepted.push(accepted(invite("not_a"), "usr_aaaa", ARRIVE_WITHIN + Duration::from_secs(1)));
    c.launcher_settle_accepted(Some(&client));
    assert!(c.view().launcher.invites.is_empty());
    assert!(calls.lock().unwrap().is_empty());
    // An account change drops them.
    c.launcher.lock().unwrap().accepted.clear();
    // B's own, arrived: the hide fails once (503), is kept, and goes a minute later.
    c.launcher.lock().unwrap().accepted.push(accepted(invite("not_b"), "usr_test", Duration::from_secs(5)));
    c.launcher_settle_accepted(Some(&client));
    assert_eq!(calls.lock().unwrap().len(), 1);
    {
        let mut rt = c.launcher.lock().unwrap();
        assert_eq!(rt.accepted.len(), 1, "a failed hide is not dropped");
        rt.accepted[0].hide_again = Some(Instant::now());
    }
    c.launcher_settle_accepted(Some(&client));
    assert_eq!(calls.lock().unwrap().len(), 2);
    assert!(c.launcher.lock().unwrap().accepted.is_empty(), "hidden: done");
}

#[test]
fn being_in_the_instance_already_is_no_arrival_and_a_second_accept_is_refused() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    let (client, calls) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    let place = launcher_feed::place(&format!("{W}:777~friends({A})~region(jp)"), "Café").unwrap();
    let invite = Invite { id: "not_1".into(), kind: InviteKind::Invite, from_id: A.into(), from_name: "Nova".into(), from_icon: String::new(), place: Some(place.clone()), message: String::new(), received_at: 0 };
    // The cached location of the account is that instance already (an old visit).
    c.update(|s| {
        s.launcher.invites = vec![invite.clone()];
        s.launcher.me = Some(orbiters_core::launcher::Friend { id: "usr_test".into(), whereabouts: Whereabouts::World(place.clone()), ..Default::default() });
    });
    c.launcher_do(mine(Action::AcceptInvite { invite: "not_1".into(), mode: LaunchMode::Vr }));
    c.launcher_settle_accepted(Some(&client));
    assert!(calls.lock().unwrap().is_empty(), "not hidden on a location seen before the press");
    // The same notification again from the pipeline: the card does not come back while the invite is on the way.
    c.update(|s| s.launcher.invites = vec![invite.clone()]);
    assert!(matches!(c.launcher_do(mine(Action::AcceptInvite { invite: "not_1".into(), mode: LaunchMode::Vr })), Some(Notice::Error(_))), "one accept per invite");
    assert_eq!(machine.started.lock().unwrap().len(), 1);
}

#[test]
fn two_joins_with_only_a_side_session_running_start_one_vrchat() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    machine.running.lock().unwrap().push((20, false));
    let c = controller(machine.clone());
    let fake = Arc::new(super::super::travel::tests::Fake { running: vec![20], side_args: vec![20], ..Default::default() });
    *c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
    let place = launcher_feed::place(&format!("{W}:1~region(eu)"), "").unwrap();
    let first = c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place.clone()) });
    assert_eq!(first, Some(Notice::Sent { to: W.into(), action: SentKind::Join }));
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place) }), Some(Notice::Error(_))), "VRChat is starting");
    assert_eq!(fake.started.lock().unwrap().len(), 1, "one VRChat started");
}

#[test]
fn an_unchanged_scheduled_full_read_publishes_nothing() {
    let (c, _) = loaded_and_idle();
    let state = (*c.view().launcher).clone();
    let before = c.publishes.load(Ordering::Relaxed);
    // The same list read again (as the 30-minute read would): applied through the read's own commit.
    c.launcher_commit_read(state.clone());
    assert_eq!(c.publishes.load(Ordering::Relaxed), before, "nothing new: nothing published");
    let mut moved = state;
    moved.friends[0].status_text = "new".into();
    c.launcher_commit_read(moved);
    assert_eq!(c.publishes.load(Ordering::Relaxed), before + 1, "a change is published");
}

#[test]
fn a_second_play_on_a_mac_keeps_the_first_destination_and_an_account_change_drops_it() {
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    c.launcher_do(mine(Action::Play { mode: LaunchMode::Desktop, destination: Some(place()) }));
    // A plain Play pressed again while the person is still to press Play in Obelisk.
    c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None });
    assert!(c.launcher.lock().unwrap().pending.as_ref().is_some_and(|(p, _, a)| p.world_id == W && a.as_deref() == Some("usr_test")), "the first destination stays, bound to its account");
    // Another account signs in before the game is up: the destination is not sent to it.
    let (_, _calls) = c.test_vrchat(vec![(503, json!({}))]);
    c.launcher.lock().unwrap().loaded_for = Some(Some("usr_aaaa".into()));
    c.launcher_tick();
    assert!(c.launcher.lock().unwrap().pending.is_none());
}

/// A list as account A (usr_aaaa) left it: a friend with a private note and a private invite.
fn as_list_of_a(c: &Shared) {
    let place = launcher_feed::place(&format!("{W}:777~private(usr_aaaa)~region(eu)"), "").unwrap();
    c.update(|s| {
        s.launcher.source = Source::SignedIn { user_id: "usr_aaaa".into(), name: String::new() };
        s.launcher.friends = vec![orbiters_core::launcher::Friend { id: A.into(), note: "A's private note".into(), ..Default::default() }];
        s.launcher.invites = vec![Invite { id: "not_a".into(), kind: InviteKind::Invite, from_id: A.into(), from_name: "Nova".into(), from_icon: String::new(), place: Some(place), message: "A's message".into(), received_at: 0 }];
    });
    c.launcher.lock().unwrap().loaded_for = Some(Some("usr_aaaa".into()));
}

#[test]
fn a_failed_first_read_of_the_new_account_shows_nothing_of_the_old_one() {
    let c = controller(Arc::new(Pretend::default()));
    // B (usr_test) is signed in now; B's first read fails.
    let (_, _) = c.test_vrchat(vec![(503, json!({}))]);
    as_list_of_a(&c);
    c.launcher_tick();
    let view = c.view();
    assert!(view.launcher.friends.is_empty() && view.launcher.invites.is_empty(), "A's friends, notes and invites are gone");
    assert!(!matches!(&view.launcher.source, Source::SignedIn { user_id, .. } if user_id == "usr_aaaa"));
    assert!(c.launcher_do(mine(Action::OpenProfile { id: A.into() })).is_none());
    assert!(c.view().launcher.profile.is_none(), "no row of A's to open");
}

#[test]
fn a_profile_pressed_on_another_accounts_list_opens_nothing() {
    let c = controller(Arc::new(Pretend::default()));
    let (_, calls) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    c.update(|s| s.launcher.friends = vec![orbiters_core::launcher::Friend { id: A.into(), ..Default::default() }]);
    c.launcher_do(Action::As { account: "usr_aaaa".into(), action: Box::new(Action::OpenProfile { id: A.into() }) });
    assert!(c.view().launcher.profile.is_none());
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn a_join_pressed_on_another_accounts_list_hands_nothing_to_the_running_vrchat() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    machine.running.lock().unwrap().push((10, false));
    let c = controller(machine.clone());
    let (_, calls) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    let fake = Arc::new(super::super::travel::tests::Fake { running: vec![10], server: Some(10), ..Default::default() });
    *c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
    let place = launcher_feed::place(&format!("{W}:777~private(usr_aaaa)~region(eu)"), "").unwrap();
    let from_a = Action::As { account: "usr_aaaa".into(), action: Box::new(Action::Play { mode: LaunchMode::Desktop, destination: Some(place.clone()) }) };
    assert!(matches!(c.launcher_do(from_a), Some(Notice::Error(_))));
    assert!(fake.written.lock().unwrap().is_empty(), "A's private room never reaches B's VRChat");
    assert!(calls.lock().unwrap().is_empty());
    // B's own press does.
    assert_eq!(c.launcher_do(mine(Action::Play { mode: LaunchMode::Desktop, destination: Some(place) })), Some(Notice::Sent { to: W.into(), action: SentKind::Join }));
    assert_eq!(fake.written.lock().unwrap().len(), 1);
}

#[test]
fn a_dismiss_pressed_on_another_accounts_list_changes_nothing() {
    let c = controller(Arc::new(Pretend::default()));
    let (_, calls) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    c.update(|s| s.launcher.boops = vec![orbiters_core::launcher::Boop { id: "not_2".into(), from_id: A.into(), from_name: String::new(), from_icon: String::new(), emoji: None, received_at: 0 }]);
    c.launcher_do(Action::As { account: "usr_aaaa".into(), action: Box::new(Action::Dismiss { id: "not_2".into() }) });
    assert_eq!(c.view().launcher.boops.len(), 1);
    assert!(c.launcher.lock().unwrap().dismissed.as_ref().is_none_or(|d| d.is_empty()));
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn an_invite_cleared_elsewhere_does_not_come_back_and_coming_back_to_the_room_counts() {
    let c = controller(Arc::new(Pretend::default()));
    let (client, calls) = c.test_vrchat(vec![(200, json!({}))]);
    signed_in_list(&c);
    let place = launcher_feed::place(&format!("{W}:777~friends({A})~region(jp)"), "Café").unwrap();
    let invite = |id: &str| Invite { id: id.into(), kind: InviteKind::Invite, from_id: A.into(), from_name: "Nova".into(), from_icon: String::new(), place: Some(place.clone()), message: String::new(), received_at: 0 };
    // Cleared on another device while accepted here: VRChat's pipeline says so.
    c.launcher.lock().unwrap().accepted.push(accepted(invite("not_1"), "usr_test", ARRIVE_WITHIN + Duration::from_secs(1)));
    c.launcher_message(&json!({"type": "hide-notification", "content": "not_1"}).to_string());
    c.launcher_settle_accepted(Some(&client));
    assert!(c.view().launcher.invites.is_empty(), "its card does not come back");
    // Accepted while in that very room: leaving, then coming back, is the arrival.
    let at = |p: Option<Place>| c.update(|s| s.launcher.me = Some(orbiters_core::launcher::Friend { id: "usr_test".into(), whereabouts: p.map_or(Whereabouts::Private, Whereabouts::World), ..Default::default() }));
    let mut a = accepted(invite("not_2"), "usr_test", Duration::from_secs(5));
    a.from = Some((place.world_id.clone(), place.instance_id.clone()));
    c.launcher.lock().unwrap().accepted.push(a);
    at(Some(place.clone()));
    c.launcher_settle_accepted(Some(&client));
    assert!(calls.lock().unwrap().is_empty(), "still where it was at the press");
    let elsewhere = launcher_feed::place(&format!("{W}:1~region(eu)"), "").unwrap();
    at(Some(elsewhere));
    c.launcher_settle_accepted(Some(&client));
    at(Some(place.clone()));
    c.launcher_settle_accepted(Some(&client));
    assert_eq!(calls.lock().unwrap().len(), 1, "back in the room: hidden");
}

#[test]
fn a_second_play_on_a_mac_opens_obelisk_once() {
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None });
    c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None });
    assert_eq!(*machine.opened.lock().unwrap(), 1);
}

/// A computer whose VRChats change between two looks: the person's quits right after the first.
struct Flicker {
    looks: Mutex<u32>,
    started: Mutex<u32>,
    written: Mutex<u32>,
}
impl super::super::travel::Travel for Flicker {
    fn running(&self) -> Vec<u32> {
        let mut n = self.looks.lock().unwrap();
        *n += 1;
        if *n == 1 { vec![10, 20] } else { vec![20] }
    }
    fn own_osc_ports(&self, pid: u32) -> bool {
        pid == 20
    }
    fn connect(&self) -> std::io::Result<(Box<dyn super::super::travel::LinkPipe>, Option<u32>)> {
        Err(std::io::ErrorKind::NotFound.into())
    }
    fn start(&self, _link: &str) -> Result<(), String> {
        *self.started.lock().unwrap() += 1;
        Ok(())
    }
}

#[test]
fn a_join_while_vrchat_is_starting_never_starts_a_second_one_when_the_processes_change() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    machine.running.lock().unwrap().push((10, false));
    let c = controller(machine.clone());
    let flicker = Arc::new(Flicker { looks: Mutex::new(0), started: Mutex::new(0), written: Mutex::new(0) });
    *c.travel.lock().unwrap() = Some(flicker.clone() as Arc<dyn super::super::travel::Travel>);
    *c.launcher_started.lock().unwrap() = Some(Instant::now());
    let place = launcher_feed::place(&format!("{W}:1~region(eu)"), "").unwrap();
    let _ = c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place) });
    assert_eq!(*flicker.started.lock().unwrap(), 0, "the route seen first (the person's VRChat) is the one taken: no start");
    let _ = *flicker.written.lock().unwrap();
}
