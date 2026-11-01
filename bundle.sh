#!/usr/bin/env bash
#
# bundle.sh -- packages the seek app into a macOS application bundle.
#
# Usage:
#   ./bundle.sh          build a release bundle in build/Seek.app
#

set -euo pipefail

PROFILE="release"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CORE="$ROOT/core"
APP="$ROOT/app"
OUT="$ROOT/build"

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
cargo build --release --manifest-path "$CORE/Cargo.toml"

# The app links against the core as a static library, and it has to be the
# one that was just built. Without this the app would link against the
# debug build of the core, which does not have the newer entry points.
export SEEK_CORE_DIR="$CARGO_TARGET_DIR/$PROFILE"

echo "==> building the app ($PROFILE)"
swift build --package-path "$APP" -c "$PROFILE"

BINARY="$APP/.build/$PROFILE/Seek"
if [ ! -e "$BINARY" ]; then
    echo "the app binary was not produced" >&2
    exit 1
fi

echo "==> assembling the bundle"
BUNDLE="$OUT/Seek.app"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"

cp "$BINARY" "$BUNDLE/Contents/MacOS/Seek"
cp "$APP/Resources/Info.plist" "$BUNDLE/Contents/Info.plist"

# The icon is drawn from source at every size rather than carried as a
# picture in the repository, so a change to it is a change with a diff and
# each rendition is drawn at its own size instead of resampled from one
# large one. See tools/make-icon.swift.
echo "==> drawing the icon"
ICONSET="$OUT/Seek.iconset"
rm -rf "$ICONSET"
swift "$ROOT/tools/make-icon.swift" "$ICONSET"
iconutil -c icns "$ICONSET" -o "$BUNDLE/Contents/Resources/Seek.icns"
rm -rf "$ICONSET"

# The ad-hoc signature is enough for the app to run locally. A real
# distribution build would sign it with a Developer ID.
codesign --force --sign - "$BUNDLE" 2>/dev/null || true

echo "==> done: $BUNDLE"
echo
echo "    install it with:"
echo "      cp -R \"$BUNDLE\" /Applications/"
