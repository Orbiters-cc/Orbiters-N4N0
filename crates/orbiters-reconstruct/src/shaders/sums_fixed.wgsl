// Gradient sums as 64-bit fixed point in units of 2^-40, with native i64 atomic adds (gpu.rs GradientSums::FixedPoint:
// Vulkan VK_KHR_shader_atomic_int64, D3D12 SM 6.6 with DXC). Integer sums do not depend on the order of the adds.

@group(0) @binding(7) var<storage, read_write> grad2d: array<atomic<i64>>;

// The first ten lanes of a subgroup add one slot each.
const LANE_ADDS: bool = true;

fn add(i: u32, v: f32) {
    if (v != 0.0) {
        atomicAdd(&grad2d[i], i64(round(v * 1099511627776.0)));
    }
}
