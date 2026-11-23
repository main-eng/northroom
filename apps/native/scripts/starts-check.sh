#!/bin/sh
# Checks that an installed app starts and draws its window. Runs <program> for a few seconds on
# the scratch drive with its own app folder, catalog and trash, and passes if a window
# snapshot lands in NR_SNAPSHOT_DIR. The package workflows run it on the setup and AppImage.
#
# Usage: scripts/starts-check.sh <program> <folder>   (<folder> must be under a system temp
#        folder; stderr is left in <folder>/stderr.txt)
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
program=${1:?usage: starts-check.sh <program> <folder>}
out=${2:?usage: starts-check.sh <program> <folder>}
seconds=${NR_START_SECONDS:-12}
rm -rf "$out"
mkdir -p "$out/support" "$out/metadata" "$out/trash" "$out/drive" "$out/picture"
out=$(cd "$out" && pwd -P)
sh "$here/scripts/scratch-drive.sh" "$out/drive" >/dev/null
NR_APP="$program" NR_SCRATCH="$out" NR_SUPPORT="$out/support" NR_METADATA="$out/metadata" \
  NR_TRASH="$out/trash" NR_WORKSPACE="$out/drive" NR_DO=board NR_SNAPSHOT_DIR="$out/picture" \
  NR_SNAPSHOT_AFTER=3 sh "$here/scripts/native-run.sh" "$seconds" "$out/stderr.txt"
if [ -f "$out/picture/window.png" ]; then
  echo "starts-check: the app started and drew its window"
else
  echo "starts-check: the app drew no window in $seconds seconds; its error output:" >&2
  cat "$out/stderr.txt" >&2
  exit 1
fi
