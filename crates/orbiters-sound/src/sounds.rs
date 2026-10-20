//! The recipes: React Bits Sound Lab's "Reveal" (logo reveal: a swell into a warm chord) and "Rocker" (a sturdy
//! two-part click). Values as in React Bits' `reveal.ts` and `rocker.ts`, helpers (`blip`, `knock`, `chord`)
//! expanded. See NOTICE.md for React Bits' licence.

use crate::synth::{Envelope, Filter, FilterKind, Layer, Ramp, Recipe, Reverb, Source, Wave};

const BELL: &[(f64, f64, f64)] = &[(1.0, 1.0, 1.0), (2.0, 0.08, 0.55), (3.0, 0.02, 0.35)];
const PLAIN: &[(f64, f64, f64)] = &[(1.0, 1.0, 1.0)];

const fn bell(freq: f64, at: f64, gain_db: f64, decay: f64) -> Layer {
    Layer {
        source: Source::Tone { freq: Ramp::Fixed(freq), wave: Wave::Sine, partials: BELL },
        at,
        gain_db,
        env: Envelope { attack: 0.004, rise: None, decay, curve: 6.0 },
        filters: &[],
    }
}

/// Played when Orbiters appears in SteamVR.
pub const REVEAL: Recipe = Recipe {
    duration: 3.4,
    seed: 142,
    loudness_db: -15.0,
    lowpass: 8500.0,
    highpass: 30.0,
    drive: 0.08,
    reverb: Some(Reverb { mix: 0.3, decay: 2.6, predelay: 0.02, damp: 5000.0 }),
    layers: &[
        Layer {
            source: Source::PinkNoise,
            at: 0.0,
            gain_db: 0.0,
            env: Envelope { attack: 1.1, rise: Some(2.1), decay: 0.12, curve: 3.0 },
            filters: &[Filter { kind: FilterKind::Lowpass, freq: Ramp::Glide { from: 400.0, to: 3800.0, time: 1.1, curve: 0.7 }, q: 0.6 }],
        },
        Layer {
            source: Source::Tone { freq: Ramp::Glide { from: 96.0, to: 58.0, time: 0.5, curve: 3.0 }, wave: Wave::Sine, partials: PLAIN },
            at: 1.12,
            gain_db: -6.0,
            env: Envelope { attack: 0.006, rise: None, decay: 1.2, curve: 5.0 },
            filters: &[],
        },
        bell(293.66, 1.12, -4.0, 2.05),
        bell(440.0, 1.132, -5.5, 2.1),
        bell(587.33, 1.144, -7.0, 2.15),
        bell(739.99, 1.156, -8.5, 2.2),
    ],
};

/// Played on a button press in the SteamVR overlay.
pub const ROCKER: Recipe = Recipe {
    duration: 0.1,
    seed: 36,
    loudness_db: -19.5,
    lowpass: 6000.0,
    highpass: 24.0,
    drive: 0.0,
    reverb: Some(Reverb { mix: 0.05, decay: 0.2, predelay: 0.01, damp: 5000.0 }),
    layers: &[
        Layer {
            source: Source::PinkNoise,
            at: 0.0,
            gain_db: -9.0,
            env: Envelope { attack: 0.0015, rise: None, decay: 0.012, curve: 6.0 },
            filters: &[
                Filter { kind: FilterKind::Bandpass, freq: Ramp::Fixed(700.0), q: 0.7 },
                Filter { kind: FilterKind::Lowpass, freq: Ramp::Fixed(1820.0), q: 0.5 },
            ],
        },
        Layer {
            source: Source::Tone { freq: Ramp::Glide { from: 240.0, to: 190.0, time: 0.012, curve: 3.0 }, wave: Wave::Triangle, partials: PLAIN },
            at: 0.0,
            gain_db: -6.0,
            env: Envelope { attack: 0.002, rise: None, decay: 0.03, curve: 6.0 },
            filters: &[Filter { kind: FilterKind::Lowpass, freq: Ramp::Fixed(1200.0), q: 0.6 }],
        },
        Layer {
            source: Source::PinkNoise,
            at: 0.032,
            gain_db: -12.0,
            env: Envelope { attack: 0.001, rise: None, decay: 0.006, curve: 6.0 },
            filters: &[
                Filter { kind: FilterKind::Bandpass, freq: Ramp::Fixed(1500.0), q: 0.9 },
                Filter { kind: FilterKind::Lowpass, freq: Ramp::Fixed(3900.0), q: 0.5 },
            ],
        },
        Layer {
            source: Source::Tone { freq: Ramp::Glide { from: 820.0, to: 720.0, time: 0.01, curve: 3.0 }, wave: Wave::Sine, partials: PLAIN },
            at: 0.032,
            gain_db: -14.0,
            env: Envelope { attack: 0.0015, rise: None, decay: 0.02, curve: 6.0 },
            filters: &[],
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::render;

    fn stats(s: &crate::synth::Sound) -> (f32, f32) {
        let peak = s.frames.iter().flat_map(|f| f.iter()).fold(0f32, |m, v| m.max(v.abs()));
        let rms = (s.frames.iter().flat_map(|f| f.iter()).map(|v| v * v).sum::<f32>() / (s.frames.len() * 2) as f32).sqrt();
        (peak, rms)
    }

    #[test]
    fn reveal_and_rocker_last_as_long_as_react_bits_renders() {
        // React Bits' exports at 48 kHz: reveal 4.644 s, rocker 0.203 s.
        let reveal = render(&REVEAL, 48_000);
        let rocker = render(&ROCKER, 48_000);
        assert!((reveal.seconds() - 4.644).abs() < 0.25, "reveal {}", reveal.seconds());
        assert!((rocker.seconds() - 0.203).abs() < 0.05, "rocker {}", rocker.seconds());
        for s in [&reveal, &rocker] {
            let (peak, rms) = stats(s);
            assert!(peak <= 0.892 && peak > 0.05 && rms > 0.005, "peak {peak} rms {rms}");
        }
        // Stereo comes from the reverb only: the channels differ, but not by much.
        let diff = reveal.frames.iter().map(|f| (f[0] - f[1]).abs()).fold(0f32, f32::max);
        assert!(diff > 1e-4, "{diff}");
    }

    #[test]
    fn rendering_is_deterministic() {
        assert_eq!(render(&ROCKER, 48_000).frames, render(&ROCKER, 48_000).frames);
    }
}
