//! The reference renderer, in f64: forward and backward exactly as the GPU kernels compute them (the same culling
//! by tile_hit, thresholds and order; the GPU decides borderline tiles in f32), slow and simple. Tests check its
//! gradients against finite differences, and the GPU against it.
//!
//! Model (3DGS with Mip-Splatting's filters): a Gaussian, its axes widened by its 3D filter, projects to a 2D Gaussian
//! (EWA splatting with the 3DGS frustum clamp) convolved with the 2D filter; both filters scale its opacity so that it
//! keeps its weight. Pixels blend the Gaussians front to back, `C = Σ cᵢ αᵢ Tᵢ + T·background`, with
//! `αᵢ = min(0.99, opacityᵢ · exp(powerᵢ))`, skipping `α < 1/255` and stopping before `T` would drop under 10⁻⁴.
//! The depth channel is `D = Σ zᵢ αᵢ Tᵢ` (unnormalised: an opaque surface renders its depth).

// Index loops here mirror the formulas (several arrays share each index).
#![allow(clippy::needless_range_loop)]

use crate::camera::Camera;
use crate::gaussians::Gaussians;

pub const TILE: u32 = 16;
pub const NEAR: f64 = 0.05;
/// Mip-Splatting's 2D filter: every splat is convolved with a screen-space Gaussian of this variance (pixels², about a
/// pixel's box), its opacity scaled by `√(det Σ / det(Σ + filter))` so that it keeps its weight. Unlike 3DGS's 0.3 px
/// dilation, a splat smaller than a pixel then fades instead of growing, so renders at other distances and
/// resolutions than the keyframes' neither thicken nor brighten it.
pub const FILTER_2D: f64 = 0.1;
/// Below this share of the 2D filter's own determinant, a projected covariance counts as flat for the filter's
/// compensation (which has an infinite slope at 0).
pub const FLAT: f64 = 1e-6;
/// Mip-Splatting's 3D filter: each Gaussian's axes get the variance `FILTER_3D · (depth / focal)²` of the training
/// camera that samples it most finely, a fifth of a pixel² there, so that no Gaussian is finer than the keyframes could
/// show: up close it stays smooth instead of turning into needles.
pub const FILTER_3D: f64 = 0.2;
pub const MAX_ALPHA: f64 = 0.99;
pub const MIN_ALPHA: f64 = 1.0 / 255.0;
pub const MIN_T: f64 = 1e-4;
/// The 3DGS frustum clamp: x/z and y/z beyond 1.3 × the half field of view use the clamped value in the Jacobian.
pub const FRUSTUM_CLAMP: f64 = 1.3;

/// How strictly pixels follow the GPU: `GPU` culls by tile and skips faint contributions exactly as the kernels
/// do; `SMOOTH` evaluates every Gaussian everywhere without the 1/255 cut-off (for finite-difference checks, where
/// a cut-off crossed by a nudge would fake a gradient error).
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    pub cull_tiles: bool,
    pub min_alpha: f64,
}

impl Rules {
    pub const GPU: Self = Self { cull_tiles: true, min_alpha: MIN_ALPHA };
    pub const SMOOTH: Self = Self { cull_tiles: false, min_alpha: 0.0 };
}

/// D-SSIM's stabilising constants (C1 = 0.01², C2 = 0.03²), as ssim.wgsl uses them.
pub const SSIM_C1: f64 = 0.0001;
pub const SSIM_C2: f64 = 0.0009;

/// A Gaussian as the image sees it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Projected {
    pub valid: bool,
    pub mean: [f64; 2],
    /// Inverse 2D covariance (a, b, c): power = −½(a·dx² + c·dy²) − b·dx·dy.
    pub conic: [f64; 3],
    pub depth: f64,
    pub opacity: f64,
    pub color: [f64; 3],
    pub radius: f64,
    /// Tile rectangle of the ellipse's own extent (x0, y0, x1, y1), half-open; within it, the tiles [`tile_hit`] passes.
    pub tiles: [u32; 4],
    /// Largest quadratic form at which this Gaussian still reaches α = 1/255 (common.wgsl `alpha_reach`).
    pub reach: f64,
}

/// Whether a Gaussian centred at `m` with conic (A, B, C) reaches the quadratic form `tau` at any pixel centre of
/// tile (`tx`, `ty`): common.wgsl's `tile_hit`, which decides the tiles a Gaussian is drawn in.
pub fn tile_hit(m: [f64; 2], conic: [f64; 3], tau: f64, tx: u32, ty: u32) -> bool {
    let lo = [(tx * TILE) as f64 - m[0], (ty * TILE) as f64 - m[1]];
    let hi = [lo[0] + (TILE - 1) as f64, lo[1] + (TILE - 1) as f64];
    if lo[0] <= 0.0 && hi[0] >= 0.0 && lo[1] <= 0.0 && hi[1] >= 0.0 {
        return true;
    }
    let q = |x: f64, y: f64| conic[0] * x * x + 2.0 * conic[1] * x * y + conic[2] * y * y;
    let mut best = f64::MAX;
    for k in 0..2 {
        let dx = if k == 1 { hi[0] } else { lo[0] };
        best = best.min(q(dx, (-conic[1] * dx / conic[2]).clamp(lo[1], hi[1])));
        let ey = if k == 1 { hi[1] } else { lo[1] };
        best = best.min(q((-conic[1] * ey / conic[0]).clamp(lo[0], hi[0]), ey));
    }
    best <= tau
}

