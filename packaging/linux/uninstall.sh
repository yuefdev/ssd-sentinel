#!/usr/bin/env bash
set -euo pipefail

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

if [[ "$MODE" == "system" ]]; then
  if [[ "$EUID" -ne 0 ]]; then
    echo "--system uninstall requires root (run with sudo)." >&2
    exit 1
  fi
  rm -f /usr/local/bin/ssd-sentinel \
    /usr/share/applications/com.ssdsentinel.desktop.desktop \
    /usr/share/metainfo/com.ssdsentinel.desktop.metainfo.xml \
    /usr/share/polkit-1/actions/com.ssdsentinel.desktop.policy \
    /usr/share/icons/hicolor/32x32/apps/com.ssdsentinel.desktop.png \
    /usr/share/icons/hicolor/128x128/apps/com.ssdsentinel.desktop.png \
    /usr/share/icons/hicolor/256x256/apps/com.ssdsentinel.desktop.png
  echo "Uninstalled system SSD Sentinel files (user data left intact)."
else
  DATA_HOME="${XDG_DATA_HOME:-${HOME}/.local/share}"
  rm -f "${HOME}/.local/bin/ssd-sentinel" \
    "$DATA_HOME/applications/com.ssdsentinel.desktop.desktop" \
    "$DATA_HOME/metainfo/com.ssdsentinel.desktop.metainfo.xml" \
    "$DATA_HOME/icons/hicolor/32x32/apps/com.ssdsentinel.desktop.png" \
    "$DATA_HOME/icons/hicolor/128x128/apps/com.ssdsentinel.desktop.png" \
    "$DATA_HOME/icons/hicolor/256x256/apps/com.ssdsentinel.desktop.png"
  echo "Uninstalled user SSD Sentinel binaries/shortcuts (data in $DATA_HOME/ssd-sentinel left intact)."
fi
