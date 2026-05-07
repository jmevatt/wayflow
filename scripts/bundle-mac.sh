#!/usr/bin/env bash
# Bundle the wayflow tray as a macOS .app so it lives in the menu bar
# (LSUIElement) without a dock icon. Run this on macOS after `cargo build`.
#
#   ./scripts/bundle-mac.sh                 # uses target/release/wayflow
#   ./scripts/bundle-mac.sh debug           # uses target/debug/wayflow
#
# Output: target/Wayflow.app, double-clickable.
set -euo pipefail

PROFILE="${1:-release}"
ROOT="$(git rev-parse --show-toplevel)"
BIN="$ROOT/target/$PROFILE/wayflow"
APP="$ROOT/target/Wayflow.app"
PLIST="$ROOT/crates/wayflow-tray/macos/Info.plist"

if [[ ! -x "$BIN" ]]; then
    echo "missing binary: $BIN" >&2
    echo "build first: cargo build --$([[ $PROFILE == release ]] && echo release || echo profile=dev) --bin wayflow" >&2
    exit 1
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$PLIST" "$APP/Contents/Info.plist"

# A tiny launcher exec'd by the bundle that calls `wayflow tray`.
cat > "$APP/Contents/MacOS/wayflow-tray-launcher" <<'EOF'
#!/usr/bin/env bash
exec "$(dirname "$0")/wayflow" tray
EOF
chmod +x "$APP/Contents/MacOS/wayflow-tray-launcher"

cp "$BIN" "$APP/Contents/MacOS/wayflow"

echo "built: $APP"
echo "run:   open $APP"
