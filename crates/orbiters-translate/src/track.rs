//! Lines followed from look to look, so that a translation stays put: a line seen again (same text, about the same
//! place in the room) refines its place a little at a time; a line not seen again where the eye could see it goes; a line
//! out of view stays where it was for a while (it is anchored in the room: looking back finds it there).
//!
//! A line whose translation is shown is hidden from the text reader by that translation (our overlays are in the eye's
//! picture, and are masked out of it). It is then kept while what surrounds it looks the same ([`Track::context`]); when
//! that changes (the sign moved, someone walked in front, a chat bubble changed), it goes, and the next look reads what
//! is there now.

use crate::geometry::{self, add, normalized, scale, sub, V3};
use crate::place::Placed;

/// Looks in a row a line may be missed where it should be seen before it goes.
pub const MISSES: u32 = 2;
/// Seconds a line out of view stays.
pub const OUT_OF_VIEW_S: f64 = 20.0;
/// How much of a new measurement a known line takes.
const FOLLOW: f32 = 0.35;

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
}

/// What a look could tell about a line already followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// Its place was in the part of the eye read, uncovered: it should have been found if still there.
    Open,
    /// Under its own translation: only its surroundings could be compared.
    Covered { changed: bool },
    /// Outside the part read, or behind the eye.
    Away,
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
    pub last_seen: f64,
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
}

impl Tracker {
    /// One look's lines. `seen` says, for each line already followed, what this look could tell about it.
    pub fn update(&mut self, now: f64, sightings: Vec<Sighting>, seen: &dyn Fn(&Track) -> Seen) {
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
                    t.last_seen = now;
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
                    sightings: 1,
                    misses: 0,
                }),
            }
        }
        let mut keep = Vec::with_capacity(self.tracks.len() + fresh.len());
        for (t, was_matched) in std::mem::take(&mut self.tracks).into_iter().zip(matched) {
            if was_matched {
                keep.push(t);
                continue;
            }
            let mut t = t;
            let stays = match seen(&t) {
                Seen::Open => {
                    t.misses += 1;
                    t.misses < MISSES
                }
                Seen::Covered { changed } => {
                    if !changed {
                        t.last_seen = now;
                    }
                    !changed
                }
                Seen::Away => now - t.last_seen < OUT_OF_VIEW_S,
            };
            if stays {
                keep.push(t);
            }
        }
        // A new line where a known one is (the same place, other letters): the newer read wins.
        keep.retain(|t| !fresh.iter().any(|f| geometry::norm(sub(f.placed.centre(), t.placed.centre())) < 0.5 * f.placed.height.max(t.placed.height) && t.sightings == 1 && t.last_seen < now));
        keep.extend(fresh);
        self.tracks = keep;
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
        Sighting { text: text.into(), confidence: 0.9, placed: placed(x), colours: Colours { background: [255; 3], text: [0; 3] }, context: vec![] }
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
        t.update(1.0, vec![], &|_| Seen::Open);
        assert!(t.tracks.is_empty());
    }

    #[test]
    fn a_line_out_of_view_stays_for_a_while() {
        let mut t = Tracker::default();
        t.update(0.0, vec![seen("出口", 0.0)], &|_| Seen::Open);
        t.update(10.0, vec![], &|_| Seen::Away);
        assert_eq!(t.tracks.len(), 1);
        t.update(25.0, vec![], &|_| Seen::Away);
        assert!(t.tracks.is_empty());
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
}
