//! One look, from the two eyes' pictures to lines placed in the room: the text reader on the left eye (with our own
//! overlays masked out of it), each line measured with the right eye, placed, coloured, and compared with what
//! surrounded the lines already shown. Pure of SteamVR: the tests feed it still pictures.

use crate::geometry::{self, add, scale, EyeView, M34, V3};
use crate::ocr::{det_size, DetInput, Detector, Line, Polygon, Reader, Recogniser, Rgba};
use crate::place::{self, Pair, Placed};
use crate::track::{Colours, Seen, Sighting, Track};
use std::time::Instant;

/// One of our own overlays as it was when the pictures were taken: what to mask out of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub pose: M34,
    pub width: f32,
    pub height: f32,
    /// The translation of this track, if it is one.
    pub track: Option<u64>,
}

impl Shown {
    pub fn corners(&self) -> [V3; 4] {
        Placed { pose: self.pose, width: self.width, height: self.height, measured: true, distance: 0.0 }.corners()
    }
    /// Its outline in an eye's whole picture, a little larger than it is (its edges fade, and poses drift a little).
    pub fn outline(&self, view: &EyeView) -> Option<Polygon> {
        let grown = Shown { width: self.width * 1.06 + 0.01, height: self.height * 1.12 + 0.01, ..*self };
        let points: Option<Vec<[f32; 2]>> = grown.corners().iter().map(|c| view.project(*c)).collect();
        points.map(Polygon)
    }
}

/// A look: the pair of pictures, the part of the left eye to read (whole-eye pixels: x, y, width, height) and our overlays.
pub struct Look<'a> {
    pub pair: Pair<'a>,
    pub area: [u32; 4],
    pub shown: &'a [Shown],
}

/// What a look cost, milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Timings {
    pub detect: f32,
    pub read: f32,
    pub place: f32,
    pub lines: usize,
}

/// The text reader's models, loaded.
pub struct Engine {
    pub detector: Detector,
    pub main: Recogniser,
    pub korean: Option<Recogniser>,
    pub cyrillic: Option<Recogniser>,
    /// The detector's input's longer side.
    pub detail: u32,
}

/// A line the main reader was not sure of is read by the other readers too, and the surest read wins.
const SURE: f32 = 0.85;
/// Lines read with less confidence than this are dropped.
const LEAST: f32 = 0.6;

impl Engine {
    /// The models from [`crate::models::folder`], each on `threads` threads.
    pub fn load(threads: usize) -> anyhow::Result<Self> {
        use crate::models::{path, CYRILLIC, DETECTOR, KOREAN, MAIN};
        let rec = |files: &[crate::models::ModelFile; 2], reader| Recogniser::load(&path(&files[0]), &path(&files[1]), reader, threads);
        Ok(Self {
            detector: Detector::load(&path(&DETECTOR), threads)?,
            main: rec(&MAIN, Reader::Main)?,
            korean: rec(&KOREAN, Reader::Korean).ok(),
            cyrillic: rec(&CYRILLIC, Reader::Cyrillic).ok(),
            detail: 960,
        })
    }

