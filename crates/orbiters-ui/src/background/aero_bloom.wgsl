// React Bits, Copyright (c) 2026 David Haz. MIT + Commons Clause (assets/licenses/react-bits.txt).
// Aero Shards as orbiters.cc draws it: the shaders of `vendor/AeroShards.jsx`, word for word.

struct PostParams {
  viewport: vec4f,
  bloomInfo: vec4f,
  finishing: vec4f,
  background: vec4f,
  temporal: vec4f,
  tint: vec4f,
}

@group(0) @binding(0) var sceneTexture: texture_2d<f32>;
@group(0) @binding(1) var sceneSampler: sampler;
@group(0) @binding(2) var<uniform> post: PostParams;

fn visibleResidual(uv: vec2f) -> vec4f {
  let scene = textureSampleLevel(sceneTexture, sceneSampler, uv, 0.0).rgb;
  let residual = scene - post.background.rgb;
  let energy = dot(abs(residual), vec3f(0.2126, 0.7152, 0.0722));
  let threshold = post.bloomInfo.z;
  let knee = post.bloomInfo.w;
  let contribution = smoothstep(threshold - knee, threshold + knee, energy);
  let coverage = energy * contribution;
  // Store a premultiplied palette halo on light surfaces, never negative radiance.
  return vec4f(mix(residual * contribution, post.tint.rgb * coverage, post.finishing.w), coverage);
}

@fragment
fn fs_main(@location(0) uv: vec2f) -> @location(0) vec4f {
  let offset = post.bloomInfo.xy * 0.25;
  var glow = visibleResidual(uv + offset);
  glow += visibleResidual(uv - offset);
  glow += visibleResidual(uv + vec2f(offset.x, -offset.y));
  glow += visibleResidual(uv + vec2f(-offset.x, offset.y));
  return glow * 0.25;
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
