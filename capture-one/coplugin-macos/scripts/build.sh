#!/usr/bin/env bash
#
# Builds LrGeniusAI.coplugin — the Capture One "Open With" shim.
#
# Two supported modes:
#
#   1. OFFICIAL SDK (preferred).  Set CAPTURE_ONE_SDK to the directory that contains
#      CaptureOnePlugins.framework as shipped by Phase One's developer portal. Its real
#      headers are used and the reconstructed ones are ignored.
#
#   2. RECONSTRUCTED (fallback, and what you get out of the box).  No SDK required: the
#      framework binary already ships inside Capture One.app, and ../Headers holds an
#      interface reconstructed from that binary's Objective-C metadata. The build is
#      byte-for-byte a normal plugin bundle, but the header is an educated reconstruction
#      — see ../Headers/README.md for what is verified and what is guessed.
#
# Either way nothing is written into /Applications: a shim framework is assembled in
# build/ from a *copy* of the binary plus whichever headers are in play.
#
# Environment:
#   CAPTURE_ONE_SDK    dir containing CaptureOnePlugins.framework (optional)
#   CAPTURE_ONE_APP    path to Capture One.app (default: /Applications/Capture One.app)
#   ARCHS              space-separated (default: "arm64 x86_64")
#   CODESIGN_IDENTITY  Developer ID Application: ... (optional; required for notarizing)
#   MACOS_MIN          deployment target (default: 14.0)

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
BUILD="$ROOT/build"
DIST="$ROOT/dist"

PLUGIN_NAME="LrGeniusAI"
EXECUTABLE="LrGeniusAIPlugin"
BUNDLE="$DIST/$PLUGIN_NAME.coplugin"

CAPTURE_ONE_APP="${CAPTURE_ONE_APP:-/Applications/Capture One.app}"
ARCHS="${ARCHS:-arm64 x86_64}"
MACOS_MIN="${MACOS_MIN:-14.0}"

die() { printf '\nerror: %s\n\n' "$1" >&2; exit 1; }

command -v swiftc >/dev/null 2>&1 || die \
"swiftc not found. Install the Xcode command line tools:
    xcode-select --install"

# --- Resolve the framework and headers -------------------------------------------------

FRAMEWORK_BIN=""
HEADER_DIR=""
MODE=""

if [[ -n "${CAPTURE_ONE_SDK:-}" ]]; then
    SDK_FW="$CAPTURE_ONE_SDK/CaptureOnePlugins.framework"
    [[ -d "$SDK_FW" ]] || die \
"CAPTURE_ONE_SDK is set to '$CAPTURE_ONE_SDK', but there is no
CaptureOnePlugins.framework inside it.

Point CAPTURE_ONE_SDK at the directory that *contains* the framework, e.g.
    export CAPTURE_ONE_SDK=\"\$HOME/CaptureOneSDK\"
so that \"\$CAPTURE_ONE_SDK/CaptureOnePlugins.framework\" exists.

Or unset CAPTURE_ONE_SDK to build against the reconstructed headers in
$ROOT/Headers instead."

    for candidate in "$SDK_FW/Versions/A/CaptureOnePlugins" "$SDK_FW/CaptureOnePlugins"; do
        [[ -f "$candidate" ]] && { FRAMEWORK_BIN="$candidate"; break; }
    done
    [[ -n "$FRAMEWORK_BIN" ]] || die "no CaptureOnePlugins binary inside '$SDK_FW'."

    for candidate in "$SDK_FW/Versions/A/Headers" "$SDK_FW/Headers"; do
        [[ -d "$candidate" ]] && { HEADER_DIR="$candidate"; break; }
    done
    [[ -n "$HEADER_DIR" ]] || die \
"'$SDK_FW' has no Headers directory, so it cannot be the official SDK.
The copy bundled inside Capture One.app has no headers either — that is expected.
Unset CAPTURE_ONE_SDK to build with the reconstructed headers."

    MODE="official SDK ($CAPTURE_ONE_SDK)"
else
    APP_FW="$CAPTURE_ONE_APP/Contents/Frameworks/CaptureOnePlugins.framework/Versions/A/CaptureOnePlugins"
    [[ -f "$APP_FW" ]] || die \
"Cannot find CaptureOnePlugins.framework.

