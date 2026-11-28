#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VERSION="$(grep '^version' "$REPO_ROOT/src-tauri/Cargo.toml" | head -n1 | sed -E 's/.*"([^"]+)".*/\1/')"
BIN_PATH="$REPO_ROOT/src-tauri/target/release/ssd-sentinel"

if [[ ! -x "$BIN_PATH" ]]; then
  (cd "$REPO_ROOT/src-tauri" && cargo build --release --locked --no-default-features --features native-gui)
fi

if ! command -v rpmbuild >/dev/null 2>&1; then
  echo "rpmbuild not installed on this host (install rpm / rpm-build)." >&2
  exit 1
fi

TOPDIR="$(mktemp -d /tmp/ssd-sentinel-rpm.XXXXXX)"
trap 'rm -rf "$TOPDIR"' EXIT
mkdir -p "$TOPDIR"/{BUILD,RPMS,SOURCES,SPECS,SRPMS}

cp "$BIN_PATH" "$TOPDIR/SOURCES/ssd-sentinel"
cp "$SCRIPT_DIR/com.ssdsentinel.desktop.desktop" "$TOPDIR/SOURCES/"
cp "$SCRIPT_DIR/com.ssdsentinel.desktop.metainfo.xml" "$TOPDIR/SOURCES/"
cp "$SCRIPT_DIR/com.ssdsentinel.desktop.policy" "$TOPDIR/SOURCES/"
cp "$REPO_ROOT/src-tauri/icons/32x32.png" "$TOPDIR/SOURCES/icon-32.png"
cp "$REPO_ROOT/src-tauri/icons/128x128.png" "$TOPDIR/SOURCES/icon-128.png"
cp "$REPO_ROOT/src-tauri/icons/icon.png" "$TOPDIR/SOURCES/icon-256.png"
sed "s/^Version:.*/Version:        ${VERSION}/" "$SCRIPT_DIR/ssd-sentinel.spec" >"$TOPDIR/SPECS/ssd-sentinel.spec"

rpmbuild --define "_topdir $TOPDIR" -bb "$TOPDIR/SPECS/ssd-sentinel.spec"
mkdir -p "$REPO_ROOT/dist"
find "$TOPDIR/RPMS" -name '*.rpm' -exec cp -v {} "$REPO_ROOT/dist/" \;
