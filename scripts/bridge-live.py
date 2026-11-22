#!/usr/bin/env python3
"""Calls the VRChat bridge's tools over its real transport, by hand, as an agent would: for looking at the real game when a
Claude Code or Codex session is more than the job needs. See docs/vrchat-bridge.md, "Running it live".

    python scripts/bridge-live.py get_state
    python scripts/bridge-live.py find_worlds '{"source":"trending"}'
    python scripts/bridge-live.py screenshot '{}' shot.jpg
    python scripts/bridge-live.py press '{"input":"esc"}'
    python scripts/bridge-live.py --timeline run.tsv walk '{"direction":"forward","seconds":2}'
    python scripts/bridge-live.py --tools

The port and token are read the way the app keeps them (Windows' protection, this user only) and never printed. Each call's
answer goes to standard output as JSON; a picture is saved to the third argument. --timeline adds a line (time, seconds taken,
tool, outcome) to a file.
"""

import base64
import ctypes
import json
import os
import sys
import time
import urllib.error
import urllib.request

ENTROPY = b"Orbiters.N4N0 VRChat bridge"


if sys.platform == "win32":
    import ctypes.wintypes as wintypes

    class Blob(ctypes.Structure):
        _fields_ = [("size", wintypes.DWORD), ("data", ctypes.POINTER(ctypes.c_char))]


def blob(data):
    return Blob(len(data), ctypes.cast(ctypes.create_string_buffer(data, len(data)), ctypes.POINTER(ctypes.c_char)))


def keys():
    """The port and token from the file the app seals them in (bridge/keeper.rs, Keys). On a Mac the file is this user's
    only and not sealed (orbiters_core::seal_for_user)."""
    if sys.platform == "darwin":
        path = os.path.expanduser("~/Library/Application Support/orbiters-desktop/vrchat-bridge.dat")
        with open(path, "rb") as file:
            plain = json.loads(file.read())
        return plain["port"], plain["token"]
    path = os.path.expandvars(r"%APPDATA%\Orbiters\Desktop\vrchat-bridge.dat")
    with open(path, "rb") as file:
        sealed = blob(file.read())
    entropy, out = blob(ENTROPY), Blob()
    if not ctypes.windll.crypt32.CryptUnprotectData(ctypes.byref(sealed), None, ctypes.byref(entropy), None, None, 0, ctypes.byref(out)):
        raise SystemExit("The bridge's keys could not be opened: not this user's?")
    plain = json.loads(ctypes.string_at(out.data, out.size))
    return plain["port"], plain["token"]


class Bridge:
    def __init__(self):
        port, self.token = keys()
        self.url = f"http://127.0.0.1:{port}/mcp"
        self.serial = 0
        self.post("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "bridge-live.py", "version": "1"}})

    def post(self, method, params, limit=70):
        self.serial += 1
        body = json.dumps({"jsonrpc": "2.0", "id": self.serial, "method": method, "params": params}).encode()
        headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream", "Authorization": f"Bearer {self.token}"}
        request = urllib.request.Request(self.url, body, headers)
        try:
            with urllib.request.urlopen(request, timeout=limit) as answer:
                return json.loads(answer.read())
        except urllib.error.HTTPError as error:
            raise SystemExit(f"The bridge answered {error.code}: {error.read()[:300]!r}")
        except urllib.error.URLError as error:
            raise SystemExit(f"The bridge did not answer at {self.url}: {error.reason}. Is it on in Settings, Experimental?")

    def call(self, tool, arguments):
        return self.post("tools/call", {"name": tool, "arguments": arguments})


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    args = sys.argv[1:]
    timeline = None
    if args[:1] == ["--timeline"]:
        timeline, args = args[1], args[2:]
    if not args:
        raise SystemExit(__doc__)
    bridge = Bridge()
    if args[0] == "--tools":
        print(json.dumps([t["name"] for t in bridge.post("tools/list", {})["result"]["tools"]], indent=1))
        return
    tool = args[0]
    arguments = json.loads(args[1]) if len(args) > 1 else {}
    begin = time.time()
    answer = bridge.call(tool, arguments)
    took = time.time() - begin
    result = answer.get("result", answer)
    said = []
    for part in result.get("content", []):
        if part.get("type") == "image":
            path = args[2] if len(args) > 2 else f"shot-{int(begin)}.jpg"
            with open(path, "wb") as file:
                file.write(base64.b64decode(part["data"]))
            said.append({"saved": path})
        elif part.get("type") == "text":
            try:
                said.append(json.loads(part["text"]))
            except ValueError:
                said.append(part["text"])
    failed = bool(result.get("isError"))
    code = said[0].get("code") if failed and said and isinstance(said[0], dict) else ""
    print(json.dumps(said[0] if len(said) == 1 else said, indent=1, ensure_ascii=False))
    if timeline:
        with open(timeline, "a", encoding="utf8") as file:
            stamp = time.strftime("%H:%M:%S", time.localtime(begin))
            file.write(f"{stamp}\t{took:.1f}\t{tool}\t{json.dumps(arguments)}\t{'FAILED ' + str(code) if failed else 'ok'}\n")
    if failed:
        sys.exit(1)


if __name__ == "__main__":
    main()
