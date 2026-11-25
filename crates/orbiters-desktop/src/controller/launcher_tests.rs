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
    /// Each running VRChat's `--profile` by process (0 when not said).
    profiles: Mutex<HashMap<u32, u32>>,
    /// Starts beside running VRChats (VRChat's own launcher), apart from Steam's.
    beside: Mutex<Vec<Vec<String>>>,
    /// The controller, to ask at the very moment of a start whether its profile was already claimed.
    ctl: Mutex<Option<std::sync::Weak<Controller>>>,
    claimed_at_start: Mutex<Vec<bool>>,
    /// Processes whose command line could not be read.
    unread: Mutex<Vec<u32>>,
    /// Why VRChat's log cannot be read here, and the VRChats looked at the moment a place is handed over (none: as at the last
    /// look).
    log_problem: Mutex<Option<LogTrouble>>,
    now: Mutex<Option<Vec<crate::state::VrchatRunShown>>>,
}
impl Pretend {
    fn note_claim(&self, args: &[String]) {
        if let Some(c) = self.ctl.lock().unwrap().as_ref().and_then(std::sync::Weak::upgrade) {
            let profile = orbiters_core::vrchat_launch::profile_of(args);
            self.claimed_at_start.lock().unwrap().push(c.launcher_claimed(profile));
        }
    }
}
impl Machine for Pretend {
    fn running(&self) -> Vec<(u32, bool)> {
        self.running.lock().unwrap().clone()
    }
    fn running_profiles(&self) -> Vec<(u32, bool, Option<u32>)> {
        let profiles = self.profiles.lock().unwrap();
        let unread = self.unread.lock().unwrap();
        self.running.lock().unwrap().iter().map(|(pid, d)| (*pid, *d, (!unread.contains(pid)).then(|| profiles.get(pid).copied().unwrap_or(0)))).collect()
    }
    fn log_problem(&self) -> Option<LogTrouble> {
        self.log_problem.lock().unwrap().clone()
    }
    fn roster_now(&self) -> Option<Vec<crate::state::VrchatRunShown>> {
        self.now.lock().unwrap().clone()
    }
    fn start_beside(&self, args: &[String]) -> Result<(), String> {
        self.note_claim(args);
        // A start takes a moment: another press arriving meanwhile must find the profile claimed.
        std::thread::sleep(Duration::from_millis(5));
        self.beside.lock().unwrap().push(args.to_vec());
        Ok(())
    }
    fn installed(&self) -> bool {
        self.installed
    }
    fn start(&self, args: &[String]) -> Result<(), String> {
        self.note_claim(args);
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

/// These profiles are on Home's choice (an account was once seen on each).
fn chips(c: &Shared, profiles: &[u32]) {
    c.update_settings(|s| s.launch.profiles.extend(profiles.iter().map(|p| (*p, "Sample Name".to_string()))));
}

/// What a restart may be made for, by the account of its launch.
fn of_account(live: impl Fn(&str) -> bool) -> impl Fn(&HomeLaunch) -> Result<Vec<String>, String> {
    move |launch| if launch.account.as_deref().is_none_or(&live) { Ok(launch.args.clone()) } else { Err("another account".into()) }
}

/// A press of `account`'s, as a start keeps it.
#[cfg(target_os = "macos")]
fn errand_of(account: &str) -> Errand {
    Errand::plain(LaunchMode::Desktop, None).to(Some(place()), Some(account))
}

/// One look of the game, as the launcher makes it every few seconds, whatever computer this test runs on.
fn look(c: &Shared, machine: &dyn Machine) {
    let roster = machine.roster().unwrap_or_else(|| c.view().vrchat.runs.clone());
    let runs = runs_with_profiles(machine, &roster);
    c.launcher_look_for_logs(machine, &runs, &roster, c.launcher_several(machine, None));
    c.launcher_go_pending(machine, &runs, &roster);
}

/// A VRChat runs as `pid` on `profile`, signed in as `account` by its own log.
fn runs_as(c: &Shared, pid: u32, profile: u32, account: Option<&str>) {
    c.update(|s| s.vrchat.runs = vec![crate::state::VrchatRunShown { pid, profile, profile_known: true, account_id: account.map(str::to_owned), ..Default::default() }]);
}

/// The session there is (a made-up one, when the test made none), as whose list is read.
fn by(c: &Shared) -> By {
    if c.vrchat_session().is_none() {
        c.test_vrchat(vec![]);
    }
    c.vrchat_session().expect("a made-up session").by
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
    by(&c);
    signed_in_list(&c);
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

/// The VRChat just started came up (seen by a look of the game, which ends its start's claim) and was quit.
fn ran_and_quit(machine: &Arc<Pretend>, c: &Shared) {
    machine.running.lock().unwrap().push((99, false));
    c.update(|s| s.vrchat.runs = vec![crate::state::VrchatRunShown { pid: 99, profile: 0, ..Default::default() }]);
    c.launcher_game(machine.as_ref());
    if !orbiters_core::osc::VRCHAT_RUNS_HERE {
        // A Mac looks at no VRChat process of this computer's here (`launcher_game`): the look that ends the claim is
        // the platform's, stood in for.
        c.launcher.lock().unwrap().starting.clear();
    }
    machine.running.lock().unwrap().clear();
    c.update(|s| s.vrchat.runs.clear());
    *c.launcher_started.lock().unwrap() = None;
}

#[test]
fn desktop_after_vr_gets_the_window_vrchat_had_before() {
    let normal = SavedWindow { width: 1920, height: 1080, mode: 1 };
    let machine = Arc::new(Pretend { installed: true, window: Mutex::new(Some(normal)), ..Default::default() });
    let c = controller(machine.clone());
    c.launcher_do(Action::Play { mode: LaunchMode::Vr, destination: None });
    assert_eq!(c.view().settings.launch.desktop_window, Some(normal), "kept before the tiny window replaces it");
    // VRChat came up (a look saw it, which ends its start's claim on profile 0), then quit; its saved window is now the
    // tiny one.
    ran_and_quit(&machine, &c);
    *machine.window.lock().unwrap() = Some(SavedWindow { width: 100, height: 100, mode: 3 });
    c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None });
    let args = machine.started.lock().unwrap()[1].clone();
    let at = args.iter().position(|a| a == "-screen-width").expect("a size is asked for");
    assert_eq!(args[at..at + 6], ["-screen-width", "1920", "-screen-height", "1080", "-screen-fullscreen", "1"]);
    // A normal saved window: Desktop passes nothing.
    ran_and_quit(&machine, &c);
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
    c.launcher_settle_accepted(Some(&client), Some(&by(&c)));
    assert!(calls.lock().unwrap().is_empty(), "nothing hidden at VRChat while VRChat is only starting");
    // The account's own location arrives (the pipeline's user-location): now VRChat hides it.
    c.update(|s| s.launcher.me = Some(orbiters_core::launcher::Friend { id: "usr_test".into(), whereabouts: Whereabouts::World(place.clone()), ..Default::default() }));
    c.launcher_settle_accepted(Some(&client), Some(&by(&c)));
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert!(calls.lock().unwrap()[0].1.ends_with("/auth/user/notifications/not_1/hide"));
    // One that never arrives comes back as a card, and VRChat was told nothing.
    c.update(|s| s.launcher.me = None);
    c.launcher.lock().unwrap().accepted.push(accepted(invite("not_2"), "usr_test", ARRIVE_WITHIN + Duration::from_secs(1)));
    c.launcher_settle_accepted(Some(&client), Some(&by(&c)));
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
    c.launcher_notice(None, failed.clone());
    assert_eq!(c.notices.lock().unwrap().len(), before + 1, "a toast");
    assert_eq!(c.view().launcher.notice, Some(failed.clone()));
    c.launcher_notice_over(None, &failed);
    assert_eq!(c.view().launcher.notice, None, "not kept for ever (the Request pill is pressable again)");
    let sent = Notice::Sent { to: A.into(), action: SentKind::RequestInvite };
    c.launcher_notice(None, sent.clone());
    assert_eq!(c.notices.lock().unwrap().len(), before + 1, "a success is no toast");
    let newer = Notice::Sent { to: B.into(), action: SentKind::Boop };
    c.launcher_notice(None, newer.clone());
    c.launcher_notice_over(None, &sent);
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
    c.launcher_resolve_worlds(&client, &by(&c));
    c.launcher_resolve_worlds(&client, &by(&c));
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
    /// What `install_engine` and the person in Obelisk change: Obelisk here, then VRChat in it.
    got_obelisk: std::sync::atomic::AtomicBool,
    got_vrchat: std::sync::atomic::AtomicBool,
    cannot_install: bool,
    installs: Mutex<u32>,
    /// A Mac that starts the game itself (the engine without a window): what each start was given, whether one is
    /// still under way, why the last one failed.
    direct: bool,
    starts: Mutex<Vec<Vec<String>>>,
    /// The VRChats that run, as their own logs say them.
    players: Mutex<Vec<crate::state::VrchatRunShown>>,
    still_starting: std::sync::atomic::AtomicBool,
    failure: Mutex<Option<String>>,
    opened: Mutex<u32>,
    cannot_open: bool,
    /// The Game session of Home's VRChat whose picture is lost (confirmed), and the restarts asked for.
    lost: Mutex<Option<String>>,
    restarts: Mutex<Vec<String>>,
    refused: Mutex<u32>,
    /// The launch a restart is of, and what it was made with.
    restarted: Mutex<Option<HomeLaunch>>,
    restarted_with: Mutex<Vec<Vec<String>>>,
    /// Why VRChat's log cannot be read here.
    log_problem: Mutex<Option<LogTrouble>>,
    /// How often the program was got before a start, and whether a start was held then.
    ready: Mutex<Vec<bool>>,
    ctl: Mutex<Option<std::sync::Weak<Controller>>>,
    /// The process its newest launch says it became (none: the first that runs).
    launched: Mutex<Option<Option<u32>>>,
    /// The engine's files no game starts on, and the repairs asked for.
    engine: Mutex<Option<orbiters_core::obelisk::EngineProblem>>,
    repairs: Mutex<u32>,
    hold_repair: std::sync::atomic::AtomicBool,
}
impl Machine for PretendMac {
    fn picture_lost(&self) -> Option<String> {
        self.lost.lock().unwrap().clone()
    }
    fn engine_problem(&self) -> Option<orbiters_core::obelisk::EngineProblem> {
        self.engine.lock().unwrap().clone()
    }
    fn repair_engine(&self, progress: &dyn Fn(Option<u8>), _stop: &std::sync::atomic::AtomicBool) -> Result<(), String> {
        *self.repairs.lock().unwrap() += 1;
        progress(Some(40));
        while self.hold_repair.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if self.cannot_install {
            return Err("no network".into());
        }
        *self.engine.lock().unwrap() = None;
        Ok(())
    }
    fn restart_lost(&self, session: &str, may: &dyn Fn(&HomeLaunch) -> Result<Vec<String>, String>) -> Result<(), String> {
        if self.picture_lost().as_deref() != Some(session) {
            return Err("checked again: not lost".into());
        }
        let launch = self.restarted.lock().unwrap().clone().unwrap_or(HomeLaunch { account: Some("usr_of_the_start".into()), ..Default::default() });
        match may(&launch) {
            Ok(args) => self.restarted_with.lock().unwrap().push(args),
            Err(_) => *self.refused.lock().unwrap() += 1,
        }
        self.restarts.lock().unwrap().push(session.to_owned());
        Ok(())
    }
    fn running(&self) -> Vec<(u32, bool)> {
        self.running.lock().unwrap().clone()
    }
    fn roster(&self) -> Option<Vec<crate::state::VrchatRunShown>> {
        Some(self.players.lock().unwrap().clone())
    }
    fn newest_profile(&self) -> Option<u32> {
        self.starts.lock().unwrap().last().map(|args| orbiters_core::vrchat_launch::profile_of(args))
    }
    fn newest_launch(&self) -> Option<(u64, Option<u32>)> {
        let starts = self.starts.lock().unwrap().len() as u64;
        let became = self.launched.lock().unwrap().unwrap_or_else(|| self.running.lock().unwrap().first().map(|run| run.0));
        (starts > 0).then_some((starts, became))
    }
    fn make_ready(&self) -> Result<(), String> {
        let held = self.ctl.lock().unwrap().as_ref().and_then(std::sync::Weak::upgrade).is_some_and(|c| c.launcher_started.try_lock().is_err());
        self.ready.lock().unwrap().push(held);
        Ok(())
    }
    fn log_problem(&self) -> Option<LogTrouble> {
        self.log_problem.lock().unwrap().clone()
    }
    fn installed(&self) -> bool {
        !self.engine_missing() && (self.vrchat || self.got_vrchat.load(Ordering::Relaxed))
    }
    fn engine_missing(&self) -> bool {
        !self.obelisk && !self.got_obelisk.load(Ordering::Relaxed)
    }
    fn by_hand(&self) -> bool {
        true
    }
    fn install_engine(&self, progress: &dyn Fn(EngineSetup), _stop: &std::sync::atomic::AtomicBool) -> Result<(), String> {
        *self.installs.lock().unwrap() += 1;
        progress(EngineSetup::Downloading { percent: 50 });
        if self.cannot_install {
            return Err("no network".into());
        }
        progress(EngineSetup::Installing);
        self.got_obelisk.store(true, Ordering::Relaxed);
        if self.direct {
            // The whole set-up: the sign-in, then the game.
            progress(EngineSetup::SignIn { qr_url: "https://s.team/q/1/2".into() });
            progress(EngineSetup::Game { game: true, percent: Some(50) });
            self.got_vrchat.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
    fn direct(&self) -> bool {
        self.direct
    }
    fn starting(&self) -> bool {
        self.still_starting.load(Ordering::Relaxed)
    }
    fn start_failure(&self) -> Option<String> {
        self.failure.lock().unwrap().take()
    }
    fn start(&self, args: &[String]) -> Result<(), String> {
        if self.direct {
            self.starts.lock().unwrap().push(args.to_vec());
            return if self.cannot_open { Err("no".into()) } else { Ok(()) };
        }
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

/// A public instance, anyone's.
fn open_place() -> Place {
    Place { instance_id: "7~region(eu)".into(), access: orbiters_core::launcher::InstanceAccess::Public, ..place() }
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
    let errand = Errand::plain(LaunchMode::Desktop, None).to(Some(place()), Some("usr_test"));
    c.launcher.lock().unwrap().pending = Some(Pending { errand, since: long_ago, proof: false, before: Vec::new(), launch: None, seen: None });
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
    c.launcher_settle_accepted(Some(&client), Some(&by(&c)));
    assert!(c.view().launcher.invites.is_empty());
    assert!(calls.lock().unwrap().is_empty());
    // An account change drops them.
    c.launcher.lock().unwrap().accepted.clear();
    // B's own, arrived: the hide fails once (503), is kept, and goes a minute later.
    c.launcher.lock().unwrap().accepted.push(accepted(invite("not_b"), "usr_test", Duration::from_secs(5)));
    c.launcher_settle_accepted(Some(&client), Some(&by(&c)));
    assert_eq!(calls.lock().unwrap().len(), 1);
    {
        let mut rt = c.launcher.lock().unwrap();
        assert_eq!(rt.accepted.len(), 1, "a failed hide is not dropped");
        rt.accepted[0].hide_again = Some(Instant::now());
    }
    c.launcher_settle_accepted(Some(&client), Some(&by(&c)));
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
    c.launcher_settle_accepted(Some(&client), Some(&by(&c)));
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
    // (A side session's profile is never the default one.)
    chips(&c, &[1]);
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
    let by = by(&c);
    c.launcher_commit_read(&by, state.clone());
    assert_eq!(c.publishes.load(Ordering::Relaxed), before, "nothing new: nothing published");
    let mut moved = state;
    moved.friends[0].status_text = "new".into();
    c.launcher_commit_read(&by, moved);
    assert_eq!(c.publishes.load(Ordering::Relaxed), before + 1, "a change is published");
}

#[test]
fn a_second_play_on_a_mac_keeps_the_first_destination_and_an_account_change_drops_it() {
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(machine.clone());
    // The press is made on the signed-in account's own list.
    let (_, _) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    c.launcher_game(machine.as_ref());
    c.launcher_do(mine(Action::Play { mode: LaunchMode::Desktop, destination: Some(place()) }));
    // A plain Play pressed again while the person is still to press Play in Obelisk.
    c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None });
    assert!(c.launcher.lock().unwrap().pending.as_ref().is_some_and(|p| p.errand.destination.as_ref().is_some_and(|p| p.world_id == W) && p.errand.account.as_deref() == Some("usr_test")), "the first destination stays, bound to its account");
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
    c.launcher_settle_accepted(Some(&client), Some(&by(&c)));
    assert!(c.view().launcher.invites.is_empty(), "its card does not come back");
    // Accepted while in that very room: leaving, then coming back, is the arrival.
    let at = |p: Option<Place>| c.update(|s| s.launcher.me = Some(orbiters_core::launcher::Friend { id: "usr_test".into(), whereabouts: p.map_or(Whereabouts::Private, Whereabouts::World), ..Default::default() }));
    let mut a = accepted(invite("not_2"), "usr_test", Duration::from_secs(5));
    a.from = Some((place.world_id.clone(), place.instance_id.clone()));
    c.launcher.lock().unwrap().accepted.push(a);
    at(Some(place.clone()));
    c.launcher_settle_accepted(Some(&client), Some(&by(&c)));
    assert!(calls.lock().unwrap().is_empty(), "still where it was at the press");
    let elsewhere = launcher_feed::place(&format!("{W}:1~region(eu)"), "").unwrap();
    at(Some(elsewhere));
    c.launcher_settle_accepted(Some(&client), Some(&by(&c)));
    at(Some(place.clone()));
    c.launcher_settle_accepted(Some(&client), Some(&by(&c)));
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
    fn whose(&self, pid: u32) -> super::super::travel::Side {
        if pid == 20 { super::super::travel::Side::Bridge } else { super::super::travel::Side::Person }
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

// Reads this computer's VRChats through the roster and `launcher_game`, which a Mac does not (VRChat is Obelisk's there,
// and Home starts no second profile): Windows only.
#[cfg(not(target_os = "macos"))]
#[test]
fn a_second_vrchat_starts_on_a_free_profile_beside_the_first_and_never_on_a_running_one() {
    // REQUEST15 7a. VRChat runs on profile 0 (in VR); nothing reaches a real VRChat: the pretend computer records.
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    machine.running.lock().unwrap().push((10, false));
    let c = controller(machine.clone());
    let refused = c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 0 });
    assert!(matches!(&refused, Some(Notice::Error(t)) if t.contains('0')), "never two on one profile: {refused:?}");
    assert!(machine.beside.lock().unwrap().is_empty() && machine.started.lock().unwrap().is_empty());
    assert_eq!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch }));
    let beside = machine.beside.lock().unwrap().clone();
    assert_eq!(beside.len(), 1, "through VRChat's own launcher, not Steam (which starts a game once)");
    assert!(beside[0].contains(&"--profile=1".to_string()) && beside[0].contains(&"--no-vr".to_string()), "{:?}", beside[0]);
    assert!(machine.started.lock().unwrap().is_empty());
    assert_eq!(c.view().launcher.profiles.iter().find(|p| p.profile == 1).and_then(|p| p.starting), Some(LaunchMode::Desktop), "the chip says it is starting");
    // While it starts, a third waits its turn.
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Vr, profile: 2 }), Some(Notice::Error(_))));
    // It is up on profile 1: the start is over, and profile 1 is now refused too.
    machine.running.lock().unwrap().push((11, true));
    machine.profiles.lock().unwrap().insert(11, 1);
    c.update(|s| s.vrchat.runs = vec![crate::state::VrchatRunShown { pid: 10, profile: 0, profile_known: true, ..Default::default() }, crate::state::VrchatRunShown { pid: 11, profile: 1, profile_known: true, account: Some("Sample Alt".into()), ..Default::default() }]);
    c.launcher_game(machine.as_ref());
    let profiles = c.view().launcher.profiles.clone();
    assert_eq!(profiles.iter().map(|p| (p.profile, p.running, p.starting)).collect::<Vec<_>>(), [(0, Some(LaunchMode::Vr), None), (1, Some(LaunchMode::Desktop), None)]);
    assert_eq!(c.view().settings.launch.profiles.get(&1).map(String::as_str), Some("Sample Alt"), "the account seen on it is remembered");
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Vr, profile: 1 }), Some(Notice::Error(_))));
    assert_eq!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Vr, profile: 2 }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch }));
    assert_eq!(machine.beside.lock().unwrap().len(), 2);
}

#[test]
fn the_first_vrchat_on_a_profile_goes_through_steam_with_that_profile() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    assert_eq!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Vr, profile: 2 }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch }));
    let started = machine.started.lock().unwrap().clone();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].last().map(String::as_str), Some("--profile=2"));
    assert!(!started[0].contains(&"--no-vr".to_string()));
    assert_eq!(c.view().launcher.game, Game::Starting(LaunchMode::Vr));
    // Profile 0 is VRChat's own start: no `--profile` at all.
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 0 });
    assert!(!machine.started.lock().unwrap()[0].iter().any(|a| a.starts_with("--profile")));
    // Out of what Home offers: nothing starts.
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 99 }), Some(Notice::Error(_))));
}

#[test]
fn the_profile_choice_lists_used_running_and_starting_profiles_with_their_accounts() {
    let known: BTreeMap<u32, String> = [(1, "Sample Alt".to_string())].into();
    let roster = [crate::state::VrchatRunShown { pid: 10, profile: 0, profile_known: true, account: Some("Sample Me".into()), ..Default::default() }, crate::state::VrchatRunShown {
        pid: 12,
        profile: 3,
        profile_known: true,
        driver: crate::state::VrchatDriverShown::Agent { session: "s1".into() },
        ..Default::default()
    }];
    let got = game_profiles(&known, &[(10, false, Some(0)), (12, true, Some(3))], &roster, &[(2, LaunchMode::Desktop)]);
    let shown: Vec<(u32, Option<&str>, Option<LaunchMode>, Option<LaunchMode>, bool)> = got.iter().map(|p| (p.profile, p.account.as_deref(), p.running, p.starting, p.assistant)).collect();
    assert_eq!(
        shown,
        [
            (0, Some("Sample Me"), Some(LaunchMode::Vr), None, false),
            (1, Some("Sample Alt"), None, None, false),
            (2, None, None, Some(LaunchMode::Desktop), false),
            (3, None, Some(LaunchMode::Desktop), None, true),
        ]
    );
    assert_eq!(game_profiles(&BTreeMap::new(), &[], &[], &[]).iter().map(|p| p.profile).collect::<Vec<_>>(), [0], "profile 0 always");
}

// --- REQUEST15 7b: the account's own profile, against recorded replies only (nothing reaches VRChat) ---

fn me_signed_in(c: &Shared) {
    signed_in_list(c);
    c.update(|s| s.launcher.me = Some(orbiters_core::launcher::Friend { id: "usr_test".into(), name: "Sample Me".into(), ..Default::default() }));
    // Its own profile as VRChat gave it (a bio is only changed from that one).
    c.update(|s| s.launcher.mine = Some(orbiters_core::launcher::MyProfile { loaded: true, ..Default::default() }));
}

fn saving(c: &Shared) -> Option<orbiters_core::launcher::Saving> {
    c.view().launcher.mine.as_ref().and_then(|m| m.saving.clone())
}

