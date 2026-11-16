# Menu mismatch reports

N4N0 draws its own menus beside VRChat's Quick Menu and around VRChat's wheel (action menu), and has to guess when VRChat's are
open: VRChat does not say. The beta's **menu mismatch buttons** let a tester flag each moment the guess was wrong, and keep what
led to it as a report, so that an agent can improve the rules from real cases (Enzo, 2026-11-16).

## For testers

Settings › **Beta feedback** › **Menu mismatch buttons** (off by default). With it on, three buttons stay in view:

| Button | Means |
|---|---|
| Menu should be off | Something of N4N0 shows around VRChat's menus while VRChat has none of them up. Covers "wheel should be off" and "quick menu should be off": whichever of ours shows, it should not. |
| Wheel should be on | VRChat's wheel is up, N4N0's ring is not (nothing of ours, or our Quick Menu parts instead). |
| Quick menu should be on | VRChat's Quick Menu is up, N4N0's parts beside it are not (nothing, or the ring instead). |

- **VR**: a strip on the watch's wrist (Settings › the watch's hand, left by default), beside the watch, along the forearm. It is
  there whenever the switch is on, whatever N4N0's or VRChat's menus do. Press a row like the watch: the other hand's laser or
  fingertip on it, and a pull of its trigger. The row says "Noted" for a moment, with the usual click.
- **Desktop mode**: three rows in the bottom left corner of VRChat's window (VRChat's own hints are in its top corners), lifted
  over N4N0's strip under the Quick Menu in squarish windows. A plain click, or **Left Alt + F7 / F8 / F9**, which also works
  while VRChat's wheel holds the pointer at the window's centre.
- A press only reports: it never opens or closes anything.
- **Add a picture of the game** (a second switch, off by default) adds a small, blurred picture of what VRChat showed.
- Reports are kept in `%LOCALAPPDATA%\Orbiters\menu-reports` (Settings › Beta feedback › Open folder); **Send reports** sends the
  ones not sent yet to Orbiters; **Delete all** deletes them.

## What a report holds

A folder `<pressed, UTC>-<kind>-<6 hex>/` with `report.json` and, when allowed, `picture.png` (at most 400 pixels wide, each
pixel the average of a block of the original: what menus show is visible, names in the world and on nameplates are not).

`report.json` (format 1):

| Field | What |
|---|---|
| `format`, `id` | 1; 32 hex characters, random |
| `kind` | `menu-should-be-off`, `wheel-should-be-on`, `quick-menu-should-be-on` |
| `pressed_at`, `pressed_ms` | when, UTC (ISO, and ms since 1970) |
| `build`, `os`, `language`, `surface` | the app's version, `windows`/`macos`, the app's language, `vr` or `desktop` |
| `controllers` | VR: each hand's SteamVR `controller_type` (left, right) |
| `bindings` | VR: per controller type, the binding SteamVR uses for N4N0's actions (`saved: <url>` or N4N0's own) |
| `state` | the menus' state at the press, as `[name, value]` pairs (below) |
| `settings` | the settings that shape the menus: `orbit` (around wheel/Quick Menu, layout, sizes, smoothing), the watch, OSC, reduce motion |
| `timeline` | the ring's lines from 30 s before the press to 4 s after: `{t, source, text}`, `t` in ms from the press |
| `vrchat_log` | VRChat's log lines about menus, input, the camera, OSC and worlds in the same time (same shape) |
| `picture` | `picture.png` or null |

