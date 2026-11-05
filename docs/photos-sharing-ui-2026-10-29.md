# Photos & captures and sharing: UI pass (2026-10-29)

Enzo found the cards, the item modal and every sharing surface rough. This pass makes them look like the rest of the
app and the website's picture viewer (`server/frontend/src/components/captures/CaptureWindow.jsx`,
`home/CardPreviewDialog.jsx`, the website's AGENTS.md "UI and interaction standards").

## What changed

- **Picture window** (`pages/sharing.rs`, used by Photos & captures, Visited worlds and the requests window). It grows
  out of the card that opened it and closes back into it (shared geometry on `Spring::EXPAND`, the website's
  `expandSpring` 340/27/0.85): only the bounds move, the picture is always cropped to its frame, never stretched; the
  surface is opaque from the first frame; the backdrop fades with the window; the card stays hidden while the window is
  out and reappears under it as it lands. The window is sized to its picture (the website's rule: at most 960 points,
  the picture at most 65 % of the height); wide pictures sit over their details, tall ones beside them. Compact
  actions sit beside the title (Share publicly, Share to …; Ask to share and Download for items shared with you;
  Reconstruct to share for a capture without a 3D view), the visibility under it, who it is shared with as chips whose
  × takes that person back (Everyone on Orbiters for public), Unshare everything. Panels slide in under the actions and
  the window follows their height on the same spring: the friend picker (search, chips with pictures or initials that
  toggle with a bounce, the selection count, a message, Share with N friends; Ask Alex to share it with an Everyone on
  Orbiters too chip for items shared with you), the explicit warning (amber, the reasons as readable tags, Share
  anyway). Progress is a light running along the picture's bottom edge and a spinner line saying what runs; done and
  error are banners (error with Try again). On the picture: what it is, View in 3D, its folder, See it whole (the
  window grows to show the whole picture, details folded away). ←/→ and the round arrows go to the gallery's previous
  and next items: the new picture slides in over the old one, the window resizes to it.
- **Cards** (`pages/gallery.rs`): the Visited worlds / Capture files card (hover lift and zoom, press, staggered
  entrance), badges on the picture (3D / Picture, Reconstructing 42 %, Sharing…, Public / Shared · N), kind and time,
  then who sees it or, for items shared with you, the sender's picture and "from Alex". A reconstruction in progress
  draws its progress along the picture's bottom.
- **Groups**: the world, the instance kind as a pill, the time span and the count; **Instance** opens a popover out of
  its button (kind, number and region, who was there as chips, open/closed/checking with a status dot or spinner, Join
  in VRChat) instead of the plain modal.
- **Filters and requests**: the segmented filter (All, Mine, Shared with me), the search, and Requests with a green
  count badge on its corner. The **requests window** grows out of that button (`orbiters_ui::dialog`): Incoming · N and
  Sent, each request a card (who, about what, for whom as chips, the message quoted, when), Approve and Deny, the
  explicit acknowledgement inside the request instead of a second modal, answered states as pills (Approved, Denied,
  Cancelled, Waiting for …), a failed answer inline with Try again, an empty state.
- **Visited worlds**: Share splits into Share publicly and Share to … on a bouncier spring (the halves part as they
  reach their places, labels cross over, icons when they fit); both open the picture window out of the card's picture
  (or the session row's thumbnail).
- **Settings, VRChat account**: signed in: picture with a presence ring, name, how the session is kept, Sign out.
  Signed out: Username and Password rows (the password with an eye), Sign in to VRChat on the right. Two-factor: the
  methods as a segmented control, six cells for authenticator and email codes (digits pop in, the next cell ringed),
  a recovery code field, Verify and sign in. Waiting and errors as the window's spinner line and banners.
- **Capture files** (Manage captures): "← Photos & captures" where the heading's caption was (it overlapped it).
- **N4N0's menu** (screen version 12 unchanged): a chosen friend in Share to … carries a filled green check, History's and
  Share to …'s counts are quiet subtitles, the explicit page says "Because of NSFW/Toggle, sexually explicit"
  (`orbiters_core::explicit::reason_label`, also used by the desktop). Toolkit dad5017 (screen files revision 18).

## Shared pieces (for other pages)

- `orbiters_ui::viewer`: `Morph` (a window between an origin rect and its target, `frame()` and `part()`),
  `picture_layout()` / `picture_width()` / `contain()` (tested), `paint_backdrop()`, `paint_surface()`,
  `close_button()`.
- `orbiters_ui::widgets`: `Chip` (person or icon, chosen with a check, removable with an ×), `flow()` (wrapping rows,
  tested), `avatar()` and `name_color()`, `count_badge()`, `Variant::Warn`.
- `orbiters_ui::paint::circle_texture()`, `anim::Spring::EXPAND`, Phosphor icons (tray, lock, eye-slash, shield
  warning, corners in, arrow clockwise, …).
- `pages::sharing::working()` (a spinner line), `sharing::morph()` (the Share button that splits).

## Screenshots

`orbiters-desktop --screenshot <dir> --sessions <dir> --only sharing` renders every state, `--only vrchat-login` the
account card, `--only overlay` the overlay's share pages, `--n4n0-menu <dir>` the avatar's screen pages. Frames in the
middle of their motion: `gallery-open-mid` (a card growing into its window, five frames in), `gallery-close-mid`,
`gallery-next-mid` (the next item sliding in), `gallery-requests-opening` (the requests window out of its button),
`world-share-mid`; settled: `gallery-open`, `gallery-next`, `gallery-done`, `gallery-requests-empty`,
`world-share-window` (Share to … from a world card), `world-share-public`. Fixtures never show this computer's friends
and never start a real share.

## Not verified here

Pointer and keyboard flows were checked by staged states, not by hand; nothing was tried against a live server or in
VRChat. The menu pages were rendered with the avatar screen's shader in Unity from the bank files.
