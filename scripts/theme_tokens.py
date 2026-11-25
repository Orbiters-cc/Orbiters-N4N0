#!/usr/bin/env python3
"""Moves a checkout from the theme's old constants (`theme::FG_3`) to the look's tokens (`theme::fg_3()`).

The colours a person can choose (Settings, Appearance) are functions since the Appearance settings; a branch made before
them does not compile on top of them. To land one: run this on the branch first (`python scripts/theme_tokens.py`),
commit, then merge: both sides then changed the same lines the same way. Safe to run again, and on code already moved.

Left alone: the colours that mean something (OK, WARN, DANGER, REC) or tell kinds apart (CYAN, VIOLET…), and what is
drawn outside the app's look (N4N0's pictures on avatars), which uses the constants of `theme::base`.
"""
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parent.parent / "crates"
NAMES = "INK_950|INK_900|INK_850|INK_800|INK_750|INK_700|INK_600|FG|FG_2|FG_3|FG_4|ACCENT|ACCENT_HOVER|ON_ACCENT|MARK"
# N4N0's menu: its palettes are the contracts' themes, baked in pictures that avatars carry.
BASE = {"overlay/menu_ui.rs", "overlay/menu_pages.rs", "overlay/menu_retro.rs", "overlay/orbit/retro.rs"}

for path in sorted(ROOT.glob("*/**/*.rs")):
    rel = path.relative_to(ROOT).as_posix()
    if rel in ("orbiters-ui/src/theme.rs", "orbiters-ui/src/look.rs"):
        continue
    text = path.read_bytes().decode("utf-8")
    if any(rel.endswith(b) for b in BASE):
        new = re.sub(rf"\btheme::({NAMES})\b(?!\()", r"theme::base::\1", text)
    else:
        new = re.sub(rf"\btheme::({NAMES})\b(?!\()", lambda m: f"theme::{m.group(1).lower()}()", text)
    if new != text:
        path.write_bytes(new.encode("utf-8"))
        print(rel)
