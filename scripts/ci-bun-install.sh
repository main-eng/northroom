#!/usr/bin/env bash
set -euo pipefail

attempts="${BUN_INSTALL_ATTEMPTS:-3}"
delay_seconds="${BUN_INSTALL_RETRY_DELAY_SECONDS:-10}"
repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

if ! [[ "$attempts" =~ ^[1-9][0-9]*$ ]]; then
  echo "::error::BUN_INSTALL_ATTEMPTS must be a positive integer; got '${attempts}'."
  exit 2
fi

if ! [[ "$delay_seconds" =~ ^[0-9]+$ ]]; then
  echo "::error::BUN_INSTALL_RETRY_DELAY_SECONDS must be a non-negative integer; got '${delay_seconds}'."
  exit 2
fi

for ((attempt = 1; attempt <= attempts; attempt += 1)); do
  echo "::group::bun install attempt ${attempt}/${attempts}"
  if bun install --frozen-lockfile; then
    echo "::endgroup::"
    exit 0
  fi
  status="$?"
  echo "::endgroup::"

  if [ "$attempt" -eq "$attempts" ]; then
    echo "::error::bun install failed after ${attempts} attempts."
    exit "$status"
  fi

  echo "::warning::bun install failed with exit ${status}; clearing Bun cache before retry."
  if ! bun pm cache rm; then
    echo "::warning::Bun cache cleanup failed; continuing with a clean workspace install."
  fi
  rm -rf "${repo_root}/node_modules"
  sleep "$((delay_seconds * attempt))"
done
