# The floating Orbiters panel, the watch and N4N0's menu in the play space

Date: 2026-10-30 (the watch, N4N0's menu on the lasers, its distance and grab, and the menu's bindings changed the same
evening after Enzo tried 0.3.4-dev.3). Code: `crates/orbiters-desktop/src/overlay/floating/`,
`overlay/menu.rs`, `overlay/actions.rs`.

**What it does.** The Orbiters interface that SteamVR's dashboard shows as a tab (recorder, N4N0, walk boost) also
floats in the play space as a panel, and VRChat keeps the controllers while it shows: you can walk, grab and press in
VRChat with the panel open beside you. The tab's **Float** button (beside the state pill) opens it. A watch on a wrist
(the N4N0 mark in a ring, left wrist by default) fades in as you turn the wrist to look at it, as XSOverlay's wrist
does; pointing at it with the other hand and pulling the trigger once opens **N4N0's menu** (the one the controller
binding opens: in the play space, or on the avatar's N4N0 screen when it has one), or closes it. N4N0's menu in the
play space is used with the same lasers, so VRChat keeps its input while it shows too, and it is grabbed and moved as
the panel is.

## Why the dashboard took the controllers, and why this does not

SteamVR gives the scene to one application (VRChat). While its dashboard shows, the dashboard owns the controllers'
laser and VRChat gets no input. An overlay with SteamVR's laser mouse (`SetOverlayInputMethod(Mouse)` together with
`MakeOverlaysInteractiveIfVisible`, as N4N0's menu in the play space uses) does the same whenever it is visible.

