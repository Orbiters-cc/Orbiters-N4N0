#!/usr/bin/env python3
"""Makes the disk image's template: crates/orbiters-desktop/assets/dmg/template.dmg.

The template is a small disk image named as the release's, holding the link to Applications, the window's picture
(`.background/background.tiff`) and the Finder's layout (`.DS_Store`: the window, the two icons' places, the picture).
`cargo xtask dist` on a Mac copies it, makes it large enough, puts the application in and compresses it; it never runs
this. Finder finds the picture through a bookmark that names the very volume it was made on, which is why the layout
cannot be a file copied into a new image: the release's image has to BE this volume. Run it again only when the window,
the icons' places or the picture change (tools/dmg/README.md):

    python3 -m venv <folder> && <folder>/bin/pip install ds-store==1.3.1 mac-alias==2.2.2
    <folder>/bin/python tools/dmg/layout.py crates/orbiters-desktop/assets/dmg/background.tiff
"""
import pathlib
import shutil
import subprocess
import sys
import tempfile

from ds_store import DSStore
from mac_alias import Alias, Bookmark

VOLUME = "Orbiters - N4N0"
APP = "Orbiters - N4N0.app"
# The window's inside, in points: the picture's size (tools/dmg/background.swift, which draws under these places).
WIDTH, HEIGHT = 660, 420
# The window's own title bar, which its bounds count.
TITLE = 32
PLACES = {APP: (180, 200), "Applications": (480, 200)}
OUT = pathlib.Path(__file__).resolve().parents[2] / "crates/orbiters-desktop/assets/dmg/template.dmg"


def main(picture: pathlib.Path) -> None:
    with tempfile.TemporaryDirectory() as work:
        work = pathlib.Path(work)
        image, mount = work / "layout.dmg", work / "mount"
        mount.mkdir()
        subprocess.run(["hdiutil", "create", "-quiet", "-size", "16m", "-fs", "HFS+", "-volname", VOLUME, str(image)], check=True)
        subprocess.run(["hdiutil", "attach", "-quiet", "-nobrowse", "-mountpoint", str(mount), str(image)], check=True)
        try:
            (mount / "Applications").symlink_to("/Applications")
            (mount / ".background").mkdir()
            on_volume = mount / ".background" / "background.tiff"
            shutil.copy(picture, on_volume)
            with DSStore.open(str(mount / ".DS_Store"), "w+") as store:
                root = store["."]
                root["vSrn"] = ("long", 1)
                root["bwsp"] = {
                    "WindowBounds": "{{200, 160}, {%d, %d}}" % (WIDTH, HEIGHT + TITLE),
                    "ShowToolbar": False,
                    "ShowSidebar": False,
                    "ContainerShowSidebar": False,
                    "SidebarWidth": 0,
                    "ShowPathbar": False,
                    "ShowStatusBar": False,
                    "ShowTabView": False,
                    "PreviewPaneVisibility": False,
                }
                root["icvp"] = {
                    "viewOptionsVersion": 1,
                    "backgroundType": 2,
                    "backgroundImageAlias": Alias.for_file(str(on_volume)).to_bytes(),
                    "backgroundColorRed": 0.035,
                    "backgroundColorGreen": 0.035,
                    "backgroundColorBlue": 0.043,
                    "iconSize": 128.0,
                    "textSize": 13.0,
                    "labelOnBottom": True,
                    "showItemInfo": False,
                    "showIconPreview": True,
                    "arrangeBy": "none",
                    "gridOffsetX": 0.0,
                    "gridOffsetY": 0.0,
                    "gridSpacing": 100.0,
                    "scrollPositionX": 0.0,
                    "scrollPositionY": 0.0,
                }
                root["pBBk"] = Bookmark.for_file(str(on_volume))
                root["icvl"] = ("type", b"icnv")
                root["vstl"] = ("type", b"icnv")
                for name, place in PLACES.items():
                    store[name]["Iloc"] = place
            for leftover in (".fseventsd", ".Trashes"):
                shutil.rmtree(mount / leftover, ignore_errors=True)
        finally:
            subprocess.run(["hdiutil", "detach", "-quiet", "-force", str(mount)], check=True)
        subprocess.run(["hdiutil", "convert", "-quiet", str(image), "-format", "UDZO", "-ov", "-o", str(OUT)], check=True)
    print(OUT)


if __name__ == "__main__":
    main(pathlib.Path(sys.argv[1]).resolve())
