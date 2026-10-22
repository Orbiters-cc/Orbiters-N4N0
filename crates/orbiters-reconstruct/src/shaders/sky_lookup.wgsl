// The sky map behind the splats (crate::sky::SkyMap): an equirectangular map of frame.horizon.w × half as many
// texels (rgb, unused) looked up by world direction, bilinear, around in longitude and clamped at the poles. Width 0:
// the constant background. Needs `frame` (common.wgsl).

@group(0) @binding(8) var<storage, read> sky: array<vec4<f32>>;

struct SkyTaps {
    index: vec4<u32>,
    weight: vec4<f32>,
}

// World direction through pixel (x, y) of this frame's camera, pixels as the rasteriser evaluates them.
fn sky_dir(x: f32, y: f32) -> vec3<f32> {
    let c = vec3<f32>((x - frame.focal.z) / frame.focal.x, (y - frame.focal.w) / frame.focal.y, 1.0);
    return normalize(frame.r0.xyz * c.x + frame.r1.xyz * c.y + frame.r2.xyz * c.z);
}

// The four texels around direction `d` on a map `w` texels across, with their bilinear weights (crate::sky::taps).
fn sky_taps(d: vec3<f32>, w: u32) -> SkyTaps {
    let h = w / 2u;
    let u = (atan2(d.x, d.z) / 6.283185307 + 0.5) * f32(w) - 0.5;
    let v = (0.5 - asin(clamp(d.y, -1.0, 1.0)) / 3.141592654) * f32(h) - 0.5;
    let u0 = floor(u);
    let v0 = floor(v);
    let fu = u - u0;
    let fv = v - v0;
    let iu0 = u32((i32(u0) % i32(w) + i32(w)) % i32(w));
    let iu1 = (iu0 + 1u) % w;
    let iv0 = u32(clamp(i32(v0), 0, i32(h) - 1));
    let iv1 = u32(clamp(i32(v0) + 1, 0, i32(h) - 1));
    var k: SkyTaps;
    k.index = vec4<u32>(iv0 * w + iu0, iv0 * w + iu1, iv1 * w + iu0, iv1 * w + iu1);
    k.weight = vec4<f32>((1.0 - fu) * (1.0 - fv), fu * (1.0 - fv), (1.0 - fu) * fv, fu * fv);
    return k;
}

// The colour behind the splats at pixel (x, y): the sky map's, else the constant background.
fn background_at(x: f32, y: f32) -> vec3<f32> {
    let w = u32(frame.horizon.w);
    if (w == 0u) {
        return frame.background.xyz;
    }
    let k = sky_taps(sky_dir(x, y), w);
    return sky[k.index.x].xyz * k.weight.x + sky[k.index.y].xyz * k.weight.y + sky[k.index.z].xyz * k.weight.z + sky[k.index.w].xyz * k.weight.w;
}
