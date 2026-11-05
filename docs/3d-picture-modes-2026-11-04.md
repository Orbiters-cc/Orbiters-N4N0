# 3D pictures of people, 3D pictures of the world (2026-11-04)

Enzo asked for two capture modes, for a Capture page with big clear buttons, and for the hide-self bug to be fixed. Code:
`crates/orbiters-desktop/src/controller/{hide_self.rs, around.rs, burst.rs}`, `pages/capture.rs`, `pages/home.rs`,
`overlay/ui.rs`, `crates/orbiters-core/src/picture.rs`, `crates/orbiters-n4n0-core/src/menu`.

## Why the avatar stayed in the picture

Enzo's seven recordings of 2026-11-04 from 22:14 on had Local User off. All seven wore The 1MB Ultirex
(`avtr_4f3b5bc4-87f0-4872-bc7e-22c5c071fdcf`).

| Question | Answer | Evidence |
|---|---|---|
| Did the app turn Local User on? | Yes, and VRChat took it | `parameters.bin`: initial `false`, then `true` three times; a strip on the very first frame; the next recording starts `false` again, so it was restored |
| Did that upload have the layer? | **No** | `Avatars/avtr_4f3b5bc4….json`: written 2026-11-02, `generator: Orbiters Toolkit 0.4.11`. The project's `Packages/orbiters.toolkit` is 0.4.11, with no `HideSelf` in it. 0.4.18 added the layer, on 2026-11-04 at 01:10 |
| Did VRChat ever list the parameter? | No | 172 `N4N0/*` parameters in each recording's `parameters.json`, none of them `HideSelf`. VRChat's `OSC/usr_…/Avatars/avtr_4f3b5bc4….json` (272 parameters, written 2026-11-03) has none either |
| Was the OSC list stale? | No: the upload itself has no layer | The list is older than any upload that could have one, but there is no such upload |
| Did the app flip the parameter? | No, by design at the time | The tree did not list it, so the plan was "mask on, avatar shown, warn" |
| Did it wait for proof? | No | It trusted the tree, and its only word was one toast on the computer, during a flight in a headset |
| Does the layer hide this avatar's renderers? | Yes, in a build | `real-build-lab-final.txt`: the full build of this avatar through VRCFury and d4rk's optimizer: 8 of 8 renderers in the hidden clip, 0 missed, 0 broken paths, the kit on while everything else is off. Its project has no Modular Avatar or AAO; the hook runs at 9500, after them all |

So the feature did what it was written to do, and the avatar simply lacked the layer. What failed was telling him. Now:

- **Proof.** VRChat echoes what it takes for a parameter the avatar has (`N4N0/Capture/Rig`, `Follow`, `RigX`: within milliseconds,
  in every recording above) and nothing for one it has not (the kitless avatar of 22:56: not one N4N0 parameter came back, though the
  app sent `N4N0/Capture/Record`). The app flips whatever the tree says and waits up to 0.65 s for that echo, before anything is
  recorded.
- **The reason, where he looks.** No echo: the message names the avatar and the Toolkit its manifest says (update and upload again), or,
  with a current Toolkit, VRChat's old list (Action Menu, Options, OSC, Reset Config). It shows as a toast, on the Capture page until the
  next capture, and on every menu's 3D picture page (N4N0's screen and the SteamVR overlay). The World button says so before it is pressed.
- **A listed parameter with no echo** is hidden as far as anyone can tell, and the person is told to check the picture.

The hidden meshes do not hide the kit's output: the kit's and the rig's renderers are under N4N0's `Capture` object, which
`HideSelfRenderers` leaves out, and the build's check found the kit on while hidden.

## The two pictures

| | 4 s | 8 s | |
|---|---|---|---|
| Look | 0.7 s | 0.7 s | Both: one level turn, what depth knows |
| **People**: around them | **2.9 s** (was 2.2) | **5.8 s** (was 4.0) | Rig off: every colour frame a whole picture of them |
| People: the room | 0.4 s (was 1.1) | 1.5 s (was 3.3) | Often only the way home |
| **World**: the place | **3.3 s** | **7.3 s** | Nobody flown around, rig on unless other players are here |

Why: on a real ring around a still avatar (`docs/3d-picture-angles-2026-11-04.md`), views of one side to views 13° apart gave the body
5 dB from the sides; 13° to 4° gave it 0.6 dB and the room around it 1.5 dB. A person gains most from every angle, and World has the room's
whole gain.

World's flight: a ring around the middle of what the look saw, looking across it; a turn looking up (at 8 s, down as well), which is what
shows the floor and ceiling; at 8 s, rings above and below; then hops, filling in places 0.75 m apart. Hops alone saw half the surface a
sweep around a person did on Dia de Muertos, and rings looking outward little more: a wall a metre away fills the frame.

A 1 s picture is the same gentle sweep in both, with the avatars left out in World: a person should not have to think about it.

**The wearer** hides in World whatever the mask says, as above. With the mask on, nothing is recorded until the avatar is gone.

**Other players:** `/usercamera/RemotePlayer` is the mask's other half. With anyone else in the instance (VRChat's log), World turns it off and
puts it back as it was; already off stays off, and alone it is not touched. No recording has it off with Local User on, so whether the kit survives
is unproved: without a strip for 1 s (the longest gap in 13 flown recordings was 0.44 s), or without depth after the look, it goes back on, the
person is told, and the camera looks again rather than fly blind through walls. **The rig's cameras have no such mask**
(`RigHiddenLayers` leaves the player layers in), so with other players here a World picture flies without the rig.

The mode is in `picture.json` in the session (`orbiters_core::picture`). The worker skips the steps on faces and their detection for a World
picture, and the gallery's tile says People or World.

## The page

Three big buttons: 3D picture of people, 3D picture of the world, Record. Under them, quietly, the picture's length, Scan and Test pattern, then what
was already there. While something runs, its card takes their place: what it is, who is hidden or why not, and Stop.

Home's 3D picture page and the overlay's card have the same two pictures. So does **N4N0's own screen** from version 15 (page 40), and
the **Action Menu** of an avatar uploaded with Toolkit's next version: **3D picture of people** and **3D picture of the world**, local
parameters with no synced bit. Older uploads keep what their upload baked: a screen of versions 12 to 14 has one Take with a line under it that a press
changes, and the old Action Menu's one button takes the last one chosen. The Action Menu holds eight controls, so with both Walk boost and Light those two
share a More submenu.
