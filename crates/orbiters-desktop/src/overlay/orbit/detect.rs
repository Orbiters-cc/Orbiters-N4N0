//! Knowing that VRChat's wheel or Quick Menu is open, in desktop mode, without touching the game.
//!
//! **What VRChat says about it: nothing.** OSC reports no menu state, and the log says nothing when a menu opens (only a page
//! of a selected user leaves a line, late). Keys say only that someone pressed Esc or R, and the game may have ignored them
//! (typing in the chatbox, a loading screen, the Main Menu in front). So a key only makes this look, and what is on the screen
//! decides: **Windows Graphics Capture of VRChat's own window**, small parts of each frame, while a key has just been pressed
//! or a menu shows. Between those no capture runs at all.
//!
//! - **The wheel:** a disc of one colour at the exact place its geometry gives ([`super::zones::wheel`]), its edge against the world
//!   all the way round, and the bright line of its rim against its own inside. On frames of 10 window sizes in 2 worlds the edge shows at 100% of
//!   its angles, the rim's line at 67 to 97% and the ring inside it is one colour at 66 to 96%, whatever hovers or which page; on everything else
//!   (no menu, Quick Menu, Main Menu) they hold at 70% and 30% at most. The rim's line is what a dark forest cannot hide: at first the edge alone
//!   was asked for, with a stronger difference, and the wheel over that forest was missed at 1280 x 720 (88%). The colour itself is not looked at: VRChat lets people
//!   choose it.
//! - **The Quick Menu:** its row of icons, on every page of it and on nothing else, compared in shape (normalised correlation, so any
//!   theme or brightness) with what they look like. 4 or more of the 7 at 0.5 or better: on 18 frames of it at 10 window
//!   sizes and 4 pages, 5 to 7; on every other frame 0.
//!
//! Once open, looks without it close the panels: two frames in a row just after Esc or R, else three looks in a row, about 180 ms, as a
//! sub-page's slide lasts that long. And it takes two frames in a row to open: a frame caught mid-animation opens nothing. When frames stop coming (a hitch, a loading screen) nothing can be said, and the
//! panels go: **a wrong "open" is worse than a late one.**

use super::zones::{self, Around};
use egui::{pos2, vec2, Rect, Vec2};
use std::time::{Duration, Instant};

/// A part of a frame in window pixels: top-down BGRA, `stride` bytes a row.
#[derive(Clone, Copy)]
pub struct Part<'a> {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub bgra: &'a [u8],
}

impl Part<'_> {
    /// The colour at window pixel (x, y), blended between pixels (BGR). Off the part: the nearest.
    fn at(&self, x: f32, y: f32) -> [f32; 3] {
        let fx = (x - self.x0 as f32).clamp(0.0, self.width as f32 - 1.001);
        let fy = (y - self.y0 as f32).clamp(0.0, self.height as f32 - 1.001);
        let (ix, iy) = (fx as usize, fy as usize);
        let (ax, ay) = (fx - ix as f32, fy - iy as f32);
        let px = |x: usize, y: usize| -> [f32; 3] {
            let i = y * self.stride as usize + x * 4;
            self.bgra.get(i..i + 3).map_or([0.0; 3], |p| [p[0] as f32, p[1] as f32, p[2] as f32])
        };
        let (a, b, c, d) = (px(ix, iy), px(ix + 1, iy), px(ix, iy + 1), px(ix + 1, iy + 1));
        std::array::from_fn(|i| (a[i] * (1.0 - ax) + b[i] * ax) * (1.0 - ay) + (c[i] * (1.0 - ax) + d[i] * ax) * ay)
    }

    fn grey(&self, x: f32, y: f32) -> f32 {
        let c = self.at(x, y);
        (c[0] + c[1] + c[2]) / 3.0
    }
}

/// What of a frame is looked at: the wheel's box and the icons' row, in window pixels, inside the window.
pub fn parts(client: Vec2) -> [Rect; 2] {
    let window = Rect::from_min_size(pos2(0.0, 0.0), client);
    let (center, radius) = zones::wheel(client);
    let wheel = Rect::from_center_size(center, Vec2::splat(radius * 2.0 * WHEEL_OUTSIDE + 4.0)).intersect(window);
    let (center, unit) = zones::quick_menu(client);
    let reach = ICON_HALF + SHIFTS.iter().fold(0.0f32, |a, s| a.max(s.abs())) + 1.0;
    let row = Rect::from_min_max(
        center + vec2(zones::QM_ICONS_X[0] - reach, zones::QM_ICONS_Y - reach) * unit,
        center + vec2(zones::QM_ICONS_X[6] + reach, zones::QM_ICONS_Y + reach) * unit,
    )
    .intersect(window);
    [wheel, row]
}

