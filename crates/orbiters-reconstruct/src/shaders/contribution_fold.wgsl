// After a contribution round (raster_contribution.wgsl, crate::gpu::Gpu::contribution): adds each Gaussian's sum for
// that view (fixed point, units of 1/1024) to the run's total and clears it for the next view.

@group(0) @binding(0) var<uniform> info: vec4<u32>;
@group(0) @binding(1) var<storage, read_write> view_sum: array<u32>;
@group(0) @binding(2) var<storage, read_write> total: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = gid.x + gid.y * nwg.x * 256u;
    if (i >= info.x) {
        return;
    }
    let v = view_sum[i];
    if (v != 0u) {
        total[i] += f32(v) / 1024.0;
        view_sum[i] = 0u;
    }
}
