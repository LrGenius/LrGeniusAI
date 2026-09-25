#!/usr/bin/env bash
# Stamp plugin/LrGeniusAI.lrdevplugin/Info.lua with the release version, then
# verify the stamp actually landed.
#
#   Usage: scripts/set_plugin_version.sh <ref-name> <build-date>
#
# Replaces four unverified `sed` calls that were duplicated verbatim across two
# release jobs. The fourth of them read
#
#     sed 's/Info.BUILD = [0-9]*/build = '"$BUILD_DATE"'/'
#
# whose search pattern covers the left-hand side but whose replacement does not,
# so every shipped Info.lua carried a bare global `build = <date>` and no
# `Info.BUILD`. `Info.VERSION.build` was therefore nil and APISearchIndex.lua
# reported plugin_build = 0. The `echo` that followed claimed success, and the
# sed only ever ran inside CI, so it could not reproduce locally.
#
# The lesson, and the reason this is a script rather than four inline seds:
# a write that is not read back is not a write. Stamp, then assert.
set -euo pipefail

REF_NAME="${1:?ref name required}"
BUILD_DATE="${2:?build date required}"
INFO_LUA="plugin/LrGeniusAI.lrdevplugin/Info.lua"

# Tag pushes carry the version; workflow_dispatch dry runs off a branch fall
# back to the same dev version the binary itself defaults to, so the
# plugin<->backend version-check contract still holds (lrg-common/src/version.rs
# accepts the 9.9.9 placeholder only for a dev backend).
if [[ "$REF_NAME" == v[0-9]* ]]; then
  VERSION="${REF_NAME#v}"
else
  VERSION="0.0.0-dev"
fi

# Info.MAJOR/MINOR/REVISION are plain Lua number literals, so drop any
# prerelease suffix ("3.0.0-pre3" -> "3.0.0") before splitting.
VERSION_NUMERIC="${VERSION%%-*}"
IFS='.' read -r MAJOR MINOR REVISION <<< "$VERSION_NUMERIC"
MAJOR=${MAJOR:-0}
MINOR=${MINOR:-0}
REVISION=${REVISION:-0}

# Refuse to write anything that is not a Lua number literal. Without this a
# branch named e.g. "ci" would stamp `Info.MAJOR = ci`, a nil global at runtime.
for pair in "MAJOR=$MAJOR" "MINOR=$MINOR" "REVISION=$REVISION" "BUILD=$BUILD_DATE"; do
  case "${pair#*=}" in
    '' | *[!0-9]*)
      echo "::error::Info.${pair%%=*} would be stamped to non-numeric '${pair#*=}' (ref '$REF_NAME')"
      exit 1
      ;;
  esac
done

tmp="$INFO_LUA.tmp"
sed -e "s/^Info\.MAJOR = .*/Info.MAJOR = $MAJOR/" \
  -e "s/^Info\.MINOR = .*/Info.MINOR = $MINOR/" \
  -e "s/^Info\.REVISION = .*/Info.REVISION = $REVISION/" \
  -e "s/^Info\.BUILD = .*/Info.BUILD = $BUILD_DATE/" \
  "$INFO_LUA" > "$tmp" && mv "$tmp" "$INFO_LUA"

# Read the stamp back. Each field must still be its own assignment line.
fail=0
for pair in "MAJOR=$MAJOR" "MINOR=$MINOR" "REVISION=$REVISION" "BUILD=$BUILD_DATE"; do
  if ! grep -qx "Info\.${pair%%=*} = ${pair#*=}" "$INFO_LUA"; then
    echo "::error::Info.${pair%%=*} was not stamped to ${pair#*=}"
    fail=1
  fi
done
if [ "$fail" -ne 0 ]; then
  echo '---- Info.lua head ----'
  sed -n '1,8p' "$INFO_LUA"
  exit 1
fi

echo "Stamped Info.lua: MAJOR=$MAJOR MINOR=$MINOR REVISION=$REVISION BUILD=$BUILD_DATE"
