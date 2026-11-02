# Walkable colliders from a capture's exact depth

A collider someone can walk on when they open a captured world in VRChat or Unity: its floors, stairs and ramps to
stand on, its walls to bump into, nothing to fall through or snag on. Lab exporter (`crates/orbiters-reconstruct`,
`orbiters_reconstruct::mesh`); the measurements are in the lab report `reports/colliders-v2.md`.

```
orbiters-reconstruct --mesh <session> <out.glb|out.obj> [--voxel 0.04] [--quest] [--no-fill] [--radius 40]
mesh_bench <session> <out dir> [--gt DIR] [--splats PLY] [--voxel 0.04,0.05] [--quest] [--reference NAME=GLB]...
```

## What it does

1. **Walked path and reachable region** (`mesh::roi`). The path is the avatar's hips from the capture kit's skeleton
   frames when the session has them, else every recorded camera pose taken as the eyes (feet one eye height below:
   VRChat's `EyeHeightAsMeters` when recorded, else 1.6 m). The region: within 40 m of the path horizontally, from 2 m
   below the lowest feet to 4 m above the highest. Far geometry (a city skyline, mountains, a planet) is not a collider:
   the first experiment spent its whole triangle budget on a 1.17 km × 1.6 km Exoplanet Journey; this one keeps
   30 × 7 × 39 m of it. A jump of more than 2 m between recorded points is a teleport and is not walked.
2. **Sparse TSDF** (`mesh::tsdf`): 8³-voxel blocks at 4 cm, hashed, allocated only around trusted surfaces in the
   region. Depth is interpolated inside each frame's sample-grid quads as inverse depth (exact on planes) only where the
   four samples lie on one surface seen at least 4° off grazing, so a depth edge never becomes a skin. Distances are point
   to plane (along the ray times the sine to the quad's surface): a walking capture sees its floor at 3–20° off grazing,
   where projective distances jump across a voxel and the floor is lost. Samples farther than 30 m are not fused.
3. **The capturing person is carved out** where the skeleton says they stood (`mesh::fill::carve_walker`): otherwise a
   statue of them stands in the collider (VRChat Home). Without a skeleton (a camera orbiting an avatar, as in the
   Exoplanet session) the avatar is not located and stays.
4. **Floor holes are closed only where evidence says floor** (`mesh::fill`): the walkable floor is flooded from under
   the path across columns within a stair step; holes enclosed by it at one plane (≤ 4 m²), occlusion shadows behind
   posts and benches bounded by it (≤ 40 m², ≤ 15 % of the rim open), and the unobserved floor within 1.5 m of the walked
   path are filled as a thin synthetic band in voxels nothing observed. Never where the capture saw through the floor
   (free space below the plane, or a floor more than 0.3 m below), never inside obstacles, never walls.
5. **Marching cubes** (`mesh::mc`) with asymptotic-decider face decisions and fans that never share a diagonal across
   cubes: a manifold surface.
6. **Quadric-error simplification** (`mesh::simplify`): half-edge collapses under 2 cm area-weighted RMS error, each
   moved vertex within 4 cm of the planes around it, link condition (no edge ever gets a third face), no flips,
   boundaries pinned; in parallel on 8 m columns, then globally, then raising the error until the budget fits.
7. **Debris**: components smaller than 0.6 m along every axis go (particles, leaves in the air, the carved walker's
   cut-off hands).

Budgets: 150 000 triangles (PC), 50 000 (`--quest`). PhysX keeps a static triangle mesh in a BVH, so triangles cost
cooked memory (about 50 bytes each) and cooking time, not frame time; Quest gets a third for its memory and the 100 MB
world cap. The captures measured so far need 11 000–74 000 triangles at the 2 cm error, so the PC budget does not bind.

## Coordinates

GLB: metres, +Y up, the capture's Unity world X reflected and the winding reversed (UnityGLTF's and glTFast's
convention): importing it gives back the Unity world exactly (`mesh_bench` reads every GLB back that way and scores the
imported mesh; the alignment renders blend it over the game's own keyframes and the splats). One node and one mesh named
"N4N0 collider", 16-bit indices up to 65 535 vertices. In Unity: add a MeshCollider to the imported object (non-convex,
static) and disable its renderer; do not mirror it again.

## Measured

Plaza (synthetic walk, 104 ground-truth views), 4 cm, byte-identical over three runs: 8.1 ± 0.5 s (worker end to end
7.9 ± 0.3 s), 39 147 triangles, 0.57 MB.
- **Ground truth:** 92.8 % of the reachable region's true pixels covered (95.1 % within 10 m of the path, 98.0 % of
  what the capture observed: 6.5 % of the region was never seen), 88.3 % within 5 % of true depth, median error 0.05 %.
- **Walking:** a capsule walking the recorded path is supported along 100 % of it with no fall, and one connected floor
  lies under 98.4 % of the keyframe cameras.
- **Topology:** 0 non-manifold edges.

The first experiment, under the same scorer: 78–80 % coverage, 64–67 % within 5 %, 1.06 % median, a walk with 5–7
falls, 21–27 k non-manifold edges, 67–186 s. Without the floor fill: 92.4 % walk support with 2 falls. VRChat Home 6.5 s
/ 74 k triangles (Quest 50 k), Exoplanet 1.5 s / 12 k, Find the Needle 2.0 s / 63 k, all manifold. Every number, render
and its reason: the lab report `reports/colliders-v2.md`.

## Limits

- Never-observed parts of the reachable region stay open (the floor behind where a capture started, walls past 30 m).
- Without skeleton frames, a camera orbiting the person fuses their avatar as an obstacle.
- Thin structures seen from both sides (a fence, a single-plane wall) can cancel in the field.
- Unity import, PhysX cooking and a real VRChat walk were not run here; the capsule walk is the proxy.
