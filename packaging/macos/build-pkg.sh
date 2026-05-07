#!/usr/bin/env bash
# Wraps the cargo-packager .app output in a .pkg installer that lets the
# user choose between system-wide (/Applications) and per-user
# (~/Applications) install via the standard macOS Installer.app domain
# chooser.
#
# cargo-packager itself doesn't produce .pkg -- only .app and .dmg.
# This script bridges the gap with pkgbuild + productbuild, which ship
# with the Xcode Command Line Tools.
#
# Usage:
#   1. cargo packager --release -p wayflow-cli --formats app
#   2. ./packaging/macos/build-pkg.sh
#
# Output: target/packages/Wayflow_<version>_<arch>.pkg
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
APP="$ROOT/target/packages/Wayflow.app"
DIST_XML="$ROOT/packaging/macos/Distribution.xml"
OUT_DIR="$ROOT/target/packages"
COMPONENT_PKG="$OUT_DIR/wayflow-component.pkg"

if [[ ! -d "$APP" ]]; then
    echo "missing .app -- run cargo packager --formats app first" >&2
    exit 1
fi

# cargo pkgid emits "...#<version>"; trim everything before the # to get
# the bare version. Matches what cargo-packager uses for its filenames.
VERSION="$(cd "$ROOT" && cargo pkgid -p wayflow-cli | sed 's/.*[#@]//')"
ARCH="$(uname -m)"
FINAL_PKG="$OUT_DIR/Wayflow_${VERSION}_${ARCH}.pkg"

# 1. Build a component package from the .app. --install-location sets the
#    default destination; Installer.app rewrites this to ~/Applications
#    when the user picks "Install for me only" because Distribution.xml
#    sets enable_currentUserHome="true".
pkgbuild \
    --component "$APP" \
    --identifier "com.evattlabs.wayflow" \
    --version "$VERSION" \
    --install-location "/Applications" \
    "$COMPONENT_PKG"

# 2. Wrap with productbuild + Distribution.xml to surface the install-
#    location chooser.
productbuild \
    --distribution "$DIST_XML" \
    --package-path "$OUT_DIR" \
    "$FINAL_PKG"

# Cleanup intermediate component package -- only the wrapped .pkg is
# distributable.
rm -f "$COMPONENT_PKG"

echo "built: $FINAL_PKG"
