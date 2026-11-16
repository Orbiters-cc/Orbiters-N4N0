#!/usr/bin/env python3
"""Turns folders of N4N0 menu mismatch reports into a readable summary per kind (docs/menu-mismatch-reports.md).

    python tools/menu-reports/summarize.py <folder> [<folder> ...] [--out summary.md] [--lines 12]

A folder is a tester's `menu-reports` folder (Settings > Beta feedback > Open folder), a server export
(`node src/scripts/exportMenuReports.js <folder>` in the server repository) or a single report's folder. Each report is a
folder with `report.json` (and `picture.png` when the tester allowed one).

For each kind ("menu should be off", "wheel should be on", "quick menu should be on") it says what the menus took as open at
the press and by which rule, which rule is at fault, what the last inputs and decisions were, and what came after; then the
patterns across the reports of that kind, so that the next agent starts from evidence rather than from a guess.
"""

import argparse
import collections
import json
import os
import sys

KINDS = {
    "menu-should-be-off": "Menu should be off (ours showed, VRChat had no menu up)",
    "wheel-should-be-on": "Wheel should be on (VRChat's wheel was up, our ring was not)",
    "quick-menu-should-be-on": "Quick menu should be on (VRChat's Quick Menu was up, our parts beside it were not)",
}


def load(paths):
    reports = []
    for root in paths:
        candidates = [root] if os.path.isfile(os.path.join(root, "report.json")) else [os.path.join(root, d) for d in sorted(os.listdir(root))]
        for d in candidates:
            f = os.path.join(d, "report.json")
            if not os.path.isfile(f):
                continue
            try:
                with open(f, encoding="utf-8") as fh:
                    r = json.load(fh)
            except (OSError, ValueError) as e:
                print(f"skipped {f}: {e}", file=sys.stderr)
                continue
            r["_dir"] = d
            r["_picture"] = os.path.join(d, "picture.png") if r.get("picture") and os.path.isfile(os.path.join(d, "picture.png")) else None
            reports.append(r)
    reports.sort(key=lambda r: r.get("pressed_ms", 0))
    return reports


def state(r):
    return {k: v for k, v in r.get("state", [])}


def taken(r):
    """What the menus took as open at the press: VR's buttons, or the desktop's "ours follow"."""
    s = state(r)
    if "taken as open" in s:
        return s["taken as open"]
    return s.get("ours follow", "?")


def rule(r):
    """The rule that last decided, as the report says it."""
    s = state(r)
    if "why" in s:
        return s["why"]
    decides = [e for e in r.get("timeline", []) if e["t"] <= 0 and e["source"].startswith("decide")]
    return decides[-1]["text"] if decides else "(no decision in the 30 s before)"


def open_kind(text):
    t = (text or "").lower()
    if "wheel" in t:
        return "wheel"
    if "quickmenu" in t or "quick menu" in t:
        return "quick menu"
    if "camera" in t:
        return "camera"
    if "closed" in t or "none" in t:
        return "nothing"
    return t or "?"


def fault(r):
    """Which side of the rules is at fault, from the kind and what was taken as open."""
    k, o = r.get("kind"), open_kind(taken(r))
    if k == "menu-should-be-off" and state(r).get("a part of ours is up around VRChat's menus") == "false" and state(r).get("N4N0's own menu open") != "true":
        return "nothing of ours was up at the press: a part that went just before it, a part outside the menus (the 3D view, a floating panel), or a press made by mistake"
    if k == "menu-should-be-off":
        return f"ours took {o} as open and nothing closed it: the rule that opened or kept it ({rule(r)})" if o not in ("nothing",) else "ours took nothing as open, yet something of ours showed: a part left up (a page, a held view, a let-go delay)"
    want = "wheel" if k == "wheel-should-be-on" else "quick menu"
    if o == want:
        return f"ours took the {want} as open, yet the tester saw it missing: drawing or placement, not the rules"
    return f"no rule took the {want} as open (taken: {o}); the last rule was: {rule(r)}"


def lines(r, prefix, before_ms, n, after=False):
    out = []
    for e in r.get("timeline", []):
        if not e["source"].startswith(prefix):
            continue
        if after and e["t"] > 0:
            out.append(e)
        elif not after and -before_ms <= e["t"] <= 0:
            out.append(e)
    return out[-n:] if not after else out[:n]


def fmt(e):
    return f"`{e['t']:+6d} ms` {e['source']}: {e['text']}"


