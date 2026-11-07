//! Walk boost: while VRChat walks the avatar with the stick, the play space is dragged the same way (the working copy
//! of SteamVR's chaperone setup, previewed live like OVR Advanced Settings' space drag, never saved), so the avatar
//! covers more ground than VRChat's own walking. The chaperone's walls move the other way, so they stay on the room's
//! walls.
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
/// Slower than this on its own (the boost's drag taken out), VRChat stands the avatar still.
const VRCHAT_STILL: f32 = 0.15;
/// VRChat's speed reaches Orbiters this late (OSC, then the service): the boost's own drag shows in it after this.
const MOTION_LAG: Duration = Duration::from_millis(500);
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
}

/// What VRChat says of the avatar over OSC (its `InStation`, `Seated` and `VelocityMagnitude` parameters).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motion {
    /// In a station (a chair, a vehicle), seated or not: VRChat does not walk the avatar.
    pub in_station: bool,
    /// How fast the avatar moves (m/s): VRChat's walking, and the boost's own drag (room-scale motion counts).
    pub speed: f32,
}

/// Whether VRChat walks the avatar, as far as walk boost can tell, push by push.
#[derive(Clone, Copy, Debug, Default)]
pub struct Gate {
    /// Carrying since then, in this push.
    since: Option<Instant>,
    /// VRChat's own walking (its speed less the boost's) has stayed below walking since then.
    still_since: Option<Instant>,
    /// Carrying stopped with the stick still pushed: VRChat's reports still count the boost's drag until then, so
    /// they cannot start it again before.
    resume_after: Option<Instant>,
}

impl Gate {
    /// Whether the boost carries the person at `now`: only while the stick is pushed (`pushed`), and, when VRChat
    /// says how the avatar moves, while it walks it. A push starts carrying once VRChat moves the avatar on its own
    /// (nothing of the boost in its speed yet). Carrying, VRChat's speed counts the boost's drag, `boost` (m/s, the
    /// last frame's): it is taken out once VRChat's reports caught up with it, and carrying stops when what is left
    /// stays below walking a moment (an Action Menu opened mid-walk took the stick, the world froze the avatar) or
    /// VRChat seats the avatar. Stopped so, it starts again only once VRChat's reports no longer carry the boost's
    /// drag (else that drag, reported late, restarted it at once and the boost never stopped, in fits).
    pub fn carries(&mut self, now: Instant, pushed: bool, motion: Option<&Motion>, boost: f32) -> bool {
        let since = |t: Instant| now.saturating_duration_since(t);
        if !pushed {
            *self = Gate::default();
            return false;
        }
        let carrying = match (motion, self.since) {
            (None, _) => true,
            (Some(m), _) if m.in_station => false,
            (Some(m), None) => m.speed >= VRCHAT_WALKS && self.resume_after.is_none_or(|t| now >= t),
            (Some(m), Some(start)) => {
                if since(start) < MOTION_LAG || m.speed - boost >= VRCHAT_STILL {
                    self.still_since = None;
                    true
                } else {
                    since(*self.still_since.get_or_insert(now)) < STILL_FOR
                }
            }
        };
        if carrying {
            self.since.get_or_insert(now);
        } else {
            let resume_after = if self.since.is_some() { Some(now + MOTION_LAG) } else { self.resume_after };
            *self = Gate { resume_after, ..Gate::default() };
        }
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
    failed: Option<String>,
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
}

impl WalkBoost {
    /// Walk boost with the overlay's actions (none when SteamVR's input could not load them: `failed` says why).
    pub fn new(actions: Option<Walk>, failed: Option<String>) -> Self {
        Self {
            actions,
            carried: None,
            place: None,
            last: None,
            failed,
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
        }
    }

    /// The headset's frame (from its refresh rate), which steps are counted in.
    pub fn set_frame(&mut self, frame: Option<Duration>) {
        self.frame = frame.filter(|f| !f.is_zero() && f.as_secs_f32() <= MAX_STEP_S);
    }

