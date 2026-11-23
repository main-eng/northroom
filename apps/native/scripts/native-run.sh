#!/bin/sh
# Starts the native app for a check (ADR-0001). A check must never touch the real user's app
# data, so NR_SUPPORT (app folder), NR_METADATA (catalog), NR_TRASH (deletions) and
# NR_WORKSPACE (drive) must all be set and inside NR_SCRATCH, which must be under a system temp
# folder. The app is stopped after <seconds>; only the process this script started is killed.
#
# NR_APP is a copy from scripts/snapshot-app.sh, so a concurrent build can't change or remove
# it. Without NR_APP, the current build is copied into <NR_SCRATCH>/app-snapshot first.
#
# Usage: NR_SCRATCH=<folder> NR_SUPPORT=… NR_METADATA=… NR_TRASH=… NR_WORKSPACE=… [NR_APP=…] [NR_DO=… …] \
#          scripts/native-run.sh <seconds> <stderr-file>
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
seconds=${1:?usage: native-run.sh <seconds> <stderr-file>}
log=${2:?usage: native-run.sh <seconds> <stderr-file>}
refuse() { echo "native-run.sh: $1; nothing was started" >&2; exit 2; }
[ -n "${NR_SCRATCH:-}" ] && [ -d "$NR_SCRATCH" ] || refuse "NR_SCRATCH is not a folder"
scratch=$(cd "$NR_SCRATCH" && pwd -P)
# System temp folders on the Mac, Linux and Git Bash on Windows.
temporary=$(cd "${TMPDIR:-/tmp}" && pwd -P)
case "$scratch" in
  /private/tmp/?*|/private/var/folders/?*|/tmp/?*|"$temporary"/?*) ;;
  *) refuse "NR_SCRATCH ($scratch) is not under the system's temporary folders" ;;
esac
for name in NR_SUPPORT NR_METADATA NR_TRASH NR_WORKSPACE; do
  value=$(eval "printf '%s' \"\${$name:-}\"")
  [ -n "$value" ] || refuse "$name is not set"
  case "$value" in *..*) refuse "$name ($value) climbs out with .." ;; esac
  # Check a missing folder by its nearest existing parent.
  probe=$value
  while [ ! -d "$probe" ]; do probe=$(dirname "$probe"); done
  real=$(cd "$probe" && pwd -P)
  case "$real/" in
    "$scratch"/*) ;;
    *) refuse "$name ($value) is not inside NR_SCRATCH ($scratch)" ;;
  esac
done
app=${NR_APP:-}
if [ -z "$app" ]; then
  app=$(sh "$here/scripts/snapshot-app.sh" "$scratch/app-snapshot") || refuse "the app could not be copied"
fi
[ -x "$app" ] || refuse "NR_APP ($app) is not a program"
"$app" >/dev/null 2>"$log" &
started=$!
sleep "$seconds"
kill "$started" 2>/dev/null || true
wait "$started" 2>/dev/null || true
