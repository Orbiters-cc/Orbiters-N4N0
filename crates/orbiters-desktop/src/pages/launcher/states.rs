//! What the request-invite and boop buttons say, and what the Play buttons can do, from the contract's state, what
//! this page itself just sent, and the clock. Pure, so the states are tested without drawing.

use orbiters_core::launcher::{Friend, Game, GameProfile, LaunchMode, LauncherState, Notice, Presence, SentKind, Source, Whereabouts};
use orbiters_core::vrchat_launch::SEVERAL_HERE;

/// How long "Sent" shows after a press before the cool-down (or the plain button) is back.
pub const SENT_FOR: f64 = 2.5;
/// How long a boop's own animation plays.
pub const BOOP_FOR: f64 = 1.1;

/// A button that reaches a person (request an invite, boop).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reach {
    /// Pressable.
    Ready,
    /// Just sent: a short confirmation.
    Sent,
    /// Asked too recently: allowed again in this many seconds.
    Cooling(i64),
    /// Not for this person now (an invite from someone not in the game, in a private instance).
    NotNow,
    /// The account is on its way back (the list last on show, its session being restored): drawn where it will be, not
    /// pressable (REQUEST28: nothing jumps when VRChat answers).
    Waiting,
    /// Needs the VRChat sign-in (database only): the page says it once, the button is not drawn.
    SignIn,
}

impl Reach {
    pub fn pressable(self) -> bool {
        self == Reach::Ready
    }
}

fn cooling(ready_at: Option<i64>, now: i64) -> Option<i64> {
    ready_at.filter(|&at| at > now).map(|at| at - now)
}

/// The request-invite button for `friend`.
pub fn request(state: &LauncherState, friend: &Friend, sent_at: Option<f64>, clock: f64, now: i64) -> Reach {
    if !state.possible.request_invite {
        // (Only who can be asked will have it.)
        let there = friend.presence == Presence::InGame && !matches!(friend.whereabouts, Whereabouts::Website | Whereabouts::Offline);
        return if state.kept.is_none() { Reach::SignIn } else if there { Reach::Waiting } else { Reach::NotNow };
    }
    if sent_at.is_some_and(|at| clock - at < SENT_FOR) || notice_sent(state, friend, SentKind::RequestInvite) {
        return Reach::Sent;
    }
    if let Some(left) = cooling(friend.request_ready_at, now).or_else(|| notice_cooling(state, friend, now)) {
        return Reach::Cooling(left);
    }
    // Only someone in the game can invite; a private instance still can (that is what a request is for).
    match (&friend.presence, &friend.whereabouts) {
        (Presence::InGame, Whereabouts::World(_) | Whereabouts::Private | Whereabouts::Traveling | Whereabouts::Unknown) => Reach::Ready,
        _ => Reach::NotNow,
    }
}

/// The boop button for `friend`: anyone, online or not, while signed in.
pub fn boop(state: &LauncherState, friend: &Friend, sent_at: Option<f64>, clock: f64, now: i64) -> Reach {
    if !state.possible.boop {
        return if state.kept.is_some() { Reach::Waiting } else { Reach::SignIn };
    }
    if sent_at.is_some_and(|at| clock - at < SENT_FOR) {
        return Reach::Sent;
    }
    match cooling(friend.boop_ready_at, now) {
        Some(left) => Reach::Cooling(left),
        None => Reach::Ready,
    }
}

fn notice_sent(state: &LauncherState, friend: &Friend, kind: SentKind) -> bool {
    matches!(&state.notice, Some(Notice::Sent { to, action }) if *to == friend.id && *action == kind)
}

fn notice_cooling(state: &LauncherState, friend: &Friend, now: i64) -> Option<i64> {
    match &state.notice {
        Some(Notice::CoolingDown { to, until }) if *to == friend.id && *until > now => Some(until - now),
        _ => None,
    }
}

