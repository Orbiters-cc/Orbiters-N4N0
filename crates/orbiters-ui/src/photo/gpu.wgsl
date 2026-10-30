// The photo editor's layers on the GPU (photo/gpu.rs): each entry point one pass of a layer, with the maths of the CPU
// passes (photo/tone.rs, effects.rs, blur.rs, text.rs, geometry.rs) so that a picture saved from it looks as it was
// seen. Working pictures are linear light, premultiplied alpha, in half floats; `take` reads the source (RGBA8,
// encoded sRGB) and `out` writes RGBA8 again, encoded, straight alpha. DST_FORMAT is put in by photo/gpu.rs: the
// working format for the layers' passes, RGBA8 for `out`.

struct Params {
    // The picture's size in pixels, the layer's opacity and whether the pass mixes its result with the picture
    // before the layer by that opacity and the layer's mask (the last pass of a layer that is faded or masked).
    size: vec2<f32>,
    opacity: f32,
    mixing: f32,
    // The mask: its shape (0 everywhere, 1 a circle, 2 a band), inverted, its radius or width, its softness.
    mask: vec4<f32>,
    // Its centre (shares), and the sine and cosine of a band's angle.
    mask_at: vec4<f32>,
    // An offset into `data`, a radius in pixels, a count (taps, a row's span), a flag.
    ints: vec4<i32>,
    v: array<vec4<f32>, 12>,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<DST_FORMAT, write>;
@group(0) @binding(2) var<uniform> P: Params;
// The picture before the layer (mixing), the softened luma (comic) or a text's letters.
@group(0) @binding(3) var aux: texture_2d<f32>;
// Tables (tone curves) and kernels.
@group(0) @binding(4) var<storage, read> data: array<f32>;
// A lens blur's running sums along each row: the weighted colour and the weights.
@group(0) @binding(5) var<storage, read_write> sums: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> weights: array<f32>;

fn outside(id: vec3<u32>) -> bool {
    return f32(id.x) >= P.size.x || f32(id.y) >= P.size.y;
}

fn to_linear(c: f32) -> f32 {
    if (c <= 0.04045) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn to_srgb(c: f32) -> f32 {
    if (c <= 0.0031308) {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

fn encode(v: f32) -> f32 {
    return to_srgb(clamp(v, 0.0, 1.0));
}

fn decode(v: f32) -> f32 {
    return to_linear(clamp(v, 0.0, 1.0));
}

fn encode3(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(encode(c.x), encode(c.y), encode(c.z));
}

fn decode3(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(decode(c.x), decode(c.y), decode(c.z));
}

fn luma(c: vec3<f32>) -> f32 {
    return c.x * 0.299 + c.y * 0.587 + c.z * 0.114;
}

fn step_smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    return a + (b - a) * t;
}

fn lerp3(a: vec3<f32>, b: vec3<f32>, t: f32) -> vec3<f32> {
    return a + (b - a) * t;
}

// A pixel of `t`, the edges held beyond it.
fn at(t: texture_2d<f32>, x: i32, y: i32) -> vec4<f32> {
    let s = vec2<i32>(textureDimensions(t)) - vec2<i32>(1, 1);
    return textureLoad(t, clamp(vec2<i32>(x, y), vec2<i32>(0, 0), s), 0);
}

// Bilinear between the pixels around (x, y), pixel centres at half pixels (Image::sample).
fn sample(t: texture_2d<f32>, x: f32, y: f32) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let ix = i32(x0);
    let iy = i32(y0);
    let a = at(t, ix, iy);
    let b = at(t, ix + 1, iy);
    let c = at(t, ix, iy + 1);
    let d = at(t, ix + 1, iy + 1);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    return top + (bottom - top) * ty;
}

// Mask::weight.
fn mask_weight(xy: vec2<i32>) -> f32 {
    let kind = P.mask.x;
    if (kind < 0.5) {
        return 1.0;
    }
    let u = (f32(xy.x) + 0.5) / P.size.x;
    let v = (f32(xy.y) + 0.5) / P.size.y;
    let aspect = P.size.x / P.size.y;
    let du = (u - P.mask_at.x) * aspect;
    let dv = v - P.mask_at.y;
    var w: f32;
    if (kind < 1.5) {
        let d = sqrt(du * du + dv * dv);
        w = 1.0 - step_smooth(P.mask.z, P.mask.z + max(P.mask.w, 1e-4), d);
    } else {
        let d = abs(-du * P.mask_at.z + dv * P.mask_at.w);
        w = 1.0 - step_smooth(P.mask.z * 0.5, P.mask.z * 0.5 + max(P.mask.w, 1e-4), d);
    }
    if (P.mask.y > 0.5) {
        w = 1.0 - w;
    }
    return w;
}

// Writes a layer's result, mixed back towards the picture before it where the layer shows less (render::mix).
fn finish(xy: vec2<i32>, p: vec4<f32>, before: vec4<f32>) {
    var o = p;
    if (P.mixing > 0.5) {
        let k = P.opacity * mask_weight(xy);
        o = before + (p - before) * k;
    }
    textureStore(dst, xy, o);
}

// A tone table of 1025 entries at `off` (tone::Lut::at).
fn lut(off: i32, x: f32) -> f32 {
    let t = clamp(x, 0.0, 1.0) * 1024.0;
    let i = min(i32(t), 1023);
    let a = data[off + i];
    let b = data[off + i + 1];
    return a + (b - a) * (t - f32(i));
}

// A pixel of the source (RGBA8, encoded, straight) in linear light, premultiplied.
fn source(x: i32, y: i32) -> vec4<f32> {
    let c = textureLoad(src, vec2<i32>(x, y), 0);
    return vec4<f32>(to_linear(c.r) * c.a, to_linear(c.g) * c.a, to_linear(c.b) * c.a, c.a);
}

// geometry::transform: the frame of the turned, straightened source at this size. v[0..3]: the homography from the
// frame's shares to the source's; v[3]: the source's width and height, and 1 when it is not straightened.
@compute @workgroup_size(8, 8)
fn take(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let q = vec3<f32>((f32(xy.x) + 0.5) / P.size.x, (f32(xy.y) + 0.5) / P.size.y, 1.0);
    let w = dot(P.v[2].xyz, q);
    let s = vec2<f32>(dot(P.v[0].xyz, q), dot(P.v[1].xyz, q)) / w;
    let sw = P.v[3].x;
    let sh = P.v[3].y;
    var fx = s.x * sw - 0.5;
    var fy = s.y * sh - 0.5;
    if (P.v[3].z < 0.5 && (fx < -1.0 || fy < -1.0 || fx > sw || fy > sh)) {
        textureStore(dst, xy, vec4<f32>(0.0));
        return;
    }
    fx = clamp(fx, 0.0, sw - 1.0);
    fy = clamp(fy, 0.0, sh - 1.0);
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let ix = i32(x0);
    let iy = i32(y0);
    let mx = i32(sw) - 1;
    let my = i32(sh) - 1;
    let a = source(ix, iy);
    let b = source(min(ix + 1, mx), iy);
    let c = source(ix, min(iy + 1, my));
    let d = source(min(ix + 1, mx), min(iy + 1, my));
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    textureStore(dst, xy, top + (bottom - top) * ty);
}

// tone::light: v[0].x the exposure's gain; ints.x the tone table.
@compute @workgroup_size(8, 8)
fn light(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let p = textureLoad(src, xy, 0);
    var o = p;
    if (p.a > 0.0) {
        let c = p.rgb / p.a;
        let g = P.v[0].x;
        let off = P.ints.x;
        let r = vec3<f32>(decode(lut(off, encode(c.x * g))), decode(lut(off, encode(c.y * g))), decode(lut(off, encode(c.z * g))));
        o = vec4<f32>(r * p.a, p.a);
    }
    finish(xy, o, p);
}

// tone::color_of: v[0].xyz the white balance's gains, v[0].w 1 with a hue turn whose matrix is v[1..4]; v[4].x the
// saturation, v[4].y the vibrance.
@compute @workgroup_size(8, 8)
fn color(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let p = textureLoad(src, xy, 0);
    var o = p;
    if (p.a > 0.0) {
        let c = p.rgb / p.a;
        var e = encode3(c * P.v[0].xyz);
        if (P.v[0].w > 0.5) {
            e = clamp(vec3<f32>(dot(P.v[1].xyz, e), dot(P.v[2].xyz, e), dot(P.v[3].xyz, e)), vec3<f32>(0.0), vec3<f32>(1.0));
        }
        let y = luma(e);
        let chroma = max(max(max(e.x, e.y), e.z), 0.0) - min(min(min(e.x, e.y), e.z), 1.0);
        let amount = max(1.0 + P.v[4].x + P.v[4].y * (1.0 - chroma) * (1.0 - chroma), 0.0);
        let r = decode3(clamp(y + (e - y) * amount, vec3<f32>(0.0), vec3<f32>(1.0)));
        o = vec4<f32>(r * p.a, p.a);
    }
    finish(xy, o, p);
}

// tone::curves: ints.x the red, green and blue tables one after the other (each channel's curve then the RGB one).
@compute @workgroup_size(8, 8)
fn curves(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let p = textureLoad(src, xy, 0);
    var o = p;
    if (p.a > 0.0) {
        let c = p.rgb / p.a;
        let off = P.ints.x;
        let r = vec3<f32>(decode(lut(off, encode(c.x))), decode(lut(off + 1025, encode(c.y))), decode(lut(off + 2050, encode(c.z))));
        o = vec4<f32>(r * p.a, p.a);
    }
    finish(xy, o, p);
}

// effects::vignette: v[0] the amount, size, fall-off exponent and centre's x; v[1].x its y.
@compute @workgroup_size(8, 8)
fn vignette(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let p = textureLoad(src, xy, 0);
    let aspect = P.size.x / P.size.y;
    let u = (f32(xy.x) + 0.5) / P.size.x;
    let v = (f32(xy.y) + 0.5) / P.size.y;
    let d = vec2<f32>((u - P.v[0].w) * aspect, v - P.v[1].x);
    let size = P.v[0].y;
    let base = clamp(1.0 - (d.x * d.x + d.y * d.y) * 1.8 / (size * size), 0.0, 1.0);
    var shade = 0.0;
    if (base > 0.0) {
        shade = pow(base, P.v[0].z);
    }
    let amount = P.v[0].x;
    var o = p;
    if (amount > 0.0) {
        o = vec4<f32>(p.rgb * lerp(1.0, shade, amount), p.a);
    } else {
        let t = (1.0 - shade) * -amount;
        o = vec4<f32>(p.rgb + (vec3<f32>(p.a) - p.rgb) * t, p.a);
    }
    finish(xy, o, p);
}

// effects::aberration: v[0].x the shift, v[0].yz the centre.
@compute @workgroup_size(8, 8)
fn aberration(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let p = textureLoad(src, xy, 0);
    let u = (f32(xy.x) + 0.5) / P.size.x;
    let v = (f32(xy.y) + 0.5) / P.size.y;
    let shift = P.v[0].x;
    let away = vec2<f32>(u - P.v[0].y, v - P.v[0].z);
    let r = vec2<f32>(u - away.x * shift, v - away.y * shift);
    let b = vec2<f32>(u + away.x * shift, v + away.y * shift);
    var o = p;
    o.r = sample(src, r.x * P.size.x, r.y * P.size.y).r;
    o.b = sample(src, b.x * P.size.x, b.y * P.size.y).b;
    finish(xy, o, p);
}

// The comic's lines are found in: the luma of the picture as seen, times alpha, and alpha (effects::edge_map).
@compute @workgroup_size(8, 8)
fn comic_luma(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let p = textureLoad(src, xy, 0);
    var o = vec4<f32>(0.0);
    if (p.a > 0.0) {
        o = vec4<f32>(luma(encode3(p.rgb / p.a)) * p.a, p.a, 0.0, 0.0);
    }
    textureStore(dst, xy, o);
}

const INK: vec3<f32> = vec3<f32>(0.06, 0.05, 0.08);

// effects::comic_tone.
fn comic_tone(c: vec3<f32>, steps: f32) -> vec3<f32> {
    let l = luma(c);
    let level = floor(l * steps + 0.5) / steps;
    let k = max(level, 0.05) / max(l, 1e-3);
    let tone = clamp(c * k, vec3<f32>(0.0), vec3<f32>(1.0));
    let fl = luma(tone);
    return clamp(fl + (tone - fl) * 1.2, vec3<f32>(0.0), vec3<f32>(1.0));
}

// effects::dot.
fn dot_ink(x: f32, y: f32, cell: f32, l: f32) -> f32 {
    let s = 0.70710677;
    let g = vec2<f32>((x + y) * s / cell, (y - x) * s / cell);
    let f = g - floor(g) - 0.5;
    let dist = sqrt(f.x * f.x + f.y * f.y);
    let radius = sqrt(clamp(1.0 - l, 0.0, 1.0) * 0.31830987) * 1.06;
    let edge = 1.0 / cell;
    return 1.0 - step_smooth(radius - edge, radius + edge, dist);
}

// effects::comic: v[0] the tone steps, dots (1 or 0), the dots' cell and the ink; ints.y the lines' reach in pixels;
// aux the softened luma (comic_luma, blurred).
@compute @workgroup_size(8, 8)
fn comic(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let p = textureLoad(src, xy, 0);
    var col = vec3<f32>(0.0);
    var sa = 0.0;
    if (p.a > 0.0) {
        col = encode3(p.rgb / p.a);
        sa = p.a;
    }
    let l = luma(col);
    var tone = comic_tone(col, P.v[0].x);
    if (P.v[0].y > 0.5) {
        let m = dot_ink(f32(xy.x) + 0.5, f32(xy.y) + 0.5, P.v[0].z, l);
        tone = lerp3(clamp(tone * 1.25 + 0.06, vec3<f32>(0.0), vec3<f32>(1.0)), tone * 0.12, m);
    }
    let ink = P.v[0].w;
    if (ink > 0.0) {
        let t = P.ints.y;
        let le = at(aux, xy.x - t, xy.y);
        let ri = at(aux, xy.x + t, xy.y);
        let up = at(aux, xy.x, xy.y - t);
        let dn = at(aux, xy.x, xy.y + t);
        let jump = abs(le.x - ri.x) + abs(up.x - dn.x);
        let outline = abs(le.y - ri.y) + abs(up.y - dn.y);
        let e = max(jump, outline * 1.5);
        let threshold = lerp(0.42, 0.1, ink);
        let k = clamp((e - threshold) * 7.0, 0.0, 1.0);
        tone = lerp3(tone, INK, k);
    }
    let a = max(sa, p.a);
    finish(xy, vec4<f32>(decode3(tone) * a, a), p);
}

// blur::gaussian's exact kernel along the rows: ints.x the kernel, ints.y its reach; aux the picture before the
// layer (for mixing).
@compute @workgroup_size(8, 8)
fn gauss_h(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let r = P.ints.y;
    var s = vec4<f32>(0.0);
    for (var k = -r; k <= r; k++) {
        s += at(src, xy.x + k, xy.y) * data[P.ints.x + k + r];
    }
    finish(xy, s, textureLoad(aux, xy, 0));
}

@compute @workgroup_size(8, 8)
fn gauss_v(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let r = P.ints.y;
    var s = vec4<f32>(0.0);
    for (var k = -r; k <= r; k++) {
        s += at(src, xy.x, xy.y + k) * data[P.ints.x + k + r];
    }
    finish(xy, s, textureLoad(aux, xy, 0));
}

// One of blur::gaussian's box blurs: ints.y pixels each side.
@compute @workgroup_size(8, 8)
fn box_h(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let r = P.ints.y;
    var s = vec4<f32>(0.0);
    for (var k = -r; k <= r; k++) {
        s += at(src, xy.x + k, xy.y);
    }
    finish(xy, s / f32(2 * r + 1), textureLoad(aux, xy, 0));
}

@compute @workgroup_size(8, 8)
fn box_v(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let r = P.ints.y;
    var s = vec4<f32>(0.0);
    for (var k = -r; k <= r; k++) {
        s += at(src, xy.x, xy.y + k);
    }
    finish(xy, s / f32(2 * r + 1), textureLoad(aux, xy, 0));
}

// blur::directional: v[0] the line's length, the sine and cosine of its angle; ints.y the taps.
@compute @workgroup_size(8, 8)
fn directional(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let taps = P.ints.y;
    let len = P.v[0].x;
    var acc = vec4<f32>(0.0);
    for (var t = 0; t < taps; t++) {
        let d = (f32(t) / f32(taps - 1) - 0.5) * len;
        acc += sample(src, f32(xy.x) + 0.5 + P.v[0].z * d, f32(xy.y) + 0.5 + P.v[0].y * d);
    }
    finish(xy, acc / f32(taps), textureLoad(src, xy, 0));
}

// blur::lens, first: each row's running sums of the pixels weighted by their brightness, the row held past its ends
// by ints.y pixels (ints.z: its span); v[0].x the bokeh's boost. One thread a row.
@compute @workgroup_size(64)
fn lens_sums(@builtin(global_invocation_id) id: vec3<u32>) {
    let y = i32(id.x);
    if (f32(y) >= P.size.y) {
        return;
    }
    let r = P.ints.y;
    let span = P.ints.z;
    let w = i32(P.size.x);
    let base = y * (span + 1);
    let boost = P.v[0].x;
    var acc = vec4<f32>(0.0);
    var total = 0.0;
    sums[base] = acc;
    weights[base] = total;
    for (var i = 0; i < span; i++) {
        let p = textureLoad(src, vec2<i32>(clamp(i - r, 0, w - 1), y), 0);
        let a = max(p.a, 1e-6);
        let bright = max(max(max(p.r, p.g), p.b) / a - 0.6, 0.0);
        let k = 1.0 + boost * bright * bright;
        acc += p * k;
        total += k;
        sums[base + i + 1] = acc;
        weights[base + i + 1] = total;
    }
}

// blur::lens, then: the disc row by row from the sums, its half width at each height, the part pixel at its ends
// counted in part. v[0].x the radius; ints.y its reach, ints.z the rows' span.
@compute @workgroup_size(8, 8)
fn lens(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let radius = P.v[0].x;
    let r = P.ints.y;
    let span = P.ints.z;
    let h = i32(P.size.y);
    var acc = vec4<f32>(0.0);
    var total = 0.0;
    for (var dy = -r; dy <= r; dy++) {
        let half = sqrt(max(radius * radius - f32(dy * dy), 0.0));
        if (half <= 0.0 && dy != 0) {
            continue;
        }
        let k = i32(floor(half));
        let frac = half - floor(half);
        let base = clamp(xy.y + dy, 0, h - 1) * (span + 1);
        let lo = base + xy.x + r - k;
        let hi = base + xy.x + r + k;
        acc += sums[hi + 1] - sums[lo];
        total += weights[hi + 1] - weights[lo];
        if (frac > 0.0 && k < r) {
            acc += (sums[lo] - sums[lo - 1] + sums[hi + 2] - sums[hi + 1]) * frac;
            total += (weights[lo] - weights[lo - 1] + weights[hi + 2] - weights[hi + 1]) * frac;
        }
    }
    finish(xy, acc / max(total, 1e-12), textureLoad(src, xy, 0));
}

// text::composite: aux the letters drawn (linear, premultiplied), ints.xy where their top left goes; v[0].x the
// opacity.
@compute @workgroup_size(8, 8)
fn text(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let p = textureLoad(src, xy, 0);
    let local = xy - P.ints.xy;
    let size = vec2<i32>(textureDimensions(aux));
    var o = p;
    if (local.x >= 0 && local.y >= 0 && local.x < size.x && local.y < size.y) {
        let s = textureLoad(aux, local, 0);
        if (s.a > 0.0) {
            let k = P.v[0].x;
            o = s * k + p * (1.0 - s.a * k);
        }
    }
    textureStore(dst, xy, o);
}

// The picture made: encoded sRGB, straight alpha, as RGBA8 (unpremultiplied_rgba8).
@compute @workgroup_size(8, 8)
fn out(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id)) {
        return;
    }
    let xy = vec2<i32>(id.xy);
    let p = textureLoad(src, xy, 0);
    let a = clamp(p.a, 0.0, 1.0);
    var o = vec4<f32>(0.0);
    if (a > 1.0 / 512.0) {
        o = vec4<f32>(encode3(clamp(p.rgb / a, vec3<f32>(0.0), vec3<f32>(1.0))), a);
    }
    textureStore(dst, xy, o);
}
