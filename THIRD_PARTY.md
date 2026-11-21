# Third-Party Notices & Licensing

SSD Sentinel's original Rust, TypeScript, CSS, HTML, and SVG source code is licensed under the **MIT License** ([LICENSE](LICENSE)).

---

## 1. Bundled External Tool: smartmontools 7.5 (`GPL-2.0-or-later`)

Disk health and SMART acquisition uses **smartmontools** (`smartctl.exe`) invoked as a separate external CLI process. `smartmontools` is **not** relicensed under MIT and remains governed by the **GNU General Public License v2 or later** (`tools/smartmontools/doc/COPYING.txt` in packaged builds).

- **Project Website:** https://www.smartmontools.org/
- **Release Tag (`7.5`):** https://github.com/smartmontools/smartmontools/releases/tag/RELEASE_7_5
- **Source Repository:** https://github.com/smartmontools/smartmontools/tree/RELEASE_7_5

### GPL Source Availability for Binary Releases

`tools/smartmontools/` binaries are excluded from the Git repository (`.gitignore`). Whenever official release binaries or NSIS installers bundling `smartctl.exe` are published via GitHub Actions (`.github/workflows/release.yml`), the corresponding upstream `smartmontools-7.5` source archive (`RELEASE_7_5.tar.gz`) and license files are attached directly alongside the installer release assets to fulfill GNU GPL source correspondence requirements.

---

## 2. Rust Dependencies (`src-tauri/Cargo.toml` / `Cargo.lock`)

Exact locked versions live in `src-tauri/Cargo.lock`:

| Crate | License | Role in SSD Sentinel |
|---|---|---|
| `tauri`, `tauri-build`, `tauri-plugin-dialog`, `tauri-plugin-notification`, `tauri-plugin-single-instance` | `MIT OR Apache-2.0` | Desktop shell, native WebView2 windowing, tray menu, dialogs, notifications, single-instance lock |
| `windows-sys` | `MIT OR Apache-2.0` | Win32 APIs: Direct-I/O IOCP benchmark, `FSCTL_MOVE_FILE` / storage IOCTLs, SetupAPI/CfgMgr32 PCIe queries, UAC elevation, WinHTTP explicit update check |
| `rusqlite` (bundled SQLite) | `MIT` (SQLite: Public Domain) | Local time-series telemetry, job history, rollback ledger, tier move ledger |
| `serde`, `serde_json`, `chrono`, `sha2`, `tempfile`, `getrandom`, `same-file`, `uuid`, `roxmltree` | `MIT` / `Apache-2.0` | Serialization, timestamps, SHA-256 verification, atomic temp files, XML/JSON parsing |

Windows UI rendering uses the OS-provided **Microsoft Edge WebView2** runtime rather than bundling Chromium binaries.

---

## 3. Frontend & Development Tooling (`package.json`)

- **Vite** (`MIT`): Development server and production bundle.
- **TypeScript** (`Apache-2.0`): Type checking.
- **Playwright** (`Apache-2.0`, dev-only): Headless end-to-end UI verification tests; not shipped in runtime bundles.
- **UI Assets:** Zero external fonts, icon fonts, CDN scripts, or third-party frontend UI frameworks. Uses native system font stacks and original inline SVG icons.
