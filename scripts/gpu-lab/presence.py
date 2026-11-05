"""The window's cost to a game on the graphics card, case by case, in blocks: one measured process runs throughout (by
default gpu-lab's GPU-bound load, or one processor thread) while the window opens in one case for a block, then nothing
for a block, then the next case, and so on. Each case is compared with the blocks without anything on either side of
it, so that what the computer does meanwhile, which drifts over minutes, weighs on both alike. Runs only while nobody
uses the computer (matrix.py's gate).

    python presence.py <out> [--block 20] [--reps 3] [--cases still,idle,scroll,splats] [--measure load|cpu-alu|cpu-mem]
                           [--session <name>] [--load "<adapter> <seconds> --work <n>"]

A case is a window scenario (orbiters-desktop --gpu-bench, on each GPU in turn) or `overlay-full` / `overlay-button`
(gpu-lab overlay on the graphics card: the SteamVR overlay's uploads, the whole picture or a button's part, 45 times a
second). Writes <out>/measured-<rep>.*, <out>/blocks.json and prints, per case, the median change of what is measured.
"""

import argparse
import json
import os
import re
import shlex
import statistics
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from matrix import GPUS, idle_seconds, wait_until_free  # noqa: E402
from run import APP, LAB, Proc  # noqa: E402

SCENARIOS = {
    "still": "still",
    "idle": "idle",
    "publish": "idle --publish 30",
    "publish-background": "idle --publish 30 --background",
    "scroll": "scroll",
    "splats": "splats",
    "splats-still": "splats-still",
    "scroll-full-screen": "scroll --size 2560x1440",
    "splats-full-screen": "splats --size 2560x1440",
    # The window on the graphics card, the 3D view's splats on the integrated GPU (settings::ViewGpu).
    "splats-view-integrated": "splats --view-gpu always",
    "splats-view-integrated-full-screen": "splats --view-gpu always --size 2560x1440",
}
# Cases whose window is on the graphics card only.
CARD_ONLY = {"splats-view-integrated", "splats-view-integrated-full-screen"}
OVERLAYS = {
    "overlay-full": "--rate 45",
    "overlay-button": "--rate 45 --rect 400,300,640,360",
}


def load_rate(csv, start, end):
    """The load's frames a second between `start` and `end` (its own clock)."""
    frames = []
    for line in open(csv, encoding="utf-8"):
        t, _, w = (float(x) for x in line.split(","))
        if start <= t <= end:
            frames.append(w)
    return len(frames) / (sum(frames) / 1e3) if frames else float("nan")


def cpu_rate(lines, start, end):
    """The processor test's rounds a second over the seconds that lie between `start` and `end` (its own clock)."""
    rates = []
    for _, line in lines:
        m = re.match(r"\s*([\d.]+) s\s+([\d.]+) rounds a second", line)
        if m and start <= float(m.group(1)) - 1 and float(m.group(1)) <= end:
            rates.append(float(m.group(2)))
    return statistics.mean(rates) if rates else float("nan")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--block", type=float, default=20)
    ap.add_argument("--reps", type=int, default=3)
    ap.add_argument("--cases", default="still,idle,scroll,splats")
    ap.add_argument("--measure", default="load", choices=["load", "cpu-alu", "cpu-mem"])
    ap.add_argument("--load", default=os.environ.get("GPU_LAB_LOAD", "nvidia 600 --work 1392"))
    ap.add_argument("--session", default="New Generation", help="the 3D view's session: part of its folder's name")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    cases = []
    for c in a.cases.split(","):
        cases += [(c, None)] if c in OVERLAYS else [(c, "graphics-card")] if c in CARD_ONLY else [(c, g) for g in GPUS]
    blocks = []
    rep = 0
    while rep < a.reps:
        wait_until_free()
        seconds = int((len(cases) * 2 + 1) * (a.block + 6) + 60)  # the whole rep and some
        csv = os.path.join(a.out, f"measured-{rep}.csv")
        if a.measure == "load":
            args = shlex.split(a.load)
            args[1] = str(seconds)
            measured = Proc([LAB, "load"] + args + ["--out", csv], os.path.join(a.out, f"measured-{rep}.txt"))
        else:
            measured = Proc([LAB, "cpu", a.measure.split("-")[1], str(seconds)], os.path.join(a.out, f"measured-{rep}.txt"))
        t_start = measured.wait_for("pid", 60)
        time.sleep(5)
        rep_blocks = []
        tainted = False
        for case in [None] + [x for c in cases for x in (c, None)]:
            if idle_seconds() < 10:
                tainted = True
                break
            start = time.time()
            other, measuring = None, start
            if case:
                name, gpu = case
                if gpu is None:
                    other = Proc([LAB, "overlay", "nvidia", str(int(a.block + 30))] + shlex.split(OVERLAYS[name]), os.path.join(a.out, f"{name}-{rep}.txt"))
                    measuring = other.wait_for("pid", 60)
                else:
                    session = ["--session", a.session] if name.startswith("splats") else []
                    other = Proc([APP, "--gpu-bench"] + shlex.split(SCENARIOS[name]) + session + ["--gpu", gpu, "--warmup", "0", "--seconds", "1000"],
                                 os.path.join(a.out, f"window-{rep}-{name}-{gpu}.txt"))
                    try:
                        measuring = other.wait_for("measuring", 120)
                    except RuntimeError:
                        measuring = start + a.block
            while time.time() < start + a.block:
                time.sleep(0.1)
            end = time.time()
            if other:
                other.p.terminate()
                other.finish(timeout=10)
            label = case and (case[0] if case[1] is None else f"{case[0]} {case[1]}")
            rep_blocks.append({"rep": rep, "case": label, "from": measuring + 2 - t_start, "to": end - 0.5 - t_start})
            # The other process ends before the next block.
            time.sleep(1.0)
        time.sleep(1.5)
        measured.p.terminate()
        measured.finish(timeout=10)
        if tainted:
            print(f"rep {rep}: someone came back; again later", flush=True)
            continue
        for b in rep_blocks:
            b["rate"] = load_rate(csv, b["from"], b["to"]) if a.measure == "load" else cpu_rate(measured.lines, b["from"], b["to"])
        blocks += rep_blocks
        print(f"rep {rep}: " + ", ".join(f"{b['case'] or 'none'} {b['rate']:.1f}" for b in rep_blocks), flush=True)
        rep += 1
    json.dump(blocks, open(os.path.join(a.out, "blocks.json"), "w"), indent=1)
    # Each case against the blocks without anything just before and after it.
    changes = {}
    for i, b in enumerate(blocks):
        if b["case"] and 0 < i < len(blocks) - 1 and blocks[i - 1]["case"] is None and blocks[i + 1]["case"] is None:
            base = (blocks[i - 1]["rate"] + blocks[i + 1]["rate"]) / 2
            changes.setdefault(b["case"], []).append((b["rate"] / base - 1) * 100)
    for case, v in changes.items():
        print(f"{case:<28} median {statistics.median(v):+6.2f} %  ({', '.join(f'{x:+.1f}' for x in v)})", flush=True)


if __name__ == "__main__":
    main()
