struct FieldOut {
  @location(0) color: vec4f,
  @location(1) aux: vec4f,
}

fn hash22(p: vec2f) -> vec2f {
  var q = fract(vec3f(p.xyx) * vec3f(0.1031, 0.103, 0.0973));
  q += dot(q, q.yzx + 33.33);
  return fract((q.xx + q.yz) * q.zy) * 2.0 - 1.0;
}

fn noise(p: vec2f) -> f32 {
  let i = floor(p);
  let f = fract(p);
  let w = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
  let a = dot(hash22(i), f);
  let b = dot(hash22(i + vec2f(1.0, 0.0)), f - vec2f(1.0, 0.0));
  let c = dot(hash22(i + vec2f(0.0, 1.0)), f - vec2f(0.0, 1.0));
  let d = dot(hash22(i + vec2f(1.0, 1.0)), f - vec2f(1.0, 1.0));
  return 0.5 + 0.5 * mix(mix(a, b, w.x), mix(c, d, w.x), w.y);
}

fn lab_to_rgb(lab: vec3f) -> vec3f {
  var l = lab.x + 0.3963377774 * lab.y + 0.2158037573 * lab.z;
  var m = lab.x - 0.1055613458 * lab.y - 0.0638541728 * lab.z;
  var s = lab.x - 0.0894841775 * lab.y - 1.2914855480 * lab.z;
  l = l * l * l;
  m = m * m * m;
  s = s * s * s;
  return vec3f(
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s
  );
}

fn encode(x: vec3f) -> vec3f {
  let c = clamp(x, vec3f(0.0), vec3f(1.0));
  return mix(c * 12.92, 1.055 * pow(c, vec3f(1.0 / 2.4)) - 0.055, step(vec3f(0.0031308), c));
}

// The shape's two tones and how they mix, then how late it is revealed.
fn horizon(p: vec2f, time: f32) -> vec4f {
  let span = 0.5 * max(u.look.w, 1.0);
  let x = p.x / span;
  let arch = -0.17 - 0.2 * x * x + 0.02 * sin(time * 0.6);
  let soft = mix(0.05, 0.22, u.field.x);
  let edge = p.y - arch;
  let rim = 0.38 * smoothstep(-soft, 0.42, edge);
  let sun = vec2f(0.08 * sin(time * 0.31) * span, 0.58);
  let spread = length((p - sun) / vec2f(0.95 * span, 0.75));
  let sky = mix(0.55, 1.0, smoothstep(0.2, 0.95, spread));
  return vec4f(rim, sky, smoothstep(0.18, 0.62, edge), clamp(edge * 1.3, 0.0, 1.0));
}

fn tone(t: f32) -> vec4f {
  let x = smoothstep(0.3, 1.0, t) * 3.0;
  let index = i32(floor(x));
  return vec4f(mix(u.ramp[min(index, 3)].rgb, u.ramp[min(index + 1, 3)].rgb, fract(x)), smoothstep(0.0, 0.38, t));
}

@fragment
fn fs_main(in: Full) -> FieldOut {
  let uv = vec2f(in.uv.x, in.uv.y);
  let p = (uv - 0.5) * vec2f(u.look.w, 1.0);
  let time = u.frame.z;
  var q = p;
  let warp = vec2f(noise(q * 1.6 + vec2f(0.0, time * 0.3)), noise(q * 1.6 + vec2f(4.3, -time * 0.24))) - 0.5;
  q += warp * u.field.y * 0.18;
  let shape = horizon(q, time);

  var reveal = clamp((u.field.z * 1.45 - shape.w) / 0.45, 0.0, 1.0);
  reveal = reveal * reveal * (3.0 - 2.0 * reveal);

  let t = clamp(shape.xy * mix(0.7, 1.0, reveal), vec2f(0.0), vec2f(1.0));
  let mixed = mix(tone(t.x), tone(t.y), shape.z);
  let color = encode(lab_to_rgb(mixed.rgb));
  let alpha = mixed.a * reveal;
  let dither = (hash12(in.position.xy + u.field.w * 13.7) - 0.5) / 255.0;
  var out: FieldOut;
  out.color = clamp(vec4f(color * alpha + dither, alpha + dither), vec4f(0.0), vec4f(1.0));
  out.aux = vec4f(reveal, 0.0, 0.0, 1.0);
  return out;
}
