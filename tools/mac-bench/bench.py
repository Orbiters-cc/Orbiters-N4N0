#!/usr/bin/env python3
"""One benchmark run of VRChat on a Mac through obelisk-cli (docs/mac-performance.md, the procedure).

Starts VRChat with `obelisk-cli steam play 438100 [--render-percent N] -- <arguments> <link>` in a private instance of a
fixed world, waits until the local player has arrived in this start's own VRChat log (the log whose command line holds
the link) and loading and shader compilation have settled, then records equal timed windows with nobody touching the
game, quits by session (never forced) and writes one results file, whatever happened (the outcome says what). Nothing
here sends input to the game, changes a setting or injects anything: it reads the engine's log, VRChat's log, `ps`,
`ioreg`, `pmset` and `lsappinfo`.

The three frame counters are kept apart (R3), per swapchain generation (a new one at each `guest swapchain: created`):
  - guest Present: Neptune's nptunix line, `<n> presents (<m> shown events), last 60: interval avg X ms, fence wait avg
    Y ms` (X: time between the game's submissions; Y: how long each waited for the host to finish it);
  - worker shown events: the `(<m> shown events)` of the same line (counts zero-time callbacks too);
  - wsd's positive Metal presentation: the Metal HUD's per-frame intervals (`metal-HUD:` lines, raw).
Rates are counted: the presents between two lines of the same generation divided by the time between those two lines
(read times), the line before a window included as its base, never a whole window's length.

A window is scored only when, from its start to its end, the screen was unlocked, VRChat's window server was the
frontmost app and the window server presented frames (positive HUD intervals); otherwise it is kept with `valid: false`
and why. `--allow-locked` scores production alone behind a locked screen (and says so in every window).

usage: bench.py --link 'vrchat://launch?...' [--percent N] [--arg A]... [--warm 90] [--window 60] [--windows 3]
                [--cli PATH] [--allow-locked] --out results.json
"""
import argparse, datetime, glob, json, os, re, subprocess, sys, threading, time

HOME = os.path.expanduser("~")
ENGINE_LOG = f"{HOME}/Library/Logs/Obelisk/Steam/wine-438100.log"
LIBRARY = f"{HOME}/Library/Application Support/cc.orbiters.obelisk-cli"
VRC_LOGS = f"{LIBRARY}/Prefixes/steam-438100-windows-linux/prefix/drive_c/users/crossover/AppData/LocalLow/VRChat/VRChat"

NPT = re.compile(r"neptune unixlib wsi \(wsd\): (\d+) presents \((\d+) shown events\), last 60: interval avg ([\d.]+) ms, fence wait avg ([\d.]+) ms")
HELD = re.compile(r"wsd: window \d+ overlay: frame held ([\d.]+) ms")
HUD = re.compile(r"metal-HUD: (.*)$")
SWAP = re.compile(r"guest swapchain: created (\d+)x(\d+) fmt=(\d+) buffers=(\d+)")
RENDER = re.compile(r"wsd: render percent: requested (\S+), applied (\d+).*\(desktop (\d+)x(\d+) px\)")
STOP = re.compile(r"lkd: sig: |lkd: ptrace: ")
START_DEADLINE = 900   # s for the engine to say the game runs (it may download first)
ARRIVE_DEADLINE = 600  # s from running to the local player's arrival


def now():
    return time.monotonic()


def run(cmd, timeout=10):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, timeout=timeout).stdout
    except Exception as e:  # noqa: BLE001 - recorded, never fatal
        return f"<{cmd[0]}: {e}>"


def screen_locked():
    """True / False from the console's own state (ioreg IOConsoleLocked), None when it cannot be told."""
    out = run(["/usr/sbin/ioreg", "-n", "Root", "-d1", "-a"])
    m = re.search(r"<key>IOConsoleLocked</key>\s*<(true|false)/>", out)
    return None if not m else m.group(1) == "true"


def frontmost():
    """The frontmost app's name (lsappinfo), or None."""
    asn = run(["/usr/bin/lsappinfo", "front"]).strip()
    out = run(["/usr/bin/lsappinfo", "info", "-only", "name", asn]) if asn else ""
    m = re.search(r'"LSDisplayName"="([^"]*)"', out)
    return m.group(1) if m else None


class Follow(threading.Thread):
    """Follows a growing file, each line stamped with the monotonic time it was read (the engine's lines carry none)."""

    def __init__(self, path_fn, sink):
        super().__init__(daemon=True)
        self.path_fn, self.sink, self.stop = path_fn, sink, False

    def run(self):
        path = None
        while not self.stop and not path:
            path = self.path_fn()
            time.sleep(0.5)
        if not path:
            return
        with open(path, "r", errors="replace") as f:
            buf = ""
            while not self.stop:
                chunk = f.read()
                if not chunk:
                    time.sleep(0.2)
                    continue
                buf += chunk
                *lines, buf = buf.split("\n")
                t = now()
                for line in lines:
                    self.sink(t, line)


