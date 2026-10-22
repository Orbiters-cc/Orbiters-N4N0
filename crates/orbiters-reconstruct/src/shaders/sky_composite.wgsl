// The sky map in place of the constant background, after the forward pass (crate::sky::SkyMap; sky_lookup.wgsl
// comes first): C += T·(S(d) − b) at every pixel, T the transmittance left behind the splats.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(4) var<storage, read_write> out_color: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read> out_state: array<vec2<u32>>;

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
