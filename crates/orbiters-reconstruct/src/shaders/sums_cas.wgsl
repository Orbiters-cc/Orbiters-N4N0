// Gradient sums with compare-and-swap loops on the f32 bits (gpu.rs GradientSums::CompareAndSwap: any device with
// subgroups). Lane 0 adds every slot: lanes racing on neighbouring slots measured slower.

@group(0) @binding(7) var<storage, read_write> grad2d: array<atomic<u32>>;

const LANE_ADDS: bool = false;

fn add(i: u32, v: f32) {
    if (v == 0.0) {
        return;
    }
    var old = atomicLoad(&grad2d[i]);
    loop {
        let r = atomicCompareExchangeWeak(&grad2d[i], old, bitcast<u32>(bitcast<f32>(old) + v));
        if (r.exchanged) {
            break;
        }
        old = r.old_value;
    }
}