def own_vrchat_log(since, link):
    """This start's VRChat log: begun since the start, its head (the command line) holding the link."""
    for p in sorted(glob.glob(f"{VRC_LOGS}/output_log_*.txt"), key=os.path.getmtime, reverse=True):
        if os.path.getmtime(p) < since - 2:
            continue
        try:
            with open(p, "r", errors="replace") as f:
                if link in f.read(256 * 1024):
                    return p
        except OSError:
            pass
    return None


def sampler(state, stop):
    """Once a second: GPU utilization (ioreg), CPU of the engine's processes' threads (ps -M), lock and front state."""
    while not stop.is_set():
        t = now()
        out = run(["/usr/sbin/ioreg", "-r", "-d", "1", "-c", "IOAccelerator"], 5)
        m = re.search(r'"Device Utilization %"=(\d+)', out)
        state["gpu"].append((t, int(m.group(1)) if m else None))
        state["lock"].append((t, screen_locked(), frontmost()))
        listing = run(["/bin/ps", "-A", "-o", "pid=,comm="], 5)
        names = {}
        for line in listing.splitlines():
            parts = line.strip().split(None, 1)
            if len(parts) == 2 and parts[1].rsplit("/", 1)[-1] in ("fexdarwin-run", "virgl_render_server", "wsd"):
                names[parts[0]] = parts[1].rsplit("/", 1)[-1]
        snap = {}
        if names:
            for line in run(["/bin/ps", "-M", "-p", ",".join(names)], 5).splitlines()[1:]:
                tok = line.split()
                try:
                    if len(tok) >= 4 and tok[1] in names:
                        snap.setdefault(tok[1], []).append(float(tok[3]))
                    elif len(tok) >= 2 and tok[0] in names:
                        snap.setdefault(tok[0], []).append(float(tok[1]))
                except ValueError:
                    pass
        state["cpu"].append((t, {p: {"name": names[p], "sum": round(sum(v), 1)} for p, v in snap.items()}))
        stop.wait(1.0)


def pct(xs, q):
    if not xs:
        return None
    xs = sorted(xs)
    k = (len(xs) - 1) * q
    lo, hi = int(k), min(int(k) + 1, len(xs) - 1)
    return round(xs[lo] + (xs[hi] - xs[lo]) * (k - lo), 3)


def generations(lines):
    """Each NPT line with its swapchain generation: (read time, generation, presents, shown events, interval, fence)."""
    gen, out = 0, []
    for t, l in lines:
        if SWAP.search(l):
            gen += 1
            continue
        m = NPT.search(l)
        if m:
            out.append((t, gen, int(m.group(1)), int(m.group(2)), float(m.group(3)), float(m.group(4))))
    return out


def counted_rate(points, t0, t1, index):
    """Counted per second over [t0, t1): per generation, from the last line before t0 (the base) or the first in the
    window to the last in it; summed counts over summed time between matching lines. A count that went back is a new
    generation's (never subtracted across). None when no two lines of one generation span part of the window."""
    total, seconds = 0, 0.0
    for gen in sorted({p[1] for p in points}):
        pts = [p for p in points if p[1] == gen]
        before = [p for p in pts if p[0] < t0]
        inside = [p for p in pts if t0 <= p[0] < t1]
        chain = ([before[-1]] if before else []) + inside
        for a, b in zip(chain, chain[1:]):
            if b[index] >= a[index] and b[0] > a[0]:
                total += b[index] - a[index]
                seconds += b[0] - a[0]
    return (round(total / seconds, 2) if seconds > 0 else None), round(seconds, 1)


