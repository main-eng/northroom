#!/bin/sh
# Builds the Windows app with the C runtime linked in, packages the per-user setup, signs the
# app, setup and uninstaller with the maintainer's Authenticode certificate and a timestamp,
# checks the signatures, and writes the feed files: the setup and native-windows.yml
# (electron-builder's channel format). Run in Git Bash. Writes only to <output folder> and
# uploads nothing.
#
#   NR_SIGN_IDENTITY=<SHA-1 thumbprint of the code-signing certificate> \
#     [NR_SIGN_ALSO=<the renewed or old certificate's thumbprint>] \
#     scripts/release-windows.sh <output folder>
#
#   scripts/release-windows.sh --unsigned <output folder>
#     Skips signing, for CI and dry runs. Output is marked NOT-FOR-RELEASE.
#
# Where the files go and what this can't check: DECISIONS.md, ADR-0010.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
refuse() { echo "release-windows.sh: $1; nothing was made" >&2; exit 2; }
# shellcheck source=windows/tools.sh
. "$here/scripts/windows/tools.sh"

mode=release
if [ "${1:-}" = --unsigned ]; then
  mode=unsigned
  shift
fi
out=${1:-}
[ -n "$out" ] && [ $# -eq 1 ] || refuse "usage: release-windows.sh [--unsigned] <output folder>"
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) ;; *) refuse "run it in Git Bash on Windows" ;; esac

if [ "$mode" = release ]; then
  [ -n "${NR_SIGN_IDENTITY:-}" ] || refuse "NR_SIGN_IDENTITY is not set (the certificate's SHA-1 thumbprint)"
  # Read-only check for a valid code-signing certificate with its private key in the user store.
  found=$(powershell.exe -NoProfile -Command "Get-ChildItem Cert:\\CurrentUser\\My -CodeSigningCert | Where-Object { \$_.Thumbprint -eq '$NR_SIGN_IDENTITY' -and \$_.HasPrivateKey -and \$_.NotAfter -gt (Get-Date) } | ForEach-Object { \$_.Subject }" | tr -d '\r')
  [ -n "$found" ] || refuse "no valid code-signing certificate with thumbprint $NR_SIGN_IDENTITY and its private key in Cert:\\CurrentUser\\My"
  if [ -n "${NR_SIGN_ALSO:-}" ]; then
    also=$(powershell.exe -NoProfile -Command "Get-ChildItem Cert:\\CurrentUser\\My -CodeSigningCert | Where-Object { \$_.Thumbprint -eq '$NR_SIGN_ALSO' -and \$_.HasPrivateKey -and \$_.NotAfter -gt (Get-Date) } | ForEach-Object { \$_.Subject }" | tr -d '\r')
    [ -n "$also" ] || refuse "no valid code-signing certificate with thumbprint $NR_SIGN_ALSO (NR_SIGN_ALSO)"
  fi
else
  unset NR_SIGN_IDENTITY NR_SIGN_ALSO
fi

mkdir -p "$out"
out=$(cd "$out" && pwd)
[ -z "$(ls -A "$out")" ] || refuse "$out is not empty"
log="$out/release.log"
say() { echo "$*" | tee -a "$log"; }
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/core/Cargo.toml" | head -1)
[ -n "$version" ] || refuse "no version in core/Cargo.toml"
say "Northroom $version for Windows ($mode), $(date -u +%Y-%m-%dT%H:%M:%SZ)"
[ "$mode" = release ] && say "signer: $found"

# --- Build and package ------------------------------------------------------------------------
# Link the C runtime statically; a clean Windows has no Visual C++ runtime.
RUSTFLAGS="-C target-feature=+crt-static" "$here/scripts/with-build-lock.sh" sh "$here/scripts/build.sh" >>"$log" 2>&1 \
  || refuse "the build failed (see $log)"
NR_RELEASE=1 sh "$here/scripts/package-windows.sh" "$out" >>"$log" 2>&1 || refuse "packaging failed (see $log)"
setup="Northroom-$version-x64.exe"
[ -f "$out/$setup" ] || refuse "no $setup was made (see $log)"
say "setup: $out/$setup"

# --- Check the signatures ---------------------------------------------------------------------
fail() { say "FAIL  $1"; exit 1; }
if [ "$mode" = release ]; then
  signtool=$(signtool_path)
  windows_run "$signtool" verify /pa /tw /v "$(cygpath -w "$out/$setup")" >>"$log" 2>&1 || fail "signtool verify /pa: the setup"
  status=$(powershell.exe -NoProfile -Command "(Get-AuthenticodeSignature '$(cygpath -w "$out/$setup")').Status" | tr -d '\r')
  [ "$status" = Valid ] || fail "Windows says the setup's signature is $status"
  # package-windows.sh signed and verified the app before packing, and Inno Setup fails the
  # build if it can't sign the uninstaller (SignedUninstaller). Installing to check would take
  # over an existing install's uninstall entry.
  say "ok    setup, programs and uninstaller signed by $found, timestamped"
else
  say "note  unsigned: Windows will warn before running it (SmartScreen)"
fi

# --- The feed's files -------------------------------------------------------------------------
sum=$(python -I -c 'import base64,hashlib,sys; print(base64.b64encode(hashlib.sha512(open(sys.argv[1],"rb").read()).digest()).decode())' "$(cygpath -w "$out/$setup")")
size=$(wc -c < "$out/$setup" | tr -d ' ')
cat >"$out/native-windows.yml" <<YML
version: $version
files:
  - url: $setup
    sha512: $sum
    size: $size
path: $setup
sha512: $sum
releaseDate: '$(date -u +%Y-%m-%dT%H:%M:%S.000Z)'
YML
say "ok    $setup ($size bytes) and native-windows.yml"

if [ "$mode" = unsigned ]; then
  for file in "$out/$setup" "$out/native-windows.yml"; do mv "$file" "$file.NOT-FOR-RELEASE"; done
  say "unsigned: the setup and channel file are marked NOT-FOR-RELEASE"
else
  say "Done. Upload $setup and native-windows.yml to the newest release, beside the other platforms' files (DECISIONS.md, ADR-0010)."
fi
