// The full-screen triangle every pass but Aero Shards' shards draws.
struct Full {
  @builtin(position) position: vec4f,
  // As WebGL's: the origin at the bottom left.
  @location(0) uv: vec2f,
}

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> Full {
  let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
  var out: Full;
  out.position = vec4f(p * 2.0 - 1.0, 0.0, 1.0);
  out.uv = p;
  return out;
}