/// The Play buttons.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlayState {
    /// Both pressable.
    Ready,
    /// This mode is starting.
    Starting(LaunchMode),
    /// VRChat runs in this mode: the buttons give way to a line that says so.
    Running(LaunchMode),
    /// VRChat is not on this computer.
    NotInstalled,
    /// A Mac without Obelisk, which runs VRChat there.
    NoEngine,
    /// Nothing can start it here (it runs on another device, or the service has not said yet).
    Unavailable,
    /// Another profile's VRChat is starting: one start at a time.
    OtherStarting,
    /// Where one VRChat runs at a time (a Mac): it runs on another profile.
    OtherRunning,
}

pub fn play(state: &LauncherState) -> PlayState {
    match state.game {
        Game::NotInstalled => PlayState::NotInstalled,
        Game::NoEngine => PlayState::NoEngine,
        Game::Starting(mode) => PlayState::Starting(mode),
        Game::Running(mode) => PlayState::Running(mode),
        Game::NotRunning if state.possible.play => PlayState::Ready,
        Game::NotRunning => PlayState::Unavailable,
    }
}

/// The profiles Home offers to start on: the service's list (profile 0 always, then those used before, running or
/// starting), and a new one the person added here.
pub fn profiles(state: &LauncherState, added: Option<u32>) -> Vec<GameProfile> {
    let mut out = state.profiles.clone();
    if out.is_empty() {
        out.push(GameProfile::default());
    }
    if let Some(n) = added.filter(|n| !out.iter().any(|p| p.profile == *n)) {
        out.push(GameProfile { profile: n, ..GameProfile::default() });
        out.sort_by_key(|p| p.profile);
    }
    out
}

/// The profile the Play buttons start next: the one the person chose, else the first one used before on which no
/// VRChat runs or starts (so a second VRChat is one press away for someone with several accounts), else profile 0.
/// Someone who only ever used profile 0 keeps it: with VRChat running, the buttons say so instead of offering a new
/// profile nobody asked for.
pub fn next_profile(offered: &[GameProfile], chosen: Option<u32>) -> u32 {
    if let Some(c) = chosen.filter(|c| offered.iter().any(|p| p.profile == *c)) {
        return c;
    }
    offered.iter().find(|p| p.running.is_none() && p.starting.is_none()).or(offered.first()).map_or(0, |p| p.profile)
}

/// A new profile's number: the first one past those offered.
pub fn new_profile(offered: &[GameProfile]) -> Option<u32> {
    // The first number Home offers that is not offered yet (a remembered 15 leaves 1 to 14 free).
    (1..=orbiters_core::vrchat_launch::PROFILE_MAX).find(|n| !offered.iter().any(|p| p.profile == *n))
}

/// The Play buttons for a start on `profile`.
pub fn play_on(state: &LauncherState, offered: &[GameProfile], profile: u32) -> PlayState {
    match state.game {
        Game::NotInstalled => return PlayState::NotInstalled,
        Game::NoEngine => return PlayState::NoEngine,
        _ => {}
    }
    if !state.possible.play {
        return PlayState::Unavailable;
    }
    let this = offered.iter().find(|p| p.profile == profile);
    if let Some(mode) = this.and_then(|p| p.running) {
        return PlayState::Running(mode);
    }
    if let Some(mode) = this.and_then(|p| p.starting) {
        return PlayState::Starting(mode);
    }
    if offered.iter().any(|p| p.starting.is_some()) {
        return PlayState::OtherStarting;
    }
    // The first VRChat, started a moment ago and not yet tied to a profile.
    if let Game::Starting(mode) = state.game {
        if !offered.iter().any(|p| p.running.is_some()) {
            return if profile == 0 { PlayState::Starting(mode) } else { PlayState::OtherStarting };
        }
    }
    PlayState::Ready
}

/// [`next_profile`], where one VRChat runs at a time (a Mac): the one the person chose, else the one VRChat runs or starts
/// on (no other can be started beside it), else the first.
pub fn next_profile_of_one(offered: &[GameProfile], chosen: Option<u32>) -> u32 {
    if let Some(c) = chosen.filter(|c| offered.iter().any(|p| p.profile == *c)) {
        return c;
    }
    offered.iter().find(|p| p.running.is_some() || p.starting.is_some()).or(offered.first()).map_or(0, |p| p.profile)
}

