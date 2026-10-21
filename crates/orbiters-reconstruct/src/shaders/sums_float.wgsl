// Gradient sums with native f32 atomic adds (gpu.rs GradientSums::Float: Vulkan VK_EXT_shader_atomic_float, Metal).

@group(0) @binding(7) var<storage, read_write> grad2d: array<atomic<f32>>;

// The first ten lanes of a subgroup add one slot each.
const LANE_ADDS: bool = true;

fn add(i: u32, v: f32) {
    if (v != 0.0) {
        atomicAdd(&grad2d[i], v);
    }
}
