// Visibility (crate::gpu::Gpu::visibility, on 16×16 tiles): the forward pass's walk, recording for every Gaussian the
// largest blending weight α·T it reaches in a scene pixel (loss mask > 0.5) of this view (`visibility`, for the PLY's
// seen statistics), or whether it reaches a threshold (`visibility_over`, for fading). Weights are positive, so their
// bits order like the floats and an atomic max on u32 works. Splats that no training pixel ever sees (under the capture
// strip and band in every view, behind everything) keep 0.

@group(0) @binding(8) var<storage, read> mask: array<f32>;
@group(0) @binding(9) var<storage, read_write> seen: array<atomic<u32>>;

// The largest weights, taken over each subgroup first: one atomic maximum a subgroup and entry instead of one a pixel and
// entry.
var<workgroup> done_count: atomic<u32>;

@diagnostic(off, subgroup_uniformity)
@compute @workgroup_size(256)
fn visibility(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let tile = wid.x + wid.y * frame.size.z;
    let px = wid.x * TILE + li % TILE;
    let py = wid.y * TILE + li / TILE;
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
        let a = s_a[slot];
        let bb = s_b[slot];
        var w = 0.0;
        if (!done) {
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
                        w = alpha * t;
                    }
                    t = next;
                }
            }
        }
        // Every lane reaches this for every entry (the loop is the workgroup's).
        let most = subgroupMax(w);
        // The lane (lanes, on a tie) holding the subgroup's largest weight adds it.
        if (most > 0.0 && w == most) {
            atomicMax(&seen[bitcast<u32>(bb.w)], bitcast<u32>(most));
        }
    }
}

// What fading asks is only whether some scene pixel's weight reaches frame.far.z, so a Gaussian's slot takes the first
// such weight seen, by a plain store after a cached check, instead of an atomic maximum at every blend. Below the
// threshold everywhere: 0.
@group(0) @binding(10) var<storage, read_write> seen_over: array<u32>;

var<workgroup> done_count_over: atomic<u32>;

@compute @workgroup_size(16, 16)
fn visibility_over(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let tile = wid.x + wid.y * frame.size.z;
    let px = wid.x * TILE + lid.x;
    let py = wid.y * TILE + lid.y;
    let inside = px < frame.size.x && py < frame.size.y;
    let first = ranges[2u * tile];
    let end = ranges[2u * tile + 1u];
    if (li == 0u) {
        atomicStore(&done_count_over, 0u);
    }
    workgroupBarrier();
    var scene = false;
    if (inside) {
        scene = mask[py * frame.size.x + px] > 0.5;
    }
    var done = !inside;
    if (done) {
        atomicAdd(&done_count_over, 1u);
    }
    var t = 1.0;
    let pix = vec2<f32>(f32(px), f32(py));
    let threshold = frame.far.z;
    for (var e = first; e < end; e++) {
        let slot = (e - first) & 255u;
        if (slot == 0u) {
            if (workgroupUniformLoad(&done_count_over) == 256u) {
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
                    atomicAdd(&done_count_over, 1u);
                } else {
                    let w = alpha * t;
                    let id = bitcast<u32>(bb.w);
                    if (scene && w >= threshold && seen_over[id] == 0u) {
                        seen_over[id] = bitcast<u32>(w);
                    }
                    t = next;
                }
            }
        }
    }
}
