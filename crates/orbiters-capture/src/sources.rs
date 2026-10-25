//! Frame sources. Each lends its frames through [`FrameAccess`]: a Spout frame is read back from the GPU only as far
//! as the decoders ask (the strip and the skeleton band of most frames, all of a keyframe or a depth frame), and
//! decoded straight from mapped GPU memory without an extra copy.

use orbiters_contracts::image::{ImageView, PixelOrder, RgbaImage};
use orbiters_contracts::raw_capture::{self, RawCapture};
use orbiters_contracts::{depth, qoi, rig, skeleton, strip};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub enum Next {
    /// A frame was handed to the callback.
    Frame,
    /// Nothing arrived within the timeout.
    Timeout,
    /// The source has no more frames (raw sequences, finite test patterns).
    End,
}

/// One frame lent by a source.
pub trait FrameAccess {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    /// When the frame was seen, on the shared clock ([`orbiters_core::clock::now_ns`]).
    fn seen_ns(&self) -> u64;
    /// The picture with at least these ranges of rows (top-down as stored, ends excluded) of this frame. Only those
    /// rows are sure to be: read nothing else.
    fn rows(&mut self, ranges: &[(u32, u32)]) -> anyhow::Result<ImageView<'_>>;
    /// The whole picture.
    fn full(&mut self) -> anyhow::Result<ImageView<'_>>;
}

/// A frame held whole in memory (files, test patterns).
pub struct Whole<'a> {
    pub view: ImageView<'a>,
    pub seen_ns: u64,
}

impl<'a> Whole<'a> {
    pub fn new(view: ImageView<'a>) -> Self {
        Self { view, seen_ns: orbiters_core::clock::now_ns() }
    }
}

impl FrameAccess for Whole<'_> {
    fn width(&self) -> u32 {
        self.view.width
    }

    fn height(&self) -> u32 {
        self.view.height
    }

    fn seen_ns(&self) -> u64 {
        self.seen_ns
    }

    fn rows(&mut self, _ranges: &[(u32, u32)]) -> anyhow::Result<ImageView<'_>> {
        Ok(self.view)
    }

    fn full(&mut self) -> anyhow::Result<ImageView<'_>> {
        Ok(self.view)
    }
}

pub trait FrameSource: Send {
    /// Short human description ("Spout: VRChat", "Raw capture: Test 2026-10-20").
    fn describe(&self) -> String;
    /// Name for the session folder.
    fn folder_hint(&self) -> String;
    fn next(&mut self, timeout: Duration, f: &mut dyn FnMut(&mut dyn FrameAccess)) -> anyhow::Result<Next>;
    /// What the person should know about the source since the last call (a sender that cannot be opened, frames
    /// that could not be read): the source keeps going.
    fn warning(&mut self) -> Option<String> {
        None
    }
}

/// Where frames differ from one another: the capture strip's time (its payload's word 2, first copy) in a band of
/// block rows at the top of the picture, and the same band at the bottom for flipped render targets; and, for frames
/// without a strip (a kit switched off), eight one-pixel rows across the whole picture. Spout compares these pixels to
/// tell a new frame from the last one, under 2 % of the picture.
pub fn strip_probe(width: u32, height: u32) -> Vec<[u32; 4]> {
    let mut rects = Vec::new();
    if let Some(l) = strip::StripLayout::new(width, height) {
        let per_row = l.data_per_row().max(1);
        let (first, last) = (64 / per_row, 95 / per_row);
        let (y0, y1) = ((l.y0 + first * l.block).min(height), (l.y0 + (last + 1) * l.block).min(height));
        let (x0, x1) = (l.x0, (l.x0 + l.cols * l.block).min(width));
        rects.extend([[x0, y0, x1, y1], [x0, height - y1, x1, height - y0]]);
    }
    rects.extend((1..=8).map(|k| height * k / 9).filter(|&y| y < height).map(|y| [0, y, width, y + 1]));
    rects
}

// ---------------------------------------------------------------- Spout

