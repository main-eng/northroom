#!/bin/sh
# Builds the Mac app (ADR-0001), bundles it as com.northroom.app, signs it with a Developer ID
# and the hardened runtime, notarises and staples it, checks it with Gatekeeper, and writes the
# feed files: the zip and native-mac.yml (electron-builder's channel format). Writes only to
# <output folder> and uploads nothing.
#
#   NR_SIGN_IDENTITY="Developer ID Application: <name> (<team id>)" \
#   NR_NOTARY_PROFILE=<profile stored with xcrun notarytool store-credentials> \
#     scripts/release-mac.sh <output folder>
#
#   scripts/release-mac.sh --ad-hoc <output folder>
#     Signs ad hoc instead, skips notarisation and stapling, and only reports Gatekeeper's
#     refusal. Output is marked NOT-FOR-RELEASE and the unzipped app is deleted, so Launch
#     Services never finds a second com.northroom.app.
#
# Where the files go and what this can't check: DECISIONS.md, ADR-0010.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
refuse() { echo "release-mac.sh: $1; nothing was made" >&2; exit 2; }

mode=release
if [ "${1:-}" = --ad-hoc ]; then
  mode=ad-hoc
  shift
fi
out=${1:-}
[ -n "$out" ] || refuse "usage: release-mac.sh [--ad-hoc] <output folder>"
[ $# -eq 1 ] || refuse "usage: release-mac.sh [--ad-hoc] <output folder>"

if [ "$mode" = release ]; then
  [ -n "${NR_SIGN_IDENTITY:-}" ] || refuse "NR_SIGN_IDENTITY is not set (\"Developer ID Application: <name> (<team id>)\")"
  [ -n "${NR_NOTARY_PROFILE:-}" ] || refuse "NR_NOTARY_PROFILE is not set (a profile stored with xcrun notarytool store-credentials)"
  case "$NR_SIGN_IDENTITY" in
    "Developer ID Application: "*) ;;
    *) refuse "NR_SIGN_IDENTITY must be a Developer ID Application identity, not '$NR_SIGN_IDENTITY'" ;;
  esac
  # Read-only check that a valid code-signing identity is in a keychain.
  security find-identity -v -p codesigning | grep -F "\"$NR_SIGN_IDENTITY\"" >/dev/null \
    || refuse "no valid code-signing identity named '$NR_SIGN_IDENTITY' (security find-identity -v -p codesigning)"
  identity=$NR_SIGN_IDENTITY
else
  identity=-
fi

mkdir -p "$out"
out=$(cd "$out" && pwd -P)
[ -z "$(ls -A "$out")" ] || refuse "$out is not empty"
log="$out/release.log"
say() { echo "$*" | tee -a "$log"; }
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/core/Cargo.toml" | head -1)
[ -n "$version" ] || refuse "no version in core/Cargo.toml"
say "Northroom $version ($mode), $(date -u +%Y-%m-%dT%H:%M:%SZ)"

# --- Build and bundle -------------------------------------------------------------------------
"$here/scripts/with-build-lock.sh" sh "$here/scripts/build.sh" >>"$log" 2>&1 || refuse "the build failed (see $log)"
NR_RELEASE=1 NR_SIGN_IDENTITY=$identity "$here/scripts/with-build-lock.sh" sh "$here/scripts/bundle.sh" "$out" >>"$log" 2>&1 \
  || refuse "the bundle failed (see $log)"
app="$out/Northroom.app"
arches=$(lipo -archs "$app/Contents/MacOS/Northroom")
case "$arches" in
  arm64) arch=arm64 ;;
  x86_64) arch=x64 ;;
  *) arch=universal ;;
esac
say "bundle: $app ($arches)"

