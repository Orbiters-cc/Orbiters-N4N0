// The tile entries with large Gaussians split off (lab, S4_SPLIT_BIG; after common.wgsl; project_split.wgsl listed
// them): duplicate.wgsl for the rest, and `duplicate_big` writes a listed Gaussian's entries with a workgroup, in any
// order inside its slice (its entries' keys all differ in the tile, so the sort orders them alike).

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> proj: array<f32>;
@group(0) @binding(2) var<storage, read> offsets: array<u32>;
@group(0) @binding(3) var<storage, read_write> keys: array<u32>;
@group(0) @binding(4) var<storage, read_write> values: array<u32>;
@group(0) @binding(5) var<storage, read> counts: array<u32>;
@group(0) @binding(6) var<storage, read_write> big: array<atomic<u32>>;

const BIG_CAP: u32 = 65535u;

fn depth_key(o: u32) -> u32 {
    let depth_bits = frame.counts.z;
    let max_d = (1u << depth_bits) - 1u;
    let near = frame.limits.z;
    let far = frame.limits.w;
    let t = clamp(log2(proj[o + 5u] / near) / log2(far / near), 0.0, 1.0);
    return min(u32(t * f32(max_d)), max_d);
}

@compute @workgroup_size(256)
fn duplicate_split(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= frame.counts.x) {
        return;
    }
    let o = i * PROJ;
    // Not shown, or listed (duplicate_big writes it).
    if (proj[o + 15u] != 1.0) {
        return;
    }
    let depth_bits = frame.counts.z;
    let dq = depth_key(o);
    let x0 = bitcast<u32>(proj[o + 11u]);
    let y0 = bitcast<u32>(proj[o + 12u]);
    let x1 = bitcast<u32>(proj[o + 13u]);
    let y1 = bitcast<u32>(proj[o + 14u]);
    var off = offsets[i];
    let end = off + counts[i];
    let cap = arrayLength(&keys);
    let m = vec2<f32>(proj[o], proj[o + 1u]);
    let conic = vec3<f32>(proj[o + 2u], proj[o + 3u], proj[o + 4u]);
    let tau = alpha_reach(proj[o + 6u]);
    for (var ty = y0; ty < y1; ty++) {
        for (var tx = x0; tx < x1; tx++) {
            if (off < end && tile_hit(m, conic, tau, tx, ty)) {
                if (off < cap) {
                    let tile = ty * frame.size.z + tx;
                    keys[off] = (tile << depth_bits) | dq;
                    values[off] = i;
                }
                off++;
            }
        }
    }
    for (; off < min(end, cap); off++) {
        keys[off] = 0xffffffffu;
        values[off] = 0u;
    }
}

var<workgroup> dup_listed: u32;
var<workgroup> dup_cursor: atomic<u32>;

@compute @workgroup_size(256)
fn duplicate_big(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    if (li == 0u) {
        dup_listed = min(atomicLoad(&big[0]), BIG_CAP);
    }
    let listed = workgroupUniformLoad(&dup_listed);
    let cap = arrayLength(&keys);
    let depth_bits = frame.counts.z;
    for (var k = wid.x; k < listed; k += nwg.x) {
        let i = atomicLoad(&big[1u + k]);
        let o = i * PROJ;
        // Shown after project_big's count (none left: nothing to write, every thread still reaches the barriers).
        let shown = proj[o + 15u] == 2.0;
        let x0 = bitcast<u32>(proj[o + 11u]);
        let y0 = bitcast<u32>(proj[o + 12u]);
        let bw = bitcast<u32>(proj[o + 13u]) - x0;
        let tiles = select(0u, bw * (bitcast<u32>(proj[o + 14u]) - y0), shown);
        let base = offsets[i];
        let n = select(0u, counts[i], shown);
        let dq = depth_key(o);
        let m = vec2<f32>(proj[o], proj[o + 1u]);
        let conic = vec3<f32>(proj[o + 2u], proj[o + 3u], proj[o + 4u]);
        let tau = alpha_reach(proj[o + 6u]);
        if (li == 0u) {
            atomicStore(&dup_cursor, 0u);
        }
        workgroupBarrier();
        for (var t = li; t < tiles; t += 256u) {
            let tx = x0 + t % bw;
            let ty = y0 + t / bw;
            if (tile_hit(m, conic, tau, tx, ty)) {
                let s = atomicAdd(&dup_cursor, 1u);
                if (s < n && base + s < cap) {
                    keys[base + s] = ((ty * frame.size.z + tx) << depth_bits) | dq;
                    values[base + s] = i;
                }
            }
        }
        workgroupBarrier();
        // Should counting and writing ever disagree, what was not written sorts last and names no tile.
        let written = min(atomicLoad(&dup_cursor), n);
        for (var s = written + li; s < n; s += 256u) {
            if (base + s < cap) {
                keys[base + s] = 0xffffffffu;
                values[base + s] = 0u;
            }
        }
        workgroupBarrier();
    }
}
