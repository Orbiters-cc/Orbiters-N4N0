// How orbiters.cc lays a creator's background behind a page (`personalization.css`, `.creator-background`): the scene over
// its palette's gradient, under a dark wash from a quarter at the top to three quarters at the bottom, the whole at the
// background's own opacity; then Orbiters - N4N0's veil, as dark as the words on the page need.
struct Over {
  // Size in points, the group's opacity, the veil's.
  frame: vec4f,
  // 1 when the target stores linear colour; pixels per point; how far the scene faded in; 1 for a picture of it alone,
  // which has no wash.
  extra: vec4f,
  a: vec4f,
  b: vec4f,
  c: vec4f,
  // The page's colour; the corners' radius in points.
  page: vec4f,
}
@group(0) @binding(0) var<uniform> u: Over;
@group(0) @binding(1) var scene: texture_2d<f32>;
@group(0) @binding(2) var linear_clamp: sampler;

fn srgb_to_linear(c: vec3f) -> vec3f {
  let lo = c / 12.92;
  let hi = pow((c + 0.055) / 1.055, vec3f(2.4));
  return select(hi, lo, c <= vec3f(0.04045));
}

@fragment
fn fs_main(in: Full) -> @location(0) vec4f {
  let uv = vec2f(in.uv.x, 1.0 - in.uv.y);
  let size = u.frame.xy;
  let p = uv * size;

  // linear-gradient(140deg, a, c)
  let angle = radians(140.0);
  let dir = vec2f(sin(angle), -cos(angle));
  let len = abs(size.x * dir.x) + abs(size.y * dir.y);
  let along = clamp(dot(p - 0.5 * size, dir) / len + 0.5, 0.0, 1.0);
  var col = mix(u.a.rgb, u.c.rgb, along);
  // radial-gradient(ellipse at 20% 20%, b, transparent 65%): to the farthest corner, in the farthest sides' ratio.
  let center = 0.2 * size;
  let far = 0.8 * size;
  let radius = far * 1.41421356;
  let t = clamp(length((p - center) / radius) / 0.65, 0.0, 1.0);
  // `transparent` is transparent black, and CSS mixes premultiplied: the colour stays b's, its alpha falls.
  col = mix(col, u.b.rgb, 1.0 - t);

  var s = vec4f(0.0);
  if (u.extra.w > 0.5) {
    // Painted smaller than it was drawn: the mean of what lies under the pixel.
    let step = 1.0 / (size * u.extra.y);
    for (var i = 0; i < 9; i++) {
      s += textureSampleLevel(scene, linear_clamp, uv + step * (vec2f(f32(i % 3), f32(i / 3)) - 1.0) / 3.0, 0.0) / 9.0;
    }
  } else {
    s = textureSampleLevel(scene, linear_clamp, uv, 0.0);
  }
  col = s.rgb * u.extra.z + col * (1.0 - s.a * u.extra.z);
  if (u.extra.w < 0.5) {
    col = mix(col, u.page.rgb, mix(0.25, 0.75, uv.y));
  }
  let k = u.frame.z;
  // The veil lies over the group and what is behind it, as one layer of black.
  var out = vec4f(col * k, k);
  out = vec4f(out.rgb * (1.0 - u.frame.w), out.a + u.frame.w * (1.0 - out.a));
  // Rounded corners (a picture of it in a card).
  let r = u.page.w;
  if (r > 0.5) {
    let d = abs(p - 0.5 * size) - (0.5 * size - r);
    out *= clamp(0.5 - (length(max(d, vec2f(0.0))) + min(max(d.x, d.y), 0.0) - r) * u.extra.y, 0.0, 1.0);
  }
  if (u.extra.x > 0.5) {
    out = vec4f(srgb_to_linear(out.rgb / max(out.a, 1e-4)) * out.a, out.a);
  }
  return out;
}
