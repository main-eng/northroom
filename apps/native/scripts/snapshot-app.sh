#!/bin/sh
# Copies the current build of gpui/target/release/northroom-gpui into <folder> as Northroom,
# so a check keeps running the same binary while other builds replace it. The core is linked
# in and the page host's files are read from the source tree, so nothing else is copied. The
# build lock is held during the copy so it never catches a half-written binary.
#
# Usage: scripts/snapshot-app.sh <folder>      prints the copied program's path
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
into=${1:?usage: snapshot-app.sh <folder>}
built="$here/gpui/target/release"
mkdir -p "$into"
into=$(cd "$into" && pwd -P)
case "$into" in
  /private/tmp/?*|/private/var/folders/?*) ;;
  *) echo "snapshot-app.sh: $into is not under the system's temporary folders" >&2; exit 2 ;;
esac
rm -rf "$into/app.writing"
mkdir -p "$into/app.writing"
"$here/scripts/with-build-lock.sh" sh -c '
  set -eu
  [ -f "$1/northroom-gpui" ] || { echo "snapshot-app.sh: no $1/northroom-gpui (run scripts/build.sh first)" >&2; exit 2; }
  cp "$1/northroom-gpui" "$2/Northroom"
' snapshot "$built" "$into/app.writing"
rm -rf "$into/app"
mv "$into/app.writing" "$into/app"
echo "$into/app/Northroom"
