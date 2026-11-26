#!/usr/bin/env python3
"""Moves a checkout to the two rules of 2026-11-25 (XCT-31, XCT-27), outside tests:

- a lock goes on after a panic (`orbiters_core::locks`): `x.lock().unwrap()` is `x.locked()`, and so on;
- what went wrong is in the app's log (`orbiters_core::app_log`): `eprintln!("…{e}")` is `log_warn!("…{e}", e = e)`.

To land a branch made before them: run this on the branch after merging main (`python scripts/visible_failures.py`), look at
what it changed, commit. Safe to run again, and on code already moved. What it cannot move it names (exit code 1). What holds
the rules afterwards is a test, `unseen::tests`, which reads the app as `orbiters_core::source` does.
"""
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent / "crates"
# Whose locks and messages: what runs in the service, the window and the overlay, and knows orbiters-core.
LOCKS = ["orbiters-desktop", "orbiters-core", "orbiters-n4n0", "orbiters-n4n0-core", "orbiters-projects"]
LINES = ["orbiters-desktop"]
# What a developer's command line writes is its output, not the app's log.
COMMANDS = {
    "background_bench.rs", "bench.rs", "harness.rs", "idle_frames.rs", "link_between.rs", "link_smoke.rs", "bridge/lab.rs", "controller/publish_cost.rs",
    "controller/picture_repair.rs", "overlay/orbit/paint_bench.rs", "overlay/orbit/desktop_mac/lab.rs", "overlay/worlds/shots.rs", "overlay/worlds/timing_lab.rs",
    "overlay/worlds/sequence.rs", "overlay/eye_picture.rs",
}


def command(rel):
    return rel in COMMANDS or rel == "screenshot.rs" or rel.startswith("screenshot/")


TOKEN = re.compile(
    r"""(?P<skip>//[^\n]*|\s+)
      |(?P<block>/\*)
      |(?P<raw>(?:br|cr|r)(?P<h>\#*)")
      |(?P<text>(?:b|c)?"(?:\\.|[^"\\])*")
      |(?P<char>b?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^\\'])')
      |(?P<life>'[A-Za-z_]\w*)
      |(?P<word>(?:r\#)?\w+)
      |(?P<sign>.)""",
    re.X | re.S,
)


def tokens(text):
    out, i = [], 0
    while i < len(text):
        m = TOKEN.match(text, i)
        kind, end = m.lastgroup if m.lastgroup != "h" else "raw", m.end()
        if m.group("block"):
            depth, i = 1, end
            while depth and i < len(text):
                n = re.compile(r"/\*|\*/").search(text, i)
                if not n:
                    i = len(text)
                    break
                depth += 1 if n.group() == "/*" else -1
                i = n.end()
            continue
        if m.group("raw"):
            close = text.find('"' + m.group("h"), end)
            end = len(text) if close < 0 else close + 1 + len(m.group("h"))
            out.append(("text", i, end))
        elif not m.group("skip"):
            out.append(({"char": "text", "life": "word"}.get(kind, kind), i, end))
        i = end
    return out


