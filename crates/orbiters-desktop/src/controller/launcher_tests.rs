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
    c.update(|s| s.launcher.friends = vec![orbiters_core::launcher::Friend { id: A.into(), ..Default::default() }]);
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
    c.launcher_game(machine.as_ref());
    // Not running: Obelisk opens, the destination waits, nothing is sent yet.
    assert_eq!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place()) }), Some(Notice::Sent { to: W.into(), action: SentKind::Launch }));
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
    assert_eq!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place()) }), Some(Notice::Sent { to: W.into(), action: SentKind::Join }));
    assert_eq!(sent(2).len(), 2);
    assert_eq!(*machine.opened.lock().unwrap(), 1);
}

#[test]
fn a_destination_that_waited_too_long_is_forgotten() {
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(machine.clone());
    let (_, calls) = c.test_vrchat(vec![(200, json!({}))]);
    let long_ago = Instant::now().checked_sub(PENDING_FOR + Duration::from_secs(1)).expect("a clock that started more than ten minutes ago");
    c.launcher.lock().unwrap().pending = Some((place(), long_ago));
    machine.running.lock().unwrap().push((42, true));
    c.launcher_game(machine.as_ref());
    std::thread::sleep(Duration::from_millis(150));
    assert!(calls.lock().unwrap().is_empty(), "the game of another evening does not get yesterday's invite");
    assert!(c.launcher.lock().unwrap().pending.is_none());
}
