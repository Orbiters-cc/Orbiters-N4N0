// D-SSIM colour loss, as 3DGS trains it: loss = (1 − λ)·L1 + λ·(1 − SSIM), means over scene pixels and channels,
// SSIM with an 11×11 Gaussian window (σ = 1.5), zero padding, C1 = 0.01², C2 = 0.03² (fused-ssim's formulation).
// Masked pixels count as black in both images and get no gradient. λ is frame.loss.z.
// Two passes over 16×16 tiles with a 5-pixel apron in workgroup memory, one channel at a time:
//   partials: per pixel and channel, a = ∂S/∂μx, b = ∂S/∂E[x²], c = ∂S/∂E[xy] of the window centred there;
//   gradient: ∂ΣS/∂x = G∗a + 2x·G∗b + y·G∗c, plus the L1 term, into d_pixel.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> out_color: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> target_rgb: array<u32>;
@group(0) @binding(3) var<storage, read> mask: array<f32>;
@group(0) @binding(4) var<storage, read_write> partials: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> d_pixel: array<vec4<f32>>;

const SIDE: u32 = 26u;
const APRON: u32 = 676u;
const ROWS: u32 = 416u;
const C1: f32 = 0.0001;
const C2: f32 = 0.0009;

var<private> GW: array<f32, 11> = array<f32, 11>(0.001028380, 0.007598758, 0.036000772, 0.109360690, 0.213005538, 0.266011725, 0.213005538, 0.109360690, 0.036000772, 0.007598758, 0.001028380);

var<workgroup> s_x: array<f32, 676>;
var<workgroup> s_y: array<f32, 676>;
var<workgroup> s_z: array<f32, 676>;
var<workgroup> s_h: array<vec4<f32>, 416>;
var<workgroup> s_h2: array<f32, 416>;

fn target_channel(p: u32, ch: u32) -> f32 {
    return f32((target_rgb[p] >> (8u * ch)) & 255u) / 255.0;
}

fn scene(px: i32, py: i32) -> i32 {
    if (px < 0 || py < 0 || px >= i32(frame.size.x) || py >= i32(frame.size.y)) {
        return -1;
    }
    let p = u32(py) * frame.size.x + u32(px);
    // Not scene: the strip, the band, or a keyframe pixel left out by its alpha byte (crate::transients).
    if (mask[p] < 0.5 || (target_rgb[p] >> 24u) < 128u) {
        return -1;
    }
    return i32(p);
}

@compute @workgroup_size(16, 16)
fn ssim_partials(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let ox = i32(wid.x * 16u) - 5;
    let oy = i32(wid.y * 16u) - 5;
    let px = i32(wid.x * 16u + lid.x);
    let py = i32(wid.y * 16u + lid.y);
    var pa = vec4<f32>(0.0);
    var pb = vec4<f32>(0.0);
    var pc = vec4<f32>(0.0);
    for (var ch = 0u; ch < 3u; ch++) {
        workgroupBarrier();
        for (var k = li; k < APRON; k += 256u) {
            let q = scene(ox + i32(k % SIDE), oy + i32(k / SIDE));
            var xv = 0.0;
            var yv = 0.0;
            if (q >= 0) {
                xv = out_color[u32(q)][ch];
                yv = target_channel(u32(q), ch);
            }
            s_x[k] = xv;
            s_y[k] = yv;
        }
        workgroupBarrier();
        for (var k = li; k < ROWS; k += 256u) {
            let base = (k / 16u) * SIDE + k % 16u;
            var acc = vec4<f32>(0.0);
            var axy = 0.0;
            for (var t = 0u; t < 11u; t++) {
                let xv = s_x[base + t];
                let yv = s_y[base + t];
                let w = GW[t];
                acc += w * vec4<f32>(xv, yv, xv * xv, yv * yv);
                axy += w * xv * yv;
            }
            s_h[k] = acc;
            s_h2[k] = axy;
        }
        workgroupBarrier();
        var st = vec4<f32>(0.0);
        var exy = 0.0;
        for (var t = 0u; t < 11u; t++) {
            let k = (lid.y + t) * 16u + lid.x;
            st += GW[t] * s_h[k];
            exy += GW[t] * s_h2[k];
        }
        let mx = st.x;
        let my = st.y;
        let a = 2.0 * mx * my + C1;
        let b = 2.0 * (exy - mx * my) + C2;
        let c = mx * mx + my * my + C1;
        let d = (st.z - mx * mx) + (st.w - my * my) + C2;
        let s = a * b / (c * d);
        pa[ch] = s * (2.0 * my / a - 2.0 * my / b - 2.0 * mx / c + 2.0 * mx / d);
        pb[ch] = -s / d;
        pc[ch] = 2.0 * s / b;
    }
    let q = scene(px, py);
    if (px < i32(frame.size.x) && py < i32(frame.size.y)) {
        let p = u32(py) * frame.size.x + u32(px);
        if (q < 0) {
            pa = vec4<f32>(0.0);
            pb = vec4<f32>(0.0);
            pc = vec4<f32>(0.0);
        }
        partials[3u * p] = pa;
        partials[3u * p + 1u] = pb;
        partials[3u * p + 2u] = pc;
    }
}

