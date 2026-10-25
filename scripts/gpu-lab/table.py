"""One line per run of a matrix.py results folder: the window's frames a second, the GPU engine time of its process
on each adapter, DWM's and the System process's on the graphics card, and the load's frames a second.

    python table.py <results folder> [label prefix ...]
"""

import json
import os
import re
import sys

NVIDIA = "0x00000000_0x00011772"
INTEL = "0x00000000_0x0001273c"


def counters(path):
    """Mean utilisation per (pid, luid, engine) over a run's collections."""
    rows = []
    for line in open(path, encoding="utf-8"):
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    acc = {}
    for r in rows:
        for k, v in r["util"].items():
            pid, luid, eng = k.split("|")
            key = (int(pid), luid.lower(), eng)
            acc[key] = acc.get(key, 0.0) + v
    return {k: v / max(len(rows), 1) for k, v in acc.items()}


def engine(c, pid, luid, engines=None):
    return sum(v for (p, l, e), v in c.items() if p == pid and l == luid and (engines is None or e in engines))


def main():
    root = sys.argv[1]
    prefixes = sys.argv[2:]
    print(f"{'run':<44} {'fps':>6} {'app@4080':>9} {'app@iGPU':>9} {'dwm 3D':>7} {'sys copy':>8} {'others':>7} {'decode':>6} {'load fps':>9}")
    for label in sorted(os.listdir(root)):
        if prefixes and not any(label.startswith(p) for p in prefixes):
            continue
        d = os.path.join(root, label)
        if not os.path.exists(os.path.join(d, "sample.jsonl")):
            continue
        c = counters(os.path.join(d, "sample.jsonl"))
        pid, fps, load = None, "", ""
        w = os.path.join(d, "window.txt")
        if os.path.exists(w):
            text = open(w, encoding="utf-8").read()
            m = re.search(r"pid (\d+)", text)
            pid = int(m.group(1)) if m else None
            m = re.search(r"\(([\d.]+) a second\)", text)
            fps = m.group(1) if m else "?"
        lt = os.path.join(d, "load.txt")
        summary_line = ""
        for log in [p for p in os.listdir(os.path.join(root, "..")) if p.startswith("matrix") and p.endswith(".log")]:
            for line in open(os.path.join(root, "..", log), encoding="utf-8"):
                if line.startswith(label + " "):
                    summary_line = line
        m = re.search(r"load ([\d.]+) frames/s", summary_line)
        load = m.group(1) if m else ""
        mine = {pid}
        for name in ("load.txt", "extra.txt"):
            path = os.path.join(d, name)
            if os.path.exists(path):
                m = re.search(r"pid (\d+)", open(path, encoding="utf-8").read())
                if m:
                    mine.add(int(m.group(1)))
        app4080 = engine(c, pid, NVIDIA) if pid else 0.0
        appi = engine(c, pid, INTEL) if pid else 0.0
        dwm = engine(c, 2080, NVIDIA, {"3D"})
        sysc = engine(c, 4, NVIDIA, {"Copy"})
        # Everything else on the graphics card's 3D engines (the background a run is measured against), and video
        # decoding (a video playing changes it).
        others = sum(v for (p, l, e), v in c.items() if l == NVIDIA and e in ("3D", "Graphics_1") and p not in mine and p != 2080)
        decode = sum(v for (p, l, e), v in c.items() if e == "VideoDecode")
        print(f"{label:<44} {fps:>6} {app4080:9.2f} {appi:9.2f} {dwm:7.2f} {sysc:8.2f} {others:7.1f} {decode:6.1f} {load:>9}")


if __name__ == "__main__":
    main()
