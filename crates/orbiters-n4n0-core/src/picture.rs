//! Small pictures for N4N0's screen: a JPEG (a video's thumbnail) or a PNG (music art) decoded and fitted to a few dozen
//! pixels, which the screen's thumbnail codec (contracts `n4n0::thumbnail`) then encodes.

use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

/// A colour picture, RGB, rows top first.
#[derive(Clone, Debug, PartialEq)]
pub struct Rgb {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

fn decode(bytes: &[u8]) -> anyhow::Result<(usize, usize, Vec<u8>)> {
    // Thumbnails are a few hundred pixels; anything far larger is not one.
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB).set_max_width(4096).set_max_height(4096);
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let pixels = decoder.decode().map_err(|e| anyhow::anyhow!("not a JPEG picture: {e:?}"))?;
    let (width, height) = decoder.dimensions().ok_or_else(|| anyhow::anyhow!("a JPEG without a size"))?;
    anyhow::ensure!(width > 0 && height > 0 && pixels.len() >= width * height * 3, "a JPEG without pixels");
    Ok((width, height, pixels))
}

pub fn rgb_jpeg(bytes: &[u8]) -> anyhow::Result<Rgb> {
    let (width, height, pixels) = decode(bytes)?;
    Ok(Rgb { width, height, pixels })
}

/// A PNG's colours (transparent parts over black), 8 bits a channel.
pub fn rgb_png(bytes: &[u8]) -> anyhow::Result<Rgb> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| anyhow::anyhow!("not a PNG picture: {e}"))?;
    let (width, height) = (reader.info().width as usize, reader.info().height as usize);
    anyhow::ensure!(width > 0 && height > 0 && width <= 4096 && height <= 4096, "a PNG of {width} × {height}");
    let mut buffer = vec![0u8; reader.output_buffer_size().ok_or_else(|| anyhow::anyhow!("a PNG too large"))?];
    let frame = reader.next_frame(&mut buffer).map_err(|e| anyhow::anyhow!("a broken PNG: {e}"))?;
    let channels = match frame.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => anyhow::bail!("a PNG left indexed"),
    };
    let mut pixels = Vec::with_capacity(width * height * 3);
    for row in buffer.chunks(frame.line_size).take(height) {
        for px in row.chunks(channels).take(width) {
            let (rgb, alpha) = match channels {
                1 => ([px[0]; 3], 255),
                2 => ([px[0]; 3], px[1]),
                3 => ([px[0], px[1], px[2]], 255),
                _ => ([px[0], px[1], px[2]], px[3]),
            };
            pixels.extend(rgb.map(|c| (c as u16 * alpha as u16 / 255) as u8));
        }
    }
    Ok(Rgb { width, height, pixels })
}

/// A JPEG or a PNG, by what its bytes start with.
pub fn rgb_any(bytes: &[u8]) -> anyhow::Result<Rgb> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        rgb_png(bytes)
    } else {
        rgb_jpeg(bytes)
    }
}

/// The picture whole in a `width` × `height` one (shrunk to fit by area means, never enlarged past it), centred on
/// `background`: music art, square, on a 16:9 thumbnail.
pub fn letterbox(source: &Rgb, width: usize, height: usize, background: [u8; 3]) -> Rgb {
    let mut pixels: Vec<u8> = std::iter::repeat_n(background, width * height).flatten().collect();
    if source.width == 0 || source.height == 0 || width == 0 || height == 0 {
        return Rgb { width, height, pixels };
    }
    let scale = (width as f64 / source.width as f64).min(height as f64 / source.height as f64);
    let (w, h) = (((source.width as f64 * scale).round() as usize).clamp(1, width), ((source.height as f64 * scale).round() as usize).clamp(1, height));
    let inner = fit(source, w, h);
    let (x0, y0) = ((width - w) / 2, (height - h) / 2);
    for y in 0..h {
        for x in 0..w {
            let at = ((y0 + y) * width + x0 + x) * 3;
            let c = inner[y * w + x];
            pixels[at..at + 3].copy_from_slice(&c.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8));
        }
    }
    Rgb { width, height, pixels }
}

