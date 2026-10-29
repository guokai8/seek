#!/usr/bin/env bash
#
# bundle.sh -- packages the fuzzylite app into a macOS application bundle.
#
# Usage:
#   ./bundle.sh          build a release bundle in build/Fuzzylite.app
#

set -euo pipefail

PROFILE="release"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CORE="$ROOT/core"
APP="$ROOT/app"
OUT="$ROOT/build"

export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="$CORE/target"

echo "==> building the core ($PROFILE)"
cargo build --release --manifest-path "$CORE/Cargo.toml"

echo "==> building the app ($PROFILE)"
swift build --package-path "$APP" -c "$PROFILE"

BINARY="$APP/.build/$PROFILE/Fuzzylite"
if [ ! -e "$BINARY" ]; then
    echo "the app binary was not produced" >&2
    exit 1
fi

echo "==> assembling the bundle"
BUNDLE="$OUT/Fuzzylite.app"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"

cp "$BINARY" "$BUNDLE/Contents/MacOS/Fuzzylite"
cp "$APP/Resources/Info.plist" "$BUNDLE/Contents/Info.plist"

# The ad-hoc signature is enough for the app to run locally. A real
# distribution build would sign it with a Developer ID.
codesign --force --sign - "$BUNDLE" 2>/dev/null || true

echo "==> done: $BUNDLE"
echo
echo "    install it with:"
echo "      cp -R \"$BUNDLE\" /Applications/"
