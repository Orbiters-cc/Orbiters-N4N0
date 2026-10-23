// Lazily dense Adam (lab, S4_LAZY_ADAM): what dense Adam does to a Gaussian no step shows (its moments decay and its
// momentum keeps moving it) is applied once the next step shows it (`project_lazy`), or when something reads the whole
// set (`catch_up_all`), instead of every step; Adam and the projection backward skip the Gaussians a step does not show.
// The same updates up to float rounding, for the cost of the Gaussians each step shows. The step a Gaussian's Adam last
// ran at is kept in its first moment's slot 14 (bits; no learning rate moves the sky flag).
// Needs common.wgsl (Frame, flat_index), and the parameter, moment and Adam bindings below.

struct AdamInfo {
    lr: array<vec4<f32>, 4>,
    // β1, β2, ε, the positions' learning rate's log decay a step (crate::train: 0.01 over the run).
    beta: vec4<f32>,
    // Parameters (floats), this step (the one about to run, or the last one run for `catch_up_all`), lazy flag, unused.
    n: vec4<u32>,
}

@group(0) @binding(6) var<storage, read_write> m1: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> m2: array<vec4<f32>>;
@group(0) @binding(8) var<uniform> adam: AdamInfo;

// At most this many missed steps move a Gaussian (each moves it (β1/√β2)^j as far as the first: past 128, under 2e-6);
// the rest only decay its moments.
const CATCH_UP_MOVES: u32 = 128u;

// Brings Gaussian i's parameters and moments from step `last` (its Adam's last) to step `to` as dense Adam would with
// no gradient, and records `to` as its last step. Returns whether its parameters moved.
fn catch_up(i: u32, to: u32) -> bool {
    let o = 4u * i;
    var a0 = m1[o];
    var a1 = m1[o + 1u];
    var a2 = m1[o + 2u];
    var a3 = m1[o + 3u];
    let last = bitcast<u32>(a3.z);
    if (last >= to) {
        return false;
    }
    a3.z = 0.0;
    var b0 = m2[o];
    var b1 = m2[o + 1u];
    var b2 = m2[o + 2u];
    var b3 = m2[o + 3u];
    let zero = vec4<f32>(0.0);
    let moved = any(a0 != zero) || any(a1 != zero) || any(a2 != zero) || any(a3 != zero);
    if (moved) {
        var p0 = params4[o];
        var p1 = params4[o + 1u];
        var p2 = params4[o + 2u];
        var p3 = params4[o + 3u];
        let restart = p3.w;
        let beta1 = adam.beta.x;
        let beta2 = adam.beta.y;
        let eps = adam.beta.z;
        let k = to - last;
        let moves = min(k, CATCH_UP_MOVES);
        for (var j = 1u; j <= moves; j++) {
            let t = last + j;
            let te = max(f32(t) - restart, 1.0);
            let c1 = 1.0 - pow(beta1, te);
            let c2 = 1.0 - pow(beta2, te);
            // The positions' rate at step t: this step's, undone back to t.
            let lm = adam.lr[0].x * exp(adam.beta.w * (f32(t) - f32(adam.n.y)));
            let l0 = vec4<f32>(lm, lm, lm, adam.lr[0].w);
            a0 *= beta1;
            a1 *= beta1;
            a2 *= beta1;
            a3 *= beta1;
            b0 *= beta2;
            b1 *= beta2;
            b2 *= beta2;
            b3 *= beta2;
            p0 -= l0 * (a0 / c1) / (sqrt(b0 / c2) + eps);
            p1 -= adam.lr[1] * (a1 / c1) / (sqrt(b1 / c2) + eps);
            p2 -= adam.lr[2] * (a2 / c1) / (sqrt(b2 / c2) + eps);
            p3 -= adam.lr[3] * (a3 / c1) / (sqrt(b3 / c2) + eps);
        }
        if (k > moves) {
            let d1 = pow(beta1, f32(k - moves));
            let d2 = pow(beta2, f32(k - moves));
            a0 *= d1;
            a1 *= d1;
            a2 *= d1;
            a3 *= d1;
            b0 *= d2;
            b1 *= d2;
            b2 *= d2;
            b3 *= d2;
        }
        params4[o] = p0;
        params4[o + 1u] = p1;
        params4[o + 2u] = p2;
        params4[o + 3u] = p3;
        m2[o] = b0;
        m2[o + 1u] = b1;
        m2[o + 2u] = b2;
        m2[o + 3u] = b3;
    }
    a3.z = bitcast<f32>(to);
    m1[o] = a0;
    m1[o + 1u] = a1;
    m1[o + 2u] = a2;
    m1[o + 3u] = a3;
    return moved;
}