/// [`play_on`], where one VRChat runs at a time: while it runs or starts, only its own profile says so, and no other one
/// starts (nor, while its profile is not known, any).
pub fn play_on_one(state: &LauncherState, offered: &[GameProfile], profile: u32) -> PlayState {
    let this = offered.iter().find(|p| p.profile == profile);
    match play(state) {
        PlayState::Running(_) => this.and_then(|p| p.running).map_or(PlayState::OtherRunning, PlayState::Running),
        PlayState::Starting(mode) => match this.and_then(|p| p.starting) {
            Some(mode) => PlayState::Starting(mode),
            None if offered.iter().all(|p| p.starting.is_none()) && profile == 0 => PlayState::Starting(mode),
            None => PlayState::OtherStarting,
        },
        other => other,
    }
}

/// [`next_profile`] on this computer.
pub fn next_here(offered: &[GameProfile], chosen: Option<u32>) -> u32 {
    if SEVERAL_HERE { next_profile(offered, chosen) } else { next_profile_of_one(offered, chosen) }
}

/// [`play_on`] on this computer.
pub fn play_here(state: &LauncherState, offered: &[GameProfile], profile: u32) -> PlayState {
    if SEVERAL_HERE { play_on(state, offered, profile) } else { play_on_one(state, offered, profile) }
}

/// Whether the list shows what is live or a copy, for the one line that says what needs the sign-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Live {
    SignedIn,
    /// A VRCX / VRCNext copy: no live status, invites or boops.
    Copy,
    Nothing,
}

