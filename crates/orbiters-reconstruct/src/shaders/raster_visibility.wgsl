// Visibility (crate::gpu::Gpu::visibility): the forward pass's walk, recording for every Gaussian the largest
// blending weight α·T it reaches in a scene pixel (loss mask > 0.5) of this view. Weights are positive, so their bits
// order like the floats and an atomic max on u32 works. Splats that no training pixel ever sees (under the capture
// strip and band in every view, behind everything) keep 0.

@group(0) @binding(8) var<storage, read> mask: array<f32>;
@group(0) @binding(9) var<storage, read_write> seen: array<atomic<u32>>;

var<workgroup> done_count: atomic<u32>;

@compute @workgroup_size(16, 16)
fn visibility(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let tile = wid.x + wid.y * frame.size.z;
    let px = wid.x * TILE + lid.x;
    let py = wid.y * TILE + lid.y;
    let inside = px < frame.size.x && py < frame.size.y;
    let first = ranges[2u * tile];
    let end = ranges[2u * tile + 1u];
    if (li == 0u) {
        atomicStore(&done_count, 0u);
    }
    workgroupBarrier();
    var scene = false;
    if (inside) {
        scene = mask[py * frame.size.x + px] > 0.5;
    }
    var done = !inside;
    if (done) {
        atomicAdd(&done_count, 1u);
    }
    var t = 1.0;
    let pix = vec2<f32>(f32(px), f32(py));
    for (var e = first; e < end; e++) {
        let slot = (e - first) & 255u;
        if (slot == 0u) {
            if (workgroupUniformLoad(&done_count) == 256u) {
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
                    atomicAdd(&done_count, 1u);
                } else {
                    if (scene) {
                        atomicMax(&seen[bitcast<u32>(bb.w)], bitcast<u32>(alpha * t));
                    }
                    t = next;
                }
            }
        }
    }
}