def summarize(lines, t0, t1, state, allow_locked):
    win = [(t, l) for t, l in lines if t0 <= t < t1]
    points = generations(lines)
    inside = [p for p in points if t0 <= p[0] < t1]
    hud = []
    for _, l in win:
        m = HUD.search(l)
        if m:
            vals = m.group(1).split(",")[3:]
            hud += [float(v) for v in vals[0::2] if re.fullmatch(r"[\d.]+", v) and float(v) > 0]
    held = [float(m.group(1)) for m in (HELD.search(l) for _, l in win) if m]
    looks = [x for x in state["lock"] if t0 <= x[0] < t1]
    locked = [x[1] for x in looks]
    front = [x[2] for x in looks]
    why = []
    if not looks:
        why.append("no lock or front state was read")
    if any(x is not False for x in locked):
        why.append("the screen was locked (or its state unknown) during the window")
    if any(f != "VRChat" for f in front):
        why.append(f"VRChat was not frontmost all along ({sorted({str(f) for f in front})})")
    if not hud:
        why.append("no frame was shown (no positive presentation time)")
    gens = sorted({p[1] for p in inside})
    if not inside:
        why.append("no guest Present line in the window")
    production_only = allow_locked and why and all(w.startswith(("the screen was locked", "VRChat was not frontmost", "no frame was shown")) for w in why)
    valid = not why
    submitted, sub_s = counted_rate(points, t0, t1, 2)
    shown_events, ev_s = counted_rate(points, t0, t1, 3)
    g = [x[1] for x in state["gpu"] if t0 <= x[0] < t1 and x[1] is not None]
    rs = {}
    for t, snap in state["cpu"]:
        if t0 <= t < t1:
            for pid, v in snap.items():
                rs.setdefault(v["name"] + ":" + pid, []).append(v["sum"])
    return {
        "seconds": round(t1 - t0, 1),
        "valid": valid,
        "production_only": bool(production_only),
        "invalid_because": why,
        "generations_in_window": gens,
        "guest_present": {
            "batches_of_60": len(inside),
            "interval_ms_per_batch": [p[4] for p in inside],
            "interval_ms_median": pct([p[4] for p in inside], 0.5),
            "interval_ms_p10_p90": [pct([p[4] for p in inside], 0.1), pct([p[4] for p in inside], 0.9)],
            "fence_wait_ms_median": pct([p[5] for p in inside], 0.5),
            "submitted_per_second_counted": submitted,
            "counted_over_seconds": sub_s,
        },
        "worker_shown_events_per_second_counted": shown_events,
        "wsd_shown": {
            "hud_intervals_ms": hud,
            "frames": len(hud),
            "per_second": round(len(hud) / (t1 - t0), 2),
            "interval_ms_p50_p90_p99": [pct(hud, 0.5), pct(hud, 0.9), pct(hud, 0.99)],
            "held_over_50ms": len(held),
            "held_over_100ms": sum(1 for x in held if x > 100),
        },
        "gpu_device_utilization_pct": {"median": pct(g, 0.5), "p10_p90": [pct(g, 0.1), pct(g, 0.9)]},
        "process_cpu_pct_median": {k: pct(v, 0.5) for k, v in rs.items()},
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cli", default=None)
    ap.add_argument("--link", required=True)
    ap.add_argument("--percent", type=int)
    ap.add_argument("--arg", action="append", default=[])
    ap.add_argument("--warm", type=float, default=90)
    ap.add_argument("--window", type=float, default=60)
    ap.add_argument("--windows", type=int, default=3)
    ap.add_argument("--label", default="")
    ap.add_argument("--no-sample", action="store_true")
    ap.add_argument("--allow-locked", action="store_true")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    if a.percent is not None and not 1 <= a.percent <= 100:
        ap.error("--percent is the engine's percent, 1 to 100")
    cli = a.cli or sorted(glob.glob(f"{HOME}/Library/Application Support/orbiters-desktop/obelisk-cli/obelisk-cli-*-arm64/obelisk-cli"))[-1]
    started_wall = time.time()
    lines, vlines, events = [], [], []
    command = [cli, "steam", "play", "438100"] + (["--render-percent", str(a.percent)] if a.percent else []) + ["--", "-screen-fullscreen", "1", "-window-mode", "borderless"] + a.arg + [a.link]
    res = {
        "label": a.label,
        "when": datetime.datetime.now().isoformat(timespec="seconds"),
        "command": command[1:-1] + ["<link>"],
        "power": {"pmset_custom": run(["/usr/bin/pmset", "-g", "custom"]), "batt": run(["/usr/bin/pmset", "-g", "batt"]).splitlines()[:2], "thermal_start": run(["/usr/bin/pmset", "-g", "therm"])},
        "not_measured": ["GPU frequency and command-buffer start/end (powermetrics and Metal counters need root or the game's process)", "queue depth", "host-thread runnable and wake-up intervals"],
        "outcome": "not started",
        "windows": [],
    }
    try:
        st = os.stat(ENGINE_LOG)
        old_log = (st.st_ino, st.st_dev)
    except FileNotFoundError:
        old_log = None
    proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    session = {"id": None, "running_at": None}
    state = {"gpu": [], "cpu": [], "lock": []}
    stop = threading.Event()
    eng = vrc = None

    def read_events():
        for line in proc.stdout:
            try:
                e = json.loads(line)
            except Exception:  # noqa: BLE001
                continue
            events.append((now(), e))
            if e.get("event") == "running" and session["id"] is None:
                session["id"], session["running_at"] = e.get("session_id"), now()
    threading.Thread(target=read_events, daemon=True).start()
    t_start = now()
    try:
        # The game's own session: running (with its id), an error or an exit, within a deadline, the program alive.
        while session["id"] is None and now() - t_start < START_DEADLINE:
            if any(e.get("event") in ("error", "exited") for _, e in events) or proc.poll() is not None:
                break
            time.sleep(0.5)
        if session["id"] is None:
            res["outcome"] = "did not start: " + json.dumps([e for _, e in events if e.get("event") in ("error", "exited")])[:400]
            return 2
        res["session"] = session["id"]

        def engine_log():
            try:
                st = os.stat(ENGINE_LOG)
            except FileNotFoundError:
                return None
            return ENGINE_LOG if (st.st_ino, st.st_dev) != old_log else None
        eng = Follow(engine_log, lambda t, l: lines.append((t, l)))
        vrc = Follow(lambda: own_vrchat_log(started_wall, a.link), lambda t, l: vlines.append((t, l)))
        eng.start(); vrc.start()
        threading.Thread(target=sampler, args=(state, stop), daemon=True).start()
        # Arrival in this start's own log.
        arrived = None
        while arrived is None and now() - session["running_at"] < ARRIVE_DEADLINE and proc.poll() is None:
            arrived = next((t for t, l in list(vlines) if "OnPlayerJoinComplete" in l), None)
            time.sleep(1)
        res["arrived_after_s"] = round(arrived - t_start, 1) if arrived else None
        if arrived is None:
            res["outcome"] = "did not arrive (no OnPlayerJoinComplete in this start's own log): nothing scored"
            return 3
        # Settled: the warm time, then 20 s without a frame held over 250 ms (deadline: warm + 180 s).
        while proc.poll() is None and now() - arrived < a.warm + 180:
            if now() - arrived >= a.warm:
                recent = [float(m.group(1)) for t, l in list(lines)[-400:] if t > now() - 20 for m in [HELD.search(l)] if m]
                if not any(x > 250 for x in recent):
                    break
            time.sleep(1)
        windows = []
        for _ in range(a.windows):
            if proc.poll() is not None:
                break
            t0 = now()
            time.sleep(a.window)
            windows.append((t0, now()))
        res["windows"] = [summarize(list(lines), t0, t1, state, a.allow_locked) | {"from_s": round(t0 - t_start, 1)} for t0, t1 in windows]
        res["outcome"] = "measured" if windows else "the game ended before any window"
        # Unscored, after the windows: 3 s stack samples of the render server and the game.
        samples = {}
        if not a.no_sample and proc.poll() is None:
            procs = [l.split(None, 3) for l in run(["/bin/ps", "-A", "-o", "pid=,rss=,%cpu=,comm="]).splitlines() if l.strip()]
            for name, ends, key in (("render-server", "virgl_render_server", 2), ("game", "fexdarwin-run", 1)):
                group = [p for p in procs if len(p) == 4 and p[3].endswith(ends)]
                if group:
                    out = run(["/usr/bin/sample", max(group, key=lambda p: float(p[key]))[0], "3", "-mayDie"], 60)
                    samples[name] = out[out.find("Call graph:"):][:200000]
        res["samples"] = samples
        return 0
    finally:
        # Always: quit this session gracefully (never forced) while it runs, and write what happened.
        if session["id"] and proc.poll() is None:
            res["quit"] = run([cli, "steam", "quit", "438100", "--session", session["id"], "--wait", "60"], 90)
        try:
            proc.wait(timeout=120)
        except subprocess.TimeoutExpired:
            res["outcome"] += "; the program had not ended 120 s after the quit"
        stop.set()
        for f in (eng, vrc):
            if f:
                f.stop = True
        res["power"]["thermal_end"] = run(["/usr/bin/pmset", "-g", "therm"])
        snapshot = list(lines)
        res["swapchains"] = [{"at_s": round(t - t_start, 1), "size": f"{m.group(1)}x{m.group(2)}", "fmt": m.group(3), "buffers": m.group(4)} for t, l in snapshot for m in [SWAP.search(l)] if m]
        res["render"] = [m.groups() for _, l in snapshot for m in [RENDER.search(l)] if m][:1]
        res["engine_trace"] = [l for _, l in snapshot if STOP.search(l)][:40]
        res["events"] = [e for _, e in events]
        with open(a.out, "w") as f:
            json.dump(res, f, indent=1)
        for w in res["windows"]:
            print(("valid " if w["valid"] else "INVALID ") + "; ".join(w["invalid_because"]), "| submitted/s", w["guest_present"]["submitted_per_second_counted"], "interval", w["guest_present"]["interval_ms_median"], "shown/s", w["wsd_shown"]["per_second"])
        print("outcome:", res["outcome"])


if __name__ == "__main__":
    sys.exit(main())
