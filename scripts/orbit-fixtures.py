#!/usr/bin/env python3
"""Makes what the panels' detector is tested on, from whole frames of the real game (PNG files named by their first word: wheel,
qm or none, see crates/orbiters-desktop/src/overlay/orbit/detect.rs):

    python scripts/orbit-fixtures.py <folder of frames> crates/orbiters-desktop/src/overlay/orbit

- testdata/<kind>-<name>_<width>x<height>_<x0>_<y0>.png: the two parts of a frame a look reads, each at its place in the window.
- qm_icons.bin, with --icons: the seven icons' 24 x 24 grey pictures, averaged over the frames named in ICON_FRAMES where each is
  not the page selected.

Standard library only (Python's own zlib), as nothing else can be counted on here.
"""

import glob
import os
import struct
import sys
import zlib

ICON_FRAMES = {
    0: ["qm-settings", "qm-chat", "qm-audio"],
    1: ["qm-settings", "qm-chat", "qm-audio"],
    2: ["qm-settings", "qm-chat", "qm-audio", "qm-launch"],
    3: ["qm-settings", "qm-chat", "qm-audio", "qm-launch"],
    4: ["qm-settings", "qm-chat", "qm-launch"],
    5: ["qm-settings", "qm-audio", "qm-launch"],
    6: ["qm-chat", "qm-audio", "qm-launch"],
}
ICONS_X = [-132, -88, -44, 0, 44, 88, 132]
ICONS_Y = 224
HALF = 15
PATCH = 24
OUTSIDE = 1.07
SHIFT = 4


def load(path):
    data = open(path, "rb").read()
    i, idat = 8, b""
    while i < len(data):
        n = struct.unpack(">I", data[i : i + 4])[0]
        tag, chunk = data[i + 4 : i + 8], data[i + 8 : i + 8 + n]
        i += 12 + n
        if tag == b"IHDR":
            w, h = struct.unpack(">II", chunk[:8])
        elif tag == b"IDAT":
            idat += chunk
    raw = zlib.decompress(idat)
    stride = w * 3 + 1
    return w, h, [raw[y * stride + 1 : (y + 1) * stride] for y in range(h)]


def save(path, rows, w):
    def chunk(tag, body):
        return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)

    raw = b"".join(b"\0" + r for r in rows)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, len(rows), 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def quick_menu(w, h):
    return w / 2, 0.55 * h, (h / 720) * min(1.0, (w / h) / (16 / 9)) ** 0.28


def wheel(w, h):
    return w / 2 + 0.1556 * h, h / 2 + 0.1583 * h, 0.139 * h


def crop(rows, x0, y0, x1, y1, w, h):
    x0, y0, x1, y1 = max(int(x0), 0), max(int(y0), 0), min(int(x1) + 1, w), min(int(y1) + 1, h)
    return x0, y0, x1 - x0, [r[x0 * 3 : x1 * 3] for r in rows[y0:y1]]


def grey(rows, x, y, w, h):
    x, y = min(max(x, 0.0), w - 1.001), min(max(y, 0.0), h - 1.001)
    ix, iy = int(x), int(y)
    ax, ay = x - ix, y - iy
    v = lambda px, py: sum(rows[py][px * 3 : px * 3 + 3]) / 3
    return (v(ix, iy) * (1 - ax) + v(ix + 1, iy) * ax) * (1 - ay) + (v(ix, iy + 1) * (1 - ax) + v(ix + 1, iy + 1) * ax) * ay


def icons(folder):
    out = bytearray()
    for icon, names in ICON_FRAMES.items():
        total = [0.0] * (PATCH * PATCH)
        for name in names:
            w, h, rows = load(os.path.join(folder, name + ".png"))
            cx, cy, unit = quick_menu(w, h)
            ax, ay = cx + ICONS_X[icon] * unit, cy + ICONS_Y * unit
            for i in range(PATCH * PATCH):
                gx = ((i % PATCH + 0.5) / PATCH * 2 - 1) * HALF * unit
                gy = ((i // PATCH + 0.5) / PATCH * 2 - 1) * HALF * unit
                total[i] += grey(rows, ax + gx, ay + gy, w, h)
        out += bytes(min(255, int(v / len(names) + 0.5)) for v in total)
    return bytes(out)


def main():
    folder, target = sys.argv[1], sys.argv[2]
    data = os.path.join(target, "testdata")
    os.makedirs(data, exist_ok=True)
    if "--icons" in sys.argv:
        open(os.path.join(target, "qm_icons.bin"), "wb").write(icons(folder))
    for path in sorted(glob.glob(os.path.join(folder, "*.png"))):
        name = os.path.splitext(os.path.basename(path))[0]
        kind = name.split("-")[0]
        if kind not in ("wheel", "qm", "none"):
            continue
        w, h, rows = load(path)
        cx, cy, r = wheel(w, h)
        reach = r * OUTSIDE + 2
        x0, y0, cw, part = crop(rows, cx - reach, cy - reach, cx + reach, cy + reach, w, h)
        save(os.path.join(data, f"{name}_{w}x{h}_{x0}_{y0}.png"), part, cw)
        cx, cy, unit = quick_menu(w, h)
        reach = HALF + SHIFT + 1
        x0, y0, cw, part = crop(rows, cx + (ICONS_X[0] - reach) * unit, cy + (ICONS_Y - reach) * unit, cx + (ICONS_X[6] + reach) * unit, cy + (ICONS_Y + reach) * unit, w, h)
        save(os.path.join(data, f"{name}-row_{w}x{h}_{x0}_{y0}.png"), part, cw)


if __name__ == "__main__":
    main()
