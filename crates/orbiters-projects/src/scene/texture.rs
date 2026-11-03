//! Textures as a preview stores them: read from the project's source files (PNG, JPEG, TGA, BMP, TIFF, GIF and
//! Photoshop's PSD and PSB, whose merged picture Unity imports), made smaller to the preview's budget and written as
//! JPEG (opaque) or PNG (alpha that matters), as Orbiters Toolkit does from the GPU copy.
//!
//! Decoding is bounded: files over [`super::Limits::max_texture_bytes`] and pictures over 8192 pixels on a side are
//! left out (the material keeps its colour), so one odd 16k texture cannot hold a preview or fill the memory.

use anyhow::{bail, ensure, Context as _};
use image::RgbaImage;
use std::path::Path;

const MAX_SIDE: u32 = 8192;

/// A picture ready for the preview file.
pub struct Encoded {
    pub width: u32,
    pub height: u32,
    pub png: bool,
    pub bytes: Vec<u8>,
    pub has_alpha: bool,
}

/// Reads a texture file whole (any size up to [`MAX_SIDE`]).
pub fn decode(path: &Path, max_bytes: u64) -> anyhow::Result<RgbaImage> {
    let size = std::fs::metadata(path)?.len();
    ensure!(size <= max_bytes, "{} MB", size >> 20);
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext == "psd" || ext == "psb" {
        let bytes = std::fs::read(path)?;
        return psd(&bytes);
    }
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(512 << 20);
    reader.limits(limits);
    Ok(reader.decode().with_context(|| format!("a .{ext} the preview cannot read"))?.to_rgba8())
}

/// `image` made to fit `max_side` (never larger) and encoded: PNG when `keep_alpha` and some pixel is not opaque.
pub fn encode(image: &RgbaImage, max_side: u32, keep_alpha: bool) -> anyhow::Result<Encoded> {
    let (w, h) = image.dimensions();
    let scale = (max_side as f32 / w.max(h) as f32).min(1.0);
    let (nw, nh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
    let small = if (nw, nh) == (w, h) { image.clone() } else { image::imageops::thumbnail(image, nw, nh) };
    let has_alpha = small.pixels().any(|p| p[3] < 250);
    let mut bytes = Vec::new();
    let png = keep_alpha && has_alpha;
    if png {
        image::codecs::png::PngEncoder::new_with_quality(&mut bytes, image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::Adaptive)
            .write_image(small.as_raw(), nw, nh, image::ExtendedColorType::Rgba8)?;
    } else {
        let rgb: Vec<u8> = small.pixels().flat_map(|p| [p[0], p[1], p[2]]).collect();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90).encode(&rgb, nw, nh, image::ExtendedColorType::Rgb8)?;
    }
    Ok(Encoded { width: nw, height: nh, png, bytes, has_alpha })
}

use image::ImageEncoder as _;

