#!/usr/bin/env python3
"""Writes THIRD_PARTY_LICENSES.md: the license text of every crate built into the programs Orbiters desktop ships
(orbiters-desktop, -capture, -reconstruct, -voice, -call) for Windows, and of the native libraries next to them.
MIT, BSD and similar licenses ask for their notice in binary distributions too, so the file ships with the binaries.

    python scripts/third_party_licenses.py            # from the repository root; regenerate after Cargo.lock changes

Crates under several licenses ("MIT OR Apache-2.0") are listed under the first of PREFERRED they offer. A crate
published without its license file gets the license's standard text with its authors as the copyright holders.
Build scripts' and procedural macros' dependencies are not in the programs and are left out.
"""

import collections
import json
import os
import re
import string
import subprocess
import sys

SHIPPED = ["orbiters-desktop", "orbiters-capture", "orbiters-reconstruct", "orbiters-voice", "orbiters-call"]
PLATFORM = "x86_64-pc-windows-msvc"
OUTPUT = "THIRD_PARTY_LICENSES.md"
# What is trimmed around a license text: a byte-order mark and blank space.
STRIP = string.whitespace + chr(0xFEFF)

# The license chosen when a crate offers several, most permissive first.
PREFERRED = ["MIT", "Apache-2.0", "BSD-3-Clause", "BSD-2-Clause", "ISC", "Zlib", "0BSD", "Unlicense", "BSL-1.0",
             "Unicode-3.0", "CDLA-Permissive-2.0", "OFL-1.1", "Ubuntu-font-1.0", "MPL-2.0"]

# Subfolders whose matching files are the licenses of what a crate bundles: (folder, file name pattern).
BUNDLED = [("fonts", r"(?i).*\.txt$"), ("opus", r"^COPYING$")]

# Source assets embedded by our own crates are not Cargo dependencies, but their notices ship with the binaries.
VENDORED = [("ALCOM project templates v1.9.3", "MIT", "crates/orbiters-projects/templates/LICENSE")] + [
    # The photo editor's text fonts (orbiters-ui photo::text), embedded in orbiters-ui.
    (f"{name} font", "OFL-1.1", f"crates/orbiters-ui/assets/fonts/{folder}/OFL.txt")
    for name, folder in [("Inter", "Inter"), ("Bonbon", "Bonbon"), ("Creepster", "Creepster"), ("Diplomata", "Diplomata"),
                         ("JetBrains Mono", "JetBrains_Mono"), ("Matemasie", "Matemasie"), ("Monoton", "Monoton"),
                         ("Pirata One", "Pirata_One"), ("Plaster", "Plaster")]
] + [
    # The interface's fonts to choose from (orbiters-ui faces), embedded in orbiters-ui.
    (f"{name} interface font", "OFL-1.1", f"crates/orbiters-ui/assets/faces/{folder}/OFL.txt")
    for name, folder in [("Inter", "Inter"), ("Atkinson Hyperlegible Next", "AtkinsonHyperlegibleNext"), ("Nunito", "Nunito"),
                         ("Montserrat", "Montserrat"), ("JetBrains Mono", "JetBrainsMono")]
] + [
    ("Roboto Slab interface font", "Apache-2.0", "crates/orbiters-ui/assets/faces/RobotoSlab/LICENSE.txt"),
    # The backgrounds' shaders (orbiters-ui background), the website's, ported.
    ("React Bits backgrounds", "MIT", "crates/orbiters-ui/assets/licenses/react-bits.txt"),
]

# How a license file's text is recognised.
SIGNATURES = {
    "MIT": ["permission is hereby granted, free of charge"],
    "Apache-2.0": ["apache license", "version 2.0"],
    "BSD-3-Clause": ["redistribution and use in source and binary forms", "neither the name"],
    "BSD-2-Clause": ["redistribution and use in source and binary forms"],
    "ISC": ["permission to use, copy, modify, and/or distribute this software"],
    "Zlib": ["this software is provided 'as-is'"],
    "0BSD": ["permission to use, copy, modify, and/or distribute this software for any purpose with or without fee"],
    "Unlicense": ["this is free and unencumbered software released into the public domain"],
    "BSL-1.0": ["boost software license"],
    "Unicode-3.0": ["unicode license"],
    "CDLA-Permissive-2.0": ["community data license agreement"],
    "OFL-1.1": ["sil open font license"],
    "Ubuntu-font-1.0": ["ubuntu font licence"],
    "MPL-2.0": ["mozilla public license"],
}

MIT_TEMPLATE = """MIT License

Copyright (c) {holders}

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the Software without restriction, including without limitation the
rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit
persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
"""

