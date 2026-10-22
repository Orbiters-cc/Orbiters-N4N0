//! The interface sounds' renderer: a port of the parts of React Bits' Sound Lab engine (`renderSound`, Web Audio's
//! OfflineAudioContext) that Orbiters' recipes use, so the sounds are computed at start-up instead of shipped as
//! files. Layers (tones with partials, pink noise) go through their envelopes and Web Audio biquads into a mono bus;
//! the bus gets the master filters, a soft drive and a convolution reverb with a generated stereo impulse, then the
//! result is trimmed and normalised to the recipe's loudness. See NOTICE.md for React Bits' licence.

use crate::fft;
use std::f64::consts::{PI, TAU};

#[derive(Clone, Copy, Debug)]
pub enum Ramp {
    Fixed(f64),
    /// From `from` to `to` over `time` seconds, eased by `curve` (≥ 1: fast start; < 1: slow start).
    Glide { from: f64, to: f64, time: f64, curve: f64 },
}

#[derive(Clone, Copy, Debug)]
pub struct Envelope {
    pub attack: f64,
    /// Attack shape `y^rise`; a quarter sine without it.
    pub rise: Option<f64>,
    pub decay: f64,
    pub curve: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    Lowpass,
    Highpass,
    Bandpass,
}

#[derive(Clone, Copy, Debug)]
pub struct Filter {
    pub kind: FilterKind,
    pub freq: Ramp,
    /// Web Audio's Q: in dB for low- and highpass, linear for bandpass.
    pub q: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wave {
    Sine,
    Triangle,
}

#[derive(Clone, Copy, Debug)]
pub enum Source {
    /// (ratio, gain, decay scale) partials of `freq`.
    Tone { freq: Ramp, wave: Wave, partials: &'static [(f64, f64, f64)] },
    PinkNoise,
}

#[derive(Clone, Copy, Debug)]
pub struct Layer {
    pub source: Source,
    pub at: f64,
    pub gain_db: f64,
    pub env: Envelope,
    pub filters: &'static [Filter],
}

#[derive(Clone, Copy, Debug)]
pub struct Reverb {
    pub mix: f64,
    pub decay: f64,
    pub predelay: f64,
    pub damp: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct Recipe {
    pub duration: f64,
    pub seed: u32,
    pub loudness_db: f64,
    pub lowpass: f64,
    pub highpass: f64,
    pub drive: f64,
    pub reverb: Option<Reverb>,
    pub layers: &'static [Layer],
}

/// A rendered sound: interleaved stereo at `rate`.
#[derive(Clone, Debug, Default)]
pub struct Sound {
    pub rate: u32,
    pub frames: Vec<[f32; 2]>,
}

impl Sound {
    pub fn seconds(&self) -> f64 {
        self.frames.len() as f64 / self.rate.max(1) as f64
    }
}

fn db(v: f64) -> f64 {
    10f64.powf(v / 20.0)
}

/// React Bits' seeded generator (mulberry32-style), in 0..1.
pub(crate) struct Random(u32);

impl Random {
    pub(crate) fn new(seed: u64) -> Self {
        let s = (seed % (1u64 << 32)) as u32;
        Self(if s == 0 { 1 } else { s })
    }

    pub(crate) fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4_294_967_296.0
    }
}

impl Ramp {
    /// Value `time` seconds into the layer; frequencies glide geometrically (`log`).
    fn at(&self, time: f64, log: bool) -> f64 {
        match *self {
            Ramp::Fixed(v) => v,
            Ramp::Glide { from, to, time: span, curve } => {
                let y = (time / span.max(1e-4)).clamp(0.0, 1.0);
                let e = if curve >= 1.0 { 1.0 - (1.0 - y).powf(curve) } else { y.powf(1.0 / curve.max(1e-3)) };
                if log && from > 0.0 && to > 0.0 {
                    from * (to / from).powf(e)
                } else {
                    from + (to - from) * e
                }
            }
        }
    }