Looked for the copy inside Capture One:
    $APP_FW

Fix by doing ONE of these:
  * install Capture One (the framework ships inside the app), or
  * point CAPTURE_ONE_APP at it if it lives elsewhere:
        CAPTURE_ONE_APP=/path/to/Capture\\ One.app $0
  * or get the official SDK from the Capture One developer portal
    (https://www.captureone.com — Developer / Plugin SDK, account signup required)
    and set:
        export CAPTURE_ONE_SDK=/path/to/sdk"

    FRAMEWORK_BIN="$APP_FW"
    HEADER_DIR="$ROOT/Headers"
    [[ -f "$HEADER_DIR/CaptureOnePlugins.h" ]] || die "missing $HEADER_DIR/CaptureOnePlugins.h"
    MODE="reconstructed headers ($HEADER_DIR)"
fi

# --- Assemble the shim framework -------------------------------------------------------

rm -rf "$BUILD" "$BUNDLE"
mkdir -p "$BUILD"

SHIM="$BUILD/CaptureOnePlugins.framework"
mkdir -p "$SHIM/Headers" "$SHIM/Modules"
cp "$FRAMEWORK_BIN" "$SHIM/CaptureOnePlugins"
cp "$HEADER_DIR"/*.h "$SHIM/Headers/"
if [[ -f "$ROOT/Headers/module.modulemap" ]]; then
    cp "$ROOT/Headers/module.modulemap" "$SHIM/Modules/module.modulemap"
else
    cat > "$SHIM/Modules/module.modulemap" <<'MODMAP'
framework module CaptureOnePlugins {
    umbrella header "CaptureOnePlugins.h"
    export *
    module * { export * }
}
MODMAP
fi

echo "==> building $PLUGIN_NAME.coplugin"
echo "    interface : $MODE"
echo "    framework : $FRAMEWORK_BIN"
echo "    archs     : $ARCHS"
echo "    min macOS : $MACOS_MIN"

# --- Compile one slice per arch, then lipo ---------------------------------------------

SLICES=()
for arch in $ARCHS; do
    slice="$BUILD/$EXECUTABLE-$arch"
    echo "==> swiftc ($arch)"
    swiftc \
        -target "${arch}-apple-macos${MACOS_MIN}" \
        -swift-version 5 \
        -O \
        -F "$BUILD" -framework CaptureOnePlugins \
        -module-name "$EXECUTABLE" \
        -emit-library -Xlinker -bundle \
        -o "$slice" \
        "$ROOT/Sources"/*.swift
    SLICES+=("$slice")
done

mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"

if [[ ${#SLICES[@]} -gt 1 ]]; then
    lipo -create "${SLICES[@]}" -output "$BUNDLE/Contents/MacOS/$EXECUTABLE"
else
    cp "${SLICES[0]}" "$BUNDLE/Contents/MacOS/$EXECUTABLE"
fi

cp "$ROOT/Info.plist" "$BUNDLE/Contents/Info.plist"
[[ -d "$ROOT/Resources" ]] && cp -R "$ROOT/Resources/." "$BUNDLE/Contents/Resources/" || true

# The plugin must NOT carry an rpath to Capture One's Frameworks directory: the XPC host
# (COPluginHostApple.xpc) supplies it at load time. The shipping Helicon plugin has no
# LC_RPATH at all, and neither does this one — swiftc adds none for a -bundle link.

# --- Sign ------------------------------------------------------------------------------

if [[ -n "${CODESIGN_IDENTITY:-}" ]]; then
    echo "==> codesign ($CODESIGN_IDENTITY)"
    # Hardened runtime is mandatory for notarization.
    codesign --force --timestamp --options runtime \
        --sign "$CODESIGN_IDENTITY" "$BUNDLE"
    codesign --verify --strict --verbose=2 "$BUNDLE"
else
    echo "==> not signed (set CODESIGN_IDENTITY to sign; required before notarizing)"
fi

echo
echo "built: $BUNDLE"
echo
echo "install with:"
echo "    rm -rf ~/Library/Application\\ Support/Capture\\ One/Plug-ins/$PLUGIN_NAME.coplugin"
echo "    cp -R '$BUNDLE' ~/Library/Application\\ Support/Capture\\ One/Plug-ins/"
echo "then restart Capture One."
