//! Radix-2 FFT, just enough for the reverb's convolution.

use std::f64::consts::TAU;

/// cos and sin of −2πk/n for k < n/2. One table serves every stage of a transform of size `n` (the stage of size `s`
/// reads every (n/s)-th entry), where each butterfly used to take a sine and a cosine of its own: n/2·log₂n of them,
/// about five million per transform of the start-up reverb.
fn twiddles(n: usize) -> Vec<(f64, f64)> {
    (0..n / 2)
        .map(|k| {
            let (s, c) = (-TAU * k as f64 / n as f64).sin_cos();
            (c, s)
        })
        .collect()
}

/// In-place FFT of `re` + i·`im` (length a power of two, the size `table` was made for); `inverse` without the 1/n
/// scale.
fn fft(re: &mut [f64], im: &mut [f64], table: &[(f64, f64)], inverse: bool) {
    let n = re.len();
    // One sample is its own transform (and would shift the bit reversal below by the whole word).
    if n < 2 {
        return;
    }
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut size = 2;
    while size <= n {
        let half = size / 2;
        let stride = n / size;
        for start in (0..n).step_by(size) {
            for k in 0..half {
                let (c, s) = table[k * stride];
                let s = if inverse { -s } else { s };
                let (a, b) = (start + k, start + k + half);
                let (xr, xi) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                re[b] = re[a] - xr;
                im[b] = im[a] - xi;
                re[a] += xr;
                im[a] += xi;
            }
        }
        size *= 2;
    }
}

/// `signal` convolved with two kernels (a reverb's two channels), each result cut to the signal's length. Three
/// transforms instead of two per channel: the kernels share one as `left + i·right`, and as the signal is real, its
/// product with that transform comes back from a single inverse as `signal∗left + i·signal∗right`.
pub fn convolve_pair(signal: &[f64], kernels: [&[f64]; 2]) -> [Vec<f64>; 2] {
    let len = signal.len();
    let n = (len + kernels[0].len().max(kernels[1].len())).next_power_of_two().max(2);
    let table = twiddles(n);
    let (mut xr, mut xi) = (vec![0f64; n], vec![0f64; n]);
    xr[..len].copy_from_slice(signal);
    fft(&mut xr, &mut xi, &table, false);
    let (mut hr, mut hi) = (vec![0f64; n], vec![0f64; n]);
    hr[..kernels[0].len()].copy_from_slice(kernels[0]);
    hi[..kernels[1].len()].copy_from_slice(kernels[1]);
    fft(&mut hr, &mut hi, &table, false);
    for k in 0..n {
        let (a, b, c, d) = (xr[k], xi[k], hr[k], hi[k]);
        xr[k] = a * c - b * d;
        xi[k] = a * d + b * c;
    }
    fft(&mut xr, &mut xi, &table, true);
    let scale = 1.0 / n as f64;
    xr.truncate(len);
    xi.truncate(len);
    xr.iter_mut().chain(xi.iter_mut()).for_each(|v| *v *= scale);
    [xr, xi]
}

#[cfg(test)]
mod tests {
    #[test]
    fn convolution_matches_the_direct_sum() {
        let mut r = crate::synth::Random::new(5);
        let a: Vec<f64> = (0..300).map(|_| r.next() - 0.5).collect();
        let left: Vec<f64> = (0..77).map(|_| r.next() - 0.5).collect();
        let right: Vec<f64> = (0..120).map(|_| r.next() - 0.5).collect();
        let wet = super::convolve_pair(&a, [left.as_slice(), right.as_slice()]);
        for (kernel, fast) in [(&left, &wet[0]), (&right, &wet[1])] {
            assert_eq!(fast.len(), a.len());
            for (i, f) in fast.iter().enumerate() {
                let direct: f64 = (0..=i.min(kernel.len() - 1)).map(|k| a[i - k] * kernel[k]).sum();
                assert!((f - direct).abs() < 1e-9, "{i}: {f} vs {direct}");
            }
        }
    }

    #[test]
    fn tiny_inputs_convolve() {
        let [l, r] = super::convolve_pair(&[2.0], [&[0.5], &[]]);
        assert!((l[0] - 1.0).abs() < 1e-12 && r[0].abs() < 1e-12, "{l:?} {r:?}");
        let [l, r] = super::convolve_pair(&[], [&[], &[]]);
        assert!(l.is_empty() && r.is_empty());
    }
}