#[test]
fn an_own_profile_change_goes_on_the_press_only_with_the_lists_account_within_vrchats_limits() {
    use orbiters_core::launcher::{ProfileEdit, ProfilePart, Saving, Status};
    let c = controller(Arc::new(Pretend::default()));
    let (_, calls) = c.test_vrchat(vec![
        (200, json!({"id": "usr_test", "status": "busy", "statusDescription": "in Unity"})),
        (400, json!({"error": {"message": "Bio contains a disallowed word", "status_code": 400}})),
        (200, json!({"id": "usr_test", "bio": "new bio"})),
    ]);
    me_signed_in(&c);
    // A press on the list of the account: sent, and what VRChat answered shows.
    assert_eq!(c.launcher_do(mine(Action::EditProfile(ProfileEdit::Status(Status::Busy)))), None);
    assert_eq!(c.view().launcher.me.as_ref().unwrap().status, Status::Busy);
    assert_eq!(c.view().launcher.me.as_ref().unwrap().status_text, "in Unity");
    assert_eq!(saving(&c), Some(Saving::Saved(ProfilePart::Status)));
    // At once again: our own pace, nothing sent.
    c.launcher_do(mine(Action::EditProfile(ProfileEdit::StatusText("afk".into()))));
    assert!(matches!(saving(&c), Some(Saving::Failed(ProfilePart::StatusText, t)) if t.contains('s')), "{:?}", saving(&c));
    c.launcher.lock().unwrap().mine_next = None;
    // Past VRChat's limits: said, nothing sent.
    c.launcher_do(mine(Action::EditProfile(ProfileEdit::StatusText("x".repeat(33)))));
    assert!(matches!(saving(&c), Some(Saving::Failed(ProfilePart::StatusText, _))));
    c.launcher_do(mine(Action::EditProfile(ProfileEdit::Bio("x".repeat(513)))));
    assert!(matches!(saving(&c), Some(Saving::Failed(ProfilePart::Bio, _))));
    c.launcher_do(mine(Action::EditProfile(ProfileEdit::Status(Status::Offline))));
    assert!(matches!(saving(&c), Some(Saving::Failed(ProfilePart::Status, _))));
    // Refused by VRChat: its words, and the bio is not changed.
    c.launcher_do(mine(Action::EditProfile(ProfileEdit::Bio("new bio".into()))));
    assert!(matches!(saving(&c), Some(Saving::Failed(ProfilePart::Bio, t)) if t.contains("disallowed")), "{:?}", saving(&c));
    assert_ne!(c.view().launcher.mine.as_ref().unwrap().bio, "new bio");
    c.launcher.lock().unwrap().mine_next = None;
    c.launcher_do(mine(Action::EditProfile(ProfileEdit::Bio("new bio".into()))));
    assert_eq!(saving(&c), Some(Saving::Saved(ProfilePart::Bio)));
    assert_eq!(c.view().launcher.mine.as_ref().unwrap().bio, "new bio");
    // Without the list's account, or on another account's list: nothing goes.
    c.launcher.lock().unwrap().mine_next = None;
    c.launcher_do(Action::EditProfile(ProfileEdit::Status(Status::JoinMe)));
    c.launcher_do(Action::As { account: "usr_other".into(), action: Box::new(Action::EditProfile(ProfileEdit::Status(Status::JoinMe))) });
    // Neither press is this account's: nothing of theirs lands on this card either (REQUEST15 fix B3).
    assert_eq!(saving(&c), Some(Saving::Saved(ProfilePart::Bio)));
    assert_eq!(c.view().launcher.me.as_ref().unwrap().status, Status::Busy);
    let calls: Vec<String> = calls.lock().unwrap().iter().map(|(m, u)| format!("{m} {}", u.trim_start_matches("https://api.vrchat.cloud/api/1"))).collect();
    assert_eq!(calls, ["PUT /users/usr_test", "PUT /profile/usr_test", "PUT /profile/usr_test"]);
}

#[test]
fn the_own_profile_and_pictures_are_read_for_the_card() {
    use orbiters_core::launcher::{BannerChoice, PictureKind, ProfileEdit, ProfilePart, Saving};
    let c = controller(Arc::new(Pretend::default()));
    let (_, calls) = c.test_vrchat(vec![
        (200, json!({"id": "usr_test", "bio": "hello", "bannerType": "customImage", "bannerCustomUrl": "https://api.vrchat.cloud/api/1/file/file_b/1/file", "userIcon": "", "hasVrcPlus": true, "currentAvatarThumbnailImageUrl": "https://api.vrchat.cloud/avatar"})),
        (200, json!([{"id": "file_1", "versions": [{"version": 0}, {"version": 1, "file": {"url": "https://api.vrchat.cloud/api/1/file/file_1/1/file"}}]}, {"id": "file_2", "versions": []}])),
        // Listed again right before the change: the picture must be the account's own now.
        (200, json!([{"id": "file_1", "versions": [{"version": 1, "file": {"url": "https://api.vrchat.cloud/api/1/file/file_1/1/file"}}]}])),
        (200, json!({"id": "usr_test", "userIcon": "https://api.vrchat.cloud/api/1/file/file_1/1/file"})),
    ]);
    me_signed_in(&c);
    c.launcher_do(mine(Action::ReadMine));
    let m = c.view().launcher.mine.clone().unwrap();
    assert_eq!((m.bio.as_str(), m.banner, m.supporter, m.avatar_picture.as_str()), ("hello", BannerChoice::Custom, true, "https://api.vrchat.cloud/avatar"));
    c.launcher_do(mine(Action::ListPictures(PictureKind::Icons)));
    let icons = c.view().launcher.mine.clone().unwrap().icons.unwrap();
    assert_eq!(icons.len(), 1, "a file with no version is no picture");
    assert_eq!(icons[0].url, "https://api.vrchat.cloud/api/1/file/file_1/1/file");
    c.launcher_do(mine(Action::EditProfile(ProfileEdit::Picture(icons[0].url.clone()))));
    assert_eq!(saving(&c), Some(Saving::Saved(ProfilePart::Picture)));
    assert_eq!(c.view().launcher.me.as_ref().unwrap().look.icon, icons[0].url);
    let calls: Vec<String> = calls.lock().unwrap().iter().map(|(m, u)| format!("{m} {}", u.trim_start_matches("https://api.vrchat.cloud/api/1"))).collect();
    assert_eq!(calls, ["GET /profile/usr_test?asSelf=true", "GET /files?tag=icon&n=100", "GET /files?tag=icon&n=100", "PUT /profile/usr_test"]);
}

// --- REQUEST15 fix B4: one VRChat per profile, whichever of Orbiters' starts asks ---

fn running_profile_0() -> (Arc<Pretend>, Shared) {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    machine.running.lock().unwrap().push((10, false));
    let c = controller(machine.clone());
    *machine.ctl.lock().unwrap() = Some(Arc::downgrade(&c));
    (machine, c)
}

#[test]
fn a_start_claims_its_profile_before_it_starts_and_keeps_it_while_vrchat_is_slow() {
    let (machine, c) = running_profile_0();
    assert_eq!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch }));
    assert_eq!(*machine.claimed_at_start.lock().unwrap(), [true], "claimed before the start, not after it");
    // VRChat takes more than a minute (Easy Anti-Cheat, an update): the claim still holds, the next look keeps it.
    for claim in c.launcher.lock().unwrap().starting.iter_mut() {
        claim.at -= Duration::from_secs(70);
    }
    c.launcher_game(machine.as_ref());
    assert!(c.launcher_claimed(1));
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Vr, profile: 1 }), Some(Notice::Error(_))));
    assert_eq!(machine.beside.lock().unwrap().len(), 1, "never a second VRChat on profile 1");
}

#[test]
fn presses_at_the_same_moment_start_one_vrchat_on_a_profile() {
    let (machine, c) = running_profile_0();
    let threads: Vec<_> = (0..8).map(|_| {
        let c = c.clone();
        std::thread::spawn(move || c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }))
    }).collect();
    threads.into_iter().for_each(|t| {
        t.join().unwrap();
    });
    assert_eq!(machine.beside.lock().unwrap().len(), 1);
}

#[test]
fn the_bridge_and_go_there_claim_the_profiles_home_starts_on() {
    // A bridge session holds profile 2 (its mark is put before its VRChat starts): Home never starts there.
    let (machine, c) = running_profile_0();
    c.sides.put(crate::bridge::sessions::SideMark { session: "p2-test".into(), profile: 2, osc_in: 9020, osc_out: 9021, pid: None, log: None, query: None });
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 2 }), Some(Notice::Error(_))));
    assert!(machine.beside.lock().unwrap().is_empty());
    // Another of Orbiters' starts (the bridge's main session, Go there) claimed profile 0 while none runs: Home's Play and
    // profile choice both wait, whatever the clock of the last Play says.
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    let claim = c.launcher_claim(0, LaunchMode::Vr).unwrap();
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Vr, destination: None }), Some(Notice::Error(_))));
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Vr, profile: 0 }), Some(Notice::Error(_))));
    assert!(machine.started.lock().unwrap().is_empty());
    // A start that failed gives its claim back.
    c.launcher_unclaim(claim);
    assert!(!c.launcher_claimed(0));
}

// --- REQUEST15 fix B5: a VRChat whose profile is not known yet is never taken for profile 0 ---

// Reads this computer's VRChats through the roster and `launcher_game`, which a Mac does not (VRChat is Obelisk's there,
// and Home starts no second profile): Windows only.
#[cfg(not(target_os = "macos"))]
#[test]
fn a_vrchat_not_yet_identified_is_not_profile_0_and_holds_play() {
    // Profile 1 started outside Orbiters: in the roster before its log said its arguments (the roster's 0 means "not
    // known" then), while its command line says --profile=1.
    let (machine, c) = running_profile_0();
    machine.running.lock().unwrap().push((11, true));
    machine.profiles.lock().unwrap().insert(11, 1);
    c.update(|s| s.vrchat.runs = vec![crate::state::VrchatRunShown { pid: 10, profile: 0, profile_known: true, ..Default::default() }, crate::state::VrchatRunShown { pid: 11, profile: 0, profile_known: false, ..Default::default() }]);
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Vr, profile: 1 }), Some(Notice::Error(_))), "its command line says profile 1");
    c.launcher_game(machine.as_ref());
    assert!(c.view().launcher.profiles.iter().any(|p| p.profile == 1 && p.running.is_some()));
    // Neither its log nor its command line said anything yet: no profile may be started until it does.
    machine.unread.lock().unwrap().push(11);
    for profile in [1, 2] {
        assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Vr, profile }), Some(Notice::Error(_))), "{profile}");
    }
    assert!(machine.beside.lock().unwrap().is_empty() && machine.started.lock().unwrap().is_empty());
}

// --- REQUEST15 fix B3: what was read or sent for one account never lands on another's card ---

#[test]
fn an_answer_for_account_a_never_lands_on_bs_card() {
    use orbiters_core::launcher::{OwnPicture, ProfilePart, Saving};
    let c = controller(Arc::new(Pretend::default()));
    let account = by(&c);
    c.update(|s| {
        s.vrchat_login.user.as_mut().unwrap().id = "usr_b".into();
        s.launcher.source = Source::SignedIn { user_id: "usr_b".into(), name: String::new() };
    });
    let (a, b) = (By { user: "usr_a".into(), ..account.clone() }, By { user: "usr_b".into(), ..account });
    // A's picture list, or A's "saved", arriving after the switch to B.
    let landed = c.launcher_mine_publish(&a, |m| m.icons = Some(vec![OwnPicture { id: "file_a".into(), url: "https://api.vrchat.cloud/api/1/file/file_a/1/file".into() }]));
    assert!(!landed);
    c.launcher_mine_saving(&a, Saving::Saved(ProfilePart::Bio), None);
    assert!(c.view().launcher.mine.is_none(), "B's card has nothing of A's");
    assert!(c.launcher_mine_publish(&b, |m| m.bio = "B".into()));
    assert_eq!(c.view().launcher.mine.as_ref().unwrap().bio, "B");
}

// --- REQUEST15 fix SF-images: a picture is sent only when it is one of the account's own files right now ---

#[test]
fn a_picture_not_among_the_accounts_own_files_is_never_sent() {
    use orbiters_core::launcher::{BannerChoice, ProfileEdit, ProfilePart, Saving};
    let c = controller(Arc::new(Pretend::default()));
    let mine_file = "https://api.vrchat.cloud/api/1/file/file_mine/1/file";
    let theirs = "https://api.vrchat.cloud/api/1/file/file_someone_elses/1/file";
    let (_, calls) = c.test_vrchat(vec![
        (200, json!([{"id": "file_mine", "versions": [{"version": 1, "file": {"url": mine_file}}]}])),
        (200, json!([{"id": "file_mine", "versions": [{"version": 1, "file": {"url": mine_file}}]}])),
        (200, json!({"id": "usr_test", "bannerType": "customImage", "bannerCustomUrl": mine_file})),
    ]);
    me_signed_in(&c);
    // Another person's file on the same host, as an icon: refused, nothing written.
    c.launcher_do(mine(Action::EditProfile(ProfileEdit::Picture(theirs.into()))));
    assert!(matches!(saving(&c), Some(Saving::Failed(ProfilePart::Picture, _))));
    c.launcher.lock().unwrap().mine_next = None;
    // The account's own gallery picture as the banner: listed, then sent.
    c.launcher_do(mine(Action::EditProfile(ProfileEdit::Banner { choice: BannerChoice::Custom, colour: None, custom: mine_file.into() })));
    assert_eq!(saving(&c), Some(Saving::Saved(ProfilePart::Banner)));
    let calls: Vec<String> = calls.lock().unwrap().iter().map(|(m, u)| format!("{m} {}", u.trim_start_matches("https://api.vrchat.cloud/api/1"))).collect();
    assert_eq!(calls, ["GET /files?tag=icon&n=100", "GET /files?tag=gallery&n=100", "PUT /profile/usr_test"], "no PUT for the foreign picture");
}

// --- REQUEST15 fix SF-refresh: a scheduled read keeps the own profile and VRChat's profiles; a list failure is no change ---

#[test]
fn a_full_read_keeps_the_own_profile_and_the_game_profiles() {
    use orbiters_core::launcher::{GameProfile, MyProfile};
    let c = controller(Arc::new(Pretend::default()));
    me_signed_in(&c);
    c.update(|s| {
        s.launcher.mine = Some(MyProfile { bio: "kept".into(), ..MyProfile::default() });
        s.launcher.profiles = vec![GameProfile { profile: 1, running: Some(LaunchMode::Vr), ..GameProfile::default() }];
    });
    let same = LauncherState { source: c.view().launcher.source.clone(), ..LauncherState::default() };
    let account = by(&c);
    c.launcher_commit_read(&account, same);
    assert_eq!(c.view().launcher.mine.as_ref().map(|m| m.bio.as_str()), Some("kept"));
    assert_eq!(c.view().launcher.profiles.len(), 1);
    // Another account's read: its own profile is not the previous one's; the computer's profiles stay.
    c.update(|s| s.vrchat_login.user.as_mut().unwrap().id = "usr_other".into());
    assert!(c.view().launcher.mine.is_none(), "the list on show is not this account's");
    c.launcher_commit_read(&By { user: "usr_other".into(), ..account }, LauncherState { source: Source::SignedIn { user_id: "usr_other".into(), name: String::new() }, ..LauncherState::default() });
    assert!(c.view().launcher.mine.is_none());
    assert_eq!(c.view().launcher.profiles.len(), 1);
}

#[test]
fn a_list_that_did_not_come_is_said_in_its_chooser_not_as_a_failed_change() {
    use orbiters_core::launcher::PictureKind;
    let c = controller(Arc::new(Pretend::default()));
    let _ = c.test_vrchat(vec![(503, json!({}))]);
    me_signed_in(&c);
    c.launcher_do(mine(Action::ListPictures(PictureKind::Gallery)));
    let m = c.view().launcher.mine.clone().unwrap();
    assert!(m.saving.is_none(), "no change failed");
    assert!(matches!(m.list_failed, Some((PictureKind::Gallery, _))));
    assert!(m.listing.is_none());
}

// --- REQUEST15 second pass, blocker 6: every start needs its claim ---

#[test]
fn play_and_join_start_nothing_while_another_profile_is_still_starting() {
    // Profile 1 asked for with no VRChat running (through Steam); a minute later its process is still not there.
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    assert_eq!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch }));
    *c.launcher_started.lock().unwrap() = Some(Instant::now() - Duration::from_secs(70));
    // Play, and Join with a destination (the same path while nothing runs): the claim is refused, nothing starts.
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Vr, destination: None }), Some(Notice::Error(_))));
    let place = launcher_feed::place(&format!("{W}:1~region(eu)"), "").unwrap();
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place) }), Some(Notice::Error(_))));
    assert_eq!(machine.started.lock().unwrap().len(), 1, "only profile 1's start");
}

// --- REQUEST15 second pass, blocker 6: a look ends only the claims it saw ---

#[test]
fn a_look_across_an_old_vrchats_exit_never_ends_the_newer_claim() {
    // A look starts: no claim yet, and it sees the old VRChat of profile 1, still quitting.
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    let observed: Vec<u64> = c.launcher.lock().unwrap().starting.iter().map(|claim| claim.id).collect();
    let seen = vec![(10, true, Some(1))];
    // The old VRChat is gone; the person starts profile 1 again, which claims it.
    let claim = c.launcher_claim(1, LaunchMode::Desktop).unwrap();
    // The look ends: what it saw of profile 1 was the old process, before the claim.
    c.launcher_retire_claims(&observed, &seen);
    assert!(c.launcher_claimed(1), "the newer claim stays until a look made after it sees profile 1");
    // The next look, made after the claim, sees the new VRChat on profile 1: the claim ends.
    let observed: Vec<u64> = c.launcher.lock().unwrap().starting.iter().map(|claim| claim.id).collect();
    assert_eq!(observed, [claim]);
    c.launcher_retire_claims(&observed, &[(11, true, Some(1))]);
    assert!(!c.launcher_claimed(1));
}

// --- REQUEST15 second pass, blocker 6: a start that never showed is not retried behind the person's back ---

#[test]
fn a_start_not_seen_after_three_minutes_needs_the_persons_second_press() {
    let (machine, c) = running_profile_0();
    let claim = c.launcher_claim(1, LaunchMode::Desktop).unwrap();
    for held in c.launcher.lock().unwrap().starting.iter_mut() {
        held.at -= Duration::from_secs(200);
    }
    // A look past the claim's time keeps it (lapsed): it still bars its profile.
    c.launcher_retire_claims(&[claim], &[(10, false, Some(0))]);
    assert!(!c.launcher_claimed(1) && c.launcher.lock().unwrap().starting.len() == 1);
    // Not for another profile.
    assert_eq!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 2 }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch }));
    c.launcher.lock().unwrap().starting.retain(|held| held.profile == 1);
    // The bridge, which no person drives, never overrides it.
    assert!(c.launcher_claim_unattended(1, LaunchMode::Desktop).is_err());
    assert!(c.launcher_claim_unattended(1, LaunchMode::Desktop).is_err());
    // The person: told first, nothing started; a second press goes.
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Error(t)) if t.contains('1')));
    let before = machine.beside.lock().unwrap().len();
    assert_eq!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch }));
    assert_eq!(machine.beside.lock().unwrap().len(), before + 1);
}

// --- REQUEST15 second pass, S4: unreadable command lines are asked again; only confirmed profiles are labelled ---

#[test]
fn an_unreadable_command_line_is_asked_again_a_few_times_and_a_reused_id_anew() {
    let mut lines = CmdLines::default();
    let t0 = Instant::now();
    assert_eq!(lines.due(&[(11, 500)], t0), [11]);
    lines.record(11, 500, &[], t0);
    assert_eq!(lines.get(11), (false, None));
    assert!(lines.due(&[(11, 500)], t0 + Duration::from_secs(1)).is_empty(), "not at every look");
    let mut at = t0;
    for _ in 1..CMD_TRIES {
        at += CMD_RETRY;
        assert_eq!(lines.due(&[(11, 500)], at), [11]);
        lines.record(11, 500, &[], at);
    }
    assert!(lines.due(&[(11, 500)], at + CMD_RETRY).is_empty(), "a bounded number of tries");
    // The same id for another process (another start time): read anew, and its profile is known.
    assert_eq!(lines.due(&[(11, 900)], at), [11]);
    lines.record(11, 900, &["VRChat.exe".into(), "--profile=2".into(), "--no-vr".into()], at);
    assert_eq!(lines.get(11), (true, Some(2)));
    assert!(lines.due(&[(11, 900)], at + CMD_RETRY * 3).is_empty(), "a known one is not read again");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn only_a_confirmed_profile_is_labelled_and_an_unresolved_vrchat_is_said() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    machine.running.lock().unwrap().push((11, true));
    machine.unread.lock().unwrap().push(11);
    let c = controller(machine.clone());
    // In the roster before its log said its arguments: profile 0 there means "not known".
    c.update(|s| s.vrchat.runs = vec![crate::state::VrchatRunShown { pid: 11, profile: 0, profile_known: false, account: Some("Sample Alt".into()), ..Default::default() }]);
    c.launcher_game(machine.as_ref());
    assert!(c.view().settings.launch.profiles.get(&0).is_none(), "no label from an unconfirmed profile");
    assert_eq!(c.view().launcher.unresolved, 1);
    // Its log says profile 1: labelled there, and nothing is unresolved.
    c.update(|s| s.vrchat.runs = vec![crate::state::VrchatRunShown { pid: 11, profile: 1, profile_known: true, account: Some("Sample Alt".into()), ..Default::default() }]);
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().settings.launch.profiles.get(&1).map(String::as_str), Some("Sample Alt"));
    assert_eq!(c.view().launcher.unresolved, 0);
}

// --- REQUEST15 second pass, S6: a placeholder's empty bio is never saved over the account's ---

#[test]
fn a_bio_is_never_saved_from_a_placeholder_profile() {
    use orbiters_core::launcher::{PictureKind, ProfileEdit, ProfilePart, Saving};
    let c = controller(Arc::new(Pretend::default()));
    let (_, calls) = c.test_vrchat(vec![
        (200, json!([])),
        (200, json!({"id": "usr_test", "bio": "the real bio"})),
        (200, json!({"id": "usr_test", "bio": "new"})),
    ]);
    me_signed_in(&c);
    // The profile's first read failed (nothing in the state); a picture list made a placeholder.
    c.update(|s| s.launcher.mine = None);
    c.launcher_do(mine(Action::ListPictures(PictureKind::Icons)));
    let placeholder = c.view().launcher.mine.clone().unwrap();
    assert!(!placeholder.loaded && placeholder.bio.is_empty());
    c.launcher_do(mine(Action::EditProfile(ProfileEdit::Bio(String::new()))));
    assert!(matches!(saving(&c), Some(Saving::Failed(ProfilePart::Bio, _))), "no bio from a placeholder");
    // Read again: the real profile replaces the placeholder, its lists kept.
    c.launcher_do(mine(Action::ReadMine));
    let read = c.view().launcher.mine.clone().unwrap();
    assert!(read.loaded && read.bio == "the real bio" && read.icons.is_some());
    c.launcher.lock().unwrap().mine_next = None;
    c.launcher_do(mine(Action::EditProfile(ProfileEdit::Bio("new".into()))));
    assert_eq!(saving(&c), Some(Saving::Saved(ProfilePart::Bio)));
    let calls: Vec<String> = calls.lock().unwrap().iter().map(|(m, u)| format!("{m} {}", u.trim_start_matches("https://api.vrchat.cloud/api/1"))).collect();
    assert_eq!(calls, ["GET /files?tag=icon&n=100", "GET /profile/usr_test?asSelf=true", "PUT /profile/usr_test"], "no PUT from the placeholder");
}

// --- REQUEST15 third pass, blocker 2: a list left on show after its account signed out accepts nothing ---

