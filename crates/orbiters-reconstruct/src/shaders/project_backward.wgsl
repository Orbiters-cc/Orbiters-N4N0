// Projection backward: each Gaussian's 2D gradients (grad2d) to gradients of its parameters (grads, laid out like
// params), the regularisers' gradients, and the largest screen radius it reached since the last relocation round
// (stats.z, as a share of each view's width, so steps on the depth-sample grid and coarse levels compare alike). The
// gradient buffer (binding 3), `grad` and `clear_grads` come from one of the sums_read_*.wgsl variants.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> params: array<f32>;
@group(0) @binding(2) var<storage, read> proj: array<f32>;
@group(0) @binding(4) var<storage, read_write> grads: array<f32>;
@group(0) @binding(5) var<storage, read_write> stats: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read> filter3d: array<f32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= frame.counts.x) {
        return;
    }
    let o = i * PARAM;
    // Lab (sparse Adam, frame.confidence.w): Adam skips a Gaussian this step does not show, so its gradients go unwritten.
    if (frame.confidence.w != 0.0 && frame.counts.w == 0u && proj[i * PROJ + 15u] == 0.0) {
        return;
    }
    for (var k = 0u; k < PARAM; k++) {
        grads[o + k] = 0.0;
    }
    // The regularisers (frame.reg.x · mean opacity, frame.reg.y · mean scale per metre) pull on the Gaussians this view
    // shows, and with frame.counts.w on every one, as 3DGS-MCMC's do (crate::train::Settings::regularise_unseen).
    let op = sigmoid(params[o + 10u]);
    let seen = proj[i * PROJ + 15u] != 0.0;
    if (!seen) {
        if (frame.counts.w != 0u) {
            let s = exp(vec3<f32>(params[o + 3u], params[o + 4u], params[o + 5u]));
            grads[o + 10u] = frame.reg.x * op * (1.0 - op);
            grads[o + 3u] = frame.reg.y * s.x;
            grads[o + 4u] = frame.reg.y * s.y;
            grads[o + 5u] = frame.reg.y * s.z;
        }
        return;
    }
    let mean = vec3<f32>(params[o], params[o + 1u], params[o + 2u]);
    let log_scale = vec3<f32>(params[o + 3u], params[o + 4u], params[o + 5u]);
    let q = vec4<f32>(params[o + 6u], params[o + 7u], params[o + 8u], params[o + 9u]);
    let filter_var = filter3d[i];
    let m = proj_math(mean, log_scale, q, filter_var);
    let g = i * GRAD2D;
    let d_mean2 = vec2<f32>(grad(g), grad(g + 1u));
    let d_conic = vec3<f32>(grad(g + 2u), grad(g + 3u), grad(g + 4u));
    let d_op = grad(g + 5u);
    let d_color = vec3<f32>(grad(g + 6u), grad(g + 7u), grad(g + 8u));
    let d_z = grad(g + 9u);
    // The next step's sums start from zero (this was the only reader).
    clear_grads(g);
    // The pixels saw op·c3·c2 (project.wgsl): d_op is that product's gradient.
    grads[o + 10u] = (d_op * m.c3 * m.c2 + frame.reg.x) * op * (1.0 - op);
    let d_c2 = d_op * op * m.c3;
    let d_c3 = d_op * op * m.c2;
    grads[o + 11u] = d_color.x;
    grads[o + 12u] = d_color.y;
    grads[o + 13u] = d_color.z;

    // Conic → 2D covariance: dL/dΣ = −K·(dL/dK)·K with the conic K the projection stored (cpu::backward does the
    // same), instead of 1/det², which overflows for near-plane splats metres wide and amplifies cancellation when det
    // is tiny. K = (A, B; B, C); dL/dB counts B once, so (dL/dK)'s off-diagonal is half of it.
    let ka = proj[i * PROJ + 2u];
    let kb = proj[i * PROJ + 3u];
    let kc = proj[i * PROJ + 4u];
    var g00 = -(ka * ka * d_conic.x + ka * kb * d_conic.y + kb * kb * d_conic.z);
    var g01 = -(ka * kb * d_conic.x + 0.5 * (kb * kb + ka * kc) * d_conic.y + kb * kc * d_conic.z);
    var g11 = -(kb * kb * d_conic.x + kb * kc * d_conic.y + kc * kc * d_conic.z);
    // The 2D filter's compensation c2 = √(det A / det(A + F)): ∂c2/∂A = ½·c2·A⁻¹ F K, or −½·c2·K for a flat A
    // (cpu::backward).
    let h = 0.5 * d_c2 * m.c2;
    if (m.det_a > flat_det()) {
        let p = m.a.z / m.det_a;
        let qq = -m.a.y / m.det_a;
        let r = m.a.x / m.det_a;
        let fx2 = frame.reg.z;
        let fy2 = frame.reg.w;
        g00 += h * (p * fx2 * ka + qq * fy2 * kb);
        g01 += h * 0.5 * ((p * fx2 * kb + qq * fy2 * kc) + (qq * fx2 * ka + r * fy2 * kb));
        g11 += h * (qq * fx2 * kb + r * fy2 * kc);
    } else {
        g00 -= h * ka;
        g01 -= h * kb;
        g11 -= h * kc;
    }
    let j0 = m.j0;
    let j1 = m.j1;
    // dL/dΣc = Jᵀ G J.
    var d_sc: array<vec3<f32>, 3>;
    for (var r = 0u; r < 3u; r++) {
        d_sc[r] = g00 * j0[r] * j0 + g01 * (j0[r] * j1 + j1[r] * j0) + g11 * j1[r] * j1;
    }
    // dL/dJ = 2 G J Σc.
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
    for (var k = 0u; k < 3u; k++) {
        grads[o + k] = dot(column(w, k), d_pc);
    }
    // dL/dΣw = Wᵀ dL/dΣc W.
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
    // M = Rq S' (the filtered scales); dL/dM = 2 dL/dΣw M.
    let mr = array<vec3<f32>, 3>(m.rq[0] * m.sf, m.rq[1] * m.sf, m.rq[2] * m.sf);
    var dr: array<vec3<f32>, 3>;
    var d_sf = vec3<f32>(0.0);
    for (var r = 0u; r < 3u; r++) {
        let dm = 2.0 * (d_sw[r].x * mr[0] + d_sw[r].y * mr[1] + d_sw[r].z * mr[2]);
        dr[r] = dm * m.sf;
        d_sf += m.rq[r] * dm;
    }
    // s' = √(s² + σ²): ∂s'/∂log s = s²/s'; c3 = Π s/s' adds c3·σ²/s'² per axis.
    let d_log_scale = d_sf * m.s * m.s / m.sf + d_c3 * m.c3 * filter_var / (m.sf * m.sf);
    // With the scale regulariser (frame.reg.y · Σ scales), which keeps Gaussians from growing where nothing holds them.
    let d_scale = d_log_scale + frame.reg.y * m.s;
    grads[o + 3u] = d_scale.x;
    grads[o + 4u] = d_scale.y;
    grads[o + 5u] = d_scale.z;
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
    grads[o + 6u] = d_rot.x;
    grads[o + 7u] = d_rot.y;
    grads[o + 8u] = d_rot.z;
    grads[o + 9u] = d_rot.w;

    // The largest screen radius since the last relocation round, as a share of this view's width.
    var st = stats[i];
    st.z = max(st.z, proj[i * PROJ + 10u] / f32(frame.size.x));
    stats[i] = st;
}
