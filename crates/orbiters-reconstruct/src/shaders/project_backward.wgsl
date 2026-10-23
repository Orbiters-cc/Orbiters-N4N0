// Projection backward (tests: crate::gpu::Gpu::backward_from and read_grads): each Gaussian's 2D gradients to gradients
// of its parameters (grads, laid out like params), with param_grads.wgsl's mathematics; a training step takes its Adam
// step in the same pass instead (project_backward_adam.wgsl).

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> params: array<f32>;
@group(0) @binding(2) var<storage, read> proj: array<f32>;
@group(0) @binding(4) var<storage, read_write> grads: array<f32>;
@group(0) @binding(5) var<storage, read_write> stats: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read> filter3d: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= frame.counts.x) {
        return;
    }
    let o = i * PARAM;
    for (var k = 0u; k < PARAM; k++) {
        grads[o + k] = 0.0;
    }
    // The regularisers (frame.reg.x · mean opacity, frame.reg.y · mean scale per metre) pull on the Gaussians this view
    // shows, and with frame.counts.w on every one, as 3DGS-MCMC's do (crate::train::Settings::regularise_unseen).
    if (proj[i * PROJ + 15u] == 0.0) {
        if (frame.counts.w != 0u) {
            let op = sigmoid(params[o + 10u]);
            let s = exp(vec3<f32>(params[o + 3u], params[o + 4u], params[o + 5u]));
            grads[o + 10u] = frame.reg.x * op * (1.0 - op);
            grads[o + 3u] = frame.reg.y * s.x;
            grads[o + 4u] = frame.reg.y * s.y;
            grads[o + 5u] = frame.reg.y * s.z;
        }
        return;
    }
    let mean = vec3<f32>(params[o], params[o + 1u], params[o + 2u]);
    let log_scale = vec3<f32>(params[o + 3u], params[o + 4u], params[o + 5u]);
    let q = vec4<f32>(params[o + 6u], params[o + 7u], params[o + 8u], params[o + 9u]);
    let g = param_grads(i, mean, log_scale, q, params[o + 10u], filter3d[i]);
    grads[o] = g.mean.x;
    grads[o + 1u] = g.mean.y;
    grads[o + 2u] = g.mean.z;
    grads[o + 3u] = g.scale.x;
    grads[o + 4u] = g.scale.y;
    grads[o + 5u] = g.scale.z;
    grads[o + 6u] = g.rotation.x;
    grads[o + 7u] = g.rotation.y;
    grads[o + 8u] = g.rotation.z;
    grads[o + 9u] = g.rotation.w;
    grads[o + 10u] = g.opacity;
    grads[o + 11u] = g.color.x;
    grads[o + 12u] = g.color.y;
    grads[o + 13u] = g.color.z;
}
