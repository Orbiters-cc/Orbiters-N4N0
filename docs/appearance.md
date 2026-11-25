# Appearance (Settings, Appearance)

How the app looks: colours, font, size and background. Kept with the other settings, per computer
(`orbiters_core::appearance::AppearanceSettings`, read tolerantly; `version` is raised only when a field changes meaning).

## Colours

- A look is two choices: the surfaces' tint and the accent. `orbiters_ui::look` derives every other colour by rule (OKLCH):
  the ink and text ramps keep Orbiters' own lightness and take the tint's hue; `fg`, `fg_2`, `fg_3` and the accent are at
  WCAG AA on every surface up to `ink_750`, and what is written on the accent is at AA on it and on its hover. An accent too
  dark is lightened just enough, its hue kept; the page says so.
- Orbiters' own look is not derived: it is the constants of `theme::base`, exactly.
- Draw with the tokens (`theme::fg_3()`, `theme::accent()`), never with a literal. What means something is a constant and is
  not themable: `theme::OK`, `WARN`, `DANGER`, `REC` (a success, "online", a saved thing is `OK`, not the accent), and the colours
  that tell kinds apart (`CYAN`, `VIOLET`, `BLUE`, `INDIGO`, `PURPLE`).
- N4N0's menu pages and pictures on avatars keep the contracts' themes: they use `theme::base`.
- `scripts/theme_tokens.py` moves a branch made before the tokens (`theme::FG_3`); run it on the branch, commit, then merge.

In the headset and beside VRChat's menus: the accent, the text steps and the font apply. Those panels keep their own ink
(they lie on VRChat's menus) and have no background. Their accent follows the app's until one is picked for them
(`OrbitSettings::accent_own`; one picked before this existed counts).

## Font

`orbiters_ui::faces`: the default (Segoe UI, Orbiters Sans), six bundled variable fonts (Latin), or a family installed on the
computer (found by a thread, from the fonts' own tables). Pages are laid out for Segoe UI, so a chosen font is fitted to it:
scaled to its lower case's height and its words' width, with its line heights, in the copy drawn with. A letter it does not
have comes from the default font, then the system's for its script, glyph by glyph. *Size* is egui's zoom (90, 100, 115 %).

## Background

`orbiters_ui::background`: orbiters.cc's profile backgrounds (`frontend/src/components/personalization`) in WGSL with the
website's parameters and colours, laid behind the pages as the website lays them (its opacity and its wash), then words a step lighter where needed and, only where that is not enough, a veil:
as little black as brings the second text step to AA over the brightest the scene can be. One rule, whatever the colours.

The contract on cost (`crate::appearance::Motion`, `Wallpaper`; tests beside them):

- Nothing is compiled until a background is chosen, and then on a thread of its own.
- A scene is drawn into its own texture, only when its time moves on: Aero Shards 30 times a second at most, the others 20.
  Soft scenes are drawn within 0.4 Mpx and scaled up, the others within 2.1 Mpx.
- It moves only while the window is in front and was used in the last half minute, and while neither VRChat nor SteamVR runs
  (unless *Move while playing* is on). Reduce motion, the app's or Windows', shows its still frame. At rest it asks for no frame.
- Under a background the glow is not drawn and its drift asks for no frames.
- egui draws the whole window for a frame: while a scene moves, the window is drawn at the scene's rate. That is why the
  rates are low, share one clock with the other decorations, and stop so soon.

Measure with `orbiters-desktop --gpu-bench backgrounds [--gpu integrated|graphics-card]` (each scene's passes, timestamps), and
`--gpu-bench live --backdrop <id>` for the window. `--gpu-bench backgrounds --pictures <folder>` saves each scene alone and as it
lies behind a page. Screenshots: `--only appearance`, and `--appearance <palette>[:<font>[:<background>]]` on any scene.