const WHEEL_ANGLES: usize = 96;
/// Where the disc's own ring and the world outside it are sampled, in radii.
const WHEEL_INSIDE: f32 = 0.955;
const WHEEL_OUTSIDE: f32 = 1.07;
/// Sum of three channels' differences that counts as a different colour.
const WHEEL_DIFFERENT: f32 = 15.0;
/// Where the rim's bright line is, in radii, and how much brighter than the disc inside it (one channel's worth).
const WHEEL_RIM: [f32; 2] = [0.985, 1.0];
const WHEEL_BRIGHTER: f32 = 6.0;
/// How near a ring's sample keeps to the ring's middle colour, to count as that colour.
const WHEEL_SAME: f32 = 36.0;
/// The share of the rim that must stand out from the world, of the ring that must be one colour, and of the rim's line brighter than the disc. A highlighted slice takes its share of the
/// last two away: a third in a page of three slices, measured at 66% and 67%, which at 70% closed the ring under an open wheel. Anything else holds the line at 30% at most, and the edge at 70%.
pub const WHEEL_EDGE: f32 = 0.85;
pub const WHEEL_UNIFORM: f32 = 0.45;
pub const WHEEL_LINE: f32 = 0.45;

/// How the wheel's place looks: (share of the rim that stands out from the world, share of the ring inside it of one colour, share of the rim's line brighter than the disc).
pub fn wheel_scores(part: &Part, client: Vec2) -> (f32, f32, f32) {
    let (center, radius) = zones::wheel(client);
    let ring = |r: f32| -> [[f32; 3]; WHEEL_ANGLES] {
        std::array::from_fn(|i| {
            let (s, c) = (i as f32 / WHEEL_ANGLES as f32 * std::f32::consts::TAU).sin_cos();
            part.at(center.x + radius * r * c, center.y + radius * r * s)
        })
    };
    let (inside, outside) = (ring(WHEEL_INSIDE), ring(WHEEL_OUTSIDE));
    let differ = |a: &[f32; 3], b: &[f32; 3]| (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs();
    let edge = inside.iter().zip(&outside).filter(|(a, b)| differ(a, b) > WHEEL_DIFFERENT).count() as f32 / WHEEL_ANGLES as f32;
    let middle: [f32; 3] = std::array::from_fn(|c| {
        let mut channel: [f32; WHEEL_ANGLES] = std::array::from_fn(|i| inside[i][c]);
        channel.sort_by(f32::total_cmp);
        channel[WHEEL_ANGLES / 2]
    });
    let uniform = inside.iter().filter(|a| differ(a, &middle) < WHEEL_SAME).count() as f32 / WHEEL_ANGLES as f32;
    let grey = |c: &[f32; 3]| (c[0] + c[1] + c[2]) / 3.0;
    let (near, on) = (ring(WHEEL_RIM[0]), ring(WHEEL_RIM[1]));
    let line = (0..WHEEL_ANGLES).filter(|&i| grey(&near[i]).max(grey(&on[i])) - grey(&inside[i]) > WHEEL_BRIGHTER).count() as f32 / WHEEL_ANGLES as f32;
    (edge, uniform, line)
}

/// The wheel's rim is in place, whatever is inside it yet. VRChat shows nothing for about 200 ms after R, then the rim is there at about 235 ms and all the rest
/// at about 300: right after R this is reason enough to start, and the whole of it must follow within [`TO_CONFIRM`].
pub fn wheel_coming(part: &Part, client: Vec2) -> bool {
    wheel_scores(part, client).0 >= WHEEL_EDGE
}

pub fn wheel_seen(part: &Part, client: Vec2) -> bool {
    let (edge, uniform, line) = wheel_scores(part, client);
    edge >= WHEEL_EDGE && uniform >= WHEEL_UNIFORM && line >= WHEEL_LINE
}

/// The icons' pictures: 7 of 24 x 24 grey bytes, averaged over real frames where each is not the selected page.
const ICONS: &[u8; 7 * ICON_PATCH * ICON_PATCH] = include_bytes!("qm_icons.bin");
pub const ICON_PATCH: usize = 24;
/// Half an icon's box, in the Quick Menu's units.
pub const ICON_HALF: f32 = 15.0;
/// Each frame's row is tried at these small shifts too (units): the scale's fit is within 1%, which is a few pixels that far down.
const SHIFTS: [f32; 3] = [-4.0, 0.0, 4.0];
pub const ICON_MATCH: f32 = 0.5;
pub const ICONS_NEEDED: usize = 4;

/// One icon's box from a frame, sampled to the template's size.
pub fn icon_patch(part: &Part, client: Vec2, icon: usize, dx: f32, dy: f32) -> [f32; ICON_PATCH * ICON_PATCH] {
    let (center, unit) = zones::quick_menu(client);
    let at = center + vec2(zones::QM_ICONS_X[icon] + dx, zones::QM_ICONS_Y + dy) * unit;
    std::array::from_fn(|i| {
        let (x, y) = (i % ICON_PATCH, i / ICON_PATCH);
        let g = |v: usize| ((v as f32 + 0.5) / ICON_PATCH as f32 * 2.0 - 1.0) * ICON_HALF * unit;
        part.grey(at.x + g(x), at.y + g(y))
    })
}

fn correlation(a: &[f32; ICON_PATCH * ICON_PATCH], b: &[u8]) -> f32 {
    let n = (ICON_PATCH * ICON_PATCH) as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().map(|v| *v as f32).sum::<f32>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x - ma, *y as f32 - mb);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    // A flat patch is no icon, whatever the rounding makes of it.
    if aa < n * 4.0 || bb < n {
        return 0.0;
    }
    ab / (aa * bb).sqrt()
}

