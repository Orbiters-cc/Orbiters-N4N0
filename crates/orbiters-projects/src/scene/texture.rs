//! Textures as a preview stores them: read from the project's source files (PNG, JPEG, TGA, BMP, TIFF, GIF and
//! Photoshop's PSD and PSB, whose merged picture Unity imports), made smaller to the preview's budget and written as
//! JPEG (opaque) or PNG (alpha that matters), as Orbiters Toolkit does from the GPU copy.
//!
//! Decoding is bounded: files over [`super::Limits::max_texture_bytes`] and pictures over 8192 pixels on a side are
//! left out (the material keeps its colour), so one odd 16k texture cannot hold a preview or fill the memory.

use anyhow::{bail, ensure, Context as _};
use orbiters_i18n::tr;
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
    Ok(reader.decode().with_context(|| tr!("a .{ext} the preview cannot read", ext = ext))?.to_rgba8())
}

/// A picture in linear light: lightmaps and skies, whose values run far above 1.
pub struct Linear {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
}

/// The largest HDR picture read, in memory: a 4096² lightmap takes 200 MB.
const MAX_LINEAR_ALLOC: u64 = 1 << 30;

/// Reads a picture as linear light: EXR and Radiance HDR as they are, the others from sRGB.
pub fn decode_linear(path: &Path, max_bytes: u64) -> anyhow::Result<Linear> {
    let size = std::fs::metadata(path)?.len();
    ensure!(size <= max_bytes, "{} MB", size >> 20);
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext != "exr" && ext != "hdr" {
        let img = decode(path, max_bytes)?;
        let (width, height) = img.dimensions();
        let pixels = img.pixels().map(|p| [0, 1, 2].map(|k| super::material::srgb_to_linear(p[k] as f32 / 255.0))).collect();
        return Ok(Linear { width, height, pixels });
    }
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_LINEAR_ALLOC);
    reader.limits(limits);
    let img = reader.decode().with_context(|| tr!("a .{ext} the preview cannot read", ext = ext))?.into_rgb32f();
    let (width, height) = img.dimensions();
    // No light of NaN or below zero (a bake's stray pixels).
    let pixels = img.pixels().map(|p| [0, 1, 2].map(|k| if p[k].is_finite() { p[k].max(0.0) } else { 0.0 })).collect();
    Ok(Linear { width, height, pixels })
}

impl Linear {
    /// At most `max_side` on a side, each pixel the average of those it covers. With `atlas` an atlas's empty
    /// (exactly black) pixels count for nothing and a pixel left empty takes its neighbours' light, so an island's edge
    /// does not darken as it shrinks (its padding was only a few pixels at the full size).
    pub fn shrink(&self, max_side: u32, atlas: bool) -> Linear {
        let (w, h) = (self.width.max(1), self.height.max(1));
        let scale = (max_side as f32 / w.max(h) as f32).min(1.0);
        let (nw, nh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
        let mut sum = vec![[0.0f32; 3]; (nw * nh) as usize];
        let mut count = vec![0u32; (nw * nh) as usize];
        for y in 0..h {
            let ty = (y as u64 * nh as u64 / h as u64) as usize;
            let row = &self.pixels[(y * w) as usize..((y + 1) * w) as usize];
            for (x, p) in row.iter().enumerate() {
                if atlas && p[0] == 0.0 && p[1] == 0.0 && p[2] == 0.0 {
                    continue;
                }
                let i = ty * nw as usize + (x as u64 * nw as u64 / w as u64) as usize;
                for k in 0..3 {
                    sum[i][k] += p[k];
                }
                count[i] += 1;
            }
        }
        let mut pixels: Vec<[f32; 3]> = sum.iter().zip(&count).map(|(s, &c)| if c > 0 { s.map(|v| v / c as f32) } else { [0.0; 3] }).collect();
        if atlas {
            // Twice: the empty pixels next to an island take the mean of their filled neighbours.
            for _ in 0..2 {
                let before = pixels.clone();
                let filled = |i: usize| before[i] != [0.0; 3];
                for y in 0..nh as usize {
                    for x in 0..nw as usize {
                        let i = y * nw as usize + x;
                        if filled(i) {
                            continue;
                        }
                        let (mut acc, mut n) = ([0.0f32; 3], 0.0f32);
                        for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                            let (ax, ay) = (x as i32 + dx, y as i32 + dy);
                            if ax < 0 || ay < 0 || ax >= nw as i32 || ay >= nh as i32 {
                                continue;
                            }
                            let j = ay as usize * nw as usize + ax as usize;
                            if filled(j) {
                                for k in 0..3 {
                                    acc[k] += before[j][k];
                                }
                                n += 1.0;
                            }
                        }
                        if n > 0.0 {
                            pixels[i] = acc.map(|v| v / n);
                        }
                    }
                }
            }
        }
        Linear { width: nw, height: nh, pixels }
    }

