#!/bin/sh
# Runs a build or test command under the machine-wide build lock, so parallel builds don't
# compile at once on a 16 GB machine (see AGENTS.md).
# Usage: scripts/with-build-lock.sh <command> [arguments...]
# The lock is a directory. One older than 20 minutes counts as abandoned.
set -u
lock="${TMPDIR:-/tmp}/northroom-native-build.lock"
while ! mkdir "$lock" 2>/dev/null; do
  if [ -n "$(find "$lock" -maxdepth 0 -mmin +20 2>/dev/null)" ]; then
    rmdir "$lock" 2>/dev/null || true
    continue
  fi
  sleep 5
done
trap 'rmdir "$lock" 2>/dev/null' EXIT INT TERM
"$@"
