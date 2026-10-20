// Stable LSD radix sort of (key, value) pairs, 4 bits a pass, 256 elements a block:
//   count:   each block's histogram of the pass's digit → hist[digit · blocks + block]
//   (an exclusive scan of hist gives every (digit, block) its first output slot)
//   scatter: each block sorts its elements by digit in shared memory (five 1-bit splits, invalid ones last) and
//            writes each to its digit's slot plus its rank among the block's elements with that digit.

struct SortInfo {
    // Elements, blocks, shift of this pass, unused.
    n: u32,
    blocks: u32,
    shift: u32,
    pad: u32,
}

@group(0) @binding(0) var<uniform> info: SortInfo;
@group(0) @binding(1) var<storage, read> keys_in: array<u32>;
@group(0) @binding(2) var<storage, read> values_in: array<u32>;
@group(0) @binding(3) var<storage, read_write> hist: array<u32>;
@group(0) @binding(4) var<storage, read_write> keys_out: array<u32>;
@group(0) @binding(5) var<storage, read_write> values_out: array<u32>;
// The entry count written on the GPU (scan.wgsl `total`: [1] is the clamped count); info.n bounds it.
@group(0) @binding(6) var<storage, read> totals: array<u32>;

var<workgroup> counts: array<atomic<u32>, 16>;
var<workgroup> skeys: array<u32, 256>;
var<workgroup> svals: array<u32, 256>;
var<workgroup> sdig: array<u32, 256>;
var<workgroup> sflag: array<u32, 256>;
var<workgroup> starts: array<u32, 17>;

fn block_of(wid: vec3<u32>, nwg: vec3<u32>) -> u32 {
    return wid.x + wid.y * nwg.x;
}

@compute @workgroup_size(256)
fn count(@builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let block = block_of(wid, nwg);
    if (li < 16u) {
        atomicStore(&counts[li], 0u);
    }
    workgroupBarrier();
    let i = block * 256u + li;
    if (i < min(info.n, totals[1]) && block < info.blocks) {
        let d = (keys_in[i] >> info.shift) & 15u;
        atomicAdd(&counts[d], 1u);
    }
    workgroupBarrier();
    if (li < 16u && block < info.blocks) {
        hist[li * info.blocks + block] = atomicLoad(&counts[li]);
    }
}

@compute @workgroup_size(256)
fn scatter(@builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let block = block_of(wid, nwg);
    let i = block * 256u + li;
    let valid = i < min(info.n, totals[1]) && block < info.blocks;
    var key = 0xffffffffu;
    var val = 0u;
    var dig = 16u;
    if (valid) {
        key = keys_in[i];
        val = values_in[i];
        dig = (key >> info.shift) & 15u;
    }
    skeys[li] = key;
    svals[li] = val;
    sdig[li] = dig;
    // Five stable 1-bit splits on the local digit (16 = invalid sorts last).
    for (var bit = 0u; bit < 5u; bit++) {
        workgroupBarrier();
        let k = skeys[li];
        let v = svals[li];
        let d = sdig[li];
        let b = (d >> bit) & 1u;
        sflag[li] = b;
        for (var off = 1u; off < 256u; off = off << 1u) {
            workgroupBarrier();
            var s = sflag[li];
            if (li >= off) {
                s = s + sflag[li - off];
            }
            workgroupBarrier();
            sflag[li] = s;
        }
        workgroupBarrier();
        let ones_before = sflag[li] - b;
        let zeros = 256u - sflag[255];
        var pos = li - ones_before;
        if (b == 1u) {
            pos = zeros + ones_before;
        }
        workgroupBarrier();
        skeys[pos] = k;
        svals[pos] = v;
        sdig[pos] = d;
    }
    workgroupBarrier();
    let d = sdig[li];
    if (d < 16u && (li == 0u || sdig[li - 1u] != d)) {
        starts[d] = li;
    }
    workgroupBarrier();
    if (d < 16u) {
        let dst = hist[d * info.blocks + block] + (li - starts[d]);
        keys_out[dst] = skeys[li];
        values_out[dst] = svals[li];
    }
}
