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
    let op = sigmoid(params[b + 10u]);
    let mid = 0.5 * (a + c);
    let lambda = mid + sqrt(max(mid * mid - det, 0.1));
    // Out to α = 1/255 at this opacity (cpu::extent_sigmas); a Gaussian that never gets there draws nothing.
    let tau = alpha_reach(op);
    if (tau <= 0.0) {
        return;
    }
    let sigmas = sqrt(tau);
    let radius = ceil(sigmas * sqrt(lambda));
    // The ellipse's own bounding box (per-axis extents), then only the tiles it reaches (tile_hit): flat discs seen at
    // grazing angles would otherwise fill the square of their largest axis.
    let rx = sigmas * sqrt(a);
    let ry = sigmas * sqrt(c);
    let tx = f32(frame.size.z);
    let ty = f32(frame.size.w);
    let tf = f32(TILE);
    let x0 = clamp(floor((mx - rx) / tf), 0.0, tx);
    let y0 = clamp(floor((my - ry) / tf), 0.0, ty);
    let x1 = clamp(floor((mx + rx) / tf + 1.0), 0.0, tx);
    let y1 = clamp(floor((my + ry) / tf + 1.0), 0.0, ty);
    if (x1 <= x0 || y1 <= y0) {
        return;
    }
    let conic = vec3<f32>(c / det, -bb / det, a / det);
    var hits = 0u;
    for (var row = u32(y0); row < u32(y1); row++) {
        for (var col = u32(x0); col < u32(x1); col++) {
            if (tile_hit(vec2<f32>(mx, my), conic, tau, col, row)) {
                hits++;
            }
        }
    }
    if (hits == 0u) {
        return;
    }
    proj[o] = mx;
    proj[o + 1u] = my;
    proj[o + 2u] = conic.x;
    proj[o + 3u] = conic.y;
    proj[o + 4u] = conic.z;
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
    counts[i] = hits;
}
