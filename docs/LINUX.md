# SSD Sentinel on Linux

SSD Sentinel runs natively on Linux with a single pure-Rust desktop & CLI binary (`ssd-sentinel`), designed to work uniformly across distributions and desktop environments with zero WebKitGTK, GTK, Qt, OpenSSL, or `libdbus` shared-library runtime dependencies.

---

## Architecture Highlights

- **Native Desktop Shell (`eframe` + `egui` + `egui_plot`):**
  - Dynamic `winit` Wayland & X11 backend (`wayland-dlopen`, `x11-dl`, `wayland-csd-adwaita`).
  - Proper Wayland `app_id` (`com.ssdsentinel.desktop`) matching `.desktop` `StartupWMClass` and embedded 128×128 icon for GNOME dock/window grouping.
  - Native XDG Desktop Portal file pickers (`rfd` `xdg-portal`) and theme detection (`org.freedesktop.portal.Settings` `color-scheme`).
  - Automatic Turkish-capable system font loading (`DejaVuSans`, `NotoSans`, `LiberationSans`, `Cantarell`, `Ubuntu`) so `ğüşöçıİĞÜŞÖÇ` renders identically everywhere.
- **Zero-Root Hardware & SMART Telemetry:**
  - `/sys/block`, `/sys/class/nvme`, `/proc/self/mountinfo`, `/proc/diskstats`, and `statvfs` read directly with zero subprocess forks.
  - Passwordless UDisks2 D-Bus SMART collection via pure-Rust `zbus::blocking` (`org.freedesktop.UDisks2`).
- **On-Demand Polkit Privilege Helper (`--privileged-helper`):**
  - The GUI **never** launches as `root` (keeping Wayland sockets and XDG Portals intact).
  - Clicking **Elevate (Polkit)** spawns a single persistent session `pkexec ssd-sentinel --privileged-helper` executing strictly typed, allowlisted `HelperOp` JSON-RPC commands (smartctl queries, `FITRIM`, sysfs scheduler writes, `hdparm`, `systemctl` fstrim.timer, raw `O_DIRECT` sector reads, read-only fsck, HDD defrag).
- **High-Performance Direct I/O (`io_linux.rs`):**
  - Asynchronous `io_uring` submission/completion queues with 4K-aligned `O_DIRECT` buffers, falling back gracefully to `pread`/`pwrite` or `POSIX_FADV_DONTNEED` + `fdatasync` inside restricted containers.

---

## Desktop Environment Compatibility

| Environment | Windowing / CSD | File Picker | System Tray Guard | Status |
| :--- | :--- | :--- | :--- | :--- |
| **GNOME 50 (Pure Wayland / Mutter)** | Native `wayland-csd-adwaita` + `wp-fractional-scale-v1` | XDG Portal (`ashpd`) | **Vanilla GNOME 50 Tray Guard:** probes `org.kde.StatusNotifierWatcher`; if absent, Close exits cleanly instead of hiding into an unreachable tray | Verified design |
| **GNOME + AppIndicator extension** | Native Wayland CSD | XDG Portal | Pure-Rust `zbus` StatusNotifierItem tray active | Supported |
| **KDE Plasma 6 (Wayland / X11)** | Wayland/X11 CSD | KDE XDG Portal | Pure-Rust `zbus` StatusNotifierItem tray active | Supported |
| **Hyprland / Sway / Niri / Cosmic** | Native Wayland | `xdg-desktop-portal-{hyprland,wlr,cosmic}` | Works with Waybar/tray watcher or direct exit | Supported |
| **XFCE / Cinnamon / MATE / i3 (X11)** | X11 (`x11-dl`) | Zenity / XDG Portal | StatusNotifier / clean close | Supported |

> **Optional Tauri 2 build:** Contributors who specifically want the webview UI can build with `cargo build --release --features desktop` (or default `cargo build --release`). On Wayland + NVIDIA proprietary drivers, SSD Sentinel automatically applies `WEBKIT_DISABLE_DMABUF_RENDERER=1` before WebKitGTK initialization.

---

## CLI Usage (`ssd-sentinel --cli`)

```bash
# Inspect disks & SMART health as formatted text or JSON
ssd-sentinel --cli status
ssd-sentinel --cli status --json

# Replay fixtures without touching live disks
ssd-sentinel --cli status --snapshot-dir src-tauri/tests/fixtures --json

# Live /proc/diskstats throughput & queue depth snapshot
ssd-sentinel --cli perf --json

# Inspect full SMART table for a disk ID or device path
ssd-sentinel --cli smart <DISK_ID>

# Export redacted HTML or JSON report
ssd-sentinel --cli report ./report.html --redact
ssd-sentinel --cli report ./state.json --redact --json

# Analyze developer workspace build caches (read-only)
ssd-sentinel --cli dev ~/projects --json

# Analyze smart tiering offload candidates
ssd-sentinel --cli tier <DISK_ID> /home --json

# Run storage benchmark (requires explicit --confirm per safety invariant S1)
ssd-sentinel --cli bench /tmp --size-mib 256 --confirm
```

---

## Build & Universal Packaging

### Build Native Binary

```bash
cd src-tauri
# Native pure-Rust GUI + CLI (recommended on Linux)
cargo build --release --no-default-features --features native-gui

# Or headless CLI-only binary (zero GUI dependencies)
cargo build --release --no-default-features
```

Verify zero unwanted desktop/C shared library links:

```bash
ldd target/release/ssd-sentinel
```

### Packaging Scripts

| Target | Command | Output |
| :--- | :--- | :--- |
| User / System Install | `./packaging/linux/install.sh [--user \| --system]` | `~/.local/bin` or `/usr/local/bin` + icons + `.desktop` + Policy |
| Debian / Ubuntu / Mint | `./packaging/linux/build-deb.sh` | `dist/ssd-sentinel_<ver>_<arch>.deb` |
| Fedora / RHEL / Nobara | `./packaging/linux/build-rpm.sh` | `dist/ssd-sentinel-<ver>-1.*.rpm` |
| Arch / CachyOS / Omarchy | `cd packaging/linux && makepkg -si` | Pacman package |
| Portable AppImage | `./packaging/linux/build-appimage.sh` | `dist/SSD-Sentinel.AppDir` / `.AppImage` |
| Optional `udev` Rule | `sudo cp packaging/linux/99-ssd-sentinel.rules /etc/udev/rules.d/` | `disk` group raw health read without Polkit prompt |
