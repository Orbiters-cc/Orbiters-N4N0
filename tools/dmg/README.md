# The disk image's window

What people see when they open `Orbiters-N4N0-<version>-macos-arm64.dmg`: the app's dark page with its glows, the
application on the left, Applications on the right, an arrow between them.

`cargo xtask dist` needs none of these tools: it copies `crates/orbiters-desktop/assets/dmg/template.dmg`, makes it
large enough, puts the application in and compresses it (`xtask/src/macos.rs`). The template holds the link to
Applications, the picture (`.background/background.tiff`) and Finder's layout (`.DS_Store`). It has to be the volume
itself and not files copied into a new one: Finder finds the picture through a bookmark that names the volume it was
made on (a layout copied into a fresh image keeps the icons' places and shows no picture; tried on macOS 27).

To change the look, from the repository's root:

```sh
# 1. The picture, at 1x and 2x, then both in one file.
swift tools/dmg/background.swift crates/orbiters-ui/assets/OrbitersSans-VF.ttf /tmp/n4n0-dmg
tiffutil -cathidpicheck /tmp/n4n0-dmg/background.png /tmp/n4n0-dmg/background@2x.png \
    -out crates/orbiters-desktop/assets/dmg/background.tiff

# 2. The template, with that picture and the layout.
python3 -m venv /tmp/n4n0-dmg/venv
/tmp/n4n0-dmg/venv/bin/pip install ds-store==1.3.1 mac-alias==2.2.2
/tmp/n4n0-dmg/venv/bin/python tools/dmg/layout.py crates/orbiters-desktop/assets/dmg/background.tiff
```

The icons' places are in both files (`app` and `applications` in `background.swift`, `PLACES` in `layout.py`): change
them together. Finder writes the names under the icons in black whatever the picture, hence the light pill under each;
a Finder that shows its path bar hides the window's last 28 points, hence nothing drawn there.
