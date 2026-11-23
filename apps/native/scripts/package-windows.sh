#!/bin/sh
# Makes the Windows setup from a release build of the GPUI app (ADR-0003), in Git Bash. The
# Windows counterpart of scripts/bundle.sh. The installed app folder:
#
#   Northroom.exe                 northroom-gpui with the core linked in, plus its icon and
#                                 version (gpui/shell/build.rs)
#   Resources/SceneHost/          the page host's pinned GSAP, three.js and fonts
#                                 (gpui/webhost/resources)
#   Resources/Licenses/           licences for everything copied in
#   Resources/release.json        version and identifier for the updater
#                                 (core/src/update/package.rs)
#
# The setup is scripts/windows/northroom.iss compiled by Inno Setup 6: a per-user install that
# needs no administrator rights and updates in place, named Northroom-<version>-x64.exe.
#
# Builds nothing: run scripts/build.sh first with RUSTFLAGS="-C target-feature=+crt-static", as
# scripts/release-windows.sh does, so no Visual C++ runtime is needed. The app may load only
# Windows system libraries.
#
#   NR_RELEASE=1        use the release identifier com.northroom.app. Otherwise
#                       com.northroom.app.dev, installed separately as "Northroom Dev" so it
#                       never updates a release install.
#   NR_SIGN_IDENTITY    SHA-1 thumbprint of a code-signing certificate in the user's store. Signs
#                       the app, the setup and its uninstaller. Unset: nothing is signed.
#   NR_SIGN_ALSO        a second thumbprint whose signature is appended, for a certificate
#                       renewal (DECISIONS.md, ADR-0010).
#   NR_SIGN_TIMESTAMP   RFC 3161 timestamp server, default DigiCert's; empty for none.
#   NR_VERSION          version, default the one in core/Cargo.toml.
# Usage: scripts/package-windows.sh <output folder>   (writes <output folder>/Northroom-<version>-x64.exe)
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
out=${1:?usage: package-windows.sh <output folder>}
# shellcheck source=windows/tools.sh
. "$here/scripts/windows/tools.sh"
build="$here/gpui/target/release"
resources="$here/gpui/webhost/resources"
[ -f "$build/northroom-gpui.exe" ] || { echo "no $build/northroom-gpui.exe: build first" >&2; exit 2; }
version=${NR_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/core/Cargo.toml" | head -1)}
[ -n "$version" ] || { echo "no version in core/Cargo.toml" >&2; exit 2; }
if [ "${NR_RELEASE:-}" = 1 ]; then identifier=com.northroom.app; else identifier=com.northroom.app.dev; fi

mkdir -p "$out"
out=$(cd "$out" && pwd)
app="$out/app"
rm -rf "$app"
mkdir -p "$app/Resources/Licenses"
cp "$build/northroom-gpui.exe" "$app/Northroom.exe"
cp -R "$resources" "$app/Resources/SceneHost"
licences="$app/Resources/Licenses"
cp "$resources/vendor/gsap-3.15.0/LICENSE.txt" "$licences/GSAP-3.15.0.txt"
cp "$resources/vendor/three-0.186.0/LICENSE" "$licences/three.js-0.186.0.txt"
cp "$resources/fonts/"OFL-*.txt "$licences/"
cp "$here/core/assets/licenses/Lucide.txt" "$licences/Lucide.txt"
printf '{"version":"%s","identifier":"%s"}\n' "$version" "$identifier" > "$app/Resources/release.json"

# Fail on any library that isn't part of Windows, including the Visual C++ runtime, which a
# clean Windows lacks.
dumpbin=$(dumpbin_path)
# A program's linked DLLs, one per line.
loads() { windows_run "$dumpbin" /nologo /dependents "$(cygpath -w "$1")" | tr -d '\r' | sed -n 's/^ \{4\}\([A-Za-z0-9_.-]*\.[dD][lL][lL]\)$/\1/p'; }
is_foreign() {
  case "$(echo "$1" | tr '[:upper:]' '[:lower:]')" in
    vcruntime*|msvcp*) return 0 ;;
    api-ms-win-*|ext-ms-win-*) return 1 ;;
  esac
  [ ! -f "${SYSTEMROOT:-/c/Windows}/System32/$1" ]
}
foreign=
for library in $(loads "$app/Northroom.exe" | sort -u); do
  if is_foreign "$library"; then foreign="$foreign $library"; fi
done
if [ -n "$foreign" ]; then
  echo "the programs load libraries that are not Windows's own:" >&2
  echo "$foreign" >&2
  exit 1
fi

signing=
if [ -n "${NR_SIGN_IDENTITY:-}" ]; then
  sign_files "$app/Northroom.exe"
  for program in "$app/Northroom.exe"; do
    windows_run "$(signtool_path)" verify /pa /q "$(cygpath -w "$program")" >/dev/null ||
      { echo "$program's signature does not verify" >&2; exit 1; }
  done
  signing=1
fi

name="Northroom-$version-x64"
rm -f "$out/$name.exe"
iscc=$(iscc_path)
set -- /Q "/DAppVersion=$version" "/DAppIdentifier=$identifier" "/DSource=$(cygpath -w "$app")" \
  "/DIcon=$(cygpath -w "$here/gpui/shell/Northroom.ico")" "/DOutputDir=$(cygpath -w "$out")" "/DOutputName=$name"
if [ -n "$signing" ]; then set -- "$@" /DSign "/Snr=$(sign_command)"; fi
windows_run "$iscc" "$@" "$(cygpath -w "$here/scripts/windows/northroom.iss")"
[ -f "$out/$name.exe" ] || { echo "Inno Setup wrote no $name.exe" >&2; exit 1; }
if [ -n "$signing" ] && [ -n "${NR_SIGN_ALSO:-}" ]; then
  # Inno Setup signed with the first certificate; append the second.
  signtool=$(signtool_path)
  timestamp=${NR_SIGN_TIMESTAMP-http://timestamp.digicert.com}
  stamp=
  [ -n "$timestamp" ] && stamp="/tr $timestamp /td sha256"
  # shellcheck disable=SC2086 # split the timestamp options on purpose
  windows_run "$signtool" sign /as /sha1 "$NR_SIGN_ALSO" /fd sha256 $stamp /q "$(cygpath -w "$out/$name.exe")"
fi
if [ -n "$signing" ]; then
  windows_run "$(signtool_path)" verify /pa /q "$(cygpath -w "$out/$name.exe")" >/dev/null ||
    { echo "the setup's signature does not verify" >&2; exit 1; }
fi
rm -rf "$app"
echo "$out/$name.exe ($(du -h "$out/$name.exe" | cut -f1))"
