#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 breitburg
#
# Regenerate flatpak/cargo-sources.json from Cargo.lock. AppCenter (and the
# release CI) build the Flatpak offline, so every crate must be pre-listed with
# its hash for flatpak-builder to fetch ahead of time. Re-run this whenever
# Cargo.lock changes, then commit the result.
#
# Usage: build-aux/gen-cargo-sources.sh
set -e

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GEN_URL="https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py"
GEN="$(mktemp --suffix=.py)"
trap 'rm -f "$GEN"' EXIT

curl -fsSL "$GEN_URL" -o "$GEN"

# Run the generator in a one-shot isolated environment (no system pollution).
if command -v uv >/dev/null 2>&1; then
    uv run --with aiohttp --with toml python3 "$GEN" \
        "$ROOT/Cargo.lock" -o "$ROOT/flatpak/cargo-sources.json"
else
    echo "uv not found; install it or run the generator manually:" >&2
    echo "  pip install aiohttp toml && python3 <gen> Cargo.lock -o flatpak/cargo-sources.json" >&2
    exit 1
fi

echo "Wrote flatpak/cargo-sources.json"
