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

# The bundle is universal: one binary that runs natively on Apple silicon
# and on Intel. `build.sh` stays native-only, because that is the one you
# wait for on every change; this is the one that ships.
ARCHS=(arm64 x86_64)
TRIPLES=(aarch64-apple-darwin x86_64-apple-darwin)

echo "==> building the core ($PROFILE, ${ARCHS[*]})"
for triple in "${TRIPLES[@]}"; do
    if ! rustup target list --installed | grep -qx "$triple"; then
        echo "the Rust target $triple is not installed" >&2
        echo "  install it with: rustup target add $triple" >&2
        exit 1
    fi

    cargo build --release --target "$triple" --manifest-path "$CORE/Cargo.toml"
done

# The two static libraries are joined into one universal archive, so that
# the linker can take the slice it needs for each architecture it is asked
# for. Naming a per-triple directory instead would mean linking twice by
# hand and lipo-ing the results, which is the same work further downstream.
UNIVERSAL="$CARGO_TARGET_DIR/universal/$PROFILE"
mkdir -p "$UNIVERSAL"
lipo -create \
    "$CARGO_TARGET_DIR/aarch64-apple-darwin/$PROFILE/libseek_core.a" \
    "$CARGO_TARGET_DIR/x86_64-apple-darwin/$PROFILE/libseek_core.a" \
    -output "$UNIVERSAL/libseek_core.a"

# The app links against the core as a static library, and it has to be the
# one that was just built. Without this the app would link against the
# debug build of the core, which does not have the newer entry points.
export SEEK_CORE_DIR="$UNIVERSAL"

ARCH_FLAGS=()
for arch in "${ARCHS[@]}"; do
    ARCH_FLAGS+=(--arch "$arch")
done

echo "==> building the app ($PROFILE, ${ARCHS[*]})"
swift build --package-path "$APP" -c "$PROFILE" "${ARCH_FLAGS[@]}"

# A multi-architecture build does not land where a single-architecture one
# does: SwiftPM puts it under the platform's own products directory, with
# the configuration capitalised.
BINARY="$APP/.build/apple/Products/Release/Seek"
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

# The bundle says what it is, so the bundle is asked. A universal build
# that quietly produced one slice — a missing Rust target, a lipo that
# took the wrong input — looks exactly like a working build until it is
# copied to the other kind of Mac.
echo "==> checking the binary"
BUILT_ARCHS="$(lipo -archs "$BUNDLE/Contents/MacOS/Seek")"
for arch in "${ARCHS[@]}"; do
    if ! grep -qw "$arch" <<< "$BUILT_ARCHS"; then
        echo "the binary has no $arch slice (it has: $BUILT_ARCHS)" >&2
        exit 1
    fi
done

# Every slice has its own minimum, and only the one that was built on this
# machine is covered by MACOSX_DEPLOYMENT_TARGET being right.
MINIMA="$(otool -l "$BUNDLE/Contents/MacOS/Seek" | awk '/minos/ {print $2}')"
for minimum in $MINIMA; do
    if [ "$minimum" != "14.0" ]; then
        echo "a slice is built for macOS $minimum rather than 14.0" >&2
        exit 1
    fi
done

echo "    $BUILT_ARCHS, macOS $(head -1 <<< "$MINIMA")+"

# A disk image, because that is what gets sent to someone else. A bare
# .app loses its signature travelling through most things that carry
# files; a .dmg arrives as the one file it left as, and the symlink next
# to it makes installing the drag everyone already knows.
echo "==> making the disk image"
VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' \
    "$BUNDLE/Contents/Info.plist")"
DMG="$OUT/Seek-$VERSION.dmg"
STAGE="$OUT/dmg"

rm -rf "$STAGE" "$DMG"
mkdir -p "$STAGE"
cp -R "$BUNDLE" "$STAGE/"
ln -s /Applications "$STAGE/Applications"

hdiutil create -volname "Seek $VERSION" -srcfolder "$STAGE" \
    -ov -format UDZO "$DMG" > /dev/null
rm -rf "$STAGE"

# The image is asked what it contains, for the same reason the binary is:
# an image built from a half-assembled bundle mounts and looks right.
echo "==> checking the disk image"
MOUNT="$(mktemp -d)"
hdiutil attach "$DMG" -mountpoint "$MOUNT" -nobrowse -quiet
INSIDE="$MOUNT/Seek.app"
if [ ! -x "$INSIDE/Contents/MacOS/Seek" ]; then
    hdiutil detach "$MOUNT" -quiet || true
    echo "the disk image has no runnable app in it" >&2
    exit 1
fi
DMG_ARCHS="$(lipo -archs "$INSIDE/Contents/MacOS/Seek")"
for arch in "${ARCHS[@]}"; do
    if ! grep -qw "$arch" <<< "$DMG_ARCHS"; then
        hdiutil detach "$MOUNT" -quiet || true
        echo "the app in the image has no $arch slice (it has: $DMG_ARCHS)" >&2
        exit 1
    fi
done
codesign --verify --deep "$INSIDE" 2>/dev/null \
    || echo "    note: the signature is ad-hoc, so Gatekeeper will ask on first open"
hdiutil detach "$MOUNT" -quiet
rmdir "$MOUNT" 2>/dev/null || true

echo "    $DMG_ARCHS, $(du -h "$DMG" | cut -f1)"

echo "==> done: $BUNDLE"
echo "==> done: $DMG"
echo
echo "    install it with:"
echo "      cp -R \"$BUNDLE\" /Applications/"
echo
echo "    or send the disk image:"
echo "      $DMG"
