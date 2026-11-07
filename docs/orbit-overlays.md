# The Orbiters menu around VRChat's own menus

Code: `crates/orbiters-desktop/src/overlay/orbit/`. Settings, Performance, "Orbiters menu in VRChat".

**What it does.** When VRChat's Action Menu wheel or Quick Menu opens, Orbiters' parts appear around it, and go when it goes. It is the default; floating and the avatar's screen remain selectable, and the wheel and Quick Menu turn on and off separately.

- **Around the wheel:** a ring of six equal cells on VRChat's own 60 degree slices, 1.72 times the wheel's radius, their names bent along the outer edge letter by letter (`panels::curved`): Worlds in 3D, YouTube, Now playing, All tools, Instances, 3D pictures. Pointing at 3D pictures blooms a second ring (of people, of this world, Record); Now playing blooms previous and next.
- **Around the Quick Menu:** one wing docked to My Profile's outer edge, with the same tiles, Ask N4N0 and what plays. With no room beside it, the same tiles lie in a strip under VRChat's tab row, as no part covers VRChat's panels at rest.
- **In front of the Quick Menu:** one sheet with its name in its head, ending at the main panel's own bottom edge so that VRChat's tab row stays: All tools (sized by what matters, what plays in full on it), the gallery, Previous instances (a row for each world), Worlds in 3D (which opens on its 3D captures) and N4N0's pages.

**The look.** Orbiters' own design system, a bit more modern than VRChat's, and sitting well beside it: filled surfaces with no outlines, line icons, mint for what is pointed at or chosen, and the Orbiters mark on each surface. It takes scale, placement and density from VRChat and never its materials: nobody should take it for VRChat's own (`reports/ui-loop/round-0/synthesis.md`).

**No mod.** Nothing is injected, the game's files and memory are not touched. In desktop mode each panel is an ordinary topmost window; in VR an ordinary SteamVR overlay.

## On the desktop

A capture's press shows it large on the sheet, with the Orbiters window as the next step, as nothing shows in 3D in the game there. While a 3D picture waits for VRChat's camera the veil lies over VRChat's wheel, whose lit slice has nothing to do with it.

## Sizes in VR

A sheet is used at about 0.55 m, where one of its page's pixels is 0.62 mm. Nothing that can be pressed on a sheet or a world's side panel is under 2 degrees on its short side, and tests hold to that. Text on them is about 1 degree at least.

## How they follow the hand in VR

Rigid parts and sheets follow a filter with its own time constant on each menu, about what VRChat's own feel like (Enzo, in the headset): `vr_smoothing_quick_menu_ms` 35, `vr_smoothing_wheel_ms` 20. The tiles around the menus are on springs of their own whose character keeps changing, or rigid, as Settings chooses.

## Knowing a menu is open

VRChat says nothing: OSC reports no menu state, its log nothing when a menu opens, and SteamVR no other application's action sets.

