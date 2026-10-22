// The sky map's gradient from a colour step (crate::sky::SkyMap; a sums_*.wgsl variant, whose `add` writes the
// map's gradient buffer at binding 7, and sky_lookup.wgsl come first): ∂L/∂S = T·∂L/∂C of every pixel, added into the
// four texels it was looked up from with their bilinear weights, four floats a texel (rgb, unused).

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(5) var<storage, read> out_state: array<vec2<u32>>;
@group(0) @binding(6) var<storage, read> d_pixel: array<vec4<f32>>;

// sin(−6°): the lowest elevation whose texels learn (crate::sky's completion backs cells from −6° up too).
const SKY_LOWEST: f32 = -0.1045;
// The transmittance a pixel needs behind the splats to teach the sky.
const SKY_OPEN: f32 = 0.5;

@compute @workgroup_size(256)
fn gradient(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let p = flat_index(gid, nwg);
    if (p >= frame.size.x * frame.size.y) {
        return;
    }
    // Only pixels the splats leave mostly open teach the sky: behind nearly opaque ones the gradient is tiny, but Adam
    // takes full steps on it, and the texels drifted to the colours of what stands in front (or out of 0..1).
    let t = bitcast<f32>(out_state[p].x);
    if (t < SKY_OPEN) {
        return;
    }
    let g = d_pixel[p].xyz * t;
    if (g.x == 0.0 && g.y == 0.0 && g.z == 0.0) {
        return;
    }
    // Only the sky learns: below the horizon the map shows through gaps in the ground's splats, and would learn the
    // ground's colour there (round 2's cubemap learnt near content this way).
    let d = sky_dir(f32(p % frame.size.x), f32(p / frame.size.x));
    if (d.y < SKY_LOWEST) {
        return;
    }
    let k = sky_taps(d, u32(frame.horizon.w));
    for (var j = 0u; j < 4u; j++) {
        let w = k.weight[j];
        if (w > 0.0) {
            add(k.index[j] * 4u, w * g.x);
            add(k.index[j] * 4u + 1u, w * g.y);
            add(k.index[j] * 4u + 2u, w * g.z);
        }
    }
}
