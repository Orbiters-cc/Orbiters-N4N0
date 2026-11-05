// Forward for training devices (lab switch crate::train::Settings::nested_forward, crate::gpu::Gpu::nested_forward):
// raster_forward.wgsl's `forward` with the same entries in the same order, the same math and the same outputs, but its
// loop nested as CUDA rasterisers write it (the gpu-stacks experiment's CUDA C++ port): an outer loop over batches of
// TILE_PIXELS entries, which stops once every pixel of the tile is done and loads the next batch, and an inner loop over
// the batch from which each pixel leaves as soon as it is done. The single flattened loop of `forward` keeps the batch
// load's branch and a done pixel's empty iterations in the walk of every entry: on the RTX 4080, the CUDA C++ port
// written like `forward` ran 0.646 ms on plaza's 910k splats, nested like this 0.582 ms. Not in the viewers' module:
// `forward` stays flattened there because D3D's FXC took 40 s to compile the nested form.

@group(0) @binding(4) var<storage, read_write> out_color: array<vec4<f32>>;

var<workgroup> done_count: atomic<u32>;

@compute @workgroup_size(TILE, TILE)
fn forward_nested(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
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
    var done = !inside;
    if (done) {
        atomicAdd(&done_count, 1u);
    }
    var t = 1.0;
    var color = vec3<f32>(0.0);
    var depth = 0.0;
    var last = first;
    let pix = vec2<f32>(f32(px), f32(py));
    for (var b0 = first; b0 < end; b0 += TILE_PIXELS) {
        // Every pixel done: stop. The uniform load is also the barrier after which the previous batch may be replaced.
        if (workgroupUniformLoad(&done_count) == TILE_PIXELS) {
            break;
        }
        if (b0 + li < end) {
            load(li, b0 + li);
        }
        workgroupBarrier();
        let count = min(end - b0, TILE_PIXELS);
        for (var j = 0u; j < count; j++) {
            if (done) {
                break;
            }
            let a = s_a[j];
            let bb = s_b[j];
            let d = pix - a.xy;
            let power = -0.5 * (a.z * d.x * d.x + bb.x * d.y * d.y) - a.w * d.x * d.y;
            let alpha = min(MAX_ALPHA, bb.y * exp(min(power, 0.0)));
            if (power <= 0.0 && alpha >= MIN_ALPHA) {
                let next = t * (1.0 - alpha);
                if (next < MIN_T) {
                    done = true;
                    atomicAdd(&done_count, 1u);
                } else {
                    color += s_c[j].xyz * alpha * t;
                    depth += bb.z * alpha * t;
                    t = next;
                    last = b0 + j + 1u;
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
