// Projection: each Gaussian's 2D mean, conic, depth, opacity, colour, radius and tile rectangle, and how many
// tiles it touches.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> params: array<f32>;
@group(0) @binding(2) var<storage, read_write> proj: array<f32>;
@group(0) @binding(3) var<storage, read_write> counts: array<u32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= frame.counts.x) {
        return;
    }
    let o = i * PROJ;
    proj[o + 15u] = 0.0;
    counts[i] = 0u;
    let b = i * PARAM;
    let mean = vec3<f32>(params[b], params[b + 1u], params[b + 2u]);
    let log_scale = vec3<f32>(params[b + 3u], params[b + 4u], params[b + 5u]);
    let q = vec4<f32>(params[b + 6u], params[b + 7u], params[b + 8u], params[b + 9u]);
    let m = proj_math(mean, log_scale, q);
    if (!m.ok) {
        return;
    }
    let a = m.cov.x;
    let bb = m.cov.y;
    let c = m.cov.z;
    let det = a * c - bb * bb;
    let mx = frame.focal.x * m.pc.x / m.pc.z + frame.focal.z;
    let my = frame.focal.y * m.pc.y / m.pc.z + frame.focal.w;
    var op = sigmoid(params[b + 10u]);
    // DropGaussian: this step leaves the Gaussian out with probability r and lifts the others' opacity by 1/(1 − r).
    let drop = frame.background.w;
    if (drop > 0.0) {
        if (unit_hash(i * 2654435769u ^ frame.counts.w) < drop) {
            return;
        }
        op = op / (1.0 - drop);
    }
    let mid = 0.5 * (a + c);
    let lambda = mid + sqrt(max(mid * mid - det, 0.1));
    // Out to α = 1/255 at this opacity (cpu::extent_sigmas).
    let sigmas = min(sqrt(2.0 * log(max(op / MIN_ALPHA, 1.0))), 3.33);
    let radius = ceil(sigmas * sqrt(lambda));
    let tx = f32(frame.size.z);
    let ty = f32(frame.size.w);
    let tf = f32(TILE);
    let x0 = clamp(floor((mx - radius) / tf), 0.0, tx);
    let y0 = clamp(floor((my - radius) / tf), 0.0, ty);
    let x1 = clamp(floor((mx + radius) / tf + 1.0), 0.0, tx);
    let y1 = clamp(floor((my + radius) / tf + 1.0), 0.0, ty);
    if (x1 <= x0 || y1 <= y0) {
        return;
    }
    proj[o] = mx;
    proj[o + 1u] = my;
    proj[o + 2u] = c / det;
    proj[o + 3u] = -bb / det;
    proj[o + 4u] = a / det;
    proj[o + 5u] = m.pc.z;
    proj[o + 6u] = op;
    proj[o + 7u] = params[b + 11u];
    proj[o + 8u] = params[b + 12u];
    proj[o + 9u] = params[b + 13u];
    proj[o + 10u] = radius;
    proj[o + 11u] = bitcast<f32>(u32(x0));
    proj[o + 12u] = bitcast<f32>(u32(y0));
    proj[o + 13u] = bitcast<f32>(u32(x1));
    proj[o + 14u] = bitcast<f32>(u32(y1));
    proj[o + 15u] = 1.0;
    counts[i] = u32(x1 - x0) * u32(y1 - y0);
}
