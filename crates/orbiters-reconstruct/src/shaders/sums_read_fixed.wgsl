// The backward's gradient sums as 64-bit fixed point in units of 2^-40 (sums_fixed.wgsl), read once a step and cleared
// for the next one (no clear of the whole buffer each step).

@group(0) @binding(3) var<storage, read_write> grad2d: array<i64>;

fn grad(i: u32) -> f32 {
    return f32(grad2d[i]) * (1.0 / 1099511627776.0);
}

fn clear_grads(g: u32) {
    for (var k = 0u; k < GRAD2D; k++) {
        grad2d[g + k] = i64(0);
    }
}
