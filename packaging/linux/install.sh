#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BIN_PATH="$REPO_ROOT/src-tauri/target/release/ssd-sentinel"
MODE="user"

for arg in "$@"; do
  case "$arg" in
    --user) MODE="user" ;;
    --system) MODE="system" ;;
    -h|--help)
      echo "Usage: $0 [--user | --system]"
      exit 0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      exit 2
      ;;
  esac
done

if [[ ! -x "$BIN_PATH" ]]; then
  echo "Release binary not found; building with cargo build --release --locked --no-default-features --features native-gui..."
  (cd "$REPO_ROOT/src-tauri" && cargo build --release --locked --no-default-features --features native-gui)
fi

if [[ "$MODE" == "system" ]]; then
  PREFIX="/usr/local"
  DATA_DIR="/usr/share"
  if [[ "$EUID" -ne 0 ]]; then
    echo "--system install requires root (run with sudo)." >&2
    exit 1
  fi
  install -Dm755 "$BIN_PATH" "$PREFIX/bin/ssd-sentinel"
  install -Dm644 "$SCRIPT_DIR/com.ssdsentinel.desktop.desktop" "$DATA_DIR/applications/com.ssdsentinel.desktop.desktop"
  install -Dm644 "$SCRIPT_DIR/com.ssdsentinel.desktop.metainfo.xml" "$DATA_DIR/metainfo/com.ssdsentinel.desktop.metainfo.xml"
  install -Dm644 "$SCRIPT_DIR/com.ssdsentinel.desktop.policy" "$DATA_DIR/polkit-1/actions/com.ssdsentinel.desktop.policy"
  install -Dm644 "$REPO_ROOT/src-tauri/icons/32x32.png" "$DATA_DIR/icons/hicolor/32x32/apps/com.ssdsentinel.desktop.png"
  install -Dm644 "$REPO_ROOT/src-tauri/icons/128x128.png" "$DATA_DIR/icons/hicolor/128x128/apps/com.ssdsentinel.desktop.png"
  install -Dm644 "$REPO_ROOT/src-tauri/icons/icon.png" "$DATA_DIR/icons/hicolor/256x256/apps/com.ssdsentinel.desktop.png"
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DATA_DIR/applications" || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f "$DATA_DIR/icons/hicolor" || true
  echo "Installed SSD Sentinel system-wide into $PREFIX/bin/ssd-sentinel"
else
  BIN_HOME="${HOME}/.local/bin"
  DATA_HOME="${XDG_DATA_HOME:-${HOME}/.local/share}"
  install -Dm755 "$BIN_PATH" "$BIN_HOME/ssd-sentinel"
  install -Dm644 "$SCRIPT_DIR/com.ssdsentinel.desktop.desktop" "$DATA_HOME/applications/com.ssdsentinel.desktop.desktop"
  install -Dm644 "$SCRIPT_DIR/com.ssdsentinel.desktop.metainfo.xml" "$DATA_HOME/metainfo/com.ssdsentinel.desktop.metainfo.xml"
  install -Dm644 "$REPO_ROOT/src-tauri/icons/32x32.png" "$DATA_HOME/icons/hicolor/32x32/apps/com.ssdsentinel.desktop.png"
  install -Dm644 "$REPO_ROOT/src-tauri/icons/128x128.png" "$DATA_HOME/icons/hicolor/128x128/apps/com.ssdsentinel.desktop.png"
  install -Dm644 "$REPO_ROOT/src-tauri/icons/icon.png" "$DATA_HOME/icons/hicolor/256x256/apps/com.ssdsentinel.desktop.png"
  command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$DATA_HOME/applications" || true
  command -v gtk-update-icon-cache >/dev/null 2>&1 && gtk-update-icon-cache -q -t -f "$DATA_HOME/icons/hicolor" || true
  echo "Installed SSD Sentinel for current user into $BIN_HOME/ssd-sentinel"
  echo "Optional Polkit policy for prompt descriptions: sudo cp \"$SCRIPT_DIR/com.ssdsentinel.desktop.policy\" /usr/share/polkit-1/actions/"
fi
