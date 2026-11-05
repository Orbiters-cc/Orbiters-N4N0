// A wide view's pixels into a cube face's texels it covers: each texel's homography into the wide view (both are
// projections from the same eye), sampled bilinearly. Texels it does not cover keep what they had.

struct Params {
    h0: vec4<f32>,
    h1: vec4<f32>,
    h2: vec4<f32>,
    // The face's width per eye, the wide view's width and height, the eye half's first column.
    size: vec4<u32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var wide: texture_2d<f32>;
@group(0) @binding(2) var wide_sampler: sampler;
@group(0) @binding(3) var face: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = params.size.x;
    if (gid.x >= w || gid.y >= w) {
        return;
    }
    let t = vec3<f32>(f32(gid.x), f32(gid.y), 1.0);
    let p = vec3<f32>(dot(params.h0.xyz, t), dot(params.h1.xyz, t), dot(params.h2.xyz, t));
    if (p.z <= 1e-6) {
        return;
    }
    let xy = p.xy / p.z;
    let size = vec2<f32>(f32(params.size.y), f32(params.size.z));
    // A pixel short of the edge: no filtering past it.
    if (xy.x < 0.5 || xy.y < 0.5 || xy.x > size.x - 1.5 || xy.y > size.y - 1.5) {
        return;
    }
    let c = textureSampleLevel(wide, wide_sampler, (xy + vec2<f32>(0.5)) / size, 0.0);
    textureStore(face, vec2<i32>(i32(gid.x + params.size.w), i32(gid.y)), vec4<f32>(c.rgb, 1.0));
}
