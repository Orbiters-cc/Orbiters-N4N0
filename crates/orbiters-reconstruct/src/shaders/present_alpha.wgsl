// present.wgsl with what the splats leave of the background as transparency: premultiplied colour, for a render over
// black with no sky map, so that a capture can stand small in a room.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> out_color: array<vec4<f32>>;
@group(0) @binding(2) var target_image: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var<storage, read> out_state: array<vec2<u32>>;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= frame.size.x || gid.y >= frame.size.y) {
        return;
    }
    let p = gid.y * frame.size.x + gid.x;
    let left = bitcast<f32>(out_state[p].x);
    let alpha = clamp(1.0 - left, 0.0, 1.0);
    textureStore(target_image, vec2<i32>(gid.xy), vec4<f32>(clamp(out_color[p].xyz, vec3<f32>(0.0), vec3<f32>(alpha)), alpha));
}
