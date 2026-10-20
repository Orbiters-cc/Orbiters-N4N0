//! Frame sources. Each lends frames as borrowed [`ImageView`]s so Spout frames are decoded straight from
//! mapped GPU memory without an extra copy.

use orbiters_contracts::image::{ImageView, PixelOrder, RgbaImage};
use orbiters_contracts::raw_capture::{self, RawCapture};
use orbiters_contracts::{depth, qoi, skeleton, strip};
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

pub trait FrameSource: Send {
    /// Short human description ("Spout: VRChat", "Raw capture: Test 2026-10-20").
    fn describe(&self) -> String;
    /// Name for the session folder.
    fn folder_hint(&self) -> String;
    fn next(&mut self, timeout: Duration, f: &mut dyn FnMut(ImageView<'_>)) -> anyhow::Result<Next>;
}

// ---------------------------------------------------------------- Spout

#[cfg(windows)]
pub struct SpoutSource {
    receiver: orbiters_spout::Receiver,
    wanted: String,
    connected: Option<String>,
    last_attempt: Option<Instant>,
}

#[cfg(windows)]
impl SpoutSource {
    pub fn new(name: &str) -> anyhow::Result<Self> {
        Ok(Self { receiver: orbiters_spout::Receiver::new()?, wanted: name.to_string(), connected: None, last_attempt: None })
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

    fn try_connect(&mut self) -> anyhow::Result<bool> {
        if self.last_attempt.map(|t| t.elapsed() < Duration::from_millis(500)).unwrap_or(false) {
            return Ok(false);
        }
        self.last_attempt = Some(Instant::now());
        let Some(name) = Self::pick_sender(&self.wanted) else { return Ok(false) };
        let info = self.receiver.connect(&name)?;
        eprintln!("spout: connected to \"{}\" {}x{} (DXGI format {})", info.name, info.width, info.height, info.format);
        self.connected = Some(name);
        Ok(true)
    }
}

#[cfg(windows)]
impl FrameSource for SpoutSource {
    fn describe(&self) -> String {
        format!("Spout: {}", self.connected.as_deref().unwrap_or(if self.wanted.is_empty() { "waiting for a sender" } else { &self.wanted }))
    }

    fn folder_hint(&self) -> String {
        self.connected.clone().unwrap_or_else(|| "Spout".into())
    }

    fn next(&mut self, timeout: Duration, f: &mut dyn FnMut(ImageView<'_>)) -> anyhow::Result<Next> {
        if self.connected.is_none() && !self.try_connect()? {
            std::thread::sleep(timeout.min(Duration::from_millis(100)));
            return Ok(Next::Timeout);
        }
        let result = self.receiver.receive(timeout, |frame| {
            let order = match frame.order {
                orbiters_spout::ChannelOrder::Rgba => PixelOrder::Rgba,
                orbiters_spout::ChannelOrder::Bgra => PixelOrder::Bgra,
            };
            f(ImageView::new(frame.width, frame.height, frame.stride, frame.data, order));
        });
        match result {
            Ok(Some(())) => Ok(Next::Frame),
            Ok(None) => Ok(Next::Timeout),
            Err(orbiters_spout::SpoutError::NotFound(name)) => {
                eprintln!("spout: sender \"{name}\" went away");
                self.connected = None;
                Ok(Next::Timeout)
            }
            Err(e) => Err(e.into()),
        }
    }
}

// ---------------------------------------------------------------- Raw capture folders

pub struct RawSequenceSource {
    dir: PathBuf,
    meta: RawCapture,
    index: u32,
    realtime: bool,
    started: Option<Instant>,
}

impl RawSequenceSource {
    pub fn open(dir: &Path, realtime: bool) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(dir.join(raw_capture::RAW_FILE)).map_err(|e| anyhow::anyhow!("{}: no capture.json ({e})", dir.display()))?;
        let meta: RawCapture = serde_json::from_str(&text)?;
        anyhow::ensure!(meta.format == raw_capture::RAW_FORMAT, "{} is not a raw capture", dir.display());
        Ok(Self { dir: dir.to_path_buf(), meta, index: 0, realtime, started: None })
    }

    pub fn frame_path(&self, index: u32) -> PathBuf {
        self.dir.join(raw_capture::FRAMES_DIR).join(raw_capture::frame_file(index))
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

    fn next(&mut self, _timeout: Duration, f: &mut dyn FnMut(ImageView<'_>)) -> anyhow::Result<Next> {
        if self.index >= self.meta.frame_count {
            return Ok(Next::End);
        }
        let path = self.frame_path(self.index);
        let data = std::fs::read(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        let (h, rgba) = qoi::decode_rgba(&data)?;
        if self.realtime && self.meta.fps > 0.0 {
            let start = *self.started.get_or_insert_with(Instant::now);
            let due = start + Duration::from_secs_f64(self.index as f64 / self.meta.fps as f64);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
        }
        self.index += 1;
        f(ImageView::new(h.width, h.height, h.width as usize * 4, &rgba, PixelOrder::Rgba));
        Ok(Next::Frame)
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
    index: u32,
    started: Option<Instant>,
    paced: bool,
}

impl TestPatternSource {
    pub fn new(width: u32, height: u32, fps: f32, depth_every: u32, frames: u32, paced: bool) -> Self {
        Self { width, height, fps, depth_every, frames, index: 0, started: None, paced }
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

    fn next(&mut self, _timeout: Duration, f: &mut dyn FnMut(ImageView<'_>)) -> anyhow::Result<Next> {
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
        f(img.view());
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

    #[test]
    fn pose_is_a_rotation() {
        let p = TestPatternSource::pose(1.3);
        let det = p[0] * (p[5] * p[10] - p[6] * p[9]) - p[1] * (p[4] * p[10] - p[6] * p[8]) + p[2] * (p[4] * p[9] - p[5] * p[8]);
        assert!((det - 1.0).abs() < 1e-5, "det {det}");
    }
}
