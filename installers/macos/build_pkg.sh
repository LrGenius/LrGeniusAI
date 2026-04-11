#!/bin/bash
set -euo pipefail

# This script builds a macOS .pkg installer for LrGeniusAI.
# It assumes the backend is built in build/lrgenius-server/
# and the plugin is built in build/LrGeniusAI.lrplugin/

VERSION="${1:-1.0.0}"
ARCH="${2:-arm64}"
IDENTIFIER="com.lrgenius.installer"
INSTALLER_NAME="LrGeniusAI-macos-${ARCH}-${VERSION}.pkg"

ROOT_DIR="pkg_root"
SCRIPTS_DIR="pkg_scripts"
rm -rf "$ROOT_DIR" "$SCRIPTS_DIR"
mkdir -p "$ROOT_DIR/Applications/LrGeniusAI/backend"
mkdir -p "$ROOT_DIR/Library/Application Support/Adobe/Lightroom/Modules"
mkdir -p "$ROOT_DIR/Library/LaunchAgents"
mkdir -p "$SCRIPTS_DIR"

# 1. Copy Backend
echo "Copying backend..."
cp -a build/lrgenius-server/. "$ROOT_DIR/Applications/LrGeniusAI/backend/"

# 2. Copy Plugin
echo "Copying plugin..."
cp -a build/LrGeniusAI.lrplugin "$ROOT_DIR/Library/Application Support/Adobe/Lightroom/Modules/LrGeniusAI.lrplugin"

# 3. Copy LaunchAgent Plist
echo "Copying launchd plist..."
cp installers/macos/com.lrgenius.server.plist "$ROOT_DIR/Library/LaunchAgents/"

# 4. Create postinstall script to load the service
cat > "$SCRIPTS_DIR/postinstall" <<EOF
#!/bin/bash
# Load the launchd agent for the current user
CURRENT_USER=\$(stat -f '%u' /dev/console)
if [ -n "\$CURRENT_USER" ] && [ "\$CURRENT_USER" -ne 0 ]; then
    launchctl asuser \$CURRENT_USER launchctl load -w /Library/LaunchAgents/com.lrgenius.server.plist
fi
exit 0
EOF
chmod +x "$SCRIPTS_DIR/postinstall"

# 5. Create preinstall script to stop existing service
cat > "$SCRIPTS_DIR/preinstall" <<EOF
#!/bin/bash
CURRENT_USER=\$(stat -f '%u' /dev/console)
if [ -n "\$CURRENT_USER" ] && [ "\$CURRENT_USER" -ne 0 ]; then
    launchctl asuser \$CURRENT_USER launchctl unload /Library/LaunchAgents/com.lrgenius.server.plist 2>/dev/null || true
fi
# Also kill any stray processes
pkill -f "geniusai_server.py" || true
exit 0
EOF
chmod +x "$SCRIPTS_DIR/preinstall"

# 6. Build the package
echo "Building package..."
pkgbuild --root "$ROOT_DIR" \
         --scripts "$SCRIPTS_DIR" \
         --identifier "$IDENTIFIER" \
         --version "$VERSION" \
         --install-location "/" \
         "LrGeniusAI_component.pkg"

# 7. Create product archive (adds UI/metadata if needed, here just a wrapper)
productbuild --package "LrGeniusAI_component.pkg" "$INSTALLER_NAME"

echo "Installer created: $INSTALLER_NAME"
rm LrGeniusAI_component.pkg
# Keep folders for debugging if needed, or remove them
# rm -rf "$ROOT_DIR" "$SCRIPTS_DIR"
