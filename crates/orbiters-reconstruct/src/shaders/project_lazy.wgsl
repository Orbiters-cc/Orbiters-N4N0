// A training step's projection with lazily dense Adam (after common.wgsl and lazy_adam.wgsl): project.wgsl's projection
// without a viewer's guards, confidence or far field; a Gaussian this step shows that missed earlier steps first catches
// up (lazy_adam.wgsl), then projects again from where dense Adam would have it. `catch_up_all` brings every Gaussian up
// to the last step run, before anything else reads the set.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read_write> params4: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> proj: array<f32>;
@group(0) @binding(3) var<storage, read_write> counts: array<u32>;
@group(0) @binding(4) var<storage, read> filter3d: array<f32>;

// Projects Gaussian i into proj and counts (as project.wgsl does for training); the tiles it touches.
fn project_one(i: u32) -> u32 {
    let o = i * PROJ;
    proj[o + 15u] = 0.0;
    counts[i] = 0u;
    let p0 = params4[4u * i];
    let p1 = params4[4u * i + 1u];
    let p2 = params4[4u * i + 2u];
    let p3 = params4[4u * i + 3u];
    let mean = p0.xyz;
    let log_scale = vec3<f32>(p0.w, p1.x, p1.y);
    let q = vec4<f32>(p1.z, p1.w, p2.x, p2.y);
    let m = proj_math(mean, log_scale, q, filter3d[i]);
    if (!m.ok) {
        return 0u;
    }
    let a = m.cov.x;
    let bb = m.cov.y;
    let c = m.cov.z;
    let mx = frame.focal.x * m.pc.x / m.pc.z + frame.focal.z;
    let my = frame.focal.y * m.pc.y / m.pc.z + frame.focal.w;
    let op = sigmoid(p2.z) * m.c3 * m.c2;
    let depth = m.pc.z;
    let color = vec3<f32>(p2.w, p3.x, p3.y);
    let tau = alpha_reach(op);
    if (tau <= 0.0) {
        return 0u;
    }
    let sigmas = sqrt(tau);
    let det = a * c - bb * bb;
    let mid = 0.5 * (a + c);
    let lambda = mid + sqrt(max(mid * mid - det, 0.1));
    let radius = ceil(sigmas * sqrt(lambda));
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
        return 0u;
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
        return 0u;
    }
    proj[o] = mx;
    proj[o + 1u] = my;
    proj[o + 2u] = conic.x;
    proj[o + 3u] = conic.y;
    proj[o + 4u] = conic.z;
    proj[o + 5u] = depth;
    proj[o + 6u] = op;
    proj[o + 7u] = color.x;
    proj[o + 8u] = color.y;
    proj[o + 9u] = color.z;
    proj[o + 10u] = radius;
    proj[o + 11u] = bitcast<f32>(u32(x0));
    proj[o + 12u] = bitcast<f32>(u32(y0));
    proj[o + 13u] = bitcast<f32>(u32(x1));
    proj[o + 14u] = bitcast<f32>(u32(y1));
    proj[o + 15u] = 1.0;
    counts[i] = hits;
    return hits;
}

@compute @workgroup_size(256)
fn project_lazy(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= frame.counts.x) {
        return;
    }
    // Shown with its parameters as they stand: catch up the steps before this one (adam.n.y), and project again.
    if (project_one(i) > 0u && adam.n.y > 0u && catch_up(i, adam.n.y - 1u)) {
        project_one(i);
    }
}

@compute @workgroup_size(256)
fn catch_up_all(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= adam.n.x / PARAM) {
        return;
    }
    catch_up(i, adam.n.y);
}