| | Desktop mode | VR |
| --- | --- | --- |
| What starts it | Esc or R, read with `GetAsyncKeyState` while VRChat is in front | VRChat's menu button of either hand, through the overlay's own actions, as VRChat's bindings have it |
| What decides | **VRChat's window's picture** (Windows Graphics Capture, while looking or open only): the wheel's disc at its exact place, the Quick Menu's row of icons by shape | Nothing further: short press the Quick Menu, held the wheel |
| It closes when | The picture no longer shows it, two frames in a row just after Esc or R, else three looks in a row (about 180 ms, a sub-page's slide lasts that long), or no new frame for 600 ms | The button, a world change, the dashboard, VRChat losing the scene, 90 s with no laser on a panel |
| How soon | On a key the parts go at once (3 to 10 ms) and looks run at each frame. VRChat shows nothing for about 200 ms after R, the wheel's rim is in place at about 235 ms and the rest at about 300: right after R the rim alone starts the ring, and the whole wheel must follow within 400 ms. From the key to the ring up about 0.25 s, the Quick Menu's wing about 135 ms | |
| Can it be wrong? | Late, not wrong: a gamepad or OSC opening it has no key to start the looking, so nothing shows. A key that closed nothing brings the parts back after 350 ms | **Yes:** VRChat's menu closing by another way leaves the panels until they time out. After that the next press counts as closing |

An eye picture could settle VR the way the window's does (`orbiters_openvr::mirror`), once a menu has been measured in a headset.

## Where they go

`zones.rs`. Measured on the real game at 10 window sizes: the wheel scales with the window's height, and sits 0.1556 of it right of the middle and 0.1583 below; the Quick Menu scales with the height down to 16:9, less below that (`aspect^0.28`), centred at 0.55 of the height. Zones are in the menu's own units, so one set of numbers serves every window. A zone the window cuts by more than 28% does not show. The ring's cells are concentric with the wheel, and what lies outside a cell's shape is no part of its window. Each part stays whole or does not show.

In VR the same units are laid on a plane: the wheel's rides the hand facing the eyes, the Quick Menu's stays where it opened. **Sizes and offsets in VR are estimates** (`vr.rs`), adjustable by Settings' size until measured.

## Pointing, and who gets the press

- **Quick Menu, desktop:** the mouse is free. A click on a panel's window stays there, measured: the button under it does not act and VRChat keeps the foreground. VRChat's hover does follow the pointer under a panel, which does nothing.
- **Wheel, desktop:** VRChat holds the mouse (clipped to one pixel; its movement picks a slice), so nothing can hover. **Holding Left Alt** points at the panels: a low-level mouse hook, set only while Alt is held with the wheel open, keeps the mouse from the game and moves a pointer of ours, which goes back on top of whatever comes into view. Let go, the mouse is VRChat's. A press that opens a sheet keeps pointing with no key held. The number keys 1 to 6 pick a cell with no pointing at all (read with `GetAsyncKeyState`, only while the wheel is open; the game sees them too). The mouse's movement still reaches VRChat's slice highlight, which cannot be stopped from outside: a veil dims the wheel while Alt is held, to show who has the mouse.
- **VR:** the controllers' own lasers. While a laser is on a panel, that hand's trigger is taken from VRChat: an action set binding the trigger, active for that hand at `k_nActionSetOverlayGlobalPriorityMin + 1`. It needs SteamVR's developer setting "Experimental overlay input overrides" (`steamvr.globalActionSetPriority`), which the overlay turns on once and Settings reports.

The block's rules (`block.rs`): blocked on hover, before any press. A press begun in the game stays the game's until let go; a press begun on a panel stays ours until let go. Off the panel, free at once; panels gone, free at once whatever is held. It is said anew every frame, so it ends by not being said, and SteamVR forgets it with the process. If the overlay's loop hangs with a block on, the process ends itself after 1.5 s.

## The gallery

The Pictures sheet lists what the app's own Photos & captures lists on this computer: `worlds::load_gallery`, VRChat's pictures and the captures together, newest first, with **All** and **Only 3D** at its top right, in the layout chosen in the app (`pages::gallery_layout::arrange`). A list can hold tens of thousands: `panels::Gallery` holds the tiles and their places once, compares by being the same list, and each frame's facts only carry the words and pictures of the tiles on the page. Pictures are decoded by two threads, what was asked last first, and let go far from the page.

A capture with a 3D view shows in 3D from its tile (`Relayed::ShowLocal`, `catalog::read_local`); on the desktop its press opens the app's window. A picture shows large on the sheet (`Look::BigPicture`), with Back and steps from picture to picture over the captures.

## Lists

The gallery, previous instances, YouTube's results and the conversation scroll by dragging on them, by a bar to grab, and thrown (`Drag`). Past an end they give less and less under the hand (`rubber_band`), run past it when thrown and come back to rest exactly there, without going beyond it the other way (`scroll_step`). A notch of the mouse's wheel at an end gives a little. With Reduce motion, no give at all.

## A capture standing small (VR)

`worlds::stereo::Small`: a capture stands small before the person. What it shows is chosen from its content (`Capture::small_frame`): around what stands up in it, not at a fixed radius, stopped under the first layer covering much of its round (`slab_over`), with soft round splats and lone floaters left out, as a round slice across the ground in which what sticks past the rim counts as past it. It stands tipped towards who looks on a plate (`small_hint::Plate`): a soft shadow, a dark disc and a rim where the capture ends. A capture at a size needs nothing of the renderer but the camera's translation divided by that size (`Placement::size`). It is sorted anew around its subject (`Capture::around_subject`) so the first splats are those near it, and drawn over nothing: `Gpu::present_alpha` writes what the splats leave of the background as transparency, premultiplied.

On a world's side panel it lies in a hole (`panel::Panel::hole`, the same hole in the sheet's back, `panels::Facts::sheet_hole`), behind which a dark pane (`small_hint::Backdrop`) hides VRChat's menu. It follows the panel until a grip closes near it (`Small::step`); then it follows that hand, grows, and that hand's stick zooms it. Let go near the hole, or asked with Back here, it returns. Taken, it stays when the menu closes, until Hide 3D.

