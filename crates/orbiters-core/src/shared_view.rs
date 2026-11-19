//! The shared 3D view's protocol, version 1 (contracts `Docs/n4n0-shared-view.md`): what a sharer's Orbiters - N4N0
//! sends about its small view and what a viewer's applies. Both sides' rules live here, apart from any transport (the
//! server's event stream in stage 1, a data channel later): a viewer applies only the share it accepted, only from a
//! revision above the last it applied, only values that are finite and in range; a sharer numbers its states and sends
//! them no faster than [`SEND_EVERY_MS`].

use serde::{Deserialize, Serialize};

/// The protocol's version, in every body (`"v": 1`).
pub const VERSION: u32 = 1;
/// How much of a capture a view shows around its middle, in the capture's metres (as the small view's own range).
pub const REACH_RANGE: (f32, f32) = (0.5, 150.0);
/// The farthest a view's middle may be said to be moved from the capture's start, in the capture's metres (the viewer
/// clamps further to its capture's own extent).
pub const MOVED_MOST: f32 = 5000.0;
/// A sharer sends its state at most this often while it changes (the server allows 10 a second).
pub const SEND_EVERY_MS: u64 = 200;
/// Changes smaller than these are not sent: a fraction of the reach, and of a capture metre.
const REACH_STEP: f32 = 0.002;
const MOVED_STEP: f32 = 0.01;

/// Who holds (moves) the view. Version 1: only the sharer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Holder {
    #[default]
    Sharer,
    Viewer,
}

/// Where each person's copy stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Before each person, placed by their own app (stage 1).
    #[default]
    BeforeViewer,
    /// At `pose` in the share's shared frame (stage 2).
    Anchored,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub mode: Mode,
    /// Which shared frame a pose is in: 0 = none.
    #[serde(default)]
    pub anchor_rev: u32,
}

/// The view in the share's shared frame (stage 2): its middle and full orientation, world metres, a point-mapping
/// transform, 3×3 row-major, right-handed, Y up (OpenVR's convention). Version 1 neither sends nor applies it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pose {
    pub frame: PoseWord,
    pub units: PoseWord,
    pub position: [f32; 3],
    pub rotation: [[f32; 3]; 3],
}

/// The two fixed words a pose carries (`"anchor"`, `"world_m"`), as an enum so a pose of another frame does not parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoseWord {
    Anchor,
    WorldM,
}

/// The view's state as the sharer's app (its authority) says it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    /// Strictly increasing within a share, from 1.
    pub rev: u64,
    /// The ownership term the state was written in ([`Arbiter`]): from 1, one more at each change of holder. A state of
    /// an older term than the last one seen is a stale owner's and is not applied.
    #[serde(default = "one")]
    pub term: u64,
    #[serde(default)]
    pub holder: Holder,
    /// While the viewer holds it (stage 3): how long its hold lasts unless renewed. `None` while the sharer holds it.
    #[serde(default)]
    pub lease: Option<Lease>,
    #[serde(default)]
    pub placement: Placement,
    #[serde(default)]
    pub pose: Option<Pose>,
    /// Its zoom: how many capture metres it shows around its middle.
    pub reach: f32,
    /// How far its middle is moved from the capture's start view along its ground (X, Z), capture metres.
    pub moved: [f32; 2],
}

fn one() -> u64 {
    1
}

/// A hold of the view by someone else than the sharer, as the arbiter granted it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub holder: Holder,
    pub term: u64,
    /// Milliseconds left when the state was written.
    pub expires_in_ms: u64,
}

impl ViewState {
    /// Before each viewer (stage 1), held by the sharer in the first term, showing `reach` around a middle `moved` from
    /// the start.
    pub fn before_viewer(rev: u64, reach: f32, moved: [f32; 2]) -> Self {
        Self { rev, term: 1, holder: Holder::Sharer, lease: None, placement: Placement::default(), pose: None, reach, moved }
    }

    /// The state as a viewer may apply it: `None` when a value is not finite; the reach and the move kept in range.
    /// Version 1 places its copy itself: a pose is dropped.
    pub fn checked(&self) -> Option<Self> {
        if !self.reach.is_finite() || !self.moved.iter().all(|m| m.is_finite()) {
            return None;
        }
        let far = self.moved[0].hypot(self.moved[1]);
        let moved = if far > MOVED_MOST { [self.moved[0] * MOVED_MOST / far, self.moved[1] * MOVED_MOST / far] } else { self.moved };
        Some(Self { reach: self.reach.clamp(REACH_RANGE.0, REACH_RANGE.1), moved, pose: None, ..self.clone() })
    }
}