#[test]
fn after_a_sign_out_the_old_list_neither_keeps_a_dismissal_nor_opens_a_profile() {
    let c = controller(Arc::new(Pretend::default()));
    // A's list is on show, A's session is gone (signed out): the launcher has not looked again yet.
    c.update(|s| {
        s.launcher.source = Source::SignedIn { user_id: "usr_a".into(), name: String::new() };
        s.launcher.boops = vec![orbiters_core::launcher::Boop { id: "not_boop_a".into(), from_id: A.into(), from_name: "Nova".into(), from_icon: String::new(), emoji: None, received_at: 1 }];
        s.launcher.friends = vec![orbiters_core::launcher::Friend { id: A.into(), name: "Nova".into(), note: "A's private note".into(), ..Default::default() }];
    });
    let as_a = |action: Action| Action::As { account: "usr_a".into(), action: Box::new(action) };
    c.launcher_do(as_a(Action::Dismiss { id: "not_boop_a".into() }));
    assert_eq!(c.view().launcher.boops.len(), 1, "nothing of A's list changes");
    assert!(c.launcher.lock().unwrap().dismissed.as_ref().is_none_or(|d| !d.contains_key("not_boop_a")), "no dismissal kept for 30 days");
    c.launcher_do(as_a(Action::OpenProfile { id: A.into() }));
    assert!(c.view().launcher.profile.is_none(), "A's note is not shown from the old row");
}

// --- REQUEST15 third pass, fix2 item 2: a start that never showed is said on Join too, and Go there arms nothing ---

#[test]
fn join_says_a_lapsed_start_and_go_there_never_arms_the_second_press() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    machine.running.lock().unwrap().push((20, false));
    let c = controller(machine.clone());
    // Only a bridge side session runs (on another profile): a Join starts the person's own VRChat (profile 0) through
    // travel.
    chips(&c, &[1]);
    let fake = Arc::new(super::super::travel::tests::Fake { running: vec![20], side_args: vec![20], ..Default::default() });
    *c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
    let claim = c.launcher_claim(0, LaunchMode::Vr).unwrap();
    for held in c.launcher.lock().unwrap().starting.iter_mut() {
        held.at -= Duration::from_secs(200);
    }
    let _ = claim;
    // Go there's own claim (unattended) is refused and arms nothing.
    assert!(c.launcher_claim_unattended(0, LaunchMode::Vr).is_err());
    assert!(c.launcher.lock().unwrap().starting.iter().all(|held| held.confirm_by.is_none()));
    // Join: the person is told what the second press does, and nothing starts.
    let place = launcher_feed::place(&format!("{W}:1~region(eu)"), "").unwrap();
    let first = c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place.clone()) });
    assert!(matches!(&first, Some(Notice::Error(t)) if t.contains("press Play again")), "{first:?}");
    assert!(fake.started.lock().unwrap().is_empty());
    // The second press, told, starts.
    assert_eq!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place) }), Some(Notice::Sent { to: W.into(), action: SentKind::Join }));
    assert_eq!(fake.started.lock().unwrap().len(), 1);
}

// --- REQUEST15 third pass, B6: a start that never showed is asked about however long a Steam update takes ---

#[test]
fn a_start_unseen_for_twenty_minutes_still_asks_the_person_before_another() {
    let (machine, c) = running_profile_0();
    c.launcher_claim(1, LaunchMode::Desktop).unwrap();
    for held in c.launcher.lock().unwrap().starting.iter_mut() {
        held.at -= Duration::from_secs(20 * 60);
    }
    // Looks go by; the claim is not dropped by its age.
    c.launcher_retire_claims(&[], &[(10, false, Some(0))]);
    c.launcher_game(machine.as_ref());
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Error(t)) if t.contains("press Play again")));
    assert!(machine.beside.lock().unwrap().is_empty(), "no second start without the person's say");
    // The bridge (nobody pressing) may take it after its lapse.
    assert!(c.launcher_claim_unattended(1, LaunchMode::Desktop).is_ok());
}

// --- REQUEST15 third pass, S4: a VRChat that never says its profile does not hold Play for its whole life ---

#[test]
fn a_vrchat_that_never_says_its_profile_can_be_passed_by_the_persons_second_press() {
    let (machine, c) = running_profile_0();
    machine.running.lock().unwrap().push((11, true));
    machine.unread.lock().unwrap().push(11);
    // At first: wait, whatever the presses.
    for _ in 0..2 {
        assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Error(t)) if !t.contains("press Play again")));
    }
    // A minute and more, still not said: told, then a second press starts.
    for since in c.launcher.lock().unwrap().unknown_since.values_mut() {
        *since -= Duration::from_secs(90);
    }
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Error(t)) if t.contains("press Play again")));
    assert!(machine.beside.lock().unwrap().is_empty());
    // A second press for another profile is not the confirmation of the first.
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 2 }), Some(Notice::Error(_))));
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 2 }), Some(Notice::Sent { .. })));
    assert_eq!(machine.beside.lock().unwrap().len(), 1);
}

// --- REQUEST15 fourth pass: the override waits for the newest unknown VRChat, and is asked again when the set changes ---

#[test]
fn a_newly_starting_unknown_vrchat_is_not_passed_because_an_older_one_never_spoke() {
    let (machine, c) = running_profile_0();
    machine.running.lock().unwrap().push((11, true));
    machine.unread.lock().unwrap().push(11);
    // The first unknown VRChat looked at a minute and a half ago.
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Error(_))));
    for since in c.launcher.lock().unwrap().unknown_since.values_mut() {
        *since -= Duration::from_secs(90);
    }
    // A second one starts now, profile not yet said: only the wait, never the override.
    machine.running.lock().unwrap().push((12, true));
    machine.unread.lock().unwrap().push(12);
    for _ in 0..2 {
        let notice = c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 });
        assert!(matches!(&notice, Some(Notice::Error(t)) if !t.contains("press Play again")), "{notice:?}");
    }
    assert!(machine.beside.lock().unwrap().is_empty());
    // Both had their minute: told; then a third VRChat appears before the second press: told again, nothing starts.
    for since in c.launcher.lock().unwrap().unknown_since.values_mut() {
        *since -= Duration::from_secs(90);
    }
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Error(t)) if t.contains("press Play again")));
    machine.running.lock().unwrap().push((13, true));
    machine.unread.lock().unwrap().push(13);
    c.launcher.lock().unwrap().unknown_since.insert(13, Instant::now() - Duration::from_secs(90));
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Error(t)) if t.contains("press Play again")), "another VRChat: asked again");
    assert!(machine.beside.lock().unwrap().is_empty());
    // The same three, pressed again: starts.
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Sent { .. })));
    assert_eq!(machine.beside.lock().unwrap().len(), 1);
}

/// REQUEST20 2: Home's Where starts the chosen profile right in the chosen instance (the link last), refuses what is not
/// VRChat's own instance id, a profile that already runs, and a destination of an account no longer on show.
#[cfg(not(target_os = "macos"))]
#[test]
fn where_starts_the_chosen_profile_in_the_chosen_instance() {
    let fresh = "48213~hidden(usr_test)~region(eu)~nonce(0a1b2c3d-0000-4000-8000-000000000000)";
    let place = launcher_feed::place(&format!("{W}:{fresh}"), "Sample Lounge").unwrap();
    // None runs: through Steam, the profile then the link.
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    let signed = |c: &Shared| {
        let (_, _) = c.test_vrchat(vec![]);
        signed_in_list(c);
    };
    signed(&c);
    assert_eq!(
        c.launcher_do(mine(Action::PlayAt { mode: LaunchMode::Desktop, profile: 2, destination: place.clone() })),
        Some(Notice::Sent { to: W.into(), action: SentKind::Launch })
    );
    let started = machine.started.lock().unwrap().clone();
    assert_eq!(started.len(), 1);
    // (R1: on another profile than the default one, a private place never goes with the start: it is delivered after it.)
    assert_eq!(started[0].last().unwrap(), "--profile=2");
    assert!(c.launcher.lock().unwrap().pending.as_ref().is_some_and(|p| p.proof && p.errand.destination.as_ref() == Some(&place)));
    assert_eq!(started[0][0], "--no-vr");
    // Profile 0 is VRChat's own start: the link alone after the options.
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    c.launcher_do(Action::PlayAt { mode: LaunchMode::Vr, profile: 0, destination: place.clone() });
    let started = machine.started.lock().unwrap().clone();
    assert!(!started[0].iter().any(|a| a.starts_with("--profile")));
    assert_eq!(started[0].last().unwrap(), &format!("vrchat://launch?ref=orbiters.cc&id={W}:{fresh}"));
    // Beside a VRChat on profile 0: VRChat's own launcher, into the instance; never on the running profile.
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    machine.running.lock().unwrap().push((10, false));
    let c = controller(machine.clone());
    signed(&c);
    let refused = c.launcher_do(mine(Action::PlayAt { mode: LaunchMode::Desktop, profile: 0, destination: place.clone() }));
    assert_eq!(refused, Some(Notice::Error("VRChat already runs on profile 0: choose another profile to start one more.".into())), "its own account's profile, which runs");
    assert!(machine.started.lock().unwrap().is_empty() && machine.beside.lock().unwrap().is_empty(), "nothing went to the running one");
    signed(&c);
    assert!(matches!(c.launcher_do(mine(Action::PlayAt { mode: LaunchMode::Desktop, profile: 1, destination: place.clone() })), Some(Notice::Sent { .. })));
    let beside = machine.beside.lock().unwrap().clone();
    assert_eq!(beside[0].last().unwrap(), "--profile=1", "its private place after it (R1)");
    assert!(c.launcher.lock().unwrap().pending.as_ref().is_some_and(|p| p.proof && p.before == [10]));
    // Only VRChat's own instance ids: nothing starts at all for another (not even a start at home).
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    for bad in ["1 --profile=0", "1\"&evil", "", "1~region(eu)&id=wrld_x"] {
        let mut odd = place.clone();
        odd.instance_id = bad.into();
        assert!(matches!(c.launcher_do(Action::PlayAt { mode: LaunchMode::Vr, profile: 1, destination: odd }), Some(Notice::Error(_))), "{bad:?}");
    }
    let mut odd = place.clone();
    odd.world_id = "wrld_../x".into();
    assert!(matches!(c.launcher_do(Action::PlayAt { mode: LaunchMode::Vr, profile: 1, destination: odd }), Some(Notice::Error(_))));
    assert!(machine.started.lock().unwrap().is_empty() && machine.beside.lock().unwrap().is_empty());
    // A destination pressed on another account's list: nothing starts.
    let (_, _) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    let other = Action::As { account: "usr_other".into(), action: Box::new(Action::PlayAt { mode: LaunchMode::Vr, profile: 1, destination: place.clone() }) };
    assert_eq!(c.launcher_do(other), Some(changed_account()));
    assert!(machine.started.lock().unwrap().is_empty());
    assert!(matches!(c.launcher_do(mine(Action::PlayAt { mode: LaunchMode::Vr, profile: 1, destination: place })), Some(Notice::Sent { .. })), "its own account's: it goes");
}

/// REQUEST20 2: an invite chosen in Home's Where starts the chosen profile in the invite's own instance, with the accept's
/// usual care (one accept, the card goes); nothing is sent to VRChat for it at the press.
#[cfg(not(target_os = "macos"))]
#[test]
fn where_accepts_an_invite_on_the_chosen_profile() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    let (_, calls) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    let place = launcher_feed::place(&format!("{W}:777~friends({A})~region(jp)"), "Café").unwrap();
    c.update(|s| {
        s.launcher.invites = vec![Invite { id: "not_1".into(), kind: InviteKind::Invite, from_id: A.into(), from_name: "Nova".into(), from_icon: String::new(), place: Some(place), message: String::new(), received_at: 0 }];
    });
    assert!(matches!(c.launcher_do(mine(Action::AcceptInviteOn { invite: "not_1".into(), mode: LaunchMode::Desktop, profile: 3 })), Some(Notice::Sent { .. })));
    let args = machine.started.lock().unwrap()[0].clone();
    assert_eq!(args.last().unwrap(), "--profile=3", "its own instance after it (R1)");
    assert!(c.launcher.lock().unwrap().pending.as_ref().is_some_and(|p| p.proof && p.errand.invite.as_deref() == Some("not_1")));
    assert!(c.view().launcher.invites.is_empty(), "the card goes");
    assert!(matches!(c.launcher_do(mine(Action::AcceptInviteOn { invite: "not_1".into(), mode: LaunchMode::Vr, profile: 3 })), Some(Notice::Error(_))), "one accept per invite");
    assert!(calls.lock().unwrap().is_empty(), "nothing asked of VRChat at the press: {:?}", calls.lock().unwrap());
    // Another account's invite: refused, nothing starts.
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    let c = controller(machine.clone());
    let (_, _) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    let other = Action::As { account: "usr_other".into(), action: Box::new(Action::AcceptInviteOn { invite: "not_1".into(), mode: LaunchMode::Vr, profile: 1 }) };
    assert!(matches!(c.launcher_do(other), Some(Notice::Error(_))));
    assert!(machine.started.lock().unwrap().is_empty());
}

/// A Mac that starts the game itself: the press of "Install & play VRChat" sets everything up, then plays, with no
/// other press and nothing opened.
#[test]
fn install_and_play_on_a_mac_sets_up_then_starts_the_game_itself() {
    let machine = Arc::new(PretendMac { direct: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::NoEngine);
    assert_eq!(c.launcher_do(Action::InstallEngine { profile: 0 }), None);
    assert_eq!(*machine.installs.lock().unwrap(), 1);
    assert_eq!(*machine.opened.lock().unwrap(), 0, "no other app is ever opened");
    let until = Instant::now() + Duration::from_secs(5);
    while c.view().launcher.game != Game::Starting(LaunchMode::Desktop) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(c.view().launcher.game, Game::Starting(LaunchMode::Desktop), "played without another press");
    assert_eq!(c.view().launcher.engine_setup, None);
    assert_eq!(machine.starts.lock().unwrap().len(), 1);
    machine.running.lock().unwrap().push((42, true));
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::Running(LaunchMode::Desktop));
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None }), Some(Notice::Error(_))));
    assert_eq!(machine.starts.lock().unwrap().len(), 1, "never a second VRChat");
}

#[test]
fn play_on_a_mac_starts_vrchat_itself_straight_into_the_destination() {
    let machine = Arc::new(PretendMac { direct: true, obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::NotRunning);
    assert_eq!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place()) }), Some(Notice::Sent { to: W.into(), action: SentKind::Launch }));
    let starts = machine.starts.lock().unwrap().clone();
    assert_eq!(starts.len(), 1);
    assert!(!starts[0].iter().any(|a| a == "--no-vr"), "a Mac has no SteamVR to keep out of: {:?}", starts[0]);
    assert!(!starts[0].iter().any(|a| a.starts_with("--")), "the PC's performance options are not a Mac's: {:?}", starts[0]);
    assert!(starts[0].windows(2).any(|w| w == ["-screen-fullscreen", "1"]), "the whole screen, not a small window: {:?}", starts[0]);
    assert!(starts[0].last().is_some_and(|a| a.starts_with("vrchat://launch") && a.contains(W)), "straight into the instance: {:?}", starts[0]);
    assert!(c.launcher.lock().unwrap().pending.is_none(), "no invite to oneself is needed");
    assert_eq!(*machine.opened.lock().unwrap(), 0);
    // A second press while it starts starts nothing more, however long the engine takes to download first.
    *c.launcher_started.lock().unwrap() = Instant::now().checked_sub(MAC_STARTING_FOR + Duration::from_secs(1));
    machine.still_starting.store(true, Ordering::Relaxed);
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::Starting(LaunchMode::Desktop), "still starting while the machine says so");
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None }), Some(Notice::Sent { .. })));
    assert_eq!(machine.starts.lock().unwrap().len(), 1);
}

#[test]
fn a_start_that_fails_on_a_mac_is_said_and_play_is_offered_again() {
    let machine = Arc::new(PretendMac { direct: true, obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None }), Some(Notice::Sent { .. })));
    machine.still_starting.store(true, Ordering::Relaxed);
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::Starting(LaunchMode::Desktop));
    // The engine gave up (Steam refused, Easy Anti-Cheat did not start…).
    machine.still_starting.store(false, Ordering::Relaxed);
    *machine.failure.lock().unwrap() = Some("Steam could not be reached.".into());
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::NotRunning, "Play again, not three minutes of Starting");
    assert!(matches!(c.view().launcher.notice, Some(Notice::Error(ref words)) if words.contains("Steam could not be reached.")));
    assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: None }), Some(Notice::Sent { .. })));
    assert_eq!(machine.starts.lock().unwrap().len(), 2);
}

#[test]
fn play_before_the_set_up_on_a_mac_sets_up_first_and_an_install_that_fails_can_be_tried_again() {
    let machine = Arc::new(PretendMac { direct: true, cannot_install: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    assert!(matches!(c.launcher_do(Action::InstallEngine { profile: 0 }), Some(Notice::Error(_))));
    assert_eq!(c.view().launcher.engine_setup, Some(EngineSetup::Failed("no network".into())));
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::NoEngine, "the button is there again");
    assert!(matches!(c.launcher_do(Action::InstallEngine { profile: 0 }), Some(Notice::Error(_))));
    assert_eq!(*machine.installs.lock().unwrap(), 2);
    // While one is under way, a second press starts no second set-up.
    c.update(|s| s.launcher.engine_setup = Some(EngineSetup::SignIn { qr_url: "https://s.team/q/1/2".into() }));
    assert_eq!(c.launcher_do(Action::InstallEngine { profile: 0 }), None);
    assert_eq!(*machine.installs.lock().unwrap(), 2);
    assert!(machine.starts.lock().unwrap().is_empty());
    // On a PC the button does not exist, and its action does nothing.
    let pc = Controller::new(Settings::default());
    pc.launcher_machine(Arc::new(Pretend::default()));
    assert_eq!(pc.launcher_do(Action::InstallEngine { profile: 0 }), None);
    assert_eq!(pc.view().launcher.engine_setup, None);
}

/// r20: while the VRChat session waits for Orbiters (the environment just switched, Orbiters out of reach), the list on
/// show stays: not cleared, not replaced by VRCX's or VRCNext's copy, nothing read again; the same account goes on after.
#[test]
fn a_session_waiting_for_orbiters_keeps_the_list_on_show() {
    let c = controller(Arc::new(Pretend::default()));
    let (_client, calls) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    {
        let mut rt = c.launcher.lock().unwrap();
        rt.loaded_for = Some(Some("usr_test".into()));
        rt.loaded_at = Some(Instant::now());
    }
    c.vrchat_login.lock().unwrap().suspend();
    c.update(|s| s.vrchat_login.waiting = true);
    for _ in 0..3 {
        c.launcher_tick();
    }
    assert!(matches!(&c.view().launcher.source, Source::SignedIn { user_id, .. } if user_id == "usr_test"), "{:?}", c.view().launcher.source);
    assert_eq!(c.launcher.lock().unwrap().loaded_for, Some(Some("usr_test".into())));
    assert!(calls.lock().unwrap().is_empty());
}

/// r21: a launch with a saved VRChat session, Orbiters answering after `orbiters` and VRChat after `vrchat` (at VRChat's
/// own pace): how long until the window can say "signed in to VRChat" (the sign-in's status), and until the friends
/// list read with the session is on show. `None`: not within 20 s.
fn launch_times(orbiters: Duration, vrchat: Duration) -> (Option<Duration>, Option<Duration>) {
    let c = controller(Arc::new(Pretend::default()));
    let me = json!({"id": "usr_test", "displayName": "Tester", "currentAvatarThumbnailImageUrl": "https://api.vrchat.cloud/api/1/file/file_me/1/file"});
    let friend = |i: usize, online: bool| json!({"id": format!("usr_00000000-0000-4000-8000-{i:012}"), "displayName": format!("Friend {i}"), "status": if online { "active" } else { "offline" }, "location": if online { "private" } else { "offline" }, "tags": []});
    let (client, _) = orbiters_core::vrchat_api::test_server(Arc::new(orbiters_core::vrchat_api::Gate::default()), vrchat, move |_, url| {
        if url.contains("/auth/user/friends") {
            let page: Vec<_> = match (url.contains("offline=true"), url.contains("offset=0")) {
                (false, true) => (0..30).map(|i| friend(i, true)).collect(),
                (true, true) => (30..130).map(|i| friend(i, false)).collect(),
                (true, false) if url.contains("offset=100") => (130..224).map(|i| friend(i, false)).collect(),
                _ => Vec::new(),
            };
            (200, json!(page))
        } else if url.ends_with("/auth/user") {
            (200, me.clone())
        } else if url.contains("/favorite") || url.contains("/notifications") {
            (200, json!([]))
        } else {
            (404, json!({}))
        }
    });
    c.test_saved_vrchat(client, move |_, _| {
        std::thread::sleep(orbiters);
        Ok(true)
    });
    // The account file is read at once (Checking); Orbiters answers its check after `orbiters`.
    {
        let mut rt = c.account_runtime.lock().unwrap();
        rt.read = Some((orbiters_core::account::Server::current(), None));
        rt.account = Some(orbiters_core::account::Account { token: "fake".into(), user: Some("1".into()), username: Some("tester".into()), avatar_url: None, features: None });
    }
    let development = orbiters_core::account::Server::current() == orbiters_core::account::Server::Development;
    c.update(|s| {
        s.account.state = crate::state::AccountState::Checking { username: Some("tester".into()) };
        s.account.development = development;
    });
    let start = Instant::now();
    {
        let c = c.clone();
        std::thread::spawn(move || {
            std::thread::sleep(orbiters);
            c.update(|s| s.account.state = crate::state::AccountState::SignedIn { username: "tester".into() });
        });
    }
    c.start_vrchat_login();
    c.start_launcher();
    let (mut status, mut list) = (None, None);
    while start.elapsed() < Duration::from_secs(20) && (status.is_none() || list.is_none()) {
        let snap = c.view();
        if status.is_none() && snap.vrchat_login.user.is_some() {
            status = Some(start.elapsed());
        }
        if list.is_none() && matches!(snap.launcher.source, Source::SignedIn { .. }) {
            list = Some(start.elapsed());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    (status, list)
}

/// r21 measurement, by hand (`--ignored --nocapture`): Orbiters at 150 ms an answer, VRChat at 250 ms.
#[test]
#[ignore]
fn launch_with_a_saved_session_times() {
    for _ in 0..3 {
        let (status, list) = launch_times(Duration::from_millis(150), Duration::from_millis(250));
        println!("r21 launch: sign-in status says signed in after {status:?}; the friends list read with it after {list:?}");
    }
}

/// R1 of the 2026-11-23 review: Home offers a restart only for a confirmed loss of its own still-running VRChat, and the
/// click checks again (a game that draws again, or another one, is left alone).
#[test]
fn a_mac_offers_a_restart_only_for_a_confirmed_lost_picture_checked_on_the_click() {
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, direct: true, ..Default::default() });
    *machine.running.lock().unwrap() = vec![(4242, true)];
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::Running(LaunchMode::Desktop));
    let restart = |session: &str| c.launcher_do(Action::RestartLostGame { session: session.into() });
    assert_eq!(c.view().launcher.picture_lost, None, "a healthy (or covered, or locked) game: nothing offered");
    assert!(matches!(restart("s1"), Some(Notice::Error(_))));
    // confirmed lost: offered
    *machine.lost.lock().unwrap() = Some("s1".into());
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.picture_lost.as_deref(), Some("s1"));
    // it draws again before the click: nothing done
    *machine.lost.lock().unwrap() = None;
    assert!(matches!(restart("s1"), Some(Notice::Error(_))));
    assert!(machine.restarts.lock().unwrap().is_empty());
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.picture_lost, None, "the offer goes with the loss");
    // lost, and the click: that very session is restarted
    *machine.lost.lock().unwrap() = Some("s2".into());
    // (fifth verification, finding 8: the button shown for s1 and pressed now restarts neither)
    assert!(matches!(restart("s1"), Some(Notice::Error(_))));
    assert!(machine.restarts.lock().unwrap().is_empty());
    assert_eq!(restart("s2"), None);
    for _ in 0..200 {
        if !machine.restarts.lock().unwrap().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(*machine.restarts.lock().unwrap(), ["s2"]);
}