    fn fixed(&self) -> bool {
        matches!(self, Ramp::Fixed(_))
    }
}

impl Envelope {
    fn length(&self) -> f64 {
        self.attack + self.decay
    }

    fn at(&self, time: f64, decay_scale: f64) -> f64 {
        let attack = self.attack.max(0.0004);
        let decay = (self.decay * decay_scale).max(0.002);
        if time <= 0.0 {
            return 0.0;
        }
        if time < attack {
            let y = time / attack;
            return match self.rise {
                Some(r) => y.powf(r),
                None => (y * PI / 2.0).sin(),
            };
        }
        let t = time - attack;
        if t < decay {
            let k = self.curve;
            // No curve is a straight line (the exponential's limit, where its formula divides 0 by 0).
            if k.abs() < 1e-9 {
                return 1.0 - t / decay;
            }
            ((-k * t / decay).exp() - (-k).exp()) / (1.0 - (-k).exp())
        } else {
            0.0
        }
    }
}

/// Web Audio's BiquadFilterNode (Audio EQ Cookbook, Q as the spec reads it), direct form I.
struct Biquad {
    kind: FilterKind,
    rate: f64,
    /// 1 / 2Q, with Q as Web Audio reads it for this kind (computed once: a glide sets the frequency every sample).
    half_inv_q: f64,
    b: [f64; 3],
    a: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
}

impl Biquad {
    fn new(kind: FilterKind, freq: f64, q: f64, rate: f64) -> Self {
        let q = match kind {
            FilterKind::Bandpass => q.max(1e-4),
            _ => 10f64.powf(q / 20.0),
        };
        let mut f = Self { kind, rate, half_inv_q: 0.5 / q, b: [0.0; 3], a: [0.0; 2], x: [0.0; 2], y: [0.0; 2] };
        f.set(freq);
        f
    }

    fn set(&mut self, freq: f64) {
        let w0 = TAU * (freq / self.rate).clamp(0.0, 0.5);
        let (s, c) = w0.sin_cos();
        let alpha = s * self.half_inv_q;
        let (b0, b1, b2) = match self.kind {
            FilterKind::Lowpass => ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0),
            FilterKind::Highpass => ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0),
            FilterKind::Bandpass => (alpha, 0.0, -alpha),
        };
        let a0 = 1.0 + alpha;
        self.b = [b0 / a0, b1 / a0, b2 / a0];
        self.a = [-2.0 * c / a0, (1.0 - alpha) / a0];
    }

    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.b[1] * self.x[0] + self.b[2] * self.x[1] - self.a[0] * self.y[0] - self.a[1] * self.y[1];
        self.x = [x, self.x[0]];
        self.y = [y, self.y[0]];
        y
    }
}

/// Pink noise (Paul Kellet's filter on React Bits' generator).
fn pink_noise(frames: usize, seed: u64) -> Vec<f64> {
    let mut next = Random::new(seed);
    let mut b = [0f64; 7];
    (0..frames)
        .map(|_| {
            let white = next.next() * 2.0 - 1.0;
            b[0] = 0.99886 * b[0] + white * 0.0555179;
            b[1] = 0.99332 * b[1] + white * 0.0750759;
            b[2] = 0.969 * b[2] + white * 0.153852;
            b[3] = 0.8665 * b[3] + white * 0.3104856;
            b[4] = 0.55 * b[4] + white * 0.5329522;
            b[5] = -0.7616 * b[5] - white * 0.016898;
            let v = (b.iter().sum::<f64>() + white * 0.5362) * 0.11;
            b[6] = white * 0.115926;
            v
        })
        .collect()
}

