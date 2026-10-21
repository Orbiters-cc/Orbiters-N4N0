// Adam on every parameter, with one learning rate per slot of the 24-float layout (padding slots have 0).

struct AdamInfo {
    lr: array<vec4<f32>, 6>,
    // β1, β2, ε, unused.
    beta: vec4<f32>,
    // Parameters (floats), step (for bias correction), unused, unused.
    n: vec4<u32>,
}

@group(0) @binding(0) var<uniform> info: AdamInfo;
@group(0) @binding(1) var<storage, read_write> params: array<f32>;
@group(0) @binding(2) var<storage, read> grads: array<f32>;
@group(0) @binding(3) var<storage, read_write> moment1: array<f32>;
@group(0) @binding(4) var<storage, read_write> moment2: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let k = gid.x + gid.y * nwg.x * 256u;
    if (k >= info.n.x) {
        return;
    }
    let slot = k % 24u;
    let lr = info.lr[slot / 4u][slot % 4u];
    if (lr == 0.0) {
        return;
    }
    let g = grads[k];
    let b1 = info.beta.x;
    let b2 = info.beta.y;
    let m = b1 * moment1[k] + (1.0 - b1) * g;
    let v = b2 * moment2[k] + (1.0 - b2) * g * g;
    moment1[k] = m;
    moment2[k] = v;
    let t = f32(info.n.y);
    let mh = m / (1.0 - pow(b1, t));
    let vh = v / (1.0 - pow(b2, t));
    params[k] = params[k] - lr * mh / (sqrt(vh) + info.beta.z);
}
