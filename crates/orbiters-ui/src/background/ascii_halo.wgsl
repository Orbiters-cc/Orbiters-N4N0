@fragment
fn fs_main(in: Full) -> @location(0) vec4f {
  let size = floor(u.frame.xy / u.atlas.w + 0.5);
  return glyph_at(frag_of(in, size.y) * u.atlas.w);
}
