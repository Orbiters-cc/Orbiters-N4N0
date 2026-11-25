// React Bits, Copyright (c) 2026 David Haz. MIT + Commons Clause (assets/licenses/react-bits.txt).
// Gradient ASCII as orbiters.cc draws it (`vendor/gradientAsciiShaders.js`, its horizon), in WGSL.
struct Ascii {
  // Size in pixels, time, glyph time.
  frame: vec4f,
  // The cell in pixels, its offset.
  cell: vec4f,
  // Atlas columns and rows, how many glyphs, the halo's downscale.
  atlas: vec4f,
  // softness, flow, intro, frame seed.
  field: vec4f,
  // density, glyph opacity, flicker, light.
  glyph: vec4f,
  // glow, gradient, grain, aspect.
  look: vec4f,
  base: vec4f,
  // Four stops in OKLab.
  ramp: array<vec4f, 4>,
}
@group(0) @binding(0) var<uniform> u: Ascii;

fn hash12(p: vec2f) -> f32 {
  var q = fract(vec3f(p.xyx) * 0.1031);
  q += dot(q, q.yzx + 33.33);
  return fract((q.x + q.y) * q.z);
}

// Where WebGL has this pixel: the origin at the bottom left.
fn frag_of(in: Full, height: f32) -> vec2f {
  return vec2f(in.position.x, height - in.position.y);
}
