// A training step's projection backward fused with dense Adam (after common.wgsl, a sums_read_*.wgsl variant and
// param_grads.wgsl): every Gaussian takes its Adam step here, one the step shows with its parameter gradients, one it
// does not with none (its moments decay and its momentum moves it, as dense Adam does), so the gradients never cross
// memory: 0.83 ms against 1.09 for the projection backward and Adam apart on VRChat Home's 823k splats. One rate a slot
// of the 16-float layout (padding slots 0: the sky flag, and slot 15, the step its moments last restarted at, from which
// its bias correction counts).

struct AdamInfo {
    lr: array<vec4<f32>, 4>,
    // β1, β2, ε, the SGLD noise's scale (crate::train::Settings::noise × the position learning rate; 0: none).
    beta: vec4<f32>,
    // Floats in the set, this step (bias correction), 1 to spare the Gaussians anchored on measured depth the noise,
    // the noise's seed.
    n: vec4<u32>,
}

// A Gaussian sits on the measured surface where depth steps see it: α·T-weighted, its centre within this share of the
// measured depth (crate::train::ANCHORED, stats[2i + 1] of param_grads.wgsl).
const ANCHORED: f32 = 0.02;

fn hash(x: u32) -> u32 {
    // PCG's output permutation of an LCG step.
    let state = x * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// Three standard normals for Gaussian i at this step (Box–Muller on four hashed uniforms).
fn normals(i: u32) -> vec3<f32> {
    var h = hash(i ^ hash(adam.n.y ^ hash(adam.n.w)));
    var u: vec4<f32>;
    for (var k = 0u; k < 4u; k++) {
        h = hash(h);
        u[k] = (f32(h >> 8u) + 0.5) / 16777216.0;
    }
    let r0 = sqrt(-2.0 * log(u.x));
    let r1 = sqrt(-2.0 * log(u.z));
    return vec3<f32>(r0 * cos(6.283185307 * u.y), r0 * sin(6.283185307 * u.y), r1 * cos(6.283185307 * u.w));
}

fn rotate_by(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    let u = q.yzw;
    return v + 2.0 * cross(u, cross(u, v) + q.x * v);
}

// 3DGS-MCMC's exploration term (Kheradmand et al. 2024, eq. 7 as gsplat implements it): Σ·η × the scale × a gate that
// opens only for nearly transparent Gaussians, σ(100·((1 − opacity) − 0.995)): one at opacity 0.005 has half the
// noise, one at 0.05 a hundredth, an opaque one none.
fn sgld(i: u32, log_scale: vec3<f32>, q: vec4<f32>, opacity_logit: f32) -> vec3<f32> {
    let op = sigmoid(opacity_logit);
    let gate = 1.0 / (1.0 + exp(-100.0 * ((1.0 - op) - 0.995)));
    if (gate < 1e-6) {
        return vec3<f32>(0.0);
    }
    if (adam.n.z != 0u) {
        let sd = stats[2u * i + 1u];
        if (sd.y > 0.0 && sd.x < ANCHORED * sd.y) {
            return vec3<f32>(0.0);
        }
    }
    let qn = q / max(length(q), 1e-12);
    let s2 = exp(2.0 * log_scale);
    // Σ η = R S² Rᵀ η.
    let local = rotate_by(vec4<f32>(qn.x, -qn.yzw), normals(i));
    return rotate_by(qn, s2 * local) * gate * adam.beta.w;
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read_write> params4: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> proj: array<f32>;
@group(0) @binding(4) var<storage, read> filter3d: array<f32>;
@group(0) @binding(5) var<storage, read_write> stats: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> m1: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> m2: array<vec4<f32>>;
@group(0) @binding(8) var<uniform> adam: AdamInfo;

struct Slots {
    p: vec4<f32>,
    m: vec4<f32>,
    v: vec4<f32>,
}

// Adam on four slots: moments m and v, gradient g, rates lr, bias corrections c1 and c2. A gradient that is no number
// (from the bits: WGSL may assume there are none) leaves its slot as it was.
fn adam4(p: vec4<f32>, m: vec4<f32>, v: vec4<f32>, g: vec4<f32>, lr: vec4<f32>, c1: f32, c2: f32) -> Slots {
    let b1 = adam.beta.x;
    let b2 = adam.beta.y;
    let ok = (bitcast<vec4<u32>>(g) & vec4<u32>(0x7f800000u)) != vec4<u32>(0x7f800000u);
    let m_new = b1 * m + (1.0 - b1) * g;
    let v_new = b2 * v + (1.0 - b2) * g * g;
    let p_new = p - lr * (m_new / c1) / (sqrt(v_new / c2) + adam.beta.z);
    var out: Slots;
    out.p = select(p, p_new, ok);
    out.m = select(m, m_new, ok);
    out.v = select(v, v_new, ok);
    return out;
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= frame.counts.x) {
        return;
    }
    let o = 4u * i;
    let p0 = params4[o];
    let p1 = params4[o + 1u];
    let p2 = params4[o + 2u];
    let p3 = params4[o + 3u];
    // Gradients in the parameters' layout (slots 14 and 15 have no rate and move nothing).
    var g0 = vec4<f32>(0.0);
    var g1 = vec4<f32>(0.0);
    var g2 = vec4<f32>(0.0);
    var g3 = vec4<f32>(0.0);
    if (proj[i * PROJ + 15u] != 0.0) {
        let g = param_grads(i, p0.xyz, vec3<f32>(p0.w, p1.x, p1.y), vec4<f32>(p1.z, p1.w, p2.x, p2.y), p2.z, filter3d[i]);
        g0 = vec4<f32>(g.mean, g.scale.x);
        g1 = vec4<f32>(g.scale.y, g.scale.z, g.rotation.x, g.rotation.y);
        g2 = vec4<f32>(g.rotation.z, g.rotation.w, g.opacity, g.color.x);
        g3 = vec4<f32>(g.color.y, g.color.z, 0.0, 0.0);
    } else if (frame.counts.w != 0u) {
        // The regularisers on every Gaussian (crate::train::Settings::regularise_unseen), as project_backward.wgsl.
        let op = sigmoid(p2.z);
        let s = exp(vec3<f32>(p0.w, p1.x, p1.y));
        g0.w = frame.reg.y * s.x;
        g1.x = frame.reg.y * s.y;
        g1.y = frame.reg.y * s.z;
        g2.z = frame.reg.x * op * (1.0 - op);
    }
    let te = max(f32(adam.n.y) - p3.w, 1.0);
    let c1 = 1.0 - pow(adam.beta.x, te);
    let c2 = 1.0 - pow(adam.beta.y, te);
    var s0 = adam4(p0, m1[o], m2[o], g0, adam.lr[0], c1, c2);
    // The sky shell (its flag in slot 14) stays where it is.
    if (adam.beta.w > 0.0 && p3.z < 0.5) {
        s0.p = vec4<f32>(s0.p.xyz + sgld(i, vec3<f32>(p0.w, p1.x, p1.y), vec4<f32>(p1.z, p1.w, p2.x, p2.y), p2.z), s0.p.w);
    }
    let s1 = adam4(p1, m1[o + 1u], m2[o + 1u], g1, adam.lr[1], c1, c2);
    let s2 = adam4(p2, m1[o + 2u], m2[o + 2u], g2, adam.lr[2], c1, c2);
    let s3 = adam4(p3, m1[o + 3u], m2[o + 3u], g3, adam.lr[3], c1, c2);
    params4[o] = s0.p;
    params4[o + 1u] = s1.p;
    params4[o + 2u] = s2.p;
    // The sky flag and the restart step are no parameters: kept as they were.
    params4[o + 3u] = vec4<f32>(s3.p.xy, p3.zw);
    m1[o] = s0.m;
    m1[o + 1u] = s1.m;
    m1[o + 2u] = s2.m;
    m1[o + 3u] = vec4<f32>(s3.m.xy, 0.0, 0.0);
    m2[o] = s0.v;
    m2[o + 1u] = s1.v;
    m2[o + 2u] = s2.v;
    m2[o + 3u] = vec4<f32>(s3.v.xy, 0.0, 0.0);
}