pub fn live(state: &LauncherState) -> Live {
    match state.source {
        Source::SignedIn { .. } => Live::SignedIn,
        Source::Database { .. } => Live::Copy,
        Source::Nothing => Live::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_core::launcher::{sample, DatabaseApp, Possible};

    const NOW: i64 = 1_795_000_000;

    fn friend<'a>(state: &'a LauncherState, name: &str) -> &'a Friend {
        state.friends.iter().find(|f| f.name == name).unwrap()
    }

    #[test]
    fn a_request_is_ready_for_someone_in_game_and_not_for_someone_offline() {
        let s = sample();
        assert_eq!(request(&s, friend(&s, "Nova Sample"), None, 0.0, NOW), Reach::Ready);
        assert_eq!(request(&s, friend(&s, "Pixel Fox"), None, 0.0, NOW), Reach::Ready, "a private instance can be asked for");
        assert_eq!(request(&s, friend(&s, "Lumen"), None, 0.0, NOW), Reach::NotNow);
        assert_eq!(request(&s, friend(&s, "Quiet Kite"), None, 0.0, NOW), Reach::NotNow);
    }

    #[test]
    fn what_reaches_a_person_waits_in_its_place_while_the_account_is_on_its_way_back() {
        let mut s = sample();
        (s.kept, s.possible) = (Some(1), Possible { live_status: true, ..Possible::default() });
        assert_eq!(request(&s, friend(&s, "Nova Sample"), None, 0.0, NOW), Reach::Waiting);
        assert_eq!(request(&s, friend(&s, "Quiet Kite"), None, 0.0, NOW), Reach::NotNow);
        assert_eq!(boop(&s, friend(&s, "Quiet Kite"), None, 0.0, NOW), Reach::Waiting);
        assert!(!Reach::Waiting.pressable());
    }

    #[test]
    fn a_press_shows_sent_at_once_then_the_cool_down() {
        let mut s = sample();
        let id = s.friends[0].id.clone();
        assert_eq!(request(&s, &s.friends[0], Some(10.0), 11.0, NOW), Reach::Sent);
        // The service's answer: cooling for 50 s more.
        s.friends[0].request_ready_at = Some(NOW + 50);
        assert_eq!(request(&s, &s.friends[0], Some(10.0), 10.0 + SENT_FOR + 0.1, NOW), Reach::Cooling(50));
        s.friends[0].request_ready_at = None;
        s.notice = Some(Notice::CoolingDown { to: id, until: NOW + 7 });
        assert_eq!(request(&s, &s.friends[0], None, 0.0, NOW), Reach::Cooling(7));
        assert_eq!(request(&s, &s.friends[0], None, 0.0, NOW + 8), Reach::Ready);
    }

    #[test]
    fn a_boop_cools_down_per_person() {
        let s = sample();
        // Orbit Tester was booped 20 s ago in the sample: 40 s left.
        assert_eq!(boop(&s, friend(&s, "Orbit Tester"), None, 0.0, NOW), Reach::Cooling(40));
        assert_eq!(boop(&s, friend(&s, "Quiet Kite"), None, 0.0, NOW), Reach::Ready, "offline people can be booped");
        assert_eq!(boop(&s, friend(&s, "Quiet Kite"), Some(5.0), 6.0, NOW), Reach::Sent);
        assert!(!Reach::Sent.pressable() && !Reach::Cooling(3).pressable() && Reach::Ready.pressable());
    }

    #[test]
    fn a_database_copy_asks_for_the_sign_in_instead_of_dead_buttons() {
        let mut s = sample();
        s.source = Source::Database { app: DatabaseApp::Vrcx, read_at: NOW - 60 };
        s.possible = Possible { play: true, ..Default::default() };
        assert_eq!(live(&s), Live::Copy);
        assert_eq!(request(&s, &s.friends[0], None, 0.0, NOW), Reach::SignIn);
        assert_eq!(boop(&s, &s.friends[0], None, 0.0, NOW), Reach::SignIn);
        assert_eq!(play(&s), PlayState::Ready, "playing needs no sign-in");
    }

    fn on(profile: u32, running: Option<LaunchMode>, starting: Option<LaunchMode>) -> GameProfile {
        GameProfile { profile, running, starting, ..GameProfile::default() }
    }

    #[test]
    fn the_next_start_goes_to_a_free_profile_and_never_onto_a_running_one() {
        // Someone who only ever used profile 0, VRChat running: the buttons stay on 0 and say it runs.
        let only = [on(0, Some(LaunchMode::Vr), None)];
        assert_eq!(next_profile(&only, None), 0);
        let mut s = sample();
        s.profiles = only.to_vec();
        assert_eq!(play_on(&s, &only, 0), PlayState::Running(LaunchMode::Vr), "the Play buttons stay, saying it runs");
        // Several accounts: the first free one is offered next.
        let several = [on(0, Some(LaunchMode::Vr), None), on(1, None, None), on(2, None, None)];
        assert_eq!(next_profile(&several, None), 1);
        assert_eq!(next_profile(&several, Some(2)), 2, "the person's choice");
        assert_eq!(next_profile(&several, Some(9)), 1, "a choice no longer offered is not kept");
        s.profiles = several.to_vec();
        assert_eq!(play_on(&s, &several, 1), PlayState::Ready, "a second VRChat, on profile 1");
        assert_eq!(play_on(&s, &several, 0), PlayState::Running(LaunchMode::Vr));
        // A start under way: that profile says so, the others wait their turn.
        let starting = [on(0, Some(LaunchMode::Vr), None), on(1, None, Some(LaunchMode::Desktop)), on(2, None, None)];
        assert_eq!(play_on(&s, &starting, 1), PlayState::Starting(LaunchMode::Desktop));
        assert_eq!(play_on(&s, &starting, 2), PlayState::OtherStarting);
        assert_eq!(next_profile(&starting, None), 2);
    }

    /// A Mac: one VRChat at a time, whatever profile it is on; no other one is startable while it runs or starts.
    #[test]
    fn where_one_vrchat_runs_at_a_time_no_other_profile_starts_beside_it() {
        const DESK: LaunchMode = LaunchMode::Desktop;
        let mut s = sample();
        let free = [on(0, None, None), on(1, None, None), on(2, None, None)];
        s.profiles = free.to_vec();
        assert_eq!(next_profile_of_one(&free, None), 0, "the default, as always");
        assert_eq!(next_profile_of_one(&free, Some(2)), 2);
        assert_eq!(next_profile_of_one(&free, Some(9)), 0, "a choice no longer offered is not kept");
        assert!((0..3).all(|n| play_on_one(&s, &free, n) == PlayState::Ready));
        // It runs on 1: that one says so, and is the one shown to someone who chose none.
        s.game = Game::Running(DESK);
        let runs = [on(0, None, None), on(1, Some(DESK), None), on(2, None, None)];
        assert_eq!(next_profile_of_one(&runs, None), 1);
        assert_eq!(next_profile_of_one(&runs, Some(2)), 2, "the person's choice, for once it was quit");
        assert_eq!(play_on_one(&s, &runs, 1), PlayState::Running(DESK));
        assert_eq!((play_on_one(&s, &runs, 0), play_on_one(&s, &runs, 2)), (PlayState::OtherRunning, PlayState::OtherRunning));
        // It runs and its profile is not known (yet): none says it is its own, none starts.
        assert!((0..3).all(|n| play_on_one(&s, &free, n) == PlayState::OtherRunning));
        // It starts on 2.
        s.game = Game::Starting(DESK);
        let starts = [on(0, None, None), on(1, None, None), on(2, None, Some(DESK))];
        assert_eq!(next_profile_of_one(&starts, None), 2);
        assert_eq!(play_on_one(&s, &starts, 2), PlayState::Starting(DESK));
        assert_eq!((play_on_one(&s, &starts, 0), play_on_one(&s, &starts, 1)), (PlayState::OtherStarting, PlayState::OtherStarting));
        assert_eq!((play_on_one(&s, &free, 0), play_on_one(&s, &free, 1)), (PlayState::Starting(DESK), PlayState::OtherStarting));
        // The profile list never makes a computer ready that is not.
        for (game, state) in [(Game::NotInstalled, PlayState::NotInstalled), (Game::NoEngine, PlayState::NoEngine)] {
            s.game = game;
            assert!((0..3).all(|n| play_on_one(&s, &free, n) == state));
        }
        s.game = Game::NotRunning;
        s.possible.play = false;
        assert_eq!(play_on_one(&s, &free, 1), PlayState::Unavailable);
    }

    #[test]
    fn a_new_profile_comes_after_the_ones_offered() {
        let mut s = sample();
        s.profiles.clear();
        let offered = profiles(&s, None);
        assert_eq!(offered.iter().map(|p| p.profile).collect::<Vec<_>>(), [0], "profile 0 always");
        assert_eq!(new_profile(&offered), Some(1));
        let offered = profiles(&s, Some(1));
        assert_eq!(offered.iter().map(|p| p.profile).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(new_profile(&offered), Some(2));
        let last = [on(0, None, None), on(orbiters_core::vrchat_launch::PROFILE_MAX, None, None)];
        assert_eq!(new_profile(&last), Some(1), "a remembered 15 leaves 1 free");
        let all: Vec<GameProfile> = (0..=orbiters_core::vrchat_launch::PROFILE_MAX).map(|n| on(n, None, None)).collect();
        assert_eq!(new_profile(&all), None, "no more than the profiles Home offers");
        let gap = [on(0, None, None), on(1, None, None), on(3, None, None)];
        assert_eq!(new_profile(&gap), Some(2));
        s.game = Game::NotInstalled;
        assert_eq!(play_on(&s, &offered, 0), PlayState::NotInstalled);
    }

    #[test]
    fn the_play_buttons_follow_the_game() {
        let mut s = sample();
        assert_eq!(play(&s), PlayState::Ready);
        s.game = Game::Starting(LaunchMode::Desktop);
        assert_eq!(play(&s), PlayState::Starting(LaunchMode::Desktop));
        s.game = Game::Running(LaunchMode::Vr);
        assert_eq!(play(&s), PlayState::Running(LaunchMode::Vr));
        s.game = Game::NotInstalled;
        assert_eq!(play(&s), PlayState::NotInstalled);
        s.game = Game::NoEngine;
        assert_eq!(play(&s), PlayState::NoEngine);
        s.game = Game::NotRunning;
        s.possible.play = false;
        assert_eq!(play(&s), PlayState::Unavailable);
    }
}