/// Fifth verification, finding 5: the engine's files no game starts on are said on Home with what ends it; Repair installs
/// them again, once at a time, says how far it is, and what it came to; a list read meanwhile keeps it on show.
#[test]
fn a_mac_s_engine_files_are_said_on_home_and_repaired_from_it() {
    use orbiters_core::obelisk::{EngineAction, EngineProblem};
    let problem = |action| EngineProblem { paths: vec!["engine/wsd".into()], reason: "why".into(), action };
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, direct: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.engine_trouble, None);
    assert_eq!(c.launcher_do(Action::RepairEngine), None);
    assert_eq!(*machine.repairs.lock().unwrap(), 0, "nothing to repair");
    *machine.engine.lock().unwrap() = Some(problem(EngineAction::Repair));
    c.launcher_game(machine.as_ref());
    let shown = c.view().launcher.engine_trouble.clone().unwrap();
    assert_eq!((shown.paths, shown.action, shown.repairing), (vec!["engine/wsd".to_string()], EngineAction::Repair, false));
    // Other ends are said as they are.
    *machine.engine.lock().unwrap() = Some(problem(EngineAction::QuitGame));
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.engine_trouble.as_ref().map(|t| t.action), Some(EngineAction::QuitGame));
    *machine.engine.lock().unwrap() = Some(problem(EngineAction::Repair));
    machine.hold_repair.store(true, Ordering::Relaxed);
    assert_eq!(c.launcher_do(Action::RepairEngine), None);
    let until = |done: &dyn Fn() -> bool| {
        for _ in 0..400 {
            if done() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("it never came");
    };
    until(&|| c.view().launcher.engine_trouble.as_ref().is_some_and(|t| t.repairing && t.percent == Some(40)));
    assert_eq!(c.launcher_do(Action::RepairEngine), None);
    c.launcher_game(machine.as_ref());
    c.publish_local_list(|l| *l = LauncherState::default());
    assert!(c.view().launcher.engine_trouble.as_ref().is_some_and(|t| t.repairing && t.percent == Some(40)), "a list read keeps it");
    machine.hold_repair.store(false, Ordering::Relaxed);
    until(&|| c.view().launcher.engine_trouble.is_none());
    assert_eq!(*machine.repairs.lock().unwrap(), 1, "once, however often it is pressed");
    // One that fails is said, and the offer stays.
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, direct: true, cannot_install: true, ..Default::default() });
    *machine.engine.lock().unwrap() = Some(problem(EngineAction::Repair));
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    c.launcher_do(Action::RepairEngine);
    until(&|| c.view().launcher.notice == Some(Notice::Error("no network".into())));
    assert!(c.view().launcher.engine_trouble.as_ref().is_some_and(|t| !t.repairing));
}

/// A program in `root` that answers as obelisk-cli 0.1.14 does: `steam play` runs a game until `stop` is written, its session
/// the next of `sessions`; `status` says the Steam account `account` holds; everything it was asked is in `asked`.
#[cfg(target_os = "macos")]
fn pretend_program(root: &std::path::Path) -> MacMachine {
    use std::os::unix::fs::PermissionsExt;
    let folder = root.join("tools/obelisk-cli-0.1.14-arm64");
    std::fs::create_dir_all(folder.join("SteamNative")).unwrap();
    std::fs::create_dir_all(root.join("library/SteamSessions")).unwrap();
    std::fs::write(folder.join("SteamNative/obelisk-steam-host"), "").unwrap();
    let script = r#"#!/bin/bash
umask 077
root=ROOT
echo "$*" >> "$root/asked"
case "$1 $2" in
"status ") echo "{\"event\":\"status\",\"account\":{\"steam_id\":\"$(cat "$root/account")\",\"account_name\":\"n\"}}" ;;
"engine repair")
    echo '{"event":"engine_repairing","bytes_done":1,"bytes_total":2,"fraction":0.5}'
    if [ -e "$root/repairs" ]; then echo '{"event":"engine_repaired","voile":"v1","keyboard_layout":"mac","engine_files":[]}'; exit 0; fi
    echo '{"event":"error","code":"voile_failed","message":"m","key":null}'; exit 1 ;;
"steam quit") touch "$root/stop-$5"; while [ -e "$root/stop-$5" ]; do sleep 0.05; done; echo '{"event":"quit_requested","app_id":438100,"forced":false}' ;;
"steam play")
    if [ -e "$root/refuses" ]; then
        echo '{"event":"error","code":"engine_files","message":"m","key":"backend.runtime.engineFiles","engine":{"paths":["engine/wsd"],"reason":"why","action":"repair"}}'; exit 1
    fi
    if [ -e "$root/switch" ]; then echo 78 > "$root/account"; fi
    if [ "$4" = --steam-id ] && [ "$5" != "$(cat "$root/account")" ]; then
        echo '{"event":"error","code":"other_account","message":"m","key":null}'; exit 1
    fi
    n=$(($(cat "$root/count" 2>/dev/null || echo 0) + 1)); echo $n > "$root/count"; session="5e551011-0000-4000-8000-00000000000$n"
    record() { echo "{\"version\":2,\"id\":\"$session\",\"account_id\":77,\"phase\":\"$1\",\"store\":{\"app_id\":438100},\"played\":{\"seconds\":400,\"frames\":{\"count\":1000,\"measured\":true}},\"revision\":1}" > "$root/writing" && mv "$root/writing" "$root/library/SteamSessions/$session.v2.json"; }
    record Running
    echo "{\"event\":\"running\",\"app_id\":438100,\"pid\":$$,\"session_id\":\"$session\"}"
    while [ ! -e "$root/stop-$session" ]; do sleep 0.05; done
    record Finished
    echo "{\"event\":\"exited\",\"app_id\":438100,\"session_id\":\"$session\",\"exit_code\":0,\"seconds\":400,\"frames\":1000}"
    rm "$root/stop-$session" ;;
esac
"#;
    let program = folder.join("obelisk-cli");
    std::fs::write(&program, script.replace("ROOT", &format!("'{}'", root.display()))).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.join("account"), "77").unwrap();
    MacMachine {
        tools: root.join("tools"),
        library: root.join("library"),
        play: Arc::new(super::super::obelisk_cli::Play::default()),
        signed_out: std::sync::atomic::AtomicBool::new(false),
        home_launch: Mutex::default(),
        render: std::sync::atomic::AtomicU32::new(100),
        restarting: Mutex::default(),
        repairing: std::sync::atomic::AtomicBool::new(false),
    }
}

#[cfg(target_os = "macos")]
fn eventually(done: impl Fn() -> bool) {
    for _ in 0..1000 {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("it never came");
}

/// The panels around VRChat's menus lie over Home's own VRChat, for as long as it runs, and over one started by hand or before this N4N0; never over the bridge's, which
/// its agent clicks in.
#[test]
#[cfg(target_os = "macos")]
fn the_panels_game_is_the_persons_own_and_never_the_bridges() {
    let root = std::env::temp_dir().join(format!("orbiters-panels-game-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let machine = pretend_program(&root);
    let game = || machine.desktop_game(&machine.running().iter().map(|(pid, _)| *pid).collect::<Vec<_>>());
    let stop = |n: u32| {
        std::fs::write(root.join(format!("stop-5e551011-0000-4000-8000-00000000000{n}")), "").unwrap();
        eventually(|| machine.running().is_empty() && !machine.play.starting() && machine.play.attached().is_none());
    };
    assert_eq!(game(), None, "nothing runs");
    // One this N4N0 did not start (by hand, or before it was restarted).
    assert_eq!(machine.desktop_game(&[4242]), Some(4242));
    machine.start_for(&["-screen-fullscreen".to_string()], &errand_of("usr_a")).unwrap();
    eventually(|| game().is_some());
    assert_eq!(game(), machine.running().first().map(|(pid, _)| *pid));
    stop(1);
    assert_eq!(game(), None, "it ended");
    machine.start_launch_reserved(&["-screen-width".to_string()], None).unwrap();
    eventually(|| !machine.running().is_empty());
    assert_eq!(game(), None, "the bridge's");
    stop(2);
    let _ = std::fs::remove_dir_all(&root);
}

/// Fifth verification, finding 8: a restart is of the session it was shown for, with what that very launch was started with
/// and for its own accounts, asked again after the quit; nothing is asked to quit for a launch that could not be started
/// again, and nothing is started over another start.
#[test]
#[cfg(target_os = "macos")]
fn a_restart_is_of_its_own_session_its_own_arguments_and_its_own_accounts() {
    use super::super::picture;
    let _publishing = picture::PUBLISHING.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::env::temp_dir().join(format!("orbiters-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let machine = pretend_program(&root);
    let session = |n: u32| format!("5e551011-0000-4000-8000-00000000000{n}");
    let asked = || std::fs::read_to_string(root.join("asked")).unwrap_or_default();
    let plays = || asked().lines().filter(|l| l.starts_with("steam play")).map(str::to_owned).collect::<Vec<_>>();
    let quits = || asked().lines().filter(|l| l.starts_with("steam quit")).count();
    let args = |link: &str| vec!["-screen-fullscreen".to_string(), link.to_string()];
    let runs = |n: u32| machine.home_session_running(&session(n));

    // One start, ended by itself; then another, of another destination and account.
    machine.start_for(&args("vrchat://a"), &errand_of("usr_a")).unwrap();
    eventually(|| runs(1));
    std::fs::write(root.join(format!("stop-{}", session(1))), "").unwrap();
    eventually(|| !runs(1) && !machine.play.starting() && machine.play.attached().is_none());
    machine.start_for(&args("vrchat://b"), &errand_of("usr_b")).unwrap();
    eventually(|| runs(2));

    // Not lost; lost but of another session; of the first one, long gone: nothing is asked.
    let live = of_account(|account| account == "usr_b");
    picture::publish(None);
    assert!(machine.restart_lost(&session(2), &live).is_err());
    picture::publish(Some((session(2), picture::Picture::Lost)));
    assert_eq!(machine.picture_lost(), Some(session(2)));
    assert!(machine.restart_lost(&session(1), &live).is_err());
    // Its account is no longer the one signed in, at VRChat or at Steam: it is left running.
    assert!(machine.restart_lost(&session(2), &of_account(|account| account == "usr_a")).is_err());
    std::fs::write(root.join("account"), "78").unwrap();
    assert!(machine.restart_lost(&session(2), &live).is_err());
    std::fs::write(root.join("account"), "77").unwrap();
    assert_eq!((quits(), plays().len()), (0, 2));
    assert!(runs(2));

    // Restarted: that session asked to quit, then started as it was.
    machine.restart_lost(&session(2), &live).unwrap();
    assert!(asked().contains(&format!("steam quit 438100 --session {} --wait 60", session(2))));
    eventually(|| runs(3));
    assert_eq!(quits(), 1);
    assert!(plays()[2].ends_with("-- -screen-fullscreen vrchat://b"), "{:?}", plays());

    // The account goes while the game quits: nothing is started.
    picture::publish(Some((session(3), picture::Picture::Lost)));
    let times = std::sync::atomic::AtomicU32::new(0);
    let first_only = of_account(|_| times.fetch_add(1, Ordering::Relaxed) == 0);
    assert!(machine.restart_lost(&session(3), &first_only).is_err());
    assert_eq!((quits(), plays().len()), (2, 3));
    picture::publish(None);
    let _ = std::fs::remove_dir_all(&root);
}

/// Sixth verification, the restart finding (the verifier's fixture): the saved Steam account changes between the restart's
/// last look at it and the session the program starts with: the program is told whom it is for, and starts nobody else's.
#[test]
#[cfg(target_os = "macos")]
fn a_restart_never_starts_on_an_account_that_changed_at_the_last_moment() {
    use super::super::picture;
    let _publishing = picture::PUBLISHING.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::env::temp_dir().join(format!("orbiters-restart-account-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let machine = pretend_program(&root);
    let session = |n: u32| format!("5e551011-0000-4000-8000-00000000000{n}");
    let asked = || std::fs::read_to_string(root.join("asked")).unwrap_or_default();
    machine.start_for(&[], &errand_of("test-person")).unwrap();
    eventually(|| machine.home_session_running(&session(1)));
    picture::publish(Some((session(1), picture::Picture::Lost)));
    std::fs::write(root.join("switch"), "").unwrap();
    let _ = machine.restart_lost(&session(1), &of_account(|_| true));
    eventually(|| asked().contains("steam play 438100 --steam-id 77") && machine.play.attached().is_none());
    assert!(!machine.home_session_running(&session(2)));
    assert!(orbiters_core::obelisk::session_record_of(&root.join("library"), &session(2)).unwrap().is_none(), "nothing started for another account");
    let said = machine.play.launch(2).and_then(|l| l.failure).map(|f| f.code);
    assert_eq!(said.as_deref(), Some("other_account"), "and Home says why");
    // A start of the person's own (Play) names none: it is for whoever is signed in.
    assert!(asked().lines().find(|l| l.starts_with("steam play")).is_some_and(|l| !l.contains("--steam-id")));
    picture::publish(None);
    let _ = std::fs::remove_dir_all(&root);
}

/// Sixth verification (the verifier's fixture): two coordinators of one computer never both reserve a start, nor start beside
/// the other's; the slot is free again once it is given back or its start is over.
#[test]
#[cfg(target_os = "macos")]
fn two_coordinators_never_both_reserve_a_start() {
    use super::super::obelisk_cli::{Play, Refused};
    let root = std::env::temp_dir().join(format!("orbiters-coordinators-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let machine = pretend_program(&{
        std::fs::create_dir_all(&root).unwrap();
        root.clone()
    });
    let (library, program) = (root.join("library"), machine.program().unwrap());
    let new = || Arc::new(Play::new(root.join("config/gate.json")));
    let (a, b) = (new(), new());
    let token = a.reserve(&library, None).unwrap();
    assert!(matches!(b.reserve(&library, None), Err(Refused::Busy)), "reserved by the other");
    assert!(matches!(b.start_reserved(&program, &library, &[], None), Err(Refused::Busy)), "nor started beside it");
    a.unreserve(token);
    let token = b.reserve(&library, None).unwrap();
    assert!(a.reserve(&library, None).is_err());
    // Its start keeps it until it is over.
    let id = b.start_reserved(&program, &library, &[], Some(token)).unwrap();
    eventually(|| b.launch(id).is_some_and(|l| l.running));
    assert!(matches!(a.reserve(&library, None), Err(Refused::Busy)));
    std::fs::write(root.join("stop-5e551011-0000-4000-8000-000000000001"), "").unwrap();
    eventually(|| b.attached().is_none());
    let token = a.reserve(&library, None).unwrap();
    a.unreserve(token);
    // A start that could not be made lets go of it.
    assert!(matches!(a.start_reserved(&root.join("no program"), &library, &[], None), Err(Refused::Failed(_))));
    let token = b.reserve(&library, None).unwrap();
    b.unreserve(token);
    // One that is gone (its process ended) holds nothing.
    let _ = a.reserve(&library, None).unwrap();
    drop(a);
    eventually(|| b.reserve(&library, None).is_ok());
    let _ = std::fs::remove_dir_all(&root);
}

/// Finding 5, end to end with the program: a start refused over the engine's files is kept with its paths, whoever it was
/// for; a repair that fails leaves it, one that succeeds ends it.
#[test]
#[cfg(target_os = "macos")]
fn a_start_refused_over_the_engine_s_files_is_kept_until_they_are_repaired() {
    use orbiters_core::obelisk::EngineAction;
    let root = std::env::temp_dir().join(format!("orbiters-repair-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let machine = pretend_program(&root);
    assert_eq!(machine.engine_problem(), None);
    std::fs::write(root.join("refuses"), "").unwrap();
    // (the bridge's start here: Home says it all the same)
    machine.start_launch_reserved(&[], None).unwrap();
    eventually(|| machine.engine_problem().is_some());
    let problem = machine.engine_problem().unwrap();
    assert_eq!((problem.paths.as_slice(), problem.action), (["engine/wsd".to_string()].as_slice(), EngineAction::Repair));
    let stop = std::sync::atomic::AtomicBool::new(false);
    let heard = Mutex::new(Vec::new());
    let repair = || machine.repair_engine(&|percent| heard.lock().unwrap().push(percent), &stop);
    assert!(repair().is_err());
    assert!(machine.engine_problem().is_some(), "still there");
    std::fs::write(root.join("repairs"), "").unwrap();
    repair().unwrap();
    assert_eq!(machine.engine_problem(), None);
    assert_eq!(*heard.lock().unwrap(), [Some(50), Some(50)]);
    let _ = std::fs::remove_dir_all(&root);
}

/// A program older than the one every start needs starts nothing (its new version could not be got).
#[test]
#[cfg(target_os = "macos")]
fn a_program_too_old_starts_no_vrchat() {
    let root = std::env::temp_dir().join(format!("orbiters-too-old-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let machine = pretend_program(&root);
    std::fs::rename(root.join("tools/obelisk-cli-0.1.14-arm64"), root.join("tools/obelisk-cli-0.1.13-arm64")).unwrap();
    assert!(machine.start(&[]).is_err());
    assert!(!root.join("asked").exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// verify3-r21 M1: a destination verified for an account starts for that very account, its friends list read or not; for
/// another login it starts nothing. A private one goes by R1 (`vrchat_launch::way`): with the start where VRChat has one
/// profile; where it has several VRChat starts without it, and only that very VRChat, once its own log proves that account, is
/// given it; one that says another account never is.
#[cfg(not(target_os = "macos"))]
#[test]
fn verify3_a_bound_destination_needs_its_account_not_its_list() {
    let there = || private_place("hidden(usr_00000000-0000-4000-8000-000000000004)");
    let link = orbiters_core::vrchat_launch::link(W, &there().instance_id);
    let unread = |several: bool| {
        let on = Computer::new(false);
        on.c.update(|s| s.launcher.source = Source::Nothing);
        if several {
            on.remembers(&[0, 1]);
        }
        // Another login than the one it was proven for, whatever the profiles.
        let other = on.c.launcher_do(Action::As { account: "usr_other".into(), action: Box::new(Action::PlayAt { mode: LaunchMode::Vr, profile: 0, destination: there() }) });
        assert_eq!(other, Some(changed_account()), "several {several}");
        assert!(on.starts().is_empty() && on.pending().is_none());
        on
    };
    let sent = Some(Notice::Sent { to: W.into(), action: SentKind::Launch });
    let on = unread(false);
    assert_eq!(on.at(0, there()), sent, "one profile");
    assert_eq!(on.starts()[0].last(), Some(&link));
    for (says, goes) in [("usr_test", true), ("usr_other", false)] {
        let on = unread(true);
        assert_eq!(on.at(0, there()), sent, "several");
        assert!(!on.starts()[0].iter().any(|arg| arg.contains("vrchat://")), "{:?}", on.starts());
        assert!(on.pending().is_some_and(|p| p.proof && p.errand.account.as_deref() == Some("usr_test")), "bound to its account");
        // It is up, and has not said whose it is.
        on.runs(&[(42, 0, None, true)]);
        on.look();
        assert_eq!(on.step(), Some(JoinStep::SigningIn));
        // It says.
        on.runs(&[(42, 0, Some(says), true)]);
        on.look();
        assert!(on.pending().is_none());
        let until = Instant::now() + Duration::from_secs(5);
        while goes && on.joining().is_some() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(on.step(), (!goes).then_some(JoinStep::Failed(JoinFailure::OtherAccount)), "{says}");
        assert!(!matches!(on.c.view().launcher.source, Source::SignedIn { .. }), "its list is not read yet");
    }
}

/// verify4-r21 (B6, for dev.46's place on its way): it is its account's: gone from the first state that is another member's,
/// nobody's or an unknown one's, and from that of another VRChat account; what ends late for the one before shows nothing.
/// Its own member's stays, their Orbiters server out of reach too.
#[test]
fn verify4_a_place_on_its_way_is_its_own_members_alone() {
    use crate::state::AccountState;
    let signed = |name: &str| AccountState::SignedIn { username: name.into() };
    let waiting = || {
        let on = Computer::new(false);
        on.remembers(&[1]);
        assert!(matches!(on.at(0, private_place("friends(usr_x)")), Some(Notice::Sent { .. })));
        assert_eq!(on.step(), Some(JoinStep::Starting));
        on
    };
    let late = |on: &Computer, errand: Errand| {
        let by = errand.by().unwrap();
        assert!(by.is_some() && !on.c.publish_joining(by.as_ref(), Joining::default()), "refused where it is written");
        on.c.launcher_join_failed(errand.clone(), JoinFailure::Gone);
        let failed = on.joining();
        on.c.launcher_join_after(on.machine().as_ref(), errand, Vec::new());
        failed.or(on.joining())
    };
    type Change = fn(&Controller);
    let changes: [(&str, Change); 6] = [
        ("another member", |c| c.sign_in_for_test("2")),
        ("another member, granted the same VRChat account", |c| {
            c.sign_in_for_test("2");
            c.update(|_| {});
            c.test_signed_in();
            c.update(|s| (s.vrchat_login.allowed, s.vrchat_login.user) = (true, Some(orbiters_core::vrchat_api::User { id: "usr_test".into(), ..Default::default() })));
            assert!(c.vrchat_now().is_some());
        }),
        ("an account with no id", |c| c.install_account(|rt| if let Some(a) = rt.account.as_mut() { a.user = None })),
        ("an account that cannot be read", |c| c.install_account(|rt| rt.unknown = true)),
        ("nobody", |c| c.update(|s| s.account.state = AccountState::SignedOut)),
        ("another VRChat account", |c| c.update(|s| s.vrchat_login.user = Some(orbiters_core::vrchat_api::User { id: "usr_second".into(), ..Default::default() }))),
    ];
    let mut wrong = Vec::new();
    for (what, change) in changes {
        let on = waiting();
        let errand = on.pending().unwrap().errand;
        change(&on.c);
        on.c.update(|_| {});
        if on.joining().is_some() {
            wrong.push(format!("{what}: still on show, {:?}", on.joining()));
        }
        if let Some(late) = late(&on, errand) {
            wrong.push(format!("{what}: shown late, {late:?}"));
        }
    }
    let on = waiting();
    on.c.update(|s| s.account.state = AccountState::Offline { username: Some("tester".into()) });
    if on.step() != Some(JoinStep::Starting) {
        wrong.push(format!("its own member, Orbiters out of reach: {:?}", on.joining()));
    }
    on.c.update(|s| s.account.state = signed("tester"));
    on.c.launcher_join_failed(on.pending().unwrap().errand, JoinFailure::Gone);
    if on.step() != Some(JoinStep::Failed(JoinFailure::Gone)) {
        wrong.push(format!("its own member, not delivered: {:?}", on.joining()));
    }
    // A public place asked for on no account's list is this computer's: said to whoever is there.
    let on = waiting();
    let errand = Errand::plain(LaunchMode::Desktop, Some(0)).to(Some(open_place()), None);
    on.c.sign_in_for_test("2");
    on.c.launcher_join_failed(errand, JoinFailure::Gone);
    if on.step() != Some(JoinStep::Failed(JoinFailure::Gone)) {
        wrong.push(format!("a public one: {:?}", on.joining()));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("
"));
}

// --- One VRChat at a time on a Mac, on the profile chosen on Home ---

fn ready_mac() -> (Arc<PretendMac>, Shared) {
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, direct: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    (machine, c)
}

/// VRChat runs on `profile` as process 42, the account its own log names when it does.
fn plays(machine: &PretendMac, profile: Option<u32>, account: Option<&str>) {
    *machine.running.lock().unwrap() = vec![(42, true)];
    *machine.players.lock().unwrap() =
        vec![crate::state::VrchatRunShown { pid: 42, profile: profile.unwrap_or(0), profile_known: profile.is_some(), account: account.map(str::to_owned), desktop: Some(true), ..Default::default() }];
}

fn quit(machine: &PretendMac) {
    machine.running.lock().unwrap().clear();
    machine.players.lock().unwrap().clear();
}

/// What the profile choice shows: number, account, running, starting.
fn choice(c: &Shared) -> Vec<(u32, Option<String>, bool, bool)> {
    c.view().launcher.profiles.iter().map(|p| (p.profile, p.account.clone(), p.running.is_some(), p.starting.is_some())).collect()
}

fn profile_args(args: &[String]) -> Vec<&str> {
    args.iter().map(String::as_str).filter(|a| a.contains("profile")).collect()
}

/// Every way Home starts VRChat on a Mac gives it exactly `--profile=N`, nothing for profile 0, and none of the PC's options.
#[test]
fn every_start_on_a_mac_is_on_the_profile_chosen() {
    let started = |go: &dyn Fn(&Arc<PretendMac>, &Shared)| {
        let (machine, c) = ready_mac();
        go(&machine, &c);
        let starts = machine.starts.lock().unwrap().clone();
        assert_eq!(starts.len(), 1, "one start");
        assert!(!starts[0].iter().any(|a| a.starts_with("--") && !a.starts_with("--profile=")), "{:?}", starts[0]);
        assert_eq!(starts[0][..4], ["-screen-fullscreen", "1", "-window-mode", "borderless"]);
        starts[0].clone()
    };
    for profile in [0, 1, 2, 15] {
        let own: Vec<String> = (profile != 0).then(|| format!("--profile={profile}")).into_iter().collect();
        // Play.
        let args = started(&|_, c| assert_eq!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch })));
        assert_eq!(args[4..], own);
        // A destination: the profile, then its link, last; on another profile than the default one (R1), a private one's after it.
        let link = |there: Place| (profile == 0).then(|| orbiters_core::vrchat_launch::link(W, &there.instance_id)).into_iter().collect::<Vec<_>>();
        let args = started(&|_, c| {
            let (_, _) = c.test_vrchat(vec![]);
            signed_in_list(c);
            assert_eq!(c.launcher_do(mine(Action::PlayAt { mode: LaunchMode::Desktop, profile, destination: place() })), Some(Notice::Sent { to: W.into(), action: SentKind::Launch }));
            assert_eq!(choice(c).iter().filter(|p| p.3).map(|p| p.0).collect::<Vec<_>>(), [profile], "shown at once");
            assert_eq!(c.launcher.lock().unwrap().pending.is_some(), profile != 0);
        });
        assert_eq!(args[4..], [own.clone(), link(place())].concat());
        // A public one: with the start, whichever the profile.
        let args = started(&|_, c| assert!(matches!(c.launcher_do(Action::PlayAt { mode: LaunchMode::Desktop, profile, destination: open_place() }), Some(Notice::Sent { .. }))));
        assert_eq!(args[4..], [own.clone(), vec![orbiters_core::vrchat_launch::link(W, &open_place().instance_id)]].concat());
        // An invite, accepted on it.
        let args = started(&|_, c| {
            let (_, _) = c.test_vrchat(vec![(200, json!({}))]);
            signed_in_list(c);
            c.update(|s| s.launcher.invites = vec![Invite { id: "not_1".into(), kind: InviteKind::Invite, from_id: A.into(), from_name: "Sample".into(), from_icon: String::new(), place: Some(place()), message: String::new(), received_at: 1 }]);
            assert!(matches!(c.launcher_do(mine(Action::AcceptInviteOn { invite: "not_1".into(), mode: LaunchMode::Desktop, profile })), Some(Notice::Sent { .. })));
        });
        assert_eq!(args[4..], [own.clone(), link(place())].concat());
        // Install & play, and Play before the set-up: the start that follows it.
        for install in [true, false] {
            let machine = Arc::new(PretendMac { direct: true, ..Default::default() });
            let c = mac(machine.clone());
            c.launcher_game(machine.as_ref());
            if install {
                assert_eq!(c.launcher_do(Action::InstallEngine { profile }), None);
            } else {
                assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile }), Some(Notice::Sent { .. })));
            }
            let until = Instant::now() + Duration::from_secs(5);
            while machine.starts.lock().unwrap().is_empty() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            let starts = machine.starts.lock().unwrap().clone();
            assert_eq!(starts.len(), 1);
            assert_eq!(starts[0][4..], own, "the delayed first start, pressed {install}");
        }
    }
    // Play as anything else asks for it (no profile named): the default one, as on a PC.
    let args = started(&|_, c| assert!(matches!(c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(place()) }), Some(Notice::Sent { .. }))));
    assert!(profile_args(&args).is_empty());
    // No profile VRChat's main start has none of.
    let (machine, c) = ready_mac();
    for profile in [16, 99, u32::MAX] {
        assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile }), Some(Notice::Error(_))));
        assert!(matches!(c.launcher_do(Action::InstallEngine { profile }), Some(Notice::Error(_))));
    }
    assert!(machine.starts.lock().unwrap().is_empty());
}

/// The set-up plays the profile last asked for, when another one is asked for while it runs.
#[test]
fn a_profile_asked_for_during_the_set_up_is_the_one_played() {
    let machine = Arc::new(PretendMac { direct: true, ..Default::default() });
    let c = mac(machine.clone());
    c.launcher_game(machine.as_ref());
    c.update(|s| s.launcher.engine_setup = Some(EngineSetup::Checking));
    c.launcher.lock().unwrap().queued = Some((Errand::plain(LaunchMode::Desktop, Some(1)).to(Some(open_place()), None), Instant::now()));
    assert_eq!(c.launcher_do(Action::InstallEngine { profile: 3 }), None);
    assert_eq!(*machine.installs.lock().unwrap(), 0, "one set-up at a time");
    assert_eq!(c.launcher.lock().unwrap().queued.as_ref().map(|(e, _)| e.clone()), Some(Errand::plain(LaunchMode::Desktop, Some(3))), "whole: not into where the other one was to go");
    // The same one again goes on as it was asked.
    c.launcher.lock().unwrap().queued = Some((Errand::plain(LaunchMode::Desktop, Some(3)).to(Some(open_place()), None), Instant::now()));
    assert_eq!(c.launcher_do(Action::InstallEngine { profile: 3 }), None);
    assert!(c.launcher.lock().unwrap().queued.as_ref().is_some_and(|(e, _)| e.destination.is_some()));
}

/// One VRChat at a time: while it starts or runs on profile 1, no other profile is started on, nothing goes to its account
/// for a press made on another's, and Home says why.
#[test]
fn no_other_profile_starts_while_vrchat_starts_or_runs_on_a_mac() {
    let (machine, c) = ready_mac();
    let (_, calls) = c.test_vrchat(vec![(200, json!({})); 4]);
    signed_in_list(&c);
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Sent { .. })));
    machine.still_starting.store(true, Ordering::Relaxed);
    let refused = |profile: u32| {
        let there = Action::PlayAt { mode: LaunchMode::Desktop, profile, destination: place() };
        [c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile }), c.launcher_do(mine(there))]
    };
    for looked in [false, true] {
        if looked {
            c.launcher_game(machine.as_ref());
        }
        assert_eq!(choice(&c), [(0, None, false, false), (1, None, false, true)], "it starts on 1");
        for profile in [0, 2] {
            assert!(refused(profile).iter().all(|n| matches!(n, Some(Notice::Error(t)) if t == "Starting VRChat…")), "{profile}");
        }
        assert!(c.launcher.lock().unwrap().pending.is_none(), "no destination of another profile waits for this one's game");
    }
    // Again on its own: it goes on, as ever.
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Sent { .. })));
    // It runs.
    machine.still_starting.store(false, Ordering::Relaxed);
    plays(&machine, Some(1), None);
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::Running(LaunchMode::Desktop));
    assert_eq!(choice(&c), [(0, None, false, false), (1, None, true, false)]);
    for profile in [0, 2] {
        assert!(refused(profile).iter().all(|n| matches!(n, Some(Notice::Error(t)) if t == ONE_GAME)), "{profile}");
    }
    assert!(refused(1).iter().all(|n| matches!(n, Some(Notice::Error(t)) if t == "VRChat is already running.")));
    // Its profile not known (yet): no profile is taken for it, and none starts.
    plays(&machine, None, None);
    c.launcher_game(machine.as_ref());
    assert!(choice(&c).iter().all(|p| !p.2));
    for profile in [0, 1, 2] {
        assert!(refused(profile).iter().all(|n| matches!(n, Some(Notice::Error(t)) if t == ONE_GAME)), "{profile}");
    }
    assert!(calls.lock().unwrap().is_empty(), "and nothing was sent for any of them");
    assert_eq!(machine.starts.lock().unwrap().len(), 1);
    // Quit, however soon after its start: another profile starts.
    quit(&machine);
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().launcher.game, Game::NotRunning);
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 2 }), Some(Notice::Sent { .. })));
    assert_eq!(profile_args(&machine.starts.lock().unwrap()[1]), ["--profile=2"]);
}

