#!/bin/sh
# Tests the native app's updater end to end against a local feed (ADR-0010), on the Mac,
# Windows (Git Bash on a CI runner) and Linux. Version 0.0.1 is installed in a scratch folder
# and 0.0.2 is offered by an electron-builder channel file served on 127.0.0.1. Each case runs
# the installed app unattended (scripts/native-run.sh) with its own scratch folders, answers
# dialogs from NR_UPDATE_ANSWERS and reads the app's stderr. Nothing reaches another host, and
# only processes started by this script or the app it installed are stopped.
#
# Packaging and signing by platform:
#   Mac      ad-hoc signed bundles (scripts/bundle.sh), zipped. The requirement is the identifier.
#   Windows  per-user setups (scripts/package-windows.sh) signed with a throwaway certificate
#            trusted only on the runner. A second certificate with the same name and a different
#            key is the other signer. The requirement is the first key's digest.
#   Linux    AppImages (scripts/package-linux.sh) signed with a throwaway minisign key
#            (`minisign` must be installed). The requirement is that key.
#
# Cases:
#   update          Check for Updates…, Download Update, Restart and Install: 0.0.2 is installed
#                   and running from the same place
#   later           Download Update, Later, quit: the update installs at quit and nothing opens
#   background      no action: the first scheduled check (set to 2 s) offers the update, and the
#                   feed request carries the rollout id
#   current         the feed offers the running version: "You're up to date"
#   menu            the Northroom menu item shows Download Update… after an update is found and
#                   put off, and Install Downloaded Update… after it is downloaded and put off
#   tampered-zip    the file doesn't match its digest: refused, nothing installed
#   tampered-app    the file matches its digest but its signed bytes changed: refused
#   other-signer    the file is intact but signed with another key: refused
#   own-signature   no requirement given, so the running app's own signature decides (an ad-hoc
#                   build's exact hash; on Windows its signer's key, offered the other signer's
#                   update; on Linux the built-in key) and refuses the update
#   downgrade       Windows and Linux: the feed offers 0.0.2 but serves a signed 0.0.0, refused
#                   because the signature carries the version
#   unsigned-app    Windows: an unsigned build with no requirement given accepts no update
#
# Usage: scripts/update-check.sh <scratch folder under the system temp folder> [case ...]
# Builds nothing: run scripts/build.sh first. Exits 1 if a case fails. On Linux run it under a
# display (xvfb-run); without FUSE, set APPIMAGE_EXTRACT_AND_RUN=1.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
scratch=${1:?usage: update-check.sh <scratch folder> [case ...]}
shift
mkdir -p "$scratch"
scratch=$(cd "$scratch" && pwd -P)
temporary=$(cd "${TMPDIR:-/tmp}" && pwd -P)
case "$scratch" in
  /private/tmp/?*|/private/var/folders/?*|/tmp/?*|"$temporary"/?*) ;;
  *) echo "update-check.sh: $scratch is not under the system's temporary folders" >&2; exit 2 ;;
esac
case "$(uname -s)" in
  Darwin) platform=mac ;;
  Linux) platform=linux ;;
  MINGW*|MSYS*|CYGWIN*) platform=windows ;;
  *) echo "update-check.sh: no packages for $(uname -s)" >&2; exit 2 ;;
esac
everywhere="update later background current menu tampered-zip tampered-app other-signer own-signature"
case "$platform" in
  mac) all="$everywhere" ;;
  linux) all="$everywhere downgrade" ;;
  windows) all="$everywhere downgrade unsigned-app" ;;
esac
cases=${*:-$all}
port=${NR_UPDATE_CHECK_PORT:-18765}

