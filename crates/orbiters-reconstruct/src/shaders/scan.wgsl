// Exclusive prefix sum of u32s, 512 per workgroup (Blelloch), with the block totals written for the next level;
// `add_sums` adds the scanned totals back; `total` writes offsets[n−1] + counts[n−1].

@group(0) @binding(0) var<uniform> info: vec4<u32>;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;
@group(0) @binding(3) var<storage, read_write> sums: array<u32>;

var<workgroup> temp: array<u32, 512>;

@compute @workgroup_size(256)
fn scan_blocks(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let block = wid.x + wid.y * nwg.x;
    let base = block * 512u;
    let t = lid.x;
    let n = info.x;
    // Blocks past the end (a two-dimensional dispatch rounds up) have nothing to scan (uniform per workgroup).
    if (base >= n) {
        return;
    }
    let i0 = base + 2u * t;
    let i1 = i0 + 1u;
    temp[2u * t] = select(0u, src[min(i0, n - 1u)], i0 < n);
    temp[2u * t + 1u] = select(0u, src[min(i1, n - 1u)], i1 < n);
    var offset = 1u;
    for (var d = 256u; d > 0u; d = d >> 1u) {
        workgroupBarrier();
        if (t < d) {
            let ai = offset * (2u * t + 1u) - 1u;
            let bi = offset * (2u * t + 2u) - 1u;
            temp[bi] = temp[bi] + temp[ai];
        }
        offset = offset << 1u;
    }
    workgroupBarrier();
    if (t == 0u) {
        sums[block] = temp[511];
        temp[511] = 0u;
    }
    for (var d = 1u; d < 512u; d = d << 1u) {
        offset = offset >> 1u;
        workgroupBarrier();
        if (t < d) {
            let ai = offset * (2u * t + 1u) - 1u;
            let bi = offset * (2u * t + 2u) - 1u;
            let x = temp[ai];
            temp[ai] = temp[bi];
            temp[bi] = temp[bi] + x;
        }
    }
    workgroupBarrier();
    if (i0 < n) {
        dst[i0] = temp[2u * t];
    }
    if (i1 < n) {
        dst[i1] = temp[2u * t + 1u];
    }
}

@compute @workgroup_size(256)
fn add_sums(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let i = gid.x + gid.y * nwg.x * 256u;
    if (i >= info.x) {
        return;
    }
    dst[i] = dst[i] + sums[i / 512u];
}

// src = counts, dst = offsets; writes the total to sums[0], the total clamped to info.y sort entries (0: no
// clamp) to sums[1], and the workgroups of a 256-wide dispatch over the clamped total to sums[2..5] (indirect
// arguments, split over two dimensions past 65535).
@compute @workgroup_size(1)
fn total() {
    let n = info.x;
    var t = 0u;
    if (n > 0u) {
        t = dst[n - 1u] + src[n - 1u];
    }
    sums[0] = t;
    var c = t;
    if (info.y > 0u) {
        c = min(t, info.y);
    }
    sums[1] = c;
    let blocks = (c + 255u) / 256u;
    sums[2] = min(blocks, 65535u);
    sums[3] = (blocks + 65534u) / 65535u;
    sums[4] = 1u;
}
