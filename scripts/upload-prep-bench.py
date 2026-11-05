"""Run the ignored release preparation test without PowerShell reformatting stderr."""

import argparse
import ctypes
from ctypes import wintypes
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def main():
    sys.stdout.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--profile", action="store_true")
    parser.add_argument("--test", default="upload_prep_timings")
    parser.add_argument("sessions", nargs="+")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, ORBITERS_TEST_SESSION=";".join(args.sessions),
               ORBITERS_PREP_TIMING="1", ORBITERS_PREP_OUT=str(args.out / "files"))
    if args.profile:
        env["ORBITERS_PREP_PROFILE"] = "1"
    else:
        env.pop("ORBITERS_PREP_PROFILE", None)
    started = time.perf_counter()
    with (args.out / "run.log").open("w", encoding="utf-8") as log:
        with subprocess.Popen([args.binary, args.test, "--ignored", "--nocapture",
                               "--test-threads=1"], env=env, stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True, encoding="utf-8") as process:
            for line in process.stdout:
                log.write(line)
                log.flush()
                print(line, end="", flush=True)
            code = process.wait()
            times = [wintypes.FILETIME() for _ in range(4)]
            get_times = ctypes.windll.kernel32.GetProcessTimes
            get_times.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
            if not get_times(wintypes.HANDLE(int(process._handle)), *(ctypes.byref(t) for t in times)):
                raise ctypes.WinError()
            seconds = lambda t: ((t.dwHighDateTime << 32) | t.dwLowDateTime) / 10_000_000
            counters = {"wall_seconds": time.perf_counter() - started,
                        "cpu_kernel_seconds": seconds(times[2]), "cpu_user_seconds": seconds(times[3]),
                        "exit_code": code, "profile": args.profile}
            (args.out / "process.json").write_text(json.dumps(counters, indent=2), encoding="utf-8")
            print(json.dumps(counters), flush=True)
            raise SystemExit(code)


if __name__ == "__main__":
    main()
