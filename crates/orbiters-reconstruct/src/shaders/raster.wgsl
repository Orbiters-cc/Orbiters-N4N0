// Tile rasterisation, shared by the forward and backward kernels (raster_forward.wgsl, raster_backward.wgsl). A
// workgroup is one 16×16 tile; its Gaussians (sorted near to far) come through shared memory 256 at a time.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> proj: array<f32>;
@group(0) @binding(2) var<storage, read> values: array<u32>;
@group(0) @binding(3) var<storage, read> ranges: array<u32>;
@group(0) @binding(5) var<storage, read_write> out_state: array<vec2<u32>>;

// Per batch entry: (mean.x, mean.y, conic.a, conic.b), (conic.c, opacity, depth, id bits), (r, g, b, 0).
var<workgroup> s_a: array<vec4<f32>, 256>;
var<workgroup> s_b: array<vec4<f32>, 256>;
var<workgroup> s_c: array<vec4<f32>, 256>;

fn load(slot: u32, entry: u32) {
    let id = values[entry];
    let o = id * PROJ;
    s_a[slot] = vec4<f32>(proj[o], proj[o + 1u], proj[o + 2u], proj[o + 3u]);
    s_b[slot] = vec4<f32>(proj[o + 4u], proj[o + 6u], proj[o + 5u], bitcast<f32>(id));
    s_c[slot] = vec4<f32>(proj[o + 7u], proj[o + 8u], proj[o + 9u], 0.0);
}
