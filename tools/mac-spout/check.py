#!/usr/bin/env python3
"""VRChat's stream camera on a Mac, without VRChat: an owned Spout sender on the engine's own Direct3D, read by the app's own capture worker.

    tools/mac-spout/check.py --server <virgl_render_server> --sender <spout_sender.exe> --home <a Wine prefix's home, written to> [--app <folder of orbiters-capture>] [--keep]

The sender is Neptune's `virglrenderer/tests/neptune/publish/spout_sender.c` (its `run.py` is where this runner comes from); `--server` a render server that publishes shared
textures (obelisk-cli 0.1.15's, or a developer generation's). It reads the installed engine's files in place, under the library's shared lock, starts no window server and no
Steam, shows nothing on the screen, and ends what it started by its own process groups.
"""
import argparse, fcntl, json, os, pathlib, re, shutil, signal, subprocess, sys, tempfile, time

REPO = pathlib.Path(__file__).resolve().parent.parent.parent
p = argparse.ArgumentParser()
p.add_argument("--server", required=True)
p.add_argument("--sender", required=True)
p.add_argument("--home", required=True)
p.add_argument("--app", default=str(REPO / "target/debug"))
p.add_argument("--keep", action="store_true")
a = p.parse_args()
runtimes = sorted((pathlib.Path.home() / "Library/Application Support/cc.orbiters.obelisk-cli/Voile").glob("*/linux"))
if not runtimes:
    sys.exit("no engine here")
runtime, home, sender = runtimes[-1], pathlib.Path(a.home).resolve(), pathlib.Path(a.sender).resolve()
lock = (runtime.parent.parent.parent / "Voile.locks/library.lock").open("rb")
fcntl.flock(lock, fcntl.LOCK_SH)
capture = str(pathlib.Path(a.app) / "orbiters-capture")
tmp = pathlib.Path(tempfile.mkdtemp(prefix="msp.", dir=os.environ.get("TMPDIR", "/tmp")))  # short: a socket's path has 104 bytes
host = tmp / "h"
for d in ("s", "h", "h/shared", "t", "m", "x", "out"):
    (tmp / d).mkdir(mode=0o700)
started, failed = [], []
env_app = dict(os.environ, LKD_HOST_TMP=str(tmp / "t"))

def ok(name, good, why=""):
    print(("PASS  " if good else "FAIL  ") + name + (f"  ({why})" if why and not good else ""), flush=True)
    if not good:
        failed.append(name)
    return good

def spawn(args, **kw):
    q = subprocess.Popen(args, start_new_session=True, **kw)
    started.append(q)
    return q

def end(q):
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(q.pid, sig)
        except ProcessLookupError:
            break
        try:
            q.wait(3)
        except subprocess.TimeoutExpired:
            continue
        time.sleep(0.2)
    q.poll()

def server():
    dxmt = runtime / "graphics/dxmt-native.framework/Versions/A/dxmt-native"
    env = dict(os.environ, RENDER_SERVER_EXEC_PATH=a.server, NPT_BACKEND="dxmt", NPT_SHARED_PUBLISH="../h/shared", **{f"NPT_{k}_LIBRARY_PATH": str(dxmt) for k in ("D3D11", "DXGI", "D3D12")})
    q = spawn([str(runtime / "graphics/bin/virgl_test_server"), "--no-virgl", "--no-fork", "--socket-path", "vtest.sock"], cwd=tmp / "s", env=env, stdout=open(tmp / "server.log", "ab"), stderr=subprocess.STDOUT)
    for _ in range(100):
        if (tmp / "s/vtest.sock").exists():
            return q
        time.sleep(0.05)
    sys.exit("the render server did not start: " + (tmp / "server.log").read_text(errors="replace")[-600:])

def guest(*lines):
    genv = dict(PATH="/usr/bin:/bin", HOME=str(home), TMPDIR=str(tmp / "x"), FEXDARWIN_LKD="1", FEXDARWIN_ROOT=str(runtime / "root"), FEXDARWIN_ROOT_RO="1",
                LKD_HOST_TMP=str(tmp / "t"), LKD_HOST_SHM=str(tmp / "m"), FEXDARWIN_SUPERVISOR_PID=str(os.getpid()),
                FEXDARWIN_BIND=f"/opt/wine={runtime}/wine:ro;/home/user={home};/w={sender.parent}:ro;/run/obelisk={tmp}/s")
    args = [str(runtime / "engine/run-with-timeout.sh"), "240", str(runtime / "engine/fexdarwin-run")]
    for kv in ["HOME=/home/user", "USER=user", "LOGNAME=user", "PATH=/opt/wine/bin:/usr/bin:/bin", "LANG=en_US.UTF-8", "WINEPREFIX=/home/user/prefix", "WINEARCH=win64", "WINEDEBUG=-all",
               "WINEDLLOVERRIDES=mscoree,mshtml=;winemenubuilder.exe=d;d3d11,dxgi,d3d12=n", "VTEST_SOCKET_NAME=/run/obelisk/vtest.sock", "NPT_DEBUG=force_vtest"]:
        args += ["--env", kv]
    script = "/opt/wine/bin/wineserver -p0; " + "; ".join(f"/opt/wine/bin/wine /w/{sender.name} {line}" for line in lines) + "; /opt/wine/bin/wineserver -k"
    return spawn(args + ["/bin/sh", "-c", script], env=genv, stdout=open(tmp / "guest.log", "ab"), stderr=subprocess.STDOUT)

