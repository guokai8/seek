#!/usr/bin/env bash
#
# build.sh -- builds the fuzzylite app.
#
# The Rust core is compiled into a static library that the Swift app links
# against. SwiftPM does not notice when a static library changes, so the
# app is relinked explicitly after the core is built.
#
# Usage:
#   ./build.sh          build everything in debug
#   ./build.sh release  build everything in release
#

set -euo pipefail

PROFILE="${1:-debug}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CORE="$ROOT/core"
APP="$ROOT/app"

export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$CORE/target"

echo "==> building the core ($PROFILE)"
if [ "$PROFILE" = "release" ]; then
    cargo build --release --manifest-path "$CORE/Cargo.toml"
    LIB_DIR="$CORE/target/release"
else
    cargo build --manifest-path "$CORE/Cargo.toml"
    LIB_DIR="$CORE/target/debug"
fi

echo "==> building the app ($PROFILE)"
swift build --package-path "$APP" -c "$PROFILE"

# SwiftPM caches the link step, so the binary is relinked when the core has
# changed. Removing it forces the linker to run again.
BINARY="$APP/.build/$PROFILE/Fuzzylite"
if [ -e "$BINARY" ]; then
    rm -f "$BINARY"
    swift build --package-path "$APP" -c "$PROFILE"
fi

echo "==> done: $BINARY"
echo "    core:  $LIB_DIR/libfuzzylite_core.a"
