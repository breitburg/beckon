#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 breitburg
#
# Build the release binary with cargo and copy it to the meson output path.
# Usage: cargo.sh <source-root> <output-path> <target-dir>
set -e

SOURCE_ROOT="$1"
OUTPUT="$2"
TARGET_DIR="$3"

cargo build \
    --manifest-path "$SOURCE_ROOT/Cargo.toml" \
    --release \
    --target-dir "$TARGET_DIR"

cp "$TARGET_DIR/release/elementary-intelligence" "$OUTPUT"