/// The profile choice on a Mac: the default one, those seen before with the account last seen, the one that runs or starts;
/// an account is learnt only of a profile its own log said, and what is not known stays so.
#[test]
fn the_profile_choice_on_a_mac_says_what_each_profile_s_own_log_said() {
    let (machine, c) = ready_mac();
    assert_eq!(choice(&c), [(0, None, false, false)], "nothing known: the default one");
    // Its log did not say yet who signed in: nobody is named.
    plays(&machine, Some(2), None);
    c.launcher_game(machine.as_ref());
    assert_eq!(choice(&c), [(0, None, false, false), (2, None, true, false)]);
    assert!(c.view().settings.launch.profiles.is_empty());
    plays(&machine, Some(2), Some("Sample Alt"));
    c.launcher_game(machine.as_ref());
    assert_eq!(choice(&c), [(0, None, false, false), (2, Some("Sample Alt".into()), true, false)]);
    quit(&machine);
    c.launcher_game(machine.as_ref());
    assert_eq!(choice(&c), [(0, None, false, false), (2, Some("Sample Alt".into()), false, false)], "remembered");
    // An account said where the profile is not known names no profile.
    plays(&machine, None, Some("Sample Nobody"));
    c.launcher_game(machine.as_ref());
    assert_eq!(c.view().settings.launch.profiles, [(2, "Sample Alt".to_string())].into());
    // Back on the default one, another account.
    plays(&machine, Some(0), Some("Sample Me"));
    c.launcher_game(machine.as_ref());
    assert_eq!(choice(&c), [(0, Some("Sample Me".into()), true, false), (2, Some("Sample Alt".into()), false, false)]);
    // A profile the main start has none of is shown while it runs, never learnt.
    plays(&machine, Some(40), Some("Sample Side"));
    c.launcher_game(machine.as_ref());
    assert!(!c.view().settings.launch.profiles.contains_key(&40));
    // The bridge's is the assistant's, as on a PC.
    machine.players.lock().unwrap()[0] = crate::state::VrchatRunShown { pid: 42, profile: 0, profile_known: true, driver: crate::state::VrchatDriverShown::Agent { session: "main".into() }, ..Default::default() };
    c.launcher_game(machine.as_ref());
    assert!(c.view().launcher.profiles.iter().any(|p| p.profile == 0 && p.assistant && p.running.is_some()));
    // A Mac whose game the person starts by hand in another app gives it no profile: no choice there.
    let by_hand = Arc::new(PretendMac { obelisk: true, vrchat: true, ..Default::default() });
    let c = mac(by_hand.clone());
    c.launcher_game(by_hand.as_ref());
    assert!(choice(&c).is_empty());
    assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Error(_))));
    assert_eq!(*by_hand.opened.lock().unwrap(), 0);
}

