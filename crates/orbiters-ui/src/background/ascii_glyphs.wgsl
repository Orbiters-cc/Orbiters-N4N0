@group(0) @binding(1) var field: texture_2d<f32>;
@group(0) @binding(2) var aux: texture_2d<f32>;
@group(0) @binding(3) var atlas: texture_2d<f32>;
@group(0) @binding(4) var halo: texture_2d<f32>;
@group(0) @binding(5) var linear_clamp: sampler;

fn sample_glyph(index: f32, local: vec2f, coord: vec2f) -> f32 {
  let columns = u.atlas.x;
  let column = index - columns * floor(index / columns);
  let row = floor(index / columns);
  let uv = vec2f((column + local.x) / columns, (row + 1.0 - local.y) / u.atlas.y);
  let continuous = coord / u.atlas.xy;
  return textureSampleGrad(atlas, linear_clamp, uv, dpdx(continuous), dpdy(continuous)).r;
}

// The targets' first row is their top here; WebGL's is the bottom.
fn at(uv: vec2f) -> vec2f {
  return vec2f(uv.x, 1.0 - uv.y);
}

fn glyph_at(px: vec2f) -> vec4f {
  let coord = (px - u.cell.zw) / u.cell.xy;
  let cell = floor(coord);
  let local = coord - cell;
  let uv = clamp(((cell + 0.5) * u.cell.xy + u.cell.zw) / u.frame.xy, vec2f(0.0), vec2f(1.0));
  let f = textureSampleLevel(field, linear_clamp, at(uv), 0.0);
  let decode = 1.0 - textureSampleLevel(aux, linear_clamp, at(uv), 0.0).r;
  let light = u.glyph.w > 0.5;
  let visible = select(f.rgb, f.rgb + (1.0 - f.a), light);
  let luma = dot(visible, vec3f(0.2126, 0.7152, 0.0722));
  let value = select(luma, clamp((1.0 - luma) * 1.5, 0.0, 1.0), light);
  let pick = hash12(cell);
  let level = hash12(cell * 1.73 + 4.1);
  let tick = floor(u.frame.w * 9.0 + level * 9.0);
  let wobble = (hash12(cell * 2.31 + tick * 17.7) - 0.5) * u.glyph.z * 0.4 + (level - 0.5) * 0.1;
  let tone = clamp(pow(value, 0.75) + wobble, 0.0, 1.0);
  var index = floor(tone * (u.atlas.z - 1.0) + 0.5);
  if (decode > 0.02) {
    let frame = floor(u.frame.w * 20.0);
    if (hash12(cell * 3.7 + frame * 5.3) < decode * 1.25) {
      index = 1.0 + floor(hash12(cell + frame * 1.37) * (u.atlas.z - 1.0));
    }
  }
  let present = step(pick, u.glyph.x * mix(1.0, 0.3 + tone * 1.1, 0.55));
  let cutoff = smoothstep(0.03, 0.1, value);
  let coverage = sample_glyph(index, local, coord);
  let alpha = u.glyph.y * present * cutoff * coverage * mix(1.0, 0.35 + 0.65 * level, 0.55);
  if (!light) {
    let lit = f.rgb * 0.55 + vec3f(0.03) * f.a;
    return vec4f(lit * alpha, 0.6 * f.a * alpha);
  }
  return vec4f(visible * 0.8 * alpha, alpha);
}
