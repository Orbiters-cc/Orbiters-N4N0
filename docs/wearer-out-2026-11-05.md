# The wearer out of a picture, without a re-upload (2026-11-05)

An avatar uploaded with a Toolkit before 0.4.18 has no hiding layer, and nothing from outside can change an uploaded avatar. So
the capture records it, and reconstruction takes it out (`crates/orbiters-reconstruct/src/wearer.rs`).

**When.** `orbiters_core::picture::wearer_goes`: the person's choice (`wearer.json`, written by Reconstruct without or with my
avatar), else `picture.json` saying the wearer is in a picture they were asked out of, else a recording that began with Local User
off and never echoed `N4N0/Capture/HideSelf`.

**How.**
1. The band gives the hips' position and bone rotations only: no bone lengths or rest pose, so it says where the body stands and
   when it moved, and exact depth says what it is.
2. Depth samples around the hips go into voxels. Flat surfaces grow in from outside (walls, floors), each along its own plane. The
   body grows out from a core around the hips through what is left, across gaps of a voxel, never along the ground.
3. Samples on the body become unknown: no seeds, no depth targets. Pixels showing it get alpha 0 and leave the loss
   (`wearer::LossMask`), the seeds' colours and the sky's.
4. When training ends, what lies in the body's volume, and what around it no pixel left in showed, goes. Holes the floor encloses
   take the floor's colour around them (`wearer::patch_floor`).

**What is said.** `reconstruction::Report::wearer` and `wearer_not_found`, shown by the capture window and the session's detail;
Capture says it beforehand and offers Update the Toolkit (`pages/projects/toolkit_update.rs`), which installs the current Toolkit in
the avatar's project and opens it in Unity. The app uploads nothing.

**Limits.** The shadow and reflections stay. Someone touching the wearer goes with them, up to the envelope; a hand's width away
they stay. A moving wearer is only covered by a unit test: none of the recordings has one in view.

`cargo run --release --example wearer_masks` and `wearer_views` draw the masks and the result from fixed cameras.
`orbiters-desktop --screenshot <out> --sessions <disposable library> --only capture-wearer` presses through it on the real worker.
Measurements: the lab's `reports/wearer-out.md`.
