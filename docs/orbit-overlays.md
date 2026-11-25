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

`worlds::stereo::Small`: a capture stands small before the person. Where it looks is chosen from its content (`Capture::small_frame`): around what stands up in it, not at a fixed radius. It shows a round of the world across the ground, in which what sticks past the rim counts as past it, and all of what is in that round from its floor to its top: nothing goes for its height, its size or for lying apart (Enzo: "show everything"). That round is the only limit left, and zoom and the move change it. It stands tipped towards who looks, on nothing: a floor of its own only hid what it held (Enzo). A capture at a size needs nothing of the renderer but the camera's translation divided by that size (`Placement::size`). It is sorted anew around its subject (`Capture::around_subject`) so the first splats are those near it, and drawn over nothing: `Gpu::present_alpha` writes what the splats leave of the background as transparency, premultiplied.

On a world's side panel it lies in a hole (`panel::Panel::hole`, the same hole in the sheet's back, `panels::Facts::sheet_hole`), behind which a dark pane (`small_hint::Backdrop`) hides VRChat's menu. In sort order: the panels (15 and up), the 3D view (2), the pane (0), the game. SteamVR composites overlays by that order and not by depth. An overlay can neither read nor blur what lies under it, so the pane is dark and translucent where Enzo drew a backdrop blur; this was not seen in a headset. It follows the panel until a grip closes near it (`Small::step`); then it follows that hand, grows, and that hand's stick zooms it. Let go near the hole, or asked with Back here, it returns. Taken, it stays when the menu closes, until Hide 3D.

`small_hint`: above it, what says how to take it and then how to zoom it, beside a real thumbstick (`stick3d`), translucent glass traced for each eye from where that eye is, pushed up and pulled down on a spring. A stick zooming it ends the hint, until a new capture opens from a new opening of the menu (`Relayed::MenuOpened`).

Once taken, it has a Hide 3D of its own under it (`small_hint::HideChip`), found by a laser with a margin or by a hand close by, as it may stay after the menu has closed.

The stick also reaches VRChat, as nothing takes it from the game while the capture is held: held in the left hand, its zoom also walks the avatar.

## Worlds in 3D

On a screen, beside the side panel every result of a short list keeps its place (`few_rects`, shorter cards there, See them all stays), and See it in 3D says it opens in Orbiters. A headset's page is its own (`VR_*`), not a smaller screen's: six worlds as pictures with their names, one head on the list that goes back to search and the filters, and a wide panel with the hole (`VR_HERO`), the name, one row of what to do and one of small tiles. Grip to take it lies under the hole, not on it. Around me and Small here both stay, the current one lit. With a sheet up, a wing stands off it by `WING_OFF_A_SHEET`: the sheet is nearer the eyes and would seem to run into it.

A press on a world opens its details on a side panel beside the list, which narrows to two columns of the same six results, so that another can be pressed at once (`Page::Detail`, `SIDE`). In a headset a press on a world with a capture shows it, around the person or small, whichever was chosen last.

## A sheet's head

Where no wing lies beside it (from the wheel, in a narrow window, with its tiles hidden), its tools are tabs (`head_tabs`): from any sheet to any other. Beside the wing they would only repeat it, and the head says the sheet's name. All tools has no tabs anywhere, as its large tiles are the same things (Enzo). Every other sheet has them, beside the wing as well, named; a page with no tab of its own has them as small icons on a screen, to leave its name room. In a headset they are named or not there: when the head has something else to say, they go. Who comes to All tools has the main features on the wing already and came for the rest: the accessories have two full rows of tiles that say what they do, and the main features stay the largest.

## The sheet of settings

