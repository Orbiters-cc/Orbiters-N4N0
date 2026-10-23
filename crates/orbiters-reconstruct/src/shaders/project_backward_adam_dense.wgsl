// The projection backward fused with dense Adam (lab, S4_FUSED_DENSE; after common.wgsl, a sums_read_*.wgsl variant
// and lazy_adam.wgsl for its bindings): every Gaussian takes its Adam step here, one this step shows with its
// parameter gradients (project_backward.wgsl's), one it does not with none (its moments decay and its momentum moves
// it, as adam.wgsl's dense steps do), so the gradients never cross memory. adam.wgsl's dense Adam, fused.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read_write> params4: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> proj: array<f32>;
@group(0) @binding(5) var<storage, read_write> stats: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read> filter3d: array<f32>;

// Adam on four parameter slots: moments m and v, gradient g, rates lr, bias corrections c1 and c2. A gradient that is
// no number leaves its slot as it was.
struct SlotsD {
    p: vec4<f32>,
    m: vec4<f32>,
    v: vec4<f32>,
}

fn adam4d(p: vec4<f32>, m: vec4<f32>, v: vec4<f32>, g: vec4<f32>, lr: vec4<f32>, c1: f32, c2: f32) -> SlotsD {
    let b1 = adam.beta.x;
    let b2 = adam.beta.y;
    let ok = (bitcast<vec4<u32>>(g) & vec4<u32>(0x7f800000u)) != vec4<u32>(0x7f800000u);
    let m_new = b1 * m + (1.0 - b1) * g;
    let v_new = b2 * v + (1.0 - b2) * g * g;
    let p_new = p - lr * (m_new / c1) / (sqrt(v_new / c2) + adam.beta.z);
    var out: SlotsD;
    out.p = select(p, p_new, ok);
    out.m = select(m, m_new, ok);
    out.v = select(v, v_new, ok);
    return out;
}