/// SSIM of one window from its means μx, μy and moments E[x²], E[y²], E[xy].
pub fn ssim(mx: f64, my: f64, exx: f64, eyy: f64, exy: f64) -> f64 {
    let a = 2.0 * mx * my + SSIM_C1;
    let b = 2.0 * (exy - mx * my) + SSIM_C2;
    let c = mx * mx + my * my + SSIM_C1;
    let d = (exx - mx * mx) + (eyy - my * my) + SSIM_C2;
    a * b / (c * d)
}

/// [`ssim`]'s partial derivatives ∂S/∂μx, ∂S/∂E[x²], ∂S/∂E[xy], as ssim.wgsl computes them (fused-ssim's algebra):
/// divided only by c ≥ C1 and d ≳ C2, never by a or b, which cross zero in anti-correlated windows.
pub fn ssim_partials(mx: f64, my: f64, exx: f64, eyy: f64, exy: f64) -> [f64; 3] {
    let a = 2.0 * mx * my + SSIM_C1;
    let b = 2.0 * (exy - mx * my) + SSIM_C2;
    let c = mx * mx + my * my + SSIM_C1;
    let d = (exx - mx * mx) + (eyy - my * my) + SSIM_C2;
    let cd = c * d;
    let s = a * b / cd;
    [2.0 * my * (b - a) / cd - 2.0 * mx * s * (1.0 / c - 1.0 / d), -s / d, 2.0 * a / cd]
}

pub struct Image {
    pub width: u32,
    pub height: u32,
    pub color: Vec<f64>,
    pub depth: Vec<f64>,
    pub transmittance: Vec<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct Grads {
    pub means: Vec<[f64; 3]>,
    pub log_scales: Vec<[f64; 3]>,
    pub rotations: Vec<[f64; 4]>,
    pub opacity_logits: Vec<f64>,
    pub colors: Vec<[f64; 3]>,
}

type M3 = [[f64; 3]; 3];

fn m3(r: &[[f32; 3]; 3]) -> M3 {
    r.map(|row| row.map(|v| v as f64))
}

fn mul(a: &M3, b: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

fn transpose(a: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = a[j][i];
        }
    }
    o
}

