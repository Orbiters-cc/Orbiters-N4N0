// A training step's projection backward fused with dense Adam (after common.wgsl, a sums_read_*.wgsl variant and
// param_grads.wgsl): every Gaussian takes its Adam step here, one the step shows with its parameter gradients, one it
// does not with none (its moments decay and its momentum moves it, as dense Adam does), so the gradients never cross
// memory: 0.83 ms against 1.09 for the projection backward and Adam apart on VRChat Home's 823k splats. One rate a slot
// of the 16-float layout (padding slots 0: the sky flag, and slot 15, the step its moments last restarted at, from which
// its bias correction counts).

struct AdamInfo {
    lr: array<vec4<f32>, 4>,
    // β1, β2, ε, unused.
    beta: vec4<f32>,
    // Floats in the set, this step (bias correction), 1: colours stay within 0–1 (crate::gpu::Gpu::clamp_colors),
    // unused.
    n: vec4<u32>,
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
    let s0 = adam4(p0, m1[o], m2[o], g0, adam.lr[0], c1, c2);
    let s1 = adam4(p1, m1[o + 1u], m2[o + 1u], g1, adam.lr[1], c1, c2);
    let s2 = adam4(p2, m1[o + 2u], m2[o + 2u], g2, adam.lr[2], c1, c2);
    let s3 = adam4(p3, m1[o + 3u], m2[o + 3u], g3, adam.lr[3], c1, c2);
    var q2 = s2.p;
    var q3 = s3.p;
    if (adam.n.z != 0u) {
        q2.w = clamp(q2.w, 0.0, 1.0);
        q3 = vec4<f32>(clamp(q3.xy, vec2<f32>(0.0), vec2<f32>(1.0)), q3.zw);
    }
    params4[o] = s0.p;
    params4[o + 1u] = s1.p;
    params4[o + 2u] = q2;
    // The sky flag and the restart step are no parameters: kept as they were.
    params4[o + 3u] = vec4<f32>(q3.xy, p3.zw);
    m1[o] = s0.m;
    m1[o + 1u] = s1.m;
    m1[o + 2u] = s2.m;
    m1[o + 3u] = vec4<f32>(s3.m.xy, 0.0, 0.0);
    m2[o] = s0.v;
    m2[o + 1u] = s1.v;
    m2[o + 2u] = s2.v;
    m2[o + 3u] = vec4<f32>(s3.v.xy, 0.0, 0.0);
}