`small_hint`: above it, what says how to take it and then how to zoom it, with a little thumbstick pushed up and pulled down. A stick zooming it ends the hint, until a new capture opens from a new opening of the menu (`Relayed::MenuOpened`).

Once taken, it has a Hide 3D of its own under it (`small_hint::HideChip`), found by a laser with a margin or by a hand close by, as it may stay after the menu has closed.

The stick also reaches VRChat, as nothing takes it from the game while the capture is held: held in the left hand, its zoom also walks the avatar.

## Worlds in 3D

A press on a world opens its details on a side panel beside the list, which narrows to two columns of the same six results, so that another can be pressed at once (`Page::Detail`, `SIDE`). In a headset a press on a world with a capture shows it, around the person or small, whichever was chosen last.

## VRChat's tooltips under a sheet

On the desktop VRChat lights what lies under the pointer even with a sheet over it, as nothing outside the game can stop that, and shows its tooltip under its tab row. While the pointer is on these parts, `Zone::SheetHint` lies over that place and says what the pointer is on. What is pointed at is said only there while it is up (`Facts::hint_band`), else in the sheet's head.

## A 3D picture from these menus, on the desktop

VRChat's camera must be open, and these menus cannot be used with it open. Measured in the game: opening either menu closes the camera, R does nothing over it, and OSC cannot open it (`/usercamera/Mode` only stores what VRChat's tree then says: no camera shows, and a recording gets no frame).

So a press **asks for** the picture (`Command::Photo3DWhenCamera`). The ring, the Quick Menu's caption, the sheet's foot and, with no menu up, a small pill over the game say what to do: open the camera, then move it if VRChat has not said where it is yet. The countdown then starts by itself, shown by the pill along with the flight, which is why the pill is there: with the camera open nothing else can say it. It then says "3D picture saved" or what stopped it. It lets clicks through and the camera records its own stream, not the screen. Pressed again while it waits, it is called off; after 90 s it says it was. In VR a press still starts at once, and with the camera closed says so: whether a menu closes the camera in VR too could not be checked here, and only a headset can say.

## Cost

Closed: no window or overlay shows, nothing is painted, no capture runs; desktop mode asks for two keys' state every 33 ms. See `reports/orbit-overlays.md` for what was measured.

## Testing

- `cargo test -p orbiters-desktop overlay::orbit`: placement, hover, the block's state machine, the detector on real frames (`orbit/testdata`, made by `scripts/orbit-fixtures.py`).
- `orbiters-desktop --screenshot <folder> --only orbit`: every panel's state.
- The real game: `docs/vrchat-bridge.md`, "Running it live". The bridge's screenshot shows VRChat's window only, so what the panels draw is checked by capturing that screen region.