class Reading:
    """`orbiters_core::source::Reading`, as far as telling what only tests compile goes."""

    def __init__(self, text):
        self.text, self.t = text, tokens(text)

    def at(self, i):
        if 0 <= i < len(self.t) and self.t[i][0] != "text":
            return self.text[self.t[i][1]:self.t[i][2]]
        return ""

    def closing(self, i):
        depth = 0
        for n in range(i, len(self.t)):
            if self.at(n) in ("(", "[", "{"):
                depth += 1
            elif self.at(n) in (")", "]", "}"):
                depth = max(depth - 1, 0)
                if depth == 0:
                    return n
        return max(len(self.t) - 1, 0)

    def attributes(self, i):
        test, path = False, None
        while self.at(i) == "#" and self.at(i + 1) == "[":
            end = self.closing(i + 1)
            if self.at(i + 2) == "cfg" and self.at(i + 3) == "(":
                test |= self.never(i + 4)[0]
            elif self.at(i + 2) == "test":
                test |= end == i + 3
            if self.at(i + 2) == "path" and self.at(i + 3) == "=":
                path = self.text[self.t[i + 4][1]:self.t[i + 4][2]].strip('"')
            i = end + 1
        return i, test, path

    def never(self, i):
        name = self.at(i)
        if self.at(i + 1) == "(":
            end = self.closing(i + 1)
            n, parts = i + 2, []
            while n < end:
                no, yes, after = self.never(n)
                parts.append((no, yes))
                n = max(after, n) + 1
            if name == "not":
                no, yes = (parts[0][1], parts[0][0]) if parts else (False, False)
            elif name == "all":
                no, yes = any(p[0] for p in parts), all(p[1] for p in parts)
            elif name == "any":
                no, yes = all(p[0] for p in parts), any(p[1] for p in parts)
            else:
                no, yes = False, False
            return no, yes, end + 1
        if self.at(i + 1) == "=":
            value = self.text[self.t[i + 2][1]:self.t[i + 2][2]] if i + 2 < len(self.t) else ""
            return name == "feature" and value == '"test-support"', False, i + 3
        return name == "test", False, i + 1

    def end(self, i):
        while self.at(i) == "pub":
            i = self.closing(i + 1) + 1 if self.at(i + 1) == "(" else i + 1
        first = self.at(i)
        value = first in ("let", "use", "static", "type") or (first == "const" and self.at(i + 1) not in ("fn", "unsafe", "async", "extern"))
        item = first in ("fn", "mod", "impl", "struct", "enum", "trait", "union", "extern", "unsafe", "async", "const", "macro_rules")
        angles, arm = 0, False
        while i < len(self.t):
            a = self.at(i)
            if a == ";":
                return i + 1
            if a in (")", "]", "}"):
                return i
            if a == "{":
                end = self.closing(i) + 1
                goes_on = self.at(end) in (".", "?", "else", "=") or value or (not item and not arm and self.at(end) in (";", ","))
                if not goes_on:
                    return end + 1 if arm and self.at(end) == "," else end
                i = end
                continue
            if a in ("(", "["):
                i = self.closing(i) + 1
                continue
            if a == "=" and self.at(i + 1) == ">":
                arm, angles = True, 0
                i += 2
                continue
            if a == "-" and self.at(i + 1) == ">":
                i += 2
                continue
            if a == "<" and re.search(r"[\w:]$", self.text[:self.t[i][1]][-1:] or " "):
                angles += 1
            elif a == ">":
                angles = max(angles - 1, 0)
            elif a == "," and not item and not value and angles == 0:
                return i + 1
            i += 1
        return len(self.t)

    def tests(self):
        out, i = [], 0
        while i < len(self.t):
            if self.at(i) == "#" and self.at(i + 1) == "!" and self.at(i + 2) == "[":
                if self.at(i + 3) == "cfg" and self.at(i + 4) == "(" and self.never(i + 5)[0]:
                    return [(0, len(self.text))]
                i = self.closing(i + 2) + 1
                continue
            if self.at(i) == "#" and self.at(i + 1) == "[":
                item, test, _ = self.attributes(i)
                if test:
                    end = max(self.end(item), item)
                    out.append((self.t[i][1], self.t[end - 1][2] if end else 0))
                    i = max(end, i + 1)
                else:
                    i = max(item, i + 1)
                continue
            i += 1
        return out

    def modules(self):
        tests, out, inside, i, path = self.tests(), [], [], 0, None
        while i < len(self.t):
            inside = [(n, e) for n, e in inside if e > i]
            if self.at(i) == "#" and self.at(i + 1) == "[":
                i2, _, path = self.attributes(i)
                i = max(i2, i + 1)
                continue
            if self.at(i) == "mod" and i + 1 < len(self.t) and self.t[i + 1][0] == "word":
                name = self.at(i + 1)
                if self.at(i + 2) == ";":
                    at = self.t[i][1]
                    out.append(([n for n, _ in inside] + [name], path, any(a <= at < b for a, b in tests)))
                elif self.at(i + 2) == "{":
                    inside.append((name, self.closing(i + 2)))
                path = None
                i += 2
                continue
            if self.at(i) == "(" and self.at(i - 1) == "pub":
                i = self.closing(i)
            elif self.at(i) != "pub":
                path = None
            i += 1
        return out