/// Icon `icon`'s picture ([`ICONS`]).
pub fn icon_template(icon: usize) -> &'static [u8] {
    &ICONS[icon * ICON_PATCH * ICON_PATCH..(icon + 1) * ICON_PATCH * ICON_PATCH]
}

/// How much a patch sampled where icon `icon` should be looks like it (normalised correlation): in VR the patch is sampled through the eye's projection
/// (`qm_buttons`).
pub fn icon_correlation(patch: &[f32; ICON_PATCH * ICON_PATCH], icon: usize) -> f32 {
    correlation(patch, icon_template(icon))
}

/// How many of the row's icons are there, at the shift that finds most.
pub fn icons_found(part: &Part, client: Vec2) -> usize {
    let mut best = 0;
    for dy in SHIFTS {
        for dx in SHIFTS {
            let found = (0..zones::QM_ICONS_X.len())
                .filter(|&i| {
                    let template = &ICONS[i * ICON_PATCH * ICON_PATCH..(i + 1) * ICON_PATCH * ICON_PATCH];
                    correlation(&icon_patch(part, client, i, dx, dy), template) >= ICON_MATCH
                })
                .count();
            best = best.max(found);
        }
    }
    best
}

pub fn quick_menu_seen(part: &Part, client: Vec2) -> bool {
    icons_found(part, client) >= ICONS_NEEDED
}

/// What a look at a frame found.
pub fn look(wheel: &Part, row: &Part, client: Vec2) -> Option<Around> {
    // The wheel first: it is cheaper, and the Quick Menu's row is not under it when it shows.
    if wheel_seen(wheel, client) {
        Some(Around::Wheel)
    } else if quick_menu_seen(row, client) {
        Some(Around::QuickMenu)
    } else {
        None
    }
}

/// How long after a key a menu may take to show. VRChat's menus show within 300 ms of it on 60 frames a second (seen in the frames);
/// a hitch can take longer, and then the panels are late, not wrong.
pub const LOOK_AFTER_KEY: Duration = Duration::from_millis(1500);
/// Looks in a row it takes to say a menu opened, or went.
pub const LOOKS_TO_OPEN: u32 = 2;
pub const LOOKS_TO_CLOSE: u32 = 2;
/// A look while a menu shows and no key was pressed: three of them, about 180 ms, ride out a sub-page's slide. Just after a key the looks come at each new frame, so
/// that the panels follow VRChat's own menu at once.
pub const LOOK_EVERY: Duration = Duration::from_millis(60);
/// Looks in a row it takes to say a menu went with no key pressed: its sub-pages slide through frames that do not look like it.
pub const LOOKS_TO_CLOSE_UNASKED: u32 = 3;
/// With no new frame for this long nothing can be said, and open panels go (a frame of VRChat's comes at least every 100 ms at 10 frames a
/// second).
pub const NO_FRAME: Duration = Duration::from_millis(600);

