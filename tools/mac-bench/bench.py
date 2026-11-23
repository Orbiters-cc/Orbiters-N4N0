#!/usr/bin/env python3
"""One benchmark run of VRChat on a Mac through obelisk-cli (docs/mac-performance.md, the procedure).

Starts VRChat with `obelisk-cli steam play 438100 [--render-percent N] -- <arguments> <link>` in a private instance of a
fixed world, waits until the local player has arrived and loading and shader compilation have settled, then records equal
timed windows with nobody touching the game (a fixed camera: the player stands where it spawned), quits by session, and
writes one results file. Nothing here sends input to the game, changes a setting or injects anything: it reads the
engine's log, VRChat's log, `ps` and `ioreg`.

The three frame counters are kept apart (R3), per swapchain generation:
  - guest Present: Neptune's nptunix line, `<n> presents (<m> shown events), last 60: interval avg X ms, fence wait avg
    Y ms` (X: time between the game's submissions; Y: how long each waited for the host to finish it);
  - worker shown events: the `(<m> shown events)` of the same line (counts zero-time callbacks too);
  - wsd's positive Metal presentation: the Metal HUD's per-frame intervals (`metal-HUD:` lines, raw) and wsd's
    `120 frames shown, present interval ...` batches.
A new generation starts at each `guest swapchain: created` line. Raw intervals are kept, so sessions merge.

usage: bench.py --link 'vrchat://launch?...' [--percent N] [--arg A]... [--warm 90] [--window 60] [--windows 3]
                [--cli PATH] --out results.json
"""
import argparse, datetime, glob, json, os, re, signal, statistics, subprocess, sys, threading, time

HOME = os.path.expanduser("~")
ENGINE_LOG = f"{HOME}/Library/Logs/Obelisk/Steam/wine-438100.log"
LIBRARY = f"{HOME}/Library/Application Support/cc.orbiters.obelisk-cli"
VRC_LOGS = f"{LIBRARY}/Prefixes/steam-438100-windows-linux/prefix/drive_c/users/crossover/AppData/LocalLow/VRChat/VRChat"

NPT = re.compile(r"neptune unixlib wsi \(wsd\): (\d+) presents \((\d+) shown events\), last 60: interval avg ([\d.]+) ms, fence wait avg ([\d.]+) ms, pacing wait avg ([\d.]+) ms")
WSD = re.compile(r"wsd: window \d+ overlay: (\d+) frames shown, present interval avg ([\d.]+) min ([\d.]+) max ([\d.]+) ms, latency submit->shown ([\d.]+) ms")
HELD = re.compile(r"wsd: window \d+ overlay: frame held ([\d.]+) ms")
HUD = re.compile(r"metal-HUD: (.*)$")
SWAP = re.compile(r"guest swapchain: created (\d+)x(\d+) fmt=(\d+) buffers=(\d+)")
RENDER = re.compile(r"wsd: render percent: requested (\S+), applied (\d+).*\(desktop (\d+)x(\d+) px\)")
STOP = re.compile(r"lkd: sig: |lkd: ptrace: ")


def now():
    return time.monotonic()


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


def newest_vrchat_log(since):
    logs = [p for p in glob.glob(f"{VRC_LOGS}/output_log_*.txt") if os.path.getmtime(p) >= since - 2]
    return max(logs, key=os.path.getmtime) if logs else None


def sampler(state, stop):
    """Once a second: GPU utilization (ioreg), and CPU of the engine's processes' busiest threads (ps -M)."""
    while not stop.is_set():
        t = now()
        try:
            out = subprocess.run(["/usr/sbin/ioreg", "-r", "-d", "1", "-c", "IOAccelerator"], capture_output=True, text=True, timeout=5).stdout
            m = re.search(r'"Device Utilization %"=(\d+)', out)
            r = re.search(r'"Renderer Utilization %"=(\d+)', out)
            state["gpu"].append((t, int(m.group(1)) if m else None, int(r.group(1)) if r else None))
        except Exception:
            pass
        try:
            # The engine's processes, then their threads' CPU (ps -M: a line per process, then one per thread whose first
            # two fields are the pid and the thread's %CPU).
            listing = subprocess.run(["/bin/ps", "-A", "-o", "pid=,comm="], capture_output=True, text=True, timeout=5).stdout
            names = {}
            for line in listing.splitlines():
                parts = line.strip().split(None, 1)
                if len(parts) == 2 and parts[1].rsplit("/", 1)[-1] in ("fexdarwin-run", "virgl_render_server", "wsd"):
                    names[parts[0]] = parts[1].rsplit("/", 1)[-1]
            snap = {}
            if names:
                out = subprocess.run(["/bin/ps", "-M", "-p", ",".join(names)], capture_output=True, text=True, timeout=5).stdout
                for line in out.splitlines()[1:]:
                    tok = line.split()
                    if len(tok) >= 2 and tok[0].isdigit() and tok[0] in names:
                        try:
                            snap.setdefault(tok[0], []).append(float(tok[1]))
                        except ValueError:
                            pass
                    elif len(tok) >= 4 and tok[1] in names:
                        try:
                            snap.setdefault(tok[1], []).append(float(tok[3]))
                        except ValueError:
                            pass
            state["cpu"].append((t, {p: {"name": names[p], "top": sorted(v, reverse=True)[:4], "sum": round(sum(v), 1)} for p, v in snap.items()}))
        except Exception:
            pass
        stop.wait(1.0)