/// The capture a share shows: its kind (`"capture"`), its listing id, the upload's key (immutable: a replaced capture is
/// another revision) and its world's name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRef {
    #[serde(rename = "type")]
    pub kind: String,
    pub id: u64,
    pub revision: u64,
    #[serde(default)]
    pub title: String,
}

/// Someone who shares: their calls contact id and the display name of their verified link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct From {
    pub id: String,
    pub name: String,
}

/// `share_offer`, as the viewer's event stream brings it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Offer {
    pub v: u32,
    pub share: String,
    pub from: From,
    pub capture: CaptureRef,
    #[serde(default = "thirty")]
    pub expires_in: u64,
}

fn thirty() -> u64 {
    30
}

impl Offer {
    /// The offer as this app takes it: version 1, a share id and a contact id of the server's shape, a capture.
    pub fn read(v: &serde_json::Value) -> Option<Self> {
        let offer: Self = serde_json::from_value(v.clone()).ok()?;
        (offer.v == VERSION && share_id(&offer.share) && contact_id(&offer.from.id) && offer.capture.kind == "capture" && offer.capture.id > 0).then_some(offer)
    }
}

/// The server's share ids: `s_` and 22 URL-safe characters.
pub fn share_id(id: &str) -> bool {
    id.len() == 24 && id.starts_with("s_") && id[2..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The calls' contact ids: `c_` and 22 URL-safe characters.
pub fn contact_id(id: &str) -> bool {
    id.len() == 24 && id.starts_with("c_") && id[2..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// How a share ended, as `share_ended` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Ended,
    Left,
    Timeout,
    Gone,
    Moved,
    Unfriended,
    #[serde(other)]
    Other,
}

/// What a viewer does with a state message.
#[derive(Clone, Debug, PartialEq)]
pub enum Applied {
    /// Apply it.
    Apply(ViewState),
    /// Another share's (or no share is being watched).
    NotOurs,
    /// Not above the last revision applied (late or replayed), or of an older ownership term than one seen.
    Stale,
    /// Not finite: dropped whole.
    Bad,
}

/// The viewer's side of one share it accepted.
#[derive(Clone, Debug, PartialEq)]
pub struct Watching {
    pub share: String,
    pub from: From,
    pub capture: CaptureRef,
    /// The last revision applied, and the newest ownership term seen.
    pub applied: Option<u64>,
    pub term: u64,
}

impl Watching {
    pub fn new(offer: &Offer) -> Self {
        Self { share: offer.share.clone(), from: offer.from.clone(), capture: offer.capture.clone(), applied: None, term: 1 }
    }

    /// A state for `share`: applied only when it is this share's and newer than what was applied.
    pub fn take(&mut self, share: &str, state: &ViewState) -> Applied {
        if share != self.share {
            return Applied::NotOurs;
        }
        if self.applied.is_some_and(|last| state.rev <= last) || state.rev == 0 || state.term < self.term {
            return Applied::Stale;
        }
        let Some(checked) = state.checked() else { return Applied::Bad };
        self.applied = Some(state.rev);
        self.term = state.term;
        Applied::Apply(checked)
    }
}

/// How long a hold granted to someone else than the sharer lasts unless its holder renews it (with its moves).
pub const LEASE_MS: u64 = 2000;

/// What the arbiter answers a grab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grab {
    /// `holder` holds it in `term` (with a lease when it is not the sharer).
    Granted { term: u64, holder: Holder, lease: Option<Lease> },
    /// Someone else holds it, or the grab named an older term than the current one: nothing changes.
    Refused { term: u64, holder: Holder },
}

/// Who holds the view, decided in one place: the sharer's app, which takes every grab, renewal and release one after
/// the other (stage 3; version 1 never hands it over, but its states already carry the term). Each change of holder
/// starts a new term; a word naming an older term is a stale owner's and is refused. The sharer's own grab is taken
/// at once, before any grab of the viewer's that comes after it, so simultaneous grabs go to the sharer. A hold of the
/// viewer's ends by itself when not renewed within [`LEASE_MS`]; while the viewer holds it, the sharer's hand cannot
/// take it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arbiter {
    pub term: u64,
    pub holder: Holder,
    until_ms: u64,
    /// The sharer's own hand has it now.
    sharer_holding: bool,
}

impl Default for Arbiter {
    fn default() -> Self {
        Self { term: 1, holder: Holder::Sharer, until_ms: 0, sharer_holding: false }
    }
}

impl Arbiter {
    fn lease(&self, now_ms: u64) -> Option<Lease> {
        (self.holder != Holder::Sharer).then(|| Lease { holder: self.holder, term: self.term, expires_in_ms: self.until_ms.saturating_sub(now_ms) })
    }

    /// A lease not renewed in time ends: the view is the sharer's again, in a new term.
    pub fn expire(&mut self, now_ms: u64) -> bool {
        if self.holder != Holder::Sharer && now_ms >= self.until_ms {
            self.holder = Holder::Sharer;
            self.term += 1;
            return true;
        }
        false
    }

    /// `by` asks to hold it, having last seen term `seen`.
    pub fn grab(&mut self, by: Holder, seen: u64, now_ms: u64) -> Grab {
        self.expire(now_ms);
        let refused = Grab::Refused { term: self.term, holder: self.holder };
        if seen < self.term && self.holder != by {
            return refused;
        }
        match (by, self.holder) {
            (Holder::Sharer, Holder::Sharer) => self.sharer_holding = true,
            (Holder::Sharer, _) => return refused,
            (_, Holder::Sharer) if self.sharer_holding => return refused,
            (_, Holder::Sharer) => {
                self.holder = by;
                self.term += 1;
                self.until_ms = now_ms + LEASE_MS;
            }
            (_, holder) if holder == by => self.until_ms = now_ms + LEASE_MS,
            _ => return refused,
        }
        Grab::Granted { term: self.term, holder: self.holder, lease: self.lease(now_ms) }
    }

    /// The holder's move in term `term` renews its lease; false (the move is not taken) when that term is over.
    pub fn renew(&mut self, by: Holder, term: u64, now_ms: u64) -> bool {
        self.expire(now_ms);
        if term != self.term || by != self.holder {
            return false;
        }
        if by != Holder::Sharer {
            self.until_ms = now_ms + LEASE_MS;
        }
        true
    }

    /// `by` lets go in term `term`: the view is the sharer's again (a new term when someone else held it).
    pub fn release(&mut self, by: Holder, term: u64) -> bool {
        if term != self.term || by != self.holder {
            return false;
        }
        if by == Holder::Sharer {
            self.sharer_holding = false;
        } else {
            self.holder = Holder::Sharer;
            self.term += 1;
        }
        true
    }
}

/// The sharer's side: numbers its states and says when one is to go.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sending {
    /// The last revision given.
    pub rev: u64,
    /// The last view sent (reach, moved) and when (ms).
    sent: Option<(f32, [f32; 2], u64)>,
}