/// The reverb's impulse response per channel: damped noise decaying by 60 dB over `decay`, a few early taps, unit
/// energy.
fn impulse(rate: f64, decay: f64, predelay: f64, damp: f64, seed: u64) -> [Vec<f64>; 2] {
    // A reverb without a tail would divide by zero below.
    let decay = decay.max(1e-3);
    let length = ((decay * 1.15 + predelay) * rate).ceil() as usize;
    let taps = [0.0071, 0.0113, 0.0167, 0.0229, 0.0311, 0.0373];
    std::array::from_fn(|channel| {
        let mut data = vec![0f64; length];
        let mut next = Random::new(seed * 31 + channel as u64 * 977 + 17);
        let (mut low, mut energy) = (0.0, 0.0);
        for (i, d) in data.iter_mut().enumerate() {
            let time = i as f64 / rate - predelay;
            if time < 0.0 {
                continue;
            }
            let cutoff = damp * 0.18f64.powf(time / decay) + 300.0;
            let a = 1.0 - (-TAU * cutoff / rate).exp();
            low += a * (next.next() * 2.0 - 1.0 - low);
            let swell = (time / 0.004).min(1.0);
            let value = low * (-6.9 * time / decay).exp() * swell;
            *d = value;
            energy += value * value;
        }
        for (index, tap) in taps.iter().enumerate() {
            let at = ((predelay + tap * if channel == 1 { 1.13 } else { 1.0 }) * rate).round() as usize;
            if at < length {
                data[at] += (0.5 - index as f64 * 0.06) * (energy / length as f64).sqrt() * 3.0;
            }
        }
        let total: f64 = data.iter().map(|v| v * v).sum();
        let scale = 1.0 / total.max(1e-12).sqrt();
        data.iter_mut().for_each(|v| *v *= scale);
        data
    })
}

/// A shape sampled like Web Audio's `setValueCurveAtTime` (`per_second` points over `span`, linear in between,
/// the last value held): React Bits automates every envelope and glide that way, which softens the shortest attacks.
struct Curve {
    values: Vec<f64>,
    span: f64,
}

impl Curve {
    fn new(span: f64, per_second: f64, shape: impl Fn(f64) -> f64) -> Self {
        let steps = ((span * per_second).ceil() as usize).clamp(2, 4096);
        Self { values: (0..steps).map(|i| shape(i as f64 / (steps - 1) as f64 * span)).collect(), span }
    }

    fn at(&self, t: f64) -> f64 {
        let x = (t / self.span).clamp(0.0, 1.0) * (self.values.len() - 1) as f64;
        let k = (x as usize).min(self.values.len() - 2);
        self.values[k] + (self.values[k + 1] - self.values[k]) * (x - k as f64)
    }

    /// The time from which [`Curve::at`] returns the same value every time (a glide that ended).
    fn settles_at(&self) -> f64 {
        let last = self.values[self.values.len() - 1];
        match self.values.iter().rposition(|v| *v != last) {
            Some(i) => (i + 1) as f64 / (self.values.len() - 1) as f64 * self.span,
            None => 0.0,
        }
    }
}

fn oscillator(wave: Wave, phase: f64) -> f64 {
    match wave {
        Wave::Sine => (TAU * phase).sin(),
        Wave::Triangle => {
            let p = phase.fract();
            if p < 0.25 {
                4.0 * p
            } else if p < 0.75 {
                2.0 - 4.0 * p
            } else {
                4.0 * p - 4.0
            }
        }
    }
}

