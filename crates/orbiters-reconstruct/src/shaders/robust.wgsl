// Robust colour loss: pixels inside coherent blobs of large error (other players, animated posters, particles: what
// no static scene explains) leave the loss for this step. `residual` writes each scene pixel's mean absolute error
// over the channels; `inliers` keeps a pixel unless more than half of its 7×7 neighbourhood errs by more than the
// threshold (frame.loss.y, which colour steps do not use otherwise), so fine texture and edges still train.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> out_color: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> target_rgb: array<u32>;
@group(0) @binding(3) var<storage, read> mask: array<f32>;
@group(0) @binding(4) var<storage, read_write> resid: array<f32>;
@group(0) @binding(5) var<storage, read_write> kept: array<f32>;

@compute @workgroup_size(256)
fn residual(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let p = flat_index(gid, nwg);
    if (p >= frame.size.x * frame.size.y) {
        return;
    }
    let packed = target_rgb[p];
    let t = vec3<f32>(f32(packed & 255u), f32((packed >> 8u) & 255u), f32((packed >> 16u) & 255u)) / 255.0;
    let d = abs(out_color[p].xyz - t);
    resid[p] = select(0.0, (d.x + d.y + d.z) / 3.0, mask[p] > 0.5);
}

@compute @workgroup_size(16, 16)
fn inliers(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = i32(frame.size.x);
    let h = i32(frame.size.y);
    let x = i32(gid.x);
    let y = i32(gid.y);
    if (x >= w || y >= h) {
        return;
    }
    let threshold = frame.loss.y;
    var bad = 0u;
    var all = 0u;
    for (var dy = -3; dy <= 3; dy++) {
        for (var dx = -3; dx <= 3; dx++) {
            let xx = x + dx;
            let yy = y + dy;
            if (xx >= 0 && yy >= 0 && xx < w && yy < h) {
                let q = u32(yy * w + xx);
                if (mask[q] > 0.5) {
                    all += 1u;
                    if (resid[q] > threshold) {
                        bad += 1u;
                    }
                }
            }
        }
    }
    let p = u32(y * w + x);
    kept[p] = select(mask[p], 0.0, 2u * bad > all);
}