impl Sending {
    /// The first state, for the offer.
    pub fn first(&mut self, reach: f32, moved: [f32; 2], now_ms: u64) -> ViewState {
        self.rev = 1;
        self.sent = Some((reach, moved, now_ms));
        ViewState::before_viewer(1, reach, moved)
    }

    /// The state to send now for the view as it is (`None`: unchanged, or the last went less than [`SEND_EVERY_MS`]
    /// ago; asked again, a change held back goes once that time is over).
    pub fn next(&mut self, reach: f32, moved: [f32; 2], now_ms: u64) -> Option<ViewState> {
        if !reach.is_finite() || !moved.iter().all(|m| m.is_finite()) {
            return None;
        }
        if let Some((r, m, at)) = self.sent {
            let same = (reach - r).abs() <= r.abs() * REACH_STEP && (moved[0] - m[0]).abs() < MOVED_STEP && (moved[1] - m[1]).abs() < MOVED_STEP;
            if same || now_ms.saturating_sub(at) < SEND_EVERY_MS {
                return None;
            }
        }
        self.rev += 1;
        self.sent = Some((reach, moved, now_ms));
        Some(ViewState::before_viewer(self.rev, reach, moved))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn offer(share: &str) -> Offer {
        Offer::read(&json!({
            "v": 1, "share": share, "from": { "id": "c_AAAAAAAAAAAAAAAAAAAAAA", "name": "Alice" },
            "capture": { "type": "capture", "id": 412, "revision": 9, "title": "The Black Cat" }, "expires_in": 30
        }))
        .expect("a well-formed offer")
    }

    const SHARE: &str = "s_BBBBBBBBBBBBBBBBBBBBBB";

    #[test]
    fn states_apply_in_order_and_late_or_replayed_ones_never() {
        let mut w = Watching::new(&offer(SHARE));
        let s = |rev, reach| ViewState::before_viewer(rev, reach, [0.0, 0.0]);
        assert_eq!(w.take(SHARE, &s(2, 10.0)), Applied::Apply(s(2, 10.0)));
        // Revision 1 came late: it would undo the zoom of 2.
        assert_eq!(w.take(SHARE, &s(1, 20.0)), Applied::Stale);
        // The same message again (a stream that reopened, a replay).
        assert_eq!(w.take(SHARE, &s(2, 10.0)), Applied::Stale);
        assert_eq!(w.take(SHARE, &s(5, 30.0)), Applied::Apply(s(5, 30.0)));
        assert_eq!(w.take(SHARE, &s(0, 30.0)), Applied::Stale, "revisions count from 1");
        assert_eq!(w.applied, Some(5));
    }

    #[test]
    fn another_share_s_states_do_nothing() {
        let mut w = Watching::new(&offer(SHARE));
        let state = ViewState::before_viewer(1, 10.0, [0.0, 0.0]);
        assert_eq!(w.take("s_CCCCCCCCCCCCCCCCCCCCCC", &state), Applied::NotOurs);
        assert_eq!(w.applied, None, "nothing applied: the next state of ours still counts");
        assert!(matches!(w.take(SHARE, &state), Applied::Apply(_)));
    }

    #[test]
    fn values_are_kept_in_range_and_bad_ones_drop_the_whole_state() {
        let mut w = Watching::new(&offer(SHARE));
        let Applied::Apply(s) = w.take(SHARE, &ViewState::before_viewer(1, 1e9, [9e6, 0.0])) else { panic!() };
        assert_eq!(s.reach, REACH_RANGE.1);
        assert!((s.moved[0] - MOVED_MOST).abs() < 1e-3);
        assert_eq!(w.take(SHARE, &ViewState::before_viewer(2, f32::NAN, [0.0, 0.0])), Applied::Bad);
        assert_eq!(w.take(SHARE, &ViewState::before_viewer(3, 1.0, [f32::INFINITY, 0.0])), Applied::Bad);
        // A pose is not version 1's to apply: the copy stays before the viewer.
        let mut anchored = ViewState::before_viewer(4, 2.0, [0.0, 0.0]);
        anchored.pose = Some(Pose { frame: PoseWord::Anchor, units: PoseWord::WorldM, position: [0.0, 9.0, 0.0], rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] });
        let Applied::Apply(s) = w.take(SHARE, &anchored) else { panic!() };
        assert_eq!(s.pose, None);
    }

