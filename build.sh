#!/usr/bin/env bash
#
# build.sh -- builds the seek app.
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

# The deployment target has to be set here rather than left to
# core/.cargo/config.toml. Cargo reads its configuration from the working
# directory, and cargo is invoked below with --manifest-path from the
# repository root, so that file is never read. Without this, the C
# libraries the core links against (notably zstd) are built for whatever
# macOS the build machine runs, and the bundle will not launch on the
# version its Info.plist promises.
export MACOSX_DEPLOYMENT_TARGET="14.0"

echo "==> building the core ($PROFILE)"
if [ "$PROFILE" = "release" ]; then
    cargo build --release --manifest-path "$CORE/Cargo.toml"
    LIB_DIR="$CORE/target/release"
else
    cargo build --manifest-path "$CORE/Cargo.toml"
    LIB_DIR="$CORE/target/debug"
fi

echo "==> building the app ($PROFILE)"
# The app's package manifest reads this to find the core library, so that a
# debug build links the debug library and a release build the release one.
export SEEK_CORE_DIR="$LIB_DIR"
swift build --package-path "$APP" -c "$PROFILE"

# SwiftPM caches the link step, so the binary is relinked when the core has
# changed. Removing it forces the linker to run again.
BINARY="$APP/.build/$PROFILE/Seek"
if [ -e "$BINARY" ]; then
    rm -f "$BINARY"
    swift build --package-path "$APP" -c "$PROFILE"
fi

echo "==> done: $BINARY"
echo "    core:  $LIB_DIR/libseek_core.a"
