#!/bin/sh
# Makes the Linux AppImage from a release build of the GPUI app (ADR-0003). The Linux
# counterpart of scripts/bundle.sh. Contents:
#
#   AppRun                        launcher (scripts/linux/AppRun)
#   northroom.desktop, northroom.png, .DirIcon   desktop entry and icon
#   app/northroom                 northroom-gpui, with the core linked in
#   app/Resources/SceneHost/      the page host's pinned GSAP, three.js and fonts
#   app/Resources/Licenses/       licences for everything copied in
#   app/Resources/release.json    version and identifier for the updater
#                                 (core/src/update/package.rs)
#
# GTK 3, WebKitGTK 4.1, GStreamer and its plugins, OpenSSL 3 and the Vulkan loader come from the
# system (ADR-0003 says why), and the binary may set no rpath. The file is the AppImage type 2
# runtime, pinned by digest, followed by the folder as zstd SquashFS.
#
# Builds nothing: run scripts/build.sh first. Needs mksquashfs (squashfs-tools 4.4 or later).
# Downloads the runtime once into <output folder>/.runtime unless NR_APPIMAGE_RUNTIME points
# at a local copy.
#
#   NR_RELEASE=1   use the release identifier com.northroom.app, not com.northroom.app.dev.
#   NR_VERSION     version, default the one in core/Cargo.toml.
# Usage: scripts/package-linux.sh <output folder>
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
out=${1:?usage: package-linux.sh <output folder>}
build="$here/gpui/target/release"
resources="$here/gpui/webhost/resources"
[ -f "$build/northroom-gpui" ] || { echo "no $build/northroom-gpui: run scripts/build.sh first" >&2; exit 2; }
command -v mksquashfs >/dev/null || { echo "no mksquashfs: install squashfs-tools" >&2; exit 2; }
version=${NR_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/core/Cargo.toml" | head -1)}
[ -n "$version" ] || { echo "no version in core/Cargo.toml" >&2; exit 2; }
if [ "${NR_RELEASE:-}" = 1 ]; then identifier=com.northroom.app; else identifier=com.northroom.app.dev; fi
arch=$(uname -m)
[ "$arch" = x86_64 ] || { echo "only x86_64 is packaged (this is $arch)" >&2; exit 2; }

# github.com/AppImage/type2-runtime, release 20251108. It is static, so it runs on any distribution
# with FUSE, or with --appimage-extract-and-run without it.
runtime_url=https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64
runtime_sha256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d

mkdir -p "$out"
out=$(cd "$out" && pwd)
runtime=${NR_APPIMAGE_RUNTIME:-$out/.runtime/runtime-x86_64}
if [ ! -f "$runtime" ]; then
  mkdir -p "$(dirname "$runtime")"
  curl -fsSL --retry 3 -o "$runtime.part" "$runtime_url"
  mv "$runtime.part" "$runtime"
fi
echo "$runtime_sha256  $runtime" | sha256sum -c --quiet - || { echo "the AppImage runtime at $runtime is not the pinned one" >&2; exit 1; }

appdir="$out/Northroom.AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/app/Resources/Licenses"
cp "$here/scripts/linux/AppRun" "$appdir/AppRun"
cp "$here/scripts/linux/northroom.desktop" "$appdir/northroom.desktop"
cp "$here/gpui/shell/Northroom.png" "$appdir/northroom.png"
cp "$here/gpui/shell/Northroom.png" "$appdir/.DirIcon"
install -m 755 "$build/northroom-gpui" "$appdir/app/northroom"
cp -R "$resources" "$appdir/app/Resources/SceneHost"
licences="$appdir/app/Resources/Licenses"
cp "$resources/vendor/gsap-3.15.0/LICENSE.txt" "$licences/GSAP-3.15.0.txt"
cp "$resources/vendor/three-0.186.0/LICENSE" "$licences/three.js-0.186.0.txt"
cp "$resources/fonts/"OFL-*.txt "$licences/"
cp "$here/core/assets/licenses/Lucide.txt" "$licences/Lucide.txt"
printf '{"version":"%s","identifier":"%s"}\n' "$version" "$identifier" > "$appdir/app/Resources/release.json"
chmod 755 "$appdir/AppRun"

# Fail on any rpath, which could point at the build machine.
for program in "$appdir/app/northroom"; do
  if readelf -d "$program" | grep -E 'RPATH|RUNPATH' >/dev/null; then
    echo "$program names a folder to load libraries from:" >&2
    readelf -d "$program" | grep -E 'RPATH|RUNPATH' >&2
    exit 1
  fi
done
# Print the system libraries and the newest glibc symbol the binary needs.
needed=$(readelf -d "$appdir/app/northroom" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | sort -u | tr '\n' ' ')
glibc=$(objdump -T "$appdir/app/northroom" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)
echo "needs: $glibc; $needed"

name="Northroom-$version-x86_64.AppImage"
rm -f "$out/$name" "$out/$name.squashfs"
mksquashfs "$appdir" "$out/$name.squashfs" -root-owned -noappend -comp zstd -Xcompression-level 19 -quiet >/dev/null
cat "$runtime" "$out/$name.squashfs" > "$out/$name"
chmod 755 "$out/$name"
rm -rf "$appdir" "$out/$name.squashfs"
echo "$out/$name ($(du -h "$out/$name" | cut -f1))"
