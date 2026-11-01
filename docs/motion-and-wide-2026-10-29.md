# Closing morphs, wide windows and tooltip shadows (29 October 2026)

Enzo's three asks after the U1–U5 passes: closing a card's or a button's window often did not look as continuous as
its opening and should end with a bounce; Home, N4N0, Videos and Settings stopped adapting past a certain width; the
shadow under some tooltips had notches ("Open the folder" in the Unity projects panel).

## Closing morphs

What was wrong (frames of `gallery-close-mid`): the picture window flew back in about 170 ms on a spring that ends
fast, covered its card to the last frame and then disappeared at full speed, and the card's shade, badges and title
appeared at once under it. Dialogs grown out of buttons did the same over their button (the label popped back);
the storage, raw import and video bookmark popovers had no closing motion at all; Visited worlds' large picture closed
instantly.

Now (`orbiters_ui::morph`, used by all of them):

- **One progress spring both ways.** Closing is the opening run backwards; a press during either reverses it from
  where it is (`travel`). Pressing a window on its way back opens it again.
- **Hand-over.** Near its source the flying shape fades out over it (in over it while it leaves), and the source shows
  under it: `shape_alpha` / `source_alpha` (below 0.2 of the way), `HANDOVER_LATE` (0.1) for dialogs much larger than
  their button.
- **The card rides.** Where a page can draw its card, the window draws it under itself mapped onto its picture's place
  in the flight (`morph::ride`, `sharing::Source`): the gallery card grows out with the window and comes back in it,
  then is the real card. Visited worlds' world page does the same with its card; its large picture lands exactly on the
  tile's crop (the uv opens from the tile's cover crop to the whole picture).
- **Landing bounce.** Back on its source, the source dips about 4 %, springs 1.5 % past its size and settles in 0.38 s
  (`bounce_at`, measured on `gallery-close-mid`: the card's edge moves in 5 points, out 2, rests 0.3 s after landing).
  Morphs say where they land (`land`); `widgets::Button`, `IconButton`, `popup::dropdown`/`menu_button`, gallery,
  world and session cards bounce on their own (`bounce`).
- **Lost source.** A gallery card scrolled off the page, or a world tile not drawn: the window closes into the middle of
  the page, fading, instead of flying to a stale place.
- **Popovers** (`popup::popover`): grow out of the corner nearest their button on a bouncy spring and shrink back into
  it; the instance (Photos & captures), storage and raw import (Capture files) and bookmark (Videos) popovers keep
  their state while they close.

## Wide windows

`layout::section_columns` and `pages::Masonry`: from two columns of 600 points (pages less their padding: 1600 and
1920-point windows get two, 2560 and 3440 three), Settings', N4N0's, Videos' days, Home's Music and QR cards sit side
by side, each card in the shortest column and keeping it while cards grow; one card alone is at most 1120 wide.
`pages::row_columns` puts a full-width card's rows side by side (a Videos bookmark list, YouTube results, previous
instances). Home's tiles: as many of 210 points as fit, two to six a row, seven past 440-point tiles, a little taller
when wide. 1100 and 1280-point windows keep their single column.

## Tooltip shadows

egui tessellates a blurred rectangle by feathering its outline by the blur; once the blur passes half the shape's
height (a 28-point tooltip under a 40-point blur) the inner edge collapses to a line: a hard crease along the middle
and streaks from the corners. Popups now keep a short shadow (`popup_shadow` offset 8, blur 16), and
`paint::shadow` (panels, popovers, dialogs, the activity capsule) is a gaussian fall-off made of rings of the shape,
a thin shape's shadow being its blurred box across the narrower side.

## Screenshots

`orbiters-desktop --screenshot <dir> --only <name> --extend <frames> --sequence <frames>`: runs the scenario that many
frames past its shot and writes the last frames (`<name>-<frame>.png`). Closings: `gallery-close-mid` (closed 6 frames
before the shot), `gallery-requests-close` (frame 120), `gallery-instance-close` (frame 120), `world-close` (frame 120,
the first card), `world-viewer-close` (frame 170); e.g. `--only gallery-close-mid --extend 40 --sequence 47`. Wide
pages: `--only home|n4n0|videos|videos-list|settings|home-results|home-music|home-instances|home-qr --width 1920
--height 1080`.

## Not verified here

Visited worlds' large picture: this computer has no VRChat pictures, so its tile-to-picture morph was not seen. The
Unity projects dialogs' landing was not rendered (the requests dialog, on the same code, was). Nothing was tried by
hand in the window; the motion was judged on 60-fps frame sequences.
