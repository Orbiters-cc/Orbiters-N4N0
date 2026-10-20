// The last rendered image into an RGBA8 storage texture (colours as trained: encoded sRGB, over the background).

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> out_color: array<vec4<f32>>;
@group(0) @binding(2) var target_image: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= frame.size.x || gid.y >= frame.size.y) {
        return;
    }
    let c = out_color[gid.y * frame.size.x + gid.x].xyz;
    textureStore(target_image, vec2<i32>(gid.xy), vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}