digest() { python3 -I -c 'import base64,hashlib,sys; print(base64.b64encode(hashlib.sha512(open(sys.argv[1],"rb").read()).digest()).decode())' "$1"; }
size_of() { wc -c < "$1" | tr -d ' '; }
# Flips one byte a third of the way in, inside the signed region.
flip_byte() { python3 -I -c 'import sys
p = sys.argv[1]; b = bytearray(open(p, "rb").read()); i = len(b) // 3; b[i] ^= 0xFF; open(p, "wb").write(b)' "$1"; }

# --- Packages: 0.0.1 to install, 0.0.2 to offer, 0.0.0 for the downgrade --------------------
rm -rf "$scratch/v0" "$scratch/v1" "$scratch/v1-unsigned" "$scratch/v2" "$scratch/feed" "$scratch/keys"
mkdir -p "$scratch/v0" "$scratch/v1" "$scratch/v2" "$scratch/feed" "$scratch/keys"
case "$platform" in
  mac)
    requirement='identifier "com.northroom.app.dev"'
    channel_file=native-mac.yml
    package() { echo "Northroom-$1-arm64.zip"; }
    "$here/scripts/with-build-lock.sh" sh -c "NR_VERSION=0.0.1 sh '$here/scripts/bundle.sh' '$scratch/v1' >/dev/null && NR_VERSION=0.0.2 sh '$here/scripts/bundle.sh' '$scratch/v2' >/dev/null"
    ditto -c -k --keepParent "$scratch/v2/Northroom.app" "$scratch/v2/$(package 0.0.2)"
    ;;
  windows)
    # Trusts its own certificates machine-wide, so CI only.
    [ "${GITHUB_ACTIONS:-}" = true ] || { echo "update-check.sh: on Windows it trusts certificates it makes, so it runs on a CI runner only" >&2; exit 2; }
    # shellcheck source=windows/tools.sh
    . "$here/scripts/windows/tools.sh"
    channel_file=native-windows.yml
    package() { echo "Northroom-$1-x64.exe"; }
    # Two code-signing certificates with the same name and different keys, trusted as roots and
    # publishers. Each output line is the thumbprint, then the base64 SHA-512 of the public key.
    # shellcheck disable=SC2016 # PowerShell variables
    powershell.exe -NoProfile -Command '
      $made = foreach ($n in 1, 2) {
        $c = New-SelfSignedCertificate -Type CodeSigningCert -Subject "CN=Northroom Update Check" -CertStoreLocation Cert:\CurrentUser\My -NotAfter (Get-Date).AddDays(2)
        $file = Join-Path $env:TEMP ("nr-check-" + $c.Thumbprint + ".cer")
        Export-Certificate -Cert $c -FilePath $file | Out-Null
        Import-Certificate -FilePath $file -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
        Import-Certificate -FilePath $file -CertStoreLocation Cert:\LocalMachine\TrustedPublisher | Out-Null
        Remove-Item $file
        $key = [Convert]::ToBase64String([Security.Cryptography.SHA512]::Create().ComputeHash($c.PublicKey.EncodedKeyValue.RawData))
        "$($c.Thumbprint) $key"
      }
      $made' | tr -d '\r' > "$scratch/keys/certificates.txt"
    signer=$(sed -n 1p "$scratch/keys/certificates.txt" | cut -d' ' -f1)
    other=$(sed -n 2p "$scratch/keys/certificates.txt" | cut -d' ' -f1)
    requirement=$(sed -n 1p "$scratch/keys/certificates.txt" | cut -d' ' -f2)
    [ -n "$signer" ] && [ -n "$other" ] && [ -n "$requirement" ] || { echo "update-check.sh: the check's certificates were not made" >&2; exit 2; }
    for version in 0.0.0 0.0.1 0.0.2; do
      folder="$scratch/v${version##*.}"
      NR_VERSION=$version NR_SIGN_IDENTITY=$signer NR_SIGN_TIMESTAMP='' sh "$here/scripts/package-windows.sh" "$folder" >/dev/null
    done
    mkdir -p "$scratch/v1-unsigned"
    NR_VERSION=0.0.1 sh "$here/scripts/package-windows.sh" "$scratch/v1-unsigned" >/dev/null
    ;;
  linux)
    command -v minisign >/dev/null || { echo "update-check.sh: minisign is not installed" >&2; exit 2; }
    channel_file=native-linux.yml
    package() { echo "Northroom-$1-x86_64.AppImage"; }
    for key in signer other; do
      minisign -G -W -f -p "$scratch/keys/$key.pub" -s "$scratch/keys/$key.key" >/dev/null
    done
    requirement=$(sed -n 2p "$scratch/keys/signer.pub")
    for version in 0.0.0 0.0.1 0.0.2; do
      folder="$scratch/v${version##*.}"
      NR_VERSION=$version NR_APPIMAGE_RUNTIME="$scratch/runtime-x86_64" sh "$here/scripts/package-linux.sh" "$folder" >/dev/null
    done
    ;;
esac

