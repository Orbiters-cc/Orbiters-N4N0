// A training device's projection with large Gaussians split off (lab, S4_SPLIT_BIG; after common.wgsl): project.wgsl's
// projection without a viewer's guards, confidence or far field, except that a Gaussian whose tile box holds more than
// BIG_TILES tiles goes on the `big` list (proj slot 15 = 2: listed) instead of having one thread test them all:
// `project_big` then gives each listed Gaussian a workgroup to count its tiles. One thread walking an 8,160-tile box
// set how long the whole pass took. A full list leaves the rest to their own threads, as before.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> params: array<f32>;
@group(0) @binding(2) var<storage, read_write> proj: array<f32>;
@group(0) @binding(3) var<storage, read_write> counts: array<u32>;
@group(0) @binding(4) var<storage, read> filter3d: array<f32>;
// [0]: Gaussians listed (may pass BIG_CAP: those past it were counted in place), then the list.
@group(0) @binding(6) var<storage, read_write> big: array<atomic<u32>>;

const BIG_TILES: u32 = 256u;
const BIG_CAP: u32 = 65535u;

@compute @workgroup_size(256)
fn project_split(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
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
    let m = proj_math(mean, log_scale, q, filter3d[i]);
    if (!m.ok) {
        return;
    }
    let a = m.cov.x;
    let bb = m.cov.y;
    let c = m.cov.z;
    let mx = frame.focal.x * m.pc.x / m.pc.z + frame.focal.z;
    let my = frame.focal.y * m.pc.y / m.pc.z + frame.focal.w;
    let op = sigmoid(params[b + 10u]) * m.c3 * m.c2;
    let depth = m.pc.z;
    let color = vec3<f32>(params[b + 11u], params[b + 12u], params[b + 13u]);
    let tau = alpha_reach(op);
    if (tau <= 0.0) {
        return;
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
        return;
    }
    let conic = vec3<f32>(c / det, -bb / det, a / det);
    var hits = 0u;
    var listed = false;
    if (u32(x1 - x0) * u32(y1 - y0) > BIG_TILES) {
        let slot = atomicAdd(&big[0], 1u);
        if (slot < BIG_CAP) {
            atomicStore(&big[1u + slot], i);
            listed = true;
        }
    }
    if (!listed) {
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
    proj[o + 15u] = select(1.0, 2.0, listed);
    counts[i] = hits;
}

var<workgroup> big_listed: u32;
var<workgroup> big_hits: atomic<u32>;

// One workgroup a listed Gaussian (a grid of workgroups going through the list): its tiles counted by all 256 threads.
// One with no tile left after all is not shown (slot 15 back to 0), as project.wgsl leaves it.
@compute @workgroup_size(256)
fn project_big(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    if (li == 0u) {
        big_listed = min(atomicLoad(&big[0]), BIG_CAP);
    }
    let listed = workgroupUniformLoad(&big_listed);
    for (var k = wid.x; k < listed; k += nwg.x) {
        let i = atomicLoad(&big[1u + k]);
        let o = i * PROJ;
        let m = vec2<f32>(proj[o], proj[o + 1u]);
        let conic = vec3<f32>(proj[o + 2u], proj[o + 3u], proj[o + 4u]);
        let tau = alpha_reach(proj[o + 6u]);
        let x0 = bitcast<u32>(proj[o + 11u]);
        let y0 = bitcast<u32>(proj[o + 12u]);
        let bw = bitcast<u32>(proj[o + 13u]) - x0;
        let tiles = bw * (bitcast<u32>(proj[o + 14u]) - y0);
        if (li == 0u) {
            atomicStore(&big_hits, 0u);
        }
        workgroupBarrier();
        var mine = 0u;
        for (var t = li; t < tiles; t += 256u) {
            if (tile_hit(m, conic, tau, x0 + t % bw, y0 + t / bw)) {
                mine++;
            }
        }
        atomicAdd(&big_hits, mine);
        workgroupBarrier();
        if (li == 0u) {
            let h = atomicLoad(&big_hits);
            counts[i] = h;
            if (h == 0u) {
                proj[o + 15u] = 0.0;
            }
        }
        workgroupBarrier();
    }
}
