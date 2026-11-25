# 3D worlds in the SteamVR overlay

The [2026-10-30 latency investigation and results](overlay-latency-results-2026-10-30.md) supersede the original
scheduling policy and add repeated measurements, an independent Opus review, and remaining headset checks.

**What it does.** N4N0's menu in the play space (the SteamVR overlay) has a pill under it, "3D worlds". It opens a
panel in the menu's place: a search field (the panel's own keyboard, or the computer's keyboard as the menu's search
takes it) over VRChat's worlds in general (see [Every VRChat world](#every-vrchat-world)), the ones members captured
and shared badged 3D, as cards with each world's thumbnail, likes, labels and frame rate (or the people in it now),
ordered as listed, most liked or smoothest first and kept to the labels picked (and to the 3D captures). Pressing a
world without a capture opens its page (Invite me or Launch VRChat, Open on VRChat). Pressing a capture's card
downloads that capture and shows it around you: its ground on your play space's floor, its start view where you stand, facing the way
you face, appearing from there outward. Its page has what Orbiters knows of the world and its feedback, given there as
in the VRChat world and on the website (Like, labels, comments). The capture's controls are on a bar of their own under
the menu or the panel, whatever page shows, while a capture shows or loads: the capture (its page a press away), See
VRChat (the capture at 55 %, VRChat behind it), Recenter, how sharp it is (Light, Balanced, Sharp) and Hide. The
capture stays around you after the menu closes. Code: `crates/orbiters-desktop/src/overlay/worlds/`.

## What the search does, against the Map explorer and the VRChat world

Inventoried 2026-10-30 from the website (`server/frontend` `MapExplorerPage.jsx`, `MapWorldPage.jsx`,
`WorldFeedback.jsx`, `WorldPerformance.jsx`, `MapPictures.jsx`; API `backend/src/routes/mapsRouter.js`) and the world
(`Assets/N4N0Viewer/Scripts/SplatMenu.cs`, `SplatFeedback.cs`, `SplatInfo.cs`, `SplatPerformance.cs`,
`SplatImages.cs`). Nowhere is there a rating scale: "rating" is the Like (one per member per world), as on the website
and in the world. Feedback belongs to the **world** (`wrld_…`), so a world's captures share it.

