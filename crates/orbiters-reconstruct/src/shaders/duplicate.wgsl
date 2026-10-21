// One sort entry per tile a Gaussian touches: key = tile in the high bits, log depth below; value = the Gaussian.
// The tiles are the ones the projection counted, by the same tile_hit test; should the two passes ever disagree, a
// Gaussian never writes past its count and fills what it did not write with sentinel keys, which sort after every
// tile and which ranges.wgsl skips.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> proj: array<f32>;
@group(0) @binding(2) var<storage, read> offsets: array<u32>;
@group(0) @binding(3) var<storage, read_write> keys: array<u32>;
@group(0) @binding(4) var<storage, read_write> values: array<u32>;
@group(0) @binding(5) var<storage, read> counts: array<u32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= frame.counts.x) {
        return;
    }
    let o = i * PROJ;
    if (proj[o + 15u] == 0.0) {
        return;
    }
    let depth_bits = frame.counts.z;
    let max_q = f32((1u << depth_bits) - 1u);
    let near = frame.limits.z;
    let far = frame.limits.w;
    let t = clamp(log2(proj[o + 5u] / near) / log2(far / near), 0.0, 1.0);
    let dq = u32(t * max_q);
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