/// The picture shrunk to `width` × `height` (sRGB values 0 to 1, rows top first), each pixel the mean of the source
/// pixels it covers; a different shape is cropped at its middle, as a cover.
pub fn fit(source: &Rgb, width: usize, height: usize) -> Vec<[f32; 3]> {
    let mut out = vec![[0f32; 3]; width * height];
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
            let (mut sum, mut area) = ([0f64; 3], 0f64);
            let mut y = top.floor() as usize;
            while (y as f64) < bottom && y < source.height {
                let cover_y = (bottom.min(y as f64 + 1.0) - top.max(y as f64)).max(0.0);
                let mut x = left.floor() as usize;
                while (x as f64) < right && x < source.width {
                    let cover = (right.min(x as f64 + 1.0) - left.max(x as f64)).max(0.0) * cover_y;
                    let at = (y * source.width + x) * 3;
                    for (s, v) in sum.iter_mut().zip(&source.pixels[at..at + 3]) {
                        *s += *v as f64 * cover;
                    }
                    area += cover;
                    x += 1;
                }
                y += 1;
            }
            if area > 0.0 {
                out[ty * width + tx] = sum.map(|v| (v / area / 255.0) as f32);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(width: usize, height: usize, grey: &[u8]) -> Rgb {
        Rgb { width, height, pixels: grey.iter().flat_map(|&g| [g, g / 2, 255 - g]).collect() }
    }

    #[test]
    fn fits_by_area_and_crops_to_shape() {
        // 4 × 2, left half black, right half white: 2 × 1 keeps the halves, 1 × 1 is their mean.
        let source = rgb(4, 2, &[0, 0, 255, 255, 0, 0, 255, 255]);
        let two = fit(&source, 2, 1);
        assert_eq!((two[0][0], two[1][0]), (0.0, 1.0));
        assert_eq!((two[0][2], two[1][2]), (1.0, 0.0), "each channel on its own");
        let square = fit(&source, 1, 1)[0][0];
        assert!((square - 0.5).abs() < 1e-6, "the middle 2 × 2 is half white: {square}");
        // 3 × 1 into 2 × 1: cropped to its middle 2 × 1 (x 0.5 to 2.5), whose halves share the white pixel.
        let row = rgb(3, 1, &[0, 255, 0]);
        assert_eq!(fit(&row, 2, 1).iter().map(|p| p[0]).collect::<Vec<_>>(), vec![0.5, 0.5]);
        let wide = rgb(3, 1, &[255, 0, 0]);
        assert_eq!(fit(&wide, 3, 1).iter().map(|p| p[0]).collect::<Vec<_>>(), vec![1.0, 0.0, 0.0], "the same shape is kept pixel for pixel");
    }

    #[test]
    fn letterboxes_a_square_into_a_wide_picture() {
        let white = rgb(2, 2, &[255, 255, 255, 255]);
        let wide = letterbox(&white, 16, 9, [10, 20, 30]);
        assert_eq!((wide.width, wide.height), (16, 9));
        let at = |x: usize, y: usize| &wide.pixels[(y * 16 + x) * 3..(y * 16 + x) * 3 + 3];
        assert_eq!(at(0, 4), &[10, 20, 30], "the sides are the background");
        assert_eq!(at(8, 4), &[255, 127, 0], "the middle is the picture, whole");
        assert_eq!(at(8, 0), &[255, 127, 0], "it reaches the top and the bottom");
    }

    #[test]
    fn decodes_a_png() {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[255, 0, 0, 255, 0, 255, 0, 0]).unwrap();
        }
        let rgb = rgb_any(&bytes).unwrap();
        assert_eq!((rgb.width, rgb.height), (2, 1));
        assert_eq!(rgb.pixels, vec![255, 0, 0, 0, 0, 0], "a transparent pixel is black");
        assert!(rgb_any(b"not a picture").is_err());
    }

    #[test]
    fn decodes_a_jpeg() {
        assert!(rgb_jpeg(b"not a picture").is_err());
        // 32 × 18, black on the left to white on the right (OpenCV, quality 95).
        let jpeg = include_bytes!("../tests/fixtures/ramp-32x18.jpg");
        let rgb = rgb_jpeg(jpeg).unwrap();
        assert_eq!((rgb.width, rgb.height, rgb.pixels.len()), (32, 18, 32 * 18 * 3));
        assert!(rgb.pixels[9 * 32 * 3] < 20 && rgb.pixels[(9 * 32 + 31) * 3] > 235);
        let small = fit(&rgb, 4, 2);
        assert!(small[0][0] < small[1][0] && small[1][0] < small[2][0] && small[2][0] < small[3][0], "{small:?}");
    }
}
