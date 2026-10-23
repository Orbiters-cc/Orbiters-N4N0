// D-SSIM's two passes reading their inputs once for the three channels (after ssim.wgsl, whose bindings, constants and
// helpers they share): each apron pixel's render, target and mask (partials pass) or its nine partials (gradient pass)
// come from memory once into workgroup memory, instead of once a channel; each channel's filters then run as in
// ssim.wgsl, in the same order, so the results are the same numbers.

var<workgroup> s_x3: array<vec4<f32>, 676>;
var<workgroup> s_y3: array<vec4<f32>, 676>;
var<workgroup> s_a3: array<vec4<f32>, 676>;
var<workgroup> s_b3: array<vec4<f32>, 676>;
var<workgroup> s_c3: array<vec4<f32>, 676>;

@compute @workgroup_size(16, 16)
fn ssim_partials2(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let ox = i32(wid.x * 16u) - 5;
    let oy = i32(wid.y * 16u) - 5;
    let px = i32(wid.x * 16u + lid.x);
    let py = i32(wid.y * 16u + lid.y);
    var pa = vec4<f32>(0.0);
    var pb = vec4<f32>(0.0);
    var pc = vec4<f32>(0.0);
    for (var k = li; k < APRON; k += 256u) {
        let q = scene(ox + i32(k % SIDE), oy + i32(k / SIDE));
        var xv = vec4<f32>(0.0);
        var yv = vec4<f32>(0.0);
        if (q >= 0) {
            xv = vec4<f32>(out_color[u32(q)].xyz, 0.0);
            yv = vec4<f32>(target_channel(u32(q), 0u), target_channel(u32(q), 1u), target_channel(u32(q), 2u), 0.0);
        }
        s_x3[k] = xv;
        s_y3[k] = yv;
    }
    for (var ch = 0u; ch < 3u; ch++) {
        workgroupBarrier();
        for (var k = li; k < ROWS; k += 256u) {
            let base = (k / 16u) * SIDE + k % 16u;
            var acc = vec4<f32>(0.0);
            var axy = 0.0;
            for (var t = 0u; t < 11u; t++) {
                let xv = s_x3[base + t][ch];
                let yv = s_y3[base + t][ch];
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
        let cd = c * d;
        let s = a * b / cd;
        pa[ch] = 2.0 * my * (b - a) / cd - 2.0 * mx * s * (1.0 / c - 1.0 / d);
        pb[ch] = -s / d;
        pc[ch] = 2.0 * a / cd;
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
fn ssim_gradient2(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let ox = i32(wid.x * 16u) - 5;
    let oy = i32(wid.y * 16u) - 5;
    let px = i32(wid.x * 16u + lid.x);
    let py = i32(wid.y * 16u + lid.y);
    let w = i32(frame.size.x);
    let h = i32(frame.size.y);
    let q = scene(px, py);
    for (var k = li; k < APRON; k += 256u) {
        let x = ox + i32(k % SIDE);
        let y = oy + i32(k / SIDE);
        var va = vec4<f32>(0.0);
        var vb = vec4<f32>(0.0);
        var vc = vec4<f32>(0.0);
        if (x >= 0 && y >= 0 && x < w && y < h) {
            let p = u32(y) * frame.size.x + u32(x);
            va = partials[3u * p];
            vb = partials[3u * p + 1u];
            vc = partials[3u * p + 2u];
        }
        s_a3[k] = va;
        s_b3[k] = vb;
        s_c3[k] = vc;
    }
    var dssim = vec3<f32>(0.0);
    for (var ch = 0u; ch < 3u; ch++) {
        workgroupBarrier();
        for (var k = li; k < ROWS; k += 256u) {
            let base = (k / 16u) * SIDE + k % 16u;
            var acc = vec4<f32>(0.0);
            for (var t = 0u; t < 11u; t++) {
                acc += GW[t] * vec4<f32>(s_a3[base + t][ch], s_b3[base + t][ch], s_c3[base + t][ch], 0.0);
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
