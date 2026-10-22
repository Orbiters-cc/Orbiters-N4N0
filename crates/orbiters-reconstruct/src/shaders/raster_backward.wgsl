// Backward: walks the forward pass's entries back to front from each pixel's last one, recovering T by division, and
// adds every Gaussian's 2D gradients (mean, conic, opacity, colour, depth), summed over each subgroup, with atomics.
// The sums cost more than the atomics: on 32-lane subgroups they are transposed (sum10).
// The gradient buffer (binding 7), `add` and LANE_ADDS come from one of the sums_*.wgsl variants; SUM10 (use the
// transposed sums on 32 lanes) and ADD_LANES (how many lanes add the ten sums, 0: the subgroup's size) from gpu.rs.
// Pixels without a loss gradient (masked, or without a depth sample) add nothing, so they neither walk nor vote: a
// tile with none skips its walk.

@group(0) @binding(6) var<storage, read> d_pixel: array<vec4<f32>>;

// Slot k (0–9) of the ten gradient sums (no dynamically indexed array: it would live in slow memory).
fn slot10(s0: vec4<f32>, s1: vec2<f32>, s2: vec4<f32>, k: u32) -> f32 {
    var v = s0[k & 3u];
    if (k >= 4u) {
        v = s1[k & 1u];
    }
    if (k >= 6u) {
        v = s2[(k - 6u) & 3u];
    }
    return v;
}

// One past the last entry any pixel of the tile blended (backward starts there).
var<workgroup> tile_last: atomic<u32>;

struct Sum10 {
    total: f32,
    slot: u32,
    owner: bool,
}

// The ten gradient slots (v0: 0–3, v1: 4–5, v2: 6–9) summed over a 32-lane subgroup by transposed butterflies: at each
// step a lane keeps part of its values and swaps the rest with its partner, so the sums spread over the lanes: 12
// shuffles instead of 10 × 5. Lane bit 4 picks slots 5–9 or 0–4, bits 3–1 one of those five; `owner` marks one lane per
// slot.
fn sum10(v0: vec4<f32>, v1: vec2<f32>, v2: vec4<f32>, lane: u32) -> Sum10 {
    let b4 = (lane & 16u) != 0u;
    let b3 = (lane & 8u) != 0u;
    let b2 = (lane & 4u) != 0u;
    let b1 = (lane & 2u) != 0u;
    // Across 16: keep slots 0–4 or 5–9 (a0..a4), send the other five.
    let a0 = select(v0.x, v1.y, b4) + subgroupShuffleXor(select(v1.y, v0.x, b4), 16u);
    let a1 = select(v0.y, v2.x, b4) + subgroupShuffleXor(select(v2.x, v0.y, b4), 16u);
    let a2 = select(v0.z, v2.y, b4) + subgroupShuffleXor(select(v2.y, v0.z, b4), 16u);
    let a3 = select(v0.w, v2.z, b4) + subgroupShuffleXor(select(v2.z, v0.w, b4), 16u);
    let a4 = select(v1.x, v2.w, b4) + subgroupShuffleXor(select(v2.w, v1.x, b4), 16u);
    // Across 8: keep a0..a2 (c0..c2) or a3, a4 (c0, c1).
    let c0 = select(a0, a3, b3) + subgroupShuffleXor(select(a3, a0, b3), 8u);
    let c1 = select(a1, a4, b3) + subgroupShuffleXor(select(a4, a1, b3), 8u);
    let c2 = a2 + subgroupShuffleXor(select(0.0, a2, b3), 8u);
    // Across 4: of a0..a2 keep a0, a1 (d0, d1) or a2 (d0); of a3, a4 keep one (d0).
    let s1 = subgroupShuffleXor(select(select(c2, c0, b2), select(c1, c0, b2), b3), 4u);
    let s2 = subgroupShuffleXor(select(select(0.0, c1, b2), 0.0, b3), 4u);
    let d0 = select(select(c0, c2, b2), select(c0, c1, b2), b3) + s1;
    let d1 = c1 + s2;
    // Across 2: lanes still holding two values keep one; then across 1 every lane holds one whole sum.
    let two = !b3 && !b2;
    let e = select(d0, select(d0, d1, b1), two) + subgroupShuffleXor(select(d0, select(d1, d0, b1), two), 2u);
    var out: Sum10;
    out.total = e + subgroupShuffleXor(e, 1u);
    var k = 4u;
    if (two) {
        k = select(0u, 1u, b1);
    } else if (!b3) {
        k = 2u;
    } else if (!b2) {
        k = 3u;
    }
    out.slot = select(0u, 5u, b4) + k;
    out.owner = (lane & 1u) == 0u && (two || !b1);
    return out;
}

// One-dimensional workgroup (subgroup builtins need one), still one 16×16 tile. The subgroup operations run under
// `subgroupAny(hit)`, which is the same in every lane of a subgroup, and every lane reaches them for every entry.
@diagnostic(off, subgroup_uniformity)
@compute @workgroup_size(256)
fn backward(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(subgroup_invocation_id) lane: u32, @builtin(subgroup_size) lanes: u32) {
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
    // Only pixels with a gradient walk: the walk starts after the last entry one of them blended (none: no walk).
    let live = inside && any(dp != vec4<f32>(0.0));
    if (li == 0u) {
        atomicStore(&tile_last, first);
    }
    workgroupBarrier();
    atomicMax(&tile_last, select(first, last, live));
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
            let hit = live && entry < last && power <= 0.0 && alpha >= MIN_ALPHA;
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
            // Sum over the subgroup first: one atomic per subgroup instead of one per pixel. Other subgroup sizes and the
            // compare-and-swap sums sum each slot with subgroupAdd.
            let any_hit = subgroupAny(hit);
            if (any_hit && LANE_ADDS && SUM10 && lanes == 32u) {
                // A lone contributing lane adds its own values; more are summed first (transposed, sum10).
                let g = bitcast<u32>(bb.w) * GRAD2D;
                if (countOneBits(subgroupBallot(hit).x) == 1u) {
                    if (hit) {
                        add(g, g0.x);
                        add(g + 1u, g0.y);
                        add(g + 2u, g0.z);
                        add(g + 3u, g0.w);
                        add(g + 4u, g1.x);
                        add(g + 5u, g1.y);
                        add(g + 6u, g2.x);
                        add(g + 7u, g2.y);
                        add(g + 8u, g2.z);
                        add(g + 9u, g2.w);
                    }
                } else {
                    let s = sum10(g0, g1, g2, lane);
                    if (s.owner) {
                        add(g + s.slot, s.total);
                    }
                }
            } else if (any_hit) {
                let s0 = subgroupAdd(g0);
                let s1 = subgroupAdd(g1);
                let s2 = subgroupAdd(g2);
                let g = bitcast<u32>(bb.w) * GRAD2D;
                if (LANE_ADDS) {
                    // Lane k adds slot k (and on subgroups narrower than ten, k + lanes, …): ten independent atomics
                    // instead of a chain of ten, whatever the subgroup's size.
                    let adders = select(lanes, min(ADD_LANES, lanes), ADD_LANES > 0u);
                    if (lane < adders) {
                        for (var k = lane; k < 10u; k += adders) {
                            add(g + k, slot10(s0, s1, s2, k));
                        }
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
