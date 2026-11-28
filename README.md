<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="116" height="116" alt="SSD Sentinel logo" />

# SSD Sentinel — Free & Open-Source SSD Health Monitor for Windows

**Free, open-source SSD & HDD health monitoring, SMART diagnostics, disk benchmarking, LBA surface scanning, and read-only NTFS file recovery for Windows 10 and Windows 11.**<br/>
Built from scratch in **Rust & Tauri 2** as a transparent, ad-free, offline-first alternative to **CrystalDiskInfo** and **Hard Disk Sentinel**.<br/>
<sub>SMART wear &amp; endurance forecasting · Win32 IOCP Direct-I/O benchmark · Deterministic LBA surface triage &amp; repair · Read-only NTFS forensic recovery · SHA-256 NTFS Junction tiering — all in a ~3.7 MB installer.</sub>

[![Latest Release](https://img.shields.io/github/v/release/yuefdev/ssd-sentinel?style=flat-square&color=4da6ff&label=Release)](https://github.com/yuefdev/ssd-sentinel/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/yuefdev/ssd-sentinel/total?style=flat-square&color=1b5ea8&label=Downloads)](https://github.com/yuefdev/ssd-sentinel/releases)
[![Windows CI](https://img.shields.io/github/actions/workflow/status/yuefdev/ssd-sentinel/ci.yml?branch=main&style=flat-square&label=Windows%20CI)](https://github.com/yuefdev/ssd-sentinel/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-37c97e?style=flat-square)](LICENSE)
[![Platform: Windows 10/11 x64](https://img.shields.io/badge/Windows-10%20%2F%2011%20(x64)-0c0f14?style=flat-square)](https://github.com/yuefdev/ssd-sentinel/releases/latest)
[![UI: EN / TR](https://img.shields.io/badge/UI%20%26%20Tray-EN%20%7C%20TR-4da6ff?style=flat-square)](#türkçe-özet)

### [Website](https://yuefdev.github.io/ssd-sentinel/) · [vs CrystalDiskInfo](https://yuefdev.github.io/ssd-sentinel/crystaldiskinfo-alternative/) · [Hard Disk Sentinel Alternative](https://yuefdev.github.io/ssd-sentinel/hard-disk-sentinel-alternative/) · [Architecture Spec](docs/ARCHITECTURE.md) · [Safety & Crash Spec](docs/SAFETY_SPEC.md) · [Download Installer (`.exe`)](https://github.com/yuefdev/ssd-sentinel/releases/latest) · [FAQ](https://yuefdev.github.io/ssd-sentinel/faq/) · [Türkçe Özet](#türkçe-özet)

</div>

[Code signing policy](CODE_SIGNING.md) · [Privacy Policy](https://yuefdev.github.io/ssd-sentinel/privacy/)

<br/>

<p align="center">
  <img src="docs/screenshots/overview-dark-en.png" alt="SSD Sentinel dark overview showing NVMe SMART health, Read-Only guard, telemetry, and endurance forecast" width="920" />
</p>

---

## Engineering Transparency & Clean-Room Architecture

Disk utilities carry real data-loss risk. **SSD Sentinel** is built from scratch in Rust and TypeScript against documented Windows Win32/NTFS kernel interfaces (`CreateFileW`, `FSCTL_MOVE_FILE`, `FSCTL_SET_REPARSE_POINT`, `IOCTL_DISK_*_CACHE_INFORMATION`, Win32 IOCP) and upstream `smartmontools` (`smartctl.exe`, GNU GPLv2+)—with **zero hidden telemetry**, **zero AI/ML models**, and full safety specifications open for review:

- **[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)** — Layered topology, WebView2 CSP & IPC trust boundaries, full module-by-module Read/Write matrix, verified Win32/IOCTL/FSCTL inventory, and SQLite 11-table persistence schema.
- **[`docs/SAFETY_SPEC.md`](docs/SAFETY_SPEC.md)** — Enforceable invariants (S1–S10), Default Read-Only Guard, Dry-Run execution manifests, Crash-Consistency & Power-Loss matrix, 25-entry system path firewall, and supply-chain verification rules.

| Capability | Safety & Engineering Design |
|---|---|
| **Default Read-Only Guard** | Fresh installs boot with `read_only_guard = true` enforced in Rust ([`core.rs`](src-tauri/src/core.rs)). All mutating jobs, IOCTLs, and tier moves are blocked at the core boundary until explicitly unlocked in the toolbar or Settings. |
| **Dry-Run Execution Manifest** | Every mutating confirmation modal displays a structured **Dry-Run Preview** listing target volume, Win32/CLI APIs invoked, write budget, rollback path, and the deterministic IPC token checked in Rust. |
| **SMART & Forecasting** | Live NVMe, SATA SSD & HDD SMART via `smartctl`, **per-disk fault isolation** (`read_incomplete`), Default/Strict modes, 720-sample SQLite write-rate & years-left projections. |
| **Raw Disk Benchmark** | Built-in **Win32 IOCP Direct-I/O** (`FILE_FLAG_NO_BUFFERING \| OVERLAPPED`), SEQ/RND 4K MB/s, IOPS & µs latency with PCIe/SATA link bottleneck detection. |
| **Surface Scan & Pinpoint Repair** | **Read-only raw LBA 200-zone map**, thermal (`53 °C`) & SMART cascade (`+8` reallocated) watchdogs, deterministic statistical triage ([`surface_triage.rs`](src-tauri/src/surface_triage.rs)), resumable checkpoints, `FSCTL_MOVE_FILE` relocation & `.ssd-sentinel-quarantine`. |
| **Smart Storage Tiering** | Blocks classic SSD defrag; identifies cold SSD data & hot HDD folders (respecting 25 protected system paths & 10 GiB reserve), relocates with **SHA-256 verification + native `FSCTL_SET_REPARSE_POINT` NTFS Junctions** and one-click rollback. |
| **Developer & WSL2 Storage** | Read-only radar for rebuildable project artifacts (`node_modules`, `target`, `.venv` — `.git`/`.env*` untouched), local LLM/package cache offloading via junctions, **WSL2/Docker `.vhdx` compaction**, and Win32 Long Paths / NTFS 8.3 toggles. |
| **Forensic Recovery** | Strictly **read-only** `$MFT`, `$UsnJrnl`, `$I30` slack, VSS Shadow Copies, Recycle Bin & carving, **Deleted Apps & Games reconstruction**, plus JPEG/PNG/MP4 buffer header repair. |

---

## Visual Tour

<table align="center">
  <tr>
    <td width="50%" align="center">
      <img src="docs/screenshots/overview-en.png" alt="SSD Sentinel English light theme overview and endurance forecast" /><br/>
      <sub><b>Precision Storage Lab UI &amp; EN/TR Localization</b> — Light &amp; dark themes, tabular monospace telemetry, and endurance forecast.</sub>
    </td>
    <td width="50%" align="center">
      <img src="docs/screenshots/optimize-tiering.png" alt="Storage policies and SHA-256 NTFS tiering" /><br/>
      <sub><b>Policy Profiles &amp; SHA-256 NTFS Tiering</b> — Hardware cache, TRIM, PCIe ASPM, Over-Provisioning &amp; Junction offloading.</sub>
    </td>
  </tr>
  <tr>
    <td width="50%" align="center">
      <img src="docs/screenshots/surface-diagnosis.png" alt="200-Zone Surface Map and Deterministic Surface Diagnosis" /><br/>
      <sub><b>200-Zone LBA Surface &amp; Pinpoint Repair</b> — Deterministic adaptive threshold triage, ZBR speed curve &amp; live task bar.</sub>
    </td>
    <td width="50%" align="center">
      <img src="docs/screenshots/recovery-center.png" alt="File recovery sample with screenshots and project files first" /><br/>
      <sub><b>File Recovery</b> — Search by name, folder, or extension. Keep selections across pages. Recognized screenshots and editable projects come first. <a href="CHANGELOG.md#unreleased">Changes on main</a></sub>
    </td>
  </tr>
</table>

---

## CI/CD, Staging & Verifiable Releases

Every pull request and release runs through automated Windows verification in [`.github/workflows/ci.yml`](.github/workflows/ci.yml) and [`.github/workflows/release.yml`](.github/workflows/release.yml):

1. **Automated Unit, Lint & Mock-IPC UI Smoke:** TypeScript unit tests (`npm test`), Vite type-check build, `rustfmt`, `clippy -D warnings`, Rust core (`--no-default-features`) + desktop tests, and Playwright UI verification across TR/EN and 390px viewports (`tools/smoke_tauri.mjs`).
2. **Real NTFS VHDX Integration Tests (`tools/vhdx_integration_test.ps1`):** Ephemeral 128 MB NTFS `.vhdx` virtual disks are attached on `windows-latest` CI runners to test the 10 GiB free-space reserve guard, real Win32 `FSCTL_SET_REPARSE_POINT` cross-volume NTFS Directory Junctions, mid-copy fault-injection cancellation, and SHA-256 rollback.
3. **Supply-Chain Audits:** `npm audit --omit=dev` and `cargo audit` run against lockfiles on CI.
4. **Staging & Release Channels:**
   - **Staging Dry-Runs:** `workflow_dispatch` builds upload staged NSIS artifacts + `SHA256SUMS.txt` without publishing publicly.
   - **Release Candidates (`v*-rc.*`, `v*-beta.*`):** Published as GitHub Pre-Releases for staged testing.
   - **Production Releases (`vX.Y.Z`):** Published with `SHA256SUMS.txt` and SLSA GitHub Build Provenance Attestations (`actions/attest-build-provenance@v2`).

Verify any downloaded release binary against GitHub Build Provenance and checksums:

```powershell
gh attestation verify "SSD-Sentinel_<version>_x64-setup.exe" --owner yuefdev
```

> **SmartScreen Notice:** Community installers are currently open-source unsigned builds (no commercial Authenticode certificate). Verify `SHA256SUMS.txt` or `gh attestation verify`, or inspect/build directly from source below.

See the [Code signing policy](CODE_SIGNING.md) for signing status, scope, and approval roles.

---

## Install or Build from Source

### Option 1 — Quick Install (Recommended)
Download the latest installer from [GitHub Releases](https://github.com/yuefdev/ssd-sentinel/releases/latest) and verify its checksum against `SHA256SUMS.txt` (or `gh attestation verify`). WebView2 uses Microsoft's lightweight bootstrapper if not already present on Windows 10/11.

### Option 1b — Portable Edition (`.zip`, no installer)
Download `SSD-Sentinel_<version>_x64-portable.zip` from [GitHub Releases](https://github.com/yuefdev/ssd-sentinel/releases/latest), extract it anywhere (USB stick, `D:\Tools`, Desktop) and run `ssd-sentinel.exe`. Nothing is installed and **nothing is written outside the folder**:

```text
SSD-Sentinel/
├─ ssd-sentinel.exe        the app (portable.txt next to it switches portable mode on)
├─ portable.txt            marker file - delete it to use %LOCALAPPDATA% again
├─ smartmontools/          bundled smartctl (GPL-2.0-or-later)
└─ data/                   created on first run
   ├─ history.sqlite3      SMART history, jobs, rollback records
   ├─ preferences.json     language, Read-Only Guard, update settings
   ├─ webview2/            WebView2 profile (cache, local storage)
   └─ updates/             verified update packages
```

Portable mode is also enabled with `--portable` or `SSD_SENTINEL_PORTABLE=1`. In portable mode the in-app updater downloads the matching `-portable.zip` into `data\updates`, verifies its SHA-256 against `SHA256SUMS.txt` and reveals it in Explorer - extract it over the old folder and `data` is kept. Build it yourself with `npm run desktop:portable` (see [`tools/package_portable.ps1`](tools/package_portable.ps1)); verify a download with `gh attestation verify "SSD-Sentinel_<version>_x64-portable.zip" --owner yuefdev`.

### Option 2 — Try Without Touching Disks (Snapshot Replay Mode)
Inspect the UI and test sample HDD + NVMe telemetry safely with **zero physical disk access** using anonymized repo fixtures:

```powershell
.\ssd-sentinel.exe --snapshot-dir .\src-tauri\tests\fixtures
```

### Option 3 — Build & Verify Locally

**Prerequisites:** Windows 10/11 (`x64`), stable Rust toolchain (`cargo`, `rustfmt`, `clippy`), Node.js `>= 22`, and `smartmontools 7.5` (`smartctl.exe`, `drivedb.h`, `COPYING.txt`, `checksums64.txt`) placed under `tools/smartmontools/` (excluded from Git; see [THIRD_PARTY.md](THIRD_PARTY.md)).

```powershell
npm ci
npm test
npm run build

cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -j 1 -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features -j 1
cargo test --manifest-path src-tauri/Cargo.toml -j 1

npm run tauri -- build --bundles nsis
```

*(Elevated Administrators can also execute the real NTFS VHDX integration suite locally via `pwsh -NoProfile -File tools/vhdx_integration_test.ps1`.)*

---

## Contribute & Help Shape SSD Sentinel

- Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md), [`docs/SAFETY_SPEC.md`](docs/SAFETY_SPEC.md), and [`CONTRIBUTING.md`](CONTRIBUTING.md)
- Browse or open [Bug Reports & Feature Requests](https://github.com/yuefdev/ssd-sentinel/issues)
- Review our [Security Policy](SECURITY.md) & [Changelog](CHANGELOG.md)

---

## Why SSD Sentinel? (Quick Comparison)

Looking for a **free, open-source alternative to CrystalDiskInfo or Hard Disk Sentinel** on Windows 10/11? **SSD Sentinel** unifies live SMART monitoring, endurance forecasting, raw Win32 IOCP benchmarking, LBA surface diagnostics, and read-only NTFS recovery behind a hardware-protective **Default Read-Only Guard**.

| Capability | SSD Sentinel (`v1.11.0`) | CrystalDiskInfo | Hard Disk Sentinel |
|---|---|---|---|
| **License & Cost** | **MIT** (Free & Open Source; `smartctl` GPLv2+) | **MIT** (Freeware / Open Source) | Commercial (Limited Free Trial; Paid Standard/Pro) |
| **Ads & Hidden Telemetry** | **Zero ads, zero telemetry, offline-first** | Clean open-source (themed editions available) | No telemetry |
| **NVMe, SATA SSD & HDD SMART** | Live via `smartctl` + per-disk fault isolation | Broad SMART & legacy USB bridge support | Deep SMART + RAID/Enterprise controller support |
| **Endurance & Lifespan Forecast** | **720-sample local SQLite write-rate & years-left projection** | Basic remaining health % | Wear & estimated lifetime projections |
| **Built-in Disk Benchmark** | **Win32 IOCP Direct-I/O** (`SEQ`, `RND 4K`, IOPS, µs, PCIe/SATA link check) | Separate app (CrystalDiskMark) | Included in paid Pro editions |
| **Surface Scan & Bad Sectors** | **200-zone raw LBA read-only scan**, `53 °C` thermal & `+8` cascade watchdogs, `FSCTL_MOVE_FILE` repair | Not included | Surface tests & reinitialization (Pro) |
| **NTFS File Recovery** | **Read-only `$MFT`, `$UsnJrnl`, `$I30`, VSS, carving & media repair** | Not included | Not included |
| **Default Write Protection** | **Read-Only Guard (`true` on boot)** + Dry-Run IPC tokens | Read-only SMART viewer | Configuration-driven |
| **Code Signing & Maturity** | Unsigned community builds (`SHA256SUMS.txt` + SLSA attestation) | Long-established & Authenticode-signed | Long-established & Authenticode-signed |

Full breakdowns: [SSD Sentinel vs CrystalDiskInfo](https://yuefdev.github.io/ssd-sentinel/crystaldiskinfo-alternative/) · [Free Hard Disk Sentinel Alternative](https://yuefdev.github.io/ssd-sentinel/hard-disk-sentinel-alternative/)

---

## Frequently Asked Questions

**Is SSD Sentinel completely free and open-source?**
Yes. SSD Sentinel's Rust, TypeScript, CSS, and PowerShell source code is MIT-licensed with zero paid Pro tiers, ads, or telemetry. Windows builds bundle upstream `smartmontools 7.5` (`smartctl.exe`, GNU GPL-2.0-or-later) alongside its matching source archive on GitHub Releases.

**How do I check NVMe SSD health and lifespan on Windows 11?**
Install SSD Sentinel, leave the **Write Guard** locked for pure read-only inspection, and check the Drive Overview for SMART health %, `Percentage Used`, `Available Spare`, temperature, Data Units Written (TBW), and the SQLite endurance projection. See our full [NVMe SSD Lifespan Guide](https://yuefdev.github.io/ssd-sentinel/nvme-lifespan/) and [Windows SSD Health Check Guide](https://yuefdev.github.io/ssd-sentinel/ssd-health-check/).

**Is SSD Sentinel safe to run on my OS drive?**
Yes. Every fresh install boots with `read_only_guard = true` enforced in Rust (`src-tauri/src/core.rs`), blocking benchmarks, repairs, tiering, and system policy writes until you explicitly unlock the guard and approve a structured Dry-Run execution manifest. You can also test the full interface with zero physical disk access using `--snapshot-dir .\src-tauri\tests\fixtures`.

**Why does Windows SmartScreen warn on the installer?**
Community installers do not currently use a paid commercial Authenticode certificate. Always verify the downloaded `.exe` against `SHA256SUMS.txt` or `gh attestation verify "SSD-Sentinel_1.11.0_x64-setup.exe" --owner yuefdev` before running, or read the full [FAQ](https://yuefdev.github.io/ssd-sentinel/faq/).

---

## Türkçe Özet

**SSD Sentinel**, Windows sistemler için **Rust + Tauri 2** ile sıfırdan (clean-room mühendislik ve belgelenmiş Win32/NTFS arayüzleriyle) geliştirilmiş; tamamen yerel çalışan, reklamsız, telemetrisiz ve anlık **TR / EN** dil destekli açık kaynak disk sağlığı ve bakım paketidir.

- **Dosya kurtarma.** Ad, klasör veya uzantıyla ara. Seçimler sayfalar arasında korunur. Tanınan ekran görüntüleri ve düzenlenebilir projeler üstte görünür.
- **Varsayılan Salt-Okunur Kalkan (`read_only_guard`):** Uygulama ilk açılışta yazma kilidi etkin başlar; hız testi, yüzey onarımı, optimize ve NTFS katmanlama işlemleri Rust çekirdeğinde engellenir. Kilidi açtığınızda bile her yazma işlemi öncesi hedef, Win32/CLI API'ler, yazma bütçesi ve geri alma yolunu gösteren **Dry-Run Etki Önizlemesi** sunulur.
- **Şeffaf Mimari & Güvenlik Şartnamesi:** Tüm modül sınırları [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) ve çökme/elektrik kesintisi senaryoları [`docs/SAFETY_SPEC.md`](docs/SAFETY_SPEC.md) içinde belgelenmiştir.
- **Gerçek NTFS VHDX Doğrulaması & Tedarik Zinciri:** Her PR'da `cargo audit` / `npm audit` ile birlikte sanal NTFS VHDX diskler üzerinde gerçek `FSCTL_SET_REPARSE_POINT` junction ve SHA-256 rollback testleri koşar; sürümler `SHA256SUMS.txt` ve SLSA Build Provenance kanıtıyla yayımlanır.

---

## Licensing

Original Rust, TypeScript, CSS, SVG, and UI source code in **SSD Sentinel** is licensed under the [MIT License](LICENSE).

Windows distributions bundle upstream **smartmontools 7.5** (`smartctl.exe`, GNU **GPL-2.0-or-later**) in a separate subdirectory alongside matching upstream source archives (`RELEASE_7_5.tar.gz`) on GitHub Releases — see [THIRD_PARTY.md](THIRD_PARTY.md) for full dependency notices.
