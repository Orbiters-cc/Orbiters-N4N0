@fragment
fn fs_main(in: Full) -> @location(0) vec4f {
  let frag = frag_of(in, u.frame.y);
  let base = textureSampleLevel(field, linear_clamp, at(in.uv), 0.0) * u.look.y;
  let glyph = glyph_at(frag);
  let glow = textureSampleLevel(halo, linear_clamp, at(in.uv), 0.0) * u.look.x;
  var color = base + glyph + glow;
  if (u.glyph.w > 0.5) {
    color = glyph + base * (1.0 - glyph.a);
    color += glow * 0.6 * (1.0 - color.a);
  }
  let grain = (hash12(frag + u.field.w * vec2f(17.0, 59.0)) + hash12(frag * 1.31 + u.field.w * 7.0) - 1.0) * u.look.z;
  let body = clamp(color.a, 0.0, 1.0);
  let a = body;
  let rgb = clamp(color.rgb + grain * (0.25 + body) * 0.5, vec3f(0.0), vec3f(a));
  // An opaque base lies under the premultiplied scene.
  return vec4f(rgb, a) + vec4f(u.base.rgb, 1.0) * (1.0 - a);
}
