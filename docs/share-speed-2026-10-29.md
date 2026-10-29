# Sharing to a friend: where the time went (2026-10-29)

Enzo: "When I share to a friend it takes forever. I don't know what's happening but that needs to be optimized."

## What his share did (measured)

His only share so far on the development server: the 3D capture *Silent Dawn 2026-10-28 23-52-12* (357,669 splats)
to one friend, from the desktop window, at 18:28 UTC. VRChat was closed (its log starts at 18:36 UTC). The times
below come from the dev database's rows (`files`, `MapPreviews`, `CaptureGrants`) and the session's files.

| When (UTC) | What | Took |
| --- | --- | ---: |
| before 18:28:21 | Share to … opened: VRChat's friends read (224 friends: a page of online friends, then the offline ones, 100 a request at VRChat's one request a second) | about 4 s (from the code and the friend count; not logged) |
| until 18:28:21.279 | Upload files made: the session was reconstructed on 10-28, before background preparation existed, so the share made them itself (cache miss) | not logged; the same session re-prepared tonight: 17.5 s (below) |
| 18:28:21.641 | 31.6 MB sent over the loopback, the server made the web `.splat` and the two sky encodings | 0.36 s |
| 18:28:21.6 - 27.9 | Six files stored in R2 **one after another**: `.splat` 11.4 MB 0.99 s, PC tree 15.1 MB 2.94 s, Quest tree 15.1 MB (the PC tree's very bytes: it fits Quest) 1.12 s, coarse 1.4 MB 0.38 s, sky WebP 0.32 s, sky JPEG 0.35 s | 6.1 s |
| 18:28:27.743 | Capture row committed | |
| 18:28:28.154 | Grant made: the app fetched the item it had just uploaded (GET), then asked for the share | 0.41 s |

About 28 s from opening Share to … to done, 24 s of it after Send. All that time the window showed one spinner line
("Sharing with your friends…"), the picker stayed open and greyed, the friend appeared only at the end, and the window
refused to close (a press outside or Escape did nothing while busy).

Over Cloudflare in production the 31.6 MB upload comes on top (not measured here: the dev app talks to
127.0.0.1:4100).

### Other repeated or serial work found on the way

- Every share of an item already uploaded fetched that item (`GET /captures/:type/:id`) before granting; a new upload
  fetched it again between the upload and the grant.
- One lock for all sharing: any share or unshare waited for every other one (a picture behind a 3D capture's upload).
- Friends were read only once Share to … was pressed.
- A world's pictures shared from Visited worlds go one upload and one grant per picture, one after the other; the
  server encoded and stored a request's pictures one by one too (each picture's two files in a row).
- After a share the app re-lists the computer's captures and refreshes Map explorer (`bump_library`, `refresh_maps`):
  in the background, after the dialog already has the answer, so not on the waiting path.

## The preparation, re-timed

A hard-linked copy of the session without its cache (`C:/Users/Enzo/orbiters-lab-share/silent`, removed afterwards),
this morning's release timing harness (`upload_prep_timings`, `ORBITERS_PREP_TIMING=1`), RTX 4080, VRChat closed,
other agents' builds running: a cache miss took **17.5 s** (GPU setup 3.9 s, PC tree tuning 12.1 s over 11 depths, the
tree also Quest's; read, build and cache write 0.6 s) and made the very 31,637,856 bytes Enzo's share sent; the cache
hit right after took **0.10 s** (0.36 s for the whole test process).

## R2 storage, one after another versus together

`r2bench.js` (scratch script, dev bucket, objects under `private/bench-share-speed/` deleted afterwards): the six sizes
of the Silent Dawn share, stored in a row and then all at once, three rounds from Enzo's line.

| Round | In a row (s) | Together (s) |
| ---: | ---: | ---: |
| 1 | 4.80 | 1.35 |
| 2 | 4.51 | 1.29 |
| 3 | 3.71 | 0.97 |

## What changed

Server (`dev`, b06a9c82): `uploadPreview` stores the web file, the three trees and the two skies at once
(`Promise.all`; rows created in the same order, so ids and the row's fields are as before); `uploadPictures` works on
three pictures at a time with its answer in the order sent, stores a picture sent twice in one request once, and starts
nothing more after one fails. Test: `backend/test/captureUploadSpeed.test.js` (six stores in flight together, three
pictures in flight, order, duplicates, failure).

Desktop (`controller/shares.rs`, `pages/sharing.rs`, IPC):

- **Optimistic window.** Send folds the picker away at once; the friends (and Everyone) appear in "Shared with" right
  away with a spinner beside them until the tray answers; a failure puts the saved item back and shows the error with
  Try again. The line under it says the step: "Preparing the 3D view for sharing…", "Sending 12.4 of 31.6 MB…" (the
  line and the picture's bottom edge fill up), "Sharing with Alex…"; "You can close this window: it carries on." The
  window closes during a share (the tray keeps the job; the card shows it sharing).
- **Stages in the job.** `ShareJob.stage` (`Starting`, `Waiting`, `Preparing`, `Sending { sent, total }`, `Sharing`,
  `Items { done, total }`) goes to every window, bytes at most every 100 ms.
- **Upload files made while friends are chosen.** Share to … on a reconstructed capture this account has not uploaded
  sends `ToService::PrepareShare`: the tray runs the same preparation the share runs (`maps::web_preview`), so Send
  finds its cache (0.04-0.15 s instead of 5-25 s on a miss). Nothing leaves the computer before Send.
- **Friends read when the window opens** on something the member can share, not when Share to … is pressed.
- **No repeated requests.** An item already uploaded (the gallery's item or its upload record) is shared with one
  request; a new upload goes straight to its grant with the identity the upload answered. Only a failed grant after
  a new upload fetches the item, so that the gallery still shows it uploaded.
- **One change at a time per item**, not for all items; the upload records file has its own small lock.
- **Several items at once.** A share of a world's pictures (Visited worlds) shares three at a time instead of one by
  one, says "Sharing with Alex · 4 of 12 done…" as they go, starts no more after a failure and answers with the
  first failure in their order.
- **`%LOCALAPPDATA%/Orbiters/logs/shares.log`**: one line per share with each step's time (waited, prepared, sent and
  stored, shared, in all), and one per preparation made ahead. The next "it takes forever" has its numbers.

## Before and after, per step (Silent Dawn to one friend)

| Step | Before | After |
| --- | ---: | ---: |
| Friends shown in Share to … | about 4 s after pressing it | already read while the window was open (same 4 s, earlier); instant within VRChat's 5-minute cache |
| Upload files | 17.5 s after Send (cache miss, re-timed) | started when Share to … opens: whatever is left of it when Send is pressed; 0.10 s once done |
| Upload over the loopback + server encodes | 0.36 s | same |
| Server storage | 6.1 s | 1.0-1.4 s (measured above) |
| Grant | 0.41 s (with the extra GET) | one request |
| Feedback | one spinner for ~18 s, friend shown at the end | friend shown at Send, then each step with its progress |

## Not done, and not verified

- Not measured end to end after the change: a throwaway backend cannot store 3D captures without R2 (preview files go
  to storage only) and pointing it at the dev bucket would overwrite real dev files by id. `shares.log` will have the
  next real share's steps.
- Only unit tests and screenshots: no real share through the window or through N4N0's menu after the change.
- N4N0's menu (avatar screen, SteamVR overlay) gets the server's faster storage and the shorter request chain, but
  does not prepare ahead when its Share to … opens: that would run the GPU preparation in VR before the member asked
  to share (the background preparation stays off beside VRChat without SteamVR for that reason).
- A capture whose PC tree fits Quest uploads and stores the same 15 MB twice (`vrchatQuest` = `vrchatPc`); skipping it
  needs a contract change (Docs/capture-sharing.md).
- A world's pictures from Visited worlds still upload one per request (now three at a time): a request of twelve
  would need a source key per picture (the multipart `sourceKey` is one per request: contract change), and the
  server's 30 picture uploads an hour per member caps such a share at 30 new pictures.
