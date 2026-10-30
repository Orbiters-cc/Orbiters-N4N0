//! Tones and colours, pixel by pixel: exposure in linear light, then the tone curve of [`Light`] on the encoded
//! values (black point, brightness, contrast, shadows, highlights), white balance, saturation and hue, and curves.

use super::{decode, encode, lerp, luma, Color, Curves, Image, Light};
use rayon::prelude::*;

/// Entries of a tone table over 0 to 1.
const LUT: usize = 1024;

/// A function from 0 to 1 sampled into a table, read between its entries.
#[derive(Clone, Debug)]
pub struct Lut(Vec<f32>);

impl Lut {
    pub fn new(f: impl Fn(f32) -> f32) -> Self {
        Self((0..=LUT).map(|i| f(i as f32 / LUT as f32)).collect())
    }

    pub fn at(&self, x: f32) -> f32 {
        let t = x.clamp(0.0, 1.0) * LUT as f32;
        let i = (t as usize).min(LUT - 1);
        lerp(self.0[i], self.0[i + 1], t - i as f32)
    }
}

/// Lifts (`s` > 0) or deepens (`s` < 0) the dark end of the tones, most around a third, never folding them over.
fn shadow_shift(x: f32, s: f32) -> f32 {
    let k = if s > 0.0 { 1.6 } else { 0.9 };
    x + s * k * x * (1.0 - x) * (1.0 - x)
}

/// [`Light`]'s tones (not its exposure), for an encoded value.
pub fn light_curve(l: &Light, x: f32) -> f32 {
    let b = l.black_point.clamp(-0.25, 0.5);
    let mut y = ((x - b) / (1.0 - b)).clamp(0.0, 1.0);
    // Brightness: a gamma on the middle tones.
    y = y.powf(2f32.powf(-l.brightness.clamp(-1.0, 1.0)));
    let c = l.contrast.clamp(-1.0, 1.0);
    y = if c >= 0.0 { lerp(y, y * y * (3.0 - 2.0 * y), c) } else { lerp(y, 0.5 + (y - 0.5) * 0.5, -c) };
    y = shadow_shift(y, l.shadows.clamp(-1.0, 1.0));
    // Highlights: the same shape mirrored at the light end.
    y = 1.0 - shadow_shift(1.0 - y, -l.highlights.clamp(-1.0, 1.0));
    y.clamp(0.0, 1.0)
}

/// Each pixel's straight colour through `f` (alpha kept, the result premultiplied again).
pub fn per_pixel(img: &mut Image, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
    img.data.par_iter_mut().for_each(|p| {
        let a = p[3];
        if a <= 0.0 {
            return;
        }
        let c = f([p[0] / a, p[1] / a, p[2] / a]);
        *p = [c[0] * a, c[1] * a, c[2] * a, a];
    });
}

pub fn light(img: &mut Image, l: &Light) {
    let gain = 2f32.powf(l.exposure.clamp(-4.0, 4.0));
    let lut = Lut::new(|x| light_curve(l, x));
    per_pixel(img, |c| c.map(|v| decode(lut.at(encode(v * gain)))));
}

/// White balance gains in linear light: warmer is redder and less blue, a magenta tint less green.
pub fn balance(temperature: f32, tint: f32) -> [f32; 3] {
    let t = temperature.clamp(-1.0, 1.0) * 0.3;
    let m = tint.clamp(-1.0, 1.0) * 0.25;
    [1.0 + t, 1.0 - m, 1.0 - t]
}

/// The CSS `hue-rotate` matrix (about the grey axis, luma kept).
pub fn hue_matrix(degrees: f32) -> [[f32; 3]; 3] {
    let (s, c) = degrees.to_radians().sin_cos();
    [
        [0.213 + c * 0.787 - s * 0.213, 0.715 - c * 0.715 - s * 0.715, 0.072 - c * 0.072 + s * 0.928],
        [0.213 - c * 0.213 + s * 0.143, 0.715 + c * 0.285 + s * 0.140, 0.072 - c * 0.072 - s * 0.283],
        [0.213 - c * 0.213 - s * 0.787, 0.715 - c * 0.715 + s * 0.715, 0.072 + c * 0.928 + s * 0.072],
    ]
}

/// [`Color`] on one straight linear colour.
pub fn color_of(c: &Color, rgb: [f32; 3]) -> [f32; 3] {
    let g = balance(c.temperature, c.tint);
    let mut e = [encode(rgb[0] * g[0]), encode(rgb[1] * g[1]), encode(rgb[2] * g[2])];
    if c.hue != 0.0 {
        let m = hue_matrix(c.hue);
        e = [0, 1, 2].map(|i| (m[i][0] * e[0] + m[i][1] * e[1] + m[i][2] * e[2]).clamp(0.0, 1.0));
    }
    let y = luma(e);
    // How colourful it is already: vibrance spares what is.
    let chroma = e.iter().fold(0f32, |m, v| m.max(*v)) - e.iter().fold(1f32, |m, v| m.min(*v));
    let amount = 1.0 + c.saturation.clamp(-1.0, 1.0) + c.vibrance.clamp(-1.0, 1.0) * (1.0 - chroma).powi(2);
    e.map(|v| decode((y + (v - y) * amount.max(0.0)).clamp(0.0, 1.0)))
}

