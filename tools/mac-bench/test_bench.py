#!/usr/bin/env python3
"""Checks of bench.py's scoring (fourth verification, finding 11): python3 -I tools/mac-bench/test_bench.py"""
import os, sys, unittest

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

    def test_a_window_without_shown_frames_or_behind_a_lock_is_not_scored(self):
        lines = [(1.0, npt(0)), (2.0, npt(60)), (2.5, "metal-HUD: 1,2,3,16.6,0,16.7,0")]
        unlocked = {"gpu": [], "cpu": [], "lock": [(1.5, False, "VRChat")]}
        good = bench.summarize(lines, 0.0, 3.0, unlocked, False)
        self.assertTrue(good["valid"], good["invalid_because"])
        locked = {"gpu": [], "cpu": [], "lock": [(1.5, True, "loginwindow")]}
        bad = bench.summarize(lines[:2], 0.0, 3.0, locked, False)
        self.assertFalse(bad["valid"])
        self.assertFalse(bad["production_only"])
        self.assertTrue(any("locked" in w for w in bad["invalid_because"]))
        self.assertTrue(any("no frame was shown" in w for w in bad["invalid_because"]))
        prod = bench.summarize(lines[:2], 0.0, 3.0, locked, True)
        self.assertTrue(prod["production_only"], "allowed explicitly: production only, and said")
        self.assertEqual(prod["wsd_shown"]["per_second"], 0.0, "zero shown is measured as zero")


if __name__ == "__main__":
    unittest.main()