| Feature | Map explorer (orbiters.cc/maps) | Viewer world | Overlay before | Overlay now |
|---|---|---|---|---|
| Search | VRChat's world search (Orbiters' known worlds when VRChat fails), a pasted world id or link | name, author or sharer containing the text | as the world | every VRChat world (the member's VRChat session, else the Map explorer's search), the captures matching the name, author or sharer first; browsing: 3D captures, Popular, Active now, Favorites, Visited |
| Order | Smoothest first (share of the refresh rate, then fps) | as listed (newest) | as listed | Newest, Most liked, Smoothest |
| Filters | none | none | none | labels (all those picked; how many worlds have each), shared by the world's author |
| Card | cover, 3D badge, picture count, fps badge, name, author, capacity | thumbnail, name, by author, 3D by sharer, Official | thumbnail, name, author, splats, Official seal | + likes, fps badge (smooth/fair/heavy as the website colours it), the two labels given most |
| Shared with some players only (`shared`, `for`) | (the captures API) | listed for those players only | listed for everyone | listed for the player whose VRChat name (VRChat signed in in Orbiters - N4N0) is in `for`, else not |
| Like ("rating") | heart with the count | Like, N likes | none | Like with the count, the member's lit |
| Labels | the 18, each with its count, the member's toggled; AI world with a reason of 10 words | the same, a reason page | none | the five given most on the capture's page, all 18 on Labels (counts, the member's lit, toggled); AI world asks its reason on the keyboard (words counted, refused under 10 or over 500 characters) |
| Others' reasons ("why") | listed | – | none | not yet (see below) |
| Comments | 20 a page and older, write, delete one's own, report others' | 5 a page, write, delete, report | none | the newest one under the picture; Comments: the newest 20, 4 a page, Write a comment on the keyboard, Delete on one's own (pressed twice); report not yet |
| Feedback for everyone | per world | the listing's `feedback`, else the app's local address | none | the listing's on cards and as a world's page opens; the app's read (the member's part with it) once there |
| World numbers | capacity, visits, favorites, author tags, description | capacity, visits, favorites, description, capture date, splats | description, splats, sky, sharer | + capacity, visits, favorites, capture date, the author's tags |
| How smoothly VRChat runs | typical fps, slowest, % of refresh, by crowd size, members and minutes | the viewer's own FPS and automatic quality | none | typical fps with smooth/fair/heavy, slowest, % of refresh |
| Pictures | 48 a page, shown whole with who and when | gallery pages of 6 | none | the first 48, 8 a page, shown whole with who and when |
| The world's other captures | a tab per sharer | up to 3 buttons | each its own card | "N more captures" steps through them (showing each) |
| Capture controls | – | Quality (with automatic), Debug | Hide, Recenter, See VRChat, sharpness on the capture's page once shown | on the bar, whatever page, while it shows or loads |
| Capture details: share, download `.splat`, report the preview | signed in | – | none | not in the overlay (the website) |
| Open in VRChat | vrchat.com link | portal | none | a world without a capture: Invite me (an invite to oneself, signed in to VRChat) or Launch VRChat, and Open on VRChat (vrchat.com in the computer's browser); a portal cannot be dropped from an overlay |

## Every VRChat world

Enzo, after trying it in VR (2026-10-30): the search should list VRChat's worlds in general, not only those with a 3D
capture, especially when searching, with the member's own VRChat account when possible. Code:
`overlay/worlds/finder.rs` (lists, merge, join links), `panel.rs` (sections, cards, a world's page), `mod.rs` (asks,
debounce), orbiters-core `vrchat_api.rs` / `vrchat_source.rs` / `maps.rs` (requests), `controller/vrchat_data.rs`
(the gate).

**What the panel lists.** While nothing is typed, a row of sections: **3D captures** (Orbiters' listing, as before),
**Popular** (VRChat's popular worlds; "Featured" without a VRChat session: Orbiters' recently captured or pictured
worlds), **Active now** and **Favorites** (VRChat only: signed out of VRChat, the panel says where to sign in and asks
nothing), **Visited** (the worlds VRChat's logs saw on this computer, `visited-worlds.json` the tray service keeps, the
latest first, at most 240). Typing two characters and pausing 400 ms (Done: at once) searches every world; the row then
has **3D only** and says who answers ("from your VRChat account" or "from Orbiters"). One character still filters what
shows. A world of any list that Orbiters has captures of is listed as its captures, each its own card with the 3D badge
(the list's thumbnail and people kept with them): pressing it shows it as before. In a search, the captures whose name,
author or sharer match come first, then VRChat's answer in its order, a world listed once. Sort (now a chip that steps
Newest / "Best match" / "Top" / "Latest" as listed, Most liked, Smoothest), labels, Shared by its author and **Has a 3D
capture** (the filter page) apply to whatever is listed: likes and labels come from the captures' listing, else
`GET /maps/vrchat/feedback` (every world given feedback lately, read with the listing), else the world's feedback once
its page asked for it; frame rates from the world's Map explorer page or Orbiters' search answer.

**Requests.** A page is 24 worlds, one more asked to know whether another follows; the next page is asked as the
results near the end of what was read (at most 20 pages). Pages are asked one at a time on a thread of their own (the
one asked meanwhile waits, the latest only), kept five minutes (an error 20 s; Try again lets it go at once) and let go
when the account, the environment or the VRChat session changes.

| List | Signed in to VRChat (the member's session, in the tray service) | Otherwise (Orbiters, public, no credential) |
|---|---|---|
| Search | `GET /worlds?search=…&sort=relevance&releaseStatus=public&n=25&offset=…` | `GET /maps/search?q=…&page=…` (the website's VRChat search, Orbiters' known worlds when VRChat cannot be asked) |
| Popular | `GET /worlds?sort=popularity&order=descending&releaseStatus=public&n=25&offset=…` | `GET /maps/featured?page=…` |
| Active now | `GET /worlds/active?n=25&offset=…` | not asked |
| Favorites | `GET /worlds/favorites?n=25&offset=…` | not asked |
| Visited | local history | local history |

The overlay asks the tray service (`ToService::VrchatData`, `Request::WorldList { list, page }`, answered
`Response::Worlds`); the service checks the request (a search of at least two characters, at most page 20) and its
scope, then `vrchat_source::Source::worlds` uses the personal session (`vrchat_api::Client::find_worlds`: the "worlds"
request family, one a second with the world lookups, 429 backing off for the whole client; pages kept five minutes, the
favorites one minute, at most 64, forgotten on sign-out) or falls back to Orbiters' Map explorer
(`Api::find_worlds`). VRChat's `LimitedWorld` gives name, author, thumbnail, capacity, people now, visits, favorites
and tags, not the description: a world's page reads it, its frame rate and its pictures from its Map explorer page when
the page opens (`GET /maps/worlds/:id`, cached three days), so cards of a list cost no request to Orbiters but their
thumbnails (VRChat's pictures over https, loaded and kept by world as before, no credential). Visited worlds have no
picture in the history: their cards read their Map explorer page.

**A world without a capture.** Its page has its picture, author, numbers, people in it now, description, author tags,
frame rate, Orbiters' likes, labels, comments and pictures (given as for a capture), and in the capture's Show place:
**Invite me** when signed in to VRChat (`Request::InviteSelf` → `POST /invite/myself/to/{world}:{instance}`, the
"invites" family; the one thing sent besides a picture upload, refused unless the instance is a public one this
builds: five digits and `~region(us|use|eu|jp)`, the region of the instance the player is in, else `us`), VRChat
showing the invite in the game; otherwise **Launch VRChat** (`vrchat://launch?ref=orbiters.cc&id=<world>:<instance>`,
opened with Windows' handler for `vrchat://`). **Open on VRChat** opens `https://vrchat.com/home/world/<id>` in the
computer's browser (also on a capture's page whose world has no other capture). What happened is said on the page.
The world's page stays open while the lists change under it.

Offline review: `--screenshot <out> --only worlds-overlay` adds `panel-search.png`, `panel-search-3d-only.png`,
`panel-search-orbiters.png`, `panel-search-failed.png`, `panel-typing-search.png`, `panel-typing-searching.png`,
`panel-browse-popular.png`, `panel-browse-active.png`, `panel-browse-visited.png`,
`panel-browse-favorites-signed-out.png`, `panel-world.png` (Launch VRChat) and `panel-world-go.png`; the earlier
captures' shots show the 3D captures section.

**Giving feedback.** The panel sends what the member gives as N4N0's menus do: `N4n0Command::Feedback` with the
contract's `FeedbackAsk` to the tray service, whose queue (`controller/world_feedback.rs`, kept on disk, coalesced,
retried) sends it to `PUT /maps/worlds/:worldId/feedback` and the comments routes under the signed-in account. Opening
a capture's page asks for its world (`FeedbackAsk::Show`, again every 2 minutes while it shows); the service now hands
the menus the feedback of the six worlds a menu asked about lately besides its subjects, so the panel shows any
capture's world, the member's part with it. A press shows at once; the service's answer replaces it. Signed out, or
before the world's feedback is read (what the member gave is not known yet), a press says why and sends nothing.

**Not done, for want of a route or a field** (the APIs exist; the overlay's path to them does not):

- Reporting a comment: `POST /maps/comments/:id/report` and the queue's `Action::Report` exist, but the menus' ask
  (`orbiters_n4n0_core::feedback::FeedbackAsk`) has no Report.
- Others' reasons for AI world: `GET …/feedback` returns `justifications`; the menus' `WorldFeedback` does not carry them.
- Comments older than the newest 20: `?before=` exists; `orbiters_core::world_feedback::Client::read` does not page.
- Frame rate by crowd size: in the world page's `performance.by_crowd`, not shown (room).
- Reporting a 3D preview (`POST /moderation/reports`, `targetType: map_preview`) needs a reason form: on the website.
- A rating scale (stars, averages): no API has one.

## The capture's bar

The controls used to be on the capture's page only, and only once it showed: not while it downloaded or prepared
(Cancel alone), not on the results, the search or another capture's page (Show around me instead), not after Back,
and not while the panel was closed (the pill alone); a failed download also lost its page's controls and its error
on the next turn. So See VRChat came and went with the page and the moment. They are now on a SteamVR overlay of
their own (`{key}.worlds-bar`, 1024 × 88 px, as wide as the menu, 12 mm under it, sort order 10), shown whenever a
capture shows or loads and the menu or the panel shows its front, whatever page: the capture (its loading ring or
progress, a press opens its page), See VRChat, Recenter (once it shows), Light / Balanced / Sharp, Hide (Cancel while
it loads, Close after an error). With no capture the pill shows as before. `--screenshot <out> --only worlds-overlay`
draws the bar alone (`bar-*.png`) and under the panel with a capture's page open and closed (`with-bar-detail.png`,
`with-bar-browse.png`). Like the menu, the bar goes when the menu closes: the capture stays around you.

## Captures kept for showing again

`overlay/worlds/cache.rs`. **In memory**, the ready captures (splats ordered for the reveal, sky map): about 60 bytes a
splat, so 60 MB for a million; at most 384 MB (`ORBITERS_3D_WORLDS_MEMORY_MB` changes it), the least lately shown let
go first, never the one on screen. Showing one of them again is at once: no download, no preparing, straight to the
GPU. **On disk** (`%LOCALAPPDATA%\…\cache\3d-worlds`), the files as downloaded (the slot's PC tree or the API's
`.splat`, the sky map) within 2 GB, the least lately used removed first, written only while the drive keeps 10 GB free
after them. A kept file is asked about again with `If-None-Match` / `If-Modified-Since` when the storage sent `ETag` /
`Last-Modified` (a `304` downloads nothing; unreachable, the kept file serves), else trusted by its name: listing id,
slot, the upload's key and splat count (a capture shared again gets a new key; a slot's tree also carries its key,
checked when read, and a file that fails its check is removed). The listing gives no updated time, so the key is the
version.

**Same captures, same placement as the VRChat world.** The panel reads what "N4N0 - 3D picture viewer"
(`Assets/N4N0Viewer/Scripts/SplatMenu.cs`) reads: `GET /maps/vrchat/previews`, searched as its menu searches (the
world's name, its author or who shared it containing what is typed). It loads the same file the world's PC renderer
loads, the slot's PC tree (`files.orbiters.cc/public/vrchat/slots/<slot>/pc`, key checked against the listing as the
world does) and the slot's `sky.jpg`, and draws the tree's leaves (its finest splats: 786 432 for VRChat Home). On a
development server, whose slots are published to storage the app has no address for, it reads the preview's web
`.splat` and sky map through the API instead. Placement is `SplatViewer.AlignToView` with the anchor at the head's
place on the floor (Y 0 of SteamVR's standing universe) and the head's level facing: the start view's eye over the
head, its `floor` on the floor. One difference: a preview without `floor` (all five public ones today) keeps its own
heights in the world, the world's ground being its anchor; here there is no world ground, so its start eye stands
1.6 m (its `eyeHeight`, else the contracts' default) over the floor, where a standing person's eyes are.

## How a 3D capture shows in SteamVR while VRChat owns the scene

SteamVR gives the scene to VRChat; another process can only show overlays: textured quads the compositor draws over
the scene. Options looked at:

| Option | Head turns | Head moves | Cost | Verdict |
|---|---|---|---|---|
| A head-locked side-by-side overlay drawn every frame from the predicted pose | swims with every prediction error, and a late picture moves the whole world with the head | right | a full stereo view at 90 Hz | no: any late picture is felt |
| `SetOverlayTransformProjection` (a projection overlay per eye) | as SteamVR reprojects it (undocumented) | as drawn | per frame | no: undocumented, nothing to verify it against without a headset |
| `Panorama` / `StereoPanorama` flags | – | – | – | legacy flags, no documented behaviour |
| **World-locked side-by-side quads forming a cube around the head** | **exact at the headset's rate** (see below) | parallax follows the batches | only when the eyes move | **chosen** |

**Why a world-locked cube is exact for head turns.** Each face of the cube is a quad fixed in the play space, its
texture side by side (`VROverlayFlags_SideBySide_Parallel`: the left half to the left eye, the right half to the right
one). Each half holds the capture projected from that eye's position onto the face's plane (an off-axis pinhole camera
through the face, `stereo.rs` `Cube::camera`). Such a picture is right for every direction the eye looks from that
position: SteamVR's compositor draws the quads with the latest head pose every frame. Rotation about a fixed eye
position is corrected independently of the picture update rate. A real head turn also moves the eyes about the neck
and head, so it still needs fresh parallax pictures; a stale eye position can look like lag despite stable quads.

**The cube.** Six faces 1.5 m from its centre (the eyes may stand anywhere inside: the projections stay exact), each
1024 + 2 × 4 texels a side per eye (Balanced; Light 768, Sharp 1440), overlapping their neighbours by 8 texels so
SteamVR's filtering never shows a seam. When the eyes wander 45 cm off its centre, a second cube around the head draws
its six faces and takes over (shown, then the old one hidden: SteamVR shows them in either order without a gap). The
cube sits under N4N0's menu and the pill (sort order 0 against 10).

**A batch.** Drawing every face an eye sees for both eyes would be ten views of the splats a batch (an Index's eye sees
4–5 faces: its field of view, about 108°, is wider than a face's 90°). Instead each batch draws **one wide view per
eye**: the eye's own field of view (`GetProjectionRaw`, `GetEyeToHeadTransform`, the pose predicted to when the batch
lights the displays: `GetTimeSinceLastVsync`, `Prop_SecondsFromVsyncToPhotons_Float` and the batch's own time) widened
6° each side, then **resamples** it into the faces it covers (`resample.wgsl`: a homography per face and eye, exact as
both are projections from the same eye; texels it does not cover keep what they had). A new cube's faces, and now and
then a face out of view, are drawn whole instead. A batch normally goes once an eye moved 2.5 mm, lowered to 1 mm
during motion; quiet tracking has a 3 mm fallback and a stop gets a settling picture. Turning 4° also requests an
update. The caps remain 20 / 30 / 45 a second (Light / Balanced / Sharp). After reveal and stale-face maintenance,
a stationary view needs no continuous rendering.

**Handing pictures to SteamVR without leaving the GPU.** The splats draw on a wgpu device (D3D12) on the headset's
adapter (`GetDXGIOutputInfo` → LUID). Each face texture is a shared D3D12 resource (simultaneous access, so it is back
in the common state after each submission) opened by a small D3D11 device on the same adapter, which hands it to
SteamVR with `SetOverlayTexture` (the D3D11 path every overlay of the app uses). Each face has two textures used in
turn; before writing one, the GPU waits on a shared fence the D3D11 side signals after handing textures over
(`interop.rs`).

**Sharing the GPU.** An overlay-specific pacer observes new scene frame timings, including while a batch is in
flight. A dropped or GPU-late frame overlapping an overlay job can increase its interval once per job. Each
500 ms without another escalation removes a level. The overload interval is capped at 250 ms for affordable
jobs; a separate 40% render-wall-cost ceiling can require longer intervals for expensive batches. Unlike the
reconstruction governor, persistent scene lateness cannot indefinitely prevent the first cube or all updates.
Temporal overlap is a conservative heuristic, not proof that the overlay caused the late scene frame.

Ready pictures are published during short service slices and after menu/dashboard painting on the existing overlay
thread. Nominal quality caps remain 20/30/45 Hz. Prediction separates wide, face and cube readiness, chooses a
display boundary after estimated completion, and adds a bounded quarter of the picture's lifetime during stable
motion. Both calibrated eyes keep their own frusta but share an image-buffer size. See the results for limits.

**The reveal.** The capture appears from its start view outward over 0.9 s, as the world's reveal does: its splats are
ordered by distance from the start eye and each batch draws those within a radius growing on a log scale; the
overlays fade in over 0.45 s meanwhile, and out over 0.35 s when hidden.

## Measured (offline, RTX 4080, VRChat Home's PC tree, 786 432 splats)

`orbiters-desktop --world-view <out> <pc tree> [--load <s> [--still] [--quality light|balanced|sharp]]`. The computer
was in use meanwhile (two Unity editors, browsers, the installed app: 15–20 % GPU at rest), so absolute times vary by
run; comparisons are within a run.

**Correctness** (an Index-like headset, 1440 × 1600 an eye; each eye's picture composed from the faces as SteamVR
would, against the renderer drawing that eye's view directly; PSNR over RGB):

| Case | PSNR |
|---|---|
| Standing at the start view, faces drawn whole | 33.4 dB (left), 33.7 dB (right) |
| Head turned 35° and tilted 20° down, faces not redrawn | 40.6 dB |
| Looking down 70°, faces not redrawn | 49.8 dB |
| A wide batch after turning 25° and moving 1.4 cm | 40.9 dB (left), 41.2 dB (right) |
| Eye moved 2 / 5 / 20 / 50 mm, faces not redrawn | 32.7 / 30.7 / 26.3 / 23.3 dB |

Faces drawn whole differ from the direct render mostly by the splats' screen-space approximation (each face camera
looks another way than the eye); a wide batch is drawn looking the eye's way and comes closer. The faces show no seam
(`eye-left.png`, `stereo.png`). The shared texture read on the D3D11 side was byte for byte what D3D12 drew.

**Cost of a batch** (median of 12, the head turning 3° between batches; the wide view's size per eye):

| Quality | Faces (per eye) | Wide view | Batches a second at most | Batch wall time, including GPU waits |
|---|---|---|---|---|
| Light | 776 × 776 | 1.15 Mpx | 20 | 5.7 ms |
| Balanced | 1032 × 1032 | 2.05 Mpx | 30 | 6.4 ms |
| Sharp | 1448 × 1448 | 4.05 Mpx | 45 | 9.0 ms |

Drawing the faces an eye sees directly instead (4–5 faces, both eyes) took 8–13 ms **a face**, 40–65 ms a batch: most
of a view's time is the splats' projection and the sort of their tile entries, whatever its pixels (a face at 776 or
1448 pixels across cost about the same), so fewer views matter more than fewer pixels. A new cube (six faces drawn
whole, both eyes) takes about 50–80 ms once.

**Cost to a game on the same GPU.** As docs/igpu-experiment.md measured the window: `gpu-lab load 4080 20 --work 1600`
(a GPU-bound stand-in, about 8.6 ms a frame at 2560 × 1440, back to back) alone, then beside `--world-view --load 26`
(the overlay's batches with the head turning ±35° and swaying ±4 cm every few seconds: never still, so a batch is
always due), two rounds, frames a second of the stand-in:

| Beside it | Round 1 | Round 2 | Change |
|---|---|---|---|
| nothing (three blocks around them) | 100.9 | 99.4 | – (98.2–104.0 over all blocks) |
| a still head | 102.8 | | none measurable |
| Light, moving | 93.3 | 92.7 | −6.6 % |
| Balanced, moving | 90.9 | 89.4 | −9.4 % |
| Sharp, moving | 86.5 | 83.4 | −14 % |

Beside the stand-in the batches ran 16–18 a second (16–24 ms each, sharing the GPU). The first version, drawing each
seen face directly at up to 45 batches a second, cost the same stand-in 20–30 % of its frames. A stand-in at 90 Hz with
headroom (`framegame`, 4.5 ms frames) lost no frames beside it (late frames 0.1–0.3 % with or without; its 95th
percentile frame 1–2 ms longer). For comparison, the window's 3D view orbiting a million splats costs the GPU-bound
stand-in 11.4 % (igpu-experiment.md). In VRChat the governor's back-off adds to this: batches stop while VRChat's
frames come late.

**Memory.** 515 MB of graphics memory held by the process with a capture shown at Balanced (DXGI's local video memory
of the offline check: the splats and the renderer's buffers, its sort included, twelve face textures, six face
pictures and two wide views), all freed when it is hidden (the render thread and its device end).

## What it covers, and walking

Overlays draw over the whole scene: while a capture shows, VRChat's world, the other players, your own avatar and
hands are behind it (See VRChat shows them through at 45 %). The capture is fixed in the play space: you walk through it
by walking in your room; VRChat's own locomotion (the stick) moves VRChat's world behind it, not the capture.
Recenter puts its start view where you stand again.

## Next, if it is kept

- A quad splat renderer like the world viewer's (each splat a quad, sorted once per head move by distance on the
  processor, both eyes and every face from one order): no tile-entry sort per view, which is most of a view's cost now.
- The tree's level of detail (its merged boxes for far places, cut from where the head stands, as the world draws it):
  fewer splats per view.
- Hands through the capture: holes where the controllers are, so VRChat's hands show.
- The avatar's projected screen (players who use N4N0's screen never see the overlay's menu, so never the pill): a new
  screen version with a 3D worlds page.

## What was not tried in a headset

Nothing of this ran in SteamVR: no headset was reachable, and the person's own SteamVR and app were left alone. Unknown
until tried: whether SteamVR takes a side-by-side overlay's height from half the texture (assumed: square faces;
otherwise the faces would be half as high and the cube would gape), the shared texture's hand-over (SteamVR's
`SetOverlayTexture` with a D3D11 texture opened from a D3D12 resource), the panel's laser input, the pill and the bar
(its place under the menu, its laser input), feedback given from the panel against the live server, the
computer's keyboard reaching the panel, the predicted poses' timing, how the cube looks with VRChat behind it at 55 %,
and what the batches cost VRChat's own frames (the governor's back-off included). The world search ran against mocked
VRChat answers and a loopback Orbiters only: VRChat's real answers to the list requests (their order, `occupants`,
paging past 24), the invite to oneself to a public instance nobody is in yet (VRCX builds such locations the same way;
VRChat may refuse one it has not made), and `vrchat://launch` while VRChat runs (it may ask to switch, or start another
VRChat) need a signed-in VRChat account and a headset.