def summarize(reports, n):
    out = [f"# Menu mismatch reports: {len(reports)}", ""]
    by_kind = collections.defaultdict(list)
    for r in reports:
        by_kind[r.get("kind", "?")].append(r)
    builds = collections.Counter(r.get("build") for r in reports)
    surfaces = collections.Counter(r.get("surface") for r in reports)
    out.append("Builds: " + ", ".join(f"{b} ({c})" for b, c in builds.most_common()) + ". Surfaces: " + ", ".join(f"{s} ({c})" for s, c in surfaces.most_common()) + ".")
    out.append("")
    for kind in list(KINDS) + [k for k in by_kind if k not in KINDS]:
        rs = by_kind.get(kind, [])
        if not rs:
            continue
        out += [f"## {KINDS.get(kind, kind)}: {len(rs)}", ""]
        # Patterns first: what the next change should look at.
        out.append("### Patterns")
        out.append("")
        faults = collections.Counter(fault(r) for r in rs)
        out.append("Rule at fault (how many reports):")
        for f, c in faults.most_common():
            out.append(f"- {c} x {f}")
        lastin = collections.Counter((lines(r, "input", 30000, 1) or [{"source": "(no input in 30 s)", "text": ""}])[-1]["source"] for r in rs)
        out.append("")
        out.append("Last input before the press: " + "; ".join(f"{s} ({c})" for s, c in lastin.most_common()))
        gaps = []
        for r in rs:
            d = lines(r, "decide", 30000, 1)
            gaps.append(-d[-1]["t"] if d else None)
        known = sorted(g for g in gaps if g is not None)
        if known:
            out.append(f"Time from the last decision to the press: median {known[len(known) // 2]} ms, shortest {known[0]} ms, longest {known[-1]} ms ({len(gaps) - len(known)} with none in 30 s)")
        vr_words = collections.Counter()
        for r in rs:
            for e in r.get("vrchat_log", []):
                if -10000 <= e["t"] <= 4000:
                    vr_words[e["text"][:60]] += 1
        if vr_words:
            out.append("VRChat's log within 10 s before to 4 s after: " + "; ".join(f"{t} ({c})" for t, c in vr_words.most_common(6)))
        seen_tabs = sum(1 for r in rs if any("tabs found Some" in e["text"] and "taken as Closed" in e["text"] for e in r.get("timeline", []) if -30000 <= e["t"] <= 0))
        if seen_tabs:
            out.append(f"The headset-picture check saw VRChat's Quick Menu tabs while the rules took it as closed (no verdict, nothing done) in {seen_tabs} of {len(rs)} reports: QmCheck::found and its verdict are where to look")
        controllers = collections.Counter(", ".join(c or "?" for c in r.get("controllers", [])) or "desktop" for r in rs)
        out.append("Controllers: " + "; ".join(f"{c} ({k})" for c, k in controllers.most_common()))
        out.append("")
        out.append("### Each report")
        out.append("")
        for r in rs:
            s = state(r)
            out.append(f"#### {r.get('pressed_at')} {r.get('surface')} {r.get('build')} ({os.path.basename(r['_dir'])})")
            out.append("")
            out.append(f"- Taken as open: **{taken(r)}**; rule: {rule(r)}")
            out.append(f"- At fault: {fault(r)}")
            for key in ("ours", "seen", "watcher", "press under way (hand, ms held, began on its wheel)", "Quick Menu check", "a part of ours is up around VRChat's menus", "N4N0's own menu open", "VRChat's camera open (OSC)", "window client size"):
                if key in s:
                    out.append(f"- {key}: {s[key]}")
            if r.get("controllers"):
                out.append(f"- Controllers: {r['controllers']}; bindings: {r.get('bindings')}")
            if r.get("_picture"):
                out.append(f"- Picture: {r['_picture']}")
            out.append("")
            out.append("Last inputs:")
            for e in lines(r, "input", 30000, n):
                out.append(f"    {fmt(e)}")
            out.append("Decisions:")
            for e in lines(r, "decide", 30000, n):
                out.append(f"    {fmt(e)}")
            log = [e for e in r.get("timeline", []) if e["source"] not in ("press",) and not e["source"].startswith(("input", "decide")) and -8000 <= e["t"] <= 0]
            if log:
                out.append("Other lines in the last 8 s (looks, VRChat, vr-test.log):")
                for e in log[-n:]:
                    out.append(f"    {fmt(e)}")
            vr = [e for e in r.get("vrchat_log", [])]
            if vr:
                out.append("VRChat's log:")
                for e in vr[-n:]:
                    out.append(f"    {fmt(e)}")
            after = [e for e in r.get("timeline", []) if e["t"] > 0 and e["source"] != "press"]
            if after:
                out.append("After the press:")
                for e in after[:n]:
                    out.append(f"    {fmt(e)}")
            out.append("")
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("folders", nargs="+")
    ap.add_argument("--out")
    ap.add_argument("--lines", type=int, default=12)
    a = ap.parse_args()
    reports = load(a.folders)
    text = summarize(reports, a.lines) if reports else "No reports found."
    if a.out:
        with open(a.out, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
        print(f"{len(reports)} reports summarized in {a.out}")
    else:
        sys.stdout.reconfigure(encoding="utf-8")
        print(text)


if __name__ == "__main__":
    main()
