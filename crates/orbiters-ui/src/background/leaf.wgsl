// Window Light, Orbiters' own (`leafShadow/leafShadowShader.js`), in WGSL line for line: window light on a dark wall,
// crossed by swaying leaf shadows. Every motion is a whole number of cycles per LOOP seconds.
struct Params {
  // Size in pixels, time, 0.
  frame: vec4f,
  region: vec4f,
  room: vec4f,
  light: vec4f,
  tint: vec4f,
}
@group(0) @binding(0) var<uniform> u: Params;
@group(0) @binding(1) var leaves_texture: texture_2d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;

const TAU = 6.28318530718;
const LOOP = 240.0;
const WINDOW_CENTER = vec2f(0.22, 0.08);
const WINDOW_HALF = vec2f(0.62, 0.35);
const CORNER = 0.06;
const TILT = 0.2618;
const ANCHOR = vec2f(1.05, 0.95);

fn wave(cycles: f32, offset: f32) -> f32 {
  return sin(u.frame.z / LOOP * TAU * cycles + offset);
}

fn hash(p: vec2f) -> f32 {
  var p3 = fract(vec3f(p.xyx) * 0.1031);
  p3 += dot(p3, p3.yzx + 33.33);
  return fract((p3.x + p3.y) * p3.z);
}

fn noise(p: vec2f) -> f32 {
  let i = floor(p);
  let f = fract(p);
  let w = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2f(1.0, 0.0)), w.x), mix(hash(i + vec2f(0.0, 1.0)), hash(i + vec2f(1.0, 1.0)), w.x), w.y);
}

fn loop_point(cycles: f32, radius: f32, offset: f32) -> vec2f {
  let a = u.frame.z / LOOP * TAU * cycles + offset;
  return vec2f(cos(a), sin(a)) * radius;
}

fn rotate_around(p: vec2f, pivot: vec2f, angle: f32) -> vec2f {
  let c = cos(angle);
  let s = sin(angle);
  let d = p - pivot;
  return pivot + vec2f(c * d.x - s * d.y, s * d.x + c * d.y);
}

fn leaf_layer(p: vec2f, angle: f32, flutter: f32, seed: f32, far: bool) -> f32 {
  var q = rotate_around(p, ANCHOR, angle);
  let drift = loop_point(29.0, 1.3, seed) + loop_point(47.0, 0.7, seed * 2.0);
  q += (vec2f(noise(q * 7.0 + drift), noise(q * 7.0 + drift.yx + 19.0)) - 0.5) * flutter;
  let uv = (q - u.region.xy) / u.region.z;
  let inside = step(0.0, uv.x) * step(uv.x, 1.0) * step(0.0, uv.y) * step(uv.y, 1.0);
  let c = clamp(uv, vec2f(0.0), vec2f(1.0));
  let texel = textureSampleLevel(leaves_texture, linear_clamp, vec2f(c.x, 1.0 - c.y), 0.0);
  return inside * select(texel.r, texel.g, far);
}

fn bird(p: vec2f, start: f32, duration: f32, start_at: vec2f, end_at: vec2f) -> f32 {
  let k = (u.frame.z - start) / duration;
  if (k < 0.0 || k > 1.0) {
    return 0.0;
  }
  let dir = normalize(end_at - start_at);
  var local = p - mix(start_at, end_at, k);
  local = vec2f(dot(local, dir), dot(local, vec2f(-dir.y, dir.x))) / 0.05;
  let flap = sin(u.frame.z * 7.5) * step(0.45, fract(k * 2.2));
  let along = abs(local.y);
  let centre = (0.35 * flap - 0.18) * along - 0.08 * along * along;
  let wing = (1.0 - smoothstep(0.85, 1.25, along)) * (1.0 - smoothstep(0.04, 0.42, abs(local.x - centre + 0.12 * along)));
  let body = 1.0 - smoothstep(0.18, 0.42, length(local * vec2f(0.55, 2.4)));
  let fade_edge = smoothstep(0.0, 0.08, k) * (1.0 - smoothstep(0.92, 1.0, k));
  return clamp(max(wing, body), 0.0, 1.0) * fade_edge;
}

