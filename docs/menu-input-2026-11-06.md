# Menu input: VRChat's contact cap — 6 November 2026

A pointer could show on N4N0's screen and press nothing, in regions of the screen that came and went with distance, angle and
the menu's place. The October fixes (`menu-input-2026-10-30.md`) were real but downstream of this.

**The cause.** VRChat pairs contacts per cell of a world-fixed 5 m grid, sweeping along the world's X
(`VRC.Dynamics.CollisionBroadphase_HybridSAP.UpdateGridCellsJob.FindPairs`). A cell stops at 1024 pairs, tags or not, and every shape
later in the sweep finds nothing that frame. N4N0's 28 large receivers made some 770 pairs alone, so one other person near reached
the cap and two or three went past it. A sender past the end reads 0 on all four receivers; its beam, a shader, goes on showing.

**Seen in a real recording** (`After The Rain 2026-11-06`, four people in the instance): the left fingertip silent for all 9.2 s with
its aim and the right hand reported, the left aim dropping three times with the hand still, the eyes silent, the gaze point silent but
for one frame. Reported senders solve to 2.00 mm and 0.300 m apart, within 5 microns: no scale or offset error at the default size.

**Reproduced with VRChat's compiled code** (its DLL fed N4N0's real contacts, other avatars modelled): alone 770 pairs and nothing
dropped; with two or three near, whole senders drop by facing, and the map over the screen is a dead region that grows with
distance. Lab and pictures: `reports/menu-hands.md`.

**The fix, on the avatar (screen version 16).** What only measures the wearer has a copy 100 m below the avatar, under one parent so
that one position offset moves them all: it reads the same and shares its cell with no one.

**The fix, in the app.** A sender's radius is known, so it is found by three linear equations with no root to choose (the free radius
was singular on a surface just behind the screen, which turned 5 microns into 11 mm; that loses at most 0.6 % of positions at 0 to 10
cm and was not the main cause). Readings wait for their frame and those of two frames are refused. A sender gone rides on
the other for 1.2 s. Each hand hovers for itself. Buttons take a press within 28 pixels, and a trigger presses what the ray rested
on before the pull. A fast poke presses, and a screen gliding onto a finger does not.

**Telling what happened.** `n4n0-pointer.log` in the app's logs says what each hand's senders say (`found`, `silent`, `partial`, `mixed`)
when that changes, what became of each trigger pull and press, and how the readings went every five seconds. A sender `silent`
with people near on an upload before version 16 is the cap; no `trigger` line for a pull is SteamVR's actions.

**Not provable without a headset:** that the running client behaves as its SDK's DLL does, and that contacts 100 m under the avatar
work in it. Both show in the log at the first test.
