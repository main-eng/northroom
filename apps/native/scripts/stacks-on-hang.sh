#!/bin/sh
# Runs a command. If it is still running after <seconds>, prints every thread's stack in every
# process it started, kills them and fails, so a CI hang shows where it is stuck. Linux only;
# attaches gdb through sudo.
#
#   scripts/stacks-on-hang.sh <seconds> <command> [arguments...]
set -eu
[ $# -ge 2 ] || { echo "usage: stacks-on-hang.sh <seconds> <command> [arguments...]" >&2; exit 2; }
limit=$1
shift

# All descendants of process `$1`, one per line.
descendants() {
  for child in $(pgrep -P "$1" || true); do
    echo "$child"
    descendants "$child"
  done
}

"$@" &
run=$!
waited=0
while kill -0 "$run" 2>/dev/null; do
  if [ "$waited" -ge "$limit" ]; then
    echo "stacks-on-hang.sh: still running after ${limit}s: $*" >&2
    started=$(descendants "$run")
    for pid in $run $started; do
      echo "== process $pid: $(cat "/proc/$pid/cmdline" 2>/dev/null | tr '\0' ' ')" >&2
      sudo gdb -p "$pid" -batch -ex "thread apply all bt" 2>&1 \
        | grep -v -e '^\[New LWP' -e '^\[Thread debugging' -e '^Using host libthread_db' >&2 || true
    done
    # shellcheck disable=SC2086 # split the ids on purpose
    kill -KILL $started "$run" 2>/dev/null || true
    exit 124
  fi
  sleep 5
  waited=$((waited + 5))
done
wait "$run"
