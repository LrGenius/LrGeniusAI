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
mkdir -p "$ROOT_DIR/Applications/LrGeniusAI/Server"
mkdir -p "$ROOT_DIR/Applications/LrGeniusAI/PluginInstallTemp"
mkdir -p "$ROOT_DIR/Library/LaunchAgents"
mkdir -p "$SCRIPTS_DIR"

# 1. Copy Backend
echo "Copying backend..."
cp -a build/lrgenius-server/. "$ROOT_DIR/Applications/LrGeniusAI/Server/"

# 2. Copy Plugin (to temporary location for postinstall relocation)
echo "Copying plugin..."
cp -a build/LrGeniusAI.lrplugin "$ROOT_DIR/Applications/LrGeniusAI/PluginInstallTemp/LrGeniusAI.lrplugin"

cp installers/macos/com.lrgenius.server.plist "$ROOT_DIR/Library/LaunchAgents/"

# 3.5 Create Uninstaller app
echo "Creating uninstaller..."
UNINSTALL_APP_PATH="$ROOT_DIR/Applications/LrGeniusAI/Uninstall LrGeniusAI.app"
UNINSTALL_SCRIPT=$(cat <<'EOF'
set currentUser to (do shell script "stat -f '%u' /dev/console")
display dialog "Are you sure you want to uninstall LrGeniusAI? This will remove the server, plugin, and all associated logs." with title "Uninstall LrGeniusAI" with icon caution buttons {"Cancel", "Uninstall"} default button "Cancel"
if button returned of result is "Uninstall" then
    try
        set userHome to (do shell script "dscl . -read /Users/$(id -un " & currentUser & ") NFSHomeDirectory | awk '{print $2}'")
        do shell script "launchctl asuser " & currentUser & " launchctl unload /Library/LaunchAgents/com.lrgenius.server.plist 2>/dev/null || true; rm -f /Library/LaunchAgents/com.lrgenius.server.plist; rm -rf '" & userHome & "/Library/Application Support/Adobe/Lightroom/Modules/LrGeniusAI.lrplugin'; rm -rf /Library/Logs/LrGeniusAI; rm -rf /Applications/LrGeniusAI" with administrator privileges
        display dialog "LrGeniusAI has been successfully uninstalled." with title "Uninstall LrGeniusAI" buttons {"OK"} default button "OK"
    on error errMsg
        display dialog "Uninstallation failed: " & errMsg with title "Uninstall LrGeniusAI" buttons {"OK"} default button "OK" with icon stop
    end try
end if
EOF
)
# Use osacompile to create the .app in the pkg_root
osacompile -o "$UNINSTALL_APP_PATH" -e "$UNINSTALL_SCRIPT"

# 4. Create postinstall script to load the service
cat > "$SCRIPTS_DIR/postinstall" <<EOF
#!/bin/bash
# Detect current GUI user
CURRENT_USER=\$(stat -f '%u' /dev/console)
if [ -z "\$CURRENT_USER" ] || [ "\$CURRENT_USER" -eq 0 ]; then
    # Fallback to the first non-root user if console info is missing
    CURRENT_USER=\$(dscl . list /Users UniqueID | awk '\$2 > 500 {print \$2; exit}')
fi

# Setup log directory with correct permissions
LOG_DIR="/Library/Logs/LrGeniusAI"
mkdir -p "\$LOG_DIR"
if [ -n "\$CURRENT_USER" ]; then
    chown "\$CURRENT_USER" "\$LOG_DIR"
    chmod 755 "\$LOG_DIR"
fi

# Load and start the service
PLIST="/Library/LaunchAgents/com.lrgenius.server.plist"
LABEL="com.lrgenius.server"

if [ -n "\$CURRENT_USER" ] && [ "\$CURRENT_USER" -ne 0 ]; then
    echo "Loading service for user \$CURRENT_USER..."
    # Attempt to unload first to handle upgrades cleanly
    launchctl asuser "\$CURRENT_USER" launchctl unload "\$PLIST" 2>/dev/null || true
    
    # Load the agent with -w (enables it)
    launchctl asuser "\$CURRENT_USER" launchctl load -w "\$PLIST"
    
    # Use kickstart to force-start the service immediately
    # Targets gui/<uid>/<label> for LaunchAgents
    launchctl asuser "\$CURRENT_USER" launchctl kickstart -k "gui/\$CURRENT_USER/\$LABEL"

    # Relocate Plugin to current user's Library
    CURRENT_USER_NAME=\$(id -un "\$CURRENT_USER")
    CURRENT_USER_HOME=\$(dscl . -read "/Users/\$CURRENT_USER_NAME" NFSHomeDirectory | awk '{print \$2}')
    if [ -d "\$CURRENT_USER_HOME" ]; then
        PLUGIN_TARGET_DIR="\$CURRENT_USER_HOME/Library/Application Support/Adobe/Lightroom/Modules"
        echo "Relocating plugin to \$PLUGIN_TARGET_DIR"
        mkdir -p "\$PLUGIN_TARGET_DIR"
        # Remove existing if any to ensure clean copy
        rm -rf "\$PLUGIN_TARGET_DIR/LrGeniusAI.lrplugin"
        cp -a "/Applications/LrGeniusAI/PluginInstallTemp/LrGeniusAI.lrplugin" "\$PLUGIN_TARGET_DIR/"
        chown -R "\$CURRENT_USER" "\$PLUGIN_TARGET_DIR/LrGeniusAI.lrplugin"
    fi
    # Cleanup temp folder
    rm -rf "/Applications/LrGeniusAI/PluginInstallTemp"
fi
exit 0
EOF
chmod +x "$SCRIPTS_DIR/postinstall"

# 5. Create preinstall script to stop existing service
cat > "$SCRIPTS_DIR/preinstall" <<EOF
#!/bin/bash
CURRENT_USER=\$(stat -f '%u' /dev/console)
if [ -n "\$CURRENT_USER" ] && [ "\$CURRENT_USER" -ne 0 ]; then
    launchctl asuser "\$CURRENT_USER" launchctl unload /Library/LaunchAgents/com.lrgenius.server.plist 2>/dev/null || true
fi
# Kill any stray backend processes
pkill -f "geniusai_server.py" || true
pkill -f "lrgenius-server" || true
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
