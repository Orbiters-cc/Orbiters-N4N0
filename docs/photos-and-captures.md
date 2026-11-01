# Photos & captures

Photos & captures combines this computer's VRChat pictures and N4N0 recordings/3D captures with the signed-in
member's `/captures` feed. Mine, All and Shared with me select ownership; Search worlds filters local names and
sends `q` to Orbiters. More items use the server's opaque cursor. Overlapping pages are deduplicated; a repeated
cursor stops pagination. Results and filtered indices are retained between frames.

Items are ordered newest first and grouped by world and recorded instance. A session's manifest supplies its
instance; pictures use their recorded VRCX instance when present, otherwise the most recent retained VRChat join before their timestamp, only when it names the same
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
(`pages/gallery_layout.rs`); the choice is saved in the settings (`photos_layout`: `masonry`, `rows`, `grid`) and is
every view's of the page: All, Mine, Shared with me, Communities (a community's and an event's gallery among them) and My
assets. **Rows is the default** (Enzo, 2026-10-30). 0.3.4-dev.3 saved its default, Masonry, for everyone under
`gallery_layout`; that key is no longer read (the next save drops it), so everyone starts in Rows and Masonry comes
back only when picked again (test `settings::tests::photos_and_captures_start_in_rows_whatever_was_saved_before`).

- **Masonry** is the website's gallery (`server/frontend/src/components/elements/GalleryMasonry.jsx`):
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

