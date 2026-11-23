#!/bin/sh
# Builds the GPUI app (northroom-gpui) with the core linked in and prints the binary's path.
# Run it through scripts/with-build-lock.sh.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
(cd "$here/gpui" && cargo build --release -j 4 -p shell)
echo "$here/gpui/target/release/northroom-gpui"
