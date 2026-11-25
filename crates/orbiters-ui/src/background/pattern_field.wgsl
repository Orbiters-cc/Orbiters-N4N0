// React Bits, Copyright (c) 2026 David Haz. MIT + Commons Clause (assets/licenses/react-bits.txt).
// Pattern Waves as orbiters.cc draws it (`vendor/patternWavesShaders.js`, the silk preset without a fade): its first pass,
// a texel per dot, in WGSL. The texture's first row is the bottom one, as in WebGL: the cell is the pixel counted up.
struct Field {
  // Size in points, pixels per point, appear.
  size: vec4f,
  // Origin and pitch in pixels.
  grid: vec4f,
  // Heading, time, unit.
  wave: vec4f,
  // Light, amp.
  light: vec4f,
  // depth, shine, contrast, ink.
  tone: vec4f,
}
@group(0) @binding(0) var<uniform> u: Field;

const FOLDS = 5.5;

fn scramble(x: vec3u) -> vec3u {
  var v = x * 1664525u + 1013904223u;
  v.x += v.y * v.z;
  v.y += v.z * v.x;
  v.z += v.x * v.y;
  v ^= v >> vec3u(16u);
  v.x += v.y * v.z;
  v.y += v.z * v.x;
  v.z += v.x * v.y;
  return v;
}

fn lattice(corner: vec3f) -> vec3f {
  let h = scramble(vec3u(vec3i(corner) + 4096));
  return vec3f(h & vec3u(65535u)) / 32767.5 - 1.0;
}

fn gradient_noise(p: vec3f) -> f32 {
  let i = floor(p);
  let f = p - i;
  let w = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
  let n000 = dot(lattice(i), f);
  let n100 = dot(lattice(i + vec3f(1.0, 0.0, 0.0)), f - vec3f(1.0, 0.0, 0.0));
  let n010 = dot(lattice(i + vec3f(0.0, 1.0, 0.0)), f - vec3f(0.0, 1.0, 0.0));
  let n110 = dot(lattice(i + vec3f(1.0, 1.0, 0.0)), f - vec3f(1.0, 1.0, 0.0));
  let n001 = dot(lattice(i + vec3f(0.0, 0.0, 1.0)), f - vec3f(0.0, 0.0, 1.0));
  let n101 = dot(lattice(i + vec3f(1.0, 0.0, 1.0)), f - vec3f(1.0, 0.0, 1.0));
  let n011 = dot(lattice(i + vec3f(0.0, 1.0, 1.0)), f - vec3f(0.0, 1.0, 1.0));
  let n111 = dot(lattice(i + vec3f(1.0, 1.0, 1.0)), f - vec3f(1.0, 1.0, 1.0));
  return mix(
    mix(mix(n000, n100, w.x), mix(n010, n110, w.x), w.y),
    mix(mix(n001, n101, w.x), mix(n011, n111, w.x), w.y),
    w.z
  );
}

fn surface(p: vec2f, t: f32) -> f32 {
  let heading = u.wave.xy;
  let side = vec2f(-heading.y, heading.x);
  let a = dot(p, heading);
  let v = dot(p, side);
  let bend = gradient_noise(vec3f(v * 0.85, a * 0.3, t * 0.05)) * 1.7 + 0.4 * sin(v * 1.6 + t * 0.2);
  let phase = a * FOLDS + bend - t * 0.45;
  let swell = 0.6 + 0.4 * gradient_noise(vec3f(a * 0.55 + 3.0, v * 0.45, t * 0.04));
  let fold = sin(phase) + 0.32 * sin(2.0 * phase + 1.3);
  let ripple = 0.16 * sin(a * FOLDS * 2.5 + bend * 1.9 - t * 0.9 + 2.1);
  return (fold + ripple) * swell;
}

fn height_at(css: vec2f) -> f32 {
  return surface((css - 0.5 * u.size.xy) / u.wave.w, u.wave.z) * u.light.w;
}

@fragment
fn fs_main(in: Full) -> @location(0) vec4f {
  // The row's number is the website's `gl_FragCoord.y`: the target is never shown, so its rows may be counted from 0 up.
  let cell = floor(in.position.xy);
  let center = u.grid.xy + (cell + 0.5) * u.grid.zw;
  let css = center / u.size.z;
  let e = max(u.grid.w / u.size.z, 4.0);
  let h = height_at(css);
  let hx = (height_at(css + vec2f(e, 0.0)) - height_at(css - vec2f(e, 0.0))) / (2.0 * e);
  let hy = (height_at(css + vec2f(0.0, e)) - height_at(css - vec2f(0.0, e))) / (2.0 * e);
  let grad = vec2f(hx, hy) * u.wave.w * u.tone.x * 0.4;
  let n = normalize(vec3f(-grad, 1.0));

  let diffuse = clamp(dot(n, u.light.xyz), 0.0, 1.0);
  let halfway = normalize(u.light.xyz + vec3f(0.0, 0.0, 1.0));
  let spec = pow(clamp(dot(n, halfway), 0.0, 1.0), 160.0) * u.tone.y * 1.15;
  let hollow = 0.7 + 0.3 * clamp(h * 0.5 + 0.5, 0.0, 1.0);
  var tone = clamp(diffuse * hollow * 0.78 + spec, 0.0, 1.0);
  tone = clamp((tone - 0.42) * u.tone.z + 0.42, 0.0, 1.0);

  var level = pow(tone, 2.2);
  var emphasis = clamp(spec * 1.6, 0.0, 1.0);
  if (u.tone.w > 0.5) {
    level = pow(clamp(1.0 - tone / 0.46, 0.0, 1.0), 2.4) * 0.72;
    emphasis = smoothstep(0.55, 0.95, level);
  }

  let reach = length(css - 0.5 * u.size.xy) / max(0.5 * length(u.size.xy), 1.0);
  let appear = smoothstep(reach - 0.05, reach + 0.3, u.size.w * 1.35);

  let alpha = appear * (0.22 + 0.78 * level);
  let lift = clamp(h * u.tone.x * 0.42, -0.48, 0.48);
  return vec4f(level, alpha, emphasis, lift + 0.5);
}
