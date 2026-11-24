#!/usr/bin/env bash
# scripts/linux/pi-wait.sh <stream> [seconds, default 50]: waits for a pi-bg.sh build, never longer than asked: DONE <exit code>, or
# WAITING with the log's last lines and what the Pi has left.
stream=$1 limit=${2:-50}
ssh pi "cd ~/n4n0-linux/logs
for ((i = 0; i < $limit; i += 5)); do
  [ -e $stream.done ] && { cat $stream.done; tail -n 15 $stream.log; exit 0; }
  sleep 5
done
echo WAITING; tail -n 3 $stream.log; tail -n 1 $stream.out queue.log; free -m | sed -n 2,3p; exit 1"