@compute @workgroup_size(256)
fn main_dense(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= frame.counts.x) {
        return;
    }
    let o = 4u * i;
    let shown = proj[i * PROJ + 15u] != 0.0;
    // Dormant (relocate.wgsl: moments zero since its restart, no step has shown it): nothing moves.
    if (!shown && m2[o + 3u].w == 1.0) {
        return;
    }
    var p0 = params4[o];
    var p1 = params4[o + 1u];
    var p2 = params4[o + 2u];
    var p3 = params4[o + 3u];
    let te_unseen = max(f32(adam.n.y) - p3.w, 1.0);
    if (!shown) {
        // Not shown: Adam with no gradient.
        let c1 = 1.0 - pow(adam.beta.x, te_unseen);
        let c2 = 1.0 - pow(adam.beta.y, te_unseen);
        let z = vec4<f32>(0.0);
        let s0 = adam4d(p0, m1[o], m2[o], z, adam.lr[0], c1, c2);
        let s1 = adam4d(p1, m1[o + 1u], m2[o + 1u], z, adam.lr[1], c1, c2);
        let s2 = adam4d(p2, m1[o + 2u], m2[o + 2u], z, adam.lr[2], c1, c2);
        let s3 = adam4d(p3, m1[o + 3u], m2[o + 3u], z, adam.lr[3], c1, c2);
        params4[o] = s0.p;
        params4[o + 1u] = s1.p;
        params4[o + 2u] = s2.p;
        params4[o + 3u] = vec4<f32>(s3.p.xy, p3.zw);
        m1[o] = s0.m;
        m1[o + 1u] = s1.m;
        m1[o + 2u] = s2.m;
        m1[o + 3u] = vec4<f32>(s3.m.xy, 0.0, 0.0);
        m2[o] = s0.v;
        m2[o + 1u] = s1.v;
        m2[o + 2u] = s2.v;
        m2[o + 3u] = vec4<f32>(s3.v.xy, 0.0, 0.0);
        return;
    }
    let op = sigmoid(p2.z);
    let mean = p0.xyz;
    let log_scale = vec3<f32>(p0.w, p1.x, p1.y);
    let q = vec4<f32>(p1.z, p1.w, p2.x, p2.y);
    let filter_var = filter3d[i];
    let m = proj_math(mean, log_scale, q, filter_var);
    let g = i * GRAD2D;
    let d_mean2 = vec2<f32>(grad(g), grad(g + 1u));
    let d_conic = vec3<f32>(grad(g + 2u), grad(g + 3u), grad(g + 4u));
    let d_op = grad(g + 5u);
    let d_color = vec3<f32>(grad(g + 6u), grad(g + 7u), grad(g + 8u));
    let d_z = grad(g + 9u);
    clear_grads(g);
    let d_opacity = (d_op * m.c3 * m.c2 + frame.reg.x) * op * (1.0 - op);
    let d_c2 = d_op * op * m.c3;
    let d_c3 = d_op * op * m.c2;
    let ka = proj[i * PROJ + 2u];
    let kb = proj[i * PROJ + 3u];
    let kc = proj[i * PROJ + 4u];
    var g00 = -(ka * ka * d_conic.x + ka * kb * d_conic.y + kb * kb * d_conic.z);
    var g01 = -(ka * kb * d_conic.x + 0.5 * (kb * kb + ka * kc) * d_conic.y + kb * kc * d_conic.z);
    var g11 = -(kb * kb * d_conic.x + kb * kc * d_conic.y + kc * kc * d_conic.z);
    let h = 0.5 * d_c2 * m.c2;
    if (m.det_a > flat_det()) {
        let pp = m.a.z / m.det_a;
        let qq = -m.a.y / m.det_a;
        let r = m.a.x / m.det_a;
        let fx2 = frame.reg.z;
        let fy2 = frame.reg.w;
        g00 += h * (pp * fx2 * ka + qq * fy2 * kb);
        g01 += h * 0.5 * ((pp * fx2 * kb + qq * fy2 * kc) + (qq * fx2 * ka + r * fy2 * kb));
        g11 += h * (qq * fx2 * kb + r * fy2 * kc);
    } else {
        g00 -= h * ka;
        g01 -= h * kb;
        g11 -= h * kc;
    }
    let j0 = m.j0;
    let j1 = m.j1;
    var d_sc: array<vec3<f32>, 3>;
    for (var r = 0u; r < 3u; r++) {
        d_sc[r] = g00 * j0[r] * j0 + g01 * (j0[r] * j1 + j1[r] * j0) + g11 * j1[r] * j1;
    }
    let js0 = vec3<f32>(dot(j0, m.sigma_c[0]), dot(j0, m.sigma_c[1]), dot(j0, m.sigma_c[2]));
    let js1 = vec3<f32>(dot(j1, m.sigma_c[0]), dot(j1, m.sigma_c[1]), dot(j1, m.sigma_c[2]));
    let dj0 = 2.0 * (g00 * js0 + g01 * js1);
    let dj1 = 2.0 * (g01 * js0 + g11 * js1);
    let fx = frame.focal.x;
    let fy = frame.focal.y;
    let x = m.pc.x;
    let y = m.pc.y;
    let z = m.pc.z;
    var d_pc = vec3<f32>(0.0);
    d_pc.x += d_mean2.x * fx / z;
    d_pc.z += -d_mean2.x * fx * x / (z * z);
    d_pc.y += d_mean2.y * fy / z;
    d_pc.z += -d_mean2.y * fy * y / (z * z);
    d_pc.z += d_z;
    d_pc.z += dj0.x * (-fx / (z * z)) + dj1.y * (-fy / (z * z));
    if (m.clamp_x) {
        d_pc.z += dj0.z * (fx * m.u / (z * z));
    } else {
        d_pc.x += dj0.z * (-fx / (z * z));
        d_pc.z += dj0.z * (2.0 * fx * x / (z * z * z));
    }
    if (m.clamp_y) {
        d_pc.z += dj1.z * (fy * m.v / (z * z));
    } else {
        d_pc.y += dj1.z * (-fy / (z * z));
        d_pc.z += dj1.z * (2.0 * fy * y / (z * z * z));
    }
    let w = array<vec3<f32>, 3>(frame.r0.xyz, frame.r1.xyz, frame.r2.xyz);
    let d_mean = vec3<f32>(dot(column(w, 0u), d_pc), dot(column(w, 1u), d_pc), dot(column(w, 2u), d_pc));
    var d_sw: array<vec3<f32>, 3>;
    for (var k = 0u; k < 3u; k++) {
        let wk = column(w, k);
        var row: vec3<f32>;
        for (var l = 0u; l < 3u; l++) {
            let wl = column(w, l);
            let dsc_wl = vec3<f32>(dot(d_sc[0], wl), dot(d_sc[1], wl), dot(d_sc[2], wl));
            row[l] = dot(wk, dsc_wl);
        }
        d_sw[k] = row;
    }
    let mr = array<vec3<f32>, 3>(m.rq[0] * m.sf, m.rq[1] * m.sf, m.rq[2] * m.sf);
    var dr: array<vec3<f32>, 3>;
    var d_sf = vec3<f32>(0.0);
    for (var r = 0u; r < 3u; r++) {
        let dm = 2.0 * (d_sw[r].x * mr[0] + d_sw[r].y * mr[1] + d_sw[r].z * mr[2]);
        dr[r] = dm * m.sf;
        d_sf += m.rq[r] * dm;
    }
    let d_log_scale = d_sf * m.s * m.s / m.sf + d_c3 * m.c3 * filter_var / (m.sf * m.sf);
    let d_scale = d_log_scale + frame.reg.y * m.s;
    let qw = m.qn.x;
    let qx = m.qn.y;
    let qy = m.qn.z;
    let qz = m.qn.w;
    let dq = 2.0 * vec4<f32>(
        -qz * dr[0].y + qy * dr[0].z + qz * dr[1].x - qx * dr[1].z - qy * dr[2].x + qx * dr[2].y,
        qy * dr[0].y + qz * dr[0].z + qy * dr[1].x - 2.0 * qx * dr[1].y - qw * dr[1].z + qz * dr[2].x + qw * dr[2].y - 2.0 * qx * dr[2].z,
        -2.0 * qy * dr[0].x + qx * dr[0].y + qw * dr[0].z + qx * dr[1].x + qz * dr[1].z - qw * dr[2].x + qz * dr[2].y - 2.0 * qy * dr[2].z,
        -2.0 * qz * dr[0].x - qw * dr[0].y + qx * dr[0].z + qw * dr[1].x - 2.0 * qz * dr[1].y + qy * dr[1].z + qx * dr[2].x + qy * dr[2].y,
    );
    let d_rot = (dq - m.qn * dot(m.qn, dq)) / m.qlen;
    var st = stats[i];
    st.z = max(st.z, proj[i * PROJ + 10u] / f32(frame.size.x));
    stats[i] = st;

    // Adam at this step (adam.n.y), its bias correction counted from the Gaussian's restart (slot 15).
    let g0 = vec4<f32>(d_mean, d_scale.x);
    let g1 = vec4<f32>(d_scale.y, d_scale.z, d_rot.x, d_rot.y);
    let g2 = vec4<f32>(d_rot.z, d_rot.w, d_opacity, d_color.x);
    let g3 = vec4<f32>(d_color.y, d_color.z, 0.0, 0.0);
    let te = max(f32(adam.n.y) - p3.w, 1.0);
    let c1 = 1.0 - pow(adam.beta.x, te);
    let c2 = 1.0 - pow(adam.beta.y, te);
    let a3 = m1[o + 3u];
    let s0 = adam4d(p0, m1[o], m2[o], g0, adam.lr[0], c1, c2);
    let s1 = adam4d(p1, m1[o + 1u], m2[o + 1u], g1, adam.lr[1], c1, c2);
    let s2 = adam4d(p2, m1[o + 2u], m2[o + 2u], g2, adam.lr[2], c1, c2);
    let s3 = adam4d(p3, a3, m2[o + 3u], g3, adam.lr[3], c1, c2);
    params4[o] = s0.p;
    params4[o + 1u] = s1.p;
    params4[o + 2u] = s2.p;
    // The sky flag and the restart step are no parameters (rate 0): kept as they were.
    params4[o + 3u] = vec4<f32>(s3.p.xy, p3.zw);
    m1[o] = s0.m;
    m1[o + 1u] = s1.m;
    m1[o + 2u] = s2.m;
    m1[o + 3u] = vec4<f32>(s3.m.xy, 0.0, 0.0);
    m2[o] = s0.v;
    m2[o + 1u] = s1.v;
    m2[o + 2u] = s2.v;
    m2[o + 3u] = vec4<f32>(s3.v.xy, 0.0, 0.0);
}