    #[test]
    fn the_wire_form_is_the_contract_s() {
        let state = ViewState::before_viewer(7, 12.5, [0.8, -2.0]);
        assert_eq!(
            serde_json::to_value(&state).unwrap(),
            json!({ "rev": 7, "term": 1, "holder": "sharer", "lease": null, "placement": { "mode": "before_viewer", "anchor_rev": 0 }, "pose": null, "reach": 12.5, "moved": [0.8, -2.0] })
        );
        // A later version's fields that this one does not know are read past; its own come through.
        let later: ViewState = serde_json::from_value(json!({
            "rev": 3, "holder": "viewer", "placement": { "mode": "anchored", "anchor_rev": 2 },
            "pose": { "frame": "anchor", "units": "world_m", "position": [1, 2, 3], "rotation": [[1, 0, 0], [0, 1, 0], [0, 0, 1]] },
            "reach": 4, "moved": [0, 0], "lease_ms": 2000
        }))
        .unwrap();
        assert_eq!((later.holder, later.placement.mode, later.placement.anchor_rev), (Holder::Viewer, Mode::Anchored, 2));
        assert!(serde_json::from_value::<ViewState>(json!({ "rev": 1, "reach": 1, "moved": [0, 0], "pose": { "frame": "head", "units": "world_m", "position": [0, 0, 0], "rotation": [[1, 0, 0], [0, 1, 0], [0, 0, 1]] } })).is_err());
    }

