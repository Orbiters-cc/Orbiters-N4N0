# The Orbiters menu around VRChat's own menus

Code: `crates/orbiters-desktop/src/overlay/orbit/`. Settings, Performance, "Orbiters menu in VRChat".

**What it does.** When VRChat's Action Menu wheel or Quick Menu opens, Orbiters' parts appear around it, and go when it goes. It is the default; floating and the avatar's screen remain selectable, and the wheel and Quick Menu turn on and off separately.

- **Around the wheel:** a ring of six equal cells on VRChat's own 60 degree slices, 1.72 times the wheel's radius, their names bent along the outer edge letter by letter (`panels::curved`): Worlds in 3D, YouTube, Now playing, All tools, Instances, 3D pictures. Pointing at 3D pictures blooms a second ring (of people, of this world, Record); Now playing blooms previous and next.
- **Around the Quick Menu:** one wing docked to My Profile's outer edge, with the same tiles, Ask N4N0 and what plays. With no room beside it, the same tiles lie in a strip under VRChat's tab row, as no part covers VRChat's panels at rest.
- **In front of the Quick Menu:** one sheet with its name in its head, ending at the main panel's own bottom edge so that VRChat's tab row stays: All tools (sized by what matters, what plays in full on it), the gallery, Previous instances (a row for each world), Worlds in 3D (which opens on its 3D captures) and N4N0's pages.

**The look.** Orbiters' own design system, a bit more modern than VRChat's, and sitting well beside it: filled surfaces with no outlines, line icons, mint for what is pointed at or chosen, and the Orbiters mark on each surface. It takes scale, placement and density from VRChat and never its materials: nobody should take it for VRChat's own (`reports/ui-loop/round-0/synthesis.md`).

**No mod.** Nothing is injected, the game's files and memory are not touched. In desktop mode each panel is an ordinary topmost window; in VR an ordinary SteamVR overlay.

## On the desktop

A capture's press shows it in live 3D on the sheet (`sheet_view.rs`), with the Orbiters window as the step for walking around in it. While pointing, and while a 3D picture waits for VRChat's camera, a smoked disc lies over VRChat's wheel, whose lit slice has nothing to do with it, and says what is pointed at or what waits (`draw_hub`).

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

## Real pictures on All tools

All tools' large cards hold real content: the newest three captures, the last three places, the newest picture, what the world's player has. `Recents::newest` picks them and `keep_the_newest` holds only those. Each is asked for once: asked for twice, a new list would lose what had been read of it. The wheel's cells and the wing's tiles hold none: tried in round 6, dark captures on three of six parts read as dead cells. They have one treatment, a solid base and the tool's colour on the icon (`part_ink`).

## A capture live on a desktop sheet

`SheetView` runs on its own thread: it reads the capture, frames it as the small view does (`Capture::around_subject`, `Small`), and draws it with `FaceRenderer::render_view` into a picture the sheet paints, with its plate as a line under it. By itself it sways around the view the capture was taken from (a capture is mostly taken from one side), a drag takes it anywhere, the mouse's wheel zooms. It holds the 3D view's budget (`pacing::DUTY`) and at most 20 frames a second, draws nothing while nothing moves, and sizes its picture by what the last ones cost (960, 720 or 540 wide). The view, its GPU device and its memory go with its thread when it is closed. Reduce motion: it does not sway. If it cannot show, the capture's own picture stays, and says so. Only on the desktop: in a headset a capture stands around the person. The same view turns on a world's side panel (`Worlds::start_live`, `panel::Live`): See it in 3D starts it there, at its smallest size, the next world pressed follows, and it goes with the panel. Its button is then Walk around it, in Orbiters, as on the gallery's sheet, which also steps from capture to capture.

## The gallery

The Pictures sheet lists what the app's own Photos & captures lists on this computer: `worlds::load_gallery`, VRChat's pictures and the captures together, newest first, with **All** and **Only 3D** at its top right, in the layout chosen in the app (`pages::gallery_layout::arrange`). A list can hold tens of thousands: `panels::Gallery` holds the tiles and their places once, compares by being the same list, and each frame's facts only carry the words and pictures of the tiles on the page. Pictures are decoded by two threads, what was asked last first, and let go far from the page.

A capture with a 3D view shows in 3D from its tile (`Relayed::ShowLocal`, `catalog::read_local`); on the desktop its press opens the app's window. A picture shows large on the sheet (`Look::BigPicture`), with Back and steps from picture to picture over the captures.

## Lists

The gallery, previous instances, YouTube's results and the conversation scroll by dragging on them, by a bar to grab, and thrown (`Drag`). Past an end they give less and less under the hand (`rubber_band`), run past it when thrown and come back to rest exactly there, without going beyond it the other way (`scroll_step`). A notch of the mouse's wheel at an end gives a little. With Reduce motion, no give at all.

## The wheel's keys, on the desktop

Each cell shows its key as a quiet mark at its inner edge, always in the same place (`digit`, `cell_icon`). Right over the ring, in the first uses, the caption says to hold Left Alt and click, or press a number; with Alt held, Click a tool. The cell pointed at lights up whole, with a line round it. While the second ring is out, 7, 8 and 9 pick its cells. While pointing, and while a picture waits, the veil is solid: VRChat goes on lighting a slice under it, and that must not read as what is pointed at. While a picture waits, the Pictures cell is what calls it off, by its key or a click: `items` makes it the press of the picture asked for, which is what toggles it. The caption says Press 6 again.