Opened from All tools: where the tiles lie (`OrbitLayout`: the wing, or the strip under the tabs, which the narrow window has anyway), whether they spring, the output Windows uses when SteamVR starts (`vr_audio.output`, the same setting as the app's page), and the look (`OrbitSettings::theme`). Modern takes an accent (`look::accent()`, for everything of these parts and the world's panel, and through `page_skin` for N4N0's pages inside a sheet, which are drawn here: nothing to update on the avatar, whose own screen is another drawing and stays as it is). The colour bar is a drag, seen as it moves and saved when let go.

Retro is N4N0's retro menu, in one colour. `orbit/retro.rs` is one pass over everything a part drew, at the end of its frame (`Canvas::in_orbit_look`), so no part has a second drawing: capitals, cut corners, a line round what can be pressed (dim for long rows), the dot grid and double frame on what covers a page, icons between brackets. It draws in greys and `finish_picture` puts them through a ramp of the colour; the colours to choose from are kept. Pictures: Fully monochrome draws them green at all the resolution they have, levels opened up (`retro::phosphor`, at the texture: `Software::set_picture_look`, so text over them stays sharp and it can be undone without loading them again). N4N0's 96 x 54 dots stay on the avatar's screen, which has no more: on a page they could not be read; Pictures in colour leaves them and `finish_picture` restores what `keep_colour` marked. N4N0's pages in a sheet draw `menu_retro` itself, with the pass off. Not the same as N4N0's: there is no top plate with its version and hatched tapes, memory plate, level bar, corner crosshairs or scanlines on these parts; gradients (the wheel's cells, a scrim over a picture) stay gradients, in the one colour; the world's chip and bar in a headset keep the modern look. All saved as any setting is, by a patch.

## Languages

What these menus say goes through `orbiters_i18n` (`tr`, English as the key), in the website's languages. The sheet of settings is the worked example (`orbit/panels/settings_sheet.rs`, `orbiters-i18n/catalog/overlay-settings`); the rest of the tool is being wrapped by the batches in `reports/i18n`. The language is the account's (`GET/PUT /n4n0/language`, kept by `controller/language.rs` on its own thread), or this computer's when there is none or nobody is signed in; a language a server cannot be asked for stays as it last was. `--screenshot --language <tag> --text-report <file>` renders any language and lists text past its box.

## A view held in a hand

Taken in a hand, a small 3D view is out of the menu's: it stays when the menu closes and when the world changes, until its X or Hide. With the menu off an X floats above it. The Quick Menu opened again goes straight to the sheet it was on (Worlds or Pictures), to go on through them. Lying across a menu it stays, behind it by sort order. It never fades for that (it did, and vanished). The hand holding it zooms with its stick; its other stick moves its middle along the ground (Touch controllers report no thumb position: their `thumbrest` is a plain button, and `trackpad` is invalid on them; a real touchpad still works). The splats are sorted from the start eye, so once moved `Gpu::set_marks` leaves out what is past the round, across the ground only. Its hint comes back whenever one is opened with none showing.

A capture around the person turns with VRChat's turning stick, the hand that does not walk (`immersive_turn`, on by default; `stereo::TurnStick`): smoothly or in 30° steps as VRChat's own Comfort Turning says, read from its registry when a capture opens. The smooth rate and the step are this code's own, as VRChat's are not readable. The stick is left alone with a menu up.

## On a Mac

Code: `overlay/orbit/desktop_mac/` beside `desktop.rs`; what both decide and draw the same is `desk_shared.rs`. The parts, places, detectors and settings are Windows'. It works with any engine Home starts; one from 0.1.15 on needs no key. The overlay is ordinary windows of this app, and nothing is injected, drawn or sent into the game, its window or its window server (`wsd`).

