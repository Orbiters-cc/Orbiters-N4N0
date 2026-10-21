// Sky cubemap passes (lab, crate::sky; sky_lookup.wgsl and a sums_*.wgsl variant come first):
//   composite: after the forward pass, the sky replaces the constant background behind the splats, C += T·(S(d) − b);
//   gradient:  ∂L/∂S = T·∂L/∂C of every pixel, added into the texels it was looked up from (bilinear weights), as four
//              floats per texel (rgb, unused) for Adam.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(4) var<storage, read_write> out_color: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read> out_state: array<vec2<u32>>;
@group(0) @binding(6) var<storage, read> d_pixel: array<vec4<f32>>;

@compute @workgroup_size(256)
fn composite(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let p = flat_index(gid, nwg);
    if (p >= frame.size.x * frame.size.y) {
        return;
    }
    let t = bitcast<f32>(out_state[p].x);
    let c = out_color[p];
    let behind = background_at(f32(p % frame.size.x), f32(p / frame.size.x));
    out_color[p] = vec4<f32>(c.xyz + t * (behind - frame.background.xyz), c.w);
}

@compute @workgroup_size(256)
fn gradient(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let p = flat_index(gid, nwg);
    if (p >= frame.size.x * frame.size.y) {
        return;
    }
    let g = d_pixel[p].xyz * bitcast<f32>(out_state[p].x);
    if (g.x == 0.0 && g.y == 0.0 && g.z == 0.0) {
        return;
    }
    let k = sky_taps(sky_dir(f32(p % frame.size.x), f32(p / frame.size.x)), frame.counts.y);
    for (var j = 0u; j < 4u; j++) {
        let w = k.weight[j];
        if (w > 0.0) {
            add(k.index[j] * 4u, w * g.x);
            add(k.index[j] * 4u + 1u, w * g.y);
            add(k.index[j] * 4u + 2u, w * g.z);
        }
    }
}