/// Photoshop's merged picture (what Unity imports of a PSD/PSB): 8, 16 or 32 bits a channel, grey or RGB (with
/// the merged alpha when there is a fourth channel), raw or PackBits-compressed.
pub fn psd(b: &[u8]) -> anyhow::Result<RgbaImage> {
    let be16 = |at: usize| -> anyhow::Result<u16> { Ok(u16::from_be_bytes(b.get(at..at + 2).context("cut short")?.try_into()?)) };
    let be32 = |at: usize| -> anyhow::Result<u32> { Ok(u32::from_be_bytes(b.get(at..at + 4).context("cut short")?.try_into()?)) };
    let be64 = |at: usize| -> anyhow::Result<u64> { Ok(u64::from_be_bytes(b.get(at..at + 8).context("cut short")?.try_into()?)) };
    ensure!(b.len() >= 26 && &b[..4] == b"8BPS", "not a Photoshop file");
    let version = be16(4)?;
    ensure!(version == 1 || version == 2, "Photoshop version {version}");
    let channels = be16(12)? as usize;
    let height = be32(14)?;
    let width = be32(18)?;
    let depth = be16(22)?;
    let mode = be16(24)?;
    ensure!(width > 0 && height > 0 && width <= MAX_SIDE && height <= MAX_SIDE, "{width}×{height} pixels");
    ensure!(matches!(depth, 8 | 16 | 32), "{depth} bits a channel");
    ensure!(matches!(mode, 1 | 3), "colour mode {mode} (only grey and RGB are read)");
    let mut at = 26usize;
    at += 4 + be32(at)? as usize; // colour mode data
    at += 4 + be32(at)? as usize; // image resources
    at += if version == 2 { 8 + be64(at)? as usize } else { 4 + be32(at)? as usize }; // layers and masks
    let compression = be16(at)?;
    at += 2;
    let (w, h) = (width as usize, height as usize);
    let bytes_per = depth as usize / 8;
    let row = w * bytes_per;
    let wanted = if mode == 3 { channels.min(4) } else { channels.min(2) };
    ensure!(wanted >= 1, "no channels");
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(wanted);
    match compression {
        0 => {
            for c in 0..wanted {
                let start = at + c * row * h;
                planes.push(b.get(start..start + row * h).context("cut short")?.to_vec());
            }
        }
        1 => {
            let count_size = if version == 2 { 4 } else { 2 };
            let counts_at = at;
            let mut data = at + channels * h * count_size;
            for c in 0..channels {
                let mut plane = Vec::with_capacity(row * h);
                for y in 0..h {
                    let k = counts_at + (c * h + y) * count_size;
                    let n = if version == 2 { be32(k)? as usize } else { be16(k)? as usize };
                    if c < wanted {
                        let src = b.get(data..data + n).context("cut short")?;
                        packbits(src, row, &mut plane);
                    }
                    data += n;
                }
                if c < wanted {
                    planes.push(plane);
                } else {
                    break;
                }
            }
        }
        other => bail!("Photoshop compression {other} (save with Maximize Compatibility)"),
    }
    let sample = |plane: &[u8], i: usize| -> u8 {
        match depth {
            8 => plane[i],
            16 => plane[i * 2],
            _ => {
                let v = f32::from_be_bytes([plane[i * 4], plane[i * 4 + 1], plane[i * 4 + 2], plane[i * 4 + 3]]);
                (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8
            }
        }
    };
    let mut out = RgbaImage::new(width, height);
    for (i, px) in out.pixels_mut().enumerate() {
        *px = if mode == 3 && planes.len() >= 3 {
            image::Rgba([sample(&planes[0], i), sample(&planes[1], i), sample(&planes[2], i), planes.get(3).map_or(255, |a| sample(a, i))])
        } else {
            let g = sample(&planes[0], i);
            image::Rgba([g, g, g, planes.get(1).map_or(255, |a| sample(a, i))])
        };
    }
    Ok(out)
}

/// PackBits: a count byte, then that many bytes plus one, or one byte repeated 257 minus the count times.
fn packbits(src: &[u8], row: usize, out: &mut Vec<u8>) {
    let start = out.len();
    let mut i = 0;
    while i < src.len() && out.len() - start < row {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let len = n as usize + 1;
            let end = (i + len).min(src.len());
            out.extend_from_slice(&src[i..end]);
            i = end;
        } else if n != -128 {
            let len = 1 - n as isize;
            if let Some(&v) = src.get(i) {
                out.extend(std::iter::repeat_n(v, len as usize));
            }
            i += 1;
        }
    }
    out.resize(start + row, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 RGB PSD, RLE-compressed: red, green / blue, white.
    fn tiny_psd(rle: bool) -> Vec<u8> {
        let mut b = b"8BPS".to_vec();
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&[0; 6]);
        b.extend_from_slice(&3u16.to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(&2u32.to_be_bytes());
        b.extend_from_slice(&8u16.to_be_bytes());
        b.extend_from_slice(&3u16.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&0u32.to_be_bytes());
        let planes = [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]];
        if rle {
            b.extend_from_slice(&1u16.to_be_bytes());
            // Each row as a literal run of two: 3 bytes.
            for _ in 0..6 {
                b.extend_from_slice(&3u16.to_be_bytes());
            }
            for p in planes {
                for y in 0..2 {
                    b.extend_from_slice(&[1, p[y * 2], p[y * 2 + 1]]);
                }
            }
        } else {
            b.extend_from_slice(&0u16.to_be_bytes());
            for p in planes {
                b.extend_from_slice(&p);
            }
        }
        b
    }

    #[test]
    fn photoshop_files_read_their_merged_picture() {
        for rle in [false, true] {
            let img = psd(&tiny_psd(rle)).unwrap();
            assert_eq!(img.dimensions(), (2, 2));
            assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
            assert_eq!(img.get_pixel(1, 0).0, [0, 255, 0, 255]);
            assert_eq!(img.get_pixel(0, 1).0, [0, 0, 255, 255]);
            assert_eq!(img.get_pixel(1, 1).0, [255, 255, 255, 255]);
        }
        let mut bad = tiny_psd(true);
        bad.truncate(bad.len() - 4);
        let _ = psd(&bad);
        assert!(psd(b"8BPS").is_err());
    }

    #[test]
    fn pictures_shrink_to_the_budget_and_keep_alpha_only_when_it_matters() {
        let mut img = RgbaImage::from_pixel(64, 32, image::Rgba([200, 100, 50, 255]));
        let e = encode(&img, 16, true).unwrap();
        assert_eq!((e.width, e.height, e.png), (16, 8, false));
        img.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        let e = encode(&img, 128, true).unwrap();
        assert_eq!((e.width, e.height, e.png), (64, 32, true), "never larger, PNG for the alpha");
        assert!(!encode(&img, 128, false).unwrap().png, "an opaque material's picture stays JPEG");
        assert!(image::load_from_memory(&e.bytes).is_ok());
    }
}
