// Sky cubemap (lab, crate::sky): 6 faces of N×N texels (rgb in xyz) behind every splat, looked up bilinearly by world
// direction within a face; frame.counts.y = N, 0 for none (the constant background). crate::sky::face_uv mirrors the
// face mapping on the CPU.

@group(0) @binding(8) var<storage, read> sky: array<vec4<f32>>;

struct SkyTaps {
    index: vec4<u32>,
    weight: vec4<f32>,
}

// World direction of the ray through pixel (px, py).
fn sky_dir(px: f32, py: f32) -> vec3<f32> {
    let dc = vec3<f32>((px - frame.focal.z) / frame.focal.x, (py - frame.focal.w) / frame.focal.y, 1.0);
    // The rows map world to camera: a camera direction maps back through their transpose.
    return normalize(frame.r0.xyz * dc.x + frame.r1.xyz * dc.y + frame.r2.xyz * dc.z);
}

fn sky_taps(d: vec3<f32>, n: u32) -> SkyTaps {
    let a = abs(d);
    var face: u32;
    var u: f32;
    var v: f32;
    if (a.x >= a.y && a.x >= a.z) {
        face = select(1u, 0u, d.x > 0.0);
        u = select(d.z, -d.z, d.x > 0.0) / a.x;
        v = -d.y / a.x;
    } else if (a.y >= a.z) {
        face = select(3u, 2u, d.y > 0.0);
        u = d.x / a.y;
        v = select(-d.z, d.z, d.y > 0.0) / a.y;
    } else {
        face = select(5u, 4u, d.z > 0.0);
        u = select(-d.x, d.x, d.z > 0.0) / a.z;
        v = -d.y / a.z;
    }
    let nf = f32(n);
    let s = (u + 1.0) * 0.5 * nf - 0.5;
    let t = (v + 1.0) * 0.5 * nf - 0.5;
    let sf = floor(s);
    let tf = floor(t);
    let fx = s - sf;
    let fy = t - tf;
    let x0 = u32(clamp(sf, 0.0, nf - 1.0));
    let x1 = u32(clamp(sf + 1.0, 0.0, nf - 1.0));
    let y0 = u32(clamp(tf, 0.0, nf - 1.0));
    let y1 = u32(clamp(tf + 1.0, 0.0, nf - 1.0));
    let base = face * n * n;
    var taps: SkyTaps;
    taps.index = vec4<u32>(base + y0 * n + x0, base + y0 * n + x1, base + y1 * n + x0, base + y1 * n + x1);
    taps.weight = vec4<f32>((1.0 - fx) * (1.0 - fy), fx * (1.0 - fy), (1.0 - fx) * fy, fx * fy);
    return taps;
}

// What lies behind every splat at pixel (px, py).
fn background_at(px: f32, py: f32) -> vec3<f32> {
    let n = frame.counts.y;
    if (n == 0u) {
        return frame.background.xyz;
    }
    let k = sky_taps(sky_dir(px, py), n);
    return k.weight.x * sky[k.index.x].xyz + k.weight.y * sky[k.index.y].xyz + k.weight.z * sky[k.index.z].xyz + k.weight.w * sky[k.index.w].xyz;
}
