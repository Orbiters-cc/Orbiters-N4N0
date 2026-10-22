// The backward's gradient sums as f32 (sums_float.wgsl, sums_cas.wgsl), read once a step and cleared for the next one
// (no clear of the whole buffer each step).

@group(0) @binding(3) var<storage, read_write> grad2d: array<f32>;

fn grad(i: u32) -> f32 {
    return grad2d[i];
}

fn clear_grads(g: u32) {
    for (var k = 0u; k < GRAD2D; k++) {
        grad2d[g + k] = 0.0;
    }
}
