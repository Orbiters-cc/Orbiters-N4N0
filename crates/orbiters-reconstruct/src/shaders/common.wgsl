// Shared definitions of the splatting kernels. Mirrors src/cpu.rs (the f64 reference): same projection, culling,
// thresholds and blending order. Prepended to every kernel file.

struct Frame {
    // World-to-camera rows (xyz) and translation (w).
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    // fx, fy, cx, cy.
    focal: vec4<f32>,
    // Frustum clamp limits on x/z and y/z, near plane, far plane (depth keys).
    limits: vec4<f32>,
    background: vec4<f32>,
    // Width, height, tiles across, tiles down.
    size: vec4<u32>,
    // Gaussians, unused, depth bits of a sort key, 1 to regularise the Gaussians a training step does not show too.
    counts: vec4<u32>,
    // Loss weights: colour L1, depth, unused, scene pixels or depth samples in this view.
    loss: vec4<f32>,
    // Regulariser weights per Gaussian: opacity, scale (per metre); screen-space dilation in pixels² along x and y
    // (0.3 at the training resolution; renders on the depth-sample grid scale it to keep the same footprint).
    reg: vec4<f32>,
}

const TILE: u32 = 16u;
const MAX_ALPHA: f32 = 0.99;
const MIN_ALPHA: f32 = 0.003921569;
const MIN_T: f32 = 0.0001;
// Floats per Gaussian: parameters (and their gradients and Adam moments), projected values, 2D gradients.
const PARAM: u32 = 16u;
const PROJ: u32 = 16u;
const GRAD2D: u32 = 10u;

// Linear invocation index of a (possibly two-dimensional) dispatch of 256-wide workgroups.
fn flat_index(gid: vec3<u32>, nwg: vec3<u32>) -> u32 {
    return gid.x + gid.y * nwg.x * 256u;
}

fn quat_rows(qn: vec4<f32>) -> array<vec3<f32>, 3> {
    let w = qn.x;
    let x = qn.y;
    let y = qn.z;
    let z = qn.w;
    return array<vec3<f32>, 3>(
        vec3<f32>(1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)),
        vec3<f32>(2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)),
        vec3<f32>(2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)),
    );
}

// Everything the projection computes, kept for its backward pass.
struct ProjMath {
    ok: bool,
    pc: vec3<f32>,
    qn: vec4<f32>,
    qlen: f32,
    rq: array<vec3<f32>, 3>,
    s: vec3<f32>,
    sigma_c: array<vec3<f32>, 3>,
    j0: vec3<f32>,
    j1: vec3<f32>,
    cov: vec3<f32>,
    clamp_x: bool,
    clamp_y: bool,
    u: f32,
    v: f32,
}

fn column(rows: array<vec3<f32>, 3>, k: u32) -> vec3<f32> {
    return vec3<f32>(rows[0][k], rows[1][k], rows[2][k]);
}

