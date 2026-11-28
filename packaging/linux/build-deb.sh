#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VERSION="$(grep '^version' "$REPO_ROOT/src-tauri/Cargo.toml" | head -n1 | sed -E 's/.*"([^"]+)".*/\1/')"
ARCH="$(dpkg --print-architecture 2>/dev/null || uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"
BIN_PATH="$REPO_ROOT/src-tauri/target/release/ssd-sentinel"

if [[ ! -x "$BIN_PATH" ]]; then
  (cd "$REPO_ROOT/src-tauri" && cargo build --release --locked --no-default-features --features native-gui)
fi

STAGE="$(mktemp -d /tmp/ssd-sentinel-deb.XXXXXX)"
trap 'rm -rf "$STAGE"' EXIT

PKG_DIR="$STAGE/ssd-sentinel_${VERSION}_${ARCH}"
mkdir -p \
  "$PKG_DIR/DEBIAN" \
  "$PKG_DIR/usr/bin" \
  "$PKG_DIR/usr/share/applications" \
  "$PKG_DIR/usr/share/metainfo" \
  "$PKG_DIR/usr/share/polkit-1/actions" \
  "$PKG_DIR/usr/share/icons/hicolor/32x32/apps" \
  "$PKG_DIR/usr/share/icons/hicolor/128x128/apps" \
  "$PKG_DIR/usr/share/icons/hicolor/256x256/apps"

install -m755 "$BIN_PATH" "$PKG_DIR/usr/bin/ssd-sentinel"
install -m644 "$SCRIPT_DIR/com.ssdsentinel.desktop.desktop" "$PKG_DIR/usr/share/applications/com.ssdsentinel.desktop.desktop"
install -m644 "$SCRIPT_DIR/com.ssdsentinel.desktop.metainfo.xml" "$PKG_DIR/usr/share/metainfo/com.ssdsentinel.desktop.metainfo.xml"
install -m644 "$SCRIPT_DIR/com.ssdsentinel.desktop.policy" "$PKG_DIR/usr/share/polkit-1/actions/com.ssdsentinel.desktop.policy"
install -m644 "$REPO_ROOT/src-tauri/icons/32x32.png" "$PKG_DIR/usr/share/icons/hicolor/32x32/apps/com.ssdsentinel.desktop.png"
install -m644 "$REPO_ROOT/src-tauri/icons/128x128.png" "$PKG_DIR/usr/share/icons/hicolor/128x128/apps/com.ssdsentinel.desktop.png"
install -m644 "$REPO_ROOT/src-tauri/icons/icon.png" "$PKG_DIR/usr/share/icons/hicolor/256x256/apps/com.ssdsentinel.desktop.png"

cat >"$PKG_DIR/DEBIAN/control" <<EOF
Package: ssd-sentinel
Version: ${VERSION}
Section: utils
Priority: optional
Architecture: ${ARCH}
Maintainer: SSD Sentinel Contributors <https://github.com/yuefdev/ssd-sentinel>
Depends: libc6, libgcc-s1
Recommends: smartmontools, policykit-1 | polkitd, udisks2, libegl1 | libgl1
Description: Storage health diagnostics, io_uring benchmark, surface repair, and file recovery
 Pure native Rust GUI (Wayland/X11) and CLI suite for NVMe/SATA/HDD telemetry,
 smart tiering, developer workspace hygiene, and read-only file recovery.
EOF

OUT_DIR="$REPO_ROOT/dist"
mkdir -p "$OUT_DIR"
OUT_DEB="$OUT_DIR/ssd-sentinel_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "$PKG_DIR" "$OUT_DEB"
echo "Built Debian package: $OUT_DEB"
