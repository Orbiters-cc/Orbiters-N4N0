//! Lines followed from look to look, so that a translation stays put: a line seen again (same text, about the same
//! place in the world) refines its place a little at a time; a line not found again where the eye could see it goes; a
//! line out of view stays where it was for a while (it is anchored to the world: looking back finds it there).
//!
//! Places are in the anchor space of [`crate::motion`], which moves with VRChat's world as the person walks and turns with
//! the sticks. What that estimate gets wrong shows when a look finds followed lines again somewhere else: all lines then
//! move by what those re-readings agree on ([`Tracker::update`]'s re-anchoring), which keeps a translation on its sign
//! after walking with a stick.
//!
//! A line whose translation is shown is hidden from the text reader by that translation (our overlays are in the eye's
//! picture, and are masked out of it). It is then kept while what surrounds it looks the same ([`Track::context`]); when
//! that changes (the sign moved, someone walked in front, a chat bubble changed) in two looks over a second apart, it
//! goes, and the next look reads what is there now.
//!
//! When a line goes (Enzo, 2026-11-13: "the translated words should time out after it's not visible on the screen for
//! long enough"), all by time, so that the reading's pace (three times slower while VRChat is short of processor time)
//! does not change it:
//! - off the screen ([`Seen::Away`]) for [`OUT_OF_VIEW_S`];
//! - on screen but its text not found where it should be ([`Seen::Open`] in [`MISSES`] looks in a row, or its
//!   surroundings changed while covered), and unconfirmed for [`MISS_S`];
//! - on screen at the edge of the view, outside the part read, unconfirmed for [`UNCONFIRMED_S`].
//!
//! A line looked at is confirmed by each look (read again, or covered with its surroundings unchanged): it never goes.

use crate::geometry::{self, add, normalized, scale, sub, V3};
use crate::place::Placed;

/// Looks in a row a line may be missed where it should be seen before it can go.
pub const MISSES: u32 = 2;
/// And how long it must have gone unconfirmed then, seconds (two looks at the fastest pace are 0.7 s).
pub const MISS_S: f64 = 1.2;
/// After the sticks moved the player since a line was confirmed, its cover may be off its text until a look finds the
/// text again and re-anchors: it may not be found where it should be for this long before it goes (the sandbox: a
/// cover moved off its sign by a short stick turn went after 1.2 s, before the sign was read again).
pub const MOVED_MISS_S: f64 = 3.0;
/// Seconds a line off the screen stays (looking away and back within this finds it where it was).
pub const OUT_OF_VIEW_S: f64 = 6.0;
/// Seconds a line on screen but outside the part read stays unconfirmed.
pub const UNCONFIRMED_S: f64 = 20.0;
/// How much of a new measurement a known line takes.
const FOLLOW: f32 = 0.35;
/// Seconds a text found to move with the head (VRChat's notices, another app's overlay) is left alone.
pub const HEAD_LOCKED_S: f64 = 60.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colours {
    pub background: [u8; 3],
    pub text: [u8; 3],
}

/// A line read in one look and placed in the room.
#[derive(Clone, Debug)]
pub struct Sighting {
    pub text: String,
    pub confidence: f32,
    pub placed: Placed,
    pub colours: Colours,
    /// Grey samples of a ring around the line, to tell later whether its surroundings changed.
    pub context: Vec<f32>,
    /// Its middle in the eye's own space (where it is before the eye, whichever way the head looks).
    pub in_eye: V3,
}

/// What a look could tell about a line already followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// Its place was in the part of the eye read, uncovered: it should have been found if still there.
    Open,
    /// Under its own translation: only its surroundings could be compared.
    Covered { changed: bool },
    /// On screen, but outside the part read (or under another of our overlays): nothing to tell.
    Edge,
    /// Off the screen: behind or beside the eye.
    Away,
}

