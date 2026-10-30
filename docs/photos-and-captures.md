# Photos & captures

Photos & captures combines this computer's VRChat pictures and N4N0 recordings/3D captures with the signed-in
member's `/captures` feed. Mine, All and Shared with me select ownership; Search worlds filters local names and
sends `q` to Orbiters. More items use the server's opaque cursor. Overlapping pages are deduplicated; a repeated
cursor stops pagination. Results and filtered indices are retained between frames.

Items are ordered newest first and grouped by world and recorded instance. A session's manifest supplies its
instance; pictures use the most recent retained VRChat join before their timestamp, only when it names the same
world. Items without an instance are grouped by world/day and say that the instance was not recorded. Shared items use
the server's `instanceId` when it is available. Instance info reads VRCX's recorded joins/leaves on a background
thread for a historical roster when available. Opening Instance info also checks liveness through the member's
eligible VRChat session on a worker. Explicitly active instances offer Open in VRChat; inactive ones do not.
Unknown/inaccessible instances retain the current-instance-only behavior. Checks are cached for 60 seconds;
errors stay in the dialog with Retry. Historical joins alone never establish liveness.

Manage captures retains reconstruction, import, storage cleanup and the original detailed capture inspector.
The main gallery opens an item dialog. Local reconstructed captures use the existing 3D viewer; downloaded `.splat`
captures use the same renderer, with the contract's starting view and a validated `.splat` reader. Pictures open a
larger view. Downloaded files are saved under `Documents/Orbiters/Shared captures` with unique names. Protected
picture/file requests carry the Orbiters credential to the capture file endpoint; thumbnails are decoded off the UI
thread. Existing picture loaders also accept PNG, JPEG and WebP remote profile pictures.

Session pictures exclude the capture strip and its reference patches at the top and the skeleton band's reserved
rows at the bottom. Cropping uses the contracts' original-frame layouts before downscaling; cards and the modal keep
their aspect-correct cover crop of the remaining scene. New captures write cropped thumbnails. Existing thumbnails
are cropped lazily into `thumbnail-picture-v2.qoi`, using the manifest's original dimensions and recognizing the
encoder's already-removed top strip. This also works after keeping only the 3D view, without original keyframes.
The display cache is kept and counted with the 3D view. Original captured frame pixels are preserved for decoding
and reconstruction. A missing/unreadable original frame size leaves an older thumbnail unchanged because its data
regions cannot be positioned reliably.

## Layouts

The three icons left of Requests (under the filters on a narrow window) choose how each group is laid out
(`pages/gallery_layout.rs`); the choice is saved in the settings (`gallery_layout`: `masonry`, `rows`, `grid`).

- **Masonry** (the default) is the website's gallery (`server/frontend/src/components/elements/GalleryMasonry.jsx`):
  `ceil((width + 16) / (240 + 16))` columns 16 points apart (so at most 240 wide), each picture in the shortest column
  (the leftmost of equals) in the gallery's order, as tall as its shape. A next page appends: nothing placed moves.
- **Rows** are the website's Even rows: pictures side by side at 210 points, each full row scaled to the width (ended
  before the picture that overfills it when that keeps it nearer 210), the last row left at 210 from the left.
- **Grid** is the card it was: every item the same card, its picture cropped to 16:9, its kind, time and who sees it
  under it.