    /// The lines of text in the look's part of the left eye, our overlays left out.
    pub fn read(&mut self, look: &Look) -> anyhow::Result<(Vec<Line>, Timings)> {
        let mut timings = Timings::default();
        let left = look.pair.pictures[0];
        let from = look.pair.from[0];
        // In the picture's own pixels (it may be a part of the eye).
        let area = [look.area[0].saturating_sub(from[0]), look.area[1].saturating_sub(from[1]), look.area[2].min(left.width), look.area[3].min(left.height)];
        let hidden: Vec<Polygon> = look
            .shown
            .iter()
            .filter_map(|s| s.outline(&look.pair.views[0]))
            .map(|p| Polygon(p.0.iter().map(|q| [q[0] - from[0] as f32, q[1] - from[1] as f32]).collect()))
            .collect();
        let t = Instant::now();
        let boxes = self.detector.detect(&DetInput { picture: left, area, size: det_size(area, self.detail), hidden: &hidden })?;
        // A line partly under one of our overlays is not read: it would read our own letters.
        let boxes: Vec<_> = boxes.into_iter().filter(|b| !b.corners.iter().chain([&b.centre()]).any(|c| hidden.iter().any(|h| h.contains(*c)))).collect();
        timings.detect = t.elapsed().as_secs_f32() * 1000.0;
        let t = Instant::now();
        let mut lines = self.main.read(left, &boxes)?;
        let unsure: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].confidence < SURE).collect();
        if !unsure.is_empty() {
            let unsure_boxes: Vec<_> = unsure.iter().map(|&i| lines[i].boxed.clone()).collect();
            for other in [self.korean.as_mut(), self.cyrillic.as_mut()].into_iter().flatten() {
                let reader = other.reader;
                for (k, l) in other.read(left, &unsure_boxes)?.into_iter().enumerate() {
                    let i = unsure[k];
                    // A reader for one script reads any other as look-alike letters of its own ("o6po noxanoBaTb" for
                    // "Добро пожаловать", and sure of it): its read counts only in its own script.
                    if own_script(reader, &l.text) && l.confidence > lines[i].confidence + 0.02 {
                        lines[i] = l;
                    }
                }
            }
        }
        lines.retain(|l| l.confidence >= LEAST && !l.text.trim().is_empty());
        timings.read = t.elapsed().as_secs_f32() * 1000.0;
        timings.lines = lines.len();
        // Back to whole-eye pixels.
        for l in &mut lines {
            for c in &mut l.boxed.corners {
                *c = [c[0] + from[0] as f32, c[1] + from[1] as f32];
            }
        }
        Ok((lines, timings))
    }
}

/// Whether a read of the Korean or the Cyrillic reader has letters of its script.
fn own_script(reader: Reader, text: &str) -> bool {
    match reader {
        Reader::Main => true,
        Reader::Korean => text.chars().any(|c| ('\u{AC00}'..='\u{D7AF}').contains(&c) || ('\u{1100}'..='\u{11FF}').contains(&c) || ('\u{3130}'..='\u{318F}').contains(&c)),
        Reader::Cyrillic => text.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c)),
    }
}

/// The lines of a look placed in the room, with their colours and surroundings.
pub fn sightings(look: &Look, lines: &[Line]) -> Vec<Sighting> {
    let found: Vec<place::Found> = lines.iter().map(|l| place::measure(&look.pair, &l.boxed)).collect();
    let placed = place::place_all(&look.pair.views[0], &found);
    lines
        .iter()
        .zip(placed)
        .map(|(l, p)| Sighting { text: l.text.clone(), confidence: l.confidence, colours: colours(&look.pair, &l.boxed), context: context(&look.pair, &p).unwrap_or_default(), placed: p })
        .collect()
}

/// The background and text colours of a line in the left eye: the median of a ring just outside its box, and the
/// mean of the pixels inside it furthest from that.
pub fn colours(pair: &Pair, b: &crate::ocr::TextBox) -> Colours {
    let p = pair.pictures[0];
    let from = pair.from[0];
    let at = |q: [f32; 2]| p.sample(q[0] - from[0] as f32, q[1] - from[1] as f32);
    let [tl, tr, br, bl] = b.corners;
    let lerp2 = |a: [f32; 2], b: [f32; 2], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let c = b.centre();
    let out = |q: [f32; 2], k: f32| [c[0] + (q[0] - c[0]) * k, c[1] + (q[1] - c[1]) * k];
    // The box is padded already: its own edge is mostly background. Sample on it and a little outside.
    let mut ring: Vec<[f32; 3]> = Vec::new();
    for k in [1.0, 1.08] {
        for i in 0..24 {
            let t = i as f32 / 24.0;
            for (a, bb) in [(tl, tr), (tr, br), (br, bl), (bl, tl)] {
                ring.push(at(out(lerp2(a, bb, t), k)));
            }
        }
    }
    let median = |ch: usize, v: &[[f32; 3]]| {
        let mut x: Vec<f32> = v.iter().map(|p| p[ch]).collect();
        x.sort_by(|a, b| a.total_cmp(b));
        x[x.len() / 2]
    };
    let bg = [median(0, &ring), median(1, &ring), median(2, &ring)];
    let mut inside: Vec<([f32; 3], f32)> = Vec::new();
    for j in 1..12 {
        for i in 1..48 {
            let q = lerp2(lerp2(tl, tr, i as f32 / 48.0), lerp2(bl, br, i as f32 / 48.0), j as f32 / 12.0);
            let s = at(q);
            let d = (0..3).map(|k| (s[k] - bg[k]).powi(2)).sum::<f32>();
            inside.push((s, d));
        }
    }
    inside.sort_by(|a, b| b.1.total_cmp(&a.1));
    let top = &inside[..(inside.len() / 8).max(1)];
    let ink = top.iter().fold([0f32; 3], |a, (s, _)| [a[0] + s[0], a[1] + s[1], a[2] + s[2]]).map(|v| v / top.len() as f32);
    Colours { background: bg.map(|v| v.round().clamp(0.0, 255.0) as u8), text: ink.map(|v| v.round().clamp(0.0, 255.0) as u8) }
}

