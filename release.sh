#!/bin/bash

# Versionsnummer aus Info.lua extrahieren
MAJOR=$(grep '^Info.MAJOR' LrGeniusAI.lrdevplugin/Info.lua | sed -e 's/Info.MAJOR = //' | tr -d ' ')
MINOR=$(grep '^Info.MINOR' LrGeniusAI.lrdevplugin/Info.lua | sed -e 's/Info.MINOR = //' | tr -d ' ')
REVISION=$(grep '^Info.REVISION' LrGeniusAI.lrdevplugin/Info.lua | sed -e 's/Info.REVISION = //' | tr -d ' ')
VERSION="${MAJOR}.${MINOR}.${REVISION}"

SERVER_VERSION=$(cat ../geniusai-server/version.txt)

echo "Starting full release process for version: $VERSION..."
echo "Server version: $SERVER_VERSION"



echo -n "Cleaning previous builds... "
./clean-all.sh
echo "done."


echo -n "Writing version file... "
mkdir -p dist
echo $VERSION > dist/version.txt
echo "done."


echo -n "Building all components... "
./build-all.sh
echo "done."
# echo -n "Building installer and repository... "
# ./build-installer.sh
# echo "done."
echo -n "Building repository... "
./build-repository.sh
echo "done."

echo "Create git tag for plugin version $VERSION"
git tag -a "v$VERSION" -m "Release version $VERSION"
git push gitea "v$VERSION"
echo "Create git tag for server version $SERVER_VERSION"
git -C ../geniusai-server/ tag -a "v$SERVER_VERSION" -m "Release version $SERVER_VERSION"
git -C ../geniusai-server/ push gitea "v$SERVER_VERSION"
echo "Git tags created and pushed."

echo "Release process completed successfully."