In Masonry and Rows a card is its picture alone, never cropped: the check and the badges on its top (3D for a capture,
not Picture for every picture, the layers, Reconstructing, Sharing…, Public / Shared · N), who shared it with you over
its bottom, and its kind and time there on hover; corners of 16. A picture's shape comes from the listing before its
thumbnail loads: a screenshot's PNG header (the index already reads it), a capture's frame size without the strip and
the skeleton band (as its thumbnail is cropped), what Orbiters says of a shared picture or a community's. Without one
it stands in as 4:3 (the website's 800 × 600) and springs to the thumbnail's shape once decoded; what is under it in
its column moves with it. Groups, selection (Ctrl/Shift-click, checks, rubber band), dragging out, the window growing
out of its card, lazy pictures, the endless list (its shimmering places in the layout's shapes) and the communities'
pictures work the same in every layout.

## VRChat multi-layer pictures

VRChat's camera in its Multi Layer mode saves, beside the picture, one PNG per mask turned on, named as the picture
with the mask after it: `VRChat_2026-10-29_21-04-11.532_3840x2160.png` with `…_Environment.png` (the world, opaque),
`…_Player.png` (the people) and `…_UI.png` (the interface), both transparent around what they show (the names as
[VRChat Photo Manager](https://github.com/phaze-the-dumb/VRChat-Photo-Manager) reads them; VRChat's docs only say
that multi-layer mode saves "multiple images for each Mask layer you have enabled", with alpha for the players and the
UI). This computer had no VRChat pictures to compare; the reading follows those names and accepts any other single
word after the size as a layer.

The listing (`worlds::group_layers`) makes the files of one folder sharing that name one picture: the picture itself
is its face, its layers listed back to front (Environment, Player, other masks, UI); it takes the world any of them has.
Without its picture (deleted), the bottom layer is the face; a layer alone is a picture of its own that still says
which layer it is. VRCX's renamed copy (`…_wrld_<id>.png`) counts as the picture; a numbered copy (`…_2.png`), another
millisecond or another size is another picture. Visited worlds counts such a picture once too.

The card says "3 layers" (or "Player layer"). The picture window shows a strip over the picture's bottom left: the
picture, then each layer (its thumbnail over a checkerboard, its name, an eye). A press on a layer shows it alone over
a checkerboard; the eyes show or hide the others stacked with it; the picture shows the picture again. The chosen
layer is what the window is about: Share publicly, Share to …, the folder and who it is shared with are that file's
(its own upload record and grants); with the picture chosen, sharing shares the picture as before. The strip waits
while a share runs. Dragging the card out hands out the picture.

For the picture editor: `Entry::layers()` is the list (`Layer { path, name, width, height, .. }`, back to front, empty
for a plain picture or a capture); the entry's own file is VRChat's composite, or the bottom layer when
`Entry::has_composite()` is false; `Entry::layer_entry(i)` is one layer as an entry; in the window,
`Dialog::layers.layered(&dialog.entry)` is the multi-layer entry while one of its layers is chosen. Layers decode with
their transparency (`Picture::Layer`, premultiplied, thumbnails kept in the cache as RGBA QOI).

Checked: unit tests `worlds::tests` (names, a whole set, partial sets, a lone layer, times, folders, sizes, VRCX's
copy, shrinking with alpha), `gallery_layout::tests` (the website's masonry test: places kept when appending, no
overlap, widths; rows; stand-ins; the grid), `shares::tests::a_chosen_layer_is_an_entry_of_its_own`; screenshots
`gallery-masonry` (also at `--width 900`, the picker under the filters), `gallery-masonry-hover`,
`gallery-masonry-scrolled` (from Alex, Shared · 1), `gallery-rows`, `gallery-grid`,
`gallery-layers-window`, `gallery-layer-chosen`, `gallery-layers-stack` on pictures the screenshot code makes
(`src/screenshot/gallery_layouts.rs`). Real multi-layer captures from VRChat were not tried.

## Editing

A picture on this computer has Edit in its window, and a 3D capture's view has Save this view: both open the photo
editor, which saves an edited copy beside the original (`docs/photo-editor.md`).

## Sharing and revoking

Preparing a 3D capture reuses its finished upload files on later shares and retries, including between Map explorer
and Photos & captures. A changed reconstruction, recorded cameras or start-view metadata makes it prepare again.
The local preparation cache lives with the reconstruction (up to about 70 MB per prepared capture); sharing still
sends the same web preview and full-quality PC, Quest and coarse files.

- Share publicly uploads an unuploaded item publicly, or shares an uploaded item with `public: true`.
- Share to … opens a searchable VRChat friends picker. An eligible signed-in VRChat session supplies friends and
  pictures through the service; these are merged with VRCX/VRCNext by user ID, preferring VRChat's data. Otherwise
  the local cache supplies the list. Loading and retry errors stay in the picker. It allows up to
  50 recipients and a 280-character optional message. New uploads are private before their grants are made.
- Recipients see who shared the item, Download / open and Request share. A request can name recipients and/or ask
  for public sharing. A 202 response is shown as a pending request, never as a successful publication.
- Requests opens incoming and outgoing share requests. Approvers can open the item, approve or deny; a public
  request for an explicit item shows the warning before approval and sends `acknowledgeExplicit: true` only after
  confirmation. Status changes appear immediately and roll back on failure, with inline Retry.
- Owners can unshare publicly, revoke a person or unshare everything. Re-sharers see and can revoke the grants the
  server returns for them. Revocation is shown immediately; a failure restores the saved item and offers Retry.
- Failures stay in the dialog, including in the picker. Busy actions disable immediately. A 409 with code
  `explicit-acknowledgement` displays the server message and Share anyway / Cancel; the retry sets
  `acknowledgeExplicit: true`. A locally flagged item shows this warning before its public request.

Visited worlds cards and capture rows share the same dialog. Share expands into Share publicly and Share to …
with animated width, label fades and separating halves. Its spring matches the website sign-in button's
340 stiffness / 25 damping / 0.8 mass. Pointer down acts immediately; outside press and Escape collapse it. A
world's card chooses its newest reconstructed capture, or its pictures when it has no reconstructed capture;
individual captures and selected pictures are also available from the world's detail.

## Copy public link

A 3D capture's window (Photos & captures and Visited worlds) has Copy public link beside Share publicly and Share to ….
It copies `https://orbiters.cc/maps/<world>/captures/<id>` (`dev.orbiters.cc` with the development environment), the
world's page opened on that capture, which Discord and other apps unfurl with the capture's picture and "3D capture,
click to open" (contracts `Docs/capture-sharing.md`, Public page). A capture that is not public yet is shared publicly
first by the same tray job as Share publicly (with the explicit warning; Share anyway still copies); a public capture
uploaded without its picture gets it then (`Api::set_capture_picture`). Uploads send the picture with the capture
(`capture_picture.rs`: the reconstruction's preview as JPEG). For items shared with you the button shows only when they
are public. The link stops showing the capture once its owner shares another capture of that world publicly (one public
capture per member and world).

## Selecting and dragging out

Ctrl-click, Shift-click, a card's corner check or a rubber band on the grid's empty space select cards; while some are
selected a click adds or removes one, Ctrl+A selects every card shown, Esc clears, and a bar at the bottom shows them
with Select all and Clear. Cards open on click (release), since a press may start a drag. Dragging a card, or any card
of the selection, hands the files to the app it is dropped on through Windows' own drag and drop (`drag_out.rs`): the
shell's data object for the files (what Explorer hands out), copy as the only allowed effect (VRChat's originals are never
moved), a drag image of the stacked pictures with a count badge. A VRChat picture is its PNG; a 3D capture or recording
is the PNG of its 3D view (its thumbnail before reconstruction), written once into `<cache>/dragged/<capture folder>.png`.
Items that are only on Orbiters are left out. The drag starts from `App::raw_input_hook` once the frame with the lifted
cards is on screen (Windows' loop is modal; the window does not repaint during it) and egui gets the button's release
afterwards. A drop back on this window copies nothing.

Checked: the ignored test `drag_out::win::tests::explorer_copies_what_is_dragged_out` drops the data object into
Explorer's drop handler for a folder; screenshots `gallery-select`, `gallery-select-mid`, `gallery-drag-lift` (and
`gallery-drag-image.png`), `gallery-copied`, `gallery-copy-link`, `gallery-copy-link-offer`. Real drags into other apps
were not tried by hand.

## From N4N0's menu

N4N0's menu on the avatar's screen (screen version 12) and in the SteamVR overlay shares 3D captures too (contracts
`Docs/orb.md`, Menu): the 3D picture page shows the last capture the desktop made with Share publicly and Share to …,
History lists the computer's captures (Photos & captures' own listing, sessions only, the newest 60), Share to … lists
the VRChat friends from `Controller::vrchat_source()` (the member's VRChat session, else VRCX or VRCNext), and a capture
flagged explicit (or answered 409) shows the contract's warning with Share anyway / Cancel. The tray controller
(`controller/menu_shares.rs`) turns each menu share into the same `capture_share` job as this page's, so uploads,
records and retries are shared; a share asked before a capture's 3D view is made waits for its reconstruction (queued
when nothing makes it) and gives up when nothing has made nor queued it for 15 s. Once a second it hands N4N0 where the
captures stand (recording, saving, waiting for or making the 3D view, sharing, shared, why it failed), which the screen,
the overlay and their pictures (the sessions' thumbnails, never sent for a flagged capture to everyone else's screen)
follow.

Share, revoke and download mutations are submitted over IPC to the tray controller. Its bounded job history is
part of the published snapshot; the dialog shows progress immediately and reconciles the controller's result.
Closing a window leaves the job running in the tray, with a completion notice waiting for the next window.
Automatic updates regard an unfinished sharing job as an upload in progress.

Uploads are remembered in `%APPDATA%/Orbiters/Desktop/capture-uploads-<dev|prod>-<member id>.json`. Each record binds
a local path to the returned `type`, `id`, `visibility` and latest Item. The upload identity is written before
fetching its Item or making grants: an interrupted grant request can retry without uploading the file again.
The account/environment scope prevents one member's upload identities being used with another account.
The controller reports the current VRChat world/instance/display name on joins and every five minutes, with a
60-second retry delay after failures, for the contract's shared-capture world listing.

## Capture-time content evidence

The desktop writes `capture-content.json` next to `manifest.json` when the capture worker starts a 3D picture or
recording. It is an atomic JSON sidecar:

```json
{
  "explicit": true,
  "explicitReasons": ["osc-parameter:NSFW/Toggle", "avatar-label:content_sex"]
}
```

The current worn avatar's known OSC parameter **names**, including parameters whose value is false, are matched
case-insensitively for `nsfw`, `nude`, `naked`, `sps`, `dps`, `tps`, `penetrat`, `orifice`, `genital` and `lewd`.
Reasons retain the original name without `/avatar/parameters/`. The app's existing OSC/OSCQuery path supplies these
names and forgets the previous avatar's parameters on avatar change. The worn avatar's Toolkit manifest is read
through the existing avatar-manifest parser and canonical filename helper. Its final parameter names contribute
the same signals; each control tagged `nsfw` adds `manifest-tag:nsfw:<labels joined by />`.

VRCNext's database is opened read-only. `user_tracking.profile_current_avatar_id` joins `avatar_tracking.tags` only
for users actually in the current VRChat log roster and whose cached `profile_location` exactly matches the
current world/instance. Real `content_sex` tags and the contract's `sexually_explicit` label are recognized, and the
raw tag is retained in the reason. [VRChat describes the sexual-content warning](https://hello.vrchat.com/blog/content-gating).
The inspected VRCX `cache_avatar` has no content-label field; its user-authored `avatar_tags` are not treated as
VRChat content warnings. Avatar names/descriptions are never used to guess explicit content.

Pictures whose timestamps fall inside a flagged session inherit its evidence when the gallery is indexed.
Both public and private uploads carry `explicit` and JSON `explicitReasons` multipart fields, alongside visibility
and the existing metadata. An absent cache/OSC signal is unavailable evidence; it cannot prove that an item is safe.
Existing recordings are not retroactively classified.

## Contract and verification limits

The binding contract is `contracts/Docs/capture-sharing.md`, including the coordinator's `c9025c2` update. The
single-item route is read as `{item}`; requests and answers have typed request/status/member/recipient structures.
Uploads send `instanceId` when recorded and `sourceKey`: a capture's folder name or the picture's cached original
SHA-256. Owner feed items are matched to local files by world, kind and sourceKey, with picture hashing queued off
the UI thread only for worlds whose owned server pictures need matching. Missing/null attribution remains unknown
until the server supplies it. Identity matching never relies on timestamps, display names or thumbnails.

Preview upload identity is read at the response root; picture identities are read from `added`, with numeric
`skipped`. An already-skipped picture without a returned id is resolved through the world's owned feed pages by its exact
sourceKey. If the server has not supplied that source identity yet, an inline error asks for a refresh; the client
does not guess its identity. Local upload records also bind server ids directly to paths.

Unit fixtures cover contract JSON, typed incoming/outgoing requests and approval/denial, 409 warnings, multipart
fields, cursor encoding/merging, revocation, source identity and instance association. A controller test writes the
actual content sidecar on a synthetic worker-start event, and another verifies that a tray job remains active after
a window snapshot is dropped and that its result round-trips through IPC. The downloaded-splat test covers
coordinates and malformed input. Offscreen screenshots use checked-in JSON fixtures and local thumbnail imagery.
They do not establish real server permissions, upload/public replacement behavior, request approval, grant
cascades, VRChat slot allocation, physical capture evidence, live download/rendering or in-client behavior.
No production uploads, version bumps, tags, pushes or releases are part of this implementation.
