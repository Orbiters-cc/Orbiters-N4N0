// Each tile's slice of the sorted entries: ranges[2·tile] = first, ranges[2·tile + 1] = one past the last.

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<storage, read> keys: array<u32>;
@group(0) @binding(2) var<storage, read_write> ranges: array<u32>;
// The entry count written on the GPU (scan.wgsl `total`: [1] is the clamped count).
@group(0) @binding(3) var<storage, read> totals: array<u32>;

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let j = flat_index(gid, nwg);
    let total = totals[1];
    if (j >= total) {
        return;
    }
    let bits = frame.counts.z;
    let tile = keys[j] >> bits;
    if (j == 0u || (keys[j - 1u] >> bits) != tile) {
        ranges[2u * tile] = j;
    }
    if (j == total - 1u || (keys[j + 1u] >> bits) != tile) {
        ranges[2u * tile + 1u] = j + 1u;
    }
}
