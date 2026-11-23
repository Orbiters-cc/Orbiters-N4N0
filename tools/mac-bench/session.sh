#!/bin/bash
# The next measurement session (docs/mac-performance.md, "The next session"), in one command, for the owner at his Mac:
#
#   tools/mac-bench/session.sh 'vrchat://launch?ref=orbiters.cc&id=wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd:<5 digits>~private(<his usr_ id>)~region(eu)'
#
# Plays the engine percents of PLAN (default: 58 100 87 67 45 87 100 58: every condition, the main three twice, in an
# alternated order) as eight starts at most, one results file each in docs/mac-bench-results/, waiting at least 90 s
# after each start ended (Easy Anti-Cheat fails a start too soon after an early end). Before each start it refuses to go
# on while the screen is locked (only the owner can unlock it) and says Low Power Mode's state; each window is scored
# only when the screen stayed unlocked, VRChat was in front and frames were shown (bench.py). Nobody touches the game
# while it runs: each start takes about 6 minutes. A start that fails, or whose session is not known to have ended, stops the
# series. Stop with Ctrl-C: the running start is quit by its session.
set -u
LINK="${1:?usage: session.sh '<private instance link of the owner>'}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
PLAN="${PLAN:-58 100 87 67 45 87 100 58}"
OUT="$ROOT/docs/mac-bench-results"
mkdir -p "$OUT"
n=0
for percent in $PLAN; do
    n=$((n + 1))
    if [ "$n" -gt 8 ]; then echo "eight starts at most: stopping"; break; fi
    if /usr/sbin/ioreg -n Root -d1 -a | grep -A1 IOConsoleLocked | grep -q true; then
        echo "The screen is locked: unlock it, then run this again (nothing measured)." >&2
        exit 1
    fi
    /usr/bin/pmset -g custom | grep -i lowpowermode
    stamp="$(date +%Y-%m-%d_%H-%M)"
    echo "== start $n: engine $percent % =="
    /usr/bin/python3 -I "$HERE/bench.py" --percent "$percent" --label "$percent" --link "$LINK" \
        --warm 90 --window 60 --windows 3 --out "$OUT/$stamp-$percent.json"
    status=$?
    if [ "$status" -ne 0 ]; then
        # Nothing is started after a run that did not go as planned: its game may still run (5), or the next start would
        # fail the same way.
        echo "start $n failed (status $status, see $OUT/$stamp-$percent.json): stopping" >&2
        [ "$status" -eq 5 ] && echo "its session had not ended: check that VRChat is closed before anything else" >&2
        exit "$status"
    fi
    echo "waiting 90 s before the next start"
    sleep 90
done
echo "Done: the results are in $OUT; summarise with python3 -I $HERE/summary.py $OUT/<date>*.json"