/// How long a menu may still look like itself after the key that closes it: VRChat's own closing lasts about 80 ms for the wheel and 200 ms for the Quick
/// Menu. Past this, a menu that still shows did not close, and its panels come back.
pub const STILL_GOING: Duration = Duration::from_millis(350);

/// How long a wheel opened on its rim alone has to be seen whole.
pub const TO_CONFIRM: Duration = Duration::from_millis(400);

/// The keys that open and close VRChat's menus on the desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKey {
    Esc,
    R,
}

/// What VRChat's menus seem to be doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    /// Nothing to look for: no capture runs.
    Resting,
    /// A key was just pressed: looking, nothing shows yet.
    Looking,
    Open(Around),
}

/// From keys and frames to what shows (tested without any window). `key`, `frame` and `reset` are told as they happen.
#[derive(Debug)]
pub struct Watcher {
    seen: Seen,
    until: Option<Instant>,
    candidate: Option<Around>,
    hits: u32,
    misses: u32,
    last_frame: Option<Instant>,
    /// The menu a key just closed, and until when frames of it are its closing.
    going: Option<(Around, Instant)>,
    /// R was just pressed with nothing showing: the wheel's rim alone is enough to start.
    early: bool,
    /// Opened on the rim alone: by when the whole wheel must have been seen.
    confirm_by: Option<Instant>,
}

impl Default for Watcher {
    fn default() -> Self {
        Self { seen: Seen::Resting, until: None, candidate: None, hits: 0, misses: 0, last_frame: None, going: None, early: false, confirm_by: None }
    }
}

impl Watcher {
    pub fn seen(&self) -> Seen {
        self.seen
    }

    /// Whether the capture should run: there is something to look for.
    pub fn wants_frames(&self) -> bool {
        self.seen != Seen::Resting
    }

    /// How long since the last look before the next: none just after a key, which is when a menu opens or closes.
    pub fn look_every(&self, now: Instant) -> Duration {
        match self.seen {
            Seen::Open(_) if !self.until.is_some_and(|u| now < u) => LOOK_EVERY,
            _ => Duration::ZERO,
        }
    }

    /// A key pressed with the game in front, and what it does. Esc closes whichever shows, R the wheel: the panels go at that very moment rather than after
    /// VRChat's own closing. R over the Quick Menu closes nothing.
    pub fn pressed(&mut self, key: MenuKey, now: Instant) {
        self.key(now);
        self.going = None;
        self.early = key == MenuKey::R;
        if let Seen::Open(menu) = self.seen {
            self.early = false;
            if key == MenuKey::Esc || menu == Around::Wheel {
                self.going = Some((menu, now + STILL_GOING));
                self.seen = Seen::Looking;
                self.candidate = None;
                self.hits = 0;
                self.confirm_by = None;
            }
        }
    }

    /// A key was pressed with the game in front.
    fn key(&mut self, now: Instant) {
        self.until = Some(now + LOOK_AFTER_KEY);
        self.last_frame.get_or_insert(now);
        if self.seen == Seen::Resting {
            self.seen = Seen::Looking;
            self.candidate = None;
            self.hits = 0;
        }
    }

    /// A new frame's look. Returns what is seen now.
    pub fn frame(&mut self, found: Option<Around>, now: Instant) -> Seen {
        self.frame_coming(found, false, now)
    }

    /// A new frame's look, with whether the wheel's rim is in place ([`wheel_coming`]).
    pub fn frame_coming(&mut self, found: Option<Around>, coming: bool, now: Instant) -> Seen {
        self.last_frame = Some(now);
        match self.seen {
            Seen::Resting => {}
            Seen::Looking => {
                let found = found.filter(|f| !self.going.is_some_and(|(menu, until)| menu == *f && now < until));
                let on_rim = found.is_none() && coming && self.early;
                let found = if on_rim { Some(Around::Wheel) } else { found };
                if found.is_some() && found == self.candidate {
                    self.hits += 1;
                } else {
                    self.candidate = found;
                    self.hits = u32::from(found.is_some());
                }
                if self.hits >= LOOKS_TO_OPEN {
                    self.seen = Seen::Open(found.expect("hits count a found menu"));
                    self.misses = 0;
                    self.until = None;
                    self.early = false;
                    self.confirm_by = on_rim.then_some(now + TO_CONFIRM);
                }
            }
            Seen::Open(menu) => {
                if found == Some(menu) {
                    self.misses = 0;
                    self.confirm_by = None;
                } else if self.confirm_by.is_none() {
                    self.misses += 1;
                    let asked = self.until.is_some_and(|u| now < u);
                    if self.misses >= if asked { LOOKS_TO_CLOSE } else { LOOKS_TO_CLOSE_UNASKED } {
                        // Gone, or become the other one. After a key it goes on looking for what is left of that key's time: if the menu was still
                        // there it comes back, and the next menu does not need a new capture.
                        self.candidate = found;
                        self.hits = u32::from(found.is_some());
                        self.seen = if found.is_some() || asked { Seen::Looking } else { Seen::Resting };
                        self.until = if found.is_some() { Some(now + LOOK_AFTER_KEY) } else { self.until.filter(|_| asked) };
                        if self.until.is_none() {
                            self.last_frame = None;
                        }
                    }
                }
            }
        }
        self.tick(now)
    }

