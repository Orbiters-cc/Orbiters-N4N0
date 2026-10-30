# The floating Orbiters panel and the watch

Date: 2026-10-30. Code: `crates/orbiters-desktop/src/overlay/floating/`.

**What it does.** The Orbiters interface that SteamVR's dashboard shows as a tab (recorder, N4N0, walk boost) also
floats in the play space as a panel, and VRChat keeps the controllers while it shows: you can walk, grab and press in
VRChat with the panel open beside you. A watch on a wrist (the N4N0 mark in a ring, left wrist by default) fades in as
you turn the wrist to look at it, as XSOverlay's wrist does; pointing at it with the other hand and pulling the trigger
once opens the panel before you, or closes it. The tab's **Float** button (beside the state pill) opens it too.

## Why the dashboard took the controllers, and why this does not

SteamVR gives the scene to one application (VRChat). While its dashboard shows, the dashboard owns the controllers'
laser and VRChat gets no input. An overlay with SteamVR's laser mouse (`SetOverlayInputMethod(Mouse)` together with
`MakeOverlaysInteractiveIfVisible`, as N4N0's menu in the play space uses) does the same whenever it is visible.

The floating panel, its bar, the watch and the lasers are plain overlays (`CreateOverlay`, not
`CreateDashboardOverlay`) with no input method and no interactive flag, so SteamVR never switches to its laser
mode for them. Input comes from the overlay's own action manifest instead (`overlay/actions.rs`, new set
`/actions/panel`): each hand's laser pose bound to `/user/hand/<hand>/pose/tip` (where SteamVR's own laser starts,
read with `GetPoseActionDataRelativeToNow`), each hand's grip (`grab_<hand>`: an Index's squeeze, a wand's grip
button, the others' grip past its click), and the existing trigger presses (`/actions/menu/in/press_<hand>`). SteamVR
gives an overlay application's actions to the scene application as well, so VRChat still gets the same trigger and
grip. Our lasers act only while they hit our own overlays. A trigger pulled on the panel also reaches VRChat, as with
XSOverlay.

## Pieces

| File | What |
|---|---|
| `floating/panel.rs` | The panel: `overlay::ui::OverlayUi::show_in(.., Surface::Floating)` (the tab's interface and state, its own egui context and CPU painter with the tab's backdrop), 1024 × 640 px, 0.8 m wide by default. Under it, a bar like N4N0's screen handle: smaller, larger, the handle, close. |
| `floating/watch.rs` | The watch: 4.5 cm, device-relative to the chosen hand's controller (`space::on_wrist`), faded with a spring from `space::watch_shown`. |
| `floating/laser.rs` | Each hand's beam (device-relative to its controller, stretched with `SetOverlayTexelAspect`, turned about itself toward the eyes) and the dot where it ends. Shown only while a laser is on the panel, its bar or the watch. |
| `floating/input.rs` | Pure interaction state machine, tested without SteamVR: what each laser is on, the panel pointer and its button, presses on the bar and watch, grabs. |
| `floating/space.rs` | Geometry: ray against quad, carry frame, before the eyes, in view, the wrist's place, watch fade, beam. |
| `floating/mod.rs` | One turn of the overlay loop: poses, watch, interaction, panel, bar, lasers. |

`orbiters-openvr` gained `Overlay::set_texel_aspect` (IVROverlay_028 entry 18) and `Input::pose`
(IVRInput_011 entry 7, `InputPoseActionData_t`, 96 bytes on Windows, checked by `layouts_match_openvr`), both checked
against Valve's `openvr_capi.h`.

## Using it

- **Point and press.** The laser's dot shows where it points; a trigger pull presses there (egui's pointer on the
  panel). A press keeps its laser on the panel's plane until the trigger is released, so sliders drag past the edge and
  Talk stays held. It also stays where it pressed until the laser moves 8 mm (`STEADY_M`), because a trigger pull
  shakes the hand. Both hands have lasers; the pointer follows the one already on the panel until the other pulls its
  trigger. A trigger already held when the laser arrives (VRChat pickup, the watch press that opened it, or the
  dashboard returning) presses nothing; only a new pull does.
- **Move it as N4N0's screen moves.** Grip with a hand at the handle (within 12 cm), or grip from a distance while the
  laser is on the panel or bar, or hold the trigger on the handle. Held, it rides the hand from where it was taken,
  turned toward the eyes and kept upright relative to the head: the carry frame of `OrbInstaller.Placement.cs`, at the
  hand, its +Z toward the eyes, the head's up. Released, it stays put. The other hand can take it over.