def listed():
    out = subprocess.run([capture, "list-sources", "--host", str(host)], env=env_app, capture_output=True, text=True, timeout=20).stdout
    return [l for l in out.splitlines() if "DXGI" in l]

def wait_listed(seconds, what):
    until = time.time() + seconds
    while time.time() < until:
        got = listed()
        if what(got):
            return got
        time.sleep(0.3)
    return None

def record(name, seconds, source="spout"):
    """One recording by the app's worker: what it says, and the processor time it took."""
    began, t0 = time.time(), os.times()
    q = subprocess.run([capture, "record", "--source", source, "--host", str(host), "--out", str(tmp / "out"), "--seconds", str(seconds), "--name", name], env=env_app, capture_output=True, text=True, timeout=seconds + 60)
    t1 = os.times()
    try:
        said = json.loads(q.stdout)
    except ValueError:
        said = {"stats": {}, "reason": (q.stdout + q.stderr)[-400:]}
    said["cpu"] = round(100 * (t1.children_user + t1.children_system - t0.children_user - t0.children_system) / max(time.time() - began, 0.1), 1)
    said["err"] = q.stderr[-600:]
    return said

def publishing():
    log = (tmp / "server.log").read_text(errors="replace")
    return len(re.findall(r"publish: reader \d+\n", log)), len(re.findall(r"publish: reader \d+ left", log))

try:
    for name in ("d3d11.dll", "dxgi.dll", "d3d12.dll"):
        shutil.copyfile(runtime / "windows" / name, home / "prefix/drive_c/windows/system32" / name)
    shutil.copyfile(runtime / "wine/lib/wine/x86_64-windows/nptunix.dll", home / "prefix/drive_c/windows/system32/nptunix.dll")
    ok("with no game, there is no sender", listed() == [])
    server()
    # A camera opened (1920 x 1080), closed, then opened again at another size, as VRChat's is.
    g = guest("VRCSender1 1920 1080 900 copy 16", "VRCSender1 1280 720 700 copy 16 2 400")
    got = wait_listed(60, lambda l: any("VRCSender1" in s for s in l))
    ok("the sender is listed by its own name, with its size", bool(got) and "1920x1080" in got[0], str(got))
    ok("found without connecting: the engine copies no picture for a list", publishing()[0] == 0, str(publishing()))
    r = record("first", 5)
    s = r["stats"]
    ok("the app's worker records it", s.get("frames", 0) > 150, str(r)[:500])
    print(f"      {s.get('frames')} frames at {s.get('fps', 0):.1f} fps; processor of the worker {r['cpu']} % of a core")
    time.sleep(0.5)
    ok("stopped, it reads no more: the engine copies nothing", publishing()[0] == publishing()[1] and publishing()[0] > 0, str(publishing()))
    r = record("second", 25)
    ok("the camera closed and opened again at another size: the recording says so", "1280x720" in str(r.get("reason")) or "size" in str(r.get("reason")), str(r)[:400])
    got = wait_listed(20, lambda l: any("1280x720" in s for s in l))
    ok("the new size is listed", bool(got), str(got))
    r = record("third", 4)
    ok("and recorded", r["stats"].get("frames", 0) > 60, str(r)[:500])
    ok("a name that is not there is waited for, and nothing else is read", record("none", 2, "spout:VRCSender9")["stats"].get("frames", 0) == 0)
    until = time.time() + 60
    while g.poll() is None and time.time() < until:
        time.sleep(0.3)
    ok("the game ended: nothing is listed", listed() == [], str(listed()))
    r = record("after", 2)
    ok("and a recording waits for it, with nothing read", r["stats"].get("frames", 0) == 0, str(r)[:300])
finally:
    for q in started[::-1]:
        end(q)
    log = (tmp / "server.log")
    for l in log.read_text(errors="replace").splitlines() if log.exists() else []:
        if "<Error>" in l:
            print("SERVER", l[:200])
    if a.keep:
        print("kept", tmp)
    else:
        shutil.rmtree(tmp, ignore_errors=True)
print(f"\n{'FAILED: ' + '; '.join(failed) if failed else 'all passed'}")
sys.exit(1 if failed else 0)