/// The launch keeps the profile it was made on: what runs is on it, a start on another is refused over it, and a restart
/// after a lost picture is of it, for the accounts it was for.
#[test]
#[cfg(target_os = "macos")]
fn a_restart_is_on_the_profile_its_launch_was_made_on() {
    use super::super::picture;
    let _publishing = picture::PUBLISHING.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::env::temp_dir().join(format!("orbiters-restart-profile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let machine = pretend_program(&root);
    let session = |n: u32| format!("5e551011-0000-4000-8000-00000000000{n}");
    let plays = || std::fs::read_to_string(root.join("asked")).unwrap_or_default().lines().filter(|l| l.starts_with("steam play")).map(str::to_owned).collect::<Vec<_>>();
    let args = |profile: u32, link: &str| {
        let mut args = vec!["-screen-fullscreen".to_string()];
        args.extend(orbiters_core::vrchat_launch::profile_arg(profile));
        args.push(link.to_string());
        args
    };
    let ended = || machine.play.attached().is_none();
    let (mut started, mut live) = (0, vec![("", 0)]);
    for (account, profile) in [("usr_a", 2), ("usr_b", 0)] {
        let account: &'static str = account;
        live[0] = (account, profile);
        machine.start_for(&args(profile, "vrchat://x"), &errand_of(account)).unwrap();
        started += 1;
        eventually(|| machine.home_session_running(&session(started)));
        let launch = machine.play.newest().unwrap();
        assert_eq!((launch.profile, machine.newest_profile()), (profile, Some(profile)));
        assert_eq!(machine.running_profiles(), [(launch.launcher.unwrap(), true, Some(profile))], "the running state's profile");
        // Another profile over it: the one start refuses it, and nothing is asked of the program.
        assert!(machine.start_for(&args(profile + 1, "vrchat://y"), &errand_of(account)).is_err());
        assert_eq!(plays().len() as u32, started);
        // Lost and restarted: the same profile, the same destination; not for another account.
        picture::publish(Some((session(started), picture::Picture::Lost)));
        assert!(machine.restart_lost(&session(started), &of_account(|who| who != account)).is_err());
        machine.restart_lost(&session(started), &of_account(|who| who == account)).unwrap();
        started += 1;
        eventually(|| machine.home_session_running(&session(started)));
        assert_eq!(machine.newest_profile(), Some(profile));
        let tail = format!("-- {}", args(profile, "vrchat://x").join(" "));
        let last = plays().last().cloned().unwrap();
        assert!(last.ends_with(&tail) && last.matches("--profile").count() == usize::from(profile != 0), "{last}");
        picture::publish(None);
        std::fs::write(root.join(format!("stop-{}", session(started))), "").unwrap();
        eventually(ended);
    }
    let _ = std::fs::remove_dir_all(&root);
}

// --- R1 (eleventh verification, the architect's rulings): a private destination never rides with a start where this computer uses
// several VRChat profiles; it is delivered after it, to that very VRChat, once that one's own log proves its account. ---

const PRIVATE: [&str; 6] = ["private(usr_test)", "private(usr_test)~canRequestInvite", "friends(usr_x)", "hidden(usr_x)", "group(grp_x)~groupAccessType(members)", "group(grp_x)~groupAccessType(plus)"];

fn private_place(of: &str) -> Place {
    let instance_id = format!("7~{of}~region(eu)");
    Place { access: launcher_feed::access(&instance_id).0, instance_id, world_name: "Sample room".into(), ..place() }
}

fn invited_to(c: &Shared, place: Place) {
    c.update(|s| s.launcher.invites = vec![Invite { id: "not_1".into(), kind: InviteKind::Invite, from_id: A.into(), from_name: "Nova".into(), from_icon: String::new(), place: Some(place.clone()), message: String::new(), received_at: 0 }]);
}

/// A PC or a Mac that can start VRChat, signed in as `usr_test` with VRChat answering `replies`.
struct Computer {
    mac: bool,
    pc: Arc<Pretend>,
    apple: Arc<PretendMac>,
    c: Shared,
    calls: Arc<Mutex<Vec<(String, String)>>>,
}

impl Computer {
    fn new(mac: bool) -> Self {
        let pc = Arc::new(Pretend { installed: true, ..Default::default() });
        let apple = Arc::new(PretendMac { obelisk: true, vrchat: true, direct: true, ..Default::default() });
        let c = Controller::new(Settings::default());
        c.launcher_machine(if mac { apple.clone() as Arc<dyn Machine> } else { pc.clone() });
        c.update(|s| s.launcher.possible.play = true);
        let (_, calls) = c.test_vrchat(vec![(200, json!({})); 4]);
        signed_in_list(&c);
        Computer { mac, pc, apple, c, calls }
    }

    fn machine(&self) -> Arc<dyn Machine> {
        if self.mac { self.apple.clone() as Arc<dyn Machine> } else { self.pc.clone() }
    }

    fn starts(&self) -> Vec<Vec<String>> {
        if self.mac { self.apple.starts.lock().unwrap().clone() } else { [self.pc.started.lock().unwrap().clone(), self.pc.beside.lock().unwrap().clone()].concat() }
    }

    /// These profiles are on Home's choice.
    fn remembers(&self, profiles: &[u32]) {
        chips(&self.c, profiles);
    }

    /// These VRChats run: process, profile, whose its own log says, and whether its log is read at all.
    fn runs(&self, runs: &[(u32, u32, Option<&str>, bool)]) {
        let shown: Vec<_> = runs
            .iter()
            .map(|(pid, profile, account, logged)| crate::state::VrchatRunShown { pid: *pid, profile: *profile, profile_known: *logged, account_id: account.map(str::to_owned), logged: *logged, desktop: Some(true), ..Default::default() })
            .collect();
        let pids: Vec<(u32, bool)> = runs.iter().map(|run| (run.0, true)).collect();
        if self.mac {
            *self.apple.running.lock().unwrap() = pids;
            *self.apple.players.lock().unwrap() = shown;
        } else {
            *self.pc.running.lock().unwrap() = pids.clone();
            *self.pc.profiles.lock().unwrap() = runs.iter().map(|run| (run.0, run.1)).collect();
            self.c.update(|s| s.vrchat.runs = shown.clone());
            let fake = super::super::travel::tests::Fake { running: pids.iter().map(|p| p.0).collect(), server: pids.last().map(|p| p.0), ..Default::default() };
            *self.c.travel.lock().unwrap() = Some(Arc::new(fake) as Arc<dyn super::super::travel::Travel>);
        }
    }

    fn look(&self) {
        look(&self.c, self.machine().as_ref());
    }

    /// The launcher's own look, whatever computer this test runs on (a PC's game does not run on a Mac, which it says).
    fn tick(&self) {
        self.c.launcher_game(self.machine().as_ref());
        self.c.update(|s| s.launcher.possible.play = true);
    }

    fn joining(&self) -> Option<Joining> {
        self.c.view().launcher.joining.clone()
    }

    fn step(&self) -> Option<JoinStep> {
        self.joining().map(|j| j.step)
    }

    fn pending(&self) -> Option<Pending> {
        self.c.launcher.lock().unwrap().pending.clone()
    }

    fn at(&self, profile: u32, destination: Place) -> Option<Notice> {
        self.c.launcher_do(mine(Action::PlayAt { mode: LaunchMode::Desktop, profile, destination }))
    }

    fn invites_sent(&self) -> usize {
        self.calls.lock().unwrap().iter().filter(|(_, url)| url.contains("/invite/myself/to/")).count()
    }
}

/// The verifier's case and its kin: whatever way a private place comes (a room, an invite, one made for the account) and whatever
/// is remembered of the profile chosen (its very account's, another's, nobody's), no start carries it, on a PC or a Mac: VRChat
/// starts without any place, Home says both steps, and nothing is sent until that VRChat says whose it is. A public one goes with
/// the start. One that is nobody's is refused. (It was: one whose account was proven by an earlier run went with the start, any
/// other was refused.)
#[test]
fn no_start_carries_a_private_place_where_there_are_several_profiles() {
    for mac in [true, false] {
        let fresh = || {
            let on = Computer::new(mac);
            on.remembers(&[0, 2, 4, 5]);
            on
        };
        let sent = Some(Notice::Sent { to: W.into(), action: SentKind::Launch });
        for kind in PRIVATE {
            for profile in [0, 1, 2, 4, 5] {
                let on = fresh();
                assert_eq!(on.at(profile, private_place(kind)), sent, "{kind} on {profile}");
                let accepted = fresh();
                invited_to(&accepted.c, private_place(kind));
                assert_eq!(accepted.c.launcher_do(mine(Action::AcceptInviteOn { invite: "not_1".into(), mode: LaunchMode::Desktop, profile })), sent);
                for on in [on, accepted] {
                    let starts = on.starts();
                    assert_eq!(starts.len(), 1);
                    assert!(!starts[0].iter().any(|arg| arg.contains("vrchat://") || arg.contains(W)), "{kind} on {profile}: {:?}", starts[0]);
                    assert_eq!(orbiters_core::vrchat_launch::profile_told(&starts[0]), Some(profile));
                    assert_eq!(on.joining(), Some(Joining { world: "Sample room".into(), step: JoinStep::Starting }));
                    assert!(on.pending().is_some_and(|p| p.proof && p.errand.profile == Some(profile)));
                    on.look();
                    assert!(on.calls.lock().unwrap().is_empty(), "nothing is sent before it says whose it is");
                }
            }
            // Nobody's list: nobody it is known to be for.
            let on = fresh();
            assert!(matches!(on.c.launcher_do(Action::PlayAt { mode: LaunchMode::Desktop, profile: 0, destination: private_place(kind) }), Some(Notice::Error(words)) if words.contains("sign in to VRChat")));
            assert!(on.starts().is_empty());
        }
        // An invite into a public room is the account's all the same; the room itself is anyone's, and goes with the start.
        for profile in [0, 2, 4] {
            let on = fresh();
            invited_to(&on.c, open_place());
            assert_eq!(on.c.launcher_do(mine(Action::AcceptInviteOn { invite: "not_1".into(), mode: LaunchMode::Desktop, profile })), sent);
            assert!(!on.starts()[0].iter().any(|arg| arg.contains("vrchat://")));
            let on = fresh();
            assert_eq!(on.at(profile, open_place()), sent);
            assert_eq!(on.starts()[0].last(), Some(&orbiters_core::vrchat_launch::link(W, &open_place().instance_id)));
            assert!(on.joining().is_none() && on.pending().is_none());
        }
        // Without a profile named, the default one's.
        let on = fresh();
        assert_eq!(on.c.launcher_do(mine(Action::Play { mode: LaunchMode::Desktop, destination: Some(private_place("friends(usr_x)")) })), sent);
        assert!(!on.starts()[0].iter().any(|arg| arg.contains("vrchat://")) && on.pending().is_some_and(|p| p.errand.profile.unwrap_or(0) == 0));
    }
}

/// Where VRChat has one profile, a private place goes with the start as it did before there were
/// several, whatever is known of its account (the long-standing behaviour): nothing is asked of a log, nothing said of two steps.
#[test]
fn where_there_is_one_profile_a_private_place_goes_with_the_start() {
    for mac in [true, false] {
        for known in [false, true] {
            for kind in PRIVATE {
                let on = Computer::new(mac);
                if known {
                    on.remembers(&[0]);
                }
                assert_eq!(on.at(0, private_place(kind)), Some(Notice::Sent { to: W.into(), action: SentKind::Launch }), "{known}");
                assert_eq!(on.starts()[0].last(), Some(&orbiters_core::vrchat_launch::link(W, &private_place(kind).instance_id)));
                assert!(on.joining().is_none() && on.pending().is_none() &&  !on.c.launcher_several(on.machine().as_ref(), None));
                let on = Computer::new(mac);
                invited_to(&on.c, private_place(kind));
                on.c.launcher_do(mine(Action::AcceptInvite { invite: "not_1".into(), mode: LaunchMode::Desktop }));
                assert!(on.starts()[0].last().is_some_and(|arg| arg.starts_with("vrchat://")));
                // (Not even an account to be for: as before.)
                let on = Computer::new(mac);
                on.c.launcher_do(Action::Play { mode: LaunchMode::Desktop, destination: Some(private_place(kind)) });
                assert!(on.starts()[0].last().is_some_and(|arg| arg.starts_with("vrchat://")));
            }
        }
    }
}

/// R4: what makes a computer one of several profiles, and nothing else: another than the default one on Home's choice, running,
/// starting or about to be started on, Home's or the bridge's. A VRChat that has not said its profile, one that ended, what earlier
/// versions remembered for good: none of it counts.
#[test]
fn several_profiles_are_those_on_the_choice_or_running_now() {
    for mac in [true, false] {
        let several = |with: &dyn Fn(&Computer)| {
            let on = Computer::new(mac);
            with(&on);
            let found = (on.c.launcher_several(on.machine().as_ref(), None), on.c.launcher_several(on.machine().as_ref(), Some(0)));
            assert_eq!(found.0, found.1);
            on.tick();
            assert_eq!(on.c.view().launcher.several_profiles, found.0, "and Home is told");
            found.0
        };
        assert!(!several(&|_| {}));
        assert!(!several(&|on| on.remembers(&[0])));
        assert!(!several(&|on| on.runs(&[(10, 0, Some("usr_test"), true)])));
        assert!(several(&|on| on.remembers(&[1])));
        assert!(several(&|on| on.runs(&[(10, 2, None, true)])));
        assert!(several(&|on| assert!(matches!(on.c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 1 }), Some(Notice::Sent { .. })))), "it starts");
        assert!(several(&|on| on.c.sides.put(crate::bridge::sessions::SideMark { session: "p20-0123456789ab".into(), profile: 20, osc_in: 9200, osc_out: 9201, pid: None, log: None, query: None })));
        if !mac {
            assert!(!several(&|on| { on.runs(&[(10, 0, None, false)]); on.pc.unread.lock().unwrap().push(10); }), "not said: no evidence");
        }
        let on = Computer::new(mac);
        assert!(on.c.launcher_several(on.machine().as_ref(), Some(1)), "about to be started on");
        // One started, whose account was never seen, that ended: as before it.
        on.runs(&[(10, 2, None, true)]);
        on.tick();
        assert!(on.c.view().launcher.several_profiles);
        on.runs(&[]);
        on.tick();
        assert!(!on.c.view().launcher.several_profiles && !on.c.launcher_several(on.machine().as_ref(), None));
    }
    // What 0.3.9-dev.46 and dev.47 kept of it in the settings is not read.
    let earlier: orbiters_core::vrchat_launch::LaunchSettings = serde_json::from_str(r#"{"profiles":{"0":"Sample Me"},"profile_ids":{"0":"usr_a","3":"usr_b"},"profiles_seen":[2],"profiles_lost":true}"#).unwrap();
    let on = Computer::new(false);
    on.c.update_settings(|s| s.launch = earlier);
    assert!(!on.c.launcher_several(on.machine().as_ref(), None));
}

/// R4: the last other profile taken off Home's choice makes the computer one of one profile again, and a private place goes with
/// the start as it did; one that runs or starts stays.
#[test]
fn the_last_other_profile_taken_off_the_choice_makes_it_one_again() {
    for mac in [true, false] {
        let on = Computer::new(mac);
        on.remembers(&[0, 1, 2]);
        on.tick();
        for (profile, several, left) in [(0, true, vec![0, 1, 2]), (1, true, vec![0, 2]), (2, false, vec![0])] {
            assert_eq!(on.c.launcher_do(Action::ForgetProfile { profile }), None);
            on.c.update(|s| s.launcher.possible.play = true);
            let view = on.c.view();
            assert_eq!((view.launcher.several_profiles, view.settings.launch.profiles.keys().copied().collect::<Vec<_>>()), (several, left.clone()), "mac {mac}: the default one stays");
            if mac || orbiters_core::osc::VRCHAT_RUNS_HERE {
                assert_eq!(view.launcher.profiles.iter().map(|p| p.profile).collect::<Vec<_>>(), left, "Home's choice at once");
            }
        }
        assert!(matches!(on.at(0, private_place("friends(usr_x)")), Some(Notice::Sent { .. })));
        assert_eq!(on.starts()[0].last(), Some(&orbiters_core::vrchat_launch::link(W, &private_place("friends(usr_x)").instance_id)));
        assert!(on.joining().is_none());
        let on = Computer::new(mac);
        on.remembers(&[1]);
        on.runs(&[(10, 1, Some("usr_test"), true)]);
        on.tick();
        assert!(matches!(on.c.launcher_do(Action::ForgetProfile { profile: 1 }), Some(Notice::Error(_))));
        assert!(on.c.view().settings.launch.profiles.contains_key(&1));
    }
}

/// The second step: this very VRChat's own log proves the account, and only then is the place delivered, once: by its link on a
/// PC (an invite to oneself where the link would go to another's VRChat), as an invite to oneself on a Mac. Another VRChat's
/// proof, one from before the start, its sign-in page, count for nothing.
#[test]
fn a_private_place_is_delivered_once_this_very_vrchat_proves_its_account() {
    for mac in [true, false] {
        let on = Computer::new(mac);
        on.remembers(&[0, 2]);
        // A PC: the person's other VRChat runs already, as that very account's (it is another profile's all the same).
        let before: &[(u32, u32, Option<&str>, bool)] = if mac { &[] } else { &[(10, 0, Some("usr_test"), true)] };
        on.runs(before);
        assert!(matches!(on.at(2, private_place("friends(usr_x)")), Some(Notice::Sent { .. })));
        assert_eq!(on.pending().unwrap().before, before.iter().map(|run| run.0).collect::<Vec<_>>());
        let with = |run: (u32, u32, Option<&'static str>, bool)| [before.to_vec(), vec![run]].concat();
        let quiet = |step: JoinStep, why: &str| {
            on.look();
            std::thread::sleep(Duration::from_millis(30));
            assert_eq!((on.step(), on.calls.lock().unwrap().len()), (Some(step), 0), "{why}");
        };
        quiet(JoinStep::Starting, "it did not come up yet");
        on.runs(&with((42, 2, None, false)));
        quiet(if mac { JoinStep::SigningIn } else { JoinStep::SigningIn }, "up, its log not there yet");
        on.runs(&with((42, 2, None, true)));
        quiet(JoinStep::SigningIn, "nobody signed in yet");
        on.runs(&with((42, 2, Some("usr_test"), true)));
        on.look();
        eventually_so(|| on.joining().is_none());
        let written = || on.c.travel.lock().unwrap().is_some();
        if mac {
            assert_eq!(on.invites_sent(), 1);
            assert!(on.calls.lock().unwrap()[0].1.contains(&format!("{W}:7~friends(usr_x)")));
        } else {
            assert!(written());
            assert_eq!(on.invites_sent(), 0, "by its link");
        }
        on.look();
        std::thread::sleep(Duration::from_millis(30));
        assert!(on.pending().is_none() && on.invites_sent() == usize::from(mac), "once");
        assert_eq!(on.starts().len(), 1);
    }
    // A PC whose link pipe is the other VRChat's (it was started first): the invite to oneself, never that one's link.
    let on = Computer::new(false);
    on.remembers(&[0, 2]);
    on.runs(&[(10, 0, Some("usr_other"), true)]);
    on.at(2, private_place("friends(usr_x)"));
    on.runs(&[(10, 0, Some("usr_other"), true), (42, 2, Some("usr_test"), true)]);
    let fake = Arc::new(super::super::travel::tests::Fake { running: vec![10, 42], server: Some(10), ..Default::default() });
    *on.c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
    on.look();
    eventually_so(|| on.invites_sent() == 1);
    assert!(fake.written.lock().unwrap().is_empty());
    // And its own: the link, once.
    let on = Computer::new(false);
    on.remembers(&[2]);
    on.at(2, private_place("friends(usr_x)"));
    on.runs(&[(42, 2, Some("usr_test"), true)]);
    let fake = Arc::new(super::super::travel::tests::Fake { running: vec![42], server: Some(42), ..Default::default() });
    *on.c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
    on.look();
    eventually_so(|| on.joining().is_none());
    assert_eq!(*fake.written.lock().unwrap(), [orbiters_core::vrchat_launch::link(W, "7~friends(usr_x)~region(eu)")]);
    assert_eq!(on.invites_sent(), 0);
}

/// Another account's VRChat, one that does not say in time, one that is gone, one whose log cannot be read: nothing is delivered,
/// Home says which, and the place is kept: pressed again once that VRChat is the account's, it goes; given up, it is gone. A
/// remembered account never helps.
#[test]
fn a_private_place_that_is_not_proven_goes_nowhere_and_is_kept() {
    for mac in [true, false] {
        let begun = || {
            let on = Computer::new(mac);
            on.remembers(&[0, 2]);
            on.at(2, private_place("friends(usr_x)"));
            on
        };
        let failed = |on: &Computer, why: JoinFailure| {
            assert_eq!(on.joining(), Some(Joining { world: "Sample room".into(), step: JoinStep::Failed(why) }));
            assert!(on.pending().is_none());
            std::thread::sleep(Duration::from_millis(30));
            assert!(on.calls.lock().unwrap().is_empty(), "{:?}", on.calls.lock().unwrap());
        };
        // Another account's.
        let on = begun();
        on.runs(&[(42, 2, Some("usr_other"), true)]);
        on.look();
        failed(&on, JoinFailure::OtherAccount);
        // Again, still another's; then the account signs in there: it goes, and no other VRChat was started for it.
        assert!(matches!(on.c.launcher_do(Action::JoinAgain), Some(Notice::Sent { .. })));
        on.look();
        failed(&on, JoinFailure::OtherAccount);
        on.runs(&[(42, 2, None, true)]);
        assert!(matches!(on.c.launcher_do(Action::JoinAgain), Some(Notice::Sent { .. })));
        assert_eq!(on.step(), Some(JoinStep::SigningIn));
        on.runs(&[(42, 2, Some("usr_test"), true)]);
        on.look();
        eventually_so(|| on.joining().is_none());
        assert_eq!((on.starts().len(), on.invites_sent()), (1, usize::from(mac)));
        assert_eq!(on.c.launcher_do(Action::JoinAgain), None, "nothing is kept any more");
        // It does not say in time.
        let on = begun();
        on.runs(&[(42, 2, None, true)]);
        on.look();
        assert_eq!(on.step(), Some(JoinStep::SigningIn));
        on.c.launcher.lock().unwrap().pending.as_mut().unwrap().seen = Some((42, Instant::now().checked_sub(PROOF_WITHIN + Duration::from_secs(1)).unwrap()));
        on.look();
        failed(&on, JoinFailure::NoProof);
        assert_eq!(on.c.launcher_do(Action::ForgetJoining), None);
        assert!(on.joining().is_none() && on.c.launcher.lock().unwrap().kept.is_none());
        // It closes before; it never comes.
        let on = begun();
        on.runs(&[(42, 2, None, true)]);
        on.look();
        if !mac {
            // (One that goes as soon as it came may be followed by the one that stays: that one is it, for a minute.)
            on.runs(&[]);
            on.look();
            assert_eq!(on.step(), Some(JoinStep::Starting));
            on.runs(&[(43, 2, None, true)]);
            on.look();
            assert_eq!((on.step(), on.pending().unwrap().seen.map(|s| s.0)), (Some(JoinStep::SigningIn), Some(43)));
            on.c.launcher.lock().unwrap().pending.as_mut().unwrap().seen = Some((43, Instant::now().checked_sub(AGAIN_WITHIN + Duration::from_secs(1)).unwrap()));
        }
        on.runs(&[]);
        on.look();
        failed(&on, JoinFailure::Gone);
        // (Started anew: again in two steps.)
        *on.c.launcher_started.lock().unwrap() = None;
        on.c.launcher.lock().unwrap().starting.clear();
        assert!(matches!(on.c.launcher_do(Action::JoinAgain), Some(Notice::Sent { .. })));
        assert_eq!(on.starts().len(), 2);
        assert!(!on.starts()[1].iter().any(|arg| arg.contains("vrchat://")));
        assert_eq!(on.step(), Some(JoinStep::Starting));
        on.c.launcher.lock().unwrap().pending.as_mut().unwrap().since = Instant::now().checked_sub(PENDING_FOR + Duration::from_secs(1)).unwrap();
        on.look();
        failed(&on, JoinFailure::Gone);
        // Given up while it waits: nothing goes.
        let on = begun();
        on.c.launcher_do(Action::ForgetJoining);
        on.runs(&[(42, 2, Some("usr_test"), true)]);
        on.look();
        std::thread::sleep(Duration::from_millis(30));
        assert!(on.joining().is_none() && on.calls.lock().unwrap().is_empty());
        // Another place joined meanwhile: the one that waited is over.
        let on = begun();
        on.runs(&[(42, 2, Some("usr_test"), true)]);
        assert!(matches!(on.c.launcher_do(mine(Action::JoinRunning { mode: LaunchMode::Desktop, destination: open_place() })), Some(Notice::Sent { .. })));
        let sent = on.calls.lock().unwrap().len();
        on.look();
        std::thread::sleep(Duration::from_millis(30));
        assert!(on.pending().is_none() && on.joining().is_none() && on.calls.lock().unwrap().len() == sent);
        // The account goes meanwhile: nothing of it goes to another one's session.
        let on = begun();
        on.runs(&[(42, 2, Some("usr_test"), true)]);
        on.c.launcher.lock().unwrap().pending.as_mut().unwrap().errand.account = Some("usr_gone".into());
        on.look();
        // (Said to that account only, which is not signed in: verify4-r21.)
        assert!(on.joining().is_none() && on.pending().is_none());
        std::thread::sleep(Duration::from_millis(30));
        assert!(on.calls.lock().unwrap().is_empty(), "{:?}", on.calls.lock().unwrap());
    }
}

/// A Mac: the proof is of the very start it was asked with: another start since (the bridge's), or a VRChat on another profile, is
/// not it.
#[test]
fn another_start_on_a_mac_is_not_the_one_a_private_place_waits_for() {
    let on = Computer::new(true);
    on.remembers(&[0, 2]);
    on.at(2, private_place("friends(usr_x)"));
    // The bridge's start, on the very profile and account.
    on.apple.starts.lock().unwrap().push(vec!["--profile=2".into()]);
    on.runs(&[(42, 2, Some("usr_test"), true)]);
    on.look();
    std::thread::sleep(Duration::from_millis(30));
    // (Kept for Try again: the ruling after the twelfth verification; it was forgotten.)
    let kept = |on: &Computer| {
        assert!(on.calls.lock().unwrap().is_empty() && on.pending().is_none());
        assert_eq!(on.step(), Some(JoinStep::Failed(JoinFailure::Elsewhere)));
        assert_eq!(on.c.launcher.lock().unwrap().kept.as_ref().and_then(|e| e.destination.clone()), Some(private_place("friends(usr_x)")));
    };
    kept(&on);
    // Its launch says it became another process than the one that was seen; and one that has not said which one it became is none
    // of those that run.
    for said in [Some(43), None] {
        let on = Computer::new(true);
        on.remembers(&[2]);
        on.at(2, private_place("friends(usr_x)"));
        *on.apple.launched.lock().unwrap() = Some(said.map(|_| 42));
        on.runs(&[(42, 2, said.is_none().then_some("usr_test"), true)]);
        on.look();
        std::thread::sleep(Duration::from_millis(30));
        assert!(on.calls.lock().unwrap().is_empty(), "{said:?}");
        if said.is_none() {
            assert_eq!(on.step(), Some(JoinStep::Starting), "not said to be it");
            continue;
        }
        *on.apple.launched.lock().unwrap() = Some(said);
        on.runs(&[(42, 2, Some("usr_test"), true)]);
        on.look();
        std::thread::sleep(Duration::from_millis(30));
        kept(&on);
    }
    // What came up runs on another profile.
    let on = Computer::new(true);
    on.remembers(&[2]);
    on.at(2, private_place("friends(usr_x)"));
    on.runs(&[(42, 0, Some("usr_test"), true)]);
    on.look();
    std::thread::sleep(Duration::from_millis(30));
    kept(&on);
    // Try again: that VRChat is of another profile, so that one is started for it again.
    quit(&on.apple);
    *on.c.launcher_started.lock().unwrap() = None;
    assert!(matches!(on.c.launcher_do(Action::JoinAgain), Some(Notice::Sent { .. })));
    assert_eq!(on.starts().len(), 2);
}

/// The invite a two-step start was for: its card comes back when it was not delivered, and pressed again it is accepted from it.
#[test]
fn an_invite_that_was_not_delivered_has_its_card_back() {
    let on = Computer::new(true);
    on.remembers(&[2]);
    let place = private_place("private(usr_x)");
    invited_to(&on.c, place.clone());
    assert!(matches!(on.c.launcher_do(mine(Action::AcceptInviteOn { invite: "not_1".into(), mode: LaunchMode::Desktop, profile: 2 })), Some(Notice::Sent { .. })));
    assert!(on.c.view().launcher.invites.is_empty());
    on.runs(&[(42, 2, Some("usr_other"), true)]);
    on.look();
    assert_eq!(on.step(), Some(JoinStep::Failed(JoinFailure::OtherAccount)));
    on.c.launcher_settle_accepted(None, Some(&by(&on.c)));
    assert_eq!(on.c.view().launcher.invites.len(), 1, "its card is back");
    on.runs(&[(42, 2, Some("usr_test"), true)]);
    assert!(matches!(on.c.launcher_do(mine(Action::JoinAgain)), Some(Notice::Sent { .. })));
    assert!(on.c.view().launcher.invites.is_empty() && on.c.launcher.lock().unwrap().accepted.len() == 1);
    on.look();
    eventually_so(|| on.invites_sent() == 1);
}

/// R6: VRChat runs and its log cannot be read: Home is told why once it has had the time to make one, a place that waits for it
/// says so, and it is said no more once a log is read. A VRChat that was not up long enough says nothing.
#[test]
fn a_log_that_cannot_be_read_is_said_and_not_an_unknown_account() {
    for trouble in [LogTrouble::NoFolder, LogTrouble::NotWritten, LogTrouble::Denied("/x".into())] {
        let on = Computer::new(true);
        on.remembers(&[2]);
        *on.apple.log_problem.lock().unwrap() = Some(trouble.clone());
        on.at(2, private_place("friends(usr_x)"));
        on.runs(&[(42, 2, None, false)]);
        // (A Mac knows the profile by its own start's command line, whatever its log.)
        on.apple.players.lock().unwrap()[0].profile_known = true;
        on.look();
        assert_eq!((on.step(), on.c.launcher.lock().unwrap().log_trouble.clone()), (Some(JoinStep::SigningIn), None), "not yet");
        let long_ago = Instant::now().checked_sub(LOG_WITHIN + Duration::from_secs(1)).unwrap();
        on.c.launcher.lock().unwrap().unlogged.insert(42, long_ago);
        on.c.launcher.lock().unwrap().pending.as_mut().unwrap().seen = Some((42, long_ago));
        on.look();
        assert_eq!(on.step(), Some(JoinStep::Failed(JoinFailure::Log(trouble.clone()))));
        assert_eq!(on.c.launcher.lock().unwrap().log_trouble, Some(trouble.clone()));
        on.c.launcher_game(on.machine().as_ref());
        assert_eq!(on.c.view().launcher.log_trouble, Some(trouble), "Home's, also once it was quit");
        on.runs(&[(42, 2, None, true)]);
        on.c.launcher_game(on.machine().as_ref());
        assert_eq!(on.c.view().launcher.log_trouble, None, "read again");
        assert!(on.calls.lock().unwrap().is_empty());
    }
    // What the folder says.
    let dir = std::env::temp_dir().join(format!("orbiters-log-trouble-{}", uuid::Uuid::new_v4()));
    assert_eq!(log_trouble(&dir), Some(LogTrouble::NoFolder));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(log_trouble(&dir), Some(LogTrouble::NotWritten));
    let log = dir.join("output_log_2026-11-24_00-00-00.txt");
    std::fs::write(&log, "x").unwrap();
    assert_eq!(log_trouble(&dir), None);
    let old = std::time::SystemTime::now() - LOG_STILL - Duration::from_secs(5);
    std::fs::File::options().write(true).open(&log).unwrap().set_modified(old).unwrap();
    assert_eq!(log_trouble(&dir), Some(LogTrouble::NotWritten), "an earlier run's");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&log, "x").unwrap();
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(log_trouble(&dir), Some(LogTrouble::Denied(log.display().to_string())));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(log_trouble(&dir), Some(LogTrouble::Denied(dir.display().to_string())));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// R6 (the verifier's race): Join, or an invite accepted into the running game, whose game goes between the two looks at the
/// processes, or of which only the assistant's remain: nothing is claimed and nothing started, the place public or private.
#[test]
fn a_join_whose_game_goes_between_two_looks_starts_nothing() {
    for destination in [open_place(), private_place("friends(usr_x)")] {
        for sides in [false, true] {
            let on = Computer::new(false);
            on.runs(&[(10, 0, Some("usr_test"), true)]);
            if sides {
                on.c.sides.put(crate::bridge::sessions::SideMark { session: "p1-0123456789ab".into(), profile: 1, osc_in: 9010, osc_out: 9011, pid: Some(10), log: None, query: None });
            } else {
                let fake = super::super::travel::tests::Fake::default();
                *on.c.travel.lock().unwrap() = Some(Arc::new(fake) as Arc<dyn super::super::travel::Travel>);
            }
            let gone = Some(no_longer_running());
            assert_eq!(on.c.launcher_do(mine(Action::JoinRunning { mode: LaunchMode::Desktop, destination: destination.clone() })), gone);
            invited_to(&on.c, destination.clone());
            assert_eq!(on.c.launcher_do(mine(Action::AcceptInviteRunning { invite: "not_1".into(), mode: LaunchMode::Desktop })), gone);
            assert_eq!(on.c.view().launcher.invites.len(), 1, "its card stays");
            assert!(on.starts().is_empty() && on.c.launcher.lock().unwrap().starting.is_empty() && on.pending().is_none(), "sides: {sides}");
            assert!(on.calls.lock().unwrap().is_empty());
        }
    }
}

/// Beside the assistant's VRChats alone, a private place is started for in two steps too: on the default profile, the place after.
#[test]
fn beside_the_assistant_s_vrchats_a_private_place_comes_after_the_start() {
    let on = Computer::new(false);
    on.runs(&[(10, 1, Some("usr_side"), true)]);
    on.c.sides.put(crate::bridge::sessions::SideMark { session: "p1-0123456789ab".into(), profile: 1, osc_in: 9010, osc_out: 9011, pid: Some(10), log: None, query: None });
    assert!(matches!(on.c.launcher_do(mine(Action::Play { mode: LaunchMode::Desktop, destination: Some(private_place("friends(usr_x)")) })), Some(Notice::Sent { .. })));
    let starts = on.starts();
    assert_eq!((starts.len(), starts[0].iter().any(|arg| arg.contains("vrchat://"))), (1, false), "{starts:?}");
    assert!(on.pending().is_some_and(|p| p.before == [10] && p.errand.profile == Some(0)));
}

/// R4 (the verifier's case): a public instance made for X stays X's, however long ago, whatever was listed since and whoever is
/// signed in: pressed as Y it is refused; pressed as X it is private.
#[test]
fn an_instance_made_for_an_account_stays_its_own() {
    for mac in [true, false] {
        let on = Computer::new(mac);
        on.remembers(&[2]);
        on.c.made_for_tests(W, &open_place().instance_id, "usr_test");
        on.c.made_for_tests(W, "5~region(eu)", "usr_other");
        assert!(matches!(on.at(2, open_place()), Some(Notice::Sent { .. })));
        assert!(!on.starts()[0].iter().any(|arg| arg.contains("vrchat://")), "its own alone: not with the start");
        assert!(on.pending().is_some_and(|p| p.errand.bound));
        let theirs = Place { instance_id: "5~region(eu)".into(), ..open_place() };
        for action in [
            Action::PlayAt { mode: LaunchMode::Desktop, profile: 2, destination: theirs.clone() },
            Action::Play { mode: LaunchMode::Desktop, destination: Some(theirs.clone()) },
            Action::JoinRunning { mode: LaunchMode::Desktop, destination: theirs.clone() },
        ] {
            assert_eq!(on.c.launcher_do(mine(action)), Some(made_for_another()));
        }
        assert_eq!(on.starts().len(), 1);
    }
}

/// Windows: a running VRChat takes the link of a private place only while its own log says it is that account's, its command
/// line and its log agreeing on its profile; else the account's own invite goes. A public one goes to it as ever.
#[test]
fn a_running_vrchat_of_another_account_is_handed_no_private_link() {
    let machine = Arc::new(Pretend { installed: true, ..Default::default() });
    machine.running.lock().unwrap().push((10, false));
    let c = controller(machine.clone());
    let (_, calls) = c.test_vrchat(vec![(200, json!({})); 8]);
    signed_in_list(&c);
    chips(&c, &[1]);
    let fake = Arc::new(super::super::travel::tests::Fake { running: vec![10], server: Some(10), ..Default::default() });
    *c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
    let written = || fake.written.lock().unwrap().len();
    let join = |destination: Place| c.launcher_do(mine(Action::JoinRunning { mode: LaunchMode::Desktop, destination }));
    for (n, account) in [None, Some("usr_other")].into_iter().enumerate() {
        runs_as(&c, 10, 0, account);
        assert_eq!(join(place()), Some(Notice::Sent { to: W.into(), action: SentKind::Join }));
        assert_eq!((written(), calls.lock().unwrap().len()), (0, n + 1), "the account's own invite");
        assert!(calls.lock().unwrap().last().unwrap().1.contains("/invite/myself/to/"));
    }
    // Its log says profile 1, its command line 0: whose log that is cannot be told.
    runs_as(&c, 10, 1, Some("usr_test"));
    join(place());
    assert_eq!(written(), 0);
    runs_as(&c, 10, 0, Some("usr_test"));
    join(place());
    assert_eq!(written(), 1, "its own");
    runs_as(&c, 10, 0, None);
    join(open_place());
    assert_eq!(written(), 2, "a public place");
}

/// A Join for the game that runs, gone by the time it is done (or never there): nothing is started for it, on the default
/// profile or any; the press says so. While it starts, the account's own invite waits for it.
#[test]
fn a_join_for_a_game_that_is_gone_starts_nothing() {
    for private in [false, true] {
        let to = if private { place() } else { open_place() };
        let join = Action::JoinRunning { mode: LaunchMode::Desktop, destination: to.clone() };
        let gone = Some(no_longer_running());
        let invited = |c: &Shared| c.update(|s| s.launcher.invites = vec![Invite { id: "not_1".into(), kind: InviteKind::Invite, from_id: A.into(), from_name: "Nova".into(), from_icon: String::new(), place: Some(to.clone()), message: String::new(), received_at: 0 }]);
        let accept = Action::AcceptInviteRunning { invite: "not_1".into(), mode: LaunchMode::Desktop };
        // A PC.
        let pc = Arc::new(Pretend { installed: true, ..Default::default() });
        let c = controller(pc.clone());
        let (_, _) = c.test_vrchat(vec![]);
        signed_in_list(&c);
        invited(&c);
        assert_eq!(c.launcher_do(mine(join.clone())), gone);
        assert_eq!(c.launcher_do(mine(accept.clone())), gone);
        assert_eq!(c.view().launcher.invites.len(), 1, "its card stays");
        assert!(pc.started.lock().unwrap().is_empty());
        // A Mac, set up or not.
        for installed in [true, false] {
            let machine = Arc::new(PretendMac { obelisk: true, vrchat: installed, direct: true, ..Default::default() });
            let c = mac(machine.clone());
            let (_, calls) = c.test_vrchat(vec![(200, json!({})); 2]);
            signed_in_list(&c);
            c.launcher_game(machine.as_ref());
            invited(&c);
            assert_eq!(c.launcher_do(mine(join.clone())), gone);
            assert_eq!(c.launcher_do(mine(accept.clone())), gone);
            assert!(machine.starts.lock().unwrap().is_empty() && *machine.installs.lock().unwrap() == 0);
            assert!(c.launcher.lock().unwrap().queued.is_none() && c.launcher.lock().unwrap().pending.is_none());
            if !installed {
                continue;
            }
            // It starts on profile 2: a public place's invite to oneself goes once it is up, a private one's once that very
            // VRChat proves whose it is (R1).
            assert!(matches!(c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 2 }), Some(Notice::Sent { .. })));
            assert!(matches!(c.launcher_do(mine(join.clone())), Some(Notice::Sent { .. })));
            assert_eq!(machine.starts.lock().unwrap().len(), 1);
            assert!(calls.lock().unwrap().is_empty());
            plays(&machine, Some(2), None);
            c.launcher_game(machine.as_ref());
            if private {
                std::thread::sleep(Duration::from_millis(60));
                assert!(calls.lock().unwrap().is_empty(), "whose it is, it has not said");
                machine.players.lock().unwrap()[0].account_id = Some("usr_test".into());
                c.launcher_game(machine.as_ref());
            }
            eventually_so(|| calls.lock().unwrap().len() == 1);
            assert!(calls.lock().unwrap()[0].1.contains("/invite/myself/to/"));
        }
    }
}

