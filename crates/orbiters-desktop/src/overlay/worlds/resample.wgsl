// A wide view's pixels into a cube face's texels it covers: each texel's homography into the wide view (both are
// projections from the same eye), sampled bilinearly. Texels it does not cover keep what they had. Or, `origin.w` set, a
// face's own picture texel for texel.
//
// With a capture standing small in a panel's hole (`eye.w` set), our backdrop under what the splats leave, where the
// texel's ray goes through the hole (opening.rs, whose `Opening::colour` is this, for the tests): the hole reads as a
// window onto a small place, not as a hole onto VRChat's menu. Where the texel's ray does not go through it, nothing: the
// capture behind the sheet showed past its edge when seen from aside.

struct Params {
    h0: vec4<f32>,
    h1: vec4<f32>,
    h2: vec4<f32>,
    // The face's width per eye, the wide view's width and height, the eye half's first column.
    size: vec4<u32>,
    // The texel's ray from the eye: ray_x * x + ray_y * y + ray_o.
    ray_x: vec4<f32>,
    ray_y: vec4<f32>,
    ray_o: vec4<f32>,
    // The eye; w: 1 with a backdrop.
    eye: vec4<f32>,
    // The hole's middle; w: 1 for a face's own picture, texel for texel.
    origin: vec4<f32>,
    // The hole's axes (across, up, towards who looks); w: its half width, half height, the backdrop's depth behind it.
    ex: vec4<f32>,
    ey: vec4<f32>,
    ez: vec4<f32>,
    // Where the backdrop is drawn, from the hole's middle: min x, min y, max x, max y.
    cover: vec4<f32>,
    // Its colours; top.w: 1 for one flat colour; horizon.w: how much wider the back is than the hole, as seen.
    top: vec4<f32>,
    horizon: vec4<f32>,
    glow: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var wide: texture_2d<f32>;
@group(0) @binding(2) var wide_sampler: sampler;
@group(0) @binding(3) var face: texture_storage_2d<rgba8unorm, write>;

fn ease(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp((x - a) / (b - a), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn dither(x: u32, y: u32) -> f32 {
    var h = x * 1973u + y * 9277u + 89u;
    h = (h ^ (h >> 13u)) * 1274126177u;
    h = h ^ (h >> 16u);
    return (f32(h & 255u) / 255.0 - 0.5) / 255.0;
}

// The backdrop seen along `dir` from the eye at texel `t`; alpha -1 where it does not cover.
fn backdrop(dir: vec3<f32>, t: vec2<u32>) -> vec4<f32> {
    let eye = params.eye.xyz;
    let ez = params.ez.xyz;
    let dz = dot(dir, ez);
    let front = dot(params.origin.xyz - eye, ez);
    if (dz >= -1e-6 || front >= 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, -1.0);
    }
    let p = eye + dir * (front / dz) - params.origin.xyz;
    let u = dot(p, params.ex.xyz);
    let v = dot(p, params.ey.xyz);
    let c = params.cover;
    if (u < c.x || u > c.z || v < c.y || v > c.w) {
        return vec4<f32>(0.0, 0.0, 0.0, -1.0);
    }
    if (params.top.w > 0.5) {
        return vec4<f32>(params.horizon.xyz * 0.85, 1.0);
    }
    let q = eye + dir * ((front - params.ez.w) / dz) - params.origin.xyz;
    let k = params.horizon.w;
    let n = vec2<f32>(dot(q, params.ex.xyz) / (params.ex.w * k), dot(q, params.ey.xyz) / (params.ey.w * k));
    var base = mix(params.horizon.xyz, params.top.xyz, ease(-0.35, 1.0, n.y));
    base = mix(base, params.top.xyz, 0.5 * ease(-0.35, -1.3, n.y));
    let pool = exp(-(n.x * n.x / 0.8 + (n.y + 0.55) * (n.y + 0.55) / 0.08));
    base = base + params.glow.xyz * pool;
    let edge = min(params.ex.w - abs(u), params.ey.w - abs(v));
    let shade = 0.62 + 0.38 * ease(0.0, 0.035, edge);
    return vec4<f32>(base * shade + vec3<f32>(dither(t.x, t.y)), 1.0);
}

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = params.size.x;
    if (gid.x >= w || gid.y >= w) {
        return;
    }
    var c: vec4<f32>;
    if (params.origin.w > 0.5) {
        c = textureLoad(wide, vec2<i32>(i32(gid.x), i32(gid.y)), 0);
    } else {
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
        c = textureSampleLevel(wide, wide_sampler, (xy + vec2<f32>(0.5)) / size, 0.0);
    }
    if (params.eye.w > 0.5) {
        let dir = params.ray_x.xyz * f32(gid.x) + params.ray_y.xyz * f32(gid.y) + params.ray_o.xyz;
        let b = backdrop(dir, gid.xy);
        if (b.a >= 0.0) {
            c = vec4<f32>(c.rgb + (1.0 - c.a) * b.rgb, 1.0);
        } else {
            // Not through the hole: the capture is seen through its window only, never past the sheet's edge from aside.
            c = vec4<f32>(0.0);
        }
    }
    textureStore(face, vec2<i32>(i32(gid.x + params.size.w), i32(gid.y)), c);
}