fn quat_matrix(qn: [f64; 4]) -> M3 {
    let [w, x, y, z] = qn;
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// Intermediate values of one Gaussian's projection, kept for its backward pass.
struct Proj {
    p: Projected,
    pc: [f64; 3],
    w: M3,
    rq: M3,
    /// Scales, and with the 3D filter's variance `filter` added to their squares; the compensations c3 = Π s / s'
    /// (3D) and c2 = √(det A / det(A + F)) (2D, A the projected covariance), and the opacity before them.
    s: [f64; 3],
    sf: [f64; 3],
    filter: f64,
    c3: f64,
    c2: f64,
    a: [f64; 3],
    det_a: f64,
    op: f64,
    qn: [f64; 4],
    qlen: f64,
    sigma_c: M3,
    j: [[f64; 3]; 2],
    clamp_x: bool,
    clamp_y: bool,
    u: f64,
    v: f64,
}

fn project_one(g: &Gaussians, i: usize, cam: &Camera) -> Proj {
    let w = m3(&cam.r);
    let m = g.means[i].map(|v| v as f64);
    let t = cam.t.map(|v| v as f64);
    let pc = [0, 1, 2].map(|k| w[k][0] * m[0] + w[k][1] * m[1] + w[k][2] * m[2] + t[k]);
    let q = g.rotations[i].map(|v| v as f64);
    let qlen = (q.iter().map(|c| c * c).sum::<f64>()).sqrt().max(1e-12);
    let qn = q.map(|c| c / qlen);
    let rq = quat_matrix(qn);
    let s = g.log_scales[i].map(|v| (v as f64).exp());
    let filter = g.filter(i) as f64;
    let sf = s.map(|v| (v * v + filter).sqrt());
    let c3 = (0..3).map(|k| s[k] / sf[k]).product();
    let mut proj = Proj {
        p: Projected::default(),
        pc,
        w,
        rq,
        s,
        sf,
        filter,
        c3,
        c2: 0.0,
        a: [0.0; 3],
        det_a: 0.0,
        op: 0.0,
        qn,
        qlen,
        sigma_c: [[0.0; 3]; 3],
        j: [[0.0; 3]; 2],
        clamp_x: false,
        clamp_y: false,
        u: 0.0,
        v: 0.0,
    };
    let (x, y, z) = (pc[0], pc[1], pc[2]);
    if z < NEAR {
        return proj;
    }
    let (fx, fy) = (cam.fx as f64, cam.fy as f64);
    // Σw = (Rq S')(Rq S')ᵀ with the filtered scales, Σc = W Σw Wᵀ.
    let mut ms = rq;
    for row in &mut ms {
        for (k, v) in row.iter_mut().enumerate() {
            *v *= sf[k];
        }
    }
    let sigma_w = mul(&ms, &transpose(&ms));
    let sigma_c = mul(&mul(&w, &sigma_w), &transpose(&w));
    let lim_x = FRUSTUM_CLAMP * (0.5 * cam.width as f64 / fx);
    let lim_y = FRUSTUM_CLAMP * (0.5 * cam.height as f64 / fy);
    let (ux, uy) = (x / z, y / z);
    let (u, v) = (ux.clamp(-lim_x, lim_x), uy.clamp(-lim_y, lim_y));
    let j = [[fx / z, 0.0, -fx * u / z], [0.0, fy / z, -fy * v / z]];
    // cov2D = A + F with A = J Σc Jᵀ and F the 2D filter.
    let js = [0, 1].map(|r| [0, 1, 2].map(|c| (0..3).map(|k| j[r][k] * sigma_c[k][c]).sum::<f64>()));
    let raw = [(0..3).map(|k| js[0][k] * j[0][k]).sum::<f64>(), (0..3).map(|k| js[0][k] * j[1][k]).sum::<f64>(), (0..3).map(|k| js[1][k] * j[1][k]).sum::<f64>()];
    let (a, b, c) = (raw[0] + FILTER_2D, raw[1], raw[2] + FILTER_2D);
    let det = a * c - b * b;
    if det <= 0.0 {
        return proj;
    }
    let mean = [fx * x / z + cam.cx as f64, fy * y / z + cam.cy as f64];
    let det_a = raw[0] * raw[2] - raw[1] * raw[1];
    let c2 = (det_a.max(FLAT * FILTER_2D * FILTER_2D) / det).sqrt();
    let op_raw = 1.0 / (1.0 + (-(g.opacity_logits[i] as f64)).exp());
    // The opacity the pixels see.
    let op = op_raw * c3 * c2;
    let mid = 0.5 * (a + c);
    let lambda = mid + (mid * mid - det).max(0.1).sqrt();
    // Out to α = 1/255 at this opacity; one that never gets there draws nothing (project.wgsl).
    let sigmas = extent_sigmas(op);
    let reach = sigmas * sigmas;
    if reach <= 0.0 {
        return proj;
    }
    let radius = (sigmas * lambda.sqrt()).ceil();
    // The ellipse's own extent per axis, then only the tiles it reaches (tile_hit), as project.wgsl counts them.
    let (rx, ry) = (sigmas * a.sqrt(), sigmas * c.sqrt());
    let tiles_x = cam.width.div_ceil(TILE) as f64;
    let tiles_y = cam.height.div_ceil(TILE) as f64;
    let tx0 = ((mean[0] - rx) / TILE as f64).floor().clamp(0.0, tiles_x);
    let ty0 = ((mean[1] - ry) / TILE as f64).floor().clamp(0.0, tiles_y);
    let tx1 = ((mean[0] + rx) / TILE as f64 + 1.0).floor().clamp(0.0, tiles_x);
    let ty1 = ((mean[1] + ry) / TILE as f64 + 1.0).floor().clamp(0.0, tiles_y);
    if tx1 <= tx0 || ty1 <= ty0 {
        return proj;
    }
    let conic = [c / det, -b / det, a / det];
    let tiles = [tx0 as u32, ty0 as u32, tx1 as u32, ty1 as u32];
    if !(tiles[1]..tiles[3]).any(|ty| (tiles[0]..tiles[2]).any(|tx| tile_hit(mean, conic, reach, tx, ty))) {
        return proj;
    }
    proj.p = Projected { valid: true, mean, conic, depth: z, opacity: op, color: g.colors[i].map(|v| v as f64), radius, tiles, reach };
    proj.c2 = c2;
    proj.a = raw;
    proj.det_a = det_a;
    proj.op = op_raw;
    proj.sigma_c = sigma_c;
    proj.j = j;
    proj.clamp_x = ux != u;
    proj.clamp_y = uy != v;
    proj.u = u;
    proj.v = v;
    proj
}

/// How many standard deviations out a Gaussian of opacity `op` can still reach α = 1/255 (none past 3.33), so
/// faint Gaussians touch fewer tiles without changing a single pixel.
pub fn extent_sigmas(op: f64) -> f64 {
    let k = 2.0 * (op / MIN_ALPHA).max(1.0).ln();
    k.sqrt().min(3.33)
}

/// Mip-Splatting's 3D filter of each Gaussian (filter3d.wgsl): `scale / ν̂²`, ν̂ the highest focal / depth among the
/// cameras that hold its centre beyond the near plane in their view widened by 15 % past each edge; 0 where none does.
pub fn filter_3d(g: &Gaussians, cams: &[Camera], scale: f64) -> Vec<f64> {
    g.means
        .iter()
        .map(|m| {
            let m = m.map(|v| v as f64);
            let mut rate = 0.0f64;
            for c in cams {
                let pc = [0, 1, 2].map(|k| (0..3).map(|l| c.r[k][l] as f64 * m[l]).sum::<f64>() + c.t[k] as f64);
                if pc[2] <= NEAR {
                    continue;
                }
                let x = pc[0] / pc[2] * c.fx as f64 + c.cx as f64;
                let y = pc[1] / pc[2] * c.fy as f64 + c.cy as f64;
                let (w, h) = (c.width as f64, c.height as f64);
                if x >= -0.15 * w && x <= 1.15 * w && y >= -0.15 * h && y <= 1.15 * h {
                    rate = rate.max(c.fx.max(c.fy) as f64 / pc[2]);
                }
            }
            if rate > 0.0 {
                scale / (rate * rate)
            } else {
                0.0
            }
        })
        .collect()
}

pub fn project(g: &Gaussians, cam: &Camera) -> Vec<Projected> {
    (0..g.len()).map(|i| project_one(g, i, cam).p).collect()
}

/// Depth order: nearer first, index breaks ties (the GPU sorts the same key).
fn order(proj: &[Projected]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..proj.len()).filter(|&i| proj[i].valid).collect();
    idx.sort_by(|&a, &b| proj[a].depth.total_cmp(&proj[b].depth).then(a.cmp(&b)));
    idx
}

