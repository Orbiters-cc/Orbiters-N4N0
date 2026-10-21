// The backward's gradient sums as 64-bit fixed point in units of 2^-40 (sums_fixed.wgsl).

@group(0) @binding(3) var<storage, read> grad2d: array<i64>;

fn grad(i: u32) -> f32 {
    return f32(grad2d[i]) * (1.0 / 1099511627776.0);
}