The floating panel, its bar, the watch, N4N0's menu and the lasers are plain overlays (`CreateOverlay`, not
`CreateDashboardOverlay`) with no input method and no interactive flag, so SteamVR never switches to its laser
mode for them. (Until 0.3.4-dev.3 N4N0's menu had SteamVR's laser mouse and `MAKE_OVERLAYS_INTERACTIVE_IF_VISIBLE`,
which took VRChat's input whenever it showed.) Input comes from the overlay's own action manifest instead (`overlay/actions.rs`, new set
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
| `floating/laser.rs` | Each hand's beam (device-relative to its controller, stretched with `SetOverlayTexelAspect`, turned about itself toward the eyes) and the dot where it ends. Shown only while a laser is on the panel, its bar, N4N0's menu or the watch. |
| `floating/input.rs` | Pure interaction state machine, tested without SteamVR: what each laser is on among the surfaces (the panel, N4N0's menu and what hangs under it), each surface's pointer and its button, presses on the bar and watch, grabs of the panel or the menu. |
| `floating/space.rs` | Geometry: ray against quad, carry frame, before the eyes, in view, the wrist's place, watch fade, beam. |
| `floating/mod.rs` | One turn of the overlay loop: poses, watch, interaction, panel, bar, lasers; `MouseSurfaces` turns the lasers on N4N0's menu into the mouse events SteamVR's laser would send it. |
| `overlay/menu.rs` | N4N0's menu: its overlay (no input method), `pointables()` for the lasers, `carry()` for a grab, `opening_place()`. |

`orbiters-openvr` gained `Overlay::set_texel_aspect` (IVROverlay_028 entry 18) and `Input::pose`
(IVRInput_011 entry 7, `InputPoseActionData_t`, 96 bytes on Windows, checked by `layouts_match_openvr`), both checked
against Valve's `openvr_capi.h`. `Overlay::inject_event` queues events this process makes for its own overlays;
`Overlay::poll_event` hands them out before SteamVR's (no SteamVR call), and `Event::mouse_event` builds a
`VREvent_Mouse_t` as SteamVR's laser fills it (x, y in the mouse scale from the bottom left, button 1).

## N4N0's menu in the play space

- **Where it opens.** 0.8 m before the eyes and 0.24 m below them (it was 0.5 m and 0.16 m: "in the face"), 0.9 m
  wide (it was 0.62 m), so it covers about the same angle as before (59 degrees across rather than 63) and its words
  read as large. Once moved, it opens next time where it was left as seen from the eyes (its place in the eyes' level
  frame when let go, `MenuOverlay::kept`), while that is 0.3 to 1.6 m from the eyes and facing them; this lasts for the
  SteamVR session (it is not saved).
- **Lasers.** The same lasers as the panel's: the dot shows where they point, a trigger pull presses there (the
  menu's own hover, press, the music timeline's drag and the 3D worlds panel on its canvas hear the same mouse events
  as from SteamVR's laser), a press stays on the menu's plane until the trigger is let go. Seen from behind it shows
  its back and takes no presses, as before.
- **Grab.** A grip with the laser on it, or with the hand within 8 cm of it (`GRAB_REACH_M`), takes it; held, it rides
  the hand turned toward the eyes, as the panel; let go, it stays. The 3D worlds pill or bar under it follows it.
- **The 3D worlds pill and bar** under the menu have no SteamVR input either (`worlds::interactive`): `Worlds::pointables()`
  lists them while they show and `MenuOverlay::pointables()` hands them to the lasers after the menu, so they get the
  same mouse events in their own pixels, and a grip on them moves the menu. The only overlay of Orbiters with SteamVR's
  laser mouse is the dashboard tab, which SteamVR's dashboard owns anyway.

## Calls on the menu and the panel

Measured 2026-10-30 (`cargo test -p orbiters-desktop call_page_timing -- --ignored --nocapture`, `cargo test -p
orbiters-n4n0-core steps_to_show_a_page -- --ignored --nocapture`, and `a_quick_page_is_whole_at_once…` in `orbiters-n4n0-core`
`text.rs`). Opening Calls in the play-space menu waits on nothing (the press 0.01 ms, the page's first picture 1.2 ms;
contacts are the service's, already in the snapshot). On the avatar's N4N0 screen (the owner's local values, a block
of 16 bytes in three steps of 28 to 40 ms) the Call page showed after 3 steps and was whole after 15 with nobody to
call; it is whole after 9 now: "No friend online" instead of "No friend can take a call now" (two blocks rather than
three) and no "1/1" page label while the friends fit one page (a block of its own). The page's header (its picture,
the rows hidden) still comes first, at step 3; a block takes three steps whatever it holds (the pointer and the values
never change together), so a text cannot show sooner. With four friends it is 27 steps (each name is a slot of its own
in the layout the avatar was uploaded with). The expressions menu's Call (`N4N0/Call/Menu`) wrote its list on the
synced text panel at its 300 ms step: "No friend to call right now. They need Orbiters and N4N0 on." took 8 blocks,
about 3 s. It now says "No friend online" and goes out at the phases' pace until whole (`TextSender::show_quick`): 0.41 s
rather than 0.85 s for that text from a cleared panel, 1.37 s rather than 2.69 s for four names. Friends' names still
show on the panel when its owner opens that list (it is how the radial picks a friend: there is no other display in
that menu); everyone else's view of the avatar's screen never names them, as before.

## Opening N4N0's menu with the controllers

The action manifest has two actions for it: `/actions/menu/in/open` (one press of whatever is bound opens or closes
the menu) and `/actions/menu/in/open_tap` (two quick taps on a touch sensor do: each touch 25 to 350 ms, the second
starting within 400 ms of the first's end; a sensor off for under 40 ms is one touch, so a thumb resting there, or
flickering at the sensor's edge, never opens it). The defaults (`actions::MENU_OPEN`) never click a stick, which wears
its mechanism and makes it drift:

| Controller | Default |
|---|---|
| Index (`knuckles`) | two taps on the left trackpad (touch, no press) |
| Quest and Rift Touch (`oculus_touch`) | two taps on the left thumbrest |
| Steam Frame (`frame_controller`) | two taps on the left thumbrest |
| WMR (`holographic_controller`) | two taps on the left trackpad |
| Vive wand | double press of the left trackpad (its only pad; a trackpad has no stick to drift) |
| HP Reverb G2, Vive Cosmos, Vive Focus 3, Pico | double press of the left X button (no thumbrest or spare touchpad) |

**Why a thumbrest "click" never worked.** Enzo's own binding (SteamVR's copy of it, in Steam's
`userdata/<id>/250820/remote/binding_config.json`) bound the left thumbrest's click and touch to "Open N4N0's menu".
His controllers are Quest Pro Touch on SteamVR's `oculus` driver, whose input profile
(`drivers/oculus/resources/input/touch_profile.json`) gives `/input/thumbrest` `"click": false, "touch": true`: there is
no click for SteamVR to send, whatever is bound (the Steam Frame's thumbrest is the same). The overlay reads `open` once
every turn (11 ms) on its rising edge, so a click or a double press from any input that has one does open it. People
who changed Orbiters' bindings before keep theirs: SteamVR's controller settings, Orbiters, choose the default
binding again to get these.

## Using the panel

- **Point and press.** The laser's dot shows where it points; a trigger pull presses there (egui's pointer on the
  panel). A press keeps its laser on the panel's plane until the trigger is released, so sliders drag past the edge and
  Talk stays held. It also stays where it pressed until the laser moves 8 mm (`STEADY_M`), because a trigger pull
  shakes the hand. Both hands have lasers; the pointer follows the one already on the panel until the other pulls its
  trigger. A trigger already held when the laser arrives (VRChat pickup, the Float press that opened it, or the
  dashboard returning) presses nothing; only a new pull does.
- **Move it as N4N0's screen moves.** Grip with a hand at the handle (within 12 cm) or at the panel (within 8 cm), or grip
  from a distance while the laser is on the panel or bar, or hold the trigger on the handle. Held, it rides the hand from where it was taken,
  turned toward the eyes and kept upright relative to the head: the carry frame of Orbiters Toolkit's N4N0 installer (its placement), at the
  hand, its +Z toward the eyes, the head's up. Released, it stays put. The other hand can take it over.
- **Where it opens.** It reopens where it was left (saved in `Settings::floating.placement`, a 3 × 4 matrix in
  SteamVR's standing universe) when that place is still in front of you (0.25 to 2.5 m away, within 40° of your gaze
  direction, facing you). Otherwise it opens 0.7 m ahead and 12 cm below the eyes, upright and facing them. The
  saved place is in the play space: walk boost and room recentres move it with the play space, as they move N4N0's
  menu.
- **Size and close.** The bar's minus and plus buttons step through 0.5, 0.65, 0.8, 1.0 and 1.25 m
  (`Settings::floating.width_m`); its X closes the panel with a fade.
- **From behind** it mirrors its picture so it still reads correctly, and the lasers' coordinates are mirrored back.

Settings (window › Settings › Performance, under "SteamVR dashboard tab"): **Orbiters watch** (on by default; it
opens N4N0's menu) and **Watch on** (left or right wrist). `FloatingSettings` in `orbiters-core` holds those, the
panel's width and its place.

## Frame pacing

Nothing runs while the watch is off and the panel and N4N0's menu are closed (`Floating::rest`). With the watch on, a turn reads the poses
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

Also verified here for the menu and the watch (2026-10-30, evening): `cargo test -p orbiters-desktop overlay::`
(the menu as a second surface beside the panel and its pill as a third, a press dragged off it staying on its plane,
grabs from afar, by its pill and at hand, let go when it closes; the mouse events in the menu's pixels from the bottom
left; where it opens, 0.8 m out, and where it was left as seen from the eyes after turning and walking; the taps:
two open, a resting thumb, a rest then a tap, slow taps, a flicker, three and four taps; every controller's default
binds an input its SteamVR profile has, never a stick), `cargo test -p orbiters-openvr` (`Event::mouse_event`).

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
- N4N0's menu on the lasers: that VRChat keeps its input while it shows (none of its overlays has SteamVR's input once
  the 3D worlds pill and bar drop theirs), presses and the music's drag on it, the keyboard page, grabbing it (from
  afar, at hand) and where it reopens; its 0.8 m and 0.9 m against "in the face" and readability.
- The taps: whether 25 to 350 ms touches, 400 ms apart, suit a thumbrest and an Index trackpad, and that resting the
  thumb, moving it from the stick to the rest, and VRChat's own use of the left trackpad never open the menu; the X
  double press on the controllers that have no touch sensor for it; that SteamVR lists "Open N4N0's menu (two quick
  taps on a touch sensor)" and loads the new defaults for people on Orbiters' default binding.
