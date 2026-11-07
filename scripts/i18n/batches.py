#!/usr/bin/env python3
"""The translation batches and what the website already translated.

    i18n_scan list > list.tsv     (cargo run -p orbiters-i18n --example i18n_scan -- list)
    python scripts/i18n/batches.py list.tsv <website frontend/src/i18n/locales>

Writes reports/i18n/BATCHES.md and reports/i18n/batches.json (each file in exactly one batch, with its count) and
crates/orbiters-i18n/catalog/website/ (every English text of the app the website already translates, word for word, so batches
get those for nothing, and the same way). Run it again only before the batches start: it is a plan, not a build step.
"""
import collections
import glob
import json
import os
import re
import sys

# Batch, what it is, then path prefixes in order: a file goes to the first batch with a prefix of it.
BATCHES = [
    ("orbit", "The menus around VRChat's: wheel, wing, strip, All tools and their sheets", ["crates/orbiters-desktop/src/overlay/orbit/"]),
    ("overlay-worlds", "Worlds in 3D: its side panel, pages and hints", ["crates/orbiters-desktop/src/overlay/worlds/"]),
    ("overlay-rest", "The other overlays: VR panel, N4N0's menu pages, its keyboard", ["crates/orbiters-desktop/src/overlay/"]),
    ("n4n0", "N4N0 itself: what its menu writes live, the assistant, its messages", ["crates/orbiters-n4n0-core/", "crates/orbiters-n4n0/"]),
    ("pages-settings", "App window: Settings and what it opens", ["crates/orbiters-desktop/src/pages/settings", "crates/orbiters-desktop/src/pages/experimental", "crates/orbiters-desktop/src/pages/network", "crates/orbiters-desktop/src/pages/vr_audio", "crates/orbiters-desktop/src/pages/storage", "crates/orbiters-desktop/src/pages/form", "crates/orbiters-desktop/src/pages/mic", "crates/orbiters-desktop/src/pages/header", "crates/orbiters-desktop/src/pages/mod.rs", "crates/orbiters-desktop/src/pages/n4n0", "crates/orbiters-desktop/src/pages/requests"]),
    ("pages-capture", "App window: Home, Capture, Sessions, reconstruction", ["crates/orbiters-desktop/src/pages/home", "crates/orbiters-desktop/src/pages/capture", "crates/orbiters-desktop/src/pages/sessions", "crates/orbiters-desktop/src/pages/reconstruct", "crates/orbiters-desktop/src/pages/splat"]),
    ("pages-gallery", "App window: Gallery, Sharing, Community, Events", ["crates/orbiters-desktop/src/pages/gallery", "crates/orbiters-desktop/src/pages/sharing", "crates/orbiters-desktop/src/pages/community", "crates/orbiters-desktop/src/pages/events"]),
    ("pages-editor", "App window: the photo and 3D editor", ["crates/orbiters-desktop/src/pages/editor"]),
    ("pages-projects", "App window: Projects", ["crates/orbiters-desktop/src/pages/projects", "crates/orbiters-projects/"]),
    ("pages-worlds", "App window: Worlds, Videos, Saved looks and the rest of the pages", ["crates/orbiters-desktop/src/pages/"]),
    ("controller", "What the service reports to the windows: status, toasts, the tray", ["crates/orbiters-desktop/src/controller", "crates/orbiters-desktop/src/"]),
    ("foundation-crates", "Setup, shared widgets and the lower crates", ["crates/"]),
]


# Word for word is not always the same sense: the website's "Record" is a file (Dossier), its "Yours" a custom option, "Sent" and "Approved" agree with a
# feminine noun in French. These are for each batch to translate, knowing what they name.
NOT_THE_SAME = {
    "Record", "Yours", "Shared", "People", "Look", "Sharp", "Top", "Base", "Light", "Test", "Comment", "Where", "Desktop", "Manual", "Automatic", "Mine", "Sent", "Approved", "Cancelled",
    "Not available", "Shared publicly", "Accepted", "Connected", "Denied", "Hidden", "Paused", "Stopped", "Ended", "Running", "Available", "Failed", "Waiting", "All", "None", "Everything", "Live", "More", "Program",
    "Object", "Source", "Material", "Product", "Address", "Location", "Host", "Hosted by", "Labels", "Requests", "Send request", "Never", "Highlights", "Visits", "Watch", "Default", "Clear", "Change", "Text", "Shape",
    "Size", "Length", "Duration", "Group", "Public", "Link", "Someone", "You", "Today", "Next", "Previous", "Finish", "Done", "Hue", "Glow", "Connect", "Join", "Like", "Show", "Hide", "Filter", "Select",
    "Language", "Settings", "Pictures", "Picture", "Color", "On Orbiters", "From Orbiters", "Where we meet", "Taken at", "Time to be announced", "Starts when the organizer is ready", "Will invite you", "Open the account",
    "Film & media", "Movies", "Roleplay", "Voting open", "Happening now",
}


def leaves(value, prefix=""):
    if isinstance(value, dict):
        for k, v in value.items():
            yield from leaves(v, f"{prefix}.{k}" if prefix else k)
    elif isinstance(value, str):
        yield prefix, value