    /// A whole sphere's light by direction: facing up (the upper half, cosine-weighted), round the horizon, facing down.
    pub fn sphere_light(&self) -> [[f32; 3]; 3] {
        let (w, h) = (self.width as usize, self.height as usize);
        let (mut up, mut down, mut round) = ([0.0f32; 3], [0.0f32; 3], [0.0f32; 3]);
        let (mut up_w, mut down_w, mut round_w) = (0.0f32, 0.0f32, 0.0f32);
        for y in 0..h {
            let theta = (y as f32 + 0.5) / h as f32 * std::f32::consts::PI;
            let (sin, cos) = theta.sin_cos();
            let mut row = [0.0f32; 3];
            for p in &self.pixels[y * w..(y + 1) * w] {
                for k in 0..3 {
                    row[k] += p[k] / w as f32;
                }
            }
            let (uw, dw, rw) = ((cos.max(0.0)) * sin, (-cos).max(0.0) * sin, sin * sin * sin);
            for k in 0..3 {
                up[k] += row[k] * uw;
                down[k] += row[k] * dw;
                round[k] += row[k] * rw;
            }
            up_w += uw;
            down_w += dw;
            round_w += rw;
        }
        [up.map(|v| v / up_w.max(1e-6)), round.map(|v| v / round_w.max(1e-6)), down.map(|v| v / down_w.max(1e-6))]
    }
}

/// Six pictures of a skybox (front +Z, back -Z, left +X, right -X, up, down) as one picture of the whole sphere, rows
/// from straight up.
pub fn sphere_from_cube(faces: &[Linear]) -> Linear {
    let (width, height) = (2048u32, 1024u32);
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        let theta = (y as f32 + 0.5) / height as f32 * std::f32::consts::PI;
        for x in 0..width {
            // Unity's latitude-longitude mapping: u = 0.5 - atan2(z, x) / 2π.
            let phi = (0.5 - (x as f32 + 0.5) / width as f32) * std::f32::consts::TAU;
            let d = [theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()];
            pixels.push(cube_sample(faces, d));
        }
    }
    Linear { width, height, pixels }
}

/// A direction's colour on a skybox's six pictures.
fn cube_sample(faces: &[Linear], d: [f32; 3]) -> [f32; 3] {
    let a = d.map(f32::abs);
    // Which face, and where on it: (right, up on the picture), each -1 to 1.
    let (face, u, v) = if a[2] >= a[0] && a[2] >= a[1] {
        if d[2] > 0.0 {
            (0, d[0] / a[2], d[1] / a[2])
        } else {
            (1, -d[0] / a[2], d[1] / a[2])
        }
    } else if a[0] >= a[1] {
        if d[0] > 0.0 {
            (2, -d[2] / a[0], d[1] / a[0])
        } else {
            (3, d[2] / a[0], d[1] / a[0])
        }
    } else if d[1] > 0.0 {
        (4, d[0] / a[1], -d[2] / a[1])
    } else {
        (5, d[0] / a[1], d[2] / a[1])
    };
    let Some(f) = faces.get(face) else { return [0.0; 3] };
    let (fw, fh) = (f.width.max(1), f.height.max(1));
    let x = (((u + 1.0) * 0.5 * fw as f32) as u32).min(fw - 1);
    let y = (((1.0 - v) * 0.5 * fh as f32) as u32).min(fh - 1);
    f.pixels.get((y * fw + x) as usize).copied().unwrap_or([0.0; 3])
}

