"""Runs one condition of docs/igpu-experiment.md: optionally a GPU-bound load on the graphics card, optionally the
window's GPU bench (orbiters-desktop --gpu-bench) on one GPU, and Windows' GPU counters while it measures.

    python run.py <label> [--window "<bench args>"] [--load "<load args>"] [--extra "<gpu-lab args>"] [--seconds 30]

Writes <out>/<label>/{window,load,extra,sample}.txt, sample.jsonl, load.csv, nvidia-smi.csv and times.json; prints a
one-line summary. With `--alternate <s>` in the window's arguments (or the extra overlay's), it also compares the load
while the window moves and while it rests. GPU_LAB and ORBITERS_BENCH name gpu-lab.exe and orbiters-desktop.exe when
they were built elsewhere (the measurements used a CARGO_TARGET_DIR on C:).
"""

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
# The programs: gpu-lab built here (cargo build --release), the app built at the repository's root, unless the
# environment names others (a build elsewhere, with CARGO_TARGET_DIR).
LAB = os.environ.get("GPU_LAB", os.path.join(HERE, "target", "release", "gpu-lab.exe"))
APP = os.environ.get("ORBITERS_BENCH", os.path.join(HERE, "..", "..", "target", "release", "orbiters-desktop.exe"))


class Proc:
    """A child whose stdout lines are kept (and timed) as they come."""

    def __init__(self, cmd, path):
        self.lines = []
        self.out = open(path, "w", encoding="utf-8")
        self.p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace")
        self.t = threading.Thread(target=self._read, daemon=True)
        self.t.start()

    def _read(self):
        for line in self.p.stdout:
            line = line.rstrip("\n")
            self.lines.append((time.time(), line))
            self.out.write(line + "\n")
            self.out.flush()

    def wait_for(self, text, timeout):
        end = time.time() + timeout
        while time.time() < end:
            for t, line in self.lines:
                if text in line:
                    return t
            if self.p.poll() is not None and not self.t.is_alive():
                break
            time.sleep(0.05)
        raise RuntimeError(f"no '{text}' from {self.p.args}: {self.lines[-5:]}")

    def finish(self, timeout=60):
        try:
            self.p.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.p.kill()
        self.t.join(timeout=5)
        self.out.close()

    def last(self, prefix):
        for _, line in reversed(self.lines):
            if line.startswith(prefix):
                return line
        return ""


def load_stats(csv_path, start, end):
    """GPU ms of the load's frames whose time falls in [start, end] (seconds since the load started)."""
    gpu, wall = [], []
    with open(csv_path, encoding="utf-8") as f:
        for row in f:
            t, g, w = (float(x) for x in row.split(","))
            if start <= t <= end:
                gpu.append(g)
                wall.append(w)
    if not gpu:
        return "no load frames"
    gpu.sort()
    wall_sum = sum(wall)
    pick = lambda v, p: v[round((len(v) - 1) * p)]
    mean = sum(gpu) / len(gpu)
    return f"load {len(gpu) / (wall_sum / 1e3):.2f} frames/s, GPU ms mean {mean:.3f} p50 {pick(gpu, 0.5):.3f} p99 {pick(gpu, 0.99):.3f} max {gpu[-1]:.3f}"


