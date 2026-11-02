# Metric-depth mesh experiment

This is an opt-in lab exporter, not a validated walkable VRChat world.

```
orbiters-reconstruct --mesh <session> <out.glb> [--voxel 0.05]
orbiters-reconstruct --mesh <session> <out.obj> [--voxel 0.05]
```

The exporter uses CPU fusion of depth validated by the usual preparation pipeline. Unknown, unused, sky and capture-mask samples do not contribute. It does not train splats or modify the session. Voxel size must be finite and between 0.01 and 1 metre. Default 5 cm is an experimental comparison setting, not a measured quality recommendation. Components smaller than 16 faces are removed, then vertex clustering reduces the result to at most 100,000 triangles. Separate substantial objects are retained. Fusion streams overlapping 32-cell blocks and welds boundary vertices. Weighted vertex clusters bound the intermediate mesh at about 200,000 vertices / 300,000 unique triangles before the final 100,000-triangle pass. The cluster cell doubles when this intermediate budget is reached; the requested voxel is the fusion resolution, not a promise of final edge length. Unused colour keyframes load at a maximum width of 640 to limit RAM; depth grid rays retain their exact calibration.

Both output formats use metres, +Y up, and reflected world X with reversed face winding to convert the captured Unity world to right-handed coordinates. This matches [UnityGLTF's X reflection](https://raw.githubusercontent.com/KhronosGroup/UnityGLTF/main/Runtime/Plugins/GLTFSerialization/Schema/SchemaExtensions.cs). A Unity glTF importer must perform that handedness conversion; do not manually mirror the imported mesh again. Actual Unity importer/collider cooking remains a validation requirement.

Unknown regions remain holes. Sparse distant depth, discontinuities, animated avatars/particles and vertex clustering can make unsafe collision geometry. Neither small-component cleanup nor accurate rendered depth guarantees traversability. The extractor uses face contours and fixed ambiguous-face pairing; it does not guarantee a manifold surface. This switch is intentionally independent of the trainer's defaults.

`mesh_bench <session> <output-directory> [ground-truth-directory] [splats.ply]` evaluates 2/5/10 cm at the same triangle cap. Ground truth uses `offpath`'s views.jsonl and little-endian eye-depth files. Outputs include GLB, OBJ, depth PNGs and per-view JSON. Accuracy is the mean of per-view median relative errors over covered true-depth pixels; within-5% and coverage use all valid true-depth pixels as denominator. Near-plane-clipped, two-sided CPU rasterization uses perspective-correct depth. Path probes report any near-horizontal support and support belonging to one connected near-horizontal component, within 0.2–3 m below recorded camera centres. These probes do not test capsule clearance or step height.

The optional opacity baseline is implemented here: Gaussian alpha >= 0.1, rotated ellipsoid bounds, half-voxel axis expansion, binary marching cubes, shared component cleanup and vertex clustering. It is not a reproduction of PlayCanvas's capsule carving or coplanar simplification. The thresholded ellipsoid comparator has explicit work/memory limits and fails rather than silently clipping enormous sky splats.

The benchmark also reports pooled pixel median error/coverage/within-5% across all views; per-view means remain available for comparison with `offpath`. Set `MESH_BENCH_VOXEL=0.05` to restrict a real-session sanity run to five centimetres. Failed arms write explicit `.error.json` files and do not suppress the remaining arms.

The opacity baseline excludes the PLY's explicitly flagged sky splats (5,663 of 970,341 in the trained plaza); depth fusion also excludes sky. It does not drop any unflagged splat or crop to a hand-selected box. The raw sky shell reaches about 400 m and has scales up to 44 m, so treating it as collision would be physically inappropriate as well as wasteful.

Initial private-build plaza result (one run, all 104 true-depth views): at 5 cm, 91,069 triangles, 1,581,296 bytes, 55.83 s fusion/export plus 2.01 s loading, pooled median relative depth error 0.805%, completeness 76.36%, and 64.01% of true pixels within 5%. Only 70.87% of path cameras have supporting floor. The render shows floor holes. This prototype therefore fails the walkable-collider acceptance gate; it remains an explicit lab command.

The report and reproducible measurements are in the lab's `reports/colliders.md` and `reports/colliders/`. Actual capsule traversal, doorway/stair fixtures, transient-debris rejection, and Unity collider cooking remain validation requirements. Coordinate round-trip tests and a mesh/splat point overlay establish native-world alignment, not Unity physics behaviour.
