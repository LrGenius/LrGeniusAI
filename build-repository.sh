#!/bin/bash
set -e

echo "===================================="
echo "Building LrGeniusAI Online Repository"
echo "===================================="
echo ""

# Get version from Info.lua
echo -n "Reading version from Info.lua: "
MAJOR=$(grep '^Info.MAJOR' LrGeniusAI.lrdevplugin/Info.lua | sed -e 's/Info.MAJOR = //' | tr -d ' ')
MINOR=$(grep '^Info.MINOR' LrGeniusAI.lrdevplugin/Info.lua | sed -e 's/Info.MINOR = //' | tr -d ' ')
REVISION=$(grep '^Info.REVISION' LrGeniusAI.lrdevplugin/Info.lua | sed -e 's/Info.REVISION = //' | tr -d ' ')
VERSION="${MAJOR}.${MINOR}.${REVISION}"
SERVER_VERSION=$(cat ../geniusai-server/version.txt)

echo "$VERSION"
echo "Server version: $SERVER_VERSION"
echo ""

# Check if QtIFW is installed
if ! command -v repogen &> /dev/null; then
    echo "ERROR: QtIFW repogen not found."
    echo "Please install Qt Installer Framework from:"
    echo "https://download.qt.io/official_releases/qt-installer-framework/"
    exit 1
fi

# Extract ZIP files to package data directories
echo "Copying distribution files to package data directories..."

# Update version in config_mac.xml
echo "Updating version in config files to $VERSION..."
sed -i '' "s/<Version>.*<\/Version>/<Version>$VERSION<\/Version>/" installer/config/config_mac.xml
sed -i '' "s/<Version>.*<\/Version>/<Version>$VERSION<\/Version>/" installer/packages/com.lrgenius.plugin/meta/package.xml
sed -i '' "s/<Version>.*<\/Version>/<Version>$SERVER_VERSION<\/Version>/" installer/packages/com.lrgenius.server/meta/package.xml

# Clean and extract Lua plugin ZIP to LrGeniusAI.lrplugin subdirectory
echo "Copying Lua plugin..."
rm -rf "installer/packages/com.lrgenius.plugin/data"/*
mkdir -p "installer/packages/com.lrgenius.plugin/data/LrGeniusAI.lrplugin"
rsync -av build/LrGeniusAI.lrplugin/ installer/packages/com.lrgenius.plugin/data/LrGeniusAI.lrplugin/


# Clean and extract Server ZIP to lrgenius-server subdirectory
echo "Cleaning server data directory..."
rm -rf "installer/packages/com.lrgenius.server/data"/*


echo "Copying server files..."
rsync -av ../geniusai-server/dist/lrgenius-server/ installer/packages/com.lrgenius.server/data/lrgenius-server/

# Create repository directory
REPO_DIR="repository"
mkdir -p "$REPO_DIR"

echo "Generating repository..."

# Generate repository using repogen
repogen --update-new-components -p installer/packages "$REPO_DIR"

echo "Compressing repository..."
# Compress repository
tar -czf dist/LrGeniusAI_Repository_macOS_$VERSION.tar.gz -C "$REPO_DIR" .