@compute @workgroup_size(16, 16)
fn ssim_gradient(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let ox = i32(wid.x * 16u) - 5;
    let oy = i32(wid.y * 16u) - 5;
    let px = i32(wid.x * 16u + lid.x);
    let py = i32(wid.y * 16u + lid.y);
    let w = i32(frame.size.x);
    let h = i32(frame.size.y);
    let q = scene(px, py);
    var dssim = vec3<f32>(0.0);
    for (var ch = 0u; ch < 3u; ch++) {
        workgroupBarrier();
        for (var k = li; k < APRON; k += 256u) {
            let x = ox + i32(k % SIDE);
            let y = oy + i32(k / SIDE);
            var va = 0.0;
            var vb = 0.0;
            var vc = 0.0;
            if (x >= 0 && y >= 0 && x < w && y < h) {
                let p = u32(y) * frame.size.x + u32(x);
                va = partials[3u * p][ch];
                vb = partials[3u * p + 1u][ch];
                vc = partials[3u * p + 2u][ch];
            }
            s_x[k] = va;
            s_y[k] = vb;
            s_z[k] = vc;
        }
        workgroupBarrier();
        for (var k = li; k < ROWS; k += 256u) {
            let base = (k / 16u) * SIDE + k % 16u;
            var acc = vec4<f32>(0.0);
            for (var t = 0u; t < 11u; t++) {
                acc += GW[t] * vec4<f32>(s_x[base + t], s_y[base + t], s_z[base + t], 0.0);
            }
            s_h[k] = acc;
        }
        workgroupBarrier();
        var g = vec4<f32>(0.0);
        for (var t = 0u; t < 11u; t++) {
            g += GW[t] * s_h[(lid.y + t) * 16u + lid.x];
        }
        if (q >= 0) {
            let xv = out_color[u32(q)][ch];
            let yv = target_channel(u32(q), ch);
            dssim[ch] = g.x + 2.0 * xv * g.y + yv * g.z;
        }
    }
    if (q >= 0) {
        let p = u32(q);
        let tc = vec3<f32>(target_channel(p, 0u), target_channel(p, 1u), target_channel(p, 2u));
        let diff = out_color[p].xyz - tc;
        let lambda = frame.loss.z;
        let k = frame.loss.x / (3.0 * max(frame.loss.w, 1.0));
        d_pixel[p] = vec4<f32>(k * ((1.0 - lambda) * sign(diff) - lambda * dssim), 0.0);
    } else if (px < w && py < h) {
        d_pixel[u32(py) * frame.size.x + u32(px)] = vec4<f32>(0.0);
    }
}
