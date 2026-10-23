//! Orb's menu in its retro look (contracts `Docs/orb.md`, Retro look): a green phosphor terminal. The page as drawn is
//! turned into one hue by its light, every third row is darker (scanlines) and the corners fade. The avatar's screen
//! does the same in its shader, so both read alike; text arriving a block at a time then reads as typed out.

/// The phosphor's three steps: the card, the labels, the brightest marks (sRGB).
const DARK: [f32; 3] = [6.0, 16.0, 9.0];
const MID: [f32; 3] = [40.0, 230.0, 110.0];
const HOT: [f32; 3] = [200.0, 255.0, 215.0];

/// One pixel of the page (sRGB, not premultiplied) at (x, y) of a `width` × `height` page, in the retro look.
pub fn pixel(rgb: [u8; 3], x: u32, y: u32, width: u32, height: u32) -> [u8; 3] {
    let light = (0.2126 * rgb[0] as f32 + 0.7152 * rgb[1] as f32 + 0.0722 * rgb[2] as f32) / 255.0;
    let t = ((light - 0.05) / 0.85).clamp(0.0, 1.0).powf(0.85);
    let (from, to, k) = if t < 0.75 { (DARK, MID, t / 0.75) } else { (MID, HOT, (t - 0.75) / 0.25) };
    let scanline = if y % 3 == 2 { 0.72 } else { 1.0 };
    let dx = (x as f32 + 0.5) / width as f32 * 2.0 - 1.0;
    let dy = (y as f32 + 0.5) / height as f32 * 2.0 - 1.0;
    let vignette = 1.0 - 0.22 * (0.6 * dx * dx + dy * dy);
    std::array::from_fn(|i| ((from[i] + (to[i] - from[i]) * k) * scanline * vignette).round().clamp(0.0, 255.0) as u8)
}

/// Writes the `rect` (x0, y0, x1, y1) of `rgba` (premultiplied, top row first, `width` × `height`) into the same place
/// of `out` in the retro look, premultiplied again with the same alpha.
pub fn apply(rgba: &[u8], out: &mut [u8], width: u32, height: u32, rect: [u32; 4]) {
    let [x0, y0, x1, y1] = rect;
    for y in y0..y1.min(height) {
        for x in x0..x1.min(width) {
            let i = ((y * width + x) * 4) as usize;
            let a = rgba[i + 3];
            if a == 0 {
                out[i..i + 4].copy_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let straight = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            let c = pixel([straight(rgba[i]), straight(rgba[i + 1]), straight(rgba[i + 2])], x, y, width, height);
            let pre = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
            out[i..i + 4].copy_from_slice(&[pre(c[0]), pre(c[1]), pre(c[2]), a]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_becomes_phosphor() {
        // The centre of a 1024 × 576 page, off a scanline: black is the dark step, white the hot one.
        assert_eq!(pixel([0, 0, 0], 512, 288, 1024, 576), [6, 16, 9]);
        assert_eq!(pixel([255, 255, 255], 512, 288, 1024, 576), [200, 255, 215]);
        // A label is near the hot step, secondary text between the card and the labels.
        let label = pixel([236, 237, 238], 512, 288, 1024, 576);
        assert!(label[1] >= 250, "{label:?}");
        let note = pixel([139, 139, 148], 512, 288, 1024, 576);
        assert!(note[1] > 150 && note[1] < 230, "{note:?}");
        // Every third row is darker, the corners too.
        assert!(pixel([128, 128, 128], 512, 290, 1024, 576)[1] < pixel([128, 128, 128], 512, 288, 1024, 576)[1]);
        assert!(pixel([128, 128, 128], 0, 0, 1024, 576)[1] < pixel([128, 128, 128], 512, 288, 1024, 576)[1]);
    }

    #[test]
    fn keeps_alpha_and_the_rest_of_the_picture() {
        let (w, h) = (4u32, 3u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        // A half-covered white pixel at (1, 1): premultiplied 128.
        let i = ((w + 1) * 4) as usize;
        rgba[i..i + 4].copy_from_slice(&[128, 128, 128, 128]);
        let mut out = vec![9u8; rgba.len()];
        apply(&rgba, &mut out, w, h, [1, 1, 2, 2]);
        assert_eq!(out[i + 3], 128);
        assert!(out[i + 1] > out[i] && out[i + 1] <= 128, "premultiplied green: {:?}", &out[i..i + 4]);
        assert!(out[..i].iter().all(|&b| b == 9), "outside the rect: untouched");
    }
}