    #[test]
    fn offers_of_another_version_or_shape_are_not_taken() {
        let good = json!({ "v": 1, "share": SHARE, "from": { "id": "c_AAAAAAAAAAAAAAAAAAAAAA", "name": "Alice" }, "capture": { "type": "capture", "id": 4, "revision": 1 } });
        assert!(Offer::read(&good).is_some());
        let mut v2 = good.clone();
        v2["v"] = json!(2);
        assert!(Offer::read(&v2).is_none());
        let mut odd = good.clone();
        odd["share"] = json!("../calls/k_1");
        assert!(Offer::read(&odd).is_none());
        let mut who = good.clone();
        who["from"]["id"] = json!("usr_1234");
        assert!(Offer::read(&who).is_none(), "contact ids only, never a VRChat id");
        let mut picture = good;
        picture["capture"]["type"] = json!("picture");
        assert!(Offer::read(&picture).is_none());
    }

    #[test]
    fn the_sharer_numbers_its_states_and_paces_them() {
        let mut s = Sending::default();
        assert_eq!(s.first(10.0, [0.0, 0.0], 1_000).rev, 1);
        assert_eq!(s.next(10.0, [0.0, 0.0], 5_000), None, "unchanged");
        assert_eq!(s.next(9.0, [0.0, 0.0], 1_100), None, "too soon after the last");
        let second = s.next(9.0, [0.0, 0.0], 1_200).expect("its time came");
        assert_eq!((second.rev, second.reach), (2, 9.0));
        let third = s.next(9.0, [1.0, 0.0], 1_400).expect("a pan");
        assert_eq!(third.rev, 3);
        assert_eq!(s.next(f32::NAN, [0.0, 0.0], 9_000), None);
    }

    #[test]
    fn a_state_of_an_older_ownership_term_is_a_stale_owner_s() {
        let mut w = Watching::new(&offer(SHARE));
        let mut later = ViewState::before_viewer(2, 4.0, [0.0, 0.0]);
        later.term = 3;
        assert!(matches!(w.take(SHARE, &later), Applied::Apply(_)));
        let mut old_owner = ViewState::before_viewer(9, 8.0, [0.0, 0.0]);
        old_owner.term = 2;
        assert_eq!(w.take(SHARE, &old_owner), Applied::Stale, "a newer revision of an older term is still stale");
    }

    #[test]
    fn the_arbiter_serialises_grabs_and_refuses_stale_terms() {
        let mut a = Arbiter::default();
        // The sharer's own grab comes first: the viewer's, right after, is refused (simultaneous grabs go to the sharer).
        assert!(matches!(a.grab(Holder::Sharer, 1, 0), Grab::Granted { term: 1, holder: Holder::Sharer, lease: None }));
        assert_eq!(a.grab(Holder::Viewer, 1, 10), Grab::Refused { term: 1, holder: Holder::Sharer });
        assert!(a.release(Holder::Sharer, 1));
        // Free now: the viewer takes it, in a new term, with a lease.
        let Grab::Granted { term, holder, lease } = a.grab(Holder::Viewer, 1, 100) else { panic!() };
        assert_eq!((term, holder, lease.map(|l| l.expires_in_ms)), (2, Holder::Viewer, Some(LEASE_MS)));
        // While the viewer holds it, the sharer's hand cannot take it.
        assert_eq!(a.grab(Holder::Sharer, 2, 200), Grab::Refused { term: 2, holder: Holder::Viewer });
        // Its moves renew it; a word of term 1 (before the hand-over) changes nothing.
        assert!(a.renew(Holder::Viewer, 2, 1_500));
        assert!(!a.renew(Holder::Viewer, 1, 1_600));
        assert!(!a.release(Holder::Viewer, 1));
        // Not renewed for LEASE_MS: the sharer's again, in a new term; the old holder's late moves are refused.
        assert!(a.expire(1_500 + LEASE_MS));
        assert_eq!((a.term, a.holder), (3, Holder::Sharer));
        assert!(!a.renew(Holder::Viewer, 2, 1_500 + LEASE_MS + 1));
        // A grab naming an older term than the current one is refused.
        assert_eq!(a.grab(Holder::Viewer, 2, 9_000), Grab::Refused { term: 3, holder: Holder::Sharer });
        assert!(matches!(a.grab(Holder::Viewer, 3, 9_000), Grab::Granted { term: 4, .. }));
        assert!(a.release(Holder::Viewer, 4));
        assert_eq!((a.term, a.holder), (5, Holder::Sharer));
    }
}
