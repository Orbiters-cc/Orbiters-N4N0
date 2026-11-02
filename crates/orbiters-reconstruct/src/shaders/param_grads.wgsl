// The projection backward's mathematics (after common.wgsl and a sums_read_*.wgsl variant, with `proj` at binding 2 and
// `stats` at binding 5): Gaussian i's 2D gradients (grad2d, cleared for the next step's sums) to gradients of its
// parameters, the regularisers' gradients, and its statistics since the last relocation round (two vec4s a Gaussian,
// cleared by each round): the largest screen radius it reached (stats[2i].z, as a share of each view's width, so steps
// on the depth-sample grid compare alike) and, on steps that record birth statistics (common.wgsl stats_mode), on
// colour steps the sum of its screen-space positional gradient's norm (x, 3DGS's densification statistic), the colour
// steps that showed it (y) and the colour error it blended (w, Σ error × α·T); on depth steps (stats[2i + 1]) the sum
// of α·T × its relative distance from the measured surface (x) and of α·T at depth samples (y).
// project_backward.wgsl writes them out (tests), project_backward_adam.wgsl takes its Adam step with them.

struct ParamGrads {
    mean: vec3<f32>,
    scale: vec3<f32>,
    rotation: vec4<f32>,
    opacity: f32,
    color: vec3<f32>,
}

fn param_grads(i: u32, mean: vec3<f32>, log_scale: vec3<f32>, q: vec4<f32>, opacity_logit: f32, filter_var: f32) -> ParamGrads {
    let op = sigmoid(opacity_logit);
    let m = proj_math(mean, log_scale, q, filter_var);
    let g = i * GRAD2D;
    let d_mean2 = vec2<f32>(grad(g), grad(g + 1u));
    let d_conic = vec3<f32>(grad(g + 2u), grad(g + 3u), grad(g + 4u));
    let d_op = grad(g + 5u);
    var d_color = vec3<f32>(grad(g + 6u), grad(g + 7u), grad(g + 8u));
    var d_z = grad(g + 9u);
    clear_grads(g);
    var st = stats[2u * i];
    st.z = max(st.z, proj[i * PROJ + 10u] / f32(frame.size.x));
    let mode = stats_mode();
    if (mode == STATS_COLOR) {
        // The depth slot summed colour error × α·T (raster_backward.wgsl stat_grads), not a depth gradient.
        st.x += length(d_mean2);
        st.y += 1.0;
        st.w += d_z;
        d_z = 0.0;
    }
    stats[2u * i] = st;
    if (mode == STATS_DEPTH) {
        // The colour slots summed distance from the measured surface and α·T at the samples, not colour gradients.
        var sd = stats[2u * i + 1u];
        sd.x += d_color.x;
        sd.y += d_color.y;
        stats[2u * i + 1u] = sd;
        d_color = vec3<f32>(0.0);
    }
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
    var out: ParamGrads;
    out.mean = d_mean;
    out.scale = d_scale;
    out.rotation = d_rot;
    out.opacity = d_opacity;
    out.color = d_color;
    return out;
}