/// One pixel's contributions: (Gaussian, α, T before it, G, clamped) in blend order.
/// (Gaussian, α, T before it, G, clamped).
type Contribution = (usize, f64, f64, f64, bool);

fn contributions(proj: &[Projected], ord: &[usize], px: u32, py: u32, rules: Rules) -> (Vec<Contribution>, f64) {
    let (tx, ty) = (px / TILE, py / TILE);
    let mut t = 1.0;
    let mut out = Vec::new();
    for &i in ord {
        let p = &proj[i];
        if rules.cull_tiles && !(tx >= p.tiles[0] && tx < p.tiles[2] && ty >= p.tiles[1] && ty < p.tiles[3] && tile_hit(p.mean, p.conic, p.reach, tx, ty)) {
            continue;
        }
        let (dx, dy) = (px as f64 - p.mean[0], py as f64 - p.mean[1]);
        let power = -0.5 * (p.conic[0] * dx * dx + p.conic[2] * dy * dy) - p.conic[1] * dx * dy;
        if power > 0.0 {
            continue;
        }
        let gauss = power.exp();
        let raw = p.opacity * gauss;
        let alpha = raw.min(MAX_ALPHA);
        if alpha < rules.min_alpha {
            continue;
        }
        let next = t * (1.0 - alpha);
        if next < MIN_T {
            break;
        }
        out.push((i, alpha, t, gauss, raw > MAX_ALPHA));
        t = next;
    }
    (out, t)
}

pub fn render(g: &Gaussians, cam: &Camera, background: [f64; 3], rules: Rules) -> Image {
    let proj = project(g, cam);
    let ord = order(&proj);
    let (w, h) = (cam.width, cam.height);
    let mut img = Image { width: w, height: h, color: vec![0.0; (w * h * 3) as usize], depth: vec![0.0; (w * h) as usize], transmittance: vec![0.0; (w * h) as usize] };
    for py in 0..h {
        for px in 0..w {
            let (list, t) = contributions(&proj, &ord, px, py, rules);
            let k = (py * w + px) as usize;
            let mut c = [0.0; 3];
            let mut d = 0.0;
            for (i, alpha, ti, _, _) in list {
                for ch in 0..3 {
                    c[ch] += proj[i].color[ch] * alpha * ti;
                }
                d += proj[i].depth * alpha * ti;
            }
            for ch in 0..3 {
                img.color[k * 3 + ch] = c[ch] + t * background[ch];
            }
            img.depth[k] = d;
            img.transmittance[k] = t;
        }
    }
    img
}

