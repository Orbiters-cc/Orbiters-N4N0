#!/usr/bin/env python3
"""The Mac's overlay (overlay/orbit/desktop_mac) over the engine's real window server, with no game, no Steam and no person.

    tools/mac-overlay/check.py [--render-percent N] [--force] [--pictures FOLDER] [--wsd PATH] [--headers PATH] [--app PATH]

It opens a bundle of its own around the engine's `wsd`, has game.c show a full-screen window there as VRChat's is (holding the pointer, its cursor hidden, real frames of VRChat's
menus stretched by the render percent), and turns the app's own loop over it (`orbiters-desktop --orbit-lab desk`) with a keyboard made up here. It covers the screen for about a
minute: it starts only while nobody has used this Mac for two minutes (--force: anyway), and ends when somebody does. Screen Recording is needed of the program it is started from.
"""
import _thread, argparse, glob, json, os, pathlib, plistlib, queue, re, shutil, signal, subprocess, sys, tempfile, threading, time

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
        subprocess.run(["open", "-n", "--stderr", str(self.log), "--stdout", str(self.log), str(self.app), "--args", "--dir", str(self.dir)] + (["--render-percent", str(self.a.render_percent)] if self.a.render_percent < 100 else []), check=True)
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
        return self.ok("the game's window has the keyboard, full screen", bool(self.game.wait(lambda l: l == "focus 1", 10)) and bool(self.wait_log("relative mouse on", 5)), self.tail())

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
        self.desk = Lines([self.a.app, "--orbit-lab", "desk", str(self.wsd_pid)], stderr=open(self.tmp / "desk.err", "a"))

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

    def close_menu(self):
        self.game.say("show none")
        return self.doing(lambda d: d["open"] is None and not d["looking"] and not d["shown"], 5)

    def open_wheel(self):
        self.game.say("show wheel")
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
        self.ok("the parts lie over the game's window, one level above it", bool(ours) and order[-1][0] == "game" and order[-1][1] == 25 and all(w[1] == 26 for w in ours), str(order))
        rate, zero = self.shown_rate(3)
        self.note("frames shown a second under the ring", round(rate, 1))
        self.ok("macOS goes on showing the game's frames under the ring", rate > 0.9 * base and zero == 0, f"{rate} of {base}, {zero} not shown")
        self.ok("wsd never says its window is not shown", "not shown by macOS" not in self.log.read_text())
        self.note("processor with the ring up, % of a core", round(self.cpu(6), 2))

        at, heard = len(self.log.read_text()), len(self.game.lines)
        self.tell("press option")
        points, took = self.doing(lambda d: d["held"] and d["keyboard"] and d["pointing"], 2)
        self.ok("Left Option takes the keyboard and points", points is not None, str(took))
        self.ok("the engine lets the pointer go", self.wait_log("focus lost, relative mouse suspended", 2, at), self.tail())
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
        self.ok("let go, the game has its mouse again at once", back is not None and self.wait_log("focus back, relative mouse resumed", 2, at) and bool(self.game.wait(lambda l: l == "focus 1", 2, heard)))

        # A page, opened by its number: the keyboard with no key held.
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
            self.ok("Esc closes the page, and the game has its keyboard again", gone is not None and self.wait_log("relative mouse resumed", 2, at))
        rests, seen = self.close_menu()
        self.ok("VRChat's menu gone: the parts go, and no picture runs", rests is not None, str(seen))

        self.game.say("show qm")
        time.sleep(0.15)
        since = len(self.desk.lines)
        self.tap("esc")
        quick, took = self.doing(lambda d: d["open"] == "QuickMenu" and len(d["shown"]) >= 6, 4, since)
        if self.ok("Esc with VRChat's Quick Menu up brings the wing", quick is not None, str(took)):
            self.note("from Esc to the wing, seconds", round(took, 3))
            time.sleep(0.6)
            self.save("wing")
            self.tell("press option")
            points, _ = self.doing(lambda d: d["pointing"] and d["keyboard"], 2)
            self.ok("Left Option points at the wing", points is not None)
            self.ok("a tile is under the pointer", self.answer("point WingAll", "pointed") is True and self.doing(lambda d: d["catching"] == 1, 2)[0] is not None)
            self.tell("click")
            page, _ = self.doing(lambda d: d["sheet"], 3)
            self.ok("a click on a tile opens its page", page is not None)
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
            order = self.answer("order", "order")
            self.ok("and the parts are above its full-screen level again", all(w[1] == 26 for w in order if w[0] == "ours") and any(w[0] == "ours" for w in order), str(order))

            # The process ended while it holds the keyboard: the game has it again by itself.
            heard = len(self.game.lines)
            self.desk.p.send_signal(signal.SIGKILL)
            self.ok("killed while it holds the keyboard, the game has it again", bool(self.game.wait(lambda l: l == "focus 1", 2, heard)))
            self.desk.end()
            self.start_desk()
            back, took = self.doing(lambda d: d["open"] == "QuickMenu" and d["shown"], 6)
            self.ok("started again (as the service does), it finds the menu that is up", back is not None, str(took))
            self.tell("press option")
            self.doing(lambda d: d["keyboard"], 2)
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
        self.desk.end()
        self.start_desk()
        self.game.say("show none")
        time.sleep(2.5)
        self.game.say("quit")
        gone, seen = self.doing(lambda d: not d["shown"] and not d["looking"], 4)
        self.ok("the game ended: nothing shows, and the overlay is alive", gone is not None and self.desk.p.poll() is None, str(seen))
        self.note("processor with no game, % of a core", round(self.cpu(5), 2))
        self.tell("quit")

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
            if now < was:
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
