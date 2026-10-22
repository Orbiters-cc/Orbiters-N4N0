// Tile rasterisation, shared by the forward and backward kernels (raster_forward.wgsl, raster_backward.wgsl). A
// workgroup is one 16×16 tile; its Gaussians (sorted near to far) come through shared memory 256 at a time.

@group(0) @binding(0) var<uniform> frame: Frame;
// The projected Gaussians (project.wgsl's 16 floats each) as four vec4s each: three 16-byte loads an entry instead of
// ten scalar ones.
@group(0) @binding(1) var<storage, read> proj: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> values: array<u32>;
@group(0) @binding(3) var<storage, read> ranges: array<u32>;
@group(0) @binding(5) var<storage, read_write> out_state: array<vec2<u32>>;

// Per batch entry: (mean.x, mean.y, conic.a, conic.b), (conic.c, opacity, depth, id bits), (r, g, b, 0).
var<workgroup> s_a: array<vec4<f32>, 256>;
var<workgroup> s_b: array<vec4<f32>, 256>;
var<workgroup> s_c: array<vec4<f32>, 256>;

fn load(slot: u32, entry: u32) {
    let id = values[entry];
    let o = id * (PROJ / 4u);
    // (mean.x, mean.y, conic.a, conic.b), (conic.c, depth, opacity, r), (g, b, radius, tile x0).
    let a = proj[o];
    let b = proj[o + 1u];
    let c = proj[o + 2u];
    s_a[slot] = a;
    s_b[slot] = vec4<f32>(b.x, b.z, b.y, bitcast<f32>(id));
    s_c[slot] = vec4<f32>(b.w, c.x, c.y, 0.0);
}
