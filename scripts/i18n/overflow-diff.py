#!/usr/bin/env python3
"""What a language broke: compares two `--text-report` files, a language's against English's.

    orbiters-desktop --screenshot out/en --only orbit --language en --text-report out/en.tsv
    orbiters-desktop --screenshot out/fr --only orbit --language fr --text-report out/fr.tsv
    python scripts/i18n/overflow-diff.py out/en.tsv out/fr.tsv

English's own findings are the baseline: for each scene and kind (box, clip, cut, over), only what the language has more of
than English is listed, worst first. Exits 1 when anything is listed. Scene names are the file names of the renders.
"""
import collections
import sys


def read(path):
    rows = collections.defaultdict(list)
    with open(path, encoding="utf8") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t", 3)
            if len(parts) == 4:
                scene, kind, by, text = parts
                rows[(scene, kind)].append((int(by), text))
    return rows


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    base, other = read(sys.argv[1]), read(sys.argv[2])
    sys.stdout.reconfigure(encoding="utf8")
    worse = 0
    for (scene, kind), found in sorted(other.items()):
        extra = len(found) - len(base.get((scene, kind), []))
        if extra <= 0:
            continue
        worse += extra
        print(f"{scene}  {kind}: {extra} more than English")
        known = {text for _, text in base.get((scene, kind), [])}
        for by, text in sorted(found, reverse=True):
            if text not in known:
                print(f"    {by:>4}  {text}")
    print(f"{worse} more than English")
    sys.exit(1 if worse else 0)


if __name__ == "__main__":
    main()
