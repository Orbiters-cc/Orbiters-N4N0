// What a training keyframe saw of each Gaussian (crate::gpu::Gpu::seen), after its visibility round left the largest
// blending weight α·T each reached in a scene pixel in `view`: folds it into the run's largest weight, and where it
// reaches the threshold counts the keyframe, sets the compass sector it looked from (the direction from the Gaussian
// towards the camera, 0 = +Z, clockwise towards +X) and keeps the finest sampling rate, focal length over depth.
// Clears `view` for the next keyframe.

struct Info {
    // Gaussians, the threshold, the focal length (the larger of fx and fy), unused.
    n: u32,
    threshold: f32,
    focal: f32,
    unused: f32,
    // The camera's position in the world.
    eye: vec4<f32>,
    // Third row of world-to-camera (xyz) and its translation (w): a point's depth.
    r2: vec4<f32>,
}

const PARAM: u32 = 16u;

@group(0) @binding(0) var<uniform> info: Info;
@group(0) @binding(1) var<storage, read> params: array<f32>;
@group(0) @binding(2) var<storage, read_write> view: array<u32>;
// Per Gaussian: largest weight (bits), keyframes, sector bits, finest rate (bits).
@group(0) @binding(3) var<storage, read_write> seen: array<vec4<u32>>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = gid.x + gid.y * nwg.x * 256u;
    if (i >= info.n) {
        return;
    }
    let bits = view[i];
    view[i] = 0u;
    var s = seen[i];
    // Positive weights: their bits order like the floats.
    s.x = max(s.x, bits);
    if (bitcast<f32>(bits) >= info.threshold) {
        let b = i * PARAM;
        let p = vec3<f32>(params[b], params[b + 1u], params[b + 2u]);
        let d = info.eye.xyz - p;
        let turn = atan2(d.x, d.z) / 6.283185307 + 1.0;
        let sector = min(u32(fract(turn) * 16.0), 15u);
        s.y += 1u;
        s.z |= 1u << sector;
        let z = dot(info.r2.xyz, p) + info.r2.w;
        if (z > 0.0) {
            s.w = max(s.w, bitcast<u32>(info.focal / z));
        }
    }
    seen[i] = s;
}
