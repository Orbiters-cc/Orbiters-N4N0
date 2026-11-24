#!/bin/bash
# The Raspberry Pi's build of the Linux port, installed there as ~/n4n0-linux/bin/n4n0-build (scripts/linux/pi-setup.sh).
# Every cargo command there goes through it: one at a time, capped, as a transient system unit (a dropped SSH session neither
# kills it nor frees the turn). The Pi also runs Hermes and a docker container: a build never swaps, dies at its cap, and is stopped
# when the Pi has little memory left for them.
#
#   n4n0-build <stream> <branch> <cargo args...>     exit code = cargo's; 75 STOP, 76 no turn, 3/4 low disk/memory, 5 refused
#
# To stop everything: touch ~/n4n0-linux/STOP; sudo systemctl stop n4n0-build
set -u
B=$HOME/n4n0-linux stream=$1 ref=$2 U=n4n0-build
shift 2
case " $* " in *" -p orbiters-desktop "* | *" --workspace "* | *" --all "*) echo "orbiters-desktop does not fit here" >&2; exit 5 ;; esac
case " $* " in *" -p "* | " --version " | " fetch "*) ;; *) echo "name the crates (-p ...)" >&2; exit 5 ;; esac
[ -e "$B/STOP" ] && exit 75
exec 9>"$B/lock"
echo "$(date -Is) $stream waits: $*" >>"$B/logs/queue.log"
flock -w 14400 9 || exit 76                                  # the line
while systemctl is-active -q "$U.service"; do sleep 5; done  # a build whose caller's SSH dropped still holds the turn
[ -e "$B/STOP" ] && exit 75
[ "$(df --output=avail -BG / | tail -1 | tr -dc 0-9)" -ge 30 ] || { echo LOW DISK >&2; exit 3; }
# Hermes' own jobs come and go: a build waits up to an hour for room rather than squeezing them.
for _ in $(seq 120); do
  [ "$(awk '/MemAvailable/{print int($2/1024)}' /proc/meminfo)" -ge "${N4N0_LEAST_MB:-1500}" ] && roomy=1 && break
  [ -e "$B/STOP" ] && exit 75
  sleep 30
done
[ -n "${roomy:-}" ] || { echo LOW MEMORY >&2; exit 4; }
w=$B/repo/work
[ -d "$w/.git" ] || git clone -q "$B/n4n0.git" "$w" || exit 1
git -C "$w" fetch -q origin && git -C "$w" checkout -q --detach "origin/$ref" || exit 1
git -C "$w" -c protocol.file.allow=always -c submodule.contracts.url="$B/contracts.git" submodule update -q --init contracts || exit 1
log=$B/logs/$stream.log
: >"$log"
echo "$(date -Is) $stream runs ($(git -C "$w" rev-parse --short HEAD)): $*" >>"$B/logs/queue.log"
# What is left for the rest of the Pi while it builds: under 600 MB three times running, the build is stopped (its crates so far
# are kept, the next one goes on from there).
(
  low=0
  sleep 10
  while systemctl is-active -q $U.service; do
    if [ "$(awk '/MemAvailable/{print int($2/1024)}' /proc/meminfo)" -lt 600 ]; then low=$((low + 1)); else low=0; fi
    [ $low -ge 3 ] && { echo "$(date -Is) $stream stopped: little memory left" >>"$B/logs/queue.log"; sudo systemctl stop $U.service; }
    sleep 5
  done
) 9>&- &
before=$(awk '/SwapFree/{print $2}' /proc/meminfo)
sudo systemd-run --quiet --collect --wait --unit=$U --uid="$(id -u)" --gid="$(id -g)" --working-directory="$w" \
  -p StandardOutput=append:"$log" -p StandardError=append:"$log" \
  -p MemoryHigh=1400M -p MemoryMax=1700M -p MemorySwapMax=0 -p OOMScoreAdjust=900 -p CPUWeight=20 -p CPUQuota=300% \
  -p Nice=15 -p IOSchedulingClass=idle -p TasksMax=512 -p RuntimeMaxSec=6h \
  -E HOME="$B/home" -E TMPDIR="$B/tmp" -E XDG_RUNTIME_DIR="$B/tmp" -E RUSTUP_HOME="$B/rustup" -E CARGO_HOME="$B/cargo" -E CARGO_TARGET_DIR="$B/target" \
  -E PATH="$B/cargo/bin:/usr/bin:/bin" -E CARGO_BUILD_JOBS="${N4N0_JOBS:-2}" -E CARGO_INCREMENTAL=0 -E CARGO_PROFILE_DEV_DEBUG=0 \
  -E RUSTFLAGS="-C link-arg=-fuse-ld=mold" -E ORBITERS_NO_NETWORK_LISTENING=1 \
  -E VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.aarch64.json "$B/cargo/bin/cargo" "$@"
rc=$?
echo "$(date -Is) $stream exit $rc (swap used meanwhile $(((before - $(awk '/SwapFree/{print $2}' /proc/meminfo)) / 1024)) MB)" >>"$B/logs/queue.log"
exit $rc
