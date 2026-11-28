#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VERSION="$(grep '^version' "$REPO_ROOT/src-tauri/Cargo.toml" | head -n1 | sed -E 's/.*"([^"]+)".*/\1/')"
ARCH="$(uname -m)"
BIN_PATH="$REPO_ROOT/src-tauri/target/release/ssd-sentinel"

if [[ ! -x "$BIN_PATH" ]]; then
  (cd "$REPO_ROOT/src-tauri" && cargo build --release --locked --no-default-features --features native-gui)
fi

OUT_DIR="$REPO_ROOT/dist"
mkdir -p "$OUT_DIR"
APPDIR="$OUT_DIR/SSD-Sentinel.AppDir"
rm -rf "$APPDIR"
mkdir -p \
  "$APPDIR/usr/bin" \
  "$APPDIR/usr/share/applications" \
  "$APPDIR/usr/share/metainfo" \
  "$APPDIR/usr/share/icons/hicolor/128x128/apps" \
  "$APPDIR/usr/share/icons/hicolor/256x256/apps"

install -m755 "$BIN_PATH" "$APPDIR/usr/bin/ssd-sentinel"
install -m644 "$SCRIPT_DIR/com.ssdsentinel.desktop.desktop" "$APPDIR/com.ssdsentinel.desktop.desktop"
install -m644 "$SCRIPT_DIR/com.ssdsentinel.desktop.desktop" "$APPDIR/usr/share/applications/com.ssdsentinel.desktop.desktop"
install -m644 "$SCRIPT_DIR/com.ssdsentinel.desktop.metainfo.xml" "$APPDIR/usr/share/metainfo/com.ssdsentinel.desktop.metainfo.xml"
install -m644 "$REPO_ROOT/src-tauri/icons/128x128.png" "$APPDIR/com.ssdsentinel.desktop.png"
install -m644 "$REPO_ROOT/src-tauri/icons/128x128.png" "$APPDIR/usr/share/icons/hicolor/128x128/apps/com.ssdsentinel.desktop.png"
install -m644 "$REPO_ROOT/src-tauri/icons/icon.png" "$APPDIR/usr/share/icons/hicolor/256x256/apps/com.ssdsentinel.desktop.png"

cat >"$APPDIR/AppRun" <<'EOF'
#!/usr/bin/env bash
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/ssd-sentinel" "$@"
EOF
chmod +x "$APPDIR/AppRun"

if command -v appimagetool >/dev/null 2>&1; then
  ARCH="$ARCH" appimagetool "$APPDIR" "$OUT_DIR/SSD-Sentinel-${VERSION}-${ARCH}.AppImage"
  echo "Built AppImage: $OUT_DIR/SSD-Sentinel-${VERSION}-${ARCH}.AppImage"
else
  echo "Staged AppDir at $APPDIR (appimagetool not found; install appimagetool to pack .AppImage)"
fi