    /// Time passing: a look that found nothing ends; frames that stopped close what shows.
    pub fn tick(&mut self, now: Instant) -> Seen {
        match self.seen {
            Seen::Resting => {}
            Seen::Looking => {
                if self.until.is_none_or(|u| now >= u) {
                    self.reset();
                }
            }
            Seen::Open(_) => {
                if self.last_frame.is_none_or(|f| now.duration_since(f) >= NO_FRAME) || self.confirm_by.is_some_and(|by| now >= by) {
                    self.reset();
                }
            }
        }
        self.seen
    }

    /// The game went behind another window, closed, or the setting went off: nothing shows, nothing is looked for.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Real frames: `testdata/<what>_<width>x<height>_<x0>_<y0>.png`, the part of a real VRChat frame at that window size that
    /// a look reads (see `scripts/orbit-fixtures.py`), and whether it shows a menu by its first word: wheel, qm or none.
    fn frames() -> Vec<(String, Vec2, Vec<u8>, u32, u32, u32, u32)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/overlay/orbit/testdata");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).expect("the real frames") {
            let path = entry.unwrap().path();
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            // The folder also holds the camera tools' own pictures: only the frames named by what they show are read here.
            let shows = name.split('-').next().unwrap_or_default().split('_').next().unwrap_or_default();
            if path.extension().is_none_or(|e| e != "png") || !["wheel", "qm", "none"].contains(&shows) {
                continue;
            }
            let fields: Vec<&str> = name.split('_').collect();
            let size: Vec<f32> = fields[fields.len() - 3].split('x').map(|v| v.parse().unwrap()).collect();
            let (x0, y0): (u32, u32) = (fields[fields.len() - 2].parse().unwrap(), fields[fields.len() - 1].parse().unwrap());
            let image = image::open(&path).unwrap().to_rgba8();
            let (w, h) = image.dimensions();
            let bgra: Vec<u8> = image.pixels().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
            out.push((name, vec2(size[0], size[1]), bgra, x0, y0, w, h));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    #[test]
    fn real_frames_say_what_they_show() {
        let frames = frames();
        assert!(frames.len() >= 40, "{}", frames.len());
        for (name, client, bgra, x0, y0, width, height) in &frames {
            let part = Part { x0: *x0, y0: *y0, width: *width, height: *height, stride: width * 4, bgra };
            let kind = name.split(['-', '_']).next().unwrap();
            let row = name.contains("-row");
            if row {
                let found = icons_found(&part, *client);
                match kind {
                    "qm" => assert!(found >= ICONS_NEEDED, "{name}: {found} icons"),
                    _ => assert!(found < ICONS_NEEDED, "{name}: {found} icons on a frame without it"),
                }
            } else {
                let (edge, uniform, line) = wheel_scores(&part, *client);
                let seen = edge >= WHEEL_EDGE && uniform >= WHEEL_UNIFORM && line >= WHEEL_LINE;
                match kind {
                    "wheel" => {
                        assert!(seen, "{name}: {edge} {uniform} {line}");
                    }
                    _ => {
                        assert!(!seen, "{name}: {edge} {uniform} {line} on a frame without it");
                        assert!(!wheel_coming(&part, *client) || kind == "qm", "{name}: a rim at {edge} where there is none");
                    }
                }
            }
        }
    }