def load_phases(csv_path, offset, period, start, end):
    """With a window that moves for `period` seconds, then rests as long (`--alternate`): the load's frames a second
    while it moves and while it rests, the first fifth of each turn (at most half a second) left out. `offset`: the
    load's time when the window started measuring."""
    skip = min(0.5, period / 5)
    frames = {True: [0, 0.0], False: [0, 0.0]}
    turns = {}  # turn number: [frames, ms]
    with open(csv_path, encoding="utf-8") as f:
        for row in f:
            t, _, w = (float(x) for x in row.split(","))
            tw = t - offset
            if not (start <= t <= end) or tw < 0 or tw % period < skip:
                continue
            turn = int(tw // period)
            moving = turn % 2 == 0
            frames[moving][0] += 1
            frames[moving][1] += w
            turns.setdefault(turn, [0, 0.0])
            turns[turn][0] += 1
            turns[turn][1] += w
    rate = {k: (n / (ms / 1e3) if ms else float("nan")) for k, (n, ms) in frames.items()}
    change = (rate[True] / rate[False] - 1) * 100 if rate[False] else float("nan")
    # Each turn of moving against the rests around it: what else the computer does changes slowly next to that, and
    # the median leaves out the turns a stall of something else fell in.
    r = {k: n / (ms / 1e3) for k, (n, ms) in turns.items() if ms}
    ratios = sorted((r[k] / ((r[k - 1] + r[k + 1]) / 2) - 1) * 100 for k in r if k % 2 == 0 and k - 1 in r and k + 1 in r)
    median = ratios[len(ratios) // 2] if ratios else float("nan")
    quart = f"{ratios[len(ratios) // 4]:+.1f}..{ratios[3 * len(ratios) // 4]:+.1f}" if ratios else ""
    return f"load moving {rate[True]:.2f} / resting {rate[False]:.2f} frames/s ({change:+.2f} %; turns: median {median:+.2f} %, quartiles {quart}, {len(ratios)} turns)"


def cpu_phases(lines, offset, period):
    """A processor test's rounds a second beside a window that moves for `period` seconds, then rests as long: the
    mean while it moves and while it rests, seconds that straddle a change left out. `offset`: the window's time when
    the test started."""
    rates = {True: [], False: []}
    for _, line in lines:
        m = re.match(r"\s*([\d.]+) s\s+([\d.]+) rounds a second", line)
        if not m:
            continue
        t, rate = float(m.group(1)), float(m.group(2))
        a, b = t - 1 + offset, t + offset
        if a < 0 or a // period != b // period:
            continue
        rates[int(a // period) % 2 == 0].append(rate)
    mean = {k: (sum(v) / len(v) if v else float("nan")) for k, v in rates.items()}
    change = (mean[True] / mean[False] - 1) * 100 if mean[False] else float("nan")
    return f"cpu moving {mean[True]:.2f} / resting {mean[False]:.2f} rounds a second ({change:+.2f} %, {len(rates[True])}+{len(rates[False])} s)"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("label")
    ap.add_argument("--window")
    ap.add_argument("--load")
    ap.add_argument("--extra")
    ap.add_argument("--seconds", type=float, default=30)
    ap.add_argument("--warmup", type=float, default=4)
    ap.add_argument("--out", default=os.environ.get("GPU_LAB_OUT", "results"))
    a = ap.parse_args()
    out = os.path.join(a.out, a.label)
    os.makedirs(out, exist_ok=True)
    started = {}
    load = extra = window = None
    if a.load:
        load = Proc([LAB, "load"] + shlex.split(a.load) + ["--out", os.path.join(out, "load.csv")], os.path.join(out, "load.txt"))
        started["load"] = load.wait_for("pid", 60)
    if a.window:
        # It measures longer than the counters run, so that they never see it close.
        window = Proc([APP, "--gpu-bench"] + shlex.split(a.window) + ["--seconds", str(a.seconds + 10), "--warmup", str(a.warmup)], os.path.join(out, "window.txt"))
        started["window"] = window.wait_for("measuring", 240)
    else:
        time.sleep(a.warmup)
    # After the window measures: what it runs along (a processor test) sees only the window measuring.
    if a.extra:
        extra = Proc([LAB] + shlex.split(a.extra), os.path.join(out, "extra.txt"))
        started["extra"] = extra.wait_for("pid", 120)
    t0 = time.time()
    # The graphics card's clock, power state and power four times a second (NVIDIA's tool, when there).
    try:
        smi = subprocess.Popen(
            ["nvidia-smi", "--query-gpu=timestamp,clocks.gr,clocks.mem,pstate,power.draw,utilization.gpu", "--format=csv,noheader", "-lms", "250"],
            stdout=open(os.path.join(out, "nvidia-smi.csv"), "w"),
            stderr=subprocess.DEVNULL,
        )
    except OSError:
        smi = None
    sample = Proc([LAB, "sample", str(a.seconds - 1), "--every", "500", "--out", os.path.join(out, "sample.jsonl")], os.path.join(out, "sample.txt"))
    sample.finish(timeout=a.seconds + 60)
    if smi:
        smi.terminate()
    t1 = time.time()
    # When each part started and the counters ran (seconds of the clock), for analyses after the run.
    with open(os.path.join(out, "times.json"), "w", encoding="utf-8") as f:
        f.write(json.dumps({**started, "counters": [t0, t1]}))
    if window:
        window.finish(timeout=60)
    summary = [a.label]
    if window:
        summary.append(window.last("adapter").replace("adapter ", "on "))
        summary.append(window.last("summary:"))
    if load:
        # Its frames reach the file once a second.
        time.sleep(1.5)
        load.p.terminate()
        load.finish(timeout=10)
        summary.append(load_stats(os.path.join(out, "load.csv"), t0 - started["load"] + 0.5, t1 - started["load"] - 0.5))
        if a.window and "--alternate" in a.window:
            period = float(a.window.split("--alternate")[1].split()[0])
            offset = started["window"] - started["load"]
            summary.append(load_phases(os.path.join(out, "load.csv"), offset, period, t0 - started["load"] + 0.5, t1 - started["load"] - 0.5))
    if extra:
        extra.finish(timeout=a.seconds + 60)
        summary.append(extra.last("summary:"))
        if a.window and "--alternate" in a.window and a.extra.startswith("cpu"):
            period = float(a.window.split("--alternate")[1].split()[0])
            summary.append(cpu_phases(extra.lines, started["extra"] - started["window"], period))
        if load and "--alternate" in a.extra:
            period = float(a.extra.split("--alternate")[1].split()[0])
            summary.append(load_phases(os.path.join(out, "load.csv"), started["extra"] - started["load"], period, t0 - started["load"] + 0.5, t1 - started["load"] - 0.5))
    print(" | ".join(s for s in summary if s))


if __name__ == "__main__":
    sys.exit(main())
