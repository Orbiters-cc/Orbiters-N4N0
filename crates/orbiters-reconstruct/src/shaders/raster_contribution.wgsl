// Contribution (crate::gpu::Gpu::contribution, on 16×16 tiles): the forward pass's walk, adding up for every Gaussian
// the blending weights α·T it reaches over the scene pixels (loss mask > 0.5) of this view: LightGaussian's global
// significance, about what leaving the Gaussian out takes from the image (the largest weight alone, the PLY's seen
// statistics, says nothing of how many pixels it covers). The sums are fixed point, units of 1/1024 (a 1920 × 1080 view
// whose every pixel gave one Gaussian its whole weight stays under 2³²), so they do not depend on the order of the adds;
// each subgroup adds its lanes first: one atomic add a subgroup and entry. `fold` (contribution_fold.wgsl) then adds
// the view's sums to the run's and clears them for the next view.

@group(0) @binding(8) var<storage, read> mask: array<f32>;
@group(0) @binding(9) var<storage, read_write> view_sum: array<atomic<u32>>;

const UNITS: f32 = 1024.0;

var<workgroup> done_count: atomic<u32>;

@diagnostic(off, subgroup_uniformity)
@compute @workgroup_size(256)
fn contribution(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) li: u32, @builtin(subgroup_invocation_id) lane: u32) {
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
        // Every lane reaches this for every entry (the loop is the workgroup's); the subgroup's first lane adds.
        let total = subgroupAdd(w);
        if (lane == 0u && total > 0.0) {
            atomicAdd(&view_sum[bitcast<u32>(bb.w)], u32(total * UNITS + 0.5));
        }
    }
}
