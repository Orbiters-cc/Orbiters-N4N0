// The backward's gradient sums as f32 (sums_float.wgsl, sums_cas.wgsl).

@group(0) @binding(3) var<storage, read> grad2d: array<f32>;

fn grad(i: u32) -> f32 {
    return grad2d[i];
}
