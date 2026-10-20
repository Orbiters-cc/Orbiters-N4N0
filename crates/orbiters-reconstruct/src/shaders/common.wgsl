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
    // Background colour; w: dropout rate of this colour step (DropGaussian, 0: none).
    background: vec4<f32>,
    // Width, height, tiles across, tiles down.
    size: vec4<u32>,
    // Gaussians, intersections, depth bits of a sort key, dropout seed.
    counts: vec4<u32>,
    // Loss weights: colour L1, depth, unused, scene pixels or depth samples in this view.
    loss: vec4<f32>,
    // Regulariser weights per Gaussian: opacity, scale (per metre); screen-space dilation (pixels²) along x and y.
    reg: vec4<f32>,
}

const TILE: u32 = 16u;
const DILATION: f32 = 0.3;
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

fn sigmoid(x: f32) -> f32 {
    return 1.0 / (1.0 + exp(-x));
}

// Uniform in [0, 1) from a 32-bit key (PCG hash).
fn unit_hash(x: u32) -> f32 {
    var v = x * 747796405u + 2891336453u;
    v = ((v >> ((v >> 28u) + 4u)) ^ v) * 277803737u;
    v = (v >> 22u) ^ v;
    return f32(v >> 8u) / 16777216.0;
}
