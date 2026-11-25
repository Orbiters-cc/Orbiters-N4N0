// React Bits, Copyright (c) 2026 David Haz. MIT + Commons Clause (assets/licenses/react-bits.txt).
// Gradient Waves as orbiters.cc draws it (`vendor/GradientWaves.jsx`): the GLSL in WGSL, line for line, without the
// pointer (the website turns it off).
struct Params {
  // Size in pixels, time, steps.
  frame: vec4f,
  // speed, amplitude, waveScale, waveRatio.
  a: vec4f,
  // swell, turbulence, tilt, zoom.
  b: vec4f,
  // height, fogDepth, grainIntensity, 0.
  c: vec4f,
  horizon: vec4f,
  wave: vec4f,
  crest: vec4f,
}
@group(0) @binding(0) var<uniform> u: Params;

const MAX_DIST = 20000.0;

fn hash21(p: vec2f) -> f32 {
  var p3 = fract(vec3f(p.xyx) * 0.1031);
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}

fn plasma(r: vec3f, freq: vec2f, tc: vec4f) -> f32 {
  var mx = r.x + tc.x;
  mx += u.b.x * sin((r.y + mx) / 20.0 + tc.y);
  var my = r.y - tc.z;
  my += u.b.y * cos(r.x / 23.0 + tc.w);
  return r.z - (sin(mx * freq.x) * u.a.y + sin(my * freq.y) * u.a.y + u.c.x);
}

fn raymarch(pos: vec3f, dir: vec3f, freq: vec2f, tc: vec4f) -> f32 {
  var dist = 0.0;
  for (var i = 0; i < 128; i++) {
    if (f32(i) >= u.frame.w) {
      break;
    }
    let scene = plasma(pos + dist * dir, freq, tc);
    if (abs(scene) < 0.1) {
      break;
    }
    dist += 0.9 * scene;
    if (!(abs(dist) < MAX_DIST)) {
      return MAX_DIST;
    }
  }
  return dist;
}

@fragment
fn fs_main(in: Full) -> @location(0) vec4f {
  let res = u.frame.xy;
  let frag = vec2f(in.position.x, res.y - in.position.y);
  let time = u.frame.z * u.a.x;
  let freq = vec2f(u.a.z / 7.0, (u.a.z * u.a.w) / 3.0);
  let tc = vec4f(time / 0.130, time / 0.810, time / 0.200, time / 0.710);
  let vfov = (3.14159 / 2.3) / max(u.b.w, 0.05);
  let cam = vec3f(0.0, 0.0, 30.0);
  var uv = frag / res - 0.5;
  uv.x *= res.x / res.y;
  uv.y *= -1.0;

  var dir = vec3f(0.0, 0.0, -1.0);
  let ulen = length(uv);
  let xrot = vfov * ulen;
  var c = cos(xrot);
  var s = sin(xrot);
  dir = mat3x3f(1.0, 0.0, 0.0, 0.0, c, -s, 0.0, s, c) * dir;
  var nuv = vec2f(1.0, 0.0);
  if (ulen > 1e-5) {
    nuv = uv / ulen;
  }
  c = nuv.x;
  s = nuv.y;
  dir = mat3x3f(c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0) * dir;
  c = cos(u.b.z);
  s = sin(u.b.z);
  dir = mat3x3f(c, 0.0, s, 0.0, 1.0, 0.0, -s, 0.0, c) * dir;

  let dist = raymarch(cam, dir, freq, tc);
  let pos = cam + dist * dir;

  let t = clamp(u.c.y / max(dist, 0.001), 0.0, 1.0);
  let body = mix(u.wave.rgb, u.crest.rgb, clamp(pos.z * 0.08 + 0.5, 0.0, 1.0));
  let col = clamp(mix(u.horizon.rgb, body, t), vec3f(0.0), vec3f(1.0));

  var alpha = clamp(t, 0.0, 1.0);
  let g = hash21(frag + (u.frame.z - 64.0 * floor(u.frame.z / 64.0)) * 11.0);
  alpha += (g - 0.5) * u.c.z;
  alpha = clamp(alpha, 0.0, 1.0);
  return vec4f(col * alpha, alpha);
}
