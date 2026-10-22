// Projection: each Gaussian's 2D mean, conic, depth, opacity, colour, radius and tile rectangle, and how many
// tiles it touches. A viewer's guards (frame.counts.y) fade splats in over the first GUARD_FADE metres from the eye
// instead of cutting them at the near plane, drop the ones whose visible extent holds the eye (they would fill the
// view), shrink the rest to GUARD_RADIUS pixels at most, and sort by distance from the eye, which stays put as the
// view turns, instead of by depth along the view (the depth they render is that distance too). With frame.confidence
// a viewer also fades, blurs or hazes the splats the keyframes saw weakly, rarely, from one side or from much farther
// than the eye is now (common.wgsl `trust`), and fogs towards the horizon's colour, or crops, what lies beyond the reach
// of the depth frames (frame.far), the sky shell aside.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> params: array<f32>;
@group(0) @binding(2) var<storage, read_write> proj: array<f32>;
@group(0) @binding(3) var<storage, read_write> counts: array<u32>;
@group(0) @binding(4) var<storage, read> filter3d: array<f32>;
// What the keyframes saw of each Gaussian, two vec4s each (crate::gaussians::Seen: weight, keyframes, sector bits, rate
// with a negative weight where unknown; metres beyond what the depth frames reached), for a viewer's confidence control
// and far field.
@group(0) @binding(5) var<storage, read> seen: array<vec4<f32>>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = flat_index(gid, nwg);
    if (i >= frame.counts.x) {
        return;
    }
    let o = i * PROJ;
    proj[o + 15u] = 0.0;
    counts[i] = 0u;
    let b = i * PARAM;
    let mean = vec3<f32>(params[b], params[b + 1u], params[b + 2u]);
    let log_scale = vec3<f32>(params[b + 3u], params[b + 4u], params[b + 5u]);
    let q = vec4<f32>(params[b + 6u], params[b + 7u], params[b + 8u], params[b + 9u]);
    let m = proj_math(mean, log_scale, q, filter3d[i]);
    if (!m.ok) {
        return;
    }
    var a = m.cov.x;
    var bb = m.cov.y;
    var c = m.cov.z;
    let mx = frame.focal.x * m.pc.x / m.pc.z + frame.focal.z;
    let my = frame.focal.y * m.pc.y / m.pc.z + frame.focal.w;
    // The opacity the pixels see: both filters' compensations keep the Gaussian's weight.
    var op = sigmoid(params[b + 10u]) * m.c3 * m.c2;
    var depth = m.pc.z;
    var color = vec3<f32>(params[b + 11u], params[b + 12u], params[b + 13u]);
    if (frame.counts.y != 0u) {
        // From the splat to the eye, in the world.
        let w = array<vec3<f32>, 3>(frame.r0.xyz, frame.r1.xyz, frame.r2.xyz);
        let eye = -(w[0] * m.pc.x + w[1] * m.pc.y + w[2] * m.pc.z);
        // The eye inside what the splat draws: its Mahalanobis distance within the splat's reach at this opacity.
        let local = vec3<f32>(dot(column(m.rq, 0u), eye), dot(column(m.rq, 1u), eye), dot(column(m.rq, 2u), eye)) / m.sf;
        if (dot(local, local) < alpha_reach(op)) {
            return;
        }
        depth = length(m.pc);
        op *= smoothstep(frame.limits.z, GUARD_FADE, depth);
        let s = seen[2u * i];
        let sky = params[b + 14u] > 0.5;
        if (frame.far.x > 0.5 && s.x >= 0.0 && !sky) {
            let past = seen[2u * i + 1u].x / max(frame.far.y, 1e-3);
            if (frame.far.x < 1.5) {
                color = mix(color, frame.horizon.rgb, 1.0 - exp(-past));
            } else {
                op *= 1.0 - smoothstep(0.0, 1.0, past);
            }
        }
        if (frame.confidence.x > 0.0 && s.x >= 0.0 && !sky) {
            let k = frame.confidence.x * (1.0 - trust(s, eye));
            if (frame.confidence.y < 0.5) {
                op *= 1.0 - k;
            } else if (frame.confidence.y < 1.5) {
                // Wider, by a Gaussian of k · CONFIDENCE_BLUR, its weight kept as the 2D filter keeps it.
                let before = a * c - bb * bb;
                a += k * CONFIDENCE_BLUR;
                c += k * CONFIDENCE_BLUR;
                op *= sqrt(before / (a * c - bb * bb));
            } else {
                color = mix(color, frame.horizon.rgb, k);
            }
        }
    }
    // Out to α = 1/255 at this opacity (cpu::extent_sigmas); a Gaussian that never gets there draws nothing.
    let tau = alpha_reach(op);
    if (tau <= 0.0) {
        return;
    }
    let sigmas = sqrt(tau);
    if (frame.counts.y != 0u) {
        // No axis reaches further than the guard's radius: larger eigenvalues of the 2D covariance shrink to it.
        let cap = pow(min(GUARD_RADIUS, f32(min(frame.size.x, frame.size.y))) / sigmas, 2.0);
        let half = 0.5 * (a - c);
        let r = sqrt(half * half + bb * bb);
        let l1 = 0.5 * (a + c) + r;
        if (l1 > cap) {
            let l2 = 0.5 * (a + c) - r;
            var e = vec2<f32>(select(0.0, 1.0, a >= c), select(1.0, 0.0, a >= c));
            if (abs(bb) > 1e-6 * l1) {
                e = normalize(vec2<f32>(bb, l1 - a));
            }
            let n1 = min(l1, cap);
            let n2 = min(l2, cap);
            a = n1 * e.x * e.x + n2 * e.y * e.y;
            c = n1 * e.y * e.y + n2 * e.x * e.x;
            bb = (n1 - n2) * e.x * e.y;
        }
    }
    let det = a * c - bb * bb;
    let mid = 0.5 * (a + c);
    let lambda = mid + sqrt(max(mid * mid - det, 0.1));
    let radius = ceil(sigmas * sqrt(lambda));
    // The ellipse's own bounding box (per-axis extents), then only the tiles it reaches (tile_hit): flat discs seen at
    // grazing angles would otherwise fill the square of their largest axis.
    let rx = sigmas * sqrt(a);
    let ry = sigmas * sqrt(c);
    let tx = f32(frame.size.z);
    let ty = f32(frame.size.w);
    let tf = f32(TILE);
    let x0 = clamp(floor((mx - rx) / tf), 0.0, tx);
    let y0 = clamp(floor((my - ry) / tf), 0.0, ty);
    let x1 = clamp(floor((mx + rx) / tf + 1.0), 0.0, tx);
    let y1 = clamp(floor((my + ry) / tf + 1.0), 0.0, ty);
    if (x1 <= x0 || y1 <= y0) {
        return;
    }
    let conic = vec3<f32>(c / det, -bb / det, a / det);
    var hits = 0u;
    for (var row = u32(y0); row < u32(y1); row++) {
        for (var col = u32(x0); col < u32(x1); col++) {
            if (tile_hit(vec2<f32>(mx, my), conic, tau, col, row)) {
                hits++;
            }
        }
    }
    if (hits == 0u) {
        return;
    }
    proj[o] = mx;
    proj[o + 1u] = my;
    proj[o + 2u] = conic.x;
    proj[o + 3u] = conic.y;
    proj[o + 4u] = conic.z;
    proj[o + 5u] = depth;
    proj[o + 6u] = op;
    proj[o + 7u] = color.x;
    proj[o + 8u] = color.y;
    proj[o + 9u] = color.z;
    proj[o + 10u] = radius;
    proj[o + 11u] = bitcast<f32>(u32(x0));
    proj[o + 12u] = bitcast<f32>(u32(y0));
    proj[o + 13u] = bitcast<f32>(u32(x1));
    proj[o + 14u] = bitcast<f32>(u32(y1));
    proj[o + 15u] = 1.0;
    counts[i] = hits;
}