    /// Why walk boost cannot work, if it cannot.
    pub fn unavailable(&self) -> Option<&str> {
        self.failed.as_deref()
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
            Some("no controls")
        } else if !m.settings.enabled || m.settings.strength <= 0.0 {
            Some("turned off")
        } else if !m.walking_allowed {
            Some("VRChat's scene not in front (dashboard, loading)")
        } else if m.held {
            Some("a menu or the calibration stands in the play space")
        } else if m.away {
            Some("away")
        } else if self.changed.is_some_and(|t| since(t) < MENU_QUIET) {
            Some("a setting just changed")
        } else if self.room_setup.is_some_and(|t| since(t) < ROOM_SETUP) {
            Some("room setup")
        } else if self.other_app.is_some() {
            Some("another application moves the play space")
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
                    self.log.say(format!("the play space read from the headset's poses does not follow the walk's shows: another application's will not be noticed (read {:?})", live.m));
                    Reading::Unrelated
                };
            }
            return;
        }
        if !ours {
            let (carried, back) = (length(&carried.offset), distance(&carried.base.standing, &live));
            self.log.say(format!("another application showed its own play space: {carried:.2} m walked are gone, it is {back:.2} m from where the walk began"));
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
        let dt = dt.min(MAX_STEP_S);
        self.last = Some(now);
        self.moving = false;
        if self.place.as_deref() != m.place {
            // Another world or instance (or VRChat closed): the play space goes back while the avatar respawns.
            self.place = m.place.map(str::to_string);
            self.say_put_back("another world, or VRChat closed");
            self.put_back(play);
        }
        let seen = (m.settings.enabled, m.settings.strength);
        if self.seen.is_some_and(|s| s != seen) {
            self.changed = Some(now);
        }
        self.seen = Some(seen);
        self.watch_others(play, now);
        match self.advance(play, m, stick, frame, dt, now) {
            Ok(()) => {
                if self.run.is_none() {
                    self.run = Some(now);
                    let carried = self.carried.as_ref().map_or(0.0, |c| length(&c.offset));
                    self.log.say(format!("walking: {carried:.2} m from where the carry began"));
                }
            }
            Err(why) => {
                if let Some(start) = self.run.take() {
                    let carried = self.carried.as_ref().map_or(0.0, |c| length(&c.offset));
                    let lasted = now.saturating_duration_since(start).as_secs_f32();
                    self.log.say(format!("stopped after {lasted:.2} s, {carried:.2} m from where the carry began: {why}"));
                }
            }
        }
    }

    /// A step further, or why not.
    fn advance(
        &mut self,
        play: &mut dyn PlaySpace,
        m: &Moment,
        stick: impl FnOnce() -> Option<[f32; 2]>,
        frame: impl FnOnce() -> Option<Matrix34>,
        dt: f32,
        now: Instant,
    ) -> Result<(), &'static str> {
        let held = self.held_back(m, now);
        let stick = if held.is_none() && m.input_read { stick() } else { None };
        let pushed = stick.is_some_and(|s| push(s) > 0.0);
        let carries = self.gate.carries(now, pushed, m.motion.as_ref(), std::mem::take(&mut self.boost));
        if let Some(why) = held {
            return Err(why);
        }
        if !m.input_read {
            return Err("the controllers could not be read");
        }
        let Some(stick) = stick else { return Err("the stick is not active") };
        if !pushed {
            return Err("the stick was let go");
        }
        if !carries {
            return Err("VRChat does not walk the avatar");
        }
        if dt <= 0.0 {
            return Err("the first frame");
        }
        let Some(frame) = frame() else { return Err("forward is not tracked") };
        let dir = direction(stick, &frame);
        if dir == [0.0; 3] {
            return Err("no way forward");
        }
        let speed = RUN_SPEED * m.settings.strength.clamp(0.0, 1.0) * WalkBoostSettings::MAX_EXTRA;
        let step = dir.map(|d| d * speed * dt);
        if !step.iter().all(|s| s.is_finite()) {
            return Err("a step that is not a number");
        }
        if self.carried.is_none() {
            // No preview of the boost's shows now: the working copy reverts to SteamVR's own setup or to another
            // application's preview. The walk starts from what everyone sees, whichever that is.
            play.revert();
            let Some(own) = play.read() else { return Err("SteamVR gave no setup") };
            let live = if self.reading == Reading::Follows { play.live_standing() } else { None };
            let base = live.map_or_else(|| own.clone(), |live| own.at_live(&live));
            let over_other = self.other_showing || !same_place(&own.standing, &base.standing);
            if over_other {
                self.log.say(format!("the walk starts on another application's play space, {:.2} m from SteamVR's own setup", distance(&own.standing, &base.standing)));
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
            self.log.say(format!("back where the carry began, {walked:.2} m ago: {why}"));
        }
    }

    /// Puts the play space back where it was.
    pub fn reset(&mut self, steamvr: &vr::Client) {
        match steamvr.chaperone_setup() {
            Ok(mut setup) => {
                self.say_put_back("the overlay ends");
                self.put_back(&mut setup);
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

    /// One of SteamVR's play-space events at `now`, with `play`. What a person does ends the carry at once, the play
    /// space back to SteamVR's own setup (theirs now): a recenter, a seated reset (not within [`OWN_EVENTS`] of the
    /// boost's own preview: the boost moves the seated zero too, and that may be its echo), another tracking universe,
    /// room setup (which also holds the boost until it is saved). The rest, raised for every preview the boost shows
    /// and for other applications, changes nothing: the boost never looks at SteamVR's setup while its carry shows
    /// (reverting the working copy to look put the play space back where it started, the person's position reset,
    /// until the next push showed the carry again).
    pub fn play_space_event_in(&mut self, play: &mut dyn PlaySpace, event: PlaySpaceEvent, now: Instant) {
        let own_echo = self.shown_at.is_some_and(|t| now.saturating_duration_since(t) < OWN_EVENTS);
        match event {
            PlaySpaceEvent::StandingReset => self.let_go(play, "a recenter"),
            PlaySpaceEvent::SeatedReset if !own_echo => self.let_go(play, "a seated reset"),
            PlaySpaceEvent::UniverseChanged { from, to } if from != to => self.let_go(play, "another tracking universe"),
            PlaySpaceEvent::RoomSetupStarting => {
                self.let_go(play, "room setup");
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
    fn carries_only_while_vrchat_walks() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let speed = |speed: f32| Motion { in_station: false, speed };
        // Nothing from VRChat: the stick alone.
        let mut g = Gate::default();
        assert!(g.carries(at(0), true, None, 0.0) && !g.carries(at(10), false, None, 0.0));
        // A push starts carrying once VRChat moves the avatar itself.
        let mut g = Gate::default();
        assert!(!g.carries(at(0), true, Some(&speed(0.0)), 0.0));
        assert!(g.carries(at(10), true, Some(&speed(3.0)), 0.0));
        // Carrying, VRChat's speed counts the boost's own drag: 4.05 m/s with 4 of boost is VRChat standing still.
        assert!(g.carries(at(100), true, Some(&speed(4.05)), 4.0), "its reports lag the boost at first");
        assert!(g.carries(at(700), true, Some(&speed(4.05)), 4.0), "a moment below walking is not a stop");
        assert!(!g.carries(at(1100), true, Some(&speed(4.05)), 4.0), "the Action Menu took the stick: the boost stops");
        // VRChat still reports the boost's drag a moment: that does not start it again (it did, the next frame, so the
        // boost never stopped and went in fits).
        assert!(!g.carries(at(1110), true, Some(&speed(4.05)), 0.0), "the boost's own drag, reported late");
        assert!(!g.carries(at(1400), true, Some(&speed(2.0)), 0.0));
        assert!(!g.carries(at(1650), true, Some(&speed(0.05)), 0.0), "VRChat stands the avatar still: still stopped");
        assert!(g.carries(at(1700), true, Some(&speed(3.0)), 0.0), "VRChat walks it again: carrying again");
        // Letting go of the stick forgets the stop: the next push starts at once.
        let mut g = Gate::default();
        assert!(g.carries(at(0), true, Some(&speed(3.0)), 0.0));
        assert!(g.carries(at(1000), true, Some(&speed(4.05)), 4.0));
        assert!(!g.carries(at(1400), true, Some(&speed(4.05)), 4.0));
        assert!(!g.carries(at(1410), false, Some(&speed(4.05)), 0.0));
        assert!(g.carries(at(1420), true, Some(&speed(3.0)), 0.0));
        // Walking: VRChat's own 4 m/s on top of the boost's 4.
        let mut g = Gate::default();
        assert!(g.carries(at(0), true, Some(&speed(3.0)), 0.0));
        assert!(g.carries(at(2000), true, Some(&speed(8.0)), 4.0));
        // In a station, never; a speed that is not a number starts nothing.
        assert!(!g.carries(at(2010), true, Some(&Motion { in_station: true, speed: 3.0 }), 4.0));
        assert!(!Gate::default().carries(at(0), true, Some(&speed(f32::NAN)), 0.0));
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
        Moment { settings, walking_allowed: true, place, held: false, away: false, motion: None, input_read: true }
    }

    fn boost() -> WalkBoost {
        WalkBoost::new(Some(Walk { set: 1, left: 2, right: 3 }), None)
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
        let still = Moment { motion: Some(Motion { in_station: false, speed: 0.0 }), ..moment(&s, None) };
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
                let stick = (push_for > 0).then_some([0.0, 1.0]);
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
                let speed = (noise(&mut state) % 8) as f32;
                let m = Moment { place: Some(place), motion: Some(Motion { in_station: false, speed }), ..moment(&s, None) };
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
}