/// What a look's following did beyond the lines themselves.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Followed {
    /// All lines moved by this (anchor space): the world was not where the motion estimate had it. After a turn about
    /// the head (`turned`), what was left once that turn was undone.
    pub moved: Option<V3>,
    /// All lines first turned by this, radians about the vertical through `about` (the head at the look): a stick turn
    /// misjudged. A turn error moves each line by its own amount (more the further it is, and the other way behind):
    /// one shift for all, as before 0.3.5-dev.54, set the lines looked at right and left the others off.
    pub turned: Option<f32>,
    pub about: V3,
    /// Lines found again that agreed on it.
    pub agreeing: usize,
    /// It was only measured ([`Tracker::hold_anchors`]): the lines did not move.
    pub held: bool,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: u64,
    /// The text as read (the best read so far), and its key for the translation cache.
    pub text: String,
    pub key: String,
    pub confidence: f32,
    pub placed: Placed,
    pub colours: Colours,
    pub context: Vec<f32>,
    pub first: f64,
    /// Last confirmed: read again, or covered with its surroundings unchanged.
    pub last_seen: f64,
    /// Last on screen.
    pub in_view: f64,
    /// [`Tracker::moved`] when it was last confirmed.
    pub moved_at: [f32; 2],
    /// Its middle in the eye's space when last read ([`Sighting::in_eye`]).
    pub in_eye: V3,
    pub sightings: u32,
    pub misses: u32,
}

impl Track {
    /// Seen enough to be drawn: twice, or once with its distance measured by both eyes.
    pub fn settled(&self) -> bool {
        self.sightings >= 2 || self.placed.measured
    }
}

/// A line's text as the cache knows it: spaces collapsed, ends trimmed, full-width forms as they are.
pub fn key(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How alike two lines read: 1 for the same letters, from the longest common subsequence.
pub fn alike(a: &str, b: &str) -> f32 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().filter(|c| !c.is_whitespace()).collect(), b.chars().filter(|c| !c.is_whitespace()).collect());
    if a.is_empty() || b.is_empty() {
        return if a == b { 1.0 } else { 0.0 };
    }
    let mut prev = vec![0u16; b.len() + 1];
    for &x in &a {
        let mut cur = vec![0u16; b.len() + 1];
        for (j, &y) in b.iter().enumerate() {
            cur[j + 1] = if x == y { prev[j] + 1 } else { prev[j + 1].max(cur[j]) };
        }
        prev = cur;
    }
    2.0 * prev[b.len()] as f32 / (a.len() + b.len()) as f32
}

/// How far two placed lines may be and still be one line: half the line's width, and more further away (a distance
/// guessed from one eye is less sure).
fn tolerance(a: &Placed, b: &Placed) -> f32 {
    (0.5 * a.width.max(b.width)).max(0.06 + 0.06 * a.distance.max(b.distance))
}

/// A blend of two placements: `t` of the way from `a` to `b`.
pub fn blend(a: &Placed, b: &Placed, t: f32) -> Placed {
    let c = geometry::lerp(a.centre(), b.centre(), t);
    let z = normalized(geometry::lerp(geometry::axis(&a.pose, 2), geometry::axis(&b.pose, 2), t));
    let x0 = geometry::lerp(geometry::axis(&a.pose, 0), geometry::axis(&b.pose, 0), t);
    let x = normalized(sub(x0, scale(z, geometry::dot(x0, z))));
    let y = geometry::cross(z, x);
    Placed {
        pose: geometry::from_axes(x, y, z, c),
        width: a.width + (b.width - a.width) * t,
        height: a.height + (b.height - a.height) * t,
        measured: a.measured || b.measured,
        distance: a.distance + (b.distance - a.distance) * t,
    }
}

#[derive(Default)]
pub struct Tracker {
    pub tracks: Vec<Track>,
    next: u64,
    /// How far the sticks moved the player so far, all told (metres walked, radians turned: [`crate::motion`]), set by
    /// the caller before each look. A line found again elsewhere moves all lines only as far as that motion since it was
    /// confirmed can have been misjudged: standing still, the same text a metre away is another sign.
    pub moved: [f32; 2],
    /// Re-anchoring is measured but not done (a development switch, to measure how far lines slide without it).
    pub hold_anchors: bool,
    /// Texts found to move with the head, and until when they are left alone.
    pub head_locked: Vec<(String, f64)>,
    /// The sticks moved the player during this look's picture or just before: VRChat shows its motion a little after
    /// the sticks are read, so where lines are found then says nothing reliable of where the world is (in the sandbox a
    /// look during a turn moved every line 24 cm one way, and the next look 57 cm back). No re-anchoring then.
    pub moving: bool,
    /// The head at this look (anchor space), set by the caller: a turn misjudged is undone about it.
    pub head: Option<V3>,
}

