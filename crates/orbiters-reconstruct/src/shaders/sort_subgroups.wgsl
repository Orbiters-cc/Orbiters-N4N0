// The radix sort's scatter on a training device (after sort.wgsl, whose bindings and `count` it shares): the same stable
// placement as `scatter`, each element's rank among its block's elements with its digit counted with ballots instead
// of five workgroup-wide 1-bit splits (about 90 barriers a block). Lanes of a subgroup hold consecutive elements, as
// they do on every device the trainer runs on (a one-dimensional workgroup); 256 is a multiple of every subgroup size.

var<workgroup> sub_counts: array<u32, 1024>;

// Lanes below `lane` among lanes lo..lo+31 (one word of a ballot).
fn lanes_below(lane: u32, lo: u32) -> u32 {
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
fn scatter_subgroups(
    @builtin(local_invocation_index) li: u32,
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(num_workgroups) nwg: vec3<u32>,
    @builtin(subgroup_invocation_id) lane: u32,
    @builtin(subgroup_size) lanes: u32,
) {
    let block = block_of(wid, nwg);
    let i = block * 256u + li;
    let valid = i < min(info.n, totals[1]) && block < info.blocks;
    var key = 0u;
    var val = 0u;
    var dig = 16u;
    if (valid) {
        key = keys_in[i];
        val = values_in[i];
        dig = (key >> info.shift) & 15u;
    }
    for (var k = li; k < 1024u; k += 256u) {
        sub_counts[k] = 0u;
    }
    // The lanes of this subgroup with the same digit: a ballot on each of its four bits, and one on validity.
    let b0 = subgroupBallot((dig & 1u) != 0u);
    let b1 = subgroupBallot((dig & 2u) != 0u);
    let b2 = subgroupBallot((dig & 4u) != 0u);
    let b3 = subgroupBallot((dig & 8u) != 0u);
    var same = subgroupBallot(valid);
    same &= select(~b0, b0, (dig & 1u) != 0u);
    same &= select(~b1, b1, (dig & 2u) != 0u);
    same &= select(~b2, b2, (dig & 4u) != 0u);
    same &= select(~b3, b3, (dig & 8u) != 0u);
    let below = vec4<u32>(lanes_below(lane, 0u), lanes_below(lane, 32u), lanes_below(lane, 64u), lanes_below(lane, 96u));
    let r = countOneBits(same & below);
    let rank = r.x + r.y + r.z + r.w;
    let t = countOneBits(same);
    let total = t.x + t.y + t.z + t.w;
    let sg = li / lanes;
    let subgroups = 256u / lanes;
    workgroupBarrier();
    if (valid && rank == 0u) {
        sub_counts[sg * 16u + dig] = total;
    }
    workgroupBarrier();
    // Each digit's elements in earlier subgroups: an exclusive scan over the subgroups, one thread a digit.
    if (li < 16u) {
        var acc = 0u;
        for (var s = 0u; s < subgroups; s++) {
            let c = sub_counts[s * 16u + li];
            sub_counts[s * 16u + li] = acc;
            acc += c;
        }
    }
    workgroupBarrier();
    if (valid) {
        let dst = hist[dig * info.blocks + block] + sub_counts[sg * 16u + dig] + rank;
        keys_out[dst] = key;
        values_out[dst] = val;
    }
}