/// Reconnection pauses: the first, and the longest while it keeps failing.
#[cfg(windows)]
const RETRY_FIRST: Duration = Duration::from_millis(500);
#[cfg(windows)]
const RETRY_MOST: Duration = Duration::from_secs(5);

#[cfg(windows)]
pub struct SpoutSource {
    receiver: orbiters_spout::Receiver,
    wanted: String,
    /// The sender read: the first one this session connected to, kept. A session never moves to another program's
    /// output (OBS's, the test pattern) when VRChat's goes away: it waits for it.
    pinned: Option<String>,
    connected: bool,
    /// When the next connection attempt may be made, and the pause after the next failure.
    retry_at: Option<Instant>,
    backoff: Duration,
    /// What went wrong last, not yet told.
    trouble: Option<String>,
}

#[cfg(windows)]
impl SpoutSource {
    pub fn new(name: &str) -> anyhow::Result<Self> {
        Ok(Self {
            receiver: orbiters_spout::Receiver::new()?,
            wanted: name.to_string(),
            pinned: None,
            connected: false,
            retry_at: None,
            backoff: RETRY_FIRST,
            trouble: None,
        })
    }

    /// The sender to read: the configured name, else VRChat's, else the active one, else the first.
    pub fn pick_sender(wanted: &str) -> Option<String> {
        let senders = orbiters_spout::list_senders();
        if !wanted.is_empty() {
            return senders.iter().find(|s| s.as_str() == wanted).cloned();
        }
        senders
            .iter()
            .find(|s| s.to_ascii_lowercase().contains("vrchat"))
            .cloned()
            .or_else(|| orbiters_spout::active_sender().filter(|a| senders.contains(a)))
            .or_else(|| senders.first().cloned())
    }

    fn try_connect(&mut self) -> bool {
        if self.retry_at.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        let name = match &self.pinned {
            Some(pinned) => Self::pick_sender(pinned),
            None => Self::pick_sender(&self.wanted),
        };
        let Some(name) = name else {
            // No sender yet: looked for again in a moment.
            self.retry_at = Some(Instant::now() + RETRY_FIRST);
            return false;
        };
        match self.receiver.connect(&name) {
            Ok(info) => {
                eprintln!("spout: connected to \"{}\" {}x{} (DXGI format {})", info.name, info.width, info.height, info.format);
                self.pinned = Some(name);
                self.connected = true;
                self.retry_at = None;
                self.backoff = RETRY_FIRST;
                true
            }
            Err(e) => {
                self.failed(format!("Spout: \"{name}\" could not be opened: {e}"));
                false
            }
        }
    }

    /// Something failed (a sender recreated at a size whose texture cannot be opened, a GPU reset, a format): the
    /// next attempt opens the sender again, after a pause that grows while it keeps failing.
    fn failed(&mut self, why: String) {
        eprintln!("{why}");
        self.connected = false;
        self.retry_at = Some(Instant::now() + self.backoff);
        self.backoff = (self.backoff * 2).min(RETRY_MOST);
        self.trouble = Some(why);
    }
}

/// A Spout frame being read back.
#[cfg(windows)]
struct SpoutFrame<'a, 'r> {
    reader: &'a mut orbiters_spout::FrameReader<'r>,
    seen_ns: u64,
}

#[cfg(windows)]
fn view_of(frame: orbiters_spout::Frame<'_>) -> anyhow::Result<ImageView<'_>> {
    let order = match frame.order {
        orbiters_spout::ChannelOrder::Rgba => PixelOrder::Rgba,
        orbiters_spout::ChannelOrder::Bgra => PixelOrder::Bgra,
    };
    Ok(ImageView::try_new(frame.width, frame.height, frame.stride, frame.data, order)?)
}

#[cfg(windows)]
impl FrameAccess for SpoutFrame<'_, '_> {
    fn width(&self) -> u32 {
        self.reader.width()
    }

    fn height(&self) -> u32 {
        self.reader.height()
    }

    fn seen_ns(&self) -> u64 {
        self.seen_ns
    }

    fn rows(&mut self, ranges: &[(u32, u32)]) -> anyhow::Result<ImageView<'_>> {
        view_of(self.reader.rows(ranges)?)
    }

    fn full(&mut self) -> anyhow::Result<ImageView<'_>> {
        view_of(self.reader.full()?)
    }
}

