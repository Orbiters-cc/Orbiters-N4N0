# Unity projects and Capture's N4N0 card: UI pass U1 (29 October 2026)

What changed after Enzo's review of the first Unity projects page, and what other pages can reuse.

## Unity projects

- **Static header** (`pages::header`): caption, title and a short count line on the left; the search, refresh, package
  repos, add existing and New project on the same row. Narrower windows turn the secondary actions into round icon
  buttons, then give the toolbar its own row. The cards scroll under the header's blur; the scroll bar starts below it.
- **Two panes** (`orbiters_ui::layout::split`): from 980-point pages (the default 1280-point window) the selected
  project's panel sits beside the cards and is always shown (the most recently edited project until another is picked).
  The cards make room with a spring, each springing to its new place in the grid's own coordinates. The panel's left
  edge drags to resize it (the content keeps 520 points; double-click resets). Narrower pages get a sheet that slides
  over the cards (a press beside it, its × or Escape closes it).
- **Cards**: the project's avatars as a collage cover, kind badge, edit time, pills; hover lift, press on press, a
  green selection ring (the focus ring's language). Avatar pictures fade in when they load. Projects found on the
  computer are compact rows (six, then Show all).
- **Panel** (Settings' card language): hero card (cover, name, folder, Unity version, edit time, Open in Unity, folder,
  Unity Hub, a "…" menu with Copy the path and Remove from the list), Avatars (Add N4N0 / My Avatar only), Packages
  (tinted tiles, versions in dropdowns, pre-releases marked, remove, Restore, a pre-release switch), Add packages,
  Recent commits. Scrolled past the hero, a compact bar (name, Open in Unity) stays on top over a blur.
- **States**: shimmering skeletons while the disks are searched or packages read, an empty state with both ways in,
  a quiet "no match", error and warning banners (once: dialog, else panel, else above the cards), an activity capsule
  over the cards while something runs (a copy's files, progress and Cancel).
- **Dialogs** (`orbiters_ui::dialog`) grow out of the control that opened them, in its colour: Package repositories,
  New Unity project (template cards or a project to copy, validation, location, the copy's progress), and Capture's
  flow (avatar tiles, the project copy, then the setup's steps).

## Capture

The "Add to my avatar" button above the heading is gone. N4N0 is the second status card (after VRChat): Add to my
avatar, Setting up…, Confirm in Unity, or On your avatar; it opens the avatar picker (worn avatar first and chosen)
or the setup out of the card. The cards' widths follow their words (`layout::share`); three then two on narrow
windows.

## Reusable pieces

| Where | What |
| --- | --- |
| `orbiters_ui::layout` | `split` (docked or sliding panel), `columns`, `toolbar` fit, `share`; tested |
| `orbiters_ui::popup` | `dropdown`, `menu_button`, `any_open` (dialogs leave Escape to an open list) |
| `orbiters_ui::feedback` | `shimmer`, `spinner`, `loaded` + `cover` (pictures fading in), `banner`, `activity` |
| `orbiters_ui::dialog` | `Dialog` (morphs from an origin rect and colour, follows its content's height), `close` |
| `pages::header` | `Header::place` before the content, `Header::show` after `under_top` |

## Screenshots

```powershell
orbiters-desktop --screenshot <out> --sessions <empty folder> --only projects-gallery
orbiters-desktop --screenshot <out> --sessions <empty folder> --only projects-sheet-open --width 1040
orbiters-desktop --screenshot <out> --sessions <empty folder> --only projects-gallery --width 1920 --height 1080
```

Scenarios (`screenshot.rs`, staged by `projects::State::stage` and `stage_frame`): `projects-gallery`, `-detail`,
`-packages`, `-loading`, `-scanning`, `-empty`, `-panel-loading`, `-error`, `-busy`, `-search`, `-proposals`, `-repos`,
`-repos-loading`, `-new`, `-new-custom`, `-new-duplicate`, `-new-copying`, `-installing`, `-setup`, `-setup-closed`,
`-setup-done`, `-setup-declined`, `-setup-failed`, `-installed`, `-avatars`, `-avatars-loading`, `-avatars-empty`,
`-choice`, `-ready`, `-capture`, `-crossfade`, `-resize`, and states reached through the interface:
`stage_frame` presses, holds or hovers a control where the page last drew it (`-dropdown`, `-menu`, `-press`,
`-hover`, `-setup-from-panel`, `-new-open`, `-avatars-open`, `-sheet-open`). `-mid` scenarios capture six frames
after the press that starts a motion (`-new-mid`, `-repos-mid`, `-avatars-mid`, `-sheet-mid`, `-switch-mid`,
`-resize-mid`).
