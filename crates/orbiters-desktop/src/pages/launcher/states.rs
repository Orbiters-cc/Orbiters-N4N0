//! What the request-invite and boop buttons say, and what the Play buttons can do, from the contract's state, what
//! this page itself just sent, and the clock. Pure, so the states are tested without drawing.

use orbiters_core::launcher::{Friend, Game, LaunchMode, LauncherState, Notice, Presence, SentKind, Source, Whereabouts};

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
        return Reach::SignIn;
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
        return Reach::SignIn;
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
    /// Nothing can start it here (a Mac, or the service has not said yet).
    Unavailable,
}

pub fn play(state: &LauncherState) -> PlayState {
    match state.game {
        Game::NotInstalled => PlayState::NotInstalled,
        Game::Starting(mode) => PlayState::Starting(mode),
        Game::Running(mode) => PlayState::Running(mode),
        Game::NotRunning if state.possible.play => PlayState::Ready,
        Game::NotRunning => PlayState::Unavailable,
    }
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
        s.game = Game::NotRunning;
        s.possible.play = false;
        assert_eq!(play(&s), PlayState::Unavailable);
    }
}
