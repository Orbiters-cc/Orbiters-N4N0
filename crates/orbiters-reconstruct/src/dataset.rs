//! A session as training data: every keyframe (lossless, upright) at the training resolution with its exact
//! camera, every depth frame's exact samples with its camera, and the mask of what is not scene (the capture
//! strip and, with a capture kit, the skeleton band).

use crate::camera::Camera;
use anyhow::{bail, Context as _};
use orbiters_contracts::depth::{self, DepthLayout};
use orbiters_contracts::frame_record::{record_flags, FrameRecord};
use orbiters_contracts::manifest::depth_file;
use orbiters_contracts::{qoi, skeleton, strip};
use orbiters_core::bundle;
use std::path::{Path, PathBuf};

/// A keyframe: RGBA8 packed in a u32 (r in the low byte, encoded sRGB as captured, alpha unused), row-major, top row
/// first: the layout the GPU loss reads.
pub struct View {
    pub camera: Camera,
    pub image: Vec<u32>,
    pub game_time: f32,
    pub keyframe: i32,
}

/// A depth frame's samples: a grid of pixel positions (training resolution) and eye depths (0: none).
pub struct DepthView {
    pub camera: Camera,
    pub grid_width: u32,
    pub grid_height: u32,
    pub pixels: Vec<[f32; 2]>,
    pub depth: Vec<f32>,
    pub game_time: f32,
    /// Sample spacing in training pixels (for footprints), in a grid tile.
    pub spacing: f32,
    /// Per grid cell, that spacing's multiple between its sample and the next ones of its tile: 2, 4 or 8 in the larger
    /// tiles towards the frame's edges (orbiters_contracts::depth::DepthLayout::grid_steps), 0 where the grid cell
    /// carries no sample. Empty: 1 everywhere.
    pub steps: Vec<u8>,
    /// Samples that say nothing: the decoder could not read them (orbiters_contracts::depth::UNKNOWN), or depth
    /// validation distrusts them (crate::depthfix). Neither seeds nor depth targets. Empty: none.
    pub dynamic: Vec<bool>,
}

impl DepthView {
    pub fn valid(&self) -> usize {
        self.depth.iter().filter(|z| **z > 0.0).count()
    }

    pub fn is_dynamic(&self, i: usize) -> bool {
        self.dynamic.get(i).copied().unwrap_or(false)
    }

    /// Grid cells between sample `i` and the next ones of its tile (1 in a grid tile).
    pub fn step(&self, i: usize) -> u32 {
        self.steps.get(i).map_or(1, |&s| s.max(1) as u32)
    }
}

pub struct Dataset {
    pub dir: PathBuf,
    pub views: Vec<View>,
    pub depth_views: Vec<DepthView>,
    pub width: u32,
    pub height: u32,
    /// 1 where pixels are scene, 0 over the capture strip and the skeleton band (training resolution).
    pub mask: Vec<f32>,
}

