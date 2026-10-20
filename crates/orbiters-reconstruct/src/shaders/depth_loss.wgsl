// Depth loss of a depth view: relative L1 at each exact sample's nearest pixel (see loss.wgsl).

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> out_color: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> samples: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> d_pixel: array<atomic<u32>>;

fn add(i: u32, v: f32) {
    var old = atomicLoad(&d_pixel[i]);
    loop {
        let r = atomicCompareExchangeWeak(&d_pixel[i], old, bitcast<u32>(bitcast<f32>(old) + v));
        if (r.exchanged) {
            break;
        }
        old = r.old_value;
    }
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let s = flat_index(gid, nwg);
    if (s >= arrayLength(&samples)) {
        return;
    }
    let smp = samples[s];
    if (smp.z <= 0.0) {
        return;
    }
    let x = i32(round(smp.x));
    let y = i32(round(smp.y));
    if (x < 0 || y < 0 || x >= i32(frame.size.x) || y >= i32(frame.size.y)) {
        return;
    }
    let p = u32(y) * frame.size.x + u32(x);
    let d = out_color[p].w;
    add(p * 4u + 3u, sign(d - smp.z) / smp.z * frame.loss.y / max(frame.loss.w, 1.0));
}
