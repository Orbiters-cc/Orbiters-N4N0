// Projection backward: each Gaussian's 2D gradients (grad2d) to gradients of its parameters (grads, laid out like
// params), and the densification statistics (stats: accumulated screen-space gradient norm, visible count,
// largest radius, unused). The gradient buffer (binding 3) and `grad` come from one of the sums_read_*.wgsl variants.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> params: array<f32>;
@group(0) @binding(2) var<storage, read> proj: array<f32>;
@group(0) @binding(4) var<storage, read_write> grads: array<f32>;
@group(0) @binding(5) var<storage, read_write> stats: array<vec4<f32>>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= frame.counts.x) {
        return;
    }
    let o = i * PARAM;
    for (var k = 0u; k < PARAM; k++) {
        grads[o + k] = 0.0;
    }
    if (proj[i * PROJ + 15u] == 0.0) {
        return;
    }
    let mean = vec3<f32>(params[o], params[o + 1u], params[o + 2u]);
    let log_scale = vec3<f32>(params[o + 3u], params[o + 4u], params[o + 5u]);
    let q = vec4<f32>(params[o + 6u], params[o + 7u], params[o + 8u], params[o + 9u]);
    let m = proj_math(mean, log_scale, q);
    let g = i * GRAD2D;
    let d_mean2 = vec2<f32>(grad(g), grad(g + 1u));
    let d_conic = vec3<f32>(grad(g + 2u), grad(g + 3u), grad(g + 4u));
    let op = proj[i * PROJ + 6u];
    // The opacity regulariser (frame.reg.x · opacity) lets Gaussians nothing needs fade out, to be relocated.
    grads[o + 10u] = (grad(g + 5u) + frame.reg.x) * op * (1.0 - op);
    grads[o + 11u] = grad(g + 6u);
    grads[o + 12u] = grad(g + 7u);
    grads[o + 13u] = grad(g + 8u);
    let d_z = grad(g + 9u);

    // Conic → 2D covariance.
    let a = m.cov.x;
    let b = m.cov.y;
    let c = m.cov.z;
    let det = a * c - b * b;
    let inv2 = 1.0 / (det * det);
    let da = inv2 * (-c * c * d_conic.x + b * c * d_conic.y - b * b * d_conic.z);
    let db = inv2 * (2.0 * b * c * d_conic.x - (det + 2.0 * b * b) * d_conic.y + 2.0 * a * b * d_conic.z);
    let dc = inv2 * (-b * b * d_conic.x + a * b * d_conic.y - a * a * d_conic.z);
    let g00 = da;
    let g01 = 0.5 * db;
    let g11 = dc;
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
    // M = Rq S; dL/dM = 2 dL/dΣw M.
    let mr = array<vec3<f32>, 3>(m.rq[0] * m.s, m.rq[1] * m.s, m.rq[2] * m.s);
    var dr: array<vec3<f32>, 3>;
    var d_log_scale = vec3<f32>(0.0);
    for (var r = 0u; r < 3u; r++) {
        let dm = 2.0 * (d_sw[r].x * mr[0] + d_sw[r].y * mr[1] + d_sw[r].z * mr[2]);
        dr[r] = dm * m.s;
        d_log_scale += m.rq[r] * dm * m.s;
    }
    // The scale regulariser (frame.reg.y · Σ scales) keeps Gaussians from growing where nothing holds them back.
    // Lab: flatness (frame.background.w · the thinnest scale, as 2DGS/PGSR flatten Gaussians onto surfaces).
    var flat = vec3<f32>(0.0);
    if (m.s.x <= m.s.y && m.s.x <= m.s.z) {
        flat.x = m.s.x;
    } else if (m.s.y <= m.s.z) {
        flat.y = m.s.y;
    } else {
        flat.z = m.s.z;
    }
    let d_scale = d_log_scale + frame.reg.y * m.s + frame.background.w * flat;
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

    // Densification statistics: the screen-space gradient in normalised device units, as 3DGS measures it.
    let ndc = d_mean2 * 0.5 * vec2<f32>(f32(frame.size.x), f32(frame.size.y));
    var s = stats[i];
    s.x += length(ndc);
    s.y += 1.0;
    s.z = max(s.z, proj[i * PROJ + 10u]);
    stats[i] = s;
}
