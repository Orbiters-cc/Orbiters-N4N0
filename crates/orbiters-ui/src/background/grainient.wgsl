// React Bits, Copyright (c) 2026 David Haz. MIT + Commons Clause (assets/licenses/react-bits.txt).
// Grainient as orbiters.cc draws it (`vendor/Grainient.jsx`): the GLSL in WGSL, line for line. Never light.
struct Params {
  // Size in pixels, time, 0.
  frame: vec4f,
  // timeSpeed, colorBalance, warpStrength, warpFrequency.
  a: vec4f,
  // warpSpeed, warpAmplitude, blendAngle, blendSoftness.
  b: vec4f,
  // rotationAmount, noiseScale, grainAmount, grainScale.
  c: vec4f,
  // contrast, gamma, saturation, zoom.
  d: vec4f,
  color1: vec4f,
  color2: vec4f,
  color3: vec4f,
}
@group(0) @binding(0) var<uniform> u: Params;

// GLSL's smoothstep, which the scene also calls with its edges the other way round.
fn ss(a: f32, b: f32, x: f32) -> f32 {
  let t = clamp((x - a) / (b - a), 0.0, 1.0);
  return t * t * (3.0 - 2.0 * t);
}

fn rot(a: f32) -> mat2x2f {
  let s = sin(a);
  let c = cos(a);
  return mat2x2f(c, -s, s, c);
}

fn hash(q: vec2f) -> vec2f {
  let p = vec2f(dot(q, vec2f(2127.1, 81.17)), dot(q, vec2f(1269.5, 283.37)));
  return fract(sin(p) * 43758.5453);
}

fn noise(p: vec2f) -> f32 {
  let i = floor(p);
  let f = fract(p);
  let w = f * f * (3.0 - 2.0 * f);
  let n = mix(
    mix(dot(-1.0 + 2.0 * hash(i + vec2f(0.0, 0.0)), f - vec2f(0.0, 0.0)), dot(-1.0 + 2.0 * hash(i + vec2f(1.0, 0.0)), f - vec2f(1.0, 0.0)), w.x),
    mix(dot(-1.0 + 2.0 * hash(i + vec2f(0.0, 1.0)), f - vec2f(0.0, 1.0)), dot(-1.0 + 2.0 * hash(i + vec2f(1.0, 1.0)), f - vec2f(1.0, 1.0)), w.x),
    w.y
  );
  return 0.5 + 0.5 * n;
}

@fragment
fn fs_main(in: Full) -> @location(0) vec4f {
  let res = u.frame.xy;
  let t = u.frame.z * u.a.x;
  let frag = vec2f(in.position.x, res.y - in.position.y);
  let uv = frag / res;
  let ratio = res.x / res.y;
  var tuv = uv - 0.5;
  tuv /= max(u.d.w, 0.001);

  let degree = noise(vec2f(t * 0.1, tuv.x * tuv.y) * u.c.y);
  tuv.y *= 1.0 / ratio;
  tuv = tuv * rot(radians((degree - 0.5) * u.c.x + 180.0));
  tuv.y *= ratio;

  let frequency = u.a.w;
  let strength = max(u.a.z, 0.001);
  let amplitude = u.b.y / strength;
  let warp = t * u.b.x;
  tuv.x += sin(tuv.y * frequency + warp) / amplitude;
  tuv.y += sin(tuv.x * (frequency * 1.5) + warp) / (amplitude * 0.5);

  let b = u.a.y;
  let s = max(u.b.w, 0.0);
  let blend_x = (tuv * rot(radians(u.b.z))).x;
  let edge0 = -0.3 - b - s;
  let edge1 = 0.2 - b + s;
  let v0 = 0.5 - b + s;
  let v1 = -0.3 - b - s;
  let layer1 = mix(u.color3.rgb, u.color2.rgb, ss(edge0, edge1, blend_x));
  let layer2 = mix(u.color2.rgb, u.color1.rgb, ss(edge0, edge1, blend_x));
  var col = mix(layer1, layer2, ss(v0, v1, tuv.y));

  let grain_uv = uv * max(u.c.w, 0.001);
  let grain = fract(sin(dot(grain_uv, vec2f(12.9898, 78.233))) * 43758.5453);
  col += (grain - 0.5) * u.c.z;

  col = (col - 0.5) * u.d.x + 0.5;
  let luma = dot(col, vec3f(0.2126, 0.7152, 0.0722));
  col = mix(vec3f(luma), col, u.d.z);
  col = pow(max(col, vec3f(0.0)), vec3f(1.0 / max(u.d.y, 0.001)));
  return vec4f(clamp(col, vec3f(0.0), vec3f(1.0)), 1.0);
}