/// Adds one layer to the bus (frames 0..`bus.len()`, `body` seconds long).
fn layer(bus: &mut [f64], l: &Layer, index: usize, recipe: &Recipe, rate: f64, body: f64) {
    let start = l.at;
    let span = (l.env.length() + 0.01).min(body - start);
    if span <= 0.0 {
        return;
    }
    // (At absurdly low rates the clamps below still get a range.)
    let nyquist = (rate / 2.0 - 100.0).max(10.0);
    let first = (start * rate).ceil() as usize;
    let last = (((start + span) * rate).ceil() as usize).min(bus.len());
    let mut signal = vec![0f64; bus.len() - first.min(bus.len())];
    match l.source {
        Source::PinkNoise => {
            let seed = recipe.seed as u64 * 7919 + index as u64 * 104_729;
            let noise = pink_noise(((span + 0.05) * rate).ceil() as usize, seed);
            let env = Curve::new(span, 600.0, |t| l.env.at(t, 1.0));
            for i in first..last {
                let t = i as f64 / rate - start;
                let j = (t * rate).round() as usize;
                signal[i - first] = noise.get(j).copied().unwrap_or(0.0) * env.at(t);
            }
        }
        Source::Tone { freq, wave, partials } => {
            for &(ratio, gain, decay_scale) in partials {
                let env = Curve::new(span, 600.0, |t| l.env.at(t, decay_scale) * gain);
                if let (Ramp::Fixed(f), Wave::Sine) = (freq, wave) {
                    // A steady sine turns by the same angle every sample: a phasor rotated by it, instead of a sine
                    // per sample (the bells' partials, most of the reveal's oscillators).
                    let (ds, dc) = (TAU * (f * ratio).clamp(1.0, nyquist) / rate).sin_cos();
                    let (mut s, mut c) = (0f64, 1f64);
                    for i in first..last {
                        let t = i as f64 / rate - start;
                        signal[i - first] += s * env.at(t);
                        (s, c) = (s * dc + c * ds, c * dc - s * ds);
                        // Rounding slowly changes its length: back onto the unit circle now and then.
                        if (i - first) % 1024 == 1023 {
                            let r = (s * s + c * c).sqrt().recip();
                            (s, c) = (s * r, c * r);
                        }
                    }
                    continue;
                }
                let pitch = Curve::new(span, 400.0, |t| (freq.at(t, true) * ratio).clamp(1.0, nyquist));
                let mut phase = 0.0;
                for i in first..last {
                    let t = i as f64 / rate - start;
                    signal[i - first] += oscillator(wave, phase) * env.at(t);
                    phase += pitch.at(t) / rate;
                }
            }
        }
    }
    let g = db(l.gain_db);
    signal.iter_mut().for_each(|v| *v *= g);
    for spec in l.filters {
        let freq = Curve::new(span, 400.0, |t| spec.freq.at(t, true).clamp(10.0, nyquist));
        let mut f = Biquad::new(spec.kind, freq.at(0.0), spec.q, rate);
        // A glide moves the filter until its curve settles; the coefficients then stay what they are.
        let moving = if spec.freq.fixed() { 0 } else { (freq.settles_at() * rate).ceil() as usize + 1 };
        for (k, v) in signal.iter_mut().enumerate() {
            if k < moving {
                f.set(freq.at(k as f64 / rate));
            }
            *v = f.run(*v);
        }
    }
    let from = first.min(bus.len());
    for (b, v) in bus[from..].iter_mut().zip(signal) {
        *b += v;
    }
}