pub fn color(img: &mut Image, c: &Color) {
    per_pixel(img, |rgb| color_of(c, rgb));
}

/// The curve through `points` (sorted by input, from 0 to 1): monotone cubic between them (Fritsch–Carlson), so it
/// never overshoots, flat beyond the first and last.
pub fn curve(points: &[[f32; 2]], x: f32) -> f32 {
    let mut p: Vec<[f32; 2]> = points.iter().copied().filter(|p| p[0].is_finite() && p[1].is_finite()).collect();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]));
    p.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-6);
    match p.len() {
        0 => return x,
        1 => return p[0][1].clamp(0.0, 1.0),
        _ => {}
    }
    if x <= p[0][0] {
        return p[0][1].clamp(0.0, 1.0);
    }
    if x >= p[p.len() - 1][0] {
        return p[p.len() - 1][1].clamp(0.0, 1.0);
    }
    let n = p.len();
    let d: Vec<f32> = (0..n - 1).map(|i| (p[i + 1][1] - p[i][1]) / (p[i + 1][0] - p[i][0])).collect();
    let mut m: Vec<f32> = (0..n)
        .map(|i| match i {
            0 => d[0],
            i if i == n - 1 => d[n - 2],
            i if d[i - 1] * d[i] <= 0.0 => 0.0,
            i => (d[i - 1] + d[i]) * 0.5,
        })
        .collect();
    for i in 0..n - 1 {
        if d[i] == 0.0 {
            m[i] = 0.0;
            m[i + 1] = 0.0;
            continue;
        }
        let (a, b) = (m[i] / d[i], m[i + 1] / d[i]);
        let s = a * a + b * b;
        if s > 9.0 {
            let t = 3.0 / s.sqrt();
            m[i] = t * a * d[i];
            m[i + 1] = t * b * d[i];
        }
    }
    let i = p.windows(2).position(|w| x >= w[0][0] && x <= w[1][0]).unwrap_or(n - 2);
    let h = p[i + 1][0] - p[i][0];
    let t = (x - p[i][0]) / h;
    let (t2, t3) = (t * t, t * t * t);
    let y = (2.0 * t3 - 3.0 * t2 + 1.0) * p[i][1] + (t3 - 2.0 * t2 + t) * h * m[i] + (-2.0 * t3 + 3.0 * t2) * p[i + 1][1] + (t3 - t2) * h * m[i + 1];
    y.clamp(0.0, 1.0)
}

/// The channel's curve then the one for all channels, as tables.
pub fn curve_luts(c: &Curves) -> [Lut; 3] {
    let all = Lut::new(|x| curve(&c.rgb, x));
    [&c.red, &c.green, &c.blue].map(|points| {
        let own = Lut::new(|x| curve(points, x));
        Lut::new(|x| all.at(own.at(x)))
    })
}

