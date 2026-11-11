//! Walk boost: while VRChat walks the avatar with the stick, every tracked device is carried the same way, so the avatar
//! covers more ground than VRChat's own walking.
//!
//! There are two ways to carry them. **Through the driver**, when Orbiters - N4N0's SteamVR driver runs (the same one as
//! tracking smoothing, [`SmoothLink`]): each device's pose gets the walk added in SteamVR's raw tracking space, and the play
//! space is never touched. Whatever else moves the play space (OVR Advanced Settings' Space Drag and Turn, a recenter)
//! simply adds up with the walk. The overlay writes where the walk is and how fast it goes on, in continuous time, so each
//! device gets it whenever its pose comes. The first few metres are checked: every device must really have followed
//! ([`Proof`]), and if one did not, the boost goes back to the other way, and says so.
//!
//! **Through the play space**, the lesser way, without the driver (SteamVR has not loaded it, or runs an older one): the
//! working copy of SteamVR's chaperone setup is previewed live, like OVR Advanced Settings' space drag, never saved. The
//! chaperone's walls move the other way, so they stay on the room's walls. It is the lesser way as SteamVR shows one
//! application's play space at a time. The rest of this comment is about it.
//!
//! The boost waits while the stick is not walking: N4N0's menu or the calibration stands in the play space, a setting
//! of the boost was just changed (the Walk boost radial of VRChat's Action Menu turns with the same stick), VRChat says
//! the owner is away, and, when VRChat says how it moves the avatar, while it sits in a station or does not walk it.
//!
//! Like OVR Advanced Settings' space drag, the boost keeps the play space it started from (the base) itself and only
//! ever writes the base moved by the carry, then shows it: it never reverts the working copy, hides its preview or
//! reads SteamVR's setup back while its carry shows. SteamVR raises play-space events for each preview (and other
//! applications' connections raise more); reverting or hiding to look at them put the play space back where it
//! started, and a carry rebased onto a setup that had it already threw the person by it. So only what a person does
//! ends a carry, at once, the play space going back to SteamVR's own setup (their new one): a recenter, a seated reset
//! (not one just after the boost's own preview, which may be its echo), a room setup, another tracking universe. The
//! play space also goes back when VRChat loads another world (the avatar respawns anyway), when VRChat closes and when
//! the overlay ends, however it ends; a process killed outside leaves nothing behind either, as SteamVR goes back to
//! the saved chaperone once the process previewing one is gone.
//!
//! SteamVR's working copy belongs to each application, and the preview that shows is whichever application showed one
//! last. OVR Advanced Settings shows its own at every step of its Space Drag and Space Turn: the play space as it knows
//! it, with none of the boost's carry in it. So its show puts the person back where the walk began, at once, and the
//! boost's next step showed the old base plus the whole carry, throwing them forward again and undoing what that drag
//! did (Enzo, 2026-11-07, "random positions resets and it starts to move me all over the place": his Advanced Settings
//! drags with Y, B and the right thumbrest, and its log has writes within 2 s of the end of 4 of his 5 walks that day). So the boost reads
//! the play space everyone sees each frame while it carries ([`PlaySpace::live_standing`]; reading changes nothing). What
//! it did not show itself is another application's: the carry is over, without touching the play space, and the boost
//! holds until that application has been quiet a moment ([`OTHER_APP_QUIET`]). A walk always starts from the play space
//! as it is then. The person is put back once, by that application, and never thrown; nothing here can keep the distance
//! walked through such a show, as the other application writes a whole play space that lacks it.
//!
//! `walk-boost.log` in the logs folder says when a walk starts and stops and why, and what another application did.

use super::actions::Walk;
use orbiters_core::settings::{Follow, Hand, WalkBoostSettings};
use orbiters_smooth::{WalkLink, WalkNow};
use orbiters_openvr::chaperone::{ChaperoneSetup, Quad};
use super::floating::space::{compose, inverse};
use orbiters_openvr::{self as vr, Matrix34, Pose, Universe, MAX_DEVICES};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// VRChat's running speed (m/s), reached with the stick pushed all the way.
pub const RUN_SPEED: f32 = 4.0;
/// Stick travel ignored around the centre.
const DEADZONE: f32 = 0.12;
/// A frame longer than this moves as far as this (no jump after a stall).
const MAX_STEP_S: f32 = 0.05;
/// After a setting of the boost changes, the stick is taken for a menu's this long: the Walk boost radial in
/// VRChat's Action Menu turns with it, and a toggle there is chosen with it.
const MENU_QUIET: Duration = Duration::from_millis(1500);
/// Room setup lets go of the play space until it is saved, this long at most (it can be left without saving).
const ROOM_SETUP: Duration = Duration::from_secs(180);
/// VRChat moves an avatar it walks at least this fast (m/s), before the boost adds anything.
const VRCHAT_WALKS: f32 = 0.4;
/// Explicit near-zero raw speed indicates the avatar is standing still.
const VRCHAT_STILL: f32 = 0.15;
/// VRChat's own walking this long below [`VRCHAT_STILL`] stops the carry (a change of push shows late, briefly).
const STILL_FOR: Duration = Duration::from_millis(300);
/// Setups this close (metres, and rotation terms) are the same one: SteamVR may hand its numbers back a little off.
#[cfg(test)]
const SAME_SETUP: f32 = 1e-3;
/// SteamVR raises play-space events for the previews the boost shows: a seated reset this soon after one may be its
/// echo (the boost moves the seated zero too), never a reason to end the carry.
const OWN_EVENTS: Duration = Duration::from_millis(1000);
/// Two readings of where a universe sits this close (metres, and rotation terms) are the same place. Another
/// application's show further off than this is a jump a person sees; one closer changes nothing they would notice.
const SAME_PLACE: f32 = 0.02;
const SAME_TURN: f32 = 0.01;
/// What the boost showed may be what every application sees this late.
const SHOWN_LATE: Duration = Duration::from_secs(1);
/// After another application moved the play space, the boost waits until it has not for this long: both showing in
/// turn would be the person flickering between the two places.
const OTHER_APP_QUIET: Duration = Duration::from_millis(400);
/// The reading of the play space is trusted once it was seen following the boost's own shows, within this long of
/// walking. A reading that does not follow them says nothing, and the boost goes without it (see [`Reading`]).
const READING_TRIAL: Duration = Duration::from_secs(2);
/// The walk boost's page says for this long that another application moved the play space.
const TAKEN_SAID_FOR: Duration = Duration::from_secs(30);
/// How fast the walk through the driver gathers or loses speed (m/s²): from standing to the boost's most in 0.125 s.
/// SteamVR predicts each frame ahead with the walk's speed, 30 to 40 ms when VRChat runs at 45 frames a second: at once from 0
/// to 8 m/s, or back, that is a lurch of 30 cm in one frame.
const WALK_ACCELERATION: f32 = 64.0;
/// A walk this far through the driver shows whether every device followed.
const PROOF_METRES: f32 = 3.0;
/// A device further than this from where the walk should have put it did not follow: a hand reaches about that far while
/// the walk goes on, and a device left behind or carried at 20° off is further.
const PROOF_MISS_METRES: f32 = 1.0;
/// Readings further apart than this start over: the person may have moved about too much for them to tell anything.
const PROOF_WITHIN: Duration = Duration::from_secs(20);

/// Whether two matrices place a universe at the same spot, the same way round.
fn same_place(a: &Matrix34, b: &Matrix34) -> bool {
    (0..3).all(|r| (a.m[r][3] - b.m[r][3]).abs() <= SAME_PLACE && (0..3).all(|c| (a.m[r][c] - b.m[r][c]).abs() <= SAME_TURN))
}

/// How far apart two matrices place a universe, in metres.
fn distance(a: &Matrix34, b: &Matrix34) -> f32 {
    (0..3).map(|r| (a.m[r][3] - b.m[r][3]).powi(2)).sum::<f32>().sqrt()
}

/// How far the stick is pushed (0 to 1, the dead zone removed; 0 for a reading that is not a number).
pub fn push(stick: [f32; 2]) -> f32 {
    if !(stick[0].is_finite() && stick[1].is_finite()) {
        return 0.0;
    }
    let len = (stick[0] * stick[0] + stick[1] * stick[1]).sqrt();
    ((len - DEADZONE) / (1.0 - DEADZONE)).clamp(0.0, 1.0)
}

/// The horizontal direction the stick points in the standing universe, scaled by [`push`]: forward is where `frame`
/// (the head or the walking hand) looks (its −Z), right its +X.
pub fn direction(stick: [f32; 2], frame: &Matrix34) -> [f32; 3] {
    let amount = push(stick);
    if amount == 0.0 {
        return [0.0; 3];
    }
    let flat = |v: [f32; 3]| {
        let len = (v[0] * v[0] + v[2] * v[2]).sqrt();
        // Looking straight up or down, or a pose that is not a number: no way forward.
        if len >= 1e-4 {
            Some([v[0] / len, 0.0, v[2] / len])
        } else {
            None
        }
    };
    let m = &frame.m;
    let Some(forward) = flat([-m[0][2], -m[1][2], -m[2][2]]) else { return [0.0; 3] };
    // Right is forward turned a quarter clockwise seen from above (the frame's own X may tilt with a rolled hand).
    let right = [-forward[2], 0.0, forward[0]];
    let len = (stick[0] * stick[0] + stick[1] * stick[1]).sqrt().max(1e-6);
    let (x, y) = (stick[0] / len, stick[1] / len);
    [(right[0] * x + forward[0] * y) * amount, 0.0, (right[2] * x + forward[2] * y) * amount]
}

/// A play space: where the standing and seated universes sit in the raw tracking space, and the chaperone's walls
/// (in the standing universe).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Setup {
    pub standing: Matrix34,
    pub seated: Option<Matrix34>,
    pub bounds: Vec<Quad>,
}

impl Setup {
    /// This setup with its standing universe where it is `live`: the seated universe moves with it, and the walls stay
    /// where they are in the room. What another application's preview adds (its Space Drag) so stays under the walk.
    fn at_live(&self, live: &Matrix34) -> Setup {
        if same_place(&self.standing, live) {
            return self.clone();
        }
        let moved = compose(live, &inverse(&self.standing));
        let to_new_standing = compose(&inverse(live), &self.standing);
        let corner = |c: [f32; 3]| [0, 1, 2].map(|r| to_new_standing.m[r][0] * c[0] + to_new_standing.m[r][1] * c[1] + to_new_standing.m[r][2] * c[2] + to_new_standing.m[r][3]);
        Setup {
            standing: *live,
            seated: self.seated.as_ref().map(|s| compose(&moved, s)),
            bounds: self.bounds.iter().map(|q| Quad { corners: q.corners.map(corner) }).collect(),
        }
    }

    /// The same play space, to a millimetre (the tests' measure).
    #[cfg(test)]
    pub fn close_to(&self, other: &Setup) -> bool {
        let near = |a: &[f32], b: &[f32]| a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= SAME_SETUP);
        let matrix = |a: &Matrix34, b: &Matrix34| near(a.m.as_flattened(), b.m.as_flattened());
        let walls = |q: &[Quad]| q.iter().flat_map(|q| q.corners.as_flattened().iter().copied()).collect::<Vec<f32>>();
        matrix(&self.standing, &other.standing)
            && match (&self.seated, &other.seated) {
                (Some(a), Some(b)) => matrix(a, b),
                (a, b) => a.is_none() && b.is_none(),
            }
            && near(&walls(&self.bounds), &walls(&other.bounds))
    }
}

/// The play space as it was before walking, and how far the person was carried since (in the standing universe).
#[derive(Clone, Debug, PartialEq)]
pub struct Carried {
    pub base: Setup,
    pub offset: [f32; 3],
    /// The base is another application's preview, not SteamVR's own setup: putting it back shows it, as hiding would
    /// take that application's offset away too.
    pub over_other: bool,
}

impl Carried {
    /// The play space that carries the person `offset` metres: the universes' origins move by −offset in the room
    /// (so every tracked position reads +offset), and the walls' corners by +offset in the standing universe (so they
    /// stay where they are in the room).
    pub fn shifted(&self) -> Setup {
        let r = &self.base.standing.m;
        let o = self.offset;
        // The offset in raw tracking space: the standing universe's rotation applied to it.
        let raw = [
            r[0][0] * o[0] + r[0][1] * o[1] + r[0][2] * o[2],
            r[1][0] * o[0] + r[1][1] * o[1] + r[1][2] * o[2],
            r[2][0] * o[0] + r[2][1] * o[1] + r[2][2] * o[2],
        ];
        let moved = |m: &Matrix34| {
            let mut out = *m;
            for (row, d) in out.m.iter_mut().zip(raw) {
                row[3] -= d;
            }
            out
        };
        let bounds = self.base.bounds.iter().map(|q| Quad { corners: q.corners.map(|c| [c[0] + o[0], c[1] + o[1], c[2] + o[2]]) }).collect();
        Setup { standing: moved(&self.base.standing), seated: self.base.seated.as_ref().map(moved), bounds }
    }
}

/// SteamVR's chaperone setup as walk boost uses it (a stand-in in tests). While a preview shows, it is what SteamVR
/// calls the live setup: every application sees it, and the working copy reverts to it rather than to the saved one.
pub trait PlaySpace {
    /// The working copy back to the live setup: the preview showing, or else the saved setup.
    fn revert(&mut self);
    /// The working copy.
    fn read(&mut self) -> Option<Setup>;
    fn write(&mut self, setup: &Setup);
    /// The working copy shown to every application: a preview, never saved.
    fn show(&mut self);
    /// The preview hidden: the saved setup is the live one again.
    fn hide(&mut self);
    /// Where the standing universe sits in the raw tracking space for every application right now, whoever's preview
    /// that is (`None` while the headset is not tracked). It only reads, and leaves the working copy alone.
    fn live_standing(&mut self) -> Option<Matrix34>;
    /// Each tracked device (its index, the headset's being 0) and where it is in the standing universe.
    fn devices(&mut self) -> Vec<(u32, [f32; 3])> {
        Vec::new()
    }
}

