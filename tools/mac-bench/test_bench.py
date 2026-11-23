#!/usr/bin/env python3
"""Checks of bench.py's scoring, arrival and cleanup (fourth verification, finding 11; fifth, findings 11 and 12), without
a game: python3 -I tools/mac-bench/test_bench.py"""
import contextlib, datetime, json, os, shutil, subprocess, sys, tempfile, unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bench  # noqa: E402


def npt(n, shown=0, interval=16.0, fence=8.0):
    return f"neptune unixlib wsi (wsd): {n} presents ({shown} shown events), last 60: interval avg {interval} ms, fence wait avg {fence} ms, pacing wait avg 0.0 ms"


class Scoring(unittest.TestCase):
    def test_the_boundary_interval_counts_and_the_rate_is_over_matching_times(self):
        lines = [(9.0, npt(0)), (10.5, npt(60)), (11.5, npt(120)), (12.5, npt(180))]
        rate, seconds = bench.counted_rate(bench.generations(lines), 10.0, 13.0, 2)
        self.assertEqual((rate, seconds), (51.43, 3.5), "from the line before the window: 180 presents in 3.5 s")

    def test_a_new_swapchain_is_never_subtracted_across(self):
        lines = [(1.0, npt(9000)), (2.0, npt(9060)), (2.5, "guest swapchain: created 1705x1109 fmt=29 buffers=1"), (3.0, npt(60)), (4.0, npt(120))]
        rate, seconds = bench.counted_rate(bench.generations(lines), 1.5, 5.0, 2)
        self.assertEqual((rate, seconds), (60.0, 2.0), "60 in gen 0 (1 s) + 60 in gen 1 (1 s)")

    def test_a_measured_zero_stays_zero(self):
        lines = [(1.0, npt(100, 5)), (2.0, npt(160, 5))]
        rate, _ = bench.counted_rate(bench.generations(lines), 0.0, 3.0, 3)
        self.assertEqual(rate, 0.0)
        self.assertIsNone(bench.counted_rate(bench.generations(lines[:1]), 0.0, 3.0, 2)[0], "one line: not known")

    def test_lines_read_together_are_one_reading(self):
        # 90 a second, read once a second: two or one line at a time (was 60/s).
        lines, n = [], 0
        for second in range(1, 11):
            for _ in range(2 if second % 2 else 1):
                n += 60
                lines.append((float(second), npt(n)))
        rate, seconds = bench.counted_rate(bench.generations(lines), 1.0, 10.5, 2)
        self.assertAlmostEqual(rate, 90.0, delta=4.0)
        self.assertEqual(seconds, 9.0)

    # ---- windows ----------------------------------------------------------------------------------------------------

    WSD = 500
    WALL = 1_800_000_000.0  # the wall clock when this run's was 0

    def look(self, at, **other):
        return {"at": at, "wall": self.WALL + at, "alive": True, "locked": False, "front": self.WSD, "wsd": [self.WSD]} | other

    def hud(self, at, frames=60, pid=None, every=16.6667):
        stamp = datetime.datetime.fromtimestamp(self.WALL + at).strftime("%Y-%m-%d %H:%M:%S.%f")[:-3]
        values = ",".join(f"{every},0.1" for _ in range(frames))
        return f"{stamp} wsd[{pid or self.WSD}:123] metal-HUD: {frames},90.0,100.0,{values}"

    def window(self, seconds=60, hud=True, production=True, looks=True):
        """A whole, healthy window of `seconds` from 10 s on: (lines, state, moves)."""
        lines, n = [], 0
        for at in range(8, 10 + seconds + 1):
            if production:
                n += 60
                lines.append((at + 0.5, npt(n)))
            if hud:
                lines.append((at + 0.9, self.hud(at + 0.9)))
        state = {"gpu": [], "cpu": [], "look": [self.look(at + 0.2) for at in range(9, 10 + seconds)] if looks else []}
        return lines, state, [(5.0, True)]

    def scored(self, lines, state, moves, allow_locked=False, seconds=60):
        return bench.summarize(lines, 10.0, 10.0 + seconds, state, allow_locked, moves)

    def test_the_front_process_is_read_on_every_macos(self):
        # 2026-11-23, macOS 27: no window of a whole series was scored, the front app always read as None.
        as_macos_27 = ' ASN:0x0-0x4c34c3: (in front) \n    bundleID=[ NULL ] \n    executable path=[ NULL ] \n    pid = 11538 !cgsConnection !signalled type=[ NULL ]  flavor=[ NULL ] \n\n'
        self.assertEqual(bench.front_pid(as_macos_27), 11538)
        self.assertEqual(bench.front_pid('"pid"=11538\n'), 11538)
        self.assertIsNone(bench.front_pid(""))
        self.assertIsNone(bench.front_pid("    pid = [ NULL ] \n"))

    def test_a_whole_window_is_scored(self):
        good = self.scored(*self.window())
        self.assertTrue(good["valid"], good["invalid_because"])
        self.assertEqual(good["guest_present"]["submitted_per_second_counted"], 60.0)
        self.assertAlmostEqual(good["wsd_shown"]["per_second"], 60.0, delta=1.5)

    def test_a_window_without_shown_frames_or_behind_a_lock_is_not_scored(self):
        lines, state, moves = self.window(hud=False)
        state["look"] = [look | {"locked": True, "front": 77} for look in state["look"]]
        bad = self.scored(lines, state, moves)
        self.assertFalse(bad["valid"])
        self.assertFalse(bad["production_only"])
        self.assertTrue(any("locked" in w for w in bad["invalid_because"]))
        self.assertTrue(any("no frame was shown" in w for w in bad["invalid_because"]))
        prod = self.scored(lines, state, moves, True)
        self.assertTrue(prod["production_only"], "allowed explicitly: production only, and said")
        self.assertEqual(prod["wsd_shown"]["per_second"], 0.0, "zero shown is measured as zero")

    def test_one_look_one_batch_and_one_shown_interval_score_nothing(self):
        # Fifth verification, finding 11: this was valid, with zero seconds counted.
        lines = [(40.0, npt(60)), (40.1, self.hud(40.1, frames=1))]
        state = {"gpu": [], "cpu": [], "look": [self.look(40.0)]}
        bad = self.scored(lines, state, [(5.0, True)])
        self.assertFalse(bad["valid"])
        for what in ("nothing was looked at for", "counted over 0.0 s", "no frame was shown all along"):
            self.assertTrue(any(what in w for w in bad["invalid_because"]), (what, bad["invalid_because"]))
        self.assertFalse(self.scored(lines, state, [(5.0, True)], True)["production_only"], "nor its production")

    def test_every_part_of_the_window_is_seen(self):
        lines, state, moves = self.window()
        invalid = lambda l, s, m, what: self.assertTrue(any(what in w for w in self.scored(l, s, m)["invalid_because"]), what)
        gap = state | {"look": [x for x in state["look"] if not 30 < x["at"] < 40]}
        invalid(lines, gap, moves, "nothing was looked at for")
        invalid(lines, state | {"look": [x for x in state["look"] if x["at"] < 60]}, moves, "nothing was looked at for")
        died = state | {"look": [x | {"alive": x["at"] < 50} for x in state["look"]]}
        invalid(lines, died, moves, "the session did not live")
        covered = state | {"look": [x | {"front": 77} if 30 < x["at"] < 31.5 else x for x in state["look"]]}
        invalid(lines, covered, moves, "not frontmost")
        invalid(lines, state | {"look": [x | {"front": None} for x in state["look"]]}, moves, "not frontmost")
        stopped = [(t, l) for t, l in lines if t < 50]
        invalid(stopped, state, moves, "its submissions were counted over")
        invalid(stopped, state, moves, "no frame was shown all along")
        # Another program's frames, or another session's window server's, are not this one's.
        others = [(t, l.replace(f"wsd[{self.WSD}:", "wsd[501:")) for t, l in lines]
        invalid(others, state, moves, "no frame was shown (no positive")
        # Few frames, each shown long ago: 100 ms of every second.
        sparse = [(t, self.hud(t, frames=6)) if "metal-HUD" in l else (t, l) for t, l in lines]
        invalid(sparse, state, moves, "no frame was shown all along")
        # The player left, or went elsewhere.
        invalid(lines, state, [], "in the destination")
        invalid(lines, state, [(5.0, True), (40.0, False)], "in the destination")
        invalid(lines, state, [(5.0, True), (40.0, False), (45.0, True)], "in the destination")
        invalid(lines, state, [(5.0, True), (8.0, False)], "in the destination")

    def test_a_start_that_does_not_settle_is_not_calm(self):
        quiet = [(t / 2, npt(t * 30)) for t in range(0, 200)]
        self.assertTrue(bench.settled(quiet, 99.0))
        held = quiet + [(90.0, "wsd: window 1 overlay: frame held 300.0 ms, shown at 11:05:58.362")]
        self.assertFalse(bench.settled(held, 99.0))
        self.assertFalse(bench.settled([], 99.0), "nothing presented is not calm")
        self.assertFalse(bench.settled(quiet, 200.0), "nothing presented lately")


