#!/bin/sh
# Writes core/assets/generated/syntax.js, the shiki highlighter from the repo's node_modules as
# one script for the shell's JavaScript engine (served by core/src/highlight.rs). Rerun when
# shiki, its grammars or entry.ts change. Bundles from a temp folder that links shiki, so it
# downloads nothing. Run `bun install` at the repository root first.
set -eu
here=$(cd "$(dirname "$0")/../.." && pwd)
shiki=$(cd "$here/../../node_modules/shiki" && pwd -P)
out="$here/core/assets/generated/syntax.js"
work=$(mktemp -d "${TMPDIR:-/tmp}/northroom-syntax.XXXXXX")
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/node_modules" "$(dirname "$out")"
ln -s "$shiki" "$work/node_modules/shiki"
cp "$here/scripts/syntax/entry.ts" "$here/scripts/syntax/atob.ts" "$work/"
(cd "$work" && bun build ./entry.ts --target=browser --format=iife --minify --outfile "$out" >/dev/null)
echo "$out $(wc -c < "$out") bytes"