impl PlaySpace for ChaperoneSetup<'_> {
    fn revert(&mut self) {
        self.revert_working_copy();
    }

    fn read(&mut self) -> Option<Setup> {
        let standing = self.working_standing_zero()?;
        Some(Setup { standing, seated: self.working_seated_zero(), bounds: self.working_collision_bounds().unwrap_or_default() })
    }

    fn write(&mut self, setup: &Setup) {
        self.set_working_standing_zero(&setup.standing);
        if let Some(seated) = &setup.seated {
            self.set_working_seated_zero(seated);
        }
        if !setup.bounds.is_empty() {
            self.set_working_collision_bounds(&setup.bounds);
        }
    }

    fn show(&mut self) {
        self.show_working_set_preview();
    }

    fn hide(&mut self) {
        self.hide_working_set_preview();
    }

    fn live_standing(&mut self) -> Option<Matrix34> {
        // The headset in both spaces: what separates them is the play space every application is given. Its poses are
        // the very ones VRChat reads, so a preview that moves it is seen here, whoever showed it.
        let client = self.client();
        let (mut standing, mut raw) = ([Pose::default(); MAX_DEVICES], [Pose::default(); MAX_DEVICES]);
        client.poses(Universe::Standing, &mut standing);
        client.poses(Universe::Raw, &mut raw);
        (standing[0].pose_is_valid && raw[0].pose_is_valid).then(|| compose(&raw[0].device_to_absolute, &inverse(&standing[0].device_to_absolute)))
    }

    fn devices(&mut self) -> Vec<(u32, [f32; 3])> {
        let client = self.client();
        let mut poses = [Pose::default(); MAX_DEVICES];
        client.poses(Universe::Standing, &mut poses);
        let mut out = Vec::new();
        for (i, pose) in poses.iter().enumerate() {
            // What is fixed in the room (a base station) follows too, but nothing rides on it.
            let moves = matches!(client.device_class(i as u32), vr::DeviceClass::Hmd | vr::DeviceClass::Controller | vr::DeviceClass::GenericTracker);
            if moves && pose.pose_is_valid && pose.device_is_connected {
                let m = &pose.device_to_absolute.m;
                out.push((i as u32, [m[0][3], m[1][3], m[2][3]]));
            }
        }
        out
    }
}

/// SteamVR's driver as walk boost uses it (a stand-in in tests).
pub trait DriverLink {
    /// Whether the driver would carry a walk now.
    fn state(&mut self) -> DriverState;
    /// Fast walk may use it: it hooks SteamVR's poses for that.
    fn want(&mut self, on: bool);
    /// Where the walk is, and how fast it goes on.
    fn send(&mut self, walk: &WalkNow);
    /// Poses it moved so far, for the log.
    fn walked(&mut self) -> u32 {
        0
    }
}

/// What the driver can do for the walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriverState {
    /// It runs, its hooks are in place.
    Ready,
    /// It runs and was just asked: its hooks come on its next frame.
    Hooking,
    /// It runs, in a version from before the walk could use it: SteamVR loads a driver when it starts.
    Old,
    /// SteamVR did not load it, or the service has not set it up yet.
    NotRunning,
}

/// How the walk goes now, for the windows and menus: the driver, or the play space and why.
pub use crate::state::{WalkPath, WhyPlaySpace};

/// The real driver, through the control block the tray service keeps. Tried again every second until it opens.
#[derive(Default)]
pub struct SmoothLink {
    link: Option<WalkLink>,
    tried: Option<Instant>,
    wanted: bool,
}

impl SmoothLink {
    fn open(&mut self) -> Option<&WalkLink> {
        if self.link.is_none() && self.tried.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            self.tried = Some(Instant::now());
            self.link = WalkLink::open().ok();
            if let Some(link) = &self.link {
                link.set_wanted(self.wanted);
            }
        }
        self.link.as_ref()
    }
}

impl DriverLink for SmoothLink {
    fn state(&mut self) -> DriverState {
        match self.open().map(WalkLink::driver) {
            Some(Some(status)) if !status.walk => DriverState::Old,
            Some(Some(status)) if status.hooks == 0 => DriverState::Hooking,
            Some(Some(_)) => DriverState::Ready,
            _ => DriverState::NotRunning,
        }
    }

    fn want(&mut self, on: bool) {
        if self.wanted != on {
            self.wanted = on;
            if let Some(link) = &self.link {
                link.set_wanted(on);
            }
        }
    }

    fn send(&mut self, walk: &WalkNow) {
        if let Some(link) = &self.link {
            link.send(walk);
        }
    }

    fn walked(&mut self) -> u32 {
        self.link.as_ref().and_then(WalkLink::driver).map_or(0, |status| status.walked)
    }
}

/// The walk through the driver.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Through {
    /// Where it put everyone, in the raw tracking space.
    offset: [f32; 3],
    /// How fast it goes, in the standing universe.
    velocity: [f32; 3],
    /// The same in the raw tracking space, as last written.
    velocity_raw: [f32; 3],
}

/// Checks that the walk through the driver really moves everything: the headset, each controller, each tracker. Hooks of other
/// drivers sit in the same place (Space Calibrator's turns what comes through it, for the devices it moves), so this can only
/// be seen by looking.
#[derive(Clone, Debug)]
struct Proof {
    since: Instant,
    /// The walk then, in the raw tracking space, and where the standing universe sat.
    offset: [f32; 3],
    standing: Matrix34,
    /// Each device then, in the standing universe.
    devices: Vec<(u32, [f32; 3])>,
}

/// The rotation of `m` applied to `v`.
fn turned(m: &Matrix34, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|r| m.m[r][0] * v[0] + m.m[r][1] * v[1] + m.m[r][2] * v[2])
}

/// The inverse of that.
fn turned_back(m: &Matrix34, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|c| m.m[0][c] * v[0] + m.m[1][c] * v[1] + m.m[2][c] * v[2])
}

/// What VRChat says of the avatar over OSC (its `InStation`, `Seated` and `VelocityMagnitude` parameters).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    /// In a station (a chair, a vehicle), seated or not: VRChat does not walk the avatar.
    pub in_station: bool,
    /// Raw VRChat velocity magnitude (m/s). Its room-motion contribution is not established.
    pub speed: f32,
    pub speed_at_ms: Option<u64>,
}

/// Conservative motion interlock, not a measurement of native velocity. OSC gives a
/// total magnitude, with no independent avatar position. A residual at or below our
/// injected speed can mean a wall/freeze OR valid native-only telemetry. Stop once
/// in either case; only a real stick release rearms, never a timer or a positive sample.
#[derive(Clone, Copy, Debug, Default)]
pub struct Gate {
    since: Option<Instant>,
    still_since: Option<Instant>,
    station: bool,
    stopped: bool,
    logged: Option<Instant>,
}
impl Gate {
    pub fn carries(&mut self, now: Instant, pushed: bool, motion: Option<&Motion>, boost: f32) -> bool {
        // A positive station exit is required even across missing OSC and stick release.
        if let Some(m) = motion { self.station = m.in_station; }
        if !pushed {
            self.since = None;
            self.still_since = None;
            self.stopped = false;
            return false;
        }
        if self.station || self.stopped {
            self.since = None;
            return false;
        }
        let uncertain = match motion {
            None => true,
            Some(m) => !m.speed.is_finite() || m.speed < VRCHAT_STILL
                || (boost > VRCHAT_STILL && m.speed <= boost + VRCHAT_STILL),
        };
        if uncertain {
            if now.saturating_duration_since(*self.still_since.get_or_insert(now)) >= STILL_FOR {
                self.stopped = true;
                self.since = None;
                return false;
            }
        } else {
            self.still_since = None;
        }
        let carrying = self.since.is_some() || motion.is_none_or(|m| m.speed >= VRCHAT_WALKS);
        if carrying { self.since.get_or_insert(now); }
        carrying
    }
}

/// SteamVR's play-space events, as walk boost takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaySpaceEvent {
    /// `VREvent_StandingZeroPoseReset`: the person recentred (SteamVR saved a new standing zero).
    StandingReset,
    /// `VREvent_SeatedZeroPoseReset`.
    SeatedReset,
    /// `VREvent_ChaperoneUniverseHasChanged`, with the tracking universe before and after (the same one for the
    /// boost's own previews).
    UniverseChanged { from: u64, to: u64 },
    /// `VREvent_ChaperoneFlushCache`: cached chaperone data to read again (raised for previews too).
    FlushCache,
    /// `VREvent_ChaperoneRoomSetupStarting`.
    RoomSetupStarting,
    /// `VREvent_ChaperoneRoomSetupCommitted`: room setup, or any application, saved a setup.
    SetupCommitted,
}

impl PlaySpaceEvent {
    /// The event SteamVR raised, if it is one of these.
    pub fn from_event(e: &vr::Event) -> Option<Self> {
        Some(match e.event_type {
            vr::event::STANDING_ZERO_POSE_RESET => Self::StandingReset,
            vr::event::SEATED_ZERO_POSE_RESET => Self::SeatedReset,
            vr::event::CHAPERONE_UNIVERSE_CHANGED => {
                let (from, to) = e.chaperone_universes();
                Self::UniverseChanged { from, to }
            }
            vr::event::CHAPERONE_FLUSH_CACHE => Self::FlushCache,
            vr::event::ROOM_SETUP_STARTING => Self::RoomSetupStarting,
            vr::event::ROOM_SETUP_COMMITTED => Self::SetupCommitted,
            _ => return None,
        })
    }
}

/// What the overlay loop knows of VRChat for walking.
pub struct Moment<'a> {
    pub settings: &'a WalkBoostSettings,
    /// VRChat runs, is in a world and has the scene (not a loading screen, not the dashboard).
    pub walking_allowed: bool,
    /// The world and instance: another one puts the play space back.
    pub place: Option<&'a str>,
    /// Something stands in the play space where it was put (N4N0's menu, the calibration's mirror): moving the play
    /// space would carry the person away from it.
    pub held: bool,
    /// VRChat says the owner is away.
    pub away: bool,
    /// How VRChat says the avatar moves, while it talks to Orbiters.
    pub motion: Option<Motion>,
    /// SteamVR's controllers were read this frame: a stick read after a failed read is the last one's.
    pub input_read: bool,
}

/// Whether [`PlaySpace::live_standing`] follows what the boost shows. It should (it is what applications are given), but
/// a headset's driver may place its raw space otherwise, and that cannot be checked without one: until seen following,
/// the boost does not act on the reading, and if it never does, it goes on without it, as it did before there was one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reading {
    /// Since when walking far enough to tell, if yet.
    Unproven(Option<Instant>),
    Follows,
    Unrelated,
}

/// `walk-boost.log` in the logs folder: what the boost did, to read after a headset session (tests keep it in memory).
#[derive(Default)]
struct Log {
    #[cfg(test)]
    said: Vec<String>,
}

impl Log {
    #[cfg(test)]
    fn say(&mut self, what: String) {
        self.said.push(what);
    }

    /// One line, with the time as SteamVR's own logs give it. The boost says little: a walk's start and end at most.
    #[cfg(not(test))]
    fn say(&mut self, what: String) {
        use std::io::Write;
        let dir = orbiters_core::paths::logs_dir();
        let path = dir.join("walk-boost.log");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
            let _ = std::fs::rename(&path, dir.join("walk-boost.previous.log"));
        }
        let t = time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(&path) {
            let _ = writeln!(file, "{} {:02}:{:02}:{:02}.{:03} {what}", t.date(), t.hour(), t.minute(), t.second(), t.millisecond());
        }
    }
}