| | What a Mac does |
| --- | --- |
| Started | By the tray service while Home's VRChat runs (`LauncherState::desktop_game`, the engine's launcher; never one the bridge started), at most once every 10 s, and it ends with the game (`wanted_on_desktop`) |
| The game's window | `wsd`'s, found from that process (`desktop_mac/game.rs`): at layer 25 over the whole screen while it is in front, at 0 behind another app. Its size is the display's pixels whatever the render percent, as macOS stretches the game's picture over it |
| A menu is open | ScreenCaptureKit's picture of that window alone, at most 1080 rows high, started by Esc or R (and as the game comes in front), ended with the menu (`capture.rs`). The frames stay in shared memory, and a look reads what `detect::parts` names. At 45 % render the game draws 860 rows on a 1470 x 956 screen, within the sizes the detectors were measured at (`lab.rs`, tests) |
| The wheel | In its place as on Windows, and after R also **around** it (`wheel.rs`): up to 0.1 of its radius off and 10 % larger or smaller, where it must have more of each measure; the ring lies round it there. The place holds to about 3 % of the radius only. A look that found nothing says so in `vr-test.log`, in numbers: the frames, what the wheel's place had against what is asked, the best around it, the disc most like a wheel anywhere, and the Quick Menu's icons. Settings, "Keep a picture when a menu is not found" (off by default), also leaves that frame in the logs' folder (`overlay-looks`, the newest 12) |
| The keys | `CGEventSourceKeyState` each turn, only with the game in front. No event tap, no hot key, no Accessibility (a test reads the sources for them). R is the key that types "r" in the Mac's layout; the numbers are the top row's places |
| The windows | One non-activating `NSPanel` a part, one level above the game's window while it is in front, else right above it and under what covers it. They join every Space and go when the game's window is not on screen. The page, its back and its hint are at levels of their own (`panel::rank`): macOS raises a window a person clicks among those of its level, and the page's back, whose head has the tabs, came over the page, plain grey (2026-11-25) |
| The mouse in the Quick Menu | **The person's, with no key**, as on Windows, from obelisk-cli 0.1.15 on: its window server (`wsd --free-unconfined-pointer`) holds the pointer only while the game confines it, and VRChat confines it to nothing in its menus (seen in the real game, 2026-11-25). The channel `<host folder>/pointer.sock` says who has it (`desktop_mac/channel.rs`: free, held, away, the game's window and where its picture lies); it is read without waiting once a turn, nothing is ever written to it, and it counts only from the window server of that very session. A part under the pointer takes the mouse, where it is painted; macOS shows its own arrow there; the game keeps its keyboard, so whoever walks, walks on. A menu that frees the mouse is also looked for, whatever opened it |
| Typing | A text field of ours (a search, a name) takes the keyboard while it is up, and only then: a panel of two points takes it without bringing this app in front (`panel::Hold`), the game hears its window lost it and gets it back the moment the field is left, the person clicks the game, another app comes in front or the menu closes; a press on ours asks again. What the layout writes, dead keys included, goes to the service (`ToService::Typed`). Esc is the page's Back there |
| The ring round the wheel | VRChat holds the mouse while its wheel is up, as on Windows. **Pushing past the wheel's rim lights the cell there** (`desktop_mac/gesture.rs`, below), one notch of the scroll wheel or a short rest on it opens it, a click never does; the numbers work as ever. A page opened from the wheel takes the keyboard, which frees the mouse |
| With an engine from before, or no channel | **Left Option**, for both menus: that engine holds the pointer for as long as its window has the keyboard. It takes the keyboard while held (`wsd` lets the pointer go and releases the keys the game held, and takes it back by itself once that panel goes: 3 to 7 ms, measured), and a page keeps it with no key. A click on the game, Esc, another app in front or the menu closing gives it back. Settings, "Right now", says which of the two it is; it goes over to the mouse by itself at VRChat's next start on an engine that has it |

**The gesture around the wheel.** The moves are those the engine says it sent the game (`WSD_POINTER_MOVE`), summed from the moment R is pressed; the game gets every one. Units are the wheel's radius (0.139 of the window's height: 133 points on a 1470 x 956 screen), VRChat's sensitivity being its own. The rim is a wall, so that ordinary use of the wheel picks none of ours, and the thresholds are in the source and tested with strokes of every length and speed:

