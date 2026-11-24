#!/usr/bin/env bash
# scripts/linux/pi-bg.sh <stream> <branch> <cargo args...>: n4n0-build on the Pi (ssh host `pi`), detached, so that no tool call
# waits on SSH. Wait with pi-wait.sh <stream>. Only a commit pushed to the Pi's own repository is built:
#   git push ssh://pi/home/orbit/n4n0-linux/n4n0.git HEAD:refs/heads/<branch>
#   git -C contracts push ssh://pi/home/orbit/n4n0-linux/contracts.git HEAD:refs/heads/main
stream=$1
quoted=$(printf '%q ' "$@")
ssh pi "cd ~/n4n0-linux; rm -f logs/$stream.done; nohup setsid env N4N0_JOBS=${N4N0_JOBS:-2} bash -c 'bin/n4n0-build \"\$@\" > logs/$stream.out 2>&1; echo \"DONE \$?\" > logs/$stream.done' n4n0 $quoted > /dev/null 2>&1 < /dev/null &"
echo "asked: $stream"