#[cfg(windows)]
impl FrameSource for SpoutSource {
    fn describe(&self) -> String {
        format!("Spout: {}", self.pinned.as_deref().unwrap_or(if self.wanted.is_empty() { "waiting for a sender" } else { &self.wanted }))
    }

    fn folder_hint(&self) -> String {
        self.pinned.clone().unwrap_or_else(|| "Spout".into())
    }

    fn next(&mut self, timeout: Duration, f: &mut dyn FnMut(&mut dyn FrameAccess)) -> anyhow::Result<Next> {
        if !self.connected && !self.try_connect() {
            std::thread::sleep(timeout.min(Duration::from_millis(100)));
            return Ok(Next::Timeout);
        }
        // The probe for the sender's size (and again for a sender the receiver opened again at another size).
        if let Some(info) = self.receiver.info() {
            let probe = strip_probe(info.width, info.height);
            self.receiver.set_probe(probe);
        }
        let result = self.receiver.receive_with(timeout, |reader| {
            // The time the frame was seen, on the shared clock.
            let seen_ns = orbiters_core::clock::now_ns().saturating_sub(reader.seen.elapsed().as_nanos() as u64);
            f(&mut SpoutFrame { reader, seen_ns });
        });
        match result {
            Ok(Some(())) => Ok(Next::Frame),
            Ok(None) => Ok(Next::Timeout),
            Err(orbiters_spout::SpoutError::NotFound(name)) => {
                eprintln!("spout: sender \"{name}\" went away");
                self.connected = false;
                Ok(Next::Timeout)
            }
            Err(e) => {
                self.failed(format!("Spout: reading \"{}\" failed: {e}", self.pinned.as_deref().unwrap_or("the sender")));
                Ok(Next::Timeout)
            }
        }
    }

    fn warning(&mut self) -> Option<String> {
        self.trouble.take()
    }
}

// ---------------------------------------------------------------- Raw capture folders

/// After this many unreadable frames in a row an import ends: the capture stopped early (fewer files than
/// `capture.json` says) or its disk went away.
const MAX_BAD_IN_A_ROW: u32 = 30;

/// A frame read and decoded ahead: its index and its pixels, or why it could not be read.
type Loaded = (u32, Result<Vec<u8>, String>);

pub struct RawSequenceSource {
    dir: PathBuf,
    meta: RawCapture,
    realtime: bool,
    started: Option<Instant>,
    /// Frames read and decoded ahead on a helper thread, in order: reading the next file overlaps processing this one.
    loaded: crossbeam_channel::Receiver<Loaded>,
    /// Frames that could not be read: how many in all, and in a row.
    skipped: u32,
    bad_in_a_row: u32,
    trouble: Option<String>,
}

impl RawSequenceSource {
    pub fn open(dir: &Path, realtime: bool) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(dir.join(raw_capture::RAW_FILE)).map_err(|e| anyhow::anyhow!("{}: no capture.json ({e})", dir.display()))?;
        let meta: RawCapture = serde_json::from_str(&text)?;
        anyhow::ensure!(meta.format == raw_capture::RAW_FORMAT, "{} is not a raw capture", dir.display());
        anyhow::ensure!(
            meta.width > 0 && meta.height > 0 && meta.width as u64 * meta.height as u64 <= qoi::MAX_PIXELS,
            "{}: capture.json gives a frame size of {}×{}",
            dir.display(),
            meta.width,
            meta.height
        );
        let (tx, loaded) = crossbeam_channel::bounded::<Loaded>(2);
        let (frames, count, (width, height)) = (dir.join(raw_capture::FRAMES_DIR), meta.frame_count, (meta.width, meta.height));
        std::thread::Builder::new().name("raw-frames".into()).spawn(move || {
            for index in 0..count {
                let path = frames.join(raw_capture::frame_file(index));
                // Each frame must be the size capture.json gives: a broken header cannot make the reader allocate more.
                let frame = std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|data| qoi::decode_rgba_sized(&data, width, height).map(|(_, rgba)| rgba).map_err(|e| e.to_string()))
                    .map_err(|e| format!("{}: {e}", path.display()));
                if tx.send((index, frame)).is_err() {
                    return;
                }
            }
        })?;
        Ok(Self { dir: dir.to_path_buf(), meta, realtime, started: None, loaded, skipped: 0, bad_in_a_row: 0, trouble: None })
    }
}

