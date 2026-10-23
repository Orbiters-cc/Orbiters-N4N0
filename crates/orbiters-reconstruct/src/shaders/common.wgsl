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
    // Gaussians, 1 for a viewer's guards (project.wgsl), depth bits of a sort key, 1 to regularise the Gaussians a
    // training step does not show too.
    counts: vec4<u32>,
    // Loss weights: colour L1, depth, unused, scene pixels or depth samples in this view.
    loss: vec4<f32>,
    // Regulariser weights per Gaussian: opacity, scale (per metre); the 2D filter's variance in pixels² along x and y
    // (cpu::FILTER_2D at the training resolution; renders on the depth-sample grid scale it to keep the same
    // footprint).
    reg: vec4<f32>,
    // A viewer's confidence control (crate::gpu::Confidence): amount (0 to 1), mode (0 fade, 1 blur, 2 haze), unused.
    confidence: vec4<f32>,
    // A viewer's far field (crate::gpu::FarField): mode (0 shown, 1 fog, 2 crop), distance (metres), unused; the
    // horizon's colour (rgb).
    far: vec4<f32>,
    horizon: vec4<f32>,
}

// Tiles of TILE × TILE pixels: 16 for colour steps and viewers; depth steps render their sample grid on 8×8 tiles
// (crate::gpu: those modules replace this line), four times the workgroups and a quarter of the walk per pixel.
const TILE: u32 = 16u;
const TILE_PIXELS: u32 = TILE * TILE;
const MAX_ALPHA: f32 = 0.99;
const MIN_ALPHA: f32 = 0.003921569;
const MIN_T: f32 = 0.0001;
// Below this share of the 2D filter's own determinant a projected covariance counts as flat (cpu::FLAT).
const FLAT: f32 = 1e-6;
// A viewer's guards (crate::gpu::Gpu::guards): splats fade in from the near plane to this distance (metres), and none
// reaches further on screen than this many pixels (nor than the image's shorter side).
const GUARD_FADE: f32 = 0.2;
const GUARD_RADIUS: f32 = 1024.0;
// The confidence control's blur at no trust at all: this much variance (pixels²) more along each axis.
const CONFIDENCE_BLUR: f32 = 16.0;

// How far a viewer can trust a splat (crate::gaussians::Seen: largest weight, keyframes, sector bits, finest rate),
// from 1 down to 0, seen from the eye `toward_eye` away (world, from the splat): RadSplat's importance thresholds on its
// weight; half as much seen in one keyframe; 0.2 less for each compass sector between the eye's and the nearest one a
// keyframe looked from (a surface seen from one side smears seen from another, not from the same; at most three); and
// none once the eye samples it four times as finely as the nearest keyframe did (magnified beyond what was observed).
fn trust(s: vec4<f32>, toward_eye: vec3<f32>) -> f32 {
    let strength = smoothstep(0.001, 0.01, s.x);
    let views = mix(0.5, 1.0, clamp((s.y - 1.0) / 4.0, 0.0, 1.0));
    let eye = min(u32(fract(atan2(toward_eye.x, toward_eye.z) / 6.283185307 + 1.0) * 16.0), 15u);
    let bits = u32(s.z);
    var nearest = 3u;
    for (var k = 0u; k < 16u; k++) {
        if ((bits & (1u << k)) != 0u) {
            let d = max(eye, k) - min(eye, k);
            nearest = min(nearest, min(d, 16u - d));
        }
    }
    let side = 1.0 - 0.2 * f32(nearest);
    let finer = max(frame.focal.x, frame.focal.y) / max(length(toward_eye), 1e-6) / max(s.w, 1e-6);
    return strength * views * side * (1.0 - smoothstep(1.0, 4.0, finer));
}
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
    // Scales, and widened by the 3D filter (its variance added to their squares); its compensation c3 = Π s / s'.
    s: vec3<f32>,
    sf: vec3<f32>,
    c3: f32,
    sigma_c: array<vec3<f32>, 3>,
    j0: vec3<f32>,
    j1: vec3<f32>,
    // The projected covariance A = J Σc Jᵀ and its determinant, A with the 2D filter (cov), and the 2D filter's
    // compensation c2 = √(det A / det cov) (det A floored at flat_det()).
    a: vec3<f32>,
    det_a: f32,
    cov: vec3<f32>,
    c2: f32,
    clamp_x: bool,
    clamp_y: bool,
    u: f32,
    v: f32,
}

fn column(rows: array<vec3<f32>, 3>, k: u32) -> vec3<f32> {
    return vec3<f32>(rows[0][k], rows[1][k], rows[2][k]);
}

// The determinant under which the 2D filter's compensation counts a projected covariance as flat.
fn flat_det() -> f32 {
    return FLAT * frame.reg.z * frame.reg.w;
}

// Projection of a Gaussian (mean, log scales, rotation, 3D filter variance) with the frame's camera.
fn proj_math(mean: vec3<f32>, log_scale: vec3<f32>, q: vec4<f32>, filter_var: f32) -> ProjMath {
    var m: ProjMath;
    let w = array<vec3<f32>, 3>(frame.r0.xyz, frame.r1.xyz, frame.r2.xyz);
    m.pc = vec3<f32>(dot(w[0], mean) + frame.r0.w, dot(w[1], mean) + frame.r1.w, dot(w[2], mean) + frame.r2.w);
    m.qlen = max(length(q), 1e-12);
    m.qn = q / m.qlen;
    m.rq = quat_rows(m.qn);
    m.s = exp(log_scale);
    m.sf = sqrt(m.s * m.s + vec3<f32>(filter_var));
    m.c3 = (m.s.x / m.sf.x) * (m.s.y / m.sf.y) * (m.s.z / m.sf.z);
    m.ok = false;
    if (m.pc.z < frame.limits.z) {
        return m;
    }
    // M = Rq S' (columns scaled by the filtered scales); Σw = M Mᵀ; Σc = W Σw Wᵀ.
    let mr = array<vec3<f32>, 3>(m.rq[0] * m.sf, m.rq[1] * m.sf, m.rq[2] * m.sf);
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
    m.a = vec3<f32>(dot(js0, m.j0), dot(js0, m.j1), dot(js1, m.j1));
    m.cov = m.a + vec3<f32>(frame.reg.z, 0.0, frame.reg.w);
    let det = m.cov.x * m.cov.z - m.cov.y * m.cov.y;
    m.det_a = m.a.x * m.a.z - m.a.y * m.a.y;
    m.ok = det > 0.0;
    if (m.ok) {
        m.c2 = sqrt(max(m.det_a, flat_det()) / det);
    }
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