def compiled(src):
    """The app's files below `src`: path -> its reading (those only tests compile are left out)."""
    readings = {}
    for path in sorted(src.rglob("*.rs")):
        readings[path] = Reading(path.read_bytes().decode("utf-8").replace("\r\n", "\n"))
    declared = []
    for path, r in readings.items():
        here = path.parent
        own = here if path.stem in ("mod", "lib", "main") else here / path.stem
        for names, named, test in r.modules():
            folder = own.joinpath(*names[:-1])
            if named is not None:
                targets = [(here if len(names) == 1 else folder) / named]
            else:
                targets = [folder / (names[-1] + ".rs"), folder / names[-1] / "mod.rs"]
            for t in targets:
                t = pathlib.Path(*[p for p in t.parts])
                if t in readings:
                    declared.append((path, t, test))
    children = {c for _, c, _ in declared}
    known = {p for p in readings if p not in children}
    while True:
        more = {c for p, c, test in declared if not test and p in known and c not in known}
        if not more:
            return {p: r for p, r in readings.items() if p in known}
        known |= more


INTO = r"(?:std::sync::)?PoisonError::into_inner|\|(\w+)\|\s*\1\.into_inner\(\)"
REST = rf"\s*\.(?:unwrap\(\)|expect\(\"[^\"]*\"\)|unwrap_or_else\(\s*(?:{INTO})\s*\))"
CALLS = [
    (re.compile(r"\.lock\(\)" + REST), ".locked()"),
    (re.compile(r"\.read\(\)" + REST), ".read_locked()"),
    (re.compile(r"\.write\(\)" + REST), ".write_locked()"),
]
WAITS = {"wait", "wait_timeout", "wait_while", "wait_timeout_while"}
AFTER_WAIT = re.compile(REST)
USE = "use orbiters_core::locks::*;\n"


def locks(r, tests):
    """The edits that move the locks of one file: (from, to, text)."""
    out = []
    inside = lambda at: any(a <= at < b for a, b in tests)
    texts = [(a, b) for kind, a, b in r.t if kind == "text"]
    for pattern, new in CALLS:
        for m in pattern.finditer(r.text):
            if not inside(m.start()) and not any(a <= m.start() < b for a, b in texts):
                out.append((m.start(), m.end(), new))
    # `changed.wait_timeout(held, most).unwrap()`
    for i, (kind, a, _) in enumerate(r.t):
        if kind == "word" and r.at(i) in WAITS and r.at(i - 1) == "." and r.at(i + 1) == "(" and not inside(a):
            end = r.t[r.closing(i + 1)][2]
            m = AFTER_WAIT.match(r.text, end)
            if m:
                out.append((r.t[i][1], r.t[i][2], r.at(i) + "_locked"))
                out.append((m.start(), m.end(), ""))
    return out


def captured(text):
    """The names a format string captures."""
    names, i = [], 0
    while i < len(text):
        if text[i] in "{}":
            if text[i:i + 2] in ("{{", "}}"):
                i += 2
                continue
            if text[i] == "{":
                end = text.find("}", i)
                name = text[i + 1:end].split(":")[0].strip()
                if re.fullmatch(r"[A-Za-z_]\w*", name) and name not in names:
                    names.append(name)
                i = end
        i += 1
    return names


def lines(r, tests, rel):
    out, wrong = [], []
    for i, (kind, a, b) in enumerate(r.t):
        if not (kind == "word" and r.at(i) == "eprintln" and r.at(i + 1) == "!") or any(x <= a < y for x, y in tests):
            continue
        if r.at(i + 2) not in ("(", "{", "["):
            continue
        if i + 3 >= len(r.t) or r.t[i + 3][0] != "text":
            wrong.append((a, "a message that is no text of the source"))
            continue
        close = r.closing(i + 2)
        text = r.text[r.t[i + 3][1]:r.t[i + 3][2]]
        given = {r.at(n) for n in range(i + 4, close) if r.at(n + 1) == "=" and r.at(n + 2) != "=" and r.at(n - 1) == ","}
        names = [n for n in captured(text) if n not in given]
        body = r.text[r.t[i + 3][2]:r.t[close][1]].rstrip()
        comma = "" if body.endswith(",") else ","
        added = (comma + " " + ", ".join(f"{n} = {n}" for n in names)) if names else ""
        said = (text + body).lower()
        error = any(w in said for w in ("could not", "cannot", "failed", "fails", "error", "did not", "not read", "refused", "panicked", "lost", "damaged", "gave up", "timed out", "unreadable", "no answer"))
        out.append((a, r.t[i + 1][2], "log_warn!" if error or any(n in ("e", "err", "error", "why") for n in names) else "log_info!"))
        out.append((r.t[close][1] - (len(r.text[:r.t[close][1]]) - len(r.text[:r.t[close][1]].rstrip())), r.t[close][1], added))
    return out, wrong