@fragment
fn fs_main(in: Full) -> @location(0) vec4f {
  let res = u.frame.xy;
  let time = u.frame.z;
  let frag = vec2f(in.position.x, res.y - in.position.y);
  let unit = min(res.y, res.x / 1.3);
  let p = (frag - 0.5 * res) / unit;

  var q = p - WINDOW_CENTER;
  q = vec2f(cos(TILT) * q.x + sin(TILT) * q.y, -sin(TILT) * q.x + cos(TILT) * q.y);
  q /= 1.0 + 0.32 * q.x;
  let d = abs(q) - (WINDOW_HALF - CORNER);
  let sd = length(max(d, vec2f(0.0))) + min(max(d.x, d.y), 0.0) - CORNER;
  let window = 1.0 - smoothstep(-0.05, 0.05, sd);
  let mullion = 1.0 - 0.82 * (1.0 - smoothstep(0.0, 0.065, abs(q.x - 0.04)));
  let transom = 1.0 - 0.82 * (1.0 - smoothstep(0.0, 0.065, abs(q.y - 0.1)));
  let panes = window * mullion * transom;

  let gust = pow(0.5 + 0.5 * wave(3.0, 0.4), 6.0) * (0.55 + 0.45 * wave(5.0, 1.7));
  let sway = 0.6 * wave(23.0, 0.0) + 0.3 * wave(37.0, 1.3) + 0.15 * wave(61.0, 2.1);
  let angle = 0.016 * sway * (1.0 + 1.6 * gust) + 0.028 * gust;
  let lagged = 0.6 * wave(23.0, -0.9) + 0.4 * wave(31.0, 0.6);
  let near = leaf_layer(p, angle, 0.004 + 0.008 * gust, 0.0, false);
  let far = leaf_layer(p, angle * 0.7 + 0.008 * lagged, 0.003 + 0.005 * gust, 3.1, true);
  let leaves = 1.0 - (1.0 - near * 0.94) * (1.0 - far * 0.72);

  let cloud = 0.5 + 0.5 * (0.6 * wave(2.0, 0.3 + p.x * 0.5) + 0.4 * wave(7.0, 2.2 + p.x * 0.8));
  let dim = 1.0 - 0.4 * smoothstep(0.4, 0.95, cloud);

  let flight = max(
    bird(p, 70.0, 5.5, vec2f(0.95, -0.4), vec2f(-0.45, 0.6)),
    bird(p, 178.0, 6.0, vec2f(-0.6, -0.15), vec2f(0.95, 0.5))
  );

  let n = q / WINDOW_HALF;
  let lower = mix(0.72, 1.0, 1.0 - smoothstep(-0.1, 0.45, n.y));
  let core = exp(-dot(n * vec2f(0.9, 1.2), n * vec2f(0.9, 1.2)) * 1.1);
  let light = panes * lower * dim;
  let halo = (exp(-max(sd, 0.0) * 16.0) * 0.22 + exp(-max(sd, 0.0) * 5.0) * 0.05) * dim;

  let luma = dot(u.light.rgb, vec3f(0.299, 0.587, 0.114));
  let vivid = clamp(mix(vec3f(luma), u.light.rgb, 1.35), vec3f(0.0), vec3f(1.0));
  let bright = min(mix(u.light.rgb, vivid, 0.5) * 1.2, vec3f(1.0));
  let lit = mix(u.light.rgb, bright, core);

  let reach = 1.0 - smoothstep(0.0, 1.4, length(p - WINDOW_CENTER));
  var color = u.room.rgb * (0.55 + 0.45 * reach);
  let shade = mix(u.room.rgb, u.tint.rgb, 0.55);
  let open = (1.0 - leaves) * (1.0 - flight * 0.32);
  color += lit * light * open;
  color += shade * 0.6 * light * leaves;
  color += vivid * halo * (1.0 - window) * (1.0 - leaves * 0.6);

  let grain = hash(frag + floor(time * 12.0) * 17.0) - 0.5;
  color += grain * 0.022;
  return vec4f(clamp(color, vec3f(0.0), vec3f(1.0)), 1.0);
}
