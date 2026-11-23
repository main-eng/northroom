# Locates Windows build tools for the Windows scripts (sourced in Git Bash): signtool from the
# newest Windows 10/11 SDK, the Inno Setup compiler, and dumpbin from the newest Visual Studio C++
# tools. Each function prints the tool's path or reports that it is missing.

windows_sdk_tool() { # <name>
  found=$(ls -d "/c/Program Files (x86)/Windows Kits/10/bin/"10.*/x64/"$1.exe" 2>/dev/null | sort -V | tail -1)
  [ -n "$found" ] || { echo "no $1.exe: install the Windows SDK" >&2; return 1; }
  echo "$found"
}

signtool_path() { windows_sdk_tool signtool; }

iscc_path() {
  for candidate in "$(command -v iscc 2>/dev/null)" "/c/Program Files (x86)/Inno Setup 6/ISCC.exe" \
    "${LOCALAPPDATA:-}/Programs/Inno Setup 6/ISCC.exe"; do
    [ -n "$candidate" ] && [ -f "$candidate" ] && { echo "$candidate"; return 0; }
  done
  echo "no ISCC.exe: install Inno Setup 6 (winget install JRSoftware.InnoSetup)" >&2
  return 1
}

dumpbin_path() {
  vswhere="/c/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe"
  [ -f "$vswhere" ] || { echo "no vswhere.exe: install Visual Studio's C++ build tools" >&2; return 1; }
  found=$("$vswhere" -latest -products '*' -find 'VC/Tools/MSVC/**/bin/Hostx64/x64/dumpbin.exe' | tr -d '\r' | tail -1)
  [ -n "$found" ] || { echo "no dumpbin.exe in Visual Studio's C++ tools" >&2; return 1; }
  cygpath -u "$found"
}

# Runs a Windows program with its arguments untouched. Otherwise Git Bash rewrites arguments that
# start with / as paths (`/DIR=…`, signtool's `/fd`). Pass Windows-style paths.
windows_run() {
  MSYS2_ARG_CONV_EXCL='*' "$@"
}

# Signs <file ...> with the user-store certificate whose SHA-1 thumbprint is NR_SIGN_IDENTITY
# (hardware token certificates appear there too), using SHA-256 and an RFC 3161 timestamp from
# NR_SIGN_TIMESTAMP unless it is empty. NR_SIGN_ALSO appends a second signature (`/as`) so the
# release that moves to a renewed certificate is signed by both.
sign_files() {
  signtool=$(signtool_path) || return 1
  timestamp=${NR_SIGN_TIMESTAMP-http://timestamp.digicert.com}
  stamp=
  [ -n "$timestamp" ] && stamp="/tr $timestamp /td sha256"
  for file in "$@"; do
    # shellcheck disable=SC2086 # split the timestamp options on purpose
    windows_run "$signtool" sign /sha1 "$NR_SIGN_IDENTITY" /fd sha256 $stamp /q "$(cygpath -w "$file")" || return 1
    if [ -n "${NR_SIGN_ALSO:-}" ]; then
      # shellcheck disable=SC2086
      windows_run "$signtool" sign /as /sha1 "$NR_SIGN_ALSO" /fd sha256 $stamp /q "$(cygpath -w "$file")" || return 1
    fi
  done
}

# The sign tool command Inno Setup uses for the setup and uninstaller (`/Snr=…`; Inno replaces
# $f with the quoted file). Quote signtool's path with Inno's `$q`. Git Bash escapes a literal `"`
# as `\"`, which ISCC can't read, so it splits the command ("You may not specify more than one
# script filename").
sign_command() {
  signtool=$(signtool_path) || return 1
  timestamp=${NR_SIGN_TIMESTAMP-http://timestamp.digicert.com}
  command="\$q$(cygpath -w "$signtool")\$q sign /sha1 $NR_SIGN_IDENTITY /fd sha256 /q"
  [ -n "$timestamp" ] && command="$command /tr $timestamp /td sha256"
  echo "$command \$f"
}