- **Where it opens.** It reopens where it was left (saved in `Settings::floating.placement`, a 3 × 4 matrix in
  SteamVR's standing universe) when that place is still in front of you (0.25 to 2.5 m away, within 40° of your gaze
  direction, facing you). Otherwise it opens 0.7 m ahead and 12 cm below the eyes, upright and facing them. The
  saved place is in the play space: walk boost and room recentres move it with the play space, as they move N4N0's
  menu.
- **Size and close.** The bar's minus and plus buttons step through 0.5, 0.65, 0.8, 1.0 and 1.25 m
  (`Settings::floating.width_m`); its X closes the panel with a fade.
- **From behind** it mirrors its picture so it still reads correctly, and the lasers' coordinates are mirrored back.

Settings (window › Settings › Performance, under "SteamVR dashboard tab"): **Orbiters watch** (on by default) and
**Watch on** (left or right wrist). `FloatingSettings` in `orbiters-core` holds those, the panel's width and its place.

## Frame pacing

Nothing runs while the watch is off and the panel closed (`Floating::rest`). With the watch on, a turn reads the poses
once, predicted to the next photons (`DisplayFrequency`, `SecondsFromVsyncToPhotons`, `GetTimeSinceLastVsync`). It
computes the watch's fade and makes SteamVR calls only for changes: the alpha when it moves by 1 %, hidden at 0. The
actions are read only while something can be pointed at. The panel paints like the tab: when the laser, the
service's news or an egui animation asks, and only changed tiles. It paints at most 45 pictures a second with a laser
on it and 10 without, because it stays open during the game. The bar and watch paint only when their hover or state
changes. The lasers' pictures are painted once. A beam's transform is sent when it moved 1 mm or turned about 2°.
All of this runs on the existing overlay loop. It adds no thread, wait or GPU work.

## Verified here (no headset)

- Unit tests (`cargo test -p orbiters-desktop floating`, 25 tests) cover ray hits and misses, the plane hit of a
  held press, back-side hits, the carry frame (the panel follows, turns to the eyes and returns where taken), opening
  in or out of view, the watch's placement (a rotation, out of the back of the hand, clear of the coverage panel on
  the same controller) and fade (shown when the wrist faces the eyes and they look at it, gone otherwise), the beam's
  frame and stretch, pointer and press behavior (press there only, no press on re-entry with a held trigger, steady
  press, drag past the edge, lost hand, two hands), grabs (handle reach, grip from a distance, trigger on the handle,
  release), the watch pressed by the other hand's laser or fingertip (not its own, not 3 cm beside it), the bar's
  buttons where they are drawn, widths, egui's pointer events, and the pictures (beam, dot, watch face).
- `bindings_are_json` checks the new actions and every controller's bindings; `layouts_match_openvr` checks
  `InputPoseActionData_t`.
- Screenshots: `orbiters-desktop --screenshot <dir> --only floating` (`floating-panel*.png`: the panel with a laser's
  dot and its bar, as they hang; `floating-bar*.png`; `floating-watch.png`; `floating-laser.png`), `--only overlay`
  (the tab's Float button), `--only settings-watch` (the two switches).

## What needs a headset

- That the lasers' default bindings load: `pose/tip` for every controller type, the grip modes (`grab` on Index,
  `button` on wands, `trigger` click elsewhere), and that people who changed Orbiters' bindings get the new set's
  defaults. SteamVR's binding UI lists them under Orbiters ("Orbiters panel").
- That VRChat really keeps its input while the panel shows (expected: no dashboard, no laser mouse), and how a trigger
  pull on the panel combines with whatever VRChat does with it at that moment.
- The beam's stretch through the texel aspect, its turn toward the eyes, and its fit on the controller (device-relative
  from the tip pose); that the dot sits on the panel without fighting it.
- The watch's place on the wrist (`space::on_wrist`: 18 cm toward the elbow, 3.5 cm down and out of the back of the
  hand, face tilted up and toward the elbow) on Index, Touch and Vive controllers, and the fade thresholds
  (`watch_shown`: face toward the eyes from 0.35 to 0.7 cosine, gaze within about 41° to 26°).
- The grab's feel compared with N4N0's screen, the handle's 12 cm reach, the 8 mm steadiness, and the panel's size
  and distance.
- The laser's timing: poses predicted to the photons, panel moves made once per turn of the overlay loop (11 ms, or
  once per compositor frame during walk boost).
