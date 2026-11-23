#!/usr/bin/env python3
"""Per condition, from bench.py result files: the session (start) is the unit. For each start the median of its valid
windows; per condition the median over starts and the spread (lowest to highest start); windows not valid are counted,
never scored. usage: python3 -I tools/mac-bench/summary.py docs/mac-bench-results/<files>.json"""
import json, statistics, sys

rows = {}
for path in sys.argv[1:]:
    r = json.load(open(path))
    valid = [w for w in r.get("windows", []) if w.get("valid")]
    key = r.get("label") or "?"
    start = {"file": path, "outcome": r.get("outcome"), "valid": len(valid), "windows": len(r.get("windows", []))}
    for name, get in (("submitted_per_s", lambda w: w["guest_present"]["submitted_per_second_counted"]),
                      ("shown_per_s", lambda w: w["wsd_shown"]["per_second"]),
                      ("interval_ms", lambda w: w["guest_present"]["interval_ms_median"]),
                      ("fence_ms", lambda w: w["guest_present"]["fence_wait_ms_median"])):
        xs = [get(w) for w in valid if get(w) is not None]
        start[name] = statistics.median(xs) if xs else None
    rows.setdefault(key, []).append(start)
for key, starts in sorted(rows.items(), key=lambda kv: kv[0]):
    print(f"== {key}: {len(starts)} start(s), {sum(s['valid'] for s in starts)} valid window(s) of {sum(s['windows'] for s in starts)}")
    for name in ("submitted_per_s", "shown_per_s", "interval_ms", "fence_ms"):
        xs = [s[name] for s in starts if s[name] is not None]
        if xs:
            print(f"   {name}: median over starts {statistics.median(xs):.2f}, starts from {min(xs):.2f} to {max(xs):.2f}" + ("" if len(xs) > 1 else " (one sample)"))
    for s in starts:
        print("   ", s["file"], "-", s["outcome"])
