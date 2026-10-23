// Forward with two pixels a thread (after raster_forward.wgsl, whose bindings it shares): a thread blends two rows of one
// column, each entry read from workgroup memory once for both; each 32 threads hold an 8×8 block of the tile, and a
// thread skips an entry whose radius misses both its pixels (a subgroup whose every lane does skips it together). The
// same blending, entry by entry, as one pixel a thread.

struct Blend {
    t: f32,
    color: vec3<f32>,
    depth: f32,
    last: u32,
    done: bool,
}

var<workgroup> done_count2: atomic<u32>;

// One pixel's blend of entry `e` (raster_forward.wgsl, per pixel).
fn blend(b: ptr<function, Blend>, pix: vec2<f32>, a: vec4<f32>, bb: vec4<f32>, col: vec3<f32>, e: u32) {
    let d = pix - a.xy;
    let power = -0.5 * (a.z * d.x * d.x + bb.x * d.y * d.y) - a.w * d.x * d.y;
    let alpha = min(MAX_ALPHA, bb.y * exp(min(power, 0.0)));
    if (power <= 0.0 && alpha >= MIN_ALPHA) {
        let next = (*b).t * (1.0 - alpha);
        if (next < MIN_T) {
            (*b).done = true;
            atomicAdd(&done_count2, 1u);
        } else {
            (*b).color += col * alpha * (*b).t;
            (*b).depth += bb.z * alpha * (*b).t;
            (*b).t = next;
            (*b).last = e + 1u;
        }
    }
}

@compute @workgroup_size(128)
fn forward2(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let tile = wid.x + wid.y * frame.size.z;
    let block = li / 32u;
    let lb = li % 32u;
    let px = wid.x * TILE + (block % 2u) * 8u + lb % 8u;
    let py0 = wid.y * TILE + (block / 2u) * 8u + 2u * (lb / 8u);
    let py1 = py0 + 1u;
    let in0 = px < frame.size.x && py0 < frame.size.y;
    let in1 = px < frame.size.x && py1 < frame.size.y;
    let first = ranges[2u * tile];
    let end = ranges[2u * tile + 1u];
    if (li == 0u) {
        atomicStore(&done_count2, 0u);
    }
    workgroupBarrier();
    var b0: Blend;
    var b1: Blend;
    b0.t = 1.0;
    b1.t = 1.0;
    b0.last = first;
    b1.last = first;
    b0.done = !in0;
    b1.done = !in1;
    if (b0.done) {
        atomicAdd(&done_count2, 1u);
    }
    if (b1.done) {
        atomicAdd(&done_count2, 1u);
    }
    let pix0 = vec2<f32>(f32(px), f32(py0));
    let pix1 = vec2<f32>(f32(px), f32(py1));
    for (var e = first; e < end; e++) {
        let slot = (e - first) & 255u;
        if (slot == 0u) {
            if (workgroupUniformLoad(&done_count2) == 256u) {
                break;
            }
            if (e + li < end) {
                load(li, e + li);
            }
            if (e + li + 128u < end) {
                load(li + 128u, e + li + 128u);
            }
            workgroupBarrier();
        }
        let a = s_a[slot];
        let cr = s_c[slot];
        // Within reach of either pixel (rows py0 and py1 of column px); no early exit, the batch barrier above follows.
        if (abs(a.x - pix0.x) <= cr.w && a.y + cr.w >= pix0.y && a.y - cr.w <= pix1.y) {
            let bb = s_b[slot];
            let col = cr.xyz;
            if (!b0.done) {
                blend(&b0, pix0, a, bb, col, e);
            }
            if (!b1.done) {
                blend(&b1, pix1, a, bb, col, e);
            }
        }
    }
    if (in0) {
        let p = py0 * frame.size.x + px;
        out_color[p] = vec4<f32>(b0.color + b0.t * frame.background.xyz, b0.depth);
        out_state[p] = vec2<u32>(bitcast<u32>(b0.t), b0.last);
    }
    if (in1) {
        let p = py1 * frame.size.x + px;
        out_color[p] = vec4<f32>(b1.color + b1.t * frame.background.xyz, b1.depth);
        out_state[p] = vec2<u32>(bitcast<u32>(b1.t), b1.last);
    }
}