- the stroke that brings the mouse to the rim never goes past it (a flick, however far), unless it goes on pushing one way for 0.45 s and 1.5 radii;
- the next stroke (after 0.12 s without a move) goes through once it pushed 0.6 radii outward, if it began within 35 degrees of straight out; sliding along the rim never does, wherever it ends;
- through, the mouse is on the ring (drawn by us: the arrow, the cell lit, the veil over VRChat's wheel), no further than 1.6 radii in that stroke, so that a push that goes on keeps its cell; the next stroke may go on to a second ring that is out;
- back inside 1 radius, or any button of the mouse, puts it out; nothing goes through for 0.25 s after a click or as the wheel opens;
- a rest (0.4 to 1.5 s, Settings; 0.6 by default) is any time without 4 points of movement, shown as a ring that closes round the arrow; a notch counts with the cell lit for 0.12 s, the scroll wheel still for 0.3 s before it and no button for 0.16 s after it (two fingers laid for a right click scroll by nothing first). macOS counts the scroll wheel's turns for anyone (`CGEventSourceCounterForEventType`), with no tap and no permission, and VRChat's wheel has no use for them.

`vr-test.log` says when the ring was lit and put out, with the moves so far: what the thresholds are judged by after a real session.

**Never the whole window.** A window whose pixels cover all of the game's makes macOS stop showing the game's, and the engine then presents nothing (measured with an opaque panel: at once). `panel::uncovering` leaves out what would, and a test goes through every menu and window shape.

**Permissions.** Screen Recording, without which nothing shows; Settings, "Right now", says so with the button that asks (by the service, the app macOS names; the overlay's process is its child and has what it has). Given while the app runs, a new overlay process sees it: one that was refused ends at the next Esc or R after 30 s, and the service starts another. Input Monitoring only if macOS hides which keys are down without it: the key-down counter then moves with no key to be seen (`keyboard::Keys::hidden`), Settings says so, and meanwhile a press that stands alone still starts a look.

**What differs from Windows.** The ring is picked by pushing past the wheel's rim rather than with Left Alt. While a text field of ours is up the game has no keyboard, as with Windows' hook, but for every key (its own included); a page opened from the wheel takes it too. The 3D pictures cell and its second ring, what takes a picture on the Pictures sheet, the pill of a picture under way and the camera's column are there where the engine gives VRChat's stream camera (`LauncherState::camera`, from obelisk-cli 0.1.15), and hidden with one from before; translation over the window, the corner's cards and the mismatch buttons are not there (`Here::MAC`: `window_text`, `mismatch_buttons`). The sheet of settings has no row of a headset's. Exclusive full screen does not exist: VRChat's full screen here is a window others can lie over.

**If it hangs or dies** while it holds the keyboard, its windows go with it and the game's is key again: a watchdog ends a loop that has not turned for 1.5 s, and the service starts the process again, which looks at once for a menu that is up.

**Checking it with no game:** `tools/mac-overlay/check.py` (`--render-percent 45`) lays it over the engine's real `wsd` with a window that shows real frames of the menus and confines the pointer as VRChat does, and checks all of the above, frames shown included, with real clicks and real movement of the mouse. `--wsd` takes the window server to use: with one that frees the pointer the parts are clicked with no key held and the ring is picked by movement; with one from before, or `--no-channel`, by Left Option. It covers the screen for a minute, and starts only while nobody uses the Mac.

## Around VRChat's camera

In a headset, tiles for a 3D picture of people, of the world, and the QR code lie beside VRChat's camera panel. It can be grabbed and it can fly, and `/usercamera/Pose` is the camera's place and not its panel's, so it is found by sight, as Enzo proposed: while VRChat says its camera is open (and only then) one eye is read (`camera_frames.rs`), and the panel's white X and white chevron looked for in it (`camera_track.rs`). It is a third menu for `vr.rs` (`Around::Camera`), so lasers, presses and painting are the same as elsewhere.

**What is read.** What the compositor has of the left eye (`GetMirrorTextureD3D11`, `EyeReader::read_part`): what it covers is known from SteamVR, so placing the tiles guesses nothing. Around the last place, ten looks a second, while the panel is seen; all of it, four a second, while it is not. If it cannot be read, VRChat's window, whose share of the eye is `camera_window_share` (below): it does not show the whole eye. `camera-tiles.log` in the logs' folder says which was read, and how many looks found the panel.

**Finding it.** At the size a headset shows it the X is 9 pixels and soft, hardly reaching 200 of 255. So blobs are taken at two levels of a pixel's darkest channel, an X is told by correlation with an ideal one on a grid, and a pair counts only if what else is always there lies where the pair says: the icon beside the X, the sliders, the column on the left, the row of buttons. Below a panel 200 pixels wide, a picture up to 1400 wide is read at twice its size too. `how_often_it_finds_the_panel_at_the_sizes_a_person_sees` prints the rates on Enzo's two frames.

**Placing the tiles.** The panel's place on the picture becomes tangents, and those a place in the room, through the left eye's pose when the picture was read, in a plane facing the eye and turned as the panel is seen. SteamVR's `bottom` is the side *up* (`camera_track::eye_sides`): taking it the other way laid them 0.476 tangents too high, well above the panel in Enzo's pictures. They lie on its left, or its right where the left would leave the eye's view (`side_with_room`), and next to its edge when a turn makes it narrower. Moving them is tracking's work, as Enzo proposed (`Follow`). A grip closing at the panel, or pointing at it, ties it to that controller until it is let go; let go, it stays still in the room. What is seen is a regular recalibration, so the tiles and VRChat's panel do not drift apart: within 6 mm a look says nothing, within 4 cm it nudges by 12%, beyond that by half, and beyond 15 cm it takes two looks agreeing and is then believed at once (it flew, or it follows the head). A moving hand is not compared with a picture, which is older than its pose. They stay 20 s with nothing seen, and go at once when VRChat says its camera closed.

**Correcting it in a headset, without a build.** `orbit-vr-tune.json` in the app's data folder, read while it runs: `camera_panel_m` (how wide the panel is: only how far away the tiles seem), `camera_eye_age_ms` and `camera_window_age_ms` (if they swim as the head turns and settle when it stops), `camera_use_window`, `camera_window_share` and `camera_window_moved` (only when reading the window), `camera_keep_frames` (a frame a second in `camera-frames` in the logs' folder, for measuring). What keeps the tiles away is said under their switch in Settings (`OrbitStatus::camera_note`).

## Hide Orbiters

Hide tiles and Hide ring mean hidden until shown again, whatever VRChat's menu does meanwhile: only the button that brings them back shows (`Panels::way_back`). It says nothing to what follows the menu's button, which stays true to VRChat's own menu.

While a sheet is up, a chip over a corner takes all of it away for a moment to reach VRChat's own menus, and brings it back as it was.

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

## Reading a headset test

`vr-test.log` in the logs' folder has what no test here can see, a line a second at most for each thing while it goes on: `view:` what the 3D view submits (phase, alpha, how many faces are visible, how many were handed over), `panel:` the sheet's page and back read back at the hole (both alphas must be 0 at its middle), `small input:` and `turn:` what the sticks say and what was done with them.

## Screens of the sheet

`Panels::keep_screen` keeps each screen as it was left, by its sheet and name: its scroll, the picture it had open, and for N4N0's pages the page they were on (the menu itself keeps its results). The Worlds panel keeps its own search, list and page. Every sheet has its Back in one place on its head (`Action::Back`, `go_back`): within what shows first, then through `trail`, which records what opened each screen, then off the sheet. N4N0's own Back does not show on a sheet (`Menu::back_on_the_sheet`), nor does the Worlds panel's (`Panel::on_sheet`). The stick that does not walk scrolls what is on the sheet with its up and down (`Panels::stick`), unless a hand holds what shows small; its side to side turns a capture around the person.

The Worlds list goes on as far as there are worlds wherever it shows, and asks for more as it nears its end. The side panel comes out on `Spring::EXPAND`, by `Panel::side_out`, while the cards go to their two columns. A long press with the Quick Menu open closes it over OSC at `CLOSE_AHEAD`, just before it is long, as VRChat opens its wheel only for a press that finds its Quick Menu closed and nothing here can open that wheel. Not yet seen working in a headset.