/// Loads a session at a training width of at most `max_width` (an integer fraction of the captured width).
pub fn load(dir: &Path, max_width: u32, mut progress: impl FnMut(f32)) -> anyhow::Result<Dataset> {
    let manifest = bundle::read_manifest(dir).context("reading the session's manifest")?;
    let records: Vec<FrameRecord> = bundle::read_records(dir).context("reading frames.bin")?.into_iter().filter(|r| r.has(record_flags::STRIP_OK)).collect();
    let keyframes: Vec<&FrameRecord> = records.iter().filter(|r| r.has(record_flags::KEYFRAME) && r.keyframe_index >= 0).collect();
    let depth_frames: Vec<&FrameRecord> = records.iter().filter(|r| r.has(record_flags::DEPTH) && r.depth_index >= 0).collect();
    if keyframes.len() < 2 {
        bail!("This session has {} keyframes: reconstruction needs a few views of the world.", keyframes.len());
    }
    let (full_w, full_h) = (keyframes[0].width, keyframes[0].height);
    let factor = full_w.div_ceil(max_width.max(64)).max(1);
    let (w, h) = (full_w / factor, full_h / factor);
    let scale = w as f32 / full_w as f32;

    // Keyframes, decoded on every core.
    let threads = crate::priority::loading_threads();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let total = keyframes.len() + depth_frames.len();
    let chunks: Vec<&[&FrameRecord]> = keyframes.chunks(keyframes.len().div_ceil(threads)).collect();
    let mut views: Vec<View> = Vec::with_capacity(keyframes.len());
    let mut failed = None;
    std::thread::scope(|scope| {
        let handles: Vec<_> = chunks
            .iter()
            .map(|chunk| {
                let done = &done;
                scope.spawn(move || -> anyhow::Result<Vec<View>> {
                    let mut out = Vec::with_capacity(chunk.len());
                    for r in chunk.iter() {
                        let path = dir.join("keyframes").join(bundle::keyframe_file(r.keyframe_index));
                        let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
                        let (head, rgba) = qoi::decode_rgba(&data).map_err(|e| anyhow::anyhow!("{}: {e:?}", path.display()))?;
                        if (head.width, head.height) != (full_w, full_h) {
                            bail!("keyframe {} is {}×{}, the others {full_w}×{full_h}", r.keyframe_index, head.width, head.height);
                        }
                        out.push(View {
                            camera: Camera::from_strip(&r.camera_to_world, &r.projection, full_w, full_h, scale),
                            image: downscale(&rgba, full_w, full_h, factor),
                            game_time: r.game_time,
                            keyframe: r.keyframe_index,
                        });
                        done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Ok(out)
                })
            })
            .collect();
        loop {
            progress(done.load(std::sync::atomic::Ordering::Relaxed) as f32 / total as f32);
            if handles.iter().all(|h| h.is_finished()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        for h in handles {
            match h.join().expect("keyframe thread") {
                Ok(v) => views.extend(v),
                Err(e) => failed = Some(e),
            }
        }
    });
    if let Some(e) = failed {
        return Err(e);
    }
    views.sort_by_key(|v| v.keyframe);

    let mut depth_views = Vec::with_capacity(depth_frames.len());
    // Every depth frame of a session has the same size, so one step map does.
    let mut steps: Option<(u32, u32, Vec<u8>)> = None;
    for (i, r) in depth_frames.iter().enumerate() {
        let path = dir.join("depth").join(bundle::depth_file(r.depth_index));
        let Ok(data) = std::fs::read(&path) else { continue };
        let Some((gw, gh, fw, fh, codes)) = depth_file::read(&data) else { continue };
        let Some(layout) = DepthLayout::new(fw, fh) else { continue };
        if steps.as_ref().is_none_or(|(sw, sh, _)| (*sw, *sh) != (fw, fh)) {
            steps = Some((fw, fh, layout.grid_steps()));
        }
        let frame_steps = steps.as_ref().map(|(_, _, v)| v).filter(|v| v.len() == codes.len()).cloned().unwrap_or_default();
        let s = w as f32 / fw as f32;
        let mut pixels = Vec::with_capacity(codes.len());
        let mut depths = Vec::with_capacity(codes.len());
        let mut unknown = Vec::with_capacity(codes.len());
        for gy in 0..gh {
            for gx in 0..gw {
                let (px, py) = layout.grid_pixel(gx, gy);
                pixels.push([(px as f32 + 0.5) * s - 0.5, (py as f32 + 0.5) * s - 0.5]);
                let code = codes[(gy * gw + gx) as usize];
                depths.push(depth::dequantize(code).unwrap_or(0.0));
                unknown.push(code == depth::UNKNOWN);
            }
        }
        depth_views.push(DepthView {
            camera: Camera::from_strip(&r.camera_to_world, &r.projection, fw, fh, s),
            grid_width: gw,
            grid_height: gh,
            pixels,
            depth: depths,
            game_time: r.game_time,
            spacing: layout.tile_px as f32 / depth::SAMPLES_X as f32 * s,
            steps: frame_steps,
            dynamic: if unknown.contains(&true) { unknown } else { Vec::new() },
        });
        progress((keyframes.len() + i + 1) as f32 / total as f32);
    }

    // What is not scene: the strip (and patches), and the skeleton band when the session read one, came from the
    // capture kit on an avatar (it draws the band into every colour frame), or the keyframes show it. A world whose
    // post-processing breaks the band's symbols (chromatic aberration at the bottom of the frame) decodes no skeleton
    // frame, and an unmasked band trains its codes as bright specks on the floor ahead of every camera.
    let kit = records.iter().any(|r| strip::flags::source(r.strip_flags) == strip::flags::SOURCE_AVATAR);
    let mut mask = vec![1.0f32; (w * h) as usize];
    let mut cut = |x0: u32, y0: u32, x1: u32, y1: u32| {
        let (x0, y0) = ((x0 as f32 * scale).floor() as u32, (y0 as f32 * scale).floor() as u32);
        let (x1, y1) = (((x1 as f32 * scale).ceil() as u32).min(w), ((y1 as f32 * scale).ceil() as u32).min(h));
        for y in y0..y1 {
            for x in x0..x1 {
                mask[(y * w + x) as usize] = 0.0;
            }
        }
    };
    if let Some(l) = strip::StripLayout::new(full_w, full_h) {
        cut(l.reserved.x0, l.reserved.y0, l.reserved.x1, l.reserved.y1);
    }
    if manifest.performance.skeleton_frames > 0 || kit || band_drawn(&views, w, h, scale, full_w, full_h) {
        if let Some(b) = skeleton::BandLayout::new(full_w, full_h) {
            let r = b.reserved();
            cut(r.x0, r.y0, r.x1, r.y1);
        }
    }
    Ok(Dataset { dir: dir.to_path_buf(), views, depth_views, width: w, height: h, mask })
}

/// Whether most keyframes show the skeleton band: the pilot rows of its tiles (levels 0, 1, 2, 3 repeated) follow
/// that ramp. Read in the red channel, which PPv2's chromatic aberration smears least.
fn band_drawn(views: &[View], w: u32, h: u32, scale: f32, full_w: u32, full_h: u32) -> bool {
    let Some(b) = skeleton::BandLayout::new(full_w, full_h) else { return false };
    let ramp: Vec<f32> = (0..depth::TILE_CELLS).map(|j| (j % depth::LEVELS as u32) as f32).collect();
    let sampled: Vec<&View> = views.iter().step_by((views.len() / 16).max(1)).collect();
    let shows = |v: &View| {
        let tiles = (b.per_row * b.rows) as usize;
        let mut sum = 0.0;
        for p in 0..tiles {
            let (x0, y0) = b.tile_origin(p);
            let cells: Vec<f32> = (0..depth::TILE_CELLS)
                .map(|j| {
                    let x = (((x0 + j * b.cell) as f32 + b.cell as f32 / 2.0) * scale) as u32;
                    let y = ((y0 as f32 + b.cell as f32 / 2.0) * scale) as u32;
                    channel(v.image[(y.min(h - 1) * w + x.min(w - 1)) as usize], 0)
                })
                .collect();
            sum += correlation(&cells, &ramp);
        }
        sum / tiles as f32 > 0.5
    };
    2 * sampled.iter().filter(|v| shows(v)).count() > sampled.len()
}

fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        ab += (x - ma) * (y - mb);
        aa += (x - ma) * (x - ma);
        bb += (y - mb) * (y - mb);
    }
    if aa <= 1e-9 || bb <= 1e-9 {
        0.0
    } else {
        ab / (aa * bb).sqrt()
    }
}

/// Box-filters RGBA to RGB floats by an integer `factor`.
fn downscale(rgba: &[u8], width: u32, height: u32, factor: u32) -> Vec<u32> {
    let (w, h) = (width / factor, height / factor);
    let mut out = vec![0u32; (w * h) as usize];
    let n = factor * factor;
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0u32; 3];
            for dy in 0..factor {
                let row = ((y * factor + dy) * width + x * factor) as usize * 4;
                for dx in 0..factor as usize {
                    let p = &rgba[row + dx * 4..row + dx * 4 + 3];
                    acc[0] += p[0] as u32;
                    acc[1] += p[1] as u32;
                    acc[2] += p[2] as u32;
                }
            }
            let [r, g, b] = acc.map(|a| (a + n / 2) / n);
            out[(y * w + x) as usize] = r | (g << 8) | (b << 16) | (255 << 24);
        }
    }
    out
}