/// Gradients of a loss given its derivatives with respect to the rendered colour (`d_color`, w·h·3) and depth
/// (`d_depth`, w·h).
pub fn backward(g: &Gaussians, cam: &Camera, background: [f64; 3], rules: Rules, d_color: &[f64], d_depth: &[f64]) -> Grads {
    let projs: Vec<Proj> = (0..g.len()).map(|i| project_one(g, i, cam)).collect();
    let proj: Vec<Projected> = projs.iter().map(|p| p.p).collect();
    let ord = order(&proj);
    let n = g.len();
    // Per Gaussian: dL/dmean2D, dL/dconic, dL/dopacity, dL/dcolor, dL/ddepth.
    let mut d_mean2 = vec![[0.0; 2]; n];
    let mut d_conic = vec![[0.0; 3]; n];
    let mut d_op = vec![0.0; n];
    let mut d_col = vec![[0.0; 3]; n];
    let mut d_z = vec![0.0; n];
    let (w, h) = (cam.width, cam.height);
    for py in 0..h {
        for px in 0..w {
            let k = (py * w + px) as usize;
            let (list, _) = contributions(&proj, &ord, px, py, rules);
            let dc = [d_color[k * 3], d_color[k * 3 + 1], d_color[k * 3 + 2]];
            let dd = d_depth[k];
            // What lies behind, normalised to the transmittance just behind the current Gaussian.
            let mut acc_c = background;
            let mut acc_d = 0.0;
            for &(i, alpha, ti, gauss, clamped) in list.iter().rev() {
                let p = &proj[i];
                for ch in 0..3 {
                    d_col[i][ch] += dc[ch] * alpha * ti;
                }
                d_z[i] += dd * alpha * ti;
                let mut d_alpha = dd * ti * (p.depth - acc_d);
                for ch in 0..3 {
                    d_alpha += dc[ch] * ti * (p.color[ch] - acc_c[ch]);
                }
                for ch in 0..3 {
                    acc_c[ch] = alpha * p.color[ch] + (1.0 - alpha) * acc_c[ch];
                }
                acc_d = alpha * p.depth + (1.0 - alpha) * acc_d;
                if clamped {
                    continue;
                }
                d_op[i] += d_alpha * gauss;
                let d_power = d_alpha * p.opacity * gauss;
                let (dx, dy) = (px as f64 - p.mean[0], py as f64 - p.mean[1]);
                d_mean2[i][0] += d_power * (p.conic[0] * dx + p.conic[1] * dy);
                d_mean2[i][1] += d_power * (p.conic[2] * dy + p.conic[1] * dx);
                d_conic[i][0] += d_power * (-0.5 * dx * dx);
                d_conic[i][1] += d_power * (-dx * dy);
                d_conic[i][2] += d_power * (-0.5 * dy * dy);
            }
        }
    }

    let mut out = Grads {
        means: vec![[0.0; 3]; n],
        log_scales: vec![[0.0; 3]; n],
        rotations: vec![[0.0; 4]; n],
        opacity_logits: vec![0.0; n],
        colors: d_col,
    };
    let (fx, fy) = (cam.fx as f64, cam.fy as f64);
    for i in 0..n {
        let pr = &projs[i];
        if !pr.p.valid {
            continue;
        }
        // The pixels saw op·c3·c2: d_op is that product's gradient.
        let op = pr.op;
        out.opacity_logits[i] = d_op[i] * pr.c3 * pr.c2 * op * (1.0 - op);
        let d_c2 = d_op[i] * op * pr.c3;
        let d_c3 = d_op[i] * op * pr.c2;
        let [x, y, z] = pr.pc;
        // Conic → 2D covariance: dL/dΣ = −K·(dL/dK)·K with the conic K = (A, B; B, C) (dL/dB counts B once, so the
        // off-diagonal of dL/dK is half of it), as project_backward.wgsl computes it: no 1/det².
        let [ka, kb, kc] = pr.p.conic;
        let [g0, g1, g2] = d_conic[i];
        let off = -(ka * kb * g0 + 0.5 * (kb * kb + ka * kc) * g1 + kb * kc * g2);
        // Symmetric gradient matrix of cov2D (b appears twice).
        let mut gm = [[-(ka * ka * g0 + ka * kb * g1 + kb * kb * g2), off], [off, -(kb * kb * g0 + kb * kc * g1 + kc * kc * g2)]];
        // The 2D filter's compensation c2 = √(det A / det(A + F)): ∂c2/∂A = ½·c2·(A⁻¹ − (A + F)⁻¹) = ½·c2·A⁻¹ F K (the
        // product spares the difference its cancellation); for a flat A (its determinant floored), only −½·c2·K.
        let h = 0.5 * d_c2 * pr.c2;
        let comp = if pr.det_a > FLAT * FILTER_2D * FILTER_2D {
            let [p, q, r] = [pr.a[2] / pr.det_a, -pr.a[1] / pr.det_a, pr.a[0] / pr.det_a];
            let f = FILTER_2D;
            [p * f * ka + q * f * kb, 0.5 * ((p * f * kb + q * f * kc) + (q * f * ka + r * f * kb)), q * f * kb + r * f * kc]
        } else {
            [-ka, -kb, -kc]
        };
        gm[0][0] += h * comp[0];
        gm[0][1] += h * comp[1];
        gm[1][0] += h * comp[1];
        gm[1][1] += h * comp[2];
        let j = pr.j;
        // dL/dΣc = Jᵀ G J.
        let mut d_sigma_c = [[0.0; 3]; 3];
        for r in 0..3 {
            for s in 0..3 {
                d_sigma_c[r][s] = (0..2).map(|u| (0..2).map(|v| j[u][r] * gm[u][v] * j[v][s]).sum::<f64>()).sum();
            }
        }
        // dL/dJ = 2 G J Σc.
        let mut d_j = [[0.0; 3]; 2];
        for u in 0..2 {
            for k in 0..3 {
                d_j[u][k] = 2.0 * (0..2).map(|v| gm[u][v] * (0..3).map(|l| j[v][l] * pr.sigma_c[l][k]).sum::<f64>()).sum::<f64>();
            }
        }
        // Camera-space position: from the 2D mean, the depth channel and the Jacobian.
        let mut d_pc = [0.0; 3];
        d_pc[0] += d_mean2[i][0] * fx / z;
        d_pc[2] += -d_mean2[i][0] * fx * x / (z * z);
        d_pc[1] += d_mean2[i][1] * fy / z;
        d_pc[2] += -d_mean2[i][1] * fy * y / (z * z);
        d_pc[2] += d_z[i];
        // J00 = fx/z, J02 = −fx·u/z, J11 = fy/z, J12 = −fy·v/z with u = x/z, v = y/z unless clamped.
        d_pc[2] += d_j[0][0] * (-fx / (z * z)) + d_j[1][1] * (-fy / (z * z));
        if pr.clamp_x {
            d_pc[2] += d_j[0][2] * (fx * pr.u / (z * z));
        } else {
            d_pc[0] += d_j[0][2] * (-fx / (z * z));
            d_pc[2] += d_j[0][2] * (2.0 * fx * x / (z * z * z));
        }
        if pr.clamp_y {
            d_pc[2] += d_j[1][2] * (fy * pr.v / (z * z));
        } else {
            d_pc[1] += d_j[1][2] * (-fy / (z * z));
            d_pc[2] += d_j[1][2] * (2.0 * fy * y / (z * z * z));
        }
        let wm = pr.w;
        out.means[i] = [0, 1, 2].map(|k| (0..3).map(|r| wm[r][k] * d_pc[r]).sum());
        // dL/dΣw = Wᵀ dL/dΣc W; M = Rq S' (the filtered scales); dL/dM = 2 dL/dΣw M.
        let d_sigma_w = mul(&mul(&transpose(&wm), &d_sigma_c), &wm);
        let mut ms = pr.rq;
        for row in &mut ms {
            for (k, v) in row.iter_mut().enumerate() {
                *v *= pr.sf[k];
            }
        }
        let d_m = mul(&d_sigma_w, &ms).map(|row| row.map(|v| 2.0 * v));
        let mut d_r = [[0.0; 3]; 3];
        let mut d_sf = [0.0; 3];
        for r in 0..3 {
            for k in 0..3 {
                d_r[r][k] = d_m[r][k] * pr.sf[k];
                d_sf[k] += pr.rq[r][k] * d_m[r][k];
            }
        }
        // s' = √(s² + σ²): ∂s'/∂log s = s²/s'; c3 = Π s/s' adds c3·σ²/s'² per axis.
        for k in 0..3 {
            out.log_scales[i][k] = d_sf[k] * pr.s[k] * pr.s[k] / pr.sf[k] + d_c3 * pr.c3 * pr.filter / (pr.sf[k] * pr.sf[k]);
        }
        let [qw, qx, qy, qz] = pr.qn;
        let dq_hat = [
            2.0 * (-qz * d_r[0][1] + qy * d_r[0][2] + qz * d_r[1][0] - qx * d_r[1][2] - qy * d_r[2][0] + qx * d_r[2][1]),
            2.0 * (qy * d_r[0][1] + qz * d_r[0][2] + qy * d_r[1][0] - 2.0 * qx * d_r[1][1] - qw * d_r[1][2] + qz * d_r[2][0] + qw * d_r[2][1] - 2.0 * qx * d_r[2][2]),
            2.0 * (-2.0 * qy * d_r[0][0] + qx * d_r[0][1] + qw * d_r[0][2] + qx * d_r[1][0] + qz * d_r[1][2] - qw * d_r[2][0] + qz * d_r[2][1] - 2.0 * qy * d_r[2][2]),
            2.0 * (-2.0 * qz * d_r[0][0] - qw * d_r[0][1] + qx * d_r[0][2] + qw * d_r[1][0] - 2.0 * qz * d_r[1][1] + qy * d_r[1][2] + qx * d_r[2][0] + qy * d_r[2][1]),
        ];
        let dot: f64 = (0..4).map(|k| pr.qn[k] * dq_hat[k]).sum();
        out.rotations[i] = [0, 1, 2, 3].map(|k| (dq_hat[k] - pr.qn[k] * dot) / pr.qlen);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A few Gaussians in front of a small camera, away from every threshold.
    fn scene() -> (Gaussians, Camera) {
        let pose = [0.96, 0.0, 0.28, 0.0, 0.0, 1.0, 0.0, 0.0, -0.28, 0.0, 0.96, 0.0];
        let cam = Camera::from_strip(&pose, &[1.2, 1.6, 0.0, 0.0], 40, 30, 1.0);
        let mut g = Gaussians::default();
        let mut rng = fastrand::Rng::with_seed(3);
        for k in 0..7 {
            let z = 2.0 + rng.f32() * 2.0;
            let p = cam.unproject(8.0 + rng.f32() * 24.0, 6.0 + rng.f32() * 18.0, z);
            g.push(
                p,
                [(-2.6 + rng.f32() * 0.8), (-2.6 + rng.f32() * 0.8), (-3.2 + rng.f32())],
                [0.8 + rng.f32() * 0.4, rng.f32() - 0.5, rng.f32() - 0.5, rng.f32() - 0.5],
                -0.6 + rng.f32() * 0.9 + 0.1 * k as f32,
                [rng.f32(), rng.f32(), rng.f32()],
            );
        }
        (g, cam)
    }

    /// Moves one parameter by the given amount.
    type Nudge = Box<dyn Fn(&mut Gaussians, f32)>;

    fn loss(img: &Image, wc: &[f64], wd: &[f64]) -> f64 {
        img.color.iter().zip(wc).map(|(a, b)| a * b).sum::<f64>() + img.depth.iter().zip(wd).map(|(a, b)| a * b).sum::<f64>()
    }

    #[test]
    fn gradients_match_finite_differences() {
        let (g, cam) = scene();
        gradients_match(&g, &cam);
    }

    /// With 3D filters about as wide as the Gaussians (their variance from a third to the whole of a typical axis's).
    #[test]
    fn filtered_gradients_match_finite_differences() {
        let (mut g, cam) = scene();
        let mut rng = fastrand::Rng::with_seed(5);
        for f in &mut g.filter_3d {
            *f = (0.04 + rng.f32() * 0.06).powi(2);
        }
        gradients_match(&g, &cam);
    }

    fn gradients_match(g: &Gaussians, cam: &Camera) {
        let (g, cam) = (g.clone(), *cam);
        let bg = [0.1, 0.2, 0.3];
        let mut rng = fastrand::Rng::with_seed(11);
        let n = (cam.width * cam.height) as usize;
        let wc: Vec<f64> = (0..n * 3).map(|_| rng.f64() - 0.5).collect();
        let wd: Vec<f64> = (0..n).map(|_| (rng.f64() - 0.5) * 0.2).collect();
        let grads = backward(&g, &cam, bg, Rules::SMOOTH, &wc, &wd);
        let eps = 1e-3f32;
        let mut checked = 0;
        let mut worst = 0.0f64;
        for i in 0..g.len() {
            let mut params: Vec<(String, f64, Nudge)> = Vec::new();
            for k in 0..3 {
                params.push((format!("mean[{i}][{k}]"), grads.means[i][k], Box::new(move |g: &mut Gaussians, d| g.means[i][k] += d)));
                params.push((format!("log_scale[{i}][{k}]"), grads.log_scales[i][k], Box::new(move |g: &mut Gaussians, d| g.log_scales[i][k] += d)));
                params.push((format!("color[{i}][{k}]"), grads.colors[i][k], Box::new(move |g: &mut Gaussians, d| g.colors[i][k] += d)));
            }
            for k in 0..4 {
                params.push((format!("rotation[{i}][{k}]"), grads.rotations[i][k], Box::new(move |g: &mut Gaussians, d| g.rotations[i][k] += d)));
            }
            params.push((format!("opacity[{i}]"), grads.opacity_logits[i], Box::new(move |g: &mut Gaussians, d| g.opacity_logits[i] += d)));
            for (name, analytic, nudge) in params {
                let mut plus = g.clone();
                nudge(&mut plus, eps);
                let mut minus = g.clone();
                nudge(&mut minus, -eps);
                let fd = (loss(&render(&plus, &cam, bg, Rules::SMOOTH), &wc, &wd) - loss(&render(&minus, &cam, bg, Rules::SMOOTH), &wc, &wd)) / (2.0 * eps as f64);
                let err = (fd - analytic).abs() / (fd.abs() + analytic.abs() + 1e-3);
                worst = worst.max(err);
                assert!(err < 0.02, "{name}: analytic {analytic:.6} vs finite difference {fd:.6}");
                checked += 1;
            }
        }
        assert!(checked > 80, "{checked}");
        eprintln!("worst relative error {worst:.2e} over {checked} parameters");
    }

    /// SSIM's partials where its numerator's factors cross zero (an anti-correlated window, B1 = 0; negative rendered
    /// colour, A1 = 0): finite, and what finite differences say; elsewhere what the quotient rule says.
    #[test]
    fn ssim_partials_are_finite_where_the_numerator_vanishes() {
        let fd = |f: &dyn Fn(f64) -> f64| (f(1e-7) - f(-1e-7)) / 2e-7;
        let (mx, my, exx, eyy) = (0.4, 0.6, 0.4 * 0.4 + 0.01, 0.6 * 0.6 + 0.02);
        for exy in [mx * my - SSIM_C2 / 2.0, mx * my - 0.003, mx * my + 0.004] {
            let p = ssim_partials(mx, my, exx, eyy, exy);
            let want = [fd(&|e| ssim(mx + e, my, exx, eyy, exy)), fd(&|e| ssim(mx, my, exx + e, eyy, exy)), fd(&|e| ssim(mx, my, exx, eyy, exy + e))];
            for k in 0..3 {
                assert!(p[k].is_finite() && (p[k] - want[k]).abs() < 1e-5 * want[k].abs().max(1.0), "E[xy] {exy}: {p:?} vs {want:?}");
            }
        }
        // A1 = 0: μx·μy = −C1/2 (a render that went negative).
        let (mx, my) = (-SSIM_C1 / 2.0 / 0.5, 0.5);
        let p = ssim_partials(mx, my, mx * mx + 0.01, my * my + 0.01, mx * my + 0.002);
        assert!(p.iter().all(|v| v.is_finite()), "{p:?}");
        let s = ssim(0.3, 0.7, 0.1, 0.5, 0.25);
        let (a, b) = (2.0 * 0.3 * 0.7 + SSIM_C1, 2.0 * (0.25 - 0.21) + SSIM_C2);
        let (c, d) = (0.09 + 0.49 + SSIM_C1, (0.1 - 0.09) + (0.5 - 0.49) + SSIM_C2);
        let quotient = s * (2.0 * 0.7 / a - 2.0 * 0.7 / b - 2.0 * 0.3 / c + 2.0 * 0.3 / d);
        assert!((ssim_partials(0.3, 0.7, 0.1, 0.5, 0.25)[0] - quotient).abs() < 1e-9);
    }

    /// Every tile with a pixel centre the Gaussian reaches passes tile_hit (it never drops a pixel a splat draws).
    #[test]
    fn tile_hit_misses_no_pixel() {
        let mut rng = fastrand::Rng::with_seed(17);
        for _ in 0..300 {
            let m = [rng.f64() * 64.0 - 8.0, rng.f64() * 64.0 - 8.0];
            let (a, c) = (0.5 + rng.f64() * 400.0, 0.5 + rng.f64() * 400.0);
            let b = (rng.f64() * 2.0 - 1.0) * 0.95 * (a * c).sqrt();
            let det = a * c - b * b;
            let conic = [c / det, -b / det, a / det];
            let tau = 1.0 + rng.f64() * 10.0;
            for (tx, ty) in [(0u32, 0u32), (1, 0), (0, 1), (2, 2), (3, 1)] {
                let reaches = (0..16).any(|y| (0..16).any(|x| {
                    let (dx, dy) = ((tx * TILE + x) as f64 - m[0], (ty * TILE + y) as f64 - m[1]);
                    conic[0] * dx * dx + 2.0 * conic[1] * dx * dy + conic[2] * dy * dy <= tau
                }));
                assert!(!reaches || tile_hit(m, conic, tau, tx, ty), "{m:?} {conic:?} {tau} tile ({tx}, {ty})");
            }
        }
    }

    /// Fusing the 3D filter into the scales and opacity renders the same image.
    #[test]
    fn a_fused_filter_renders_what_the_filter_did() {
        let (mut g, cam) = scene();
        for (k, f) in g.filter_3d.iter_mut().enumerate() {
            *f = (0.03 + 0.01 * k as f32).powi(2);
        }
        let mut fused = g.clone();
        fused.fuse_filter();
        assert!(fused.filter_3d.iter().all(|f| *f == 0.0));
        let (a, b) = (render(&g, &cam, [0.1; 3], Rules::SMOOTH), render(&fused, &cam, [0.1; 3], Rules::SMOOTH));
        let worst = a.color.iter().zip(&b.color).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max);
        assert!(worst < 1e-5, "{worst}");
        // The filter widens and fades: the fused Gaussians are larger and fainter.
        assert!((0..g.len()).all(|i| fused.log_scales[i].iter().zip(&g.log_scales[i]).all(|(f, s)| f > s) && fused.opacity_logits[i] < g.opacity_logits[i]));
    }

    /// A splat smaller than a pixel keeps its weight: its opacity falls with its area instead of the 2D filter
    /// spreading it at full opacity (3DGS's dilation rendered 0.1 and 0.2 px splats alike).
    #[test]
    fn a_subpixel_splat_keeps_its_weight() {
        let cam = Camera::from_strip(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], &[1.0, 1.0, 0.0, 0.0], 32, 32, 1.0);
        // The weight a splat of `px` pixels (σ) leaves in the image, over 8 × 8 positions within a pixel: the integral
        // of its footprint, which pixel centres alone sample unevenly.
        let weight = |px: f32| {
            let mut sum = 0.0;
            for k in 0..64 {
                let mut g = Gaussians::default();
                let at = cam.unproject(15.0 + (k % 8) as f32 / 8.0, 15.0 + (k / 8) as f32 / 8.0, 4.0);
                g.push(at, [(px * 4.0 / cam.fx).ln(); 3], [1.0, 0.0, 0.0, 0.0], crate::gaussians::logit(0.99), [1.0; 3]);
                sum += render(&g, &cam, [0.0; 3], Rules::SMOOTH).color.iter().step_by(3).sum::<f64>();
            }
            sum / 64.0
        };
        // Twice as wide, four times the area (less what the 1/255 cut-off leaves out of each).
        let (big, small) = (weight(0.2), weight(0.1));
        assert!((big / small - 4.0).abs() < 0.25, "{big} {small}");
    }

    #[test]
    fn an_opaque_surface_renders_its_colour_and_depth() {
        // A wall of overlapping opaque discs at 3 m facing the camera.
        let cam = Camera::from_strip(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], &[1.0, 1.333, 0.0, 0.0], 32, 24, 1.0);
        let mut g = Gaussians::default();
        for gy in -12..=12 {
            for gx in -16..=16 {
                g.push([gx as f32 * 0.05, gy as f32 * 0.05, 3.0], [(0.06f32).ln(), (0.06f32).ln(), (0.005f32).ln()], [1.0, 0.0, 0.0, 0.0], 4.0, [0.8, 0.4, 0.2]);
            }
        }
        let img = render(&g, &cam, [0.0; 3], Rules::GPU);
        let k = (12 * 32 + 16) as usize;
        assert!((img.color[k * 3] - 0.8).abs() < 0.01, "{}", img.color[k * 3]);
        assert!((img.depth[k] - 3.0).abs() < 0.03, "{}", img.depth[k]);
        assert!(img.transmittance[k] < 1e-3);
    }
}
