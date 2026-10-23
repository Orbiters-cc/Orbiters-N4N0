// Relocation on the device (crate::train::plan_relocation decides on the CPU from what `weights` reads back):
//   weights: per Gaussian, −1 if dead (faded, broken or huge on screen), −2 − opacity if alive on the sky shell
//            (its flag in a padding slot), else its opacity (the sampling weight);
//   adjust:  sources get their copies' opacity and footprint (plan entries: index, opacity logit, log-scale delta);
//   copy:    copies of the adjusted sources into dead slots or past the end, inside the source's footprint (entries:
//            target, source, three standard normal offsets).
// Adam moments of every Gaussian touched restart at zero, and its bias correction counts from `info.step` (RESTART).

struct Info {
    n: u32,
    count: u32,
    // Largest screen radius alive, as a share of a view's width (project_backward.wgsl's statistic).
    max_radius: f32,
    // The step a restarted Gaussian's bias correction counts from (adam.wgsl): this round's with the `adam_restart`
    // setting, else 0, the run's first, as 3DGS-MCMC counts it.
    step: f32,
}

@group(0) @binding(0) var<uniform> info: Info;
@group(0) @binding(1) var<storage, read_write> params: array<f32>;
@group(0) @binding(2) var<storage, read> stats: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> weights: array<f32>;
@group(0) @binding(4) var<storage, read> plan: array<u32>;
@group(0) @binding(5) var<storage, read_write> m1: array<f32>;
@group(0) @binding(6) var<storage, read_write> m2: array<f32>;

const DEAD: f32 = 0.005;
const PARAM: u32 = 16u;
// Padding slots of the parameters no learning rate moves (crate::gaussians): the sky flag, the restart step.
const SKY: u32 = 14u;
const RESTART: u32 = 15u;

// The same as common.wgsl's (which needs the frame uniform).
fn flat_index(gid: vec3<u32>, nwg: vec3<u32>) -> u32 {
    return gid.x + gid.y * nwg.x * 256u;
}

fn rotate(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    let n = q / max(length(q), 1e-12);
    let u = n.yzw;
    return v + 2.0 * cross(u, cross(u, v) + n.x * v);
}

// Infinite or NaN, from the bits (WGSL may assume neither exists in arithmetic).
fn broken(v: f32) -> bool {
    return (bitcast<u32>(v) & 0x7f800000u) == 0x7f800000u;
}

fn restart(i: u32) {
    for (var k = 0u; k < PARAM; k++) {
        m1[i * PARAM + k] = 0.0;
        m2[i * PARAM + k] = 0.0;
    }
    params[i * PARAM + RESTART] = info.step;
}

@compute @workgroup_size(256)
fn classify(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= info.n) {
        return;
    }
    let o = i * PARAM;
    // Any parameter that is no number (a NaN rotation or colour renders nowhere and holds a slot).
    var bad = false;
    for (var k = 0u; k < SKY; k++) {
        bad = bad || broken(params[o + k]);
    }
    let opacity = 1.0 / (1.0 + exp(-params[o + 10u]));
    if (bad || opacity < DEAD || stats[i].z > info.max_radius) {
        weights[i] = -1.0;
    } else if (params[o + SKY] > 0.5) {
        weights[i] = -2.0 - opacity;
    } else {
        weights[i] = opacity;
    }
}

@compute @workgroup_size(256)
fn adjust(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let k = flat_index(gid, nwg);
    if (k >= info.count) {
        return;
    }
    let i = plan[4u * k];
    let o = i * PARAM;
    let delta = bitcast<f32>(plan[4u * k + 2u]);
    params[o + 10u] = bitcast<f32>(plan[4u * k + 1u]);
    params[o + 3u] += delta;
    params[o + 4u] += delta;
    params[o + 5u] += delta;
    restart(i);
}

@compute @workgroup_size(256)
fn copy(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let k = flat_index(gid, nwg);
    if (k >= info.count) {
        return;
    }
    let t = plan[8u * k];
    let s = plan[8u * k + 1u];
    let e = vec3<f32>(bitcast<f32>(plan[8u * k + 2u]), bitcast<f32>(plan[8u * k + 3u]), bitcast<f32>(plan[8u * k + 4u]));
    let so = s * PARAM;
    let to = t * PARAM;
    let q = vec4<f32>(params[so + 6u], params[so + 7u], params[so + 8u], params[so + 9u]);
    let off = rotate(q, e * exp(vec3<f32>(params[so + 3u], params[so + 4u], params[so + 5u])));
    params[to] = params[so] + off.x;
    params[to + 1u] = params[so + 1u] + off.y;
    params[to + 2u] = params[so + 2u] + off.z;
    for (var c = 3u; c < PARAM; c++) {
        params[to + c] = params[so + c];
    }
    restart(t);
}
