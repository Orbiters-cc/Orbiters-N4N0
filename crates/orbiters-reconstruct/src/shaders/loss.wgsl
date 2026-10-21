// Loss gradients per pixel (d_pixel: r, g, b, depth).
//   colour: masked L1 against the keyframe, weight frame.loss.x over frame.loss.w scene pixels;
//   depth:  relative L1 against exact depth samples (u, v, z, 0) at their nearest pixel, weight frame.loss.y over
//           frame.loss.w samples (added with float atomics: two samples may share a pixel).

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> out_color: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> target_rgb: array<u32>;
@group(0) @binding(3) var<storage, read> mask: array<f32>;
@group(0) @binding(4) var<storage, read_write> d_pixel: array<vec4<f32>>;

@compute @workgroup_size(256)
fn color(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let p = flat_index(gid, nwg);
    if (p >= frame.size.x * frame.size.y) {
        return;
    }
    let m = mask[p];
    let packed = target_rgb[p];
    let target_c = vec3<f32>(f32(packed & 255u), f32((packed >> 8u) & 255u), f32((packed >> 16u) & 255u)) / 255.0;
    let diff = out_color[p].xyz - target_c;
    let k = m * frame.loss.x / (3.0 * max(frame.loss.w, 1.0));
    d_pixel[p] = vec4<f32>(sign(diff) * k, 0.0);
}

var<workgroup> s_error: array<vec2<f32>, 256>;

// Scoring a render (crate::gpu::Gpu::score): per workgroup, the squared error of the clamped colour over the scene
// pixels and their channel count, into d_pixel[workgroup] (the CPU sums the workgroups).
@compute @workgroup_size(256)
fn squared_error(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let p = flat_index(gid, nwg);
    var e = vec2<f32>(0.0);
    if (p < frame.size.x * frame.size.y && mask[p] > 0.5) {
        let packed = target_rgb[p];
        let target_c = vec3<f32>(f32(packed & 255u), f32((packed >> 8u) & 255u), f32((packed >> 16u) & 255u)) / 255.0;
        let d = clamp(out_color[p].xyz, vec3<f32>(0.0), vec3<f32>(1.0)) - target_c;
        e = vec2<f32>(dot(d, d), 3.0);
    }
    s_error[li] = e;
    for (var stride = 128u; stride > 0u; stride >>= 1u) {
        workgroupBarrier();
        if (li < stride) {
            s_error[li] += s_error[li + stride];
        }
    }
    if (li == 0u) {
        d_pixel[wid.x + wid.y * nwg.x] = vec4<f32>(s_error[0], 0.0, 0.0);
    }
}