# The WebView2 SDK's LICENSE.txt (NuGet package Microsoft.Web.WebView2 1.0.3800.47, whose x64 WebView2LoaderStatic.lib
# is byte for byte the one webview2-com-sys 0.39.1 carries and links into orbiters-desktop). The crate's own MIT licence
# does not cover Microsoft's loader. Check it again when webview2-com-sys changes.
WEBVIEW2_SDK_LICENSE = """Copyright (C) Microsoft Corporation. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * The name of Microsoft Corporation, or the names of its contributors
may not be used to endorse or promote products derived from this
software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE."""

# Native code that is not Rust: the libraries the voice worker loads, and the WebView2 loader linked into the window's
# program. Their texts are not in the Cargo registry.
NATIVE = [
    ("ONNX Runtime (onnxruntime.dll)", "MIT", MIT_TEMPLATE.format(holders="Microsoft Corporation")),
    ("sherpa-onnx C API (sherpa-onnx-c-api.dll)", "Apache-2.0", None),
    ("Microsoft WebView2 SDK loader 1.0.3800.47 (WebView2LoaderStatic.lib, linked into orbiters-desktop by webview2-com-sys)",
     "BSD-3-Clause", WEBVIEW2_SDK_LICENSE),
]


def metadata():
    cmd = ["cargo", "metadata", "--format-version", "1", "--filter-platform", PLATFORM, "--locked"]
    try:
        out = subprocess.run(cmd + ["--offline"], check=True, capture_output=True).stdout
    except subprocess.CalledProcessError:
        out = subprocess.run(cmd, check=True, capture_output=True).stdout
    return json.loads(out)


