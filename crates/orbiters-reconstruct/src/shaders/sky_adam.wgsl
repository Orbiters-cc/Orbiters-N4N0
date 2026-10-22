// The sky map's Adam step (crate::sky::SkyMap; a sums_read_*.wgsl variant, which reads the map's gradient sums at
// binding 3, comes first, after GRAD2D set to the four slots of a texel): only the texels this step's pixels looked up
// move. A texel whose gradient is zero keeps its colour and moments, as a view sees a sixth of the sky at most; one
// that moves has its sums cleared for the next step, so the whole map's gradients are never cleared.

struct SkyAdam {
    // Learning rate, β1, β2, ε.
    rate: vec4<f32>,
    // Texels, step (for bias correction), unused, unused.
    n: vec4<u32>,
}

@group(0) @binding(0) var<uniform> info: SkyAdam;
@group(0) @binding(1) var<storage, read_write> texels: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> moment1: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> moment2: array<vec4<f32>>;

@compute @workgroup_size(256)
fn adam(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let t = gid.x + gid.y * nwg.x * 256u;
    if (t >= info.n.x) {
        return;
    }
    let g = vec3<f32>(grad(4u * t), grad(4u * t + 1u), grad(4u * t + 2u));
    if (all(g == vec3<f32>(0.0))) {
        return;
    }
    clear_grads(4u * t);
    // A gradient that is no number (from the bits: WGSL may assume there are none) never reaches the moments.
    if (any((bitcast<vec3<u32>>(g) & vec3<u32>(0x7f800000u)) == vec3<u32>(0x7f800000u))) {
        return;
    }
    let b1 = info.rate.y;
    let b2 = info.rate.z;
    let m = b1 * moment1[t].xyz + (1.0 - b1) * g;
    let v = b2 * moment2[t].xyz + (1.0 - b2) * g * g;
    moment1[t] = vec4<f32>(m, 0.0);
    moment2[t] = vec4<f32>(v, 0.0);
    let step = max(f32(info.n.y), 1.0);
    let mh = m / (1.0 - pow(b1, step));
    let vh = v / (1.0 - pow(b2, step));
    texels[t] = vec4<f32>(texels[t].xyz - info.rate.x * mh / (sqrt(vh) + info.rate.w), 0.0);
}
