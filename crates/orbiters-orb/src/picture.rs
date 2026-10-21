//! Small pictures for Orb's screen: a JPEG (a video's thumbnail) made grey, fitted to a few dozen pixels and dithered
//! to a few grey levels, packed a few bits a pixel, which is what the avatar's screen can be sent.

use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// A grey picture, 0 black to 255 white, rows top first.
#[derive(Clone, Debug, PartialEq)]
pub struct Grey {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// A colour picture, RGB, rows top first.
#[derive(Clone, Debug, PartialEq)]
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

fn decode(bytes: &[u8], colour: ColorSpace) -> anyhow::Result<(usize, usize, Vec<u8>)> {
    // Thumbnails are a few hundred pixels; anything far larger is not one.
    let options = DecoderOptions::default().jpeg_set_out_colorspace(colour).set_max_width(4096).set_max_height(4096);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let pixels = decoder.decode().map_err(|e| anyhow::anyhow!("not a JPEG picture: {e:?}"))?;
    let (width, height) = decoder.dimensions().ok_or_else(|| anyhow::anyhow!("a JPEG without a size"))?;
    anyhow::ensure!(width > 0 && height > 0 && pixels.len() >= width * height * colour.num_components(), "a JPEG without pixels");
    Ok((width, height, pixels))
}

pub fn grey_jpeg(bytes: &[u8]) -> anyhow::Result<Grey> {
    let (width, height, pixels) = decode(bytes, ColorSpace::Luma)?;
    Ok(Grey { width, height, pixels })
}

pub fn rgb_jpeg(bytes: &[u8]) -> anyhow::Result<Rgb> {
    let (width, height, pixels) = decode(bytes, ColorSpace::RGB)?;
    Ok(Rgb { width, height, pixels })
}

/// The picture shrunk to `width` × `height` (0 to 1 per pixel), each pixel the mean of the source pixels it covers;
/// a different shape is cropped at its middle, as a cover.
pub fn fit(source: &Grey, width: usize, height: usize) -> Vec<f32> {
    let mut out = vec![0f32; width * height];
    if width == 0 || height == 0 || source.width == 0 || source.height == 0 {
        return out;
    }
    // The part of the source with the target's shape.
    let (sw, sh) = (source.width as f64, source.height as f64);
    let target = width as f64 / height as f64;
    let (cw, ch) = if sw / sh > target { (sh * target, sh) } else { (sw, sw / target) };
    let (x0, y0) = ((sw - cw) / 2.0, (sh - ch) / 2.0);
    let (step_x, step_y) = (cw / width as f64, ch / height as f64);
    for ty in 0..height {
        let (top, bottom) = (y0 + ty as f64 * step_y, y0 + (ty + 1) as f64 * step_y);
        for tx in 0..width {
            let (left, right) = (x0 + tx as f64 * step_x, x0 + (tx + 1) as f64 * step_x);
            let (mut sum, mut area) = (0f64, 0f64);
            let mut y = top.floor() as usize;
            while (y as f64) < bottom && y < source.height {
                let cover_y = (bottom.min(y as f64 + 1.0) - top.max(y as f64)).max(0.0);
                let mut x = left.floor() as usize;
                while (x as f64) < right && x < source.width {
                    let cover = (right.min(x as f64 + 1.0) - left.max(x as f64)).max(0.0) * cover_y;
                    sum += source.pixels[y * source.width + x] as f64 * cover;
                    area += cover;
                    x += 1;
                }
                y += 1;
            }
            out[ty * width + tx] = if area > 0.0 { (sum / area / 255.0) as f32 } else { 0.0 };
        }
    }
    out
}

/// Each value (0 to 1) as one of `levels` grey levels (0 to `levels` − 1), the error spread to the neighbours
/// (Floyd–Steinberg, rows walked back and forth so it leaves no diagonal streaks). The values are taken as they look:
/// a level's share of white is its brightness.
pub fn dither(values: &[f32], width: usize, height: usize, levels: u8) -> Vec<u8> {
    let top = levels.max(2) as f32 - 1.0;
    let mut error = values.iter().map(|v| v.clamp(0.0, 1.0) * top).collect::<Vec<f32>>();
    let mut out = vec![0u8; values.len()];
    for y in 0..height {
        let forward = y % 2 == 0;
        for i in 0..width {
            let x = if forward { i } else { width - 1 - i };
            let at = y * width + x;
            let value = error[at];
            let level = value.round().clamp(0.0, top);
            out[at] = level as u8;
            let rest = value - level;
            let mut spread = |dx: isize, dy: usize, share: f32| {
                let nx = x as isize + if forward { dx } else { -dx };
                let ny = y + dy;
                if nx >= 0 && (nx as usize) < width && ny < height {
                    error[ny * width + nx as usize] += rest * share;
                }
            };
            spread(1, 0, 7.0 / 16.0);
            spread(-1, 1, 3.0 / 16.0);
            spread(0, 1, 5.0 / 16.0);
            spread(1, 1, 1.0 / 16.0);
        }
    }
    out
}

/// `values` of `bits` bits each (1, 2, 4 or 8), packed into bytes, the first value in the highest bits.
pub fn pack(values: &[u8], bits: u8) -> Vec<u8> {
    assert!(matches!(bits, 1 | 2 | 4 | 8), "pixels are packed 1, 2, 4 or 8 bits each");
    let per = 8 / bits as usize;
    let mask = ((1u16 << bits) - 1) as u8;
    values.chunks(per).map(|chunk| chunk.iter().enumerate().fold(0u8, |byte, (i, v)| byte | ((v & mask) << (8 - bits as usize * (i + 1))))).collect()
}

/// The inverse of [`pack`] for `count` values.
pub fn unpack(bytes: &[u8], bits: u8, count: usize) -> Vec<u8> {
    let per = 8 / bits as usize;
    let mask = ((1u16 << bits) - 1) as u8;
    (0..count).map(|i| bytes.get(i / per).map_or(0, |b| (b >> (8 - bits as usize * (i % per + 1))) & mask)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_by_area_and_crops_to_shape() {
        // 4 × 2, left half black, right half white: 2 × 1 keeps the halves, 1 × 1 is their mean.
        let source = Grey { width: 4, height: 2, pixels: vec![0, 0, 255, 255, 0, 0, 255, 255] };
        assert_eq!(fit(&source, 2, 1), vec![0.0, 1.0]);
        let square = fit(&source, 1, 1)[0];
        assert!((square - 0.5).abs() < 1e-6, "the middle 2 × 2 is half white: {square}");
        // 3 × 1 into 2 × 1: cropped to its middle 2 × 1 (x 0.5 to 2.5), whose halves share the white pixel.
        let row = Grey { width: 3, height: 1, pixels: vec![0, 255, 0] };
        assert_eq!(fit(&row, 2, 1), vec![0.5, 0.5]);
        let wide = Grey { width: 3, height: 1, pixels: vec![255, 0, 0] };
        let three = fit(&wide, 3, 1);
        assert_eq!(three, vec![1.0, 0.0, 0.0], "the same shape is kept pixel for pixel");
    }

    #[test]
    fn dithers_to_the_right_share_of_white() {
        let (w, h) = (32, 18);
        for grey in [0.0f32, 0.25, 0.5, 0.8, 1.0] {
            let out = dither(&vec![grey; w * h], w, h, 2);
            let white = out.iter().filter(|&&v| v == 1).count() as f32 / (w * h) as f32;
            assert!((white - grey).abs() < 0.03, "grey {grey}: {white} white");
        }
        let four = dither(&vec![0.5; w * h], w, h, 4);
        let mean = four.iter().map(|&v| v as f32 / 3.0).sum::<f32>() / (w * h) as f32;
        assert!((mean - 0.5).abs() < 0.02 && four.iter().all(|&v| v <= 3));
    }

    #[test]
    fn packs_highest_bits_first() {
        assert_eq!(pack(&[1, 0, 1, 1, 0, 0, 0, 1, 1], 1), vec![0b1011_0001, 0b1000_0000]);
        assert_eq!(pack(&[3, 0, 2, 1], 2), vec![0b11_00_10_01]);
        let values = (0..37).map(|i| (i * 7 % 4) as u8).collect::<Vec<_>>();
        assert_eq!(unpack(&pack(&values, 2), 2, values.len()), values);
    }

    #[test]
    fn decodes_a_jpeg() {
        assert!(grey_jpeg(b"not a picture").is_err());
        // 32 × 18, black on the left to white on the right (OpenCV, quality 95).
        let jpeg = include_bytes!("../tests/fixtures/ramp-32x18.jpg");
        let grey = grey_jpeg(jpeg).unwrap();
        assert_eq!((grey.width, grey.height), (32, 18));
        assert!(grey.pixels[9 * 32] < 20 && grey.pixels[9 * 32 + 31] > 235, "{} {}", grey.pixels[9 * 32], grey.pixels[9 * 32 + 31]);
        let rgb = rgb_jpeg(jpeg).unwrap();
        assert_eq!(rgb.pixels.len(), 32 * 18 * 3);
        let small = fit(&grey, 4, 2);
        assert!(small[0] < small[1] && small[1] < small[2] && small[2] < small[3], "{small:?}");
    }
}