# --- Check the signature ----------------------------------------------------------------------
fail() { say "FAIL  $1"; exit 1; }
codesign --verify --deep --strict --verbose=2 "$app" >>"$log" 2>&1 || fail "codesign --verify --deep --strict"
say "ok    codesign --verify --deep --strict"
signature=$(codesign -d --verbose=4 "$app" 2>&1)
echo "$signature" | grep -q '^Identifier=com\.northroom\.app$' || fail "the identifier is not com.northroom.app"
echo "$signature" | grep -q 'flags=.*runtime' || fail "the app is not signed with the hardened runtime"
entitlements=$(codesign -d --entitlements - --xml "$app" 2>/dev/null)
if echo "$entitlements" | grep -q 'get-task-allow'; then fail "the app may be debugged (get-task-allow), which notarisation refuses"; fi
if echo "$entitlements" | grep -q 'disable-library-validation'; then fail "the app must keep library validation"; fi
if [ "$mode" = release ]; then
  echo "$signature" | grep -q '^Timestamp=' || fail "the signature has no secure timestamp"
  team=$(echo "$signature" | sed -n 's/^TeamIdentifier=//p')
  say "ok    hardened runtime, secure timestamp and team $team throughout"
else
  say "ok    hardened runtime throughout (ad hoc: no timestamp, no team)"
fi

# --- Notarise and staple ----------------------------------------------------------------------
if [ "$mode" = release ]; then
  submission="$out/notarise.zip"
  ditto -c -k --keepParent "$app" "$submission"
  say "notarising (xcrun notarytool submit --wait)…"
  xcrun notarytool submit "$submission" --keychain-profile "$NR_NOTARY_PROFILE" --wait --output-format json >"$out/notarytool.json" 2>>"$log" \
    || fail "notarytool could not submit (see $log)"
  status=$(sed -n 's/.*"status" *: *"\([^"]*\)".*/\1/p' "$out/notarytool.json" | head -1)
  id=$(sed -n 's/.*"id" *: *"\([^"]*\)".*/\1/p' "$out/notarytool.json" | head -1)
  if [ "$status" != Accepted ]; then
    xcrun notarytool log "$id" --keychain-profile "$NR_NOTARY_PROFILE" "$out/notarytool-log.json" >>"$log" 2>&1 || true
    fail "notarisation $id ended $status (its log: $out/notarytool-log.json)"
  fi
  rm -f "$submission"
  say "ok    notarised ($id)"
  xcrun stapler staple "$app" >>"$log" 2>&1 || fail "stapler staple"
  xcrun stapler validate "$app" >>"$log" 2>&1 || fail "stapler validate"
  say "ok    ticket stapled"
fi

# --- What Gatekeeper says ---------------------------------------------------------------------
verdict=$(spctl --assess --type execute --verbose=4 "$app" 2>&1 || true)
echo "$verdict" >>"$log"
if [ "$mode" = release ]; then
  echo "$verdict" | grep -q 'accepted' || fail "Gatekeeper refuses the app: $verdict"
  echo "$verdict" | grep -q 'source=Notarized Developer ID' || fail "Gatekeeper does not see a notarised Developer ID app: $verdict"
  say "ok    spctl: accepted, notarised Developer ID"
else
  say "note  spctl (expected to refuse an ad-hoc app): $(echo "$verdict" | tr '\n' ' ')"
fi
codesign --verify --deep --strict "$app" || fail "codesign --verify after stapling"

# --- The feed's files -------------------------------------------------------------------------
# electron-builder's format, read by core/src/update/feed.rs. The updater unpacks the zip with
# ditto, so ditto makes it.
zip="Northroom-$version-$arch.zip"
ditto -c -k --keepParent "$app" "$out/$zip"
sum=$(shasum -a 512 -b "$out/$zip" | cut -d' ' -f1 | xxd -r -p | base64)
size=$(stat -f %z "$out/$zip")
minimum=$(/usr/libexec/PlistBuddy -c "Print :LSMinimumSystemVersion" "$app/Contents/Info.plist")
cat >"$out/native-mac.yml" <<YML
version: $version
files:
  - url: $zip
    sha512: $sum
    size: $size
path: $zip
sha512: $sum
releaseDate: '$(date -u +%Y-%m-%dT%H:%M:%S.000Z)'
minimumSystemVersion: '$minimum'
YML
say "ok    $zip ($size bytes) and native-mac.yml"

if [ "$mode" = ad-hoc ]; then
  rm -rf "$app"
  for file in "$out/$zip" "$out/native-mac.yml"; do mv "$file" "$file.NOT-FOR-RELEASE"; done
  say "ad hoc: the app was removed; the zip and channel file are marked NOT-FOR-RELEASE"
else
  say "Done. Upload $zip and native-mac.yml to the newest release, beside the other platforms' files (DECISIONS.md, ADR-0010)."
fi