# sign_linux <AppImage> <key> <version> <minisig>: signs prehashed, as release-linux.sh does.
# minisign before 0.10 needs `-H` for that.
sign_linux() {
  minisign -S -s "$scratch/keys/$2.key" -m "$1" -x "$4" -t "file:$(basename "$1") version:$3" >/dev/null
  if [ "$(sed -n 2p "$4" | cut -c1-2)" != RU ]; then
    minisign -S -H -s "$scratch/keys/$2.key" -m "$1" -x "$4" -t "file:$(basename "$1") version:$3" >/dev/null
  fi
}
# channel <folder> <version> <file> [<minisig>]: writes a channel file in electron-builder's
# js-yaml format, with the minisign signature on Linux.
channel() {
  sum=$(digest "$1/$3")
  {
    echo "version: $2"
    echo "files:"
    echo "  - url: $3"
    echo "    sha512: $sum"
    echo "    size: $(size_of "$1/$3")"
    [ -n "${4:-}" ] && echo "    signature: $(python3 -I -c 'import base64,sys; print(base64.b64encode(open(sys.argv[1],"rb").read()).decode())' "$4")"
    echo "path: $3"
    echo "sha512: $sum"
    echo "releaseDate: '2026-11-13T12:00:00.000Z'"
  } > "$1/$channel_file"
}
# feed <name> <version offered> <file to serve as the offered version's> [<minisig>]
feed() {
  mkdir -p "$scratch/feed/$1"
  cp "$3" "$scratch/feed/$1/$(package "$2")"
  channel "$scratch/feed/$1" "$2" "$(package "$2")" "${4:-}"
}

work="$scratch/work"
rm -rf "$work"
mkdir -p "$work"
v2="$scratch/v2/$(package 0.0.2)"
case "$platform" in
  mac)
    feed current 0.0.2 "$v2"
    # A sealed file changed after signing; the digest matches the changed zip.
    mkdir -p "$work/tampered" "$work/other"
    ditto "$scratch/v2/Northroom.app" "$work/tampered/Northroom.app"
    printf '\n' >> "$work/tampered/Northroom.app/Contents/Resources/Licenses/three.js-0.186.0.txt"
    ditto -c -k --keepParent "$work/tampered/Northroom.app" "$work/tampered.zip"
    feed tampered-app 0.0.2 "$work/tampered.zip"
    # Intact, but signed with another identifier.
    ditto "$scratch/v2/Northroom.app" "$work/other/Northroom.app"
    codesign --force --sign - -i com.example.other "$work/other/Northroom.app" 2>/dev/null
    ditto -c -k --keepParent "$work/other/Northroom.app" "$work/other.zip"
    feed other-signer 0.0.2 "$work/other.zip"
    ;;
  windows)
    feed current 0.0.2 "$v2"
    cp "$v2" "$work/tampered.exe"
    flip_byte "$work/tampered.exe"
    feed tampered-app 0.0.2 "$work/tampered.exe"
    cp "$v2" "$work/other.exe"
    (NR_SIGN_IDENTITY=$other NR_SIGN_TIMESTAMP='' && sign_files "$work/other.exe")
    feed other-signer 0.0.2 "$work/other.exe"
    feed downgrade 0.0.2 "$scratch/v0/$(package 0.0.0)"
    ;;
  linux)
    sign_linux "$v2" signer 0.0.2 "$work/v2.minisig"
    feed current 0.0.2 "$v2" "$work/v2.minisig"
    cp "$v2" "$work/tampered.AppImage"
    flip_byte "$work/tampered.AppImage"
    feed tampered-app 0.0.2 "$work/tampered.AppImage" "$work/v2.minisig"
    sign_linux "$v2" other 0.0.2 "$work/other.minisig"
    feed other-signer 0.0.2 "$v2" "$work/other.minisig"
    sign_linux "$scratch/v0/$(package 0.0.0)" signer 0.0.0 "$work/v0.minisig"
    feed downgrade 0.0.2 "$scratch/v0/$(package 0.0.0)" "$work/v0.minisig"
    ;;
esac
# The feed offers the running version.
mkdir -p "$scratch/feed/same"
cp "$scratch/feed/current/"* "$scratch/feed/same/"
sed 's/^version: 0.0.2$/version: 0.0.1/' "$scratch/feed/current/$channel_file" > "$scratch/feed/same/$channel_file"
# The file changed after its digest was written.
mkdir -p "$scratch/feed/tampered-zip"
cp "$scratch/feed/current/"* "$scratch/feed/tampered-zip/"
printf 'x' >> "$scratch/feed/tampered-zip/$(package 0.0.2)"

