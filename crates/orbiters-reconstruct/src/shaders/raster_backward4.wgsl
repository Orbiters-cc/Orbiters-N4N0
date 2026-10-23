// Backward with four pixels a thread (rows 4k to 4k + 3 of one column): raster_backward2.wgsl's scheme taken further.

var<workgroup> tile_last4: atomic<u32>;

@diagnostic(off, subgroup_uniformity)
@compute @workgroup_size(64)
fn backward4(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(subgroup_invocation_id) lane: u32, @builtin(subgroup_size) lanes: u32) {
    let tile = wid.x + wid.y * frame.size.z;
    let px = wid.x * TILE + li % TILE;
    let py0 = wid.y * TILE + 4u * (li / TILE);
    let first = ranges[2u * tile];
    var p0: Pixel;
    var p1: Pixel;
    var p2: Pixel;
    var p3: Pixel;
    var last0 = first;
    var last1 = first;
    var last2 = first;
    var last3 = first;
    var dp0 = vec4<f32>(0.0);
    var dp1 = vec4<f32>(0.0);
    var dp2 = vec4<f32>(0.0);
    var dp3 = vec4<f32>(0.0);
    p0.t = 1.0;
    p1.t = 1.0;
    p2.t = 1.0;
    p3.t = 1.0;
    let inx = px < frame.size.x;
    let in0 = inx && py0 < frame.size.y;
    let in1 = inx && py0 + 1u < frame.size.y;
    let in2 = inx && py0 + 2u < frame.size.y;
    let in3 = inx && py0 + 3u < frame.size.y;
    if (in0) {
        let p = py0 * frame.size.x + px;
        let state = out_state[p];
        p0.t = bitcast<f32>(state.x);
        last0 = state.y;
        dp0 = d_pixel[p];
    }
    if (in1) {
        let p = (py0 + 1u) * frame.size.x + px;
        let state = out_state[p];
        p1.t = bitcast<f32>(state.x);
        last1 = state.y;
        dp1 = d_pixel[p];
    }
    if (in2) {
        let p = (py0 + 2u) * frame.size.x + px;
        let state = out_state[p];
        p2.t = bitcast<f32>(state.x);
        last2 = state.y;
        dp2 = d_pixel[p];
    }
    if (in3) {
        let p = (py0 + 3u) * frame.size.x + px;
        let state = out_state[p];
        p3.t = bitcast<f32>(state.x);
        last3 = state.y;
        dp3 = d_pixel[p];
    }
    let live0 = in0 && any(dp0 != vec4<f32>(0.0));
    let live1 = in1 && any(dp1 != vec4<f32>(0.0));
    let live2 = in2 && any(dp2 != vec4<f32>(0.0));
    let live3 = in3 && any(dp3 != vec4<f32>(0.0));
    if (li == 0u) {
        atomicStore(&tile_last4, first);
    }
    workgroupBarrier();
    atomicMax(&tile_last4, max(max(select(first, last0, live0), select(first, last1, live1)), max(select(first, last2, live2), select(first, last3, live3))));
    let used_end = workgroupUniformLoad(&tile_last4);
    p0.acc_c = background_at(f32(px), f32(py0));
    p1.acc_c = background_at(f32(px), f32(py0 + 1u));
    p2.acc_c = background_at(f32(px), f32(py0 + 2u));
    p3.acc_c = background_at(f32(px), f32(py0 + 3u));
    p0.acc_d = 0.0;
    p1.acc_d = 0.0;
    p2.acc_d = 0.0;
    p3.acc_d = 0.0;
    let pix0 = vec2<f32>(f32(px), f32(py0));
    let pix1 = vec2<f32>(f32(px), f32(py0 + 1u));
    let pix2 = vec2<f32>(f32(px), f32(py0 + 2u));
    let pix3 = vec2<f32>(f32(px), f32(py0 + 3u));
    let batches = (used_end - first + 255u) / 256u;
    for (var b = 0u; b < batches; b++) {
        // Batch b holds entries [stop − 256, stop), loaded far to near, four a thread.
        let stop = used_end - b * 256u;
        let start = max(first, select(0u, stop - 256u, stop >= 256u));
        let count = stop - start;
        workgroupBarrier();
        for (var k = li; k < count; k += 64u) {
            load(k, stop - 1u - k);
        }
        workgroupBarrier();
        for (var j = 0u; j < count; j++) {
            // No early exits: every lane of a subgroup reaches the reduction below for every entry.
            let entry = stop - 1u - j;
            let a = s_a[j];
            let bb = s_b[j];
            let col = s_c[j].xyz;
            let r0 = pixel_grad(&p0, pix0, dp0, live0 && entry < last0, a, bb, col);
            let r1 = pixel_grad(&p1, pix1, dp1, live1 && entry < last1, a, bb, col);
            let r2 = pixel_grad(&p2, pix2, dp2, live2 && entry < last2, a, bb, col);
            let r3 = pixel_grad(&p3, pix3, dp3, live3 && entry < last3, a, bb, col);
            let g0 = (r0.g0 + r1.g0) + (r2.g0 + r3.g0);
            let g1 = (r0.g1 + r1.g1) + (r2.g1 + r3.g1);
            let g2 = (r0.g2 + r1.g2) + (r2.g2 + r3.g2);
            let hit = (r0.hit || r1.hit) || (r2.hit || r3.hit);
            let any_hit = subgroupAny(hit);
            if (any_hit && LANE_ADDS && SUM10 && lanes == 32u) {
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
