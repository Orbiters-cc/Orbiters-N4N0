// The forward with each 32 threads on an 8×4 block of the tile instead of two rows (lab, S4_FORWARD_B): neighbouring
// pixels finish their walks together. The same blending, pixel by pixel.

var<workgroup> done_count_b: atomic<u32>;

@compute @workgroup_size(256)
fn forward_b(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let tile = wid.x + wid.y * frame.size.z;
    // Threads 32b..32b+31 hold the tile's 8×4 block b (two across, four down).
    let block = li / 32u;
    let lb = li % 32u;
    let px = wid.x * TILE + (block % 2u) * 8u + lb % 8u;
    let py = wid.y * TILE + (block / 2u) * 4u + lb / 8u;
    let inside = px < frame.size.x && py < frame.size.y;
    let first = ranges[2u * tile];
    let end = ranges[2u * tile + 1u];
    if (li == 0u) {
        atomicStore(&done_count_b, 0u);
    }
    workgroupBarrier();
    var done = !inside;
    if (done) {
        atomicAdd(&done_count_b, 1u);
    }
    var t = 1.0;
    var color = vec3<f32>(0.0);
    var depth = 0.0;
    var last = first;
    let pix = vec2<f32>(f32(px), f32(py));
    // One loop over the tile's entries, a new batch every 256 (shaders compilers may unroll a loop with a known small
    // bound: D3D's FXC took 40 s over the nested form). No early exits but the uniform one at a batch start.
    for (var e = first; e < end; e++) {
        let slot = (e - first) & 255u;
        if (slot == 0u) {
            // Every pixel done: stop. Else load the next batch (the uniform load is also the barrier that lets every
            // pixel finish the previous one).
            if (workgroupUniformLoad(&done_count_b) == 256u) {
                break;
            }
            if (e + li < end) {
                load(li, e + li);
            }
            workgroupBarrier();
        }
        if (!done) {
            let a = s_a[slot];
            let bb = s_b[slot];
            let d = pix - a.xy;
            let power = -0.5 * (a.z * d.x * d.x + bb.x * d.y * d.y) - a.w * d.x * d.y;
            let alpha = min(MAX_ALPHA, bb.y * exp(min(power, 0.0)));
            if (power <= 0.0 && alpha >= MIN_ALPHA) {
                let next = t * (1.0 - alpha);
                if (next < MIN_T) {
                    done = true;
                    atomicAdd(&done_count_b, 1u);
                } else {
                    color += s_c[slot].xyz * alpha * t;
                    depth += bb.z * alpha * t;
                    t = next;
                    last = e + 1u;
                }
            }
        }
    }
    if (inside) {
        let p = py * frame.size.x + px;
        out_color[p] = vec4<f32>(color + t * frame.background.xyz, depth);
        out_state[p] = vec2<u32>(bitcast<u32>(t), last);
    }
}