/// Grey samples of the left eye on a ring around a placed line, outside any translation drawn over it (which may be up
/// to [`crate::paint`]'s longer and taller box). `None` when part of it is outside the picture.
pub fn context(pair: &Pair, placed: &Placed) -> Option<Vec<f32>> {
    let ring = Placed { width: placed.width * 2.0 + 0.02, height: placed.height * 2.2 + 0.02, ..*placed };
    let [tl, tr, br, bl] = ring.corners();
    let mut out = Vec::with_capacity(64);
    for (a, b) in [(tl, tr), (tr, br), (br, bl), (bl, tl)] {
        for i in 0..16 {
            let q = geometry::lerp(a, b, i as f32 / 16.0);
            let px = pair.views[0].project(q)?;
            let (x, y) = (px[0] - pair.from[0][0] as f32, px[1] - pair.from[0][1] as f32);
            let p = pair.pictures[0];
            if x < 0.0 || y < 0.0 || x >= p.width as f32 || y >= p.height as f32 {
                return None;
            }
            let s = p.sample(x, y);
            out.push(0.299 * s[0] + 0.587 * s[1] + 0.114 * s[2]);
        }
    }
    Some(out)
}

/// Whether two context rings differ: by their mean change once the overall brightness is taken out.
pub fn context_changed(before: &[f32], now: &[f32]) -> bool {
    if before.len() != now.len() || before.is_empty() {
        return false;
    }
    let n = before.len() as f32;
    let shift = (now.iter().sum::<f32>() - before.iter().sum::<f32>()) / n;
    let diff = before.iter().zip(now).map(|(a, b)| (b - a - shift).abs()).sum::<f32>() / n;
    diff > 28.0 || shift.abs() > 60.0
}

/// What a look could tell of a followed line: open when its middle is in the part read and nothing of ours covers it,
/// covered when its own translation does (its surroundings then compared), away otherwise.
pub fn seen(look: &Look, track: &Track) -> Seen {
    let view = &look.pair.views[0];
    let Some(c) = view.project(track.placed.centre()) else { return Seen::Away };
    let a = look.area;
    if c[0] < a[0] as f32 || c[1] < a[1] as f32 || c[0] > (a[0] + a[2]) as f32 || c[1] > (a[1] + a[3]) as f32 {
        return Seen::Away;
    }
    for s in look.shown {
        if let Some(outline) = s.outline(view) {
            if outline.contains(c) {
                return if s.track == Some(track.id) {
                    let changed = context(&look.pair, &track.placed).is_some_and(|now| context_changed(&track.context, &now));
                    Seen::Covered { changed }
                } else {
                    // Under a menu or another translation: nothing to tell.
                    Seen::Away
                };
            }
        }
    }
    Seen::Open
}

/// A point in front of an eye, for the tests and the stand-in eyes.
pub fn ahead(view: &EyeView, px: [f32; 2], distance: f32) -> V3 {
    let (o, d) = view.ray(px);
    add(o, scale(d, distance))
}

/// A picture as the pipeline reads it.
pub fn rgba(width: u32, height: u32, pixels: &[u8]) -> Rgba<'_> {
    Rgba { width, height, pixels }
}