pub fn curves(img: &mut Image, c: &Curves) {
    let luts = curve_luts(c);
    per_pixel(img, |rgb| [0, 1, 2].map(|i| decode(luts[i].at(encode(rgb[i])))));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo::{to_linear, to_srgb};

    fn grey(v: f32) -> Image {
        Image::filled(2, 2, [to_linear(v), to_linear(v), to_linear(v), 1.0])
    }

    fn shown(img: &Image) -> f32 {
        to_srgb(img.data[0][0])
    }

    #[test]
    fn exposure_is_stops_of_linear_light() {
        let mut img = Image::filled(1, 1, [0.1, 0.2, 0.05, 1.0]);
        light(&mut img, &Light { exposure: 1.0, ..Default::default() });
        let p = img.data[0];
        for (got, want) in p.iter().zip([0.2, 0.4, 0.1]) {
            assert!((got - want).abs() < 0.004, "{p:?}");
        }
        let mut img = Image::filled(1, 1, [0.4, 0.4, 0.4, 1.0]);
        light(&mut img, &Light { exposure: -2.0, ..Default::default() });
        assert!((img.data[0][0] - 0.1).abs() < 0.002, "{:?}", img.data[0]);
    }

    #[test]
    fn black_point_becomes_black_and_white_stays_white() {
        let l = Light { black_point: 0.2, ..Default::default() };
        assert_eq!(light_curve(&l, 0.2), 0.0);
        assert_eq!(light_curve(&l, 0.1), 0.0);
        assert!((light_curve(&l, 1.0) - 1.0).abs() < 1e-6);
        assert!((light_curve(&l, 0.6) - 0.5).abs() < 1e-6);
        // Below zero, black lifts to a faded grey.
        let faded = Light { black_point: -0.2, ..Default::default() };
        assert!((light_curve(&faded, 0.0) - 0.2 / 1.2).abs() < 1e-6);
        let mut img = grey(0.2);
        light(&mut img, &l);
        assert!(shown(&img) < 0.002);
    }

    #[test]
    fn tone_curves_stay_in_order_at_every_extreme() {
        for &(b, c, s, h, k) in &[(1.0, 1.0, 1.0, 1.0, 0.5), (-1.0, -1.0, -1.0, -1.0, -0.25), (0.0, 0.0, 1.0, -1.0, 0.0), (0.0, 0.0, -1.0, 1.0, 0.3)] {
            let l = Light { exposure: 0.0, brightness: b, contrast: c, shadows: s, highlights: h, black_point: k };
            let mut last = -1.0;
            for i in 0..=200 {
                let y = light_curve(&l, i as f32 / 200.0);
                assert!(y >= last - 1e-6, "{l:?} at {i}: {y} < {last}");
                last = y;
            }
        }
        // Shadows lift the dark tones more than the light ones; highlights the reverse.
        let up = Light { shadows: 1.0, ..Default::default() };
        assert!(light_curve(&up, 0.2) - 0.2 > light_curve(&up, 0.8) - 0.8);
        let up = Light { highlights: 1.0, ..Default::default() };
        assert!(light_curve(&up, 0.8) - 0.8 > light_curve(&up, 0.2) - 0.2);
        assert!((light_curve(&Light::default(), 0.37) - 0.37).abs() < 1e-6);
    }

    #[test]
    fn brightness_moves_the_middle_and_contrast_spreads_it() {
        let x = 0.5;
        assert!(light_curve(&Light { brightness: 0.5, ..Default::default() }, x) > 0.6);
        assert!(light_curve(&Light { brightness: -0.5, ..Default::default() }, x) < 0.4);
        let c = Light { contrast: 1.0, ..Default::default() };
        assert!(light_curve(&c, 0.3) < 0.3 && light_curve(&c, 0.7) > 0.7);
        assert!((light_curve(&c, 0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn curves_pass_through_their_points_without_overshooting() {
        let points = [[0.0, 0.0], [0.25, 0.4], [0.75, 0.6], [1.0, 1.0]];
        for p in points {
            assert!((curve(&points, p[0]) - p[1]).abs() < 1e-5, "{p:?}");
        }
        let mut last = 0.0;
        for i in 0..=100 {
            let y = curve(&points, i as f32 / 100.0);
            assert!(y >= last - 1e-6 && (0.0..=1.0).contains(&y));
            last = y;
        }
        // The straight line changes nothing; a raised black lifts the shadows.
        assert!((0..=10).all(|i| (curve(&[[0.0, 0.0], [1.0, 1.0]], i as f32 / 10.0) - i as f32 / 10.0).abs() < 1e-6));
        let mut img = grey(0.5);
        curves(&mut img, &Curves { rgb: vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]], ..Default::default() });
        assert!((shown(&img) - 0.7).abs() < 0.003, "{}", shown(&img));
        // A channel's own curve, then the curve of all three.
        let c = Curves { red: vec![[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]], rgb: vec![[0.0, 0.0], [0.25, 0.5], [1.0, 1.0]], ..Default::default() };
        let luts = curve_luts(&c);
        assert!((luts[0].at(0.5) - 0.5).abs() < 0.003);
        assert!((luts[1].at(0.25) - 0.5).abs() < 0.003);
    }

    #[test]
    fn colour_keeps_grey_grey_unless_balanced() {
        let c = Color { saturation: 1.0, vibrance: 1.0, hue: 90.0, ..Default::default() };
        let g = color_of(&c, [0.2, 0.2, 0.2]);
        assert!(g.iter().all(|v| (v - 0.2).abs() < 0.003), "{g:?}");
        let warm = color_of(&Color { temperature: 1.0, ..Default::default() }, [0.2, 0.2, 0.2]);
        assert!(warm[0] > warm[1] && warm[1] > warm[2], "{warm:?}");
        let grey = color_of(&Color { saturation: -1.0, ..Default::default() }, [0.5, 0.1, 0.1]);
        assert!((grey[0] - grey[1]).abs() < 0.003 && (grey[1] - grey[2]).abs() < 0.003, "{grey:?}");
        // Half a turn of hue sends red towards cyan.
        let turned = color_of(&Color { hue: 180.0, ..Default::default() }, [0.6, 0.05, 0.05]);
        assert!(turned[0] < turned[1] && turned[0] < turned[2], "{turned:?}");
    }
}