WORLD = "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd"
OTHER = "wrld_99999999-729c-46e3-8eaf-846aa0a37fdd"
HERE = f"{WORLD}:25038~private(usr_1)~region(eu)"
LINK = f"vrchat://launch?ref=orbiters.cc&id={HERE}"


class Arrival(unittest.TestCase):
    def joined(self, *said):
        return [(float(n), f"2026.11.23 11:29:24 Debug      -  [Behaviour] {text}") for n, text in enumerate(said)]

    def test_arrival_is_the_local_player_in_this_very_destination(self):
        wanted = bench.destination(LINK)
        self.assertEqual(wanted, (WORLD, "25038", {"private": "usr_1", "region": "eu"}))
        self.assertEqual(bench.destination(LINK.replace("~", "%7E").replace("(", "%28")), wanted)
        self.assertIsNone(bench.destination("vrchat://launch?ref=x"))
        there = lambda *said: [x for _, x in bench.whereabouts(self.joined(*said), wanted)]
        local = 'Initialized PlayerAPI "Some One" is local'
        self.assertEqual(there(f"Joining {HERE}", "OnPlayerJoinComplete Some One", local), [False, True])
        self.assertEqual(there(f"Joining {HERE}~nonce(x)", local), [False, True], "a tag the link did not ask for")
        # Finding 12: any join used to do, the fallback's too.
        for elsewhere in (f"{OTHER}:1~private(usr_1)~region(eu)", f"{WORLD}:25039~private(usr_1)~region(eu)", f"{WORLD}:25038~private(usr_2)~region(eu)", f"{WORLD}:25038", f"{WORLD}:25038~private(usr_1)~region(us)"):
            self.assertEqual(there(f"Joining {elsewhere}", "OnPlayerJoinComplete Some One", local), [False], elsewhere)
        # Another player's arrival, a join not yet made, a join after it.
        self.assertEqual(there(f"Joining {HERE}", "OnPlayerJoinComplete Another", 'Initialized PlayerAPI "Another" is remote'), [False])
        self.assertEqual(there(local), [])
        self.assertEqual(there(f"Joining {HERE}", local, f"Joining {OTHER}:1", local), [False, True, False])
        self.assertEqual(there(f"Joining {HERE}", local, "OnLeftRoom"), [False, True, False])


