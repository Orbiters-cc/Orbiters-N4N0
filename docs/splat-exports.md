# Export a reconstruction

The worker can export an Orbiters reconstruction PLY without loading the GPU:

```
orbiters-reconstruct --export-gltf <splats.ply> <out.glb>
orbiters-reconstruct --export-spz <splats.ply> <out.spz>
```

Both exports contain degree-zero splats only. Sky-shell discs marked by Orbiters and the separate sky panorama are omitted. The input is the binary reference-layout PLY accepted by the worker, not an arbitrary polygon mesh.

GLB uses the ratified `KHR_gaussian_splatting` ellipse kernel, perspective projection, and camera-distance sorting declaration. Positions are right-handed Y-up (Unity Z negated), rotations are normalised xyzw, scale is linear, opacity is sigmoid-activated, and colour is SH0 in `srgb_rec709_display`. Blend in the declared space before display conversion. Different renderers can still differ in sorting, filtering and cutoff; exact image equivalence is not promised.

SPZ writes version 4: a plaintext 32-byte header, a five-entry table of contents for degree zero, and independent Zstandard streams. Positions are signed 24-bit fixed point with 12 fractional bits in RUB coordinates; positions outside that range are rejected. The standard quantises log scale to 8 bits, opacity and SH0 colour to 8 bits, and xyzw rotation to a 32-bit smallest-three quaternion. Log scales outside [-10, 5.9375] and SH0 values outside the standard's representable range saturate, as in the reference encoder. The CLI prints the number of affected components; use GLB when their full range matters. SPZ v4 requires an updated reader; it is not the draft glTF SPZ-v2 compression extension.

No runtime dependency or desktop export UI is introduced. `serde_json`, `zstd` and the lab example's `half` reuse versions already in Cargo.lock. The independent-reader results and compact tree proposal are in the compact-formats lab report; the deployed tree remains version 1.