`state`, VR: `taken as open` (the buttons' `Open`: `Closed`, `QuickMenu(hand)`, `Wheel(hand)`, 0 left 1 right), `why` (the rule that
last changed it, in words, e.g. "menu button held past HOLD (500 ms) with no wheel open on that hand: the wheel"), `press under way`,
`Main Menu taken as up`, `last hand`, `ours` (what of ours is up), `Quick Menu check` (why a look at the headset's picture is under way),
`a part of ours is up around VRChat's menus`, `N4N0's own menu open`, `SteamVR's dashboard up`, `VRChat shows a scene`,
`VRChat's camera open (OSC)`.

`state`, desktop: `seen` (the window watcher's `Seen`: `Resting`, `Looking`, `Open(Wheel|QuickMenu)`), `watcher` (its counting:
candidate, hits, misses, closing, R first, opened on the rim alone, looking after a key), `ours follow`, `ours`,
`VRChat's camera panel seen`, `window client size`, `Left Alt held`.

`timeline` sources (each written only when its value changes, so the ring costs nothing while nothing happens):

| Source | Where | What |
|---|---|---|
| `input: menu buttons [left, right]` | VR | the menu button of each hand, down or up |
| `input: triggers [left, right]` | VR | each trigger |
| `input: left stick pushed (quarters, walks at 2)` | VR | the left stick's push, in quarters (walking closes VRChat's Quick Menu) |
| `input: key` | desktop | Esc or R pressed with VRChat in front |
| `input: Left Alt held` | desktop | Left Alt |
| `decide: taken as open, and why` | VR | `(Open, why)` at each change of what the buttons take as open |
| `decide: ours up around VRChat's menus` | VR | how many of our parts are up |
| `decide: seen` | desktop | the watcher's `Seen` at each change, with its counting |
| `decide: ours follow` | desktop | the menu our parts follow |
| `look: VRChat's window shows (menu, the wheel's rim coming)` | desktop | each change of what a look at VRChat's window found |
| `vr-test.log` | VR | every line of `vr-test.log` (the Quick Menu's look at the headset picture and its verdict, toggles sent to VRChat, lasers, sticks) |
| `vr: VRChat shows a scene, SteamVR's dashboard is up` | VR | |
| `vrchat: camera open (OSC)`, `vrchat: OSC receiving, in a station or seated`, `vrchat: instance changed` | both | what VRChat says |
| `window: VRChat in front, its size` | desktop | |
| `press` | both | the press itself (t = 0) |

**Never recorded**: what is typed (the desktop reads only Esc, R, Left Alt, F7 to F9 and the numbers 1 to 9, as before), what is said,
anyone's name (a VRChat log line with a user id, "OnPlayer", a display name, "player", "user", "avatar", an invite or a friend in it is
dropped; an instance id is cut to its world id), the account. Nothing is recorded while the switch is off, and turning it off empties
the ring. The server drops any line with a VRChat user id once more.

## Where reports go

- Kept on the tester's computer until deleted (`mismatch::folder`; a sandbox debug build keeps them under `ORBITERS_SANDBOX_LOGS`).
- **Send reports** posts each unsent one to `POST /n4n0/menu-reports` `{ report, picture }` (base64 PNG or null), no sign-in, as the
  usage statistics. 201 `{ id }` (200 when already kept); a `sent` file then marks the folder. A body over 900 KB loses its oldest lines,
  then its picture (the server's JSON limit is 1 MB).
- Server (`services/n4n0/menuReports.js`, table `N4N0MenuReports`, 400 days): Admin `GET /admin/n4n0/menu-reports`,
  `/admin/n4n0/menu-reports/:id`, `/admin/n4n0/menu-reports/:id/picture`, and `node src/scripts/exportMenuReports.js <folder>`, which
  writes them as the app keeps them.

## Reading them

```
python tools/menu-reports/summarize.py <folder> [<folder> ...] --out summary.md
```

Per kind: the rule at fault (from the kind and what was taken as open: "no rule took the wheel as open; the last rule was …", or
"ours took the Quick Menu as open and nothing closed it: …"), the last input before the press, the time from the last decision, what
VRChat's log said around it, the controllers; then each report's state, last inputs, decisions, other lines, VRChat's log and what came
after. Start there, then open the reports and pictures it points at.

Code: `crates/orbiters-desktop/src/mismatch/` (ring, report, send), `overlay/floating/strip.rs` (VR), `overlay/orbit/desktop.rs`
(`Corner`), `overlay/orbit/vr.rs` (`MenuButtons::why`), `pages/beta_feedback.rs` (the switch). A new kind is a variant of
`mismatch::Kind` with its words: both the strip and the corner show every kind of `Kind::ALL`.
