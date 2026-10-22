// Mip-Splatting's 3D filter (cpu::FILTER_3D): per Gaussian the variance scale / ν̂², ν̂ = focal / depth the highest
// sampling rate among the training cameras that hold its centre beyond the near plane, in their view widened by 15 %
// past each edge (Mip-Splatting's margin); 0 for a Gaussian none of them sees.

struct Info {
    // Gaussians, cameras, the filter's scale, the near plane.
    n: u32,
    cameras: u32,
    scale: f32,
    near: f32,
}

struct View {
    // World-to-camera rows (xyz) and translation (w).
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
    // fx, fy, cx, cy.
    focal: vec4<f32>,
    // Width, height (pixels), unused.
    size: vec4<f32>,
}

const PARAM: u32 = 16u;

@group(0) @binding(0) var<uniform> info: Info;
@group(0) @binding(1) var<storage, read> params: array<f32>;
@group(0) @binding(2) var<storage, read> views: array<View>;
@group(0) @binding(3) var<storage, read_write> filter3d: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = gid.x + gid.y * nwg.x * 256u;
    if (i >= info.n) {
        return;
    }
    let b = i * PARAM;
    let p = vec3<f32>(params[b], params[b + 1u], params[b + 2u]);
    var rate = 0.0;
    for (var c = 0u; c < info.cameras; c++) {
        let v = views[c];
        let z = dot(v.r2.xyz, p) + v.r2.w;
        if (z <= info.near) {
            continue;
        }
        let x = (dot(v.r0.xyz, p) + v.r0.w) / z * v.focal.x + v.focal.z;
        let y = (dot(v.r1.xyz, p) + v.r1.w) / z * v.focal.y + v.focal.w;
        if (x >= -0.15 * v.size.x && x <= 1.15 * v.size.x && y >= -0.15 * v.size.y && y <= 1.15 * v.size.y) {
            rate = max(rate, max(v.focal.x, v.focal.y) / z);
        }
    }
    filter3d[i] = select(0.0, info.scale / (rate * rate), rate > 0.0);
}
