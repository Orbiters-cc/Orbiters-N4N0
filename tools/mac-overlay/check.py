#!/usr/bin/env python3
"""The Mac's overlay (overlay/orbit/desktop_mac) over the engine's real window server, with no game, no Steam and no person.

    tools/mac-overlay/check.py [--render-percent N] [--force] [--pictures FOLDER] [--wsd PATH] [--no-channel] [--headers PATH] [--app PATH]

With a `wsd` that lets the pointer go where its game does (obelisk-cli 0.1.15), the parts are clicked with no key, as that engine's are, and the ring round the wheel is picked by
the mouse's movement; with one from before, or with --no-channel, by Left Option.

It opens a bundle of its own around the engine's `wsd`, has game.c show a full-screen window there as VRChat's is (holding the pointer, its cursor hidden, real frames of VRChat's
menus stretched by the render percent), and turns the app's own loop over it (`orbiters-desktop --orbit-lab desk`) with a keyboard made up here. It covers the screen for about a
minute: it starts only while nobody has used this Mac for two minutes (--force: anyway), and ends when somebody does. Screen Recording is needed of the program it is started from.
"""
import _thread, argparse, ctypes, glob, json, os, pathlib, plistlib, queue, re, shutil, signal, subprocess, sys, tempfile, threading, time

HERE = pathlib.Path(__file__).resolve().parent
REPO = HERE.parent.parent
TESTDATA = REPO / "crates/orbiters-desktop/src/overlay/orbit/testdata"

def idle():
    out = subprocess.run(["ioreg", "-c", "IOHIDSystem"], capture_output=True, text=True).stdout
    return int(re.search(r'"HIDIdleTime" = (\d+)', out).group(1)) / 1e9

def locked():
    """Whether the screen is locked: macOS then shows nothing of any window, and nothing here could be told."""
    out = subprocess.run(["ioreg", "-n", "Root", "-d1"], capture_output=True, text=True).stdout
    return re.search(r'"CGSSessionScreenIsLocked"=Yes', out) is not None

class Mouse:
    """Real clicks of the mouse, as the person's: macOS does more with one than with an event of our own (it raises the window clicked). Only where the pointer already is,
    which is over a window of ours."""
    class Point(ctypes.Structure):
        _fields_ = [("x", ctypes.c_double), ("y", ctypes.c_double)]
    def __init__(self):
        cg = self.cg = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
        cg.CGEventCreate.restype, cg.CGEventCreate.argtypes = ctypes.c_void_p, [ctypes.c_void_p]
        cg.CGEventGetLocation.restype, cg.CGEventGetLocation.argtypes = self.Point, [ctypes.c_void_p]
        cg.CGEventCreateMouseEvent.restype = ctypes.c_void_p
        cg.CGEventCreateMouseEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint32, self.Point, ctypes.c_uint32]
        cg.CGEventPost.argtypes = [ctypes.c_uint32, ctypes.c_void_p]
        cg.CGPreflightPostEventAccess.restype = ctypes.c_bool
        cg.CGEventSetIntegerValueField.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int64]
        cg.CGCursorIsVisible.restype = ctypes.c_bool
        self.allowed, self.at = cg.CGPreflightPostEventAccess(), 0.0
    def where(self):
        p = self.cg.CGEventGetLocation(self.cg.CGEventCreate(None))
        return p.x, p.y
    def move(self, x, y, dx=0, dy=0):
        """The mouse's own movement: to a place, or (where the game holds the pointer) by so much."""
        self.at = time.time()
        e = self.cg.CGEventCreateMouseEvent(None, 5, self.Point(x, y), 0)
        self.cg.CGEventSetIntegerValueField(e, 4, int(dx))
        self.cg.CGEventSetIntegerValueField(e, 5, int(dy))
        self.cg.CGEventPost(0, e)
    def push(self, dx, dy, seconds=0.12):
        """A stroke of the mouse while the game holds the pointer."""
        n = max(1, int(seconds / 0.008))
        x, y = self.where()
        done = [0, 0]
        for i in range(1, n + 1):
            step = [int(round(v * i / n)) - d for v, d in zip((dx, dy), done)]
            done = [d + s for d, s in zip(done, step)]
            self.move(x, y, *step)
            time.sleep(0.008)
    def post(self, kind):
        self.at = time.time()
        at = self.cg.CGEventGetLocation(self.cg.CGEventCreate(None))
        self.cg.CGEventPost(0, self.cg.CGEventCreateMouseEvent(None, kind, at, 0))

class Lines:
    """A program whose lines are kept as they come."""
    def __init__(self, args, **kw):
        self.p = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=kw.pop("stderr", subprocess.DEVNULL), text=True, bufsize=1, **kw)
        self.lines, self.new = [], queue.Queue()
        threading.Thread(target=self.read, daemon=True).start()
    def read(self):
        for line in self.p.stdout:
            line = line.rstrip("\n")
            self.lines.append((time.time(), line))
            self.new.put(line)
    def say(self, line):
        try:
            self.p.stdin.write(line + "\n"); self.p.stdin.flush()
        except (BrokenPipeError, ValueError):
            pass
    def wait(self, found, seconds, since=0):
        until = time.time() + seconds
        while True:
            for at, line in self.lines[since:]:
                got = found(line)
                if got:
                    return got
            if time.time() > until or self.p.poll() is not None and time.time() > until - seconds + 1:
                return None
            time.sleep(0.005)
    def end(self):
        if self.p.poll() is None:
            self.p.kill()
        self.p.wait()