The listing (`worlds::group_layers`) makes the files sharing that timestamp and size across configured folders one picture: the picture itself
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
editor, which saves an edited copy beside the original (`docs/photo-editor.md`). A picture shared with the member (or
theirs only on Orbiters) has Edit beside Download, and a community's or an event's gallery picture has Edit over its
picture in its window: the full picture is downloaded, and the copy goes into VRChat's pictures folder with the
picture's world and time, listed here at once.

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
The drag starts from `App::raw_input_hook` once the frame with the lifted
cards is on screen (Windows' loop is modal; the window does not repaint during it) and egui gets the button's release
afterwards. A drop back on this window copies nothing.

**Pictures from Orbiters** (shared with the member or by them, a community's or an event's gallery, an asset's) are
selected, dragged and copied like the others (`gallery_select.rs`, 2026-10-30). Windows needs real files when the drag
starts, so each is fetched whole first into `<cache>/dragged/orbiters/<world or gallery or asset> <UTC time> <key>.<ext>`
(the extension its bytes say; the name is what the app it lands in shows), by three worker threads: from the press on its
card (a drag may follow), as it is selected (the first 30 of a selection), or when a drag or a copy asks for it. A shared
picture is the sharing contract's file (`captures/picture/:id/file`, the member's credential), a shared 3D capture its
picture (`picture.url`, else its thumbnail), a community's its full picture through its renewed link
(`/galleries/images/:id/source`, then the listed one), an asset's its media link. The lifted cards wait for what is
still coming (the bar: a spinner and "Getting 2 of 3 from Orbiters…"), and Windows gets the drag once everything is
there while the button is still down; let go earlier, the fetching goes on and the bar says to drag again in a moment.
A picture that cannot be fetched is left out (alone, the bar says why). A file fetched before (another drag, another
run) is taken as it is. The drag image of a picture from Orbiters is a small copy made as it was fetched.

**Copy** (Ctrl+C with cards selected, a card's menu) puts the same files on the clipboard as Explorer's Copy does
(`drag_out::copy_files`: the shell's data object, `OleSetClipboard`, flushed so it outlives the app): pasted in a
folder, Discord or a browser they are copied there. It waits for pictures from Orbiters as a drag does. A multi-layer
picture's menu also has Copy with its layers (the picture and each layer file).

Checked: the ignored test `drag_out::win::tests::explorer_copies_what_is_dragged_out` drops the data object into
Explorer's drop handler for a folder; `gallery_select::tests::pictures_from_orbiters_are_fetched_from_where_they_are`
(each kind's source, the file's name) and `remote_cards_are_selected_and_dragged_with_the_local_ones` (a selection of a
local picture, a community's fetched and an asset's still coming: the drag waits, then hands out all three; a failed one
is left out and said); screenshots `gallery-select`, `gallery-select-mid`, `gallery-drag-lift` (and
`gallery-drag-image.png`), `gallery-copied`, `gallery-copy-link`, `gallery-copy-link-offer`, `gallery-select-remote`
(three asset pictures selected and lifted, the bar waiting for two). Real drags and pastes into other apps and the
clipboard copy were not tried by hand; real fetches of shared and community pictures were not run.

## My assets (2026-10-30)

A fifth filter, **My assets**, shows the **showcase galleries** of the Orbiters assets the member has (made, bought,
licensed): on an asset's page (`https://orbiters.cc/assets/7`, the website's `AssetGallerySection` /
`AssetShowcaseGallery`) the gallery of pictures from the Discord rooms its creator chose. Not the assets' own pictures
(their thumbnails, product gallery or store previews): Enzo, on 0.3.4-dev.4, "I don't want to see the asset
thumbnails". One group for each asset that has showcase pictures (its name, a pill with its kind and Made by you /
Yours on Orbiters, how many showcase pictures and when the latest was posted, Open on Orbiters to `/assets/<id>#gallery`,
`/commissions/assets/<id>` for a commission listing), its pictures in its page's order (`sort=relevant`), the group at
its newest picture's time; All shows them among the rest. Search finds an asset by name or kind, a picture by who
posted it or its words. Cards are the gallery's own (the layouts, selection, dragging out, Copy, their menu, Edit); a
picture opens in the community picture window (who posted it, its words, ←/→ through the showcase pictures shown) with
the asset's name and page (`pages/gallery_assets.rs`, `orbiters_core::assets`).

Server routes (backend `dev`, 2026-10-30; `utils/authUtils.js` `TOOL_ROUTES`):

- `GET /assets/library` (new, read only, tool route): the assets the member made, holds an enabled licence to, or
  reaches through a supporter tier or a Discord role (`services/assets/memberLibrary.js`; unlike the website's
  `GET /assets` it writes no licence rows and grants no roles). `{ assets: [{ id, name, type, how: made|licensed,
  commissionListing, createdAt, updatedAt }] }`.
- `GET /assets/:id/showcase-images` and `/:placementId/source` (existing, now tool routes): the showcase as the page
  shows it to the member (its owner sees every picture, others the rooms open to their whole server) and a picture's
  renewed Discord link. Hiding a picture (`PATCH …/hide`) stays the website's.
- A server without these (production until the backend is deployed): the library answers an error, so the app falls
  back on `GET /creator/assets` and `GET /mcb/assets` for which assets; showcases answering 401 to every asset show
  "Orbiters does not let this app read your assets' showcases yet." with Try again.

A showcase picture carries its asset (`community::Picture::asset_id`, `worlds::Picture::Gallery { asset }`,
`gallery_select::Remote::Gallery { asset }`, `editor::files::Remote::asset`): its expired Discord link is renewed at
`/assets/<asset>/showcase-images/<id>/source` when shown, dragged out, downloaded or edited. At most 120 pictures per
asset (two pages of 60), six assets read at once.

Checked: `orbiters_core::assets::tests` (the library's and a showcase page's JSON, groups by newest picture, a hidden
asset's 404, every showcase refused, the fallback merge), `gallery::assets::tests::each_asset_is_a_group_of_its_showcase_pictures`
(no asset thumbnails or media links, renewal at the asset's route); backend `test/memberLibrary.test.js` (made,
licences, tier and role access, read only, hidden listings; both routes with the tool credential through the mounted
routers, hide refused) and `test/authUtils.test.js`. Screenshots `gallery-assets`, `gallery-assets-all`,
`gallery-assets-open` use the fixture (showcase pictures by made-up members, this computer's pictures standing in).

## The cards' menus (2026-10-30)

A right press on any card opens its menu (`gallery::card_menu`, `orbiters_ui::popup::context_menu`), the actions the
item's window, the selection and its group already have:

- a VRChat picture: Open, Edit, Share publicly (when not public) and Share to … (theirs to share), Copy (Copy the N
  selected when it is in a selection), Copy with its layers (multi-layer), Show in folder;
- a 3D capture or recording: Open, Reconstruct (not reconstructed, not waiting), Share publicly / Share to … (once
  reconstructed), Copy public link (public), Copy its picture, Open its folder;
- shared with the member: Open, Ask to share, Copy public link and Open on Orbiters (a public 3D capture), Copy,
  Download; the member's own on Orbiters only: their shares instead of Ask to share;
- a community's, an event's or an asset's picture: Open, Copy, Download, Open on Orbiters (the gallery, the asset);
- then Instance… (the instance popover, from the card) when the group's instance is known, World on VRChat for a VRChat
  world, and Select / Deselect.

Share publicly, Share to … and Edit open the item's window out of its card and do it there (its warnings, its picker,
its editor). Download saves the fetched picture in `Documents/Orbiters/Downloaded pictures` (another number when the
name is taken) and shows it in Explorer. Nothing deletes from the menus: Photos & captures has no removal today (Manage
captures keeps deleting captures, the window keeps unsharing). Screenshots `gallery-menu` (a 3D capture's),
`gallery-menu-asset` (an asset picture's; with the fixture's stand-in pictures Copy and Download are left out, as
nothing can be fetched for them).

## From N4N0's menu

N4N0's menu on the avatar's screen (screen version 12) and in the SteamVR overlay shares 3D captures too (contracts
`Docs/n4n0.md`, Menu): the 3D picture page shows the last capture the desktop made with Share publicly and Share to …,
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


## Additional picture folders and archive thumbnails (2026-10-30)

Photos & captures has a **Manage folders** button beside Manage captures (it was Folders until 0.3.4-dev.4; its
popover closed on the very click that opened it: the button acted on the release, and that release also counted as a
click outside the popover, so it opened and closed on one frame. It now opens on the press like the page's other
buttons, and a click on its own button is never "outside"; it shrinks back into the button when closed.) Add folder opens the native directory picker;
Remove removes only that source. `Settings.picture_folders` persists the additional absolute paths. VRChat's configured
output and default Pictures/VRChat folder remain included. The popover checks availability on a worker and marks
unavailable folders Offline. A disconnected drive retains its known pictures and cached thumbnails; deleting a
picture from an available folder removes it on the next scan. Removing a source removes its indexed entries too.
Subfolders are scanned recursively (64-level safety bound, symbolic links skipped), and overlapping roots are walked
once. The existing screenshot index reads only PNG headers and metadata, seeking past image data.

The gallery reads the saved index first and checks sources in a background job, then refreshes when that job changes
the index. It checks again every 15 seconds while the page is shown; there was no existing picture-folder watcher to
extend. Other library callers keep their existing synchronous worker listing. Pictures without world metadata remain
visible under Pictures without a world, with the gallery's day grouping. Multi-layer files match across archives by
VRChat timestamp and dimensions. VRChat XMP supplies world/name/date; VRCX's Description iTXt supplies world and
instance using its [published schema](https://github.com/vrcx-team/VRCX/blob/master/Dotnet/docs/screenshotMetadata-schema.json).
The retained join history remains the fallback when no instance was recorded.

The measured bottleneck was a cache eligibility mismatch: gallery cards request 560-pixel thumbnails, while the
old cache accepted at most 480. Every gallery visit decoded the full PNG again. The cache now includes 560-pixel
cards and transparent layers, stores small QOI files in Orbiters' local cache (`thumbs-v2`, normally on C:), and
validates path, requested width, source size and nanosecond modification time. Indexed cache hits use the identity
already read by the background scan, without querying or opening an external-drive original. A file changed during
decode is not cached. Bad cache data is regenerated; unavailable originals can use their last valid preview.
Writes use a 512 MiB budget, oldest writes evicted first, and stop when less than 600 MiB would remain free.
The existing three workers, visible-request priority and 48 MiB texture budget remain in use.

A Windows shell thumbnail provider, denoising and an additional blurry-preview format were not added: the measured
repeat-load bottleneck is removed by using the existing lossless thumbnail path. The initial uncached decode still
reads the full PNG. This benchmark does not establish cold USB/HDD performance or prove a best concurrency count.

Reproduce with a small disposable folder of representative PNGs:

```powershell
$env:CARGO_INCREMENTAL = '0'
$env:ORBITERS_PICTURE_BENCH = '<disposable folder>'
cargo test -p orbiters-desktop picture_folder_benchmark -- --ignored --nocapture
```

Measured in the debug profile on H:, seven readable 3840x2160 / 2160x3840 VRChat pictures copied from the monthly
resources folder: baseline listing 291.9 ms, first seven thumbnails
3460.7 ms, second pass 3416.5 ms. With 560-pixel caching: listing 14.6 ms, first pass including cache writes
3557.0 ms, cached pass 23.5 ms (about 145 times faster). Listing runs had different OS cache warmth: their difference
is not attributed to an indexing speedup. A separate warm rerun loaded the saved index in 0.447 ms and
served the cached thumbnails in 26.2 / 25.8 ms on two passes. No release-build or physical unplug/replug timing is claimed.

Verification: `cargo check -p orbiters-desktop --tests`; `cargo test -p orbiters-core visited::tests` (9 passed,
1 ignored); `cargo test -p orbiters-desktop worlds::` (37 passed, 1 benchmark ignored, including existing overlay
world tests). Fixtures cover deep/overlapping roots, saved-index reload, offline retention, deletion/removal,
VRCX world/instance, same-size modification-time cache invalidation, corrupt cache data and cross-folder layers.
`pages::gallery::folders::tests::the_click_that_opens_it_leaves_it_open_and_add_folder_saves_the_picked_one` opens it
with a real press and release, checks it stays open, presses Add folder (the picker, faked, runs on another thread
than the UI's) and checks the folder is saved once, then a click outside closes it and the button toggles it.
`gallery-folders-click` opens it in a screenshot the same way (and panics if it closed). The list of folders is
checked again as soon as the settings' folders change (`visited::screenshot_dirs_with`, the snapshot's folders, not
the file the tray saves a moment later). `gallery-folders` is an offscreen screenshot scenario with an available default folder and offline archive,
rendered and inspected at widths 1280 and 900 after `cargo build -p orbiters-desktop`. The settings round-trip test
also passes with an extra archive folder.
Native-picker interaction and physical drive reconnection remain unverified. These changes are local; the lead owns
commits, release builds and publication.
