// Backward: walks the forward pass's entries back to front from each pixel's last one, recovering T by division, and
// adds every Gaussian's 2D gradients (mean, conic, opacity, colour, depth), summed over each subgroup, with atomics.
// The gradient buffer (binding 7), `add` and LANE_ADDS come from one of the sums_*.wgsl variants.

@group(0) @binding(6) var<storage, read> d_pixel: array<vec4<f32>>;

// One past the last entry any pixel of the tile blended (backward starts there).
var<workgroup> tile_last: atomic<u32>;

// One-dimensional workgroup (subgroup builtins need one), still one 16×16 tile.
@compute @workgroup_size(256)
fn backward(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(subgroup_invocation_id) lane: u32) {
    let tile = wid.x + wid.y * frame.size.z;
    let px = wid.x * TILE + li % TILE;
    let py = wid.y * TILE + li / TILE;
    let inside = px < frame.size.x && py < frame.size.y;
    let first = ranges[2u * tile];
    let end = ranges[2u * tile + 1u];
    var t = 1.0;
    var last = first;
    var dp = vec4<f32>(0.0);
    if (inside) {
        let p = py * frame.size.x + px;
        let state = out_state[p];
        t = bitcast<f32>(state.x);
        last = state.y;
        dp = d_pixel[p];
    }
    if (li == 0u) {
        atomicStore(&tile_last, first);
    }
    workgroupBarrier();
    atomicMax(&tile_last, last);
    let used_end = workgroupUniformLoad(&tile_last);
    var acc_c = frame.background.xyz;
    var acc_d = 0.0;
    let pix = vec2<f32>(f32(px), f32(py));
    let batches = (used_end - first + 255u) / 256u;
    for (var b = 0u; b < batches; b++) {
        // Batch b holds entries [stop − 256, stop), loaded far to near.
        let stop = used_end - b * 256u;
        let start = max(first, select(0u, stop - 256u, stop >= 256u));
        let count = stop - start;
        workgroupBarrier();
        if (li < count) {
            load(li, stop - 1u - li);
        }
        workgroupBarrier();
        for (var j = 0u; j < count; j++) {
            // No early exits: every lane of a subgroup reaches the reduction below for every entry.
            let entry = stop - 1u - j;
            let a = s_a[j];
            let bb = s_b[j];
            let col = s_c[j].xyz;
            let d = pix - a.xy;
            let power = -0.5 * (a.z * d.x * d.x + bb.x * d.y * d.y) - a.w * d.x * d.y;
            let gauss = exp(min(power, 0.0));
            let raw = bb.y * gauss;
            let alpha = min(MAX_ALPHA, raw);
            // GRAD2D slots 0–3, 4–5 and 6–9 (no dynamically indexed array: it would live in slow memory).
            var g0 = vec4<f32>(0.0);
            var g1 = vec2<f32>(0.0);
            var g2 = vec4<f32>(0.0);
            let hit = inside && entry < last && power <= 0.0 && alpha >= MIN_ALPHA;
            if (hit) {
                let tb = t / (1.0 - alpha);
                let w = alpha * tb;
                g2 = dp * w;
                let d_alpha = dot(dp.xyz, col - acc_c) * tb + dp.w * (bb.z - acc_d) * tb;
                acc_c = alpha * col + (1.0 - alpha) * acc_c;
                acc_d = alpha * bb.z + (1.0 - alpha) * acc_d;
                t = tb;
                if (raw <= MAX_ALPHA) {
                    let d_power = d_alpha * bb.y * gauss;
                    g0 = d_power * vec4<f32>(a.z * d.x + a.w * d.y, bb.x * d.y + a.w * d.x, -0.5 * d.x * d.x, -d.x * d.y);
                    g1 = vec2<f32>(d_power * (-0.5 * d.y * d.y), d_alpha * gauss);
                }
            }
            // Sum over the subgroup first: one atomic per subgroup instead of one per pixel.
            if (subgroupAny(hit)) {
                let s0 = subgroupAdd(g0);
                let s1 = subgroupAdd(g1);
                let s2 = subgroupAdd(g2);
                let g = bitcast<u32>(bb.w) * GRAD2D;
                if (LANE_ADDS) {
                    // Lane k adds slot k: ten independent atomics instead of a chain of ten.
                    if (lane < 10u) {
                        var v = s0[lane & 3u];
                        if (lane >= 4u) {
                            v = s1[lane & 1u];
                        }
                        if (lane >= 6u) {
                            v = s2[(lane - 6u) & 3u];
                        }
                        add(g + lane, v);
                    }
                } else if (lane == 0u) {
                    add(g, s0.x);
                    add(g + 1u, s0.y);
                    add(g + 2u, s0.z);
                    add(g + 3u, s0.w);
                    add(g + 4u, s1.x);
                    add(g + 5u, s1.y);
                    add(g + 6u, s2.x);
                    add(g + 7u, s2.y);
                    add(g + 8u, s2.z);
                    add(g + 9u, s2.w);
                }
            }
        }
    }
}