class Check:
    def __init__(self, a):
        self.a, self.failed, self.notes = a, [], {}
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="orbo."))
        self.wsd_pid, self.game, self.desk = None, None, None
        self.app = None
        self.mouse = Mouse()

    def ok(self, name, good, why=""):
        print(("PASS  " if good else "FAIL  ") + name + (f"  ({why})" if why and not good else ""), flush=True)
        if not good:
            self.failed.append(name)
        return good

    def note(self, name, value):
        self.notes[name] = value
        print(f"      {name}: {value}", flush=True)

    # ---- the engine's window server and its one client
    def start_game(self):
        wsd = self.a.wsd or sorted(glob.glob(os.path.expanduser("~/Library/Application Support/cc.orbiters.obelisk-cli/Voile/*/linux/engine/wsd")))[-1]
        self.app = self.tmp / "OverlayCheck.app"
        (self.app / "Contents/MacOS").mkdir(parents=True)
        shutil.copy2(wsd, self.app / "Contents/MacOS/wsd")
        (self.app / "Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleName": "OverlayCheck", "CFBundleIdentifier": "cc.orbiters.overlay-check", "CFBundleExecutable": "wsd", "CFBundlePackageType": "APPL"}))
        self.dir = self.tmp / "wsd"
        self.dir.mkdir(mode=0o700)
        self.log = self.tmp / "wsd.log"
        # An engine that lets the pointer go, and says so: with the options the launcher gives it for VRChat.
        self.host = self.tmp / "h"
        self.host.mkdir(mode=0o700)
        frees = b"--free-unconfined-pointer" in pathlib.Path(wsd).read_bytes()
        self.free = frees and not self.a.no_channel
        self.lets_go, self.takes = ("pointer given back (focus lost)", "pointer held by the game (focus back") if frees else ("focus lost, relative mouse suspended", "relative mouse resumed")
        options = (["--free-unconfined-pointer"] if frees else []) + (["--pointer-socket", str(self.host / "pointer.sock")] if self.free else [])
        self.note("the engine", "lets the pointer go" + ("" if self.free else ", its word of it not given") if frees else "holds the pointer")
        subprocess.run(["open", "-n", "--stderr", str(self.log), "--stdout", str(self.log), str(self.app), "--args", "--dir", str(self.dir)] + options + (["--render-percent", str(self.a.render_percent)] if self.a.render_percent < 100 else []), check=True)
        sock = self.dir / "wsd.sock"
        for _ in range(100):
            if sock.exists():
                break
            time.sleep(0.05)
        if not self.wait_log("(pid ", 5):
            sys.exit("the window server did not start: " + self.tail())
        self.wsd_pid = int(re.search(r"\(pid (\d+)", self.log.read_text()).group(1))
        client = self.tmp / "game"
        subprocess.run(["cc", "-O2", "-I", self.a.headers, "-o", str(client), str(HERE / "game.c")], check=True)
        frames = self.tmp / "frames"
        self.game = Lines([str(client), str(sock), str(frames)])
        size = self.game.wait(lambda l: re.match(r"size (\d+) (\d+)", l), 5)
        self.size = (int(size.group(1)), int(size.group(2)))
        self.note("the game draws", f"{self.size[0]} x {self.size[1]}")
        subprocess.run([self.a.app, "--orbit-lab", "frames", str(TESTDATA), str(frames), str(self.size[0]), str(self.size[1])], check=True, stdout=subprocess.DEVNULL)
        self.game.say("ready")
        # (a question of macOS's waiting for somebody keeps the keyboard from an app that opens by itself: a click on it, as a person would. Its window is over the whole screen.)
        if not self.game.wait(lambda l: l == "focus 1", 4) and self.mouse.allowed and self.wait_log("[fullscreen-on]", 1):
            # (at the screen's foot on the left, where no question of macOS's is)
            self.note("another window had the keyboard", "clicked on the game's")
            self.mouse.move(6, 700); time.sleep(0.1)
            self.mouse.post(1); time.sleep(0.08); self.mouse.post(2)
        there = self.ok("the game's window has the keyboard, full screen", bool(self.game.wait(lambda l: l == "focus 1", 10)) and bool(self.wait_log("relative mouse on", 5)), self.tail())
        if frees:
            self.game.say("lock")
            there = there and self.ok("the engine holds the pointer, as while playing", self.wait_log("pointer held by the game", 3), self.tail())
        return there

    def tail(self):
        return self.log.read_text()[-400:] if hasattr(self, "log") and self.log.exists() else ""

    def wait_log(self, what, seconds, since=0):
        until = time.time() + seconds
        while time.time() < until:
            if what in self.log.read_text()[since:]:
                return True
            time.sleep(0.01)
        return False

    def start_desk(self):
        self.desk = Lines([self.a.app, "--orbit-lab", "desk", str(self.wsd_pid)] + ([str(self.host)] if self.free else []), stderr=open(self.tmp / "desk.err", "a"))

    # ---- what the overlay does
    def doing(self, good, seconds=3.0, since=None):
        """Waits until what it does is as `good` says; returns it and how long it took."""
        since = len(self.desk.lines) if since is None else since
        began, until = time.time(), time.time() + seconds
        self.desk.say("say")
        seen = None
        while time.time() < until:
            for at, line in self.desk.lines[since:]:
                try:
                    said = json.loads(line)
                except ValueError:
                    continue
                if "doing" in said:
                    seen = said["doing"]
                    if good(seen):
                        return seen, at - began
            time.sleep(0.004)
        return None, seen

    def tell(self, *lines):
        for line in lines:
            self.desk.say(line)

    def tap(self, key):
        self.tell(f"press {key}")
        time.sleep(0.08)
        self.tell(f"let go {key}")

    def answer(self, command, key, seconds=3):
        since = len(self.desk.lines)
        self.desk.say(command)
        def found(line):
            try:
                said = json.loads(line)
            except ValueError:
                return None
            return said if key in said else None
        got = self.desk.wait(found, seconds, since)
        return got[key] if got else None

    def cpu(self, seconds):
        self.answer("cpu", "cpu_percent")
        time.sleep(seconds)
        return self.answer("cpu", "cpu_percent")

    def shown_rate(self, seconds):
        def last():
            for _, line in reversed(self.game.lines):
                m = re.match(r"shown (\d+) zero (\d+)", line)
                if m:
                    return int(m.group(1)), int(m.group(2))
            return 0, 0
        time.sleep(0.6)
        a = last(); time.sleep(seconds); b = last()
        return (b[0] - a[0]) / seconds, b[1] - a[1]

    def save(self, name):
        if self.a.pictures:
            os.makedirs(self.a.pictures, exist_ok=True)
            self.answer(f"save {self.a.pictures}/{name}.png", "saved", 30)

    def ring_place(self, name):
        open_, _ = self.open_wheel(name)
        if open_ is None:
            return None
        time.sleep(0.7)
        return self.answer("spot WheelWorlds", "spot")

    def client_high(self):
        return self.answer("client", "client")[1]

    def close_menu(self):
        self.game.say("show none")
        self.game.say("lock")
        return self.doing(lambda d: d["open"] is None and not d["looking"] and not d["shown"], 5)

    def click(self):
        """A real click where the pointer is, when macOS allows it of the program this runs in, and only on a window of ours or of the game's: with a question of macOS's over
        them (it lies over every window, whatever its level, and waits for a person), an event of our own instead."""
        if not self.mouse.allowed or self.answer("top", "top") == "other":
            if self.mouse.allowed and "another program's window" not in self.notes:
                self.note("another program's window", "lies over part of them: no real click there")
            self.tell("click")
            return
        self.tell("button down"); self.mouse.post(1); time.sleep(0.08)
        self.tell("button up"); self.mouse.post(2)

    def over(self, zone, action=None):
        """The pointer over a part, or the control of it that does `action`: by the mouse itself where it is the person's."""
        if not self.free:
            return self.answer(f"point {zone}" if action is None else f"control {zone} {action}", "pointed")
        # (the parts move a little as they come)
        place = None
        for _ in range(20):
            now = self.answer("place " + zone + (f" {action}" if action else ""), "place")
            if now and now == place:
                break
            place = now
            time.sleep(0.05)
        if not place:
            return False
        x, y = self.mouse.where()
        for i in range(1, 9):
            self.mouse.move(x + (place[0] - x) * i / 8, y + (place[1] - y) * i / 8, (place[0] - x) / 8, (place[1] - y) / 8)
            time.sleep(0.01)
        time.sleep(0.1)
        return True

    def page_shows(self, name):
        """A short time after a press, with no input since: the page's own panel has a picture on the screen (macOS's, not ours), and nothing of ours hides it."""
        time.sleep(0.5)
        seen = self.answer("panel Takeover", "panel", 8)
        self.ok(f"{name}: its page is on the screen, painted, with nothing of ours over it", bool(seen) and seen["opaque"] > 0.9 and seen["spread"] > 4 and not seen["over"], str(seen))

    def open_wheel(self, name="wheel"):
        self.game.say("lock")
        self.game.say(f"show {name}")
        time.sleep(0.15)
        since = len(self.desk.lines)
        self.tap("r")
        return self.doing(lambda d: d["open"] == "Wheel" and len(d["shown"]) >= 5, 4, since)

    def run(self):
        if not self.start_game():
            return
        self.start_desk()
        rests, _ = self.doing(lambda d: not d["looking"] and not d["shown"], 6)
        self.ok("it looks once as the game comes in front, finds no menu, and rests", rests is not None)
        time.sleep(1.8)
        rests, seen = self.doing(lambda d: not d["looking"] and not d["shown"] and not d["held"], 3)
        self.ok("at rest: no picture is taken, no window shows, no keyboard is held", rests is not None, str(seen))
        base, _ = self.shown_rate(2)
        self.note("frames shown a second with nothing over the game", round(base, 1))
        self.note("processor at rest, the game in front, % of a core", round(self.cpu(10), 2))

        since = len(self.desk.lines)
        self.tap("esc")
        looked, _ = self.doing(lambda d: d["looking"], 2, since)
        self.ok("Esc starts a look", looked is not None)
        rests, _ = self.doing(lambda d: not d["looking"] and d["open"] is None, 4)
        self.ok("a key that opened no menu shows nothing, and the picture ends", rests is not None)

        open_, took = self.open_wheel()
        if not self.ok("R with VRChat's wheel up brings the ring", open_ is not None, f"{took} {self.tail()}"):
            return
        self.note("from R to the ring, seconds", round(took, 3))
        time.sleep(0.6)
        self.save("ring")
        self.ok("nothing takes the mouse", open_["catching"] == 0 and not open_["held"])
        self.ok("the 3D pictures cell is not there (six cells less one, the caption)", 5 <= len(open_["shown"]) <= 7, str(open_["shown"]))
        order = self.answer("order", "order")
        ours = [w for w in order if w[0] == "ours"]
        self.ok("the parts lie over the game's window, one level above it", bool(ours) and order[-1][0] == "game" and order[-1][1] == 25 and all(26 <= w[1] <= 31 for w in ours), str(order))
        rate, zero = self.shown_rate(3)
        self.note("frames shown a second under the ring", round(rate, 1))
        self.ok("macOS goes on showing the game's frames under the ring", rate > 0.9 * base and zero == 0, f"{rate} of {base}, {zero} not shown")
        self.ok("wsd never says its window is not shown", "not shown by macOS" not in self.log.read_text())
        self.note("processor with the ring up, % of a core", round(self.cpu(6), 2))

        if self.free:
            self.gesture(base)
        else:
            self.option_on_the_wheel(base)

        # A page, opened by its number: the keyboard with no key held.
        self.page_from_the_wheel(base)
        rests, seen = self.close_menu()
        self.ok("VRChat's menu gone: the parts go, and no picture runs", rests is not None, str(seen))
        self.quick_menu()
        self.camera(base)
        self.desk.end()
        self.start_desk()
        self.game.say("show none")
        time.sleep(2.5)
        self.game.say("quit")
        gone, seen = self.doing(lambda d: not d["shown"] and not d["looking"], 4)
        self.ok("the game ended: nothing shows, and the overlay is alive", gone is not None and self.desk.p.poll() is None, str(seen))
        self.note("processor with no game, % of a core", round(self.cpu(5), 2))
        self.tell("quit")

    def camera(self, base):
        """VRChat's camera, where the engine gives its stream: the column beside its panel, and the ring's sixth cell."""
        self.desk.end()
        self.start_desk()
        self.game.say("show none")
        self.game.say("lock")
        # (it was put away in the Dock a moment ago)
        subprocess.run(["open", str(self.app)])
        time.sleep(0.6)
        self.mouse.move(6, 700); time.sleep(0.2)
        if self.answer("top", "top") == "game":
            self.mouse.post(1); time.sleep(0.08); self.mouse.post(2)
        self.tell("camera nothing")
        time.sleep(2.5)
        rests, seen = self.doing(lambda d: not d["looking"] and not d["shown"], 4)
        self.ok("with its camera closed, no picture is taken for it", rests is not None, str(seen))
        since = len(self.desk.lines)
        self.game.say("show camera")
        self.game.say("free" if self.free else "lock")
        self.tell("camera open")
        column, took = self.doing(lambda d: d["open"] == "Camera" and len(d["shown"]) >= 4, 4, since)
        self.ok("VRChat says its camera is open: the column is beside its panel", column is not None, str(took))
        if column:
            self.note("from VRChat's word to the column, seconds", round(took, 3))
            time.sleep(0.8)
            self.save("camera")
            rate, zero = self.shown_rate(2)
            self.ok("the game's frames go on beside it", rate > 0.9 * base and zero == 0, f"{rate} of {base}, {zero}")
            self.note("processor with the camera's column up, % of a core", round(self.cpu(5), 2))
            if not self.free:
                self.tell("press option")
                self.doing(lambda d: d["pointing"] and d["keyboard"], 2)
            heard, since = len(self.game.lines), len(self.desk.lines)
            there = self.over("CameraPeople") is True
            caught, seen = self.doing(lambda d: d["catching"] == 1, 2)
            self.ok("its first tile is under the pointer", there and caught is not None, f"{there} {seen} {self.answer('place CameraPeople', 'place')} {self.mouse.where()}")
            self.click()
            took = self.desk.wait(lambda l: '"n4n0 ' in l and "Photo" in l, 3, since)
            self.ok("a click on it asks for a 3D picture of people", bool(took), str([l for _, l in self.desk.lines[since:] if "said" in l][:4]))
            self.ok("with no click of it in the game", not any(l.startswith("button") for _, l in self.game.lines[heard:]))
            self.tell("let go option")
        since = len(self.desk.lines)
        self.game.say("show none")
        self.tell("camera closed")
        gone, seen = self.doing(lambda d: d["open"] is None and not d["shown"] and not d["looking"], 4, since)
        self.ok("closed: the column goes, and no picture runs", gone is not None, str(seen))
        open_, took = self.open_wheel()
        self.ok("round the wheel, the 3D pictures cell is there", open_ is not None and len(open_["shown"]) >= 7, str(open_ and open_["shown"]) + str(took))
        time.sleep(0.6)
        self.save("ring-with-pictures")
        self.close_menu()

    def gesture(self, base):
        """The ring picked by the mouse's movement alone, while the game holds the pointer."""
        radius = 0.139 * self.client_high() / self.scale()
        at, heard = len(self.log.read_text()), len(self.game.lines)
        self.mouse.push(1.5 * radius, 0)
        time.sleep(1.6)
        still, seen = self.doing(lambda d: d["lit"] is None and d["open"] == "Wheel" and not d["sheet"], 1)
        self.ok("a flick to VRChat's rim lights none of ours, however long one stays", still is not None and not still["held"], str(seen))
        for dx, dy in ((0, 2.0), (-2.6, 0.4), (0, -3.0), (2.2, 0)):
            self.mouse.push(dx * radius, dy * radius, 0.15)
            time.sleep(0.5)
        lit = [l for _, l in self.desk.lines if '"lit":"' in l]
        self.ok("nor does going from slice to slice, as fast as one likes", not lit, str(lit[:1]))
        # Straight out to the right, twice: YouTube's cell (30 to 90 degrees) is there, a little up.
        self.mouse.push(-1.0 * radius, 0)
        self.tell("pick scroll 600")
        time.sleep(0.4)
        self.mouse.push(1.5 * radius, -0.5 * radius)
        time.sleep(0.4)
        since = len(self.desk.lines)
        self.mouse.push(1.0 * radius, -0.35 * radius)
        lit, seen = self.doing(lambda d: d["lit"] == "WheelYouTube", 2, since)
        self.ok("a push past the rim lights the cell there", lit is not None and not lit["held"] and lit["catching"] == 0, str(seen))
        self.save("pushed")
        time.sleep(1.2)
        still, _ = self.doing(lambda d: d["lit"] == "WheelYouTube" and not d["sheet"], 1)
        self.ok("with the scroll wheel chosen, a rest opens nothing", still is not None, str(_))
        self.note("processor with a cell lit, % of a core", round(self.cpu(3), 2))
        since = len(self.desk.lines)
        self.mouse.push(-1.1 * radius, 0.3 * radius)
        out, seen = self.doing(lambda d: d["lit"] is None, 2, since)
        self.ok("back inside the rim, it is out", out is not None, str(seen))
        # A click is VRChat's: what is lit goes out, and opens nothing.
        self.tell("pick both 600")
        time.sleep(0.4)
        self.mouse.push(1.5 * radius, -0.5 * radius)
        time.sleep(0.4)
        self.mouse.push(1.0 * radius, -0.35 * radius)
        self.doing(lambda d: d["lit"] == "WheelYouTube", 2)
        self.tell("button down")
        time.sleep(0.1)
        self.tell("button up")
        time.sleep(1.2)
        still, seen = self.doing(lambda d: d["lit"] is None and not d["sheet"], 1)
        self.ok("a click opens none of ours", still is not None, str(seen))
        # A notch.
        self.mouse.push(1.0 * radius, -0.35 * radius)
        lit, seen = self.doing(lambda d: d["lit"] == "WheelYouTube", 2)
        time.sleep(0.15)
        self.tell("scroll")
        page, took = self.doing(lambda d: d["sheet"], 2)
        self.ok("one notch of the scroll wheel opens it", lit is not None and page is not None, f"{seen} {took}")
        self.ok("the game was sent every move, and no button, all along", not any(l.startswith("button") for _, l in self.game.lines[heard:]))
        self.close_menu()
        time.sleep(0.4)
        # And a rest.
        if self.ok("the wheel again, for a rest", self.open_wheel()[0] is not None):
            time.sleep(0.5)
            self.mouse.push(0, 1.5 * radius)
            time.sleep(0.4)
            self.mouse.push(0, 1.0 * radius)
            lit, _ = self.doing(lambda d: d["lit"] is not None, 2)
            began = time.time()
            page, seen = self.doing(lambda d: d["sheet"] and d["keyboard"], 3)
            self.ok("a rest on it opens it, and its page has the mouse", lit is not None and page is not None, str(seen))
            self.note("from lit to open by a rest of 0.6 s, seconds", round(time.time() - began, 2))
        self.close_menu()
        time.sleep(0.4)
        self.ok("the wheel again", self.open_wheel()[0] is not None)
        rate, zero = self.shown_rate(2)
        self.ok("the game's frames go on", rate > 0.9 * base and zero == 0, f"{rate} of {base}")

    def scale(self):
        return self.answer("scale", "scale") or 2.0

    def option_on_the_wheel(self, base):
        at, heard = len(self.log.read_text()), len(self.game.lines)
        self.tell("press option")
        points, took = self.doing(lambda d: d["held"] and d["keyboard"] and d["pointing"], 2)
        self.ok("Left Option takes the keyboard and points", points is not None, str(took))
        self.ok("the engine lets the pointer go", self.wait_log(self.lets_go, 2, at), self.tail())
        self.ok("the game hears its window lost the keyboard, and stays the app in front", bool(self.game.wait(lambda l: l == "focus 0", 2, heard)) and not any(l == "deactivated" for _, l in self.game.lines[heard:]))
        order = self.answer("order", "order")
        self.ok("the game's window stays at its full-screen level", order[-1][0] == "game" and order[-1][1] == 25, str(order))
        self.ok("the pointer goes over a cell, whose window alone takes the mouse", self.answer("point WheelWorlds", "pointed") is True and self.doing(lambda d: d["catching"] == 1, 2)[0] is not None)
        rate, zero = self.shown_rate(2)
        self.ok("the game's frames go on while pointing", rate > 0.9 * base and zero == 0, f"{rate} of {base}")
        self.note("processor while pointing, % of a core", round(self.cpu(5), 2))
        at, heard = len(self.log.read_text()), len(self.game.lines)
        self.tell("let go option")
        back, _ = self.doing(lambda d: not d["held"] and d["catching"] == 0, 2)
        self.ok("let go, the game has its mouse again at once", back is not None and self.wait_log(self.takes, 2, at) and bool(self.game.wait(lambda l: l == "focus 1", 2, heard)))

    def page_from_the_wheel(self, base):
        self.tap("2")
        page, _ = self.doing(lambda d: d["sheet"] and d["keyboard"] and d["pointing"], 4)
        self.ok("a number opens its page, which keeps the mouse with no key held", page is not None)
        rate, zero = self.shown_rate(3)
        self.ok("the game's frames go on under a page", rate > 0.9 * base and zero == 0, f"{rate} of {base}, {zero}")
        self.ok("no part was left out to keep the game in sight", page is not None and page["left_out"] is None)
        since = len(self.desk.lines)
        self.tell("type 0")
        self.ok("a key typed in YouTube's search goes to N4N0's menu", bool(self.desk.wait(lambda l: '"typed Text' in l, 2, since)), str(self.desk.lines[-3:]))
        since = len(self.desk.lines)
        self.tell("type 53")
        self.ok("Esc there is the page's own Back", bool(self.desk.wait(lambda l: "typed Escape" in l, 2, since)))
        self.tap("esc")
        still, _ = self.doing(lambda d: d["open"] == "Wheel", 1)
        self.ok("an Esc the game never heard is not taken for its menu closing", still is not None)
        self.close_menu()

        # Another page, with no keyboard in it: Esc closes it.
        if self.ok("the wheel again", self.open_wheel()[0] is not None):
            self.tap("4")
            page, _ = self.doing(lambda d: d["sheet"] and d["pointing"], 4)
            self.ok("All tools opens", page is not None)
            time.sleep(1)
            self.note("processor with a page up, % of a core", round(self.cpu(5), 2))
            at = len(self.log.read_text())
            self.tell("type 53")
            gone, _ = self.doing(lambda d: not d["sheet"] and not d["held"], 3)
            self.ok("Esc closes the page, and the game has its keyboard again", gone is not None and self.wait_log(self.takes, 2, at))
        self.close_menu()
        # VRChat's wheel a little off its place, and smaller: found where it is, with the ring round it there.
        time.sleep(0.5)
        place = self.ring_place("wheel")
        self.close_menu()
        time.sleep(0.5)
        beside = self.ring_place("beside")
        self.ok("a wheel beside its place is found", beside is not None, str(beside))
        if place and beside:
            radius = 0.139 * self.client_high()
            moved = ((beside[0] - place[0]) / radius, (beside[1] - place[1]) / radius)
            self.note("that wheel's ring moved, in radii (it lies -0.06, +0.04 off)", (round(moved[0], 3), round(moved[1], 3)))
            self.ok("and the ring lies round it", abs(moved[0] + 0.06) < 0.035 and abs(moved[1] - 0.04) < 0.035, str(moved))
    def quick_menu(self):
        self.game.say("show qm")
        self.game.say("free")
        time.sleep(0.15)
        since = len(self.desk.lines)
        self.tap("esc")
        quick, took = self.doing(lambda d: d["open"] == "QuickMenu" and len(d["shown"]) >= 6, 4, since)
        if self.ok("Esc with VRChat's Quick Menu up brings the wing", quick is not None, str(took)):
            self.note("from Esc to the wing, seconds", round(took, 3))
            time.sleep(0.6)
            self.save("wing")
            heard, at = len(self.game.lines), len(self.log.read_text())
            if self.free:
                points, seen = self.doing(lambda d: d["mouse"] and d["pointer"] == "Free" and not d["held"] and not d["keyboard"], 2)
                self.ok("the mouse is the person's, with no key, and the keyboard the game's", points is not None, str(seen))
            else:
                self.tell("press option")
                points, _ = self.doing(lambda d: d["pointing"] and d["keyboard"], 2)
                self.ok("Left Option points at the wing", points is not None)
            self.ok("a tile is under the pointer", self.over("WingAll") is True and self.doing(lambda d: d["catching"] == 1, 2)[0] is not None)
            if self.free:
                time.sleep(0.3)
                self.ok("macOS shows its own arrow over it", self.mouse.cg.CGCursorIsVisible())
                self.note("processor with the mouse still over a tile, % of a core", round(self.cpu(4), 2))
            if not self.mouse.allowed:
                self.note("macOS does not let this program click", "the clicks are events of our own, which raise no window")
            self.click()
            page, _ = self.doing(lambda d: d["sheet"], 3)
            self.ok("a click on a tile opens its page", page is not None)
            self.page_shows("All tools' tile")
            for zone in ("WingYouTube", "WingInstances", "WingTalk", "WingWorlds"):
                if self.over(zone) is True:
                    time.sleep(0.15); self.click()
                    self.page_shows(zone)
            # The tabs are on the page's head, which is the panel behind the page: a click raises it.
            tabs = 0
            for action in ("Pictures", "Page(Instances)", "Page(YouTube)", "Talk", "Worlds", "All"):
                if self.over("SheetBack", action) is True:
                    tabs += 1
                    time.sleep(0.15); self.click()
                    self.page_shows(f"the tab {action}")
            self.ok("the page's head has its tabs", tabs >= 3, str(tabs))
            if self.free:
                self.ok("no click of these reached the game", not any(l.startswith("button") for _, l in self.game.lines[heard:]), str([l for _, l in self.game.lines[heard:] if l.startswith("button")][:3]))
                # YouTube's page is typed in as it opens, and only it takes the keyboard. (All tools is up, the last tab clicked.)
                away, seen = self.doing(lambda d: d["sheet"] and d["mouse"] and not d["held"], 3)
                self.ok("a page with nothing to type in leaves the game its keyboard", away is not None, str(seen))
                time.sleep(0.5)
                heard, at = len(self.game.lines), len(self.log.read_text())
                self.ok("YouTube's tile is there", self.over("WingYouTube") is True)
                self.click()
                page, seen = self.doing(lambda d: d["sheet"] and d["keyboard"], 3)
                self.ok("a text field takes the keyboard", page is not None and bool(self.game.wait(lambda l: l == "focus 0", 2, heard)), str(seen))
                since = len(self.desk.lines)
                self.tell("type 0")
                self.ok("what is typed goes there", bool(self.desk.wait(lambda l: '"typed Text' in l, 2, since)))
                heard = len(self.game.lines)
                self.over("WingWorlds"); self.click()
                away, seen = self.doing(lambda d: d["sheet"] and d["mouse"] and not d["held"], 3)
                self.ok("left, the game has its keyboard again at once, the page still up", away is not None and bool(self.game.wait(lambda l: l == "focus 1", 2, heard)), str(seen))
                self.ok("YouTube's again", self.over("WingYouTube") is True)
                self.click()
                self.doing(lambda d: d["keyboard"], 3)
                # A click on the game, beside everything of ours: its keyboard, at once, the field still up.
                heard = len(self.game.lines)
                w, h = self.answer("client", "client")
                x, y = self.mouse.where()
                self.mouse.move(8, h / self.scale() - 8, 8 - x, 8 - y)
                time.sleep(0.15)
                self.click()
                back, seen = self.doing(lambda d: d["sheet"] and not d["keyboard"] and not d["held"], 3)
                self.ok("a click on the game gives it the keyboard, with the field still up", back is not None and bool(self.game.wait(lambda l: l == "focus 1", 2, heard)), str(seen))
                self.ok("the game heard that click, or another program's window lay there", "another program's window" in self.notes or bool(self.game.wait(lambda l: l == "button 0 1" or l.startswith("button"), 2, heard)))
                self.over("Takeover"); self.click()
                again, seen = self.doing(lambda d: d["sheet"] and d["keyboard"], 3)
                self.ok("and a press on the page takes it again", again is not None, str(seen))
            else:
                self.tell("let go option")
                time.sleep(0.4)
                page, _ = self.doing(lambda d: d["sheet"] and d["keyboard"], 1)
                self.ok("the page keeps the mouse once the key is let go", page is not None)
            time.sleep(0.5)
            self.save("page")

            # Another app in front, the game still on the screen: the parts stay with it, under what covers it.
            heard = len(self.game.lines)
            subprocess.run(["open", "-a", "Finder"])
            self.game.wait(lambda l: l == "deactivated", 4, heard)
            away, seen = self.doing(lambda d: not d["held"] and d["catching"] == 0 and d["shown"], 3)
            self.ok("another app in front: nothing is held, the parts stay", away is not None, str(seen))
            time.sleep(0.8)
            order = self.answer("order", "order")
            names = [w[0] for w in order if w[1] == 0]
            n = len([x for x in names if x == "ours"])
            self.ok("the parts lie right above the game's window, at the ordinary level", n > 0 and names[-1] == "game" and names[-1 - n:-1] == ["ours"] * n and all(w[1] == 0 for w in order if w[0] == "ours"), str(order[-12:]))
            since = len(self.desk.lines)
            self.tap("r")
            time.sleep(0.3)
            still, _ = self.doing(lambda d: d["open"] == "QuickMenu", 1)
            self.ok("a key pressed in another app is not the game's", still is not None)
            self.note("processor with parts up behind another app, % of a core", round(self.cpu(5), 2))
            subprocess.run(["open", str(self.app)])
            front, seen = self.doing(lambda d: d["shown"] and d["keyboard"], 5)
            self.ok("the game back in front: the page has the mouse again", front is not None, str(seen))
            # (the engine raises its window a moment after it comes in front; ours follow at their next look at it)
            time.sleep(0.3)
            order = self.answer("order", "order")
            self.ok("and the parts are above its full-screen level again", all(26 <= w[1] <= 31 for w in order if w[0] == "ours") and any(w[0] == "ours" for w in order), str(order))

            # The process ended while it holds the keyboard: the game has it again by itself.
            heard = len(self.game.lines)
            self.desk.p.send_signal(signal.SIGKILL)
            self.ok("killed while it holds the keyboard, the game has it again", bool(self.game.wait(lambda l: l == "focus 1", 2, heard)))
            self.desk.end()
            self.start_desk()
            back, took = self.doing(lambda d: d["open"] == "QuickMenu" and d["shown"], 6)
            self.ok("started again (as the service does), it finds the menu that is up", back is not None, str(took))
            if self.free:
                self.over("WingYouTube"); self.click()
            else:
                self.tell("press option")
            self.ok("the keyboard is ours", self.doing(lambda d: d["keyboard"], 3)[0] is not None)
            heard = len(self.game.lines)
            self.tell("hang")
            began = time.time()
            freed = self.game.wait(lambda l: l == "focus 1", 5, heard)
            self.ok("hung while it holds the keyboard, it ends itself and the game has it again", bool(freed), f"{time.time() - began:.1f} s")
            self.note("from hanging to the game's keyboard back, seconds", round(time.time() - began, 2))
            self.desk.end()
            self.start_desk()
            self.doing(lambda d: d["open"] == "QuickMenu" and d["shown"], 6)

            # Put away in the Dock and back.
            self.game.say("hide")
            gone, seen = self.doing(lambda d: not d["shown"] and not d["looking"] and not d["held"], 5)
            self.ok("the game minimised: nothing shows, nothing is looked for", gone is not None, str(seen))
            self.game.say("unhide")
            subprocess.run(["open", str(self.app)])
            back, seen = self.doing(lambda d: d["open"] == "QuickMenu" and d["shown"], 8)
            self.ok("back from the Dock, with its menu still up, the parts come back", back is not None, str(seen))
            self.tell("hide-menus")
            off, _ = self.doing(lambda d: not d["shown"], 3)
            self.ok("turned off in Settings: the parts go", off is not None)

    def end(self):
        for p in (self.desk, self.game):
            if p:
                p.say("quit")
        time.sleep(0.6)
        for p in (self.desk, self.game):
            if p:
                p.end()
        if self.wsd_pid:
            try:
                os.kill(self.wsd_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        err = (self.tmp / "desk.err")
        if err.exists() and err.read_text().strip():
            print("the overlay's own words:\n" + "\n".join(err.read_text().splitlines()[-12:]))
        if self.a.keep:
            print("kept", self.tmp)
        else:
            shutil.rmtree(self.tmp, ignore_errors=True)

def main():
    p = argparse.ArgumentParser()
    p.add_argument("--render-percent", type=int, default=100)
    p.add_argument("--force", action="store_true")
    p.add_argument("--idle", type=int, default=120, help="seconds nobody must have used this Mac for")
    p.add_argument("--keep", action="store_true")
    p.add_argument("--pictures", help="a folder for pictures of the parts as they lie")
    p.add_argument("--no-channel", action="store_true", help="as if the engine's word of the pointer were not there")
    p.add_argument("--wsd")
    p.add_argument("--headers", default=os.path.expanduser("~/.orbiters-forks/src/voile-lkd/wsd"))
    p.add_argument("--app", default=str(REPO / "target/debug/orbiters-desktop"))
    a = p.parse_args()
    if not a.force and idle() < a.idle:
        sys.exit(f"somebody used this Mac {idle():.0f} s ago: not started (--force)")
    if locked():
        sys.exit("the screen is locked: not started")
    # (the display must not go to sleep, nor the screen lock, half way)
    awake = subprocess.Popen(["caffeinate", "-d", "-w", str(os.getpid())])
    c = Check(a)
    def watch():
        # (an idle time that went back: a key, the mouse, the trackpad)
        was = idle()
        while True:
            time.sleep(0.5)
            now = idle()
            if now < was and time.time() - c.mouse.at > 2:
                print("somebody uses this Mac: ended", flush=True)
                _thread.interrupt_main()
                return
            was = now
    if not a.force:
        threading.Thread(target=watch, daemon=True).start()
    try:
        c.run()
    except KeyboardInterrupt:
        c.failed.append("ended before it was done")
    finally:
        c.end()
    if locked():
        c.failed.append("the screen was locked meanwhile")
    awake.kill()
    print(f"\n{'FAILED: ' + '; '.join(c.failed) if c.failed else 'all passed'}")
    sys.exit(1 if c.failed else 0)

if __name__ == "__main__":
    main()