/// Light as a picture a viewer samples like any other: its colours, read as sRGB, are the light over the returned
/// scale. The scale leaves the brightest `1 - keep` of the pixels out (a sun's disc, a bake's hot pixel), which would
/// else crush the rest into a few shades.
pub fn encode_light(p: &Linear, keep: f32, quality: u8) -> anyhow::Result<(Encoded, f32)> {
    let mut peaks: Vec<f32> = p.pixels.iter().step_by((p.pixels.len() / 65536).max(1)).map(|c| c[0].max(c[1]).max(c[2])).collect();
    peaks.sort_by(f32::total_cmp);
    let at = ((peaks.len() as f32 * keep) as usize).min(peaks.len().saturating_sub(1));
    let scale = peaks.get(at).copied().unwrap_or(1.0).max(1e-3);
    let oetf = |c: f32| {
        let c = (c / scale).clamp(0.0, 1.0);
        let v = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        (v * 255.0 + 0.5) as u8
    };
    let rgb: Vec<u8> = p.pixels.iter().flat_map(|c| [oetf(c[0]), oetf(c[1]), oetf(c[2])]).collect();
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality).encode(&rgb, p.width, p.height, image::ExtendedColorType::Rgb8)?;
    Ok((Encoded { width: p.width, height: p.height, png: false, bytes, has_alpha: false }, scale))
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
    let be16 = |at: usize| -> anyhow::Result<u16> { Ok(u16::from_be_bytes(b.get(at..at + 2).with_context(|| tr("cut short"))?.try_into()?)) };
    let be32 = |at: usize| -> anyhow::Result<u32> { Ok(u32::from_be_bytes(b.get(at..at + 4).with_context(|| tr("cut short"))?.try_into()?)) };
    let be64 = |at: usize| -> anyhow::Result<u64> { Ok(u64::from_be_bytes(b.get(at..at + 8).with_context(|| tr("cut short"))?.try_into()?)) };
    ensure!(b.len() >= 26 && &b[..4] == b"8BPS", "{}", tr("not a Photoshop file"));
    let version = be16(4)?;
    ensure!(version == 1 || version == 2, "{}", tr!("Photoshop version {version}", version = version));
    let channels = be16(12)? as usize;
    let height = be32(14)?;
    let width = be32(18)?;
    let depth = be16(22)?;
    let mode = be16(24)?;
    ensure!(width > 0 && height > 0 && width <= MAX_SIDE && height <= MAX_SIDE, "{}", tr!("{width}×{height} pixels", width = width, height = height));
    ensure!(matches!(depth, 8 | 16 | 32), "{}", tr!("{depth} bits a channel", depth = depth));
    ensure!(matches!(mode, 1 | 3), "{}", tr!("colour mode {mode} (only grey and RGB are read)", mode = mode));
    let skip = |at: usize, n: u64| -> anyhow::Result<usize> { usize::try_from(n).ok().and_then(|n| at.checked_add(n)).with_context(|| tr("cut short")) };
    let mut at = 26usize;
    at = skip(at + 4, be32(at)? as u64)?; // colour mode data
    at = skip(at + 4, be32(at)? as u64)?; // image resources
    at = if version == 2 { skip(at + 8, be64(at)?)? } else { skip(at + 4, be32(at)? as u64)? }; // layers and masks
    let compression = be16(at)?;
    at += 2;
    let (w, h) = (width as usize, height as usize);
    let bytes_per = depth as usize / 8;
    let row = w * bytes_per;
    let wanted = if mode == 3 { channels.min(4) } else { channels.min(2) };
    ensure!(wanted >= 1, "{}", tr("no channels"));
    // What the planes take once decoded: within `image`'s own limit for the other formats, and never more than the
    // file could hold (PackBits makes at most 128 bytes of 2), so a header claiming 8192² at 32 bits in a 64 KB file
    // is not given a gigabyte on each of the decoding threads.
    let planes_bytes = row * h * wanted;
    ensure!(planes_bytes <= 512 << 20, "{}", tr!("{width}×{height} pixels at {depth} bits", width = width, height = height, depth = depth));
    ensure!(planes_bytes <= b.len().saturating_mul(64), "{}", tr("cut short"));
    let mut planes: Vec<Vec<u8>> = Vec::with_capacity(wanted);
    match compression {
        0 => {
            for c in 0..wanted {
                let start = at + c * row * h;
                planes.push(b.get(start..start + row * h).with_context(|| tr("cut short"))?.to_vec());
            }
        }
        1 => {
            let count_size = if version == 2 { 4 } else { 2 };
            let counts_at = at;
            let mut data = channels.checked_mul(h * count_size).and_then(|n| at.checked_add(n)).with_context(|| tr("cut short"))?;
            for c in 0..channels {
                let mut plane = Vec::with_capacity(row * h);
                for y in 0..h {
                    let k = counts_at + (c * h + y) * count_size;
                    let n = if version == 2 { be32(k)? as usize } else { be16(k)? as usize };
                    if c < wanted {
                        let src = b.get(data..data.checked_add(n).with_context(|| tr("cut short"))?).with_context(|| tr("cut short"))?;
                        packbits(src, row, &mut plane);
                    }
                    data = data.checked_add(n).with_context(|| tr("cut short"))?;
                }
                if c < wanted {
                    planes.push(plane);
                } else {
                    break;
                }
            }
        }
        other => bail!("{}", tr!("Photoshop compression {other} (save with Maximize Compatibility)", other = other)),
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
        // A header claiming 8192² at 32 bits in a file of a few bytes: refused before anything is made of it.
        let mut huge = tiny_psd(true);
        huge[14..18].copy_from_slice(&8192u32.to_be_bytes());
        huge[18..22].copy_from_slice(&8192u32.to_be_bytes());
        huge[22..24].copy_from_slice(&32u16.to_be_bytes());
        assert!(psd(&huge).is_err());
        // Section lengths that run past the end of memory (a PSB's 64-bit one): an error, not an overflow.
        let mut long = tiny_psd(false);
        long[4..6].copy_from_slice(&2u16.to_be_bytes());
        long[34..38].copy_from_slice(&u32::MAX.to_be_bytes());
        long.splice(34..34, [0xff; 4]);
        assert!(psd(&long).is_err());
    }

    #[test]
    fn damaged_photoshop_files_never_panic_or_hang() {
        let start = std::time::Instant::now();
        for rle in [false, true] {
            for b in crate::scene::damaged(&tiny_psd(rle), 3000) {
                let _ = psd(&b);
            }
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "{:?}", start.elapsed());
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

    /// A preview that cannot be drawn says why in the language now: these errors reach the "No preview" chip, and what reads
    /// them (the cache's record, the retry) must not depend on their words, so the language is read when the error happens.
    #[test]
    fn a_picture_that_cannot_be_read_says_so_in_the_language_now() {
        use orbiters_i18n::{in_language, Language};
        assert_eq!(psd(b"not a psd at all, not a psd at all").unwrap_err().to_string(), "not a Photoshop file");
        for (language, said) in [
            (Language::Ja, "Photoshopファイルではありません"),
            (Language::Fr, "pas un fichier Photoshop"),
            (Language::Es, "no es un archivo de Photoshop"),
            (Language::PtBr, "não é um arquivo do Photoshop"),
        ] {
            let error = in_language(language, || psd(b"not a psd at all, not a psd at all").unwrap_err().to_string());
            assert_eq!(error, said);
        }
    }
}
