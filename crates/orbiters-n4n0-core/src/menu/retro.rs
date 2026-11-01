//! N4N0's menu in its retro look (contracts `Docs/n4n0.md`, Retro look): pictures as green dots. The overlay draws a
//! thumbnail this way from its pixels, the avatar's retro screen from the thumbnail's bytes (the light the codec carries
//! at each dot's centre), with the same steps and dither, so both read alike.

/// The dots across and down a picture.
pub const DOTS_X: usize = 96;
pub const DOTS_Y: usize = 54;
/// The four steps a dot takes (ground, dim, line, hot), as levels of its light.
pub const STEPS: usize = 4;
/// Light below `LOW` is the ground, above `LOW + SPAN` the hottest step: a fixed stretch, the same on both sides.
pub const LOW: f32 = 0.08;
pub const SPAN: f32 = 0.84;
/// The ordered dither's threshold map (4 × 4 Bayer), of 16 levels.
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// The light (JPEG's Y of the sRGB values, 0 to 1) of each dot of a `width` × `height` picture read through `pixel`,
/// averaged over the pixels the dot covers, row by row from the top.
pub fn lights(width: usize, height: usize, pixel: impl Fn(usize, usize) -> [u8; 3]) -> Vec<f32> {
    let mut out = Vec::with_capacity(DOTS_X * DOTS_Y);
    for dy in 0..DOTS_Y {
        let (y0, y1) = (dy * height / DOTS_Y, ((dy + 1) * height / DOTS_Y).max(dy * height / DOTS_Y + 1).min(height));
        for dx in 0..DOTS_X {
            let (x0, x1) = (dx * width / DOTS_X, ((dx + 1) * width / DOTS_X).max(dx * width / DOTS_X + 1).min(width));
            let mut sum = 0.0;
            for y in y0..y1 {
                for x in x0..x1 {
                    let [r, g, b] = pixel(x, y);
                    sum += 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
                }
            }
            out.push(sum / ((y1 - y0) * (x1 - x0)).max(1) as f32 / 255.0);
        }
    }
    out
}

/// The step (0 to 3) of a dot at (`x`, `y`) with `light`: stretched, then dithered between the two nearest steps.
pub fn step(light: f32, x: usize, y: usize) -> usize {
    let level = ((light - LOW) / SPAN).clamp(0.0, 1.0) * (STEPS - 1) as f32;
    let threshold = (BAYER[y % 4][x % 4] as f32 + 0.5) / 16.0;
    ((level + threshold).floor() as usize).min(STEPS - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_follow_the_light_and_dither_between() {
        assert_eq!(step(0.0, 0, 0), 0);
        assert_eq!(step(1.0, 3, 3), 3);
        // Halfway between two steps: half the dots of a 4 × 4 block take the upper one.
        let mid = LOW + SPAN * (0.5 / 3.0);
        let upper = (0..4).flat_map(|y| (0..4).map(move |x| (x, y))).filter(|&(x, y)| step(mid, x, y) == 1).count();
        assert_eq!(upper, 8);
    }

    #[test]
    fn lights_average_what_each_dot_covers() {
        // A picture twice the dots' size, white on its left half.
        let (w, h) = (DOTS_X * 2, DOTS_Y * 2);
        let light = lights(w, h, |x, _| if x < w / 2 { [255; 3] } else { [0; 3] });
        assert_eq!(light.len(), DOTS_X * DOTS_Y);
        assert!((light[0] - 1.0).abs() < 1e-5 && light[DOTS_X - 1].abs() < 1e-5);
    }
}