fn eventually_so(done: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !done() {
        assert!(Instant::now() < until, "it never came");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The verifier's case: a destination asked for before VRChat is set up goes with the start that follows, on its profile, once
/// (a private one after it: R1). One whose account is no longer the one signed in by then is dropped and said: nothing starts.
#[test]
fn a_destination_asked_for_before_the_set_up_goes_whole_or_not_at_all() {
    let queued = |destination: Place, between: &dyn Fn(&Shared)| {
        let machine = Arc::new(PretendMac { direct: true, obelisk: true, ..Default::default() });
        let c = mac(machine.clone());
        let (_, _) = c.test_vrchat(vec![]);
        signed_in_list(&c);
        c.launcher_game(machine.as_ref());
        // A set-up is under way: the press waits for it.
        c.update(|s| s.launcher.engine_setup = Some(EngineSetup::Checking));
        let pressed = c.launcher_do(mine(Action::PlayAt { mode: LaunchMode::Desktop, profile: 2, destination }));
        assert_eq!(pressed, Some(Notice::Sent { to: W.into(), action: SentKind::Launch }));
        std::thread::sleep(Duration::from_millis(50));
        assert!(machine.starts.lock().unwrap().is_empty());
        between(&c);
        // It ends.
        machine.got_vrchat.store(true, Ordering::Relaxed);
        c.launcher.lock().unwrap().play_when_ready = Some(Instant::now());
        c.launcher_game(machine.as_ref());
        (machine, c)
    };
    for destination in [open_place(), place()] {
        let (machine, c) = queued(destination.clone(), &|_| {});
        eventually_so(|| !machine.starts.lock().unwrap().is_empty());
        let starts = machine.starts.lock().unwrap().clone();
        assert_eq!(starts.len(), 1);
        let private = destination == place();
        let link = (!private).then(|| orbiters_core::vrchat_launch::link(W, &destination.instance_id));
        assert_eq!(starts[0][4..], ["--profile=2".to_string()].into_iter().chain(link).collect::<Vec<_>>());
        eventually_so(|| c.launcher.lock().unwrap().pending.is_some() == private);
        assert!(c.launcher.lock().unwrap().queued.is_none(), "used once");
    }
    let dropped = |destination: Place, between: &dyn Fn(&Shared)| {
        let (machine, c) = queued(destination, between);
        eventually_so(|| matches!(c.view().launcher.notice, Some(Notice::Error(_))));
        assert!(machine.starts.lock().unwrap().is_empty() && c.launcher.lock().unwrap().queued.is_none());
        c.view().launcher.notice.clone()
    };
    for to in [place(), open_place()] {
        // The account it was asked for is no longer the one signed in.
        let gone = dropped(to, &|c| c.launcher.lock().unwrap().queued.as_mut().unwrap().0.account = Some("usr_gone".into()));
        assert_eq!(gone, Some(changed_account()));
    }
    // One that is nobody's waits for nothing.
    let machine = Arc::new(PretendMac { direct: true, obelisk: true, ..Default::default() });
    let c = mac(machine.clone());
    let (_, _) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    c.launcher_game(machine.as_ref());
    assert!(matches!(c.launcher_do(Action::PlayAt { mode: LaunchMode::Desktop, profile: 2, destination: place() }), Some(Notice::Error(_))));
    std::thread::sleep(Duration::from_millis(50));
    assert!(c.launcher.lock().unwrap().queued.is_none() && *machine.installs.lock().unwrap() == 0);
}

/// A restart after a lost picture is held to the rule too: with its private place where there is one profile; where there are
/// several, without it, and the place after it.
#[test]
fn a_restart_carries_no_private_place_where_there_are_several_profiles() {
    let machine = Arc::new(PretendMac { obelisk: true, vrchat: true, direct: true, ..Default::default() });
    *machine.running.lock().unwrap() = vec![(42, true)];
    *machine.lost.lock().unwrap() = Some("s".into());
    let c = mac(machine.clone());
    let (_, _) = c.test_vrchat(vec![]);
    signed_in_list(&c);
    plays(&machine, Some(0), None);
    c.launcher_game(machine.as_ref());
    let launch = |profile: u32, place: &Place, bound: bool| HomeLaunch {
        args: orbiters_core::vrchat_launch::profile_arg(profile).into_iter().chain([orbiters_core::vrchat_launch::link(W, &place.instance_id)]).collect(),
        account: Some("usr_test".into()),
        bound,
        ..Default::default()
    };
    // What it is made with again, and whether its place comes after it; `None`: refused.
    let asked = |launch: HomeLaunch| {
        c.launcher.lock().unwrap().pending = None;
        *machine.restarted.lock().unwrap() = Some(launch);
        let (before, made) = (machine.restarts.lock().unwrap().len(), machine.restarted_with.lock().unwrap().len());
        assert_eq!(c.launcher_do(Action::RestartLostGame { session: "s".into() }), None);
        eventually_so(|| machine.restarts.lock().unwrap().len() > before);
        let with = machine.restarted_with.lock().unwrap().get(made).cloned()?;
        let private = !with.iter().any(|arg| arg.starts_with("vrchat://"));
        if private {
            eventually_so(|| c.launcher.lock().unwrap().pending.as_ref().is_some_and(|p| p.proof));
        }
        Some((with, private))
    };
    let link = |place: &Place| orbiters_core::vrchat_launch::link(W, &place.instance_id);
    assert_eq!(asked(launch(0, &place(), false)), Some((vec![link(&place())], false)), "one profile: as it was");
    assert_eq!(asked(launch(2, &open_place(), false)), Some((vec!["--profile=2".into(), link(&open_place())], false)), "a public place");
    assert_eq!(asked(launch(2, &place(), false)), Some((vec!["--profile=2".into()], true)));
    assert_eq!(asked(launch(2, &open_place(), true)), Some((vec!["--profile=2".into()], true)), "an invite's");
    chips(&c, &[2]);
    assert_eq!(asked(launch(0, &place(), false)), Some((vec![], true)), "another one is on the choice");
    // Started in two steps: its place comes after it again.
    let after = HomeLaunch { args: vec!["--profile=2".into()], account: Some("usr_test".into()), destination: Some(place()), ..Default::default() };
    assert_eq!(asked(after.clone()), Some((vec!["--profile=2".into()], true)));
    // Another account's by now, or nobody's: refused.
    assert_eq!(asked(HomeLaunch { account: Some("usr_gone".into()), ..after }), None);
    assert_eq!(asked(HomeLaunch { account: None, ..launch(2, &place(), false) }), None);
}

// --- The twelfth verification's checks, as the architect ruled on them (AGENTS.md, Several VRChat accounts on one computer). ---

/// A fresh update on a computer of one account, whose game's logs cannot be read: Play, Join into the game that runs, an invite
/// accepted, and a private place of Where all go as they did before there were profiles, on a PC and on a Mac; no log is asked.
#[test]
fn one_account_whose_logs_cannot_be_read_meets_no_dead_end() {
    let private = || private_place("friends(usr_x)");
    let link = || orbiters_core::vrchat_launch::link(W, &private().instance_id);
    let launched = Some(Notice::Sent { to: W.into(), action: SentKind::Launch });
    let joined = Some(Notice::Sent { to: W.into(), action: SentKind::Join });
    for mac in [true, false] {
        let fresh = || {
            let on = Computer::new(mac);
            let trouble = Some(LogTrouble::Denied("/x".into()));
            *on.pc.log_problem.lock().unwrap() = trouble.clone();
            *on.apple.log_problem.lock().unwrap() = trouble;
            on
        };
        // Nothing runs: Play; Where's private place; an invite.
        let on = fresh();
        assert!(matches!(on.c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 0 }), Some(Notice::Sent { .. })), "mac {mac}");
        assert_eq!(on.starts().len(), 1);
        let on = fresh();
        assert_eq!(on.at(0, private()), launched);
        assert_eq!(on.starts()[0].last(), Some(&link()));
        assert!(on.pending().is_none() && on.joining().is_none());
        let on = fresh();
        invited_to(&on.c, private());
        assert_eq!(on.c.launcher_do(mine(Action::AcceptInviteOn { invite: "not_1".into(), mode: LaunchMode::Desktop, profile: 0 })), launched);
        assert_eq!(on.starts()[0].last(), Some(&link()));
        // It runs, and neither its log nor its command line can be read: the friend's Join, the invite's Join now.
        for accept in [false, true] {
            let on = fresh();
            on.runs(&[(10, 0, None, false)]);
            on.pc.unread.lock().unwrap().push(10);
            let fake = Arc::new(super::super::travel::tests::Fake { running: vec![10], server: Some(10), unknown: vec![10], ..Default::default() });
            *on.c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
            invited_to(&on.c, private());
            let press = if accept { Action::AcceptInviteRunning { invite: "not_1".into(), mode: LaunchMode::Desktop } } else { Action::JoinRunning { mode: LaunchMode::Desktop, destination: private() } };
            assert_eq!(on.c.launcher_do(mine(press)), joined, "mac {mac}, accept {accept}");
            if mac {
                assert_eq!(on.invites_sent(), 1, "the account's own invite, as ever on a Mac");
            } else {
                assert_eq!(*fake.written.lock().unwrap(), [link()], "its link, as before there were profiles");
                assert_eq!(on.invites_sent(), 0);
            }
            assert!(on.starts().is_empty());
        }
    }
}

/// The same press on a PC of several profiles: the game cannot be told from the assistant's, nor whose it is: the account's own
/// invite goes, nothing is written to it and none is started; with no account, what the log's trouble is.
#[test]
fn a_game_that_cannot_be_identified_takes_the_account_s_own_invite() {
    for destination in [open_place(), private_place("friends(usr_x)")] {
        let on = Computer::new(false);
        on.remembers(&[1]);
        *on.pc.log_problem.lock().unwrap() = Some(LogTrouble::NotWritten);
        on.runs(&[(10, 0, None, false)]);
        let fake = Arc::new(super::super::travel::tests::Fake { running: vec![10], server: Some(10), unknown: vec![10], ..Default::default() });
        *on.c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
        let join = Action::JoinRunning { mode: LaunchMode::Desktop, destination: destination.clone() };
        assert_eq!(on.c.launcher_do(mine(join.clone())), Some(Notice::Sent { to: W.into(), action: SentKind::Join }));
        assert_eq!((on.invites_sent(), fake.written.lock().unwrap().len(), fake.started.lock().unwrap().len()), (1, 0, 0));
        // Nobody signed in, and nobody's press.
        let (machine, c) = (on.pc.clone(), controller(on.pc.clone()));
        chips(&c, &[1]);
        *c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
        assert!(matches!(c.launcher_do(join), Some(Notice::Error(words)) if words.contains("turn on Logging")), "what is wrong, and no \"no longer running\"");
        assert!(machine.started.lock().unwrap().is_empty() && fake.written.lock().unwrap().is_empty());
    }
}

/// A place whose VRChat could not be told whose it is (no log, or none that says in time) has a way on: pressed again, the
/// account's own invite goes, and it is over.
#[test]
fn a_place_that_could_not_be_proven_goes_as_the_account_s_own_invite() {
    for mac in [true, false] {
        for why in [JoinFailure::NoProof, JoinFailure::Log(LogTrouble::NotWritten)] {
            let on = Computer::new(mac);
            on.remembers(&[2]);
            on.at(2, private_place("friends(usr_x)"));
            on.runs(&[(42, 2, None, false)]);
            if !mac {
                on.pc.unread.lock().unwrap().push(42);
            }
            let errand = on.c.launcher.lock().unwrap().pending.take().unwrap().errand;
            on.c.launcher_join_failed(errand, why.clone());
            assert!(matches!(on.c.launcher_do(Action::JoinAgain), Some(Notice::Sent { .. })), "mac {mac}, {why:?}");
            assert_eq!((on.invites_sent(), on.starts().len()), (1, 1), "its own invite, and no other start");
            assert!(on.joining().is_none() && on.c.launcher.lock().unwrap().kept.is_none());
        }
    }
}

/// The verifier's case: a player switches accounts in the game after the last look; or the pipe is served by another process of
/// the very account than the one it was proven for. Looked at the moment the link would be written: nothing is, and the account's
/// own invite goes.
#[test]
fn a_private_link_is_written_only_to_the_very_vrchat_proven_at_that_moment() {
    let shown = |pid: u32, account: &str| crate::state::VrchatRunShown { pid, profile: 2, profile_known: true, account_id: Some(account.into()), logged: true, ..Default::default() };
    let proven_for = |running: Vec<u32>, server: u32, now: Vec<crate::state::VrchatRunShown>| {
        let on = Computer::new(false);
        on.remembers(&[2]);
        on.at(2, private_place("friends(usr_x)"));
        on.runs(&[(42, 2, Some("usr_test"), true)]);
        let fake = Arc::new(super::super::travel::tests::Fake { running, server: Some(server), ..Default::default() });
        *on.c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
        *on.pc.now.lock().unwrap() = Some(now);
        on.look();
        eventually_so(|| on.joining().is_none());
        let written = fake.written.lock().unwrap().len();
        (written, on.invites_sent())
    };
    assert_eq!(proven_for(vec![42], 42, vec![shown(42, "usr_test")]), (1, 0));
    assert_eq!(proven_for(vec![42], 42, vec![shown(42, "usr_other")]), (0, 1), "signed in as another since");
    assert_eq!(proven_for(vec![42], 42, vec![]), (0, 1), "its log cannot be read any more");
    assert_eq!(proven_for(vec![42, 43], 43, vec![shown(42, "usr_test"), shown(43, "usr_test")]), (0, 1), "another process than the one proven");
    // The Join of a friend's card, the same.
    let on = Computer::new(false);
    on.remembers(&[1]);
    on.runs(&[(10, 0, Some("usr_test"), true)]);
    *on.pc.now.lock().unwrap() = Some(vec![crate::state::VrchatRunShown { pid: 10, profile_known: true, account_id: Some("usr_other".into()), ..Default::default() }]);
    on.c.launcher_do(mine(Action::JoinRunning { mode: LaunchMode::Desktop, destination: private_place("friends(usr_x)") }));
    assert_eq!(on.invites_sent(), 1);
}

