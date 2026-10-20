//! Radix-2 FFT, just enough for the reverb's convolution.

use std::f64::consts::TAU;

/// In-place FFT of `re` + i·`im` (length a power of two); `inverse` without the 1/n scale.
fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut size = 2;
    while size <= n {
        let half = size / 2;
        let step = sign * TAU / size as f64;
        for start in (0..n).step_by(size) {
            for k in 0..half {
                let (s, c) = (step * k as f64).sin_cos();
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

/// `signal` convolved with `kernel`, cut to the signal's length.
pub fn convolve(signal: &[f64], kernel: &[f64]) -> Vec<f64> {
    let n = (signal.len() + kernel.len()).next_power_of_two();
    // Both real: one complex transform carries the signal (real part) and the kernel (imaginary part).
    let mut re = vec![0f64; n];
    let mut im = vec![0f64; n];
    re[..signal.len()].copy_from_slice(signal);
    im[..kernel.len()].copy_from_slice(kernel);
    fft(&mut re, &mut im, false);
    // X = (Z[k] + conj Z[n−k]) / 2, H = (Z[k] − conj Z[n−k]) / 2i; the product X·H goes back.
    let (mut pr, mut pi) = (vec![0f64; n], vec![0f64; n]);
    for k in 0..n {
        let m = (n - k) % n;
        let (xr, xi) = ((re[k] + re[m]) / 2.0, (im[k] - im[m]) / 2.0);
        let (hr, hi) = ((im[k] + im[m]) / 2.0, (re[m] - re[k]) / 2.0);
        pr[k] = xr * hr - xi * hi;
        pi[k] = xr * hi + xi * hr;
    }
    fft(&mut pr, &mut pi, true);
    pr.truncate(signal.len());
    pr.iter_mut().for_each(|v| *v /= n as f64);
    pr
}

#[cfg(test)]
mod tests {
    #[test]
    fn convolution_matches_the_direct_sum() {
        let mut r = crate::synth::Random::new(5);
        let a: Vec<f64> = (0..300).map(|_| r.next() - 0.5).collect();
        let b: Vec<f64> = (0..77).map(|_| r.next() - 0.5).collect();
        let fast = super::convolve(&a, &b);
        for (i, f) in fast.iter().enumerate() {
            let direct: f64 = (0..=i.min(b.len() - 1)).map(|k| a[i - k] * b[k]).sum();
            assert!((f - direct).abs() < 1e-9, "{i}: {f} vs {direct}");
        }
    }
}