def put_use(text, use, edits_at):
    """`use` before the file's first `use` (or its first item), and in each module written inside the file that needs it
    and does not see what is around it."""
    r = Reading(text)
    modules = []
    for i in range(len(r.t)):
        if r.at(i) == "mod" and r.at(i + 2) == "{":
            modules.append((r.t[i + 2][2], r.t[r.closing(i + 2)][1]))
    at_root, inner = False, set()
    for at in edits_at:
        around = [m for m in modules if m[0] <= at < m[1]]
        if not around:
            at_root = True
            continue
        # (The innermost; `use super::*` sees the outer one's.)
        start, end = max(around)
        if "use super::*;" in text[start:end]:
            at_root = True
        else:
            inner.add((start, end))
    places = []
    for start, end in inner:
        if use.strip() in text[start:end]:
            continue
        first = re.compile(r"\n( *)(?=\S)").search(text, start)
        places.append((first.end(), first.group(1)))
    if at_root and not re.search("^" + re.escape(use.strip()), text, re.M):
        m = re.compile(r"^(?:pub(?:\([^)]*\))? )?use ", re.M).search(text)
        if not m:
            m = re.compile(r"^(?:#\[|pub |fn |struct |enum |impl |const |static |type |mod |trait )", re.M).search(text)
        at = m.start()
        # (Above an attribute or a comment of that item.)
        while at and not text[:at].endswith("\n\n"):
            before = text[:at - 1].rsplit("\n", 1)[-1]
            if not (before.startswith("#[") or before.startswith("///") or before.startswith("// ")):
                break
            at -= len(before) + 1
        places.append((at, ""))
    for at, indent in sorted(places, reverse=True):
        text = text[:at] + use + indent + text[at:]
    return text


def main():
    wrong, changed = [], 0
    only = set(sys.argv[1:])  # locks, lines: one of the two alone
    for crate in LOCKS:
        src = ROOT / crate / "src"
        for path, r in compiled(src).items():
            rel = path.relative_to(src).as_posix()
            if crate == "orbiters-core" and rel in ("locks.rs", "app_log.rs", "panic_log.rs"):
                continue
            tests = r.tests()
            edits, uses = [], []
            if not only or "locks" in only:
                found = locks(r, tests)
                if found:
                    edits += found
                    uses.append(USE.replace("orbiters_core", "crate") if crate == "orbiters-core" else USE)
            if crate in LINES and not command(rel) and (not only or "lines" in only):
                found, bad = lines(r, tests, rel)
                edits += found
                wrong += [f"{crate}/src/{rel}:{r.text[:a].count(chr(10)) + 1}: {why}" for a, why in bad]
            if not edits:
                continue
            text = r.text
            for a, b, new in sorted(edits, reverse=True):
                text = text[:a] + new + text[b:]
            for use in uses:
                tests_now = Reading(text).tests()
                sites = [m.start() for m in re.finditer(r"\.(?:try_|read_|write_)?locked\(|\.wait\w*_locked\(", text)]
                text = put_use(text, use, [s for s in sites if not any(a <= s < b for a, b in tests_now)])
            raw = path.read_bytes()
            path.write_bytes((text.replace("\n", "\r\n") if b"\r\n" in raw else text).encode("utf-8"))
            changed += 1
            print(f"{crate}/src/{rel}: {len(edits)}")
    print(f"{changed} files")
    for w in wrong:
        print("NOT MOVED", w)
    return 1 if wrong else 0


if __name__ == "__main__":
    sys.exit(main())