pub fn render(recipe: &Recipe, rate: u32) -> Sound {
    let r = rate as f64;
    let body = recipe.duration;
    let reverb = recipe.reverb.filter(|v| v.mix > 0.0);
    let space_mix = reverb.map(|v| v.mix.clamp(0.0, 0.9)).unwrap_or(0.0);
    let end = body + reverb.map(|v| v.decay * 1.1 + v.predelay).unwrap_or(0.0) + 0.05;
    let n = (end * r).ceil() as usize;
    let mut bus = vec![0f64; n];
    for (i, l) in recipe.layers.iter().enumerate() {
        layer(&mut bus, l, i, recipe, r, body);
    }

    // Master: highpass, lowpass (the shelves are flat at the default tone), drive.
    let mut hp = Biquad::new(FilterKind::Highpass, recipe.highpass, 0.6, r);
    let mut lp = Biquad::new(FilterKind::Lowpass, recipe.lowpass.clamp(400.0, (r / 2.0 - 200.0).max(400.0)), 0.5, r);
    let drive = recipe.drive.clamp(0.0, 1.0);
    let (k, norm) = (1.0 + drive * 6.0, (1.0 + drive * 6.0).tanh());
    for v in bus.iter_mut() {
        let mut x = lp.run(hp.run(*v));
        if drive > 0.0 {
            x = ((x * 0.5).clamp(-1.0, 1.0) * k).tanh() / norm;
        }
        *v = x;
    }

    let mut channels = [bus.clone(), bus];
    if let Some(rv) = reverb {
        // Both channels start as the same dry bus: one convolution gives both wet sides.
        let ir = impulse(r, rv.decay, rv.predelay, rv.damp, recipe.seed as u64);
        let wet = fft::convolve_pair(&channels[0], [ir[0].as_slice(), ir[1].as_slice()]);
        for (c, w) in channels.iter_mut().zip(wet) {
            for (v, w) in c.iter_mut().zip(w) {
                *v += w * space_mix * 2.2;
            }
        }
    }
    finish(channels, recipe, rate)
}

/// Loudest 50 ms window's RMS (both channels).
fn loudness(channels: &[Vec<f64>; 2], rate: f64) -> f64 {
    let window = ((rate * 0.05).round() as usize).max(1);
    let len = channels[0].len();
    let mut best = 0f64;
    let mut start = 0;
    while start < len {
        let stop = (start + window).min(len);
        let sum: f64 = channels.iter().map(|c| c[start..stop].iter().map(|v| v * v).sum::<f64>()).sum();
        best = best.max(sum / ((stop - start) * 2) as f64);
        start += (window as f64 / 2.0).round() as usize;
    }
    best.sqrt()
}