# --- Feed server: serves files and logs each request with its rollout header ---
cat > "$scratch/serve.py" <<'PY'
import http.server, sys
folder, port, log = sys.argv[1], int(sys.argv[2]), sys.argv[3]
class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=folder, **kwargs)
    def log_message(self, *args):
        pass
    def do_GET(self):
        with open(log, "a") as out:
            out.write(f"{self.path}\t{self.headers.get('x-user-staging-id', '')}\n")
        super().do_GET()
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
: > "$scratch/requests.txt"
python3 -I "$scratch/serve.py" "$scratch/feed" "$port" "$scratch/requests.txt" &
server=$!
relaunched=""

# --- What each platform installs and runs ---------------------------------------------------
# install_first <case folder> [unsigned]: installs 0.0.1 in the case folder.
install_first() {
  case "$platform" in
    mac) mkdir -p "$1/Applications" && ditto "$scratch/v1/Northroom.app" "$1/Applications/Northroom.app" ;;
    windows)
      setup="$scratch/v1/$(package 0.0.1)"
      [ "${2:-}" = unsigned ] && setup="$scratch/v1-unsigned/$(package 0.0.1)"
      windows_run "$setup" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART "/DIR=$(cygpath -w "$1/Northroom")" ;;
    linux)
      mkdir -p "$1/Applications"
      install -m 755 "$scratch/v1/$(package 0.0.1)" "$1/Applications/Northroom.AppImage" ;;
  esac
}
# What an update replaces: the bundle, install folder or AppImage.
installed() {
  case "$platform" in
    mac) echo "$1/Applications/Northroom.app" ;;
    windows) echo "$1/Northroom" ;;
    linux) echo "$1/Applications/Northroom.AppImage" ;;
  esac
}
program() {
  case "$platform" in
    mac) echo "$(installed "$1")/Contents/MacOS/Northroom" ;;
    windows) echo "$(installed "$1")/Northroom.exe" ;;
    linux) installed "$1" ;;
  esac
}
installed_version() {
  case "$platform" in
    mac) /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$(installed "$1")/Contents/Info.plist" ;;
    windows) python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$(installed "$1")/Resources/release.json" ;;
    linux)
      for version in 0.0.0 0.0.1 0.0.2; do
        if cmp -s "$(installed "$1")" "$scratch/v${version##*.}/$(package "$version")"; then echo "$version"; return; fi
      done
      echo "another" ;;
  esac
}
# Whether the install is intact: its signature on the Mac, otherwise 0.0.1's exact bytes.
whole() {
  case "$platform" in
    mac) codesign --deep --strict --verify "$(installed "$1")" 2>/dev/null ;;
    # Compare with the matching signed or unsigned reference install.
    windows)
      reference=update-reference
      [ "$(basename "$1")" = unsigned-app ] && reference=unsigned-reference
      cmp -s "$(program "$1")" "$(program "$scratch/cases/$reference")" ;;
    linux) cmp -s "$(installed "$1")" "$scratch/v1/$(package 0.0.1)" ;;
  esac
}
# Windows: the id of the process running <path>, started by the setup.
running_at() {
  powershell.exe -NoProfile -Command "Get-Process | Where-Object { \$_.Path -eq '$(cygpath -w "$1")' } | ForEach-Object { \$_.Id }" | tr -d '\r' | head -1
}
# Windows: waits up to <seconds> for the setup to install <version> in <case folder>.
wait_installed() {
  waited=0
  while [ "$waited" -lt "$3" ] && [ "$(installed_version "$1" 2>/dev/null)" != "$2" ]; do
    sleep 1
    waited=$((waited + 1))
  done
  # The setup writes its uninstaller after the files.
  sleep 2
}
cleanup() {
  kill "$server" 2>/dev/null || true
  [ -n "$relaunched" ] && kill "$relaunched" 2>/dev/null || true
  if [ "$platform" = windows ]; then
    # Stop what the setups started and remove the check's certificates.
    powershell.exe -NoProfile -Command "
      Get-Process | Where-Object { \$_.Path -like '$(cygpath -w "$scratch")*' } | Stop-Process -Force -ErrorAction SilentlyContinue
      foreach (\$store in 'Cert:\\LocalMachine\\Root', 'Cert:\\LocalMachine\\TrustedPublisher', 'Cert:\\CurrentUser\\My') {
        Get-ChildItem \$store | Where-Object { \$_.Subject -eq 'CN=Northroom Update Check' } | Remove-Item -ErrorAction SilentlyContinue
      }" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT INT TERM
sleep 1

status=0
verdict() { # <case> <0 pass|else fail> <words>
  if [ "$2" -eq 0 ]; then printf '%-14s pass  %s\n' "$1" "$3"; else printf '%-14s FAIL  %s\n' "$1" "$3"; status=1; fi
}

# run <case> <feed> <seconds> [NAME=value ...]: installs 0.0.1 in the case folder and runs it.
run() {
  name=$1 feed=$2 seconds=$3
  shift 3
  folder="$scratch/cases/$name"
  rm -rf "$folder"
  mkdir -p "$folder/workspace" "$folder/shots"
  if [ "$name" = unsigned-app ]; then install_first "$folder" unsigned; else install_first "$folder"; fi
  env NR_SCRATCH="$scratch" NR_SUPPORT="$folder/support" NR_METADATA="$folder/metadata" NR_TRASH="$folder/trash" \
    NR_WORKSPACE="$folder/workspace" NR_NO_TERMINAL=1 NR_SNAPSHOT_DIR="$folder/shots" \
    NR_APP="$(program "$folder")" NR_UPDATE_FEED="http://127.0.0.1:$port/$feed/" \
    "$@" sh "$here/scripts/native-run.sh" "$seconds" "$folder/stderr.txt"
  # A relaunched app outlives the run, so stop it by the pid it logged (Mac, Linux).
  relaunched=$(sed -n 's/^update	launched	[^	]*	\([0-9]*\)	.*/\1/p' "$folder/stderr.txt" | tail -1)
  if [ -n "$relaunched" ]; then
    sleep 2
    kill "$relaunched" 2>/dev/null || true
  fi
}
said() { grep -c "$2" "$scratch/cases/$1/stderr.txt" || true; }

if [ "$platform" = windows ]; then
  # Signed and unsigned 0.0.1 installs that a refused update must leave unchanged.
  install_first "$scratch/cases/update-reference"
  install_first "$scratch/cases/unsigned-reference" unsigned
fi

for name in $cases; do
  case "$name" in
    update)
      run update current 20 NR_DO=update:check NR_UPDATE_ANSWERS=0,0 NR_UPDATE_REQUIREMENT="$requirement"
      log="$scratch/cases/update/stderr.txt"
      app=$(installed "$scratch/cases/update")
      ok=0
      [ "$(said update 'Northroom 0.0.2 is available')" -ge 1 ] && [ "$(said update 'Update ready to install')" -ge 1 ] || ok=1
      if [ "$platform" = windows ]; then
        # The setup waits for the app to exit, installs, then opens the new version.
        wait_installed "$scratch/cases/update" 0.0.2 90
        new=$(running_at "$(program "$scratch/cases/update")")
        [ "$(installed_version "$scratch/cases/update")" = 0.0.2 ] && [ -n "$new" ] || ok=1
        [ -n "$new" ] && windows_run taskkill /PID "$new" /F >/dev/null 2>&1
        opened="process $new"
      else
        [ "$(installed_version "$scratch/cases/update")" = 0.0.2 ] || ok=1
        [ "$platform" = mac ] && { codesign --deep --strict --verify "$app" 2>/dev/null || ok=1; }
        grep -q "^update	launched	0.0.2	[0-9]*	$app\$" "$log" || ok=1
        opened=$(grep '^update	launched' "$log" | cut -f3,4 | tr '\t' ' ')
      fi
      verdict update $ok "installed $(installed_version "$scratch/cases/update"); relaunched: $opened" ;;
    later)
      run later current 20 NR_DO=update:check,update:quitwhenready NR_UPDATE_ANSWERS=0,1 NR_UPDATE_REQUIREMENT="$requirement"
      log="$scratch/cases/later/stderr.txt"
      ok=0
      if [ "$platform" = windows ]; then
        wait_installed "$scratch/cases/later" 0.0.2 90
        [ -z "$(running_at "$(program "$scratch/cases/later")")" ] || ok=1
      else
        ! grep -q '^update	launched' "$log" || ok=1
      fi
      [ "$(installed_version "$scratch/cases/later")" = 0.0.2 ] || ok=1
      grep -q '^update	installed	0.0.2' "$log" || ok=1
      verdict later $ok "Later, then quit: installed $(installed_version "$scratch/cases/later"), nothing opened" ;;
    background)
      : > "$scratch/requests.txt"
      run background current 8 NR_DO=update:launched NR_UPDATE_FIRST_CHECK=2 NR_UPDATE_ANSWERS=1 NR_UPDATE_REQUIREMENT="$requirement"
      ok=0
      [ "$(said background 'Northroom 0.0.2 is available')" -ge 1 ] || ok=1
      id=$(cat "$scratch/cases/background/support/.updaterId" 2>/dev/null || true)
      grep -q "^/current/$channel_file?noCache=[0-9a-v]*	$id\$" "$scratch/requests.txt" || ok=1
      [ -n "$id" ] || ok=1
      [ "$(installed_version "$scratch/cases/background")" = 0.0.1 ] || ok=1
      verdict background $ok "offered after the first scheduled check; asked $(head -1 "$scratch/requests.txt" | cut -f1)" ;;
    current)
      run current same 6 NR_DO=update:check NR_UPDATE_ANSWERS=0 NR_UPDATE_REQUIREMENT="$requirement"
      ok=0
      grep -q "^dialog	box-1	informational	You're up to date	Northroom 0.0.1 is the latest version available for this device.	OK=return" "$scratch/cases/current/stderr.txt" || ok=1
      verdict current $ok "You're up to date" ;;
    menu)
      # The update item in the Northroom menu, matched by its label.
      update_item() {
        grep '^Northroom: ' "$1" | tr '|' '\n' | sed 's/^ *//; s/ *$//' |
          grep -E '^(Check for Updates…|Download Update…|Install Downloaded Update…)' | head -n 1
      }
      waits="ev:wait,ev:wait,ev:wait,ev:wait"
      run menu-found current 8 NR_DO="update:check,$waits,menubar" NR_UPDATE_ANSWERS=1 NR_UPDATE_REQUIREMENT="$requirement"
      run menu-ready current 15 NR_DO="update:check,$waits,$waits,$waits,menubar" NR_UPDATE_ANSWERS=0,1 NR_UPDATE_REQUIREMENT="$requirement"
      found=$(update_item "$scratch/cases/menu-found/stderr.txt")
      ready=$(update_item "$scratch/cases/menu-ready/stderr.txt")
      ok=0
      [ "$found" = "Download Update…" ] && [ "$ready" = "Install Downloaded Update…" ] || ok=1
      verdict menu $ok "found and put off: $found; downloaded and put off: $ready" ;;
    tampered-zip|tampered-app|other-signer|own-signature|downgrade|unsigned-app)
      feed=$name
      extra="NR_UPDATE_REQUIREMENT=$requirement"
      case "$name" in
        own-signature)
          extra="NR_UPDATE_REQUIREMENT_UNSET=1"
          # On Windows the app's own key is the check's signer, so it must refuse the other
          # signer's update.
          if [ "$platform" = windows ]; then feed="other-signer"; else feed="current"; fi ;;
        unsigned-app) extra="NR_UPDATE_REQUIREMENT_UNSET=1" feed=current ;;
      esac
      run "$name" "$feed" 15 NR_DO=update:check NR_UPDATE_ANSWERS=0,0 "$extra"
      log="$scratch/cases/$name/stderr.txt"
      reason=$(sed -n 's/^dialog	box-2	critical	Could not complete the update	\(.*\)	OK=return$/\1/p' "$log" | head -1)
      [ "$platform" = windows ] && sleep 3
      ok=0
      [ -n "$reason" ] || ok=1
      [ "$(installed_version "$scratch/cases/$name")" = 0.0.1 ] || ok=1
      whole "$scratch/cases/$name" || ok=1
      case "$name" in
        tampered-zip) expected='^sha512 checksum mismatch, expected ' ;;
        unsigned-app) expected="^This app's own code signature could not be read\\.$" ;;
        downgrade) expected='did not pass validation: .*\(version 0\.0\.0\.0, not 0\.0\.2\.0\|not of version 0\.0\.2\)' ;;
        # On Linux without a built-in key, the app refuses because it has no key of its own.
        own-signature) expected="did not pass validation\\|^This app's own code signature could not be read\\.$" ;;
        *) expected='did not pass validation' ;;
      esac
      echo "$reason" | grep -q "$expected" || ok=1
      verdict "$name" $ok "refused, 0.0.1 left whole: $(echo "$reason" | sed -E 's#Code signature at URL [^ ]+ ##' | cut -c1-110)" ;;
    *) echo "no case named $name" >&2; status=2 ;;
  esac
done
exit $status
