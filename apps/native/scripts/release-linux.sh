#!/bin/sh
# Builds and packages the Linux AppImage, signs it with the maintainer's minisign key, checks
# the signature against the app's key (core/src/update/update-key.pub), and writes the feed
# files: the AppImage, native-linux.yml (electron-builder's channel format, carrying the
# signature) and a .minisig for manual checks. Writes only to <output folder> and uploads nothing.
#
#   NR_MINISIGN_KEY=<your minisign secret key file> scripts/release-linux.sh <output folder>
#     On Linux: build, package, sign. minisign asks for the key's password.
#
#   NR_MINISIGN_KEY=… scripts/release-linux.sh --from <AppImage> <output folder>
#     Anywhere minisign runs: signs an AppImage built elsewhere, such as by the "Native packages"
#     workflow on the release commit, and writes the feed files.
#
#   scripts/release-linux.sh --unsigned <output folder>
#     On Linux, for CI and dry runs: build and package without signing. Output is marked
#     NOT-FOR-RELEASE.
#
# Where the files go and what this can't check: DECISIONS.md, ADR-0010.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
refuse() { echo "release-linux.sh: $1; nothing was made" >&2; exit 2; }
usage="usage: release-linux.sh [--unsigned | --from <AppImage>] <output folder>"

mode=release
from=
case "${1:-}" in
  --unsigned) mode=unsigned; shift ;;
  --from) from=${2:-}; [ -n "$from" ] || refuse "$usage"; shift 2 ;;
esac
out=${1:-}
[ -n "$out" ] && [ $# -eq 1 ] || refuse "$usage"
key="$here/core/src/update/update-key.pub"
if [ "$mode" = release ]; then
  command -v minisign >/dev/null || refuse "minisign is not installed (brew install minisign, apt install minisign)"
  [ -n "${NR_MINISIGN_KEY:-}" ] && [ -f "$NR_MINISIGN_KEY" ] || refuse "NR_MINISIGN_KEY does not name the minisign secret key file"
  [ -n "$(tr -d '[:space:]' < "$key")" ] || refuse "the app has no update key yet ($key): the app would refuse every update"
fi
if [ -n "$from" ]; then
  [ -f "$from" ] || refuse "no AppImage at $from"
elif [ "$(uname -s)" != Linux ]; then
  refuse "building needs Linux (or sign an AppImage built elsewhere with --from)"
fi

mkdir -p "$out"
out=$(cd "$out" && pwd)
[ -z "$(ls -A "$out")" ] || refuse "$out is not empty"
log="$out/release.log"
say() { echo "$*" | tee -a "$log"; }
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/core/Cargo.toml" | head -1)
[ -n "$version" ] || refuse "no version in core/Cargo.toml"
name="Northroom-$version-x86_64.AppImage"
say "Northroom $version for Linux ($mode), $(date -u +%Y-%m-%dT%H:%M:%SZ)"
fail() { say "FAIL  $1"; exit 1; }

# --- Build and package, or take the AppImage given --------------------------------------------
if [ -n "$from" ]; then
  [ "$(basename "$from")" = "$name" ] || refuse "$from is not named $name (core/Cargo.toml's version)"
  cp "$from" "$out/$name"
else
  "$here/scripts/with-build-lock.sh" sh "$here/scripts/build.sh" >>"$log" 2>&1 || refuse "the build failed (see $log)"
  NR_RELEASE=1 NR_APPIMAGE_RUNTIME="${NR_APPIMAGE_RUNTIME:-}" sh "$here/scripts/package-linux.sh" "$out/package" >>"$log" 2>&1 \
    || refuse "packaging failed (see $log)"
  mv "$out/package/$name" "$out/$name"
  rm -rf "$out/package"
fi
# Read release.json from the SquashFS without running the AppImage. The runtime's code also
# contains the "hsqs" magic, so try each match until one opens.
if command -v unsquashfs >/dev/null 2>&1; then
  offsets=$(LC_ALL=C grep -obUaP 'hsqs' "$out/$name" 2>/dev/null | cut -d: -f1 || true)
  if [ -n "$offsets" ]; then
    said=
    for offset in $offsets; do
      said=$(unsquashfs -o "$offset" -cat "$out/$name" app/Resources/release.json 2>/dev/null) && break
      said=
    done
    echo "$said" | grep -q "\"version\":\"$version\",\"identifier\":\"com.northroom.app\"" || fail "the AppImage says $said, not $version com.northroom.app"
    say "ok    the AppImage is com.northroom.app $version"
  fi
fi

# --- Sign and check ---------------------------------------------------------------------------
if [ "$mode" = release ]; then
  minisign -S -s "$NR_MINISIGN_KEY" -m "$out/$name" -x "$out/$name.minisig" -t "file:$name version:$version" || fail "minisign could not sign"
  # The app accepts only prehashed (BLAKE2b) signatures. minisign before 0.10 needs -H for that.
  if [ "$(sed -n 2p "$out/$name.minisig" | cut -c1-2)" != RU ]; then
    minisign -S -H -s "$NR_MINISIGN_KEY" -m "$out/$name" -x "$out/$name.minisig" -t "file:$name version:$version" || fail "minisign could not sign"
  fi
  [ "$(sed -n 2p "$out/$name.minisig" | cut -c1-2)" = RU ] || fail "the signature is minisign's legacy kind; use minisign 0.10 or later"
  minisign -V -q -p "$key" -m "$out/$name" -x "$out/$name.minisig" || fail "the signature does not check against the app's key ($key)"
  say "ok    signed, and checked with the key built into the app"
  signature=$(base64 < "$out/$name.minisig" | tr -d '\n')
fi

# --- The feed's files -------------------------------------------------------------------------
sum=$(python3 -I -c 'import base64,hashlib,sys; print(base64.b64encode(hashlib.sha512(open(sys.argv[1],"rb").read()).digest()).decode())' "$out/$name")
size=$(wc -c < "$out/$name" | tr -d ' ')
{
  echo "version: $version"
  echo "files:"
  echo "  - url: $name"
  echo "    sha512: $sum"
  echo "    size: $size"
  [ "$mode" = release ] && echo "    signature: $signature"
  echo "path: $name"
  echo "sha512: $sum"
  echo "releaseDate: '$(date -u +%Y-%m-%dT%H:%M:%S.000Z)'"
} > "$out/native-linux.yml"
say "ok    $name ($size bytes) and native-linux.yml"

if [ "$mode" = unsigned ]; then
  for file in "$out/$name" "$out/native-linux.yml"; do mv "$file" "$file.NOT-FOR-RELEASE"; done
  say "unsigned: the AppImage and channel file are marked NOT-FOR-RELEASE"
else
  say "Done. Upload $name, $name.minisig and native-linux.yml to the newest release, beside the other platforms' files (DECISIONS.md, ADR-0010)."
fi
