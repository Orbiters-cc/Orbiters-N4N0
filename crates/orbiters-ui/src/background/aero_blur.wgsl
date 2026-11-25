// React Bits, Copyright (c) 2026 David Haz. MIT + Commons Clause (assets/licenses/react-bits.txt).
// Aero Shards as orbiters.cc draws it: the shaders of `vendor/AeroShards.jsx`, word for word.

struct BlurParams {
  direction: vec4f,
}

@group(0) @binding(0) var bloomTexture: texture_2d<f32>;
@group(0) @binding(1) var linearSampler: sampler;
@group(0) @binding(2) var<uniform> blur: BlurParams;

@fragment
fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
  let nearOffset = blur.direction.xy * 1.3846153846;
  let farOffset = blur.direction.xy * 3.2307692308;
  var color = textureSampleLevel(bloomTexture, linearSampler, uv, 0.0) * 0.2270270270;
  color += textureSampleLevel(bloomTexture, linearSampler, uv + nearOffset, 0.0) * 0.3162162162;
  color += textureSampleLevel(bloomTexture, linearSampler, uv - nearOffset, 0.0) * 0.3162162162;
  color += textureSampleLevel(bloomTexture, linearSampler, uv + farOffset, 0.0) * 0.0702702703;
  color += textureSampleLevel(bloomTexture, linearSampler, uv - farOffset, 0.0) * 0.0702702703;
  return color;
}

struct Full {
  @builtin(position) position: vec4f,
  @location(0) uv: vec2f,
}

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> Full {
  var pos = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
  var uv = array<vec2f, 3>(vec2f(0.0, 1.0), vec2f(2.0, 1.0), vec2f(0.0, -1.0));
  var out: Full;
  out.position = vec4f(pos[vi], 0.0, 1.0);
  out.uv = uv[vi];
  return out;
}