def pct(xs, q):
    if not xs:
        return None
    xs = sorted(xs)
    k = (len(xs) - 1) * q
    lo, hi = int(k), min(int(k) + 1, len(xs) - 1)
    return round(xs[lo] + (xs[hi] - xs[lo]) * (k - lo), 3)


def summarize(lines, t0, t1, gpu, cpu):
    win = [(t, l) for t, l in lines if t0 <= t < t1]
    npt = [NPT.search(l) for _, l in win]
    npt = [(int(m.group(1)), int(m.group(2)), float(m.group(3)), float(m.group(4))) for m in npt if m]
    wsd = [WSD.search(l) for _, l in win]
    wsd = [tuple(float(x) for x in m.groups()) for m in wsd if m]
    held = [float(m.group(1)) for m in (HELD.search(l) for _, l in win) if m]
    hud = []
    for _, l in win:
        m = HUD.search(l)
        if m:
            vals = m.group(1).split(",")[3:]
            hud += [float(v) for v in vals[0::2] if re.fullmatch(r"[\d.]+", v) and float(v) > 0]
    span = t1 - t0
    presents = (npt[-1][0] - npt[0][0]) if len(npt) >= 2 else None
    shown_events = (npt[-1][1] - npt[0][1]) if len(npt) >= 2 else None
    g = [x[1] for x in gpu if t0 <= x[0] < t1 and x[1] is not None]
    rs = {}
    for t, snap in cpu:
        if t0 <= t < t1:
            for pid, v in snap.items():
                rs.setdefault(v["name"] + ":" + pid, []).append(v["sum"])
    return {
        "seconds": round(span, 1),
        "guest_present": {
            "batches_of_60": len(npt),
            "interval_ms_per_batch": [x[2] for x in npt],
            "interval_ms_median": pct([x[2] for x in npt], 0.5),
            "interval_ms_p10_p90": [pct([x[2] for x in npt], 0.1), pct([x[2] for x in npt], 0.9)],
            "fence_wait_ms_per_batch": [x[3] for x in npt],
            "fence_wait_ms_median": pct([x[3] for x in npt], 0.5),
            "submitted_per_second": round(presents / span, 2) if presents else None,
        },
        "worker_shown_events_per_second": round(shown_events / span, 2) if shown_events else None,
        "wsd_shown": {
            "hud_intervals_ms": hud,
            "frames": len(hud),
            "per_second": round(len(hud) / span, 2) if hud else None,
            "interval_ms_p50_p90_p99": [pct(hud, 0.5), pct(hud, 0.9), pct(hud, 0.99)],
            "share_at_16_7": round(sum(1 for x in hud if x < 20) / len(hud), 3) if hud else None,
            "share_at_33_3": round(sum(1 for x in hud if 30 < x < 37) / len(hud), 3) if hud else None,
            "batches": [{"frames": int(w[0]), "avg": w[1], "min": w[2], "max": w[3], "latency": w[4]} for w in wsd],
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
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    cli = a.cli or sorted(glob.glob(f"{HOME}/Library/Application Support/orbiters-desktop/obelisk-cli/obelisk-cli-*-arm64/obelisk-cli"))[-1]
    started_wall = time.time()
    lines, vlines, events = [], [], []
    command = [cli, "steam", "play", "438100"] + (["--render-percent", str(a.percent)] if a.percent else []) + ["--", "-screen-fullscreen", "1", "-window-mode", "borderless"] + a.arg + [a.link]
    power = subprocess.run(["/usr/bin/pmset", "-g", "custom"], capture_output=True, text=True).stdout
    batt = subprocess.run(["/usr/bin/pmset", "-g", "batt"], capture_output=True, text=True).stdout
    therm0 = subprocess.run(["/usr/bin/pmset", "-g", "therm"], capture_output=True, text=True).stdout
    try:
        st = os.stat(ENGINE_LOG)
        old_log = (st.st_ino, st.st_dev)
    except FileNotFoundError:
        old_log = None
    proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    session = {"id": None}

    def read_events():
        for line in proc.stdout:
            try:
                e = json.loads(line)
            except Exception:
                continue
            events.append((now(), e))
            if e.get("event") == "running":
                session["id"] = e.get("session_id")
    threading.Thread(target=read_events, daemon=True).start()
    # The engine's log is new at each launch: follow it once the game's supervisor holds the game.
    while not any(e.get("event") in ("starting", "error", "exited") for _, e in events):
        time.sleep(0.5)
    if any(e.get("event") == "error" for _, e in events):
        print("start failed:", [e for _, e in events if e.get("event") == "error"])
        return 2
    # The launch writes a new log (the last one renamed to .previous): follow the file that was not there before this run.
    def engine_log():
        try:
            st = os.stat(ENGINE_LOG)
        except FileNotFoundError:
            return None
        return ENGINE_LOG if (st.st_ino, st.st_dev) != old_log else None
    eng = Follow(engine_log, lambda t, l: lines.append((t, l)))
    vrc = Follow(lambda: newest_vrchat_log(started_wall), lambda t, l: vlines.append((t, l)))
    eng.start(); vrc.start()
    state = {"gpu": [], "cpu": []}
    stop = threading.Event()
    threading.Thread(target=sampler, args=(state, stop), daemon=True).start()
    # Arrival: the local player in the world.
    t_start = now()
    arrived = None
    while arrived is None and now() - t_start < 600 and proc.poll() is None:
        for t, l in vlines:
            if "OnPlayerJoinComplete" in l or "Finished entering world" in l:
                arrived = t
                break
        time.sleep(1)
    if arrived is None:
        print("never arrived")
    # Settled: the warm time, then 20 s without a frame held over 250 ms.
    while arrived is not None and proc.poll() is None:
        if now() - arrived >= a.warm:
            recent = [float(m.group(1)) for t, l in lines[-400:] if t > now() - 20 for m in [HELD.search(l)] if m]
            if not any(x > 250 for x in recent):
                break
        time.sleep(1)
    windows = []
    for i in range(a.windows):
        if proc.poll() is not None:
            break
        t0 = now()
        time.sleep(a.window)
        windows.append((t0, now()))
    therm1 = subprocess.run(["/usr/bin/pmset", "-g", "therm"], capture_output=True, text=True).stdout
    locked = "CGSSessionScreenIsLocked" in subprocess.run(["/usr/sbin/ioreg", "-n", "Root", "-d1"], capture_output=True, text=True).stdout and '"CGSSessionScreenIsLocked"=Yes' in subprocess.run(["/usr/sbin/ioreg", "-n", "Root", "-d1"], capture_output=True, text=True).stdout
    # Unscored, after the windows: 3 s stack samples of the render server's busiest process and of the game (where the
    # host's time goes: Neptune's decoder, DXMT's encoding, waits for the GPU).
    samples = {}
    if not a.no_sample and proc.poll() is None:
        listing = subprocess.run(["/bin/ps", "-A", "-o", "pid=,rss=,%cpu=,comm="], capture_output=True, text=True).stdout
        procs = [l.split(None, 3) for l in listing.splitlines() if l.strip()]
        rs = [p for p in procs if len(p) == 4 and p[3].endswith("virgl_render_server")]
        game = [p for p in procs if len(p) == 4 and p[3].endswith("fexdarwin-run")]
        for name, group, key in (("render-server", rs, 2), ("game", game, 1)):
            if group:
                pid = max(group, key=lambda p: float(p[key]))[0]
                out = subprocess.run(["/usr/bin/sample", pid, "3", "-mayDie"], capture_output=True, text=True, timeout=60).stdout
                samples[name] = out[out.find("Call graph:"):][:200000]
    # Quit by session, never forced.
    quit = None
    if session["id"] and proc.poll() is None:
        quit = subprocess.run([cli, "steam", "quit", "438100", "--session", session["id"], "--wait", "60"], capture_output=True, text=True).stdout
    try:
        proc.wait(timeout=120)
    except subprocess.TimeoutExpired:
        pass
    stop.set()
    eng.stop = vrc.stop = True
    swaps = [(t, m.groups()) for t, l in lines for m in [SWAP.search(l)] if m]
    render = [m.groups() for _, l in lines for m in [RENDER.search(l)] if m]
    res = {
        "label": a.label,
        "when": datetime.datetime.now().isoformat(timespec="seconds"),
        "command": command[1:-1] + ["<link>"],
        "render": render[:1],
        "power": {"pmset_custom": power, "batt": batt.splitlines()[:1], "thermal_start": therm0, "thermal_end": therm1},
        "arrived_after_s": round(arrived - t_start, 1) if arrived else None,
        "swapchains": [{"at_s": round(t - t_start, 1), "size": f"{g[0]}x{g[1]}", "fmt": g[2], "buffers": g[3]} for t, g in swaps],
        "windows": [summarize(lines, t0, t1, state["gpu"], state["cpu"]) | {"from_s": round(t0 - t_start, 1)} for t0, t1 in windows],
        "screen_locked": locked,
        "engine_trace": [l for _, l in lines if STOP.search(l)][:40],
        "ptrace_attaches_at_least": max([int(m.group(1)) for _, l in lines for m in [re.search(r"lkd: ptrace: attach (\d+):", l)] if m] or [0]),
        "samples": samples,
        "events": [e for _, e in events],
        "quit": quit,
    }
    with open(a.out, "w") as f:
        json.dump(res, f, indent=1)
    for w in res["windows"]:
        print(json.dumps({k: w[k] for k in ("seconds", "worker_shown_events_per_second")}), "guest", w["guest_present"]["interval_ms_median"], "fence", w["guest_present"]["fence_wait_ms_median"], "shown/s", w["wsd_shown"]["per_second"], "p50/90/99", w["wsd_shown"]["interval_ms_p50_p90_p99"], "gpu", w["gpu_device_utilization_pct"]["median"])
    return 0


if __name__ == "__main__":
    sys.exit(main())