def website(root):
    """English text -> {tag: translation}, word for word. {{name}} is written {name} here. The shared namespace wins, then the commonest."""
    tags = ["ja", "fr", "es", "pt-BR"]
    votes = collections.defaultdict(lambda: {t: collections.Counter() for t in tags})
    for path in glob.glob(os.path.join(root, "en", "*.json")):
        ns = os.path.basename(path)
        en = dict(leaves(json.load(open(path, encoding="utf8"))))
        others = {}
        for t in tags:
            other = os.path.join(root, t, ns)
            others[t] = dict(leaves(json.load(open(other, encoding="utf8")))) if os.path.exists(other) else {}
        weight = 1000 if ns == "common.json" else 1
        for key, text in en.items():
            if "<" in text or "$t(" in text or re.search(r"_(one|other|many|zero)$", key):
                continue
            plain = re.sub(r"\{\{\s*(\w+)[^}]*\}\}", r"{\1}", text)
            for t in tags:
                translated = others[t].get(key)
                if translated:
                    votes[plain][t][re.sub(r"\{\{\s*(\w+)[^}]*\}\}", r"{\1}", translated)] += weight
    return {en: {t: c.most_common(1)[0][0] for t, c in v.items() if c} for en, v in votes.items() if all(v[t] for t in tags)}


def main():
    listed, locales = sys.argv[1], sys.argv[2]
    rows = []
    with open(listed, encoding="utf8") as f:
        for line in f:
            file, number, error, text = line.rstrip("\n").split("\t", 3)
            rows.append((file, int(number), error == "1", json.loads(text)))
    plan = {name: {"about": about, "files": collections.defaultdict(lambda: [0, 0, 0])} for name, about, _ in BATCHES}
    used = collections.defaultdict(set)
    texts = set()
    for file, _, error, text in rows:
        batch = next(b for b, _, prefixes in BATCHES if any(file.startswith(p) for p in prefixes))
        entry = plan[batch]["files"][file]
        if error:
            entry[2] += 1
        else:
            entry[0] += 1
            entry[1] += len(text.split())
            used[text].add(batch)
            texts.add(text)
    translated = website(locales)
    matches = {t: translated[t] for t in sorted(texts) if t in translated and t not in NOT_THE_SAME}
    area = "crates/orbiters-i18n/catalog/website"
    os.makedirs(area, exist_ok=True)
    for tag in ["ja", "fr", "es", "pt-BR"]:
        with open(f"{area}/{tag}.json", "w", encoding="utf8", newline="\n") as f:
            json.dump({t: v[tag] for t, v in matches.items()}, f, ensure_ascii=False, indent=2, sort_keys=True)
            f.write("\n")
    with open(f"{area}/files.txt", "w", newline="\n") as f:
        f.write("# Copied from the website's translations by scripts/i18n/batches.py: this area covers no file of its own.\n")
    os.makedirs("reports/i18n", exist_ok=True)
    shared = sum(1 for t, b in used.items() if len(b) > 1)
    out = ["# Translation batches", "", f"{len(texts)} different texts, {sum(len(t.split()) for t in texts)} words to translate into 4 languages. "
           f"{len(matches)} of them ({sum(len(t.split()) for t in matches)} words) are in `catalog/website` already, from the website, word for word. "
           f"{shared} texts are used by more than one batch: see \"The same text in two batches\" in BATCH-BRIEF.md.", "",
           "Each file is in one batch, so batches can be wrapped side by side and merge cleanly. Strings and words are of what is not yet wrapped, errors apart: \"Errors\" are error strings, also to translate (Enzo) and held to the test.", "",
           "| Batch | What | Files | Strings | Words | Not in `website` | Errors |", "|---|---|---:|---:|---:|---:|---:|"]
    summary = {}
    for name, about, _ in BATCHES:
        files = plan[name]["files"]
        names = {t for t, b in used.items() if name in b}
        missing = sorted(t for t in names if t not in matches)
        out.append(f"| `{name}` | {about} | {len(files)} | {sum(v[0] for v in files.values())} | {sum(v[1] for v in files.values())} | {len(missing)} | {sum(v[2] for v in files.values())} |")
        summary[name] = {"about": about, "files": {k: {"strings": v[0], "words": v[1], "errors": v[2]} for k, v in sorted(files.items())}}
    out.append("")
    for name, about, _ in BATCHES:
        out.append(f"## `{name}`: {about}")
        out.append("")
        for file, v in sorted(plan[name]["files"].items(), key=lambda kv: -kv[1][1]):
            out.append(f"- `{file}`: {v[0]} strings, {v[1]} words" + (f", {v[2]} errors" if v[2] else ""))
        out.append("")
    with open("reports/i18n/BATCHES.md", "w", encoding="utf8", newline="\n") as f:
        f.write("\n".join(out))
    with open("reports/i18n/batches.json", "w", encoding="utf8", newline="\n") as f:
        json.dump(summary, f, indent=1)
    print("\n".join(out[:22]))


if __name__ == "__main__":
    main()
