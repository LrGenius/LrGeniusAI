#!/usr/bin/env bash
#
# Notarizes LrGeniusAI.coplugin and produces the distributable archive.
#
# WHY THIS IS A SCRIPT AND NOT AN XCODE BUTTON
# --------------------------------------------
# Xcode's Organizer can only notarize what it can archive: apps, and packages it builds
# through a scheme's Archive action. A loadable bundle (CFBundlePackageType BNDL) that is
# not embedded in an app never appears there, so "Product > Archive > Distribute" is not
# an option for a .coplugin. The notarization service itself has no such limitation — it
# just needs a signed payload uploaded by notarytool. Hence the CLI.
#
# ORDER OF OPERATIONS (this part is easy to get wrong)
# ----------------------------------------------------
#   1. sign the bundle (hardened runtime, secure timestamp)   <- scripts/build.sh does this
#   2. zip it, because notarytool cannot accept a bare directory
#   3. submit that zip and wait for acceptance
#   4. staple the ticket to THE BUNDLE, not to the zip
#      A zip cannot hold a stapled ticket. Stapling the zip either fails outright or
#      staples nothing useful; the ticket has to live inside the bundle so that it
#      survives being expanded on the user's machine.
#   5. re-archive the now-stapled bundle for distribution
#
# Step 4 is the reason the upload zip and the distribution zip are two different files:
# the first is a throwaway, the second contains the ticket.
#
# Credentials — use ONE of:
#   NOTARY_PROFILE   name of a profile stored with:
#                        xcrun notarytool store-credentials <name> \
#                            --apple-id <id> --team-id <team> --password <app-specific-pw>
#   or APPLE_ID + TEAM_ID + APP_PASSWORD  (app-specific password, not your Apple ID password)
#
# Optional:
#   BUNDLE           path to the .coplugin (default: ../dist/LrGeniusAI.coplugin)

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"

BUNDLE="${BUNDLE:-$ROOT/dist/LrGeniusAI.coplugin}"
NAME="$(basename "$BUNDLE" .coplugin)"
WORK="$ROOT/build/notarize"
UPLOAD_ZIP="$WORK/$NAME-upload.zip"
DIST_ARCHIVE="$ROOT/dist/$NAME.coplugin.zip"

die() { printf '\nerror: %s\n\n' "$1" >&2; exit 1; }

[[ -d "$BUNDLE" ]] || die \
"no bundle at '$BUNDLE'.
Build it first:
    scripts/build.sh
or point BUNDLE at an existing one:
    BUNDLE=/path/to/LrGeniusAI.coplugin $0"

command -v xcrun >/dev/null 2>&1 || die "xcrun not found. Install the Xcode command line tools."

# --- Credentials -----------------------------------------------------------------------

NOTARY_AUTH=()
if [[ -n "${NOTARY_PROFILE:-}" ]]; then
    NOTARY_AUTH=(--keychain-profile "$NOTARY_PROFILE")
elif [[ -n "${APPLE_ID:-}" && -n "${TEAM_ID:-}" && -n "${APP_PASSWORD:-}" ]]; then
    NOTARY_AUTH=(--apple-id "$APPLE_ID" --team-id "$TEAM_ID" --password "$APP_PASSWORD")
else
    die \
"no notarization credentials.

Store a reusable profile once:
    xcrun notarytool store-credentials LrGeniusAI \\
        --apple-id you@example.com --team-id ABCDE12345 --password <app-specific-password>
    export NOTARY_PROFILE=LrGeniusAI

or export APPLE_ID, TEAM_ID and APP_PASSWORD for a one-off run.
APP_PASSWORD must be an app-specific password from appleid.apple.com, not your
Apple ID password."
fi

# --- 1. The bundle must already be signed with the hardened runtime ---------------------

echo "==> verifying signature"
codesign --verify --strict --verbose=2 "$BUNDLE" 2>&1 || die \
"'$BUNDLE' is not validly signed.
Rebuild with a Developer ID identity:
    CODESIGN_IDENTITY='Developer ID Application: Your Name (TEAMID)' scripts/build.sh
Ad-hoc and unsigned bundles are rejected by the notary service."

if ! codesign --display --verbose=2 "$BUNDLE" 2>&1 | grep -q 'flags=.*runtime'; then
    die \
"'$BUNDLE' is signed without the hardened runtime, which notarization requires.
scripts/build.sh passes --options runtime; re-run it with CODESIGN_IDENTITY set."
fi

# --- 2. Zip for upload -----------------------------------------------------------------

rm -rf "$WORK"; mkdir -p "$WORK"
echo "==> packing for upload"
# ditto --keepParent preserves the .coplugin directory as the archive's top-level entry
# and keeps resource forks / extended attributes that codesign relies on. Plain `zip`
# does not and can invalidate the signature.
ditto -c -k --keepParent "$BUNDLE" "$UPLOAD_ZIP"

# --- 3. Submit and wait ----------------------------------------------------------------

echo "==> submitting to the notary service (this can take a few minutes)"
if ! xcrun notarytool submit "$UPLOAD_ZIP" "${NOTARY_AUTH[@]}" --wait; then
    echo
    echo "Submission was not accepted. Get the details with:"
    echo "    xcrun notarytool history ${NOTARY_AUTH[*]}"
    echo "    xcrun notarytool log <submission-id> ${NOTARY_AUTH[*]}"
    die "notarization failed."
fi

# --- 4. Staple the BUNDLE (never the zip) ----------------------------------------------

echo "==> stapling ticket to the bundle"
xcrun stapler staple "$BUNDLE" || die \
"stapling failed. The notary service accepted the upload, but the ticket could not be
attached. Re-run this script; tickets become available a short while after acceptance."

xcrun stapler validate "$BUNDLE" || die "the stapled ticket did not validate."

# --- 5. Re-archive the stapled bundle for distribution ----------------------------------

echo "==> packing the stapled bundle for distribution"
rm -f "$DIST_ARCHIVE"
ditto -c -k --keepParent "$BUNDLE" "$DIST_ARCHIVE"

# Capture One distributes plugins as a zip archive that carries the .coplugin extension;
# the app expands it on install. Ship BOTH names from the same bytes so a download works
# whether the user double-clicks it or drops the expanded bundle into Plug-ins/ by hand.
cp "$DIST_ARCHIVE" "$ROOT/dist/$NAME-installer.coplugin"

echo
echo "notarized and stapled: $BUNDLE"
echo "distributable archive: $DIST_ARCHIVE"
echo "installer (same bytes, Capture One extension): $ROOT/dist/$NAME-installer.coplugin"
echo
echo "sanity check on another Mac:"
echo "    spctl -a -vvv -t install '$BUNDLE'"