## A capture standing small (VR)

`worlds::stereo::Small`: a capture stands small before the person. What it shows is chosen from its content (`Capture::small_frame`): around what stands up in it, not at a fixed radius, stopped under the first layer covering much of its round (`slab_over`), with soft round splats and lone floaters left out, as a round slice across the ground in which what sticks past the rim counts as past it. It stands tipped towards who looks on a plate (`small_hint::Plate`): a soft shadow, a dark disc and a rim where the capture ends. A capture at a size needs nothing of the renderer but the camera's translation divided by that size (`Placement::size`). It is sorted anew around its subject (`Capture::around_subject`) so the first splats are those near it, and drawn over nothing: `Gpu::present_alpha` writes what the splats leave of the background as transparency, premultiplied.

On a world's side panel it lies in a hole (`panel::Panel::hole`, the same hole in the sheet's back, `panels::Facts::sheet_hole`), behind which a dark pane (`small_hint::Backdrop`) hides VRChat's menu. It follows the panel until a grip closes near it (`Small::step`); then it follows that hand, grows, and that hand's stick zooms it. Let go near the hole, or asked with Back here, it returns. Taken, it stays when the menu closes, until Hide 3D.

`small_hint`: above it, what says how to take it and then how to zoom it, with a little thumbstick pushed up and pulled down. A stick zooming it ends the hint, until a new capture opens from a new opening of the menu (`Relayed::MenuOpened`).

Once taken, it has a Hide 3D of its own under it (`small_hint::HideChip`), found by a laser with a margin or by a hand close by, as it may stay after the menu has closed.

The stick also reaches VRChat, as nothing takes it from the game while the capture is held: held in the left hand, its zoom also walks the avatar.

## Worlds in 3D

On a screen, beside the side panel every result of a short list keeps its place (`few_rects`, shorter cards there, See them all stays), and See it in 3D says it opens in Orbiters. A headset's page is its own (`VR_*`), not a smaller screen's: six worlds as pictures with their names, one head on the list that goes back to search and the filters, and a wide panel with the hole (`VR_HERO`), the name, one row of what to do and one of small tiles. Grip to take it lies under the hole, not on it. Around me and Small here both stay, the current one lit. With a sheet up, a wing stands off it by `WING_OFF_A_SHEET`: the sheet is nearer the eyes and would seem to run into it.

A press on a world opens its details on a side panel beside the list, which narrows to two columns of the same six results, so that another can be pressed at once (`Page::Detail`, `SIDE`). In a headset a press on a world with a capture shows it, around the person or small, whichever was chosen last.

## A sheet's head

Where no wing lies beside it (from the wheel, in a narrow window, with its tiles hidden), its tools are tabs (`head_tabs`): from any sheet to any other. Beside the wing they would only repeat it, and the head says the sheet's name. On a screen they have their names, and while the head has something to say, icons but for the open one. In a headset they are named or are not there.

## VRChat's tooltips under a sheet

On the desktop VRChat lights what lies under the pointer even with a sheet over it, as nothing outside the game can stop that, and shows its tooltip under its tab row. While the pointer is on these parts, `Zone::SheetHint` lies over that place and says what the pointer is on. What is pointed at is said only there while it is up (`Facts::hint_band`), else in the sheet's head.

## A 3D picture from these menus, on the desktop

VRChat's camera must be open, and these menus cannot be used with it open. Measured in the game: opening either menu closes the camera, R does nothing over it, and OSC cannot open it (`/usercamera/Mode` only stores what VRChat's tree then says: no camera shows, and a recording gets no frame).

So a press **asks for** the picture (`Command::Photo3DWhenCamera`). The ring, the Quick Menu's caption, the sheet's foot and, with no menu up, a small pill over the game say what to do: open the camera, then move it if VRChat has not said where it is yet. The countdown then starts by itself, shown by the pill along with the flight, which is why the pill is there: with the camera open nothing else can say it. It then says "3D picture saved" or what stopped it. It lets clicks through and the camera records its own stream, not the screen. Pressed again while it waits, it is called off; after 90 s it says it was. In VR a press still starts at once, and with the camera closed says so: whether a menu closes the camera in VR too could not be checked here, and only a headset can say.

## Cost

Closed: no window or overlay shows, nothing is painted, no capture runs; desktop mode asks for two keys' state every 33 ms. See `reports/orbit-overlays.md` for what was measured.

## Testing

A script cannot type in the search: the keyboard's hook (`keys.rs`) leaves out injected keys, and they go on to VRChat. So YouTube's results in the pack are painted, from a real search over a real frame of the game (`results_over_the_game`, `ORBIT_QM_FRAME`). `ORBIT_PANEL_WORLDS` gives the panel's scene real names and pictures.

- `cargo test -p orbiters-desktop overlay::orbit`: placement, hover, the block's state machine, the detector on real frames (`orbit/testdata`, made by `scripts/orbit-fixtures.py`).
- `orbiters-desktop --screenshot <folder> --only orbit`: every panel's state.
- The real game: `docs/vrchat-bridge.md`, "Running it live". The bridge's screenshot shows VRChat's window only, so what the panels draw is checked by capturing that screen region.
