"""Runs of docs/igpu-experiment.md one after the other, each through run.py (whose summary line it prints; the files
of every run land in <out>/<label>/). The report's counter table comes from `window`, the 3D view's frame rates from
`sizes` and `large`; the loads' changes come from presence.py, whose blocks replaced the `load` and `ab*` parts here
(what the computer does in the background drifts more than those runs could tell apart).

    python matrix.py <out> [part ...]      parts, in the order given: window, ab, ab-overlay, ab-cpu, publish, sizes,
                                           large (those by default), load

It waits for the computer to be free (no VRChat or SteamVR, no video playing, nobody at it for five minutes): the load
runs make the graphics card busy on purpose. presence.py uses the same wait.
"""

import ctypes
import os
import subprocess
import sys
import time

from run import LAB

HERE = os.path.dirname(os.path.abspath(__file__))
GPUS = ["integrated", "graphics-card"]
# Runs only while nobody plays or uses the computer: no game or VR, and no keyboard or mouse for this long.
BUSY = ["VRChat.exe", "vrserver.exe", "vrcompositor.exe"]
AWAY = float(os.environ.get("GPU_LAB_AWAY", "300"))


def idle_seconds():
    class LASTINPUTINFO(ctypes.Structure):
        _fields_ = [("cbSize", ctypes.c_uint), ("dwTime", ctypes.c_uint)]

    info = LASTINPUTINFO(ctypes.sizeof(LASTINPUTINFO), 0)
    ctypes.windll.user32.GetLastInputInfo(ctypes.byref(info))
    return ((ctypes.windll.kernel32.GetTickCount() - info.dwTime) & 0xFFFFFFFF) / 1000.0


def media():
    """What two seconds of GPU counters say someone may be watching: Virtual Desktop streaming to a headset (its
    video encoder works), a video playing (any video decoder works)."""
    lab = LAB
    out = subprocess.run([lab, "sample", "2", "--every", "1000"], capture_output=True, text=True, errors="replace").stdout
    found = set()
    for line in out.splitlines():
        parts = line.split()
        if len(parts) < 4 or not parts[0].replace(".", "", 1).isdigit() or not parts[1].isdigit():
            continue
        value, engine = float(parts[0]), parts[-1]
        if "VirtualDesktop.Streamer.exe" in parts and engine == "VideoEncode" and value > 1.0:
            found.add("Virtual Desktop streaming")
        if engine == "VideoDecode" and value > 1.0:
            found.add(f"a video playing ({parts[2]})")
    return sorted(found)


def playing():
    out = subprocess.run(["tasklist", "/FO", "CSV", "/NH"], capture_output=True, text=True, errors="replace").stdout
    return sorted({n for n in BUSY if f'"{n}"' in out}) + media()


def wait_until_free():
    said = False
    while True:
        busy, idle = playing(), idle_seconds()
        if not busy and idle >= AWAY:
            if said:
                print(f"free again (idle {idle:.0f} s)", flush=True)
            return
        if not said:
            print(f"waiting: {', '.join(busy) or 'someone at the computer'} (idle {idle:.0f} s)", flush=True)
            said = True
        time.sleep(30)


def run(out, label, seconds=30, **kw):
    wait_until_free()
    cmd = [sys.executable, os.path.join(HERE, "run.py"), label, "--out", out, "--seconds", str(seconds)]
    for k, v in kw.items():
        cmd += [f"--{k}", v]
    r = subprocess.run(cmd, capture_output=True, text=True)
    line = (r.stdout.strip().splitlines() or [""])[-1]
    print(line if r.returncode == 0 else f"{label}: failed: {r.stdout[-400:]} {r.stderr[-400:]}", flush=True)


def calibrate(target_ms=8.0):
    """The load's work for about `target_ms` of GPU time a frame on the graphics card (VRChat's frame at 90 Hz in VR,
    with a little room)."""
    wait_until_free()
    lab = LAB
    work = 256
    for _ in range(3):
        out = subprocess.run([lab, "load", "nvidia", "6", "--work", str(work)], capture_output=True, text=True).stdout
        line = next((l for l in out.splitlines() if l.startswith("summary:")), "")
        ms = float(line.split("GPU ms mean ")[1].split()[0])
        if abs(ms - target_ms) < 0.5:
            break
        work = max(16, round(work * target_ms / ms / 16) * 16)
    return work


PERIOD = 2  # seconds a window or an overlay moves, then rests, in the A/B runs


