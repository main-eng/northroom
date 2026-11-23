#!/bin/sh
# Makes Northroom.app from a release build of the GPUI app (ADR-0001), with no build machine
# paths in it:
#
#   Northroom.app/Contents/
#     Info.plist
#     MacOS/Northroom               northroom-gpui, with the core linked in
#     Resources/Northroom.icns      the app icon
#     Resources/SceneHost/          the page host's pinned GSAP, three.js and fonts
#                                   (gpui/webhost/resources), found relative to Contents/MacOS
#     Resources/Licenses/           licences for everything copied in
#
# Fonts, icons and the page host's scripts are compiled into the binary. It links only system
# libraries (media uses AVFoundation and VideoToolbox, ADR-0004), so there is no Frameworks
# folder and the bundle runs from anywhere.
#
# Builds nothing: run scripts/build.sh first. Every build is signed with the hardened runtime
# and gpui/shell/Northroom.entitlements, so dev builds run the way a release does.
#
#   NR_RELEASE=1        use the release identifier com.northroom.app so installed copies' updater
#                       accepts it (ADR-0001). Otherwise com.northroom.app.dev, which stays apart
#                       from an installed Northroom in Launch Services, the Dock and privacy
#                       settings.
#   NR_SIGN_IDENTITY    codesign identity, default "-" (ad hoc, runs on this Mac only).
#                       scripts/release-mac.sh passes a Developer ID.
#   NR_VERSION          bundle version, default the one in core/Cargo.toml.
# Usage: scripts/bundle.sh <output folder>   (writes <output folder>/Northroom.app)
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
out=${1:?usage: bundle.sh <output folder>}
build="$here/gpui/target/release"
resources="$here/gpui/webhost/resources"
[ -f "$build/northroom-gpui" ] || { echo "no $build/northroom-gpui: run scripts/build.sh first" >&2; exit 2; }
app="$out/Northroom.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/Licenses"

# A binary's linked libraries (one per line) and its rpaths.
loads() { otool -L "$1" | tail -n +2 | sed -E 's/^[[:space:]]+//; s/ \(compatibility version.*$//'; }
rpaths() { otool -l "$1" | awk '/LC_RPATH/ {getline; getline; print $2}'; }

cp "$build/northroom-gpui" "$app/Contents/MacOS/Northroom"
chmod u+w "$app/Contents/MacOS/Northroom"

# Page host files and the licences for what they include.
cp -R "$resources" "$app/Contents/Resources/SceneHost"
licences="$app/Contents/Resources/Licenses"
cp "$resources/vendor/gsap-3.15.0/LICENSE.txt" "$licences/GSAP-3.15.0.txt"
cp "$resources/vendor/three-0.186.0/LICENSE" "$licences/three.js-0.186.0.txt"
cp "$resources/fonts/"OFL-*.txt "$licences/"
cp "$here/core/assets/licenses/Lucide.txt" "$licences/Lucide.txt"

# The update check sets NR_VERSION to build two versions.
version=${NR_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/core/Cargo.toml" | head -1)}
[ -n "$version" ] || { echo "no version in core/Cargo.toml" >&2; exit 2; }
if [ "${NR_RELEASE:-}" = 1 ]; then identifier=com.northroom.app; else identifier=com.northroom.app.dev; fi
identity=${NR_SIGN_IDENTITY:--}
# Drawn by scripts/generate-brand-assets.mjs in main-eng/northroom-web.
cp "$here/gpui/shell/Northroom.icns" "$app/Contents/Resources/Northroom.icns"
cat >"$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key><string>Northroom</string>
  <key>CFBundleIconFile</key><string>Northroom</string>
  <key>CFBundleIdentifier</key><string>$identifier</string>
  <key>CFBundleName</key><string>Northroom</string>
  <key>CFBundleDisplayName</key><string>Northroom</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>NSHumanReadableCopyright</key><string>© Northroom</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <!-- The app is made for macOS 14 and later; the update feed's channel file says so too. -->
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Folder or file</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array><string>public.folder</string><string>public.item</string></array>
    </dict>
  </array>
  <!-- The drive is any folder the person picks, and its terminals run their commands there:
       what the system says when either reaches a protected folder. -->
  <key>NSDesktopFolderUsageDescription</key><string>Northroom shows and organises the files in the folder you chose, and runs your terminals there.</string>
  <key>NSDocumentsFolderUsageDescription</key><string>Northroom shows and organises the files in the folder you chose, and runs your terminals there.</string>
  <key>NSDownloadsFolderUsageDescription</key><string>Northroom shows and organises the files in the folder you chose, and runs your terminals there.</string>
  <key>NSRemovableVolumesUsageDescription</key><string>Northroom shows and organises the files in the folder you chose, and runs your terminals there.</string>
  <key>NSNetworkVolumesUsageDescription</key><string>Northroom shows and organises the files in the folder you chose, and runs your terminals there.</string>
</dict>
</plist>
PLIST

# Signing the app seals Info.plist and Contents/Resources. Notarisation needs a secure
# timestamp, which an ad-hoc signature can't carry. Library validation stays on because only
# system libraries load.
if [ "$identity" = - ]; then timestamp=--timestamp=none; else timestamp=--timestamp; fi
sign() { said=$(codesign --force --options runtime "$timestamp" --sign "$identity" "$@" 2>&1) || { echo "$said" >&2; return 1; }; }
sign --entitlements "$here/gpui/shell/Northroom.entitlements" "$app"
codesign --deep --strict --verify "$app"

# Fail on any non-system library or build machine path.
left=$(for binary in "$app/Contents/MacOS/"*; do loads "$binary"; rpaths "$binary"; done | grep -v -e '^/System/' -e '^/usr/lib/' || true)
if [ -n "$left" ]; then
  echo "the bundle loads libraries or names paths that are not the system's:" >&2
  echo "$left" >&2
  exit 1
fi
echo "$app ($(du -sh "$app" | cut -f1))"