/// The signed angle about up (+Y) from `a` to `b`, seen from `about`, in the horizontal plane; `None` too near it.
fn yaw_between(about: V3, a: V3, b: V3) -> Option<f32> {
    let (a, b) = (sub(a, about), sub(b, about));
    let (la, lb) = ((a[0] * a[0] + a[2] * a[2]).sqrt(), (b[0] * b[0] + b[2] * b[2]).sqrt());
    (la > 0.3 && lb > 0.3).then(|| (a[2] * b[0] - a[0] * b[2]).atan2(a[0] * b[0] + a[2] * b[2]))
}

impl Tracker {
    /// One look's lines. `seen` says, for each line already followed, what this look could tell about it.
    pub fn update(&mut self, now: f64, sightings: Vec<Sighting>, seen: &dyn Fn(&Track) -> Seen) -> Followed {
        // Text that moves with the head is not on the world (VRChat's notices and loading screen, other apps' overlays):
        // its translation, anchored to the world, stayed after the notice went, covering it so that nothing ever told it
        // was gone (the sandbox, 2026-11-13: "You have joined an Invite instance" shown in French for minutes). Read again
        // at the same place before the eye after the head moved, while its anchor is elsewhere, it is left alone.
        self.head_locked.retain(|(_, until)| *until > now);
        //
        // Only a move across the view counts: a far sign's distance from both eyes varies by centimetres a pixel, so
        // standing still it is read again deeper or nearer at the same place before the eye. Before 0.3.5-dev.54 that
        // made a sign 4 m away "move with the head" when read 18 cm deeper, and it was not followed for a minute.
        let head = self.head;
        let across = |a: V3, b: V3| {
            let d = sub(b, a);
            match head {
                Some(h) => {
                    let ray = normalized(sub(b, h));
                    geometry::norm(sub(d, scale(ray, geometry::dot(d, ray))))
                }
                None => geometry::norm(d),
            }
        };
        let mut sightings = sightings;
        for s in &sightings {
            let k = key(&s.text);
            let locked = self.tracks.iter().any(|t| {
                t.key == k && geometry::norm(sub(t.in_eye, s.in_eye)) < 0.03 + 0.02 * s.placed.distance && across(t.placed.centre(), s.placed.centre()) > 0.04 + 0.03 * s.placed.distance
            });
            if locked && !self.head_locked.iter().any(|(h, _)| *h == k) {
                self.head_locked.push((k, now + HEAD_LOCKED_S));
            }
        }
        let locked: Vec<String> = self.head_locked.iter().map(|(k, _)| k.clone()).collect();
        sightings.retain(|s| !locked.contains(&key(&s.text)));
        self.tracks.retain(|t| !locked.contains(&t.key));
        let mut followed = if self.moving { Followed::default() } else { self.reanchor(&sightings) };
        followed.held = self.hold_anchors && (followed.moved.is_some() || followed.turned.is_some());
        if !followed.held {
            let turn = followed.turned.map_or(geometry::IDENTITY, |a| crate::motion::turn_about(followed.about, a));
            let by = followed.moved.unwrap_or([0.0; 3]);
            if followed.moved.is_some() || followed.turned.is_some() {
                for t in &mut self.tracks {
                    t.placed.pose = geometry::compose(&crate::motion::moved_by(by), &geometry::compose(&turn, &t.placed.pose));
                }
            }
        }
        let mut matched = vec![false; self.tracks.len()];
        let mut fresh = Vec::new();
        for s in sightings {
            let k = key(&s.text);
            let best = self
                .tracks
                .iter()
                .enumerate()
                .filter(|(i, _)| !matched[*i])
                .filter_map(|(i, t)| {
                    let d = geometry::norm(sub(t.placed.centre(), s.placed.centre()));
                    let same = t.key == k || alike(&t.key, &k) >= 0.75;
                    (same && d < tolerance(&t.placed, &s.placed)).then_some((i, d))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((i, _)) => {
                    matched[i] = true;
                    let t = &mut self.tracks[i];
                    // A first measurement from both eyes replaces a guess outright.
                    let follow = if s.placed.measured && !t.placed.measured { 1.0 } else if !s.placed.measured && t.placed.measured { 0.1 } else { FOLLOW };
                    t.placed = blend(&t.placed, &s.placed, follow);
                    if s.confidence > t.confidence && t.key != k {
                        // A clearer read of the same line: its translation is asked for anew.
                        t.text = s.text;
                        t.key = k;
                    }
                    t.confidence = t.confidence.max(s.confidence);
                    t.colours = s.colours;
                    t.context = s.context;
                    t.in_eye = s.in_eye;
                    t.last_seen = now;
                    t.in_view = now;
                    t.moved_at = self.moved;
                    t.sightings += 1;
                    t.misses = 0;
                }
                None => fresh.push(Track {
                    id: {
                        self.next += 1;
                        self.next
                    },
                    key: k,
                    text: s.text,
                    confidence: s.confidence,
                    placed: s.placed,
                    colours: s.colours,
                    context: s.context,
                    first: now,
                    last_seen: now,
                    in_view: now,
                    moved_at: self.moved,
                    in_eye: s.in_eye,
                    sightings: 1,
                    misses: 0,
                }),
            }
        }
        let mut keep = Vec::with_capacity(self.tracks.len() + fresh.len());
        let moved = self.moved;
        for (t, was_matched) in std::mem::take(&mut self.tracks).into_iter().zip(matched) {
            if was_matched {
                keep.push(t);
                continue;
            }
            let mut t = t;
            let missed = |t: &mut Track| {
                t.misses += 1;
                t.in_view = now;
                let moved_since = (moved[0] - t.moved_at[0]) + (moved[1] - t.moved_at[1]) > 0.05;
                t.misses < MISSES || now - t.last_seen < if moved_since { MOVED_MISS_S } else { MISS_S }
            };
            let stays = match seen(&t) {
                Seen::Open | Seen::Covered { changed: true } => missed(&mut t),
                Seen::Covered { changed: false } => {
                    t.last_seen = now;
                    t.in_view = now;
                    t.moved_at = moved;
                    t.misses = 0;
                    true
                }
                Seen::Edge => {
                    t.in_view = now;
                    now - t.last_seen < UNCONFIRMED_S
                }
                Seen::Away => now - t.in_view < OUT_OF_VIEW_S,
            };
            if stays {
                keep.push(t);
            }
        }
        // A new line where a known one is (the same place, other letters): the newer read wins.
        keep.retain(|t| !fresh.iter().any(|f| geometry::norm(sub(f.placed.centre(), t.placed.centre())) < 0.5 * f.placed.height.max(t.placed.height) && t.sightings == 1 && t.last_seen < now));
        keep.extend(fresh);
        self.tracks = keep;
        followed
    }

    /// Lines measured from both eyes found again, each alone of its text in the look and near where it was followed, but
    /// further than a measurement's own noise: where they agree the world is, against where the anchors have it.
    ///
    /// When the sticks turned the player more than they walked since those lines were confirmed (a turn's error at
    /// their distance against what was walked), the error is taken as a turn about the head first, and what is left as
    /// a shift. Standing still since, one line alone never moves the others (a far line's distance from both eyes is
    /// a few centimetres per pixel: Enzo's 2026-11-14 log has lines moved 18 cm "after walking 0.00 m and turning 0
    /// degrees").
    fn reanchor(&self, sightings: &[Sighting]) -> Followed {
        // Each: where the anchors had it, where it was read, how far the sticks moved the player since it was confirmed.
        let mut pairs: Vec<(V3, V3, [f32; 2])> = Vec::new();
        for s in sightings.iter().filter(|s| s.placed.measured) {
            let k = key(&s.text);
            let same = |text: &str| text == k || alike(text, &k) >= 0.75;
            if sightings.iter().filter(|o| same(&key(&o.text))).count() != 1 {
                continue;
            }
            let since = |t: &Track| [(self.moved[0] - t.moved_at[0]).max(0.0), (self.moved[1] - t.moved_at[1]).max(0.0)];
            // As far as the sticks' motion since a line was confirmed can have been misjudged: half of what was walked,
            // and half of what was turned at that distance.
            let reach = |t: &Track| 0.25 + 0.5 * (since(t)[0] + since(t)[1] * s.placed.distance);
            let near: Vec<&Track> = self.tracks.iter().filter(|t| t.placed.measured && same(&t.key) && geometry::norm(sub(t.placed.centre(), s.placed.centre())) < reach(t)).collect();
            if let [t] = near[..] {
                pairs.push((t.placed.centre(), s.placed.centre(), since(t)));
            }
        }
        let by: Vec<V3> = pairs.iter().map(|(p, q, _)| sub(*q, *p)).collect();
        let Some(m) = crate::motion::median(&by) else { return Followed::default() };
        // A turn about the head, when turning is what the sticks did most since.
        let turned = self.head.and_then(|h| {
            let walked = crate::motion::median(&pairs.iter().map(|(_, _, d)| [d[0], 0.0, 0.0]).collect::<Vec<_>>())?[0];
            let turn_reach = crate::motion::median(&pairs.iter().map(|(p, _, d)| [d[1] * geometry::norm(sub(*p, h)), 0.0, 0.0]).collect::<Vec<_>>())?[0];
            if turn_reach <= walked.max(0.05) {
                return None;
            }
            let angles: Vec<V3> = pairs.iter().filter_map(|(p, q, _)| yaw_between(h, *p, *q)).map(|a| [a, 0.0, 0.0]).collect();
            let a = crate::motion::median(&angles)?[0];
            let turn = crate::motion::turn_about(h, a);
            let rest = crate::motion::median(&pairs.iter().map(|(p, q, _)| sub(*q, geometry::apply(&turn, *p))).collect::<Vec<_>>())?;
            Some((a, rest, turn))
        });
        // Where each line would be after the correction.
        let corrected = |p: V3| match &turned {
            Some((_, rest, turn)) => add(geometry::apply(turn, p), *rest),
            None => add(p, m),
        };
        let agreeing = pairs.iter().filter(|(p, q, _)| geometry::norm(sub(*q, corrected(*p))) < 0.15).count();
        let size = crate::motion::median(&pairs.iter().map(|(p, _, _)| [geometry::norm(sub(corrected(*p), *p)), 0.0, 0.0]).collect::<Vec<_>>()).map_or(0.0, |v| v[0]);
        // One line alone may be a misplacement: it must be well off to count. Several must mostly agree.
        let distance = sightings.iter().filter(|s| s.placed.measured).map(|s| s.placed.distance).fold(0.0f32, f32::max);
        let noise = 0.08f32.max(0.04 * distance);
        let still = pairs.iter().all(|(_, _, d)| d[0] < 0.05 && d[1] < 0.02);
        let enough = if by.len() == 1 { !still && size > 2.0 * noise } else { agreeing * 2 > by.len() && agreeing >= 2 - (!still) as usize && size > noise };
        match (enough, turned) {
            (false, _) => Followed { agreeing, ..Followed::default() },
            (true, Some((a, rest, _))) => Followed { moved: Some(rest), turned: Some(a), about: self.head.unwrap_or_default(), agreeing, held: false },
            (true, None) => Followed { moved: Some(m), turned: None, about: [0.0; 3], agreeing, held: false },
        }
    }

    pub fn get(&self, id: u64) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn remove(&mut self, id: u64) {
        self.tracks.retain(|t| t.id != id);
    }
}

/// The middle of a set of points.
pub fn middle(points: &[V3]) -> V3 {
    scale(points.iter().fold([0.0; 3], |a, p| add(a, *p)), 1.0 / points.len().max(1) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::IDENTITY;

    fn placed(x: f32) -> Placed {
        let mut pose = IDENTITY;
        pose[0][3] = x;
        pose[2][3] = -2.0;
        Placed { pose, width: 0.5, height: 0.08, measured: true, distance: 2.0 }
    }
    fn seen(text: &str, x: f32) -> Sighting {
        Sighting { text: text.into(), confidence: 0.9, placed: placed(x), colours: Colours { background: [255; 3], text: [0; 3] }, context: vec![], in_eye: [x, 0.0, -2.0] }
    }

    #[test]
    fn text_that_moves_with_the_head_is_left_alone() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("Vous avez rejoint", 0.0), seen("出口", 1.0)], &|_| Seen::Open);
        // The head turned: the notice is read at the same place before the eye, half a metre away in the world.
        let mut notice = seen("Vous avez rejoint", 0.5);
        notice.in_eye = [0.0, 0.0, -2.0];
        t.update(1.0, vec![notice.clone()], &|_| Seen::Covered { changed: false });
        assert!(!t.tracks.iter().any(|x| x.text == "Vous avez rejoint"), "its translation goes");
        assert!(t.tracks.iter().any(|x| x.text == "出口"), "the sign stays");
        // And is not followed again while it shows.
        t.update(2.0, vec![notice], &|_| Seen::Open);
        assert!(!t.tracks.iter().any(|x| x.text == "Vous avez rejoint"));
        // A sign read again where it is, after the head moved: not mistaken for one (its place before the eye changed).
        let mut sign = seen("出口", 1.0);
        sign.in_eye = [1.0, 0.0, -2.3];
        t.update(3.0, vec![sign], &|_| Seen::Open);
        assert!(t.tracks.iter().any(|x| x.text == "出口"));
    }

    #[test]
    fn a_line_seen_again_is_one_line_that_moves_a_little() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        t.update(0.5, vec![seen("出口", 0.04)], &|_| Seen::Open);
        assert_eq!(t.tracks.len(), 1);
        let x = t.tracks[0].placed.centre()[0];
        assert!(x > 0.0 && x < 0.04 * 0.5, "smoothed, not jumped: {x}");
        assert!(t.tracks[0].settled());
    }

    #[test]
    fn a_line_gone_from_where_it_was_seen_goes() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        t.update(0.5, vec![], &|_| Seen::Open);
        assert_eq!(t.tracks.len(), 1, "one miss is forgiven");
        t.update(1.5, vec![], &|_| Seen::Open);
        assert!(t.tracks.is_empty());
    }

    #[test]
    fn a_line_out_of_view_stays_for_a_while() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        t.update(4.0, vec![], &|_| Seen::Away);
        assert_eq!(t.tracks.len(), 1, "looking away a moment and back finds it");
        t.update(5.5, vec![], &|_| Seen::Away);
        assert_eq!(t.tracks.len(), 1);
        t.update(OUT_OF_VIEW_S + 0.5, vec![], &|_| Seen::Away);
        assert!(t.tracks.is_empty(), "off the screen long enough: gone");
    }

    #[test]
    fn looking_back_at_it_restarts_its_time() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        t.update(5.0, vec![], &|_| Seen::Away);
        // On screen at the edge for a moment, then away again: counted from then.
        t.update(5.5, vec![], &|_| Seen::Edge);
        t.update(10.0, vec![], &|_| Seen::Away);
        assert_eq!(t.tracks.len(), 1);
        t.update(12.0, vec![], &|_| Seen::Away);
        assert!(t.tracks.is_empty());
    }

    #[test]
    fn at_the_edge_of_the_view_unconfirmed_it_goes_in_time() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        t.update(UNCONFIRMED_S - 1.0, vec![], &|_| Seen::Edge);
        assert_eq!(t.tracks.len(), 1);
        t.update(UNCONFIRMED_S + 0.5, vec![], &|_| Seen::Edge);
        assert!(t.tracks.is_empty());
    }

    #[test]
    fn a_sign_looked_at_never_goes() {
        // Hundreds of looks at both paces with its translation over it; a noisy compare now and then (someone walking by)
        // never twice in a row: it stays all along.
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        let mut now = 0.0;
        for k in 0..300 {
            now += if k % 3 == 0 { 0.35 } else { 1.5 };
            let changed = k % 17 == 5;
            t.update(now, vec![], &|_| Seen::Covered { changed });
            assert_eq!(t.tracks.len(), 1, "look {k} at {now:.1} s");
        }
    }

    #[test]
    fn a_line_missed_in_quick_looks_waits_a_little() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        t.update(0.35, vec![], &|_| Seen::Open);
        t.update(0.7, vec![], &|_| Seen::Open);
        assert_eq!(t.tracks.len(), 1, "two quick misses are not yet {MISS_S} s");
        t.update(1.3, vec![], &|_| Seen::Open);
        assert!(t.tracks.is_empty());
    }

    #[test]
    fn after_the_sticks_moved_a_missed_line_waits_to_be_found_again() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        t.moved = [0.0, 0.3];
        for now in [0.5, 1.0, 1.5, 2.0, 2.5] {
            t.update(now, vec![], &|_| Seen::Covered { changed: true });
            assert_eq!(t.tracks.len(), 1, "{now}");
        }
        t.update(3.2, vec![], &|_| Seen::Covered { changed: true });
        assert!(t.tracks.is_empty());
    }

    #[test]
    fn found_again_elsewhere_all_lines_move_with_it() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0), seen("入口", 1.0)], &|_| Seen::Open);
        // Walked 3 m with the stick and the estimate fell 60 cm short: the next look finds 出口 0.6 m further along X.
        t.moved = [3.0, 0.0];
        let f = t.update(1.5, vec![seen("出口", 0.6)], &|_| Seen::Away);
        let by = f.moved.expect("re-anchored");
        assert!((by[0] - 0.6).abs() < 1e-4, "{by:?}");
        assert_eq!(t.tracks.len(), 2, "one line, not a new one beside the old");
        let other = t.tracks.iter().find(|x| x.text == "入口").unwrap();
        assert!((other.placed.centre()[0] - 1.6).abs() < 1e-4, "the line not looked at moved too");
        let exit = t.tracks.iter().find(|x| x.text == "出口").unwrap();
        assert!((exit.placed.centre()[0] - 0.6).abs() < 1e-4);
    }

    #[test]
    fn standing_still_the_same_text_elsewhere_is_another_sign() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        let f = t.update(0.5, vec![seen("出口", 1.0)], &|_| Seen::Open);
        assert_eq!(f.moved, None);
        // Turned a quarter turn with the stick: at 2 m a sign may be up to 0.25 + 0.5 x 1.57 x 2 m off.
        t.moved = [0.0, std::f32::consts::FRAC_PI_2];
        let f = t.update(1.0, vec![seen("出口", 2.0)], &|_| Seen::Away);
        assert!(f.moved.is_some());
    }

    #[test]
    fn no_re_anchoring_from_a_look_taken_while_moving() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0), seen("入口", 1.0)], &|_| Seen::Open);
        t.moved = [3.0, 0.0];
        t.moving = true;
        let f = t.update(1.5, vec![seen("出口", 0.6)], &|_| Seen::Away);
        assert_eq!(f.moved, None);
        assert!((t.tracks.iter().find(|x| x.text == "入口").unwrap().placed.centre()[0] - 1.0).abs() < 1e-6);
        // The next look, the sticks at rest again: now it does.
        t.moving = false;
        let f = t.update(3.0, vec![seen("出口", 0.6), seen("入口", 1.6)], &|_| Seen::Open);
        assert!(f.moved.is_some());
    }

    #[test]
    fn a_small_difference_is_noise_not_a_move() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0), seen("入口", 1.0)], &|_| Seen::Open);
        t.moved = [3.0, 0.0];
        let f = t.update(0.5, vec![seen("出口", 0.03), seen("入口", 1.02)], &|_| Seen::Open);
        assert_eq!(f.moved, None);
    }

    #[test]
    fn two_signs_alike_do_not_re_anchor() {
        // Two 出口 in one look: which is which cannot be told.
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        t.moved = [3.0, 0.0];
        let f = t.update(0.5, vec![seen("出口", 0.7), seen("出口", -0.7)], &|_| Seen::Open);
        assert_eq!(f.moved, None);
    }

    #[test]
    fn lines_that_disagree_do_not_re_anchor() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0), seen("入口", 2.0), seen("非常口", 4.0)], &|_| Seen::Open);
        t.moved = [3.0, 0.0];
        let f = t.update(0.5, vec![seen("出口", 0.5), seen("入口", 1.5), seen("非常口", 4.0)], &|_| Seen::Open);
        assert_eq!(f.moved, None, "{f:?}");
    }

    #[test]
    fn a_covered_line_stays_until_its_surroundings_change() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        for k in 1..10 {
            t.update(k as f64, vec![], &|_| Seen::Covered { changed: false });
        }
        assert_eq!(t.tracks.len(), 1);
        t.update(11.0, vec![], &|_| Seen::Covered { changed: true });
        assert_eq!(t.tracks.len(), 1, "one changed compare may be someone passing");
        t.update(12.5, vec![], &|_| Seen::Covered { changed: true });
        assert!(t.tracks.is_empty());
    }

    #[test]
    fn other_lines_and_other_places_are_other_tracks() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0), seen("入口", 1.0)], &|_| Seen::Open);
        t.update(0.5, vec![seen("出口", 2.0)], &|_| Seen::Open);
        assert_eq!(t.tracks.len(), 3, "the same text a metre away is another sign");
        // Other letters where a line was seen only once: the newer read replaces it.
        t.update(1.0, vec![seen("非常口", 1.0)], &|_| Seen::Open);
        assert!(t.tracks.iter().any(|x| x.text == "非常口") && !t.tracks.iter().any(|x| x.text == "入口"));
    }

    #[test]
    fn misreads_of_one_line_are_one_line() {
        assert!(alike("Добро пожаловать", "Добро пожаловат") > 0.9);
        assert!(alike("出口はこちら", "入口はあちら") < 0.75);
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("Добро пожаловат", 0.0)], &|_| Seen::Open);
        let mut better = seen("Добро пожаловать", 0.01);
        better.confidence = 0.97;
        t.update(0.5, vec![better], &|_| Seen::Open);
        assert_eq!(t.tracks.len(), 1);
        assert_eq!(t.tracks[0].text, "Добро пожаловать");
    }

    /// A line read at `p` (anchor space), measured from both eyes; its place before the eye as if the head were at the
    /// origin looking down -Z.
    fn seen_at(text: &str, p: V3) -> Sighting {
        let mut pose = IDENTITY;
        for i in 0..3 {
            pose[i][3] = p[i];
        }
        let distance = geometry::norm(p);
        Sighting { text: text.into(), confidence: 0.9, placed: Placed { pose, width: 0.5, height: 0.08, measured: true, distance }, colours: Colours { background: [255; 3], text: [0; 3] }, context: vec![], in_eye: p }
    }

    /// Enzo's 2026-11-14 log: "found lines again 65 cm from their anchors after walking 4.49 m and turning 131 degrees".
    /// Short stick turns came out 3.5 to 11 degrees off in the sandbox. A turn misjudged by 10 degrees, found again on
    /// two signs ahead: every line turns back about the head, the one behind too (one shift for all, as before, would
    /// have moved the line behind the wrong way).
    #[test]
    fn a_misjudged_turn_is_undone_about_the_head() {
        let mut t = Tracker { head: Some([0.0; 3]), ..Tracker::default() };
        let (exit, entrance, behind) = ([0.0, 0.0, -3.0], [2.0, 0.0, -3.0], [0.5, 0.0, 4.0]);
        t.update(0.0, vec![seen_at("出口", exit), seen_at("入口", entrance), seen_at("非常口", behind)], &|_| Seen::Open);
        // Turned two radians with the stick since, and the estimate was 10 degrees short.
        t.moved = [0.0, 2.0];
        let error = crate::motion::turn_about([0.0; 3], 10f32.to_radians());
        let f = t.update(1.0, vec![seen_at("出口", geometry::apply(&error, exit)), seen_at("入口", geometry::apply(&error, entrance))], &|t: &Track| if t.text == "非常口" { Seen::Away } else { Seen::Open });
        let turned = f.turned.expect("a turn");
        assert!((turned.to_degrees() - 10.0).abs() < 0.5, "{}", turned.to_degrees());
        assert!(geometry::norm(f.moved.unwrap_or_default()) < 0.02, "no shift left over: {f:?}");
        for (text, was) in [("出口", exit), ("入口", entrance), ("非常口", behind)] {
            let now = t.tracks.iter().find(|x| x.text == text).unwrap().placed.centre();
            let want = geometry::apply(&error, was);
            assert!(geometry::norm(sub(now, want)) < 0.02, "{text}: {now:?}, the world has it at {want:?}");
        }
    }

    /// Walking more than turning since: a shift, as before (the walking speed learns from it).
    #[test]
    fn a_misjudged_walk_is_still_a_shift() {
        let mut t = Tracker { head: Some([0.0; 3]), ..Tracker::default() };
        t.update(0.0, vec![seen_at("出口", [0.0, 0.0, -3.0]), seen_at("入口", [2.0, 0.0, -3.0])], &|_| Seen::Open);
        t.moved = [3.0, 0.05];
        let f = t.update(1.0, vec![seen_at("出口", [0.0, 0.0, -2.5]), seen_at("入口", [2.0, 0.0, -2.5])], &|_| Seen::Open);
        assert_eq!(f.turned, None);
        assert!((f.moved.unwrap()[2] - 0.5).abs() < 1e-4, "{f:?}");
    }

    /// Standing still, a far line read 18 cm from where it was is its distance's noise: nothing moves. Two lines that
    /// agree are not noise.
    #[test]
    fn standing_still_one_line_alone_moves_nothing() {
        let mut t = Tracker { head: Some([0.0; 3]), ..Tracker::default() };
        t.update(0.0, vec![seen_at("出口", [0.0, 0.0, -3.0]), seen_at("入口", [1.5, 0.0, -3.0])], &|_| Seen::Open);
        let f = t.update(1.0, vec![seen_at("出口", [0.0, 0.0, -2.82])], &|_| Seen::Open);
        assert_eq!((f.moved, f.turned), (None, None));
        assert_eq!(t.tracks.len(), 2, "read deeper at the same place before the eye: still the sign, not a notice");
        let f = t.update(2.0, vec![seen_at("出口", [0.0, 0.0, -2.78]), seen_at("入口", [1.5, 0.0, -2.78])], &|_| Seen::Open);
        assert!(f.moved.is_some(), "{f:?}");
    }
}