/// No start is held while VRChat or a download answers: the link's pipe and the invite that follows on a PC, the program got
/// before a start on a Mac.
#[test]
fn nothing_holds_the_start_while_the_network_answers() {
    struct Watching(std::sync::Weak<Controller>, Arc<Mutex<Vec<bool>>>);
    impl super::super::travel::Travel for Watching {
        fn running(&self) -> Vec<u32> {
            vec![10]
        }
        fn whose(&self, _: u32) -> super::super::travel::Side {
            super::super::travel::Side::Person
        }
        fn connect(&self) -> std::io::Result<(Box<dyn super::super::travel::LinkPipe>, Option<u32>)> {
            let held = self.0.upgrade().is_some_and(|c| c.launcher_started.try_lock().is_err());
            self.1.lock().unwrap().push(held);
            Err(std::io::ErrorKind::NotFound.into())
        }
        fn start(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }
    let on = Computer::new(false);
    on.runs(&[(10, 0, Some("usr_test"), true)]);
    let held = Arc::new(Mutex::new(Vec::new()));
    *on.c.travel.lock().unwrap() = Some(Arc::new(Watching(Arc::downgrade(&on.c), held.clone())) as Arc<dyn super::super::travel::Travel>);
    on.c.launcher_do(mine(Action::JoinRunning { mode: LaunchMode::Desktop, destination: private_place("friends(usr_x)") }));
    assert_eq!((held.lock().unwrap().clone(), on.invites_sent()), (vec![false], 1));
    let on = Computer::new(true);
    *on.apple.ctl.lock().unwrap() = Some(Arc::downgrade(&on.c));
    on.at(0, private_place("friends(usr_x)"));
    assert_eq!((on.apple.ready.lock().unwrap().clone(), on.starts().len()), (vec![false], 1), "got before, and started");
}

/// Both computers' own adapters, on a folder as VRChat leaves it: a log that cannot be read is said.
#[cfg(unix)]
#[test]
fn an_unreadable_log_is_said_by_the_adapters() {
    use std::os::unix::fs::PermissionsExt;
    let allow = |path: &std::path::Path, mode: u32| std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    let home = std::env::temp_dir().join(format!("orbiters-adapters-{}", uuid::Uuid::new_v4()));
    let of_mac = orbiters_core::obelisk::vrchat_log_dir(&orbiters_core::obelisk::library(&home));
    let of_pc = home.join("LocalLow/VRChat/VRChat");
    let apple = MacMachine::new(&home);
    let pc = RealMachine { folder: Some(of_pc.clone()), ..RealMachine::default() };
    for (machine, dir) in [(&apple as &dyn Machine, &of_mac), (&pc, &of_pc)] {
        assert_eq!(machine.log_problem(), Some(LogTrouble::NoFolder));
        std::fs::create_dir_all(dir).unwrap();
        let log = dir.join("output_log_2026-11-24_00-00-00.txt");
        std::fs::write(&log, "x").unwrap();
        allow(&log, 0o000);
        assert_eq!(machine.log_problem(), Some(LogTrouble::Denied(log.display().to_string())));
        allow(&log, 0o600);
        assert!(machine.log_problem().is_none());
    }
    std::fs::remove_dir_all(&home).unwrap();
}

// --- The walkthroughs: an ordinary player of one account after an update from before there were profiles, as each computer's
// adapter does it; and a computer of several profiles. ---

/// An old private room of this computer's history: no owner was kept with it, and no log of then is left.
const OLD_ROOM: &str = "48213~friends(usr_x)~region(eu)";

/// What an ordinary player presses, on a PC or a Mac, with the settings and the history that earlier versions left and no VRChat log
/// to read: each goes as it did in 0.3.9-dev.42.
fn an_ordinary_player_s_presses(mac: bool) {
    use crate::controller::vrchat_data::{Request, Response, Scope};
    use crate::pages::launcher::destinations::{self, Go, HistoryView, Inputs};
    let root = std::env::temp_dir().join(format!("orbiters-walkthrough-{}", uuid::Uuid::new_v4()));
    let sessions = root.join("Sessions");
    std::fs::create_dir_all(sessions.join("one")).unwrap();
    // The settings and the history as a version from before profiles wrote them.
    let settings = root.join("settings.json");
    std::fs::write(&settings, r#"{"launch": {"fps": 72}}"#).unwrap();
    let history = root.join("visited-worlds.json");
    let file = format!(r#"{{"worlds":{{"{W}":{{"name":"Sample room","visits":[1794990000]}}}},"logs":{{"output_log_2026-11-23_20-00-00.txt":900}},"last":"{W}","instances":[{{"world_id":"{W}","instance":"{OLD_ROOM}","at":1794990000}}],"instances_read":true}}"#);
    std::fs::write(&history, file).unwrap();
    let fresh = || {
        let on = Computer::new(mac);
        let loaded = Settings::load_from(&settings);
        on.c.update_settings(|s| s.launch = loaded.launch);
        *on.pc.log_problem.lock().unwrap() = Some(LogTrouble::NoFolder);
        *on.apple.log_problem.lock().unwrap() = Some(LogTrouble::NoFolder);
        on.tick();
        assert!(!on.c.view().launcher.several_profiles && on.c.view().launcher.log_trouble.is_none());
        on
    };
    let room = || private_place("friends(usr_x)");
    let link = |place: &Place| orbiters_core::vrchat_launch::link(W, &place.instance_id);
    let launched = Some(Notice::Sent { to: W.into(), action: SentKind::Launch });
    let joined = Some(Notice::Sent { to: W.into(), action: SentKind::Join });
    let in_one_step = |on: &Computer, place: &Place| {
        assert_eq!((on.starts().len(), on.starts()[0].last().cloned()), (1, Some(link(place))), "mac {mac}: the place goes with the start");
        assert!(on.joining().is_none() && on.pending().is_none() && on.calls.lock().unwrap().is_empty());
    };
    // Play.
    let on = fresh();
    assert_eq!(on.c.launcher_do(Action::PlayOn { mode: LaunchMode::Desktop, profile: 0 }), Some(Notice::Sent { to: String::new(), action: SentKind::Launch }));
    assert!(!on.starts()[0].iter().any(|arg| arg.contains("vrchat://") || arg.starts_with("--profile")));
    // Play with a private destination, signed in or not.
    let on = fresh();
    assert_eq!(on.at(0, room()), launched);
    in_one_step(&on, &room());
    let on = fresh();
    assert_eq!(on.c.launcher_do(Action::PlayAt { mode: LaunchMode::Desktop, profile: 0, destination: room() }), launched);
    in_one_step(&on, &room());
    // A friend's Join: nothing runs (a PC's press names no profile, a Mac's the default one).
    let on = fresh();
    let press = if mac { Action::PlayAt { mode: LaunchMode::Desktop, profile: 0, destination: room() } } else { Action::Play { mode: LaunchMode::Desktop, destination: Some(room()) } };
    assert_eq!(on.c.launcher_do(mine(press)), launched);
    in_one_step(&on, &room());
    // It runs: straight into it on a PC, as an invite to oneself on a Mac; the same for an invite accepted.
    for accept in [false, true] {
        let on = fresh();
        on.runs(&[(10, 0, None, false)]);
        let fake = Arc::new(super::super::travel::tests::Fake { running: vec![10], server: Some(10), unknown: vec![10], ..Default::default() });
        *on.c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
        invited_to(&on.c, room());
        let press = if accept { Action::AcceptInviteRunning { invite: "not_1".into(), mode: LaunchMode::Desktop } } else { Action::JoinRunning { mode: LaunchMode::Desktop, destination: room() } };
        assert_eq!(on.c.launcher_do(mine(press)), joined);
        assert_eq!((fake.written.lock().unwrap().clone(), on.invites_sent()), if mac { (Vec::new(), 1) } else { (vec![link(&room())], 0) });
        assert!(on.starts().is_empty());
    }
    // An invite accepted, nothing running.
    let on = fresh();
    invited_to(&on.c, room());
    assert_eq!(on.c.launcher_do(mine(Action::AcceptInviteOn { invite: "not_1".into(), mode: LaunchMode::Desktop, profile: 0 })), launched);
    in_one_step(&on, &room());
    // Where, and its Previous instances: the old private room is listed, asked about, and gone to.
    let kept = orbiters_core::visited::History::load(&history);
    let listed = crate::pages::launcher::previous::visits(&kept);
    assert_eq!(listed.iter().map(|v| v.instance.as_str()).collect::<Vec<_>>(), [OLD_ROOM]);
    let view = HistoryView::of(&kept, 1_794_990_100);
    let cards = destinations::suggestions(&Inputs { now: 1_794_990_100, invites: &[], dismissed: &|_| false, events: &[], history: Some(&view), last_open: Some(true), me: Some("usr_test"), selection: None, offer: None, several: false });
    let card = cards.iter().find(|card| card.world_id == W).expect("its card");
    let Go::Instance(there) = &card.go else { panic!("{:?}", card.go) };
    assert_eq!(there.instance_id, OLD_ROOM);
    let plan = destinations::launch(card, orbiters_core::vrchat_api::CreateAccess::FriendsPlus, true, 0);
    let on = fresh();
    assert_eq!(on.c.launcher_do(mine(destinations::action(&plan, LaunchMode::Desktop, Some(0)).unwrap())), launched);
    in_one_step(&on, there);
    let open = || json!({"worldId": W, "instanceId": OLD_ROOM, "accessType": "friends", "region": "eu", "n_users": 3, "capacity": 20, "closedAt": null});
    let on = fresh();
    let (client, calls) = on.c.test_vrchat(vec![(200, open()), (200, open()), (200, json!({}))]);
    let seen = on.c.vrchat_data(Scope::of(&on.c.view()), Request::InstanceSeen { world: W.into(), instance: OLD_ROOM.into(), max_age_ms: 30_000 }).unwrap();
    assert!(matches!(seen, Response::InstanceSeen { open: Some(true), .. }), "{seen:?}");
    // Invite me (Previous instances of the menus): listed, and from the person's own account when it is connected; Orbiters' own
    // account is `orbiters-n4n0`'s (`invite_me_through_orbiters_goes_to_a_private_room`).
    let rows = orbiters_n4n0_core::invites::rows(&kept);
    let location = format!("{W}:{OLD_ROOM}");
    assert_eq!(rows.iter().map(|row| (row.location.clone(), row.kind())).collect::<Vec<_>>(), [(location.clone(), "Friends")]);
    assert_eq!(super::super::previous_invite::invite_self(&client, "usr_test", &rows, &location), orbiters_n4n0_core::menu::InviteAnswer::Sent);
    assert!(calls.lock().unwrap().last().unwrap().1.ends_with(&format!("/invite/myself/to/{location}")));
    // A capture of then, opened: its room is shown, still open, and may be joined.
    {
        use orbiters_core::contracts::manifest::{SessionManifest, WorldInfo, MANIFEST_FILE, MANIFEST_FORMAT};
        let world = WorldInfo { id: Some(W.into()), name: Some("Sample room".into()), instance: Some(OLD_ROOM.into()), ..Default::default() };
        let manifest = SessionManifest { format: MANIFEST_FORMAT.into(), created_utc: "2026-11-23T20:00:00Z".into(), world: Some(world), ..Default::default() };
        std::fs::write(sessions.join("one").join(MANIFEST_FILE), serde_json::to_vec(&manifest).unwrap()).unwrap();
        let entries = super::super::shares::local_entries_at(&sessions, Default::default(), &history, None);
        let entry = entries.iter().find(|entry| entry.world_id == W).expect("listed");
        assert_eq!(entry.instance.as_deref(), Some(OLD_ROOM));
        let on = fresh();
        let (_, calls) = on.c.test_vrchat(vec![(200, open())]);
        let Ok(Response::Instance(alive)) = on.c.vrchat_data(Scope::of(&on.c.view()), Request::Instance { world: entry.world_id.clone(), instance: entry.instance.clone().unwrap() }) else { panic!("its check") };
        assert_eq!((alive, calls.lock().unwrap().len()), (Some(true), 1));
        assert!(crate::pages::gallery::join_available(false, alive));
    }
    // The assistant's "go to previous instance", with VRChat signed in and without.
    for signed_in in [true, false] {
        let on = fresh();
        if !signed_in {
            on.c.update(|s| s.launcher.source = Source::Nothing);
        }
        let (go, item) = (orbiters_n4n0_core::intents::Intent::GoToInstance { index: 0 }, json!({ "ref": location }));
        let target = on.c.assistant_target(&go, Some(&item)).unwrap();
        let went = on.c.assistant_intent(&go, Some(&item), &target);
        assert!(went.is_ok(), "{went:?}");
        eventually_so(|| !on.starts().is_empty());
        in_one_step(&on, there);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_ordinary_player_on_a_pc_meets_what_dev_42_did_after_an_update() {
    an_ordinary_player_s_presses(false);
}

#[test]
fn an_ordinary_player_on_a_mac_meets_what_dev_42_did_after_an_update() {
    an_ordinary_player_s_presses(true);
}

/// A computer of several profiles: VRChat starts without the private place; signed in there as another account, it is withheld and
/// Home says so; Try again keeps it, through another failure too; once that very VRChat is the account's, it is delivered, once.
#[test]
fn a_private_place_on_a_computer_of_several_profiles_is_delivered_on_a_match_and_kept_otherwise() {
    for mac in [true, false] {
        let on = Computer::new(mac);
        on.remembers(&[0, 1]);
        on.tick();
        assert!(on.c.view().launcher.several_profiles);
        assert!(matches!(on.at(1, private_place("friends(usr_x)")), Some(Notice::Sent { .. })));
        assert!(!on.starts()[0].iter().any(|arg| arg.contains("vrchat://")), "mac {mac}: {:?}", on.starts()[0]);
        let withheld = || {
            on.look();
            assert_eq!(on.joining(), Some(Joining { world: "Sample room".into(), step: JoinStep::Failed(JoinFailure::OtherAccount) }));
            assert_eq!(on.c.launcher.lock().unwrap().kept.as_ref().and_then(|errand| errand.destination.clone()), Some(private_place("friends(usr_x)")), "kept");
            std::thread::sleep(Duration::from_millis(30));
            assert!(on.calls.lock().unwrap().is_empty());
        };
        let fake = Arc::new(super::super::travel::tests::Fake { running: vec![42], server: Some(42), ..Default::default() });
        on.runs(&[(42, 1, Some("usr_other"), true)]);
        *on.c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
        withheld();
        assert!(matches!(on.c.launcher_do(Action::JoinAgain), Some(Notice::Sent { .. })));
        withheld();
        assert!(fake.written.lock().unwrap().is_empty());
        on.runs(&[(42, 1, Some("usr_test"), true)]);
        *on.c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
        assert!(matches!(on.c.launcher_do(Action::JoinAgain), Some(Notice::Sent { .. })));
        on.look();
        eventually_so(|| on.joining().is_none());
        assert_eq!((on.invites_sent(), fake.written.lock().unwrap().len(), on.starts().len()), if mac { (1, 0, 1) } else { (0, 1, 1) }, "once, to that one");
        assert!(on.c.launcher.lock().unwrap().kept.is_none());
    }
}

// --- The fourteenth verification's findings on computers of several profiles. ---

/// A Mac of several profiles, the game that runs positively another account's: Join sends nothing (the invite would reach the
/// app's account and no game), says why, and keeps the place: pressed again it stays so, and goes once that game is the account's.
/// Whose it is not known: the account's own invite, as ever; on a Mac of one profile too, whoever plays.
#[test]
fn a_join_into_a_mac_s_game_of_another_account_sends_nothing_and_is_kept() {
    let join = || mine(Action::JoinRunning { mode: LaunchMode::Desktop, destination: private_place("friends(usr_x)") });
    let joined = Some(Notice::Sent { to: W.into(), action: SentKind::Join });
    for (chips, profile) in [(vec![0, 1], 1), (vec![0, 1], 0), (vec![], 1)] {
        for accept in [false, true] {
            let on = Computer::new(true);
            on.remembers(&chips);
            on.runs(&[(42, profile, Some("usr_other"), true)]);
            invited_to(&on.c, private_place("friends(usr_x)"));
            let press = if accept { Action::AcceptInviteRunning { invite: "not_1".into(), mode: LaunchMode::Desktop } } else { Action::JoinRunning { mode: LaunchMode::Desktop, destination: private_place("friends(usr_x)") } };
            let withheld = |notice: Option<Notice>| {
                assert!(matches!(&notice, Some(Notice::Error(words)) if words.contains("running as another account")), "{notice:?}");
                assert_eq!(on.joining(), Some(Joining { world: "Sample room".into(), step: JoinStep::Failed(JoinFailure::OtherAccount) }));
                assert_eq!(on.c.launcher.lock().unwrap().kept.as_ref().and_then(|errand| errand.destination.clone()), Some(private_place("friends(usr_x)")), "kept");
                assert!(on.calls.lock().unwrap().is_empty() && on.starts().is_empty(), "nothing sent, nothing started");
                assert_eq!(on.c.view().launcher.invites.len(), 1, "its card stays");
            };
            withheld(on.c.launcher_do(mine(press)));
            // Again, still another's (on the default profile that game is waited for, as after a start).
            let again = on.c.launcher_do(Action::JoinAgain);
            if profile == 0 {
                on.look();
                on.c.launcher_settle_accepted(None, Some(&by(&on.c)));
                withheld(Some(Notice::Error("running as another account".into())));
            } else {
                withheld(again);
            }
            on.runs(&[(42, profile, Some("usr_test"), true)]);
            assert!(matches!(on.c.launcher_do(Action::JoinAgain), Some(Notice::Sent { .. })));
            on.look();
            eventually_so(|| on.invites_sent() == 1 && on.joining().is_none());
            assert!(on.c.launcher.lock().unwrap().kept.is_none() && on.starts().is_empty());
        }
    }
    // Not known whose it is.
    let on = Computer::new(true);
    on.remembers(&[0, 1]);
    on.runs(&[(42, 1, None, true)]);
    assert_eq!((on.c.launcher_do(join()), on.invites_sent()), (joined.clone(), 1));
    assert!(on.joining().is_none());
    // One profile: as before there were several.
    let on = Computer::new(true);
    on.runs(&[(42, 0, Some("usr_other"), true)]);
    assert_eq!((on.c.launcher_do(join()), on.invites_sent()), (joined, 1));
    // A place kept after a start that did not prove its account ("Invite me"): that very game says another's by now.
    for why in [JoinFailure::NoProof, JoinFailure::Log(LogTrouble::NotWritten)] {
        let on = Computer::new(true);
        on.remembers(&[2]);
        on.at(2, private_place("friends(usr_x)"));
        let errand = on.c.launcher.lock().unwrap().pending.take().unwrap().errand;
        on.c.launcher_join_failed(errand, why);
        on.runs(&[(42, 2, Some("usr_other"), true)]);
        assert!(matches!(on.c.launcher_do(Action::JoinAgain), Some(Notice::Error(words)) if words.contains("running as another account")));
        assert_eq!((on.invites_sent(), on.step()), (0, Some(JoinStep::Failed(JoinFailure::OtherAccount))));
        assert!(on.c.launcher.lock().unwrap().kept.is_some());
    }
}

/// The verifier's probe: a roster row of the default profile says nothing against a command line that names another: both count,
/// and where they disagree no private link is written into that game, whoever the row says is signed in.
#[test]
fn a_stale_default_row_does_not_hide_another_profile_that_runs() {
    for account in ["usr_other", "usr_test"] {
        let on = Computer::new(false);
        on.runs(&[(10, 0, Some(account), true)]);
        on.pc.profiles.lock().unwrap().insert(10, 1);
        let fake = Arc::new(super::super::travel::tests::Fake { running: vec![10], server: Some(10), ..Default::default() });
        *on.c.travel.lock().unwrap() = Some(fake.clone() as Arc<dyn super::super::travel::Travel>);
        assert!(on.c.launcher_several(on.machine().as_ref(), None), "its command line says so");
        assert_eq!(on.c.launcher_do(mine(Action::JoinRunning { mode: LaunchMode::Desktop, destination: private_place("friends(usr_x)") })), Some(Notice::Sent { to: W.into(), action: SentKind::Join }));
        assert_eq!((fake.written.lock().unwrap().len(), on.invites_sent()), (0, 1), "{account}: the account's own invite, nothing to that game");
    }
    // The other way round, as before; and both say the default one: one profile.
    let on = Computer::new(false);
    on.runs(&[(10, 1, Some("usr_test"), true)]);
    on.pc.profiles.lock().unwrap().insert(10, 0);
    assert!(on.c.launcher_several(on.machine().as_ref(), None));
    let on = Computer::new(false);
    on.runs(&[(10, 0, Some("usr_test"), true)]);
    assert!(!on.c.launcher_several(on.machine().as_ref(), None));
}

/// The verifier's probe: VRChat's answer may take minutes (an upload). A Join asked for meanwhile by a signed-in account's list
/// holds no start while it waits: not the first look, not the last; and Play goes on.
#[test]
fn no_start_waits_for_what_is_asked_of_vrchat() {
    for mac in [true, false] {
        for several in [true, false] {
            let on = Computer::new(mac);
            if several {
                on.remembers(&[0, 1]);
            }
            let client = on.c.vrchat_session().unwrap().client;
            let busy = client.lock().unwrap();
            let (c, (done, answer)) = (on.c.clone(), std::sync::mpsc::channel());
            let press = std::thread::spawn(move || drop(done.send(c.launcher_do(mine(Action::PlayAt { mode: LaunchMode::Desktop, profile: 0, destination: private_place("friends(usr_x)") })))));
            let said = answer.recv_timeout(Duration::from_secs(5));
            let free = on.c.launcher_started.try_lock().is_ok();
            let started = on.starts().len();
            drop(busy);
            press.join().unwrap();
            assert!(free, "mac {mac}: the start is not held");
            assert!(matches!(said, Ok(Some(Notice::Sent { .. }))), "mac {mac}, several {several}: {said:?}");
            assert_eq!(started, 1);
        }
    }
    // Whose session it is is still what counts, at that moment: another's, and none.
    let on = Computer::new(false);
    let client = on.c.vrchat_session().unwrap().client;
    assert!(on.c.launcher_live(Some("usr_test")) && !on.c.launcher_live(Some("usr_other")) && !on.c.launcher_live(None));
    let _ = client.lock().unwrap().sign_out();
    assert!(!on.c.launcher_live(Some("usr_test")), "signed out");
}

/// The verifier's probe: what was said of a log that cannot be read is over with the last game it was said of.
#[test]
fn a_log_that_could_not_be_read_is_said_no_more_once_its_game_is_gone() {
    for mac in [true, false] {
        let on = Computer::new(mac);
        on.remembers(&[0, 2]);
        *on.apple.log_problem.lock().unwrap() = Some(LogTrouble::NotWritten);
        *on.pc.log_problem.lock().unwrap() = Some(LogTrouble::NotWritten);
        let trouble = || on.c.launcher.lock().unwrap().log_trouble.clone();
        let long_ago = Instant::now().checked_sub(LOG_WITHIN + Duration::from_secs(1)).unwrap();
        let unreadable = |pid: u32| {
            on.runs(&[(pid, 2, None, false)]);
            on.look();
            on.c.launcher.lock().unwrap().unlogged.insert(pid, long_ago);
            on.look();
            assert_eq!(trouble(), Some(LogTrouble::NotWritten), "mac {mac}");
        };
        unreadable(42);
        on.runs(&[]);
        on.look();
        assert_eq!(trouble(), None, "mac {mac}: it ended");
        // Another came up meanwhile: it has its time to make one.
        unreadable(43);
        on.runs(&[(44, 2, None, false)]);
        on.look();
        assert_eq!(trouble(), None, "mac {mac}: it is another's");
        unreadable(45);
        on.tick();
        on.runs(&[]);
        on.tick();
        assert!(!mac || on.c.view().launcher.log_trouble.is_none(), "Where goes by it");
    }
}

/// The verifier's probe: a restart after a lost picture keeps its place whatever became of the profiles since its start: started
/// in two steps, made again straight into it; and nothing is left waiting for it.
#[test]
fn a_restart_keeps_its_place_when_the_profiles_change() {
    let on = Computer::new(true);
    on.remembers(&[0, 1]);
    on.at(0, private_place("friends(usr_x)"));
    on.runs(&[(42, 0, None, true)]);
    on.tick();
    let args = on.starts()[0].clone();
    assert!(!args.iter().any(|arg| arg.starts_with("vrchat://")) && on.pending().is_some());
    *on.apple.lost.lock().unwrap() = Some("s".into());
    *on.apple.restarted.lock().unwrap() = Some(HomeLaunch { args: args.clone(), account: Some("usr_test".into()), destination: Some(private_place("friends(usr_x)")), ..Default::default() });
    let restart = || {
        let before = on.apple.restarts.lock().unwrap().len();
        assert_eq!(on.c.launcher_do(Action::RestartLostGame { session: "s".into() }), None);
        eventually_so(|| on.apple.restarts.lock().unwrap().len() > before);
        on.apple.restarted_with.lock().unwrap().last().cloned().expect("made again")
    };
    let link = orbiters_core::vrchat_launch::link(W, "7~friends(usr_x)~region(eu)");
    // Another profile is there still: in two steps again.
    assert_eq!(restart(), args);
    eventually_so(|| on.pending().is_some());
    // The unused chip goes.
    assert_eq!(on.c.launcher_do(Action::ForgetProfile { profile: 1 }), None);
    let direct = [args.clone(), vec![link.clone()]].concat();
    assert_eq!(restart(), direct, "straight into it");
    eventually_so(|| on.pending().is_none() && on.joining().is_none());
    // And the opposite: one made straight into it, restarted once there are several, keeps its place for the next restart too.
    *on.apple.restarted.lock().unwrap() = Some(HomeLaunch { args: direct, account: Some("usr_test".into()), destination: Some(private_place("friends(usr_x)")), ..Default::default() });
    on.remembers(&[1]);
    assert_eq!(restart(), args);
    eventually_so(|| on.pending().is_some_and(|p| p.errand.destination == Some(private_place("friends(usr_x)"))));
}

/// What a Mac keeps of its own start: the place it was asked for, whether it rode with it or not, through every restart.
#[test]
#[cfg(target_os = "macos")]
fn a_mac_s_start_keeps_the_place_it_was_asked_for() {
    use super::super::picture;
    let _publishing = picture::PUBLISHING.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::env::temp_dir().join(format!("orbiters-restart-place-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let machine = pretend_program(&root);
    let session = |n: u32| format!("5e551011-0000-4000-8000-00000000000{n}");
    let link = orbiters_core::vrchat_launch::link(W, &place().instance_id);
    let kept = |n: u32| {
        eventually(|| machine.home_session_running(&session(n)));
        machine.home_launch_of(&session(n)).unwrap().destination
    };
    machine.start_for(&["-x".to_string(), link.clone()], &errand_of("usr_a")).unwrap();
    assert_eq!(kept(1), Some(place()));
    // Made again without it (there are several profiles by now), then with it.
    for (n, args) in [(1, vec!["-x".to_string()]), (2, vec!["-x".to_string(), link])] {
        picture::publish(Some((session(n), picture::Picture::Lost)));
        machine.restart_lost(&session(n), &|_| Ok(args.clone())).unwrap();
        assert_eq!(kept(n + 1), Some(place()));
    }
    picture::publish(None);
    std::fs::write(root.join(format!("stop-{}", session(3))), "").unwrap();
    eventually(|| machine.play.attached().is_none());
    let _ = std::fs::remove_dir_all(&root);
}

/// A chip is taken off the choice only where no start on its profile was just decided: the two wait for each other; and a Mac's
/// start counts from the engine's own word, before Home shows it.
#[test]
fn a_profile_whose_start_was_just_decided_stays_on_the_choice() {
    let on = Computer::new(false);
    on.remembers(&[0, 1]);
    on.tick();
    let deciding = on.c.launcher_started.lock().unwrap();
    let c = on.c.clone();
    let press = std::thread::spawn(move || c.launcher_do(Action::ForgetProfile { profile: 1 }));
    std::thread::sleep(Duration::from_millis(50));
    assert!(on.c.view().settings.launch.profiles.contains_key(&1), "a start is being decided");
    on.c.launcher_claim(1, LaunchMode::Desktop).unwrap();
    drop(deciding);
    assert!(matches!(press.join().unwrap(), Some(Notice::Error(_))));
    assert!(on.c.view().settings.launch.profiles.contains_key(&1));
    let on = Computer::new(true);
    on.remembers(&[0, 1]);
    on.tick();
    on.apple.starts.lock().unwrap().push(vec!["--profile=1".into()]);
    on.apple.still_starting.store(true, Ordering::Relaxed);
    assert!(matches!(on.c.launcher_do(Action::ForgetProfile { profile: 1 }), Some(Notice::Error(_))), "the engine starts it");
    assert!(on.c.view().settings.launch.profiles.contains_key(&1));
    on.apple.still_starting.store(false, Ordering::Relaxed);
    assert_eq!(on.c.launcher_do(Action::ForgetProfile { profile: 1 }), None);
}