def parts(out, load, reps):
    """The parts, by name: each a list of runs."""

    def window():
        run(out, "baseline-1")
        for gpu in GPUS:
            for scenario in ["idle", "idle --publish 30", "live", "scroll", "pages", "splats-still --publish 30", "splats", "splats --session Canine"]:
                run(out, f"window-{gpu}-" + scenario.replace(" --", "-").replace(" ", "-"), window=f"{scenario} --gpu {gpu}")
        run(out, "baseline-2")

    def load_runs():
        # The load beside each case, one run after the other: what else the computer does in between weighs in too
        # (the A/B runs below replaced these).
        for rep in reps:
            run(out, f"load-baseline-{rep}", load=load)
            for gpu in GPUS:
                run(out, f"load-scroll-{gpu}-{rep}", load=load, window=f"scroll --gpu {gpu}")
                run(out, f"load-splats-{gpu}-{rep}", load=load, window=f"splats --gpu {gpu}")

    def large():
        for gpu in GPUS:
            run(out, f"large-splats-{gpu}", window=f"splats --gpu {gpu} --size 2560x1440")
            run(out, f"large-scroll-{gpu}", window=f"scroll --gpu {gpu} --size 2560x1440")

    def ab():
        # The window moves PERIOD seconds, rests as long, in turn, beside the load: each run compares the load's frames
        # in both, so that what else the computer does meanwhile weighs on both alike.
        cases = [
            ("scroll", "scroll", GPUS, {}),
            ("splats", "splats", GPUS, {}),
            ("large-scroll", "scroll --size 2560x1440", GPUS, {}),
            # The swapchain made from a DirectComposition visual instead of the window's HWND (wgpu's DxgiFromVisual).
            ("visual-scroll", "scroll", ["integrated"], {"WGPU_DX12_PRESENTATION_SYSTEM": "Visual"}),
        ]
        for rep in reps:
            for name, scenario, gpus, env in cases:
                for gpu in gpus:
                    os.environ.update(env)
                    run(out, f"ab-{name}-{gpu}-{rep}", seconds=60, load=load, window=f"{scenario} --gpu {gpu} --alternate {PERIOD}")
                    for k in env:
                        del os.environ[k]

    def ab_overlay():
        for rep in reps:
            run(out, f"ab-overlay-full-{rep}", seconds=60, load=load, extra=f"overlay nvidia 80 --rate 45 --alternate {PERIOD}")
            run(out, f"ab-overlay-button-{rep}", seconds=60, load=load, extra=f"overlay nvidia 80 --rate 45 --rect 400,300,640,360 --alternate {PERIOD}")

    def ab_cpu():
        # The 3D view orbiting and resting in turn beside one processor thread.
        for rep in reps:
            for kind in ["alu", "mem"]:
                for gpu in GPUS:
                    run(out, f"ab-cpu-{kind}-{gpu}-{rep}", seconds=60, extra=f"cpu {kind} 56", window=f"splats --gpu {gpu} --alternate {PERIOD}")

    def publish():
        # The service's news 30 times a second (one frame each since anim::request_frame; two before).
        for gpu in GPUS:
            run(out, f"publish-one-frame-{gpu}", window=f"idle --publish 30 --gpu {gpu}")

    def sizes():
        # How the 3D view's frame rate on the integrated GPU follows its size (pixels).
        for size in ["640x410", "960x615", "1920x1230"]:
            for session in ["New Generation", "Canine"]:
                run(out, f"sizes-splats-{session.split()[0]}-{size}", window=f'splats --gpu integrated --size {size} --session "{session}"')

    return {"window": window, "load": load_runs, "large": large, "ab": ab, "ab-overlay": ab_overlay, "ab-cpu": ab_cpu, "publish": publish, "sizes": sizes}


def main():
    out = sys.argv[1]
    names = sys.argv[2:] or ["window", "ab", "ab-overlay", "ab-cpu", "publish", "sizes", "large"]
    reps = [int(r) for r in os.environ.get("GPU_LAB_REPS", "0,1,2,3").split(",")]
    load = os.environ.get("GPU_LAB_LOAD")
    if not load and any(n in names for n in ("load", "ab", "ab-overlay")):
        load = f"nvidia 600 --work {calibrate()}"
        print(f"load: {load}", flush=True)
    table = parts(out, load, reps)
    for name in names:
        table[name]()


if __name__ == "__main__":
    main()
