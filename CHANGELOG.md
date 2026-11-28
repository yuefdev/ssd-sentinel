# Changelog

All notable changes to **SSD Sentinel** are documented in this file.

## Unreleased

- File recovery results are paginated. Search by name, folder, or extension and keep selections across pages.
- Screenshots and editable project files sort ahead of caches and build output. Both have their own filters.

## [v1.11.0] — 2026-11-28

### Added
- **Windows x64 Portable `.zip` Edition:** Shipped `SSD-Sentinel_<version>_x64-portable.zip` (extract anywhere and run `ssd-sentinel.exe`). When `portable.txt` / `portable`, `--portable`, or `SSD_SENTINEL_PORTABLE=1` is present, `desktop::resolve_storage_paths` isolates SQLite (`history.sqlite3`), `preferences.json`, WebView2 (`webview2\`), and downloaded updates (`updates\`) strictly inside `<exe_dir>\data\` with zero `%LOCALAPPDATA%` writes (`tauri.conf.json` defers window creation to `.setup()`). Added Settings **Storage mode** indicator, SHA-256 verified portable `.zip` update flow, `tools/package_portable.ps1` (`npm run desktop:portable`), and CI release packaging + SLSA provenance attestation.
- **Anti-Regression i18n Guardrails:** Added `npm run check:i18n` (`tools/check_i18n.mjs`) and `npm run gen:i18n` (`tools/gen_backend_i18n.mjs`), `tests/i18n_backend.test.ts`, and a full-DOM Playwright **English purity sweep** across all 8 views (`tools/smoke_tauri.mjs`), wired into CI and Release workflows.

### Fixed
- **Complete English Localization:** Localized Rust job progress phases (`phase_bi`), Autopilot Safe Profile steps/scope & HDD/SSD advisories, pinpoint surface repair results/advice/scope, optimization messages/errors, tray notifications (`format_event_notification`), HTML reports (`report.rs`), `space-*` free-space alerts, deleted app categories/sources, `ReadFile` dry-run label, and added UI-boundary translation (`src/i18n_backend.ts`) for previously saved SQLite history/optimizations and forensic/media-repair output.

## [v1.10.3] — 2026-11-27

### Fixed
- **Administrator-mode drive detection ("No drives detected"):** Added `parse_scan_devices` to safely skip elevated controller `open_error` entries and missing `type` hints, built a `--scan-open` → `--scan` → Win32 `/dev/pd{N}` fallback chain with `-d`-less auto-detect retry in `probe_smart_target`, and introduced live-only `deduplicate_live_aliases` (`core.rs`) to merge Windows `/dev/csmi*`, `/dev/nvme*`, `/dev/pd*`, and `/dev/sd*` aliases of the same physical drive (preferring complete non-CSMI reads) while keeping strict replay and real identity-conflict rejections intact.
- **Admin/UAC `smartctl.exe` path normalization:** Added `strip_verbatim_prefix` (`\\?\` stripping) and an executable-sibling `smartmontools/bin/smartctl.exe` fallback in `desktop.rs`.

## [v1.10.2] — 2026-11-27

### Fixed
- **Reliable Windows disk & volume mapping:** Added Win32 `StorageDeviceIdProperty` (VPD Page 0x83 EUI-64/WWN/NGUID/SCSI Name String) and NVMe `Identify Controller` / `Identify Namespace` (`StorageAdapterProtocolSpecificProperty` / `StorageDeviceProtocolSpecificProperty`) queries alongside `\\.\PhysicalDrive0..31` fallback discovery, `DRIVE_REMOVABLE` USB SSD support, and same-disk multi-extent handling.
- **3-tier deterministic S3 physical matching:** Added normalized model matching (stripping `"NVMe "`/`"ATA "`/`"SCSI "` prefixes, device suffixes, and 16-char SCSI truncation), SATA hex-encoded and 16-bit byte-swapped serial decoding, `nvme_nguid` + `wwn` support, and a non-contradictory unique model + exact byte-capacity + bus fallback while still rejecting collisions and duplicate identities.
- **ReTrim alignment, serial redaction & granular diagnostics:** Aligned `ReTrim` validation (`os_model`, `-ExpectedDiskNumber`, normalized matching), redacted `nvme_serial` in diagnostic exports, and surfaced distinct messages for missing identity, matched disks without drive letters, non-admin access, and RAID/VMD collisions.

### Added
- **Startup auto-update check & banner:** Live sessions check GitHub Releases once on launch (disabled in snapshot replay, toggleable via Settings → `auto_check_updates`, default `true`) and display a dismissible top banner with one-click SHA-256 verified install.

## [v1.10.1] — 2026-11-26

### Fixed
- **NSIS installer language auto-detection:** Reordered `bundle.windows.nsis.languages` to `["English", "Turkish"]` (with `displayLanguageSelector: false`) so Turkish Windows auto-selects Turkish while English and all other Windows locales automatically use English installer buttons and dialogs instead of falling back to Turkish.
- **OS UI language auto-detection in Rust & Tray:** Added Win32 `GetUserDefaultUILanguage()` detection (`platform::detect_system_language`) so fresh installs without `preferences.json` match the Windows UI language across backend state, system tray menus, startup tooltip, tray notifications, and HTML report save dialogs.
- **Frontend locale detection & sync:** Defaulted `<html lang="en">` and `detectInitialLang()` fallback to English, extracted `resolveLangFromLocale`, made initial backend/frontend language sync symmetric, and added unit + Playwright `de-DE` auto-boot-to-English verification.

## [v1.10.0] — 2026-11-23

### Added
- **Default-on Read-Only Guard:** `read_only_guard` is enforced at the Rust core boundary for every mutating path (`start_job`, `disk_action`, tier relocate/rollback, `clean_dev_artifact`, `optimize`), persisted in `preferences.json`, and exposed through a toolbar + Settings **Write Guard** toggle. Every confirmation dialog now shows a structured `DryRunSpec` execution manifest before anything is written.
- **Developer Workspace & WSL2/Docker Storage Tools (`dev_workspace.rs`):** Read-only project artifact radar (`node_modules`, `target`, `.venv`, etc. paired with manifests; `.git`/`.env*` untouched), allowlisted package & local LLM model cache discovery (AI models offloadable via SHA-256 NTFS junctions, never deletable), WSL2/Docker `.vhdx` compaction (`wsl --shutdown` + `diskpart compact vdisk`), Win32 Long Paths (`LongPathsEnabled`) and NTFS 8.3 short-name controls with SQLite rollback, plus developer-focused recovery filtering.
- **NTFS safety test suite:** `ntfs_safety_integration` (fault-injected mid-copy cancel, forbidden-path firewall, read-only scanner immutability, read-only guard persistence, dev workspace archive roundtrip) plus a real cross-volume VHDX NTFS junction/rollback harness (`tools/vhdx_integration_test.ps1`).
- **Formal docs:** `docs/ARCHITECTURE.md` and `docs/SAFETY_SPEC.md`; updated `SECURITY.md`, `README.md`, `CONTRIBUTING.md`.
- **CI/CD & supply chain:** Playwright UI smoke gate, VHDX NTFS CI job, `cargo-audit` + `npm audit`, `src-tauri/deny.toml`, RC/beta/alpha pre-release channel and manual staging artifacts, SLSA build provenance attestation on tagged releases.

### Changed
- Renamed the `surface_ai` module to `surface_triage`; the diagnosis engine is a deterministic statistics + rules engine (offline) and is labelled as such.
- **Precision Storage Lab UI:** steel-cobalt interactive accent separated from emerald health status, monospace tabular telemetry, flatter bordered panels, new shield mark, refreshed screenshots; removed consumer/AI copy and sparkle icons.

### Security
- Updated `rusqlite` to 0.43 (`libsqlite3-sys` 0.40, RUSTSEC-2026-0459), `tokio` to 1.54.1 (RUSTSEC-2026-0384), `url` to 2.5.9 (RUSTSEC-2026-0592), and `rustix` to 1.1.6.

## [v1.9.0] — 2026-11-21

### Added
- **Bilingual TR / EN Interface & Tray:** Instant `TR | EN` switcher in the sidebar and Settings (`#language-setting`) persisted across `localStorage`, `preferences.json`, and Windows system tray menus/tooltips.
- **Per-Disk SMART Fault Isolation (`read_incomplete`):** Failing or unprivileged ATA/SATA SMART reads stay isolated in a non-blocking `.warning-banner` (`partial_warnings`) while healthy NVMe/SATA drives remain live (`stale: false`).
- **In-App GitHub Updater (`yuefdev/ssd-sentinel`):** Queries GitHub Releases (`check_updates`), validates download hosts against strict GitHub domains, downloads `-setup.exe`, verifies against published `SHA256SUMS.txt` before launch (`download_and_launch_update`), and opens official release notes (`open_release_url`).
- **Minimalist Desktop Icon & Matching Brand Mark:** Custom geometric obsidian + emerald squircle app icon (`icon.ico`, `icon.png`, StoreLogos, `assets/sentinel.svg`).
- **Chunked Tiering Cancellation & Cleanup Warnings:** Real-time progress reporting (`CopyProgress`), `AtomicBool` cancellation between chunks, direct non-blocked Stop button (`cancel_job`), and non-fatal `cleanup_warning` surfacing after junction/restore commits.
- **Launch & CI Infrastructure:** Automated `ci.yml` and tag-triggered `release.yml` workflows (attaching NSIS installer, `smartmontools 7.5` GPLv2+ source archive, and `SHA256SUMS.txt`).

### Changed
- Removed legacy Python v0.4 scaffolding and internal research artifacts into `_archive/` (gitignored).
- Replaced private serial fixtures with synthetic `smart-{0,1,2}.json` demo snapshots.
- Cleaned all `"AI"` promotional badges and legacy third-party naming labels across frontend views and TypeScript helpers.
- Fixed tooltip formatting so unknown metrics render `—` instead of `null`.
