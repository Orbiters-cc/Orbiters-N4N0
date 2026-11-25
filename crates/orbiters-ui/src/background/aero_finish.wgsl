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
@group(0) @binding(1) var bloomTexture: texture_2d<f32>;
@group(0) @binding(2) var linearSampler: sampler;
@group(0) @binding(3) var<uniform> post: PostParams;

fn hash12(value: vec2f) -> f32 {
  let p = fract(value * vec2f(0.1031, 0.1030));
  let mixed = p + dot(p, p.yx + 33.33);
  return fract((mixed.x + mixed.y) * mixed.x);
}

@fragment
fn fs_main(@location(0) uv: vec2f, @builtin(position) pixel: vec4f) -> @location(0) vec4f {
  let background = post.background.rgb;
  // Scene and output have identical dimensions; never filter the sharp base image.
  var scene = textureLoad(sceneTexture, vec2i(pixel.xy), 0).rgb;

  if (post.finishing.z > 0.000001) {
    let aspect = post.viewport.x / max(post.viewport.y, 1.0);
    let centered = (uv - vec2f(0.5)) * vec2f(aspect, 1.0);
    let radius = clamp(length(centered) / 0.78, 0.0, 1.0);
    let radialDirection = centered / max(length(centered), 0.0001);
    let minResolution = min(post.viewport.x, post.viewport.y);
    let pixelOffset = radialDirection * (post.finishing.z * minResolution * radius * radius);
    let uvOffset = pixelOffset * post.viewport.zw;
    let positive = textureSampleLevel(sceneTexture, linearSampler, uv + uvOffset, 0.0).rgb;
    let negative = textureSampleLevel(sceneTexture, linearSampler, uv - uvOffset, 0.0).rgb;
    scene = vec3f(positive.r, scene.g, negative.b);
  }

  var foreground = scene - background;
  if (post.finishing.x > 0.0001) {
    let bloom = textureSampleLevel(bloomTexture, linearSampler, uv, 0.0);
    // A colored haze remains visible on white; protect the opaque facet colors underneath.
    let haloMask = 1.0 - smoothstep(0.04, 0.4, length(foreground));
    let haloOpacity = min(bloom.a * post.finishing.x * 1.8, 0.65) * haloMask;
    let haloColor = bloom.rgb / max(bloom.a, 0.00001);
    let lightForeground = mix(foreground, haloColor - background, haloOpacity);
    foreground = mix(foreground + bloom.rgb * post.finishing.x, lightForeground, post.finishing.w);
  }

  let signal = smoothstep(0.008, 0.18, length(foreground));
  if (post.finishing.y > 0.0001) {
    let grainSeed = floor(post.temporal.x * 60.0);
    let noise = hash12(floor(pixel.xy) + vec2f(grainSeed, grainSeed * 1.6180339)) - 0.5;
    foreground += vec3f(noise * post.finishing.y * signal);
  }

  return vec4f(clamp(background + foreground, vec3f(0.0), vec3f(1.0)), 1.0);
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