fn length(v: &[f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

pub struct WalkBoost {
    actions: Option<Walk>,
    carried: Option<Carried>,
    place: Option<String>,
    last: Option<Instant>,
    /// The manifest could not be loaded: said once.
    gate: Gate,
    /// How fast the boost dragged the play space on the last frame (m/s; 0 when it did not).
    boost: f32,
    /// The play space moved on the last frame.
    moving: bool,
    /// The boost's settings last seen (on, strength), and when they last changed.
    seen: Option<(bool, f32)>,
    changed: Option<Instant>,
    /// Room setup started then, and was not saved yet.
    room_setup: Option<Instant>,
    /// When the boost last showed its preview (a seated reset that soon after may be its own).
    shown_at: Option<Instant>,
    /// The headset's frame, when known: steps are whole frames of it, so that the loop's timing noise does not
    /// make some frames move further than others.
    frame: Option<Duration>,
    /// What the boost showed lately (and the base, first), to tell it from another application's show.
    shown: VecDeque<(Instant, Matrix34)>,
    /// A preview of the boost's may still show: a recenter hides it, even when the carry already went.
    showing: bool,
    reading: Reading,
    /// Another application moved the play space: where to last, and the boost holds until then at least.
    other_app: Option<(Matrix34, Instant)>,
    /// When it last did so while the boost carried.
    taken_at: Option<Instant>,
    /// Its preview is the one that shows, as far as the boost saw: reverting may not tell, as SteamVR may hand over either
    /// the preview or its own setup.
    other_showing: bool,
    /// Since when the play space moves in this run of frames, for the log.
    run: Option<Instant>,
    log: Log,
    /// The way through SteamVR's driver, when there is one here.
    driver: Option<Box<dyn DriverLink>>,
    /// The walk through it, while it has carried anyone.
    through: Option<Through>,
    /// Where the standing universe last sat: the walk turns with it.
    standing: Option<Matrix34>,
    /// The driver did not move every device: not asked again until the overlay starts over.
    refused: bool,
    proof: Option<Proof>,
    /// The first walk through it was checked, and every device followed.
    proven: bool,
    /// How the walk goes now, and what was last said of it.
    path: WalkPath,
    path_said: Option<WalkPath>,
}

impl WalkBoost {
    /// Walk boost with the overlay's actions (none when SteamVR's input could not load them).
    pub fn new(actions: Option<Walk>) -> Self {
        Self {
            actions,
            carried: None,
            place: None,
            last: None,
            gate: Gate::default(),
            boost: 0.0,
            moving: false,
            seen: None,
            changed: None,
            room_setup: None,
            shown_at: None,
            frame: None,
            shown: VecDeque::new(),
            showing: false,
            reading: Reading::Unproven(None),
            other_app: None,
            taken_at: None,
            other_showing: false,
            run: None,
            log: Log::default(),
            driver: None,
            through: None,
            standing: None,
            refused: false,
            proof: None,
            proven: false,
            path: WalkPath::PlaySpace(WhyPlaySpace::NotRunning),
            path_said: None,
        }
    }

    /// The way through SteamVR's driver.
    pub fn set_driver(&mut self, driver: Box<dyn DriverLink>) {
        self.driver = Some(driver);
    }

    /// How the walk goes now, when that changed since this was last asked.
    pub fn path_changed(&mut self) -> Option<WalkPath> {
        (self.path_said != Some(self.path)).then(|| {
            self.path_said = Some(self.path);
            self.path
        })
    }

    /// How the walk goes now.
    fn choose_path(&mut self, settings: &WalkBoostSettings) -> WalkPath {
        let wanted = settings.enabled && settings.through_driver && !self.refused;
        let Some(driver) = self.driver.as_mut() else { return WalkPath::PlaySpace(WhyPlaySpace::NotRunning) };
        driver.want(wanted);
        if !settings.through_driver {
            return WalkPath::PlaySpace(WhyPlaySpace::Setting);
        }
        if self.refused {
            return WalkPath::PlaySpace(WhyPlaySpace::Refused);
        }
        match driver.state() {
            DriverState::Ready => WalkPath::Driver,
            DriverState::Old => WalkPath::PlaySpace(WhyPlaySpace::OldDriver),
            // The hooks come at its next frame: only while it is off is there nothing to wait for.
            DriverState::Hooking if wanted => WalkPath::Driver,
            DriverState::Hooking | DriverState::NotRunning => WalkPath::PlaySpace(WhyPlaySpace::NotRunning),
        }
    }

    /// Nobody carried through the driver any more.
    fn end_through(&mut self, why: &str) {
        if let Some(through) = self.through.take() {
            let walked = length(&through.offset);
            self.log.say(format!("through the driver: back by {walked:.2} m: {why}")); // i18n: not shown (the walk's log)
            if let Some(driver) = self.driver.as_mut() {
                driver.send(&WalkNow::default());
            }
        }
        self.proof = None;
        self.boost = 0.0;
        self.moving = false;
    }

    /// The headset's frame (from its refresh rate), which steps are counted in.
    pub fn set_frame(&mut self, frame: Option<Duration>) {
        self.frame = frame.filter(|f| !f.is_zero() && f.as_secs_f32() <= MAX_STEP_S);
    }

    /// The play space moved on the last frame: the overlay loop then runs once per frame the compositor shows.
    pub fn moving(&self) -> bool {
        self.moving
    }

    /// Another application moved the play space lately, and the distance walked went with it: its page says so.
    pub fn taken_lately(&self, now: Instant) -> bool {
        self.taken_at.is_some_and(|t| now.saturating_duration_since(t) < TAKEN_SAID_FOR)
    }

    /// What holds the boost back, if anything, in words for the log.
    fn held_back(&self, m: &Moment, now: Instant) -> Option<&'static str> {
        let since = |t: Instant| now.saturating_duration_since(t);
        if self.actions.is_none() {
            Some("no controls") // i18n: not shown (the walk's log)
        } else if !m.settings.enabled || m.settings.strength <= 0.0 {
            Some("turned off") // i18n: not shown (the walk's log)
        } else if !m.walking_allowed {
            Some("VRChat's scene not in front (dashboard, loading)") // i18n: not shown (the walk's log)
        } else if m.held {
            Some("a menu or the calibration stands in the play space") // i18n: not shown (the walk's log)
        } else if m.away {
            Some("away")
        } else if self.changed.is_some_and(|t| since(t) < MENU_QUIET) {
            Some("a setting just changed") // i18n: not shown (the walk's log)
        } else if self.room_setup.is_some_and(|t| since(t) < ROOM_SETUP) {
            Some("room setup") // i18n: not shown (the walk's log)
        } else if self.other_app.is_some() {
            Some("another application moves the play space") // i18n: not shown (the walk's log)
        } else {
            None
        }
    }

    /// Reads the play space everyone sees: whether it still is what the boost showed. When it is not, another
    /// application showed its own: the carry is over and nothing here touches the play space, which is that
    /// application's now. While it goes on moving it, the boost keeps waiting.
    fn watch_others(&mut self, play: &mut dyn PlaySpace, now: Instant) {
        if self.reading == Reading::Unrelated || (self.carried.is_none() && self.other_app.is_none()) {
            return;
        }
        let Some(live) = play.live_standing() else { return };
        if let Some((place, until)) = self.other_app {
            if !same_place(&place, &live) {
                self.other_app = Some((live, now + OTHER_APP_QUIET));
            } else if now >= until {
                self.other_app = None;
            }
            return;
        }
        let Some(carried) = &self.carried else { return };
        let keep = self.shown.len().saturating_sub(1);
        let late = self.shown.iter().take(keep).take_while(|(at, _)| now.saturating_duration_since(*at) > SHOWN_LATE).count();
        self.shown.drain(..late);
        let ours = self.shown.iter().any(|(_, m)| same_place(m, &live));
        let carried_far = length(&carried.offset) > SAME_PLACE * 10.0;
        if let Reading::Unproven(since) = self.reading {
            if ours && carried_far && !same_place(&carried.base.standing, &live) {
                self.reading = Reading::Follows;
            } else if carried_far {
                let since = since.unwrap_or(now);
                self.reading = if now.saturating_duration_since(since) < READING_TRIAL {
                    Reading::Unproven(Some(since))
                } else {
                    self.log.say(format!("the play space read from the headset's poses does not follow the walk's shows: another application's will not be noticed (read {:?})", live.m)); // i18n: not shown (the walk's log)
                    Reading::Unrelated
                };
            }
            return;
        }
        if !ours {
            let (carried, back) = (length(&carried.offset), distance(&carried.base.standing, &live));
            self.log.say(format!("another application showed its own play space: {carried:.2} m walked are gone, it is {back:.2} m from where the walk began")); // i18n: not shown (the walk's log)
            self.carried = None;
            self.shown.clear();
            self.taken_at = Some(now);
            self.other_showing = true;
            self.other_app = Some((live, now + OTHER_APP_QUIET));
        }
    }

    /// One frame: reads the stick and carries the person further.
    pub fn update(&mut self, steamvr: &vr::Client, m: &Moment) {
        let Ok(mut setup) = steamvr.chaperone_setup() else { return };
        let actions = self.actions;
        // The overlay loop read every action set this frame (actions::Actions::update).
        let stick = || {
            let walk = actions?;
            let action = if m.settings.hand == Hand::Left { walk.left } else { walk.right };
            let stick = steamvr.input().ok()?.analog(action).ok()?;
            stick.active.then_some([stick.x, stick.y])
        };
        let frame = || {
            let mut poses = [Pose::default(); MAX_DEVICES];
            steamvr.poses(Universe::Standing, &mut poses);
            let device = match m.settings.follow {
                Follow::Head => 0,
                Follow::Hand => {
                    let role = if m.settings.hand == Hand::Left { vr::ControllerRole::LeftHand } else { vr::ControllerRole::RightHand };
                    steamvr.controller_index(role).unwrap_or(0)
                }
            };
            let pose = poses[device as usize];
            pose.pose_is_valid.then_some(pose.device_to_absolute)
        };
        self.step(&mut setup, m, stick, frame, Instant::now());
    }

    /// One frame at `now`, reading what SteamVR says only when needed: the stick (`None`: not active) and where
    /// forward points (the head or the walking hand, `None` while not tracked).
    pub fn step(
        &mut self,
        play: &mut dyn PlaySpace,
        m: &Moment,
        stick: impl FnOnce() -> Option<[f32; 2]>,
        frame: impl FnOnce() -> Option<Matrix34>,
        now: Instant,
    ) {
        let mut dt = self.last.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
        if let (Some(frame), true) = (self.frame, dt > 0.0) {
            // Whole frames: the loop waits for each of the compositor's frames while walking, give or take a little.
            let f = frame.as_secs_f32();
            dt = (dt / f).round().max(1.0) * f;
        }
        let real_dt = self.last.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32()).min(MAX_STEP_S);
        let dt = dt.min(MAX_STEP_S);
        self.last = Some(now);
        self.moving = false;
        if self.place.as_deref() != m.place {
            // Another world or instance (or VRChat closed): the play space goes back while the avatar respawns.
            self.place = m.place.map(str::to_string);
            self.say_put_back("another world, or VRChat closed"); // i18n: not shown (the walk's log)
            self.put_back(play);
            self.end_through("another world, or VRChat closed"); // i18n: not shown (the walk's log)
        }
        let path = self.choose_path(m.settings);
        if path != self.path {
            self.path = path;
            self.log.say(format!("the walk goes {path:?}")); // i18n: not shown (the walk's log)
        }
        // One way at a time: what the other one carried goes back as this one takes over.
        if path == WalkPath::Driver {
            if self.carried.is_some() {
                self.say_put_back("now through the driver"); // i18n: not shown (the walk's log)
                self.put_back(play);
            }
        } else if self.through.is_some() {
            self.end_through("now through the play space"); // i18n: not shown (the walk's log)
        }
        let seen = (m.settings.enabled, m.settings.strength);
        if self.seen.is_some_and(|s| s != seen) {
            self.changed = Some(now);
        }
        self.seen = Some(seen);
        let advanced = if path == WalkPath::Driver {
            self.advance_through(play, m, stick, frame, real_dt, now)
        } else {
            self.watch_others(play, now);
            self.advance(play, m, stick, frame, dt, now)
        };
        match advanced {
            Ok(()) => {
                if self.run.is_none() {
                    self.run = Some(now);
                    let carried = self.through.as_ref().map_or_else(|| self.carried.as_ref().map_or(0.0, |c| length(&c.offset)), |t| length(&t.offset));
                    self.log.say(format!("walking: {carried:.2} m from where the carry began")); // i18n: not shown (the walk's log)
                }
            }
            Err(why) => {
                if let Some(start) = self.run.take() {
                    let carried = self.through.as_ref().map_or_else(|| self.carried.as_ref().map_or(0.0, |c| length(&c.offset)), |t| length(&t.offset));
                    let lasted = now.saturating_duration_since(start).as_secs_f32();
                    self.log.say(format!("stopped after {lasted:.2} s, {carried:.2} m from where the carry began: {why}")); // i18n: not shown (the walk's log)
                }
            }
        }
    }

    /// Where the stick wants to go now in the standing universe, at what speed, or why it does not. Asked once a frame:
    /// VRChat's gate counts its calls.
    fn wanted(
        &mut self,
        m: &Moment,
        stick: impl FnOnce() -> Option<[f32; 2]>,
        frame: impl FnOnce() -> Option<Matrix34>,
        now: Instant,
    ) -> Result<([f32; 3], f32), &'static str> {
        let held = self.held_back(m, now);
        let stick = if held.is_none() && m.input_read { stick() } else { None };
        let pushed = stick.is_some_and(|s| push(s) > 0.0);
        let was_carrying = self.gate.since.is_some();
        let boost = std::mem::take(&mut self.boost);
        let released = stick.is_some_and(|s| s.iter().all(|v| v.is_finite()) && push(s) == 0.0);
        let carries = self.gate.carries(now, pushed || (self.gate.stopped && !released), m.motion.as_ref(), boost);
        if was_carrying != carries || (pushed && self.gate.logged.is_none_or(|at| now.saturating_duration_since(at) >= Duration::from_secs(1))) {
            self.gate.logged = Some(now);
            let age = m.motion.and_then(|m| m.speed_at_ms).map(|at| {
                (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64).saturating_sub(at)
            });
            self.log.say(format!("motion gate={carries} stick={stick:?} raw={:?} age_ms={age:?} held={held:?} input={} boost={}", m.motion, m.input_read, boost)); // i18n: not shown (diagnostic log)
        }
        if let Some(why) = held {
            return Err(why);
        }
        if !m.input_read {
            return Err("the controllers could not be read"); // i18n: not shown (the walk's log)
        }
        let Some(stick) = stick else { return Err("the stick is not active") }; // i18n: not shown (the walk's log)
        if !pushed {
            return Err("the stick was let go"); // i18n: not shown (the walk's log)
        }
        if !carries {
            return Err("VRChat motion is stopped or uncertain; release the stick to try again"); // i18n: not shown (the walk's log)
        }
        let Some(frame) = frame() else { return Err("forward is not tracked") }; // i18n: not shown (the walk's log)
        let dir = direction(stick, &frame);
        if dir == [0.0; 3] {
            return Err("no way forward"); // i18n: not shown (the walk's log)
        }
        Ok((dir, RUN_SPEED * m.settings.strength.clamp(0.0, 1.0) * WalkBoostSettings::MAX_EXTRA))
    }

    /// A step further through the driver, or why not. The walk's speed follows the stick within [`WALK_ACCELERATION`], and what
    /// it covered at the speed last written is what the driver has been adding since, so the two agree at every moment:
    /// the driver's own coasting stops at [`MAX_STEP_S`] as this does, after a stall too.
    fn advance_through(
        &mut self,
        play: &mut dyn PlaySpace,
        m: &Moment,
        stick: impl FnOnce() -> Option<[f32; 2]>,
        frame: impl FnOnce() -> Option<Matrix34>,
        dt: f32,
        now: Instant,
    ) -> Result<(), &'static str> {
        let wanted = self.wanted(m, stick, frame, now);
        if let Some(live) = play.live_standing() {
            self.standing = Some(live);
        }
        let standing = self.standing.unwrap_or(Matrix34::IDENTITY);
        let goal = wanted.map_or([0.0; 3], |(dir, speed)| dir.map(|d| d * speed));
        let idle = self.through.is_none_or(|t| t.velocity == [0.0; 3]);
        if idle && goal == [0.0; 3] {
            return wanted.map(drop);
        }
        let mut through = self.through.unwrap_or_default();
        let before = through;
        for (o, v) in through.offset.iter_mut().zip(through.velocity_raw) {
            *o += v * dt;
        }
        let change = [0, 1, 2].map(|i| goal[i] - through.velocity[i]);
        let most = WALK_ACCELERATION * dt;
        let scale = if length(&change) > most { most / length(&change) } else { 1.0 };
        for (v, c) in through.velocity.iter_mut().zip(change) {
            *v += c * scale;
        }
        through.velocity_raw = turned(&standing, through.velocity);
        if through != before {
            if let Some(driver) = self.driver.as_mut() {
                driver.send(&WalkNow { offset: through.offset, velocity: through.velocity_raw });
            }
        }
        self.through = Some(through);
        self.boost = length(&through.velocity);
        self.moving = self.boost > 0.0;
        self.check_proof(play, standing, now);
        wanted.map(drop)
    }

    /// Whether every device went where the walk should have put it: see [`Proof`]. What another application does to the play
    /// space does not count against it, and starts the check over.
    fn check_proof(&mut self, play: &mut dyn PlaySpace, standing: Matrix34, now: Instant) {
        if self.proven {
            return;
        }
        let Some(through) = self.through else { return };
        let stale = self.proof.as_ref().is_some_and(|p| now.saturating_duration_since(p.since) > PROOF_WITHIN || !same_place(&p.standing, &standing));
        if stale {
            self.proof = None;
        }
        let Some(proof) = &self.proof else {
            self.proof = Some(Proof { since: now, offset: through.offset, standing, devices: play.devices() });
            return;
        };
        let moved = [0, 1, 2].map(|i| through.offset[i] - proof.offset[i]);
        if length(&moved) < PROOF_METRES {
            return;
        }
        let expected = turned_back(&standing, moved);
        let devices = play.devices();
        let mut lines = Vec::new();
        let mut behind = false;
        for (device, then) in &proof.devices {
            let Some((_, now_at)) = devices.iter().find(|(d, _)| d == device) else { continue };
            let went = [0, 1, 2].map(|i| now_at[i] - then[i]);
            let missed = length(&[went[0] - expected[0], went[1] - expected[1], went[2] - expected[2]]);
            lines.push(format!("device {device} {:.2} m, missed by {missed:.2}", length(&went))); // i18n: not shown (the walk's log)
            behind |= missed > PROOF_MISS_METRES;
        }
        if lines.is_empty() {
            self.proof = None;
            return;
        }
        let poses = self.driver.as_mut().map_or(0, |d| d.walked());
        let asked = length(&moved);
        if behind {
            self.log.say(format!("through the driver: NOT every device followed {asked:.2} m ({}), {poses} poses moved: the walk goes back to the play space", lines.join("; "))); // i18n: not shown (the walk's log)
            self.refused = true;
        } else {
            self.log.say(format!("through the driver: every device followed {asked:.2} m ({}), {poses} poses moved", lines.join("; "))); // i18n: not shown (the walk's log)
            self.proven = true;
        }
        self.proof = None;
    }

    /// A step further through the play space, or why not.
    fn advance(
        &mut self,
        play: &mut dyn PlaySpace,
        m: &Moment,
        stick: impl FnOnce() -> Option<[f32; 2]>,
        frame: impl FnOnce() -> Option<Matrix34>,
        dt: f32,
        now: Instant,
    ) -> Result<(), &'static str> {
        let (dir, speed) = self.wanted(m, stick, frame, now)?;
        if dt <= 0.0 {
            return Err("the first frame"); // i18n: not shown (the walk's log)
        }
        let step = dir.map(|d| d * speed * dt);
        if !step.iter().all(|s| s.is_finite()) {
            return Err("a step that is not a number"); // i18n: not shown (the walk's log)
        }
        if self.carried.is_none() {
            // No preview of the boost's shows now: the working copy reverts to SteamVR's own setup or to another
            // application's preview. The walk starts from what everyone sees, whichever that is.
            play.revert();
            let Some(own) = play.read() else { return Err("SteamVR gave no setup") }; // i18n: not shown (the walk's log)
            let live = if self.reading == Reading::Follows { play.live_standing() } else { None };
            let base = live.map_or_else(|| own.clone(), |live| own.at_live(&live));
            let over_other = self.other_showing || !same_place(&own.standing, &base.standing);
            if over_other {
                self.log.say(format!("the walk starts on another application's play space, {:.2} m from SteamVR's own setup", distance(&own.standing, &base.standing))); // i18n: not shown (the walk's log)
            }
            self.shown.clear();
            self.shown.push_back((now, base.standing));
            self.carried = Some(Carried { base, offset: [0.0; 3], over_other });
        }
        let carried = self.carried.as_mut().expect("set above");
        for (o, s) in carried.offset.iter_mut().zip(step) {
            *o += s;
        }
        let setup = carried.shifted();
        play.write(&setup);
        play.show();
        self.shown.push_back((now, setup.standing));
        self.showing = true;
        self.shown_at = Some(now);
        self.boost = speed * dir[0].hypot(dir[2]);
        self.moving = true;
        Ok(())
    }

    fn say_put_back(&mut self, why: &str) {
        if let Some(carried) = &self.carried {
            let walked = length(&carried.offset);
            self.log.say(format!("back where the carry began, {walked:.2} m ago: {why}")); // i18n: not shown (the walk's log)
        }
    }

    /// Puts the play space back where it was.
    pub fn reset(&mut self, steamvr: &vr::Client) {
        match steamvr.chaperone_setup() {
            Ok(mut setup) => {
                self.say_put_back("the overlay ends"); // i18n: not shown (the walk's log)
                self.put_back(&mut setup);
                self.end_through("the overlay ends"); // i18n: not shown (the walk's log)
            }
            // Nothing can be put back without the interface (SteamVR drops the preview with this process).
            Err(_) => self.carried = None,
        }
    }

    /// The same with `play`: the preview hidden first, so that the working copy reverts to the saved setup; or, when the
    /// walk began on another application's preview, that one shown again. A carry another application already ended
    /// leaves nothing to put back, and its preview stays.
    pub fn put_back(&mut self, play: &mut dyn PlaySpace) {
        self.moving = false;
        self.boost = 0.0;
        self.shown.clear();
        if let Some(carried) = self.carried.take() {
            self.showing = false;
            if carried.over_other {
                play.write(&carried.base);
                play.show();
            } else {
                play.hide();
                play.revert();
            }
        }
    }

    /// What a person did to the play space itself (a recenter, room setup): SteamVR's own setup is theirs now, and no
    /// preview of the boost's stays over it, even one whose carry another application seemed to end a moment ago (the
    /// recentred play space, read before SteamVR's event came).
    fn let_go(&mut self, play: &mut dyn PlaySpace, why: &str) {
        self.say_put_back(why);
        self.moving = false;
        self.boost = 0.0;
        self.shown.clear();
        self.carried = None;
        self.other_app = None;
        self.other_showing = false;
        if std::mem::take(&mut self.showing) {
            play.hide();
            play.revert();
        }
    }

    /// One of SteamVR's play-space events (see [`WalkBoost::play_space_event_in`]).
    pub fn play_space_event(&mut self, steamvr: &vr::Client, event: PlaySpaceEvent) {
        match steamvr.chaperone_setup() {
            Ok(mut setup) => self.play_space_event_in(&mut setup, event, Instant::now()),
            Err(_) => {
                if matches!(event, PlaySpaceEvent::RoomSetupStarting) {
                    self.carried = None;
                    self.showing = false;
                    self.room_setup = Some(Instant::now());
                }
            }
        }
    }

    /// The same through the driver, where nothing of the play space is the boost's: what a person does to it is theirs, and the
    /// walk adds up with the result. A recenter keeps the walk. Only room setup needs the devices where they really are in the
    /// room, and it holds the walk until it is saved. How far a recenter moved the play space goes in the log: SteamVR may
    /// take the new zero from the poses it is given, walk included, and that would stay in its saved setup.
    fn play_space_event_through(&mut self, play: &mut dyn PlaySpace, event: PlaySpaceEvent, now: Instant) {
        match event {
            PlaySpaceEvent::StandingReset | PlaySpaceEvent::SeatedReset => {
                if let (Some(through), Some(was), Some(live)) = (self.through, self.standing, play.live_standing()) {
                    self.log.say(format!("{event:?} with {:.2} m walked through the driver: the play space moved {:.2} m, the walk goes on", length(&through.offset), distance(&was, &live))); // i18n: not shown (the walk's log)
                    self.standing = Some(live);
                }
            }
            PlaySpaceEvent::RoomSetupStarting => {
                self.end_through("room setup"); // i18n: not shown (the walk's log)
                self.room_setup = Some(now);
            }
            PlaySpaceEvent::SetupCommitted => self.room_setup = None,
            PlaySpaceEvent::UniverseChanged { .. } | PlaySpaceEvent::FlushCache => {}
        }
    }

    /// One of SteamVR's play-space events at `now`, with `play`. What a person does ends the carry at once, the play
    /// space back to SteamVR's own setup (theirs now): a recenter, a seated reset (not within [`OWN_EVENTS`] of the
    /// boost's own preview: the boost moves the seated zero too, and that may be its echo), another tracking universe,
    /// room setup (which also holds the boost until it is saved). The rest, raised for every preview the boost shows
    /// and for other applications, changes nothing: the boost never looks at SteamVR's setup while its carry shows
    /// (reverting the working copy to look put the play space back where it started, the person's position reset,
    /// until the next push showed the carry again).
    pub fn play_space_event_in(&mut self, play: &mut dyn PlaySpace, event: PlaySpaceEvent, now: Instant) {
        if self.path == WalkPath::Driver {
            return self.play_space_event_through(play, event, now);
        }
        let own_echo = self.shown_at.is_some_and(|t| now.saturating_duration_since(t) < OWN_EVENTS);
        match event {
            PlaySpaceEvent::StandingReset => self.let_go(play, "a recenter"), // i18n: not shown (the walk's log)
            PlaySpaceEvent::SeatedReset if !own_echo => self.let_go(play, "a seated reset"), // i18n: not shown (the walk's log)
            PlaySpaceEvent::UniverseChanged { from, to } if from != to => self.let_go(play, "another tracking universe"), // i18n: not shown (the walk's log)
            PlaySpaceEvent::RoomSetupStarting => {
                self.let_go(play, "room setup"); // i18n: not shown (the walk's log)
                self.room_setup = Some(now);
            }
            // Room setup saved its play space: the boost may move it. Another application's saved change leaves the
            // carry as it is (it goes back to that setup when the carry ends).
            PlaySpaceEvent::SetupCommitted => self.room_setup = None,
            PlaySpaceEvent::SeatedReset | PlaySpaceEvent::UniverseChanged { .. } | PlaySpaceEvent::FlushCache => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaw(degrees: f32) -> Matrix34 {
        let (s, c) = degrees.to_radians().sin_cos();
        // A rotation about +Y (a head turned left by `degrees`).
        Matrix34 { m: [[c, 0.0, s, 0.0], [0.0, 1.0, 0.0, 1.7], [-s, 0.0, c, 0.0]] }
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn the_stick_points_where_the_head_looks() {
        // Looking down −Z: forward is −Z, right is +X.
        assert!(close(direction([0.0, 1.0], &yaw(0.0)), [0.0, 0.0, -1.0]));
        assert!(close(direction([1.0, 0.0], &yaw(0.0)), [1.0, 0.0, 0.0]));
        // Turned left a quarter: forward is −X.
        assert!(close(direction([0.0, 1.0], &yaw(90.0)), [-1.0, 0.0, 0.0]));
        // The dead zone, and half a push.
        assert_eq!(direction([0.05, 0.05], &yaw(0.0)), [0.0; 3]);
        let half = direction([0.0, 0.56], &yaw(0.0));
        assert!((half[2] + 0.5).abs() < 1e-3, "{half:?}");
    }

    #[test]
    fn carrying_moves_tracking_and_keeps_the_walls_in_the_room() {
        let standing = yaw(30.0);
        let wall = Quad { corners: [[1.0, 0.0, 1.0], [1.0, 2.0, 1.0], [1.0, 2.0, -1.0], [1.0, 0.0, -1.0]] };
        let carried = Carried { base: Setup { standing, seated: None, bounds: vec![wall] }, offset: [0.5, 0.0, -2.0], over_other: false };
        let Setup { standing: moved, bounds, .. } = carried.shifted();
        // A device fixed in the room (raw p) reads `offset` further in the standing universe.
        let raw = [0.3, 1.2, 0.4];
        let standing_of = |z: &Matrix34, p: [f32; 3]| {
            let d = [p[0] - z.m[0][3], p[1] - z.m[1][3], p[2] - z.m[2][3]];
            [0, 1, 2].map(|c| z.m[0][c] * d[0] + z.m[1][c] * d[1] + z.m[2][c] * d[2])
        };
        let before = standing_of(&standing, raw);
        let after = standing_of(&moved, raw);
        assert!(close([after[0] - before[0], after[1] - before[1], after[2] - before[2]], [0.5, 0.0, -2.0]));
        // A wall corner sits at the same place in the room.
        let room = |z: &Matrix34, c: [f32; 3]| [0, 1, 2].map(|r| z.m[r][0] * c[0] + z.m[r][1] * c[1] + z.m[r][2] * c[2] + z.m[r][3]);
        assert!(close(room(&standing, wall.corners[0]), room(&moved, bounds[0].corners[0])));
    }

    #[test]
    fn readings_that_are_not_numbers_do_not_move() {
        assert_eq!(push([f32::NAN, 0.5]), 0.0);
        assert_eq!(direction([f32::NAN, 1.0], &yaw(0.0)), [0.0; 3]);
        let mut broken = yaw(0.0);
        broken.m[0][2] = f32::NAN;
        assert_eq!(direction([0.0, 1.0], &broken), [0.0; 3]);
    }

    #[test]
    fn review_station_stop_survives_missing_and_repush() {
        let t = Instant::now();
        let mut g = Gate::default();
        let seated = Motion { in_station: true, speed: 4.0, ..Motion::default() };
        assert!(!g.carries(t, true, Some(&seated), 8.0));
        assert!(!g.carries(t + Duration::from_secs(1), true, None, 8.0));
        assert!(!g.carries(t, false, None, 0.0));
        assert!(!g.carries(t, true, None, 0.0));
        let exited = Motion { in_station: false, ..seated };
        assert!(g.carries(t, true, Some(&exited), 0.0));
    }

    #[test]
    fn review_positive_residual_stop_latches_until_release() {
        let t = Instant::now();
        let mut g = Gate::default();
        let native = Motion { speed: 12.0, ..Motion::default() };
        assert!(g.carries(t, true, Some(&native), 0.0));
        for ms in (10..2000).step_by(10) {
            assert!(g.carries(t + Duration::from_millis(ms), true, Some(&native), 8.0));
        }
        // Native progress ceased; scalar still contains 8 m/s of injected movement.
        let frozen = Motion { speed: 8.0, ..native };
        for ms in (2000..2400).step_by(10) {
            g.carries(t + Duration::from_millis(ms), true, Some(&frozen), 8.0);
        }
        for ms in (2400..10000).step_by(10) {
            assert!(!g.carries(t + Duration::from_millis(ms), true, Some(&native), 0.0));
        }
        assert!(!g.carries(t, false, Some(&native), 0.0));
        assert!(g.carries(t, true, Some(&native), 0.0));
    }

    #[test]
    fn carries_only_while_vrchat_walks() {
        let start = Instant::now();
        for hz in [72, 90] {
            for strength in [0.2, 0.5, 1.0] {
                let mut g = Gate::default();
                let mut m = Motion { speed: 12.0 * strength, ..Motion::default() };
                for frame in 0..30 * hz {
                    let now = start + Duration::from_secs_f64(frame as f64 / hz as f64);
                    // Delayed, unchanged and missing OSC samples must not create a boost cycle.
                    let motion = if frame % 19 == 0 { None } else { Some(&m) };
                    assert!(g.carries(now, true, motion, 8.0 * strength));
                }
                let now = start + Duration::from_secs(30);
                m.speed = 0.0;
                assert!(g.carries(now, true, Some(&m), 8.0));
                assert!(!g.carries(now + STILL_FOR, true, Some(&m), 8.0));
                m.speed = 4.0;
                assert!(!g.carries(now + Duration::from_secs(1), true, Some(&m), 0.0));
                m.in_station = true;
                assert!(!g.carries(now, true, Some(&m), 8.0));
                m.in_station = false;
                assert!(!g.carries(now, false, Some(&m), 0.0));
                assert!(g.carries(now, true, Some(&m), 0.0));
            }
        }
    }

    /// How SteamVR's working copy, preview and saved setup relate. Without a headset that cannot be checked, so walk
    /// boost must hold under each.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    enum Semantics {
        /// A preview is a copy of the working copy when shown; reverting the working copy gives the preview while one
        /// shows (what the boost assumed until now).
        #[default]
        Snapshot,
        /// A shown preview follows the working copy until hidden, and reverting re-reads the saved setup (SteamVR's
        /// log reads the chaperone file at each revert): reverting while a preview shows puts the play space back.
        Bound,
        /// A preview is a copy when shown; reverting gives the saved setup.
        SnapshotRevertSaved,
    }

    const EVERY_SEMANTICS: [Semantics; 3] = [Semantics::Snapshot, Semantics::Bound, Semantics::SnapshotRevertSaved];

    /// SteamVR's chaperone as walk boost meets it: a saved setup, the preview that shows (the live setup while it
    /// does), and the working copy.
    #[derive(Default)]
    struct Room {
        saved: Setup,
        preview: Option<Setup>,
        working: Setup,
        shows: usize,
        hides: usize,
        reverts: usize,
        semantics: Semantics,
        /// The preview that shows is another application's: this one's working copy has no hold on it.
        theirs: bool,
        /// The headset is not tracked: the play space cannot be read.
        untracked: bool,
        /// What the reading is off by, for a driver whose raw space is not the one the setup is given in.
        reading_off: f32,
    }

    impl Room {
        fn new(standing: Matrix34) -> Self {
            Self { saved: Setup { standing, ..Default::default() }, ..Default::default() }
        }

        fn with(semantics: Semantics) -> Self {
            Self { semantics, ..Self::new(Matrix34::IDENTITY) }
        }

        /// What every application sees.
        fn live(&self) -> &Setup {
            self.preview.as_ref().unwrap_or(&self.saved)
        }
    }

    impl PlaySpace for Room {
        fn revert(&mut self) {
            self.reverts += 1;
            self.working = match self.semantics {
                Semantics::Snapshot => self.live().clone(),
                Semantics::Bound | Semantics::SnapshotRevertSaved => self.saved.clone(),
            };
            if self.semantics == Semantics::Bound && self.preview.is_some() && !self.theirs {
                self.preview = Some(self.working.clone());
            }
        }

        fn read(&mut self) -> Option<Setup> {
            Some(self.working.clone())
        }

        fn write(&mut self, setup: &Setup) {
            self.working = setup.clone();
            if self.semantics == Semantics::Bound && self.preview.is_some() && !self.theirs {
                self.preview = Some(self.working.clone());
            }
        }

        fn show(&mut self) {
            self.preview = Some(self.working.clone());
            self.theirs = false;
            self.shows += 1;
        }

        fn hide(&mut self) {
            self.preview = None;
            self.theirs = false;
            self.hides += 1;
        }

        fn live_standing(&mut self) -> Option<Matrix34> {
            let mut live = self.live().standing;
            live.m[0][3] += self.reading_off;
            (!self.untracked).then_some(live)
        }
    }

    impl Room {
        /// OVR Advanced Settings' Space Drag at one tick: its own working copy shown, the saved setup moved by what it
        /// was dragged, with nothing of the boost's carry.
        fn other_app_shows(&mut self, dragged: [f32; 3]) {
            let mut theirs = self.saved.clone();
            for (row, d) in theirs.standing.m.iter_mut().zip(dragged) {
                row[3] -= d;
            }
            self.preview = Some(theirs);
            self.theirs = true;
        }
    }

    fn settings() -> WalkBoostSettings {
        WalkBoostSettings { enabled: true, strength: 0.5, ..Default::default() }
    }

    fn moment<'a>(settings: &'a WalkBoostSettings, place: Option<&'a str>) -> Moment<'a> {
        Moment { settings, walking_allowed: true, place, held: false, away: false, motion: Some(Motion { speed: 100.0, ..Motion::default() }), input_read: true }
    }

    fn boost() -> WalkBoost {
        WalkBoost::new(Some(Walk { set: 1, left: 2, right: 3 }))
    }

    const FORWARD: Option<[f32; 2]> = Some([0.0, 1.0]);

    /// The play space's standing origin, moved by the carry, as `room` shows it.
    fn carried_by(room: &Room) -> [f32; 3] {
        let (live, saved) = (&room.live().standing.m, &room.saved.standing.m);
        [0, 1, 2].map(|r| saved[r][3] - live[r][3])
    }

    #[test]
    fn carries_and_puts_back() {
        let (s, mut room, mut walk) = (settings(), Room::new(Matrix34::IDENTITY), boost());
        let t0 = Instant::now();
        let frame = |ms: u64| t0 + Duration::from_millis(ms);
        let m = moment(&s, Some("wrld_1"));
        walk.step(&mut room, &m, || FORWARD, || Some(yaw(0.0)), frame(0));
        assert!(!walk.moving(), "the first frame only starts the clock");
        walk.step(&mut room, &m, || FORWARD, || Some(yaw(0.0)), frame(10));
        assert!(walk.moving() && room.preview.is_some());
        // 10 ms at the full push: 4 m/s × 0.5 × 2 more, ahead (−Z) of the head.
        assert!(close(carried_by(&room), [0.0, 0.0, -0.04]), "{:?}", carried_by(&room));
        // Controllers that could not be read this frame: no step on a stale stick.
        walk.step(&mut room, &Moment { input_read: false, ..moment(&s, Some("wrld_1")) }, || FORWARD, || Some(yaw(0.0)), frame(20));
        assert!(!walk.moving());
        // Let go: the play space stays where it was carried.
        walk.step(&mut room, &m, || Some([0.0, 0.0]), || Some(yaw(0.0)), frame(30));
        assert!(!walk.moving() && room.preview.is_some() && room.hides == 0);
        // Another instance: back where it was, the working copy the saved setup again.
        walk.step(&mut room, &moment(&s, Some("wrld_2")), || None, || None, frame(40));
        assert!(room.preview.is_none() && room.hides == 1 && room.working == room.saved);
    }

    /// Walks forward `steps` frames of 10 ms from `from` ms; returns the time of the last step.
    fn walk_for(walk: &mut WalkBoost, room: &mut Room, s: &WalkBoostSettings, t0: Instant, from: u64, steps: u64) -> u64 {
        let m = moment(s, None);
        for i in 0..=steps {
            walk.step(room, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_millis(from + i * 10));
        }
        from + steps * 10
    }

    /// The reset Enzo met (2026-10-26): SteamVR raises play-space events for the boost's own previews, for other
    /// applications connecting, and when anything saves a setup. The boost reverted the working copy to look at
    /// them; when the preview follows the working copy that put the play space back where the walk started, and
    /// left it there until the next push showed the carry again (the position reset, then thrown forward). Now no
    /// such event touches the play space, under any of SteamVR's possible behaviours, while walking or standing.
    #[test]
    fn events_never_move_the_play_space() {
        let quiet = [PlaySpaceEvent::FlushCache, PlaySpaceEvent::UniverseChanged { from: 7, to: 7 }, PlaySpaceEvent::SetupCommitted];
        for semantics in EVERY_SEMANTICS {
            let (s, mut walk, mut room) = (settings(), boost(), Room::with(semantics));
            let t0 = Instant::now();
            let mut last = walk_for(&mut walk, &mut room, &s, t0, 0, 10);
            let mut expected = carried_by(&room);
            assert!(expected[2] < -0.3, "{semantics:?}: carried {expected:?}");
            for (round, after) in [5u64, 500, 1200, 2500, 6000].into_iter().enumerate() {
                let at = last + after;
                for event in quiet {
                    walk.play_space_event_in(&mut room, event, t0 + Duration::from_millis(at));
                    assert!(
                        close(carried_by(&room), expected),
                        "{semantics:?}: {event:?} {after} ms after a step moved {expected:?} to {:?}",
                        carried_by(&room)
                    );
                }
                // A seated reset just after the boost's own preview may be its echo: nothing either.
                if after < 1000 {
                    walk.play_space_event_in(&mut room, PlaySpaceEvent::SeatedReset, t0 + Duration::from_millis(at));
                    assert!(close(carried_by(&room), expected), "{semantics:?}: an echo moved the play space");
                }
                assert_eq!((room.hides, room.reverts), (0, 1), "{semantics:?}: reverted once, at the start, and never hidden");
                // Walking on carries on from where the person is, every round.
                last = walk_for(&mut walk, &mut room, &s, t0, at + 10, 5);
                assert!(carried_by(&room)[2] < expected[2] - 0.15, "{semantics:?} round {round}: the walk goes on");
                expected = carried_by(&room);
            }
        }
    }

    /// A recenter ends the carry at once, the play space back to SteamVR's own new setup: the next push starts from
    /// it, never thrown by the distance walked before (the old rebase kept the carry on the new setup: a recenter, or
    /// a setup someone saved with the carry in it, threw the person that far).
    #[test]
    fn a_recenter_ends_the_carry_without_throwing_the_person() {
        for semantics in EVERY_SEMANTICS {
            for steamvr_drops_the_preview in [true, false] {
                let (s, mut walk, mut room) = (settings(), boost(), Room::with(semantics));
                let t0 = Instant::now();
                let last = walk_for(&mut walk, &mut room, &s, t0, 0, 20);
                assert!(carried_by(&room)[2] < -0.7);
                // The person recentres: SteamVR saves a new standing zero.
                let mut recentred = yaw(90.0);
                recentred.m[0][3] = 0.4;
                room.saved.standing = recentred;
                if steamvr_drops_the_preview {
                    room.preview = None;
                }
                walk.play_space_event_in(&mut room, PlaySpaceEvent::StandingReset, t0 + Duration::from_millis(last + 5));
                let case = format!("{semantics:?}, preview dropped: {steamvr_drops_the_preview}");
                assert!(room.preview.is_none(), "{case}: the carry ended");
                assert!(room.live().close_to(&room.saved), "{case}: the recentred setup is the live one");
                // The next push starts from the recentred setup: two steps away from it (the first after a pause moves
                // 50 ms' worth at most, then 10 ms, at 4 m/s), not the 0.8 m walked before on top.
                walk_for(&mut walk, &mut room, &s, t0, last + 100, 1);
                let moved = carried_by(&room);
                let distance = (moved[0] * moved[0] + moved[2] * moved[2]).sqrt();
                assert!((distance - 0.24).abs() < 1e-3, "{case}: two steps from the new setup, not {distance} m");
            }
        }
    }

    #[test]
    fn a_seated_reset_after_the_walk_another_universe_and_room_setup_end_the_carry() {
        for semantics in EVERY_SEMANTICS {
            let (s, mut walk, mut room) = (settings(), boost(), Room::with(semantics));
            let t0 = Instant::now();
            let last = walk_for(&mut walk, &mut room, &s, t0, 0, 10);
            walk.play_space_event_in(&mut room, PlaySpaceEvent::SeatedReset, t0 + Duration::from_millis(last + 1500));
            assert!(room.preview.is_none() && room.live().close_to(&room.saved), "{semantics:?}: a seated reset, long after");

            let last = walk_for(&mut walk, &mut room, &s, t0, last + 2000, 10);
            assert!(room.preview.is_some());
            walk.play_space_event_in(&mut room, PlaySpaceEvent::UniverseChanged { from: 1, to: 2 }, t0 + Duration::from_millis(last + 5));
            assert!(room.preview.is_none(), "{semantics:?}: another tracking universe");

            // Room setup: back at once, and the boost holds until the new setup is saved.
            let last = walk_for(&mut walk, &mut room, &s, t0, last + 100, 10);
            walk.play_space_event_in(&mut room, PlaySpaceEvent::RoomSetupStarting, t0 + Duration::from_millis(last + 5));
            assert!(room.preview.is_none());
            let last = walk_for(&mut walk, &mut room, &s, t0, last + 100, 10);
            assert!(room.preview.is_none() && !walk.moving(), "{semantics:?}: held during room setup");
            walk.play_space_event_in(&mut room, PlaySpaceEvent::SetupCommitted, t0 + Duration::from_millis(last + 5));
            walk_for(&mut walk, &mut room, &s, t0, last + 100, 2);
            assert!(walk.moving() && room.preview.is_some(), "{semantics:?}: saved, the boost moves the new play space");
        }
    }

    #[test]
    fn steps_are_whole_frames() {
        let (s, mut walk, mut room) = (settings(), boost(), Room::new(Matrix34::IDENTITY));
        let frame = Duration::from_secs_f32(1.0 / 90.0);
        walk.set_frame(Some(frame));
        let t0 = Instant::now();
        let m = moment(&s, None);
        // The loop's turns land a little early or late on the compositor's frames.
        let mut last = 0.0;
        for (i, us) in [0u64, 11_900, 21_400, 34_100, 44_000, 67_300].into_iter().enumerate() {
            walk.step(&mut room, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_micros(us));
            let now = -carried_by(&room)[2];
            let step = now - last;
            last = now;
            if i == 0 {
                continue;
            }
            // 4 m/s × 0.5 × 2 = 4 m/s: a whole number of frames' worth each time (one, but two after the late turn).
            let frames = step / (4.0 * frame.as_secs_f32());
            assert!((frames - frames.round()).abs() < 1e-3 && frames.round() >= 1.0, "step {i}: {frames} frames");
        }
        assert!((last - 4.0 * 6.0 * frame.as_secs_f32()).abs() < 1e-3, "six frames in all: {last}");
    }

    #[test]
    fn waits_while_the_stick_drives_a_menu() {
        let (mut s, mut room, mut walk) = (settings(), Room::default(), boost());
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        walk.step(&mut room, &moment(&s, None), || FORWARD, || Some(yaw(0.0)), at(0));
        // The Walk boost radial turned: the stick is on the Action Menu for a moment.
        s.strength = 0.6;
        walk.step(&mut room, &moment(&s, None), || FORWARD, || Some(yaw(0.0)), at(10));
        walk.step(&mut room, &moment(&s, None), || FORWARD, || Some(yaw(0.0)), at(1000));
        assert!(!walk.moving() && room.preview.is_none());
        walk.step(&mut room, &moment(&s, None), || FORWARD, || Some(yaw(0.0)), at(1600));
        assert!(walk.moving());
        // N4N0's menu in the play space, or the owner away: it waits too.
        let held = Moment { held: true, ..moment(&s, None) };
        walk.step(&mut room, &held, || FORWARD, || Some(yaw(0.0)), at(1610));
        assert!(!walk.moving());
        let away = Moment { away: true, ..moment(&s, None) };
        walk.step(&mut room, &away, || FORWARD, || Some(yaw(0.0)), at(1620));
        assert!(!walk.moving());
        // VRChat says the avatar stands still: a push does not start the carry.
        let still = Moment { motion: Some(Motion { in_station: false, speed: 0.0, speed_at_ms: None }), ..moment(&s, None) };
        walk.step(&mut room, &still, || FORWARD, || Some(yaw(0.0)), at(1630));
        assert!(!walk.moving());
    }

    fn metres(v: [f32; 3]) -> f32 {
        length(&v)
    }

    /// A first walk with the play space read back, so that the reading is known to follow.
    fn proven(walk: &mut WalkBoost, room: &mut Room, s: &WalkBoostSettings, t0: Instant) -> u64 {
        let last = walk_for(walk, room, s, t0, 0, 30);
        assert_eq!(walk.reading, Reading::Follows);
        last
    }

    /// The reset Enzo met (2026-11-07), as his logs suggest: OVR Advanced Settings wrote the play space right after most of his
    /// walks (its Space Drag on Y, B and the right thumbrest), its show has none of the carry, so he went back where the
    /// walk began, and the next push threw him forward by all of it. That throw is what must never happen again.
    #[test]
    fn another_application_s_show_puts_the_person_back_once_and_the_next_push_starts_from_there() {
        for semantics in EVERY_SEMANTICS {
            let (s, mut walk, mut room) = (settings(), boost(), Room::with(semantics));
            let t0 = Instant::now();
            let last = proven(&mut walk, &mut room, &s, t0);
            let last = walk_for(&mut walk, &mut room, &s, t0, last + 10, 200);
            assert!(metres(carried_by(&room)) > 8.0, "{semantics:?}: {:?}", carried_by(&room));
            // The other application shows its play space: back, by its doing.
            room.other_app_shows([0.0; 3]);
            let idle = moment(&s, None);
            walk.step(&mut room, &idle, || None, || None, t0 + Duration::from_millis(last + 10));
            assert!(walk.carried.is_none() && walk.taken_lately(t0 + Duration::from_millis(last + 10)));
            assert!(room.theirs && room.hides == 0, "{semantics:?}: its preview stays as it is");
            // Pushing at once: the boost holds until it has been quiet a moment, and shows nothing over it.
            let shows = room.shows;
            let at = walk_for(&mut walk, &mut room, &s, t0, last + 20, 5);
            assert!(!walk.moving() && room.shows == shows && room.theirs, "{semantics:?}: no fight over the play space");
            // Then a walk from there, a step away: not the 8 m walked before.
            walk_for(&mut walk, &mut room, &s, t0, at + 500, 1);
            assert!(walk.moving());
            assert!(metres(carried_by(&room)) < 0.5, "{semantics:?}: thrown {:?}", carried_by(&room));
        }
    }

    /// A drag that goes on while the stick is pushed: nobody flickers between the two, and the boost waits for it.
    #[test]
    fn a_drag_under_way_is_not_fought_and_what_it_did_stays_under_the_next_walk() {
        for semantics in EVERY_SEMANTICS {
            let (s, mut walk, mut room) = (settings(), boost(), Room::with(semantics));
            let t0 = Instant::now();
            let mut at = proven(&mut walk, &mut room, &s, t0) + 10;
            let m = moment(&s, None);
            let mut dragged = 0.0;
            for _ in 0..60 {
                dragged += 0.03;
                room.other_app_shows([dragged, 0.0, 0.0]);
                walk.step(&mut room, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_millis(at));
                assert!(!walk.moving() && room.theirs, "{semantics:?}: the boost keeps out of it");
                assert!((carried_by(&room)[0] - dragged).abs() < 1e-3, "{semantics:?}: the drag is what shows");
                at += 10;
            }
            // Let go of the drag: after a quiet moment the walk starts on top of what it did.
            walk.step(&mut room, &m, || None, || None, t0 + Duration::from_millis(at + 500));
            assert!(walk.other_app.is_none(), "{semantics:?}: quiet");
            walk_for(&mut walk, &mut room, &s, t0, at + 600, 1);
            let moved = carried_by(&room);
            assert!(walk.moving() && (moved[0] - dragged).abs() < 1e-3, "{semantics:?}: the drag is kept: {moved:?}");
            assert!(moved[2] < -0.05 && moved[2] > -0.5, "{semantics:?}: a step ahead of it: {moved:?}");
            // And the end of that walk gives the drag back, not SteamVR's setup without it.
            walk.step(&mut room, &moment(&s, Some("wrld_2")), || None, || None, t0 + Duration::from_millis(at + 700));
            assert!((carried_by(&room)[0] - dragged).abs() < 1e-3 && carried_by(&room)[2].abs() < 1e-3, "{semantics:?}: {:?}", carried_by(&room));
        }
    }

    /// The boost's own shows, read back a little late, are its own.
    #[test]
    fn its_own_shows_read_late_are_its_own() {
        struct Late(Room, VecDeque<Matrix34>);
        impl PlaySpace for Late {
            fn revert(&mut self) {
                self.0.revert()
            }
            fn read(&mut self) -> Option<Setup> {
                self.0.read()
            }
            fn write(&mut self, setup: &Setup) {
                self.0.write(setup)
            }
            fn show(&mut self) {
                self.0.show();
                self.1.push_back(self.0.live().standing);
            }
            fn hide(&mut self) {
                self.0.hide()
            }
            fn live_standing(&mut self) -> Option<Matrix34> {
                // Ten frames behind.
                if self.1.len() > 10 {
                    self.1.pop_front();
                }
                Some(self.1.front().copied().unwrap_or(self.0.live().standing))
            }
        }
        let (s, mut walk, mut late) = (settings(), boost(), Late(Room::new(Matrix34::IDENTITY), VecDeque::new()));
        let t0 = Instant::now();
        let m = moment(&s, None);
        for i in 0..500 {
            walk.step(&mut late, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_millis(i * 10));
            assert!(walk.taken_at.is_none(), "frame {i}");
        }
        assert_eq!(walk.reading, Reading::Follows);
        assert!(metres(carried_by(&late.0)) > 19.0);
    }

    /// A reading that does not follow what is shown tells nothing: the boost carries on as it would without one, and
    /// never takes its own walk for another application's.
    #[test]
    fn a_reading_that_does_not_follow_is_left_alone() {
        let (s, mut walk, mut room) = (settings(), boost(), Room::new(Matrix34::IDENTITY));
        room.reading_off = 3.0;
        let t0 = Instant::now();
        walk_for(&mut walk, &mut room, &s, t0, 0, 400);
        assert_eq!(walk.reading, Reading::Unrelated);
        assert!(walk.taken_at.is_none() && walk.moving());
        assert_eq!(room.reverts, 1, "one carry all along");
        assert!(walk.log.said.iter().any(|l| l.contains("does not follow")), "{:?}", walk.log.said);
    }

    /// No reading while the headset is not tracked: nothing is decided on it, and a walk held up by that goes on.
    #[test]
    fn tracking_lost_a_moment_decides_nothing() {
        let (s, mut walk, mut room) = (settings(), boost(), Room::new(Matrix34::IDENTITY));
        let t0 = Instant::now();
        let last = proven(&mut walk, &mut room, &s, t0);
        let before = carried_by(&room);
        room.untracked = true;
        for i in 1..=30u64 {
            walk.step(&mut room, &moment(&s, None), || FORWARD, || None, t0 + Duration::from_millis(last + i * 10));
            assert!(!walk.moving(), "no way forward without tracking");
        }
        assert!(walk.carried.is_some() && close(carried_by(&room), before), "it stays where it is");
        room.untracked = false;
        walk_for(&mut walk, &mut room, &s, t0, last + 310, 1);
        let moved = metres(carried_by(&room)) - metres(before);
        assert!(walk.moving() && moved > 0.0 && moved < 0.3, "on from there: {moved}");
    }

    /// A recenter read as a move of the play space before SteamVR's event says what it is: its preview still goes.
    #[test]
    fn a_recenter_seen_before_its_event_still_hides_the_preview() {
        let (s, mut walk, mut room) = (settings(), boost(), Room::new(Matrix34::IDENTITY));
        let t0 = Instant::now();
        let last = proven(&mut walk, &mut room, &s, t0);
        room.other_app_shows([0.4, 0.0, 0.0]);
        walk.step(&mut room, &moment(&s, None), || None, || None, t0 + Duration::from_millis(last + 10));
        assert!(walk.carried.is_none());
        walk.play_space_event_in(&mut room, PlaySpaceEvent::StandingReset, t0 + Duration::from_millis(last + 20));
        assert!(room.preview.is_none() && walk.other_app.is_none(), "SteamVR's own setup, and free to walk");
    }

    /// A long walk with all that comes with it, at 90, 60, 45 and 30 frames a second: stalls, tracking lost, the stick
    /// let go, the avatar stopped by VRChat, recenters, another world and another application's drags. Between two frames
    /// the play space moves one step at most, unless another application or a person moved it at that very moment.
    #[test]
    fn a_long_walk_never_jumps() {
        /// A small repeatable noise (xorshift).
        fn noise(state: &mut u64) -> u64 {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            *state
        }
        for semantics in EVERY_SEMANTICS {
            let s = WalkBoostSettings { enabled: true, strength: 1.0, ..Default::default() };
            let (mut walk, mut room, mut state) = (boost(), Room::with(semantics), 0x9E37_79B9_7F4A_7C15u64);
            walk.set_frame(Some(Duration::from_secs_f32(1.0 / 90.0)));
            let t0 = Instant::now();
            let (mut ms, mut last, mut jumps_by_others, mut walked) = (0.0f64, carried_by(&room), 0, 0.0);
            let mut push_for = 0u32;
            let mut heading = 0.0f32;
            // Half an hour of frames.
            while ms < 30.0 * 60.0 * 1000.0 {
                let roll = noise(&mut state) % 10_000;
                // The frame rate falls and comes back; now and then a long stall.
                let frame_ms = [11.1, 16.7, 22.2, 33.3][(ms / 60_000.0) as usize % 4] + if roll == 0 { 400.0 } else { 0.0 };
                ms += frame_ms;
                let now = t0 + Duration::from_micros((ms * 1000.0) as u64);
                if push_for == 0 && noise(&mut state) % 40 == 0 {
                    push_for = (noise(&mut state) % 400) as u32;
                }
                push_for = push_for.saturating_sub(1);
                heading += (noise(&mut state) % 7) as f32 - 3.0;
                // A neutral reading is release; unavailable input must not rearm a safety stop.
                let stick = Some([0.0, if push_for > 0 { 1.0 } else { 0.0 }]);
                let tracked = noise(&mut state) % 500 != 0;
                room.untracked = !tracked;
                // Another application's drag, a recenter, another world: all may move the play space in one go.
                let event = noise(&mut state) % 4_000;
                let mut moved_by_someone = false;
                if event < 3 {
                    room.other_app_shows([0.1 * event as f32, 0.0, 0.0]);
                    moved_by_someone = true;
                } else if event == 3 {
                    walk.play_space_event_in(&mut room, PlaySpaceEvent::StandingReset, now);
                    moved_by_someone = true;
                }
                let place = if event == 4 { "wrld_2" } else { "wrld_1" };
                moved_by_someone |= walk.place.as_deref() != Some(place);
                // This fixture models total speed including injection; zero still models a freeze.
                let native = (noise(&mut state) % 8) as f32;
                let speed = if native > 0.0 { native + 8.0 } else { 0.0 };
                let m = Moment { place: Some(place), motion: Some(Motion { in_station: false, speed, speed_at_ms: None }), ..moment(&s, None) };
                walk.step(&mut room, &m, || stick, || tracked.then(|| yaw(heading)), now);
                let at = carried_by(&room);
                let jump = metres([at[0] - last[0], at[1] - last[1], at[2] - last[2]]);
                // 8 m/s for 50 ms at most.
                assert!(moved_by_someone || jump <= 0.4 + 1e-2, "{semantics:?} at {ms} ms: a jump of {jump} m");
                if moved_by_someone {
                    jumps_by_others += 1;
                } else {
                    walked += jump;
                }
                last = at;
            }
            assert!(walked > 1000.0 && jumps_by_others > 50, "{semantics:?}: walked {walked} m with {jumps_by_others} moves by others");
        }
    }

    /// The driver as a test sees it: what it was last written, and what it would add at each moment.
    #[derive(Default)]
    struct FakeDriver {
        state: Option<DriverState>,
        wanted: bool,
        written: WalkNow,
        written_at: Option<Instant>,
        writes: usize,
    }

    /// A shared handle to it: the boost owns the link, the test looks at the same one.
    #[derive(Clone)]
    struct Fake(std::sync::Arc<std::sync::Mutex<FakeDriver>>);

    impl DriverLink for Fake {
        fn state(&mut self) -> DriverState {
            let d = self.0.lock().unwrap();
            d.state.unwrap_or(if d.wanted { DriverState::Ready } else { DriverState::Hooking })
        }
        fn want(&mut self, on: bool) {
            self.0.lock().unwrap().wanted = on;
        }
        fn send(&mut self, walk: &WalkNow) {
            let mut d = self.0.lock().unwrap();
            d.written = *walk;
            d.writes += 1;
        }
    }

    impl Fake {
        fn new() -> Self {
            Self(Default::default())
        }

        /// What the driver adds to a pose at `now`, written at the last `at`.
        fn at(&self, age: Duration) -> WalkNow {
            self.0.lock().unwrap().written.after(age.as_micros() as u32)
        }

        fn set(&self, state: DriverState) {
            self.0.lock().unwrap().state = Some(state);
        }
    }

    /// The headset (and other devices) as SteamVR gives them with the driver in the way: each raw pose moved by the walk, then
    /// read in the universe that shows. What moves the play space is [`Room::other_app_shows`]'s, or a turn of it here.
    struct Rig {
        room: Room,
        fake: Fake,
        /// The head's real place in the raw tracking space.
        head: [f32; 3],
        /// A device whose driver hook does not follow the walk at all.
        left_behind: Option<u32>,
    }

    impl Rig {
        fn new(standing: Matrix34, fake: &Fake) -> Self {
            Self { room: Room::new(standing), fake: fake.clone(), head: [0.2, 1.7, -0.3], left_behind: None }
        }

        fn universe(&self) -> Matrix34 {
            self.room.live().standing
        }

        /// A place in the raw tracking space, as read in the standing universe that shows.
        fn standing_of(&self, raw: [f32; 3]) -> [f32; 3] {
            let u = self.universe();
            turned_back(&u, [raw[0] - u.m[0][3], raw[1] - u.m[1][3], raw[2] - u.m[2][3]])
        }

        /// The head, walk included, in the standing universe.
        fn head_standing(&self) -> [f32; 3] {
            let o = self.fake.at(Duration::ZERO).offset;
            self.standing_of([self.head[0] + o[0], self.head[1] + o[1], self.head[2] + o[2]])
        }

        /// Where the head looks in the standing universe, for a head looking `yaw` degrees in the raw tracking space.
        fn head_frame(&self, degrees: f32) -> Matrix34 {
            compose(&inverse(&self.universe()), &yaw(degrees))
        }

        /// Another application shows its play space: as its own working copy, this one turned and moved, no walk in it.
        fn other_app_moves(&mut self, standing: Matrix34) {
            let mut theirs = self.room.saved.clone();
            theirs.standing = standing;
            self.room.preview = Some(theirs);
            self.room.theirs = true;
        }
    }

    impl PlaySpace for Rig {
        fn revert(&mut self) {
            self.room.revert();
        }
        fn read(&mut self) -> Option<Setup> {
            self.room.read()
        }
        fn write(&mut self, setup: &Setup) {
            self.room.write(setup);
        }
        fn show(&mut self) {
            self.room.show();
        }
        fn hide(&mut self) {
            self.room.hide();
        }
        fn live_standing(&mut self) -> Option<Matrix34> {
            Some(self.universe())
        }
        fn devices(&mut self) -> Vec<(u32, [f32; 3])> {
            let o = self.fake.at(Duration::ZERO).offset;
            let mut out = vec![(0, self.head_standing())];
            let hand = [self.head[0] + 0.3, self.head[1] - 0.4, self.head[2]];
            out.push((3, self.standing_of([hand[0] + o[0], hand[1] + o[1], hand[2] + o[2]])));
            if let Some(device) = self.left_behind {
                out.push((device, self.standing_of(hand)));
            }
            out
        }
    }

    fn through_driver(fake: &Fake) -> WalkBoost {
        let mut walk = boost();
        walk.set_driver(Box::new(fake.clone()));
        walk
    }

    #[test]
    fn continuous_push_never_interrupts_driver_at_72_or_90_hz() {
        for hz in [72, 90] {
            for stick in [0.3, 0.6, 1.0] {
                let fake = Fake::new();
                let s = settings();
                let mut walk = through_driver(&fake);
                let mut rig = Rig::new(Matrix34::IDENTITY, &fake);
                let start = Instant::now();
                for frame in 0..=30 * hz {
                    let now = start + Duration::from_secs_f64(frame as f64 / hz as f64);
                    let motion = (frame % 19 != 0).then_some(Motion { speed: 12.0 * stick, speed_at_ms: Some(1), ..Motion::default() });
                    let m = Moment { motion, ..moment(&s, None) };
                    let head = rig.head_frame(0.0);
                    walk.step(&mut rig, &m, || Some([0.0, stick]), || Some(head), now);
                    if frame > hz { assert!(walk.moving(), "{hz} Hz, stick {stick}, frame {frame}"); }
                }
                assert!(!walk.log.said.iter().any(|s| s.contains("motion is stopped or uncertain")));
            }
        }
    }

    #[test]
    fn review_driver_station_missing_and_positive_frozen_motion_never_cycle() {
        for station in [false, true] {
            let fake = Fake::new();
            let s = settings();
            let mut walk = through_driver(&fake);
            let mut rig = Rig::new(Matrix34::IDENTITY, &fake);
            let t = Instant::now();
            for frame in 0..1000 {
                let motion = if frame < 100 { Some(Motion { speed: 12.0, ..Motion::default() }) }
                    else if station && frame == 100 { Some(Motion { in_station: true, speed: 12.0, ..Motion::default() }) }
                    else if station { None }
                    else { Some(Motion { speed: if frame < 150 { 4.0 } else { 12.0 }, ..Motion::default() }) };
                let m = Moment { motion, ..moment(&s, None) };
                let head = rig.head_frame(0.0);
                // Station survives release; a motion stop survives unavailable/nonfinite input.
                let stick = match (station, frame) {
                    (true, 200) => Some([0.0, 0.0]),
                    (false, 200) => None,
                    (false, 201) => Some([f32::NAN, 1.0]),
                    _ => FORWARD,
                };
                walk.step(&mut rig, &m, || stick, || Some(head), t + Duration::from_millis(frame * 10));
                if frame > 200 { assert!(!walk.moving(), "station={station}, frame={frame}"); }
            }
            let stopped = fake.at(Duration::ZERO).offset;
            for frame in 1000..1100 {
                let m = Moment { motion: None, ..moment(&s, None) };
                let head = rig.head_frame(0.0);
                walk.step(&mut rig, &m, || FORWARD, || Some(head), t + Duration::from_millis(frame * 10));
            }
            assert_eq!(fake.at(Duration::ZERO).offset, stopped);
        }
    }

    fn minus(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    /// Enzo: "I want both to work well together". Fast walk and OVR Advanced Settings' drag at once: at every frame, at any
    /// frame rate, in a play space turned any way, what the head does in the standing universe is the walk plus the drag,
    /// and the play space is never written by the boost.
    #[test]
    fn the_walk_and_another_application_s_drag_add_up_at_every_frame() {
        for start in [Matrix34::IDENTITY, yaw(30.0), yaw(-140.0)] {
            let fake = Fake::new();
            let (s, mut walk, mut rig) = (settings(), through_driver(&fake), Rig::new(start, &fake));
            let t0 = Instant::now();
            let m = moment(&s, None);
            let speed = RUN_SPEED * 0.5 * WalkBoostSettings::MAX_EXTRA;
            let mut ms = 0.0f64;
            // Up to speed, with the stick pushed and nobody else moving.
            for _ in 0..30 {
                ms += 11.1;
                let frame = rig.head_frame(0.0);
                walk.step(&mut rig, &m, || FORWARD, || Some(frame), t0 + Duration::from_micros((ms * 1000.0) as u64));
            }
            assert_eq!(walk.path, WalkPath::Driver);
            assert!(walk.moving());
            // Then the drag on top, frame after frame, with the frame rate falling: a metre a second in the standing universe.
            let forward = direction([0.0, 1.0], &rig.head_frame(0.0));
            let (mut last, mut dragged) = (rig.head_standing(), [0.0f32; 3]);
            for (i, frame_ms) in [11.1f64, 16.7, 22.2, 33.3].into_iter().cycle().take(400).enumerate() {
                ms += frame_ms;
                let dt = frame_ms as f32 / 1000.0;
                let drag_step = [dt, 0.0, -0.5 * dt];
                for (d, s) in dragged.iter_mut().zip(drag_step) {
                    *d += s;
                }
                // In the raw tracking space, the universe goes the other way.
                let raw = turned(&start, dragged);
                let mut moved = start;
                for (row, d) in moved.m.iter_mut().zip(raw) {
                    row[3] -= d;
                }
                rig.other_app_moves(moved);
                let frame = rig.head_frame(0.0);
                walk.step(&mut rig, &m, || FORWARD, || Some(frame), t0 + Duration::from_micros((ms * 1000.0) as u64));
                let head = rig.head_standing();
                let went = minus(head, last);
                let expected = [0, 1, 2].map(|c| forward[c] * speed * dt + drag_step[c]);
                let off = length(&minus(went, expected));
                assert!(off < 2e-3, "frame {i}: went {went:?}, the walk and the drag are {expected:?}");
                last = head;
                assert!(walk.moving() && walk.carried.is_none());
            }
            assert_eq!((rig.room.shows, rig.room.reverts), (0, 0), "the boost never touched the play space");
            assert!(walk.taken_at.is_none(), "nobody took anything from anybody");
            // Let go of the stick: the walk stays where it got to, the drag goes on, and nobody goes back.
            let held = rig.head_standing();
            for _ in 0..40 {
                ms += 11.1;
                walk.step(&mut rig, &m, || Some([0.0, 0.0]), || None, t0 + Duration::from_micros((ms * 1000.0) as u64));
            }
            assert!(!walk.moving());
            let still = rig.head_standing();
            let slowed = length(&minus(still, held));
            // At most what it takes to stop from that speed.
            assert!(slowed < speed * speed / (2.0 * WALK_ACCELERATION) + 0.2, "{slowed}");
            for _ in 0..40 {
                ms += 11.1;
                walk.step(&mut rig, &m, || None, || None, t0 + Duration::from_micros((ms * 1000.0) as u64));
            }
            assert_eq!(rig.head_standing(), still, "standing still, it stays");
        }
    }

    /// Space Turn around the head while walking: the walk goes on where the head looks in the universe that shows, no jump.
    #[test]
    fn the_walk_goes_on_through_another_application_s_turn() {
        let fake = Fake::new();
        let (s, mut walk, mut rig) = (settings(), through_driver(&fake), Rig::new(Matrix34::IDENTITY, &fake));
        let t0 = Instant::now();
        let m = moment(&s, None);
        let speed = RUN_SPEED * 0.5 * WalkBoostSettings::MAX_EXTRA;
        let mut ms = 0u64;
        for _ in 0..30 {
            ms += 10;
            let frame = rig.head_frame(0.0);
            walk.step(&mut rig, &m, || FORWARD, || Some(frame), t0 + Duration::from_millis(ms));
        }
        let mut last = rig.head_standing();
        for i in 1..=300 {
            // A degree every 10 ms about where the head is now: its standing place stays, its standing way round turns.
            let o = fake.at(Duration::ZERO).offset;
            let head = [rig.head[0] + o[0], rig.head[1] + o[1], rig.head[2] + o[2]];
            let at = rig.head_standing();
            let mut turned_universe = yaw(i as f32);
            let r = turned(&turned_universe, at);
            for (k, row) in turned_universe.m.iter_mut().enumerate() {
                row[3] = head[k] - r[k];
            }
            rig.other_app_moves(turned_universe);
            let head_after_turn = rig.head_standing();
            assert!(length(&minus(head_after_turn, at)) < 1e-3, "a turn about the head leaves it there");
            ms += 10;
            let frame = rig.head_frame(0.0);
            let forward = direction([0.0, 1.0], &frame);
            walk.step(&mut rig, &m, || FORWARD, || Some(frame), t0 + Duration::from_millis(ms));
            let now = rig.head_standing();
            let went = minus(now, last);
            assert!((length(&went) - speed * 0.01).abs() < 2e-3, "turn {i}: went {} m in a frame", length(&went));
            // What was written a frame ago: a degree behind at most.
            let along = went[0] * forward[0] + went[2] * forward[2];
            assert!(along > 0.99 * speed * 0.01, "turn {i}: went {went:?}, forward is {forward:?}");
            last = now;
        }
    }

    /// The driver coasts between the overlay's writes, at the very speed it was given: wherever a device's pose falls between
    /// two frames, it is on the line between them. After a stall too, the next write starts where the driver stopped.
    #[test]
    fn what_the_driver_adds_between_two_writes_is_on_the_way_from_one_to_the_next() {
        let fake = Fake::new();
        let (s, mut walk, mut rig) = (WalkBoostSettings { strength: 1.0, ..settings() }, through_driver(&fake), Rig::new(Matrix34::IDENTITY, &fake));
        let t0 = Instant::now();
        let m = moment(&s, None);
        let mut ms = 0u64;
        for _ in 0..30 {
            ms += 11;
            walk.step(&mut rig, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_millis(ms));
        }
        for gap in [11u64, 17, 33, 45, 400, 11, 2000, 11] {
            let before = fake.at(Duration::ZERO);
            // What a pose arriving just before the next write would get, the driver's own coasting cut off as it is.
            let coasted = fake.at(Duration::from_millis(gap.saturating_sub(1)));
            ms += gap;
            walk.step(&mut rig, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_millis(ms));
            let after = fake.at(Duration::ZERO);
            let jump = length(&minus(after.offset, coasted.offset));
            assert!(jump < 8.0 * 0.0015, "after {gap} ms: from {:?} to {:?}, a jump of {jump}", coasted.offset, after.offset);
            let step = length(&minus(after.offset, before.offset));
            assert!(step <= 8.0 * MAX_STEP_S + 1e-3, "after {gap} ms: a step of {step} m");
        }
    }

    /// Starting and stopping are gathered over a few frames: SteamVR predicts each frame ahead with the walk's speed.
    #[test]
    fn the_walk_gathers_and_loses_speed_within_its_acceleration() {
        let fake = Fake::new();
        let (s, mut walk, mut rig) = (WalkBoostSettings { strength: 1.0, ..settings() }, through_driver(&fake), Rig::new(Matrix34::IDENTITY, &fake));
        let t0 = Instant::now();
        let m = moment(&s, None);
        let mut last = 0.0f32;
        for i in 1..200u64 {
            let pushed = i < 100;
            walk.step(&mut rig, &m, || if pushed { FORWARD } else { Some([0.0, 0.0]) }, || Some(yaw(0.0)), t0 + Duration::from_millis(i * 10));
            let speed = length(&fake.at(Duration::ZERO).velocity);
            assert!((speed - last).abs() <= WALK_ACCELERATION * 0.01 + 1e-3, "frame {i}: from {last} to {speed} m/s");
            last = speed;
        }
        assert_eq!(last, 0.0, "stopped");
        assert!(walk.boost == 0.0 && !walk.moving());
    }

    /// A long walk with everything that comes with it, as `a_long_walk_never_jumps` for the play space: the frame rate falling,
    /// stalls, tracking lost, the stick let go, VRChat stopping the avatar, another application dragging on and off, recenters.
    /// At every frame the head does what the drag did plus at most a step of the walk: nothing is ever lost, nothing thrown.
    #[test]
    fn a_long_walk_with_a_drag_never_jumps() {
        fn noise(state: &mut u64) -> u64 {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            *state
        }
        let fake = Fake::new();
        let s = WalkBoostSettings { enabled: true, strength: 1.0, ..Default::default() };
        let (mut walk, mut rig, mut state) = (through_driver(&fake), Rig::new(Matrix34::IDENTITY, &fake), 0x9E37_79B9_7F4A_7C15u64);
        let t0 = Instant::now();
        let (mut ms, mut push_for, mut drag_for, mut heading) = (0.0f64, 0u32, 0u32, 0.0f32);
        let mut dragged = [0.0f32; 3];
        let mut last = rig.head_standing();
        let (mut walked, mut total_dragged, mut frames) = (0.0f32, 0.0f32, 0u64);
        while ms < 30.0 * 60.0 * 1000.0 {
            let roll = noise(&mut state) % 10_000;
            let frame_ms = [11.1, 16.7, 22.2, 33.3][(ms / 60_000.0) as usize % 4] + if roll == 0 { 400.0 } else { 0.0 };
            ms += frame_ms;
            let now = t0 + Duration::from_micros((ms * 1000.0) as u64);
            if push_for == 0 && noise(&mut state) % 40 == 0 {
                push_for = (noise(&mut state) % 400) as u32;
            }
            push_for = push_for.saturating_sub(1);
            if drag_for == 0 && noise(&mut state) % 200 == 0 {
                drag_for = (noise(&mut state) % 200) as u32;
            }
            heading += (noise(&mut state) % 7) as f32 - 3.0;
            let stick = (push_for > 0).then_some([0.0, 1.0]);
            let tracked = noise(&mut state) % 500 != 0;
            // The drag this frame.
            let mut drag_step = [0.0; 3];
            if drag_for > 0 {
                drag_for -= 1;
                let dt = frame_ms.min(50.0) as f32 / 1000.0;
                drag_step = [dt, 0.0, 0.3 * dt];
                for (d, s) in dragged.iter_mut().zip(drag_step) {
                    *d += s;
                }
                let mut moved = Matrix34::IDENTITY;
                for (row, d) in moved.m.iter_mut().zip(dragged) {
                    row[3] = -d;
                }
                rig.other_app_moves(moved);
            }
            if noise(&mut state) % 20_000 == 0 {
                walk.play_space_event_in(&mut rig, PlaySpaceEvent::StandingReset, now);
            }
            let speed = (noise(&mut state) % 16) as f32;
            let m = Moment { place: None, motion: Some(Motion { in_station: false, speed, speed_at_ms: None }), ..moment(&s, None) };
            let frame = rig.head_frame(heading);
            walk.step(&mut rig, &m, || stick, || tracked.then_some(frame), now);
            let head = rig.head_standing();
            let by_walk = minus(minus(head, last), drag_step);
            let jump = length(&by_walk);
            // 8 m/s for 50 ms at most.
            assert!(jump <= 0.4 + 1e-2, "at {ms} ms: {jump} m besides the drag");
            walked += jump;
            total_dragged += length(&drag_step);
            frames += 1;
            last = head;
        }
        assert!(walked > 1000.0 && total_dragged > 100.0, "walked {walked} m, dragged {total_dragged} m in {frames} frames");
        // All of it is there: the drag whole, and the walk the driver was told, in one place.
        let o = fake.at(Duration::ZERO).offset;
        let expected = [rig.head[0] + o[0] + dragged[0], rig.head[1] + o[1] + dragged[1], rig.head[2] + o[2] + dragged[2]];
        assert!(length(&minus(last, expected)) < 0.05, "{last:?} is not {expected:?}");
        assert_eq!((rig.room.shows, rig.room.reverts), (0, 0));
    }

    /// Without the driver the lesser way goes on as it did, and says why; a driver SteamVR loaded before this version too.
    #[test]
    fn without_the_driver_the_walk_moves_the_play_space_and_says_why() {
        for (state, why) in [(DriverState::NotRunning, WhyPlaySpace::NotRunning), (DriverState::Old, WhyPlaySpace::OldDriver)] {
            let fake = Fake::new();
            fake.set(state);
            let (s, mut walk, mut room) = (settings(), through_driver(&fake), Room::new(Matrix34::IDENTITY));
            walk_for(&mut walk, &mut room, &s, Instant::now(), 0, 20);
            assert_eq!(walk.path, WalkPath::PlaySpace(why));
            assert!(room.shows > 0 && metres(carried_by(&room)) > 0.5);
            assert_eq!(fake.0.lock().unwrap().writes, 0, "nothing written to a driver that would not add it");
            assert_eq!(walk.path_changed(), Some(WalkPath::PlaySpace(why)));
            assert_eq!(walk.path_changed(), None, "said once");
        }
        // The setting turned off: the same, for that reason, and the driver is not asked to hook anything.
        let fake = Fake::new();
        let s = WalkBoostSettings { through_driver: false, ..settings() };
        let (mut walk, mut room) = (through_driver(&fake), Room::new(Matrix34::IDENTITY));
        walk_for(&mut walk, &mut room, &s, Instant::now(), 0, 20);
        assert_eq!(walk.path, WalkPath::PlaySpace(WhyPlaySpace::Setting));
        assert!(room.shows > 0 && !fake.0.lock().unwrap().wanted);
        // Fast walk off: nothing asks the driver to hook SteamVR's poses for it.
        let fake = Fake::new();
        let s = WalkBoostSettings { enabled: false, ..settings() };
        let (mut walk, mut room) = (through_driver(&fake), Room::new(Matrix34::IDENTITY));
        walk_for(&mut walk, &mut room, &s, Instant::now(), 0, 2);
        assert!(!fake.0.lock().unwrap().wanted);
        assert_eq!(room.shows, 0);
    }

    /// Whichever way it goes, the first metres are looked at. A device that did not come along (a hook in the way, a driver that
    /// sends its poses another way) means the driver cannot carry this rig: back to the lesser way, with what it had carried
    /// taken back, and the log says which device.
    #[test]
    fn a_device_that_does_not_follow_sends_the_walk_back_to_the_play_space() {
        let fake = Fake::new();
        let (s, mut walk, mut rig) = (settings(), through_driver(&fake), Rig::new(Matrix34::IDENTITY, &fake));
        rig.left_behind = Some(5);
        let t0 = Instant::now();
        let m = moment(&s, None);
        let mut ms = 0u64;
        while walk.path != WalkPath::PlaySpace(WhyPlaySpace::Refused) {
            ms += 11;
            assert!(ms < 5000, "never noticed");
            walk.step(&mut rig, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_millis(ms));
            if walk.refused {
                walk.step(&mut rig, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_millis(ms + 11));
            }
        }
        assert!(walk.log.said.iter().any(|l| l.contains("NOT every device") && l.contains("device 5")), "{:?}", walk.log.said);
        assert!(fake.at(Duration::ZERO).is_zero(), "what it carried goes back");
        assert!(!fake.0.lock().unwrap().wanted, "and the driver is no longer asked");
        // It walks on the other way.
        ms += 100;
        for _ in 0..5 {
            ms += 11;
            walk.step(&mut rig, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_millis(ms));
        }
        assert!(rig.room.shows > 0);

        // Every device there: said once, and on with the driver.
        let fake = Fake::new();
        let (mut walk, mut rig) = (through_driver(&fake), Rig::new(Matrix34::IDENTITY, &fake));
        for i in 1..200u64 {
            walk.step(&mut rig, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_millis(i * 11));
        }
        assert!(walk.proven && walk.path == WalkPath::Driver);
        assert_eq!(walk.log.said.iter().filter(|l| l.contains("every device followed")).count(), 1, "{:?}", walk.log.said);
    }

    /// A drag in the middle of the check does not count against the driver.
    #[test]
    fn a_drag_does_not_fail_the_check() {
        let fake = Fake::new();
        let (s, mut walk, mut rig) = (settings(), through_driver(&fake), Rig::new(Matrix34::IDENTITY, &fake));
        let t0 = Instant::now();
        let m = moment(&s, None);
        for i in 1..300u64 {
            let mut moved = Matrix34::IDENTITY;
            moved.m[0][3] = -(i as f32) * 0.05;
            rig.other_app_moves(moved);
            walk.step(&mut rig, &m, || FORWARD, || Some(rig_frame()), t0 + Duration::from_millis(i * 11));
        }
        assert!(!walk.refused, "{:?}", walk.log.said);
        assert_eq!(walk.path, WalkPath::Driver);
    }

    fn rig_frame() -> Matrix34 {
        yaw(0.0)
    }

    /// What a person does to the play space stays theirs, under the walk: a recenter keeps it, as nothing of the play space is the
    /// boost's. Room setup needs the devices where they are: back, and held until it is saved. Another world: back too.
    #[test]
    fn through_the_driver_a_recenter_keeps_the_walk_and_room_setup_holds_it() {
        let fake = Fake::new();
        let (s, mut walk, mut rig) = (settings(), through_driver(&fake), Rig::new(Matrix34::IDENTITY, &fake));
        let t0 = Instant::now();
        let m = moment(&s, Some("wrld_1"));
        let mut ms = 0u64;
        let mut go = |walk: &mut WalkBoost, rig: &mut Rig, m: &Moment, stick: Option<[f32; 2]>, frames: u64| {
            for _ in 0..frames {
                ms += 10;
                walk.step(rig, m, || stick, || Some(yaw(0.0)), t0 + Duration::from_millis(ms));
            }
            t0 + Duration::from_millis(ms)
        };
        let now = go(&mut walk, &mut rig, &m, FORWARD, 100);
        let carried = fake.at(Duration::ZERO).offset;
        assert!(length(&carried) > 2.0);
        rig.room.saved.standing = yaw(90.0);
        rig.room.preview = None;
        walk.play_space_event_in(&mut rig, PlaySpaceEvent::StandingReset, now);
        assert_eq!(fake.at(Duration::ZERO).offset, carried, "the walk goes on");
        assert_eq!((rig.room.hides, rig.room.reverts), (0, 0), "SteamVR's setup is left as the person made it");
        assert!(walk.log.said.iter().any(|l| l.contains("StandingReset") && l.contains("the walk goes on")), "{:?}", walk.log.said);
        // Walking on, in the recentred play space: the way it faces now.
        let now = go(&mut walk, &mut rig, &m, FORWARD, 30);
        assert!(length(&minus(fake.at(Duration::ZERO).offset, carried)) > 0.5);
        // Room setup: back, and held.
        walk.play_space_event_in(&mut rig, PlaySpaceEvent::RoomSetupStarting, now);
        assert!(fake.at(Duration::ZERO).is_zero());
        let now = go(&mut walk, &mut rig, &m, FORWARD, 20);
        assert!(fake.at(Duration::ZERO).is_zero() && !walk.moving(), "held");
        walk.play_space_event_in(&mut rig, PlaySpaceEvent::SetupCommitted, now);
        go(&mut walk, &mut rig, &m, FORWARD, 20);
        assert!(!fake.at(Duration::ZERO).is_zero(), "saved, it walks again");
        // Another world: back.
        let other = moment(&s, Some("wrld_2"));
        go(&mut walk, &mut rig, &other, None, 1);
        assert!(fake.at(Duration::ZERO).is_zero());
        assert!(walk.through.is_none());
    }

    /// OVR Advanced Settings' Space Drag as its source has it (`MoveCenterTabController::updateHandDrag`): it reads the hand in the
    /// standing universe, adds its own offset, and adds what that changed by to its offset, which moves the universe. So it takes
    /// everything the hand does in the standing universe for the hand's own motion, the walk included.
    struct RealDrag {
        offset: [f32; 3],
        last: Option<[f32; 3]>,
        /// The dragging hand's real place in the raw tracking space.
        hand: [f32; 3],
    }

    impl RealDrag {
        fn tick(&mut self, rig: &mut Rig, hand_moved: [f32; 3], held: bool) {
            if !held {
                self.last = None;
                return;
            }
            self.hand = [self.hand[0] + hand_moved[0], self.hand[1] + hand_moved[1], self.hand[2] + hand_moved[2]];
            let walked = rig.fake.at(Duration::ZERO).offset;
            let relative = rig.standing_of([self.hand[0] + walked[0], self.hand[1] + walked[1], self.hand[2] + walked[2]]);
            let absolute = [relative[0] + self.offset[0], relative[1] + self.offset[1], relative[2] + self.offset[2]];
            if let Some(last) = self.last {
                for ((o, a), l) in self.offset.iter_mut().zip(absolute).zip(last) {
                    *o += a - l;
                }
            }
            self.last = Some(absolute);
            let mut universe = Matrix34::IDENTITY;
            for (row, o) in universe.m.iter_mut().zip(self.offset) {
                row[3] = o;
            }
            rig.other_app_moves(universe);
        }
    }

    /// With its real arithmetic: a drag held holds the hand where it is in the universe, so for as long as it is held the walk
    /// (which it reads as the hand moving) is taken back by the drag itself, and the hand's own motion drags as it always does.
    /// That is what a drag means. What must hold: nothing jumps going in or coming out, nobody goes back, and the walk goes on
    /// at its speed once the drag is let go.
    #[test]
    fn a_real_drag_held_holds_the_person_and_letting_go_walks_on_from_there() {
        let fake = Fake::new();
        let (s, mut walk, mut rig) = (settings(), through_driver(&fake), Rig::new(Matrix34::IDENTITY, &fake));
        let mut drag = RealDrag { offset: [0.0; 3], last: None, hand: [0.5, 1.2, -0.3] };
        let t0 = Instant::now();
        let m = moment(&s, None);
        let speed = RUN_SPEED * 0.5 * WalkBoostSettings::MAX_EXTRA;
        let step_most = speed * 0.0111 + 1e-3;
        let mut ms = 0.0f64;
        let mut go = |walk: &mut WalkBoost, rig: &mut Rig, drag: &mut RealDrag, held: bool, hand: [f32; 3]| {
            ms += 11.1;
            drag.tick(rig, hand, held);
            walk.step(rig, &m, || FORWARD, || Some(yaw(0.0)), t0 + Duration::from_micros((ms * 1000.0) as u64));
        };
        for _ in 0..200 {
            go(&mut walk, &mut rig, &mut drag, false, [0.0; 3]);
        }
        let walked = rig.head_standing();
        assert!(walked[2] < -5.0, "{walked:?}");
        // Held still: the walk is taken back as it comes, with no jump on the way in, and what was walked stays.
        let mut last = walked;
        for i in 0..100 {
            go(&mut walk, &mut rig, &mut drag, true, [0.0; 3]);
            let head = rig.head_standing();
            let moved = length(&minus(head, last));
            assert!(moved <= step_most, "frame {i} of the drag: {moved} m");
            last = head;
        }
        let held = rig.head_standing();
        assert!(length(&minus(held, walked)) < 3.0 * step_most, "held where it was: from {walked:?} to {held:?}");
        // The hand's own motion drags as it does alone: half a metre forward, the person half a metre back.
        for _ in 0..50 {
            go(&mut walk, &mut rig, &mut drag, true, [0.0, 0.0, -0.01]);
        }
        let dragged = rig.head_standing();
        assert!((dragged[2] - held[2] - 0.5).abs() < 0.1, "from {held:?} to {dragged:?}");
        // Let go: no jump, and the walk goes on from where the drag left it, at its speed.
        last = dragged;
        for i in 0..100 {
            go(&mut walk, &mut rig, &mut drag, false, [0.0; 3]);
            let head = rig.head_standing();
            let moved = minus(head, last);
            assert!(length(&moved) <= step_most, "frame {i} after the drag: {moved:?}");
            assert!(moved[2] <= 1e-3, "frame {i}: never back");
            last = head;
        }
        assert!((dragged[2] - last[2] - speed * 1.11).abs() < 0.1, "on at its speed: from {dragged:?} to {last:?}");
        assert_eq!((rig.room.shows, rig.room.reverts, rig.room.hides), (0, 0, 0), "the boost never touched the play space");
    }
}
