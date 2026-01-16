#!/bin/bash
set -e

echo "===================================="
echo "Building LrGeniusAI Installer"
echo "===================================="
echo ""

# Versionsnummer aus Info.lua extrahieren
echo -n "Reading version from Info.lua: "
MAJOR=$(grep '^Info.MAJOR' LrGeniusAI.lrdevplugin/Info.lua | sed -e 's/Info.MAJOR = //' | tr -d ' ')
MINOR=$(grep '^Info.MINOR' LrGeniusAI.lrdevplugin/Info.lua | sed -e 's/Info.MINOR = //' | tr -d ' ')
REVISION=$(grep '^Info.REVISION' LrGeniusAI.lrdevplugin/Info.lua | sed -e 's/Info.REVISION = //' | tr -d ' ')
VERSION="${MAJOR}.${MINOR}.${REVISION}"

echo "$VERSION"
echo ""

# Check if QtIFW is installed
if ! command -v binarycreator &> /dev/null; then
    echo "ERROR: QtIFW binarycreator not found."
    echo "Please install Qt Installer Framework from:"
    echo "https://download.qt.io/official_releases/qt-installer-framework/"
    exit 1
fi

echo "Building installer..."

# Create installer
INSTALLER_OUTPUT="dist/LrGeniusAI-Installer-macOS"

echo "Creating online installer (requires repository)..."
binarycreator --online-only \
    -c installer/config/config_mac.xml \
    -p installer/packages \
    "$INSTALLER_OUTPUT"


echo ""
echo "Creating ZIP archive of installer..."
INSTALLER_ZIP="${INSTALLER_OUTPUT}.zip"
(cd "$(dirname "$INSTALLER_OUTPUT")" && zip -r "$(basename "$INSTALLER_ZIP")" "$(basename "$INSTALLER_OUTPUT.app")")


echo ""
echo "===================================="
echo "Installer created successfully!"
echo "Output: $INSTALLER_OUTPUT.app"
echo "ZIP: $INSTALLER_ZIP"
echo "===================================="