class Ownership(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        self.kept = bench.RECORDS
        bench.RECORDS = self.root.name

    def tearDown(self):
        bench.RECORDS = self.kept
        self.root.cleanup()

    def write(self, session, pointer=True, **fields):
        value = {"id": session, "store": {"app_id": 438100}, "phase": "Running", "supervisor": {"birth_seconds": 1000}, "game": {"pid": 5}} | fields
        for name in ([session] + (["session"] if pointer else [])):
            with open(f"{self.root.name}/{name}.v2.json", "w") as f:
                json.dump(value, f)

    def test_a_session_is_known_from_its_admission(self):
        self.assertIsNone(bench.admitted(None, 1000))
        self.write("a", phase="Finished")
        self.assertIsNone(bench.admitted("a", 1000), "the one there was")
        self.assertIsNone(bench.admitted("z", 2000), "another launch's, before this one")
        self.write("b", phase="Preparing", game=None)
        self.assertEqual(bench.admitted("a", 1000), "b", "before it runs")
        self.write("c", store={"app_id": 1})
        self.assertIsNone(bench.admitted("a", 1000), "another game's")

    def test_a_launch_is_closed_by_its_own_record_whether_or_not_its_program_lives(self):
        asked, clock = [], [0.0]
        kept = bench.run, bench.now, bench.time.sleep
        bench.run = lambda command, timeout=10: asked.append(command) or ""
        bench.now = lambda: clock[0]

        class Proc:
            def __init__(self, code):
                self.code = code

            def poll(self):
                return self.code

            def terminate(self):
                self.code = -15

        def slept(seconds):
            clock[0] += seconds
        bench.time.sleep = slept
        try:
            # The program died and the game runs on: its session is asked to quit all the same; it does not end.
            self.write("a")
            res = {"outcome": "x"}
            self.assertFalse(bench.close("/cli", "a", Proc(1), res))
            self.assertEqual(asked[-1], ["/cli", "steam", "quit", "438100", "--session", "a", "--wait", "60"])
            self.assertIn("may still run", res["outcome"])
            # It ends: known from its record.
            self.write("a", phase="Finished", game=None, exit_code=0)
            res = {"outcome": "x"}
            self.assertTrue(bench.close("/cli", "a", Proc(0), res))
            self.assertEqual(res["terminal"]["phase"], "Finished")
            self.assertEqual(len(asked), 1, "an ended session is asked nothing")
            # Its session ended and the program hangs: the program alone is ended.
            hung = Proc(None)
            self.assertTrue(bench.close("/cli", "a", hung, {"outcome": "x"}))
            self.assertEqual(hung.code, -15)
            # No session was admitted: the program's own end says so; one that hangs is not resolved.
            self.assertTrue(bench.close("/cli", None, Proc(1), {"outcome": "x"}))
            self.assertFalse(bench.close("/cli", None, Proc(None), {"outcome": "x"}))
            # A record that cannot be read, or whose game still lives, is no end.
            with open(f"{self.root.name}/b.v2.json", "w") as f:
                f.write("{ half")
            self.assertFalse(bench.close("/cli", "b", Proc(1), {"outcome": "x"}))
            self.write("c", phase="Finished")
            self.assertFalse(bench.close("/cli", "c", Proc(1), {"outcome": "x"}))
        finally:
            bench.run, bench.now, bench.time.sleep = kept

    def test_only_the_launcher_s_own_tree_is_sampled(self):
        listing = "  1     0 /sbin/launchd\n 10     1 /bin/bash\n 11    10 /x/wsd\n 12    11 /x/a b/child\n 20     1 /y/wsd\n"
        self.assertEqual(bench.tree(listing, 10), {10: "bash", 11: "wsd", 12: "child"})
        self.assertEqual(bench.tree(listing, -1), {})


FAKE_CLI = r"""#!/usr/bin/env python3
# Answers as obelisk-cli does, for a game that is this very process: root is the folder it keeps everything in.
import datetime, json, os, sys, time
root, how = ROOT, json.load(open(ROOT + "/how.json"))
say = lambda **event: print(json.dumps(event), flush=True)
def write(session, phase, game):
    value = {"id": session, "store": {"app_id": 438100}, "phase": phase, "supervisor": {"birth_seconds": int(time.time())}, "game": game, "exit_code": 0}
    for name in (session, "session"):
        open(f"{root}/records/{name}.new", "w").write(json.dumps(value))
        os.rename(f"{root}/records/{name}.new", f"{root}/records/{name}.v2.json")
if sys.argv[1:3] == ["steam", "quit"]:
    open(f"{root}/quit", "w").write(" ".join(sys.argv))
    sys.exit(0)
say(event="preparing", app_id=438100)
if how.get("fails"):
    say(event="error", code="busy", message="m")
    sys.exit(1)
write("s-1", "Preparing", None)
say(event="starting", app_id=438100)
time.sleep(1.5)  # the admission is seen well before it runs
write("s-1", "Running", {"pid": 4242})
if how.get("dies"):
    sys.exit(1)
say(event="running", app_id=438100, pid=4242, session_id="s-1")
link = sys.argv[-1]
log = open(f"{root}/vrchat/output_log_1.txt", "w")
log.write(f"Arg: {link}\n[Behaviour] Joining {how['joins']}\n[Behaviour] OnPlayerJoinComplete Some One\n[Behaviour] Initialized PlayerAPI \"Some One\" is local\n")
log.flush()
engine, n = open(f"{root}/engine.log", "w"), 0
while not os.path.exists(f"{root}/quit"):
    time.sleep(0.2)
    n += 12
    engine.write(f"neptune unixlib wsi (wsd): {n} presents (0 shown events), last 60: interval avg 16.6 ms, fence wait avg 8.0 ms, x\n")
    stamp = datetime.datetime.now().strftime("%Y-%m-%d %H:%M:%S.%f")[:-3]
    engine.write(f"{stamp} wsd[500:1] metal-HUD: 12,1,1," + ",".join(["16.7,0.1"] * 12) + "\n")
    engine.flush()
if how.get("stays"):
    sys.exit(0)
write("s-1", "Finished", None)
say(event="exited", app_id=438100, session_id="s-1", exit_code=0)
"""


class WholeRun(unittest.TestCase):
    """measure() against a program that answers as obelisk-cli does; no game, nothing of this Mac's own."""

    def run_with(self, deadline=8, **how):
        with tempfile.TemporaryDirectory() as root:
            for folder in ("records", "vrchat"):
                os.mkdir(f"{root}/{folder}")
            with open(f"{root}/how.json", "w") as f:
                json.dump({"joins": HERE} | how, f)
            cli = f"{root}/obelisk-cli"
            with open(cli, "w") as f:
                f.write(FAKE_CLI.replace("ROOT,", repr(root) + ",").replace("ROOT +", repr(root) + " +"))
            os.chmod(cli, 0o755)
            patches = {
                "ENGINE_LOG": f"{root}/engine.log", "VRC_LOGS": f"{root}/vrchat", "RECORDS": f"{root}/records",
                "START_DEADLINE": deadline, "ARRIVE_DEADLINE": 4, "CLOSE_DEADLINE": 4, "SETTLE_DEADLINE": 3,
                "settled": lambda lines, at: not how.get("never_calm"),
                "screen_locked": lambda: False, "frontmost": lambda: 500, "tree": lambda listing, pid: {pid: "bash", 500: "wsd"},
            }
            kept = {name: getattr(bench, name) for name in patches}
            argv, sys.argv = sys.argv, ["bench.py", "--cli", cli, "--link", LINK, "--warm", "0", "--window", "3", "--windows", "1", "--no-sample", "--out", f"{root}/out.json"]
            try:
                for name, value in patches.items():
                    setattr(bench, name, value)
                with open(os.devnull, "w") as quiet, contextlib.redirect_stdout(quiet):
                    status = bench.main()
            finally:
                sys.argv = argv
                for name, value in kept.items():
                    setattr(bench, name, value)
            with open(f"{root}/out.json") as f:
                return status, json.load(f), os.path.exists(f"{root}/quit")

    def test_a_start_admitted_before_it_runs_is_measured(self):
        # 2026-11-23 11:51, a real start: admitted, then running, and "did not start: []" 900 s later, the game untouched.
        status, res, quit = self.run_with()
        self.assertEqual((status, res["outcome"], res["session"]), (0, "measured", "s-1"), res["outcome"])
        self.assertEqual(len(res["windows"]), 1)
        self.assertTrue(res["windows"][0]["valid"], res["windows"][0]["invalid_because"])
        self.assertTrue(quit)
        self.assertEqual(res["terminal"]["phase"], "Finished")

    def test_each_run_that_does_not_go_as_planned_ends_its_session_and_says_so(self):
        for how, expected, outcome in (
            ({"joins": f"{OTHER}:1"}, 3, "did not arrive"),
            ({"never_calm": True}, 4, "did not settle"),
            ({"fails": True}, 2, "did not start"),
            # The program died once its game ran: the session is known from its admission, and asked to quit; here nothing
            # ends it.
            ({"dies": True}, 5, "did not start"),
            ({"stays": True}, 5, "measured"),
        ):
            status, res, quit = self.run_with(**how)
            self.assertEqual(status, expected, (how, res["outcome"]))
            self.assertIn(outcome, res["outcome"], how)
            self.assertEqual(quit, "fails" not in how, how)

    def test_running_is_the_program_s_word(self):
        session = {"id": "a", "running_at": None}
        bench.told(session, {"event": "starting"}, 1.0)
        self.assertEqual(session, {"id": "a", "running_at": None})
        bench.told(session, {"event": "running", "session_id": "b"}, 2.0)
        self.assertEqual(session, {"id": "b", "running_at": 2.0})
        bench.told(session, {"event": "running", "session_id": "c"}, 3.0)
        self.assertEqual(session, {"id": "b", "running_at": 2.0})
        older = {"id": "a", "running_at": None}
        bench.told(older, {"event": "running", "session_id": None}, 2.0)
        self.assertEqual(older, {"id": "a", "running_at": 2.0})


class Series(unittest.TestCase):
    def test_the_series_stops_after_a_run_that_failed_or_did_not_end(self):
        with tempfile.TemporaryDirectory() as root:
            here = os.path.join(root, "tools/mac-bench")
            os.makedirs(here)
            shutil.copy(os.path.join(os.path.dirname(os.path.abspath(__file__)), "session.sh"), here)
            script = open(f"{here}/session.sh").read().replace("/usr/bin/pmset", "/usr/bin/true").replace("sleep 90", "sleep 0").replace("/usr/sbin/ioreg", "/usr/bin/true")
            open(f"{here}/session.sh", "w").write(script)
            for code, runs in ((0, 3), (2, 1), (3, 1), (4, 1), (5, 1)):
                open(f"{here}/bench.py", "w").write(f"open('{root}/runs', 'a').write('x'); raise SystemExit({code})")
                done = subprocess.run(["/bin/bash", f"{here}/session.sh", LINK], env={"PATH": "/usr/bin:/bin", "PLAN": "58 100 87"}, capture_output=True, text=True)
                self.assertEqual(done.returncode, code, done.stderr)
                self.assertEqual(len(open(f"{root}/runs").read()), runs, code)
                os.remove(f"{root}/runs")


if __name__ == "__main__":
    unittest.main()
