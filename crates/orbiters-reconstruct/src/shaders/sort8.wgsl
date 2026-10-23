// Stable LSD radix sort with 8-bit digits on a training device (lab, S4_SORT8): four passes instead of sort.wgsl's
// eight, 1024 elements a block (four rounds of 256), each element ranked among its block's elements with its digit by
// ballots on the digit's bits (lanes of a subgroup hold consecutive elements, as on every device the trainer runs on):
//   count8:   each block's histogram of the pass's digit → hist[digit · blocks + block]
//   (an exclusive scan of hist gives every (digit, block) its first output slot)
//   scatter8: each element to its digit's slot plus its rank among the block's elements with that digit, rounds in
//             order. The same order as sort.wgsl's (both stable), so the same sorted keys and values.

struct SortInfo {
    // Elements, blocks (of 1024), shift of this pass, unused.
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
@group(0) @binding(6) var<storage, read> totals: array<u32>;

var<workgroup> bins: array<atomic<u32>, 256>;
// Per round: each subgroup's count of each digit, then their exclusive scan (SUB8, from gpu.rs: 256 digits times the
// most subgroups a workgroup of 256 has on this adapter).
var<workgroup> sub8: array<u32, SUB8>;
// Elements of each digit in the block's earlier rounds.
var<workgroup> before8: array<u32, 256>;

fn block_of8(wid: vec3<u32>, nwg: vec3<u32>) -> u32 {
    return wid.x + wid.y * nwg.x;
}

@compute @workgroup_size(256)
fn count8(@builtin(local_invocation_index) li: u32, @builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let block = block_of8(wid, nwg);
    atomicStore(&bins[li], 0u);
    workgroupBarrier();
    let n = min(info.n, totals[1]);
    if (block < info.blocks) {
        for (var r = 0u; r < 4u; r++) {
            let i = block * 1024u + r * 256u + li;
            if (i < n) {
                atomicAdd(&bins[(keys_in[i] >> info.shift) & 255u], 1u);
            }
        }
    }
    workgroupBarrier();
    if (block < info.blocks) {
        hist[li * info.blocks + block] = atomicLoad(&bins[li]);
    }
}

fn lanes_below8(lane: u32, lo: u32) -> u32 {
    if (lane >= lo + 32u) {
        return 0xffffffffu;
    }
    if (lane <= lo) {
        return 0u;
    }
    return (1u << (lane - lo)) - 1u;
}

@diagnostic(off, subgroup_uniformity)
@compute @workgroup_size(256)
fn scatter8(
    @builtin(local_invocation_index) li: u32,
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(num_workgroups) nwg: vec3<u32>,
    @builtin(subgroup_invocation_id) lane: u32,
    @builtin(subgroup_size) lanes: u32,
) {
    let block = block_of8(wid, nwg);
    let n = min(info.n, totals[1]);
    let sg = li / lanes;
    let subgroups = 256u / lanes;
    before8[li] = 0u;
    let below = vec4<u32>(lanes_below8(lane, 0u), lanes_below8(lane, 32u), lanes_below8(lane, 64u), lanes_below8(lane, 96u));
    for (var r = 0u; r < 4u; r++) {
        // Every thread is done with the last round's scan before this one's counts start.
        workgroupBarrier();
        let i = block * 1024u + r * 256u + li;
        let valid = i < n && block < info.blocks;
        var key = 0u;
        var val = 0u;
        var dig = 0u;
        if (valid) {
            key = keys_in[i];
            val = values_in[i];
            dig = (key >> info.shift) & 255u;
        }
        for (var k = li; k < subgroups * 256u; k += 256u) {
            sub8[k] = 0u;
        }
        // The lanes of this subgroup with the same digit: a ballot on each of its eight bits, and one on validity.
        var same = subgroupBallot(valid);
        for (var bit = 0u; bit < 8u; bit++) {
            let on = ((dig >> bit) & 1u) != 0u;
            let b = subgroupBallot(on);
            same &= select(~b, b, on);
        }
        let rk = countOneBits(same & below);
        let rank = rk.x + rk.y + rk.z + rk.w;
        let tt = countOneBits(same);
        let total = tt.x + tt.y + tt.z + tt.w;
        workgroupBarrier();
        if (valid && rank == 0u) {
            sub8[dig * subgroups + sg] = total;
        }
        workgroupBarrier();
        // Thread d: digit d's elements in earlier subgroups of this round, after those of earlier rounds.
        var acc = before8[li];
        for (var s = 0u; s < subgroups; s++) {
            let c = sub8[li * subgroups + s];
            sub8[li * subgroups + s] = acc;
            acc += c;
        }
        workgroupBarrier();
        if (valid) {
            let dst = hist[dig * info.blocks + block] + sub8[dig * subgroups + sg] + rank;
            keys_out[dst] = key;
            values_out[dst] = val;
        }
        // The next round counts from here (each thread keeps its own digit's running count).
        before8[li] = acc;
    }
}