// Projection of Gaussian parameters `p` (mean, log scales, rotation) with the frame's camera.
fn proj_math(mean: vec3<f32>, log_scale: vec3<f32>, q: vec4<f32>) -> ProjMath {
    var m: ProjMath;
    let w = array<vec3<f32>, 3>(frame.r0.xyz, frame.r1.xyz, frame.r2.xyz);
    m.pc = vec3<f32>(dot(w[0], mean) + frame.r0.w, dot(w[1], mean) + frame.r1.w, dot(w[2], mean) + frame.r2.w);
    m.qlen = max(length(q), 1e-12);
    m.qn = q / m.qlen;
    m.rq = quat_rows(m.qn);
    m.s = exp(log_scale);
    m.ok = false;
    if (m.pc.z < frame.limits.z) {
        return m;
    }
    // M = Rq S (columns scaled); Σw = M Mᵀ; Σc = W Σw Wᵀ.
    let mr = array<vec3<f32>, 3>(m.rq[0] * m.s, m.rq[1] * m.s, m.rq[2] * m.s);
    var sw: array<vec3<f32>, 3>;
    for (var r = 0u; r < 3u; r++) {
        sw[r] = vec3<f32>(dot(mr[r], mr[0]), dot(mr[r], mr[1]), dot(mr[r], mr[2]));
    }
    // Σc[r][c] = W_r · (Σw W_c).
    for (var r = 0u; r < 3u; r++) {
        var row: vec3<f32>;
        for (var c = 0u; c < 3u; c++) {
            let swc = vec3<f32>(dot(sw[0], w[c]), dot(sw[1], w[c]), dot(sw[2], w[c]));
            row[c] = dot(w[r], swc);
        }
        m.sigma_c[r] = row;
    }
    let fx = frame.focal.x;
    let fy = frame.focal.y;
    let z = m.pc.z;
    let ux = m.pc.x / z;
    let uy = m.pc.y / z;
    m.u = clamp(ux, -frame.limits.x, frame.limits.x);
    m.v = clamp(uy, -frame.limits.y, frame.limits.y);
    m.clamp_x = ux != m.u;
    m.clamp_y = uy != m.v;
    m.j0 = vec3<f32>(fx / z, 0.0, -fx * m.u / z);
    m.j1 = vec3<f32>(0.0, fy / z, -fy * m.v / z);
    // J Σc rows (Σc is symmetric: its columns are its rows).
    let js0 = vec3<f32>(dot(m.j0, m.sigma_c[0]), dot(m.j0, m.sigma_c[1]), dot(m.j0, m.sigma_c[2]));
    let js1 = vec3<f32>(dot(m.j1, m.sigma_c[0]), dot(m.j1, m.sigma_c[1]), dot(m.j1, m.sigma_c[2]));
    m.cov = vec3<f32>(dot(js0, m.j0) + frame.reg.z, dot(js0, m.j1), dot(js1, m.j1) + frame.reg.w);
    let det = m.cov.x * m.cov.z - m.cov.y * m.cov.y;
    m.ok = det > 0.0;
    return m;
}

// Largest quadratic form q = dᵀ·conic·d at which a Gaussian of opacity `op` still reaches α = 1/255 (capped at
// 3.33 σ, like cpu::extent_sigmas); 0 when it never does.
fn alpha_reach(op: f32) -> f32 {
    let s = min(sqrt(2.0 * log(max(op / MIN_ALPHA, 1.0))), 3.33);
    return s * s;
}

// Whether a Gaussian centred at `m` with conic (A, B, C) (q = A·dx² + 2B·dx·dy + C·dy²) reaches q ≤ tau at any pixel
// centre of tile (tx, ty): the minimum of q over the tile's rectangle of pixel centres is at the centre when it lies
// inside, otherwise on an edge, where q is a one-dimensional quadratic minimised in closed form.
fn tile_hit(m: vec2<f32>, conic: vec3<f32>, tau: f32, tx: u32, ty: u32) -> bool {
    let lo = vec2<f32>(f32(tx * TILE), f32(ty * TILE)) - m;
    let hi = lo + vec2<f32>(f32(TILE - 1u));
    if (lo.x <= 0.0 && hi.x >= 0.0 && lo.y <= 0.0 && hi.y >= 0.0) {
        return true;
    }
    var best = 3.4e38;
    for (var k = 0u; k < 2u; k++) {
        let dx = select(lo.x, hi.x, k == 1u);
        let dy = clamp(-conic.y * dx / conic.z, lo.y, hi.y);
        best = min(best, conic.x * dx * dx + 2.0 * conic.y * dx * dy + conic.z * dy * dy);
        let ey = select(lo.y, hi.y, k == 1u);
        let ex = clamp(-conic.y * ey / conic.x, lo.x, hi.x);
        best = min(best, conic.x * ex * ex + 2.0 * conic.y * ex * ey + conic.z * ey * ey);
    }
    return best <= tau;
}

fn sigmoid(x: f32) -> f32 {
    return 1.0 / (1.0 + exp(-x));
}