impl FrameSource for RawSequenceSource {
    fn describe(&self) -> String {
        format!("Raw capture: {}", self.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
    }

    fn folder_hint(&self) -> String {
        self.meta
            .world
            .as_ref()
            .and_then(|w| w.name.clone())
            .unwrap_or_else(|| self.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Raw capture".into()))
    }

    fn next(&mut self, _timeout: Duration, f: &mut dyn FnMut(&mut dyn FrameAccess)) -> anyhow::Result<Next> {
        loop {
            // The helper ends after the last frame.
            let Ok((index, frame)) = self.loaded.recv() else { return Ok(Next::End) };
            let rgba = match frame {
                Ok(rgba) => rgba,
                Err(why) => {
                    // A missing or broken frame is skipped and counted; too many in a row end the import.
                    self.skipped += 1;
                    self.bad_in_a_row += 1;
                    eprintln!("raw capture: frame {index} skipped: {why}");
                    self.trouble = Some(format!("{} frame(s) of the raw capture could not be read ({why})", self.skipped));
                    if self.bad_in_a_row >= MAX_BAD_IN_A_ROW {
                        self.trouble = Some(format!("The raw capture ends at frame {index}: {} frames in a row could not be read ({why})", self.bad_in_a_row));
                        return Ok(Next::End);
                    }
                    continue;
                }
            };
            self.bad_in_a_row = 0;
            if self.realtime && self.meta.fps > 0.0 {
                let start = *self.started.get_or_insert_with(Instant::now);
                let due = start + Duration::from_secs_f64(index as f64 / self.meta.fps as f64);
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
            }
            let (w, h) = (self.meta.width, self.meta.height);
            f(&mut Whole::new(ImageView::new(w, h, w as usize * 4, &rgba, PixelOrder::Rgba)));
            return Ok(Next::Frame);
        }
    }

    fn warning(&mut self) -> Option<String> {
        self.trouble.take()
    }
}

// ---------------------------------------------------------------- Test pattern

/// Generated frames: a simple scene with a camera circling it, a valid strip on every frame and a depth frame
/// every `depth_every` frames. Lets the whole pipeline run without VRChat or Unity.
pub struct TestPatternSource {
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub depth_every: u32,
    pub frames: u32,
    /// Odd colour frames are rig frames (contracts' Docs/rig-frames.md), the rig where the camera is; `late_face` (a
    /// face number) is stamped with the previous frame's time, as when its camera renders after VRChat's.
    pub rig: bool,
    pub late_face: Option<u32>,
    index: u32,
    started: Option<Instant>,
    paced: bool,
}

impl TestPatternSource {
    pub fn new(width: u32, height: u32, fps: f32, depth_every: u32, frames: u32, paced: bool) -> Self {
        Self { width, height, fps, depth_every, frames, rig: false, late_face: None, index: 0, started: None, paced }
    }

    /// The same with rig frames, `late_face` stamped late.
    pub fn with_rig(mut self, late_face: Option<u32>) -> Self {
        self.rig = true;
        self.late_face = late_face;
        self
    }

    /// Face `face`'s render at `t`: its own colour, a lighter square in the middle, the stamp.
    pub fn face_render(face: u32, t: f32) -> RgbaImage {
        let mut img = RgbaImage::new(128, 128);
        img.fill_rect(0, 0, 128, 128, [(face * 40) as u8, 120, 200 - (face * 20) as u8]);
        img.fill_rect(40, 40, 48, 48, [230, 220, 200]);
        rig::draw_stamp(&mut img, t);
        img
    }

    /// Camera on a 3 m circle at eye height, looking at the centre.
    pub fn pose(t: f32) -> [f32; 12] {
        let a = t * 0.4;
        let (s, c) = a.sin_cos();
        let pos = [3.0 * s, 1.6, -3.0 * c];
        // Forward points to the centre: (-s, 0, c). Right = up × forward, up = (0, 1, 0).
        let fwd = [-s, 0.0, c];
        let right = [fwd[2], 0.0, -fwd[0]];
        [right[0], 0.0, fwd[0], pos[0], right[1], 1.0, fwd[1], pos[1], right[2], 0.0, fwd[2], pos[2]]
    }

    /// The pose a capture-kit avatar would draw at `t`: walking around the centre, the left elbow swinging (jaw
    /// and upper chest missing, like many avatars).
    pub fn skeleton(t: f32) -> skeleton::Skeleton {
        let (s, c) = (t * 0.4).sin_cos();
        let mut pose = skeleton::Skeleton { time: t, hips: [2.0 * s, 0.95, -2.0 * c], avatar: 0x7E57, ..Default::default() };
        let yaw = |degrees: f32| {
            let (h, w) = (degrees.to_radians() * 0.5).sin_cos();
            [0.0, h, 0.0, w]
        };
        for (b, bone) in pose.bones.iter_mut().enumerate() {
            if b != 23 && b != 54 {
                *bone = Some(yaw(t * 22.9));
            }
        }
        let (h, w) = ((t * 3.0).sin() * 0.6).sin_cos();
        pose.bones[15] = Some([0.0, 0.0, h, w]);
        pose
    }

    pub fn render(&self, index: u32) -> RgbaImage {
        let t = index as f32 / self.fps.max(1.0);
        let pose = Self::pose(t);
        let depth_frame = self.depth_every > 0 && index % self.depth_every == self.depth_every - 1;
        let (w, h) = (self.width, self.height);
        if self.rig && !depth_frame && index % 2 == 1 {
            let mut img = RgbaImage::new(w, h);
            let faces: [RgbaImage; rig::FACES] = std::array::from_fn(|i| {
                let face = i as u32 + 1;
                let stamped = if self.late_face == Some(face) { t - 1.0 / self.fps.max(1.0) } else { t };
                Self::face_render(face, stamped)
            });
            if rig::encode(&mut img, &rig::payload(pose, t, w, h, 0, strip::flags::SOURCE_TEST), &faces).is_some() {
                return img;
            }
        }
        let mut img = RgbaImage::new(w, h);
        let shift = (t * 120.0) as u32;
        // Ground (bottom third) and wall stripes that slide with the camera, so frames differ.
        for y in 0..h {
            for x in 0..w {
                let ground = y > h * 2 / 3;
                let stripe = ((x + shift) / 64).is_multiple_of(2);
                let rgb = if ground {
                    if stripe {
                        [86, 70, 58]
                    } else {
                        [70, 58, 48]
                    }
                } else if stripe {
                    [70, 110, 180]
                } else {
                    [96, 140, 210]
                };
                img.set(x, y, rgb);
            }
        }
        if depth_frame {
            img.fill_rect(0, 0, w, h, [0, 0, 0]);
            let horizon = h as f32 * 0.5;
            depth::encode(&mut img, |_x, y| {
                let yf = y as f32;
                if yf < horizon {
                    8.0 + (horizon - yf) * 0.01
                } else {
                    1.6 * (h as f32 * 0.866) / (yf - horizon + 1.0)
                }
            });
        }
        let payload = strip::StripPayload {
            flags: strip::flags::compose(if depth_frame { strip::flags::KIND_DEPTH } else { strip::flags::KIND_COLOUR }, 0, 0, 0, strip::flags::SOURCE_TEST),
            time: t,
            camera_to_world: pose,
            projection: [1.732_050_8 * h as f32 / w as f32, 1.732_050_8, 0.0, 0.0],
            near: 0.05,
            far: 1000.0,
            screen_width: w,
            screen_height: h,
        };
        strip::encode(&mut img, &payload);
        if !depth_frame {
            skeleton::draw(&mut img, &Self::skeleton(t));
        }
        img
    }
}

impl FrameSource for TestPatternSource {
    fn describe(&self) -> String {
        format!("Test pattern {}x{}", self.width, self.height)
    }

    fn folder_hint(&self) -> String {
        "Test pattern".into()
    }

    fn next(&mut self, _timeout: Duration, f: &mut dyn FnMut(&mut dyn FrameAccess)) -> anyhow::Result<Next> {
        if self.frames > 0 && self.index >= self.frames {
            return Ok(Next::End);
        }
        if self.paced {
            let start = *self.started.get_or_insert_with(Instant::now);
            let due = start + Duration::from_secs_f64(self.index as f64 / self.fps.max(1.0) as f64);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        let img = self.render(self.index);
        self.index += 1;
        f(&mut Whole::new(img.view()));
        Ok(Next::Frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_frames_decode() {
        let src = TestPatternSource::new(1280, 720, 30.0, 4, 8, false);
        for i in 0..8 {
            let img = src.render(i);
            let d = strip::decode(&img.view()).unwrap();
            assert_eq!(d.payload.is_depth_frame(), i % 4 == 3);
            assert_eq!(d.payload.camera_to_world, TestPatternSource::pose(i as f32 / 30.0));
            if d.payload.is_depth_frame() {
                let dd = depth::decode(&img.view(), false).unwrap();
                assert_eq!(dd.tiles_rejected, 0);
            }
        }
    }

    #[test]
    fn colour_frames_carry_the_skeleton_band() {
        let src = TestPatternSource::new(1280, 720, 30.0, 4, 8, false);
        for i in 0..8 {
            let band = skeleton::decode(&src.render(i).view(), false).map(|b| b.result);
            if i % 4 == 3 {
                assert!(!matches!(band, Some(Ok(_))), "depth frame {i} read as a skeleton");
                continue;
            }
            let pose = band.expect("band").expect("pose");
            let want = TestPatternSource::skeleton(i as f32 / 30.0);
            assert_eq!((pose.time, pose.hips, pose.avatar, pose.mask()), (want.time, want.hips, want.avatar, want.mask()));
            for (got, want) in pose.bones.iter().zip(&want.bones) {
                if let (Some(a), Some(b)) = (got, want) {
                    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
                    assert!(dot.abs() > 0.9999, "frame {i}: {a:?} vs {b:?}");
                }
            }
        }
    }

    /// The probe Spout compares to tell frames apart covers the strip's time: two frames that differ by their time
    /// alone differ there, either way up, and it is a sliver of the picture.
    #[test]
    fn the_strip_probe_covers_the_strip_time() {
        let (w, h) = (1920u32, 1080u32);
        let probe = strip_probe(w, h);
        // The strip's two bands, and eight rows across the picture for frames without a strip.
        assert_eq!(probe.len(), 10);
        assert!(probe[2..].iter().all(|r| (r[0], r[2], r[3] - r[1]) == (0, w, 1)));
        assert_eq!(strip_probe(96, 54).len(), 8, "a frame too small for a strip still has its rows");
        let frame = |time: f32| {
            let mut img = RgbaImage::new(w, h);
            let payload = strip::StripPayload {
                flags: strip::flags::compose(strip::flags::KIND_COLOUR, 0, 0, 0, strip::flags::SOURCE_TEST),
                time,
                camera_to_world: TestPatternSource::pose(0.0),
                projection: [0.97, 1.73, 0.0, 0.0],
                near: 0.05,
                far: 1000.0,
                screen_width: w,
                screen_height: h,
            };
            strip::encode(&mut img, &payload).unwrap();
            img
        };
        let inside = |img: &RgbaImage, r: [u32; 4]| -> Vec<u8> { (r[1]..r[3]).flat_map(|y| (r[0]..r[2]).flat_map(move |x| img.get(x, y))).collect() };
        let (a, b) = (frame(100.0), frame(100.016_67));
        assert_ne!(inside(&a, probe[0]), inside(&b, probe[0]), "upright");
        assert_ne!(inside(&a.flipped_vertically(), probe[1]), inside(&b.flipped_vertically(), probe[1]), "flipped");
        let area: u32 = probe.iter().map(|r| (r[2] - r[0]) * (r[3] - r[1])).sum();
        assert!(area * 50 < w * h, "{area} pixels");
        // Without a strip, a picture that moves below the strip's rows still differs in the probe.
        let (mut c, mut d) = (RgbaImage::new(w, h), RgbaImage::new(w, h));
        c.fill_rect(0, h / 2, w, h / 2, [40, 80, 120]);
        d.fill_rect(0, h / 2 - 100, w, h / 2 + 100, [40, 80, 120]);
        assert!(probe.iter().any(|&r| inside(&c, r) != inside(&d, r)));
    }

    fn raw_capture(name: &str, frame_count: u32, frames: &[(u32, Option<Vec<u8>>)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orbiters-raw-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(raw_capture::FRAMES_DIR)).unwrap();
        let meta = RawCapture {
            format: raw_capture::RAW_FORMAT.into(),
            version: raw_capture::RAW_VERSION,
            width: 64,
            height: 64,
            fps: 30.0,
            frame_count,
            ..Default::default()
        };
        std::fs::write(dir.join(raw_capture::RAW_FILE), serde_json::to_vec(&meta).unwrap()).unwrap();
        let picture = qoi::encode(&vec![128u8; 64 * 64 * 4], 64, 64, 4);
        for (index, bytes) in frames {
            std::fs::write(dir.join(raw_capture::FRAMES_DIR).join(raw_capture::frame_file(*index)), bytes.clone().unwrap_or_else(|| picture.clone())).unwrap();
        }
        dir
    }

    fn read_all(src: &mut RawSequenceSource) -> u32 {
        let mut seen = 0;
        while !matches!(
            src.next(Duration::from_millis(10), &mut |f| {
                assert_eq!((f.width(), f.height()), (64, 64));
                seen += 1;
            }),
            Ok(Next::End)
        ) {}
        seen
    }

    /// A missing or broken frame (a file of another size than capture.json says, among them) is skipped and told; the
    /// import goes on to the end instead of retrying it forever.
    #[test]
    fn raw_captures_skip_frames_they_cannot_read() {
        let other_size = qoi::encode(&vec![0u8; 32 * 32 * 4], 32, 32, 4);
        let dir = raw_capture("skip", 6, &[(0, None), (1, None), (3, Some(other_size)), (4, Some(b"qoif garbage".to_vec())), (5, None)]);
        let mut src = RawSequenceSource::open(&dir, false).unwrap();
        assert_eq!(read_all(&mut src), 3);
        let told = src.warning().expect("told");
        assert!(told.contains("3 frame(s)"), "{told}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A capture that stopped early (fewer files than capture.json says) ends after a run of missing frames.
    #[test]
    fn a_raw_capture_that_stopped_early_ends() {
        let dir = raw_capture("short", 1000, &[(0, None), (1, None)]);
        let mut src = RawSequenceSource::open(&dir, false).unwrap();
        assert_eq!(read_all(&mut src), 2);
        assert!(src.warning().is_some_and(|w| w.contains("ends at frame")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pose_is_a_rotation() {
        let p = TestPatternSource::pose(1.3);
        let det = p[0] * (p[5] * p[10] - p[6] * p[9]) - p[1] * (p[4] * p[10] - p[6] * p[8]) + p[2] * (p[4] * p[9] - p[5] * p[8]);
        assert!((det - 1.0).abs() < 1e-5, "det {det}");
    }
}