def shipped_packages(meta):
    """External packages reachable from the shipped programs through normal (not build or dev) dependencies."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    roots = [i for i in meta["workspace_members"] if packages[i]["name"] in SHIPPED]
    seen, stack = set(), list(roots)
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        for dep in nodes[current]["deps"]:
            if not any(kind["kind"] is None for kind in dep["dep_kinds"]):
                continue
            package = packages[dep["pkg"]]
            if any("proc-macro" in target["kind"] for target in package["targets"]):
                continue
            stack.append(dep["pkg"])
    # Path dependencies (the workspace, the contracts submodule) are Orbiters' own.
    return sorted((packages[i] for i in seen if packages[i]["source"]), key=lambda p: (p["name"], p["version"]))


def choose(expression):
    """The licenses that apply under `expression`: every part of an AND, the preferred option of an OR."""
    expression = (expression or "").replace("/", " OR ")
    expression = re.sub(r"\s+", " ", expression).strip()
    while expression.startswith("(") and expression.endswith(")") and balanced(expression[1:-1]):
        expression = expression[1:-1]
    parts = split_top(expression, " AND ")
    if len(parts) > 1:
        return [license for part in parts for license in choose(part)]
    options = [o.strip("() ") for o in split_top(expression, " OR ")]
    if len(options) > 1:
        options = [o for o in options if o]
        for preferred in PREFERRED:
            for option in options:
                if option == preferred:
                    return [option]
        return choose(options[0])
    return [expression] if expression else []


def balanced(text):
    depth = 0
    for c in text:
        depth += {"(": 1, ")": -1}.get(c, 0)
        if depth < 0:
            return False
    return depth == 0


def split_top(expression, operator):
    parts, depth, start, i = [], 0, 0, 0
    while i < len(expression):
        c = expression[i]
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
        elif depth == 0 and expression.startswith(operator, i):
            parts.append(expression[start:i])
            i += len(operator)
            start = i
            continue
        i += 1
    parts.append(expression[start:])
    return [p.strip() for p in parts]


def license_files(package):
    folder = os.path.dirname(package["manifest_path"])
    names = sorted(f for f in os.listdir(folder) if re.match(r"(?i)^(licen[cs]e|copying|copyright|notice|unlicense)", f))
    files = []
    for name in names:
        path = os.path.join(folder, name)
        if os.path.isfile(path):
            with open(path, encoding="utf-8", errors="replace") as f:
                files.append((name, f.read().strip(STRIP)))
        elif os.path.isdir(path):
            for inner in sorted(os.listdir(path)):
                full = os.path.join(path, inner)
                if os.path.isfile(full):
                    with open(full, encoding="utf-8", errors="replace") as f:
                        files.append((f"{name}/{inner}", f.read().strip(STRIP)))
    # Bundled code and fonts carry their licenses next to them (epaint_default_fonts: OFL, the Ubuntu Font Licence,
    # Hack's; audiopus_sys: libopus, built into the call worker).
    for sub, pattern in BUNDLED:
        inner_folder = os.path.join(folder, sub)
        if os.path.isdir(inner_folder):
            for inner in sorted(os.listdir(inner_folder)):
                if re.match(pattern, inner) and os.path.isfile(os.path.join(inner_folder, inner)):
                    with open(os.path.join(inner_folder, inner), encoding="utf-8", errors="replace") as f:
                        files.append((f"{sub}/{inner}", f.read().strip(STRIP)))
    if package.get("license_file"):
        path = os.path.join(folder, package["license_file"])
        if os.path.isfile(path) and not any(n == package["license_file"] for n, _ in files):
            with open(path, encoding="utf-8", errors="replace") as f:
                files.append((package["license_file"], f.read().strip(STRIP)))
    return files


def matches(license, name, text):
    lowered = text.lower()
    if license == "BSD-2-Clause" and "neither the name" in lowered:
        return False
    if all(s in lowered for s in SIGNATURES.get(license, ["\0"])):
        return True
    tag = {"Apache-2.0": "apache", "MIT": "mit", "BSL-1.0": "boost", "Zlib": "zlib", "ISC": "isc"}.get(license)
    return bool(tag) and tag in name.lower() and len(lowered) > 200


def texts_for(package, licenses):
    """(license, text) for each license that applies, plus any NOTICE file (Apache-2.0 asks for it)."""
    files = license_files(package)
    chosen = []
    for license in licenses:
        found = [text for name, text in files if matches(license, name, text)]
        if not found and len(files) == 1 and len(licenses) == 1 and not files[0][0].lower().startswith("notice"):
            found = [files[0][1]]
        if found:
            chosen.append((license, found[0]))
        elif license == "MIT":
            holders = ", ".join(a.split("<")[0].strip() for a in package.get("authors") or []) or f"the {package['name']} authors"
            chosen.append((license, MIT_TEMPLATE.format(holders=holders).strip()))
        else:
            chosen.append((license, None))  # the standard text, given once below
    # Notices, what the crate bundles, and for a crate under several licenses at once (ring: Apache-2.0 AND ISC, with
    # BoringSSL's) every license file not given yet.
    used = {text for _, text in chosen if text}
    several = " AND " in (package["license"] or "")
    for name, text in files:
        bundled = "/" in name and not name.lower().startswith("licen")
        if name.lower().startswith("notice") or ((bundled or several) and text not in used):
            chosen.append(("NOTICE", text))
            used.add(text)
    return chosen


def normalized(text):
    return re.sub(r"\s+", " ", text).strip()


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    os.chdir(root)
    packages = shipped_packages(metadata())
    groups = collections.OrderedDict()  # (license, normalized text) -> (text, [crate versions])
    standard = collections.defaultdict(list)  # license -> crates that use its standard text
    for package in packages:
        crate = f"{package['name']} {package['version']}"
        for license, text in texts_for(package, choose(package["license"]) or ["MIT"]):
            if text is None:
                standard[license].append(crate)
                continue
            key = (license, normalized(text))
            groups.setdefault(key, (text, []))[1].append(crate)
    for name, license, path in VENDORED:
        with open(path, encoding="utf-8") as source:
            text = source.read().strip(STRIP)
        groups.setdefault((license, normalized(text)), (text, []))[1].append(name)
    for name, license, text in NATIVE:
        if text is None:
            standard[license].append(name)
        else:
            groups.setdefault((license, normalized(text)), (text, []))[1].append(name)

    out = ["# Third-party licenses", "",
           "The license texts of the components built into Orbiters - N4N0's programs (`orbiters-desktop`, "
           "`orbiters-capture`, `orbiters-reconstruct`, `orbiters-voice`, `orbiters-call`) and of the native libraries "
           "next to them. Ship this file with the programs. See `THIRD_PARTY_NOTICES.md` for an overview, the fonts and "
           "the models.", "",
           f"Generated by `scripts/third_party_licenses.py` from `Cargo.lock` ({len(packages)} crates): regenerate it "
           "when the dependencies change.", ""]
    by_license = collections.defaultdict(list)
    for (license, _), (text, crates) in groups.items():
        by_license[license].append((text, crates))
    order = PREFERRED + ["NOTICE"] + sorted(set(by_license) - set(PREFERRED) - {"NOTICE"})
    for license in order:
        entries = by_license.get(license, [])
        if not entries and license not in standard:
            continue
        out += [f"## {license if license != 'NOTICE' else 'Notices'}", ""]
        if license in standard:
            where = "the first text below" if entries else f"https://spdx.org/licenses/{license}.html"
            out += [f"Under the standard {license} text ({where}), published without a copy of their own: "
                    + ", ".join(sorted(standard[license])) + ".", ""]
        for text, crates in sorted(entries, key=lambda e: e[1][0]):
            out += ["Used by " + ", ".join(crates) + ":", "", "```text", text.replace("```", "'''"), "```", ""]
    with open(OUTPUT, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out))
    print(f"{OUTPUT}: {len(packages)} crates, {sum(len(v) for v in by_license.values())} texts", file=sys.stderr)


if __name__ == "__main__":
    main()
