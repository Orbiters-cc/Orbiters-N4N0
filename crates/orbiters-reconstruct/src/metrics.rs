//! Held-out metrics for lab experiments.

/// Held-out metrics of one render: PSNR over scene pixels, PSNR without the worst tenth of pixels (dynamic content
/// no static model can predict), and SSIM (11×11 Gaussian window, σ 1.5, over scene pixels).
pub fn metrics(img: &[[f32; 4]], truth: &[u32], mask: &[f32], w: usize, h: usize) -> (f64, f64, f64) {
    let ch = |p: u32, c: usize| ((p >> (8 * c)) & 255) as f32 / 255.0;
    let mut se: Vec<f64> = Vec::with_capacity(w * h);
    for k in 0..w * h {
        if mask[k] < 0.5 {
            continue;
        }
        let e: f64 = (0..3).map(|c| ((img[k][c].clamp(0.0, 1.0) - ch(truth[k], c)) as f64).powi(2)).sum::<f64>() / 3.0;
        se.push(e);
    }
    let psnr = |v: &[f64]| 10.0 * (1.0 / (v.iter().sum::<f64>() / v.len().max(1) as f64).max(1e-10)).log10();
    let full = psnr(&se);
    se.sort_by(f64::total_cmp);
    let trimmed = psnr(&se[..se.len() * 9 / 10]);
    // Separable Gaussian SSIM per channel.
    let g: Vec<f32> = {
        let v: Vec<f32> = (0..11).map(|i| (-(((i as f32) - 5.0).powi(2)) / 4.5).exp()).collect();
        let s: f32 = v.iter().sum();
        v.iter().map(|x| x / s).collect()
    };
    let blur = |src: &[f32]| -> Vec<f32> {
        let mut tmp = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut a = 0.0;
                for (t, gw) in g.iter().enumerate() {
                    let xx = x as i64 + t as i64 - 5;
                    if xx >= 0 && (xx as usize) < w {
                        a += gw * src[y * w + xx as usize];
                    }
                }
                tmp[y * w + x] = a;
            }
        }
        let mut out = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut a = 0.0;
                for (t, gw) in g.iter().enumerate() {
                    let yy = y as i64 + t as i64 - 5;
                    if yy >= 0 && (yy as usize) < h {
                        a += gw * tmp[yy as usize * w + x];
                    }
                }
                out[y * w + x] = a;
            }
        }
        out
    };
    let (mut ssim_sum, mut count) = (0f64, 0f64);
    for c in 0..3 {
        let x: Vec<f32> = (0..w * h).map(|k| if mask[k] > 0.5 { img[k][c].clamp(0.0, 1.0) } else { 0.0 }).collect();
        let y: Vec<f32> = (0..w * h).map(|k| if mask[k] > 0.5 { ch(truth[k], c) } else { 0.0 }).collect();
        let mx = blur(&x);
        let my = blur(&y);
        let xx = blur(&x.iter().map(|v| v * v).collect::<Vec<_>>());
        let yy = blur(&y.iter().map(|v| v * v).collect::<Vec<_>>());
        let xy = blur(&x.iter().zip(&y).map(|(a, b)| a * b).collect::<Vec<_>>());
        for k in 0..w * h {
            if mask[k] < 0.5 {
                continue;
            }
            let (c1, c2) = (0.0001f32, 0.0009f32);
            let s = (2.0 * mx[k] * my[k] + c1) * (2.0 * (xy[k] - mx[k] * my[k]) + c2) / ((mx[k] * mx[k] + my[k] * my[k] + c1) * ((xx[k] - mx[k] * mx[k]) + (yy[k] - my[k] * my[k]) + c2));
            ssim_sum += s as f64;
            count += 1.0;
        }
    }
    (full, trimmed, ssim_sum / count.max(1.0))
}

