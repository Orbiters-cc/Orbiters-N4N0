# The Orbiters menu around VRChat's own menus

Code: `crates/orbiters-desktop/src/overlay/orbit/`. Settings, Performance, "Orbiters menu in VRChat".

**What it does.** When VRChat's Action Menu wheel or Quick Menu opens, Orbiters' panels appear around it, in the places Enzo drew: 3D pictures, record, recent captures, N4N0's pages, talk, music. They go when the menu goes. N4N0's pages open in front of the Quick Menu. It is the default; floating and the avatar's screen remain selectable.

**No mod.** Nothing is injected, the game's files and memory are not touched. In desktop mode each panel is an ordinary topmost window; in VR an ordinary SteamVR overlay.

## Knowing a menu is open

VRChat says nothing: OSC reports no menu state, its log nothing when a menu opens, and SteamVR no other application's action sets.

| | Desktop mode | VR |
| --- | --- | --- |
| What starts it | Esc or R, read with `GetAsyncKeyState` while VRChat is in front | VRChat's menu button of either hand, through the overlay's own actions, as VRChat's bindings have it |
| What decides | **VRChat's window's picture** (Windows Graphics Capture, while looking or open only): the wheel's disc at its exact place, the Quick Menu's row of icons by shape | Nothing further: short press the Quick Menu, held the wheel |
| It closes when | The picture no longer shows it, two looks in a row (about 200 ms), or no new frame for 600 ms | The button, a world change, the dashboard, VRChat losing the scene, 90 s with no laser on a panel |
| Can it be wrong? | Late, not wrong: a gamepad or OSC opening it has no key to start the looking, so nothing shows | **Yes:** VRChat's menu closing by another way leaves the panels until they time out. After that the next press counts as closing |

An eye picture could settle VR the way the window's does (`orbiters_openvr::mirror`), once a menu has been measured in a headset.

## Where they go

`zones.rs`. Measured on the real game at 10 window sizes: the wheel scales with the window's height, and sits 0.1556 of it right of the middle and 0.1583 below; the Quick Menu scales with the height down to 16:9, less below that (`aspect^0.28`), centred at 0.55 of the height. Zones are in the menu's own units, so one set of numbers serves every window. A zone the window cuts by more than 28% does not show. The three panels hugging the wheel have an edge concentric with it, and that part is no part of the panel.

In VR the same units are laid on a plane: the wheel's rides the hand facing the eyes, the Quick Menu's stays where it opened. **Sizes and offsets in VR are estimates** (`vr.rs`), adjustable by Settings' size until measured.

## Pointing, and who gets the press

- **Quick Menu, desktop:** the mouse is free. A click on a panel's window stays there, measured: the button under it does not act and VRChat keeps the foreground. VRChat's hover does follow the pointer under a panel, which does nothing.
- **Wheel, desktop:** VRChat holds the mouse (clipped to one pixel; its movement picks a slice), so nothing can hover. **Holding Left Alt** points at the panels: a low-level mouse hook, set only while Alt is held with the wheel open, keeps the mouse from the game and moves a pointer of ours. Let go, the mouse is VRChat's.
- **VR:** the controllers' own lasers. While a laser is on a panel, that hand's trigger is taken from VRChat: an action set binding the trigger, active for that hand at `k_nActionSetOverlayGlobalPriorityMin + 1`. It needs SteamVR's developer setting "Experimental overlay input overrides" (`steamvr.globalActionSetPriority`), which the overlay turns on once and Settings reports.

The block's rules (`block.rs`): blocked on hover, before any press. A press begun in the game stays the game's until let go; a press begun on a panel stays ours until let go. Off the panel, free at once; panels gone, free at once whatever is held. It is said anew every frame, so it ends by not being said, and SteamVR forgets it with the process. If the overlay's loop hangs with a block on, the process ends itself after 1.5 s.

## Cost

Closed: no window or overlay shows, nothing is painted, no capture runs; desktop mode asks for two keys' state every 33 ms. See `reports/orbit-overlays.md` for what was measured.

## Testing

- `cargo test -p orbiters-desktop overlay::orbit`: placement, hover, the block's state machine, the detector on real frames (`orbit/testdata`, made by `scripts/orbit-fixtures.py`).
- `orbiters-desktop --screenshot <folder> --only orbit`: every panel's state.
- The real game: `docs/vrchat-bridge.md`, "Running it live". The bridge's screenshot shows VRChat's window only, so what the panels draw is checked by capturing that screen region.