    #[test]
    fn looks_take_only_small_parts_of_a_frame() {
        let client = vec2(1920.0, 1080.0);
        let [wheel, row] = parts(client);
        let share = (wheel.area() + row.area()) / (client.x * client.y);
        assert!(share < 0.12, "{share}");
    }

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn a_key_alone_opens_nothing_and_ends_without_a_menu() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        assert_eq!(w.seen(), Seen::Resting);
        assert!(!w.wants_frames(), "no capture at rest");
        w.key(t0);
        assert_eq!(w.seen(), Seen::Looking);
        assert!(w.wants_frames());
        for ms in (0..1400).step_by(100) {
            assert_eq!(w.frame(None, at(t0, ms)), Seen::Looking, "at {ms}");
        }
        assert_eq!(w.tick(at(t0, 1600)), Seen::Resting);
        assert!(!w.wants_frames());
    }

    #[test]
    fn it_takes_two_looks_in_a_row_to_open_and_one_frame_is_no_menu() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.key(t0);
        assert_eq!(w.frame(Some(Around::Wheel), at(t0, 50)), Seen::Looking);
        assert_eq!(w.frame(None, at(t0, 100)), Seen::Looking, "only one, then nothing");
        assert_eq!(w.frame(Some(Around::QuickMenu), at(t0, 150)), Seen::Looking);
        assert_eq!(w.frame(Some(Around::Wheel), at(t0, 200)), Seen::Looking, "two different ones");
        assert_eq!(w.frame(Some(Around::Wheel), at(t0, 250)), Seen::Open(Around::Wheel));
    }

    #[test]
    fn it_goes_when_the_menu_goes_without_a_key() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.key(t0);
        w.frame(Some(Around::QuickMenu), at(t0, 50));
        assert_eq!(w.frame(Some(Around::QuickMenu), at(t0, 100)), Seen::Open(Around::QuickMenu));
        // It stays open long after the key, and through one bad frame.
        assert_eq!(w.frame(Some(Around::QuickMenu), at(t0, 9000)), Seen::Open(Around::QuickMenu));
        assert_eq!(w.frame(None, at(t0, 9100)), Seen::Open(Around::QuickMenu));
        assert_eq!(w.frame(Some(Around::QuickMenu), at(t0, 9200)), Seen::Open(Around::QuickMenu));
        // No key: the menu closed on its own, or a sub-page slides, which takes more looks.
        assert_eq!(w.frame(None, at(t0, 9300)), Seen::Open(Around::QuickMenu));
        assert_eq!(w.frame(None, at(t0, 9360)), Seen::Open(Around::QuickMenu));
        assert_eq!(w.frame(None, at(t0, 9420)), Seen::Resting);
        assert!(!w.wants_frames(), "the capture stops");
    }

    #[test]
    fn a_key_makes_it_follow_at_each_frame_and_only_then() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.key(t0);
        assert_eq!(w.look_every(at(t0, 10)), Duration::ZERO, "opening");
        w.frame(Some(Around::Wheel), at(t0, 200));
        // Two frames in a row are enough, and they come a frame apart.
        assert_eq!(w.frame(Some(Around::Wheel), at(t0, 216)), Seen::Open(Around::Wheel));
        assert_eq!(w.look_every(at(t0, 217)), LOOK_EVERY, "open, and nothing asked");
        // A key that does not close it by itself: two frames after the menu starts to go.
        w.key(at(t0, 5000));
        assert_eq!(w.look_every(at(t0, 5001)), Duration::ZERO);
        assert_eq!(w.frame(None, at(t0, 5050)), Seen::Open(Around::Wheel));
        assert_eq!(w.frame(None, at(t0, 5066)), Seen::Looking, "gone, with the capture kept for the next menu");
        assert_eq!(w.look_every(at(t0, 5067)), Duration::ZERO);
        assert!(w.wants_frames());
        // That time runs out, as any look does.
        assert_eq!(w.tick(at(t0, 6600)), Seen::Resting);
        assert!(!w.wants_frames());
    }

    #[test]
    fn the_key_that_closes_a_menu_takes_the_panels_at_once_through_its_closing() {
        for (menu, key) in [(Around::Wheel, MenuKey::R), (Around::Wheel, MenuKey::Esc), (Around::QuickMenu, MenuKey::Esc)] {
            let t0 = Instant::now();
            let mut w = Watcher::default();
            w.pressed(key, t0);
            w.frame(Some(menu), at(t0, 50));
            assert_eq!(w.frame(Some(menu), at(t0, 66)), Seen::Open(menu));
            // The panels go with the key, with no look at all.
            w.pressed(key, at(t0, 5000));
            assert_eq!(w.tick(at(t0, 5000)), Seen::Looking, "{menu:?}");
            // VRChat's menu still looks like itself for a while: that opens nothing.
            for ms in [5016, 5032, 5048, 5100, 5200, 5300] {
                assert_eq!(w.frame(Some(menu), at(t0, ms)), Seen::Looking, "{menu:?} at {ms}");
            }
            // Gone: the capture stays for what is left of the key's time, then rests.
            assert_eq!(w.frame(None, at(t0, 5340)), Seen::Looking);
            assert_eq!(w.tick(at(t0, 6600)), Seen::Resting);
        }
    }

    #[test]
    fn right_after_r_the_wheels_rim_starts_it_and_the_whole_of_it_must_follow() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.pressed(MenuKey::R, t0);
        assert_eq!(w.frame_coming(None, false, at(t0, 200)), Seen::Looking);
        assert_eq!(w.frame_coming(None, true, at(t0, 220)), Seen::Looking, "one frame is no wheel");
        assert_eq!(w.frame_coming(None, true, at(t0, 236)), Seen::Open(Around::Wheel));
        // Not whole yet: that is its opening, no reason to go.
        assert_eq!(w.frame_coming(None, true, at(t0, 260)), Seen::Open(Around::Wheel));
        assert_eq!(w.frame_coming(None, true, at(t0, 280)), Seen::Open(Around::Wheel));
        assert_eq!(w.frame_coming(None, true, at(t0, 296)), Seen::Open(Around::Wheel));
        assert_eq!(w.frame_coming(Some(Around::Wheel), true, at(t0, 310)), Seen::Open(Around::Wheel));
        // Whole once: from now on as any wheel, a long time on.
        assert_eq!(w.frame(None, at(t0, 800)), Seen::Open(Around::Wheel));
        assert_eq!(w.frame(Some(Around::Wheel), at(t0, 860)), Seen::Open(Around::Wheel));
    }

    #[test]
    fn a_rim_that_never_becomes_a_wheel_goes_again() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.pressed(MenuKey::R, t0);
        w.frame_coming(None, true, at(t0, 220));
        assert_eq!(w.frame_coming(None, true, at(t0, 236)), Seen::Open(Around::Wheel));
        assert_eq!(w.frame_coming(None, false, at(t0, 300)), Seen::Open(Around::Wheel));
        assert_eq!(w.frame_coming(None, false, at(t0, 236) + TO_CONFIRM), Seen::Resting);
    }

    #[test]
    fn only_r_with_nothing_showing_can_start_on_the_rim() {
        let t0 = Instant::now();
        // Esc opens no wheel.
        let mut w = Watcher::default();
        w.pressed(MenuKey::Esc, t0);
        w.frame_coming(None, true, at(t0, 100));
        assert_eq!(w.frame_coming(None, true, at(t0, 116)), Seen::Looking);
        // R over the Quick Menu opens no wheel.
        let mut w = Watcher::default();
        w.pressed(MenuKey::Esc, t0);
        w.frame(Some(Around::QuickMenu), at(t0, 50));
        w.frame(Some(Around::QuickMenu), at(t0, 66));
        w.frame(Some(Around::QuickMenu), at(t0, 500));
        w.pressed(MenuKey::R, at(t0, 510));
        w.frame_coming(Some(Around::QuickMenu), true, at(t0, 530));
        assert_eq!(w.frame_coming(Some(Around::QuickMenu), true, at(t0, 546)), Seen::Open(Around::QuickMenu));
        // The wheel R just closed: its rim stays through its closing, and opens nothing.
        let mut w = Watcher::default();
        w.pressed(MenuKey::R, t0);
        w.frame(Some(Around::Wheel), at(t0, 50));
        w.frame(Some(Around::Wheel), at(t0, 66));
        w.frame(Some(Around::Wheel), at(t0, 500));
        w.pressed(MenuKey::R, at(t0, 510));
        w.frame_coming(None, true, at(t0, 530));
        assert_eq!(w.frame_coming(None, true, at(t0, 546)), Seen::Looking);
    }

    #[test]
    fn a_menu_the_key_did_not_close_comes_back() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.pressed(MenuKey::Esc, t0);
        w.frame(Some(Around::QuickMenu), at(t0, 50));
        w.frame(Some(Around::QuickMenu), at(t0, 66));
        w.pressed(MenuKey::Esc, at(t0, 5000));
        w.frame(Some(Around::QuickMenu), at(t0, 5340));
        w.frame(Some(Around::QuickMenu), at(t0, 5356));
        assert_eq!(w.frame(Some(Around::QuickMenu), at(t0, 5372)), Seen::Open(Around::QuickMenu));
    }

    #[test]
    fn r_over_the_quick_menu_closes_nothing_and_esc_over_the_wheel_can_bring_it() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.pressed(MenuKey::Esc, t0);
        w.frame(Some(Around::QuickMenu), at(t0, 50));
        w.frame(Some(Around::QuickMenu), at(t0, 66));
        w.frame(Some(Around::QuickMenu), at(t0, 2990));
        w.pressed(MenuKey::R, at(t0, 3000));
        assert_eq!(w.tick(at(t0, 3000)), Seen::Open(Around::QuickMenu));
        assert_eq!(w.frame(Some(Around::QuickMenu), at(t0, 3016)), Seen::Open(Around::QuickMenu));

        let mut w = Watcher::default();
        w.pressed(MenuKey::R, t0);
        w.frame(Some(Around::Wheel), at(t0, 50));
        w.frame(Some(Around::Wheel), at(t0, 66));
        w.pressed(MenuKey::Esc, at(t0, 3000));
        // Another menu is no closing of this one.
        w.frame(Some(Around::QuickMenu), at(t0, 3100));
        assert_eq!(w.frame(Some(Around::QuickMenu), at(t0, 3116)), Seen::Open(Around::QuickMenu));
    }

    #[test]
    fn a_key_pressed_again_at_once_reopens_through_the_closing() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.pressed(MenuKey::R, t0);
        w.frame(Some(Around::Wheel), at(t0, 50));
        w.frame(Some(Around::Wheel), at(t0, 66));
        w.pressed(MenuKey::R, at(t0, 3000));
        w.pressed(MenuKey::R, at(t0, 3100));
        w.frame(Some(Around::Wheel), at(t0, 3300));
        assert_eq!(w.frame(Some(Around::Wheel), at(t0, 3316)), Seen::Open(Around::Wheel));
    }

    #[test]
    fn a_slice_pressed_right_after_opening_keeps_the_panels_through_its_slide() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.key(t0);
        w.frame(Some(Around::Wheel), at(t0, 200));
        w.frame(Some(Around::Wheel), at(t0, 216));
        // Well inside the time of the key that opened it: the slide of a sub-page is not taken for a close.
        assert_eq!(w.frame(None, at(t0, 300)), Seen::Open(Around::Wheel));
        assert_eq!(w.frame(None, at(t0, 360)), Seen::Open(Around::Wheel));
        assert_eq!(w.frame(Some(Around::Wheel), at(t0, 420)), Seen::Open(Around::Wheel));
    }

    #[test]
    fn when_frames_stop_nothing_shows() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.key(t0);
        w.frame(Some(Around::Wheel), at(t0, 50));
        w.frame(Some(Around::Wheel), at(t0, 100));
        assert_eq!(w.tick(at(t0, 500)), Seen::Open(Around::Wheel));
        assert_eq!(w.tick(at(t0, 100) + NO_FRAME), Seen::Resting, "stay away when not sure");
    }

    #[test]
    fn one_menu_can_become_the_other() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.key(t0);
        w.frame(Some(Around::Wheel), at(t0, 50));
        w.frame(Some(Around::Wheel), at(t0, 100));
        w.key(at(t0, 2000));
        assert_eq!(w.frame(Some(Around::QuickMenu), at(t0, 2100)), Seen::Open(Around::Wheel));
        assert_eq!(w.frame(Some(Around::QuickMenu), at(t0, 2200)), Seen::Looking, "the wheel's panels go first");
        assert_eq!(w.frame(Some(Around::QuickMenu), at(t0, 2300)), Seen::Open(Around::QuickMenu));
    }

    #[test]
    fn behind_another_window_everything_rests() {
        let t0 = Instant::now();
        let mut w = Watcher::default();
        w.key(t0);
        w.frame(Some(Around::Wheel), at(t0, 50));
        w.frame(Some(Around::Wheel), at(t0, 100));
        w.reset();
        assert_eq!(w.seen(), Seen::Resting);
        assert!(!w.wants_frames());
        assert_eq!(w.frame(Some(Around::Wheel), at(t0, 200)), Seen::Resting, "frames still in the pipe open nothing");
    }
}