/// One channel (0 red, 1 green, 2 blue) of a packed pixel, in 0..1.
pub fn channel(pixel: u32, c: usize) -> f32 {
    ((pixel >> (8 * c)) & 255) as f32 / 255.0
}

/// Bilinear sample of a keyframe at pixel coordinates (pixel centres at integers), RGB in 0..1; `None` outside.
pub fn sample_rgb(image: &[u32], width: u32, height: u32, u: f32, v: f32) -> Option<[f32; 3]> {
    if !(u >= 0.0 && v >= 0.0 && u <= (width - 1) as f32 && v <= (height - 1) as f32) {
        return None;
    }
    let (x0, y0) = (u.floor() as u32, v.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let at = |x: u32, y: u32, c: usize| channel(image[(y * width + x) as usize], c);
    let mut out = [0f32; 3];
    for (c, o) in out.iter_mut().enumerate() {
        let top = at(x0, y0, c) * (1.0 - fx) + at(x1, y0, c) * fx;
        let bottom = at(x0, y1, c) * (1.0 - fx) + at(x1, y1, c) * fx;
        *o = top * (1.0 - fy) + bottom * fy;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orbiters_contracts::image::RgbaImage;

    fn view(img: &RgbaImage) -> View {
        let (w, h) = (img.width, img.height);
        let image = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| {
            let p = img.get(x, y);
            p[0] as u32 | (p[1] as u32) << 8 | (p[2] as u32) << 16 | 255 << 24
        });
        View { camera: Camera::look([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.0, w, h), image: image.collect(), game_time: 0.0, keyframe: 0 }
    }

    /// The band is found in pictures whose band does not decode: with green and blue displaced towards the centre
    /// (chromatic aberration), and not in pictures without one.
    #[test]
    fn band_found_without_decoding() {
        let (w, h) = (1920u32, 1080u32);
        let mut rng = fastrand::Rng::with_seed(5);
        let noise = |rng: &mut fastrand::Rng| {
            let mut img = RgbaImage::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    img.set(x, y, [rng.u8(..), rng.u8(..), rng.u8(..)]);
                }
            }
            img
        };
        let skeleton = skeleton::Skeleton { time: 3.5, hips: [0.0, 1.0, 0.0], avatar: 7, ..Default::default() };
        let (mut plain, mut banded, mut smeared) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..4 {
            let img = noise(&mut rng);
            plain.push(view(&img));
            let mut b = img;
            skeleton::draw(&mut b, &skeleton).expect("band");
            banded.push(view(&b));
            // Green taken from 4 px and blue from 8 px nearer the centre, as chromatic aberration does.
            let mut s = b.clone();
            for y in 0..h {
                for x in 0..w {
                    let towards = |d: i64| (x as i64 + if x < w / 2 { d } else { -d }).clamp(0, w as i64 - 1) as u32;
                    let (g, bl) = (b.get(towards(4), y)[1], b.get(towards(8), y)[2]);
                    let r = b.get(x, y)[0];
                    s.set(x, y, [r, g, bl]);
                }
            }
            smeared.push(view(&s));
        }
        assert!(!band_drawn(&plain, w, h, 1.0, w, h));
        assert!(band_drawn(&banded, w, h, 1.0, w, h));
        assert!(band_drawn(&smeared, w, h, 1.0, w, h));
        // At half the training resolution too.
        let half: Vec<View> = banded
            .iter()
            .map(|v| View { camera: v.camera, image: downscale(&v.image.iter().flat_map(|p| p.to_le_bytes()).collect::<Vec<u8>>(), w, h, 2), game_time: 0.0, keyframe: 0 })
            .collect();
        assert!(band_drawn(&half, w / 2, h / 2, 0.5, w, h));
    }
}
