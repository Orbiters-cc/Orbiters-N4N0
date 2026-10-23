// Backward with two pixels a thread (after raster_backward.wgsl, whose bindings and sums it shares): a thread walks two
// rows of one column, so each entry is read from workgroup memory once for both, and a lane adds its two pixels'
// gradients in registers before the subgroup sums them: half the sums and atomics of one pixel a thread. Each 32
// threads hold an 8×8 block of the tile, and a subgroup skips, all lanes together, an entry whose radius misses its
// live pixels' box or that lies past every one of its pixels' last blended entry: those entries would hit none of its
// pixels. Same entries, same order, the same gradients up to the order of float additions.

struct Pixel {
    t: f32,
    acc_c: vec3<f32>,
    acc_d: f32,
}

struct PixelGrad {
    g0: vec4<f32>,
    g1: vec2<f32>,
    g2: vec4<f32>,
    hit: bool,
}

// One pixel's share of an entry's gradients (raster_backward.wgsl `backward`, per pixel), moving its walk one entry on.
fn pixel_grad(px: ptr<function, Pixel>, pix: vec2<f32>, dp: vec4<f32>, live: bool, a: vec4<f32>, bb: vec4<f32>, col: vec3<f32>) -> PixelGrad {
    var out: PixelGrad;
    out.g0 = vec4<f32>(0.0);
    out.g1 = vec2<f32>(0.0);
    out.g2 = vec4<f32>(0.0);
    let d = pix - a.xy;
    let power = -0.5 * (a.z * d.x * d.x + bb.x * d.y * d.y) - a.w * d.x * d.y;
    let gauss = exp(min(power, 0.0));
    let raw = bb.y * gauss;
    let alpha = min(MAX_ALPHA, raw);
    out.hit = live && power <= 0.0 && alpha >= MIN_ALPHA;
    if (out.hit) {
        let tb = (*px).t / (1.0 - alpha);
        let w = alpha * tb;
        out.g2 = dp * w;
        let d_alpha = dot(dp.xyz, col - (*px).acc_c) * tb + dp.w * (bb.z - (*px).acc_d) * tb;
        (*px).acc_c = alpha * col + (1.0 - alpha) * (*px).acc_c;
        (*px).acc_d = alpha * bb.z + (1.0 - alpha) * (*px).acc_d;
        (*px).t = tb;
        if (raw <= MAX_ALPHA) {
            let d_power = d_alpha * bb.y * gauss;
            out.g0 = d_power * vec4<f32>(a.z * d.x + a.w * d.y, bb.x * d.y + a.w * d.x, -0.5 * d.x * d.x, -d.x * d.y);
            out.g1 = vec2<f32>(d_power * (-0.5 * d.y * d.y), d_alpha * gauss);
        }
    }
    return out;
}

var<workgroup> tile_last2: atomic<u32>;

@diagnostic(off, subgroup_uniformity)
@compute @workgroup_size(128)
fn backward2(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(subgroup_invocation_id) lane: u32, @builtin(subgroup_size) lanes: u32) {
    let tile = wid.x + wid.y * frame.size.z;
    // Threads 32b..32b+31 hold the tile's 8×8 block b (two by two), two rows each.
    let block = li / 32u;
    let lb = li % 32u;
    let px = wid.x * TILE + (block % 2u) * 8u + lb % 8u;
    let py0 = wid.y * TILE + (block / 2u) * 8u + 2u * (lb / 8u);
    let py1 = py0 + 1u;
    let in0 = px < frame.size.x && py0 < frame.size.y;
    let in1 = px < frame.size.x && py1 < frame.size.y;
    let first = ranges[2u * tile];
    var p0: Pixel;
    var p1: Pixel;
    p0.t = 1.0;
    p1.t = 1.0;
    var last0 = first;
    var last1 = first;
    var dp0 = vec4<f32>(0.0);
    var dp1 = vec4<f32>(0.0);
    if (in0) {
        let p = py0 * frame.size.x + px;
        let state = out_state[p];
        p0.t = bitcast<f32>(state.x);
        last0 = state.y;
        dp0 = d_pixel[p];
    }
    if (in1) {
        let p = py1 * frame.size.x + px;
        let state = out_state[p];
        p1.t = bitcast<f32>(state.x);
        last1 = state.y;
        dp1 = d_pixel[p];
    }
    let live0 = in0 && any(dp0 != vec4<f32>(0.0));
    let live1 = in1 && any(dp1 != vec4<f32>(0.0));
    if (li == 0u) {
        atomicStore(&tile_last2, first);
    }
    workgroupBarrier();
    atomicMax(&tile_last2, max(select(first, last0, live0), select(first, last1, live1)));
    let used_end = workgroupUniformLoad(&tile_last2);
    p0.acc_c = background_at(f32(px), f32(py0));
    p1.acc_c = background_at(f32(px), f32(py1));
    p0.acc_d = 0.0;
    p1.acc_d = 0.0;
    let pix0 = vec2<f32>(f32(px), f32(py0));
    let pix1 = vec2<f32>(f32(px), f32(py1));
    // This subgroup's live pixels: their box, and one past the last entry any of them blended.
    let live = live0 || live1;
    let lo = subgroupMin(select(vec2<f32>(3.4e38), vec2<f32>(f32(px), f32(select(py1, py0, live0))), live));
    let hi = subgroupMax(select(vec2<f32>(-3.4e38), vec2<f32>(f32(px), f32(select(py0, py1, live1))), live));
    let sub_last = subgroupMax(max(select(first, last0, live0), select(first, last1, live1)));
    let batches = (used_end - first + 255u) / 256u;
    for (var b = 0u; b < batches; b++) {
        // Batch b holds entries [stop − 256, stop), loaded far to near, two a thread.
        let stop = used_end - b * 256u;
        let start = max(first, select(0u, stop - 256u, stop >= 256u));
        let count = stop - start;
        workgroupBarrier();
        if (li < count) {
            load(li, stop - 1u - li);
        }
        if (li + 128u < count) {
            load(li + 128u, stop - 1u - (li + 128u));
        }
        workgroupBarrier();
        for (var j = 0u; j < count; j++) {
            // No early exits: every lane of a subgroup reaches the reduction below for every entry.
            let entry = stop - 1u - j;
            let a = s_a[j];
            let cr = s_c[j];
            // Subgroup-uniform: no pixel of this subgroup's box is within the entry's reach, or none blended it.
            if (entry >= sub_last || a.x + cr.w < lo.x || a.x - cr.w > hi.x || a.y + cr.w < lo.y || a.y - cr.w > hi.y) {
                continue;
            }
            let bb = s_b[j];
            let col = cr.xyz;
            let r0 = pixel_grad(&p0, pix0, dp0, live0 && entry < last0, a, bb, col);
            let r1 = pixel_grad(&p1, pix1, dp1, live1 && entry < last1, a, bb, col);
            let g0 = r0.g0 + r1.g0;
            let g1 = r0.g1 + r1.g1;
            let g2 = r0.g2 + r1.g2;
            let hit = r0.hit || r1.hit;
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
