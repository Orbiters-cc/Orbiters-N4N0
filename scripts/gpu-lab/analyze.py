"""Summarises a `gpu-lab sample --out` file: mean utilisation per process, adapter and engine type over its
collections (or the ones between --from and --to seconds), with process names from the processes running now.

    python analyze.py <sample.jsonl> [--from s] [--to s] [--top n] [--pid p ...]
"""

import argparse
import collections
import csv
import io
import json
import subprocess

ADAPTERS = {"0x00000000_0x00011772": "RTX 4080", "0x00000000_0x0001273c": "UHD 770", "0x00000000_0x000126cf": "Basic Render", "0x00000000_0x0bbb559b": "RTX 4080 (2)"}


def names():
    out = subprocess.run(["tasklist", "/FO", "CSV", "/NH"], capture_output=True, text=True, errors="replace").stdout
    return {int(r[1]): r[0] for r in csv.reader(io.StringIO(out)) if len(r) > 1 and r[1].isdigit()}


def load(path, start=None, end=None):
    rows = []
    for line in open(path, encoding="utf-8"):
        try:
            r = json.loads(line)
        except json.JSONDecodeError:
            continue  # the line being written
        if (start is None or r["t"] >= start) and (end is None or r["t"] <= end):
            rows.append(r)
    return rows


def summary(rows):
    agg = collections.defaultdict(float)
    for r in rows:
        for k, v in r["util"].items():
            pid, luid, eng = k.split("|")
            agg[(int(pid), luid.lower(), eng)] += v
    n = max(len(rows), 1)
    return {k: v / n for k, v in agg.items()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path")
    ap.add_argument("--from", dest="start", type=float)
    ap.add_argument("--to", dest="end", type=float)
    ap.add_argument("--top", type=int, default=25)
    ap.add_argument("--pid", type=int, nargs="*")
    a = ap.parse_args()
    rows = load(a.path, a.start, a.end)
    s = summary(rows)
    pn = names()
    print(f"{len(rows)} collections, {rows[0]['t'] if rows else 0:.0f}-{rows[-1]['t'] if rows else 0:.0f} s")
    items = sorted(s.items(), key=lambda kv: -kv[1])
    if a.pid:
        items = [kv for kv in items if kv[0][0] in a.pid]
    for (pid, luid, eng), v in items[: a.top]:
        print(f"{v:8.3f}  {pid:>6} {pn.get(pid, '?'):<30} {ADAPTERS.get(luid, luid):<14} {eng}")
    totals = collections.defaultdict(float)
    for (pid, luid, eng), v in s.items():
        totals[(ADAPTERS.get(luid, luid), eng)] += v
    print("totals:")
    for (ad, eng), v in sorted(totals.items()):
        if v >= 0.01:
            print(f"{v:8.3f}  {ad:<14} {eng}")


if __name__ == "__main__":
    main()