/// Trims the silence around the sound, removes DC, normalises to the recipe's loudness (peaks under −1 dBFS) and
/// fades the ends.
fn finish(mut channels: [Vec<f64>; 2], recipe: &Recipe, rate: u32) -> Sound {
    let r = rate as f64;
    let floor = db(-70.0);
    let len = channels[0].len();
    let first = channels.iter().filter_map(|c| c.iter().position(|v| v.abs() > floor)).min().unwrap_or(len);
    let lead = first.saturating_sub((r * 0.002).round() as usize);
    if lead > 0 {
        for c in channels.iter_mut() {
            c.copy_within(lead.., 0);
            c[len - lead..].fill(0.0);
        }
    }
    let last = channels.iter().filter_map(|c| c.iter().rposition(|v| v.abs() > floor)).max().unwrap_or(0);
    let length = len.min(last + (r * 0.02).round() as usize);
    let fade_out = ((r * 0.02).round() as usize).min(length);
    let fade_in = (r * 0.0006).round() as usize;
    let mean = channels.iter().map(|c| c[..length].iter().sum::<f64>()).sum::<f64>() / (length * 2).max(1) as f64;
    let level = loudness(&channels, r);
    let mut gain = if level > 1e-6 { db(recipe.loudness_db) / level } else { 1.0 };
    let peak = channels.iter().flat_map(|c| c[..length].iter()).fold(0f64, |m, v| m.max((v - mean).abs()));
    if peak * gain > db(-1.0) {
        gain = db(-1.0) / peak;
    }
    let frames = (0..length)
        .map(|i| {
            std::array::from_fn(|ch| {
                let mut v = (channels[ch][i] - mean) * gain;
                if i < fade_in {
                    v *= (i as f64 / fade_in as f64 * PI / 2.0).sin();
                }
                let remaining = length - i;
                if remaining < fade_out {
                    v *= 0.5 - 0.5 * (remaining as f64 / fade_out as f64 * PI).cos();
                }
                v.clamp(-0.999, 0.999) as f32
            })
        })
        .collect();
    Sound { rate, frames }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generator_matches_react_bits() {
        // First values of `random(1)` in React Bits' engine.
        let mut r = Random::new(1);
        let v: Vec<f64> = (0..3).map(|_| r.next()).collect();
        assert!((v[0] - 0.6270739405881613).abs() < 1e-12, "{v:?}");
        assert!((v[1] - 0.002735721180215478).abs() < 1e-12, "{v:?}");
    }

    #[test]
    fn biquads_pass_what_they_should() {
        let rate = 48_000.0;
        let gain = |kind, f_test: f64| {
            let mut f = Biquad::new(kind, 1000.0, 0.7, rate);
            let mut peak = 0f64;
            for i in 0..48_000 {
                let y = f.run((TAU * f_test * i as f64 / rate).sin());
                if i > 24_000 {
                    peak = peak.max(y.abs());
                }
            }
            peak
        };
        assert!(gain(FilterKind::Lowpass, 100.0) > 0.95 && gain(FilterKind::Lowpass, 10_000.0) < 0.02);
        assert!(gain(FilterKind::Highpass, 10_000.0) > 0.95 && gain(FilterKind::Highpass, 100.0) < 0.02);
        assert!(gain(FilterKind::Bandpass, 1000.0) > 0.95 && gain(FilterKind::Bandpass, 100.0) < 0.2);
    }

    #[test]
    fn a_glide_settles_where_it_ends() {
        // The reveal's noise sweep: its filter stops being set once the sweep is over.
        let glide = Ramp::Glide { from: 400.0, to: 3800.0, time: 1.1, curve: 0.7 };
        let curve = Curve::new(1.5, 400.0, |t| glide.at(t, true));
        let at = curve.settles_at();
        assert!(at > 1.09 && at < 1.11, "{at}");
        assert_eq!(curve.at(at + 0.01), curve.at(1.5));
        assert_ne!(curve.at(at - 0.01), curve.at(1.5));
        assert_eq!(Curve::new(1.0, 400.0, |_| 700.0).settles_at(), 0.0);
    }

    #[test]
    fn odd_settings_neither_panic_nor_give_nan() {
        // No curve: a straight decay.
        let env = Envelope { attack: 0.01, rise: None, decay: 0.1, curve: 0.0 };
        assert!((env.at(0.06, 1.0) - 0.5).abs() < 1e-9);
        // A low sample rate squeezes every clamp's range.
        let s = render(&crate::sounds::ROCKER, 800);
        assert!(!s.frames.is_empty() && s.frames.iter().all(|f| f[0].is_finite() && f[1].is_finite()));
    }

    #[test]
    fn a_steady_sine_rotates_like_the_sine_it_replaces() {
        // One bell partial: the phasor's samples against sin() of the accumulated phase, over two seconds.
        const BELL: &[Layer] = &[Layer {
            source: Source::Tone { freq: Ramp::Fixed(440.0), wave: Wave::Sine, partials: &[(1.0, 1.0, 1.0)] },
            at: 0.0,
            gain_db: 0.0,
            env: Envelope { attack: 0.004, rise: None, decay: 2.0, curve: 6.0 },
            filters: &[],
        }];
        let recipe = Recipe { duration: 2.1, seed: 1, loudness_db: -15.0, lowpass: 8000.0, highpass: 30.0, drive: 0.0, reverb: None, layers: BELL };
        let rate = 48_000.0;
        let mut bus = vec![0f64; (2.1 * rate) as usize];
        layer(&mut bus, &BELL[0], 0, &recipe, rate, recipe.duration);
        let env = Curve::new(2.014, 600.0, |t| BELL[0].env.at(t, 1.0));
        for (i, v) in bus.iter().enumerate().step_by(97) {
            let t = i as f64 / rate;
            let expected = (TAU * 440.0 * t).sin() * env.at(t);
            assert!((v - expected).abs() < 1e-9, "{i}: {v} vs {expected}");
        }
    }
}
