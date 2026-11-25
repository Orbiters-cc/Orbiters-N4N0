// React Bits, Copyright (c) 2026 David Haz. MIT + Commons Clause (assets/licenses/react-bits.txt).
// Pattern Waves' second pass (`vendor/patternWavesShaders.js`), its dots.
struct Mark {
  // Origin and pitch in pixels.
  grid: vec4f,
  // Columns, rows, the dots' size, the height in pixels.
  cells: vec4f,
  color: vec4f,
  accent: vec4f,
  background: vec4f,
}
@group(0) @binding(0) var<uniform> u: Mark;
@group(0) @binding(1) var field: texture_2d<f32>;

fn coverage(local: vec2f, level: f32) -> f32 {
  let span = u.grid.w * u.cells.z;
  let area = max(sqrt(level), 0.14);
  return clamp(0.5 - (length(local) - 0.5 * span * area), 0.0, 1.0);
}

@fragment
fn fs_main(in: Full) -> @location(0) vec4f {
  let rel = vec2f(in.position.x, u.cells.w - in.position.y) - u.grid.xy;
  var ink = vec4f(0.0);
  let cx = floor(rel.x / u.grid.z);
  let row = floor(rel.y / u.grid.w);
  if (cx >= 0.0 && cx < u.cells.x) {
    for (var k = -1; k <= 1; k++) {
      let cy = row + f32(k);
      if (cy < 0.0 || cy >= u.cells.y) {
        continue;
      }
      let f = textureLoad(field, vec2i(i32(cx), i32(cy)), 0);
      if (f.g < 0.002) {
        continue;
      }
      let center = (vec2f(cx, cy) + 0.5) * u.grid.zw + vec2f(0.0, (f.a - 0.5) * u.grid.w);
      let alpha = coverage(rel - center, f.r) * f.g;
      if (alpha > ink.a) {
        ink = vec4f(mix(u.color.rgb, u.accent.rgb, f.b) * alpha, alpha);
      }
    }
  }
  return ink + vec4f(u.background.rgb, 1.0) * (1.0 - ink.a);
}
