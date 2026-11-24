#!/bin/sh
# Sets the release secrets on main-eng/northroom. Asks for each value without echoing it and
# sends it straight to GitHub; nothing is written to disk or shell history.
#
#   apps/native/scripts/set-release-secrets.sh /path/to/DeveloperID.p12
set -eu
repo=main-eng/northroom
p12=${1:-}
[ -f "$p12" ] || { echo "usage: set-release-secrets.sh <Developer ID .p12 file>" >&2; exit 2; }

base64 -i "$p12" | gh secret set APPLE_CERTIFICATE -R "$repo"
echo "APPLE_CERTIFICATE set from $p12"

for name in APPLE_CERTIFICATE_PASSWORD APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID \
  LINUX_UPDATE_KEY LINUX_UPDATE_KEY_PASSWORD; do
  printf '%s (hidden): ' "$name"
  stty -echo
  IFS= read -r value
  stty echo
  echo
  printf '%s' "$value" | gh secret set "$name" -R "$repo"
done

# CI only uses this to lock a temporary keychain, so any new random value works.
openssl rand -hex 32 | gh secret set KEYCHAIN_PASSWORD -R "$repo"
echo "KEYCHAIN_PASSWORD set to a new random value"

gh secret list -R "$repo"
