#!/bin/bash
set -euo pipefail

# This script builds a macOS .pkg installer for LrGeniusAI.
# It assumes the single Rust backend binary is at
# build/lrgenius-server/lrgenius-server and the plugin is built in
# build/LrGeniusAI.lrplugin/
#
# Codesigning (optional, controlled by env vars set by CI):
#   MACOS_SIGN_IDENTITY            "Developer ID Application: NAME (TEAMID)"
#   MACOS_INSTALLER_SIGN_IDENTITY  "Developer ID Installer: NAME (TEAMID)"
# When both are unset, the .pkg is built unsigned (useful for local/dev builds).

VERSION="${1:-1.0.0}"
ARCH="${2:-arm64}"
IDENTIFIER="com.lrgenius.installer"
INSTALLER_NAME="LrGeniusAI-macos-${ARCH}-${VERSION}.pkg"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ROOT_DIR="pkg_root"
SCRIPTS_DIR="pkg_scripts"
rm -rf "$ROOT_DIR" "$SCRIPTS_DIR"
mkdir -p "$ROOT_DIR/Applications/LrGeniusAI/Server"
mkdir -p "$ROOT_DIR/Applications/LrGeniusAI/PluginInstallTemp"
mkdir -p "$ROOT_DIR/Library/LaunchAgents"
mkdir -p "$SCRIPTS_DIR"

# 1. Copy backend binary
echo "Copying backend binary..."
cp "build/lrgenius-server/lrgenius-server" "$ROOT_DIR/Applications/LrGeniusAI/Server/lrgenius-server"
chmod +x "$ROOT_DIR/Applications/LrGeniusAI/Server/lrgenius-server"

# 1.5 Codesign the binary (before it's placed into the pkg payload).
if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
  echo "Codesigning backend binary with identity: ${MACOS_SIGN_IDENTITY}"
  codesign --force --options runtime --timestamp \
    --entitlements "$SCRIPT_DIR/entitlements.plist" \
    --sign "$MACOS_SIGN_IDENTITY" \
    "$ROOT_DIR/Applications/LrGeniusAI/Server/lrgenius-server"
  codesign --verify --verbose "$ROOT_DIR/Applications/LrGeniusAI/Server/lrgenius-server"
else
  echo "MACOS_SIGN_IDENTITY not set — building unsigned binary (dev build)."
fi

# 2. Copy Plugin (to temporary location for postinstall relocation)
echo "Copying plugin..."
cp -a build/LrGeniusAI.lrplugin "$ROOT_DIR/Applications/LrGeniusAI/PluginInstallTemp/LrGeniusAI.lrplugin"

cp "$SCRIPT_DIR/com.lrgenius.server.plist" "$ROOT_DIR/Library/LaunchAgents/"

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
if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
  codesign --force --options runtime --timestamp --sign "$MACOS_SIGN_IDENTITY" "$UNINSTALL_APP_PATH"
fi

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
    CURRENT_USER_HOME=\$(dscl . -read "/Users/\$CURRENT_USER_NAME" NFSHomeDirectory | awk '{print $2}')
    if [ -d "\$CURRENT_USER_HOME" ]; then
        PLUGIN_TARGET_DIR="\$CURRENT_USER_HOME/Library/Application Support/Adobe/Lightroom/Modules"
        echo "Relocating plugin to \$PLUGIN_TARGET_DIR"
        sudo -u "\$CURRENT_USER_NAME" mkdir -p "\$PLUGIN_TARGET_DIR"
        # Remove existing if any to ensure clean copy
        rm -rf "\$PLUGIN_TARGET_DIR/LrGeniusAI.lrplugin"
        cp -a "/Applications/LrGeniusAI/PluginInstallTemp/LrGeniusAI.lrplugin" "\$PLUGIN_TARGET_DIR/"
        chown -R "\$CURRENT_USER" "\$PLUGIN_TARGET_DIR/LrGeniusAI.lrplugin"
    fi
    # Cleanup temp folder
    rm -rf "/Applications/LrGeniusAI/PluginInstallTemp"

    # Download InsightFace face-recognition models if not already present
    # (same location Python's insightface library uses: ~/.insightface).
    # Backgrounded + logged since the zip is ~275MB and shouldn't block
    # the installer UI from finishing.
    (
        INSIGHTFACE_DIR="\$CURRENT_USER_HOME/.insightface/models/buffalo_l"
        if [ ! -f "\$INSIGHTFACE_DIR/det_10g.onnx" ] || [ ! -f "\$INSIGHTFACE_DIR/w600k_r50.onnx" ]; then
            echo "Downloading InsightFace buffalo_l models..."
            sudo -u "\$CURRENT_USER_NAME" mkdir -p "\$INSIGHTFACE_DIR"
            TMP_ZIP=\$(mktemp /tmp/buffalo_l.XXXXXX.zip)
            TMP_EXTRACT=\$(mktemp -d /tmp/buffalo_l_extract.XXXXXX)
            if curl -fL -o "\$TMP_ZIP" "https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip"; then
                unzip -oq "\$TMP_ZIP" -d "\$TMP_EXTRACT"
                DET_SRC=\$(find "\$TMP_EXTRACT" -name "det_10g.onnx" -print -quit)
                REC_SRC=\$(find "\$TMP_EXTRACT" -name "w600k_r50.onnx" -print -quit)
                if [ -n "\$DET_SRC" ] && [ -n "\$REC_SRC" ]; then
                    cp "\$DET_SRC" "\$INSIGHTFACE_DIR/det_10g.onnx"
                    cp "\$REC_SRC" "\$INSIGHTFACE_DIR/w600k_r50.onnx"
                    chown -R "\$CURRENT_USER" "\$CURRENT_USER_HOME/.insightface"
                    echo "InsightFace models installed successfully."
                else
                    echo "Warning: expected model files not found in buffalo_l.zip" >&2
                fi
            else
                echo "Warning: failed to download InsightFace models; face detection will be unavailable." >&2
            fi
            rm -rf "\$TMP_ZIP" "\$TMP_EXTRACT"
        fi
    ) >> "\$LOG_DIR/insightface-download.log" 2>&1 &
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
if [ -n "${MACOS_INSTALLER_SIGN_IDENTITY:-}" ]; then
  echo "Signing product archive with identity: ${MACOS_INSTALLER_SIGN_IDENTITY}"
  productbuild --package "LrGeniusAI_component.pkg" --sign "$MACOS_INSTALLER_SIGN_IDENTITY" "$INSTALLER_NAME"
else
  echo "MACOS_INSTALLER_SIGN_IDENTITY not set — building unsigned installer (dev build)."
  productbuild --package "LrGeniusAI_component.pkg" "$INSTALLER_NAME"
fi

echo "Installer created: $INSTALLER_NAME"
rm LrGeniusAI_component.pkg
# Keep folders for debugging if needed, or remove them
# rm -rf "$ROOT_DIR" "$SCRIPTS_DIR"
